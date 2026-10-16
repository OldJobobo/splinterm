//! Remote connection-status Window: connect, report, retry, or hand over.

use std::{future::Future, io::IsTerminal as _, time::Duration};

use anyhow::{Context, Result, bail};
use splinterm::{
    WindowCommand, WindowOptions, WindowUpdate,
    config::AppConfig,
    diagnostics::{DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, global},
    endpoint::ConnectionFactory,
    remote::{RemoteCatalog, RemoteProfile},
    remote_askpass::{
        self as askpass, AskpassKind, AskpassReply, AskpassRequest, FieldOutcome, NativePrompt,
        Secret, apply_secret_input, confirm_key,
    },
    remote_session::RemoteFailureKind,
    remote_status::{
        RemoteFailureCategory, RemoteProfileError, RemoteStatus, RemoteStatusEffect,
        RemoteStatusFailure, profile_label, status_key,
    },
    run_window,
};
use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::SplintId;
use splinterm_protocol::{Request, Response};
use tokio::sync::mpsc;

use super::{
    sessions::configure_picker_renderer, theme_watch::load_startup_theme,
    window::resolved_window_app_id,
};

const STATUS_UPDATE_CAPACITY: usize = 8;
const STATUS_COMMAND_CAPACITY: usize = 64;
/// OpenSSH asks one question at a time; the broker serves one helper at a time.
const PROMPT_CAPACITY: usize = 1;
/// Detail shown when the user cancels a password prompt.
pub(in crate::app) const PROMPT_CANCELLED: &str = "The Splinterm password prompt was cancelled.";

/// The status Window closed before a connection was made: after a shown
/// failure, by cancelling an attempt, or by the compositor. The process exits
/// with status 1 so a terminal caller sees that no remote Window opened.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct RemoteNotConnected {
    /// The category of the last failure shown, if any.
    pub(crate) last_failure: Option<DiagnosticErrorCode>,
}

impl RemoteNotConnected {
    /// The process exit status for a remote launch that never connected.
    pub(crate) const EXIT_CODE: i32 = 1;
}

impl std::fmt::Display for RemoteNotConnected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("remote connection was not opened")
    }
}

impl std::error::Error for RemoteNotConnected {}

/// Formats the OpenSSH destination a profile connects to.
pub(in crate::app) fn destination(profile: &RemoteProfile) -> String {
    let user = profile
        .user()
        .map_or_else(String::new, |user| format!("{user}@"));
    let port = profile
        .port()
        .map_or_else(String::new, |port| format!(":{port}"));
    format!("{user}{}{port}", profile.host())
}

/// The status screen's channel to its Window.
struct StatusWindow {
    splint_id: SplintId,
    revision: u64,
    updates: mpsc::Sender<WindowUpdate>,
}

impl StatusWindow {
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
}

/// What a key press on an open prompt asks the flow to do.
enum PromptInput {
    None,
    Redraw,
    /// Escape on a masked field: refuse the prompt and end the attempt, so
    /// OpenSSH cannot re-ask or send an empty password.
    CancelAttempt,
}

