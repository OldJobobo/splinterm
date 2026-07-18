//! Deterministic CPU text-row rasterization for the native client.
//!
//! Font selection, cell placement, fallback, and CPU blending are compared against
//! Foot 1.27.0 `fonts.c` and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`. This module renders only the
//! fixed evidence corpus; terminal snapshot rendering is intentionally not attached.

use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use crate::box_drawing;
use anyhow::{Context, Result, bail};

use swash::{
    FontRef,
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    shape::ShapeContext,
    zeno::Format,
};

const BASE_FONT_SIZE: f32 = 22.0;
const BASE_ROW_X: i32 = 32;
const BASE_ROW_Y: i32 = 96;
const PRIMARY_FONT: &str = "JetBrains Mono Nerd Font:style=Regular";
const PRIMARY_BOLD_FONT: &str = "JetBrains Mono Nerd Font:style=Bold";
const PRIMARY_ITALIC_FONT: &str = "JetBrains Mono Nerd Font:style=Italic";
const PRIMARY_BOLD_ITALIC_FONT: &str = "JetBrains Mono Nerd Font:style=Bold Italic";
const CJK_FONT: &str = "Noto Sans CJK JP:style=Regular";
const EMOJI_FONT: &str = "Noto Color Emoji";

const CORPUS: &[(CorpusKind, &str)] = &[
    (CorpusKind::Ascii, "ASCII"),
    (CorpusKind::BoxDrawing, "┌─┼─┐"),
    (CorpusKind::NerdFont, "\u{f120}"),
    (CorpusKind::Combining, "e\u{0301}"),
    (CorpusKind::Cjk, "界"),
    (CorpusKind::Emoji, "🙂"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CorpusKind {
    Ascii,
    BoxDrawing,
    NerdFont,
    Combining,
    Cjk,
    Emoji,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphKey {
    face: usize,
    glyph: u16,
}

const BOX_DRAWING_FACE: usize = usize::MAX;

struct FontFace {
    label: &'static str,
    family: String,
    style: String,
    path: PathBuf,
    index: usize,
    data: Vec<u8>,
}

struct CachedGlyph {
    content: Content,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InkBounds {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl CachedGlyph {
    fn ink_bounds(&self) -> Option<InkBounds> {
        let covered = |index: usize| match self.content {
            Content::Mask => self.data[index] != 0,
            Content::SubpixelMask => self.data[index * 4..index * 4 + 3]
                .iter()
                .any(|channel| *channel != 0),
            Content::Color => self.data[index * 4 + 3] != 0,
        };
        let mut bounds: Option<InkBounds> = None;
        for y in 0..self.height {
            for x in 0..self.width {
                let index = usize::try_from(y * self.width + x).expect("glyph index fits usize");
                if !covered(index) {
                    continue;
                }
                bounds = Some(match bounds {
                    None => InkBounds {
                        left: x,
                        top: y,
                        right: x + 1,
                        bottom: y + 1,
                    },
                    Some(current) => InkBounds {
                        left: current.left.min(x),
                        top: current.top.min(y),
                        right: current.right.max(x + 1),
                        bottom: current.bottom.max(y + 1),
                    },
                });
            }
        }
        bounds
    }
}

#[derive(Clone, Copy)]
struct PlacedGlyph {
    key: GlyphKey,
    cell: u32,
    cells: u32,
    cluster_advance: f32,
    x_offset: f32,
    y_offset: f32,
}

pub(crate) struct TextRow {
    glyphs: Vec<PlacedGlyph>,
    cache: HashMap<GlyphKey, CachedGlyph>,
    cell_width: u32,
    cell_height: u32,
    baseline: i32,
    cell_count: u32,
    origin_x: i32,
    origin_y: i32,
}

impl TextRow {
    #[allow(
        clippy::too_many_lines,
        reason = "the evidence loader keeps font selection, shaping, raster timing, and layout diagnostics together"
    )]
    pub(crate) fn load(integer_scale: u32) -> Result<Self> {
        let scale = u16::try_from(integer_scale).context("integer scale fits u16")?;
        if scale == 0 {
            bail!("integer scale must be positive");
        }
        let scale = f32::from(scale);
        let font_size = BASE_FONT_SIZE * scale;
        let origin_x = BASE_ROW_X
            .checked_mul(i32::try_from(integer_scale).context("integer scale fits i32")?)
            .context("scaled row x overflow")?;
        let origin_y = BASE_ROW_Y
            .checked_mul(i32::try_from(integer_scale).context("integer scale fits i32")?)
            .context("scaled row y overflow")?;
        let started = Instant::now();
        let faces = [
            resolve_face("primary", PRIMARY_FONT, "jetbrains mono")?,
            resolve_face("CJK fallback", CJK_FONT, "noto sans cjk")?,
            resolve_face("emoji fallback", EMOJI_FONT, "noto color emoji")?,
        ];
        println!(
            "Resolved and loaded explicit font set in {:.3} ms (before Wayland connection)",
            started.elapsed().as_secs_f64() * 1_000.0
        );

        let (cell_width, cell_height, baseline, mono_advance) = cell_metrics(&faces[0], font_size)?;
        println!(
            "Text row metrics: scale={integer_scale} size={font_size:.1}px cell={cell_width}x{cell_height}px baseline={baseline}px primary-M-advance={mono_advance:.3}px"
        );
        verify_style_advances(&faces[0], mono_advance, font_size)?;

        let mut scale_context = ScaleContext::new();
        let mut shape_context = ShapeContext::new();
        let mut cache = HashMap::new();
        let mut glyphs = Vec::new();
        let mut next_cell: u32 = 0;
        for (segment_index, (kind, text)) in CORPUS.iter().enumerate() {
            if segment_index != 0 {
                next_cell += 1;
            }
            if *kind == CorpusKind::Combining {
                let face_index = 0;
                let font = font_ref(&faces[face_index])?;
                let mut shaped_glyphs = Vec::new();
                let mut shaper = shape_context.builder(font).size(font_size).build();
                shaper.add_str(text);
                shaper.shape_with(|cluster| {
                    let cluster_advance = cluster.advance();
                    let mut pen = 0.0;
                    for glyph in cluster.glyphs {
                        shaped_glyphs.push((glyph.id, cluster_advance, pen + glyph.x, glyph.y));
                        pen += glyph.advance;
                    }
                });
                if shaped_glyphs.is_empty() {
                    bail!("Swash produced no glyphs for combining evidence sequence");
                }
                for (glyph_id, cluster_advance, x_offset, y_offset) in shaped_glyphs {
                    cache_glyph(
                        &mut scale_context,
                        &mut cache,
                        &faces,
                        face_index,
                        glyph_id,
                        font_size,
                    )?;
                    println!(
                        "  Combining face={} glyph={glyph_id} advance={cluster_advance:.3}px layout-cell={} shaped-offset=({x_offset:.3},{y_offset:.3})",
                        faces[face_index].family, next_cell
                    );
                    glyphs.push(PlacedGlyph {
                        key: GlyphKey {
                            face: face_index,
                            glyph: glyph_id,
                        },
                        cell: next_cell,
                        cells: 1,
                        cluster_advance,
                        x_offset,
                        y_offset,
                    });
                }
                next_cell += 1;
                continue;
            }

            for character in text.chars() {
                let cells = if matches!(kind, CorpusKind::Cjk | CorpusKind::Emoji) {
                    2
                } else {
                    1
                };
                if *kind == CorpusKind::BoxDrawing {
                    let key = GlyphKey {
                        face: BOX_DRAWING_FACE,
                        glyph: u16::try_from(u32::from(character))
                            .context("box-drawing codepoint fits u16")?,
                    };
                    let mask =
                        box_drawing::generate(character, cell_width, cell_height, integer_scale)
                            .with_context(|| format!("generate box-drawing glyph {character}"))?;
                    cache.entry(key).or_insert(CachedGlyph {
                        content: Content::Mask,
                        left: 0,
                        top: baseline,
                        width: mask.width,
                        height: mask.height,
                        data: mask.data,
                    });
                    println!(
                        "  BoxDrawing U+{:04X} source=Foot-custom layout-cell={next_cell}",
                        u32::from(character)
                    );
                    glyphs.push(PlacedGlyph {
                        key,
                        cell: next_cell,
                        cells: 1,
                        cluster_advance: f32::from(
                            u16::try_from(cell_width).context("cell width fits u16")?,
                        ),
                        x_offset: 0.0,
                        y_offset: 0.0,
                    });
                    next_cell += 1;
                    continue;
                }
                let (face_index, glyph_id) = select_glyph(&faces, *kind, character)?;
                let font = font_ref(&faces[face_index])?;
                let advance = font
                    .glyph_metrics(&[])
                    .scale(font_size)
                    .advance_width(glyph_id);
                let raster_started = Instant::now();
                cache_glyph(
                    &mut scale_context,
                    &mut cache,
                    &faces,
                    face_index,
                    glyph_id,
                    font_size,
                )?;
                println!(
                    "  {:?} U+{:04X} face={} glyph={} advance={advance:.3}px layout-cell={} raster/cache={:.3} ms",
                    kind,
                    u32::from(character),
                    faces[face_index].family,
                    glyph_id,
                    next_cell,
                    raster_started.elapsed().as_secs_f64() * 1_000.0
                );
                glyphs.push(PlacedGlyph {
                    key: GlyphKey {
                        face: face_index,
                        glyph: glyph_id,
                    },
                    cell: next_cell,
                    cells,
                    cluster_advance: advance,
                    x_offset: 0.0,
                    y_offset: 0.0,
                });
                next_cell += cells;
            }
        }
        println!(
            "Rasterized {} unique glyph images for {} layout cells",
            cache.len(),
            next_cell
        );

        Ok(Self {
            glyphs,
            cache,
            cell_width,
            cell_height,
            baseline,
            cell_count: next_cell,
            origin_x,
            origin_y,
        })
    }
}

fn cache_glyph(
    context: &mut ScaleContext,
    cache: &mut HashMap<GlyphKey, CachedGlyph>,
    faces: &[FontFace; 3],
    face_index: usize,
    glyph_id: u16,
    font_size: f32,
) -> Result<()> {
    let key = GlyphKey {
        face: face_index,
        glyph: glyph_id,
    };
    if cache.contains_key(&key) {
        return Ok(());
    }
    let font = font_ref(&faces[face_index])?;
    let mut scaler = context.builder(font).size(font_size).hint(true).build();
    let image = Render::new(&[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ])
    .format(Format::Alpha)
    .render(&mut scaler, glyph_id)
    .with_context(|| {
        format!(
            "rasterize glyph {glyph_id} from {}",
            faces[face_index].label
        )
    })?;
    let glyph = CachedGlyph {
        content: image.content,
        left: image.placement.left,
        top: image.placement.top,
        width: image.placement.width,
        height: image.placement.height,
        data: image.data,
    };
    println!(
        "    raster ink={:?} placement=({}, {}) image={}x{}",
        glyph.ink_bounds(),
        glyph.left,
        glyph.top,
        glyph.width,
        glyph.height
    );
    cache.insert(key, glyph);
    Ok(())
}

fn cell_metrics(primary_face: &FontFace, font_size: f32) -> Result<(u32, u32, i32, f32)> {
    let primary = font_ref(primary_face)?;
    let metrics = primary.metrics(&[]).scale(font_size);
    let mono_advance = primary
        .glyph_metrics(&[])
        .scale(font_size)
        .advance_width(primary.charmap().map('M'));
    let cell_width = positive_ceil_to_u32(mono_advance);
    let cell_height =
        positive_ceil_to_u32(metrics.ascent + metrics.descent + metrics.leading.max(0.0));
    let baseline = ceil_to_i32(metrics.ascent);
    Ok((cell_width, cell_height, baseline, mono_advance))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn positive_ceil_to_u32(value: f32) -> u32 {
    assert!(value.is_finite() && value > 0.0);
    value.ceil() as u32
}

#[allow(clippy::cast_possible_truncation)]
fn ceil_to_i32(value: f32) -> i32 {
    assert!(value.is_finite());
    value.ceil() as i32
}

fn resolve_face(
    label: &'static str,
    pattern: &str,
    expected_family_fragment: &str,
) -> Result<FontFace> {
    let output = Command::new("fc-match")
        .args([
            "-f",
            "%{file}\\n%{index}\\n%{family}\\n%{style}\\n",
            pattern,
        ])
        .output()
        .with_context(|| format!("run fc-match for {label}"))?;
    if !output.status.success() {
        bail!(
            "fc-match failed for {label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8(output.stdout).context("fc-match output is not UTF-8")?;
    let mut lines = stdout.lines();
    let path = PathBuf::from(
        lines
            .next()
            .with_context(|| format!("fc-match returned no font path for {label}"))?,
    );
    let index = lines
        .next()
        .with_context(|| format!("fc-match returned no face index for {label}"))?
        .parse::<usize>()
        .with_context(|| format!("fc-match returned a non-numeric face index for {label}"))?;
    let family = lines
        .next()
        .with_context(|| format!("fc-match returned no family for {label}"))?
        .to_owned();
    let style = lines
        .next()
        .with_context(|| format!("fc-match returned no style for {label}"))?
        .to_owned();
    let normalized_family: String = family
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let normalized_expected: String = expected_family_fragment
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if !normalized_family.contains(&normalized_expected) {
        bail!("explicit {label} pattern {pattern:?} resolved unexpectedly to {family:?}");
    }
    let data = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let face = FontFace {
        label,
        family,
        style,
        path,
        index,
        data,
    };
    let _ = font_ref(&face)?;
    println!(
        "Selected {label}: {} {} (face {}, {})",
        face.family,
        face.style,
        face.index,
        face.path.display()
    );
    Ok(face)
}

fn verify_style_advances(regular: &FontFace, regular_advance: f32, font_size: f32) -> Result<()> {
    let mut identities = vec![(regular.path.clone(), regular.index)];
    for (label, pattern, expected_style) in [
        ("primary bold", PRIMARY_BOLD_FONT, "bold"),
        ("primary italic", PRIMARY_ITALIC_FONT, "italic"),
        (
            "primary bold italic",
            PRIMARY_BOLD_ITALIC_FONT,
            "bold italic",
        ),
    ] {
        let face = resolve_face(label, pattern, "jetbrains mono")?;
        let identity = (face.path.clone(), face.index);
        if identities.contains(&identity) {
            bail!("{label} silently resolved to an already selected face");
        }
        if !face.style.eq_ignore_ascii_case(expected_style) {
            bail!("{label} resolved to unexpected style {:?}", face.style);
        }
        identities.push(identity);
        let font = font_ref(&face)?;
        let advance = font
            .glyph_metrics(&[])
            .scale(font_size)
            .advance_width(font.charmap().map('M'));
        if !advance.is_finite() || (advance - regular_advance).abs() > 0.01 {
            bail!("{label} advance {advance:.3}px differs from regular {regular_advance:.3}px");
        }
        println!(
            "Style evidence: {} path={} M-advance={advance:.3}px",
            face.style,
            face.path.display()
        );
    }
    Ok(())
}

fn font_ref(face: &FontFace) -> Result<FontRef<'_>> {
    FontRef::from_index(&face.data, face.index).with_context(|| {
        format!(
            "parse {} face {} with Swash",
            face.path.display(),
            face.index
        )
    })
}

fn select_glyph(faces: &[FontFace; 3], kind: CorpusKind, character: char) -> Result<(usize, u16)> {
    let order: &[usize] = match kind {
        CorpusKind::Cjk => &[1, 0, 2],
        CorpusKind::Emoji => &[2, 0, 1],
        _ => &[0, 1, 2],
    };
    order
        .iter()
        .find_map(|face_index| {
            let glyph = font_ref(&faces[*face_index]).ok()?.charmap().map(character);
            (glyph != 0).then_some((*face_index, glyph))
        })
        .with_context(|| format!("no explicit font covers U+{:04X}", u32::from(character)))
}

pub(crate) fn paint(canvas: &mut [u8], width: u32, height: u32, row: &TextRow) {
    let expected_len = usize::try_from(width)
        .expect("canvas width fits usize")
        .checked_mul(usize::try_from(height).expect("canvas height fits usize"))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("canvas dimensions fit usize");
    assert_eq!(canvas.len(), expected_len, "canvas matches its dimensions");
    for pixel in canvas.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[22, 18, 14, 0xff]);
    }

    let row_width = row.cell_width.saturating_mul(row.cell_count);
    fill_rect(
        canvas,
        width,
        height,
        (row.origin_x, row.origin_y, row_width, row.cell_height),
        [31, 27, 22, 0xff],
    );
    for cell in 0..=row.cell_count {
        fill_rect(
            canvas,
            width,
            height,
            (
                row.origin_x
                    + i32::try_from(cell.saturating_mul(row.cell_width)).expect("cell x fits i32"),
                row.origin_y,
                1,
                row.cell_height,
            ),
            [48, 43, 36, 0xff],
        );
    }

    for placed in &row.glyphs {
        let glyph = &row.cache[&placed.key];
        let span = row.cell_width.saturating_mul(placed.cells);
        let centered_pen = (u32_to_f32(span) - placed.cluster_advance) / 2.0;
        let (x, y) = glyph_origin(row, placed, glyph, centered_pen);
        blend_glyph(canvas, width, height, x, y, glyph);
    }
}

fn glyph_origin(
    row: &TextRow,
    placed: &PlacedGlyph,
    glyph: &CachedGlyph,
    centered_pen: f32,
) -> (i32, i32) {
    let pen_x = row.origin_x
        + i32::try_from(placed.cell.saturating_mul(row.cell_width)).expect("glyph cell x fits i32")
        + round_to_i32(centered_pen + placed.x_offset);
    let baseline_y = row.origin_y + row.baseline - round_to_i32(placed.y_offset);
    (pen_x + glyph.left, baseline_y - glyph.top)
}

#[allow(clippy::cast_precision_loss)]
fn u32_to_f32(value: u32) -> f32 {
    value as f32
}

#[allow(clippy::cast_possible_truncation)]
fn round_to_i32(value: f32) -> i32 {
    assert!(value.is_finite());
    value.round() as i32
}

fn fill_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    rect: (i32, i32, u32, u32),
    color: [u8; 4],
) {
    let (x, y, width, height) = rect;
    for dy in 0..height {
        for dx in 0..width {
            put_pixel(
                canvas,
                canvas_width,
                canvas_height,
                x + i32::try_from(dx).expect("rectangle x fits i32"),
                y + i32::try_from(dy).expect("rectangle y fits i32"),
                color,
            );
        }
    }
}

fn blend_glyph(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: i32,
    y: i32,
    glyph: &CachedGlyph,
) {
    let pixel_count = usize::try_from(glyph.width)
        .expect("glyph width fits usize")
        .saturating_mul(usize::try_from(glyph.height).expect("glyph height fits usize"));
    let bytes_per_pixel = match glyph.content {
        Content::Mask => 1,
        Content::SubpixelMask | Content::Color => 4,
    };
    assert_eq!(
        glyph.data.len(),
        pixel_count.saturating_mul(bytes_per_pixel),
        "Swash glyph image has the expected byte count"
    );
    for gy in 0..glyph.height {
        for gx in 0..glyph.width {
            let pixel_index = usize::try_from(gy.saturating_mul(glyph.width) + gx)
                .expect("glyph pixel index fits usize");
            let source = match glyph.content {
                Content::Mask => [235, 235, 235, glyph.data[pixel_index]],
                Content::Color => {
                    let offset = pixel_index * 4;
                    [
                        glyph.data[offset],
                        glyph.data[offset + 1],
                        glyph.data[offset + 2],
                        glyph.data[offset + 3],
                    ]
                }
                Content::SubpixelMask => {
                    let offset = pixel_index * 4;
                    let alpha = glyph.data[offset..offset + 4]
                        .iter()
                        .copied()
                        .max()
                        .unwrap_or(0);
                    [235, 235, 235, alpha]
                }
            };
            blend_pixel(
                canvas,
                canvas_width,
                canvas_height,
                x + i32::try_from(gx).expect("glyph x fits i32"),
                y + i32::try_from(gy).expect("glyph y fits i32"),
                source,
            );
        }
    }
}

fn pixel_index(width: u32, height: u32, x: i32, y: i32) -> Option<usize> {
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    if x >= width || y >= height {
        return None;
    }
    usize::try_from(y.checked_mul(width)?.checked_add(x)?)
        .ok()?
        .checked_mul(4)
}

fn put_pixel(canvas: &mut [u8], width: u32, height: u32, x: i32, y: i32, rgba: [u8; 4]) {
    let Some(index) = pixel_index(width, height, x, y) else {
        return;
    };
    canvas[index..index + 4].copy_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
}

fn blend_pixel(canvas: &mut [u8], width: u32, height: u32, x: i32, y: i32, rgba: [u8; 4]) {
    let Some(index) = pixel_index(width, height, x, y) else {
        return;
    };
    let alpha = u32::from(rgba[3]);
    let inverse = 255 - alpha;
    for (destination_channel, source_channel) in canvas[index..index + 3]
        .iter_mut()
        .zip([rgba[2], rgba[1], rgba[0]])
    {
        *destination_channel = u8::try_from(
            (u32::from(source_channel) * alpha + u32::from(*destination_channel) * inverse + 127)
                / 255,
        )
        .expect("blended channel fits u8");
    }
    canvas[index + 3] = 0xff;
}

/// Runs the deterministic renderer evidence benchmark and returns a JSON report.
///
/// The caller should use a release build. Timings separate complete setup (fontconfig,
/// file loading, shaping, and cold raster), warm cache lookup, blend, and generated
/// box-mask work. They are evidence, not latency assertions.
///
/// # Errors
///
/// Returns an error if the font stack or deterministic row cannot be initialized.
pub fn benchmark_json(samples: usize) -> Result<serde_json::Value> {
    if samples == 0 {
        bail!("benchmark sample count must be positive");
    }
    let setup_started = Instant::now();
    let row = TextRow::load(1)?;
    let setup_ns =
        u64::try_from(setup_started.elapsed().as_nanos()).context("setup duration fits u64")?;
    let mut canvas = vec![0_u8; 960 * 600 * 4];
    let mut lookup = Vec::with_capacity(samples);
    let mut blend = Vec::with_capacity(samples);
    let mut boxes = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        for placed in &row.glyphs {
            std::hint::black_box(&row.cache[&placed.key]);
        }
        lookup.push(u64::try_from(started.elapsed().as_nanos()).context("lookup duration")?);

        let started = Instant::now();
        paint(&mut canvas, 960, 600, &row);
        std::hint::black_box(&canvas);
        blend.push(u64::try_from(started.elapsed().as_nanos()).context("blend duration")?);

        let started = Instant::now();
        for character in ['┌', '─', '┼', '┐'] {
            std::hint::black_box(box_drawing::generate(
                character,
                row.cell_width,
                row.cell_height,
                1,
            ));
        }
        boxes.push(u64::try_from(started.elapsed().as_nanos()).context("box duration")?);
    }
    Ok(serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "samples": samples,
        "setup_ns": setup_ns,
        "warm_cache_lookup_ns": timing_summary(&mut lookup),
        "full_canvas_blend_ns": timing_summary(&mut blend),
        "box_generation_ns": timing_summary(&mut boxes),
        "cell": { "width": row.cell_width, "height": row.cell_height, "baseline": row.baseline },
        "canvas": { "width": 960, "height": 600 }
    }))
}

