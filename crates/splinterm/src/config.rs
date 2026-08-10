//! Project-owned MVP configuration and native Omarchy palette integration.
//!
//! This parser deliberately accepts only the documented Foot-compatible subset.
//! Unknown sections and keys are diagnostics, never silently accepted compatibility.

#![allow(
    clippy::too_many_lines,
    clippy::unreadable_literal,
    reason = "configuration tables remain more auditable when values stay adjacent to their keys"
)]

use std::{
    collections::HashMap,
    env, fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    geometry::{FontSize, FontSizingPolicy, TerminalPadding},
    keymap::{KeymapProfile, ResolvedKeymap, resolve_keymap},
    preset::PresetCatalog,
};

pub const APP_ID: &str = "com.oldjobobo.splinterm";
pub const DEFAULT_FONT: &str = "JetBrains Mono Nerd Font:style=Regular";

#[derive(Clone, Debug, PartialEq)]
pub struct AppConfig {
    pub font: String,
    pub font_size: FontSize,
    pub font_sizing_policy: FontSizingPolicy,
    pub padding: TerminalPadding,
    pub initial_columns: u16,
    pub initial_rows: u16,
    pub shell: Option<String>,
    pub login_shell: bool,
    pub title: Option<String>,
    pub scrollback_lines: usize,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub resize_delay_ms: u64,
    /// Optional Foot-compatible background-alpha override (`[colors] alpha`).
    /// When absent, the active theme owns background translucency.
    pub background_alpha: Option<u16>,
    /// Optional native background-blur override (`[colors] blur`).
    /// When absent, the active theme owns the requested blur state.
    pub background_blur: Option<bool>,
    /// Explicit project JSON override. When absent, Splinterm follows the
    /// active Omarchy theme directly.
    pub theme_path: Option<PathBuf>,
    pub pane_divider_style: PaneDividerStyle,
    pub frame_title_mode: FrameTitleMode,
    /// Effective, fully validated client-local keymap.
    pub keymap: ResolvedKeymap,
    pub keymap_profile: KeymapProfile,
    pub keymap_path: Option<PathBuf>,
    pub prefix_timeout_ms: u64,
    /// Explicit client-local static preset catalog, resolved relative to config.ini.
    pub preset_path: Option<PathBuf>,
    pub preset_catalog: Option<PresetCatalog>,
    pub allow_unrestricted_commands: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorStyle {
    #[default]
    Block,
    Beam,
    Underline,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PaneDividerStyle {
    None,
    #[default]
    Line,
    Frame,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrameTitleMode {
    None,
    #[default]
    Splint,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            font: DEFAULT_FONT.to_owned(),
            font_size: FontSize::Pixels(14.0),
            font_sizing_policy: FontSizingPolicy::OutputScale,
            padding: TerminalPadding::DEFAULT,
            initial_columns: 80,
            initial_rows: 24,
            shell: None,
            login_shell: true,
            title: None,
            scrollback_lines: 1_000,
            cursor_style: CursorStyle::Block,
            cursor_blink: true,
            resize_delay_ms: 100,
            background_alpha: None,
            background_blur: None,
            theme_path: None,
            pane_divider_style: PaneDividerStyle::Line,
            frame_title_mode: FrameTitleMode::Splint,
            keymap: ResolvedKeymap::default(),
            keymap_profile: KeymapProfile::Splinterm,
            keymap_path: None,
            prefix_timeout_ms: 1_000,
            preset_path: None,
            preset_catalog: None,
            allow_unrestricted_commands: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigLoad {
    pub config: AppConfig,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeSource {
    Omarchy(PathBuf),
    Json(PathBuf),
}

impl AppConfig {
    #[must_use]
    pub fn theme_source(&self) -> ThemeSource {
        self.theme_path.clone().map_or_else(
            || ThemeSource::Omarchy(default_omarchy_theme_dir()),
            ThemeSource::Json,
        )
    }
}

pub fn default_config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map_or_else(
            || {
                env::var_os("HOME")
                    .map_or_else(|| PathBuf::from("."), PathBuf::from)
                    .join(".config")
            },
            PathBuf::from,
        )
        .join("splinterm")
}

pub fn default_omarchy_theme_dir() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map_or_else(
            || {
                env::var_os("HOME")
                    .map_or_else(|| PathBuf::from("."), PathBuf::from)
                    .join(".local/state")
            },
            PathBuf::from,
        )
        .join("omarchy/current/theme")
}

/// Loads the supported configuration from the standard or overridden path.
///
/// # Errors
/// Returns an error when the file cannot be read or a supported value is invalid.
pub fn load_default() -> Result<ConfigLoad> {
    let path = env::var_os("SPLINTERM_CONFIG")
        .map_or_else(|| default_config_dir().join("config.ini"), PathBuf::from);
    if !path.exists() {
        return Ok(ConfigLoad {
            config: AppConfig::default(),
            diagnostics: Vec::new(),
        });
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    parse_with_base(&text, path.parent().unwrap_or_else(|| Path::new(".")))
        .with_context(|| format!("parse {}", path.display()))
}

/// Parses the documented MVP configuration subset.
///
/// # Errors
/// Returns an error for malformed syntax or invalid supported values.
pub fn parse(text: &str) -> Result<ConfigLoad> {
    parse_with_base(text, &default_config_dir())
}

fn parse_with_base(text: &str, config_dir: &Path) -> Result<ConfigLoad> {
    let mut config = AppConfig::default();
    let mut diagnostics = Vec::new();
    let mut explicit_font_policy_line = None;
    let mut legacy_dpi_aware_line = None;
    let mut explicit_font_size_line = None;
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            if !matches!(
                section.as_str(),
                "main"
                    | "scrollback"
                    | "cursor"
                    | "colors"
                    | "key-bindings"
                    | "presets"
                    | "multiplexer"
            ) {
                diagnostics.push(format!(
                    "line {}: unsupported section [{section}]",
                    index + 1
                ));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("line {}: expected key=value", index + 1);
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let full = if section.is_empty() {
            key.clone()
        } else {
            format!("{section}.{key}")
        };
        let unsupported = match full.as_str() {
            "main.font" | "font" => {
                let font = nonempty(value, index)?;
                let normalized = font.to_ascii_lowercase();
                if normalized.contains(":size=") || normalized.contains(":pixelsize=") {
                    bail!(
                        "line {}: main.font is only a face/style pattern; use main.font-pixelsize or main.font-point-size",
                        index + 1
                    );
                }
                config.font = font;
                false
            }
            "main.font-size" | "font-size" | "main.font-pixelsize" => {
                if let Some(previous) = explicit_font_size_line {
                    bail!(
                        "line {}: font size conflicts with line {previous}; configure exactly one size key",
                        index + 1
                    );
                }
                config.font_size = FontSize::Pixels(parse_range(value, 6.0, 96.0, index)?);
                explicit_font_size_line = Some(index + 1);
                if full != "main.font-pixelsize" {
                    diagnostics.push(format!(
                        "line {}: font-size is deprecated; use main.font-pixelsize",
                        index + 1
                    ));
                }
                false
            }
            "main.font-point-size" => {
                if let Some(previous) = explicit_font_size_line {
                    bail!(
                        "line {}: font size conflicts with line {previous}; configure exactly one size key",
                        index + 1
                    );
                }
                config.font_size = FontSize::Points(parse_range(value, 6.0, 96.0, index)?);
                explicit_font_size_line = Some(index + 1);
                false
            }
            "main.font-sizing-policy" => {
                config.font_sizing_policy = match value.to_ascii_lowercase().as_str() {
                    "output-scale" => FontSizingPolicy::OutputScale,
                    "physical-dpi" => FontSizingPolicy::PhysicalDpi,
                    _ => bail!(
                        "line {}: font-sizing-policy must be output-scale or physical-dpi",
                        index + 1
                    ),
                };
                explicit_font_policy_line = Some(index + 1);
                false
            }
            "main.padding-left" => {
                config.padding.left = parse_range(value, 0, 10_000, index)?;
                false
            }
            "main.padding-right" => {
                config.padding.right = parse_range(value, 0, 10_000, index)?;
                false
            }
            "main.padding-top" => {
                config.padding.top = parse_range(value, 0, 10_000, index)?;
                false
            }
            "main.padding-bottom" => {
                config.padding.bottom = parse_range(value, 0, 10_000, index)?;
                false
            }
            "main.initial-columns" => {
                config.initial_columns = parse_range(value, 2, 240, index)?;
                false
            }
            "main.initial-rows" => {
                config.initial_rows = parse_range(value, 2, 80, index)?;
                false
            }
            "main.shell" | "shell" => {
                config.shell = Some(nonempty(value, index)?);
                false
            }
            "main.login-shell" | "login-shell" => {
                config.login_shell = parse_bool(value, index)?;
                false
            }
            "main.title" | "title" => {
                config.title = Some(value.to_owned());
                false
            }
            "main.app-id" | "app-id" => {
                if value != APP_ID {
                    diagnostics.push(format!(
                        "line {}: app-id is fixed to {APP_ID}; requested value ignored",
                        index + 1
                    ));
                }
                false
            }
            "main.resize-delay-ms" => {
                config.resize_delay_ms = parse_range(value, 0, 1_000, index)?;
                false
            }
            "main.dpi-aware" => {
                let value = parse_bool(value, index)?;
                if !value {
                    bail!(
                        "line {}: legacy Splinterm dpi-aware=no forced the whole surface to 1x and cannot be migrated safely; remove it and choose main.font-sizing-policy explicitly",
                        index + 1
                    );
                }
                legacy_dpi_aware_line = Some(index + 1);
                config.font_sizing_policy = FontSizingPolicy::OutputScale;
                diagnostics.push(format!(
                    "line {}: legacy Splinterm dpi-aware=yes is deprecated and maps only to main.font-sizing-policy=output-scale",
                    index + 1
                ));
                false
            }
            "main.theme" => {
                config.theme_path = Some(expand_path(value));
                false
            }
            "colors.alpha" => {
                config.background_alpha = Some(foot_alpha(parse_alpha(value, index)?));
                false
            }
            "colors.blur" => {
                config.background_blur = Some(parse_bool(value, index)?);
                false
            }
            "scrollback.lines" => {
                config.scrollback_lines = parse_range(value, 0, 1_000_000, index)?;
                false
            }
            "multiplexer.divider-style" => {
                config.pane_divider_style = match value.to_ascii_lowercase().as_str() {
                    "none" => PaneDividerStyle::None,
                    "line" => PaneDividerStyle::Line,
                    "frame" => PaneDividerStyle::Frame,
                    _ => bail!(
                        "line {}: divider-style must be none, line, or frame",
                        index + 1
                    ),
                };
                false
            }
            "multiplexer.frame-title" => {
                config.frame_title_mode = match value.to_ascii_lowercase().as_str() {
                    "none" => FrameTitleMode::None,
                    "splint" => FrameTitleMode::Splint,
                    _ => bail!("line {}: frame-title must be none or splint", index + 1),
                };
                false
            }
            "cursor.style" => {
                config.cursor_style = match value.to_ascii_lowercase().as_str() {
                    "block" => CursorStyle::Block,
                    "beam" => CursorStyle::Beam,
                    "underline" => CursorStyle::Underline,
                    _ => bail!(
                        "line {}: cursor style must be block, beam, or underline",
                        index + 1
                    ),
                };
                false
            }
            "cursor.blink" => {
                config.cursor_blink = parse_bool(value, index)?;
                false
            }
            "key-bindings.profile" => {
                config.keymap_profile = KeymapProfile::parse(value)
                    .with_context(|| format!("line {}: invalid keymap profile", index + 1))?;
                false
            }
            "key-bindings.file" => {
                let value = nonempty(value, index)?;
                config.keymap_path = Some(expand_relative_path(&value, config_dir));
                false
            }
            "key-bindings.prefix-timeout-ms" => {
                config.prefix_timeout_ms = parse_range(value, 250, 5_000, index)?;
                false
            }
            "presets.file" => {
                let value = nonempty(value, index)?;
                config.preset_path = Some(expand_relative_path(&value, config_dir));
                false
            }
            "presets.allow-unrestricted-commands" => {
                config.allow_unrestricted_commands = parse_bool(value, index)?;
                false
            }
            key if key.starts_with("colors.") => {
                diagnostics.push(format!(
                    "line {}: colors come from the active Omarchy theme or explicit main.theme JSON; {key} ignored",
                    index + 1
                ));
                false
            }
            _ => true,
        };
        if unsupported {
            diagnostics.push(format!("line {}: unsupported option {full}", index + 1));
        }
    }
    if let (Some(legacy), Some(explicit)) = (legacy_dpi_aware_line, explicit_font_policy_line) {
        bail!(
            "line {legacy}: legacy dpi-aware conflicts with font-sizing-policy on line {explicit}; remove the legacy key"
        );
    }
    let keymap = resolve_keymap(config.keymap_profile, config.keymap_path.as_deref())?;
    config.keymap = keymap.keymap;
    diagnostics.extend(keymap.diagnostics);
    config.preset_catalog = config
        .preset_path
        .as_deref()
        .map(PresetCatalog::load)
        .transpose()?;
    Ok(ConfigLoad {
        config,
        diagnostics,
    })
}

fn nonempty(value: &str, line: usize) -> Result<String> {
    if value.is_empty() {
        bail!("line {}: value cannot be empty", line + 1)
    }
    Ok(value.to_owned())
}
fn parse_bool(value: &str, line: usize) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Ok(true),
        "no" | "false" | "off" | "0" => Ok(false),
        _ => bail!("line {}: expected boolean", line + 1),
    }
}
fn parse_range<T>(value: &str, min: T, max: T, line: usize) -> Result<T>
where
    T: std::str::FromStr + PartialOrd + Copy,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| anyhow::anyhow!("line {}: invalid number", line + 1))?;
    if parsed < min || parsed > max {
        bail!("line {}: number outside supported range", line + 1);
    }
    Ok(parsed)
}

