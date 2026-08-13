//! Explicit, reversible user-level desktop integrations.

use std::{
    env, fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use super::commands::{IntegrationAction, IntegrationCommand};

const OMARCHY_LAUNCHER: &str = "omarchy-launch-screensaver";
const PACKAGED_HELPER: &str = "/usr/lib/splinterm/integrations/omarchy-launch-screensaver";
const CANONICAL_OMARCHY_LAUNCHER: &str = "/usr/share/omarchy/bin/omarchy-launch-screensaver";

pub(super) fn run(command: IntegrationCommand) -> Result<()> {
    match command {
        IntegrationCommand::Omarchy { action } => super::omarchy_integration::run(action),
        IntegrationCommand::OmarchyScreensaver { action } => {
            let home = env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute() && !path.as_os_str().is_empty())
                .context("HOME must name an absolute directory")?;
            if action == IntegrationAction::Enable
                && !Path::new(CANONICAL_OMARCHY_LAUNCHER).is_file()
            {
                bail!(
                    "Omarchy's canonical screensaver launcher is unavailable: {CANONICAL_OMARCHY_LAUNCHER}"
                );
            }
            run_omarchy_screensaver(
                action,
                &home,
                Path::new(PACKAGED_HELPER),
                &mut std::io::stdout(),
                || resolve_login_launcher(&home),
            )
        }
    }
}

pub(super) fn run_omarchy_screensaver(
    action: IntegrationAction,
    home: &Path,
    helper: &Path,
    output: &mut dyn std::io::Write,
    resolve_launcher: impl Fn() -> Result<PathBuf>,
) -> Result<()> {
    let launcher = home.join(".local/bin").join(OMARCHY_LAUNCHER);
    let disabled = home
        .join(".local/share/splinterm/integrations")
        .join(format!("{OMARCHY_LAUNCHER}.disabled"));
    match action {
        IntegrationAction::Enable => enable(&launcher, &disabled, helper, output, resolve_launcher),
        IntegrationAction::Disable => disable(&launcher, &disabled, helper, output),
        IntegrationAction::Status => status(&launcher, &disabled, helper, output),
    }
}

fn managed_link(path: &Path, helper: &Path) -> bool {
    fs::read_link(path).is_ok_and(|target| target == helper)
}

fn resolve_login_launcher(home: &Path) -> Result<PathBuf> {
    let result = Command::new("bash")
        .args(["-lc", "command -v omarchy-launch-screensaver"])
        .env("HOME", home)
        .output()
        .context("resolve launcher through the login shell")?;
    if !result.status.success() {
        bail!("the login shell cannot resolve omarchy-launch-screensaver");
    }
    let resolved = String::from_utf8(result.stdout)
        .context("login shell returned a non-UTF-8 launcher path")?;
    let resolved = PathBuf::from(resolved.trim());
    if !resolved.is_absolute() {
        bail!("the login shell returned an invalid launcher path");
    }
    Ok(resolved)
}

fn enable(
    path: &Path,
    disabled: &Path,
    helper: &Path,
    output: &mut dyn std::io::Write,
    resolve_launcher: impl Fn() -> Result<PathBuf>,
) -> Result<()> {
    if !helper.is_file() {
        bail!(
            "Splinterm's packaged Omarchy launcher is unavailable: {}",
            helper.display()
        );
    }
    if managed_link(path, helper) {
        writeln!(output, "Omarchy screensaver integration is already enabled")?;
        return Ok(());
    }
    if fs::symlink_metadata(path).is_ok() {
        bail!(
            "refusing to replace existing launcher {}; review it explicitly",
            path.display()
        );
    }
    let parent = path.parent().context("launcher path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create integration directory {}", parent.display()))?;
    if managed_link(disabled, helper) {
        if !move_managed_link(disabled, path, helper, || {})? {
            bail!("preserved integration link disappeared before activation");
        }
    } else if fs::symlink_metadata(disabled).is_ok() {
        bail!(
            "refusing to replace existing disabled integration path {}",
            disabled.display()
        );
    } else {
        symlink(helper, path).with_context(|| {
            format!(
                "create managed launcher without replacing an existing path: {}",
                path.display()
            )
        })?;
    }
    let resolution = resolve_launcher();
    let resolution_failed = match &resolution {
        Ok(resolved) => resolved != path,
        Err(_) => true,
    };
    if resolution_failed {
        let disabled_parent = disabled
            .parent()
            .context("disabled integration path has no parent")?;
        fs::create_dir_all(disabled_parent).with_context(|| {
            format!(
                "create integration state directory {}",
                disabled_parent.display()
            )
        })?;
        if !move_managed_link(path, disabled, helper, || {})? {
            bail!(
                "managed launcher changed before login-shell rollback: {}",
                path.display()
            );
        }
        let resolved = resolution?;
        bail!(
            "managed launcher did not win login-shell resolution; resolved {} instead",
            resolved.display()
        );
    }
    writeln!(
        output,
        "Enabled Splinterm's Omarchy screensaver integration"
    )?;
    writeln!(output, "  Launcher  {}", path.display())?;
    writeln!(
        output,
        "  Disable   splinterm integration omarchy-screensaver disable"
    )?;
    Ok(())
}

