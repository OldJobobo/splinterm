//! Font discovery, fallback, shaping, metrics, and persistent glyph caches.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    process::Command,
    sync::{Arc, OnceLock},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use splinterm_filemap::ReadOnlyFileMap;
use splinterm_freetype::RasterFace;
use swash::{
    FontRef,
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    shape::ShapeContext,
    zeno::Format,
};

use crate::box_drawing;

use super::raster::{blend_glyph, fill_rect};
use super::{BASE_FONT_SIZE, PRIMARY_FONT, effective_font_size, renderer_options};

pub(super) const BASE_ROW_X: i32 = 32;
pub(super) const BASE_ROW_Y: i32 = 96;
pub(super) const CJK_FONT: &str = "Noto Sans CJK JP:style=Regular";
pub(super) const EMOJI_FONT: &str = "Noto Color Emoji";
pub(super) const SNAPSHOT_GLYPH_CACHE_BUDGET: usize = 2_048;
pub(super) const SNAPSHOT_GLYPH_CACHE_BYTE_BUDGET: usize = 64 * 1024 * 1024;
pub(super) const SNAPSHOT_RASTER_FACE_BUDGET: usize = 24;

pub(super) const SNAPSHOT_PRIMARY_REGULAR: usize = 0;
pub(super) const SNAPSHOT_PRIMARY_BOLD: usize = 1;
pub(super) const SNAPSHOT_PRIMARY_ITALIC: usize = 2;
pub(super) const SNAPSHOT_PRIMARY_BOLD_ITALIC: usize = 3;
pub(super) const SNAPSHOT_CJK: usize = 4;
pub(super) const SNAPSHOT_EMOJI: usize = 5;

pub(super) static SNAPSHOT_FACES: OnceLock<Result<[FontFace; 6], String>> = OnceLock::new();
#[derive(Default)]
pub(super) struct PersistentGlyphCache {
    pub(super) raster_faces: HashMap<(isize, usize), RasterFace>,
    pub(super) raster_face_order: VecDeque<(isize, usize)>,
    pub(super) glyphs: HashMap<(isize, GlyphKey), Arc<CachedGlyph>>,
    pub(super) advances: HashMap<(isize, GlyphKey), i32>,
    pub(super) order: VecDeque<(isize, GlyphKey)>,
    pub(super) glyph_bytes: usize,
    pub(super) hits: u64,
    pub(super) misses: u64,
    pub(super) evictions: u64,
    pub(super) raster_face_evictions: u64,
}

