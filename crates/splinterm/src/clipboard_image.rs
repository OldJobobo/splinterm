//! Worker-only, bounded PNG validation and private unnamed-file staging.
//!
//! None of these filesystem operations, including descriptor disposal, belong on
//! the Wayland dispatch thread. Workers should retain these objects and send only
//! readiness tokens or copied paths to the UI for exact-target authorization.
//! Dropping a stage closes an unnamed inode. Publication is irreversible here:
//! removing a named file by path cannot safely exclude same-UID substitution.

use std::{
    fs::File,
    io::{Cursor, Write},
    os::fd::{AsRawFd, OwnedFd},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rustix::fs::{AtFlags, CWD, Mode, OFlags, fstat, linkat, open, openat, statat};
use uuid::Uuid;

pub const PNG_MIME: &str = "image/png";
pub const MAX_PNG_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PNG_DIMENSION: u32 = 8192;
pub const MAX_PNG_PIXELS: u64 = 16 * 1024 * 1024;
pub const MAX_PNG_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
pub const PNG_DECODER_ALLOCATION_BYTES: usize = 64 * 1024 * 1024;
const MAX_DIRECTORY_BYTES: usize = 4000;
const PUBLICATION_ATTEMPTS: usize = 32;

/// Checks syntax only; no expansion, directory creation, or filesystem I/O.
///
/// # Errors
/// Rejects non-UTF-8, relative, non-normal, overlong, and control-bearing paths.
pub fn validate_directory_path(path: &Path) -> Result<()> {
    let text = path.to_str().context("image directory must be UTF-8")?;
    if !path.is_absolute()
        || text.len() > MAX_DIRECTORY_BYTES
        || text.chars().any(char::is_control)
        || text
            .split('/')
            .skip(1)
            .any(|part| matches!(part, "" | "." | ".."))
    {
        bail!(
            "image directory must be an absolute normal path of at most 4000 bytes, without controls, trailing slashes, or dot components"
        );
    }
    Ok(())
}

/// Fully decodes one static PNG before any filesystem side effects.
///
/// # Errors
/// Rejects malformed/truncated PNG, APNG, trailing data, and resource-limit excess.
pub fn validate_png(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        bail!("clipboard must contain a PNG of at most 16 MiB");
    }
    // Check the complete envelope, not merely the first decodable frame. This
    // also catches APNG controls after IDAT and data hidden after IEND.
    let mut offset = 8_usize;
    let mut ended = false;
    while offset < bytes.len() {
        let header = bytes
            .get(offset..offset + 8)
            .context("truncated PNG chunk")?;
        let length = u32::from_be_bytes(header[..4].try_into()?) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|start| start.checked_add(length))
            .filter(|end| *end <= bytes.len())
            .context("truncated PNG chunk")?;
        let kind = &header[4..8];
        if offset == 8 && (kind != b"IHDR" || length != 13) {
            bail!("PNG must begin with IHDR");
        }
        if matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            bail!("animated PNG clipboard images are not supported");
        }
        if kind == b"IEND" {
            if length != 0 || end != bytes.len() {
                bail!("PNG has invalid end marker or trailing data");
            }
            ended = true;
        }
        offset = end;
    }
    if !ended {
        bail!("PNG is missing its end marker");
    }
    let mut options = png::DecodeOptions::default();
    options.set_skip_ancillary_crc_failures(false);
    let mut decoder = png::Decoder::new_with_options(Cursor::new(bytes), options);
    decoder.set_limits(png::Limits {
        bytes: PNG_DECODER_ALLOCATION_BYTES,
    });
    // Retain metadata in the original file without decompressing text/profiles.
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|_| anyhow::anyhow!("invalid PNG header"))?;
    let info = reader.info();
    if info.width == 0
        || info.height == 0
        || info.width > MAX_PNG_DIMENSION
        || info.height > MAX_PNG_DIMENSION
        || u64::from(info.width) * u64::from(info.height) > MAX_PNG_PIXELS
    {
        bail!("PNG exceeds 8192 pixels per edge or 16 megapixels");
    }
    validate_image_data_length(bytes, info)?;
    let size = reader
        .output_buffer_size()
        .filter(|size| *size <= MAX_PNG_OUTPUT_BYTES)
        .context("PNG decoded output exceeds 64 MiB")?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .context("cannot allocate bounded PNG output")?;
    output.resize(size, 0);
    reader
        .next_frame(&mut output)
        .map_err(|_| anyhow::anyhow!("invalid PNG image data"))?;
    reader
        .finish()
        .map_err(|_| anyhow::anyhow!("invalid PNG end data"))?;
    Ok(())
}

