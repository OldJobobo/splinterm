//! Low-level deterministic CPU raster and alpha-composition primitives.

use swash::scale::image::Content;

use crate::{box_drawing, geometry::Rect};

use super::CachedGlyph;

pub(crate) fn fill_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    rect: (i32, i32, u32, u32),
    color: [u8; 4],
) {
    let (x, y, width, height) = rect;
    let left = i64::from(x).clamp(0, i64::from(canvas_width));
    let top = i64::from(y).clamp(0, i64::from(canvas_height));
    let right = (i64::from(x) + i64::from(width)).clamp(0, i64::from(canvas_width));
    let bottom = (i64::from(y) + i64::from(height)).clamp(0, i64::from(canvas_height));
    if left >= right || top >= bottom {
        return;
    }
    let stride = usize::try_from(canvas_width).expect("canvas width fits usize") * 4;
    let left = usize::try_from(left).expect("clipped rectangle left fits usize") * 4;
    let right = usize::try_from(right).expect("clipped rectangle right fits usize") * 4;
    let top = usize::try_from(top).expect("clipped rectangle top fits usize");
    let bottom = usize::try_from(bottom).expect("clipped rectangle bottom fits usize");
    let first_start = top * stride + left;
    let first_end = top * stride + right;
    let bgra = [color[2], color[1], color[0], color[3]];
    canvas[first_start..first_start + 4].copy_from_slice(&bgra);
    let mut filled = 4;
    while first_start + filled < first_end {
        let copied = filled.min(first_end - first_start - filled);
        canvas.copy_within(first_start..first_start + copied, first_start + filled);
        filled += copied;
    }
    for row in top + 1..bottom {
        let target = row * stride + left;
        canvas.copy_within(first_start..first_end, target);
    }
}

