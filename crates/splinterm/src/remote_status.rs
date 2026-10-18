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
    remote::is_valid_profile_name,
    remote_askpass::AskpassKind,
    remote_session::{
        LocalSshFailure, LocalSshProblem, RemoteFailure, RemoteFailureKind, is_bidi_control,
    },
};

const MAX_DETAIL_LINES: usize = 6;
const MAX_DETAIL_CHARS: usize = 100;
/// Quoted OpenSSH prompt text wraps at this many cells so that, after its
/// four-cell quote prefix, it stays inside the 72 columns the status screen
/// always has instead of running past the Window edge. Prompt lines arrive
/// up to [`crate::remote_askpass::MAX_PROMPT_LINE_CHARS`] long.
const PROMPT_WRAP_CELLS: usize = 68;
/// The status Window's rows unless [`RemoteStatus::with_rows`] says otherwise:
/// the default `main.initial-rows`.
const DEFAULT_STATUS_ROWS: u16 = 24;

/// What a prompt-screen line is, in the order lines give way when the status
/// Window is too short for all of them: blank spacing first, the key hints
/// last. Quoted prompt rows give way before any of these, down to the last
/// one, which holds OpenSSH's question.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PromptLine {
    Blank,
    Guidance,
    HandlingNote,
    Header,
    Status,
    Intro,
    Field,
    Question,
    Controls,
}
/// Shown instead of a `--remote` argument that fails the profile-name rule.
pub const FALLBACK_PROFILE_LABEL: &str = "remote";

/// Returns `name` when it satisfies the profile-name rule, so untrusted argv
/// text (bidi or control characters) never reaches the title or screen.
#[must_use]
pub fn profile_label(name: &str) -> &str {
    if is_valid_profile_name(name) {
        name
    } else {
        FALLBACK_PROFILE_LABEL
    }
}

/// Marks an attempt error as a profile-loading problem: `remotes.toml` could
/// not be read or does not configure the requested profile.
#[derive(Debug)]
pub struct RemoteProfileError {
    message: String,
}

impl RemoteProfileError {
    /// Wraps a catalog load or lookup error, keeping its message chain.
    #[must_use]
    pub fn wrap(error: &anyhow::Error) -> anyhow::Error {
        anyhow::Error::new(Self {
            message: format!("{error:#}"),
        })
    }
}

impl std::fmt::Display for RemoteProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RemoteProfileError {}

/// What a failed connection attempt is attributed to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteFailureCategory {
    /// `remotes.toml` could not be loaded or lacks the profile.
    Profile,
    /// A local OpenSSH setup problem found before connecting.
    LocalSetup(LocalSshProblem),
    /// A categorized OpenSSH, relay, or remote-daemon failure. Errors that
    /// carry no category are reported as [`RemoteFailureKind::TransportFailed`].
    Remote(RemoteFailureKind),
}

/// The preferred way to authenticate without a terminal (Plan 0055 decision).
pub const AGENT_GUIDANCE: [&str; 2] = [
    "Without a terminal, load your key into ssh-agent (ssh-add) or set",
    "SSH_ASKPASS to a graphical prompt. This is the preferred setup.",
];

/// Shown while connecting without a terminal (Plan 0055 Milestone 4).
pub const NATIVE_PROMPT_NOTE: &str = "Otherwise, if OpenSSH needs a password, it asks here.";

/// Shown under a masked field.
pub const SECRET_HANDLING_NOTE: &str =
    "Splinterm hands this only to OpenSSH and never shows or stores it.";

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

