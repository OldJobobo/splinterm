//! Terminal cursor policy, colors, geometry, and painting.

use crate::config::CursorStyle;

use super::{DecorationMetrics, SnapshotFrame, raster::fill_rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnfocusedCursorStyle {
    Unchanged,
    Hollow,
    None,
}

impl UnfocusedCursorStyle {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Hollow => "hollow",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorPresentation {
    pub keyboard_focused: bool,
    pub unfocused_style: UnfocusedCursorStyle,
}

impl CursorPresentation {
    pub const FOCUSED_STEADY: Self = Self {
        keyboard_focused: true,
        unfocused_style: UnfocusedCursorStyle::Unchanged,
    };
    pub const INACTIVE_PANE: Self = Self {
        keyboard_focused: false,
        unfocused_style: UnfocusedCursorStyle::None,
    };

    #[must_use]
    pub const fn for_keyboard_focus(keyboard_focused: bool) -> Self {
        Self {
            keyboard_focused,
            unfocused_style: UnfocusedCursorStyle::Hollow,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveCursorShape {
    Block,
    Beam,
    Underline,
    Hollow,
    None,
}

impl EffectiveCursorShape {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Beam => "beam",
            Self::Underline => "underline",
            Self::Hollow => "hollow",
            Self::None => "none",
        }
    }
}

#[must_use]
pub const fn effective_cursor_shape(
    configured: CursorStyle,
    visible: bool,
    presentation: CursorPresentation,
) -> EffectiveCursorShape {
    if !visible {
        return EffectiveCursorShape::None;
    }
    if !presentation.keyboard_focused {
        match presentation.unfocused_style {
            UnfocusedCursorStyle::Hollow => return EffectiveCursorShape::Hollow,
            UnfocusedCursorStyle::None => return EffectiveCursorShape::None,
            UnfocusedCursorStyle::Unchanged => {}
        }
    }
    match configured {
        CursorStyle::Block => EffectiveCursorShape::Block,
        CursorStyle::Beam => EffectiveCursorShape::Beam,
        CursorStyle::Underline => EffectiveCursorShape::Underline,
    }
}

pub(super) fn cursor_colors_for_cell(
    explicit_cursor: Option<[u8; 3]>,
    foreground: [u8; 3],
    background: [u8; 3],
) -> ([u8; 3], [u8; 3]) {
    let mut cursor = explicit_cursor.unwrap_or(foreground);
    let mut text = background;
    if cursor == text {
        text = background;
        cursor = foreground;
        if cursor == text {
            cursor = cursor.map(|channel| !channel);
        }
    }
    (cursor, text)
}

pub(super) fn cursor_span(frame: &SnapshotFrame, column: u32, row: u32) -> u32 {
    usize::try_from(row * frame.columns + column)
        .ok()
        .and_then(|index| frame.cell_spans.get(index))
        .copied()
        .unwrap_or(1)
        .max(1)
}

#[allow(
    clippy::too_many_arguments,
    reason = "cursor geometry is an explicit Foot contract"
)]
pub(super) fn paint_effective_cursor(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    x: i32,
    y: i32,
    span: u32,
    metrics: DecorationMetrics,
    color: [u8; 4],
    shape: EffectiveCursorShape,
) {
    let cursor_width = frame.cell_width.saturating_mul(span);
    match shape {
        EffectiveCursorShape::Block => fill_rect(
            canvas,
            width,
            height,
            (x, y, cursor_width, frame.cell_height),
            color,
        ),
        EffectiveCursorShape::Beam => {
            let thickness = (2 * u32::from(frame.scale_120) + 60) / 120;
            fill_rect(
                canvas,
                width,
                height,
                (
                    x,
                    y + frame.baseline - i32::try_from(frame.ascent).unwrap_or(i32::MAX),
                    thickness,
                    frame.ascent.saturating_add(frame.descent),
                ),
                color,
            );
        }
        EffectiveCursorShape::Underline => {
            let thickness = metrics.underline_thickness.max(1);
            let natural = frame
                .baseline
                .saturating_sub(metrics.underline_position)
                .saturating_add(i32::try_from(thickness).unwrap_or(i32::MAX));
            let maximum =
                i32::try_from(frame.cell_height.saturating_sub(thickness)).unwrap_or(i32::MAX);
            fill_rect(
                canvas,
                width,
                height,
                (x, y + natural.min(maximum), cursor_width, thickness),
                color,
            );
        }
        EffectiveCursorShape::Hollow => {
            let border = (u32::from(frame.scale_120) + 60) / 120;
            let border = border.min(frame.cell_height).min(cursor_width);
            for rect in [
                (x, y, cursor_width, border),
                (x, y, border, frame.cell_height),
                (
                    x + i32::try_from(cursor_width.saturating_sub(border)).unwrap_or(0),
                    y,
                    border,
                    frame.cell_height,
                ),
                (
                    x,
                    y + i32::try_from(frame.cell_height.saturating_sub(border)).unwrap_or(0),
                    cursor_width,
                    border,
                ),
            ] {
                fill_rect(canvas, width, height, rect, color);
            }
        }
        EffectiveCursorShape::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_focus_policy_and_cell_relative_color_fallback_are_truthful() {
        for style in [
            CursorStyle::Block,
            CursorStyle::Beam,
            CursorStyle::Underline,
        ] {
            assert_eq!(
                effective_cursor_shape(style, true, CursorPresentation::for_keyboard_focus(false)),
                EffectiveCursorShape::Hollow
            );
            assert_eq!(
                effective_cursor_shape(style, true, CursorPresentation::INACTIVE_PANE),
                EffectiveCursorShape::None
            );
        }
        assert_eq!(
            cursor_colors_for_cell(None, [1, 2, 3], [4, 5, 6]),
            ([1, 2, 3], [4, 5, 6])
        );
        assert_eq!(
            cursor_colors_for_cell(Some([9, 9, 9]), [1, 2, 3], [9, 9, 9]),
            ([1, 2, 3], [9, 9, 9])
        );
        assert_eq!(
            cursor_colors_for_cell(None, [7, 7, 7], [7, 7, 7]),
            ([248, 248, 248], [7, 7, 7])
        );
    }
}
