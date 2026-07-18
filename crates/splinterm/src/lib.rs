#![forbid(unsafe_code)]

//! Graphical client mechanisms for Splinterm.
//!
//! Wayland objects, shared-memory canvases, font data, and glyph caches remain
//! client-owned. No terminal snapshot is attached to the deterministic evidence
//! renderer yet.

mod box_drawing;
pub mod renderer;
pub mod wayland;

pub use wayland::{WindowOptions, run as run_window};
