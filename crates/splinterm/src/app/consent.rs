use std::{
    io::{self, Read, Write},
    sync::mpsc as std_mpsc,
};

use anyhow::{Context, Result, bail};
use splinterd::dialog_shortcut::{BackgroundRegistration, DialogShortcut};
use splinterm::{
    AccessConsentView, TrustedConsentUi, WindowOptions,
    config::{AppConfig, ResolvedTheme},
    run_window,
};
use splinterm_protocol::{
    AccessLevel, ActiveScreen, CellAttributes, ColorSource, ConsentDecision, ConsentPrompt,
    ConsentReplacement, ConsentReply, ConsentTarget, GrantDuration, MAX_CONSENT_FRAME_BYTES,
    MAX_CONSENT_PATH_CHARS, TerminalCell, TerminalInputModes, TerminalRow, TerminalSnapshot,
    UnderlineStyle, shared_identity_line,
};

/// Reads one length-prefixed JSON frame of at most `maximum` bytes from a
/// trusted dialog's private channel.
pub(in crate::app) fn read_private_frame_bounded<T: serde::de::DeserializeOwned>(
    reader: &mut impl Read,
    maximum: usize,
) -> Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > maximum {
        bail!("invalid private dialog frame length");
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).context("invalid private dialog frame")
}

/// Writes one length-prefixed JSON frame of at most `maximum` bytes to a
/// trusted dialog's private channel.
pub(in crate::app) fn write_private_frame_bounded<T: serde::Serialize>(
    writer: &mut impl Write,
    value: &T,
    maximum: usize,
) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode private dialog frame")?;
    if body.is_empty() || body.len() > maximum {
        bail!("private dialog frame exceeds limit");
    }
    writer.write_all(&u32::try_from(body.len())?.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn read_private_frame<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    read_private_frame_bounded(reader, MAX_CONSENT_FRAME_BYTES)
}

fn write_private_frame<T: serde::Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    write_private_frame_bounded(writer, value, MAX_CONSENT_FRAME_BYTES)
}

fn unsafe_label_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}'
                | '\u{feff}'
        )
}

