//! A managed Window's reconnection after a daemon connection lost during a
//! handoff (Plan 0061 M3.12b).
//!
//! The topology manager coordinates it, so the Window's pieces never run
//! reconnect loops of their own. It reconnects with backoff for up to
//! [`RECONNECT_WINDOW`], checks which daemon answered, and resnapshots every
//! tab from the authoritative topology: each live Splint gets a fresh pane
//! (observation attachment, control connection and subscription) that
//! replaces the old one in the frontend, and the old pane tasks are retired.
//! Splints that exited meanwhile take the frontend's exit path. Only then
//! does the frontend end the pause, on `WindowTopologyUpdate::Reconnected`.
//!
//! Nothing is replayed: input typed while paused was dropped, and commands
//! the frontend sent before the pause are handled normally afterwards. A
//! restarted daemon (a new process) is not a continuation, so the Window
//! does not resnapshot from it; it gives up and keeps its last screen.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use splinterm::{
    WindowTopologyUpdate,
    automation::{
        Connection, DaemonChange, DaemonIdentity, ReconnectBackoff, SharedImageContentCache,
    },
    config::AppConfig,
    endpoint::ConnectionFactory,
    handoff_status::RECONNECT_WINDOW,
};
use splinterm_core::{DojoId, LayoutNode, SplintId};
use splinterm_protocol::TopologySnapshot;
use tokio::{sync::mpsc, time::Instant};

use super::{
    PaneTask, TopologyManagerState, cancel_pane_tasks, inspect_managed_topology, layout_splint_ids,
    prepare_live_pane_at_incarnation, remove_frontend_tab, retire_pane_tasks, window_dojo_identity,
};
use crate::app::pane_bridge::{UNTRUSTED_AFTER_UPGRADE, refused_as_untrusted};

/// How a managed Window's reconnection ended.
pub(super) enum ManagedReconnect {
    /// Every tab was resnapshotted from the daemon behind `connection`.
    Resumed {
        connection: Box<Connection>,
        daemon: DaemonIdentity,
    },
    /// Reconnection failed or found a restarted daemon. The Window stays
    /// open, paused, with its last screen.
    Lost,
    /// The frontend went away, or the Window has no tab left.
    WindowGone,
}

/// One reconnection attempt's result, when it reached a daemon.
enum Attempt {
    Resumed {
        connection: Box<Connection>,
        daemon: DaemonIdentity,
    },
    Restarted,
    /// The daemon refused this client build as no longer the installed one.
    Untrusted,
    WindowGone,
}

/// What every reconnection attempt of one managed Window works with.
struct AttemptContext<'a> {
    factory: &'a ConnectionFactory,
    config: &'a AppConfig,
    image_cache: &'a SharedImageContentCache,
    state: &'a mut TopologyManagerState,
    updates: &'a mpsc::Sender<WindowTopologyUpdate>,
    before: DaemonIdentity,
    deadline: Instant,
}

/// Reconnects the Window `state` manages after its daemon connection was
/// lost during a handoff; `before` is the daemon it was connected to.
pub(super) async fn reconnect_managed_window(
    factory: &ConnectionFactory,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    state: &mut TopologyManagerState,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    before: DaemonIdentity,
) -> ManagedReconnect {
    let hub = factory.handoff_status();
    let deadline = Instant::now() + RECONNECT_WINDOW;
    let mut context = AttemptContext {
        factory,
        config,
        image_cache,
        state,
        updates,
        before,
        deadline,
    };
    let outcome = retry_reconnection(deadline, updates.closed(), &mut context, |context| {
        Box::pin(attempt(
            context.factory,
            context.config,
            context.image_cache,
            context.state,
            context.updates,
            context.before,
            context.deadline,
        ))
    })
    .await;
    match outcome {
        Retry::Reached(Attempt::Resumed { connection, daemon }) => {
            if let DaemonChange::Upgraded { from, to } = before.change_to(daemon) {
                eprintln!(
                    "splinterm: reconnected to upgraded splinterd (generation {from} → {to})"
                );
            }
            ManagedReconnect::Resumed { connection, daemon }
        }
        Retry::Reached(Attempt::Restarted) => {
            eprintln!(
                "splinterm: splinterd restarted during the upgrade; this Window keeps its last screen"
            );
            hub.reconnect_failed();
            ManagedReconnect::Lost
        }
        Retry::Reached(Attempt::Untrusted) => {
            eprintln!("{UNTRUSTED_AFTER_UPGRADE}");
            hub.reconnect_failed();
            ManagedReconnect::Lost
        }
        Retry::Reached(Attempt::WindowGone) | Retry::Closed => ManagedReconnect::WindowGone,
        Retry::TimedOut(error) => {
            eprintln!("splinterm: could not reconnect to splinterd: {error:#}");
            hub.reconnect_failed();
            ManagedReconnect::Lost
        }
    }
}

