//! Snapshot image resolution, ordering, sampling, and composition.

use std::collections::HashMap;

use anyhow::{Context, Result};
use splinterm_automation_client::{ImageContentLeaseSet, ImageContentSource};
use splinterm_protocol::{ImageContentMetadata, ImagePlacement, TerminalSnapshot};

use crate::geometry::{Rect, WindowGeometry};

use super::{SnapshotFrame, raster::blend_premultiplied_pixel};

pub(super) const KITTY_BACKGROUND_Z_THRESHOLD: i32 = -1_073_741_824;

#[derive(Clone, Debug)]
pub(super) struct SnapshotImage {
    pub(super) metadata: ImageContentMetadata,
    pub(super) placement: ImagePlacement,
    pub(super) row: u32,
    pub(super) source: ImageContentSource,
}

pub(super) fn image_tier(z_index: i32) -> u8 {
    if z_index < KITTY_BACKGROUND_Z_THRESHOLD {
        0
    } else if z_index < 0 {
        1
    } else {
        2
    }
}

pub(super) fn compare_snapshot_images(
    left: &SnapshotImage,
    right: &SnapshotImage,
) -> std::cmp::Ordering {
    image_tier(left.placement.z_index)
        .cmp(&image_tier(right.placement.z_index))
        .then_with(|| left.placement.z_index.cmp(&right.placement.z_index))
        .then_with(|| {
            match (
                left.placement.application_image_id,
                right.placement.application_image_id,
            ) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left
                    .placement
                    .creation_order
                    .cmp(&right.placement.creation_order),
            }
        })
        .then_with(|| {
            left.placement
                .creation_order
                .cmp(&right.placement.creation_order)
        })
}

pub(super) fn prepare_snapshot_images(
    snapshot: &TerminalSnapshot,
    sources: Option<&ImageContentLeaseSet>,
) -> Result<Vec<SnapshotImage>> {
    let Some(plane) = &snapshot.images else {
        return Ok(Vec::new());
    };
    let sources = sources.context("snapshot image sources are unavailable")?;
    let contents = plane
        .contents
        .iter()
        .map(|content| (content.content_id, content))
        .collect::<HashMap<_, _>>();
    let rows = snapshot
        .visible_rows
        .iter()
        .enumerate()
        .filter_map(|(row, value)| value.row_id.map(|id| (id, row)))
        .collect::<HashMap<_, _>>();
    let mut images = Vec::with_capacity(plane.placements.len());
    for placement in &plane.placements {
        let Some(row) = rows.get(&placement.row_id).copied() else {
            continue;
        };
        let metadata = contents
            .get(&placement.content_id)
            .context("image placement references missing content metadata")?;
        let source = sources
            .get(metadata)
            .context("snapshot references an unresolved image source")?;
        images.push(SnapshotImage {
            metadata: (*metadata).clone(),
            placement: placement.clone(),
            row: u32::try_from(row).context("image anchor row fits u32")?,
            source,
        });
    }
    images.sort_by(compare_snapshot_images);
    Ok(images)
}

fn scaled_image_offset(offset: i32, destination_cell: u32, source_cell: Option<u32>) -> i64 {
    let source_cell = source_cell
        .filter(|size| *size > 0)
        .unwrap_or(destination_cell);
    i64::from(offset) * i64::from(destination_cell) / i64::from(source_cell)
}

const IMAGE_FILTER_ONE: u64 = 1 << 16;
const IMAGE_FILTER_HALF: u64 = IMAGE_FILTER_ONE / 2;
type BilinearAxis = (u32, u32, u64);

fn bilinear_axis_from_center(center: u64, source_extent: u32) -> Option<BilinearAxis> {
    let maximum = u64::from(source_extent.checked_sub(1)?) * IMAGE_FILTER_ONE;
    let position = center.saturating_sub(IMAGE_FILTER_HALF).min(maximum);
    let lower = u32::try_from(position / IMAGE_FILTER_ONE).unwrap_or(source_extent - 1);
    let upper = lower.saturating_add(1).min(source_extent - 1);
    Some((lower, upper, position % IMAGE_FILTER_ONE))
}

