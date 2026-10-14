//! Explicit, reversible user-level desktop integrations.

use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use super::commands::{IntegrationAction, IntegrationCommand};

const OMARCHY_LAUNCHER: &str = "omarchy-launch-screensaver";
const PACKAGED_HELPER: &str = "/usr/lib/splinterm/integrations/omarchy-launch-screensaver";
const CANONICAL_OMARCHY_LAUNCHER: &str = "/usr/share/omarchy/bin/omarchy-launch-screensaver";
/// Omarchy's Hyprland configuration moves `$OMARCHY_PATH/bin` to the front of
/// every launched command's `PATH`, so a user-level launcher can only win from
/// a Hyprland module evaluated after Omarchy's bootstrap. The module puts a
/// Splinterm-owned directory, holding only the launcher link, ahead of it.
pub(super) const SCREENSAVER_MODULE: &str = "-- Managed by `splinterm integration omarchy-screensaver`; edit only after disabling it.\nlocal splinterm_bin = os.getenv(\"HOME\") .. \"/.local/share/splinterm/omarchy/bin\"\nlocal omarchy_bin = (os.getenv(\"OMARCHY_PATH\") or \"/usr/share/omarchy\") .. \"/bin\"\nlocal path = { splinterm_bin, omarchy_bin }\nfor entry in (os.getenv(\"PATH\") or \"/usr/local/bin:/usr/bin\"):gmatch(\"[^:]+\") do\n  if entry ~= splinterm_bin and entry ~= omarchy_bin then table.insert(path, entry) end\nend\nhl.env(\"PATH\", table.concat(path, \":\"))\n";
pub(super) const SCREENSAVER_REQUIRE_BLOCK: &str = "\n-- BEGIN SPLINTERM SCREENSAVER PATH\nrequire(\"hypr.splinterm-screensaver\")\n-- END SPLINTERM SCREENSAVER PATH\n";

pub(super) fn run(command: IntegrationCommand) -> Result<()> {
    match command {
        IntegrationCommand::Omarchy { action } => super::omarchy_integration::run(action),
        IntegrationCommand::OmarchyScreensaver { action } => {
            let home = absolute_dir_env("HOME")?.context("HOME must name an absolute directory")?;
            let config =
                absolute_dir_env("XDG_CONFIG_HOME")?.unwrap_or_else(|| home.join(".config"));
            if action == IntegrationAction::Enable
                && !Path::new(CANONICAL_OMARCHY_LAUNCHER).is_file()
            {
                bail!(
                    "Omarchy's canonical screensaver launcher is unavailable: {CANONICAL_OMARCHY_LAUNCHER}"
                );
            }
            run_omarchy_screensaver(
                action,
                &ScreensaverPaths::new(&home, &config),
                Path::new(PACKAGED_HELPER),
                &mut std::io::stdout(),
                hyprland_reloader()?,
            )
        }
    }
}

fn absolute_dir_env(name: &str) -> Result<Option<PathBuf>> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || !path.is_absolute() {
        bail!("{name} must name an absolute directory");
    }
    Ok(Some(path))
}

/// Return a reload step that applies the managed `PATH` module and fails only on
/// configuration errors that were not already present before the change, so an
/// unrelated existing error never blocks enabling or disabling. Outside a
/// Hyprland session the next login applies the change.
pub(super) fn hyprland_reloader() -> Result<impl FnMut() -> Result<()>> {
    let session = env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some();
    let baseline = if session {
        config_errors()?
    } else {
        String::new()
    };
    Ok(move || {
        if !session {
            return Ok(());
        }
        reload_hyprland(&baseline)
    })
}

/// Reload Hyprland and fail on any configuration error absent from `baseline`.
pub(super) fn reload_hyprland(baseline: &str) -> Result<()> {
    let reload = Command::new("hyprctl")
        .arg("reload")
        .output()
        .context("reload Hyprland")?;
    if !reload.status.success() {
        bail!("Hyprland rejected the integration reload");
    }
    let errors = config_errors()?;
    if !errors.is_empty() && errors != baseline {
        bail!("Hyprland reported new configuration errors after reload: {errors}");
    }
    Ok(())
}

