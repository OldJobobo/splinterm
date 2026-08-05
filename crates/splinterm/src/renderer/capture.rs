//! Deterministic final-buffer capture over the production frame compositor.

use anyhow::{Context, Result};
use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_protocol::TerminalSnapshot;

use crate::{
    config::CursorStyle,
    geometry::{BufferPadding, TerminalPadding, WindowGeometry},
};

use super::{
    CursorPresentation, SnapshotFrame, paint_snapshot_presented,
    raster::{background_alpha_u8, premultiplied_rgba},
};

/// Exact output of the production snapshot painter before Wayland submission.
#[derive(Clone, Debug)]
pub struct FinalBufferCapture {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub ascent: u32,
    pub descent: u32,
    pub baseline: i32,
    pub requested_padding: TerminalPadding,
    pub effective_base_padding: BufferPadding,
    pub residual_right: u32,
    pub residual_bottom: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub grid_rect: crate::geometry::Rect,
    pub visible_grid_rect: crate::geometry::Rect,
    pub padding_left: u32,
    pub padding_right: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub origin_x: u32,
    pub origin_y: u32,
    pub cursor: Option<(u32, u32)>,
    pub background_bgra: [u8; 4],
}

/// Paints an owned terminal snapshot through `SnapshotFrame` and the production
/// full-frame compositor into a tightly packed little-endian ARGB8888 buffer.
///
/// # Errors
/// Returns an error for invalid snapshot geometry, scale, font state, or buffer
/// size overflow.
pub fn capture_final_buffer(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
) -> Result<FinalBufferCapture> {
    capture_final_buffer_presented(
        snapshot,
        scale_120,
        show_cursor,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    )
}

/// Paints a snapshot with resolved immutable image sources.
///
/// # Errors
/// Returns an error for missing/mismatched sources, geometry, scale, font, or allocation bounds.
pub fn capture_final_buffer_with_sources(
    snapshot: &TerminalSnapshot,
    sources: &ImageContentLeaseSet,
    scale_120: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
) -> Result<FinalBufferCapture> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let frame = SnapshotFrame::load_scaled_with_sources(snapshot, scale_120, Some(sources))?;
    let geometry = frame.tight_geometry()?;
    capture_prepared_frame(
        &frame,
        geometry,
        show_cursor,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    )
}

/// Captures with an explicit semantic focus/unfocused-cursor policy.
///
/// # Errors
/// Returns an error for invalid snapshot, font, scale, geometry, or allocation bounds.
pub fn capture_final_buffer_presented(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) -> Result<FinalBufferCapture> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let frame = SnapshotFrame::load_scaled(snapshot, scale_120)?;
    let geometry = frame.tight_geometry()?;
    capture_prepared_frame(&frame, geometry, show_cursor, cursor_style, presentation)
}

/// Paints a snapshot into an explicitly configured logical surface.
///
/// This is used by the Foot oracle when the compositor contributes trailing
/// residual pixels to an otherwise fixed cell grid.
///
/// # Errors
/// Returns an error if the surface does not fit exactly the snapshot grid.
pub fn capture_final_buffer_sized(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    logical_width: u32,
    logical_height: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
) -> Result<FinalBufferCapture> {
    capture_final_buffer_sized_presented(
        snapshot,
        scale_120,
        logical_width,
        logical_height,
        show_cursor,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    )
}

/// Sized capture with an explicit semantic focus/unfocused-cursor policy.
///
/// # Errors
/// Returns an error when the declared surface cannot contain the exact snapshot grid.
#[allow(
    clippy::too_many_arguments,
    reason = "capture geometry and cursor policy are explicit"
)]
pub fn capture_final_buffer_sized_presented(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    logical_width: u32,
    logical_height: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) -> Result<FinalBufferCapture> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let frame = SnapshotFrame::load_scaled(snapshot, scale_120)?;
    let geometry = WindowGeometry::grid_in_surface(
        frame.columns,
        frame.rows,
        logical_width,
        logical_height,
        frame.cell_geometry()?,
        frame.padding,
        scale_120,
    )?;
    capture_prepared_frame(&frame, geometry, show_cursor, cursor_style, presentation)
}

pub(super) fn capture_prepared_frame(
    frame: &SnapshotFrame,
    geometry: WindowGeometry,
    show_cursor: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) -> Result<FinalBufferCapture> {
    let origin_x = geometry.actual_padding.left;
    let origin_y = geometry.actual_padding.top;
    let width = geometry.buffer_width();
    let height = geometry.buffer_height();
    let stride = width.checked_mul(4).context("capture stride overflow")?;
    let length = usize::try_from(stride)
        .ok()
        .and_then(|stride| usize::try_from(height).ok()?.checked_mul(stride))
        .context("capture allocation overflow")?;
    let mut pixels = vec![0_u8; length];
    paint_snapshot_presented(
        &mut pixels,
        width,
        height,
        frame,
        &geometry,
        show_cursor,
        cursor_style,
        presentation,
    );
    Ok(FinalBufferCapture {
        pixels,
        width,
        height,
        stride,
        columns: frame.columns,
        rows: frame.rows,
        cell_width: frame.cell_width,
        cell_height: frame.cell_height,
        ascent: frame.ascent,
        descent: frame.descent,
        baseline: frame.baseline,
        requested_padding: geometry.requested_padding,
        effective_base_padding: geometry.effective_base_padding,
        residual_right: geometry.residual_right,
        residual_bottom: geometry.residual_bottom,
        logical_width: geometry.logical_width(),
        logical_height: geometry.logical_height(),
        grid_rect: geometry.grid_rect,
        visible_grid_rect: geometry.visible_grid_rect,
        padding_left: geometry.actual_padding.left,
        padding_right: geometry.actual_padding.right,
        padding_top: geometry.actual_padding.top,
        padding_bottom: geometry.actual_padding.bottom,
        origin_x,
        origin_y,
        cursor: show_cursor.then_some(frame.cursor).flatten(),
        background_bgra: {
            let background = premultiplied_rgba(frame.canvas_background, background_alpha_u8());
            [background[2], background[1], background[0], background[3]]
        },
    })
}
