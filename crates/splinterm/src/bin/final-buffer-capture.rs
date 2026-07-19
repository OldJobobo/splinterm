use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;
use splinterm::{
    config::CursorStyle,
    geometry::{FontSize, FontSizingPolicy, TerminalPadding, resolve_font_size},
    renderer::{
        CursorPresentation, RendererOptions, UnfocusedCursorStyle, capture_final_buffer,
        capture_final_buffer_presented, capture_final_buffer_sized,
        capture_final_buffer_sized_presented, configure, effective_cursor_shape,
    },
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
    #[arg(long, default_value = "pixels")]
    font_size_unit: String,
    #[arg(long, default_value = "output-scale")]
    font_sizing_policy: String,
    #[arg(long, default_value_t = 96.0)]
    physical_dpi: f32,
    #[arg(long, default_value_t = 12)]
    pad_left: u32,
    #[arg(long, default_value_t = 12)]
    pad_right: u32,
    #[arg(long, default_value_t = 12)]
    pad_top: u32,
    #[arg(long, default_value_t = 12)]
    pad_bottom: u32,
    #[arg(long, default_value_t = 120)]
    scale_120: u32,
    #[arg(long, default_value_t = 95)]
    columns: usize,
    #[arg(long, default_value_t = 1)]
    rows: usize,
    #[arg(long, requires = "logical_height")]
    logical_width: Option<u32>,
    #[arg(long, requires = "logical_width")]
    logical_height: Option<u32>,
    #[arg(long)]
    text_hex: Option<String>,
    #[arg(long, conflicts_with = "text_hex")]
    cells_json: Option<PathBuf>,
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
    #[arg(long, default_value = "v1")]
    capture_schema: String,
    #[arg(long, default_value = "focused-steady")]
    target_focus_semantics: String,
    #[arg(long, default_value = "unchanged")]
    unfocused_style: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredGrid {
    rows: Vec<Vec<TerminalCell>>,
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

fn structured_rows(arguments: &Arguments) -> Result<Option<Vec<TerminalRow>>> {
    let Some(path) = &arguments.cells_json else {
        return Ok(None);
    };
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() > 1024 * 1024 {
        bail!("structured cell grid exceeds 1 MiB");
    }
    let grid: StructuredGrid =
        serde_json::from_slice(&bytes).context("parse structured cell grid")?;
    if grid.rows.len() != arguments.rows
        || grid.rows.iter().any(|row| row.len() != arguments.columns)
    {
        bail!("structured cell grid dimensions do not match --columns/--rows");
    }
    for row in &grid.rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(remaining) = cell.spacer_remaining {
                let valid_leader = index > 0
                    && row[index - 1].spacer_remaining.is_none()
                    && !row[index - 1].content.is_empty();
                if remaining != 1 || !cell.content.is_empty() || !valid_leader {
                    bail!(
                        "structured wide spacer must be an empty width-2 continuation after a leader"
                    );
                }
            }
        }
    }
    Ok(Some(
        grid.rows
            .into_iter()
            .map(|cells| TerminalRow {
                row_id: None,
                linebreak: true,
                cells,
            })
            .collect(),
    ))
}

fn cursor_presentation(arguments: &Arguments) -> Result<CursorPresentation> {
    let unfocused_style = match arguments.unfocused_style.as_str() {
        "unchanged" => UnfocusedCursorStyle::Unchanged,
        "hollow" => UnfocusedCursorStyle::Hollow,
        "none" => UnfocusedCursorStyle::None,
        _ => bail!("unfocused-style must be unchanged, hollow, or none"),
    };
    match arguments.target_focus_semantics.as_str() {
        "focused-steady" if unfocused_style == UnfocusedCursorStyle::Unchanged => {
            Ok(CursorPresentation::FOCUSED_STEADY)
        }
        "unfocused" => Ok(CursorPresentation {
            keyboard_focused: false,
            unfocused_style,
        }),
        "focused-steady" => bail!("focused-steady requires unfocused-style=unchanged"),
        _ => bail!("target-focus-semantics must be focused-steady or unfocused"),
    }
}

