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
//! # Staging (Plan 0061 M3.11b)
//!
//! Production builds contain the forward exec and the M3.11a rollback
//! executor. [`live_handoff_support`] is the one decision that keeps a
//! running daemon from using them. It reports `Unsupported` for:
//!
//! * a Nix-store build, for all of Splinterm 0.2.0 (operator decision
//!   2026-10-18): live upgrade ships for Arch and AUR packages only, and a
//!   Nix-provenance daemon upgrades by restarting;
//! * every build while [`WINDOW_CONTINUITY_READY`] is false. Windows do not
//!   survive a handoff yet: without the client banner and reconnect (M3.12)
//!   and the continuation, resume, and ticket work (M3.13 to M3.16), every
//!   open Window would lose its daemon.
//!
//! Every resume ticket claim fails closed because this daemon issues no
//! tickets (M3.16). No handoff prepare is ever pending, so every Window
//! acknowledgement finds nothing to acknowledge (M3.13).

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

/// Whether open Windows survive a live handoff. Until they do, live handoff
/// stays unavailable in every build (see the module documentation).
///
/// Window continuity is the client banner and reconnect (Plan 0061 M3.12)
/// and the Window continuation, resume, and ticket work (M3.13 to M3.16).
/// Setting this to `true` is the production enablement step for live
/// handoff. It may happen only once that work has landed, the M3.17 proofs
/// pass, and VM acceptance is recorded. No build feature, environment
/// variable, or test seam overrides it. Unit tests reach the gate only
/// through [`live_handoff_support_with`], with readiness passed explicitly.
pub(crate) const WINDOW_CONTINUITY_READY: bool = false;

/// Why this daemon cannot perform a live handoff at all. On the wire every
/// cause is [`HandoffBlockingReason::Unsupported`]; the cause is named only
/// in an `Execute` refusal's message and the daemon's log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LiveHandoffUnsupported {
    /// A Nix-store build. Live handoff is refused for all of Splinterm
    /// 0.2.0, and these daemons upgrade by restarting.
    NixStore,
    /// Windows do not survive a handoff yet ([`WINDOW_CONTINUITY_READY`]).
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
                "live handoff is not available in this release because open Windows cannot survive it yet; restart the daemon to run the upgraded build"
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
/// first, so a Nix-store daemon keeps being named as such after window
/// continuity lands.
pub(crate) fn live_handoff_support(
    provenance: HandoffBuildProvenance,
) -> Result<(), LiveHandoffUnsupported> {
    live_handoff_support_with(provenance, WINDOW_CONTINUITY_READY)
}

