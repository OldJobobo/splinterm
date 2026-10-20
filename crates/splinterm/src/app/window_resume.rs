//! A Window relaunched across a daemon upgrade (Plan 0061 M3.15; ADR 0011,
//! "Smooth local client recovery").
//!
//! `main` claimed the sealed resume record and the continuation socket
//! before Tokio (`crate::handoff_acquire`). Here the relaunched image
//! connects as trusted UI to the daemon that now serves (the adopted forward
//! daemon pins and trusts the sealed client snapshot this image runs from;
//! the rollback daemon trusts the installed client), reads the authoritative
//! topology, and rebuilds the Window: the recorded Dojos that still run, in
//! their tab order, each with its recorded focus, then the recorded active
//! tab. Each tab is a fresh resnapshot from the daemon; nothing typed before
//! the upgrade is replayed. The Window then declares its handoff anchor and
//! runs as any other managed Window.
//!
//! After an adoption, with its trusted identity established and the
//! authoritative topology read, the Window claims its resume ticket over the
//! continuation socket, which then closes, and arms it for the Splints of
//! the tabs it restores (Plan 0061 M3.16, `splinterm::handoff_control`).
//! Each pane presents it once on its own control connection after its own
//! resnapshot, and keeps the controller it held before the upgrade. A
//! rollback daemon issues no ticket, so after a rollback the socket simply
//! closes. Without a ticket, the panes take control the ordinary way.
//!
//! A recorded Dojo that no longer runs is skipped. When none runs, the
//! Window fails with a clear message instead of opening something unasked.

use anyhow::{Context, Result, bail};
use splinterm::{
    automation::retry_until,
    config::{ConfigLoad, load_default},
    diagnostics::{
        DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, global as diagnostics_global,
        initialize_graphical, install_termination_signal_task,
    },
    endpoint::ConnectionFactory,
    handoff_control::{CONTROL_RESTORE_WINDOW, TICKET_CLAIM_DEADLINE, claim_resume_ticket},
    handoff_status::RECONNECT_WINDOW,
    session_picker::dojo_has_fully_running_pane_layout,
};
use splinterm_core::{DojoId, SplintId};
use splinterm_protocol::{Request, Response, window_resume::WindowResumeRecord};

use super::{
    pane_bridge::{UNTRUSTED_AFTER_UPGRADE, layout_splint_ids, refused_as_untrusted},
    window::{WindowResume, run_resumed_multipane_window},
};
use crate::handoff_acquire::AcquiredWindowResume;

/// What a relaunched Window rebuilds from the daemon's topology.
#[derive(Debug)]
pub(super) struct WindowRestorePlan {
    /// The first surviving tab, its default focus set to the recorded one.
    pub(super) first: splinterm_core::Dojo,
    /// The other surviving tabs, in order, with their recorded focus.
    pub(super) rest: Vec<(DojoId, Option<SplintId>)>,
    /// The recorded active tab, or the nearest surviving tab after it, else
    /// the last one.
    pub(super) active: DojoId,
}

impl WindowRestorePlan {
    fn tabs(&self) -> usize {
        1 + self.rest.len()
    }

    /// Every Splint of the tabs this plan restores, from the daemon's Lairs:
    /// the panes that may present the resume ticket.
    fn splints(&self, lairs: &[splinterm_core::Lair]) -> Vec<SplintId> {
        let mut splints = Vec::new();
        layout_splint_ids(&self.first.root, &mut splints);
        for (dojo_id, _) in &self.rest {
            if let Some(dojo) = lairs
                .iter()
                .flat_map(|lair| lair.dojos.iter())
                .find(|dojo| dojo.id == *dojo_id)
            {
                layout_splint_ids(&dojo.root, &mut splints);
            }
        }
        splints
    }
}

/// Matches the record against the daemon's Lairs. A recorded Dojo that is
/// gone, or no longer has every pane running, is skipped, as the ordinary
/// reopen path would refuse it; a recorded focus that its Dojo no longer
/// holds falls back to the Dojo's own default.
pub(super) fn plan_window_restore(
    record: &WindowResumeRecord,
    lairs: &[splinterm_core::Lair],
) -> Option<WindowRestorePlan> {
    let mut surviving = Vec::with_capacity(record.tabs.len());
    for (index, tab) in record.tabs.iter().enumerate() {
        let Some(dojo) = lairs
            .iter()
            .flat_map(|lair| lair.dojos.iter())
            .find(|dojo| dojo.id == tab.dojo_id)
        else {
            continue;
        };
        if !dojo_has_fully_running_pane_layout(dojo) {
            continue;
        }
        let focus = tab
            .focus
            .filter(|focus| dojo.root.find_splint(*focus).is_some());
        surviving.push((index, dojo.clone(), focus));
    }
    let active = surviving
        .iter()
        .find(|(index, ..)| *index >= record.active)
        .or_else(|| surviving.last())
        .map(|(_, dojo, _)| dojo.id)?;
    let mut surviving = surviving.into_iter();
    let (_, mut first, focus) = surviving.next()?;
    if let Some(focus) = focus {
        first.default_focus = focus;
    }
    Some(WindowRestorePlan {
        first,
        rest: surviving.map(|(_, dojo, focus)| (dojo.id, focus)).collect(),
        active,
    })
}

