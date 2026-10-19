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
//! The dialog keeps the consent Window's keys and pointer areas, which this
//! slice does not change (they live in `wayland.rs`): `D` or `Escape` and the
//! red left area cancel; `G` or `Enter` and the green right area proceed.

use std::{
    fmt::Write as _,
    io,
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc as std_mpsc},
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
    respond_within_seconds: u64,
    windows: Vec<HandoffUnresponsiveWindow>,
}

impl UpgradeDialogPrompt {
    fn validate(&self) -> Result<()> {
        let valid = self.capability.len() == CAPABILITY_BYTES
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
    let mut lines = vec![
        "TRUSTED SPLINTERM UPGRADE QUESTION".to_owned(),
        String::new(),
        "splinterd is upgrading. These Windows cannot take part:".to_owned(),
    ];
    lines.extend(prompt.windows.iter().map(window_line));
    lines.extend([
        String::new(),
        "Proceeding closes these Windows. Their terminals keep running in the".to_owned(),
        "upgraded daemon; reopen them from the Recent Dojos picker.".to_owned(),
        "Input is paused in every Window until you answer.".to_owned(),
        format!(
            "Closing this dialog, or no answer within {} seconds, cancels.",
            prompt.respond_within_seconds
        ),
        String::new(),
        "D / Escape: CANCEL the upgrade     G / Enter: PROCEED without them".to_owned(),
        "Click the red left area to cancel or the green right area to proceed.".to_owned(),
    ]);
    lines
}

fn dialog_snapshot(prompt: &UpgradeDialogPrompt) -> TerminalSnapshot {
    let lines = dialog_lines(prompt);
    let rows = (lines.len() * 100).div_ceil(TEXT_ROWS_PERCENT);
    static_dialog_snapshot(
        lines,
        rows.max(18),
        "Upgrade question",
        splinterm_core::SplintId::new(),
        0,
    )
}

/// The hidden `upgrade-dialog` child: reads one prompt on stdin, shows it,
/// and writes the answer on stdout. Closing the Window answers cancel.
pub(in crate::app) fn run_upgrade_dialog_client() -> Result<()> {
    let prompt: UpgradeDialogPrompt =
        read_private_frame_bounded(&mut io::stdin().lock(), MAX_UPGRADE_DIALOG_FRAME_BYTES)?;
    prompt.validate()?;
    let (decision, receiver) = std_mpsc::channel();
    run_window(WindowOptions {
        snapshot: Some(dialog_snapshot(&prompt)),
        trusted_consent: Some(TrustedConsentUi { decision }),
        ..WindowOptions::default()
    })?;
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

/// Kills and reaps the dialog child unless it already finished, so an
/// abandoned question (the daemon ended the attempt, or the deadline passed)
/// never leaves a dialog on screen.
struct DialogChild(Arc<Mutex<Option<Child>>>);

impl DialogChild {
    /// Waits briefly for a child that has answered to exit, then reaps it.
    fn finish(&self) {
        let Some(mut child) = self.0.lock().ok().and_then(|mut child| child.take()) else {
            return;
        };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for DialogChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.lock().ok().and_then(|mut child| child.take()) {
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
    let guard = DialogChild(Arc::new(Mutex::new(Some(process))));
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
    fn every_window_gets_a_line_above_the_action_areas() {
        let windows = (1..=64).map(|pid| window(pid, 32, 128)).collect::<Vec<_>>();
        let dialog = prompt(windows);
        let lines = dialog_lines(&dialog);
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("  • PID "))
                .count(),
            64
        );
        assert!(lines.iter().all(|line| line.len() <= MAX_DIALOG_COLUMNS));
        let snapshot = dialog_snapshot(&dialog);
        // The text ends above the bottom 22% the action areas cover.
        assert!(lines.len() * 100 <= snapshot.rows * TEXT_ROWS_PERCENT);
        assert_eq!(snapshot.title, "Upgrade question");
        assert_eq!(snapshot.cursor_row, -1);
    }

    #[test]
    fn the_dialog_says_what_proceeding_does_and_how_to_answer() {
        let text = dialog_lines(&prompt(vec![window(5, 0, 0)])).join("\n");
        assert!(text.contains("PID 5 — did not answer in time — Dojos: none named"));
        assert!(text.contains("Proceeding closes these Windows"));
        assert!(text.contains("terminals keep running"));
        assert!(text.contains("Recent Dojos picker"));
        assert!(text.contains("no answer within 18 seconds, cancels"));
        assert!(text.contains("D / Escape: CANCEL"));
        assert!(text.contains("G / Enter: PROCEED"));
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
        drop(DialogChild(Arc::new(Mutex::new(Some(child)))));
        // Reaped: the pid no longer names our child.
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}/stat")).exists()
                || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .is_ok_and(|stat| !stat.contains("(sleep)"))
        );
    }
}