/// Explains a failure category.
#[must_use]
pub const fn failure_copy(category: RemoteFailureCategory) -> FailureCopy {
    use DiagnosticErrorCode as Code;
    use RemoteFailureCategory as Category;
    use RemoteFailureKind as Kind;
    let (summary, remedy, diagnostic_code) = match category {
        Category::Profile => (
            "The remote profile could not be loaded.",
            "Fix remotes.toml; `splinterm remote list` shows the error.",
            Code::RemoteProfile,
        ),
        Category::LocalSetup(LocalSshProblem::InvalidAskpass) => (
            "SSH_ASKPASS does not name a usable prompt program.",
            "Set SSH_ASKPASS to an absolute executable path, or unset it.",
            Code::RemoteLocalSetup,
        ),
        Category::LocalSetup(LocalSshProblem::OpenSshUnavailable) => (
            "OpenSSH (ssh) could not be started on this computer.",
            "Install the OpenSSH client so `ssh` is on PATH, then retry.",
            Code::RemoteLocalSetup,
        ),
        Category::Remote(Kind::UnknownHostKey) => (
            "The server's SSH host key is not known.",
            "Verify its fingerprint, then accept it once with `ssh HOST` in a terminal.",
            Code::RemoteHostKey,
        ),
        Category::Remote(Kind::ChangedHostKey) => (
            "The server's SSH host key has changed.",
            "Confirm the change with the server's owner before updating known_hosts.",
            Code::RemoteHostKey,
        ),
        Category::Remote(Kind::RoutingOrTimeout) => (
            "The server could not be reached.",
            "Check the host name, port, and network, then retry.",
            Code::RemoteUnreachable,
        ),
        Category::Remote(Kind::AuthenticationFailed) => (
            "SSH authentication failed.",
            "Check the user and key in remotes.toml, and load the key into ssh-agent.",
            Code::RemoteAuthentication,
        ),
        Category::Remote(Kind::InteractiveAuthenticationUnavailable) => (
            "SSH needs a password or passphrase, but nothing can ask for it.",
            "Load the key into ssh-agent or set SSH_ASKPASS, then retry.",
            Code::RemoteAuthentication,
        ),
        Category::Remote(Kind::RemoteCommandUnavailable) => (
            "Splinterm is not installed on the server.",
            "Install Splinterm there, or set `executable` in remotes.toml.",
            Code::RemoteInstallation,
        ),
        Category::Remote(Kind::RemoteDaemonUnavailable) => (
            "splinterd is not running on the server.",
            "Start it there with `systemctl --user start splinterd.service`.",
            Code::RemoteDaemon,
        ),
        Category::Remote(Kind::RelayIdentityRejected) => (
            "The server's splinterd rejected its Splinterm relay.",
            "Reinstall Splinterm on the server so the relay and daemon match.",
            Code::RemoteInstallation,
        ),
        Category::Remote(Kind::MultiplexerIncompatible | Kind::DaemonProtocolIncompatible) => (
            "The server runs an incompatible Splinterm version.",
            "Install the same Splinterm version on both machines.",
            Code::RemoteIncompatible,
        ),
        Category::Remote(Kind::TransportFailed) => (
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

/// A failed attempt: its category and bounded, sanitized detail lines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteStatusFailure {
    pub category: RemoteFailureCategory,
    pub details: Vec<String>,
}

impl RemoteStatusFailure {
    /// Classifies an attempt error. Categorized remote failures keep their
    /// OpenSSH diagnostic as detail, local setup and [`RemoteProfileError`]
    /// failures keep their message, and any other error is a transport failure
    /// that keeps its message chain. All detail text is sanitized again here.
    #[must_use]
    pub fn from_error(error: &anyhow::Error) -> Self {
        let classified = error.chain().find_map(|cause| {
            if let Some(failure) = cause.downcast_ref::<RemoteFailure>() {
                Some((
                    RemoteFailureCategory::Remote(failure.kind()),
                    failure.diagnostic().to_owned(),
                ))
            } else if let Some(failure) = cause.downcast_ref::<LocalSshFailure>() {
                Some((
                    RemoteFailureCategory::LocalSetup(failure.problem()),
                    failure.message().to_owned(),
                ))
            } else {
                cause
                    .downcast_ref::<RemoteProfileError>()
                    .map(|failure| (RemoteFailureCategory::Profile, failure.to_string()))
            }
        });
        let (category, text) = classified.unwrap_or_else(|| {
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::TransportFailed),
                format!("{error:#}"),
            )
        });
        Self {
            category,
            details: detail_lines(&text),
        }
    }

    #[must_use]
    pub const fn copy(&self) -> FailureCopy {
        failure_copy(self.category)
    }
}

