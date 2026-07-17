//! Row storage and linebreak metadata.
//!
//! Derived from Foot 1.27.0 `terminal.h`, `grid.c`, and `terminal.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `row` and
//! `grid_row_alloc` plus erase/fill behavior. A fresh row is not dirty, ends in
//! a hard linebreak, and contains clean empty cells. URI, underline, and shell-integration metadata
//! remain deferred until the phases that consume them.

use std::ops::{Index, IndexMut, Range};

use crate::{Attributes, Cell, CellContent, Color};

/// A terminal row and the metadata needed to distinguish hard line endings
/// from soft wrapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
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
        for cell in &mut self.cells[range] {
            cell.set_content(content);
            cell.set_attributes(attributes);
            cell.attributes_mut().set_clean(false);
        }
        self.dirty = true;
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

    fn mark_cells_clean(&mut self) {
        for cell in &mut self.cells {
            cell.attributes_mut().set_clean(true);
        }
    }

    fn assert_range(&self, range: &Range<usize>) {
        assert!(
            range.start <= range.end && range.end <= self.cells.len(),
            "cell range must fit within the row"
        );
    }
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
