//! Shaped trusted-chrome text backed by the terminal renderer's existing frame path.

use anyhow::{Context, Result};
use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, TerminalCell, TerminalInputModes, TerminalRow,
    TerminalSnapshot, UnderlineStyle,
};
use unicode_width::UnicodeWidthChar;

use crate::geometry::Rect;

use super::{
    GlyphKey, RenderContext, SnapshotFrame, SnapshotGlyph, blend_glyph,
    icons::{ChromeIcon, ICON_FACE, IconCell, icon_glyph},
    packed_rgb, round_to_i32, u32_to_f32,
};

/// The tightest fixed chrome text line, in logical pixels: an Explorer row's
/// name and status lines and a saved-layout preview line are each this tall,
/// and every other chrome row is taller.
///
/// Chrome text follows the terminal font, runtime zoom included, only until
/// its cell fills this line. Past that it stops growing, because chrome rows
/// have fixed heights: at maximum zoom, glyphs larger than their rows were
/// clipped into unreadable fragments.
pub(crate) const CHROME_MAXIMUM_LINE_HEIGHT: u32 = 24;

/// The tallest chrome text cell, in buffer pixels, at `scale_120`.
pub(crate) const fn chrome_maximum_cell_height(scale_120: u32) -> u32 {
    CHROME_MAXIMUM_LINE_HEIGHT.saturating_mul(scale_120) / 120
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ChromeTextStyle {
    Regular,
    Bold,
}

pub(crate) struct ChromeText {
    pub(super) frame: SnapshotFrame,
    cells: u32,
}

fn text_cells(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0).min(2))
        .sum()
}

/// Trusted chrome text with icons in reserved cells.
///
/// Each icon holds [`ChromeIcon::CELLS`] spaces in `text` and is drawn from the
/// embedded icon face. Icons enter only through [`Self::push_icon`]: no
/// character in a label, title, or agent detail ever becomes an icon.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct ChromeSource {
    text: String,
    /// `(first cell, icon)`, in cell order.
    icons: Vec<(u32, ChromeIcon)>,
}

impl ChromeSource {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub(crate) fn icons(&self) -> &[(u32, ChromeIcon)] {
        &self.icons
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Display cells, counting each icon as [`ChromeIcon::CELLS`].
    pub(crate) fn cells(&self) -> usize {
        text_cells(&self.text)
    }

    pub(crate) fn push_str(&mut self, text: &str) -> &mut Self {
        self.text.push_str(text);
        self
    }

    pub(crate) fn push_icon(&mut self, icon: ChromeIcon) -> &mut Self {
        let cell = u32::try_from(self.cells()).unwrap_or(u32::MAX);
        self.icons.push((cell, icon));
        for _ in 0..ChromeIcon::CELLS {
            self.text.push(' ');
        }
        self
    }

    pub(crate) fn push_source(&mut self, other: &Self) -> &mut Self {
        let offset = u32::try_from(self.cells()).unwrap_or(u32::MAX);
        self.icons.extend(
            other
                .icons
                .iter()
                .map(|(cell, icon)| (cell.saturating_add(offset), *icon)),
        );
        self.text.push_str(&other.text);
        self
    }

    /// Keeps `text`, a prefix of this source's text possibly ended by an
    /// ellipsis, and only the icons wholly inside its first `kept_cells`.
    pub(super) fn with_prefix_text(&self, text: String, kept_cells: usize) -> Self {
        let kept_cells = u32::try_from(kept_cells).unwrap_or(u32::MAX);
        Self {
            text,
            icons: self
                .icons
                .iter()
                .copied()
                .filter(|(cell, _)| cell.saturating_add(ChromeIcon::CELLS) <= kept_cells)
                .collect(),
        }
    }

    /// The text with each icon written as `[name]`, for assertions.
    #[cfg(test)]
    pub(crate) fn describe(&self) -> String {
        let mut described = String::new();
        let mut cell = 0_u32;
        let mut skip = 0_u32;
        for character in self.text.chars() {
            let width = u32::try_from(character.width().unwrap_or(0).min(2)).unwrap_or(0);
            if skip > 0 && width > 0 {
                skip -= 1;
                cell += width;
                continue;
            }
            if let Some((_, icon)) = self.icons.iter().find(|(start, _)| *start == cell) {
                described.push('[');
                described.push_str(icon.name());
                described.push(']');
                skip = ChromeIcon::CELLS - 1;
            } else {
                described.push(character);
            }
            cell += width;
        }
        described
    }
}

impl From<&str> for ChromeSource {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            icons: Vec::new(),
        }
    }
}

impl From<String> for ChromeSource {
    fn from(text: String) -> Self {
        Self {
            text,
            icons: Vec::new(),
        }
    }
}

const fn chrome_attributes(style: ChromeTextStyle) -> CellAttributes {
    CellAttributes {
        bold: matches!(style, ChromeTextStyle::Bold),
        dim: false,
        italic: false,
        underline: UnderlineStyle::None,
        underline_color_source: ColorSource::Default,
        underline_color: 0,
        strikethrough: false,
        blink: false,
        conceal: false,
        reverse: false,
        foreground_source: ColorSource::Default,
        foreground: 0,
        background_source: ColorSource::Default,
        background: 0,
    }
}