fn rename_no_replace(source: &Path, destination: &Path) -> rustix::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
}

fn move_managed_link(
    source: &Path,
    destination: &Path,
    helper: &Path,
    after_move: impl FnOnce(),
) -> Result<bool> {
    match rename_no_replace(source, destination) {
        Ok(()) => {}
        Err(rustix::io::Errno::NOENT) => return Ok(false),
        Err(rustix::io::Errno::EXIST) => bail!(
            "refusing to replace existing integration path {}",
            destination.display()
        ),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "move managed integration {} to {}",
                    source.display(),
                    destination.display()
                )
            });
        }
    }
    after_move();
    if managed_link(destination, helper) {
        return Ok(true);
    }
    match rename_no_replace(destination, source) {
        Ok(()) => bail!(
            "refusing to move launcher not managed by Splinterm: {}",
            source.display()
        ),
        Err(rustix::io::Errno::EXIST) => bail!(
            "launcher changed concurrently; preserved the displaced user object at {}",
            destination.display()
        ),
        Err(error) => Err(error).with_context(|| {
            format!(
                "restore unowned launcher from {} to {}",
                destination.display(),
                source.display()
            )
        }),
    }
}

fn disable(
    path: &Path,
    disabled: &Path,
    helper: &Path,
    output: &mut dyn std::io::Write,
) -> Result<()> {
    if managed_link(disabled, helper) && fs::symlink_metadata(path).is_err() {
        writeln!(output, "Omarchy screensaver integration is not enabled")?;
        return Ok(());
    }
    if fs::symlink_metadata(disabled).is_ok() {
        bail!(
            "refusing to replace existing disabled integration path {}",
            disabled.display()
        );
    }
    let parent = disabled
        .parent()
        .context("disabled integration path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create integration state directory {}", parent.display()))?;
    if move_managed_link(path, disabled, helper, || {})? {
        writeln!(
            output,
            "Disabled Splinterm's Omarchy screensaver integration"
        )?;
        writeln!(output, "  Preserved  {}", disabled.display())?;
    } else {
        writeln!(output, "Omarchy screensaver integration is not enabled")?;
    }
    Ok(())
}

