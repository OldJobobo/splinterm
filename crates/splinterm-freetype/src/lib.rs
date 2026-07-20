#![forbid(unsafe_code)]

//! Bounded owned wrapper around `FreeType`'s Foot-compatible grayscale path.
//!
//! Native state remains private to this crate. Callers provide a selected font
//! file/index and receive owned, tightly packed alpha bytes; no `FreeType` pointer
//! or borrowed bitmap escapes the API.

use std::path::{Path, PathBuf};

use freetype::{
    Face, Library, RenderMode, bitmap::PixelMode, face::LoadFlag, tt_os2::TrueTypeOS2Table,
};
use thiserror::Error;

pub const MIN_PIXEL_SIZE_26_6: isize = 6 * 64;
pub const MAX_PIXEL_SIZE_26_6: isize = 768 * 64;
pub const MAX_GLYPH_DIMENSION: u32 = 4_096;
pub const MAX_GLYPH_PIXELS: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FontMetrics {
    pub ascent: i32,
    pub descent: i32,
    pub height: i32,
    pub underline_position: i32,
    pub underline_thickness: i32,
    pub strike_position: i32,
    pub strike_thickness: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterizedGlyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub advance_x: i32,
    pub advance_y: i32,
    pub alpha: Box<[u8]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterizedColorGlyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub advance_x: i32,
    pub advance_y: i32,
    pub rgba: Box<[u8]>,
}

#[derive(Debug, Error)]
pub enum RasterError {
    #[error("physical pixel size must be between 6 and 768 pixels in 26.6 units")]
    InvalidPixelSize,
    #[error("font face index does not fit FreeType's signed index")]
    InvalidFaceIndex,
    #[error("initialize FreeType: {0}")]
    Initialize(freetype::Error),
    #[error("open font face {path} index {index}: {source}")]
    OpenFace {
        path: PathBuf,
        index: u32,
        source: freetype::Error,
    },
    #[error("set FreeType character size: {0}")]
    SetSize(freetype::Error),
    #[error("load glyph {glyph_id}: {source}")]
    LoadGlyph {
        glyph_id: u32,
        source: freetype::Error,
    },
    #[error("render glyph {glyph_id}: {source}")]
    RenderGlyph {
        glyph_id: u32,
        source: freetype::Error,
    },
    #[error("glyph {glyph_id} produced unsupported FreeType pixel mode {mode:?}")]
    UnsupportedPixelMode { glyph_id: u32, mode: PixelMode },
    #[error("glyph bitmap dimensions or pitch are invalid")]
    InvalidBitmapGeometry,
    #[error("glyph bitmap exceeds the bounded raster budget")]
    BitmapTooLarge,
    #[error("glyph bitmap buffer is shorter than its declared geometry")]
    TruncatedBitmap,
    #[error("FreeType advance does not fit the owned raster contract")]
    InvalidAdvance,
    #[error("scale color glyph with pixman: {0}")]
    ScaleColor(#[from] splinterm_pixman::ScaleError),
}

pub struct RasterFace {
    face: Face,
}

impl RasterFace {
    /// Opens one font face and applies a fractional pixel size at 72 DPI, which
    /// is the sizing contract used by fcft.
    ///
    /// # Errors
    /// Returns a typed error for invalid bounds or any `FreeType` failure.
    pub fn open(
        path: impl AsRef<Path>,
        face_index: u32,
        pixel_size_26_6: isize,
    ) -> Result<Self, RasterError> {
        if !(MIN_PIXEL_SIZE_26_6..=MAX_PIXEL_SIZE_26_6).contains(&pixel_size_26_6) {
            return Err(RasterError::InvalidPixelSize);
        }
        let index = isize::try_from(face_index).map_err(|_| RasterError::InvalidFaceIndex)?;
        let library = Library::init().map_err(RasterError::Initialize)?;
        let path = path.as_ref();
        let face = library
            .new_face(path, index)
            .map_err(|source| RasterError::OpenFace {
                path: path.to_path_buf(),
                index: face_index,
                source,
            })?;
        face.set_char_size(pixel_size_26_6, 0, 72, 72)
            .map_err(RasterError::SetSize)?;
        Ok(Self { face })
    }

