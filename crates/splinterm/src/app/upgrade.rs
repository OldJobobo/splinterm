//! `splinterm upgrade`: inspect or start a live daemon upgrade (Plan 0061
//! M3.13d).
//!
//! The command connects as the trusted local client, so the daemon accepts it
//! only from the installed `splinterm` beside the running `splinterd` (by
//! device and inode). Without `--now` it prints the daemon's dry-run report
//! and changes nothing. With `--now` it starts the handoff on its own
//! connection, which is never a Window's anchor, and answers the daemon's one
//! question about Windows that cannot take part: in the terminal when stdin
//! and stderr are terminals outside a Splinterm terminal, otherwise in a small
//! trusted dialog (`upgrade_dialog`). A Splinterm terminal cannot be asked,
//! because the daemon pauses input to every terminal while it waits for the
//! answer. Every unclear answer cancels. Nothing is ever restarted
//! automatically.

use std::{
    fmt::Write as _,
    io::{self, IsTerminal, Write as _},
    os::fd::{AsFd, BorrowedFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use splinterm::automation::{
    Connection, DaemonHandoffOutcome, HandoffDecision, HandoffDecisionPrompt, protocol_error,
};
use splinterm_protocol::{
    ErrorCode, HandoffBlockingReason, HandoffBuildProvenance, HandoffReport,
    HandoffUnresponsiveWindow, HandoffWindowIssue,
};

use super::{consent::trusted_label, upgrade_dialog::ask_in_dialog};

/// The safe manual path while live upgrade is unavailable, as the package
/// scriptlets and `docs/headless.md` document it.
const RESTART_COMMANDS: [&str; 3] = [
    "systemctl --user stop splinterd.service",
    "systemctl --user daemon-reload",
    "systemctl --user start splinterd.service",
];

/// How long a completed handoff is given to bring the next daemon up before
/// the command stops trying to confirm which build serves.
const CONFIRMATION_WINDOW: Duration = Duration::from_secs(15);

/// Longest terminal answer read; anything longer is not a valid answer.
const MAX_TERMINAL_ANSWER_BYTES: usize = 256;

/// `splinterm upgrade` exit statuses. Clap's usage errors exit 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum UpgradeExit {
    /// The report was shown, or the daemon committed the upgrade and the
    /// upgraded build serves (or could not be asked yet).
    Success,
    /// The request failed: no daemon, not the installed trusted client, a
    /// refused request, an invalid daemon response, or, after a handoff, no
    /// later daemon generation serving.
    Error,
    /// The upgrade was cancelled: by the person, by closing the dialog, by
    /// no answer in time, or because no way to ask was available.
    Cancelled,
    /// Live upgrade is not possible now. Nothing changed.
    Unavailable,
    /// Another upgrade is already running.
    InProgress,
    /// The daemon handed off, but the previous build serves again: the
    /// upgrade rolled back. Terminals were kept.
    RolledBack,
}

impl UpgradeExit {
    pub(in crate::app) const fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Error => 1,
            Self::Cancelled => 3,
            Self::Unavailable => 4,
            Self::InProgress => 5,
            Self::RolledBack => 6,
        }
    }
}

/// Runs `splinterm upgrade [--now]` and exits with its [`UpgradeExit`].
pub(in crate::app) async fn run(now: bool) -> Result<()> {
    let exit = run_upgrade(now).await?;
    io::stdout().flush().ok();
    if exit != UpgradeExit::Success {
        std::process::exit(exit.code());
    }
    Ok(())
}

async fn run_upgrade(now: bool) -> Result<UpgradeExit> {
    let mut connection = Connection::connect().await?;
    let report = match connection.daemon_handoff_report().await {
        Ok(report) => report,
        Err(error) if is_unauthorized(&error) => {
            for line in untrusted_client_lines() {
                eprintln!("{line}");
            }
            return Ok(UpgradeExit::Error);
        }
        Err(error) => return Err(error),
    };
    if !now {
        for line in report_lines(&report) {
            println!("{line}");
        }
        return Ok(UpgradeExit::Success);
    }
    let interactive = asks_in_terminal(
        io::stdin().is_terminal(),
        io::stderr().is_terminal(),
        std::env::var_os("SPLINTERM_SPLINT_ID").is_some(),
    );
    let prompted = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&prompted);
    let outcome = connection
        .execute_daemon_handoff(move |prompt| {
            seen.lock()
                .expect("prompt record lock")
                .clone_from(&prompt.windows);
            ask(prompt, interactive)
        })
        .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) if is_unauthorized(&error) => {
            for line in untrusted_client_lines() {
                eprintln!("{line}");
            }
            return Ok(UpgradeExit::Error);
        }
        Err(error) => return Err(error.context("the upgrade failed")),
    };
    let (exit, lines) = outcome_lines(&outcome, &report);
    for line in &lines {
        if exit == UpgradeExit::Success {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    }
    let DaemonHandoffOutcome::HandedOff { proceeded_without } = &outcome else {
        return Ok(exit);
    };
    let windows = prompted.lock().expect("prompt record lock").clone();
    for line in closed_window_lines(proceeded_without, &windows) {
        println!("{line}");
    }
    Ok(confirm_handoff(&report).await)
}