pub(super) fn config_errors() -> Result<String> {
    let errors = Command::new("hyprctl")
        .arg("configerrors")
        .output()
        .context("read Hyprland configuration errors")?;
    if !errors.status.success() {
        bail!("could not read Hyprland configuration errors");
    }
    Ok(normalize_config_errors(&errors.stdout))
}

/// Omarchy 4's `hyprctl configerrors` prints blank lines when there are none.
fn normalize_config_errors(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout).trim().to_owned()
}

/// Every user-owned object the screensaver component manages.
#[derive(Clone, Debug)]
pub(super) struct ScreensaverPaths {
    pub(super) launcher: PathBuf,
    pub(super) module: PathBuf,
    pub(super) entrypoint: PathBuf,
    /// Pre-Omarchy-4 link location, which Omarchy 4 always shadows.
    pub(super) legacy_launcher: PathBuf,
    pub(super) legacy_disabled: PathBuf,
}

impl ScreensaverPaths {
    pub(super) fn new(home: &Path, config: &Path) -> Self {
        Self {
            launcher: home
                .join(".local/share/splinterm/omarchy/bin")
                .join(OMARCHY_LAUNCHER),
            module: config.join("hypr/splinterm-screensaver.lua"),
            entrypoint: config.join("hypr/hyprland.lua"),
            legacy_launcher: home.join(".local/bin").join(OMARCHY_LAUNCHER),
            legacy_disabled: home
                .join(".local/share/splinterm/integrations")
                .join(format!("{OMARCHY_LAUNCHER}.disabled")),
        }
    }

    fn module_ready(&self) -> bool {
        fs::read_to_string(&self.module).is_ok_and(|text| text == SCREENSAVER_MODULE)
    }

    fn require_count(&self) -> usize {
        fs::read_to_string(&self.entrypoint)
            .map_or(0, |text| text.matches(SCREENSAVER_REQUIRE_BLOCK).count())
    }

    pub(super) fn ready(&self, helper: &Path) -> bool {
        managed_link(&self.launcher, helper) && self.module_ready() && self.require_count() == 1
    }

    pub(super) fn legacy_enabled(&self, helper: &Path) -> bool {
        managed_link(&self.legacy_launcher, helper)
    }
}

pub(super) fn run_omarchy_screensaver(
    action: IntegrationAction,
    paths: &ScreensaverPaths,
    helper: &Path,
    output: &mut dyn Write,
    reload: impl FnMut() -> Result<()>,
) -> Result<()> {
    match action {
        IntegrationAction::Enable => enable(paths, helper, output, reload),
        IntegrationAction::Disable => disable(paths, helper, output, reload),
        IntegrationAction::Status => status(paths, helper, output),
    }
}

fn managed_link(path: &Path, helper: &Path) -> bool {
    fs::read_link(path).is_ok_and(|target| target == helper)
}