/// How [`retry_reconnection`] ended.
#[derive(Debug)]
pub(super) enum Retry<T> {
    /// An attempt reached the daemon.
    Reached(T),
    /// Every attempt failed until the deadline; the last error.
    TimedOut(anyhow::Error),
    /// The Window closed meanwhile.
    Closed,
}

/// One reconnection attempt's future, borrowing the state it works on.
pub(super) type AttemptFuture<'a, T> =
    std::pin::Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// The reconnection loop apart from what one attempt does: backoff between
/// failed attempts, the deadline, and the Window closing. Each attempt gets
/// `context` lent to it. A Window closed meanwhile ends the loop at once;
/// panes an abandoned attempt prepared end with their dropped queues.
pub(super) async fn retry_reconnection<C, T>(
    deadline: Instant,
    closed: impl Future<Output = ()>,
    context: &mut C,
    mut attempt: impl for<'a> FnMut(&'a mut C) -> AttemptFuture<'a, T>,
) -> Retry<T> {
    tokio::pin!(closed);
    let mut backoff = ReconnectBackoff::default();
    loop {
        let error = tokio::select! {
            biased;
            () = &mut closed => return Retry::Closed,
            outcome = attempt(context) => match outcome {
                Ok(reached) => return Retry::Reached(reached),
                Err(error) => error,
            },
        };
        let resume = Instant::now() + backoff.next_delay();
        if resume >= deadline {
            return Retry::TimedOut(error);
        }
        tokio::select! {
            biased;
            () = &mut closed => return Retry::Closed,
            () = tokio::time::sleep_until(resume) => {}
        }
    }
}

/// Connects, checks the daemon, and resnapshots every tab. Each daemon wait
/// is bounded by `deadline`; panes prepared before a failure are cancelled.
async fn attempt(
    factory: &ConnectionFactory,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    state: &mut TopologyManagerState,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    before: DaemonIdentity,
    deadline: Instant,
) -> Result<Attempt> {
    let mut connection = match tokio::time::timeout_at(deadline, factory.connect()).await {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) if refused_as_untrusted(&error) => return Ok(Attempt::Untrusted),
        Ok(Err(error)) => return Err(error),
        Err(_) => anyhow::bail!("splinterd did not answer before the reconnection deadline"),
    };
    let daemon = DaemonIdentity::of(&connection);
    if before.change_to(daemon) == DaemonChange::Restarted {
        return Ok(Attempt::Restarted);
    }
    let snapshot = tokio::time::timeout_at(deadline, inspect_managed_topology(&mut connection))
        .await
        .context("splinterd did not return topology before the reconnection deadline")??;
    let Some(exited) = resnapshot_managed_tabs(
        factory,
        config,
        image_cache,
        state,
        &snapshot,
        updates,
        deadline,
    )
    .await?
    else {
        return Ok(Attempt::WindowGone);
    };
    if updates
        .send(WindowTopologyUpdate::Reconnected {
            daemon_generation: daemon.generation,
            exited,
        })
        .await
        .is_err()
    {
        return Ok(Attempt::WindowGone);
    }
    Ok(Attempt::Resumed {
        connection: Box::new(connection),
        daemon,
    })
}

/// What resnapshotting one tab's layout needs: fresh panes for the live
/// Splints, and the Splints that exited while the connection was lost.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct TabResnapshotPlan {
    /// Live Splints with the incarnation to attach, in layout order.
    pub(super) attach: Vec<(SplintId, u64)>,
    /// Splints the frontend already shows that have no live process now:
    /// their pane is kept and takes the exit path.
    pub(super) exited: Vec<SplintId>,
    /// Splints whose frontend pane is replaced or dropped.
    pub(super) removed: Vec<SplintId>,
}

