use std::{env, fs, path::PathBuf, process::Command};

const ZERO_COMMIT: &str = "0000000000000000000000000000000000000000";
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

/// Best-effort commit for diagnostics of unpackaged builds. It never reaches
/// the handoff identity, so a missing or unreadable checkout is not an error.
fn diagnostic_source_commit() -> Option<String> {
    println!("cargo:rerun-if-env-changed=SPLINTERM_SOURCE_COMMIT");
    if let Ok(declared) = env::var("SPLINTERM_SOURCE_COMMIT") {
        assert!(
            valid_commit(&declared),
            "SPLINTERM_SOURCE_COMMIT must be exactly 40 lowercase hexadecimal characters"
        );
        return Some(declared);
    }
    println!("cargo:rerun-if-changed=../../.splinterm-build-commit");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR")?);
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
    fs::read_to_string(workspace.join(".splinterm-build-commit"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| value != ARCHIVE_TOKEN)
        .or_else(|| git_output(&workspace, &["rev-parse", "--verify", "HEAD"]))
        .filter(|value| valid_commit(value))
}

fn main() {
    println!("cargo:rerun-if-env-changed=SPLINTERM_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=SPLINTERM_BUILD_PROVENANCE");

    let commit = env::var("SPLINTERM_BUILD_COMMIT").unwrap_or_else(|_| ZERO_COMMIT.to_owned());
    let provenance =
        env::var("SPLINTERM_BUILD_PROVENANCE").unwrap_or_else(|_| "unpackaged".to_owned());
    assert!(
        valid_commit(&commit),
        "SPLINTERM_BUILD_COMMIT must be exactly 40 lowercase hexadecimal characters"
    );
    assert!(
        matches!(
            provenance.as_str(),
            "unpackaged" | "local-committed-head" | "release-candidate" | "nix-store"
        ),
        "SPLINTERM_BUILD_PROVENANCE is unsupported"
    );
    assert!(
        provenance == "unpackaged" || commit != ZERO_COMMIT,
        "packaged builds require a nonzero source commit"
    );
    assert!(
        provenance != "unpackaged" || commit == ZERO_COMMIT,
        "unpackaged builds must use the reserved zero source commit; set SPLINTERM_BUILD_PROVENANCE for a packaged build"
    );

    let source_commit = if provenance == "unpackaged" {
        diagnostic_source_commit()
    } else {
        Some(commit.clone())
    };
    println!("cargo:rustc-env=SPLINTERM_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=SPLINTERM_BUILD_PROVENANCE={provenance}");
    if let Some(source_commit) = source_commit {
        println!("cargo:rustc-env=SPLINTERM_SOURCE_COMMIT={source_commit}");
    }
}
