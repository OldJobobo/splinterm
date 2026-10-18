//! Daemon handoff requests answered before the handoff gate exists.
//!
//! A dry run reports what the running daemon knows cheaply. It never fences,
//! captures, or opens the installed candidate. `Execute` is refused with the
//! same blocking reasons until the old-side gate lands (Plan 0061 M3.6), and
//! every resume ticket claim fails closed because this daemon issues no
//! tickets (M3.16). No handoff prepare is ever pending, so every Window
//! acknowledgement finds nothing to acknowledge (M3.13).

use splinterd::{
    handoff_compatibility::{HandoffCapabilities, VersionRange},
    handoff_coordinator::{HandoffCoordinatorError, RollbackHandoffAuthority},
    handoff_preflight::compiled_handoff_generation,
    package_provenance::{BuildProvenanceKind, compiled_build_provenance},
};
use splinterm_core::{LairLifetime, Topology};
use splinterm_protocol::{
    DaemonHandoffMode, ErrorCode, HandoffBlockingReason, HandoffBuild, HandoffBuildProvenance,
    HandoffReport, HandoffVersionRange, MAX_HANDOFF_REPORTED_LAIRS, ProtocolError, Response,
};
use tracing::{debug, warn};

/// The generation a freshly started daemon serves. Each adopted handoff will
/// publish a later one.
pub(crate) const FRESH_DAEMON_GENERATION: u64 = 1;

/// What startup learned about the daemon's rollback authority.
pub(crate) enum RollbackAuthority {
    /// The sealed packaged daemon/client rollback pair is retained. It is
    /// held, unread, until a handoff (M3.7) borrows it.
    Retained {
        _authority: Box<RollbackHandoffAuthority>,
    },
    /// The daemon is not installed from a package.
    NotPackaged,
    /// The daemon is packaged, but its rollback pair could not be retained.
    Unavailable,
}

impl RollbackAuthority {
    pub(crate) fn from_startup(
        prepared: Result<RollbackHandoffAuthority, HandoffCoordinatorError>,
    ) -> Self {
        match prepared {
            Ok(authority) => Self::Retained {
                _authority: Box::new(authority),
            },
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

    const fn blocking_reason(&self) -> Option<HandoffBlockingReason> {
        match self {
            Self::Retained { .. } => None,
            Self::NotPackaged => Some(HandoffBlockingReason::NotPackaged),
            Self::Unavailable => Some(HandoffBlockingReason::RollbackUnavailable),
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
        provenance: match provenance.kind {
            BuildProvenanceKind::Unpackaged => HandoffBuildProvenance::Unpackaged,
            BuildProvenanceKind::LocalCommittedHead => HandoffBuildProvenance::LocalCommittedHead,
            BuildProvenanceKind::ReleaseCandidate => HandoffBuildProvenance::ReleaseCandidate,
            BuildProvenanceKind::NixStore => HandoffBuildProvenance::NixStore,
        },
    };
    let report = report_from(
        daemon_generation,
        build,
        running.capabilities,
        topology,
        live_splints,
        rollback,
    );
    report.validate().map_err(|_| unavailable())?;
    Ok(report)
}

fn report_from(
    daemon_generation: u64,
    build: HandoffBuild,
    capabilities: HandoffCapabilities,
    topology: &Topology,
    live_splints: usize,
    rollback: &RollbackAuthority,
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
    // No live handoff exists before the old-side gate and adoption land.
    blocking_reasons.push(HandoffBlockingReason::Unsupported);
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

/// Answers a dry run with its report, and refuses `Execute` with every
/// reason the report names.
///
/// # Errors
/// Returns `HandoffUnavailable` for `Execute`; the report always names at
/// least `Unsupported` until the handoff gate exists.
pub(crate) fn prepare_daemon_handoff(
    mode: DaemonHandoffMode,
    report: HandoffReport,
) -> Result<Response, ProtocolError> {
    match mode {
        DaemonHandoffMode::DryRun => Ok(Response::HandoffReport { report }),
        DaemonHandoffMode::Execute => {
            Err(ProtocolError::handoff_unavailable(report.blocking_reasons))
        }
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
        assert!(matches!(
            prepare_daemon_handoff(DaemonHandoffMode::DryRun, report.clone()),
            Ok(Response::HandoffReport { report: answered }) if answered == report
        ));
        let refused = prepare_daemon_handoff(DaemonHandoffMode::Execute, report.clone())
            .expect_err("execute is refused until the handoff gate exists");
        assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
        assert_eq!(refused.handoff_blocking_reasons, report.blocking_reasons);
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
}
