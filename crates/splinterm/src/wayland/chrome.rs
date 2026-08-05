//! Trusted consent and pane chrome painting.

use std::collections::HashMap;

use anyhow::{Context, Result};
use unicode_width::UnicodeWidthChar;

use crate::{
    config::ResolvedTheme,
    geometry::Rect,
    pane::{PaneChrome, PaneDivider, PaneLayout},
    renderer::{background_bgra, paint_box_drawing_cell},
};
use splinterm_core::SplintId;

use super::{App, CachedFrameTitle};

pub(super) fn paint_trusted_consent_chrome(canvas: &mut [u8], width: u32, height: u32) {
    fn fill(canvas: &mut [u8], width: u32, x0: u32, y0: u32, x1: u32, y1: u32, rgb: u32) {
        let [_, red, green, blue] = rgb.to_be_bytes();
        for y in y0.min(y1)..y1 {
            for x in x0.min(x1)..x1 {
                let Ok(index) = usize::try_from((y * width + x) * 4) else {
                    continue;
                };
                if let Some(pixel) = canvas.get_mut(index..index + 4) {
                    pixel.copy_from_slice(&[blue, green, red, 0xff]);
                }
            }
        }
    }
    let border = width.min(height).div_ceil(80).max(4);
    fill(canvas, width, 0, 0, width, border, 0x00e0_a030);
    fill(
        canvas,
        width,
        0,
        height.saturating_sub(border),
        width,
        height,
        0x00e0_a030,
    );
    fill(canvas, width, 0, 0, border, height, 0x00e0_a030);
    fill(
        canvas,
        width,
        width.saturating_sub(border),
        0,
        width,
        height,
        0x00e0_a030,
    );
    let button_top = height.saturating_mul(78) / 100;
    let middle = width / 2;
    fill(
        canvas,
        width,
        border,
        button_top,
        middle.saturating_sub(2),
        height.saturating_sub(border),
        0x0070_2020,
    );
    fill(
        canvas,
        width,
        middle.saturating_add(2),
        button_top,
        width.saturating_sub(border),
        height.saturating_sub(border),
        0x0020_7040,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "bounded box cells require explicit canvas, clip, metrics, color, scale, and direction"
)]
fn paint_box_sequence(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    character: char,
    clip: Rect,
    cell_width: u32,
    cell_height: u32,
    color: u32,
    scale_120: u16,
    horizontal: bool,
) {
    let step = if horizontal { cell_width } else { cell_height };
    if step == 0 {
        return;
    }
    let end = if horizontal {
        clip.x.saturating_add(clip.width)
    } else {
        clip.y.saturating_add(clip.height)
    };
    let mut position = if horizontal { clip.x } else { clip.y };
    while position < end {
        let cell = if horizontal {
            Rect {
                x: position,
                y: clip.y,
                width: cell_width,
                height: cell_height,
            }
        } else {
            Rect {
                x: clip.x,
                y: position,
                width: cell_width,
                height: cell_height,
            }
        };
        paint_box_drawing_cell(
            canvas, width, height, character, cell, clip, color, scale_120,
        );
        position = position.saturating_add(step);
    }
}

pub(super) fn sanitize_frame_title(title: &str, maximum_cells: u32) -> String {
    let mut output = String::new();
    let mut cells = 0_u32;
    let mut previous_space = false;
    for character in title.chars() {
        let character = if character.is_control() || character.is_whitespace() {
            ' '
        } else {
            character
        };
        if character == ' ' && (previous_space || output.is_empty()) {
            continue;
        }
        let width = u32::try_from(character.width().unwrap_or(0)).unwrap_or(0);
        if width > 0 && cells.saturating_add(width) > maximum_cells {
            break;
        }
        output.push(character);
        cells = cells.saturating_add(width);
        previous_space = character == ' ';
    }
    output.trim_end().to_owned()
}

