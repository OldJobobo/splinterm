//! Unified, explicit Omarchy desktop integration lifecycle.

use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};

use super::commands::IntegrationAction;

const DESKTOP_ID: &str = "com.oldjobobo.splinterm.desktop";
const SCREENSAVER_HELPER: &str = "/usr/lib/splinterm/integrations/omarchy-launch-screensaver";
const SCREENSAVER_LAUNCHER: &str = "omarchy-launch-screensaver";
const MANAGED_TERMINALS: &str = "# Managed by `splinterm integration omarchy`; edit only after disabling it.\ncom.oldjobobo.splinterm.desktop\n";
const HYPR_MODULE: &str = "-- Managed by `splinterm integration omarchy`; edit only after disabling it.\nhl.window_rule({\n  name = \"splinterm-terminal-tag\",\n  match = { initial_class = \"^com\\\\.oldjobobo\\\\.splinterm$\" },\n  tag = \"+terminal\",\n})\n";
const HYPR_REQUIRE_BLOCK: &str = "\n-- BEGIN SPLINTERM OMARCHY INTEGRATION\nrequire(\"hypr.splinterm-integration\")\n-- END SPLINTERM OMARCHY INTEGRATION\n";
const EXTERNAL_HYPR_RULE: &str = "hl.window_rule({\n  name = \"splinterm-terminal-tag\",\n  match = { initial_class = \"^com\\\\.oldjobobo\\\\.splinterm$\" },\n  tag = \"+terminal\",\n})";

#[derive(Clone, Debug)]
struct Roots {
    home: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl Roots {
    fn from_env() -> Result<Self> {
        let home = absolute_env("HOME")?.context("HOME must name an absolute directory")?;
        if !home.is_dir() {
            bail!("HOME must name an existing directory");
        }
        let config = absolute_env("XDG_CONFIG_HOME")?.unwrap_or_else(|| home.join(".config"));
        let state = absolute_env("XDG_STATE_HOME")?.unwrap_or_else(|| home.join(".local/state"));
        Ok(Self {
            home,
            config,
            state,
        })
    }

    fn integration_state(&self) -> PathBuf {
        self.state.join("splinterm/integrations")
    }

    fn manifest(&self) -> PathBuf {
        self.integration_state().join("omarchy.json")
    }

    fn lock(&self) -> PathBuf {
        self.integration_state().join("omarchy.lock")
    }

    fn terminals(&self) -> PathBuf {
        self.config.join("xdg-terminals.list")
    }

    fn terminals_backup(&self) -> PathBuf {
        self.config.join("xdg-terminals.list.splinterm-original")
    }

    fn hypr_entrypoint(&self) -> PathBuf {
        self.config.join("hypr/hyprland.lua")
    }

    fn hypr_module(&self) -> PathBuf {
        self.config.join("hypr/splinterm-integration.lua")
    }

    fn terminal_staged(&self) -> PathBuf {
        self.config.join("xdg-terminals.list.splinterm-disabled")
    }

    fn hypr_entrypoint_staged(&self) -> PathBuf {
        self.config.join("hypr/hyprland.lua.splinterm-enabled")
    }

    fn hypr_module_staged(&self) -> PathBuf {
        self.config.join("hypr/splinterm-integration.lua.disabled")
    }

    fn screensaver_launcher(&self) -> PathBuf {
        self.home.join(".local/bin").join(SCREENSAVER_LAUNCHER)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the journal records four independent ownership/restoration facts, not combinable state"
)]
struct Manifest {
    version: u8,
    phase: Phase,
    terminal_owned: bool,
    terminal_had_original: bool,
    hypr_owned: bool,
    screensaver_owned: bool,
    screensaver_created: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Pending,
    Committed,
    PendingDisable,
}

pub(super) fn run(action: IntegrationAction) -> Result<()> {
    let roots = Roots::from_env()?;
    let mut runtime = SystemRuntime;
    let mut output = std::io::stdout().lock();
    if action == IntegrationAction::Status {
        return run_with(action, &roots, &mut runtime, &mut output);
    }
    fs::create_dir_all(roots.integration_state()).with_context(|| {
        format!(
            "create integration state directory {}",
            roots.integration_state().display()
        )
    })?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(roots.lock())
        .context("open Omarchy integration lock")?;
    flock(&lock, FlockOperation::LockExclusive).context("lock Omarchy integration state")?;
    run_with(action, &roots, &mut runtime, &mut output)
}

fn absolute_env(name: &str) -> Result<Option<PathBuf>> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || !path.is_absolute() {
        bail!("{name} must name an absolute directory");
    }
    if path.exists() && !path.is_dir() {
        bail!("{name} names an existing non-directory");
    }
    Ok(Some(path))
}

