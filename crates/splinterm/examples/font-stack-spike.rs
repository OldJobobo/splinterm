//! Initial system-font discovery and Swash coverage/metrics spike.

use std::time::Instant;

use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use swash::FontRef;

const CORPUS: &[(&str, char)] = &[
    ("ASCII", 'A'),
    ("box drawing", '┼'),
    ("Nerd Font", '\u{f120}'),
    ("combining mark", '\u{0301}'),
    ("CJK", '界'),
    ("emoji", '🙂'),
];

fn main() {
    let started = Instant::now();
    let mut database = Database::new();
    database.load_system_fonts();
    println!(
        "Loaded {} font faces in {:.2} ms",
        database.faces().count(),
        started.elapsed().as_secs_f64() * 1_000.0
    );

    inspect(
        &database,
        "monospace regular",
        &[Family::Monospace],
        Weight::NORMAL,
        Style::Normal,
    );
    inspect(
        &database,
        "monospace bold",
        &[Family::Monospace],
        Weight::BOLD,
        Style::Normal,
    );
    inspect(
        &database,
        "monospace italic",
        &[Family::Monospace],
        Weight::NORMAL,
        Style::Italic,
    );
    inspect(
        &database,
        "emoji fallback candidate",
        &[Family::Name("Noto Color Emoji"), Family::SansSerif],
        Weight::NORMAL,
        Style::Normal,
    );
    inspect(
        &database,
        "CJK fallback candidate",
        &[Family::Name("Noto Sans CJK JP"), Family::SansSerif],
        Weight::NORMAL,
        Style::Normal,
    );
}

fn inspect(
    database: &Database,
    label: &str,
    families: &[Family<'_>],
    weight: Weight,
    style: Style,
) {
    let query = Query {
        families,
        weight,
        stretch: Stretch::Normal,
        style,
    };
    let Some(id) = database.query(&query) else {
        println!("\n{label}: NO MATCH");
        return;
    };
    let face = database.face(id).expect("queried face remains present");
    let family = face
        .families
        .first()
        .map_or("unknown", |(name, _)| name.as_str());
    println!(
        "\n{label}: {family} ({}, weight={}, style={:?}, monospaced={})",
        face.post_script_name, face.weight.0, face.style, face.monospaced
    );
    let loaded = database.with_face_data(id, |data, index| {
        let font = FontRef::from_index(data, usize::try_from(index).expect("index fits usize"))?;
        let metrics = font.metrics(&[]);
        println!(
            "  metrics: upem={} glyphs={} mono={} ascent={} descent={} avg_width={} max_width={}",
            metrics.units_per_em,
            metrics.glyph_count,
            metrics.is_monospace,
            metrics.ascent,
            metrics.descent,
            metrics.average_width,
            metrics.max_width,
        );
        let charmap = font.charmap();
        for (name, character) in CORPUS {
            let glyph = charmap.map(*character);
            println!(
                "  {name:<15} U+{:04X} {} glyph={}",
                u32::from(*character),
                if glyph == 0 { "MISS" } else { "HIT " },
                glyph,
            );
        }
        Some(())
    });
    if loaded.flatten().is_none() {
        println!("  unable to parse selected face with Swash");
    }
}
