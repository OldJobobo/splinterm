//! Compatibility re-exports for the reusable non-Wayland automation client.
//!
//! New non-graphical clients should depend on `splinterm-automation-client`
//! directly. The CLI keeps this module path to avoid unnecessary wiring churn.

pub use splinterm_automation_client::*;
