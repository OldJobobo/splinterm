//! Layered snapshot composition, row damage painting, and scroll copying.

use splinterm_protocol::{ScrollDirection, TerminalScroll};

use crate::{
    box_drawing,
    config::CursorStyle,
    geometry::{Rect, WindowGeometry},
};

use super::{
    BOX_DRAWING_FACE, CursorPresentation, EffectiveCursorShape, SnapshotFrame, SnapshotGlyph,
    cursor::{cursor_colors_for_cell, cursor_span, effective_cursor_shape, paint_effective_cursor},
    decorations::paint_decoration_span,
    images::paint_snapshot_images,
    raster::{background_alpha_u8, blend_glyph, fill_rect, premultiplied_rgba},
    round_to_i32,
};

pub(super) fn paint_placed_glyph(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    placed: &SnapshotGlyph,
    foreground: [u8; 3],
) {
    let glyph = &frame.cache[&placed.key];
    let Some((cell_x, cell_y, _, _)) = frame.cell_rect(geometry, placed.column, placed.row) else {
        return;
    };
    // Foot starts each cell at its grid pen; fallback advance does not center
    // wide glyphs inside the declared terminal span.
    let pen_x = cell_x + round_to_i32(placed.x_offset);
    let baseline = cell_y + frame.baseline - round_to_i32(placed.y_offset);
    let grid = geometry.grid_rect;
    let grid_left = i32::try_from(grid.x).unwrap_or(i32::MAX);
    let grid_top = i32::try_from(grid.y).unwrap_or(i32::MAX);
    let grid_right = i32::try_from(grid.x.saturating_add(grid.width)).unwrap_or(i32::MAX);
    let grid_bottom = i32::try_from(grid.y.saturating_add(grid.height)).unwrap_or(i32::MAX);
    if placed.key.face == BOX_DRAWING_FACE {
        let character = char::from_u32(u32::from(placed.key.glyph));
        if let Some((rect_x, rect_y, rect_width, rect_height)) = character.and_then(|character| {
            box_drawing::opaque_block_rect(character, glyph.width, glyph.height)
        }) {
            fill_rect(
                canvas,
                width,
                height,
                (
                    pen_x + glyph.left + i32::try_from(rect_x).expect("block x fits i32"),
                    baseline - glyph.top + i32::try_from(rect_y).expect("block y fits i32"),
                    rect_width,
                    rect_height,
                ),
                [foreground[0], foreground[1], foreground[2], 0xff],
            );
            return;
        }
    }
    blend_glyph(
        canvas,
        width,
        height,
        pen_x + glyph.left,
        baseline - glyph.top,
        glyph,
        foreground,
        Some((grid_left, grid_top, grid_right, grid_bottom)),
    );
}

