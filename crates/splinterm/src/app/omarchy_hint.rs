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
    thread::JoinHandle,
};

use super::omarchy_integration::{Roots, active_lua, terminal_tag_ready};

/// Created once the hint has been handed to the notification service.
const MARKER: &str = "omarchy-hint-shown";
const DEFAULT_OMARCHY_PATH: &str = "/usr/share/omarchy";
/// Omarchy's rule deciding which windows are terminals, relative to its root.
const OMARCHY_TERMINAL_RULE: &str = "default/hypr/apps/terminals.lua";
const APP_ID: &str = "com.oldjobobo.splinterm";
const SUMMARY: &str = "Splinterm: enable the Omarchy integration";
const BODY: &str = "Omarchy does not tag Splinterm as a terminal yet, so Super+C sends Ctrl+C \
(interrupt) and Super+V sends Ctrl+V instead of copying and pasting. `splinterm integration \
omarchy enable` fixes it (it also makes Splinterm the default and screensaver terminal); \
`disable` reverses it. Shown once. Set [main] omarchy-integration-hint=no to silence it.";

/// The process environment the decision depends on, captured once.
#[derive(Clone, Debug, Default)]
struct Session {
    hyprland: bool,
    /// A private or development daemon (`SPLINTERM_SOCKET`), as graphical test
    /// harnesses use with a fresh state directory per run.
    private_daemon: bool,
    omarchy_path: Option<PathBuf>,
}

impl Session {
    fn from_values(
        hyprland_signature: Option<OsString>,
        private_daemon: bool,
        omarchy_path: Option<OsString>,
    ) -> Self {
        Self {
            hyprland: hyprland_signature.is_some_and(|value| !value.is_empty()),
            private_daemon,
            omarchy_path: omarchy_path
                .map(PathBuf::from)
                .filter(|path| path.is_absolute()),
        }
    }