/// Whether to ask in the terminal rather than in the dialog. A Splint's
/// terminal (`SPLINTERM_SPLINT_ID` is set inside one) cannot be typed into
/// while the daemon waits for the answer, because the handoff fence pauses
/// input to every Splint.
pub(in crate::app) const fn asks_in_terminal(
    stdin_terminal: bool,
    stderr_terminal: bool,
    inside_splint: bool,
) -> bool {
    stdin_terminal && stderr_terminal && !inside_splint
}

fn is_unauthorized(error: &anyhow::Error) -> bool {
    protocol_error(error).is_some_and(|error| error.code == ErrorCode::Unauthorized)
}

fn untrusted_client_lines() -> Vec<String> {
    vec![
        "splinterd accepts upgrade requests only from the installed splinterm beside the running splinterd (on packaged systems /usr/bin/splinterm), matched by device and inode.".to_owned(),
        "Run that installed client, not a development build or a copy earlier on PATH; check with `command -v splinterm`.".to_owned(),
    ]
}

async fn ask(prompt: HandoffDecisionPrompt, interactive: bool) -> HandoffDecision {
    if interactive {
        return ask_in_terminal(prompt).await;
    }
    match ask_in_dialog(prompt).await {
        Ok(true) => HandoffDecision::ProceedWithout,
        Ok(false) => HandoffDecision::Cancel,
        Err(error) => {
            eprintln!("Could not ask about the Windows ({error:#}); cancelling.");
            HandoffDecision::Cancel
        }
    }
}

fn provenance_label(provenance: HandoffBuildProvenance) -> &'static str {
    match provenance {
        HandoffBuildProvenance::Unpackaged => "development build (not packaged)",
        HandoffBuildProvenance::LocalCommittedHead => "local package build",
        HandoffBuildProvenance::ReleaseCandidate => "release package",
        HandoffBuildProvenance::NixStore => "Nix store build",
    }
}

/// One sentence per blocking reason, in the daemon's order.
pub(in crate::app) fn reason_line(
    reason: &HandoffBlockingReason,
    provenance: HandoffBuildProvenance,
) -> String {
    match reason {
        HandoffBlockingReason::TransientLairsOpen { count, lair_ids } => {
            let mut named = lair_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let unnamed = count.saturating_sub(lair_ids.len());
            if unnamed > 0 {
                let _ = write!(named, " and {unnamed} more");
            }
            format!(
                "{count} transient Lair(s) are open, and transient Lairs cannot cross an upgrade: {named}. Close them first."
            )
        }
        HandoffBlockingReason::NotPackaged => "The running daemon is not an installed package build, so there is no installed build to hand off to and no rollback.".to_owned(),
        HandoffBlockingReason::RollbackUnavailable => "The running daemon could not keep its rollback copy, so a failed upgrade could not roll back.".to_owned(),
        HandoffBlockingReason::Unsupported if provenance == HandoffBuildProvenance::NixStore => {
            "Live upgrade is not supported for Nix installations in Splinterm 0.2.0.".to_owned()
        }
        HandoffBlockingReason::Unsupported => {
            "Live upgrade is not available in this build yet.".to_owned()
        }
    }
}

/// Whether restarting is the only way to run an upgraded build now: every
/// reason other than open transient Lairs is beyond the person's control.
fn restart_is_the_path(reasons: &[HandoffBlockingReason]) -> bool {
    reasons
        .iter()
        .any(|reason| !matches!(reason, HandoffBlockingReason::TransientLairsOpen { .. }))
}

