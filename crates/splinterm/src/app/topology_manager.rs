use std::{
    collections::{HashMap, HashSet},
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use splinterm::{
    SessionPickerItem, WindowDojoIdentity, WindowPaneOptions, WindowTopologyCommand,
    WindowTopologyUpdate,
    automation::{Connection, SharedImageContentCache, protocol_error},
    config::AppConfig,
    endpoint::{ConnectionFactory, LaunchSemantics},
    session_picker::{SessionEntry, collect_sessions},
    tab::{DojoTab, OpenTabOutcome, WindowTabSet},
};
use splinterm_core::{
    Axis, DojoId, LairId, LayoutNode, SplintId, SplintState, SplitRatio, SplitSide,
    TopologyRevision,
};
use splinterm_protocol::{ErrorCode, Request, Response};
use tokio::sync::mpsc;

use super::{
    pane_bridge::{PaneTask, layout_splint_ids, prepare_live_pane},
    session_catalog::{
        automation_launch, create_request, launch_parameters, recent_dojo_ids, remember_dojo,
        select_dojo_from, session_picker_item,
    },
};

fn parent_ratio(root: &LayoutNode, target: SplintId) -> Option<SplitRatio> {
    match root {
        LayoutNode::Leaf(_) => None,
        LayoutNode::Branch {
            ratio,
            first,
            second,
            ..
        } => {
            let direct_child =
                |node: &LayoutNode| matches!(node, LayoutNode::Leaf(splint) if splint.id == target);
            if direct_child(first) || direct_child(second) {
                Some(*ratio)
            } else {
                parent_ratio(first, target).or_else(|| parent_ratio(second, target))
            }
        }
    }
}

async fn inspect_optional_dojo_state(
    connection: &mut Connection,
    dojo_id: DojoId,
) -> Result<(TopologyRevision, Option<LayoutNode>)> {
    let Response::Topology { snapshot } = connection.request(Request::InspectTopology).await?
    else {
        bail!("splinterd did not return topology after edit");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let root = snapshot
        .topology
        .lairs()
        .flat_map(|dojo| &dojo.dojos)
        .find(|dojo| dojo.id == dojo_id)
        .map(|dojo| dojo.root.clone());
    Ok((snapshot.revision, root))
}

async fn inspect_dojo_state(
    connection: &mut Connection,
    dojo_id: DojoId,
) -> Result<(TopologyRevision, LayoutNode)> {
    let (revision, root) = inspect_optional_dojo_state(connection, dojo_id).await?;
    Ok((
        revision,
        root.context("edited Dojo is absent from committed topology")?,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseAction {
    CloseExited,
    KillAndClose { incarnation: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingTopologyFocus {
    splint_id: SplintId,
    revision: TopologyRevision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TopologyCommandOutcome {
    Updated {
        pending_focus: Option<PendingTopologyFocus>,
    },
    WindowClosed,
}

const MAX_CLOSE_TOPOLOGY_RETRIES: usize = 64;

fn close_action(root: &LayoutNode, target: SplintId) -> Result<CloseAction> {
    let splint = root
        .find_splint(target)
        .context("focused pane is absent from committed topology")?;
    if matches!(splint.state, SplintState::Exited(_)) {
        return Ok(CloseAction::CloseExited);
    }
    Ok(CloseAction::KillAndClose {
        incarnation: splint
            .last_incarnation
            .context("live focused pane has no process incarnation")?,
    })
}

fn validate_exited_close_target(
    root: &LayoutNode,
    target: SplintId,
    expected_incarnation: Option<u64>,
) -> Result<bool> {
    let splint = root
        .find_splint(target)
        .context("focused pane is absent from committed topology")?;
    anyhow::ensure!(
        matches!(splint.state, SplintState::Exited(_)),
        "pane remained live before close"
    );
    if let Some(expected) = expected_incarnation {
        anyhow::ensure!(
            splint.last_incarnation == Some(expected),
            "pane incarnation changed before close"
        );
    }
    Ok(root.splint_count() == 1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefreshedCloseState {
    WindowClosed,
    TargetClosed,
    Retry,
}

fn refreshed_close_state(
    root: Option<&LayoutNode>,
    target: SplintId,
    expected_incarnation: Option<u64>,
) -> Result<RefreshedCloseState> {
    let Some(root) = root else {
        return Ok(RefreshedCloseState::WindowClosed);
    };
    if root.find_splint(target).is_none() {
        return Ok(RefreshedCloseState::TargetClosed);
    }
    validate_exited_close_target(root, target, expected_incarnation)?;
    Ok(RefreshedCloseState::Retry)
}

fn captured_dojo_kill_targets(
    root: &LayoutNode,
    captured: &[(SplintId, u64)],
) -> Result<Vec<(SplintId, u64)>> {
    anyhow::ensure!(
        !captured.is_empty() && captured.len() == root.splint_count(),
        "captured Dojo pane set changed before termination"
    );
    let mut unique = HashSet::with_capacity(captured.len());
    let mut live = Vec::with_capacity(captured.len());
    for &(splint_id, incarnation) in captured {
        anyhow::ensure!(
            unique.insert(splint_id),
            "captured Dojo pane set contains a duplicate"
        );
        let splint = root
            .find_splint(splint_id)
            .context("captured Dojo pane disappeared before termination")?;
        anyhow::ensure!(
            splint.last_incarnation == Some(incarnation),
            "captured Dojo pane incarnation changed before termination"
        );
        if !matches!(splint.state, SplintState::Exited(_)) {
            live.push((splint_id, incarnation));
        }
    }
    Ok(live)
}

async fn terminate_dojo(
    connection: &mut Connection,
    dojo_id: DojoId,
    captured: &[(SplintId, u64)],
) -> Result<()> {
    for &(splint_id, incarnation) in captured {
        let (_, Some(root)) = inspect_optional_dojo_state(connection, dojo_id).await? else {
            return Ok(());
        };
        let live = captured_dojo_kill_targets(&root, captured)?;
        if !live.contains(&(splint_id, incarnation)) {
            continue;
        }
        match connection
            .request(Request::KillSplint {
                splint_id,
                incarnation,
            })
            .await?
        {
            Response::SplintKilled {
                splint_id: killed_id,
                incarnation: killed_incarnation,
                ..
            } if killed_id == splint_id && killed_incarnation == incarnation => {}
            response => bail!("splinterd returned unexpected Dojo kill response: {response:?}"),
        }
    }

    let (mut revision, root) = inspect_optional_dojo_state(connection, dojo_id).await?;
    let Some(root) = root else {
        return Ok(());
    };
    anyhow::ensure!(
        captured_dojo_kill_targets(&root, captured)?.is_empty(),
        "captured Dojo retained a live pane after termination"
    );
    for attempt in 0..=MAX_CLOSE_TOPOLOGY_RETRIES {
        match connection
            .request(Request::CloseDojo {
                expected_topology_revision: revision,
                dojo_id,
            })
            .await
        {
            Ok(Response::TopologyCommitted { .. }) => return Ok(()),
            Ok(response) => {
                bail!("splinterd returned unexpected Dojo close response: {response:?}");
            }
            Err(error)
                if protocol_error(&error).is_some_and(|failure| {
                    matches!(failure.code, ErrorCode::NotFound | ErrorCode::StaleTopology)
                }) =>
            {
                let (refreshed_revision, refreshed_root) =
                    inspect_optional_dojo_state(connection, dojo_id).await?;
                let Some(refreshed_root) = refreshed_root else {
                    return Ok(());
                };
                anyhow::ensure!(
                    captured_dojo_kill_targets(&refreshed_root, captured)?.is_empty(),
                    "captured Dojo changed during closure"
                );
                if attempt == MAX_CLOSE_TOPOLOGY_RETRIES {
                    return Err(error).context("bounded Dojo close retries exhausted");
                }
                revision = refreshed_revision;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded Dojo close retry loop returns on its final attempt")
}

async fn close_focused_splint(
    connection: &mut Connection,
    dojo_id: DojoId,
    root: &LayoutNode,
    expected_topology_revision: TopologyRevision,
    target: SplintId,
) -> Result<TopologyCommandOutcome> {
    let (mut close_revision, mut close_root, expected_incarnation) =
        match close_action(root, target)? {
            CloseAction::CloseExited => (expected_topology_revision, root.clone(), None),
            CloseAction::KillAndClose { incarnation } => {
                match connection
                    .request(Request::KillSplint {
                        splint_id: target,
                        incarnation,
                    })
                    .await?
                {
                    Response::SplintKilled {
                        splint_id,
                        incarnation: killed_incarnation,
                        ..
                    } if splint_id == target && killed_incarnation == incarnation => {}
                    response => bail!("splinterd returned unexpected kill response: {response:?}"),
                }
                let (revision, refreshed_root) = inspect_dojo_state(connection, dojo_id).await?;
                (revision, refreshed_root, Some(incarnation))
            }
        };

    for attempt in 0..=MAX_CLOSE_TOPOLOGY_RETRIES {
        let final_leaf = validate_exited_close_target(&close_root, target, expected_incarnation)?;
        match connection
            .request(Request::CloseSplint {
                expected_topology_revision: close_revision,
                splint_id: target,
            })
            .await
        {
            Ok(Response::TopologyCommitted { .. }) if final_leaf => {
                return Ok(TopologyCommandOutcome::WindowClosed);
            }
            Ok(Response::TopologyCommitted { .. }) => {
                return Ok(TopologyCommandOutcome::Updated {
                    pending_focus: None,
                });
            }
            Ok(response) => {
                bail!("splinterd returned unexpected close response: {response:?}");
            }
            Err(error)
                if protocol_error(&error)
                    .is_some_and(|failure| failure.code == ErrorCode::StaleTopology)
                    && attempt < MAX_CLOSE_TOPOLOGY_RETRIES =>
            {
                let (revision, refreshed_root) =
                    inspect_optional_dojo_state(connection, dojo_id).await?;
                match refreshed_close_state(refreshed_root.as_ref(), target, expected_incarnation)?
                {
                    RefreshedCloseState::WindowClosed => {
                        return Ok(TopologyCommandOutcome::WindowClosed);
                    }
                    RefreshedCloseState::TargetClosed => {
                        return Ok(TopologyCommandOutcome::Updated {
                            pending_focus: None,
                        });
                    }
                    RefreshedCloseState::Retry => {}
                }
                close_revision = revision;
                close_root = refreshed_root.context("close retry Dojo disappeared")?;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded close retry loop returns on its final attempt")
}

#[allow(
    clippy::too_many_lines,
    reason = "the closed topology command set preserves exact endpoint and revision handling"
)]
async fn apply_topology_command(
    factory: &ConnectionFactory,
    connection: &mut Connection,
    config: &AppConfig,
    dojo_id: DojoId,
    root: &LayoutNode,
    expected_topology_revision: TopologyRevision,
    command: WindowTopologyCommand,
) -> Result<TopologyCommandOutcome> {
    if let WindowTopologyCommand::Close {
        dojo_id: target_dojo,
        target,
    } = &command
    {
        anyhow::ensure!(
            *target_dojo == dojo_id,
            "topology close targeted another Dojo"
        );
        return close_focused_splint(
            connection,
            dojo_id,
            root,
            expected_topology_revision,
            *target,
        )
        .await;
    }
    let request = match command {
        WindowTopologyCommand::Split {
            dojo_id: target_dojo,
            target,
            axis,
        } => {
            anyhow::ensure!(
                target_dojo == dojo_id,
                "topology split targeted another Dojo"
            );
            let ratio = SplitRatio::new(500).expect("fixed split ratio is valid");
            match factory.capabilities().launch_semantics {
                LaunchSemantics::LocalTrusted => Request::SplitSplint {
                    expected_topology_revision,
                    target_splint_id: target,
                    axis,
                    side: SplitSide::Second,
                    ratio,
                    launch: launch_parameters(
                        env::current_dir().context("failed to read current directory")?,
                        Vec::new(),
                        config,
                    ),
                },
                LaunchSemantics::RemoteAutomation => Request::SplitSplintAutomation {
                    expected_topology_revision,
                    target_splint_id: target,
                    axis,
                    side: SplitSide::Second,
                    ratio,
                    launch: automation_launch(None, Vec::new()),
                },
            }
        }
        WindowTopologyCommand::AdjustRatio {
            dojo_id: target_dojo,
            target,
            delta,
        } => {
            anyhow::ensure!(target_dojo == dojo_id, "ratio edit targeted another Dojo");
            let current = i32::from(
                parent_ratio(root, target)
                    .context("focused pane has no adjustable parent ratio")?
                    .get(),
            );
            let next = u16::try_from((current + i32::from(delta)).clamp(1, 999))?;
            Request::SetSplitRatio {
                expected_topology_revision,
                target_splint_id: target,
                ancestor: 0,
                ratio: SplitRatio::new(next).map_err(|_| anyhow::anyhow!("invalid ratio"))?,
            }
        }
        WindowTopologyCommand::SetRatio {
            dojo_id: target_dojo,
            target,
            ancestor,
            ratio,
        } => {
            anyhow::ensure!(target_dojo == dojo_id, "ratio edit targeted another Dojo");
            Request::SetSplitRatio {
                expected_topology_revision,
                target_splint_id: target,
                ancestor,
                ratio,
            }
        }
        WindowTopologyCommand::Close { .. } => unreachable!("close handled above"),
        WindowTopologyCommand::RequestSessionPicker
        | WindowTopologyCommand::OpenDojo { .. }
        | WindowTopologyCommand::NewLair
        | WindowTopologyCommand::NewDojo { .. }
        | WindowTopologyCommand::RenameDojo { .. }
        | WindowTopologyCommand::TerminateDojo { .. }
        | WindowTopologyCommand::ActivateTab { .. }
        | WindowTopologyCommand::CloseTab { .. }
        | WindowTopologyCommand::CloseTabs { .. } => {
            unreachable!("session commands are handled by the topology manager")
        }
    };
    topology_command_outcome(connection.request(request).await?)
}

fn topology_command_outcome(response: Response) -> Result<TopologyCommandOutcome> {
    match response {
        Response::SplintStarted {
            splint_id,
            topology_revision,
            ..
        } => Ok(TopologyCommandOutcome::Updated {
            pending_focus: Some(PendingTopologyFocus {
                splint_id,
                revision: topology_revision,
            }),
        }),
        Response::TopologyCommitted { .. } => Ok(TopologyCommandOutcome::Updated {
            pending_focus: None,
        }),
        response => bail!("splinterd returned unexpected topology response: {response:?}"),
    }
}

fn topology_identity_diff(
    previous: &LayoutNode,
    current: &LayoutNode,
) -> (Vec<SplintId>, Vec<SplintId>) {
    let mut previous_ids = Vec::new();
    let mut current_ids = Vec::new();
    layout_splint_ids(previous, &mut previous_ids);
    layout_splint_ids(current, &mut current_ids);
    let previous = previous_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let current = current_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    (
        current.difference(&previous).copied().collect(),
        previous.difference(&current).copied().collect(),
    )
}

fn pending_focus_for_observation(
    pending: Option<PendingTopologyFocus>,
    observed_revision: TopologyRevision,
    added: &[SplintId],
) -> (Option<SplintId>, bool) {
    let Some(pending) = pending else {
        return (None, false);
    };
    if observed_revision < pending.revision {
        return (None, false);
    }
    (
        added
            .contains(&pending.splint_id)
            .then_some(pending.splint_id),
        true,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "one transactional reconciliation owns identity, layout, updates, and task cleanup"
)]
async fn reconcile_window_topology(
    factory: &ConnectionFactory,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    dojo_id: DojoId,
    root: &mut LayoutNode,
    next: LayoutNode,
    focused: Option<SplintId>,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: &mut HashMap<SplintId, PaneTask>,
) -> Result<bool> {
    if *root == next {
        return Ok(true);
    }
    let (added_ids, removed) = topology_identity_diff(root, &next);
    let mut prepared = Vec::new();
    for splint_id in added_ids {
        match prepare_live_pane(factory, config, splint_id, image_cache.clone(), false).await {
            Ok(pane) => prepared.push((splint_id, pane)),
            Err(error) => {
                let tasks = prepared
                    .into_iter()
                    .map(|(splint_id, pane)| (splint_id, pane.task))
                    .collect();
                cancel_pane_tasks(tasks).await;
                return Err(error);
            }
        }
    }
    let mut added = Vec::with_capacity(prepared.len());
    let mut new_tasks = HashMap::with_capacity(prepared.len());
    for (splint_id, pane) in prepared {
        added.push(pane.options);
        new_tasks.insert(splint_id, pane.task);
    }
    if updates
        .send(WindowTopologyUpdate::Apply {
            dojo_id,
            layout: next.clone(),
            added,
            removed: removed.clone(),
            focused,
        })
        .await
        .is_err()
    {
        cancel_pane_tasks(new_tasks).await;
        return Ok(false);
    }
    let mut removed_tasks = HashMap::new();
    for removed_id in &removed {
        if let Some(task) = pane_tasks.remove(removed_id) {
            removed_tasks.insert(*removed_id, task);
        }
    }
    cancel_pane_tasks(removed_tasks).await;
    pane_tasks.extend(new_tasks);
    *root = next;
    Ok(true)
}

async fn session_picker_catalog(
    factory: &ConnectionFactory,
    connection: &mut Connection,
) -> Result<(Vec<SessionPickerItem>, Vec<(LairId, DojoId)>)> {
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let entries = collect_sessions(&lairs, &recent_dojo_ids(factory))
        .into_iter()
        .filter(SessionEntry::reopenable)
        .collect::<Vec<_>>();
    let items = entries.iter().map(session_picker_item).collect();
    let targets = entries
        .iter()
        .map(|entry| (entry.lair_id, entry.dojo_id))
        .collect();
    Ok((items, targets))
}

fn window_dojo_identity(
    lair: &splinterm_core::Lair,
    dojo: &splinterm_core::Dojo,
) -> WindowDojoIdentity {
    WindowDojoIdentity {
        lair_id: lair.id,
        dojo_id: dojo.id,
        lair_name: lair.name.clone(),
        dojo_name: dojo.name.clone(),
    }
}

async fn reopenable_dojo(
    connection: &mut Connection,
    lair_id: LairId,
    dojo_id: DojoId,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let lair = lairs
        .iter()
        .find(|lair| lair.id == lair_id)
        .context("selected Lair is absent")?;
    let dojo = select_dojo_from(&lairs, (lair_id, dojo_id))?;
    anyhow::ensure!(
        collect_sessions(&lairs, &[])
            .into_iter()
            .any(|entry| entry.dojo_id == dojo_id && entry.reopenable()),
        "selected session no longer has a fully running pane layout"
    );
    Ok((window_dojo_identity(lair, &dojo), dojo))
}

async fn create_daily_dojo(
    factory: &ConnectionFactory,
    connection: &mut Connection,
    config: &AppConfig,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expected = connection.topology_revision().await?;
    let Response::LairCreated { lair, .. } = connection
        .request(create_request(
            factory,
            expected,
            format!("terminal-{stamp}-{}", std::process::id()),
            if factory.is_local() {
                Some(env::current_dir().context("failed to read current directory")?)
            } else {
                None
            },
            Vec::new(),
            config,
        )?)
        .await?
    else {
        bail!("splinterd did not create the requested terminal");
    };
    let dojo = lair
        .dojos
        .first()
        .cloned()
        .context("new Lair did not contain a Dojo")?;
    Ok((window_dojo_identity(&lair, &dojo), dojo))
}

async fn create_dojo_in_lair(
    factory: &ConnectionFactory,
    connection: &mut Connection,
    config: &AppConfig,
    lair_id: LairId,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let expected_topology_revision = connection.topology_revision().await?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let request = match factory.capabilities().launch_semantics {
        LaunchSemantics::LocalTrusted => Request::NewDojo {
            expected_topology_revision,
            lair_id,
            name: format!("terminal-{stamp}"),
            launch: launch_parameters(
                env::current_dir().context("failed to read current directory")?,
                Vec::new(),
                config,
            ),
        },
        LaunchSemantics::RemoteAutomation => Request::NewDojoAutomation {
            expected_topology_revision,
            lair_id,
            name: format!("terminal-{stamp}"),
            launch: automation_launch(None, Vec::new()),
        },
    };
    let Response::DojoStarted { dojo_id, .. } = connection.request(request).await? else {
        bail!("splinterd did not create the requested Dojo");
    };
    reopenable_dojo(connection, lair_id, dojo_id).await
}

struct ManagedDojo {
    identity: WindowDojoIdentity,
    root: LayoutNode,
    pending_focus: Option<PendingTopologyFocus>,
    pane_tasks: HashMap<SplintId, PaneTask>,
}

async fn cancel_pane_tasks(pane_tasks: HashMap<SplintId, PaneTask>) {
    let tasks = pane_tasks.into_values().collect::<Vec<_>>();
    for task in &tasks {
        task.cancellation.cancel();
    }
    for task in tasks {
        let _ = task.task.await;
    }
}

struct PreparedManagedDojo {
    identity: WindowDojoIdentity,
    dojo: splinterm_core::Dojo,
    panes: Vec<WindowPaneOptions>,
    pane_tasks: HashMap<SplintId, PaneTask>,
}

async fn prepare_managed_dojo(
    factory: &ConnectionFactory,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    identity: WindowDojoIdentity,
    dojo: splinterm_core::Dojo,
) -> Result<PreparedManagedDojo> {
    anyhow::ensure!(
        dojo.root.find_splint(dojo.default_focus).is_some(),
        "target Dojo focus is absent from its layout"
    );
    let mut ids = Vec::new();
    layout_splint_ids(&dojo.root, &mut ids);
    let mut panes = Vec::with_capacity(ids.len());
    let mut pane_tasks = HashMap::with_capacity(ids.len());
    for splint_id in ids {
        match prepare_live_pane(factory, config, splint_id, image_cache.clone(), false).await {
            Ok(pane) => {
                panes.push(pane.options);
                pane_tasks.insert(splint_id, pane.task);
            }
            Err(error) => {
                cancel_pane_tasks(pane_tasks).await;
                return Err(error);
            }
        }
    }
    Ok(PreparedManagedDojo {
        identity,
        dojo,
        panes,
        pane_tasks,
    })
}

enum TopologyManagerCommandOutcome {
    Continue,
    Stop,
    Edit(WindowTopologyCommand),
}

struct TopologyManagerState {
    tabs: WindowTabSet<ManagedDojo>,
}

const fn window_has_tab_capacity(tab_count: usize) -> bool {
    tab_count < splinterm::tab::MAX_WINDOW_TABS
}

async fn finish_managed_window_open(
    factory: &ConnectionFactory,
    target: Result<(WindowDojoIdentity, splinterm_core::Dojo)>,
    state: &mut TopologyManagerState,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
) -> TopologyManagerCommandOutcome {
    let target_id = target.as_ref().ok().map(|(_, dojo)| dojo.id);
    let result = async {
        let (identity, dojo) = target?;
        if state.tabs.activate(dojo.id) {
            remember_dojo(factory, dojo.id);
            updates
                .send(WindowTopologyUpdate::ActivateTab { dojo_id: dojo.id })
                .await
                .map_err(|_| anyhow::anyhow!("Wayland tab update channel closed"))?;
            return Ok(OpenTabOutcome::ActivatedExisting);
        }
        anyhow::ensure!(
            state.tabs.len() < splinterm::tab::MAX_WINDOW_TABS,
            "a Window may contain at most {} Dojo tabs",
            splinterm::tab::MAX_WINDOW_TABS
        );
        let prepared = prepare_managed_dojo(factory, config, image_cache, identity, dojo).await?;
        let dojo_id = prepared.dojo.id;
        let lair_id = prepared.identity.lair_id;
        let (acknowledged, acknowledgement) = tokio::sync::oneshot::channel();
        if updates
            .send(WindowTopologyUpdate::OpenTab {
                identity: prepared.identity.clone(),
                layout: prepared.dojo.root.clone(),
                panes: prepared.panes,
                focused: prepared.dojo.default_focus,
                acknowledged,
            })
            .await
            .is_err()
        {
            cancel_pane_tasks(prepared.pane_tasks).await;
            bail!("Wayland tab update channel closed");
        }
        match acknowledgement.await {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                cancel_pane_tasks(prepared.pane_tasks).await;
                bail!("Wayland rejected Dojo tab: {message}");
            }
            Err(_) => {
                cancel_pane_tasks(prepared.pane_tasks).await;
                bail!("Wayland dropped Dojo tab acknowledgement");
            }
        }
        state.tabs.open_or_activate(DojoTab::new(
            lair_id,
            dojo_id,
            ManagedDojo {
                identity: prepared.identity,
                root: prepared.dojo.root,
                pending_focus: None,
                pane_tasks: prepared.pane_tasks,
            },
        ))?;
        remember_dojo(factory, dojo_id);
        Ok(OpenTabOutcome::Opened)
    }
    .await;
    match result {
        Ok(_) => TopologyManagerCommandOutcome::Continue,
        Err(error) => {
            let _ = updates
                .send(WindowTopologyUpdate::TabFailed {
                    dojo_id: target_id,
                    message: format!("{error:#}"),
                })
                .await;
            TopologyManagerCommandOutcome::Continue
        }
    }
}

async fn remove_frontend_tab(
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    dojo_id: DojoId,
) -> bool {
    let (acknowledged, acknowledgement) = tokio::sync::oneshot::channel();
    if updates
        .send(WindowTopologyUpdate::RemoveTab {
            dojo_id,
            acknowledged,
        })
        .await
        .is_err()
    {
        return false;
    }
    acknowledgement.await.is_ok()
}

fn close_other_tab_targets(
    retain_dojo_id: DojoId,
    retain_present: bool,
    dojo_ids: Vec<DojoId>,
) -> Option<Vec<DojoId>> {
    if !retain_present || dojo_ids.len() > splinterm::tab::MAX_WINDOW_TABS {
        return None;
    }
    let mut unique = HashSet::new();
    Some(
        dojo_ids
            .into_iter()
            .filter(|dojo_id| *dojo_id != retain_dojo_id && unique.insert(*dojo_id))
            .collect(),
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "session-level topology commands share one serialized daemon and frontend reconciliation boundary"
)]
async fn handle_session_manager_command(
    factory: &ConnectionFactory,
    command: WindowTopologyCommand,
    connection: &mut Connection,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    state: &mut TopologyManagerState,
) -> TopologyManagerCommandOutcome {
    match command {
        WindowTopologyCommand::RequestSessionPicker => {
            match session_picker_catalog(factory, connection).await {
                Ok((items, targets)) => {
                    if updates
                        .send(WindowTopologyUpdate::ShowSessionPicker { items, targets })
                        .await
                        .is_err()
                    {
                        return TopologyManagerCommandOutcome::Stop;
                    }
                }
                Err(error) => {
                    let _ = updates
                        .send(WindowTopologyUpdate::SessionPickerFailed(format!(
                            "{error:#}"
                        )))
                        .await;
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::OpenDojo {
            lair_id,
            dojo_id: target_id,
        } => {
            let target = reopenable_dojo(connection, lair_id, target_id).await;
            finish_managed_window_open(factory, target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::NewLair => {
            if !window_has_tab_capacity(state.tabs.len()) {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!(
                            "a Window may contain at most {} Dojo tabs",
                            splinterm::tab::MAX_WINDOW_TABS
                        ),
                    })
                    .await;
                return TopologyManagerCommandOutcome::Continue;
            }
            let target = create_daily_dojo(factory, connection, config).await;
            finish_managed_window_open(factory, target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::NewDojo { lair_id } => {
            if !window_has_tab_capacity(state.tabs.len()) {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!(
                            "a Window may contain at most {} Dojo tabs",
                            splinterm::tab::MAX_WINDOW_TABS
                        ),
                    })
                    .await;
                return TopologyManagerCommandOutcome::Continue;
            }
            let target = create_dojo_in_lair(factory, connection, config, lair_id).await;
            finish_managed_window_open(factory, target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::RenameDojo { dojo_id, name } => {
            let result = async {
                anyhow::ensure!(
                    state.tabs.get(dojo_id).is_some(),
                    "rename targeted a closed Dojo tab"
                );
                let expected_topology_revision = connection.topology_revision().await?;
                let response = connection
                    .request(Request::RenameDojo {
                        expected_topology_revision,
                        dojo_id,
                        name,
                    })
                    .await?;
                anyhow::ensure!(
                    matches!(response, Response::TopologyCommitted { .. }),
                    "splinterd did not acknowledge Dojo rename"
                );
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: Some(dojo_id),
                        message: format!("{error:#}"),
                    })
                    .await;
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::TerminateDojo { dojo_id, splints } => {
            let result = async {
                anyhow::ensure!(
                    state.tabs.get(dojo_id).is_some(),
                    "termination targeted a closed Dojo tab"
                );
                terminate_dojo(connection, dojo_id, &splints).await
            }
            .await;
            match result {
                Ok(()) => {
                    if let Some(removed) = state.tabs.close(dojo_id) {
                        let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                        cancel_pane_tasks(removed.value.pane_tasks).await;
                        if !acknowledged {
                            return TopologyManagerCommandOutcome::Stop;
                        }
                        if state.tabs.is_empty() {
                            let _ = updates.send(WindowTopologyUpdate::Closed).await;
                            return TopologyManagerCommandOutcome::Stop;
                        }
                    }
                }
                Err(error) => {
                    let _ = updates
                        .send(WindowTopologyUpdate::TabFailed {
                            dojo_id: Some(dojo_id),
                            message: format!("{error:#}"),
                        })
                        .await;
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::ActivateTab { dojo_id } => {
            if state.tabs.activate(dojo_id)
                && updates
                    .send(WindowTopologyUpdate::ActivateTab { dojo_id })
                    .await
                    .is_err()
            {
                return TopologyManagerCommandOutcome::Stop;
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::CloseTab { dojo_id } => {
            if let Some(removed) = state.tabs.close(dojo_id) {
                let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                cancel_pane_tasks(removed.value.pane_tasks).await;
                if !acknowledged {
                    return TopologyManagerCommandOutcome::Stop;
                }
                if state.tabs.is_empty() {
                    let _ = updates.send(WindowTopologyUpdate::Closed).await;
                    return TopologyManagerCommandOutcome::Stop;
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::CloseTabs {
            retain_dojo_id,
            dojo_ids,
        } => {
            let Some(dojo_ids) = close_other_tab_targets(
                retain_dojo_id,
                state.tabs.get(retain_dojo_id).is_some(),
                dojo_ids,
            ) else {
                return TopologyManagerCommandOutcome::Continue;
            };
            for dojo_id in dojo_ids {
                if let Some(removed) = state.tabs.close(dojo_id) {
                    let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                    cancel_pane_tasks(removed.value.pane_tasks).await;
                    if !acknowledged {
                        return TopologyManagerCommandOutcome::Stop;
                    }
                    if state.tabs.is_empty() {
                        let _ = updates.send(WindowTopologyUpdate::Closed).await;
                        return TopologyManagerCommandOutcome::Stop;
                    }
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        command => TopologyManagerCommandOutcome::Edit(command),
    }
}

async fn inspect_managed_topology(
    connection: &mut Connection,
) -> Result<splinterm_protocol::TopologySnapshot> {
    let Response::Topology { snapshot } = connection.request(Request::InspectTopology).await?
    else {
        bail!("splinterd did not return topology for Window reconciliation");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    Ok(snapshot)
}

async fn reconcile_managed_topology(
    factory: &ConnectionFactory,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    state: &mut TopologyManagerState,
    snapshot: &splinterm_protocol::TopologySnapshot,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
) -> Result<bool> {
    let authoritative = snapshot
        .topology
        .lairs()
        .flat_map(|lair| {
            lair.dojos.iter().map(move |dojo| {
                (
                    dojo.id,
                    (window_dojo_identity(lair, dojo), dojo.root.clone()),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let dojo_ids = state.tabs.iter().map(|tab| tab.dojo_id).collect::<Vec<_>>();
    for dojo_id in dojo_ids {
        let Some((identity, root)) = authoritative.get(&dojo_id).cloned() else {
            if let Some(removed) = state.tabs.close(dojo_id) {
                let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                cancel_pane_tasks(removed.value.pane_tasks).await;
                if !acknowledged {
                    return Ok(false);
                }
            }
            continue;
        };
        let managed = &mut state
            .tabs
            .get_mut(dojo_id)
            .context("managed Dojo disappeared during reconciliation")?
            .value;
        if managed.identity != identity {
            if updates
                .send(WindowTopologyUpdate::UpdateIdentity(identity.clone()))
                .await
                .is_err()
            {
                return Ok(false);
            }
            managed.identity = identity;
        }
        let (added, _) = topology_identity_diff(&managed.root, &root);
        let (focused, consumed) =
            pending_focus_for_observation(managed.pending_focus, snapshot.revision, &added);
        match reconcile_window_topology(
            factory,
            config,
            image_cache,
            dojo_id,
            &mut managed.root,
            root,
            focused,
            updates,
            &mut managed.pane_tasks,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: Some(dojo_id),
                        message: format!("{error:#}"),
                    })
                    .await;
            }
        }
        if consumed {
            managed.pending_focus = None;
        }
    }
    if state.tabs.is_empty() {
        let _ = updates.send(WindowTopologyUpdate::Closed).await;
        return Ok(false);
    }
    Ok(true)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "endpoint-bound poll reconciliation, command targeting, and task shutdown share one loop"
)]
pub(in crate::app) async fn run_topology_manager(
    factory: ConnectionFactory,
    config: AppConfig,
    image_cache: SharedImageContentCache,
    initial_identity: WindowDojoIdentity,
    root: LayoutNode,
    mut commands: mpsc::Receiver<WindowTopologyCommand>,
    updates: mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: HashMap<SplintId, PaneTask>,
) -> Result<()> {
    let mut connection = factory.connect().await?;
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let initial_lair_id = initial_identity.lair_id;
    let initial_dojo_id = initial_identity.dojo_id;
    let mut state = TopologyManagerState {
        tabs: WindowTabSet::new(DojoTab::new(
            initial_lair_id,
            initial_dojo_id,
            ManagedDojo {
                identity: initial_identity,
                root,
                pending_focus: None,
                pane_tasks,
            },
        )),
    };
    loop {
        let command = tokio::select! {
            command = commands.recv() => command,
            _ = poll.tick() => None,
        };
        let command = if let Some(command) = command {
            match handle_session_manager_command(
                &factory,
                command,
                &mut connection,
                &config,
                &image_cache,
                &updates,
                &mut state,
            )
            .await
            {
                TopologyManagerCommandOutcome::Continue => continue,
                TopologyManagerCommandOutcome::Stop => break,
                TopologyManagerCommandOutcome::Edit(command) => Some(command),
            }
        } else {
            None
        };
        let snapshot = match inspect_managed_topology(&mut connection).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = updates
                    .send(WindowTopologyUpdate::Shutdown(format!("{error:#}")))
                    .await;
                return Err(error);
            }
        };
        if !reconcile_managed_topology(
            &factory,
            &config,
            &image_cache,
            &mut state,
            &snapshot,
            &updates,
        )
        .await?
        {
            break;
        }
        let Some(command) = command else {
            if commands.is_closed() {
                break;
            }
            continue;
        };
        let dojo_id = match &command {
            WindowTopologyCommand::Split { dojo_id, .. }
            | WindowTopologyCommand::Close { dojo_id, .. }
            | WindowTopologyCommand::AdjustRatio { dojo_id, .. }
            | WindowTopologyCommand::SetRatio { dojo_id, .. } => *dojo_id,
            _ => unreachable!("session command escaped manager dispatch"),
        };
        let managed = &mut state
            .tabs
            .get_mut(dojo_id)
            .context("topology command targeted a closed Dojo tab")?
            .value;
        match apply_topology_command(
            &factory,
            &mut connection,
            &config,
            dojo_id,
            &managed.root,
            snapshot.revision,
            command,
        )
        .await
        {
            Ok(TopologyCommandOutcome::Updated { pending_focus }) => {
                managed.pending_focus = pending_focus;
            }
            Ok(TopologyCommandOutcome::WindowClosed) => {
                let removed = state.tabs.close(dojo_id).expect("edited tab remains");
                let acknowledged = remove_frontend_tab(&updates, dojo_id).await;
                cancel_pane_tasks(removed.value.pane_tasks).await;
                if !acknowledged || state.tabs.is_empty() {
                    break;
                }
            }
            Err(error) => eprintln!("splinterm topology edit rejected: {error:#}"),
        }
    }
    let mut remaining_tasks = Vec::new();
    for tab in state.tabs.iter_mut() {
        remaining_tasks.extend(tab.value.pane_tasks.drain().map(|(_, task)| task));
    }
    for task in &remaining_tasks {
        task.cancellation.cancel();
    }
    for task in remaining_tasks {
        let _ = task.task.await;
    }
    Ok(())
}

pub(in crate::app) fn spawn_topology_smoke(
    commands: mpsc::Sender<WindowTopologyCommand>,
    dojo_id: DojoId,
    target: SplintId,
) -> Result<Option<tokio::task::JoinHandle<Result<()>>>> {
    if env::var_os("SPLINTERM_TOPOLOGY_SMOKE").is_none() {
        return Ok(None);
    }
    anyhow::ensure!(
        env::var_os("SPLINTERM_ENABLE_DEV_ATTACH").is_some(),
        "SPLINTERM_TOPOLOGY_SMOKE requires development attach"
    );
    Ok(Some(tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        commands
            .send(WindowTopologyCommand::Split {
                dojo_id,
                target,
                axis: Axis::Horizontal,
            })
            .await
            .map_err(|_| anyhow::anyhow!("topology smoke split channel closed"))?;
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        commands
            .send(WindowTopologyCommand::AdjustRatio {
                dojo_id,
                target,
                delta: 100,
            })
            .await
            .map_err(|_| anyhow::anyhow!("topology smoke ratio channel closed"))
    })))
}

pub(in crate::app) async fn initial_window_dojo_identity(
    factory: &ConnectionFactory,
    dojo_id: DojoId,
) -> Result<WindowDojoIdentity> {
    let mut connection = factory.connect().await?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its Lairs for Window identity");
    };
    for lair in &lairs {
        if let Some(dojo) = lair.dojos.iter().find(|dojo| dojo.id == dojo_id) {
            return Ok(window_dojo_identity(lair, dojo));
        }
    }
    bail!("initial Dojo is absent from daemon topology")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Axis, CloseAction, DojoId, LayoutNode, PendingTopologyFocus, RefreshedCloseState, Response,
        SplintId, SplintState, SplitRatio, TopologyCommandOutcome, TopologyRevision,
        captured_dojo_kill_targets, close_action, close_other_tab_targets, parent_ratio,
        pending_focus_for_observation, refreshed_close_state, topology_command_outcome,
        topology_identity_diff, validate_exited_close_target, window_has_tab_capacity,
    };
    use crate::app::pane_bridge::pane_claims_initial_control;

    #[test]
    fn termination_targets_exact_captured_incarnations_without_expanding_scope() {
        let mut live = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let live_id = live.id;
        live.state = SplintState::Running;
        live.last_incarnation = Some(7);
        let mut exited = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let exited_id = exited.id;
        exited.state = SplintState::Exited(0);
        exited.last_incarnation = Some(8);
        let root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(LayoutNode::Leaf(live)),
            second: Box::new(LayoutNode::Leaf(exited)),
        };
        assert_eq!(
            captured_dojo_kill_targets(&root, &[(live_id, 7), (exited_id, 8)]).unwrap(),
            vec![(live_id, 7)]
        );
        assert!(captured_dojo_kill_targets(&root, &[(live_id, 7)]).is_err());
        assert!(captured_dojo_kill_targets(&root, &[(live_id, 9), (exited_id, 8)]).is_err());
        assert!(captured_dojo_kill_targets(&root, &[(live_id, 7), (live_id, 7)]).is_err());
    }

    #[test]
    fn close_action_kills_live_panes_and_removes_exited_panes() {
        let mut live = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let live_id = live.id;
        live.state = SplintState::Running;
        live.last_incarnation = Some(7);
        assert_eq!(
            close_action(&LayoutNode::Leaf(live), live_id).unwrap(),
            CloseAction::KillAndClose { incarnation: 7 }
        );

        let mut exited = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let exited_id = exited.id;
        exited.state = SplintState::Exited(0);
        exited.last_incarnation = Some(7);
        let exited_root = LayoutNode::Leaf(exited.clone());
        assert_eq!(
            close_action(&exited_root, exited_id).unwrap(),
            CloseAction::CloseExited
        );
        assert!(validate_exited_close_target(&exited_root, exited_id, Some(7)).unwrap());
        assert!(validate_exited_close_target(&exited_root, exited_id, Some(8)).is_err());

        let sibling = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let split_root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(LayoutNode::Leaf(exited)),
            second: Box::new(LayoutNode::Leaf(sibling)),
        };
        assert!(!validate_exited_close_target(&split_root, exited_id, Some(7)).unwrap());
        assert_eq!(
            refreshed_close_state(Some(&split_root), exited_id, Some(7)).unwrap(),
            RefreshedCloseState::Retry
        );
        assert_eq!(
            refreshed_close_state(None, exited_id, Some(7)).unwrap(),
            RefreshedCloseState::WindowClosed
        );
        let unrelated = LayoutNode::Leaf(splinterm_core::Splint::shell(PathBuf::from("/tmp")));
        assert_eq!(
            refreshed_close_state(Some(&unrelated), exited_id, Some(7)).unwrap(),
            RefreshedCloseState::TargetClosed
        );

        let missing_incarnation = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let missing_id = missing_incarnation.id;
        assert!(close_action(&LayoutNode::Leaf(missing_incarnation), missing_id).is_err());
    }

    #[test]
    fn tab_creation_capacity_is_rejected_before_daemon_creation() {
        assert!(window_has_tab_capacity(0));
        assert!(window_has_tab_capacity(splinterm::tab::MAX_WINDOW_TABS - 1));
        assert!(!window_has_tab_capacity(splinterm::tab::MAX_WINDOW_TABS));
    }

    #[test]
    fn close_other_tabs_is_bounded_deduplicated_and_retains_exact_target() {
        let retained = DojoId::new();
        let first = DojoId::new();
        let second = DojoId::new();
        assert_eq!(
            close_other_tab_targets(
                retained,
                true,
                vec![first, retained, second, first, retained]
            ),
            Some(vec![first, second])
        );
        assert_eq!(close_other_tab_targets(retained, false, vec![first]), None);
        assert_eq!(
            close_other_tab_targets(
                retained,
                true,
                vec![first; splinterm::tab::MAX_WINDOW_TABS + 1]
            ),
            None
        );
    }

    #[test]
    fn topology_diff_and_parent_ratio_are_identity_local() {
        let first = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let first_id = first.id;
        let second = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let second_id = second.id;
        let third = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let third_id = third.id;
        let initial = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(400).unwrap(),
            first: Box::new(LayoutNode::Leaf(first.clone())),
            second: Box::new(LayoutNode::Leaf(second.clone())),
        };
        let nested = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(400).unwrap(),
            first: Box::new(LayoutNode::Leaf(first)),
            second: Box::new(LayoutNode::Branch {
                axis: Axis::Vertical,
                ratio: SplitRatio::new(650).unwrap(),
                first: Box::new(LayoutNode::Leaf(second)),
                second: Box::new(LayoutNode::Leaf(third)),
            }),
        };
        let (added, removed) = topology_identity_diff(&initial, &nested);
        assert_eq!(added, vec![third_id]);
        assert!(removed.is_empty());
        assert_eq!(parent_ratio(&nested, first_id).unwrap().get(), 400);
        assert_eq!(parent_ratio(&nested, second_id).unwrap().get(), 650);
        assert!(pane_claims_initial_control(second_id, second_id));
        assert!(!pane_claims_initial_control(first_id, second_id));
        assert_eq!(parent_ratio(&nested, third_id).unwrap().get(), 650);
    }

    #[test]
    fn successful_split_focuses_the_new_local_splint() {
        let splint_id = SplintId::new();
        assert_eq!(
            topology_command_outcome(Response::SplintStarted {
                splint_id,
                incarnation: 1,
                topology_revision: TopologyRevision::new(2),
            })
            .unwrap(),
            TopologyCommandOutcome::Updated {
                pending_focus: Some(PendingTopologyFocus {
                    splint_id,
                    revision: TopologyRevision::new(2),
                })
            }
        );
        assert_eq!(
            topology_command_outcome(Response::TopologyCommitted {
                topology_revision: TopologyRevision::new(3),
            })
            .unwrap(),
            TopologyCommandOutcome::Updated {
                pending_focus: None
            }
        );
    }

    #[test]
    fn pending_split_focus_is_revision_bound_and_requires_the_added_splint() {
        let splint_id = SplintId::new();
        let unrelated = SplintId::new();
        let pending = Some(PendingTopologyFocus {
            splint_id,
            revision: TopologyRevision::new(4),
        });

        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(3), &[splint_id]),
            (None, false)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(4), &[splint_id]),
            (Some(splint_id), true)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(4), &[unrelated]),
            (None, true)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(5), &[]),
            (None, true)
        );
    }
}