fn timing_summary(samples: &mut [u64]) -> serde_json::Value {
    samples.sort_unstable();
    let percentile = |numerator: usize| {
        let index = (samples.len() - 1).saturating_mul(numerator) / 100;
        samples[index]
    };
    serde_json::json!({
        "min": samples[0],
        "median": percentile(50),
        "p95": percentile(95),
        "max": samples[samples.len() - 1]
    })
}

/// Writes an opaque ARGB8888 canvas as a lossless binary PPM (P6) capture.
///
/// PPM keeps the evidence path dependency-free. The alpha byte is omitted because
/// the window renderer always produces an opaque canvas.
///
/// # Errors
///
/// Returns an error when dimensions overflow, the canvas length does not match
/// the dimensions, or the capture cannot be created or written.
pub fn write_ppm(path: impl AsRef<Path>, canvas: &[u8], width: u32, height: u32) -> io::Result<()> {
    let expected_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "capture dimensions overflow")
        })?;
    if canvas.len() != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture canvas does not match dimensions",
        ));
    }

    let mut file = fs::File::create(path)?;
    write!(file, "P6\n{width} {height}\n255\n")?;
    for pixel in canvas.chunks_exact(4) {
        file.write_all(&[pixel[2], pixel[1], pixel[0]])?;
    }
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_row() -> TextRow {
        let key = GlyphKey { face: 0, glyph: 1 };
        TextRow {
            glyphs: vec![
                PlacedGlyph {
                    key,
                    cell: 0,
                    cells: 1,
                    cluster_advance: 4.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                },
                PlacedGlyph {
                    key,
                    cell: 1,
                    cells: 1,
                    cluster_advance: 4.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                },
            ],
            cache: HashMap::from([(
                key,
                CachedGlyph {
                    content: Content::Mask,
                    left: 0,
                    top: 2,
                    width: 2,
                    height: 2,
                    data: vec![0xff, 0x80, 0x40, 0],
                },
            )]),
            cell_width: 8,
            cell_height: 12,
            baseline: 9,
            cell_count: 2,
            origin_x: BASE_ROW_X,
            origin_y: BASE_ROW_Y,
        }
    }

    #[test]
    fn deterministic_row_paints_identical_opaque_canvases() {
        let row = synthetic_row();
        let (width, height) = (96_u32, 128_u32);
        let mut first = vec![0; 96 * 128 * 4];
        let mut second = vec![0; 96 * 128 * 4];

        paint(&mut first, width, height, &row);
        paint(&mut second, width, height, &row);

        assert_eq!(first, second);
        assert!(first.chunks_exact(4).all(|pixel| pixel[3] == 0xff));
        assert_eq!(row.cache.len(), 1, "repeated glyphs share one cached image");
    }

    #[test]
    fn placement_centers_the_pen_then_applies_bearings_and_shaped_offsets() {
        let row = synthetic_row();
        let mut placed = PlacedGlyph {
            key: GlyphKey { face: 0, glyph: 1 },
            cell: 2,
            cells: 1,
            cluster_advance: 4.0,
            x_offset: -1.0,
            y_offset: 3.0,
        };
        let glyph = CachedGlyph {
            content: Content::Mask,
            left: -2,
            top: 5,
            width: 7,
            height: 8,
            data: vec![0; 56],
        };
        let centered_pen = (u32_to_f32(row.cell_width) - placed.cluster_advance) / 2.0;

        assert_eq!(glyph_origin(&row, &placed, &glyph, centered_pen), (47, 97));

        placed.x_offset = 2.0;
        assert_eq!(glyph_origin(&row, &placed, &glyph, centered_pen), (50, 97));
    }

    #[test]
    fn shaped_combining_glyphs_share_a_cluster_advance_and_cell() {
        let key = GlyphKey { face: 0, glyph: 2 };
        let base = PlacedGlyph {
            key,
            cell: 3,
            cells: 1,
            cluster_advance: 8.0,
            x_offset: 0.0,
            y_offset: 0.0,
        };
        let mark = PlacedGlyph {
            key,
            x_offset: -2.0,
            y_offset: 4.0,
            ..base
        };

        assert_eq!(base.cell, mark.cell);
        assert!((base.cluster_advance - mark.cluster_advance).abs() < f32::EPSILON);
        assert!((base.x_offset - mark.x_offset).abs() > f32::EPSILON);
        assert!((base.y_offset - mark.y_offset).abs() > f32::EPSILON);
    }

    #[test]
    fn ink_bounds_cover_mask_and_color_images() {
        let mask = CachedGlyph {
            content: Content::Mask,
            left: 0,
            top: 0,
            width: 3,
            height: 2,
            data: vec![0, 1, 0, 0, 1, 0],
        };
        assert_eq!(
            mask.ink_bounds(),
            Some(InkBounds {
                left: 1,
                top: 0,
                right: 2,
                bottom: 2,
            })
        );

        let color = CachedGlyph {
            content: Content::Color,
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            data: vec![10, 20, 30, 0, 10, 20, 30, 40],
        };
        assert_eq!(
            color.ink_bounds(),
            Some(InkBounds {
                left: 1,
                top: 0,
                right: 2,
                bottom: 1,
            })
        );
    }

    #[test]
    fn paint_clips_the_row_outside_a_small_canvas() {
        let row = synthetic_row();
        let mut canvas = vec![0; 3 * 2 * 4];

        paint(&mut canvas, 3, 2, &row);

        assert_eq!(canvas, [22, 18, 14, 0xff].repeat(6));
    }

    #[test]
    fn corpus_contains_each_required_evidence_segment() {
        assert_eq!(CORPUS.len(), 6);
        assert_eq!(CORPUS[0], (CorpusKind::Ascii, "ASCII"));
        assert_eq!(CORPUS[1], (CorpusKind::BoxDrawing, "┌─┼─┐"));
        assert_eq!(CORPUS[2], (CorpusKind::NerdFont, "\u{f120}"));
        assert_eq!(CORPUS[3], (CorpusKind::Combining, "e\u{0301}"));
        assert_eq!(CORPUS[4], (CorpusKind::Cjk, "界"));
        assert_eq!(CORPUS[5], (CorpusKind::Emoji, "🙂"));
    }

    #[test]
    fn ppm_capture_is_lossless_rgb_and_checks_dimensions() {
        let path = std::env::temp_dir().join(format!(
            "splinterm-renderer-{}-{}.ppm",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let canvas = [3, 2, 1, 0xff, 30, 20, 10, 0xff];

        write_ppm(&path, &canvas, 2, 1).expect("write capture");
        let capture = fs::read(&path).expect("read capture");
        fs::remove_file(path).expect("remove capture");

        assert_eq!(&capture[..11], b"P6\n2 1\n255\n");
        assert_eq!(&capture[11..], &[1, 2, 3, 10, 20, 30]);
        assert_eq!(
            write_ppm(std::env::temp_dir().join("unused.ppm"), &canvas, 1, 1)
                .expect_err("dimension mismatch")
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
