//! Daemon handoff reports and the answers that need no handoff gate.
//!
//! A dry run reports what the running daemon knows cheaply. It never fences,
//! captures, or opens the installed candidate. `Execute` is refused before
//! the old-side gate (Plan 0061 M3.6) runs whenever the report names any
//! blocking reason, `Unsupported` included, and whenever
//! [`live_handoff_support`] refuses. Once allowed, the gate prepares the
//! installed forward pair beside the retained rollback pair, fences,
//! captures every live Splint, and builds the manifests (M3.7). It then
//! stops the runtime and crosses the exec boundary (M3.9), where the binary
//! arms the forward exec (M3.10) only when [`live_handoff_support`] allows
//! it.
//!
//! # Enablement (Plan 0061 M3.11b and M3.17)
//!
//! Production builds contain the forward exec and the M3.11a rollback
//! executor, and live handoff is enabled for Splinterm 0.2.0 (operator
//! decision 2026-10-22). Window continuity, the client banner and reconnect
//! (M3.12) and the continuation, resume, and ticket work (M3.13 to M3.16),
//! has landed, so [`WINDOW_CONTINUITY_READY`] is `true`.
//! [`live_handoff_support`] is the one decision that lets a running daemon
//! use them. It still reports `Unsupported` for:
//!
//! * a Nix-store build, for all of Splinterm 0.2.0 (operator decision
//!   2026-10-18): live upgrade ships for Arch and AUR packages only, and a
//!   Nix-provenance daemon upgrades by restarting;
//! * a daemon whose compiled build identity is unreadable, so that its
//!   provenance is unknown. A correctly built daemon never is.
//!
//! Support is not the whole answer. The dry-run report also names, and
//! `Execute` is refused before the gate for, open transient Lairs (D1), a
//! daemon that is not an installed package build, and one without a
//! retained rollback pair, which includes every daemon that was itself
//! started by a live handoff (D8).
//!
//! The test-only `vm-acceptance-live-handoff` feature (Plan 0061 M3.17,
//! operator-approved 2026-10-19) treats window continuity as ready for a
//! local package build (`local-committed-head`) only, which let VM
//! acceptance drive a real live upgrade before the constant flipped. With
//! the constant `true` it changes no decision, and Nix stays refused in that
//! build as in every other. The feature stays for VM acceptance. Such a
//! build carries `VM_ACCEPTANCE_MARKER`, which package provenance generation
//! and package validation refuse for release and Nix packages.
//!
//! A daemon whose live handoff is unsupported never hands off, so no daemon
//! adopted from it holds a Window and every resume ticket claim fails closed
//! (M3.16, `splinterd::handoff_continuation`). No handoff prepare is ever
//! pending there, so every Window acknowledgement finds nothing to
//! acknowledge (M3.13).

use splinterd::{
    handoff_compatibility::{HandoffCapabilities, VersionRange},
    handoff_coordinator::{
        ForwardHandoffAuthority, HandoffCoordinatorError, RollbackHandoffAuthority,
    },
    handoff_preflight::compiled_handoff_generation,
    install_prefix::InstallPrefix,
    package_provenance::{BuildProvenanceKind, compiled_build_provenance},
};
use splinterm_core::{LairLifetime, Topology};
use splinterm_protocol::{
    ErrorCode, HandoffBlockingReason, HandoffBuild, HandoffBuildProvenance, HandoffCancelReason,
    HandoffReport, HandoffVersionRange, MAX_HANDOFF_REPORTED_LAIRS, ProtocolError,
};
use tracing::{debug, warn};

/// The generation a freshly started daemon serves. Each adopted handoff will
/// publish a later one.
pub(crate) const FRESH_DAEMON_GENERATION: u64 = 1;

/// Whether open Windows survive a live handoff: `true`, so live handoff is
/// enabled (operator decision 2026-10-22; see the module documentation).
///
/// Window continuity is the client banner and reconnect (Plan 0061 M3.12)
/// and the Window continuation, resume, and ticket work (M3.13 to M3.16).
/// Setting this to `true` was the production enablement step for live
/// handoff. Clearing it would refuse live handoff in every build again, as
/// [`LiveHandoffUnsupported::WindowContinuityPending`]. No environment
/// variable or test seam overrides it, and the test-only
/// `vm-acceptance-live-handoff` build feature ([`VM_ACCEPTANCE_BUILD`]) can
/// only add readiness, never remove it. Unit tests that cover a build
/// without window continuity pass readiness explicitly through
/// [`live_handoff_support_with`].
pub(crate) const WINDOW_CONTINUITY_READY: bool = true;

/// Whether this is the test-only VM acceptance build (Plan 0061 M3.17): the
/// `vm-acceptance-live-handoff` feature treats window continuity as ready
/// for a local package build ([`window_continuity_ready`]), as every build
/// does now that [`WINDOW_CONTINUITY_READY`] is `true`.
pub(crate) const VM_ACCEPTANCE_BUILD: bool = cfg!(feature = "vm-acceptance-live-handoff");

/// The bytes a VM acceptance build carries, which package provenance
/// generation and package validation search for
/// (`tools/package/generate-package-provenance.py`). Compiled only into that
/// build, and read at run time so that the binary keeps it.
#[cfg(feature = "vm-acceptance-live-handoff")]
#[used]
pub(crate) static VM_ACCEPTANCE_MARKER: [u8; 37] = *b"splinterm-vm-acceptance-live-handoff:";

/// Whether window continuity counts as ready for a build of `provenance`:
/// the shipped [`WINDOW_CONTINUITY_READY`], or the test-only VM acceptance
/// build, and that only for a local package build (`local-committed-head`),
/// the only provenance packaging lets it carry.
fn window_continuity_ready(provenance: HandoffBuildProvenance) -> bool {
    continuity_ready_with(WINDOW_CONTINUITY_READY, vm_acceptance_build(), provenance)
}

