//! Circular row indexing and lazy allocation.
//!
//! Derived from Foot 1.27.0 `terminal.h`, `grid.h`, and `grid.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `grid.rows`,
//! `grid.offset`, `grid.view`, row-coordinate conversion,
//! `grid_row_absolute`, and `_grid_row_maybe_alloc`.
//!
//! This module intentionally contains storage and coordinate spaces only.
//! Scrolling commands, resizing, reflow, cursor coordination, and damage
//! tracking remain deferred until these representations are reviewed.

use crate::Row;

/// Power-of-two circular row storage for terminal screen and scrollback rows.
///
/// This grid surface currently exposes row storage, coordinate conversion, and
/// viewport-relative lookup. Cursor coordination, viewport movement commands,
/// scrolling, resize, reflow, and damage remain deferred.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grid {
    rows: Vec<Option<Row>>,
    columns: usize,
    offset: usize,
    view: usize,
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
            view: 0,
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

    /// Returns the physical row at the top of the live screen.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the physical row at the top of the current viewport.
    #[must_use]
    pub fn view(&self) -> usize {
        self.view
    }

    /// Returns whether the viewport is following the live screen.
    #[must_use]
    pub fn view_follows_offset(&self) -> bool {
        self.view == self.offset
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

    /// Maps a viewport-relative row to its physical circular slot.
    #[must_use]
    pub fn absolute_index_in_view(&self, relative_row: i32) -> usize {
        let mask = self.rows.len() - 1;
        self.view.wrapping_add_signed(relative_row as isize) & mask
    }

    /// Returns an allocated row relative to the current viewport.
    #[must_use]
    pub fn row_in_view(&self, relative_row: i32) -> Option<&Row> {
        self.rows[self.absolute_index_in_view(relative_row)].as_ref()
    }

    /// Returns an allocated mutable row relative to the current viewport.
    pub fn row_in_view_mut(&mut self, relative_row: i32) -> Option<&mut Row> {
        let index = self.absolute_index_in_view(relative_row);
        self.rows[index].as_mut()
    }

    /// Converts a physical row index to Foot's scrollback-relative space.
    ///
    /// Scrollback-relative row zero is the oldest circular slot, immediately
    /// after the live screen. Higher rows proceed toward the bottom of the
    /// current screen.
    ///
    /// # Panics
    ///
    /// Panics if `screen_rows` is zero or exceeds the grid capacity, or if
    /// `absolute_row` is not a physical row index in this grid.
    #[must_use]
    pub fn absolute_to_scrollback(&self, screen_rows: usize, absolute_row: usize) -> usize {
        self.assert_coordinate_inputs(screen_rows, absolute_row);
        let start = self.scrollback_start(screen_rows);
        absolute_row.wrapping_sub(start) & (self.rows.len() - 1)
    }

    /// Converts a Foot scrollback-relative row to a physical row index.
    ///
    /// # Panics
    ///
    /// Panics if `screen_rows` is zero or exceeds the grid capacity, or if
    /// `scrollback_row` is not a relative row index in this grid.
    #[must_use]
    pub fn scrollback_to_absolute(&self, screen_rows: usize, scrollback_row: usize) -> usize {
        self.assert_coordinate_inputs(screen_rows, scrollback_row);
        self.scrollback_start(screen_rows)
            .wrapping_add(scrollback_row)
            & (self.rows.len() - 1)
    }

    /// Finds the oldest initialized row, starting at Foot's theoretical
    /// scrollback beginning.
    ///
    /// Unlike Foot's internal helper, this bounded safe variant returns `None`
    /// when the grid has no initialized rows instead of relying on a populated
    /// visible screen.
    ///
    /// # Panics
    ///
    /// Panics if `screen_rows` is zero or exceeds the grid capacity.
    #[must_use]
    pub fn scrollback_start_ignoring_uninitialized(&self, screen_rows: usize) -> Option<usize> {
        self.assert_screen_rows(screen_rows);
        let start = self.scrollback_start(screen_rows);
        (0..self.rows.len())
            .map(|distance| start.wrapping_add(distance) & (self.rows.len() - 1))
            .find(|&index| self.rows[index].is_some())
    }

    /// Changes which physical slot corresponds to live-screen row zero.
    ///
    /// This primitive does not move the viewport automatically. Call
    /// [`Self::reset_view`] when the viewport should follow the new offset.
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset & (self.rows.len() - 1);
    }

    /// Changes which physical slot corresponds to viewport row zero.
    pub fn set_view(&mut self, view: usize) {
        self.view = view & (self.rows.len() - 1);
    }

    /// Returns the viewport to the top of the live screen.
    pub fn reset_view(&mut self) {
        self.view = self.offset;
    }

    fn scrollback_start(&self, screen_rows: usize) -> usize {
        self.offset.wrapping_add(screen_rows) & (self.rows.len() - 1)
    }

    fn assert_screen_rows(&self, screen_rows: usize) {
        assert!(
            (1..=self.rows.len()).contains(&screen_rows),
            "screen rows must fit within the grid"
        );
    }

    fn assert_coordinate_inputs(&self, screen_rows: usize, row: usize) {
        self.assert_screen_rows(screen_rows);
        assert!(row < self.rows.len(), "row index must fit within the grid");
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

    #[test]
    fn absolute_and_scrollback_coordinates_round_trip_across_wraparound() {
        let mut grid = Grid::new(8, 1);
        grid.set_offset(6);

        assert_eq!(grid.absolute_to_scrollback(3, 1), 0);
        assert_eq!(grid.absolute_to_scrollback(3, 0), 7);
        assert_eq!(grid.scrollback_to_absolute(3, 0), 1);
        assert_eq!(grid.scrollback_to_absolute(3, 7), 0);

        for absolute in 0..grid.row_capacity() {
            let scrollback = grid.absolute_to_scrollback(3, absolute);
            assert_eq!(grid.scrollback_to_absolute(3, scrollback), absolute);
        }
    }

    #[test]
    fn oldest_initialized_row_skips_empty_scrollback_slots() {
        let mut grid = Grid::new(8, 1);
        grid.set_offset(6);
        assert_eq!(grid.scrollback_start_ignoring_uninitialized(3), None);

        // Physical slot 3 is offset-relative row 5 and is the first allocated
        // slot encountered from the theoretical scrollback start at slot 1.
        grid.row_or_allocate(5);
        grid.row_or_allocate(0);
        assert_eq!(grid.scrollback_start_ignoring_uninitialized(3), Some(3));
    }

    #[test]
    fn viewport_lookup_is_independent_until_reset_to_live_screen() {
        let mut grid = Grid::new(8, 1);
        assert!(grid.view_follows_offset());

        grid.set_offset(6);
        grid.set_view(7);
        grid.row_or_allocate(2)[0].set_content(CellContent::Scalar('v'));

        assert_eq!(grid.offset(), 6);
        assert_eq!(grid.view(), 7);
        assert!(!grid.view_follows_offset());
        assert_eq!(grid.absolute_index_in_view(i32::MIN), 7);
        assert_eq!(grid.absolute_index_in_view(-1), 6);
        assert_eq!(grid.absolute_index_in_view(1), 0);
        assert_eq!(grid.absolute_index_in_view(i32::MAX), 6);
        assert_eq!(
            grid.row_in_view(1).unwrap()[0].content(),
            CellContent::Scalar('v')
        );

        grid.row_in_view_mut(1).unwrap()[0].set_content(CellContent::Scalar('w'));
        assert_eq!(grid.row(2).unwrap()[0].content(), CellContent::Scalar('w'));

        grid.reset_view();
        assert!(grid.view_follows_offset());
        assert_eq!(grid.view(), 6);
    }

    #[test]
    #[should_panic(expected = "screen rows must fit within the grid")]
    fn coordinate_conversion_rejects_zero_screen_rows() {
        let grid = Grid::new(8, 1);
        let _ = grid.absolute_to_scrollback(0, 0);
    }

    #[test]
    #[should_panic(expected = "screen rows must fit within the grid")]
    fn coordinate_conversion_rejects_oversized_screen() {
        let grid = Grid::new(8, 1);
        let _ = grid.absolute_to_scrollback(9, 0);
    }

    #[test]
    #[should_panic(expected = "row index must fit within the grid")]
    fn absolute_conversion_rejects_out_of_range_row() {
        let grid = Grid::new(8, 1);
        let _ = grid.absolute_to_scrollback(3, 8);
    }

    #[test]
    #[should_panic(expected = "row index must fit within the grid")]
    fn scrollback_conversion_rejects_out_of_range_row() {
        let grid = Grid::new(8, 1);
        let _ = grid.scrollback_to_absolute(3, 8);
    }
}
