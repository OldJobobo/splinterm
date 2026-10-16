//! Connection-status screen for opening a remote Window (Plan 0055 Milestone 3).
//!
//! A remote launch maps this screen immediately, shows "Connecting to …" while
//! OpenSSH authenticates, and either hands over to the remote's own Window or
//! shows the categorized failure with a one-line remedy and Retry / Close. The
//! screen is a read-only text snapshot in an ordinary client Window, so it uses
//! the configured theme and the terminal accessibility path.

use splinterm_core::SplintId;
use splinterm_protocol::TerminalSnapshot;

use crate::{
    diagnostics::DiagnosticErrorCode,
    frontend::synthetic_text_snapshot,
    remote_session::{RemoteFailure, RemoteFailureKind},
};

const MAX_DETAIL_LINES: usize = 6;
const MAX_DETAIL_CHARS: usize = 100;

/// The preferred way to authenticate without a terminal (Plan 0055 decision).
pub const AGENT_GUIDANCE: [&str; 2] = [
    "Without a terminal, load your key into ssh-agent (ssh-add) or set",
    "SSH_ASKPASS to a graphical prompt. This is the preferred setup.",
];

/// A key the status screen acts on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteStatusKey {
    Retry,
    Close,
}

/// Maps one encoded key press to a status action: R or Enter retries; C, Q,
/// or Escape closes. Anything else, including modified keys, is ignored.
#[must_use]
pub fn status_key(bytes: &[u8]) -> Option<RemoteStatusKey> {
    match bytes {
        b"r" | b"R" | b"\r" => Some(RemoteStatusKey::Retry),
        b"c" | b"C" | b"q" | b"Q" | b"\x1b" => Some(RemoteStatusKey::Close),
        _ => None,
    }
}

/// User-facing explanation of one failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FailureCopy {
    pub summary: &'static str,
    pub remedy: &'static str,
    pub diagnostic_code: DiagnosticErrorCode,
}

/// Explains a failure category; `None` is a profile or configuration problem.
#[must_use]
pub const fn failure_copy(kind: Option<RemoteFailureKind>) -> FailureCopy {
    use DiagnosticErrorCode as Code;
    use RemoteFailureKind as Kind;
    let (summary, remedy, diagnostic_code) = match kind {
        None => (
            "The remote profile could not be loaded.",
            "Fix remotes.toml; `splinterm remote list` shows the error.",
            Code::RemoteProfile,
        ),
        Some(Kind::UnknownHostKey) => (
            "The server's SSH host key is not known.",
            "Verify its fingerprint, then accept it once with `ssh HOST` in a terminal.",
            Code::RemoteHostKey,
        ),
        Some(Kind::ChangedHostKey) => (
            "The server's SSH host key has changed.",
            "Confirm the change with the server's owner before updating known_hosts.",
            Code::RemoteHostKey,
        ),
        Some(Kind::RoutingOrTimeout) => (
            "The server could not be reached.",
            "Check the host name, port, and network, then retry.",
            Code::RemoteUnreachable,
        ),
        Some(Kind::AuthenticationFailed) => (
            "SSH authentication failed.",
            "Check the user and key in remotes.toml, and load the key into ssh-agent.",
            Code::RemoteAuthentication,
        ),
        Some(Kind::InteractiveAuthenticationUnavailable) => (
            "SSH needs a password or passphrase, but nothing can ask for it.",
            "Load the key into ssh-agent or set SSH_ASKPASS, then retry.",
            Code::RemoteAuthentication,
        ),
        Some(Kind::RemoteCommandUnavailable) => (
            "Splinterm is not installed on the server.",
            "Install Splinterm there, or set `executable` in remotes.toml.",
            Code::RemoteInstallation,
        ),
        Some(Kind::RemoteDaemonUnavailable) => (
            "splinterd is not running on the server.",
            "Start it there with `systemctl --user start splinterd.service`.",
            Code::RemoteDaemon,
        ),
        Some(Kind::RelayIdentityRejected) => (
            "The server's splinterd rejected its Splinterm relay.",
            "Reinstall Splinterm on the server so the relay and daemon match.",
            Code::RemoteInstallation,
        ),
        Some(Kind::MultiplexerIncompatible | Kind::DaemonProtocolIncompatible) => (
            "The server runs an incompatible Splinterm version.",
            "Install the same Splinterm version on both machines.",
            Code::RemoteIncompatible,
        ),
        Some(Kind::TransportFailed) => (
            "The SSH connection failed.",
            "Run `splinterm remote check PROFILE` for details, then retry.",
            Code::RemoteTransport,
        ),
    };
    FailureCopy {
        summary,
        remedy,
        diagnostic_code,
    }
}

