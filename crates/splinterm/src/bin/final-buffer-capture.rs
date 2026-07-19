use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use splinterm::{
    config::CursorStyle,
    renderer::{RendererOptions, capture_final_buffer, configure},
};
use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, MouseTracking, TerminalCell, TerminalInputModes,
    TerminalRow, TerminalSnapshot, UnderlineStyle,
};

#[derive(Debug, Parser)]
#[command(about = "Export the production Splinterm final terminal buffer")]
struct Arguments {
    #[arg(long)]
    output_prefix: PathBuf,
    #[arg(long, default_value = "ascii")]
    fixture: String,
    #[arg(long, default_value = "JetBrains Mono Nerd Font:style=Regular")]
    font: String,
    #[arg(long, default_value_t = 12.0)]
    font_size: f32,
    #[arg(long, default_value_t = 120)]
    scale_120: u32,
    #[arg(long, default_value_t = 95)]
    columns: usize,
    #[arg(long, default_value_t = 1)]
    rows: usize,
    #[arg(long)]
    text_hex: Option<String>,
    #[arg(long, default_value = "normal")]
    style: String,
    #[arg(long, default_value = "block")]
    cursor_shape: String,
    #[arg(long, default_value_t = 0)]
    cursor_column: usize,
    #[arg(long, default_value_t = 0)]
    cursor_row: usize,
    #[arg(long)]
    hide_cursor: bool,
    #[arg(long)]
    frame_id: Option<String>,
}

fn attributes(style: &str) -> Result<CellAttributes> {
    let mut attributes = CellAttributes {
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
    match style {
        "normal" => {}
        "reverse" => attributes.reverse = true,
        "dim" => attributes.dim = true,
        "conceal" => attributes.conceal = true,
        other => bail!("unsupported fixture style {other:?}"),
    }
    Ok(attributes)
}

fn decode_hex(value: &str) -> Result<String> {
    if value.len() % 2 != 0 || value.len() > 2 * usize::from(splinterm_protocol::MAX_COLUMNS) * 4096
    {
        bail!("fixture text hex is malformed or exceeds bounds");
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).context("fixture hex is ASCII")?;
            u8::from_str_radix(text, 16).context("fixture text contains non-hex bytes")
        })
        .collect::<Result<Vec<_>>>()?;
    String::from_utf8(bytes).context("fixture text is not UTF-8")
}

fn fixture_text(name: &str, columns: usize, rows: usize) -> Result<String> {
    let text = match name {
        "ascii" => (0x20_u8..=0x7e).map(char::from).collect(),
        "spacing" => "  !?[]{}()<>  iiii WWWW .... ____ ||||  ".to_owned(),
        "drift" => (0..columns.saturating_mul(rows))
            .map(|index| char::from(b'!' + u8::try_from(index % 94).unwrap()))
            .collect(),
        other => bail!("unsupported fixture {other:?}"),
    };
    Ok(text)
}

fn snapshot(arguments: &Arguments) -> Result<TerminalSnapshot> {
    if arguments.columns == 0 || arguments.rows == 0 {
        bail!("grid dimensions must be positive");
    }
    if arguments.cursor_column >= arguments.columns || arguments.cursor_row >= arguments.rows {
        bail!("cursor must be inside the declared grid");
    }
    let text = arguments.text_hex.as_deref().map_or_else(
        || fixture_text(&arguments.fixture, arguments.columns, arguments.rows),
        decode_hex,
    )?;
    if text.chars().count() > arguments.columns.saturating_mul(arguments.rows) {
        bail!("fixture text exceeds the declared grid");
    }
    let attributes = attributes(&arguments.style)?;
    let mut characters = text.chars();
    let visible_rows = (0..arguments.rows)
        .map(|_| TerminalRow {
            row_id: None,
            linebreak: true,
            cells: (0..arguments.columns)
                .map(|_| TerminalCell {
                    content: characters.next().unwrap_or(' ').to_string(),
                    spacer_remaining: None,
                    attributes,
                })
                .collect(),
        })
        .collect();
    Ok(TerminalSnapshot {
        splint_id: SplintId::new(),
        incarnation: 1,
        revision: 1,
        columns: arguments.columns,
        rows: arguments.rows,
        cursor_column: i32::try_from(arguments.cursor_column).context("cursor column")?,
        cursor_row: i32::try_from(arguments.cursor_row).context("cursor row")?,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: TerminalInputModes {
            application_cursor: false,
            application_keypad: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor_visible: !arguments.hide_cursor,
            cursor_blink: false,
            mouse_tracking: MouseTracking::None,
            sgr_mouse: false,
        },
        palette: vec![0; 256],
        default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
        title: arguments.fixture.clone(),
        visible_rows,
        history_generation: 1,
        oldest_available_scrollback_row_id: None,
        newest_available_scrollback_row_id: None,
        scrollback_rows: Vec::new(),
        available_scrollback_rows: 0,
        omitted_oldest_scrollback_rows: 0,
        exited_code: None,
        exited_signal: None,
    })
}

