//! Close Tab and Window close against a scripted protocol peer, never a user daemon.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::*;
use splinterm_core::Lair;
use splinterm_protocol::{
    ClientFrame, DojoUse, PROTOCOL_VERSION, ProtocolError, ServerFrame, ServerLimits,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

type Reply = Result<Response, ProtocolError>;

/// Answers each request in turn, recording it with whether every pane had
/// finished detaching when it arrived.
async fn scripted_connection(
    replies: Vec<Reply>,
    panes_detached: Arc<AtomicBool>,
) -> (
    Connection,
    tokio::task::JoinHandle<Vec<(Request, bool)>>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (client, server) = tokio::io::duplex(65536);
    let (finished, finish) = tokio::sync::oneshot::channel::<()>();
    let peer = tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(server);
        let size = reader.read_u32().await.unwrap() as usize;
        let mut bytes = vec![0; size];
        reader.read_exact(&mut bytes).await.unwrap();
        assert!(matches!(
            serde_json::from_slice::<ClientFrame>(&bytes).unwrap(),
            ClientFrame::Hello { .. }
        ));
        writer
            .write_all(
                &splinterm_protocol::encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                    daemon_generation: 1,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let mut requests = Vec::new();
        for reply in replies {
            let size = reader.read_u32().await.unwrap() as usize;
            let mut bytes = vec![0; size];
            reader.read_exact(&mut bytes).await.unwrap();
            let ClientFrame::Request {
                request_id,
                request,
                ..
            } = serde_json::from_slice(&bytes).unwrap()
            else {
                panic!("expected request")
            };
            requests.push((request, panes_detached.load(Ordering::Acquire)));
            let frame = match reply {
                Ok(result) => ServerFrame::Response { request_id, result },
                Err(error) => ServerFrame::Error {
                    request_id: Some(request_id),
                    error,
                },
            };
            writer
                .write_all(&splinterm_protocol::encode_frame(&frame).unwrap())
                .await
                .unwrap();
        }
        let _ = finish.await;
        requests
    });
    let (reader, writer) = tokio::io::split(client);
    let connection = Connection::connect_remote_interactive_transport(reader, writer)
        .await
        .unwrap();
    (connection, peer, finished)
}

fn managed_tab(pane_tasks: HashMap<SplintId, PaneTask>) -> DojoTab<ManagedDojo> {
    let lair = Lair::new("tabs", "/tmp".into());
    let dojo = &lair.dojos[0];
    DojoTab::new(
        lair.id,
        dojo.id,
        ManagedDojo {
            identity: WindowDojoIdentity {
                topology_revision: TopologyRevision::new(1),
                lair_id: lair.id,
                dojo_id: dojo.id,
                lair_name: lair.name.clone(),
                lair_retention: lair.retention,
                dojo_name: dojo.name.clone(),
            },
            root: dojo.root.clone(),
            pending_focus: None,
            pane_tasks,
        },
    )
}

/// A pane that takes a moment to finish and records whether it was asked to
/// detach explicitly.
fn slow_pane(detached: &Arc<AtomicBool>) -> HashMap<SplintId, PaneTask> {
    let cancellation = CancellationToken::new();
    let detach_on_cancel = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn({
        let cancellation = cancellation.clone();
        let detach_on_cancel = Arc::clone(&detach_on_cancel);
        let detached = Arc::clone(detached);
        async move {
            cancellation.cancelled().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            detached.store(detach_on_cancel.load(Ordering::Acquire), Ordering::Release);
            Ok(())
        }
    });
    HashMap::from([(
        SplintId::new(),
        PaneTask {
            cancellation,
            task,
            detach_on_cancel,
        },
    )])
}

fn acknowledge_tab_removals(mut receiver: mpsc::Receiver<WindowTopologyUpdate>) {
    tokio::spawn(async move {
        while let Some(update) = receiver.recv().await {
            if let WindowTopologyUpdate::RemoveTab { acknowledged, .. } = update {
                let _ = acknowledged.send(());
            }
        }
    });
}

