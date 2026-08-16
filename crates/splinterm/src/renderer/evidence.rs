//! Deterministic renderer benchmarks, metrics, and PPM evidence output.

use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, TerminalCell, TerminalInputModes, TerminalRow,
    TerminalSnapshot, UnderlineStyle,
};

use crate::{box_drawing, config::CursorStyle};

use super::{
    SnapshotFrame, TextRow, evict_snapshot_glyphs, paint, paint_snapshot, paint_snapshot_rows,
    process_rss_bytes, reset_snapshot_cache, snapshot_cache_metrics,
};

/// Runs the deterministic renderer evidence benchmark and returns a JSON report.
///
/// The caller should use a release build. Timings separate complete setup (fontconfig,
/// file loading, shaping, and cold raster), warm cache lookup, blend, and generated
/// box-mask work. They are evidence, not latency assertions.
///
/// # Errors
///
/// Returns an error if the font stack or deterministic row cannot be initialized.
pub fn benchmark_json(samples: usize) -> Result<serde_json::Value> {
    if samples == 0 {
        bail!("benchmark sample count must be positive");
    }
    let setup_started = Instant::now();
    let row = TextRow::load(1)?;
    let setup_ns =
        u64::try_from(setup_started.elapsed().as_nanos()).context("setup duration fits u64")?;
    let mut canvas = vec![0_u8; 960 * 600 * 4];
    let mut lookup = Vec::with_capacity(samples);
    let mut blend = Vec::with_capacity(samples);
    let mut boxes = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        for placed in &row.glyphs {
            std::hint::black_box(&row.cache[&placed.key]);
        }
        lookup.push(u64::try_from(started.elapsed().as_nanos()).context("lookup duration")?);

        let started = Instant::now();
        paint(&mut canvas, 960, 600, &row);
        std::hint::black_box(&canvas);
        blend.push(u64::try_from(started.elapsed().as_nanos()).context("blend duration")?);

        let started = Instant::now();
        for character in ['┌', '─', '┼', '┐'] {
            std::hint::black_box(box_drawing::generate(
                character,
                row.cell_width,
                row.cell_height,
                1,
            ));
        }
        boxes.push(u64::try_from(started.elapsed().as_nanos()).context("box duration")?);
    }
    Ok(serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "samples": samples,
        "setup_ns": setup_ns,
        "warm_cache_lookup_ns": timing_summary(&mut lookup),
        "full_canvas_blend_ns": timing_summary(&mut blend),
        "box_generation_ns": timing_summary(&mut boxes),
        "cell": { "width": row.cell_width, "height": row.cell_height, "baseline": row.baseline },
        "canvas": { "width": 960, "height": 600 }
    }))
}

