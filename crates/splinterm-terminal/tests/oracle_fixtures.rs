use splinterm_terminal::{Attributes, CellContent, Color, ColorSource, Terminal, TerminalConfig};

const PINNED_FOOT: &str = "3c5b584b0eafa772eb4376fb6eaf6643399e190e";

#[derive(Clone, Copy)]
struct Fixture {
    id: &'static str,
    foot_commit: &'static str,
    verification: &'static str,
    columns: usize,
    rows: usize,
    input: &'static [u8],
    cursor: ExpectedCursor,
    expected_rows: &'static [ExpectedRow],
    attribute_runs: &'static [ExpectedAttributeRun],
    expected_event_count: usize,
}

#[derive(Clone, Copy)]
struct ExpectedCursor {
    column: i32,
    row: i32,
    deferred_wrap: bool,
}

#[derive(Clone, Copy)]
struct ExpectedRow {
    text: &'static str,
    linebreak: bool,
}

#[derive(Clone, Copy)]
struct ExpectedAttributeRun {
    row: usize,
    start: usize,
    end: usize,
    attributes: ExpectedAttributes,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "fixture fields mirror independent terminal attribute semantics"
)]
#[derive(Clone, Copy, Default)]
struct ExpectedAttributes {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    reverse: bool,
    conceal: bool,
    strikethrough: bool,
    foreground: Option<ExpectedColor>,
    background: Option<ExpectedColor>,
}

#[derive(Clone, Copy)]
struct ExpectedColor {
    source: ExpectedColorSource,
    value: u32,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum ExpectedColorSource {
    Default,
    Base16,
    Base256,
    Rgb,
}

include!("data/oracle_fixture_vectors.rs");

fn new_terminal(fixture: Fixture) -> Terminal {
    Terminal::new(fixture.columns, fixture.rows, TerminalConfig::default())
}

fn advance_chunks(fixture: Fixture, chunks: &[usize]) -> Terminal {
    let mut terminal = new_terminal(fixture);
    let mut offset = 0;
    for size in chunks {
        let end = (offset + size).min(fixture.input.len());
        terminal.advance(&fixture.input[offset..end]);
        offset = end;
    }
    terminal.advance(&fixture.input[offset..]);
    terminal
}

fn color(value: Option<ExpectedColor>) -> Color {
    let Some(value) = value else {
        return Color::default();
    };
    let source = match value.source {
        ExpectedColorSource::Default => ColorSource::Default,
        ExpectedColorSource::Base16 => ColorSource::Base16,
        ExpectedColorSource::Base256 => ColorSource::Base256,
        ExpectedColorSource::Rgb => ColorSource::Rgb,
    };
    Color::new(source, value.value)
}

fn assert_attributes(actual: Attributes, expected: ExpectedAttributes, context: &str) {
    assert_eq!(actual.bold(), expected.bold, "{context}: bold");
    assert_eq!(actual.dim(), expected.dim, "{context}: dim");
    assert_eq!(actual.italic(), expected.italic, "{context}: italic");
    assert_eq!(
        actual.underline(),
        expected.underline,
        "{context}: underline"
    );
    assert_eq!(actual.blink(), expected.blink, "{context}: blink");
    assert_eq!(actual.reverse(), expected.reverse, "{context}: reverse");
    assert_eq!(actual.conceal(), expected.conceal, "{context}: conceal");
    assert_eq!(
        actual.strikethrough(),
        expected.strikethrough,
        "{context}: strikethrough"
    );
    assert_eq!(
        actual.foreground(),
        color(expected.foreground),
        "{context}: foreground"
    );
    assert_eq!(
        actual.background(),
        color(expected.background),
        "{context}: background"
    );
}

fn assert_semantic_state(terminal: &mut Terminal, fixture: Fixture) {
    let cursor = terminal.grid().cursor();
    assert_eq!(
        cursor.position().column,
        fixture.cursor.column,
        "{}: cursor column",
        fixture.id
    );
    assert_eq!(
        cursor.position().row,
        fixture.cursor.row,
        "{}: cursor row",
        fixture.id
    );
    assert_eq!(
        cursor.deferred_wrap(),
        fixture.cursor.deferred_wrap,
        "{}: deferred wrap",
        fixture.id
    );

    let mut expected_attributes =
        vec![vec![ExpectedAttributes::default(); fixture.columns]; fixture.rows];
    for run in fixture.attribute_runs {
        expected_attributes[run.row][run.start..run.end].fill(run.attributes);
    }

    for (row_index, expected_row) in fixture.expected_rows.iter().enumerate() {
        let fixture_row = i32::try_from(row_index).expect("fixture row fits in i32");
        let actual_row = terminal.grid().row(fixture_row).unwrap();
        let text = actual_row
            .cells()
            .iter()
            .map(|cell| match cell.content() {
                CellContent::Empty | CellContent::Spacer(_) => ' ',
                CellContent::Scalar(character) => character,
                CellContent::Composed(_) => {
                    panic!("{}: v1 ASCII fixture produced composed content", fixture.id)
                }
            })
            .collect::<String>();
        assert_eq!(
            text, expected_row.text,
            "{}: row {row_index} text",
            fixture.id
        );
        assert_eq!(
            actual_row.has_linebreak(),
            expected_row.linebreak,
            "{}: row {row_index} linebreak",
            fixture.id
        );
        for (column, cell) in actual_row.cells().iter().enumerate() {
            assert_attributes(
                cell.attributes(),
                expected_attributes[row_index][column],
                &format!("{}: cell {column},{row_index}", fixture.id),
            );
        }
    }
    assert_eq!(
        terminal.drain_events().count(),
        fixture.expected_event_count,
        "{}: event count",
        fixture.id
    );
}

#[test]
fn every_pinned_foot_semantic_fixture_matches_whole_and_chunked_input() {
    assert_eq!(FIXTURES.len(), 5, "the v1 Foot fixture inventory changed");

    for fixture in FIXTURES.iter().copied() {
        assert_eq!(
            fixture.foot_commit, PINNED_FOOT,
            "{}: Foot authority drift",
            fixture.id
        );
        assert_eq!(
            fixture.verification, "oracle_verified",
            "{}: fixture is not oracle verified",
            fixture.id
        );

        let whole = advance_chunks(fixture, &[fixture.input.len()]);
        for split in 0..=fixture.input.len() {
            assert_eq!(
                advance_chunks(fixture, &[split]),
                whole,
                "{}: split at byte {split}",
                fixture.id
            );
        }
        assert_eq!(
            advance_chunks(fixture, &vec![1; fixture.input.len()]),
            whole,
            "{}: bytewise input",
            fixture.id
        );

        let mut state = 0x5eed_u64;
        let mut random_chunks = Vec::new();
        let mut remaining = fixture.input.len();
        while remaining > 0 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let chunk_offset = usize::try_from(state % 7).unwrap();
            let size = 1 + chunk_offset.min(remaining - 1);
            random_chunks.push(size);
            remaining -= size;
        }
        assert_eq!(
            advance_chunks(fixture, &random_chunks),
            whole,
            "{}: deterministic chunking",
            fixture.id
        );

        let mut actual = whole;
        assert_semantic_state(&mut actual, fixture);
    }
}
