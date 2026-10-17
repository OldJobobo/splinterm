use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use splinterm_pty::PtyCommand;
use tracing::debug;

const MAX_GUM_ENVIRONMENT_BYTES: u64 = 64 * 1024;
const GENERIC_GUM_VARIABLES: [&str; 4] = [
    "FOREGROUND",
    "BACKGROUND",
    "BORDER_FOREGROUND",
    "BORDER_BACKGROUND",
];

pub(crate) async fn refresh_gum_environment(mut command: PtyCommand) -> PtyCommand {
    let inherited_names = env::vars_os().map(|(name, _)| name).collect::<Vec<_>>();
    // Choosing the root probes the filesystem, so it runs off the executor too.
    let updates = tokio::task::spawn_blocking(move || {
        let path = default_gum_environment_path();
        let updates = gum_environment_updates(&path, inherited_names);
        (path, updates)
    })
    .await;
    let updates = match updates {
        Ok((_, Ok(updates))) => updates,
        Ok((diagnostic_path, Err(error))) => {
            debug!(%error, path = %diagnostic_path.display(), "active Omarchy Gum environment unavailable; preserving daemon environment");
            return command;
        }
        Err(error) => {
            debug!(%error, "Omarchy Gum environment task failed; preserving daemon environment");
            return command;
        }
    };

    for (name, value) in updates {
        command = if let Some(value) = value {
            command.env(name, value)
        } else {
            command.env_remove(name)
        };
    }
    command
}

fn default_gum_environment_path() -> PathBuf {
    gum_environment_path(env::var_os("XDG_STATE_HOME"), env::var_os("HOME"))
}

/// The active Omarchy theme directories, highest priority first, matching the
/// client's `OmarchyThemeRoots`: `${XDG_STATE_HOME}`, Omarchy 4's hard-coded
/// `$HOME/.local/state` when that differs, then Omarchy 3.x `$HOME/.config`.
fn omarchy_theme_candidates(
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
) -> Vec<PathBuf> {
    let home = home
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let home_state = home.join(".local/state/omarchy/current/theme");
    let mut candidates = Vec::with_capacity(3);
    if let Some(xdg_state) = xdg_state_home
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("omarchy/current/theme"))
        .filter(|path| *path != home_state)
    {
        candidates.push(xdg_state);
    }
    candidates.push(home_state);
    candidates.push(home.join(".config/omarchy/current/theme"));
    candidates
}

/// Reads from the first root whose theme, or the theme Omarchy is staging as
/// `current/next-theme`, exists. One of the two exists throughout a switch,
/// so a spawn inside that gap fails to read and keeps the daemon environment
/// instead of falling through to a stale Omarchy 3.x palette.
fn gum_environment_path(xdg_state_home: Option<OsString>, home: Option<OsString>) -> PathBuf {
    let candidates = omarchy_theme_candidates(xdg_state_home, home);
    let selected = candidates
        .iter()
        .position(|theme_dir| theme_dir.is_dir() || theme_dir.with_file_name("next-theme").is_dir())
        .unwrap_or(0);
    candidates[selected].join("gum_env.lua")
}

fn gum_environment_updates(
    path: &Path,
    inherited_names: impl IntoIterator<Item = OsString>,
) -> Result<BTreeMap<OsString, Option<OsString>>> {
    let raw = read_bounded_regular_file(path)?;
    let palette = parse_gum_environment(&raw)?;
    let mut updates = inherited_names
        .into_iter()
        .filter(|name| is_managed_gum_variable(name.to_string_lossy().as_ref()))
        .map(|name| (name, None))
        .collect::<BTreeMap<_, _>>();
    updates.extend(
        palette
            .into_iter()
            .map(|(name, value)| (OsString::from(name), Some(OsString::from(value)))),
    );
    Ok(updates)
}