/// At most `maximum` characters of `value`, with control and bidi-formatting
/// characters replaced, so daemon- or peer-supplied text cannot restyle or
/// reorder trusted UI.
pub(in crate::app) fn trusted_label(value: &str, maximum: usize) -> String {
    value
        .chars()
        .take(maximum)
        .map(|character| {
            if unsafe_label_character(character) {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

/// The longest agent or workspace name the prompt shows, in characters;
/// longer names end in an ellipsis. Fixed text always comes first on a line,
/// so a long name can never push it out of view.
const DISPLAY_NAME_CHARS: usize = 60;

/// The widest line the prompt draws, in characters. Longer text wraps onto
/// indented lines, so nothing the person must read is ever cut off.
const PROMPT_LINE_CHARS: usize = 100;

/// `value` as one line of display text: unsafe characters replaced, runs of
/// whitespace collapsed to one space, and at most `maximum` characters, the
/// last an ellipsis when it was cut.
fn display_name(value: &str, maximum: usize) -> String {
    // Whitespace (tabs and newlines among it) collapses first; what stays
    // unsafe is then replaced.
    let collapsed = trusted_label(
        &value.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_CONSENT_PATH_CHARS,
    );
    if collapsed.chars().count() <= maximum {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(maximum.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Wraps `text` into lines of at most [`PROMPT_LINE_CHARS`] characters,
/// breaking at spaces where it can and inside a word (a path) where it must.
/// Every line after the first starts with `indent`.
fn wrap(text: &str, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let body = text.trim_start_matches(' ');
    // A leading indent is kept as given.
    let mut line_chars = text.len() - body.len();
    let mut line = " ".repeat(line_chars);
    for word in body.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let separator = usize::from(line_chars > 0 && !line.ends_with(' '));
            if line_chars + separator + word.len() <= PROMPT_LINE_CHARS {
                if separator == 1 {
                    line.push(' ');
                }
                line.extend(word.iter());
                line_chars += separator + word.len();
                break;
            }
            let indent_only = line.trim().is_empty();
            if !indent_only {
                // The word starts a new line.
                lines.push(std::mem::take(&mut line));
                line.push_str(indent);
                line_chars = indent.chars().count();
                continue;
            }
            // A word too long for any line is split.
            let room = PROMPT_LINE_CHARS.saturating_sub(line_chars).max(1);
            line.extend(word.drain(..room));
            lines.push(std::mem::take(&mut line));
            line.push_str(indent);
            line_chars = indent.chars().count();
        }
    }
    if !line.trim().is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// `lines` padded with blank lines to `count`, so every selection keeps
/// the same rows.
fn padded(mut lines: Vec<String>, count: usize) -> Vec<String> {
    lines.resize(count.max(lines.len()), String::new());
    lines
}

fn consent_target_line(target: &ConsentTarget) -> String {
    match target {
        ConsentTarget::Splint {
            splint_id,
            incarnation,
        } => format!("Splint: {splint_id:?} · incarnation {incarnation}"),
        // The name is user data any agent with Manage can change, so the
        // short ID is always shown, before it.
        ConsentTarget::Lair { lair_id, lair_name } => {
            let short_id: String = lair_id.to_string().chars().take(8).collect();
            format!(
                "Workspace {short_id}: '{}'",
                display_name(lair_name, DISPLAY_NAME_CHARS)
            )
        }
    }
}

/// The workspace name as grant labels and duration rows show it.
fn workspace_label(target: &ConsentTarget) -> String {
    let ConsentTarget::Lair { lair_name, .. } = target else {
        return String::new();
    };
    display_name(lair_name, DISPLAY_NAME_CHARS)
}

/// What the grant names: the level, "Custom access" for any other workspace
/// scope set, or "this access" for a Splint request.
fn access_label(level: Option<AccessLevel>, target: &ConsentTarget) -> &'static str {
    match (level, target) {
        (Some(level), _) => level.label(),
        (None, ConsentTarget::Lair { .. }) => "Custom access",
        (None, ConsentTarget::Splint { .. }) => "this access",
    }
}

/// The grant button's exact words for one duration (ADR 0014), for example
/// "Always allow Manage for 'Workspace'".
fn grant_label(access: &str, duration: GrantDuration, workspace: &str) -> String {
    match duration {
        GrantDuration::Once => format!("Allow {access} once"),
        GrantDuration::WorkspaceSession => format!("Allow {access} while '{workspace}' is open"),
        GrantDuration::Remembered => format!("Always allow {access} for '{workspace}'"),
    }
}

fn duration_row(duration: GrantDuration, workspace: &str) -> String {
    match duration {
        GrantDuration::Once => "Just this once: five minutes, for this process only".to_owned(),
        GrantDuration::WorkspaceSession => format!(
            "While '{workspace}' is open: until this workspace is closed or Splinterm restarts"
        ),
        GrantDuration::Remembered => {
            format!("Always for '{workspace}': until you revoke it or the workspace is removed")
        }
    }
}

fn replacement_line(
    replaced: &ConsentReplacement,
    duration: GrantDuration,
    workspace: &str,
) -> String {
    let access = match replaced.level {
        Some(level) => level.label().to_owned(),
        None => format!("Custom access ({} scopes)", replaced.scopes.len()),
    };
    format!(
        "This replaces the earlier answer: {}.",
        grant_label(&access, duration, workspace)
    )
}

/// Wraps the scope labels into lines of at most [`PROMPT_LINE_CHARS`].
fn scope_lines(prompt: &ConsentPrompt) -> Vec<String> {
    let labels: Vec<_> = prompt.scopes.iter().map(|scope| scope.label()).collect();
    wrap(&format!("Scopes: {}", labels.join(", ")), "        ")
}

/// One rendering of the access prompt's text.
struct ConsentLayout {
    lines: Vec<String>,
    /// Each line that shows a duration, with that duration's index.
    duration_rows: Vec<(usize, usize)>,
}

/// The lines one chosen duration adds below the duration list.
struct ChosenLines {
    shared: Vec<String>,
    replaced: Vec<String>,
    grant: Vec<String>,
}

/// The lines a chosen duration adds below the list: who shares it, what it
/// replaces, and the grant action that names it exactly.
fn chosen_lines(prompt: &ConsentPrompt, selected: usize, workspace: &str) -> ChosenLines {
    let chosen = prompt.durations.get(selected);
    let shared = chosen
        .and_then(|offer| offer.shared_by.as_deref())
        .map(|path| {
            wrap(
                &shared_identity_line(&trusted_label(path, MAX_CONSENT_PATH_CHARS)),
                "    ",
            )
        })
        .unwrap_or_default();
    let replaced = chosen
        .and_then(|offer| {
            offer.replaces.as_ref().map(|replaced| {
                wrap(
                    &replacement_line(replaced, offer.duration, workspace),
                    "    ",
                )
            })
        })
        .unwrap_or_default();
    let label = grant_label(
        access_label(prompt.level, &prompt.target),
        chosen.map_or(GrantDuration::Once, |offer| offer.duration),
        workspace,
    );
    ChosenLines {
        shared,
        replaced,
        grant: wrap(&format!("Grant: {label}"), "       "),
    }
}

/// The prompt's text with the duration at `selected` chosen. Every
/// selection has the same lines in the same places, so choosing a duration
/// never moves the rows the pointer selects. No line is wider than
/// [`PROMPT_LINE_CHARS`].
fn consent_layout(
    prompt: &ConsentPrompt,
    selected: usize,
    shortcut: Option<DialogShortcut>,
) -> ConsentLayout {
    let workspace = workspace_label(&prompt.target);
    let mut lines = vec!["TRUSTED SPLINTERM ACCESS REQUEST".to_owned(), String::new()];
    if let Some(name) = &prompt.agent_name {
        lines.extend(wrap(
            &format!(
                "Agent (reported by the agent, not verified): {}",
                display_name(name, DISPLAY_NAME_CHARS)
            ),
            "    ",
        ));
    }
    lines.extend(wrap(
        &format!(
            "Program: {}",
            trusted_label(&prompt.requester, MAX_CONSENT_PATH_CHARS)
        ),
        "         ",
    ));
    lines.push(format!(
        "Process: PID {} · UID {}",
        prompt.requester_pid, prompt.requester_uid
    ));
    lines.extend(wrap(&consent_target_line(&prompt.target), "    "));
    lines.push(String::new());
    lines.extend(wrap(
        &match prompt.level {
            Some(level) => format!("Access: {} ({})", level.label(), level.summary()),
            None if matches!(prompt.target, ConsentTarget::Lair { .. }) => {
                "Access: Custom, exactly these scopes and nothing more".to_owned()
            }
            None => "Access: exactly these scopes, once".to_owned(),
        },
        "    ",
    ));
    lines.extend(scope_lines(prompt));
    lines.push(String::new());
    lines.push("How long (choose with the arrow keys or a click):".to_owned());
    let mut duration_rows = Vec::new();
    for (index, offer) in prompt.durations.iter().enumerate() {
        let marker = match (offer.withheld, index == selected) {
            (Some(_), _) => "(-)",
            (None, true) => "(•)",
            (None, false) => "( )",
        };
        for line in wrap(
            &format!("  {marker} {}", duration_row(offer.duration, &workspace)),
            "      ",
        ) {
            duration_rows.push((lines.len(), index));
            lines.push(line);
        }
        if let Some(withheld) = offer.withheld {
            lines.extend(wrap(
                &format!("      Unavailable: {}", withheld.reason()),
                "        ",
            ));
        }
    }
    lines.push(String::new());
    // Reserved for every selection at its tallest, blank when they do not
    // apply, so the layout never moves.
    let every: Vec<_> = (0..prompt.durations.len())
        .map(|index| chosen_lines(prompt, index, &workspace))
        .collect();
    let tallest = |part: fn(&ChosenLines) -> usize| every.iter().map(part).max().unwrap_or(0);
    let shared_rows = tallest(|lines| lines.shared.len());
    let replaced_rows = tallest(|lines| lines.replaced.len());
    let grant_rows = tallest(|lines| lines.grant.len());
    let ChosenLines {
        shared,
        replaced,
        grant,
    } = chosen_lines(prompt, selected, &workspace);
    lines.extend(padded(shared, shared_rows));
    lines.extend(padded(replaced, replaced_rows));
    lines.push(String::new());
    lines.extend(padded(grant, grant_rows));
    lines.extend([
        "G or the green area (right) grants. Enter, Escape, D, or the red area (left) deny."
            .to_owned(),
        keyboard_hint("prompt", shortcut),
    ]);
    ConsentLayout {
        lines,
        duration_rows,
    }
}

/// The share of the dialog's rows above the action areas, which cover the
/// bottom 22% of the Window.
const CONSENT_TEXT_ROWS_PERCENT: usize = 72;

/// Renders the prompt for each selection at one fixed size: as wide as its
/// widest selection and tall enough that the text stays above the action
/// areas.
fn consent_renderer(
    prompt: ConsentPrompt,
    shortcut: Option<DialogShortcut>,
) -> splinterm::AccessConsentRenderer {
    let layouts =
        || (0..prompt.durations.len()).map(|selected| consent_layout(&prompt, selected, shortcut));
    let width = layouts()
        .flat_map(|layout| layout.lines.into_iter().map(|line| line.len()))
        .max()
        .unwrap_or(1)
        .clamp(64, 120);
    let rows = layouts()
        .map(|layout| layout.lines.len())
        .max()
        .unwrap_or(1);
    let minimum_rows = (rows * 100).div_ceil(CONSENT_TEXT_ROWS_PERCENT);
    let snapshot_splint_id = match &prompt.target {
        ConsentTarget::Splint { splint_id, .. } => *splint_id,
        ConsentTarget::Lair { .. } => splinterm_core::SplintId::new(),
    };
    Box::new(move |selected| {
        let mut layout = consent_layout(&prompt, selected, shortcut);
        // Every selection is drawn as wide as the widest one.
        let title = &mut layout.lines[0];
        let padding = width.saturating_sub(title.len());
        title.extend(std::iter::repeat_n(' ', padding));
        AccessConsentView {
            snapshot: consent_snapshot(
                &prompt.target,
                snapshot_splint_id,
                layout.lines,
                minimum_rows,
            ),
            duration_rows: layout.duration_rows,
        }
    })
}

fn consent_input_modes() -> TerminalInputModes {
    TerminalInputModes {
        application_cursor: false,
        application_keypad: false,
        focus_reporting: false,
        bracketed_paste: false,
        cursor_visible: false,
        cursor_blink: false,
        mouse_tracking: splinterm_protocol::MouseTracking::None,
        sgr_mouse: false,
    }
}

/// A static, cursorless snapshot of `lines` for a trusted dialog Window, in
/// the consent dialog's colors. The column count is the longest line's byte
/// length clamped to 64..=120, and lines are cut there; blank rows fill up to
/// at least `minimum_rows`.
pub(in crate::app) fn static_dialog_snapshot(
    lines: Vec<String>,
    minimum_rows: usize,
    title: &str,
    splint_id: splinterm_core::SplintId,
    incarnation: u64,
) -> TerminalSnapshot {
    let columns = lines
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(1)
        .clamp(64, 120);
    let rows = lines.len().max(minimum_rows);
    let blank_attributes = CellAttributes {
        bold: false,
        dim: false,
        italic: false,
        underline: UnderlineStyle::None,
        underline_color_source: ColorSource::Default,
        underline_color: 0,
        strikethrough: false,
        blink: false,
        conceal: false,
        reverse: false,
        foreground_source: ColorSource::Default,
        foreground: 0,
        background_source: ColorSource::Default,
        background: 0,
    };
    let mut visible_rows: Vec<_> = lines
        .into_iter()
        .map(|line| TerminalRow {
            row_id: None,
            linebreak: false,
            cells: line
                .chars()
                .take(columns)
                .map(|character| TerminalCell {
                    content: character.to_string(),
                    spacer_remaining: None,
                    attributes: blank_attributes,
                })
                .collect(),
        })
        .collect();
    visible_rows.resize_with(rows, || TerminalRow {
        row_id: None,
        linebreak: false,
        cells: Vec::new(),
    });
    // A Window refuses a snapshot whose visible rows lack distinct, nonzero
    // identities (`TerminalSnapshot::validate`), before it maps.
    for (row, id) in visible_rows.iter_mut().zip(1_u64..) {
        row.row_id = Some(id);
    }
    TerminalSnapshot {
        splint_id,
        incarnation,
        revision: 1,
        columns,
        rows,
        cursor_column: -1,
        cursor_row: -1,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: consent_input_modes(),
        palette: vec![0; 256],
        default_colors: [0x00f4_f0e8, 0x0014_1820, 0x00e0_a030],
        title: title.to_owned(),
        visible_rows,
        history_generation: 1,
        oldest_available_scrollback_row_id: None,
        newest_available_scrollback_row_id: None,
        scrollback_rows: Vec::new(),
        available_scrollback_rows: 0,
        omitted_oldest_scrollback_rows: 0,
        images: None,
        exited_code: None,
        exited_signal: None,
    }
}

/// The dialog's snapshot, with one Splint identity for every rendering.
fn consent_snapshot(
    target: &ConsentTarget,
    snapshot_splint_id: splinterm_core::SplintId,
    lines: Vec<String>,
    minimum_rows: usize,
) -> TerminalSnapshot {
    let snapshot_incarnation = match target {
        ConsentTarget::Splint { incarnation, .. } => *incarnation,
        ConsentTarget::Lair { .. } => 0,
    };
    static_dialog_snapshot(
        lines,
        minimum_rows,
        "Trusted access request",
        snapshot_splint_id,
        snapshot_incarnation,
    )
}

/// The line that tells the person how to engage a trusted dialog's keyboard:
/// a click inside, or the dialog shortcut when it is on.
pub(in crate::app) fn keyboard_hint(noun: &str, shortcut: Option<DialogShortcut>) -> String {
    shortcut.map_or_else(
        || format!("Click inside this {noun} first to answer with the keyboard."),
        |shortcut| {
            format!(
                "Click inside this {noun} or press {} first to answer with the keyboard.",
                shortcut.label()
            )
        },
    )
}

/// Shows a trusted dialog Window and returns once it closes.
///
/// When Hyprland maps trusted dialogs without keyboard focus (the Omarchy
/// integration's module, or the same rule in the person's own file), the
/// dialog shortcut is also registered with Hyprland, in the background while
/// the Window maps, so it reaches the dialog from any Window
/// (`splinterd::dialog_shortcut`). The caller writes its answer first and
/// then finishes the returned registration; the process that reaps this one
/// removes it again. Otherwise the shortcut works only inside the dialog
/// once it has focus.
pub(in crate::app) fn run_trusted_dialog_window(
    options: WindowOptions,
) -> Result<BackgroundRegistration> {
    let shortcut = options
        .trusted_consent
        .as_ref()
        .and_then(TrustedConsentUi::shortcut)
        .filter(|_| global_dialog_shortcut());
    let registration = BackgroundRegistration::start(shortcut);
    run_window(options)?;
    Ok(registration)
}

/// Whether trusted dialogs map without keyboard focus here, so they need the
/// Hyprland binding to be reached from the keyboard. Only reads files.
fn global_dialog_shortcut() -> bool {
    super::omarchy_integration::Roots::from_env()
        .is_ok_and(|roots| super::omarchy_integration::dialog_focus_ready(&roots))
}

/// Configures the renderer a trusted dialog child draws with from the
/// person's configuration, as every other Window does. Without it the child
/// draws with the explicit startup fallback font alone, ignoring
/// `main.font` and Fontconfig's `monospace`, and fails where that font is
/// not installed. Dialogs keep the default, opaque theme.
pub(in crate::app) fn configure_dialog_renderer(config: &AppConfig) -> Result<()> {
    super::sessions::configure_picker_renderer(config, ResolvedTheme::default())
}

/// The consent dialog's Window, sized to its first rendering, answering on
/// `decision`.
fn consent_window_options(
    prompt: ConsentPrompt,
    decision: std_mpsc::Sender<ConsentDecision>,
    shortcut: Option<DialogShortcut>,
) -> WindowOptions {
    let durations = prompt
        .durations
        .iter()
        .map(|offer| (offer.duration, offer.withheld.is_none()))
        .collect();
    let (trusted_consent, snapshot) =
        TrustedConsentUi::access_consent(decision, durations, consent_renderer(prompt, shortcut));
    let trusted_consent = trusted_consent.with_shortcut(shortcut);
    let columns = u16::try_from(snapshot.columns).unwrap_or(u16::MAX);
    let rows = u16::try_from(snapshot.rows).unwrap_or(u16::MAX);
    WindowOptions {
        snapshot: Some(snapshot),
        trusted_consent: Some(trusted_consent),
        initial_columns: columns,
        initial_rows: rows,
        ..WindowOptions::default()
    }
}

pub(in crate::app) fn run_consent_client(shortcut: Option<DialogShortcut>) -> Result<()> {
    let prompt: ConsentPrompt = read_private_frame(&mut io::stdin().lock())?;
    if let Err(reason) = prompt.validate() {
        bail!("invalid trusted consent prompt: {reason}");
    }
    let capability = prompt.capability.clone();
    let (decision, receiver) = std_mpsc::channel();
    let registration =
        run_trusted_dialog_window(consent_window_options(prompt, decision, shortcut))?;
    // Closing the Window, or any way out without an answer, denies.
    let decision = receiver.try_recv().unwrap_or(ConsentDecision::Deny);
    let written = write_private_frame(
        &mut io::stdout().lock(),
        &ConsentReply {
            capability,
            decision,
        },
    );
    // The answer goes first; the shortcut registration ends after it.
    registration.finish();
    written
}

#[cfg(test)]
mod tests {
    use splinterm_protocol::{
        AccessLevel, AccessScope, ConsentDurationOffer, ConsentPrompt, ConsentReplacement,
        ConsentTarget, DurationWithheld, GrantDuration, MAX_CONSENT_PATH_CHARS,
    };

    use splinterd::dialog_shortcut::DialogShortcut;

    use super::{
        consent_layout, consent_renderer, consent_window_options, grant_label, keyboard_hint,
        read_private_frame_bounded, trusted_label, write_private_frame_bounded,
    };

    #[allow(
        clippy::unnecessary_wraps,
        reason = "the layout takes the configured setting, which can be off"
    )]
    fn shortcut() -> Option<DialogShortcut> {
        Some(DialogShortcut::default())
    }

    #[test]
    fn the_keyboard_hint_names_the_dialog_shortcut_only_when_it_is_on() {
        assert_eq!(
            keyboard_hint("prompt", shortcut()),
            "Click inside this prompt or press Ctrl+Alt+G first to answer with the keyboard."
        );
        assert_eq!(
            keyboard_hint(
                "dialog",
                DialogShortcut::parse_setting("ctrl+alt+7").unwrap()
            ),
            "Click inside this dialog or press Ctrl+Alt+7 first to answer with the keyboard."
        );
        assert_eq!(
            keyboard_hint("dialog", None),
            "Click inside this dialog first to answer with the keyboard."
        );
    }

    fn offer(duration: GrantDuration, withheld: Option<DurationWithheld>) -> ConsentDurationOffer {
        ConsentDurationOffer {
            duration,
            withheld,
            shared_by: (withheld.is_none() && duration != GrantDuration::Once)
                .then(|| "/usr/bin/splinterm-mcp".to_owned()),
            replaces: None,
        }
    }

    fn manage_prompt() -> ConsentPrompt {
        ConsentPrompt {
            capability: vec![1; splinterm_protocol::CONSENT_CAPABILITY_BYTES],
            requester: "/usr/bin/splinterm-mcp".to_owned(),
            requester_pid: 42,
            requester_uid: 1000,
            agent_name: Some("Claude Code".to_owned()),
            target: ConsentTarget::Lair {
                lair_id: splinterm_core::LairId::new(),
                lair_name: "Workspace".to_owned(),
            },
            scopes: AccessLevel::Manage.scopes().to_vec(),
            level: Some(AccessLevel::Manage),
            durations: vec![
                offer(GrantDuration::Once, None),
                offer(GrantDuration::WorkspaceSession, None),
                offer(GrantDuration::Remembered, None),
            ],
        }
    }

    #[test]
    fn grant_labels_name_the_exact_choice() {
        assert_eq!(
            grant_label("Look", GrantDuration::Once, "Workspace"),
            "Allow Look once"
        );
        assert_eq!(
            grant_label(
                "Look and type",
                GrantDuration::WorkspaceSession,
                "Workspace"
            ),
            "Allow Look and type while 'Workspace' is open"
        );
        assert_eq!(
            grant_label("Manage", GrantDuration::Remembered, "Workspace"),
            "Always allow Manage for 'Workspace'"
        );
    }

    #[test]
    fn the_prompt_names_the_level_agent_program_and_workspace_with_its_short_id() {
        let prompt = manage_prompt();
        let ConsentTarget::Lair { lair_id, .. } = &prompt.target else {
            unreachable!()
        };
        let text = consent_layout(&prompt, 0, shortcut()).lines.join("\n");
        assert!(text.contains("Agent (reported by the agent, not verified): Claude Code"));
        assert!(text.contains("Program: /usr/bin/splinterm-mcp"));
        let short_id: String = lair_id.to_string().chars().take(8).collect();
        assert!(text.contains(&format!("Workspace {short_id}: 'Workspace'")));
        assert!(text.contains(
            "Access: Manage (Read terminals, type commands, and create or close sessions)"
        ));
        assert!(text.contains("start processes in this Lair"));
        // Just this once is preselected and needs no shared-identity line.
        assert!(text.contains("(•) Just this once"));
        assert!(text.contains("Grant: Allow Manage once"));
        assert!(!text.contains("shares this access"));
    }

    #[test]
    fn a_longer_choice_shows_who_shares_it_and_what_it_replaces() {
        let mut prompt = manage_prompt();
        prompt.durations[2].replaces = Some(ConsentReplacement {
            level: Some(AccessLevel::Look),
            scopes: AccessLevel::Look.scopes().to_vec(),
        });
        let text = consent_layout(&prompt, 2, shortcut()).lines.join("\n");
        assert!(text.contains("(•) Always for 'Workspace'"));
        assert!(text.contains("Grant: Always allow Manage for 'Workspace'"));
        assert!(text.contains("Any program that runs /usr/bin/splinterm-mcp shares this access."));
        assert!(
            text.contains("This replaces the earlier answer: Always allow Look for 'Workspace'.")
        );
        let session = consent_layout(&prompt, 1, shortcut()).lines.join("\n");
        assert!(session.contains("Grant: Allow Manage while 'Workspace' is open"));
        assert!(!session.contains("replaces"));
    }

    #[test]
    fn withheld_durations_show_their_reason_and_custom_lists_every_scope() {
        let mut prompt = manage_prompt();
        prompt.scopes = vec![AccessScope::Observe, AccessScope::ControlTakeover];
        prompt.level = None;
        prompt.durations = vec![
            offer(GrantDuration::Once, None),
            offer(
                GrantDuration::WorkspaceSession,
                Some(DurationWithheld::CustomTakeover),
            ),
            offer(
                GrantDuration::Remembered,
                Some(DurationWithheld::CustomTakeover),
            ),
        ];
        assert_eq!(prompt.validate(), Ok(()));
        let layout = consent_layout(&prompt, 0, shortcut());
        let text = layout.lines.join("\n");
        assert!(text.contains("Access: Custom, exactly these scopes and nothing more"));
        assert!(text.contains("observe visible terminal, take over terminal control"));
        assert!(text.contains("(-) While 'Workspace' is open"));
        assert_eq!(
            text.matches(DurationWithheld::CustomTakeover.reason())
                .count(),
            2
        );
        assert!(text.contains("Grant: Allow Custom access once"));
        // Each duration's first row is where the layout says it is.
        for (index, duration) in ["Just this once", "While", "Always"]
            .into_iter()
            .enumerate()
        {
            let (row, _) = layout
                .duration_rows
                .iter()
                .find(|(_, shown)| *shown == index)
                .unwrap();
            assert!(
                layout.lines[*row].contains(duration),
                "{}",
                layout.lines[*row]
            );
        }
    }

    /// The longest inputs a valid prompt can carry: maximal paths, names,
    /// and scopes, with every longer duration shared and replacing.
    fn longest_prompt() -> ConsentPrompt {
        let path = format!("/{}", "p".repeat(MAX_CONSENT_PATH_CHARS - 1));
        let shared = format!("/{}", "s".repeat(MAX_CONSENT_PATH_CHARS - 1));
        let mut prompt = manage_prompt();
        prompt.agent_name = Some(format!("Agent  \t name {}", "a".repeat(200))[..128].to_owned());
        prompt.requester.clone_from(&path);
        prompt.target = ConsentTarget::Lair {
            lair_id: splinterm_core::LairId::new(),
            lair_name: "n".repeat(MAX_CONSENT_PATH_CHARS),
        };
        prompt.scopes = vec![
            AccessScope::TopologyObserve,
            AccessScope::Observe,
            AccessScope::Scrollback,
            AccessScope::Input,
            AccessScope::Resize,
            AccessScope::ControlTakeover,
            AccessScope::ProcessSpawn,
            AccessScope::TopologyLayout,
            AccessScope::TopologyName,
            AccessScope::Terminate,
            AccessScope::ProcessRestore,
        ];
        prompt.level = None;
        for offer in &mut prompt.durations[1..] {
            offer.shared_by = Some(shared.clone());
            offer.replaces = Some(ConsentReplacement {
                level: None,
                scopes: prompt.scopes.clone(),
            });
        }
        assert_eq!(prompt.validate(), Ok(()));
        prompt
    }

    /// A Splint prompt as the daemon sends it: "Just this once" only.
    fn splint_prompt() -> ConsentPrompt {
        let prompt = ConsentPrompt {
            capability: vec![1; splinterm_protocol::CONSENT_CAPABILITY_BYTES],
            requester: "/usr/bin/splinterm-mcp".to_owned(),
            requester_pid: 42,
            requester_uid: 1000,
            agent_name: None,
            target: ConsentTarget::Splint {
                splint_id: splinterm_core::SplintId::new(),
                incarnation: 3,
            },
            scopes: vec![AccessScope::Observe, AccessScope::Input],
            level: None,
            durations: vec![offer(GrantDuration::Once, None)],
        };
        assert_eq!(prompt.validate(), Ok(()));
        prompt
    }

    /// Every consent Window gets as far as the compositor: each selection's
    /// snapshot validates, and the Window passes every check `run_window`
    /// makes before it connects. Dialog snapshots once had no visible row
    /// identities, which `TerminalSnapshot::validate` refuses, so no trusted
    /// dialog ever mapped.
    #[test]
    fn every_consent_window_passes_the_checks_before_the_compositor() {
        // The renderer the dialog child configures, from the default
        // configuration: Fontconfig's `monospace`.
        super::configure_dialog_renderer(&splinterm::config::AppConfig::default()).unwrap();
        for prompt in [manage_prompt(), splint_prompt(), longest_prompt()] {
            let render = consent_renderer(prompt.clone(), shortcut());
            for selected in 0..prompt.durations.len() {
                let snapshot = render(selected).snapshot;
                assert_eq!(snapshot.validate().map_err(|error| error.message), Ok(()));
            }
            for shortcut in [shortcut(), None] {
                let (decision, _receiver) = std::sync::mpsc::channel();
                let options = consent_window_options(prompt.clone(), decision, shortcut);
                splinterm::check_window_options(options).unwrap();
            }
        }
    }

    /// The longest inputs a valid prompt can carry still show every fixed
    /// word and every character of the paths: nothing is cut off.
    #[test]
    fn the_longest_prompt_fits_its_columns_and_cuts_nothing() {
        let path = format!("/{}", "p".repeat(MAX_CONSENT_PATH_CHARS - 1));
        let shared = format!("/{}", "s".repeat(MAX_CONSENT_PATH_CHARS - 1));
        let prompt = longest_prompt();
        let render = consent_renderer(prompt.clone(), shortcut());
        for selected in 0..prompt.durations.len() {
            let view = render(selected);
            let layout = consent_layout(&prompt, selected, shortcut());
            for line in &layout.lines {
                assert!(
                    line.chars().count() <= super::PROMPT_LINE_CHARS
                        && line.chars().count() <= view.snapshot.columns,
                    "{} chars: {line}",
                    line.chars().count()
                );
            }
            // Joined back, the lines hold every character of each path.
            let joined: String = layout
                .lines
                .iter()
                .map(|line| line.trim())
                .collect::<Vec<_>>()
                .concat();
            assert!(joined.contains(&path));
            if selected > 0 {
                assert!(joined.contains(&shared));
            }
            let text = layout.lines.join("\n");
            assert!(text.contains("Agent (reported by the agent, not verified): Agent name"));
            assert!(text.contains(&format!("{}…", "a".repeat(10))));
            assert!(text.contains(&format!("'{}…'", "n".repeat(59))));
            assert!(text.contains(
                "Click inside this prompt or press Ctrl+Alt+G first to answer with the keyboard."
            ));
            // The rendered snapshot keeps every row of the text.
            assert!(layout.lines.len() <= view.snapshot.rows);
        }
    }

    #[test]
    fn every_selection_renders_at_one_size_with_the_same_rows() {
        let prompt = manage_prompt();
        let render = consent_renderer(prompt.clone(), shortcut());
        let views: Vec<_> = (0..prompt.durations.len()).map(&render).collect();
        for view in &views {
            assert_eq!(view.snapshot.columns, views[0].snapshot.columns);
            assert_eq!(view.snapshot.rows, views[0].snapshot.rows);
            assert_eq!(view.snapshot.splint_id, views[0].snapshot.splint_id);
            assert_eq!(view.duration_rows, views[0].duration_rows);
        }
        // The text stays above the action areas in the bottom 22%.
        let last_text_row = consent_layout(&prompt, 2, shortcut()).lines.len();
        assert!(last_text_row * 100 <= views[0].snapshot.rows * 72);
    }

    #[test]
    fn workspace_names_are_bounded_in_labels_and_sanitized() {
        let mut prompt = manage_prompt();
        prompt.target = ConsentTarget::Lair {
            lair_id: splinterm_core::LairId::new(),
            lair_name: format!("{}\u{202e}", "w".repeat(60)),
        };
        let text = consent_layout(&prompt, 2, shortcut()).lines.join("\n");
        assert!(text.contains(&format!("Always allow Manage for '{}…'", "w".repeat(59))));
        assert!(!text.contains('\u{202e}'));
    }

    #[test]
    fn trusted_consent_labels_replace_control_characters() {
        assert_eq!(
            trusted_label("safe\nspoof\u{1b}\u{202e}rtl\u{2066}", 64),
            "safe�spoof��rtl�"
        );
        assert_eq!(trusted_label("abcdef", 3), "abc");
    }

    #[test]
    fn private_frames_round_trip_within_their_bound_only() {
        let mut buffer = Vec::new();
        write_private_frame_bounded(&mut buffer, &vec![1_u8, 2, 3], 64).unwrap();
        let decoded: Vec<u8> = read_private_frame_bounded(&mut buffer.as_slice(), 64).unwrap();
        assert_eq!(decoded, [1, 2, 3]);
        assert!(read_private_frame_bounded::<Vec<u8>>(&mut buffer.as_slice(), 4).is_err());
        assert!(write_private_frame_bounded(&mut Vec::new(), &"x".repeat(64), 16).is_err());
    }
}
