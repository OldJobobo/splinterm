//! Headless manager routing against a scripted protocol peer, never a user daemon.
use super::*;
use splinterm_core::{Lair, LairRetention};
use splinterm_protocol::{ClientFrame, PROTOCOL_VERSION, ServerFrame, ServerLimits};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn routed(
    command: WindowTopologyCommand,
    replies: Vec<Response>,
) -> (Vec<Request>, Vec<WindowTopologyUpdate>) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (client, server) = tokio::io::duplex(65536);
        let (finished, finish) = tokio::sync::oneshot::channel();
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
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            let mut requests = Vec::new();
            for result in replies {
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
                requests.push(request);
                writer
                    .write_all(
                        &splinterm_protocol::encode_frame(&ServerFrame::Response {
                            request_id,
                            result,
                        })
                        .unwrap(),
                    )
                    .await
                    .unwrap();
            }
            let _ = finish.await;
            requests
        });
        let (reader, writer) = tokio::io::split(client);
        let mut connection = Connection::connect_remote_interactive_transport(reader, writer)
            .await
            .unwrap();
        let (mut state, unrelated_dojo) = unrelated_state();
        let (updates, mut receiver) = mpsc::channel(16);
        let outcome = handle_session_manager_command(
            &ConnectionFactory::local(),
            command,
            &mut connection,
            &AppConfig::default(),
            &SharedImageContentCache::default(),
            &updates,
            &mut state,
        )
        .await;
        assert!(matches!(outcome, TopologyManagerCommandOutcome::Continue));
        assert_eq!(state.tabs.len(), 1);
        assert!(state.tabs.get(unrelated_dojo).is_some());
        let _ = finished.send(());
        let requests = peer.await.unwrap();
        let mut output = Vec::new();
        while let Ok(update) = receiver.try_recv() {
            output.push(update);
        }
        (requests, output)
    })
    .await
    .expect("bounded scripted manager routing")
}

fn unrelated_state() -> (TopologyManagerState, DojoId) {
    let unrelated = Lair::new("unrelated active", "/tmp".into());
    let dojo = &unrelated.dojos[0];
    let unrelated_dojo = dojo.id;
    let identity = WindowDojoIdentity {
        topology_revision: TopologyRevision::new(1),
        lair_id: unrelated.id,
        dojo_id: dojo.id,
        lair_name: unrelated.name.clone(),
        lair_retention: unrelated.retention,
        dojo_name: dojo.name.clone(),
    };
    let state = TopologyManagerState {
        tabs: WindowTabSet::new(DojoTab::new(
            unrelated.id,
            dojo.id,
            ManagedDojo {
                identity,
                root: dojo.root.clone(),
                pending_focus: None,
                pane_tasks: HashMap::new(),
            },
        )),
        transient_owners: HashMap::new(),
    };
    (state, unrelated_dojo)
}

fn saved() -> (Lair, ExplorerContextTarget) {
    let mut lair = Lair::new("saved authoritative name", "/tmp".into());
    lair.retention = LairRetention::Saved;
    let splint_id = lair.dojos[0].default_focus;
    let pane = lair.dojos[0].root.find_splint_mut(splint_id).unwrap();
    pane.state = SplintState::Exited(0);
    pane.last_incarnation = Some(7);
    let target = ExplorerContextTarget {
        topology_revision: TopologyRevision::new(8),
        node: NavigationNodeId::Lair(lair.id),
        live_incarnation: None,
    };
    (lair, target)
}
fn catalog(lair: &Lair, target: ExplorerContextTarget) -> Response {
    Response::Lairs {
        lairs: vec![lair.clone()],
        topology_revision: target.topology_revision,
    }
}
fn committed() -> Response {
    Response::TopologyCommitted {
        topology_revision: TopologyRevision::new(9),
    }
}