/// Replaces control and bidirectional-override characters with spaces.
fn display_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() || is_bidi_control(character) {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// Bounds diagnostic text to a few short, non-empty lines free of control and
/// bidirectional-override characters.
fn detail_lines(text: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .map(display_text)
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

/// An open OpenSSH prompt as the screen shows it. It never holds the answer,
/// only whether the masked field has any input, so neither the screen text,
/// the Window title, nor accessibility text can contain the secret.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptDisplay {
    kind: AskpassKind,
    lines: Vec<String>,
    has_input: bool,
}

impl PromptDisplay {
    #[must_use]
    pub const fn kind(&self) -> AskpassKind {
        self.kind
    }

    /// Whether the masked field has any input; never its length or contents.
    #[must_use]
    pub const fn has_input(&self) -> bool {
        self.has_input
    }
}

/// The status screen's state machine. The first attempt starts with the screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteStatus {
    profile: String,
    destination: String,
    phase: RemoteStatusPhase,
    attempts: u32,
    terminal: bool,
    prompt: Option<PromptDisplay>,
    rows: u16,
}

impl RemoteStatus {
    /// Starts the screen. A `profile` that fails the profile-name rule is shown
    /// as [`FALLBACK_PROFILE_LABEL`]; control and bidi characters in
    /// `destination` are replaced.
    #[must_use]
    pub fn new(profile: &str, destination: &str) -> Self {
        Self {
            profile: profile_label(profile).to_owned(),
            destination: display_text(destination),
            phase: RemoteStatusPhase::Connecting,
            attempts: 1,
            terminal: false,
            prompt: None,
            rows: DEFAULT_STATUS_ROWS,
        }
    }

    /// Records whether Splinterm was started from a terminal, where OpenSSH
    /// may prompt instead of using `SSH_ASKPASS`.
    #[must_use]
    pub const fn with_terminal(mut self, terminal: bool) -> Self {
        self.terminal = terminal;
        self
    }

    /// Records the status Window's row count. The Window keeps its configured
    /// size, so a prompt screen is fitted to it rather than overflowing it.
    #[must_use]
    pub const fn with_rows(mut self, rows: u16) -> Self {
        self.rows = rows;
        self
    }

    #[must_use]
    pub const fn phase(&self) -> &RemoteStatusPhase {
        &self.phase
    }

    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    #[must_use]
    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn set_destination(&mut self, destination: &str) {
        self.destination = display_text(destination);
    }

    /// Shows an OpenSSH prompt while connecting. Returns false, showing
    /// nothing, in any other phase. Lines are cleaned again here.
    pub fn show_prompt(&mut self, kind: AskpassKind, lines: &[String]) -> bool {
        if self.phase != RemoteStatusPhase::Connecting {
            return false;
        }
        self.prompt = Some(PromptDisplay {
            kind,
            lines: lines.iter().map(|line| display_text(line)).collect(),
            has_input: false,
        });
        true
    }

    /// Records whether the masked field is empty; never its contents.
    pub fn set_prompt_input(&mut self, has_input: bool) {
        if let Some(prompt) = &mut self.prompt {
            prompt.has_input = has_input;
        }
    }

    /// Removes the prompt (answered, cancelled, or abandoned by OpenSSH).
    pub fn clear_prompt(&mut self) {
        self.prompt = None;
    }

    #[must_use]
    pub const fn prompt(&self) -> Option<&PromptDisplay> {
        self.prompt.as_ref()
    }

    /// Handles one key. Retry acts only on a shown failure, so repeated
    /// presses while connecting never start a second attempt.
    pub fn on_key(&mut self, key: RemoteStatusKey) -> RemoteStatusEffect {
        if key == RemoteStatusKey::Close {
            self.prompt = None;
        }
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
        self.prompt = None;
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
        self.prompt = None;
        self.phase = RemoteStatusPhase::Closed;
        RemoteStatusEffect::Close
    }

