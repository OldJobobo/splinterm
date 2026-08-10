use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_splinterm"))
}

fn test_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = env::temp_dir().join(format!(
        "splinterm-preset-cli-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap()
}

fn write_catalog(directory: &Path) -> PathBuf {
    let path = directory.join("presets.toml");
    fs::write(
        &path,
        r#"version = 1
[commands.editor]
kind = "editor-env"
fallback = ["nvim"]
append = ["."]
[commands.review]
kind = "argv"
argv = ["codex", "literal;$HOME", "*.rs"]
[presets.review]
kind = "dojo"
display-name = "Review workspace"
name = "{cwd.basename}"
root = "main"
focus = "editor"
[presets.review.nodes.main]
type = "split"
orientation = "columns"
ratio = 650
first = "editor"
second = "review"
[presets.review.nodes.editor]
type = "pane"
command = "editor"
cwd = "{cwd}"
[presets.review.nodes.review]
type = "pane"
command = "review"
cwd = "{cwd}"
"#,
    )
    .unwrap();
    path
}

#[test]
fn preset_inspection_and_dry_run_are_local_and_side_effect_free() {
    let directory = test_directory();
    let catalog = write_catalog(&directory);
    let config = directory.join("config.ini");
    fs::write(&config, "[presets]\nfile=presets.toml\n").unwrap();

    let checked = output(binary().args(["preset", "check", catalog.to_str().unwrap()]));
    assert!(checked.status.success(), "{:?}", checked.stderr);
    assert_eq!(
        String::from_utf8(checked.stdout).unwrap(),
        "Preset catalog OK\n  Presets  1\n"
    );

    let listed = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .args(["preset", "list"]),
    );
    assert!(listed.status.success(), "{:?}", listed.stderr);
    let stdout = String::from_utf8(listed.stdout).unwrap();
    assert!(stdout.starts_with("Presets\n"));
    assert!(stdout.contains("review"));
    assert!(stdout.contains("2 panes"));

    let shown = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .args(["preset", "show", "review"]),
    );
    assert!(shown.status.success(), "{:?}", shown.stderr);
    let stdout = String::from_utf8(shown.stdout).unwrap();
    assert!(stdout.contains("Display  Review workspace"));
    assert!(stdout.contains("Focus    editor"));
    assert!(!stdout.contains("$HOME"));

    let previewed = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .env_remove("EDITOR")
            .args([
                "preset",
                "run",
                "review",
                "--cwd",
                directory.to_str().unwrap(),
                "--dry-run",
            ]),
    );
    assert!(previewed.status.success(), "{:?}", previewed.stderr);
    let stdout = String::from_utf8(previewed.stdout).unwrap();
    assert!(stdout.contains("columns 650/350"));
    assert!(stdout.contains("Panes    2"));
    assert!(stdout.contains("no daemon connection or topology mutation"));
    assert!(!stdout.contains("codex"));

    let unavailable = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .args(["preset", "run", "review"]),
    );
    assert!(!unavailable.status.success());
    assert!(unavailable.stdout.is_empty());
    assert!(
        String::from_utf8(unavailable.stderr)
            .unwrap()
            .contains("atomic Milestone 6 protocol")
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unsafe_editor_is_rejected_without_echoing_its_value() {
    let directory = test_directory();
    write_catalog(&directory);
    let config = directory.join("config.ini");
    fs::write(&config, "[presets]\nfile=presets.toml\n").unwrap();
    let secret = "nvim $VERY_SECRET_VALUE";
    let rejected = output(
        binary()
            .env("SPLINTERM_CONFIG", &config)
            .env("EDITOR", secret)
            .args([
                "preset",
                "run",
                "review",
                "--cwd",
                directory.to_str().unwrap(),
                "--dry-run",
            ]),
    );
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let stderr = String::from_utf8(rejected.stderr).unwrap();
    assert!(stderr.contains("ShellMetacharacter"));
    assert!(!stderr.contains("VERY_SECRET_VALUE"));
    fs::remove_dir_all(directory).unwrap();
}