#[tokio::test]
async fn successful_restore_notifies_explorer_without_attaching() {
    let (lair, target) = saved();
    for command in [
        WindowTopologyCommand::RestoreLair {
            lair_id: lair.id,
            expected_topology_revision: target.topology_revision,
        },
        WindowTopologyCommand::RestoreDojo {
            dojo_id: lair.dojos[0].id,
            expected_topology_revision: target.topology_revision,
        },
    ] {
        let mut target = target;
        if let WindowTopologyCommand::RestoreDojo { dojo_id, .. } = &command {
            target.node = NavigationNodeId::Dojo {
                lair_id: lair.id,
                dojo_id: *dojo_id,
            };
        }
        let (_, updates) = routed(
            target.command(command),
            vec![
                catalog(&lair, target),
                Response::RestoreCompleted {
                    topology_revision: target.topology_revision,
                    results: Vec::new(),
                },
            ],
        )
        .await;
        assert!(matches!(
            updates.as_slice(),
            [WindowTopologyUpdate::LairExplorerInvalidated]
        ));
        assert!(
            !updates
                .iter()
                .any(|update| matches!(update, WindowTopologyUpdate::TabFailed { .. }))
        );
    }
}

#[tokio::test]
async fn restore_rejects_invalid_acknowledgement_without_success_notification() {
    let (lair, target) = saved();
    for command in [
        WindowTopologyCommand::RestoreLair {
            lair_id: lair.id,
            expected_topology_revision: target.topology_revision,
        },
        WindowTopologyCommand::RestoreDojo {
            dojo_id: lair.dojos[0].id,
            expected_topology_revision: target.topology_revision,
        },
    ] {
        let mut target = target;
        if let WindowTopologyCommand::RestoreDojo { dojo_id, .. } = &command {
            target.node = NavigationNodeId::Dojo {
                lair_id: lair.id,
                dojo_id: *dojo_id,
            };
        }
        let (_, updates) = routed(
            target.command(command),
            vec![catalog(&lair, target), committed()],
        )
        .await;
        assert!(matches!(
            updates.as_slice(),
            [WindowTopologyUpdate::TabFailed { .. }]
        ));
    }
}

#[test]
fn explorer_observes_detached_stop_and_restore_without_revision_change() {
    use splinterm_core::Topology;
    use splinterm_protocol::{SplintLifecycle, SplintRuntimeSummary, TopologySnapshot};
    let lair = Lair::new("detached", "/tmp".into());
    let splint_id = lair.dojos[0].default_focus;
    let snapshot = |runtime: SplintRuntimeSummary| {
        let mut lair = lair.clone();
        lair.dojos[0]
            .root
            .find_splint_mut(splint_id)
            .unwrap()
            .last_incarnation = runtime.last_incarnation;
        let mut topology = Topology::new();
        topology.insert_lair_at(topology.revision(), lair).unwrap();
        let snapshot = TopologySnapshot {
            revision: topology.revision(),
            topology,
            runtimes: vec![runtime],
        };
        snapshot.validate().unwrap();
        snapshot
    };
    let running = SplintRuntimeSummary {
        splint_id,
        live_incarnation: Some(7),
        last_incarnation: Some(7),
        restorable: false,
        lifecycle: SplintLifecycle::Running,
        exit_status: None,
    };
    let stopped = SplintRuntimeSummary {
        live_incarnation: None,
        restorable: true,
        lifecycle: SplintLifecycle::Exited,
        ..running.clone()
    };
    let restored = SplintRuntimeSummary {
        live_incarnation: Some(8),
        last_incarnation: Some(8),
        ..running.clone()
    };
    let mut observer = ExplorerObservation::default();
    for runtime in [running, stopped, restored] {
        let snapshot = snapshot(runtime);
        assert!(observer.observe(&snapshot));
        assert!(
            !observer.observe(&snapshot),
            "unchanged polls must not trigger refresh loops"
        );
    }
}

#[test]
fn explorer_observation_tracks_revision_and_ignores_runtime_order() {
    use splinterm_protocol::{SplintLifecycle, SplintRuntimeSummary, TopologySnapshot};
    let mut snapshot = TopologySnapshot {
        topology: splinterm_core::Topology::new(),
        revision: TopologyRevision::new(1),
        runtimes: (0..2)
            .map(|_| SplintRuntimeSummary {
                splint_id: SplintId::new(),
                live_incarnation: None,
                last_incarnation: None,
                restorable: false,
                lifecycle: SplintLifecycle::Exited,
                exit_status: None,
            })
            .collect(),
    };
    let mut observer = ExplorerObservation::default();
    assert!(observer.observe(&snapshot));
    snapshot.runtimes.reverse();
    assert!(!observer.observe(&snapshot));
    snapshot.revision = TopologyRevision::new(2);
    assert!(observer.observe(&snapshot));
    snapshot.runtimes.pop();
    assert!(observer.observe(&snapshot));
}

