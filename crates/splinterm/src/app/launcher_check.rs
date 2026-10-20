//! `splinterm launcher-check`: the desktop launcher's daemon state boundary
//! (Plan 0037 Milestone 4).
//!
//! `splinterm-xdg-terminal-exec` runs this hidden command before it opens
//! anything and only acts on its exit status; it never infers safety from a
//! failed `ping`. The command connects as the trusted local client and reads
//! the running daemon's dry-run handoff report, which carries the daemon's
//! build identity and its exact live-Splint count. It compares that identity
//! with the one compiled into this client, which is the identity of the
//! daemon/client pair it was installed with.
//!
//! | Daemon state | Result |
//! | --- | --- |
//! | none is listening | exit 10: the launcher starts the service |
//! | this build | exit 0: open the Window |
//! | another build that reports no live Splint | exit 11: restart without asking |
//! | another build with live Splints | ask; exit 11 only on an explicit confirmation |
//! | unreadable: another protocol (such as 0.1.x), no answer, a bad answer | ask, counting "all running shells" |
//! | refuses this client as untrusted | exit 1 |
//!
//! "Not now", closing the dialog, no answer, or no way to ask exits 3, and the
//! launcher opens nothing. After the launcher started or restarted the service
//! (`--after start|restart`), the command waits up to five seconds for the
//! socket; after a restart it never asks or restarts again. Live upgrade is
//! never started from here (automatic upgrade on launch is deferred, Plan
//! 0061); when the daemon reports it possible, the question points to
//! `splinterm upgrade`.

use std::{
    io,
    sync::mpsc as std_mpsc,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use splinterm::automation::{Connection, protocol_error, socket_path};
use splinterm::{TrustedConsentUi, WindowOptions};
use splinterm_protocol::{ErrorCode, HandoffReport};

use super::{
    consent::{static_dialog_snapshot, trusted_label},
    upgrade::reason_line,
    upgrade_dialog::{
        CAPABILITY_BYTES, DialogPrompt, MAX_DIALOG_COLUMNS, ask_dialog_child, dialog_capability,
        dialog_rows, run_dialog_client,
    },
};

/// The hidden subcommand the restart dialog child runs.
pub(in crate::app) const RESTART_DIALOG_COMMAND: &str = "restart-dialog";

/// How long one probe may take to connect, shake hands, and read the report.
const ANSWER_BOUND: Duration = Duration::from_secs(5);

/// How long the check waits for the socket after the launcher started or
/// restarted the service.
const SOCKET_WAIT: Duration = Duration::from_secs(5);

const SOCKET_POLL: Duration = Duration::from_millis(50);

/// How long the restart question stays open before it means "Not now".
const RESPOND_WITHIN: Duration = Duration::from_secs(120);

/// Probes per check. A confirmed restart re-reads the daemon first and asks
/// again if what it would end changed, so this bounds the questions too.
const MAX_PROBES: usize = 4;

/// Width the question text wraps at, below the dialog's column clamp.
const WRAP_COLUMNS: usize = 76;

/// Most lines one blocking reason section may take in the question.
const MAX_REASON_LINES: usize = 10;

/// Most lines a restart dialog shows.
const MAX_RESTART_DIALOG_LINES: usize = 48;

/// What the launcher just did before running the check again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(in crate::app) enum LauncherStage {
    /// It started `splinterd.service` because no daemon was listening.
    Start,
    /// It restarted `splinterd.service` after an idle report or a confirmation.
    Restart,
}

/// `splinterm launcher-check` exit statuses, which the launcher script acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum LauncherExit {
    /// The running daemon is this build: open the Window.
    Ready,
    /// The check failed and said why. Nothing was started or restarted.
    Error,
    /// Not now: declined, closed, unanswered, or nothing could ask. Nothing
    /// opens and nothing changed.
    NotNow,
    /// No daemon is listening: start the service.
    StartService,
    /// Restart the service: the daemon proved it runs no live Splint, or the
    /// person confirmed the exact restart.
    RestartService,
}

impl LauncherExit {
    pub(in crate::app) const fn code(self) -> i32 {
        match self {
            Self::Ready => 0,
            Self::Error => 1,
            Self::NotNow => 3,
            Self::StartService => 10,
            Self::RestartService => 11,
        }
    }
}

