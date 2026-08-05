//! Shaped trusted-chrome text backed by the terminal renderer's existing frame path.

use anyhow::{Context, Result};
use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, TerminalCell, TerminalInputModes, TerminalRow,
    TerminalSnapshot, UnderlineStyle,
};
use unicode_width::UnicodeWidthChar;

use crate::geometry::Rect;

use super::{SnapshotFrame, blend_glyph, packed_rgb, round_to_i32};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ChromeTextStyle {
    Regular,
    Bold,
}

pub(crate) struct ChromeText {
    pub(super) frame: SnapshotFrame,
    cells: u32,
}

impl ChromeText {
    pub(crate) fn load(text: &str, scale_120: u32) -> Result<Self> {
        Self::load_styled(text, scale_120, ChromeTextStyle::Regular)
    }

    pub(crate) fn load_styled(text: &str, scale_120: u32, style: ChromeTextStyle) -> Result<Self> {
        let attributes = CellAttributes {
            bold: style == ChromeTextStyle::Bold,
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
        };
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
        Ok(Self {
            frame: SnapshotFrame::load_scaled(&snapshot, scale_120)?,
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
        assert!(canvas.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }
}
