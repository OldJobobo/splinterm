//! One-time desktop hint recommending the opt-in Omarchy integration.
//!
//! Omarchy's Hyprland `terminal` tag excludes Splinterm's app ID, so Omarchy's
//! universal Super+C and Super+V send Ctrl+C and Ctrl+V to Splinterm instead of
//! copy and paste. `splinterm integration omarchy enable` adds the tag, but it
//! stays opt-in, so a graphical client under Omarchy says so once through a
//! desktop notification. It never runs the integration itself, never blocks
//! startup, and `[main] omarchy-integration-hint=no` suppresses it.

use std::{
    env,
    ffi::OsString,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use super::omarchy_integration::{Roots, active_lua, terminal_tag_ready};

/// Created once the hint has been handed to the notification service.
const MARKER: &str = "omarchy-hint-shown";
const DEFAULT_OMARCHY_PATH: &str = "/usr/share/omarchy";
const APP_ID: &str = "com.oldjobobo.splinterm";
const SUMMARY: &str = "Splinterm: enable the Omarchy integration";
const BODY: &str = "Omarchy does not tag Splinterm as a terminal yet, so Super+C and Super+V send \
Ctrl+C and Ctrl+V (interrupt) instead of copy and paste. Run `splinterm integration omarchy \
enable` once to fix it; `disable` reverses it. Shown once. Set [main] \
omarchy-integration-hint=no to silence it.";

/// The process environment the decision depends on, captured once.
#[derive(Clone, Debug, Default)]
struct Session {
    hyprland: bool,
    omarchy_path: Option<PathBuf>,
}

impl Session {
    fn from_values(hyprland_signature: Option<OsString>, omarchy_path: Option<OsString>) -> Self {
        Self {
            hyprland: hyprland_signature.is_some_and(|value| !value.is_empty()),
            omarchy_path: omarchy_path
                .map(PathBuf::from)
                .filter(|path| path.is_absolute()),
        }
    }

    fn from_env() -> Self {
        Self::from_values(
            env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
            env::var_os("OMARCHY_PATH"),
        )
    }

    fn omarchy_root(&self) -> PathBuf {
        self.omarchy_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_OMARCHY_PATH))
    }
}

/// Offer the hint for a local graphical launch. Every failure is silent: the
/// hint is advisory and must never delay or break opening a Window.
pub(super) fn offer() {
    let Ok(roots) = Roots::from_env() else {
        return;
    };
    let session = Session::from_env();
    if !hint_needed(&session, &roots) {
        return;
    }
    let marker = marker_path(&roots);
    if !matches!(claim_once(&marker), Ok(true)) {
        return;
    }
    if notify().is_err() {
        // Nothing reached the user, so a later launch may try again.
        let _ = fs::remove_file(&marker);
    }
}

/// Cheap, read-only detection: a Hyprland session running Omarchy whose
/// configuration does not give Splinterm the `terminal` tag.
fn hint_needed(session: &Session, roots: &Roots) -> bool {
    session.hyprland
        && (session.omarchy_path.is_some() || roots.hypr_entrypoint().is_file())
        && !terminal_tag_ready(roots)
        && !omarchy_tags_splinterm(&session.omarchy_root())
        && !marker_path(roots).exists()
}

/// Whether Omarchy's own terminal rule already names Splinterm, as a future
/// Omarchy release might.
fn omarchy_tags_splinterm(omarchy_root: &Path) -> bool {
    fs::read_to_string(omarchy_root.join("default/hypr/apps/terminals.lua"))
        .is_ok_and(|text| active_lua(&text).contains("splinterm"))
}

fn marker_path(roots: &Roots) -> PathBuf {
    roots.integration_state().join(MARKER)
}

