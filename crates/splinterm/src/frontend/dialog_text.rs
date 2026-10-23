//! The text of a trusted dialog, kept as logical lines so the Window can lay
//! it out again for whatever size the compositor gives it.
//!
//! A dialog asks for one fixed size, its *natural* size: as wide as its
//! longest line (within [`NATURAL_MIN_COLUMNS`]..=[`NATURAL_MAX_COLUMNS`])
//! and tall enough that its text ends above the action areas. Tiling
//! compositors float a Window whose minimum and maximum sizes are equal. A
//! compositor that gives it another size anyway gets the same text wrapped
//! again, at spaces, to the columns that fit (see [`layout`]).

use serde::{Deserialize, Serialize};
use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, TerminalCell, TerminalInputModes, TerminalRow,
    TerminalSnapshot, UnderlineStyle,
};

/// The narrowest a dialog asks to be, in columns.
pub const NATURAL_MIN_COLUMNS: usize = 64;

/// The widest a dialog asks to be, in columns. A longer line wraps.
pub const NATURAL_MAX_COLUMNS: usize = 120;

/// The share of a dialog's natural rows its text may take: the action areas
/// cover the bottom 22%, and a little room is left between.
pub const TEXT_ROWS_PERCENT: usize = 72;

/// What shows in place of the last body row kept when a dialog too small
/// even at its smallest font must drop rows.
pub const CLIPPED_MARKER: &str = "…";

/// How a line ranks when a dialog too small to show all of its text must
/// drop rows: hint rows are dropped last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogLineRole {
    #[default]
    Body,
    /// How to answer: the key and click hints and the keyboard hint.
    Hint,
}

/// One logical line of dialog text: a paragraph the layout wraps at spaces.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogLine {
    /// The text. Leading spaces indent its first row.
    pub text: String,
    /// The columns every wrapped row after the first is indented by.
    pub indent: usize,
    /// The widest this line is laid out, in columns, even in a wider
    /// dialog; `None` for the dialog's width.
    pub wrap_at: Option<usize>,
    pub role: DialogLineRole,
    /// The access consent duration a click on any of its rows selects.
    #[serde(skip)]
    pub duration: Option<usize>,
    /// Lines in one slot take as many rows in every variant (selection) of
    /// a dialog as in its tallest, so choosing never moves the rows below.
    /// An empty line in a slot takes no row; it only marks where the slot
    /// is.
    #[serde(skip)]
    pub slot: Option<u8>,
}

impl DialogLine {
    /// A body line.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// An empty line.
    #[must_use]
    pub fn blank() -> Self {
        Self::default()
    }

    /// A line that says how to answer, dropped last when rows must go.
    #[must_use]
    pub fn hint(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: DialogLineRole::Hint,
            ..Self::default()
        }
    }

    /// Indents its wrapped rows by `columns`.
    #[must_use]
    pub const fn with_indent(mut self, columns: usize) -> Self {
        self.indent = columns;
        self
    }

    /// Wraps it at `columns` at most, however wide the dialog is.
    #[must_use]
    pub const fn with_wrap_at(mut self, columns: usize) -> Self {
        self.wrap_at = Some(columns);
        self
    }

    /// Marks every row of it as selecting the duration at `index`.
    #[must_use]
    pub const fn with_duration(mut self, index: usize) -> Self {
        self.duration = Some(index);
        self
    }

    /// Puts it in `slot` (see [`DialogLine::slot`]).
    #[must_use]
    pub const fn in_slot(mut self, slot: u8) -> Self {
        self.slot = Some(slot);
        self
    }
}

/// A dialog's whole text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogText {
    pub lines: Vec<DialogLine>,
    /// The fewest grid rows the dialog asks for.
    pub minimum_rows: usize,
}

/// What a dialog's snapshot is called and which Splint identity it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogIdentity {
    pub title: String,
    pub splint_id: SplintId,
    pub incarnation: u64,
}

impl Default for DialogIdentity {
    fn default() -> Self {
        Self {
            title: String::new(),
            splint_id: SplintId::new(),
            incarnation: 0,
        }
    }
}

