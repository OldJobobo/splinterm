//! Remote connection-status Window: connect, report, retry, or hand over.

use std::{future::Future, io::IsTerminal as _, time::Duration};

use anyhow::{Context, Result, bail};
use splinterm::{
    WindowCommand, WindowOptions, WindowUpdate,
    config::AppConfig,
    diagnostics::{DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, global},
    endpoint::ConnectionFactory,
    remote::{RemoteCatalog, RemoteProfile},
    remote_status::{
        RemoteProfileError, RemoteStatus, RemoteStatusEffect, RemoteStatusFailure, profile_label,
        status_key,
    },
    run_window,
};
use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::SplintId;
use splinterm_protocol::{Request, Response};
use tokio::sync::{mpsc, oneshot};

use super::{
    sessions::configure_picker_renderer, theme_watch::load_startup_theme,
    window::resolved_window_app_id,
};

const STATUS_UPDATE_CAPACITY: usize = 8;
const STATUS_COMMAND_CAPACITY: usize = 64;

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

/// Drives one status screen until it hands over a connected endpoint, or the
/// Window closes first. The first attempt starts once the Window has mapped.
/// `connect` starts one attempt and returns the destination it connects to,
/// so the shown destination and the connection come from one profile load.
/// `report` records each failure. Nothing here writes to stdout or stderr.
async fn run_status_flow<T, F, Fut>(
    mut status: RemoteStatus,
    mut window: StatusWindow,
    mut commands: mpsc::Receiver<WindowCommand>,
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
    loop {
        tokio::select! {
            // A cancellation queued alongside a finished attempt wins, so C,
            // Escape, or a compositor close is never overtaken by hand-over.
            biased;
            command = commands.recv() => {
                let effect = match command {
                    None => status.on_window_closed(),
                    Some(WindowCommand::Input(bytes)) => status_key(&bytes)
                        .map_or(RemoteStatusEffect::None, |key| status.on_key(key)),
                    Some(_) => RemoteStatusEffect::None,
                };
                match effect {
                    RemoteStatusEffect::StartAttempt => {
                        let (destination, future) = connect();
                        status.set_destination(&destination);
                        attempt = Some(Box::pin(future));
                        window.show(&status);
                    }
                    RemoteStatusEffect::Close => {
                        // Dropping the attempt kills its OpenSSH child.
                        drop(attempt.take());
                        window.close().await;
                        return Err(RemoteNotConnected { last_failure });
                    }
                    RemoteStatusEffect::None
                    | RemoteStatusEffect::ShowFailure
                    | RemoteStatusEffect::HandOver => {}
                }
            }
            result = async { attempt.as_mut().expect("guarded attempt").await }, if attempt.is_some() => {
                attempt = None;
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
) -> (
    String,
    impl Future<Output = Result<ConnectionFactory>> + use<>,
) {
    let profile = load_profile(profile_name);
    let shown = profile
        .as_ref()
        .map_or_else(|_| profile_label(profile_name).to_owned(), destination);
    (shown, connect_profile(profile))
}

/// One attempt: authenticate with the loaded profile and prove the remote
/// daemon answers, so daemon failures are reported here rather than after
/// hand-over. Only catalog errors are marked as profile errors; every later
/// error keeps its own category or is reported as a transport failure.
async fn connect_profile(profile: Result<RemoteProfile>) -> Result<ConnectionFactory> {
    let profile = profile.map_err(|error| RemoteProfileError::wrap(&error))?;
    let deadline = Duration::from_secs(u64::from(profile.connect_timeout_seconds()));
    let factory = ConnectionFactory::remote(&profile).await?;
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
    let initial = load_profile(&profile_name).map_or_else(
        |_| profile_label(&profile_name).to_owned(),
        |profile| destination(&profile),
    );
    let status =
        RemoteStatus::new(&profile_name, &initial).with_terminal(std::io::stdin().is_terminal());
    let splint_id = SplintId::new();
    let snapshot = status.snapshot(splint_id, 1);
    let (updates, update_receiver) = mpsc::channel(STATUS_UPDATE_CAPACITY);
    let (command_sender, commands) = mpsc::channel(STATUS_COMMAND_CAPACITY);
    let (mapped_sender, mapped) = oneshot::channel();
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
            mapped: Some(mapped_sender),
            ..WindowOptions::default()
        })
    });
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
        || start_attempt(&profile_name),
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

    use splinterm::remote_session::{RemoteFailure, RemoteFailureKind};
    use splinterm_protocol::TerminalSnapshot;

    use super::*;

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
    async fn split_flow<T, F, Fut>(
        status: RemoteStatus,
        window: StatusWindow,
        commands: mpsc::Receiver<WindowCommand>,
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
            Err(RemoteNotConnected { last_failure: None })
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
            || async { Ok("factory") },
            |_| panic!("a cancelled attempt is not a failure"),
            fixed_destination,
        )
        .await;
        assert_eq!(connected, Err(RemoteNotConnected { last_failure: None }));
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
            std::future::pending,
            |_| {},
            fixed_destination,
        )
        .await;
        assert_eq!(connected, Err(RemoteNotConnected { last_failure: None }));
    }

    #[tokio::test]
    async fn closing_after_a_profile_failure_reports_it_and_is_not_connected() {
        let (mut harness, window, commands) = harness();
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("gone", "case@wintermute"),
            window,
            commands,
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
        let flow = tokio::spawn(split_flow::<(), _, _>(
            RemoteStatus::new("wintermute", "case@wintermute"),
            window,
            commands,
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
            Err(RemoteNotConnected {
                last_failure: Some(DiagnosticErrorCode::RemoteTransport)
            })
        );
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
            || -> std::future::Ready<Result<()>> { panic!("an unmapped Window never connects") },
            |_| {},
            fixed_destination,
        )
        .await;
        assert_eq!(connected, Err(RemoteNotConnected { last_failure: None }));
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