fn snapshot(arguments: &Arguments) -> Result<TerminalSnapshot> {
    if arguments.columns == 0 || arguments.rows == 0 {
        bail!("grid dimensions must be positive");
    }
    if arguments.cursor_column >= arguments.columns || arguments.cursor_row >= arguments.rows {
        bail!("cursor must be inside the declared grid");
    }
    let visible_rows = if let Some(rows) = structured_rows(arguments)? {
        rows
    } else {
        let text = arguments.text_hex.as_deref().map_or_else(
            || fixture_text(&arguments.fixture, arguments.columns, arguments.rows),
            decode_hex,
        )?;
        if text.chars().count() > arguments.columns.saturating_mul(arguments.rows) {
            bail!("fixture text exceeds the declared grid");
        }
        let attributes = attributes(&arguments.style)?;
        let mut characters = text.chars();
        (0..arguments.rows)
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
            .collect()
    };
    let mut palette = vec![0; 256];
    palette[..16].copy_from_slice(&[
        0x0000_0000,
        0x0080_0000,
        0x0000_8000,
        0x0080_8000,
        0x0000_0080,
        0x0080_0080,
        0x0000_8080,
        0x00c0_c0c0,
        0x0080_8080,
        0x00ff_0000,
        0x0000_ff00,
        0x00ff_ff00,
        0x0000_00ff,
        0x00ff_00ff,
        0x0000_ffff,
        0x00ff_ffff,
    ]);
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
        palette,
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

#[allow(
    clippy::too_many_lines,
    reason = "capture configuration and the emitted provenance contract remain one auditable transaction"
)]
fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let font_size = match arguments.font_size_unit.as_str() {
        "pixels" | "px" => FontSize::Pixels(arguments.font_size),
        "points" | "pt" => FontSize::Points(arguments.font_size),
        _ => bail!("font-size-unit must be pixels or points"),
    };
    let font_sizing_policy = match arguments.font_sizing_policy.as_str() {
        "output-scale" => FontSizingPolicy::OutputScale,
        "physical-dpi" => FontSizingPolicy::PhysicalDpi,
        _ => bail!("font-sizing-policy must be output-scale or physical-dpi"),
    };
    let padding = TerminalPadding {
        left: arguments.pad_left,
        right: arguments.pad_right,
        top: arguments.pad_top,
        bottom: arguments.pad_bottom,
    };
    let resolved_font = resolve_font_size(
        font_size,
        font_sizing_policy,
        arguments.scale_120,
        arguments.physical_dpi,
    )?;
    configure(RendererOptions {
        font: arguments.font.clone(),
        font_size,
        font_sizing_policy,
        physical_dpi: arguments.physical_dpi,
        padding,
    })?;
    let style = cursor_style(&arguments.cursor_shape)?;
    let presentation = cursor_presentation(&arguments)?;
    let snapshot = snapshot(&arguments)?;
    let is_v2 = match arguments.capture_schema.as_str() {
        "v1" => false,
        "slice3-v2" => true,
        _ => bail!("capture-schema must be v1 or slice3-v2"),
    };
    if !is_v2 && presentation != CursorPresentation::FOCUSED_STEADY {
        bail!("v1 captures support only focused-steady cursor presentation");
    }
    let capture = match (arguments.logical_width, arguments.logical_height, is_v2) {
        (Some(width), Some(height), true) => capture_final_buffer_sized_presented(
            &snapshot,
            arguments.scale_120,
            width,
            height,
            !arguments.hide_cursor,
            style,
            presentation,
        )?,
        (Some(width), Some(height), false) => capture_final_buffer_sized(
            &snapshot,
            arguments.scale_120,
            width,
            height,
            !arguments.hide_cursor,
            style,
        )?,
        (None, None, true) => capture_final_buffer_presented(
            &snapshot,
            arguments.scale_120,
            !arguments.hide_cursor,
            style,
            presentation,
        )?,
        (None, None, false) => capture_final_buffer(
            &snapshot,
            arguments.scale_120,
            !arguments.hide_cursor,
            style,
        )?,
        _ => unreachable!("clap requires both logical surface dimensions"),
    };
    let frame_id = arguments.frame_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}x{}-{}",
            arguments.fixture, arguments.columns, arguments.rows, arguments.scale_120
        )
    });
    let effective = effective_cursor_shape(style, !arguments.hide_cursor, presentation);
    let cursor = if is_v2 {
        let position = (!arguments.hide_cursor).then(
            || serde_json::json!({"column": arguments.cursor_column, "row": arguments.cursor_row}),
        );
        serde_json::json!({
            "position": position,
            "configured_shape": arguments.cursor_shape,
            "effective_shape": effective.name(),
            "target_focus_semantics": arguments.target_focus_semantics,
        })
    } else {
        capture
            .cursor
            .map_or(serde_json::Value::Null, |(column, row)| {
                serde_json::json!({
                    "column": column,
                    "row": row,
                    "shape": arguments.cursor_shape,
                })
            })
    };
    let schema = if is_v2 {
        "splinterm.final-buffer.slice3.v2"
    } else {
        "splinterm.final-buffer.v1"
    };
    let composition = if is_v2 {
        serde_json::json!("foot-cell-rtl-v1")
    } else {
        serde_json::json!(["terminal-backgrounds", "glyphs", "decorations", "cursor"])
    };
    let capture_context = is_v2.then(|| {
        serde_json::json!({
            "actual_keyboard_focus": false,
            "unfocused_style": arguments.unfocused_style,
        })
    });
    let mut metadata = serde_json::json!({
        "schema": schema,
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
        "composition": composition,
        "provenance": {
            "implementation": "splinterm",
            "font_pattern": arguments.font,
            "font_size": {"value": arguments.font_size, "unit": font_size.unit_name()},
            "font_sizing_policy": font_sizing_policy.name(),
            "font_resolution": {
                "configured_size": {"value": arguments.font_size, "unit": font_size.unit_name()},
                "policy": font_sizing_policy.name(),
                "observed_output_dpi": resolved_font.observed_output_dpi,
                "observed_output_id": resolved_font.observed_output_id,
                "observed_output_name": resolved_font.observed_output_name,
                "observed_dpi_source": resolved_font.observed_dpi_source,
                "observed_dpi_fallback_reason": resolved_font.observed_dpi_fallback_reason,
                "sizing_dpi": resolved_font.sizing_dpi,
                "sizing_dpi_source": resolved_font.dpi_source,
                "surface_scale_120": resolved_font.surface_scale_120,
                "effective_pixel_size_26_6": resolved_font.effective_pixel_size_26_6,
                "effective_pixel_size": resolved_font.pixel_size,
            },
            "window_geometry": {
                "surface_logical_size": {"width": capture.logical_width, "height": capture.logical_height},
                "surface_buffer_size": {"width": capture.width, "height": capture.height},
                "surface_scale_120": arguments.scale_120,
                "grid": {"columns": capture.columns, "rows": capture.rows},
                "cell": {"width": capture.cell_width, "height": capture.cell_height, "ascent": capture.ascent, "descent": capture.descent, "baseline_from_top": capture.baseline, "advance_policy": "integer-primary-advance"},
                "grid_rect": {"x": capture.grid_rect.x, "y": capture.grid_rect.y, "width": capture.grid_rect.width, "height": capture.grid_rect.height},
                "visible_grid_rect": {"x": capture.visible_grid_rect.x, "y": capture.visible_grid_rect.y, "width": capture.visible_grid_rect.width, "height": capture.visible_grid_rect.height},
                "requested_padding": {"left": capture.requested_padding.left, "right": capture.requested_padding.right, "top": capture.requested_padding.top, "bottom": capture.requested_padding.bottom},
                "effective_base_padding": {"left": capture.effective_base_padding.left, "right": capture.effective_base_padding.right, "top": capture.effective_base_padding.top, "bottom": capture.effective_base_padding.bottom},
                "actual_padding": {"left": capture.padding_left, "right": capture.padding_right, "top": capture.padding_top, "bottom": capture.padding_bottom},
                "residual_right": capture.residual_right,
                "residual_bottom": capture.residual_bottom,
                "residual_policy": "trailing-edges",
            },
            "cargo_lock": "Cargo.lock",
        },
    });

    if let Some(capture_context) = capture_context {
        metadata
            .as_object_mut()
            .expect("capture metadata is an object")
            .insert("capture_context".into(), capture_context);
    }

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
