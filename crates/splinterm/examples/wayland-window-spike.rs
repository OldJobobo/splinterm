//! Thin launcher for the candidate native Wayland deterministic-row mechanism.
//!
//! Foot 1.27.0 at commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
//! remains the behavioral reference in the production-owned modules.

use std::{env, path::PathBuf};

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut capture = None;
    let mut capture_scale = None;
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--capture" {
            capture = Some(PathBuf::from(
                arguments.next().context("--capture requires a path")?,
            ));
        } else if argument == "--capture-scale" {
            capture_scale = Some(
                arguments
                    .next()
                    .context("--capture-scale requires an integer")?
                    .to_string_lossy()
                    .parse::<u32>()
                    .context("--capture-scale must be a positive integer")?,
            );
            if capture_scale == Some(0) {
                bail!("--capture-scale must be positive");
            }
        } else {
            bail!("unknown argument: {}", argument.to_string_lossy());
        }
    }
    splinterm::run_window(splinterm::WindowOptions {
        capture,
        snapshot: None,
        updates: None,
        commands: None,
        evidence_close_shortcuts: true,
        capture_scale,
    })
}
