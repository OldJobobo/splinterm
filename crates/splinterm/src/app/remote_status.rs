//! Remote connection-status Window: connect, report, retry, or hand over.

use std::{future::Future, io::IsTerminal as _, time::Duration};

use anyhow::{Context, Result, bail};
use splinterm::{
    WindowCommand, WindowOptions, WindowSurface, WindowUpdate,
    config::{AppConfig, ResolvedTheme},
    diagnostics::{DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, global},
    endpoint::ConnectionFactory,
    remote::{RemoteCatalog, RemoteProfile},
    remote_askpass::{
        self as askpass, AskpassKind, AskpassReply, AskpassRequest, FieldOutcome, NativePrompt,
        Secret, apply_secret_input, confirm_key,
    },
    remote_launch::destination,
    remote_session::RemoteFailureKind,
    remote_status::{
        RemoteFailureCategory, RemoteProfileError, RemoteStatus, RemoteStatusEffect,
        RemoteStatusFailure, profile_label, status_key,
    },
};
use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::{Lair, SplintId};
use splinterm_protocol::{Request, Response, TerminalSnapshot};
use tokio::sync::{mpsc, oneshot};

use super::{
    sessions::configure_picker_renderer,
    theme_watch::load_startup_theme,
    window::{resolved_window_app_id, run_window_phase},
};

const STATUS_UPDATE_CAPACITY: usize = 8;
const STATUS_COMMAND_CAPACITY: usize = 64;
/// OpenSSH asks one question at a time; the broker serves one helper at a time.
const PROMPT_CAPACITY: usize = 1;
/// Detail shown when the user cancels a password prompt.
pub(in crate::app) const PROMPT_CANCELLED: &str = "The Splinterm password prompt was cancelled.";
/// Detail shown when a prompt was left unanswered.
pub(in crate::app) const PROMPT_TIMED_OUT: &str = "The Splinterm prompt was not answered in time.";

/// The status Window closed before a connection was made: after a shown
/// failure, by cancelling an attempt, or by the compositor. A direct
/// `splinterm --remote PROFILE` exits with status 1 so a terminal caller sees
/// that no remote Window opened; the remote launcher returns to its profiles.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct RemoteNotConnected {
    /// The category of the last failure shown, if any.
    pub(crate) last_failure: Option<DiagnosticErrorCode>,
    /// Whether the Window itself went away (the compositor or user closed
    /// it, or it never mapped) rather than a status key ending the attempt.
    pub(crate) window_closed: bool,
}

impl RemoteNotConnected {
    /// A status key (C, Q, or Escape) ended the attempt.
    #[cfg(test)]
    pub(crate) const fn cancelled(last_failure: Option<DiagnosticErrorCode>) -> Self {
        Self {
            last_failure,
            window_closed: false,
        }
    }

    /// The Window went away before connecting.
    #[cfg(test)]
    pub(crate) const fn window_gone(last_failure: Option<DiagnosticErrorCode>) -> Self {
        Self {
            last_failure,
            window_closed: true,
        }
    }

    /// The process exit status for a remote launch that never connected.
    pub(crate) const EXIT_CODE: i32 = 1;
}

impl std::fmt::Display for RemoteNotConnected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("remote connection was not opened")
    }
}

impl std::error::Error for RemoteNotConnected {}

/// The status screen's channel to its Window.
struct StatusWindow {
    splint_id: SplintId,
    revision: u64,
    updates: mpsc::Sender<WindowUpdate>,
    /// Fires once the Window has mapped; see [`WindowOptions::mapped`].
    mapped: oneshot::Receiver<()>,
}

impl StatusWindow {
    /// Waits until the Window maps. Until then the Wayland thread may drain
    /// queued updates before its first dispatch, so an early close could end
    /// the Window before it was ever shown. False when the Window ended
    /// without mapping.
    async fn wait_mapped(&mut self) -> bool {
        (&mut self.mapped).await.is_ok()
    }

    fn show(&mut self, status: &RemoteStatus) {
        self.revision = self.revision.saturating_add(1);
        // A full queue or a closed Window only loses an intermediate frame.
        let _ = self.updates.try_send(WindowUpdate::Snapshot {
            snapshot: status.snapshot(self.splint_id, self.revision),
            image_sources: ImageContentLeaseSet::default(),
            authoritative: true,
        });
    }

    async fn close(&self) {
        // The status snapshot's own Splint exiting closes the Window cleanly.
        let _ = self
            .updates
            .send(WindowUpdate::Exited {
                splint_id: self.splint_id,
            })
            .await;
    }
}

/// An OpenSSH prompt open in the status Window and its masked field.
struct ActivePrompt {
    request: AskpassRequest,
    field: Secret,
    /// When an unanswered secret or question ends the attempt.
    deadline: tokio::time::Instant,
}

/// What a key press on an open prompt asks the flow to do.
enum PromptInput {
    None,
    Redraw,
    /// Escape on a masked field: end the attempt, and only then drop the
    /// prompt unanswered. OpenSSH turns a refused prompt into an empty
    /// password and asks again, so it must be dead before the helper exits.
    CancelAttempt,
}

fn prompt_failure(detail: &str) -> RemoteStatusFailure {
    RemoteStatusFailure {
        category: RemoteFailureCategory::Remote(RemoteFailureKind::AuthenticationFailed),
        details: vec![detail.to_owned()],
    }
}

/// Applies one key batch to the open prompt. Answers move the masked field to
/// the broker, which hands it to OpenSSH and wipes it.
fn prompt_input(
    active: &mut Option<ActivePrompt>,
    status: &mut RemoteStatus,
    bytes: &[u8],
) -> PromptInput {
    let Some(prompt) = active.as_mut() else {
        return PromptInput::None;
    };
    match prompt.request.kind() {
        AskpassKind::Secret => match apply_secret_input(&mut prompt.field, bytes) {
            FieldOutcome::Editing => {
                let has_input = !prompt.field.is_empty();
                if status
                    .prompt()
                    .is_some_and(|shown| shown.has_input() == has_input)
                {
                    PromptInput::None
                } else {
                    status.set_prompt_input(has_input);
                    PromptInput::Redraw
                }
            }
            FieldOutcome::Submit => {
                let ActivePrompt { request, field, .. } = active.take().expect("guarded prompt");
                request.answer(AskpassReply::Submit(field));
                status.clear_prompt();
                PromptInput::Redraw
            }
            // Not answered here: the flow kills the attempt first.
            FieldOutcome::Cancel => PromptInput::CancelAttempt,
        },
        AskpassKind::Confirm => match confirm_key(bytes) {
            Some(yes) => {
                let prompt = active.take().expect("guarded prompt");
                prompt.request.answer(if yes {
                    AskpassReply::Confirm
                } else {
                    AskpassReply::Cancel
                });
                status.clear_prompt();
                PromptInput::Redraw
            }
            None => PromptInput::None,
        },
        // A notice takes no answer; the ordinary status keys apply.
        AskpassKind::Notice => PromptInput::None,
    }
}