#[cfg(test)]
pub(super) fn paint_glyphs(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    dirty_rows: Option<&[bool]>,
) {
    for row in 0..frame.rows {
        if dirty_rows.is_some_and(|rows| {
            !rows
                .get(usize::try_from(row).expect("row fits usize"))
                .copied()
                .unwrap_or(false)
        }) {
            continue;
        }
        let start = frame.glyphs.partition_point(|glyph| glyph.row < row);
        let end = frame.glyphs.partition_point(|glyph| glyph.row <= row);
        // Foot renders each row from right to left. This order is observable
        // when a glyph overhangs into its neighbor and both masks touch the
        // same antialiased pixel.
        for placed in frame.glyphs[start..end].iter().rev() {
            paint_placed_glyph(
                canvas,
                width,
                height,
                frame,
                geometry,
                placed,
                placed.foreground,
            );
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one layered compositor owns backgrounds, image tiers, text, cursor, and row damage"
)]
fn compose_snapshot_rows(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    region: Rect,
    dirty_rows: Option<&[bool]>,
    cursor_visible: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) {
    let row_is_dirty = |row: u32| {
        dirty_rows.is_none_or(|rows| {
            rows.get(usize::try_from(row).expect("row fits usize"))
                .copied()
                .unwrap_or(false)
        })
    };
    let canvas_background = premultiplied_rgba(frame.canvas_background, background_alpha_u8());
    let visible_columns = frame.columns.min(geometry.columns);
    for row in 0..frame.rows {
        if !row_is_dirty(row) || visible_columns == 0 {
            continue;
        }
        let Some((x, y, cell_width, cell_height)) = frame.cell_rect(geometry, 0, row) else {
            continue;
        };
        fill_rect(
            canvas,
            width,
            height,
            (
                x,
                y,
                cell_width.saturating_mul(visible_columns),
                cell_height,
            ),
            canvas_background,
        );
    }
    paint_snapshot_images(
        canvas, width, height, frame, geometry, region, dirty_rows, 0,
    );
    for row in 0..frame.rows {
        if !row_is_dirty(row) {
            continue;
        }
        for column in (0..frame.columns).rev() {
            let index = usize::try_from(row * frame.columns + column).expect("cell index");
            if frame.default_backgrounds[index] {
                continue;
            }
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
                    y,
                    cell_width.saturating_mul(frame.cell_spans[index].max(1)),
                    cell_height,
                ),
                [
                    frame.backgrounds[index][0],
                    frame.backgrounds[index][1],
                    frame.backgrounds[index][2],
                    u8::MAX,
                ],
            );
        }
    }
    paint_snapshot_images(
        canvas, width, height, frame, geometry, region, dirty_rows, 1,
    );

    let effective = effective_cursor_shape(cursor_style, cursor_visible, presentation);
    for row in 0..frame.rows {
        if !row_is_dirty(row) {
            continue;
        }
        let glyph_start = frame.glyphs.partition_point(|glyph| glyph.row < row);
        let glyph_end = frame.glyphs.partition_point(|glyph| glyph.row <= row);
        let row_glyphs = &frame.glyphs[glyph_start..glyph_end];
        let decoration_start = frame
            .decorations
            .partition_point(|decoration| decoration.row < row);
        let decoration_end = frame
            .decorations
            .partition_point(|decoration| decoration.row <= row);
        let row_decorations = &frame.decorations[decoration_start..decoration_end];
        let mut glyph_end = row_glyphs.len();
        let mut decoration_end = row_decorations.len();
        for column in (0..frame.columns).rev() {
            let has_block_cursor =
                frame.cursor == Some((column, row)) && effective == EffectiveCursorShape::Block;
            let mut glyph_start = glyph_end;
            while glyph_start > 0 && row_glyphs[glyph_start - 1].column == column {
                glyph_start -= 1;
            }
            if !has_block_cursor {
                for placed in row_glyphs[glyph_start..glyph_end].iter().rev() {
                    paint_placed_glyph(
                        canvas,
                        width,
                        height,
                        frame,
                        geometry,
                        placed,
                        placed.foreground,
                    );
                }
            }
            glyph_end = glyph_start;

            let mut decoration_start = decoration_end;
            while decoration_start > 0 && row_decorations[decoration_start - 1].column == column {
                decoration_start -= 1;
            }
            if !has_block_cursor {
                for decoration in &row_decorations[decoration_start..decoration_end] {
                    paint_decoration_span(canvas, width, height, frame, geometry, decoration, None);
                }
            }
            decoration_end = decoration_start;
        }
    }
    paint_snapshot_images(
        canvas, width, height, frame, geometry, region, dirty_rows, 2,
    );

    if let Some((column, row)) = frame.cursor.filter(|(_, row)| row_is_dirty(*row)) {
        let index = usize::try_from(row * frame.columns + column).expect("cursor cell index");
        if let Some((x, y, _, _)) = frame.cell_rect(geometry, column, row) {
            let cursor_color = cursor_colors_for_cell(
                Some(frame.cursor_color),
                frame.foregrounds[index],
                frame.backgrounds[index],
            )
            .0;
            paint_effective_cursor(
                canvas,
                width,
                height,
                frame,
                x,
                y,
                cursor_span(frame, column, row),
                frame.cell_metrics[index],
                [cursor_color[0], cursor_color[1], cursor_color[2], 0xff],
                effective,
            );
        }
    }
}

