//! Emit printable-ASCII Swash glyph evidence for the Foot differential oracle.

use std::io::{self, Write};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for record in splinterm::renderer::ascii_glyph_evidence()? {
        serde_json::to_writer(&mut output, &record).context("write glyph evidence record")?;
        output
            .write_all(b"\n")
            .context("finish glyph evidence record")?;
    }
    Ok(())
}