/// Shows a prompt from the running attempt. A prompt that arrives with no
/// attempt running is refused; one already abandoned is skipped.
fn open_prompt(
    request: AskpassRequest,
    attempt_running: bool,
    status: &mut RemoteStatus,
    prompt_timeout: Duration,
) -> Option<ActivePrompt> {
    if request.is_abandoned() {
        return None;
    }
    if attempt_running && status.show_prompt(request.kind(), request.prompt_lines()) {
        Some(ActivePrompt {
            request,
            field: Secret::new(),
            deadline: tokio::time::Instant::now() + prompt_timeout,
        })
    } else {
        request.answer(AskpassReply::Cancel);
        None
    }
}

/// Reports a failed attempt, shows it, and returns its diagnostic code.
fn show_failure(
    status: &mut RemoteStatus,
    window: &mut StatusWindow,
    report: &mut impl FnMut(&RemoteStatusFailure),
    failure: RemoteStatusFailure,
) -> DiagnosticErrorCode {
    report(&failure);
    let code = failure.copy().diagnostic_code;
    if status.on_attempt(Err(failure)) == RemoteStatusEffect::ShowFailure {
        window.show(status);
    }
    code
}

/// What one Window command means to the flow.
enum CommandEffect {
    /// Input consumed by an open secret or question prompt.
    Prompt(PromptInput),
    Status(RemoteStatusEffect),
}

/// Classifies one Window command. An open secret or question takes key input
/// before the status keys; key bytes are wiped once used.
fn command_effect(
    command: Option<WindowCommand>,
    active: &mut Option<ActivePrompt>,
    status: &mut RemoteStatus,
) -> CommandEffect {
    let intercepted = active
        .as_ref()
        .is_some_and(|prompt| prompt.request.kind() != AskpassKind::Notice);
    match command {
        None => CommandEffect::Status(status.on_window_closed()),
        Some(WindowCommand::Input(mut bytes)) if intercepted => {
            let input = prompt_input(active, status, &bytes);
            askpass::wipe(&mut bytes);
            CommandEffect::Prompt(input)
        }
        Some(WindowCommand::Input(mut bytes)) => {
            let effect =
                status_key(&bytes).map_or(RemoteStatusEffect::None, |key| status.on_key(key));
            askpass::wipe(&mut bytes);
            CommandEffect::Status(effect)
        }
        Some(_) => CommandEffect::Status(RemoteStatusEffect::None),
    }
}

/// Drives one status screen until it hands over a connected endpoint, or the
/// Window closes first. The first attempt starts once the Window has mapped.
/// `connect` starts one attempt and returns the destination it connects to,
/// so the shown destination and the connection come from one profile load;
/// `report` records each failure; `prompts` delivers OpenSSH prompts from the
/// current attempt's private askpass broker, and an unanswered secret or
/// question ends the attempt after `prompt_timeout`. Nothing here writes to
/// stdout or stderr, and key input is wiped after use because it may be a
/// password.
#[allow(
    clippy::too_many_lines,
    reason = "one select loop orders cancellation, prompts, attempts, and hand-over"
)]
async fn run_status_flow<T, F, Fut>(
    mut status: RemoteStatus,
    mut window: StatusWindow,
    mut commands: mpsc::Receiver<WindowCommand>,
    mut prompts: mpsc::Receiver<AskpassRequest>,
    prompt_timeout: Duration,
    mut connect: F,
    mut report: impl FnMut(&RemoteStatusFailure),
) -> Result<T, RemoteNotConnected>
where
    F: FnMut() -> (String, Fut),
    Fut: Future<Output = Result<T>>,
{
    // A Window that ended unmapped has also closed `commands`, so the loop
    // ends the flow without starting an attempt.
    let mut attempt = if window.wait_mapped().await {
        // The profile may have changed while the Window mapped.
        let (destination, future) = connect();
        let shown = status.destination().to_owned();
        status.set_destination(&destination);
        if status.destination() != shown {
            window.show(&status);
        }
        Some(Box::pin(future))
    } else {
        None
    };
    let mut last_failure = None;
    let mut prompts_open = true;
    let mut active: Option<ActivePrompt> = None;
    loop {
        // Set by an arm that must end the attempt because of its prompt.
        let mut end_attempt: Option<&str> = None;
        let timed = active
            .as_ref()
            .filter(|prompt| prompt.request.kind() != AskpassKind::Notice)
            .map(|prompt| prompt.deadline);
        tokio::select! {
            // A cancellation queued alongside a finished attempt wins, so C,
            // Escape, or a compositor close is never overtaken by hand-over.
            biased;
            command = commands.recv() => {
                let window_closed = command.is_none();
                let effect = match command_effect(command, &mut active, &mut status) {
                    CommandEffect::Status(effect) => effect,
                    CommandEffect::Prompt(PromptInput::None) => RemoteStatusEffect::None,
                    CommandEffect::Prompt(PromptInput::Redraw) => {
                        window.show(&status);
                        RemoteStatusEffect::None
                    }
                    CommandEffect::Prompt(PromptInput::CancelAttempt) => {
                        end_attempt = Some(PROMPT_CANCELLED);
                        RemoteStatusEffect::None
                    }
                };
                match effect {
                    RemoteStatusEffect::StartAttempt => {
                        let (destination, future) = connect();
                        status.set_destination(&destination);
                        attempt = Some(Box::pin(future));
                        window.show(&status);
                    }
                    RemoteStatusEffect::Close => {
                        // Kill OpenSSH first (dropping the attempt SIGKILLs
                        // it), then drop the prompt unanswered and wipe it.
                        drop(attempt.take());
                        drop(active.take());
                        window.close().await;
                        return Err(RemoteNotConnected {
                            last_failure,
                            window_closed,
                        });
                    }
                    RemoteStatusEffect::None
                    | RemoteStatusEffect::ShowFailure
                    | RemoteStatusEffect::HandOver => {}
                }
            }
            () = async { tokio::time::sleep_until(timed.expect("guarded deadline")).await }, if timed.is_some() => {
                end_attempt = Some(PROMPT_TIMED_OUT);
            }
            result = async { attempt.as_mut().expect("guarded attempt").await }, if attempt.is_some() => {
                attempt = None;
                // The attempt's broker is gone; wipe any unanswered field.
                active = None;
                match result {
                    Ok(connected) => {
                        if status.on_attempt(Ok(())) == RemoteStatusEffect::HandOver {
                            window.close().await;
                            return Ok(connected);
                        }
                    }
                    Err(error) => {
                        let failure = RemoteStatusFailure::from_error(&error);
                        last_failure = Some(show_failure(&mut status, &mut window, &mut report, failure));
                    }
                }
            }
            request = prompts.recv(), if prompts_open && active.is_none() => {
                if let Some(request) = request {
                    active = open_prompt(request, attempt.is_some(), &mut status, prompt_timeout);
                    if active.is_some() {
                        window.show(&status);
                    }
                } else {
                    prompts_open = false;
                }
            }
            () = async { active.as_mut().expect("guarded prompt").request.abandoned().await }, if active.is_some() => {
                // OpenSSH gave up, the prompt timed out, or the helper died.
                active = None;
                status.clear_prompt();
                window.show(&status);
            }
        }
        if let Some(detail) = end_attempt {
            // Order matters: dropping the attempt SIGKILLs OpenSSH before the
            // prompt is dropped, so OpenSSH never reads a refusal.
            drop(attempt.take());
            drop(active.take());
            let failure = prompt_failure(detail);
            last_failure = Some(show_failure(&mut status, &mut window, &mut report, failure));
        }
    }
}

