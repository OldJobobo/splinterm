use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A private temporary home with an installed adapter.
struct Home {
    root: PathBuf,
    adapter: PathBuf,
}

impl Home {
    fn new() -> Self {
        Self::with_adapter_dir("bin")
    }

    fn with_adapter_dir(directory: &str) -> Self {
        let root = env::temp_dir().join(format!(
            "splinterm-mcp-setup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let bin = root.join(directory);
        fs::create_dir_all(&bin).unwrap();
        let adapter = bin.join(ADAPTER_NAME);
        fs::write(&adapter, "#!/bin/false\n").unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, adapter }
    }

    fn environment(&self) -> Environment {
        Environment {
            home: Some(self.root.clone()),
            ..Environment::default()
        }
    }

    fn adapter(&self) -> String {
        self.adapter.to_str().unwrap().to_owned()
    }

    fn setup(&self, host: McpHostArg) -> Request {
        Request {
            operation: McpSetupOperation::Setup,
            host,
            dry_run: false,
            replace: false,
            config: None,
            adapter: Some(self.adapter.clone()),
        }
    }

    fn remove(&self, host: McpHostArg) -> Request {
        Request {
            operation: McpSetupOperation::Remove,
            adapter: None,
            ..self.setup(host)
        }
    }

    fn run(&self, request: &Request) -> Result<Report> {
        execute(request, &self.environment())
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative)).unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn kind(error: &anyhow::Error) -> FailureKind {
    failure_kind(error).unwrap_or_else(|| panic!("untyped failure: {error:#}"))
}

fn claude_entry(adapter: &str) -> String {
    format!(
        "{{\n      \"type\": \"stdio\",\n      \"command\": {},\n      \"args\": [],\n      \"env\": {{}}\n    }}",
        json_config::quote(adapter)
    )
}

const CLAUDE_EXISTING: &str = r#"{
  "numStartups": 3,
  "mcpServers": {
    "other": {
      "type": "stdio",
      "command": "/usr/bin/other",
      "args": [],
      "env": {
        "TOKEN": "keep-me"
      }
    }
  },
  "projects": {
    "/work": {
      "mcpServers": {}
    }
  }
}
"#;

