//! Pure terminal/window geometry and font-sizing contracts.
//!
//! Logical surface extents and configured padding are converted exactly once.
//! The fitted grid is top-left anchored and every trailing residual belongs to
//! the right and bottom edges.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_arguments,
    reason = "the bounded numeric contract keeps conversions and exact-value tests adjacent"
)]

use anyhow::{Context, Result, bail};

pub const SCALE_DENOMINATOR: u32 = 120;
pub const MIN_SCALE_120: u32 = 120;
pub const MAX_SCALE_120: u32 = 960;
pub const DEFAULT_DPI: f32 = 96.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalPx(u32);

impl LogicalPx {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferPx(u32);

impl BufferPx {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceScale120(u32);

impl SurfaceScale120 {
    pub fn new(value: u32) -> Result<Self> {
        validate_scale(value)?;
        Ok(Self(value))
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalSize {
    pub width: LogicalPx,
    pub height: LogicalPx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferSize {
    pub width: BufferPx,
    pub height: BufferPx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceGeometry {
    pub logical_size: LogicalSize,
    pub buffer_size: BufferSize,
    pub scale: SurfaceScale120,
}

impl SurfaceGeometry {
    pub fn new(logical_width: u32, logical_height: u32, scale_120: u32) -> Result<Self> {
        let scale = SurfaceScale120::new(scale_120)?;
        Ok(Self {
            logical_size: LogicalSize {
                width: LogicalPx::new(logical_width),
                height: LogicalPx::new(logical_height),
            },
            buffer_size: BufferSize {
                width: BufferPx::new(logical_extent_to_buffer(logical_width, scale_120)?),
                height: BufferPx::new(logical_extent_to_buffer(logical_height, scale_120)?),
            },
            scale,
        })
    }

    pub fn buffer_layout(self) -> Result<(u32, u32, i32)> {
        let width = self.buffer_size.width.get();
        let height = self.buffer_size.height.get();
        let stride = i32::try_from(width.checked_mul(4).context("buffer stride overflow")?)
            .context("buffer stride fits i32")?;
        i32::try_from(width).context("buffer width fits i32")?;
        i32::try_from(height).context("buffer height fits i32")?;
        Ok((width, height, stride))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FontSize {
    Pixels(f32),
    Points(f32),
}

impl FontSize {
    pub fn value(self) -> f32 {
        match self {
            Self::Pixels(value) | Self::Points(value) => value,
        }
    }

    pub const fn unit_name(self) -> &'static str {
        match self {
            Self::Pixels(_) => "pixels",
            Self::Points(_) => "points",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontSizingPolicy {
    OutputScale,
    PhysicalDpi,
}

impl FontSizingPolicy {
    pub const fn name(self) -> &'static str {
        match self {
            Self::OutputScale => "output-scale",
            Self::PhysicalDpi => "physical-dpi",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputDpiObservation {
    pub dpi: f32,
    pub output_id: Option<u32>,
    pub output_name: Option<String>,
    pub source: &'static str,
    pub fallback_reason: Option<&'static str>,
}

impl OutputDpiObservation {
    pub const fn unavailable(reason: &'static str) -> Self {
        Self {
            dpi: DEFAULT_DPI,
            output_id: None,
            output_name: None,
            source: "fallback-96-dpi",
            fallback_reason: Some(reason),
        }
    }

    pub fn provided(dpi: f32) -> Result<Self> {
        validate_dpi(dpi)?;
        Ok(Self {
            dpi,
            output_id: None,
            output_name: None,
            source: "provided-physical-dpi",
            fallback_reason: None,
        })
    }

    pub fn from_wayland(
        output_id: u32,
        output_name: Option<String>,
        current_mode: Option<(i32, i32)>,
        physical_size_mm: (i32, i32),
    ) -> Self {
        let fallback = |reason| Self {
            dpi: DEFAULT_DPI,
            output_id: Some(output_id),
            output_name: output_name.clone(),
            source: "fallback-96-dpi",
            fallback_reason: Some(reason),
        };
        let Some((pixel_width, pixel_height)) = current_mode else {
            return fallback("missing-current-mode");
        };
        let (millimeter_width, millimeter_height) = physical_size_mm;
        if pixel_width <= 0 || pixel_height <= 0 {
            return fallback("invalid-current-mode");
        }
        if millimeter_width <= 0 || millimeter_height <= 0 {
            return fallback("missing-physical-size");
        }
        let pixel_diagonal = f64::from(pixel_width).hypot(f64::from(pixel_height));
        let millimeter_diagonal = f64::from(millimeter_width).hypot(f64::from(millimeter_height));
        let dpi = pixel_diagonal / millimeter_diagonal * 25.4;
        if !dpi.is_finite() || dpi <= 0.0 || dpi > 1_000.0 {
            return fallback("unreasonable-physical-dpi");
        }
        Self {
            dpi: dpi as f32,
            output_id: Some(output_id),
            output_name,
            source: "wayland-mode-and-physical-size",
            fallback_reason: None,
        }
    }
}

fn validate_dpi(dpi: f32) -> Result<()> {
    if !dpi.is_finite() || dpi <= 0.0 || dpi > 1_000.0 {
        bail!("physical DPI must be finite and between 0 and 1000");
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedFontSize {
    pub source: FontSize,
    pub policy: FontSizingPolicy,
    pub surface_scale_120: u32,
    /// DPI observed/provided by the output source, whether or not sizing used it.
    pub observed_output_dpi: Option<f32>,
    pub observed_output_id: Option<u32>,
    pub observed_output_name: Option<String>,
    pub observed_dpi_source: &'static str,
    pub observed_dpi_fallback_reason: Option<&'static str>,
    /// DPI which affected raster sizing. Pixel sizes under `PhysicalDpi` use none.
    pub sizing_dpi: Option<f32>,
    pub dpi_source: &'static str,
    pub effective_pixel_size_26_6: u32,
    pub pixel_size: f32,
}

pub fn resolve_font_size(
    source: FontSize,
    policy: FontSizingPolicy,
    surface_scale_120: u32,
    physical_dpi: f32,
) -> Result<ResolvedFontSize> {
    resolve_font_size_with_output(
        source,
        policy,
        surface_scale_120,
        &OutputDpiObservation::provided(physical_dpi)?,
    )
}

pub fn resolve_font_size_with_output(
    source: FontSize,
    policy: FontSizingPolicy,
    surface_scale_120: u32,
    output_dpi: &OutputDpiObservation,
) -> Result<ResolvedFontSize> {
    validate_scale(surface_scale_120)?;
    if !source.value().is_finite() || source.value() <= 0.0 {
        bail!("font size must be finite and positive");
    }
    validate_dpi(output_dpi.dpi)?;
    let physical_dpi = output_dpi.dpi;
    let output_scale_dpi = DEFAULT_DPI * surface_scale_120 as f32 / SCALE_DENOMINATOR as f32;
    let (pixel_size, sizing_dpi, dpi_source) = match (source, policy) {
        (FontSize::Pixels(value), FontSizingPolicy::OutputScale) => (
            value * surface_scale_120 as f32 / SCALE_DENOMINATOR as f32,
            Some(output_scale_dpi),
            "output-scale",
        ),
        (FontSize::Points(value), FontSizingPolicy::OutputScale) => (
            value * output_scale_dpi / 72.0,
            Some(output_scale_dpi),
            "output-scale",
        ),
        (FontSize::Pixels(value), FontSizingPolicy::PhysicalDpi) => {
            (value, None, "not-used-for-pixels")
        }
        (FontSize::Points(value), FontSizingPolicy::PhysicalDpi) => (
            value * physical_dpi / 72.0,
            Some(physical_dpi),
            "provided-physical-dpi",
        ),
    };
    if !pixel_size.is_finite() || pixel_size <= 0.0 {
        bail!("resolved font pixel size is invalid");
    }
    // Pinned fcft passes fractional pixel values to FT_Set_Char_Size(). Round
    // only when converting to FreeType's 26.6 request and cache-key boundary.
    let rounded_pixel_size_26_6 = (pixel_size * 64.0).round();
    if !(64.0..=(768.0 * 64.0)).contains(&rounded_pixel_size_26_6) {
        bail!("resolved FreeType font size is out of range");
    }
    let effective_pixel_size_26_6 = rounded_pixel_size_26_6 as u32;
    Ok(ResolvedFontSize {
        source,
        policy,
        surface_scale_120,
        observed_output_dpi: Some(physical_dpi),
        observed_output_id: output_dpi.output_id,
        observed_output_name: output_dpi.output_name.clone(),
        observed_dpi_source: output_dpi.source,
        observed_dpi_fallback_reason: output_dpi.fallback_reason,
        sizing_dpi,
        dpi_source,
        effective_pixel_size_26_6,
        pixel_size: effective_pixel_size_26_6 as f32 / 64.0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvancePolicy {
    IntegerPrimaryAdvance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellGeometry {
    pub width: u32,
    pub height: u32,
    pub ascent: u32,
    pub descent: u32,
    pub baseline_from_top: u32,
    pub advance_policy: AdvancePolicy,
}

impl CellGeometry {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        Self::from_metrics(width, height, height, 0, height)
    }

    pub fn from_metrics(
        width: u32,
        height: u32,
        ascent: u32,
        descent: u32,
        baseline_from_top: u32,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("cell dimensions must be positive");
        }
        if ascent
            .checked_add(descent)
            .context("cell metrics overflow")?
            > height
        {
            bail!("cell height must contain ascent and descent");
        }
        if baseline_from_top != height - descent {
            bail!("cell baseline must equal height minus descent");
        }
        Ok(Self {
            width,
            height,
            ascent,
            descent,
            baseline_from_top,
            advance_policy: AdvancePolicy::IntegerPrimaryAdvance,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalPadding {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl TerminalPadding {
    pub const DEFAULT: Self = Self::uniform(12);

    pub const fn uniform(value: u32) -> Self {
        Self {
            left: value,
            right: value,
            top: value,
            bottom: value,
        }
    }

    pub fn to_buffer_floor(self, scale_120: u32) -> Result<BufferPadding> {
        Ok(BufferPadding {
            left: logical_padding_to_buffer(self.left, scale_120)?,
            right: logical_padding_to_buffer(self.right, scale_120)?,
            top: logical_padding_to_buffer(self.top, scale_120)?,
            bottom: logical_padding_to_buffer(self.bottom, scale_120)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BufferPadding {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidualPolicy {
    TrailingEdges,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowGeometry {
    pub surface: SurfaceGeometry,
    pub cell: CellGeometry,
    pub requested_padding: TerminalPadding,
    pub effective_base_padding: BufferPadding,
    pub actual_padding: BufferPadding,
    pub columns: u32,
    pub rows: u32,
    pub grid_rect: Rect,
    pub visible_grid_rect: Rect,
    pub residual_right: u32,
    pub residual_bottom: u32,
    pub residual_policy: ResidualPolicy,
}

impl WindowGeometry {
    /// Computes the initial logical surface needed for an explicitly requested grid.
    pub fn for_grid(
        columns: u32,
        rows: u32,
        cell: CellGeometry,
        padding: TerminalPadding,
        surface_scale_120: u32,
    ) -> Result<Self> {
        if columns == 0 || rows == 0 {
            bail!("grid dimensions must be positive");
        }
        let surface_scale = SurfaceScale120::new(surface_scale_120)?;
        let base = padding.to_buffer_floor(surface_scale.get())?;
        let desired_width = checked_sum3(
            base.left,
            checked_mul(columns, cell.width, "grid width overflow")?,
            base.right,
            "initial buffer width overflow",
        )?;
        let desired_height = checked_sum3(
            base.top,
            checked_mul(rows, cell.height, "grid height overflow")?,
            base.bottom,
            "initial buffer height overflow",
        )?;
        let logical_width = buffer_to_logical_ceil(desired_width, surface_scale.get())?;
        let logical_height = buffer_to_logical_ceil(desired_height, surface_scale.get())?;
        let surface = SurfaceGeometry::new(logical_width, logical_height, surface_scale.get())?;
        let extra_width = surface.buffer_size.width.get() - desired_width;
        let extra_height = surface.buffer_size.height.get() - desired_height;
        if extra_width >= cell.width || extra_height >= cell.height {
            bail!(
                "UnrepresentableGrid: integer logical extent at scale {} adds a complete cell",
                surface_scale.get()
            );
        }
        Self::from_parts(
            logical_width,
            logical_height,
            cell,
            padding,
            surface_scale_120,
            columns,
            rows,
            base,
        )
    }

    /// Places an explicit grid in an actual compositor-configured logical surface.
    ///
    /// Unlike runtime fitting, this preserves one-row oracle fixtures and assigns
    /// any extra pixels to the trailing edges.
    pub fn grid_in_surface(
        columns: u32,
        rows: u32,
        logical_width: u32,
        logical_height: u32,
        cell: CellGeometry,
        padding: TerminalPadding,
        surface_scale_120: u32,
    ) -> Result<Self> {
        if columns == 0 || rows == 0 {
            bail!("grid dimensions must be positive");
        }
        let base = padding.to_buffer_floor(surface_scale_120)?;
        Self::from_parts(
            logical_width,
            logical_height,
            cell,
            padding,
            surface_scale_120,
            columns,
            rows,
            base,
        )
    }

    /// Fits a protocol grid into an actual compositor-configured logical surface.
    pub fn fit_window(
        logical_width: u32,
        logical_height: u32,
        cell: CellGeometry,
        padding: TerminalPadding,
        surface_scale_120: u32,
        min_columns: u32,
        max_columns: u32,
        min_rows: u32,
        max_rows: u32,
    ) -> Result<Self> {
        let surface_scale = SurfaceScale120::new(surface_scale_120)?;
        if min_columns != 2 || min_rows != 2 || min_columns > max_columns || min_rows > max_rows {
            bail!("grid bounds must reserve the protocol minimum of 2x2 cells");
        }
        let surface = SurfaceGeometry::new(logical_width, logical_height, surface_scale.get())?;
        let buffer_width = surface.buffer_size.width.get();
        let buffer_height = surface.buffer_size.height.get();
        let minimum_width = checked_mul(2, cell.width, "minimum grid width overflow")?;
        let minimum_height = checked_mul(2, cell.height, "minimum grid height overflow")?;
        if buffer_width < minimum_width || buffer_height < minimum_height {
            bail!("SurfaceTooSmall: surface cannot contain the protocol minimum 2x2 grid");
        }
        let requested = padding.to_buffer_floor(surface_scale.get())?;
        let max_horizontal_edge = (buffer_width - minimum_width) / 2;
        let max_vertical_edge = (buffer_height - minimum_height) / 2;
        let base = BufferPadding {
            left: requested.left.min(max_horizontal_edge),
            right: requested.right.min(max_horizontal_edge),
            top: requested.top.min(max_vertical_edge),
            bottom: requested.bottom.min(max_vertical_edge),
        };
        let usable_width = buffer_width - base.left - base.right;
        let usable_height = buffer_height - base.top - base.bottom;
        let columns = (usable_width / cell.width).min(max_columns);
        let rows = (usable_height / cell.height).min(max_rows);
        debug_assert!(columns >= 2 && rows >= 2);
        Self::from_parts(
            logical_width,
            logical_height,
            cell,
            padding,
            surface_scale_120,
            columns,
            rows,
            base,
        )
    }

    fn from_parts(
        logical_width: u32,
        logical_height: u32,
        cell: CellGeometry,
        requested_padding: TerminalPadding,
        surface_scale_120: u32,
        columns: u32,
        rows: u32,
        effective_base_padding: BufferPadding,
    ) -> Result<Self> {
        let surface = SurfaceGeometry::new(logical_width, logical_height, surface_scale_120)?;
        let buffer_width = surface.buffer_size.width.get();
        let buffer_height = surface.buffer_size.height.get();
        let grid_width = checked_mul(columns, cell.width, "grid width overflow")?;
        let grid_height = checked_mul(rows, cell.height, "grid height overflow")?;
        let grid_right = effective_base_padding
            .left
            .checked_add(grid_width)
            .context("grid right overflow")?;
        let grid_bottom = effective_base_padding
            .top
            .checked_add(grid_height)
            .context("grid bottom overflow")?;
        if grid_right > buffer_width || grid_bottom > buffer_height {
            bail!("grid lies outside surface buffer");
        }
        let actual_padding = BufferPadding {
            left: effective_base_padding.left,
            right: buffer_width - grid_right,
            top: effective_base_padding.top,
            bottom: buffer_height - grid_bottom,
        };
        let residual_right = actual_padding
            .right
            .checked_sub(effective_base_padding.right)
            .context("right padding exceeds available surface")?;
        let residual_bottom = actual_padding
            .bottom
            .checked_sub(effective_base_padding.bottom)
            .context("bottom padding exceeds available surface")?;
        let grid_rect = Rect {
            x: actual_padding.left,
            y: actual_padding.top,
            width: grid_width,
            height: grid_height,
        };
        Ok(Self {
            surface,
            cell,
            requested_padding,
            effective_base_padding,
            actual_padding,
            columns,
            rows,
            grid_rect,
            visible_grid_rect: grid_rect,
            residual_right,
            residual_bottom,
            residual_policy: ResidualPolicy::TrailingEdges,
        })
    }

    pub const fn grid_rect(self) -> Rect {
        self.grid_rect
    }

    pub const fn surface_scale_120(self) -> u32 {
        self.surface.scale.get()
    }

    pub const fn logical_width(self) -> u32 {
        self.surface.logical_size.width.get()
    }

    pub const fn logical_height(self) -> u32 {
        self.surface.logical_size.height.get()
    }

    pub const fn buffer_width(self) -> u32 {
        self.surface.buffer_size.width.get()
    }

    pub const fn buffer_height(self) -> u32 {
        self.surface.buffer_size.height.get()
    }

    pub fn buffer_layout(self) -> Result<(u32, u32, i32)> {
        self.surface.buffer_layout()
    }

    pub fn cell_rect(self, column: u32, row: u32) -> Option<Rect> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        Some(Rect {
            x: self
                .grid_rect
                .x
                .checked_add(column.checked_mul(self.cell.width)?)?,
            y: self
                .grid_rect
                .y
                .checked_add(row.checked_mul(self.cell.height)?)?,
            width: self.cell.width,
            height: self.cell.height,
        })
    }

    pub fn row_rect(self, row: usize) -> Option<Rect> {
        self.cell_rect(0, u32::try_from(row).ok()?)
            .map(|cell| Rect {
                width: self.grid_rect.width,
                ..cell
            })
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite coordinates are range checked"
    )]
    pub fn cell_at_logical(self, x: f64, y: f64) -> Option<(usize, usize)> {
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return None;
        }
        let scale_120 = self.surface_scale_120();
        let bx = (x * f64::from(scale_120) / f64::from(SCALE_DENOMINATOR)).floor();
        let by = (y * f64::from(scale_120) / f64::from(SCALE_DENOMINATOR)).floor();
        if bx > f64::from(u32::MAX) || by > f64::from(u32::MAX) {
            return None;
        }
        let bx = bx as u32;
        let by = by as u32;
        let right = self.grid_rect.x.checked_add(self.grid_rect.width)?;
        let bottom = self.grid_rect.y.checked_add(self.grid_rect.height)?;
        if bx < self.grid_rect.x || bx >= right || by < self.grid_rect.y || by >= bottom {
            return None;
        }
        let column = (bx - self.grid_rect.x) / self.cell.width;
        let row = (by - self.grid_rect.y) / self.cell.height;
        Some((row as usize, column as usize))
    }

    pub fn logical_cell_rect(self, column: u32, row: u32) -> Option<LogicalRect> {
        buffer_rect_to_logical_enclosing(self.cell_rect(column, row)?, self.surface_scale_120())
            .ok()
    }

    pub fn terminal_pixels(self) -> Result<(u16, u16)> {
        Ok((
            u16::try_from(self.grid_rect.width).context("terminal pixel width fits u16")?,
            u16::try_from(self.grid_rect.height).context("terminal pixel height fits u16")?,
        ))
    }
}

pub fn validate_scale(scale_120: u32) -> Result<()> {
    if !(MIN_SCALE_120..=MAX_SCALE_120).contains(&scale_120) {
        bail!("surface scale must be between 120 and 960");
    }
    Ok(())
}

pub fn logical_extent_to_buffer(logical: u32, scale_120: u32) -> Result<u32> {
    validate_scale(scale_120)?;
    let value = u64::from(logical) * u64::from(scale_120);
    u32::try_from(value.div_ceil(u64::from(SCALE_DENOMINATOR))).context("scaled extent overflow")
}

pub fn logical_to_buffer_ceil(logical: u32, scale_120: u32) -> Result<u32> {
    logical_extent_to_buffer(logical, scale_120)
}

pub fn logical_padding_to_buffer(logical: u32, scale_120: u32) -> Result<u32> {
    validate_scale(scale_120)?;
    u32::try_from(u64::from(logical) * u64::from(scale_120) / u64::from(SCALE_DENOMINATOR))
        .context("scaled padding overflow")
}

pub fn buffer_to_logical_ceil(buffer: u32, scale_120: u32) -> Result<u32> {
    validate_scale(scale_120)?;
    let value = u64::from(buffer) * u64::from(SCALE_DENOMINATOR);
    u32::try_from(value.div_ceil(u64::from(scale_120))).context("logical extent overflow")
}

pub fn buffer_rect_to_logical_enclosing(rect: Rect, scale_120: u32) -> Result<LogicalRect> {
    validate_scale(scale_120)?;
    let right = rect
        .x
        .checked_add(rect.width)
        .context("rectangle right overflow")?;
    let bottom = rect
        .y
        .checked_add(rect.height)
        .context("rectangle bottom overflow")?;
    let left = u64::from(rect.x) * u64::from(SCALE_DENOMINATOR) / u64::from(scale_120);
    let top = u64::from(rect.y) * u64::from(SCALE_DENOMINATOR) / u64::from(scale_120);
    let right = (u64::from(right) * u64::from(SCALE_DENOMINATOR)).div_ceil(u64::from(scale_120));
    let bottom = (u64::from(bottom) * u64::from(SCALE_DENOMINATOR)).div_ceil(u64::from(scale_120));
    let left = i32::try_from(left).context("logical rectangle left fits i32")?;
    let top = i32::try_from(top).context("logical rectangle top fits i32")?;
    let right = i32::try_from(right).context("logical rectangle right fits i32")?;
    let bottom = i32::try_from(bottom).context("logical rectangle bottom fits i32")?;
    Ok(LogicalRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn checked_mul(left: u32, right: u32, message: &'static str) -> Result<u32> {
    left.checked_mul(right).context(message)
}

fn checked_sum3(first: u32, second: u32, third: u32, message: &'static str) -> Result<u32> {
    first
        .checked_add(second)
        .and_then(|value| value.checked_add(third))
        .context(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> CellGeometry {
        CellGeometry::from_metrics(9, 17, 12, 5, 12).unwrap()
    }

    #[test]
    fn scale_bounds_rounding_and_overflow_are_checked() {
        for invalid in [0, 119, 961, u32::MAX] {
            assert!(validate_scale(invalid).is_err());
        }
        for scale in [120, 150, 180, 240, 960] {
            assert!(validate_scale(scale).is_ok());
        }
        assert!(logical_extent_to_buffer(u32::MAX, 960).is_err());
        for (scale, extent, padding) in [(120, 1, 1), (150, 2, 1), (180, 2, 1), (240, 2, 2)] {
            assert_eq!(logical_extent_to_buffer(1, scale).unwrap(), extent);
            assert_eq!(logical_padding_to_buffer(1, scale).unwrap(), padding);
        }
    }

    #[test]
    fn rectangle_conversion_transforms_endpoints() {
        let rect = buffer_rect_to_logical_enclosing(
            Rect {
                x: 1,
                y: 2,
                width: 1,
                height: 3,
            },
            180,
        )
        .unwrap();
        assert_eq!(
            rect,
            LogicalRect {
                x: 0,
                y: 1,
                width: 2,
                height: 3
            }
        );
        assert_ne!(
            rect.width,
            i32::try_from(buffer_to_logical_ceil(1, 180).unwrap()).unwrap()
        );
    }

    #[test]
    fn oversized_padding_clamps_and_surface_too_small_is_targeted() {
        let geometry = WindowGeometry::fit_window(
            40,
            40,
            cell(),
            TerminalPadding::uniform(10_000),
            120,
            2,
            240,
            2,
            80,
        )
        .unwrap();
        assert_eq!((geometry.columns, geometry.rows), (2, 2));
        assert_eq!(
            geometry.effective_base_padding,
            BufferPadding {
                left: 11,
                right: 11,
                top: 3,
                bottom: 3
            }
        );
        assert!(
            WindowGeometry::fit_window(
                17,
                33,
                cell(),
                TerminalPadding::uniform(0),
                120,
                2,
                240,
                2,
                80
            )
            .unwrap_err()
            .to_string()
            .contains("SurfaceTooSmall")
        );
    }

    #[test]
    fn asymmetric_padding_residuals_and_sums_are_exact() {
        for padding in [
            TerminalPadding::uniform(0),
            TerminalPadding {
                left: 1,
                right: 2,
                top: 3,
                bottom: 4,
            },
            TerminalPadding::uniform(10_000),
        ] {
            for scale in [120, 150, 180, 240] {
                let geometry =
                    WindowGeometry::fit_window(101, 77, cell(), padding, scale, 2, 240, 2, 80)
                        .unwrap();
                let grid = geometry.grid_rect;
                assert!(grid.x + grid.width <= geometry.buffer_width());
                assert!(grid.y + grid.height <= geometry.buffer_height());
                assert_eq!(
                    geometry.actual_padding.left + grid.width + geometry.actual_padding.right,
                    geometry.buffer_width()
                );
                assert_eq!(
                    geometry.actual_padding.top + grid.height + geometry.actual_padding.bottom,
                    geometry.buffer_height()
                );
                assert_eq!(
                    geometry.actual_padding.left,
                    geometry.effective_base_padding.left
                );
                assert_eq!(
                    geometry.actual_padding.top,
                    geometry.effective_base_padding.top
                );
                assert_eq!(
                    geometry.actual_padding.right,
                    geometry.effective_base_padding.right + geometry.residual_right
                );
                assert_eq!(
                    geometry.actual_padding.bottom,
                    geometry.effective_base_padding.bottom + geometry.residual_bottom
                );
                assert_eq!(
                    geometry.terminal_pixels().unwrap(),
                    (
                        u16::try_from(grid.width).unwrap(),
                        u16::try_from(grid.height).unwrap()
                    )
                );
            }
        }
    }

    #[test]
    fn initial_grid_round_trips_without_minimum_window_floor() {
        for columns in [2, 80, 240] {
            for scale in [120, 150, 180, 240] {
                let initial = WindowGeometry::for_grid(
                    columns,
                    24,
                    cell(),
                    TerminalPadding {
                        left: 3,
                        right: 5,
                        top: 7,
                        bottom: 9,
                    },
                    scale,
                )
                .unwrap();
                let fitted = WindowGeometry::fit_window(
                    initial.logical_width(),
                    initial.logical_height(),
                    cell(),
                    initial.requested_padding,
                    scale,
                    2,
                    240,
                    2,
                    80,
                )
                .unwrap();
                assert_eq!((fitted.columns, fitted.rows), (columns, 24));
                assert_eq!(
                    fitted.grid_rect.x + columns * cell().width,
                    fitted.grid_rect.x + fitted.grid_rect.width
                );
            }
        }
    }

    #[test]
    fn hit_testing_is_half_open_at_every_grid_and_cell_boundary() {
        let geometry = WindowGeometry::for_grid(
            240,
            80,
            cell(),
            TerminalPadding {
                left: 3,
                right: 5,
                top: 7,
                bottom: 9,
            },
            120,
        )
        .unwrap();
        assert_eq!(geometry.cell_at_logical(3.0, 7.0), Some((0, 0)));
        assert_eq!(geometry.cell_at_logical(12.0, 7.0), Some((0, 1)));
        assert_eq!(
            geometry.cell_at_logical(
                f64::from(geometry.grid_rect.x + geometry.grid_rect.width),
                7.0
            ),
            None
        );
        assert_eq!(
            geometry.cell_at_logical(
                3.0,
                f64::from(geometry.grid_rect.y + geometry.grid_rect.height)
            ),
            None
        );
        assert_eq!(geometry.cell_at_logical(f64::NAN, 0.0), None);
        assert_eq!(geometry.cell_at_logical(0.0, f64::INFINITY), None);
        assert_eq!(
            geometry.cell_rect(239, 79).unwrap().x + cell().width,
            geometry.grid_rect.x + 240 * cell().width
        );
    }

    #[test]
    fn font_policy_table_and_metadata_are_exact() {
        let cases = [
            (
                FontSize::Pixels(12.0),
                FontSizingPolicy::OutputScale,
                180,
                144.0,
                18.0,
                Some(144.0),
            ),
            (
                FontSize::Pixels(12.0),
                FontSizingPolicy::PhysicalDpi,
                180,
                144.0,
                12.0,
                None,
            ),
            (
                FontSize::Points(12.0),
                FontSizingPolicy::PhysicalDpi,
                120,
                96.0,
                16.0,
                Some(96.0),
            ),
            (
                FontSize::Points(12.0),
                FontSizingPolicy::PhysicalDpi,
                120,
                144.0,
                24.0,
                Some(144.0),
            ),
            (
                FontSize::Points(12.0),
                FontSizingPolicy::OutputScale,
                180,
                96.0,
                24.0,
                Some(144.0),
            ),
        ];
        for (size, policy, scale, dpi, pixels, sizing_dpi) in cases {
            let resolved = resolve_font_size(size, policy, scale, dpi).unwrap();
            assert_eq!(resolved.pixel_size, pixels);
            assert_eq!(resolved.effective_pixel_size_26_6, (pixels * 64.0) as u32);
            assert_eq!(resolved.sizing_dpi, sizing_dpi);
            assert_eq!(resolved.surface_scale_120, scale);
        }
        assert!(
            resolve_font_size(
                FontSize::Pixels(f32::NAN),
                FontSizingPolicy::OutputScale,
                120,
                96.0
            )
            .is_err()
        );
        assert!(
            resolve_font_size(
                FontSize::Points(96.0),
                FontSizingPolicy::PhysicalDpi,
                120,
                1_000.0
            )
            .is_err()
        );
    }

    #[test]
    fn font_resolution_preserves_fractional_pixels_at_free_type_26_6() {
        let low = resolve_font_size(
            FontSize::Pixels(12.49),
            FontSizingPolicy::PhysicalDpi,
            120,
            96.0,
        )
        .unwrap();
        let high = resolve_font_size(
            FontSize::Pixels(12.5),
            FontSizingPolicy::PhysicalDpi,
            120,
            96.0,
        )
        .unwrap();
        assert_eq!(
            (low.pixel_size, low.effective_pixel_size_26_6),
            (799.0 / 64.0, 799)
        );
        assert_eq!(
            (high.pixel_size, high.effective_pixel_size_26_6),
            (12.5, 800)
        );
    }

    #[test]
    fn wayland_output_dpi_has_identity_and_targeted_fallbacks() {
        let observed = OutputDpiObservation::from_wayland(
            7,
            Some("DP-2".into()),
            Some((2560, 1440)),
            (597, 336),
        );
        assert_eq!(observed.output_id, Some(7));
        assert_eq!(observed.output_name.as_deref(), Some("DP-2"));
        assert_eq!(observed.source, "wayland-mode-and-physical-size");
        assert!(observed.fallback_reason.is_none());
        assert!((observed.dpi - 108.9).abs() < 0.5);

        for (mode, millimeters, reason) in [
            (None, (597, 336), "missing-current-mode"),
            (Some((0, 1440)), (597, 336), "invalid-current-mode"),
            (Some((2560, 1440)), (0, 0), "missing-physical-size"),
            (Some((32_768, 32_768)), (1, 1), "unreasonable-physical-dpi"),
        ] {
            let fallback =
                OutputDpiObservation::from_wayland(9, Some("virtual".into()), mode, millimeters);
            assert_eq!(fallback.dpi, DEFAULT_DPI);
            assert_eq!(fallback.source, "fallback-96-dpi");
            assert_eq!(fallback.fallback_reason, Some(reason));
        }
    }

    #[test]
    fn same_surface_scale_changed_dpi_changes_only_physical_point_fonts() {
        let low = OutputDpiObservation::from_wayland(
            1,
            Some("low-dpi".into()),
            Some((1920, 1080)),
            (508, 285),
        );
        let high = OutputDpiObservation::from_wayland(
            2,
            Some("high-dpi".into()),
            Some((3840, 2160)),
            (508, 285),
        );
        let low_points = resolve_font_size_with_output(
            FontSize::Points(12.0),
            FontSizingPolicy::PhysicalDpi,
            120,
            &low,
        )
        .unwrap();
        let high_points = resolve_font_size_with_output(
            FontSize::Points(12.0),
            FontSizingPolicy::PhysicalDpi,
            120,
            &high,
        )
        .unwrap();
        assert_ne!(
            low_points.effective_pixel_size_26_6,
            high_points.effective_pixel_size_26_6
        );
        let low_pixels = resolve_font_size_with_output(
            FontSize::Pixels(12.0),
            FontSizingPolicy::PhysicalDpi,
            120,
            &low,
        )
        .unwrap();
        let high_pixels = resolve_font_size_with_output(
            FontSize::Pixels(12.0),
            FontSizingPolicy::PhysicalDpi,
            120,
            &high,
        )
        .unwrap();
        assert_eq!(
            low_pixels.effective_pixel_size_26_6,
            high_pixels.effective_pixel_size_26_6
        );
    }

    #[test]
    fn initial_grid_at_max_scale_round_trips_or_rejects_unrepresentable_cells() {
        let padding = TerminalPadding {
            left: 3,
            right: 5,
            top: 7,
            bottom: 9,
        };
        let initial = WindowGeometry::for_grid(80, 24, cell(), padding, 960).unwrap();
        let fitted = WindowGeometry::fit_window(
            initial.logical_width(),
            initial.logical_height(),
            cell(),
            padding,
            960,
            2,
            240,
            2,
            80,
        )
        .unwrap();
        assert_eq!((fitted.columns, fitted.rows), (80, 24));

        let tiny = CellGeometry::new(1, 1).unwrap();
        let error = WindowGeometry::for_grid(3, 3, tiny, TerminalPadding::uniform(0), 960)
            .unwrap_err()
            .to_string();
        assert!(error.contains("UnrepresentableGrid"));
    }

    #[test]
    fn typed_surface_geometry_owns_buffer_layout() {
        let surface = SurfaceGeometry::new(801, 601, 150).unwrap();
        assert_eq!(surface.logical_size.width, LogicalPx::new(801));
        assert_eq!(surface.buffer_size.width, BufferPx::new(1_002));
        assert_eq!(surface.scale, SurfaceScale120::new(150).unwrap());
        assert_eq!(surface.buffer_layout().unwrap(), (1_002, 752, 4_008));
    }
}
