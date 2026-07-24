//! Renderer-independent bounded terminal image content and placements.

mod sixel;

pub(crate) use sixel::{MAX_SIXEL_COLORS, SixelDecoder, SixelError};

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use sha2::{Digest, Sha256};

use crate::ActiveScreen;

pub const DEFAULT_IMAGE_BYTES_PER_CONTENT: usize = 16 * 1024 * 1024;
pub const DEFAULT_IMAGE_BYTES_PER_TERMINAL: usize = 32 * 1024 * 1024;
pub const DEFAULT_IMAGE_CONTENTS_PER_TERMINAL: usize = 64;
pub const DEFAULT_IMAGE_PLACEMENTS_PER_TERMINAL: usize = 256;
pub const DEFAULT_IMAGE_MAX_DIMENSION: u32 = 4096;
pub const DEFAULT_IMAGE_MAX_PIXELS: usize = 4_194_304;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImageContentId(u64);

impl ImageContentId {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImagePlacementId(u64);

impl ImagePlacementId {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageSourceFormat {
    Sixel,
    KittyRgb,
    KittyRgba,
    KittyPng,
    Iterm2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageAlphaMode {
    Opaque,
    Premultiplied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageRetention {
    WhilePlaced,
    ExplicitDelete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageErasePolicy {
    TextOverwrite,
    ExplicitDelete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellExtent {
    pub columns: usize,
    pub rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageContentMetadata {
    pub id: ImageContentId,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub source_format: ImageSourceFormat,
    pub alpha_mode: ImageAlphaMode,
    pub digest: [u8; 32],
    pub byte_charge: usize,
    pub retention: ImageRetention,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageContent {
    metadata: ImageContentMetadata,
    pixels: Arc<[u8]>,
}

impl ImageContent {
    #[must_use]
    pub const fn metadata(&self) -> ImageContentMetadata {
        self.metadata
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImagePlacement {
    pub id: ImagePlacementId,
    pub content_id: ImageContentId,
    pub screen: ActiveScreen,
    pub row_id: u64,
    pub column: usize,
    pub source: PixelRect,
    pub destination: CellExtent,
    pub x_offset: i32,
    pub y_offset: i32,
    pub z_index: i32,
    pub application_image_id: Option<u32>,
    pub application_placement_id: Option<u32>,
    pub creation_order: u64,
    pub erase_policy: ImageErasePolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewImageContent<'a> {
    pub width: u32,
    pub height: u32,
    pub source_format: ImageSourceFormat,
    pub alpha_mode: ImageAlphaMode,
    pub pixels: &'a [u8],
    pub retention: ImageRetention,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewImagePlacementOptions {
    pub column: usize,
    pub source: PixelRect,
    pub destination: CellExtent,
    pub x_offset: i32,
    pub y_offset: i32,
    pub z_index: i32,
    pub application_image_id: Option<u32>,
    pub application_placement_id: Option<u32>,
    pub erase_policy: ImageErasePolicy,
}

impl NewImagePlacementOptions {
    #[must_use]
    pub const fn bind(self, content_id: ImageContentId, row_id: u64) -> NewImagePlacement {
        NewImagePlacement {
            content_id,
            row_id,
            column: self.column,
            source: self.source,
            destination: self.destination,
            x_offset: self.x_offset,
            y_offset: self.y_offset,
            z_index: self.z_index,
            application_image_id: self.application_image_id,
            application_placement_id: self.application_placement_id,
            erase_policy: self.erase_policy,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewImagePlacement {
    pub content_id: ImageContentId,
    pub row_id: u64,
    pub column: usize,
    pub source: PixelRect,
    pub destination: CellExtent,
    pub x_offset: i32,
    pub y_offset: i32,
    pub z_index: i32,
    pub application_image_id: Option<u32>,
    pub application_placement_id: Option<u32>,
    pub erase_policy: ImageErasePolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageLimits {
    pub bytes_per_content: usize,
    pub bytes_per_terminal: usize,
    pub contents_per_terminal: usize,
    pub placements_per_terminal: usize,
    pub maximum_dimension: u32,
    pub maximum_pixels: usize,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            bytes_per_content: DEFAULT_IMAGE_BYTES_PER_CONTENT,
            bytes_per_terminal: DEFAULT_IMAGE_BYTES_PER_TERMINAL,
            contents_per_terminal: DEFAULT_IMAGE_CONTENTS_PER_TERMINAL,
            placements_per_terminal: DEFAULT_IMAGE_PLACEMENTS_PER_TERMINAL,
            maximum_dimension: DEFAULT_IMAGE_MAX_DIMENSION,
            maximum_pixels: DEFAULT_IMAGE_MAX_PIXELS,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImageMetrics {
    pub content_bytes: usize,
    pub content_count: usize,
    pub placement_count: usize,
    pub high_water_content_bytes: usize,
    pub high_water_content_count: usize,
    pub high_water_placement_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    Dimensions,
    PixelLength,
    ContentBytes,
    TerminalBytes,
    ContentCount,
    PlacementCount,
    UnknownContent,
    UnknownPlacement,
    InvalidAnchor,
    InvalidCrop,
    InvalidDestination,
    IdentityExhausted,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ImageCatalog {
    contents: BTreeMap<ImageContentId, ImageContent>,
    placements: BTreeMap<ImagePlacementId, ImagePlacement>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImagePlane {
    normal: ImageCatalog,
    alternate: ImageCatalog,
    limits: ImageLimits,
    metrics: ImageMetrics,
    next_content_id: u64,
    next_placement_id: u64,
    next_generation: u64,
    next_creation_order: u64,
}

impl Default for ImagePlane {
    fn default() -> Self {
        Self::new(ImageLimits::default())
    }
}

impl ImagePlane {
    #[must_use]
    pub const fn new(limits: ImageLimits) -> Self {
        Self {
            normal: ImageCatalog {
                contents: BTreeMap::new(),
                placements: BTreeMap::new(),
            },
            alternate: ImageCatalog {
                contents: BTreeMap::new(),
                placements: BTreeMap::new(),
            },
            limits,
            metrics: ImageMetrics {
                content_bytes: 0,
                content_count: 0,
                placement_count: 0,
                high_water_content_bytes: 0,
                high_water_content_count: 0,
                high_water_placement_count: 0,
            },
            next_content_id: 1,
            next_placement_id: 1,
            next_generation: 1,
            next_creation_order: 1,
        }
    }

    #[must_use]
    pub const fn limits(&self) -> ImageLimits {
        self.limits
    }

    #[must_use]
    pub const fn metrics(&self) -> ImageMetrics {
        self.metrics
    }

    /// Stores one immutable canonical image under aggregate terminal limits.
    ///
    /// # Errors
    ///
    /// Returns a deterministic dimensions, byte, count, or identity error.
    pub fn insert_content(
        &mut self,
        screen: ActiveScreen,
        input: NewImageContent<'_>,
    ) -> Result<ImageContentId, ImageError> {
        let expected = self.validate_content(&input)?;
        self.reclaim_unplaced_while_placed();
        if self.metrics.content_count >= self.limits.contents_per_terminal {
            return Err(ImageError::ContentCount);
        }
        let next_bytes = self
            .metrics
            .content_bytes
            .checked_add(expected)
            .filter(|bytes| *bytes <= self.limits.bytes_per_terminal)
            .ok_or(ImageError::TerminalBytes)?;
        let id = ImageContentId::new(self.next_content_id).ok_or(ImageError::IdentityExhausted)?;
        self.next_content_id = self
            .next_content_id
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        let digest: [u8; 32] = Sha256::digest(input.pixels).into();
        let content = ImageContent {
            metadata: ImageContentMetadata {
                id,
                generation,
                width: input.width,
                height: input.height,
                source_format: input.source_format,
                alpha_mode: input.alpha_mode,
                digest,
                byte_charge: expected,
                retention: input.retention,
            },
            pixels: Arc::from(input.pixels),
        };
        self.catalog_mut(screen).contents.insert(id, content);
        self.metrics.content_bytes = next_bytes;
        self.metrics.content_count += 1;
        self.update_high_water();
        Ok(id)
    }

    /// Atomically stores content and places it at one stable row anchor.
    ///
    /// # Errors
    ///
    /// Returns a deterministic validation, budget, or identity error. Validation
    /// failures leave current accounting and semantic state unchanged.
    pub fn insert_content_and_placement(
        &mut self,
        screen: ActiveScreen,
        content: NewImageContent<'_>,
        row_id: u64,
        placement: NewImagePlacementOptions,
    ) -> Result<(ImageContentId, ImagePlacementId), ImageError> {
        self.validate_content(&content)?;
        if row_id == 0 {
            return Err(ImageError::InvalidAnchor);
        }
        if placement.destination.columns == 0 || placement.destination.rows == 0 {
            return Err(ImageError::InvalidDestination);
        }
        validate_crop(placement.source, content.width, content.height)?;
        if self.metrics.placement_count >= self.limits.placements_per_terminal {
            return Err(ImageError::PlacementCount);
        }
        let content_id = self.insert_content(screen, content)?;
        match self.insert_placement(screen, placement.bind(content_id, row_id)) {
            Ok(placement_id) => Ok((content_id, placement_id)),
            Err(error) => {
                let rollback = self.remove_content_only(screen, content_id);
                debug_assert!(rollback.is_ok(), "new content must remain for rollback");
                Err(error)
            }
        }
    }

    /// Places existing content on one screen under the placement limit.
    ///
    /// # Errors
    ///
    /// Returns an anchor, crop, destination, content, count, or identity error.
    pub fn insert_placement(
        &mut self,
        screen: ActiveScreen,
        input: NewImagePlacement,
    ) -> Result<ImagePlacementId, ImageError> {
        if input.row_id == 0 {
            return Err(ImageError::InvalidAnchor);
        }
        if input.destination.columns == 0 || input.destination.rows == 0 {
            return Err(ImageError::InvalidDestination);
        }
        let content = self
            .catalog(screen)
            .contents
            .get(&input.content_id)
            .ok_or(ImageError::UnknownContent)?;
        validate_crop(
            input.source,
            content.metadata.width,
            content.metadata.height,
        )?;
        if self.metrics.placement_count >= self.limits.placements_per_terminal {
            return Err(ImageError::PlacementCount);
        }
        let id =
            ImagePlacementId::new(self.next_placement_id).ok_or(ImageError::IdentityExhausted)?;
        self.next_placement_id = self
            .next_placement_id
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        let creation_order = self.next_creation_order;
        self.next_creation_order = self
            .next_creation_order
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        self.catalog_mut(screen).placements.insert(
            id,
            ImagePlacement {
                id,
                content_id: input.content_id,
                screen,
                row_id: input.row_id,
                column: input.column,
                source: input.source,
                destination: input.destination,
                x_offset: input.x_offset,
                y_offset: input.y_offset,
                z_index: input.z_index,
                application_image_id: input.application_image_id,
                application_placement_id: input.application_placement_id,
                creation_order,
                erase_policy: input.erase_policy,
            },
        );
        self.metrics.placement_count += 1;
        self.update_high_water();
        Ok(id)
    }

    /// Removes one exact placement and reclaims eligible unplaced content.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::UnknownPlacement`] for absence or double removal.
    pub fn remove_placement(
        &mut self,
        screen: ActiveScreen,
        id: ImagePlacementId,
    ) -> Result<ImagePlacement, ImageError> {
        let placement = self
            .catalog_mut(screen)
            .placements
            .remove(&id)
            .ok_or(ImageError::UnknownPlacement)?;
        self.metrics.placement_count -= 1;
        self.reclaim_unplaced_while_placed();
        Ok(placement)
    }

    /// Removes content and all placements that reference it on one screen.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::UnknownContent`] for absence or double removal.
    pub fn remove_content(
        &mut self,
        screen: ActiveScreen,
        id: ImageContentId,
    ) -> Result<(), ImageError> {
        let placement_ids: Vec<_> = self
            .catalog(screen)
            .placements
            .values()
            .filter(|placement| placement.content_id == id)
            .map(|placement| placement.id)
            .collect();
        for placement_id in placement_ids {
            self.catalog_mut(screen).placements.remove(&placement_id);
            self.metrics.placement_count -= 1;
        }
        self.remove_content_only(screen, id)
    }

    pub fn clear_screen(&mut self, screen: ActiveScreen) {
        let catalog = self.catalog_mut(screen);
        let bytes = catalog
            .contents
            .values()
            .map(|content| content.metadata.byte_charge)
            .sum::<usize>();
        let content_count = catalog.contents.len();
        let placement_count = catalog.placements.len();
        catalog.contents.clear();
        catalog.placements.clear();
        self.metrics.content_bytes -= bytes;
        self.metrics.content_count -= content_count;
        self.metrics.placement_count -= placement_count;
    }

    pub fn retain_anchors(&mut self, screen: ActiveScreen, row_ids: &HashSet<u64>) {
        let removed: Vec<_> = self
            .catalog(screen)
            .placements
            .values()
            .filter(|placement| !row_ids.contains(&placement.row_id))
            .map(|placement| placement.id)
            .collect();
        for id in removed {
            self.catalog_mut(screen).placements.remove(&id);
            self.metrics.placement_count -= 1;
        }
        self.reclaim_unplaced_while_placed();
    }

    /// Rebinds placements whose grid rows received new stable identities.
    pub fn remap_anchors(
        &mut self,
        screen: ActiveScreen,
        replacements: &BTreeMap<u64, u64>,
    ) -> bool {
        let mut changed = false;
        for placement in self.catalog_mut(screen).placements.values_mut() {
            if let Some(replacement) = replacements.get(&placement.row_id) {
                placement.row_id = *replacement;
                changed = true;
            }
        }
        changed
    }

    /// Removes text-overwrite placements intersecting a cell rectangle.
    pub fn remove_text_overlaps(
        &mut self,
        screen: ActiveScreen,
        row_order: &[u64],
        start_row: usize,
        end_row: usize,
        start_column: usize,
        end_column: usize,
    ) -> bool {
        if start_row >= end_row || start_column >= end_column {
            return false;
        }
        let row_positions: BTreeMap<_, _> = row_order
            .iter()
            .copied()
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect();
        let removed: Vec<_> = self
            .catalog(screen)
            .placements
            .values()
            .filter(|placement| {
                if placement.erase_policy != ImageErasePolicy::TextOverwrite {
                    return false;
                }
                let Some(anchor_row) = row_positions.get(&placement.row_id).copied() else {
                    return false;
                };
                let placement_end_row = anchor_row.saturating_add(placement.destination.rows);
                let placement_end_column = placement
                    .column
                    .saturating_add(placement.destination.columns);
                anchor_row < end_row
                    && placement_end_row > start_row
                    && placement.column < end_column
                    && placement_end_column > start_column
            })
            .map(|placement| placement.id)
            .collect();
        for id in &removed {
            self.catalog_mut(screen).placements.remove(id);
            self.metrics.placement_count -= 1;
        }
        if !removed.is_empty() {
            self.reclaim_unplaced_while_placed();
        }
        !removed.is_empty()
    }

    #[must_use]
    pub fn has_placements(&self, screen: ActiveScreen) -> bool {
        !self.catalog(screen).placements.is_empty()
    }

    #[must_use]
    pub fn content_metadata(
        &self,
        screen: ActiveScreen,
    ) -> impl ExactSizeIterator<Item = ImageContentMetadata> + '_ {
        self.catalog(screen)
            .contents
            .values()
            .map(ImageContent::metadata)
    }

    #[must_use]
    pub fn placements(
        &self,
        screen: ActiveScreen,
    ) -> impl ExactSizeIterator<Item = ImagePlacement> + '_ {
        self.catalog(screen).placements.values().copied()
    }

    #[must_use]
    pub fn content(&self, screen: ActiveScreen, id: ImageContentId) -> Option<&ImageContent> {
        self.catalog(screen).contents.get(&id)
    }

    #[must_use]
    pub fn ordered_placements(&self, screen: ActiveScreen) -> Vec<ImagePlacement> {
        let mut placements: Vec<_> = self.placements(screen).collect();
        placements.sort_by_key(|placement| {
            (
                placement.z_index,
                placement
                    .application_image_id
                    .map_or(placement.creation_order, u64::from),
                placement.creation_order,
            )
        });
        placements
    }

    fn validate_content(&self, input: &NewImageContent<'_>) -> Result<usize, ImageError> {
        if input.width == 0
            || input.height == 0
            || input.width > self.limits.maximum_dimension
            || input.height > self.limits.maximum_dimension
        {
            return Err(ImageError::Dimensions);
        }
        let pixels = usize::try_from(input.width)
            .ok()
            .and_then(|width| {
                usize::try_from(input.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .filter(|pixels| *pixels <= self.limits.maximum_pixels)
            .ok_or(ImageError::Dimensions)?;
        let expected = pixels.checked_mul(4).ok_or(ImageError::PixelLength)?;
        if input.pixels.len() != expected {
            return Err(ImageError::PixelLength);
        }
        if expected > self.limits.bytes_per_content {
            return Err(ImageError::ContentBytes);
        }
        Ok(expected)
    }

    fn remove_content_only(
        &mut self,
        screen: ActiveScreen,
        id: ImageContentId,
    ) -> Result<(), ImageError> {
        let content = self
            .catalog_mut(screen)
            .contents
            .remove(&id)
            .ok_or(ImageError::UnknownContent)?;
        self.metrics.content_bytes -= content.metadata.byte_charge;
        self.metrics.content_count -= 1;
        Ok(())
    }

    fn reclaim_unplaced_while_placed(&mut self) {
        for screen in [ActiveScreen::Normal, ActiveScreen::Alternate] {
            let referenced: HashSet<_> = self
                .catalog(screen)
                .placements
                .values()
                .map(|placement| placement.content_id)
                .collect();
            let reclaim: Vec<_> = self
                .catalog(screen)
                .contents
                .values()
                .filter(|content| {
                    content.metadata.retention == ImageRetention::WhilePlaced
                        && !referenced.contains(&content.metadata.id)
                })
                .map(|content| content.metadata.id)
                .collect();
            for id in reclaim {
                self.remove_content_only(screen, id)
                    .expect("reclaim identity came from the catalog");
            }
        }
    }

    fn update_high_water(&mut self) {
        self.metrics.high_water_content_bytes = self
            .metrics
            .high_water_content_bytes
            .max(self.metrics.content_bytes);
        self.metrics.high_water_content_count = self
            .metrics
            .high_water_content_count
            .max(self.metrics.content_count);
        self.metrics.high_water_placement_count = self
            .metrics
            .high_water_placement_count
            .max(self.metrics.placement_count);
    }

    const fn catalog(&self, screen: ActiveScreen) -> &ImageCatalog {
        match screen {
            ActiveScreen::Normal => &self.normal,
            ActiveScreen::Alternate => &self.alternate,
        }
    }

    const fn catalog_mut(&mut self, screen: ActiveScreen) -> &mut ImageCatalog {
        match screen {
            ActiveScreen::Normal => &mut self.normal,
            ActiveScreen::Alternate => &mut self.alternate,
        }
    }
}

fn validate_crop(crop: PixelRect, width: u32, height: u32) -> Result<(), ImageError> {
    if crop.width == 0
        || crop.height == 0
        || crop.x.checked_add(crop.width).is_none_or(|end| end > width)
        || crop
            .y
            .checked_add(crop.height)
            .is_none_or(|end| end > height)
    {
        return Err(ImageError::InvalidCrop);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(pixels: &[u8], retention: ImageRetention) -> NewImageContent<'_> {
        NewImageContent {
            width: 1,
            height: 1,
            source_format: ImageSourceFormat::Sixel,
            alpha_mode: ImageAlphaMode::Opaque,
            pixels,
            retention,
        }
    }

    fn placement(content_id: ImageContentId, row_id: u64, z_index: i32) -> NewImagePlacement {
        NewImagePlacement {
            content_id,
            row_id,
            column: 0,
            source: PixelRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            destination: CellExtent {
                columns: 1,
                rows: 1,
            },
            x_offset: 0,
            y_offset: 0,
            z_index,
            application_image_id: None,
            application_placement_id: None,
            erase_policy: ImageErasePolicy::TextOverwrite,
        }
    }

    #[test]
    fn content_and_placements_are_bounded_shared_and_screen_local() {
        let mut plane = ImagePlane::new(ImageLimits {
            bytes_per_content: 4,
            bytes_per_terminal: 8,
            contents_per_terminal: 2,
            placements_per_terminal: 2,
            maximum_dimension: 2,
            maximum_pixels: 2,
        });
        let first = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[1, 2, 3, 255], ImageRetention::ExplicitDelete),
            )
            .unwrap();
        plane
            .insert_placement(ActiveScreen::Normal, placement(first, 7, 0))
            .unwrap();
        plane
            .insert_placement(ActiveScreen::Normal, placement(first, 7, 1))
            .unwrap();
        assert_eq!(
            plane.content(ActiveScreen::Normal, first).unwrap().pixels(),
            &[1, 2, 3, 255]
        );
        assert_eq!(plane.metrics().content_bytes, 4);
        assert_eq!(plane.metrics().placement_count, 2);
        assert_eq!(
            plane.insert_placement(ActiveScreen::Normal, placement(first, 7, 2)),
            Err(ImageError::PlacementCount)
        );
        assert!(plane.content(ActiveScreen::Alternate, first).is_none());
    }

    #[test]
    fn admission_reclaims_only_while_placed_content_and_tracks_high_water() {
        let mut plane = ImagePlane::new(ImageLimits {
            bytes_per_content: 4,
            bytes_per_terminal: 4,
            contents_per_terminal: 1,
            placements_per_terminal: 1,
            maximum_dimension: 1,
            maximum_pixels: 1,
        });
        let transient = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[0, 0, 0, 0], ImageRetention::WhilePlaced),
            )
            .unwrap();
        let retained = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[1, 1, 1, 255], ImageRetention::ExplicitDelete),
            )
            .unwrap();
        assert!(plane.content(ActiveScreen::Normal, transient).is_none());
        assert!(plane.content(ActiveScreen::Normal, retained).is_some());
        assert_eq!(plane.metrics().high_water_content_bytes, 4);
        assert_eq!(plane.metrics().high_water_content_count, 1);
    }

    #[test]
    fn anchor_pruning_reclaims_sixel_but_preserves_explicit_content() {
        let mut plane = ImagePlane::default();
        let sixel = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[0, 0, 255, 255], ImageRetention::WhilePlaced),
            )
            .unwrap();
        plane
            .insert_placement(ActiveScreen::Normal, placement(sixel, 10, 0))
            .unwrap();
        let kitty = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[0, 255, 0, 255], ImageRetention::ExplicitDelete),
            )
            .unwrap();
        plane
            .insert_placement(ActiveScreen::Normal, placement(kitty, 11, 0))
            .unwrap();
        plane.retain_anchors(ActiveScreen::Normal, &HashSet::from([11]));
        assert!(plane.content(ActiveScreen::Normal, sixel).is_none());
        assert!(plane.content(ActiveScreen::Normal, kitty).is_some());
        assert_eq!(plane.placements(ActiveScreen::Normal).count(), 1);
    }

    #[test]
    fn text_overlap_removes_sixel_policy_but_preserves_explicit_policy() {
        let mut plane = ImagePlane::default();
        let id = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[0, 0, 0, 255], ImageRetention::ExplicitDelete),
            )
            .unwrap();
        let removable = plane
            .insert_placement(ActiveScreen::Normal, placement(id, 10, 0))
            .unwrap();
        let mut explicit = placement(id, 10, 1);
        explicit.erase_policy = ImageErasePolicy::ExplicitDelete;
        let retained = plane
            .insert_placement(ActiveScreen::Normal, explicit)
            .unwrap();
        assert!(plane.remove_text_overlaps(ActiveScreen::Normal, &[10, 11], 0, 1, 0, 1,));
        assert_eq!(
            plane
                .placements(ActiveScreen::Normal)
                .map(|placement| placement.id)
                .collect::<Vec<_>>(),
            vec![retained]
        );
        assert_ne!(removable, retained);
    }

    #[test]
    fn ordering_uses_z_then_application_image_then_creation() {
        let mut plane = ImagePlane::default();
        let id = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[0, 0, 0, 255], ImageRetention::ExplicitDelete),
            )
            .unwrap();
        let mut later_low_id = placement(id, 1, 0);
        later_low_id.application_image_id = Some(2);
        let mut earlier_high_id = placement(id, 1, 0);
        earlier_high_id.application_image_id = Some(9);
        let high = plane
            .insert_placement(ActiveScreen::Normal, earlier_high_id)
            .unwrap();
        let low = plane
            .insert_placement(ActiveScreen::Normal, later_low_id)
            .unwrap();
        let behind = plane
            .insert_placement(ActiveScreen::Normal, placement(id, 1, -1))
            .unwrap();
        assert_eq!(
            plane
                .ordered_placements(ActiveScreen::Normal)
                .iter()
                .map(|placement| placement.id)
                .collect::<Vec<_>>(),
            vec![behind, low, high]
        );
    }
}