    /// Rasterize one fixed-strike color glyph using pinned fcft's pixel fixup.
    ///
    /// `selected_pixel_size_26_6` is Fontconfig's actual fixed strike size;
    /// `requested_pixel_size_26_6` is the terminal's effective requested size.
    ///
    /// # Errors
    /// Returns a typed error for invalid bounds, `FreeType` failures, non-BGRA
    /// output, malformed bitmap geometry, or pixman scaling failures.
    pub fn rasterize_color(
        path: impl AsRef<Path>,
        face_index: u32,
        requested_pixel_size_26_6: isize,
        selected_pixel_size_26_6: isize,
        glyph_id: u32,
    ) -> Result<RasterizedColorGlyph, RasterError> {
        if !(MIN_PIXEL_SIZE_26_6..=MAX_PIXEL_SIZE_26_6).contains(&requested_pixel_size_26_6)
            || !(MIN_PIXEL_SIZE_26_6..=MAX_PIXEL_SIZE_26_6).contains(&selected_pixel_size_26_6)
        {
            return Err(RasterError::InvalidPixelSize);
        }
        let index = isize::try_from(face_index).map_err(|_| RasterError::InvalidFaceIndex)?;
        let library = Library::init().map_err(RasterError::Initialize)?;
        let path = path.as_ref();
        let face = library
            .new_face(path, index)
            .map_err(|source| RasterError::OpenFace {
                path: path.to_path_buf(),
                index: face_index,
                source,
            })?;
        face.set_char_size(selected_pixel_size_26_6, 0, 72, 72)
            .map_err(RasterError::SetSize)?;
        face.load_glyph(
            glyph_id,
            LoadFlag::DEFAULT | LoadFlag::TARGET_LIGHT | LoadFlag::COLOR,
        )
        .map_err(|source| RasterError::LoadGlyph { glyph_id, source })?;
        if face.glyph().raw().format != freetype::ffi::FT_GLYPH_FORMAT_BITMAP {
            face.glyph()
                .render_glyph(RenderMode::Normal)
                .map_err(|source| RasterError::RenderGlyph { glyph_id, source })?;
        }
        let slot = face.glyph();
        let bitmap = slot.bitmap();
        let mode = bitmap
            .pixel_mode()
            .map_err(|_| RasterError::InvalidBitmapGeometry)?;
        if mode != PixelMode::Bgra {
            return Err(RasterError::UnsupportedPixelMode { glyph_id, mode });
        }
        let width =
            u32::try_from(bitmap.width()).map_err(|_| RasterError::InvalidBitmapGeometry)?;
        let height =
            u32::try_from(bitmap.rows()).map_err(|_| RasterError::InvalidBitmapGeometry)?;
        let bgra = normalize_bgra_bitmap(
            bitmap.buffer(),
            bitmap.width(),
            bitmap.rows(),
            bitmap.pitch(),
        )?;
        let requested =
            i32::try_from(requested_pixel_size_26_6).map_err(|_| RasterError::InvalidPixelSize)?;
        let selected =
            i32::try_from(selected_pixel_size_26_6).map_err(|_| RasterError::InvalidPixelSize)?;
        let pixel_fixup = f64::from(requested) / f64::from(selected);
        let (scaled_width, scaled_height, rgba) =
            splinterm_pixman::scale_bgra_lanczos3(&bgra, width, height, pixel_fixup)?;
        let advance = slot.advance();
        Ok(RasterizedColorGlyph {
            left: scale_trunc(slot.bitmap_left(), pixel_fixup)?,
            top: scale_trunc(slot.bitmap_top(), pixel_fixup)?,
            width: scaled_width,
            height: scaled_height,
            advance_x: scale_advance_trunc(advance.x, pixel_fixup)?,
            advance_y: scale_advance_trunc(advance.y, pixel_fixup)?,
            rgba: rgba.into_boxed_slice(),
        })
    }

    /// Rasterizes one glyph with `FreeType` light hinting and normal grayscale,
    /// matching the pinned fcft regular-ASCII policy.
    ///
    /// # Errors
    /// Returns a typed error for `FreeType` failures, non-gray output, malformed
    /// bitmap geometry, or exceeded bounds.
    pub fn rasterize_gray(&mut self, glyph_id: u32) -> Result<RasterizedGlyph, RasterError> {
        self.face
            .load_glyph(glyph_id, LoadFlag::DEFAULT | LoadFlag::TARGET_LIGHT)
            .map_err(|source| RasterError::LoadGlyph { glyph_id, source })?;
        self.face
            .glyph()
            .render_glyph(RenderMode::Normal)
            .map_err(|source| RasterError::RenderGlyph { glyph_id, source })?;
        let slot = self.face.glyph();
        let bitmap = slot.bitmap();
        let mode = bitmap
            .pixel_mode()
            .map_err(|_| RasterError::InvalidBitmapGeometry)?;
        if !matches!(mode, PixelMode::Gray | PixelMode::None) {
            return Err(RasterError::UnsupportedPixelMode { glyph_id, mode });
        }
        let alpha = normalize_gray_bitmap(
            bitmap.buffer(),
            bitmap.width(),
            bitmap.rows(),
            bitmap.pitch(),
        )?;
        let width =
            u32::try_from(bitmap.width()).map_err(|_| RasterError::InvalidBitmapGeometry)?;
        let height =
            u32::try_from(bitmap.rows()).map_err(|_| RasterError::InvalidBitmapGeometry)?;
        let advance = slot.advance();
        Ok(RasterizedGlyph {
            left: slot.bitmap_left(),
            top: slot.bitmap_top(),
            width,
            height,
            advance_x: i32::try_from(advance.x / 64).map_err(|_| RasterError::InvalidAdvance)?,
            advance_y: i32::try_from(advance.y / 64).map_err(|_| RasterError::InvalidAdvance)?,
            alpha,
        })
    }