fn enable(
    paths: &ScreensaverPaths,
    helper: &Path,
    output: &mut dyn Write,
    mut reload: impl FnMut() -> Result<()>,
) -> Result<()> {
    if !helper.is_file() {
        bail!(
            "Splinterm's packaged Omarchy launcher is unavailable: {}",
            helper.display()
        );
    }
    if paths.ready(helper) && !paths.legacy_enabled(helper) {
        writeln!(output, "Omarchy screensaver integration is already enabled")?;
        return Ok(());
    }
    let plan = plan_enable(paths, helper)?;
    let EnablePlan {
        create_link,
        create_module,
        add_require,
    } = plan;
    let result = (|| {
        if create_link {
            let parent = paths.launcher.parent().context("launcher has no parent")?;
            fs::create_dir_all(parent)
                .with_context(|| format!("create launcher directory {}", parent.display()))?;
            symlink(helper, &paths.launcher).with_context(|| {
                format!(
                    "create managed launcher without replacing an existing path: {}",
                    paths.launcher.display()
                )
            })?;
        }
        if create_module {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&paths.module)
                .and_then(|mut file| file.write_all(SCREENSAVER_MODULE.as_bytes()))
                .context("write managed screensaver module")?;
        }
        if add_require {
            let mut text = fs::read_to_string(&paths.entrypoint)?;
            text.push_str(SCREENSAVER_REQUIRE_BLOCK);
            fs::write(&paths.entrypoint, text).context("append managed screensaver require")?;
        }
        reload()
    })();
    if let Err(error) = result {
        let failures = rollback_enable(paths, helper, plan);
        if failures.is_empty() {
            return Err(error)
                .context("enable Omarchy screensaver integration; changes were rolled back");
        }
        return Err(error).context(format!(
            "enable Omarchy screensaver integration; rollback also failed: {}",
            failures.join("; ")
        ));
    }
    // The shadowed legacy link is retired only once the replacement is live,
    // so a failed enable leaves it untouched.
    if paths.legacy_enabled(helper) {
        retire_legacy(paths, helper)?;
        writeln!(
            output,
            "Moved the legacy launcher {} out of PATH",
            paths.legacy_launcher.display()
        )?;
    }

    writeln!(
        output,
        "Enabled Splinterm's Omarchy screensaver integration"
    )?;
    writeln!(output, "  Launcher  {}", paths.launcher.display())?;
    writeln!(output, "  Module    {}", paths.module.display())?;
    writeln!(
        output,
        "  Run omarchy-restart-shell, or log in again, so the idle service uses it"
    )?;
    writeln!(
        output,
        "  Disable   splinterm integration omarchy-screensaver disable"
    )?;
    Ok(())
}

/// Which managed objects enable must create; existing ones are already exact.
#[derive(Clone, Copy)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag records one independent object to create and roll back"
)]
struct EnablePlan {
    create_link: bool,
    create_module: bool,
    add_require: bool,
}

fn plan_enable(paths: &ScreensaverPaths, helper: &Path) -> Result<EnablePlan> {
    let entrypoint = fs::symlink_metadata(&paths.entrypoint)
        .context("Omarchy Hyprland entrypoint is unavailable")?;
    if !entrypoint.file_type().is_file() {
        bail!("Omarchy Hyprland entrypoint must be a regular file");
    }
    let create_link = fs::symlink_metadata(&paths.launcher).is_err();
    if !create_link && !managed_link(&paths.launcher, helper) {
        bail!(
            "refusing to replace existing launcher {}; review it explicitly",
            paths.launcher.display()
        );
    }
    let create_module = fs::symlink_metadata(&paths.module).is_err();
    if !create_module && !paths.module_ready() {
        bail!(
            "refusing to replace edited screensaver module {}",
            paths.module.display()
        );
    }
    let add_require = match paths.require_count() {
        0 => true,
        1 => false,
        _ => bail!("Hyprland entrypoint contains duplicate Splinterm screensaver markers"),
    };
    if add_require && fs::read_to_string(&paths.entrypoint)?.contains("SPLINTERM SCREENSAVER PATH")
    {
        bail!("Hyprland entrypoint contains an unowned Splinterm screensaver marker");
    }
    if paths.legacy_enabled(helper) && fs::symlink_metadata(&paths.legacy_disabled).is_ok() {
        bail!(
            "refusing to replace existing disabled integration path {}",
            paths.legacy_disabled.display()
        );
    }
    Ok(EnablePlan {
        create_link,
        create_module,
        add_require,
    })
}

/// Move the shadowed pre-Omarchy-4 `~/.local/bin` link out of `PATH`. It is
/// only moved, never unlinked, so a raced user object is preserved.
fn retire_legacy(paths: &ScreensaverPaths, helper: &Path) -> Result<()> {
    let parent = paths
        .legacy_disabled
        .parent()
        .context("disabled integration path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create integration state directory {}", parent.display()))?;
    move_managed_link(
        &paths.legacy_launcher,
        &paths.legacy_disabled,
        helper,
        || {},
    )?;
    Ok(())
}