#[test]
fn claude_code_golden_creates_the_file_claude_mcp_add_writes() {
    let home = Home::new();
    let report = home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    assert_eq!(report.outcome.action, Action::Created);
    let expected = format!(
        "{{\n  \"mcpServers\": {{\n    \"splinterm\": {}\n  }}\n}}\n",
        claude_entry(&home.adapter())
    );
    assert_eq!(home.read(".claude.json"), expected);
    let mode = fs::metadata(home.root.join(".claude.json")).unwrap().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn claude_code_golden_adds_beside_unrelated_entries_and_keeps_them_byte_for_byte() {
    let home = Home::new();
    home.write(".claude.json", CLAUDE_EXISTING);
    let report = home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    assert_eq!(report.outcome.action, Action::Added);
    let expected = CLAUDE_EXISTING.replace(
        "        \"TOKEN\": \"keep-me\"\n      }\n    }\n  },",
        &format!(
            "        \"TOKEN\": \"keep-me\"\n      }}\n    }},\n    \"splinterm\": {}\n  }},",
            claude_entry(&home.adapter())
        ),
    );
    assert_eq!(home.read(".claude.json"), expected);
    // The project-scoped servers are not the user-scope list.
    let value: serde_json::Value = serde_json::from_str(&expected).unwrap();
    assert_eq!(
        value["projects"]["/work"]["mcpServers"],
        serde_json::json!({})
    );
}

#[test]
fn claude_code_adds_the_server_list_when_there_is_none() {
    let home = Home::new();
    home.write(".claude.json", "{\n  \"numStartups\": 3\n}\n");
    home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    let expected = format!(
        "{{\n  \"numStartups\": 3,\n  \"mcpServers\": {{\n    \"splinterm\": {}\n  }}\n}}\n",
        claude_entry(&home.adapter())
    );
    assert_eq!(home.read(".claude.json"), expected);
}

#[test]
fn claude_config_dir_selects_the_file_claude_code_reads() {
    let home = Home::new();
    let directory = home.root.join("claude-config");
    fs::create_dir(&directory).unwrap();
    let environment = Environment {
        claude_config_dir: Some(directory.clone()),
        ..home.environment()
    };
    execute(&home.setup(McpHostArg::ClaudeCode), &environment).unwrap();
    assert!(directory.join(".claude.json").is_file());
    assert!(!home.root.join(".claude.json").exists());
}

#[test]
fn codex_golden_matches_codex_mcp_add_plus_the_forwarded_variables() {
    let home = Home::new();
    let existing = "# my config\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
    home.write(".codex/config.toml", existing);
    let report = home.run(&home.setup(McpHostArg::Codex)).unwrap();
    assert_eq!(report.outcome.action, Action::Added);
    let block = format!(
        "[mcp_servers.splinterm]\ncommand = \"{}\"\nenv_vars = [\"SPLINTERM_SPLINT_ID\", \"SPLINTERM_SPLINT_INCARNATION\"]\n",
        home.adapter()
    );
    assert_eq!(
        home.read(".codex/config.toml"),
        format!("{existing}\n{block}")
    );
    assert_eq!(
        report.outcome.entry,
        Some(serde_json::json!({
            "command": home.adapter(),
            "env_vars": ["SPLINTERM_SPLINT_ID", "SPLINTERM_SPLINT_INCARNATION"],
        }))
    );
}

#[test]
fn codex_creates_its_home_directory_privately() {
    let home = Home::new();
    let report = home.run(&home.setup(McpHostArg::Codex)).unwrap();
    assert_eq!(report.outcome.action, Action::Created);
    let directory = fs::metadata(home.root.join(".codex")).unwrap();
    assert_eq!(directory.mode() & 0o777, 0o700);
    assert!(
        home.read(".codex/config.toml")
            .starts_with("[mcp_servers.splinterm]\n")
    );
}

#[test]
fn codex_home_must_already_exist() {
    let home = Home::new();
    let environment = Environment {
        codex_home: Some(home.root.join("missing")),
        ..home.environment()
    };
    let error = execute(&home.setup(McpHostArg::Codex), &environment).unwrap_err();
    assert_eq!(kind(&error), FailureKind::NotFound);
}

const VSCODE_EXISTING: &str = r#"{
  // my servers
  "servers": {
    "other": {"type": "stdio", "command": "/usr/bin/other"}, // keep
  },
  "inputs": []
}
"#;

#[test]
fn vscode_golden_keeps_comments_and_trailing_commas() {
    let home = Home::new();
    home.write(".config/Code/User/mcp.json", VSCODE_EXISTING);
    let report = home.run(&home.setup(McpHostArg::Vscode)).unwrap();
    assert_eq!(report.outcome.action, Action::Added);
    assert_eq!(report.outcome.config_format, McpConfigFormat::Jsonc);
    let expected = VSCODE_EXISTING.replace(
        "// keep\n",
        &format!(
            "// keep\n    \"splinterm\": {{\n      \"type\": \"stdio\",\n      \"command\": {},\n      \"args\": []\n    }}\n",
            json_config::quote(&home.adapter())
        ),
    );
    assert_eq!(home.read(".config/Code/User/mcp.json"), expected);
}

#[test]
fn vscode_finds_exactly_one_user_directory() {
    let home = Home::new();
    let error = home.run(&home.setup(McpHostArg::Vscode)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::NotFound);
    fs::create_dir_all(home.root.join(".config/Code - OSS/User")).unwrap();
    let report = home.run(&home.setup(McpHostArg::Vscode)).unwrap();
    assert!(
        report
            .outcome
            .config_path
            .ends_with("/.config/Code - OSS/User/mcp.json")
    );
    fs::create_dir_all(home.root.join(".config/Code/User")).unwrap();
    let error = home.run(&home.setup(McpHostArg::Vscode)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Invalid);
    let explicit = Request {
        config: Some(home.root.join(".config/Code/User/mcp.json")),
        ..home.setup(McpHostArg::Vscode)
    };
    assert_eq!(home.run(&explicit).unwrap().outcome.action, Action::Created);
}

