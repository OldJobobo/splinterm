//! The daemon handoff (upgrade) banner (Plan 0061 M3.12a).
//!
//! A non-interactive status card centered at the top of the terminal area,
//! below the tab strip, so it never covers the strip's controls and never
//! changes terminal geometry. It keeps clear of the history overlay's
//! "return to live" control at the top right, and draws its text in trusted
//! chrome type, whose size follows the output scale but not terminal zoom.

use std::collections::HashSet;

use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use crate::geometry::Rect;

use super::super::{ChromeText, ChromeTextStyle, RenderContext, fill_rect};
use super::picker::{
    PickerTextAlignment, SessionPickerPalette, SessionPickerTextCache, paint_picker_text,
};

fn scaled(logical: u32, scale_120: u32) -> u32 {
    logical.saturating_mul(scale_120).div_ceil(120).max(1)
}

fn rect_tuple(rect: Rect) -> (i32, i32, u32, u32) {
    (
        i32::try_from(rect.x).unwrap_or(i32::MAX),
        i32::try_from(rect.y).unwrap_or(i32::MAX),
        rect.width,
        rect.height,
    )
}

/// Where the banner goes in buffer pixels, for text of `text_size` pixels,
/// inside `terminal` and left of `avoid` (the history overlay) when that
/// shares the banner's band. `None` when the terminal is too small for a
/// legible banner.
#[must_use]
pub(crate) fn handoff_banner_layout(
    terminal: Rect,
    avoid: Option<Rect>,
    text_size: (u32, u32),
    scale_120: u32,
) -> Option<Rect> {
    if scale_120 == 0 || terminal.width == 0 || terminal.height == 0 {
        return None;
    }
    let margin = scaled(8, scale_120);
    let padding = scaled(14, scale_120);
    let height = text_size
        .1
        .saturating_add(scaled(12, scale_120))
        .max(scaled(28, scale_120));
    if terminal.height < height.saturating_add(margin.saturating_mul(2)) {
        return None;
    }
    let y = terminal.y.saturating_add(margin);
    let left = terminal.x.saturating_add(margin);
    let mut right = terminal
        .x
        .saturating_add(terminal.width)
        .saturating_sub(margin);
    if let Some(avoid) = avoid {
        let band = y..y.saturating_add(height);
        let shares_band = avoid.y < band.end && avoid.y.saturating_add(avoid.height) > band.start;
        if shares_band && avoid.x > left && avoid.x < right {
            right = avoid.x.saturating_sub(margin).max(left);
        }
    }
    let available = right.saturating_sub(left);
    if available < scaled(96, scale_120) {
        return None;
    }
    let width = text_size
        .0
        .saturating_add(padding.saturating_mul(2))
        .min(available);
    Some(Rect {
        x: left.saturating_add(available.saturating_sub(width) / 2),
        y,
        width,
        height,
    })
}

