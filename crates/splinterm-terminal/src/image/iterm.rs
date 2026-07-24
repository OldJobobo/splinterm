//! Bounded iTerm2 OSC 1337 inline-image metadata and PNG decoding.

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::kitty::{self, DecodedImage, Error};
use crate::ImageSourceFormat;

pub(crate) const MAX_METADATA_BYTES: usize = 1024;
pub(crate) const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Extent {
    #[default]
    Auto,
    Cells(usize),
    Pixels(u32),
    Percent(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Command {
    pub width: Extent,
    pub height: Extent,
    pub preserve_aspect_ratio: bool,
    pub do_not_move_cursor: bool,
    pub declared_size: Option<usize>,
}

pub(crate) fn parse_metadata(metadata: &[u8], truncated: bool) -> Result<Command, Error> {
    if truncated || metadata.len() > MAX_METADATA_BYTES {
        return Err(Error::TooBig("iTerm2 metadata exceeds limit"));
    }
    let mut command = Command {
        width: Extent::Auto,
        height: Extent::Auto,
        preserve_aspect_ratio: true,
        do_not_move_cursor: false,
        declared_size: None,
    };
    let mut inline = false;
    let mut seen = Vec::<&[u8]>::new();
    for field in metadata.split(|byte| *byte == b';') {
        if field.is_empty() {
            continue;
        }
        let Some(separator) = field.iter().position(|byte| *byte == b'=') else {
            return Err(Error::Invalid("malformed iTerm2 metadata"));
        };
        let (key, value_with_separator) = field.split_at(separator);
        let value = &value_with_separator[1..];
        if key.is_empty() || value.is_empty() || seen.contains(&key) {
            return Err(Error::Invalid("malformed iTerm2 metadata"));
        }
        seen.push(key);
        match key {
            b"inline" => inline = parse_bool(value)?,
            b"width" => command.width = parse_extent(value)?,
            b"height" => command.height = parse_extent(value)?,
            b"preserveAspectRatio" => command.preserve_aspect_ratio = parse_bool(value)?,
            b"doNotMoveCursor" => command.do_not_move_cursor = parse_bool(value)?,
            b"size" => command.declared_size = Some(parse_usize(value)?),
            // The optional name is base64 metadata only. Validate its grammar,
            // then discard it without interpreting it as a path or authority.
            b"name" => {
                STANDARD
                    .decode(value)
                    .map_err(|_| Error::BadMessage("invalid iTerm2 name"))?;
            }
            _ => return Err(Error::Unsupported("unsupported iTerm2 metadata")),
        }
    }
    if !inline {
        return Err(Error::Unsupported("iTerm2 download transfer is disabled"));
    }
    Ok(command)
}

pub(crate) fn decode_png_payload(
    encoded: &[u8],
    declared_size: Option<usize>,
) -> Result<DecodedImage, Error> {
    let decoded =
        kitty::decode_base64_payload(encoded, MAX_ENCODED_BYTES, kitty::MAX_DECODED_BYTES)?;
    if declared_size.is_some_and(|size| size != decoded.len()) {
        return Err(Error::BadMessage("iTerm2 declared size mismatch"));
    }
    let mut image = kitty::decode_png(&decoded)?;
    image.format = ImageSourceFormat::Iterm2;
    Ok(image)
}

fn parse_extent(value: &[u8]) -> Result<Extent, Error> {
    if value == b"auto" {
        return Ok(Extent::Auto);
    }
    if let Some(pixels) = value.strip_suffix(b"px") {
        let pixels = parse_u32(pixels)?;
        return (pixels > 0)
            .then_some(Extent::Pixels(pixels))
            .ok_or(Error::Invalid("iTerm2 extent must be non-zero"));
    }
    if let Some(percent) = value.strip_suffix(b"%") {
        let percent = parse_u32(percent)?;
        return (percent > 0)
            .then_some(Extent::Percent(percent))
            .ok_or(Error::Invalid("iTerm2 extent must be non-zero"));
    }
    let cells = parse_usize(value)?;
    (cells > 0)
        .then_some(Extent::Cells(cells))
        .ok_or(Error::Invalid("iTerm2 extent must be non-zero"))
}

fn parse_bool(value: &[u8]) -> Result<bool, Error> {
    match value {
        b"0" => Ok(false),
        b"1" => Ok(true),
        _ => Err(Error::Invalid("invalid iTerm2 boolean")),
    }
}

fn parse_u32(value: &[u8]) -> Result<u32, Error> {
    let text = std::str::from_utf8(value).map_err(|_| Error::Invalid("non-ASCII iTerm2 number"))?;
    text.parse::<u32>()
        .map_err(|_| Error::Invalid("invalid iTerm2 number"))
}

fn parse_usize(value: &[u8]) -> Result<usize, Error> {
    let value = parse_u32(value)?;
    usize::try_from(value).map_err(|_| Error::TooBig("iTerm2 number exceeds platform limit"))
}
