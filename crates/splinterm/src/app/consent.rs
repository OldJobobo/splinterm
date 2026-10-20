use std::{
    io::{self, Read, Write},
    sync::mpsc as std_mpsc,
};

use anyhow::{Context, Result, bail};
use splinterm::{AccessConsentView, TrustedConsentUi, WindowOptions, run_window};
use splinterm_protocol::{
    AccessLevel, ActiveScreen, CellAttributes, ColorSource, ConsentDecision, ConsentPrompt,
    ConsentReplacement, ConsentReply, ConsentTarget, GrantDuration, MAX_AGENT_NAME_CHARS,
    MAX_CONSENT_FRAME_BYTES, MAX_CONSENT_PATH_CHARS, TerminalCell, TerminalInputModes, TerminalRow,
    TerminalSnapshot, UnderlineStyle, shared_identity_line,
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

/// The longest workspace name, in characters, inside a grant label or a
/// duration row; longer names end in an ellipsis there. The workspace line
/// shows the name in full.
const WORKSPACE_LABEL_CHARS: usize = 40;

/// The widest wrapped scope line, in characters.
const SCOPE_LINE_CHARS: usize = 100;

fn consent_target_line(target: &ConsentTarget) -> String {
    match target {
        ConsentTarget::Splint {
            splint_id,
            incarnation,
        } => format!("Splint: {splint_id:?} · incarnation {incarnation}"),
        // The name is user data any agent with Manage can change, so the
        // short ID is always shown with it.
        ConsentTarget::Lair { lair_id, lair_name } => {
            let short_id: String = lair_id.to_string().chars().take(8).collect();
            format!(
                "Workspace: '{}' · {short_id}",
                trusted_label(lair_name, MAX_CONSENT_PATH_CHARS)
            )
        }
    }
}

/// The workspace name as grant labels and duration rows show it.
fn workspace_label(target: &ConsentTarget) -> String {
    let ConsentTarget::Lair { lair_name, .. } = target else {
        return String::new();
    };
    let mut label = trusted_label(lair_name, WORKSPACE_LABEL_CHARS);
    if lair_name.chars().count() > WORKSPACE_LABEL_CHARS {
        label.push('…');
    }
    label
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

/// Wraps the scope labels into lines of at most [`SCOPE_LINE_CHARS`].
fn scope_lines(prompt: &ConsentPrompt) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = "Scopes: ".to_owned();
    for (index, scope) in prompt.scopes.iter().enumerate() {
        let item = if index + 1 == prompt.scopes.len() {
            scope.label().to_owned()
        } else {
            format!("{},", scope.label())
        };
        if line.chars().count() + item.chars().count() + 1 > SCOPE_LINE_CHARS
            && line.trim() != "Scopes:"
        {
            lines.push(std::mem::replace(&mut line, "        ".to_owned()));
        } else if !line.ends_with(' ') {
            line.push(' ');
        }
        line.push_str(&item);
    }
    lines.push(line);
    lines
}

/// One rendering of the access prompt's text.
struct ConsentLayout {
    lines: Vec<String>,
    /// The line of each shown duration, in order.
    duration_rows: Vec<usize>,
}

/// The prompt's text with the duration at `selected` chosen. Every
/// selection has the same lines in the same places, so choosing a duration
/// never moves the rows the pointer selects.
fn consent_layout(prompt: &ConsentPrompt, selected: usize) -> ConsentLayout {
    let workspace = workspace_label(&prompt.target);
    let access = access_label(prompt.level, &prompt.target);
    let mut lines = vec!["TRUSTED SPLINTERM ACCESS REQUEST".to_owned(), String::new()];
    if let Some(name) = &prompt.agent_name {
        lines.push(format!(
            "Agent: {} (as the agent reports it; not verified)",
            trusted_label(name, MAX_AGENT_NAME_CHARS)
        ));
    }
    lines.extend([
        format!(
            "Program: {}",
            trusted_label(&prompt.requester, MAX_CONSENT_PATH_CHARS)
        ),
        format!(
            "Process: PID {} · UID {}",
            prompt.requester_pid, prompt.requester_uid
        ),
        consent_target_line(&prompt.target),
        String::new(),
    ]);
    lines.push(match prompt.level {
        Some(level) => format!("Access: {} ({})", level.label(), level.summary()),
        None if matches!(prompt.target, ConsentTarget::Lair { .. }) => {
            "Access: Custom, exactly these scopes and nothing more".to_owned()
        }
        None => "Access: exactly these scopes, once".to_owned(),
    });
    lines.extend(scope_lines(prompt));
    lines.push(String::new());
    lines.push("How long (choose with the arrow keys or a click):".to_owned());
    let mut duration_rows = Vec::with_capacity(prompt.durations.len());
    for (index, offer) in prompt.durations.iter().enumerate() {
        duration_rows.push(lines.len());
        let marker = match (offer.withheld, index == selected) {
            (Some(_), _) => "(-)",
            (None, true) => "(•)",
            (None, false) => "( )",
        };
        lines.push(format!(
            "  {marker} {}",
            duration_row(offer.duration, &workspace)
        ));
        if let Some(withheld) = offer.withheld {
            lines.push(format!("      Unavailable: {}", withheld.reason()));
        }
    }
    lines.push(String::new());
    // Reserved for every selection, blank when they do not apply.
    let chosen = prompt.durations.get(selected);
    lines.push(
        chosen
            .and_then(|offer| offer.shared_by.as_deref())
            .map(|path| shared_identity_line(&trusted_label(path, MAX_CONSENT_PATH_CHARS)))
            .unwrap_or_default(),
    );
    lines.push(
        chosen
            .and_then(|offer| {
                offer
                    .replaces
                    .as_ref()
                    .map(|replaced| replacement_line(replaced, offer.duration, &workspace))
            })
            .unwrap_or_default(),
    );
    lines.push(String::new());
    let label = grant_label(
        access,
        chosen.map_or(GrantDuration::Once, |offer| offer.duration),
        &workspace,
    );
    lines.extend([
        format!("Enter / Escape / D: DENY        G: {label}"),
        format!("Red area (left): DENY        Green area (right): {label}"),
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
fn consent_renderer(prompt: ConsentPrompt) -> splinterm::AccessConsentRenderer {
    let layouts = || (0..prompt.durations.len()).map(|selected| consent_layout(&prompt, selected));
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
        let mut layout = consent_layout(&prompt, selected);
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

pub(in crate::app) fn run_consent_client() -> Result<()> {
    let prompt: ConsentPrompt = read_private_frame(&mut io::stdin().lock())?;
    if let Err(reason) = prompt.validate() {
        bail!("invalid trusted consent prompt: {reason}");
    }
    let capability = prompt.capability.clone();
    let durations = prompt
        .durations
        .iter()
        .map(|offer| (offer.duration, offer.withheld.is_none()))
        .collect();
    let (decision, receiver) = std_mpsc::channel();
    let (trusted_consent, snapshot) =
        TrustedConsentUi::access_consent(decision, durations, consent_renderer(prompt));
    let columns = u16::try_from(snapshot.columns).unwrap_or(u16::MAX);
    let rows = u16::try_from(snapshot.rows).unwrap_or(u16::MAX);
    run_window(WindowOptions {
        snapshot: Some(snapshot),
        trusted_consent: Some(trusted_consent),
        initial_columns: columns,
        initial_rows: rows,
        ..WindowOptions::default()
    })?;
    // Closing the Window, or any way out without an answer, denies.
    let decision = receiver.try_recv().unwrap_or(ConsentDecision::Deny);
    write_private_frame(
        &mut io::stdout().lock(),
        &ConsentReply {
            capability,
            decision,
        },
    )
}

#[cfg(test)]
mod tests {
    use splinterm_protocol::{
        AccessLevel, AccessScope, ConsentDurationOffer, ConsentPrompt, ConsentReplacement,
        ConsentTarget, DurationWithheld, GrantDuration,
    };

    use super::{
        consent_layout, consent_renderer, grant_label, read_private_frame_bounded, trusted_label,
        write_private_frame_bounded,
    };

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
        let text = consent_layout(&prompt, 0).lines.join("\n");
        assert!(text.contains("Agent: Claude Code (as the agent reports it; not verified)"));
        assert!(text.contains("Program: /usr/bin/splinterm-mcp"));
        let short_id: String = lair_id.to_string().chars().take(8).collect();
        assert!(text.contains(&format!("Workspace: 'Workspace' · {short_id}")));
        assert!(text.contains(
            "Access: Manage (Read terminals, type commands, and create or close sessions)"
        ));
        assert!(text.contains("start processes in this Lair"));
        // Just this once is preselected and needs no shared-identity line.
        assert!(text.contains("(•) Just this once"));
        assert!(text.contains("G: Allow Manage once"));
        assert!(!text.contains("shares this access"));
    }

    #[test]
    fn a_longer_choice_shows_who_shares_it_and_what_it_replaces() {
        let mut prompt = manage_prompt();
        prompt.durations[2].replaces = Some(ConsentReplacement {
            level: Some(AccessLevel::Look),
            scopes: AccessLevel::Look.scopes().to_vec(),
        });
        let text = consent_layout(&prompt, 2).lines.join("\n");
        assert!(text.contains("(•) Always for 'Workspace'"));
        assert!(text.contains("G: Always allow Manage for 'Workspace'"));
        assert!(text.contains("Any program that runs /usr/bin/splinterm-mcp shares this access."));
        assert!(
            text.contains("This replaces the earlier answer: Always allow Look for 'Workspace'.")
        );
        let session = consent_layout(&prompt, 1).lines.join("\n");
        assert!(session.contains("G: Allow Manage while 'Workspace' is open"));
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
        let layout = consent_layout(&prompt, 0);
        let text = layout.lines.join("\n");
        assert!(text.contains("Access: Custom, exactly these scopes and nothing more"));
        assert!(text.contains("observe visible terminal, take over terminal control"));
        assert!(text.contains("(-) While 'Workspace' is open"));
        assert_eq!(
            text.matches(DurationWithheld::CustomTakeover.reason())
                .count(),
            2
        );
        assert!(text.contains("G: Allow Custom access once"));
        // Each duration row is where the layout says it is.
        for (row, duration) in
            layout
                .duration_rows
                .iter()
                .zip(["Just this once", "While", "Always"])
        {
            assert!(
                layout.lines[*row].contains(duration),
                "{}",
                layout.lines[*row]
            );
        }
    }

    #[test]
    fn every_selection_renders_at_one_size_with_the_same_rows() {
        let prompt = manage_prompt();
        let render = consent_renderer(prompt.clone());
        let views: Vec<_> = (0..prompt.durations.len()).map(&render).collect();
        for view in &views {
            assert_eq!(view.snapshot.columns, views[0].snapshot.columns);
            assert_eq!(view.snapshot.rows, views[0].snapshot.rows);
            assert_eq!(view.snapshot.splint_id, views[0].snapshot.splint_id);
            assert_eq!(view.duration_rows, views[0].duration_rows);
        }
        // The text stays above the action areas in the bottom 22%.
        let last_text_row = consent_layout(&prompt, 2).lines.len();
        assert!(last_text_row * 100 <= views[0].snapshot.rows * 72);
    }

    #[test]
    fn workspace_names_are_bounded_in_labels_and_sanitized() {
        let mut prompt = manage_prompt();
        prompt.target = ConsentTarget::Lair {
            lair_id: splinterm_core::LairId::new(),
            lair_name: format!("{}\u{202e}", "w".repeat(60)),
        };
        let text = consent_layout(&prompt, 2).lines.join("\n");
        assert!(text.contains(&format!("Always allow Manage for '{}…'", "w".repeat(40))));
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
