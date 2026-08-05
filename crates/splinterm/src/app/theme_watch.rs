use std::{
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::Result;
use splinterm::{
    ThemeUpdate, WindowTopologyUpdate, WindowUpdate,
    config::{AppConfig, ResolvedTheme, ThemeSource, load_live_theme_source, load_theme_source},
};
use tokio::sync::mpsc;

static NEXT_THEME_UPDATE_GENERATION: AtomicU64 = AtomicU64::new(1);

type ThemeFileFingerprint = (u64, u64, u64, i64, i64);

fn theme_file_fingerprint(path: &std::path::Path) -> Option<ThemeFileFingerprint> {
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
        directory: Option<ThemeFileFingerprint>,
        colors: Option<ThemeFileFingerprint>,
        foot: Option<ThemeFileFingerprint>,
    },
    Json(Option<ThemeFileFingerprint>),
}

fn theme_source_fingerprint(source: &ThemeSource) -> ThemeSourceFingerprint {
    match source {
        ThemeSource::Omarchy(theme_dir) => ThemeSourceFingerprint::Omarchy {
            directory: theme_file_fingerprint(theme_dir),
            colors: theme_file_fingerprint(&theme_dir.join("colors.toml")),
            foot: theme_file_fingerprint(&theme_dir.join("foot.ini")),
        },
        ThemeSource::Json(path) => ThemeSourceFingerprint::Json(theme_file_fingerprint(path)),
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

fn resolve_live_theme_update(
    source: &ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
    current: ResolvedTheme,
) -> Result<Option<ResolvedTheme>> {
    let next = load_live_theme_source(source)?.with_color_overrides(alpha_override, blur_override);
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
    let mut diagnostics = ThemeReloadDiagnostics::default();
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(500));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        poll.tick().await;
        let next_fingerprint = theme_source_fingerprint(&source);
        if observed.as_ref() == Some(&next_fingerprint) {
            continue;
        }
        observed = Some(next_fingerprint);
        match resolve_live_theme_update(&source, alpha_override, blur_override, current) {
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
        let source = ThemeSource::Omarchy(theme.clone());
        let first = theme_source_fingerprint(&source);

        std::fs::rename(&theme, &previous).unwrap();
        assert!(resolve_live_theme_update(&source, None, None, ResolvedTheme::default()).is_err());
        std::fs::create_dir(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), b"accent = \"#010203\"\n").unwrap();
        std::fs::write(theme.join("foot.ini"), b"[colors-dark]\n").unwrap();
        assert_ne!(theme_source_fingerprint(&source), first);
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
        let error = resolve_live_theme_update(&source, None, None, current).unwrap_err();
        assert_eq!(current, preserved);

        let mut diagnostics = ThemeReloadDiagnostics::default();
        assert!(diagnostics.rejected(&error).is_some());
        assert!(diagnostics.rejected(&error).is_none());
        diagnostics.accepted();
        assert!(diagnostics.rejected(&error).is_some());

        std::fs::write(&path, valid_theme_json(0.5, true)).unwrap();
        let next = resolve_live_theme_update(&source, Some(2468), Some(false), current)
            .unwrap()
            .unwrap();
        assert_eq!(next.background_alpha, 2468);
        assert!(!next.background_blur);
        std::fs::remove_file(path).unwrap();
    }
}
