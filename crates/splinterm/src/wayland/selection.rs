//! Pure terminal selection, copied-text, overlay-row, and URL interpretation.

use splinterm_protocol::{ActiveScreen, TerminalSnapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct CellPosition {
    pub(in crate::wayland) row: usize,
    pub(in crate::wayland) column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SelectionEndpoint {
    pub(super) active_screen: ActiveScreen,
    pub(super) history_generation: u64,
    pub(super) row_id: u64,
    pub(super) column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    pub(super) anchor: SelectionEndpoint,
    pub(super) end: SelectionEndpoint,
}

pub(super) fn selection_endpoint(
    snapshot: &TerminalSnapshot,
    position: CellPosition,
) -> Option<SelectionEndpoint> {
    let row_id = snapshot.visible_rows.get(position.row)?.row_id?;
    Some(SelectionEndpoint {
        active_screen: snapshot.active_screen,
        history_generation: snapshot.history_generation,
        row_id,
        column: position.column,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectionRange {
    start_row: usize,
    start_column: usize,
    end_row: usize,
    end_column: usize,
}

fn loaded_row_position(snapshot: &TerminalSnapshot, row_id: u64) -> Option<usize> {
    snapshot
        .scrollback_rows
        .iter()
        .chain(&snapshot.visible_rows)
        .position(|row| row.row_id == Some(row_id))
}

fn selection_range(snapshot: &TerminalSnapshot, selection: Selection) -> Option<SelectionRange> {
    if selection.anchor.active_screen != snapshot.active_screen
        || selection.end.active_screen != snapshot.active_screen
        || selection.anchor.history_generation != snapshot.history_generation
        || selection.end.history_generation != snapshot.history_generation
    {
        return None;
    }
    let anchor_row = loaded_row_position(snapshot, selection.anchor.row_id)?;
    let end_row = loaded_row_position(snapshot, selection.end.row_id)?;
    let anchor = (anchor_row, selection.anchor.column);
    let end = (end_row, selection.end.column);
    let (start, end) = if anchor <= end {
        (anchor, end)
    } else {
        (end, anchor)
    };
    Some(SelectionRange {
        start_row: start.0,
        start_column: start.1,
        end_row: end.0,
        end_column: end.1,
    })
}

pub(super) fn selection_display_bounds(
    snapshot: &TerminalSnapshot,
    display: &TerminalSnapshot,
    selection: Selection,
) -> Option<(CellPosition, CellPosition)> {
    let range = selection_range(snapshot, selection)?;
    let mut selected = display
        .visible_rows
        .iter()
        .enumerate()
        .filter_map(|(display_row, row)| {
            let loaded_row = loaded_row_position(snapshot, row.row_id?)?;
            (loaded_row >= range.start_row && loaded_row <= range.end_row)
                .then_some((display_row, loaded_row))
        });
    let first = selected.next()?;
    let last = selected.next_back().unwrap_or(first);
    Some((
        CellPosition {
            row: first.0,
            column: if first.1 == range.start_row {
                range.start_column
            } else {
                0
            },
        },
        CellPosition {
            row: last.0,
            column: if last.1 == range.end_row {
                range.end_column
            } else {
                snapshot.columns.saturating_sub(1)
            },
        },
    ))
}

pub(super) fn transient_overlay_rows(
    row_count: usize,
    selection: Option<((usize, usize), (usize, usize))>,
    hovered_url: Option<((usize, usize), (usize, usize))>,
) -> Vec<bool> {
    let mut dirty = vec![false; row_count];
    let mut mark = |start: usize, end: usize| {
        let start = start.min(row_count);
        let end = end.saturating_add(1).min(row_count);
        dirty[start..end].fill(true);
    };
    if let Some((start, end)) = selection {
        mark(start.0, end.0);
    }
    if let Some((start, end)) = hovered_url {
        mark(start.0, end.0);
    }
    dirty
}

pub(super) fn selection_is_retained(snapshot: &TerminalSnapshot, selection: Selection) -> bool {
    selection_range(snapshot, selection).is_some()
}

pub(super) fn selection_text(snapshot: &TerminalSnapshot, selection: Selection) -> Option<String> {
    let range = selection_range(snapshot, selection)?;
    let rows = snapshot
        .scrollback_rows
        .iter()
        .chain(&snapshot.visible_rows);
    let mut output = String::new();
    for (row_index, row) in rows
        .enumerate()
        .skip(range.start_row)
        .take(range.end_row.saturating_sub(range.start_row) + 1)
    {
        let first = if row_index == range.start_row {
            range.start_column
        } else {
            0
        };
        let last = if row_index == range.end_row {
            range.end_column
        } else {
            snapshot.columns.saturating_sub(1)
        };
        let mut line = String::new();
        for cell in row.cells.iter().take(last.saturating_add(1)).skip(first) {
            if cell.spacer_remaining.is_none() {
                line.push_str(&cell.content);
            }
        }
        output.push_str(line.trim_end_matches(' '));
        if row_index != range.end_row {
            output.push('\n');
        }
    }
    Some(output)
}

pub(super) fn url_at(
    snapshot: &TerminalSnapshot,
    position: CellPosition,
) -> Option<(CellPosition, CellPosition, String)> {
    let row = snapshot.visible_rows.get(position.row)?;
    let mut text = String::new();
    let mut columns = Vec::new();
    for (column, cell) in row.cells.iter().take(snapshot.columns).enumerate() {
        if cell.spacer_remaining.is_some() {
            continue;
        }
        for character in cell.content.chars() {
            columns.push(column);
            text.push(character);
        }
    }
    let byte_at = text
        .char_indices()
        .zip(columns.iter().copied())
        .find_map(|((byte, _), column)| (column == position.column).then_some(byte))?;
    let is_url_char = |character: char| {
        !character.is_whitespace() && !matches!(character, '<' | '>' | '"' | '\'')
    };
    let start = text[..byte_at]
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!is_url_char(character)).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    let end = text[byte_at..]
        .char_indices()
        .find_map(|(index, character)| (!is_url_char(character)).then_some(byte_at + index))
        .unwrap_or(text.len());
    let candidate = text[start..end].trim_end_matches(['.', ',', ')', ']', '}', ';', ':']);
    if !(candidate.starts_with("https://") || candidate.starts_with("http://")) {
        return None;
    }
    let start_char = text[..start].chars().count();
    let end_char = start_char + candidate.chars().count().saturating_sub(1);
    Some((
        CellPosition {
            row: position.row,
            column: *columns.get(start_char)?,
        },
        CellPosition {
            row: position.row,
            column: *columns.get(end_char)?,
        },
        candidate.to_owned(),
    ))
}
