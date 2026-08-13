//! Trusted command-palette layout and CPU composition.

use std::collections::HashMap;

use anyhow::Result;

use crate::{
    frontend::{
        BindingHelpUi, BuiltInCommandId, COMMAND_PALETTE_PAGE_ITEMS, CommandPaletteUi,
        DojoPromptUi, TAB_MENU_ACTIONS, TabContextMenuUi, TabMenuActionId, TerminationDecision,
        command_descriptor, tab_menu_descriptor,
    },
    geometry::Rect,
    keymap::ResolvedKeymap,
};

use super::{
    super::{ChromeText, ChromeTextStyle, RenderContext, blend_rect, fill_rect},
    picker::SessionPickerPalette,
};

const HEADER_HEIGHT: u32 = 52;
const ROW_HEIGHT: u32 = 48;
const FOOTER_HEIGHT: u32 = 36;
const PANEL_MAX_WIDTH: u32 = 680;
const PANEL_HORIZONTAL_MARGIN: u32 = 24;
const PANEL_VERTICAL_MARGIN: u32 = 16;
const CONTENT_INSET: u32 = 24;
const ROW_GUTTER_X: u32 = 8;
const ROW_GUTTER_Y: u32 = 2;
const PANEL_TOP_OFFSET: u32 = 48;
const TAB_MENU_WIDTH: u32 = 156;
const TAB_MENU_ROW_HEIGHT: u32 = 28;
const TAB_MENU_PADDING: u32 = 2;
const TAB_MENU_CONTENT_INSET: u32 = 4;
const TAB_MENU_INDICATOR_WIDTH: u32 = 12;
const TAB_MENU_SHADOW: u32 = 6;
const TAB_MENU_ANCHOR_GAP: u32 = 4;
const PROMPT_MAX_WIDTH: u32 = 520;
const PROMPT_MARGIN: u32 = 24;
const PROMPT_PADDING: u32 = 24;
const PROMPT_TITLE_HEIGHT: u32 = 34;
const PROMPT_BODY_HEIGHT: u32 = 42;
const PREVIEW_BODY_HEIGHT: u32 = 168;
const PREVIEW_LINE_HEIGHT: u32 = 24;
const MAX_PREVIEW_LINES: usize = 7;
const MAX_PREVIEW_LINE_SCALARS: usize = 120;
const PROMPT_INPUT_HEIGHT: u32 = 38;
const PROMPT_BUTTON_HEIGHT: u32 = 34;
const PROMPT_GAP: u32 = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommandPaletteRowLayout {
    pub(crate) command: BuiltInCommandId,
    pub(crate) rect: Rect,
    pub(crate) category: Rect,
    pub(crate) title: Rect,
    pub(crate) shortcut: Rect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommandPaletteLayout {
    pub(crate) panel: Rect,
    pub(crate) header: Rect,
    pub(crate) list: Rect,
    pub(crate) footer: Rect,
    pub(crate) rows: Vec<CommandPaletteRowLayout>,
    pub(crate) visible_start: usize,
    pub(crate) compact: bool,
}

#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one pure layout seam keeps responsive geometry and hit targets coherent"
)]
pub(crate) fn command_palette_layout(
    content: Rect,
    filtered: &[BuiltInCommandId],
    selected: usize,
    requested_start: usize,
) -> Option<CommandPaletteLayout> {
    if content.width == 0 || content.height == 0 {
        return None;
    }
    let horizontal_margin = PANEL_HORIZONTAL_MARGIN.min(content.width / 4);
    let vertical_margin = PANEL_VERTICAL_MARGIN.min(content.height / 4);
    let available_width = content
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2));
    let available_height = content
        .height
        .saturating_sub(vertical_margin.saturating_mul(2));
    if available_width == 0 || available_height < HEADER_HEIGHT.saturating_add(FOOTER_HEIGHT) {
        return None;
    }
    let panel_width = available_width.min(PANEL_MAX_WIDTH);
    let available_list_height =
        available_height.saturating_sub(HEADER_HEIGHT.saturating_add(FOOTER_HEIGHT));
    let capacity = usize::try_from(available_list_height / ROW_HEIGHT)
        .unwrap_or(0)
        .min(COMMAND_PALETTE_PAGE_ITEMS);
    let visible_count = filtered.len().min(capacity);
    let mut visible_start = requested_start.min(filtered.len().saturating_sub(visible_count));
    if !filtered.is_empty() {
        let selected = selected.min(filtered.len() - 1);
        if selected < visible_start {
            visible_start = selected;
        } else if selected >= visible_start.saturating_add(visible_count) {
            visible_start = selected.saturating_add(1).saturating_sub(visible_count);
        }
    }
    let list_height = if filtered.is_empty() || visible_count == 0 {
        available_list_height.min(ROW_HEIGHT)
    } else {
        ROW_HEIGHT.saturating_mul(u32::try_from(visible_count).unwrap_or(u32::MAX))
    };
    let panel_height = HEADER_HEIGHT
        .saturating_add(list_height)
        .saturating_add(FOOTER_HEIGHT)
        .min(available_height);
    let x = content
        .x
        .saturating_add(content.width.saturating_sub(panel_width) / 2);
    let maximum_y = content
        .y
        .saturating_add(content.height.saturating_sub(panel_height));
    let y = content
        .y
        .saturating_add(PANEL_TOP_OFFSET.min(content.height.saturating_sub(panel_height)))
        .min(maximum_y);
    let panel = Rect {
        x,
        y,
        width: panel_width,
        height: panel_height,
    };
    let header = Rect {
        x,
        y,
        width: panel_width,
        height: HEADER_HEIGHT,
    };
    let list = Rect {
        x,
        y: y.saturating_add(HEADER_HEIGHT),
        width: panel_width,
        height: list_height,
    };
    let footer = Rect {
        x,
        y: list.y.saturating_add(list.height),
        width: panel_width,
        height: panel_height.saturating_sub(HEADER_HEIGHT.saturating_add(list_height)),
    };
    let compact = panel_width < 420;
    let rows = filtered
        .iter()
        .copied()
        .skip(visible_start)
        .take(visible_count)
        .enumerate()
        .map(|(slot, command)| {
            let slot_y = list
                .y
                .saturating_add(ROW_HEIGHT.saturating_mul(u32::try_from(slot).unwrap_or(u32::MAX)));
            let horizontal_gutter = ROW_GUTTER_X.min(panel_width / 6);
            let vertical_gutter = ROW_GUTTER_Y.min(ROW_HEIGHT / 4);
            let rect = Rect {
                x: x.saturating_add(horizontal_gutter),
                y: slot_y.saturating_add(vertical_gutter),
                width: panel_width.saturating_sub(horizontal_gutter.saturating_mul(2)),
                height: ROW_HEIGHT.saturating_sub(vertical_gutter.saturating_mul(2)),
            };
            let inset = CONTENT_INSET.min(rect.width / 6);
            let shortcut_width = if compact {
                0
            } else {
                (rect.width / 3).min(180)
            };
            let content_x = rect.x.saturating_add(inset).saturating_add(20);
            let category_width = if compact { 0 } else { 72.min(rect.width / 5) };
            let category_gap = if category_width == 0 { 0 } else { 12 };
            let title_x = content_x
                .saturating_add(category_width)
                .saturating_add(category_gap);
            CommandPaletteRowLayout {
                command,
                rect,
                category: Rect {
                    x: content_x,
                    y: rect.y,
                    width: category_width,
                    height: rect.height,
                },
                title: Rect {
                    x: title_x,
                    y: rect.y,
                    width: rect
                        .x
                        .saturating_add(rect.width)
                        .saturating_sub(title_x)
                        .saturating_sub(shortcut_width)
                        .saturating_sub(inset),
                    height: rect.height,
                },
                shortcut: Rect {
                    x: rect
                        .x
                        .saturating_add(rect.width)
                        .saturating_sub(shortcut_width)
                        .saturating_sub(inset),
                    y: rect.y,
                    width: shortcut_width,
                    height: rect.height,
                },
            }
        })
        .collect();
    Some(CommandPaletteLayout {
        panel,
        header,
        list,
        footer,
        rows,
        visible_start,
        compact,
    })
}

