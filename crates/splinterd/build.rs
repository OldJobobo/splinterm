use std::{env, fs, path::PathBuf, process::Command};

const ARCHIVE_TOKEN: &str = "$Format:%H$";

fn valid_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn git_output(workspace: &PathBuf, arguments: &[&str]) -> Option<String> {
    Command::new("git")
        .args(arguments)
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=SPLINTERM_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=../../.splinterm-build-commit");

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let workspace = manifest.join("../..");
    for git_path in [
        git_output(&workspace, &["rev-parse", "--git-path", "HEAD"]),
        git_output(&workspace, &["symbolic-ref", "-q", "HEAD"])
            .and_then(|reference| git_output(&workspace, &["rev-parse", "--git-path", &reference])),
    ]
    .into_iter()
    .flatten()
    {
        let git_path = PathBuf::from(git_path);
        let git_path = if git_path.is_absolute() {
            git_path
        } else {
            workspace.join(git_path)
        };
        println!("cargo:rerun-if-changed={}", git_path.display());
    }
    let declared = env::var("SPLINTERM_BUILD_COMMIT")
        .ok()
        .or_else(|| {
            fs::read_to_string(workspace.join(".splinterm-build-commit"))
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| value != ARCHIVE_TOKEN)
        })
        .or_else(|| git_output(&workspace, &["rev-parse", "--verify", "HEAD"]))
        .unwrap_or_else(|| {
            panic!(
                "SPLINTERM_BUILD_COMMIT is required outside a Git checkout or substituted source archive"
            )
        });
    assert!(
        valid_commit(&declared),
        "SPLINTERM_BUILD_COMMIT must be exactly 40 lowercase hexadecimal characters"
    );
    println!("cargo:rustc-env=SPLINTERM_BUILD_COMMIT={declared}");
}
