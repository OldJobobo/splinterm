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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DividerSide {
    First,
    Second,
}

fn divider_active_segment(
    divider: PaneDivider,
    pane: Rect,
    split_shared_border: bool,
) -> Option<Rect> {
    let divider_right = divider.rect.x.checked_add(divider.rect.width)?;
    let divider_bottom = divider.rect.y.checked_add(divider.rect.height)?;
    let pane_right = pane.x.checked_add(pane.width)?;
    let pane_bottom = pane.y.checked_add(pane.height)?;
    let (side, mut segment) = match divider.axis {
        splinterm_core::Axis::Horizontal => {
            let side = if pane_right == divider.rect.x {
                DividerSide::First
            } else if pane.x == divider_right {
                DividerSide::Second
            } else {
                return None;
            };
            let top = pane.y.max(divider.rect.y);
            let bottom = pane_bottom.min(divider_bottom);
            (bottom > top).then_some((
                side,
                Rect {
                    y: top,
                    height: bottom - top,
                    ..divider.rect
                },
            ))?
        }
        splinterm_core::Axis::Vertical => {
            let side = if pane_bottom == divider.rect.y {
                DividerSide::First
            } else if pane.y == divider_bottom {
                DividerSide::Second
            } else {
                return None;
            };
            let left = pane.x.max(divider.rect.x);
            let right = pane_right.min(divider_right);
            (right > left).then_some((
                side,
                Rect {
                    x: left,
                    width: right - left,
                    ..divider.rect
                },
            ))?
        }
    };

    // A single shared border touches both leaves over its complete length. tmux
    // disambiguates that case by assigning the leading half to the first
    // (left/top) pane and the trailing half to the second (right/bottom) pane.
    if split_shared_border && segment == divider.rect {
        match divider.axis {
            splinterm_core::Axis::Horizontal => {
                let leading = segment.height / 2 + segment.height % 2;
                if side == DividerSide::First {
                    segment.height = leading;
                } else {
                    segment.y = segment.y.checked_add(leading)?;
                    segment.height = segment.height.checked_sub(leading)?;
                }
            }
            splinterm_core::Axis::Vertical => {
                let leading = segment.width / 2 + segment.width % 2;
                if side == DividerSide::First {
                    segment.width = leading;
                } else {
                    segment.x = segment.x.checked_add(leading)?;
                    segment.width = segment.width.checked_sub(leading)?;
                }
            }
        }
    }
    (segment.width > 0 && segment.height > 0).then_some(segment)
}

