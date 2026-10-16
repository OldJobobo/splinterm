//! Row storage and linebreak metadata.
//!
//! Derived from Foot 1.27.0 `terminal.h`, `grid.c`, and `terminal.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `row` and
//! `grid_row_alloc` plus erase/fill behavior. A fresh row is not dirty, ends in
//! a hard linebreak, and contains clean empty cells. URI, underline, and shell-integration metadata
//! remain deferred until the phases that consume them.

use std::ops::{Index, IndexMut, Range};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Attributes, Cell, CellContent, Color, cell::CHECKPOINT_CELL_BYTES};

pub(crate) const MAX_CHECKPOINT_COLUMNS: usize = 1 << 16;

/// A terminal row and the metadata needed to distinguish hard line endings
/// from soft wrapping.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    #[serde(
        serialize_with = "serialize_cells",
        deserialize_with = "deserialize_cells"
    )]
    cells: Vec<Cell>,
    dirty: bool,
    linebreak: bool,
}

impl Row {
    /// Allocates an initialized row matching Foot's initialized-row state.
    #[must_use]
    pub fn new(columns: usize) -> Self {
        let mut row = Self {
            cells: vec![Cell::default(); columns],
            dirty: false,
            linebreak: true,
        };
        row.mark_cells_clean();
        row
    }

    /// Allocates a dirty empty row for newly exposed grid content.
    #[must_use]
    pub fn new_dirty(columns: usize) -> Self {
        Self {
            cells: vec![Cell::default(); columns],
            dirty: true,
            linebreak: true,
        }
    }

    pub(crate) fn from_cells(cells: Vec<Cell>, linebreak: bool, dirty: bool) -> Self {
        Self {
            cells,
            dirty,
            linebreak,
        }
    }

    /// Returns the number of columns in this row.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Returns whether the row contains no cells.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Returns all cells in column order.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Returns mutable cell storage and marks the row and every returned cell
    /// dirty. Prefer indexed mutation when only one cell changes.
    pub fn cells_mut(&mut self) -> &mut [Cell] {
        self.dirty = true;
        for cell in &mut self.cells {
            cell.attributes_mut().set_clean(false);
        }
        &mut self.cells
    }