/// Adds each icon to a chrome frame as a mask glyph at its slot's first cell.
/// Icons past the frame or missing from the face leave their blank cells.
fn place_icons(
    frame: &mut SnapshotFrame,
    icons: &[(u32, ChromeIcon)],
    cell_count: u32,
) -> Result<()> {
    let cell = IconCell {
        width: frame.cell_width,
        height: frame.cell_height,
        ascent: frame.ascent,
        baseline: frame.baseline,
    };
    for (column, icon) in icons {
        if column.saturating_add(ChromeIcon::CELLS) > cell_count {
            continue;
        }
        let Some(glyph) = icon_glyph(*icon, frame.font_generation.id, frame.font_size, cell) else {
            continue;
        };
        let key = GlyphKey {
            face: ICON_FACE,
            glyph: u16::try_from(*icon as usize).context("icon index fits u16")?,
        };
        frame.cache.insert(key, glyph);
        frame.glyphs.push(SnapshotGlyph {
            key,
            column: *column,
            row: 0,
            cells: ChromeIcon::CELLS,
            cluster_advance: u32_to_f32(frame.cell_width.saturating_mul(ChromeIcon::CELLS)),
            x_offset: 0.0,
            y_offset: 0.0,
            foreground: [0xff; 3],
        });
    }
    Ok(())
}

impl ChromeText {
    #[cfg(test)]
    pub(crate) fn load(text: &str, scale_120: u32) -> Result<Self> {
        Self::load_styled(text, scale_120, ChromeTextStyle::Regular)
    }

    pub(crate) fn load_with_context(
        text: &str,
        scale_120: u32,
        context: &RenderContext,
    ) -> Result<Self> {
        Self::load_styled_with_context(text, scale_120, ChromeTextStyle::Regular, context)
    }

    #[cfg(test)]
    pub(crate) fn load_styled(text: &str, scale_120: u32, style: ChromeTextStyle) -> Result<Self> {
        let context = super::compatibility_render_context()?;
        Self::load_styled_with_context(text, scale_120, style, &context)
    }

    #[cfg(test)]
    pub(crate) fn load_source_styled(
        source: &ChromeSource,
        scale_120: u32,
        style: ChromeTextStyle,
    ) -> Result<Self> {
        let context = super::compatibility_render_context()?;
        Self::load_source_styled_with_context(source, scale_120, style, &context)
    }

    pub(crate) fn load_styled_with_context(
        text: &str,
        scale_120: u32,
        style: ChromeTextStyle,
        context: &RenderContext,
    ) -> Result<Self> {
        Self::load_parts(
            text,
            &[],
            scale_120,
            style,
            context,
            chrome_maximum_cell_height(scale_120),
        )
    }

    pub(crate) fn load_source_styled_with_context(
        source: &ChromeSource,
        scale_120: u32,
        style: ChromeTextStyle,
        context: &RenderContext,
    ) -> Result<Self> {
        Self::load_parts(
            &source.text,
            &source.icons,
            scale_120,
            style,
            context,
            chrome_maximum_cell_height(scale_120),
        )
    }

    /// Loads like [`Self::load_source_styled_with_context`], with a cell at
    /// most `maximum_cell_height` buffer pixels tall and never taller than the
    /// chrome line cap. A chrome row shorter than that cap, such as a tab
    /// strip that follows a short Omarchy bar, keeps its text inside the row.
    pub(crate) fn load_source_styled_within(
        source: &ChromeSource,
        scale_120: u32,
        style: ChromeTextStyle,
        context: &RenderContext,
        maximum_cell_height: u32,
    ) -> Result<Self> {
        Self::load_parts(
            &source.text,
            &source.icons,
            scale_120,
            style,
            context,
            maximum_cell_height.min(chrome_maximum_cell_height(scale_120)),
        )
    }

