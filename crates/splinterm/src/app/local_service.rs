use std::{
    env,
    io::{self, IsTerminal, Write},
    os::unix::{fs::FileTypeExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use splinterm::automation::socket_path as configured_socket_path;
use splinterm_core::SplintId;

use super::commands::{Cli, PolicyCommand};

pub(in crate::app) fn confirm_kill(splint_id: SplintId) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!(
            "refusing to kill {splint_id} without an interactive terminal; pass --yes to confirm"
        );
    }
    eprint!("Kill Splint {splint_id} and its live process? [y/N] ");
    io::stderr()
        .flush()
        .context("failed to display confirmation")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn confirm_reset() -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!(
            "refusing to reset every session without an interactive terminal; pass --yes to confirm"
        );
    }
    eprint!("Stop every Splint, clear all sessions, and restart splinterd? [y/N] ");
    io::stderr()
        .flush()
        .context("failed to display confirmation")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

// The client needs concurrent local IPC and theme watching, not one allocator
// arena per CPU. Wayland rendering already runs on a bounded blocking worker.
pub(in crate::app) fn usage_error(message: &str) -> ! {
    Cli::command()
        .error(clap::error::ErrorKind::ArgumentConflict, message)
        .exit()
}

pub(in crate::app) fn run_relay_command(stdio: bool, graphical_stdio: bool) -> Result<()> {
    let relay_argument = match (stdio, graphical_stdio) {
        (true, false) => "--stdio",
        (false, true) => "--graphical-stdio",
        _ => bail!("relay requires exactly one stdio transport mode"),
    };
    let current = env::current_exe().context("cannot resolve the splinterm executable")?;
    let relay = current.with_file_name("splinterm-relay");
    let error = ProcessCommand::new(&relay).arg(relay_argument).exec();
    Err(error).with_context(|| format!("failed to execute {}", relay.display()))
}

pub(in crate::app) fn run_policy_command(command: PolicyCommand) -> Result<()> {
    match command {
        PolicyCommand::Validate { path } => {
            let (rule_count, _) = splinterd::inspect_policy_file(&path)
                .with_context(|| format!("policy validation failed for {}", path.display()))?;
            println!("valid splinterm.policy.v2 policy ({rule_count} rules)");
        }
        PolicyCommand::Inspect { path } => {
            let (_, document) = splinterd::inspect_policy_file(&path)
                .with_context(|| format!("policy inspection failed for {}", path.display()))?;
            serde_json::to_writer_pretty(io::stdout().lock(), &document)
                .context("failed to write validated policy")?;
            println!();
        }
        PolicyCommand::Reload => {
            let status = ProcessCommand::new("systemctl")
                .args(["--user", "reload", "splinterd.service"])
                .status()
                .context("failed to invoke systemctl --user reload splinterd.service")?;
            if !status.success() {
                bail!("systemctl --user reload splinterd.service failed with {status}");
            }
            println!(
                "policy reload requested; inspect daemon logs or bounded audit metadata for acceptance"
            );
        }
    }
    Ok(())
}

fn splinterm_state_directory() -> Result<PathBuf> {
    let base = match env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env::var_os("HOME").context("XDG_STATE_HOME and HOME are unset")?)
            .join(".local/state"),
    };
    if !base.is_absolute() {
        bail!("state directory base must be absolute");
    }
    Ok(base.join("splinterm"))
}

fn move_session_state_no_replace(state: &Path, backup: &Path) -> Result<bool> {
    match rustix::fs::renameat_with(
        rustix::fs::CWD,
        state,
        rustix::fs::CWD,
        backup,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::EXIST) => Ok(false),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to move session state {} to {}",
                state.display(),
                backup.display()
            )
        }),
    }
}

fn backup_session_state(state: &Path) -> Result<Option<PathBuf>> {
    let metadata = match std::fs::symlink_metadata(state) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        bail!("session state path is not a directory: {}", state.display());
    }
    let parent = state.parent().context("session state path has no parent")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis();
    for suffix in 0_u8..100 {
        let name = if suffix == 0 {
            format!("splinterm.reset-{stamp}")
        } else {
            format!("splinterm.reset-{stamp}-{suffix}")
        };
        let backup = parent.join(name);
        if move_session_state_no_replace(state, &backup)? {
            return Ok(Some(backup));
        }
    }
    bail!("could not allocate a unique session backup path");
}