fn parse_alpha(value: &str, line: usize) -> Result<f32> {
    let alpha = value
        .parse::<f32>()
        .map_err(|_| anyhow::anyhow!("line {}: invalid number", line + 1))?;
    if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
        bail!("line {}: number outside supported range", line + 1);
    }
    Ok(alpha)
}

fn expand_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(rest)
    } else {
        PathBuf::from(value)
    }
}

fn expand_relative_path(value: &str, base: &Path) -> PathBuf {
    let expanded = expand_path(value);
    if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePalette {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    #[serde(default)]
    pub selection_foreground: Option<String>,
    pub url: String,
    pub ui_accent: String,
    #[serde(default = "opaque_alpha")]
    pub alpha: f32,
    #[serde(default)]
    pub blur: bool,
    #[serde(default)]
    pub pane_border: Option<String>,
    #[serde(default)]
    pub pane_border_active: Option<String>,
    pub ansi: [String; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedTheme {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
    pub selection_foreground: u32,
    pub url: u32,
    pub ui_accent: u32,
    pub pane_border: u32,
    pub pane_border_active: u32,
    pub background_alpha: u16,
    pub background_blur: bool,
    pub ansi: [u32; 16],
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            background: 0x0e1216,
            foreground: 0xebebeb,
            cursor: 0xebebeb,
            selection: 0x354a60,
            selection_foreground: 0xebebeb,
            url: 0x78beff,
            ui_accent: 0x78d2ff,
            pane_border: 0x7c7e80,
            pane_border_active: 0x78d2ff,
            background_alpha: u16::MAX,
            background_blur: false,
            ansi: [
                0x1d2021, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ],
        }
    }
}

const fn opaque_alpha() -> f32 {
    1.0
}

impl ResolvedTheme {
    #[must_use]
    pub fn with_color_overrides(mut self, alpha: Option<u16>, blur: Option<bool>) -> Self {
        if let Some(alpha) = alpha {
            self.background_alpha = alpha;
        }
        if let Some(blur) = blur {
            self.background_blur = blur;
        }
        self
    }
}

impl ThemePalette {
    /// Resolves all strict `#RRGGBB` roles to packed colors.
    ///
    /// # Errors
    /// Returns an error when any required role is not a six-digit RGB color.
    pub fn resolve(&self) -> Result<ResolvedTheme> {
        let mut ansi = [0; 16];
        for (out, value) in ansi.iter_mut().zip(&self.ansi) {
            *out = parse_color(value)?;
        }
        if !self.alpha.is_finite() || !(0.0..=1.0).contains(&self.alpha) {
            bail!("theme alpha must be between 0.0 and 1.0");
        }
        let background = parse_color(&self.background)?;
        let foreground = parse_color(&self.foreground)?;
        let ui_accent = parse_color(&self.ui_accent)?;
        Ok(ResolvedTheme {
            background,
            foreground,
            cursor: parse_color(&self.cursor)?,
            selection: parse_color(&self.selection)?,
            selection_foreground: self
                .selection_foreground
                .as_deref()
                .map(parse_color)
                .transpose()?
                .unwrap_or(foreground),
            url: parse_color(&self.url)?,
            ui_accent,
            pane_border: self
                .pane_border
                .as_deref()
                .map(parse_color)
                .transpose()?
                .unwrap_or_else(|| blend_rgb(background, foreground)),
            pane_border_active: self
                .pane_border_active
                .as_deref()
                .map(parse_color)
                .transpose()?
                .unwrap_or(ui_accent),
            background_alpha: foot_alpha(self.alpha),
            background_blur: self.blur,
            ansi,
        })
    }
}

fn omarchy_color_values(raw: &str) -> HashMap<String, String> {
    let mut colors = HashMap::new();
    for line in raw.lines() {
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key: String = key
            .chars()
            .filter(|character| !matches!(character, '"' | '\'' | ' ' | '\t'))
            .collect();
        if key.is_empty() || key.starts_with('#') {
            continue;
        }
        let value = if let Some(start) = raw_value.find(['"', '\'']) {
            let quoted = &raw_value[start + 1..];
            let end = quoted.find(['"', '\'']).unwrap_or(quoted.len());
            quoted[..end].to_owned()
        } else {
            raw_value.trim().to_owned()
        };
        colors.insert(key, value);
    }
    colors
}

fn foot_color_values(raw: &str) -> HashMap<String, String> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut section = String::new();
    let mut colors_dark_seen = false;
    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            colors_dark_seen |= section == "colors-dark";
            continue;
        }
        if !matches!(section.as_str(), "colors" | "colors-dark") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        sections
            .entry(section.clone())
            .or_default()
            .insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    if colors_dark_seen {
        sections.remove("colors-dark").unwrap_or_default()
    } else {
        sections.remove("colors").unwrap_or_default()
    }
}