#[tokio::test]
async fn detached_saved_lair_mutations_reach_protocol_without_attaching() {
    let (lair, target) = saved();
    for (command, request, response) in [
        (
            WindowTopologyCommand::RenameLair {
                lair_id: lair.id,
                name: "renamed".into(),
            },
            Request::RenameLair {
                expected_topology_revision: target.topology_revision,
                lair_id: lair.id,
                name: "renamed".into(),
            },
            committed(),
        ),
        (
            WindowTopologyCommand::SetLairRetention {
                lair_id: lair.id,
                retention: LairRetention::Pinned,
            },
            Request::SetLairRetention {
                expected_topology_revision: target.topology_revision,
                lair_id: lair.id,
                retention: LairRetention::Pinned,
            },
            committed(),
        ),
        (
            WindowTopologyCommand::RestoreLair {
                lair_id: lair.id,
                expected_topology_revision: target.topology_revision,
            },
            Request::RestoreLair {
                expected_topology_revision: target.topology_revision,
                lair_id: lair.id,
            },
            Response::RestoreCompleted {
                topology_revision: TopologyRevision::new(9),
                results: Vec::new(),
            },
        ),
    ] {
        let (requests, updates) = routed(
            target.command(command),
            vec![catalog(&lair, target), response],
        )
        .await;
        let restore = matches!(request, Request::RestoreLair { .. });
        assert_eq!(requests, vec![Request::ListLairs, request]);
        if restore {
            assert!(matches!(
                updates.as_slice(),
                [WindowTopologyUpdate::LairExplorerInvalidated]
            ));
        } else {
            assert!(updates.is_empty());
        }
    }
}

#[tokio::test]
async fn explorer_primary_dojo_restore_only_previews_and_correlates_stale_failure() {
    let (lair, guard) = saved();
    let dojo_id = lair.dojos[0].id;
    let target = SessionPickerTarget {
        topology_revision: guard.topology_revision,
        lair_id: lair.id,
        dojo_id,
        action: NavigationAction::PreviewRestoreDojo,
    };
    let correlation = Some(LairExplorerActivationTarget::Dojo(target));
    for stale in [false, true] {
        let reply_guard = ExplorerContextTarget {
            topology_revision: if stale {
                TopologyRevision::new(9)
            } else {
                guard.topology_revision
            },
            ..guard
        };
        let (requests, updates) = routed(
            WindowTopologyCommand::OpenDojo {
                target,
                explorer_target: correlation,
            },
            vec![catalog(&lair, reply_guard)],
        )
        .await;
        // Primary activation requests a preview only; no Attach/Restore/Focus is sent.
        assert_eq!(requests, vec![Request::ListLairs]);
        assert_eq!(updates.len(), 1);
        if stale {
            assert!(matches!(&updates[0], WindowTopologyUpdate::TabFailed {
                explorer_target, dojo_id: Some(id), ..
            } if *explorer_target == correlation && *id == dojo_id));
        } else {
            let WindowTopologyUpdate::ShowLairPrompt {
                kind,
                target: prompt,
            } = &updates[0]
            else {
                panic!("expected restore confirmation")
            };
            assert_eq!(*kind, LairPromptKind::Restore);
            assert_eq!(prompt.topology_revision, guard.topology_revision);
            assert_eq!(prompt.lair_id, lair.id);
            assert_eq!(prompt.dojo_id, Some(dojo_id));
            assert_eq!(prompt.targets.len(), 1);
            assert_eq!(prompt.targets[0].splint_id, lair.dojos[0].default_focus);
            assert_eq!(prompt.targets[0].incarnation, 7);
        }
    }
}

