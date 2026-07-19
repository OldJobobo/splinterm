//! Deterministic CPU text-row rasterization for the native client.
//!
//! Font selection, cell placement, fallback, and CPU blending are compared against
//! Foot 1.27.0 `fonts.c` and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`. It also owns the persistent,
//! scale-keyed glyph cache and damage-oriented terminal snapshot painter.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, OnceLock},
    time::Instant,
};

use crate::{box_drawing, config::CursorStyle};
use anyhow::{Context, Result, bail};

use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, MAX_COLUMNS, MAX_ROWS, ScrollDirection,
    TerminalCell, TerminalInputModes, TerminalRow, TerminalScroll, TerminalSnapshot,
};
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
const SNAPSHOT_GLYPH_CACHE_BUDGET: usize = 2_048;

static SNAPSHOT_FACES: OnceLock<Result<[FontFace; 3], String>> = OnceLock::new();
static RENDERER_OPTIONS: OnceLock<RendererOptions> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct RendererOptions {
    pub font: String,
    pub font_size: f32,
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            font: PRIMARY_FONT.to_owned(),
            font_size: BASE_FONT_SIZE,
        }
    }
}

/// Installs immutable per-process renderer configuration before a window opens.
/// A process owns one graphical window in the MVP, so caches cannot mix fonts.
///
/// # Errors
/// Returns an error for an invalid size or a second configuration attempt.
pub fn configure(options: RendererOptions) -> Result<()> {
    if !options.font_size.is_finite() || !(6.0..=96.0).contains(&options.font_size) {
        bail!("font size must be between 6 and 96 pixels");
    }
    RENDERER_OPTIONS
        .set(options)
        .map_err(|_| anyhow::anyhow!("renderer is already configured"))
}

fn renderer_options() -> &'static RendererOptions {
    RENDERER_OPTIONS.get_or_init(RendererOptions::default)
}