trait Runtime {
    fn terminal_id(&mut self) -> Result<String>;
    fn reload_hyprland(&mut self) -> Result<()>;
    fn screensaver(
        &mut self,
        action: IntegrationAction,
        roots: &Roots,
        output: &mut dyn Write,
    ) -> Result<()>;
}

struct SystemRuntime;

impl Runtime for SystemRuntime {
    fn terminal_id(&mut self) -> Result<String> {
        let result = Command::new("xdg-terminal-exec")
            .arg("--print-id")
            .output()
            .context("run xdg-terminal-exec --print-id")?;
        if !result.status.success() {
            bail!("xdg-terminal-exec could not resolve the default terminal");
        }
        let id = String::from_utf8(result.stdout)
            .context("xdg-terminal-exec returned a non-UTF-8 desktop ID")?;
        let id = id.trim();
        if id.is_empty() || id.lines().count() != 1 {
            bail!("xdg-terminal-exec returned an invalid desktop ID");
        }
        Ok(id.to_owned())
    }

    fn reload_hyprland(&mut self) -> Result<()> {
        let reload = Command::new("hyprctl")
            .arg("reload")
            .output()
            .context("reload Hyprland")?;
        if !reload.status.success() {
            bail!("Hyprland rejected the integration reload");
        }
        let errors = Command::new("hyprctl")
            .arg("configerrors")
            .output()
            .context("read Hyprland configuration errors")?;
        if !errors.status.success() || !errors.stdout.is_empty() {
            bail!("Hyprland reported configuration errors after reload");
        }
        Ok(())
    }

    fn screensaver(
        &mut self,
        action: IntegrationAction,
        roots: &Roots,
        output: &mut dyn Write,
    ) -> Result<()> {
        if action == IntegrationAction::Enable
            && !Path::new("/usr/share/omarchy/bin/omarchy-launch-screensaver").is_file()
        {
            bail!("Omarchy's canonical screensaver launcher is unavailable");
        }
        super::integrations::run_omarchy_screensaver(
            action,
            &roots.home,
            Path::new(SCREENSAVER_HELPER),
            output,
            || {
                let result = Command::new("bash")
                    .args(["-lc", "command -v omarchy-launch-screensaver"])
                    .env("HOME", &roots.home)
                    .output()
                    .context("resolve launcher through the login shell")?;
                if !result.status.success() {
                    bail!("the login shell cannot resolve omarchy-launch-screensaver");
                }
                let resolved = String::from_utf8(result.stdout)
                    .context("login shell returned a non-UTF-8 launcher path")?;
                let path = PathBuf::from(resolved.trim());
                if !path.is_absolute() {
                    bail!("the login shell returned an invalid launcher path");
                }
                Ok(path)
            },
        )
    }
}

fn run_with(
    action: IntegrationAction,
    roots: &Roots,
    runtime: &mut impl Runtime,
    output: &mut dyn Write,
) -> Result<()> {
    if roots.manifest().exists() {
        let manifest = read_manifest(roots)?;
        if manifest.phase != Phase::Committed {
            bail!(
                "Omarchy integration recovery is required; pending state: {}",
                roots.manifest().display()
            );
        }
    }
    match action {
        IntegrationAction::Enable => enable(roots, runtime, output),
        IntegrationAction::Disable => disable(roots, runtime, output),
        IntegrationAction::Status => status(roots, runtime, output),
    }
}

fn read_manifest(roots: &Roots) -> Result<Manifest> {
    let bytes = fs::read(roots.manifest()).context("read Omarchy integration state")?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).context("parse Omarchy integration state")?;
    if manifest.version != 1 {
        bail!("unsupported Omarchy integration state version");
    }
    Ok(manifest)
}

fn write_manifest(roots: &Roots, manifest: &Manifest) -> Result<()> {
    let temporary = roots.integration_state().join("omarchy.json.new");
    let mut bytes = serde_json::to_vec_pretty(manifest).context("encode integration state")?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes).context("write pending Omarchy integration state")?;
    fs::rename(&temporary, roots.manifest()).context("publish Omarchy integration state")
}

fn terminal_ready(runtime: &mut impl Runtime) -> bool {
    runtime.terminal_id().is_ok_and(|id| id == DESKTOP_ID)
}

fn screensaver_ready(roots: &Roots) -> bool {
    fs::read_link(roots.screensaver_launcher())
        .is_ok_and(|target| target == Path::new(SCREENSAVER_HELPER))
}

fn hypr_external_ready(roots: &Roots) -> bool {
    let Ok(entries) = fs::read_dir(roots.config.join("hypr")) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "lua")
            && fs::read_to_string(entry.path())
                .is_ok_and(|text| active_lua(&text).contains(EXTERNAL_HYPR_RULE))
    })
}

