//! Process-global renderer configuration, output DPI, runtime zoom, and alpha.

use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicI32, AtomicU16, Ordering},
};

use anyhow::{Context, Result, bail};
use splinterm_freetype::{MAX_PIXEL_SIZE_26_6, MIN_PIXEL_SIZE_26_6};

use crate::geometry::{
    FontSize, FontSizingPolicy, OutputDpiObservation, TerminalPadding, resolve_font_size,
    resolve_font_size_with_output,
};

pub(super) const BASE_FONT_SIZE: f32 = 22.0;
pub(super) const PRIMARY_FONT: &str = "JetBrains Mono Nerd Font:style=Regular";

static RENDERER_OPTIONS: OnceLock<RendererOptions> = OnceLock::new();
static OUTPUT_DPI: OnceLock<Mutex<OutputDpiObservation>> = OnceLock::new();
static FONT_ZOOM_STEPS: AtomicI32 = AtomicI32::new(0);
pub(super) static BACKGROUND_ALPHA: AtomicU16 = AtomicU16::new(u16::MAX);
const FONT_ZOOM_STEP_POINTS: f32 = 0.5;

#[derive(Clone, Debug)]
pub struct RendererOptions {
    pub font: String,
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
            font_size: FontSize::Pixels(BASE_FONT_SIZE),
            font_sizing_policy: FontSizingPolicy::OutputScale,
            physical_dpi: 96.0,
            padding: TerminalPadding::DEFAULT,
            background_alpha: u16::MAX,
        }
    }
}

pub(super) fn compatible_renderer_options(
    current: &RendererOptions,
    next: &RendererOptions,
) -> bool {
    current.font == next.font
        && current.font_size == next.font_size
        && current.font_sizing_policy == next.font_sizing_policy
        && current.physical_dpi.to_bits() == next.physical_dpi.to_bits()
        && current.padding == next.padding
}

fn accept_compatible_reconfiguration(options: &RendererOptions) -> Result<()> {
    let current = RENDERER_OPTIONS
        .get()
        .context("renderer configuration disappeared during initialization")?;
    anyhow::ensure!(
        compatible_renderer_options(current, options),
        "renderer is already configured with different immutable options"
    );
    BACKGROUND_ALPHA.store(options.background_alpha, Ordering::Relaxed);
    Ok(())
}

/// Installs immutable per-process renderer configuration before a window opens.
/// Repeated compatible setup supports application-owned chooser-to-window
/// transitions without allowing font or geometry caches to mix configurations.
///
/// # Errors
/// Returns an error for an invalid size or an incompatible configuration attempt.
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
    match RENDERER_OPTIONS.set(options) {
        Ok(()) => {
            BACKGROUND_ALPHA.store(background_alpha, Ordering::Relaxed);
            Ok(())
        }
        Err(options) => accept_compatible_reconfiguration(&options),
    }
}

pub(super) fn renderer_options() -> &'static RendererOptions {
    RENDERER_OPTIONS.get_or_init(RendererOptions::default)
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

fn configured_zoom_steps() -> Result<i16> {
    i16::try_from(FONT_ZOOM_STEPS.load(Ordering::Relaxed)).context("font zoom steps fit i16")
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
    let observation = output_dpi()?;
    resolve_font_size_with_output(
        zoomed_font_size(options, configured_zoom_steps()?, &observation)?,
        options.font_sizing_policy,
        surface_scale_120,
        &observation,
    )
}

/// Applies Foot's default 0.5-point runtime zoom offset.
/// Returns true when the effective raster size changed.
///
/// # Errors
/// Returns an error if the adjusted size leaves the bounded renderer range.
pub(crate) fn set_font_zoom_steps(steps: i16, surface_scale_120: u32) -> Result<Option<bool>> {
    let previous = effective_font_resolution(surface_scale_120)?;
    let options = renderer_options();
    let observation = output_dpi()?;
    let Ok(size) = zoomed_font_size(options, steps, &observation) else {
        return Ok(None);
    };
    let next = resolve_font_size_with_output(
        size,
        options.font_sizing_policy,
        surface_scale_120,
        &observation,
    )?;
    if !effective_raster_size_supported(next.effective_pixel_size_26_6)? {
        return Ok(None);
    }
    FONT_ZOOM_STEPS.store(i32::from(steps), Ordering::Relaxed);
    Ok(Some(
        previous.effective_pixel_size_26_6 != next.effective_pixel_size_26_6,
    ))
}

pub(super) fn effective_font_size(surface_scale_120: u32) -> Result<f32> {
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
        zoomed_font_size(options, configured_zoom_steps()?, &observation)?,
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
    Ok(changed)
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
}