fn fill_chrome_background(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    rect: Rect,
    color: u32,
    background_alpha: u16,
) {
    let [_, red, green, blue] = color.to_be_bytes();
    let pixel = background_bgra([red, green, blue], background_alpha);
    let right = rect.x.saturating_add(rect.width).min(width);
    let bottom = rect.y.saturating_add(rect.height).min(height);
    for y in rect.y.min(height)..bottom {
        for x in rect.x.min(width)..right {
            let Ok(index) = usize::try_from((y * width + x) * 4) else {
                continue;
            };
            if let Some(target) = canvas.get_mut(index..index + 4) {
                target.copy_from_slice(&pixel);
            }
        }
    }
}

fn divider_touches_pane(divider: PaneDivider, pane: Rect) -> bool {
    let divider_right = divider.rect.x.saturating_add(divider.rect.width);
    let divider_bottom = divider.rect.y.saturating_add(divider.rect.height);
    let pane_right = pane.x.saturating_add(pane.width);
    let pane_bottom = pane.y.saturating_add(pane.height);
    match divider.axis {
        splinterm_core::Axis::Horizontal => {
            (pane_right == divider.rect.x || pane.x == divider_right)
                && pane.y < divider_bottom
                && divider.rect.y < pane_bottom
        }
        splinterm_core::Axis::Vertical => {
            (pane_bottom == divider.rect.y || pane.y == divider_bottom)
                && pane.x < divider_right
                && divider.rect.x < pane_right
        }
    }
}