fn active_lua(text: &str) -> String {
    text.lines()
        .map(|line| line.split_once("--").map_or(line, |(active, _)| active))
        .collect::<Vec<_>>()
        .join("\n")
}

fn managed_hypr_ready(roots: &Roots) -> bool {
    fs::read_to_string(roots.hypr_module()).is_ok_and(|text| text == HYPR_MODULE)
        && fs::read_to_string(roots.hypr_entrypoint())
            .is_ok_and(|text| text.matches(HYPR_REQUIRE_BLOCK).count() == 1)
}

fn enable(roots: &Roots, runtime: &mut impl Runtime, output: &mut dyn Write) -> Result<()> {
    if roots.manifest().exists() {
        writeln!(output, "Splinterm's Omarchy integration is already enabled")?;
        return status(roots, runtime, output);
    }
    let terminal_owned = runtime.terminal_id()? != DESKTOP_ID;
    let hypr_owned = !hypr_external_ready(roots);
    // The exact packaged link is already a Splinterm-owned object, including
    // links created by the legacy component-only command. Adopt it so the
    // unified lifecycle can reverse all Splinterm-managed Omarchy setup.
    let screensaver_already_managed = screensaver_ready(roots);
    let screensaver_owned = true;
    preflight(
        roots,
        terminal_owned,
        hypr_owned,
        !screensaver_already_managed,
    )?;

    let mut manifest = Manifest {
        version: 1,
        phase: Phase::Pending,
        terminal_owned,
        terminal_had_original: terminal_owned && fs::symlink_metadata(roots.terminals()).is_ok(),
        hypr_owned,
        screensaver_owned,
        screensaver_created: !screensaver_already_managed,
    };
    write_manifest(roots, &manifest)?;

    let result = (|| {
        if terminal_owned {
            enable_terminal(roots, manifest.terminal_had_original)?;
        }
        if hypr_owned {
            enable_hypr(roots)?;
        }
        if !screensaver_already_managed {
            runtime.screensaver(IntegrationAction::Enable, roots, output)?;
        }
        if terminal_owned && !terminal_ready(runtime) {
            bail!("xdg-terminal-exec did not select Splinterm after integration");
        }
        if hypr_owned {
            runtime.reload_hyprland()?;
        }
        Ok(())
    })();

    if let Err(error) = result {
        let rollback = rollback_enable(roots, runtime, &manifest, output);
        if rollback.is_ok() {
            let _ = fs::remove_file(roots.manifest());
            return Err(error).context("enable Omarchy integration; changes were rolled back");
        }
        return Err(error).context(format!(
            "enable Omarchy integration; rollback also failed: {}",
            rollback.unwrap_err()
        ));
    }

    manifest.phase = Phase::Committed;
    write_manifest(roots, &manifest)?;
    writeln!(output, "Omarchy integration: ready")?;
    print_components(
        output,
        terminal_owned,
        hypr_owned,
        !screensaver_already_managed,
    )?;
    writeln!(
        output,
        "  Disable      splinterm integration omarchy disable"
    )?;
    Ok(())
}

fn preflight(
    roots: &Roots,
    terminal_owned: bool,
    hypr_owned: bool,
    screensaver_owned: bool,
) -> Result<()> {
    if terminal_owned && fs::symlink_metadata(roots.terminals_backup()).is_ok() {
        bail!(
            "refusing to replace terminal preference backup {}",
            roots.terminals_backup().display()
        );
    }
    if hypr_owned {
        let metadata = fs::symlink_metadata(roots.hypr_entrypoint())
            .context("Omarchy Hyprland entrypoint is unavailable")?;
        if !metadata.file_type().is_file() {
            bail!("Omarchy Hyprland entrypoint must be a regular file");
        }
        if fs::symlink_metadata(roots.hypr_module()).is_ok() {
            bail!(
                "refusing to replace existing Hyprland integration module {}",
                roots.hypr_module().display()
            );
        }
        let entrypoint = fs::read_to_string(roots.hypr_entrypoint())
            .context("read Omarchy Hyprland entrypoint as UTF-8")?;
        if entrypoint.contains("SPLINTERM OMARCHY INTEGRATION") {
            bail!("Hyprland entrypoint contains an unowned Splinterm integration marker");
        }
    }
    if screensaver_owned && fs::symlink_metadata(roots.screensaver_launcher()).is_ok() {
        bail!(
            "refusing to replace existing screensaver launcher {}",
            roots.screensaver_launcher().display()
        );
    }
    Ok(())
}

