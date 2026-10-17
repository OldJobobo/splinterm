//! Daemon-owned runtime components.

pub mod authorization;
pub mod executable_snapshot;
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

/// Ceiling on Splint processes one daemon owns: registered runtimes, persistent
/// and transient, plus runtimes still retiring and slots reserved for launches.
///
/// The runtime registry refuses every launch at this bound. Handoff manifests
/// carry only registered live Splints, a subset, so they are bounded by it too.
/// A retiring Splint has left the registry and is never captured as live.
pub const MAX_LIVE_SPLINTS: usize = 256;
// A full persisted topology must be able to run entirely live.
const _: () = assert!(MAX_LIVE_SPLINTS >= splinterm_core::MAX_SPLINTS);

/// Source commit for diagnostics: the packaged commit, or a best-effort commit
/// for unpackaged builds. It is never part of the handoff identity.
pub const SOURCE_COMMIT: Option<&str> = option_env!("SPLINTERM_SOURCE_COMMIT");