fn foot_theme_color(values: &HashMap<String, String>, key: &str) -> Result<u32> {
    parse_color(
        values
            .get(key)
            .with_context(|| format!("active Omarchy foot.ini is missing {key}"))?,
    )
    .with_context(|| format!("active Omarchy foot.ini has invalid {key}"))
}

fn resolve_omarchy_theme(colors_raw: &str, foot_raw: &str) -> Result<ResolvedTheme> {
    let colors = omarchy_color_values(colors_raw);
    let foot = foot_color_values(foot_raw);
    if foot.is_empty() {
        bail!("active Omarchy foot.ini has no [colors-dark] or [colors] palette");
    }

    let mut ansi = [0_u32; 16];
    for (index, output) in ansi.iter_mut().enumerate().take(8) {
        *output = foot_theme_color(&foot, &format!("regular{index}"))?;
    }
    for (index, output) in ansi.iter_mut().enumerate().skip(8) {
        *output = foot_theme_color(&foot, &format!("bright{}", index - 8))?;
    }

    let background = foot_theme_color(&foot, "background")?;
    let foreground = foot_theme_color(&foot, "foreground")?;
    let selection = foot
        .get("selection-background")
        .map(|value| parse_color(value))
        .transpose()
        .context("active Omarchy foot.ini has invalid selection-background")?
        .unwrap_or(ansi[8]);
    let selection_foreground = foot
        .get("selection-foreground")
        .map(|value| parse_color(value))
        .transpose()
        .context("active Omarchy foot.ini has invalid selection-foreground")?
        .unwrap_or(foreground);
    let cursor = foot
        .get("cursor")
        .and_then(|value| value.split_whitespace().last())
        .map(parse_color)
        .transpose()
        .context("active Omarchy foot.ini has invalid cursor")?
        .unwrap_or(foreground);
    let ui_accent = ["accent", "cursor", "color4", "blue"]
        .iter()
        .find_map(|key| colors.get(*key))
        .map(|value| parse_color(value))
        .transpose()
        .context("active Omarchy colors.toml has invalid accent")?
        .unwrap_or(cursor);
    let alpha = foot.get("alpha").map_or(Ok(1.0), |value| {
        value
            .parse::<f32>()
            .context("active Omarchy foot.ini alpha must be a number")
    })?;
    if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
        bail!("active Omarchy foot.ini alpha must be between 0.0 and 1.0");
    }
    let blur = foot.get("blur").map_or(Ok(false), |value| {
        match value.to_ascii_lowercase().as_str() {
            "yes" | "true" | "on" | "1" => Ok(true),
            "no" | "false" | "off" | "0" => Ok(false),
            _ => bail!("active Omarchy foot.ini blur must be a boolean"),
        }
    })?;

    Ok(ResolvedTheme {
        background,
        foreground,
        cursor,
        selection,
        selection_foreground,
        url: ansi[4],
        ui_accent,
        pane_border: ansi[8],
        pane_border_active: ui_accent,
        background_alpha: foot_alpha(alpha),
        background_blur: blur,
        ansi,
    })
}