fn enable_terminal(roots: &Roots, had_original: bool) -> Result<()> {
    if had_original {
        rename_no_replace(&roots.terminals(), &roots.terminals_backup())?;
    }
    if let Some(parent) = roots.terminals().parent() {
        fs::create_dir_all(parent)?;
    }
    let result = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(roots.terminals())
        .and_then(|mut file| file.write_all(MANAGED_TERMINALS.as_bytes()));
    if let Err(error) = result {
        if had_original {
            let _ = rename_no_replace(&roots.terminals_backup(), &roots.terminals());
        }
        return Err(error).context("write managed terminal preference");
    }
    Ok(())
}

fn enable_hypr(roots: &Roots) -> Result<()> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(roots.hypr_module())
        .and_then(|mut file| file.write_all(HYPR_MODULE.as_bytes()))
        .context("write managed Hyprland integration module")?;
    let mut entrypoint = fs::read_to_string(roots.hypr_entrypoint())?;
    entrypoint.push_str(HYPR_REQUIRE_BLOCK);
    if let Err(error) = fs::write(roots.hypr_entrypoint(), entrypoint) {
        let _ = fs::remove_file(roots.hypr_module());
        return Err(error).context("append managed Hyprland integration require");
    }
    Ok(())
}

fn disable(roots: &Roots, runtime: &mut impl Runtime, output: &mut dyn Write) -> Result<()> {
    if !roots.manifest().exists() {
        writeln!(output, "Omarchy integration is not managed by Splinterm")?;
        return status(roots, runtime, output);
    }
    let mut manifest = read_manifest(roots)?;
    preflight_disable(roots, &manifest)?;
    manifest.phase = Phase::PendingDisable;
    write_manifest(roots, &manifest)?;

    let result: Result<()> = (|| {
        if manifest.terminal_owned {
            stage_disable_terminal(roots, manifest.terminal_had_original)?;
        }
        if manifest.hypr_owned {
            stage_disable_hypr(roots)?;
            runtime.reload_hyprland()?;
        }
        if manifest.screensaver_owned {
            runtime.screensaver(IntegrationAction::Disable, roots, output)?;
        }
        fs::remove_file(roots.manifest()).context("remove Omarchy integration state")?;
        Ok(())
    })();

    if let Err(error) = result {
        let rollback = rollback_disable(roots, runtime, &manifest, output);
        if rollback.is_ok() {
            manifest.phase = Phase::Committed;
            write_manifest(roots, &manifest)?;
            return Err(error).context("disable Omarchy integration; changes were rolled back");
        }
        return Err(error).context(format!(
            "disable Omarchy integration; rollback also failed: {}",
            rollback.unwrap_err()
        ));
    }

    cleanup_staged_disable(roots, &manifest)?;
    writeln!(output, "Disabled Splinterm's Omarchy integration")?;
    writeln!(output, "  External configuration was left unchanged")?;
    Ok(())
}

fn preflight_disable(roots: &Roots, manifest: &Manifest) -> Result<()> {
    if manifest.terminal_owned {
        let current =
            fs::read_to_string(roots.terminals()).context("read managed terminal preference")?;
        if current != MANAGED_TERMINALS {
            bail!("refusing to replace edited terminal preference");
        }
        if manifest.terminal_had_original && fs::symlink_metadata(roots.terminals_backup()).is_err()
        {
            bail!("managed terminal preference backup is missing");
        }
    }
    if manifest.hypr_owned {
        let module =
            fs::read_to_string(roots.hypr_module()).context("read managed Hyprland module")?;
        if module != HYPR_MODULE {
            bail!("refusing to remove edited Hyprland integration module");
        }
        let entrypoint = fs::read_to_string(roots.hypr_entrypoint())?;
        if entrypoint.matches(HYPR_REQUIRE_BLOCK).count() != 1 {
            bail!("refusing to alter changed Hyprland integration marker");
        }
    }
    for staged in [
        roots.terminal_staged(),
        roots.hypr_entrypoint_staged(),
        roots.hypr_module_staged(),
    ] {
        if fs::symlink_metadata(&staged).is_ok() {
            bail!(
                "refusing to replace existing staged integration path {}",
                staged.display()
            );
        }
    }
    if manifest.screensaver_owned && !screensaver_ready(roots) {
        bail!("managed screensaver launcher is missing or changed");
    }
    Ok(())
}

fn stage_disable_terminal(roots: &Roots, had_original: bool) -> Result<()> {
    rename_no_replace(&roots.terminals(), &roots.terminal_staged())?;
    if had_original
        && let Err(error) = rename_no_replace(&roots.terminals_backup(), &roots.terminals())
    {
        let _ = rename_no_replace(&roots.terminal_staged(), &roots.terminals());
        return Err(error).context("restore original terminal preference");
    }
    Ok(())
}

