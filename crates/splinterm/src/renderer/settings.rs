//! Immutable renderer resources, explicit per-window state, and compatibility adapters.

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use splinterm_freetype::{MAX_PIXEL_SIZE_26_6, MIN_PIXEL_SIZE_26_6};

use super::{FontGeneration, clear_snapshot_caches, snapshot_font_generation};

use crate::{
    config::{FontAuthority, STARTUP_FONT_FALLBACK},
    geometry::{
        FontSize, FontSizingPolicy, OutputDpiObservation, TerminalPadding, resolve_font_size,
        resolve_font_size_with_output,
    },
};

pub(super) const BASE_FONT_SIZE: f32 = 22.0;
pub(super) const PRIMARY_FONT: &str = STARTUP_FONT_FALLBACK;
const FONT_ZOOM_STEP_POINTS: f32 = 0.5;

/// Immutable process resources shared by renderer contexts.
#[derive(Debug)]
pub struct RendererResources {
    options: RendererOptions,
}

static RENDERER_RESOURCES: OnceLock<RendererResources> = OnceLock::new();
static COMPATIBILITY_CONTEXT: OnceLock<Mutex<RenderContext>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct RendererOptions {
    pub font: String,
    pub font_authority: FontAuthority,
    pub font_size: FontSize,
    pub font_sizing_policy: FontSizingPolicy,
    pub physical_dpi: f32,
    pub padding: TerminalPadding,
    pub background_alpha: u16,
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            font: PRIMARY_FONT.to_owned(),
            font_authority: FontAuthority::Explicit,
            font_size: FontSize::Pixels(BASE_FONT_SIZE),
            font_sizing_policy: FontSizingPolicy::OutputScale,
            physical_dpi: 96.0,
            padding: TerminalPadding::DEFAULT,
            background_alpha: u16::MAX,
        }
    }
}

/// Mutable renderer settings owned by one graphical window.
#[derive(Clone, Debug)]
pub struct RenderContext {
    resources: &'static RendererResources,
    font_generation: Result<Arc<FontGeneration>, Arc<String>>,
    output_dpi: OutputDpiObservation,
    font_zoom_steps: i16,
    background_alpha: u16,
}

impl RenderContext {
    /// Creates independent mutable state over the configured immutable resources.
    ///
    /// # Panics
    /// Panics only if the already-validated configured physical DPI becomes invalid.
    #[must_use]
    pub fn new(background_alpha: u16) -> Self {
        let resources = renderer_resources();
        let font_generation = snapshot_font_generation()
            .map(Arc::clone)
            .map_err(|error| Arc::new(format!("{error:#}")));
        Self {
            resources,
            font_generation,
            output_dpi: OutputDpiObservation::provided(resources.options.physical_dpi)
                .expect("configured physical DPI was validated"),
            font_zoom_steps: 0,
            background_alpha,
        }
    }

