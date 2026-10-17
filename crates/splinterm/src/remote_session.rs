//! One-authentication OpenSSH session for remote graphical daemon channels.

use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    io::IsTerminal as _,
    os::unix::fs::PermissionsExt as _,
    path::Path,
    pin::Pin,
    process::Stdio,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use splinterm_automation_client::Connection;
use splinterm_graphical_relay::{ClientMultiplexer, LogicalChannel};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    process::{Child, Command},
    sync::watch,
};
use tokio_util::sync::CancellationToken;

use crate::{
    remote::RemoteProfile,
    remote_askpass::{self as askpass, AskpassBroker, NativePrompt},
};

const MAX_SSH_DIAGNOSTIC_BYTES: usize = 16 * 1024;
const CHILD_GRACE_PERIOD: Duration = Duration::from_millis(250);
const AUTHENTICATION_GRACE_PERIOD: Duration = Duration::from_secs(120);
/// The most time negotiation may spend paused on native prompts in total, so
/// sequential questions each get their full prompt timeout while a helper that
/// keeps asking still ends.
const MAX_NATIVE_PROMPT_PAUSE: Duration = Duration::from_secs(10 * 60);

/// Stable high-level classes for remote startup and transport failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteFailureKind {
    UnknownHostKey,
    ChangedHostKey,
    RoutingOrTimeout,
    AuthenticationFailed,
    InteractiveAuthenticationUnavailable,
    RemoteCommandUnavailable,
    RemoteDaemonUnavailable,
    RelayIdentityRejected,
    MultiplexerIncompatible,
    DaemonProtocolIncompatible,
    TransportFailed,
}

impl std::fmt::Display for RemoteFailureKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::UnknownHostKey => "unknown host key",
            Self::ChangedHostKey => "changed host key",
            Self::RoutingOrTimeout => "routing or connection timeout",
            Self::AuthenticationFailed => "SSH authentication failed",
            Self::InteractiveAuthenticationUnavailable => {
                "interactive SSH authentication is unavailable"
            }
            Self::RemoteCommandUnavailable => "remote Splinterm command is unavailable",
            Self::RemoteDaemonUnavailable => "remote splinterd is unavailable",
            Self::RelayIdentityRejected => "remote relay or daemon identity validation failed",
            Self::MultiplexerIncompatible => "graphical relay protocol is incompatible",
            Self::DaemonProtocolIncompatible => "private daemon protocol is incompatible",
            Self::TransportFailed => "remote transport failed",
        };
        formatter.write_str(label)
    }
}

/// A categorized, bounded remote-session failure.
#[derive(Debug)]
pub struct RemoteFailure {
    kind: RemoteFailureKind,
    diagnostic: String,
}

impl RemoteFailure {
    /// Builds a categorized failure; the diagnostic is sanitized and bounded.
    #[must_use]
    pub fn new(kind: RemoteFailureKind, diagnostic: &str) -> Self {
        Self {
            kind,
            diagnostic: bounded_failure_message(diagnostic),
        }
    }

    /// Returns the stable failure category.
    #[must_use]
    pub const fn kind(&self) -> RemoteFailureKind {
        self.kind
    }

    /// Returns the bounded, sanitized OpenSSH and relay diagnostic text.
    #[must_use]
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

impl std::fmt::Display for RemoteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.diagnostic.is_empty() {
            write!(formatter, "{}", self.kind)
        } else {
            write!(formatter, "{}: {}", self.kind, self.diagnostic)
        }
    }
}

impl std::error::Error for RemoteFailure {}

/// A local OpenSSH setup problem found before OpenSSH runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalSshProblem {
    /// `SSH_ASKPASS` is set but does not name an absolute executable file.
    InvalidAskpass,
    /// The OpenSSH client executable could not be started.
    OpenSshUnavailable,
}

/// A local setup failure; no connection was attempted.
#[derive(Debug)]
pub struct LocalSshFailure {
    problem: LocalSshProblem,
    message: String,
}