/// A failed attempt: its category, if known, and bounded detail lines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteStatusFailure {
    pub kind: Option<RemoteFailureKind>,
    pub details: Vec<String>,
}

impl RemoteStatusFailure {
    /// Classifies an attempt error. Categorized remote failures keep their raw,
    /// already sanitized OpenSSH diagnostic as detail; other errors (profile
    /// loading) keep their message chain.
    #[must_use]
    pub fn from_error(error: &anyhow::Error) -> Self {
        let remote = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<RemoteFailure>());
        let (kind, text) = match remote {
            Some(failure) => (Some(failure.kind()), failure.diagnostic().to_owned()),
            None => (None, format!("{error:#}")),
        };
        Self {
            kind,
            details: detail_lines(&text),
        }
    }

    #[must_use]
    pub const fn copy(&self) -> FailureCopy {
        failure_copy(self.kind)
    }
}

/// Bounds diagnostic text to a few short, control-free, non-empty lines.
fn detail_lines(text: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .map(|line| {
            line.chars()
                .map(|character| {
                    if character.is_control() {
                        ' '
                    } else {
                        character
                    }
                })
                .collect::<String>()
        })
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .map(|line| {
            if line.chars().count() > MAX_DETAIL_CHARS {
                let mut short = line.chars().take(MAX_DETAIL_CHARS - 1).collect::<String>();
                short.push('…');
                short
            } else {
                line
            }
        })
        .collect::<Vec<_>>();
    if lines.len() > MAX_DETAIL_LINES {
        // The newest OpenSSH lines usually carry the cause.
        lines.drain(..lines.len() - MAX_DETAIL_LINES);
    }
    lines
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteStatusPhase {
    Connecting,
    Failed(RemoteStatusFailure),
    Connected,
    Closed,
}

/// What the caller must do after the screen handles an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteStatusEffect {
    /// Nothing changes.
    None,
    /// Start exactly one new connection attempt and redraw.
    StartAttempt,
    /// Show the failure.
    ShowFailure,
    /// Close the screen and open the connected remote's Window.
    HandOver,
    /// Cancel any attempt and close the screen.
    Close,
}

/// The status screen's state machine. The first attempt starts with the screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteStatus {
    profile: String,
    destination: String,
    phase: RemoteStatusPhase,
    attempts: u32,
}