    fn load_parts(
        text: &str,
        icons: &[(u32, ChromeIcon)],
        scale_120: u32,
        style: ChromeTextStyle,
        context: &RenderContext,
        maximum_cell_height: u32,
    ) -> Result<Self> {
        let attributes = chrome_attributes(style);
        let mut cell_row: Vec<TerminalCell> = Vec::new();
        for character in text.chars() {
            let width = character.width().unwrap_or(0).min(2);
            if width == 0 {
                if let Some(leader) = cell_row
                    .iter_mut()
                    .rev()
                    .find(|cell| cell.spacer_remaining.is_none())
                {
                    leader.content.push(character);
                }
                continue;
            }
            cell_row.push(TerminalCell {
                content: character.to_string(),
                spacer_remaining: None,
                attributes,
            });
            if width == 2 {
                cell_row.push(TerminalCell {
                    content: String::new(),
                    spacer_remaining: Some(1),
                    attributes,
                });
            }
        }
        let cell_count = u32::try_from(cell_row.len()).context("chrome title width fits u32")?;
        let columns = cell_count.max(1);
        let snapshot = TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision: 1,
            columns: usize::try_from(columns).context("chrome title columns fit usize")?,
            rows: 1,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: false,
                cursor_blink: false,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00ff_ffff, 0, 0],
            title: String::new(),
            visible_rows: vec![TerminalRow {
                row_id: None,
                linebreak: false,
                cells: {
                    cell_row.resize(
                        usize::try_from(columns).context("chrome title row width fits usize")?,
                        TerminalCell {
                            content: String::new(),
                            spacer_remaining: None,
                            attributes,
                        },
                    );
                    cell_row
                },
            }],
            history_generation: 1,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            images: None,
            exited_code: None,
            exited_signal: None,
        };
        let mut frame = SnapshotFrame::load_scaled_with_maximum_cell_height(
            &snapshot,
            scale_120,
            context,
            maximum_cell_height,
        )?;
        place_icons(&mut frame, icons, cell_count)?;
        Ok(Self {
            frame,
            cells: cell_count,
        })
    }

    pub(crate) const fn cells(&self) -> u32 {
        self.cells
    }

    pub(crate) const fn pixel_width(&self) -> u32 {
        self.cells.saturating_mul(self.frame.cell_width)
    }

    pub(crate) const fn pixel_height(&self) -> u32 {
        self.frame.cell_height
    }

    pub(crate) fn paint(
        &self,
        canvas: &mut [u8],
        width: u32,
        height: u32,
        origin: (u32, u32),
        clip: Rect,
        color: u32,
    ) {
        let foreground = packed_rgb(color);
        let clip = (
            i32::try_from(clip.x).unwrap_or(i32::MAX),
            i32::try_from(clip.y).unwrap_or(i32::MAX),
            i32::try_from(clip.x.saturating_add(clip.width)).unwrap_or(i32::MAX),
            i32::try_from(clip.y.saturating_add(clip.height)).unwrap_or(i32::MAX),
        );
        for placed in &self.frame.glyphs {
            let glyph = &self.frame.cache[&placed.key];
            let x = origin
                .0
                .saturating_add(placed.column.saturating_mul(self.frame.cell_width));
            let baseline = origin
                .1
                .saturating_add(u32::try_from(self.frame.baseline).unwrap_or(0));
            blend_glyph(
                canvas,
                width,
                height,
                i32::try_from(x).unwrap_or(i32::MAX) + round_to_i32(placed.x_offset) + glyph.left,
                i32::try_from(baseline).unwrap_or(i32::MAX)
                    - round_to_i32(placed.y_offset)
                    - glyph.top,
                glyph,
                foreground,
                Some(clip),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_text_keeps_combining_clusters_and_wide_cell_spans() {
        let text = ChromeText::load("e\u{301}界", 120).unwrap();
        assert_eq!(text.cells(), 3);
        let mut canvas = vec![0; 256 * 64 * 4];
        text.paint(
            &mut canvas,
            256,
            64,
            (8, 0),
            Rect {
                x: 0,
                y: 0,
                width: 256,
                height: 64,
            },
            0x12_34_56,
        );
        assert!(canvas.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0));
    }

    /// VM case E6: after about 180 zoom-in steps chrome drew terminal-sized
    /// glyphs inside fixed chrome rows, clipping every label into fragments.
    #[test]
    fn chrome_text_stops_growing_at_its_line_height_at_maximum_zoom() {
        for scale in [120, 150, 180, 240] {
            let context = RenderContext::new(u16::MAX).at_maximum_zoom(scale);
            let bound = chrome_maximum_cell_height(scale);
            // The terminal itself keeps the zoomed size, far taller than a
            // chrome line; only chrome text is bounded.
            let zoomed = context.effective_font_size(scale).unwrap();
            let primary = &context.font_generation().unwrap().faces[0];
            assert!(super::super::cell_metrics(primary, zoomed).unwrap().height > bound * 3);
            for style in [ChromeTextStyle::Regular, ChromeTextStyle::Bold] {
                let text = ChromeText::load_styled_with_context(
                    "Preview saved layout",
                    scale,
                    style,
                    &context,
                )
                .unwrap();
                assert!(
                    text.pixel_height() <= bound,
                    "scale {scale}: {} > {bound}",
                    text.pixel_height()
                );
                // Bounded, not collapsed: it still fills most of the line.
                assert!(text.pixel_height() * 4 >= bound * 3, "scale {scale}");
                assert!(text.frame.font_size < zoomed);
            }
        }
    }

    #[test]
    fn chrome_text_below_its_line_height_keeps_the_terminal_font_size() {
        let mut context = RenderContext::new(u16::MAX);
        context.apply_font_zoom_steps(-10, 120).unwrap().unwrap();
        let size = context.effective_font_size(120).unwrap();
        let text = ChromeText::load_with_context("Lair 1", 120, &context).unwrap();
        assert!(text.pixel_height() <= chrome_maximum_cell_height(120));
        assert_eq!(text.frame.font_size.to_bits(), size.to_bits());
    }
}