/// Why the running daemon could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum Unreadable {
    /// It refused this client's protocol version, or could not parse its
    /// greeting: an older release such as 0.1.x, or another build whose
    /// private protocol differs.
    OtherProtocol,
    /// No complete answer within [`ANSWER_BOUND`].
    NoAnswer,
    /// It answered with something other than a usable handshake and report,
    /// or hung up.
    InvalidAnswer,
}

/// What one probe of the daemon socket found.
#[derive(Debug)]
pub(in crate::app) enum Probe {
    /// Nothing listens on the socket (no socket, or connection refused).
    NotRunning,
    /// The socket could not be used for another local reason.
    ConnectFailed(String),
    /// The daemon refused this client as not the installed trusted client.
    Untrusted,
    Unreadable(Unreadable),
    Report(Box<HandoffReport>),
}

/// The build this client was installed with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct InstalledBuild {
    pub(in crate::app) identity: [u8; splinterm_protocol::HANDOFF_BUILD_IDENTITY_BYTES],
    pub(in crate::app) version: String,
}

/// What the running daemon is, as the question describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum RunningDaemon {
    Build { version: String, generation: u64 },
    Unreadable(Unreadable),
}

/// Whether live upgrade could keep the shells instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum LiveUpgrade {
    /// The daemon reports nothing blocking it.
    Possible,
    /// One sentence per reason the daemon reported.
    Blocked(Vec<String>),
    /// The daemon could not be asked.
    Unknown,
}

/// The restart question: what runs, what is installed, and what ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct RestartQuestion {
    pub(in crate::app) running: RunningDaemon,
    pub(in crate::app) installed_version: String,
    /// The exact live-Splint count the daemon reported, or `None` when it
    /// could not report one.
    pub(in crate::app) live_splints: Option<usize>,
    pub(in crate::app) live_upgrade: LiveUpgrade,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum Decision {
    Launch,
    StartService,
    /// The daemon reported zero live Splints.
    RestartIdle,
    Ask(RestartQuestion),
    Fail(Vec<String>),
}

/// Decides what the launcher does from one probe. Pure: every state the
/// launcher can meet is decided here and unit tested.
///
/// Only a report from the daemon itself proves anything: a matching build
/// launches, and only zero live Splints restarts without asking. Anything
/// that cannot be read asks, because it may still run shells. Nothing
/// restarts after a restart.
pub(in crate::app) fn decide(
    probe: &Probe,
    installed: &InstalledBuild,
    after: Option<LauncherStage>,
) -> Decision {
    match probe {
        Probe::Report(report) if report.build.identity == installed.identity => Decision::Launch,
        Probe::NotRunning => match after {
            None => Decision::StartService,
            Some(_) => Decision::Fail(vec![format!(
                "splinterm: splinterd did not start listening within {} seconds; inspect `systemctl --user status splinterd.service`.",
                SOCKET_WAIT.as_secs()
            )]),
        },
        Probe::ConnectFailed(message) => Decision::Fail(vec![format!(
            "splinterm: cannot connect to splinterd: {}. Nothing was started or restarted.",
            trusted_label(message, 512)
        )]),
        Probe::Untrusted => Decision::Fail(untrusted_lines()),
        _ if after == Some(LauncherStage::Restart) => {
            Decision::Fail(still_not_installed_lines(probe, installed))
        }
        Probe::Report(report) if report.live_splints == 0 => Decision::RestartIdle,
        Probe::Report(report) => Decision::Ask(RestartQuestion {
            running: RunningDaemon::Build {
                version: report.build.version.clone(),
                generation: report.daemon_generation,
            },
            installed_version: installed.version.clone(),
            live_splints: Some(report.live_splints),
            live_upgrade: if report.blocking_reasons.is_empty() {
                LiveUpgrade::Possible
            } else {
                LiveUpgrade::Blocked(
                    report
                        .blocking_reasons
                        .iter()
                        .map(|reason| reason_line(reason, report.build.provenance))
                        .collect(),
                )
            },
        }),
        Probe::Unreadable(reason) => Decision::Ask(RestartQuestion {
            running: RunningDaemon::Unreadable(*reason),
            installed_version: installed.version.clone(),
            live_splints: None,
            live_upgrade: LiveUpgrade::Unknown,
        }),
    }
}

fn untrusted_lines() -> Vec<String> {
    vec![
        "splinterm: splinterd answers the launcher only from the installed splinterm beside the running splinterd (on packaged systems /usr/bin/splinterm), matched by device and inode.".to_owned(),
        "Run that installed client, not a development build or a copy earlier on PATH; check with `command -v splinterm`. Nothing was started or restarted.".to_owned(),
    ]
}