    /// Returns whether presentation state for this row needs refreshing.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Sets the row's dirty state.
    pub fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
    }

    /// Returns true for a hard line ending and false when the next row is a
    /// soft-wrapped continuation.
    #[must_use]
    pub const fn has_linebreak(&self) -> bool {
        self.linebreak
    }

    /// Sets hard-linebreak (`true`) or soft-wrap (`false`) metadata.
    pub fn set_linebreak(&mut self, linebreak: bool) {
        self.linebreak = linebreak;
    }

    /// Erases a half-open cell range while preserving only the active
    /// background, matching Foot's erase-cell behavior.
    ///
    /// # Panics
    ///
    /// Panics if the range is reversed or extends past the row.
    pub fn erase(&mut self, range: Range<usize>, background: Color) {
        self.assert_range(&range);
        self.clear_wide_intersections(range.clone());
        let mut attributes = Attributes::default();
        attributes.set_background(background);
        for cell in &mut self.cells[range] {
            *cell = Cell::default();
            cell.set_attributes(attributes);
        }
        self.dirty = true;
    }

    /// Erases the complete row and restores hard-linebreak metadata.
    pub fn erase_all(&mut self, background: Color) {
        self.erase(0..self.cells.len(), background);
        self.linebreak = true;
    }

    /// Fills a half-open range with one content/attribute pair.
    ///
    /// # Panics
    ///
    /// Panics if the range is reversed or extends past the row.
    pub fn fill(&mut self, range: Range<usize>, content: CellContent, attributes: Attributes) {
        self.assert_range(&range);
        self.clear_wide_intersections(range.clone());
        for cell in &mut self.cells[range] {
            cell.set_content(content);
            cell.set_attributes(attributes);
            cell.attributes_mut().set_clean(false);
        }
        self.dirty = true;
    }

    /// Inserts blank cells, shifting existing content right and dropping the
    /// rightmost cells.
    ///
    /// # Panics
    ///
    /// Panics if `column` is outside the row.
    #[allow(
        clippy::range_plus_one,
        reason = "the terminal row mutation API intentionally uses half-open Range values"
    )]
    pub fn insert_cells(&mut self, column: usize, count: usize, background: Color) {
        assert!(
            column < self.cells.len(),
            "insert column must fit within the row"
        );
        let count = count.min(self.cells.len() - column);
        if count == 0 {
            return;
        }
        self.clear_wide_intersections(column..column + 1);
        let truncation = self.cells.len() - count;
        if truncation < self.cells.len() {
            self.clear_wide_intersections(truncation..truncation + 1);
        }
        let end = self.cells.len() - count;
        self.cells.copy_within(column..end, column + count);
        self.erase(column..column + count, background);
        self.repair_wide_cells();
    }

    /// Deletes cells, shifting following content left and erasing the tail.
    ///
    /// # Panics
    ///
    /// Panics if `column` is outside the row.
    #[allow(
        clippy::range_plus_one,
        reason = "the terminal row mutation API intentionally uses half-open Range values"
    )]
    pub fn delete_cells(&mut self, column: usize, count: usize, background: Color) {
        assert!(
            column < self.cells.len(),
            "delete column must fit within the row"
        );
        let count = count.min(self.cells.len() - column);
        if count == 0 {
            return;
        }
        self.clear_wide_intersections(column..column + count);
        if column + count < self.cells.len() {
            self.clear_wide_intersections(column + count..column + count + 1);
        }
        self.cells.copy_within(column + count.., column);
        self.erase(self.cells.len() - count..self.cells.len(), background);
        self.repair_wide_cells();
    }

    /// Clears any complete wide-cell sequence intersecting a half-open range.
    ///
    /// # Panics
    ///
    /// Panics if the range does not fit within the row.
    pub fn clear_wide_intersections(&mut self, range: Range<usize>) {
        self.assert_range(&range);
        let mut column = 0;
        while column < self.cells.len() {
            let width = if column + 1 < self.cells.len() {
                match self.cells[column + 1].content() {
                    CellContent::Spacer(remaining) if remaining > 0 => usize::try_from(remaining)
                        .unwrap_or(usize::MAX)
                        .saturating_add(1),
                    _ => 1,
                }
            } else {
                1
            };
            let end = column.saturating_add(width).min(self.cells.len());
            let valid = width > 1
                && end - column == width
                && (1..width).all(|offset| {
                    self.cells[column + offset].content()
                        == CellContent::Spacer(u32::try_from(width - offset).unwrap())
                });
            if valid {
                if column < range.end && range.start < end {
                    for cell in &mut self.cells[column..end] {
                        *cell = Cell::default();
                    }
                    self.dirty = true;
                }
                column = end;
            } else {
                column += 1;
            }
        }
    }

    /// Returns whether positive spacer payloads form complete, descending
    /// continuation sequences with a leader cell.
    #[must_use]
    pub fn has_valid_wide_cells(&self) -> bool {
        let mut column = 0;
        while column < self.cells.len() {
            match self.cells[column].content() {
                CellContent::Spacer(0) => column += 1,
                CellContent::Spacer(_) => return false,
                _ => {
                    let mut next = column + 1;
                    if next >= self.cells.len() {
                        column = next;
                        continue;
                    }
                    let CellContent::Spacer(first) = self.cells[next].content() else {
                        column = next;
                        continue;
                    };
                    if first == 0 {
                        column = next;
                        continue;
                    }
                    let mut expected = first;
                    while expected > 0 {
                        if next >= self.cells.len()
                            || self.cells[next].content() != CellContent::Spacer(expected)
                        {
                            return false;
                        }
                        next += 1;
                        expected -= 1;
                    }
                    column = next;
                }
            }
        }
        true
    }

    /// Resizes row storage without reflowing its content.
    #[allow(
        clippy::comparison_chain,
        reason = "ordered grow/shrink branches mirror the terminal resize behavior"
    )]
    pub(crate) fn resize_without_reflow(&mut self, columns: usize) {
        let old_columns = self.cells.len();
        if columns > old_columns {
            self.cells.resize(columns, Cell::default());
            self.dirty = true;
        } else if columns < old_columns {
            if columns > 0
                && matches!(
                    self.cells[columns].content(),
                    CellContent::Spacer(remaining) if remaining > 0
                )
            {
                let mut leader = columns;
                while leader > 0
                    && matches!(
                        self.cells[leader].content(),
                        CellContent::Spacer(remaining) if remaining > 0
                    )
                {
                    leader -= 1;
                }
                for cell in &mut self.cells[leader..columns] {
                    cell.set_content(CellContent::Empty);
                }
            }
            self.cells.truncate(columns);
        }
    }

    /// Restores the fresh, initialized Foot row state without reallocating
    /// storage.
    ///
    /// This is an allocation/reuse primitive, not a visible erase operation:
    /// callers must arrange damage before resetting a row already presented to
    /// a renderer.
    pub fn reset(&mut self) {
        self.cells.fill(Cell::default());
        self.mark_cells_clean();
        self.dirty = false;
        self.linebreak = true;
    }

    pub(crate) fn checkpoint_is_valid(&self, composed_entries: usize) -> bool {
        if !self
            .cells
            .iter()
            .all(|cell| cell.checkpoint_is_valid(composed_entries))
        {
            return false;
        }
        let mut repaired = self.clone();
        repaired.repair_wide_cells();
        repaired == *self
    }

    fn mark_cells_clean(&mut self) {
        for cell in &mut self.cells {
            cell.attributes_mut().set_clean(true);
        }
    }

    fn repair_wide_cells(&mut self) {
        let mut column = 0;
        while column < self.cells.len() {
            let CellContent::Spacer(remaining) = self.cells[column].content() else {
                column += 1;
                continue;
            };
            if remaining == 0 {
                column += 1;
                continue;
            }
            let mut leader = column;
            while leader > 0
                && matches!(self.cells[leader].content(), CellContent::Spacer(value) if value > 0)
            {
                leader -= 1;
            }
            let expected_width = usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .saturating_add(1);
            let end = leader
                .checked_add(expected_width)
                .filter(|end| *end <= self.cells.len());
            let valid = leader < column
                && end.is_some_and(|end| {
                    (1..end - leader).all(|offset| {
                        self.cells[leader + offset].content()
                            == CellContent::Spacer(u32::try_from(expected_width - offset).unwrap())
                    })
                });
            if valid {
                column = end.expect("validated wide-cell end");
            } else {
                self.cells[column].set_content(CellContent::Empty);
                column += 1;
            }
        }
    }

    fn assert_range(&self, range: &Range<usize>) {
        assert!(
            range.start <= range.end && range.end <= self.cells.len(),
            "cell range must fit within the row"
        );
    }
}

