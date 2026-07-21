#![forbid(unsafe_code)]

//! Graphical client mechanisms for Splinterm.
//!
//! Wayland objects, shared-memory canvases, font data, and glyph caches remain
//! client-owned. Live protocol snapshots are replaceable derived renderer state;
//! the daemon remains the terminal-state and shell-lifetime authority.

#[doc(hidden)]
pub mod automation;
mod box_drawing;
pub mod config;
pub mod geometry;
pub mod pane;
pub mod renderer;
pub mod viewport;
pub mod wayland;

pub use wayland::{
    AuthorityStatus, TrustedConsentUi, WindowCommand, WindowOptions, WindowPaneOptions,
    WindowTopologyCommand, WindowTopologyUpdate, WindowUpdate, run as run_window,
};