fn still_not_installed_lines(probe: &Probe, installed: &InstalledBuild) -> Vec<String> {
    let found = match probe {
        Probe::Report(report) => format!(
            "it serves Splinterm {} (generation {})",
            report.build.version, report.daemon_generation
        ),
        Probe::Unreadable(reason) => unreadable_phrase(*reason).to_owned(),
        _ => "it could not be read".to_owned(),
    };
    vec![
        format!(
            "splinterm: splinterd.service was restarted, but it still does not serve this client's build (Splinterm {}): {found}.",
            installed.version
        ),
        "It is not restarted again. Check `command -v splinterm` and `systemctl --user cat splinterd.service`.".to_owned(),
    ]
}

const fn unreadable_phrase(reason: Unreadable) -> &'static str {
    match reason {
        Unreadable::OtherProtocol => "it does not speak this client's protocol",
        Unreadable::NoAnswer => "it did not answer in time",
        Unreadable::InvalidAnswer => "it did not give a usable answer",
    }
}

/// "4 running shells", "1 running shell", or "all running shells".
pub(in crate::app) fn shells_phrase(count: Option<usize>) -> String {
    match count {
        Some(1) => "1 running shell".to_owned(),
        Some(count) => format!("{count} running shells"),
        None => "all running shells".to_owned(),
    }
}

/// The confirming action's label, with the exact count when known.
pub(in crate::app) fn confirm_label(count: Option<usize>) -> String {
    match count {
        Some(1) => "Restart now (ends 1 shell)".to_owned(),
        Some(count) => format!("Restart now (ends {count} shells)"),
        None => "Restart now (ends all running shells)".to_owned(),
    }
}

