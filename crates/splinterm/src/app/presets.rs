//! Local human-facing static preset inspection and dry-run compilation.

use std::{env, fmt::Write as _, path::Path};

use anyhow::{Context, Result, bail, ensure};
use splinterm::{
    automation::Connection,
    config::{AppConfig, load_default},
    endpoint::ConnectionFactory,
    preset::{
        DojoLaunchSpec, PresetCatalog, PresetCompileContext, PresetLayoutLaunch, PresetOrientation,
    },
};

use splinterm_core::{DojoId, LairId, SplintId, TopologyRevision};
use splinterm_protocol::{
    PresetDojoLaunch, PresetLayoutLaunch as WirePresetLayoutLaunch, PresetTarget, Request,
    Response, validate_preset_materialized,
};

use super::{commands::PresetCommand, human_output::print_response};

pub(in crate::app) fn run_preset_command(command: PresetCommand) -> Result<()> {
    match command {
        PresetCommand::Check { path } => {
            if let Some(path) = path {
                let catalog = PresetCatalog::load(&path)?;
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
        PresetCommand::Run { name, cwd, dry_run } => {
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
            let compiled = catalog.compile(&name, &context)?;
            print!("{}", render_dry_run(&name, &root, &compiled));
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

#[allow(
    clippy::too_many_lines,
    reason = "preset compilation, exact context capture, one request, and response reconciliation stay adjacent"
)]
pub(in crate::app) async fn materialize_preset(
    name: String,
    cwd: Option<std::path::PathBuf>,
    config: &AppConfig,
    factory: &ConnectionFactory,
) -> Result<()> {
    ensure!(
        factory.is_local(),
        "preset materialization is currently available only to the trusted local Splinterm client"
    );
    let catalog = require_catalog(config)?;
    let mut connection = factory.connect().await?;
    let (expected_topology_revision, lair_id, dojo_id, root) =
        resolve_preset_context(&mut connection, cwd).await?;
    let editor = env::var_os("EDITOR");
    let compiled = catalog.compile(
        &name,
        &PresetCompileContext {
            root_cwd: &root,
            editor: editor.as_deref(),
            shell: config.shell.as_deref(),
            login_shell: config.login_shell,
            scrollback_lines: config.scrollback_lines,
        },
    )?;
    print!("{}", render_preview(&name, &root, &compiled));
    println!("Target   Lair {lair_id} / invoking Dojo {dojo_id}");
    println!("Creating Dojo…");
    let expected_keys = {
        fn collect(layout: &PresetLayoutLaunch, keys: &mut Vec<String>) {
            match layout {
                PresetLayoutLaunch::Pane { key, .. } => keys.push(key.as_str().to_owned()),
                PresetLayoutLaunch::Split { first, second, .. } => {
                    collect(first, keys);
                    collect(second, keys);
                }
            }
        }
        let mut keys = Vec::new();
        collect(&compiled.root, &mut keys);
        keys
    };
    let response = connection
        .request(Request::MaterializePreset {
            expected_topology_revision,
            target: PresetTarget::ExistingLair {
                lair_id,
                rename: None,
            },
            dojos: vec![wire_dojo(compiled)],
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
        dojo_ids.len() == 1,
        "preset response changed Dojo cardinality"
    );
    let mut returned_keys = panes
        .iter()
        .map(|pane| pane.key.clone())
        .collect::<Vec<_>>();
    let mut expected_keys = expected_keys;
    returned_keys.sort();
    expected_keys.sort();
    ensure!(
        returned_keys == expected_keys,
        "preset response pane mapping is incomplete"
    );
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
    let dojo_id = dojo_ids[0];
    let committed = snapshot
        .topology
        .find_dojo(dojo_id)
        .context("materialized Dojo is absent from committed topology")?;
    ensure!(
        panes.iter().all(|pane| {
            pane.dojo_id == dojo_id && committed.root.find_splint(pane.splint_id).is_some()
        }),
        "preset response pane mapping disagrees with committed topology"
    );
    print_response(response)
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
    output
}

fn render_show(catalog: &PresetCatalog, name: &str) -> Result<String> {
    let summary = catalog
        .summary(name)
        .with_context(|| format!("unknown preset {name:?}"))?;
    let mut output = String::new();
    writeln!(output, "Preset  {}", summary.name).expect("writing to String cannot fail");
    writeln!(output, "  Display  {}", summary.display_name).expect("writing to String cannot fail");
    writeln!(output, "  Panes    {}", summary.panes).expect("writing to String cannot fail");
    writeln!(output, "  Focus    {}", summary.focus).expect("writing to String cannot fail");
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