pub(super) fn blend_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    rect: (i32, i32, u32, u32),
    color: [u8; 4],
) {
    let (x, y, width, height) = rect;
    for dy in 0..height {
        for dx in 0..width {
            blend_pixel(
                canvas,
                canvas_width,
                canvas_height,
                x + i32::try_from(dx).expect("rectangle x fits i32"),
                y + i32::try_from(dy).expect("rectangle y fits i32"),
                color,
            );
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "glyph blending keeps explicit canvas, placement, color, and clip contracts"
)]
pub(super) fn blend_glyph(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: i32,
    y: i32,
    glyph: &CachedGlyph,
    foreground: [u8; 3],
    clip: Option<(i32, i32, i32, i32)>,
) {
    let pixel_count = usize::try_from(glyph.width)
        .expect("glyph width fits usize")
        .saturating_mul(usize::try_from(glyph.height).expect("glyph height fits usize"));
    let bytes_per_pixel = match glyph.content {
        Content::Mask => 1,
        Content::SubpixelMask | Content::Color => 4,
    };
    assert_eq!(
        glyph.data.len(),
        pixel_count.saturating_mul(bytes_per_pixel),
        "Swash glyph image has the expected byte count"
    );
    for gy in 0..glyph.height {
        for gx in 0..glyph.width {
            let pixel_index = usize::try_from(gy.saturating_mul(glyph.width) + gx)
                .expect("glyph pixel index fits usize");
            let source = match glyph.content {
                Content::Mask => [
                    foreground[0],
                    foreground[1],
                    foreground[2],
                    glyph.data[pixel_index],
                ],
                Content::Color => {
                    let offset = pixel_index * 4;
                    [
                        glyph.data[offset],
                        glyph.data[offset + 1],
                        glyph.data[offset + 2],
                        glyph.data[offset + 3],
                    ]
                }
                Content::SubpixelMask => {
                    let offset = pixel_index * 4;
                    let alpha = glyph.data[offset..offset + 4]
                        .iter()
                        .copied()
                        .max()
                        .unwrap_or(0);
                    [foreground[0], foreground[1], foreground[2], alpha]
                }
            };
            let target_x = x + i32::try_from(gx).expect("glyph x fits i32");
            let target_y = y + i32::try_from(gy).expect("glyph y fits i32");
            if clip.is_some_and(|(left, top, right, bottom)| {
                target_x < left || target_x >= right || target_y < top || target_y >= bottom
            }) {
                continue;
            }
            if glyph.content == Content::Color {
                blend_premultiplied_pixel(
                    canvas,
                    canvas_width,
                    canvas_height,
                    target_x,
                    target_y,
                    source,
                );
            } else {
                blend_pixel(
                    canvas,
                    canvas_width,
                    canvas_height,
                    target_x,
                    target_y,
                    source,
                );
            }
        }
    }
}

pub(super) fn pixel_index(width: u32, height: u32, x: i32, y: i32) -> Option<usize> {
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    if x >= width || y >= height {
        return None;
    }
    usize::try_from(y.checked_mul(width)?.checked_add(x)?)
        .ok()?
        .checked_mul(4)
}

fn pixman_multiply_unorm8(value: u8, alpha: u32) -> u32 {
    let product = u32::from(value) * alpha + 0x80;
    ((product >> 8) + product) >> 8
}

pub(super) fn alpha_u8(alpha: u16) -> u8 {
    u8::try_from(u32::from(alpha) * 255 / u32::from(u16::MAX)).expect("16-bit alpha maps to u8")
}

pub(super) fn premultiplied_rgba(rgb: [u8; 3], alpha: u8) -> [u8; 4] {
    [
        u8::try_from(pixman_multiply_unorm8(rgb[0], u32::from(alpha))).unwrap(),
        u8::try_from(pixman_multiply_unorm8(rgb[1], u32::from(alpha))).unwrap(),
        u8::try_from(pixman_multiply_unorm8(rgb[2], u32::from(alpha))).unwrap(),
        alpha,
    ]
}

pub(crate) fn background_bgra(rgb: [u8; 3], alpha: u16) -> [u8; 4] {
    let rgba = premultiplied_rgba(rgb, alpha_u8(alpha));
    [rgba[2], rgba[1], rgba[0], rgba[3]]
}

pub(super) fn blend_premultiplied_pixel(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rgba: [u8; 4],
) {
    let Some(index) = pixel_index(width, height, x, y) else {
        return;
    };
    let inverse = 255 - u32::from(rgba[3]);
    for (destination_channel, source_channel) in canvas[index..index + 3]
        .iter_mut()
        .zip([rgba[2], rgba[1], rgba[0]])
    {
        *destination_channel = u8::try_from(
            u32::from(source_channel)
                .saturating_add(pixman_multiply_unorm8(*destination_channel, inverse))
                .min(255),
        )
        .expect("premultiplied channel fits u8");
    }
    canvas[index + 3] = u8::try_from(
        u32::from(rgba[3])
            .saturating_add(pixman_multiply_unorm8(canvas[index + 3], inverse))
            .min(255),
    )
    .expect("composited alpha fits u8");
}

pub(super) fn blend_pixel(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rgba: [u8; 4],
) {
    let Some(index) = pixel_index(width, height, x, y) else {
        return;
    };
    let alpha = u32::from(rgba[3]);
    let inverse = 255 - alpha;
    for (destination_channel, source_channel) in canvas[index..index + 3]
        .iter_mut()
        .zip([rgba[2], rgba[1], rgba[0]])
    {
        *destination_channel = u8::try_from(
            pixman_multiply_unorm8(source_channel, alpha)
                .saturating_add(pixman_multiply_unorm8(*destination_channel, inverse))
                .min(255),
        )
        .expect("blended channel fits u8");
    }
    canvas[index + 3] = u8::try_from(
        alpha
            .saturating_add(pixman_multiply_unorm8(canvas[index + 3], inverse))
            .min(255),
    )
    .expect("composited alpha fits u8");
}

#[allow(
    clippy::too_many_arguments,
    reason = "canvas bounds, glyph cell, clip, color, and scale are independent raster inputs"
)]
pub(crate) fn paint_box_drawing_cell(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    character: char,
    cell: Rect,
    clip: Rect,
    color: u32,
    scale_120: u16,
) {
    let thickness = box_drawing::default_thickness(cell.width, cell.height, scale_120);
    let Some(mask) = box_drawing::generate(character, cell.width, cell.height, thickness) else {
        return;
    };
    let red = u8::try_from(color >> 16 & 0xff).expect("red channel fits");
    let green = u8::try_from(color >> 8 & 0xff).expect("green channel fits");
    let blue = u8::try_from(color & 0xff).expect("blue channel fits");
    for y in 0..mask.height {
        for x in 0..mask.width {
            let index = usize::try_from(y * mask.width + x).expect("box mask index fits");
            let coverage = mask.data[index];
            if coverage == 0 {
                continue;
            }
            let Some(x) = cell.x.checked_add(x) else {
                continue;
            };
            let Some(y) = cell.y.checked_add(y) else {
                continue;
            };
            if x < clip.x
                || y < clip.y
                || x >= clip.x.saturating_add(clip.width)
                || y >= clip.y.saturating_add(clip.height)
            {
                continue;
            }
            let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
                continue;
            };
            blend_pixel(
                canvas,
                canvas_width,
                canvas_height,
                x,
                y,
                [red, green, blue, coverage],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_clip_preserves_inside_overhang_and_rejects_outside_pixels() {
        let glyph = CachedGlyph {
            content: Content::Mask,
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            data: vec![255, 255],
        };
        let mut canvas = vec![0; 2 * 4];
        blend_glyph(
            &mut canvas,
            2,
            1,
            0,
            0,
            &glyph,
            [255, 255, 255],
            Some((0, 0, 1, 1)),
        );
        assert_eq!(&canvas[..4], &[255, 255, 255, 255]);
        assert_eq!(&canvas[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn premultiplied_color_over_does_not_multiply_source_twice() {
        let mut canvas = vec![10, 20, 30, 255];
        blend_premultiplied_pixel(&mut canvas, 1, 1, 0, 0, [40, 20, 10, 128]);
        assert_eq!(canvas, [15, 30, 55, 255]);
    }
}
