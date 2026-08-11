use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_splinterm"))
}

fn test_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = env::temp_dir().join(format!(
        "splinterm-keymap-cli-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap()
}

#[test]
fn local_config_and_keymap_commands_resolve_relative_overlay_without_daemon() {
    let directory = test_directory("valid");
    let config = directory.join("config.ini");
    fs::write(
        &config,
        "[key-bindings]\nprofile=splinterm\nfile=keybindings.toml\nprefix-timeout-ms=750\n",
    )
    .unwrap();
    fs::write(
        directory.join("keybindings.toml"),
        r#"version = 1
inherits = "splinterm"
[[unbind]]
sequence = ["Ctrl+Shift+P"]
[[binding]]
sequence = ["Ctrl+Alt+P"]
action = "app.command-palette"
"#,
    )
    .unwrap();

    let checked = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .args(["config", "check"]),
    );
    assert!(checked.status.success(), "{:?}", checked.stderr);
    let stdout = String::from_utf8(checked.stdout).unwrap();
    assert!(stdout.contains("Configuration OK"));
    assert!(stdout.contains("Keymap   splinterm (31 bindings)"));
    assert!(stdout.contains("Prefix timeout   750 ms"));

    let shown = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .args(["keymap", "show"]),
    );
    assert!(shown.status.success(), "{:?}", shown.stderr);
    let stdout = String::from_utf8(shown.stdout).unwrap();
    assert!(stdout.contains("Ctrl+Alt+P"));
    assert!(stdout.contains("app.command-palette"));
    assert!(stdout.contains("keybindings.toml:5"));
    assert!(!stdout.contains("Ctrl+Shift+P"));

    let conflicts = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .args(["keymap", "conflicts"]),
    );
    assert!(conflicts.status.success(), "{:?}", conflicts.stderr);
    assert!(
        String::from_utf8(conflicts.stdout)
            .unwrap()
            .contains("No keymap conflicts")
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn conflicting_overlay_fails_locally_with_both_sources() {
    let directory = test_directory("conflict");
    let config = directory.join("config.ini");
    fs::write(&config, "[key-bindings]\nfile=keybindings.toml\n").unwrap();
    fs::write(
        directory.join("keybindings.toml"),
        r#"version = 1
[[binding]]
sequence = ["Ctrl+Shift+C"]
action = "clipboard.paste"
"#,
    )
    .unwrap();

    let rejected = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .args(["config", "check"]),
    );
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let stderr = String::from_utf8(rejected.stderr).unwrap();
    assert!(stderr.contains("conflicts"));
    assert!(stderr.contains("built-in profile splinterm"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn keymap_list_is_small_and_machine_flags_are_rejected() {
    let listed = output(binary().args(["keymap", "list"]));
    assert!(listed.status.success(), "{:?}", listed.stderr);
    assert_eq!(
        String::from_utf8(listed.stdout).unwrap(),
        "Built-in keymaps\n  splinterm (default)\n  omarchy-tmux\n"
    );

    let shown = output(binary().args(["keymap", "show", "omarchy-tmux"]));
    assert!(shown.status.success(), "{:?}", shown.stderr);
    let stdout = String::from_utf8(shown.stdout).unwrap();
    assert!(stdout.contains("Keymap  omarchy-tmux"));
    assert!(stdout.contains("Prefix ?"));
    assert!(stdout.contains("Prefix ["));
    assert!(stdout.contains("copy-mode.enter"));
    assert!(stdout.contains("Super+C"));
    assert!(stdout.contains("Super+V"));
    assert!(!stdout.contains("copy-mode.unavailable"));
    assert!(stdout.contains("Alt+1"));
    assert!(stdout.contains("dojo.choose"));
    assert!(stdout.contains("lair.terminate-confirmed"));
    assert!(stdout.contains("window.detach"));
    assert!(stdout.contains("Ctrl+Alt+Shift+Left"));

    let rejected = output(binary().args(["--output", "json", "keymap", "list"]));
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
}
