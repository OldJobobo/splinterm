//! Bounded streaming Sixel raster decoder.
//!
//! The state machine is derived from Foot 1.27.0 `sixel.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, especially `sixel_init`,
//! `decsixel_*`, `decgra`, `decgri_*`, and `decgci`. Placement, scrolling, and
//! terminal cursor behavior remain owned by the terminal coordinator.

use super::ImageLimits;

pub(crate) const MAX_SIXEL_COLORS: usize = 1024;
pub(crate) const MAX_SIXEL_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_SIXEL_PIXEL_WRITES: usize = 16_777_216;
pub(crate) const MAX_SIXEL_EXPANSION_RATIO: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SixelError {
    InputLimit,
    Dimensions,
    ExpansionRatio,
    PixelWrites,
    Malformed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SixelImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub opaque: bool,
    pub cursor_pixel_row: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Data,
    Raster,
    Repeat,
    Color,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SixelDecoder {
    limits: ImageLimits,
    palette_size: usize,
    maximum_width: u32,
    maximum_height: u32,
    state: State,
    params: [u32; 5],
    parameter: u32,
    parameter_index: usize,
    repeat_count: u32,
    palette: Box<[u32; MAX_SIXEL_COLORS]>,
    color_index: usize,
    color: u32,
    transparent: bool,
    pan: u32,
    pad: u32,
    column: u32,
    row: u32,
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    bottom_pixel: u8,
    bottommost_painted: Option<u32>,
    input_bytes: usize,
    pixel_writes: usize,
    failed: Option<SixelError>,
}

impl SixelDecoder {
    pub(crate) fn new(
        p1: u32,
        p2: u32,
        limits: ImageLimits,
        palette_size: usize,
        maximum_width: u32,
        maximum_height: u32,
    ) -> Self {
        let pan = match p1 {
            2 => 5,
            3 | 4 => 3,
            7..=9 => 1,
            _ => 2,
        };
        let mut palette = Box::new([0xff00_0000; MAX_SIXEL_COLORS]);
        // The first ANSI colors provide deterministic useful defaults. Protocol
        // color definitions replace these entries for oracle fixtures.
        palette[1] = 0xff00_00ff;
        palette[2] = 0xff00_ff00;
        palette[3] = 0xffff_0000;
        Self {
            limits,
            palette_size: palette_size.clamp(2, MAX_SIXEL_COLORS),
            maximum_width: maximum_width.min(limits.maximum_dimension),
            maximum_height: maximum_height.min(limits.maximum_dimension),
            state: State::Data,
            params: [0; 5],
            parameter: 0,
            parameter_index: 0,
            repeat_count: 1,
            palette,
            color_index: 0,
            color: 0xff00_0000,
            transparent: p2 == 1,
            pan,
            pad: 1,
            column: 0,
            row: 0,
            width: 0,
            height: 0,
            pixels: Vec::new(),
            bottom_pixel: 0,
            bottommost_painted: None,
            input_bytes: 0,
            pixel_writes: 0,
            failed: None,
        }
    }

    pub(crate) fn put(&mut self, byte: u8) -> Result<(), SixelError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        self.input_bytes = self
            .input_bytes
            .checked_add(1)
            .filter(|bytes| *bytes <= MAX_SIXEL_INPUT_BYTES)
            .ok_or(SixelError::InputLimit)?;
        let result = match self.state {
            State::Data => self.data(byte),
            State::Raster => self.parameter_byte(byte, State::Raster),
            State::Repeat => self.parameter_byte(byte, State::Repeat),
            State::Color => self.parameter_byte(byte, State::Color),
        };
        if let Err(error) = result {
            self.failed = Some(error);
        }
        result
    }

    pub(crate) fn finish(mut self) -> Result<SixelImage, SixelError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if self.width == 0 || self.height == 0 {
            return Err(SixelError::Malformed);
        }
        let height = if self.transparent {
            self.bottommost_painted
                .and_then(|row| row.checked_add(1))
                .ok_or(SixelError::Malformed)?
        } else {
            self.height
        };
        let cursor_pixel_row = if self.column == 0 {
            self.row
        } else {
            let used_rows = (u8::BITS - self.bottom_pixel.leading_zeros())
                .min(6)
                .checked_mul(self.pan)
                .ok_or(SixelError::Dimensions)?;
            self.row
                .checked_add(used_rows)
                .ok_or(SixelError::Dimensions)?
        };
        self.pixels.truncate(
            usize::try_from(self.width)
                .ok()
                .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
                .ok_or(SixelError::Dimensions)?,
        );
        let output_bytes = self
            .pixels
            .len()
            .checked_mul(4)
            .ok_or(SixelError::Dimensions)?;
        let expansion_limit = self.input_bytes.saturating_mul(MAX_SIXEL_EXPANSION_RATIO);
        if output_bytes > expansion_limit {
            return Err(SixelError::ExpansionRatio);
        }
        let mut pixels = Vec::with_capacity(output_bytes);
        for pixel in self.pixels {
            pixels.extend_from_slice(&pixel.to_le_bytes());
        }
        Ok(SixelImage {
            width: self.width,
            height,
            pixels,
            opaque: !self.transparent,
            cursor_pixel_row,
        })
    }

    fn data(&mut self, byte: u8) -> Result<(), SixelError> {
        match byte {
            b'"' => self.start_parameters(State::Raster),
            b'!' => {
                self.start_parameters(State::Repeat);
                self.repeat_count = 1;
            }
            b'#' => {
                self.start_parameters(State::Color);
                self.color_index = 0;
            }
            b'$' => self.column = 0,
            b'-' => {
                self.row = self
                    .row
                    .checked_add(6_u32.checked_mul(self.pan).ok_or(SixelError::Dimensions)?)
                    .ok_or(SixelError::Dimensions)?;
                self.column = 0;
                self.bottom_pixel = 0;
                let height = self
                    .row
                    .checked_add(6_u32.checked_mul(self.pan).ok_or(SixelError::Dimensions)?)
                    .ok_or(SixelError::Dimensions)?;
                self.ensure(self.width, height)?;
            }
            b'?'..=b'~' => self.add_sixels(byte - b'?', 1)?,
            b' ' | b'\n' | b'\r' => {}
            _ => return Err(SixelError::Malformed),
        }
        Ok(())
    }

    fn start_parameters(&mut self, state: State) {
        self.state = state;
        self.params = [0; 5];
        self.parameter = 0;
        self.parameter_index = 0;
    }

    fn parameter_byte(&mut self, byte: u8, state: State) -> Result<(), SixelError> {
        match byte {
            b'0'..=b'9' => {
                self.parameter = self
                    .parameter
                    .saturating_mul(10)
                    .saturating_add(u32::from(byte - b'0'));
                if state == State::Repeat {
                    self.repeat_count = self.parameter;
                }
                Ok(())
            }
            b';' => {
                self.store_parameter();
                Ok(())
            }
            _ => {
                self.store_parameter();
                match state {
                    State::Raster => self.finish_raster()?,
                    State::Repeat => {
                        let count = self.repeat_count.max(1);
                        self.state = State::Data;
                        if !(b'?'..=b'~').contains(&byte) {
                            return self.data(byte);
                        }
                        self.add_sixels(byte - b'?', count)?;
                        return Ok(());
                    }
                    State::Color => self.finish_color(),
                    State::Data => unreachable!(),
                }
                self.state = State::Data;
                self.data(byte)
            }
        }
    }

    fn store_parameter(&mut self) {
        if self.parameter_index < self.params.len() {
            self.params[self.parameter_index] = self.parameter;
            self.parameter_index += 1;
        }
        self.parameter = 0;
    }

    fn finish_raster(&mut self) -> Result<(), SixelError> {
        let pan = self.params[0].clamp(1, 5);
        let pad = self.params[1].clamp(1, 5);
        if self.width == 0 && self.height == 0 {
            self.pan = pan;
            self.pad = pad;
        }
        let width = self.params[2]
            .checked_mul(self.pad)
            .ok_or(SixelError::Dimensions)?;
        let height = self.params[3]
            .checked_mul(self.pan)
            .ok_or(SixelError::Dimensions)?;
        if width >= self.width && height >= self.height {
            self.ensure(width, height)?;
        }
        Ok(())
    }

    fn finish_color(&mut self) {
        if self.parameter_index > 0 {
            self.color_index = usize::try_from(self.params[0])
                .unwrap_or(usize::MAX)
                .min(self.palette_size - 1);
        }
        if self.parameter_index > 4 {
            let c1 = self.params[2];
            let c2 = self.params[3];
            let c3 = self.params[4];
            let color = match self.params[1] {
                1 => {
                    let hue = (c1.min(360) + 240) % 360;
                    hsl_to_bgra(hue, c3.min(100), c2.min(100))
                }
                2 => {
                    let red = 255 * c1.min(100) / 100;
                    let green = 255 * c2.min(100) / 100;
                    let blue = 255 * c3.min(100) / 100;
                    0xff00_0000 | (red << 16) | (green << 8) | blue
                }
                _ => self.palette[self.color_index],
            };
            self.palette[self.color_index] = color;
        } else {
            self.color = self.palette[self.color_index];
        }
    }

    fn add_sixels(&mut self, sixel: u8, count: u32) -> Result<(), SixelError> {
        let columns = count.checked_mul(self.pad).ok_or(SixelError::Dimensions)?;
        let end_column = self
            .column
            .checked_add(columns)
            .ok_or(SixelError::Dimensions)?;
        let end_row = self
            .row
            .checked_add(6_u32.checked_mul(self.pan).ok_or(SixelError::Dimensions)?)
            .ok_or(SixelError::Dimensions)?;
        self.ensure(end_column, end_row)?;
        let writes = usize::try_from(columns)
            .ok()
            .and_then(|columns| {
                usize::try_from(6_u32.checked_mul(self.pan)?)
                    .ok()?
                    .checked_mul(columns)
            })
            .ok_or(SixelError::PixelWrites)?;
        self.pixel_writes = self
            .pixel_writes
            .checked_add(writes)
            .filter(|writes| *writes <= MAX_SIXEL_PIXEL_WRITES)
            .ok_or(SixelError::PixelWrites)?;

        let width = usize::try_from(self.width).map_err(|_| SixelError::Dimensions)?;
        for x in self.column..end_column {
            for bit in 0..6 {
                if sixel & (1 << bit) == 0 {
                    continue;
                }
                for vertical in 0..self.pan {
                    let y = self.row + bit * self.pan + vertical;
                    let index = usize::try_from(y)
                        .ok()
                        .and_then(|y| y.checked_mul(width))
                        .and_then(|base| usize::try_from(x).ok()?.checked_add(base))
                        .ok_or(SixelError::Dimensions)?;
                    self.pixels[index] = self.color;
                    self.bottommost_painted =
                        Some(self.bottommost_painted.map_or(y, |old| old.max(y)));
                }
            }
        }
        self.column = end_column;
        self.bottom_pixel |= sixel;
        Ok(())
    }

    fn ensure(&mut self, requested_width: u32, requested_height: u32) -> Result<(), SixelError> {
        let width = self.width.max(requested_width);
        let height = self.height.max(requested_height);
        if width == 0 || height == 0 {
            return Ok(());
        }
        if width > self.maximum_width || height > self.maximum_height {
            return Err(SixelError::Dimensions);
        }
        let pixel_count = usize::try_from(width)
            .ok()
            .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
            .filter(|pixels| *pixels <= self.limits.maximum_pixels)
            .ok_or(SixelError::Dimensions)?;
        pixel_count
            .checked_mul(4)
            .filter(|bytes| *bytes <= self.limits.bytes_per_content)
            .ok_or(SixelError::Dimensions)?;
        if width == self.width && height == self.height {
            return Ok(());
        }
        let background = if self.transparent { 0 } else { self.palette[0] };
        let mut resized = vec![background; pixel_count];
        let old_width = usize::try_from(self.width).map_err(|_| SixelError::Dimensions)?;
        let new_width = usize::try_from(width).map_err(|_| SixelError::Dimensions)?;
        for row in 0..usize::try_from(self.height).map_err(|_| SixelError::Dimensions)? {
            let old_start = row.checked_mul(old_width).ok_or(SixelError::Dimensions)?;
            let new_start = row.checked_mul(new_width).ok_or(SixelError::Dimensions)?;
            resized[new_start..new_start + old_width]
                .copy_from_slice(&self.pixels[old_start..old_start + old_width]);
        }
        self.width = width;
        self.height = height;
        self.pixels = resized;
        Ok(())
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "bounded HSL channels are rounded exactly like pinned Foot before conversion"
)]
fn hsl_to_bgra(hue: u32, saturation: u32, luminance: u32) -> u32 {
    let luminance = f64::from(luminance) / 100.0;
    let saturation = f64::from(saturation) / 100.0;
    let chroma = (1.0 - (2.0 * luminance - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((f64::from(hue) / 60.0) % 2.0 - 1.0).abs());
    let middle = luminance - chroma / 2.0;
    let (red, green, blue) = match hue {
        0..=60 => (chroma, x, 0.0),
        61..=120 => (x, chroma, 0.0),
        121..=180 => (0.0, chroma, x),
        181..=240 => (0.0, x, chroma),
        241..=300 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let channel =
        |value: f64| u32::try_from(((value + middle) * 255.0).round() as i64).unwrap_or(0);
    0xff00_0000 | (channel(red) << 16) | (channel(green) << 8) | channel(blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(input: &[u8], p1: u32, p2: u32) -> SixelImage {
        let limits = ImageLimits::default();
        let mut decoder = SixelDecoder::new(
            p1,
            p2,
            limits,
            MAX_SIXEL_COLORS,
            limits.maximum_dimension,
            limits.maximum_dimension,
        );
        for byte in input {
            decoder.put(*byte).unwrap();
        }
        decoder.finish().unwrap()
    }

    fn solid(width: usize, height: usize, pixel: [u8; 4]) -> Vec<u8> {
        pixel.repeat(width * height)
    }

    #[test]
    fn matches_pinned_foot_semantic_fixtures() {
        let opaque = decode(b"\"1;1;1;6#1;2;100;0;0#1~", 7, 0);
        assert_eq!((opaque.width, opaque.height, opaque.opaque), (1, 6, true));
        assert_eq!(opaque.pixels, solid(1, 6, [0, 0, 255, 255]));

        let transparent = decode(b"\"1;1;1;1#2;2;0;100;0#2@", 7, 1);
        assert_eq!(
            (transparent.width, transparent.height, transparent.opaque),
            (1, 1, false)
        );
        assert_eq!(transparent.pixels, solid(1, 1, [0, 255, 0, 255]));

        let repeated = decode(b"\"1;1;3;6#1;2;100;0;0#1!3~", 7, 0);
        assert_eq!((repeated.width, repeated.height), (3, 6));
        assert_eq!(repeated.pixels, solid(3, 6, [0, 0, 255, 255]));

        let overlap = decode(
            b"\"1;1;2;12#1;2;100;0;0#1~~$#2;2;0;100;0#2@@-#3;2;0;0;100#3~~",
            7,
            0,
        );
        let mut expected = solid(2, 6, [0, 0, 255, 255]);
        expected[..8].copy_from_slice(&solid(2, 1, [0, 255, 0, 255]));
        expected.extend_from_slice(&solid(2, 6, [255, 0, 0, 255]));
        assert_eq!((overlap.width, overlap.height), (2, 12));
        assert_eq!(overlap.pixels, expected);

        let hls = decode(b"\"1;1;1;6#1;1;120;50;100#1~", 7, 0);
        assert_eq!(hls.pixels, solid(1, 6, [0, 0, 255, 255]));
    }

    #[test]
    fn dimensions_and_work_are_bounded_before_growth() {
        let limits = ImageLimits {
            maximum_dimension: 2,
            ..ImageLimits::default()
        };
        let mut dimensions = SixelDecoder::new(7, 0, limits, MAX_SIXEL_COLORS, 2, 2);
        for byte in b"!3~" {
            let result = dimensions.put(*byte);
            if *byte == b'~' {
                assert_eq!(result, Err(SixelError::Dimensions));
            }
        }

        let limits = ImageLimits::default();
        let mut writes = SixelDecoder::new(
            7,
            0,
            limits,
            MAX_SIXEL_COLORS,
            limits.maximum_dimension,
            limits.maximum_dimension,
        );
        writes.pixel_writes = MAX_SIXEL_PIXEL_WRITES;
        assert_eq!(writes.put(b'~'), Err(SixelError::PixelWrites));
        assert_eq!(writes.finish(), Err(SixelError::PixelWrites));

        let mut expansion = SixelDecoder::new(
            7,
            0,
            limits,
            MAX_SIXEL_COLORS,
            limits.maximum_dimension,
            limits.maximum_dimension,
        );
        for byte in b"!100~" {
            expansion.put(*byte).unwrap();
        }
        assert_eq!(expansion.finish(), Err(SixelError::ExpansionRatio));
    }
}