fn cancelled_prompt_failure() -> RemoteStatusFailure {
    RemoteStatusFailure {
        category: RemoteFailureCategory::Remote(RemoteFailureKind::AuthenticationFailed),
        details: vec![PROMPT_CANCELLED.to_owned()],
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
                let ActivePrompt { request, field } = active.take().expect("guarded prompt");
                request.answer(AskpassReply::Submit(field));
                status.clear_prompt();
                PromptInput::Redraw
            }
            FieldOutcome::Cancel => {
                if let Some(prompt) = active.take() {
                    prompt.request.answer(AskpassReply::Cancel);
                }
                status.clear_prompt();
                PromptInput::CancelAttempt
            }
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

/// Drives one status screen until it hands over a connected endpoint, or the
/// Window closes first. `connect` runs one attempt; `report` records each
/// failure; `prompts` delivers OpenSSH prompts from the current attempt's
/// private askpass broker. Nothing here writes to stdout or stderr, and key
/// input is wiped after use because it may be a password.
async fn run_status_flow<T, F, Fut>(
    mut status: RemoteStatus,
    mut window: StatusWindow,
    mut commands: mpsc::Receiver<WindowCommand>,
    mut prompts: mpsc::Receiver<AskpassRequest>,
    mut connect: F,
    mut report: impl FnMut(&RemoteStatusFailure),
) -> Result<T, RemoteNotConnected>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = Some(Box::pin(connect()));
    let mut last_failure = None;
    let mut prompts_open = true;
    let mut active: Option<ActivePrompt> = None;
    loop {
        tokio::select! {
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
                        report(&failure);
                        last_failure = Some(failure.copy().diagnostic_code);
                        if status.on_attempt(Err(failure)) == RemoteStatusEffect::ShowFailure {
                            window.show(&status);
                        }
                    }
                }
            }
            request = prompts.recv(), if prompts_open && active.is_none() => {
                match request {
                    None => prompts_open = false,
                    Some(request) if request.is_abandoned() => {}
                    Some(request) => {
                        if attempt.is_some()
                            && status.show_prompt(request.kind(), request.prompt_lines())
                        {
                            active = Some(ActivePrompt { request, field: Secret::new() });
                            window.show(&status);
                        } else {
                            request.answer(AskpassReply::Cancel);
                        }
                    }
                }
            }
            () = async { active.as_mut().expect("guarded prompt").request.abandoned().await }, if active.is_some() => {
                // OpenSSH gave up, the prompt timed out, or the helper died.
                active = None;
                status.clear_prompt();
                window.show(&status);
            }
            command = commands.recv() => {
                let intercepted = active
                    .as_ref()
                    .is_some_and(|prompt| prompt.request.kind() != AskpassKind::Notice);
                let effect = match command {
                    None => status.on_window_closed(),
                    Some(WindowCommand::Input(mut bytes)) if intercepted => {
                        let input = prompt_input(&mut active, &mut status, &bytes);
                        askpass::wipe(&mut bytes);
                        match input {
                            PromptInput::None => {}
                            PromptInput::Redraw => window.show(&status),
                            PromptInput::CancelAttempt => {
                                // Dropping the attempt kills OpenSSH and its helper.
                                drop(attempt.take());
                                let failure = cancelled_prompt_failure();
                                report(&failure);
                                last_failure = Some(failure.copy().diagnostic_code);
                                if status.on_attempt(Err(failure)) == RemoteStatusEffect::ShowFailure {
                                    window.show(&status);
                                }
                            }
                        }
                        RemoteStatusEffect::None
                    }
                    Some(WindowCommand::Input(mut bytes)) => {
                        let effect = status_key(&bytes)
                            .map_or(RemoteStatusEffect::None, |key| status.on_key(key));
                        askpass::wipe(&mut bytes);
                        effect
                    }
                    Some(_) => RemoteStatusEffect::None,
                };
                match effect {
                    RemoteStatusEffect::StartAttempt => {
                        attempt = Some(Box::pin(connect()));
                        window.show(&status);
                    }
                    RemoteStatusEffect::Close => {
                        // Dropping the attempt kills its OpenSSH child, and
                        // dropping the prompt refuses it and wipes the field.
                        drop(active.take());
                        drop(attempt.take());
                        window.close().await;
                        return Err(RemoteNotConnected { last_failure });
                    }
                    RemoteStatusEffect::None
                    | RemoteStatusEffect::ShowFailure
                    | RemoteStatusEffect::HandOver => {}
                }
            }
        }
    }
}