/// `line` wrapped into rows of at most `columns` characters, or of its own
/// [`DialogLine::wrap_at`] when that is narrower. It breaks at spaces and
/// keeps the spaces between words on one row; only a word wider than a whole
/// row is split. An empty line is one empty row.
#[must_use]
pub fn wrap_line(line: &DialogLine, columns: usize) -> Vec<String> {
    let width = line
        .wrap_at
        .map_or(columns, |wrap| wrap.min(columns))
        .max(1);
    // A hanging indent never takes more than half a row.
    let indent = line.indent.min(width / 2);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_chars = 0;
    let mut row_has_word = false;
    let mut gap = 0;
    let mut word = Vec::new();
    let mut words = Vec::new();
    for character in line.text.chars() {
        if character == ' ' {
            if !word.is_empty() {
                words.push((gap, std::mem::take(&mut word)));
                gap = 0;
            }
            gap += 1;
        } else {
            word.push(character);
        }
    }
    if !word.is_empty() {
        words.push((gap, word));
    }
    for (gap, mut word) in words {
        loop {
            // The first row keeps its leading spaces; a wrapped row starts
            // at the indent instead.
            let mut separator = if row_has_word || rows.is_empty() {
                gap
            } else {
                0
            };
            if row_chars + separator + word.len() <= width {
                row.extend(std::iter::repeat_n(' ', separator));
                row.extend(word.iter());
                row_chars += separator + word.len();
                row_has_word = true;
                break;
            }
            if row_has_word {
                rows.push(std::mem::take(&mut row));
                row.extend(std::iter::repeat_n(' ', indent));
                row_chars = indent;
                row_has_word = false;
                continue;
            }
            // A word that fits a row once its leading spaces go keeps them
            // out; only a word wider than any row is split.
            if row_chars + word.len() <= width {
                row.extend(word.iter());
                row_chars += word.len();
                row_has_word = true;
                break;
            }
            if row_chars + separator >= width {
                separator = 0;
            }
            let room = width.saturating_sub(row_chars + separator).max(1);
            row.extend(std::iter::repeat_n(' ', separator));
            row.extend(word.drain(..room.min(word.len())));
            rows.push(std::mem::take(&mut row));
            row.extend(std::iter::repeat_n(' ', indent));
            row_chars = indent;
            row_has_word = false;
            if word.is_empty() {
                break;
            }
        }
    }
    if row_has_word || rows.is_empty() {
        if !row_has_word {
            row.clear();
        }
        rows.push(row);
    }
    rows
}

/// One wrapped row and where it came from.
struct LaidRow {
    text: String,
    role: DialogLineRole,
    duration: Option<usize>,
}

/// How many rows each slot takes in `text` at `columns`.
fn slot_rows(text: &DialogText, columns: usize) -> Vec<(u8, usize)> {
    let mut slots: Vec<(u8, usize)> = Vec::new();
    for line in &text.lines {
        let Some(slot) = line.slot else { continue };
        let rows = if line.text.trim().is_empty() {
            0
        } else {
            wrap_line(line, columns).len()
        };
        match slots.iter_mut().find(|(id, _)| *id == slot) {
            Some((_, total)) => *total += rows,
            None => slots.push((slot, rows)),
        }
    }
    slots
}

/// Every slot's tallest row count over `variants` at `columns`.
fn tallest_slots(variants: &[DialogText], columns: usize) -> Vec<(u8, usize)> {
    let mut tallest: Vec<(u8, usize)> = Vec::new();
    for variant in variants {
        for (slot, rows) in slot_rows(variant, columns) {
            match tallest.iter_mut().find(|(id, _)| *id == slot) {
                Some((_, most)) => *most = (*most).max(rows),
                None => tallest.push((slot, rows)),
            }
        }
    }
    tallest
}

/// `text` wrapped at `columns`, each slot padded to `tallest`.
fn wrapped_rows(text: &DialogText, columns: usize, tallest: &[(u8, usize)]) -> Vec<LaidRow> {
    let mut rows = Vec::new();
    let mut used: Vec<(u8, usize)> = Vec::new();
    for (index, line) in text.lines.iter().enumerate() {
        let anchor_only = line.slot.is_some() && line.text.trim().is_empty();
        if !anchor_only {
            for row in wrap_line(line, columns) {
                rows.push(LaidRow {
                    text: row,
                    role: line.role,
                    duration: line.duration,
                });
            }
        }
        let Some(slot) = line.slot else { continue };
        let taken = if anchor_only {
            0
        } else {
            wrap_line(line, columns).len()
        };
        match used.iter_mut().find(|(id, _)| *id == slot) {
            Some((_, total)) => *total += taken,
            None => used.push((slot, taken)),
        }
        // After the slot's last line, blank rows fill it to its tallest.
        let last_of_slot = !text.lines[index + 1..]
            .iter()
            .any(|later| later.slot == Some(slot));
        if last_of_slot {
            let most = tallest
                .iter()
                .find_map(|(id, rows)| (*id == slot).then_some(*rows))
                .unwrap_or(0);
            let have = used
                .iter()
                .find_map(|(id, rows)| (*id == slot).then_some(*rows))
                .unwrap_or(0);
            for _ in have..most {
                rows.push(LaidRow {
                    text: String::new(),
                    role: line.role,
                    duration: None,
                });
            }
        }
    }
    rows
}