/// Rows checkpoint as `[columns, base64]`: packed fixed-width cells. A trailing
/// run of cells equal to the last one is stored once and repeated on restore,
/// so blank, short and erased rows stay small.
fn serialize_cells<S>(cells: &[Cell], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let used = cells.last().map_or(0, |last| {
        cells
            .iter()
            .rposition(|cell| cell != last)
            .map_or(1, |differs| differs + 2)
    });
    let mut packed = Vec::with_capacity(used * CHECKPOINT_CELL_BYTES);
    for cell in &cells[..used] {
        cell.write_checkpoint(&mut packed);
    }
    (cells.len(), BASE64_STANDARD.encode(packed)).serialize(serializer)
}

fn deserialize_cells<'de, D>(deserializer: D) -> Result<Vec<Cell>, D::Error>
where
    D: Deserializer<'de>,
{
    let (columns, encoded) = <(usize, String)>::deserialize(deserializer)?;
    let maximum_encoded = (MAX_CHECKPOINT_COLUMNS * CHECKPOINT_CELL_BYTES).div_ceil(3) * 4;
    if columns > MAX_CHECKPOINT_COLUMNS || encoded.len() > maximum_encoded {
        return Err(serde::de::Error::custom(
            "terminal row exceeds checkpoint bound",
        ));
    }
    let packed = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| serde::de::Error::custom("terminal row is not canonical base64"))?;
    let (chunks, remainder) = packed.as_chunks::<CHECKPOINT_CELL_BYTES>();
    if !remainder.is_empty() || chunks.len() > columns || chunks.is_empty() != (columns == 0) {
        return Err(serde::de::Error::custom(
            "terminal row cells do not fit its width",
        ));
    }
    let mut cells = Vec::with_capacity(columns);
    for chunk in chunks {
        cells.push(
            Cell::read_checkpoint(chunk)
                .ok_or_else(|| serde::de::Error::custom("terminal cell is malformed"))?,
        );
    }
    if let Some(&last) = cells.last() {
        cells.resize(columns, last);
    }
    Ok(cells)
}

impl Index<usize> for Row {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        &self.cells[index]
    }
}

impl IndexMut<usize> for Row {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.dirty = true;
        self.cells[index].attributes_mut().set_clean(false);
        &mut self.cells[index]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn packed_checkpoint_rows_round_trip_and_reject_malformed_cells() {
        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
        use serde_json::json;

        use crate::cell::CHECKPOINT_CELL_BYTES;

        let mut row = Row::new(6);
        row[1] = Cell::new(CellContent::Scalar('A'));
        let encoded = serde_json::to_value(&row).unwrap();
        // The trailing run of clean blanks is stored once.
        let packed = BASE64_STANDARD
            .decode(encoded["cells"][1].as_str().unwrap())
            .unwrap();
        assert_eq!(encoded["cells"][0], json!(6));
        assert_eq!(packed.len(), 3 * CHECKPOINT_CELL_BYTES);
        assert_eq!(serde_json::from_value::<Row>(encoded).unwrap(), row);

        let cell = |offset: usize, byte: u8| {
            let mut cell = [0_u8; CHECKPOINT_CELL_BYTES];
            cell[offset] = byte;
            cell
        };
        let malformed = [
            json!([4, BASE64_STANDARD.encode(cell(12, 6))]),
            json!([4, BASE64_STANDARD.encode(cell(13, 4))]),
            json!([4, BASE64_STANDARD.encode([0_u8; CHECKPOINT_CELL_BYTES - 1])]),
            json!([1, BASE64_STANDARD.encode([1_u8; 2 * CHECKPOINT_CELL_BYTES])]),
            json!([MAX_CHECKPOINT_COLUMNS + 1, ""]),
            json!([3, ""]),
            json!([4, "not base64!"]),
        ];
        for cells in malformed {
            let value = json!({"cells": cells, "dirty": false, "linebreak": true});
            assert!(serde_json::from_value::<Row>(value).is_err());
        }
    }