/// One attempt: reload the profile, authenticate, and prove the remote daemon
/// answers, so daemon failures are reported here rather than after hand-over.
/// Only catalog errors are marked as profile errors; every later error keeps
/// its own category or is reported as a transport failure.
async fn connect_profile(
    profile_name: String,
    prompt: Option<NativePrompt>,
) -> Result<ConnectionFactory> {
    let catalog =
        RemoteCatalog::load_default().map_err(|error| RemoteProfileError::wrap(&error))?;
    let profile = catalog
        .get(&profile_name)
        .map_err(|error| RemoteProfileError::wrap(&error))?;
    let deadline = Duration::from_secs(u64::from(profile.connect_timeout_seconds()));
    let factory = match &prompt {
        Some(prompt) => ConnectionFactory::remote_with_prompt(profile, prompt).await?,
        None => ConnectionFactory::remote(profile).await?,
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
    Ok(factory)
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

/// Maps a status Window, connects to `profile_name`, and returns the connected
/// endpoint once the Window has closed.
///
/// # Errors
///
/// Returns [`RemoteNotConnected`] when the Window closed before connecting,
/// or an error when the Window itself failed.
pub(in crate::app) async fn connect_with_status(
    profile_name: String,
    config: &AppConfig,
) -> Result<ConnectionFactory> {
    let destination_label = RemoteCatalog::load_default()
        .ok()
        .and_then(|catalog| catalog.get(&profile_name).ok().map(destination))
        .unwrap_or_else(|| profile_label(&profile_name).to_owned());
    let status = RemoteStatus::new(&profile_name, &destination_label)
        .with_terminal(std::io::stdin().is_terminal());
    let splint_id = SplintId::new();
    let snapshot = status.snapshot(splint_id, 1);
    let (updates, update_receiver) = mpsc::channel(STATUS_UPDATE_CAPACITY);
    let (command_sender, commands) = mpsc::channel(STATUS_COMMAND_CAPACITY);
    let window_config = config.clone();
    let window_task = tokio::task::spawn_blocking(move || {
        let theme = load_startup_theme(&window_config);
        configure_picker_renderer(&window_config, theme)?;
        run_window(WindowOptions {
            snapshot: Some(snapshot),
            updates: Some(update_receiver),
            commands: Some(command_sender),
            app_id: resolved_window_app_id(None),
            initial_columns: window_config.initial_columns,
            initial_rows: window_config.initial_rows,
            cursor_style: window_config.cursor_style,
            cursor_blink: false,
            theme,
            keymap: window_config.keymap.clone(),
            prefix_timeout_ms: window_config.prefix_timeout_ms,
            ..WindowOptions::default()
        })
    });
    // Without a usable executable path, OpenSSH cannot run the helper; the
    // sender then drops and the flow stops listening for prompts.
    let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
    let native_prompt = NativePrompt::current_executable(prompt_sender);
    let window = StatusWindow {
        splint_id,
        revision: 1,
        updates,
    };
    let connected = run_status_flow(
        status,
        window,
        commands,
        prompts,
        || connect_profile(profile_name.clone(), native_prompt.clone()),
        report_failure,
    )
    .await;
    window_task
        .await
        .context("remote status Window task failed")??;
    connected.map_err(anyhow::Error::new)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::{Arc, Mutex},
    };

    use splinterm::remote_session::RemoteFailure;
    use splinterm_protocol::TerminalSnapshot;

    use super::*;

    struct Harness {
        updates: mpsc::Receiver<WindowUpdate>,
        commands: mpsc::Sender<WindowCommand>,
        splint_id: SplintId,
    }

    fn harness() -> (Harness, StatusWindow, mpsc::Receiver<WindowCommand>) {
        let (updates, update_receiver) = mpsc::channel(STATUS_UPDATE_CAPACITY);
        let (command_sender, commands) = mpsc::channel(STATUS_COMMAND_CAPACITY);
        let splint_id = SplintId::new();
        (
            Harness {
                updates: update_receiver,
                commands: command_sender,
                splint_id,
            },
            StatusWindow {
                splint_id,
                revision: 1,
                updates,
            },
            commands,
        )
    }

    /// A prompt channel whose sender is gone, for flows that never prompt.
    fn no_prompts() -> mpsc::Receiver<AskpassRequest> {
        mpsc::channel(PROMPT_CAPACITY).1
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
        let connected = run_status_flow(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            no_prompts(),
            || async { Ok("factory") },
            |_| reports.set(reports.get() + 1),
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
            tokio::spawn(run_status_flow(
                RemoteStatus::new("wintermute", "case@wintermute"),
                window,
                commands,
                no_prompts(),
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
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            no_prompts(),
            std::future::pending,
            |_| panic!("a cancelled attempt is not a failure"),
        ));
        harness
            .commands
            .send(WindowCommand::Input(b"\x1b".to_vec()))
            .await
            .unwrap();
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected { last_failure: None })
        );
        expect_closed(&mut harness).await;
    }

    #[tokio::test]
    async fn a_window_closed_by_the_compositor_ends_the_flow() {
        let (harness, window, commands) = harness();
        let Harness {
            commands: sender, ..
        } = harness;
        drop(sender);
        let connected = run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            no_prompts(),
            std::future::pending,
            |_| {},
        )
        .await;
        assert_eq!(connected, Err(RemoteNotConnected { last_failure: None }));
    }

    #[tokio::test]
    async fn closing_after_a_profile_failure_reports_it_and_is_not_connected() {
        let (mut harness, window, commands) = harness();
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("gone", "gone"),
            window,
            commands,
            no_prompts(),
            || async {
                Err(RemoteProfileError::wrap(&anyhow::anyhow!(
                    "remote profile \"gone\" is not configured"
                )))
            },
            |_| {},
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
            Err(RemoteNotConnected {
                last_failure: Some(DiagnosticErrorCode::RemoteProfile)
            })
        );
        // `main` exits nonzero for this error, as before the status Window.
        let error = anyhow::Error::new(outcome.unwrap_err());
        assert!(error.downcast_ref::<RemoteNotConnected>().is_some());
        assert_ne!(RemoteNotConnected::EXIT_CODE, 0);
    }

    #[tokio::test]
    async fn uncategorized_attempt_errors_are_shown_as_transport_failures() {
        let (mut harness, window, commands) = harness();
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            no_prompts(),
            || async {
                Err(anyhow::anyhow!(
                    "remote splinterd returned an invalid ping response"
                ))
            },
            |_| {},
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
            Err(RemoteNotConnected {
                last_failure: Some(DiagnosticErrorCode::RemoteTransport)
            })
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

    /// Sets a flag when the attempt future is dropped (OpenSSH killed).
    struct DropFlag(Arc<std::sync::atomic::AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn a_typed_password_reaches_openssh_and_never_the_screen() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let release = Arc::new(tokio::sync::Notify::new());
        let attempt_release = release.clone();
        let flow = tokio::spawn(run_status_flow(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
            prompts,
            move || {
                let release = attempt_release.clone();
                async move {
                    release.notified().await;
                    Ok("factory")
                }
            },
            |_| panic!("no failure"),
        ));
        let (request, reply) =
            AskpassRequest::for_test(AskpassKind::Secret, b"case@wintermute's password: ");
        prompt_sender.send(request).await.unwrap();
        let shown = next_snapshot(&mut harness).await;
        assert_eq!(shown.title, "Authenticating to wintermute");
        let screen = text(&shown);
        assert!(screen.contains("OpenSSH is asking for a credential for case@wintermute."));
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

    #[tokio::test]
    async fn escape_refuses_the_prompt_and_fails_the_attempt_as_authentication() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let flow = {
            let dropped = dropped.clone();
            let reports = reports.clone();
            tokio::spawn(run_status_flow::<(), _, _>(
                RemoteStatus::new("wintermute", "wintermute"),
                window,
                commands,
                prompts,
                move || {
                    let guard = DropFlag(dropped.clone());
                    async move {
                        let _guard = guard;
                        std::future::pending().await
                    }
                },
                move |failure: &RemoteStatusFailure| {
                    reports.lock().unwrap().push(failure.clone());
                },
            ))
        };
        let (request, reply) = AskpassRequest::for_test(AskpassKind::Secret, b"password:");
        prompt_sender.send(request).await.unwrap();
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
        assert!(matches!(reply.await.unwrap(), AskpassReply::Cancel));
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "cancelling ends the attempt so OpenSSH cannot re-ask"
        );
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
            Err(RemoteNotConnected {
                last_failure: Some(DiagnosticErrorCode::RemoteAuthentication)
            })
        );
    }

    #[tokio::test]
    async fn confirmations_need_an_explicit_yes_or_no() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            prompts,
            std::future::pending,
            |_| {},
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
            Err(RemoteNotConnected { last_failure: None })
        );
    }

    #[tokio::test]
    async fn an_abandoned_prompt_is_withdrawn_and_status_keys_return() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            prompts,
            std::future::pending,
            |_| {},
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
            Err(RemoteNotConnected { last_failure: None })
        );
    }

    #[tokio::test]
    async fn prompts_without_a_running_attempt_are_refused() {
        let (mut harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            prompts,
            || async { Err(denied()) },
            |_| {},
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
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            prompts,
            std::future::pending,
            |_| {},
        ));
        let (request, reply) =
            AskpassRequest::for_test(AskpassKind::Notice, b"Confirm user presence for key");
        prompt_sender.send(request).await.unwrap();
        let shown = text(&next_snapshot(&mut harness).await);
        assert!(shown.contains("OpenSSH says, for wintermute:"));
        input(&harness, b"c").await;
        assert_eq!(
            flow.await.unwrap(),
            Err(RemoteNotConnected { last_failure: None })
        );
        assert!(
            reply.await.is_err(),
            "the prompt was dropped, which refuses it"
        );
    }

    #[tokio::test]
    async fn a_compositor_close_drops_an_open_secret_prompt() {
        let (harness, window, commands) = harness();
        let (prompt_sender, prompts) = mpsc::channel(PROMPT_CAPACITY);
        let Harness {
            commands: sender,
            mut updates,
            ..
        } = harness;
        let flow = tokio::spawn(run_status_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "wintermute"),
            window,
            commands,
            prompts,
            std::future::pending,
            |_| {},
        ));
        let (request, reply) = AskpassRequest::for_test(AskpassKind::Secret, b"password:");
        prompt_sender.send(request).await.unwrap();
        assert!(matches!(
            updates.recv().await,
            Some(WindowUpdate::Snapshot { .. })
        ));
        drop(sender);
        assert!(flow.await.unwrap().is_err());
        assert!(reply.await.is_err());
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
