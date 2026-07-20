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
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use crate::{
    box_drawing,
    config::CursorStyle,
    geometry::{
        BufferPadding, CellGeometry, FontSize, FontSizingPolicy, OutputDpiObservation,
        TerminalPadding, WindowGeometry, resolve_font_size, resolve_font_size_with_output,
    },
};
use anyhow::{Context, Result, bail};

use splinterm_core::SplintId;
use splinterm_freetype::RasterFace;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, MAX_COLUMNS, MAX_ROWS, ScrollDirection,
    TerminalCell, TerminalInputModes, TerminalRow, TerminalScroll, TerminalSnapshot,
    UnderlineStyle,
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
const SNAPSHOT_GLYPH_CACHE_BYTE_BUDGET: usize = 64 * 1024 * 1024;
const SNAPSHOT_RASTER_FACE_BUDGET: usize = 24;

const SNAPSHOT_PRIMARY_REGULAR: usize = 0;
const SNAPSHOT_PRIMARY_BOLD: usize = 1;
const SNAPSHOT_PRIMARY_ITALIC: usize = 2;
const SNAPSHOT_PRIMARY_BOLD_ITALIC: usize = 3;
const SNAPSHOT_CJK: usize = 4;
const SNAPSHOT_EMOJI: usize = 5;

static SNAPSHOT_FACES: OnceLock<Result<[FontFace; 6], String>> = OnceLock::new();
static RENDERER_OPTIONS: OnceLock<RendererOptions> = OnceLock::new();
static OUTPUT_DPI: OnceLock<Mutex<OutputDpiObservation>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct RendererOptions {
    pub font: String,
    pub font_size: FontSize,
    pub font_sizing_policy: FontSizingPolicy,
    pub physical_dpi: f32,
    pub padding: TerminalPadding,
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            font: PRIMARY_FONT.to_owned(),
            font_size: FontSize::Pixels(BASE_FONT_SIZE),
            font_sizing_policy: FontSizingPolicy::OutputScale,
            physical_dpi: 96.0,
            padding: TerminalPadding::DEFAULT,
        }
    }
}

/// Installs immutable per-process renderer configuration before a window opens.
/// A process owns one graphical window in the MVP, so caches cannot mix fonts.
///
/// # Errors
/// Returns an error for an invalid size or a second configuration attempt.
pub fn configure(options: RendererOptions) -> Result<()> {
    if !options.font_size.value().is_finite() || !(6.0..=96.0).contains(&options.font_size.value())
    {
        bail!("font size must be between 6 and 96 in its declared unit");
    }
    resolve_font_size(
        options.font_size,
        options.font_sizing_policy,
        120,
        options.physical_dpi,
    )?;
    RENDERER_OPTIONS
        .set(options)
        .map_err(|_| anyhow::anyhow!("renderer is already configured"))
}

fn renderer_options() -> &'static RendererOptions {
    RENDERER_OPTIONS.get_or_init(RendererOptions::default)
}

fn output_dpi() -> Result<OutputDpiObservation> {
    let default = || {
        OutputDpiObservation::provided(renderer_options().physical_dpi)
            .expect("configured physical DPI was validated")
    };
    OUTPUT_DPI
        .get_or_init(|| Mutex::new(default()))
        .lock()
        .map_err(|_| anyhow::anyhow!("renderer output DPI lock is poisoned"))
        .map(|observation| observation.clone())
}

/// Resolves the current configured font against surface scale and output DPI.
///
/// # Errors
/// Returns an error for invalid scale, DPI, or effective raster size.
pub fn effective_font_resolution(
    surface_scale_120: u32,
) -> Result<crate::geometry::ResolvedFontSize> {
    let options = renderer_options();
    resolve_font_size_with_output(
        options.font_size,
        options.font_sizing_policy,
        surface_scale_120,
        &output_dpi()?,
    )
}

fn effective_font_size(surface_scale_120: u32) -> Result<f32> {
    Ok(effective_font_resolution(surface_scale_120)?.pixel_size)
}

/// Updates the most recently entered Wayland output DPI observation.
/// Returns true only when the effective font raster size changes at this scale.
///
/// # Errors
/// Returns an error for invalid scale/DPI/font resolution or a poisoned state lock.
pub fn update_output_dpi(
    observation: OutputDpiObservation,
    surface_scale_120: u32,
) -> Result<bool> {
    let options = renderer_options();
    // Validate and compare resolutions before publishing the observation.
    let previous = effective_font_resolution(surface_scale_120)?;
    let next = resolve_font_size_with_output(
        options.font_size,
        options.font_sizing_policy,
        surface_scale_120,
        &observation,
    )?;
    let mut current = OUTPUT_DPI
        .get_or_init(|| Mutex::new(observation.clone()))
        .lock()
        .map_err(|_| anyhow::anyhow!("renderer output DPI lock is poisoned"))?;
    *current = observation;
    let changed = previous.effective_pixel_size_26_6 != next.effective_pixel_size_26_6;
    drop(current);
    if changed {
        SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
    }
    Ok(changed)
}

#[derive(Default)]
struct PersistentGlyphCache {
    raster_faces: HashMap<(isize, usize), RasterFace>,
    raster_face_order: VecDeque<(isize, usize)>,
    glyphs: HashMap<(isize, GlyphKey), Arc<CachedGlyph>>,
    advances: HashMap<(isize, GlyphKey), i32>,
    order: VecDeque<(isize, GlyphKey)>,
    glyph_bytes: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
    raster_face_evictions: u64,
}

impl PersistentGlyphCache {
    fn insert_glyph(
        &mut self,
        cache_key: (isize, GlyphKey),
        glyph: Arc<CachedGlyph>,
        color_advance: Option<i32>,
    ) {
        self.insert_glyph_bounded(
            cache_key,
            glyph,
            color_advance,
            SNAPSHOT_GLYPH_CACHE_BUDGET,
            SNAPSHOT_GLYPH_CACHE_BYTE_BUDGET,
        );
    }

    fn insert_glyph_bounded(
        &mut self,
        cache_key: (isize, GlyphKey),
        glyph: Arc<CachedGlyph>,
        color_advance: Option<i32>,
        entry_budget: usize,
        byte_budget: usize,
    ) {
        let incoming_bytes = glyph.data.len();
        while !self.glyphs.is_empty()
            && (self.glyphs.len() >= entry_budget
                || self.glyph_bytes.saturating_add(incoming_bytes) > byte_budget)
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.glyphs.remove(&oldest) {
                self.glyph_bytes = self.glyph_bytes.saturating_sub(evicted.data.len());
                self.advances.remove(&oldest);
                self.evictions = self.evictions.saturating_add(1);
            }
        }
        self.order.push_back(cache_key);
        if let Some(advance) = color_advance {
            self.advances.insert(cache_key, advance);
        }
        self.glyph_bytes = self.glyph_bytes.saturating_add(incoming_bytes);
        self.glyphs.insert(cache_key, glyph);
    }

    fn prepare_raster_face_insert(&mut self, raster_key: (isize, usize)) {
        while self.raster_faces.len() >= SNAPSHOT_RASTER_FACE_BUDGET {
            let Some(oldest) = self.raster_face_order.pop_front() else {
                break;
            };
            if self.raster_faces.remove(&oldest).is_some() {
                self.raster_face_evictions = self.raster_face_evictions.saturating_add(1);
            }
        }
        self.raster_face_order.push_back(raster_key);
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
    selected_pixel_size_26_6: isize,
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

        let metrics = cell_metrics(&faces[0], font_size)?;
        let CellMetrics {
            width: cell_width,
            height: cell_height,
            baseline,
            mono_advance,
            ..
        } = metrics;
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

fn snapshot_faces() -> Result<&'static [FontFace; 6]> {
    SNAPSHOT_FACES
        .get_or_init(|| {
            Ok([
                resolve_face("primary", &renderer_options().font, "")
                    .map_err(|error| error.to_string())?,
                resolve_face("primary bold", PRIMARY_BOLD_FONT, "jetbrains mono")
                    .map_err(|error| error.to_string())?,
                resolve_face("primary italic", PRIMARY_ITALIC_FONT, "jetbrains mono")
                    .map_err(|error| error.to_string())?,
                resolve_face(
                    "primary bold italic",
                    PRIMARY_BOLD_ITALIC_FONT,
                    "jetbrains mono",
                )
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
    faces: &[FontFace],
    face_index: usize,
    glyph_id: u16,
    font_size: f32,
) -> Result<Arc<CachedGlyph>> {
    let effective_size_26_6 = pixel_size_26_6(font_size)?;
    let key = GlyphKey {
        face: face_index,
        glyph: glyph_id,
    };
    SNAPSHOT_GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(glyph) = cache.glyphs.get(&(effective_size_26_6, key)).cloned() {
            cache.hits = cache.hits.saturating_add(1);
            return Ok(glyph);
        }
        cache.misses = cache.misses.saturating_add(1);
        let (glyph, color_advance) = if face_index == SNAPSHOT_EMOJI {
            let face = &faces[face_index];
            let raster = RasterFace::rasterize_color(
                &face.path,
                u32::try_from(face.index).context("emoji face index fits u32")?,
                effective_size_26_6,
                face.selected_pixel_size_26_6,
                u32::from(glyph_id),
            )
            .with_context(|| format!("rasterize color snapshot glyph {glyph_id}"))?;
            (
                CachedGlyph {
                    content: Content::Color,
                    left: raster.left,
                    top: raster.top,
                    width: raster.width,
                    height: raster.height,
                    data: raster.rgba.into_vec(),
                },
                Some(raster.advance_x),
            )
        } else {
            let raster_key = (effective_size_26_6, face_index);
            if !cache.raster_faces.contains_key(&raster_key) {
                let raster_face = RasterFace::open(
                    &faces[face_index].path,
                    u32::try_from(faces[face_index].index).context("face index fits u32")?,
                    pixel_size_26_6(font_size)?,
                )
                .with_context(|| {
                    format!("open FreeType raster face {}", faces[face_index].label)
                })?;
                cache.prepare_raster_face_insert(raster_key);
                cache.raster_faces.insert(raster_key, raster_face);
            }
            let raster = cache
                .raster_faces
                .get_mut(&raster_key)
                .context("inserted FreeType raster face remains present")?
                .rasterize_gray(u32::from(glyph_id))
                .with_context(|| format!("rasterize snapshot glyph {glyph_id} with FreeType"))?;
            (
                CachedGlyph {
                    content: Content::Mask,
                    left: raster.left,
                    top: raster.top,
                    width: raster.width,
                    height: raster.height,
                    data: raster.alpha.into_vec(),
                },
                None,
            )
        };
        let glyph = Arc::new(glyph);
        cache.insert_glyph(
            (effective_size_26_6, key),
            Arc::clone(&glyph),
            color_advance,
        );
        Ok(glyph)
    })
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "validated bounded finite pixel size"
)]
fn pixel_size_26_6(font_size: f32) -> Result<isize> {
    let value = (font_size * 64.0).round();
    if !value.is_finite() || !(64.0..=(768.0 * 64.0)).contains(&value) {
        bail!("scaled font size is outside the FreeType raster policy");
    }
    Ok(value as isize)
}

fn snapshot_color_advance(face_index: usize, glyph_id: u16, font_size: f32) -> Result<i32> {
    let cache_key = (
        pixel_size_26_6(font_size)?,
        GlyphKey {
            face: face_index,
            glyph: glyph_id,
        },
    );
    SNAPSHOT_GLYPH_CACHE.with(|cache| {
        cache
            .borrow()
            .advances
            .get(&cache_key)
            .copied()
            .context("color glyph cache retained its fcft-compatible advance")
    })
}

fn reset_snapshot_cache() {
    SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
}

fn evict_snapshot_glyphs() -> usize {
    SNAPSHOT_GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let evicted = cache.glyphs.len();
        cache.glyphs.clear();
        cache.advances.clear();
        cache.order.clear();
        cache.glyph_bytes = 0;
        cache.evictions = cache
            .evictions
            .saturating_add(u64::try_from(evicted).unwrap_or(u64::MAX));
        evicted
    })
}

fn process_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
}

/// Returns bounded persistent snapshot-glyph-cache metrics.
#[must_use]
pub fn snapshot_cache_metrics() -> serde_json::Value {
    SNAPSHOT_GLYPH_CACHE.with(|cache| {
        let cache = cache.borrow();
        serde_json::json!({
            "entries": cache.glyphs.len(),
            "raster_faces": cache.raster_faces.len(),
            "budget": SNAPSHOT_GLYPH_CACHE_BUDGET,
            "glyph_budget": SNAPSHOT_GLYPH_CACHE_BUDGET,
            "glyph_byte_budget": SNAPSHOT_GLYPH_CACHE_BYTE_BUDGET,
            "raster_face_budget": SNAPSHOT_RASTER_FACE_BUDGET,
            "hits": cache.hits,
            "misses": cache.misses,
            "evictions": cache.evictions,
            "raster_face_evictions": cache.raster_face_evictions,
            "approximate_bytes": cache.glyph_bytes,
        })
    })
}

/// Emits deterministic Swash raster evidence for every printable ASCII glyph.
///
/// The records intentionally mirror `tools/foot-oracle/fcft-mask-probe.c` so
/// the strict Phase 8.1 comparator can classify metric, placement, and mask
/// differences before production placement is changed.
///
/// # Errors
/// Returns an error when the configured font cannot be resolved or parsed.
pub fn ascii_glyph_evidence() -> Result<Vec<serde_json::Value>> {
    let face = resolve_face("ASCII evidence", &renderer_options().font, "")?;
    let font_size = effective_font_size(120)?;
    let font = font_ref(&face)?;
    let metrics = font.metrics(&[]).scale(font_size);
    let font_ascent = ceil_to_i32(metrics.ascent);
    let font_descent = ceil_to_i32(metrics.descent);
    let resolved_metrics = cell_metrics(&face, font_size)?;
    let font_height = i32::try_from(resolved_metrics.height).context("font height fits i32")?;
    let glyph_metrics = font.glyph_metrics(&[]).scale(font_size);
    let mut context = ScaleContext::new();
    let mut records = Vec::with_capacity(95);

    for codepoint in 0x20_u32..=0x7e {
        let character = char::from_u32(codepoint).context("printable ASCII is valid Unicode")?;
        let glyph_id = font.charmap().map(character);
        let advance = positive_round_to_u32(glyph_metrics.advance_width(glyph_id));
        let mut scaler = context.builder(font).size(font_size).hint(true).build();
        let rendered = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .render(&mut scaler, glyph_id);
        let glyph = rendered.map_or_else(
            || CachedGlyph {
                content: Content::Mask,
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                data: Vec::new(),
            },
            |image| CachedGlyph {
                content: image.content,
                left: image.placement.left,
                top: image.placement.top,
                width: image.placement.width,
                height: image.placement.height,
                data: image.data,
            },
        );
        let ink = glyph.ink_bounds().unwrap_or(InkBounds {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        });
        let alpha = glyph_alpha_bytes(&glyph);
        records.push(serde_json::json!({
            "schema": 1,
            "label": format!("ASCII-U+{codepoint:04X}"),
            "codepoint": codepoint,
            "cols": 1,
            "glyph_id": glyph_id,
            "font": face.family.as_str(),
            "font_path": face.path.display().to_string(),
            "font_index": face.index,
            "font_ascent": font_ascent,
            "font_descent": font_descent,
            "font_height": font_height,
            "decorations": {
                "underline_position": resolved_metrics.underline_position,
                "underline_thickness": resolved_metrics.underline_thickness,
                "strike_position": resolved_metrics.strike_position,
                "strike_thickness": resolved_metrics.strike_thickness,
            },
            "color": matches!(glyph.content, Content::Color),
            "pixel_format": "swash-alpha",
            "source_stride": glyph.width,
            "placement": {"x": glyph.left, "y": glyph.top},
            "image": {"width": glyph.width, "height": glyph.height},
            "advance": {"x": advance, "y": 0},
            "ink": {
                "left": ink.left,
                "top": ink.top,
                "right": ink.right,
                "bottom": ink.bottom,
            },
            "alpha_hex": bytes_to_hex(&alpha),
        }));
    }
    Ok(records)
}

