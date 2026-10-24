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
    /// The allocation grown across its share of each adjacent pane gap, up to
    /// the divider line that bounds it (Line chrome) or the gap's middle.
    /// Equal to `allocation` when the gap is zero. Active line borders are
    /// measured against it so they keep reaching their junctions.
    pub extent: Rect,
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
    /// Leading edge of the lane between the two children.
    pub boundary: u32,
    /// Lane width: the divider line, if any, plus the pane gap. Hit-testing
    /// and drag ratios use the lane's middle.
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
    /// Empty logical pixels between the outer edges of adjacent leaves, in
    /// addition to any divider line. Never applied at the area's edges.
    pub gap: u32,
}

/// How far an area's edges are from the divider lines that bound it: each
/// side's share of the pane gap, or zero at the layout's outer edges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Reach {
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
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
        Self::compute_with_gap(root, area, chrome, 0, minimum_width, minimum_height)
    }

    /// Computes the layout with `gap` empty logical pixels between the outer
    /// edges of adjacent leaves.
    ///
    /// Each branch reserves one lane of the chrome's divider width plus
    /// `gap`. With Line chrome the divider line sits in the middle of that
    /// lane, the leading half of the gap (rounded down) before it and the
    /// trailing half after, and perpendicular divider lines reach across the
    /// gap to meet it. Frame and None chrome reserve only the gap. The area's
    /// outer edges get no gap. A zero gap reproduces
    /// [`Self::compute_with_chrome`] exactly.
    ///
    /// # Errors
    ///
    /// Returns an error for empty areas, zero minimums, arithmetic overflow, or
    /// any layout whose chrome and gap leave undersized pane content.
    pub fn compute_with_gap(
        root: &LayoutNode,
        area: Rect,
        chrome: PaneChrome,
        gap: u32,
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
            gap,
        };
        layout.visit(root, area, Reach::default(), minimum_width, minimum_height)?;
        Ok(layout)
    }

    /// Computes the layout with `preferred_gap` when every pane fits with it,
    /// and otherwise with the largest smaller gap that fits, down to none.
    /// The gap is a preference, never a reason for a layout to fail: the
    /// result's [`Self::gap`] is the gap actually used, and only a tree that
    /// cannot fit even without a gap fails, with the gapless layout's error.
    ///
    /// A larger gap never gives any pane more room, so the fitting gaps are
    /// one range from zero and a binary search finds its end. The search
    /// runs only when the preferred gap does not fit.
    ///
    /// # Errors
    ///
    /// Returns the error [`Self::compute_with_gap`] returns with a zero gap.
    pub fn compute_with_preferred_gap(
        root: &LayoutNode,
        area: Rect,
        chrome: PaneChrome,
        preferred_gap: u32,
        minimum_width: u32,
        minimum_height: u32,
    ) -> Result<Self> {
        let compute =
            |gap| Self::compute_with_gap(root, area, chrome, gap, minimum_width, minimum_height);
        let preferred = compute(preferred_gap);
        if preferred.is_ok() || preferred_gap == 0 {
            return preferred;
        }
        // `fits` holds a layout for `low`; `high` is known not to fit.
        let mut fits = compute(0)?;
        let (mut low, mut high) = (0, preferred_gap);
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            if let Ok(layout) = compute(middle) {
                fits = layout;
                low = middle;
            } else {
                high = middle;
            }
        }
        Ok(fits)
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
        reach: Reach,
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
                    extent: grow(area, reach)?,
                });
            }
            LayoutNode::Branch {
                axis,
                ratio,
                first,
                second,
            } => {
                let ratio = u64::from(ratio.get());
                let line = match (self.chrome, axis) {
                    (PaneChrome::Line { vertical_width, .. }, Axis::Horizontal) => vertical_width,
                    (
                        PaneChrome::Line {
                            horizontal_height, ..
                        },
                        Axis::Vertical,
                    ) => horizontal_height,
                    (PaneChrome::None | PaneChrome::Frame { .. }, _) => 0,
                };
                // One lane holds the divider line and the gap around it.
                let separator = line
                    .checked_add(self.gap)
                    .ok_or_else(|| anyhow::anyhow!("pane gap overflow"))?;
                let lead = self.gap / 2;
                let trail = self.gap - lead;
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
                if line > 0 {
                    self.separators.push(PaneDivider {
                        axis: *axis,
                        rect: divider_line(*axis, separator_rect, line, lead, reach)?,
                    });
                }
                let (first_reach, second_reach) = match axis {
                    Axis::Horizontal => (
                        Reach {
                            right: lead,
                            ..reach
                        },
                        Reach {
                            left: trail,
                            ..reach
                        },
                    ),
                    Axis::Vertical => (
                        Reach {
                            bottom: lead,
                            ..reach
                        },
                        Reach {
                            top: trail,
                            ..reach
                        },
                    ),
                };
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
                self.visit(
                    first,
                    first_rect,
                    first_reach,
                    minimum_width,
                    minimum_height,
                )?;
                self.visit(
                    second,
                    second_rect,
                    second_reach,
                    minimum_width,
                    minimum_height,
                )?;
            }
        }
        Ok(())
    }

    /// The pane's [`PaneGeometry::extent`].
    #[must_use]
    pub fn extent(&self, splint_id: SplintId) -> Option<Rect> {
        self.panes
            .iter()
            .find(|pane| pane.splint_id == splint_id)
            .map(|pane| pane.extent)
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
    // As for a divider drag, a ratio is accepted when the panes fit with some
    // gap; the gap shrinks before a resize is refused.
    PaneLayout::compute_with_preferred_gap(
        &candidate,
        root_area,
        layout.chrome,
        layout.gap,
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

/// Grows `area` by `reach` on each side.
fn grow(area: Rect, reach: Reach) -> Result<Rect> {
    let overflow = || anyhow::anyhow!("pane extent overflow");
    Ok(Rect {
        x: area.x.checked_sub(reach.left).ok_or_else(overflow)?,
        y: area.y.checked_sub(reach.top).ok_or_else(overflow)?,
        width: area
            .width
            .checked_add(reach.left)
            .and_then(|width| width.checked_add(reach.right))
            .ok_or_else(overflow)?,
        height: area
            .height
            .checked_add(reach.top)
            .and_then(|height| height.checked_add(reach.bottom))
            .ok_or_else(overflow)?,
    })
}

/// The divider line inside a branch's `lane`: `thickness` pixels thick, `lead`
/// pixels past the lane's leading edge, and stretched along its length across
/// the gaps at the area's ends so it meets the lines that bound them.
fn divider_line(axis: Axis, lane: Rect, thickness: u32, lead: u32, reach: Reach) -> Result<Rect> {
    let overflow = || anyhow::anyhow!("pane divider overflow");
    Ok(match axis {
        Axis::Horizontal => Rect {
            x: lane.x.checked_add(lead).ok_or_else(overflow)?,
            width: thickness,
            ..grow(
                lane,
                Reach {
                    left: 0,
                    right: 0,
                    ..reach
                },
            )?
        },
        Axis::Vertical => Rect {
            y: lane.y.checked_add(lead).ok_or_else(overflow)?,
            height: thickness,
            ..grow(
                lane,
                Reach {
                    top: 0,
                    bottom: 0,
                    ..reach
                },
            )?
        },
    })
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
                extent: Rect {
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
                extent: Rect {
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
                extent: Rect {
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

    const LINE: PaneChrome = PaneChrome::Line {
        vertical_width: 8,
        horizontal_height: 16,
    };
    const FRAME: PaneChrome = PaneChrome::Frame {
        vertical_width: 8,
        horizontal_height: 16,
    };
    const CHROMES: [PaneChrome; 3] = [PaneChrome::None, LINE, FRAME];
    const SCALES: [u32; 3] = [120, 150, 240];

    fn right(rect: Rect) -> u32 {
        rect.x + rect.width
    }

    fn bottom(rect: Rect) -> u32 {
        rect.y + rect.height
    }

    fn overlaps(first: Rect, second: Rect) -> bool {
        first.x < right(second)
            && second.x < right(first)
            && first.y < bottom(second)
            && second.y < bottom(first)
    }

    fn within(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && right(inner) <= right(outer)
            && bottom(inner) <= bottom(outer)
    }

    fn contains_point(rect: Rect, point: (f64, f64)) -> bool {
        point.0 >= f64::from(rect.x)
            && point.0 < f64::from(right(rect))
            && point.1 >= f64::from(rect.y)
            && point.1 < f64::from(bottom(rect))
    }

    fn line_width(chrome: PaneChrome, axis: Axis) -> u32 {
        match (chrome, axis) {
            (PaneChrome::Line { vertical_width, .. }, Axis::Horizontal) => vertical_width,
            (
                PaneChrome::Line {
                    horizontal_height, ..
                },
                Axis::Vertical,
            ) => horizontal_height,
            _ => 0,
        }
    }

    /// The rectangle as `App::buffer_rect` converts it at `scale_120`.
    fn buffer(rect: Rect, scale_120: u32) -> Rect {
        let x = crate::geometry::logical_extent_to_buffer(rect.x, scale_120).unwrap();
        let y = crate::geometry::logical_extent_to_buffer(rect.y, scale_120).unwrap();
        Rect {
            x,
            y,
            width: crate::geometry::logical_extent_to_buffer(right(rect), scale_120).unwrap() - x,
            height: crate::geometry::logical_extent_to_buffer(bottom(rect), scale_120).unwrap() - y,
        }
    }

    fn branch(axis: Axis, ratio: u16, first: LayoutNode, second: LayoutNode) -> LayoutNode {
        LayoutNode::Branch {
            axis,
            ratio: SplitRatio::new(ratio).unwrap(),
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// Left | (top-right / bottom-right), a 2×2 grid with uneven ratios, and
    /// a three-deep alternating chain.
    fn nested_trees() -> Vec<LayoutNode> {
        vec![
            branch(
                Axis::Horizontal,
                500,
                leaf().0,
                branch(Axis::Vertical, 500, leaf().0, leaf().0),
            ),
            branch(
                Axis::Vertical,
                417,
                branch(Axis::Horizontal, 333, leaf().0, leaf().0),
                branch(Axis::Horizontal, 650, leaf().0, leaf().0),
            ),
            branch(
                Axis::Horizontal,
                400,
                branch(Axis::Vertical, 600, leaf().0, leaf().0),
                branch(
                    Axis::Vertical,
                    300,
                    leaf().0,
                    branch(Axis::Horizontal, 500, leaf().0, leaf().0),
                ),
            ),
        ]
    }

    #[test]
    fn zero_pane_gap_keeps_the_tiled_layout_exactly() {
        let (first, first_id) = leaf();
        let (second, second_id) = leaf();
        let root = branch(Axis::Horizontal, 500, first, second);
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 80,
        };
        let gapless = PaneLayout::compute_with_gap(&root, area, LINE, 0, 10, 10).unwrap();
        assert_eq!(gapless.gap, 0);
        assert_eq!(gapless.panes[0].rect.width, 46);
        assert_eq!(gapless.panes[1].rect.x, 54);
        assert_eq!(
            gapless.separators,
            vec![PaneDivider {
                axis: Axis::Horizontal,
                rect: Rect {
                    x: 46,
                    y: 0,
                    width: 8,
                    height: 80,
                },
            }]
        );
        assert_eq!(gapless.extent(first_id), gapless.rect(first_id));
        assert_eq!(gapless.extent(second_id), gapless.rect(second_id));
        for chrome in CHROMES {
            for tree in nested_trees() {
                let area = Rect {
                    x: 3,
                    y: 30,
                    width: 997,
                    height: 701,
                };
                let layout = PaneLayout::compute_with_gap(&tree, area, chrome, 0, 16, 32).unwrap();
                assert_eq!(
                    layout,
                    PaneLayout::compute_with_chrome(&tree, area, chrome, 16, 32).unwrap()
                );
                for pane in &layout.panes {
                    assert_eq!(pane.extent, pane.allocation);
                }
                for split in &layout.splits {
                    assert_eq!(split.separator, line_width(chrome, split.axis));
                }
            }
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one table checks allocations, dividers, extents, hit lanes and drags per style"
    )]
    fn pane_gap_separates_two_panes_exactly_in_both_axes() {
        for chrome in CHROMES {
            for axis in [Axis::Horizontal, Axis::Vertical] {
                for gap in [1, 7, 8, 64] {
                    let label = format!("{chrome:?} {axis:?} gap {gap}");
                    let (first, first_id) = leaf();
                    let (second, second_id) = leaf();
                    let mut root = branch(axis, 500, first, second);
                    let area = Rect {
                        x: 10,
                        y: 20,
                        width: 400,
                        height: 300,
                    };
                    let layout =
                        PaneLayout::compute_with_gap(&root, area, chrome, gap, 10, 10).unwrap();
                    let line = line_width(chrome, axis);
                    let lane_width = line + gap;
                    let first = layout.panes[0];
                    let second = layout.panes[1];
                    assert_eq!((first.splint_id, second.splint_id), (first_id, second_id));
                    let (first_end, second_start, extent, first_extent) = match axis {
                        Axis::Horizontal => {
                            assert_eq!(first.allocation.y, area.y, "{label}");
                            assert_eq!(first.allocation.height, area.height, "{label}");
                            assert_eq!(second.allocation.height, area.height, "{label}");
                            assert_eq!(
                                first.allocation.width + lane_width + second.allocation.width,
                                area.width,
                                "{label}"
                            );
                            (
                                right(first.allocation),
                                second.allocation.x,
                                area.width,
                                first.allocation.width,
                            )
                        }
                        Axis::Vertical => {
                            assert_eq!(first.allocation.x, area.x, "{label}");
                            assert_eq!(first.allocation.width, area.width, "{label}");
                            assert_eq!(second.allocation.width, area.width, "{label}");
                            assert_eq!(
                                first.allocation.height + lane_width + second.allocation.height,
                                area.height,
                                "{label}"
                            );
                            (
                                bottom(first.allocation),
                                second.allocation.y,
                                area.height,
                                first.allocation.height,
                            )
                        }
                    };
                    assert_eq!(second_start - first_end, lane_width, "{label}");
                    assert_eq!(first_extent, (extent - lane_width) / 2, "{label}");
                    assert_eq!(layout.splits[0].boundary, first_end, "{label}");
                    assert_eq!(layout.splits[0].separator, lane_width, "{label}");
                    for pane in [first, second] {
                        assert!(within(pane.rect, pane.allocation), "{label}");
                        assert!(within(pane.allocation, pane.extent), "{label}");
                        assert!(within(pane.extent, area), "{label}");
                        if chrome == FRAME {
                            // Each pane keeps its whole frame; the gap is outside it.
                            assert_eq!(
                                pane.rect,
                                inset_frame(pane.allocation, 8, 16).unwrap(),
                                "{label}"
                            );
                        } else {
                            assert_eq!(pane.rect, pane.allocation, "{label}");
                        }
                    }
                    assert!(!overlaps(first.allocation, second.allocation), "{label}");

                    // The divider line, when there is one, sits between the
                    // gap's halves; the extents reach it (or meet midway).
                    let divider_start = first_end + gap / 2;
                    let (first_extent_end, second_extent_start) = match axis {
                        Axis::Horizontal => (right(first.extent), second.extent.x),
                        Axis::Vertical => (bottom(first.extent), second.extent.y),
                    };
                    assert_eq!(first_extent_end, divider_start, "{label}");
                    assert_eq!(second_extent_start, divider_start + line, "{label}");
                    if line == 0 {
                        assert!(layout.separators.is_empty(), "{label}");
                    } else {
                        let divider = layout.separators[0].rect;
                        let expected = match axis {
                            Axis::Horizontal => Rect {
                                x: divider_start,
                                y: area.y,
                                width: line,
                                height: area.height,
                            },
                            Axis::Vertical => Rect {
                                x: area.x,
                                y: divider_start,
                                width: area.width,
                                height: line,
                            },
                        };
                        assert_eq!(divider, expected, "{label}");
                        assert!(!overlaps(divider, first.allocation), "{label}");
                        assert!(!overlaps(divider, second.allocation), "{label}");
                    }

                    // Every point across the lane, gap included, is a drag
                    // handle and belongs to no pane.
                    let orthogonal = 150.0;
                    for offset in 0..lane_width {
                        let primary = f64::from(first_end + offset) + 0.5;
                        let point = match axis {
                            Axis::Horizontal => (primary, orthogonal),
                            Axis::Vertical => (orthogonal, primary),
                        };
                        assert_eq!(
                            layout.split_at(point, 6),
                            Some(layout.splits[0]),
                            "{label} +{offset}"
                        );
                        assert!(
                            layout
                                .panes
                                .iter()
                                .all(|pane| !contains_point(pane.rect, point)),
                            "{label} +{offset}"
                        );
                    }

                    // Dragging puts the lane's middle under the pointer.
                    let start = match axis {
                        Axis::Horizontal => area.x,
                        Axis::Vertical => area.y,
                    };
                    let target = start + 150;
                    let point = match axis {
                        Axis::Horizontal => (f64::from(target), orthogonal),
                        Axis::Vertical => (orthogonal, f64::from(target)),
                    };
                    let ratio = split_ratio_at(layout.splits[0], point, 10).unwrap();
                    assert!(apply_preview_ratio(&mut root, first_id, 0, ratio));
                    let dragged =
                        PaneLayout::compute_with_gap(&root, area, chrome, gap, 10, 10).unwrap();
                    let middle =
                        f64::from(dragged.splits[0].boundary) + f64::from(lane_width) / 2.0;
                    let quantum = f64::from(extent - lane_width) / 1000.0;
                    assert!(
                        (middle - f64::from(target)).abs() <= quantum + 2.0,
                        "{label}: {middle} for {target}"
                    );
                }
            }
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one sweep checks tiling, exact gaps, junctions and grids per style and scale"
    )]
    fn pane_gap_nested_layouts_never_overlap_at_any_output_scale() {
        let area = Rect {
            x: 0,
            y: 30,
            width: 1000,
            height: 700,
        };
        for chrome in CHROMES {
            for gap in [0, 3, 8, 64] {
                for (tree_index, tree) in nested_trees().into_iter().enumerate() {
                    let label = format!("{chrome:?} gap {gap} tree {tree_index}");
                    let layout =
                        PaneLayout::compute_with_gap(&tree, area, chrome, gap, 16, 32).unwrap();
                    assert_eq!(layout.panes.len(), tree.splint_count(), "{label}");
                    let dividers = layout
                        .separators
                        .iter()
                        .map(|divider| divider.rect)
                        .collect::<Vec<_>>();

                    // Extents and divider lines tile the area exactly once.
                    let mut tiles = layout
                        .panes
                        .iter()
                        .map(|pane| pane.extent)
                        .chain(dividers.iter().copied())
                        .collect::<Vec<_>>();
                    let covered: u64 = tiles
                        .iter()
                        .map(|tile| u64::from(tile.width) * u64::from(tile.height))
                        .sum();
                    assert_eq!(
                        covered,
                        u64::from(area.width) * u64::from(area.height),
                        "{label}"
                    );
                    while let Some(tile) = tiles.pop() {
                        assert!(within(tile, area), "{label}");
                        for other in &tiles {
                            assert!(!overlaps(tile, *other), "{label}: {tile:?} {other:?}");
                        }
                    }

                    // Each split's lane is exactly its line plus the gap, with a
                    // pane edge on each side and no pane inside it.
                    for split in &layout.splits {
                        let lane = line_width(chrome, split.axis) + gap;
                        assert_eq!(split.separator, lane, "{label}");
                        let lane_end = split.boundary + lane;
                        let inside = layout
                            .panes
                            .iter()
                            .filter(|pane| within(pane.allocation, split.area))
                            .map(|pane| match split.axis {
                                Axis::Horizontal => (pane.allocation.x, right(pane.allocation)),
                                Axis::Vertical => (pane.allocation.y, bottom(pane.allocation)),
                            })
                            .collect::<Vec<_>>();
                        assert!(
                            inside
                                .iter()
                                .all(|&(start, end)| end <= split.boundary || start >= lane_end),
                            "{label}"
                        );
                        assert!(
                            inside.iter().any(|&(_, end)| end == split.boundary),
                            "{label}"
                        );
                        assert!(
                            inside.iter().any(|&(start, _)| start == lane_end),
                            "{label}"
                        );
                    }

                    // Line ends meet a perpendicular line or the area's edge, so
                    // every junction is still found and drawn.
                    for divider in &layout.separators {
                        let rect = divider.rect;
                        let meets = |at_start: bool| {
                            layout.separators.iter().any(|other| {
                                other.axis != divider.axis
                                    && match divider.axis {
                                        Axis::Vertical => {
                                            (if at_start {
                                                right(other.rect) == rect.x
                                            } else {
                                                other.rect.x == right(rect)
                                            }) && other.rect.y <= rect.y
                                                && bottom(rect) <= bottom(other.rect)
                                        }
                                        Axis::Horizontal => {
                                            (if at_start {
                                                bottom(other.rect) == rect.y
                                            } else {
                                                other.rect.y == bottom(rect)
                                            }) && other.rect.x <= rect.x
                                                && right(rect) <= right(other.rect)
                                        }
                                    }
                            })
                        };
                        let (start_edge, end_edge) = match divider.axis {
                            Axis::Vertical => (rect.x == area.x, right(rect) == right(area)),
                            Axis::Horizontal => (rect.y == area.y, bottom(rect) == bottom(area)),
                        };
                        assert!(start_edge || meets(true), "{label}: {divider:?}");
                        assert!(end_edge || meets(false), "{label}: {divider:?}");
                    }

                    for pane in &layout.panes {
                        assert!(within(pane.rect, pane.allocation), "{label}");
                        assert!(within(pane.allocation, pane.extent), "{label}");
                        // The PTY grid comes from the content rectangle, which
                        // never reaches into a gap or a line.
                        assert!(pane.rect.width / 8 >= 2, "{label}");
                        assert!(pane.rect.height / 16 >= 2, "{label}");
                        for divider in &dividers {
                            assert!(!overlaps(pane.allocation, *divider), "{label}");
                        }
                    }

                    // The same holds for the buffer rectangles at each scale,
                    // and a gap scales with the output.
                    for scale in SCALES {
                        let rects = layout
                            .panes
                            .iter()
                            .map(|pane| buffer(pane.allocation, scale))
                            .chain(dividers.iter().map(|rect| buffer(*rect, scale)))
                            .collect::<Vec<_>>();
                        for (index, first) in rects.iter().enumerate() {
                            for second in &rects[index + 1..] {
                                assert!(!overlaps(*first, *second), "{label} @{scale}");
                            }
                        }
                        for split in &layout.splits {
                            let lane = split.separator;
                            let start =
                                crate::geometry::logical_extent_to_buffer(split.boundary, scale)
                                    .unwrap();
                            let end = crate::geometry::logical_extent_to_buffer(
                                split.boundary + lane,
                                scale,
                            )
                            .unwrap();
                            let scaled = u64::from(lane) * u64::from(scale);
                            assert!(
                                u64::from(end - start) * 120 >= scaled.saturating_sub(119),
                                "{label} @{scale}"
                            );
                            assert!(
                                u64::from(end - start) * 120 <= scaled + 119,
                                "{label} @{scale}"
                            );
                            if scale % 120 == 0 {
                                assert_eq!(end - start, lane * (scale / 120), "{label} @{scale}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pane_gap_directional_resize_and_minimums_account_for_the_gap() {
        let (left, left_id) = leaf();
        let (right_leaf, right_id) = leaf();
        let root = branch(Axis::Horizontal, 500, left, right_leaf);
        let area = Rect {
            x: 0,
            y: 0,
            width: 1_000,
            height: 400,
        };
        let layout = PaneLayout::compute_with_gap(&root, area, PaneChrome::None, 8, 1, 1).unwrap();
        assert_eq!(layout.panes[0].rect.width, 496);
        assert_eq!(layout.panes[1].rect.x, 504);
        let metrics = PaneResizeMetrics {
            cell_width: 10,
            cell_height: 20,
            minimum_width: 20,
            minimum_height: 40,
        };
        assert_eq!(
            directional_resize_ratio(&root, &layout, left_id, FocusDirection::Right, 5, metrics),
            Some((left_id, 0, SplitRatio::new(550).unwrap()))
        );
        assert_eq!(
            directional_resize_ratio(&root, &layout, right_id, FocusDirection::Left, 5, metrics),
            Some((left_id, 0, SplitRatio::new(449).unwrap()))
        );
        // A gap the panes cannot spare is rejected rather than squeezed.
        assert!(PaneLayout::compute_with_gap(&root, area, FRAME, 64, 470, 10).is_err());
        assert!(PaneLayout::compute_with_gap(&root, area, FRAME, 0, 470, 10).is_ok());
        // A single pane has no neighbour and so no gap: the gap is never
        // applied at the area's outer edges.
        let (single, _) = leaf();
        let alone = PaneLayout::compute_with_gap(&single, area, FRAME, 64, 10, 10).unwrap();
        assert_eq!(alone.panes[0].allocation, area);
        assert_eq!(alone.panes[0].extent, area);
    }

    #[test]
    fn preferred_gap_shrinks_to_the_largest_gap_that_fits_and_then_fails_as_gapless() {
        let (left, _) = leaf();
        let (right_leaf, _) = leaf();
        let root = branch(Axis::Horizontal, 500, left, right_leaf);
        let area = |width| Rect {
            x: 0,
            y: 0,
            width,
            height: 300,
        };
        let preferred = |width| {
            PaneLayout::compute_with_preferred_gap(
                &root,
                area(width),
                PaneChrome::None,
                64,
                100,
                10,
            )
        };
        // Room for the whole gap: it is used as configured.
        assert_eq!(preferred(400).unwrap().gap, 64);
        assert_eq!(
            preferred(400).unwrap(),
            PaneLayout::compute_with_gap(&root, area(400), PaneChrome::None, 64, 100, 10).unwrap()
        );
        // Two 100 px panes in 230 px leave room for a 30 px gap, not 31.
        let shrunk = preferred(230).unwrap();
        assert_eq!(shrunk.gap, 30);
        assert_eq!(shrunk.panes[0].rect.width, 100);
        assert_eq!(shrunk.panes[1].rect.x, 130);
        // Only the gapless layout fits.
        let gapless = preferred(200).unwrap();
        assert_eq!(gapless.gap, 0);
        assert_eq!(
            gapless,
            PaneLayout::compute_with_gap(&root, area(200), PaneChrome::None, 0, 100, 10).unwrap()
        );
        // Nothing fits: the gapless layout's own error, as before the gap.
        let error = preferred(199).unwrap_err().to_string();
        assert_eq!(
            error,
            PaneLayout::compute_with_gap(&root, area(199), PaneChrome::None, 0, 100, 10)
                .unwrap_err()
                .to_string()
        );
        // A zero preference is the plain gapless computation.
        assert!(
            PaneLayout::compute_with_preferred_gap(&root, area(199), PaneChrome::None, 0, 100, 10)
                .is_err()
        );
    }

    #[test]
    fn preferred_gap_in_tight_nested_layouts_is_the_largest_fit_at_any_output_scale() {
        let (mut whole, mut shrunk, mut gapless, mut failed) = (0, 0, 0, 0);
        for scale in SCALES {
            // A 17×34 buffer-px cell in logical px at this scale, as
            // `computed_pane_layout` derives the chrome and the minimums.
            let cell_width = crate::geometry::buffer_to_logical_ceil(17, scale).unwrap();
            let cell_height = crate::geometry::buffer_to_logical_ceil(34, scale).unwrap();
            let chromes = [
                PaneChrome::None,
                PaneChrome::Line {
                    vertical_width: cell_width,
                    horizontal_height: cell_height,
                },
                PaneChrome::Frame {
                    vertical_width: cell_width,
                    horizontal_height: cell_height,
                },
            ];
            for chrome in chromes {
                for (tree_index, tree) in nested_trees().into_iter().enumerate() {
                    for width in (80..=520).step_by(20) {
                        let height = width * 3 / 4;
                        let label =
                            format!("{chrome:?} tree {tree_index} {width}×{height} @{scale}");
                        let area = Rect {
                            x: 0,
                            y: 24,
                            width,
                            height,
                        };
                        let (minimum_width, minimum_height) = (cell_width * 2, cell_height * 2);
                        let at = |gap| {
                            PaneLayout::compute_with_gap(
                                &tree,
                                area,
                                chrome,
                                gap,
                                minimum_width,
                                minimum_height,
                            )
                        };
                        let Ok(layout) = PaneLayout::compute_with_preferred_gap(
                            &tree,
                            area,
                            chrome,
                            64,
                            minimum_width,
                            minimum_height,
                        ) else {
                            assert!(at(0).is_err(), "{label}");
                            failed += 1;
                            continue;
                        };
                        // The largest gap that fits: every smaller gap fits too,
                        // and no larger one does.
                        assert_eq!(layout, at(layout.gap).unwrap(), "{label}");
                        for gap in 0..=64 {
                            assert_eq!(at(gap).is_ok(), gap <= layout.gap, "{label} gap {gap}");
                        }
                        match layout.gap {
                            64 => whole += 1,
                            0 => gapless += 1,
                            _ => shrunk += 1,
                        }
                        let rects = layout
                            .panes
                            .iter()
                            .map(|pane| pane.allocation)
                            .chain(layout.separators.iter().map(|divider| divider.rect))
                            .collect::<Vec<_>>();
                        for (index, first) in rects.iter().enumerate() {
                            assert!(within(*first, area), "{label}");
                            for second in &rects[index + 1..] {
                                assert!(!overlaps(*first, *second), "{label}");
                                assert!(
                                    !overlaps(buffer(*first, scale), buffer(*second, scale)),
                                    "{label}"
                                );
                            }
                        }
                    }
                }
            }
        }
        // The sweep reaches every outcome.
        assert!(whole > 0 && shrunk > 0 && gapless > 0 && failed > 0);
    }
}
