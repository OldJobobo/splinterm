#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, BufReader, Cursor, IoSlice, IoSliceMut, Read, Seek, SeekFrom},
    os::{
        fd::{AsFd, OwnedFd},
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;
use png::{ColorType, Decoder, Limits, Transformations};
use rustix::{
    fs::{MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, fstat, memfd_create},
    io::write,
    net::{
        RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, SendAncillaryBuffer,
        SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
    },
    rand::{GetRandomFlags, getrandom},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DECODED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DIMENSION: u32 = 4096;
pub const MAX_PIXELS: usize = 4_194_304;
pub const KITTY_ENCODED_CHUNK_BYTES: usize = 4096;
pub const KITTY_DECODED_FULL_CHUNK_BYTES: usize = 3072;
pub const CONTENT_CHUNK_BYTES: usize = 64 * 1024;
pub const CONTENT_WINDOW_CHUNKS: usize = 4;
pub const TOKEN_BYTES: usize = 32;
pub const TOKEN_TTL: Duration = Duration::from_secs(5);
pub const MAX_PENDING_TOKENS_PER_PEER: usize = 4;
pub const MAX_PENDING_TOKENS_PER_DAEMON: usize = 32;
pub const MAX_UNAUTHENTICATED_CONTENT_CONNECTIONS: usize = 8;
pub const MAX_CONTENTS_PER_SPLINT: usize = 64;
pub const MAX_AUTHORITATIVE_BYTES_PER_SPLINT: usize = 32 * 1024 * 1024;
pub const MAX_AUTHORITATIVE_BYTES_PER_DAEMON: usize = 64 * 1024 * 1024;
pub const MAX_PLACEMENTS_PER_SPLINT: usize = 256;
pub const MAX_INBOUND_KITTY_UPLOADS_PER_PTY: usize = 1;
pub const MAX_OUTBOUND_TRANSFERS_PER_SPLINT: usize = 2;
pub const MAX_OUTBOUND_TRANSFERS_PER_DAEMON: usize = 4;
pub const MAX_ENCODED_BYTES_IN_FLIGHT: usize = 16 * 1024 * 1024;
pub const MAX_CONTENT_HANDSHAKE_BYTES: usize = 512;
pub const MAX_KITTY_CONTROL_BYTES: usize = 1024;
pub const MAX_REPLY_TEXT_BYTES: usize = 512;
pub const MAX_DECODER_EXPANSION_RATIO: usize = 64;
pub const MAX_PIXEL_WRITES_PER_COMMAND: usize = 16_777_216;
pub const MAX_CLIENT_SOURCE_CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CLIENT_SCALED_CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CLIENT_TOTAL_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SIXEL_COLORS: usize = 1024;
pub const CONTENT_SOCKET_MODE: u32 = 0o600;
pub const CONTENT_CONNECTION_DEADLINE: Duration = Duration::from_secs(5);
pub const CONTENT_HANDSHAKE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SpikeError {
    #[error("input exceeds its encoded bound")]
    EncodedLimit,
    #[error("decoded output exceeds its bound")]
    DecodedLimit,
    #[error("decoded output exceeds its expansion-ratio bound")]
    ExpansionRatio,
    #[error("image dimensions are invalid or exceed their bound")]
    Dimensions,
    #[error("image data is malformed or unsupported")]
    Malformed,
    #[error("operation was cancelled")]
    Cancelled,
    #[error("chunk violates framing or receive-window rules")]
    Chunk,
    #[error("transfer token is invalid, expired, replayed, or mismatched")]
    Token,
    #[error("resource capacity is exhausted")]
    Capacity,
    #[error("admission identity is duplicate, missing, or mismatched")]
    Admission,
    #[error("content socket permissions are invalid")]
    Permissions,
    #[error("operation exceeded its deadline")]
    Deadline,
    #[error("operating-system operation failed")]
    Os,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

fn checked_pixel_bytes(width: u32, height: u32) -> Result<usize, SpikeError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(SpikeError::Dimensions);
    }
    let pixels = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(SpikeError::Dimensions)?;
    if pixels > MAX_PIXELS {
        return Err(SpikeError::Dimensions);
    }
    pixels
        .checked_mul(4)
        .filter(|bytes| *bytes <= MAX_DECODED_BYTES)
        .ok_or(SpikeError::DecodedLimit)
}

fn premultiply(component: u8, alpha: u8) -> u8 {
    u8::try_from((u16::from(component) * u16::from(alpha) + 127) / 255)
        .expect("premultiplied component fits in one byte")
}

const PNG_READ_WORK_BYTES: usize = 4096;
const PNG_CONVERSION_WORK_PIXELS: usize = 4096;

struct CancellableReader<'a, F> {
    input: Cursor<&'a [u8]>,
    cancelled: &'a mut F,
}

impl<F: FnMut() -> bool> Read for CancellableReader<'_, F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let length = buffer.len().min(PNG_READ_WORK_BYTES);
        self.input.read(&mut buffer[..length])
    }
}

impl<F> Seek for CancellableReader<'_, F> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.input.seek(position)
    }
}

fn map_png_error(error: &png::DecodingError) -> SpikeError {
    if matches!(&error, png::DecodingError::IoError(source) if source.kind() == io::ErrorKind::Interrupted)
    {
        SpikeError::Cancelled
    } else {
        SpikeError::Malformed
    }
}

/// Decodes one bounded PNG into canonical premultiplied BGRA8.
///
/// # Errors
///
/// Returns a bounded error for cancellation, malformed input, dimensions,
/// expansion, or encoded/decoded resource exhaustion.
pub fn decode_png(input: &[u8], cancelled: bool) -> Result<Pixels, SpikeError> {
    decode_png_with_cancel(input, || cancelled)
}

