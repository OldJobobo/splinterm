//! Narrow safe pixman scaling bridge for pinned fcft color glyph parity.

use std::{mem::MaybeUninit, ptr};

use pixman_sys as ffi;
use thiserror::Error;

const MAX_COLOR_PIXELS: usize = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ScaleError {
    #[error("color bitmap geometry or scale is invalid")]
    InvalidGeometry,
    #[error("color bitmap exceeds the bounded raster budget")]
    BitmapTooLarge,
    #[error("color bitmap bytes do not match the declared geometry")]
    InvalidBuffer,
    #[error("pixman failed to create or configure a color image")]
    Pixman,
}

struct Image(*mut ffi::pixman_image_t);

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: `Image` is constructed only from non-null owned pixman images.
        unsafe {
            ffi::pixman_image_unref(self.0);
        }
    }
}

/// Scale premultiplied `FreeType` BGRA with fcft's pinned pixman cubic policy.
///
/// Returned bytes are tightly packed premultiplied RGBA for Splinterm's color
/// compositor. Dimensions use fcft's truncating `source * pixel_fixup` rule.
///
/// # Errors
/// Returns an error for invalid or oversized geometry, inconsistent input
/// bytes, or any pixman image/filter setup failure.
#[allow(
    clippy::cast_possible_truncation,
    clippy::float_cmp,
    clippy::too_many_lines,
    reason = "this is a bounded line-for-line fcft/pixman scaling adapter"
)]
pub fn scale_bgra_cubic(
    bgra: &[u8],
    width: u32,
    height: u32,
    pixel_fixup: f64,
) -> Result<(u32, u32, Vec<u8>), ScaleError> {
    if width == 0 || height == 0 || !pixel_fixup.is_finite() || pixel_fixup <= 0.0 {
        return Err(ScaleError::InvalidGeometry);
    }
    let source_pixels = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(usize::try_from(height).ok()?))
        .ok_or(ScaleError::BitmapTooLarge)?;
    if source_pixels > MAX_COLOR_PIXELS {
        return Err(ScaleError::BitmapTooLarge);
    }
    if bgra.len()
        != source_pixels
            .checked_mul(4)
            .ok_or(ScaleError::BitmapTooLarge)?
    {
        return Err(ScaleError::InvalidBuffer);
    }
    if pixel_fixup == 1.0 {
        let mut rgba = Vec::with_capacity(bgra.len());
        for pixel in bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
        return Ok((width, height, rgba));
    }

    let scaled_width = trunc_dimension(width, pixel_fixup)?;
    let scaled_height = trunc_dimension(height, pixel_fixup)?;
    let scaled_pixels = usize::try_from(scaled_width)
        .ok()
        .and_then(|value| value.checked_mul(usize::try_from(scaled_height).ok()?))
        .ok_or(ScaleError::BitmapTooLarge)?;
    if scaled_pixels == 0 || scaled_pixels > MAX_COLOR_PIXELS {
        return Err(ScaleError::BitmapTooLarge);
    }

    let mut source: Vec<u32> = bgra
        .chunks_exact(4)
        .map(|pixel| {
            u32::from(pixel[3]) << 24
                | u32::from(pixel[2]) << 16
                | u32::from(pixel[1]) << 8
                | u32::from(pixel[0])
        })
        .collect();
    let mut destination = vec![0_u32; scaled_pixels];
    let source_stride = width.checked_mul(4).ok_or(ScaleError::BitmapTooLarge)?;
    let destination_stride = scaled_width
        .checked_mul(4)
        .ok_or(ScaleError::BitmapTooLarge)?;

    // SAFETY: both backing vectors are sized for their declared dimensions and
    // outlive the pixman images. Every returned pointer is checked before use.
    unsafe {
        let source_image = ffi::pixman_image_create_bits_no_clear(
            ffi::pixman_format_code_t_PIXMAN_a8r8g8b8,
            i32::try_from(width).map_err(|_| ScaleError::InvalidGeometry)?,
            i32::try_from(height).map_err(|_| ScaleError::InvalidGeometry)?,
            source.as_mut_ptr(),
            i32::try_from(source_stride).map_err(|_| ScaleError::InvalidGeometry)?,
        );
        if source_image.is_null() {
            return Err(ScaleError::Pixman);
        }
        let source_image = Image(source_image);
        let destination_image = ffi::pixman_image_create_bits_no_clear(
            ffi::pixman_format_code_t_PIXMAN_a8r8g8b8,
            i32::try_from(scaled_width).map_err(|_| ScaleError::InvalidGeometry)?,
            i32::try_from(scaled_height).map_err(|_| ScaleError::InvalidGeometry)?,
            destination.as_mut_ptr(),
            i32::try_from(destination_stride).map_err(|_| ScaleError::InvalidGeometry)?,
        );
        if destination_image.is_null() {
            return Err(ScaleError::Pixman);
        }
        let destination_image = Image(destination_image);

        let inverse = 1.0 / pixel_fixup;
        let mut floating = MaybeUninit::<ffi::pixman_f_transform>::uninit();
        ffi::pixman_f_transform_init_scale(floating.as_mut_ptr(), inverse, inverse);
        let floating = floating.assume_init();
        let mut transform = MaybeUninit::<ffi::pixman_transform>::uninit();
        ffi::pixman_transform_from_pixman_f_transform(transform.as_mut_ptr(), &raw const floating);
        let transform = transform.assume_init();
        if ffi::pixman_image_set_transform(source_image.0, &raw const transform) == 0 {
            return Err(ScaleError::Pixman);
        }

        let mut parameter_count = 0;
        let fixed_inverse = (inverse * 65_536.0) as ffi::pixman_fixed_t;
        let parameters = ffi::pixman_filter_create_separable_convolution(
            &raw mut parameter_count,
            fixed_inverse,
            fixed_inverse,
            ffi::pixman_kernel_t_PIXMAN_KERNEL_CUBIC,
            ffi::pixman_kernel_t_PIXMAN_KERNEL_CUBIC,
            ffi::pixman_kernel_t_PIXMAN_KERNEL_CUBIC,
            ffi::pixman_kernel_t_PIXMAN_KERNEL_CUBIC,
            1,
            1,
        );
        if parameters.is_null() || parameter_count <= 0 {
            return Err(ScaleError::Pixman);
        }
        let filter_ok = ffi::pixman_image_set_filter(
            source_image.0,
            ffi::pixman_filter_t_PIXMAN_FILTER_SEPARABLE_CONVOLUTION,
            parameters,
            parameter_count,
        );
        libc::free(parameters.cast());
        if filter_ok == 0 {
            return Err(ScaleError::Pixman);
        }
        ffi::pixman_image_composite32(
            ffi::pixman_op_t_PIXMAN_OP_SRC,
            source_image.0,
            ptr::null_mut(),
            destination_image.0,
            0,
            0,
            0,
            0,
            0,
            0,
            i32::try_from(scaled_width).map_err(|_| ScaleError::InvalidGeometry)?,
            i32::try_from(scaled_height).map_err(|_| ScaleError::InvalidGeometry)?,
        );
    }

    let mut rgba = Vec::with_capacity(scaled_pixels * 4);
    for pixel in destination {
        rgba.extend_from_slice(&[
            ((pixel >> 16) & 0xff) as u8,
            ((pixel >> 8) & 0xff) as u8,
            (pixel & 0xff) as u8,
            ((pixel >> 24) & 0xff) as u8,
        ]);
    }
    Ok((scaled_width, scaled_height, rgba))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated positive bounded dimensions intentionally use fcft's truncation"
)]
fn trunc_dimension(value: u32, scale: f64) -> Result<u32, ScaleError> {
    let scaled = f64::from(value) * scale;
    if !scaled.is_finite() || scaled < 1.0 || scaled > f64::from(i32::MAX) {
        return Err(ScaleError::InvalidGeometry);
    }
    Ok(scaled as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_scale_converts_premultiplied_bgra_to_rgba() {
        let source = [0x10, 0x20, 0x30, 0x40, 0x01, 0x02, 0x03, 0xff];
        let (width, height, rgba) = scale_bgra_cubic(&source, 2, 1, 1.0).unwrap();
        assert_eq!((width, height), (2, 1));
        assert_eq!(rgba, [0x30, 0x20, 0x10, 0x40, 0x03, 0x02, 0x01, 0xff]);
    }

    #[test]
    fn invalid_or_unbounded_inputs_are_rejected() {
        assert!(matches!(
            scale_bgra_cubic(&[], 0, 1, 1.0),
            Err(ScaleError::InvalidGeometry)
        ));
        assert!(matches!(
            scale_bgra_cubic(&[0; 3], 1, 1, 1.0),
            Err(ScaleError::InvalidBuffer)
        ));
    }
}
