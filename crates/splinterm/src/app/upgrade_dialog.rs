//! The desktop question for `splinterm upgrade --now` when it cannot ask in a
//! terminal: there is none, or it runs inside a Splinterm terminal whose input
//! the upgrade pauses.
//!
//! It reuses the trusted consent dialog's mechanism and styling: a private
//! socketpair on the child's stdin and stdout, a random capability echoed in
//! the reply, a static snapshot in a `TrustedConsentUi` Window, and "closing
//! the Window means no". Unlike consent, the daemon does not launch it. The
//! upgrade command keeps its own daemon connection and starts the dialog as
//! a child of itself through `/proc/self/exe`, which names the exact image
//! the command runs, the installed client the daemon just verified, even if
//! the file was replaced since. The child never connects to the daemon and
//! holds no anchor; it only shows the question and returns one bit, and the
//! command treats a missing, late, or unauthenticated reply as cancel.
//!
//! Its keys differ from consent because proceeding closes Windows: `D`,
//! `Escape`, `Enter`, and the red left area cancel; only `G` or the green
//! right area proceeds. Keys and the proceed click count only 600 ms after
//! the Window first gains focus, and key repeats never count
//! (`frontend::trusted_dialog`). The Window title is "Splinterm — Upgrade".

use std::{
    fmt::Write as _,
    io,
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    sync::mpsc as std_mpsc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use splinterm::automation::HandoffDecisionPrompt;
use splinterm::{TrustedConsentUi, WindowOptions, run_window};
use splinterm_protocol::{
    HandoffUnresponsiveWindow, MAX_HANDOFF_DOJO_NAME_BYTES, MAX_HANDOFF_WINDOW_DOJOS,
    MAX_HANDOFF_WINDOWS, TerminalSnapshot,
};

use super::consent::{
    read_private_frame_bounded, static_dialog_snapshot, trusted_label, write_private_frame_bounded,
};

/// The hidden subcommand the dialog child runs.
pub(in crate::app) const UPGRADE_DIALOG_COMMAND: &str = "upgrade-dialog";

/// Random bytes the child must echo, as the consent dialog does.
const CAPABILITY_BYTES: usize = 32;

/// The largest prompt frame: 64 Windows of 32 Dojos with 128-byte names and
/// their identifiers stay well below it.
const MAX_UPGRADE_DIALOG_FRAME_BYTES: usize = 1024 * 1024;

/// The widest dialog line, matching the consent snapshot's column clamp.
const MAX_DIALOG_COLUMNS: usize = 120;

/// The share of the dialog's rows above the consent chrome's action areas,
/// which cover the bottom 22% of the Window.
const TEXT_ROWS_PERCENT: usize = 72;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UpgradeDialogPrompt {
    capability: Vec<u8>,
    /// The asking command, which the child checks is still its parent.
    parent_pid: u32,
    respond_within_seconds: u64,
    windows: Vec<HandoffUnresponsiveWindow>,
}

impl UpgradeDialogPrompt {
    fn validate(&self) -> Result<()> {
        let valid = self.capability.len() == CAPABILITY_BYTES
            && self.parent_pid > 0
            && !self.windows.is_empty()
            && self.windows.len() <= MAX_HANDOFF_WINDOWS
            && self.windows.iter().all(|window| {
                window.dojos.len() <= MAX_HANDOFF_WINDOW_DOJOS
                    && window
                        .dojos
                        .iter()
                        .all(|dojo| dojo.name.len() <= MAX_HANDOFF_DOJO_NAME_BYTES)
            });
        if !valid {
            bail!("invalid upgrade dialog prompt");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UpgradeDialogReply {
    capability: Vec<u8>,
    proceed: bool,
}

/// One Window's dialog line, at most [`MAX_DIALOG_COLUMNS`] bytes. Every
/// Window gets its own line; when its Dojo names do not fit, the line keeps
/// as many as fit and counts the rest, so nothing is silently dropped.
fn window_line(window: &HandoffUnresponsiveWindow) -> String {
    let reason = match window.reason {
        splinterm_protocol::HandoffWindowIssue::NoAcknowledgement => "did not answer in time",
        splinterm_protocol::HandoffWindowIssue::Unpinned => "could not be identified",
        splinterm_protocol::HandoffWindowIssue::Undeclared => "cannot take part",
    };
    let mut line = format!("  • PID {} — {reason} — Dojos: ", window.pid);
    if window.dojos.is_empty() {
        line.push_str("none named");
        return line;
    }
    let names = window
        .dojos
        .iter()
        .map(|dojo| trusted_label(&dojo.name, MAX_HANDOFF_DOJO_NAME_BYTES))
        .collect::<Vec<_>>();
    for (shown, name) in names.iter().enumerate() {
        let remaining = names.len() - shown;
        let separator = if shown == 0 { "" } else { ", " };
        let tail = if remaining > 1 {
            format!(" (+{} more)", remaining - 1)
        } else {
            String::new()
        };
        if line.len() + separator.len() + name.len() + tail.len() > MAX_DIALOG_COLUMNS {
            if shown == 0 {
                let _ = write!(line, "{} Dojo(s)", names.len());
            } else {
                let _ = write!(line, " (+{remaining} more)");
            }
            return line;
        }
        line.push_str(separator);
        line.push_str(name);
    }
    line
}

/// The dialog text: the Windows, what proceeding does, and how to answer.
fn dialog_lines(prompt: &UpgradeDialogPrompt) -> Vec<String> {
    let count = prompt.windows.len();
    let mut lines = vec![
        "TRUSTED SPLINTERM UPGRADE QUESTION".to_owned(),
        String::new(),
        format!(
            "splinterd is upgrading. {count} Window{} cannot take part:",
            if count == 1 { "" } else { "s" }
        ),
    ];
    // Every Window gets a line while they fit; past that, the last line
    // counts the rest, so the number that would close is always visible.
    if count <= MAX_LISTED_WINDOWS {
        lines.extend(prompt.windows.iter().map(window_line));
    } else {
        let shown = MAX_LISTED_WINDOWS - 1;
        lines.extend(prompt.windows[..shown].iter().map(window_line));
        let rest = &prompt.windows[shown..];
        let dojos: usize = rest.iter().map(|window| window.dojos.len()).sum();
        lines.push(format!(
            "  +{} more Windows ({dojos} Dojo{})",
            rest.len(),
            if dojos == 1 { "" } else { "s" }
        ));
    }
    lines.extend(DIALOG_FOOTER.map(str::to_owned));
    lines.insert(
        lines.len() - 3,
        format!(
            "Closing this dialog, or no answer within {} seconds, cancels.",
            prompt.respond_within_seconds
        ),
    );
    lines
}

/// The explanation and the actions, always shown in full below the list.
/// The deadline line is inserted before the last three.
const DIALOG_FOOTER: [&str; 7] = [
    "",
    "Proceeding closes these Windows. Their terminals keep running in the",
    "upgraded daemon; reopen them from the Recent Dojos picker.",
    "Input is paused in every Window until you answer.",
    "",
    "[D / Esc / Enter  Cancel]     [G  Proceed]",
    "Click the red left area to cancel or the green right area to proceed.",
];

/// Lines above and below the Window list: the three header lines, the
/// footer, and its deadline line.
const FIXED_DIALOG_LINES: usize = 3 + DIALOG_FOOTER.len() + 1;

/// The tallest dialog, in terminal rows. The static snapshot cannot scroll,
/// so the dialog is sized to its content within this bound.
const MAX_DIALOG_ROWS: usize = 40;

/// The most Window lines that fit above the action areas of the tallest
/// dialog; a longer list is summarized in its last line.
const MAX_LISTED_WINDOWS: usize = MAX_DIALOG_ROWS * TEXT_ROWS_PERCENT / 100 - FIXED_DIALOG_LINES;

/// The dialog's rows: enough that its text ends above the action areas,
/// which cover the bottom 22% of the Window.
fn dialog_rows(lines: usize) -> usize {
    (lines * 100).div_ceil(TEXT_ROWS_PERCENT).max(18)
}

fn dialog_snapshot(prompt: &UpgradeDialogPrompt) -> TerminalSnapshot {
    let lines = dialog_lines(prompt);
    let rows = dialog_rows(lines.len());
    static_dialog_snapshot(
        lines,
        rows,
        "Upgrade question",
        splinterm_core::SplintId::new(),
        0,
    )
}

/// Window options that show the whole snapshot: the grid is sized to it.
fn dialog_window_options(
    prompt: &UpgradeDialogPrompt,
    decision: std_mpsc::Sender<bool>,
) -> WindowOptions {
    let snapshot = dialog_snapshot(prompt);
    let columns = u16::try_from(snapshot.columns).unwrap_or(u16::MAX);
    let rows = u16::try_from(snapshot.rows).unwrap_or(u16::MAX);
    WindowOptions {
        snapshot: Some(snapshot),
        trusted_consent: Some(TrustedConsentUi::upgrade(decision)),
        initial_columns: columns,
        initial_rows: rows,
        ..WindowOptions::default()
    }
}

/// The hidden `upgrade-dialog` child: reads one prompt on stdin, shows it,
/// and writes the answer on stdout. Closing the Window answers cancel.
///
/// It dies with the command that asked. The workspace forbids `unsafe`, so
/// the child cannot be given a `pre_exec` hook; instead it sets its own
/// parent-death signal (`PR_SET_PDEATHSIG`, through rustix) before anything
/// else, then checks that its parent is still the command named in the
/// prompt, which covers a parent that died before the signal was set. The
/// signal follows the thread that spawned the child, the runtime thread that
/// drives the command for as long as the process lives.
pub(in crate::app) fn run_upgrade_dialog_client() -> Result<()> {
    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))
        .context("tie the upgrade dialog to its command")?;
    let prompt: UpgradeDialogPrompt =
        read_private_frame_bounded(&mut io::stdin().lock(), MAX_UPGRADE_DIALOG_FRAME_BYTES)?;
    prompt.validate()?;
    if !is_parent(prompt.parent_pid, rustix::process::getppid()) {
        bail!("the upgrade command that asked has exited");
    }
    let (decision, receiver) = std_mpsc::channel();
    run_window(dialog_window_options(&prompt, decision))?;
    let proceed = receiver.try_recv().unwrap_or(false);
    write_private_frame_bounded(
        &mut io::stdout().lock(),
        &UpgradeDialogReply {
            capability: prompt.capability,
            proceed,
        },
        MAX_UPGRADE_DIALOG_FRAME_BYTES,
    )
}

/// Whether `parent` (from `getppid`) is the command that sent the prompt.
/// An orphaned child is reparented to init or a subreaper, never to it.
fn is_parent(expected: u32, parent: Option<rustix::process::Pid>) -> bool {
    i32::try_from(expected).is_ok_and(|expected| rustix::process::Pid::as_raw(parent) == expected)
}

/// Kills and reaps the dialog child unless it already answered, so an
/// abandoned question (the daemon ended the attempt, or the deadline passed)
/// never leaves a dialog on screen.
struct DialogChild(Option<Child>);

impl DialogChild {
    /// Hands a child that has answered to a detached thread, which gives it
    /// a moment to exit and then reaps it, so the answer reaches the daemon
    /// without waiting on the child.
    fn finish(mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        // Without a thread the child still exits on its own after replying,
        // and init reaps it once this process exits.
        let _ = std::thread::Builder::new()
            .name("upgrade-dialog-reaper".to_owned())
            .spawn(move || {
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(2) {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
    }
}

impl Drop for DialogChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn capability() -> Result<Vec<u8>> {
    let mut capability = vec![0_u8; CAPABILITY_BYTES];
    let mut offset = 0;
    while offset < capability.len() {
        offset += rustix::rand::getrandom(
            &mut capability[offset..],
            rustix::rand::GetRandomFlags::empty(),
        )
        .context("OS randomness unavailable")?;
    }
    Ok(capability)
}

/// Asks the question in the dialog and returns whether to proceed. The
/// answer must arrive within the prompt's `respond_within`; any failure is an
/// error, which the caller treats as cancel.
pub(in crate::app) async fn ask_in_dialog(prompt: HandoffDecisionPrompt) -> Result<bool> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|display| display.is_empty()) {
        bail!(
            "there is no terminal to ask in and no graphical session for the dialog; run `splinterm upgrade --now` from a terminal Splinterm does not own"
        );
    }
    let capability = capability()?;
    let request = UpgradeDialogPrompt {
        capability: capability.clone(),
        parent_pid: std::process::id(),
        respond_within_seconds: prompt.respond_within.as_secs(),
        windows: prompt.windows,
    };
    request.validate()?;
    let (mut parent, child) = UnixStream::pair().context("create private dialog socket")?;
    let child_read = child
        .try_clone()
        .context("duplicate private dialog descriptor")?;
    let process = Command::new("/proc/self/exe")
        .arg(UPGRADE_DIALOG_COMMAND)
        .stdin(Stdio::from(OwnedFd::from(child_read)))
        .stdout(Stdio::from(OwnedFd::from(child)))
        .stderr(Stdio::null())
        .spawn()
        .context("launch the upgrade dialog")?;
    let guard = DialogChild(Some(process));
    let budget = prompt.respond_within;
    let reply = tokio::task::spawn_blocking(move || -> Result<UpgradeDialogReply> {
        parent.set_write_timeout(Some(budget))?;
        parent.set_read_timeout(Some(budget))?;
        write_private_frame_bounded(&mut parent, &request, MAX_UPGRADE_DIALOG_FRAME_BYTES)?;
        read_private_frame_bounded(&mut parent, MAX_UPGRADE_DIALOG_FRAME_BYTES)
    })
    .await
    .context("upgrade dialog task failed")?
    .context("the upgrade dialog gave no answer")?;
    if reply.capability != capability {
        bail!("upgrade dialog authentication failed");
    }
    guard.finish();
    Ok(reply.proceed)
}

#[cfg(test)]
mod tests {
    use super::*;

    use splinterm_core::{DojoId, LairId};
    use splinterm_protocol::{HandoffWindowDojo, HandoffWindowIssue};

    fn window(pid: u32, dojos: usize, name_bytes: usize) -> HandoffUnresponsiveWindow {
        HandoffUnresponsiveWindow {
            index: u8::try_from(pid % 64).unwrap(),
            pid,
            reason: HandoffWindowIssue::NoAcknowledgement,
            dojos: (0..dojos)
                .map(|number| HandoffWindowDojo {
                    lair_id: LairId::new(),
                    dojo_id: DojoId::new(),
                    name: format!("{number:0>name_bytes$}"),
                })
                .collect(),
        }
    }

    fn prompt(windows: Vec<HandoffUnresponsiveWindow>) -> UpgradeDialogPrompt {
        UpgradeDialogPrompt {
            capability: vec![9; CAPABILITY_BYTES],
            parent_pid: 1234,
            respond_within_seconds: 18,
            windows,
        }
    }

    #[test]
    fn short_dojo_lists_are_shown_in_full() {
        let mut short = window(4242, 2, 4);
        short.dojos[1].name = "lo\u{202e}gs".to_owned();
        let line = window_line(&short);
        assert_eq!(
            line,
            "  • PID 4242 — did not answer in time — Dojos: 0000, lo\u{fffd}gs"
        );
    }

    #[test]
    fn long_dojo_lists_keep_a_count_and_never_exceed_the_width() {
        let line = window_line(&window(7, 32, 20));
        assert!(line.len() <= MAX_DIALOG_COLUMNS, "{line}");
        assert!(line.contains("more)"), "{line}");
        let shown = line.matches(", ").count() + 1;
        let counted: usize = line
            .rsplit("(+")
            .next()
            .and_then(|tail| tail.split(' ').next())
            .and_then(|count| count.parse().ok())
            .unwrap();
        assert_eq!(shown + counted, 32, "{line}");

        let one_huge = window_line(&window(8, 3, MAX_HANDOFF_DOJO_NAME_BYTES));
        assert!(one_huge.len() <= MAX_DIALOG_COLUMNS, "{one_huge}");
        assert!(one_huge.ends_with("3 Dojo(s)"), "{one_huge}");
    }

    #[test]
    fn the_largest_prompt_fits_the_bounded_dialog_with_a_count_and_the_actions() {
        let windows = (1..=64).map(|pid| window(pid, 32, 128)).collect::<Vec<_>>();
        let dialog = prompt(windows);
        let lines = dialog_lines(&dialog);
        let listed = lines
            .iter()
            .filter(|line| line.starts_with("  • PID "))
            .count();
        assert_eq!(listed, MAX_LISTED_WINDOWS - 1);
        let summary = format!(
            "  +{} more Windows ({} Dojos)",
            64 - listed,
            (64 - listed) * 32
        );
        assert!(lines.contains(&summary), "{lines:#?}");
        assert!(
            lines[2].contains("64 Windows cannot take part"),
            "{}",
            lines[2]
        );
        for footer in DIALOG_FOOTER.iter().filter(|line| !line.is_empty()) {
            assert!(
                lines.iter().any(|line| line == footer),
                "missing {footer:?}"
            );
        }
        assert!(
            lines
                .iter()
                .any(|line| line.contains("no answer within 18 seconds"))
        );
        assert!(lines.iter().all(|line| line.len() <= MAX_DIALOG_COLUMNS));
        let (decision, _receiver) = std_mpsc::channel();
        let options = dialog_window_options(&dialog, decision);
        let snapshot = options.snapshot.as_ref().unwrap();
        assert!(snapshot.rows <= MAX_DIALOG_ROWS, "{}", snapshot.rows);
        // The grid shows the whole snapshot, and the text ends above the
        // bottom 22% the action areas cover.
        assert_eq!(usize::from(options.initial_rows), snapshot.rows);
        assert_eq!(usize::from(options.initial_columns), snapshot.columns);
        assert!(lines.len() * 100 <= snapshot.rows * TEXT_ROWS_PERCENT);
        assert_eq!(snapshot.visible_rows.len(), snapshot.rows);
        assert_eq!(snapshot.title, "Upgrade question");
        assert_eq!(snapshot.cursor_row, -1);
    }

    #[test]
    fn every_window_is_listed_while_the_list_fits() {
        let windows = (1..=u32::try_from(MAX_LISTED_WINDOWS).unwrap())
            .map(|pid| window(pid, 32, 128))
            .collect::<Vec<_>>();
        let lines = dialog_lines(&prompt(windows));
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("  • PID "))
                .count(),
            MAX_LISTED_WINDOWS
        );
        assert!(!lines.iter().any(|line| line.contains("more Windows")));
        assert!(dialog_rows(lines.len()) <= MAX_DIALOG_ROWS);
        let single = dialog_lines(&prompt(vec![window(5, 0, 0)]));
        assert!(
            single[2].contains("1 Window cannot take part"),
            "{}",
            single[2]
        );
    }

    #[test]
    fn the_dialog_says_what_proceeding_does_and_how_to_answer() {
        let text = dialog_lines(&prompt(vec![window(5, 0, 0)])).join("\n");
        assert!(text.contains("PID 5 — did not answer in time — Dojos: none named"));
        assert!(text.contains("Proceeding closes these Windows"));
        assert!(text.contains("terminals keep running"));
        assert!(text.contains("Recent Dojos picker"));
        assert!(text.contains("no answer within 18 seconds, cancels"));
        assert!(text.contains("[D / Esc / Enter  Cancel]"));
        assert!(text.contains("[G  Proceed]"));
        assert!(!text.contains("G / Enter"));
    }

    #[test]
    fn prompts_are_bounded_and_round_trip_through_the_private_frame() {
        let largest = prompt((1..=64).map(|pid| window(pid, 32, 128)).collect());
        largest.validate().unwrap();
        let mut buffer = Vec::new();
        write_private_frame_bounded(&mut buffer, &largest, MAX_UPGRADE_DIALOG_FRAME_BYTES).unwrap();
        let decoded: UpgradeDialogPrompt =
            read_private_frame_bounded(&mut buffer.as_slice(), MAX_UPGRADE_DIALOG_FRAME_BYTES)
                .unwrap();
        assert_eq!(decoded, largest);

        assert!(prompt(Vec::new()).validate().is_err());
        let mut short_capability = prompt(vec![window(1, 0, 0)]);
        short_capability.capability.pop();
        assert!(short_capability.validate().is_err());
        assert!(prompt(vec![window(1, 33, 1)]).validate().is_err());
        assert!(prompt(vec![window(1, 1, 129)]).validate().is_err());
    }

    #[test]
    fn a_dropped_question_kills_and_reaps_its_dialog() {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        drop(DialogChild(Some(child)));
        // Reaped: the pid no longer names our child.
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}/stat")).exists()
                || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .is_ok_and(|stat| !stat.contains("(sleep)"))
        );
    }

    #[test]
    fn finishing_an_answered_dialog_never_blocks_the_caller() {
        // A child that lingers after answering is reaped in the background.
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let started = Instant::now();
        DialogChild(Some(child)).finish();
        assert!(started.elapsed() < Duration::from_millis(500));
        // The reaper kills it after its grace period.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|stat| stat.contains("(sleep)"))
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|stat| stat.contains("(sleep)"))
        );
    }

    #[test]
    fn only_the_asking_command_counts_as_the_parent() {
        let parent = rustix::process::getppid();
        let raw = u32::try_from(rustix::process::Pid::as_raw(parent)).unwrap();
        assert!(is_parent(raw, parent));
        // Reparented to init, or to a subreaper.
        if raw != 1 {
            assert!(!is_parent(raw, rustix::process::Pid::from_raw(1)));
        }
        assert!(!is_parent(raw, None));
        assert!(!is_parent(u32::MAX, parent));
    }
}
