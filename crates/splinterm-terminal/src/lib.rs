#![forbid(unsafe_code)]
//! Renderer-independent terminal state for Splinterm.
//!
//! This crate begins a direct Rust port of Foot 1.27.0. The foundational
//! representations are derived from Foot's `terminal.h` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`. Grid algorithms, parsing, PTY
//! ownership, rendering, and protocol serialization deliberately live outside
//! this initial layer. Renderer damage events and higher terminal-feature
//! coordination remain deferred.

mod cell;
mod composed;
mod config;
mod coord;
mod cursor;
mod event;
mod grid;
mod mode;
mod row;
mod terminal;
mod vt;

pub use cell::{Attributes, Cell, CellContent, Color, ColorSource};
pub(crate) use composed::ComposedTable;
pub use config::TerminalConfig;
pub use coord::{Coordinate, CoordinateRange, ScrollRegion};
pub use cursor::Cursor;
pub use event::TerminalEvent;
pub use grid::{Grid, ScrollDirection, ScrollResult};
pub use mode::{ActiveScreen, MouseTracking, TerminalModes};
pub use row::Row;
pub use terminal::Terminal;