/// [`window_continuity_ready`] with the shipped constant and the build kind
/// passed in, so unit tests can cover every build.
const fn continuity_ready_with(
    shipped: bool,
    vm_acceptance: bool,
    provenance: HandoffBuildProvenance,
) -> bool {
    shipped || (vm_acceptance && matches!(provenance, HandoffBuildProvenance::LocalCommittedHead))
}

#[cfg(feature = "vm-acceptance-live-handoff")]
fn vm_acceptance_build() -> bool {
    // Through `black_box`, so the marker stays in the binary.
    std::hint::black_box(&VM_ACCEPTANCE_MARKER)[0] == b's'
}

#[cfg(not(feature = "vm-acceptance-live-handoff"))]
const fn vm_acceptance_build() -> bool {
    false
}

/// Why this daemon cannot perform a live handoff at all. On the wire every
/// cause is [`HandoffBlockingReason::Unsupported`]; the cause is named only
/// in an `Execute` refusal's message and the daemon's log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LiveHandoffUnsupported {
    /// A Nix-store build. Live handoff is refused for all of Splinterm
    /// 0.2.0, and these daemons upgrade by restarting.
    NixStore,
    /// Window continuity is not ready: [`WINDOW_CONTINUITY_READY`] is false,
    /// as only in a build from before live handoff was enabled for 0.2.0.
    /// No build of this source reports it; it keeps the readiness input of
    /// the decision ([`live_handoff_support_with`]) a real refusal.
    WindowContinuityPending,
    /// The build identity compiled into this daemon is unreadable, so its
    /// provenance is unknown. A correctly built daemon never reports this.
    UnknownProvenance,
}

impl LiveHandoffUnsupported {
    /// The cause, worded for an `Execute` refusal.
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::NixStore => {
                "live handoff is not supported for Nix-store installations in Splinterm 0.2.0; restart the daemon to run the upgraded build"
            }
            Self::WindowContinuityPending => {
                "live handoff is not available in this build because open Windows cannot survive it; restart the daemon to run the upgraded build"
            }
            Self::UnknownProvenance => {
                "live handoff is not available because this daemon's build provenance is unknown"
            }
        }
    }
}

/// The one decision that allows live handoff (Plan 0061 M3.11b): the
/// dry-run report adds `Unsupported` exactly when this refuses, `Execute`
/// is refused before the gate whenever it refuses, and the binary arms the
/// forward exec only when it allows.
///
/// Nix is detected from the compiled build provenance (`nix-store`, which
/// `nix/package.nix` sets for a clean Nix build). A Nix build from a dirty
/// tree is `unpackaged` and is refused as not packaged. A daemon under
/// `/nix/store` that claims any other provenance cannot retain rollback
/// authority, because a package record's location must match its kind.
///
/// # Errors
/// Returns the cause whenever live handoff is unavailable. Nix is checked
/// first, so a Nix-store daemon is named as such whatever window continuity
/// says, in the VM acceptance build too.
pub(crate) fn live_handoff_support(
    provenance: HandoffBuildProvenance,
) -> Result<(), LiveHandoffUnsupported> {
    live_handoff_support_with(provenance, window_continuity_ready(provenance))
}

/// [`live_handoff_support`] with window-continuity readiness passed in.
/// Production passes [`window_continuity_ready`], which is `true` in every
/// build since enablement; only unit tests pass `false`, to cover a build
/// without window continuity.
fn live_handoff_support_with(
    provenance: HandoffBuildProvenance,
    window_continuity_ready: bool,
) -> Result<(), LiveHandoffUnsupported> {
    if provenance == HandoffBuildProvenance::NixStore {
        return Err(LiveHandoffUnsupported::NixStore);
    }
    if !window_continuity_ready {
        return Err(LiveHandoffUnsupported::WindowContinuityPending);
    }
    Ok(())
}

/// [`live_handoff_support`] for the build compiled into this process. The
/// exec boundary uses it to decide whether to arm the forward exec. An
/// unreadable build identity refuses.
///
/// # Errors
/// As [`live_handoff_support`], or `UnknownProvenance`.
pub(crate) fn compiled_live_handoff_support() -> Result<(), LiveHandoffUnsupported> {
    let running =
        compiled_handoff_generation().map_err(|_| LiveHandoffUnsupported::UnknownProvenance)?;
    let provenance = compiled_build_provenance(running.capabilities)
        .map_err(|_| LiveHandoffUnsupported::UnknownProvenance)?;
    live_handoff_support(wire_provenance(provenance.kind))
}

const fn wire_provenance(kind: BuildProvenanceKind) -> HandoffBuildProvenance {
    match kind {
        BuildProvenanceKind::Unpackaged => HandoffBuildProvenance::Unpackaged,
        BuildProvenanceKind::LocalCommittedHead => HandoffBuildProvenance::LocalCommittedHead,
        BuildProvenanceKind::ReleaseCandidate => HandoffBuildProvenance::ReleaseCandidate,
        BuildProvenanceKind::NixStore => HandoffBuildProvenance::NixStore,
    }
}

/// What startup learned about the daemon's rollback authority.
pub(crate) enum RollbackAuthority {
    /// The sealed packaged daemon/client rollback pair is retained for the
    /// daemon's life. Each handoff attempt borrows it to prepare the forward
    /// pair and never consumes it.
    Retained {
        authority: Box<RollbackHandoffAuthority>,
    },
    /// The daemon is not installed from a package.
    NotPackaged,
    /// The daemon is packaged, but its rollback pair could not be retained.
    Unavailable,
    /// This daemon was started by a live handoff: it is the adopted forward
    /// daemon or the rollback daemon. Its sealed rollback pair closed at the
    /// adoption commit, and no new one is captured, so it can perform no
    /// further live handoff until it restarts (Plan 0061 D8). Rebuilding
    /// rollback authority after a handoff is deferred to Milestone 4. On the
    /// wire this is `RollbackUnavailable`; an `Execute` refusal names the
    /// restart ([`SPENT_BY_HANDOFF`]).
    SpentByHandoff,
}