fn read_bounded_regular_file(path: &Path) -> Result<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("active Omarchy Gum environment is not a regular file");
    }
    if metadata.len() > MAX_GUM_ENVIRONMENT_BYTES {
        bail!("active Omarchy Gum environment exceeds the byte limit");
    }

    let mut raw = String::new();
    file.take(MAX_GUM_ENVIRONMENT_BYTES + 1)
        .read_to_string(&mut raw)
        .with_context(|| format!("read {}", path.display()))?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > MAX_GUM_ENVIRONMENT_BYTES {
        bail!("active Omarchy Gum environment exceeds the byte limit");
    }
    Ok(raw)
}

fn parse_gum_environment(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut palette = BTreeMap::new();
    for (line_number, raw_line) in raw.lines().enumerate() {
        let line = raw_line.trim();
        let Some(arguments) = line.strip_prefix("hl.env(") else {
            continue;
        };
        let arguments = arguments
            .strip_suffix(')')
            .with_context(|| format!("invalid hl.env call on line {}", line_number + 1))?;
        let (name, value) = arguments
            .split_once(',')
            .with_context(|| format!("invalid hl.env call on line {}", line_number + 1))?;
        let name = quoted_literal(name.trim())
            .with_context(|| format!("invalid environment name on line {}", line_number + 1))?;
        let value = quoted_literal(value.trim())
            .with_context(|| format!("invalid environment value on line {}", line_number + 1))?;
        if !is_managed_gum_variable(name) {
            continue;
        }
        if !is_hex_color(value) {
            bail!("invalid Gum color on line {}", line_number + 1);
        }
        if palette.insert(name.to_owned(), value.to_owned()).is_some() {
            bail!("duplicate Gum variable {name}");
        }
    }

    for name in GENERIC_GUM_VARIABLES {
        if !palette.contains_key(name) {
            bail!("active Omarchy Gum environment is missing {name}");
        }
    }
    if !palette.keys().any(|name| name.starts_with("GUM_")) {
        bail!("active Omarchy Gum environment has no GUM_* variables");
    }
    Ok(palette)
}

fn quoted_literal(value: &str) -> Option<&str> {
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    (!value.contains(['"', '\\'])).then_some(value)
}