fn active_junction_arm_clips(
    divider: PaneDivider,
    active_segment: Rect,
    junction: Rect,
) -> Vec<(char, Rect)> {
    let mut arms = Vec::with_capacity(2);
    let active_right = active_segment.x.saturating_add(active_segment.width);
    let active_bottom = active_segment.y.saturating_add(active_segment.height);
    let junction_right = junction.x.saturating_add(junction.width);
    let junction_bottom = junction.y.saturating_add(junction.height);
    match divider.axis {
        splinterm_core::Axis::Horizontal => {
            let half = junction.height / 2;
            if active_segment.y < junction.y && active_bottom >= junction.y {
                arms.push((
                    '│',
                    Rect {
                        height: half + junction.height % 2,
                        ..junction
                    },
                ));
            }
            if active_segment.y <= junction_bottom && active_bottom > junction_bottom {
                arms.push((
                    '│',
                    Rect {
                        y: junction.y.saturating_add(half),
                        height: junction.height.saturating_sub(half),
                        ..junction
                    },
                ));
            }
        }
        splinterm_core::Axis::Vertical => {
            let half = junction.width / 2;
            if active_segment.x < junction.x && active_right >= junction.x {
                arms.push((
                    '─',
                    Rect {
                        width: half + junction.width % 2,
                        ..junction
                    },
                ));
            }
            if active_segment.x <= junction_right && active_right > junction_right {
                arms.push((
                    '─',
                    Rect {
                        x: junction.x.saturating_add(half),
                        width: junction.width.saturating_sub(half),
                        ..junction
                    },
                ));
            }
        }
    }
    arms
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
            let split_shared_border = layout.panes.len() == 2;

            // Establish the complete inactive network first. Active segments and
            // junction arms are overlays so focus never erases connectivity.
            for divider in &layout.separators {
                let clip = App::buffer_rect(divider.rect, scale_120)?;
                let horizontal = divider.axis == splinterm_core::Axis::Vertical;
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    if horizontal { '─' } else { '│' },
                    clip,
                    cell_width,
                    cell_height,
                    theme.pane_border,
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
                        theme.pane_border,
                        scale,
                    );
                }
            }

            if let Some(pane) = active_rect {
                for divider in &layout.separators {
                    let Some(segment) = divider_active_segment(*divider, pane, split_shared_border)
                    else {
                        continue;
                    };
                    let clip = App::buffer_rect(segment, scale_120)?;
                    let horizontal = divider.axis == splinterm_core::Axis::Vertical;
                    paint_box_sequence(
                        canvas,
                        width,
                        height,
                        if horizontal { '─' } else { '│' },
                        clip,
                        cell_width,
                        cell_height,
                        theme.pane_border_active,
                        scale,
                        horizontal,
                    );
                }

                // A junction can border more than one pane. Overlay each active
                // arm independently instead of promoting the complete tee.
                for (index, first) in layout.separators.iter().copied().enumerate() {
                    for second in layout.separators[index + 1..].iter().copied() {
                        let Some((_, logical_cell)) = divider_junction(first, second) else {
                            continue;
                        };
                        let cell = App::buffer_rect(logical_cell, scale_120)?;
                        for divider in [first, second] {
                            let Some(segment) =
                                divider_active_segment(divider, pane, split_shared_border)
                            else {
                                continue;
                            };
                            for (character, logical_clip) in
                                active_junction_arm_clips(divider, segment, logical_cell)
                            {
                                let clip = App::buffer_rect(logical_clip, scale_120)?;
                                paint_box_drawing_cell(
                                    canvas,
                                    width,
                                    height,
                                    character,
                                    Rect {
                                        x: cell.x,
                                        y: cell.y,
                                        width: cell_width,
                                        height: cell_height,
                                    },
                                    clip,
                                    theme.pane_border_active,
                                    scale,
                                );
                            }
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneGeometry;
    use splinterm_core::Axis;

    #[test]
    fn two_pane_active_dividers_use_tmux_directional_halves() {
        let vertical = PaneDivider {
            axis: Axis::Horizontal,
            rect: Rect {
                x: 40,
                y: 0,
                width: 8,
                height: 100,
            },
        };
        let left = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 100,
        };
        let right = Rect {
            x: 48,
            y: 0,
            width: 52,
            height: 100,
        };
        assert_eq!(
            divider_active_segment(vertical, left, true),
            Some(Rect {
                height: 50,
                ..vertical.rect
            })
        );
        assert_eq!(
            divider_active_segment(vertical, right, true),
            Some(Rect {
                y: 50,
                height: 50,
                ..vertical.rect
            })
        );

        let horizontal = PaneDivider {
            axis: Axis::Vertical,
            rect: Rect {
                x: 0,
                y: 40,
                width: 100,
                height: 16,
            },
        };
        let top = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let bottom = Rect {
            x: 0,
            y: 56,
            width: 100,
            height: 44,
        };
        assert_eq!(
            divider_active_segment(horizontal, top, true),
            Some(Rect {
                width: 50,
                ..horizontal.rect
            })
        );
        assert_eq!(
            divider_active_segment(horizontal, bottom, true),
            Some(Rect {
                x: 50,
                width: 50,
                ..horizontal.rect
            })
        );
    }

    #[test]
    fn odd_shared_dividers_assign_extra_unit_to_leading_half() {
        let vertical = PaneDivider {
            axis: Axis::Horizontal,
            rect: Rect {
                x: 40,
                y: 10,
                width: 8,
                height: 5,
            },
        };
        let left = Rect {
            x: 0,
            y: 10,
            width: 40,
            height: 5,
        };
        let right = Rect {
            x: 48,
            y: 10,
            width: 52,
            height: 5,
        };
        assert_eq!(
            divider_active_segment(vertical, left, true),
            Some(Rect {
                height: 3,
                ..vertical.rect
            })
        );
        assert_eq!(
            divider_active_segment(vertical, right, true),
            Some(Rect {
                y: 13,
                height: 2,
                ..vertical.rect
            })
        );

        let horizontal = PaneDivider {
            axis: Axis::Vertical,
            rect: Rect {
                x: 10,
                y: 40,
                width: 5,
                height: 8,
            },
        };
        let top = Rect {
            x: 10,
            y: 0,
            width: 5,
            height: 40,
        };
        let bottom = Rect {
            x: 10,
            y: 48,
            width: 5,
            height: 52,
        };
        assert_eq!(
            divider_active_segment(horizontal, top, true),
            Some(Rect {
                width: 3,
                ..horizontal.rect
            })
        );
        assert_eq!(
            divider_active_segment(horizontal, bottom, true),
            Some(Rect {
                x: 13,
                width: 2,
                ..horizontal.rect
            })
        );
    }

    #[test]
    fn nested_active_dividers_keep_overlap_and_directional_junction_arms() {
        let vertical = PaneDivider {
            axis: Axis::Horizontal,
            rect: Rect {
                x: 40,
                y: 0,
                width: 8,
                height: 100,
            },
        };
        let horizontal = PaneDivider {
            axis: Axis::Vertical,
            rect: Rect {
                x: 48,
                y: 40,
                width: 52,
                height: 16,
            },
        };
        let active_top_right = Rect {
            x: 48,
            y: 0,
            width: 52,
            height: 40,
        };
        let vertical_segment = divider_active_segment(vertical, active_top_right, false).unwrap();
        let horizontal_segment =
            divider_active_segment(horizontal, active_top_right, false).unwrap();
        assert_eq!(
            vertical_segment,
            Rect {
                y: 0,
                height: 40,
                ..vertical.rect
            }
        );
        assert_eq!(horizontal_segment, horizontal.rect);

        let (character, junction) = divider_junction(vertical, horizontal).unwrap();
        assert_eq!(character, '├');
        assert_eq!(
            active_junction_arm_clips(vertical, vertical_segment, junction),
            vec![(
                '│',
                Rect {
                    height: 8,
                    ..junction
                }
            )]
        );
        assert_eq!(
            active_junction_arm_clips(horizontal, horizontal_segment, junction),
            vec![(
                '─',
                Rect {
                    x: 44,
                    width: 4,
                    ..junction
                }
            )]
        );
    }

    fn scaled_junction_layout() -> (PaneLayout, SplintId) {
        let left_id = SplintId::new();
        let active_id = SplintId::new();
        let bottom_id = SplintId::new();
        let left = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 100,
        };
        let top_right = Rect {
            x: 48,
            y: 0,
            width: 52,
            height: 40,
        };
        let bottom_right = Rect {
            x: 48,
            y: 56,
            width: 52,
            height: 44,
        };
        let panes = [
            (left_id, left),
            (active_id, top_right),
            (bottom_id, bottom_right),
        ]
        .map(|(splint_id, rect)| PaneGeometry {
            splint_id,
            rect,
            allocation: rect,
        })
        .to_vec();
        let separators = vec![
            PaneDivider {
                axis: Axis::Horizontal,
                rect: Rect {
                    x: 40,
                    y: 0,
                    width: 8,
                    height: 100,
                },
            },
            PaneDivider {
                axis: Axis::Vertical,
                rect: Rect {
                    x: 48,
                    y: 40,
                    width: 52,
                    height: 16,
                },
            },
        ];
        (
            PaneLayout {
                panes,
                separators,
                splits: Vec::new(),
                chrome: PaneChrome::Line {
                    vertical_width: 8,
                    horizontal_height: 16,
                },
            },
            active_id,
        )
    }

    fn rect_has_color(canvas: &[u8], canvas_width: u32, rect: Rect, color: [u8; 4]) -> bool {
        (rect.y..rect.y + rect.height).any(|y| {
            (rect.x..rect.x + rect.width).any(|x| {
                let index = usize::try_from((y * canvas_width + x) * 4).unwrap();
                canvas[index..index + 4] == color
            })
        })
    }

    #[test]
    fn scaled_junction_paint_preserves_inactive_arm_and_overlays_active_arms() {
        let (layout, active_id) = scaled_junction_layout();
        let width = 125;
        let height = 125;
        let mut canvas = vec![0; usize::try_from(width * height * 4).unwrap()];
        let theme = ResolvedTheme {
            pane_border: 0x11_22_33,
            pane_border_active: 0xcc_88_44,
            ..ResolvedTheme::default()
        };
        paint_pane_chrome(
            &mut canvas,
            width,
            height,
            &layout,
            Some(active_id),
            theme,
            10,
            20,
            150,
            &HashMap::new(),
        )
        .unwrap();

        let inactive = [0x33, 0x22, 0x11, 0xff];
        let active = [0x44, 0x88, 0xcc, 0xff];
        let top_arm = Rect {
            x: 50,
            y: 50,
            width: 10,
            height: 5,
        };
        let bottom_arm = Rect {
            x: 50,
            y: 65,
            width: 10,
            height: 5,
        };
        let right_arm = Rect {
            x: 57,
            y: 50,
            width: 3,
            height: 20,
        };

        // The 150/120 scale maps the logical junction to x=50..60, y=50..70.
        assert!(rect_has_color(&canvas, width, top_arm, active));
        assert!(rect_has_color(&canvas, width, bottom_arm, inactive));
        assert!(!rect_has_color(&canvas, width, bottom_arm, active));
        assert!(rect_has_color(&canvas, width, right_arm, active));
    }
}
