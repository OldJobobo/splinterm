#![forbid(unsafe_code)]

//! Graphical client mechanisms for Splinterm.
//!
//! Wayland objects, shared-memory canvases, font data, and glyph caches remain
//! client-owned. Live protocol snapshots are replaceable derived renderer state;
//! the daemon remains the terminal-state and shell-lifetime authority.

#[doc(hidden)]
pub mod automation;
pub mod background_effect;
mod box_drawing;
pub mod config;
pub mod endpoint;
pub mod frontend;
pub mod geometry;
pub mod keymap;
pub mod pane;
pub mod remote;
pub mod remote_session;
pub mod renderer;
pub mod session_picker;
#[doc(hidden)]
pub mod tab;
pub mod viewport;
pub mod wayland;

pub use frontend::{
    AuthorityStatus, LairDirection, LairPromptKind, LairPromptTarget, PerfTraceCorrelation,
    SelectorKind, SessionPickerDecision, SessionPickerItem, SessionPickerUi, ThemeUpdate,
    TrustedConsentUi, WindowCommand, WindowDojoIdentity, WindowOptions, WindowPaneOptions,
    WindowTopologyCommand, WindowTopologyUpdate, WindowUpdate,
};
pub use wayland::run as run_window;
