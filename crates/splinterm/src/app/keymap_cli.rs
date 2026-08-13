//! Local human-facing configuration and keymap inspection.

use std::fmt::Write as _;

use anyhow::Result;
use splinterm::{
    config::{ConfigLoad, load_default},
    keymap::{BUILT_IN_PROFILE_NAMES, KeymapProfile, ResolvedKeymap, built_in_keymap},
};

use super::commands::{ConfigCommand, KeymapCommand};

pub(in crate::app) fn run_config_command(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Check => {
            let loaded = load_default()?;
            print!("{}", render_config_check(&loaded));
            Ok(())
        }
    }
}

pub(in crate::app) fn run_keymap_command(command: KeymapCommand) -> Result<()> {
    match command {
        KeymapCommand::List => {
            println!("Built-in keymaps");
            for profile in BUILT_IN_PROFILE_NAMES {
                let suffix = if *profile == "splinterm" {
                    " (default)"
                } else {
                    ""
                };
                println!("  {profile}{suffix}");
            }
            Ok(())
        }
        KeymapCommand::Show { profile } => {
            if let Some(profile) = profile {
                let keymap = built_in_keymap(KeymapProfile::parse(&profile)?);
                print!("{}", render_keymap(&keymap));
            } else {
                let loaded = load_default()?;
                print!("{}", render_keymap(&loaded.config.keymap));
                print!("{}", render_diagnostics(&loaded.diagnostics));
            }
            Ok(())
        }
        KeymapCommand::Conflicts => {
            let loaded = load_default()?;
            println!(
                "No keymap conflicts ({} effective bindings).",
                loaded.config.keymap.bindings().len()
            );
            print!("{}", render_diagnostics(&loaded.diagnostics));
            Ok(())
        }
    }
}

fn render_config_check(loaded: &ConfigLoad) -> String {
    let mut output = String::new();
    writeln!(output, "Configuration OK").expect("writing to String cannot fail");
    writeln!(
        output,
        "  Keymap   {} ({} bindings)",
        loaded.config.keymap.profile().name(),
        loaded.config.keymap.bindings().len()
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "  Prefix timeout   {} ms (used when prefix support is enabled)",
        loaded.config.prefix_timeout_ms
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "  Presets          {}",
        loaded
            .config
            .preset_catalog
            .as_ref()
            .map_or(0, |catalog| catalog.names().count())
    )
    .expect("writing to String cannot fail");
    if loaded.diagnostics.is_empty() {
        writeln!(output, "  Diagnostics      none").expect("writing to String cannot fail");
    } else {
        output.push_str(&render_diagnostics(&loaded.diagnostics));
    }
    output
}

fn render_diagnostics(diagnostics: &[String]) -> String {
    if diagnostics.is_empty() {
        return String::new();
    }
    let mut output = String::new();
    writeln!(output, "\nDiagnostics ({})", diagnostics.len())
        .expect("writing to String cannot fail");
    for diagnostic in diagnostics {
        writeln!(output, "  - {diagnostic}").expect("writing to String cannot fail");
    }
    output
}

fn render_keymap(keymap: &ResolvedKeymap) -> String {
    let mut output = String::new();
    writeln!(output, "Keymap  {}", keymap.profile().name()).expect("writing to String cannot fail");
    writeln!(output, "Bindings  {}", keymap.bindings().len())
        .expect("writing to String cannot fail");
    for binding in keymap.bindings() {
        writeln!(
            output,
            "  {:<24}  {:<30}  {}",
            binding.display(),
            binding.action().config_name(),
            binding.source().short_label()
        )
        .expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use splinterm::config::AppConfig;

    use super::*;

    #[test]
    fn config_check_is_calm_and_surfaces_diagnostics() {
        let loaded = ConfigLoad {
            config: AppConfig::default(),
            diagnostics: vec!["line 4: unsupported option example".to_owned()],
        };
        let rendered = render_config_check(&loaded);
        assert!(rendered.starts_with("Configuration OK\n"));
        assert!(rendered.contains("Keymap   splinterm"));
        assert!(rendered.contains("Presets          5"));
        assert!(rendered.contains("Diagnostics (1)"));
        assert!(rendered.contains("unsupported option example"));
    }

    #[test]
    fn keymap_output_is_grouped_bounded_and_source_aware() {
        let keymap = ResolvedKeymap::default();
        let rendered = render_keymap(&keymap);
        assert!(rendered.starts_with("Keymap  splinterm\n"));
        assert!(rendered.contains(&format!("Bindings  {}\n", keymap.bindings().len())));
        assert!(rendered.contains("Super+C"));
        assert!(rendered.contains("Super+V"));
        assert!(rendered.contains("Ctrl+Shift+P"));
        assert!(rendered.contains("app.command-palette"));
        assert!(rendered.contains("built-in profile splinterm"));
        assert!(!rendered.contains("UUID"));
    }
}