    /// Returns the integer extents exposed by fcft for this configured face.
    ///
    /// # Errors
    /// Returns an error when `FreeType` has no active size or an extent cannot
    /// fit the bounded public contract.
    #[allow(
        clippy::cast_precision_loss,
        reason = "FreeType 26.6 metrics are bounded font extents"
    )]
    pub fn metrics(&mut self) -> Result<FontMetrics, RasterError> {
        let metrics = self
            .face
            .size_metrics()
            .ok_or(RasterError::InvalidBitmapGeometry)?;
        let ascent = metrics.ascender as f64 / 64.0;
        let descent = metrics.descender as f64 / 64.0;
        let y_scale = metrics.y_scale as f64 / 65_536.0;
        let mut underline_position = f64::from(self.face.underline_position()) * y_scale / 64.0;
        let mut underline_thickness = f64::from(self.face.underline_thickness()) * y_scale / 64.0;
        if underline_position == 0.0 {
            underline_thickness = (descent / 5.0).abs();
            underline_position = -2.0 * underline_thickness;
        }
        let underline_top = (underline_position + underline_thickness / 2.0).trunc();
        let underline_width = underline_thickness.max(1.0).round();
        let (mut strike_position, mut strike_thickness) =
            TrueTypeOS2Table::from_face(&mut self.face).map_or((0.0, 0.0), |os2| {
                (
                    f64::from(os2.y_strikeout_position()) * y_scale / 64.0,
                    f64::from(os2.y_strikeout_size()) * y_scale / 64.0,
                )
            });
        if strike_position == 0.0 {
            strike_thickness = underline_thickness;
            strike_position = 3.0 * ascent / 8.0 - strike_thickness / 2.0;
        }
        Ok(FontMetrics {
            ascent: ceil_26_6(metrics.ascender)?,
            descent: ceil_26_6(-metrics.descender)?,
            // fcft rounds the complete hinted face height once. Independently
            // ceiling ascent/descent adds one pixel for common terminal faces.
            height: round_26_6(metrics.height)?,
            underline_position: float_to_i32(underline_top)?,
            underline_thickness: float_to_i32(underline_width)?,
            strike_position: float_to_i32((strike_position + strike_thickness / 2.0).trunc())?,
            strike_thickness: float_to_i32(strike_thickness.max(1.0).round())?,
        })
    }

    #[must_use]
    pub fn glyph_index(&self, character: char) -> u32 {
        self.face.get_char_index(character as usize).unwrap_or(0)
    }
}

#[allow(clippy::cast_possible_truncation)]
fn float_to_i32(value: f64) -> Result<i32, RasterError> {
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(RasterError::InvalidBitmapGeometry);
    }
    Ok(value as i32)
}

fn ceil_26_6(value: i64) -> Result<i32, RasterError> {
    let pixels = value.div_euclid(64) + i64::from(value.rem_euclid(64) != 0);
    i32::try_from(pixels).map_err(|_| RasterError::InvalidBitmapGeometry)
}

fn round_26_6(value: i64) -> Result<i32, RasterError> {
    let pixels = value
        .checked_add(32)
        .ok_or(RasterError::InvalidBitmapGeometry)?
        .div_euclid(64);
    i32::try_from(pixels).map_err(|_| RasterError::InvalidBitmapGeometry)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn scale_trunc(value: i32, pixel_fixup: f64) -> Result<i32, RasterError> {
    let scaled = f64::from(value) * pixel_fixup;
    if !scaled.is_finite() || scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
        return Err(RasterError::InvalidBitmapGeometry);
    }
    Ok(scaled as i32)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn scale_advance_trunc(value_26_6: i64, pixel_fixup: f64) -> Result<i32, RasterError> {
    let scaled = value_26_6 as f64 / 64.0 * pixel_fixup;
    if !scaled.is_finite() || scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
        return Err(RasterError::InvalidAdvance);
    }
    Ok(scaled as i32)
}