fn restart_lines() -> Vec<String> {
    let mut lines = vec![
        "To run an upgraded build now, restart the daemon from a terminal Splinterm does not own (a text console or another terminal emulator). Restarting ends every running terminal:".to_owned(),
    ];
    lines.extend(
        RESTART_COMMANDS
            .iter()
            .map(|command| format!("  {command}")),
    );
    lines
}

/// The human dry-run report.
pub(in crate::app) fn report_lines(report: &HandoffReport) -> Vec<String> {
    let range = |range: splinterm_protocol::HandoffVersionRange| {
        format!("{}–{}", range.minimum, range.maximum)
    };
    let mut lines = vec![
        format!(
            "Daemon: generation {} · Splinterm {} · {}",
            report.daemon_generation,
            report.build.version,
            provenance_label(report.build.provenance)
        ),
        format!(
            "Sessions: {} Lair(s) ({} transient) · {} Dojo(s) · {} live terminal(s)",
            report.lairs, report.transient_lairs, report.dojos, report.live_splints
        ),
        format!(
            "Rollback copy: {}",
            if report.rollback_available {
                "kept"
            } else {
                "not kept"
            }
        ),
        format!(
            "Compatibility: private protocol {} · handoff {} · terminal checkpoint {} · descriptor manifest {}",
            range(report.private_protocol),
            range(report.handoff_protocol),
            range(report.terminal_checkpoint),
            range(report.descriptor_manifest)
        ),
    ];
    if report.blocking_reasons.is_empty() {
        lines.push("Live upgrade: nothing blocks it.".to_owned());
        lines.push(
            "Run `splinterm upgrade --now` to hand the running terminals to the installed build."
                .to_owned(),
        );
        return lines;
    }
    lines.push("Live upgrade: not available now.".to_owned());
    lines.extend(
        report
            .blocking_reasons
            .iter()
            .map(|reason| format!("  • {}", reason_line(reason, report.build.provenance))),
    );
    if restart_is_the_path(&report.blocking_reasons) {
        lines.extend(restart_lines());
    }
    lines
}

/// The exit status and message for a finished `Execute`.
pub(in crate::app) fn outcome_lines(
    outcome: &DaemonHandoffOutcome,
    report: &HandoffReport,
) -> (UpgradeExit, Vec<String>) {
    let unchanged = "Nothing changed; every terminal keeps running.".to_owned();
    match outcome {
        DaemonHandoffOutcome::HandedOff { .. } => (
            UpgradeExit::Success,
            vec![
                "splinterd accepted the upgrade and closed this connection to start the installed build.".to_owned(),
            ],
        ),
        DaemonHandoffOutcome::Unavailable { reasons, message } => {
            let mut lines = vec!["Live upgrade is not available now.".to_owned()];
            lines.extend(
                reasons
                    .iter()
                    .map(|reason| format!("  • {}", reason_line(reason, report.build.provenance))),
            );
            lines.push(format!("splinterd: {}", trusted_label(message, 512)));
            lines.push(unchanged);
            if restart_is_the_path(reasons) {
                lines.extend(restart_lines());
            }
            (UpgradeExit::Unavailable, lines)
        }
        DaemonHandoffOutcome::InProgress { message } => (
            UpgradeExit::InProgress,
            vec![
                "Another daemon upgrade is already in progress; try again after it finishes."
                    .to_owned(),
                format!("splinterd: {}", trusted_label(message, 512)),
            ],
        ),
        DaemonHandoffOutcome::Refused(error) => (
            UpgradeExit::Error,
            vec![format!(
                "splinterd refused the upgrade [{}]: {}",
                format!("{:?}", error.code).to_lowercase(),
                trusted_label(&error.message, 512)
            )],
        ),
        DaemonHandoffOutcome::Cancelled { .. } => (
            UpgradeExit::Cancelled,
            vec![format!("Upgrade cancelled. {unchanged}")],
        ),
        DaemonHandoffOutcome::TimedOut { .. } => (
            UpgradeExit::Cancelled,
            vec![format!(
                "Upgrade cancelled: no answer before the deadline. {unchanged}"
            )],
        ),
    }
}