// png 0.18 intentionally discards excess image data after the declared rows.
// Independently bound and validate the complete zlib stream, including its
// checksum, without allocating the inflated body. Dimensions were checked above.
fn validate_image_data_length(bytes: &[u8], info: &png::Info<'_>) -> Result<()> {
    let bits = info.color_type.samples() as u64 * u64::from(info.bit_depth as u8);
    let row_bytes = |width: u64, height: u64| ((width * bits).div_ceil(8) + 1) * height;
    let width = u64::from(info.width);
    let height = u64::from(info.height);
    let expected = if info.interlaced {
        // Adam7 start-column, start-row, column stride, row stride.
        [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ]
        .into_iter()
        .map(|(x, y, dx, dy)| {
            let columns = width.saturating_sub(x).div_ceil(dx);
            let rows = height.saturating_sub(y).div_ceil(dy);
            if columns == 0 || rows == 0 {
                0
            } else {
                row_bytes(columns, rows)
            }
        })
        .sum()
    } else {
        row_bytes(width, height)
    };
    // The envelope was validated already; the compressed copy cannot exceed the
    // 16 MiB offer cap. It is dropped before allocating decoded pixel output.
    let mut compressed = Vec::new();
    compressed
        .try_reserve_exact(bytes.len())
        .context("cannot allocate bounded PNG input")?;
    let mut offset = 8;
    while offset < bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into()?) as usize;
        if &bytes[offset + 4..offset + 8] == b"IDAT" {
            compressed.extend_from_slice(&bytes[offset + 8..offset + 8 + length]);
        }
        offset += length + 12;
    }
    let mut decoder = flate2::Decompress::new(true);
    let mut output = [0_u8; 8192];
    loop {
        let before = (decoder.total_in(), decoder.total_out());
        // Inflate at most one byte beyond the declared filtered frame size.
        let limit = usize::try_from((expected + 1 - before.1).min(output.len() as u64))?;
        let status = decoder
            .decompress(
                &compressed[usize::try_from(before.0)?..],
                &mut output[..limit],
                flate2::FlushDecompress::None,
            )
            .map_err(|_| anyhow::anyhow!("invalid PNG compressed image data"))?;
        if decoder.total_out() > expected {
            bail!("PNG contains excess decompressed image data");
        }
        if status == flate2::Status::StreamEnd {
            break;
        }
        if before == (decoder.total_in(), decoder.total_out()) {
            bail!("PNG compressed image stream is incomplete");
        }
    }
    if decoder.total_out() != expected || decoder.total_in() != compressed.len() as u64 {
        bail!("PNG image data length does not match its declared frame");
    }
    Ok(())
}

fn open_private_directory(path: &Path) -> Result<OwnedFd> {
    validate_directory_path(path)?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", flags, Mode::empty()).context("open image directory root")?;
    let uid = rustix::process::geteuid().as_raw();
    for component in path.components() {
        if let Component::Normal(name) = component {
            directory = openat(&directory, name, flags, Mode::empty())
                .context("image directory must already exist without symlink components")?;
            let stat = fstat(&directory)?;
            if (stat.st_uid != 0 && stat.st_uid != uid) || stat.st_mode & 0o022 != 0 {
                bail!(
                    "image directory ancestors must be owned by you or root and not writable by other users"
                );
            }
        }
    }
    let stat = fstat(&directory)?;
    if stat.st_uid != uid || stat.st_mode & 0o077 != 0 {
        bail!("image directory must be owned by you with private permissions (0700)");
    }
    Ok(directory)
}

fn same_inode(left: &OwnedFd, right: &OwnedFd) -> Result<bool> {
    let left = fstat(left)?;
    let right = fstat(right)?;
    Ok(left.st_dev == right.st_dev && left.st_ino == right.st_ino)
}

/// Unnamed validated bytes. Safe to drop on cancellation or a failed save.
/// Move this object between bounded workers; do not call its methods on the UI thread.
#[derive(Debug)]
pub struct StagedClipboardImage {
    directory_path: PathBuf,
    directory: OwnedFd,
    file: File,
}

impl StagedClipboardImage {
    /// Validates and writes original PNG bytes into an unnamed private inode.
    ///
    /// # Errors
    /// Fails closed for invalid PNG, unsafe destinations, unsupported `O_TMPFILE`,
    /// and I/O failures. No named image is left behind on failure.
    pub fn stage(directory_path: &Path, bytes: &[u8]) -> Result<Self> {
        validate_png(bytes)?;
        let directory = open_private_directory(directory_path)?;
        let fd = openat(&directory, ".", OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ).context("image directory filesystem must support Linux O_TMPFILE; choose a supported private directory")?;
        let mut file = File::from(fd);
        file.write_all(bytes)
            .context("write staged clipboard PNG")?;
        file.sync_all().context("sync staged clipboard PNG")?;
        Ok(Self {
            directory_path: directory_path.to_owned(),
            directory,
            file,
        })
    }