pub(super) fn divider_junction(first: PaneDivider, second: PaneDivider) -> Option<(char, Rect)> {
    let (vertical, horizontal) = match (first.axis, second.axis) {
        (splinterm_core::Axis::Horizontal, splinterm_core::Axis::Vertical) => (first, second),
        (splinterm_core::Axis::Vertical, splinterm_core::Axis::Horizontal) => (second, first),
        _ => return None,
    };
    let vertical_right = vertical.rect.x.checked_add(vertical.rect.width)?;
    let vertical_bottom = vertical.rect.y.checked_add(vertical.rect.height)?;
    let horizontal_right = horizontal.rect.x.checked_add(horizontal.rect.width)?;
    let horizontal_bottom = horizontal.rect.y.checked_add(horizontal.rect.height)?;
    if horizontal_right == vertical.rect.x
        && horizontal.rect.y < vertical_bottom
        && vertical.rect.y < horizontal_bottom
    {
        return Some((
            '┤',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    if horizontal.rect.x == vertical_right
        && horizontal.rect.y < vertical_bottom
        && vertical.rect.y < horizontal_bottom
    {
        return Some((
            '├',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    if vertical_bottom == horizontal.rect.y
        && vertical.rect.x < horizontal_right
        && horizontal.rect.x < vertical_right
    {
        return Some((
            '┴',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    if vertical.rect.y == horizontal_bottom
        && vertical.rect.x < horizontal_right
        && horizontal.rect.x < vertical_right
    {
        return Some((
            '┬',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    None
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "line junctions and complete framed panels share one trusted clipped chrome pass"
)]
pub(super) fn paint_pane_chrome(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    layout: &PaneLayout,
    active_splint: Option<SplintId>,
    theme: ResolvedTheme,
    cell_width: u32,
    cell_height: u32,
    scale_120: u32,
    frame_titles: &HashMap<SplintId, CachedFrameTitle>,
) -> Result<()> {
    let scale = u16::try_from(scale_120).context("pane chrome scale fits u16")?;
    match layout.chrome {
        PaneChrome::None => {}
        PaneChrome::Line { .. } => {
            let active_rect = active_splint.and_then(|id| layout.rect(id));
            for divider in &layout.separators {
                let clip = App::buffer_rect(divider.rect, scale_120)?;
                let active = active_rect.is_some_and(|pane| divider_touches_pane(*divider, pane));
                let color = if active {
                    theme.pane_border_active
                } else {
                    theme.pane_border
                };
                let horizontal = divider.axis == splinterm_core::Axis::Vertical;
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    if horizontal { '─' } else { '│' },
                    clip,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    horizontal,
                );
            }
            for (index, first) in layout.separators.iter().copied().enumerate() {
                for second in layout.separators[index + 1..].iter().copied() {
                    let Some((character, logical_cell)) = divider_junction(first, second) else {
                        continue;
                    };
                    let clip = App::buffer_rect(logical_cell, scale_120)?;
                    let active = active_rect.is_some_and(|pane| {
                        divider_touches_pane(first, pane) || divider_touches_pane(second, pane)
                    });
                    let color = if active {
                        theme.pane_border_active
                    } else {
                        theme.pane_border
                    };
                    paint_box_drawing_cell(
                        canvas,
                        width,
                        height,
                        character,
                        Rect {
                            x: clip.x,
                            y: clip.y,
                            width: cell_width,
                            height: cell_height,
                        },
                        clip,
                        color,
                        scale,
                    );
                }
            }
        }
        PaneChrome::Frame { .. } => {
            for pane in &layout.panes {
                let allocation = App::buffer_rect(pane.allocation, scale_120)?;
                let content = App::buffer_rect(pane.rect, scale_120)?;
                let right = allocation.x.saturating_add(allocation.width);
                let bottom = allocation.y.saturating_add(allocation.height);
                let content_right = content.x.saturating_add(content.width);
                let content_bottom = content.y.saturating_add(content.height);
                let color = if Some(pane.splint_id) == active_splint {
                    theme.pane_border_active
                } else {
                    theme.pane_border
                };
                let top = Rect {
                    x: allocation.x.saturating_add(cell_width),
                    y: allocation.y,
                    width: allocation
                        .width
                        .saturating_sub(cell_width.saturating_mul(2)),
                    height: content.y.saturating_sub(allocation.y),
                };
                let bottom_edge = Rect {
                    x: allocation.x.saturating_add(cell_width),
                    y: content_bottom,
                    width: allocation
                        .width
                        .saturating_sub(cell_width.saturating_mul(2)),
                    height: bottom.saturating_sub(content_bottom),
                };
                let left = Rect {
                    x: allocation.x,
                    y: allocation.y.saturating_add(cell_height),
                    width: content.x.saturating_sub(allocation.x),
                    height: allocation
                        .height
                        .saturating_sub(cell_height.saturating_mul(2)),
                };
                let right_edge = Rect {
                    x: content_right,
                    y: allocation.y.saturating_add(cell_height),
                    width: right.saturating_sub(content_right),
                    height: allocation
                        .height
                        .saturating_sub(cell_height.saturating_mul(2)),
                };
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '─',
                    top,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    true,
                );
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '─',
                    bottom_edge,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    true,
                );
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '│',
                    left,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    false,
                );
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '│',
                    right_edge,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    false,
                );
                let top_left = Rect {
                    x: allocation.x,
                    y: allocation.y,
                    width: cell_width,
                    height: cell_height,
                };
                let top_right = Rect {
                    x: right.saturating_sub(cell_width),
                    y: allocation.y,
                    width: cell_width,
                    height: cell_height,
                };
                let bottom_left = Rect {
                    x: allocation.x,
                    y: bottom.saturating_sub(cell_height),
                    width: cell_width,
                    height: cell_height,
                };
                let bottom_right = Rect {
                    x: right.saturating_sub(cell_width),
                    y: bottom.saturating_sub(cell_height),
                    width: cell_width,
                    height: cell_height,
                };
                for (character, cell) in [
                    ('┌', top_left),
                    ('┐', top_right),
                    ('└', bottom_left),
                    ('┘', bottom_right),
                ] {
                    let clip = cell;
                    paint_box_drawing_cell(
                        canvas, width, height, character, cell, clip, color, scale,
                    );
                }
                if let Some(title) = frame_titles.get(&pane.splint_id) {
                    let clear = Rect {
                        x: allocation.x.saturating_add(cell_width.saturating_mul(2)),
                        y: allocation.y,
                        width: title
                            .text
                            .cells()
                            .saturating_add(2)
                            .saturating_mul(cell_width),
                        height: cell_height,
                    };
                    fill_chrome_background(
                        canvas,
                        width,
                        height,
                        clear,
                        theme.background,
                        theme.background_alpha,
                    );
                    title.text.paint(
                        canvas,
                        width,
                        height,
                        (
                            allocation.x.saturating_add(cell_width.saturating_mul(3)),
                            allocation.y,
                        ),
                        clear,
                        color,
                    );
                }
            }
        }
    }
    Ok(())
}
