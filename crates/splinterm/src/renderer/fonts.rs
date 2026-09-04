//! Font discovery, fallback, shaping, metrics, and persistent glyph caches.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use anyhow::{Context, Result, bail};
use splinterm_filemap::{FileIdentity, ReadOnlyFileMap};
use splinterm_freetype::{MAX_STAGED_FONT_BYTES, RasterFace};
use swash::{
    FontRef, NormalizedCoord,
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    shape::ShapeContext,
    zeno::Format,
};

use crate::{
    box_drawing,
    config::{FontAuthority, STARTUP_FONT_FALLBACK},
};

use super::raster::{blend_glyph, fill_rect};
use super::{BASE_FONT_SIZE, PRIMARY_FONT, effective_font_size, renderer_options};

pub(super) const BASE_ROW_X: i32 = 32;
pub(super) const BASE_ROW_Y: i32 = 96;
pub(super) const CJK_FONT: &str = "Noto Sans CJK JP:style=Regular";
pub(super) const EMOJI_FONT: &str = "Noto Color Emoji";
pub(super) const SNAPSHOT_GLYPH_CACHE_BUDGET: usize = 2_048;
pub(super) const SNAPSHOT_GLYPH_CACHE_BYTE_BUDGET: usize = 64 * 1024 * 1024;
pub(super) const SNAPSHOT_RASTER_FACE_BUDGET: usize = 24;
pub(super) const SNAPSHOT_FALLBACK_MAPPING_BUDGET: usize = 24;

pub(super) const SNAPSHOT_PRIMARY_REGULAR: usize = 0;
pub(super) const SNAPSHOT_PRIMARY_BOLD: usize = 1;
pub(super) const SNAPSHOT_PRIMARY_ITALIC: usize = 2;
pub(super) const SNAPSHOT_PRIMARY_BOLD_ITALIC: usize = 3;
pub(super) const SNAPSHOT_CJK: usize = 4;
pub(super) const SNAPSHOT_EMOJI: usize = 5;
pub(super) const SNAPSHOT_FALLBACK_START: usize = 6;

static NEXT_FONT_GENERATION_ID: AtomicU64 = AtomicU64::new(1);
pub(super) static SNAPSHOT_FONT_GENERATION: OnceLock<Result<Arc<FontGeneration>, String>> =
    OnceLock::new();
#[derive(Default)]
pub(super) struct PersistentFontMappingCache {
    pub(super) mappings: HashMap<(PathBuf, FileIdentity), Arc<ReadOnlyFileMap>>,
    pub(super) order: VecDeque<(PathBuf, FileIdentity)>,
    pub(super) hits: u64,
    pub(super) misses: u64,
    pub(super) evictions: u64,
}

impl PersistentFontMappingCache {
    fn insert(
        &mut self,
        key: (PathBuf, FileIdentity),
        mapping: Arc<ReadOnlyFileMap>,
    ) -> Arc<ReadOnlyFileMap> {
        while self.mappings.len() >= SNAPSHOT_FALLBACK_MAPPING_BUDGET {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if self.mappings.remove(&oldest).is_some() {
                self.evictions = self.evictions.saturating_add(1);
            }
        }
        self.order.push_back(key.clone());
        self.mappings.insert(key, Arc::clone(&mapping));
        mapping
    }
}

#[derive(Default)]
pub(super) struct PersistentGlyphCache {
    pub(super) raster_faces: HashMap<(u64, isize, usize), RasterFace>,
    pub(super) raster_face_order: VecDeque<(u64, isize, usize)>,
    pub(super) glyphs: HashMap<(u64, isize, GlyphKey), Arc<CachedGlyph>>,
    pub(super) advances: HashMap<(u64, isize, GlyphKey), i32>,
    pub(super) order: VecDeque<(u64, isize, GlyphKey)>,
    pub(super) glyph_bytes: usize,
    pub(super) hits: u64,
    pub(super) misses: u64,
    pub(super) evictions: u64,
    pub(super) raster_face_evictions: u64,
}

