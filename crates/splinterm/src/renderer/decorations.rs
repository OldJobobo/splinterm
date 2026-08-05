//! Foot-derived underline, strike, and decoration rasterization.

use splinterm_protocol::UnderlineStyle;

use crate::geometry::WindowGeometry;

use super::{
    CellMetrics, SnapshotFrame,
    raster::{blend_pixel, fill_rect},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DecorationMetrics {
    pub(super) underline_position: i32,
    pub(super) underline_thickness: u32,
    pub(super) strike_position: i32,
    pub(super) strike_thickness: u32,
}

impl From<CellMetrics> for DecorationMetrics {
    fn from(metrics: CellMetrics) -> Self {
        Self {
            underline_position: metrics.underline_position,
            underline_thickness: metrics.underline_thickness,
            strike_position: metrics.strike_position,
            strike_thickness: metrics.strike_thickness,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DecorationSpan {
    pub(super) column: u32,
    pub(super) row: u32,
    pub(super) cells: u32,
    pub(super) underline: UnderlineStyle,
    pub(super) strikethrough: bool,
    pub(super) underline_color: [u8; 3],
    pub(super) underline_uses_foreground: bool,
    pub(super) strike_color: [u8; 3],
    pub(super) metrics: DecorationMetrics,
}

const PIXMAN_FIXED_ONE: i64 = 1 << 16;
const PIXMAN_A8_Y_SAMPLES: i64 = 15;
const PIXMAN_A8_X_SAMPLES: i64 = 17;
const PIXMAN_A8_Y_STEP: i64 = PIXMAN_FIXED_ONE / PIXMAN_A8_Y_SAMPLES;
const PIXMAN_A8_Y_FIRST: i64 =
    (PIXMAN_FIXED_ONE - (PIXMAN_A8_Y_SAMPLES - 1) * PIXMAN_A8_Y_STEP) / 2;
const PIXMAN_A8_X_STEP: i64 = PIXMAN_FIXED_ONE / PIXMAN_A8_X_SAMPLES;
const PIXMAN_A8_X_FIRST: i64 =
    (PIXMAN_FIXED_ONE - (PIXMAN_A8_X_SAMPLES - 1) * PIXMAN_A8_X_STEP) / 2;

fn pixman_edge_x_at(first: (i32, i32), second: (i32, i32), sample_y_fixed: i64) -> i64 {
    let (top, bottom) = if first.1 <= second.1 {
        (first, second)
    } else {
        (second, first)
    };
    let x_top = i64::from(top.0) * PIXMAN_FIXED_ONE;
    let y_top = i64::from(top.1) * PIXMAN_FIXED_ONE;
    let x_bottom = i64::from(bottom.0) * PIXMAN_FIXED_ONE;
    let y_bottom = i64::from(bottom.1) * PIXMAN_FIXED_ONE;
    let dx = x_bottom - x_top;
    let dy = y_bottom - y_top;
    if dy == 0 {
        return x_top;
    }
    let (sign_dx, step_x, edge_dx, edge_error) = if dx >= 0 {
        (1_i64, dx / dy, dx % dy, -dy)
    } else {
        (-1_i64, -((-dx) / dy), (-dx) % dy, 0_i64)
    };
    let steps = sample_y_fixed - y_top;
    let mut x = x_top + steps * step_x;
    let mut next_error = edge_error + steps * edge_dx;
    if steps >= 0 {
        if next_error > 0 {
            let crossed = (next_error + dy - 1) / dy;
            next_error -= crossed * dy;
            x += crossed * sign_dx;
        }
    } else if next_error <= -dy {
        let crossed = (-next_error) / dy;
        next_error += crossed * dy;
        x -= crossed * sign_dx;
    }
    let _ = next_error;
    x
}

fn pixman_a8_sample_x(x_fixed: i64) -> i64 {
    (x_fixed.rem_euclid(PIXMAN_FIXED_ONE) + PIXMAN_A8_X_FIRST) / PIXMAN_A8_X_STEP
}

fn pixman_a8_add_span(mask: &mut [u8], row: usize, width: usize, mut left: i64, mut right: i64) {
    left = left.max(0);
    if right.div_euclid(PIXMAN_FIXED_ONE) >= i64::try_from(width).unwrap_or(i64::MAX) {
        right = i64::try_from(width)
            .unwrap_or(i64::MAX)
            .saturating_mul(PIXMAN_FIXED_ONE)
            .saturating_sub(1);
    }
    if right <= left {
        return;
    }
    let left_pixel = left.div_euclid(PIXMAN_FIXED_ONE);
    let right_pixel = right.div_euclid(PIXMAN_FIXED_ONE);
    if left_pixel < 0 || right_pixel < 0 {
        return;
    }
    let Ok(left_pixel) = usize::try_from(left_pixel) else {
        return;
    };
    let Ok(right_pixel) = usize::try_from(right_pixel) else {
        return;
    };
    if left_pixel >= width || right_pixel >= width {
        return;
    }
    let left_samples = pixman_a8_sample_x(left);
    let right_samples = pixman_a8_sample_x(right);
    let mut add = |column: usize, coverage: i64| {
        let index = row * width + column;
        let coverage = u8::try_from(coverage.clamp(0, 255)).unwrap_or(255);
        mask[index] = mask[index].saturating_add(coverage);
    };
    if left_pixel == right_pixel {
        add(left_pixel, right_samples - left_samples);
        return;
    }
    add(left_pixel, PIXMAN_A8_X_SAMPLES - left_samples);
    for column in left_pixel + 1..right_pixel {
        add(column, PIXMAN_A8_X_SAMPLES);
    }
    add(right_pixel, right_samples);
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "terminal-bounded dimensions keep the rounded pixman edge within i32"
)]
fn pixman_curly_a8_mask(span_width: u32, thickness: u32) -> Vec<u8> {
    let height = thickness.saturating_mul(3);
    let Some(length) = usize::try_from(span_width)
        .ok()
        .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
    else {
        return Vec::new();
    };
    let mut mask = vec![0_u8; length];
    if span_width == 0 || thickness == 0 {
        return mask;
    }
    let full = i32::try_from(span_width).unwrap_or(i32::MAX);
    let half = full / 2;
    let bottom = i32::try_from(height).unwrap_or(i32::MAX);
    let thickness_f = f64::from(thickness);
    let bottom_f = f64::from(bottom);
    let width_f = f64::from(span_width);
    let edge = ((thickness_f.powi(2)
        + thickness_f.powi(2) * bottom_f.powi(2) / (width_f.powi(2) / 4.0))
        .sqrt()
        / 2.0)
        .round() as i32;
    let traps = [
        (
            ((0, bottom - edge), (half, -edge)),
            ((0, bottom + edge), (half, edge)),
        ),
        (
            ((half, edge), (full, bottom + edge)),
            ((half, -edge), (full, bottom - edge)),
        ),
    ];
    let width = usize::try_from(span_width).unwrap_or(0);
    for row in 0..height {
        for sample in 0..PIXMAN_A8_Y_SAMPLES {
            let sample_y =
                i64::from(row) * PIXMAN_FIXED_ONE + PIXMAN_A8_Y_FIRST + sample * PIXMAN_A8_Y_STEP;
            for (left, right) in traps {
                let left_x = pixman_edge_x_at(left.0, left.1, sample_y);
                let right_x = pixman_edge_x_at(right.0, right.1, sample_y);
                pixman_a8_add_span(
                    &mut mask,
                    usize::try_from(row).unwrap_or(0),
                    width,
                    left_x,
                    right_x,
                );
            }
        }
    }
    mask
}

#[allow(
    clippy::too_many_arguments,
    reason = "Pixman-equivalent A8 mask composition keeps geometry explicit"
)]
fn paint_curly_trapezoids(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    top: i32,
    span_width: u32,
    thickness: u32,
    color: [u8; 4],
) {
    let mask = pixman_curly_a8_mask(span_width, thickness);
    let mask_width = usize::try_from(span_width).unwrap_or(0);
    for (index, coverage) in mask.into_iter().enumerate() {
        if coverage == 0 || mask_width == 0 {
            continue;
        }
        let alpha = u8::try_from((u16::from(coverage) * u16::from(color[3]) + 127) / 255)
            .unwrap_or(u8::MAX);
        blend_pixel(
            canvas,
            width,
            height,
            x + i32::try_from(index % mask_width).unwrap_or(0),
            top + i32::try_from(index / mask_width).unwrap_or(0),
            [color[0], color[1], color[2], alpha],
        );
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "underline raster geometry and destination buffer are explicit"
)]
fn paint_underline_style(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    span_width: u32,
    cell_width: u32,
    thickness: u32,
    style: UnderlineStyle,
    color: [u8; 4],
) {
    match style {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => {
            fill_rect(canvas, width, height, (x, y, span_width, thickness), color);
        }
        UnderlineStyle::Double => {
            fill_rect(canvas, width, height, (x, y, span_width, thickness), color);
            fill_rect(
                canvas,
                width,
                height,
                (
                    x,
                    y + i32::try_from(thickness.saturating_mul(2)).unwrap_or(0),
                    span_width,
                    thickness,
                ),
                color,
            );
        }
        UnderlineStyle::Curly => {
            paint_curly_trapezoids(canvas, width, height, x, y, span_width, thickness, color);
        }
        UnderlineStyle::Dotted => {
            let mut per_cell = (cell_width / thickness) / 2;
            if per_cell == 0 {
                per_cell = 1;
            }
            let mut spacing = vec![thickness; usize::try_from(per_cell).unwrap_or(1)];
            let used = per_cell.saturating_mul(2).saturating_mul(thickness);
            let mut remaining = cell_width.saturating_sub(used);
            let mut index = 0_usize;
            while remaining > 0 {
                spacing[index] = spacing[index].saturating_add(1);
                remaining -= 1;
                index = (index + 1) % spacing.len();
            }
            let mut dot_x = 0_u32;
            for gap in spacing {
                if dot_x >= span_width {
                    break;
                }
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        x + i32::try_from(dot_x).unwrap_or(0),
                        y,
                        thickness.min(span_width - dot_x),
                        thickness,
                    ),
                    color,
                );
                dot_x = dot_x.saturating_add(thickness).saturating_add(gap);
            }
        }
        UnderlineStyle::Dashed => {
            let dash = span_width.div_ceil(3);
            for offset in [0, dash.saturating_mul(2)] {
                if offset < span_width {
                    fill_rect(
                        canvas,
                        width,
                        height,
                        (
                            x + i32::try_from(offset).unwrap_or(0),
                            y,
                            dash.min(span_width - offset),
                            thickness,
                        ),
                        color,
                    );
                }
            }
        }
    }
}

