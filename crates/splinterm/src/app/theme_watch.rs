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
        load_theme, load_theme_source,
    },
};
use tokio::sync::mpsc;

static NEXT_THEME_UPDATE_GENERATION: AtomicU64 = AtomicU64::new(1);

type ThemeFileFingerprint = (u64, u64, u64, i64, i64);

/// The concrete files one poll reads. Omarchy sources pick one of their
/// ordered roots per poll, so a migration or root change is followed live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveThemeSource<'a> {
    Omarchy { index: usize, theme_dir: &'a Path },
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
enum ThemeSourceFingerprint {
    Omarchy {
        root: PathBuf,
        directory: Option<ThemeFileFingerprint>,
        colors: Option<ThemeFileFingerprint>,
        foot: Option<ThemeFileFingerprint>,
    },
    Json(Option<ThemeFileFingerprint>),
}

fn theme_source_fingerprint(source: ActiveThemeSource<'_>) -> ThemeSourceFingerprint {
    match source {
        ActiveThemeSource::Omarchy { theme_dir, .. } => ThemeSourceFingerprint::Omarchy {
            root: theme_dir.to_path_buf(),
            directory: theme_file_fingerprint(theme_dir),
            colors: theme_file_fingerprint(&theme_dir.join("colors.toml")),
            foot: theme_file_fingerprint(&theme_dir.join("foot.ini")),
        },
        ActiveThemeSource::Json(path) => ThemeSourceFingerprint::Json(theme_file_fingerprint(path)),
    }
}

fn resolve_configured_theme(
    source: &ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
) -> Result<ResolvedTheme> {
    load_theme_source(source).map(|theme| theme.with_color_overrides(alpha_override, blur_override))
}

struct StartupThemeLoad {
    theme: ResolvedTheme,
    diagnostic: Option<String>,
}

fn prepare_startup_theme(
    source: &ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
) -> StartupThemeLoad {
    match resolve_configured_theme(source, alpha_override, blur_override) {
        Ok(theme) => StartupThemeLoad {
            theme,
            diagnostic: None,
        },
        Err(error) => StartupThemeLoad {
            theme: ResolvedTheme::default().with_color_overrides(alpha_override, blur_override),
            diagnostic: Some(format!(
                "splinterm theme: {error:#}; using safe fallback palette"
            )),
        },
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
    if let Some(diagnostic) = loaded.diagnostic {
        report(&diagnostic);
    }
    loaded.theme
}

pub(in crate::app) fn load_startup_theme(config: &AppConfig) -> ResolvedTheme {
    load_startup_theme_with_reporter(config, |diagnostic| eprintln!("{diagnostic}"))
}

/// Loads the active source. A coherent Omarchy load records its root as the
/// last good one, which `OmarchyThemeSelection` holds through transient gaps.
fn resolve_live_theme_update(
    source: ActiveThemeSource<'_>,
    selection: &mut OmarchyThemeSelection,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
    current: ResolvedTheme,
) -> Result<Option<ResolvedTheme>> {
    let next = match source {
        ActiveThemeSource::Omarchy { index, theme_dir } => {
            let theme = load_live_omarchy_theme(theme_dir)?;
            selection.accepted(index);
            theme
        }
        ActiveThemeSource::Json(path) => load_theme(path)?,
    }
    .with_color_overrides(alpha_override, blur_override);
    Ok((next != current).then_some(next))
}

#[derive(Default)]
struct ThemeReloadDiagnostics {
    rejection_reported: bool,
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
        match resolve_live_theme_update(
            active,
            &mut selection,
            alpha_override,
            blur_override,
            current,
        ) {
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
        let loaded =
            resolve_live_theme_update(active, selection, None, None, ResolvedTheme::default())
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

        // omarchy-theme-set: rm -rf current/theme; mv current/next-theme
        // current/theme. current/ stays, so the claimed root is kept for as
        // long as the gap lasts and each read is rejected.
        std::fs::remove_dir_all(&home_state).unwrap();
        for _ in 0..=OMARCHY_ROOT_DEMOTION_GRACE_POLLS * 2 {
            let (selected, loaded) = poll(&source, &mut selection);
            assert_eq!(selected, home_state);
            assert!(loaded.is_none());
        }
        write_theme(&state_current.join("next-theme"), 0x41_41_41);
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
}
