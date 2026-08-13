use std::{
    collections::{HashMap, HashSet},
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use splinterm::{
    LairDirection, LairPromptKind, LairPromptTarget, SelectorKind, SessionPickerItem,
    WindowDojoIdentity, WindowPaneOptions, WindowTopologyCommand, WindowTopologyUpdate,
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
use splinterm_protocol::{
    ErrorCode, MutationTarget, Request, Response, validate_preset_materialized,
};
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
    placeholder: Option<SplintId>,
}

fn topology_edit_label(command: &WindowTopologyCommand) -> &'static str {
    match command {
        WindowTopologyCommand::Split { .. } => "split",
        WindowTopologyCommand::Close { .. } => "close",
        WindowTopologyCommand::AdjustRatio { .. } | WindowTopologyCommand::SetRatio { .. } => {
            "resize"
        }
        _ => "session",
    }
}

fn command_has_pending_split(command: Option<&WindowTopologyCommand>) -> bool {
    matches!(
        command,
        Some(WindowTopologyCommand::Split {
            pending: Some(_),
            ..
        })
    )
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
    let pending_placeholder = match &command {
        WindowTopologyCommand::Split { pending, .. } => *pending,
        _ => None,
    };
    let request = match command {
        WindowTopologyCommand::Split {
            dojo_id: target_dojo,
            target,
            axis,
            pending: _,
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
                LaunchSemantics::RemoteInteractive => Request::SplitSplintAutomation {
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
        | WindowTopologyCommand::RequestSelector { .. }
        | WindowTopologyCommand::OpenDojo { .. }
        | WindowTopologyCommand::NewLair { .. }
        | WindowTopologyCommand::NewDojo { .. }
        | WindowTopologyCommand::MaterializePreset { .. }
        | WindowTopologyCommand::NavigateLair { .. }
        | WindowTopologyCommand::RequestLairPrompt { .. }
        | WindowTopologyCommand::RequestDojoRestorePrompt { .. }
        | WindowTopologyCommand::RenameLair { .. }
        | WindowTopologyCommand::TerminateLair { .. }
        | WindowTopologyCommand::SetLairRetention { .. }
        | WindowTopologyCommand::RestoreLair { .. }
        | WindowTopologyCommand::RestoreDojo { .. }
        | WindowTopologyCommand::RenameDojo { .. }
        | WindowTopologyCommand::TerminateDojo { .. }
        | WindowTopologyCommand::ActivateTab { .. }
        | WindowTopologyCommand::CloseTab { .. }
        | WindowTopologyCommand::CloseTabs { .. } => {
            unreachable!("session commands are handled by the topology manager")
        }
    };
    topology_command_outcome(connection.request(request).await?, pending_placeholder)
}

fn topology_command_outcome(
    response: Response,
    pending_placeholder: Option<SplintId>,
) -> Result<TopologyCommandOutcome> {
    match response {
        Response::SplintStarted {
            splint_id,
            topology_revision,
            ..
        } => Ok(TopologyCommandOutcome::Updated {
            pending_focus: Some(PendingTopologyFocus {
                splint_id,
                revision: topology_revision,
                placeholder: pending_placeholder,
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
) -> (Option<SplintId>, Option<SplintId>, bool) {
    let Some(pending) = pending else {
        return (None, None, false);
    };
    if observed_revision < pending.revision {
        return (None, None, false);
    }
    (
        added
            .contains(&pending.splint_id)
            .then_some(pending.splint_id),
        pending.placeholder,
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
    topology_revision: TopologyRevision,
    dojo_id: DojoId,
    root: &mut LayoutNode,
    next: LayoutNode,
    focused: Option<SplintId>,
    placeholder: Option<SplintId>,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: &mut HashMap<SplintId, PaneTask>,
) -> Result<bool> {
    if *root == next {
        let Some(placeholder) = placeholder else {
            return Ok(true);
        };
        return Ok(updates
            .send(WindowTopologyUpdate::Apply {
                topology_revision,
                dojo_id,
                layout: next,
                added: Vec::new(),
                removed: vec![placeholder],
                focused,
            })
            .await
            .is_ok());
    }
    let (added_ids, mut removed) = topology_identity_diff(root, &next);
    if let Some(placeholder) = placeholder {
        removed.push(placeholder);
    }
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
            topology_revision,
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
    retire_pane_tasks(removed_tasks);
    pane_tasks.extend(new_tasks);
    *root = next;
    Ok(true)
}

async fn selector_catalog(
    factory: &ConnectionFactory,
    connection: &mut Connection,
    lair_filter: Option<LairId>,
) -> Result<(Vec<SessionPickerItem>, Vec<(LairId, DojoId)>)> {
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let entries = collect_sessions(&lairs, &recent_dojo_ids(factory))
        .into_iter()
        .filter(SessionEntry::reopenable)
        .filter(|entry| lair_filter.is_none_or(|lair_id| entry.lair_id == lair_id))
        .collect::<Vec<_>>();
    let items = entries.iter().map(session_picker_item).collect();
    let targets = entries
        .iter()
        .map(|entry| (entry.lair_id, entry.dojo_id))
        .collect();
    Ok((items, targets))
}

fn window_dojo_identity(
    topology_revision: TopologyRevision,
    lair: &splinterm_core::Lair,
    dojo: &splinterm_core::Dojo,
) -> WindowDojoIdentity {
    WindowDojoIdentity {
        topology_revision,
        lair_id: lair.id,
        dojo_id: dojo.id,
        lair_name: lair.name.clone(),
        lair_retention: lair.retention,
        dojo_name: dojo.name.clone(),
    }
}

fn materialized_dojo_targets(
    lairs: &[splinterm_core::Lair],
    topology_revision: TopologyRevision,
    lair_id: LairId,
    dojo_ids: &[DojoId],
    panes: &[splinterm_protocol::PresetPaneIdentity],
) -> Result<Vec<(WindowDojoIdentity, splinterm_core::Dojo)>> {
    validate_preset_materialized(dojo_ids, panes)
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let lair = lairs
        .iter()
        .find(|lair| lair.id == lair_id)
        .context("materialized Lair is absent from committed topology")?;
    let mut targets = Vec::with_capacity(dojo_ids.len());
    for dojo_id in dojo_ids {
        let dojo = lair
            .dojos
            .iter()
            .find(|dojo| dojo.id == *dojo_id)
            .context("materialized Dojo is absent from committed topology")?;
        let mapped = panes
            .iter()
            .filter(|pane| pane.dojo_id == *dojo_id)
            .collect::<Vec<_>>();
        anyhow::ensure!(
            mapped.len() == dojo.root.splint_count()
                && mapped
                    .iter()
                    .all(|pane| dojo.root.find_splint(pane.splint_id).is_some()),
            "preset pane mapping disagrees with committed Dojo"
        );
        targets.push((
            window_dojo_identity(topology_revision, lair, dojo),
            dojo.clone(),
        ));
    }
    Ok(targets)
}

async fn reopenable_dojo(
    connection: &mut Connection,
    lair_id: LairId,
    dojo_id: DojoId,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let Response::Lairs {
        lairs,
        topology_revision,
    } = connection.request(Request::ListLairs).await?
    else {
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
    Ok((window_dojo_identity(topology_revision, lair, &dojo), dojo))
}

async fn create_daily_dojo(
    factory: &ConnectionFactory,
    connection: &mut Connection,
    config: &AppConfig,
    cwd: std::path::PathBuf,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expected = connection.topology_revision().await?;
    let Response::LairCreated {
        lair,
        topology_revision,
        ..
    } = connection
        .request(create_request(
            factory,
            expected,
            format!("terminal-{stamp}-{}", std::process::id()),
            Some(cwd),
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
    Ok((window_dojo_identity(topology_revision, &lair, &dojo), dojo))
}

async fn create_dojo_in_lair(
    factory: &ConnectionFactory,
    connection: &mut Connection,
    config: &AppConfig,
    lair_id: LairId,
    cwd: std::path::PathBuf,
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
            launch: launch_parameters(cwd.clone(), Vec::new(), config),
        },
        LaunchSemantics::RemoteInteractive => Request::NewDojoAutomation {
            expected_topology_revision,
            lair_id,
            name: format!("terminal-{stamp}"),
            launch: automation_launch(Some(cwd), Vec::new()),
        },
    };
    let Response::DojoStarted { dojo_id, .. } = connection.request(request).await? else {
        bail!("splinterd did not create the requested Dojo");
    };
    reopenable_dojo(connection, lair_id, dojo_id).await
}

fn collect_lair_targets(lair: &splinterm_core::Lair) -> Result<Vec<MutationTarget>> {
    fn collect(
        lair_id: LairId,
        dojo_id: DojoId,
        node: &LayoutNode,
        targets: &mut Vec<MutationTarget>,
    ) -> Result<()> {
        match node {
            LayoutNode::Leaf(splint) => targets.push(MutationTarget {
                lair_id,
                dojo_id,
                splint_id: splint.id,
                incarnation: splint
                    .last_incarnation
                    .context("captured Lair pane has no process incarnation")?,
            }),
            LayoutNode::Branch { first, second, .. } => {
                collect(lair_id, dojo_id, first, targets)?;
                collect(lair_id, dojo_id, second, targets)?;
            }
        }
        Ok(())
    }

    let mut targets = Vec::new();
    for dojo in &lair.dojos {
        collect(lair.id, dojo.id, &dojo.root, &mut targets)?;
    }
    Ok(targets)
}

fn saved_layout_preview(lair: &splinterm_core::Lair) -> String {
    fn leaves(node: &LayoutNode, depth: usize, output: &mut Vec<String>) {
        match node {
            LayoutNode::Leaf(splint) => {
                let recipe = if let Some(executable) = splint.command.first() {
                    format!("Application: {executable}")
                } else if splint.launch.shell.is_some() || splint.command.is_empty() {
                    "Shell".to_owned()
                } else {
                    "No restorable recipe".to_owned()
                };
                output.push(format!(
                    "{}{} — {} — {}",
                    "  ".repeat(depth),
                    splint.title,
                    recipe,
                    splint.cwd.display()
                ));
            }
            LayoutNode::Branch {
                axis,
                ratio,
                first,
                second,
            } => {
                output.push(format!(
                    "{}{:?} split {}/1000",
                    "  ".repeat(depth),
                    axis,
                    ratio.get()
                ));
                leaves(first, depth + 1, output);
                leaves(second, depth + 1, output);
            }
        }
    }
    let mut lines = vec![format!(
        "{} — {:?} — {} Dojo(s)",
        lair.name,
        lair.retention,
        lair.dojos.len()
    )];
    for dojo in &lair.dojos {
        lines.push(format!(
            "{} ({} Splints)",
            dojo.name,
            dojo.root.splint_count()
        ));
        leaves(&dojo.root, 1, &mut lines);
    }
    if let Some(provenance) = &lair.provenance {
        lines.push(format!("Origin: {provenance}"));
    }
    lines.push("Not restored: terminal/scrollback bodies, process memory, shell state, environment, clipboard, images".to_owned());
    lines.join("\n")
}

async fn lair_prompt_target(
    connection: &mut Connection,
    lair_id: LairId,
) -> Result<LairPromptTarget> {
    let Response::Lairs {
        lairs,
        topology_revision,
    } = connection.request(Request::ListLairs).await?
    else {
        bail!("splinterd did not return its Lair catalog");
    };
    let lair = lairs
        .iter()
        .find(|lair| lair.id == lair_id)
        .context("captured Lair is absent")?;
    Ok(LairPromptTarget {
        topology_revision,
        lair_id,
        dojo_id: None,
        name: lair.name.clone(),
        retention: lair.retention,
        preview: saved_layout_preview(lair),
        targets: collect_lair_targets(lair)?,
    })
}

fn lair_navigation_target<T>(
    ordered: &[LairId],
    entries: &[SessionEntry],
    tabs: &WindowTabSet<T>,
    current_lair_id: LairId,
    direction: LairDirection,
) -> Result<(LairId, DojoId)> {
    anyhow::ensure!(!ordered.is_empty(), "no Lairs are available");
    let current = ordered
        .iter()
        .position(|lair_id| *lair_id == current_lair_id)
        .context("current Lair is absent from the captured catalog")?;
    for distance in 1..=ordered.len() {
        let index = match direction {
            LairDirection::Previous => {
                current
                    .saturating_add(ordered.len())
                    .saturating_sub(distance)
                    % ordered.len()
            }
            LairDirection::Next => current.saturating_add(distance) % ordered.len(),
        };
        let target_lair = ordered[index];
        let target_dojo = tabs.recent_in_lair(target_lair).or_else(|| {
            entries
                .iter()
                .find(|entry| entry.lair_id == target_lair)
                .map(|entry| entry.dojo_id)
        });
        if let Some(target_dojo) = target_dojo {
            return Ok((target_lair, target_dojo));
        }
    }
    bail!("captured Lair catalog has no reopenable Dojo")
}

async fn navigate_lair(
    factory: &ConnectionFactory,
    connection: &mut Connection,
    tabs: &WindowTabSet<ManagedDojo>,
    current_lair_id: LairId,
    direction: LairDirection,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its Lair catalog");
    };
    let recent = recent_dojo_ids(factory);
    let entries = collect_sessions(&lairs, &recent)
        .into_iter()
        .filter(SessionEntry::reopenable)
        .collect::<Vec<_>>();
    let ordered = lairs
        .iter()
        .filter(|lair| lair.lifetime.is_persistent())
        .map(|lair| lair.id)
        .collect::<Vec<_>>();
    let (target_lair, target_dojo) =
        lair_navigation_target(&ordered, &entries, tabs, current_lair_id, direction)?;
    reopenable_dojo(connection, target_lair, target_dojo).await
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

fn retire_pane_tasks(pane_tasks: HashMap<SplintId, PaneTask>) {
    if pane_tasks.is_empty() {
        return;
    }
    tokio::spawn(cancel_pane_tasks(pane_tasks));
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

fn topology_edit_target<T>(tabs: &mut WindowTabSet<T>, dojo_id: DojoId) -> Option<&mut T> {
    tabs.get_mut(dojo_id).map(|tab| &mut tab.value)
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
            match selector_catalog(factory, connection, None).await {
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
        WindowTopologyCommand::RequestSelector { kind, lair_id } => {
            let filter = (kind == SelectorKind::Dojo).then_some(lair_id);
            match selector_catalog(factory, connection, filter).await {
                Ok((items, targets)) => {
                    if updates
                        .send(WindowTopologyUpdate::ShowSelector {
                            kind,
                            items,
                            targets,
                        })
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
        WindowTopologyCommand::NewLair { cwd } => {
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
            let target = create_daily_dojo(factory, connection, config, cwd).await;
            finish_managed_window_open(factory, target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::MaterializePreset { target, dojos } => {
            if !factory.is_local() {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message:
                            "preset materialization is available only to the trusted local client"
                                .into(),
                    })
                    .await;
                return TopologyManagerCommandOutcome::Continue;
            }
            if dojos.is_empty()
                || state.tabs.len().saturating_add(dojos.len()) > splinterm::tab::MAX_WINDOW_TABS
            {
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
            let materialized = async {
                let expected = connection.topology_revision().await?;
                let Response::PresetMaterialized {
                    lair_id,
                    dojo_ids,
                    panes,
                    topology_revision,
                } = connection
                    .request(Request::MaterializePreset {
                        expected_topology_revision: expected,
                        target,
                        dojos,
                        directory_identities: Vec::new(),
                    })
                    .await?
                else {
                    bail!("splinterd did not return preset materialization metadata");
                };
                anyhow::ensure!(
                    dojo_ids.len() <= splinterm::tab::MAX_WINDOW_TABS,
                    "preset response exceeds Window tab capacity"
                );
                let Response::Lairs {
                    lairs,
                    topology_revision: inspected_revision,
                } = connection.request(Request::ListLairs).await?
                else {
                    bail!("splinterd did not return sessions after preset materialization");
                };
                anyhow::ensure!(
                    inspected_revision == topology_revision,
                    "preset topology revision drifted before Window reconciliation"
                );
                materialized_dojo_targets(&lairs, topology_revision, lair_id, &dojo_ids, &panes)
            }
            .await;
            let targets = match materialized {
                Ok(targets) => targets,
                Err(error) => {
                    let _ = updates
                        .send(WindowTopologyUpdate::TabFailed {
                            dojo_id: None,
                            message: format!("{error:#}"),
                        })
                        .await;
                    return TopologyManagerCommandOutcome::Continue;
                }
            };
            for target in targets {
                if matches!(
                    finish_managed_window_open(
                        factory,
                        Ok(target),
                        state,
                        config,
                        image_cache,
                        updates,
                    )
                    .await,
                    TopologyManagerCommandOutcome::Stop
                ) {
                    return TopologyManagerCommandOutcome::Stop;
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::NewDojo { lair_id, cwd } => {
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
            let target = create_dojo_in_lair(factory, connection, config, lair_id, cwd).await;
            finish_managed_window_open(factory, target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::NavigateLair {
            current_lair_id,
            direction,
        } => {
            let target =
                navigate_lair(factory, connection, &state.tabs, current_lair_id, direction).await;
            finish_managed_window_open(factory, target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::RequestLairPrompt { lair_id, kind } => {
            match lair_prompt_target(connection, lair_id).await {
                Ok(target) => {
                    if updates
                        .send(WindowTopologyUpdate::ShowLairPrompt { kind, target })
                        .await
                        .is_err()
                    {
                        return TopologyManagerCommandOutcome::Stop;
                    }
                }
                Err(error) => {
                    let _ = updates
                        .send(WindowTopologyUpdate::TabFailed {
                            dojo_id: None,
                            message: format!("{error:#}"),
                        })
                        .await;
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::RequestDojoRestorePrompt { dojo_id } => {
            let result = async {
                let Response::Lairs {
                    lairs,
                    topology_revision,
                } = connection.request(Request::ListLairs).await?
                else {
                    bail!("splinterd did not return its Lair catalog");
                };
                let (lair, dojo) = lairs
                    .iter()
                    .find_map(|lair| {
                        lair.dojos
                            .iter()
                            .find(|dojo| dojo.id == dojo_id)
                            .map(|dojo| (lair, dojo))
                    })
                    .context("captured Dojo is absent")?;
                let mut target = LairPromptTarget {
                    topology_revision,
                    lair_id: lair.id,
                    dojo_id: Some(dojo_id),
                    name: dojo.name.clone(),
                    retention: lair.retention,
                    preview: saved_layout_preview(lair),
                    targets: collect_lair_targets(lair)?
                        .into_iter()
                        .filter(|target| target.dojo_id == dojo_id)
                        .collect(),
                };
                target.preview = format!("Selected Dojo: {}\n{}", dojo.name, target.preview);
                updates
                    .send(WindowTopologyUpdate::ShowLairPrompt {
                        kind: LairPromptKind::Restore,
                        target,
                    })
                    .await
                    .map_err(|_| anyhow::anyhow!("Wayland tab update channel closed"))
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
        WindowTopologyCommand::RenameLair { lair_id, name } => {
            let result = async {
                anyhow::ensure!(
                    state.tabs.iter().any(|tab| tab.lair_id == lair_id),
                    "rename targeted a detached Lair"
                );
                let expected_topology_revision = connection.topology_revision().await?;
                let response = connection
                    .request(Request::RenameLair {
                        expected_topology_revision,
                        lair_id,
                        name,
                    })
                    .await?;
                anyhow::ensure!(
                    matches!(response, Response::TopologyCommitted { .. }),
                    "splinterd did not acknowledge Lair rename"
                );
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!("{error:#}"),
                    })
                    .await;
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::RestoreLair {
            expected_topology_revision,
            lair_id,
        } => {
            let result = async {
                anyhow::ensure!(
                    state.tabs.iter().any(|tab| tab.lair_id == lair_id),
                    "restore targeted a detached Lair"
                );
                let response = connection
                    .request(Request::RestoreLair {
                        expected_topology_revision,
                        lair_id,
                    })
                    .await?;
                anyhow::ensure!(
                    matches!(response, Response::RestoreCompleted { .. }),
                    "splinterd did not acknowledge Lair restore"
                );
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!("{error:#}"),
                    })
                    .await;
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::RestoreDojo {
            expected_topology_revision,
            dojo_id,
        } => {
            let result = connection
                .request(Request::RestoreDojo {
                    expected_topology_revision,
                    dojo_id,
                })
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
        WindowTopologyCommand::SetLairRetention { lair_id, retention } => {
            let result = async {
                anyhow::ensure!(
                    state.tabs.iter().any(|tab| tab.lair_id == lair_id),
                    "retention change targeted a detached Lair"
                );
                let expected_topology_revision = connection.topology_revision().await?;
                let response = connection
                    .request(Request::SetLairRetention {
                        expected_topology_revision,
                        lair_id,
                        retention,
                    })
                    .await?;
                let Response::TopologyCommitted {
                    topology_revision, ..
                } = response
                else {
                    bail!("splinterd did not acknowledge Lair retention change");
                };
                for tab in state.tabs.iter_mut().filter(|tab| tab.lair_id == lair_id) {
                    tab.value.identity.topology_revision = topology_revision;
                    tab.value.identity.lair_retention = retention;
                    updates
                        .send(WindowTopologyUpdate::UpdateIdentity(
                            tab.value.identity.clone(),
                        ))
                        .await
                        .map_err(|_| anyhow::anyhow!("Wayland tab update channel closed"))?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!("{error:#}"),
                    })
                    .await;
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::TerminateLair { lair_id, targets } => {
            let result = async {
                let expected_topology_revision = connection.topology_revision().await?;
                let response = connection
                    .request(Request::TerminateLair {
                        expected_topology_revision,
                        lair_id,
                        targets,
                    })
                    .await?;
                anyhow::ensure!(
                    matches!(response, Response::TopologyCommitted { .. }),
                    "splinterd did not acknowledge Lair termination"
                );
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!("{error:#}"),
                    })
                    .await;
                return TopologyManagerCommandOutcome::Continue;
            }
            let dojo_ids = state
                .tabs
                .iter()
                .filter_map(|tab| (tab.lair_id == lair_id).then_some(tab.dojo_id))
                .collect::<Vec<_>>();
            for dojo_id in dojo_ids {
                if let Some(removed) = state.tabs.close(dojo_id) {
                    let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                    retire_pane_tasks(removed.value.pane_tasks);
                    if !acknowledged {
                        return TopologyManagerCommandOutcome::Stop;
                    }
                }
            }
            if state.tabs.is_empty() {
                let _ = updates.send(WindowTopologyUpdate::Closed).await;
                return TopologyManagerCommandOutcome::Stop;
            }
            TopologyManagerCommandOutcome::Continue
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
                        retire_pane_tasks(removed.value.pane_tasks);
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
                retire_pane_tasks(removed.value.pane_tasks);
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
                    retire_pane_tasks(removed.value.pane_tasks);
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
                    (
                        window_dojo_identity(snapshot.revision, lair, dojo),
                        dojo.root.clone(),
                    ),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let dojo_ids = state.tabs.iter().map(|tab| tab.dojo_id).collect::<Vec<_>>();
    for dojo_id in dojo_ids {
        let Some((identity, root)) = authoritative.get(&dojo_id).cloned() else {
            if let Some(removed) = state.tabs.close(dojo_id) {
                let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                retire_pane_tasks(removed.value.pane_tasks);
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
        let (focused, placeholder, consumed) =
            pending_focus_for_observation(managed.pending_focus, snapshot.revision, &added);
        match reconcile_window_topology(
            factory,
            config,
            image_cache,
            snapshot.revision,
            dojo_id,
            &mut managed.root,
            root,
            focused,
            placeholder,
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

enum TopologyManagerWake {
    Command(WindowTopologyCommand),
    Poll,
    Shutdown,
}

async fn next_topology_manager_wake(
    commands: &mut mpsc::Receiver<WindowTopologyCommand>,
    poll: &mut tokio::time::Interval,
    poll_priority: &mut bool,
) -> TopologyManagerWake {
    if commands.is_closed() {
        return TopologyManagerWake::Shutdown;
    }
    let wake = if *poll_priority {
        tokio::select! {
            biased;
            _ = poll.tick() => TopologyManagerWake::Poll,
            command = commands.recv() => command.map_or(
                TopologyManagerWake::Shutdown,
                TopologyManagerWake::Command,
            ),
        }
    } else {
        tokio::select! {
            biased;
            command = commands.recv() => command.map_or(
                TopologyManagerWake::Shutdown,
                TopologyManagerWake::Command,
            ),
            _ = poll.tick() => TopologyManagerWake::Poll,
        }
    };
    if commands.is_closed() {
        return TopologyManagerWake::Shutdown;
    }
    *poll_priority = matches!(wake, TopologyManagerWake::Command(_));
    wake
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
    let mut poll_priority = false;
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
        let command =
            match next_topology_manager_wake(&mut commands, &mut poll, &mut poll_priority).await {
                TopologyManagerWake::Command(command) => Some(command),
                TopologyManagerWake::Poll => None,
                TopologyManagerWake::Shutdown => break,
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
        // The placeholder exists only in the frontend until this mutation is
        // acknowledged. An unrelated external topology change must therefore
        // reconcile together with the split, not against the placeholder alone.
        if !command_has_pending_split(command.as_ref())
            && !reconcile_managed_topology(
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
            continue;
        };
        let dojo_id = match &command {
            WindowTopologyCommand::Split { dojo_id, .. }
            | WindowTopologyCommand::Close { dojo_id, .. }
            | WindowTopologyCommand::AdjustRatio { dojo_id, .. }
            | WindowTopologyCommand::SetRatio { dojo_id, .. } => *dojo_id,
            _ => unreachable!("session command escaped manager dispatch"),
        };
        let operation = topology_edit_label(&command);
        let pending_split = match &command {
            WindowTopologyCommand::Split {
                target,
                pending: Some(pending),
                ..
            } => Some((*target, *pending)),
            _ => None,
        };
        let Some(managed) = topology_edit_target(&mut state.tabs, dojo_id) else {
            continue;
        };
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
                retire_pane_tasks(removed.value.pane_tasks);
                if !acknowledged || state.tabs.is_empty() {
                    break;
                }
            }
            Err(error) => {
                if let Some((target, pending)) = pending_split {
                    let _ = updates
                        .send(WindowTopologyUpdate::Apply {
                            topology_revision: snapshot.revision,
                            dojo_id,
                            layout: managed.root.clone(),
                            added: Vec::new(),
                            removed: vec![pending],
                            focused: Some(target),
                        })
                        .await;
                }
                let message = format!("{operation} failed: {error:#}");
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: Some(dojo_id),
                        message: message.clone(),
                    })
                    .await;
                let _ = message;
                eprintln!("splinterm topology edit rejected");
            }
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
                pending: None,
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
    let Response::Lairs {
        lairs,
        topology_revision,
    } = connection.request(Request::ListLairs).await?
    else {
        bail!("splinterd did not return its Lairs for Window identity");
    };
    for lair in &lairs {
        if let Some(dojo) = lair.dojos.iter().find(|dojo| dojo.id == dojo_id) {
            return Ok(window_dojo_identity(topology_revision, lair, dojo));
        }
    }
    bail!("initial Dojo is absent from daemon topology")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::{
        Axis, CloseAction, DojoId, DojoTab, LairDirection, LairId, LayoutNode,
        PendingTopologyFocus, RefreshedCloseState, Response, SessionEntry, SplintId, SplintState,
        SplitRatio, TopologyCommandOutcome, TopologyManagerWake, TopologyRevision, WindowTabSet,
        WindowTopologyCommand, cancel_pane_tasks, captured_dojo_kill_targets, close_action,
        close_other_tab_targets, command_has_pending_split, lair_navigation_target,
        materialized_dojo_targets, next_topology_manager_wake, parent_ratio,
        pending_focus_for_observation, refreshed_close_state, topology_command_outcome,
        topology_edit_target, topology_identity_diff, validate_exited_close_target,
        window_has_tab_capacity,
    };
    use crate::app::pane_bridge::{PaneTask, pane_claims_initial_control};

    #[tokio::test]
    async fn closed_window_command_channel_stops_before_another_topology_poll() {
        let (sender, mut commands) = mpsc::channel(1);
        drop(sender);
        let mut poll = tokio::time::interval(Duration::from_secs(60));
        let mut poll_priority = false;

        assert!(matches!(
            next_topology_manager_wake(&mut commands, &mut poll, &mut poll_priority).await,
            TopologyManagerWake::Shutdown
        ));
    }

    #[tokio::test]
    async fn ready_poll_follows_one_command_when_both_remain_ready() {
        let (sender, mut commands) = mpsc::channel(2);
        sender
            .send(WindowTopologyCommand::RequestSessionPicker)
            .await
            .unwrap();
        let mut poll = tokio::time::interval(Duration::from_millis(1));
        let mut poll_priority = false;

        assert!(matches!(
            next_topology_manager_wake(&mut commands, &mut poll, &mut poll_priority).await,
            TopologyManagerWake::Command(_)
        ));
        sender
            .send(WindowTopologyCommand::RequestSessionPicker)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(matches!(
            next_topology_manager_wake(&mut commands, &mut poll, &mut poll_priority).await,
            TopologyManagerWake::Poll
        ));
    }

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

    #[tokio::test]
    async fn closed_tab_tasks_are_cancelled_and_joined_before_cleanup_returns() {
        let completed = Arc::new(AtomicUsize::new(0));
        let mut tasks = HashMap::new();
        for _ in 0..3 {
            let cancellation = CancellationToken::new();
            let task_cancellation = cancellation.clone();
            let task_completed = Arc::clone(&completed);
            tasks.insert(
                SplintId::new(),
                PaneTask {
                    cancellation,
                    task: tokio::spawn(async move {
                        task_cancellation.cancelled().await;
                        task_completed.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }),
                },
            );
        }

        cancel_pane_tasks(tasks).await;

        assert_eq!(completed.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn uncooperative_retired_tab_task_does_not_block_topology_work() {
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            task_cancellation.cancelled().await;
            std::future::pending::<()>().await;
            Ok(())
        });
        let abort = task.abort_handle();
        let tasks = HashMap::from([(SplintId::new(), PaneTask { cancellation, task })]);

        super::retire_pane_tasks(tasks);
        tokio::task::yield_now().await;

        assert!(!abort.is_finished());
        abort.abort();
    }

    #[test]
    fn closed_dojo_topology_edits_are_stale_instead_of_fatal() {
        let dojo_id = DojoId::new();
        let mut tabs = WindowTabSet::new(DojoTab::new(LairId::new(), dojo_id, 7));
        assert_eq!(topology_edit_target(&mut tabs, dojo_id).copied(), Some(7));

        assert!(tabs.close(dojo_id).is_some());
        assert_eq!(topology_edit_target(&mut tabs, dojo_id), None);
    }

    #[test]
    fn tab_creation_capacity_is_rejected_before_daemon_creation() {
        assert!(window_has_tab_capacity(0));
        assert!(window_has_tab_capacity(splinterm::tab::MAX_WINDOW_TABS - 1));
        assert!(!window_has_tab_capacity(splinterm::tab::MAX_WINDOW_TABS));
    }

    #[test]
    fn preset_reconciliation_requires_every_stable_pane_before_opening() {
        let lair_id = LairId::new();
        let dojo_id = DojoId::new();
        let first = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let first_id = first.id;
        let second = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let second_id = second.id;
        let dojo = splinterm_core::Dojo {
            id: dojo_id,
            name: "preset".into(),
            default_focus: second_id,
            root: LayoutNode::Branch {
                axis: Axis::Horizontal,
                ratio: SplitRatio::new(500).unwrap(),
                first: Box::new(LayoutNode::Leaf(first)),
                second: Box::new(LayoutNode::Leaf(second)),
            },
        };
        let lair = splinterm_core::Lair {
            id: lair_id,
            name: "main".into(),
            lifetime: splinterm_core::LairLifetime::Persistent,
            retention: splinterm_core::LairRetention::default(),
            provenance: None,
            dojos: vec![dojo],
        };
        let pane = |key: &str, splint_id| splinterm_protocol::PresetPaneIdentity {
            dojo_id,
            key: key.into(),
            splint_id,
        };
        let targets = materialized_dojo_targets(
            std::slice::from_ref(&lair),
            TopologyRevision::new(8),
            lair_id,
            &[dojo_id],
            &[pane("first", first_id), pane("second", second_id)],
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0.topology_revision, TopologyRevision::new(8));
        assert_eq!(targets[0].1.default_focus, second_id);
        assert!(
            materialized_dojo_targets(
                &[lair],
                TopologyRevision::new(8),
                lair_id,
                &[dojo_id],
                &[pane("first", first_id)],
            )
            .is_err()
        );
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
    fn lair_navigation_prefers_most_recent_attached_dojo_by_stable_id() {
        let first_lair = LairId::new();
        let empty_lair = LairId::new();
        let second_lair = LairId::new();
        let first_dojo = DojoId::new();
        let second_dojo = DojoId::new();
        let recent_second_dojo = DojoId::new();
        let mut tabs = WindowTabSet::new(DojoTab::new(first_lair, first_dojo, ()));
        tabs.open_or_activate(DojoTab::new(second_lair, second_dojo, ()))
            .unwrap();
        tabs.open_or_activate(DojoTab::new(second_lair, recent_second_dojo, ()))
            .unwrap();
        assert!(tabs.activate(second_dojo));
        assert!(tabs.activate(first_dojo));
        let entry = |lair_id, dojo_id| SessionEntry {
            lair_id,
            dojo_id,
            lair_name: "lair".to_owned(),
            dojo_name: "dojo".to_owned(),
            cwd: PathBuf::from("/tmp"),
            pane_count: 1,
            running_panes: 1,
            exited_panes: 0,
        };
        let entries = vec![
            entry(first_lair, first_dojo),
            entry(second_lair, recent_second_dojo),
            entry(second_lair, second_dojo),
        ];
        assert_eq!(
            lair_navigation_target(
                &[first_lair, empty_lair, second_lair],
                &entries,
                &tabs,
                first_lair,
                LairDirection::Next,
            )
            .unwrap(),
            (second_lair, second_dojo)
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
    fn optimistic_split_defers_only_its_pre_mutation_reconciliation() {
        let dojo_id = DojoId::new();
        let target = SplintId::new();
        assert!(command_has_pending_split(Some(
            &WindowTopologyCommand::Split {
                dojo_id,
                target,
                axis: Axis::Horizontal,
                pending: Some(SplintId::new()),
            }
        )));
        assert!(!command_has_pending_split(Some(
            &WindowTopologyCommand::Split {
                dojo_id,
                target,
                axis: Axis::Horizontal,
                pending: None,
            }
        )));
        assert!(!command_has_pending_split(None));
    }

    #[test]
    fn successful_split_focuses_the_new_local_splint() {
        let splint_id = SplintId::new();
        let placeholder = SplintId::new();
        assert_eq!(
            topology_command_outcome(
                Response::SplintStarted {
                    splint_id,
                    incarnation: 1,
                    topology_revision: TopologyRevision::new(2),
                },
                Some(placeholder),
            )
            .unwrap(),
            TopologyCommandOutcome::Updated {
                pending_focus: Some(PendingTopologyFocus {
                    splint_id,
                    revision: TopologyRevision::new(2),
                    placeholder: Some(placeholder),
                })
            }
        );
        assert_eq!(
            topology_command_outcome(
                Response::TopologyCommitted {
                    topology_revision: TopologyRevision::new(3),
                },
                None,
            )
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
        let placeholder = SplintId::new();
        let pending = Some(PendingTopologyFocus {
            splint_id,
            revision: TopologyRevision::new(4),
            placeholder: Some(placeholder),
        });

        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(3), &[splint_id]),
            (None, None, false)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(4), &[splint_id]),
            (Some(splint_id), Some(placeholder), true)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(4), &[unrelated]),
            (None, Some(placeholder), true)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(5), &[]),
            (None, Some(placeholder), true)
        );
    }
}