#[tokio::test]
async fn detached_dojo_rename_and_restore_reach_exact_authoritative_target() {
    let (lair, mut target) = saved();
    let dojo_id = lair.dojos[0].id;
    target.node = NavigationNodeId::Dojo {
        lair_id: lair.id,
        dojo_id,
    };
    for (command, request) in [
        (
            WindowTopologyCommand::RenameDojo {
                dojo_id,
                name: "other dojo".into(),
            },
            Request::RenameDojo {
                expected_topology_revision: target.topology_revision,
                dojo_id,
                name: "other dojo".into(),
                promote_transient_lair: false,
            },
        ),
        (
            WindowTopologyCommand::RestoreDojo {
                dojo_id,
                expected_topology_revision: target.topology_revision,
            },
            Request::RestoreDojo {
                expected_topology_revision: target.topology_revision,
                dojo_id,
            },
        ),
    ] {
        let restore = matches!(request, Request::RestoreDojo { .. });
        let response = if restore {
            Response::RestoreCompleted {
                topology_revision: target.topology_revision,
                results: Vec::new(),
            }
        } else {
            committed()
        };
        let (requests, updates) = routed(
            target.command(command),
            vec![catalog(&lair, target), response],
        )
        .await;
        assert_eq!(requests, vec![Request::ListLairs, request]);
        if restore {
            assert!(matches!(
                updates.as_slice(),
                [WindowTopologyUpdate::LairExplorerInvalidated]
            ));
        } else {
            assert!(updates.is_empty());
        }
    }
}

#[tokio::test]
async fn detached_prompts_fetch_names_and_complete_incarnations_without_navigation() {
    let (mut lair, mut target) = saved();
    let dojo_id = lair.dojos[0].id;
    target.node = NavigationNodeId::Dojo {
        lair_id: lair.id,
        dojo_id,
    };
    for kind in [LairPromptKind::Rename, LairPromptKind::Restore] {
        let (requests, updates) = routed(
            target.command(WindowTopologyCommand::RequestDojoPrompt { dojo_id, kind }),
            vec![catalog(&lair, target), catalog(&lair, target)],
        )
        .await;
        assert_eq!(requests, vec![Request::ListLairs, Request::ListLairs]);
        assert!(
            matches!(&updates[..], [WindowTopologyUpdate::ShowExplorerPrompt { guard, target: prompt, .. }] if *guard == target && prompt.name == lair.dojos[0].name && prompt.dojo_id == Some(dojo_id))
        );
    }
    // Saved-only Dojos cannot acquire a termination prompt.
    let (_, updates) = routed(
        target.command(WindowTopologyCommand::RequestDojoPrompt {
            dojo_id,
            kind: LairPromptKind::Terminate,
        }),
        vec![catalog(&lair, target), catalog(&lair, target)],
    )
    .await;
    assert!(matches!(
        &updates[..],
        [WindowTopologyUpdate::TabFailed { .. }]
    ));
    let mut running = splinterm_core::Splint::shell("/tmp".into());
    running.state = SplintState::Running;
    running.last_incarnation = Some(11);
    let running_id = running.id;
    lair.dojos[0].root = LayoutNode::Branch {
        axis: Axis::Horizontal,
        ratio: SplitRatio::new(500).unwrap(),
        first: Box::new(lair.dojos[0].root.clone()),
        second: Box::new(LayoutNode::Leaf(running)),
    };
    let (_, updates) = routed(
        target.command(WindowTopologyCommand::RequestDojoPrompt {
            dojo_id,
            kind: LairPromptKind::Terminate,
        }),
        vec![catalog(&lair, target), catalog(&lair, target)],
    )
    .await;
    assert!(
        matches!(&updates[..], [WindowTopologyUpdate::ShowExplorerPrompt { target: prompt, .. }] if prompt.targets.len() == 2 && prompt.targets[1].splint_id == running_id && prompt.targets[1].incarnation == 11)
    );
    let exited_id = lair.dojos[0].default_focus;
    lair.dojos[0]
        .root
        .find_splint_mut(exited_id)
        .unwrap()
        .last_incarnation = None;
    let (_, updates) = routed(
        target.command(WindowTopologyCommand::RequestDojoPrompt {
            dojo_id,
            kind: LairPromptKind::Terminate,
        }),
        vec![catalog(&lair, target), catalog(&lair, target)],
    )
    .await;
    assert!(matches!(
        &updates[..],
        [WindowTopologyUpdate::TabFailed { .. }]
    ));
}