    fn from_env() -> Self {
        Self::from_values(
            env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
            env::var_os("SPLINTERM_SOCKET").is_some(),
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
    let session = Session::from_env();
    if !session.hyprland || session.private_daemon {
        return;
    }
    let Ok(roots) = Roots::from_env() else {
        return;
    };
    if !hint_needed(&session, &roots) {
        return;
    }
    let marker = marker_path(&roots);
    if !matches!(claim_once(&marker), Ok(true)) {
        return;
    }
    if send(notification_command(), marker.clone()).is_err() {
        // Nothing reached the user, so a later launch may try again.
        let _ = fs::remove_file(&marker);
    }
}

/// Cheap, read-only detection: a Hyprland session running Omarchy 4, whose
/// Lua configuration the integration can extend, that does not give Splinterm
/// the `terminal` tag. The cheapest checks come first.
fn hint_needed(session: &Session, roots: &Roots) -> bool {
    session.hyprland
        && !session.private_daemon
        && !marker_path(roots).exists()
        && roots.hypr_entrypoint().is_file()
        && omarchy_rule_omits_splinterm(&session.omarchy_root())
        && !terminal_tag_ready(roots)
}

/// Whether Omarchy's own terminal rule exists, which marks an Omarchy 4 root,
/// and does not already name Splinterm, as a future Omarchy release might.
fn omarchy_rule_omits_splinterm(omarchy_root: &Path) -> bool {
    fs::read_to_string(omarchy_root.join(OMARCHY_TERMINAL_RULE))
        .is_ok_and(|text| !active_lua(&text).contains("splinterm"))
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
/// The sender is reaped off the startup path; if it fails, for example because
/// no notification service is running, the claim is released so a later
/// launch may try again.
fn send(mut command: Command, marker: PathBuf) -> io::Result<JoinHandle<()>> {
    let mut child = command.spawn()?;
    Ok(std::thread::spawn(move || {
        if !child.wait().is_ok_and(|status| status.success()) {
            let _ = fs::remove_file(&marker);
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRoot(PathBuf);

    const OMARCHY_TERMINALS: &str = "o.window(\n  \"(Alacritty|kitty|foot|org\\\\.omarchy\\\\..*|TUI\\\\..*)\",\n  { tag = \"+terminal\" }\n)\n";

    impl TestRoot {
        /// An Omarchy 4 session: a Lua entrypoint and Omarchy's terminal rule.
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
            fs::write(roots.hypr_entrypoint(), "-- user config\n").unwrap();
            let test_root = Self(root);
            let rule = test_root.terminal_rule();
            fs::create_dir_all(rule.parent().unwrap()).unwrap();
            fs::write(rule, OMARCHY_TERMINALS).unwrap();
            (test_root, roots)
        }

        fn terminal_rule(&self) -> PathBuf {
            self.0.join("omarchy").join(OMARCHY_TERMINAL_RULE)
        }

        fn session(&self) -> Session {
            Session {
                hyprland: true,
                private_daemon: false,
                omarchy_path: Some(self.0.join("omarchy")),
            }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn session_reads_hyprland_private_daemon_and_absolute_omarchy_path() {
        let session =
            Session::from_values(Some("abc".into()), false, Some("/home/u/omarchy".into()));
        assert!(session.hyprland);
        assert!(!session.private_daemon);
        assert_eq!(session.omarchy_root(), PathBuf::from("/home/u/omarchy"));
        let session = Session::from_values(Some("".into()), true, Some("relative/omarchy".into()));
        assert!(!session.hyprland);
        assert!(session.private_daemon);
        assert_eq!(session.omarchy_path, None);
        assert_eq!(session.omarchy_root(), PathBuf::from(DEFAULT_OMARCHY_PATH));
        assert_eq!(Session::from_values(None, false, None).omarchy_path, None);
    }

    #[test]
    fn untagged_omarchy_4_session_needs_the_hint() {
        let (root, roots) = TestRoot::new();
        assert!(hint_needed(&root.session(), &roots));
    }

    #[test]
    fn plain_hyprland_lua_without_omarchy_gets_no_hint() {
        let (root, roots) = TestRoot::new();
        fs::remove_file(root.terminal_rule()).unwrap();
        assert!(!hint_needed(&root.session(), &roots));
    }

    #[test]
    fn omarchy_3_layout_without_a_lua_entrypoint_gets_no_hint() {
        // Omarchy 3.x sets OMARCHY_PATH but keeps hyprland.conf, which the
        // integration cannot extend.
        let (root, roots) = TestRoot::new();
        fs::remove_file(roots.hypr_entrypoint()).unwrap();
        fs::write(
            roots.hypr_entrypoint().with_file_name("hyprland.conf"),
            "# user config\n",
        )
        .unwrap();
        assert!(!hint_needed(&root.session(), &roots));
    }

    #[test]
    fn outside_hyprland_or_with_a_private_daemon_no_hint_is_needed() {
        let (root, roots) = TestRoot::new();
        let mut session = root.session();
        session.hyprland = false;
        assert!(!hint_needed(&session, &roots));
        let mut session = root.session();
        session.private_daemon = true;
        assert!(!hint_needed(&session, &roots));
        assert!(!hint_needed(&Session::default(), &roots));
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
        fs::write(
            root.terminal_rule(),
            "-- com\\\\.oldjobobo\\\\.splinterm is not listed yet\n".to_owned() + OMARCHY_TERMINALS,
        )
        .unwrap();
        assert!(
            hint_needed(&root.session(), &roots),
            "a comment is not a rule"
        );
        fs::write(
            root.terminal_rule(),
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
    fn a_failed_sender_releases_the_claim_and_a_successful_one_keeps_it() {
        let (_root, roots) = TestRoot::new();
        let marker = marker_path(&roots);
        for (program, kept) in [("false", false), ("true", true)] {
            assert!(claim_once(&marker).unwrap());
            send(Command::new(program), marker.clone())
                .unwrap()
                .join()
                .unwrap();
            assert_eq!(marker.exists(), kept, "{program}");
        }
        assert!(
            send(
                Command::new("/nonexistent/splinterm-notify-send"),
                marker.clone()
            )
            .is_err()
        );
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
        assert!(body.contains("Super+C sends Ctrl+C (interrupt) and Super+V sends Ctrl+V"));
        assert!(body.contains("`splinterm integration omarchy enable` fixes it"));
        assert!(body.contains("default and screensaver terminal"));
        assert!(body.contains("omarchy-integration-hint=no"));
    }
}
