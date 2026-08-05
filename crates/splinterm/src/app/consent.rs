use std::{
    io::{self, Read, Write},
    sync::mpsc as std_mpsc,
};

use anyhow::{Context, Result, bail};
use splinterm::{TrustedConsentUi, WindowOptions, run_window};
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, ConsentPrompt, ConsentReply,
    MAX_CONSENT_FRAME_BYTES, TerminalCell, TerminalInputModes, TerminalRow, TerminalSnapshot,
    UnderlineStyle,
};

fn read_private_frame<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONSENT_FRAME_BYTES {
        bail!("invalid private consent frame length");
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).context("invalid private consent frame")
}

fn write_private_frame<T: serde::Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode private consent frame")?;
    if body.is_empty() || body.len() > MAX_CONSENT_FRAME_BYTES {
        bail!("private consent frame exceeds limit");
    }
    writer.write_all(&u32::try_from(body.len())?.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
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

fn consent_snapshot(prompt: &ConsentPrompt) -> TerminalSnapshot {
    let mut lines = vec![
        "TRUSTED SPLINTERM ACCESS REQUEST".to_owned(),
        String::new(),
        format!("Requester: {}", prompt.requester),
        format!(
            "Process: PID {} · UID {}",
            prompt.requester_pid, prompt.requester_uid
        ),
        format!(
            "Splint: {:?} · incarnation {}",
            prompt.splint_id, prompt.incarnation
        ),
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
    let columns = lines
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(1)
        .clamp(64, 120);
    let rows = lines.len().max(18);
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
        splint_id: prompt.splint_id,
        incarnation: prompt.incarnation,
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
        title: "Trusted access request".to_owned(),
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

pub(crate) fn run_consent_client() -> Result<()> {
    let prompt: ConsentPrompt = read_private_frame(&mut io::stdin().lock())?;
    if prompt.capability.len() != splinterm_protocol::CONSENT_CAPABILITY_BYTES
        || prompt.scopes.is_empty()
        || prompt.scopes.len() > splinterm_protocol::MAX_ACCESS_SCOPES
        || prompt.requester.chars().count() > 1024
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