fn status(
    path: &Path,
    disabled: &Path,
    helper: &Path,
    output: &mut dyn std::io::Write,
) -> Result<()> {
    if managed_link(path, helper) {
        writeln!(output, "Omarchy screensaver integration: enabled")?;
    } else if fs::symlink_metadata(path).is_ok() {
        writeln!(
            output,
            "Omarchy screensaver integration: blocked by existing launcher"
        )?;
        writeln!(output, "  File  {}", path.display())?;
    } else if managed_link(disabled, helper) {
        writeln!(output, "Omarchy screensaver integration: disabled")?;
        writeln!(output, "  Preserved  {}", disabled.display())?;
    } else {
        writeln!(output, "Omarchy screensaver integration: disabled")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "splinterm-integration-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn enable_status_and_disable_are_exact_and_idempotent() {
        let root = TestRoot::new();
        let home = root.0.join("home");
        let helper = root.0.join("helper");
        fs::write(&helper, "helper").unwrap();
        let mut output = Cursor::new(Vec::new());

        let launcher = home.join(".local/bin").join(OMARCHY_LAUNCHER);
        let resolve = || Ok(launcher.clone());
        for action in [
            IntegrationAction::Enable,
            IntegrationAction::Enable,
            IntegrationAction::Status,
            IntegrationAction::Disable,
            IntegrationAction::Disable,
            IntegrationAction::Enable,
            IntegrationAction::Disable,
        ] {
            run_omarchy_screensaver(action, &home, &helper, &mut output, resolve).unwrap();
            if action == IntegrationAction::Enable {
                assert_eq!(fs::read_link(&launcher).unwrap(), helper);
            }
        }
        assert!(fs::symlink_metadata(&launcher).is_err());
        let text = String::from_utf8(output.into_inner()).unwrap();
        assert!(text.contains("already enabled"));
        assert!(text.contains("integration: enabled"));
        assert!(text.contains("not enabled"));
    }

    #[test]
    fn existing_or_replaced_launchers_are_never_modified() {
        let root = TestRoot::new();
        let home = root.0.join("home");
        let helper = root.0.join("helper");
        fs::write(&helper, "helper").unwrap();
        let launcher = home.join(".local/bin").join(OMARCHY_LAUNCHER);
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        fs::write(&launcher, "user launcher").unwrap();
        let original = fs::read(&launcher).unwrap();
        let mut output = Cursor::new(Vec::new());

        assert!(
            run_omarchy_screensaver(
                IntegrationAction::Enable,
                &home,
                &helper,
                &mut output,
                || unreachable!(),
            )
            .is_err()
        );
        assert!(
            run_omarchy_screensaver(
                IntegrationAction::Disable,
                &home,
                &helper,
                &mut output,
                || unreachable!(),
            )
            .is_err()
        );
        assert_eq!(fs::read(&launcher).unwrap(), original);
    }

    #[test]
    fn dangling_managed_link_is_preserved_outside_path_after_package_uninstall() {
        let root = TestRoot::new();
        let home = root.0.join("home");
        let helper = root.0.join("missing-helper");
        let launcher = home.join(".local/bin").join(OMARCHY_LAUNCHER);
        let disabled = home
            .join(".local/share/splinterm/integrations")
            .join(format!("{OMARCHY_LAUNCHER}.disabled"));
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        symlink(&helper, &launcher).unwrap();
        let mut output = Cursor::new(Vec::new());

        run_omarchy_screensaver(
            IntegrationAction::Disable,
            &home,
            &helper,
            &mut output,
            || unreachable!(),
        )
        .unwrap();
        assert!(fs::symlink_metadata(&launcher).is_err());
        assert_eq!(fs::read_link(disabled).unwrap(), helper);
    }

    #[test]
    fn replacement_before_move_is_restored_unchanged() {
        let root = TestRoot::new();
        let helper = root.0.join("helper");
        let destination = root.0.join("disabled");
        let launcher = root.0.join(OMARCHY_LAUNCHER);
        fs::write(&helper, "helper").unwrap();
        fs::write(&launcher, "user replacement").unwrap();

        let error = move_managed_link(&launcher, &destination, &helper, || {}).unwrap_err();
        assert!(error.to_string().contains("not managed"));
        assert_eq!(fs::read(&launcher).unwrap(), b"user replacement");
        assert!(fs::symlink_metadata(destination).is_err());
    }

    #[test]
    fn replacement_after_move_preserves_both_objects() {
        let root = TestRoot::new();
        let helper = root.0.join("helper");
        let destination = root.0.join("disabled");
        let launcher = root.0.join(OMARCHY_LAUNCHER);
        fs::write(&helper, "helper").unwrap();
        fs::write(&launcher, "original user file").unwrap();

        let error = move_managed_link(&launcher, &destination, &helper, || {
            fs::write(&launcher, "concurrent user file").unwrap();
        })
        .unwrap_err();
        assert!(error.to_string().contains("preserved the displaced"));
        assert_eq!(fs::read(&launcher).unwrap(), b"concurrent user file");
        assert_eq!(fs::read(&destination).unwrap(), b"original user file");
    }

    #[test]
    fn failed_login_resolution_moves_the_new_link_out_of_path() {
        let root = TestRoot::new();
        let home = root.0.join("home");
        let helper = root.0.join("helper");
        fs::write(&helper, "helper").unwrap();
        let launcher = home.join(".local/bin").join(OMARCHY_LAUNCHER);
        let mut output = Cursor::new(Vec::new());

        let error = run_omarchy_screensaver(
            IntegrationAction::Enable,
            &home,
            &helper,
            &mut output,
            || {
                Ok(PathBuf::from(
                    "/usr/share/omarchy/bin/omarchy-launch-screensaver",
                ))
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("did not win"));
        assert!(fs::symlink_metadata(&launcher).is_err());
        let disabled = home
            .join(".local/share/splinterm/integrations")
            .join(format!("{OMARCHY_LAUNCHER}.disabled"));
        assert_eq!(fs::read_link(disabled).unwrap(), helper);
    }

    #[test]
    fn resolver_error_also_moves_the_new_link_out_of_path() {
        let root = TestRoot::new();
        let home = root.0.join("home");
        let helper = root.0.join("helper");
        fs::write(&helper, "helper").unwrap();
        let launcher = home.join(".local/bin").join(OMARCHY_LAUNCHER);
        let mut output = Cursor::new(Vec::new());

        let error = run_omarchy_screensaver(
            IntegrationAction::Enable,
            &home,
            &helper,
            &mut output,
            || bail!("resolver unavailable"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("resolver unavailable"));
        assert!(fs::symlink_metadata(&launcher).is_err());
        let disabled = home
            .join(".local/share/splinterm/integrations")
            .join(format!("{OMARCHY_LAUNCHER}.disabled"));
        assert_eq!(fs::read_link(disabled).unwrap(), helper);
    }
}