/// How the first connection after the relaunch went.
enum Reached {
    Topology {
        lairs: Vec<splinterm_core::Lair>,
        daemon_generation: u64,
    },
    Untrusted,
}

async fn read_topology(factory: &ConnectionFactory) -> Result<Reached> {
    let mut connection = match factory.connect().await {
        Err(error) if refused_as_untrusted(&error) => return Ok(Reached::Untrusted),
        connection => connection?,
    };
    let daemon_generation = connection.daemon_generation();
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its Lairs");
    };
    Ok(Reached::Topology {
        lairs,
        daemon_generation,
    })
}

/// Rebuilds and runs a Window relaunched across a daemon upgrade.
pub(crate) async fn run_resumed(resume: AcquiredWindowResume) -> Result<()> {
    let AcquiredWindowResume {
        record,
        continuation,
        closed_unexpected,
    } = resume;
    eprintln!(
        "splinterm: relaunched across a daemon upgrade from generation {} (prepare {}): {} recorded tabs; {closed_unexpected} stray inherited descriptors closed",
        record.daemon_generation,
        record.prepare_id,
        record.tabs.len(),
    );
    let factory = ConnectionFactory::local();
    let deadline = tokio::time::Instant::now() + RECONNECT_WINDOW;
    let reached = retry_until(deadline, || read_topology(&factory))
        .await
        .context(
            "cannot reach splinterd to restore this Window; reopen its Dojos from the launcher",
        )?;
    let (lairs, daemon_generation) = match reached {
        Reached::Topology {
            lairs,
            daemon_generation,
        } => (lairs, daemon_generation),
        Reached::Untrusted => bail!("{UNTRUSTED_AFTER_UPGRADE}"),
    };
    let plan = plan_window_restore(&record, &lairs);
    // A rollback keeps the old generation; an adoption serves the next one.
    let rolled_back = daemon_generation == record.daemon_generation;
    eprintln!(
        "splinterm: restoring {} of {} tabs from daemon generation {daemon_generation}{}",
        plan.as_ref().map_or(0, WindowRestorePlan::tabs),
        record.tabs.len(),
        if rolled_back { " (rolled back)" } else { "" },
    );
    let Some(plan) = plan else {
        bail!(
            "none of this Window's Dojos is still running in splinterd; reopen them from the launcher"
        );
    };
    if rolled_back {
        // A rollback daemon issues no ticket (ADR 0011): the channel closes
        // and the panes take control the ordinary way.
        drop(continuation);
    } else {
        claim_controllers(
            &factory,
            continuation,
            daemon_generation,
            plan.splints(&lairs),
        )
        .await;
    }

    initialize_graphical()
        .map_err(|_| anyhow::anyhow!("graphical diagnostics initialization failed"))?
        .begin_window(Some(plan.first.id), Some(plan.first.default_focus));
    install_termination_signal_task();
    let ConfigLoad {
        config,
        diagnostics,
    } = load_default()?;
    if !diagnostics.is_empty() {
        if let Some(client_diagnostics) = diagnostics_global() {
            client_diagnostics.emit(
                DiagnosticLevel::Warn,
                DiagnosticEventCode::ConfigWarning,
                Some(DiagnosticErrorCode::Config),
            );
        }
        eprintln!("splinterm configuration warning");
    }
    if rolled_back {
        // The forward daemon failed and the old generation resumed: the
        // restored Window says so briefly (M3.12's notice).
        factory.handoff_status().rolled_back();
    }
    let WindowRestorePlan {
        first,
        rest,
        active,
    } = plan;
    Box::pin(run_resumed_multipane_window(
        config,
        first,
        factory,
        record.app_id,
        WindowResume { tabs: rest, active },
    ))
    .await
}

/// Claims this Window's resume ticket on its continuation socket, which
/// closes on return, and arms it for `splints`, the panes it restores. Any
/// refusal or timeout only costs the restore: a short notice, and the panes
/// take control the ordinary way.
async fn claim_controllers(
    factory: &ConnectionFactory,
    continuation: std::os::fd::OwnedFd,
    daemon_generation: u64,
    splints: Vec<SplintId>,
) {
    match claim_resume_ticket(continuation, daemon_generation, TICKET_CLAIM_DEADLINE).await {
        Ok(ticket) => {
            eprintln!(
                "splinterm: claimed this Window's resume ticket for {} panes",
                splints.len()
            );
            factory
                .handoff_control()
                .arm(ticket, splints, CONTROL_RESTORE_WINDOW);
        }
        Err(error) => eprintln!(
            "splinterm: this Window's controllers are not restored ({error}); its panes take control the ordinary way"
        ),
    }
}