#[tokio::test]
async fn stale_or_cross_target_context_dispatch_never_reaches_mutation() {
    let (lair, target) = saved();
    for command in [
        target.command(WindowTopologyCommand::RenameLair {
            lair_id: LairId::new(),
            name: "wrong".into(),
        }),
        ExplorerContextTarget {
            topology_revision: TopologyRevision::new(6),
            ..target
        }
        .command(WindowTopologyCommand::RenameLair {
            lair_id: lair.id,
            name: "stale".into(),
        }),
    ] {
        let (requests, updates) = routed(command, vec![catalog(&lair, target)]).await;
        assert_eq!(requests, vec![Request::ListLairs]);
        assert!(matches!(
            &updates[..],
            [WindowTopologyUpdate::TabFailed { .. }]
        ));
    }
}

#[test]
fn pane_edits_revalidate_revision_parent_and_incarnation() {
    let (mut lair, mut target) = saved();
    let dojo_id = lair.dojos[0].id;
    let splint_id = lair.dojos[0].default_focus;
    lair.dojos[0].root.find_splint_mut(splint_id).unwrap().state = SplintState::Running;
    target.node = NavigationNodeId::Splint {
        lair_id: lair.id,
        dojo_id,
        splint_id,
    };
    target.live_incarnation = Some(7);
    for command in [
        WindowTopologyCommand::Close {
            dojo_id,
            target: splint_id,
        },
        WindowTopologyCommand::Split {
            dojo_id,
            target: splint_id,
            axis: Axis::Horizontal,
            pending: None,
        },
    ] {
        assert!(
            validate_explorer_context(target, &command, target.topology_revision, &[lair.clone()])
                .is_ok()
        );
        assert!(
            validate_explorer_context(
                ExplorerContextTarget {
                    live_incarnation: Some(8),
                    ..target
                },
                &command,
                target.topology_revision,
                &[lair.clone()]
            )
            .is_err()
        );
        assert!(
            validate_explorer_context(target, &command, TopologyRevision::new(9), &[lair.clone()])
                .is_err()
        );
        assert!(
            validate_explorer_context(
                ExplorerContextTarget {
                    node: NavigationNodeId::Splint {
                        lair_id: lair.id,
                        dojo_id: DojoId::new(),
                        splint_id
                    },
                    ..target
                },
                &command,
                target.topology_revision,
                &[lair.clone()]
            )
            .is_err()
        );
    }
}

fn topology_response(lair: Lair) -> Response {
    let mut ids = Vec::new();
    layout_splint_ids(&lair.dojos[0].root, &mut ids);
    let runtimes = ids
        .into_iter()
        .map(|id| {
            let pane = lair.dojos[0].root.find_splint(id).unwrap();
            let live = pane.state == SplintState::Running;
            splinterm_protocol::SplintRuntimeSummary {
                splint_id: id,
                live_incarnation: live.then_some(pane.last_incarnation.unwrap()),
                last_incarnation: pane.last_incarnation,
                restorable: !live,
                lifecycle: if live {
                    splinterm_protocol::SplintLifecycle::Running
                } else {
                    splinterm_protocol::SplintLifecycle::Exited
                },
                exit_status: None,
            }
        })
        .collect();
    let mut topology = splinterm_core::Topology::new();
    topology.insert_lair_at(topology.revision(), lair).unwrap();
    Response::Topology {
        snapshot: splinterm_protocol::TopologySnapshot {
            revision: topology.revision(),
            topology,
            runtimes,
        },
    }
}

