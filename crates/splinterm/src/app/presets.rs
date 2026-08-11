//! Local human-facing static preset inspection and dry-run compilation.

use std::{
    env,
    ffi::OsStr,
    fmt::Write as _,
    fs,
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use splinterm::{
    automation::Connection,
    config::{AppConfig, load_default},
    endpoint::ConnectionFactory,
    preset::{
        DojoLaunchSpec, PresetCatalog, PresetCompileContext, PresetLayoutLaunch, PresetOrientation,
        PresetWorkflow,
    },
};

use splinterm_core::{DojoId, LairId, SplintId, TopologyRevision};
use splinterm_protocol::{
    PresetDirectoryIdentity, PresetDojoLaunch, PresetLayoutLaunch as WirePresetLayoutLaunch,
    PresetTarget, Request, Response, validate_preset_materialized,
};

use super::{
    commands::PresetCommand,
    human_output::print_response,
    session_catalog::{recent_dojo_ids, remember_dojo},
    sessions::launch,
    shell_integration,
    window::run_live_multipane_window,
};
use splinterm::session_picker::collect_sessions;

pub(in crate::app) fn run_preset_command(command: PresetCommand) -> Result<()> {
    match command {
        PresetCommand::Check { path } => {
            if let Some(path) = path {
                let catalog = PresetCatalog::load_user_overlay(&path)?;
                print!("{}", render_check(Some(&catalog)));
            } else {
                let loaded = load_default()?;
                print!("{}", render_check(loaded.config.preset_catalog.as_ref()));
            }
            Ok(())
        }
        PresetCommand::List => {
            let loaded = load_default()?;
            print!("{}", render_list(loaded.config.preset_catalog.as_ref()));
            Ok(())
        }
        PresetCommand::Show { name } => {
            let loaded = load_default()?;
            let catalog = require_catalog(&loaded.config)?;
            print!("{}", render_show(catalog, &name)?);
            Ok(())
        }
        PresetCommand::ShellInit { integration, shell } => {
            print!("{}", shell_integration::render(integration, shell));
            Ok(())
        }
        PresetCommand::ShellInstall { integration, shell } => {
            let path = shell_integration::install(integration, shell)?;
            println!(
                "Installed Splinterm shell integration\n  File  {}\n\nShell startup files were not changed. Review and source this file explicitly.",
                path.display()
            );
            Ok(())
        }
        PresetCommand::Run {
            name,
            cwd,
            params,
            no_open: _,
            dry_run,
        } => {
            if !dry_run {
                bail!("preset materialization must be routed through the local daemon");
            }
            let loaded = load_default()?;
            let catalog = require_catalog(&loaded.config)?;
            let root = cwd.unwrap_or(env::current_dir().context("read invocation cwd")?);
            let editor = env::var_os("EDITOR");
            let context = PresetCompileContext {
                root_cwd: &root,
                editor: editor.as_deref(),
                shell: loaded.config.shell.as_deref(),
                login_shell: loaded.config.login_shell,
                scrollback_lines: loaded.config.scrollback_lines,
            };
            match catalog.workflow(&name) {
                Some(PresetWorkflow::Attach) => {
                    ensure!(params.is_empty(), "omarchy.t does not accept parameters");
                    print!(
                        "Preset   {name}\nAction   attach first reopenable Dojo, or create Work\nRoot     {}\n\nDry run OK — no daemon connection or topology mutation.\n",
                        root.display()
                    );
                }
                Some(PresetWorkflow::DirectorySet) => {
                    let bundled = PresetCatalog::bundled();
                    let directories = immediate_child_directories(&root)?;
                    for directory in directories {
                        directory.revalidate()?;
                        let compiled = bundled.compile_with_parameters(
                            "omarchy.tdl",
                            &PresetCompileContext {
                                root_cwd: &directory.path,
                                ..context
                            },
                            &params,
                            loaded.config.allow_unrestricted_commands,
                        )?;
                        print!("{}", render_preview(&name, &directory.path, &compiled));
                    }
                    print!("\nDry run OK — no daemon connection or topology mutation.\n");
                }
                None => {
                    let compiled = catalog.compile_with_parameters(
                        &name,
                        &context,
                        &params,
                        loaded.config.allow_unrestricted_commands,
                    )?;
                    print!("{}", render_dry_run(&name, &root, &compiled));
                }
            }
            Ok(())
        }
    }
}

fn wire_layout(layout: PresetLayoutLaunch) -> WirePresetLayoutLaunch {
    match layout {
        PresetLayoutLaunch::Pane { key, title, launch } => WirePresetLayoutLaunch::Pane {
            key: key.as_str().to_owned(),
            title,
            launch,
        },
        PresetLayoutLaunch::Split {
            orientation,
            ratio,
            first,
            second,
        } => WirePresetLayoutLaunch::Split {
            axis: orientation.axis(),
            ratio,
            first: Box::new(wire_layout(*first)),
            second: Box::new(wire_layout(*second)),
        },
    }
}

fn wire_dojo(launch: DojoLaunchSpec) -> PresetDojoLaunch {
    PresetDojoLaunch {
        name: launch.name,
        focus_key: launch.focus.as_str().to_owned(),
        root: wire_layout(launch.root),
    }
}

fn environment_topology_hints() -> Result<Option<(LairId, DojoId, SplintId)>> {
    let values = [
        env::var_os("SPLINTERM_LAIR_ID"),
        env::var_os("SPLINTERM_DOJO_ID"),
        env::var_os("SPLINTERM_SPLINT_ID"),
    ];
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    ensure!(
        values.iter().all(Option::is_some),
        "Splinterm topology environment hints are incomplete"
    );
    let parse = |value: &std::ffi::OsStr, label: &str| -> Result<String> {
        value
            .to_str()
            .map(str::to_owned)
            .with_context(|| format!("{label} is not valid UTF-8"))
    };
    let [Some(lair), Some(dojo), Some(splint)] = values else {
        unreachable!("complete topology hints checked above")
    };
    Ok(Some((
        parse(&lair, "SPLINTERM_LAIR_ID")?
            .parse()
            .context("SPLINTERM_LAIR_ID is invalid")?,
        parse(&dojo, "SPLINTERM_DOJO_ID")?
            .parse()
            .context("SPLINTERM_DOJO_ID is invalid")?,
        parse(&splint, "SPLINTERM_SPLINT_ID")?
            .parse()
            .context("SPLINTERM_SPLINT_ID is invalid")?,
    )))
}

fn locate_splint(
    snapshot: &splinterm_protocol::TopologySnapshot,
    splint_id: SplintId,
) -> Option<(LairId, DojoId, &splinterm_core::Splint)> {
    snapshot.topology.lairs().find_map(|lair| {
        lair.dojos.iter().find_map(|dojo| {
            dojo.root
                .find_splint(splint_id)
                .map(|splint| (lair.id, dojo.id, splint))
        })
    })
}

async fn resolve_preset_context(
    connection: &mut Connection,
    explicit_cwd: Option<std::path::PathBuf>,
) -> Result<(TopologyRevision, LairId, DojoId, std::path::PathBuf)> {
    let Response::Topology { snapshot } = connection.request(Request::InspectTopology).await?
    else {
        bail!("splinterd did not return topology for preset context");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let selected = if let Some((lair_id, dojo_id, splint_id)) = environment_topology_hints()? {
        let (actual_lair, actual_dojo, splint) = locate_splint(&snapshot, splint_id)
            .context("hinted Splint is absent from current topology")?;
        ensure!(
            (actual_lair, actual_dojo) == (lair_id, dojo_id),
            "Splinterm topology environment hints no longer identify one exact target"
        );
        (lair_id, dojo_id, splint.cwd.clone())
    } else {
        let Response::GraphicalFocus {
            focused_splint_id: Some(splint_id),
            ..
        } = connection.request(Request::ReadGraphicalFocus).await?
        else {
            bail!(
                "no invoking Splint hints or graphical focus are available; run from a managed Splint or focus one and retry"
            );
        };
        let (lair_id, dojo_id, splint) = locate_splint(&snapshot, splint_id)
            .context("graphical focus is stale; focus a managed Splint and retry")?;
        (lair_id, dojo_id, splint.cwd.clone())
    };
    Ok((
        snapshot.revision,
        selected.0,
        selected.1,
        explicit_cwd.unwrap_or(selected.2),
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VerifiedDirectory {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl VerifiedDirectory {
    fn capture(path: PathBuf) -> Result<Self> {
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspect child directory {}", path.display()))?;
        ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "tdlm child is not a no-follow directory"
        );
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn revalidate(&self) -> Result<()> {
        let current = Self::capture(self.path.clone())?;
        ensure!(
            current.device == self.device && current.inode == self.inode,
            "tdlm child directory identity changed before materialization"
        );
        Ok(())
    }
}

struct MaterializationPlan {
    dojos: Vec<DojoLaunchSpec>,
    rename: Option<String>,
    verified_directories: Vec<VerifiedDirectory>,
}

fn compile_materialization_plan(
    catalog: &PresetCatalog,
    name: &str,
    root: &Path,
    params: &[String],
    editor: Option<&OsStr>,
    config: &AppConfig,
) -> Result<MaterializationPlan> {
    if catalog.workflow(name) == Some(PresetWorkflow::DirectorySet) {
        let bundled = PresetCatalog::bundled();
        let directories = immediate_child_directories(root)?;
        let rename = root
            .file_name()
            .and_then(OsStr::to_str)
            .filter(|name| !name.is_empty())
            .context("tdlm parent directory has no UTF-8 basename")?
            .to_owned();
        let compiled = directories
            .iter()
            .map(|directory| {
                directory.revalidate()?;
                bundled.compile_with_parameters(
                    "omarchy.tdl",
                    &compile_context(&directory.path, editor, config),
                    params,
                    config.allow_unrestricted_commands,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(MaterializationPlan {
            dojos: compiled,
            rename: Some(rename),
            verified_directories: directories,
        })
    } else {
        Ok(MaterializationPlan {
            dojos: vec![catalog.compile_with_parameters(
                name,
                &compile_context(root, editor, config),
                params,
                config.allow_unrestricted_commands,
            )?],
            rename: None,
            verified_directories: Vec::new(),
        })
    }
}

fn omarchy_attach_target(
    lairs: &[splinterm_core::Lair],
    recent: &[DojoId],
) -> Result<Option<splinterm_core::Dojo>> {
    let selected = collect_sessions(lairs, recent)
        .into_iter()
        .find(splinterm::session_picker::SessionEntry::reopenable);
    if let Some(selected) = selected {
        return lairs
            .iter()
            .find(|lair| lair.id == selected.lair_id)
            .and_then(|lair| lair.dojos.iter().find(|dojo| dojo.id == selected.dojo_id))
            .cloned()
            .map(Some)
            .context("selected reopenable Dojo disappeared from the captured topology");
    }
    ensure!(
        !lairs.iter().any(|lair| lair.name == "Work"),
        "an inactive non-reopenable Work Lair already exists; restore or rename that exact Lair before retrying"
    );
    Ok(None)
}

async fn run_omarchy_attach(
    cwd: Option<PathBuf>,
    config: &AppConfig,
    factory: &ConnectionFactory,
) -> Result<()> {
    ensure!(
        factory.is_local(),
        "omarchy.t is available only to the trusted local Splinterm client"
    );
    let mut connection = factory.connect().await?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    if let Some(dojo) = omarchy_attach_target(&lairs, &recent_dojo_ids(factory))? {
        remember_dojo(factory, dojo.id);
        drop(connection);
        return run_live_multipane_window(config.clone(), dojo, factory.clone()).await;
    }
    drop(connection);
    let cwd = cwd.unwrap_or(env::current_dir().context("read invocation cwd")?);
    launch(
        Some("Work".to_owned()),
        Some(cwd),
        None,
        true,
        Vec::new(),
        config.clone(),
        factory.clone(),
    )
    .await
}

fn immediate_child_directories(parent: &Path) -> Result<Vec<VerifiedDirectory>> {
    let mut children = fs::read_dir(parent)
        .with_context(|| format!("read child directories beneath {}", parent.display()))?
        .map(|entry| entry.context("read child directory entry"))
        .collect::<Result<Vec<_>>>()?;
    children.sort_by(|left, right| {
        left.file_name()
            .as_bytes()
            .cmp(right.file_name().as_bytes())
    });
    let mut directories = Vec::new();
    for entry in children {
        let name = entry.file_name();
        if name.as_bytes().first() == Some(&b'.') {
            continue;
        }
        if entry
            .file_type()
            .context("inspect child directory entry")?
            .is_dir()
        {
            ensure!(
                name.to_str().is_some(),
                "child directory names must be valid UTF-8"
            );
            directories.push(VerifiedDirectory::capture(entry.path())?);
        }
    }
    ensure!(
        !directories.is_empty(),
        "no immediate non-hidden child directories were found"
    );
    ensure!(
        directories.len() <= 32,
        "found {} child directories; maximum is 32",
        directories.len()
    );
    Ok(directories)
}

fn layout_root_cwd(layout: &PresetLayoutLaunch) -> &Path {
    match layout {
        PresetLayoutLaunch::Pane { launch, .. } => &launch.cwd,
        PresetLayoutLaunch::Split { first, .. } => layout_root_cwd(first),
    }
}

fn compile_context<'a>(
    root: &'a Path,
    editor: Option<&'a OsStr>,
    config: &'a AppConfig,
) -> PresetCompileContext<'a> {
    PresetCompileContext {
        root_cwd: root,
        editor,
        shell: config.shell.as_deref(),
        login_shell: config.login_shell,
        scrollback_lines: config.scrollback_lines,
    }
}

fn collect_expected_keys(layout: &PresetLayoutLaunch, keys: &mut Vec<String>) {
    match layout {
        PresetLayoutLaunch::Pane { key, .. } => keys.push(key.as_str().to_owned()),
        PresetLayoutLaunch::Split { first, second, .. } => {
            collect_expected_keys(first, keys);
            collect_expected_keys(second, keys);
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "preset compilation, exact context capture, one request, and response reconciliation stay adjacent"
)]
pub(in crate::app) async fn materialize_preset(
    name: String,
    cwd: Option<PathBuf>,
    params: Vec<String>,
    open_created: bool,
    config: &AppConfig,
    factory: &ConnectionFactory,
) -> Result<()> {
    ensure!(
        factory.is_local(),
        "preset materialization is currently available only to the trusted local Splinterm client"
    );
    let catalog = require_catalog(config)?;
    if catalog.workflow(&name) == Some(PresetWorkflow::Attach) {
        ensure!(params.is_empty(), "omarchy.t does not accept parameters");
        ensure!(open_created, "omarchy.t cannot be used with --no-open");
        return run_omarchy_attach(cwd, config, factory).await;
    }
    let mut connection = factory.connect().await?;
    let (expected_topology_revision, lair_id, dojo_id, root) =
        resolve_preset_context(&mut connection, cwd).await?;
    let editor = env::var_os("EDITOR");
    let MaterializationPlan {
        dojos: compiled,
        rename,
        verified_directories,
    } = compile_materialization_plan(catalog, &name, &root, &params, editor.as_deref(), config)?;
    for dojo in &compiled {
        print!(
            "{}",
            render_preview(&name, layout_root_cwd(&dojo.root), dojo)
        );
    }
    println!("Target   Lair {lair_id} / invoking Dojo {dojo_id}");
    if compiled.len() == 1 {
        println!("Creating Dojo…");
    } else {
        println!("Creating {} Dojos…", compiled.len());
    }
    let expected_keys = compiled
        .iter()
        .map(|dojo| {
            let mut keys = Vec::new();
            collect_expected_keys(&dojo.root, &mut keys);
            keys.sort();
            keys
        })
        .collect::<Vec<_>>();
    for directory in &verified_directories {
        directory.revalidate()?;
    }
    let directory_identities = verified_directories
        .iter()
        .map(|directory| PresetDirectoryIdentity {
            path: directory.path.clone(),
            device: directory.device,
            inode: directory.inode,
        })
        .collect();
    let response = connection
        .request(Request::MaterializePreset {
            expected_topology_revision,
            target: PresetTarget::ExistingLair { lair_id, rename },
            dojos: compiled.into_iter().map(wire_dojo).collect(),
            directory_identities,
        })
        .await?;
    let Response::PresetMaterialized {
        lair_id: committed_lair,
        ref dojo_ids,
        ref panes,
        topology_revision,
    } = response
    else {
        bail!("splinterd returned an unexpected preset response");
    };
    validate_preset_materialized(dojo_ids, panes)
        .map_err(|error| anyhow::anyhow!(error.message))?;
    ensure!(
        committed_lair == lair_id,
        "preset response changed target Lair"
    );
    ensure!(
        dojo_ids.len() == expected_keys.len(),
        "preset response changed Dojo cardinality"
    );
    for (index, dojo_id) in dojo_ids.iter().enumerate() {
        let mut returned = panes
            .iter()
            .filter(|pane| pane.dojo_id == *dojo_id)
            .map(|pane| pane.key.clone())
            .collect::<Vec<_>>();
        returned.sort();
        ensure!(
            returned == expected_keys[index],
            "preset response pane mapping is incomplete"
        );
    }
    let Response::Topology { snapshot } = connection.request(Request::InspectTopology).await?
    else {
        bail!("splinterd did not return topology after preset materialization");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    ensure!(
        snapshot.revision == topology_revision,
        "preset topology revision drifted before reconciliation"
    );
    let mut first_committed = None;
    for dojo_id in dojo_ids {
        let committed = snapshot
            .topology
            .find_dojo(*dojo_id)
            .context("materialized Dojo is absent from committed topology")?;
        if first_committed.is_none() {
            first_committed = Some(committed.clone());
        }
        ensure!(
            panes
                .iter()
                .filter(|pane| pane.dojo_id == *dojo_id)
                .all(|pane| { committed.root.find_splint(pane.splint_id).is_some() }),
            "preset response pane mapping disagrees with committed topology"
        );
    }
    print_response(response)?;
    if open_created {
        let dojo = first_committed.context("preset response contained no created Dojo")?;
        remember_dojo(factory, dojo.id);
        drop(connection);
        return run_live_multipane_window(config.clone(), dojo, factory.clone()).await;
    }
    Ok(())
}

fn require_catalog(config: &AppConfig) -> Result<&PresetCatalog> {
    config
        .preset_catalog
        .as_ref()
        .context("no preset catalog is configured; set [presets] file=presets.toml in config.ini")
}

fn render_check(catalog: Option<&PresetCatalog>) -> String {
    let count = catalog.map_or(0, |catalog| catalog.names().count());
    let mut output = String::new();
    writeln!(output, "Preset catalog OK").expect("writing to String cannot fail");
    writeln!(output, "  Presets  {count}").expect("writing to String cannot fail");
    if catalog.is_none() {
        writeln!(output, "  Source   none configured").expect("writing to String cannot fail");
    }
    output
}

fn render_list(catalog: Option<&PresetCatalog>) -> String {
    let mut output = String::from("Presets\n");
    let Some(catalog) = catalog else {
        output.push_str("  none configured\n");
        output.push_str("  Next: set [presets] file=presets.toml in config.ini\n");
        return output;
    };
    for name in catalog.names() {
        let summary = catalog
            .summary(name)
            .expect("catalog names always resolve to summaries");
        if summary.workflow.is_some() {
            writeln!(
                output,
                "  {:<28}  workflow  {}",
                summary.name, summary.display_name
            )
            .expect("writing to String cannot fail");
        } else if summary.parameterized {
            writeln!(
                output,
                "  {:<28}  parameterized (max {} panes)  {}",
                summary.name, summary.panes, summary.display_name
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                output,
                "  {:<28}  {:>2} pane{}  {}",
                summary.name,
                summary.panes,
                if summary.panes == 1 { " " } else { "s" },
                summary.display_name
            )
            .expect("writing to String cannot fail");
        }
    }
    output
}

fn render_show(catalog: &PresetCatalog, name: &str) -> Result<String> {
    let summary = catalog
        .summary(name)
        .with_context(|| format!("unknown preset {name:?}"))?;
    let mut output = String::new();
    writeln!(output, "Preset  {}", summary.name).expect("writing to String cannot fail");
    writeln!(output, "  Display  {}", summary.display_name).expect("writing to String cannot fail");
    if let Some(workflow) = summary.workflow {
        writeln!(
            output,
            "  Kind     {}",
            match workflow {
                PresetWorkflow::Attach => "attach-or-create",
                PresetWorkflow::DirectorySet => "directory-set",
            }
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(
            output,
            "  Panes    {}{}",
            if summary.parameterized { "up to " } else { "" },
            summary.panes
        )
        .expect("writing to String cannot fail");
        writeln!(output, "  Focus    {}", summary.focus).expect("writing to String cannot fail");
    }
    Ok(output)
}

fn render_preview(name: &str, root: &Path, compiled: &DojoLaunchSpec) -> String {
    let mut output = String::new();
    writeln!(output, "Preset   {name}").expect("writing to String cannot fail");
    writeln!(output, "Dojo     {}", compiled.name).expect("writing to String cannot fail");
    writeln!(output, "Root     {}", root.display()).expect("writing to String cannot fail");
    writeln!(output, "Panes    {}", compiled.root.pane_count())
        .expect("writing to String cannot fail");
    writeln!(output, "Focus    {}", compiled.focus.as_str())
        .expect("writing to String cannot fail");
    output.push_str("Layout\n");
    render_layout(&compiled.root, 1, &mut output);
    output
}

fn render_dry_run(name: &str, root: &Path, compiled: &DojoLaunchSpec) -> String {
    let mut output = render_preview(name, root, compiled);
    output.push_str("\nDry run OK — no daemon connection or topology mutation.\n");
    output
}

fn render_layout(layout: &PresetLayoutLaunch, depth: usize, output: &mut String) {
    let indent = "  ".repeat(depth);
    match layout {
        PresetLayoutLaunch::Pane { key, title, .. } => {
            writeln!(output, "{indent}pane {} ({title})", key.as_str())
                .expect("writing to String cannot fail");
        }
        PresetLayoutLaunch::Split {
            orientation,
            ratio,
            first,
            second,
        } => {
            let orientation = match orientation {
                PresetOrientation::Columns => "columns",
                PresetOrientation::Rows => "rows",
            };
            writeln!(
                output,
                "{indent}{orientation} {}/{}",
                ratio.get(),
                1_000_u16 - ratio.get()
            )
            .expect("writing to String cannot fail");
            render_layout(first, depth + 1, output);
            render_layout(second, depth + 1, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use super::*;

    const SOURCE: &str = r#"
version = 1
[commands.shell-tool]
kind = "argv"
argv = ["printf", "%s", "literal;$HOME"]
[presets.review]
kind = "dojo"
display-name = "Review workspace"
name = "{cwd.basename}"
root = "root"
focus = "left"
[presets.review.nodes.root]
type = "split"
orientation = "columns"
ratio = 600
first = "left"
second = "right"
[presets.review.nodes.left]
type = "pane"
command = "shell-tool"
cwd = "{cwd}"
[presets.review.nodes.right]
type = "pane"
shell = true
cwd = "{cwd}"
"#;

    #[test]
    fn omarchy_attach_uses_recent_reopenable_identity_and_blocks_stale_work() {
        let mut first = splinterm_core::Lair::new("first", PathBuf::from("/tmp"));
        let mut second = splinterm_core::Lair::new("second", PathBuf::from("/tmp"));
        let first_dojo = first.dojos[0].id;
        let second_dojo = second.dojos[0].id;
        let first_focus = first.dojos[0].default_focus;
        first.dojos[0]
            .root
            .find_splint_mut(first_focus)
            .unwrap()
            .state = splinterm_core::SplintState::Running;
        let second_focus = second.dojos[0].default_focus;
        second.dojos[0]
            .root
            .find_splint_mut(second_focus)
            .unwrap()
            .state = splinterm_core::SplintState::Running;
        let selected = omarchy_attach_target(&[first, second], &[second_dojo]).unwrap();
        assert_eq!(selected.unwrap().id, second_dojo);
        assert_ne!(first_dojo, second_dojo);

        let mut stale_work = splinterm_core::Lair::new("Work", PathBuf::from("/tmp"));
        let stale_focus = stale_work.dojos[0].default_focus;
        stale_work.dojos[0]
            .root
            .find_splint_mut(stale_focus)
            .unwrap()
            .state = splinterm_core::SplintState::Exited(0);
        assert!(omarchy_attach_target(&[stale_work], &[]).is_err());
        assert!(omarchy_attach_target(&[], &[]).unwrap().is_none());
    }

    #[test]
    fn tdlm_children_are_bytewise_sorted_and_exclude_hidden_non_directories() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("splinterm-tdlm-{unique}"));
        fs::create_dir_all(root.join("zeta")).unwrap();
        fs::create_dir_all(root.join("Alpha")).unwrap();
        fs::create_dir_all(root.join("beta")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join("file"), "not a directory").unwrap();
        std::os::unix::fs::symlink(root.join("beta"), root.join("linked")).unwrap();
        let children = immediate_child_directories(&root).unwrap();
        assert_eq!(
            children
                .iter()
                .map(|directory| {
                    directory
                        .path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>(),
            ["Alpha", "beta", "zeta"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tdlm_plan_renames_once_and_compiles_sorted_child_dojos() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("splinterm-tdlm-plan-{unique}"));
        fs::create_dir_all(root.join("zeta")).unwrap();
        fs::create_dir_all(root.join("alpha")).unwrap();
        let config = AppConfig::default();
        let plan = compile_materialization_plan(
            config.preset_catalog.as_ref().unwrap(),
            "omarchy.tdlm",
            &root,
            &["ai=opencode".into()],
            None,
            &config,
        )
        .unwrap();
        assert_eq!(
            plan.rename.as_deref(),
            root.file_name().and_then(OsStr::to_str)
        );
        assert_eq!(plan.verified_directories.len(), 2);
        assert!(
            plan.verified_directories
                .iter()
                .all(|directory| directory.device != 0 && directory.inode != 0)
        );
        assert_eq!(
            plan.dojos
                .iter()
                .map(|dojo| dojo.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
        assert!(plan.dojos.iter().all(|dojo| {
            dojo.focus.as_str() == "editor"
                && dojo.root.pane_count() == 3
                && layout_root_cwd(&dojo.root).ends_with(&dojo.name)
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tdlm_revalidation_rejects_directory_replaced_by_symlink() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("splinterm-tdlm-swap-{unique}"));
        let child = root.join("child");
        let replacement = root.join("replacement");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&replacement).unwrap();
        let verified = immediate_child_directories(&root)
            .unwrap()
            .into_iter()
            .find(|directory| directory.path == child)
            .unwrap();
        fs::remove_dir(&child).unwrap();
        std::os::unix::fs::symlink(&replacement, &child).unwrap();
        assert!(
            verified
                .revalidate()
                .unwrap_err()
                .to_string()
                .contains("no-follow directory")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tdlm_rejects_empty_and_more_than_thirty_two_children() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("splinterm-tdlm-bounds-{unique}"));
        fs::create_dir(&root).unwrap();
        assert!(
            immediate_child_directories(&root)
                .unwrap_err()
                .to_string()
                .contains("no immediate")
        );
        for index in 0..33 {
            fs::create_dir(root.join(format!("child-{index:02}"))).unwrap();
        }
        assert!(
            immediate_child_directories(&root)
                .unwrap_err()
                .to_string()
                .contains("maximum is 32")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_and_show_are_grouped_and_stable() {
        let catalog = PresetCatalog::parse(SOURCE).unwrap();
        let list = render_list(Some(&catalog));
        assert!(list.starts_with("Presets\n"));
        assert!(list.contains("review"));
        assert!(list.contains("2 panes"));
        let show = render_show(&catalog, "review").unwrap();
        assert!(show.starts_with("Preset  review\n"));
        assert!(show.contains("Focus    left"));
        assert!(!show.contains("$HOME"));
    }

    #[test]
    fn dry_run_preview_is_side_effect_free_and_omits_argv() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("splinterm-preview-{unique}"));
        fs::create_dir(&root).unwrap();
        let catalog = PresetCatalog::parse(SOURCE).unwrap();
        let compiled = catalog
            .compile(
                "review",
                &PresetCompileContext {
                    root_cwd: &root,
                    editor: None,
                    shell: Some("/bin/sh"),
                    login_shell: true,
                    scrollback_lines: 1_000,
                },
            )
            .unwrap();
        let preview = render_dry_run("review", &root, &compiled);
        assert!(preview.contains("columns 600/400"));
        assert!(preview.contains("pane left"));
        assert!(preview.contains("no daemon connection or topology mutation"));
        assert!(!preview.contains("printf"));
        fs::remove_dir(root).unwrap();
    }
}
