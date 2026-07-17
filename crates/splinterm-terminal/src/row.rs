//! Row storage and linebreak metadata.
//!
//! Derived from Foot 1.27.0 `terminal.h` and `grid.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `row` and
//! `grid_row_alloc`. A fresh row is not dirty, ends in a hard linebreak, and
//! contains clean empty cells. URI, underline, and shell-integration metadata
//! remain deferred until the phases that consume them.

use std::ops::{Index, IndexMut};

use crate::Cell;

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