#[tokio::test]
async fn detached_dojo_termination_kills_only_captured_live_incarnation_then_closes() {
    let (mut lair, mut guard) = saved();
    let dojo_id = lair.dojos[0].id;
    let splint_id = lair.dojos[0].default_focus;
    guard.node = NavigationNodeId::Dojo {
        lair_id: lair.id,
        dojo_id,
    };
    let exited = lair.clone();
    lair.dojos[0].root.find_splint_mut(splint_id).unwrap().state = SplintState::Running;
    let (requests, updates) = routed(
        guard.command(WindowTopologyCommand::TerminateDojo {
            dojo_id,
            splints: vec![(splint_id, 7)],
        }),
        vec![
            catalog(&lair, guard),
            topology_response(lair.clone()),
            Response::SplintKilled {
                splint_id,
                incarnation: 7,
                exit_status: splinterm_protocol::ProcessExitStatus {
                    code: Some(0),
                    signal: None,
                },
            },
            topology_response(exited),
            committed(),
        ],
    )
    .await;
    assert!(updates.is_empty());
    assert_eq!(
        requests,
        vec![
            Request::ListLairs,
            Request::InspectTopology,
            Request::KillSplint {
                splint_id,
                incarnation: 7
            },
            Request::InspectTopology,
            Request::CloseDojo {
                expected_topology_revision: TopologyRevision::new(1),
                dojo_id
            }
        ]
    );
    // A restarted process is rejected before the first KillSplint, not recaptured.
    lair.dojos[0]
        .root
        .find_splint_mut(splint_id)
        .unwrap()
        .last_incarnation = Some(8);
    let (requests, updates) = routed(
        guard.command(WindowTopologyCommand::TerminateDojo {
            dojo_id,
            splints: vec![(splint_id, 7)],
        }),
        vec![catalog(&lair, guard), topology_response(lair)],
    )
    .await;
    assert_eq!(requests, vec![Request::ListLairs, Request::InspectTopology]);
    assert!(matches!(
        &updates[..],
        [WindowTopologyUpdate::TabFailed { .. }]
    ));
}

#[tokio::test]
async fn detached_lair_termination_preserves_the_confirmed_live_target_list() {
    let (mut lair, guard) = saved();
    let dojo_id = lair.dojos[0].id;
    let splint_id = lair.dojos[0].default_focus;
    lair.dojos[0].root.find_splint_mut(splint_id).unwrap().state = SplintState::Running;
    let targets = vec![MutationTarget {
        lair_id: lair.id,
        dojo_id,
        splint_id,
        incarnation: 7,
    }];
    let (requests, updates) = routed(
        guard.command(WindowTopologyCommand::TerminateLair {
            lair_id: lair.id,
            targets: targets.clone(),
        }),
        vec![catalog(&lair, guard), committed()],
    )
    .await;
    assert_eq!(
        requests,
        vec![
            Request::ListLairs,
            Request::TerminateLair {
                expected_topology_revision: guard.topology_revision,
                lair_id: lair.id,
                targets
            }
        ]
    );
    assert!(updates.is_empty());
}

#[tokio::test]
async fn dojo_prompt_scope_excludes_unrelated_missing_incarnations() {
    let (mut lair, mut guard) = saved();
    let dojo_id = lair.dojos[0].id;
    let splint_id = lair.dojos[0].default_focus;
    guard.node = NavigationNodeId::Dojo {
        lair_id: lair.id,
        dojo_id,
    };
    let unrelated = splinterm_core::Dojo::with_shell("unrelated", "/tmp".into());
    lair.dojos.push(unrelated);
    for kind in [
        LairPromptKind::Rename,
        LairPromptKind::Restore,
        LairPromptKind::Terminate,
    ] {
        lair.dojos[0].root.find_splint_mut(splint_id).unwrap().state =
            if kind == LairPromptKind::Restore {
                SplintState::Exited(0)
            } else {
                SplintState::Running
            };
        let (_, updates) = routed(
            guard.command(WindowTopologyCommand::RequestDojoPrompt { dojo_id, kind }),
            vec![catalog(&lair, guard), catalog(&lair, guard)],
        )
        .await;
        assert!(
            matches!(&updates[..], [WindowTopologyUpdate::ShowExplorerPrompt { target, .. }] if target.targets.len() == 1 && target.targets[0].splint_id == splint_id && target.targets[0].incarnation == 7)
        );
    }
}