fn closed_window_lines(
    proceeded_without: &[u8],
    windows: &[HandoffUnresponsiveWindow],
) -> Vec<String> {
    if proceeded_without.is_empty() {
        return Vec::new();
    }
    let pids = windows
        .iter()
        .filter(|window| proceeded_without.contains(&window.index))
        .map(|window| window.pid.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    vec![format!(
        "The Windows with PIDs {pids} close. Their terminals keep running; reopen them from the Recent Dojos picker (`splinterm dojos`)."
    )]
}

/// Reconnects to whichever daemon serves next and says which build that is.
async fn confirm_handoff(previous: &HandoffReport) -> UpgradeExit {
    let started = Instant::now();
    let current = loop {
        let remaining = CONFIRMATION_WINDOW.saturating_sub(started.elapsed());
        // A daemon still adopting holds connections in the listen backlog,
        // so the handshake itself is bounded by what is left of the window.
        let attempt = tokio::time::timeout(remaining, async {
            let mut connection = Connection::connect().await?;
            connection.daemon_handoff_report().await
        });
        match attempt.await {
            Ok(Ok(report)) => break Some(report),
            _ if started.elapsed() < CONFIRMATION_WINDOW => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            _ => break None,
        }
    };
    let (exit, line) = confirmation_line(previous, current.as_ref());
    println!("{line}");
    exit
}

/// What the report from the daemon serving after a handoff says.
pub(in crate::app) fn confirmation_line(
    previous: &HandoffReport,
    current: Option<&HandoffReport>,
) -> (UpgradeExit, String) {
    let Some(current) = current else {
        return (
            UpgradeExit::Success,
            "Could not reach the daemon yet to confirm which build serves; run `splinterm upgrade` to check.".to_owned(),
        );
    };
    if current.daemon_generation <= previous.daemon_generation {
        return (
            UpgradeExit::Error,
            format!(
                "The daemon now serving is generation {}, not a later one, so the upgrade did not complete; the daemon may have restarted. Inspect `journalctl --user -u splinterd.service`.",
                current.daemon_generation
            ),
        );
    }
    if current.build.identity == previous.build.identity {
        return (
            UpgradeExit::RolledBack,
            format!(
                "The upgrade rolled back: generation {} runs the previous build, Splinterm {}. Terminals were kept; inspect `journalctl --user -u splinterd.service`.",
                current.daemon_generation, current.build.version
            ),
        );
    }
    (
        UpgradeExit::Success,
        format!(
            "Upgrade complete: generation {} runs Splinterm {} (was generation {}, Splinterm {}).",
            current.daemon_generation,
            current.build.version,
            previous.daemon_generation,
            previous.build.version
        ),
    )
}

fn issue_label(issue: HandoffWindowIssue) -> &'static str {
    match issue {
        HandoffWindowIssue::NoAcknowledgement => "did not answer the upgrade request in time",
        HandoffWindowIssue::Unpinned => "could not be identified safely",
        HandoffWindowIssue::Undeclared => "runs a client that cannot take part",
    }
}

/// One Window's description, its Dojo names sanitized and listed in full.
pub(in crate::app) fn window_description(window: &HandoffUnresponsiveWindow) -> String {
    let dojos = if window.dojos.is_empty() {
        "no Dojos the daemon can name".to_owned()
    } else {
        window
            .dojos
            .iter()
            .map(|dojo| trusted_label(&dojo.name, splinterm_protocol::MAX_HANDOFF_DOJO_NAME_BYTES))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "PID {} — {} — Dojos: {dojos}",
        window.pid,
        issue_label(window.reason)
    )
}

/// The terminal question, ending without a newline before the answer.
pub(in crate::app) fn terminal_prompt_text(prompt: &HandoffDecisionPrompt) -> String {
    let mut text = String::from("These Splinterm Windows cannot take part in the upgrade:\n");
    for window in &prompt.windows {
        text.push_str("  • ");
        text.push_str(&window_description(window));
        text.push('\n');
    }
    text.push_str(
        "If you proceed, these Windows close. Their terminals keep running in the upgraded daemon; reopen them from the Recent Dojos picker (`splinterm dojos`).\n",
    );
    text.push_str("Input is paused in every Window until you answer.\n");
    let _ = write!(
        text,
        "[c]ancel / [p]roceed without these Windows ({}s left, default cancel): ",
        prompt.respond_within.as_secs()
    );
    text
}

/// What reading the terminal answer produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum TerminalAnswer {
    Line(String),
    EndOfInput,
    TimedOut,
    Unreadable,
}

