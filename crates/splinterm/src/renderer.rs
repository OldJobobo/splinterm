//! Deterministic CPU text-row rasterization for the native client.
//!
//! Font selection, cell placement, fallback, and CPU blending are compared against
//! Foot 1.27.0 `fonts.c` and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`. It also owns the persistent,
//! scale-keyed glyph cache and damage-oriented terminal snapshot painter.

#[cfg(test)]
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    sync::Arc,
};

#[cfg(test)]
use crate::config::CursorStyle;
use crate::geometry::OutputDpiObservation;
#[cfg(test)]
use crate::geometry::{Rect, TerminalPadding};
use anyhow::Result;

#[cfg(test)]
use splinterm_automation_client::ImageContentSource;
#[cfg(test)]
use splinterm_core::SplintId;
#[cfg(test)]
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, ImageContentMetadata, ImagePlacement, MAX_COLUMNS,
    MAX_ROWS, ScrollDirection, TerminalCell, TerminalInputModes, TerminalRow, TerminalScroll,
    TerminalSnapshot, UnderlineStyle,
};
#[cfg(test)]
use swash::scale::image::Content;
mod capture;
mod compose;
mod cursor;
mod decorations;
mod evidence;
mod fonts;
mod frame;
mod images;
mod overlays;
mod raster;
mod settings;
mod text;

pub(crate) use crate::frontend::PickerHitTarget;
#[cfg(test)]
use capture::capture_prepared_frame;
pub use capture::{
    FinalBufferCapture, capture_final_buffer, capture_final_buffer_in_context,
    capture_final_buffer_presented, capture_final_buffer_presented_in_context,
    capture_final_buffer_sized, capture_final_buffer_sized_presented,
    capture_final_buffer_with_sources,
};
#[cfg(test)]
use compose::{paint_glyphs, paint_placed_glyph};
pub(crate) use compose::{
    paint_snapshot, paint_snapshot_presented, paint_snapshot_region_presented, paint_snapshot_rows,
    paint_snapshot_rows_presented, scroll_snapshot_pixels, snapshot_row_rect,
};
#[cfg(test)]
use cursor::paint_effective_cursor;
pub use cursor::{
    CursorPresentation, EffectiveCursorShape, UnfocusedCursorStyle, effective_cursor_shape,
};
use decorations::DecorationMetrics;
#[cfg(test)]
use decorations::DecorationSpan;
pub use evidence::{benchmark_json, phase4_benchmark_json, write_ppm};
pub(super) use fonts::*;
pub use fonts::{ascii_glyph_evidence, production_ascii_glyph_evidence, snapshot_cache_metrics};
pub(crate) use frame::SnapshotFrame;
use frame::{SnapshotGlyph, packed_rgb};
#[cfg(test)]
use frame::{
    cell_is_renderable, default_background, default_foreground, leader_span, primary_face_index,
    rendition_colors,
};
#[cfg(test)]
use images::{KITTY_BACKGROUND_Z_THRESHOLD, SnapshotImage, compare_snapshot_images, image_tier};
pub(crate) use overlays::actions::{
    CommandPaletteLayout, CommandPaletteTextCache, DojoPromptLayout, TabContextMenuLayout,
    command_palette_hit_test, command_palette_layout, dojo_prompt_hit_test, dojo_prompt_layout,
    paint_command_palette, paint_dojo_prompt, paint_tab_context_menu, tab_context_menu_hit_test,
    tab_context_menu_layout,
};
#[allow(
    unused_imports,
    reason = "preserve the crate-local renderer facade for the inferred layout return type"
)]
pub(crate) use overlays::history::{
    HistoryOverlayLayout, HistoryOverlayStatus, SnapshotOverlays, history_overlay_layout,
    paint_history_overlay, paint_snapshot_overlays,
};
pub(crate) use overlays::picker::{
    SessionPickerOverlayLayout, SessionPickerTextCache, SessionPickerTextItem,
    paint_session_picker_overlay, session_picker_hit_test, session_picker_overlay_layout,
    session_picker_palette,
};
#[cfg(test)]
use raster::pixel_index;
use raster::{alpha_u8, blend_glyph, blend_rect, premultiplied_rgba};
pub(crate) use raster::{background_bgra, fill_rect, paint_box_drawing_cell};
use settings::{
    BASE_FONT_SIZE, PRIMARY_FONT, compatibility_render_context, effective_font_size,
    renderer_options,
};
pub use settings::{
    RenderContext, RendererOptions, RendererResources, configure, effective_font_resolution,
};
pub(crate) use text::{ChromeText, ChromeTextStyle};

pub(crate) fn premultiplied_theme_rgba(color: u32, alpha: u16) -> [u8; 4] {
    premultiplied_rgba(
        [
            ((color >> 16) & 0xff) as u8,
            ((color >> 8) & 0xff) as u8,
            (color & 0xff) as u8,
        ],
        alpha_u8(alpha),
    )
}

impl RenderContext {
    pub(crate) fn set_font_zoom_steps(
        &mut self,
        steps: i16,
        surface_scale_120: u32,
    ) -> Result<Option<bool>> {
        let changed = self.apply_font_zoom_steps(steps, surface_scale_120)?;
        if changed.is_some() {
            clear_snapshot_caches();
        }
        Ok(changed)
    }

    /// Updates this context's output observation and invalidates size-dependent caches.
    ///
    /// # Errors
    /// Returns an error for invalid scale, DPI, font resolution, or cache state.
    pub fn update_output_dpi(
        &mut self,
        observation: OutputDpiObservation,
        surface_scale_120: u32,
    ) -> Result<bool> {
        let changed = self.apply_output_dpi(observation, surface_scale_120)?;
        if changed {
            clear_snapshot_caches();
        }
        Ok(changed)
    }
}

/// Updates the active output DPI and clears font caches when its raster size changes.
///
/// # Errors
/// Returns an error for invalid scale/DPI/font resolution or poisoned renderer state.
pub fn update_output_dpi(
    observation: OutputDpiObservation,
    surface_scale_120: u32,
) -> Result<bool> {
    let changed = settings::update_output_dpi(observation, surface_scale_120)?;
    if changed {
        clear_snapshot_caches();
    }
    Ok(changed)
}

#[cfg(test)]
mod tests;
