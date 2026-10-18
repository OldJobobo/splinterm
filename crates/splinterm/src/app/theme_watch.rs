use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::Result;
use splinterm::{
    ThemeUpdate, WindowTopologyUpdate, WindowUpdate,
    config::{
        AppConfig, OmarchyShellConcern, OmarchyShellSettings, OmarchyTextSizeFallback,
        OmarchyThemeRoots, OmarchyThemeSelection, ResolvedTheme, ThemeSource,
        load_live_omarchy_theme, load_omarchy_shell, load_theme, load_theme_source,
    },
};
use tokio::sync::mpsc;

static NEXT_THEME_UPDATE_GENERATION: AtomicU64 = AtomicU64::new(1);

type ThemeFileFingerprint = (u64, u64, u64, i64, i64);

/// What `config.ini` overrides in the active theme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) struct ThemeOverrides {
    pub(in crate::app) alpha: Option<u16>,
    pub(in crate::app) blur: Option<bool>,
    /// Whether the Omarchy text size applies: only while `config.ini` sets no
    /// font size.
    pub(in crate::app) follow_text_size: bool,
}

impl ThemeOverrides {
    pub(in crate::app) fn from_config(config: &AppConfig) -> Self {
        Self {
            alpha: config.background_alpha,
            blur: config.background_blur,
            follow_text_size: config.follows_omarchy_text_size(),
        }
    }

    fn apply(self, mut theme: ResolvedTheme) -> ResolvedTheme {
        if !self.follow_text_size {
            theme.shell.text_size_px = None;
        }
        theme.with_color_overrides(self.alpha, self.blur)
    }
}

/// The `shell.toml` layers one poll reads: the selected theme directory's
/// file and the user's override. `panels` is false for an explicit JSON
/// palette, which follows only the text size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShellLayers<'a> {
    theme_dir: &'a Path,
    user: Option<&'a Path>,
    panels: bool,
}

/// The concrete files one poll reads. Omarchy sources pick one of their
/// ordered roots per poll, so a migration or root change is followed live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveThemeSource<'a> {
    Omarchy {
        index: usize,
        theme_dir: &'a Path,
        user_shell: Option<&'a Path>,
    },
    Json {
        path: &'a Path,
        shell: Option<JsonShellRoot<'a>>,
    },
}

/// The Omarchy root an explicit JSON palette reads `shell.toml` from, chosen
/// with the same live selection and demotion grace as the native palette.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct JsonShellRoot<'a> {
    layers: ShellLayers<'a>,
    index: usize,
    /// The selected theme directory is missing while another root is claimed:
    /// a theme switch or a briefly vanished root, so the current shell
    /// settings are kept rather than read from a partial or stale root.
    gap: bool,
}

impl<'a> ActiveThemeSource<'a> {
    fn shell(self) -> Option<ShellLayers<'a>> {
        match self {
            Self::Omarchy {
                theme_dir,
                user_shell,
                ..
            } => Some(ShellLayers {
                theme_dir,
                user: user_shell,
                panels: true,
            }),
            Self::Json { shell, .. } => shell.map(|root| root.layers),
        }
    }
}

/// The stateless JSON shell layers for a one-shot startup read.
fn json_shell_layers(roots: Option<&OmarchyThemeRoots>) -> Option<ShellLayers<'_>> {
    roots.map(|roots| ShellLayers {
        theme_dir: roots.select(),
        user: roots.user_shell(),
        panels: false,
    })
}

fn active_theme_source<'a>(
    source: &'a ThemeSource,
    selection: &mut OmarchyThemeSelection,
) -> ActiveThemeSource<'a> {
    match source {
        ThemeSource::Omarchy(roots) => {
            let index = selection.select(roots);
            ActiveThemeSource::Omarchy {
                index,
                theme_dir: &roots.candidates()[index],
                user_shell: roots.user_shell(),
            }
        }
        ThemeSource::Json { path, shell } => ActiveThemeSource::Json {
            path,
            shell: shell.as_ref().map(|roots| {
                let index = selection.select(roots);
                let theme_dir = roots.candidates()[index].as_path();
                JsonShellRoot {
                    layers: ShellLayers {
                        theme_dir,
                        user: roots.user_shell(),
                        panels: false,
                    },
                    index,
                    gap: !theme_dir.is_dir() && roots.any_claimed(),
                }
            }),
        },
    }
}

fn theme_file_fingerprint(path: &Path) -> Option<ThemeFileFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PaletteFingerprint {
    Omarchy {
        root: PathBuf,
        directory: Option<ThemeFileFingerprint>,
        colors: Option<ThemeFileFingerprint>,
        foot: Option<ThemeFileFingerprint>,
    },
    Json(Option<ThemeFileFingerprint>),
}

/// The theme's own `shell.toml` and the user's override.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ShellFingerprint {
    theme: Option<ThemeFileFingerprint>,
    user: Option<ThemeFileFingerprint>,
}