/// Atomically claim the one showing; `false` when it was already claimed,
/// including by a concurrently starting Window.
fn claim_once(marker: &Path) -> io::Result<bool> {
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(marker) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

fn notification_command() -> Command {
    let mut command = Command::new("notify-send");
    command
        .arg("--app-name=Splinterm")
        .arg(format!("--icon={APP_ID}"))
        .arg("--urgency=normal")
        .arg("--")
        .arg(SUMMARY)
        .arg(BODY)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Hand the hint to the desktop notification service without waiting for it.
fn notify() -> io::Result<()> {
    let mut child = notification_command().spawn()?;
    // Reap the short-lived sender off the startup path.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> (Self, Roots) {
            let root = env::temp_dir().join(format!(
                "splinterm-omarchy-hint-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let roots =
                Roots::from_parts(root.join("home"), root.join("config"), root.join("state"));
            fs::create_dir_all(root.join("config/hypr")).unwrap();
            fs::create_dir_all(root.join("omarchy/default/hypr/apps")).unwrap();
            (Self(root), roots)
        }

        fn omarchy(&self) -> PathBuf {
            self.0.join("omarchy")
        }

        fn session(&self) -> Session {
            Session {
                hyprland: true,
                omarchy_path: Some(self.omarchy()),
            }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const OMARCHY_TERMINALS: &str = "o.window(\n  \"(Alacritty|kitty|foot|org\\\\.omarchy\\\\..*|TUI\\\\..*)\",\n  { tag = \"+terminal\" }\n)\n";

    #[test]
    fn session_requires_hyprland_and_an_absolute_omarchy_path() {
        let session = Session::from_values(Some("abc".into()), Some("/home/u/omarchy".into()));
        assert!(session.hyprland);
        assert_eq!(session.omarchy_root(), PathBuf::from("/home/u/omarchy"));
        let session = Session::from_values(Some("".into()), Some("relative/omarchy".into()));
        assert!(!session.hyprland);
        assert_eq!(session.omarchy_path, None);
        assert_eq!(session.omarchy_root(), PathBuf::from(DEFAULT_OMARCHY_PATH));
        assert_eq!(Session::from_values(None, None).omarchy_path, None);
    }

    #[test]
    fn untagged_omarchy_session_needs_the_hint() {
        let (root, roots) = TestRoot::new();
        fs::write(
            root.omarchy().join("default/hypr/apps/terminals.lua"),
            OMARCHY_TERMINALS,
        )
        .unwrap();
        assert!(hint_needed(&root.session(), &roots));
    }

    #[test]
    fn hyprland_lua_entrypoint_alone_detects_omarchy() {
        let (_root, roots) = TestRoot::new();
        let session = Session {
            hyprland: true,
            omarchy_path: None,
        };
        assert!(!hint_needed(&session, &roots));
        fs::write(roots.hypr_entrypoint(), "-- user config\n").unwrap();
        assert!(hint_needed(&session, &roots));
    }

    #[test]
    fn outside_hyprland_or_omarchy_no_hint_is_needed() {
        let (root, roots) = TestRoot::new();
        let mut session = root.session();
        session.hyprland = false;
        assert!(!hint_needed(&session, &roots));
        let session = Session::default();
        assert!(!hint_needed(&session, &roots));
    }

    #[test]
    fn existing_terminal_tag_suppresses_the_hint() {
        let (root, roots) = TestRoot::new();
        fs::write(
            roots.hypr_entrypoint().with_file_name("external.lua"),
            "hl.window_rule({\n  name = \"splinterm-terminal-tag\",\n  match = { initial_class = \"^com\\\\.oldjobobo\\\\.splinterm$\" },\n  tag = \"+terminal\",\n})\n",
        )
        .unwrap();
        assert!(!hint_needed(&root.session(), &roots));
    }

    #[test]
    fn omarchy_rule_naming_splinterm_suppresses_the_hint() {
        let (root, roots) = TestRoot::new();
        let terminals = root.omarchy().join("default/hypr/apps/terminals.lua");
        fs::write(
            &terminals,
            "-- com\\\\.oldjobobo\\\\.splinterm is not listed yet\n".to_owned() + OMARCHY_TERMINALS,
        )
        .unwrap();
        assert!(
            hint_needed(&root.session(), &roots),
            "a comment is not a rule"
        );
        fs::write(
            &terminals,
            "o.window(\"(foot|com\\\\.oldjobobo\\\\.splinterm)\", { tag = \"+terminal\" })\n",
        )
        .unwrap();
        assert!(!hint_needed(&root.session(), &roots));
    }

    #[test]
    fn the_hint_is_claimed_exactly_once() {
        let (root, roots) = TestRoot::new();
        let marker = marker_path(&roots);
        assert!(hint_needed(&root.session(), &roots));
        assert!(claim_once(&marker).unwrap());
        assert!(!claim_once(&marker).unwrap());
        assert!(!hint_needed(&root.session(), &roots));
        fs::remove_file(&marker).unwrap();
        assert!(hint_needed(&root.session(), &roots));
    }

    #[test]
    fn unwritable_state_is_an_error_not_a_claim() {
        let (_root, roots) = TestRoot::new();
        let state = roots.integration_state();
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, "not a directory").unwrap();
        assert!(claim_once(&marker_path(&roots)).is_err());
    }

    #[test]
    fn notification_names_the_command_and_the_suppression_key() {
        let command = notification_command();
        assert_eq!(command.get_program(), "notify-send");
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert!(arguments.contains(&"--app-name=Splinterm".to_owned()));
        assert!(arguments.contains(&format!("--icon={APP_ID}")));
        let body = arguments.last().unwrap();
        assert!(body.contains("splinterm integration omarchy enable"));
        assert!(body.contains("omarchy-integration-hint=no"));
        assert!(body.contains("Super+C"));
    }
}