#[allow(clippy::too_many_arguments, reason = "cursor presentation is explicit")]
pub(crate) fn paint_snapshot_presented(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    cursor_visible: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) {
    paint_snapshot_region_presented(
        canvas,
        width,
        height,
        frame,
        geometry,
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
        cursor_visible,
        cursor_style,
        presentation,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "pane region and cursor presentation are explicit"
)]
pub(crate) fn paint_snapshot_region_presented(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    region: Rect,
    cursor_visible: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) {
    let background = premultiplied_rgba(frame.canvas_background, background_alpha_u8());
    fill_rect(
        canvas,
        width,
        height,
        (
            i32::try_from(region.x).unwrap_or(i32::MAX),
            i32::try_from(region.y).unwrap_or(i32::MAX),
            region.width,
            region.height,
        ),
        background,
    );
    compose_snapshot_rows(
        canvas,
        width,
        height,
        frame,
        geometry,
        region,
        None,
        cursor_visible,
        cursor_style,
        presentation,
    );
}

pub(crate) fn paint_snapshot(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    cursor_visible: bool,
    cursor_style: CursorStyle,
) {
    paint_snapshot_presented(
        canvas,
        width,
        height,
        frame,
        geometry,
        cursor_visible,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "damage and cursor presentation are explicit"
)]
pub(crate) fn paint_snapshot_rows_presented(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    dirty_rows: &[bool],
    cursor_visible: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) {
    compose_snapshot_rows(
        canvas,
        width,
        height,
        frame,
        geometry,
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
        Some(dirty_rows),
        cursor_visible,
        cursor_style,
        presentation,
    );
}

#[allow(clippy::too_many_arguments, reason = "damage inputs remain explicit")]
pub(crate) fn paint_snapshot_rows(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    dirty_rows: &[bool],
    cursor_visible: bool,
    cursor_style: CursorStyle,
) {
    paint_snapshot_rows_presented(
        canvas,
        width,
        height,
        frame,
        geometry,
        dirty_rows,
        cursor_visible,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    );
}

pub(crate) fn scroll_snapshot_pixels(
    canvas: &mut [u8],
    width: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    scroll: TerminalScroll,
) {
    if scroll.rows == 0
        || scroll.start_row >= scroll.end_row
        || scroll.end_row > frame.rows as usize
    {
        return;
    }
    let Some(stride) = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .filter(|stride| *stride != 0)
    else {
        return;
    };
    let canvas_height = canvas.len() / stride;
    let grid = geometry.grid_rect();
    let Some(x) = usize::try_from(grid.x)
        .ok()
        .and_then(|origin| origin.checked_mul(4))
        .filter(|x| *x < stride)
    else {
        return;
    };
    let copy_width = usize::try_from(grid.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .map_or(0, |width| width.min(stride - x));
    if copy_width == 0 {
        return;
    }
    let cell_height = usize::try_from(geometry.cell.height).expect("cell height");
    let origin_y = usize::try_from(grid.y).expect("origin fits usize");
    let Some(start_y) = scroll
        .start_row
        .checked_mul(cell_height)
        .and_then(|offset| origin_y.checked_add(offset))
    else {
        return;
    };
    let Some(end_y) = scroll
        .end_row
        .checked_mul(cell_height)
        .and_then(|offset| origin_y.checked_add(offset))
        .map(|end| end.min(canvas_height))
    else {
        return;
    };
    let Some(shift) = scroll
        .rows
        .min(scroll.end_row - scroll.start_row)
        .checked_mul(cell_height)
    else {
        return;
    };
    if start_y >= end_y || shift >= end_y - start_y {
        return;
    }
    match scroll.direction {
        ScrollDirection::Forward => {
            for y in start_y..end_y - shift {
                let source = (y + shift) * stride + x;
                let destination = y * stride + x;
                canvas.copy_within(source..source + copy_width, destination);
            }
        }
        ScrollDirection::Reverse => {
            for y in (start_y + shift..end_y).rev() {
                let source = (y - shift) * stride + x;
                let destination = y * stride + x;
                canvas.copy_within(source..source + copy_width, destination);
            }
        }
    }
}

pub(crate) fn snapshot_row_rect(
    geometry: &WindowGeometry,
    row: usize,
) -> Option<(i32, i32, i32, i32)> {
    let rect = geometry.row_rect(row)?;
    Some((
        i32::try_from(rect.x).ok()?,
        i32::try_from(rect.y).ok()?,
        i32::try_from(rect.width).ok()?,
        i32::try_from(rect.height).ok()?,
    ))
}