fn contains(rect: Rect, point: (f64, f64)) -> bool {
    point.0 >= f64::from(rect.x)
        && point.1 >= f64::from(rect.y)
        && point.0 < f64::from(rect.x.saturating_add(rect.width))
        && point.1 < f64::from(rect.y.saturating_add(rect.height))
}

#[must_use]
pub(crate) fn command_palette_hit_test(
    layout: &CommandPaletteLayout,
    point: (f64, f64),
) -> Option<BuiltInCommandId> {
    layout
        .rows
        .iter()
        .find(|row| contains(row.rect, point))
        .map(|row| row.command)
}

#[derive(Default)]
pub(crate) struct CommandPaletteTextCache {
    scale_120: u32,
    renderer_generation: u64,
    entries: HashMap<(String, ChromeTextStyle), ChromeText>,
}

impl CommandPaletteTextCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.scale_120 = 0;
        self.renderer_generation = 0;
    }

    fn text<'a>(
        &'a mut self,
        context: &RenderContext,
        source: &str,
        style: ChromeTextStyle,
        scale_120: u32,
        renderer_generation: u64,
    ) -> Result<&'a ChromeText> {
        if self.scale_120 != scale_120 || self.renderer_generation != renderer_generation {
            self.entries.clear();
            self.scale_120 = scale_120;
            self.renderer_generation = renderer_generation;
        }
        let key = (source.to_owned(), style);
        if !self.entries.contains_key(&key) {
            self.entries.insert(
                key.clone(),
                ChromeText::load_styled_with_context(source, scale_120, style, context)?,
            );
        }
        Ok(self
            .entries
            .get(&key)
            .expect("inserted command text remains"))
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

fn buffer_rect(rect: Rect, scale_120: u32) -> Rect {
    let scale = |value: u32| value.saturating_mul(scale_120).div_ceil(120);
    Rect {
        x: scale(rect.x),
        y: scale(rect.y),
        width: scale(rect.width),
        height: scale(rect.height),
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
        u8::try_from((color >> 16) & 0xff).unwrap_or(0),
        u8::try_from((color >> 8) & 0xff).unwrap_or(0),
        u8::try_from(color & 0xff).unwrap_or(0),
        u8::MAX,
    ]
}