/// Loads `profile_name` from the current `remotes.toml`.
fn load_profile(profile_name: &str) -> Result<RemoteProfile> {
    RemoteCatalog::load_default()?.get(profile_name).cloned()
}

/// Starts one attempt from a single load of the profile: the destination to
/// show (the profile label when it cannot be loaded) and the connection.
fn start_attempt(
    profile_name: &str,
    prompt: Option<NativePrompt>,
) -> (
    String,
    impl Future<Output = Result<RemoteConnected>> + use<>,
) {
    let profile = load_profile(profile_name);
    let shown = profile
        .as_ref()
        .map_or_else(|_| profile_label(profile_name).to_owned(), destination);
    (shown, connect_profile(profile, prompt))
}

/// A connected remote endpoint and the Lairs it listed when it connected.
pub(in crate::app) struct RemoteConnected {
    pub(in crate::app) factory: ConnectionFactory,
    pub(in crate::app) lairs: Vec<Lair>,
}

/// One attempt: authenticate with the loaded profile and prove the remote
/// daemon answers and lists its Lairs (as `splinterm remote check` does), so
/// daemon and listing failures are reported here rather than after hand-over.
/// Only catalog errors are marked as profile errors; every later error keeps
/// its own category or is reported as a transport failure.
async fn connect_profile(
    profile: Result<RemoteProfile>,
    prompt: Option<NativePrompt>,
) -> Result<RemoteConnected> {
    let profile = profile.map_err(|error| RemoteProfileError::wrap(&error))?;
    let deadline = Duration::from_secs(u64::from(profile.connect_timeout_seconds()));
    let factory = match &prompt {
        Some(prompt) => ConnectionFactory::remote_with_prompt(&profile, prompt).await?,
        None => ConnectionFactory::remote(&profile).await?,
    };
    let mut connection = factory.connect().await?;
    if !matches!(
        connection
            .request_with_deadline(Request::Ping, deadline)
            .await?,
        Response::Pong
    ) {
        bail!("remote splinterd returned an invalid ping response");
    }
    let Response::Lairs { lairs, .. } = connection
        .request_with_deadline(Request::ListLairs, deadline)
        .await?
    else {
        bail!("remote splinterd returned an invalid topology response");
    };
    Ok(RemoteConnected { factory, lairs })
}

fn report_failure(failure: &RemoteStatusFailure) {
    // Structured category only: raw OpenSSH text stays in the status Window.
    if let Some(diagnostics) = global() {
        diagnostics.emit(
            DiagnosticLevel::Warn,
            DiagnosticEventCode::RemoteConnectFailure,
            Some(failure.copy().diagnostic_code),
        );
    }
}

/// The status Window's options. It keeps the default control so R, Enter, and
/// the password field reach the flow, but it controls no terminal, so its
/// title does not call it the local controller.
fn status_window_options(
    config: &AppConfig,
    theme: ResolvedTheme,
    snapshot: TerminalSnapshot,
    updates: mpsc::Receiver<WindowUpdate>,
    commands: mpsc::Sender<WindowCommand>,
    mapped: oneshot::Sender<()>,
) -> WindowOptions {
    WindowOptions {
        snapshot: Some(snapshot),
        updates: Some(updates),
        commands: Some(commands),
        controller_title: false,
        app_id: resolved_window_app_id(None),
        initial_columns: config.initial_columns,
        initial_rows: config.initial_rows,
        cursor_style: config.cursor_style,
        cursor_blink: false,
        theme,
        ui_font_size: config.ui_font_size,
        keymap: config.keymap.clone(),
        prefix_timeout_ms: config.prefix_timeout_ms,
        // The status Window may hold a password field: no IME composition.
        input_method: false,
        // The one non-picker content that may keep a staged Window.
        connection_status_screen: true,
        mapped: Some(mapped),
        ..WindowOptions::default()
    }
}

/// What a status screen ended with, and the Window it left parked.
pub(in crate::app) struct StatusEnd {
    /// The connected endpoint, or why none was opened.
    pub(in crate::app) connected: Result<RemoteConnected, RemoteNotConnected>,
    /// The parked Window, when the phase kept its surface and the Window
    /// was not closed.
    pub(in crate::app) surface: Option<WindowSurface>,
}