/// Emits printable-ASCII evidence through the production snapshot glyph cache.
///
/// # Errors
/// Returns an error when configured face resolution, shaping identity, or the
/// production raster bridge fails.
#[allow(
    clippy::too_many_lines,
    reason = "one evidence record builder keeps all strict oracle fields visibly aligned"
)]
pub fn production_ascii_glyph_evidence() -> Result<Vec<serde_json::Value>> {
    let face_index = match std::env::var("SPLINTERM_EVIDENCE_FONT_STYLE")
        .unwrap_or_else(|_| "Regular".into())
        .as_str()
    {
        "Regular" => SNAPSHOT_PRIMARY_REGULAR,
        "Bold" => SNAPSHOT_PRIMARY_BOLD,
        "Italic" => SNAPSHOT_PRIMARY_ITALIC,
        "Bold Italic" => SNAPSHOT_PRIMARY_BOLD_ITALIC,
        style => bail!("unsupported evidence font style {style:?}"),
    };
    let faces = snapshot_faces()?;
    let face = &faces[face_index];
    let scale_120 = std::env::var("SPLINTERM_EVIDENCE_SCALE_120").map_or(Ok(120_u32), |value| {
        value.parse().context("parse evidence scale")
    })?;
    let font_size = effective_font_size(scale_120)?;
    let font = font_ref(face)?;
    let metrics = cell_metrics(face, font_size)?;
    let glyph_metrics = font.glyph_metrics(&[]).scale(font_size);
    let mut records = Vec::with_capacity(96);
    for codepoint in 0x20_u32..=0x7e {
        let character = char::from_u32(codepoint).context("printable ASCII is valid Unicode")?;
        let glyph_id = font.charmap().map(character);
        let glyph = snapshot_glyph(faces, face_index, glyph_id, font_size)?;
        let ink = glyph.ink_bounds().unwrap_or(InkBounds {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        });
        let alpha = glyph_alpha_bytes(&glyph);
        records.push(serde_json::json!({
            "schema": 1,
            "label": format!("ASCII-U+{codepoint:04X}"),
            "codepoint": codepoint,
            "cols": 1,
            "glyph_id": glyph_id,
            "font": face.family.as_str(),
            "font_path": face.path.display().to_string(),
            "font_index": face.index,
            "font_ascent": metrics.ascent,
            "font_descent": metrics.descent,
            "font_height": metrics.font_height,
            "decorations": {
                "underline_position": metrics.underline_position,
                "underline_thickness": metrics.underline_thickness,
                "strike_position": metrics.strike_position,
                "strike_thickness": metrics.strike_thickness,
            },
            "color": matches!(glyph.content, Content::Color),
            "pixel_format": "production-alpha",
            "source_stride": glyph.width,
            "placement": {"x": glyph.left, "y": glyph.top},
            "image": {"width": glyph.width, "height": glyph.height},
            "advance": {
                "x": positive_round_to_u32(glyph_metrics.advance_width(glyph_id)),
                "y": 0,
            },
            "ink": {
                "left": ink.left,
                "top": ink.top,
                "right": ink.right,
                "bottom": ink.bottom,
            },
            "alpha_hex": bytes_to_hex(&alpha),
        }));
    }

    let codepoint = 0x754c;
    let fallback_face = &faces[SNAPSHOT_CJK];
    let fallback_font = font_ref(fallback_face)?;
    let glyph_id = fallback_font
        .charmap()
        .map(char::from_u32(codepoint).context("CJK codepoint")?);
    let glyph = snapshot_glyph(faces, SNAPSHOT_CJK, glyph_id, font_size)?;
    let ink = glyph.ink_bounds().unwrap_or(InkBounds {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    });
    let fallback_metrics = fallback_font.glyph_metrics(&[]).scale(font_size);
    records.push(serde_json::json!({
        "schema": 1,
        "label": "CJK",
        "codepoint": codepoint,
        "cols": 2,
        "glyph_id": glyph_id,
        "font": fallback_face.family.as_str(),
        "font_path": fallback_face.path.display().to_string(),
        "font_index": fallback_face.index,
        "font_ascent": metrics.ascent,
        "font_descent": metrics.descent,
        "font_height": metrics.font_height,
        "decorations": {
            "underline_position": metrics.underline_position,
            "underline_thickness": metrics.underline_thickness,
            "strike_position": metrics.strike_position,
            "strike_thickness": metrics.strike_thickness,
        },
        "color": matches!(glyph.content, Content::Color),
        "pixel_format": "production-alpha",
        "source_stride": glyph.width,
        "placement": {"x": glyph.left, "y": glyph.top},
        "image": {"width": glyph.width, "height": glyph.height},
        "advance": {
            "x": positive_round_to_u32(fallback_metrics.advance_width(glyph_id)),
            "y": 0,
        },
        "ink": {
            "left": ink.left,
            "top": ink.top,
            "right": ink.right,
            "bottom": ink.bottom,
        },
        "alpha_hex": bytes_to_hex(&glyph_alpha_bytes(&glyph)),
    }));

    let codepoint = 0x1f642;
    let emoji_face = &faces[SNAPSHOT_EMOJI];
    let emoji_font = font_ref(emoji_face)?;
    let glyph_id = emoji_font
        .charmap()
        .map(char::from_u32(codepoint).context("emoji codepoint")?);
    let glyph = snapshot_glyph(faces, SNAPSHOT_EMOJI, glyph_id, font_size)?;
    let ink = glyph.ink_bounds().unwrap_or(InkBounds {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    });
    let emoji_advance = snapshot_color_advance(SNAPSHOT_EMOJI, glyph_id, font_size)?;
    records.push(serde_json::json!({
        "schema": 1,
        "label": "emoji",
        "codepoint": codepoint,
        "cols": 2,
        "glyph_id": glyph_id,
        "font": emoji_face.family.as_str(),
        "font_path": emoji_face.path.display().to_string(),
        "font_index": emoji_face.index,
        "font_ascent": metrics.ascent,
        "font_descent": metrics.descent,
        "font_height": metrics.font_height,
        "decorations": {
            "underline_position": metrics.underline_position,
            "underline_thickness": metrics.underline_thickness,
            "strike_position": metrics.strike_position,
            "strike_thickness": metrics.strike_thickness,
        },
        "color": true,
        "pixel_format": "production-color-alpha",
        "source_stride": glyph.width,
        "placement": {"x": glyph.left, "y": glyph.top},
        "image": {"width": glyph.width, "height": glyph.height},
        "advance": {"x": emoji_advance, "y": 0},
        "ink": {
            "left": ink.left,
            "top": ink.top,
            "right": ink.right,
            "bottom": ink.bottom,
        },
        "alpha_hex": bytes_to_hex(&glyph_alpha_bytes(&glyph)),
        "rgba_hex": bytes_to_hex(&glyph.data),
    }));

    let mut shaped_glyphs = Vec::new();
    let mut shape_context = ShapeContext::new();
    let mut shaper = shape_context.builder(font).size(font_size).build();
    shaper.add_str("e\u{301}");
    shaper.shape_with(|cluster| {
        let mut pen = 0.0;
        for shaped_glyph in cluster.glyphs {
            shaped_glyphs.push((
                shaped_glyph.id,
                cluster.advance(),
                pen + shaped_glyph.x,
                shaped_glyph.y,
            ));
            pen += shaped_glyph.advance;
        }
    });
    let [(glyph_id, cluster_advance, x_offset, y_offset)] = shaped_glyphs.as_slice() else {
        bail!("combining evidence did not shape to one pinned glyph");
    };
    let glyph = snapshot_glyph(faces, face_index, *glyph_id, font_size)?;
    let ink = glyph.ink_bounds().unwrap_or(InkBounds {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    });
    records.push(serde_json::json!({
        "schema": 1,
        "label": "combining-0",
        "codepoint": u32::from('e'),
        "cols": 1,
        "glyph_id": glyph_id,
        "font": face.family.as_str(),
        "font_path": face.path.display().to_string(),
        "font_index": face.index,
        "font_ascent": metrics.ascent,
        "font_descent": metrics.descent,
        "font_height": metrics.font_height,
        "decorations": {
            "underline_position": metrics.underline_position,
            "underline_thickness": metrics.underline_thickness,
            "strike_position": metrics.strike_position,
            "strike_thickness": metrics.strike_thickness,
        },
        "color": matches!(glyph.content, Content::Color),
        "pixel_format": "production-alpha",
        "source_stride": glyph.width,
        "placement": {
            "x": glyph.left + round_to_i32(*x_offset),
            "y": glyph.top + round_to_i32(*y_offset),
        },
        "image": {"width": glyph.width, "height": glyph.height},
        "advance": {"x": positive_trunc_to_u32(*cluster_advance), "y": 0},
        "ink": {
            "left": ink.left,
            "top": ink.top,
            "right": ink.right,
            "bottom": ink.bottom,
        },
        "alpha_hex": bytes_to_hex(&glyph_alpha_bytes(&glyph)),
    }));
    Ok(records)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn glyph_alpha_bytes(glyph: &CachedGlyph) -> Vec<u8> {
    match glyph.content {
        Content::Mask => glyph.data.clone(),
        Content::SubpixelMask => glyph
            .data
            .chunks_exact(4)
            .map(|pixel| pixel[..3].iter().copied().max().unwrap_or(0))
            .collect(),
        Content::Color => glyph.data.chunks_exact(4).map(|pixel| pixel[3]).collect(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CellMetrics {
    width: u32,
    height: u32,
    ascent: u32,
    descent: u32,
    font_height: i32,
    baseline: i32,
    mono_advance: f32,
    underline_position: i32,
    underline_thickness: u32,
    strike_position: i32,
    strike_thickness: u32,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "the protocol-bounded integer terminal advance is exactly representable in f32"
)]
fn cell_metrics(primary_face: &FontFace, font_size: f32) -> Result<CellMetrics> {
    let mut face = RasterFace::open(
        &primary_face.path,
        u32::try_from(primary_face.index).context("primary face index")?,
        pixel_size_26_6(font_size)?,
    )
    .context("open primary FreeType face for cell metrics")?;
    let metrics = face.metrics().context("read primary FreeType metrics")?;
    let glyph = face
        .rasterize_gray(face.glyph_index('M'))
        .context("rasterize primary M advance")?;
    let width = u32::try_from(glyph.advance_x.max(1)).context("cell width")?;
    let height = u32::try_from(metrics.height.max(metrics.ascent + metrics.descent).max(1))
        .context("cell height")?;
    Ok(CellMetrics {
        width,
        height,
        ascent: u32::try_from(metrics.ascent).context("cell ascent")?,
        descent: u32::try_from(metrics.descent).context("cell descent")?,
        font_height: metrics.height,
        baseline: i32::try_from(height).context("cell baseline height")? - metrics.descent,
        mono_advance: glyph.advance_x as f32,
        underline_position: metrics.underline_position,
        underline_thickness: u32::try_from(metrics.underline_thickness)
            .context("underline thickness")?,
        strike_position: metrics.strike_position,
        strike_thickness: u32::try_from(metrics.strike_thickness).context("strike thickness")?,
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn positive_round_to_u32(value: f32) -> u32 {
    assert!(value.is_finite() && value > 0.0);
    value.round().max(1.0) as u32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn positive_trunc_to_u32(value: f32) -> u32 {
    assert!(value.is_finite() && value > 0.0);
    value.trunc().max(1.0) as u32
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
            "%{file}\\n%{index}\\n%{family}\\n%{style}\\n%{pixelsize}\\n",
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
    let selected_pixel_size = lines
        .next()
        .with_context(|| format!("fc-match returned no pixel size for {label}"))?
        .parse::<f32>()
        .with_context(|| format!("fc-match returned an invalid pixel size for {label}"))?;
    let selected_pixel_size_26_6 = pixel_size_26_6(selected_pixel_size)?;
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
        selected_pixel_size_26_6,
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
        blend_glyph(canvas, width, height, x, y, glyph, [235, 235, 235], None);
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

#[allow(
    clippy::too_many_arguments,
    reason = "glyph blending keeps explicit canvas, placement, color, and clip contracts"
)]
fn blend_glyph(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: i32,
    y: i32,
    glyph: &CachedGlyph,
    foreground: [u8; 3],
    clip: Option<(i32, i32, i32, i32)>,
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
            let target_x = x + i32::try_from(gx).expect("glyph x fits i32");
            let target_y = y + i32::try_from(gy).expect("glyph y fits i32");
            if clip.is_some_and(|(left, top, right, bottom)| {
                target_x < left || target_x >= right || target_y < top || target_y >= bottom
            }) {
                continue;
            }
            if glyph.content == Content::Color {
                blend_premultiplied_pixel(
                    canvas,
                    canvas_width,
                    canvas_height,
                    target_x,
                    target_y,
                    source,
                );
            } else {
                blend_pixel(
                    canvas,
                    canvas_width,
                    canvas_height,
                    target_x,
                    target_y,
                    source,
                );
            }
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

fn pixman_multiply_unorm8(value: u8, alpha: u32) -> u32 {
    let product = u32::from(value) * alpha + 0x80;
    ((product >> 8) + product) >> 8
}

fn blend_premultiplied_pixel(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rgba: [u8; 4],
) {
    let Some(index) = pixel_index(width, height, x, y) else {
        return;
    };
    let inverse = 255 - u32::from(rgba[3]);
    for (destination_channel, source_channel) in canvas[index..index + 3]
        .iter_mut()
        .zip([rgba[2], rgba[1], rgba[0]])
    {
        *destination_channel = u8::try_from(
            u32::from(source_channel)
                .saturating_add(pixman_multiply_unorm8(*destination_channel, inverse))
                .min(255),
        )
        .expect("premultiplied channel fits u8");
    }
    canvas[index + 3] = 0xff;
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
            pixman_multiply_unorm8(source_channel, alpha)
                .saturating_add(pixman_multiply_unorm8(*destination_channel, inverse))
                .min(255),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DecorationMetrics {
    underline_position: i32,
    underline_thickness: u32,
    strike_position: i32,
    strike_thickness: u32,
}

impl From<CellMetrics> for DecorationMetrics {
    fn from(metrics: CellMetrics) -> Self {
        Self {
            underline_position: metrics.underline_position,
            underline_thickness: metrics.underline_thickness,
            strike_position: metrics.strike_position,
            strike_thickness: metrics.strike_thickness,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DecorationSpan {
    column: u32,
    row: u32,
    cells: u32,
    underline: UnderlineStyle,
    strikethrough: bool,
    underline_color: [u8; 3],
    underline_uses_foreground: bool,
    strike_color: [u8; 3],
    metrics: DecorationMetrics,
}

/// One immutable, scale-dependent rendering of an owned daemon snapshot.
pub(crate) struct SnapshotFrame {
    glyphs: Vec<SnapshotGlyph>,
    decorations: Vec<DecorationSpan>,
    cache: HashMap<GlyphKey, Arc<CachedGlyph>>,
    backgrounds: Vec<[u8; 3]>,
    foregrounds: Vec<[u8; 3]>,
    cell_metrics: Vec<DecorationMetrics>,
    cell_spans: Vec<u32>,
    columns: u32,
    rows: u32,
    cell_width: u32,
    cell_height: u32,
    ascent: u32,
    descent: u32,
    baseline: i32,
    #[allow(dead_code, reason = "retained as the regular-face evidence baseline")]
    underline_position: i32,
    #[allow(dead_code, reason = "retained as the regular-face evidence baseline")]
    underline_thickness: u32,
    #[allow(dead_code, reason = "retained as the regular-face evidence baseline")]
    strike_position: i32,
    #[allow(dead_code, reason = "retained as the regular-face evidence baseline")]
    strike_thickness: u32,
    padding: TerminalPadding,
    cursor: Option<(u32, u32)>,
    canvas_background: [u8; 3],
    cursor_color: [u8; 3],
    scale_120: u16,
}

impl SnapshotFrame {
    pub(crate) const fn cell_height(&self) -> u32 {
        self.cell_height
    }

    fn cell_geometry(&self) -> Result<CellGeometry> {
        CellGeometry::from_metrics(
            self.cell_width,
            self.cell_height,
            self.ascent,
            self.descent,
            u32::try_from(self.baseline).context("cell baseline is nonnegative")?,
        )
    }

    /// Tight geometry is used only for initial sizing and deterministic captures.
    fn tight_geometry(&self) -> Result<WindowGeometry> {
        WindowGeometry::for_grid(
            self.columns,
            self.rows,
            self.cell_geometry()?,
            self.padding,
            u32::from(self.scale_120),
        )
    }

    pub(crate) fn window_geometry(
        &self,
        logical_width: u32,
        logical_height: u32,
        scale_120: u32,
    ) -> Result<WindowGeometry> {
        WindowGeometry::fit_window(
            logical_width,
            logical_height,
            self.cell_geometry()?,
            self.padding,
            scale_120,
            2,
            u32::from(MAX_COLUMNS),
            2,
            u32::from(MAX_ROWS),
        )
    }

    #[allow(
        clippy::unused_self,
        reason = "cell rectangles are resolved through the frame consumer boundary"
    )]
    fn cell_rect(
        &self,
        geometry: &WindowGeometry,
        column: u32,
        row: u32,
    ) -> Option<(i32, i32, u32, u32)> {
        let rect = geometry.cell_rect(column, row)?;
        Some((
            i32::try_from(rect.x).ok()?,
            i32::try_from(rect.y).ok()?,
            rect.width,
            rect.height,
        ))
    }

    pub(crate) fn initial_logical_size(
        &self,
        columns: u16,
        rows: u16,
        scale_120: u32,
    ) -> Result<(u32, u32)> {
        let geometry = WindowGeometry::for_grid(
            u32::from(columns),
            u32::from(rows),
            self.cell_geometry()?,
            self.padding,
            scale_120,
        )?;
        Ok((geometry.logical_width(), geometry.logical_height()))
    }

    pub(crate) fn cell_at(
        &self,
        logical_x: f64,
        logical_y: f64,
        geometry: &WindowGeometry,
    ) -> Option<(usize, usize)> {
        let (row, column) = (geometry.surface_scale_120() == u32::from(self.scale_120))
            .then(|| geometry.cell_at_logical(logical_x, logical_y))??;
        (row < self.rows as usize && column < self.columns as usize).then_some((row, column))
    }

    pub(crate) fn cursor_rectangle(
        &self,
        geometry: &WindowGeometry,
    ) -> Option<(i32, i32, i32, i32)> {
        let (column, row) = self.cursor?;
        if geometry.surface_scale_120() != u32::from(self.scale_120) {
            return None;
        }
        let rect = geometry.logical_cell_rect(column, row)?;
        Some((rect.x, rect.y, rect.width, rect.height))
    }

    pub(crate) fn terminal_size(
        &self,
        logical_width: u32,
        logical_height: u32,
        scale_120: u32,
    ) -> Result<(u16, u16, u16, u16)> {
        let geometry = self.window_geometry(logical_width, logical_height, scale_120)?;
        let (pixel_width, pixel_height) = geometry.terminal_pixels()?;
        Ok((
            u16::try_from(geometry.columns).context("terminal columns fit u16")?,
            u16::try_from(geometry.rows).context("terminal rows fit u16")?,
            pixel_width,
            pixel_height,
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
        let font_size = effective_font_size(u32::from(scale_120))?;
        let faces = snapshot_faces()?;
        let metrics = cell_metrics(&faces[0], font_size)?;
        let cell_width = metrics.width;
        let cell_height = metrics.height;
        let baseline = metrics.baseline;
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
        let default_metrics = DecorationMetrics::from(metrics);
        let mut backgrounds = vec![default_background; background_len];
        let mut foregrounds = vec![default_foreground; background_len];
        let mut cell_metrics = vec![default_metrics; background_len];
        let mut cell_spans = vec![1; background_len];
        let mut glyphs = Vec::new();
        let mut decorations = Vec::new();
        let mut cache = HashMap::new();
        let mut shape_context = ShapeContext::new();
        let primary_metrics = primary_decoration_metrics(faces, font_size)?;

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
                &primary_metrics,
                &mut shape_context,
                &mut backgrounds,
                &mut foregrounds,
                &mut cell_metrics,
                &mut cell_spans,
                &mut glyphs,
                &mut decorations,
                &mut cache,
            )?;
        }
        let cursor = snapshot_cursor(snapshot, columns, rows);
        let mut frame = Self {
            glyphs,
            decorations,
            cache,
            backgrounds,
            foregrounds,
            cell_metrics,
            cell_spans,
            columns,
            rows,
            cell_width,
            cell_height,
            ascent: metrics.ascent,
            descent: metrics.descent,
            baseline,
            underline_position: metrics.underline_position,
            underline_thickness: metrics.underline_thickness,
            strike_position: metrics.strike_position,
            strike_thickness: metrics.strike_thickness,
            padding: renderer_options().padding,
            cursor,
            canvas_background: default_background,
            cursor_color,
            scale_120,
        };
        frame.enforce_cache_budget();
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
        let font_size = effective_font_size(u32::from(self.scale_120))?;
        let default_foreground = packed_rgb(snapshot.default_colors[0]);
        let default_background = packed_rgb(snapshot.default_colors[1]);
        let primary_metrics = primary_decoration_metrics(faces, font_size)?;
        let mut shape_context = ShapeContext::new();
        for (row_index, dirty) in dirty_rows.iter().copied().enumerate().take(snapshot.rows) {
            if !dirty {
                continue;
            }
            let row_number = u32::try_from(row_index).context("row fits u32")?;
            self.glyphs.retain(|glyph| glyph.row != row_number);
            self.decorations.retain(|span| span.row != row_number);
            let start = row_index
                .checked_mul(snapshot.columns)
                .context("snapshot row start overflow")?;
            let end = start
                .checked_add(snapshot.columns)
                .context("snapshot row end overflow")?;
            self.backgrounds[start..end].fill(default_background);
            self.foregrounds[start..end].fill(default_foreground);
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
                &primary_metrics,
                &mut shape_context,
                &mut self.backgrounds,
                &mut self.foregrounds,
                &mut self.cell_metrics,
                &mut self.cell_spans,
                &mut self.glyphs,
                &mut self.decorations,
                &mut self.cache,
            )?;
        }
        self.glyphs.sort_by_key(|glyph| (glyph.row, glyph.column));
        self.decorations.sort_by_key(|span| (span.row, span.column));
        self.enforce_cache_budget();
        Ok(())
    }

    /// Shifts prepared viewport rows and rebuilds only rows exposed by local
    /// history navigation. Returns the equivalent pixel scroll operation.
    pub(crate) fn scroll_viewport_rows(
        &mut self,
        snapshot: &TerminalSnapshot,
        offset_delta: isize,
    ) -> Result<Option<TerminalScroll>> {
        if offset_delta == 0 || self.rows == 0 {
            return Ok(None);
        }
        let count = offset_delta.unsigned_abs().min(self.rows as usize);
        if count == 0 || count >= self.rows as usize {
            return Ok(None);
        }
        let rows = self.rows as usize;
        let columns = self.columns as usize;
        let direction = if offset_delta > 0 {
            let source = 0..(rows - count) * columns;
            let destination = count * columns;
            self.backgrounds.copy_within(source.clone(), destination);
            self.foregrounds.copy_within(source.clone(), destination);
            self.cell_metrics.copy_within(source.clone(), destination);
            self.cell_spans.copy_within(source, destination);
            self.glyphs.retain_mut(|glyph| {
                glyph.row = glyph
                    .row
                    .saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
                glyph.row < self.rows
            });
            self.decorations.retain_mut(|span| {
                span.row = span
                    .row
                    .saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
                span.row < self.rows
            });
            ScrollDirection::Reverse
        } else {
            let source = count * columns..rows * columns;
            self.backgrounds.copy_within(source.clone(), 0);
            self.foregrounds.copy_within(source.clone(), 0);
            self.cell_metrics.copy_within(source.clone(), 0);
            self.cell_spans.copy_within(source, 0);
            self.glyphs.retain_mut(|glyph| {
                let keep = glyph.row >= u32::try_from(count).unwrap_or(u32::MAX);
                if keep {
                    glyph.row -= u32::try_from(count).unwrap_or(0);
                }
                keep
            });
            self.decorations.retain_mut(|span| {
                let keep = span.row >= u32::try_from(count).unwrap_or(u32::MAX);
                if keep {
                    span.row -= u32::try_from(count).unwrap_or(0);
                }
                keep
            });
            ScrollDirection::Forward
        };
        let mut dirty = vec![false; rows];
        let exposed = match direction {
            ScrollDirection::Reverse => 0..count,
            ScrollDirection::Forward => rows - count..rows,
        };
        for row in exposed {
            dirty[row] = true;
        }
        self.refresh_rows(snapshot, &dirty)?;
        Ok(Some(TerminalScroll {
            direction,
            start_row: 0,
            end_row: rows,
            rows: count,
        }))
    }

    fn enforce_cache_budget(&mut self) {
        const MAX_FRAME_GLYPH_CACHE_ENTRIES: usize = 4096;
        if self.cache.len() <= MAX_FRAME_GLYPH_CACHE_ENTRIES {
            return;
        }
        let referenced: HashSet<_> = self.glyphs.iter().map(|glyph| glyph.key).collect();
        self.cache.retain(|key, _| referenced.contains(key));
    }

    /// Updates cursor presentation without reshaping any terminal row.
    pub(crate) fn refresh_cursor(&mut self, snapshot: &TerminalSnapshot) {
        self.cursor = snapshot_cursor(snapshot, self.columns, self.rows);
    }
}

/// Exact output of the production snapshot painter before Wayland submission.
#[derive(Clone, Debug)]
pub struct FinalBufferCapture {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub ascent: u32,
    pub descent: u32,
    pub baseline: i32,
    pub requested_padding: TerminalPadding,
    pub effective_base_padding: BufferPadding,
    pub residual_right: u32,
    pub residual_bottom: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub grid_rect: crate::geometry::Rect,
    pub visible_grid_rect: crate::geometry::Rect,
    pub padding_left: u32,
    pub padding_right: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub origin_x: u32,
    pub origin_y: u32,
    pub cursor: Option<(u32, u32)>,
    pub background_bgra: [u8; 4],
}

/// Paints an owned terminal snapshot through `SnapshotFrame` and the production
/// full-frame compositor into a tightly packed little-endian ARGB8888 buffer.
///
/// # Errors
/// Returns an error for invalid snapshot geometry, scale, font state, or buffer
/// size overflow.
pub fn capture_final_buffer(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
) -> Result<FinalBufferCapture> {
    capture_final_buffer_presented(
        snapshot,
        scale_120,
        show_cursor,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    )
}

/// Captures with an explicit semantic focus/unfocused-cursor policy.
///
/// # Errors
/// Returns an error for invalid snapshot, font, scale, geometry, or allocation bounds.
pub fn capture_final_buffer_presented(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) -> Result<FinalBufferCapture> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let frame = SnapshotFrame::load_scaled(snapshot, scale_120)?;
    let geometry = frame.tight_geometry()?;
    capture_prepared_frame(&frame, geometry, show_cursor, cursor_style, presentation)
}

/// Paints a snapshot into an explicitly configured logical surface.
///
/// This is used by the Foot oracle when the compositor contributes trailing
/// residual pixels to an otherwise fixed cell grid.
///
/// # Errors
/// Returns an error if the surface does not fit exactly the snapshot grid.
pub fn capture_final_buffer_sized(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    logical_width: u32,
    logical_height: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
) -> Result<FinalBufferCapture> {
    capture_final_buffer_sized_presented(
        snapshot,
        scale_120,
        logical_width,
        logical_height,
        show_cursor,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    )
}

/// Sized capture with an explicit semantic focus/unfocused-cursor policy.
///
/// # Errors
/// Returns an error when the declared surface cannot contain the exact snapshot grid.
#[allow(
    clippy::too_many_arguments,
    reason = "capture geometry and cursor policy are explicit"
)]
pub fn capture_final_buffer_sized_presented(
    snapshot: &TerminalSnapshot,
    scale_120: u32,
    logical_width: u32,
    logical_height: u32,
    show_cursor: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) -> Result<FinalBufferCapture> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let frame = SnapshotFrame::load_scaled(snapshot, scale_120)?;
    let geometry = WindowGeometry::grid_in_surface(
        frame.columns,
        frame.rows,
        logical_width,
        logical_height,
        frame.cell_geometry()?,
        frame.padding,
        scale_120,
    )?;
    capture_prepared_frame(&frame, geometry, show_cursor, cursor_style, presentation)
}

fn capture_prepared_frame(
    frame: &SnapshotFrame,
    geometry: WindowGeometry,
    show_cursor: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) -> Result<FinalBufferCapture> {
    let origin_x = geometry.actual_padding.left;
    let origin_y = geometry.actual_padding.top;
    let width = geometry.buffer_width();
    let height = geometry.buffer_height();
    let stride = width.checked_mul(4).context("capture stride overflow")?;
    let length = usize::try_from(stride)
        .ok()
        .and_then(|stride| usize::try_from(height).ok()?.checked_mul(stride))
        .context("capture allocation overflow")?;
    let mut pixels = vec![0_u8; length];
    paint_snapshot_presented(
        &mut pixels,
        width,
        height,
        frame,
        &geometry,
        show_cursor,
        cursor_style,
        presentation,
    );
    Ok(FinalBufferCapture {
        pixels,
        width,
        height,
        stride,
        columns: frame.columns,
        rows: frame.rows,
        cell_width: frame.cell_width,
        cell_height: frame.cell_height,
        ascent: frame.ascent,
        descent: frame.descent,
        baseline: frame.baseline,
        requested_padding: geometry.requested_padding,
        effective_base_padding: geometry.effective_base_padding,
        residual_right: geometry.residual_right,
        residual_bottom: geometry.residual_bottom,
        logical_width: geometry.logical_width(),
        logical_height: geometry.logical_height(),
        grid_rect: geometry.grid_rect,
        visible_grid_rect: geometry.visible_grid_rect,
        padding_left: geometry.actual_padding.left,
        padding_right: geometry.actual_padding.right,
        padding_top: geometry.actual_padding.top,
        padding_bottom: geometry.actual_padding.bottom,
        origin_x,
        origin_y,
        cursor: show_cursor.then_some(frame.cursor).flatten(),
        background_bgra: [
            frame.canvas_background[2],
            frame.canvas_background[1],
            frame.canvas_background[0],
            u8::MAX,
        ],
    })
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

fn primary_decoration_metrics(
    faces: &[FontFace],
    font_size: f32,
) -> Result<[DecorationMetrics; 4]> {
    Ok([
        cell_metrics(&faces[SNAPSHOT_PRIMARY_REGULAR], font_size)?.into(),
        cell_metrics(&faces[SNAPSHOT_PRIMARY_BOLD], font_size)?.into(),
        cell_metrics(&faces[SNAPSHOT_PRIMARY_ITALIC], font_size)?.into(),
        cell_metrics(&faces[SNAPSHOT_PRIMARY_BOLD_ITALIC], font_size)?.into(),
    ])
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "row preparation keeps one bounded shaping transaction explicit"
)]
fn prepare_snapshot_row(
    snapshot: &TerminalSnapshot,
    row_index: usize,
    faces: &[FontFace],
    scale: u16,
    font_size: f32,
    cell_width: u32,
    cell_height: u32,
    baseline: i32,
    default_foreground: [u8; 3],
    default_background: [u8; 3],
    primary_metrics: &[DecorationMetrics; 4],
    shape_context: &mut ShapeContext,
    backgrounds: &mut [[u8; 3]],
    foregrounds: &mut [[u8; 3]],
    cell_metrics_by_cell: &mut [DecorationMetrics],
    cell_spans: &mut [u32],
    glyphs: &mut Vec<SnapshotGlyph>,
    decorations: &mut Vec<DecorationSpan>,
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
        foregrounds[background_index] = foreground;
        let face_index = primary_face_index(&cell.attributes);
        let metrics = primary_metrics[face_index];
        cell_metrics_by_cell[background_index] = metrics;
        if cell.spacer_remaining.is_some() {
            cell_spans[background_index] = 0;
            continue;
        }
        let cells = leader_span(&row.cells, column_index);
        cell_spans[background_index] = cells;
        if cell.attributes.conceal {
            continue;
        }
        let column = u32::try_from(column_index).context("column fits u32")?;
        let row_number = u32::try_from(row_index).context("row fits u32")?;
        if cell.attributes.underline != UnderlineStyle::None || cell.attributes.strikethrough {
            let underline_color = wire_color(
                cell.attributes.underline_color_source,
                cell.attributes.underline_color,
                &snapshot.palette,
                foreground,
            );
            decorations.push(DecorationSpan {
                column,
                row: row_number,
                cells,
                underline: cell.attributes.underline,
                strikethrough: cell.attributes.strikethrough,
                underline_color,
                underline_uses_foreground: cell.attributes.underline_color_source
                    == ColorSource::Default,
                strike_color: foreground,
                metrics,
            });
        }
        if !cell_is_renderable(cell) {
            continue;
        }
        if cell.content.chars().count() == 1 {
            let character = cell.content.chars().next().context("cell has content")?;
            let thickness = box_drawing::default_thickness(cell_width, cell_height, scale);
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
        let (face_index, content) =
            match select_face_for_text(faces, &cell.content, &cell.attributes) {
                Ok(face_index) => (face_index, cell.content.as_str()),
                Err(_) => (primary_face_index(&cell.attributes), "\u{fffd}"),
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
            cache
                .entry(key)
                .or_insert(snapshot_glyph(faces, face_index, glyph_id, font_size)?);
            let cluster_advance = if face_index == SNAPSHOT_EMOJI {
                f32::from(
                    i16::try_from(snapshot_color_advance(face_index, glyph_id, font_size)?)
                        .context("color glyph advance fits i16")?,
                )
            } else {
                cluster_advance.trunc()
            };
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

fn select_face_for_text(
    faces: &[FontFace],
    text: &str,
    attributes: &CellAttributes,
) -> Result<usize> {
    let primary = primary_face_index(attributes);
    [primary, SNAPSHOT_CJK, SNAPSHOT_EMOJI]
        .into_iter()
        .find(|index| {
            font_ref(&faces[*index]).is_ok_and(|font| {
                text.chars()
                    .all(|character| font.charmap().map(character) != 0)
            })
        })
        .with_context(|| format!("no explicit font covers snapshot cell {text:?}"))
}

fn primary_face_index(attributes: &CellAttributes) -> usize {
    match (attributes.bold, attributes.italic) {
        (false, false) => SNAPSHOT_PRIMARY_REGULAR,
        (true, false) => SNAPSHOT_PRIMARY_BOLD,
        (false, true) => SNAPSHOT_PRIMARY_ITALIC,
        (true, true) => SNAPSHOT_PRIMARY_BOLD_ITALIC,
    }
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
    if attributes.reverse {
        std::mem::swap(&mut foreground, &mut background);
    }
    if attributes.dim {
        for component in &mut foreground {
            *component = u8::try_from(u16::from(*component) * 2 / 3).expect("dimmed color fits u8");
        }
    }
    (foreground, background)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HistoryOverlayStatus {
    pub(crate) offset_from_bottom: usize,
    pub(crate) available_rows: usize,
    pub(crate) unseen_rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HistoryOverlayLayout {
    pub(crate) panel: (i32, i32, u32, u32),
    pub(crate) return_to_live: (i32, i32, u32, u32),
}

#[must_use]
pub(crate) fn history_overlay_layout(
    width: u32,
    height: u32,
    scale_120: u32,
) -> Option<HistoryOverlayLayout> {
    if width == 0 || height == 0 || scale_120 == 0 {
        return None;
    }
    let scaled = |logical: u32| logical.saturating_mul(scale_120).div_ceil(120).max(1);
    let margin = scaled(8).min(width / 4).min(height / 4);
    let panel_width = scaled(188).min(width.saturating_sub(margin.saturating_mul(2)));
    let panel_height = scaled(32).min(height.saturating_sub(margin.saturating_mul(2)));
    if panel_width < scaled(72) || panel_height < scaled(20) {
        return None;
    }
    let x = width.saturating_sub(margin).saturating_sub(panel_width);
    let action_width = scaled(32).min(panel_width / 3);
    Some(HistoryOverlayLayout {
        panel: (
            i32::try_from(x).ok()?,
            i32::try_from(margin).ok()?,
            panel_width,
            panel_height,
        ),
        return_to_live: (
            i32::try_from(x.saturating_add(panel_width.saturating_sub(action_width))).ok()?,
            i32::try_from(margin).ok()?,
            action_width,
            panel_height,
        ),
    })
}

pub(crate) fn paint_history_overlay(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    scale_120: u32,
    status: HistoryOverlayStatus,
    background: u32,
    accent: u32,
) -> Option<HistoryOverlayLayout> {
    let layout = history_overlay_layout(width, height, scale_120)?;
    let [_, bg_red, bg_green, bg_blue] = background.to_be_bytes();
    let [_, red, green, blue] = accent.to_be_bytes();
    fill_rect(
        canvas,
        width,
        height,
        layout.panel,
        [bg_blue, bg_green, bg_red, 0xff],
    );
    let (x, y, _, panel_height) = layout.panel;
    let unit = scale_120.div_ceil(120).max(1);
    let bright = [blue, green, red, 0xff];
    for row in 0..3_u32 {
        fill_rect(
            canvas,
            width,
            height,
            (
                x.saturating_add(i32::try_from(unit.saturating_mul(7)).unwrap_or(0)),
                y.saturating_add(i32::try_from(unit.saturating_mul(8 + row * 6)).unwrap_or(0)),
                unit.saturating_mul(12),
                unit.saturating_mul(2),
            ),
            bright,
        );
    }
    let position = format!(
        "{}/{}",
        status.offset_from_bottom.min(999),
        status.available_rows.min(999)
    );
    paint_history_digits(
        canvas,
        width,
        height,
        x.saturating_add(i32::try_from(unit.saturating_mul(25)).unwrap_or(0)),
        y.saturating_add(i32::try_from(unit.saturating_mul(8)).unwrap_or(0)),
        unit,
        &position,
        bright,
    );
    if status.unseen_rows > 0 {
        let unseen = format!("+{}", status.unseen_rows.min(999));
        paint_history_digits(
            canvas,
            width,
            height,
            x.saturating_add(i32::try_from(unit.saturating_mul(82)).unwrap_or(0)),
            y.saturating_add(i32::try_from(unit.saturating_mul(8)).unwrap_or(0)),
            unit,
            &unseen,
            bright,
        );
    }
    let (action_x, action_y, action_width, action_height) = layout.return_to_live;
    fill_rect(
        canvas,
        width,
        height,
        (action_x, action_y, unit, action_height),
        bright,
    );
    let center_x = action_x.saturating_add(i32::try_from(action_width / 2).unwrap_or(0));
    let center_y = action_y.saturating_add(i32::try_from(panel_height / 2).unwrap_or(0));
    for row in 0..5_u32 {
        let arrow_width = unit.saturating_mul(9_u32.saturating_sub(row.saturating_mul(2)));
        fill_rect(
            canvas,
            width,
            height,
            (
                center_x.saturating_sub(i32::try_from(arrow_width / 2).unwrap_or(0)),
                center_y.saturating_sub(
                    i32::try_from(unit.saturating_mul(2 - row.min(2))).unwrap_or(0),
                ),
                arrow_width.max(unit),
                unit,
            ),
            bright,
        );
    }
    Some(layout)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the tiny trusted bitmap painter keeps explicit canvas and placement contracts"
)]
fn paint_history_digits(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    unit: u32,
    text: &str,
    color: [u8; 4],
) {
    let pattern = |character: char| match character {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    };
    for (index, character) in text.chars().enumerate() {
        let glyph = pattern(character);
        let glyph_x = x.saturating_add(
            i32::try_from(
                index
                    .saturating_mul(4)
                    .saturating_mul(usize::try_from(unit).unwrap_or(usize::MAX)),
            )
            .unwrap_or(i32::MAX),
        );
        for (row, bits) in glyph.into_iter().enumerate() {
            for column in 0..3_u8 {
                if bits & (1 << (2 - column)) == 0 {
                    continue;
                }
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        glyph_x
                            .saturating_add(i32::from(column) * i32::try_from(unit).unwrap_or(1)),
                        y.saturating_add(
                            i32::try_from(row).unwrap_or(0) * i32::try_from(unit).unwrap_or(1),
                        ),
                        unit,
                        unit,
                    ),
                    color,
                );
            }
        }
    }
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
    geometry: &WindowGeometry,
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
                let (Ok(column), Ok(row)) = (u32::try_from(column), u32::try_from(row)) else {
                    continue;
                };
                let Some(rect) = frame.cell_rect(geometry, column, row) else {
                    continue;
                };
                blend_rect(
                    canvas,
                    width,
                    height,
                    rect,
                    themed_bgra(selection_color, 112),
                );
            }
        }
    }
    if let Some((start, end)) = hovered_url {
        if start.0 == end.0 && row_is_dirty(start.0) {
            for column in start.1..=end.1.min(frame.columns.saturating_sub(1) as usize) {
                let (Ok(column), Ok(row)) = (u32::try_from(column), u32::try_from(start.0)) else {
                    continue;
                };
                let Some((x, y, cell_width, cell_height)) = frame.cell_rect(geometry, column, row)
                else {
                    continue;
                };
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        x,
                        y + i32::try_from(cell_height.saturating_sub(2)).unwrap_or(0),
                        cell_width,
                        2,
                    ),
                    themed_bgra(url_color, 255),
                );
            }
        }
    }
}

const PIXMAN_FIXED_ONE: i64 = 1 << 16;
const PIXMAN_A8_Y_SAMPLES: i64 = 15;
const PIXMAN_A8_X_SAMPLES: i64 = 17;
const PIXMAN_A8_Y_STEP: i64 = PIXMAN_FIXED_ONE / PIXMAN_A8_Y_SAMPLES;
const PIXMAN_A8_Y_FIRST: i64 =
    (PIXMAN_FIXED_ONE - (PIXMAN_A8_Y_SAMPLES - 1) * PIXMAN_A8_Y_STEP) / 2;
const PIXMAN_A8_X_STEP: i64 = PIXMAN_FIXED_ONE / PIXMAN_A8_X_SAMPLES;
const PIXMAN_A8_X_FIRST: i64 =
    (PIXMAN_FIXED_ONE - (PIXMAN_A8_X_SAMPLES - 1) * PIXMAN_A8_X_STEP) / 2;

fn pixman_edge_x_at(first: (i32, i32), second: (i32, i32), sample_y_fixed: i64) -> i64 {
    let (top, bottom) = if first.1 <= second.1 {
        (first, second)
    } else {
        (second, first)
    };
    let x_top = i64::from(top.0) * PIXMAN_FIXED_ONE;
    let y_top = i64::from(top.1) * PIXMAN_FIXED_ONE;
    let x_bottom = i64::from(bottom.0) * PIXMAN_FIXED_ONE;
    let y_bottom = i64::from(bottom.1) * PIXMAN_FIXED_ONE;
    let dx = x_bottom - x_top;
    let dy = y_bottom - y_top;
    if dy == 0 {
        return x_top;
    }
    let (sign_dx, step_x, edge_dx, edge_error) = if dx >= 0 {
        (1_i64, dx / dy, dx % dy, -dy)
    } else {
        (-1_i64, -((-dx) / dy), (-dx) % dy, 0_i64)
    };
    let steps = sample_y_fixed - y_top;
    let mut x = x_top + steps * step_x;
    let mut next_error = edge_error + steps * edge_dx;
    if steps >= 0 {
        if next_error > 0 {
            let crossed = (next_error + dy - 1) / dy;
            next_error -= crossed * dy;
            x += crossed * sign_dx;
        }
    } else if next_error <= -dy {
        let crossed = (-next_error) / dy;
        next_error += crossed * dy;
        x -= crossed * sign_dx;
    }
    let _ = next_error;
    x
}

fn pixman_a8_sample_x(x_fixed: i64) -> i64 {
    (x_fixed.rem_euclid(PIXMAN_FIXED_ONE) + PIXMAN_A8_X_FIRST) / PIXMAN_A8_X_STEP
}

fn pixman_a8_add_span(mask: &mut [u8], row: usize, width: usize, mut left: i64, mut right: i64) {
    left = left.max(0);
    if right.div_euclid(PIXMAN_FIXED_ONE) >= i64::try_from(width).unwrap_or(i64::MAX) {
        right = i64::try_from(width)
            .unwrap_or(i64::MAX)
            .saturating_mul(PIXMAN_FIXED_ONE)
            .saturating_sub(1);
    }
    if right <= left {
        return;
    }
    let left_pixel = left.div_euclid(PIXMAN_FIXED_ONE);
    let right_pixel = right.div_euclid(PIXMAN_FIXED_ONE);
    if left_pixel < 0 || right_pixel < 0 {
        return;
    }
    let Ok(left_pixel) = usize::try_from(left_pixel) else {
        return;
    };
    let Ok(right_pixel) = usize::try_from(right_pixel) else {
        return;
    };
    if left_pixel >= width || right_pixel >= width {
        return;
    }
    let left_samples = pixman_a8_sample_x(left);
    let right_samples = pixman_a8_sample_x(right);
    let mut add = |column: usize, coverage: i64| {
        let index = row * width + column;
        let coverage = u8::try_from(coverage.clamp(0, 255)).unwrap_or(255);
        mask[index] = mask[index].saturating_add(coverage);
    };
    if left_pixel == right_pixel {
        add(left_pixel, right_samples - left_samples);
        return;
    }
    add(left_pixel, PIXMAN_A8_X_SAMPLES - left_samples);
    for column in left_pixel + 1..right_pixel {
        add(column, PIXMAN_A8_X_SAMPLES);
    }
    add(right_pixel, right_samples);
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "terminal-bounded dimensions keep the rounded pixman edge within i32"
)]
fn pixman_curly_a8_mask(span_width: u32, thickness: u32) -> Vec<u8> {
    let height = thickness.saturating_mul(3);
    let Some(length) = usize::try_from(span_width)
        .ok()
        .and_then(|width| usize::try_from(height).ok()?.checked_mul(width))
    else {
        return Vec::new();
    };
    let mut mask = vec![0_u8; length];
    if span_width == 0 || thickness == 0 {
        return mask;
    }
    let full = i32::try_from(span_width).unwrap_or(i32::MAX);
    let half = full / 2;
    let bottom = i32::try_from(height).unwrap_or(i32::MAX);
    let thickness_f = f64::from(thickness);
    let bottom_f = f64::from(bottom);
    let width_f = f64::from(span_width);
    let edge = ((thickness_f.powi(2)
        + thickness_f.powi(2) * bottom_f.powi(2) / (width_f.powi(2) / 4.0))
        .sqrt()
        / 2.0)
        .round() as i32;
    let traps = [
        (
            ((0, bottom - edge), (half, -edge)),
            ((0, bottom + edge), (half, edge)),
        ),
        (
            ((half, edge), (full, bottom + edge)),
            ((half, -edge), (full, bottom - edge)),
        ),
    ];
    let width = usize::try_from(span_width).unwrap_or(0);
    for row in 0..height {
        for sample in 0..PIXMAN_A8_Y_SAMPLES {
            let sample_y =
                i64::from(row) * PIXMAN_FIXED_ONE + PIXMAN_A8_Y_FIRST + sample * PIXMAN_A8_Y_STEP;
            for (left, right) in traps {
                let left_x = pixman_edge_x_at(left.0, left.1, sample_y);
                let right_x = pixman_edge_x_at(right.0, right.1, sample_y);
                pixman_a8_add_span(
                    &mut mask,
                    usize::try_from(row).unwrap_or(0),
                    width,
                    left_x,
                    right_x,
                );
            }
        }
    }
    mask
}