    /// The Window title for the current phase.
    #[must_use]
    pub fn title(&self) -> String {
        match &self.phase {
            RemoteStatusPhase::Failed(_) => format!("Could not connect to {}", self.profile),
            RemoteStatusPhase::Connected => format!("Connected to {}", self.profile),
            RemoteStatusPhase::Connecting if self.prompt.is_some() => {
                format!("Authenticating to {}", self.profile)
            }
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
            RemoteStatusPhase::Connecting if self.prompt.is_some() => {
                if let Some(prompt) = &self.prompt {
                    return self.prompt_screen(prompt);
                }
            }
            RemoteStatusPhase::Connecting | RemoteStatusPhase::Closed => {
                lines.push(format!("Connecting to {}…", self.destination));
                lines.push(String::new());
                lines.push("OpenSSH is authenticating. An agent key or SSH_ASKPASS".to_owned());
                lines.push("is used when available.".to_owned());
                if self.terminal {
                    lines.push(
                        "OpenSSH may prompt on the terminal that started Splinterm.".to_owned(),
                    );
                } else {
                    lines.push(NATIVE_PROMPT_NOTE.to_owned());
                }
                if self.attempts > 1 {
                    lines.push(format!("Attempt {}.", self.attempts));
                }
                lines.push(String::new());
                lines.push("C / Esc: cancel".to_owned());
            }
        }
        lines
    }

    /// A prompt screen fitted to the status Window's rows: the key hints and
    /// OpenSSH's question always stay, quoted prompt rows give way first
    /// (keeping the end, behind a `…` row), then the other lines by
    /// [`PromptLine`] order.
    fn prompt_screen(&self, prompt: &PromptDisplay) -> Vec<String> {
        use PromptLine::{
            Blank, Controls, Field, Guidance, HandlingNote, Header, Intro, Question, Status,
        };

        let mut before = vec![
            (Header, format!("REMOTE · {}", self.profile)),
            (Blank, String::new()),
            (Status, format!("Connecting to {}…", self.destination)),
            (Blank, String::new()),
        ];
        let after = match prompt.kind {
            AskpassKind::Secret => {
                // A ProxyJump hop prompts too, so the destination is only
                // context; OpenSSH's own text names who is asking.
                before.push((
                    Intro,
                    format!(
                        "OpenSSH, connecting to {}, is asking for a credential.",
                        self.destination
                    ),
                ));
                before.push((
                    Guidance,
                    "Its prompt names who it is for; it may be a jump host:".to_owned(),
                ));
                vec![
                    (Blank, String::new()),
                    (
                        Field,
                        if prompt.has_input {
                            "Password field: entered (hidden)".to_owned()
                        } else {
                            "Password field: empty".to_owned()
                        },
                    ),
                    (HandlingNote, SECRET_HANDLING_NOTE.to_owned()),
                    (Blank, String::new()),
                    (Controls, "Enter: submit          Esc: cancel".to_owned()),
                ]
            }
            AskpassKind::Confirm => {
                before.push((
                    Intro,
                    format!(
                        "OpenSSH, connecting to {}, asks you to confirm:",
                        self.destination
                    ),
                ));
                vec![
                    (Blank, String::new()),
                    (Controls, "Y: yes          N / Esc: no".to_owned()),
                ]
            }
            AskpassKind::Notice => {
                before.push((
                    Intro,
                    format!("OpenSSH, connecting to {}, says:", self.destination),
                ));
                vec![
                    (Blank, String::new()),
                    (Controls, "C / Esc: cancel".to_owned()),
                ]
            }
        };
        let rows = usize::from(self.rows).max(1);
        let mut quoted = prompt
            .lines
            .iter()
            .flat_map(|line| wrap_cells(line, PROMPT_WRAP_CELLS))
            .collect::<Vec<_>>();
        // OpenSSH ends with the question, so the end is kept.
        let budget = rows.saturating_sub(before.len() + after.len()).max(1);
        if quoted.len() > budget {
            quoted.drain(..quoted.len() - budget);
            if budget > 1 {
                "…".clone_into(&mut quoted[0]);
            }
        }
        let mut screen = before;
        screen.extend(
            quoted
                .into_iter()
                .map(|line| (Question, format!("  │ {line}"))),
        );
        screen.extend(after);
        while screen.len() > rows {
            let Some(index) = screen
                .iter()
                .enumerate()
                .min_by_key(|(_, (kind, _))| *kind)
                .map(|(index, _)| index)
            else {
                break;
            };
            screen.remove(index);
        }
        screen.into_iter().map(|(_, line)| line).collect()
    }

