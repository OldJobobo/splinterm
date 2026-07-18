//! Release-mode baseline for full-grid and semantic row-damage rendering.

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let samples = std::env::args()
        .nth(1)
        .map_or(Ok(20), |value| value.parse::<usize>())
        .context("sample count must be a positive integer")?;
    let report = splinterm::renderer::phase4_benchmark_json(samples)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