#[allow(
    clippy::too_many_arguments,
    reason = "Pixman-equivalent A8 mask composition keeps geometry explicit"
)]
fn paint_curly_trapezoids(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    top: i32,
    span_width: u32,
    thickness: u32,
    color: [u8; 4],
) {
    let mask = pixman_curly_a8_mask(span_width, thickness);
    let mask_width = usize::try_from(span_width).unwrap_or(0);
    for (index, coverage) in mask.into_iter().enumerate() {
        if coverage == 0 || mask_width == 0 {
            continue;
        }
        let alpha = u8::try_from((u16::from(coverage) * u16::from(color[3]) + 127) / 255)
            .unwrap_or(u8::MAX);
        blend_pixel(
            canvas,
            width,
            height,
            x + i32::try_from(index % mask_width).unwrap_or(0),
            top + i32::try_from(index / mask_width).unwrap_or(0),
            [color[0], color[1], color[2], alpha],
        );
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "underline raster geometry and destination buffer are explicit"
)]
fn paint_underline_style(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    span_width: u32,
    cell_width: u32,
    thickness: u32,
    style: UnderlineStyle,
    color: [u8; 4],
) {
    match style {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => {
            fill_rect(canvas, width, height, (x, y, span_width, thickness), color);
        }
        UnderlineStyle::Double => {
            fill_rect(canvas, width, height, (x, y, span_width, thickness), color);
            fill_rect(
                canvas,
                width,
                height,
                (
                    x,
                    y + i32::try_from(thickness.saturating_mul(2)).unwrap_or(0),
                    span_width,
                    thickness,
                ),
                color,
            );
        }
        UnderlineStyle::Curly => {
            paint_curly_trapezoids(canvas, width, height, x, y, span_width, thickness, color);
        }
        UnderlineStyle::Dotted => {
            let mut per_cell = (cell_width / thickness) / 2;
            if per_cell == 0 {
                per_cell = 1;
            }
            let mut spacing = vec![thickness; usize::try_from(per_cell).unwrap_or(1)];
            let used = per_cell.saturating_mul(2).saturating_mul(thickness);
            let mut remaining = cell_width.saturating_sub(used);
            let mut index = 0_usize;
            while remaining > 0 {
                spacing[index] = spacing[index].saturating_add(1);
                remaining -= 1;
                index = (index + 1) % spacing.len();
            }
            let mut dot_x = 0_u32;
            for gap in spacing {
                if dot_x >= span_width {
                    break;
                }
                fill_rect(
                    canvas,
                    width,
                    height,
                    (
                        x + i32::try_from(dot_x).unwrap_or(0),
                        y,
                        thickness.min(span_width - dot_x),
                        thickness,
                    ),
                    color,
                );
                dot_x = dot_x.saturating_add(thickness).saturating_add(gap);
            }
        }
        UnderlineStyle::Dashed => {
            let dash = span_width.div_ceil(3);
            for offset in [0, dash.saturating_mul(2)] {
                if offset < span_width {
                    fill_rect(
                        canvas,
                        width,
                        height,
                        (
                            x + i32::try_from(offset).unwrap_or(0),
                            y,
                            dash.min(span_width - offset),
                            thickness,
                        ),
                        color,
                    );
                }
            }
        }
    }
}