fn rollback_disable_terminal(roots: &Roots, had_original: bool) -> Result<()> {
    if had_original {
        rename_no_replace(&roots.terminals(), &roots.terminals_backup())?;
    }
    rename_no_replace(&roots.terminal_staged(), &roots.terminals())
}

fn disable_terminal(roots: &Roots, had_original: bool) -> Result<()> {
    let current =
        fs::read_to_string(roots.terminals()).context("read managed terminal preference")?;
    if current != MANAGED_TERMINALS {
        bail!("refusing to replace edited terminal preference");
    }
    fs::remove_file(roots.terminals()).context("remove managed terminal preference")?;
    if had_original {
        rename_no_replace(&roots.terminals_backup(), &roots.terminals())?;
    }
    Ok(())
}

fn stage_disable_hypr(roots: &Roots) -> Result<()> {
    rename_no_replace(&roots.hypr_entrypoint(), &roots.hypr_entrypoint_staged())?;
    if let Err(error) = rename_no_replace(&roots.hypr_module(), &roots.hypr_module_staged()) {
        let _ = rename_no_replace(&roots.hypr_entrypoint_staged(), &roots.hypr_entrypoint());
        return Err(error).context("stage managed Hyprland module");
    }
    let staged = fs::read_to_string(roots.hypr_entrypoint_staged())?;
    let restored = staged.replacen(HYPR_REQUIRE_BLOCK, "", 1);
    if let Err(error) = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(roots.hypr_entrypoint())
        .and_then(|mut file| file.write_all(restored.as_bytes()))
    {
        let _ = rename_no_replace(&roots.hypr_module_staged(), &roots.hypr_module());
        let _ = rename_no_replace(&roots.hypr_entrypoint_staged(), &roots.hypr_entrypoint());
        return Err(error).context("stage Hyprland entrypoint without managed require");
    }
    Ok(())
}

fn rollback_disable_hypr(roots: &Roots) -> Result<()> {
    let without_require = roots
        .hypr_entrypoint()
        .with_extension("lua.splinterm-rollback");
    rename_no_replace(&roots.hypr_entrypoint(), &without_require)?;
    if let Err(error) = rename_no_replace(&roots.hypr_entrypoint_staged(), &roots.hypr_entrypoint())
    {
        let _ = rename_no_replace(&without_require, &roots.hypr_entrypoint());
        return Err(error);
    }
    if let Err(error) = rename_no_replace(&roots.hypr_module_staged(), &roots.hypr_module()) {
        let _ = rename_no_replace(&roots.hypr_entrypoint(), &roots.hypr_entrypoint_staged());
        let _ = rename_no_replace(&without_require, &roots.hypr_entrypoint());
        return Err(error);
    }
    fs::remove_file(without_require).context("remove staged Hyprland rollback entrypoint")
}

fn disable_hypr(roots: &Roots) -> Result<()> {
    let module = fs::read_to_string(roots.hypr_module()).context("read managed Hyprland module")?;
    if module != HYPR_MODULE {
        bail!("refusing to remove edited Hyprland integration module");
    }
    let entrypoint = fs::read_to_string(roots.hypr_entrypoint())?;
    if entrypoint.matches(HYPR_REQUIRE_BLOCK).count() != 1 {
        bail!("refusing to alter changed Hyprland integration marker");
    }
    fs::write(
        roots.hypr_entrypoint(),
        entrypoint.replacen(HYPR_REQUIRE_BLOCK, "", 1),
    )?;
    fs::remove_file(roots.hypr_module())?;
    Ok(())
}

fn rollback_disable(
    roots: &Roots,
    runtime: &mut impl Runtime,
    manifest: &Manifest,
    output: &mut dyn Write,
) -> Result<()> {
    let mut failures = Vec::new();
    if manifest.screensaver_owned
        && !screensaver_ready(roots)
        && let Err(error) = runtime.screensaver(IntegrationAction::Enable, roots, output)
    {
        failures.push(format!("screensaver: {error}"));
    }
    if manifest.hypr_owned && fs::symlink_metadata(roots.hypr_entrypoint_staged()).is_ok() {
        if let Err(error) = rollback_disable_hypr(roots) {
            failures.push(format!("Hyprland files: {error}"));
        } else if let Err(error) = runtime.reload_hyprland() {
            failures.push(format!("Hyprland reload: {error}"));
        }
    }
    if manifest.terminal_owned
        && fs::symlink_metadata(roots.terminal_staged()).is_ok()
        && let Err(error) = rollback_disable_terminal(roots, manifest.terminal_had_original)
    {
        failures.push(format!("default terminal: {error}"));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{}", failures.join("; "))
    }
}