fn cursor_style(value: &str) -> Result<CursorStyle> {
    match value {
        "block" => Ok(CursorStyle::Block),
        "beam" => Ok(CursorStyle::Beam),
        "underline" => Ok(CursorStyle::Underline),
        _ => bail!("cursor shape must be block, beam, or underline"),
    }
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    configure(RendererOptions {
        font: arguments.font.clone(),
        font_size: arguments.font_size,
    })?;
    let style = cursor_style(&arguments.cursor_shape)?;
    let capture = capture_final_buffer(
        &snapshot(&arguments)?,
        arguments.scale_120,
        !arguments.hide_cursor,
        style,
    )?;
    let frame_id = arguments.frame_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}x{}-{}",
            arguments.fixture, arguments.columns, arguments.rows, arguments.scale_120
        )
    });
    let cursor = capture.cursor.map(|(column, row)| {
        serde_json::json!({
            "column": column,
            "row": row,
            "shape": arguments.cursor_shape,
        })
    });
    let metadata = serde_json::json!({
        "schema": "splinterm.final-buffer.v1",
        "width": capture.width,
        "height": capture.height,
        "stride": capture.stride,
        "format": "argb8888",
        "byte_order": "bgra",
        "endianness": "little",
        "scale_120": arguments.scale_120,
        "grid": {"columns": capture.columns, "rows": capture.rows},
        "cell": {
            "width": capture.cell_width,
            "height": capture.cell_height,
            "baseline": capture.baseline,
        },
        "padding": {
            "left": capture.padding_left,
            "right": capture.padding_right,
            "top": capture.padding_top,
            "bottom": capture.padding_bottom,
        },
        "origin": {"x": capture.origin_x, "y": capture.origin_y},
        "cursor": cursor,
        "fixture": arguments.fixture,
        "frame_id": frame_id,
        "background_bgra": capture.background_bgra,
        "composition": ["terminal-backgrounds", "glyphs", "decorations", "cursor"],
        "provenance": {
            "implementation": "splinterm",
            "font_pattern": arguments.font,
            "logical_font_size_px": arguments.font_size,
            "cargo_lock": "Cargo.lock",
        },
    });

    let raw_path = arguments.output_prefix.with_extension("argb");
    let metadata_path = arguments.output_prefix.with_extension("json");
    let raw_temporary = raw_path.with_extension("argb.tmp");
    let metadata_temporary = metadata_path.with_extension("json.tmp");
    if let Some(parent) = raw_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&raw_temporary, &capture.pixels)
        .with_context(|| format!("write {}", raw_temporary.display()))?;
    fs::write(
        &metadata_temporary,
        serde_json::to_vec_pretty(&metadata).context("serialize capture metadata")?,
    )
    .with_context(|| format!("write {}", metadata_temporary.display()))?;
    fs::rename(&raw_temporary, &raw_path)
        .with_context(|| format!("publish {}", raw_path.display()))?;
    fs::rename(&metadata_temporary, &metadata_path)
        .with_context(|| format!("publish {}", metadata_path.display()))?;
    println!("{}\n{}", metadata_path.display(), raw_path.display());
    Ok(())
}