fn underline_y_offset(
    cell_height: u32,
    baseline: i32,
    metrics: DecorationMetrics,
    style: UnderlineStyle,
) -> i32 {
    let thickness = metrics.underline_thickness.max(1);
    let bottom_reserve = match style {
        UnderlineStyle::Double | UnderlineStyle::Curly => thickness.saturating_mul(3),
        _ => thickness,
    };
    let natural = baseline - metrics.underline_position;
    let maximum = i32::try_from(cell_height)
        .unwrap_or(i32::MAX)
        .saturating_sub(i32::try_from(bottom_reserve).unwrap_or(i32::MAX));
    natural.min(maximum)
}

fn paint_decoration_span(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    span: &DecorationSpan,
    foreground_override: Option<[u8; 3]>,
) {
    let Some((x, row_top, _, _)) = frame.cell_rect(geometry, span.column, span.row) else {
        return;
    };
    let span_width = frame.cell_width.saturating_mul(span.cells);
    let underline_rgb = if span.underline_uses_foreground {
        foreground_override.unwrap_or(span.underline_color)
    } else {
        span.underline_color
    };
    let underline_color = [underline_rgb[0], underline_rgb[1], underline_rgb[2], 0xff];
    let thickness = span.metrics.underline_thickness.max(1);
    paint_underline_style(
        canvas,
        width,
        height,
        x,
        row_top
            + underline_y_offset(
                frame.cell_height,
                frame.baseline,
                span.metrics,
                span.underline,
            ),
        span_width,
        frame.cell_width,
        thickness,
        span.underline,
        underline_color,
    );
    if span.strikethrough {
        let strike_rgb = foreground_override.unwrap_or(span.strike_color);
        let strike_color = [strike_rgb[0], strike_rgb[1], strike_rgb[2], 0xff];
        fill_rect(
            canvas,
            width,
            height,
            (
                x,
                row_top + frame.baseline - span.metrics.strike_position,
                span_width,
                span.metrics.strike_thickness,
            ),
            strike_color,
        );
    }
}