impl LocalSshFailure {
    /// Builds a local setup failure; the message is sanitized and bounded.
    #[must_use]
    pub fn new(problem: LocalSshProblem, message: &str) -> Self {
        Self {
            problem,
            message: bounded_failure_message(message),
        }
    }

    /// Returns which local setup step failed.
    #[must_use]
    pub const fn problem(&self) -> LocalSshProblem {
        self.problem
    }

    /// Returns the bounded, sanitized message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for LocalSshFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LocalSshFailure {}

#[derive(Debug, Default)]
struct DiagnosticRing {
    bytes: VecDeque<u8>,
    truncated: bool,
}

impl DiagnosticRing {
    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let byte = if byte == b'\n' || byte == b'\t' || !byte.is_ascii_control() {
                byte
            } else {
                b' '
            };
            if self.bytes.len() == MAX_SSH_DIAGNOSTIC_BYTES {
                self.bytes.pop_front();
                self.truncated = true;
            }
            self.bytes.push_back(byte);
        }
    }

    fn render(&self) -> String {
        let bytes = self.bytes.iter().copied().collect::<Vec<_>>();
        let value = String::from_utf8_lossy(&bytes)
            .chars()
            .map(sanitize_diagnostic_character)
            .collect::<String>();
        let value = value.trim();
        if self.truncated && !value.is_empty() {
            format!("…{value}")
        } else {
            value.to_owned()
        }
    }
}

#[derive(Debug)]
struct RemoteLifetime {
    cancellation: CancellationToken,
}

impl Drop for RemoteLifetime {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

/// One authenticated OpenSSH child and its negotiated graphical multiplexer.
#[derive(Clone, Debug)]
pub struct RemoteSession {
    multiplexer: ClientMultiplexer,
    diagnostics: Arc<Mutex<DiagnosticRing>>,
    lifetime: Arc<RemoteLifetime>,
    terminal: bool,
    askpass_available: bool,
    operation_timeout: Duration,
}

impl RemoteSession {
    /// Starts OpenSSH from a validated profile and negotiates the graphical relay.
    ///
    /// # Errors
    ///
    /// Returns a categorized failure when process construction, SSH, relay
    /// negotiation, or lifecycle setup fails.
    pub async fn connect(profile: &RemoteProfile) -> Result<Self> {
        Self::connect_with_program(profile, OsStr::new("ssh")).await
    }

    #[doc(hidden)]
    pub async fn connect_with_program(profile: &RemoteProfile, program: &OsStr) -> Result<Self> {
        let negotiation_timeout = Duration::from_secs(u64::from(profile.connect_timeout_seconds()))
            .saturating_add(AUTHENTICATION_GRACE_PERIOD);
        Self::connect_with_program_and_timeout(profile, program, negotiation_timeout).await
    }

    /// Like [`Self::connect`], but when OpenSSH needs a password, passphrase,
    /// or confirmation and neither a terminal nor the user's `SSH_ASKPASS` is
    /// available, it asks through `prompt` (the status Window).
    ///
    /// # Errors
    ///
    /// Returns a categorized failure as [`Self::connect`] does.
    pub async fn connect_with_prompt(
        profile: &RemoteProfile,
        prompt: &NativePrompt,
    ) -> Result<Self> {
        let negotiation_timeout = Duration::from_secs(u64::from(profile.connect_timeout_seconds()))
            .saturating_add(AUTHENTICATION_GRACE_PERIOD);
        Self::connect_with_options(
            profile,
            OsStr::new("ssh"),
            negotiation_timeout,
            &AuthEnvironment::current(),
            Some(prompt),
        )
        .await
    }

