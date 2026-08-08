//! Trusted inline session-picker layout and CPU composition.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
};

use anyhow::Result;
use unicode_width::UnicodeWidthChar;

use crate::{config::ResolvedTheme, frontend::PickerHitTarget, geometry::Rect};

use super::super::{ChromeText, ChromeTextStyle, RenderContext, blend_rect, fill_rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionPickerPresentationMode {
    Normal,
    Compact,
    Minimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionPickerRowLayout {
    pub(crate) target: PickerHitTarget,
    pub(crate) rect: Rect,
    pub(crate) surface: Rect,
    pub(crate) title_clip: Rect,
    pub(crate) metadata_clip: Rect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionPickerOverlayLayout {
    pub(crate) panel: Rect,
    pub(crate) header: Rect,
    pub(crate) action: Rect,
    pub(crate) list: Rect,
    pub(crate) footer: Rect,
    pub(crate) rows: Vec<SessionPickerRowLayout>,
    pub(crate) visible_range: Range<usize>,
    pub(crate) visible_capacity: usize,
    pub(crate) mode: SessionPickerPresentationMode,
    pub(crate) cache_key: (u32, u32, u32),
}

fn picker_item_rect(slot: Rect, mode: SessionPickerPresentationMode) -> Rect {
    let (horizontal_margin, vertical_margin) = match mode {
        SessionPickerPresentationMode::Normal => (8_u32, 3_u32),
        SessionPickerPresentationMode::Compact => (6_u32, 3_u32),
        SessionPickerPresentationMode::Minimal => (0_u32, 0_u32),
    };
    Rect {
        x: slot.x.saturating_add(horizontal_margin.min(slot.width / 2)),
        y: slot.y.saturating_add(vertical_margin.min(slot.height / 2)),
        width: slot
            .width
            .saturating_sub(horizontal_margin.saturating_mul(2)),
        height: slot
            .height
            .saturating_sub(vertical_margin.saturating_mul(2)),
    }
}

fn picker_row_layout(
    target: PickerHitTarget,
    slot: Rect,
    two_lines: bool,
    mode: SessionPickerPresentationMode,
) -> SessionPickerRowLayout {
    let surface = picker_item_rect(slot, mode);
    let horizontal_inset = 12_u32.min(surface.width / 4);
    let content_x = surface.x.saturating_add(horizontal_inset);
    let content_width = surface
        .width
        .saturating_sub(horizontal_inset.saturating_mul(2));
    let title_height = if two_lines {
        surface.height / 2
    } else {
        surface.height
    };
    SessionPickerRowLayout {
        target,
        rect: slot,
        surface,
        title_clip: Rect {
            x: content_x,
            y: surface.y,
            width: content_width,
            height: title_height,
        },
        metadata_clip: Rect {
            x: content_x,
            y: surface.y.saturating_add(title_height),
            width: content_width,
            height: surface.height.saturating_sub(title_height),
        },
    }
}

fn picker_visible_start(
    item_count: usize,
    selected_action: usize,
    requested_start: usize,
    capacity: usize,
) -> usize {
    if item_count == 0 || capacity == 0 {
        return 0;
    }
    let mut start = requested_start.min(item_count.saturating_sub(capacity));
    if selected_action > 0 {
        let selected_item = selected_action - 1;
        if selected_item < start {
            start = selected_item;
        } else if selected_item >= start.saturating_add(capacity) {
            start = selected_item.saturating_add(1).saturating_sub(capacity);
        }
    }
    start.min(item_count.saturating_sub(capacity))
}

#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "the specialized responsive layout keeps all mode invariants in one pure seam"
)]
pub(crate) fn session_picker_overlay_layout(
    logical_width: u32,
    logical_height: u32,
    scale_120: u32,
    item_count: usize,
    selected_action: usize,
    requested_start: usize,
) -> Option<SessionPickerOverlayLayout> {
    if logical_width == 0 || logical_height == 0 || scale_120 == 0 {
        return None;
    }
    let preferred_mode = if logical_width >= 480 && logical_height >= 320 {
        SessionPickerPresentationMode::Normal
    } else if logical_width >= 280 && logical_height >= 180 {
        SessionPickerPresentationMode::Compact
    } else {
        SessionPickerPresentationMode::Minimal
    };
    let (margin, header_height, row_height, footer_height): (u32, u32, u32, u32) =
        match preferred_mode {
            SessionPickerPresentationMode::Normal => (16, 64, 56, 40),
            SessionPickerPresentationMode::Compact => (8, 44, 44, 36),
            SessionPickerPresentationMode::Minimal => (0, 32, 44, 28),
        };
    let available_width = logical_width.saturating_sub(margin.saturating_mul(2));
    let available_height = logical_height.saturating_sub(margin.saturating_mul(2));
    if available_width == 0 || available_height == 0 {
        return None;
    }
    let panel_width = available_width.min(680);
    let normal_fixed = header_height
        .saturating_add(row_height)
        .saturating_add(footer_height);
    let computed_capacity =
        usize::try_from(available_height.saturating_sub(normal_fixed) / row_height.max(1))
            .unwrap_or(0);
    let mode = if preferred_mode != SessionPickerPresentationMode::Minimal
        && item_count > 0
        && computed_capacity == 0
    {
        SessionPickerPresentationMode::Minimal
    } else {
        preferred_mode
    };
    if mode == SessionPickerPresentationMode::Minimal {
        let action_height = if available_height >= 104 {
            available_height.saturating_sub(60)
        } else {
            available_height.min(44)
        }
        .max(1);
        let chrome_height = available_height.saturating_sub(action_height);
        let header_height = 32.min(chrome_height.div_ceil(2));
        let footer_height = chrome_height.saturating_sub(header_height);
        let panel_height = header_height
            .saturating_add(action_height)
            .saturating_add(footer_height)
            .min(available_height);
        let panel = Rect {
            x: (logical_width.saturating_sub(panel_width)) / 2,
            y: (logical_height.saturating_sub(panel_height)) / 2,
            width: panel_width,
            height: panel_height,
        };
        let header = Rect {
            x: panel.x,
            y: panel.y,
            width: panel.width,
            height: header_height,
        };
        let action = Rect {
            x: panel.x,
            y: header.y.saturating_add(header.height),
            width: panel.width,
            height: action_height,
        };
        let footer = Rect {
            x: panel.x,
            y: action.y.saturating_add(action.height),
            width: panel.width,
            height: footer_height,
        };
        let target = if selected_action == 0 || item_count == 0 {
            PickerHitTarget::New
        } else {
            PickerHitTarget::Open((selected_action - 1).min(item_count - 1))
        };
        let visible_range = match target {
            PickerHitTarget::New => 0..0,
            PickerHitTarget::Open(index) => index..index.saturating_add(1),
        };
        return Some(SessionPickerOverlayLayout {
            panel,
            header,
            action,
            list: action,
            footer,
            rows: vec![picker_row_layout(target, action, false, mode)],
            visible_range,
            visible_capacity: usize::from(item_count > 0),
            mode,
            cache_key: (logical_width, logical_height, scale_120),
        });
    }

    let capacity = computed_capacity.max(usize::from(item_count > 0));
    let visible_count = item_count.min(capacity);
    let panel_height = normal_fixed
        .saturating_add(row_height.saturating_mul(u32::try_from(visible_count).unwrap_or(u32::MAX)))
        .min(available_height);
    let panel = Rect {
        x: (logical_width.saturating_sub(panel_width)) / 2,
        y: (logical_height.saturating_sub(panel_height)) / 2,
        width: panel_width,
        height: panel_height,
    };
    let header = Rect {
        x: panel.x,
        y: panel.y,
        width: panel.width,
        height: header_height,
    };
    let action = Rect {
        x: panel.x,
        y: header.y.saturating_add(header.height),
        width: panel.width,
        height: row_height,
    };
    let list = Rect {
        x: panel.x,
        y: action.y.saturating_add(action.height),
        width: panel.width,
        height: row_height.saturating_mul(u32::try_from(visible_count).unwrap_or(u32::MAX)),
    };
    let footer = Rect {
        x: panel.x,
        y: list.y.saturating_add(list.height),
        width: panel.width,
        height: footer_height,
    };
    let start = picker_visible_start(item_count, selected_action, requested_start, visible_count);
    let visible_range = start..start.saturating_add(visible_count);
    let mut rows = Vec::with_capacity(visible_count.saturating_add(1));
    rows.push(picker_row_layout(PickerHitTarget::New, action, false, mode));
    for (slot, index) in visible_range.clone().enumerate() {
        rows.push(picker_row_layout(
            PickerHitTarget::Open(index),
            Rect {
                x: list.x,
                y: list.y.saturating_add(
                    row_height.saturating_mul(u32::try_from(slot).unwrap_or(u32::MAX)),
                ),
                width: list.width,
                height: row_height,
            },
            mode == SessionPickerPresentationMode::Normal,
            mode,
        ));
    }
    Some(SessionPickerOverlayLayout {
        panel,
        header,
        action,
        list,
        footer,
        rows,
        visible_range,
        visible_capacity: visible_count,
        mode,
        cache_key: (logical_width, logical_height, scale_120),
    })
}

#[must_use]
pub(crate) fn session_picker_hit_test(
    layout: &SessionPickerOverlayLayout,
    position: (f64, f64),
) -> Option<PickerHitTarget> {
    layout.rows.iter().find_map(|row| {
        let right = f64::from(row.rect.x.saturating_add(row.rect.width));
        let bottom = f64::from(row.rect.y.saturating_add(row.rect.height));
        (position.0 >= f64::from(row.rect.x)
            && position.0 < right
            && position.1 >= f64::from(row.rect.y)
            && position.1 < bottom)
            .then_some(row.target)
    })
}

fn linear_channel(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_channel(channel: f64) -> u8 {
    let value = if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let encoded = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    encoded
}

fn rgb_channels(color: u32) -> [u8; 3] {
    let [_, red, green, blue] = color.to_be_bytes();
    [red, green, blue]
}

fn packed_channels([red, green, blue]: [u8; 3]) -> u32 {
    u32::from_be_bytes([0, red, green, blue])
}

fn mix_linear(left: u32, right: u32, right_weight: f64) -> u32 {
    let left = rgb_channels(left);
    let right = rgb_channels(right);
    let weight = right_weight.clamp(0.0, 1.0);
    packed_channels(std::array::from_fn(|index| {
        srgb_channel(
            linear_channel(left[index]) * (1.0 - weight) + linear_channel(right[index]) * weight,
        )
    }))
}

fn relative_luminance(color: u32) -> f64 {
    let [red, green, blue] = rgb_channels(color).map(linear_channel);
    red * 0.2126 + green * 0.7152 + blue * 0.0722
}

fn contrast_ratio(left: u32, right: u32) -> f64 {
    let left = relative_luminance(left);
    let right = relative_luminance(right);
    (left.max(right) + 0.05) / (left.min(right) + 0.05)
}

fn strongest_contrast_endpoint(background: u32) -> u32 {
    if contrast_ratio(0x00ff_ffff, background) >= contrast_ratio(0, background) {
        0x00ff_ffff
    } else {
        0
    }
}

fn contrast_corrected_toward(foreground: u32, target: u32, background: u32) -> u32 {
    if contrast_ratio(foreground, background) >= 4.5 {
        return foreground;
    }
    if contrast_ratio(target, background) < 4.5 {
        return strongest_contrast_endpoint(background);
    }
    let mut failing = 0.0_f64;
    let mut passing = 1.0_f64;
    for _ in 0..24 {
        let weight = f64::midpoint(failing, passing);
        if contrast_ratio(mix_linear(foreground, target, weight), background) >= 4.5 {
            passing = weight;
        } else {
            failing = weight;
        }
    }
    mix_linear(foreground, target, passing)
}

fn contrast_corrected(foreground: u32, background: u32) -> u32 {
    contrast_corrected_toward(
        foreground,
        strongest_contrast_endpoint(background),
        background,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionPickerPalette {
    pub(crate) scrim: [u8; 4],
    pub(crate) panel: u32,
    pub(crate) primary: u32,
    pub(crate) secondary: u32,
    pub(crate) frame: u32,
    pub(crate) focused_frame: u32,
    pub(crate) selected_rail: u32,
    pub(crate) selected_fill: u32,
    pub(crate) selected_primary: u32,
    pub(crate) selected_secondary: u32,
    pub(crate) shadow: [u8; 4],
}

#[must_use]
pub(crate) fn session_picker_palette(theme: ResolvedTheme) -> SessionPickerPalette {
    let primary = contrast_corrected(theme.foreground, theme.background);
    let secondary = contrast_corrected_toward(
        mix_linear(primary, theme.background, 0.30),
        primary,
        theme.background,
    );
    let selected_fill = mix_linear(theme.background, theme.selection, 0.24);
    let selected_primary = contrast_corrected(primary, selected_fill);
    let selected_secondary = contrast_corrected_toward(
        mix_linear(selected_primary, selected_fill, 0.30),
        selected_primary,
        selected_fill,
    );
    let focused_frame = if contrast_ratio(theme.pane_border_active, theme.pane_border) >= 1.2 {
        theme.pane_border_active
    } else {
        theme.ui_accent
    };
    let [red, green, blue] = rgb_channels(mix_linear(theme.background, 0, 0.80));
    SessionPickerPalette {
        scrim: [red, green, blue, 140],
        panel: theme.background,
        primary,
        secondary,
        frame: theme.pane_border,
        focused_frame,
        selected_rail: theme.ui_accent,
        selected_fill,
        selected_primary,
        selected_secondary,
        shadow: [0, 0, 0, 89],
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SessionPickerTextItem<'a> {
    pub(crate) display_title: &'a str,
    pub(crate) working_directory: &'a str,
    pub(crate) pane_count: usize,
    pub(crate) running_pane_count: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SessionPickerTextKey {
    source: String,
    constrained_width: u32,
    style: ChromeTextStyle,
    scale_120: u32,
    renderer_generation: u64,
}

#[derive(Default)]
pub(crate) struct SessionPickerTextCache {
    entries: HashMap<SessionPickerTextKey, ChromeText>,
    recent_frames: VecDeque<HashSet<SessionPickerTextKey>>,
}

impl SessionPickerTextCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.recent_frames.clear();
    }

    fn finish_frame(&mut self, used: HashSet<SessionPickerTextKey>) {
        self.recent_frames.push_back(used);
        while self.recent_frames.len() > 3 {
            self.recent_frames.pop_front();
        }
        let retained = self
            .recent_frames
            .iter()
            .flat_map(HashSet::iter)
            .collect::<HashSet<_>>();
        self.entries.retain(|key, _| retained.contains(key));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

fn logical_coordinate_to_buffer(value: u32, scale_120: u32) -> u32 {
    value.saturating_mul(scale_120).div_ceil(120)
}

fn picker_buffer_rect(rect: Rect, scale_120: u32) -> Rect {
    let left = logical_coordinate_to_buffer(rect.x, scale_120);
    let top = logical_coordinate_to_buffer(rect.y, scale_120);
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

fn rect_tuple(rect: Rect) -> (i32, i32, u32, u32) {
    (
        i32::try_from(rect.x).unwrap_or(i32::MAX),
        i32::try_from(rect.y).unwrap_or(i32::MAX),
        rect.width,
        rect.height,
    )
}

fn opaque_rgba(color: u32) -> [u8; 4] {
    let [red, green, blue] = rgb_channels(color);
    [red, green, blue, 0xff]
}

#[derive(Clone, Copy)]
enum PickerTextAlignment {
    Left,
    Center,
    Right,
}

fn truncate_picker_text(source: &str, maximum_cells: usize) -> String {
    let total_cells = source
        .chars()
        .map(|character| character.width().unwrap_or(0).min(2))
        .sum::<usize>();
    if total_cells <= maximum_cells {
        return source.to_owned();
    }
    if maximum_cells == 0 {
        return String::new();
    }
    let content_cells = maximum_cells - 1;
    let mut cells = 0_usize;
    let mut truncated = String::new();
    for character in source.chars() {
        let width = character.width().unwrap_or(0).min(2);
        if cells.saturating_add(width) > content_cells {
            break;
        }
        truncated.push(character);
        cells = cells.saturating_add(width);
    }
    truncated.push('…');
    truncated
}

#[allow(clippy::too_many_arguments)]
fn paint_picker_text(
    cache: &mut SessionPickerTextCache,
    context: &RenderContext,
    used: &mut HashSet<SessionPickerTextKey>,
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    source: &str,
    style: ChromeTextStyle,
    scale_120: u32,
    renderer_generation: u64,
    clip: Rect,
    alignment: PickerTextAlignment,
    color: u32,
) -> Result<()> {
    if source.is_empty() || clip.width == 0 || clip.height == 0 {
        return Ok(());
    }
    let key = SessionPickerTextKey {
        source: source.to_owned(),
        constrained_width: clip.width,
        style,
        scale_120,
        renderer_generation,
    };
    if !cache.entries.contains_key(&key) {
        let mut text = ChromeText::load_styled_with_context(source, scale_120, style, context)?;
        if text.pixel_width() > clip.width {
            let maximum_cells =
                usize::try_from(clip.width / text.frame.cell_width.max(1)).unwrap_or(usize::MAX);
            let truncated = truncate_picker_text(source, maximum_cells);
            text = ChromeText::load_styled_with_context(&truncated, scale_120, style, context)?;
        }
        cache.entries.insert(key.clone(), text);
    }
    let text = &cache.entries[&key];
    let x = match alignment {
        PickerTextAlignment::Left => clip.x,
        PickerTextAlignment::Center => clip
            .x
            .saturating_add(clip.width.saturating_sub(text.pixel_width()) / 2),
        PickerTextAlignment::Right => clip
            .x
            .saturating_add(clip.width.saturating_sub(text.pixel_width())),
    };
    let y = clip
        .y
        .saturating_add(clip.height.saturating_sub(text.pixel_height()) / 2);
    text.paint(canvas, canvas_width, canvas_height, (x, y), clip, color);
    used.insert(key);
    Ok(())
}

#[allow(
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the specialized picker painter keeps ordered chrome composition in one bounded seam"
)]
pub(crate) fn paint_session_picker_overlay(
    cache: &mut SessionPickerTextCache,
    context: &RenderContext,
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    scrim_rect: Rect,
    scale_120: u32,
    renderer_generation: u64,
    layout: &SessionPickerOverlayLayout,
    palette: SessionPickerPalette,
    items: &[SessionPickerTextItem<'_>],
    selected: PickerHitTarget,
    hovered: Option<PickerHitTarget>,
    pressed: Option<PickerHitTarget>,
    new_enabled: bool,
    keyboard_focused: bool,
) -> Result<()> {
    blend_rect(
        canvas,
        canvas_width,
        canvas_height,
        rect_tuple(scrim_rect),
        palette.scrim,
    );
    let panel = picker_buffer_rect(layout.panel, scale_120);
    let shadow_offset = 6_u32.saturating_mul(scale_120).div_ceil(120);
    blend_rect(
        canvas,
        canvas_width,
        canvas_height,
        (
            i32::try_from(panel.x.saturating_add(shadow_offset)).unwrap_or(i32::MAX),
            i32::try_from(panel.y.saturating_add(shadow_offset)).unwrap_or(i32::MAX),
            panel.width,
            panel.height,
        ),
        palette.shadow,
    );
    fill_rect(
        canvas,
        canvas_width,
        canvas_height,
        rect_tuple(panel),
        opaque_rgba(palette.panel),
    );
    let frame_color = if keyboard_focused {
        palette.focused_frame
    } else {
        palette.frame
    };
    let border = scale_120.div_ceil(120).max(1);
    for rect in [
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
            canvas_width,
            canvas_height,
            (
                i32::try_from(rect.0).unwrap_or(i32::MAX),
                i32::try_from(rect.1).unwrap_or(i32::MAX),
                rect.2,
                rect.3,
            ),
            opaque_rgba(frame_color),
        );
    }
    for separator_y in [
        layout.header.y.saturating_add(layout.header.height),
        layout.footer.y,
    ] {
        let separator = picker_buffer_rect(
            Rect {
                x: layout.panel.x,
                y: separator_y,
                width: layout.panel.width,
                height: 1,
            },
            scale_120,
        );
        fill_rect(
            canvas,
            canvas_width,
            canvas_height,
            rect_tuple(separator),
            opaque_rgba(palette.frame),
        );
    }

    let mut used = HashSet::new();
    let header = picker_buffer_rect(layout.header, scale_120);
    let header_inset = 14_u32.saturating_mul(scale_120).div_ceil(120);
    let header_content = Rect {
        x: header.x.saturating_add(header_inset),
        y: header.y,
        width: header.width.saturating_sub(header_inset.saturating_mul(2)),
        height: if layout.mode == SessionPickerPresentationMode::Normal {
            header.height / 2
        } else {
            header.height
        },
    };
    let header_title = Rect {
        width: if layout.mode == SessionPickerPresentationMode::Minimal {
            header_content.width
        } else {
            header_content.width.saturating_mul(2) / 3
        },
        ..header_content
    };
    paint_picker_text(
        cache,
        context,
        &mut used,
        canvas,
        canvas_width,
        canvas_height,
        if layout.mode == SessionPickerPresentationMode::Minimal {
            "SESSIONS"
        } else {
            "RECENT SESSIONS"
        },
        ChromeTextStyle::Bold,
        scale_120,
        renderer_generation,
        header_title,
        PickerTextAlignment::Left,
        palette.primary,
    )?;
    if layout.mode != SessionPickerPresentationMode::Minimal {
        let count = format!("{} available", items.len());
        paint_picker_text(
            cache,
            context,
            &mut used,
            canvas,
            canvas_width,
            canvas_height,
            &count,
            ChromeTextStyle::Regular,
            scale_120,
            renderer_generation,
            Rect {
                x: header_title.x.saturating_add(header_title.width),
                width: header_content.width.saturating_sub(header_title.width),
                ..header_content
            },
            PickerTextAlignment::Right,
            palette.secondary,
        )?;
    }
    if layout.mode == SessionPickerPresentationMode::Normal {
        paint_picker_text(
            cache,
            context,
            &mut used,
            canvas,
            canvas_width,
            canvas_height,
            "Switch to a running Dojo.",
            ChromeTextStyle::Regular,
            scale_120,
            renderer_generation,
            Rect {
                x: header_content.x,
                y: header.y.saturating_add(header.height / 2),
                width: header_content.width,
                height: header.height.saturating_sub(header.height / 2),
            },
            PickerTextAlignment::Left,
            palette.secondary,
        )?;
    }

    for row in &layout.rows {
        let row_buffer = picker_buffer_rect(row.surface, scale_120);
        let row_enabled = new_enabled || !matches!(row.target, PickerHitTarget::New);
        let is_selected = row_enabled && row.target == selected;
        if is_selected {
            fill_rect(
                canvas,
                canvas_width,
                canvas_height,
                rect_tuple(row_buffer),
                opaque_rgba(palette.selected_fill),
            );
            let rail_width = 3_u32.saturating_mul(scale_120).div_ceil(120).max(1);
            fill_rect(
                canvas,
                canvas_width,
                canvas_height,
                (
                    i32::try_from(row_buffer.x).unwrap_or(i32::MAX),
                    i32::try_from(row_buffer.y).unwrap_or(i32::MAX),
                    rail_width,
                    row_buffer.height,
                ),
                opaque_rgba(palette.selected_rail),
            );
        } else if row_enabled && hovered == Some(row.target) {
            blend_rect(
                canvas,
                canvas_width,
                canvas_height,
                rect_tuple(row_buffer),
                [
                    rgb_channels(palette.selected_fill)[0],
                    rgb_channels(palette.selected_fill)[1],
                    rgb_channels(palette.selected_fill)[2],
                    64,
                ],
            );
        }
        if row_enabled && pressed == Some(row.target) {
            blend_rect(
                canvas,
                canvas_width,
                canvas_height,
                rect_tuple(row_buffer),
                [
                    rgb_channels(palette.selected_rail)[0],
                    rgb_channels(palette.selected_rail)[1],
                    rgb_channels(palette.selected_rail)[2],
                    48,
                ],
            );
        }
        let primary = if !row_enabled {
            palette.secondary
        } else if is_selected {
            palette.selected_primary
        } else {
            palette.primary
        };
        let secondary = if is_selected {
            palette.selected_secondary
        } else {
            palette.secondary
        };
        let marker_width = 22_u32.saturating_mul(scale_120).div_ceil(120);
        if is_selected {
            paint_picker_text(
                cache,
                context,
                &mut used,
                canvas,
                canvas_width,
                canvas_height,
                "›",
                ChromeTextStyle::Bold,
                scale_120,
                renderer_generation,
                Rect {
                    x: row_buffer.x.saturating_add(border),
                    y: row_buffer.y,
                    width: marker_width,
                    height: row_buffer.height,
                },
                PickerTextAlignment::Center,
                primary,
            )?;
        }
        let content = Rect {
            x: row_buffer.x.saturating_add(marker_width),
            y: row_buffer.y,
            width: row_buffer
                .width
                .saturating_sub(marker_width.saturating_add(header_inset)),
            height: row_buffer.height,
        };
        let (title, working_directory, status) = match row.target {
            PickerHitTarget::New if new_enabled => {
                ("+ New terminal", "", "Start a fresh shell".to_owned())
            }
            PickerHitTarget::New => (
                "+ New terminal",
                "",
                "Policy republish and reopen required".to_owned(),
            ),
            PickerHitTarget::Open(index) => {
                let Some(item) = items.get(index) else {
                    continue;
                };
                let status = if layout.mode == SessionPickerPresentationMode::Normal {
                    format!("{}/{} running", item.running_pane_count, item.pane_count)
                } else {
                    format!("{}/{}", item.running_pane_count, item.pane_count)
                };
                (item.display_title, item.working_directory, status)
            }
        };
        if layout.mode == SessionPickerPresentationMode::Normal
            && matches!(row.target, PickerHitTarget::Open(_))
        {
            let top = Rect {
                x: content.x,
                y: content.y,
                width: content.width,
                height: content.height / 2,
            };
            let title_width = top.width.saturating_mul(2) / 3;
            let title_rect = Rect {
                width: title_width,
                ..top
            };
            let status_rect = Rect {
                x: top.x.saturating_add(title_width),
                width: top.width.saturating_sub(title_width),
                ..top
            };
            paint_picker_text(
                cache,
                context,
                &mut used,
                canvas,
                canvas_width,
                canvas_height,
                title,
                ChromeTextStyle::Bold,
                scale_120,
                renderer_generation,
                title_rect,
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
                &status,
                ChromeTextStyle::Regular,
                scale_120,
                renderer_generation,
                status_rect,
                PickerTextAlignment::Right,
                secondary,
            )?;
            paint_picker_text(
                cache,
                context,
                &mut used,
                canvas,
                canvas_width,
                canvas_height,
                working_directory,
                ChromeTextStyle::Regular,
                scale_120,
                renderer_generation,
                Rect {
                    x: content.x,
                    y: content.y.saturating_add(content.height / 2),
                    width: content.width,
                    height: content.height.saturating_sub(content.height / 2),
                },
                PickerTextAlignment::Left,
                secondary,
            )?;
        } else {
            let title_width = if layout.mode == SessionPickerPresentationMode::Minimal {
                content.width
            } else {
                content.width.saturating_mul(2) / 3
            };
            let title_rect = Rect {
                width: title_width,
                ..content
            };
            paint_picker_text(
                cache,
                context,
                &mut used,
                canvas,
                canvas_width,
                canvas_height,
                title,
                ChromeTextStyle::Bold,
                scale_120,
                renderer_generation,
                title_rect,
                PickerTextAlignment::Left,
                primary,
            )?;
            if layout.mode != SessionPickerPresentationMode::Minimal {
                paint_picker_text(
                    cache,
                    context,
                    &mut used,
                    canvas,
                    canvas_width,
                    canvas_height,
                    &status,
                    ChromeTextStyle::Regular,
                    scale_120,
                    renderer_generation,
                    Rect {
                        x: content.x.saturating_add(title_width),
                        width: content.width.saturating_sub(title_width),
                        ..content
                    },
                    PickerTextAlignment::Right,
                    secondary,
                )?;
            }
        }
    }

    let footer = picker_buffer_rect(layout.footer, scale_120);
    let footer_text = match layout.mode {
        SessionPickerPresentationMode::Normal => {
            "↑↓ / J K navigate   Enter open   N new   Esc cancel"
        }
        SessionPickerPresentationMode::Compact => "↑↓ navigate   Enter open   Esc cancel",
        SessionPickerPresentationMode::Minimal => "Enter open   Esc cancel",
    };
    paint_picker_text(
        cache,
        context,
        &mut used,
        canvas,
        canvas_width,
        canvas_height,
        footer_text,
        ChromeTextStyle::Regular,
        scale_120,
        renderer_generation,
        footer,
        PickerTextAlignment::Center,
        palette.secondary,
    )?;
    cache.finish_frame(used);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::super::pixel_index;
    use super::*;

    #[test]
    fn picker_text_truncation_reserves_ellipsis_and_preserves_combining_marks() {
        assert_eq!(truncate_picker_text("short", 5), "short");
        assert_eq!(truncate_picker_text("abcdef", 5), "abcd…");
        assert_eq!(truncate_picker_text("界界界", 5), "界界…");
        assert_eq!(
            truncate_picker_text("e\u{301}e\u{301}xy", 3),
            "e\u{301}e\u{301}…"
        );
        assert_eq!(truncate_picker_text("anything", 0), "");
    }

    #[test]
    fn session_picker_layout_is_responsive_bounded_and_scale_deterministic() {
        for (width, height, expected_mode) in [
            (960, 600, SessionPickerPresentationMode::Normal),
            (400, 240, SessionPickerPresentationMode::Compact),
            (240, 140, SessionPickerPresentationMode::Minimal),
        ] {
            let mut previous = None;
            for scale in [120, 150, 240] {
                let layout = session_picker_overlay_layout(width, height, scale, 256, 256, 0)
                    .expect("picker layout");
                assert_eq!(layout.mode, expected_mode);
                assert!(layout.panel.x + layout.panel.width <= width);
                assert!(layout.panel.y + layout.panel.height <= height);
                assert!(layout.rows.iter().all(|row| {
                    row.rect.x >= layout.panel.x
                        && row.rect.y >= layout.panel.y
                        && row.rect.x + row.rect.width <= layout.panel.x + layout.panel.width
                        && row.rect.y + row.rect.height <= layout.panel.y + layout.panel.height
                        && row.surface.x >= row.rect.x
                        && row.surface.y >= row.rect.y
                        && row.surface.x + row.surface.width <= row.rect.x + row.rect.width
                        && row.surface.y + row.surface.height <= row.rect.y + row.rect.height
                }));
                for pair in layout.rows.windows(2) {
                    assert_eq!(pair[0].rect.y + pair[0].rect.height, pair[1].rect.y);
                    let first = picker_buffer_rect(pair[0].surface, scale);
                    let second = picker_buffer_rect(pair[1].surface, scale);
                    assert!(first.y + first.height < second.y);
                }
                let first = &layout.rows[0].surface;
                match expected_mode {
                    SessionPickerPresentationMode::Normal => {
                        assert_eq!(first.x, layout.action.x + 8);
                        assert_eq!(first.y, layout.action.y + 3);
                        assert_eq!(first.width, layout.action.width - 16);
                        assert_eq!(first.height, layout.action.height - 6);
                    }
                    SessionPickerPresentationMode::Compact => {
                        assert_eq!(first.x, layout.action.x + 6);
                        assert_eq!(first.y, layout.action.y + 3);
                        assert_eq!(first.width, layout.action.width - 12);
                        assert_eq!(first.height, layout.action.height - 6);
                    }
                    SessionPickerPresentationMode::Minimal => {
                        assert_eq!(*first, layout.action);
                    }
                }
                let header = picker_buffer_rect(layout.header, scale);
                let action = picker_buffer_rect(layout.action, scale);
                let footer = picker_buffer_rect(layout.footer, scale);
                assert_eq!(header.y + header.height, action.y);
                assert!(action.y + action.height <= footer.y);
                assert!(layout.rows.iter().any(|row| {
                    row.target == PickerHitTarget::Open(255)
                        || expected_mode != SessionPickerPresentationMode::Minimal
                            && layout.visible_range.contains(&255)
                }));
                let geometry = (
                    layout.panel,
                    layout.header,
                    layout.action,
                    layout.list,
                    layout.footer,
                    layout.rows.clone(),
                    layout.visible_range.clone(),
                    layout.mode,
                );
                if let Some(previous) = &previous {
                    assert_eq!(previous, &geometry);
                }
                previous = Some(geometry);
            }
        }
    }

    #[test]
    fn minimal_picker_prioritizes_pointer_target_height() {
        let constrained = session_picker_overlay_layout(240, 50, 150, 8, 2, 0).unwrap();
        assert_eq!(constrained.mode, SessionPickerPresentationMode::Minimal);
        assert_eq!(constrained.action.height, 44);
        let impossible = session_picker_overlay_layout(240, 40, 150, 8, 2, 0).unwrap();
        assert_eq!(impossible.action.height, 40);
    }

    #[test]
    fn session_picker_hit_rectangles_are_half_open_and_stable() {
        let layout = session_picker_overlay_layout(960, 600, 120, 8, 1, 0).unwrap();
        let first = &layout.rows[0];
        assert_eq!(
            session_picker_hit_test(&layout, (f64::from(first.rect.x), f64::from(first.rect.y))),
            Some(PickerHitTarget::New)
        );
        assert_ne!(
            session_picker_hit_test(
                &layout,
                (
                    f64::from(first.rect.x + first.rect.width),
                    f64::from(first.rect.y)
                )
            ),
            Some(PickerHitTarget::New)
        );
        assert_eq!(
            session_picker_hit_test(
                &layout,
                (
                    f64::from(layout.action.x + 1),
                    f64::from(layout.action.y + 1)
                )
            ),
            Some(PickerHitTarget::New),
            "visual menu padding must preserve the full pointer target"
        );
    }

    #[test]
    fn session_picker_palette_corrects_low_contrast_text() {
        for theme in [
            ResolvedTheme::default(),
            ResolvedTheme {
                background: 0x77_77_77,
                foreground: 0x78_78_78,
                selection: 0x79_79_79,
                pane_border: 0x77_77_77,
                pane_border_active: 0x77_77_77,
                ui_accent: 0xee_55_44,
                ..ResolvedTheme::default()
            },
            ResolvedTheme {
                background: 0xf5_f1_e8,
                foreground: 0x22_24_28,
                selection: 0xb8_d8_f0,
                pane_border: 0x76_78_7c,
                pane_border_active: 0x18_62_9f,
                ui_accent: 0x18_62_9f,
                ..ResolvedTheme::default()
            },
        ] {
            let palette = session_picker_palette(theme);
            assert!(contrast_ratio(palette.primary, palette.panel) >= 4.5);
            assert!(contrast_ratio(palette.secondary, palette.panel) >= 4.5);
            assert!(contrast_ratio(palette.selected_primary, palette.selected_fill) >= 4.5);
            assert!(contrast_ratio(palette.selected_secondary, palette.selected_fill) >= 4.5);
            assert_eq!(palette.scrim[3], 140);
            assert_eq!(palette.shadow[3], 89);
        }
        let blue_theme = ResolvedTheme {
            background: 0xff_ff_ff,
            foreground: 0x00_66_cc,
            selection: 0xd8_ea_f8,
            ..ResolvedTheme::default()
        };
        let blue_palette = session_picker_palette(blue_theme);
        assert!(contrast_ratio(blue_palette.secondary, blue_palette.panel) >= 4.5);
        assert_ne!(blue_palette.secondary, 0);
        assert_ne!(blue_palette.secondary, 0x00ff_ffff);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn session_picker_painter_marks_transient_chrome_without_rebuilding_text() {
        let theme = ResolvedTheme::default();
        let palette = session_picker_palette(theme);
        let layout = session_picker_overlay_layout(960, 600, 120, 2, 0, 0).unwrap();
        let items = [
            SessionPickerTextItem {
                display_title: "work / editor",
                working_directory: "/work",
                pane_count: 2,
                running_pane_count: 2,
            },
            SessionPickerTextItem {
                display_title: "notes",
                working_directory: "/notes",
                pane_count: 1,
                running_pane_count: 1,
            },
        ];
        let [_, red, green, blue] = theme.background.to_be_bytes();
        let mut canvas = vec![0_u8; 960 * 600 * 4];
        for pixel in canvas.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[blue, green, red, 0xff]);
        }
        let mut cache = SessionPickerTextCache::default();
        paint_session_picker_overlay(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            960,
            600,
            Rect {
                x: 0,
                y: 0,
                width: 960,
                height: 600,
            },
            120,
            1,
            &layout,
            palette,
            &items,
            PickerHitTarget::New,
            None,
            None,
            true,
            true,
        )
        .unwrap();
        let shaped = cache.len();
        assert!(shaped > 0);
        assert!(
            cache
                .entries
                .iter()
                .all(|(key, text)| text.pixel_width() <= key.constrained_width)
        );
        let outside = pixel_index(960, 600, 0, 0).unwrap();
        assert_ne!(&canvas[outside..outside + 4], &[blue, green, red, 0xff]);
        let frame = pixel_index(
            960,
            600,
            i32::try_from(layout.panel.x).unwrap(),
            i32::try_from(layout.panel.y).unwrap(),
        )
        .unwrap();
        let [_, frame_red, frame_green, frame_blue] = palette.focused_frame.to_be_bytes();
        assert_eq!(
            &canvas[frame..frame + 4],
            &[frame_blue, frame_green, frame_red, 0xff]
        );
        let rail = pixel_index(
            960,
            600,
            i32::try_from(layout.rows[0].surface.x).unwrap(),
            i32::try_from(layout.rows[0].surface.y + layout.rows[0].surface.height / 2).unwrap(),
        )
        .unwrap();
        let [_, accent_red, accent_green, accent_blue] = theme.ui_accent.to_be_bytes();
        assert_eq!(
            &canvas[rail..rail + 4],
            &[accent_blue, accent_green, accent_red, 0xff]
        );
        paint_session_picker_overlay(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            960,
            600,
            Rect {
                x: 0,
                y: 0,
                width: 960,
                height: 600,
            },
            120,
            1,
            &layout,
            palette,
            &items,
            PickerHitTarget::Open(0),
            None,
            None,
            true,
            true,
        )
        .unwrap();
        assert_eq!(cache.len(), shaped);

        paint_session_picker_overlay(
            &mut cache,
            &RenderContext::new(u16::MAX),
            &mut canvas,
            960,
            600,
            Rect {
                x: 0,
                y: 0,
                width: 960,
                height: 600,
            },
            120,
            1,
            &layout,
            palette,
            &items,
            PickerHitTarget::New,
            Some(PickerHitTarget::New),
            Some(PickerHitTarget::New),
            false,
            true,
        )
        .unwrap();
        assert_ne!(
            &canvas[rail..rail + 4],
            &[accent_blue, accent_green, accent_red, 0xff]
        );
        assert!(cache.len() <= shaped + 1);
    }

    #[test]
    fn session_picker_text_cache_stays_bounded_for_large_catalogs() {
        let owned = (0..256)
            .map(|index| (format!("session {index}"), format!("/work/{index}")))
            .collect::<Vec<_>>();
        let items = owned
            .iter()
            .map(|(title, cwd)| SessionPickerTextItem {
                display_title: title,
                working_directory: cwd,
                pane_count: 2,
                running_pane_count: 2,
            })
            .collect::<Vec<_>>();
        let theme = ResolvedTheme::default();
        let mut cache = SessionPickerTextCache::default();
        let mut canvas = vec![0_u8; 960 * 600 * 4];
        let mut visible_start = 0;
        for selected in [0, 32, 96, 255] {
            let layout = session_picker_overlay_layout(
                960,
                600,
                120,
                items.len(),
                selected + 1,
                visible_start,
            )
            .unwrap();
            visible_start = layout.visible_range.start;
            paint_session_picker_overlay(
                &mut cache,
                &RenderContext::new(u16::MAX),
                &mut canvas,
                960,
                600,
                Rect {
                    x: 0,
                    y: 0,
                    width: 960,
                    height: 600,
                },
                120,
                1,
                &layout,
                session_picker_palette(theme),
                &items,
                PickerHitTarget::Open(selected),
                None,
                None,
                true,
                true,
            )
            .unwrap();
        }
        assert!(
            cache.len() < 128,
            "cache held {} shaped strings",
            cache.len()
        );
    }
}
