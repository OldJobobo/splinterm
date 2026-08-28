//! Persistent automation policy loading shared by Splinterm clients and daemon.

use std::path::Path;

use anyhow::Result;

pub mod executable_identity;
pub mod policy;

/// Loads one policy through the secure loader without publishing it.
///
/// The returned JSON is normalized from the validated typed representation.
///
/// # Errors
///
/// Returns an error when the path, ownership, permissions, file shape, JSON, or
/// semantic policy constraints fail the bounded validation rules.
pub fn inspect_policy_file(path: &Path) -> Result<(usize, serde_json::Value)> {
    let document = policy::inspect_file(path)?;
    let rule_count = document.rule_count();
    Ok((rule_count, serde_json::to_value(document)?))
}
