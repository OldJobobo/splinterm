//! Terminal cursor state.
//!
//! Derived from Foot 1.27.0 `terminal.h` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, specifically `cursor` and its
//! last-column flag (`lcf`). Splinterm names that flag `deferred_wrap` to state
//! the behavior it records.

use crate::Coordinate;

/// Cursor position and deferred soft-wrap state.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    position: Coordinate,
    deferred_wrap: bool,
}

impl Cursor {
    /// Constructs a cursor with no pending wrap.
    #[must_use]
    pub const fn new(position: Coordinate) -> Self {
        Self {
            position,
            deferred_wrap: false,
        }
    }

    /// Returns the cursor position.
    #[must_use]
    pub const fn position(self) -> Coordinate {
        self.position
    }

    /// Moves the cursor without changing deferred-wrap state.
    pub fn set_position(&mut self, position: Coordinate) {
        self.position = position;
    }

    /// Returns whether printing the next character must first wrap.
    #[must_use]
    pub const fn deferred_wrap(self) -> bool {
        self.deferred_wrap
    }

    /// Sets the deferred-wrap flag.
    pub fn set_deferred_wrap(&mut self, deferred_wrap: bool) {
        self.deferred_wrap = deferred_wrap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_starts_without_deferred_wrap() {
        let mut cursor = Cursor::new(Coordinate::new(79, 23));
        assert_eq!(cursor.position(), Coordinate::new(79, 23));
        assert!(!cursor.deferred_wrap());

        cursor.set_deferred_wrap(true);
        cursor.set_position(Coordinate::new(0, 24));
        assert!(cursor.deferred_wrap());
        assert_eq!(cursor.position(), Coordinate::new(0, 24));
    }
}