#[test]
fn rerunning_setup_changes_nothing() {
    for (host, relative) in [
        (McpHostArg::ClaudeCode, ".claude.json"),
        (McpHostArg::Codex, ".codex/config.toml"),
        (McpHostArg::Vscode, ".config/Code/User/mcp.json"),
    ] {
        let home = Home::new();
        fs::create_dir_all(home.root.join(".config/Code/User")).unwrap();
        home.run(&home.setup(host)).unwrap();
        let path = home.root.join(relative);
        let before = fs::read(&path).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let report = home.run(&home.setup(host)).unwrap();
        assert_eq!(report.outcome.action, Action::Unchanged, "{host:?}");
        assert!(report.new_text.is_none());
        assert_eq!(fs::read(&path).unwrap(), before);
        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            (after.ino(), after.mtime(), after.mtime_nsec()),
            (metadata.ino(), metadata.mtime(), metadata.mtime_nsec())
        );
        assert!(human(&report).contains("nothing changed"));
    }
}

#[test]
fn a_different_entry_is_left_alone_unless_replace_is_given() {
    let home = Home::new();
    let custom = CLAUDE_EXISTING.replace("\"other\"", "\"splinterm\"");
    home.write(".claude.json", &custom);
    let error = home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Conflict);
    assert!(format!("{error:#}").contains("--replace"));
    assert_eq!(home.read(".claude.json"), custom);
    let replace = Request {
        replace: true,
        ..home.setup(McpHostArg::ClaudeCode)
    };
    let report = home.run(&replace).unwrap();
    assert_eq!(report.outcome.action, Action::Replaced);
    let text = home.read(".claude.json");
    assert!(text.contains(&format!(
        "\"splinterm\": {}\n  }},",
        claude_entry(&home.adapter())
    )));
    assert!(!text.contains("keep-me"));
    assert!(text.starts_with("{\n  \"numStartups\": 3,\n"));
}

#[test]
fn codex_replace_rewrites_the_table_and_its_subtables_only() {
    let home = Home::new();
    let existing = "[mcp_servers.splinterm]\ncommand = \"/old/splinterm-mcp\"\n\n[mcp_servers.splinterm.env]\nA = \"1\"\n\n# profile settings\n[profile]\nname = \"x\"\n";
    home.write(".codex/config.toml", existing);
    let error = home.run(&home.setup(McpHostArg::Codex)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Conflict);
    let replace = Request {
        replace: true,
        ..home.setup(McpHostArg::Codex)
    };
    home.run(&replace).unwrap();
    let text = home.read(".codex/config.toml");
    assert!(!text.contains("/old/"));
    assert!(!text.contains("A = \"1\""));
    assert!(text.contains("# profile settings\n[profile]\nname = \"x\"\n"));
    assert!(text.starts_with("[mcp_servers.splinterm]\ncommand = "));
}

#[test]
fn a_dry_run_writes_nothing_and_shows_the_exact_change() {
    let home = Home::new();
    home.write(".claude.json", CLAUDE_EXISTING);
    let dry = Request {
        dry_run: true,
        ..home.setup(McpHostArg::ClaudeCode)
    };
    let report = home.run(&dry).unwrap();
    assert_eq!(report.outcome.action, Action::Added);
    assert_eq!(home.read(".claude.json"), CLAUDE_EXISTING);
    let text = human(&report);
    assert!(text.starts_with("Dry run: this would add"));
    assert!(
        text.contains("@@ -10,0 +11,6 @@\n+    },\n+    \"splinterm\": {\n"),
        "{text}"
    );
    assert!(
        !text
            .lines()
            .any(|line| line.starts_with('-') && !line.starts_with("---"))
    );
    assert!(text.contains("Nothing was written."));
    assert!(text.contains(GRANTS_NOTHING));
    // No context lines: the neighbouring entry's secret is never printed.
    assert!(!text.contains("keep-me"));

    let missing = Request {
        dry_run: true,
        ..home.setup(McpHostArg::Codex)
    };
    let report = home.run(&missing).unwrap();
    assert_eq!(report.outcome.action, Action::Created);
    assert!(!home.root.join(".codex").exists());
    let removal = Request {
        dry_run: true,
        ..home.remove(McpHostArg::ClaudeCode)
    };
    home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    let before = home.read(".claude.json");
    let report = home.run(&removal).unwrap();
    assert_eq!(report.outcome.action, Action::Removed);
    assert_eq!(home.read(".claude.json"), before);
}

