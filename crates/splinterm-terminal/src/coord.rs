//! Grid coordinates, ranges, and scroll regions.
//!
//! Derived from Foot 1.27.0 `terminal.h` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `coord`, `range`,
//! and `scroll_region`. Signed coordinates are retained because later Foot
//! grid and scrollback operations use signed row positions.

/// A zero-based terminal-grid coordinate.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Coordinate {
    /// Zero-based column.
    pub column: i32,
    /// Zero-based row.
    pub row: i32,
}

impl Coordinate {
    /// Constructs a coordinate.
    #[must_use]
    pub const fn new(column: i32, row: i32) -> Self {
        Self { column, row }
    }
}

/// An inclusive pair of terminal coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CoordinateRange {
    /// First coordinate in the range.
    pub start: Coordinate,
    /// Last coordinate in the range.
    pub end: Coordinate,
}

impl CoordinateRange {
    /// Constructs an inclusive coordinate range.
    #[must_use]
    pub const fn new(start: Coordinate, end: Coordinate) -> Self {
        Self { start, end }
    }
}

/// A half-open vertical region used by terminal scrolling operations.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScrollRegion {
    start: i32,
    end: i32,
}

impl ScrollRegion {
    /// Constructs a half-open `start..end` region.
    ///
    /// # Panics
    ///
    /// Panics if `start` follows `end`.
    #[must_use]
    pub const fn new(start: i32, end: i32) -> Self {
        assert!(start <= end, "scroll region start must not follow its end");
        Self { start, end }
    }

    /// Returns the first row in the region.
    #[must_use]
    pub const fn start(self) -> i32 {
        self.start
    }

    /// Returns the exclusive row after the region.
    #[must_use]
    pub const fn end(self) -> i32 {
        self.end
    }

    /// Returns whether `row` lies inside the half-open region.
    #[must_use]
    pub const fn contains(self, row: i32) -> bool {
        row >= self.start && row < self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_region_is_half_open_like_foot() {
        let region = ScrollRegion::new(2, 5);

        assert!(!region.contains(1));
        assert!(region.contains(2));
        assert!(region.contains(4));
        assert!(!region.contains(5));
    }

    #[test]
    #[should_panic(expected = "scroll region start must not follow its end")]
    fn reversed_scroll_region_is_rejected() {
        let _ = ScrollRegion::new(3, 2);
    }
}