/// How many rows of text the tallest of `variants` takes at `columns`.
#[must_use]
pub fn text_rows(variants: &[DialogText], columns: usize) -> usize {
    let tallest = tallest_slots(variants, columns);
    variants
        .iter()
        .map(|variant| wrapped_rows(variant, columns, &tallest).len())
        .max()
        .unwrap_or(0)
}

/// The columns a dialog asks for: its widest row, laid out no wider than
/// [`NATURAL_MAX_COLUMNS`], within
/// [`NATURAL_MIN_COLUMNS`]..=[`NATURAL_MAX_COLUMNS`].
#[must_use]
pub fn natural_columns(variants: &[DialogText]) -> usize {
    let tallest = tallest_slots(variants, NATURAL_MAX_COLUMNS);
    variants
        .iter()
        .flat_map(|variant| wrapped_rows(variant, NATURAL_MAX_COLUMNS, &tallest))
        .map(|row| row.text.chars().count())
        .max()
        .unwrap_or(1)
        .clamp(NATURAL_MIN_COLUMNS, NATURAL_MAX_COLUMNS)
}

/// The grid rows a dialog asks for at [`natural_columns`]: enough that its
/// text takes at most [`TEXT_ROWS_PERCENT`] of them, and at least its
/// [`DialogText::minimum_rows`].
#[must_use]
pub fn natural_rows(variants: &[DialogText]) -> usize {
    let rows = text_rows(variants, natural_columns(variants));
    let minimum = variants
        .iter()
        .map(|variant| variant.minimum_rows)
        .max()
        .unwrap_or(0);
    (rows * 100).div_ceil(TEXT_ROWS_PERCENT).max(minimum).max(1)
}

/// One laid-out rendering of a dialog's text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogLayout {
    /// The rows of text, top to bottom.
    pub rows: Vec<String>,
    /// Every row that shows a duration, with that duration's index.
    pub duration_rows: Vec<(usize, usize)>,
    /// Whether rows were dropped because they did not fit.
    pub clipped: bool,
}

/// Lays out `variants[selected]` at `columns`, in at most `max_rows` rows.
///
/// Slots keep the rows of every variant in the same places. When the text
/// needs more than `max_rows` rows, the body is cut from its end and its last
/// kept row becomes [`CLIPPED_MARKER`], so every hint row stays; the hint
/// rows are cut from their end only when they alone do not fit. Rows keep
/// their order either way.
#[must_use]
pub fn layout(
    variants: &[DialogText],
    selected: usize,
    columns: usize,
    max_rows: usize,
) -> DialogLayout {
    layout_with_notice(variants, selected, columns, max_rows, None)
}