fn underline_y_offset(
    cell_height: u32,
    baseline: i32,
    metrics: DecorationMetrics,
    style: UnderlineStyle,
) -> i32 {
    let thickness = metrics.underline_thickness.max(1);
    let bottom_reserve = match style {
        UnderlineStyle::Double | UnderlineStyle::Curly => thickness.saturating_mul(3),
        _ => thickness,
    };
    let natural = baseline - metrics.underline_position;
    let maximum = i32::try_from(cell_height)
        .unwrap_or(i32::MAX)
        .saturating_sub(i32::try_from(bottom_reserve).unwrap_or(i32::MAX));
    natural.min(maximum)
}

pub(super) fn paint_decoration_span(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    span: &DecorationSpan,
    foreground_override: Option<[u8; 3]>,
) {
    let Some((x, row_top, _, _)) = frame.cell_rect(geometry, span.column, span.row) else {
        return;
    };
    let span_width = frame.cell_width.saturating_mul(span.cells);
    let underline_rgb = if span.underline_uses_foreground {
        foreground_override.unwrap_or(span.underline_color)
    } else {
        span.underline_color
    };
    let underline_color = [underline_rgb[0], underline_rgb[1], underline_rgb[2], 0xff];
    let thickness = span.metrics.underline_thickness.max(1);
    paint_underline_style(
        canvas,
        width,
        height,
        x,
        row_top
            + underline_y_offset(
                frame.cell_height,
                frame.baseline,
                span.metrics,
                span.underline,
            ),
        span_width,
        frame.cell_width,
        thickness,
        span.underline,
        underline_color,
    );
    if span.strikethrough {
        let strike_rgb = foreground_override.unwrap_or(span.strike_color);
        let strike_color = [strike_rgb[0], strike_rgb[1], strike_rgb[2], 0xff];
        fill_rect(
            canvas,
            width,
            height,
            (
                x,
                row_top + frame.baseline - span.metrics.strike_position,
                span_width,
                span.metrics.strike_thickness,
            ),
            strike_color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn underline_red_mask(
        style: UnderlineStyle,
        width: u32,
        cell_width: u32,
        thickness: u32,
    ) -> Vec<u8> {
        let height = thickness.saturating_mul(3).max(1);
        let mut canvas = vec![0; usize::try_from(width * height * 4).unwrap()];
        paint_underline_style(
            &mut canvas,
            width,
            height,
            0,
            0,
            width,
            cell_width,
            thickness,
            style,
            [255, 255, 255, 255],
        );
        canvas.chunks_exact(4).map(|pixel| pixel[2]).collect()
    }

    #[test]
    fn foot_dashed_dotted_and_double_vectors_are_exact() {
        let dashed = underline_red_mask(UnderlineStyle::Dashed, 14, 7, 1);
        assert_eq!(
            &dashed[..14],
            &[255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 255, 255, 255, 255]
        );
        let dotted = underline_red_mask(UnderlineStyle::Dotted, 7, 7, 1);
        assert_eq!(&dotted[..7], &[255, 0, 0, 255, 0, 255, 0]);
        let double = underline_red_mask(UnderlineStyle::Double, 7, 7, 2);
        assert!(double[..14].iter().all(|value| *value == 255));
        assert!(double[14..28].iter().all(|value| *value == 0));
        assert!(double[28..42].iter().all(|value| *value == 255));
    }

    #[test]
    fn underline_bottom_clamps_match_single_and_three_thickness_styles() {
        let metrics = DecorationMetrics {
            underline_position: -100,
            underline_thickness: 3,
            strike_position: 0,
            strike_thickness: 1,
        };
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Single),
            7
        );
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Dotted),
            7
        );
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Dashed),
            7
        );
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Double),
            1
        );
        assert_eq!(underline_y_offset(10, 7, metrics, UnderlineStyle::Curly), 1);
    }

    #[test]
    fn curly_trapezoid_a8_vectors_match_pinned_pixman_0_46_4() {
        let vectors = [
            (
                7,
                1,
                vec![
                    0, 137, 255, 255, 211, 44, 0, 127, 255, 128, 96, 245, 245, 96, 255, 128, 0, 0,
                    44, 211, 255,
                ],
            ),
            (
                9,
                1,
                vec![
                    0, 44, 245, 255, 255, 255, 128, 9, 0, 96, 245, 245, 96, 76, 220, 255, 221, 77,
                    255, 211, 44, 0, 0, 9, 127, 246, 255,
                ],
            ),
            (
                11,
                2,
                vec![
                    0, 0, 0, 66, 255, 255, 128, 0, 0, 0, 0, 0, 0, 36, 240, 150, 127, 255, 128, 0,
                    0, 0, 0, 15, 219, 189, 3, 0, 127, 255, 128, 0, 0, 3, 189, 219, 15, 0, 0, 0,
                    127, 255, 128, 0, 150, 240, 36, 0, 0, 0, 0, 0, 127, 255, 128, 252, 66, 0, 0, 0,
                    0, 0, 0, 0, 127, 255,
                ],
            ),
            (
                14,
                2,
                vec![
                    0, 0, 0, 0, 12, 181, 255, 255, 181, 12, 0, 0, 0, 0, 0, 0, 0, 28, 207, 252, 108,
                    108, 252, 207, 28, 0, 0, 0, 0, 0, 48, 227, 243, 77, 0, 0, 77, 243, 227, 48, 0,
                    0, 0, 77, 243, 227, 48, 0, 0, 0, 0, 48, 227, 243, 77, 0, 108, 252, 207, 28, 0,
                    0, 0, 0, 0, 0, 28, 207, 252, 108, 255, 178, 12, 0, 0, 0, 0, 0, 0, 0, 0, 12,
                    178, 255,
                ],
            ),
        ];
        for (width, thickness, expected) in vectors {
            assert_eq!(pixman_curly_a8_mask(width, thickness), expected);
        }
    }

    #[test]
    fn underline_styles_render_distinct_bounded_patterns() {
        let styles = [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ];
        let mut outputs = Vec::new();
        for style in styles {
            let mut canvas = vec![0; 16 * 6 * 4];
            paint_underline_style(
                &mut canvas,
                16,
                6,
                0,
                1,
                16,
                16,
                1,
                style,
                [255, 255, 255, 255],
            );
            assert!(canvas.chunks_exact(4).any(|pixel| pixel[3] != 0));
            outputs.push(canvas);
        }
        for left in 0..outputs.len() {
            for right in left + 1..outputs.len() {
                assert_ne!(outputs[left], outputs[right]);
            }
        }
    }
}