impl PersistentGlyphCache {
    pub(super) fn insert_glyph(
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

    pub(super) fn insert_glyph_bounded(
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

    pub(super) fn prepare_raster_face_insert(&mut self, raster_key: (isize, usize)) {
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
    pub(super) static SNAPSHOT_GLYPH_CACHE: RefCell<PersistentGlyphCache> =
        RefCell::new(PersistentGlyphCache::default());
}

pub(super) fn clear_snapshot_caches() {
    SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
}

pub(super) const CORPUS: &[(CorpusKind, &str)] = &[
    (CorpusKind::Ascii, "ASCII"),
    (CorpusKind::BoxDrawing, "┌─┼─┐"),
    (CorpusKind::NerdFont, "\u{f120}"),
    (CorpusKind::Combining, "e\u{0301}"),
    (CorpusKind::Cjk, "界"),
    (CorpusKind::Emoji, "🙂"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CorpusKind {
    Ascii,
    BoxDrawing,
    NerdFont,
    Combining,
    Cjk,
    Emoji,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct GlyphKey {
    pub(super) face: usize,
    pub(super) glyph: u16,
}

pub(super) const BOX_DRAWING_FACE: usize = usize::MAX;

pub(super) struct FontFace {
    pub(super) label: &'static str,
    pub(super) family: String,
    pub(super) style: String,
    pub(super) path: PathBuf,
    pub(super) index: usize,
    pub(super) weight: i32,
    pub(super) slant: i32,
    pub(super) selected_pixel_size_26_6: isize,
    pub(super) data: OnceLock<Result<ReadOnlyFileMap, String>>,
}

impl FontFace {
    fn fallback_for(&self, label: &'static str) -> Self {
        Self {
            label,
            family: self.family.clone(),
            style: self.style.clone(),
            path: self.path.clone(),
            index: self.index,
            weight: self.weight,
            slant: self.slant,
            selected_pixel_size_26_6: self.selected_pixel_size_26_6,
            data: OnceLock::new(),
        }
    }

    fn identity(&self) -> (&std::path::Path, usize) {
        (&self.path, self.index)
    }
}

pub(super) struct CachedGlyph {
    pub(super) content: Content,
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InkBounds {
    pub(super) left: u32,
    pub(super) top: u32,
    pub(super) right: u32,
    pub(super) bottom: u32,
}

impl CachedGlyph {
    pub(super) fn ink_bounds(&self) -> Option<InkBounds> {
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
pub(super) struct PlacedGlyph {
    pub(super) key: GlyphKey,
    pub(super) cell: u32,
    pub(super) cells: u32,
    pub(super) cluster_advance: f32,
    pub(super) x_offset: f32,
    pub(super) y_offset: f32,
}

pub(crate) struct TextRow {
    pub(super) glyphs: Vec<PlacedGlyph>,
    pub(super) cache: HashMap<GlyphKey, CachedGlyph>,
    pub(super) cell_width: u32,
    pub(super) cell_height: u32,
    pub(super) baseline: i32,
    pub(super) cell_count: u32,
    pub(super) origin_x: i32,
    pub(super) origin_y: i32,
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
        let [primary, _, _, _] = resolve_primary_faces(PRIMARY_FONT)?;
        let faces = [
            primary,
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

pub(super) fn cache_glyph(
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

pub(super) fn snapshot_faces() -> Result<&'static [FontFace; 6]> {
    SNAPSHOT_FACES
        .get_or_init(|| {
            let [regular, bold, italic, bold_italic] =
                resolve_primary_faces(&renderer_options().font)
                    .map_err(|error| error.to_string())?;
            Ok([
                regular,
                bold,
                italic,
                bold_italic,
                resolve_face("CJK fallback", CJK_FONT, "noto sans cjk")
                    .map_err(|error| error.to_string())?,
                resolve_face("emoji fallback", EMOJI_FONT, "noto color emoji")
                    .map_err(|error| error.to_string())?,
            ])
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

pub(super) fn snapshot_glyph(
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
pub(super) fn pixel_size_26_6(font_size: f32) -> Result<isize> {
    let value = (font_size * 64.0).round();
    if !value.is_finite() || !(64.0..=(768.0 * 64.0)).contains(&value) {
        bail!("scaled font size is outside the FreeType raster policy");
    }
    Ok(value as isize)
}

pub(super) fn snapshot_color_advance(
    face_index: usize,
    glyph_id: u16,
    font_size: f32,
) -> Result<i32> {
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

pub(super) fn reset_snapshot_cache() {
    SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
}

pub(super) fn evict_snapshot_glyphs() -> usize {
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

pub(super) fn process_rss_bytes() -> Option<u64> {
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

pub(super) fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

pub(super) fn glyph_alpha_bytes(glyph: &CachedGlyph) -> Vec<u8> {
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
pub(super) struct CellMetrics {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) ascent: u32,
    pub(super) descent: u32,
    pub(super) font_height: i32,
    pub(super) baseline: i32,
    pub(super) mono_advance: f32,
    pub(super) underline_position: i32,
    pub(super) underline_thickness: u32,
    pub(super) strike_position: i32,
    pub(super) strike_thickness: u32,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "the protocol-bounded integer terminal advance is exactly representable in f32"
)]
pub(super) fn cell_metrics(primary_face: &FontFace, font_size: f32) -> Result<CellMetrics> {
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
pub(super) fn positive_round_to_u32(value: f32) -> u32 {
    assert!(value.is_finite() && value > 0.0);
    value.round().max(1.0) as u32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn positive_trunc_to_u32(value: f32) -> u32 {
    assert!(value.is_finite() && value > 0.0);
    value.trunc().max(1.0) as u32
}

#[allow(clippy::cast_possible_truncation)]
pub(super) fn ceil_to_i32(value: f32) -> i32 {
    assert!(value.is_finite());
    value.ceil() as i32
}

pub(super) fn resolve_face(
    label: &'static str,
    pattern: &str,
    expected_family_fragment: &str,
) -> Result<FontFace> {
    let output = Command::new("fc-match")
        .args([
            "-f",
            "%{file}\\n%{index}\\n%{family[0]}\\n%{style}\\n%{weight}\\n%{slant}\\n%{pixelsize}\\n",
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
        .with_context(|| format!("fc-match returned no font family for {label}"))?
        .to_owned();
    let style = lines
        .next()
        .with_context(|| format!("fc-match returned no style for {label}"))?
        .to_owned();
    let weight = lines
        .next()
        .with_context(|| format!("fc-match returned no weight for {label}"))?
        .parse::<i32>()
        .with_context(|| format!("fc-match returned a non-numeric weight for {label}"))?;
    let slant = lines
        .next()
        .with_context(|| format!("fc-match returned no slant for {label}"))?
        .parse::<i32>()
        .with_context(|| format!("fc-match returned a non-numeric slant for {label}"))?;
    let selected_pixel_size = lines
        .next()
        .with_context(|| format!("fc-match returned no pixel size for {label}"))?
        .parse::<f32>()
        .with_context(|| format!("fc-match returned an invalid pixel size for {label}"))?;
    let selected_pixel_size_26_6 = pixel_size_26_6(selected_pixel_size)?;
    let normalized_family = normalize_family(&family);
    let normalized_expected = normalize_family(expected_family_fragment);
    if !normalized_expected.is_empty() && !normalized_family.contains(&normalized_expected) {
        bail!("explicit {label} pattern {pattern:?} resolved unexpectedly to {family:?}");
    }
    let face = FontFace {
        label,
        family,
        style,
        path,
        index,
        weight,
        slant,
        selected_pixel_size_26_6,
        data: OnceLock::new(),
    };
    eprintln!(
        "Resolved {label}: {} {} (face {}, {})",
        face.family,
        face.style,
        face.index,
        face.path.display()
    );
    Ok(face)
}

#[derive(Clone, Copy)]
struct PrimaryStyleRequest {
    label: &'static str,
    style: &'static str,
    bold: bool,
    italic: bool,
}

const PRIMARY_STYLE_REQUESTS: [PrimaryStyleRequest; 3] = [
    PrimaryStyleRequest {
        label: "primary bold",
        style: "Bold",
        bold: true,
        italic: false,
    },
    PrimaryStyleRequest {
        label: "primary italic",
        style: "Italic",
        bold: false,
        italic: true,
    },
    PrimaryStyleRequest {
        label: "primary bold italic",
        style: "Bold Italic",
        bold: true,
        italic: true,
    },
];

fn normalize_family(family: &str) -> String {
    family
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn escape_fontconfig_pattern_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '-' | ',' | ':') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn primary_style_pattern(family: &str, style: &str) -> String {
    format!(
        "{}:style={}",
        escape_fontconfig_pattern_value(family),
        escape_fontconfig_pattern_value(style)
    )
}

fn face_advance(face: &FontFace, font_size: f32) -> Result<f32> {
    let font = font_ref(face)?;
    let advance = font
        .glyph_metrics(&[])
        .scale(font_size)
        .advance_width(font.charmap().map('M'));
    anyhow::ensure!(advance.is_finite() && advance > 0.0, "invalid M advance");
    Ok(advance)
}

fn style_candidate_rejection(
    regular: &FontFace,
    candidate: &FontFace,
    request: PrimaryStyleRequest,
    regular_advance: f32,
    candidate_advance: f32,
) -> Option<&'static str> {
    if normalize_family(&candidate.family) != normalize_family(&regular.family) {
        return Some("resolved to another family");
    }
    if candidate.identity() == regular.identity() {
        return Some("resolved to the regular face");
    }
    if request.bold != (candidate.weight > regular.weight) {
        return Some("did not resolve the requested weight");
    }
    if request.italic != (candidate.slant != regular.slant) {
        return Some("did not resolve the requested slant");
    }
    if !candidate_advance.is_finite() || (candidate_advance - regular_advance).abs() > 0.01 {
        return Some("has incompatible terminal-cell metrics");
    }
    None
}

fn resolve_primary_style(
    regular: &FontFace,
    request: PrimaryStyleRequest,
    regular_advance: f32,
) -> FontFace {
    let pattern = primary_style_pattern(&regular.family, request.style);
    let resolved = resolve_face(request.label, &pattern, &regular.family).and_then(|candidate| {
        let candidate_advance = face_advance(&candidate, BASE_FONT_SIZE)?;
        if let Some(reason) = style_candidate_rejection(
            regular,
            &candidate,
            request,
            regular_advance,
            candidate_advance,
        ) {
            bail!("{reason}");
        }
        Ok(candidate)
    });
    match resolved {
        Ok(candidate) => candidate,
        Err(error) => {
            eprintln!(
                "splinterm font warning: {} for family {:?} is unavailable ({error}); using the regular face",
                request.style, regular.family
            );
            regular.fallback_for(request.label)
        }
    }
}

fn resolve_primary_faces(pattern: &str) -> Result<[FontFace; 4]> {
    let regular = resolve_face("primary regular", pattern, "")?;
    let regular_advance = face_advance(&regular, BASE_FONT_SIZE)
        .context("selected primary regular face is unusable")?;
    let [bold_request, italic_request, bold_italic_request] = PRIMARY_STYLE_REQUESTS;
    let bold = resolve_primary_style(&regular, bold_request, regular_advance);
    let italic = resolve_primary_style(&regular, italic_request, regular_advance);
    let bold_italic = resolve_primary_style(&regular, bold_italic_request, regular_advance);
    Ok([regular, bold, italic, bold_italic])
}

pub(super) fn font_data(face: &FontFace) -> Result<&[u8]> {
    face.data
        .get_or_init(|| {
            ReadOnlyFileMap::open(&face.path)
                .with_context(|| format!("map {}", face.path.display()))
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map(|mapping| &**mapping)
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

pub(super) fn font_ref(face: &FontFace) -> Result<FontRef<'_>> {
    FontRef::from_index(font_data(face)?, face.index).with_context(|| {
        format!(
            "parse {} face {} with Swash",
            face.path.display(),
            face.index
        )
    })
}

pub(super) fn select_glyph(
    faces: &[FontFace; 3],
    kind: CorpusKind,
    character: char,
) -> Result<(usize, u16)> {
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

pub(super) fn glyph_origin(
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
pub(super) fn u32_to_f32(value: u32) -> f32 {
    value as f32
}

#[allow(clippy::cast_possible_truncation)]
pub(super) fn round_to_i32(value: f32) -> i32 {
    assert!(value.is_finite());
    value.round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_face(
        label: &'static str,
        family: &str,
        path: &str,
        weight: i32,
        slant: i32,
    ) -> FontFace {
        FontFace {
            label,
            family: family.to_owned(),
            style: label.to_owned(),
            path: PathBuf::from(path),
            index: 0,
            weight,
            slant,
            selected_pixel_size_26_6: 12 * 64,
            data: OnceLock::new(),
        }
    }

    #[test]
    fn style_patterns_are_derived_from_and_escape_the_selected_family() {
        assert_eq!(
            primary_style_pattern("Chosen-Family, Mono: Propo", "Bold Italic"),
            "Chosen\\-Family\\, Mono\\: Propo:style=Bold Italic"
        );
        assert!(!primary_style_pattern("Chosen", "Bold").contains("JetBrains"));
    }

    #[test]
    fn style_policy_accepts_only_compatible_faces_from_the_selected_family() {
        let regular = synthetic_face("regular", "Chosen Mono", "/fonts/regular.ttf", 80, 0);
        let request = PrimaryStyleRequest {
            label: "primary bold italic",
            style: "Bold Italic",
            bold: true,
            italic: true,
        };
        let compatible = synthetic_face(
            "bold italic",
            "Chosen Mono",
            "/fonts/bold-italic.ttf",
            200,
            100,
        );
        assert_eq!(
            style_candidate_rejection(&regular, &compatible, request, 8.0, 8.0),
            None
        );

        let foreign = synthetic_face("bold italic", "Other Mono", "/fonts/other.ttf", 200, 100);
        assert_eq!(
            style_candidate_rejection(&regular, &foreign, request, 8.0, 8.0),
            Some("resolved to another family")
        );

        let duplicate =
            synthetic_face("bold italic", "Chosen Mono", "/fonts/regular.ttf", 200, 100);
        assert_eq!(
            style_candidate_rejection(&regular, &duplicate, request, 8.0, 8.0),
            Some("resolved to the regular face")
        );

        let wrong_style =
            synthetic_face("bold italic", "Chosen Mono", "/fonts/italic.ttf", 80, 100);
        assert_eq!(
            style_candidate_rejection(&regular, &wrong_style, request, 8.0, 8.0),
            Some("did not resolve the requested weight")
        );

        assert_eq!(
            style_candidate_rejection(&regular, &compatible, request, 8.0, 8.5),
            Some("has incompatible terminal-cell metrics")
        );
    }

    #[test]
    fn unavailable_style_fallback_preserves_the_selected_regular_identity() {
        let regular = synthetic_face("regular", "Chosen Mono", "/fonts/regular.ttf", 80, 0);
        let fallback = regular.fallback_for("primary bold");
        assert_eq!(fallback.family, "Chosen Mono");
        assert_eq!(fallback.identity(), regular.identity());
        assert_eq!(fallback.weight, regular.weight);
        assert_eq!(fallback.slant, regular.slant);
    }

    #[test]
    fn effective_system_monospace_resolves_one_coherent_primary_family() {
        let faces = resolve_primary_faces("monospace:style=Regular").unwrap();
        let regular_family = normalize_family(&faces[0].family);
        assert!(!regular_family.is_empty());
        for face in &faces[1..] {
            assert_eq!(normalize_family(&face.family), regular_family);
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
}