/// [`layout`], except that when the text does not fit, `notice` (when
/// given) takes the place of every hint row: a dialog that hides part of
/// its text says so instead of saying how to say yes.
#[must_use]
pub fn layout_with_notice(
    variants: &[DialogText],
    selected: usize,
    columns: usize,
    max_rows: usize,
    notice: Option<&str>,
) -> DialogLayout {
    let Some(text) = variants.get(selected).or_else(|| variants.first()) else {
        return DialogLayout::default();
    };
    let tallest = tallest_slots(variants, columns);
    let mut rows = wrapped_rows(text, columns, &tallest);
    let clipped = rows.len() > max_rows;
    if let Some(notice) = notice.filter(|_| clipped) {
        rows.retain(|row| row.role != DialogLineRole::Hint);
        rows.extend(
            wrap_line(&DialogLine::hint(notice), columns)
                .into_iter()
                .map(|text| LaidRow {
                    text,
                    role: DialogLineRole::Hint,
                    duration: None,
                }),
        );
    }
    let keep: Vec<bool> = if rows.len() > max_rows {
        let hints = rows
            .iter()
            .filter(|row| row.role == DialogLineRole::Hint)
            .count();
        let mut body_room = max_rows.saturating_sub(hints);
        let mut hint_room = max_rows.min(hints);
        rows.iter()
            .map(|row| {
                let room = if row.role == DialogLineRole::Hint {
                    &mut hint_room
                } else {
                    &mut body_room
                };
                let kept = *room > 0;
                *room = room.saturating_sub(1);
                kept
            })
            .collect()
    } else {
        vec![true; rows.len()]
    };
    // The marker shows only where body rows were dropped.
    let body_dropped = rows
        .iter()
        .zip(&keep)
        .any(|(row, kept)| !*kept && row.role == DialogLineRole::Body);
    let last_body = body_dropped
        .then(|| {
            rows.iter()
                .zip(&keep)
                .rposition(|(row, kept)| *kept && row.role == DialogLineRole::Body)
        })
        .flatten();
    let mut laid = DialogLayout {
        clipped,
        ..DialogLayout::default()
    };
    for (index, (row, kept)) in rows.into_iter().zip(keep).enumerate() {
        if !kept {
            continue;
        }
        if Some(index) == last_body {
            laid.rows.push(CLIPPED_MARKER.to_owned());
            continue;
        }
        if let Some(duration) = row.duration {
            laid.duration_rows.push((laid.rows.len(), duration));
        }
        laid.rows.push(row.text);
    }
    laid
}

/// `variants[selected]` laid out at its natural size, every row shown.
#[must_use]
pub fn natural_layout(variants: &[DialogText], selected: usize) -> DialogLayout {
    layout(variants, selected, natural_columns(variants), usize::MAX)
}

fn dialog_input_modes() -> TerminalInputModes {
    TerminalInputModes {
        application_cursor: false,
        application_keypad: false,
        focus_reporting: false,
        bracketed_paste: false,
        cursor_visible: false,
        cursor_blink: false,
        mouse_tracking: splinterm_protocol::MouseTracking::None,
        sgr_mouse: false,
    }
}

