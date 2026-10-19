use std::{
    io::{self, Read, Write},
    sync::mpsc as std_mpsc,
};

use anyhow::{Context, Result, bail};
use splinterm::{TrustedConsentUi, WindowOptions, run_window};
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, ConsentPrompt, ConsentReply, ConsentTarget,
    MAX_CONSENT_FRAME_BYTES, TerminalCell, TerminalInputModes, TerminalRow, TerminalSnapshot,
    UnderlineStyle,
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

fn consent_target_line(target: &ConsentTarget) -> String {
    match target {
        ConsentTarget::Splint {
            splint_id,
            incarnation,
        } => format!("Splint: {splint_id:?} · incarnation {incarnation}"),
        ConsentTarget::Lair { lair_id, lair_name } => {
            format!("Lair: {} · {lair_id:?}", trusted_label(lair_name, 1024))
        }
    }
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

fn consent_snapshot(prompt: &ConsentPrompt) -> TerminalSnapshot {
    let (snapshot_splint_id, snapshot_incarnation) = match &prompt.target {
        ConsentTarget::Splint {
            splint_id,
            incarnation,
        } => (*splint_id, *incarnation),
        ConsentTarget::Lair { .. } => (splinterm_core::SplintId::new(), 0),
    };
    let mut lines = vec![
        "TRUSTED SPLINTERM ACCESS REQUEST".to_owned(),
        String::new(),
        format!("Requester: {}", trusted_label(&prompt.requester, 1024)),
        format!(
            "Process: PID {} · UID {}",
            prompt.requester_pid, prompt.requester_uid
        ),
        consent_target_line(&prompt.target),
        String::new(),
        "Requested one-time capabilities:".to_owned(),
    ];
    lines.extend(
        prompt
            .scopes
            .iter()
            .map(|scope| format!("  • {}", scope.label())),
    );
    lines.extend([
        String::new(),
        "This grant expires automatically and is not persisted.".to_owned(),
        "D / Escape: DENY          G / Enter: GRANT ONCE".to_owned(),
        "Click the red left or green right action area below.".to_owned(),
    ]);
    static_dialog_snapshot(
        lines,
        18,
        "Trusted access request",
        snapshot_splint_id,
        snapshot_incarnation,
    )
}

pub(in crate::app) fn run_consent_client() -> Result<()> {
    let prompt: ConsentPrompt = read_private_frame(&mut io::stdin().lock())?;
    if prompt.capability.len() != splinterm_protocol::CONSENT_CAPABILITY_BYTES
        || prompt.scopes.is_empty()
        || prompt.scopes.len() > splinterm_protocol::MAX_ACCESS_SCOPES
        || prompt.requester.chars().count() > 1024
        || matches!(
            &prompt.target,
            ConsentTarget::Lair { lair_name, .. } if lair_name.chars().count() > 1024
        )
    {
        bail!("invalid trusted consent prompt");
    }
    let (decision, receiver) = std_mpsc::channel();
    run_window(WindowOptions {
        snapshot: Some(consent_snapshot(&prompt)),
        trusted_consent: Some(TrustedConsentUi { decision }),
        ..WindowOptions::default()
    })?;
    let granted = receiver.try_recv().unwrap_or(false);
    write_private_frame(
        &mut io::stdout().lock(),
        &ConsentReply {
            capability: prompt.capability,
            granted,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{read_private_frame_bounded, trusted_label, write_private_frame_bounded};

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