    pub(super) fn font_generation(&self) -> Result<&Arc<FontGeneration>> {
        self.font_generation
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    #[must_use]
    #[allow(dead_code, reason = "used by the next Plan 0038 delivery milestone")]
    pub(crate) fn font_generation_id(&self) -> Option<u64> {
        self.font_generation
            .as_ref()
            .ok()
            .map(|generation| generation.id)
    }

    #[allow(dead_code, reason = "used by the next Plan 0038 delivery milestone")]
    pub(crate) fn replace_font_generation(&mut self, next: Arc<FontGeneration>) -> bool {
        if self
            .font_generation
            .as_ref()
            .is_ok_and(|current| current.fingerprint == next.fingerprint)
        {
            return false;
        }
        self.font_generation = Ok(next);
        clear_snapshot_caches();
        true
    }

    #[must_use]
    pub(super) fn padding(&self) -> TerminalPadding {
        self.resources.options.padding
    }

    #[must_use]
    pub const fn background_alpha(&self) -> u16 {
        self.background_alpha
    }

    pub(crate) const fn set_background_alpha(&mut self, alpha: u16) {
        self.background_alpha = alpha;
    }

    /// Resolves this context's font against surface scale and output DPI.
    ///
    /// # Errors
    /// Returns an error for invalid scale, DPI, zoom, or effective raster size.
    pub fn effective_font_resolution(
        &self,
        surface_scale_120: u32,
    ) -> Result<crate::geometry::ResolvedFontSize> {
        let options = &self.resources.options;
        resolve_font_size_with_output(
            zoomed_font_size(options, self.font_zoom_steps, &self.output_dpi)?,
            options.font_sizing_policy,
            surface_scale_120,
            &self.output_dpi,
        )
    }

    pub(super) fn effective_font_size(&self, surface_scale_120: u32) -> Result<f32> {
        Ok(self
            .effective_font_resolution(surface_scale_120)?
            .pixel_size)
    }

    /// Applies Foot's default 0.5-point runtime zoom offset.
    pub(super) fn apply_font_zoom_steps(
        &mut self,
        steps: i16,
        surface_scale_120: u32,
    ) -> Result<Option<bool>> {
        let previous = self.effective_font_resolution(surface_scale_120)?;
        let options = &self.resources.options;
        let Ok(size) = zoomed_font_size(options, steps, &self.output_dpi) else {
            return Ok(None);
        };
        let next = resolve_font_size_with_output(
            size,
            options.font_sizing_policy,
            surface_scale_120,
            &self.output_dpi,
        )?;
        if !effective_raster_size_supported(next.effective_pixel_size_26_6)? {
            return Ok(None);
        }
        self.font_zoom_steps = steps;
        Ok(Some(
            previous.effective_pixel_size_26_6 != next.effective_pixel_size_26_6,
        ))
    }

    /// Updates this context's current output observation.
    pub(super) fn apply_output_dpi(
        &mut self,
        observation: OutputDpiObservation,
        surface_scale_120: u32,
    ) -> Result<bool> {
        let options = &self.resources.options;
        let previous = self.effective_font_resolution(surface_scale_120)?;
        let next = resolve_font_size_with_output(
            zoomed_font_size(options, self.font_zoom_steps, &observation)?,
            options.font_sizing_policy,
            surface_scale_120,
            &observation,
        )?;
        self.output_dpi = observation;
        Ok(previous.effective_pixel_size_26_6 != next.effective_pixel_size_26_6)
    }
}

pub(super) fn compatible_renderer_options(
    current: &RendererOptions,
    next: &RendererOptions,
) -> bool {
    current.font == next.font
        && current.font_authority == next.font_authority
        && current.font_size == next.font_size
        && current.font_sizing_policy == next.font_sizing_policy
        && current.physical_dpi.to_bits() == next.physical_dpi.to_bits()
        && current.padding == next.padding
}

fn compatibility_context() -> Result<std::sync::MutexGuard<'static, RenderContext>> {
    COMPATIBILITY_CONTEXT
        .get_or_init(|| Mutex::new(RenderContext::new(renderer_options().background_alpha)))
        .lock()
        .map_err(|_| anyhow::anyhow!("renderer compatibility context lock is poisoned"))
}

/// Installs immutable per-process resources and updates only the legacy compatibility context.
/// Explicit window contexts remain isolated from compatible reconfiguration.
///
/// # Errors
/// Returns an error for invalid sizing or incompatible immutable reconfiguration.
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
    let background_alpha = options.background_alpha;
    match RENDERER_RESOURCES.set(RendererResources { options }) {
        Ok(()) => {}
        Err(resources) => {
            anyhow::ensure!(
                compatible_renderer_options(&renderer_resources().options, &resources.options),
                "renderer is already configured with different immutable options"
            );
        }
    }
    compatibility_context()?.set_background_alpha(background_alpha);
    Ok(())
}

pub(super) fn renderer_resources() -> &'static RendererResources {
    RENDERER_RESOURCES.get_or_init(|| RendererResources {
        options: RendererOptions::default(),
    })
}

pub(super) fn renderer_options() -> &'static RendererOptions {
    &renderer_resources().options
}

pub(super) fn zoomed_font_size(
    options: &RendererOptions,
    steps: i16,
    observation: &OutputDpiObservation,
) -> Result<FontSize> {
    if steps == 0 {
        return Ok(options.font_size);
    }
    let sizing_dpi = match options.font_sizing_policy {
        FontSizingPolicy::OutputScale => 96.0,
        FontSizingPolicy::PhysicalDpi => observation.dpi,
    };
    let base_points = match options.font_size {
        FontSize::Points(points) => points,
        FontSize::Pixels(pixels) => pixels * 72.0 / sizing_dpi,
    };
    let points = base_points + f32::from(steps) * FONT_ZOOM_STEP_POINTS;
    if !points.is_finite() || !(6.0..=96.0).contains(&points) {
        bail!("runtime font size must remain between 6 and 96 points");
    }
    Ok(FontSize::Points(points))
}