    /// Renders the current phase as a snapshot of the status Window.
    #[must_use]
    pub fn snapshot(&self, splint_id: SplintId, revision: u64) -> TerminalSnapshot {
        synthetic_text_snapshot(splint_id, revision, &self.title(), self.lines())
    }
}

/// Splits `line` into rows of at most `maximum` display cells, breaking at
/// spaces where it can and inside a word only when the word alone is longer.
fn wrap_cells(line: &str, maximum: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;

    let cells = |text: &str| {
        text.chars()
            .map(|character| character.width().unwrap_or(0).min(2))
            .sum::<usize>()
    };
    let mut rows = Vec::new();
    let mut current = String::new();
    for (index, word) in line.split(' ').enumerate() {
        if index > 0 {
            // A row break replaces the space between two words.
            if !current.is_empty() && cells(&current) + 1 + cells(word) > maximum {
                rows.push(std::mem::take(&mut current));
            } else if cells(&current) < maximum {
                current.push(' ');
            }
        }
        for character in word.chars() {
            let width = character.width().unwrap_or(0).min(2);
            if width > 0 && !current.is_empty() && cells(&current) + width > maximum {
                rows.push(std::mem::take(&mut current));
            }
            current.push(character);
        }
    }
    rows.push(current);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(kind: RemoteFailureKind) -> RemoteStatusFailure {
        RemoteStatusFailure {
            category: RemoteFailureCategory::Remote(kind),
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
        let denied = failure(RemoteFailureKind::AuthenticationFailed);
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
            (
                RemoteFailureCategory::Profile,
                DiagnosticErrorCode::RemoteProfile,
                "remotes.toml",
            ),
            (
                RemoteFailureCategory::LocalSetup(LocalSshProblem::InvalidAskpass),
                DiagnosticErrorCode::RemoteLocalSetup,
                "SSH_ASKPASS to an absolute executable",
            ),
            (
                RemoteFailureCategory::LocalSetup(LocalSshProblem::OpenSshUnavailable),
                DiagnosticErrorCode::RemoteLocalSetup,
                "OpenSSH client",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::UnknownHostKey),
                DiagnosticErrorCode::RemoteHostKey,
                "fingerprint",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::ChangedHostKey),
                DiagnosticErrorCode::RemoteHostKey,
                "known_hosts",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::RoutingOrTimeout),
                DiagnosticErrorCode::RemoteUnreachable,
                "network",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::AuthenticationFailed),
                DiagnosticErrorCode::RemoteAuthentication,
                "ssh-agent",
            ),
            (
                RemoteFailureCategory::Remote(
                    RemoteFailureKind::InteractiveAuthenticationUnavailable,
                ),
                DiagnosticErrorCode::RemoteAuthentication,
                "SSH_ASKPASS",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::RemoteCommandUnavailable),
                DiagnosticErrorCode::RemoteInstallation,
                "executable",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::RemoteDaemonUnavailable),
                DiagnosticErrorCode::RemoteDaemon,
                "splinterd.service",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::RelayIdentityRejected),
                DiagnosticErrorCode::RemoteInstallation,
                "Reinstall",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::MultiplexerIncompatible),
                DiagnosticErrorCode::RemoteIncompatible,
                "same Splinterm version",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::DaemonProtocolIncompatible),
                DiagnosticErrorCode::RemoteIncompatible,
                "same Splinterm version",
            ),
            (
                RemoteFailureCategory::Remote(RemoteFailureKind::TransportFailed),
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
        assert_eq!(
            failure.category,
            RemoteFailureCategory::Remote(RemoteFailureKind::UnknownHostKey)
        );
        assert_eq!(
            failure.details,
            ["line one", "Host key verification failed."]
        );

        let profile = RemoteProfileError::wrap(&anyhow::anyhow!(
            "remote profile \"gone\" is not configured"
        ))
        .context("load profile");
        let failure = RemoteStatusFailure::from_error(&profile);
        assert_eq!(failure.category, RemoteFailureCategory::Profile);
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
    fn uncategorized_errors_are_transport_failures_not_profile_errors() {
        for error in [
            anyhow::anyhow!("remote splinterd returned an invalid ping response"),
            anyhow::anyhow!("deadline elapsed").context("ping remote splinterd"),
        ] {
            let failure = RemoteStatusFailure::from_error(&error);
            assert_ne!(failure.category, RemoteFailureCategory::Profile);
            assert_eq!(
                failure.category,
                RemoteFailureCategory::Remote(RemoteFailureKind::TransportFailed)
            );
            assert_eq!(
                failure.copy().diagnostic_code,
                DiagnosticErrorCode::RemoteTransport
            );
        }
    }

    #[test]
    fn local_setup_failures_get_a_local_remedy() {
        let askpass = anyhow::Error::new(LocalSshFailure::new(
            LocalSshProblem::InvalidAskpass,
            "SSH_ASKPASS must name an absolute local executable",
        ));
        let failure = RemoteStatusFailure::from_error(&askpass);
        assert_eq!(
            failure.category,
            RemoteFailureCategory::LocalSetup(LocalSshProblem::InvalidAskpass)
        );
        let copy = failure.copy();
        assert!(copy.summary.contains("SSH_ASKPASS"));
        assert!(copy.remedy.contains("SSH_ASKPASS"));
        assert!(!copy.remedy.contains("remotes.toml"));
        assert_eq!(copy.diagnostic_code, DiagnosticErrorCode::RemoteLocalSetup);
        assert_eq!(
            failure.details,
            ["SSH_ASKPASS must name an absolute local executable"]
        );

        let missing = anyhow::Error::new(LocalSshFailure::new(
            LocalSshProblem::OpenSshUnavailable,
            "cannot launch OpenSSH executable ssh: No such file or directory",
        ));
        let failure = RemoteStatusFailure::from_error(&missing);
        assert!(failure.copy().remedy.contains("OpenSSH client"));
    }

    #[test]
    fn bidi_and_control_characters_never_reach_the_screen() {
        let error = anyhow::anyhow!("evil\u{202e}txt.exe\u{2066} \u{1b}[31mred");
        let failure = RemoteStatusFailure::from_error(&error);
        assert_eq!(failure.details, ["evil txt.exe   [31mred"]);

        for spoofed in ["gone\u{202e}", "\u{202e}", "-gone", "a b", ""] {
            let mut status = RemoteStatus::new(spoofed, "gone\u{202e}host");
            assert_eq!(status.title(), "Connecting to remote", "{spoofed:?}");
            let lines = status.lines();
            assert_eq!(lines[0], "REMOTE · remote");
            assert!(lines.contains(&"Connecting to gone host…".to_owned()));
            status.on_attempt(Err(failure.clone()));
            assert_eq!(status.title(), "Could not connect to remote");
            assert!(status.lines().iter().all(|line| {
                !line
                    .chars()
                    .any(|character| character.is_control() || is_bidi_control(character))
            }));
        }
        assert_eq!(profile_label("wintermute"), "wintermute");
        assert_eq!(profile_label("gone\u{202e}"), FALLBACK_PROFILE_LABEL);
    }

    #[test]
    fn a_terminal_launch_says_openssh_may_prompt_there() {
        let prompt = "OpenSSH may prompt on the terminal that started Splinterm.".to_owned();
        let detached = RemoteStatus::new("wintermute", "wintermute");
        assert!(!detached.lines().contains(&prompt));
        let attached = RemoteStatus::new("wintermute", "wintermute").with_terminal(true);
        assert!(attached.lines().contains(&prompt));
    }

    #[test]
    fn screens_explain_progress_failure_remedy_and_the_agent_path() {
        let mut status = RemoteStatus::new("wintermute", "case@wintermute.example:2222");
        let connecting = status.lines();
        assert_eq!(connecting[0], "REMOTE · wintermute");
        assert!(connecting.contains(&"Connecting to case@wintermute.example:2222…".to_owned()));
        assert!(connecting.contains(&"C / Esc: cancel".to_owned()));
        assert_eq!(status.title(), "Connecting to wintermute");

        status.on_attempt(Err(failure(RemoteFailureKind::AuthenticationFailed)));
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

    fn prompt_lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn a_secret_prompt_shows_openssh_text_and_a_field_without_its_contents() {
        let mut status = RemoteStatus::new("wintermute", "case@wintermute");
        let connecting = status.lines();
        assert!(connecting.contains(&NATIVE_PROMPT_NOTE.to_owned()));
        assert!(status.show_prompt(
            AskpassKind::Secret,
            &prompt_lines(&["case@wintermute's password:"])
        ));
        assert_eq!(status.title(), "Authenticating to wintermute");
        let screen = status.lines();
        assert!(screen.contains(
            &"OpenSSH, connecting to case@wintermute, is asking for a credential.".to_owned()
        ));
        assert!(screen.contains(&"  │ case@wintermute's password:".to_owned()));
        assert!(screen.contains(&"Password field: empty".to_owned()));
        assert!(screen.contains(&SECRET_HANDLING_NOTE.to_owned()));
        assert!(screen.contains(&"Enter: submit          Esc: cancel".to_owned()));
        status.set_prompt_input(true);
        let typed = status.lines();
        assert!(typed.contains(&"Password field: entered (hidden)".to_owned()));
        assert!(status.prompt().unwrap().has_input());
        // The field line is fixed text: no length, no mask per character.
        assert!(
            typed
                .iter()
                .all(|line| !line.contains('•') && !line.contains('*'))
        );
        let snapshot = status.snapshot(SplintId::new(), 3);
        assert!(snapshot.validate().is_ok());
        assert_eq!(snapshot.title, "Authenticating to wintermute");

        status.clear_prompt();
        assert_eq!(status.title(), "Connecting to wintermute");
        assert!(status.prompt().is_none());
    }

    #[test]
    fn confirm_and_notice_prompts_are_distinct_from_secrets() {
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        status.show_prompt(
            AskpassKind::Confirm,
            &prompt_lines(&[
                "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
            ]),
        );
        let screen = status.lines();
        assert!(screen.contains(&"Y: yes          N / Esc: no".to_owned()));
        assert!(!screen.iter().any(|line| line.starts_with("Password field")));
        status.show_prompt(
            AskpassKind::Notice,
            &prompt_lines(&["Confirm user presence for key ED25519-SK"]),
        );
        let screen = status.lines();
        assert!(screen.contains(&"C / Esc: cancel".to_owned()));
        assert!(!screen.iter().any(|line| line.starts_with("Password field")));
    }

    #[test]
    fn prompts_appear_only_while_connecting_and_end_with_the_attempt() {
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        status.show_prompt(AskpassKind::Secret, &prompt_lines(&["password:"]));
        status.on_attempt(Err(failure(RemoteFailureKind::AuthenticationFailed)));
        assert!(
            status.prompt().is_none(),
            "a finished attempt drops its prompt"
        );
        assert!(
            !status.show_prompt(AskpassKind::Secret, &prompt_lines(&["password:"])),
            "no prompt over a failure screen"
        );
        status.on_key(RemoteStatusKey::Retry);
        assert!(status.show_prompt(AskpassKind::Secret, &prompt_lines(&["password:"])));
        status.on_window_closed();
        assert!(status.prompt().is_none());

        let mut status = RemoteStatus::new("wintermute", "wintermute");
        status.show_prompt(AskpassKind::Secret, &prompt_lines(&["password:"]));
        status.on_key(RemoteStatusKey::Close);
        assert!(status.prompt().is_none());
    }

    #[test]
    fn long_prompt_lines_wrap_inside_the_status_screen() {
        use unicode_width::UnicodeWidthStr;

        let long = "The authenticity of host 'omarchy-test.example.internal (192.0.2.10)' can't be established, ok?";
        assert!(long.chars().count() <= crate::remote_askpass::MAX_PROMPT_LINE_CHARS);
        let word = "x".repeat(crate::remote_askpass::MAX_PROMPT_LINE_CHARS);
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        for kind in [
            AskpassKind::Secret,
            AskpassKind::Confirm,
            AskpassKind::Notice,
        ] {
            status.show_prompt(kind, &prompt_lines(&[long, &word, "short:"]));
            let quoted = status
                .lines()
                .into_iter()
                .filter_map(|line| line.strip_prefix("  │ ").map(str::to_owned))
                .collect::<Vec<_>>();
            assert!(quoted.iter().all(|line| line.width() <= PROMPT_WRAP_CELLS));
            assert!(quoted.iter().all(|line| line.width() + 4 <= 72));
            assert_eq!(quoted.len(), 5, "{quoted:?}");
            assert_eq!(quoted[..2].join(" "), long);
            assert_eq!(quoted[2..4].concat(), word);
            assert_eq!(quoted[4], "short:");
        }
        assert_eq!(wrap_cells("", 68), vec![String::new()]);
        assert_eq!(
            wrap_cells("界界界", 4),
            vec!["界界".to_owned(), "界".to_owned()]
        );
    }

    /// Codex review on #153: the status Window keeps its configured rows
    /// (`main.initial-rows` 2..), so a maximal prompt must fit every height
    /// and keep the key hints and OpenSSH's question.
    #[test]
    fn maximal_prompts_fit_every_supported_window_height() {
        let long = format!("{} end", "word ".repeat(19));
        assert!(long.chars().count() <= crate::remote_askpass::MAX_PROMPT_LINE_CHARS);
        let mut lines = vec![long; crate::remote_askpass::MAX_PROMPT_LINES - 1];
        lines.push("Password for omarchy@localhost:".to_owned());
        let cases = [
            (AskpassKind::Secret, "Enter: submit          Esc: cancel"),
            (AskpassKind::Confirm, "Y: yes          N / Esc: no"),
            (AskpassKind::Notice, "C / Esc: cancel"),
        ];
        for rows in 2..=30_u16 {
            for (kind, controls) in cases {
                let mut status = RemoteStatus::new("wintermute", "wintermute").with_rows(rows);
                status.show_prompt(kind, &lines);
                let screen = status.lines();
                assert!(
                    screen.len() <= usize::from(rows),
                    "{kind:?} at {rows} rows: {screen:?}"
                );
                assert_eq!(screen.last().unwrap(), controls, "{kind:?} at {rows} rows");
                let quoted = screen
                    .iter()
                    .filter(|line| line.starts_with("  │ "))
                    .collect::<Vec<_>>();
                assert_eq!(
                    quoted.last().unwrap().as_str(),
                    "  │ Password for omarchy@localhost:",
                    "{kind:?} at {rows} rows"
                );
                if kind == AskpassKind::Secret && (19..=24).contains(&rows) {
                    // Every line of the full screen stays; only quoted rows
                    // give way, behind a `…` row.
                    assert_eq!(screen.len(), usize::from(rows));
                    assert_eq!(quoted[0], "  │ …");
                    assert!(screen.contains(&SECRET_HANDLING_NOTE.to_owned()));
                    assert!(screen.contains(&"Password field: empty".to_owned()));
                }
            }
        }
        // The default height leaves the whole wrapped prompt but its start.
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        status.show_prompt(AskpassKind::Secret, &lines);
        assert_eq!(status.lines().len(), usize::from(DEFAULT_STATUS_ROWS));
    }

    #[test]
    fn prompt_text_is_cleaned_again_before_display() {
        let mut status = RemoteStatus::new("wintermute", "wintermute");
        status.show_prompt(
            AskpassKind::Secret,
            &prompt_lines(&["pass\u{202e}word\u{1b}[2J:"]),
        );
        assert!(status.lines().iter().all(|line| {
            !line
                .chars()
                .any(|character| character.is_control() || is_bidi_control(character))
        }));
    }
}
