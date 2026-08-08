//! One-authentication OpenSSH session for remote graphical daemon channels.

use std::{
    collections::VecDeque,
    ffi::OsStr,
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
};
use tokio_util::sync::CancellationToken;

use crate::remote::RemoteProfile;

const MAX_SSH_DIAGNOSTIC_BYTES: usize = 16 * 1024;
const CHILD_GRACE_PERIOD: Duration = Duration::from_millis(250);
const AUTHENTICATION_GRACE_PERIOD: Duration = Duration::from_secs(120);

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
    PolicyDenied,
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
            Self::PolicyDenied => "remote Splinterm policy denied the operation",
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
    /// Returns the stable failure category.
    #[must_use]
    pub const fn kind(&self) -> RemoteFailureKind {
        self.kind
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

    #[doc(hidden)]
    pub async fn connect_with_program_and_timeout(
        profile: &RemoteProfile,
        program: &OsStr,
        negotiation_timeout: Duration,
    ) -> Result<Self> {
        if negotiation_timeout.is_zero() {
            bail!("remote relay negotiation timeout must be nonzero");
        }
        let terminal = std::io::stdin().is_terminal();
        let askpass_available = validate_askpass(terminal)?;
        let plan = profile.ssh_plan();
        let mut command = Command::new(program);
        command
            .args(plan.arguments())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !terminal && askpass_available {
            command.env("SSH_ASKPASS_REQUIRE", "force");
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "cannot launch OpenSSH executable {}",
                program.to_string_lossy()
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
        let diagnostics = Arc::new(Mutex::new(DiagnosticRing::default()));
        let stderr_diagnostics = diagnostics.clone();
        let stderr_task = tokio::spawn(async move {
            drain_stderr(stderr, stderr_diagnostics).await;
        });

        let multiplexer = match tokio::time::timeout(
            negotiation_timeout,
            ClientMultiplexer::negotiate(stdout, stdin),
        )
        .await
        {
            Ok(Ok(multiplexer)) => multiplexer,
            Ok(Err(error)) => {
                terminate_child(&mut child).await;
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
            Err(_) => {
                terminate_child(&mut child).await;
                let _ = stderr_task.await;
                return Err(anyhow::Error::new(RemoteFailure {
                    kind: RemoteFailureKind::TransportFailed,
                    diagnostic: "SSH authentication or graphical relay negotiation timed out"
                        .to_owned(),
                }));
            }
        };

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

    /// Opens and negotiates one independent automation-role daemon connection.
    ///
    /// # Errors
    ///
    /// Returns an error when channel admission or the private daemon handshake
    /// fails. The connection never receives trusted image authority.
    pub async fn connect_automation(&self) -> Result<Connection> {
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
            Connection::connect_automation_transport(reader, writer),
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
                } else if lower.contains("unauthorized") || lower.contains("policy") {
                    RemoteFailureKind::PolicyDenied
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

fn validate_askpass(terminal: bool) -> Result<bool> {
    let value = std::env::var_os("SSH_ASKPASS");
    validate_askpass_value(terminal, value.as_deref())
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

fn is_bidi_control(character: char) -> bool {
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
    use std::{fs, os::unix::fs::PermissionsExt as _, time::SystemTime};

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
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_askpass_value(false, Some(executable.as_os_str())).unwrap());
        assert!(!validate_askpass_value(true, Some(executable.as_os_str())).unwrap());
        assert!(validate_askpass_value(false, Some(OsStr::new("relative"))).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