/// Undo [`retire_legacy`] for a rolled-back disable, without replacing anything.
pub(super) fn restore_legacy(paths: &ScreensaverPaths, helper: &Path) -> Result<()> {
    if paths.legacy_enabled(helper) || !managed_link(&paths.legacy_disabled, helper) {
        return Ok(());
    }
    move_managed_link(
        &paths.legacy_disabled,
        &paths.legacy_launcher,
        helper,
        || {},
    )?;
    Ok(())
}

fn rollback_enable(paths: &ScreensaverPaths, helper: &Path, plan: EnablePlan) -> Vec<String> {
    let mut failures = Vec::new();
    if plan.add_require
        && let Err(error) = remove_require(paths)
    {
        failures.push(format!("Hyprland entrypoint: {error}"));
    }
    if plan.create_module
        && paths.module_ready()
        && let Err(error) = fs::remove_file(&paths.module)
    {
        failures.push(format!("screensaver module: {error}"));
    }
    if plan.create_link
        && managed_link(&paths.launcher, helper)
        && let Err(error) = fs::remove_file(&paths.launcher)
    {
        failures.push(format!("launcher: {error}"));
    }
    failures
}

fn remove_require(paths: &ScreensaverPaths) -> Result<()> {
    let text = fs::read_to_string(&paths.entrypoint)?;
    if text.matches(SCREENSAVER_REQUIRE_BLOCK).count() != 1 {
        bail!("refusing to alter changed Splinterm screensaver marker");
    }
    fs::write(
        &paths.entrypoint,
        text.replacen(SCREENSAVER_REQUIRE_BLOCK, "", 1),
    )
    .context("remove managed screensaver require")
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
    paths: &ScreensaverPaths,
    helper: &Path,
    output: &mut dyn Write,
    mut reload: impl FnMut() -> Result<()>,
) -> Result<()> {
    let require = paths.require_count();
    let module = fs::symlink_metadata(&paths.module).is_ok();
    if require > 1 {
        bail!("refusing to alter duplicate Splinterm screensaver markers");
    }
    if module && !paths.module_ready() {
        bail!(
            "refusing to remove edited screensaver module {}",
            paths.module.display()
        );
    }
    let legacy = paths.legacy_enabled(helper);
    if legacy && fs::symlink_metadata(&paths.legacy_disabled).is_ok() {
        bail!(
            "refusing to replace existing disabled integration path {}",
            paths.legacy_disabled.display()
        );
    }
    if require == 0 && !module && !legacy {
        writeln!(output, "Omarchy screensaver integration is not enabled")?;
        return Ok(());
    }

    if require == 1 {
        remove_require(paths)?;
        if let Err(error) = reload() {
            let mut text = fs::read_to_string(&paths.entrypoint)?;
            text.push_str(SCREENSAVER_REQUIRE_BLOCK);
            fs::write(&paths.entrypoint, text)?;
            // Best effort: bring the live session back to the restored file.
            let _ = reload();
            return Err(error)
                .context("disable Omarchy screensaver integration; changes were rolled back");
        }
    }
    if module {
        fs::remove_file(&paths.module).context("remove managed screensaver module")?;
    }
    // The launcher link stays in its Splinterm-owned directory, which the next
    // session no longer puts ahead of Omarchy's bin.
    if legacy {
        retire_legacy(paths, helper)?;
    }
    writeln!(
        output,
        "Disabled Splinterm's Omarchy screensaver integration"
    )?;
    writeln!(
        output,
        "  Run omarchy-restart-shell, or log in again, so the idle service stops using it"
    )?;
    Ok(())
}