/// [`live_handoff_support`] with window-continuity readiness passed in.
/// Production passes only [`WINDOW_CONTINUITY_READY`]; only unit tests pass
/// anything else.
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
}

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
            Self::NotPackaged | Self::Unavailable => None,
        }
    }

    const fn blocking_reason(&self) -> Option<HandoffBlockingReason> {
        match self {
            Self::Retained { .. } => None,
            Self::NotPackaged => Some(HandoffBlockingReason::NotPackaged),
            Self::Unavailable => Some(HandoffBlockingReason::RollbackUnavailable),
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
    // Nix-store builds for 0.2.0, and every build until window continuity.
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
/// handoff names its cause (Nix, or window continuity) in the message, and
/// its reasons always include `Unsupported`. Since Plan 0061 M3.11b,
/// `Unsupported` refuses like every other reason: no part of the gate runs
/// (no forward preparation, fence, capture, or runtime stop) while live
/// handoff is unavailable.
///
/// # Errors
/// Returns `HandoffUnavailable` naming every reason the report names.
pub(crate) fn refuse_blocked_execute(
    report: &HandoffReport,
    support: Result<(), LiveHandoffUnsupported>,
) -> Result<(), ProtocolError> {
    match support {
        Err(unsupported) => {
            let mut reasons = report.blocking_reasons.clone();
            if !reasons.contains(&HandoffBlockingReason::Unsupported) {
                reasons.push(HandoffBlockingReason::Unsupported);
            }
            Err(ProtocolError {
                message: format!("daemon handoff is unavailable: {}", unsupported.message()),
                ..ProtocolError::handoff_unavailable(reasons)
            })
        }
        Ok(()) if report.blocking_reasons.is_empty() => Ok(()),
        Ok(()) => Err(ProtocolError::handoff_unavailable(
            report.blocking_reasons.clone(),
        )),
    }
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

/// This daemon never sends a handoff prepare frame, so no acknowledgement
/// matches a pending one. The old-side preparation (M3.13) keeps this answer
/// for a stale, cancelled, or mismatched acknowledgement.
pub(crate) fn no_pending_handoff_prepare() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::NotFound,
        "no handoff prepare is pending for this generation",
    )
}

/// This daemon issued no resume tickets, so every claim fails closed with
/// the same error an invalid, expired, or replayed ticket gets.
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

    #[test]
    fn an_unpackaged_build_always_reports_not_packaged() {
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
            [
                HandoffBlockingReason::NotPackaged,
                HandoffBlockingReason::Unsupported
            ]
        );
    }

    #[test]
    fn idle_development_daemon_reports_not_packaged_and_unsupported() {
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
            [
                HandoffBlockingReason::NotPackaged,
                HandoffBlockingReason::Unsupported
            ]
        );
        let refused =
            refuse_blocked_execute(&report, live_handoff_support(report.build.provenance))
                .expect_err("an unpackaged daemon refuses execute before the gate runs");
        assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
        assert_eq!(refused.handoff_blocking_reasons, report.blocking_reasons);
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
        // Even with window continuity ready.
        let support = live_handoff_support_with(nix.provenance, true);
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
        let refused = refuse_blocked_execute(&report, support).unwrap_err();
        assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
        assert_eq!(
            refused.message,
            "daemon handoff is unavailable: live handoff is not supported for Nix-store installations in Splinterm 0.2.0; restart the daemon to run the upgraded build"
        );
        assert_eq!(refused.handoff_blocking_reasons, report.blocking_reasons);
        // With nothing else blocking, Nix alone still refuses, as Unsupported.
        let mut alone = report;
        alone.blocking_reasons.clear();
        let refused = refuse_blocked_execute(&alone, support).unwrap_err();
        assert!(refused.message.contains("Nix-store"), "{}", refused.message);
        assert_eq!(
            refused.handoff_blocking_reasons,
            [HandoffBlockingReason::Unsupported]
        );
    }

    /// A packaged Arch build is unsupported only until window continuity
    /// lands; once it has, nothing in the decision blocks it.
    #[test]
    fn a_packaged_build_is_unsupported_only_until_window_continuity() {
        for provenance in [
            HandoffBuildProvenance::ReleaseCandidate,
            HandoffBuildProvenance::LocalCommittedHead,
        ] {
            // The shipped decision: window continuity has not landed.
            assert_eq!(
                live_handoff_support(provenance),
                Err(LiveHandoffUnsupported::WindowContinuityPending)
            );
            assert_eq!(
                live_handoff_support_with(provenance, false),
                Err(LiveHandoffUnsupported::WindowContinuityPending)
            );
            assert_eq!(live_handoff_support_with(provenance, true), Ok(()));

            let packaged = HandoffBuild {
                provenance,
                ..build()
            };
            let pending = report_from(
                FRESH_DAEMON_GENERATION,
                packaged.clone(),
                capabilities(),
                &Topology::new(),
                0,
                &RollbackAuthority::Unavailable,
                live_handoff_support(provenance),
            );
            pending.validate().unwrap();
            assert_eq!(
                pending.blocking_reasons,
                [
                    HandoffBlockingReason::RollbackUnavailable,
                    HandoffBlockingReason::Unsupported
                ]
            );
            let refused =
                refuse_blocked_execute(&pending, live_handoff_support(provenance)).unwrap_err();
            assert_eq!(
                refused.message,
                "daemon handoff is unavailable: live handoff is not available in this release because open Windows cannot survive it yet; restart the daemon to run the upgraded build"
            );
            assert_eq!(refused.handoff_blocking_reasons, pending.blocking_reasons);

            // Forced ready (test only): no Unsupported reason remains.
            let ready = report_from(
                FRESH_DAEMON_GENERATION,
                packaged,
                capabilities(),
                &Topology::new(),
                0,
                &RollbackAuthority::Unavailable,
                live_handoff_support_with(provenance, true),
            );
            ready.validate().unwrap();
            assert_eq!(
                ready.blocking_reasons,
                [HandoffBlockingReason::RollbackUnavailable]
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
        assert!(refuse_blocked_execute(&gated, supported).is_ok());
        // Unsupported support refuses a report without reasons, and names
        // Unsupported.
        let refused = refuse_blocked_execute(&gated, unsupported).unwrap_err();
        assert_eq!(
            refused.handoff_blocking_reasons,
            [HandoffBlockingReason::Unsupported]
        );
        // `Unsupported` in the report refuses on its own now.
        let mut unsupported_only = gated.clone();
        unsupported_only.blocking_reasons = vec![HandoffBlockingReason::Unsupported];
        let refused = refuse_blocked_execute(&unsupported_only, supported).unwrap_err();
        assert_eq!(refused.message, "daemon handoff is unavailable");
        assert_eq!(
            refused.handoff_blocking_reasons,
            [HandoffBlockingReason::Unsupported]
        );
        let refused = refuse_blocked_execute(&blocked, unsupported).unwrap_err();
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
        assert_eq!(
            report.blocking_reasons[1..],
            [
                HandoffBlockingReason::RollbackUnavailable,
                HandoffBlockingReason::Unsupported
            ]
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
        assert!(
            report
                .blocking_reasons
                .contains(&HandoffBlockingReason::Unsupported)
        );
        // The exec boundary's view of this build agrees with the report's.
        assert_eq!(
            compiled_live_handoff_support(),
            live_handoff_support(report.build.provenance)
        );
        assert!(compiled_live_handoff_support().is_err());
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
