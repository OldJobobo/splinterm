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

pub const APP_ID: &str = "com.oldjobobo.splinterm";
pub const DEFAULT_FONT: &str = "JetBrains Mono Nerd Font:style=Regular";

#[derive(Clone, Debug, PartialEq)]
pub struct AppConfig {
    pub font: String,
    pub font_size: f32,
    pub initial_columns: u16,
    pub initial_rows: u16,
    pub shell: Option<String>,
    pub login_shell: bool,
    pub title: Option<String>,
    pub scrollback_lines: usize,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub resize_delay_ms: u64,
    pub dpi_aware: bool,
    pub theme_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorStyle {
    #[default]
    Block,
    Beam,
    Underline,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            font: DEFAULT_FONT.to_owned(),
            font_size: 22.0,
            initial_columns: 80,
            initial_rows: 24,
            shell: None,
            login_shell: true,
            title: None,
            scrollback_lines: 1_000,
            cursor_style: CursorStyle::Block,
            cursor_blink: true,
            resize_delay_ms: 0,
            dpi_aware: true,
            theme_path: default_config_dir().join("theme.json"),
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
                "main" | "scrollback" | "cursor" | "colors" | "key-bindings"
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
                config.font = nonempty(value, index)?;
                false
            }
            "main.font-size" | "font-size" => {
                config.font_size = parse_range(value, 6.0, 96.0, index)?;
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
                config.dpi_aware = parse_bool(value, index)?;
                false
            }
            "main.theme" => {
                config.theme_path = expand_path(value);
                false
            }
            "scrollback.lines" => {
                config.scrollback_lines = parse_range(value, 0, 1_000_000, index)?;
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
        Ok(ResolvedTheme {
            background: parse_color(&self.background)?,
            foreground: parse_color(&self.foreground)?,
            cursor: parse_color(&self.cursor)?,
            selection: parse_color(&self.selection)?,
            url: parse_color(&self.url)?,
            ui_accent: parse_color(&self.ui_accent)?,
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

fn parse_color(value: &str) -> Result<u32> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 {
        bail!("color {value:?} must be #RRGGBB");
    }
    u32::from_str_radix(hex, 16).with_context(|| format!("invalid color {value:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_subset_and_diagnostics_are_explicit() {
        let loaded = parse("[main]\nfont=Mono\nfont-size=14\ninitial-columns=100\napp-id=spoof\nunknown=x\n[cursor]\nstyle=beam\nblink=no\n").unwrap();
        assert_eq!(loaded.config.font, "Mono");
        assert!((loaded.config.font_size - 14.0).abs() < f32::EPSILON);
        assert_eq!(loaded.config.initial_columns, 100);
        assert_eq!(loaded.config.cursor_style, CursorStyle::Beam);
        assert!(!loaded.config.cursor_blink);
        assert_eq!(loaded.diagnostics.len(), 2);
    }
    #[test]
    fn invalid_ranges_and_values_fail() {
        assert!(parse("font-size=2").is_err());
        assert!(parse("[cursor]\nstyle=round").is_err());
        assert!(parse("login-shell=perhaps").is_err());
    }
    #[test]
    fn theme_maps_every_omarchy_role() {
        let json = r##"{"background":"#000001","foreground":"#000002","cursor":"#000003","selection":"#000004","url":"#000005","ui_accent":"#000006","ansi":["#000000","#000001","#000002","#000003","#000004","#000005","#000006","#000007","#000008","#000009","#00000a","#00000b","#00000c","#00000d","#00000e","#00000f"]}"##;
        let theme: ThemePalette = serde_json::from_str(json).unwrap();
        let resolved = theme.resolve().unwrap();
        assert_eq!(resolved.background, 1);
        assert_eq!(resolved.ui_accent, 6);
        assert_eq!(resolved.ansi[15], 15);
    }
}