impl RemoteStatus {
    #[must_use]
    pub fn new(profile: &str, destination: &str) -> Self {
        Self {
            profile: profile.to_owned(),
            destination: destination.to_owned(),
            phase: RemoteStatusPhase::Connecting,
            attempts: 1,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> &RemoteStatusPhase {
        &self.phase
    }

    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn set_destination(&mut self, destination: &str) {
        destination.clone_into(&mut self.destination);
    }

    /// Handles one key. Retry acts only on a shown failure, so repeated
    /// presses while connecting never start a second attempt.
    pub fn on_key(&mut self, key: RemoteStatusKey) -> RemoteStatusEffect {
        match (&self.phase, key) {
            (RemoteStatusPhase::Failed(_), RemoteStatusKey::Retry) => {
                self.phase = RemoteStatusPhase::Connecting;
                self.attempts = self.attempts.saturating_add(1);
                RemoteStatusEffect::StartAttempt
            }
            (
                RemoteStatusPhase::Connecting | RemoteStatusPhase::Failed(_),
                RemoteStatusKey::Close,
            ) => {
                self.phase = RemoteStatusPhase::Closed;
                RemoteStatusEffect::Close
            }
            _ => RemoteStatusEffect::None,
        }
    }

    /// Records the result of the current attempt.
    pub fn on_attempt(&mut self, result: Result<(), RemoteStatusFailure>) -> RemoteStatusEffect {
        if self.phase != RemoteStatusPhase::Connecting {
            return RemoteStatusEffect::None;
        }
        match result {
            Ok(()) => {
                self.phase = RemoteStatusPhase::Connected;
                RemoteStatusEffect::HandOver
            }
            Err(failure) => {
                self.phase = RemoteStatusPhase::Failed(failure);
                RemoteStatusEffect::ShowFailure
            }
        }
    }

    /// The Window closed outside the screen's control (compositor close).
    pub fn on_window_closed(&mut self) -> RemoteStatusEffect {
        if matches!(
            self.phase,
            RemoteStatusPhase::Connected | RemoteStatusPhase::Closed
        ) {
            return RemoteStatusEffect::None;
        }
        self.phase = RemoteStatusPhase::Closed;
        RemoteStatusEffect::Close
    }

    /// The Window title for the current phase.
    #[must_use]
    pub fn title(&self) -> String {
        match &self.phase {
            RemoteStatusPhase::Failed(_) => format!("Could not connect to {}", self.profile),
            RemoteStatusPhase::Connected => format!("Connected to {}", self.profile),
            RemoteStatusPhase::Connecting | RemoteStatusPhase::Closed => {
                format!("Connecting to {}", self.profile)
            }
        }
    }

    /// The screen text for the current phase.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("REMOTE · {}", self.profile), String::new()];
        match &self.phase {
            RemoteStatusPhase::Failed(failure) => {
                let copy = failure.copy();
                lines.push(format!("Could not connect to {}.", self.destination));
                lines.push(String::new());
                lines.push(copy.summary.to_owned());
                lines.push(format!("Fix: {}", copy.remedy));
                if !failure.details.is_empty() {
                    lines.push(String::new());
                    lines.push("Details:".to_owned());
                    lines.extend(failure.details.iter().map(|line| format!("  {line}")));
                }
                lines.push(String::new());
                lines.extend(AGENT_GUIDANCE.iter().map(|line| (*line).to_owned()));
                lines.push(String::new());
                lines.push("R / Enter: retry          C / Esc: close".to_owned());
            }
            RemoteStatusPhase::Connected => {
                lines.push(format!("Connected to {}.", self.destination));
            }
            RemoteStatusPhase::Connecting | RemoteStatusPhase::Closed => {
                lines.push(format!("Connecting to {}…", self.destination));
                lines.push(String::new());
                lines.push("OpenSSH is authenticating. An agent key or SSH_ASKPASS".to_owned());
                lines.push("is used when available.".to_owned());
                if self.attempts > 1 {
                    lines.push(format!("Attempt {}.", self.attempts));
                }
                lines.push(String::new());
                lines.push("C / Esc: cancel".to_owned());
            }
        }
        lines
    }

    /// Renders the current phase as a snapshot of the status Window.
    #[must_use]
    pub fn snapshot(&self, splint_id: SplintId, revision: u64) -> TerminalSnapshot {
        synthetic_text_snapshot(splint_id, revision, &self.title(), self.lines())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(kind: Option<RemoteFailureKind>) -> RemoteStatusFailure {
        RemoteStatusFailure {
            kind,
            details: vec!["Permission denied (publickey).".to_owned()],
        }
    }

    #[test]
    fn keys_map_to_retry_and_close_only_when_unmodified() {
        for bytes in [&b"r"[..], b"R", b"\r"] {
            assert_eq!(status_key(bytes), Some(RemoteStatusKey::Retry));
        }
        for bytes in [&b"c"[..], b"C", b"q", b"Q", b"\x1b"] {
            assert_eq!(status_key(bytes), Some(RemoteStatusKey::Close));
        }
        for bytes in [&b"x"[..], b"\x1br", b"\x12", b"rr", b"", b"\x1b[A"] {
            assert_eq!(status_key(bytes), None, "{bytes:?}");
        }
    }

    #[test]
    fn connecting_then_connected_hands_over_once() {
        let mut status = RemoteStatus::new("wintermute", "case@wintermute.example");
        assert_eq!(status.phase(), &RemoteStatusPhase::Connecting);
        assert_eq!(status.on_attempt(Ok(())), RemoteStatusEffect::HandOver);
        assert_eq!(status.phase(), &RemoteStatusPhase::Connected);
        assert_eq!(status.on_attempt(Ok(())), RemoteStatusEffect::None);
        assert_eq!(
            status.on_key(RemoteStatusKey::Close),
            RemoteStatusEffect::None
        );
        assert_eq!(status.on_window_closed(), RemoteStatusEffect::None);
    }

    #[test]
    fn failure_then_retry_starts_exactly_one_attempt_per_press() {
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        assert_eq!(
            status.on_key(RemoteStatusKey::Retry),
            RemoteStatusEffect::None,
            "retry does nothing while the first attempt runs"
        );
        let denied = failure(Some(RemoteFailureKind::AuthenticationFailed));
        assert_eq!(
            status.on_attempt(Err(denied.clone())),
            RemoteStatusEffect::ShowFailure
        );
        assert_eq!(status.phase(), &RemoteStatusPhase::Failed(denied.clone()));
        assert_eq!(
            status.on_key(RemoteStatusKey::Retry),
            RemoteStatusEffect::StartAttempt
        );
        assert_eq!(status.attempts(), 2);
        assert_eq!(
            status.on_key(RemoteStatusKey::Retry),
            RemoteStatusEffect::None,
            "a second press while reconnecting is ignored"
        );
        assert_eq!(status.attempts(), 2);
        assert_eq!(
            status.on_attempt(Err(denied)),
            RemoteStatusEffect::ShowFailure
        );
        assert_eq!(
            status.on_key(RemoteStatusKey::Close),
            RemoteStatusEffect::Close
        );
        assert_eq!(status.phase(), &RemoteStatusPhase::Closed);
        assert_eq!(
            status.on_attempt(Ok(())),
            RemoteStatusEffect::None,
            "a late result after close is ignored"
        );
    }

    #[test]
    fn cancel_while_connecting_and_compositor_close_both_close() {
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        assert_eq!(
            status.on_key(RemoteStatusKey::Close),
            RemoteStatusEffect::Close
        );
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        assert_eq!(status.on_window_closed(), RemoteStatusEffect::Close);
        assert_eq!(status.on_window_closed(), RemoteStatusEffect::None);
    }

    #[test]
    fn every_category_has_a_summary_remedy_and_structured_code() {
        let cases = [
            (None, DiagnosticErrorCode::RemoteProfile, "remotes.toml"),
            (
                Some(RemoteFailureKind::UnknownHostKey),
                DiagnosticErrorCode::RemoteHostKey,
                "fingerprint",
            ),
            (
                Some(RemoteFailureKind::ChangedHostKey),
                DiagnosticErrorCode::RemoteHostKey,
                "known_hosts",
            ),
            (
                Some(RemoteFailureKind::RoutingOrTimeout),
                DiagnosticErrorCode::RemoteUnreachable,
                "network",
            ),
            (
                Some(RemoteFailureKind::AuthenticationFailed),
                DiagnosticErrorCode::RemoteAuthentication,
                "ssh-agent",
            ),
            (
                Some(RemoteFailureKind::InteractiveAuthenticationUnavailable),
                DiagnosticErrorCode::RemoteAuthentication,
                "SSH_ASKPASS",
            ),
            (
                Some(RemoteFailureKind::RemoteCommandUnavailable),
                DiagnosticErrorCode::RemoteInstallation,
                "executable",
            ),
            (
                Some(RemoteFailureKind::RemoteDaemonUnavailable),
                DiagnosticErrorCode::RemoteDaemon,
                "splinterd.service",
            ),
            (
                Some(RemoteFailureKind::RelayIdentityRejected),
                DiagnosticErrorCode::RemoteInstallation,
                "Reinstall",
            ),
            (
                Some(RemoteFailureKind::MultiplexerIncompatible),
                DiagnosticErrorCode::RemoteIncompatible,
                "same Splinterm version",
            ),
            (
                Some(RemoteFailureKind::DaemonProtocolIncompatible),
                DiagnosticErrorCode::RemoteIncompatible,
                "same Splinterm version",
            ),
            (
                Some(RemoteFailureKind::TransportFailed),
                DiagnosticErrorCode::RemoteTransport,
                "remote check",
            ),
        ];
        for (kind, code, remedy_word) in cases {
            let copy = failure_copy(kind);
            assert_eq!(copy.diagnostic_code, code, "{kind:?}");
            assert!(copy.summary.ends_with('.'), "{kind:?}");
            assert!(copy.remedy.contains(remedy_word), "{kind:?}");
            assert!(
                copy.remedy.chars().count() <= 80,
                "{kind:?} remedy is one line"
            );
        }
    }

    #[test]
    fn errors_keep_their_category_and_bounded_details() {
        let remote = anyhow::Error::new(RemoteFailure::new(
            RemoteFailureKind::UnknownHostKey,
            "line one\n\nHost key verification failed.\u{7}",
        ))
        .context("open remote");
        let failure = RemoteStatusFailure::from_error(&remote);
        assert_eq!(failure.kind, Some(RemoteFailureKind::UnknownHostKey));
        assert_eq!(
            failure.details,
            ["line one", "Host key verification failed."]
        );

        let profile = anyhow::anyhow!("remote profile \"gone\" is not configured");
        let failure = RemoteStatusFailure::from_error(&profile);
        assert_eq!(failure.kind, None);
        assert_eq!(
            failure.details,
            ["remote profile \"gone\" is not configured"]
        );

        let long = (0..20)
            .map(|index| format!("{index} {}", "x".repeat(200)))
            .collect::<Vec<_>>()
            .join("\n");
        let details = detail_lines(&long);
        assert_eq!(details.len(), MAX_DETAIL_LINES);
        assert!(details[0].starts_with("14 "), "the newest lines are kept");
        assert!(
            details
                .iter()
                .all(|line| line.chars().count() <= MAX_DETAIL_CHARS)
        );
    }

    #[test]
    fn screens_explain_progress_failure_remedy_and_the_agent_path() {
        let mut status = RemoteStatus::new("wintermute", "case@wintermute.example:2222");
        let connecting = status.lines();
        assert_eq!(connecting[0], "REMOTE · wintermute");
        assert!(connecting.contains(&"Connecting to case@wintermute.example:2222…".to_owned()));
        assert!(connecting.contains(&"C / Esc: cancel".to_owned()));
        assert_eq!(status.title(), "Connecting to wintermute");

        status.on_attempt(Err(failure(Some(RemoteFailureKind::AuthenticationFailed))));
        let failed = status.lines();
        assert_eq!(status.title(), "Could not connect to wintermute");
        assert!(failed.contains(&"SSH authentication failed.".to_owned()));
        assert!(failed.iter().any(|line| line.starts_with("Fix: ")));
        assert!(failed.contains(&"  Permission denied (publickey).".to_owned()));
        for line in AGENT_GUIDANCE {
            assert!(failed.contains(&line.to_owned()));
        }
        assert!(failed.contains(&"R / Enter: retry          C / Esc: close".to_owned()));

        status.on_key(RemoteStatusKey::Retry);
        assert!(status.lines().contains(&"Attempt 2.".to_owned()));

        let id = SplintId::new();
        let first = status.snapshot(id, 1);
        let second = status.snapshot(id, 2);
        assert!(first.validate().is_ok() && second.validate().is_ok());
        assert_eq!(first.title, "Connecting to wintermute");
        assert_eq!((first.splint_id, second.revision), (id, 2));
    }
}