fn cleanup_staged_disable(roots: &Roots, manifest: &Manifest) -> Result<()> {
    if manifest.terminal_owned {
        fs::remove_file(roots.terminal_staged()).context("remove staged terminal preference")?;
    }
    if manifest.hypr_owned {
        fs::remove_file(roots.hypr_entrypoint_staged())
            .context("remove staged managed Hyprland entrypoint")?;
        fs::remove_file(roots.hypr_module_staged())
            .context("remove staged managed Hyprland module")?;
    }
    Ok(())
}

fn rollback_enable(
    roots: &Roots,
    runtime: &mut impl Runtime,
    manifest: &Manifest,
    output: &mut dyn Write,
) -> Result<()> {
    let mut failures = Vec::new();
    if manifest.screensaver_created
        && screensaver_ready(roots)
        && let Err(error) = runtime.screensaver(IntegrationAction::Disable, roots, output)
    {
        failures.push(format!("screensaver: {error}"));
    }
    if manifest.hypr_owned && managed_hypr_ready(roots) {
        if let Err(error) = disable_hypr(roots) {
            failures.push(format!("Hyprland files: {error}"));
        } else if let Err(error) = runtime.reload_hyprland() {
            failures.push(format!("Hyprland reload: {error}"));
        }
    }
    if manifest.terminal_owned
        && fs::symlink_metadata(roots.terminals()).is_ok()
        && let Err(error) = disable_terminal(roots, manifest.terminal_had_original)
    {
        failures.push(format!("default terminal: {error}"));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{}", failures.join("; "))
    }
}

fn status(roots: &Roots, runtime: &mut impl Runtime, output: &mut dyn Write) -> Result<()> {
    let terminal = terminal_ready(runtime);
    let hypr = managed_hypr_ready(roots) || hypr_external_ready(roots);
    let screensaver = screensaver_ready(roots);
    let managed = roots.manifest().exists();
    writeln!(
        output,
        "Omarchy integration: {}",
        if terminal && hypr && screensaver {
            "ready"
        } else {
            "not ready"
        }
    )?;
    writeln!(output, "  Default terminal  {}", ready_label(terminal))?;
    writeln!(output, "  Terminal tag      {}", ready_label(hypr))?;
    writeln!(output, "  Screensaver       {}", ready_label(screensaver))?;
    writeln!(
        output,
        "  Ownership         {}",
        if managed {
            "tracked by Splinterm"
        } else {
            "external or disabled"
        }
    )?;
    Ok(())
}

fn ready_label(ready: bool) -> &'static str {
    if ready { "ready" } else { "missing" }
}

fn print_components(
    output: &mut dyn Write,
    terminal_owned: bool,
    hypr_owned: bool,
    screensaver_owned: bool,
) -> Result<()> {
    for (name, owned) in [
        ("Default terminal", terminal_owned),
        ("Terminal tag", hypr_owned),
        ("Screensaver", screensaver_owned),
    ] {
        writeln!(
            output,
            "  {name:<13} {}",
            if owned {
                "enabled"
            } else {
                "already configured"
            }
        )?;
    }
    Ok(())
}

