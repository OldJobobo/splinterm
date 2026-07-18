//! Thin launcher for the candidate native Wayland deterministic-row mechanism.
//!
//! Foot 1.27.0 at commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
//! remains the behavioral reference in the production-owned modules.

use std::{env, path::PathBuf};

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let mut arguments = env::args_os().skip(1);
    let capture = match arguments.next() {
        None => None,
        Some(flag) if flag == "--capture" => {
            Some(PathBuf::from(arguments.next().ok_or_else(|| {
                anyhow::anyhow!("--capture requires a path")
            })?))
        }
        Some(argument) => bail!("unknown argument: {}", argument.to_string_lossy()),
    };
    if let Some(argument) = arguments.next() {
        bail!("unexpected argument: {}", argument.to_string_lossy());
    }
    splinterm::run_window(splinterm::WindowOptions { capture })
}
