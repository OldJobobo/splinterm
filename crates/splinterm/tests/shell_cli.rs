use std::{
    env, fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
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
        "splinterm-shell-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap()
}

fn generate(directory: &Path) -> PathBuf {
    let generated = output(
        binary()
            .env("SPLINTERM_CONFIG", directory.join("missing-config.ini"))
            .args(["preset", "shell-init", "omarchy", "--shell", "bash"]),
    );
    assert!(generated.status.success(), "{:?}", generated.stderr);
    assert!(generated.stderr.is_empty());
    let path = directory.join("omarchy.bash");
    fs::write(&path, generated.stdout).unwrap();
    path
}

fn fake_splinterm(directory: &Path) -> PathBuf {
    let path = directory.join("splinterm");
    fs::write(&path, "#!/bin/bash\nprintf '%s\\0' \"$@\" > \"$CAPTURE\"\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn invoke(directory: &Path, integration: &Path, body: &str) -> (Output, Vec<String>) {
    let capture = directory.join("argv.bin");
    let result = output(
        Command::new("/bin/bash")
            .args(["--noprofile", "--norc", "-c", body])
            .env("INTEGRATION", integration)
            .env("CAPTURE", &capture)
            .env("PATH", directory),
    );
    let argv = fs::read(&capture).map_or_else(
        |_| Vec::new(),
        |bytes| {
            bytes
                .split(|byte| *byte == 0)
                .filter(|argument| !argument.is_empty())
                .map(|argument| String::from_utf8(argument.to_vec()).unwrap())
                .collect()
        },
    );
    (result, argv)
}

#[test]
fn generated_functions_preserve_one_two_ai_and_quoted_swarm_arguments() {
    let directory = test_directory("argv");
    let integration = generate(&directory);
    fake_splinterm(&directory);

    let (one, argv) = invoke(
        &directory,
        &integration,
        "source \"$INTEGRATION\" && sdl 'codex -a on-request'",
    );
    assert!(one.status.success(), "{:?}", one.stderr);
    assert_eq!(
        argv,
        [
            "preset",
            "run",
            "omarchy.tdl",
            "--param",
            "ai=codex -a on-request"
        ]
    );

    let (two, argv) = invoke(
        &directory,
        &integration,
        "source \"$INTEGRATION\" && sdl 'opencode --auto' 'claude --permission-mode bypassPermissions'",
    );
    assert!(two.status.success(), "{:?}", two.stderr);
    assert_eq!(
        argv,
        [
            "preset",
            "run",
            "omarchy.tdl",
            "--param",
            "ai=opencode --auto",
            "--param",
            "ai2=claude --permission-mode bypassPermissions"
        ]
    );

    let (swarm, argv) = invoke(
        &directory,
        &integration,
        "source \"$INTEGRATION\" && ssl 4 'codex -s danger-full-access -a never'",
    );
    assert!(swarm.status.success(), "{:?}", swarm.stderr);
    assert_eq!(
        argv,
        [
            "preset",
            "run",
            "omarchy.tsl",
            "--param",
            "count=4",
            "--param",
            "command=codex -s danger-full-access -a never"
        ]
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn generated_functions_cover_attach_square_and_directory_set_without_tmux_names() {
    let directory = test_directory("mappings");
    let integration = generate(&directory);
    fake_splinterm(&directory);

    for (call, expected) in [
        ("s", vec!["preset", "run", "omarchy.t"]),
        ("sds", vec!["preset", "run", "omarchy.tds"]),
        (
            "sdlm c cx",
            vec![
                "preset",
                "run",
                "omarchy.tdlm",
                "--param",
                "ai=c",
                "--param",
                "ai2=cx",
            ],
        ),
    ] {
        let body = format!("source \"$INTEGRATION\" && {call}");
        let (result, argv) = invoke(&directory, &integration, &body);
        assert!(result.status.success(), "{:?}", result.stderr);
        assert_eq!(argv, expected);
    }

    let text = fs::read_to_string(integration).unwrap();
    for name in ["t", "tdl", "tds", "tdlm", "tsl", "ic", "ix", "icx"] {
        assert!(!text.contains(&format!("{name}() {{")));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn function_usage_errors_do_not_launch_splinterm() {
    let directory = test_directory("usage");
    let integration = generate(&directory);
    fake_splinterm(&directory);

    for (call, usage) in [
        ("s unexpected", "usage: s"),
        ("sdl", "usage: sdl AI [AI2]"),
        ("sds unexpected", "usage: sds"),
        ("sdlm one two three", "usage: sdlm AI [AI2]"),
        ("ssl 4", "usage: ssl COUNT COMMAND"),
    ] {
        let body = format!("source \"$INTEGRATION\" && {call}");
        let (result, argv) = invoke(&directory, &integration, &body);
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8(result.stderr).unwrap().contains(usage));
        assert!(argv.is_empty());
    }

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn sourcing_reports_every_conflict_and_defines_none_of_the_new_functions() {
    let directory = test_directory("conflict");
    let integration = generate(&directory);
    fake_splinterm(&directory);
    let executable_conflict = directory.join("sdl");
    fs::write(&executable_conflict, "#!/bin/bash\nexit 0\n").unwrap();
    fs::set_permissions(&executable_conflict, fs::Permissions::from_mode(0o755)).unwrap();

    let body = r#"
shopt -s expand_aliases
alias sds='printf existing'
ssl() { return 23; }
source "$INTEGRATION"
status=$?
[[ $status -eq 1 ]]
[[ $(type -t sds) == alias ]]
[[ $(type -t ssl) == function ]]
[[ $(type -t sdl) == file ]]
[[ -z $(type -t s 2>/dev/null) ]]
[[ -z $(type -t sdlm 2>/dev/null) ]]
"#;
    let result = output(
        Command::new("/bin/bash")
            .args(["--noprofile", "--norc", "-c", body])
            .env("INTEGRATION", &integration)
            .env("PATH", &directory),
    );
    assert!(result.status.success(), "{:?}", result.stderr);
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains("sdl (file)"));
    assert!(stderr.contains("sds (alias)"));
    assert!(stderr.contains("ssl (function)"));
    assert!(stderr.contains("not loaded"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn installer_creates_only_a_new_dedicated_file_and_never_replaces_it() {
    let directory = test_directory("install");
    let xdg = directory.join("xdg");
    let home = directory.join("home");
    fs::create_dir(&home).unwrap();
    let bashrc = home.join(".bashrc");
    fs::write(&bashrc, "# keep me\n").unwrap();

    let first = output(
        binary()
            .env("XDG_CONFIG_HOME", &xdg)
            .env("HOME", &home)
            .env("SPLINTERM_CONFIG", directory.join("missing-config.ini"))
            .args(["preset", "shell-install", "omarchy", "--shell", "bash"]),
    );
    assert!(first.status.success(), "{:?}", first.stderr);
    assert!(first.stderr.is_empty());
    let destination = xdg.join("splinterm/shell/omarchy.bash");
    let installed = fs::read(&destination).unwrap();
    assert_eq!(fs::read_to_string(&bashrc).unwrap(), "# keep me\n");
    assert_eq!(fs::metadata(&destination).unwrap().mode() & 0o777, 0o600);
    let stdout = String::from_utf8(first.stdout).unwrap();
    assert!(stdout.contains(destination.to_str().unwrap()));
    assert!(stdout.contains("startup files were not changed"));

    let second = output(
        binary()
            .env("XDG_CONFIG_HOME", &xdg)
            .env("HOME", &home)
            .args(["preset", "shell-install", "omarchy", "--shell", "bash"]),
    );
    assert!(!second.status.success());
    assert!(second.stdout.is_empty());
    assert!(
        String::from_utf8(second.stderr)
            .unwrap()
            .contains("refuse to replace")
    );
    assert_eq!(fs::read(&destination).unwrap(), installed);
    assert_eq!(fs::read_to_string(&bashrc).unwrap(), "# keep me\n");

    fs::remove_dir_all(directory).unwrap();
}
