use std::io::{Cursor, Read};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;

use crate::{ImageAlphaMode, ImageSourceFormat};

pub(crate) const MAX_CONTROL_BYTES: usize = 1024;
pub(crate) const MAX_ENCODED_CHUNK_BYTES: usize = 4096;
pub(crate) const MAX_ENCODED_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_DECODED_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXPANSION_RATIO: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    Transmit,
    TransmitAndDisplay,
    Display,
    Query,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Medium {
    Direct,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Format {
    Rgb,
    Rgba,
    Png,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Command {
    pub action: Action,
    pub medium: Medium,
    pub format: Format,
    pub compressed: bool,
    pub more: bool,
    pub image_id: u32,
    pub placement_id: u32,
    pub quiet: u8,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: Option<u32>,
    pub source_height: Option<u32>,
    pub columns: Option<usize>,
    pub rows: Option<usize>,
    pub x_offset: i32,
    pub y_offset: i32,
    pub z_index: i32,
    pub no_cursor_move: bool,
    pub delete_selector: Option<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub format: ImageSourceFormat,
    pub alpha_mode: ImageAlphaMode,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Error {
    Invalid(&'static str),
    Unsupported(&'static str),
    NoSpace(&'static str),
    TooBig(&'static str),
    BadMessage(&'static str),
    NotFound(&'static str),
}

impl Error {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Invalid(_) => "EINVAL",
            Self::Unsupported(_) => "ENOTSUP",
            Self::NoSpace(_) => "ENOSPC",
            Self::TooBig(_) => "E2BIG",
            Self::BadMessage(_) => "EBADMSG",
            Self::NotFound(_) => "ENOENT",
        }
    }

    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Invalid(message)
            | Self::Unsupported(message)
            | Self::NoSpace(message)
            | Self::TooBig(message)
            | Self::BadMessage(message)
            | Self::NotFound(message) => message,
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the bounded one-byte Kitty key table is clearest in one exhaustive parser"
)]
pub(crate) fn parse_control(control: &[u8], truncated: bool) -> Result<Command, Error> {
    if truncated || control.len() > MAX_CONTROL_BYTES {
        return Err(Error::TooBig("control data exceeds limit"));
    }
    let mut command = Command {
        action: Action::Transmit,
        medium: Medium::Direct,
        format: Format::Rgba,
        compressed: false,
        more: false,
        image_id: 0,
        placement_id: 0,
        quiet: 0,
        width: None,
        height: None,
        source_x: 0,
        source_y: 0,
        source_width: None,
        source_height: None,
        columns: None,
        rows: None,
        x_offset: 0,
        y_offset: 0,
        z_index: 0,
        no_cursor_move: false,
        delete_selector: None,
    };
    if control.is_empty() {
        return Ok(command);
    }
    let mut seen = [false; 256];
    for field in control.split(|byte| *byte == b',') {
        let Some(separator) = field.iter().position(|byte| *byte == b'=') else {
            return Err(Error::Invalid("malformed control field"));
        };
        let (key, value_with_separator) = field.split_at(separator);
        let value = &value_with_separator[1..];
        if key.len() != 1 || value.is_empty() {
            return Err(Error::Invalid("malformed control field"));
        }
        let key_index = usize::from(key[0]);
        if std::mem::replace(&mut seen[key_index], true) {
            return Err(Error::Invalid("duplicate control field"));
        }
        let number = || parse_u32(value);
        match key[0] {
            b'a' if value.len() == 1 => {
                command.action = match value[0] {
                    b't' => Action::Transmit,
                    b'T' => Action::TransmitAndDisplay,
                    b'p' => Action::Display,
                    b'q' => Action::Query,
                    b'd' => Action::Delete,
                    _ => return Err(Error::Unsupported("unsupported action")),
                };
            }
            b't' if value.len() == 1 => {
                command.medium = if value[0] == b'd' {
                    Medium::Direct
                } else {
                    Medium::Unsupported
                };
            }
            b'f' => {
                command.format = match number()? {
                    24 => Format::Rgb,
                    32 => Format::Rgba,
                    100 => Format::Png,
                    _ => return Err(Error::Unsupported("unsupported pixel format")),
                };
            }
            b'o' if value == b"z" => command.compressed = true,
            b'o' => return Err(Error::Unsupported("unsupported compression")),
            b'm' => command.more = parse_bool(value)?,
            b'i' => command.image_id = number()?,
            b'p' => command.placement_id = number()?,
            b'q' => {
                command.quiet =
                    u8::try_from(number()?).map_err(|_| Error::Invalid("invalid quiet mode"))?;
            }
            b's' => command.width = Some(number()?),
            b'v' => command.height = Some(number()?),
            b'x' => command.source_x = number()?,
            b'y' => command.source_y = number()?,
            b'w' => command.source_width = Some(number()?),
            b'h' => command.source_height = Some(number()?),
            b'c' => command.columns = Some(parse_usize(value)?),
            b'r' => command.rows = Some(parse_usize(value)?),
            b'X' => {
                command.x_offset = i32::try_from(number()?)
                    .map_err(|_| Error::Invalid("cell offset is too large"))?;
            }
            b'Y' => {
                command.y_offset = i32::try_from(number()?)
                    .map_err(|_| Error::Invalid("cell offset is too large"))?;
            }
            b'z' => command.z_index = parse_i32(value)?,
            b'C' => command.no_cursor_move = parse_bool(value)?,
            b'd' if value.len() == 1 => command.delete_selector = Some(value[0]),
            b'U' | b'P' | b'Q' | b'H' | b'V' => {
                return Err(Error::Unsupported("unsupported placement mode"));
            }
            _ => return Err(Error::Unsupported("unsupported control field")),
        }
    }
    if command.quiet > 2 {
        return Err(Error::Invalid("invalid quiet mode"));
    }
    Ok(command)
}

pub(crate) fn decode_base64_payload(
    encoded: &[u8],
    encoded_limit: usize,
    decoded_limit: usize,
) -> Result<Vec<u8>, Error> {
    if encoded.len() > encoded_limit {
        return Err(Error::TooBig("encoded payload exceeds limit"));
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|_| Error::BadMessage("invalid base64 payload"))?;
    if decoded.len() > decoded_limit {
        return Err(Error::TooBig("decoded chunk exceeds limit"));
    }
    Ok(decoded)
}

pub(crate) fn decode_image(command: &Command, mut bytes: Vec<u8>) -> Result<DecodedImage, Error> {
    if command.medium != Medium::Direct {
        return Err(Error::Unsupported("unsupported transmission medium"));
    }
    if command.compressed {
        let compressed_len = bytes.len();
        let expansion_limit = compressed_len
            .checked_mul(MAX_EXPANSION_RATIO)
            .unwrap_or(MAX_DECODED_BYTES)
            .min(MAX_DECODED_BYTES);
        let mut decoded = Vec::new();
        ZlibDecoder::new(bytes.as_slice())
            .take(u64::try_from(expansion_limit.saturating_add(1)).unwrap_or(u64::MAX))
            .read_to_end(&mut decoded)
            .map_err(|_| Error::BadMessage("invalid zlib payload"))?;
        if decoded.len() > expansion_limit {
            return Err(Error::TooBig("compressed payload expands beyond limit"));
        }
        bytes = decoded;
    }
    match command.format {
        Format::Rgb => decode_raw(command, &bytes, 3, ImageSourceFormat::KittyRgb),
        Format::Rgba => decode_raw(command, &bytes, 4, ImageSourceFormat::KittyRgba),
        Format::Png => decode_png(&bytes),
    }
}

fn decode_raw(
    command: &Command,
    bytes: &[u8],
    channels: usize,
    format: ImageSourceFormat,
) -> Result<DecodedImage, Error> {
    let width = command
        .width
        .filter(|value| *value > 0)
        .ok_or(Error::Invalid("raw width is required"))?;
    let height = command
        .height
        .filter(|value| *value > 0)
        .ok_or(Error::Invalid("raw height is required"))?;
    if width > 4096 || height > 4096 {
        return Err(Error::TooBig("raw dimensions exceed limit"));
    }
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(Error::TooBig("raw dimensions overflow"))?;
    if pixel_count > 4_194_304 {
        return Err(Error::TooBig("raw dimensions exceed limit"));
    }
    let expected = pixel_count
        .checked_mul(channels)
        .ok_or(Error::TooBig("raw byte length overflow"))?;
    if bytes.len() != expected {
        return Err(Error::BadMessage("raw byte length mismatch"));
    }
    let output_len = pixel_count
        .checked_mul(4)
        .ok_or(Error::TooBig("decoded byte length overflow"))?;
    if output_len > MAX_DECODED_BYTES {
        return Err(Error::TooBig("decoded image exceeds limit"));
    }
    let mut pixels = Vec::with_capacity(output_len);
    for pixel in bytes.chunks_exact(channels) {
        let alpha = if channels == 4 { pixel[3] } else { 255 };
        pixels.extend_from_slice(&[
            premultiply(pixel[2], alpha),
            premultiply(pixel[1], alpha),
            premultiply(pixel[0], alpha),
            alpha,
        ]);
    }
    Ok(DecodedImage {
        width,
        height,
        format,
        alpha_mode: if channels == 4 {
            ImageAlphaMode::Premultiplied
        } else {
            ImageAlphaMode::Opaque
        },
        pixels,
    })
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<DecodedImage, Error> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: MAX_DECODED_BYTES * 2,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|_| Error::BadMessage("invalid PNG"))?;
    let info = reader.info();
    let pixel_count = usize::try_from(info.width)
        .ok()
        .and_then(|width| {
            usize::try_from(info.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(Error::TooBig("PNG dimensions overflow"))?;
    if info.width == 0
        || info.height == 0
        || info.width > 4096
        || info.height > 4096
        || pixel_count > 4_194_304
    {
        return Err(Error::TooBig("PNG dimensions exceed limit"));
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or(Error::TooBig("PNG output size unavailable"))?;
    if output_size > MAX_DECODED_BYTES {
        return Err(Error::TooBig("PNG output exceeds limit"));
    }
    let mut output = vec![0; output_size];
    let info = reader
        .next_frame(&mut output)
        .map_err(|_| Error::BadMessage("invalid PNG data"))?;
    let bytes = &output[..info.buffer_size()];
    let pixel_count = usize::try_from(info.width)
        .ok()
        .and_then(|width| {
            usize::try_from(info.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(Error::TooBig("PNG dimensions overflow"))?;
    let output_len = pixel_count
        .checked_mul(4)
        .ok_or(Error::TooBig("PNG byte length overflow"))?;
    if output_len > MAX_DECODED_BYTES {
        return Err(Error::TooBig("decoded PNG exceeds limit"));
    }
    let mut pixels = Vec::with_capacity(output_len);
    match info.color_type {
        png::ColorType::Rgb => {
            for pixel in bytes.chunks_exact(3) {
                pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        png::ColorType::Rgba => {
            for pixel in bytes.chunks_exact(4) {
                let alpha = pixel[3];
                pixels.extend_from_slice(&[
                    premultiply(pixel[2], alpha),
                    premultiply(pixel[1], alpha),
                    premultiply(pixel[0], alpha),
                    alpha,
                ]);
            }
        }
        png::ColorType::Grayscale => {
            for value in bytes {
                pixels.extend_from_slice(&[*value, *value, *value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in bytes.chunks_exact(2) {
                let alpha = pixel[1];
                let value = premultiply(pixel[0], alpha);
                pixels.extend_from_slice(&[value, value, value, alpha]);
            }
        }
        png::ColorType::Indexed => return Err(Error::BadMessage("PNG palette was not expanded")),
    }
    Ok(DecodedImage {
        width: info.width,
        height: info.height,
        format: ImageSourceFormat::KittyPng,
        alpha_mode: if matches!(
            info.color_type,
            png::ColorType::Rgba | png::ColorType::GrayscaleAlpha
        ) {
            ImageAlphaMode::Premultiplied
        } else {
            ImageAlphaMode::Opaque
        },
        pixels,
    })
}

fn parse_u32(value: &[u8]) -> Result<u32, Error> {
    let value =
        std::str::from_utf8(value).map_err(|_| Error::Invalid("control value is not ASCII"))?;
    value
        .parse()
        .map_err(|_| Error::Invalid("invalid unsigned control value"))
}

fn parse_usize(value: &[u8]) -> Result<usize, Error> {
    usize::try_from(parse_u32(value)?).map_err(|_| Error::Invalid("control value is too large"))
}

fn parse_i32(value: &[u8]) -> Result<i32, Error> {
    let value =
        std::str::from_utf8(value).map_err(|_| Error::Invalid("control value is not ASCII"))?;
    value
        .parse()
        .map_err(|_| Error::Invalid("invalid signed control value"))
}

fn parse_bool(value: &[u8]) -> Result<bool, Error> {
    match value {
        b"0" => Ok(false),
        b"1" => Ok(true),
        _ => Err(Error::Invalid("expected zero or one")),
    }
}

fn premultiply(value: u8, alpha: u8) -> u8 {
    let result = (u16::from(value) * u16::from(alpha) + 127) / 255;
    u8::try_from(result).expect("premultiplied channel is bounded to u8")
}