#[cfg(test)]
mod tests {
    use splinterm_core::{Dojo, Lair, LayoutNode, SplintState};
    use splinterm_protocol::window_resume::WindowResumeTab;

    use super::*;

    fn running_dojo(name: &str) -> Dojo {
        let mut dojo = Dojo::with_shell(name, std::env::temp_dir());
        if let LayoutNode::Leaf(splint) = &mut dojo.root {
            splint.state = SplintState::Running;
        }
        dojo
    }

    fn lair_of(dojos: Vec<Dojo>) -> Lair {
        let mut lair = Lair::new("lair", std::env::temp_dir());
        lair.dojos = dojos;
        lair
    }

    fn tab(lair: &Lair, dojo: &Dojo, focus: Option<SplintId>) -> WindowResumeTab {
        WindowResumeTab {
            lair_id: lair.id,
            dojo_id: dojo.id,
            focus,
        }
    }

    fn record(tabs: Vec<WindowResumeTab>, active: usize) -> WindowResumeRecord {
        WindowResumeRecord {
            daemon_generation: 3,
            prepare_id: 1,
            active,
            tabs,
            app_id: None,
        }
    }

    #[test]
    fn surviving_tabs_keep_their_order_focus_and_active_tab() {
        let dojos = (0..3)
            .map(|index| running_dojo(&format!("d{index}")))
            .collect::<Vec<_>>();
        let lair = lair_of(dojos.clone());
        let focus = dojos[0].default_focus;
        let plan = plan_window_restore(
            &record(
                vec![
                    tab(&lair, &dojos[0], Some(focus)),
                    tab(&lair, &dojos[1], None),
                    tab(&lair, &dojos[2], Some(dojos[2].default_focus)),
                ],
                1,
            ),
            std::slice::from_ref(&lair),
        )
        .unwrap();
        assert_eq!(plan.first.id, dojos[0].id);
        assert_eq!(plan.first.default_focus, focus);
        assert_eq!(
            plan.rest,
            vec![
                (dojos[1].id, None),
                (dojos[2].id, Some(dojos[2].default_focus))
            ]
        );
        assert_eq!(plan.active, dojos[1].id);
    }

    #[test]
    fn gone_or_stopped_dojos_and_stale_focus_are_skipped() {
        let dojos = (0..3)
            .map(|index| running_dojo(&format!("d{index}")))
            .collect::<Vec<_>>();
        let mut stopped = dojos[1].clone();
        if let LayoutNode::Leaf(splint) = &mut stopped.root {
            splint.state = SplintState::Exited(0);
        }
        let lair = lair_of(vec![dojos[0].clone(), stopped]);
        let plan = plan_window_restore(
            &record(
                vec![
                    // A focus the Dojo no longer holds falls back to its own.
                    tab(&lair, &dojos[0], Some(SplintId::new())),
                    tab(&lair, &dojos[1], None),
                    tab(&lair, &dojos[2], None),
                ],
                1,
            ),
            std::slice::from_ref(&lair),
        )
        .unwrap();
        assert_eq!(plan.first.id, dojos[0].id);
        assert_eq!(plan.first.default_focus, dojos[0].default_focus);
        assert!(plan.rest.is_empty());
        // The active tab and every tab after it are gone: the last survivor.
        assert_eq!(plan.active, dojos[0].id);

        let unrelated = lair_of(vec![running_dojo("other")]);
        assert!(
            plan_window_restore(&record(vec![tab(&lair, &dojos[2], None)], 0), &[unrelated])
                .is_none()
        );
    }

    #[test]
    fn the_resume_ticket_covers_every_pane_of_the_restored_tabs_only() {
        let dojos = (0..3)
            .map(|index| running_dojo(&format!("d{index}")))
            .collect::<Vec<_>>();
        let lair = lair_of(dojos.clone());
        let plan = plan_window_restore(
            &record(
                vec![tab(&lair, &dojos[0], None), tab(&lair, &dojos[2], None)],
                0,
            ),
            std::slice::from_ref(&lair),
        )
        .unwrap();
        let splints = plan.splints(std::slice::from_ref(&lair));
        // Each test Dojo holds one pane; the unrecorded middle tab is not
        // restored, so its pane never presents the ticket.
        assert_eq!(
            splints,
            vec![dojos[0].default_focus, dojos[2].default_focus]
        );
    }
}