/// Paints the banner with `text` and returns its panel, or `None` when the
/// terminal is too small for it.
#[allow(
    clippy::too_many_arguments,
    reason = "trusted overlay painting keeps geometry, palette, and cache explicit"
)]
pub(crate) fn paint_handoff_banner(
    cache: &mut SessionPickerTextCache,
    context: &RenderContext,
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    terminal: Rect,
    avoid: Option<Rect>,
    scale_120: u32,
    renderer_generation: u64,
    palette: SessionPickerPalette,
    text: &str,
) -> Result<Option<Rect>> {
    if scale_120 == 0 {
        return Ok(None);
    }
    let cell =
        ChromeText::load_styled_with_context("_", scale_120, ChromeTextStyle::Regular, context)?;
    let cell_width = cell.frame.cell_width.max(1);
    let text_width = u32::try_from(UnicodeWidthStr::width(text))
        .unwrap_or(u32::MAX)
        .saturating_mul(cell_width);
    let Some(panel) = handoff_banner_layout(
        terminal,
        avoid,
        (text_width, cell.pixel_height()),
        scale_120,
    ) else {
        return Ok(None);
    };
    fill_rect(
        canvas,
        canvas_width,
        canvas_height,
        rect_tuple(panel),
        palette.panel_fill,
    );
    let [_, red, green, blue] = palette.focused_frame.to_be_bytes();
    let frame = [red, green, blue, u8::MAX];
    let border = scale_120.div_ceil(120).max(1);
    let right = panel.x.saturating_add(panel.width).saturating_sub(border);
    let bottom = panel.y.saturating_add(panel.height).saturating_sub(border);
    for stripe in [
        Rect {
            height: border,
            ..panel
        },
        Rect {
            y: bottom,
            height: border,
            ..panel
        },
        Rect {
            width: border,
            ..panel
        },
        Rect {
            x: right,
            width: border,
            ..panel
        },
    ] {
        fill_rect(
            canvas,
            canvas_width,
            canvas_height,
            rect_tuple(stripe),
            frame,
        );
    }
    let padding = scaled(14, scale_120).min(panel.width / 4);
    let clip = Rect {
        x: panel.x.saturating_add(padding),
        y: panel.y,
        width: panel.width.saturating_sub(padding.saturating_mul(2)),
        height: panel.height,
    };
    let mut used = HashSet::new();
    paint_picker_text(
        cache,
        context,
        &mut used,
        canvas,
        canvas_width,
        canvas_height,
        text,
        ChromeTextStyle::Regular,
        scale_120,
        renderer_generation,
        clip,
        PickerTextAlignment::Center,
        palette.primary,
    )?;
    cache.finish_frame(used);
    Ok(Some(panel))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERMINAL: Rect = Rect {
        x: 0,
        y: 40,
        width: 800,
        height: 480,
    };

    #[test]
    fn the_banner_is_centered_below_the_strip_inside_the_terminal() {
        let panel = handoff_banner_layout(TERMINAL, None, (240, 18), 120).unwrap();
        assert_eq!(
            panel.y,
            TERMINAL.y + 8,
            "below the tab strip, never over it"
        );
        assert_eq!(panel.width, 240 + 28);
        assert_eq!(panel.height, 30);
        assert_eq!(panel.x, (800 - panel.width) / 2);
    }

    #[test]
    fn the_banner_scales_with_the_output_and_stays_inside_at_fractional_scale() {
        let scale = 150; // 1.25
        let terminal = Rect {
            x: 0,
            y: 50,
            width: 1000,
            height: 600,
        };
        let panel = handoff_banner_layout(terminal, None, (300, 23), scale).unwrap();
        assert_eq!(panel.y, 50 + 10);
        assert!(panel.height >= 35, "at least the scaled minimum height");
        assert!(panel.x >= terminal.x + 10);
        assert!(panel.x + panel.width <= terminal.x + terminal.width - 10);
    }

    #[test]
    fn a_long_text_is_clamped_to_the_terminal() {
        let panel = handoff_banner_layout(TERMINAL, None, (5_000, 18), 120).unwrap();
        assert_eq!(panel.x, TERMINAL.x + 8);
        assert_eq!(panel.width, TERMINAL.width - 16);
    }

    #[test]
    fn the_banner_keeps_clear_of_the_history_overlay() {
        let history = Rect {
            x: 604,
            y: TERMINAL.y + 8,
            width: 188,
            height: 32,
        };
        let panel = handoff_banner_layout(TERMINAL, Some(history), (700, 18), 120).unwrap();
        assert!(panel.x + panel.width <= history.x - 8);
        // An overlay outside the banner's band does not narrow it.
        let below = Rect {
            y: TERMINAL.y + 200,
            ..history
        };
        let panel = handoff_banner_layout(TERMINAL, Some(below), (700, 18), 120).unwrap();
        assert_eq!(panel.width, 700 + 28);
    }

    #[test]
    fn a_terminal_too_small_for_a_legible_banner_shows_none() {
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        assert_eq!(handoff_banner_layout(tiny, None, (100, 18), 120), None);
        assert_eq!(handoff_banner_layout(TERMINAL, None, (100, 18), 0), None);
    }
}