#[allow(clippy::too_many_arguments)]
fn paint_text(
    cache: &mut CommandPaletteTextCache,
    context: &RenderContext,
    canvas: &mut [u8],
    width: u32,
    height: u32,
    source: &str,
    style: ChromeTextStyle,
    scale_120: u32,
    renderer_generation: u64,
    clip: Rect,
    color: u32,
    right_aligned: bool,
) -> Result<()> {
    let text = cache.text(context, source, style, scale_120, renderer_generation)?;
    let x = if right_aligned {
        clip.x
            .saturating_add(clip.width.saturating_sub(text.pixel_width()))
    } else {
        clip.x
    };
    let y = clip
        .y
        .saturating_add(clip.height.saturating_sub(text.pixel_height()) / 2);
    text.paint(canvas, width, height, (x, y), clip, color);
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn paint_command_palette(
    cache: &mut CommandPaletteTextCache,
    context: &RenderContext,
    canvas: &mut [u8],
    width: u32,
    height: u32,
    content_rect: Rect,
    scale_120: u32,
    renderer_generation: u64,
    layout: &CommandPaletteLayout,
    palette: SessionPickerPalette,
    state: &CommandPaletteUi,
    keymap: &ResolvedKeymap,
    pressed: Option<BuiltInCommandId>,
    keyboard_focused: bool,
    binding_help: Option<&BindingHelpUi>,
) -> Result<()> {
    blend_rect(
        canvas,
        width,
        height,
        tuple(buffer_rect(content_rect, scale_120)),
        [palette.scrim[0], palette.scrim[1], palette.scrim[2], 96],
    );
    let panel = buffer_rect(layout.panel, scale_120);
    let shadow_offset = 6_u32.saturating_mul(scale_120).div_ceil(120);
    blend_rect(
        canvas,
        width,
        height,
        (
            i32::try_from(panel.x.saturating_add(shadow_offset)).unwrap_or(i32::MAX),
            i32::try_from(panel.y.saturating_add(shadow_offset)).unwrap_or(i32::MAX),
            panel.width,
            panel.height,
        ),
        palette.shadow,
    );
    fill_rect(canvas, width, height, tuple(panel), rgba(palette.panel));
    let border = scale_120.div_ceil(120).max(1);
    let frame = if keyboard_focused {
        palette.focused_frame
    } else {
        palette.frame
    };
    for edge in [
        (panel.x, panel.y, panel.width, border),
        (
            panel.x,
            panel.y.saturating_add(panel.height.saturating_sub(border)),
            panel.width,
            border,
        ),
        (panel.x, panel.y, border, panel.height),
        (
            panel.x.saturating_add(panel.width.saturating_sub(border)),
            panel.y,
            border,
            panel.height,
        ),
    ] {
        fill_rect(
            canvas,
            width,
            height,
            (
                i32::try_from(edge.0).unwrap_or(i32::MAX),
                i32::try_from(edge.1).unwrap_or(i32::MAX),
                edge.2,
                edge.3,
            ),
            rgba(frame),
        );
    }
    for y in [
        layout.header.y.saturating_add(layout.header.height),
        layout.footer.y,
    ] {
        let divider_inset = CONTENT_INSET.min(layout.panel.width / 4);
        fill_rect(
            canvas,
            width,
            height,
            tuple(buffer_rect(
                Rect {
                    x: layout.panel.x.saturating_add(divider_inset),
                    y,
                    width: layout
                        .panel
                        .width
                        .saturating_sub(divider_inset.saturating_mul(2)),
                    height: 1,
                },
                scale_120,
            )),
            rgba(palette.frame),
        );
    }

    let header = buffer_rect(layout.header, scale_120);
    let inset = CONTENT_INSET.saturating_mul(scale_120).div_ceil(120);
    let title_width = 98_u32.saturating_mul(scale_120).div_ceil(120);
    paint_text(
        cache,
        context,
        canvas,
        width,
        height,
        if binding_help.is_some() {
            "KEYS"
        } else {
            "COMMANDS"
        },
        ChromeTextStyle::Bold,
        scale_120,
        renderer_generation,
        Rect {
            x: header.x.saturating_add(inset),
            y: header.y,
            width: title_width,
            height: header.height,
        },
        palette.primary,
        false,
    )?;
    let query = if binding_help.is_some() {
        format!("{} profile · generated bindings", keymap.profile().name())
    } else if state.query().is_empty() {
        "> Type a command".to_owned()
    } else {
        format!("> {}_", state.query())
    };
    paint_text(
        cache,
        context,
        canvas,
        width,
        height,
        &query,
        ChromeTextStyle::Regular,
        scale_120,
        renderer_generation,
        Rect {
            x: header.x.saturating_add(inset).saturating_add(title_width),
            y: header.y,
            width: header
                .width
                .saturating_sub(inset.saturating_mul(2))
                .saturating_sub(title_width),
            height: header.height,
        },
        if binding_help.is_some() || state.query().is_empty() {
            palette.secondary
        } else {
            palette.primary
        },
        false,
    )?;

    if state.filtered().is_empty() {
        let empty = buffer_rect(layout.list, scale_120);
        paint_text(
            cache,
            context,
            canvas,
            width,
            height,
            "No matching commands",
            ChromeTextStyle::Regular,
            scale_120,
            renderer_generation,
            Rect {
                x: empty.x.saturating_add(inset),
                width: empty.width.saturating_sub(inset.saturating_mul(2)),
                ..empty
            },
            palette.secondary,
            false,
        )?;
    }
    if let Some(help) = binding_help {
        for (offset, row) in layout.rows.iter().enumerate() {
            let index = layout.visible_start.saturating_add(offset);
            let Some(help_row) = help.rows().get(index) else {
                continue;
            };
            let rect = buffer_rect(row.rect, scale_120);
            let is_selected = index == help.selected_index();
            if is_selected {
                fill_rect(
                    canvas,
                    width,
                    height,
                    tuple(rect),
                    rgba(palette.selected_fill),
                );
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        i32::try_from(rect.x).unwrap_or(i32::MAX),
                        i32::try_from(rect.y).unwrap_or(i32::MAX),
                        3_u32.saturating_mul(scale_120).div_ceil(120).max(1),
                        rect.height,
                    ),
                    rgba(palette.selected_rail),
                );
            }
            let primary = if is_selected {
                palette.selected_primary
            } else {
                palette.primary
            };
            if row.category.width > 0 {
                paint_text(
                    cache,
                    context,
                    canvas,
                    width,
                    height,
                    &help_row.source,
                    ChromeTextStyle::Regular,
                    scale_120,
                    renderer_generation,
                    buffer_rect(row.category, scale_120),
                    if is_selected {
                        palette.selected_secondary
                    } else {
                        palette.secondary
                    },
                    false,
                )?;
            }
            paint_text(
                cache,
                context,
                canvas,
                width,
                height,
                if layout.compact {
                    &help_row.compact
                } else {
                    &help_row.action
                },
                if is_selected {
                    ChromeTextStyle::Bold
                } else {
                    ChromeTextStyle::Regular
                },
                scale_120,
                renderer_generation,
                buffer_rect(row.title, scale_120),
                primary,
                false,
            )?;
            if !layout.compact {
                paint_text(
                    cache,
                    context,
                    canvas,
                    width,
                    height,
                    &help_row.shortcut,
                    ChromeTextStyle::Regular,
                    scale_120,
                    renderer_generation,
                    buffer_rect(row.shortcut, scale_120),
                    if is_selected {
                        palette.selected_secondary
                    } else {
                        palette.secondary
                    },
                    true,
                )?;
            }
        }
    } else {
        let selected = state.selected_command();
        for row in &layout.rows {
            let rect = buffer_rect(row.rect, scale_120);
            let enabled = state.command_enabled(row.command);
            let is_selected = enabled && selected == Some(row.command);
            if is_selected {
                fill_rect(
                    canvas,
                    width,
                    height,
                    tuple(rect),
                    rgba(palette.selected_fill),
                );
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        i32::try_from(rect.x).unwrap_or(i32::MAX),
                        i32::try_from(rect.y).unwrap_or(i32::MAX),
                        3_u32.saturating_mul(scale_120).div_ceil(120).max(1),
                        rect.height,
                    ),
                    rgba(palette.selected_rail),
                );
            } else if enabled && state.hovered() == Some(row.command) {
                blend_rect(
                    canvas,
                    width,
                    height,
                    tuple(rect),
                    [
                        u8::try_from((palette.selected_fill >> 16) & 0xff).unwrap_or(0),
                        u8::try_from((palette.selected_fill >> 8) & 0xff).unwrap_or(0),
                        u8::try_from(palette.selected_fill & 0xff).unwrap_or(0),
                        64,
                    ],
                );
            }
            if enabled && pressed == Some(row.command) {
                blend_rect(
                    canvas,
                    width,
                    height,
                    tuple(rect),
                    [
                        u8::try_from((palette.selected_rail >> 16) & 0xff).unwrap_or(0),
                        u8::try_from((palette.selected_rail >> 8) & 0xff).unwrap_or(0),
                        u8::try_from(palette.selected_rail & 0xff).unwrap_or(0),
                        48,
                    ],
                );
            }
            let primary = if !enabled {
                palette.secondary
            } else if is_selected {
                palette.selected_primary
            } else {
                palette.primary
            };
            if is_selected {
                paint_text(
                    cache,
                    context,
                    canvas,
                    width,
                    height,
                    "›",
                    ChromeTextStyle::Bold,
                    scale_120,
                    renderer_generation,
                    Rect {
                        x: rect.x.saturating_add(inset / 2),
                        y: rect.y,
                        width: 20_u32.saturating_mul(scale_120).div_ceil(120),
                        height: rect.height,
                    },
                    primary,
                    false,
                )?;
            }
            let descriptor = command_descriptor(row.command);
            if row.category.width > 0 {
                paint_text(
                    cache,
                    context,
                    canvas,
                    width,
                    height,
                    descriptor.category.label(),
                    ChromeTextStyle::Regular,
                    scale_120,
                    renderer_generation,
                    buffer_rect(row.category, scale_120),
                    if is_selected {
                        palette.selected_secondary
                    } else {
                        palette.secondary
                    },
                    false,
                )?;
            }
            paint_text(
                cache,
                context,
                canvas,
                width,
                height,
                descriptor.title,
                if is_selected {
                    ChromeTextStyle::Bold
                } else {
                    ChromeTextStyle::Regular
                },
                scale_120,
                renderer_generation,
                buffer_rect(row.title, scale_120),
                primary,
                false,
            )?;
            if !layout.compact {
                paint_text(
                    cache,
                    context,
                    canvas,
                    width,
                    height,
                    descriptor.shortcut(keymap),
                    ChromeTextStyle::Regular,
                    scale_120,
                    renderer_generation,
                    buffer_rect(row.shortcut, scale_120),
                    if is_selected {
                        palette.selected_secondary
                    } else {
                        palette.secondary
                    },
                    true,
                )?;
            }
        }
    }
    let footer = buffer_rect(layout.footer, scale_120);
    paint_text(
        cache,
        context,
        canvas,
        width,
        height,
        if binding_help.is_some() {
            "↑↓/Pg navigate · Prefix+[ copy · v/y · Super+C/V · fields X/Z · Esc"
        } else {
            "↑↓ navigate   Enter run   Esc close"
        },
        ChromeTextStyle::Regular,
        scale_120,
        renderer_generation,
        Rect {
            x: footer.x.saturating_add(inset),
            width: footer.width.saturating_sub(inset.saturating_mul(2)),
            ..footer
        },
        palette.secondary,
        false,
    )?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabMenuRowLayout {
    pub(crate) action: TabMenuActionId,
    pub(crate) rect: Rect,
    pub(crate) title: Rect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabContextMenuLayout {
    pub(crate) panel: Rect,
    pub(crate) rows: Vec<TabMenuRowLayout>,
}

#[must_use]
pub(crate) fn tab_context_menu_layout(
    bounds: Rect,
    anchor: (u32, u32),
) -> Option<TabContextMenuLayout> {
    let rows_height = TAB_MENU_ROW_HEIGHT
        .saturating_mul(u32::try_from(TAB_MENU_ACTIONS.len()).unwrap_or(u32::MAX));
    if bounds.width <= TAB_MENU_SHADOW
        || bounds.height < rows_height.saturating_add(TAB_MENU_SHADOW)
    {
        return None;
    }
    let panel_width = TAB_MENU_WIDTH.min(bounds.width.saturating_sub(TAB_MENU_SHADOW));
    let available_height = bounds.height.saturating_sub(TAB_MENU_SHADOW);
    let vertical_padding = TAB_MENU_PADDING.min(available_height.saturating_sub(rows_height) / 2);
    let panel_height = rows_height.saturating_add(vertical_padding.saturating_mul(2));
    let maximum_x = bounds
        .x
        .saturating_add(bounds.width)
        .saturating_sub(panel_width)
        .saturating_sub(TAB_MENU_SHADOW);
    let x = anchor.0.min(maximum_x).max(bounds.x);
    let below = anchor.1.saturating_add(TAB_MENU_ANCHOR_GAP);
    let maximum_y = bounds
        .y
        .saturating_add(bounds.height)
        .saturating_sub(panel_height)
        .saturating_sub(TAB_MENU_SHADOW);
    let y = if below <= maximum_y {
        below.max(bounds.y)
    } else {
        anchor
            .1
            .saturating_sub(panel_height)
            .max(bounds.y)
            .min(maximum_y)
    };
    let panel = Rect {
        x,
        y,
        width: panel_width,
        height: panel_height,
    };
    let horizontal_inset = TAB_MENU_CONTENT_INSET.min(panel_width / 6);
    let rows = TAB_MENU_ACTIONS
        .iter()
        .enumerate()
        .map(|(index, descriptor)| {
            let rect = Rect {
                x,
                y: y.saturating_add(vertical_padding).saturating_add(
                    TAB_MENU_ROW_HEIGHT.saturating_mul(u32::try_from(index).unwrap_or(u32::MAX)),
                ),
                width: panel_width,
                height: TAB_MENU_ROW_HEIGHT,
            };
            TabMenuRowLayout {
                action: descriptor.id,
                rect,
                title: Rect {
                    x: rect
                        .x
                        .saturating_add(horizontal_inset)
                        .saturating_add(TAB_MENU_INDICATOR_WIDTH),
                    y: rect.y,
                    width: rect
                        .width
                        .saturating_sub(horizontal_inset.saturating_mul(2))
                        .saturating_sub(TAB_MENU_INDICATOR_WIDTH),
                    height: rect.height,
                },
            }
        })
        .collect();
    Some(TabContextMenuLayout { panel, rows })
}

#[must_use]
pub(crate) fn tab_context_menu_hit_test(
    layout: &TabContextMenuLayout,
    point: (f64, f64),
) -> Option<TabMenuActionId> {
    layout
        .rows
        .iter()
        .find(|row| contains(row.rect, point))
        .map(|row| row.action)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one specialized painter keeps tab-menu composition and selection treatment coherent"
)]
pub(crate) fn paint_tab_context_menu(
    cache: &mut CommandPaletteTextCache,
    context: &RenderContext,
    canvas: &mut [u8],
    width: u32,
    height: u32,
    scale_120: u32,
    renderer_generation: u64,
    layout: &TabContextMenuLayout,
    palette: SessionPickerPalette,
    state: &TabContextMenuUi,
    pressed: Option<TabMenuActionId>,
    keyboard_focused: bool,
) -> Result<()> {
    let panel = buffer_rect(layout.panel, scale_120);
    let shadow_offset = TAB_MENU_SHADOW.saturating_mul(scale_120).div_ceil(120);
    blend_rect(
        canvas,
        width,
        height,
        (
            i32::try_from(panel.x.saturating_add(shadow_offset)).unwrap_or(i32::MAX),
            i32::try_from(panel.y.saturating_add(shadow_offset)).unwrap_or(i32::MAX),
            panel.width,
            panel.height,
        ),
        palette.shadow,
    );
    fill_rect(canvas, width, height, tuple(panel), rgba(palette.panel));
    let border = scale_120.div_ceil(120).max(1);
    let frame = if keyboard_focused {
        palette.focused_frame
    } else {
        palette.frame
    };
    for edge in [
        (panel.x, panel.y, panel.width, border),
        (
            panel.x,
            panel.y.saturating_add(panel.height.saturating_sub(border)),
            panel.width,
            border,
        ),
        (panel.x, panel.y, border, panel.height),
        (
            panel.x.saturating_add(panel.width.saturating_sub(border)),
            panel.y,
            border,
            panel.height,
        ),
    ] {
        fill_rect(
            canvas,
            width,
            height,
            (
                i32::try_from(edge.0).unwrap_or(i32::MAX),
                i32::try_from(edge.1).unwrap_or(i32::MAX),
                edge.2,
                edge.3,
            ),
            rgba(frame),
        );
    }
    for row in &layout.rows {
        let rect = buffer_rect(row.rect, scale_120);
        let enabled = state.action_enabled(row.action);
        let selected = enabled && state.selected_action() == row.action;
        if selected {
            fill_rect(
                canvas,
                width,
                height,
                tuple(rect),
                rgba(palette.selected_fill),
            );
            fill_rect(
                canvas,
                width,
                height,
                (
                    i32::try_from(rect.x).unwrap_or(i32::MAX),
                    i32::try_from(rect.y).unwrap_or(i32::MAX),
                    3_u32.saturating_mul(scale_120).div_ceil(120).max(1),
                    rect.height,
                ),
                rgba(palette.selected_rail),
            );
        } else if enabled && state.hovered() == Some(row.action) {
            blend_rect(
                canvas,
                width,
                height,
                tuple(rect),
                [
                    u8::try_from((palette.selected_fill >> 16) & 0xff).unwrap_or(0),
                    u8::try_from((palette.selected_fill >> 8) & 0xff).unwrap_or(0),
                    u8::try_from(palette.selected_fill & 0xff).unwrap_or(0),
                    64,
                ],
            );
        }
        if enabled && pressed == Some(row.action) {
            blend_rect(
                canvas,
                width,
                height,
                tuple(rect),
                [
                    u8::try_from((palette.selected_rail >> 16) & 0xff).unwrap_or(0),
                    u8::try_from((palette.selected_rail >> 8) & 0xff).unwrap_or(0),
                    u8::try_from(palette.selected_rail & 0xff).unwrap_or(0),
                    48,
                ],
            );
        }
        let primary = if !enabled {
            palette.secondary
        } else if selected {
            palette.selected_primary
        } else {
            palette.primary
        };
        if selected {
            let indicator = Rect {
                x: rect
                    .x
                    .saturating_add(3_u32.saturating_mul(scale_120).div_ceil(120)),
                y: rect.y,
                width: TAB_MENU_INDICATOR_WIDTH
                    .saturating_mul(scale_120)
                    .div_ceil(120),
                height: rect.height,
            };
            paint_text(
                cache,
                context,
                canvas,
                width,
                height,
                "›",
                ChromeTextStyle::Bold,
                scale_120,
                renderer_generation,
                indicator,
                primary,
                false,
            )?;
        }
        paint_text(
            cache,
            context,
            canvas,
            width,
            height,
            tab_menu_descriptor(row.action).title,
            if selected {
                ChromeTextStyle::Bold
            } else {
                ChromeTextStyle::Regular
            },
            scale_120,
            renderer_generation,
            buffer_rect(row.title, scale_120),
            primary,
            false,
        )?;
    }
    fill_rect(
        canvas,
        width,
        height,
        (
            i32::try_from(panel.x.saturating_add(panel.width.saturating_sub(border)))
                .unwrap_or(i32::MAX),
            i32::try_from(panel.y).unwrap_or(i32::MAX),
            border,
            panel.height,
        ),
        rgba(frame),
    );
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DojoPromptLayout {
    pub(crate) panel: Rect,
    pub(crate) title: Rect,
    pub(crate) body: Rect,
    pub(crate) input: Option<Rect>,
    pub(crate) cancel: Option<Rect>,
    pub(crate) terminate: Option<Rect>,
}

#[must_use]
#[allow(clippy::too_many_lines, reason = "shared bounded prompt geometry")]
pub(crate) fn dojo_prompt_layout(content: Rect, state: &DojoPromptUi) -> Option<DojoPromptLayout> {
    if content.width == 0 || content.height == 0 {
        return None;
    }
    let margin = PROMPT_MARGIN.min(content.width / 4).min(content.height / 4);
    let width = PROMPT_MAX_WIDTH.min(content.width.saturating_sub(margin.saturating_mul(2)));
    let controls_height = if state.is_rename() {
        PROMPT_INPUT_HEIGHT
    } else {
        PROMPT_BUTTON_HEIGHT
    };
    let body_height = if state.is_preview() {
        PREVIEW_BODY_HEIGHT
    } else {
        PROMPT_BODY_HEIGHT
    };
    let height = PROMPT_PADDING
        .saturating_mul(2)
        .saturating_add(PROMPT_TITLE_HEIGHT)
        .saturating_add(body_height)
        .saturating_add(PROMPT_GAP.saturating_mul(2))
        .saturating_add(controls_height);
    if width < 180 || content.height < height.saturating_add(margin.saturating_mul(2)) {
        return None;
    }
    let panel = Rect {
        x: content
            .x
            .saturating_add(content.width.saturating_sub(width) / 2),
        y: content
            .y
            .saturating_add(content.height.saturating_sub(height) / 2),
        width,
        height,
    };
    let inner_width = width.saturating_sub(PROMPT_PADDING.saturating_mul(2));
    let title = Rect {
        x: panel.x.saturating_add(PROMPT_PADDING),
        y: panel.y.saturating_add(PROMPT_PADDING),
        width: inner_width,
        height: PROMPT_TITLE_HEIGHT,
    };
    let body = Rect {
        x: title.x,
        y: title
            .y
            .saturating_add(title.height)
            .saturating_add(PROMPT_GAP),
        width: inner_width,
        height: body_height,
    };
    let controls_y = body
        .y
        .saturating_add(body.height)
        .saturating_add(PROMPT_GAP);
    let (input, cancel, terminate) = if state.is_rename() {
        (
            Some(Rect {
                x: title.x,
                y: controls_y,
                width: inner_width,
                height: PROMPT_INPUT_HEIGHT,
            }),
            None,
            None,
        )
    } else if state.is_preview() {
        (
            None,
            Some(Rect {
                x: title.x,
                y: controls_y,
                width: inner_width,
                height: PROMPT_BUTTON_HEIGHT,
            }),
            None,
        )
    } else {
        let button_width = inner_width.saturating_sub(PROMPT_GAP) / 2;
        (
            None,
            Some(Rect {
                x: title.x,
                y: controls_y,
                width: button_width,
                height: PROMPT_BUTTON_HEIGHT,
            }),
            Some(Rect {
                x: title
                    .x
                    .saturating_add(button_width)
                    .saturating_add(PROMPT_GAP),
                y: controls_y,
                width: button_width,
                height: PROMPT_BUTTON_HEIGHT,
            }),
        )
    };
    Some(DojoPromptLayout {
        panel,
        title,
        body,
        input,
        cancel,
        terminate,
    })
}

#[must_use]
pub(crate) fn dojo_prompt_hit_test(
    layout: &DojoPromptLayout,
    point: (f64, f64),
) -> Option<TerminationDecision> {
    if layout.cancel.is_some_and(|rect| contains(rect, point)) {
        Some(TerminationDecision::Cancel)
    } else if layout.terminate.is_some_and(|rect| contains(rect, point)) {
        Some(TerminationDecision::Terminate)
    } else {
        None
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "trusted Dojo prompts share one bounded painter and composition transaction"
)]
pub(crate) fn paint_dojo_prompt(
    cache: &mut CommandPaletteTextCache,
    context: &RenderContext,
    canvas: &mut [u8],
    width: u32,
    height: u32,
    content_rect: Rect,
    scale_120: u32,
    renderer_generation: u64,
    layout: &DojoPromptLayout,
    palette: SessionPickerPalette,
    state: &DojoPromptUi,
    keyboard_focused: bool,
) -> Result<()> {
    blend_rect(
        canvas,
        width,
        height,
        tuple(buffer_rect(content_rect, scale_120)),
        [palette.scrim[0], palette.scrim[1], palette.scrim[2], 112],
    );
    let panel = buffer_rect(layout.panel, scale_120);
    fill_rect(canvas, width, height, tuple(panel), rgba(palette.panel));
    let border = scale_120.div_ceil(120).max(1);
    let frame = if keyboard_focused {
        palette.focused_frame
    } else {
        palette.frame
    };
    for edge in [
        (panel.x, panel.y, panel.width, border),
        (
            panel.x,
            panel.y.saturating_add(panel.height.saturating_sub(border)),
            panel.width,
            border,
        ),
        (panel.x, panel.y, border, panel.height),
        (
            panel.x.saturating_add(panel.width.saturating_sub(border)),
            panel.y,
            border,
            panel.height,
        ),
    ] {
        fill_rect(
            canvas,
            width,
            height,
            (
                i32::try_from(edge.0).unwrap_or(i32::MAX),
                i32::try_from(edge.1).unwrap_or(i32::MAX),
                edge.2,
                edge.3,
            ),
            rgba(frame),
        );
    }
    let (title, body) = state.title_and_body();
    paint_text(
        cache,
        context,
        canvas,
        width,
        height,
        title,
        ChromeTextStyle::Bold,
        scale_120,
        renderer_generation,
        buffer_rect(layout.title, scale_120),
        palette.primary,
        false,
    )?;
    if state.is_preview() {
        let body_rect = buffer_rect(layout.body, scale_120);
        let line_height = PREVIEW_LINE_HEIGHT.saturating_mul(scale_120).div_ceil(120);
        for (index, source) in body.lines().take(MAX_PREVIEW_LINES).enumerate() {
            let mut line = source
                .chars()
                .take(MAX_PREVIEW_LINE_SCALARS)
                .collect::<String>();
            if source.chars().count() > MAX_PREVIEW_LINE_SCALARS {
                line.push('…');
            }
            paint_text(
                cache,
                context,
                canvas,
                width,
                height,
                &line,
                ChromeTextStyle::Regular,
                scale_120,
                renderer_generation,
                Rect {
                    y: body_rect.y.saturating_add(
                        u32::try_from(index)
                            .unwrap_or(u32::MAX)
                            .saturating_mul(line_height),
                    ),
                    height: line_height,
                    ..body_rect
                },
                palette.secondary,
                false,
            )?;
        }
    } else {
        paint_text(
            cache,
            context,
            canvas,
            width,
            height,
            &body,
            ChromeTextStyle::Regular,
            scale_120,
            renderer_generation,
            buffer_rect(layout.body, scale_120),
            palette.secondary,
            false,
        )?;
    }
    if let Some(prompt_input) = state.input() {
        let input = buffer_rect(layout.input.expect("rename input exists"), scale_120);
        fill_rect(
            canvas,
            width,
            height,
            tuple(input),
            rgba(palette.selected_fill),
        );
        let inset = 12_u32.saturating_mul(scale_120).div_ceil(120);
        paint_text(
            cache,
            context,
            canvas,
            width,
            height,
            &format!("> {prompt_input}_"),
            ChromeTextStyle::Regular,
            scale_120,
            renderer_generation,
            Rect {
                x: input.x.saturating_add(inset),
                width: input.width.saturating_sub(inset.saturating_mul(2)),
                ..input
            },
            palette.selected_primary,
            false,
        )?;
    } else if state.is_preview() {
        let rect = buffer_rect(layout.cancel.expect("preview close exists"), scale_120);
        fill_rect(
            canvas,
            width,
            height,
            tuple(rect),
            rgba(palette.selected_fill),
        );
        paint_text(
            cache,
            context,
            canvas,
            width,
            height,
            "Close",
            ChromeTextStyle::Bold,
            scale_120,
            renderer_generation,
            rect,
            palette.selected_primary,
            false,
        )?;
    } else {
        let selected_decision = state.decision().expect("confirmation decision exists");
        for (decision, rect, label) in [
            (
                TerminationDecision::Cancel,
                layout.cancel.expect("cancel exists"),
                "Cancel",
            ),
            (
                TerminationDecision::Terminate,
                layout.terminate.expect("terminate exists"),
                "Terminate",
            ),
        ] {
            let rect = buffer_rect(rect, scale_120);
            let selected = selected_decision == decision;
            fill_rect(
                canvas,
                width,
                height,
                tuple(rect),
                rgba(if selected {
                    palette.selected_fill
                } else {
                    palette.panel
                }),
            );
            paint_text(
                cache,
                context,
                canvas,
                width,
                height,
                label,
                if selected {
                    ChromeTextStyle::Bold
                } else {
                    ChromeTextStyle::Regular
                },
                scale_120,
                renderer_generation,
                rect,
                if selected {
                    palette.selected_primary
                } else if decision == TerminationDecision::Terminate {
                    palette.selected_rail
                } else {
                    palette.primary
                },
                false,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::ResolvedTheme,
        frontend::{CommandPaletteContext, CommandTabMoveAvailability},
        renderer::session_picker_palette,
    };
    use splinterm_core::{DojoId, LairId, SplintId};

    #[test]
    fn layout_is_bounded_selected_visible_and_hit_targets_are_half_open() {
        let commands = vec![
            BuiltInCommandId::NewDojo,
            BuiltInCommandId::SplitHorizontal,
            BuiltInCommandId::SplitVertical,
        ];
        for content in [
            Rect {
                x: 0,
                y: 34,
                width: 960,
                height: 566,
            },
            Rect {
                x: 0,
                y: 34,
                width: 360,
                height: 240,
            },
        ] {
            let layout = command_palette_layout(content, &commands, 2, 0).unwrap();
            assert!(layout.panel.x >= content.x && layout.panel.y >= content.y);
            assert!(layout.panel.x + layout.panel.width <= content.x + content.width);
            assert!(layout.panel.y + layout.panel.height <= content.y + content.height);
            assert!(layout.rows.iter().any(|row| row.command == commands[2]));
            for row in &layout.rows {
                assert_eq!(row.rect.height, ROW_HEIGHT - ROW_GUTTER_Y * 2);
                assert!(row.rect.x > layout.panel.x);
                assert!(row.rect.x + row.rect.width < layout.panel.x + layout.panel.width);
                assert!(row.title.x >= row.rect.x + CONTENT_INSET);
                assert_eq!(row.category.width == 0, layout.compact);
                if !layout.compact {
                    assert!(row.title.x > row.category.x + row.category.width);
                    assert_eq!(row.shortcut.width, (row.rect.width / 3).min(180));
                    assert!(row.shortcut.width > 60);
                }
                assert_eq!(
                    command_palette_hit_test(
                        &layout,
                        (f64::from(row.rect.x), f64::from(row.rect.y))
                    ),
                    Some(row.command)
                );
                assert_ne!(
                    command_palette_hit_test(
                        &layout,
                        (
                            f64::from(row.rect.x + row.rect.width),
                            f64::from(row.rect.y)
                        )
                    ),
                    Some(row.command)
                );
            }
        }
    }

    #[test]
    fn ultra_short_layout_is_rowless_and_keeps_children_inside_panel() {
        let content = Rect {
            x: 0,
            y: 34,
            width: 640,
            height: 120,
        };
        let commands = [BuiltInCommandId::NewDojo];
        let layout = command_palette_layout(content, &commands, 0, 0).unwrap();
        assert!(layout.rows.is_empty());
        assert_eq!(layout.list.height, 0);
        assert_eq!(
            layout.footer.y + layout.footer.height,
            layout.panel.y + layout.panel.height
        );
        assert!(layout.header.y + layout.header.height <= layout.panel.y + layout.panel.height);
        assert!(layout.list.y + layout.list.height <= layout.panel.y + layout.panel.height);
    }

    #[test]
    fn empty_results_keep_a_visible_inert_list_row() {
        let content = Rect {
            x: 0,
            y: 34,
            width: 640,
            height: 400,
        };
        let layout = command_palette_layout(content, &[], 0, 0).unwrap();
        assert!(layout.rows.is_empty());
        assert_eq!(layout.list.height, ROW_HEIGHT);
        assert_eq!(command_palette_hit_test(&layout, (10.0, 100.0)), None);
    }

    #[test]
    fn tab_context_menu_clamps_all_edges_and_keeps_half_open_rows() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 400,
        };
        for anchor in [(0, 0), (639, 0), (0, 399), (639, 399)] {
            let layout = tab_context_menu_layout(bounds, anchor).unwrap();
            assert_eq!(layout.panel.width, TAB_MENU_WIDTH);
            assert_eq!(
                layout.panel.height,
                TAB_MENU_ROW_HEIGHT * u32::try_from(TAB_MENU_ACTIONS.len()).unwrap()
                    + TAB_MENU_PADDING * 2
            );
            assert!(layout.panel.x >= bounds.x && layout.panel.y >= bounds.y);
            assert!(
                layout.panel.x + layout.panel.width + TAB_MENU_SHADOW <= bounds.x + bounds.width
            );
            assert!(
                layout.panel.y + layout.panel.height + TAB_MENU_SHADOW <= bounds.y + bounds.height
            );
            assert_eq!(layout.rows.len(), TAB_MENU_ACTIONS.len());
            for row in &layout.rows {
                assert_eq!(row.rect.height, TAB_MENU_ROW_HEIGHT);
                assert_eq!(
                    row.title.x - row.rect.x,
                    TAB_MENU_CONTENT_INSET + TAB_MENU_INDICATOR_WIDTH
                );
                assert_eq!(
                    tab_context_menu_hit_test(
                        &layout,
                        (f64::from(row.rect.x), f64::from(row.rect.y))
                    ),
                    Some(row.action)
                );
                assert_ne!(
                    tab_context_menu_hit_test(
                        &layout,
                        (
                            f64::from(row.rect.x + row.rect.width),
                            f64::from(row.rect.y)
                        )
                    ),
                    Some(row.action)
                );
            }
            assert!(layout.rows[0].rect.y + layout.rows[0].rect.height <= layout.rows[1].rect.y);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "one shared prompt paint scenario")]
    fn dojo_prompts_are_bounded_default_cancel_and_half_open() {
        let content = Rect {
            x: 0,
            y: 34,
            width: 640,
            height: 366,
        };
        let rename = DojoPromptUi::rename(
            DojoId::new(),
            "captured".to_owned(),
            2,
            vec![(SplintId::new(), 1), (SplintId::new(), 2)],
        );
        let rename_layout = dojo_prompt_layout(content, &rename).unwrap();
        assert!(rename_layout.input.is_some());
        assert!(rename_layout.cancel.is_none());
        assert!(contains(
            content,
            (
                f64::from(rename_layout.panel.x),
                f64::from(rename_layout.panel.y)
            )
        ));

        let terminate = DojoPromptUi::terminate(
            DojoId::new(),
            "captured".to_owned(),
            3,
            vec![
                (SplintId::new(), 1),
                (SplintId::new(), 2),
                (SplintId::new(), 3),
            ],
        );
        let layout = dojo_prompt_layout(content, &terminate).unwrap();
        let cancel = layout.cancel.unwrap();
        let destructive = layout.terminate.unwrap();
        assert_eq!(
            dojo_prompt_hit_test(&layout, (f64::from(cancel.x), f64::from(cancel.y))),
            Some(TerminationDecision::Cancel)
        );
        assert_eq!(
            dojo_prompt_hit_test(
                &layout,
                (
                    f64::from(destructive.x + destructive.width),
                    f64::from(destructive.y)
                )
            ),
            None
        );
        let mut canvas = vec![0_u8; 640 * 400 * 4];
        let mut cache = CommandPaletteTextCache::default();
        paint_dojo_prompt(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            640,
            400,
            content,
            120,
            1,
            &layout,
            session_picker_palette(ResolvedTheme::default()),
            &terminate,
            true,
        )
        .unwrap();
        assert!(canvas.iter().any(|byte| *byte != 0));
        assert!(cache.len() <= 4);
        let DojoPromptUi::Terminate(confirmation) = terminate else {
            unreachable!();
        };
        assert_eq!(confirmation.decision(), TerminationDecision::Cancel);

        let preview = DojoPromptUi::preview_lair(crate::frontend::LairPromptTarget {
            topology_revision: splinterm_core::TopologyRevision::new(9),
            lair_id: LairId::new(),
            dojo_id: None,
            name: "saved".to_owned(),
            retention: splinterm_core::LairRetention::Saved,
            preview: format!(
                "Lair: saved\nDojo: shell\n  Horizontal split 758/1000\n    shell — Application: /usr/bin/bash — /home/oldjobobo/Projects/splinterm\n    shell — Shell — /home/oldjobobo/Projects/splinterm\n  shell — Shell — /home/oldjobobo/Projects/splinterm\nNot restored: {}",
                "terminal/scrollback bodies, process memory, shell state, environment, clipboard, images".repeat(4)
            ),
            targets: Vec::new(),
        });
        let preview_layout = dojo_prompt_layout(content, &preview).unwrap();
        assert!(preview_layout.cancel.is_some());
        assert!(preview_layout.terminate.is_none());
        assert_eq!(preview_layout.body.height, PREVIEW_BODY_HEIGHT);
        canvas.fill(0);
        paint_dojo_prompt(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            640,
            400,
            content,
            120,
            1,
            &preview_layout,
            session_picker_palette(ResolvedTheme::default()),
            &preview,
            true,
        )
        .unwrap();
        assert!(canvas.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn tab_context_menu_paints_without_a_window_scrim() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 400,
        };
        let layout = tab_context_menu_layout(bounds, (240, 40)).unwrap();
        let mut state = TabContextMenuUi::new(crate::frontend::TabMenuContext {
            lair_id: LairId::new(),
            focused_cwd: "/tmp".into(),
            dojo_id: DojoId::new(),
            dojo_name: "test".to_owned(),
            pane_count: 2,
            splints: vec![(SplintId::new(), 1), (SplintId::new(), 2)],
            active: false,
            other_dojo_ids: vec![DojoId::new()],
        });
        let mut canvas = vec![0_u8; 640 * 400 * 4];
        let mut cache = CommandPaletteTextCache::default();
        let right_x = layout.panel.x + layout.panel.width - 1;
        let top_edge = usize::try_from((layout.panel.y * 640 + right_x) * 4).unwrap();
        let close_row = layout
            .rows
            .iter()
            .position(|row| row.action == TabMenuActionId::CloseTab)
            .unwrap();
        for (hovered, pressed, row) in [
            (None, None, 0),
            (Some(TabMenuActionId::CloseTab), None, close_row),
            (
                Some(TabMenuActionId::CloseTab),
                Some(TabMenuActionId::CloseTab),
                close_row,
            ),
        ] {
            state.update_hovered(hovered);
            canvas.fill(0);
            paint_tab_context_menu(
                &mut cache,
                &RenderContext::new(u16::MAX),
                &mut canvas,
                640,
                400,
                120,
                1,
                &layout,
                session_picker_palette(ResolvedTheme::default()),
                &state,
                pressed,
                true,
            )
            .unwrap();
            assert_eq!(&canvas[..4], &[0, 0, 0, 0]);
            let panel_offset =
                usize::try_from((layout.panel.y * 640 + layout.panel.x) * 4).unwrap();
            assert_ne!(&canvas[panel_offset..panel_offset + 4], &[0, 0, 0, 0]);
            let row_y = layout.rows[row].rect.y + layout.rows[row].rect.height / 2;
            let row_edge = usize::try_from((row_y * 640 + right_x) * 4).unwrap();
            assert_eq!(
                &canvas[row_edge..row_edge + 4],
                &canvas[top_edge..top_edge + 4]
            );
        }
        assert!(cache.len() <= TAB_MENU_ACTIONS.len() + 1);
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "one bounded paint/cache scenario")]
    fn painter_marks_transient_panel_and_reuses_bounded_text() {
        let content = Rect {
            x: 0,
            y: 34,
            width: 640,
            height: 366,
        };
        let state = CommandPaletteUi::new(CommandPaletteContext {
            lair_id: LairId::new(),
            lair_retention: splinterm_core::LairRetention::Disposable,
            focused_cwd: "/tmp".into(),
            dojo_id: DojoId::new(),
            dojo_name: "test".to_owned(),
            pane_count: 1,
            splint_id: SplintId::new(),
            dojo_splints: vec![(SplintId::new(), 1)],
            other_dojo_ids: Vec::new(),
            previous_dojo_id: None,
            next_dojo_id: None,
            tab_move: CommandTabMoveAvailability::Neither,
            focus_left: None,
            focus_right: None,
            focus_up: None,
            focus_down: None,
            viewport_detached: false,
            controller_active: false,
            forced_control_transfer: true,
            grant_ids: Vec::new(),
            pending_transfer_id: None,
        });
        let layout = command_palette_layout(
            content,
            state.filtered(),
            state.selected_index(),
            state.visible_start(),
        )
        .unwrap();
        let theme = ResolvedTheme::default();
        let mut canvas = vec![0_u8; 640 * 400 * 4];
        let before = canvas.clone();
        let mut cache = CommandPaletteTextCache::default();
        let keymap = ResolvedKeymap::default();
        paint_command_palette(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            640,
            400,
            content,
            120,
            1,
            &layout,
            session_picker_palette(theme),
            &state,
            &keymap,
            None,
            true,
            None,
        )
        .unwrap();
        let shaped = cache.len();
        assert!(shaped > 0);
        assert_ne!(canvas, before);
        paint_command_palette(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            640,
            400,
            content,
            120,
            1,
            &layout,
            session_picker_palette(theme),
            &state,
            &keymap,
            None,
            true,
            None,
        )
        .unwrap();
        assert_eq!(cache.len(), shaped);
        let help = BindingHelpUi::new(&keymap);
        paint_command_palette(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            640,
            400,
            content,
            120,
            1,
            &layout,
            session_picker_palette(theme),
            &state,
            &keymap,
            None,
            true,
            Some(&help),
        )
        .unwrap();
        assert!(cache.len() > shaped);
    }
}
