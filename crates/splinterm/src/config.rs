//! Project-owned MVP configuration and Omarchy palette bridge.
//!
//! This parser deliberately accepts only the documented Foot-compatible subset.
//! Unknown sections and keys are diagnostics, never silently accepted compatibility.

#![allow(
    clippy::too_many_lines,
    clippy::unreadable_literal,
    reason = "configuration tables remain more auditable when values stay adjacent to their keys"
)]

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::geometry::{FontSize, FontSizingPolicy, TerminalPadding};

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
    /// Foot-compatible 16-bit background alpha (`[colors] alpha`).
    pub background_alpha: u16,
    pub theme_path: PathBuf,
    pub pane_divider_style: PaneDividerStyle,
    pub frame_title_mode: FrameTitleMode,
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
            resize_delay_ms: 0,
            background_alpha: u16::MAX,
            theme_path: default_config_dir().join("theme.json"),
            pane_divider_style: PaneDividerStyle::Line,
            frame_title_mode: FrameTitleMode::Splint,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigLoad {
    pub config: AppConfig,
    pub diagnostics: Vec<String>,
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
    parse(&fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}

/// Parses the documented MVP configuration subset.
///
/// # Errors
/// Returns an error for malformed syntax or invalid supported values.
pub fn parse(text: &str) -> Result<ConfigLoad> {
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
                "main" | "scrollback" | "cursor" | "colors" | "key-bindings" | "multiplexer"
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
                config.theme_path = expand_path(value);
                false
            }
            "colors.alpha" => {
                let alpha = parse_range(value, 0.0_f32, 1.0_f32, index)?;
                config.background_alpha = foot_alpha(alpha);
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
            key if key.starts_with("colors.") => {
                diagnostics.push(format!(
                    "line {}: put colors in the generated theme JSON; {key} ignored",
                    index + 1
                ));
                false
            }
            key if key.starts_with("key-bindings.") => {
                diagnostics.push(format!(
                    "line {}: built-in MVP key binding {key} is documented but not remappable",
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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePalette {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    pub url: String,
    pub ui_accent: String,
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
    pub url: u32,
    pub ui_accent: u32,
    pub pane_border: u32,
    pub pane_border_active: u32,
    pub ansi: [u32; 16],
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            background: 0x0e1216,
            foreground: 0xebebeb,
            cursor: 0xebebeb,
            selection: 0x354a60,
            url: 0x78beff,
            ui_accent: 0x78d2ff,
            pane_border: 0x7c7e80,
            pane_border_active: 0x78d2ff,
            ansi: [
                0x1d2021, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ],
        }
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
        let background = parse_color(&self.background)?;
        let foreground = parse_color(&self.foreground)?;
        let ui_accent = parse_color(&self.ui_accent)?;
        Ok(ResolvedTheme {
            background,
            foreground,
            cursor: parse_color(&self.cursor)?,
            selection: parse_color(&self.selection)?,
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
            ansi,
        })
    }
}

/// Loads a generated Omarchy role map, or the safe fallback when absent.
///
/// # Errors
/// Returns an error when an existing theme file is unreadable or invalid.
pub fn load_theme(path: &Path) -> Result<ResolvedTheme> {
    if !path.exists() {
        return Ok(ResolvedTheme::default());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str::<ThemePalette>(&raw)
        .context("parse generated Omarchy theme JSON")?
        .resolve()
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
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 {
        bail!("color {value:?} must be #RRGGBB");
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
    fn default_font_size_is_laptop_reasonable() {
        assert_eq!(AppConfig::default().font_size, FontSize::Pixels(14.0));
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
    fn foot_background_alpha_is_bounded_and_exact() {
        assert_eq!(
            parse("[colors]\nalpha=0.888\n")
                .unwrap()
                .config
                .background_alpha,
            foot_alpha(0.888)
        );
        assert!(parse("[colors]\nalpha=-0.1\n").is_err());
        assert!(parse("[colors]\nalpha=1.1\n").is_err());
    }

    #[test]
    fn invalid_ranges_and_values_fail() {
        assert!(parse("font-size=2").is_err());
        assert!(parse("[cursor]\nstyle=round").is_err());
        assert!(parse("login-shell=perhaps").is_err());
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
    fn theme_maps_every_omarchy_role_and_defaults_new_pane_roles() {
        let json = r##"{"background":"#000001","foreground":"#000003","cursor":"#000003","selection":"#000004","url":"#000005","ui_accent":"#000006","ansi":["#000000","#000001","#000002","#000003","#000004","#000005","#000006","#000007","#000008","#000009","#00000a","#00000b","#00000c","#00000d","#00000e","#00000f"]}"##;
        let theme: ThemePalette = serde_json::from_str(json).unwrap();
        let resolved = theme.resolve().unwrap();
        assert_eq!(resolved.background, 1);
        assert_eq!(resolved.ui_accent, 6);
        assert_eq!(resolved.pane_border, 2);
        assert_eq!(resolved.pane_border_active, 6);
        assert_eq!(resolved.ansi[15], 15);

        let explicit = json.replace(
            "\"ansi\"",
            "\"pane_border\":\"#000007\",\"pane_border_active\":\"#000008\",\"ansi\"",
        );
        let resolved = serde_json::from_str::<ThemePalette>(&explicit)
            .unwrap()
            .resolve()
            .unwrap();
        assert_eq!(resolved.pane_border, 7);
        assert_eq!(resolved.pane_border_active, 8);
    }
}