/// Decodes PNG while checking cancellation during bounded reads and conversion.
///
/// # Errors
///
/// Returns a bounded error for cancellation, malformed input, dimensions,
/// expansion, or encoded/decoded resource exhaustion.
pub fn decode_png_with_cancel(
    input: &[u8],
    mut cancelled: impl FnMut() -> bool,
) -> Result<Pixels, SpikeError> {
    if input.len() > MAX_ENCODED_BYTES {
        return Err(SpikeError::EncodedLimit);
    }
    let (frame, frame_bytes, output_bytes) = {
        let source = CancellableReader {
            input: Cursor::new(input),
            cancelled: &mut cancelled,
        };
        let mut decoder = Decoder::new_with_limits(
            BufReader::with_capacity(PNG_READ_WORK_BYTES, source),
            Limits {
                bytes: MAX_DECODED_BYTES,
            },
        );
        decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|error| map_png_error(&error))?;
        let info = reader.info();
        let output_bytes = checked_pixel_bytes(info.width, info.height)?;
        let ratio_limit = input
            .len()
            .checked_mul(MAX_DECODER_EXPANSION_RATIO)
            .unwrap_or(MAX_DECODED_BYTES)
            .min(MAX_DECODED_BYTES);
        if output_bytes > ratio_limit {
            return Err(SpikeError::ExpansionRatio);
        }
        let buffer_bytes = reader
            .output_buffer_size()
            .ok_or(SpikeError::DecodedLimit)?;
        if buffer_bytes > MAX_DECODED_BYTES {
            return Err(SpikeError::DecodedLimit);
        }
        let mut frame_bytes = vec![0; buffer_bytes];
        let frame = reader
            .next_frame(&mut frame_bytes)
            .map_err(|error| map_png_error(&error))?;
        (frame, frame_bytes, output_bytes)
    };

    let source = &frame_bytes[..frame.buffer_size()];
    let pixel_count = output_bytes / 4;
    let mut bgra = Vec::with_capacity(output_bytes);
    let mut check_cancel = |pixel: usize| {
        if pixel.is_multiple_of(PNG_CONVERSION_WORK_PIXELS) && cancelled() {
            Err(SpikeError::Cancelled)
        } else {
            Ok(())
        }
    };
    match frame.color_type {
        ColorType::Rgba => {
            if source.len() != pixel_count * 4 {
                return Err(SpikeError::Malformed);
            }
            for (index, pixel) in source.chunks_exact(4).enumerate() {
                check_cancel(index)?;
                let alpha = pixel[3];
                bgra.extend_from_slice(&[
                    premultiply(pixel[2], alpha),
                    premultiply(pixel[1], alpha),
                    premultiply(pixel[0], alpha),
                    alpha,
                ]);
            }
        }
        ColorType::Rgb => {
            if source.len() != pixel_count * 3 {
                return Err(SpikeError::Malformed);
            }
            for (index, pixel) in source.chunks_exact(3).enumerate() {
                check_cancel(index)?;
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        ColorType::GrayscaleAlpha => {
            if source.len() != pixel_count * 2 {
                return Err(SpikeError::Malformed);
            }
            for (index, pixel) in source.chunks_exact(2).enumerate() {
                check_cancel(index)?;
                let alpha = pixel[1];
                let value = premultiply(pixel[0], alpha);
                bgra.extend_from_slice(&[value, value, value, alpha]);
            }
        }
        ColorType::Grayscale => {
            if source.len() != pixel_count {
                return Err(SpikeError::Malformed);
            }
            for (index, value) in source.iter().enumerate() {
                check_cancel(index)?;
                bgra.extend_from_slice(&[*value, *value, *value, 255]);
            }
        }
        ColorType::Indexed => return Err(SpikeError::Malformed),
    }
    Ok(Pixels {
        width: frame.width,
        height: frame.height,
        bgra,
    })
}

/// Decodes bounded Kitty base64 chunks and optional RFC 1950 zlib output.
///
/// # Errors
///
/// Returns a bounded error for cancellation, framing, malformed data, or
/// encoded/decoded resource exhaustion.
pub fn decode_kitty_chunks(
    chunks: &[(&[u8], bool)],
    compressed: bool,
    cancelled: bool,
) -> Result<Vec<u8>, SpikeError> {
    decode_kitty_chunks_with_cancel(chunks, compressed, || cancelled)
}

/// Decodes Kitty chunks while checking cancellation between bounded work units.
///
/// # Errors
///
/// Returns a bounded error for cancellation, framing, malformed data, expansion,
/// or encoded/decoded resource exhaustion.
pub fn decode_kitty_chunks_with_cancel(
    chunks: &[(&[u8], bool)],
    compressed: bool,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<u8>, SpikeError> {
    let mut decoded = Vec::new();
    for (index, (chunk, more)) in chunks.iter().enumerate() {
        let payload_limit = if chunks.len() == 1 {
            MAX_ENCODED_BYTES
        } else {
            KITTY_ENCODED_CHUNK_BYTES
        };
        if chunk.len() > payload_limit
            || (*more && chunk.len() % 4 != 0)
            || (!*more && index + 1 != chunks.len())
            || (*more && index + 1 == chunks.len())
        {
            return Err(SpikeError::Chunk);
        }
        for encoded in chunk.chunks(KITTY_ENCODED_CHUNK_BYTES) {
            if cancelled() {
                return Err(SpikeError::Cancelled);
            }
            let mut buffer = [0_u8; KITTY_DECODED_FULL_CHUNK_BYTES];
            let length = STANDARD
                .decode_slice(encoded, &mut buffer)
                .map_err(|_| SpikeError::Malformed)?;
            let next = decoded
                .len()
                .checked_add(length)
                .filter(|size| *size <= MAX_ENCODED_BYTES)
                .ok_or(SpikeError::EncodedLimit)?;
            decoded.reserve(next - decoded.len());
            decoded.extend_from_slice(&buffer[..length]);
        }
    }
    if !compressed {
        return Ok(decoded);
    }

    let ratio_limit = decoded
        .len()
        .checked_mul(MAX_DECODER_EXPANSION_RATIO)
        .unwrap_or(MAX_DECODED_BYTES)
        .min(MAX_DECODED_BYTES);
    let mut inflater = ZlibDecoder::new(decoded.as_slice());
    let mut output = Vec::new();
    let mut buffer = vec![0_u8; CONTENT_CHUNK_BYTES].into_boxed_slice();
    loop {
        if cancelled() {
            return Err(SpikeError::Cancelled);
        }
        let length = inflater
            .read(&mut buffer)
            .map_err(|_| SpikeError::Malformed)?;
        if length == 0 {
            break;
        }
        let next = output
            .len()
            .checked_add(length)
            .ok_or(SpikeError::DecodedLimit)?;
        if next > ratio_limit {
            return Err(SpikeError::ExpansionRatio);
        }
        output.reserve(next - output.len());
        output.extend_from_slice(&buffer[..length]);
    }
    Ok(output)
}

#[derive(Debug)]
pub struct ChunkWindow {
    total: usize,
    next: usize,
    pending: BTreeMap<usize, Vec<u8>>,
    output: Vec<u8>,
    cancelled: bool,
}

impl ChunkWindow {
    /// Creates a bounded raw-content receive window.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::DecodedLimit`] when `total` exceeds the content cap.
    pub fn new(total: usize) -> Result<Self, SpikeError> {
        if total > MAX_DECODED_BYTES {
            return Err(SpikeError::DecodedLimit);
        }
        Ok(Self {
            total,
            next: 0,
            pending: BTreeMap::new(),
            output: Vec::with_capacity(total),
            cancelled: false,
        })
    }

    /// Accepts one in-window chunk and returns the next contiguous offset.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for cancellation or invalid framing/window state.
    pub fn accept(&mut self, offset: usize, bytes: &[u8]) -> Result<usize, SpikeError> {
        if self.cancelled {
            return Err(SpikeError::Cancelled);
        }
        if bytes.is_empty()
            || bytes.len() > CONTENT_CHUNK_BYTES
            || !offset.is_multiple_of(CONTENT_CHUNK_BYTES)
            || offset < self.next
            || offset
                >= self
                    .next
                    .saturating_add(CONTENT_CHUNK_BYTES * CONTENT_WINDOW_CHUNKS)
            || offset
                .checked_add(bytes.len())
                .filter(|end| *end <= self.total)
                .is_none()
            || self.pending.contains_key(&offset)
        {
            return Err(SpikeError::Chunk);
        }
        self.pending.insert(offset, bytes.to_vec());
        while let Some(bytes) = self.pending.remove(&self.next) {
            self.next += bytes.len();
            self.output.extend_from_slice(&bytes);
        }
        Ok(self.next)
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.pending.clear();
        self.output.clear();
    }

    /// Verifies completion and integrity, returning the assembled bytes.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for cancellation, gaps, or digest mismatch.
    pub fn finish(self, digest: [u8; 32]) -> Result<Vec<u8>, SpikeError> {
        if self.cancelled {
            return Err(SpikeError::Cancelled);
        }
        if self.next != self.total || !self.pending.is_empty() {
            return Err(SpikeError::Chunk);
        }
        if Sha256::digest(&self.output).as_slice() != digest {
            return Err(SpikeError::Malformed);
        }
        Ok(self.output)
    }
}

/// Bounded admission state for image identities and Kitty command work.
#[derive(Debug, Default)]
pub struct SemanticAdmission {
    contents: HashSet<u64>,
    placements: HashSet<u64>,
    inbound_upload: Option<u64>,
}

impl SemanticAdmission {
    /// Charges one authoritative content identity.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for duplicates or the per-Splint cap.
    pub fn admit_content(&mut self, content_id: u64) -> Result<(), SpikeError> {
        if self.contents.contains(&content_id) {
            return Err(SpikeError::Admission);
        }
        if self.contents.len() >= MAX_CONTENTS_PER_SPLINT {
            return Err(SpikeError::Capacity);
        }
        self.contents.insert(content_id);
        Ok(())
    }

    /// Releases exactly one retained content identity.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Admission`] for unknown or double release.
    pub fn release_content(&mut self, content_id: u64) -> Result<(), SpikeError> {
        self.contents
            .remove(&content_id)
            .then_some(())
            .ok_or(SpikeError::Admission)
    }

    /// Charges one placement identity.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for duplicates or the per-Splint cap.
    pub fn admit_placement(&mut self, placement_id: u64) -> Result<(), SpikeError> {
        if self.placements.contains(&placement_id) {
            return Err(SpikeError::Admission);
        }
        if self.placements.len() >= MAX_PLACEMENTS_PER_SPLINT {
            return Err(SpikeError::Capacity);
        }
        self.placements.insert(placement_id);
        Ok(())
    }

    /// Releases exactly one retained placement identity.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Admission`] for unknown or double release.
    pub fn release_placement(&mut self, placement_id: u64) -> Result<(), SpikeError> {
        self.placements
            .remove(&placement_id)
            .then_some(())
            .ok_or(SpikeError::Admission)
    }

    /// Begins one non-interleavable Kitty upload.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Capacity`] when an upload is already active.
    pub fn begin_inbound_upload(&mut self, upload_id: u64) -> Result<(), SpikeError> {
        if self.inbound_upload.is_some() {
            return Err(SpikeError::Capacity);
        }
        self.inbound_upload = Some(upload_id);
        Ok(())
    }

    /// Finishes only the matching active upload.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Admission`] for mismatch or double release.
    pub fn finish_inbound_upload(&mut self, upload_id: u64) -> Result<(), SpikeError> {
        if self.inbound_upload != Some(upload_id) {
            return Err(SpikeError::Admission);
        }
        self.inbound_upload = None;
        Ok(())
    }

    /// Checks bounded Kitty control, reply, and pixel-write work.
    ///
    /// # Errors
    ///
    /// Returns a resource error before any command work is admitted.
    pub fn admit_command(
        control_bytes: usize,
        reply_bytes: usize,
        pixel_writes: usize,
    ) -> Result<(), SpikeError> {
        if control_bytes > MAX_KITTY_CONTROL_BYTES || reply_bytes > MAX_REPLY_TEXT_BYTES {
            return Err(SpikeError::EncodedLimit);
        }
        if pixel_writes > MAX_PIXEL_WRITES_PER_COMMAND {
            return Err(SpikeError::DecodedLimit);
        }
        Ok(())
    }
}

/// Process-wide authoritative decoded-content admission.
#[derive(Debug, Default)]
pub struct AuthoritativeAdmission {
    contents: HashMap<([u8; 16], u64), usize>,
    bytes_by_splint: HashMap<[u8; 16], usize>,
    total_bytes: usize,
}

impl AuthoritativeAdmission {
    /// Retains one exactly charged content object.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for duplicate identity or any byte/count cap.
    pub fn admit(
        &mut self,
        splint_id: [u8; 16],
        content_id: u64,
        decoded_bytes: usize,
    ) -> Result<(), SpikeError> {
        let key = (splint_id, content_id);
        if self.contents.contains_key(&key) {
            return Err(SpikeError::Admission);
        }
        if decoded_bytes > MAX_DECODED_BYTES
            || self
                .contents
                .keys()
                .filter(|(owner, _)| *owner == splint_id)
                .count()
                >= MAX_CONTENTS_PER_SPLINT
        {
            return Err(SpikeError::Capacity);
        }
        let splint_bytes = self.bytes_by_splint.get(&splint_id).copied().unwrap_or(0);
        let next_splint = splint_bytes
            .checked_add(decoded_bytes)
            .filter(|bytes| *bytes <= MAX_AUTHORITATIVE_BYTES_PER_SPLINT)
            .ok_or(SpikeError::Capacity)?;
        let next_total = self
            .total_bytes
            .checked_add(decoded_bytes)
            .filter(|bytes| *bytes <= MAX_AUTHORITATIVE_BYTES_PER_DAEMON)
            .ok_or(SpikeError::Capacity)?;
        self.contents.insert(key, decoded_bytes);
        self.bytes_by_splint.insert(splint_id, next_splint);
        self.total_bytes = next_total;
        Ok(())
    }

    /// Releases the internally retained charge for one exact identity.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Admission`] for unknown or double release.
    pub fn release(&mut self, splint_id: [u8; 16], content_id: u64) -> Result<(), SpikeError> {
        let charge = self
            .contents
            .remove(&(splint_id, content_id))
            .ok_or(SpikeError::Admission)?;
        let splint_bytes = self
            .bytes_by_splint
            .get_mut(&splint_id)
            .ok_or(SpikeError::Admission)?;
        *splint_bytes -= charge;
        self.total_bytes -= charge;
        if *splint_bytes == 0 {
            self.bytes_by_splint.remove(&splint_id);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClientCacheKind {
    Source,
    Scaled,
}

/// Exact-record admission for client source and scaled image caches.
#[derive(Debug, Default)]
pub struct ClientCacheAdmission {
    entries: HashMap<(ClientCacheKind, u64), usize>,
    source_bytes: usize,
    scaled_bytes: usize,
}

impl ClientCacheAdmission {
    /// Retains one cache entry under source, scaled, and total byte caps.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for duplicate identity or cache exhaustion.
    pub fn admit(
        &mut self,
        kind: ClientCacheKind,
        cache_id: u64,
        bytes: usize,
    ) -> Result<(), SpikeError> {
        let key = (kind, cache_id);
        if self.entries.contains_key(&key) {
            return Err(SpikeError::Admission);
        }
        let (current, limit) = match kind {
            ClientCacheKind::Source => (self.source_bytes, MAX_CLIENT_SOURCE_CACHE_BYTES),
            ClientCacheKind::Scaled => (self.scaled_bytes, MAX_CLIENT_SCALED_CACHE_BYTES),
        };
        let next = current
            .checked_add(bytes)
            .filter(|value| *value <= limit)
            .ok_or(SpikeError::Capacity)?;
        self.source_bytes
            .checked_add(self.scaled_bytes)
            .and_then(|total| total.checked_add(bytes))
            .filter(|total| *total <= MAX_CLIENT_TOTAL_CACHE_BYTES)
            .ok_or(SpikeError::Capacity)?;
        self.entries.insert(key, bytes);
        match kind {
            ClientCacheKind::Source => self.source_bytes = next,
            ClientCacheKind::Scaled => self.scaled_bytes = next,
        }
        Ok(())
    }

    /// Releases the internally retained cache charge.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Admission`] for unknown or double release.
    pub fn release(&mut self, kind: ClientCacheKind, cache_id: u64) -> Result<(), SpikeError> {
        let bytes = self
            .entries
            .remove(&(kind, cache_id))
            .ok_or(SpikeError::Admission)?;
        match kind {
            ClientCacheKind::Source => self.source_bytes -= bytes,
            ClientCacheKind::Scaled => self.scaled_bytes -= bytes,
        }
        Ok(())
    }
}

/// Tracks defined Sixel palette indices under the accepted 1,024-color bound.
#[derive(Debug, Default)]
pub struct SixelPaletteAdmission {
    colors: HashSet<u16>,
}

impl SixelPaletteAdmission {
    /// Defines or redefines one in-range palette index.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Capacity`] for indices outside the accepted palette.
    pub fn define(&mut self, index: u16) -> Result<(), SpikeError> {
        if usize::from(index) >= MAX_SIXEL_COLORS {
            return Err(SpikeError::Capacity);
        }
        self.colors.insert(index);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct TransferCharge {
    splint_id: [u8; 16],
    encoded_bytes: usize,
}

/// Bounded admission state for the dedicated content socket.
#[derive(Debug, Default)]
pub struct ContentAdmission {
    unauthenticated: HashSet<u64>,
    transfers: HashMap<u64, TransferCharge>,
    encoded_bytes: usize,
}

impl ContentAdmission {
    /// Admits one uniquely identified pre-authentication connection.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for duplicate identity, permissions, deadline, or cap.
    pub fn open_unauthenticated(
        &mut self,
        connection_id: u64,
        socket_mode: u32,
        elapsed: Duration,
    ) -> Result<(), SpikeError> {
        if socket_mode != CONTENT_SOCKET_MODE {
            return Err(SpikeError::Permissions);
        }
        if elapsed > CONTENT_CONNECTION_DEADLINE {
            return Err(SpikeError::Deadline);
        }
        if self.unauthenticated.contains(&connection_id) {
            return Err(SpikeError::Admission);
        }
        if self.unauthenticated.len() >= MAX_UNAUTHENTICATED_CONTENT_CONNECTIONS {
            return Err(SpikeError::Capacity);
        }
        self.unauthenticated.insert(connection_id);
        Ok(())
    }

    /// Completes only the matching connection's authentication.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for unknown identity or deadline.
    pub fn finish_authentication(
        &mut self,
        connection_id: u64,
        elapsed: Duration,
    ) -> Result<(), SpikeError> {
        if !self.unauthenticated.remove(&connection_id) {
            return Err(SpikeError::Admission);
        }
        if elapsed > CONTENT_HANDSHAKE_DEADLINE {
            return Err(SpikeError::Deadline);
        }
        Ok(())
    }

    /// Admits one uniquely identified outbound transfer and retains its charge.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for duplicate identity or any transfer limit.
    pub fn start_transfer(
        &mut self,
        transfer_id: u64,
        splint_id: [u8; 16],
        handshake_bytes: usize,
        handshake_elapsed: Duration,
        encoded_bytes: usize,
    ) -> Result<(), SpikeError> {
        if self.transfers.contains_key(&transfer_id) {
            return Err(SpikeError::Admission);
        }
        if handshake_elapsed > CONTENT_HANDSHAKE_DEADLINE {
            return Err(SpikeError::Deadline);
        }
        if handshake_bytes > MAX_CONTENT_HANDSHAKE_BYTES || encoded_bytes > MAX_ENCODED_BYTES {
            return Err(SpikeError::EncodedLimit);
        }
        let per_splint = self
            .transfers
            .values()
            .filter(|charge| charge.splint_id == splint_id)
            .count();
        let next_bytes = self
            .encoded_bytes
            .checked_add(encoded_bytes)
            .filter(|bytes| *bytes <= MAX_ENCODED_BYTES_IN_FLIGHT)
            .ok_or(SpikeError::Capacity)?;
        if self.transfers.len() >= MAX_OUTBOUND_TRANSFERS_PER_DAEMON
            || per_splint >= MAX_OUTBOUND_TRANSFERS_PER_SPLINT
        {
            return Err(SpikeError::Capacity);
        }
        self.transfers.insert(
            transfer_id,
            TransferCharge {
                splint_id,
                encoded_bytes,
            },
        );
        self.encoded_bytes = next_bytes;
        Ok(())
    }

    /// Releases the internally retained transfer charge.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Admission`] for unknown or double release.
    pub fn finish_transfer(&mut self, transfer_id: u64) -> Result<(), SpikeError> {
        let charge = self
            .transfers
            .remove(&transfer_id)
            .ok_or(SpikeError::Admission)?;
        self.encoded_bytes -= charge.encoded_bytes;
        Ok(())
    }
}

/// A real owner-only Unix listener used by the transport spike.
#[derive(Debug)]
pub struct BoundContentSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl BoundContentSocket {
    /// Accepts one connection using the production five-second deadline.
    ///
    /// # Errors
    ///
    /// Returns a bounded deadline or OS error.
    pub fn accept(&self) -> Result<UnixStream, SpikeError> {
        accept_with_deadline(&self.listener, CONTENT_CONNECTION_DEADLINE)
    }
}

impl Drop for BoundContentSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Binds and verifies a mode-0600 socket inside an owner-only directory.
///
/// # Errors
///
/// Returns a bounded permissions or OS error.
pub fn bind_content_socket(path: &Path) -> Result<BoundContentSocket, SpikeError> {
    let parent = path.parent().ok_or(SpikeError::Permissions)?;
    let parent_mode = std::fs::metadata(parent)
        .map_err(|_| SpikeError::Os)?
        .permissions()
        .mode()
        & 0o777;
    if parent_mode & 0o077 != 0 {
        return Err(SpikeError::Permissions);
    }
    let listener = UnixListener::bind(path).map_err(|_| SpikeError::Os)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(CONTENT_SOCKET_MODE))
        .map_err(|_| SpikeError::Os)?;
    let mode = std::fs::metadata(path)
        .map_err(|_| SpikeError::Os)?
        .permissions()
        .mode()
        & 0o777;
    if mode != CONTENT_SOCKET_MODE {
        return Err(SpikeError::Permissions);
    }
    Ok(BoundContentSocket {
        listener,
        path: path.to_path_buf(),
    })
}

fn accept_with_deadline(
    listener: &UnixListener,
    deadline: Duration,
) -> Result<UnixStream, SpikeError> {
    listener.set_nonblocking(true).map_err(|_| SpikeError::Os)?;
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if started.elapsed() >= deadline {
                    return Err(SpikeError::Deadline);
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return Err(SpikeError::Os),
        }
    }
}

/// Reads a bounded handshake using the production five-second timeout.
///
/// # Errors
///
/// Returns a bounded framing, deadline, or OS error.
pub fn read_content_handshake(stream: &UnixStream) -> Result<Vec<u8>, SpikeError> {
    read_handshake_with_deadline(stream, CONTENT_HANDSHAKE_DEADLINE)
}

fn read_handshake_with_deadline(
    stream: &UnixStream,
    deadline: Duration,
) -> Result<Vec<u8>, SpikeError> {
    stream
        .set_read_timeout(Some(deadline))
        .map_err(|_| SpikeError::Os)?;
    let mut stream = stream;
    let mut bytes = vec![0_u8; MAX_CONTENT_HANDSHAKE_BYTES + 1];
    match stream.read(&mut bytes) {
        Ok(0) => Err(SpikeError::Malformed),
        Ok(length) if length > MAX_CONTENT_HANDSHAKE_BYTES => Err(SpikeError::EncodedLimit),
        Ok(length) => {
            bytes.truncate(length);
            Ok(bytes)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Err(SpikeError::Deadline)
        }
        Err(_) => Err(SpikeError::Os),
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TokenBinding {
    pub uid: u32,
    pub pid: u32,
    pub executable_digest: [u8; 32],
    pub splint_id: [u8; 16],
    pub incarnation: u64,
    pub generation: u64,
    pub content_id: u64,
    pub content_digest: [u8; 32],
    pub length: usize,
}

#[derive(Clone, Debug)]
struct TokenRecord {
    binding: TokenBinding,
    expires: Instant,
}

#[derive(Debug, Default)]
pub struct TokenRegistry {
    records: HashMap<[u8; TOKEN_BYTES], TokenRecord>,
}

impl TokenRegistry {
    /// Issues one CSPRNG token under peer and daemon caps.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for capacity or operating-system randomness failure.
    pub fn issue(
        &mut self,
        binding: &TokenBinding,
        now: Instant,
    ) -> Result<[u8; TOKEN_BYTES], SpikeError> {
        self.records.retain(|_, record| record.expires > now);
        if self.records.len() >= MAX_PENDING_TOKENS_PER_DAEMON
            || self
                .records
                .values()
                .filter(|record| {
                    record.binding.uid == binding.uid && record.binding.pid == binding.pid
                })
                .count()
                >= MAX_PENDING_TOKENS_PER_PEER
        {
            return Err(SpikeError::Capacity);
        }
        for _ in 0..4 {
            let mut token = [0; TOKEN_BYTES];
            let count =
                getrandom(&mut token, GetRandomFlags::empty()).map_err(|_| SpikeError::Os)?;
            if count == TOKEN_BYTES && !self.records.contains_key(&token) {
                self.records.insert(
                    token,
                    TokenRecord {
                        binding: binding.clone(),
                        expires: now + TOKEN_TTL,
                    },
                );
                return Ok(token);
            }
        }
        Err(SpikeError::Os)
    }

    /// Atomically consumes and validates one token.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Token`] for mismatch, expiry, absence, or replay.
    pub fn consume(
        &mut self,
        token: [u8; TOKEN_BYTES],
        binding: &TokenBinding,
        now: Instant,
    ) -> Result<(), SpikeError> {
        let record = self.records.remove(&token).ok_or(SpikeError::Token)?;
        if record.expires <= now || &record.binding != binding {
            return Err(SpikeError::Token);
        }
        Ok(())
    }
}

const REQUIRED_SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);

/// Creates an exactly-sized immutable close-on-exec memfd.
///
/// # Errors
///
/// Returns a bounded error for resource limits or operating-system failures.
pub fn sealed_memfd(bytes: &[u8]) -> Result<OwnedFd, SpikeError> {
    if bytes.len() > MAX_DECODED_BYTES {
        return Err(SpikeError::DecodedLimit);
    }
    let fd = memfd_create(
        "splinterm-image-spike",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .map_err(|_| SpikeError::Os)?;
    let mut written = 0;
    while written < bytes.len() {
        written += write(&fd, &bytes[written..]).map_err(|_| SpikeError::Os)?;
    }
    fcntl_add_seals(&fd, REQUIRED_SEALS).map_err(|_| SpikeError::Os)?;
    if !fcntl_get_seals(&fd)
        .map_err(|_| SpikeError::Os)?
        .contains(REQUIRED_SEALS)
        || usize::try_from(fstat(&fd).map_err(|_| SpikeError::Os)?.st_size).ok()
            != Some(bytes.len())
    {
        return Err(SpikeError::Os);
    }
    Ok(fd)
}

/// Passes exactly one FD and receives it close-on-exec.
///
/// # Errors
///
/// Returns a bounded error for ancillary truncation, count, or OS failures.
pub fn pass_one_fd(
    sender: &impl AsFd,
    receiver: &impl AsFd,
    fd: &OwnedFd,
) -> Result<OwnedFd, SpikeError> {
    let borrowed = [fd.as_fd()];
    let mut send_space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut send_space);
    if !control.push(SendAncillaryMessage::ScmRights(&borrowed)) {
        return Err(SpikeError::Os);
    }
    sendmsg(
        sender,
        &[IoSlice::new(b"I")],
        &mut control,
        SendFlags::empty(),
    )
    .map_err(|_| SpikeError::Os)?;

    let mut byte = [0_u8; 1];
    let mut iov = [IoSliceMut::new(&mut byte)];
    let mut recv_space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut recv_space);
    let message = recvmsg(receiver, &mut iov, &mut ancillary, RecvFlags::CMSG_CLOEXEC)
        .map_err(|_| SpikeError::Os)?;
    if message.bytes != 1 || byte != [b'I'] || message.flags.contains(ReturnFlags::CTRUNC) {
        return Err(SpikeError::Os);
    }
    let mut result = None;
    for item in ancillary.drain() {
        if let RecvAncillaryMessage::ScmRights(mut fds) = item {
            if result.is_some() {
                return Err(SpikeError::Os);
            }
            result = fds.next();
            if fds.next().is_some() {
                return Err(SpikeError::Os);
            }
        }
    }
    result.ok_or(SpikeError::Os)
}

#[cfg(test)]
mod tests {
    use std::{io::Write as _, net::Shutdown, os::unix::net::UnixStream};

    use super::*;

    fn png_rgba(width: u32, height: u32, bytes: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut output, width, height);
            encoder.set_color(ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(bytes).unwrap();
        }
        output
    }

    #[test]
    fn png_decode_is_bounded_and_premultiplied() {
        let encoded = png_rgba(1, 1, &[200, 100, 50, 128]);
        assert_eq!(
            decode_png(&encoded, false).unwrap(),
            Pixels {
                width: 1,
                height: 1,
                bgra: vec![25, 50, 100, 128]
            }
        );
        assert_eq!(decode_png(&encoded, true), Err(SpikeError::Cancelled));
        let mut conversion_checks = 0;
        assert_eq!(
            decode_png_with_cancel(&encoded, || {
                conversion_checks += 1;
                conversion_checks > 1
            }),
            Err(SpikeError::Cancelled)
        );

        let mut random = 1_u32;
        let mut noisy_pixels = vec![0_u8; 128 * 128 * 4];
        for byte in &mut noisy_pixels {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            *byte = random.to_le_bytes()[0];
        }
        let noisy = png_rgba(128, 128, &noisy_pixels);
        let mut read_checks = 0;
        assert_eq!(
            decode_png_with_cancel(&noisy, || {
                read_checks += 1;
                read_checks > 2
            }),
            Err(SpikeError::Cancelled)
        );

        let compressed = png_rgba(256, 256, &vec![0_u8; 256 * 256 * 4]);
        assert_eq!(
            decode_png(&compressed, false),
            Err(SpikeError::ExpansionRatio)
        );
        let invalid = png_rgba(
            MAX_DIMENSION + 1,
            1,
            &vec![0; (MAX_DIMENSION as usize + 1) * 4],
        );
        assert_eq!(decode_png(&invalid, false), Err(SpikeError::Dimensions));
    }

    #[test]
    fn kitty_chunks_enforce_encoded_limit_and_zlib_output() {
        let chunks = [(b"/wAA".as_slice(), true), (b"/wAA".as_slice(), false)];
        assert_eq!(
            decode_kitty_chunks(&chunks, false, false).unwrap(),
            [255, 0, 0, 255, 0, 0]
        );
        assert_eq!(
            decode_kitty_chunks(&[(b"A".as_slice(), true)], false, false),
            Err(SpikeError::Chunk)
        );

        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(b"bounded").unwrap();
        let payload = STANDARD.encode(encoder.finish().unwrap());
        assert_eq!(
            decode_kitty_chunks(&[(payload.as_bytes(), false)], true, false).unwrap(),
            b"bounded"
        );

        let cancellable_payload = STANDARD.encode(vec![1_u8; 6_000]);
        let mut checks = 0;
        assert_eq!(
            decode_kitty_chunks_with_cancel(
                &[(cancellable_payload.as_bytes(), false)],
                false,
                || {
                    checks += 1;
                    checks > 1
                }
            ),
            Err(SpikeError::Cancelled)
        );

        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&vec![0_u8; 4096]).unwrap();
        let payload = STANDARD.encode(encoder.finish().unwrap());
        assert_eq!(
            decode_kitty_chunks(&[(payload.as_bytes(), false)], true, false),
            Err(SpikeError::ExpansionRatio)
        );
    }

    #[test]
    fn chunk_window_handles_order_stall_cancel_and_digest() {
        let bytes = vec![7_u8; CONTENT_CHUNK_BYTES * 2 + 3];
        let mut receiver = ChunkWindow::new(bytes.len()).unwrap();
        assert_eq!(
            receiver
                .accept(
                    CONTENT_CHUNK_BYTES,
                    &bytes[CONTENT_CHUNK_BYTES..CONTENT_CHUNK_BYTES * 2]
                )
                .unwrap(),
            0
        );
        assert_eq!(
            receiver.accept(0, &bytes[..CONTENT_CHUNK_BYTES]).unwrap(),
            CONTENT_CHUNK_BYTES * 2
        );
        assert_eq!(
            receiver
                .accept(CONTENT_CHUNK_BYTES * 2, &bytes[CONTENT_CHUNK_BYTES * 2..])
                .unwrap(),
            bytes.len()
        );
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        assert_eq!(receiver.finish(digest).unwrap(), bytes);

        let mut cancelled = ChunkWindow::new(10).unwrap();
        cancelled.cancel();
        assert_eq!(cancelled.accept(0, &[1]), Err(SpikeError::Cancelled));
        let mut out_of_window = ChunkWindow::new(CONTENT_CHUNK_BYTES * 8).unwrap();
        assert_eq!(
            out_of_window.accept(CONTENT_CHUNK_BYTES * 4, &[1]),
            Err(SpikeError::Chunk)
        );
    }

    #[test]
    fn semantic_admission_enforces_identity_counts_uploads_and_work() {
        let mut admission = SemanticAdmission::default();
        for id in 0..MAX_CONTENTS_PER_SPLINT {
            admission.admit_content(u64::try_from(id).unwrap()).unwrap();
        }
        assert_eq!(admission.admit_content(0), Err(SpikeError::Admission));
        assert_eq!(admission.admit_content(1000), Err(SpikeError::Capacity));
        admission.release_content(0).unwrap();
        assert_eq!(admission.release_content(0), Err(SpikeError::Admission));
        admission.admit_content(1000).unwrap();

        for id in 0..MAX_PLACEMENTS_PER_SPLINT {
            admission
                .admit_placement(u64::try_from(id).unwrap())
                .unwrap();
        }
        assert_eq!(admission.admit_placement(0), Err(SpikeError::Admission));
        assert_eq!(admission.admit_placement(1000), Err(SpikeError::Capacity));
        admission.release_placement(0).unwrap();
        assert_eq!(admission.release_placement(0), Err(SpikeError::Admission));
        admission.admit_placement(1000).unwrap();

        admission.begin_inbound_upload(7).unwrap();
        assert_eq!(admission.begin_inbound_upload(8), Err(SpikeError::Capacity));
        assert_eq!(
            admission.finish_inbound_upload(8),
            Err(SpikeError::Admission)
        );
        admission.finish_inbound_upload(7).unwrap();
        assert_eq!(
            admission.finish_inbound_upload(7),
            Err(SpikeError::Admission)
        );

        SemanticAdmission::admit_command(
            MAX_KITTY_CONTROL_BYTES,
            MAX_REPLY_TEXT_BYTES,
            MAX_PIXEL_WRITES_PER_COMMAND,
        )
        .unwrap();
        assert_eq!(
            SemanticAdmission::admit_command(MAX_KITTY_CONTROL_BYTES + 1, 0, 0),
            Err(SpikeError::EncodedLimit)
        );
        assert_eq!(
            SemanticAdmission::admit_command(0, MAX_REPLY_TEXT_BYTES + 1, 0),
            Err(SpikeError::EncodedLimit)
        );
        assert_eq!(
            SemanticAdmission::admit_command(0, 0, MAX_PIXEL_WRITES_PER_COMMAND + 1),
            Err(SpikeError::DecodedLimit)
        );
    }

    #[test]
    fn authoritative_cache_and_palette_admission_are_exact_and_recoverable() {
        let mut authoritative = AuthoritativeAdmission::default();
        authoritative.admit([1; 16], 1, MAX_DECODED_BYTES).unwrap();
        authoritative.admit([1; 16], 2, MAX_DECODED_BYTES).unwrap();
        assert_eq!(
            authoritative.admit([1; 16], 3, 1),
            Err(SpikeError::Capacity)
        );
        authoritative.release([1; 16], 1).unwrap();
        assert_eq!(
            authoritative.release([1; 16], 1),
            Err(SpikeError::Admission)
        );
        authoritative.admit([1; 16], 3, MAX_DECODED_BYTES).unwrap();

        let mut daemon = AuthoritativeAdmission::default();
        for (owner, first_id) in [(1_u8, 10_u64), (2, 20)] {
            daemon
                .admit([owner; 16], first_id, MAX_DECODED_BYTES)
                .unwrap();
            daemon
                .admit([owner; 16], first_id + 1, MAX_DECODED_BYTES)
                .unwrap();
        }
        assert_eq!(daemon.admit([3; 16], 30, 1), Err(SpikeError::Capacity));
        daemon.release([1; 16], 10).unwrap();
        daemon.admit([3; 16], 30, MAX_DECODED_BYTES).unwrap();

        let mut count = AuthoritativeAdmission::default();
        for content_id in 0..MAX_CONTENTS_PER_SPLINT {
            count
                .admit([4; 16], u64::try_from(content_id).unwrap(), 0)
                .unwrap();
        }
        assert_eq!(count.admit([4; 16], 1000, 0), Err(SpikeError::Capacity));

        let mut cache = ClientCacheAdmission::default();
        cache
            .admit(ClientCacheKind::Source, 1, MAX_CLIENT_SOURCE_CACHE_BYTES)
            .unwrap();
        assert_eq!(
            cache.admit(ClientCacheKind::Source, 2, 1),
            Err(SpikeError::Capacity)
        );
        cache
            .admit(ClientCacheKind::Scaled, 1, MAX_CLIENT_SCALED_CACHE_BYTES)
            .unwrap();
        assert_eq!(
            cache.admit(ClientCacheKind::Scaled, 1, 0),
            Err(SpikeError::Admission)
        );
        cache.release(ClientCacheKind::Source, 1).unwrap();
        assert_eq!(
            cache.release(ClientCacheKind::Source, 1),
            Err(SpikeError::Admission)
        );
        cache
            .admit(ClientCacheKind::Source, 2, MAX_CLIENT_SOURCE_CACHE_BYTES)
            .unwrap();

        let mut palette = SixelPaletteAdmission::default();
        palette.define(1023).unwrap();
        palette.define(1023).unwrap();
        assert_eq!(palette.define(1024), Err(SpikeError::Capacity));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one stateful matrix proves transfer identity, caps, and exact recovery"
    )]
    fn content_socket_admission_enforces_identity_boundaries_and_recovers() {
        let mut admission = ContentAdmission::default();
        assert_eq!(
            admission.open_unauthenticated(1, 0o644, Duration::ZERO),
            Err(SpikeError::Permissions)
        );
        assert_eq!(
            admission.open_unauthenticated(
                1,
                CONTENT_SOCKET_MODE,
                CONTENT_CONNECTION_DEADLINE + Duration::from_millis(1),
            ),
            Err(SpikeError::Deadline)
        );
        for id in 0..MAX_UNAUTHENTICATED_CONTENT_CONNECTIONS {
            admission
                .open_unauthenticated(
                    u64::try_from(id).unwrap(),
                    CONTENT_SOCKET_MODE,
                    CONTENT_CONNECTION_DEADLINE,
                )
                .unwrap();
        }
        assert_eq!(
            admission.open_unauthenticated(0, CONTENT_SOCKET_MODE, Duration::ZERO),
            Err(SpikeError::Admission)
        );
        assert_eq!(
            admission.open_unauthenticated(100, CONTENT_SOCKET_MODE, Duration::ZERO),
            Err(SpikeError::Capacity)
        );
        assert_eq!(
            admission
                .finish_authentication(0, CONTENT_HANDSHAKE_DEADLINE + Duration::from_millis(1)),
            Err(SpikeError::Deadline)
        );
        assert_eq!(
            admission.finish_authentication(0, Duration::ZERO),
            Err(SpikeError::Admission)
        );
        admission
            .open_unauthenticated(100, CONTENT_SOCKET_MODE, Duration::ZERO)
            .unwrap();
        admission
            .finish_authentication(100, CONTENT_HANDSHAKE_DEADLINE)
            .unwrap();

        assert_eq!(
            admission.start_transfer(
                1,
                [1; 16],
                MAX_CONTENT_HANDSHAKE_BYTES + 1,
                Duration::ZERO,
                1,
            ),
            Err(SpikeError::EncodedLimit)
        );
        assert_eq!(
            admission.start_transfer(
                1,
                [1; 16],
                1,
                CONTENT_HANDSHAKE_DEADLINE + Duration::from_millis(1),
                1,
            ),
            Err(SpikeError::Deadline)
        );
        assert_eq!(
            admission.start_transfer(1, [1; 16], 1, Duration::ZERO, MAX_ENCODED_BYTES + 1,),
            Err(SpikeError::EncodedLimit)
        );
        admission
            .start_transfer(
                1,
                [1; 16],
                MAX_CONTENT_HANDSHAKE_BYTES,
                CONTENT_HANDSHAKE_DEADLINE,
                MAX_ENCODED_BYTES,
            )
            .unwrap();
        admission
            .start_transfer(2, [1; 16], 1, Duration::ZERO, MAX_ENCODED_BYTES)
            .unwrap();
        assert_eq!(
            admission.start_transfer(1, [2; 16], 1, Duration::ZERO, 0),
            Err(SpikeError::Admission)
        );
        assert_eq!(
            admission.start_transfer(3, [1; 16], 1, Duration::ZERO, 0),
            Err(SpikeError::Capacity)
        );
        assert_eq!(
            admission.start_transfer(3, [2; 16], 1, Duration::ZERO, 1),
            Err(SpikeError::Capacity)
        );
        admission.finish_transfer(1).unwrap();
        assert_eq!(admission.finish_transfer(1), Err(SpikeError::Admission));
        admission
            .start_transfer(3, [2; 16], 1, Duration::ZERO, MAX_ENCODED_BYTES)
            .unwrap();
        admission.finish_transfer(2).unwrap();
        admission.finish_transfer(3).unwrap();

        for transfer_id in 0..MAX_OUTBOUND_TRANSFERS_PER_DAEMON {
            admission
                .start_transfer(
                    u64::try_from(transfer_id).unwrap(),
                    [u8::try_from(transfer_id).unwrap(); 16],
                    1,
                    Duration::ZERO,
                    0,
                )
                .unwrap();
        }
        assert_eq!(
            admission.start_transfer(99, [9; 16], 1, Duration::ZERO, 0),
            Err(SpikeError::Capacity)
        );
    }

    #[test]
    fn content_socket_uses_real_permissions_and_timer_deadlines() {
        let mut random = [0_u8; 8];
        assert_eq!(
            getrandom(&mut random, GetRandomFlags::empty()).unwrap(),
            random.len()
        );
        let directory = std::env::temp_dir().join(format!(
            "splinterm-image-spike-{}-{}",
            std::process::id(),
            u64::from_le_bytes(random)
        ));
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("content.sock");
        {
            let socket = bind_content_socket(&path).unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                CONTENT_SOCKET_MODE
            );
            assert!(matches!(
                accept_with_deadline(&socket.listener, Duration::from_millis(5)),
                Err(SpikeError::Deadline)
            ));
            let client = UnixStream::connect(&path).unwrap();
            let server = accept_with_deadline(&socket.listener, Duration::from_millis(20)).unwrap();
            client.shutdown(Shutdown::Both).unwrap();
            server.shutdown(Shutdown::Both).unwrap();
        }
        std::fs::remove_dir(&directory).unwrap();

        let (mut sender, receiver) = UnixStream::pair().unwrap();
        assert_eq!(
            read_handshake_with_deadline(&receiver, Duration::from_millis(5)),
            Err(SpikeError::Deadline)
        );
        sender
            .write_all(&vec![1_u8; MAX_CONTENT_HANDSHAKE_BYTES + 1])
            .unwrap();
        assert_eq!(
            read_handshake_with_deadline(&receiver, Duration::from_millis(20)),
            Err(SpikeError::EncodedLimit)
        );
    }

    #[test]
    fn tokens_are_bound_expiring_single_use_and_capped() {
        let now = Instant::now();
        let binding = TokenBinding {
            uid: 1,
            pid: 2,
            executable_digest: [3; 32],
            splint_id: [4; 16],
            incarnation: 5,
            generation: 6,
            content_id: 7,
            content_digest: [8; 32],
            length: 9,
        };
        let mut registry = TokenRegistry::default();
        let token = registry.issue(&binding, now).unwrap();
        let mut mismatch = binding.clone();
        mismatch.incarnation += 1;
        assert_eq!(
            registry.consume(token, &mismatch, now),
            Err(SpikeError::Token)
        );
        assert_eq!(
            registry.consume(token, &binding, now),
            Err(SpikeError::Token)
        );
        let token = registry.issue(&binding, now).unwrap();
        assert_eq!(
            registry.consume(token, &binding, now + TOKEN_TTL),
            Err(SpikeError::Token)
        );

        for _ in 0..MAX_PENDING_TOKENS_PER_PEER {
            registry.issue(&binding, now).unwrap();
        }
        assert_eq!(registry.issue(&binding, now), Err(SpikeError::Capacity));
        registry.issue(&binding, now + TOKEN_TTL).unwrap();

        let mut daemon_registry = TokenRegistry::default();
        for peer in 0..MAX_PENDING_TOKENS_PER_DAEMON {
            let mut peer_binding = binding.clone();
            peer_binding.pid = u32::try_from(peer).unwrap() + 100;
            daemon_registry.issue(&peer_binding, now).unwrap();
        }
        let mut next_peer = binding.clone();
        next_peer.pid = 10_000;
        assert_eq!(
            daemon_registry.issue(&next_peer, now),
            Err(SpikeError::Capacity)
        );
        daemon_registry.issue(&next_peer, now + TOKEN_TTL).unwrap();
    }

    #[test]
    fn memfd_is_sealed_sized_and_passed_close_on_exec() {
        let fd = sealed_memfd(b"immutable pixels").unwrap();
        let (sender, receiver) = UnixStream::pair().unwrap();
        let passed_fd = pass_one_fd(&sender, &receiver, &fd).unwrap();
        assert!(
            fcntl_get_seals(&passed_fd)
                .unwrap()
                .contains(REQUIRED_SEALS)
        );
        assert_eq!(fstat(&passed_fd).unwrap().st_size, 16);
        assert!(write(&passed_fd, b"x").is_err());
    }
}
