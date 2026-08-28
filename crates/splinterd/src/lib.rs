//! Daemon-owned runtime components.

pub mod authorization;
pub mod executable_snapshot;
pub mod handoff_checkpoint;
pub mod handoff_compatibility;
pub mod handoff_coordinator;
pub mod handoff_descriptors;
#[allow(
    dead_code,
    reason = "the bounded manifest ABI lands before its coordinator adoption slice"
)]
pub(crate) mod handoff_manifest;
pub mod handoff_preflight;
pub mod image_transport;
mod live;
pub mod package_provenance;

pub use live::{
    CompactSubscription, LiveCell, LiveError, LiveEvent, LiveRow, LiveRuntimeMetrics,
    LiveScrollbackPage, LiveSearchPage, LiveSnapshot, LiveSplintConfig, LiveSplintHandle,
    LiveSplintRuntime, PreparedPtyHandoff, ProcessExit, ProcessIncarnation, ProcessPlacement,
    Subscription, SubscriptionReceive, TerminalPublicationMemoryLease,
};
pub use splinterm_policy::{executable_identity, inspect_policy_file, policy};

/// Source commit for diagnostics: the packaged commit, or a best-effort commit
/// for unpackaged builds. It is never part of the handoff identity.
pub const SOURCE_COMMIT: Option<&str> = option_env!("SPLINTERM_SOURCE_COMMIT");