pub(super) fn effective_raster_size_supported(pixel_size_26_6: u32) -> Result<bool> {
    let pixels = isize::try_from(pixel_size_26_6).context("effective pixel size fits isize")?;
    Ok((MIN_PIXEL_SIZE_26_6..=MAX_PIXEL_SIZE_26_6).contains(&pixels))
}

/// Resolves the legacy compatibility context's effective font size.
///
/// # Errors
/// Returns an error for invalid scale, DPI, zoom, or poisoned compatibility state.
pub fn effective_font_resolution(
    surface_scale_120: u32,
) -> Result<crate::geometry::ResolvedFontSize> {
    compatibility_context()?.effective_font_resolution(surface_scale_120)
}

pub(super) fn effective_font_size(surface_scale_120: u32) -> Result<f32> {
    Ok(effective_font_resolution(surface_scale_120)?.pixel_size)
}

pub fn update_output_dpi(
    observation: OutputDpiObservation,
    surface_scale_120: u32,
) -> Result<bool> {
    compatibility_context()?.apply_output_dpi(observation, surface_scale_120)
}

pub(super) fn compatibility_render_context() -> Result<RenderContext> {
    Ok(compatibility_context()?.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    use splinterm_freetype::MAX_PIXEL_SIZE_26_6;

    #[test]
    fn compatible_renderer_reconfiguration_allows_only_mutable_alpha_changes() {
        let current = RendererOptions::default();
        let mut alpha_only = current.clone();
        alpha_only.background_alpha = 32_768;
        assert!(compatible_renderer_options(&current, &alpha_only));

        let mut different_font = current.clone();
        different_font.font = "different font".to_owned();
        assert!(!compatible_renderer_options(&current, &different_font));

        let mut different_authority = current.clone();
        different_authority.font_authority = FontAuthority::NativeOmarchy;
        assert!(!compatible_renderer_options(&current, &different_authority));

        let mut different_padding = current.clone();
        different_padding.padding.left += 1;
        assert!(!compatible_renderer_options(&current, &different_padding));
    }

    #[test]
    fn foot_runtime_zoom_uses_half_points_and_converts_pixel_bases() {
        let observation = OutputDpiObservation::provided(144.0).unwrap();
        let mut options = RendererOptions {
            font_size: FontSize::Points(10.3),
            ..RendererOptions::default()
        };
        assert_eq!(
            zoomed_font_size(&options, 1, &observation).unwrap(),
            FontSize::Points(10.8)
        );
        assert_eq!(
            zoomed_font_size(&options, 0, &observation).unwrap(),
            FontSize::Points(10.3)
        );

        options.font_size = FontSize::Pixels(22.0);
        assert_eq!(
            zoomed_font_size(&options, 1, &observation).unwrap(),
            FontSize::Points(17.0)
        );
        options.font_sizing_policy = FontSizingPolicy::PhysicalDpi;
        assert_eq!(
            zoomed_font_size(&options, 1, &observation).unwrap(),
            FontSize::Points(11.5)
        );
        assert_eq!(
            zoomed_font_size(&options, -10, &observation).unwrap(),
            FontSize::Points(6.0)
        );
        assert!(zoomed_font_size(&options, -11, &observation).is_err());
        assert!(!effective_raster_size_supported(6 * 64 - 1).unwrap());
        assert!(effective_raster_size_supported(6 * 64).unwrap());
        let maximum = u32::try_from(MAX_PIXEL_SIZE_26_6).unwrap();
        assert!(effective_raster_size_supported(maximum).unwrap());
        assert!(!effective_raster_size_supported(maximum + 1).unwrap());
    }

    #[test]
    fn render_contexts_keep_dpi_zoom_and_alpha_isolated() {
        let first = RenderContext::new(10_000);
        let mut second = RenderContext::new(50_000);
        let first_before = first.effective_font_resolution(120).unwrap();
        second
            .apply_output_dpi(OutputDpiObservation::provided(192.0).unwrap(), 120)
            .unwrap();
        second.apply_font_zoom_steps(2, 120).unwrap();
        assert_eq!(first.background_alpha(), 10_000);
        assert_eq!(second.background_alpha(), 50_000);
        assert_eq!(first.effective_font_resolution(120).unwrap(), first_before);
        assert_ne!(
            first.effective_font_resolution(120).unwrap(),
            second.effective_font_resolution(120).unwrap()
        );
    }
}