impl ShellFingerprint {
    fn of(layers: ShellLayers<'_>) -> Self {
        Self {
            theme: theme_file_fingerprint(&layers.theme_dir.join("shell.toml")),
            user: layers.user.and_then(theme_file_fingerprint),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ThemeSourceFingerprint {
    palette: PaletteFingerprint,
    shell: Option<ShellFingerprint>,
}

fn theme_source_fingerprint(source: ActiveThemeSource<'_>) -> ThemeSourceFingerprint {
    let palette = match source {
        ActiveThemeSource::Omarchy { theme_dir, .. } => PaletteFingerprint::Omarchy {
            root: theme_dir.to_path_buf(),
            directory: theme_file_fingerprint(theme_dir),
            colors: theme_file_fingerprint(&theme_dir.join("colors.toml")),
            foot: theme_file_fingerprint(&theme_dir.join("foot.ini")),
        },
        ActiveThemeSource::Json { path, .. } => {
            PaletteFingerprint::Json(theme_file_fingerprint(path))
        }
    };
    ThemeSourceFingerprint {
        palette,
        shell: source.shell().map(ShellFingerprint::of),
    }
}

/// Layers the Omarchy shell settings onto a resolved palette. A JSON palette
/// takes only the text size and keeps Omarchy's opaque panel defaults.
/// Diagnostics for a setting Splinterm ignores are dropped: alpha for a JSON
/// palette, and the text size while `config.ini` sets a font size.
fn with_omarchy_shell(
    mut theme: ResolvedTheme,
    layers: ShellLayers<'_>,
    follow_text_size: bool,
    fallback: OmarchyTextSizeFallback,
    diagnostics: &mut Vec<String>,
) -> ResolvedTheme {
    if !layers.panels && !follow_text_size {
        theme.shell = OmarchyShellSettings::default();
        return theme;
    }
    let load = load_omarchy_shell(layers.theme_dir, layers.user, fallback);
    diagnostics.extend(
        load.diagnostics
            .into_iter()
            .filter(|(concern, _)| match concern {
                OmarchyShellConcern::Read => true,
                OmarchyShellConcern::PanelAlpha => layers.panels,
                OmarchyShellConcern::TextSize => follow_text_size,
            })
            .map(|(_, diagnostic)| diagnostic),
    );
    theme.shell = if layers.panels {
        load.settings
    } else {
        OmarchyShellSettings {
            text_size_px: load.settings.text_size_px,
            ..OmarchyShellSettings::default()
        }
    };
    theme
}

struct StartupThemeLoad {
    theme: ResolvedTheme,
    /// Everything to print at startup, prefixed.
    diagnostics: Vec<String>,
    /// The raw shell diagnostics among them, which seed the live watcher.
    shell_diagnostics: Vec<String>,
}

fn prepare_startup_theme(source: &ThemeSource, overrides: ThemeOverrides) -> StartupThemeLoad {
    let mut diagnostics = Vec::new();
    let theme = load_theme_source(source).unwrap_or_else(|error| {
        diagnostics.push(format!("{error:#}; using safe fallback palette"));
        ResolvedTheme::default()
    });
    let layers = match source {
        ThemeSource::Omarchy(roots) => Some(ShellLayers {
            theme_dir: roots.select(),
            user: roots.user_shell(),
            panels: true,
        }),
        ThemeSource::Json { shell, .. } => json_shell_layers(shell.as_ref()),
    };
    let mut shell_diagnostics = Vec::new();
    let theme = match layers {
        Some(layers) => with_omarchy_shell(
            theme,
            layers,
            overrides.follow_text_size,
            OmarchyTextSizeFallback::OmarchyDefault,
            &mut shell_diagnostics,
        ),
        None => theme,
    };
    diagnostics.extend(shell_diagnostics.iter().cloned());
    StartupThemeLoad {
        theme: overrides.apply(theme),
        diagnostics: diagnostics
            .into_iter()
            .map(|diagnostic| format!("splinterm theme: {diagnostic}"))
            .collect(),
        shell_diagnostics,
    }
}

/// The startup theme a live watcher continues from, with the shell
/// diagnostics startup already printed so the watcher's first poll does not
/// print them again.
pub(in crate::app) struct StartupTheme {
    pub(in crate::app) theme: ResolvedTheme,
    shell_reported: Vec<String>,
}

fn load_startup_theme_with_reporter(
    config: &AppConfig,
    mut report: impl FnMut(&str),
) -> StartupTheme {
    let loaded = prepare_startup_theme(&config.theme_source(), ThemeOverrides::from_config(config));
    for diagnostic in &loaded.diagnostics {
        report(diagnostic);
    }
    StartupTheme::from(loaded)
}

impl From<StartupThemeLoad> for StartupTheme {
    fn from(loaded: StartupThemeLoad) -> Self {
        Self {
            theme: loaded.theme,
            shell_reported: loaded.shell_diagnostics,
        }
    }
}

pub(in crate::app) fn load_startup_theme(config: &AppConfig) -> ResolvedTheme {
    load_startup_theme_for_watch(config).theme
}

/// Loads the startup theme for a Window that also starts `watch_theme`.
pub(in crate::app) fn load_startup_theme_for_watch(config: &AppConfig) -> StartupTheme {
    load_startup_theme_with_reporter(config, |diagnostic| eprintln!("{diagnostic}"))
}

/// Loads the active source. A coherent Omarchy load records its root as the
/// last good one, which `OmarchyThemeSelection` holds through transient gaps.
/// Rejected shell settings are reported through `shell_diagnostics` and never
/// reject the palette; an invalid text size keeps the current one.
fn resolve_live_theme_update(
    source: ActiveThemeSource<'_>,
    selection: &mut OmarchyThemeSelection,
    overrides: ThemeOverrides,
    current: ResolvedTheme,
    shell_diagnostics: &mut Vec<String>,
) -> Result<Option<ResolvedTheme>> {
    let palette = match source {
        ActiveThemeSource::Omarchy {
            index, theme_dir, ..
        } => {
            let theme = load_live_omarchy_theme(theme_dir)?;
            selection.accepted(index);
            theme
        }
        ActiveThemeSource::Json { path, .. } => load_theme(path)?,
    };
    if let ActiveThemeSource::Json {
        shell: Some(root), ..
    } = source
    {
        if root.gap {
            let mut theme = palette;
            theme.shell = current.shell;
            let next = overrides.apply(theme);
            return Ok((next != current).then_some(next));
        }
        if root.layers.theme_dir.is_dir() {
            selection.accepted(root.index);
        }
    }
    let next = overrides.apply(match source.shell() {
        Some(layers) => with_omarchy_shell(
            palette,
            layers,
            overrides.follow_text_size,
            OmarchyTextSizeFallback::Current(current.shell.text_size_px),
            shell_diagnostics,
        ),
        None => palette,
    });
    Ok((next != current).then_some(next))
}

#[derive(Default)]
struct ThemeReloadDiagnostics {
    rejection_reported: bool,
    shell_reported: Vec<String>,
}

impl ThemeReloadDiagnostics {
    /// Continues from startup, which already printed its shell diagnostics.
    fn after_startup(startup: &StartupTheme) -> Self {
        Self {
            rejection_reported: false,
            shell_reported: startup.shell_reported.clone(),
        }
    }

    fn accepted(&mut self) {
        self.rejection_reported = false;
    }

    fn rejected(&mut self, error: &anyhow::Error) -> Option<String> {
        if self.rejection_reported {
            return None;
        }
        self.rejection_reported = true;
        Some(format!("splinterm theme reload rejected: {error:#}"))
    }

    /// Returns the shell diagnostics not already reported by the previous
    /// load, so a persisting invalid value is reported once.
    fn shell(&mut self, diagnostics: Vec<String>) -> Vec<String> {
        let fresh = diagnostics
            .iter()
            .filter(|diagnostic| !self.shell_reported.contains(diagnostic))
            .map(|diagnostic| format!("splinterm theme: {diagnostic}"))
            .collect();
        self.shell_reported = diagnostics;
        fresh
    }
}

pub(in crate::app) enum ThemeUpdateSink {
    Panes(Vec<mpsc::Sender<WindowUpdate>>),
    Topology(mpsc::Sender<WindowTopologyUpdate>),
}

pub(in crate::app) async fn watch_theme(
    source: ThemeSource,
    overrides: ThemeOverrides,
    startup: StartupTheme,
    sink: ThemeUpdateSink,
) {
    let mut current = startup.theme;
    let mut observed = None;
    let mut selection = OmarchyThemeSelection::default();
    let mut diagnostics = ThemeReloadDiagnostics::after_startup(&startup);
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(500));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        poll.tick().await;
        let active = active_theme_source(&source, &mut selection);
        let next_fingerprint = theme_source_fingerprint(active);
        if observed.as_ref() == Some(&next_fingerprint) {
            continue;
        }
        observed = Some(next_fingerprint);
        let mut shell_diagnostics = Vec::new();
        let resolved = resolve_live_theme_update(
            active,
            &mut selection,
            overrides,
            current,
            &mut shell_diagnostics,
        );
        if resolved.is_ok() {
            for diagnostic in diagnostics.shell(shell_diagnostics) {
                eprintln!("{diagnostic}");
            }
        }
        match resolved {
            Ok(Some(next)) => {
                current = next;
                diagnostics.accepted();
                let update = ThemeUpdate {
                    generation: NEXT_THEME_UPDATE_GENERATION.fetch_add(1, Ordering::Relaxed),
                    theme: next,
                };
                let delivered = match &sink {
                    ThemeUpdateSink::Panes(updates) => {
                        let mut delivered = false;
                        for updates in updates {
                            delivered |= updates.send(WindowUpdate::Theme(update)).await.is_ok();
                        }
                        delivered
                    }
                    ThemeUpdateSink::Topology(updates) => updates
                        .send(WindowTopologyUpdate::Theme(update))
                        .await
                        .is_ok(),
                };
                if !delivered {
                    break;
                }
            }
            Ok(None) => diagnostics.accepted(),
            Err(error) => {
                if let Some(diagnostic) = diagnostics.rejected(&error) {
                    eprintln!("{diagnostic}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm::config::OMARCHY_ROOT_DEMOTION_GRACE_POLLS;

    const NO_OVERRIDES: ThemeOverrides = ThemeOverrides {
        alpha: None,
        blur: None,
        follow_text_size: true,
    };

    fn fingerprint(source: &ThemeSource) -> ThemeSourceFingerprint {
        theme_source_fingerprint(active_theme_source(
            source,
            &mut OmarchyThemeSelection::default(),
        ))
    }

    fn live_update(
        source: &ThemeSource,
        alpha_override: Option<u16>,
        blur_override: Option<bool>,
        current: ResolvedTheme,
    ) -> Result<Option<ResolvedTheme>> {
        let mut selection = OmarchyThemeSelection::default();
        resolve_live_theme_update(
            active_theme_source(source, &mut selection),
            &mut selection,
            ThemeOverrides {
                alpha: alpha_override,
                blur: blur_override,
                follow_text_size: true,
            },
            current,
            &mut Vec::new(),
        )
    }

    fn write_theme(theme_dir: &Path, background: u32) {
        use std::fmt::Write as _;

        std::fs::create_dir_all(theme_dir).unwrap();
        std::fs::write(theme_dir.join("colors.toml"), "accent=\"#010203\"\n").unwrap();
        let mut foot = format!("[colors-dark]\nbackground={background:06x}\nforeground=e0e1e2\n");
        for index in 0..8 {
            write!(
                foot,
                "regular{index}={index:06x}\nbright{index}={:06x}\n",
                index + 8
            )
            .unwrap();
        }
        std::fs::write(theme_dir.join("foot.ini"), foot).unwrap();
    }

    /// `None` marks a rejected read, which keeps the last published palette.
    fn poll(source: &ThemeSource, selection: &mut OmarchyThemeSelection) -> (PathBuf, Option<u32>) {
        let active = active_theme_source(source, selection);
        let ActiveThemeSource::Omarchy { theme_dir, .. } = active else {
            unreachable!("Omarchy source");
        };
        let loaded = resolve_live_theme_update(
            active,
            selection,
            NO_OVERRIDES,
            ResolvedTheme::default(),
            &mut Vec::new(),
        )
        .map(|theme| {
            theme.map_or(ResolvedTheme::default().background, |theme| {
                theme.background
            })
        })
        .ok();
        (theme_dir.to_path_buf(), loaded)
    }

    #[test]
    fn omarchy_roots_follow_xdg_then_home_then_omarchy_3_and_migrations_live() {
        let root = std::env::temp_dir().join(format!(
            "splinterm-omarchy-root-order-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let xdg = root.join("xdg/omarchy/current/theme");
        let home_state = root.join("home/.local/state/omarchy/current/theme");
        let legacy = root.join("home/.config/omarchy/current/theme");
        let source = ThemeSource::Omarchy(
            OmarchyThemeRoots::ordered(vec![xdg.clone(), home_state.clone(), legacy.clone()])
                .unwrap(),
        );
        let mut selection = OmarchyThemeSelection::default();

        // Omarchy 3.x only: the read-only fallback is used.
        write_theme(&legacy, 0x30_30_30);
        assert_eq!(
            poll(&source, &mut selection),
            (legacy.clone(), Some(0x30_30_30))
        );

        // The Omarchy 4 migration moves current/ to ~/.local/state; a higher
        // root is followed immediately.
        write_theme(&home_state, 0x40_40_40);
        assert_eq!(
            poll(&source, &mut selection),
            (home_state.clone(), Some(0x40_40_40))
        );

        // XDG_STATE_HOME outranks Omarchy's hard-coded $HOME path.
        write_theme(&xdg, 0x50_50_50);
        assert_eq!(
            poll(&source, &mut selection),
            (xdg.clone(), Some(0x50_50_50))
        );
        std::fs::remove_dir_all(root.join("xdg")).unwrap();
        for _ in 0..OMARCHY_ROOT_DEMOTION_GRACE_POLLS {
            let (selected, loaded) = poll(&source, &mut selection);
            assert_eq!(selected, xdg);
            assert!(loaded.is_none());
        }
        assert_eq!(
            poll(&source, &mut selection),
            (home_state.clone(), Some(0x40_40_40))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one bounded on-off-on scenario per fixture"
    )]
    fn glassy_windows_on_off_on_is_followed_live_through_in_place_edits() {
        // Each glassy fixture is the plain one after Omarchy's
        // omarchy-theme-set-glass (4a9c83de) with Glassy Windows on; turning
        // it off restores the plain file byte for byte.
        let glass_alpha = splinterm::config::parse("[colors]\nalpha=0.65\n")
            .unwrap()
            .config
            .background_alpha
            .unwrap();
        let theme_alpha = splinterm::config::parse("[colors]\nalpha=0.9\n")
            .unwrap()
            .config
            .background_alpha
            .unwrap();
        // (fixture, plain foot.ini, glassy foot.ini, plain alpha/blur, glassy alpha/blur)
        let cases = [
            (
                "stock-template",
                include_str!("../../tests/fixtures/omarchy4/glass/stock-template.foot.ini"),
                include_str!("../../tests/fixtures/omarchy4/glass/stock-template.glassy.foot.ini"),
                (u16::MAX, true),
                (glass_alpha, true),
            ),
            (
                "user-template",
                include_str!("../../tests/fixtures/omarchy4/glass/user-template.foot.ini"),
                include_str!("../../tests/fixtures/omarchy4/glass/user-template.glassy.foot.ini"),
                (u16::MAX, false),
                (glass_alpha, true),
            ),
            (
                "theme-alpha",
                include_str!("../../tests/fixtures/omarchy4/glass/theme-alpha.foot.ini"),
                include_str!("../../tests/fixtures/omarchy4/glass/theme-alpha.glassy.foot.ini"),
                (theme_alpha, false),
                (theme_alpha, true),
            ),
        ];
        let root =
            std::env::temp_dir().join(format!("splinterm-glass-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, plain, glassy, plain_effect, glassy_effect) in cases {
            let theme = root.join(name);
            std::fs::create_dir_all(&theme).unwrap();
            std::fs::write(
                theme.join("colors.toml"),
                include_str!("../../tests/fixtures/omarchy4/tokyo-night/colors.toml"),
            )
            .unwrap();
            std::fs::write(theme.join("foot.ini"), plain).unwrap();
            let source = ThemeSource::Omarchy(OmarchyThemeRoots::fixed(theme.clone()));
            let mut selection = OmarchyThemeSelection::default();
            let mut observed = fingerprint(&source);
            let mut current = live_update(&source, None, None, ResolvedTheme::default())
                .unwrap()
                .unwrap();
            assert_eq!(
                (current.background_alpha, current.background_blur),
                plain_effect,
                "{name}"
            );
            let palette = current;
            for (step, (contents, effect)) in [
                (glassy, glassy_effect),
                (plain, plain_effect),
                (glassy, glassy_effect),
            ]
            .into_iter()
            .enumerate()
            {
                let foot = theme.join("foot.ini");
                if step == 1 {
                    // Truncate and rewrite the same inode.
                    std::fs::write(&foot, contents).unwrap();
                } else {
                    // sed -i, as omarchy-theme-set-glass uses: a new inode.
                    let staged = theme.join("foot.ini.sed");
                    std::fs::write(&staged, contents).unwrap();
                    std::fs::rename(&staged, &foot).unwrap();
                }
                let next_fingerprint = fingerprint(&source);
                assert_ne!(next_fingerprint, observed, "{name} step {step}");
                observed = next_fingerprint;
                let active = active_theme_source(&source, &mut selection);
                let next = resolve_live_theme_update(
                    active,
                    &mut selection,
                    NO_OVERRIDES,
                    current,
                    &mut Vec::new(),
                )
                .unwrap()
                .expect("a glass toggle publishes a theme update");
                assert_eq!(
                    (next.background_alpha, next.background_blur),
                    effect,
                    "{name} step {step}"
                );
                assert_eq!(
                    next.with_color_overrides(
                        Some(palette.background_alpha),
                        Some(palette.background_blur)
                    ),
                    palette,
                    "{name} step {step}: only the background effect changes"
                );
                current = next;
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn omarchy_theme_switch_gap_never_flaps_to_a_stale_omarchy_3_root() {
        let root =
            std::env::temp_dir().join(format!("splinterm-omarchy-root-gap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let state_current = root.join("home/.local/state/omarchy/current");
        let home_state = state_current.join("theme");
        let legacy = root.join("home/.config/omarchy/current/theme");
        let source = ThemeSource::Omarchy(
            OmarchyThemeRoots::ordered(vec![home_state.clone(), legacy.clone()]).unwrap(),
        );
        let mut selection = OmarchyThemeSelection::default();
        write_theme(&legacy, 0x30_30_30);
        write_theme(&home_state, 0x40_40_40);
        assert_eq!(
            poll(&source, &mut selection),
            (home_state.clone(), Some(0x40_40_40))
        );

        // omarchy-theme-set stages current/next-theme, then runs
        // rm -rf current/theme; mv current/next-theme current/theme. The staged
        // theme keeps the root claimed for as long as the gap lasts, and each
        // read is rejected.
        write_theme(&state_current.join("next-theme"), 0x41_41_41);
        std::fs::remove_dir_all(&home_state).unwrap();
        for _ in 0..=OMARCHY_ROOT_DEMOTION_GRACE_POLLS * 2 {
            let (selected, loaded) = poll(&source, &mut selection);
            assert_eq!(selected, home_state);
            assert!(loaded.is_none());
        }
        std::fs::rename(state_current.join("next-theme"), &home_state).unwrap();
        assert_eq!(
            poll(&source, &mut selection),
            (home_state.clone(), Some(0x41_41_41))
        );

        // A whole-current replacement keeps the last good root through the
        // bounded grace window.
        let displaced = root.join("displaced-current");
        std::fs::rename(&state_current, &displaced).unwrap();
        for _ in 0..OMARCHY_ROOT_DEMOTION_GRACE_POLLS {
            let (selected, loaded) = poll(&source, &mut selection);
            assert_eq!(selected, home_state);
            assert!(loaded.is_none());
        }
        std::fs::rename(&displaced, &state_current).unwrap();
        assert_eq!(
            poll(&source, &mut selection),
            (home_state.clone(), Some(0x41_41_41))
        );

        // A sustained removal, such as a downgrade, is accepted after it.
        std::fs::rename(&state_current, &displaced).unwrap();
        for _ in 0..OMARCHY_ROOT_DEMOTION_GRACE_POLLS {
            assert_eq!(poll(&source, &mut selection).0, home_state);
        }
        assert_eq!(
            poll(&source, &mut selection),
            (legacy.clone(), Some(0x30_30_30))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_theme_reload_switches_dark_light_dark() {
        use std::fmt::Write as _;

        let root = std::env::temp_dir().join(format!(
            "splinterm-light-theme-reload-{}",
            std::process::id()
        ));
        let theme = root.join("theme");
        std::fs::create_dir_all(&theme).unwrap();
        let source = ThemeSource::Omarchy(OmarchyThemeRoots::fixed(theme.clone()));
        let mut current = ResolvedTheme::default();
        for (generation, mode, background, foreground) in [
            (0, "dark", 0x10_11_12, 0xe0_e1_e2),
            (1, "light", 0xf8_f9_fa, 0x20_21_22),
            (2, "dark", 0x10_11_12, 0xe0_e1_e2),
        ] {
            let before = fingerprint(&source);
            std::fs::rename(&theme, root.join(format!("previous-{generation}"))).unwrap();
            assert!(live_update(&source, None, None, current).is_err());
            std::fs::create_dir(&theme).unwrap();
            std::fs::write(theme.join("colors.toml"), "accent=\"#010203\"\n").unwrap();
            let mut foot = format!(
                "[main]\ninitial-color-theme={mode}\n[colors-{mode}]\nbackground={background:06x}\nforeground={foreground:06x}\nalpha=0.85\nblur=yes\n"
            );
            for index in 0..8 {
                write!(
                    foot,
                    "regular{index}={index:06x}\nbright{index}={:06x}\n",
                    index + 8
                )
                .unwrap();
            }
            std::fs::write(theme.join("foot.ini"), foot).unwrap();
            assert_ne!(fingerprint(&source), before);
            let next = live_update(&source, Some(4321), Some(false), current)
                .unwrap()
                .expect("theme switch must publish a palette");
            assert_eq!(next.background, background);
            assert_eq!(next.foreground, foreground);
            assert_eq!(next.background_alpha, 4321);
            assert!(!next.background_blur);
            assert_eq!(load_theme_source(&source).unwrap().background, background);
            assert!(
                live_update(&source, Some(4321), Some(false), next)
                    .unwrap()
                    .is_none()
            );
            current = next;
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_theme_fingerprint_tracks_omarchy_atomic_directory_replacement() {
        let root = std::env::temp_dir().join(format!(
            "splinterm-omarchy-theme-fingerprint-{}",
            std::process::id()
        ));
        let theme = root.join("theme");
        let previous = root.join("previous");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), b"accent = \"#010203\"\n").unwrap();
        std::fs::write(theme.join("foot.ini"), b"[colors-dark]\n").unwrap();
        let source = ThemeSource::Omarchy(OmarchyThemeRoots::fixed(theme.clone()));
        let first = fingerprint(&source);

        std::fs::rename(&theme, &previous).unwrap();
        assert!(live_update(&source, None, None, ResolvedTheme::default()).is_err());
        std::fs::create_dir(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), b"accent = \"#010203\"\n").unwrap();
        std::fs::write(theme.join("foot.ini"), b"[colors-dark]\n").unwrap();
        assert_ne!(fingerprint(&source), first);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn theme_fingerprint_changes_without_parsing_unchanged_content() {
        let path = std::env::temp_dir().join(format!(
            "splinterm-theme-fingerprint-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"one").unwrap();
        let first = theme_file_fingerprint(&path).unwrap();
        assert_eq!(theme_file_fingerprint(&path), Some(first));
        std::fs::write(&path, b"different-length").unwrap();
        assert_ne!(theme_file_fingerprint(&path), Some(first));
        std::fs::remove_file(path).unwrap();
    }

    fn valid_theme_json(alpha: f32, blur: bool) -> String {
        format!(
            r##"{{"background":"#000001","foreground":"#000003","cursor":"#000003","selection":"#000004","url":"#000005","ui_accent":"#000006","alpha":{alpha},"blur":{blur},"ansi":["#000000","#000001","#000002","#000003","#000004","#000005","#000006","#000007","#000008","#000009","#00000a","#00000b","#00000c","#00000d","#00000e","#00000f"]}}"##
        )
    }

    #[test]
    fn malformed_startup_and_live_themes_preserve_bounded_safe_state() {
        let path =
            std::env::temp_dir().join(format!("splinterm-theme-resolution-{}", std::process::id()));
        std::fs::write(&path, b"{}").unwrap();

        let mut config = AppConfig {
            theme_path: Some(path.clone()),
            background_alpha: Some(1234),
            background_blur: Some(true),
            // An explicit size and a JSON palette ignore every shell.toml
            // setting, so this host's own Omarchy files cannot add output.
            font_size_authority: splinterm::config::FontAuthority::Explicit,
            ..AppConfig::default()
        };
        for _launch_path in ["single-pane", "multi-pane"] {
            let mut startup_diagnostics = Vec::new();
            let startup = load_startup_theme_with_reporter(&config, |diagnostic| {
                startup_diagnostics.push(diagnostic.to_owned());
            })
            .theme;
            assert_eq!(startup_diagnostics.len(), 1);
            assert_eq!(startup.background_alpha, 1234);
            assert!(startup.background_blur);
        }

        std::fs::write(&path, valid_theme_json(0.25, true)).unwrap();
        config.background_alpha = None;
        config.background_blur = None;
        let current = load_startup_theme_with_reporter(&config, |_| {}).theme;
        assert_eq!(current.background, 1);
        assert_eq!(current.foreground, 3);
        assert!(current.background_blur);
        let preserved = current;

        std::fs::write(&path, b"{}").unwrap();
        let source = ThemeSource::Json {
            path: path.clone(),
            shell: None,
        };
        let error = live_update(&source, None, None, current).unwrap_err();
        assert_eq!(current, preserved);

        let mut diagnostics = ThemeReloadDiagnostics::default();
        assert!(diagnostics.rejected(&error).is_some());
        assert!(diagnostics.rejected(&error).is_none());
        diagnostics.accepted();
        assert!(diagnostics.rejected(&error).is_some());

        std::fs::write(&path, valid_theme_json(0.5, true)).unwrap();
        let next = live_update(&source, Some(2468), Some(false), current)
            .unwrap()
            .unwrap();
        assert_eq!(next.background_alpha, 2468);
        assert!(!next.background_blur);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn shell_toml_layers_are_fingerprinted_and_followed_live() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("theme");
        let user = root.join("home/.config/omarchy/shell.toml");
        write_theme(&theme, 0x10_10_10);
        let source = ThemeSource::Omarchy(
            OmarchyThemeRoots::fixed(theme.clone()).with_user_shell(user.clone()),
        );
        let mut selection = OmarchyThemeSelection::default();
        let mut current = ResolvedTheme::default();
        let mut observed = fingerprint(&source);
        let mut step = |edit: &dyn Fn(), expected: (u16, u16, Option<u16>), diagnostics: usize| {
            edit();
            let next_fingerprint = fingerprint(&source);
            assert_ne!(next_fingerprint, observed);
            observed = next_fingerprint;
            let mut reported = Vec::new();
            let active = active_theme_source(&source, &mut selection);
            if let Some(next) = resolve_live_theme_update(
                active,
                &mut selection,
                NO_OVERRIDES,
                current,
                &mut reported,
            )
            .unwrap()
            {
                current = next;
            }
            let shell = current.shell;
            assert_eq!(
                (shell.menu_alpha, shell.popup_alpha, shell.text_size_px),
                expected
            );
            assert_eq!(reported.len(), diagnostics, "{reported:?}");
        };
        let half = splinterm::config::parse("[colors]\nalpha=0.5\n")
            .unwrap()
            .config
            .background_alpha
            .unwrap();
        let rewrite = |path: &Path, contents: &str| {
            // omarchy-display-text-size writes a temporary file and renames it.
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let staged = path.with_extension("tmp");
            std::fs::write(&staged, contents).unwrap();
            std::fs::rename(&staged, path).unwrap();
        };
        step(
            &|| {
                rewrite(
                    &theme.join("shell.toml"),
                    "[menu]\nbackground-alpha = 0.5\n[font]\nbase-size = 12\n",
                );
            },
            (half, u16::MAX, Some(12)),
            0,
        );
        step(
            &|| rewrite(&user, "[font]\nbase-size = 16\n"),
            (half, u16::MAX, Some(16)),
            0,
        );
        step(
            &|| {
                rewrite(
                    &user,
                    "[font]\nbase-size = big\n[popups]\nbackground-alpha = 0.5\n",
                );
            },
            (half, half, Some(16)),
            1,
        );
        // omarchy-display-text-size reset drops the user key; the theme's
        // base-size applies again.
        step(&|| rewrite(&user, ""), (half, u16::MAX, Some(12)), 0);
        step(
            &|| std::fs::remove_file(theme.join("shell.toml")).unwrap(),
            (u16::MAX, u16::MAX, None),
            0,
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_shell_diagnostics_are_reported_once_until_they_change() {
        let mut diagnostics = ThemeReloadDiagnostics::default();
        let invalid = vec!["user/shell.toml: bad".to_owned()];
        assert_eq!(diagnostics.shell(invalid.clone()).len(), 1);
        assert_eq!(diagnostics.shell(invalid.clone()), Vec::<String>::new());
        assert_eq!(diagnostics.shell(Vec::new()), Vec::<String>::new());
        assert_eq!(diagnostics.shell(invalid).len(), 1);
    }

    #[test]
    fn startup_theme_layers_shell_settings_and_reports_rejections() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-startup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("theme");
        let user = root.join("user-shell.toml");
        write_theme(&theme, 0x20_20_20);
        std::fs::write(theme.join("shell.toml"), "[font]\nbase-size = 13\n").unwrap();
        std::fs::write(&user, "[menu]\nbackground-alpha = 2\n").unwrap();
        let source = ThemeSource::Omarchy(OmarchyThemeRoots::fixed(theme).with_user_shell(user));
        let loaded = prepare_startup_theme(&source, NO_OVERRIDES);
        assert_eq!(loaded.theme.background, 0x20_20_20);
        assert_eq!(loaded.theme.shell.text_size_px, Some(13));
        assert_eq!(loaded.theme.shell.menu_alpha, u16::MAX);
        assert_eq!(loaded.diagnostics.len(), 1);
        assert!(loaded.diagnostics[0].starts_with("splinterm theme: "));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_explicit_font_size_never_follows_the_omarchy_text_size() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-explicit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("theme");
        let user = root.join("user-shell.toml");
        write_theme(&theme, 0x30_30_30);
        std::fs::write(theme.join("shell.toml"), "[font]\nbase-size = 12\n").unwrap();
        std::fs::write(&user, "[font]\nbase-size = 16\n").unwrap();
        let source =
            ThemeSource::Omarchy(OmarchyThemeRoots::fixed(theme).with_user_shell(user.clone()));
        let explicit = ThemeOverrides::from_config(
            &splinterm::config::parse("[main]\nfont-pixelsize=15\n")
                .unwrap()
                .config,
        );
        assert!(!explicit.follow_text_size);
        let startup = prepare_startup_theme(&source, explicit).theme;
        assert_eq!(startup.shell.text_size_px, None);
        assert_eq!(
            prepare_startup_theme(&source, NO_OVERRIDES)
                .theme
                .shell
                .text_size_px,
            Some(16)
        );
        std::fs::write(&user, "[font]\nbase-size = 18\n").unwrap();
        let mut selection = OmarchyThemeSelection::default();
        let active = active_theme_source(&source, &mut selection);
        assert!(
            resolve_live_theme_update(active, &mut selection, explicit, startup, &mut Vec::new())
                .unwrap()
                .is_none(),
            "a base-size edit publishes nothing while config.ini owns the size"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_explicit_json_palette_follows_only_the_shell_text_size() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-json-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("theme");
        let user = root.join("user-shell.toml");
        let json = root.join("theme.json");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(&json, valid_theme_json(1.0, false)).unwrap();
        std::fs::write(
            theme.join("shell.toml"),
            "[menu]\nbackground-alpha = 0.5\n[font]\nbase-size = 12\n",
        )
        .unwrap();
        let source = ThemeSource::Json {
            path: json,
            shell: Some(OmarchyThemeRoots::fixed(theme).with_user_shell(user.clone())),
        };
        let startup = prepare_startup_theme(&source, NO_OVERRIDES).theme;
        assert_eq!(startup.background, 1);
        assert_eq!(
            startup.shell,
            OmarchyShellSettings {
                text_size_px: Some(12),
                ..OmarchyShellSettings::default()
            }
        );
        let before = fingerprint(&source);
        std::fs::write(&user, "[font]\nbase-size = 16\n").unwrap();
        assert_ne!(fingerprint(&source), before);
        let next = live_update(&source, None, None, startup)
            .unwrap()
            .expect("a text size edit publishes an update");
        assert_eq!(next.shell.text_size_px, Some(16));
        assert_eq!(next.shell.menu_alpha, u16::MAX);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_first_live_poll_does_not_repeat_startup_shell_diagnostics() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("theme");
        let user = root.join("user-shell.toml");
        write_theme(&theme, 0x40_40_40);
        std::fs::write(&user, "[menu]\nbackground-alpha = opaque\n").unwrap();
        let source =
            ThemeSource::Omarchy(OmarchyThemeRoots::fixed(theme).with_user_shell(user.clone()));
        let loaded = prepare_startup_theme(&source, NO_OVERRIDES);
        assert_eq!(loaded.diagnostics.len(), 1, "startup prints the problem");
        let startup = StartupTheme::from(loaded);
        let mut diagnostics = ThemeReloadDiagnostics::after_startup(&startup);

        // The watcher's first tick fires at once with nothing observed yet and
        // reloads the same files.
        let mut selection = OmarchyThemeSelection::default();
        let mut reported = Vec::new();
        let active = active_theme_source(&source, &mut selection);
        assert!(
            resolve_live_theme_update(
                active,
                &mut selection,
                NO_OVERRIDES,
                startup.theme,
                &mut reported
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(reported.len(), 1);
        assert_eq!(diagnostics.shell(reported), Vec::<String>::new());

        // A new problem is still reported.
        std::fs::write(&user, "[menu]\nbackground-alpha = never\n").unwrap();
        let mut reported = Vec::new();
        let active = active_theme_source(&source, &mut selection);
        resolve_live_theme_update(
            active,
            &mut selection,
            NO_OVERRIDES,
            startup.theme,
            &mut reported,
        )
        .unwrap();
        assert_eq!(diagnostics.shell(reported).len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostics_for_ignored_shell_settings_are_dropped() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-ignored-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("theme");
        let user = root.join("user-shell.toml");
        let json = root.join("theme.json");
        write_theme(&theme, 0x50_50_50);
        std::fs::write(&json, valid_theme_json(1.0, false)).unwrap();
        std::fs::write(
            &user,
            "[menu]\nbackground-alpha = opaque\n[font]\nbase-size = big\n",
        )
        .unwrap();
        let roots = OmarchyThemeRoots::fixed(theme).with_user_shell(user);
        let explicit = ThemeOverrides {
            follow_text_size: false,
            ..NO_OVERRIDES
        };
        let omarchy = ThemeSource::Omarchy(roots.clone());
        let json_source = ThemeSource::Json {
            path: json,
            shell: Some(roots),
        };
        for (source, overrides, expected) in [
            // Both settings are followed and both are reported.
            (&omarchy, NO_OVERRIDES, 2),
            // An explicit font size ignores the text size.
            (&omarchy, explicit, 1),
            // A JSON palette ignores panel alpha.
            (&json_source, NO_OVERRIDES, 1),
            // A JSON palette with an explicit size ignores both.
            (&json_source, explicit, 0),
        ] {
            let loaded = prepare_startup_theme(source, overrides);
            assert_eq!(loaded.shell_diagnostics.len(), expected, "{overrides:?}");
            let mut selection = OmarchyThemeSelection::default();
            let mut reported = Vec::new();
            let active = active_theme_source(source, &mut selection);
            resolve_live_theme_update(
                active,
                &mut selection,
                overrides,
                ResolvedTheme::default(),
                &mut reported,
            )
            .unwrap();
            assert_eq!(reported.len(), expected, "{overrides:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_json_palette_follows_shell_roots_with_the_native_demotion_grace() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-json-roots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let high = root.join("state/omarchy/current/theme");
        let low = root.join("legacy/omarchy/current/theme");
        let json = root.join("theme.json");
        for (theme, size) in [(&high, 16), (&low, 10)] {
            std::fs::create_dir_all(theme).unwrap();
            std::fs::write(
                theme.join("shell.toml"),
                format!("[font]\nbase-size = {size}\n"),
            )
            .unwrap();
        }
        std::fs::write(&json, valid_theme_json(1.0, false)).unwrap();
        let source = ThemeSource::Json {
            path: json,
            shell: OmarchyThemeRoots::ordered(vec![high.clone(), low]),
        };
        let mut selection = OmarchyThemeSelection::default();
        let mut current = ResolvedTheme::default();
        let mut poll = |current: &mut ResolvedTheme| {
            let active = active_theme_source(&source, &mut selection);
            if let Some(next) = resolve_live_theme_update(
                active,
                &mut selection,
                NO_OVERRIDES,
                *current,
                &mut Vec::new(),
            )
            .unwrap()
            {
                *current = next;
            }
            current.shell.text_size_px
        };
        assert_eq!(poll(&mut current), Some(16));

        // The higher root vanishes briefly: its size is kept through the grace
        // polls rather than snapping to the stale lower root.
        let displaced = root.join("displaced");
        std::fs::rename(&high, &displaced).unwrap();
        for _ in 0..OMARCHY_ROOT_DEMOTION_GRACE_POLLS {
            assert_eq!(poll(&mut current), Some(16));
        }
        std::fs::rename(&displaced, &high).unwrap();
        assert_eq!(poll(&mut current), Some(16));

        // A sustained removal is accepted after the grace polls.
        std::fs::rename(&high, &displaced).unwrap();
        for _ in 0..OMARCHY_ROOT_DEMOTION_GRACE_POLLS {
            assert_eq!(poll(&mut current), Some(16));
        }
        assert_eq!(poll(&mut current), Some(10));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_invalid_live_base_size_keeps_the_configured_fallback_in_use() {
        let root =
            std::env::temp_dir().join(format!("splinterm-shell-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("theme");
        let user = root.join("user-shell.toml");
        write_theme(&theme, 0x60_60_60);
        let source =
            ThemeSource::Omarchy(OmarchyThemeRoots::fixed(theme).with_user_shell(user.clone()));
        // No shell size anywhere: the window renders at Splinterm's fallback.
        let startup = prepare_startup_theme(&source, NO_OVERRIDES).theme;
        assert_eq!(startup.shell.text_size_px, None);

        // Adding an unparseable base-size live keeps that fallback rather than
        // switching to the startup-only 12 px default.
        std::fs::write(&user, "[font]\nbase-size = big\n").unwrap();
        let mut selection = OmarchyThemeSelection::default();
        let mut reported = Vec::new();
        let active = active_theme_source(&source, &mut selection);
        assert!(
            resolve_live_theme_update(active, &mut selection, NO_OVERRIDES, startup, &mut reported)
                .unwrap()
                .is_none()
        );
        assert_eq!(reported.len(), 1);

        // The same file at startup uses Omarchy's 12 px, as the shell does.
        assert_eq!(
            prepare_startup_theme(&source, NO_OVERRIDES)
                .theme
                .shell
                .text_size_px,
            Some(12)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
