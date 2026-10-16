//! Detached self-launches for opening remote Windows from the graphical client.
//!
//! A remote Window runs in its own `splinterm --remote PROFILE` process so the
//! one-endpoint-per-Window rule holds. These launches inherit no stdio and, via
//! [`DETACH_TERMINAL_FLAG`], start a new session without a controlling terminal,
//! so OpenSSH never prompts on whatever terminal started the parent client.

use std::{
    ffi::OsString,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
};

use anyhow::{Context, Result};

/// Hidden client flag that makes the launched process call `setsid` first.
pub const DETACH_TERMINAL_FLAG: &str = "--detach-terminal";

/// Builds `splinterm --detach-terminal --remote PROFILE`, the remote Window launch.
#[must_use]
pub fn remote_window_command(executable: &Path, profile: &str) -> Command {
    detached_command(
        executable,
        [
            OsString::from(DETACH_TERMINAL_FLAG),
            OsString::from("--remote"),
            OsString::from(profile),
        ],
    )
}

/// Builds `splinterm --detach-terminal remotes`, the standalone remote chooser.
#[must_use]
pub fn remote_chooser_command(executable: &Path) -> Command {
    detached_command(
        executable,
        [
            OsString::from(DETACH_TERMINAL_FLAG),
            OsString::from("remotes"),
        ],
    )
}

/// Do not add `process_group(0)` here: it would make the child a process-group
/// leader, and the `setsid` behind [`DETACH_TERMINAL_FLAG`] would then fail.
fn detached_command(executable: &Path, arguments: impl IntoIterator<Item = OsString>) -> Command {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Opens one configured remote in a new Window process.
///
/// The launched client maps a connection-status Window at once, shows
/// categorized failures with Retry / Close, and hands over to the remote's
/// Window when connected; this call returns as soon as the process starts.
///
/// # Errors
///
/// Returns an error when the running client executable cannot be located or
/// the detached process cannot be started.
pub fn open_remote_window(profile: &str) -> Result<()> {
    let executable = std::env::current_exe().context("locate the running Splinterm client")?;
    spawn_detached(remote_window_command(&executable, profile))
        .with_context(|| format!("open remote profile {profile:?} in a new Window"))
}

/// Opens the standalone remote chooser Window.
///
/// # Errors
///
/// Returns an error when the running client executable cannot be located or
/// the detached process cannot be started.
pub fn open_remote_chooser() -> Result<()> {
    let executable = std::env::current_exe().context("locate the running Splinterm client")?;
    spawn_detached(remote_chooser_command(&executable)).context("open the remote chooser")
}

/// Starts `command` and reaps it on a background thread, so a long-lived parent
/// never accumulates zombie children.
///
/// # Errors
///
/// Returns an error when the process cannot be started.
pub fn spawn_detached(mut command: Command) -> Result<()> {
    let child = command
        .spawn()
        .context("start a detached Splinterm process")?;
    reap_in_background(child);
    Ok(())
}

fn reap_in_background(mut child: Child) {
    // If no reaper thread can start, the child is reaped when this client exits.
    let _ = thread::Builder::new()
        .name("splinterm-launch-reaper".to_owned())
        .spawn(move || {
            let _ = child.wait();
        });
}

/// Starts a new session so this process has no controlling terminal.
///
/// Called first by a client started with [`DETACH_TERMINAL_FLAG`]. A failure
/// (for example, already a session or group leader) leaves the process as it
/// was, which is no worse than not detaching.
pub fn detach_from_controlling_terminal() {
    let _ = rustix::process::setsid();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        time::{Duration, Instant},
    };

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn remote_launches_run_the_client_detached_with_exact_argv() {
        let executable = Path::new("/opt/splinterm/bin/splinterm");
        let window = remote_window_command(executable, "wintermute");
        assert_eq!(window.get_program(), executable);
        assert_eq!(
            arguments(&window),
            ["--detach-terminal", "--remote", "wintermute"]
        );
        assert_eq!(window.get_envs().count(), 0, "environment is inherited");
        assert_eq!(window.get_current_dir(), None);

        // A profile name is one argv entry and is never shell-interpreted.
        let odd = remote_window_command(executable, "a b;$(x)");
        assert_eq!(
            arguments(&odd),
            ["--detach-terminal", "--remote", "a b;$(x)"]
        );

        let chooser = remote_chooser_command(executable);
        assert_eq!(chooser.get_program(), executable);
        assert_eq!(arguments(&chooser), ["--detach-terminal", "remotes"]);
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "splinterm-remote-launch-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn detached_launch_inherits_no_stdio() {
        let directory = scratch_dir("stdio");
        let record = directory.join("record");
        let double = directory.join("splinterm");
        // Resolve the inherited descriptors before redirecting the record write.
        let shown = record.display();
        fs::write(
            &double,
            format!(
                "#!/bin/sh\nstdio=$(for fd in 0 1 2; do readlink /proc/$$/fd/$fd; done)\n\
                 {{ printf '%s\\n' \"$@\"; printf '%s\\n' \"$stdio\"; }} > '{shown}.tmp'\n\
                 mv '{shown}.tmp' '{shown}'\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&double, fs::Permissions::from_mode(0o755)).unwrap();

        // Another test thread forking while the double was open for writing can
        // briefly make it busy; retry that transient case only.
        let mut attempts = 0;
        while let Err(error) = spawn_detached(remote_window_command(&double, "wintermute")) {
            let busy = error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::ExecutableFileBusy);
            attempts += 1;
            assert!(busy && attempts < 50, "{error:#}");
            thread::sleep(Duration::from_millis(20));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !record.exists() {
            assert!(Instant::now() < deadline, "test double did not run");
            thread::sleep(Duration::from_millis(10));
        }
        let lines = fs::read_to_string(&record).unwrap();
        let lines = lines.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            [
                "--detach-terminal",
                "--remote",
                "wintermute",
                "/dev/null",
                "/dev/null",
                "/dev/null"
            ]
        );
        let _ = fs::remove_dir_all(directory);
    }
}
