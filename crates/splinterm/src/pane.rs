//! Pure pane layout and client-local focus navigation.
//!
//! The daemon owns the binary tree and its ratios. A graphical client derives
//! rectangles and focus locally; computing or traversing this view never mutates
//! daemon topology or another client's focus.

use anyhow::{Result, bail};
use splinterm_core::{Axis, LayoutNode, SplintId, SplitRatio};

use crate::geometry::Rect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneGeometry {
    pub splint_id: SplintId,
    /// Terminal content; input, selection, IME, and PTY sizing use this rectangle.
    pub rect: Rect,
    /// Complete leaf allocation; frame chrome occupies its inset edges.
    pub allocation: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneDivider {
    pub axis: Axis,
    pub rect: Rect,
}

/// Stable identity and geometry for one draggable branch boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneSplit {
    pub axis: Axis,
    pub area: Rect,
    pub boundary: u32,
    pub separator: u32,
    pub target: SplintId,
    pub ancestor: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneChrome {
    None,
    Line {
        vertical_width: u32,
        horizontal_height: u32,
    },
    Frame {
        vertical_width: u32,
        horizontal_height: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneResizeMetrics {
    pub cell_width: u32,
    pub cell_height: u32,
    pub minimum_width: u32,
    pub minimum_height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneLayout {
    pub panes: Vec<PaneGeometry>,
    pub separators: Vec<PaneDivider>,
    pub splits: Vec<PaneSplit>,
    pub chrome: PaneChrome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneLayout {
    /// Derives deterministic leaf and separator rectangles from one window tree.
    ///
    /// `Horizontal` arranges children left-to-right and `Vertical` top-to-bottom.
    /// Ratio multiplication rounds toward the first child's leading edge; the
    /// second child owns the residual pixel.
    ///
    /// # Errors
    ///
    /// Returns an error when the area is empty, arithmetic overflows, a separator
    /// cannot fit, or any derived leaf is smaller than the requested minimum.
    pub fn compute(
        root: &LayoutNode,
        area: Rect,
        separator: u32,
        minimum_width: u32,
        minimum_height: u32,
    ) -> Result<Self> {
        Self::compute_with_chrome(
            root,
            area,
            PaneChrome::Line {
                vertical_width: separator,
                horizontal_height: separator,
            },
            minimum_width,
            minimum_height,
        )
    }

    /// Computes terminal content, leaf allocations, and trusted chrome lanes.
    ///
    /// # Errors
    ///
    /// Returns an error for empty areas, zero minimums, arithmetic overflow, or
    /// any layout whose style-specific chrome leaves undersized pane content.
    pub fn compute_with_chrome(
        root: &LayoutNode,
        area: Rect,
        chrome: PaneChrome,
        minimum_width: u32,
        minimum_height: u32,
    ) -> Result<Self> {
        if area.width == 0 || area.height == 0 {
            bail!("pane area must be nonempty");
        }
        if minimum_width == 0 || minimum_height == 0 {
            bail!("minimum pane dimensions must be nonzero");
        }
        let mut layout = Self {
            panes: Vec::with_capacity(root.splint_count()),
            separators: Vec::with_capacity(root.splint_count().saturating_sub(1)),
            splits: Vec::with_capacity(root.splint_count().saturating_sub(1)),
            chrome,
        };
        layout.visit(root, area, minimum_width, minimum_height)?;
        Ok(layout)
    }

    #[must_use]
    pub fn rect(&self, splint_id: SplintId) -> Option<Rect> {
        self.panes
            .iter()
            .find(|pane| pane.splint_id == splint_id)
            .map(|pane| pane.rect)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "binary-tree partition and style-specific chrome remain one recursive transaction"
    )]
    fn visit(
        &mut self,
        node: &LayoutNode,
        area: Rect,
        minimum_width: u32,
        minimum_height: u32,
    ) -> Result<()> {
        match node {
            LayoutNode::Leaf(splint) => {
                let content = match self.chrome {
                    PaneChrome::Frame {
                        vertical_width,
                        horizontal_height,
                    } => inset_frame(area, vertical_width, horizontal_height)?,
                    PaneChrome::None | PaneChrome::Line { .. } => area,
                };
                if content.width < minimum_width || content.height < minimum_height {
                    bail!("surface cannot fit every pane at its minimum dimensions");
                }
                self.panes.push(PaneGeometry {
                    splint_id: splint.id,
                    rect: content,
                    allocation: area,
                });
            }
            LayoutNode::Branch {
                axis,
                ratio,
                first,
                second,
            } => {
                let ratio = u64::from(ratio.get());
                let separator = match (self.chrome, axis) {
                    (PaneChrome::Line { vertical_width, .. }, Axis::Horizontal) => vertical_width,
                    (
                        PaneChrome::Line {
                            horizontal_height, ..
                        },
                        Axis::Vertical,
                    ) => horizontal_height,
                    (PaneChrome::None | PaneChrome::Frame { .. }, _) => 0,
                };
                let (first_rect, separator_rect, second_rect) = match axis {
                    Axis::Horizontal => {
                        let available = area
                            .width
                            .checked_sub(separator)
                            .ok_or_else(|| anyhow::anyhow!("separator exceeds pane width"))?;
                        let first_width = u32::try_from(u64::from(available) * ratio / 1000)?;
                        let second_width = available - first_width;
                        let separator_x = area
                            .x
                            .checked_add(first_width)
                            .ok_or_else(|| anyhow::anyhow!("pane x coordinate overflow"))?;
                        let second_x = separator_x
                            .checked_add(separator)
                            .ok_or_else(|| anyhow::anyhow!("pane x coordinate overflow"))?;
                        (
                            Rect {
                                width: first_width,
                                ..area
                            },
                            Rect {
                                x: separator_x,
                                y: area.y,
                                width: separator,
                                height: area.height,
                            },
                            Rect {
                                x: second_x,
                                y: area.y,
                                width: second_width,
                                height: area.height,
                            },
                        )
                    }
                    Axis::Vertical => {
                        let available = area
                            .height
                            .checked_sub(separator)
                            .ok_or_else(|| anyhow::anyhow!("separator exceeds pane height"))?;
                        let first_height = u32::try_from(u64::from(available) * ratio / 1000)?;
                        let second_height = available - first_height;
                        let separator_y = area
                            .y
                            .checked_add(first_height)
                            .ok_or_else(|| anyhow::anyhow!("pane y coordinate overflow"))?;
                        let second_y = separator_y
                            .checked_add(separator)
                            .ok_or_else(|| anyhow::anyhow!("pane y coordinate overflow"))?;
                        (
                            Rect {
                                height: first_height,
                                ..area
                            },
                            Rect {
                                x: area.x,
                                y: separator_y,
                                width: area.width,
                                height: separator,
                            },
                            Rect {
                                x: area.x,
                                y: second_y,
                                width: area.width,
                                height: second_height,
                            },
                        )
                    }
                };
                if separator > 0 {
                    self.separators.push(PaneDivider {
                        axis: *axis,
                        rect: separator_rect,
                    });
                }
                let target = first.first_splint_id();
                let ancestor = ancestor_distance(first, target)
                    .ok_or_else(|| anyhow::anyhow!("split target is absent"))?;
                self.splits.push(PaneSplit {
                    axis: *axis,
                    area,
                    boundary: match axis {
                        Axis::Horizontal => separator_rect.x,
                        Axis::Vertical => separator_rect.y,
                    },
                    separator,
                    target,
                    ancestor,
                });
                self.visit(first, first_rect, minimum_width, minimum_height)?;
                self.visit(second, second_rect, minimum_width, minimum_height)?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn next(&self, current: SplintId, reverse: bool) -> Option<SplintId> {
        let index = self
            .panes
            .iter()
            .position(|pane| pane.splint_id == current)?;
        let next = if reverse {
            index.checked_sub(1).unwrap_or(self.panes.len() - 1)
        } else {
            (index + 1) % self.panes.len()
        };
        Some(self.panes[next].splint_id)
    }

    #[must_use]
    pub fn directional(&self, current: SplintId, direction: FocusDirection) -> Option<SplintId> {
        let current = self.panes.iter().find(|pane| pane.splint_id == current)?;
        self.panes
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.splint_id != current.splint_id)
            .filter_map(|(index, candidate)| {
                directional_score(current.rect, candidate.rect, direction)
                    .map(|score| (score, index, candidate.splint_id))
            })
            .min_by_key(|(score, index, _)| (*score, *index))
            .map(|(_, _, id)| id)
    }

    /// Finds the closest branch boundary under a pointer-sized hit lane.
    #[must_use]
    pub fn split_at(&self, position: (f64, f64), hit_slop: u32) -> Option<PaneSplit> {
        if !position.0.is_finite() || !position.1.is_finite() {
            return None;
        }
        self.splits
            .iter()
            .filter_map(|split| {
                let (primary, orthogonal, orthogonal_start, orthogonal_extent) = match split.axis {
                    Axis::Horizontal => (position.0, position.1, split.area.y, split.area.height),
                    Axis::Vertical => (position.1, position.0, split.area.x, split.area.width),
                };
                let orthogonal_start = f64::from(orthogonal_start);
                if orthogonal < orthogonal_start
                    || orthogonal >= orthogonal_start + f64::from(orthogonal_extent)
                {
                    return None;
                }
                let center = f64::from(split.boundary) + f64::from(split.separator) / 2.0;
                let distance = (primary - center).abs();
                (distance <= f64::from(hit_slop.max(split.separator.div_ceil(2)))).then_some((
                    distance,
                    u64::from(orthogonal_extent),
                    *split,
                ))
            })
            .min_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
            })
            .map(|(_, _, split)| split)
    }
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the finite pointer coordinate is rounded and clamped to bounded nonnegative extents"
)]
pub fn split_ratio_at(
    split: PaneSplit,
    position: (f64, f64),
    minimum_extent: u32,
) -> Option<SplitRatio> {
    let (coordinate, start, extent) = match split.axis {
        Axis::Horizontal => (position.0, split.area.x, split.area.width),
        Axis::Vertical => (position.1, split.area.y, split.area.height),
    };
    if !coordinate.is_finite() {
        return None;
    }
    let available = extent.checked_sub(split.separator)?;
    let maximum_first = available.checked_sub(minimum_extent)?;
    if minimum_extent > maximum_first {
        return None;
    }
    let desired = (coordinate - f64::from(start) - f64::from(split.separator) / 2.0)
        .round()
        .clamp(f64::from(minimum_extent), f64::from(maximum_first));
    let ratio = (desired as u64 * 1000 / u64::from(available)).clamp(1, 999);
    SplitRatio::new(u16::try_from(ratio).ok()?).ok()
}

#[must_use]
pub fn directional_resize_ratio(
    root: &LayoutNode,
    layout: &PaneLayout,
    current: SplintId,
    direction: FocusDirection,
    cells: u16,
    metrics: PaneResizeMetrics,
) -> Option<(SplintId, u16, SplitRatio)> {
    fn contains(outer: Rect, inner: Rect) -> bool {
        let Some(outer_right) = outer.x.checked_add(outer.width) else {
            return false;
        };
        let Some(outer_bottom) = outer.y.checked_add(outer.height) else {
            return false;
        };
        let Some(inner_right) = inner.x.checked_add(inner.width) else {
            return false;
        };
        let Some(inner_bottom) = inner.y.checked_add(inner.height) else {
            return false;
        };
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner_right <= outer_right
            && inner_bottom <= outer_bottom
    }

    let pane = layout.panes.iter().find(|pane| pane.splint_id == current)?;
    let right = pane.allocation.x.checked_add(pane.allocation.width)?;
    let bottom = pane.allocation.y.checked_add(pane.allocation.height)?;
    let ancestors = layout
        .splits
        .iter()
        .filter(|split| contains(split.area, pane.allocation));
    let split = match direction {
        FocusDirection::Left => ancestors
            .filter(|split| split.axis == Axis::Horizontal && split.boundary <= pane.allocation.x)
            .max_by_key(|split| split.boundary),
        FocusDirection::Right => ancestors
            .filter(|split| split.axis == Axis::Horizontal && split.boundary >= right)
            .min_by_key(|split| split.boundary),
        FocusDirection::Up => ancestors
            .filter(|split| split.axis == Axis::Vertical && split.boundary <= pane.allocation.y)
            .max_by_key(|split| split.boundary),
        FocusDirection::Down => ancestors
            .filter(|split| split.axis == Axis::Vertical && split.boundary >= bottom)
            .min_by_key(|split| split.boundary),
    }
    .copied()?;
    let (cell_extent, delta_sign, minimum_extent) = match direction {
        FocusDirection::Left => (metrics.cell_width, -1_i64, metrics.minimum_width),
        FocusDirection::Right => (metrics.cell_width, 1_i64, metrics.minimum_width),
        FocusDirection::Up => (metrics.cell_height, -1_i64, metrics.minimum_height),
        FocusDirection::Down => (metrics.cell_height, 1_i64, metrics.minimum_height),
    };
    let delta = i64::from(cell_extent)
        .checked_mul(i64::from(cells))?
        .checked_mul(delta_sign)?;
    let center = i64::from(split.boundary) + i64::from(split.separator) / 2;
    let coordinate = i32::try_from(center.checked_add(delta)?).ok()?;
    let position = match split.axis {
        Axis::Horizontal => (f64::from(coordinate), f64::from(split.area.y)),
        Axis::Vertical => (f64::from(split.area.x), f64::from(coordinate)),
    };
    let ratio = split_ratio_at(split, position, minimum_extent)?;
    let mut candidate = root.clone();
    if !apply_preview_ratio(&mut candidate, split.target, split.ancestor, ratio) {
        return None;
    }
    let root_area = layout.splits.first()?.area;
    PaneLayout::compute_with_chrome(
        &candidate,
        root_area,
        layout.chrome,
        metrics.minimum_width,
        metrics.minimum_height,
    )
    .ok()?;
    Some((split.target, split.ancestor, ratio))
}

pub fn apply_preview_ratio(
    root: &mut LayoutNode,
    target: SplintId,
    ancestor: u16,
    ratio: SplitRatio,
) -> bool {
    fn path_to_target(node: &LayoutNode, target: SplintId, path: &mut Vec<bool>) -> bool {
        match node {
            LayoutNode::Leaf(splint) => splint.id == target,
            LayoutNode::Branch { first, second, .. } => {
                path.push(false);
                if path_to_target(first, target, path) {
                    return true;
                }
                path.pop();
                path.push(true);
                if path_to_target(second, target, path) {
                    return true;
                }
                path.pop();
                false
            }
        }
    }

    let mut path = Vec::new();
    if !path_to_target(root, target, &mut path) || usize::from(ancestor) >= path.len() {
        return false;
    }
    let branch_depth = path.len() - 1 - usize::from(ancestor);
    let mut node = root;
    for second in path.into_iter().take(branch_depth) {
        let LayoutNode::Branch {
            first,
            second: second_node,
            ..
        } = node
        else {
            return false;
        };
        node = if second { second_node } else { first };
    }
    let LayoutNode::Branch { ratio: current, .. } = node else {
        return false;
    };
    *current = ratio;
    true
}

fn ancestor_distance(node: &LayoutNode, target: SplintId) -> Option<u16> {
    match node {
        LayoutNode::Leaf(splint) => (splint.id == target).then_some(0),
        LayoutNode::Branch { first, second, .. } => ancestor_distance(first, target)
            .or_else(|| ancestor_distance(second, target))
            .and_then(|distance| distance.checked_add(1)),
    }
}

fn inset_frame(area: Rect, vertical_width: u32, horizontal_height: u32) -> Result<Rect> {
    if vertical_width == 0 || horizontal_height == 0 {
        bail!("frame edges must be nonzero");
    }
    let horizontal_chrome = vertical_width
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("frame width overflow"))?;
    let vertical_chrome = horizontal_height
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("frame height overflow"))?;
    Ok(Rect {
        x: area
            .x
            .checked_add(vertical_width)
            .ok_or_else(|| anyhow::anyhow!("frame x overflow"))?,
        y: area
            .y
            .checked_add(horizontal_height)
            .ok_or_else(|| anyhow::anyhow!("frame y overflow"))?,
        width: area
            .width
            .checked_sub(horizontal_chrome)
            .ok_or_else(|| anyhow::anyhow!("surface cannot fit pane frame width"))?,
        height: area
            .height
            .checked_sub(vertical_chrome)
            .ok_or_else(|| anyhow::anyhow!("surface cannot fit pane frame height"))?,
    })
}

fn directional_score(
    current: Rect,
    candidate: Rect,
    direction: FocusDirection,
) -> Option<(u8, u64, u64)> {
    let current_center_x = u64::from(current.x) * 2 + u64::from(current.width);
    let current_center_y = u64::from(current.y) * 2 + u64::from(current.height);
    let candidate_center_x = u64::from(candidate.x) * 2 + u64::from(candidate.width);
    let candidate_center_y = u64::from(candidate.y) * 2 + u64::from(candidate.height);
    let (forward, primary, orthogonal, overlaps) = match direction {
        FocusDirection::Left => (
            candidate_center_x < current_center_x,
            current_center_x.saturating_sub(candidate_center_x),
            current_center_y.abs_diff(candidate_center_y),
            intervals_overlap(current.y, current.height, candidate.y, candidate.height),
        ),
        FocusDirection::Right => (
            candidate_center_x > current_center_x,
            candidate_center_x.saturating_sub(current_center_x),
            current_center_y.abs_diff(candidate_center_y),
            intervals_overlap(current.y, current.height, candidate.y, candidate.height),
        ),
        FocusDirection::Up => (
            candidate_center_y < current_center_y,
            current_center_y.saturating_sub(candidate_center_y),
            current_center_x.abs_diff(candidate_center_x),
            intervals_overlap(current.x, current.width, candidate.x, candidate.width),
        ),
        FocusDirection::Down => (
            candidate_center_y > current_center_y,
            candidate_center_y.saturating_sub(current_center_y),
            current_center_x.abs_diff(candidate_center_x),
            intervals_overlap(current.x, current.width, candidate.x, candidate.width),
        ),
    };
    forward.then_some((u8::from(!overlaps), primary, orthogonal))
}

fn intervals_overlap(first: u32, first_size: u32, second: u32, second_size: u32) -> bool {
    let first_end = u64::from(first) + u64::from(first_size);
    let second_end = u64::from(second) + u64::from(second_size);
    u64::from(first) < second_end && u64::from(second) < first_end
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use splinterm_core::{Splint, SplitRatio};

    use super::*;

    fn leaf() -> (LayoutNode, SplintId) {
        let splint = Splint::shell(PathBuf::from("/tmp"));
        let id = splint.id;
        (LayoutNode::Leaf(splint), id)
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the nested layout's pane and divider rectangles form one exact geometry table"
    )]
    fn nested_layout_owns_every_pixel_once_with_stable_rounding() {
        let (first, first_id) = leaf();
        let (second, second_id) = leaf();
        let (third, third_id) = leaf();
        let root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(333).unwrap(),
            first: Box::new(first),
            second: Box::new(LayoutNode::Branch {
                axis: Axis::Vertical,
                ratio: SplitRatio::new(500).unwrap(),
                first: Box::new(second),
                second: Box::new(third),
            }),
        };

        let layout = PaneLayout::compute(
            &root,
            Rect {
                x: 10,
                y: 20,
                width: 101,
                height: 81,
            },
            1,
            10,
            10,
        )
        .unwrap();
        assert_eq!(layout.panes.len(), 3);
        assert_eq!(layout.separators.len(), 2);
        assert_eq!(
            layout.panes[0],
            PaneGeometry {
                splint_id: first_id,
                rect: Rect {
                    x: 10,
                    y: 20,
                    width: 33,
                    height: 81
                },
                allocation: Rect {
                    x: 10,
                    y: 20,
                    width: 33,
                    height: 81
                },
            }
        );
        assert_eq!(
            layout.panes[1],
            PaneGeometry {
                splint_id: second_id,
                rect: Rect {
                    x: 44,
                    y: 20,
                    width: 67,
                    height: 40
                },
                allocation: Rect {
                    x: 44,
                    y: 20,
                    width: 67,
                    height: 40
                },
            }
        );
        assert_eq!(
            layout.panes[2],
            PaneGeometry {
                splint_id: third_id,
                rect: Rect {
                    x: 44,
                    y: 61,
                    width: 67,
                    height: 40
                },
                allocation: Rect {
                    x: 44,
                    y: 61,
                    width: 67,
                    height: 40
                },
            }
        );
        assert_eq!(
            layout.separators[0],
            PaneDivider {
                axis: Axis::Horizontal,
                rect: Rect {
                    x: 43,
                    y: 20,
                    width: 1,
                    height: 81
                }
            }
        );
        assert_eq!(
            layout.separators[1],
            PaneDivider {
                axis: Axis::Vertical,
                rect: Rect {
                    x: 44,
                    y: 60,
                    width: 67,
                    height: 1
                }
            }
        );
    }

    #[test]
    fn nested_branch_dividers_hit_and_resize_the_exact_ancestor() {
        let (first, first_id) = leaf();
        let (second, _) = leaf();
        let (third, _) = leaf();
        let (fourth, _) = leaf();
        let mut root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(LayoutNode::Branch {
                axis: Axis::Vertical,
                ratio: SplitRatio::new(400).unwrap(),
                first: Box::new(first),
                second: Box::new(second),
            }),
            second: Box::new(LayoutNode::Branch {
                axis: Axis::Vertical,
                ratio: SplitRatio::new(600).unwrap(),
                first: Box::new(third),
                second: Box::new(fourth),
            }),
        };
        let layout = PaneLayout::compute(
            &root,
            Rect {
                x: 0,
                y: 0,
                width: 101,
                height: 101,
            },
            1,
            2,
            2,
        )
        .unwrap();

        let split = layout.split_at((50.5, 10.0), 6).unwrap();
        assert_eq!(split.axis, Axis::Horizontal);
        assert_eq!(split.target, first_id);
        assert_eq!(split.ancestor, 1);
        let ratio = split_ratio_at(split, (70.0, 10.0), 2).unwrap();
        assert_eq!(ratio.get(), 700);
        assert!(apply_preview_ratio(
            &mut root,
            split.target,
            split.ancestor,
            ratio
        ));
        let LayoutNode::Branch {
            ratio: outer,
            first,
            ..
        } = root
        else {
            panic!("expected outer branch");
        };
        assert_eq!(outer, ratio);
        let LayoutNode::Branch { ratio: inner, .. } = *first else {
            panic!("expected nested branch");
        };
        assert_eq!(inner.get(), 400);
    }

    #[test]
    fn line_chrome_uses_orientation_specific_cell_lanes() {
        let (first, _) = leaf();
        let (second, _) = leaf();
        let root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(first),
            second: Box::new(second),
        };
        let layout = PaneLayout::compute_with_chrome(
            &root,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 80,
            },
            PaneChrome::Line {
                vertical_width: 8,
                horizontal_height: 16,
            },
            10,
            10,
        )
        .unwrap();
        assert_eq!(layout.separators[0].rect.width, 8);
        assert_eq!(layout.panes[0].rect.width, 46);
        assert_eq!(layout.panes[1].rect.x, 54);
    }

    #[test]
    fn frame_chrome_insets_each_leaf_without_a_shared_separator() {
        let (first, _) = leaf();
        let (second, _) = leaf();
        let root = LayoutNode::Branch {
            axis: Axis::Vertical,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(first),
            second: Box::new(second),
        };
        let layout = PaneLayout::compute_with_chrome(
            &root,
            Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 80,
            },
            PaneChrome::Frame {
                vertical_width: 8,
                horizontal_height: 16,
            },
            10,
            5,
        )
        .unwrap();
        assert!(layout.separators.is_empty());
        assert_eq!(layout.panes[0].allocation.height, 40);
        assert_eq!(layout.panes[0].rect.x, 18);
        assert_eq!(layout.panes[0].rect.y, 36);
        assert_eq!(layout.panes[0].rect.width, 84);
        assert_eq!(layout.panes[0].rect.height, 8);
        assert_eq!(layout.panes[1].rect.y, 76);
    }

    #[test]
    fn frame_chrome_rejects_content_below_the_minimum() {
        let (root, _) = leaf();
        assert!(
            PaneLayout::compute_with_chrome(
                &root,
                Rect {
                    x: 0,
                    y: 0,
                    width: 19,
                    height: 39,
                },
                PaneChrome::Frame {
                    vertical_width: 5,
                    horizontal_height: 10,
                },
                10,
                20,
            )
            .is_err()
        );
    }

    #[test]
    fn minimum_dimensions_reject_an_unrenderable_tree() {
        let (first, _) = leaf();
        let (second, _) = leaf();
        let root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(first),
            second: Box::new(second),
        };
        assert!(
            PaneLayout::compute(
                &root,
                Rect {
                    x: 0,
                    y: 0,
                    width: 20,
                    height: 10
                },
                1,
                10,
                10,
            )
            .is_err()
        );
    }

    #[test]
    fn traversal_is_tree_ordered_and_directional_focus_prefers_overlap() {
        let (left, left_id) = leaf();
        let (top_right, top_right_id) = leaf();
        let (bottom_right, bottom_right_id) = leaf();
        let root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(left),
            second: Box::new(LayoutNode::Branch {
                axis: Axis::Vertical,
                ratio: SplitRatio::new(500).unwrap(),
                first: Box::new(top_right),
                second: Box::new(bottom_right),
            }),
        };
        let layout = PaneLayout::compute(
            &root,
            Rect {
                x: 0,
                y: 0,
                width: 101,
                height: 101,
            },
            1,
            1,
            1,
        )
        .unwrap();

        assert_eq!(layout.next(left_id, false), Some(top_right_id));
        assert_eq!(layout.next(left_id, true), Some(bottom_right_id));
        assert_eq!(
            layout.directional(left_id, FocusDirection::Right),
            Some(top_right_id)
        );
        assert_eq!(
            layout.directional(top_right_id, FocusDirection::Down),
            Some(bottom_right_id)
        );
        assert_eq!(
            layout.directional(bottom_right_id, FocusDirection::Left),
            Some(left_id)
        );
        assert_eq!(layout.directional(left_id, FocusDirection::Left), None);
    }

    #[test]
    fn directional_resize_moves_the_authoritative_boundary_by_exact_cells() {
        let (left, left_id) = leaf();
        let (right, right_id) = leaf();
        let root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(left),
            second: Box::new(right),
        };
        let layout = PaneLayout::compute(
            &root,
            Rect {
                x: 0,
                y: 0,
                width: 1_000,
                height: 400,
            },
            0,
            1,
            1,
        )
        .unwrap();
        assert_eq!(
            directional_resize_ratio(
                &root,
                &layout,
                left_id,
                FocusDirection::Right,
                5,
                PaneResizeMetrics {
                    cell_width: 10,
                    cell_height: 20,
                    minimum_width: 20,
                    minimum_height: 40,
                },
            ),
            Some((left_id, 0, SplitRatio::new(550).unwrap()))
        );
        assert_eq!(
            directional_resize_ratio(
                &root,
                &layout,
                left_id,
                FocusDirection::Right,
                5,
                PaneResizeMetrics {
                    cell_width: 20,
                    cell_height: 40,
                    minimum_width: 40,
                    minimum_height: 80,
                },
            ),
            Some((left_id, 0, SplitRatio::new(600).unwrap()))
        );
        assert_eq!(
            directional_resize_ratio(
                &root,
                &layout,
                right_id,
                FocusDirection::Left,
                5,
                PaneResizeMetrics {
                    cell_width: 10,
                    cell_height: 20,
                    minimum_width: 20,
                    minimum_height: 40,
                },
            ),
            Some((left_id, 0, SplitRatio::new(450).unwrap()))
        );
        assert_eq!(
            directional_resize_ratio(
                &root,
                &layout,
                left_id,
                FocusDirection::Left,
                5,
                PaneResizeMetrics {
                    cell_width: 10,
                    cell_height: 20,
                    minimum_width: 20,
                    minimum_height: 40,
                },
            ),
            None
        );
    }

    #[test]
    fn directional_resize_stays_on_the_focus_path_and_rejects_unsafe_descendants() {
        let (top_left, top_left_id) = leaf();
        let (top_right, _) = leaf();
        let (bottom_left, bottom_left_id) = leaf();
        let (bottom_right, _) = leaf();
        let disjoint = LayoutNode::Branch {
            axis: Axis::Vertical,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(LayoutNode::Branch {
                axis: Axis::Horizontal,
                ratio: SplitRatio::new(700).unwrap(),
                first: Box::new(top_left),
                second: Box::new(top_right),
            }),
            second: Box::new(LayoutNode::Branch {
                axis: Axis::Horizontal,
                ratio: SplitRatio::new(700).unwrap(),
                first: Box::new(bottom_left),
                second: Box::new(bottom_right),
            }),
        };
        let layout = PaneLayout::compute(
            &disjoint,
            Rect {
                x: 0,
                y: 0,
                width: 1_000,
                height: 400,
            },
            0,
            20,
            40,
        )
        .unwrap();
        let (target, ancestor, ratio) = directional_resize_ratio(
            &disjoint,
            &layout,
            top_left_id,
            FocusDirection::Right,
            5,
            PaneResizeMetrics {
                cell_width: 10,
                cell_height: 20,
                minimum_width: 20,
                minimum_height: 40,
            },
        )
        .unwrap();
        assert_eq!((target, ancestor), (top_left_id, 0));
        assert_eq!(ratio, SplitRatio::new(750).unwrap());
        assert_ne!(target, bottom_left_id);

        let (nested_left, _) = leaf();
        let (nested_right, _) = leaf();
        let (outer_right, outer_right_id) = leaf();
        let constrained = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(667).unwrap(),
            first: Box::new(LayoutNode::Branch {
                axis: Axis::Horizontal,
                ratio: SplitRatio::new(500).unwrap(),
                first: Box::new(nested_left),
                second: Box::new(nested_right),
            }),
            second: Box::new(outer_right),
        };
        let constrained_layout = PaneLayout::compute(
            &constrained,
            Rect {
                x: 0,
                y: 0,
                width: 300,
                height: 200,
            },
            0,
            100,
            40,
        )
        .unwrap();
        assert_eq!(
            directional_resize_ratio(
                &constrained,
                &constrained_layout,
                outer_right_id,
                FocusDirection::Left,
                5,
                PaneResizeMetrics {
                    cell_width: 10,
                    cell_height: 20,
                    minimum_width: 100,
                    minimum_height: 40,
                },
            ),
            None
        );
    }
}