/// Why an adopted or rollback daemon refuses another live handoff, worded
/// for an `Execute` refusal (Plan 0061 D8).
pub(crate) const SPENT_BY_HANDOFF: &str = "this daemon was itself started by a live upgrade or a rollback and keeps no rollback copy, so another live upgrade needs a daemon restart first";

impl RollbackAuthority {
    /// Classifies the startup rollback capture, which must agree with the
    /// installed-prefix authority.
    ///
    /// The rollback pair is captured in `main`'s single-threaded preamble,
    /// before the prefix is recorded; both resolve the running path then. A
    /// pair captured from anywhere but `<prefix>/splinterd` and
    /// `<prefix>/splinterm` is not retained, so every forward preparation,
    /// package record, and manifest derived from the pair's recorded paths
    /// names the same installation as the prefix.
    pub(crate) fn from_startup(
        prepared: Result<RollbackHandoffAuthority, HandoffCoordinatorError>,
        install_prefix: &InstallPrefix,
    ) -> Self {
        match prepared {
            Ok(authority) if install_prefix.names_pair(authority.pair()) => Self::Retained {
                authority: Box::new(authority),
            },
            Ok(_) => {
                warn!("packaged rollback pair disagrees with the installed prefix");
                Self::Unavailable
            }
            Err(HandoffCoordinatorError::NotPackaged) => {
                debug!("development daemon has no packaged rollback handoff authority");
                Self::NotPackaged
            }
            Err(error) => {
                warn!(%error, "packaged rollback handoff authority is unavailable");
                Self::Unavailable
            }
        }
    }

    /// The retained rollback pair, when startup retained one.
    pub(crate) fn retained(&self) -> Option<&RollbackHandoffAuthority> {
        match self {
            Self::Retained { authority } => Some(authority),
            Self::NotPackaged | Self::Unavailable | Self::SpentByHandoff => None,
        }
    }

    const fn blocking_reason(&self) -> Option<HandoffBlockingReason> {
        match self {
            Self::Retained { .. } => None,
            Self::NotPackaged => Some(HandoffBlockingReason::NotPackaged),
            Self::Unavailable | Self::SpentByHandoff => {
                Some(HandoffBlockingReason::RollbackUnavailable)
            }
        }
    }

    /// Discovers, seals, preflights, and authenticates the installed forward
    /// pair beside the retained rollback pair, which stays retained whatever
    /// the outcome.
    ///
    /// Blocks on sealed preflight; the gate runs it on a blocking thread while
    /// the daemon keeps serving (Plan 0061 D2).
    ///
    /// # Errors
    /// `NoInstalledUpgrade` when the installed pair is the running build;
    /// `NoPackageRecord` when no package record is installed beside it;
    /// `Failed` for any other failure, or without retained authority, which
    /// `Execute` refuses before the gate in any case.
    pub(crate) fn prepare_forward(&self) -> Result<ForwardHandoffAuthority, ForwardRefusal> {
        let Self::Retained { authority } = self else {
            return Err(ForwardRefusal::Failed);
        };
        authority
            .prepare_forward_installed()
            .map_err(|error| match error {
                HandoffCoordinatorError::NoInstalledUpgrade => ForwardRefusal::NoInstalledUpgrade,
                HandoffCoordinatorError::NoInstalledPackageRecord => {
                    warn!(
                        "installed daemon has no package record; handoff needs a packaged install"
                    );
                    ForwardRefusal::NoPackageRecord
                }
                error => {
                    warn!(%error, "installed forward daemon pair cannot be prepared for handoff");
                    ForwardRefusal::Failed
                }
            })
    }
}

/// Why a handoff attempt stopped before it fenced: the installed forward pair
/// could not be prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForwardRefusal {
    /// The installed pair is the running build, so there is nothing to hand
    /// off to. The attempt cancels as `Blocked`: nothing failed, and
    /// installing an upgrade lifts the condition.
    NoInstalledUpgrade,
    /// No package record is installed beside the forward pair, as after a
    /// dirty-tree install over a packaged daemon. The attempt cancels as
    /// `Failed`, but the answer names the cause.
    NoPackageRecord,
    /// The forward pair is missing, changing, unsafe, unauthenticated, or
    /// incompatible, or its preparation could not run.
    Failed,
}

impl ForwardRefusal {
    pub(crate) const fn cancel_reason(self) -> HandoffCancelReason {
        match self {
            Self::NoInstalledUpgrade => HandoffCancelReason::Blocked,
            Self::NoPackageRecord | Self::Failed => HandoffCancelReason::Failed,
        }
    }
}

/// Builds the dry-run report from point-in-time daemon state.
///
/// # Errors
/// Returns `Internal` when the build identity compiled into this daemon is
/// malformed, which a correctly built daemon never is.
pub(crate) fn handoff_report(
    daemon_generation: u64,
    topology: &Topology,
    live_splints: usize,
    rollback: &RollbackAuthority,
) -> Result<HandoffReport, ProtocolError> {
    let unavailable =
        || ProtocolError::new(ErrorCode::Internal, "daemon build identity is unavailable");
    let running = compiled_handoff_generation().map_err(|_| unavailable())?;
    let provenance = compiled_build_provenance(running.capabilities).map_err(|_| unavailable())?;
    let build = HandoffBuild {
        version: provenance.build_version.to_owned(),
        identity: provenance.pair_build_identity.as_bytes(),
        provenance: wire_provenance(provenance.kind),
    };
    let support = live_handoff_support(build.provenance);
    let report = report_from(
        daemon_generation,
        build,
        running.capabilities,
        topology,
        live_splints,
        rollback,
        support,
    );
    report.validate().map_err(|_| unavailable())?;
    Ok(report)
}