#[test]
fn remove_restores_the_original_file() {
    let home = Home::new();
    home.write(".claude.json", CLAUDE_EXISTING);
    home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    let report = home.run(&home.remove(McpHostArg::ClaudeCode)).unwrap();
    assert_eq!(report.outcome.action, Action::Removed);
    assert_eq!(report.outcome.entry, None);
    assert_eq!(home.read(".claude.json"), CLAUDE_EXISTING);
    assert!(human(&report).contains("revokes nothing"));
    let again = home.run(&home.remove(McpHostArg::ClaudeCode)).unwrap();
    assert_eq!(again.outcome.action, Action::Absent);
    assert_eq!(home.read(".claude.json"), CLAUDE_EXISTING);

    let codex = "# my config\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
    home.write(".codex/config.toml", codex);
    home.run(&home.setup(McpHostArg::Codex)).unwrap();
    home.run(&home.remove(McpHostArg::Codex)).unwrap();
    assert_eq!(home.read(".codex/config.toml"), codex);
}

#[test]
fn remove_handles_the_only_member_and_a_middle_member() {
    let home = Home::new();
    home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    home.run(&home.remove(McpHostArg::ClaudeCode)).unwrap();
    assert_eq!(home.read(".claude.json"), "{\n  \"mcpServers\": {}\n}\n");

    let middle = "{\n  \"mcpServers\": {\n    \"a\": {},\n    \"splinterm\": {\"command\": \"/x\"},\n    \"b\": {}\n  }\n}\n";
    home.write(".claude.json", middle);
    home.run(&home.remove(McpHostArg::ClaudeCode)).unwrap();
    assert_eq!(
        home.read(".claude.json"),
        "{\n  \"mcpServers\": {\n    \"a\": {},\n    \"b\": {}\n  }\n}\n"
    );

    home.write(".config/Code/User/mcp.json", VSCODE_EXISTING);
    home.run(&home.setup(McpHostArg::Vscode)).unwrap();
    home.run(&home.remove(McpHostArg::Vscode)).unwrap();
    assert_eq!(
        home.read(".config/Code/User/mcp.json"),
        VSCODE_EXISTING.replace("}, // keep", "} // keep")
    );

    let codex =
        "[mcp_servers.splinterm]\ncommand = \"/x\"\n\n[mcp_servers.other]\ncommand = \"y\"\n";
    home.write(".codex/config.toml", codex);
    home.run(&home.remove(McpHostArg::Codex)).unwrap();
    assert_eq!(
        home.read(".codex/config.toml"),
        "[mcp_servers.other]\ncommand = \"y\"\n"
    );
}

#[test]
fn remove_of_a_missing_file_changes_nothing() {
    let home = Home::new();
    for host in [McpHostArg::ClaudeCode, McpHostArg::Codex] {
        let report = home.run(&home.remove(host)).unwrap();
        assert_eq!(report.outcome.action, Action::Absent);
    }
    assert!(!home.root.join(".claude.json").exists());
    assert!(!home.root.join(".codex").exists());
}

#[test]
fn malformed_configuration_is_refused_and_left_alone() {
    let cases = [
        (
            McpHostArg::ClaudeCode,
            ".claude.json",
            "{\n  \"mcpServers\": {\n",
        ),
        (McpHostArg::ClaudeCode, ".claude.json", "{\"a\": 1,}\n"),
        (
            McpHostArg::ClaudeCode,
            ".claude.json",
            "// no comments\n{}\n",
        ),
        (McpHostArg::ClaudeCode, ".claude.json", "[]\n"),
        (
            McpHostArg::ClaudeCode,
            ".claude.json",
            "{\"mcpServers\": []}\n",
        ),
        (McpHostArg::Codex, ".codex/config.toml", "model = \n"),
        (McpHostArg::Codex, ".codex/config.toml", "mcp_servers = 3\n"),
        (
            McpHostArg::Vscode,
            ".config/Code/User/mcp.json",
            "{ /* open\n",
        ),
    ];
    for (host, relative, text) in cases {
        let home = Home::new();
        home.write(relative, text);
        for request in [home.setup(host), home.remove(host)] {
            let error = home.run(&request).unwrap_err();
            assert_eq!(kind(&error), FailureKind::Invalid, "{text:?}: {error:#}");
            assert!(
                format!("{error:#}").contains("Nothing was written"),
                "{error:#}"
            );
        }
        assert_eq!(home.read(relative), text);
    }
}