    use super::*;
    use crate::{CellContent, Color};

    #[test]
    fn initialized_row_matches_foot_metadata() {
        let row = Row::new(4);

        assert_eq!(row.len(), 4);
        assert!(!row.is_dirty());
        assert!(row.has_linebreak());
        assert!(
            row.cells()
                .iter()
                .all(|cell| { cell.content() == CellContent::Empty && cell.attributes().clean() })
        );
    }

    #[test]
    fn mutable_access_marks_row_dirty() {
        let mut row = Row::new(2);
        row[0].set_content(CellContent::Scalar('x'));

        assert!(row.is_dirty());
        assert!(!row[0].attributes().clean());
        assert_eq!(row[0].content(), CellContent::Scalar('x'));
    }

    #[test]
    fn bulk_mutable_access_marks_every_cell_dirty() {
        let mut row = Row::new(2);
        let _ = row.cells_mut();

        assert!(row.is_dirty());
        assert!(row.cells().iter().all(|cell| !cell.attributes().clean()));
    }

    #[test]
    fn cloned_row_is_an_independent_equal_snapshot() {
        let mut row = Row::new(2);
        row[0].set_content(CellContent::Scalar('x'));
        let snapshot = row.clone();

        assert_eq!(row, snapshot);
        row[1].set_content(CellContent::Scalar('y'));
        assert_ne!(row, snapshot);
        assert_eq!(snapshot[1].content(), CellContent::Empty);
    }

    #[test]
    fn erase_preserves_only_background_and_marks_row_dirty() {
        let mut row = Row::new(3);
        let mut attributes = Attributes::default();
        attributes.set_bold(true);
        row.fill(0..3, CellContent::Scalar('x'), attributes);
        row.set_dirty(false);

        row.erase(1..3, Color::rgb(0x12_34_56));

        assert!(row.is_dirty());
        assert_eq!(row[0].content(), CellContent::Scalar('x'));
        for cell in &row.cells()[1..] {
            assert_eq!(cell.content(), CellContent::Empty);
            assert!(!cell.attributes().bold());
            assert_eq!(cell.attributes().background(), Color::rgb(0x12_34_56));
            assert!(!cell.attributes().clean());
        }
    }

    #[test]
    fn wide_cell_validation_accepts_complete_sequences() {
        let mut row = Row::new_dirty(5);
        row[0].set_content(CellContent::Scalar('界'));
        row[1].set_content(CellContent::Spacer(2));
        row[2].set_content(CellContent::Spacer(1));
        row[3].set_content(CellContent::Spacer(0));
        assert!(row.has_valid_wide_cells());

        row[2].set_content(CellContent::Empty);
        assert!(!row.has_valid_wide_cells());
    }

    #[test]
    fn shrinking_drops_a_severed_wide_character() {
        let mut row = Row::new_dirty(4);
        row[0].set_content(CellContent::Scalar('界'));
        row[1].set_content(CellContent::Spacer(2));
        row[2].set_content(CellContent::Spacer(1));
        row[3].set_content(CellContent::Scalar('x'));
        let mut leader_attributes = Attributes::default();
        leader_attributes.set_background(Color::rgb(0x12_34_56));
        row[0].set_attributes(leader_attributes);

        row.resize_without_reflow(2);

        assert_eq!(row.len(), 2);
        assert!(
            row.cells()
                .iter()
                .all(|cell| cell.content() == CellContent::Empty)
        );
        assert_eq!(row[0].attributes().background(), Color::rgb(0x12_34_56));
        assert!(row.has_valid_wide_cells());
    }

    #[test]
    fn reset_restores_cells_and_linebreak_metadata() {
        let mut row = Row::new(2);
        row[0].set_content(CellContent::Scalar('x'));
        row[0]
            .attributes_mut()
            .set_background(Color::rgb(0xab_cdef));
        row.set_linebreak(false);

        row.reset();

        assert!(!row.is_dirty());
        assert!(row.has_linebreak());
        assert!(row.cells().iter().all(|cell| {
            cell.content() == CellContent::Empty
                && cell.attributes().clean()
                && cell.attributes().background() == Color::default()
        }));
    }
}
