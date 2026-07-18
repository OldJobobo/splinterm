#![forbid(unsafe_code)]

//! Graphical client mechanisms for Splinterm.
//!
//! Wayland objects, shared-memory canvases, font data, and glyph caches remain
//! client-owned. Live protocol snapshots are replaceable derived renderer state;
//! the daemon remains the terminal-state and shell-lifetime authority.

mod box_drawing;
pub mod renderer;
pub mod wayland;

pub use wayland::{
    AuthorityStatus, TrustedConsentUi, WindowCommand, WindowOptions, WindowUpdate,
    run as run_window,
};