fn rename_no_replace(source: &Path, destination: &Path) -> Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .with_context(|| {
        format!(
            "move {} to {} without replacement",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, os::unix::fs::symlink};

    use super::*;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> (Self, Roots) {
            let root = env::temp_dir().join(format!(
                "splinterm-omarchy-integration-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let roots = Roots {
                home: root.join("home"),
                config: root.join("config"),
                state: root.join("state"),
            };
            fs::create_dir_all(roots.integration_state()).unwrap();
            fs::create_dir_all(roots.config.join("hypr")).unwrap();
            fs::create_dir_all(roots.home.join(".local/bin")).unwrap();
            fs::write(roots.hypr_entrypoint(), "-- user config\n").unwrap();
            (Self(root), roots)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FakeRuntime {
        terminal: String,
        reloads: usize,
        screensaver: bool,
        fail_reload: bool,
        fail_screensaver_disable: bool,
        terminals: Option<PathBuf>,
    }

    impl Runtime for FakeRuntime {
        fn terminal_id(&mut self) -> Result<String> {
            if self.terminal == "follow-file" {
                let path = self
                    .terminals
                    .as_ref()
                    .context("test terminal path is missing")?;
                return Ok(if fs::read_to_string(path)?.contains(DESKTOP_ID) {
                    DESKTOP_ID.to_owned()
                } else {
                    "foot.desktop".to_owned()
                });
            }
            Ok(self.terminal.clone())
        }

        fn reload_hyprland(&mut self) -> Result<()> {
            self.reloads += 1;
            if self.fail_reload {
                bail!("injected reload failure");
            }
            Ok(())
        }

        fn screensaver(
            &mut self,
            action: IntegrationAction,
            roots: &Roots,
            _output: &mut dyn Write,
        ) -> Result<()> {
            match action {
                IntegrationAction::Enable => {
                    symlink(SCREENSAVER_HELPER, roots.screensaver_launcher())?;
                    self.screensaver = true;
                }
                IntegrationAction::Disable => {
                    if self.fail_screensaver_disable {
                        bail!("injected screensaver disable failure");
                    }
                    fs::remove_file(roots.screensaver_launcher())?;
                    self.screensaver = false;
                }
                IntegrationAction::Status => {}
            }
            Ok(())
        }
    }

    #[test]
    fn terminal_preference_restores_relative_symlink_exactly() {
        let (_root, roots) = TestRoot::new();
        let target = Path::new("../dotfiles/xdg-terminals.list");
        symlink(target, roots.terminals()).unwrap();
        enable_terminal(&roots, true).unwrap();
        assert_eq!(
            fs::read_to_string(roots.terminals()).unwrap(),
            MANAGED_TERMINALS
        );
        disable_terminal(&roots, true).unwrap();
        assert_eq!(fs::read_link(roots.terminals()).unwrap(), target);
    }

    #[test]
    fn managed_hypr_module_and_marker_are_exact_and_reversible() {
        let (_root, roots) = TestRoot::new();
        enable_hypr(&roots).unwrap();
        assert!(managed_hypr_ready(&roots));
        fs::write(
            roots.hypr_entrypoint(),
            format!(
                "-- later user edit\n{}",
                fs::read_to_string(roots.hypr_entrypoint()).unwrap()
            ),
        )
        .unwrap();
        disable_hypr(&roots).unwrap();
        assert_eq!(
            fs::read_to_string(roots.hypr_entrypoint()).unwrap(),
            "-- later user edit\n-- user config\n"
        );
    }

    #[test]
    fn status_is_read_only_and_reports_each_component() {
        let (_root, roots) = TestRoot::new();
        fs::write(
            roots.config.join("hypr/external.lua"),
            format!("{EXTERNAL_HYPR_RULE}\n"),
        )
        .unwrap();
        symlink(SCREENSAVER_HELPER, roots.screensaver_launcher()).unwrap();
        let mut runtime = FakeRuntime {
            terminal: DESKTOP_ID.to_owned(),
            reloads: 0,
            screensaver: true,
            fail_reload: false,
            fail_screensaver_disable: false,
            terminals: None,
        };
        fs::remove_dir_all(roots.integration_state()).unwrap();
        let mut output = Cursor::new(Vec::new());
        status(&roots, &mut runtime, &mut output).unwrap();
        assert_eq!(runtime.reloads, 0);
        assert!(!roots.integration_state().exists());
        let text = String::from_utf8(output.into_inner()).unwrap();
        assert!(text.contains("integration: ready"));
        assert!(text.contains("Default terminal  ready"));
        assert!(text.contains("Terminal tag      ready"));
        assert!(text.contains("Screensaver       ready"));
    }

    #[test]
    fn unified_enable_and_disable_restore_every_owned_component() {
        let (_root, roots) = TestRoot::new();
        fs::write(roots.terminals(), "foot.desktop\n").unwrap();
        let original_entrypoint = fs::read(roots.hypr_entrypoint()).unwrap();
        let mut runtime = FakeRuntime {
            terminal: "follow-file".to_owned(),
            reloads: 0,
            screensaver: false,
            fail_reload: false,
            fail_screensaver_disable: false,
            terminals: Some(roots.terminals()),
        };
        let mut output = Cursor::new(Vec::new());

        enable(&roots, &mut runtime, &mut output).unwrap();
        let manifest = read_manifest(&roots).unwrap();
        assert_eq!(manifest.phase, Phase::Committed);
        assert!(manifest.terminal_owned && manifest.hypr_owned && manifest.screensaver_owned);
        assert!(terminal_ready(&mut runtime));
        assert!(managed_hypr_ready(&roots));
        assert!(screensaver_ready(&roots));

        disable(&roots, &mut runtime, &mut output).unwrap();
        assert_eq!(fs::read(roots.terminals()).unwrap(), b"foot.desktop\n");
        assert_eq!(
            fs::read(roots.hypr_entrypoint()).unwrap(),
            original_entrypoint
        );
        assert!(fs::symlink_metadata(roots.hypr_module()).is_err());
        assert!(fs::symlink_metadata(roots.screensaver_launcher()).is_err());
        assert!(fs::symlink_metadata(roots.manifest()).is_err());
        assert_eq!(runtime.reloads, 2);
    }

    #[test]
    fn reload_failure_rolls_enable_back_without_claiming_state() {
        let (_root, roots) = TestRoot::new();
        fs::write(roots.terminals(), "foot.desktop\n").unwrap();
        let mut runtime = FakeRuntime {
            terminal: "follow-file".to_owned(),
            reloads: 0,
            screensaver: false,
            fail_reload: true,
            fail_screensaver_disable: false,
            terminals: Some(roots.terminals()),
        };
        let error = enable(&roots, &mut runtime, &mut Cursor::new(Vec::new())).unwrap_err();
        assert!(
            error.to_string().contains("rolled back") || error.to_string().contains("rollback")
        );
        assert_eq!(fs::read(roots.terminals()).unwrap(), b"foot.desktop\n");
        assert!(fs::symlink_metadata(roots.hypr_module()).is_err());
        assert!(fs::symlink_metadata(roots.screensaver_launcher()).is_err());
    }

    #[test]
    fn adopted_screensaver_survives_failed_unified_enable() {
        let (_root, roots) = TestRoot::new();
        fs::write(roots.terminals(), "foot.desktop\n").unwrap();
        symlink(SCREENSAVER_HELPER, roots.screensaver_launcher()).unwrap();
        let mut runtime = FakeRuntime {
            terminal: "follow-file".to_owned(),
            reloads: 0,
            screensaver: true,
            fail_reload: true,
            fail_screensaver_disable: false,
            terminals: Some(roots.terminals()),
        };
        assert!(enable(&roots, &mut runtime, &mut Cursor::new(Vec::new())).is_err());
        assert!(screensaver_ready(&roots));
        assert!(runtime.screensaver);
    }

    #[test]
    fn disable_failure_rolls_every_staged_component_back() {
        let (_root, roots) = TestRoot::new();
        fs::write(roots.terminals(), "foot.desktop\n").unwrap();
        let mut runtime = FakeRuntime {
            terminal: "follow-file".to_owned(),
            reloads: 0,
            screensaver: false,
            fail_reload: false,
            fail_screensaver_disable: false,
            terminals: Some(roots.terminals()),
        };
        enable(&roots, &mut runtime, &mut Cursor::new(Vec::new())).unwrap();
        runtime.fail_screensaver_disable = true;
        assert!(disable(&roots, &mut runtime, &mut Cursor::new(Vec::new())).is_err());
        assert!(terminal_ready(&mut runtime));
        assert!(managed_hypr_ready(&roots));
        assert!(screensaver_ready(&roots));
        assert_eq!(read_manifest(&roots).unwrap().phase, Phase::Committed);
        assert!(fs::symlink_metadata(roots.terminal_staged()).is_err());
        assert!(fs::symlink_metadata(roots.hypr_entrypoint_staged()).is_err());
        assert!(fs::symlink_metadata(roots.hypr_module_staged()).is_err());
    }

    #[test]
    fn commented_external_rule_is_not_treated_as_ready() {
        let (_root, roots) = TestRoot::new();
        let commented = EXTERNAL_HYPR_RULE
            .lines()
            .fold(String::new(), |mut text, line| {
                use std::fmt::Write as _;
                writeln!(text, "-- {line}").unwrap();
                text
            });
        fs::write(roots.config.join("hypr/comment.lua"), commented).unwrap();
        assert!(!hypr_external_ready(&roots));
    }

    #[test]
    fn pending_manifest_blocks_guessing_recovery() {
        let (_root, roots) = TestRoot::new();
        let manifest = Manifest {
            version: 1,
            phase: Phase::Pending,
            terminal_owned: false,
            terminal_had_original: false,
            hypr_owned: false,
            screensaver_owned: false,
            screensaver_created: false,
        };
        write_manifest(&roots, &manifest).unwrap();
        let mut runtime = FakeRuntime {
            terminal: DESKTOP_ID.to_owned(),
            reloads: 0,
            screensaver: false,
            fail_reload: false,
            fail_screensaver_disable: false,
            terminals: None,
        };
        let error = run_with(
            IntegrationAction::Status,
            &roots,
            &mut runtime,
            &mut Cursor::new(Vec::new()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("recovery is required"));
    }

    #[test]
    fn external_ready_components_are_not_claimed() {
        let (_root, roots) = TestRoot::new();
        fs::write(
            roots.config.join("hypr/external.lua"),
            format!("{EXTERNAL_HYPR_RULE}\n"),
        )
        .unwrap();
        symlink(SCREENSAVER_HELPER, roots.screensaver_launcher()).unwrap();
        let mut runtime = FakeRuntime {
            terminal: DESKTOP_ID.to_owned(),
            reloads: 0,
            screensaver: true,
            fail_reload: false,
            fail_screensaver_disable: false,
            terminals: None,
        };
        let terminal_owned = !terminal_ready(&mut runtime);
        let hypr_owned = !hypr_external_ready(&roots);
        let screensaver_owned = screensaver_ready(&roots);
        assert!(!terminal_owned && !hypr_owned && screensaver_owned);
    }
}
