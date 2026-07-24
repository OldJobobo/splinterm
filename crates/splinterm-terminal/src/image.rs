//! Renderer-independent bounded terminal image content and placements.

pub(crate) mod iterm;
pub(crate) mod kitty;
mod sixel;

pub(crate) use sixel::{MAX_SIXEL_COLORS, SixelDecoder, SixelError, SixelImage};

use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use sha2::{Digest, Sha256};

use crate::ActiveScreen;

pub const DEFAULT_IMAGE_BYTES_PER_CONTENT: usize = 16 * 1024 * 1024;
pub const DEFAULT_IMAGE_BYTES_PER_TERMINAL: usize = 32 * 1024 * 1024;
pub const DEFAULT_IMAGE_CONTENTS_PER_TERMINAL: usize = 64;
pub const DEFAULT_IMAGE_PLACEMENTS_PER_TERMINAL: usize = 256;
pub const DEFAULT_IMAGE_MAX_DIMENSION: u32 = 4096;
pub const DEFAULT_IMAGE_MAX_PIXELS: usize = 4_194_304;
pub const DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SharedKittyUploadBudgetMetrics {
    pub reserved_bytes: usize,
    pub high_water_reserved_bytes: usize,
}

#[derive(Debug)]
struct SharedKittyUploadBudgetInner {
    limit: usize,
    reserved_bytes: AtomicUsize,
    high_water_reserved_bytes: AtomicUsize,
}

#[derive(Clone, Debug)]
pub struct SharedKittyUploadBudget(Arc<SharedKittyUploadBudgetInner>);

impl PartialEq for SharedKittyUploadBudget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.limit == other.0.limit && self.metrics() == other.metrics())
    }
}

impl Eq for SharedKittyUploadBudget {}

impl SharedKittyUploadBudget {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(SharedKittyUploadBudgetInner {
            limit,
            reserved_bytes: AtomicUsize::new(0),
            high_water_reserved_bytes: AtomicUsize::new(0),
        }))
    }

    #[must_use]
    pub fn metrics(&self) -> SharedKittyUploadBudgetMetrics {
        SharedKittyUploadBudgetMetrics {
            reserved_bytes: self.0.reserved_bytes.load(Ordering::Acquire),
            high_water_reserved_bytes: self.0.high_water_reserved_bytes.load(Ordering::Acquire),
        }
    }

    pub(crate) fn reserve(&self, bytes: usize) -> Result<KittyUploadReservation, ImageError> {
        let mut current = self.0.reserved_bytes.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(bytes)
                .filter(|next| *next <= self.0.limit)
                .ok_or(ImageError::DaemonBytes)?;
            match self.0.reserved_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.0
                        .high_water_reserved_bytes
                        .fetch_max(next, Ordering::AcqRel);
                    return Ok(KittyUploadReservation(Arc::new(KittyUploadLease {
                        budget: Arc::clone(&self.0),
                        bytes,
                    })));
                }
                Err(actual) => current = actual,
            }
        }
    }
}

#[derive(Debug)]
struct KittyUploadLease {
    budget: Arc<SharedKittyUploadBudgetInner>,
    bytes: usize,
}

impl Drop for KittyUploadLease {
    fn drop(&mut self) {
        let previous = self
            .budget
            .reserved_bytes
            .fetch_sub(self.bytes, Ordering::AcqRel);
        debug_assert!(
            previous >= self.bytes,
            "Kitty upload budget release underflow"
        );
    }
}

#[derive(Clone, Debug)]
pub(crate) struct KittyUploadReservation(Arc<KittyUploadLease>);

impl PartialEq for KittyUploadReservation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for KittyUploadReservation {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SharedImageBudgetMetrics {
    pub content_bytes: usize,
    pub high_water_content_bytes: usize,
}

#[derive(Debug)]
struct SharedImageBudgetInner {
    limit: usize,
    content_bytes: AtomicUsize,
    high_water_content_bytes: AtomicUsize,
}

#[derive(Clone, Debug)]
pub struct SharedImageBudget(Arc<SharedImageBudgetInner>);

impl PartialEq for SharedImageBudget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SharedImageBudget {}

impl SharedImageBudget {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(SharedImageBudgetInner {
            limit,
            content_bytes: AtomicUsize::new(0),
            high_water_content_bytes: AtomicUsize::new(0),
        }))
    }

    #[must_use]
    pub fn metrics(&self) -> SharedImageBudgetMetrics {
        SharedImageBudgetMetrics {
            content_bytes: self.0.content_bytes.load(Ordering::Acquire),
            high_water_content_bytes: self.0.high_water_content_bytes.load(Ordering::Acquire),
        }
    }

    fn reserve(&self, bytes: usize) -> Result<ImageBudgetReservation, ImageError> {
        let mut current = self.0.content_bytes.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(bytes)
                .filter(|next| *next <= self.0.limit)
                .ok_or(ImageError::DaemonBytes)?;
            match self.0.content_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.0
                        .high_water_content_bytes
                        .fetch_max(next, Ordering::AcqRel);
                    return Ok(ImageBudgetReservation(Arc::new(ImageBudgetLease {
                        budget: Arc::clone(&self.0),
                        bytes: AtomicUsize::new(bytes),
                    })));
                }
                Err(actual) => current = actual,
            }
        }
    }
}

#[derive(Debug)]
struct ImageBudgetLease {
    budget: Arc<SharedImageBudgetInner>,
    bytes: AtomicUsize,
}

impl Drop for ImageBudgetLease {
    fn drop(&mut self) {
        let bytes = self.bytes.load(Ordering::Acquire);
        let previous = self.budget.content_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "image budget release underflow");
    }
}

#[derive(Clone, Debug)]
struct ImageBudgetReservation(Arc<ImageBudgetLease>);

impl PartialEq for ImageBudgetReservation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ImageBudgetReservation {}

