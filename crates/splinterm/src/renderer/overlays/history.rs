//! Trusted history, selection, and URL overlay presentation.

use crate::geometry::{Rect, WindowGeometry};

use super::super::{
    SnapshotFrame, compose::paint_placed_glyph_clipped, decorations::paint_decoration_span,
    raster::fill_rect,
};

fn themed_rgba(color: u32, alpha: u8) -> [u8; 4] {
    let [_, red, green, blue] = color.to_be_bytes();
    [red, green, blue, alpha]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HistoryOverlayStatus {
    pub(crate) offset_from_bottom: usize,
    pub(crate) available_rows: usize,
    pub(crate) unseen_rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HistoryOverlayLayout {
    pub(crate) panel: (i32, i32, u32, u32),
    pub(crate) return_to_live: (i32, i32, u32, u32),
}

#[must_use]
pub(crate) fn history_overlay_layout(
    width: u32,
    height: u32,
    scale_120: u32,
) -> Option<HistoryOverlayLayout> {
    if width == 0 || height == 0 || scale_120 == 0 {
        return None;
    }
    let scaled = |logical: u32| logical.saturating_mul(scale_120).div_ceil(120).max(1);
    let margin = scaled(8).min(width / 4).min(height / 4);
    let panel_width = scaled(188).min(width.saturating_sub(margin.saturating_mul(2)));
    let panel_height = scaled(32).min(height.saturating_sub(margin.saturating_mul(2)));
    if panel_width < scaled(72) || panel_height < scaled(20) {
        return None;
    }
    let x = width.saturating_sub(margin).saturating_sub(panel_width);
    let action_width = scaled(32).min(panel_width / 3);
    Some(HistoryOverlayLayout {
        panel: (
            i32::try_from(x).ok()?,
            i32::try_from(margin).ok()?,
            panel_width,
            panel_height,
        ),
        return_to_live: (
            i32::try_from(x.saturating_add(panel_width.saturating_sub(action_width))).ok()?,
            i32::try_from(margin).ok()?,
            action_width,
            panel_height,
        ),
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "content-relative trusted overlay painting keeps geometry and palette explicit"
)]
pub(crate) fn paint_history_overlay(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    content: Rect,
    scale_120: u32,
    status: HistoryOverlayStatus,
    background: u32,
    accent: u32,
) -> Option<HistoryOverlayLayout> {
    let mut layout = history_overlay_layout(content.width, content.height, scale_120)?;
    let offset_x = i32::try_from(content.x).ok()?;
    let offset_y = i32::try_from(content.y).ok()?;
    layout.panel.0 = layout.panel.0.saturating_add(offset_x);
    layout.panel.1 = layout.panel.1.saturating_add(offset_y);
    layout.return_to_live.0 = layout.return_to_live.0.saturating_add(offset_x);
    layout.return_to_live.1 = layout.return_to_live.1.saturating_add(offset_y);
    fill_rect(
        canvas,
        width,
        height,
        layout.panel,
        themed_rgba(background, u8::MAX),
    );
    let (x, y, _, panel_height) = layout.panel;
    let unit = scale_120.div_ceil(120).max(1);
    let bright = themed_rgba(accent, u8::MAX);
    for row in 0..3_u32 {
        fill_rect(
            canvas,
            width,
            height,
            (
                x.saturating_add(i32::try_from(unit.saturating_mul(7)).unwrap_or(0)),
                y.saturating_add(i32::try_from(unit.saturating_mul(8 + row * 6)).unwrap_or(0)),
                unit.saturating_mul(12),
                unit.saturating_mul(2),
            ),
            bright,
        );
    }
    let position = format!(
        "{}/{}",
        status.offset_from_bottom.min(999),
        status.available_rows.min(999)
    );
    paint_history_digits(
        canvas,
        width,
        height,
        x.saturating_add(i32::try_from(unit.saturating_mul(25)).unwrap_or(0)),
        y.saturating_add(i32::try_from(unit.saturating_mul(8)).unwrap_or(0)),
        unit,
        &position,
        bright,
    );
    if status.unseen_rows > 0 {
        let unseen = format!("+{}", status.unseen_rows.min(999));
        paint_history_digits(
            canvas,
            width,
            height,
            x.saturating_add(i32::try_from(unit.saturating_mul(82)).unwrap_or(0)),
            y.saturating_add(i32::try_from(unit.saturating_mul(8)).unwrap_or(0)),
            unit,
            &unseen,
            bright,
        );
    }
    let (action_x, action_y, action_width, action_height) = layout.return_to_live;
    fill_rect(
        canvas,
        width,
        height,
        (action_x, action_y, unit, action_height),
        bright,
    );
    let center_x = action_x.saturating_add(i32::try_from(action_width / 2).unwrap_or(0));
    let center_y = action_y.saturating_add(i32::try_from(panel_height / 2).unwrap_or(0));
    for row in 0..5_u32 {
        let arrow_width = unit.saturating_mul(9_u32.saturating_sub(row.saturating_mul(2)));
        fill_rect(
            canvas,
            width,
            height,
            (
                center_x.saturating_sub(i32::try_from(arrow_width / 2).unwrap_or(0)),
                center_y.saturating_sub(
                    i32::try_from(unit.saturating_mul(2 - row.min(2))).unwrap_or(0),
                ),
                arrow_width.max(unit),
                unit,
            ),
            bright,
        );
    }
    Some(layout)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the tiny trusted bitmap painter keeps explicit canvas and placement contracts"
)]
fn paint_history_digits(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    unit: u32,
    text: &str,
    color: [u8; 4],
) {
    let pattern = |character: char| match character {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    };
    for (index, character) in text.chars().enumerate() {
        let glyph = pattern(character);
        let glyph_x = x.saturating_add(
            i32::try_from(
                index
                    .saturating_mul(4)
                    .saturating_mul(usize::try_from(unit).unwrap_or(usize::MAX)),
            )
            .unwrap_or(i32::MAX),
        );
        for (row, bits) in glyph.into_iter().enumerate() {
            for column in 0..3_u8 {
                if bits & (1 << (2 - column)) == 0 {
                    continue;
                }
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        glyph_x
                            .saturating_add(i32::from(column) * i32::try_from(unit).unwrap_or(1)),
                        y.saturating_add(
                            i32::try_from(row).unwrap_or(0) * i32::try_from(unit).unwrap_or(1),
                        ),
                        unit,
                        unit,
                    ),
                    color,
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SnapshotOverlays<'a> {
    pub(crate) selection: Option<((usize, usize), (usize, usize))>,
    pub(crate) hovered_url: Option<((usize, usize), (usize, usize))>,
    pub(crate) dirty_rows: Option<&'a [bool]>,
    pub(crate) focused: bool,
    /// Packed `0xRRGGBB` project theme roles.
    pub(crate) selection_color: u32,
    pub(crate) selection_foreground: u32,
    pub(crate) url_color: u32,
    pub(crate) accent_color: u32,
}

#[allow(
    clippy::too_many_lines,
    reason = "selection background and foreground remain one ordered theme-composition boundary"
)]
pub(crate) fn paint_snapshot_overlays(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    overlays: SnapshotOverlays<'_>,
) {
    let SnapshotOverlays {
        selection,
        hovered_url,
        dirty_rows,
        focused: _focused,
        selection_color,
        selection_foreground,
        url_color,
        accent_color: _accent_color,
    } = overlays;
    // Focus framing belongs to the compositor. Painting a second solid frame
    // inside the client obscures Hyprland's active-border gradient.
    let row_is_dirty =
        |row: usize| dirty_rows.is_none_or(|dirty| dirty.get(row).copied().unwrap_or(false));
    if let Some((start, end)) = selection {
        for row in start.0..=end.0.min(frame.rows.saturating_sub(1) as usize) {
            if !row_is_dirty(row) {
                continue;
            }
            let first = if row == start.0 { start.1 } else { 0 };
            let last = if row == end.0 {
                end.1
            } else {
                frame.columns.saturating_sub(1) as usize
            };
            let last = last.min(frame.columns.saturating_sub(1) as usize);
            let (Ok(first), Ok(last), Ok(row)) = (
                u32::try_from(first),
                u32::try_from(last),
                u32::try_from(row),
            ) else {
                continue;
            };
            let (Some((left, top, cell_width, cell_height)), Some((right, _, _, _))) = (
                frame.cell_rect(geometry, first, row),
                frame.cell_rect(geometry, last, row),
            ) else {
                continue;
            };
            let selection_width = u32::try_from(right.saturating_sub(left))
                .unwrap_or(0)
                .saturating_add(cell_width);
            fill_rect(
                canvas,
                width,
                height,
                (left, top, selection_width, cell_height),
                themed_rgba(selection_color, u8::MAX),
            );
            let clip = (
                left,
                top,
                left.saturating_add(i32::try_from(selection_width).unwrap_or(i32::MAX)),
                top.saturating_add(i32::try_from(cell_height).unwrap_or(i32::MAX)),
            );
            let selected_foreground = {
                let [_, red, green, blue] = selection_foreground.to_be_bytes();
                [red, green, blue]
            };
            for glyph in frame
                .glyphs
                .iter()
                .filter(|glyph| {
                    glyph.row == row
                        && glyph.column < last.saturating_add(1)
                        && glyph.column.saturating_add(glyph.cells.max(1)) > first
                })
                .rev()
            {
                paint_placed_glyph_clipped(
                    canvas,
                    width,
                    height,
                    frame,
                    geometry,
                    glyph,
                    selected_foreground,
                    Some(clip),
                );
            }
            for decoration in frame.decorations.iter().filter(|decoration| {
                decoration.row == row
                    && decoration.column < last.saturating_add(1)
                    && decoration.column.saturating_add(decoration.cells) > first
            }) {
                let mut selected = *decoration;
                let decoration_end = selected.column.saturating_add(selected.cells);
                selected.column = selected.column.max(first);
                selected.cells = decoration_end
                    .min(last.saturating_add(1))
                    .saturating_sub(selected.column);
                paint_decoration_span(
                    canvas,
                    width,
                    height,
                    frame,
                    geometry,
                    &selected,
                    Some(selected_foreground),
                );
            }
        }
    }
    if let Some((start, end)) = hovered_url
        && start.0 == end.0
        && row_is_dirty(start.0)
    {
        for column in start.1..=end.1.min(frame.columns.saturating_sub(1) as usize) {
            let (Ok(column), Ok(row)) = (u32::try_from(column), u32::try_from(start.0)) else {
                continue;
            };
            let Some((x, y, cell_width, cell_height)) = frame.cell_rect(geometry, column, row)
            else {
                continue;
            };
            fill_rect(
                canvas,
                width,
                height,
                (
                    x,
                    y + i32::try_from(cell_height.saturating_sub(2)).unwrap_or(0),
                    cell_width,
                    2,
                ),
                themed_rgba(url_color, u8::MAX),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_history_overlay_is_bounded_static_and_clamps_counts() {
        let width = 320;
        let height = 180;
        let sentinel = [1, 2, 3, 4];
        let mut clamped = sentinel.repeat(usize::try_from(width * height).unwrap());
        let layout = paint_history_overlay(
            &mut clamped,
            width,
            height,
            Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            120,
            HistoryOverlayStatus {
                offset_from_bottom: 12,
                available_rows: 4_096,
                unseen_rows: 1_000,
            },
            0x0010_1820,
            0x00f2_3888,
        )
        .expect("overlay fits");
        let mut maximum = sentinel.repeat(usize::try_from(width * height).unwrap());
        paint_history_overlay(
            &mut maximum,
            width,
            height,
            Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            120,
            HistoryOverlayStatus {
                offset_from_bottom: 12,
                available_rows: 999,
                unseen_rows: 999,
            },
            0x0010_1820,
            0x00f2_3888,
        );
        assert_eq!(clamped, maximum);
        let (panel_x, panel_y, panel_width, panel_height) = layout.panel;
        let pixel = |canvas: &[u8], x: i32, y: i32| -> [u8; 4] {
            let index = usize::try_from(
                (u32::try_from(y).unwrap() * width + u32::try_from(x).unwrap()) * 4,
            )
            .unwrap();
            canvas[index..index + 4].try_into().unwrap()
        };
        assert_eq!(
            pixel(
                &clamped,
                panel_x.saturating_add(1),
                panel_y.saturating_add(i32::try_from(panel_height - 1).unwrap())
            ),
            [0x20, 0x18, 0x10, 0xff],
            "panel uses the exact theme background without red/blue reversal"
        );
        assert_eq!(
            pixel(
                &clamped,
                panel_x.saturating_add(7),
                panel_y.saturating_add(8)
            ),
            [0x88, 0x38, 0xf2, 0xff],
            "history marks use the exact Sakura Mochi accent"
        );
        let (action_x, action_y, action_width, action_height) = layout.return_to_live;
        assert!(action_x >= panel_x);
        assert_eq!(action_y, panel_y);
        assert!(action_width <= panel_width);
        assert_eq!(action_height, panel_height);
        for y in 0..height {
            for x in 0..width {
                let inside = i32::try_from(x).unwrap() >= panel_x
                    && i32::try_from(x).unwrap()
                        < panel_x.saturating_add(i32::try_from(panel_width).unwrap())
                    && i32::try_from(y).unwrap() >= panel_y
                    && i32::try_from(y).unwrap()
                        < panel_y.saturating_add(i32::try_from(panel_height).unwrap());
                if !inside {
                    let index = usize::try_from((y * width + x) * 4).unwrap();
                    assert_eq!(&clamped[index..index + 4], sentinel.as_slice());
                }
            }
        }
        assert!(history_overlay_layout(40, 20, 120).is_none());
    }
}