fn is_managed_gum_variable(name: &str) -> bool {
    GENERIC_GUM_VARIABLES.contains(&name)
        || name.strip_prefix("GUM_").is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

fn is_hex_color(value: &str) -> bool {
    matches!(value.len(), 7 | 9)
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_palette(contents: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "splinterd-gum-environment-{}-{nonce}.lua",
            std::process::id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    const PALETTE: &str = r##"
-- unrelated Lua is ignored
hl.env("FOREGROUND", "#afaaa2")
hl.env("BACKGROUND", "#0c1928")
hl.env("BORDER_FOREGROUND", "#d4bda2")
hl.env("BORDER_BACKGROUND", "#0c1928")
hl.env("GUM_CHOOSE_SELECTED_BACKGROUND", "#d4bda2")
hl.env("NOT_SPLINTERM_OWNED", "#ffffff")
"##;

    #[test]
    fn empty_xdg_state_home_uses_the_home_fallback() {
        let expected = PathBuf::from("/home/test/.local/state/omarchy/current/theme/gum_env.lua");
        assert_eq!(
            gum_environment_path(Some(OsString::new()), Some(OsString::from("/home/test"))),
            expected
        );
        assert_eq!(
            gum_environment_path(None, Some(OsString::from("/home/test"))),
            expected
        );
    }

    #[test]
    fn omarchy_gum_environment_follows_xdg_then_home_state_then_omarchy_3() {
        let home = || Some(OsString::from("/home/test"));
        let state = PathBuf::from("/home/test/.local/state/omarchy/current/theme");
        let legacy = PathBuf::from("/home/test/.config/omarchy/current/theme");
        for xdg in [None, Some(""), Some("/home/test/.local/state")] {
            assert_eq!(
                omarchy_theme_candidates(xdg.map(OsString::from), home()),
                [state.clone(), legacy.clone()],
                "{xdg:?}"
            );
        }
        assert_eq!(
            omarchy_theme_candidates(Some(OsString::from("/srv/state")), home()),
            [
                PathBuf::from("/srv/state/omarchy/current/theme"),
                state,
                legacy
            ]
        );

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "splinterd-omarchy-roots-{}-{nonce}",
            std::process::id()
        ));
        let xdg = root.join("xdg");
        let home = root.join("home");
        let lookup = || {
            gum_environment_path(
                Some(xdg.clone().into_os_string()),
                Some(home.clone().into_os_string()),
            )
        };
        let legacy = home.join(".config/omarchy/current/theme");
        let state_current = home.join(".local/state/omarchy/current");
        assert_eq!(
            lookup(),
            xdg.join("omarchy/current/theme/gum_env.lua"),
            "nothing claimed keeps the first root"
        );
        fs::create_dir_all(&legacy).unwrap();
        assert_eq!(lookup(), legacy.join("gum_env.lua"));
        // An empty current claims nothing; the theme staged as
        // current/next-theme while current/theme is replaced does, so there is
        // no flap to the Omarchy 3.x leftover.
        fs::create_dir_all(&state_current).unwrap();
        assert_eq!(lookup(), legacy.join("gum_env.lua"));
        fs::create_dir_all(state_current.join("next-theme")).unwrap();
        assert_eq!(lookup(), state_current.join("theme/gum_env.lua"));
        // A stale empty current in a higher root never shadows the live theme.
        fs::create_dir_all(xdg.join("omarchy/current")).unwrap();
        assert_eq!(lookup(), state_current.join("theme/gum_env.lua"));
        fs::create_dir_all(xdg.join("omarchy/current/theme")).unwrap();
        assert_eq!(lookup(), xdg.join("omarchy/current/theme/gum_env.lua"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_palette_is_rejected_before_parsing() {
        let oversized_length = usize::try_from(MAX_GUM_ENVIRONMENT_BYTES).unwrap() + 1;
        let path = write_palette(&"x".repeat(oversized_length));
        assert!(read_bounded_regular_file(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn current_palette_replaces_stale_managed_values_only() {
        let path = write_palette(PALETTE);
        let updates = gum_environment_updates(
            &path,
            [
                OsString::from("GUM_OLD_THEME_ONLY"),
                OsString::from("BACKGROUND"),
                OsString::from("UNRELATED"),
            ],
        )
        .unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(
            updates.get(&OsString::from("GUM_OLD_THEME_ONLY")),
            Some(&None)
        );
        assert_eq!(
            updates.get(&OsString::from("BACKGROUND")),
            Some(&Some(OsString::from("#0c1928")))
        );
        assert_eq!(
            updates.get(&OsString::from("GUM_CHOOSE_SELECTED_BACKGROUND")),
            Some(&Some(OsString::from("#d4bda2")))
        );
        assert!(!updates.contains_key(&OsString::from("UNRELATED")));
        assert!(!updates.contains_key(&OsString::from("NOT_SPLINTERM_OWNED")));
    }

    #[test]
    fn malformed_or_incomplete_palette_is_rejected() {
        assert!(parse_gum_environment("hl.env(\"BACKGROUND\", \"old\")").is_err());
        assert!(
            parse_gum_environment(
                "hl.env(\"FOREGROUND\", \"#ffffff\")\n\
                 hl.env(\"BACKGROUND\", \"#000000\")\n\
                 hl.env(\"BORDER_FOREGROUND\", \"#ffffff\")\n\
                 hl.env(\"BORDER_BACKGROUND\", \"#000000\")"
            )
            .is_err()
        );
        assert!(
            parse_gum_environment(&format!(
                "{PALETTE}\nhl.env(\"GUM_FILTER_INDICATOR\", \"#ffffff\""
            ))
            .is_err()
        );
    }

    #[test]
    fn managed_namespace_is_bounded() {
        assert!(is_managed_gum_variable("FOREGROUND"));
        assert!(is_managed_gum_variable("GUM_FILTER_SELECTED_BACKGROUND"));
        assert!(!is_managed_gum_variable("GUM_"));
        assert!(!is_managed_gum_variable("GUM_bad"));
        assert!(!is_managed_gum_variable("PATH"));
    }
}