impl ImageBudgetReservation {
    fn resize(&self, bytes: usize) -> Result<(), ImageError> {
        let old = self.0.bytes.load(Ordering::Acquire);
        if bytes > old {
            let delta = bytes - old;
            let mut current = self.0.budget.content_bytes.load(Ordering::Acquire);
            loop {
                let next = current
                    .checked_add(delta)
                    .filter(|next| *next <= self.0.budget.limit)
                    .ok_or(ImageError::DaemonBytes)?;
                match self.0.budget.content_bytes.compare_exchange_weak(
                    current,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.0
                            .budget
                            .high_water_content_bytes
                            .fetch_max(next, Ordering::AcqRel);
                        self.0.bytes.store(bytes, Ordering::Release);
                        return Ok(());
                    }
                    Err(actual) => current = actual,
                }
            }
        } else if bytes < old {
            self.0.bytes.store(bytes, Ordering::Release);
            let previous = self
                .0
                .budget
                .content_bytes
                .fetch_sub(old - bytes, Ordering::AcqRel);
            debug_assert!(previous >= old - bytes, "image budget resize underflow");
        }
        Ok(())
    }
}

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
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
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
    pub source_cell_size: Option<PixelSize>,
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

struct PreparedSixel {
    width: u32,
    height: u32,
    alpha_mode: ImageAlphaMode,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewImagePlacementOptions {
    pub column: usize,
    pub source: PixelRect,
    pub destination: CellExtent,
    pub source_cell_size: Option<PixelSize>,
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
            source_cell_size: self.source_cell_size,
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
    pub source_cell_size: Option<PixelSize>,
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
    DaemonBytes,
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
    reservations: BTreeMap<ImageContentId, ImageBudgetReservation>,
    placements: BTreeMap<ImagePlacementId, ImagePlacement>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImagePlane {
    normal: ImageCatalog,
    alternate: ImageCatalog,
    limits: ImageLimits,
    shared_budget: Option<SharedImageBudget>,
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
    pub fn new(limits: ImageLimits) -> Self {
        Self::new_with_shared_budget(limits, None)
    }

    #[must_use]
    pub fn new_with_shared_budget(
        limits: ImageLimits,
        shared_budget: Option<SharedImageBudget>,
    ) -> Self {
        Self {
            normal: ImageCatalog {
                contents: BTreeMap::new(),
                reservations: BTreeMap::new(),
                placements: BTreeMap::new(),
            },
            alternate: ImageCatalog {
                contents: BTreeMap::new(),
                reservations: BTreeMap::new(),
                placements: BTreeMap::new(),
            },
            limits,
            shared_budget,
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
        let next_content_id = self
            .next_content_id
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        let generation = self.next_generation;
        let next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        let reservation = self
            .shared_budget
            .as_ref()
            .map(|budget| budget.reserve(expected))
            .transpose()?;
        self.next_content_id = next_content_id;
        self.next_generation = next_generation;
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
        let catalog = self.catalog_mut(screen);
        let replaced = catalog.contents.insert(id, content);
        debug_assert!(
            replaced.is_none(),
            "new image content identity must be unique"
        );
        if let Some(reservation) = reservation {
            let replaced = catalog.reservations.insert(id, reservation);
            debug_assert!(
                replaced.is_none(),
                "new image reservation identity must be unique"
            );
        }
        self.metrics.content_bytes = next_bytes;
        self.metrics.content_count += 1;
        self.update_high_water();
        Ok(id)
    }

    /// Atomically replaces one content object while admitting only a positive
    /// byte delta against terminal and shared budgets. All old placements are
    /// removed after validation and admission succeed.
    ///
    /// # Errors
    ///
    /// Returns a validation, identity, unknown-content, or budget error without
    /// changing content or placement state.
    pub(crate) fn replace_content(
        &mut self,
        screen: ActiveScreen,
        id: ImageContentId,
        input: NewImageContent<'_>,
    ) -> Result<ImageContentId, ImageError> {
        let expected = self.validate_content(&input)?;
        let previous_charge = self
            .catalog(screen)
            .contents
            .get(&id)
            .ok_or(ImageError::UnknownContent)?
            .metadata
            .byte_charge;
        let next_bytes = self
            .metrics
            .content_bytes
            .checked_sub(previous_charge)
            .and_then(|bytes| bytes.checked_add(expected))
            .filter(|bytes| *bytes <= self.limits.bytes_per_terminal)
            .ok_or(ImageError::TerminalBytes)?;
        let generation = self.next_generation;
        let next_generation = generation
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        if let Some(reservation) = self.catalog(screen).reservations.get(&id) {
            reservation.resize(expected)?;
        }
        let placement_ids = self
            .catalog(screen)
            .placements
            .values()
            .filter(|placement| placement.content_id == id)
            .map(|placement| placement.id)
            .collect::<Vec<_>>();
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
        self.next_generation = next_generation;
        self.metrics.content_bytes = next_bytes;
        for placement_id in placement_ids {
            self.catalog_mut(screen).placements.remove(&placement_id);
            self.metrics.placement_count -= 1;
        }
        let replaced = self.catalog_mut(screen).contents.insert(id, content);
        debug_assert!(replaced.is_some(), "validated replacement content exists");
        Ok(id)
    }

    pub(crate) fn preflight_replacement_placement(
        &self,
        screen: ActiveScreen,
        replaced_id: ImageContentId,
        content: NewImageContent<'_>,
        placement: NewImagePlacement,
    ) -> Result<(), ImageError> {
        let expected = self.validate_content(&content)?;
        let previous = self
            .catalog(screen)
            .contents
            .get(&replaced_id)
            .ok_or(ImageError::UnknownContent)?;
        let _final_bytes = self
            .metrics
            .content_bytes
            .checked_sub(previous.metadata.byte_charge)
            .and_then(|bytes| bytes.checked_add(expected))
            .filter(|bytes| *bytes <= self.limits.bytes_per_terminal)
            .ok_or(ImageError::TerminalBytes)?;
        if placement.row_id == 0 {
            return Err(ImageError::InvalidAnchor);
        }
        if placement.destination.columns == 0 || placement.destination.rows == 0 {
            return Err(ImageError::InvalidDestination);
        }
        validate_crop(placement.source, content.width, content.height)?;
        validate_source_cell_size(placement.source_cell_size)?;
        let removed = self
            .catalog(screen)
            .placements
            .values()
            .filter(|candidate| candidate.content_id == replaced_id)
            .count();
        let final_placements = self
            .metrics
            .placement_count
            .checked_sub(removed)
            .and_then(|count| count.checked_add(1))
            .ok_or(ImageError::PlacementCount)?;
        if final_placements > self.limits.placements_per_terminal {
            return Err(ImageError::PlacementCount);
        }
        self.next_generation
            .checked_add(1)
            .and_then(|_| self.next_placement_id.checked_add(1))
            .and_then(|_| self.next_creation_order.checked_add(1))
            .ok_or(ImageError::IdentityExhausted)?;
        if let Some(reservation) = self.catalog(screen).reservations.get(&replaced_id) {
            let old = reservation.0.bytes.load(Ordering::Acquire);
            if expected > old {
                self.shared_budget
                    .as_ref()
                    .and_then(|budget| {
                        budget
                            .metrics()
                            .content_bytes
                            .checked_add(expected - old)
                            .filter(|bytes| *bytes <= budget.0.limit)
                    })
                    .ok_or(ImageError::DaemonBytes)?;
            }
        }
        Ok(())
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
        validate_source_cell_size(placement.source_cell_size)?;
        if self.metrics.placement_count >= self.limits.placements_per_terminal {
            return Err(ImageError::PlacementCount);
        }
        self.next_content_id
            .checked_add(1)
            .and_then(|_| self.next_generation.checked_add(1))
            .and_then(|_| self.next_placement_id.checked_add(1))
            .and_then(|_| self.next_creation_order.checked_add(1))
            .ok_or(ImageError::IdentityExhausted)?;
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
        validate_source_cell_size(input.source_cell_size)?;
        if self.metrics.placement_count >= self.limits.placements_per_terminal {
            return Err(ImageError::PlacementCount);
        }
        let id =
            ImagePlacementId::new(self.next_placement_id).ok_or(ImageError::IdentityExhausted)?;
        let next_placement_id = self
            .next_placement_id
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        let creation_order = self.next_creation_order;
        let next_creation_order = self
            .next_creation_order
            .checked_add(1)
            .ok_or(ImageError::IdentityExhausted)?;
        self.next_placement_id = next_placement_id;
        self.next_creation_order = next_creation_order;
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
                source_cell_size: input.source_cell_size,
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
        catalog.reservations.clear();
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

    /// Crops or splits text-overwrite placements intersecting a cell rectangle.
    /// If fragment admission would exceed a hard bound, the intersecting image
    /// is removed instead so text mutation always remains bounded and coherent.
    pub fn remove_text_overlaps(
        &mut self,
        screen: ActiveScreen,
        row_order: &[u64],
        start_row: usize,
        end_row: usize,
        start_column: usize,
        end_column: usize,
    ) -> bool {
        let mut candidate = self.clone();
        match candidate.split_text_overlaps(
            screen,
            row_order,
            start_row,
            end_row,
            start_column,
            end_column,
        ) {
            Ok(true) => {
                *self = candidate;
                true
            }
            Ok(false) => false,
            Err(_) => self.remove_text_overlaps_whole(
                screen,
                row_order,
                start_row,
                end_row,
                start_column,
                end_column,
            ),
        }
    }

    pub(crate) fn insert_sixel_content_and_placement(
        &mut self,
        screen: ActiveScreen,
        content: NewImageContent<'_>,
        row_order: &[u64],
        row_id: u64,
        mut placement: NewImagePlacementOptions,
    ) -> Result<(ImageContentId, ImagePlacementId), ImageError> {
        self.validate_content(&content)?;
        validate_sixel_source_cell_size(
            placement.source,
            placement.destination,
            placement.source_cell_size,
        )?;
        let anchor_row = row_order
            .iter()
            .position(|candidate| *candidate == row_id)
            .ok_or(ImageError::InvalidAnchor)?;
        let end_row = anchor_row
            .checked_add(placement.destination.rows)
            .ok_or(ImageError::InvalidDestination)?;
        let end_column = placement
            .column
            .checked_add(placement.destination.columns)
            .ok_or(ImageError::InvalidDestination)?;
        let prepared =
            self.prepare_sixel_overlap(screen, row_order, anchor_row, end_row, placement, content)?;
        placement.source.width = prepared.width;
        placement.source.height = prepared.height;
        let content = NewImageContent {
            width: prepared.width,
            height: prepared.height,
            source_format: content.source_format,
            alpha_mode: prepared.alpha_mode,
            pixels: &prepared.pixels,
            retention: content.retention,
        };
        for preserve_fragments in [true, false] {
            let mut candidate = self.clone();
            let overlap = if preserve_fragments {
                candidate.split_text_overlaps(
                    screen,
                    row_order,
                    anchor_row,
                    end_row,
                    placement.column,
                    end_column,
                )
            } else {
                candidate.remove_text_overlaps_whole(
                    screen,
                    row_order,
                    anchor_row,
                    end_row,
                    placement.column,
                    end_column,
                );
                Ok(true)
            };
            if overlap.is_err() {
                continue;
            }
            if let Ok(ids) =
                candidate.insert_content_and_placement(screen, content, row_id, placement)
            {
                *self = candidate;
                return Ok(ids);
            }
        }
        self.insert_content_and_placement(screen, content, row_id, placement)
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the bounded Foot underlay composition keeps checked geometry and pixels together"
    )]
    fn prepare_sixel_overlap(
        &self,
        screen: ActiveScreen,
        row_order: &[u64],
        anchor_row: usize,
        end_row: usize,
        placement: NewImagePlacementOptions,
        content: NewImageContent<'_>,
    ) -> Result<PreparedSixel, ImageError> {
        let targets = self.text_overlap_targets(
            screen,
            row_order,
            anchor_row,
            end_row,
            placement.column,
            placement
                .column
                .saturating_add(placement.destination.columns),
        );
        let Some(cell_size) = placement.source_cell_size else {
            return Ok(PreparedSixel {
                width: content.width,
                height: content.height,
                alpha_mode: content.alpha_mode,
                pixels: content.pixels.to_vec(),
            });
        };
        if targets.is_empty() {
            return Ok(PreparedSixel {
                width: content.width,
                height: content.height,
                alpha_mode: content.alpha_mode,
                pixels: content.pixels.to_vec(),
            });
        }
        let cell_width = usize::try_from(cell_size.width).map_err(|_| ImageError::Dimensions)?;
        let cell_height = usize::try_from(cell_size.height).map_err(|_| ImageError::Dimensions)?;
        let full_width = placement
            .destination
            .columns
            .checked_mul(cell_width)
            .ok_or(ImageError::Dimensions)?;
        let full_height = placement
            .destination
            .rows
            .checked_mul(cell_height)
            .ok_or(ImageError::Dimensions)?;
        let new_origin_x = placement
            .column
            .checked_mul(cell_width)
            .ok_or(ImageError::Dimensions)?;
        let new_origin_y = anchor_row
            .checked_mul(cell_height)
            .ok_or(ImageError::Dimensions)?;
        let mut output_width =
            usize::try_from(content.width).map_err(|_| ImageError::Dimensions)?;
        let mut output_height =
            usize::try_from(content.height).map_err(|_| ImageError::Dimensions)?;
        for (old, old_anchor_row) in &targets {
            if old.source_cell_size != Some(cell_size) {
                continue;
            }
            let old_origin_x = old
                .column
                .checked_mul(cell_width)
                .ok_or(ImageError::Dimensions)?;
            let old_origin_y = old_anchor_row
                .checked_mul(cell_height)
                .ok_or(ImageError::Dimensions)?;
            let old_end_x = old_origin_x
                .checked_add(usize::try_from(old.source.width).map_err(|_| ImageError::Dimensions)?)
                .ok_or(ImageError::Dimensions)?;
            let old_end_y = old_origin_y
                .checked_add(
                    usize::try_from(old.source.height).map_err(|_| ImageError::Dimensions)?,
                )
                .ok_or(ImageError::Dimensions)?;
            if let Some(relative_end) = old_end_x.checked_sub(new_origin_x) {
                output_width = output_width.max(relative_end.min(full_width));
            }
            if let Some(relative_end) = old_end_y.checked_sub(new_origin_y) {
                output_height = output_height.max(relative_end.min(full_height));
            }
        }
        let width = u32::try_from(output_width).map_err(|_| ImageError::Dimensions)?;
        let height = u32::try_from(output_height).map_err(|_| ImageError::Dimensions)?;
        let byte_count = self.validate_content_geometry(width, height)?;
        let mut pixels = vec![0; byte_count];

        for (old, old_anchor_row) in targets {
            let Some(old_cell_size) = old.source_cell_size else {
                continue;
            };
            if old_cell_size != cell_size {
                continue;
            }
            let Some(old_content) = self.catalog(screen).contents.get(&old.content_id) else {
                continue;
            };
            let old_origin_x = old
                .column
                .checked_mul(usize::try_from(cell_size.width).map_err(|_| ImageError::Dimensions)?)
                .ok_or(ImageError::Dimensions)?;
            let old_origin_y = old_anchor_row
                .checked_mul(usize::try_from(cell_size.height).map_err(|_| ImageError::Dimensions)?)
                .ok_or(ImageError::Dimensions)?;
            let old_content_width =
                usize::try_from(old_content.metadata.width).map_err(|_| ImageError::Dimensions)?;
            for source_y in
                0..usize::try_from(old.source.height).map_err(|_| ImageError::Dimensions)?
            {
                let global_y = old_origin_y
                    .checked_add(source_y)
                    .ok_or(ImageError::Dimensions)?;
                let Some(output_y) = global_y.checked_sub(new_origin_y) else {
                    continue;
                };
                if output_y >= output_height {
                    continue;
                }
                for source_x in
                    0..usize::try_from(old.source.width).map_err(|_| ImageError::Dimensions)?
                {
                    let global_x = old_origin_x
                        .checked_add(source_x)
                        .ok_or(ImageError::Dimensions)?;
                    let Some(output_x) = global_x.checked_sub(new_origin_x) else {
                        continue;
                    };
                    if output_x >= output_width {
                        continue;
                    }
                    let old_x = usize::try_from(old.source.x)
                        .map_err(|_| ImageError::Dimensions)?
                        .checked_add(source_x)
                        .ok_or(ImageError::Dimensions)?;
                    let old_y = usize::try_from(old.source.y)
                        .map_err(|_| ImageError::Dimensions)?
                        .checked_add(source_y)
                        .ok_or(ImageError::Dimensions)?;
                    let old_index = old_y
                        .checked_mul(old_content_width)
                        .and_then(|base| base.checked_add(old_x))
                        .and_then(|pixel| pixel.checked_mul(4))
                        .ok_or(ImageError::Dimensions)?;
                    let output_index = output_y
                        .checked_mul(output_width)
                        .and_then(|base| base.checked_add(output_x))
                        .and_then(|pixel| pixel.checked_mul(4))
                        .ok_or(ImageError::Dimensions)?;
                    pixels[output_index..output_index + 4]
                        .copy_from_slice(&old_content.pixels[old_index..old_index + 4]);
                }
            }
        }

        let source_width = usize::try_from(content.width).map_err(|_| ImageError::Dimensions)?;
        let source_height = usize::try_from(content.height).map_err(|_| ImageError::Dimensions)?;
        for y in 0..source_height {
            for x in 0..source_width {
                let source_index = y
                    .checked_mul(source_width)
                    .and_then(|base| base.checked_add(x))
                    .and_then(|pixel| pixel.checked_mul(4))
                    .ok_or(ImageError::Dimensions)?;
                let output_index = y
                    .checked_mul(output_width)
                    .and_then(|base| base.checked_add(x))
                    .and_then(|pixel| pixel.checked_mul(4))
                    .ok_or(ImageError::Dimensions)?;
                composite_bgra(
                    &mut pixels[output_index..output_index + 4],
                    &content.pixels[source_index..source_index + 4],
                );
            }
        }
        let alpha_mode = if pixels.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX) {
            ImageAlphaMode::Opaque
        } else {
            ImageAlphaMode::Premultiplied
        };
        let prepared = PreparedSixel {
            width,
            height,
            alpha_mode,
            pixels,
        };
        self.validate_content(&NewImageContent {
            width: prepared.width,
            height: prepared.height,
            source_format: content.source_format,
            alpha_mode: prepared.alpha_mode,
            pixels: &prepared.pixels,
            retention: content.retention,
        })?;
        Ok(prepared)
    }

    fn split_text_overlaps(
        &mut self,
        screen: ActiveScreen,
        row_order: &[u64],
        start_row: usize,
        end_row: usize,
        start_column: usize,
        end_column: usize,
    ) -> Result<bool, ImageError> {
        let targets = self.text_overlap_targets(
            screen,
            row_order,
            start_row,
            end_row,
            start_column,
            end_column,
        );
        for (placement, anchor_row) in &targets {
            let fragments = placement_fragments(
                *placement,
                *anchor_row,
                row_order,
                start_row,
                end_row,
                start_column,
                end_column,
            )?;
            self.catalog_mut(screen).placements.remove(&placement.id);
            self.metrics.placement_count -= 1;
            for fragment in fragments {
                self.insert_placement(screen, fragment)?;
            }
        }
        if !targets.is_empty() {
            self.reclaim_unplaced_while_placed();
        }
        Ok(!targets.is_empty())
    }

    fn remove_text_overlaps_whole(
        &mut self,
        screen: ActiveScreen,
        row_order: &[u64],
        start_row: usize,
        end_row: usize,
        start_column: usize,
        end_column: usize,
    ) -> bool {
        let targets = self.text_overlap_targets(
            screen,
            row_order,
            start_row,
            end_row,
            start_column,
            end_column,
        );
        for (placement, _) in &targets {
            self.catalog_mut(screen).placements.remove(&placement.id);
            self.metrics.placement_count -= 1;
        }
        if !targets.is_empty() {
            self.reclaim_unplaced_while_placed();
        }
        !targets.is_empty()
    }

    fn text_overlap_targets(
        &self,
        screen: ActiveScreen,
        row_order: &[u64],
        start_row: usize,
        end_row: usize,
        start_column: usize,
        end_column: usize,
    ) -> Vec<(ImagePlacement, usize)> {
        if start_row >= end_row || start_column >= end_column {
            return Vec::new();
        }
        let row_positions: BTreeMap<_, _> = row_order
            .iter()
            .copied()
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect();
        self.catalog(screen)
            .placements
            .values()
            .filter_map(|placement| {
                if placement.erase_policy != ImageErasePolicy::TextOverwrite {
                    return None;
                }
                let anchor_row = row_positions.get(&placement.row_id).copied()?;
                let placement_end_row = anchor_row.saturating_add(placement.destination.rows);
                let placement_end_column = placement
                    .column
                    .saturating_add(placement.destination.columns);
                (anchor_row < end_row
                    && placement_end_row > start_row
                    && placement.column < end_column
                    && placement_end_column > start_column)
                    .then_some((*placement, anchor_row))
            })
            .collect()
    }

    pub fn remove_text_placements_outside_columns(
        &mut self,
        screen: ActiveScreen,
        columns: usize,
    ) -> bool {
        let removed = self
            .catalog(screen)
            .placements
            .values()
            .filter(|placement| {
                placement.erase_policy == ImageErasePolicy::TextOverwrite
                    && placement.column >= columns
            })
            .map(|placement| placement.id)
            .collect::<Vec<_>>();
        for id in &removed {
            self.catalog_mut(screen).placements.remove(id);
            self.metrics.placement_count -= 1;
        }
        if !removed.is_empty() {
            self.reclaim_unplaced_while_placed();
        }
        !removed.is_empty()
    }

    pub fn resolve_text_overlaps(&mut self, screen: ActiveScreen, row_order: &[u64]) -> bool {
        let row_positions: BTreeMap<_, _> = row_order
            .iter()
            .copied()
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect();
        let mut placements = self
            .catalog(screen)
            .placements
            .values()
            .filter(|placement| placement.erase_policy == ImageErasePolicy::TextOverwrite)
            .copied()
            .collect::<Vec<_>>();
        placements.sort_by_key(|placement| placement.creation_order);
        let participants = placements
            .iter()
            .copied()
            .filter(|placement| {
                placements.iter().any(|other| {
                    placement.id != other.id
                        && placements_overlap(*placement, *other, &row_positions)
                })
            })
            .collect::<Vec<_>>();
        if participants.is_empty() {
            return false;
        }

        let mut candidate = self.clone();
        for placement in &participants {
            candidate
                .catalog_mut(screen)
                .placements
                .remove(&placement.id);
            candidate.metrics.placement_count -= 1;
        }
        for placement in participants {
            let Some(anchor_row) = row_positions.get(&placement.row_id).copied() else {
                continue;
            };
            let end_row = anchor_row.saturating_add(placement.destination.rows);
            let end_column = placement
                .column
                .saturating_add(placement.destination.columns);
            if candidate
                .split_text_overlaps(
                    screen,
                    row_order,
                    anchor_row,
                    end_row,
                    placement.column,
                    end_column,
                )
                .is_err()
            {
                candidate.remove_text_overlaps_whole(
                    screen,
                    row_order,
                    anchor_row,
                    end_row,
                    placement.column,
                    end_column,
                );
            }
            candidate
                .catalog_mut(screen)
                .placements
                .insert(placement.id, placement);
            candidate.metrics.placement_count += 1;
        }
        candidate.reclaim_unplaced_while_placed();
        *self = candidate;
        true
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
        let expected = self.validate_content_geometry(input.width, input.height)?;
        if input.pixels.len() != expected {
            return Err(ImageError::PixelLength);
        }
        Ok(expected)
    }

    fn validate_content_geometry(&self, width: u32, height: u32) -> Result<usize, ImageError> {
        if width == 0
            || height == 0
            || width > self.limits.maximum_dimension
            || height > self.limits.maximum_dimension
        {
            return Err(ImageError::Dimensions);
        }
        let pixels = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .filter(|pixels| *pixels <= self.limits.maximum_pixels)
            .ok_or(ImageError::Dimensions)?;
        let expected = pixels.checked_mul(4).ok_or(ImageError::PixelLength)?;
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
        let reservation = self.catalog_mut(screen).reservations.remove(&id);
        debug_assert_eq!(reservation.is_some(), self.shared_budget.is_some());
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

fn composite_bgra(destination: &mut [u8], source: &[u8]) {
    let inverse_alpha = u16::from(u8::MAX - source[3]);
    for channel in 0..3 {
        let value = u16::from(source[channel])
            + (u16::from(destination[channel]) * inverse_alpha + 127) / 255;
        destination[channel] = u8::try_from(value.min(u16::from(u8::MAX))).unwrap();
    }
    let alpha = u16::from(source[3]) + (u16::from(destination[3]) * inverse_alpha + 127) / 255;
    destination[3] = u8::try_from(alpha.min(u16::from(u8::MAX))).unwrap();
}

fn placements_overlap(
    left: ImagePlacement,
    right: ImagePlacement,
    row_positions: &BTreeMap<u64, usize>,
) -> bool {
    let (Some(left_row), Some(right_row)) = (
        row_positions.get(&left.row_id).copied(),
        row_positions.get(&right.row_id).copied(),
    ) else {
        return false;
    };
    left_row < right_row.saturating_add(right.destination.rows)
        && left_row.saturating_add(left.destination.rows) > right_row
        && left.column < right.column.saturating_add(right.destination.columns)
        && left.column.saturating_add(left.destination.columns) > right.column
}

#[allow(
    clippy::too_many_lines,
    reason = "the four Foot rectangle-subtraction regions share one checked cell-to-pixel mapping"
)]
fn placement_fragments(
    placement: ImagePlacement,
    anchor_row: usize,
    row_order: &[u64],
    start_row: usize,
    end_row: usize,
    start_column: usize,
    end_column: usize,
) -> Result<Vec<NewImagePlacement>, ImageError> {
    let Some(cell_size) = placement.source_cell_size else {
        return Ok(Vec::new());
    };
    if cell_size.width == 0 || cell_size.height == 0 {
        return Err(ImageError::InvalidDestination);
    }
    let relative_left = start_column
        .saturating_sub(placement.column)
        .min(placement.destination.columns);
    let relative_right = end_column
        .saturating_sub(placement.column)
        .min(placement.destination.columns)
        .max(relative_left);
    let relative_top = start_row
        .saturating_sub(anchor_row)
        .min(placement.destination.rows);
    let relative_bottom = end_row
        .saturating_sub(anchor_row)
        .min(placement.destination.rows)
        .max(relative_top);

    let mut fragments = Vec::with_capacity(4);
    let mut push_fragment =
        |x: usize, y: usize, columns: usize, rows: usize| -> Result<(), ImageError> {
            if columns == 0 || rows == 0 {
                return Ok(());
            }
            let pixel_x = u32::try_from(x)
                .ok()
                .and_then(|value| value.checked_mul(cell_size.width))
                .map(|value| value.min(placement.source.width))
                .ok_or(ImageError::InvalidCrop)?;
            let pixel_y = u32::try_from(y)
                .ok()
                .and_then(|value| value.checked_mul(cell_size.height))
                .map(|value| value.min(placement.source.height))
                .ok_or(ImageError::InvalidCrop)?;
            let pixel_end_x = u32::try_from(x.checked_add(columns).ok_or(ImageError::InvalidCrop)?)
                .ok()
                .and_then(|value| value.checked_mul(cell_size.width))
                .map(|value| value.min(placement.source.width))
                .ok_or(ImageError::InvalidCrop)?;
            let pixel_end_y = u32::try_from(y.checked_add(rows).ok_or(ImageError::InvalidCrop)?)
                .ok()
                .and_then(|value| value.checked_mul(cell_size.height))
                .map(|value| value.min(placement.source.height))
                .ok_or(ImageError::InvalidCrop)?;
            let width = pixel_end_x
                .checked_sub(pixel_x)
                .filter(|value| *value > 0)
                .ok_or(ImageError::InvalidCrop)?;
            let height = pixel_end_y
                .checked_sub(pixel_y)
                .filter(|value| *value > 0)
                .ok_or(ImageError::InvalidCrop)?;
            let row_id = *row_order
                .get(anchor_row.checked_add(y).ok_or(ImageError::InvalidAnchor)?)
                .ok_or(ImageError::InvalidAnchor)?;
            fragments.push(NewImagePlacement {
                content_id: placement.content_id,
                row_id,
                column: placement
                    .column
                    .checked_add(x)
                    .ok_or(ImageError::InvalidDestination)?,
                source: PixelRect {
                    x: placement
                        .source
                        .x
                        .checked_add(pixel_x)
                        .ok_or(ImageError::InvalidCrop)?,
                    y: placement
                        .source
                        .y
                        .checked_add(pixel_y)
                        .ok_or(ImageError::InvalidCrop)?,
                    width,
                    height,
                },
                destination: CellExtent { columns, rows },
                source_cell_size: placement.source_cell_size,
                x_offset: if x == 0 { placement.x_offset } else { 0 },
                y_offset: if y == 0 { placement.y_offset } else { 0 },
                z_index: placement.z_index,
                application_image_id: placement.application_image_id,
                application_placement_id: placement.application_placement_id,
                erase_policy: placement.erase_policy,
            });
            Ok(())
        };

    push_fragment(0, 0, placement.destination.columns, relative_top)?;
    push_fragment(
        0,
        relative_bottom,
        placement.destination.columns,
        placement.destination.rows - relative_bottom,
    )?;
    push_fragment(
        0,
        relative_top,
        relative_left,
        relative_bottom - relative_top,
    )?;
    push_fragment(
        relative_right,
        relative_top,
        placement.destination.columns - relative_right,
        relative_bottom - relative_top,
    )?;
    Ok(fragments)
}

fn validate_source_cell_size(cell_size: Option<PixelSize>) -> Result<(), ImageError> {
    if cell_size.is_some_and(|size| size.width == 0 || size.height == 0) {
        return Err(ImageError::InvalidDestination);
    }
    Ok(())
}

fn validate_sixel_source_cell_size(
    source: PixelRect,
    destination: CellExtent,
    cell_size: Option<PixelSize>,
) -> Result<(), ImageError> {
    let Some(cell_size) = cell_size else {
        return Ok(());
    };
    if cell_size.width == 0 || cell_size.height == 0 {
        return Err(ImageError::InvalidDestination);
    }
    let columns = u32::try_from(destination.columns).map_err(|_| ImageError::InvalidDestination)?;
    let rows = u32::try_from(destination.rows).map_err(|_| ImageError::InvalidDestination)?;
    let maximum_width = columns
        .checked_mul(cell_size.width)
        .ok_or(ImageError::InvalidDestination)?;
    let maximum_height = rows
        .checked_mul(cell_size.height)
        .ok_or(ImageError::InvalidDestination)?;
    let minimum_width = columns
        .saturating_sub(1)
        .checked_mul(cell_size.width)
        .ok_or(ImageError::InvalidDestination)?;
    let minimum_height = rows
        .saturating_sub(1)
        .checked_mul(cell_size.height)
        .ok_or(ImageError::InvalidDestination)?;
    if source.width > maximum_width
        || source.height > maximum_height
        || source.width <= minimum_width
        || source.height <= minimum_height
    {
        return Err(ImageError::InvalidDestination);
    }
    Ok(())
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
            source_cell_size: Some(PixelSize {
                width: 1,
                height: 1,
            }),
            x_offset: 0,
            y_offset: 0,
            z_index,
            application_image_id: None,
            application_placement_id: None,
            erase_policy: ImageErasePolicy::TextOverwrite,
        }
    }