/// Benchmarks the Phase 4 full-grid and one-row damage paths.
///
/// # Errors
///
/// Returns an error for zero samples or when snapshot rendering cannot initialize.
#[allow(
    clippy::too_many_lines,
    reason = "the benchmark keeps all measured scenarios adjacent for comparable output"
)]
pub fn phase4_benchmark_json(samples: usize) -> Result<serde_json::Value> {
    if samples == 0 {
        bail!("benchmark sample count must be positive");
    }
    let mut grids = Vec::new();
    let benchmark_rss_before = process_rss_bytes();
    for (columns, rows) in [(80_usize, 24_usize), (240, 80), (480, 128)] {
        reset_snapshot_cache();
        let cell = TerminalCell {
            content: "x".into(),
            spacer_remaining: None,
            attributes: CellAttributes {
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
            },
        };
        let snapshot = TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision: 1,
            columns,
            rows,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: true,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
            title: "phase4 benchmark".into(),
            visible_rows: (0..rows)
                .map(|row| {
                    Ok(TerminalRow {
                        row_id: Some(u64::try_from(row + 1).context("benchmark row ID fits u64")?),
                        linebreak: false,
                        cells: vec![cell.clone(); columns],
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            history_generation: 1,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            images: None,
            exited_code: None,
            exited_signal: None,
        };
        let mut block_snapshot = snapshot.clone();
        for row in &mut block_snapshot.visible_rows {
            for cell in &mut row.cells {
                cell.content = "█".into();
            }
        }
        let cold_started = Instant::now();
        let mut frame = SnapshotFrame::load(&snapshot, 1)?;
        let cold_ns = u64::try_from(cold_started.elapsed().as_nanos())
            .context("cold frame duration fits u64")?;
        let mut block_frame = SnapshotFrame::load(&block_snapshot, 1)?;
        let geometry = frame.tight_geometry()?;
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        let canvas_len = usize::try_from(
            width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                .context("benchmark canvas overflow")?,
        )
        .context("benchmark canvas fits usize")?;
        let mut canvas = vec![0; canvas_len];
        let mut warm = Vec::with_capacity(samples);
        let mut full = Vec::with_capacity(samples);
        let mut row_prepare = Vec::with_capacity(samples);
        let mut row_damage = Vec::with_capacity(samples);
        let mut all_rows_prepare = Vec::with_capacity(samples);
        let mut all_rows_damage = Vec::with_capacity(samples);
        let mut block_all_rows_prepare = Vec::with_capacity(samples);
        let mut block_all_rows_damage = Vec::with_capacity(samples);
        let mut dirty = vec![false; rows];
        dirty[rows / 2] = true;
        let all_dirty = vec![true; rows];
        for _ in 0..samples {
            let started = Instant::now();
            std::hint::black_box(SnapshotFrame::load(&snapshot, 1)?);
            warm.push(u64::try_from(started.elapsed().as_nanos()).context("warm frame duration")?);

            let started = Instant::now();
            frame.refresh_rows(&snapshot, &dirty)?;
            row_prepare.push(
                u64::try_from(started.elapsed().as_nanos()).context("row preparation duration")?,
            );

            let started = Instant::now();
            frame.refresh_rows(&snapshot, &all_dirty)?;
            all_rows_prepare.push(
                u64::try_from(started.elapsed().as_nanos())
                    .context("all-row preparation duration")?,
            );

            let started = Instant::now();
            block_frame.refresh_rows(&block_snapshot, &all_dirty)?;
            block_all_rows_prepare.push(
                u64::try_from(started.elapsed().as_nanos())
                    .context("block all-row preparation duration")?,
            );

            let started = Instant::now();
            paint_snapshot(
                &mut canvas,
                width,
                height,
                &frame,
                &geometry,
                true,
                CursorStyle::Block,
            );
            std::hint::black_box(&canvas);
            full.push(u64::try_from(started.elapsed().as_nanos()).context("full paint duration")?);

            let started = Instant::now();
            paint_snapshot_rows(
                &mut canvas,
                width,
                height,
                &frame,
                &geometry,
                &dirty,
                true,
                CursorStyle::Block,
            );
            std::hint::black_box(&canvas);
            row_damage
                .push(u64::try_from(started.elapsed().as_nanos()).context("row paint duration")?);

            let started = Instant::now();
            paint_snapshot_rows(
                &mut canvas,
                width,
                height,
                &frame,
                &geometry,
                &all_dirty,
                true,
                CursorStyle::Block,
            );
            std::hint::black_box(&canvas);
            all_rows_damage.push(
                u64::try_from(started.elapsed().as_nanos()).context("all-row paint duration")?,
            );

            let started = Instant::now();
            paint_snapshot_rows(
                &mut canvas,
                width,
                height,
                &block_frame,
                &geometry,
                &all_dirty,
                true,
                CursorStyle::Block,
            );
            std::hint::black_box(&canvas);
            block_all_rows_damage.push(
                u64::try_from(started.elapsed().as_nanos())
                    .context("block all-row paint duration")?,
            );
        }
        let evicted_entries = evict_snapshot_glyphs();
        let repopulate_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 120)?);
        let repopulate_ns = u64::try_from(repopulate_started.elapsed().as_nanos())
            .context("repopulate duration fits u64")?;
        let scale_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 150)?);
        let scale_change_ns = u64::try_from(scale_started.elapsed().as_nanos())
            .context("scale change duration fits u64")?;
        let scale_return_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 120)?);
        let scale_return_ns = u64::try_from(scale_return_started.elapsed().as_nanos())
            .context("scale return duration fits u64")?;
        let mut alternate_theme = snapshot.clone();
        alternate_theme.default_colors = [0x0011_2233, 0x0044_5566, 0x0077_8899];
        let theme_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&alternate_theme, 120)?);
        let theme_change_ns = u64::try_from(theme_started.elapsed().as_nanos())
            .context("theme change duration fits u64")?;
        let theme_return_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 120)?);
        let theme_return_ns = u64::try_from(theme_return_started.elapsed().as_nanos())
            .context("theme return duration fits u64")?;
        grids.push(serde_json::json!({
            "columns": columns,
            "rows": rows,
            "canvas": { "width": width, "height": height, "bytes": canvas_len },
            "cold_frame_ns": cold_ns,
            "warm_full_prepare_ns": timing_summary(&mut warm),
            "one_row_prepare_ns": timing_summary(&mut row_prepare),
            "all_rows_prepare_ns": timing_summary(&mut all_rows_prepare),
            "block_all_rows_prepare_ns": timing_summary(&mut block_all_rows_prepare),
            "full_paint_ns": timing_summary(&mut full),
            "one_row_paint_ns": timing_summary(&mut row_damage),
            "all_rows_paint_ns": timing_summary(&mut all_rows_damage),
            "block_all_rows_paint_ns": timing_summary(&mut block_all_rows_damage),
            "forced_eviction": { "entries": evicted_entries, "repopulate_ns": repopulate_ns },
            "scale_invalidation_ns": { "change": scale_change_ns, "return": scale_return_ns },
            "theme_invalidation_ns": { "change": theme_change_ns, "return": theme_return_ns },
            "rss_bytes_after_grid": process_rss_bytes(),
            "glyph_cache": snapshot_cache_metrics(),
        }));
    }
    Ok(serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "samples": samples,
        "rss_bytes": { "before": benchmark_rss_before, "after": process_rss_bytes() },
        "grids": grids,
        "glyph_cache": snapshot_cache_metrics(),
    }))
}

fn timing_summary(samples: &mut [u64]) -> serde_json::Value {
    samples.sort_unstable();
    let percentile = |numerator: usize| {
        let index = (samples.len() - 1).saturating_mul(numerator) / 100;
        samples[index]
    };
    serde_json::json!({
        "min": samples[0],
        "median": percentile(50),
        "p95": percentile(95),
        "max": samples[samples.len() - 1]
    })
}

/// Writes an opaque ARGB8888 canvas as a lossless binary PPM (P6) capture.
///
/// PPM keeps the evidence path dependency-free. The alpha byte is omitted because
/// the window renderer always produces an opaque canvas.
///
/// # Errors
///
/// Returns an error when dimensions overflow, the canvas length does not match
/// the dimensions, or the capture cannot be created or written.
pub fn write_ppm(path: impl AsRef<Path>, canvas: &[u8], width: u32, height: u32) -> io::Result<()> {
    let expected_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "capture dimensions overflow")
        })?;
    if canvas.len() != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture canvas does not match dimensions",
        ));
    }

    let mut file = fs::File::create(path)?;
    write!(file, "P6\n{width} {height}\n255\n")?;
    for pixel in canvas.chunks_exact(4) {
        file.write_all(&[pixel[2], pixel[1], pixel[0]])?;
    }
    file.flush()
}
