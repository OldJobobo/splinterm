//! Circular row indexing and lazy allocation.
//!
//! Derived from Foot 1.27.0 `terminal.h`, `grid.h`, and `grid.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `grid.rows`,
//! `grid.offset`, `grid_row_absolute`, and `_grid_row_maybe_alloc`.
//!
//! This module intentionally contains storage only. Scrolling, viewport
//! movement, resizing, reflow, cursor coordination, and damage tracking remain
//! deferred until this representation is reviewed.

use crate::Row;

/// Power-of-two circular row storage for terminal screen and scrollback rows.
///
/// This initial grid surface exposes storage only. Cursor, viewport, scrolling,
/// resize, reflow, and damage behavior will be added in later reviewed slices.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grid {
    rows: Vec<Option<Row>>,
    columns: usize,
    offset: usize,
}

impl Grid {
    /// Creates empty row slots. Rows are allocated on first mutable use.
    ///
    /// # Panics
    ///
    /// Panics unless `row_capacity` is a non-zero power of two and `columns`
    /// is non-zero.
    #[must_use]
    pub fn new(row_capacity: usize, columns: usize) -> Self {
        assert!(
            row_capacity.is_power_of_two(),
            "grid row capacity must be a non-zero power of two"
        );
        assert!(columns > 0, "grid must have at least one column");

        Self {
            rows: vec![None; row_capacity],
            columns,
            offset: 0,
        }
    }

    /// Returns the number of physical slots in the circular row array.
    #[must_use]
    pub fn row_capacity(&self) -> usize {
        self.rows.len()
    }

    /// Returns the cell width used to initialize newly allocated rows.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Maps an offset-relative row to its physical circular slot.
    #[must_use]
    pub fn absolute_index(&self, relative_row: i32) -> usize {
        let mask = self.rows.len() - 1;
        self.offset.wrapping_add_signed(relative_row as isize) & mask
    }

    /// Returns an allocated row without changing storage.
    #[must_use]
    pub fn row(&self, relative_row: i32) -> Option<&Row> {
        self.rows[self.absolute_index(relative_row)].as_ref()
    }

    /// Returns an allocated mutable row without allocating it.
    pub fn row_mut(&mut self, relative_row: i32) -> Option<&mut Row> {
        let index = self.absolute_index(relative_row);
        self.rows[index].as_mut()
    }

    /// Returns a row, safely initializing its cells when allocating its slot.
    pub fn row_or_allocate(&mut self, relative_row: i32) -> &mut Row {
        let index = self.absolute_index(relative_row);
        self.rows[index].get_or_insert_with(|| Row::new(self.columns))
    }

    /// Changes which physical slot corresponds to relative row zero.
    ///
    /// This is a storage primitive for future scrolling code; it does not
    /// allocate, clear, or damage rows.
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset & (self.rows.len() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellContent;

    #[test]
    #[should_panic(expected = "non-zero power of two")]
    fn rejects_zero_row_capacity() {
        let _ = Grid::new(0, 80);
    }

    #[test]
    #[should_panic(expected = "non-zero power of two")]
    fn rejects_non_power_of_two_row_capacity() {
        let _ = Grid::new(6, 80);
    }

    #[test]
    #[should_panic(expected = "at least one column")]
    fn rejects_zero_columns() {
        let _ = Grid::new(8, 0);
    }

    #[test]
    fn absolute_indices_wrap_in_both_directions() {
        let mut storage = Grid::new(8, 80);
        storage.set_offset(6);

        assert_eq!(storage.absolute_index(i32::MIN), 6);
        assert_eq!(storage.absolute_index(-7), 7);
        assert_eq!(storage.absolute_index(-1), 5);
        assert_eq!(storage.absolute_index(0), 6);
        assert_eq!(storage.absolute_index(1), 7);
        assert_eq!(storage.absolute_index(2), 0);
        assert_eq!(storage.absolute_index(9), 7);
        assert_eq!(storage.absolute_index(i32::MAX), 5);
    }

    #[test]
    fn rows_are_allocated_lazily_and_initialized_safely() {
        let mut storage = Grid::new(8, 3);
        assert_eq!(storage.row_capacity(), 8);
        assert_eq!(storage.columns(), 3);
        assert!(storage.row(0).is_none());

        let row = storage.row_or_allocate(0);
        assert_eq!(row.len(), 3);
        assert!(row.cells().iter().all(|cell| cell.attributes().clean()));

        storage.row_or_allocate(0)[0].set_content(CellContent::Scalar('x'));
        assert_eq!(
            storage.row_or_allocate(0)[0].content(),
            CellContent::Scalar('x')
        );
        assert!(storage.row(1).is_none());
    }

    #[test]
    fn wrapped_rows_keep_distinct_physical_storage() {
        let mut storage = Grid::new(8, 2);
        storage.set_offset(7);
        storage.row_or_allocate(0)[0].set_content(CellContent::Scalar('a'));
        storage.row_or_allocate(1)[0].set_content(CellContent::Scalar('b'));
        storage.row_or_allocate(-1)[0].set_content(CellContent::Scalar('c'));

        assert_eq!(
            storage.row(0).unwrap()[0].content(),
            CellContent::Scalar('a')
        );
        assert_eq!(
            storage.row(1).unwrap()[0].content(),
            CellContent::Scalar('b')
        );
        assert_eq!(
            storage.row(-1).unwrap()[0].content(),
            CellContent::Scalar('c')
        );
    }

    #[test]
    fn changing_offset_changes_logical_lookup_without_moving_rows() {
        let mut storage = Grid::new(4, 1);
        storage.row_or_allocate(0)[0].set_content(CellContent::Scalar('x'));

        storage.set_offset(1);
        assert!(storage.row(0).is_none());
        assert_eq!(
            storage.row(-1).unwrap()[0].content(),
            CellContent::Scalar('x')
        );

        storage.row_mut(-1).unwrap()[0].set_content(CellContent::Scalar('y'));
        assert_eq!(
            storage.row(-1).unwrap()[0].content(),
            CellContent::Scalar('y')
        );
    }
}