impl PersistentGlyphCache {
    pub(super) fn insert_glyph(
        &mut self,
        cache_key: (u64, isize, GlyphKey),
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
        cache_key: (u64, isize, GlyphKey),
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

    pub(super) fn prepare_raster_face_insert(&mut self, raster_key: (u64, isize, usize)) {
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
    pub(super) static SNAPSHOT_FALLBACK_MAPPING_CACHE: RefCell<PersistentFontMappingCache> =
        RefCell::new(PersistentFontMappingCache::default());
}

pub(crate) fn clear_snapshot_caches() {
    SNAPSHOT_GLYPH_CACHE.with(|cache| *cache.borrow_mut() = PersistentGlyphCache::default());
    SNAPSHOT_FALLBACK_MAPPING_CACHE.with(|cache| {
        *cache.borrow_mut() = PersistentFontMappingCache::default();
    });
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

/// Fontconfig exposes `FC_INDEX` using `FreeType`'s packed face index: the low
/// 16 bits select a face in a collection and bits 16-30 select a one-based
/// named variable instance. Swash expects only the collection index.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct FontconfigFaceIndex(u32);

impl FontconfigFaceIndex {
    const COLLECTION_MASK: u32 = 0xffff;
    const NAMED_INSTANCE_MASK: u32 = 0x7fff;

    const fn new(raw: u32) -> Self {
        Self(raw)
    }

    fn from_fontconfig(raw: u32) -> Result<Self> {
        anyhow::ensure!(
            raw & 0x8000_0000 == 0,
            "Fontconfig face index sets reserved bit 31"
        );
        Ok(Self::new(raw))
    }

    pub(super) const fn raw(self) -> u32 {
        self.0
    }

    pub(super) const fn collection_index(self) -> usize {
        (self.0 & Self::COLLECTION_MASK) as usize
    }

    const fn named_instance_index(self) -> Option<usize> {
        let one_based = (self.0 >> 16) & Self::NAMED_INSTANCE_MASK;
        if one_based == 0 {
            None
        } else {
            Some((one_based - 1) as usize)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FontFaceFingerprint {
    pub(super) family: String,
    pub(super) style: String,
    pub(super) path: PathBuf,
    pub(super) index: u32,
    pub(super) weight: i32,
    pub(super) slant: i32,
    pub(super) selected_pixel_size_26_6: isize,
    pub(super) source_identity: FileIdentity,
}

#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontFingerprint {
    pub(super) pattern: String,
    pub(super) authority: FontAuthority,
    pub(super) faces: Vec<FontFaceFingerprint>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct FontGeneration {
    pub(super) id: u64,
    pub(super) fingerprint: FontFingerprint,
    pub(super) faces: Vec<FontFace>,
}

impl FontGeneration {
    /// Returns the process-local monotonic generation identity.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Returns the stable effective source fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &FontFingerprint {
        &self.fingerprint
    }
}

#[derive(Debug)]
pub(super) struct FontFace {
    pub(super) label: &'static str,
    pub(super) family: String,
    pub(super) style: String,
    pub(super) path: PathBuf,
    pub(super) index: FontconfigFaceIndex,
    pub(super) weight: i32,
    pub(super) slant: i32,
    pub(super) generation_id: u64,
    pub(super) selected_pixel_size_26_6: isize,
    pub(super) source_identity: FileIdentity,
    pub(super) outline: bool,
    pub(super) coverage: Box<[(u32, u32)]>,
    pub(super) data: Option<Arc<ReadOnlyFileMap>>,
    normalized_coords: OnceLock<Result<Box<[NormalizedCoord]>, String>>,
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
            generation_id: self.generation_id,
            selected_pixel_size_26_6: self.selected_pixel_size_26_6,
            source_identity: self.source_identity,
            outline: self.outline,
            coverage: self.coverage.clone(),
            data: self.data.clone(),
            normalized_coords: OnceLock::new(),
        }
    }

    fn identity(&self) -> (&std::path::Path, u32) {
        (&self.path, self.index.raw())
    }

    pub(super) fn covers_text(&self, text: &str) -> bool {
        text.chars().all(|character| {
            let codepoint = u32::from(character);
            self.coverage
                .iter()
                .any(|(start, end)| (*start..=*end).contains(&codepoint))
        })
    }

    fn fingerprint(&self) -> FontFaceFingerprint {
        FontFaceFingerprint {
            family: self.family.clone(),
            style: self.style.clone(),
            path: self.path.clone(),
            index: self.index.raw(),
            weight: self.weight,
            slant: self.slant,
            selected_pixel_size_26_6: self.selected_pixel_size_26_6,
            source_identity: self.source_identity,
        }
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
        let [primary, _, _, _] = resolve_primary_faces(PRIMARY_FONT, FontAuthority::Explicit)?;
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
                let (font, coords) = font_ref_with_coords(&faces[face_index])?;
                let mut shaped_glyphs = Vec::new();
                let mut shaper = shape_context
                    .builder(font)
                    .size(font_size)
                    .normalized_coords(coords)
                    .build();
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
                let (font, coords) = font_ref_with_coords(&faces[face_index])?;
                let advance = font
                    .glyph_metrics(coords)
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
    let (font, coords) = font_ref_with_coords(&faces[face_index])?;
    let mut scaler = context
        .builder(font)
        .size(font_size)
        .hint(true)
        .normalized_coords(coords)
        .build();
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

/// Returns the configured immutable startup generation.
///
/// # Errors
/// Returns the retained startup staging failure.
#[doc(hidden)]
pub fn snapshot_font_generation() -> Result<&'static Arc<FontGeneration>> {
    SNAPSHOT_FONT_GENERATION
        .get_or_init(|| {
            let options = renderer_options();
            #[cfg(not(test))]
            let generation = stage_startup_font_generation(&options.font, options.font_authority);
            #[cfg(test)]
            let generation = stage_renderer_test_generation(&options.font, options.font_authority);
            generation.map(Arc::new).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

pub(super) fn snapshot_faces() -> Result<&'static [FontFace]> {
    Ok(&snapshot_font_generation()?.faces)
}

pub(super) fn snapshot_glyph(
    faces: &[FontFace],
    face_index: usize,
    glyph_id: u16,
    font_size: f32,
) -> Result<Arc<CachedGlyph>> {
    let effective_size_26_6 = pixel_size_26_6(font_size)?;
    let generation_id = faces
        .get(face_index)
        .context("glyph face index is in the active font generation")?
        .generation_id;
    let key = GlyphKey {
        face: face_index,
        glyph: glyph_id,
    };
    SNAPSHOT_GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(glyph) = cache
            .glyphs
            .get(&(generation_id, effective_size_26_6, key))
            .cloned()
        {
            cache.hits = cache.hits.saturating_add(1);
            return Ok(glyph);
        }
        cache.misses = cache.misses.saturating_add(1);
        let (glyph, color_advance) = if face_index == SNAPSHOT_EMOJI {
            let face = &faces[face_index];
            let raster_key = (generation_id, face.selected_pixel_size_26_6, face_index);
            if !cache.raster_faces.contains_key(&raster_key) {
                let raster_face = RasterFace::open_memory(
                    face.data.clone().context("emoji face is staged")?,
                    face.index.raw(),
                    face.selected_pixel_size_26_6,
                )
                .context("open staged emoji raster face")?;
                cache.prepare_raster_face_insert(raster_key);
                cache.raster_faces.insert(raster_key, raster_face);
            }
            let raster = cache
                .raster_faces
                .get_mut(&raster_key)
                .context("inserted emoji raster face remains present")?
                .rasterize_color_glyph(
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
            let raster_key = (generation_id, effective_size_26_6, face_index);
            if !cache.raster_faces.contains_key(&raster_key) {
                let face = &faces[face_index];
                let pixel_size = pixel_size_26_6(font_size)?;
                let data = if let Some(data) = &face.data {
                    Arc::clone(data)
                } else {
                    fallback_font_mapping(face)?
                };
                let raster_face = RasterFace::open_memory(data, face.index.raw(), pixel_size)
                    .with_context(|| format!("open FreeType raster face {}", face.label))?;
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
            (generation_id, effective_size_26_6, key),
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
    faces: &[FontFace],
    face_index: usize,
    glyph_id: u16,
    font_size: f32,
) -> Result<i32> {
    let cache_key = (
        faces
            .get(face_index)
            .context("color face index is in the active font generation")?
            .generation_id,
        pixel_size_26_6(font_size)?,
        GlyphKey {
            face: face_index,
            glyph: glyph_id,
        },
    );
    if let Some(advance) =
        SNAPSHOT_GLYPH_CACHE.with(|cache| cache.borrow().advances.get(&cache_key).copied())
    {
        return Ok(advance);
    }
    snapshot_glyph(faces, face_index, glyph_id, font_size)?;
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
    clear_snapshot_caches();
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
    SNAPSHOT_GLYPH_CACHE.with(|glyph_cache| {
        SNAPSHOT_FALLBACK_MAPPING_CACHE.with(|mapping_cache| {
            let glyph_cache = glyph_cache.borrow();
            let mapping_cache = mapping_cache.borrow();
            serde_json::json!({
                "entries": glyph_cache.glyphs.len(),
                "raster_faces": glyph_cache.raster_faces.len(),
                "fallback_mappings": mapping_cache.mappings.len(),
                "budget": SNAPSHOT_GLYPH_CACHE_BUDGET,
                "glyph_budget": SNAPSHOT_GLYPH_CACHE_BUDGET,
                "glyph_byte_budget": SNAPSHOT_GLYPH_CACHE_BYTE_BUDGET,
                "raster_face_budget": SNAPSHOT_RASTER_FACE_BUDGET,
                "fallback_mapping_budget": SNAPSHOT_FALLBACK_MAPPING_BUDGET,
                "hits": glyph_cache.hits,
                "misses": glyph_cache.misses,
                "evictions": glyph_cache.evictions,
                "raster_face_evictions": glyph_cache.raster_face_evictions,
                "fallback_mapping_hits": mapping_cache.hits,
                "fallback_mapping_misses": mapping_cache.misses,
                "fallback_mapping_evictions": mapping_cache.evictions,
                "approximate_bytes": glyph_cache.glyph_bytes,
            })
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
    let (font, coords) = font_ref_with_coords(&face)?;
    let metrics = font.metrics(coords).scale(font_size);
    let font_ascent = ceil_to_i32(metrics.ascent);
    let font_descent = ceil_to_i32(metrics.descent);
    let resolved_metrics = cell_metrics(&face, font_size)?;
    let font_height = i32::try_from(resolved_metrics.height).context("font height fits i32")?;
    let glyph_metrics = font.glyph_metrics(coords).scale(font_size);
    let mut context = ScaleContext::new();
    let mut records = Vec::with_capacity(95);

    for codepoint in 0x20_u32..=0x7e {
        let character = char::from_u32(codepoint).context("printable ASCII is valid Unicode")?;
        let glyph_id = font.charmap().map(character);
        let advance = positive_round_to_u32(glyph_metrics.advance_width(glyph_id));
        let mut scaler = context
            .builder(font)
            .size(font_size)
            .hint(true)
            .normalized_coords(coords)
            .build();
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
            "font_index": face.index.raw(),
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
    let (font, coords) = font_ref_with_coords(face)?;
    let metrics = cell_metrics(face, font_size)?;
    let glyph_metrics = font.glyph_metrics(coords).scale(font_size);
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
            "font_index": face.index.raw(),
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
    let (fallback_font, fallback_coords) = font_ref_with_coords(fallback_face)?;
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
    let fallback_metrics = fallback_font
        .glyph_metrics(fallback_coords)
        .scale(font_size);
    records.push(serde_json::json!({
        "schema": 1,
        "label": "CJK",
        "codepoint": codepoint,
        "cols": 2,
        "glyph_id": glyph_id,
        "font": fallback_face.family.as_str(),
        "font_path": fallback_face.path.display().to_string(),
        "font_index": fallback_face.index.raw(),
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
    let emoji_advance = snapshot_color_advance(faces, SNAPSHOT_EMOJI, glyph_id, font_size)?;
    records.push(serde_json::json!({
        "schema": 1,
        "label": "emoji",
        "codepoint": codepoint,
        "cols": 2,
        "glyph_id": glyph_id,
        "font": emoji_face.family.as_str(),
        "font_path": emoji_face.path.display().to_string(),
        "font_index": emoji_face.index.raw(),
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
    let mut shaper = shape_context
        .builder(font)
        .size(font_size)
        .normalized_coords(coords)
        .build();
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
        "font_index": face.index.raw(),
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
    let mut face = RasterFace::open_memory(
        primary_face
            .data
            .clone()
            .context("primary face is staged")?,
        primary_face.index.raw(),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedFaceSource {
    label: &'static str,
    family: String,
    style: String,
    path: PathBuf,
    index: FontconfigFaceIndex,
    weight: i32,
    slant: i32,
    selected_pixel_size_26_6: isize,
    source_identity: FileIdentity,
    outline: bool,
    coverage: Box<[(u32, u32)]>,
}

impl ResolvedFaceSource {
    fn fingerprint(&self) -> FontFaceFingerprint {
        FontFaceFingerprint {
            family: self.family.clone(),
            style: self.style.clone(),
            path: self.path.clone(),
            index: self.index.raw(),
            weight: self.weight,
            slant: self.slant,
            selected_pixel_size_26_6: self.selected_pixel_size_26_6,
            source_identity: self.source_identity,
        }
    }
}

const FONTCONFIG_FACE_FORMAT: &str = "%{file}\\n%{index}\\n%{family[0]}\\n%{style}\\n%{weight}\\n%{slant}\\n%{pixelsize}\\n%{outline}\\n%{charset}\\n--record--\\n";

fn parse_fontconfig_charset(value: &str) -> Result<Box<[(u32, u32)]>> {
    value
        .split_whitespace()
        .map(|range| {
            let (start, end) = range.split_once('-').unwrap_or((range, range));
            let start = u32::from_str_radix(start, 16)
                .with_context(|| format!("invalid Fontconfig charset start {start:?}"))?;
            let end = u32::from_str_radix(end, 16)
                .with_context(|| format!("invalid Fontconfig charset end {end:?}"))?;
            anyhow::ensure!(
                start <= end && end <= u32::from(char::MAX),
                "invalid Fontconfig charset range {range:?}"
            );
            Ok((start, end))
        })
        .collect::<Result<Vec<_>>>()
        .map(Vec::into_boxed_slice)
}

fn parse_fontconfig_sources(label: &'static str, stdout: &[u8]) -> Result<Vec<ResolvedFaceSource>> {
    let stdout = std::str::from_utf8(stdout).context("fc-match output is not UTF-8")?;
    stdout
        .split("--record--\n")
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let mut lines = record.lines();
            let path = PathBuf::from(
                lines
                    .next()
                    .with_context(|| format!("fc-match returned no font path for {label}"))?,
            );
            let index = FontconfigFaceIndex::from_fontconfig(
                lines
                    .next()
                    .with_context(|| format!("fc-match returned no face index for {label}"))?
                    .parse::<u32>()
                    .with_context(|| {
                        format!("fc-match returned a non-numeric face index for {label}")
                    })?,
            )?;
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
            let outline = match lines
                .next()
                .with_context(|| format!("fc-match returned no outline flag for {label}"))?
            {
                "True" => true,
                "False" => false,
                value => bail!("fc-match returned an invalid outline flag {value:?} for {label}"),
            };
            let coverage = parse_fontconfig_charset(
                lines
                    .next()
                    .with_context(|| format!("fc-match returned no charset for {label}"))?,
            )?;
            let metadata = std::fs::metadata(&path)
                .with_context(|| format!("inspect resolved {label} font {}", path.display()))?;
            anyhow::ensure!(
                metadata.is_file() && metadata.len() > 0,
                "resolved font is not a non-empty regular file"
            );
            Ok(ResolvedFaceSource {
                label,
                family,
                style,
                path,
                index,
                weight,
                slant,
                selected_pixel_size_26_6: pixel_size_26_6(selected_pixel_size)?,
                source_identity: FileIdentity {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    length: metadata.len(),
                    modified_seconds: metadata.mtime(),
                    modified_nanoseconds: metadata.mtime_nsec(),
                },
                outline,
                coverage,
            })
        })
        .collect()
}

fn run_fontconfig_sources(
    label: &'static str,
    pattern: &str,
    sorted: bool,
) -> Result<Vec<ResolvedFaceSource>> {
    let mut command = Command::new("fc-match");
    if sorted {
        command.arg("-s");
    }
    let output = command
        .args(["-f", FONTCONFIG_FACE_FORMAT, pattern])
        .output()
        .with_context(|| format!("run fc-match for {label}"))?;
    if !output.status.success() {
        bail!(
            "fc-match failed for {label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    parse_fontconfig_sources(label, &output.stdout)
}

fn resolve_face_source(
    label: &'static str,
    pattern: &str,
    expected_family_fragment: &str,
) -> Result<ResolvedFaceSource> {
    let source = run_fontconfig_sources(label, pattern, false)?
        .into_iter()
        .next()
        .with_context(|| format!("fc-match returned no face for {label}"))?;
    let normalized_family = normalize_family(&source.family);
    let normalized_expected = normalize_family(expected_family_fragment);
    if !normalized_expected.is_empty() && !normalized_family.contains(&normalized_expected) {
        bail!(
            "explicit {label} pattern {pattern:?} resolved unexpectedly to {:?}",
            source.family
        );
    }
    Ok(source)
}

fn stage_face(source: ResolvedFaceSource) -> Result<FontFace> {
    let snapshot = ReadOnlyFileMap::immutable_snapshot(&source.path, MAX_STAGED_FONT_BYTES)
        .with_context(|| format!("snapshot resolved {} font", source.label))?;
    anyhow::ensure!(
        snapshot.source_identity == source.source_identity,
        "resolved font source changed before staging"
    );
    let data = Arc::new(snapshot.mapping);
    let face = FontFace {
        label: source.label,
        family: source.family,
        style: source.style,
        path: source.path,
        index: source.index,
        weight: source.weight,
        slant: source.slant,
        generation_id: 0,
        selected_pixel_size_26_6: source.selected_pixel_size_26_6,
        source_identity: source.source_identity,
        outline: source.outline,
        coverage: source.coverage,
        data: Some(data),
        normalized_coords: OnceLock::new(),
    };
    font_ref(&face).with_context(|| format!("parse resolved {} font", face.label))?;
    eprintln!(
        "Resolved {}: {} {} (face {}, {})",
        face.label,
        face.family,
        face.style,
        face.index.raw(),
        face.path.display()
    );
    Ok(face)
}

fn unstaged_fallback_face(source: ResolvedFaceSource) -> FontFace {
    FontFace {
        label: source.label,
        family: source.family,
        style: source.style,
        path: source.path,
        index: source.index,
        weight: source.weight,
        slant: source.slant,
        generation_id: 0,
        selected_pixel_size_26_6: source.selected_pixel_size_26_6,
        source_identity: source.source_identity,
        outline: source.outline,
        coverage: source.coverage,
        data: None,
        normalized_coords: OnceLock::new(),
    }
}

fn resolve_fallback_sources(
    pattern: &str,
    excluded: &HashSet<(PathBuf, FontconfigFaceIndex)>,
) -> Result<Vec<ResolvedFaceSource>> {
    let mut identities = excluded.clone();
    Ok(
        run_fontconfig_sources("fontconfig fallback", pattern, true)?
            .into_iter()
            .filter(|source| {
                source.outline && identities.insert((source.path.clone(), source.index))
            })
            .collect(),
    )
}

pub(super) fn resolve_face(
    label: &'static str,
    pattern: &str,
    expected_family_fragment: &str,
) -> Result<FontFace> {
    stage_face(resolve_face_source(
        label,
        pattern,
        expected_family_fragment,
    )?)
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

fn primary_style_pattern(family: &str, request: PrimaryStyleRequest) -> String {
    let weight = if request.bold { "bold" } else { "regular" };
    let slant = if request.italic { "italic" } else { "roman" };
    format!(
        "{}:weight={weight}:slant={slant}",
        escape_fontconfig_pattern_value(family)
    )
}

fn probe_primary_style_source(
    regular: &ResolvedFaceSource,
    request: PrimaryStyleRequest,
) -> ResolvedFaceSource {
    let pattern = primary_style_pattern(&regular.family, request);
    let candidate = resolve_face_source(request.label, &pattern, &regular.family);
    match candidate {
        Ok(candidate)
            if normalize_family(&candidate.family) == normalize_family(&regular.family)
                && (candidate.path != regular.path || candidate.index != regular.index)
                && request.bold == (candidate.weight > regular.weight)
                && request.italic == (candidate.slant != regular.slant) =>
        {
            candidate
        }
        _ => {
            let mut fallback = regular.clone();
            fallback.label = request.label;
            fallback
        }
    }
}

/// Probes the effective fontconfig source identities without mapping font bytes.
///
/// # Errors
/// Returns an error when Fontconfig output or selected source metadata is invalid.
#[doc(hidden)]
pub fn probe_live_font_sources(pattern: &str, authority: FontAuthority) -> Result<FontFingerprint> {
    let regular = resolve_face_source(
        "primary regular",
        pattern,
        expected_primary_family_fragment(pattern),
    )?;
    let [bold_request, italic_request, bold_italic_request] = PRIMARY_STYLE_REQUESTS;
    let mut sources = Vec::from([
        regular.clone(),
        probe_primary_style_source(&regular, bold_request),
        probe_primary_style_source(&regular, italic_request),
        probe_primary_style_source(&regular, bold_italic_request),
        resolve_face_source("CJK fallback", CJK_FONT, "noto sans cjk")?,
        resolve_face_source("emoji fallback", EMOJI_FONT, "noto color emoji")?,
    ]);
    let excluded = sources
        .iter()
        .map(|source| (source.path.clone(), source.index))
        .collect::<HashSet<_>>();
    sources.extend(resolve_fallback_sources(pattern, &excluded)?);
    Ok(FontFingerprint {
        pattern: pattern.to_owned(),
        authority,
        faces: sources
            .iter()
            .map(ResolvedFaceSource::fingerprint)
            .collect(),
    })
}

fn face_advance(face: &FontFace, font_size: f32) -> Result<f32> {
    let (font, coords) = font_ref_with_coords(face)?;
    let advance = font
        .glyph_metrics(coords)
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
    let pattern = primary_style_pattern(&regular.family, request);
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

fn expected_primary_family_fragment(pattern: &str) -> &'static str {
    if pattern == STARTUP_FONT_FALLBACK {
        "jetbrains mono"
    } else {
        ""
    }
}

fn resolve_primary_faces_exact(pattern: &str) -> Result<[FontFace; 4]> {
    let regular = resolve_face(
        "primary regular",
        pattern,
        expected_primary_family_fragment(pattern),
    )?;
    let regular_advance = face_advance(&regular, BASE_FONT_SIZE)
        .context("selected primary regular face is unusable")?;
    let [bold_request, italic_request, bold_italic_request] = PRIMARY_STYLE_REQUESTS;
    let bold = resolve_primary_style(&regular, bold_request, regular_advance);
    let italic = resolve_primary_style(&regular, italic_request, regular_advance);
    let bold_italic = resolve_primary_style(&regular, bold_italic_request, regular_advance);
    Ok([regular, bold, italic, bold_italic])
}

fn resolve_startup_primary_with<T>(
    pattern: &str,
    authority: FontAuthority,
    mut resolve: impl FnMut(&str) -> Result<T>,
) -> Result<T> {
    match resolve(pattern) {
        Ok(resolved) => Ok(resolved),
        Err(error) if authority == FontAuthority::Explicit => Err(error).with_context(|| {
            format!("explicit primary font pattern {pattern:?} could not be resolved")
        }),
        Err(native_error) => {
            eprintln!(
                "splinterm font warning: native system monospace resolution failed ({native_error:#}); trying the documented JetBrains Mono Nerd Font fallback"
            );
            resolve(STARTUP_FONT_FALLBACK).with_context(|| {
                format!(
                    "native primary font pattern {pattern:?} failed ({native_error:#}) and startup fallback {STARTUP_FONT_FALLBACK:?} could not be resolved"
                )
            })
        }
    }
}

fn resolve_primary_faces(pattern: &str, authority: FontAuthority) -> Result<[FontFace; 4]> {
    resolve_startup_primary_with(pattern, authority, resolve_primary_faces_exact)
}

fn resolve_font_candidate(
    pattern: &str,
    authority: FontAuthority,
    startup: bool,
) -> Result<FontGeneration> {
    let [regular, bold, italic, bold_italic] = if startup {
        resolve_primary_faces(pattern, authority)?
    } else {
        resolve_primary_faces_exact(pattern)?
    };
    let id = NEXT_FONT_GENERATION_ID.fetch_add(1, Ordering::Relaxed);
    let mut faces = Vec::from([
        regular,
        bold,
        italic,
        bold_italic,
        resolve_face("CJK fallback", CJK_FONT, "noto sans cjk")?,
        resolve_face("emoji fallback", EMOJI_FONT, "noto color emoji")?,
    ]);
    let excluded = faces
        .iter()
        .map(|face| (face.path.clone(), face.index))
        .collect::<HashSet<_>>();
    let fallback_sources = resolve_fallback_sources(pattern, &excluded)?;
    eprintln!(
        "Resolved {} ordered Fontconfig fallback faces",
        fallback_sources.len()
    );
    faces.extend(fallback_sources.into_iter().map(unstaged_fallback_face));
    for face in &mut faces {
        face.generation_id = id;
    }
    let fingerprint = FontFingerprint {
        pattern: pattern.to_owned(),
        authority,
        faces: faces.iter().map(FontFace::fingerprint).collect(),
    };
    Ok(FontGeneration {
        id,
        fingerprint,
        faces,
    })
}

fn stage_stable_with<F, T>(mut stage: impl FnMut() -> Result<(F, T)>) -> Result<T>
where
    F: std::fmt::Debug + Eq,
{
    let (first_fingerprint, first) = stage()?;
    let (second_fingerprint, second) = stage()?;
    anyhow::ensure!(
        first_fingerprint == second_fingerprint,
        "font resolution changed while staging: first={first_fingerprint:?}, second={second_fingerprint:?}"
    );
    drop(first);
    Ok(second)
}

fn stage_font_generation(
    pattern: &str,
    authority: FontAuthority,
    startup: bool,
) -> Result<FontGeneration> {
    stage_stable_with(|| {
        let generation = resolve_font_candidate(pattern, authority, startup)?;
        Ok((generation.fingerprint.clone(), generation))
    })
}

fn stage_startup_font_generation(
    pattern: &str,
    authority: FontAuthority,
) -> Result<FontGeneration> {
    stage_font_generation(pattern, authority, true)
}

#[cfg(test)]
fn pinned_startup_font_is_available() -> Result<bool> {
    let sources = run_fontconfig_sources(
        "pinned renderer test prerequisite",
        STARTUP_FONT_FALLBACK,
        false,
    )?;
    let source = sources
        .first()
        .context("fc-match returned no pinned renderer test prerequisite")?;
    Ok(normalize_family(&source.family).contains(&normalize_family("jetbrains mono")))
}

#[cfg(test)]
fn use_host_renderer_test_font(pattern: &str, pinned_startup_font_available: bool) -> bool {
    pattern == STARTUP_FONT_FALLBACK && !pinned_startup_font_available
}

/// Stages the pinned renderer-test generation, falling back only when Fontconfig
/// positively reports that the pinned `JetBrains` family is absent.
#[cfg(test)]
pub(super) fn stage_renderer_test_generation(
    pattern: &str,
    authority: FontAuthority,
) -> Result<FontGeneration> {
    if !use_host_renderer_test_font(pattern, pinned_startup_font_is_available()?) {
        return stage_startup_font_generation(pattern, authority);
    }
    stage_live_font_generation("monospace:style=Regular", FontAuthority::Explicit)
        .context("stage the host's generic monospace renderer-test generation")
}

/// Stages one stable live generation without applying the startup fallback.
///
/// # Errors
/// Returns an error when resolution is unstable or any complete face set is invalid.
#[doc(hidden)]
pub fn stage_live_font_generation(
    pattern: &str,
    authority: FontAuthority,
) -> Result<FontGeneration> {
    stage_font_generation(pattern, authority, false)
}

pub(super) fn font_data(face: &FontFace) -> Result<&[u8]> {
    face.data
        .as_deref()
        .map(|mapping| &**mapping)
        .context("fallback font data requires the bounded mapping cache")
}

fn parse_font_ref<'a>(face: &FontFace, data: &'a [u8]) -> Result<FontRef<'a>> {
    FontRef::from_index(data, face.index.collection_index()).with_context(|| {
        format!(
            "parse {} Fontconfig face {} (collection face {}) with Swash",
            face.path.display(),
            face.index.raw(),
            face.index.collection_index()
        )
    })
}

fn normalized_coords<'a>(face: &'a FontFace, font: FontRef<'_>) -> Result<&'a [NormalizedCoord]> {
    face.normalized_coords
        .get_or_init(|| {
            let Some(instance_index) = face.index.named_instance_index() else {
                return Ok(Box::new([]));
            };
            let Some(instance) = font.instances().nth(instance_index) else {
                return Err(format!(
                    "Fontconfig face {} selects missing named instance {} in {}",
                    face.index.raw(),
                    instance_index + 1,
                    face.path.display()
                ));
            };
            Ok(instance
                .normalized_coords()
                .collect::<Vec<_>>()
                .into_boxed_slice())
        })
        .as_ref()
        .map(Box::as_ref)
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

pub(super) fn font_ref(face: &FontFace) -> Result<FontRef<'_>> {
    let font = parse_font_ref(face, font_data(face)?)?;
    normalized_coords(face, font)?;
    Ok(font)
}

fn font_ref_with_coords(face: &FontFace) -> Result<(FontRef<'_>, &[NormalizedCoord])> {
    let font = font_ref(face)?;
    let coords = normalized_coords(face, font)?;
    Ok((font, coords))
}

fn fallback_font_mapping(face: &FontFace) -> Result<Arc<ReadOnlyFileMap>> {
    let key = (face.path.clone(), face.source_identity);
    SNAPSHOT_FALLBACK_MAPPING_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(mapping) = cache.mappings.get(&key).cloned() {
            cache.hits = cache.hits.saturating_add(1);
            return Ok(mapping);
        }
        cache.misses = cache.misses.saturating_add(1);
        let mapping = Arc::new(
            ReadOnlyFileMap::open(&face.path)
                .with_context(|| format!("map fallback {}", face.path.display()))?,
        );
        anyhow::ensure!(
            mapping.identity() == face.source_identity,
            "fallback font source changed before mapping"
        );
        Ok(cache.insert(key, mapping))
    })
}

pub(super) fn with_font_ref<T>(
    face: &FontFace,
    use_font: impl FnOnce(FontRef<'_>, &[NormalizedCoord]) -> Result<T>,
) -> Result<T> {
    if face.data.is_some() {
        let (font, coords) = font_ref_with_coords(face)?;
        return use_font(font, coords);
    }
    let mapping = fallback_font_mapping(face)?;
    let font = parse_font_ref(face, &mapping)?;
    let coords = normalized_coords(face, font)?;
    use_font(font, coords)
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
        static NEXT_SYNTHETIC_FACE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        let mapped_path = std::env::temp_dir().join(format!(
            "splinterm-synthetic-face-{}-{}",
            std::process::id(),
            NEXT_SYNTHETIC_FACE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&mapped_path, b"synthetic face bytes").unwrap();
        let data = Arc::new(ReadOnlyFileMap::open(&mapped_path).unwrap());
        std::fs::remove_file(mapped_path).unwrap();
        FontFace {
            label,
            family: family.to_owned(),
            style: label.to_owned(),
            path: PathBuf::from(path),
            index: FontconfigFaceIndex::new(0),
            weight,
            slant,
            generation_id: 0,
            selected_pixel_size_26_6: 12 * 64,
            source_identity: data.identity(),
            outline: true,
            coverage: vec![(0, u32::from(char::MAX))].into_boxed_slice(),
            data: Some(data),
            normalized_coords: OnceLock::new(),
        }
    }

    #[test]
    fn packed_fontconfig_face_index_separates_collection_and_named_instance() {
        let static_face = FontconfigFaceIndex::new(7);
        assert_eq!(static_face.raw(), 7);
        assert_eq!(static_face.collection_index(), 7);
        assert_eq!(static_face.named_instance_index(), None);

        let victor_regular = FontconfigFaceIndex::new(0x0004_0000);
        assert_eq!(victor_regular.raw(), 262_144);
        assert_eq!(victor_regular.collection_index(), 0);
        assert_eq!(victor_regular.named_instance_index(), Some(3));

        let collection_instance = FontconfigFaceIndex::new(0x0003_0004);
        assert_eq!(collection_instance.collection_index(), 4);
        assert_eq!(collection_instance.named_instance_index(), Some(2));
        assert!(FontconfigFaceIndex::from_fontconfig(0x8000_0000).is_err());
    }

    #[test]
    fn fontconfig_source_parser_preserves_packed_variable_instance_index() {
        let path = std::env::temp_dir().join(format!(
            "splinterm-fontconfig-index-fixture-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"font fixture").unwrap();
        let output = format!(
            "{}\n262144\nVariable Mono\nRegular\n80\n0\n14\nTrue\n20-7e\n--record--\n",
            path.display()
        );
        let sources = parse_fontconfig_sources("variable fixture", output.as_bytes()).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].index.raw(), 262_144);
        assert_eq!(sources[0].index.collection_index(), 0);
        assert_eq!(sources[0].index.named_instance_index(), Some(3));
    }

    #[test]
    #[ignore = "manual variable-font integration; requires SPLINTERM_VARIABLE_FONT_FIXTURE and SPLINTERM_VARIABLE_FONT_INDEX"]
    fn variable_font_named_instance_shapes_and_rasterizes_with_freetype_identity() {
        let path = PathBuf::from(
            std::env::var("SPLINTERM_VARIABLE_FONT_FIXTURE")
                .expect("SPLINTERM_VARIABLE_FONT_FIXTURE names a variable font"),
        );
        let raw_index = std::env::var("SPLINTERM_VARIABLE_FONT_INDEX")
            .expect("SPLINTERM_VARIABLE_FONT_INDEX names a Fontconfig packed index")
            .parse::<u32>()
            .expect("packed index is numeric");
        let snapshot = ReadOnlyFileMap::immutable_snapshot(&path, MAX_STAGED_FONT_BYTES).unwrap();
        let mut face = synthetic_face("variable", "Variable Mono", "/fonts/variable.ttf", 80, 0);
        face.path = path;
        face.index = FontconfigFaceIndex::from_fontconfig(raw_index).unwrap();
        face.source_identity = snapshot.source_identity;
        face.data = Some(Arc::new(snapshot.mapping));

        let (font, coords) = font_ref_with_coords(&face).unwrap();
        assert!(face.index.named_instance_index().is_some());
        assert!(!coords.is_empty());
        let glyph_id = font.charmap().map('M');
        assert_ne!(glyph_id, 0);
        let swash_advance = font
            .glyph_metrics(coords)
            .scale(BASE_FONT_SIZE)
            .advance_width(glyph_id);
        assert!(swash_advance.is_finite() && swash_advance > 0.0);

        let mut shape_context = ShapeContext::new();
        let mut shaped_glyph_ids = Vec::new();
        let mut shaper = shape_context
            .builder(font)
            .size(BASE_FONT_SIZE)
            .normalized_coords(coords)
            .build();
        shaper.add_str("Mono");
        shaper.shape_with(|cluster| {
            shaped_glyph_ids.extend(cluster.glyphs.iter().map(|glyph| glyph.id));
        });
        assert!(!shaped_glyph_ids.is_empty());

        let mut scale_context = ScaleContext::new();
        let mut scaler = scale_context
            .builder(font)
            .size(BASE_FONT_SIZE)
            .normalized_coords(coords)
            .hint(true)
            .build();
        assert!(
            Render::new(&[Source::Outline])
                .format(Format::Alpha)
                .render(&mut scaler, glyph_id)
                .is_some()
        );

        let mut freetype = RasterFace::open_memory(
            face.data.clone().unwrap(),
            face.index.raw(),
            pixel_size_26_6(BASE_FONT_SIZE).unwrap(),
        )
        .unwrap();
        assert_eq!(freetype.glyph_index('M'), u32::from(glyph_id));
        assert!(freetype.rasterize_gray(u32::from(glyph_id)).is_ok());
    }

    #[test]
    fn fontconfig_charset_parser_covers_singletons_ranges_and_non_bmp_codepoints() {
        let coverage = parse_fontconfig_charset("20-7e 21d5 1f4e6-1f4e7").unwrap();
        let mut face = synthetic_face("fallback", "Chosen Mono", "/fonts/fallback.ttf", 80, 0);
        face.coverage = coverage;
        assert!(face.covers_text("A⇕📦"));
        assert!(!face.covers_text("A🦀"));
        assert!(parse_fontconfig_charset("7e-20").is_err());
        assert!(parse_fontconfig_charset("110000").is_err());
    }

    #[test]
    fn fallback_font_mapping_cache_remains_bounded() {
        let face = synthetic_face("mapping", "Chosen Mono", "/fonts/mapping.ttf", 80, 0);
        let mapping = face.data.expect("synthetic face is mapped");
        let identity = mapping.identity();
        let mut cache = PersistentFontMappingCache::default();
        for index in 0..=SNAPSHOT_FALLBACK_MAPPING_BUDGET {
            cache.insert(
                (
                    PathBuf::from(format!("/fonts/fallback-{index}.ttf")),
                    identity,
                ),
                Arc::clone(&mapping),
            );
        }
        assert_eq!(cache.mappings.len(), SNAPSHOT_FALLBACK_MAPPING_BUDGET);
        assert_eq!(cache.evictions, 1);
    }

    #[test]
    fn style_patterns_use_fontconfig_weight_and_slant_for_the_selected_family() {
        let bold_italic = PRIMARY_STYLE_REQUESTS[2];
        assert_eq!(
            primary_style_pattern("Chosen-Family, Mono: Propo", bold_italic),
            "Chosen\\-Family\\, Mono\\: Propo:weight=bold:slant=italic"
        );
        assert_eq!(
            primary_style_pattern("Chosen", PRIMARY_STYLE_REQUESTS[0]),
            "Chosen:weight=bold:slant=roman"
        );
        assert_eq!(
            primary_style_pattern("Chosen", PRIMARY_STYLE_REQUESTS[1]),
            "Chosen:weight=regular:slant=italic"
        );
        assert!(!primary_style_pattern("Chosen", bold_italic).contains("JetBrains"));
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
        let compatible_oblique = synthetic_face(
            "bold oblique",
            "Chosen Mono",
            "/fonts/bold-oblique.ttf",
            200,
            110,
        );
        assert_eq!(
            style_candidate_rejection(&regular, &compatible_oblique, request, 8.0, 8.0),
            None,
            "Fontconfig oblique faces satisfy an italic slant request"
        );

        let mut named_instance =
            synthetic_face("bold italic", "Chosen Mono", "/fonts/regular.ttf", 200, 100);
        named_instance.index = FontconfigFaceIndex::new(0x0007_0000);
        assert_eq!(
            style_candidate_rejection(&regular, &named_instance, request, 8.0, 8.0),
            None,
            "a distinct named instance in the same variable-font file is a distinct face"
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
    fn documented_startup_fallback_requires_the_jetbrains_family() {
        assert_eq!(
            expected_primary_family_fragment(STARTUP_FONT_FALLBACK),
            "jetbrains mono"
        );
        assert_eq!(expected_primary_family_fragment("monospace"), "");
    }

    #[test]
    fn host_test_font_is_used_only_when_the_pinned_default_is_absent() {
        assert!(!use_host_renderer_test_font(STARTUP_FONT_FALLBACK, true));
        assert!(use_host_renderer_test_font(STARTUP_FONT_FALLBACK, false));
        assert!(!use_host_renderer_test_font(
            "explicit configured font",
            false
        ));
    }

    #[test]
    fn native_startup_tries_the_documented_fallback_after_resolution_failure() {
        let mut patterns = Vec::new();
        let resolved = resolve_startup_primary_with(
            "monospace:style=Regular",
            FontAuthority::NativeOmarchy,
            |pattern| {
                patterns.push(pattern.to_owned());
                if pattern == STARTUP_FONT_FALLBACK {
                    Ok("fallback")
                } else {
                    bail!("native unavailable")
                }
            },
        )
        .unwrap();
        assert_eq!(resolved, "fallback");
        assert_eq!(
            patterns,
            [
                "monospace:style=Regular".to_owned(),
                STARTUP_FONT_FALLBACK.to_owned()
            ]
        );
    }

    #[test]
    fn explicit_startup_never_uses_the_native_fallback() {
        let mut patterns = Vec::new();
        let error = resolve_startup_primary_with(
            "monospace:style=Regular",
            FontAuthority::Explicit,
            |pattern| -> Result<()> {
                patterns.push(pattern.to_owned());
                bail!("explicit unavailable")
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("explicit primary font pattern"));
        assert_eq!(patterns, ["monospace:style=Regular".to_owned()]);
    }

    #[test]
    fn native_startup_fails_when_native_and_fallback_resolution_fail() {
        let error = resolve_startup_primary_with(
            "monospace:style=Regular",
            FontAuthority::NativeOmarchy,
            |_pattern| -> Result<()> { bail!("unavailable") },
        )
        .unwrap_err();
        assert!(error.to_string().contains("startup fallback"));
    }

    #[test]
    fn stable_staging_accepts_identical_fingerprints_and_returns_the_second_candidate() {
        let mut candidates = [(7_u8, "first"), (7_u8, "second")].into_iter();
        let staged = stage_stable_with(|| Ok(candidates.next().unwrap())).unwrap();
        assert_eq!(staged, "second");
    }

    #[test]
    fn stable_staging_rejects_a_mixed_resolution() {
        let mut candidates = [(7_u8, "first"), (8_u8, "second")].into_iter();
        let error = stage_stable_with(|| Ok(candidates.next().unwrap())).unwrap_err();
        assert!(error.to_string().contains("changed while staging"));
    }

    #[test]
    #[ignore = "manual host resource timing; requires fontconfig and installed fonts"]
    fn repeated_live_staging_is_fd_bounded() {
        let options = renderer_options();
        let before = std::fs::read_dir("/proc/self/fd").unwrap().count();
        let started = Instant::now();
        for _ in 0..3 {
            drop(stage_live_font_generation(&options.font, options.font_authority).unwrap());
        }
        let elapsed = started.elapsed();
        let after = std::fs::read_dir("/proc/self/fd").unwrap().count();
        eprintln!(
            "live-font-stage three_generations_ms={:.3} fd_before={before} fd_after={after}",
            elapsed.as_secs_f64() * 1_000.0
        );
        assert!(after <= before.saturating_add(1));
    }

    #[test]
    #[ignore = "requires host fontconfig and installed system fonts"]
    fn live_probe_matches_the_staged_generation_on_the_supported_host() {
        let options = renderer_options();
        let probe = probe_live_font_sources(&options.font, options.font_authority).unwrap();
        let staged = stage_live_font_generation(&options.font, options.font_authority).unwrap();
        assert_eq!(probe, staged.fingerprint);
    }

    #[test]
    #[ignore = "requires host fontconfig and installed system fonts"]
    fn effective_system_monospace_resolves_one_coherent_primary_family() {
        let faces =
            resolve_primary_faces("monospace:style=Regular", FontAuthority::NativeOmarchy).unwrap();
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
                1,
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
            1,
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
                (1, 768, GlyphKey { face: 0, glyph }),
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