/// The decision a terminal answer means, and a note for anything but an
/// explicit answer. Only `p` or `proceed` proceeds.
pub(in crate::app) fn decision_from_answer(
    answer: &TerminalAnswer,
) -> (HandoffDecision, Option<&'static str>) {
    match answer {
        TerminalAnswer::Line(line) => match line.trim().to_ascii_lowercase().as_str() {
            "p" | "proceed" => (HandoffDecision::ProceedWithout, None),
            "c" | "cancel" => (HandoffDecision::Cancel, None),
            "" => (
                HandoffDecision::Cancel,
                Some("No answer given; cancelling."),
            ),
            _ => (
                HandoffDecision::Cancel,
                Some("Unrecognized answer; cancelling."),
            ),
        },
        TerminalAnswer::EndOfInput => (
            HandoffDecision::Cancel,
            Some("No answer (end of input); cancelling."),
        ),
        TerminalAnswer::TimedOut => (
            HandoffDecision::Cancel,
            Some("No answer in time; cancelling."),
        ),
        TerminalAnswer::Unreadable => (
            HandoffDecision::Cancel,
            Some("Could not read the answer; cancelling."),
        ),
    }
}

/// Sets its flag when dropped, so an abandoned blocking read stops polling.
/// The daemon may end the attempt while the question is open; then the
/// prompt is dropped, its line is ended, and the read stops.
struct StopOnDrop {
    stop: Arc<AtomicBool>,
    answered: bool,
}

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if !self.answered {
            eprintln!();
        }
    }
}

async fn ask_in_terminal(prompt: HandoffDecisionPrompt) -> HandoffDecision {
    eprint!("{}", terminal_prompt_text(&prompt));
    let _ = io::stderr().flush();
    let stop = Arc::new(AtomicBool::new(false));
    let mut stop_on_drop = StopOnDrop {
        stop: Arc::clone(&stop),
        answered: false,
    };
    let deadline = Instant::now() + prompt.respond_within;
    let answer = tokio::task::spawn_blocking(move || {
        let stdin = io::stdin();
        read_terminal_answer(stdin.as_fd(), deadline, &stop)
    })
    .await
    .unwrap_or(TerminalAnswer::Unreadable);
    stop_on_drop.answered = true;
    if !matches!(answer, TerminalAnswer::Line(_)) {
        eprintln!();
    }
    let (decision, note) = decision_from_answer(&answer);
    if let Some(note) = note {
        eprintln!("{note}");
    }
    decision
}