fn bilinear_axis(
    destination_coordinate: u64,
    source_extent: u32,
    destination_extent: u32,
) -> Option<BilinearAxis> {
    if source_extent == 0 || destination_extent == 0 {
        return None;
    }
    let center = u64::try_from(
        (u128::from(destination_coordinate) * 2 + 1)
            * u128::from(source_extent)
            * u128::from(IMAGE_FILTER_ONE)
            / (u128::from(destination_extent) * 2),
    )
    .ok()?;
    bilinear_axis_from_center(center, source_extent)
}

struct BilinearAxisStepper {
    center: u64,
    remainder: u128,
    denominator: u128,
    step: u64,
    step_remainder: u128,
    source_extent: u32,
}

impl BilinearAxisStepper {
    fn new(
        destination_coordinate: u64,
        source_extent: u32,
        destination_extent: u32,
    ) -> Option<Self> {
        if source_extent == 0 || destination_extent == 0 {
            return None;
        }
        let denominator = u128::from(destination_extent) * 2;
        let numerator = (u128::from(destination_coordinate) * 2 + 1)
            * u128::from(source_extent)
            * u128::from(IMAGE_FILTER_ONE);
        let increment = u128::from(source_extent) * u128::from(IMAGE_FILTER_ONE) * 2;
        Some(Self {
            center: u64::try_from(numerator / denominator).ok()?,
            remainder: numerator % denominator,
            denominator,
            step: u64::try_from(increment / denominator).ok()?,
            step_remainder: increment % denominator,
            source_extent,
        })
    }
}

impl Iterator for BilinearAxisStepper {
    type Item = BilinearAxis;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = bilinear_axis_from_center(self.center, self.source_extent)?;
        self.center = self.center.checked_add(self.step)?;
        self.remainder += self.step_remainder;
        if self.remainder >= self.denominator {
            self.remainder -= self.denominator;
            self.center = self.center.checked_add(1)?;
        }
        Some(sample)
    }
}

fn image_source_pixel(source: &[u8], source_stride: usize, x: u32, y: u32) -> Option<&[u8]> {
    usize::try_from(y)
        .ok()
        .and_then(|row| row.checked_mul(source_stride))
        .and_then(|row| {
            usize::try_from(x)
                .ok()
                .and_then(|column| column.checked_mul(4))
                .and_then(|column| row.checked_add(column))
        })
        .and_then(|index| source.get(index..index + 4))
}

fn bilinear_premultiplied_bgra_from_axes(
    source: &[u8],
    source_stride: usize,
    crop: splinterm_protocol::ImagePixelRect,
    x_axis: BilinearAxis,
    y_axis: BilinearAxis,
) -> Option<[u8; 4]> {
    let (x0, x1, x_weight) = x_axis;
    let (y0, y1, y_weight) = y_axis;
    let x0 = crop.x.checked_add(x0)?;
    let x1 = crop.x.checked_add(x1)?;
    let y0 = crop.y.checked_add(y0)?;
    let y1 = crop.y.checked_add(y1)?;
    let top_left = image_source_pixel(source, source_stride, x0, y0)?;
    let top_right = image_source_pixel(source, source_stride, x1, y0)?;
    let bottom_left = image_source_pixel(source, source_stride, x0, y1)?;
    let bottom_right = image_source_pixel(source, source_stride, x1, y1)?;
    let x_inverse = IMAGE_FILTER_ONE - x_weight;
    let y_inverse = IMAGE_FILTER_ONE - y_weight;
    let weights = [
        x_inverse * y_inverse,
        x_weight * y_inverse,
        x_inverse * y_weight,
        x_weight * y_weight,
    ];
    let mut result = [0; 4];
    for (channel, output) in result.iter_mut().enumerate() {
        let value = u64::from(top_left[channel]) * weights[0]
            + u64::from(top_right[channel]) * weights[1]
            + u64::from(bottom_left[channel]) * weights[2]
            + u64::from(bottom_right[channel]) * weights[3];
        *output = u8::try_from((value + (1 << 31)) >> 32).unwrap_or(u8::MAX);
    }
    Some(result)
}