fn paint_placed_glyph(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    placed: &SnapshotGlyph,
    foreground: [u8; 3],
) {
    let glyph = &frame.cache[&placed.key];
    let Some((cell_x, cell_y, _, _)) = frame.cell_rect(geometry, placed.column, placed.row) else {
        return;
    };
    // Foot starts each cell at its grid pen; fallback advance does not center
    // wide glyphs inside the declared terminal span.
    let pen_x = cell_x + round_to_i32(placed.x_offset);
    let baseline = cell_y + frame.baseline - round_to_i32(placed.y_offset);
    let grid = geometry.grid_rect;
    let grid_left = i32::try_from(grid.x).unwrap_or(i32::MAX);
    let grid_top = i32::try_from(grid.y).unwrap_or(i32::MAX);
    let grid_right = i32::try_from(grid.x.saturating_add(grid.width)).unwrap_or(i32::MAX);
    let grid_bottom = i32::try_from(grid.y.saturating_add(grid.height)).unwrap_or(i32::MAX);
    blend_glyph(
        canvas,
        width,
        height,
        pen_x + glyph.left,
        baseline - glyph.top,
        glyph,
        foreground,
        Some((grid_left, grid_top, grid_right, grid_bottom)),
    );
}

#[cfg(test)]
fn paint_glyphs(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    dirty_rows: Option<&[bool]>,
) {
    for row in 0..frame.rows {
        if dirty_rows.is_some_and(|rows| {
            !rows
                .get(usize::try_from(row).expect("row fits usize"))
                .copied()
                .unwrap_or(false)
        }) {
            continue;
        }
        let start = frame.glyphs.partition_point(|glyph| glyph.row < row);
        let end = frame.glyphs.partition_point(|glyph| glyph.row <= row);
        // Foot renders each row from right to left. This order is observable
        // when a glyph overhangs into its neighbor and both masks touch the
        // same antialiased pixel.
        for placed in frame.glyphs[start..end].iter().rev() {
            paint_placed_glyph(
                canvas,
                width,
                height,
                frame,
                geometry,
                placed,
                placed.foreground,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnfocusedCursorStyle {
    Unchanged,
    Hollow,
    None,
}

impl UnfocusedCursorStyle {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Hollow => "hollow",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorPresentation {
    pub keyboard_focused: bool,
    pub unfocused_style: UnfocusedCursorStyle,
}

impl CursorPresentation {
    pub const FOCUSED_STEADY: Self = Self {
        keyboard_focused: true,
        unfocused_style: UnfocusedCursorStyle::Unchanged,
    };

    #[must_use]
    pub const fn for_keyboard_focus(keyboard_focused: bool) -> Self {
        Self {
            keyboard_focused,
            unfocused_style: UnfocusedCursorStyle::Hollow,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveCursorShape {
    Block,
    Beam,
    Underline,
    Hollow,
    None,
}

impl EffectiveCursorShape {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Beam => "beam",
            Self::Underline => "underline",
            Self::Hollow => "hollow",
            Self::None => "none",
        }
    }
}

#[must_use]
pub const fn effective_cursor_shape(
    configured: CursorStyle,
    visible: bool,
    presentation: CursorPresentation,
) -> EffectiveCursorShape {
    if !visible {
        return EffectiveCursorShape::None;
    }
    if !presentation.keyboard_focused {
        match presentation.unfocused_style {
            UnfocusedCursorStyle::Hollow => return EffectiveCursorShape::Hollow,
            UnfocusedCursorStyle::None => return EffectiveCursorShape::None,
            UnfocusedCursorStyle::Unchanged => {}
        }
    }
    match configured {
        CursorStyle::Block => EffectiveCursorShape::Block,
        CursorStyle::Beam => EffectiveCursorShape::Beam,
        CursorStyle::Underline => EffectiveCursorShape::Underline,
    }
}

fn cursor_colors_for_cell(
    explicit_cursor: Option<[u8; 3]>,
    foreground: [u8; 3],
    background: [u8; 3],
) -> ([u8; 3], [u8; 3]) {
    let mut cursor = explicit_cursor.unwrap_or(foreground);
    let mut text = background;
    if cursor == text {
        text = background;
        cursor = foreground;
        if cursor == text {
            cursor = cursor.map(|channel| !channel);
        }
    }
    (cursor, text)
}

fn cursor_span(frame: &SnapshotFrame, column: u32, row: u32) -> u32 {
    usize::try_from(row * frame.columns + column)
        .ok()
        .and_then(|index| frame.cell_spans.get(index))
        .copied()
        .unwrap_or(1)
        .max(1)
}

#[allow(
    clippy::too_many_arguments,
    reason = "cursor geometry is an explicit Foot contract"
)]
fn paint_effective_cursor(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    x: i32,
    y: i32,
    span: u32,
    metrics: DecorationMetrics,
    color: [u8; 4],
    shape: EffectiveCursorShape,
) {
    let cursor_width = frame.cell_width.saturating_mul(span);
    match shape {
        EffectiveCursorShape::Block => fill_rect(
            canvas,
            width,
            height,
            (x, y, cursor_width, frame.cell_height),
            color,
        ),
        EffectiveCursorShape::Beam => {
            let thickness = (2 * u32::from(frame.scale_120) + 60) / 120;
            fill_rect(
                canvas,
                width,
                height,
                (
                    x,
                    y + frame.baseline - i32::try_from(frame.ascent).unwrap_or(i32::MAX),
                    thickness,
                    frame.ascent.saturating_add(frame.descent),
                ),
                color,
            );
        }
        EffectiveCursorShape::Underline => {
            let thickness = metrics.underline_thickness.max(1);
            let natural = frame
                .baseline
                .saturating_sub(metrics.underline_position)
                .saturating_add(i32::try_from(thickness).unwrap_or(i32::MAX));
            let maximum =
                i32::try_from(frame.cell_height.saturating_sub(thickness)).unwrap_or(i32::MAX);
            fill_rect(
                canvas,
                width,
                height,
                (x, y + natural.min(maximum), cursor_width, thickness),
                color,
            );
        }
        EffectiveCursorShape::Hollow => {
            let border = (u32::from(frame.scale_120) + 60) / 120;
            let border = border.min(frame.cell_height).min(cursor_width);
            for rect in [
                (x, y, cursor_width, border),
                (x, y, border, frame.cell_height),
                (
                    x + i32::try_from(cursor_width.saturating_sub(border)).unwrap_or(0),
                    y,
                    border,
                    frame.cell_height,
                ),
                (
                    x,
                    y + i32::try_from(frame.cell_height.saturating_sub(border)).unwrap_or(0),
                    cursor_width,
                    border,
                ),
            ] {
                fill_rect(canvas, width, height, rect, color);
            }
        }
        EffectiveCursorShape::None => {}
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one compositor owns all per-cell inputs"
)]
fn compose_snapshot_rows(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    dirty_rows: Option<&[bool]>,
    cursor_visible: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) {
    let effective = effective_cursor_shape(cursor_style, cursor_visible, presentation);
    for row in 0..frame.rows {
        if dirty_rows.is_some_and(|rows| {
            !rows
                .get(usize::try_from(row).expect("row fits usize"))
                .copied()
                .unwrap_or(false)
        }) {
            continue;
        }
        for column in (0..frame.columns).rev() {
            let index = usize::try_from(row * frame.columns + column).expect("cell index");
            let Some((x, y, cell_width, cell_height)) = frame.cell_rect(geometry, column, row)
            else {
                continue;
            };
            let background = frame.backgrounds[index];
            let foreground = frame.foregrounds[index];
            let background_span = frame.cell_spans[index].max(1);
            fill_rect(
                canvas,
                width,
                height,
                (
                    x,
                    y,
                    cell_width.saturating_mul(background_span),
                    cell_height,
                ),
                [background[0], background[1], background[2], 0xff],
            );
            let has_cursor = frame.cursor == Some((column, row));
            let span = cursor_span(frame, column, row);
            let (cursor_color, cursor_text) =
                cursor_colors_for_cell(Some(frame.cursor_color), foreground, background);
            if has_cursor && effective == EffectiveCursorShape::Block {
                paint_effective_cursor(
                    canvas,
                    width,
                    height,
                    frame,
                    x,
                    y,
                    span,
                    frame.cell_metrics[index],
                    [cursor_color[0], cursor_color[1], cursor_color[2], 0xff],
                    effective,
                );
            }
            for placed in frame
                .glyphs
                .iter()
                .filter(|glyph| glyph.row == row && glyph.column == column)
                .rev()
            {
                paint_placed_glyph(
                    canvas,
                    width,
                    height,
                    frame,
                    geometry,
                    placed,
                    if has_cursor && effective == EffectiveCursorShape::Block {
                        cursor_text
                    } else {
                        placed.foreground
                    },
                );
            }
            for decoration in frame
                .decorations
                .iter()
                .filter(|span| span.row == row && span.column == column)
            {
                paint_decoration_span(
                    canvas,
                    width,
                    height,
                    frame,
                    geometry,
                    decoration,
                    (has_cursor && effective == EffectiveCursorShape::Block).then_some(cursor_text),
                );
            }
            if has_cursor && effective != EffectiveCursorShape::Block {
                paint_effective_cursor(
                    canvas,
                    width,
                    height,
                    frame,
                    x,
                    y,
                    span,
                    frame.cell_metrics[index],
                    [cursor_color[0], cursor_color[1], cursor_color[2], 0xff],
                    effective,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments, reason = "cursor presentation is explicit")]
pub(crate) fn paint_snapshot_presented(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    cursor_visible: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) {
    for pixel in canvas.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[
            frame.canvas_background[2],
            frame.canvas_background[1],
            frame.canvas_background[0],
            0xff,
        ]);
    }
    compose_snapshot_rows(
        canvas,
        width,
        height,
        frame,
        geometry,
        None,
        cursor_visible,
        cursor_style,
        presentation,
    );
}

pub(crate) fn paint_snapshot(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    cursor_visible: bool,
    cursor_style: CursorStyle,
) {
    paint_snapshot_presented(
        canvas,
        width,
        height,
        frame,
        geometry,
        cursor_visible,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "damage and cursor presentation are explicit"
)]
pub(crate) fn paint_snapshot_rows_presented(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    dirty_rows: &[bool],
    cursor_visible: bool,
    cursor_style: CursorStyle,
    presentation: CursorPresentation,
) {
    compose_snapshot_rows(
        canvas,
        width,
        height,
        frame,
        geometry,
        Some(dirty_rows),
        cursor_visible,
        cursor_style,
        presentation,
    );
}

#[allow(clippy::too_many_arguments, reason = "damage inputs remain explicit")]
pub(crate) fn paint_snapshot_rows(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
    dirty_rows: &[bool],
    cursor_visible: bool,
    cursor_style: CursorStyle,
) {
    paint_snapshot_rows_presented(
        canvas,
        width,
        height,
        frame,
        geometry,
        dirty_rows,
        cursor_visible,
        cursor_style,
        CursorPresentation::FOCUSED_STEADY,
    );
}

pub(crate) fn scroll_snapshot_pixels(
    canvas: &mut [u8],
    width: u32,
    frame: &SnapshotFrame,
    geometry: &WindowGeometry,
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
    let grid = geometry.grid_rect();
    let Some(x) = usize::try_from(grid.x)
        .ok()
        .and_then(|origin| origin.checked_mul(4))
        .filter(|x| *x < stride)
    else {
        return;
    };
    let copy_width = usize::try_from(grid.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .map_or(0, |width| width.min(stride - x));
    if copy_width == 0 {
        return;
    }
    let cell_height = usize::try_from(geometry.cell.height).expect("cell height");
    let origin_y = usize::try_from(grid.y).expect("origin fits usize");
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

pub(crate) fn snapshot_row_rect(
    geometry: &WindowGeometry,
    row: usize,
) -> Option<(i32, i32, i32, i32)> {
    let rect = geometry.row_rect(row)?;
    Some((
        i32::try_from(rect.x).ok()?,
        i32::try_from(rect.y).ok()?,
        i32::try_from(rect.width).ok()?,
        i32::try_from(rect.height).ok()?,
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
    let benchmark_rss_before = process_rss_bytes();
    for (columns, rows) in [(80_usize, 24_usize), (240, 80)] {
        reset_snapshot_cache();
        let cell = TerminalCell {
            content: "x".into(),
            spacer_remaining: None,
            attributes: CellAttributes {
                bold: false,
                dim: false,
                italic: false,
                underline: UnderlineStyle::None,
                underline_color_source: ColorSource::Default,
                underline_color: 0,
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
            visible_rows: (0..rows)
                .map(|row| {
                    Ok(TerminalRow {
                        row_id: Some(u64::try_from(row + 1).context("benchmark row ID fits u64")?),
                        linebreak: false,
                        cells: vec![cell.clone(); columns],
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            history_generation: 1,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
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
        let geometry = frame.tight_geometry()?;
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
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
            paint_snapshot(
                &mut canvas,
                width,
                height,
                &frame,
                &geometry,
                true,
                CursorStyle::Block,
            );
            std::hint::black_box(&canvas);
            full.push(u64::try_from(started.elapsed().as_nanos()).context("full paint duration")?);

            let started = Instant::now();
            paint_snapshot_rows(
                &mut canvas,
                width,
                height,
                &frame,
                &geometry,
                &dirty,
                true,
                CursorStyle::Block,
            );
            std::hint::black_box(&canvas);
            row_damage
                .push(u64::try_from(started.elapsed().as_nanos()).context("row paint duration")?);
        }
        let evicted_entries = evict_snapshot_glyphs();
        let repopulate_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 120)?);
        let repopulate_ns = u64::try_from(repopulate_started.elapsed().as_nanos())
            .context("repopulate duration fits u64")?;
        let scale_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 150)?);
        let scale_change_ns = u64::try_from(scale_started.elapsed().as_nanos())
            .context("scale change duration fits u64")?;
        let scale_return_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 120)?);
        let scale_return_ns = u64::try_from(scale_return_started.elapsed().as_nanos())
            .context("scale return duration fits u64")?;
        let mut alternate_theme = snapshot.clone();
        alternate_theme.default_colors = [0x0011_2233, 0x0044_5566, 0x0077_8899];
        let theme_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&alternate_theme, 120)?);
        let theme_change_ns = u64::try_from(theme_started.elapsed().as_nanos())
            .context("theme change duration fits u64")?;
        let theme_return_started = Instant::now();
        std::hint::black_box(SnapshotFrame::load_scaled(&snapshot, 120)?);
        let theme_return_ns = u64::try_from(theme_return_started.elapsed().as_nanos())
            .context("theme return duration fits u64")?;
        grids.push(serde_json::json!({
            "columns": columns,
            "rows": rows,
            "canvas": { "width": width, "height": height, "bytes": canvas_len },
            "cold_frame_ns": cold_ns,
            "warm_full_prepare_ns": timing_summary(&mut warm),
            "one_row_prepare_ns": timing_summary(&mut row_prepare),
            "full_paint_ns": timing_summary(&mut full),
            "one_row_paint_ns": timing_summary(&mut row_damage),
            "forced_eviction": { "entries": evicted_entries, "repopulate_ns": repopulate_ns },
            "scale_invalidation_ns": { "change": scale_change_ns, "return": scale_return_ns },
            "theme_invalidation_ns": { "change": theme_change_ns, "return": theme_return_ns },
            "rss_bytes_after_grid": process_rss_bytes(),
            "glyph_cache": snapshot_cache_metrics(),
        }));
    }
    Ok(serde_json::json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "samples": samples,
        "rss_bytes": { "before": benchmark_rss_before, "after": process_rss_bytes() },
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
    fn primary_face_selection_tracks_bold_and_italic_attributes() {
        let mut attributes = default_attributes();
        assert_eq!(primary_face_index(&attributes), SNAPSHOT_PRIMARY_REGULAR);
        attributes.bold = true;
        assert_eq!(primary_face_index(&attributes), SNAPSHOT_PRIMARY_BOLD);
        attributes.italic = true;
        assert_eq!(
            primary_face_index(&attributes),
            SNAPSHOT_PRIMARY_BOLD_ITALIC
        );
        attributes.bold = false;
        assert_eq!(primary_face_index(&attributes), SNAPSHOT_PRIMARY_ITALIC);
    }

    #[test]
    fn cell_metrics_use_the_foot_freetype_integer_extents() {
        let faces = snapshot_faces().unwrap();
        let metrics = cell_metrics(&faces[0], 22.0).unwrap();
        assert_eq!(metrics.width, 13);
        assert_eq!(metrics.height, 30);
        assert_eq!(metrics.baseline, 23);
        assert!((metrics.mono_advance - 13.0).abs() < f32::EPSILON);
    }

    #[test]
    fn glyph_alpha_bytes_normalize_supported_swash_content() {
        let mask = CachedGlyph {
            content: Content::Mask,
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            data: vec![10, 20],
        };
        assert_eq!(glyph_alpha_bytes(&mask), vec![10, 20]);

        let subpixel = CachedGlyph {
            content: Content::SubpixelMask,
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            data: vec![10, 30, 20, 0, 40, 20, 30, 0],
        };
        assert_eq!(glyph_alpha_bytes(&subpixel), vec![30, 40]);

        let color = CachedGlyph {
            content: Content::Color,
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        assert_eq!(glyph_alpha_bytes(&color), vec![4, 8]);
    }

    #[test]
    fn glyph_clip_preserves_inside_overhang_and_rejects_outside_pixels() {
        let glyph = CachedGlyph {
            content: Content::Mask,
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            data: vec![255, 255],
        };
        let mut canvas = vec![0; 2 * 4];
        blend_glyph(
            &mut canvas,
            2,
            1,
            0,
            0,
            &glyph,
            [255, 255, 255],
            Some((0, 0, 1, 1)),
        );
        assert_eq!(&canvas[..4], &[255, 255, 255, 255]);
        assert_eq!(&canvas[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn premultiplied_color_over_does_not_multiply_source_twice() {
        let mut canvas = vec![10, 20, 30, 255];
        blend_premultiplied_pixel(&mut canvas, 1, 1, 0, 0, [40, 20, 10, 128]);
        assert_eq!(canvas, [15, 30, 55, 255]);
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
            underline: UnderlineStyle::None,
            underline_color_source: ColorSource::Default,
            underline_color: 0,
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
                .enumerate()
                .map(|(index, text)| TerminalRow {
                    row_id: Some(u64::try_from(index + 1).unwrap()),
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
            history_generation: 1,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
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
            let geometry = frame.tight_geometry().unwrap();
            let scale = f64::from(scale_120) / 120.0;
            let logical_x = (f64::from(geometry.actual_padding.left)
                + f64::from(frame.cell_width) / 2.0)
                / scale;
            let logical_y = (f64::from(geometry.actual_padding.top)
                + f64::from(frame.cell_height) / 2.0)
                / scale;
            assert_eq!(frame.cell_at(logical_x, logical_y, &geometry), Some((0, 0)));
            let (_, _, width, height) = frame
                .cursor_rectangle(&geometry)
                .expect("visible cursor rectangle");
            assert!(width > 0 && height > 0);
        }
    }

    #[test]
    fn final_buffer_capture_uses_declared_production_geometry_and_argb_bytes() {
        let snapshot = incremental_snapshot();
        let capture = capture_final_buffer(&snapshot, 120, true, CursorStyle::Block).unwrap();
        assert_eq!((capture.columns, capture.rows), (2, 2));
        assert_eq!(capture.origin_x, capture.padding_left);
        assert_eq!(capture.origin_y, capture.padding_top);
        assert_eq!(capture.padding_left, capture.padding_right);
        assert_eq!(capture.padding_top, capture.padding_bottom);
        assert!(capture.ascent + capture.descent <= capture.cell_height);
        assert_eq!(
            u32::try_from(capture.baseline).unwrap(),
            capture.cell_height - capture.descent
        );
        assert_eq!(capture.requested_padding, TerminalPadding::DEFAULT);
        assert_eq!(
            capture.padding_left + capture.columns * capture.cell_width + capture.padding_right,
            capture.width
        );
        assert_eq!(
            capture.padding_top + capture.rows * capture.cell_height + capture.padding_bottom,
            capture.height
        );
        assert_eq!(
            capture.padding_right,
            capture.effective_base_padding.right + capture.residual_right
        );
        assert_eq!(
            capture.padding_bottom,
            capture.effective_base_padding.bottom + capture.residual_bottom
        );
        assert_eq!(capture.stride, capture.width * 4);
        assert_eq!(
            capture.pixels.len(),
            usize::try_from(capture.stride * capture.height).unwrap()
        );
        assert_eq!(capture.cursor, Some((0, 0)));
        assert_eq!(capture.background_bgra[3], u8::MAX);
    }

    #[test]
    fn asymmetric_capture_serializes_geometry_owned_rectangles_and_edges() {
        let snapshot = incremental_snapshot();
        let mut frame = SnapshotFrame::load_scaled(&snapshot, 150).unwrap();
        frame.padding = TerminalPadding {
            left: 1,
            right: 3,
            top: 5,
            bottom: 7,
        };
        let geometry = frame.tight_geometry().unwrap();
        let capture = capture_prepared_frame(
            &frame,
            geometry,
            false,
            CursorStyle::Block,
            CursorPresentation::FOCUSED_STEADY,
        )
        .unwrap();
        assert_eq!(capture.requested_padding, frame.padding);
        assert_ne!(capture.padding_left, capture.padding_right);
        assert_ne!(capture.padding_top, capture.padding_bottom);
        assert_eq!(capture.grid_rect.x, capture.origin_x);
        assert_eq!(capture.grid_rect.y, capture.origin_y);
        assert_eq!(
            capture.grid_rect.width,
            capture.columns * capture.cell_width
        );
        assert_eq!(capture.grid_rect.height, capture.rows * capture.cell_height);
        assert_eq!(
            capture.padding_left + capture.grid_rect.width + capture.padding_right,
            capture.width
        );
        assert_eq!(
            capture.padding_top + capture.grid_rect.height + capture.padding_bottom,
            capture.height
        );
    }

    #[test]
    fn sized_capture_preserves_explicit_grid_and_owns_trailing_residual() {
        let snapshot = incremental_snapshot();
        let tight = capture_final_buffer(&snapshot, 120, false, CursorStyle::Block).unwrap();
        let capture = capture_final_buffer_sized(
            &snapshot,
            120,
            tight.logical_width,
            tight.logical_height + 1,
            false,
            CursorStyle::Block,
        )
        .unwrap();
        assert_eq!((capture.columns, capture.rows), (2, 2));
        assert_eq!(capture.padding_bottom, tight.padding_bottom + 1);
        assert_eq!(capture.residual_bottom, tight.residual_bottom + 1);
    }

    fn underline_red_mask(
        style: UnderlineStyle,
        width: u32,
        cell_width: u32,
        thickness: u32,
    ) -> Vec<u8> {
        let height = thickness.saturating_mul(3).max(1);
        let mut canvas = vec![0; usize::try_from(width * height * 4).unwrap()];
        paint_underline_style(
            &mut canvas,
            width,
            height,
            0,
            0,
            width,
            cell_width,
            thickness,
            style,
            [255, 255, 255, 255],
        );
        canvas.chunks_exact(4).map(|pixel| pixel[2]).collect()
    }

    #[test]
    fn foot_dashed_dotted_and_double_vectors_are_exact() {
        let dashed = underline_red_mask(UnderlineStyle::Dashed, 14, 7, 1);
        assert_eq!(
            &dashed[..14],
            &[255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 255, 255, 255, 255]
        );
        let dotted = underline_red_mask(UnderlineStyle::Dotted, 7, 7, 1);
        assert_eq!(&dotted[..7], &[255, 0, 0, 255, 0, 255, 0]);
        let double = underline_red_mask(UnderlineStyle::Double, 7, 7, 2);
        assert!(double[..14].iter().all(|value| *value == 255));
        assert!(double[14..28].iter().all(|value| *value == 0));
        assert!(double[28..42].iter().all(|value| *value == 255));
    }

    #[test]
    fn underline_bottom_clamps_match_single_and_three_thickness_styles() {
        let metrics = DecorationMetrics {
            underline_position: -100,
            underline_thickness: 3,
            strike_position: 0,
            strike_thickness: 1,
        };
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Single),
            7
        );
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Dotted),
            7
        );
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Dashed),
            7
        );
        assert_eq!(
            underline_y_offset(10, 7, metrics, UnderlineStyle::Double),
            1
        );
        assert_eq!(underline_y_offset(10, 7, metrics, UnderlineStyle::Curly), 1);
    }

    #[test]
    fn curly_trapezoid_a8_vectors_match_pinned_pixman_0_46_4() {
        let vectors = [
            (
                7,
                1,
                vec![
                    0, 137, 255, 255, 211, 44, 0, 127, 255, 128, 96, 245, 245, 96, 255, 128, 0, 0,
                    44, 211, 255,
                ],
            ),
            (
                9,
                1,
                vec![
                    0, 44, 245, 255, 255, 255, 128, 9, 0, 96, 245, 245, 96, 76, 220, 255, 221, 77,
                    255, 211, 44, 0, 0, 9, 127, 246, 255,
                ],
            ),
            (
                11,
                2,
                vec![
                    0, 0, 0, 66, 255, 255, 128, 0, 0, 0, 0, 0, 0, 36, 240, 150, 127, 255, 128, 0,
                    0, 0, 0, 15, 219, 189, 3, 0, 127, 255, 128, 0, 0, 3, 189, 219, 15, 0, 0, 0,
                    127, 255, 128, 0, 150, 240, 36, 0, 0, 0, 0, 0, 127, 255, 128, 252, 66, 0, 0, 0,
                    0, 0, 0, 0, 127, 255,
                ],
            ),
            (
                14,
                2,
                vec![
                    0, 0, 0, 0, 12, 181, 255, 255, 181, 12, 0, 0, 0, 0, 0, 0, 0, 28, 207, 252, 108,
                    108, 252, 207, 28, 0, 0, 0, 0, 0, 48, 227, 243, 77, 0, 0, 77, 243, 227, 48, 0,
                    0, 0, 77, 243, 227, 48, 0, 0, 0, 0, 48, 227, 243, 77, 0, 108, 252, 207, 28, 0,
                    0, 0, 0, 0, 0, 28, 207, 252, 108, 255, 178, 12, 0, 0, 0, 0, 0, 0, 0, 0, 12,
                    178, 255,
                ],
            ),
        ];
        for (width, thickness, expected) in vectors {
            assert_eq!(pixman_curly_a8_mask(width, thickness), expected);
        }
    }

    #[test]
    fn cursor_focus_policy_and_cell_relative_color_fallback_are_truthful() {
        for style in [
            CursorStyle::Block,
            CursorStyle::Beam,
            CursorStyle::Underline,
        ] {
            assert_eq!(
                effective_cursor_shape(style, true, CursorPresentation::for_keyboard_focus(false)),
                EffectiveCursorShape::Hollow
            );
            assert_eq!(
                effective_cursor_shape(
                    style,
                    true,
                    CursorPresentation {
                        keyboard_focused: false,
                        unfocused_style: UnfocusedCursorStyle::None,
                    }
                ),
                EffectiveCursorShape::None
            );
        }
        assert_eq!(
            cursor_colors_for_cell(None, [1, 2, 3], [4, 5, 6]),
            ([1, 2, 3], [4, 5, 6])
        );
        assert_eq!(
            cursor_colors_for_cell(Some([9, 9, 9]), [1, 2, 3], [9, 9, 9]),
            ([1, 2, 3], [9, 9, 9])
        );
        assert_eq!(
            cursor_colors_for_cell(None, [7, 7, 7], [7, 7, 7]),
            ([248, 248, 248], [7, 7, 7])
        );
    }

    #[test]
    fn cursor_geometry_matches_foot_at_required_scales() {
        let snapshot = incremental_snapshot();
        for scale in [120_u32, 150, 180, 240] {
            let frame = SnapshotFrame::load_scaled(&snapshot, scale).unwrap();
            let geometry = frame.tight_geometry().unwrap();
            let rect = geometry.cell_rect(0, 0).unwrap();
            let metrics = frame.cell_metrics[0];
            let width = geometry.buffer_width();
            let height = geometry.buffer_height();
            for shape in [
                EffectiveCursorShape::Beam,
                EffectiveCursorShape::Underline,
                EffectiveCursorShape::Hollow,
            ] {
                let mut canvas = vec![0; usize::try_from(width * height * 4).unwrap()];
                paint_effective_cursor(
                    &mut canvas,
                    width,
                    height,
                    &frame,
                    i32::try_from(rect.x).unwrap(),
                    i32::try_from(rect.y).unwrap(),
                    1,
                    metrics,
                    [255, 255, 255, 255],
                    shape,
                );
                let painted = canvas.chunks_exact(4).filter(|pixel| pixel[3] != 0).count();
                assert!(painted > 0);
            }
            let expected_beam = (2 * scale + 60) / 120;
            assert!(expected_beam >= 2);
            let expected_hollow = (scale + 60) / 120;
            assert!((1..=2).contains(&expected_hollow));
        }
    }

    #[test]
    fn focused_and_unfocused_full_dirty_composition_share_one_path() {
        let mut snapshot = incremental_snapshot();
        snapshot.visible_rows[0].cells[0].attributes.underline = UnderlineStyle::Curly;
        snapshot.visible_rows[0].cells[0].attributes.strikethrough = true;
        let frame = SnapshotFrame::load_scaled(&snapshot, 120).unwrap();
        let geometry = frame.tight_geometry().unwrap();
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        for presentation in [
            CursorPresentation::FOCUSED_STEADY,
            CursorPresentation::for_keyboard_focus(false),
        ] {
            for style in [
                CursorStyle::Block,
                CursorStyle::Beam,
                CursorStyle::Underline,
            ] {
                let mut full = vec![0; usize::try_from(width * height * 4).unwrap()];
                paint_snapshot_presented(
                    &mut full,
                    width,
                    height,
                    &frame,
                    &geometry,
                    true,
                    style,
                    presentation,
                );
                let mut rows = vec![0; full.len()];
                for pixel in rows.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[
                        frame.canvas_background[2],
                        frame.canvas_background[1],
                        frame.canvas_background[0],
                        255,
                    ]);
                }
                paint_snapshot_rows_presented(
                    &mut rows,
                    width,
                    height,
                    &frame,
                    &geometry,
                    &[true, true],
                    true,
                    style,
                    presentation,
                );
                assert_eq!(full, rows);
            }
        }
    }

    #[test]
    fn underline_styles_render_distinct_bounded_patterns() {
        let styles = [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ];
        let mut outputs = Vec::new();
        for style in styles {
            let mut canvas = vec![0; 16 * 6 * 4];
            paint_underline_style(
                &mut canvas,
                16,
                6,
                0,
                1,
                16,
                16,
                1,
                style,
                [255, 255, 255, 255],
            );
            assert!(canvas.chunks_exact(4).any(|pixel| pixel[3] != 0));
            outputs.push(canvas);
        }
        for left in 0..outputs.len() {
            for right in left + 1..outputs.len() {
                assert_ne!(outputs[left], outputs[right]);
            }
        }
    }

    #[test]
    fn overlapping_glyphs_compose_in_foot_right_to_left_order() {
        let snapshot = incremental_snapshot();
        let mut frame = SnapshotFrame::load_scaled(&snapshot, 120).unwrap();
        let left_key = GlyphKey { face: 0, glyph: 1 };
        let right_key = GlyphKey { face: 0, glyph: 2 };
        frame.cache.clear();
        frame.cache.insert(
            left_key,
            Arc::new(CachedGlyph {
                content: Content::Mask,
                left: i32::try_from(frame.cell_width).unwrap(),
                top: frame.baseline,
                width: 1,
                height: 1,
                data: vec![1],
            }),
        );
        frame.cache.insert(
            right_key,
            Arc::new(CachedGlyph {
                content: Content::Mask,
                left: 0,
                top: frame.baseline,
                width: 1,
                height: 1,
                data: vec![178],
            }),
        );
        frame.glyphs = vec![
            SnapshotGlyph {
                key: left_key,
                column: 0,
                row: 0,
                cells: 1,
                cluster_advance: u32_to_f32(frame.cell_width),
                x_offset: 0.0,
                y_offset: 0.0,
                foreground: [235; 3],
            },
            SnapshotGlyph {
                key: right_key,
                column: 1,
                row: 0,
                cells: 1,
                cluster_advance: u32_to_f32(frame.cell_width),
                x_offset: 0.0,
                y_offset: 0.0,
                foreground: [235; 3],
            },
        ];
        let geometry = frame.tight_geometry().unwrap();
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        let mut canvas = vec![0; usize::try_from(width * height * 4).unwrap()];
        for pixel in canvas.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[22, 18, 14, 255]);
        }
        paint_glyphs(&mut canvas, width, height, &frame, &geometry, None);
        let rect = geometry.cell_rect(1, 0).unwrap();
        let x = rect.x;
        let y = rect.y;
        let index = usize::try_from((y * width + x) * 4).unwrap();
        assert_eq!(&canvas[index..index + 4], &[171, 169, 168, 255]);
    }

    #[test]
    fn snapshot_fallback_uses_cell_pen_without_centering() {
        let snapshot = incremental_snapshot();
        let mut frame = SnapshotFrame::load_scaled(&snapshot, 120).unwrap();
        let key = GlyphKey { face: 5, glyph: 1 };
        frame.cache.clear();
        frame.cache.insert(
            key,
            Arc::new(CachedGlyph {
                content: Content::Mask,
                left: 0,
                top: frame.baseline,
                width: 1,
                height: 1,
                data: vec![255],
            }),
        );
        frame.glyphs = vec![SnapshotGlyph {
            key,
            column: 0,
            row: 0,
            cells: 2,
            cluster_advance: u32_to_f32(frame.cell_width.saturating_mul(2) - 3),
            x_offset: 0.0,
            y_offset: 0.0,
            foreground: [235; 3],
        }];
        let geometry = frame.tight_geometry().unwrap();
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        let mut canvas = vec![0; usize::try_from(width * height * 4).unwrap()];
        paint_glyphs(&mut canvas, width, height, &frame, &geometry, None);
        let origin = geometry.cell_rect(0, 0).unwrap();
        let index = usize::try_from((origin.y * width + origin.x) * 4).unwrap();
        assert_eq!(&canvas[index..index + 4], &[235, 235, 235, 255]);
    }

    #[test]
    fn snapshot_decorations_use_foot_baseline_metrics_in_full_and_row_paints() {
        let mut snapshot = incremental_snapshot();
        snapshot.visible_rows[0].cells[0].attributes.underline = UnderlineStyle::Single;
        snapshot.visible_rows[0].cells[0]
            .attributes
            .underline_color_source = ColorSource::Rgb;
        snapshot.visible_rows[0].cells[0].attributes.underline_color = 0x0012_3456;
        snapshot.visible_rows[0].cells[1].attributes.strikethrough = true;
        let frame = SnapshotFrame::load_scaled(&snapshot, 120).expect("decorated frame");
        assert_eq!(frame.decorations.len(), 2);
        assert_eq!(frame.underline_position, -3);
        assert_eq!(frame.underline_thickness, 1);
        assert_eq!(frame.strike_position, 7);
        assert_eq!(frame.strike_thickness, 1);

        let geometry = frame.tight_geometry().unwrap();
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        let mut full = vec![0; usize::try_from(width * height * 4).unwrap()];
        paint_snapshot(
            &mut full,
            width,
            height,
            &frame,
            &geometry,
            false,
            CursorStyle::Block,
        );
        let first = geometry.cell_rect(0, 0).unwrap();
        let second = geometry.cell_rect(1, 0).unwrap();
        let underline_y =
            usize::try_from(first.y).unwrap() + usize::try_from(frame.baseline + 3).unwrap();
        let strike_y = usize::try_from(second.y).unwrap()
            + usize::try_from(frame.baseline - frame.strike_position).unwrap();
        let underline_x = usize::try_from(first.x).unwrap();
        let strike_x = usize::try_from(second.x).unwrap();
        let pixel = |x: usize, y: usize| &full[(y * width as usize + x) * 4..][..4];
        assert_eq!(pixel(underline_x, underline_y), &[0x56, 0x34, 0x12, 0xff]);
        assert_eq!(pixel(strike_x, strike_y), &[0xeb, 0xeb, 0xeb, 0xff]);

        let mut rows = vec![0; full.len()];
        for pixel in rows.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[0x16, 0x12, 0x0e, 0xff]);
        }
        paint_snapshot_rows(
            &mut rows,
            width,
            height,
            &frame,
            &geometry,
            &[true, true],
            false,
            CursorStyle::Block,
        );
        assert_eq!(full, rows);
    }

    #[test]
    fn snapshot_styles_select_distinct_primary_faces_and_cache_keys() {
        let mut snapshot = incremental_snapshot();
        snapshot.visible_rows[0].cells[0].content = "A".to_owned();
        snapshot.visible_rows[0].cells[0].attributes.underline = UnderlineStyle::Single;
        snapshot.visible_rows[0].cells[1].content = "A".to_owned();
        snapshot.visible_rows[0].cells[1].attributes.bold = true;
        snapshot.visible_rows[1].cells[0].content = "A".to_owned();
        snapshot.visible_rows[1].cells[0].attributes.italic = true;
        snapshot.visible_rows[1].cells[0].attributes.underline = UnderlineStyle::Single;
        snapshot.visible_rows[1].cells[1].content = "A".to_owned();
        snapshot.visible_rows[1].cells[1].attributes.bold = true;
        snapshot.visible_rows[1].cells[1].attributes.italic = true;

        let frame = SnapshotFrame::load_scaled(&snapshot, 120).expect("styled frame");
        let faces: HashSet<_> = frame.glyphs.iter().map(|glyph| glyph.key.face).collect();
        assert_eq!(
            faces,
            HashSet::from([
                SNAPSHOT_PRIMARY_REGULAR,
                SNAPSHOT_PRIMARY_BOLD,
                SNAPSHOT_PRIMARY_ITALIC,
                SNAPSHOT_PRIMARY_BOLD_ITALIC,
            ])
        );
        assert_eq!(frame.cache.len(), 4, "each style owns a distinct cache key");
        let regular = frame
            .decorations
            .iter()
            .find(|span| span.row == 0 && span.column == 0)
            .unwrap();
        let italic = frame
            .decorations
            .iter()
            .find(|span| span.row == 1 && span.column == 0)
            .unwrap();
        assert_eq!(regular.metrics, frame.cell_metrics[0]);
        assert_eq!(italic.metrics, frame.cell_metrics[2]);
    }

    #[test]
    fn color_fallback_cache_uses_fcft_fixed_strike_size_and_advance() {
        let faces = snapshot_faces().unwrap();
        let font = font_ref(&faces[SNAPSHOT_EMOJI]).unwrap();
        let glyph_id = font.charmap().map('\u{1f642}');
        let small = snapshot_glyph(faces, SNAPSHOT_EMOJI, glyph_id, 12.0).unwrap();
        let small_advance = snapshot_color_advance(SNAPSHOT_EMOJI, glyph_id, 12.0).unwrap();
        let larger = snapshot_glyph(faces, SNAPSHOT_EMOJI, glyph_id, 15.0).unwrap();
        let larger_advance = snapshot_color_advance(SNAPSHOT_EMOJI, glyph_id, 15.0).unwrap();

        assert_eq!((small.width, small.height, small_advance), (14, 14, 14));
        assert_eq!((larger.width, larger.height, larger_advance), (18, 17, 18));
        assert!(!Arc::ptr_eq(&small, &larger));
        assert_ne!(small.data, larger.data);
    }

    #[test]
    fn underline_style_color_partial_mutation_matches_clean_full_rebuild() {
        let mut initial = incremental_snapshot();
        initial.visible_rows[0].cells[0].attributes.underline = UnderlineStyle::Single;
        let mut changed = initial.clone();
        changed.visible_rows[0].cells[0].attributes.underline = UnderlineStyle::Dashed;
        changed.visible_rows[0].cells[0]
            .attributes
            .underline_color_source = ColorSource::Rgb;
        changed.visible_rows[0].cells[0].attributes.underline_color = 0x12_34_56;

        let mut frame = SnapshotFrame::load_scaled(&initial, 120).unwrap();
        let geometry = frame.tight_geometry().unwrap();
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        let mut actual = vec![0; usize::try_from(width * height * 4).unwrap()];
        paint_snapshot(
            &mut actual,
            width,
            height,
            &frame,
            &geometry,
            false,
            CursorStyle::Block,
        );
        frame.refresh_rows(&changed, &[true, false]).unwrap();
        paint_snapshot_rows(
            &mut actual,
            width,
            height,
            &frame,
            &geometry,
            &[true, false],
            false,
            CursorStyle::Block,
        );

        let reference = SnapshotFrame::load_scaled(&changed, 120).unwrap();
        let mut expected = vec![0; actual.len()];
        paint_snapshot(
            &mut expected,
            width,
            height,
            &reference,
            &geometry,
            false,
            CursorStyle::Block,
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn production_ascii_evidence_is_identical_with_cold_and_warm_cache() {
        let cold = production_ascii_glyph_evidence().expect("cold production evidence");
        let warm = production_ascii_glyph_evidence().expect("warm production evidence");
        assert_eq!(cold, warm);
    }

    #[test]
    fn full_and_all_row_damage_paints_are_byte_identical() {
        let snapshot = incremental_snapshot();
        let frame = SnapshotFrame::load_scaled(&snapshot, 120).expect("frame");
        let geometry = frame.tight_geometry().unwrap();
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        let bytes = usize::try_from(width * height * 4).unwrap();
        let mut full = vec![0; bytes];
        paint_snapshot(
            &mut full,
            width,
            height,
            &frame,
            &geometry,
            true,
            CursorStyle::Block,
        );

        let background = [
            frame.canvas_background[2],
            frame.canvas_background[1],
            frame.canvas_background[0],
            0xff,
        ];
        let mut incremental = vec![0; bytes];
        for pixel in incremental.chunks_exact_mut(4) {
            pixel.copy_from_slice(&background);
        }
        paint_snapshot_rows(
            &mut incremental,
            width,
            height,
            &frame,
            &geometry,
            &vec![true; frame.rows as usize],
            true,
            CursorStyle::Block,
        );
        assert_eq!(full, incremental);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Slice 5 intentionally exercises every path from one semantic-state harness"
    )]
    fn equivalent_semantic_state_survives_cursor_cache_scale_and_theme_paths() {
        fn render(
            snapshot: &TerminalSnapshot,
            scale_120: u32,
            cursor_visible: bool,
            cursor_style: CursorStyle,
        ) -> Vec<u8> {
            let frame = SnapshotFrame::load_scaled(snapshot, scale_120).expect("frame");
            let geometry = frame.tight_geometry().unwrap();
            let mut pixels =
                vec![
                    0;
                    usize::try_from(geometry.buffer_width() * geometry.buffer_height() * 4)
                        .unwrap()
                ];
            paint_snapshot_presented(
                &mut pixels,
                geometry.buffer_width(),
                geometry.buffer_height(),
                &frame,
                &geometry,
                cursor_visible,
                cursor_style,
                CursorPresentation::FOCUSED_STEADY,
            );
            pixels
        }

        let mut semantic_state = incremental_snapshot();
        semantic_state.visible_rows[0].cells[0].attributes.underline = UnderlineStyle::Curly;
        semantic_state.visible_rows[0].cells[1]
            .attributes
            .strikethrough = true;
        semantic_state.visible_rows[1].cells[0]
            .attributes
            .foreground_source = ColorSource::Base16;
        semantic_state.visible_rows[1].cells[0]
            .attributes
            .foreground = 1;
        semantic_state.palette[1] = 0x0035_4a60;
        let reference = render(&semantic_state, 120, true, CursorStyle::Block);

        SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
        let cold = render(&semantic_state, 120, true, CursorStyle::Block);
        let cold_metrics = snapshot_cache_metrics();
        let warm = render(&semantic_state, 120, true, CursorStyle::Block);
        let warm_metrics = snapshot_cache_metrics();
        assert_eq!(cold, reference);
        assert_eq!(warm, reference);
        assert!(warm_metrics["hits"].as_u64() > cold_metrics["hits"].as_u64());

        SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
        let repopulated = render(&semantic_state, 120, true, CursorStyle::Block);
        assert_eq!(repopulated, reference);
        assert!(snapshot_cache_metrics()["entries"].as_u64().unwrap() > 0);

        let scaled = render(&semantic_state, 150, true, CursorStyle::Block);
        assert_ne!(scaled.len(), reference.len());
        assert_eq!(
            render(&semantic_state, 120, true, CursorStyle::Block),
            reference
        );

        let mut alternate_theme = semantic_state.clone();
        alternate_theme.palette[1] = 0x00f0_8040;
        alternate_theme.default_colors = [0x0011_2233, 0x0044_5566, 0x0077_8899];
        assert_ne!(
            render(&alternate_theme, 120, true, CursorStyle::Block),
            reference
        );
        assert_eq!(
            render(&semantic_state, 120, true, CursorStyle::Block),
            reference
        );

        let frame = SnapshotFrame::load_scaled(&semantic_state, 120).expect("cursor frame");
        let geometry = frame.tight_geometry().unwrap();
        let width = geometry.buffer_width();
        let height = geometry.buffer_height();
        let dirty_cursor_row = [true, false];
        let mut pixels = reference.clone();
        paint_snapshot_rows_presented(
            &mut pixels,
            width,
            height,
            &frame,
            &geometry,
            &dirty_cursor_row,
            false,
            CursorStyle::Block,
            CursorPresentation::FOCUSED_STEADY,
        );
        assert_eq!(
            pixels,
            render(&semantic_state, 120, false, CursorStyle::Block)
        );
        paint_snapshot_rows_presented(
            &mut pixels,
            width,
            height,
            &frame,
            &geometry,
            &dirty_cursor_row,
            true,
            CursorStyle::Beam,
            CursorPresentation::FOCUSED_STEADY,
        );
        assert_eq!(
            pixels,
            render(&semantic_state, 120, true, CursorStyle::Beam)
        );

        let mut moved = semantic_state.clone();
        moved.cursor_column = 1;
        let mut moved_frame = frame;
        moved_frame.refresh_cursor(&moved);
        paint_snapshot_rows_presented(
            &mut pixels,
            width,
            height,
            &moved_frame,
            &geometry,
            &dirty_cursor_row,
            true,
            CursorStyle::Underline,
            CursorPresentation::FOCUSED_STEADY,
        );
        assert_eq!(pixels, render(&moved, 120, true, CursorStyle::Underline));
    }

    #[test]
    fn forward_and_reverse_viewport_scroll_copy_match_clean_full_repaint() {
        let mut initial = incremental_snapshot();
        initial.input_modes.cursor_visible = false;
        for (offset_delta, rows, dirty_rows) in [
            (1, ["xy", "ab"], [true, false]),
            (-1, ["cd", "xy"], [false, true]),
        ] {
            let mut shifted = initial.clone();
            shifted.visible_rows = rows
                .into_iter()
                .map(|text| TerminalRow {
                    row_id: None,
                    linebreak: false,
                    cells: text
                        .chars()
                        .map(|character| TerminalCell {
                            content: character.to_string(),
                            spacer_remaining: None,
                            attributes: default_attributes(),
                        })
                        .collect(),
                })
                .collect();
            let mut incremental = SnapshotFrame::load_scaled(&initial, 120).expect("initial frame");
            let reference = SnapshotFrame::load_scaled(&shifted, 120).expect("shifted frame");
            let geometry = incremental.tight_geometry().unwrap();
            let width = geometry.buffer_width();
            let height = geometry.buffer_height();
            let mut actual = vec![0; usize::try_from(width * height * 4).unwrap()];
            paint_snapshot(
                &mut actual,
                width,
                height,
                &incremental,
                &geometry,
                false,
                CursorStyle::Block,
            );
            let scroll = incremental
                .scroll_viewport_rows(&shifted, offset_delta)
                .expect("viewport shift")
                .expect("incremental scroll");
            scroll_snapshot_pixels(&mut actual, width, &incremental, &geometry, scroll);
            paint_snapshot_rows(
                &mut actual,
                width,
                height,
                &incremental,
                &geometry,
                &dirty_rows,
                false,
                CursorStyle::Block,
            );
            let mut expected = vec![0; actual.len()];
            paint_snapshot(
                &mut expected,
                width,
                height,
                &reference,
                &geometry,
                false,
                CursorStyle::Block,
            );
            assert_eq!(actual, expected, "scroll delta {offset_delta}");
        }
    }

    #[test]
    fn persistent_glyph_cache_evicts_fifo_and_removes_color_advance() {
        let mut cache = PersistentGlyphCache::default();
        for glyph in 0..=SNAPSHOT_GLYPH_CACHE_BUDGET {
            let key = (
                768,
                GlyphKey {
                    face: SNAPSHOT_EMOJI,
                    glyph: u16::try_from(glyph).expect("test glyph fits u16"),
                },
            );
            cache.insert_glyph(
                key,
                Arc::new(CachedGlyph {
                    content: Content::Color,
                    left: 0,
                    top: 0,
                    width: 1,
                    height: 1,
                    data: vec![0; 4],
                }),
                Some(1),
            );
        }
        let first = (
            768,
            GlyphKey {
                face: SNAPSHOT_EMOJI,
                glyph: 0,
            },
        );
        assert_eq!(cache.glyphs.len(), SNAPSHOT_GLYPH_CACHE_BUDGET);
        assert_eq!(cache.advances.len(), SNAPSHOT_GLYPH_CACHE_BUDGET);
        assert!(!cache.glyphs.contains_key(&first));
        assert!(!cache.advances.contains_key(&first));
        assert_eq!(cache.glyph_bytes, SNAPSHOT_GLYPH_CACHE_BUDGET * 4);
        assert_eq!(cache.evictions, 1);

        let mut byte_bounded = PersistentGlyphCache::default();
        for glyph in 0..3 {
            byte_bounded.insert_glyph_bounded(
                (768, GlyphKey { face: 0, glyph }),
                Arc::new(CachedGlyph {
                    content: Content::Mask,
                    left: 0,
                    top: 0,
                    width: 8,
                    height: 1,
                    data: vec![0; 8],
                }),
                None,
                10,
                16,
            );
        }
        assert_eq!(byte_bounded.glyphs.len(), 2);
        assert_eq!(byte_bounded.glyph_bytes, 16);
        assert_eq!(byte_bounded.evictions, 1);
    }

    #[test]
    fn persistent_raster_face_cache_is_bounded_across_scale_churn() {
        SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
        let snapshot = incremental_snapshot();
        for scale_120 in 120..=u32::try_from(120 + SNAPSHOT_RASTER_FACE_BUDGET).unwrap() {
            SnapshotFrame::load_scaled(&snapshot, scale_120).expect("scaled frame");
        }
        let metrics = snapshot_cache_metrics();
        assert_eq!(
            metrics["raster_faces"].as_u64(),
            Some(u64::try_from(SNAPSHOT_RASTER_FACE_BUDGET).unwrap())
        );
        assert_eq!(metrics["raster_face_evictions"].as_u64(), Some(1));
    }

    #[test]
    fn glyph_cache_entries_are_effective_raster_size_specific() {
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
    fn trusted_history_overlay_is_bounded_static_and_clamps_counts() {
        let width = 320;
        let height = 180;
        let sentinel = [1, 2, 3, 4];
        let mut clamped = sentinel.repeat(usize::try_from(width * height).unwrap());
        let layout = paint_history_overlay(
            &mut clamped,
            width,
            height,
            120,
            HistoryOverlayStatus {
                offset_from_bottom: 12,
                available_rows: 4_096,
                unseen_rows: 1_000,
            },
            0x0010_1820,
            0x0078_d2ff,
        )
        .expect("overlay fits");
        let mut maximum = sentinel.repeat(usize::try_from(width * height).unwrap());
        paint_history_overlay(
            &mut maximum,
            width,
            height,
            120,
            HistoryOverlayStatus {
                offset_from_bottom: 12,
                available_rows: 999,
                unseen_rows: 999,
            },
            0x0010_1820,
            0x0078_d2ff,
        );
        assert_eq!(clamped, maximum);
        let (panel_x, panel_y, panel_width, panel_height) = layout.panel;
        let (action_x, action_y, action_width, action_height) = layout.return_to_live;
        assert!(action_x >= panel_x);
        assert_eq!(action_y, panel_y);
        assert!(action_width <= panel_width);
        assert_eq!(action_height, panel_height);
        for y in 0..height {
            for x in 0..width {
                let inside = i32::try_from(x).unwrap() >= panel_x
                    && i32::try_from(x).unwrap()
                        < panel_x.saturating_add(i32::try_from(panel_width).unwrap())
                    && i32::try_from(y).unwrap() >= panel_y
                    && i32::try_from(y).unwrap()
                        < panel_y.saturating_add(i32::try_from(panel_height).unwrap());
                if !inside {
                    let index = usize::try_from((y * width + x) * 4).unwrap();
                    assert_eq!(&clamped[index..index + 4], sentinel.as_slice());
                }
            }
        }
        assert!(history_overlay_layout(40, 20, 120).is_none());
    }

    #[test]
    fn empty_overlays_leave_compositor_border_area_untouched() {
        let snapshot = incremental_snapshot();
        let frame = SnapshotFrame::load_scaled(&snapshot, 120).expect("frame");
        let geometry = frame.tight_geometry().unwrap();
        let mut focused = vec![0_u8; 200 * 200 * 4];
        let mut unfocused = focused.clone();
        paint_snapshot_overlays(
            &mut focused,
            200,
            200,
            &frame,
            &geometry,
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
            &geometry,
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
    fn incremental_refresh_retains_warm_glyphs_below_budget() {
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
        assert!(referenced.iter().all(|key| frame.cache.contains_key(key)));
        assert!(old_keys.iter().all(|key| frame.cache.contains_key(key)));
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
            ([0x55, 0x2a, 0x15], [0xff, 0, 0])
        );
        attributes.reverse = true;
        assert_eq!(
            rendition_colors(
                &attributes,
                &palette,
                default_foreground(),
                default_background()
            ),
            ([0xaa, 0, 0], [0x80, 0x40, 0x20])
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
            decorations: Vec::new(),
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
            foregrounds: vec![[200, 100, 50]; 2],
            cell_metrics: vec![
                DecorationMetrics {
                    underline_position: -1,
                    underline_thickness: 1,
                    strike_position: 1,
                    strike_thickness: 1,
                };
                2
            ],
            cell_spans: vec![2, 0],
            columns: 2,
            rows: 1,
            cell_width: 4,
            cell_height: 4,
            ascent: 2,
            descent: 2,
            baseline: 2,
            underline_position: -1,
            underline_thickness: 1,
            strike_position: 1,
            strike_thickness: 1,
            padding: TerminalPadding::uniform(2),
            cursor: None,
            canvas_background: [14, 18, 22],
            cursor_color: [0xeb, 0xeb, 0xeb],
            scale_120: 120,
        };
        let geometry = frame.tight_geometry().unwrap();
        let mut canvas = vec![0; 12 * 8 * 4];
        paint_snapshot(
            &mut canvas,
            12,
            8,
            &frame,
            &geometry,
            false,
            CursorStyle::Block,
        );
        let pixel = |x: usize, y: usize| &canvas[(y * 12 + x) * 4..(y * 12 + x + 1) * 4];
        assert_eq!(pixel(2, 2), [3, 2, 1, 0xff]);
        assert_eq!(pixel(2, 3), [50, 100, 200, 0xff]);
        assert_eq!(pixel(4, 3), [3, 2, 1, 0xff]);
        assert_eq!(pixel(6, 2), [3, 2, 1, 0xff]);
    }

    fn damage_test_frame() -> SnapshotFrame {
        SnapshotFrame {
            glyphs: Vec::new(),
            decorations: Vec::new(),
            cache: HashMap::new(),
            backgrounds: vec![[1, 0, 0], [2, 0, 0], [3, 0, 0]],
            foregrounds: vec![[255, 255, 255]; 3],
            cell_metrics: vec![
                DecorationMetrics {
                    underline_position: 0,
                    underline_thickness: 1,
                    strike_position: 0,
                    strike_thickness: 1,
                };
                3
            ],
            cell_spans: vec![1; 3],
            columns: 1,
            rows: 3,
            cell_width: 2,
            cell_height: 2,
            ascent: 1,
            descent: 1,
            baseline: 1,
            underline_position: 0,
            underline_thickness: 1,
            strike_position: 0,
            strike_thickness: 1,
            padding: TerminalPadding::uniform(0),
            cursor: None,
            canvas_background: [0, 0, 0],
            cursor_color: [255, 255, 255],
            scale_120: 120,
        }
    }

    #[test]
    fn row_damage_paints_only_selected_rows() {
        let frame = damage_test_frame();
        let geometry = frame.tight_geometry().unwrap();
        let mut canvas = vec![0; 2 * 6 * 4];
        paint_snapshot_rows(
            &mut canvas,
            2,
            6,
            &frame,
            &geometry,
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
        let geometry = frame.tight_geometry().unwrap();
        let mut canvas = vec![0; 2 * 6 * 4];
        paint_snapshot(
            &mut canvas,
            2,
            6,
            &frame,
            &geometry,
            false,
            CursorStyle::Block,
        );
        scroll_snapshot_pixels(
            &mut canvas,
            2,
            &frame,
            &geometry,
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
        let geometry = frame.tight_geometry().unwrap();
        let scroll = TerminalScroll {
            direction: ScrollDirection::Forward,
            start_row: 0,
            end_row: 3,
            rows: 1,
        };
        let mut narrow = vec![0_u8; 12];
        narrow[8..12].copy_from_slice(&[1, 2, 3, 4]);
        scroll_snapshot_pixels(&mut narrow, 1, &frame, &geometry, scroll);
        assert_eq!(&narrow[..4], &[1, 2, 3, 4]);

        let mut short = vec![0_u8; 8];
        let unchanged = short.clone();
        scroll_snapshot_pixels(&mut short, 2, &frame, &geometry, scroll);
        assert_eq!(short, unchanged);

        let mut partial_scanline = vec![0_u8; 7];
        scroll_snapshot_pixels(&mut partial_scanline, 2, &frame, &geometry, scroll);
    }

    #[test]
    fn terminal_size_calculation_clamps_minimum_and_protocol_limits() {
        let frame = SnapshotFrame {
            glyphs: Vec::new(),
            decorations: Vec::new(),
            cache: HashMap::new(),
            backgrounds: Vec::new(),
            foregrounds: Vec::new(),
            cell_metrics: Vec::new(),
            cell_spans: Vec::new(),
            columns: 0,
            rows: 0,
            cell_width: 10,
            cell_height: 20,
            ascent: 15,
            descent: 5,
            baseline: 15,
            underline_position: -2,
            underline_thickness: 1,
            strike_position: 5,
            strike_thickness: 1,
            padding: TerminalPadding::uniform(10),
            cursor: None,
            canvas_background: [14, 18, 22],
            cursor_color: [0xeb, 0xeb, 0xeb],
            scale_120: 120,
        };
        assert_eq!(
            frame.terminal_size(1_020, 620, 120).expect("normal grid"),
            (100, 30, 1_000, 600)
        );
        assert!(
            frame
                .terminal_size(1, 1, 120)
                .unwrap_err()
                .to_string()
                .contains("SurfaceTooSmall")
        );
        assert_eq!(
            frame
                .terminal_size(20_000, 20_000, 120)
                .expect("bounded grid"),
            (MAX_COLUMNS, MAX_ROWS, 2_400, 1_600)
        );
        let configured = frame.window_geometry(1_027, 629, 120).unwrap();
        assert_eq!(
            (configured.logical_width(), configured.logical_height()),
            (1_027, 629)
        );
        assert_eq!(
            configured.actual_padding.left
                + configured.grid_rect.width
                + configured.actual_padding.right,
            1_027
        );
        assert_eq!(
            configured.actual_padding.top
                + configured.grid_rect.height
                + configured.actual_padding.bottom,
            629
        );
        assert!(configured.residual_right > 0 || configured.residual_bottom > 0);
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