/// Plans one tab's resnapshot from its previous and authoritative layouts.
///
/// # Errors
///
/// When the authoritative layout holds a Splint the tab never showed that
/// has no live process: no pane can show it.
pub(super) fn plan_tab_resnapshot(
    previous: &LayoutNode,
    next: &LayoutNode,
    live: &HashMap<SplintId, u64>,
) -> Result<TabResnapshotPlan> {
    let mut previous_ids = Vec::new();
    layout_splint_ids(previous, &mut previous_ids);
    let mut next_ids = Vec::new();
    layout_splint_ids(next, &mut next_ids);
    let shown = previous_ids.iter().copied().collect::<HashSet<_>>();
    let mut plan = TabResnapshotPlan::default();
    for splint_id in next_ids {
        match live.get(&splint_id) {
            Some(incarnation) => plan.attach.push((splint_id, *incarnation)),
            None if shown.contains(&splint_id) => plan.exited.push(splint_id),
            None => anyhow::bail!("a Splint added during the upgrade has no live process"),
        }
    }
    plan.removed = previous_ids
        .into_iter()
        .filter(|splint_id| !plan.exited.contains(splint_id))
        .collect();
    Ok(plan)
}

/// Replaces every managed tab's panes with ones attached to the serving
/// daemon and reconciles tab identity and layout with `snapshot`. Returns
/// the Splints that exited meanwhile, by tab, or `None` once the frontend
/// went away or no tab is left.
#[allow(
    clippy::too_many_lines,
    reason = "one transactional resnapshot owns identity, layout, panes, and task cleanup per tab"
)]
async fn resnapshot_managed_tabs(
    factory: &ConnectionFactory,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    state: &mut TopologyManagerState,
    snapshot: &TopologySnapshot,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    deadline: Instant,
) -> Result<Option<Vec<(DojoId, SplintId)>>> {
    let authoritative = snapshot
        .topology
        .lairs()
        .flat_map(|lair| {
            lair.dojos.iter().map(move |dojo| {
                (
                    dojo.id,
                    (
                        window_dojo_identity(snapshot.revision, lair, dojo),
                        dojo.root.clone(),
                    ),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let live = snapshot
        .runtimes
        .iter()
        .filter_map(|runtime| {
            runtime
                .live_incarnation
                .map(|incarnation| (runtime.splint_id, incarnation))
        })
        .collect::<HashMap<_, _>>();
    let mut exited = Vec::new();
    let dojo_ids = state.tabs.iter().map(|tab| tab.dojo_id).collect::<Vec<_>>();
    for dojo_id in dojo_ids {
        let Some((identity, root)) = authoritative.get(&dojo_id).cloned() else {
            // The Dojo closed meanwhile: its tab goes, as in reconciliation.
            if let Some(removed) = state.tabs.close(dojo_id) {
                let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                retire_pane_tasks(removed.value.pane_tasks);
                if !acknowledged {
                    return Ok(None);
                }
            }
            continue;
        };
        let managed = &mut state
            .tabs
            .get_mut(dojo_id)
            .context("managed Dojo disappeared during reconnection")?
            .value;
        if managed.identity != identity {
            if updates
                .send(WindowTopologyUpdate::UpdateIdentity(identity.clone()))
                .await
                .is_err()
            {
                return Ok(None);
            }
            managed.identity = identity;
        }
        let plan = plan_tab_resnapshot(&managed.root, &root, &live)?;
        let mut added = Vec::with_capacity(plan.attach.len());
        let mut new_tasks: HashMap<SplintId, PaneTask> = HashMap::with_capacity(plan.attach.len());
        for (splint_id, incarnation) in plan.attach {
            let prepared = tokio::time::timeout_at(
                deadline,
                prepare_live_pane_at_incarnation(
                    factory,
                    config,
                    splint_id,
                    Some(incarnation),
                    image_cache.clone(),
                    false,
                ),
            )
            .await
            .context("a pane did not reattach before the reconnection deadline")
            .and_then(|result| result);
            match prepared {
                Ok(pane) => {
                    added.push(pane.options);
                    new_tasks.insert(splint_id, pane.task);
                }
                Err(error) => {
                    cancel_pane_tasks(new_tasks).await;
                    return Err(error);
                }
            }
        }
        exited.extend(plan.exited.iter().map(|splint_id| (dojo_id, *splint_id)));
        if updates
            .send(WindowTopologyUpdate::Apply {
                topology_revision: snapshot.revision,
                dojo_id,
                layout: root.clone(),
                added,
                removed: plan.removed,
                focused: None,
            })
            .await
            .is_err()
        {
            cancel_pane_tasks(new_tasks).await;
            return Ok(None);
        }
        retire_pane_tasks(std::mem::replace(&mut managed.pane_tasks, new_tasks));
        managed.root = root;
        managed.pending_focus = None;
    }
    if state.tabs.is_empty() {
        let _ = updates.send(WindowTopologyUpdate::Closed).await;
        return Ok(None);
    }
    Ok(Some(exited))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use splinterm_core::{Axis, Splint, SplitRatio};

    use super::*;

    fn leaf(id: SplintId) -> LayoutNode {
        let mut splint = Splint::shell(PathBuf::from("/tmp"));
        splint.id = id;
        LayoutNode::Leaf(splint)
    }

    fn split(first: LayoutNode, second: LayoutNode) -> LayoutNode {
        LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// A Window whose only pane lost its connection while `phase` was heard.
    fn reconnecting_window(
        phase: splinterm_protocol::HandoffStatusPhase,
    ) -> splinterm::handoff_status::HandoffStatusHub {
        let hub = splinterm::handoff_status::HandoffStatusHub::default();
        let pane = hub.source();
        pane.observe(4, phase);
        pane.connection_lost();
        assert!(hub.reconnect_pending().is_some());
        hub
    }

    #[tokio::test(start_paused = true)]
    async fn retries_back_off_until_the_daemon_answers_then_the_window_resumes() {
        use splinterm::handoff_status::WindowHandoffPhase;
        let hub = reconnecting_window(splinterm_protocol::HandoffStatusPhase::Fenced);
        let started = Instant::now();
        let mut attempts = 0_u32;
        let outcome = retry_reconnection(
            started + RECONNECT_WINDOW,
            std::future::pending(),
            &mut attempts,
            |attempts| {
                Box::pin(async move {
                    *attempts += 1;
                    // The new image is still adopting: the first two attempts fail.
                    anyhow::ensure!(*attempts > 2, "connection refused");
                    Ok(5_u64)
                })
            },
        )
        .await;
        let Retry::Reached(generation) = outcome else {
            panic!("the third attempt reaches the daemon: {outcome:?}");
        };
        assert_eq!(attempts, 3);
        assert_eq!(started.elapsed(), std::time::Duration::from_millis(300));
        // Still paused until the frontend applied the resnapshot.
        assert_eq!(hub.view().phase, WindowHandoffPhase::Reconnecting);
        hub.reconnected(generation);
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
        assert!(!hub.input_paused());
    }

    #[tokio::test(start_paused = true)]
    async fn the_exec_race_reconnects_and_a_same_generation_daemon_simply_resumes() {
        use splinterm::handoff_status::WindowHandoffPhase;
        // Only Preparing was heard before the loss.
        let hub = reconnecting_window(splinterm_protocol::HandoffStatusPhase::Preparing);
        let before = DaemonIdentity {
            generation: 4,
            pid: Some(40),
        };
        let outcome = retry_reconnection(
            Instant::now() + RECONNECT_WINDOW,
            std::future::pending(),
            &mut (),
            |()| {
                Box::pin(async {
                    Ok(DaemonIdentity {
                        generation: 4,
                        pid: Some(40),
                    })
                })
            },
        )
        .await;
        let Retry::Reached(daemon) = outcome else {
            panic!("the daemon answers at once: {outcome:?}");
        };
        assert_eq!(before.change_to(daemon), DaemonChange::Same);
        hub.reconnected(daemon.generation);
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
        assert!(hub.view().notice.is_none());
    }

    #[test]
    fn a_generation_change_is_told_from_a_restart() {
        let before = DaemonIdentity {
            generation: 4,
            pid: Some(40),
        };
        assert_eq!(
            before.change_to(DaemonIdentity {
                generation: 5,
                pid: Some(40),
            }),
            DaemonChange::Upgraded { from: 4, to: 5 }
        );
        // A restarted daemon is not a continuation: the Window gives up
        // instead of resnapshotting from it.
        assert_eq!(
            before.change_to(DaemonIdentity {
                generation: 1,
                pid: Some(77),
            }),
            DaemonChange::Restarted
        );
    }

    #[tokio::test(start_paused = true)]
    async fn timing_out_leaves_the_window_open_lost_and_paused() {
        use splinterm::handoff_status::WindowHandoffPhase;
        let hub = reconnecting_window(splinterm_protocol::HandoffStatusPhase::Fenced);
        let started = Instant::now();
        let outcome: Retry<()> = retry_reconnection(
            started + RECONNECT_WINDOW,
            std::future::pending(),
            &mut (),
            |()| Box::pin(async { anyhow::bail!("connection refused") }),
        )
        .await;
        let Retry::TimedOut(error) = outcome else {
            panic!("every attempt fails: {outcome:?}");
        };
        assert!(error.to_string().contains("connection refused"));
        assert!(started.elapsed() <= RECONNECT_WINDOW);
        assert!(started.elapsed() >= RECONNECT_WINDOW - std::time::Duration::from_secs(2));
        // What the coordinator does on a timeout.
        hub.reconnect_failed();
        assert_eq!(hub.view().phase, WindowHandoffPhase::Lost);
        assert!(hub.input_paused(), "nothing typed reaches the daemon");
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_closed_meanwhile_ends_the_reconnection_at_once() {
        let started = Instant::now();
        let outcome: Retry<()> = retry_reconnection(
            started + RECONNECT_WINDOW,
            tokio::time::sleep(std::time::Duration::from_secs(1)),
            &mut (),
            |()| Box::pin(async { anyhow::bail!("connection refused") }),
        )
        .await;
        assert!(matches!(outcome, Retry::Closed));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn every_live_splint_is_reattached_and_every_old_pane_replaced() {
        let (left, right) = (SplintId::new(), SplintId::new());
        let layout = split(leaf(left), leaf(right));
        let live = HashMap::from([(left, 3), (right, 5)]);
        let plan = plan_tab_resnapshot(&layout, &layout, &live).unwrap();
        assert_eq!(plan.attach, vec![(left, 3), (right, 5)]);
        assert!(plan.exited.is_empty());
        assert_eq!(plan.removed, vec![left, right]);
    }

    #[test]
    fn a_splint_that_exited_meanwhile_keeps_its_pane_for_the_exit_path() {
        let (left, right) = (SplintId::new(), SplintId::new());
        let layout = split(leaf(left), leaf(right));
        let plan = plan_tab_resnapshot(&layout, &layout, &HashMap::from([(left, 3)])).unwrap();
        assert_eq!(plan.attach, vec![(left, 3)]);
        assert_eq!(plan.exited, vec![right]);
        assert_eq!(plan.removed, vec![left], "the exited pane stays shown");
    }

    #[test]
    fn a_closed_splint_is_dropped_and_a_new_live_one_attached() {
        let (kept, closed, added) = (SplintId::new(), SplintId::new(), SplintId::new());
        let previous = split(leaf(kept), leaf(closed));
        let next = split(leaf(kept), leaf(added));
        let live = HashMap::from([(kept, 1), (added, 1)]);
        let plan = plan_tab_resnapshot(&previous, &next, &live).unwrap();
        assert_eq!(plan.attach, vec![(kept, 1), (added, 1)]);
        assert_eq!(plan.removed, vec![kept, closed]);
    }

    #[test]
    fn an_unseen_splint_without_a_process_cannot_be_shown() {
        let (kept, ghost) = (SplintId::new(), SplintId::new());
        let previous = leaf(kept);
        let next = split(leaf(kept), leaf(ghost));
        assert!(plan_tab_resnapshot(&previous, &next, &HashMap::from([(kept, 1)])).is_err());
    }
}