    #[doc(hidden)]
    pub async fn connect_with_program_and_timeout(
        profile: &RemoteProfile,
        program: &OsStr,
        negotiation_timeout: Duration,
    ) -> Result<Self> {
        Self::connect_with_options(
            profile,
            program,
            negotiation_timeout,
            &AuthEnvironment::current(),
            None,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn connect_with_options(
        profile: &RemoteProfile,
        program: &OsStr,
        negotiation_timeout: Duration,
        environment: &AuthEnvironment,
        native_prompt: Option<&NativePrompt>,
    ) -> Result<Self> {
        if negotiation_timeout.is_zero() {
            bail!("remote relay negotiation timeout must be nonzero");
        }
        let terminal = environment.terminal;
        let (provider, broker) = prepare_prompt(environment, native_prompt)?;
        let askpass_available = matches!(
            provider,
            PromptProvider::UserAskpass | PromptProvider::NativePrompt
        );
        let mut child = ssh_command(profile, program, provider, environment, broker.as_ref())
            .spawn()
            .map_err(|error| {
                LocalSshFailure::new(
                    LocalSshProblem::OpenSshUnavailable,
                    &format!(
                        "cannot launch OpenSSH executable {}: {error}",
                        program.to_string_lossy()
                    ),
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .context("OpenSSH child stdin pipe is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("OpenSSH child stdout pipe is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("OpenSSH child stderr pipe is unavailable")?;
        // Serves prompts only from this OpenSSH child's descendants, and only
        // until negotiation ends. Dropping it unreleased (a failure, or the
        // status Window dropping this attempt) SIGKILLs OpenSSH before closing
        // any open prompt, so OpenSSH never reads a refusal as an empty password.
        let askpass_service = broker.and_then(|broker| child.id().map(|pid| broker.serve(pid)));
        let diagnostics = Arc::new(Mutex::new(DiagnosticRing::default()));
        let stderr_diagnostics = diagnostics.clone();
        let stderr_task = tokio::spawn(async move {
            drain_stderr(stderr, stderr_diagnostics).await;
        });

        // Time a native prompt waits on the user does not count against the
        // negotiation deadline: each sequential question gets its own full
        // prompt timeout from the status Window.
        let prompt_open = askpass_service
            .as_ref()
            .map(askpass::AskpassService::prompt_open);
        let multiplexer = match timeout_outside_prompts(
            negotiation_timeout,
            MAX_NATIVE_PROMPT_PAUSE,
            prompt_open,
            ClientMultiplexer::negotiate(stdout, stdin),
        )
        .await
        {
            Some(Ok(multiplexer)) => multiplexer,
            Some(Err(error)) => {
                terminate_child(&mut child).await;
                // Ends a helper still waiting on the socket, so it releases
                // the inherited stderr pipe. OpenSSH is already dead.
                drop(askpass_service);
                let _ = stderr_task.await;
                let diagnostic = rendered_diagnostics(&diagnostics);
                return Err(anyhow::Error::new(RemoteFailure {
                    kind: classify_failure(
                        &format!("{error:#}\n{diagnostic}"),
                        terminal,
                        askpass_available,
                    ),
                    diagnostic: bounded_failure_message(&format!("{error:#}\n{diagnostic}")),
                }));
            }
            None => {
                terminate_child(&mut child).await;
                drop(askpass_service);
                let _ = stderr_task.await;
                return Err(anyhow::Error::new(RemoteFailure {
                    kind: RemoteFailureKind::TransportFailed,
                    diagnostic: "SSH authentication or graphical relay negotiation timed out"
                        .to_owned(),
                }));
            }
        };

        // Authenticated: stop serving prompts without killing OpenSSH.
        if let Some(service) = askpass_service {
            service.release();
        }
        let cancellation = CancellationToken::new();
        let lifetime = Arc::new(RemoteLifetime {
            cancellation: cancellation.clone(),
        });
        tokio::spawn(supervise_child(child, cancellation, stderr_task));
        Ok(Self {
            multiplexer,
            diagnostics,
            lifetime,
            terminal,
            askpass_available,
            operation_timeout: Duration::from_secs(u64::from(profile.connect_timeout_seconds())),
        })
    }

    /// Opens and negotiates one independent human-interactive daemon connection.
    ///
    /// # Errors
    ///
    /// Returns an error when channel admission or the private daemon handshake
    /// fails. The connection never receives local compositor or image authority.
    pub async fn connect_interactive(&self) -> Result<Connection> {
        let deadline = tokio::time::Instant::now() + self.operation_timeout;
        let channel = if let Ok(result) =
            tokio::time::timeout_at(deadline, self.multiplexer.open_channel()).await
        {
            result.map_err(|error| {
                let diagnostic = rendered_diagnostics(&self.diagnostics);
                anyhow::Error::new(RemoteFailure {
                    kind: classify_failure(
                        &format!("{error:#}\n{diagnostic}"),
                        self.terminal,
                        self.askpass_available,
                    ),
                    diagnostic: bounded_failure_message(&format!("{error:#}\n{diagnostic}")),
                })
            })?
        } else {
            let stage = self
                .multiplexer
                .terminal_failure()
                .unwrap_or_else(|| "graphical relay channel admission was cancelled".to_owned());
            return Err(anyhow::Error::new(RemoteFailure {
                kind: RemoteFailureKind::TransportFailed,
                diagnostic: format!("remote logical channel admission timed out: {stage}"),
            }));
        };
        let channel_id = channel.channel_id();
        let channel = SessionChannel {
            inner: channel,
            _lifetime: self.lifetime.clone(),
        };
        let (reader, writer) = tokio::io::split(channel);
        match tokio::time::timeout_at(
            deadline,
            Connection::connect_remote_interactive_transport(reader, writer),
        )
        .await
        {
            Ok(result) => result.map_err(|error| {
                let cause = format!("{error:#}");
                let text = format!(
                    "remote private daemon handshake failed on logical channel {channel_id}: {cause}"
                );
                let lower = cause.to_ascii_lowercase();
                let kind = if lower.contains("incompatibleversion")
                    || lower.contains("incompatible version")
                    || lower.contains("invalid handshake")
                {
                    RemoteFailureKind::DaemonProtocolIncompatible
                } else if lower.contains("unauthorized")
                    || lower.contains("installed graphical relay")
                {
                    RemoteFailureKind::RelayIdentityRejected
                } else {
                    RemoteFailureKind::TransportFailed
                };
                anyhow::Error::new(RemoteFailure {
                    kind,
                    diagnostic: bounded_failure_message(&text),
                })
            }),
            Err(_) => Err(anyhow::Error::new(RemoteFailure {
                kind: RemoteFailureKind::TransportFailed,
                diagnostic: format!(
                    "remote private daemon Hello timed out on logical channel {channel_id}"
                ),
            })),
        }
    }

    /// Returns bounded sanitized SSH diagnostics retained for this session.
    #[must_use]
    pub fn diagnostics(&self) -> String {
        rendered_diagnostics(&self.diagnostics)
    }

    /// Returns the terminal multiplexer failure, if one has occurred.
    #[must_use]
    pub fn terminal_failure(&self) -> Option<String> {
        self.multiplexer.terminal_failure()
    }
}

#[derive(Debug)]
struct SessionChannel {
    inner: LogicalChannel,
    _lifetime: Arc<RemoteLifetime>,
}

impl AsyncRead for SessionChannel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncWrite for SessionChannel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

/// Selects the prompt provider and, for the native prompt, binds this
/// attempt's private broker.
fn prepare_prompt(
    environment: &AuthEnvironment,
    native_prompt: Option<&NativePrompt>,
) -> Result<(PromptProvider, Option<AskpassBroker>)> {
    let provider =
        select_prompt_provider(environment, native_prompt.is_some()).map_err(|error| {
            LocalSshFailure::new(LocalSshProblem::InvalidAskpass, &format!("{error:#}"))
        })?;
    let broker = match (provider, native_prompt) {
        // Without a private socket nothing can prompt; agent keys may still
        // authenticate, and a needed prompt is then reported as unavailable.
        (PromptProvider::NativePrompt, Some(prompt)) => AskpassBroker::bind(prompt).ok(),
        _ => None,
    };
    if provider == PromptProvider::NativePrompt && broker.is_none() {
        return Ok((PromptProvider::Unavailable, None));
    }
    Ok((provider, broker))
}

/// Builds the OpenSSH command with piped protocol stdio and only the askpass
/// environment the selected provider needs.
fn ssh_command(
    profile: &RemoteProfile,
    program: &OsStr,
    provider: PromptProvider,
    environment: &AuthEnvironment,
    broker: Option<&AskpassBroker>,
) -> Command {
    let plan = profile.ssh_plan();
    let mut command = Command::new(program);
    command
        .args(plan.arguments())
        .env_remove(askpass::SOCKET_ENV)
        .env_remove(askpass::TOKEN_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if provider == PromptProvider::UserAskpass
        && let Some(user_askpass) = &environment.user_askpass
    {
        // Exactly the program that was validated.
        command
            .env("SSH_ASKPASS", user_askpass)
            .env("SSH_ASKPASS_REQUIRE", "force");
    }
    if let Some(broker) = broker {
        command.envs(broker.environment());
    }
    command
}

/// Who answers OpenSSH when it needs a password, passphrase, or confirmation.
/// OpenSSH always tries ssh-agent keys first and asks only when they fail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptProvider {
    /// Stdin is a terminal; OpenSSH may prompt on it and nothing is forced.
    Terminal,
    /// The user's own `SSH_ASKPASS`, forced with `SSH_ASKPASS_REQUIRE=force`.
    UserAskpass,
    /// Splinterm's prompt in the status Window, used only when neither of the
    /// above is available.
    NativePrompt,
    /// Nothing can ask; only agent or other non-interactive methods can work.
    Unavailable,
}

/// The process facts that select a [`PromptProvider`].
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthEnvironment {
    pub terminal: bool,
    pub user_askpass: Option<OsString>,
}

impl AuthEnvironment {
    /// Reads whether stdin is a terminal and the inherited `SSH_ASKPASS`.
    #[must_use]
    pub fn current() -> Self {
        Self {
            terminal: std::io::stdin().is_terminal(),
            user_askpass: std::env::var_os("SSH_ASKPASS"),
        }
    }
}

/// Chooses the prompt provider: a terminal, then the user's `SSH_ASKPASS`,
/// then the native prompt when the caller offers one.
///
/// # Errors
///
/// Returns an error when, without a terminal, `SSH_ASKPASS` is set but does
/// not name an absolute executable file. That explicit setting is reported,
/// never silently replaced by the native prompt.
pub fn select_prompt_provider(
    environment: &AuthEnvironment,
    native_prompt_offered: bool,
) -> Result<PromptProvider> {
    if environment.terminal {
        return Ok(PromptProvider::Terminal);
    }
    if validate_askpass_value(false, environment.user_askpass.as_deref())? {
        return Ok(PromptProvider::UserAskpass);
    }
    Ok(if native_prompt_offered {
        PromptProvider::NativePrompt
    } else {
        PromptProvider::Unavailable
    })
}

fn validate_askpass_value(terminal: bool, value: Option<&OsStr>) -> Result<bool> {
    if terminal {
        return Ok(false);
    }
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(false);
    };
    let path = Path::new(value);
    if !path.is_absolute() {
        bail!("SSH_ASKPASS must name an absolute local executable");
    }
    let metadata = path
        .metadata()
        .with_context(|| format!("cannot inspect SSH_ASKPASS executable {}", path.display()))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        bail!(
            "SSH_ASKPASS is not an executable local file: {}",
            path.display()
        );
    }
    Ok(true)
}

async fn drain_stderr<R>(mut stderr: R, diagnostics: Arc<Mutex<DiagnosticRing>>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 1024];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                if let Ok(mut diagnostics) = diagnostics.lock() {
                    diagnostics.push(&buffer[..count]);
                }
            }
        }
    }
}

async fn supervise_child(
    mut child: Child,
    cancellation: CancellationToken,
    mut stderr_task: tokio::task::JoinHandle<()>,
) {
    tokio::select! {
        _ = child.wait() => {}
        () = cancellation.cancelled() => {
            if tokio::time::timeout(CHILD_GRACE_PERIOD, child.wait()).await.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
    }
    if tokio::time::timeout(CHILD_GRACE_PERIOD, &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
        let _ = stderr_task.await;
    }
}

/// Awaits `future` for at most `budget` of time spent outside open native
/// prompts. While `prompt_open` reads true the budget stops running, and that
/// paused time is itself bounded by `max_pause` in total. Returns `None` when
/// either bound is exhausted first.
async fn timeout_outside_prompts<T>(
    budget: Duration,
    max_pause: Duration,
    prompt_open: Option<watch::Receiver<bool>>,
    future: impl std::future::Future<Output = T>,
) -> Option<T> {
    let Some(mut prompt_open) = prompt_open else {
        return tokio::time::timeout(budget, future).await.ok();
    };
    tokio::pin!(future);
    let mut remaining = budget;
    let mut pause_left = max_pause;
    loop {
        let open = *prompt_open.borrow_and_update();
        let started = tokio::time::Instant::now();
        let limit = if open { pause_left } else { remaining };
        tokio::select! {
            output = &mut future => return Some(output),
            () = tokio::time::sleep(limit) => return None,
            changed = prompt_open.changed() => {
                let spent = started.elapsed();
                if open {
                    pause_left = pause_left.saturating_sub(spent);
                } else {
                    remaining = remaining.saturating_sub(spent);
                }
                if changed.is_err() {
                    // No more prompts can open; finish on the plain budget.
                    return tokio::time::timeout(remaining, future).await.ok();
                }
            }
        }
    }
}

async fn terminate_child(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn rendered_diagnostics(diagnostics: &Mutex<DiagnosticRing>) -> String {
    diagnostics.lock().map_or_else(
        |_| "SSH diagnostic buffer is unavailable".to_owned(),
        |ring| ring.render(),
    )
}

pub(crate) fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn sanitize_diagnostic_character(character: char) -> char {
    if is_bidi_control(character) || character.is_control() && !matches!(character, '\n' | '\t') {
        ' '
    } else {
        character
    }
}

fn bounded_failure_message(value: &str) -> String {
    value
        .chars()
        .map(sanitize_diagnostic_character)
        .take(2048)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn classify_failure(text: &str, terminal: bool, askpass_available: bool) -> RemoteFailureKind {
    let lower = text.to_ascii_lowercase();
    if lower.contains("remote host identification has changed") {
        RemoteFailureKind::ChangedHostKey
    } else if lower.contains("host key verification failed")
        || lower.contains("no host key is known")
    {
        RemoteFailureKind::UnknownHostKey
    } else if lower.contains("connection timed out")
        || lower.contains("could not resolve hostname")
        || lower.contains("no route to host")
        || lower.contains("connection refused")
    {
        RemoteFailureKind::RoutingOrTimeout
    } else if !terminal
        && !askpass_available
        && (lower.contains("can't open /dev/tty")
            || lower.contains("cannot open /dev/tty")
            || lower.contains("askpass"))
    {
        RemoteFailureKind::InteractiveAuthenticationUnavailable
    } else if lower.contains("permission denied")
        || lower.contains("no supported authentication methods")
    {
        RemoteFailureKind::AuthenticationFailed
    } else if lower.contains("command not found")
        || lower.contains("no such file or directory") && lower.contains("splinterm")
    {
        RemoteFailureKind::RemoteCommandUnavailable
    } else if lower.contains("cannot connect to the validated daemon socket")
        || lower.contains("xdg_runtime_dir is unset")
    {
        RemoteFailureKind::RemoteDaemonUnavailable
    } else if lower.contains("adjacent splinterd")
        || lower.contains("daemon endpoint")
        || lower.contains("daemon socket")
    {
        RemoteFailureKind::RelayIdentityRejected
    } else if lower.contains("graphical relay version")
        || lower.contains("invalid handshake")
        || lower.contains("magic is invalid")
    {
        RemoteFailureKind::MultiplexerIncompatible
    } else {
        RemoteFailureKind::TransportFailed
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, time::SystemTime};

    use super::*;

    #[test]
    fn failure_categories_are_specific_and_diagnostics_are_bounded() {
        assert_eq!(
            classify_failure("REMOTE HOST IDENTIFICATION HAS CHANGED", true, false),
            RemoteFailureKind::ChangedHostKey
        );
        assert_eq!(
            classify_failure("Host key verification failed", true, false),
            RemoteFailureKind::UnknownHostKey
        );
        assert_eq!(
            classify_failure("Permission denied (publickey,password)", true, false),
            RemoteFailureKind::AuthenticationFailed
        );
        assert_eq!(
            classify_failure("read_passphrase: can't open /dev/tty", false, false),
            RemoteFailureKind::InteractiveAuthenticationUnavailable
        );
        let mut ring = DiagnosticRing::default();
        ring.push(&vec![b'x'; MAX_SSH_DIAGNOSTIC_BYTES + 100]);
        assert!(ring.truncated);
        assert!(ring.render().len() <= MAX_SSH_DIAGNOSTIC_BYTES + '…'.len_utf8());
        let mut bidi = DiagnosticRing::default();
        bidi.push("safe\u{202e}spoof".as_bytes());
        assert_eq!(bidi.render(), "safe spoof");
        assert_eq!(bounded_failure_message("a\u{2066}b"), "a b");
    }

    #[test]
    fn askpass_requires_an_absolute_executable_file() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("splinterm-askpass-{nonce}"));
        fs::create_dir(&directory).unwrap();
        let executable = directory.join("askpass");
        fs::write(&executable, "fixture").unwrap();
        fs::set_permissions(
            &executable,
            <fs::Permissions as PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        assert!(validate_askpass_value(false, Some(executable.as_os_str())).unwrap());
        assert!(!validate_askpass_value(true, Some(executable.as_os_str())).unwrap());
        assert!(validate_askpass_value(false, Some(OsStr::new("relative"))).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn prompt_provider_prefers_terminal_then_user_askpass_then_native() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("splinterm-provider-{nonce}"));
        fs::create_dir(&directory).unwrap();
        let executable = directory.join("askpass");
        fs::write(&executable, "fixture").unwrap();
        fs::set_permissions(
            &executable,
            <fs::Permissions as PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        let environment = |terminal, askpass: Option<&std::path::Path>| AuthEnvironment {
            terminal,
            user_askpass: askpass.map(|path| path.as_os_str().to_owned()),
        };
        let select = |environment: &AuthEnvironment, native| {
            select_prompt_provider(environment, native).unwrap()
        };

        // A terminal wins: OpenSSH prompts there and nothing is forced, even
        // with a user askpass or a native prompt on offer.
        for native in [false, true] {
            assert_eq!(
                select(&environment(true, Some(&executable)), native),
                PromptProvider::Terminal
            );
            assert_eq!(
                select(&environment(true, None), native),
                PromptProvider::Terminal
            );
        }
        // The user's own SSH_ASKPASS is preferred over the native prompt.
        for native in [false, true] {
            assert_eq!(
                select(&environment(false, Some(&executable)), native),
                PromptProvider::UserAskpass
            );
        }
        // Otherwise the native prompt, only when the caller offers one.
        assert_eq!(
            select(&environment(false, None), true),
            PromptProvider::NativePrompt
        );
        assert_eq!(
            select(&environment(false, None), false),
            PromptProvider::Unavailable
        );
        let empty = AuthEnvironment {
            terminal: false,
            user_askpass: Some(OsString::new()),
        };
        assert_eq!(select(&empty, true), PromptProvider::NativePrompt);
        // An explicit but broken SSH_ASKPASS is reported, never replaced.
        let broken = AuthEnvironment {
            terminal: false,
            user_askpass: Some(OsString::from("relative-askpass")),
        };
        assert!(select_prompt_provider(&broken, true).is_err());
        let missing = environment(false, Some(&directory.join("missing")));
        assert!(select_prompt_provider(&missing, true).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_prompt_failures_classify_as_authentication_not_unavailable() {
        // With a native or user prompt available, a refused prompt surfaces as
        // OpenSSH's own authentication failure.
        assert_eq!(
            classify_failure("Permission denied (publickey,password).", false, true),
            RemoteFailureKind::AuthenticationFailed
        );
        assert_eq!(
            classify_failure(
                "splinterm askpass: the Splinterm prompt is unavailable\nPermission denied (password).",
                false,
                true
            ),
            RemoteFailureKind::AuthenticationFailed
        );
    }

    /// Drives the prompt watch through `phases` of (prompt open, seconds), then
    /// completes the negotiation stand-in.
    fn scripted_negotiation(
        phases: &'static [(bool, u64)],
    ) -> (
        watch::Receiver<bool>,
        impl std::future::Future<Output = &'static str>,
    ) {
        let (sender, receiver) = watch::channel(false);
        let negotiation = async move {
            for &(open, seconds) in phases {
                sender.send_replace(open);
                tokio::time::sleep(Duration::from_secs(seconds)).await;
            }
            "negotiated"
        };
        (receiver, negotiation)
    }

    const BUDGET: Duration = Duration::from_secs(10);

    #[tokio::test(start_paused = true)]
    async fn each_sequential_native_prompt_gets_its_full_timeout() {
        // A key passphrase and then a password, each answered after 100 s, with
        // 9 s of OpenSSH work in all: far past a fixed 10 s deadline.
        let (prompt_open, negotiation) =
            scripted_negotiation(&[(false, 4), (true, 100), (false, 4), (true, 100), (false, 1)]);
        let started = tokio::time::Instant::now();
        assert_eq!(
            timeout_outside_prompts(
                BUDGET,
                MAX_NATIVE_PROMPT_PAUSE,
                Some(prompt_open),
                negotiation
            )
            .await,
            Some("negotiated")
        );
        assert_eq!(started.elapsed(), Duration::from_secs(209));
    }

    #[tokio::test(start_paused = true)]
    async fn time_outside_prompts_still_exhausts_the_negotiation_deadline() {
        let (prompt_open, negotiation) =
            scripted_negotiation(&[(false, 4), (true, 100), (false, 60)]);
        let started = tokio::time::Instant::now();
        assert_eq!(
            timeout_outside_prompts(
                BUDGET,
                MAX_NATIVE_PROMPT_PAUSE,
                Some(prompt_open),
                negotiation
            )
            .await,
            None
        );
        // 4 s before the prompt plus the remaining 6 s after it.
        assert_eq!(started.elapsed(), Duration::from_secs(110));
    }

    #[tokio::test(start_paused = true)]
    async fn paused_prompt_time_has_an_overall_cap() {
        let (prompt_open, negotiation) =
            scripted_negotiation(&[(true, 300), (false, 1), (true, 400), (false, 1)]);
        let started = tokio::time::Instant::now();
        let cap = Duration::from_secs(600);
        assert_eq!(
            timeout_outside_prompts(BUDGET, cap, Some(prompt_open), negotiation).await,
            None
        );
        // 300 s + 1 s + the last 300 s of pause allowance.
        assert_eq!(started.elapsed(), Duration::from_secs(601));
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_native_prompt_the_deadline_is_fixed() {
        let started = tokio::time::Instant::now();
        assert_eq!(
            timeout_outside_prompts(
                BUDGET,
                MAX_NATIVE_PROMPT_PAUSE,
                None,
                tokio::time::sleep(Duration::from_secs(11)),
            )
            .await,
            None
        );
        assert_eq!(started.elapsed(), BUDGET);

        // A prompt watch whose broker is gone falls back to the same budget.
        let (sender, receiver) = watch::channel(false);
        drop(sender);
        assert_eq!(
            timeout_outside_prompts(BUDGET, MAX_NATIVE_PROMPT_PAUSE, Some(receiver), async {
                tokio::time::sleep(Duration::from_secs(9)).await;
                "negotiated"
            },)
            .await,
            Some("negotiated")
        );
    }
}