fn close_unused_config(enabled: bool) -> AppConfig {
    let mut config = AppConfig::default();
    config.multiplexer_lifetime.close_unused = enabled;
    config
}

fn closed(dojo_id: DojoId) -> Response {
    Response::UnusedDojoClose {
        dojo_id,
        outcome: UnusedDojoCloseOutcome::Closed {
            topology_revision: TopologyRevision::new(2),
            lair_removed: false,
        },
    }
}

/// Closes the second of two tabs; returns the requests the daemon saw.
async fn close_second_tab(config: AppConfig, replies: Vec<Reply>) -> (Vec<(Request, bool)>, bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let detached = Arc::new(AtomicBool::new(false));
        let (mut connection, peer, finished) =
            scripted_connection(replies, Arc::clone(&detached)).await;
        let target = managed_tab(slow_pane(&detached));
        let target_id = target.dojo_id;
        let mut state = TopologyManagerState {
            tabs: WindowTabSet::new(managed_tab(HashMap::new())),
            transient_owners: HashMap::new(),
            agent_status_republish: false,
        };
        state.tabs.open_or_activate(target).unwrap();
        let (updates, receiver) = mpsc::channel(16);
        acknowledge_tab_removals(receiver);
        let outcome = handle_session_manager_command(
            &ConnectionFactory::local(),
            WindowTopologyCommand::CloseTab { dojo_id: target_id },
            &mut connection,
            &config,
            &SharedImageContentCache::default(),
            &updates,
            &mut state,
        )
        .await;
        assert!(matches!(outcome, TopologyManagerCommandOutcome::Continue));
        assert!(state.tabs.get(target_id).is_none(), "the tab always closes");
        assert_eq!(state.tabs.len(), 1);
        let _ = finished.send(());
        let requests = peer.await.unwrap();
        (requests, detached.load(Ordering::Acquire))
    })
    .await
    .expect("bounded scripted tab close")
}

#[tokio::test]
async fn close_tab_detaches_without_asking_when_close_unused_is_off() {
    let (requests, detached) = close_second_tab(close_unused_config(false), Vec::new()).await;
    assert!(requests.is_empty());
    assert!(!detached, "a plain detach just drops the pane");
}

#[tokio::test]
async fn close_tab_detaches_its_pane_before_asking_to_close_an_unused_dojo() {
    let (requests, detached) =
        close_second_tab(close_unused_config(true), vec![Ok(closed(DojoId::new()))]).await;
    assert!(detached, "the pane was asked to detach explicitly");
    let [(Request::CloseUnusedDojo { .. }, panes_done)] = requests[..] else {
        panic!("expected one unused Dojo close, got {requests:?}");
    };
    assert!(panes_done, "the request waited for the pane to detach");
}

#[tokio::test]
async fn a_used_or_unsupported_answer_leaves_the_tab_detached() {
    for reply in [
        Ok(Response::UnusedDojoClose {
            dojo_id: DojoId::new(),
            outcome: UnusedDojoCloseOutcome::Used {
                reason: DojoUse::Input,
            },
        }),
        // A daemon that does not know the request.
        Err(ProtocolError::new(
            ErrorCode::InvalidFrame,
            "unknown request type close_unused_dojo",
        )),
        Err(ProtocolError::new(ErrorCode::Unauthorized, "denied")),
    ] {
        let (requests, _) = close_second_tab(close_unused_config(true), vec![reply]).await;
        assert!(matches!(
            requests[..],
            [(Request::CloseUnusedDojo { .. }, true)]
        ));
    }
}