struct PersistentGlyphCache {
    context: ScaleContext,
    glyphs: HashMap<(u16, GlyphKey), Arc<CachedGlyph>>,
    order: VecDeque<(u16, GlyphKey)>,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl Default for PersistentGlyphCache {
    fn default() -> Self {
        Self {
            context: ScaleContext::new(),
            glyphs: HashMap::new(),
            order: VecDeque::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }
}

thread_local! {
    static SNAPSHOT_GLYPH_CACHE: RefCell<PersistentGlyphCache> =
        RefCell::new(PersistentGlyphCache::default());
}

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
        eprintln!(
            "Resolved and loaded explicit font set in {:.3} ms (before Wayland connection)",
            started.elapsed().as_secs_f64() * 1_000.0
        );

        let (cell_width, cell_height, baseline, mono_advance) = cell_metrics(&faces[0], font_size)?;
        eprintln!(
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
                    eprintln!(
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
                    eprintln!(
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
                eprintln!(
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
        eprintln!(
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
    eprintln!(
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

fn snapshot_faces() -> Result<&'static [FontFace; 3]> {
    SNAPSHOT_FACES
        .get_or_init(|| {
            Ok([
                resolve_face("primary", &renderer_options().font, "")
                    .map_err(|error| error.to_string())?,
                resolve_face("CJK fallback", CJK_FONT, "noto sans cjk")
                    .map_err(|error| error.to_string())?,
                resolve_face("emoji fallback", EMOJI_FONT, "noto color emoji")
                    .map_err(|error| error.to_string())?,
            ])
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

fn snapshot_glyph(
    faces: &[FontFace; 3],
    scale: u16,
    face_index: usize,
    glyph_id: u16,
    font_size: f32,
) -> Result<Arc<CachedGlyph>> {
    let key = GlyphKey {
        face: face_index,
        glyph: glyph_id,
    };
    SNAPSHOT_GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(glyph) = cache.glyphs.get(&(scale, key)).cloned() {
            cache.hits = cache.hits.saturating_add(1);
            return Ok(glyph);
        }
        cache.misses = cache.misses.saturating_add(1);
        let font = font_ref(&faces[face_index])?;
        let mut scaler = cache
            .context
            .builder(font)
            .size(font_size)
            .hint(true)
            .build();
        let image = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .render(&mut scaler, glyph_id)
        .with_context(|| format!("rasterize snapshot glyph {glyph_id}"))?;
        let glyph = Arc::new(CachedGlyph {
            content: image.content,
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            data: image.data,
        });
        while cache.glyphs.len() >= SNAPSHOT_GLYPH_CACHE_BUDGET {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            if cache.glyphs.remove(&oldest).is_some() {
                cache.evictions = cache.evictions.saturating_add(1);
            }
        }
        cache.order.push_back((scale, key));
        cache.glyphs.insert((scale, key), Arc::clone(&glyph));
        Ok(glyph)
    })
}

/// Returns bounded persistent snapshot-glyph-cache metrics.
#[must_use]
pub fn snapshot_cache_metrics() -> serde_json::Value {
    SNAPSHOT_GLYPH_CACHE.with(|cache| {
        let cache = cache.borrow();
        serde_json::json!({
            "entries": cache.glyphs.len(),
            "budget": SNAPSHOT_GLYPH_CACHE_BUDGET,
            "hits": cache.hits,
            "misses": cache.misses,
            "evictions": cache.evictions,
            "approximate_bytes": cache.glyphs.values().map(|glyph| glyph.data.len()).sum::<usize>(),
        })
    })
}

fn cell_metrics(primary_face: &FontFace, font_size: f32) -> Result<(u32, u32, i32, f32)> {
    let primary = font_ref(primary_face)?;
    let metrics = primary.metrics(&[]).scale(font_size);
    let mono_advance = primary
        .glyph_metrics(&[])
        .scale(font_size)
        .advance_width(primary.charmap().map('M'));
    // fcft exposes the 26.6 advance as an integer cell width (13 px for the
    // accepted 22 px JetBrains Mono fixture). Ceil widened 13.2 px to 14 px,
    // creating a visible extra column between every ASCII character.
    let cell_width = positive_round_to_u32(mono_advance);
    let cell_height =
        positive_ceil_to_u32(metrics.ascent + metrics.descent + metrics.leading.max(0.0));
    let baseline = ceil_to_i32(metrics.ascent);
    Ok((cell_width, cell_height, baseline, mono_advance))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn positive_round_to_u32(value: f32) -> u32 {
    assert!(value.is_finite() && value > 0.0);
    value.round().max(1.0) as u32
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
    if !normalized_expected.is_empty() && !normalized_family.contains(&normalized_expected) {
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
    eprintln!(
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
        eprintln!(
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
        blend_glyph(canvas, width, height, x, y, glyph, [235, 235, 235]);
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

fn blend_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    rect: (i32, i32, u32, u32),
    color: [u8; 4],
) {
    let (x, y, width, height) = rect;
    for dy in 0..height {
        for dx in 0..width {
            blend_pixel(
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
    foreground: [u8; 3],
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
                Content::Mask => [
                    foreground[0],
                    foreground[1],
                    foreground[2],
                    glyph.data[pixel_index],
                ],
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
                    [foreground[0], foreground[1], foreground[2], alpha]
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct SnapshotGlyph {
    key: GlyphKey,
    column: u32,
    row: u32,
    cells: u32,
    cluster_advance: f32,
    x_offset: f32,
    y_offset: f32,
    foreground: [u8; 3],
}

/// One immutable, scale-dependent rendering of an owned daemon snapshot.
pub(crate) struct SnapshotFrame {
    glyphs: Vec<SnapshotGlyph>,
    cache: HashMap<GlyphKey, Arc<CachedGlyph>>,
    backgrounds: Vec<[u8; 3]>,
    columns: u32,
    rows: u32,
    cell_width: u32,
    cell_height: u32,
    baseline: i32,
    origin_x: i32,
    origin_y: i32,
    cursor: Option<(u32, u32)>,
    canvas_background: [u8; 3],
    cursor_color: [u8; 3],
    scale_120: u16,
}

impl SnapshotFrame {
    pub(crate) fn initial_logical_size(
        &self,
        columns: u16,
        rows: u16,
        scale_120: u32,
    ) -> Result<(u32, u32)> {
        let physical_width = u32::from(columns)
            .checked_mul(self.cell_width)
            .and_then(|value| {
                value.checked_add(u32::try_from(self.origin_x.max(0)).ok()?.checked_mul(2)?)
            })
            .context("initial width overflow")?;
        let physical_height = u32::from(rows)
            .checked_mul(self.cell_height)
            .and_then(|value| {
                value.checked_add(u32::try_from(self.origin_y.max(0)).ok()?.checked_mul(2)?)
            })
            .context("initial height overflow")?;
        if scale_120 == 0 {
            bail!("initial scale cannot be zero");
        }
        Ok((
            physical_width
                .saturating_mul(120)
                .div_ceil(scale_120)
                .max(480),
            physical_height
                .saturating_mul(120)
                .div_ceil(scale_120)
                .max(300),
        ))
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite nonnegative coordinates are divided and bounds-checked before use"
    )]
    pub(crate) fn cell_at(
        &self,
        logical_x: f64,
        logical_y: f64,
        scale_120: u32,
    ) -> Option<(usize, usize)> {
        if !logical_x.is_finite() || !logical_y.is_finite() || scale_120 == 0 {
            return None;
        }
        let scale = f64::from(scale_120) / 120.0;
        let x = logical_x * scale - f64::from(self.origin_x);
        let y = logical_y * scale - f64::from(self.origin_y);
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let column = (x / f64::from(self.cell_width)) as usize;
        let row = (y / f64::from(self.cell_height)) as usize;
        (column < self.columns as usize && row < self.rows as usize).then_some((row, column))
    }

    pub(crate) fn cursor_rectangle(&self, scale_120: u32) -> Option<(i32, i32, i32, i32)> {
        let (column, row) = self.cursor?;
        if scale_120 == 0 {
            return None;
        }
        let scale = f64::from(scale_120) / 120.0;
        let physical_x =
            self.origin_x + i32::try_from(column.checked_mul(self.cell_width)?).ok()?;
        let physical_y = self.origin_y + i32::try_from(row.checked_mul(self.cell_height)?).ok()?;
        Some((
            checked_floor_i32(f64::from(physical_x) / scale)?,
            checked_floor_i32(f64::from(physical_y) / scale)?,
            checked_ceil_i32(f64::from(self.cell_width) / scale)?,
            checked_ceil_i32(f64::from(self.cell_height) / scale)?,
        ))
    }

    pub(crate) fn terminal_size(
        &self,
        logical_width: u32,
        logical_height: u32,
        scale_120: u32,
    ) -> Result<(u16, u16, u16, u16)> {
        let physical_width = scaled_dimension(logical_width, scale_120)?;
        let physical_height = scaled_dimension(logical_height, scale_120)?;
        let horizontal_padding = u32::try_from(self.origin_x.max(0))
            .context("terminal x padding")?
            .checked_mul(2)
            .context("terminal horizontal padding overflow")?;
        let vertical_padding = u32::try_from(self.origin_y.max(0))
            .context("terminal y padding")?
            .checked_mul(2)
            .context("terminal vertical padding overflow")?;
        let drawable_width = physical_width.saturating_sub(horizontal_padding);
        let drawable_height = physical_height.saturating_sub(vertical_padding);
        let columns = (drawable_width / self.cell_width).clamp(2, u32::from(MAX_COLUMNS));
        let rows = (drawable_height / self.cell_height).clamp(2, u32::from(MAX_ROWS));
        let pixel_width = columns
            .checked_mul(self.cell_width)
            .context("terminal pixel width overflow")?;
        let pixel_height = rows
            .checked_mul(self.cell_height)
            .context("terminal pixel height overflow")?;
        Ok((
            u16::try_from(columns).context("terminal columns fit u16")?,
            u16::try_from(rows).context("terminal rows fit u16")?,
            u16::try_from(pixel_width).context("terminal pixel width fits u16")?,
            u16::try_from(pixel_height).context("terminal pixel height fits u16")?,
        ))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "static snapshot preparation keeps bounded font, shaping, cache, cell-span, and color conversion together"
    )]
    pub(crate) fn load(snapshot: &TerminalSnapshot, integer_scale: u32) -> Result<Self> {
        let scale_120 = integer_scale
            .checked_mul(120)
            .context("integer scale overflow")?;
        Self::load_scaled(snapshot, scale_120)
    }

    pub(crate) fn load_scaled(snapshot: &TerminalSnapshot, scale_120: u32) -> Result<Self> {
        if snapshot.columns > usize::from(MAX_COLUMNS) || snapshot.rows > usize::from(MAX_ROWS) {
            bail!(
                "snapshot dimensions {}x{} exceed protocol limits {}x{}",
                snapshot.columns,
                snapshot.rows,
                MAX_COLUMNS,
                MAX_ROWS
            );
        }
        if !(120..=960).contains(&scale_120) {
            bail!("scale must be between 1x and 8x");
        }
        let scale_120 = u16::try_from(scale_120).context("scale fits u16")?;
        let scale_factor = f32::from(scale_120) / 120.0;
        let font_size = renderer_options().font_size * scale_factor;
        let faces = snapshot_faces()?;
        let (cell_width, cell_height, baseline, _) = cell_metrics(&faces[0], font_size)?;
        let origin = u32::from(scale_120)
            .checked_mul(12)
            .and_then(|value| value.checked_add(60))
            .map(|value| value / 120)
            .and_then(|value| i32::try_from(value).ok())
            .context("scaled snapshot origin fits i32")?;
        let origin_x = origin;
        let origin_y = origin;
        let columns = u32::try_from(snapshot.columns).context("snapshot columns fit u32")?;
        let rows = u32::try_from(snapshot.rows).context("snapshot rows fit u32")?;
        let background_len = usize::try_from(columns)
            .ok()
            .and_then(|columns| {
                usize::try_from(rows)
                    .ok()
                    .and_then(|rows| columns.checked_mul(rows))
            })
            .context("snapshot background size overflow")?;
        if snapshot.palette.len() != 256 {
            bail!("snapshot palette must contain exactly 256 colors");
        }
        let default_foreground = packed_rgb(snapshot.default_colors[0]);
        let default_background = packed_rgb(snapshot.default_colors[1]);
        let cursor_color = packed_rgb(snapshot.default_colors[2]);
        let mut backgrounds = vec![default_background; background_len];
        let mut glyphs = Vec::new();
        let mut cache = HashMap::new();
        let mut shape_context = ShapeContext::new();

        for row_index in 0..snapshot.rows {
            prepare_snapshot_row(
                snapshot,
                row_index,
                faces,
                scale_120,
                font_size,
                cell_width,
                cell_height,
                baseline,
                default_foreground,
                default_background,
                &mut shape_context,
                &mut backgrounds,
                &mut glyphs,
                &mut cache,
            )?;
        }
        let cursor = snapshot_cursor(snapshot, columns, rows);
        let mut frame = Self {
            glyphs,
            cache,
            backgrounds,
            columns,
            rows,
            cell_width,
            cell_height,
            baseline,
            origin_x,
            origin_y,
            cursor,
            canvas_background: default_background,
            cursor_color,
            scale_120,
        };
        frame.prune_unreferenced_glyphs();
        Ok(frame)
    }

    /// Rebuilds only rows whose semantic cell content changed.
    pub(crate) fn refresh_rows(
        &mut self,
        snapshot: &TerminalSnapshot,
        dirty_rows: &[bool],
    ) -> Result<()> {
        if snapshot.columns != self.columns as usize || snapshot.rows != self.rows as usize {
            bail!("incremental frame dimensions changed");
        }
        if snapshot.palette.len() != 256 {
            bail!("snapshot palette must contain exactly 256 colors");
        }
        let faces = snapshot_faces()?;
        let font_size = renderer_options().font_size * f32::from(self.scale_120) / 120.0;
        let default_foreground = packed_rgb(snapshot.default_colors[0]);
        let default_background = packed_rgb(snapshot.default_colors[1]);
        let mut shape_context = ShapeContext::new();
        for (row_index, dirty) in dirty_rows.iter().copied().enumerate().take(snapshot.rows) {
            if !dirty {
                continue;
            }
            let row_number = u32::try_from(row_index).context("row fits u32")?;
            self.glyphs.retain(|glyph| glyph.row != row_number);
            let start = row_index
                .checked_mul(snapshot.columns)
                .context("snapshot row start overflow")?;
            let end = start
                .checked_add(snapshot.columns)
                .context("snapshot row end overflow")?;
            self.backgrounds[start..end].fill(default_background);
            prepare_snapshot_row(
                snapshot,
                row_index,
                faces,
                self.scale_120,
                font_size,
                self.cell_width,
                self.cell_height,
                self.baseline,
                default_foreground,
                default_background,
                &mut shape_context,
                &mut self.backgrounds,
                &mut self.glyphs,
                &mut self.cache,
            )?;
        }
        self.prune_unreferenced_glyphs();
        Ok(())
    }

    fn prune_unreferenced_glyphs(&mut self) {
        let referenced: HashSet<_> = self.glyphs.iter().map(|glyph| glyph.key).collect();
        self.cache.retain(|key, _| referenced.contains(key));
    }

    /// Updates cursor presentation without reshaping any terminal row.
    pub(crate) fn refresh_cursor(&mut self, snapshot: &TerminalSnapshot) {
        self.cursor = snapshot_cursor(snapshot, self.columns, self.rows);
    }
}

fn snapshot_cursor(snapshot: &TerminalSnapshot, columns: u32, rows: u32) -> Option<(u32, u32)> {
    if !snapshot.input_modes.cursor_visible {
        return None;
    }
    match (
        u32::try_from(snapshot.cursor_column),
        u32::try_from(snapshot.cursor_row),
    ) {
        (Ok(column), Ok(row)) if column < columns && row < rows => Some((column, row)),
        _ => None,
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "row preparation keeps one bounded shaping transaction explicit"
)]
fn prepare_snapshot_row(
    snapshot: &TerminalSnapshot,
    row_index: usize,
    faces: &[FontFace; 3],
    scale: u16,
    font_size: f32,
    cell_width: u32,
    cell_height: u32,
    baseline: i32,
    default_foreground: [u8; 3],
    default_background: [u8; 3],
    shape_context: &mut ShapeContext,
    backgrounds: &mut [[u8; 3]],
    glyphs: &mut Vec<SnapshotGlyph>,
    cache: &mut HashMap<GlyphKey, Arc<CachedGlyph>>,
) -> Result<()> {
    let Some(row) = snapshot.visible_rows.get(row_index) else {
        return Ok(());
    };
    for (column_index, cell) in row.cells.iter().take(snapshot.columns).enumerate() {
        let (foreground, background) = rendition_colors(
            &cell.attributes,
            &snapshot.palette,
            default_foreground,
            default_background,
        );
        let background_index = row_index
            .checked_mul(snapshot.columns)
            .and_then(|index| index.checked_add(column_index))
            .context("snapshot cell index overflow")?;
        backgrounds[background_index] = background;
        if !cell_is_renderable(cell) {
            continue;
        }
        let cells = leader_span(&row.cells, column_index);
        let column = u32::try_from(column_index).context("column fits u32")?;
        let row_number = u32::try_from(row_index).context("row fits u32")?;
        if cell.content.chars().count() == 1 {
            let character = cell.content.chars().next().context("cell has content")?;
            let thickness = u32::from(scale).div_ceil(120).max(1);
            if let Some(mask) = box_drawing::generate(character, cell_width, cell_height, thickness)
            {
                let key = GlyphKey {
                    face: BOX_DRAWING_FACE,
                    glyph: u16::try_from(u32::from(character)).context("box codepoint fits u16")?,
                };
                cache.entry(key).or_insert_with(|| {
                    Arc::new(CachedGlyph {
                        content: Content::Mask,
                        left: 0,
                        top: baseline,
                        width: mask.width,
                        height: mask.height,
                        data: mask.data,
                    })
                });
                glyphs.push(SnapshotGlyph {
                    key,
                    column,
                    row: row_number,
                    cells,
                    cluster_advance: u32_to_f32(cell_width),
                    x_offset: 0.0,
                    y_offset: 0.0,
                    foreground,
                });
                continue;
            }
        }
        let (face_index, content) = match select_face_for_text(faces, &cell.content) {
            Ok(face_index) => (face_index, cell.content.as_str()),
            Err(_) => (0, "\u{fffd}"),
        };
        let font = font_ref(&faces[face_index])?;
        let mut shaped_glyphs = Vec::new();
        let mut shaper = shape_context.builder(font).size(font_size).build();
        shaper.add_str(content);
        shaper.shape_with(|cluster| {
            let advance = cluster.advance();
            let mut pen = 0.0;
            for glyph in cluster.glyphs {
                shaped_glyphs.push((glyph.id, advance, pen + glyph.x, glyph.y));
                pen += glyph.advance;
            }
        });
        for (glyph_id, cluster_advance, x_offset, y_offset) in shaped_glyphs {
            let key = GlyphKey {
                face: face_index,
                glyph: glyph_id,
            };
            cache.entry(key).or_insert(snapshot_glyph(
                faces, scale, face_index, glyph_id, font_size,
            )?);
            glyphs.push(SnapshotGlyph {
                key,
                column,
                row: row_number,
                cells,
                cluster_advance,
                x_offset,
                y_offset,
                foreground,
            });
        }
    }
    Ok(())
}

fn cell_is_renderable(cell: &splinterm_protocol::TerminalCell) -> bool {
    !cell.content.is_empty() && cell.spacer_remaining.is_none() && !cell.attributes.conceal
}

fn leader_span(cells: &[splinterm_protocol::TerminalCell], leader: usize) -> u32 {
    let mut span = 1_u32;
    for following in cells.iter().skip(leader + 1) {
        if following
            .spacer_remaining
            .is_none_or(|remaining| remaining == 0)
        {
            break;
        }
        span = span.saturating_add(1);
    }
    span
}

fn select_face_for_text(faces: &[FontFace; 3], text: &str) -> Result<usize> {
    [0_usize, 1, 2]
        .into_iter()
        .find(|index| {
            font_ref(&faces[*index]).is_ok_and(|font| {
                text.chars()
                    .all(|character| font.charmap().map(character) != 0)
            })
        })
        .with_context(|| format!("no explicit font covers snapshot cell {text:?}"))
}

fn packed_rgb(value: u32) -> [u8; 3] {
    [
        u8::try_from((value >> 16) & 0xff).expect("red fits"),
        u8::try_from((value >> 8) & 0xff).expect("green fits"),
        u8::try_from(value & 0xff).expect("blue fits"),
    ]
}

#[cfg(test)]
fn default_foreground() -> [u8; 3] {
    [0xeb, 0xeb, 0xeb]
}
#[cfg(test)]
fn default_background() -> [u8; 3] {
    [0x0e, 0x12, 0x16]
}

fn wire_color(source: ColorSource, value: u32, palette: &[u32], default: [u8; 3]) -> [u8; 3] {
    match source {
        ColorSource::Default => default,
        ColorSource::Base16 | ColorSource::Base256 => usize::try_from(value)
            .ok()
            .and_then(|index| palette.get(index))
            .copied()
            .map_or(default, packed_rgb),
        ColorSource::Rgb => packed_rgb(value),
    }
}

fn rendition_colors(
    attributes: &CellAttributes,
    palette: &[u32],
    default_foreground: [u8; 3],
    default_background: [u8; 3],
) -> ([u8; 3], [u8; 3]) {
    let mut foreground = wire_color(
        attributes.foreground_source,
        attributes.foreground,
        palette,
        default_foreground,
    );
    let mut background = wire_color(
        attributes.background_source,
        attributes.background,
        palette,
        default_background,
    );
    if attributes.dim {
        for component in &mut foreground {
            *component /= 2;
        }
    }
    if attributes.reverse {
        std::mem::swap(&mut foreground, &mut background);
    }
    (foreground, background)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the explicit finite i32 bounds make the float-to-int conversion checked"
)]
fn checked_floor_i32(value: f64) -> Option<i32> {
    let value = value.floor();
    (value.is_finite() && value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX))
        .then_some(value as i32)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the explicit finite i32 bounds make the float-to-int conversion checked"
)]
fn checked_ceil_i32(value: f64) -> Option<i32> {
    let value = value.ceil();
    (value.is_finite() && value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX))
        .then_some(value as i32)
}

fn scaled_dimension(logical: u32, scale_120: u32) -> Result<u32> {
    if scale_120 == 0 {
        bail!("scale must be positive");
    }
    logical
        .checked_mul(scale_120)
        .and_then(|value| value.checked_add(119))
        .map(|value| value / 120)
        .context("scaled dimension overflow")
}

fn grid_pixel_offset(index: usize, cell_size: u32) -> Option<i32> {
    u32::try_from(index)
        .ok()?
        .checked_mul(cell_size)
        .and_then(|offset| i32::try_from(offset).ok())
}

#[derive(Clone, Copy)]
pub(crate) struct SnapshotOverlays<'a> {
    pub(crate) selection: Option<((usize, usize), (usize, usize))>,
    pub(crate) hovered_url: Option<((usize, usize), (usize, usize))>,
    pub(crate) dirty_rows: Option<&'a [bool]>,
    pub(crate) focused: bool,
    /// Packed `0xRRGGBB` project theme roles.
    pub(crate) selection_color: u32,
    pub(crate) url_color: u32,
    pub(crate) accent_color: u32,
}

pub(crate) fn paint_snapshot_overlays(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    overlays: SnapshotOverlays<'_>,
) {
    let SnapshotOverlays {
        selection,
        hovered_url,
        dirty_rows,
        focused: _focused,
        selection_color,
        url_color,
        accent_color: _accent_color,
    } = overlays;
    let themed_bgra = |color: u32, alpha: u8| {
        let [_, red, green, blue] = color.to_be_bytes();
        [blue, green, red, alpha]
    };
    // Focus framing belongs to the compositor. Painting a second solid frame
    // inside the client obscures Hyprland's active-border gradient.
    let row_is_dirty =
        |row: usize| dirty_rows.is_none_or(|dirty| dirty.get(row).copied().unwrap_or(false));
    if let Some((start, end)) = selection {
        for row in start.0..=end.0.min(frame.rows.saturating_sub(1) as usize) {
            if !row_is_dirty(row) {
                continue;
            }
            let first = if row == start.0 { start.1 } else { 0 };
            let last = if row == end.0 {
                end.1
            } else {
                frame.columns.saturating_sub(1) as usize
            };
            for column in first..=last.min(frame.columns.saturating_sub(1) as usize) {
                let (Some(x_offset), Some(y_offset)) = (
                    grid_pixel_offset(column, frame.cell_width),
                    grid_pixel_offset(row, frame.cell_height),
                ) else {
                    continue;
                };
                let x = frame.origin_x + x_offset;
                let y = frame.origin_y + y_offset;
                blend_rect(
                    canvas,
                    width,
                    height,
                    (x, y, frame.cell_width, frame.cell_height),
                    themed_bgra(selection_color, 112),
                );
            }
        }
    }
    if let Some((start, end)) = hovered_url {
        if start.0 == end.0 && row_is_dirty(start.0) {
            for column in start.1..=end.1.min(frame.columns.saturating_sub(1) as usize) {
                let (Some(x_offset), Some(row_offset)) = (
                    grid_pixel_offset(column, frame.cell_width),
                    grid_pixel_offset(start.0, frame.cell_height),
                ) else {
                    continue;
                };
                let x = frame.origin_x + x_offset;
                let y = frame.origin_y
                    + row_offset
                    + i32::try_from(frame.cell_height.saturating_sub(2)).unwrap_or(0);
                fill_rect(
                    canvas,
                    width,
                    height,
                    (x, y, frame.cell_width, 2),
                    themed_bgra(url_color, 255),
                );
            }
        }
    }
}

pub(crate) fn paint_snapshot(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    cursor_visible: bool,
    cursor_style: CursorStyle,
) {
    for pixel in canvas.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[
            frame.canvas_background[2],
            frame.canvas_background[1],
            frame.canvas_background[0],
            0xff,
        ]);
    }
    for row in 0..frame.rows {
        for column in 0..frame.columns {
            let index = usize::try_from(row * frame.columns + column).expect("background index");
            let color = frame.backgrounds[index];
            fill_rect(
                canvas,
                width,
                height,
                (
                    frame.origin_x + i32::try_from(column * frame.cell_width).expect("cell x"),
                    frame.origin_y + i32::try_from(row * frame.cell_height).expect("cell y"),
                    frame.cell_width,
                    frame.cell_height,
                ),
                [color[0], color[1], color[2], 0xff],
            );
        }
    }
    for placed in &frame.glyphs {
        let glyph = &frame.cache[&placed.key];
        let span = frame.cell_width.saturating_mul(placed.cells);
        let centered_pen = (u32_to_f32(span) - placed.cluster_advance) / 2.0;
        let pen_x = frame.origin_x
            + i32::try_from(placed.column * frame.cell_width).expect("glyph x")
            + round_to_i32(centered_pen + placed.x_offset);
        let baseline = frame.origin_y
            + i32::try_from(placed.row * frame.cell_height).expect("glyph y")
            + frame.baseline
            - round_to_i32(placed.y_offset);
        blend_glyph(
            canvas,
            width,
            height,
            pen_x + glyph.left,
            baseline - glyph.top,
            glyph,
            placed.foreground,
        );
    }
    if cursor_visible {
        let Some((column, row)) = frame.cursor else {
            return;
        };
        let x = frame.origin_x + i32::try_from(column * frame.cell_width).expect("cursor x");
        let y = frame.origin_y + i32::try_from(row * frame.cell_height).expect("cursor y");
        let color = [
            frame.cursor_color[0],
            frame.cursor_color[1],
            frame.cursor_color[2],
            0xff,
        ];
        paint_cursor(
            canvas,
            width,
            height,
            x,
            y,
            frame.cell_width,
            frame.cell_height,
            color,
            cursor_style,
        );
    }
}

pub(crate) fn paint_snapshot_rows(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    dirty_rows: &[bool],
    cursor_visible: bool,
    cursor_style: CursorStyle,
) {
    for row in 0..frame.rows {
        if !dirty_rows
            .get(usize::try_from(row).expect("row fits usize"))
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        for column in 0..frame.columns {
            let index = usize::try_from(row * frame.columns + column).expect("background index");
            let color = frame.backgrounds[index];
            fill_rect(
                canvas,
                width,
                height,
                (
                    frame.origin_x + i32::try_from(column * frame.cell_width).expect("cell x"),
                    frame.origin_y + i32::try_from(row * frame.cell_height).expect("cell y"),
                    frame.cell_width,
                    frame.cell_height,
                ),
                [color[0], color[1], color[2], 0xff],
            );
        }
    }
    for placed in &frame.glyphs {
        if !dirty_rows
            .get(usize::try_from(placed.row).expect("row fits usize"))
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        let glyph = &frame.cache[&placed.key];
        let span = frame.cell_width.saturating_mul(placed.cells);
        let centered_pen = (u32_to_f32(span) - placed.cluster_advance) / 2.0;
        let pen_x = frame.origin_x
            + i32::try_from(placed.column * frame.cell_width).expect("glyph x")
            + round_to_i32(centered_pen + placed.x_offset);
        let baseline = frame.origin_y
            + i32::try_from(placed.row * frame.cell_height).expect("glyph y")
            + frame.baseline
            - round_to_i32(placed.y_offset);
        blend_glyph(
            canvas,
            width,
            height,
            pen_x + glyph.left,
            baseline - glyph.top,
            glyph,
            placed.foreground,
        );
    }
    if cursor_visible {
        if let Some((column, row)) = frame.cursor.filter(|(_, row)| {
            dirty_rows
                .get(usize::try_from(*row).expect("row fits usize"))
                .copied()
                .unwrap_or(false)
        }) {
            let x = frame.origin_x + i32::try_from(column * frame.cell_width).expect("cursor x");
            let y = frame.origin_y + i32::try_from(row * frame.cell_height).expect("cursor y");
            let color = [
                frame.cursor_color[0],
                frame.cursor_color[1],
                frame.cursor_color[2],
                0xff,
            ];
            paint_cursor(
                canvas,
                width,
                height,
                x,
                y,
                frame.cell_width,
                frame.cell_height,
                color,
                cursor_style,
            );
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "cursor geometry is explicit and allocation-free"
)]
fn paint_cursor(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    cell_width: u32,
    cell_height: u32,
    color: [u8; 4],
    style: CursorStyle,
) {
    match style {
        CursorStyle::Block => fill_rect(
            canvas,
            width,
            height,
            (x, y, cell_width, cell_height),
            [color[0], color[1], color[2], 96],
        ),
        CursorStyle::Beam => fill_rect(canvas, width, height, (x, y, 2, cell_height), color),
        CursorStyle::Underline => fill_rect(
            canvas,
            width,
            height,
            (
                x,
                y + i32::try_from(cell_height.saturating_sub(2)).unwrap_or(0),
                cell_width,
                2,
            ),
            color,
        ),
    }
}

pub(crate) fn scroll_snapshot_pixels(
    canvas: &mut [u8],
    width: u32,
    frame: &SnapshotFrame,
    scroll: TerminalScroll,
) {
    if scroll.rows == 0
        || scroll.start_row >= scroll.end_row
        || scroll.end_row > frame.rows as usize
    {
        return;
    }
    let Some(stride) = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .filter(|stride| *stride != 0)
    else {
        return;
    };
    let canvas_height = canvas.len() / stride;
    let Some(x) = usize::try_from(frame.origin_x.max(0))
        .ok()
        .and_then(|origin| origin.checked_mul(4))
        .filter(|x| *x < stride)
    else {
        return;
    };
    let Some(grid_width) = frame.columns.checked_mul(frame.cell_width) else {
        return;
    };
    let copy_width = usize::try_from(grid_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .map_or(0, |width| width.min(stride - x));
    if copy_width == 0 {
        return;
    }
    let cell_height = usize::try_from(frame.cell_height).expect("cell height");
    let origin_y = usize::try_from(frame.origin_y.max(0)).expect("origin fits usize");
    let Some(start_y) = scroll
        .start_row
        .checked_mul(cell_height)
        .and_then(|offset| origin_y.checked_add(offset))
    else {
        return;
    };
    let Some(end_y) = scroll
        .end_row
        .checked_mul(cell_height)
        .and_then(|offset| origin_y.checked_add(offset))
        .map(|end| end.min(canvas_height))
    else {
        return;
    };
    let Some(shift) = scroll
        .rows
        .min(scroll.end_row - scroll.start_row)
        .checked_mul(cell_height)
    else {
        return;
    };
    if start_y >= end_y || shift >= end_y - start_y {
        return;
    }
    match scroll.direction {
        ScrollDirection::Forward => {
            for y in start_y..end_y - shift {
                let source = (y + shift) * stride + x;
                let destination = y * stride + x;
                canvas.copy_within(source..source + copy_width, destination);
            }
        }
        ScrollDirection::Reverse => {
            for y in (start_y + shift..end_y).rev() {
                let source = (y - shift) * stride + x;
                let destination = y * stride + x;
                canvas.copy_within(source..source + copy_width, destination);
            }
        }
    }
}

pub(crate) fn snapshot_row_rect(frame: &SnapshotFrame, row: usize) -> Option<(i32, i32, i32, i32)> {
    if row >= frame.rows as usize {
        return None;
    }
    Some((
        frame.origin_x,
        frame.origin_y
            + i32::try_from(u32::try_from(row).ok()?.checked_mul(frame.cell_height)?).ok()?,
        i32::try_from(frame.columns * frame.cell_width).ok()?,
        i32::try_from(frame.cell_height).ok()?,
    ))
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

/// Benchmarks the Phase 4 full-grid and one-row damage paths.
///
/// # Errors
///
/// Returns an error for zero samples or when snapshot rendering cannot initialize.
#[allow(
    clippy::too_many_lines,
    reason = "the benchmark keeps all measured scenarios adjacent for comparable output"
)]
pub fn phase4_benchmark_json(samples: usize) -> Result<serde_json::Value> {
    if samples == 0 {
        bail!("benchmark sample count must be positive");
    }
    let mut grids = Vec::new();
    for (columns, rows) in [(80_usize, 24_usize), (240, 80)] {
        let cell = TerminalCell {
            content: "x".into(),
            spacer_remaining: None,
            attributes: CellAttributes {
                bold: false,
                dim: false,
                italic: false,
                underline: false,
                strikethrough: false,
                blink: false,
                conceal: false,
                reverse: false,
                foreground_source: ColorSource::Default,
                foreground: 0,
                background_source: ColorSource::Default,
                background: 0,
            },
        };
        let snapshot = TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision: 1,
            columns,
            rows,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: true,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
            title: "phase4 benchmark".into(),
            visible_rows: vec![
                TerminalRow {
                    linebreak: false,
                    cells: vec![cell; columns],
                };
                rows
            ],
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            exited_code: None,
            exited_signal: None,
        };
        let cold_started = Instant::now();
        let mut frame = SnapshotFrame::load(&snapshot, 1)?;
        let cold_ns = u64::try_from(cold_started.elapsed().as_nanos())
            .context("cold frame duration fits u64")?;
        let width = frame
            .columns
            .checked_mul(frame.cell_width)
            .and_then(|width| width.checked_add(24))
            .context("benchmark width overflow")?;
        let height = frame
            .rows
            .checked_mul(frame.cell_height)
            .and_then(|height| height.checked_add(24))
            .context("benchmark height overflow")?;
        let canvas_len = usize::try_from(
            width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                .context("benchmark canvas overflow")?,
        )
        .context("benchmark canvas fits usize")?;
        let mut canvas = vec![0; canvas_len];
        let mut warm = Vec::with_capacity(samples);
        let mut full = Vec::with_capacity(samples);
        let mut row_prepare = Vec::with_capacity(samples);
        let mut row_damage = Vec::with_capacity(samples);
        let mut dirty = vec![false; rows];
        dirty[rows / 2] = true;
        for _ in 0..samples {
            let started = Instant::now();
            std::hint::black_box(SnapshotFrame::load(&snapshot, 1)?);
            warm.push(u64::try_from(started.elapsed().as_nanos()).context("warm frame duration")?);

            let started = Instant::now();
            frame.refresh_rows(&snapshot, &dirty)?;
            row_prepare.push(
                u64::try_from(started.elapsed().as_nanos()).context("row preparation duration")?,
            );

            let started = Instant::now();
            paint_snapshot(&mut canvas, width, height, &frame, true, CursorStyle::Block);
            std::hint::black_box(&canvas);
            full.push(u64::try_from(started.elapsed().as_nanos()).context("full paint duration")?);

            let started = Instant::now();
            paint_snapshot_rows(
                &mut canvas,
                width,
                height,
                &frame,
                &dirty,
                true,
                CursorStyle::Block,
            );
            std::hint::black_box(&canvas);
            row_damage
                .push(u64::try_from(started.elapsed().as_nanos()).context("row paint duration")?);
        }
        grids.push(serde_json::json!({
            "columns": columns,
            "rows": rows,
            "canvas": { "width": width, "height": height },
            "cold_frame_ns": cold_ns,
            "warm_full_prepare_ns": timing_summary(&mut warm),
            "one_row_prepare_ns": timing_summary(&mut row_prepare),
            "full_paint_ns": timing_summary(&mut full),
            "one_row_paint_ns": timing_summary(&mut row_damage),
        }));
    }
    Ok(serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "samples": samples,
        "grids": grids,
        "glyph_cache": snapshot_cache_metrics(),
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

    fn default_attributes() -> CellAttributes {
        CellAttributes {
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            strikethrough: false,
            blink: false,
            conceal: false,
            reverse: false,
            foreground_source: ColorSource::Default,
            foreground: 0,
            background_source: ColorSource::Default,
            background: 0,
        }
    }

    fn incremental_snapshot() -> TerminalSnapshot {
        let attributes = default_attributes();
        TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision: 1,
            columns: 2,
            rows: 2,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: false,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
            title: "incremental".into(),
            visible_rows: ["ab", "cd"]
                .into_iter()
                .map(|text| TerminalRow {
                    linebreak: false,
                    cells: text
                        .chars()
                        .map(|character| TerminalCell {
                            content: character.to_string(),
                            spacer_remaining: None,
                            attributes,
                        })
                        .collect(),
                })
                .collect(),
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            exited_code: None,
            exited_signal: None,
        }
    }

    #[test]
    fn fractional_scale_frames_map_cells_and_cursor_rectangles() {
        let snapshot = incremental_snapshot();
        for scale_120 in [120_u32, 150, 180, 240] {
            let frame = SnapshotFrame::load_scaled(&snapshot, scale_120).expect("scaled frame");
            assert_eq!(u32::from(frame.scale_120), scale_120);
            let scale = f64::from(scale_120) / 120.0;
            let logical_x = (f64::from(frame.origin_x) + f64::from(frame.cell_width) / 2.0) / scale;
            let logical_y =
                (f64::from(frame.origin_y) + f64::from(frame.cell_height) / 2.0) / scale;
            assert_eq!(frame.cell_at(logical_x, logical_y, scale_120), Some((0, 0)));
            let (_, _, width, height) = frame
                .cursor_rectangle(scale_120)
                .expect("visible cursor rectangle");
            assert!(width > 0 && height > 0);
        }
    }

    #[test]
    fn glyph_cache_entries_are_scale_specific() {
        let snapshot = incremental_snapshot();
        let one = SnapshotFrame::load_scaled(&snapshot, 120).expect("1x frame");
        let fractional = SnapshotFrame::load_scaled(&snapshot, 150).expect("1.25x frame");
        let key = one
            .cache
            .keys()
            .find(|key| fractional.cache.contains_key(key))
            .copied()
            .expect("common glyph key");
        assert!(!Arc::ptr_eq(&one.cache[&key], &fractional.cache[&key]));
    }

    #[test]
    fn empty_overlays_leave_compositor_border_area_untouched() {
        let snapshot = incremental_snapshot();
        let frame = SnapshotFrame::load_scaled(&snapshot, 120).expect("frame");
        let mut focused = vec![0_u8; 200 * 200 * 4];
        let mut unfocused = focused.clone();
        paint_snapshot_overlays(
            &mut focused,
            200,
            200,
            &frame,
            SnapshotOverlays {
                selection: None,
                hovered_url: None,
                dirty_rows: None,
                focused: true,
                selection_color: 0x0035_4a60,
                url_color: 0x0078_beff,
                accent_color: 0x0078_d2ff,
            },
        );
        paint_snapshot_overlays(
            &mut unfocused,
            200,
            200,
            &frame,
            SnapshotOverlays {
                selection: None,
                hovered_url: None,
                dirty_rows: None,
                focused: false,
                selection_color: 0x0035_4a60,
                url_color: 0x0078_beff,
                accent_color: 0x0078_d2ff,
            },
        );
        assert_eq!(&focused[..4], &[0, 0, 0, 0]);
        assert_eq!(focused, unfocused);
    }

    #[test]
    fn incremental_refresh_preserves_unchanged_prepared_rows() {
        let mut snapshot = incremental_snapshot();
        let mut frame = SnapshotFrame::load(&snapshot, 1).expect("initial frame");
        let row_zero_glyphs: Vec<_> = frame
            .glyphs
            .iter()
            .copied()
            .filter(|glyph| glyph.row == 0)
            .collect();
        let row_zero_backgrounds = frame.backgrounds[..snapshot.columns].to_vec();

        snapshot.visible_rows[1].cells[0].content = "z".into();
        snapshot.visible_rows[1].cells[0].attributes.reverse = true;
        frame
            .refresh_rows(&snapshot, &[false, true])
            .expect("refresh damaged row");

        assert_eq!(
            frame
                .glyphs
                .iter()
                .copied()
                .filter(|glyph| glyph.row == 0)
                .collect::<Vec<_>>(),
            row_zero_glyphs
        );
        assert_eq!(
            &frame.backgrounds[..snapshot.columns],
            row_zero_backgrounds.as_slice()
        );
    }

    #[test]
    fn incremental_refresh_prunes_stale_frame_glyphs() {
        let mut snapshot = incremental_snapshot();
        let mut frame = SnapshotFrame::load(&snapshot, 1).expect("initial frame");
        let old_keys: HashSet<_> = frame.cache.keys().copied().collect();
        for row in &mut snapshot.visible_rows {
            for cell in &mut row.cells {
                cell.content = "z".into();
            }
        }
        frame
            .refresh_rows(&snapshot, &[true, true])
            .expect("refresh every row");
        let referenced: HashSet<_> = frame.glyphs.iter().map(|glyph| glyph.key).collect();
        assert_eq!(
            frame.cache.keys().copied().collect::<HashSet<_>>(),
            referenced
        );
        assert!(old_keys.iter().any(|key| !frame.cache.contains_key(key)));
    }

    #[test]
    fn cursor_and_title_changes_do_not_reshape_rows() {
        let mut snapshot = incremental_snapshot();
        let mut frame = SnapshotFrame::load(&snapshot, 1).expect("initial frame");
        let glyphs = frame.glyphs.clone();
        let backgrounds = frame.backgrounds.clone();

        snapshot.cursor_column = 1;
        snapshot.cursor_row = 1;
        snapshot.title = "new title".into();
        frame.refresh_cursor(&snapshot);

        assert_eq!(frame.cursor, Some((1, 1)));
        assert_eq!(frame.glyphs, glyphs);
        assert_eq!(frame.backgrounds, backgrounds);
    }

    #[test]
    fn snapshot_empty_spacer_and_concealed_cells_do_not_render() {
        let attributes = default_attributes();
        let mut cell = splinterm_protocol::TerminalCell {
            content: String::new(),
            spacer_remaining: None,
            attributes,
        };
        assert!(!cell_is_renderable(&cell));
        cell.content = "x".into();
        cell.spacer_remaining = Some(1);
        assert!(!cell_is_renderable(&cell));
        cell.spacer_remaining = None;
        cell.attributes.conceal = true;
        assert!(!cell_is_renderable(&cell));
        cell.attributes.conceal = false;
        assert!(cell_is_renderable(&cell));
    }

    #[test]
    fn snapshot_spacer_run_defines_wide_leader_span() {
        let attributes = default_attributes();
        let cells = vec![
            splinterm_protocol::TerminalCell {
                content: "界".into(),
                spacer_remaining: None,
                attributes,
            },
            splinterm_protocol::TerminalCell {
                content: String::new(),
                spacer_remaining: Some(1),
                attributes,
            },
            splinterm_protocol::TerminalCell {
                content: "x".into(),
                spacer_remaining: None,
                attributes,
            },
        ];
        assert_eq!(leader_span(&cells, 0), 2);
        assert_eq!(leader_span(&cells, 2), 1);
    }

    #[test]
    fn snapshot_colors_cover_rgb_palette_dim_and_reverse() {
        let mut attributes = default_attributes();
        attributes.foreground_source = ColorSource::Rgb;
        attributes.foreground = 0x80_40_20;
        attributes.background_source = ColorSource::Base256;
        attributes.background = 196;
        attributes.dim = true;
        let mut palette = vec![0; 256];
        palette[196] = 0xff_00_00;
        assert_eq!(
            rendition_colors(
                &attributes,
                &palette,
                default_foreground(),
                default_background()
            ),
            ([0x40, 0x20, 0x10], [0xff, 0, 0])
        );
        attributes.reverse = true;
        assert_eq!(
            rendition_colors(
                &attributes,
                &palette,
                default_foreground(),
                default_background()
            ),
            ([0xff, 0, 0], [0x40, 0x20, 0x10])
        );
    }

    #[test]
    fn snapshot_framebuffer_paints_background_wide_composed_glyphs_and_cursor() {
        let key = GlyphKey { face: 0, glyph: 1 };
        let frame = SnapshotFrame {
            glyphs: vec![
                SnapshotGlyph {
                    key,
                    column: 0,
                    row: 0,
                    cells: 2,
                    cluster_advance: 2.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                    foreground: [200, 100, 50],
                },
                SnapshotGlyph {
                    key,
                    column: 0,
                    row: 0,
                    cells: 2,
                    cluster_advance: 2.0,
                    x_offset: -1.0,
                    y_offset: 0.0,
                    foreground: [200, 100, 50],
                },
            ],
            cache: HashMap::from([(
                key,
                Arc::new(CachedGlyph {
                    content: Content::Mask,
                    left: 0,
                    top: 1,
                    width: 1,
                    height: 1,
                    data: vec![0xff],
                }),
            )]),
            backgrounds: vec![[1, 2, 3], [4, 5, 6]],
            columns: 2,
            rows: 1,
            cell_width: 4,
            cell_height: 4,
            baseline: 2,
            origin_x: 2,
            origin_y: 2,
            cursor: Some((1, 0)),
            canvas_background: [14, 18, 22],
            cursor_color: [0xeb, 0xeb, 0xeb],
            scale_120: 120,
        };
        let mut canvas = vec![0; 12 * 8 * 4];
        paint_snapshot(&mut canvas, 12, 8, &frame, true, CursorStyle::Block);
        let pixel = |x: usize, y: usize| &canvas[(y * 12 + x) * 4..(y * 12 + x + 1) * 4];
        assert_eq!(pixel(2, 2), [3, 2, 1, 0xff]);
        assert_eq!(pixel(4, 3), [50, 100, 200, 0xff]);
        assert_eq!(pixel(5, 3), [50, 100, 200, 0xff]);
        assert_eq!(pixel(6, 2), [0xeb, 0xeb, 0xeb, 96]);
    }

    fn damage_test_frame() -> SnapshotFrame {
        SnapshotFrame {
            glyphs: Vec::new(),
            cache: HashMap::new(),
            backgrounds: vec![[1, 0, 0], [2, 0, 0], [3, 0, 0]],
            columns: 1,
            rows: 3,
            cell_width: 2,
            cell_height: 2,
            baseline: 1,
            origin_x: 0,
            origin_y: 0,
            cursor: None,
            canvas_background: [0, 0, 0],
            cursor_color: [255, 255, 255],
            scale_120: 120,
        }
    }

    #[test]
    fn row_damage_paints_only_selected_rows() {
        let frame = damage_test_frame();
        let mut canvas = vec![0; 2 * 6 * 4];
        paint_snapshot_rows(
            &mut canvas,
            2,
            6,
            &frame,
            &[false, true, false],
            false,
            CursorStyle::Block,
        );
        assert_eq!(&canvas[0..4], &[0, 0, 0, 0]);
        assert_eq!(&canvas[2 * 2 * 4..2 * 2 * 4 + 4], &[0, 0, 2, 0xff]);
        assert_eq!(&canvas[4 * 2 * 4..4 * 2 * 4 + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn scroll_damage_copies_existing_grid_pixels() {
        let frame = damage_test_frame();
        let mut canvas = vec![0; 2 * 6 * 4];
        paint_snapshot(&mut canvas, 2, 6, &frame, false, CursorStyle::Block);
        scroll_snapshot_pixels(
            &mut canvas,
            2,
            &frame,
            TerminalScroll {
                direction: ScrollDirection::Forward,
                start_row: 0,
                end_row: 3,
                rows: 1,
            },
        );
        assert_eq!(&canvas[0..4], &[0, 0, 2, 0xff]);
        assert_eq!(&canvas[2 * 2 * 4..2 * 2 * 4 + 4], &[0, 0, 3, 0xff]);
    }

    #[test]
    fn scroll_copy_clips_to_undersized_framebuffers() {
        let frame = damage_test_frame();
        let scroll = TerminalScroll {
            direction: ScrollDirection::Forward,
            start_row: 0,
            end_row: 3,
            rows: 1,
        };
        let mut narrow = vec![0_u8; 12];
        narrow[8..12].copy_from_slice(&[1, 2, 3, 4]);
        scroll_snapshot_pixels(&mut narrow, 1, &frame, scroll);
        assert_eq!(&narrow[..4], &[1, 2, 3, 4]);

        let mut short = vec![0_u8; 8];
        let unchanged = short.clone();
        scroll_snapshot_pixels(&mut short, 2, &frame, scroll);
        assert_eq!(short, unchanged);

        let mut partial_scanline = vec![0_u8; 7];
        scroll_snapshot_pixels(&mut partial_scanline, 2, &frame, scroll);
    }

    #[test]
    fn terminal_size_calculation_clamps_minimum_and_protocol_limits() {
        let frame = SnapshotFrame {
            glyphs: Vec::new(),
            cache: HashMap::new(),
            backgrounds: Vec::new(),
            columns: 0,
            rows: 0,
            cell_width: 10,
            cell_height: 20,
            baseline: 15,
            origin_x: 10,
            origin_y: 10,
            cursor: None,
            canvas_background: [14, 18, 22],
            cursor_color: [0xeb, 0xeb, 0xeb],
            scale_120: 120,
        };
        assert_eq!(
            frame.terminal_size(1_020, 620, 120).expect("normal grid"),
            (100, 30, 1_000, 600)
        );
        assert_eq!(
            frame.terminal_size(1, 1, 120).expect("minimum grid"),
            (2, 2, 20, 40)
        );
        assert_eq!(
            frame
                .terminal_size(20_000, 20_000, 120)
                .expect("bounded grid"),
            (MAX_COLUMNS, MAX_ROWS, 2_400, 1_600)
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
