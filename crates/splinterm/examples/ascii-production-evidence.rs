//! Emit printable-ASCII evidence through the production snapshot glyph cache.

use std::io::{self, Write};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let value = std::env::var("SPLINTERM_EVIDENCE_FONT_SIZE").unwrap_or_else(|_| "22".into());
    let font_size: f32 = value.parse().context("parse evidence font size")?;
    splinterm::renderer::configure(splinterm::renderer::RendererOptions {
        font: splinterm::config::DEFAULT_FONT.into(),
        font_size: splinterm::geometry::FontSize::Pixels(font_size),
        ..splinterm::renderer::RendererOptions::default()
    })?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for record in splinterm::renderer::production_ascii_glyph_evidence()? {
        serde_json::to_writer(&mut output, &record).context("write glyph evidence record")?;
        output
            .write_all(b"\n")
            .context("finish glyph evidence record")?;
    }
    Ok(())
}