    /// Publishes without overwriting, after the caller revalidates captured input authority.
    ///
    /// # Errors
    /// Returns an error (still leaving no named file) on directory substitution,
    /// publication failure, or exhaustion of bounded collision retries.
    /// Once successful, subsequent input failure must retain and report the path.
    pub fn publish(self) -> Result<PublishedClipboardImage> {
        for _ in 0..PUBLICATION_ATTEMPTS {
            let name = format!("clipboard-{}.png", Uuid::new_v4());
            match self.link_name(&name) {
                Ok(true) => return Ok(self.into_published(name)),
                Ok(false) => {}
                Err(error) => return Err(error),
            }
        }
        bail!("clipboard image filename collision limit reached")
    }

    fn link_name(&self, name: &str) -> Result<bool> {
        let current = open_private_directory(&self.directory_path)?;
        if !same_inode(&self.directory, &current)? {
            bail!("image directory changed during save");
        }
        // AT_EMPTY_PATH requires a capability on some kernels. Linux documents
        // this procfs equivalent for publishing O_TMPFILE as an unprivileged user.
        let source = format!("/proc/self/fd/{}", self.file.as_raw_fd());
        match linkat(CWD, source, &self.directory, name, AtFlags::SYMLINK_FOLLOW) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::EXIST) => Ok(false),
            Err(error) => {
                Err(error).context("publish clipboard PNG (requires accessible /proc/self/fd)")
            }
        }
    }

    fn into_published(self, name: String) -> PublishedClipboardImage {
        PublishedClipboardImage {
            path: self.directory_path.join(&name),
            stage: self,
            name,
        }
    }
}

/// A completed persistent image. Drop only closes descriptors, never unlinks.
/// Retain/report `path()` if input insertion is cancelled or rejected.
#[derive(Debug)]
pub struct PublishedClipboardImage {
    path: PathBuf,
    stage: StagedClipboardImage,
    name: String,
}