fn status(paths: &ScreensaverPaths, helper: &Path, output: &mut dyn Write) -> Result<()> {
    if paths.ready(helper) {
        writeln!(output, "Omarchy screensaver integration: enabled")?;
    } else if paths.legacy_enabled(helper) {
        writeln!(
            output,
            "Omarchy screensaver integration: inactive (legacy launcher is shadowed by Omarchy)"
        )?;
        writeln!(
            output,
            "  Migrate  splinterm integration omarchy-screensaver enable"
        )?;
    } else if fs::symlink_metadata(&paths.module).is_ok()
        || paths.require_count() > 0
        || fs::symlink_metadata(&paths.launcher)
            .is_ok_and(|_| !managed_link(&paths.launcher, helper))
    {
        writeln!(
            output,
            "Omarchy screensaver integration: incomplete or changed"
        )?;
        writeln!(output, "  Module    {}", paths.module.display())?;
        writeln!(output, "  Launcher  {}", paths.launcher.display())?;
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

        /// A home with an Omarchy entrypoint and a packaged helper.
        fn omarchy(&self) -> (ScreensaverPaths, PathBuf) {
            let paths = ScreensaverPaths::new(&self.0.join("home"), &self.0.join("config"));
            fs::create_dir_all(paths.entrypoint.parent().unwrap()).unwrap();
            fs::write(&paths.entrypoint, "-- user config\n").unwrap();
            let helper = self.0.join("helper");
            fs::write(&helper, "helper").unwrap();
            (paths, helper)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(
        action: IntegrationAction,
        paths: &ScreensaverPaths,
        helper: &Path,
        output: &mut Cursor<Vec<u8>>,
    ) -> Result<()> {
        run_omarchy_screensaver(action, paths, helper, output, || Ok(()))
    }

    #[test]
    fn enable_status_and_disable_are_exact_and_idempotent() {
        let root = TestRoot::new();
        let (paths, helper) = root.omarchy();
        let mut output = Cursor::new(Vec::new());

        for action in [
            IntegrationAction::Enable,
            IntegrationAction::Enable,
            IntegrationAction::Status,
            IntegrationAction::Disable,
            IntegrationAction::Disable,
            IntegrationAction::Enable,
        ] {
            run(action, &paths, &helper, &mut output).unwrap();
            if action == IntegrationAction::Enable {
                assert!(paths.ready(&helper));
            }
        }
        assert_eq!(
            fs::read_to_string(&paths.entrypoint).unwrap(),
            format!("-- user config\n{SCREENSAVER_REQUIRE_BLOCK}")
        );
        run(IntegrationAction::Disable, &paths, &helper, &mut output).unwrap();
        assert_eq!(
            fs::read_to_string(&paths.entrypoint).unwrap(),
            "-- user config\n"
        );
        assert!(fs::symlink_metadata(&paths.module).is_err());
        let text = String::from_utf8(output.into_inner()).unwrap();
        assert!(text.contains("already enabled"));
        assert!(text.contains("integration: enabled"));
        assert!(text.contains("not enabled"));
    }

    #[test]
    fn blank_config_error_output_means_no_errors() {
        assert_eq!(normalize_config_errors(b"\n\n"), "");
        assert_eq!(
            normalize_config_errors(b"\nbad option\n"),
            "bad option".to_owned()
        );
    }

    #[test]
    fn module_puts_the_launcher_directory_ahead_of_omarchy_bin() {
        assert!(SCREENSAVER_MODULE.contains("\"/.local/share/splinterm/omarchy/bin\""));
        assert!(SCREENSAVER_MODULE.contains("local path = { splinterm_bin, omarchy_bin }"));
        let root = TestRoot::new();
        let (paths, _) = root.omarchy();
        assert!(
            paths
                .launcher
                .ends_with(".local/share/splinterm/omarchy/bin/omarchy-launch-screensaver")
        );
    }

    #[test]
    fn legacy_link_is_migrated_out_of_path() {
        let root = TestRoot::new();
        let (paths, helper) = root.omarchy();
        fs::create_dir_all(paths.legacy_launcher.parent().unwrap()).unwrap();
        symlink(&helper, &paths.legacy_launcher).unwrap();
        let mut output = Cursor::new(Vec::new());
        run(IntegrationAction::Status, &paths, &helper, &mut output).unwrap();
        assert!(String::from_utf8_lossy(output.get_ref()).contains("shadowed"));

        run(IntegrationAction::Enable, &paths, &helper, &mut output).unwrap();
        assert!(paths.ready(&helper));
        assert!(fs::symlink_metadata(&paths.legacy_launcher).is_err());
        assert_eq!(fs::read_link(&paths.legacy_disabled).unwrap(), helper);
    }

    #[test]
    fn unrelated_user_launcher_in_local_bin_is_left_alone() {
        let root = TestRoot::new();
        let (paths, helper) = root.omarchy();
        fs::create_dir_all(paths.legacy_launcher.parent().unwrap()).unwrap();
        fs::write(&paths.legacy_launcher, "user launcher").unwrap();
        let mut output = Cursor::new(Vec::new());
        run(IntegrationAction::Enable, &paths, &helper, &mut output).unwrap();
        run(IntegrationAction::Disable, &paths, &helper, &mut output).unwrap();
        assert_eq!(fs::read(&paths.legacy_launcher).unwrap(), b"user launcher");
    }

    #[test]
    fn existing_or_edited_objects_are_never_replaced() {
        let root = TestRoot::new();
        let (paths, helper) = root.omarchy();
        fs::create_dir_all(paths.launcher.parent().unwrap()).unwrap();
        fs::write(&paths.launcher, "user launcher").unwrap();
        let mut output = Cursor::new(Vec::new());
        assert!(run(IntegrationAction::Enable, &paths, &helper, &mut output).is_err());
        assert_eq!(fs::read(&paths.launcher).unwrap(), b"user launcher");
        assert_eq!(
            fs::read_to_string(&paths.entrypoint).unwrap(),
            "-- user config\n"
        );
        assert!(fs::symlink_metadata(&paths.module).is_err());

        fs::remove_file(&paths.launcher).unwrap();
        run(IntegrationAction::Enable, &paths, &helper, &mut output).unwrap();
        fs::write(&paths.module, "-- edited\n").unwrap();
        assert!(run(IntegrationAction::Disable, &paths, &helper, &mut output).is_err());
        assert_eq!(fs::read(&paths.module).unwrap(), b"-- edited\n");
        assert_eq!(paths.require_count(), 1);
    }

    #[test]
    fn reload_failure_rolls_enable_back() {
        let root = TestRoot::new();
        let (paths, helper) = root.omarchy();
        let error = run_omarchy_screensaver(
            IntegrationAction::Enable,
            &paths,
            &helper,
            &mut Cursor::new(Vec::new()),
            || bail!("injected reload failure"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("rolled back"));
        assert_eq!(
            fs::read_to_string(&paths.entrypoint).unwrap(),
            "-- user config\n"
        );
        assert!(fs::symlink_metadata(&paths.module).is_err());
        assert!(fs::symlink_metadata(&paths.launcher).is_err());
    }

    #[test]
    fn failed_enable_keeps_the_legacy_link_in_place() {
        let root = TestRoot::new();
        let (paths, helper) = root.omarchy();
        fs::create_dir_all(paths.legacy_launcher.parent().unwrap()).unwrap();
        symlink(&helper, &paths.legacy_launcher).unwrap();
        assert!(
            run_omarchy_screensaver(
                IntegrationAction::Enable,
                &paths,
                &helper,
                &mut Cursor::new(Vec::new()),
                || bail!("injected reload failure"),
            )
            .is_err()
        );
        assert!(paths.legacy_enabled(&helper));
        assert!(fs::symlink_metadata(&paths.legacy_disabled).is_err());
        assert!(!paths.ready(&helper));
    }

    #[test]
    fn missing_entrypoint_refuses_without_changes() {
        let root = TestRoot::new();
        let (paths, helper) = root.omarchy();
        fs::remove_file(&paths.entrypoint).unwrap();
        assert!(
            run(
                IntegrationAction::Enable,
                &paths,
                &helper,
                &mut Cursor::new(Vec::new())
            )
            .is_err()
        );
        assert!(fs::symlink_metadata(&paths.launcher).is_err());
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
}
