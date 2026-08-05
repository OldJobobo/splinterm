//! Binary-owned application services.

mod cli;
mod commands;
mod consent;
mod human_output;
mod local_service;
mod machine;
mod pane_bridge;
mod session_catalog;
mod sessions;
mod theme_watch;
mod topology_manager;
mod window;

pub(crate) use cli::run;