#[test]
fn duplicate_keys_are_refused_at_any_depth() {
    let cases = [
        (
            McpHostArg::ClaudeCode,
            ".claude.json",
            "{\"mcpServers\": {}, \"mcpServers\": {}}\n",
        ),
        (
            McpHostArg::ClaudeCode,
            ".claude.json",
            "{\"mcpServers\": {\"splinterm\": {}, \"splinterm\": {}}}\n",
        ),
        (
            McpHostArg::ClaudeCode,
            ".claude.json",
            "{\"projects\": {\"a\": 1, \"a\": 2}}\n",
        ),
        (
            McpHostArg::Codex,
            ".codex/config.toml",
            "[mcp_servers.splinterm]\ncommand = \"a\"\n[mcp_servers.splinterm]\ncommand = \"b\"\n",
        ),
    ];
    for (host, relative, text) in cases {
        let home = Home::new();
        home.write(relative, text);
        let error = home.run(&home.setup(host)).unwrap_err();
        assert_eq!(kind(&error), FailureKind::Invalid, "{text:?}");
        let message = format!("{error:#}");
        assert!(
            message.contains("duplicate") || message.contains("invalid TOML"),
            "{message}"
        );
        assert_eq!(home.read(relative), text);
    }
}

#[test]
fn a_form_setup_cannot_rewrite_safely_is_refused() {
    let home = Home::new();
    let inline = "mcp_servers = { other = { command = \"x\" } }\n";
    home.write(".codex/config.toml", inline);
    let error = home.run(&home.setup(McpHostArg::Codex)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Invalid);
    assert!(format!("{error:#}").contains("cannot be edited safely"));
    assert_eq!(home.read(".codex/config.toml"), inline);

    let dotted = "mcp_servers.splinterm.command = \"/x\"\n";
    home.write(".codex/config.toml", dotted);
    let error = home.run(&home.remove(McpHostArg::Codex)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Invalid);
    assert_eq!(home.read(".codex/config.toml"), dotted);
}

#[test]
fn symlinked_and_unsafe_targets_are_refused() {
    let home = Home::new();
    let real = home.write("dotfiles/claude.json", "{}\n");
    symlink(&real, home.root.join(".claude.json")).unwrap();
    let error = home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Invalid);
    assert!(format!("{error:#}").contains("symbolic link"));
    assert_eq!(home.read("dotfiles/claude.json"), "{}\n");
    assert!(
        fs::symlink_metadata(home.root.join(".claude.json"))
            .unwrap()
            .is_symlink()
    );
    // A dangling link is refused too, never created through.
    let home = Home::new();
    symlink(home.root.join("nowhere"), home.root.join(".claude.json")).unwrap();
    assert!(home.run(&home.setup(McpHostArg::ClaudeCode)).is_err());
    assert!(!home.root.join("nowhere").exists());

    let home = Home::new();
    let path = home.write(".claude.json", "{}\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
    let error = home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Invalid);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&path, home.root.join("second-link")).unwrap();
    let error = home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap_err();
    assert!(format!("{error:#}").contains("hard link"));
    assert_eq!(home.read(".claude.json"), "{}\n");

    let home = Home::new();
    fs::create_dir(home.root.join("shared")).unwrap();
    fs::set_permissions(home.root.join("shared"), fs::Permissions::from_mode(0o777)).unwrap();
    let request = Request {
        config: Some(home.root.join("shared/mcp.json")),
        ..home.setup(McpHostArg::ClaudeCode)
    };
    let error = home.run(&request).unwrap_err();
    assert!(format!("{error:#}").contains("writable by every user"));
    assert!(!home.root.join("shared/mcp.json").exists());

    let directory = Request {
        config: Some(home.root.join("bin")),
        ..home.setup(McpHostArg::ClaudeCode)
    };
    assert_eq!(
        kind(&home.run(&directory).unwrap_err()),
        FailureKind::Invalid
    );
    let relative = Request {
        config: Some(PathBuf::from("relative.json")),
        ..home.setup(McpHostArg::ClaudeCode)
    };
    assert_eq!(
        kind(&home.run(&relative).unwrap_err()),
        FailureKind::Invalid
    );
}

