#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, HashMap},
    io::{BufReader, Cursor, IoSlice, IoSliceMut, Read},
    os::fd::{AsFd, OwnedFd},
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

/// Decodes one bounded PNG into canonical premultiplied BGRA8.
///
/// # Errors
///
/// Returns a bounded error for cancellation, malformed input, dimensions, or
/// encoded/decoded resource exhaustion.
pub fn decode_png(input: &[u8], cancelled: bool) -> Result<Pixels, SpikeError> {
    if cancelled {
        return Err(SpikeError::Cancelled);
    }
    if input.len() > MAX_ENCODED_BYTES {
        return Err(SpikeError::EncodedLimit);
    }
    let mut decoder = Decoder::new_with_limits(
        BufReader::new(Cursor::new(input)),
        Limits {
            bytes: MAX_DECODED_BYTES,
        },
    );
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| SpikeError::Malformed)?;
    let info = reader.info();
    let output_bytes = checked_pixel_bytes(info.width, info.height)?;
    let buffer_bytes = reader
        .output_buffer_size()
        .ok_or(SpikeError::DecodedLimit)?;
    if buffer_bytes > MAX_DECODED_BYTES {
        return Err(SpikeError::DecodedLimit);
    }
    let mut frame_bytes = vec![0; buffer_bytes];
    let frame = reader
        .next_frame(&mut frame_bytes)
        .map_err(|_| SpikeError::Malformed)?;
    let source = &frame_bytes[..frame.buffer_size()];
    let pixel_count = output_bytes / 4;
    let mut bgra = Vec::with_capacity(output_bytes);
    match frame.color_type {
        ColorType::Rgba => {
            if source.len() != pixel_count * 4 {
                return Err(SpikeError::Malformed);
            }
            for pixel in source.chunks_exact(4) {
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
            for pixel in source.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        ColorType::GrayscaleAlpha => {
            if source.len() != pixel_count * 2 {
                return Err(SpikeError::Malformed);
            }
            for pixel in source.chunks_exact(2) {
                let alpha = pixel[1];
                let value = premultiply(pixel[0], alpha);
                bgra.extend_from_slice(&[value, value, value, alpha]);
            }
        }
        ColorType::Grayscale => {
            if source.len() != pixel_count {
                return Err(SpikeError::Malformed);
            }
            for value in source {
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

/// Bounded admission state for the dedicated content socket.
#[derive(Debug, Default)]
pub struct ContentAdmission {
    unauthenticated: usize,
    transfers: usize,
    encoded_bytes: usize,
    transfers_by_splint: HashMap<[u8; 16], usize>,
}

impl ContentAdmission {
    /// Admits one pre-authentication connection.
    ///
    /// # Errors
    ///
    /// Returns [`SpikeError::Capacity`] at the daemon cap.
    pub fn open_unauthenticated(&mut self) -> Result<(), SpikeError> {
        if self.unauthenticated >= MAX_UNAUTHENTICATED_CONTENT_CONNECTIONS {
            return Err(SpikeError::Capacity);
        }
        self.unauthenticated += 1;
        Ok(())
    }

    pub fn finish_authentication(&mut self) {
        self.unauthenticated = self.unauthenticated.saturating_sub(1);
    }

    /// Admits one outbound transfer and charges its encoded bytes.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the handshake, transfer, or byte cap fails.
    pub fn start_transfer(
        &mut self,
        splint_id: [u8; 16],
        handshake_bytes: usize,
        encoded_bytes: usize,
    ) -> Result<(), SpikeError> {
        if handshake_bytes > MAX_CONTENT_HANDSHAKE_BYTES {
            return Err(SpikeError::EncodedLimit);
        }
        let per_splint = self
            .transfers_by_splint
            .get(&splint_id)
            .copied()
            .unwrap_or(0);
        let next_bytes = self
            .encoded_bytes
            .checked_add(encoded_bytes)
            .filter(|bytes| *bytes <= MAX_ENCODED_BYTES_IN_FLIGHT)
            .ok_or(SpikeError::Capacity)?;
        if self.transfers >= MAX_OUTBOUND_TRANSFERS_PER_DAEMON
            || per_splint >= MAX_OUTBOUND_TRANSFERS_PER_SPLINT
        {
            return Err(SpikeError::Capacity);
        }
        self.transfers += 1;
        self.encoded_bytes = next_bytes;
        self.transfers_by_splint.insert(splint_id, per_splint + 1);
        Ok(())
    }

    pub fn finish_transfer(&mut self, splint_id: [u8; 16], encoded_bytes: usize) {
        let Some(per_splint) = self.transfers_by_splint.get_mut(&splint_id) else {
            return;
        };
        if *per_splint == 0 {
            return;
        }
        *per_splint -= 1;
        self.transfers -= 1;
        self.encoded_bytes = self.encoded_bytes.saturating_sub(encoded_bytes);
        if *per_splint == 0 {
            self.transfers_by_splint.remove(&splint_id);
        }
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
    use std::{io::Write as _, os::unix::net::UnixStream};

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
    fn content_socket_admission_enforces_every_boundary_and_recovers() {
        let mut admission = ContentAdmission::default();
        for _ in 0..MAX_UNAUTHENTICATED_CONTENT_CONNECTIONS {
            admission.open_unauthenticated().unwrap();
        }
        assert_eq!(admission.open_unauthenticated(), Err(SpikeError::Capacity));
        admission.finish_authentication();
        admission.open_unauthenticated().unwrap();

        assert_eq!(
            admission.start_transfer([1; 16], MAX_CONTENT_HANDSHAKE_BYTES + 1, 1),
            Err(SpikeError::EncodedLimit)
        );
        admission
            .start_transfer([1; 16], MAX_CONTENT_HANDSHAKE_BYTES, MAX_ENCODED_BYTES)
            .unwrap();
        admission
            .start_transfer([1; 16], 1, MAX_ENCODED_BYTES)
            .unwrap();
        assert_eq!(
            admission.start_transfer([1; 16], 1, 0),
            Err(SpikeError::Capacity)
        );
        admission.finish_transfer([1; 16], MAX_ENCODED_BYTES);
        admission.finish_transfer([1; 16], MAX_ENCODED_BYTES);

        for splint in 1..=MAX_OUTBOUND_TRANSFERS_PER_DAEMON {
            admission
                .start_transfer([u8::try_from(splint).unwrap(); 16], 1, 0)
                .unwrap();
        }
        assert_eq!(
            admission.start_transfer([9; 16], 1, 0),
            Err(SpikeError::Capacity)
        );
        admission.finish_transfer([1; 16], 0);
        admission.start_transfer([9; 16], 1, 0).unwrap();
        for splint in [2_u8, 3, 4, 9] {
            admission.finish_transfer([splint; 16], 0);
        }

        admission
            .start_transfer([1; 16], 1, MAX_ENCODED_BYTES_IN_FLIGHT)
            .unwrap();
        assert_eq!(
            admission.start_transfer([2; 16], 1, 1),
            Err(SpikeError::Capacity)
        );
        admission.finish_transfer([1; 16], MAX_ENCODED_BYTES_IN_FLIGHT);
        admission.start_transfer([2; 16], 1, 1).unwrap();
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