impl PublishedClipboardImage {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Worker-side identity recheck before requesting path insertion.
    ///
    /// # Errors
    /// Rejects renamed/replaced directories or a replaced/symlink image name.
    /// This is not protection against a malicious process with the same UID:
    /// that process can alter the namespace again after any successful check.
    pub fn verify_path(&self) -> Result<()> {
        let current = open_private_directory(&self.stage.directory_path)?;
        if !same_inode(&self.stage.directory, &current)? {
            bail!("published image directory changed; image retained in original directory");
        }
        let named = statat(&current, &self.name, AtFlags::SYMLINK_NOFOLLOW)?;
        let file = fstat(&self.stage.file)?;
        if named.st_dev != file.st_dev || named.st_ino != file.st_ino {
            bail!("published image path changed; no file was removed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::{
            ffi::OsStringExt,
            fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink},
        },
    };

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let base =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/clipboard-image-tests");
            fs::create_dir_all(&base).unwrap();
            let path = base
                .canonicalize()
                .unwrap()
                .join(Uuid::new_v4().to_string());
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn count(&self) -> usize {
            fs::read_dir(&self.0).unwrap().count()
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn png_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[255, 0, 0, 255, 0, 0, 0, 0])
                .unwrap();
        }
        bytes
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (0_u32.wrapping_sub(crc & 1)));
            }
        }
        !crc
    }

    fn chunk(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut bytes = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(&crc32(&bytes[4..]).to_be_bytes());
        bytes
    }

    #[test]
    fn png_validation_checks_complete_data_crc_and_truncation() {
        let bytes = png_bytes();
        validate_png(&bytes).unwrap();
        for end in 0..bytes.len() {
            assert!(validate_png(&bytes[..end]).is_err(), "length {end}");
        }
        let mut invalid = bytes.clone();
        invalid[29] ^= 1;
        assert!(validate_png(&invalid).is_err());
        let mut invalid = bytes.clone();
        *invalid.last_mut().unwrap() ^= 1;
        assert!(validate_png(&invalid).is_err());
        let mut invalid = bytes.clone();
        invalid.extend_from_slice(b"trailing");
        assert!(validate_png(&invalid).is_err());
        let mut invalid = bytes;
        invalid[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(validate_png(&invalid).is_err());
        assert!(validate_png(b"GIF89a").is_err());
    }

    #[test]
    fn png_validation_rejects_corrupt_ancillary_crc_even_when_metadata_is_ignored() {
        let original = png_bytes();
        let metadata = chunk(b"tEXt", b"Comment\0clipboard screenshot");
        let mut valid = original.clone();
        valid.splice(valid.len() - 12..valid.len() - 12, metadata.clone());
        validate_png(&valid).unwrap();
        let mut corrupt = metadata;
        *corrupt.last_mut().unwrap() ^= 1;
        let mut invalid = original;
        invalid.splice(invalid.len() - 12..invalid.len() - 12, corrupt);
        assert!(validate_png(&invalid).is_err());
    }

    #[test]
    fn png_validation_rejects_animation_and_all_resource_limit_excess() {
        let bytes = png_bytes();
        for kind in [b"acTL", b"fcTL", b"fdAT"] {
            let mut animated = bytes.clone();
            animated.splice(
                animated.len() - 12..animated.len() - 12,
                chunk(kind, &[0; 8]),
            );
            assert!(validate_png(&animated).is_err());
        }
        for (width, height) in [(8193_u32, 1_u32), (1, 8193), (4097, 4097), (0, 1)] {
            let mut invalid = bytes.clone();
            invalid[16..20].copy_from_slice(&width.to_be_bytes());
            invalid[20..24].copy_from_slice(&height.to_be_bytes());
            let crc = crc32(&invalid[12..29]);
            invalid[29..33].copy_from_slice(&crc.to_be_bytes());
            assert!(validate_png(&invalid).is_err());
        }
        let mut huge = bytes;
        huge.resize(MAX_PNG_BYTES + 1, 0);
        assert!(validate_png(&huge).is_err());
    }

    #[test]
    fn png_validation_decodes_pixels_not_just_headers() {
        let bytes = png_bytes();
        let mut corrupt = bytes[..33].to_vec();
        corrupt.extend(chunk(b"IDAT", b"not zlib"));
        corrupt.extend(chunk(b"IEND", &[]));
        assert!(validate_png(&corrupt).is_err());
        let mut missing_pixels = bytes[..33].to_vec();
        missing_pixels.extend(chunk(b"IEND", &[]));
        assert!(validate_png(&missing_pixels).is_err());
        // Valid zlib/CRC but more decompressed pixels than the declared frame.
        let mut excess_pixels = bytes;
        excess_pixels[16..20].copy_from_slice(&1_u32.to_be_bytes());
        let crc = crc32(&excess_pixels[12..29]);
        excess_pixels[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(validate_png(&excess_pixels).is_err());
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&vec![0; 1024 * 1024]).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < 2048);
        let mut bomb = excess_pixels[..33].to_vec();
        bomb.extend(chunk(b"IDAT", &compressed));
        bomb.extend(chunk(b"IEND", &[]));
        assert!(validate_png(&bomb).is_err());
    }

    #[test]
    fn png_validation_accepts_supported_edge_and_sixteen_bit_input() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, MAX_PNG_DIMENSION, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Sixteen);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&vec![0; MAX_PNG_DIMENSION as usize * 2])
                .unwrap();
        }
        validate_png(&bytes).unwrap();
    }

    #[test]
    fn png_validation_checks_zlib_end_and_adam7_scanline_length() {
        // A 2x2 RGBA8 Adam7 frame has one pixel in pass 1, one in pass 6,
        // and two in pass 7: sixteen sample bytes plus three filter bytes.
        let mut header = png_bytes()[16..29].to_vec();
        header[4..8].copy_from_slice(&2_u32.to_be_bytes());
        header[12] = 1;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&[0; 19]).unwrap();
        let data = encoder.finish().unwrap();
        let build = |data: &[u8]| {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            bytes.extend(chunk(b"IHDR", &header));
            bytes.extend(chunk(b"IDAT", data));
            bytes.extend(chunk(b"IEND", &[]));
            bytes
        };
        validate_png(&build(&data)).unwrap();
        // CRCs remain correct: it is the zlib stream/checksum that is invalid.
        assert!(validate_png(&build(&data[..data.len() - 1])).is_err());
        let mut corrupt = data.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(validate_png(&build(&corrupt)).is_err());
        let mut trailing = data;
        trailing.push(0);
        assert!(validate_png(&build(&trailing)).is_err());
    }

    #[test]
    fn directory_syntax_is_strict_and_shell_safe() {
        validate_directory_path(Path::new(
            "/home/user/Images with 'quotes' and Unicode-日本語",
        ))
        .unwrap();
        for path in [
            "", "relative", "~/Images", "/a/../b", "/a/./b", "/a//b", "/a/", "/a\nb", "/a\tb",
            "/a\0b",
        ] {
            assert!(
                validate_directory_path(Path::new(path)).is_err(),
                "{path:?}"
            );
        }
        assert!(
            validate_directory_path(Path::new(&format!("/{}", "a".repeat(MAX_DIRECTORY_BYTES))))
                .is_err()
        );
        let invalid_utf8 = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert!(validate_directory_path(&invalid_utf8).is_err());
    }

    #[test]
    fn staging_is_unnamed_private_and_cancellation_leaves_nothing() {
        let root = TestDirectory::new();
        let stage = StagedClipboardImage::stage(&root.0, &png_bytes()).unwrap();
        assert_eq!(root.count(), 0);
        assert_eq!(stage.file.metadata().unwrap().mode() & 0o777, 0o600);
        assert_eq!(stage.file.metadata().unwrap().nlink(), 0);
        drop(stage);
        assert_eq!(root.count(), 0);
        assert!(StagedClipboardImage::stage(&root.0, b"not png").is_err());
        assert_eq!(root.count(), 0);
    }

    #[test]
    fn publication_retains_exact_original_png_even_if_insertion_is_cancelled() {
        let root = TestDirectory::new();
        let bytes = png_bytes();
        let saved = StagedClipboardImage::stage(&root.0, &bytes)
            .unwrap()
            .publish()
            .unwrap();
        saved.verify_path().unwrap();
        let path = saved.path().to_owned();
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("clipboard-")
        );
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        // Simulate post-publication insertion cancellation by dropping the result.
        drop(saved);
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(root.count(), 1);
    }

    #[test]
    fn publication_collision_never_overwrites_files_or_symlinks() {
        let root = TestDirectory::new();
        let stage = StagedClipboardImage::stage(&root.0, &png_bytes()).unwrap();
        fs::write(root.0.join("collision.png"), b"original").unwrap();
        symlink("collision.png", root.0.join("link.png")).unwrap();
        assert!(!stage.link_name("collision.png").unwrap());
        assert!(!stage.link_name("link.png").unwrap());
        assert_eq!(stage.file.metadata().unwrap().nlink(), 0);
        assert_eq!(fs::read(root.0.join("collision.png")).unwrap(), b"original");
        let saved = stage.publish().unwrap();
        saved.verify_path().unwrap();
        assert_eq!(root.count(), 3);
    }

    #[test]
    fn staging_rejects_missing_nonprivate_symlink_and_writable_ancestor_paths() {
        let root = TestDirectory::new();
        let bytes = png_bytes();
        assert!(StagedClipboardImage::stage(&root.0.join("missing"), &bytes).is_err());
        assert!(!root.0.join("missing").exists());
        fs::write(root.0.join("file"), b"sentinel").unwrap();
        assert!(StagedClipboardImage::stage(&root.0.join("file"), &bytes).is_err());
        let private = root.0.join("private");
        fs::DirBuilder::new().mode(0o700).create(&private).unwrap();
        symlink(&private, root.0.join("link")).unwrap();
        assert!(StagedClipboardImage::stage(&root.0.join("link"), &bytes).is_err());
        fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(StagedClipboardImage::stage(&private, &bytes).is_err());
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o770)).unwrap();
        assert!(StagedClipboardImage::stage(&private, &bytes).is_err());
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn directory_substitution_before_publication_leaves_no_named_image() {
        let root = TestDirectory::new();
        let destination = root.0.join("destination");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&destination)
            .unwrap();
        let stage = StagedClipboardImage::stage(&destination, &png_bytes()).unwrap();
        fs::rename(&destination, root.0.join("original")).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&destination)
            .unwrap();
        assert!(stage.publish().is_err());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
        assert_eq!(fs::read_dir(root.0.join("original")).unwrap().count(), 0);
    }

    #[test]
    fn publication_permission_change_fails_without_named_file() {
        let root = TestDirectory::new();
        let stage = StagedClipboardImage::stage(&root.0, &png_bytes()).unwrap();
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(stage.publish().is_err());
        assert_eq!(root.count(), 0);
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn published_path_substitution_is_detected_and_never_deleted() {
        let root = TestDirectory::new();
        let saved = StagedClipboardImage::stage(&root.0, &png_bytes())
            .unwrap()
            .publish()
            .unwrap();
        let path = saved.path().to_owned();
        fs::rename(&path, root.0.join("original.png")).unwrap();
        symlink("original.png", &path).unwrap();
        assert!(saved.verify_path().is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert!(saved.verify_path().is_err());
        drop(saved);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(root.count(), 2);
    }
}