/// A static, cursorless snapshot of `rows` in a `columns` by `grid_rows`
/// grid, in the trusted dialog colors. The grid grows to hold every row;
/// blank rows fill the rest.
#[must_use]
pub fn dialog_snapshot(
    rows: Vec<String>,
    columns: usize,
    grid_rows: usize,
    identity: &DialogIdentity,
) -> TerminalSnapshot {
    let columns = rows
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0)
        .max(columns)
        .max(1);
    let grid_rows = rows.len().max(grid_rows).max(1);
    let attributes = CellAttributes {
        bold: false,
        dim: false,
        italic: false,
        underline: UnderlineStyle::None,
        underline_color_source: ColorSource::Default,
        underline_color: 0,
        strikethrough: false,
        blink: false,
        conceal: false,
        reverse: false,
        foreground_source: ColorSource::Default,
        foreground: 0,
        background_source: ColorSource::Default,
        background: 0,
    };
    let mut visible_rows: Vec<_> = rows
        .into_iter()
        .map(|row| TerminalRow {
            row_id: None,
            linebreak: false,
            cells: row
                .chars()
                .map(|character| TerminalCell {
                    content: character.to_string(),
                    spacer_remaining: None,
                    attributes,
                })
                .collect(),
        })
        .collect();
    visible_rows.resize_with(grid_rows, || TerminalRow {
        row_id: None,
        linebreak: false,
        cells: Vec::new(),
    });
    // A Window refuses a snapshot whose visible rows lack distinct, nonzero
    // identities (`TerminalSnapshot::validate`), before it maps.
    for (row, id) in visible_rows.iter_mut().zip(1_u64..) {
        row.row_id = Some(id);
    }
    TerminalSnapshot {
        splint_id: identity.splint_id,
        incarnation: identity.incarnation,
        revision: 1,
        columns,
        rows: grid_rows,
        cursor_column: -1,
        cursor_row: -1,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: dialog_input_modes(),
        palette: vec![0; 256],
        default_colors: [0x00f4_f0e8, 0x0014_1820, 0x00e0_a030],
        title: identity.title.clone(),
        visible_rows,
        history_generation: 1,
        oldest_available_scrollback_row_id: None,
        newest_available_scrollback_row_id: None,
        scrollback_rows: Vec::new(),
        available_scrollback_rows: 0,
        omitted_oldest_scrollback_rows: 0,
        images: None,
        exited_code: None,
        exited_signal: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: Vec<DialogLine>) -> Vec<DialogText> {
        vec![DialogText {
            lines,
            minimum_rows: 0,
        }]
    }

    #[test]
    fn rewrapping_keeps_words_whole_and_paragraphs_apart() {
        let line = DialogLine::new(
            "Proceeding closes these Windows. Their terminals keep running in the upgraded daemon.",
        );
        for columns in [20, 31, 47, 64] {
            let rows = wrap_line(&line, columns);
            for row in &rows {
                assert!(row.chars().count() <= columns, "{columns}: {row:?}");
                assert!(!row.starts_with(' ') && !row.ends_with(' '), "{row:?}");
            }
            // Joined back, every word is there, whole and in order.
            assert_eq!(rows.join(" "), line.text, "{columns}");
        }
        // Two paragraphs never run into one row.
        let laid = layout(
            &text(vec![
                DialogLine::new("first one"),
                DialogLine::new("second"),
            ]),
            0,
            40,
            usize::MAX,
        );
        assert_eq!(laid.rows, ["first one", "second"]);
    }

    #[test]
    fn wrapping_keeps_inner_spaces_indents_and_splits_only_overlong_words() {
        let key = DialogLine::hint("[D / Esc / Enter  Cancel]     [G  Proceed]");
        assert_eq!(wrap_line(&key, 80), [key.text.clone()]);
        assert_eq!(
            wrap_line(&key, 30),
            ["[D / Esc / Enter  Cancel]", "[G  Proceed]"]
        );
        let program = DialogLine::new("Program: /usr/bin/splinterm-mcp --stdio").with_indent(9);
        assert_eq!(
            wrap_line(&program, 31),
            ["Program: /usr/bin/splinterm-mcp", "         --stdio"]
        );
        // The leading spaces of the first row stay.
        let bullet = DialogLine::new("  • PID 5 — did not answer").with_indent(4);
        assert_eq!(
            wrap_line(&bullet, 16),
            ["  • PID 5 — did", "    not answer"]
        );
        // A path wider than a row is split, and only then.
        let path = DialogLine::new(format!("Path: /{}", "p".repeat(30))).with_indent(4);
        let rows = wrap_line(&path, 20);
        assert_eq!(rows[0], "Path:");
        assert!(rows.iter().all(|row| row.chars().count() <= 20), "{rows:?}");
        assert_eq!(
            rows.iter().map(|row| row.trim()).collect::<String>(),
            format!("Path:/{}", "p".repeat(30))
        );
        // Its own narrower width wins over the dialog's.
        let narrow = DialogLine::new("one two three").with_wrap_at(8);
        assert_eq!(wrap_line(&narrow, 100), ["one two", "three"]);
        assert_eq!(wrap_line(&DialogLine::blank(), 10), [""]);
        assert_eq!(wrap_line(&DialogLine::new("   "), 10), [""]);
    }

    #[test]
    fn natural_size_holds_the_widest_line_and_leaves_the_action_rows() {
        let variants = vec![DialogText {
            lines: vec![
                DialogLine::new("TITLE"),
                DialogLine::new("x".repeat(80)),
                DialogLine::new("y ".repeat(90)).with_wrap_at(100),
            ],
            minimum_rows: 0,
        }];
        assert_eq!(natural_columns(&variants), 99);
        let rows = text_rows(&variants, 100);
        assert_eq!(rows, 4);
        assert_eq!(natural_rows(&variants), (4 * 100_usize).div_ceil(72));
        assert!(rows * 100 <= natural_rows(&variants) * TEXT_ROWS_PERCENT);
        // Short text still asks for the narrowest width and its minimum rows.
        let short = vec![DialogText {
            lines: vec![DialogLine::new("hi")],
            minimum_rows: 18,
        }];
        assert_eq!(natural_columns(&short), NATURAL_MIN_COLUMNS);
        assert_eq!(natural_rows(&short), 18);
        // Nothing is ever cut at the natural size.
        let laid = natural_layout(&variants, 0);
        assert!(!laid.clipped);
        assert_eq!(laid.rows.len(), rows);
    }

    #[test]
    fn slots_keep_every_variant_on_the_same_rows_at_any_width() {
        let variant = |grant: &str, shared: &str| DialogText {
            lines: vec![
                DialogLine::new("( ) one").with_duration(0),
                DialogLine::new("( ) two words here").with_duration(1),
                DialogLine::new(shared).in_slot(0),
                DialogLine::new(grant).in_slot(1),
                DialogLine::hint("hint"),
            ],
            minimum_rows: 0,
        };
        let variants = vec![
            variant("Grant: once", ""),
            variant(
                "Grant: always allow this for the workspace",
                "shared by any program",
            ),
        ];
        for columns in [12, 20, 40, 100] {
            let first = layout(&variants, 0, columns, usize::MAX);
            let second = layout(&variants, 1, columns, usize::MAX);
            assert_eq!(first.rows.len(), second.rows.len(), "{columns}");
            assert_eq!(first.rows.len(), text_rows(&variants, columns));
            assert_eq!(first.duration_rows, second.duration_rows);
            assert_eq!(first.rows.last(), Some(&"hint".to_owned()));
        }
        // Every row of a wrapped duration selects it.
        let laid = layout(&variants, 0, 12, usize::MAX);
        assert_eq!(laid.duration_rows, [(0, 0), (1, 1), (2, 1)]);
    }

    /// Clipping keeps the hint rows to the last: the body is cut first and
    /// says so, and only hints that alone do not fit are cut.
    #[test]
    fn clipping_drops_body_rows_before_the_hint_rows() {
        let variants = text(vec![
            DialogLine::new("TITLE"),
            DialogLine::new("body one"),
            DialogLine::new("body two"),
            DialogLine::new("body three").with_duration(0),
            DialogLine::hint("keys"),
            DialogLine::hint("click"),
            DialogLine::hint("Ctrl+Alt+G"),
        ]);
        let all = layout(&variants, 0, 40, 7);
        assert!(!all.clipped);
        assert_eq!(all.rows.len(), 7);
        assert_eq!(all.duration_rows, [(3, 0)]);
        let cut = layout(&variants, 0, 40, 5);
        assert!(cut.clipped);
        assert_eq!(
            cut.rows,
            ["TITLE", CLIPPED_MARKER, "keys", "click", "Ctrl+Alt+G"]
        );
        assert!(cut.duration_rows.is_empty());
        assert_eq!(
            layout(&variants, 0, 40, 3).rows,
            ["keys", "click", "Ctrl+Alt+G"]
        );
        assert_eq!(layout(&variants, 0, 40, 2).rows, ["keys", "click"]);
        assert!(layout(&variants, 0, 40, 0).rows.is_empty());
    }

    /// A dialog that hides text says so where its answer hints were.
    #[test]
    fn a_clipped_layout_shows_its_notice_instead_of_the_hints() {
        let variants = text(vec![
            DialogLine::new("TITLE"),
            DialogLine::new("body one"),
            DialogLine::new("body two"),
            DialogLine::hint("G grants"),
            DialogLine::hint("click green"),
        ]);
        let notice = Some("Part of this is hidden");
        let fits = layout_with_notice(&variants, 0, 40, 5, notice);
        assert!(!fits.clipped);
        assert_eq!(fits.rows[3..], ["G grants", "click green"]);
        let cut = layout_with_notice(&variants, 0, 40, 3, notice);
        assert!(cut.clipped);
        assert_eq!(
            cut.rows,
            ["TITLE", CLIPPED_MARKER, "Part of this is hidden"]
        );
        // Hiding only a hint still counts as hidden text, and the body,
        // all in view, gets no marker.
        let hints_only = layout_with_notice(&variants, 0, 40, 4, notice);
        assert!(hints_only.clipped);
        assert_eq!(
            hints_only.rows,
            ["TITLE", "body one", "body two", "Part of this is hidden"]
        );
        // The notice wraps like any line, and is cut last.
        let narrow = layout_with_notice(&variants, 0, 12, 3, notice);
        assert_eq!(narrow.rows, [CLIPPED_MARKER, "Part of this", "is hidden"]);
    }

    #[test]
    fn dialog_snapshots_validate_and_hold_every_row() {
        let identity = DialogIdentity {
            title: "Upgrade question".to_owned(),
            ..DialogIdentity::default()
        };
        let snapshot = dialog_snapshot(vec!["one".into(), "a longer row".into()], 4, 6, &identity);
        assert_eq!(snapshot.validate().map_err(|error| error.message), Ok(()));
        assert_eq!((snapshot.columns, snapshot.rows), (12, 6));
        assert_eq!(snapshot.title, "Upgrade question");
        assert_eq!(snapshot.cursor_row, -1);
    }
}