/// The report for `build`, where `support` is [`live_handoff_support`] for
/// `build.provenance` (unit tests pass [`live_handoff_support_with`]).
fn report_from(
    daemon_generation: u64,
    build: HandoffBuild,
    capabilities: HandoffCapabilities,
    topology: &Topology,
    live_splints: usize,
    rollback: &RollbackAuthority,
    support: Result<(), LiveHandoffUnsupported>,
) -> HandoffReport {
    // An unpackaged build has no installed pair, whatever startup found.
    let rollback = if build.provenance == HandoffBuildProvenance::Unpackaged {
        &RollbackAuthority::NotPackaged
    } else {
        rollback
    };
    let range = |range: VersionRange| HandoffVersionRange {
        minimum: range.minimum(),
        maximum: range.maximum(),
    };
    let mut transient = topology
        .lairs()
        .filter(|lair| lair.lifetime == LairLifetime::Transient)
        .map(|lair| lair.id)
        .collect::<Vec<_>>();
    // Name a deterministic bounded subset; the count stays exact.
    transient.sort_unstable();
    let transient_lairs = transient.len();
    transient.truncate(MAX_HANDOFF_REPORTED_LAIRS);
    let mut blocking_reasons = Vec::with_capacity(3);
    // Operator decision D1: transient Lairs block a handoff until they close.
    if transient_lairs > 0 {
        blocking_reasons.push(HandoffBlockingReason::TransientLairsOpen {
            count: transient_lairs,
            lair_ids: transient,
        });
    }
    blocking_reasons.extend(rollback.blocking_reason());
    // Nix-store builds, for all of 0.2.0.
    if let Err(unsupported) = support {
        debug!(?unsupported, "live handoff is unsupported");
        blocking_reasons.push(HandoffBlockingReason::Unsupported);
    }
    HandoffReport {
        daemon_generation,
        build,
        private_protocol: range(capabilities.private_protocol),
        handoff_protocol: range(capabilities.handoff_protocol),
        terminal_checkpoint: range(capabilities.terminal_checkpoint),
        descriptor_manifest: range(capabilities.descriptor_manifest),
        lairs: topology.lairs().len(),
        transient_lairs,
        dojos: topology.lairs().map(|lair| lair.dojos.len()).sum(),
        live_splints,
        rollback_available: matches!(rollback, RollbackAuthority::Retained { .. }),
        blocking_reasons,
    }
}

/// Refuses `Execute` before the handoff gate runs when `support` refuses
/// live handoff or the report names any blocking reason.
///
/// `support` is [`live_handoff_support`] for the report's build, evaluated
/// by the caller at `Execute` time and not read back from the report. Unit
/// tests that drive the gate pass `Ok(())`. A refusal for unsupported live
/// handoff names its cause (Nix, an unknown provenance, or missing window
/// continuity) in the message, and its reasons always include
/// `Unsupported`. Since Plan 0061 M3.11b,
/// `Unsupported` refuses like every other reason: no part of the gate runs
/// (no forward preparation, fence, capture, or runtime stop) while live
/// handoff is unavailable.
///
/// `rollback` is this daemon's rollback authority. An adopted or rollback
/// daemon ([`RollbackAuthority::SpentByHandoff`]) always refuses, because its
/// report names a rollback reason, and the message also says that a restart
/// is needed first (Plan 0061 D8).
///
/// # Errors
/// Returns `HandoffUnavailable` naming every reason the report names.
pub(crate) fn refuse_blocked_execute(
    report: &HandoffReport,
    support: Result<(), LiveHandoffUnsupported>,
    rollback: &RollbackAuthority,
) -> Result<(), ProtocolError> {
    let mut reasons = report.blocking_reasons.clone();
    let mut causes = Vec::with_capacity(2);
    if let Err(unsupported) = support {
        if !reasons.contains(&HandoffBlockingReason::Unsupported) {
            reasons.push(HandoffBlockingReason::Unsupported);
        }
        causes.push(unsupported.message());
    }
    if matches!(rollback, RollbackAuthority::SpentByHandoff) {
        causes.push(SPENT_BY_HANDOFF);
    }
    if reasons.is_empty() && causes.is_empty() {
        return Ok(());
    }
    let refused = ProtocolError::handoff_unavailable(reasons);
    if causes.is_empty() {
        return Err(refused);
    }
    Err(ProtocolError {
        message: format!("daemon handoff is unavailable: {}", causes.join("; ")),
        ..refused
    })
}

/// The answer to an `Execute` whose gate ran and cancelled: unavailable, with
/// every reason a fresh report names and why this attempt stopped.
pub(crate) fn cancelled_handoff(
    reason: HandoffCancelReason,
    blocking_reasons: Vec<HandoffBlockingReason>,
) -> ProtocolError {
    let reason = match reason {
        HandoffCancelReason::Requested => "it was cancelled",
        HandoffCancelReason::Blocked => "a blocking condition appeared",
        HandoffCancelReason::Failed => "preparation could not complete",
        HandoffCancelReason::TimedOut => "a step missed its deadline",
    };
    ProtocolError {
        message: format!("daemon handoff is unavailable: {reason}"),
        ..ProtocolError::handoff_unavailable(blocking_reasons)
    }
}

/// The answer to an `Execute` whose forward preparation stopped. A failure
/// reads like any cancelled attempt. An installed pair that is the running
/// build, or one without a package record, says so, since the cancel reason
/// alone (`Blocked` or `Failed`) cannot.
pub(crate) fn refused_forward_handoff(
    refusal: ForwardRefusal,
    blocking_reasons: Vec<HandoffBlockingReason>,
) -> ProtocolError {
    match refusal {
        ForwardRefusal::NoInstalledUpgrade => ProtocolError {
            message: "daemon handoff is unavailable: the installed daemon is the running build"
                .to_owned(),
            ..ProtocolError::handoff_unavailable(blocking_reasons)
        },
        ForwardRefusal::NoPackageRecord => ProtocolError {
            message: "daemon handoff is unavailable: the installed daemon has no package record"
                .to_owned(),
            ..ProtocolError::handoff_unavailable(blocking_reasons)
        },
        ForwardRefusal::Failed => cancelled_handoff(refusal.cancel_reason(), blocking_reasons),
    }
}