fn run_user_systemctl(action: &str) -> Result<()> {
    let status = ProcessCommand::new("systemctl")
        .args(["--user", action, "splinterd.service"])
        .status()
        .with_context(|| format!("failed to invoke systemctl --user {action} splinterd.service"))?;
    if !status.success() {
        bail!("systemctl --user {action} splinterd.service failed with {status}");
    }
    Ok(())
}

fn wait_for_splinterd_socket(socket: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::fs::symlink_metadata(socket).is_ok_and(|metadata| metadata.file_type().is_socket())
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!(
        "splinterd restarted but did not create {} within 5 seconds; inspect `systemctl --user status splinterd.service`",
        socket.display()
    );
}

fn reset_readiness_context(backup: Option<&Path>) -> String {
    match backup {
        Some(path) => format!(
            "sessions were reset and backed up to {}, but splinterd did not become ready",
            path.display()
        ),
        None => "session state was empty, but splinterd did not become ready".to_owned(),
    }
}

pub(in crate::app) fn run_reset_command(yes: bool) -> Result<()> {
    if !yes && !confirm_reset()? {
        println!("Reset cancelled.");
        return Ok(());
    }

    let state = splinterm_state_directory()?;
    let socket = configured_socket_path()?;
    run_user_systemctl("stop")?;

    let backup = match backup_session_state(&state) {
        Ok(backup) => backup,
        Err(error) => {
            if let Err(restart) = run_user_systemctl("start") {
                return Err(error).context(format!(
                    "session reset failed, then splinterd restart also failed: {restart:#}"
                ));
            }
            if let Err(readiness) = wait_for_splinterd_socket(&socket) {
                return Err(error).context(format!(
                    "session reset failed; splinterd restart also did not become ready: {readiness:#}"
                ));
            }
            return Err(error).context("session reset failed; splinterd was restarted unchanged");
        }
    };

    run_user_systemctl("start").with_context(|| match &backup {
        Some(path) => format!(
            "sessions were backed up to {}, but splinterd failed to restart",
            path.display()
        ),
        None => "no session state existed, but splinterd failed to restart".to_owned(),
    })?;
    wait_for_splinterd_socket(&socket)
        .with_context(|| reset_readiness_context(backup.as_deref()))?;

    println!("Splinterd restarted with no sessions.");
    match backup {
        Some(path) => println!("Previous sessions: {}", path.display()),
        None => println!("No previous session database was present."),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn session_reset_moves_state_to_a_reversible_backup() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "splinterm-reset-test-{}-{nonce}",
            std::process::id()
        ));
        let state = root.join("splinterm");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("lair.json"), b"current").unwrap();
        std::fs::write(state.join("lair.json.previous"), b"previous").unwrap();

        let backup = backup_session_state(&state).unwrap().unwrap();
        assert!(!state.exists());
        assert_eq!(std::fs::read(backup.join("lair.json")).unwrap(), b"current");
        assert_eq!(
            std::fs::read(backup.join("lair.json.previous")).unwrap(),
            b"previous"
        );
        assert!(backup_session_state(&state).unwrap().is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_backup_never_replaces_existing_or_dangling_destinations() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "splinterm-reset-collision-test-{}-{nonce}",
            std::process::id()
        ));
        let state = root.join("splinterm");
        let backup = root.join("splinterm.reset-collision");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("lair.json"), b"sessions").unwrap();
        std::fs::write(&backup, b"existing").unwrap();

        assert!(!move_session_state_no_replace(&state, &backup).unwrap());
        assert!(state.exists());
        assert_eq!(std::fs::read(&backup).unwrap(), b"existing");

        std::fs::remove_file(&backup).unwrap();
        std::os::unix::fs::symlink(root.join("missing-target"), &backup).unwrap();
        assert!(!move_session_state_no_replace(&state, &backup).unwrap());
        assert!(state.exists());
        assert!(std::fs::symlink_metadata(&backup).unwrap().is_symlink());

        std::fs::remove_file(&backup).unwrap();
        assert!(move_session_state_no_replace(&state, &backup).unwrap());
        assert!(!state.exists());
        assert_eq!(
            std::fs::read(backup.join("lair.json")).unwrap(),
            b"sessions"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn readiness_failure_context_preserves_partial_reset_backup() {
        let backup = Path::new("/tmp/splinterm.reset-example");
        let message = reset_readiness_context(Some(backup));
        assert!(message.contains("sessions were reset"));
        assert!(message.contains(backup.to_string_lossy().as_ref()));
        assert!(message.contains("did not become ready"));
    }
}