#[test]
fn the_existing_file_mode_is_kept() {
    let home = Home::new();
    let path = home.write(".claude.json", "{}\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
    let leftovers: Vec<_> = fs::read_dir(&home.root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".splinterm-"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn paths_with_spaces_and_quotes_are_escaped_for_each_format() {
    let home = Home::with_adapter_dir("opt/my \"agent\" tools\\bin");
    let adapter = home.adapter();
    assert!(adapter.contains(' ') && adapter.contains('"') && adapter.contains('\\'));
    fs::create_dir_all(home.root.join(".config/Code/User")).unwrap();
    home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap();
    let claude: serde_json::Value = serde_json::from_str(&home.read(".claude.json")).unwrap();
    assert_eq!(
        claude["mcpServers"]["splinterm"]["command"],
        adapter.as_str()
    );
    assert!(
        home.read(".claude.json")
            .contains("my \\\"agent\\\" tools\\\\bin")
    );

    home.run(&home.setup(McpHostArg::Codex)).unwrap();
    let codex: ::toml::Table = home.read(".codex/config.toml").parse().unwrap();
    assert_eq!(
        codex["mcp_servers"]["splinterm"]["command"].as_str(),
        Some(adapter.as_str())
    );
    assert!(
        home.read(".codex/config.toml")
            .contains("my \\\"agent\\\" tools\\\\bin")
    );

    home.run(&home.setup(McpHostArg::Vscode)).unwrap();
    let vscode = json_config::parse_value(
        &home.read(".config/Code/User/mcp.json"),
        json_config::Dialect::Jsonc,
    )
    .unwrap();
    assert_eq!(vscode["servers"]["splinterm"]["command"], adapter.as_str());
    for host in [
        McpHostArg::ClaudeCode,
        McpHostArg::Codex,
        McpHostArg::Vscode,
    ] {
        assert_eq!(
            home.run(&home.setup(host)).unwrap().outcome.action,
            Action::Unchanged
        );
    }
    // A control character is never written into a host file.
    let home = Home::with_adapter_dir("bad\nname");
    let error = home.run(&home.setup(McpHostArg::Codex)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::Invalid);
}

#[test]
fn a_missing_adapter_is_reported_before_anything_is_read() {
    let home = Home::new();
    home.write(".claude.json", "not json");
    let missing = Request {
        adapter: Some(home.root.join("nowhere/splinterm-mcp")),
        ..home.setup(McpHostArg::ClaudeCode)
    };
    let error = home.run(&missing).unwrap_err();
    assert_eq!(kind(&error), FailureKind::NotFound);
    assert!(format!("{error:#}").contains("splinterm-mcp package"));
    fs::set_permissions(&home.adapter, fs::Permissions::from_mode(0o644)).unwrap();
    let error = home.run(&home.setup(McpHostArg::ClaudeCode)).unwrap_err();
    assert_eq!(kind(&error), FailureKind::NotFound);
    // Remove needs no adapter.
    assert_eq!(
        kind(&home.run(&home.remove(McpHostArg::ClaudeCode)).unwrap_err()),
        FailureKind::Invalid
    );
}

#[test]
fn the_adapter_is_the_one_installed_next_to_splinterm() {
    let home = Home::new();
    let executable = home.root.join("bin/splinterm");
    let store = home.root.join("store");
    assert_eq!(adjacent_adapter(&executable, &store, &[]), home.adapter);
    let empty = home.root.join("empty/splinterm");
    let adjacent = adjacent_adapter(&empty, &store, &[]);
    assert_eq!(adjacent, home.root.join("empty/splinterm-mcp"));
    let error = resolve_adapter(Some(&adjacent), &home.environment()).unwrap_err();
    assert_eq!(kind(&error), FailureKind::NotFound);
}

#[test]
fn a_nix_store_adapter_is_named_by_its_profile_entry() {
    let home = Home::with_adapter_dir("store/abc-splinterm/bin");
    let profile = home.root.join(".nix-profile/bin");
    fs::create_dir_all(&profile).unwrap();
    symlink(&home.adapter, profile.join(ADAPTER_NAME)).unwrap();
    let other = home.root.join("other-profile");
    fs::create_dir_all(&other).unwrap();
    symlink(home.root.join("elsewhere"), other.join(ADAPTER_NAME)).unwrap();
    let executable = home.root.join("store/abc-splinterm/bin/splinterm");
    let entries = vec![other.join(ADAPTER_NAME), profile.join(ADAPTER_NAME)];
    assert_eq!(
        adjacent_adapter(&executable, &home.root.join("store"), &entries),
        profile.join(ADAPTER_NAME)
    );
    // Outside the store, the adjacent path is used as is.
    assert_eq!(
        adjacent_adapter(&executable, &home.root.join("elsewhere"), &entries),
        home.adapter
    );
    // A profile entry is a link, which resolve_adapter accepts.
    assert_eq!(
        resolve_adapter(Some(&profile.join(ADAPTER_NAME)), &home.environment()).unwrap(),
        profile.join(ADAPTER_NAME).to_str().unwrap()
    );
    let environment = Environment {
        user: Some("alice".to_owned()),
        ..home.environment()
    };
    let listed = nix_profile_entries(&environment);
    assert_eq!(
        listed[0],
        Path::new("/run/current-system/sw/bin/splinterm-mcp")
    );
    assert_eq!(
        listed[1],
        Path::new("/etc/profiles/per-user/alice/bin/splinterm-mcp")
    );
    assert_eq!(listed[4], home.root.join(".nix-profile/bin/splinterm-mcp"));
    assert_eq!(listed.len(), 6);
}

#[test]
fn setup_never_writes_policy_or_grants() {
    let home = Home::new();
    for host in [McpHostArg::ClaudeCode, McpHostArg::Codex] {
        let report = home.run(&home.setup(host)).unwrap();
        assert!(human(&report).contains(GRANTS_NOTHING));
    }
    let mut names: Vec<_> = walk(&home.root)
        .into_iter()
        .map(|path| path.strip_prefix(&home.root).unwrap().to_path_buf())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            PathBuf::from(".claude.json"),
            PathBuf::from(".codex/config.toml"),
            PathBuf::from("bin/splinterm-mcp"),
        ]
    );
}

fn walk(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn diff_shows_only_changed_lines() {
    assert_eq!(
        diff("a\nb\nc\n", "a\nB\nc\n", "/f"),
        "--- /f\n+++ /f\n@@ -2,1 +2,1 @@\n-b\n+B\n"
    );
    assert_eq!(
        diff("", "x\n", "/f"),
        "--- /f\n+++ /f\n@@ -0,0 +1,1 @@\n+x\n"
    );
}

#[test]
fn the_json_reader_rejects_what_hosts_would_misread() {
    use json_config::{Dialect, parse_value};
    for text in [
        "{\"a\": \"\\ud800\"}",
        "{\"a\": 01}",
        "{\"a\": \"tab\there\"}",
        "{\"a\": 1} trailing",
        "",
    ] {
        assert!(parse_value(text, Dialect::Json).is_err(), "{text:?}");
    }
    let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
    assert!(parse_value(&deep, Dialect::Json).is_err());
    assert_eq!(
        parse_value(
            "{\"a\": \"\\ud83d\\ude00\\n\", \"b\": [1.5e3, true, null]}",
            Dialect::Json
        )
        .unwrap(),
        serde_json::json!({"a": "\u{1f600}\n", "b": [1500.0, true, null]})
    );
    assert_eq!(
        parse_value("{/* c */ \"a\": [1,], // d\n}", Dialect::Jsonc).unwrap(),
        serde_json::json!({"a": [1]})
    );
}
