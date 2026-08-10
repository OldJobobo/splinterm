//! Binary-owned application services.

mod cli;
mod commands;
mod consent;
mod diagnostics_cli;
mod human_output;
mod keymap_cli;
mod local_service;
mod machine;
mod pane_bridge;
mod remote_cli;
mod session_catalog;
mod sessions;
mod theme_watch;
mod topology_manager;
mod window;

pub(crate) use cli::run;