    #[test]
    fn shared_authoritative_budget_is_exact_across_planes_clones_and_release() {
        let budget = SharedImageBudget::new(8);
        let mut first =
            ImagePlane::new_with_shared_budget(ImageLimits::default(), Some(budget.clone()));
        let mut second =
            ImagePlane::new_with_shared_budget(ImageLimits::default(), Some(budget.clone()));
        let first_id = first
            .insert_content(
                ActiveScreen::Normal,
                content(&[1, 2, 3, 255], ImageRetention::ExplicitDelete),
            )
            .unwrap();
        let second_id = second
            .insert_content(
                ActiveScreen::Normal,
                content(&[4, 5, 6, 255], ImageRetention::ExplicitDelete),
            )
            .unwrap();
        assert_eq!(
            budget.metrics(),
            SharedImageBudgetMetrics {
                content_bytes: 8,
                high_water_content_bytes: 8,
            }
        );
        assert_eq!(
            second.insert_content(
                ActiveScreen::Normal,
                content(&[7, 8, 9, 255], ImageRetention::ExplicitDelete),
            ),
            Err(ImageError::DaemonBytes)
        );
        assert_eq!(budget.metrics().content_bytes, 8);

        let cloned_plane = first.clone();
        first
            .remove_content(ActiveScreen::Normal, first_id)
            .unwrap();
        assert_eq!(budget.metrics().content_bytes, 8);
        drop(cloned_plane);
        assert_eq!(budget.metrics().content_bytes, 4);

        let transfer_clone = second
            .content(ActiveScreen::Normal, second_id)
            .unwrap()
            .clone();
        second
            .remove_content(ActiveScreen::Normal, second_id)
            .unwrap();
        assert_eq!(budget.metrics().content_bytes, 0);
        assert_eq!(transfer_clone.pixels(), &[4, 5, 6, 255]);

        first
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 1,
                    height: 2,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &[1, 2, 3, 255, 4, 5, 6, 255],
                    retention: ImageRetention::ExplicitDelete,
                },
            )
            .unwrap();
        assert_eq!(budget.metrics().content_bytes, 8);
        drop(first);
        assert_eq!(budget.metrics().content_bytes, 0);
        assert_eq!(budget.metrics().high_water_content_bytes, 8);
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
    #[allow(
        clippy::too_many_lines,
        reason = "the four expected Foot subtraction regions are asserted explicitly"
    )]
    fn text_overlap_splits_cell_aligned_sixel_fragments_without_copying_content() {
        let mut plane = ImagePlane::default();
        let pixels = [0, 0, 255, 255].repeat(9);
        let content_id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 3,
                    height: 3,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &pixels,
                    retention: ImageRetention::WhilePlaced,
                },
            )
            .unwrap();
        let mut input = placement(content_id, 10, 0);
        input.source = PixelRect {
            x: 0,
            y: 0,
            width: 3,
            height: 3,
        };
        input.destination = CellExtent {
            columns: 3,
            rows: 3,
        };
        plane.insert_placement(ActiveScreen::Normal, input).unwrap();

        assert!(plane.remove_text_overlaps(ActiveScreen::Normal, &[10, 11, 12], 1, 2, 1, 2,));
        let fragments = plane
            .placements(ActiveScreen::Normal)
            .map(|placement| {
                (
                    placement.row_id,
                    placement.column,
                    placement.source,
                    placement.destination,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            fragments,
            vec![
                (
                    10,
                    0,
                    PixelRect {
                        x: 0,
                        y: 0,
                        width: 3,
                        height: 1,
                    },
                    CellExtent {
                        columns: 3,
                        rows: 1,
                    },
                ),
                (
                    12,
                    0,
                    PixelRect {
                        x: 0,
                        y: 2,
                        width: 3,
                        height: 1,
                    },
                    CellExtent {
                        columns: 3,
                        rows: 1,
                    },
                ),
                (
                    11,
                    0,
                    PixelRect {
                        x: 0,
                        y: 1,
                        width: 1,
                        height: 1,
                    },
                    CellExtent {
                        columns: 1,
                        rows: 1,
                    },
                ),
                (
                    11,
                    2,
                    PixelRect {
                        x: 2,
                        y: 1,
                        width: 1,
                        height: 1,
                    },
                    CellExtent {
                        columns: 1,
                        rows: 1,
                    },
                ),
            ]
        );
        assert_eq!(plane.metrics().content_count, 1);
        assert_eq!(plane.metrics().placement_count, 4);
        assert_eq!(plane.metrics().content_bytes, pixels.len());
    }

    #[test]
    fn transparent_and_partial_sixel_overlap_preserve_old_pixels() {
        let build = |new_pixels: &[u8], new_width: u32, new_height: u32| {
            let mut plane = ImagePlane::default();
            let old_pixels = [0, 0, 255, 255].repeat(4);
            let old_id = plane
                .insert_content(
                    ActiveScreen::Normal,
                    NewImageContent {
                        width: 2,
                        height: 2,
                        source_format: ImageSourceFormat::Sixel,
                        alpha_mode: ImageAlphaMode::Opaque,
                        pixels: &old_pixels,
                        retention: ImageRetention::WhilePlaced,
                    },
                )
                .unwrap();
            let mut old = placement(old_id, 10, 0);
            old.source = PixelRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            };
            old.source_cell_size = Some(PixelSize {
                width: 2,
                height: 2,
            });
            plane.insert_placement(ActiveScreen::Normal, old).unwrap();
            let (_, placement_id) = plane
                .insert_sixel_content_and_placement(
                    ActiveScreen::Normal,
                    NewImageContent {
                        width: new_width,
                        height: new_height,
                        source_format: ImageSourceFormat::Sixel,
                        alpha_mode: ImageAlphaMode::Premultiplied,
                        pixels: new_pixels,
                        retention: ImageRetention::WhilePlaced,
                    },
                    &[10],
                    10,
                    NewImagePlacementOptions {
                        column: 0,
                        source: PixelRect {
                            x: 0,
                            y: 0,
                            width: new_width,
                            height: new_height,
                        },
                        destination: CellExtent {
                            columns: 1,
                            rows: 1,
                        },
                        source_cell_size: Some(PixelSize {
                            width: 2,
                            height: 2,
                        }),
                        x_offset: 0,
                        y_offset: 0,
                        z_index: -1,
                        application_image_id: None,
                        application_placement_id: None,
                        erase_policy: ImageErasePolicy::TextOverwrite,
                    },
                )
                .unwrap();
            let placement = plane
                .placements(ActiveScreen::Normal)
                .find(|placement| placement.id == placement_id)
                .unwrap();
            plane
                .content(ActiveScreen::Normal, placement.content_id)
                .unwrap()
                .pixels()
                .to_vec()
        };

        assert_eq!(
            build(&[0, 255, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 2, 2),
            vec![
                0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255
            ]
        );
        assert_eq!(
            build(&[0, 255, 0, 255], 1, 1),
            vec![
                0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255
            ]
        );
    }

    #[test]
    fn partial_overlap_bounds_before_allocation_and_uses_actual_old_edge() {
        let mut plane = ImagePlane::default();
        let old_id = plane
            .insert_content(
                ActiveScreen::Normal,
                content(&[0, 0, 255, 255], ImageRetention::WhilePlaced),
            )
            .unwrap();
        let mut old = placement(old_id, 10, 0);
        old.source_cell_size = Some(PixelSize {
            width: 4096,
            height: 4096,
        });
        plane.insert_placement(ActiveScreen::Normal, old).unwrap();
        let transparent = [0, 0, 0, 0];
        let (new_id, _) = plane
            .insert_sixel_content_and_placement(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 1,
                    height: 1,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Premultiplied,
                    pixels: &transparent,
                    retention: ImageRetention::WhilePlaced,
                },
                &[10],
                10,
                NewImagePlacementOptions {
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
                    source_cell_size: Some(PixelSize {
                        width: 4096,
                        height: 4096,
                    }),
                    x_offset: 0,
                    y_offset: 0,
                    z_index: -1,
                    application_image_id: None,
                    application_placement_id: None,
                    erase_policy: ImageErasePolicy::TextOverwrite,
                },
            )
            .unwrap();
        let content = plane.content(ActiveScreen::Normal, new_id).unwrap();
        assert_eq!(
            (content.metadata().width, content.metadata().height),
            (1, 1)
        );
        assert_eq!(content.pixels(), &[0, 0, 255, 255]);
    }

    #[test]
    fn partial_edge_cell_crop_uses_original_sixel_cell_geometry() {
        let mut plane = ImagePlane::default();
        let pixels = [0, 0, 255, 255].repeat(100);
        let content_id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 10,
                    height: 10,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &pixels,
                    retention: ImageRetention::WhilePlaced,
                },
            )
            .unwrap();
        let mut input = placement(content_id, 10, 0);
        input.source = PixelRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        input.destination = CellExtent {
            columns: 2,
            rows: 2,
        };
        input.source_cell_size = Some(PixelSize {
            width: 8,
            height: 8,
        });
        plane.insert_placement(ActiveScreen::Normal, input).unwrap();

        assert!(plane.remove_text_overlaps(ActiveScreen::Normal, &[10, 11], 0, 1, 0, 1,));
        let fragments = plane
            .placements(ActiveScreen::Normal)
            .map(|placement| (placement.column, placement.source, placement.destination))
            .collect::<Vec<_>>();
        assert_eq!(
            fragments,
            vec![
                (
                    0,
                    PixelRect {
                        x: 0,
                        y: 8,
                        width: 10,
                        height: 2,
                    },
                    CellExtent {
                        columns: 2,
                        rows: 1,
                    },
                ),
                (
                    1,
                    PixelRect {
                        x: 8,
                        y: 0,
                        width: 2,
                        height: 8,
                    },
                    CellExtent {
                        columns: 1,
                        rows: 1,
                    },
                ),
            ]
        );
    }

    #[test]
    fn reflow_created_overlap_preserves_newer_sixel_and_crops_older() {
        let mut plane = ImagePlane::default();
        let pixels = [0, 0, 255, 255].repeat(3);
        let content_id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 3,
                    height: 1,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &pixels,
                    retention: ImageRetention::WhilePlaced,
                },
            )
            .unwrap();
        let mut older = placement(content_id, 10, 0);
        older.source.width = 3;
        older.destination.columns = 3;
        let older_id = plane.insert_placement(ActiveScreen::Normal, older).unwrap();
        let mut newer = placement(content_id, 11, 0);
        newer.column = 1;
        newer.source.width = 2;
        newer.destination.columns = 2;
        let newer_id = plane.insert_placement(ActiveScreen::Normal, newer).unwrap();
        let mut unrelated = placement(content_id, 12, 0);
        unrelated.column = 5;
        let unrelated_id = plane
            .insert_placement(ActiveScreen::Normal, unrelated)
            .unwrap();

        assert!(plane.remap_anchors(
            ActiveScreen::Normal,
            &BTreeMap::from([(10, 20), (11, 20), (12, 20)]),
        ));
        assert!(plane.resolve_text_overlaps(ActiveScreen::Normal, &[20]));
        let placements = plane.placements(ActiveScreen::Normal).collect::<Vec<_>>();
        assert_eq!(placements.len(), 3);
        assert!(placements.iter().all(|placement| placement.id != older_id));
        assert!(placements.iter().any(|placement| placement.id == newer_id));
        assert!(
            placements
                .iter()
                .any(|placement| placement.id == unrelated_id)
        );
        assert!(placements.iter().any(|placement| {
            placement.column == 0
                && placement.destination.columns == 1
                && placement.source.width == 1
        }));
        assert!(placements.iter().any(|placement| {
            placement.column == 1
                && placement.destination.columns == 2
                && placement.source.width == 2
        }));
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