/// Closes the second of two tabs, whose transient Lair this Window owns, and
/// reports whether the Window still holds that Lair's owner connection.
async fn close_transient_tab(lair_removed: bool) -> bool {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let detached = Arc::new(AtomicBool::new(false));
        let target = managed_tab(slow_pane(&detached));
        let (target_id, target_lair) = (target.dojo_id, target.lair_id);
        let (mut connection, peer, finished) = scripted_connection(
            vec![Ok(Response::UnusedDojoClose {
                dojo_id: target_id,
                outcome: UnusedDojoCloseOutcome::Closed {
                    topology_revision: TopologyRevision::new(2),
                    lair_removed,
                },
            })],
            Arc::clone(&detached),
        )
        .await;
        let (owner, owner_peer, owner_finished) =
            scripted_connection(Vec::new(), Arc::new(AtomicBool::new(false))).await;
        let mut state = TopologyManagerState {
            tabs: WindowTabSet::new(managed_tab(HashMap::new())),
            transient_owners: HashMap::from([(target_lair, owner)]),
            agent_status_republish: false,
        };
        state.tabs.open_or_activate(target).unwrap();
        let (updates, receiver) = mpsc::channel(16);
        acknowledge_tab_removals(receiver);
        let outcome = handle_session_manager_command(
            &ConnectionFactory::local(),
            WindowTopologyCommand::CloseTab { dojo_id: target_id },
            &mut connection,
            &close_unused_config(true),
            &SharedImageContentCache::default(),
            &updates,
            &mut state,
        )
        .await;
        assert!(matches!(outcome, TopologyManagerCommandOutcome::Continue));
        let _ = finished.send(());
        let _ = owner_finished.send(());
        assert_eq!(peer.await.unwrap().len(), 1);
        assert!(owner_peer.await.unwrap().is_empty());
        state.transient_owners.contains_key(&target_lair)
    })
    .await
    .expect("bounded scripted transient tab close")
}

#[tokio::test]
async fn a_removed_transient_lair_releases_its_owner_connection() {
    assert!(
        !close_transient_tab(true).await,
        "the removed Lair's owner connection is dropped"
    );
    assert!(
        close_transient_tab(false).await,
        "a Lair that still exists keeps its owner"
    );
}

#[test]
fn only_a_confirmed_close_of_the_same_dojo_counts() {
    let dojo_id = DojoId::new();
    let confirmed = |closed_id, lair_removed| {
        Ok(Response::UnusedDojoClose {
            dojo_id: closed_id,
            outcome: UnusedDojoCloseOutcome::Closed {
                topology_revision: TopologyRevision::new(2),
                lair_removed,
            },
        })
    };
    assert_eq!(
        unused_dojo_closed(dojo_id, &confirmed(dojo_id, false)),
        Some(false)
    );
    assert_eq!(
        unused_dojo_closed(dojo_id, &confirmed(dojo_id, true)),
        Some(true)
    );
    assert_eq!(
        unused_dojo_closed(dojo_id, &confirmed(DojoId::new(), true)),
        None
    );
    assert_eq!(
        unused_dojo_closed(
            dojo_id,
            &Ok(Response::UnusedDojoClose {
                dojo_id,
                outcome: UnusedDojoCloseOutcome::Used {
                    reason: DojoUse::Attached,
                },
            })
        ),
        None
    );
    assert_eq!(
        unused_dojo_closed(dojo_id, &Ok(Response::Acknowledged)),
        None
    );
    assert_eq!(
        unused_dojo_closed(dojo_id, &Err(anyhow::anyhow!("connection closed"))),
        None
    );
}

#[tokio::test]
async fn window_close_asks_only_for_single_pane_tabs() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let detached = Arc::new(AtomicBool::new(false));
        let single = managed_tab(slow_pane(&detached));
        let single_id = single.dojo_id;
        let mut split_panes = slow_pane(&Arc::new(AtomicBool::new(false)));
        split_panes.extend(slow_pane(&Arc::new(AtomicBool::new(false))));
        let split = managed_tab(split_panes);
        let (mut connection, peer, finished) =
            scripted_connection(vec![Ok(closed(single_id))], Arc::clone(&detached)).await;
        let mut state = TopologyManagerState {
            tabs: WindowTabSet::new(split),
            transient_owners: HashMap::new(),
            agent_status_republish: false,
        };
        state.tabs.open_or_activate(single).unwrap();
        close_unused_tabs_on_exit(&mut connection, &close_unused_config(true), &mut state).await;
        let _ = finished.send(());
        let requests = peer.await.unwrap();
        assert_eq!(
            requests,
            vec![(Request::CloseUnusedDojo { dojo_id: single_id }, true)]
        );
        // The split tab keeps its panes for the ordinary shutdown.
        let remaining = state
            .tabs
            .iter()
            .map(|tab| tab.value.pane_tasks.len())
            .sum::<usize>();
        assert_eq!(remaining, 2);
    })
    .await
    .expect("bounded scripted Window close");
}

