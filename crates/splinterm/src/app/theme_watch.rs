use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::Result;
use splinterm::{
    ThemeUpdate, WindowTopologyUpdate, WindowUpdate,
    config::{
        AppConfig, OmarchyThemeSelection, ResolvedTheme, ThemeSource, load_live_omarchy_theme,
        load_omarchy_shell, load_theme, load_theme_source,
    },
};
use tokio::sync::mpsc;

static NEXT_THEME_UPDATE_GENERATION: AtomicU64 = AtomicU64::new(1);

type ThemeFileFingerprint = (u64, u64, u64, i64, i64);

/// The concrete files one poll reads. Omarchy sources pick one of their
/// ordered roots per poll, so a migration or root change is followed live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveThemeSource<'a> {
    Omarchy {
        index: usize,
        theme_dir: &'a Path,
        user_shell: Option<&'a Path>,
    },
    Json(&'a Path),
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
        ThemeSource::Json(path) => ActiveThemeSource::Json(path),
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
    fn of(theme_dir: &Path, user_shell: Option<&Path>) -> Self {
        Self {
            theme: theme_file_fingerprint(&theme_dir.join("shell.toml")),
            user: user_shell.and_then(theme_file_fingerprint),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ThemeSourceFingerprint {
    palette: PaletteFingerprint,
    shell: Option<ShellFingerprint>,
}

fn theme_source_fingerprint(source: ActiveThemeSource<'_>) -> ThemeSourceFingerprint {
    match source {
        ActiveThemeSource::Omarchy {
            theme_dir,
            user_shell,
            ..
        } => ThemeSourceFingerprint {
            palette: PaletteFingerprint::Omarchy {
                root: theme_dir.to_path_buf(),
                directory: theme_file_fingerprint(theme_dir),
                colors: theme_file_fingerprint(&theme_dir.join("colors.toml")),
                foot: theme_file_fingerprint(&theme_dir.join("foot.ini")),
            },
            shell: Some(ShellFingerprint::of(theme_dir, user_shell)),
        },
        ActiveThemeSource::Json(path) => ThemeSourceFingerprint {
            palette: PaletteFingerprint::Json(theme_file_fingerprint(path)),
            shell: None,
        },
    }
}

/// Layers the Omarchy shell settings for `theme_dir` onto a resolved palette.
fn with_omarchy_shell(
    mut theme: ResolvedTheme,
    theme_dir: &Path,
    user_shell: Option<&Path>,
    previous_text_size_px: Option<u8>,
    diagnostics: &mut Vec<String>,
) -> ResolvedTheme {
    let load = load_omarchy_shell(theme_dir, user_shell, previous_text_size_px);
    diagnostics.extend(load.diagnostics);
    theme.shell = load.settings;
    theme
}

struct StartupThemeLoad {
    theme: ResolvedTheme,
    diagnostics: Vec<String>,
}

fn prepare_startup_theme(
    source: &ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
) -> StartupThemeLoad {
    let mut diagnostics = Vec::new();
    let theme = load_theme_source(source).unwrap_or_else(|error| {
        diagnostics.push(format!("{error:#}; using safe fallback palette"));
        ResolvedTheme::default()
    });
    let theme = match source {
        ThemeSource::Omarchy(roots) => with_omarchy_shell(
            theme,
            roots.select(),
            roots.user_shell(),
            None,
            &mut diagnostics,
        ),
        ThemeSource::Json(_) => theme,
    };
    StartupThemeLoad {
        theme: theme.with_color_overrides(alpha_override, blur_override),
        diagnostics: diagnostics
            .into_iter()
            .map(|diagnostic| format!("splinterm theme: {diagnostic}"))
            .collect(),
    }
}

fn load_startup_theme_with_reporter(
    config: &AppConfig,
    mut report: impl FnMut(&str),
) -> ResolvedTheme {
    let loaded = prepare_startup_theme(
        &config.theme_source(),
        config.background_alpha,
        config.background_blur,
    );
    for diagnostic in &loaded.diagnostics {
        report(diagnostic);
    }
    loaded.theme
}

pub(in crate::app) fn load_startup_theme(config: &AppConfig) -> ResolvedTheme {
    load_startup_theme_with_reporter(config, |diagnostic| eprintln!("{diagnostic}"))
}

/// Loads the active source. A coherent Omarchy load records its root as the
/// last good one, which `OmarchyThemeSelection` holds through transient gaps.
/// Rejected shell settings are reported through `shell_diagnostics` and never
/// reject the palette.
fn resolve_live_theme_update(
    source: ActiveThemeSource<'_>,
    selection: &mut OmarchyThemeSelection,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
    current: ResolvedTheme,
    shell_diagnostics: &mut Vec<String>,
) -> Result<Option<ResolvedTheme>> {
    let next = match source {
        ActiveThemeSource::Omarchy {
            index,
            theme_dir,
            user_shell,
        } => {
            let theme = load_live_omarchy_theme(theme_dir)?;
            selection.accepted(index);
            with_omarchy_shell(
                theme,
                theme_dir,
                user_shell,
                current.shell.text_size_px,
                shell_diagnostics,
            )
        }
        ActiveThemeSource::Json(path) => load_theme(path)?,
    }
    .with_color_overrides(alpha_override, blur_override);
    Ok((next != current).then_some(next))
}

#[derive(Default)]
struct ThemeReloadDiagnostics {
    rejection_reported: bool,
    shell_reported: Vec<String>,
}

impl ThemeReloadDiagnostics {
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
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
    mut current: ResolvedTheme,
    sink: ThemeUpdateSink,
) {
    let mut observed = None;
    let mut selection = OmarchyThemeSelection::default();
    let mut diagnostics = ThemeReloadDiagnostics::default();
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
            alpha_override,
            blur_override,
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
    use splinterm::config::{OMARCHY_ROOT_DEMOTION_GRACE_POLLS, OmarchyThemeRoots};

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
            alpha_override,
            blur_override,
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
            None,
            None,
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
                    None,
                    None,
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
            ..AppConfig::default()
        };
        for _launch_path in ["single-pane", "multi-pane"] {
            let mut startup_diagnostics = Vec::new();
            let startup = load_startup_theme_with_reporter(&config, |diagnostic| {
                startup_diagnostics.push(diagnostic.to_owned());
            });
            assert_eq!(startup_diagnostics.len(), 1);
            assert_eq!(startup.background_alpha, 1234);
            assert!(startup.background_blur);
        }

        std::fs::write(&path, valid_theme_json(0.25, true)).unwrap();
        config.background_alpha = None;
        config.background_blur = None;
        let current = load_startup_theme_with_reporter(&config, |_| {});
        assert_eq!(current.background, 1);
        assert_eq!(current.foreground, 3);
        assert!(current.background_blur);
        let preserved = current;

        std::fs::write(&path, b"{}").unwrap();
        let source = ThemeSource::Json(path.clone());
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
        let mut step = |edit: &dyn Fn(), expected: (u16, u16, Option<u8>), diagnostics: usize| {
            edit();
            let next_fingerprint = fingerprint(&source);
            assert_ne!(next_fingerprint, observed);
            observed = next_fingerprint;
            let mut reported = Vec::new();
            let active = active_theme_source(&source, &mut selection);
            if let Some(next) = resolve_live_theme_update(
                active,
                &mut selection,
                None,
                None,
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
                    "[font]\nbase-size = 42\n[popups]\nbackground-alpha = 0.5\n",
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
        let loaded = prepare_startup_theme(&source, None, None);
        assert_eq!(loaded.theme.background, 0x20_20_20);
        assert_eq!(loaded.theme.shell.text_size_px, Some(13));
        assert_eq!(loaded.theme.shell.menu_alpha, u16::MAX);
        assert_eq!(loaded.diagnostics.len(), 1);
        assert!(loaded.diagnostics[0].starts_with("splinterm theme: "));
        std::fs::remove_dir_all(root).unwrap();
    }
}