/// The answer to an acknowledgement nothing waits for: no prepare frame was
/// sent to that anchor, the attempt stopped waiting or was cancelled, it was
/// already acknowledged, the generation differs, or the Window failed its
/// participant binding (Plan 0061 M3.13b1). One answer for all, so it tells
/// a caller nothing more.
pub(crate) fn no_pending_handoff_prepare() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::NotFound,
        "no handoff prepare is pending for this generation",
    )
}

/// The one error every invalid, expired, ended, transferred, or replayed
/// resume ticket gets, whatever refused it.
pub(crate) fn invalid_handoff_ticket() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::InvalidHandoffTicket,
        "handoff resume ticket is invalid or expired",
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use splinterm_core::Lair;
    use splinterm_protocol::{HANDOFF_BUILD_IDENTITY_BYTES, PROTOCOL_VERSION};

    use super::*;

    fn capabilities() -> HandoffCapabilities {
        let exact = |version| VersionRange::new(version, version).unwrap();
        HandoffCapabilities {
            private_protocol: exact(PROTOCOL_VERSION),
            handoff_protocol: exact(1),
            terminal_checkpoint: exact(2),
            descriptor_manifest: exact(1),
        }
    }

    fn build() -> HandoffBuild {
        HandoffBuild {
            version: "0.2.0-dev".into(),
            identity: [3; HANDOFF_BUILD_IDENTITY_BYTES],
            provenance: HandoffBuildProvenance::Unpackaged,
        }
    }

    fn insert(topology: &mut Topology, lair: Lair) {
        topology.insert_lair_at(topology.revision(), lair).unwrap();
    }

    /// Live handoff is supported for an unpackaged build, but it has no
    /// installed pair to hand off to or roll back to, so its report names
    /// `NotPackaged` alone.
    #[test]
    fn an_unpackaged_build_always_reports_not_packaged() {
        assert_eq!(
            live_handoff_support(HandoffBuildProvenance::Unpackaged),
            Ok(())
        );
        // Startup found a record but could not retain the pair; an unpackaged
        // build still has no installed pair to roll back to.
        let report = report_from(
            FRESH_DAEMON_GENERATION,
            build(),
            capabilities(),
            &Topology::new(),
            0,
            &RollbackAuthority::Unavailable,
            live_handoff_support(HandoffBuildProvenance::Unpackaged),
        );
        report.validate().unwrap();
        assert!(!report.rollback_available);
        assert_eq!(
            report.blocking_reasons,
            [HandoffBlockingReason::NotPackaged]
        );
    }

    /// An idle development daemon is refused before the gate as not
    /// packaged, and for nothing else.
    #[test]
    fn idle_development_daemon_reports_only_not_packaged() {
        let topology = Topology::new();
        let report = report_from(
            FRESH_DAEMON_GENERATION,
            build(),
            capabilities(),
            &topology,
            0,
            &RollbackAuthority::NotPackaged,
            live_handoff_support(HandoffBuildProvenance::Unpackaged),
        );
        report.validate().unwrap();
        assert_eq!(report.daemon_generation, 1);
        assert_eq!(report.private_protocol.maximum, PROTOCOL_VERSION);
        assert_eq!((report.lairs, report.dojos, report.live_splints), (0, 0, 0));
        assert!(!report.rollback_available);
        assert_eq!(
            report.blocking_reasons,
            [HandoffBlockingReason::NotPackaged]
        );
        let refused = refuse_blocked_execute(
            &report,
            live_handoff_support(report.build.provenance),
            &RollbackAuthority::NotPackaged,
        )
        .expect_err("an unpackaged daemon refuses execute before the gate runs");
        assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
        assert_eq!(refused.message, "daemon handoff is unavailable");
        assert_eq!(
            refused.handoff_blocking_reasons,
            [HandoffBlockingReason::NotPackaged]
        );
    }

    /// Nix is refused for all of 0.2.0, whatever window continuity says, and
    /// the `Execute` refusal names it.
    #[test]
    fn a_nix_store_build_is_unsupported_and_execute_names_nix() {
        for ready in [false, true] {
            assert_eq!(
                live_handoff_support_with(HandoffBuildProvenance::NixStore, ready),
                Err(LiveHandoffUnsupported::NixStore)
            );
        }
        assert_eq!(
            live_handoff_support(HandoffBuildProvenance::NixStore),
            Err(LiveHandoffUnsupported::NixStore)
        );
        let nix = HandoffBuild {
            provenance: HandoffBuildProvenance::NixStore,
            ..build()
        };
        // The shipped decision, with window continuity ready.
        let support = live_handoff_support(nix.provenance);
        let report = report_from(
            FRESH_DAEMON_GENERATION,
            nix,
            capabilities(),
            &Topology::new(),
            0,
            &RollbackAuthority::Unavailable,
            support,
        );
        report.validate().unwrap();
        assert_eq!(report.build.provenance, HandoffBuildProvenance::NixStore);
        assert_eq!(
            report.blocking_reasons,
            [
                HandoffBlockingReason::RollbackUnavailable,
                HandoffBlockingReason::Unsupported
            ]
        );
        let refused =
            refuse_blocked_execute(&report, support, &RollbackAuthority::Unavailable).unwrap_err();
        assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
        assert_eq!(
            refused.message,
            "daemon handoff is unavailable: live handoff is not supported for Nix-store installations in Splinterm 0.2.0; restart the daemon to run the upgraded build"
        );
        assert_eq!(refused.handoff_blocking_reasons, report.blocking_reasons);
        // With nothing else blocking, Nix alone still refuses, as Unsupported.
        let mut alone = report;
        alone.blocking_reasons.clear();
        let refused =
            refuse_blocked_execute(&alone, support, &RollbackAuthority::Unavailable).unwrap_err();
        assert!(refused.message.contains("Nix-store"), "{}", refused.message);
        assert_eq!(
            refused.handoff_blocking_reasons,
            [HandoffBlockingReason::Unsupported]
        );
    }

    /// Live handoff is enabled: the shipped decision supports a packaged
    /// Arch build (release or local package), so its report names no
    /// `Unsupported` reason and, with nothing else blocking, `Execute`
    /// reaches the gate. Only a build without window continuity, which no
    /// build of this source is, would still be refused as unsupported.
    #[test]
    fn a_packaged_build_supports_live_handoff_now_that_window_continuity_is_ready() {
        for provenance in [
            HandoffBuildProvenance::ReleaseCandidate,
            HandoffBuildProvenance::LocalCommittedHead,
        ] {
            // The shipped decision: window continuity is ready.
            assert!(window_continuity_ready(provenance));
            assert_eq!(live_handoff_support(provenance), Ok(()));
            assert_eq!(
                live_handoff_support(provenance),
                live_handoff_support_with(provenance, true)
            );

            let packaged = HandoffBuild {
                provenance,
                ..build()
            };
            // A packaged daemon that could not retain its rollback pair is
            // blocked for that alone.
            let report = report_from(
                FRESH_DAEMON_GENERATION,
                packaged.clone(),
                capabilities(),
                &Topology::new(),
                0,
                &RollbackAuthority::Unavailable,
                live_handoff_support(provenance),
            );
            report.validate().unwrap();
            assert_eq!(
                report.blocking_reasons,
                [HandoffBlockingReason::RollbackUnavailable]
            );
            let refused = refuse_blocked_execute(
                &report,
                live_handoff_support(provenance),
                &RollbackAuthority::Unavailable,
            )
            .unwrap_err();
            assert_eq!(refused.message, "daemon handoff is unavailable");
            assert_eq!(refused.handoff_blocking_reasons, report.blocking_reasons);
            // With nothing blocking, the shipped decision lets `Execute`
            // through to the gate.
            let mut clear = report;
            clear.blocking_reasons.clear();
            refuse_blocked_execute(
                &clear,
                live_handoff_support(provenance),
                &RollbackAuthority::Unavailable,
            )
            .expect("a supported report naming no reason reaches the gate");

            // A build without window continuity would still be refused, as
            // unsupported, and would say why.
            let pending = live_handoff_support_with(provenance, false);
            assert_eq!(
                pending,
                Err(LiveHandoffUnsupported::WindowContinuityPending)
            );
            let pending_report = report_from(
                FRESH_DAEMON_GENERATION,
                packaged,
                capabilities(),
                &Topology::new(),
                0,
                &RollbackAuthority::Unavailable,
                pending,
            );
            pending_report.validate().unwrap();
            assert_eq!(
                pending_report.blocking_reasons,
                [
                    HandoffBlockingReason::RollbackUnavailable,
                    HandoffBlockingReason::Unsupported
                ]
            );
            let refused = refuse_blocked_execute(&clear, pending, &RollbackAuthority::Unavailable)
                .unwrap_err();
            assert_eq!(
                refused.message,
                "daemon handoff is unavailable: live handoff is not available in this build because open Windows cannot survive it; restart the daemon to run the upgraded build"
            );
            assert_eq!(
                refused.handoff_blocking_reasons,
                [HandoffBlockingReason::Unsupported]
            );
        }
    }

    #[test]
    fn only_a_supported_report_without_reasons_reaches_the_handoff_gate() {
        let supported = Ok(());
        let unsupported = Err(LiveHandoffUnsupported::WindowContinuityPending);
        let blocked = report_from(
            FRESH_DAEMON_GENERATION,
            HandoffBuild {
                provenance: HandoffBuildProvenance::ReleaseCandidate,
                ..build()
            },
            capabilities(),
            &Topology::new(),
            0,
            &RollbackAuthority::Unavailable,
            unsupported,
        );
        let mut gated = blocked.clone();
        gated.blocking_reasons.clear();
        assert!(refuse_blocked_execute(&gated, supported, &RollbackAuthority::Unavailable).is_ok());
        // Unsupported support refuses a report without reasons, and names
        // Unsupported.
        let refused = refuse_blocked_execute(&gated, unsupported, &RollbackAuthority::Unavailable)
            .unwrap_err();
        assert_eq!(
            refused.handoff_blocking_reasons,
            [HandoffBlockingReason::Unsupported]
        );
        // `Unsupported` in the report refuses on its own now.
        let mut unsupported_only = gated.clone();
        unsupported_only.blocking_reasons = vec![HandoffBlockingReason::Unsupported];
        let refused = refuse_blocked_execute(
            &unsupported_only,
            supported,
            &RollbackAuthority::Unavailable,
        )
        .unwrap_err();
        assert_eq!(refused.message, "daemon handoff is unavailable");
        assert_eq!(
            refused.handoff_blocking_reasons,
            [HandoffBlockingReason::Unsupported]
        );
        let refused =
            refuse_blocked_execute(&blocked, unsupported, &RollbackAuthority::Unavailable)
                .unwrap_err();
        assert_eq!(
            refused.handoff_blocking_reasons,
            [
                HandoffBlockingReason::RollbackUnavailable,
                HandoffBlockingReason::Unsupported
            ]
        );

        for reason in [
            HandoffCancelReason::Requested,
            HandoffCancelReason::Blocked,
            HandoffCancelReason::Failed,
            HandoffCancelReason::TimedOut,
        ] {
            let cancelled = cancelled_handoff(reason, blocked.blocking_reasons.clone());
            assert_eq!(cancelled.code, ErrorCode::HandoffUnavailable);
            assert_eq!(cancelled.handoff_blocking_reasons, blocked.blocking_reasons);
            assert!(
                cancelled
                    .message
                    .starts_with("daemon handoff is unavailable: ")
            );
        }
    }

    #[test]
    fn transient_lairs_block_with_an_exact_count_and_a_bounded_named_subset() {
        let mut topology = Topology::new();
        insert(&mut topology, Lair::new("durable", PathBuf::from("/tmp")));
        let transient_count = MAX_HANDOFF_REPORTED_LAIRS + 4;
        for index in 0..transient_count {
            insert(
                &mut topology,
                Lair::transient(format!("transient-{index}"), PathBuf::from("/tmp")),
            );
        }
        // A packaged daemon that could not retain its rollback pair.
        let packaged = HandoffBuild {
            provenance: HandoffBuildProvenance::ReleaseCandidate,
            ..build()
        };
        let report = report_from(
            FRESH_DAEMON_GENERATION,
            packaged,
            capabilities(),
            &topology,
            7,
            &RollbackAuthority::Unavailable,
            live_handoff_support(HandoffBuildProvenance::ReleaseCandidate),
        );
        report.validate().unwrap();
        assert_eq!(report.lairs, transient_count + 1);
        assert_eq!(report.dojos, transient_count + 1);
        assert_eq!(report.transient_lairs, transient_count);
        assert_eq!(report.live_splints, 7);
        let HandoffBlockingReason::TransientLairsOpen { count, lair_ids } =
            &report.blocking_reasons[0]
        else {
            panic!("transient Lairs block first: {:?}", report.blocking_reasons);
        };
        assert_eq!(*count, transient_count);
        assert_eq!(lair_ids.len(), MAX_HANDOFF_REPORTED_LAIRS);
        assert!(lair_ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(lair_ids.iter().all(|id| {
            topology
                .find_lair(*id)
                .is_some_and(|lair| lair.lifetime == LairLifetime::Transient)
        }));
        // Live handoff is supported for this build, so only the missing
        // rollback pair follows.
        assert_eq!(
            report.blocking_reasons[1..],
            [HandoffBlockingReason::RollbackUnavailable]
        );
        let refused = refuse_blocked_execute(
            &report,
            live_handoff_support(report.build.provenance),
            &RollbackAuthority::Unavailable,
        )
        .unwrap_err();
        assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
        assert_eq!(refused.handoff_blocking_reasons, report.blocking_reasons);
        // Transient Lairs alone still refuse.
        let mut transient_only = report.clone();
        transient_only.blocking_reasons.truncate(1);
        let refused = refuse_blocked_execute(
            &transient_only,
            live_handoff_support(transient_only.build.provenance),
            &RollbackAuthority::Unavailable,
        )
        .unwrap_err();
        assert_eq!(
            refused.handoff_blocking_reasons,
            transient_only.blocking_reasons
        );
    }

    /// Plan 0061 D8: a daemon started by a live handoff, whether the adopted
    /// forward daemon or the rollback daemon, refuses another live handoff
    /// until it restarts, although live handoff is enabled and nothing else
    /// blocks. Its report names `RollbackUnavailable` and no rollback
    /// copy, and the `Execute` refusal says a restart comes first.
    #[test]
    fn a_daemon_started_by_a_handoff_refuses_another_until_restart() {
        let spent = RollbackAuthority::SpentByHandoff;
        assert!(spent.retained().is_none());
        assert_eq!(spent.prepare_forward().err(), Some(ForwardRefusal::Failed));
        for provenance in [
            HandoffBuildProvenance::ReleaseCandidate,
            HandoffBuildProvenance::LocalCommittedHead,
        ] {
            let packaged = HandoffBuild {
                provenance,
                ..build()
            };
            // The shipped decision supports the build: only D8 remains.
            let support = live_handoff_support(provenance);
            assert_eq!(support, Ok(()));
            let report = report_from(
                FRESH_DAEMON_GENERATION + 1,
                packaged,
                capabilities(),
                &Topology::new(),
                2,
                &spent,
                support,
            );
            report.validate().unwrap();
            assert!(!report.rollback_available);
            assert_eq!(
                report.blocking_reasons,
                [HandoffBlockingReason::RollbackUnavailable]
            );
            let refused = refuse_blocked_execute(&report, support, &spent).unwrap_err();
            assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
            assert_eq!(
                refused.message,
                format!("daemon handoff is unavailable: {SPENT_BY_HANDOFF}")
            );
            assert_eq!(refused.handoff_blocking_reasons, report.blocking_reasons);
            // Even a report stripped of its reasons is refused for D8.
            let mut stripped = report.clone();
            stripped.blocking_reasons.clear();
            let refused = refuse_blocked_execute(&stripped, support, &spent).unwrap_err();
            assert!(
                refused.message.contains("needs a daemon restart first"),
                "{}",
                refused.message
            );
            // A build without window continuity would name both causes.
            let pending = live_handoff_support_with(provenance, false);
            let refused = refuse_blocked_execute(&report, pending, &spent).unwrap_err();
            assert_eq!(
                refused.message,
                format!(
                    "daemon handoff is unavailable: {}; {SPENT_BY_HANDOFF}",
                    LiveHandoffUnsupported::WindowContinuityPending.message()
                )
            );
            assert_eq!(
                refused.handoff_blocking_reasons,
                [
                    HandoffBlockingReason::RollbackUnavailable,
                    HandoffBlockingReason::Unsupported
                ]
            );
        }
        // An unpackaged build (the real-exec tests) reports `NotPackaged`, as
        // every unpackaged build does, and still names the restart.
        let report = report_from(
            FRESH_DAEMON_GENERATION + 1,
            build(),
            capabilities(),
            &Topology::new(),
            0,
            &spent,
            live_handoff_support(HandoffBuildProvenance::Unpackaged),
        );
        report.validate().unwrap();
        assert_eq!(
            report.blocking_reasons,
            [HandoffBlockingReason::NotPackaged]
        );
        let refused = refuse_blocked_execute(&report, Ok(()), &spent).unwrap_err();
        assert!(
            refused.message.contains(SPENT_BY_HANDOFF),
            "{}",
            refused.message
        );
    }

    #[test]
    fn compiled_build_identity_reports_a_valid_dry_run() {
        let report = handoff_report(
            FRESH_DAEMON_GENERATION,
            &Topology::new(),
            0,
            &RollbackAuthority::NotPackaged,
        )
        .unwrap();
        assert_eq!(report.build.version, env!("CARGO_PKG_VERSION"));
        assert_ne!(report.build.identity, [0; HANDOFF_BUILD_IDENTITY_BYTES]);
        assert_eq!(report.private_protocol.minimum, PROTOCOL_VERSION);
        // The exec boundary's view of this build agrees with the report's.
        let support = live_handoff_support(report.build.provenance);
        assert_eq!(compiled_live_handoff_support(), support);
        // A plain `cargo test` build is unpackaged; a package's `check()`
        // builds the tests with its provenance. Either way live handoff is
        // supported, except for a Nix-store build.
        if report.build.provenance == HandoffBuildProvenance::NixStore {
            assert_eq!(support, Err(LiveHandoffUnsupported::NixStore));
        } else {
            assert_eq!(support, Ok(()));
        }
        assert_eq!(
            report
                .blocking_reasons
                .contains(&HandoffBlockingReason::Unsupported),
            support.is_err(),
            "{:?}",
            report.blocking_reasons
        );
        // Without a retained rollback pair it is still refused, as not
        // packaged.
        assert!(
            report
                .blocking_reasons
                .contains(&HandoffBlockingReason::NotPackaged)
        );
    }

    /// Window continuity is ready in every build, the shipped one and the
    /// test-only VM acceptance build alike, so the two make the same
    /// decision for every provenance: Nix is refused and every other kind is
    /// supported (its report may still block it, as not packaged).
    #[test]
    fn window_continuity_is_ready_in_every_build_and_only_nix_is_refused() {
        assert_eq!(
            VM_ACCEPTANCE_BUILD,
            cfg!(feature = "vm-acceptance-live-handoff")
        );
        for provenance in [
            HandoffBuildProvenance::Unpackaged,
            HandoffBuildProvenance::LocalCommittedHead,
            HandoffBuildProvenance::ReleaseCandidate,
            HandoffBuildProvenance::NixStore,
        ] {
            assert!(window_continuity_ready(provenance), "{provenance:?}");
            assert_eq!(
                window_continuity_ready(provenance),
                continuity_ready_with(WINDOW_CONTINUITY_READY, false, provenance),
                "the VM acceptance feature adds nothing: {provenance:?}"
            );
            let expected = if provenance == HandoffBuildProvenance::NixStore {
                Err(LiveHandoffUnsupported::NixStore)
            } else {
                Ok(())
            };
            assert_eq!(live_handoff_support(provenance), expected, "{provenance:?}");
        }
    }

    /// The VM acceptance override applies only to a local package build: a
    /// feature build with release-candidate, Nix-store, or unpackaged
    /// provenance would wait for window continuity if the shipped constant
    /// were false (and Nix stays refused first), while the shipped constant,
    /// now true, covers every kind.
    #[test]
    fn the_vm_acceptance_override_covers_only_local_package_builds() {
        let kinds = [
            HandoffBuildProvenance::Unpackaged,
            HandoffBuildProvenance::LocalCommittedHead,
            HandoffBuildProvenance::ReleaseCandidate,
            HandoffBuildProvenance::NixStore,
        ];
        for provenance in kinds {
            let local = provenance == HandoffBuildProvenance::LocalCommittedHead;
            assert_eq!(continuity_ready_with(false, true, provenance), local);
            assert!(!continuity_ready_with(false, false, provenance));
            assert!(continuity_ready_with(true, false, provenance));
        }
        for refused in [
            HandoffBuildProvenance::ReleaseCandidate,
            HandoffBuildProvenance::NixStore,
        ] {
            let support =
                live_handoff_support_with(refused, continuity_ready_with(false, true, refused));
            assert!(support.is_err(), "{refused:?}: {support:?}");
        }
        assert_eq!(
            live_handoff_support_with(
                HandoffBuildProvenance::NixStore,
                continuity_ready_with(false, true, HandoffBuildProvenance::NixStore),
            ),
            Err(LiveHandoffUnsupported::NixStore)
        );
    }

    #[test]
    fn every_acknowledgement_finds_no_pending_prepare() {
        assert_eq!(no_pending_handoff_prepare().code, ErrorCode::NotFound);
        assert!(
            no_pending_handoff_prepare()
                .handoff_blocking_reasons
                .is_empty()
        );
    }

    #[test]
    fn every_ticket_claim_fails_closed() {
        assert_eq!(
            invalid_handoff_ticket().code,
            ErrorCode::InvalidHandoffTicket
        );
    }

    #[test]
    fn each_forward_refusal_names_its_cause_and_cancel_reason() {
        let reasons = vec![HandoffBlockingReason::Unsupported];
        for (refusal, cancel, message) in [
            (
                ForwardRefusal::NoInstalledUpgrade,
                HandoffCancelReason::Blocked,
                "daemon handoff is unavailable: the installed daemon is the running build",
            ),
            (
                ForwardRefusal::NoPackageRecord,
                HandoffCancelReason::Failed,
                "daemon handoff is unavailable: the installed daemon has no package record",
            ),
            (
                ForwardRefusal::Failed,
                HandoffCancelReason::Failed,
                "daemon handoff is unavailable: preparation could not complete",
            ),
        ] {
            assert_eq!(refusal.cancel_reason(), cancel);
            let error = refused_forward_handoff(refusal, reasons.clone());
            assert_eq!(error.code, ErrorCode::HandoffUnavailable);
            assert_eq!(error.message, message);
            assert_eq!(error.handoff_blocking_reasons, reasons);
        }
    }
}