type OmarchySourceIdentity = Option<(u64, u64, u64, i64, i64)>;

fn omarchy_path_identity(path: &Path) -> OmarchySourceIdentity {
    let metadata = fs::metadata(path).ok()?;
    Some((
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    ))
}

fn omarchy_theme_identity(
    theme_dir: &Path,
) -> (
    OmarchySourceIdentity,
    OmarchySourceIdentity,
    OmarchySourceIdentity,
) {
    (
        omarchy_path_identity(theme_dir),
        omarchy_path_identity(&theme_dir.join("colors.toml")),
        omarchy_path_identity(&theme_dir.join("foot.ini")),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MissingOmarchyTheme {
    UseFallback,
    Reject,
}

fn load_omarchy_theme_with(
    theme_dir: &Path,
    missing: MissingOmarchyTheme,
    after_identity: impl FnOnce(),
    between_reads: impl FnOnce(),
) -> Result<ResolvedTheme> {
    let colors_path = theme_dir.join("colors.toml");
    let foot_path = theme_dir.join("foot.ini");
    let before = omarchy_theme_identity(theme_dir);
    if missing == MissingOmarchyTheme::UseFallback
        && before.0.is_none()
        && before.1.is_none()
        && before.2.is_none()
    {
        return Ok(ResolvedTheme::default());
    }
    after_identity();
    let colors = fs::read_to_string(&colors_path)
        .with_context(|| format!("read {}", colors_path.display()))?;
    between_reads();
    let foot =
        fs::read_to_string(&foot_path).with_context(|| format!("read {}", foot_path.display()))?;
    anyhow::ensure!(
        omarchy_theme_identity(theme_dir) == before,
        "active Omarchy theme changed while its palette was loading"
    );
    resolve_omarchy_theme(&colors, &foot)
}

/// Loads the effective active Omarchy terminal palette directly.
///
/// # Errors
/// Returns an error when an active theme exists but its `colors.toml` or
/// generated `foot.ini` cannot be read as one coherent theme generation.
pub fn load_omarchy_theme(theme_dir: &Path) -> Result<ResolvedTheme> {
    load_omarchy_theme_with(theme_dir, MissingOmarchyTheme::UseFallback, || {}, || {})
}

fn load_live_omarchy_theme(theme_dir: &Path) -> Result<ResolvedTheme> {
    load_omarchy_theme_with(theme_dir, MissingOmarchyTheme::Reject, || {}, || {})
}

/// Loads an explicit project JSON role map, or the safe fallback when absent.
///
/// # Errors
/// Returns an error when an existing theme file is unreadable or invalid.
pub fn load_theme(path: &Path) -> Result<ResolvedTheme> {
    if !path.exists() {
        return Ok(ResolvedTheme::default());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str::<ThemePalette>(&raw)
        .context("parse explicit Splinterm theme JSON")?
        .resolve()
}

/// Resolves either native Omarchy state or an explicit JSON override.
///
/// # Errors
/// Returns an error when the selected source exists but is malformed.
pub fn load_theme_source(source: &ThemeSource) -> Result<ResolvedTheme> {
    match source {
        ThemeSource::Omarchy(theme_dir) => load_omarchy_theme(theme_dir),
        ThemeSource::Json(path) => load_theme(path),
    }
}

/// Resolves a live source without converting transient native-theme absence
/// into the startup fallback palette.
///
/// # Errors
/// Returns an error when a native theme is absent, incomplete, changing, or
/// malformed, or when an explicit JSON override is invalid.
pub fn load_live_theme_source(source: &ThemeSource) -> Result<ResolvedTheme> {
    match source {
        ThemeSource::Omarchy(theme_dir) => load_live_omarchy_theme(theme_dir),
        ThemeSource::Json(path) => load_theme(path),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the caller validates Foot alpha in the closed 0.0..=1.0 range"
)]
fn foot_alpha(alpha: f32) -> u16 {
    (alpha * f32::from(u16::MAX)) as u16
}

fn parse_color(value: &str) -> Result<u32> {
    let value = value.trim();
    let hex = value
        .strip_prefix('#')
        .or_else(|| value.strip_prefix("0x"))
        .unwrap_or(value);
    let hex = if value.starts_with("0x") && hex.len() == 8 {
        &hex[2..]
    } else {
        hex
    };
    if hex.len() != 6 {
        bail!("color {value:?} must be #RRGGBB or 0xRRGGBB");
    }
    u32::from_str_radix(hex, 16).with_context(|| format!("invalid color {value:?}"))
}

fn blend_rgb(first: u32, second: u32) -> u32 {
    let red = u32::midpoint(first >> 16 & 0xff, second >> 16 & 0xff);
    let green = u32::midpoint(first >> 8 & 0xff, second >> 8 & 0xff);
    let blue = u32::midpoint(first & 0xff, second & 0xff);
    red << 16 | green << 8 | blue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_foot_font_and_resize_behavior() {
        let defaults = AppConfig::default();
        assert_eq!(defaults.font_size, FontSize::Pixels(14.0));
        assert_eq!(defaults.resize_delay_ms, 100);
        assert_eq!(defaults.keymap_profile, KeymapProfile::Splinterm);
        assert_eq!(defaults.keymap.bindings().len(), 31);
        assert_eq!(defaults.prefix_timeout_ms, 1_000);
        assert_eq!(defaults.preset_path, None);
        assert_eq!(defaults.preset_catalog, None);
        assert!(!defaults.allow_unrestricted_commands);
    }

    #[test]
    fn supported_subset_and_diagnostics_are_explicit() {
        let loaded = parse("[main]\nfont=Mono\nfont-size=14\ninitial-columns=100\napp-id=spoof\nunknown=x\n[cursor]\nstyle=beam\nblink=no\n").unwrap();
        assert_eq!(loaded.config.font, "Mono");
        assert_eq!(loaded.config.font_size, FontSize::Pixels(14.0));
        assert_eq!(loaded.config.initial_columns, 100);
        assert_eq!(loaded.config.cursor_style, CursorStyle::Beam);
        assert!(!loaded.config.cursor_blink);
        assert_eq!(loaded.diagnostics.len(), 3);
        assert!(
            loaded
                .diagnostics
                .iter()
                .any(|line| line.contains("font-size is deprecated"))
        );
    }
    #[test]
    fn geometry_font_units_and_padding_are_explicit() {
        let loaded = parse(
            "[main]\nfont-point-size=12\nfont-sizing-policy=physical-dpi\npadding-left=1\npadding-right=2\npadding-top=3\npadding-bottom=4\n",
        )
        .unwrap();
        assert_eq!(loaded.config.font_size, FontSize::Points(12.0));
        assert_eq!(
            loaded.config.font_sizing_policy,
            FontSizingPolicy::PhysicalDpi
        );
        assert_eq!(
            loaded.config.padding,
            TerminalPadding {
                left: 1,
                right: 2,
                top: 3,
                bottom: 4
            }
        );
    }

    #[test]
    fn legacy_splinterm_dpi_aware_migration_is_safe_and_conflicts_fail() {
        let legacy = parse("[main]\ndpi-aware=yes\n").unwrap();
        assert_eq!(
            legacy.config.font_sizing_policy,
            FontSizingPolicy::OutputScale
        );
        assert!(legacy.diagnostics[0].contains("legacy Splinterm"));
        let no = parse("[main]\ndpi-aware=no\n").unwrap_err().to_string();
        assert!(no.contains("forced the whole surface to 1x"));
        assert!(
            parse("[main]\ndpi-aware=yes\nfont-sizing-policy=output-scale\n")
                .unwrap_err()
                .to_string()
                .contains("conflicts")
        );
        assert!(
            parse("[main]\nfont-sizing-policy=output-scale\ndpi-aware=yes\n")
                .unwrap_err()
                .to_string()
                .contains("conflicts")
        );
    }

    #[test]
    fn font_sizing_authorities_cannot_conflict_or_hide_in_face_pattern() {
        assert!(parse("[main]\nfont-pixelsize=12\nfont-point-size=12\n").is_err());
        assert!(
            parse("[main]\nfont=Mono:size=12\n")
                .unwrap_err()
                .to_string()
                .contains("face/style")
        );
    }

    #[test]
    fn foot_background_alpha_and_blur_are_strict_last_assignment_overrides() {
        let loaded = parse("[colors]\nalpha=0.5\nblur=no\nalpha=0.888\nblur=yes\n").unwrap();
        assert_eq!(loaded.config.background_alpha, Some(foot_alpha(0.888)));
        assert_eq!(loaded.config.background_blur, Some(true));

        let defaults = parse("").unwrap().config;
        assert_eq!(defaults.background_alpha, None);
        assert_eq!(defaults.background_blur, None);
        assert_eq!(
            parse("[colors]\nblur=yes\nblur=no\n")
                .unwrap()
                .config
                .background_blur,
            Some(false)
        );
        for alpha in ["-0.1", "1.1", "NaN", "inf", "-inf"] {
            let error = parse(&format!("[colors]\nalpha={alpha}\n"))
                .unwrap_err()
                .to_string();
            assert_eq!(error, "line 2: number outside supported range");
        }
        assert!(
            parse("[colors]\nblur=perhaps\n")
                .unwrap_err()
                .to_string()
                .contains("line 2: expected boolean")
        );
    }

    #[test]
    fn invalid_ranges_and_values_fail() {
        assert!(parse("font-size=2").is_err());
        assert!(parse("[cursor]\nstyle=round").is_err());
        assert!(parse("login-shell=perhaps").is_err());
    }

    #[test]
    fn keymap_selection_is_strict_and_resolves_the_builtin_profile() {
        let loaded =
            parse("[key-bindings]\nprofile=splinterm\nprefix-timeout-ms=750\nunknown=value\n")
                .unwrap();
        assert_eq!(loaded.config.keymap.profile(), KeymapProfile::Splinterm);
        assert_eq!(loaded.config.keymap.bindings().len(), 31);
        assert_eq!(loaded.config.prefix_timeout_ms, 750);
        assert_eq!(loaded.diagnostics.len(), 1);
        assert!(loaded.diagnostics[0].contains("key-bindings.unknown"));
        let omarchy = parse("[key-bindings]\nprofile=omarchy-tmux\n").unwrap();
        assert_eq!(omarchy.config.keymap.profile(), KeymapProfile::OmarchyTmux);
        assert_eq!(omarchy.config.keymap.prefixes().len(), 2);
        assert!(parse("[key-bindings]\nprofile=missing\n").is_err());
        assert!(parse("[key-bindings]\nprefix-timeout-ms=249\n").is_err());
        assert!(parse("[key-bindings]\nprefix-timeout-ms=5001\n").is_err());
    }
    #[test]
    fn preset_selection_resolves_relative_paths_and_validates_explicit_files() {
        let root = std::env::temp_dir().join(format!(
            "splinterm-preset-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("presets.toml"),
            "version=1\n[commands.run]\nkind=\"argv\"\nargv=[\"true\"]\n[presets.one]\nkind=\"dojo\"\nname=\"one\"\nroot=\"pane\"\nfocus=\"pane\"\n[presets.one.nodes.pane]\ntype=\"pane\"\ncommand=\"run\"\n",
        )
        .unwrap();
        let loaded = parse_with_base(
            "[presets]\nfile=presets.toml\nallow-unrestricted-commands=yes\n",
            &root,
        )
        .unwrap();
        assert_eq!(loaded.config.preset_path, Some(root.join("presets.toml")));
        assert!(
            loaded
                .config
                .preset_catalog
                .as_ref()
                .unwrap()
                .contains("one")
        );
        assert!(loaded.config.allow_unrestricted_commands);
        assert!(parse_with_base("[presets]\nfile=missing.toml\n", &root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pane_divider_configuration_is_explicit_and_bounded() {
        let defaults = parse("").unwrap().config;
        assert_eq!(defaults.pane_divider_style, PaneDividerStyle::Line);
        assert_eq!(defaults.frame_title_mode, FrameTitleMode::Splint);
        for (value, expected) in [
            ("none", PaneDividerStyle::None),
            ("line", PaneDividerStyle::Line),
            ("frame", PaneDividerStyle::Frame),
        ] {
            let loaded = parse(&format!("[multiplexer]\ndivider-style={value}\n")).unwrap();
            assert_eq!(loaded.config.pane_divider_style, expected);
        }
        for (value, expected) in [
            ("none", FrameTitleMode::None),
            ("splint", FrameTitleMode::Splint),
        ] {
            let loaded = parse(&format!("[multiplexer]\nframe-title={value}\n")).unwrap();
            assert_eq!(loaded.config.frame_title_mode, expected);
        }
        assert!(parse("[multiplexer]\ndivider-style=tmux\n").is_err());
        assert!(parse("[multiplexer]\nframe-title=osc\n").is_err());
    }

    #[test]
    fn theme_source_defaults_to_native_omarchy_and_allows_explicit_json() {
        let defaults = AppConfig::default();
        assert!(defaults.theme_path.is_none());
        assert!(matches!(defaults.theme_source(), ThemeSource::Omarchy(_)));

        let loaded = parse("[main]\ntheme=/tmp/splinterm-theme.json\n")
            .unwrap()
            .config;
        assert_eq!(
            loaded.theme_source(),
            ThemeSource::Json(PathBuf::from("/tmp/splinterm-theme.json"))
        );
    }

    #[test]
    fn native_omarchy_theme_uses_effective_foot_palette_and_semantic_accent() {
        let colors = "accent = \"0x010203\" # inline comment\n";
        let foot = "[colors]\nforeground=000003\nbackground=000001\nselection-foreground=000002\nselection-background=000004\ncursor=000001 000006\nalpha=0.75\nblur=yes\nregular0=000000\nregular1=000001\nregular2=000002\nregular3=000003\nregular4=000004\nregular5=000005\nregular6=000006\nregular7=000007\nbright0=000008\nbright1=000009\nbright2=00000a\nbright3=00000b\nbright4=00000c\nbright5=00000d\nbright6=00000e\nbright7=00000f\n";
        let theme = resolve_omarchy_theme(colors, foot).unwrap();
        assert_eq!(theme.background, 1);
        assert_eq!(theme.foreground, 3);
        assert_eq!(theme.cursor, 6);
        assert_eq!(theme.selection, 4);
        assert_eq!(theme.selection_foreground, 2);
        assert_eq!(theme.url, 4);
        assert_eq!(theme.ui_accent, 0x01_02_03);
        assert_eq!(theme.pane_border, 8);
        assert_eq!(theme.pane_border_active, 0x01_02_03);
        assert_eq!(
            theme.ansi,
            std::array::from_fn(|index| u32::try_from(index).unwrap())
        );
        assert_eq!(theme.background_alpha, foot_alpha(0.75));
        assert!(theme.background_blur);
    }

    #[test]
    fn native_omarchy_theme_prefers_colors_dark_and_rejects_incomplete_input() {
        let complete = |background: &str| {
            format!(
                "foreground=000003\nbackground={background}\nregular0=000000\nregular1=000001\nregular2=000002\nregular3=000003\nregular4=000004\nregular5=000005\nregular6=000006\nregular7=000007\nbright0=000008\nbright1=000009\nbright2=00000a\nbright3=00000b\nbright4=00000c\nbright5=00000d\nbright6=00000e\nbright7=00000f\n"
            )
        };
        let foot = format!(
            "[colors]\n{}[colors-dark]\n{}",
            complete("000001"),
            complete("000002")
        );
        let resolved = resolve_omarchy_theme("cursor=\"#000006\"", &foot).unwrap();
        assert_eq!(resolved.background, 2);
        assert_eq!(resolved.selection_foreground, resolved.foreground);
        assert!(
            resolve_omarchy_theme("accent=\"#000006\"", "[colors-dark]\nbackground=000001")
                .unwrap_err()
                .to_string()
                .contains("missing regular0")
        );
        let empty_dark = format!("[colors]\n{}[colors-dark]\n", complete("000001"));
        assert!(
            resolve_omarchy_theme("accent=\"#000006\"", &empty_dark)
                .unwrap_err()
                .to_string()
                .contains("no [colors-dark] or [colors] palette")
        );
    }

    #[test]
    fn native_omarchy_theme_rejects_cross_generation_reads() {
        let root = std::env::temp_dir().join(format!(
            "splinterm-native-theme-snapshot-{}",
            std::process::id()
        ));
        let theme = root.join("theme");
        let old = root.join("old");
        let next = root.join("next");
        let foot = |background: &str| {
            format!(
                "[colors-dark]\nforeground=000003\nbackground={background}\nregular0=000000\nregular1=000001\nregular2=000002\nregular3=000003\nregular4=000004\nregular5=000005\nregular6=000006\nregular7=000007\nbright0=000008\nbright1=000009\nbright2=00000a\nbright3=00000b\nbright4=00000c\nbright5=00000d\nbright6=00000e\nbright7=00000f\n"
            )
        };
        for (directory, accent, background) in
            [(&theme, "#000006", "000001"), (&next, "#000007", "000002")]
        {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(
                directory.join("colors.toml"),
                format!("accent=\"{accent}\"\n"),
            )
            .unwrap();
            std::fs::write(directory.join("foot.ini"), foot(background)).unwrap();
        }
        let error = load_omarchy_theme_with(
            &theme,
            MissingOmarchyTheme::Reject,
            || {},
            || {
                std::fs::rename(&theme, &old).unwrap();
                std::fs::rename(&next, &theme).unwrap();
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("changed while its palette was loading"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_live_theme_never_publishes_fallback_during_replacement_gap() {
        let root =
            std::env::temp_dir().join(format!("splinterm-native-theme-gap-{}", std::process::id()));
        let theme = root.join("theme");
        let displaced = root.join("displaced");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), "accent=\"#000006\"\n").unwrap();
        std::fs::write(theme.join("foot.ini"), "[colors-dark]\n").unwrap();

        let error = load_omarchy_theme_with(
            &theme,
            MissingOmarchyTheme::Reject,
            || std::fs::rename(&theme, &displaced).unwrap(),
            || {},
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("colors.toml"));
        assert!(load_live_omarchy_theme(&theme).is_err());
        assert_eq!(
            load_omarchy_theme(&theme).unwrap(),
            ResolvedTheme::default()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn theme_maps_every_omarchy_role_and_defaults_new_pane_roles() {
        let json = r##"{"background":"#000001","foreground":"#000003","cursor":"#000003","selection":"#000004","url":"#000005","ui_accent":"#000006","ansi":["#000000","#000001","#000002","#000003","#000004","#000005","#000006","#000007","#000008","#000009","#00000a","#00000b","#00000c","#00000d","#00000e","#00000f"]}"##;
        let theme: ThemePalette = serde_json::from_str(json).unwrap();
        let resolved = theme.resolve().unwrap();
        assert_eq!(resolved.background, 1);
        assert_eq!(resolved.selection_foreground, 3);
        assert_eq!(resolved.ui_accent, 6);
        assert_eq!(resolved.pane_border, 2);
        assert_eq!(resolved.pane_border_active, 6);
        assert_eq!(resolved.background_alpha, u16::MAX);
        assert!(!resolved.background_blur);
        assert_eq!(resolved.ansi[15], 15);

        let explicit = json.replace(
            "\"ansi\"",
            "\"selection_foreground\":\"#000002\",\"pane_border\":\"#000007\",\"pane_border_active\":\"#000008\",\"ansi\"",
        );
        let resolved = serde_json::from_str::<ThemePalette>(&explicit)
            .unwrap()
            .resolve()
            .unwrap();
        assert_eq!(resolved.selection_foreground, 2);
        assert_eq!(resolved.pane_border, 7);
        assert_eq!(resolved.pane_border_active, 8);

        let themed = json.replace(
            "\"ui_accent\"",
            "\"alpha\":0.85,\"blur\":true,\"ui_accent\"",
        );
        let resolved = serde_json::from_str::<ThemePalette>(&themed)
            .unwrap()
            .resolve()
            .unwrap();
        assert_eq!(resolved.background_alpha, foot_alpha(0.85));
        assert!(resolved.background_blur);
        let overridden = resolved.with_color_overrides(Some(foot_alpha(0.7)), Some(false));
        assert_eq!(overridden.background_alpha, foot_alpha(0.7));
        assert!(!overridden.background_blur);
        assert!(
            serde_json::from_str::<ThemePalette>(
                &json.replace("\"ui_accent\"", "\"alpha\":1.1,\"ui_accent\"")
            )
            .unwrap()
            .resolve()
            .is_err()
        );
        assert!(
            serde_json::from_str::<ThemePalette>(
                &json.replace("\"ui_accent\"", "\"blur\":\"yes\",\"ui_accent\"")
            )
            .is_err()
        );
    }
}