#[cfg(test)]
fn bilinear_premultiplied_bgra(
    source: &[u8],
    source_stride: usize,
    crop: splinterm_protocol::ImagePixelRect,
    destination_x: u64,
    destination_y: u64,
    destination_width: u32,
    destination_height: u32,
) -> Option<[u8; 4]> {
    bilinear_premultiplied_bgra_from_axes(
        source,
        source_stride,
        crop,
        bilinear_axis(destination_x, crop.width, destination_width)?,
        bilinear_axis(destination_y, crop.height, destination_height)?,
    )
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "image composition keeps bounded source, destination, pane, tier, and damage arithmetic explicit"
)]
pub(super) fn paint_snapshot_images(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    region: Rect,
    dirty_rows: Option<&[bool]>,
    tier: u8,
) {
    let grid = geometry.visible_grid_rect;
    let clip_left = i64::from(region.x.max(grid.x));
    let clip_top = i64::from(region.y.max(grid.y));
    let clip_right = i64::from(
        region
            .x
            .saturating_add(region.width)
            .min(grid.x.saturating_add(grid.width))
            .min(width),
    );
    let clip_bottom = i64::from(
        region
            .y
            .saturating_add(region.height)
            .min(grid.y.saturating_add(grid.height))
            .min(height),
    );
    if clip_left >= clip_right || clip_top >= clip_bottom {
        return;
    }
    for image in frame
        .images
        .iter()
        .filter(|image| image_tier(image.placement.z_index) == tier)
    {
        let Some((anchor_x, anchor_y, cell_width, cell_height)) = frame.cell_rect(
            geometry,
            u32::try_from(image.placement.column).unwrap_or(u32::MAX),
            image.row,
        ) else {
            continue;
        };
        let cell_destination = u32::try_from(image.placement.destination_columns)
            .ok()
            .and_then(|columns| columns.checked_mul(cell_width))
            .zip(
                u32::try_from(image.placement.destination_rows)
                    .ok()
                    .and_then(|rows| rows.checked_mul(cell_height)),
            );
        let destination =
            if image.metadata.source_format == splinterm_protocol::ImageSourceFormat::Sixel {
                image
                    .placement
                    .source_cell_size
                    .and_then(|source_cell| {
                        let width = u64::from(image.placement.source.width)
                            .checked_mul(u64::from(cell_width))?
                            .div_ceil(u64::from(source_cell.width));
                        let height = u64::from(image.placement.source.height)
                            .checked_mul(u64::from(cell_height))?
                            .div_ceil(u64::from(source_cell.height));
                        Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
                    })
                    .or(cell_destination)
            } else {
                cell_destination
            };
        let Some((destination_width, destination_height)) = destination else {
            continue;
        };
        if destination_width == 0 || destination_height == 0 {
            continue;
        }
        let destination_x = i64::from(anchor_x)
            + scaled_image_offset(
                image.placement.x_offset,
                cell_width,
                image.placement.source_cell_size.map(|size| size.width),
            );
        let destination_y = i64::from(anchor_y)
            + scaled_image_offset(
                image.placement.y_offset,
                cell_height,
                image.placement.source_cell_size.map(|size| size.height),
            );
        let left = destination_x.max(clip_left);
        let top = destination_y.max(clip_top);
        let right = destination_x
            .saturating_add(i64::from(destination_width))
            .min(clip_right);
        let bottom = destination_y
            .saturating_add(i64::from(destination_height))
            .min(clip_bottom);
        if left >= right || top >= bottom {
            continue;
        }
        let source = image.source.as_bytes();
        let source_stride = usize::try_from(image.metadata.width)
            .ok()
            .and_then(|width| width.checked_mul(4));
        let Some(source_stride) = source_stride else {
            continue;
        };
        let identity_scale = image.placement.source.width == destination_width
            && image.placement.source.height == destination_height;
        for target_y in top..bottom {
            let grid_row = usize::try_from((target_y - i64::from(grid.y)) / i64::from(cell_height))
                .unwrap_or(usize::MAX);
            if dirty_rows.is_some_and(|rows| !rows.get(grid_row).copied().unwrap_or(false)) {
                continue;
            }
            let relative_y = u64::try_from(target_y - destination_y).unwrap_or(0);
            let y_axis = (!identity_scale)
                .then(|| {
                    bilinear_axis(
                        relative_y,
                        image.placement.source.height,
                        destination_height,
                    )
                })
                .flatten();
            let relative_left = u64::try_from(left - destination_x).unwrap_or(0);
            let mut x_axis = (!identity_scale)
                .then(|| {
                    BilinearAxisStepper::new(
                        relative_left,
                        image.placement.source.width,
                        destination_width,
                    )
                })
                .flatten();
            for target_x in left..right {
                let relative_x = u64::try_from(target_x - destination_x).unwrap_or(0);
                let pixel = if identity_scale {
                    u32::try_from(relative_x)
                        .ok()
                        .and_then(|x| image.placement.source.x.checked_add(x))
                        .and_then(|x| {
                            u32::try_from(relative_y)
                                .ok()
                                .and_then(|y| image.placement.source.y.checked_add(y))
                                .and_then(|y| image_source_pixel(source, source_stride, x, y))
                        })
                        .and_then(|pixel| pixel.try_into().ok())
                } else {
                    x_axis.as_mut().and_then(Iterator::next).and_then(|x| {
                        bilinear_premultiplied_bgra_from_axes(
                            source,
                            source_stride,
                            image.placement.source,
                            x,
                            y_axis?,
                        )
                    })
                };
                let Some(pixel) = pixel else {
                    continue;
                };
                blend_premultiplied_pixel(
                    canvas,
                    width,
                    height,
                    i32::try_from(target_x).unwrap_or(i32::MAX),
                    i32::try_from(target_y).unwrap_or(i32::MAX),
                    [pixel[2], pixel[1], pixel[0], pixel[3]],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_image_sampling_is_exact_clamped_and_premultiplied() {
        let full = splinterm_protocol::ImagePixelRect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        let identity = [
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
        ];
        for y in 0..2 {
            for x in 0..2 {
                let index = usize::try_from((y * 2 + x) * 4).unwrap();
                assert_eq!(
                    bilinear_premultiplied_bgra(&identity, 8, full, x, y, 2, 2),
                    Some(identity[index..index + 4].try_into().unwrap())
                );
            }
        }
        let two_dimensional = [
            0, 0, 0, 255, 64, 64, 64, 255, 128, 128, 128, 255, 255, 255, 255, 255,
        ];
        assert_eq!(
            bilinear_premultiplied_bgra(&two_dimensional, 8, full, 1, 1, 3, 3),
            Some([112, 112, 112, 255])
        );

        let mut stepped = BilinearAxisStepper::new(3, 7, 11).unwrap();
        for coordinate in 3..11 {
            assert_eq!(stepped.next(), bilinear_axis(coordinate, 7, 11));
        }

        let opaque_ramp = [0, 0, 0, 255, 200, 200, 200, 255];
        let row = splinterm_protocol::ImagePixelRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let upscaled = (0..4)
            .map(|x| bilinear_premultiplied_bgra(&opaque_ramp, 8, row, x, 0, 4, 1).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            upscaled,
            [
                [0, 0, 0, 255],
                [50, 50, 50, 255],
                [150, 150, 150, 255],
                [200, 200, 200, 255],
            ]
        );

        let downscale = [
            0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255, 255, 255, 255, 255,
        ];
        let wide = splinterm_protocol::ImagePixelRect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        assert_eq!(
            bilinear_premultiplied_bgra(&downscale, 16, wide, 0, 0, 2, 1),
            Some([50, 50, 50, 255])
        );
        assert_eq!(
            bilinear_premultiplied_bgra(&downscale, 16, wide, 1, 0, 2, 1),
            Some([228, 228, 228, 255])
        );

        let crop_source = [
            10, 10, 10, 255, 20, 20, 20, 255, 220, 220, 220, 255, 240, 240, 240, 255,
        ];
        let crop = splinterm_protocol::ImagePixelRect {
            x: 1,
            y: 0,
            width: 2,
            height: 1,
        };
        let cropped = (0..4)
            .map(|x| bilinear_premultiplied_bgra(&crop_source, 16, crop, x, 0, 4, 1).unwrap()[0])
            .collect::<Vec<_>>();
        assert_eq!(cropped, [20, 70, 170, 220]);

        let alpha = [0, 0, 0, 0, 0, 0, 128, 128];
        assert_eq!(
            bilinear_premultiplied_bgra(&alpha, 8, row, 1, 0, 4, 1),
            Some([0, 0, 32, 32])
        );
        let empty = splinterm_protocol::ImagePixelRect { width: 0, ..row };
        assert_eq!(
            bilinear_premultiplied_bgra(&alpha, 8, empty, 0, 0, 1, 1),
            None
        );
    }
}