#[tokio::test]
async fn stale_explicit_navigation_failure_keeps_exact_activation_correlation() {
    let (lair, guard) = saved();
    let target = SessionPickerTarget {
        topology_revision: TopologyRevision::new(6),
        lair_id: lair.id,
        dojo_id: lair.dojos[0].id,
        action: NavigationAction::ActivateDojo,
    };
    let correlation = Some(LairExplorerActivationTarget::Dojo(target));
    let guard = ExplorerContextTarget {
        node: NavigationNodeId::Dojo {
            lair_id: lair.id,
            dojo_id: target.dojo_id,
        },
        ..guard
    };
    let (requests, updates) = routed(
        guard.command(WindowTopologyCommand::OpenDojo {
            target,
            explorer_target: correlation,
        }),
        vec![catalog(&lair, guard)],
    )
    .await;
    assert_eq!(requests, vec![Request::ListLairs]);
    assert!(
        matches!(&updates[..], [WindowTopologyUpdate::TabFailed { explorer_target, .. }] if *explorer_target == correlation)
    );
}

#[tokio::test]
async fn mixed_lair_termination_matches_daemon_complete_pane_contract() {
    let (mut lair, guard) = saved();
    let mut live_dojo = splinterm_core::Dojo::with_shell("live", "/tmp".into());
    let live_id = live_dojo.default_focus;
    let pane = live_dojo.root.find_splint_mut(live_id).unwrap();
    pane.state = SplintState::Running;
    pane.last_incarnation = Some(11);
    lair.dojos.push(live_dojo);
    let full = collect_lair_targets(&lair, LairTargetState::All).unwrap();
    assert_eq!(full.len(), 2);
    let live_only = collect_lair_targets(&lair, LairTargetState::Live).unwrap();
    assert!(validate_explorer_lair_termination_capture(&lair, &full).is_ok());
    assert!(validate_explorer_lair_termination_capture(&lair, &live_only).is_err());
    let (_, updates) = routed(
        guard.command(WindowTopologyCommand::RequestLairPrompt {
            lair_id: lair.id,
            kind: LairPromptKind::Terminate,
        }),
        vec![catalog(&lair, guard), catalog(&lair, guard)],
    )
    .await;
    assert!(
        matches!(&updates[..], [WindowTopologyUpdate::ShowExplorerPrompt { target, .. }] if target.targets == full)
    );
    let (requests, updates) = routed(
        guard.command(WindowTopologyCommand::TerminateLair {
            lair_id: lair.id,
            targets: full.clone(),
        }),
        vec![catalog(&lair, guard), committed()],
    )
    .await;
    assert_eq!(
        requests[1],
        Request::TerminateLair {
            expected_topology_revision: guard.topology_revision,
            lair_id: lair.id,
            targets: full.clone()
        }
    );
    assert!(updates.is_empty());
    for targets in [live_only, {
        let mut stale = full;
        stale[0].incarnation += 1;
        stale
    }] {
        let (requests, updates) = routed(
            guard.command(WindowTopologyCommand::TerminateLair {
                lair_id: lair.id,
                targets,
            }),
            vec![catalog(&lair, guard)],
        )
        .await;
        assert_eq!(requests, vec![Request::ListLairs]);
        assert!(matches!(
            &updates[..],
            [WindowTopologyUpdate::TabFailed { .. }]
        ));
    }
    let exited_id = lair.dojos[0].default_focus;
    lair.dojos[0]
        .root
        .find_splint_mut(exited_id)
        .unwrap()
        .last_incarnation = None;
    let (_, updates) = routed(
        guard.command(WindowTopologyCommand::RequestLairPrompt {
            lair_id: lair.id,
            kind: LairPromptKind::Terminate,
        }),
        vec![catalog(&lair, guard), catalog(&lair, guard)],
    )
    .await;
    assert!(matches!(
        &updates[..],
        [WindowTopologyUpdate::TabFailed { .. }]
    ));
    let (saved, guard) = saved();
    let (_, updates) = routed(
        guard.command(WindowTopologyCommand::RequestLairPrompt {
            lair_id: saved.id,
            kind: LairPromptKind::Terminate,
        }),
        vec![catalog(&saved, guard), catalog(&saved, guard)],
    )
    .await;
    assert!(matches!(
        &updates[..],
        [WindowTopologyUpdate::TabFailed { .. }]
    ));
}