/// Reads one answer line from `input` until `deadline`. It polls in short
/// slices and returns as soon as `stop` is set, so an abandoned prompt never
/// keeps a thread or the runtime waiting. The descriptor is read directly,
/// leaving its blocking mode untouched for the shell after exit.
pub(in crate::app) fn read_terminal_answer(
    input: BorrowedFd<'_>,
    deadline: Instant,
    stop: &AtomicBool,
) -> TerminalAnswer {
    const SLICE: Duration = Duration::from_millis(100);
    let mut line = Vec::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            return TerminalAnswer::TimedOut;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return TerminalAnswer::TimedOut;
        };
        let wait = remaining.min(SLICE);
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: i64::from(wait.subsec_nanos()),
        };
        let mut descriptors = [PollFd::new(&input, PollFlags::IN)];
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(0) | Err(rustix::io::Errno::INTR) => continue,
            Ok(_) => {}
            Err(_) => return TerminalAnswer::Unreadable,
        }
        let returned = descriptors[0].revents();
        if returned.intersects(PollFlags::ERR | PollFlags::NVAL) {
            return TerminalAnswer::Unreadable;
        }
        let mut chunk = [0_u8; 64];
        match rustix::io::read(input, &mut chunk) {
            Ok(0) => {
                return if line.is_empty() {
                    TerminalAnswer::EndOfInput
                } else {
                    TerminalAnswer::Line(String::from_utf8_lossy(&line).into_owned())
                };
            }
            Ok(read) => {
                line.extend_from_slice(&chunk[..read]);
                if let Some(end) = line.iter().position(|&byte| byte == b'\n') {
                    line.truncate(end);
                    return TerminalAnswer::Line(String::from_utf8_lossy(&line).into_owned());
                }
                if line.len() > MAX_TERMINAL_ANSWER_BYTES {
                    return TerminalAnswer::Line(String::from_utf8_lossy(&line).into_owned());
                }
            }
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
            Err(_) => return TerminalAnswer::Unreadable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use splinterm_core::{DojoId, LairId};
    use splinterm_protocol::{HandoffBuild, HandoffVersionRange, HandoffWindowDojo};

    fn report(
        generation: u64,
        identity: u8,
        provenance: HandoffBuildProvenance,
        reasons: Vec<HandoffBlockingReason>,
    ) -> HandoffReport {
        let range = HandoffVersionRange {
            minimum: 1,
            maximum: 2,
        };
        HandoffReport {
            daemon_generation: generation,
            build: HandoffBuild {
                version: format!("0.2.{identity}"),
                identity: [identity; splinterm_protocol::HANDOFF_BUILD_IDENTITY_BYTES],
                provenance,
            },
            private_protocol: range,
            handoff_protocol: range,
            terminal_checkpoint: range,
            descriptor_manifest: range,
            lairs: 2,
            transient_lairs: 0,
            dojos: 3,
            live_splints: 4,
            rollback_available: true,
            blocking_reasons: reasons,
        }
    }

    fn window(
        index: u8,
        pid: u32,
        reason: HandoffWindowIssue,
        dojos: &[&str],
    ) -> HandoffUnresponsiveWindow {
        HandoffUnresponsiveWindow {
            index,
            pid,
            reason,
            dojos: dojos
                .iter()
                .map(|name| HandoffWindowDojo {
                    lair_id: LairId::new(),
                    dojo_id: DojoId::new(),
                    name: (*name).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn exit_codes_are_distinct_and_documented() {
        let all = [
            UpgradeExit::Success,
            UpgradeExit::Error,
            UpgradeExit::Cancelled,
            UpgradeExit::Unavailable,
            UpgradeExit::InProgress,
            UpgradeExit::RolledBack,
        ];
        let codes = all.map(UpgradeExit::code);
        assert_eq!(codes, [0, 1, 3, 4, 5, 6]);
        // 2 stays Clap's usage error.
        assert!(!codes.contains(&2));
    }

    #[test]
    fn outcomes_map_to_their_exit_codes() {
        let base = report(1, 1, HandoffBuildProvenance::ReleaseCandidate, Vec::new());
        let cases = [
            (
                DaemonHandoffOutcome::HandedOff {
                    proceeded_without: Vec::new(),
                },
                UpgradeExit::Success,
            ),
            (
                DaemonHandoffOutcome::Unavailable {
                    reasons: vec![HandoffBlockingReason::Unsupported],
                    message: "unavailable".to_owned(),
                },
                UpgradeExit::Unavailable,
            ),
            (
                DaemonHandoffOutcome::InProgress {
                    message: "busy".to_owned(),
                },
                UpgradeExit::InProgress,
            ),
            (
                DaemonHandoffOutcome::Refused(splinterm_protocol::ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "anchor",
                )),
                UpgradeExit::Error,
            ),
            (
                DaemonHandoffOutcome::Cancelled {
                    message: "cancelled".to_owned(),
                },
                UpgradeExit::Cancelled,
            ),
            (
                DaemonHandoffOutcome::TimedOut {
                    message: "cancelled".to_owned(),
                },
                UpgradeExit::Cancelled,
            ),
        ];
        for (outcome, expected) in cases {
            assert_eq!(outcome_lines(&outcome, &base).0, expected, "{outcome:?}");
        }
    }

    #[test]
    fn window_continuity_refusal_says_live_upgrade_is_not_in_this_build_and_how_to_restart() {
        let base = report(1, 1, HandoffBuildProvenance::ReleaseCandidate, Vec::new());
        let (exit, lines) = outcome_lines(
            &DaemonHandoffOutcome::Unavailable {
                reasons: vec![HandoffBlockingReason::Unsupported],
                message: "daemon handoff is unavailable: live handoff is not available in this release because open Windows cannot survive it yet; restart the daemon to run the upgraded build".to_owned(),
            },
            &base,
        );
        assert_eq!(exit, UpgradeExit::Unavailable);
        let text = lines.join("\n");
        assert!(
            text.contains("Live upgrade is not available in this build yet."),
            "{text}"
        );
        assert!(
            text.contains("open Windows cannot survive it yet"),
            "{text}"
        );
        assert!(text.contains("Nothing changed"), "{text}");
        for command in RESTART_COMMANDS {
            assert!(text.contains(command), "{text}");
        }
        assert!(text.contains("ends every running terminal"), "{text}");
        assert!(!text.contains("Nix"), "{text}");
    }

    #[test]
    fn nix_refusal_names_nix_and_the_restart_path() {
        let nix = report(1, 1, HandoffBuildProvenance::NixStore, Vec::new());
        let (exit, lines) = outcome_lines(
            &DaemonHandoffOutcome::Unavailable {
                reasons: vec![HandoffBlockingReason::Unsupported],
                message: "daemon handoff is unavailable: live handoff is not supported for Nix-store installations in Splinterm 0.2.0; restart the daemon to run the upgraded build".to_owned(),
            },
            &nix,
        );
        assert_eq!(exit, UpgradeExit::Unavailable);
        let text = lines.join("\n");
        assert!(
            text.contains(
                "Live upgrade is not supported for Nix installations in Splinterm 0.2.0."
            ),
            "{text}"
        );
        assert!(text.contains(RESTART_COMMANDS[0]), "{text}");
        // The dry run names Nix from the reported provenance too.
        let dry = report(
            1,
            1,
            HandoffBuildProvenance::NixStore,
            vec![HandoffBlockingReason::Unsupported],
        );
        let text = report_lines(&dry).join("\n");
        assert!(text.contains("Nix store build"), "{text}");
        assert!(
            text.contains("not supported for Nix installations"),
            "{text}"
        );
        assert!(text.contains(RESTART_COMMANDS[2]), "{text}");
    }

    #[test]
    fn transient_lairs_alone_ask_to_close_them_without_restart_advice() {
        let lair = LairId::new();
        let reasons = vec![HandoffBlockingReason::TransientLairsOpen {
            count: 3,
            lair_ids: vec![lair],
        }];
        let line = reason_line(&reasons[0], HandoffBuildProvenance::ReleaseCandidate);
        assert!(line.contains(&lair.to_string()), "{line}");
        assert!(line.contains("and 2 more"), "{line}");
        assert!(!restart_is_the_path(&reasons));
        let mut dry = report(1, 1, HandoffBuildProvenance::ReleaseCandidate, reasons);
        dry.lairs = 3;
        dry.transient_lairs = 3;
        assert!(!report_lines(&dry).join("\n").contains("systemctl"));
    }

    #[test]
    fn a_clear_report_points_at_upgrade_now() {
        let clear = report(7, 1, HandoffBuildProvenance::ReleaseCandidate, Vec::new());
        let text = report_lines(&clear).join("\n");
        assert!(text.contains("generation 7"), "{text}");
        assert!(text.contains("release package"), "{text}");
        assert!(text.contains("nothing blocks it"), "{text}");
        assert!(text.contains("splinterm upgrade --now"), "{text}");
        assert!(text.contains("private protocol 1–2"), "{text}");
    }

    #[test]
    fn confirmation_distinguishes_upgrade_rollback_and_no_change() {
        let before = report(1, 1, HandoffBuildProvenance::ReleaseCandidate, Vec::new());
        let upgraded = report(2, 2, HandoffBuildProvenance::ReleaseCandidate, Vec::new());
        let rolled_back = report(2, 1, HandoffBuildProvenance::ReleaseCandidate, Vec::new());
        assert_eq!(
            confirmation_line(&before, Some(&upgraded)).0,
            UpgradeExit::Success
        );
        assert!(
            confirmation_line(&before, Some(&upgraded))
                .1
                .contains("Upgrade complete")
        );
        assert_eq!(
            confirmation_line(&before, Some(&rolled_back)).0,
            UpgradeExit::RolledBack
        );
        assert_eq!(
            confirmation_line(&before, Some(&before)).0,
            UpgradeExit::Error
        );
        let (exit, line) = confirmation_line(&before, None);
        assert_eq!(exit, UpgradeExit::Success);
        assert!(line.contains("splinterm upgrade"), "{line}");
    }

    #[test]
    fn the_terminal_prompt_lists_every_window_and_says_what_proceeding_does() {
        let prompt = HandoffDecisionPrompt {
            daemon_generation: 3,
            deadline: Duration::from_secs(20),
            respond_within: Duration::from_millis(18_500),
            windows: vec![
                window(
                    2,
                    4242,
                    HandoffWindowIssue::NoAcknowledgement,
                    &["work", "lo\u{1b}[31mgs"],
                ),
                window(5, 4300, HandoffWindowIssue::Undeclared, &[]),
            ],
        };
        let text = terminal_prompt_text(&prompt);
        assert!(text.contains("PID 4242 — did not answer the upgrade request in time — Dojos: work, lo\u{fffd}[31mgs"), "{text}");
        assert!(text.contains("PID 4300 — runs a client that cannot take part — Dojos: no Dojos the daemon can name"), "{text}");
        assert!(!text.contains('\u{1b}'));
        assert!(text.contains("these Windows close"), "{text}");
        assert!(
            text.contains("Their terminals keep running in the upgraded daemon"),
            "{text}"
        );
        assert!(text.contains("Recent Dojos picker"), "{text}");
        assert!(
            text.ends_with(
                "[c]ancel / [p]roceed without these Windows (18s left, default cancel): "
            ),
            "{text}"
        );
    }

    #[test]
    fn only_a_terminal_outside_a_splint_is_asked_in_place() {
        assert!(asks_in_terminal(true, true, false));
        // Input to every Splint is paused while the daemon waits.
        assert!(!asks_in_terminal(true, true, true));
        assert!(!asks_in_terminal(false, true, false));
        assert!(!asks_in_terminal(true, false, false));
    }

    #[test]
    fn only_an_explicit_proceed_proceeds() {
        let line = |text: &str| TerminalAnswer::Line(text.to_owned());
        assert_eq!(
            decision_from_answer(&line("p")).0,
            HandoffDecision::ProceedWithout
        );
        assert_eq!(
            decision_from_answer(&line(" Proceed \r")).0,
            HandoffDecision::ProceedWithout
        );
        assert_eq!(
            decision_from_answer(&line("P")).0,
            HandoffDecision::ProceedWithout
        );
        assert_eq!(
            decision_from_answer(&line("c")),
            (HandoffDecision::Cancel, None)
        );
        assert_eq!(
            decision_from_answer(&line("cancel")),
            (HandoffDecision::Cancel, None)
        );
        for answer in [
            line(""),
            line("y"),
            line("yes"),
            line("pp"),
            TerminalAnswer::EndOfInput,
            TerminalAnswer::TimedOut,
            TerminalAnswer::Unreadable,
        ] {
            let (decision, note) = decision_from_answer(&answer);
            assert_eq!(decision, HandoffDecision::Cancel, "{answer:?}");
            assert!(note.is_some(), "{answer:?}");
        }
    }

    #[test]
    fn terminal_answers_are_read_until_newline_end_of_input_or_deadline() {
        let soon = || Instant::now() + Duration::from_secs(5);
        let stop = AtomicBool::new(false);

        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut (&writer), b"proceed\nignored").unwrap();
        assert_eq!(
            read_terminal_answer(reader.as_fd(), soon(), &stop),
            TerminalAnswer::Line("proceed".to_owned())
        );

        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(writer);
        assert_eq!(
            read_terminal_answer(reader.as_fd(), soon(), &stop),
            TerminalAnswer::EndOfInput
        );

        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut (&writer), b"c").unwrap();
        drop(writer);
        assert_eq!(
            read_terminal_answer(reader.as_fd(), soon(), &stop),
            TerminalAnswer::Line("c".to_owned())
        );

        let (reader, _writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let started = Instant::now();
        assert_eq!(
            read_terminal_answer(
                reader.as_fd(),
                Instant::now() + Duration::from_millis(150),
                &stop
            ),
            TerminalAnswer::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));

        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut (&writer), &[b'x'; MAX_TERMINAL_ANSWER_BYTES + 10]).unwrap();
        let TerminalAnswer::Line(long) = read_terminal_answer(reader.as_fd(), soon(), &stop) else {
            panic!("a long answer is still a line");
        };
        assert_eq!(
            decision_from_answer(&TerminalAnswer::Line(long)).0,
            HandoffDecision::Cancel
        );

        // A stopped prompt returns at once, however long its deadline.
        let (reader, _writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let stopped = AtomicBool::new(true);
        let started = Instant::now();
        assert_eq!(
            read_terminal_answer(
                reader.as_fd(),
                Instant::now() + Duration::from_secs(60),
                &stopped
            ),
            TerminalAnswer::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn closed_windows_are_named_by_pid() {
        let windows = [
            window(2, 4242, HandoffWindowIssue::NoAcknowledgement, &[]),
            window(5, 4300, HandoffWindowIssue::Unpinned, &[]),
        ];
        let lines = closed_window_lines(&[2, 5], &windows);
        assert!(lines[0].contains("PIDs 4242, 4300"), "{lines:?}");
        assert!(lines[0].contains("splinterm dojos"), "{lines:?}");
        assert!(closed_window_lines(&[], &windows).is_empty());
    }
}
