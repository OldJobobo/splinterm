//! Local human-facing static preset inspection and dry-run compilation.

use std::{env, fmt::Write as _, path::Path};

use anyhow::{Context, Result, bail};
use splinterm::{
    config::{AppConfig, load_default},
    preset::{
        DojoLaunchSpec, PresetCatalog, PresetCompileContext, PresetLayoutLaunch, PresetOrientation,
    },
};

use super::commands::PresetCommand;

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
                bail!(
                    "preset materialization is unavailable until the atomic Milestone 6 protocol; rerun with --dry-run"
                );
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

fn render_dry_run(name: &str, root: &Path, compiled: &DojoLaunchSpec) -> String {
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