#[test]
fn a_close_notice_queued_before_the_window_exits_is_seen() {
    let (sender, mut receiver) = mpsc::channel(4);
    sender
        .try_send(WindowTopologyCommand::RequestSessionPicker)
        .unwrap();
    sender.try_send(WindowTopologyCommand::CloseWindow).unwrap();
    drop(sender);
    let mut config = AppConfig::default();
    assert!(close_notice_queued(&mut receiver, &mut config));
    let (sender, mut receiver) = mpsc::channel::<WindowTopologyCommand>(4);
    drop(sender);
    assert!(!close_notice_queued(&mut receiver, &mut config));
}

#[test]
fn a_configuration_reload_is_consumed_and_others_pass_through() {
    let mut config = close_unused_config(false);
    assert_eq!(
        apply_configuration_reload(
            &mut config,
            WindowTopologyCommand::ReloadConfiguration { close_unused: true }
        ),
        None
    );
    assert!(config.multiplexer_lifetime.close_unused);
    assert_eq!(
        apply_configuration_reload(&mut config, WindowTopologyCommand::RequestSessionPicker),
        Some(WindowTopologyCommand::RequestSessionPicker)
    );
    assert!(config.multiplexer_lifetime.close_unused);
}

#[tokio::test]
async fn close_tab_after_a_reload_uses_the_reloaded_close_unused() {
    let mut config = close_unused_config(false);
    apply_configuration_reload(
        &mut config,
        WindowTopologyCommand::ReloadConfiguration { close_unused: true },
    );
    let (requests, detached) = close_second_tab(config, vec![Ok(closed(DojoId::new()))]).await;
    assert!(detached, "the reloaded setting asks the pane to detach");
    assert!(matches!(
        requests[..],
        [(Request::CloseUnusedDojo { .. }, true)]
    ));

    let mut config = close_unused_config(true);
    apply_configuration_reload(
        &mut config,
        WindowTopologyCommand::ReloadConfiguration {
            close_unused: false,
        },
    );
    let (requests, detached) = close_second_tab(config, Vec::new()).await;
    assert!(requests.is_empty(), "the reload turned close-unused off");
    assert!(!detached);
}

#[test]
fn a_reload_queued_before_the_window_exits_still_applies_to_the_close() {
    let (sender, mut receiver) = mpsc::channel(4);
    sender
        .try_send(WindowTopologyCommand::ReloadConfiguration { close_unused: true })
        .unwrap();
    sender.try_send(WindowTopologyCommand::CloseWindow).unwrap();
    drop(sender);
    let mut config = close_unused_config(false);
    assert!(close_notice_queued(&mut receiver, &mut config));
    assert!(config.multiplexer_lifetime.close_unused);
}

// Waits out the real three-second detach bound.
#[tokio::test]
async fn a_pane_that_cannot_detach_leaves_the_tab_detached_without_asking() {
    let (mut connection, peer, finished) =
        scripted_connection(Vec::new(), Arc::new(AtomicBool::new(false))).await;
    let cancellation = CancellationToken::new();
    let stuck = HashMap::from([(
        SplintId::new(),
        PaneTask {
            cancellation,
            task: tokio::spawn(std::future::pending()),
            detach_on_cancel: Arc::default(),
        },
    )]);
    release_closed_tab(
        &mut connection,
        &close_unused_config(true),
        DojoId::new(),
        stuck,
    )
    .await;
    let _ = finished.send(());
    assert!(peer.await.unwrap().is_empty());
}
