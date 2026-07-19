//! Emit printable-ASCII FreeType-light glyph evidence for the Foot oracle.

use std::{
    io::{self, Write},
    path::PathBuf,
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde_json::json;
use splinterm_freetype::{RasterFace, RasterizedGlyph};

fn main() -> Result<()> {
    let font_size = evidence_font_size()?;
    let style = evidence_font_style()?;
    let (path, index, family) = resolve_primary(font_size, style)?;
    let mut face = RasterFace::open(&path, index, pixel_size_26_6(font_size))?;
    let metrics = face.metrics()?;
    let stdout = io::stdout();
    let mut output = stdout.lock();

    for codepoint in 0x20_u32..=0x7e {
        let character = char::from_u32(codepoint).context("printable ASCII codepoint")?;
        let glyph_id = face.glyph_index(character);
        let glyph = face.rasterize_gray(glyph_id)?;
        let ink = ink_bounds(&glyph);
        let record = json!({
            "schema": 1,
            "label": format!("ASCII-U+{codepoint:04X}"),
            "codepoint": codepoint,
            "cols": 1,
            "glyph_id": glyph_id,
            "style": style,
            "font": family.as_str(),
            "font_path": path.display().to_string(),
            "font_index": index,
            "font_ascent": metrics.ascent,
            "font_descent": metrics.descent,
            "font_height": metrics.height,
            "decorations": {
                "underline_position": metrics.underline_position,
                "underline_thickness": metrics.underline_thickness,
                "strike_position": metrics.strike_position,
                "strike_thickness": metrics.strike_thickness,
            },
            "color": false,
            "pixel_format": "freetype-gray",
            "source_stride": glyph.width,
            "placement": {"x": glyph.left, "y": glyph.top},
            "image": {"width": glyph.width, "height": glyph.height},
            "advance": {"x": glyph.advance_x, "y": glyph.advance_y},
            "ink": ink,
            "alpha_hex": bytes_to_hex(&glyph.alpha),
        });
        serde_json::to_writer(&mut output, &record)?;
        output.write_all(b"\n")?;
    }
    Ok(())
}

fn evidence_font_size() -> Result<f32> {
    let value = std::env::var("SPLINTERM_EVIDENCE_FONT_SIZE").unwrap_or_else(|_| "22".into());
    let size: f32 = value.parse().context("parse evidence font size")?;
    if !size.is_finite() || !(6.0..=192.0).contains(&size) {
        bail!("effective evidence font size must be between 6 and 192 pixels");
    }
    Ok(size)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "validated evidence sizes fit the FreeType 26.6 policy"
)]
fn pixel_size_26_6(font_size: f32) -> isize {
    (font_size * 64.0).round() as isize
}

fn evidence_font_style() -> Result<&'static str> {
    match std::env::var("SPLINTERM_EVIDENCE_FONT_STYLE")
        .unwrap_or_else(|_| "Regular".into())
        .as_str()
    {
        "Regular" => Ok("Regular"),
        "Bold" => Ok("Bold"),
        "Italic" => Ok("Italic"),
        "Bold Italic" => Ok("Bold Italic"),
        style => bail!("unsupported evidence font style {style:?}"),
    }
}

fn resolve_primary(font_size: f32, style: &str) -> Result<(PathBuf, u32, String)> {
    let pattern = format!("JetBrains Mono Nerd Font:style={style}:pixelsize={font_size}");
    let output = Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}\n%{family}\n", &pattern])
        .output()
        .context("run fc-match")?;
    if !output.status.success() {
        bail!(
            "fc-match failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8(output.stdout).context("fc-match output is not UTF-8")?;
    let mut lines = stdout.lines();
    let path = PathBuf::from(lines.next().context("fc-match omitted font path")?);
    let index = lines
        .next()
        .context("fc-match omitted face index")?
        .parse()
        .context("fc-match returned invalid face index")?;
    let family = lines.next().context("fc-match omitted family")?.to_owned();
    Ok((path, index, family))
}

fn ink_bounds(glyph: &RasterizedGlyph) -> serde_json::Value {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..glyph.height {
        for x in 0..glyph.width {
            let index = usize::try_from(y * glyph.width + x).expect("bounded glyph index");
            if glyph.alpha[index] == 0 {
                continue;
            }
            bounds = Some(bounds.map_or((x, y, x + 1, y + 1), |current| {
                (
                    current.0.min(x),
                    current.1.min(y),
                    current.2.max(x + 1),
                    current.3.max(y + 1),
                )
            }));
        }
    }
    let (left, top, right, bottom) = bounds.unwrap_or((0, 0, 0, 0));
    json!({"left": left, "top": top, "right": right, "bottom": bottom})
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
