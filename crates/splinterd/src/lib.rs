//! Daemon-owned runtime components.

use std::path::Path;

use anyhow::Result;

pub mod authorization;
pub mod executable_identity;
pub mod image_transport;
mod live;
pub mod policy;

/// Loads one policy through the daemon's secure loader without publishing it.
///
/// The returned JSON is normalized from the validated typed representation.
///
/// # Errors
///
/// Returns an error when the path, ownership, permissions, file shape, JSON, or
/// semantic policy constraints fail the daemon's bounded validation rules.
pub fn inspect_policy_file(path: &Path) -> Result<(usize, serde_json::Value)> {
    let document = policy::inspect_file(path)?;
    let rule_count = document.rule_count();
    Ok((rule_count, serde_json::to_value(document)?))
}

pub use live::{
    CompactSubscription, LiveCell, LiveError, LiveEvent, LiveRow, LiveRuntimeMetrics,
    LiveScrollbackPage, LiveSearchPage, LiveSnapshot, LiveSplintConfig, LiveSplintHandle,
    LiveSplintRuntime, ProcessExit, ProcessIncarnation, ProcessPlacement, Subscription,
    SubscriptionReceive, TerminalPublicationMemoryLease,
};