/// Shows the status screen, on `surface` when given or else in a new Window,
/// connects to `profile_name`, and returns once the screen has ended. With
/// `keep_surface`, the Window is left parked for the next phase, both after
/// connecting and after a status key cancelled.
///
/// # Errors
///
/// Returns an error when the Window itself failed.
pub(in crate::app) async fn connect_with_status(
    profile_name: String,
    config: &AppConfig,
    surface: Option<WindowSurface>,
    keep_surface: bool,
) -> Result<StatusEnd> {
    let initial = load_profile(&profile_name).map_or_else(
        |_| profile_label(&profile_name).to_owned(),
        |profile| destination(&profile),
    );
    // The status Window keeps its configured rows; prompts are fitted to them.
    let status = RemoteStatus::new(&profile_name, &initial)
        .with_terminal(std::io::stdin().is_terminal())
        .with_rows(config.initial_rows);
    let splint_id = SplintId::new();
    let snapshot = status.snapshot(splint_id, 1);
    let (updates, update_receiver) = mpsc::channel(STATUS_UPDATE_CAPACITY);
    let (command_sender, commands) = mpsc::channel(STATUS_COMMAND_CAPACITY);
    let (mapped_sender, mapped) = oneshot::channel();
    let window_config = config.clone();
    let window_task = tokio::task::spawn_blocking(move || {
        let theme = load_startup_theme(&window_config);
        configure_picker_renderer(&window_config, theme)?;
        let options = WindowOptions {
            keep_surface,
            ..status_window_options(
                &window_config,
                theme,
                snapshot,
                update_receiver,
                command_sender,
                mapped_sender,
            )
        };
        run_window_phase(surface, options)
    });
    // Without a usable executable path, OpenSSH cannot run the helper; the
    // sender then drops and the flow stops listening for prompts.
    let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
    let native_prompt = NativePrompt::current_executable(prompt_sender);
    let window = StatusWindow {
        splint_id,
        revision: 1,
        updates,
        mapped,
    };
    let connected = run_status_flow(
        status,
        window,
        commands,
        prompts,
        askpass::PROMPT_TIMEOUT,
        || start_attempt(&profile_name, native_prompt.clone()),
        report_failure,
    )
    .await;
    let surface = window_task
        .await
        .context("remote status Window task failed")??;
    Ok(StatusEnd { connected, surface })
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::{Arc, Mutex},
    };

    use splinterm::remote_session::RemoteFailure;

    use super::*;

    #[test]
    fn the_status_window_keeps_control_but_not_the_controller_title() {
        let (_updates, update_receiver) = mpsc::channel(STATUS_UPDATE_CAPACITY);
        let (command_sender, _commands) = mpsc::channel(STATUS_COMMAND_CAPACITY);
        let (mapped_sender, _mapped) = oneshot::channel();
        let status = RemoteStatus::new("work", "work.example");
        let options = status_window_options(
            &AppConfig::default(),
            ResolvedTheme::default(),
            status.snapshot(SplintId::new(), 1),
            update_receiver,
            command_sender,
            mapped_sender,
        );
        assert!(!options.controller_title);
        assert!(
            options.controlled,
            "R, Enter, and the password field still work"
        );
        assert!(options.commands.is_some());
        assert!(!options.input_method);
        assert!(options.connection_status_screen);
        assert!(!WindowOptions::default().connection_status_screen);
        assert!(WindowOptions::default().controller_title);
    }

    struct Harness {
        updates: mpsc::Receiver<WindowUpdate>,
        commands: mpsc::Sender<WindowCommand>,
        splint_id: SplintId,
        /// The fake Window's map signal, until a test sends or drops it.
        mapped: Option<oneshot::Sender<()>>,
    }

    /// A fake status Window that has not mapped yet.
    fn unmapped_harness() -> (Harness, StatusWindow, mpsc::Receiver<WindowCommand>) {
        let (updates, update_receiver) = mpsc::channel(STATUS_UPDATE_CAPACITY);
        let (command_sender, commands) = mpsc::channel(STATUS_COMMAND_CAPACITY);
        let (mapped_sender, mapped) = oneshot::channel();
        let splint_id = SplintId::new();
        (
            Harness {
                updates: update_receiver,
                commands: command_sender,
                splint_id,
                mapped: Some(mapped_sender),
            },
            StatusWindow {
                splint_id,
                revision: 1,
                updates,
                mapped,
            },
            commands,
        )
    }

    /// Long enough that no prompt test times out unless it means to.
    const TEST_PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

    /// A prompt channel whose sender is gone, for flows that never prompt.
    fn no_prompts() -> mpsc::Receiver<AskpassRequest> {
        mpsc::channel(PROMPT_CAPACITY).1
    }

    /// A fake status Window that has already mapped.
    fn harness() -> (Harness, StatusWindow, mpsc::Receiver<WindowCommand>) {
        let (mut harness, window, commands) = unmapped_harness();
        harness.mapped.take().unwrap().send(()).unwrap();
        (harness, window, commands)
    }

    fn fixed_destination() -> String {
        "case@wintermute".to_owned()
    }

    /// Runs the flow with the destination and the attempt as separate
    /// closures; each attempt reads the destination just before it starts.
    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the flow's independent seams plus the split destination"
    )]
    async fn split_flow<T, F, Fut>(
        status: RemoteStatus,
        window: StatusWindow,
        commands: mpsc::Receiver<WindowCommand>,
        prompts: mpsc::Receiver<AskpassRequest>,
        prompt_timeout: Duration,
        mut connect: F,
        report: impl FnMut(&RemoteStatusFailure),
        mut destination: impl FnMut() -> String,
    ) -> Result<T, RemoteNotConnected>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        run_status_flow(
            status,
            window,
            commands,
            prompts,
            prompt_timeout,
            move || (destination(), connect()),
            report,
        )
        .await
    }

    fn text(snapshot: &TerminalSnapshot) -> String {
        snapshot
            .visible_rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.content.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn next_snapshot(harness: &mut Harness) -> TerminalSnapshot {
        match harness.updates.recv().await {
            Some(WindowUpdate::Snapshot { snapshot, .. }) => snapshot,
            other => panic!("expected a status snapshot, got {other:?}"),
        }
    }

    async fn expect_closed(harness: &mut Harness) {
        assert!(matches!(
            harness.updates.recv().await,
            Some(WindowUpdate::Exited { splint_id }) if splint_id == harness.splint_id
        ));
    }

    fn denied() -> anyhow::Error {
        anyhow::Error::new(RemoteFailure::new(
            RemoteFailureKind::AuthenticationFailed,
            "case@wintermute: Permission denied (publickey).",
        ))
    }

    #[tokio::test]
    async fn success_closes_the_status_window_and_hands_over() {
        let (mut harness, window, commands) = harness();
        let reports = Cell::new(0);
        let connected = split_flow(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            || async { Ok("factory") },
            |_| reports.set(reports.get() + 1),
            fixed_destination,
        )
        .await;
        assert_eq!(connected, Ok("factory"));
        expect_closed(&mut harness).await;
        assert_eq!(reports.get(), 0);
    }

    #[tokio::test]
    async fn failure_is_shown_and_reported_then_retry_reconnects_once_per_press() {
        let (mut harness, window, commands) = harness();
        let attempts = Arc::new(Mutex::new(0_u32));
        let reports = Arc::new(Mutex::new(Vec::new()));
        // The second attempt waits here, so input can arrive while it runs.
        let release = Arc::new(tokio::sync::Notify::new());
        let flow = {
            let attempts = attempts.clone();
            let reports = reports.clone();
            let release = release.clone();
            tokio::spawn(split_flow(
                RemoteStatus::new("wintermute", "case@wintermute"),
                window,
                commands,
                no_prompts(),
                TEST_PROMPT_TIMEOUT,
                move || {
                    let attempt = {
                        let mut attempts = attempts.lock().unwrap();
                        *attempts += 1;
                        *attempts
                    };
                    let release = release.clone();
                    async move {
                        if attempt == 2 {
                            release.notified().await;
                        }
                        if attempt < 3 {
                            Err(denied())
                        } else {
                            Ok(attempt)
                        }
                    }
                },
                move |failure: &RemoteStatusFailure| {
                    reports.lock().unwrap().push(failure.copy().diagnostic_code);
                },
                fixed_destination,
            ))
        };

        let failed = next_snapshot(&mut harness).await;
        assert_eq!(failed.title, "Could not connect to wintermute");
        let screen = text(&failed);
        assert!(screen.contains("SSH authentication failed."));
        assert!(screen.contains("Permission denied (publickey)."));
        assert!(screen.contains("ssh-agent"));
        assert_eq!(*attempts.lock().unwrap(), 1);

        harness
            .commands
            .send(WindowCommand::Input(b"r".to_vec()))
            .await
            .unwrap();
        let retrying = next_snapshot(&mut harness).await;
        assert_eq!(retrying.title, "Connecting to wintermute");
        assert!(text(&retrying).contains("Attempt 2."));
        // A second press and unrelated input while reconnecting are ignored.
        for bytes in [&b"\r"[..], b"x"] {
            harness
                .commands
                .send(WindowCommand::Input(bytes.to_vec()))
                .await
                .unwrap();
        }
        harness
            .commands
            .send(WindowCommand::Resynchronize)
            .await
            .unwrap();
        while harness.commands.capacity() < STATUS_COMMAND_CAPACITY {
            tokio::task::yield_now().await;
        }
        assert_eq!(*attempts.lock().unwrap(), 2);
        release.notify_one();
        let failed_again = next_snapshot(&mut harness).await;
        assert_eq!(failed_again.title, "Could not connect to wintermute");
        assert!(failed_again.revision > retrying.revision);
        assert_eq!(*attempts.lock().unwrap(), 2);

        harness
            .commands
            .send(WindowCommand::Input(b"R".to_vec()))
            .await
            .unwrap();
        let _retrying = next_snapshot(&mut harness).await;
        assert_eq!(flow.await.unwrap(), Ok(3));
        expect_closed(&mut harness).await;
        assert_eq!(*attempts.lock().unwrap(), 3);
        assert_eq!(
            *reports.lock().unwrap(),
            [
                splinterm::diagnostics::DiagnosticErrorCode::RemoteAuthentication,
                splinterm::diagnostics::DiagnosticErrorCode::RemoteAuthentication
            ]
        );
    }

    #[tokio::test]
    async fn closing_cancels_the_attempt_and_the_window() {
        let (mut harness, window, commands) = harness();
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            std::future::pending,
            |_| panic!("a cancelled attempt is not a failure"),
            fixed_destination,
        ));
        harness
            .commands
            .send(WindowCommand::Input(b"\x1b".to_vec()))
            .await
            .unwrap();
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected::cancelled(None))
        );
        expect_closed(&mut harness).await;
    }

    #[tokio::test]
    async fn a_queued_cancel_wins_over_an_attempt_that_is_ready() {
        let (mut harness, window, commands) = harness();
        harness
            .commands
            .send(WindowCommand::Input(b"\x1b".to_vec()))
            .await
            .unwrap();
        let connected = split_flow(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            || async { Ok("factory") },
            |_| panic!("a cancelled attempt is not a failure"),
            fixed_destination,
        )
        .await;
        assert_eq!(connected, Err(RemoteNotConnected::cancelled(None)));
        expect_closed(&mut harness).await;
    }

    #[tokio::test]
    async fn a_window_closed_by_the_compositor_ends_the_flow() {
        let (harness, window, commands) = harness();
        let Harness {
            commands: sender, ..
        } = harness;
        drop(sender);
        let connected = split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            std::future::pending,
            |_| {},
            fixed_destination,
        )
        .await;
        assert_eq!(connected, Err(RemoteNotConnected::window_gone(None)));
    }

    #[tokio::test]
    async fn closing_after_a_profile_failure_reports_it_and_is_not_connected() {
        let (mut harness, window, commands) = harness();
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("gone", "case@wintermute"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            || async {
                Err(RemoteProfileError::wrap(&anyhow::anyhow!(
                    "remote profile \"gone\" is not configured"
                )))
            },
            |_| {},
            fixed_destination,
        ));
        let screen = text(&next_snapshot(&mut harness).await);
        assert!(screen.contains("The remote profile could not be loaded."));
        assert!(screen.contains("splinterm remote list"));
        harness
            .commands
            .send(WindowCommand::Input(b"c".to_vec()))
            .await
            .unwrap();
        let outcome = flow.await.unwrap();
        assert_eq!(
            outcome,
            Err(RemoteNotConnected::cancelled(Some(
                DiagnosticErrorCode::RemoteProfile
            )))
        );
        // `main` exits nonzero for this error, as before the status Window.
        let error = anyhow::Error::new(outcome.unwrap_err());
        assert!(error.downcast_ref::<RemoteNotConnected>().is_some());
        assert_ne!(RemoteNotConnected::EXIT_CODE, 0);
    }

    #[tokio::test]
    async fn uncategorized_attempt_errors_are_shown_as_transport_failures() {
        let (mut harness, window, commands) = harness();
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            || async {
                Err(anyhow::anyhow!(
                    "remote splinterd returned an invalid ping response"
                ))
            },
            |_| {},
            fixed_destination,
        ));
        let screen = text(&next_snapshot(&mut harness).await);
        assert!(screen.contains("The SSH connection failed."));
        assert!(!screen.contains("remote profile could not be loaded"));
        assert!(screen.contains("invalid ping response"));
        harness
            .commands
            .send(WindowCommand::Input(b"q".to_vec()))
            .await
            .unwrap();
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected::cancelled(Some(
                DiagnosticErrorCode::RemoteTransport
            )))
        );
    }

    const TYPED_SECRET: &str = "hunter2";

    async fn input(harness: &Harness, bytes: &[u8]) {
        harness
            .commands
            .send(WindowCommand::Input(bytes.to_vec()))
            .await
            .unwrap();
    }

    async fn settle(harness: &Harness) {
        while harness.commands.capacity() < STATUS_COMMAND_CAPACITY {
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;
    }

    fn assert_no_secret(snapshot: &TerminalSnapshot) {
        assert!(!snapshot.title.contains(TYPED_SECRET));
        assert!(!text(snapshot).contains(TYPED_SECRET), "{}", text(snapshot));
    }

    type SharedReply = Arc<Mutex<Option<tokio::sync::oneshot::Receiver<AskpassReply>>>>;
    type Seen = Arc<Mutex<Option<&'static str>>>;

    /// Stands in for OpenSSH inside an attempt. When the attempt is dropped
    /// (OpenSSH killed), it records the prompt's state at that moment, so
    /// a test can prove OpenSSH died before any refusal could reach it.
    struct KillProbe {
        reply: SharedReply,
        seen: Seen,
    }

    impl Drop for KillProbe {
        fn drop(&mut self) {
            use tokio::sync::oneshot::error::TryRecvError;
            let state = match self
                .reply
                .lock()
                .unwrap()
                .as_mut()
                .map(tokio::sync::oneshot::Receiver::try_recv)
            {
                None => "no prompt",
                Some(Err(TryRecvError::Empty)) => "prompt still open",
                Some(Err(TryRecvError::Closed)) => "prompt already dropped",
                Some(Ok(_)) => "prompt already answered",
            };
            *self.seen.lock().unwrap() = Some(state);
        }
    }

    /// A never-finishing attempt that carries a [`KillProbe`].
    fn probed_attempt(
        reply: &SharedReply,
        seen: &Seen,
    ) -> impl FnMut() -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>> + use<> {
        let reply = reply.clone();
        let seen = seen.clone();
        move || {
            let probe = KillProbe {
                reply: reply.clone(),
                seen: seen.clone(),
            };
            Box::pin(async move {
                let _probe = probe;
                std::future::pending().await
            })
        }
    }

    /// Sends a secret prompt and parks its reply where the probe can see it.
    async fn open_secret_prompt(prompt_sender: &mpsc::Sender<AskpassRequest>, reply: &SharedReply) {
        let (request, receiver) = AskpassRequest::for_test(AskpassKind::Secret, b"password:");
        *reply.lock().unwrap() = Some(receiver);
        prompt_sender.send(request).await.unwrap();
    }

    /// OpenSSH died first, and the prompt was then dropped without a refusal.
    async fn assert_killed_before_refusal(reply: &SharedReply, seen: &Seen) {
        assert_eq!(*seen.lock().unwrap(), Some("prompt still open"));
        let receiver = reply.lock().unwrap().take().unwrap();
        assert!(receiver.await.is_err(), "no refusal was sent");
    }

    #[tokio::test]
    async fn a_typed_password_reaches_openssh_and_never_the_screen() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let release = Arc::new(tokio::sync::Notify::new());
        let attempt_release = release.clone();
        let flow = tokio::spawn(split_flow(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            TEST_PROMPT_TIMEOUT,
            move || {
                let release = attempt_release.clone();
                async move {
                    release.notified().await;
                    Ok("factory")
                }
            },
            |_| panic!("no failure"),
            fixed_destination,
        ));
        let (request, reply) =
            AskpassRequest::for_test(AskpassKind::Secret, b"case@wintermute's password: ");
        prompt_sender.send(request).await.unwrap();
        let shown = next_snapshot(&mut harness).await;
        assert_eq!(shown.title, "Authenticating to wintermute");
        let screen = text(&shown);
        assert!(
            screen.contains("OpenSSH, connecting to case@wintermute, is asking for a credential.")
        );
        assert!(screen.contains("│ case@wintermute's password:"));
        assert!(screen.contains("Password field: empty"));

        // "c", "q", and "r" are password characters here, not status keys.
        input(&harness, b"hunc").await;
        let typed = next_snapshot(&mut harness).await;
        assert!(text(&typed).contains("Password field: entered (hidden)"));
        assert_no_secret(&typed);
        input(&harness, b"\x7fter2qr\x7f\x7f").await;
        settle(&harness).await;
        input(&harness, b"\r").await;
        let submitted = next_snapshot(&mut harness).await;
        assert_eq!(submitted.title, "Connecting to wintermute");
        assert_no_secret(&submitted);
        match reply.await.unwrap() {
            AskpassReply::Submit(secret) => assert_eq!(secret.expose(), TYPED_SECRET.as_bytes()),
            other => panic!("expected an answer, got {other:?}"),
        }
        release.notify_one();
        assert_eq!(flow.await.unwrap(), Ok("factory"));
        expect_closed(&mut harness).await;
        assert!(
            harness.updates.try_recv().is_err(),
            "no other frame was sent"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn escape_kills_openssh_before_dropping_the_prompt_unanswered() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let reply = SharedReply::default();
        let seen = Seen::default();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let flow = {
            let reports = reports.clone();
            tokio::spawn(split_flow(
                RemoteStatus::new("wintermute", "case@wintermute"),
                window,
                commands,
                prompts,
                TEST_PROMPT_TIMEOUT,
                probed_attempt(&reply, &seen),
                move |failure: &RemoteStatusFailure| {
                    reports.lock().unwrap().push(failure.clone());
                },
                fixed_destination,
            ))
        };
        open_secret_prompt(&prompt_sender, &reply).await;
        let _shown = next_snapshot(&mut harness).await;
        input(&harness, TYPED_SECRET.as_bytes()).await;
        let _typed = next_snapshot(&mut harness).await;
        input(&harness, b"\x1b").await;
        let failed = next_snapshot(&mut harness).await;
        assert_eq!(failed.title, "Could not connect to wintermute");
        let screen = text(&failed);
        assert!(screen.contains("SSH authentication failed."));
        assert!(screen.contains(PROMPT_CANCELLED));
        assert!(
            screen.contains("ssh-agent"),
            "the agent path stays preferred"
        );
        assert_no_secret(&failed);
        assert_killed_before_refusal(&reply, &seen).await;
        {
            let reports = reports.lock().unwrap();
            assert_eq!(reports.len(), 1);
            assert_eq!(
                reports[0].copy().diagnostic_code,
                DiagnosticErrorCode::RemoteAuthentication
            );
            assert!(
                reports[0]
                    .details
                    .iter()
                    .all(|line| !line.contains(TYPED_SECRET))
            );
        }
        input(&harness, b"c").await;
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected::cancelled(Some(
                DiagnosticErrorCode::RemoteAuthentication
            )))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unanswered_prompt_times_out_by_killing_the_attempt_first() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let reply = SharedReply::default();
        let seen = Seen::default();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let flow = {
            let reports = reports.clone();
            tokio::spawn(split_flow(
                RemoteStatus::new("wintermute", "case@wintermute"),
                window,
                commands,
                prompts,
                Duration::from_millis(100),
                probed_attempt(&reply, &seen),
                move |failure: &RemoteStatusFailure| {
                    reports.lock().unwrap().push(failure.copy().diagnostic_code);
                },
                fixed_destination,
            ))
        };
        open_secret_prompt(&prompt_sender, &reply).await;
        let _shown = next_snapshot(&mut harness).await;
        input(&harness, b"partial").await;
        let _typed = next_snapshot(&mut harness).await;
        let failed = tokio::time::timeout(Duration::from_secs(10), next_snapshot(&mut harness))
            .await
            .unwrap();
        assert_eq!(failed.title, "Could not connect to wintermute");
        assert!(text(&failed).contains(PROMPT_TIMED_OUT));
        assert_killed_before_refusal(&reply, &seen).await;
        assert_eq!(
            *reports.lock().unwrap(),
            [DiagnosticErrorCode::RemoteAuthentication]
        );
        input(&harness, b"q").await;
        assert!(flow.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn a_notice_does_not_time_out() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            Duration::from_millis(20),
            std::future::pending,
            |_| panic!("a notice never fails the attempt"),
            fixed_destination,
        ));
        let (request, reply) =
            AskpassRequest::for_test(AskpassKind::Notice, b"Confirm user presence for key");
        prompt_sender.send(request).await.unwrap();
        let _shown = next_snapshot(&mut harness).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(harness.updates.try_recv().is_err());
        input(&harness, b"\x1b").await;
        assert!(flow.await.unwrap().is_err());
        assert!(reply.await.is_err());
    }

    #[tokio::test]
    async fn confirmations_need_an_explicit_yes_or_no() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            TEST_PROMPT_TIMEOUT,
            std::future::pending,
            |_| {},
            fixed_destination,
        ));
        let (request, yes) = AskpassRequest::for_test(
            AskpassKind::Confirm,
            b"Are you sure you want to continue connecting (yes/no/[fingerprint])?",
        );
        prompt_sender.send(request).await.unwrap();
        let shown = text(&next_snapshot(&mut harness).await);
        assert!(shown.contains("Y: yes          N / Esc: no"));
        assert!(!shown.contains("Password field"));
        input(&harness, b"\r").await;
        input(&harness, b"c").await;
        settle(&harness).await;
        assert!(
            harness.updates.try_recv().is_err(),
            "Enter and C do not answer"
        );
        input(&harness, b"y").await;
        let _cleared = next_snapshot(&mut harness).await;
        assert!(matches!(yes.await.unwrap(), AskpassReply::Confirm));

        let (request, no) = AskpassRequest::for_test(AskpassKind::Confirm, b"Allow key? (yes/no)");
        prompt_sender.send(request).await.unwrap();
        let _shown = next_snapshot(&mut harness).await;
        input(&harness, b"N").await;
        let _cleared = next_snapshot(&mut harness).await;
        assert!(matches!(no.await.unwrap(), AskpassReply::Cancel));

        input(&harness, b"\x1b").await;
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected::cancelled(None))
        );
    }

    #[tokio::test]
    async fn an_abandoned_prompt_is_withdrawn_and_status_keys_return() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            TEST_PROMPT_TIMEOUT,
            std::future::pending,
            |_| {},
            fixed_destination,
        ));
        let (request, reply) = AskpassRequest::for_test(AskpassKind::Secret, b"password:");
        prompt_sender.send(request).await.unwrap();
        let _shown = next_snapshot(&mut harness).await;
        input(&harness, b"abc").await;
        let _typed = next_snapshot(&mut harness).await;
        // OpenSSH gave up or the helper died.
        drop(reply);
        let withdrawn = next_snapshot(&mut harness).await;
        assert_eq!(withdrawn.title, "Connecting to wintermute");
        assert!(!text(&withdrawn).contains("Password field"));
        // An already-abandoned request is skipped without being shown.
        let (request, reply) = AskpassRequest::for_test(AskpassKind::Secret, b"password:");
        drop(reply);
        prompt_sender.send(request).await.unwrap();
        settle(&harness).await;
        assert!(harness.updates.try_recv().is_err());
        input(&harness, b"c").await;
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected::cancelled(None))
        );
    }

    #[tokio::test]
    async fn prompts_without_a_running_attempt_are_refused() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            TEST_PROMPT_TIMEOUT,
            || async { Err(denied()) },
            |_| {},
            fixed_destination,
        ));
        let _failed = next_snapshot(&mut harness).await;
        let (request, reply) = AskpassRequest::for_test(AskpassKind::Secret, b"password:");
        prompt_sender.send(request).await.unwrap();
        assert!(matches!(reply.await.unwrap(), AskpassReply::Cancel));
        assert!(harness.updates.try_recv().is_err());
        input(&harness, b"q").await;
        assert!(flow.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn notices_keep_status_keys_and_closing_drops_an_open_prompt() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            TEST_PROMPT_TIMEOUT,
            std::future::pending,
            |_| {},
            fixed_destination,
        ));
        let (request, reply) =
            AskpassRequest::for_test(AskpassKind::Notice, b"Confirm user presence for key");
        prompt_sender.send(request).await.unwrap();
        let shown = text(&next_snapshot(&mut harness).await);
        assert!(shown.contains("OpenSSH, connecting to case@wintermute, says:"));
        input(&harness, b"c").await;
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected::cancelled(None))
        );
        assert!(
            reply.await.is_err(),
            "the prompt was dropped, which refuses it"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_compositor_close_kills_openssh_before_dropping_the_prompt() {
        let (harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let Harness {
            commands: sender,
            mut updates,
            ..
        } = harness;
        let reply = SharedReply::default();
        let seen = Seen::default();
        let flow = tokio::spawn(split_flow(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            TEST_PROMPT_TIMEOUT,
            probed_attempt(&reply, &seen),
            |_| {},
            fixed_destination,
        ));
        open_secret_prompt(&prompt_sender, &reply).await;
        assert!(matches!(
            updates.recv().await,
            Some(WindowUpdate::Snapshot { .. })
        ));
        drop(sender);
        assert!(flow.await.unwrap().is_err());
        assert_killed_before_refusal(&reply, &seen).await;
    }

    #[tokio::test]
    async fn the_first_attempt_waits_for_the_status_window_to_map() {
        let (mut harness, window, commands) = unmapped_harness();
        let attempts = Arc::new(Mutex::new(0_u32));
        let flow = {
            let attempts = attempts.clone();
            tokio::spawn(split_flow(
                RemoteStatus::new("wintermute", "case@wintermute"),
                window,
                commands,
                no_prompts(),
                TEST_PROMPT_TIMEOUT,
                move || {
                    *attempts.lock().unwrap() += 1;
                    async { Ok("factory") }
                },
                |_| {},
                fixed_destination,
            ))
        };
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        // A fast success must not queue the close before the Window can show.
        assert_eq!(*attempts.lock().unwrap(), 0);
        assert!(matches!(
            harness.updates.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(!flow.is_finished());

        harness.mapped.take().unwrap().send(()).unwrap();
        assert_eq!(flow.await.unwrap(), Ok("factory"));
        expect_closed(&mut harness).await;
        assert_eq!(*attempts.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_window_that_ends_unmapped_never_connects() {
        let (harness, window, commands) = unmapped_harness();
        let Harness {
            commands: sender,
            mapped,
            ..
        } = harness;
        drop((sender, mapped));
        let connected = split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            || -> std::future::Ready<Result<()>> { panic!("an unmapped Window never connects") },
            |_| {},
            fixed_destination,
        )
        .await;
        assert_eq!(connected, Err(RemoteNotConnected::window_gone(None)));
    }

    #[tokio::test]
    async fn retry_shows_the_destination_reloaded_from_the_profile() {
        let (mut harness, window, commands) = harness();
        let configured = Arc::new(Mutex::new("old.example".to_owned()));
        let flow = {
            let configured = configured.clone();
            tokio::spawn(split_flow::<(), _, _>(
                RemoteStatus::new("wintermute", "old.example"),
                window,
                commands,
                no_prompts(),
                TEST_PROMPT_TIMEOUT,
                || async { Err(denied()) },
                |_| {},
                move || configured.lock().unwrap().clone(),
            ))
        };
        let failed = text(&next_snapshot(&mut harness).await);
        assert!(failed.contains("Could not connect to old.example."));

        // remotes.toml is edited between the failure and the retry.
        *configured.lock().unwrap() = "case@new.example:2222".to_owned();
        harness
            .commands
            .send(WindowCommand::Input(b"r".to_vec()))
            .await
            .unwrap();
        let retrying = text(&next_snapshot(&mut harness).await);
        assert!(retrying.contains("Connecting to case@new.example:2222…"));
        assert!(!retrying.contains("old.example"));
        let failed_again = text(&next_snapshot(&mut harness).await);
        assert!(failed_again.contains("Could not connect to case@new.example:2222."));

        harness
            .commands
            .send(WindowCommand::Input(b"c".to_vec()))
            .await
            .unwrap();
        assert!(flow.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn the_first_attempt_shows_the_destination_reloaded_after_mapping() {
        let (mut harness, window, commands) = harness();
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "old.example"),
            window,
            commands,
            no_prompts(),
            TEST_PROMPT_TIMEOUT,
            std::future::pending,
            |_| {},
            || "case@new.example:2222".to_owned(),
        ));
        let connecting = text(&next_snapshot(&mut harness).await);
        assert!(connecting.contains("Connecting to case@new.example:2222…"));
        assert!(!connecting.contains("old.example"));
        harness
            .commands
            .send(WindowCommand::Input(b"c".to_vec()))
            .await
            .unwrap();
        assert!(flow.await.unwrap().is_err());
    }

    #[test]
    fn destinations_include_only_configured_user_and_port() {
        let catalog = RemoteCatalog::parse(
            "version = 1\n[remotes.a]\nhost = \"a.example\"\nuser = \"case\"\nport = 2222\n[remotes.b]\nhost = \"b\"\n",
            None,
        )
        .unwrap();
        assert_eq!(
            destination(catalog.get("a").unwrap()),
            "case@a.example:2222"
        );
        assert_eq!(destination(catalog.get("b").unwrap()), "b");
    }
}