fn normalize_bgra_bitmap(
    source: &[u8],
    width: i32,
    height: i32,
    pitch: i32,
) -> Result<Box<[u8]>, RasterError> {
    let width = u32::try_from(width).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    let height = u32::try_from(height).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    if width > MAX_GLYPH_DIMENSION || height > MAX_GLYPH_DIMENSION {
        return Err(RasterError::BitmapTooLarge);
    }
    let row_bytes = usize::try_from(width)
        .map_err(|_| RasterError::InvalidBitmapGeometry)?
        .checked_mul(4)
        .ok_or(RasterError::BitmapTooLarge)?;
    let rows = usize::try_from(height).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    let pitch_abs =
        usize::try_from(pitch.unsigned_abs()).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    if row_bytes > pitch_abs && rows != 0 {
        return Err(RasterError::InvalidBitmapGeometry);
    }
    let bytes = row_bytes
        .checked_mul(rows)
        .ok_or(RasterError::BitmapTooLarge)?;
    if bytes / 4 > MAX_GLYPH_PIXELS {
        return Err(RasterError::BitmapTooLarge);
    }
    let source_len = pitch_abs
        .checked_mul(rows)
        .ok_or(RasterError::BitmapTooLarge)?;
    if source.len() < source_len {
        return Err(RasterError::TruncatedBitmap);
    }
    let mut output = Vec::with_capacity(bytes);
    for output_row in 0..rows {
        let source_row = if pitch < 0 {
            rows.saturating_sub(output_row + 1)
        } else {
            output_row
        };
        let start = source_row
            .checked_mul(pitch_abs)
            .ok_or(RasterError::BitmapTooLarge)?;
        output.extend_from_slice(&source[start..start + row_bytes]);
    }
    Ok(output.into_boxed_slice())
}

fn normalize_gray_bitmap(
    source: &[u8],
    width: i32,
    height: i32,
    pitch: i32,
) -> Result<Box<[u8]>, RasterError> {
    let width = u32::try_from(width).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    let height = u32::try_from(height).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    if width > MAX_GLYPH_DIMENSION || height > MAX_GLYPH_DIMENSION {
        return Err(RasterError::BitmapTooLarge);
    }
    let row_bytes = usize::try_from(width).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    let rows = usize::try_from(height).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    let pitch_abs =
        usize::try_from(pitch.unsigned_abs()).map_err(|_| RasterError::InvalidBitmapGeometry)?;
    if row_bytes > pitch_abs && rows != 0 {
        return Err(RasterError::InvalidBitmapGeometry);
    }
    let pixels = row_bytes
        .checked_mul(rows)
        .ok_or(RasterError::BitmapTooLarge)?;
    if pixels > MAX_GLYPH_PIXELS {
        return Err(RasterError::BitmapTooLarge);
    }
    let source_len = pitch_abs
        .checked_mul(rows)
        .ok_or(RasterError::BitmapTooLarge)?;
    if source.len() < source_len {
        return Err(RasterError::TruncatedBitmap);
    }
    let mut output = Vec::with_capacity(pixels);
    for output_row in 0..rows {
        let source_row = if pitch < 0 {
            rows.saturating_sub(output_row + 1)
        } else {
            output_row
        };
        let start = source_row
            .checked_mul(pitch_abs)
            .ok_or(RasterError::BitmapTooLarge)?;
        output.extend_from_slice(&source[start..start + row_bytes]);
    }
    Ok(output.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_positive_pitch_and_removes_padding() {
        let result = normalize_gray_bitmap(&[1, 2, 9, 9, 3, 4, 9, 9], 2, 2, 4).unwrap();
        assert_eq!(&*result, &[1, 2, 3, 4]);
    }

    #[test]
    fn normalizes_negative_pitch_to_top_first_rows() {
        let result = normalize_gray_bitmap(&[3, 4, 1, 2], 2, 2, -2).unwrap();
        assert_eq!(&*result, &[1, 2, 3, 4]);
    }

    #[test]
    fn accepts_empty_space_bitmap() {
        assert!(normalize_gray_bitmap(&[], 0, 0, 0).unwrap().is_empty());
    }

    #[test]
    fn rejects_truncated_and_invalid_geometry() {
        assert!(matches!(
            normalize_gray_bitmap(&[1], 2, 1, 2),
            Err(RasterError::TruncatedBitmap)
        ));
        assert!(matches!(
            normalize_gray_bitmap(&[], 2, 1, 1),
            Err(RasterError::InvalidBitmapGeometry)
        ));
        assert!(matches!(
            normalize_gray_bitmap(&[], -1, 1, 1),
            Err(RasterError::InvalidBitmapGeometry)
        ));
    }

    #[test]
    fn rejects_sizes_outside_terminal_policy() {
        let missing = "/definitely/missing/font.ttf";
        assert!(matches!(
            RasterFace::open(missing, 0, 0),
            Err(RasterError::InvalidPixelSize)
        ));
        assert!(matches!(
            RasterFace::open(missing, 0, MAX_PIXEL_SIZE_26_6 + 1),
            Err(RasterError::InvalidPixelSize)
        ));
    }
}