/// Greedy word wrap at `width` bytes; every line after the first starts
/// with `indent`. A word longer than the width gets a line of its own.
fn wrap(text: &str, width: usize, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if line.is_empty() {
            if !lines.is_empty() {
                line.push_str(indent);
            }
        } else {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The question's explanation, shared by the dialog and the terminal note:
/// everything above the action hints.
pub(in crate::app) fn question_text(question: &RestartQuestion) -> Vec<String> {
    let mut lines = vec![
        "TRUSTED SPLINTERM UPGRADE QUESTION".to_owned(),
        String::new(),
    ];
    let installed = trusted_label(&question.installed_version, 64);
    match &question.running {
        RunningDaemon::Build {
            version,
            generation,
        } => {
            lines.push("The running Splinterm daemon is not the installed build.".to_owned());
            lines.push(format!(
                "  Running:   Splinterm {}, generation {generation}",
                trusted_label(version, 64)
            ));
            lines.push(format!("  Installed: Splinterm {installed}"));
            lines.extend(wrap(
                &format!(
                    "It still has {}, so it is not restarted without asking.",
                    shells_phrase(question.live_splints)
                ),
                WRAP_COLUMNS,
                "",
            ));
        }
        RunningDaemon::Unreadable(reason) => {
            let explanation = match reason {
                Unreadable::OtherProtocol => format!(
                    "The running Splinterm daemon does not speak this client's protocol. It is an older release, such as 0.1.x, or another incompatible build, so this client (Splinterm {installed}) cannot count its shells or ask it to hand them over."
                ),
                Unreadable::NoAnswer => format!(
                    "The running Splinterm daemon did not answer within {} seconds, so its shells cannot be counted. If a live upgrade is in progress, choose Not now and try again shortly. Installed: Splinterm {installed}.",
                    ANSWER_BOUND.as_secs()
                ),
                Unreadable::InvalidAnswer => format!(
                    "The running Splinterm daemon did not give a usable answer, so its shells cannot be counted. If a live upgrade is in progress, choose Not now and try again shortly. Installed: Splinterm {installed}."
                ),
            };
            lines.extend(wrap(&explanation, WRAP_COLUMNS, ""));
        }
    }
    lines.push(String::new());
    lines.extend(wrap(
        &format!(
            "Restarting it starts the installed build and ends {} and the programs in them. Open Splinterm Windows must be reopened afterwards.",
            shells_phrase(question.live_splints)
        ),
        WRAP_COLUMNS,
        "",
    ));
    match &question.live_upgrade {
        LiveUpgrade::Possible => {
            lines.push(String::new());
            lines.extend(wrap(
                "Live upgrade can keep them running instead: choose Not now, then run `splinterm upgrade` in a terminal.",
                WRAP_COLUMNS,
                "",
            ));
        }
        LiveUpgrade::Blocked(reasons) => {
            lines.push(String::new());
            lines.push("Live upgrade is not available now:".to_owned());
            let mut section = Vec::new();
            for reason in reasons {
                section.extend(wrap(
                    &format!("• {}", trusted_label(reason, 1024)),
                    WRAP_COLUMNS - 2,
                    "    ",
                ));
            }
            if section.len() > MAX_REASON_LINES {
                section.truncate(MAX_REASON_LINES - 1);
                section.push("• … run `splinterm upgrade` for the full report.".to_owned());
            }
            lines.extend(section.into_iter().map(|line| format!("  {line}")));
        }
        LiveUpgrade::Unknown => {}
    }
    lines.push(String::new());
    lines.push("Not now opens nothing and changes nothing.".to_owned());
    lines
}

/// The whole dialog: the question, the deadline, and the actions.
pub(in crate::app) fn dialog_lines(question: &RestartQuestion) -> Vec<String> {
    let mut lines = question_text(question);
    lines.push(format!(
        "Closing this dialog, or no answer within {} seconds, means Not now.",
        RESPOND_WITHIN.as_secs()
    ));
    lines.push(String::new());
    lines.push(format!(
        "[D / Esc / Enter  Not now]     [R  {}]",
        confirm_label(question.live_splints)
    ));
    lines
        .push("Click the red left area for Not now or the green right area to restart.".to_owned());
    lines
}

/// What the launcher prints when the answer is "Not now".
pub(in crate::app) fn not_now_lines(question: &RestartQuestion) -> Vec<String> {
    let mut lines =
        vec!["Not now: nothing was opened, and the daemon and its shells keep running.".to_owned()];
    if question.live_upgrade == LiveUpgrade::Possible {
        lines.push(
            "Run `splinterm upgrade` to move them to the installed build without ending them."
                .to_owned(),
        );
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RestartDialogPrompt {
    capability: Vec<u8>,
    parent_pid: u32,
    lines: Vec<String>,
}

impl DialogPrompt for RestartDialogPrompt {
    fn capability(&self) -> &[u8] {
        &self.capability
    }

    fn parent_pid(&self) -> u32 {
        self.parent_pid
    }

    fn validate(&self) -> Result<()> {
        let valid = self.capability.len() == CAPABILITY_BYTES
            && self.parent_pid > 0
            && !self.lines.is_empty()
            && self.lines.len() <= MAX_RESTART_DIALOG_LINES
            && self
                .lines
                .iter()
                .all(|line| line.len() <= MAX_DIALOG_COLUMNS);
        if !valid {
            anyhow::bail!("invalid restart dialog prompt");
        }
        Ok(())
    }

    fn window_options(&self, decision: std_mpsc::Sender<bool>) -> WindowOptions {
        let lines = self
            .lines
            .iter()
            .map(|line| trusted_label(line, MAX_DIALOG_COLUMNS))
            .collect::<Vec<_>>();
        let rows = dialog_rows(lines.len());
        let snapshot = static_dialog_snapshot(
            lines,
            rows,
            "Upgrade question",
            splinterm_core::SplintId::new(),
            0,
        );
        let columns = u16::try_from(snapshot.columns).unwrap_or(u16::MAX);
        let rows = u16::try_from(snapshot.rows).unwrap_or(u16::MAX);
        WindowOptions {
            snapshot: Some(snapshot),
            trusted_consent: Some(TrustedConsentUi::restart(decision)),
            initial_columns: columns,
            initial_rows: rows,
            ..WindowOptions::default()
        }
    }
}

/// The hidden `restart-dialog` child: shows one restart question and
/// answers on stdout. Closing the Window answers "Not now".
pub(in crate::app) fn run_restart_dialog_client() -> Result<()> {
    run_dialog_client::<RestartDialogPrompt>()
}

/// Runs the check and exits with its [`LauncherExit`].
pub(in crate::app) async fn run(after: Option<LauncherStage>) -> Result<()> {
    let exit = check(after).await;
    std::process::exit(exit.code());
}

async fn check(after: Option<LauncherStage>) -> LauncherExit {
    let installed = match installed_build() {
        Ok(installed) => installed,
        Err(error) => {
            eprintln!("splinterm: {error:#}; nothing was started or restarted.");
            return LauncherExit::Error;
        }
    };
    let wait = if after.is_some() {
        SOCKET_WAIT
    } else {
        Duration::ZERO
    };
    let mut confirmed: Option<RestartQuestion> = None;
    for _ in 0..MAX_PROBES {
        match decide(&probe(wait).await, &installed, after) {
            Decision::Launch => return LauncherExit::Ready,
            Decision::StartService => return LauncherExit::StartService,
            Decision::RestartIdle => return LauncherExit::RestartService,
            Decision::Fail(lines) => {
                for line in lines {
                    eprintln!("{line}");
                }
                return LauncherExit::Error;
            }
            // Re-read after a confirmation: restart only if what ends is
            // still exactly what the person confirmed.
            Decision::Ask(question) if confirmed.as_ref() == Some(&question) => {
                return LauncherExit::RestartService;
            }
            Decision::Ask(question) => {
                if !ask(&question).await {
                    for line in not_now_lines(&question) {
                        eprintln!("{line}");
                    }
                    return LauncherExit::NotNow;
                }
                confirmed = Some(question);
            }
        }
    }
    eprintln!("splinterm: the daemon kept changing while asking, so nothing was restarted.");
    LauncherExit::NotNow
}

fn installed_build() -> Result<InstalledBuild> {
    let generation = splinterd::handoff_preflight::compiled_handoff_generation()
        .map_err(|_| anyhow!("this client's build identity is unavailable"))?;
    Ok(InstalledBuild {
        identity: generation.build_identity.as_bytes(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}

/// Probes until something answers other than "not running", for at most
/// `wait`.
async fn probe(wait: Duration) -> Probe {
    let started = Instant::now();
    loop {
        let probe = probe_once().await;
        if matches!(probe, Probe::NotRunning) && started.elapsed() < wait {
            tokio::time::sleep(SOCKET_POLL).await;
            continue;
        }
        return probe;
    }
}

async fn probe_once() -> Probe {
    // Resolved first, so a missing runtime directory is a local failure and
    // never reads as an unreadable daemon.
    if let Err(error) = socket_path() {
        return Probe::ConnectFailed(format!("{error:#}"));
    }
    let attempt = tokio::time::timeout(ANSWER_BOUND, async {
        let mut connection = match Connection::connect().await {
            Ok(connection) => connection,
            Err(error) => return classify_connect_error(&error),
        };
        match connection.daemon_handoff_report().await {
            Ok(report) => Probe::Report(Box::new(report)),
            Err(error) => classify_report_error(&error),
        }
    })
    .await;
    attempt.unwrap_or(Probe::Unreadable(Unreadable::NoAnswer))
}

/// Classifies a failed connection and handshake.
pub(in crate::app) fn classify_connect_error(error: &anyhow::Error) -> Probe {
    if let Some(error) = protocol_error(error) {
        return match error.code {
            ErrorCode::Unauthorized => Probe::Untrusted,
            ErrorCode::IncompatibleVersion | ErrorCode::HandshakeRequired => {
                Probe::Unreadable(Unreadable::OtherProtocol)
            }
            _ => Probe::Unreadable(Unreadable::InvalidAnswer),
        };
    }
    match error.root_cause().downcast_ref::<io::Error>() {
        Some(io_error)
            if matches!(
                io_error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Probe::NotRunning
        }
        // The daemon accepted and then hung up during the handshake, as a
        // daemon that cannot parse this client's greeting may.
        Some(io_error)
            if matches!(
                io_error.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
            ) =>
        {
            Probe::Unreadable(Unreadable::InvalidAnswer)
        }
        Some(_) => Probe::ConnectFailed(format!("{error:#}")),
        // A frame that did not decode, or a handshake this client rejected.
        None => Probe::Unreadable(Unreadable::InvalidAnswer),
    }
}

/// Classifies a failed report request on an established connection.
pub(in crate::app) fn classify_report_error(error: &anyhow::Error) -> Probe {
    match protocol_error(error) {
        Some(error) if error.code == ErrorCode::Unauthorized => Probe::Untrusted,
        _ => Probe::Unreadable(Unreadable::InvalidAnswer),
    }
}

/// Asks the question and returns whether to restart. Without a graphical
/// session, or when the dialog fails, it explains on stderr and answers no.
async fn ask(question: &RestartQuestion) -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|display| display.is_empty()) {
        for line in question_text(question) {
            eprintln!("{line}");
        }
        eprintln!(
            "There is no graphical session to ask in, so nothing was restarted. To restart anyway, run from a terminal Splinterm does not own: systemctl --user restart splinterd.service"
        );
        return false;
    }
    let request = match dialog_capability() {
        Ok(capability) => RestartDialogPrompt {
            capability,
            parent_pid: std::process::id(),
            lines: dialog_lines(question),
        },
        Err(error) => {
            eprintln!("splinterm: could not ask ({error:#}); nothing was restarted.");
            return false;
        }
    };
    match ask_dialog_child(RESTART_DIALOG_COMMAND, request, RESPOND_WITHIN).await {
        Ok(restart) => restart,
        Err(error) => {
            eprintln!(
                "splinterm: could not ask about restarting ({error:#}); nothing was restarted."
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use splinterm::automation::response_protocol_error;
    use splinterm_protocol::{
        HandoffBlockingReason, HandoffBuild, HandoffBuildProvenance, HandoffVersionRange,
        ProtocolError,
    };

    const INSTALLED: u8 = 7;
    const OLDER: u8 = 6;

    fn installed() -> InstalledBuild {
        InstalledBuild {
            identity: [INSTALLED; splinterm_protocol::HANDOFF_BUILD_IDENTITY_BYTES],
            version: "0.2.1".to_owned(),
        }
    }

    fn report(identity: u8, live_splints: usize, reasons: Vec<HandoffBlockingReason>) -> Probe {
        let range = HandoffVersionRange {
            minimum: 1,
            maximum: 1,
        };
        Probe::Report(Box::new(HandoffReport {
            daemon_generation: 3,
            build: HandoffBuild {
                version: format!("0.2.{identity}"),
                identity: [identity; splinterm_protocol::HANDOFF_BUILD_IDENTITY_BYTES],
                provenance: HandoffBuildProvenance::ReleaseCandidate,
            },
            private_protocol: range,
            handoff_protocol: range,
            terminal_checkpoint: range,
            descriptor_manifest: range,
            lairs: 1,
            transient_lairs: 0,
            dojos: 1,
            live_splints,
            rollback_available: true,
            blocking_reasons: reasons,
        }))
    }

    fn protocol_failure(code: ErrorCode) -> anyhow::Error {
        response_protocol_error(ProtocolError::new(code, "refused"))
    }

    #[test]
    fn exit_codes_are_distinct_and_keep_clap_usage() {
        let codes = [
            LauncherExit::Ready,
            LauncherExit::Error,
            LauncherExit::NotNow,
            LauncherExit::StartService,
            LauncherExit::RestartService,
        ]
        .map(LauncherExit::code);
        assert_eq!(codes, [0, 1, 3, 10, 11]);
        assert!(!codes.contains(&2));
    }

    #[test]
    fn the_matching_build_launches_whatever_runs_in_it_and_after_any_stage() {
        for live in [0, 1, 40] {
            for after in [
                None,
                Some(LauncherStage::Start),
                Some(LauncherStage::Restart),
            ] {
                assert_eq!(
                    decide(&report(INSTALLED, live, Vec::new()), &installed(), after),
                    Decision::Launch
                );
            }
        }
    }

    #[test]
    fn only_a_reported_zero_restarts_without_asking() {
        assert_eq!(
            decide(&report(OLDER, 0, Vec::new()), &installed(), None),
            Decision::RestartIdle
        );
        assert_eq!(
            decide(
                &report(OLDER, 0, Vec::new()),
                &installed(),
                Some(LauncherStage::Start)
            ),
            Decision::RestartIdle
        );
        // Neither a failed handshake, a missing answer, nor a bad answer is
        // proof of idleness.
        for reason in [
            Unreadable::OtherProtocol,
            Unreadable::NoAnswer,
            Unreadable::InvalidAnswer,
        ] {
            assert!(matches!(
                decide(&Probe::Unreadable(reason), &installed(), None),
                Decision::Ask(_)
            ));
        }
    }

    #[test]
    fn another_build_with_live_shells_asks_with_the_exact_count() {
        let Decision::Ask(question) = decide(&report(OLDER, 4, Vec::new()), &installed(), None)
        else {
            panic!("expected a question");
        };
        assert_eq!(question.live_splints, Some(4));
        assert_eq!(question.live_upgrade, LiveUpgrade::Possible);
        assert_eq!(
            question.running,
            RunningDaemon::Build {
                version: "0.2.6".to_owned(),
                generation: 3
            }
        );
        assert_eq!(question.installed_version, "0.2.1");
        assert_eq!(
            confirm_label(question.live_splints),
            "Restart now (ends 4 shells)"
        );

        let Decision::Ask(blocked) = decide(
            &report(OLDER, 1, vec![HandoffBlockingReason::Unsupported]),
            &installed(),
            None,
        ) else {
            panic!("expected a question");
        };
        assert_eq!(
            blocked.live_upgrade,
            LiveUpgrade::Blocked(vec![
                "Live upgrade is not available in this build yet.".to_owned()
            ])
        );
        assert_eq!(
            confirm_label(blocked.live_splints),
            "Restart now (ends 1 shell)"
        );
    }

    #[test]
    fn an_unreadable_daemon_is_the_bootstrap_boundary_counting_all_shells() {
        let Decision::Ask(question) = decide(
            &Probe::Unreadable(Unreadable::OtherProtocol),
            &installed(),
            None,
        ) else {
            panic!("expected a question");
        };
        assert_eq!(question.live_splints, None);
        assert_eq!(question.live_upgrade, LiveUpgrade::Unknown);
        assert_eq!(
            confirm_label(question.live_splints),
            "Restart now (ends all running shells)"
        );
        let text = dialog_lines(&question).join(" ");
        assert!(text.contains("such as 0.1.x"), "{text}");
        assert!(text.contains("ends all running shells"), "{text}");
        assert!(!text.contains("splinterm upgrade"), "{text}");
    }

    #[test]
    fn no_daemon_starts_the_service_once_and_never_restarts() {
        assert_eq!(
            decide(&Probe::NotRunning, &installed(), None),
            Decision::StartService
        );
        for after in [LauncherStage::Start, LauncherStage::Restart] {
            let Decision::Fail(lines) = decide(&Probe::NotRunning, &installed(), Some(after))
            else {
                panic!("expected a failure");
            };
            assert!(lines[0].contains("did not start listening"), "{lines:?}");
        }
    }

    #[test]
    fn nothing_asks_or_restarts_again_after_a_restart() {
        for probe in [
            report(OLDER, 0, Vec::new()),
            report(OLDER, 5, Vec::new()),
            Probe::Unreadable(Unreadable::OtherProtocol),
            Probe::Unreadable(Unreadable::NoAnswer),
        ] {
            let decision = decide(&probe, &installed(), Some(LauncherStage::Restart));
            let Decision::Fail(lines) = decision else {
                panic!("expected a failure for {probe:?}");
            };
            assert!(lines[0].contains("still does not serve"), "{lines:?}");
            assert!(lines[1].contains("not restarted again"), "{lines:?}");
        }
    }

    #[test]
    fn untrusted_and_local_failures_never_restart() {
        for after in [
            None,
            Some(LauncherStage::Start),
            Some(LauncherStage::Restart),
        ] {
            let Decision::Fail(lines) = decide(&Probe::Untrusted, &installed(), after) else {
                panic!("expected a failure");
            };
            assert!(lines.join(" ").contains("command -v splinterm"));
            assert!(matches!(
                decide(
                    &Probe::ConnectFailed("permission denied".to_owned()),
                    &installed(),
                    after
                ),
                Decision::Fail(_)
            ));
        }
    }

    #[test]
    fn errors_are_classified_without_guessing_idleness() {
        let missing = anyhow::Error::new(io::Error::from(io::ErrorKind::NotFound))
            .context("cannot connect to splinterd at /run/user/1/splinterm/splinterd.sock");
        assert!(matches!(
            classify_connect_error(&missing),
            Probe::NotRunning
        ));
        let refused = anyhow::Error::new(io::Error::from(io::ErrorKind::ConnectionRefused))
            .context("cannot connect");
        assert!(matches!(
            classify_connect_error(&refused),
            Probe::NotRunning
        ));
        let denied = anyhow::Error::new(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(matches!(
            classify_connect_error(&denied),
            Probe::ConnectFailed(_)
        ));
        let hung_up = anyhow::Error::new(io::Error::from(io::ErrorKind::UnexpectedEof));
        assert!(matches!(
            classify_connect_error(&hung_up),
            Probe::Unreadable(Unreadable::InvalidAnswer)
        ));
        assert!(matches!(
            classify_connect_error(&protocol_failure(ErrorCode::IncompatibleVersion)),
            Probe::Unreadable(Unreadable::OtherProtocol)
        ));
        assert!(matches!(
            classify_connect_error(&protocol_failure(ErrorCode::HandshakeRequired)),
            Probe::Unreadable(Unreadable::OtherProtocol)
        ));
        assert!(matches!(
            classify_connect_error(&protocol_failure(ErrorCode::Unauthorized)),
            Probe::Untrusted
        ));
        assert!(matches!(
            classify_connect_error(&anyhow!("splinterd sent an invalid handshake")),
            Probe::Unreadable(Unreadable::InvalidAnswer)
        ));
        assert!(matches!(
            classify_report_error(&protocol_failure(ErrorCode::Unauthorized)),
            Probe::Untrusted
        ));
        assert!(matches!(
            classify_report_error(&protocol_failure(ErrorCode::Internal)),
            Probe::Unreadable(Unreadable::InvalidAnswer)
        ));
    }

    #[test]
    fn the_dialog_names_the_builds_the_count_the_keys_and_the_default() {
        let Decision::Ask(question) = decide(&report(OLDER, 4, Vec::new()), &installed(), None)
        else {
            panic!("expected a question");
        };
        let lines = dialog_lines(&question);
        let text = lines.join(" ");
        for expected in [
            "TRUSTED SPLINTERM UPGRADE QUESTION",
            "Running:   Splinterm 0.2.6, generation 3",
            "Installed: Splinterm 0.2.1",
            "It still has 4 running shells",
            "ends 4 running shells",
            "choose Not now, then run `splinterm upgrade`",
            "Not now opens nothing and changes nothing.",
            "no answer within 120 seconds, means Not now",
            "[D / Esc / Enter  Not now]     [R  Restart now (ends 4 shells)]",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert!(!text.contains("[G"), "{text}");
        assert!(lines.iter().all(|line| line.len() <= MAX_DIALOG_COLUMNS));
        assert_eq!(
            not_now_lines(&question)[1],
            "Run `splinterm upgrade` to move them to the installed build without ending them."
        );
    }

    #[test]
    fn long_blocking_reasons_stay_bounded_and_point_to_the_full_report() {
        let reasons = (0..12)
            .map(|index| format!("reason {index} {}", "word ".repeat(40)))
            .collect::<Vec<_>>();
        let question = RestartQuestion {
            running: RunningDaemon::Build {
                version: "0.2.0".to_owned(),
                generation: 9,
            },
            installed_version: "0.2.1".to_owned(),
            live_splints: Some(2),
            live_upgrade: LiveUpgrade::Blocked(reasons),
        };
        let lines = dialog_lines(&question);
        assert!(lines.len() <= MAX_RESTART_DIALOG_LINES, "{}", lines.len());
        assert!(lines.iter().all(|line| line.len() <= MAX_DIALOG_COLUMNS));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("for the full report"))
        );
        assert_eq!(not_now_lines(&question).len(), 1);

        let prompt = RestartDialogPrompt {
            capability: vec![1; CAPABILITY_BYTES],
            parent_pid: 42,
            lines,
        };
        prompt.validate().unwrap();
        let (decision, _receiver) = std_mpsc::channel();
        let options = prompt.window_options(decision);
        let snapshot = options.snapshot.as_ref().unwrap();
        assert_eq!(usize::from(options.initial_rows), snapshot.rows);
        assert_eq!(snapshot.title, "Upgrade question");
    }

    #[test]
    fn restart_prompts_are_validated() {
        let good = RestartDialogPrompt {
            capability: vec![1; CAPABILITY_BYTES],
            parent_pid: 42,
            lines: vec!["question".to_owned()],
        };
        good.validate().unwrap();
        for bad in [
            RestartDialogPrompt {
                capability: vec![1; CAPABILITY_BYTES - 1],
                ..good.clone()
            },
            RestartDialogPrompt {
                parent_pid: 0,
                ..good.clone()
            },
            RestartDialogPrompt {
                lines: Vec::new(),
                ..good.clone()
            },
            RestartDialogPrompt {
                lines: vec!["x".repeat(MAX_DIALOG_COLUMNS + 1)],
                ..good.clone()
            },
            RestartDialogPrompt {
                lines: vec![String::new(); MAX_RESTART_DIALOG_LINES + 1],
                ..good.clone()
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn wrapping_respects_the_width_and_indents_continuations() {
        let lines = wrap("one two three four five six seven eight nine ten", 14, "  ");
        assert_eq!(
            lines,
            [
                "one two three",
                "  four five",
                "  six seven",
                "  eight nine",
                "  ten"
            ]
        );
        assert!(wrap("", 10, "").is_empty());
    }
}
