//! Responsive layout and CPU composition for the non-modal Lair explorer.

use std::collections::HashSet;

use anyhow::Result;

use crate::{frontend::LairExplorerRow, geometry::Rect, navigation_projection::NavigationNodeId};

use super::{
    super::{ChromeTextStyle, RenderContext, fill_rect},
    picker::{
        PickerTextAlignment, SessionPickerPalette, SessionPickerTextCache, paint_picker_text,
    },
};

const PREFERRED_WIDTH: u32 = 288;
const HEADER_HEIGHT: u32 = 76;
const FOOTER_HEIGHT: u32 = 30;
const ROW_HEIGHT: u32 = 34;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LairExplorerPresentationMode {
    Docked,
    Drawer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LairExplorerRowLayout {
    pub(crate) id: NavigationNodeId,
    pub(crate) rect: Rect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LairExplorerLayout {
    pub(crate) panel: Rect,
    pub(crate) terminal: Rect,
    pub(crate) header: Rect,
    pub(crate) footer: Rect,
    pub(crate) rows: Vec<LairExplorerRowLayout>,
    pub(crate) visible_start: usize,
    pub(crate) visible_capacity: usize,
    pub(crate) mode: LairExplorerPresentationMode,
}

#[must_use]
pub(crate) fn lair_explorer_layout(
    content: Rect,
    minimum_terminal_width: u32,
    rows: &[LairExplorerRow],
    selected: Option<NavigationNodeId>,
    requested_start: usize,
) -> Option<LairExplorerLayout> {
    if content.width == 0 || content.height == 0 {
        return None;
    }
    let width = PREFERRED_WIDTH.min(content.width);
    let mode = if content.width.saturating_sub(width) >= minimum_terminal_width {
        LairExplorerPresentationMode::Docked
    } else {
        LairExplorerPresentationMode::Drawer
    };
    let panel = Rect { width, ..content };
    let terminal = match mode {
        LairExplorerPresentationMode::Docked => Rect {
            x: content.x.saturating_add(width),
            y: content.y,
            width: content.width.saturating_sub(width),
            height: content.height,
        },
        LairExplorerPresentationMode::Drawer => content,
    };
    let header_height = HEADER_HEIGHT.min(panel.height);
    let footer_height = FOOTER_HEIGHT.min(panel.height.saturating_sub(header_height));
    let list_height = panel
        .height
        .saturating_sub(header_height)
        .saturating_sub(footer_height);
    let capacity = usize::try_from(list_height / ROW_HEIGHT).unwrap_or(0);
    let selected_index = selected.and_then(|id| rows.iter().position(|row| row.id == id));
    let maximum_start = rows.len().saturating_sub(capacity);
    let mut start = requested_start.min(maximum_start);
    if let Some(index) = selected_index {
        if index < start {
            start = index;
        } else if index >= start.saturating_add(capacity) {
            start = index.saturating_add(1).saturating_sub(capacity);
        }
    }
    let row_layouts = rows
        .iter()
        .skip(start)
        .take(capacity)
        .enumerate()
        .map(|(slot, row)| LairExplorerRowLayout {
            id: row.id,
            rect: Rect {
                x: panel.x,
                y: panel.y.saturating_add(header_height).saturating_add(
                    ROW_HEIGHT.saturating_mul(u32::try_from(slot).unwrap_or(u32::MAX)),
                ),
                width: panel.width,
                height: ROW_HEIGHT,
            },
        })
        .collect();
    Some(LairExplorerLayout {
        panel,
        terminal,
        header: Rect {
            x: panel.x,
            y: panel.y,
            width: panel.width,
            height: header_height,
        },
        footer: Rect {
            x: panel.x,
            y: panel
                .y
                .saturating_add(panel.height.saturating_sub(footer_height)),
            width: panel.width,
            height: footer_height,
        },
        rows: row_layouts,
        visible_start: start,
        visible_capacity: capacity,
        mode,
    })
}

#[must_use]
pub(crate) fn lair_explorer_hit_test(
    layout: &LairExplorerLayout,
    position: (f64, f64),
) -> Option<NavigationNodeId> {
    layout.rows.iter().find_map(|row| {
        let right = f64::from(row.rect.x.saturating_add(row.rect.width));
        let bottom = f64::from(row.rect.y.saturating_add(row.rect.height));
        (position.0 >= f64::from(row.rect.x)
            && position.0 < right
            && position.1 >= f64::from(row.rect.y)
            && position.1 < bottom)
            .then_some(row.id)
    })
}

#[must_use]
pub(crate) fn lair_explorer_disclosure_hit_test(
    layout: &LairExplorerLayout,
    rows: &[LairExplorerRow],
    position: (f64, f64),
) -> Option<NavigationNodeId> {
    layout
        .rows
        .iter()
        .zip(rows.iter().skip(layout.visible_start))
        .find_map(|(layout_row, row)| {
            row.expanded?;
            let bottom = f64::from(layout_row.rect.y.saturating_add(layout_row.rect.height));
            let disclosure_width = 12_u32.saturating_mul(
                u32::try_from(row.level)
                    .unwrap_or(u32::MAX)
                    .saturating_add(3),
            );
            let right = f64::from(
                layout_row
                    .rect
                    .x
                    .saturating_add(disclosure_width.min(layout_row.rect.width)),
            );
            (position.0 >= f64::from(layout_row.rect.x)
                && position.0 < right
                && position.1 >= f64::from(layout_row.rect.y)
                && position.1 < bottom)
                .then_some(row.id)
        })
}

fn buffer_rect(rect: Rect, scale_120: u32) -> Rect {
    let left = rect.x.saturating_mul(scale_120).div_ceil(120);
    let top = rect.y.saturating_mul(scale_120).div_ceil(120);
    let right = rect
        .x
        .saturating_add(rect.width)
        .saturating_mul(scale_120)
        .div_ceil(120);
    let bottom = rect
        .y
        .saturating_add(rect.height)
        .saturating_mul(scale_120)
        .div_ceil(120);
    Rect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    }
}

fn tuple(rect: Rect) -> (i32, i32, u32, u32) {
    (
        i32::try_from(rect.x).unwrap_or(i32::MAX),
        i32::try_from(rect.y).unwrap_or(i32::MAX),
        rect.width,
        rect.height,
    )
}

fn rgba(color: u32) -> [u8; 4] {
    [
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
        0xff,
    ]
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn paint_lair_explorer(
    cache: &mut SessionPickerTextCache,
    context: &RenderContext,
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    scale_120: u32,
    renderer_generation: u64,
    layout: &LairExplorerLayout,
    palette: SessionPickerPalette,
    rows: &[LairExplorerRow],
    selected: Option<NavigationNodeId>,
    breadcrumb: Option<&str>,
    query: &str,
    status_message: Option<&str>,
    focused: bool,
) -> Result<()> {
    let panel = buffer_rect(layout.panel, scale_120);
    fill_rect(
        canvas,
        canvas_width,
        canvas_height,
        tuple(panel),
        rgba(palette.panel),
    );
    let border = scale_120.div_ceil(120).max(1);
    let right = panel.x.saturating_add(panel.width.saturating_sub(border));
    fill_rect(
        canvas,
        canvas_width,
        canvas_height,
        (
            i32::try_from(right).unwrap_or(i32::MAX),
            i32::try_from(panel.y).unwrap_or(i32::MAX),
            border,
            panel.height,
        ),
        rgba(if focused {
            palette.focused_frame
        } else {
            palette.frame
        }),
    );
    let mut used = HashSet::new();
    let header = buffer_rect(layout.header, scale_120);
    let inset = 12_u32.saturating_mul(scale_120).div_ceil(120);
    let header_line = Rect {
        x: header.x.saturating_add(inset),
        y: header.y,
        width: header.width.saturating_sub(inset.saturating_mul(2)),
        height: header.height / 2,
    };
    paint_picker_text(
        cache,
        context,
        &mut used,
        canvas,
        canvas_width,
        canvas_height,
        "LAIRS",
        ChromeTextStyle::Bold,
        scale_120,
        renderer_generation,
        header_line,
        PickerTextAlignment::Left,
        palette.primary,
    )?;
    let context_line = Rect {
        y: header.y.saturating_add(header.height / 2),
        ..header_line
    };
    let context_text = if let Some(status_message) = status_message {
        status_message
    } else if query.is_empty() {
        breadcrumb.unwrap_or("No current Dojo")
    } else {
        query
    };
    paint_picker_text(
        cache,
        context,
        &mut used,
        canvas,
        canvas_width,
        canvas_height,
        context_text,
        ChromeTextStyle::Regular,
        scale_120,
        renderer_generation,
        context_line,
        PickerTextAlignment::Left,
        palette.secondary,
    )?;

    for (layout_row, row) in layout
        .rows
        .iter()
        .zip(rows.iter().skip(layout.visible_start))
    {
        let rect = buffer_rect(layout_row.rect, scale_120);
        if selected == Some(row.id) {
            fill_rect(
                canvas,
                canvas_width,
                canvas_height,
                tuple(rect),
                rgba(palette.selected_fill),
            );
            fill_rect(
                canvas,
                canvas_width,
                canvas_height,
                (
                    i32::try_from(rect.x).unwrap_or(i32::MAX),
                    i32::try_from(rect.y).unwrap_or(i32::MAX),
                    border.saturating_mul(3),
                    rect.height,
                ),
                rgba(palette.selected_rail),
            );
        }
        let marker = if row.pending {
            "  …"
        } else {
            match (row.expanded, row.current) {
                (Some(true), true) => "● ▾",
                (Some(false), true) => "● ▸",
                (Some(true), false) => "  ▾",
                (Some(false), false) => "  ▸",
                (None, true) => "  ●",
                (None, false) => "   ",
            }
        };
        let source = format!("{marker} {}", row.label);
        let row_inset = inset.saturating_add(
            u32::try_from(row.level.saturating_sub(1))
                .unwrap_or(u32::MAX)
                .saturating_mul(inset),
        );
        let label_clip = Rect {
            x: rect.x.saturating_add(row_inset),
            y: rect.y,
            width: rect.width.saturating_sub(row_inset).saturating_mul(3) / 5,
            height: rect.height,
        };
        let status_clip = Rect {
            x: label_clip.x.saturating_add(label_clip.width),
            y: rect.y,
            width: rect
                .x
                .saturating_add(rect.width)
                .saturating_sub(label_clip.x.saturating_add(label_clip.width))
                .saturating_sub(inset),
            height: rect.height,
        };
        let primary = if selected == Some(row.id) {
            if row.enabled {
                palette.selected_primary
            } else {
                palette.selected_secondary
            }
        } else if row.enabled {
            palette.primary
        } else {
            palette.secondary
        };
        let secondary = if selected == Some(row.id) {
            palette.selected_secondary
        } else {
            palette.secondary
        };
        paint_picker_text(
            cache,
            context,
            &mut used,
            canvas,
            canvas_width,
            canvas_height,
            &source,
            ChromeTextStyle::Regular,
            scale_120,
            renderer_generation,
            label_clip,
            PickerTextAlignment::Left,
            primary,
        )?;
        paint_picker_text(
            cache,
            context,
            &mut used,
            canvas,
            canvas_width,
            canvas_height,
            &row.status,
            ChromeTextStyle::Regular,
            scale_120,
            renderer_generation,
            status_clip,
            PickerTextAlignment::Right,
            secondary,
        )?;
    }
    let footer = buffer_rect(layout.footer, scale_120);
    paint_picker_text(
        cache,
        context,
        &mut used,
        canvas,
        canvas_width,
        canvas_height,
        "↑↓ move  ←→ open  R reveal  / search  Esc terminal",
        ChromeTextStyle::Regular,
        scale_120,
        renderer_generation,
        Rect {
            x: footer.x.saturating_add(inset),
            width: footer.width.saturating_sub(inset.saturating_mul(2)),
            ..footer
        },
        PickerTextAlignment::Left,
        palette.secondary,
    )?;
    cache.finish_frame(used);
    Ok(())
}

#[cfg(test)]
mod tests {
    use splinterm_core::LairId;

    use super::*;
    use crate::frontend::LairExplorerRowKind;

    fn row(id: NavigationNodeId) -> LairExplorerRow {
        LairExplorerRow {
            id,
            kind: LairExplorerRowKind::Lair,
            label: "work".into(),
            status: "Saved".into(),
            level: 1,
            expanded: Some(false),
            current: false,
            enabled: true,
            pending: false,
        }
    }

    #[test]
    fn dock_preserves_minimum_terminal_width_and_narrow_surface_draws_over() {
        let id = NavigationNodeId::Lair(LairId::new());
        let rows = [row(id)];
        let dock = lair_explorer_layout(
            Rect {
                x: 0,
                y: 20,
                width: 1000,
                height: 600,
            },
            640,
            &rows,
            Some(id),
            0,
        )
        .unwrap();
        assert_eq!(dock.mode, LairExplorerPresentationMode::Docked);
        assert!(dock.terminal.width >= 640);
        let drawer = lair_explorer_layout(
            Rect {
                x: 0,
                y: 20,
                width: 700,
                height: 600,
            },
            640,
            &rows,
            Some(id),
            0,
        )
        .unwrap();
        assert_eq!(drawer.mode, LairExplorerPresentationMode::Drawer);
        assert_eq!(drawer.terminal.width, 700);
        assert_eq!(lair_explorer_hit_test(&drawer, (10.0, 110.0)), Some(id));
        assert_eq!(
            lair_explorer_disclosure_hit_test(&drawer, &rows, (10.0, 110.0)),
            Some(id)
        );
        assert_eq!(
            lair_explorer_disclosure_hit_test(&drawer, &rows, (100.0, 110.0)),
            None
        );
    }
}
