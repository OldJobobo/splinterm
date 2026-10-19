use std::{
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread,
};

use serde_json::Value;
use splinterm_core::{
    Dojo, DojoId, Lair, LairId, LayoutNode, Splint, SplintId, SplintState, Topology,
    TopologyRevision,
};
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen, AuditPage, AutomationScope, CellAttributes,
    ClientFrame, ClientRole, ColorSource, ControlStatus, ErrorCode, MouseTracking,
    PROTOCOL_VERSION, PersistentAuthorizationStatus, ProcessExitStatus, ProtocolError, Response,
    RestoreLeafResult, ScrollbackPage, ServerFrame, ServerLimits, SplintLifecycle,
    SplintRuntimeSummary, SubscriptionEvent, TerminalCell, TerminalInputModes, TerminalProvenance,
    TerminalRow, TerminalSnapshot, TopologySnapshot, UnderlineStyle, encode_frame,
};

static NEXT_SOCKET: AtomicU64 = AtomicU64::new(1);

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_splinterm"))
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "splinterm-automation-cli-{}-{}.sock",
        std::process::id(),
        NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
    ))
}

fn read_client_frame(stream: &mut UnixStream) -> ClientFrame {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn accept_client(listener: &UnixListener) -> UnixStream {
    let (mut stream, _) = listener.accept().unwrap();
    assert!(matches!(
        read_client_frame(&mut stream),
        ClientFrame::Hello { .. }
    ));
    stream
        .write_all(
            &encode_frame(&ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                limits: ServerLimits::default(),
                development_terminal_access: false,
                daemon_hostname: None,
                daemon_generation: 1,
            })
            .unwrap(),
        )
        .unwrap();
    stream
}

fn run_json_ping(arguments: &[&str], socket: &std::path::Path) -> Output {
    binary()
        .env("SPLINTERM_SOCKET", socket)
        .args(arguments)
        .output()
        .unwrap()
}

fn reviewed_topology() -> TopologySnapshot {
    let lair_id: LairId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap();
    let dojo_id: DojoId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap();
    let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
    let mut splint = Splint::shell(PathBuf::from("/tmp"));
    splint.id = splint_id;
    "build".clone_into(&mut splint.title);
    splint.last_incarnation = Some(2);
    splint.state = SplintState::Running;
    let dojo = Lair {
        id: lair_id,
        name: "main".to_owned(),
        lifetime: splinterm_core::LairLifetime::default(),
        retention: splinterm_core::LairRetention::default(),
        provenance: None,
        dojos: vec![Dojo {
            id: dojo_id,
            name: "terminal".to_owned(),
            default_focus: splint_id,
            root: LayoutNode::Leaf(splint),
        }],
    };
    let mut topology = Topology::new();
    topology
        .insert_lair_at(TopologyRevision::new(0), dojo)
        .unwrap();
    TopologySnapshot {
        revision: topology.revision(),
        topology,
        runtimes: vec![SplintRuntimeSummary {
            splint_id,
            live_incarnation: Some(2),
            last_incarnation: Some(2),
            restorable: false,
            lifecycle: SplintLifecycle::Running,
            exit_status: None,
            agent_status: None,
        }],
    }
}

fn reviewed_terminal_provenance(
    terminal_revision: u64,
    history_generation: u64,
) -> TerminalProvenance {
    TerminalProvenance {
        lair_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
        dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
        splint_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap(),
        incarnation: 2,
        topology_revision: TopologyRevision::new(1),
        terminal_revision,
        history_generation,
        title: "build".to_owned(),
    }
}

fn reviewed_terminal_snapshot() -> TerminalSnapshot {
    TerminalSnapshot {
        splint_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap(),
        incarnation: 2,
        revision: 9,
        columns: 80,
        rows: 1,
        cursor_column: 2,
        cursor_row: 0,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: TerminalInputModes {
            application_cursor: false,
            application_keypad: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor_visible: true,
            cursor_blink: false,
            mouse_tracking: MouseTracking::None,
            sgr_mouse: false,
        },
        palette: vec![0; 256],
        default_colors: [0; 3],
        title: "build".to_owned(),
        visible_rows: vec![TerminalRow {
            row_id: Some(1),
            linebreak: true,
            cells: vec![TerminalCell {
                content: "ok".to_owned(),
                spacer_remaining: None,
                attributes: CellAttributes {
                    bold: false,
                    dim: false,
                    italic: false,
                    underline: UnderlineStyle::None,
                    underline_color_source: ColorSource::Default,
                    underline_color: 0,
                    strikethrough: false,
                    blink: false,
                    conceal: false,
                    reverse: false,
                    foreground_source: ColorSource::Default,
                    foreground: 0,
                    background_source: ColorSource::Default,
                    background: 0,
                },
            }],
        }],
        history_generation: 3,
        oldest_available_scrollback_row_id: None,
        newest_available_scrollback_row_id: None,
        scrollback_rows: Vec::new(),
        available_scrollback_rows: 0,
        omitted_oldest_scrollback_rows: 0,
        images: None,
        exited_code: None,
        exited_signal: None,
    }
}

fn send_response(stream: &mut UnixStream, request_id: u64, result: Response) {
    stream
        .write_all(&encode_frame(&ServerFrame::Response { request_id, result }).unwrap())
        .unwrap();
}

fn serve_history_response(search: bool, result: Response) -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Hello { .. }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                    daemon_generation: 1,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::InspectTopology
            }
        ));
        send_response(
            &mut stream,
            1,
            Response::Topology {
                snapshot: reviewed_topology(),
            },
        );
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 2,
                request: splinterm_protocol::Request::Attach { .. }
            }
        ));
        send_response(
            &mut stream,
            2,
            Response::Attached {
                subscription_id: 7,
                provenance: reviewed_terminal_provenance(9, 2),
                snapshot: reviewed_terminal_snapshot(),
            },
        );
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 3,
                request: splinterm_protocol::Request::Detach { subscription_id: 7 }
            }
        ));
        send_response(&mut stream, 3, Response::Acknowledged);
        let request = read_client_frame(&mut stream);
        assert!(if search {
            matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 4,
                    request: splinterm_protocol::Request::SearchScrollback { .. }
                }
            )
        } else {
            matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 4,
                    request: splinterm_protocol::Request::ScrollbackPage {
                        before_row_id: 2,
                        max_rows: 16,
                        ..
                    }
                }
            )
        });
        send_response(&mut stream, 4, result);
    });
    (socket, server)
}

fn serve_authorization_status() -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::AuthorizationStatus {
                    incarnation: None,
                    ..
                }
            }
        ));
        send_response(
            &mut stream,
            1,
            Response::AuthorizationStatus {
                lair_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
                dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
                incarnation: 2,
                topology_revision: TopologyRevision::new(1),
                policy_generation: 1,
                grants: Vec::new(),
                lair_grants: Vec::new(),
                persistent: vec![PersistentAuthorizationStatus {
                    policy_rule_id: "editor".to_owned(),
                    scopes: vec![AutomationScope::TerminalVisibleRead],
                    expires_at_unix_seconds: None,
                }],
                development_bypass: false,
            },
        );
    });
    (socket, server)
}

fn serve_audit_page() -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::AuditInspect {
                    after_audit_id: Some(72),
                    max_records: 12
                }
            }
        ));
        send_response(
            &mut stream,
            1,
            Response::AuditPage {
                page: AuditPage {
                    records: Vec::new(),
                    retention_gap: false,
                    oldest_available_audit_id: None,
                    newest_available_audit_id: None,
                    next_after_audit_id: None,
                },
            },
        );
    });
    (socket, server)
}

enum ExpectedSubscription {
    Terminal,
    Topology,
    Control,
}

#[allow(
    clippy::too_many_lines,
    reason = "one mock keeps all three subscription handshakes and resync paths comparable"
)]
fn serve_subscription(stream_kind: ExpectedSubscription) -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
        match stream_kind {
            ExpectedSubscription::Terminal | ExpectedSubscription::Control => {
                assert!(matches!(
                    read_client_frame(&mut stream),
                    ClientFrame::Request {
                        diagnostic_correlation: _,
                        request_id: 1,
                        request: splinterm_protocol::Request::InspectSplint { .. }
                    }
                ));
                send_response(
                    &mut stream,
                    1,
                    Response::Splint {
                        resolved_cwd: None,
                        lair_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
                        dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
                        title: "build".to_owned(),
                        topology_revision: TopologyRevision::new(1),
                        runtime: reviewed_topology().runtimes[0].clone(),
                    },
                );
            }
            ExpectedSubscription::Topology => {}
        }
        match stream_kind {
            ExpectedSubscription::Terminal => {
                assert!(matches!(
                    read_client_frame(&mut stream),
                    ClientFrame::Request {
                        diagnostic_correlation: _,
                        request_id: 2,
                        request: splinterm_protocol::Request::Attach { .. }
                    }
                ));
                send_response(
                    &mut stream,
                    2,
                    Response::Attached {
                        subscription_id: 7,
                        provenance: reviewed_terminal_provenance(9, 2),
                        snapshot: reviewed_terminal_snapshot(),
                    },
                );
                stream
                    .write_all(
                        &encode_frame(&ServerFrame::Event {
                            subscription_id: 7,
                            sequence: 2,
                            event: SubscriptionEvent::ResyncRequired {
                                current_revision: 10,
                            },
                        })
                        .unwrap(),
                    )
                    .unwrap();
            }
            ExpectedSubscription::Topology => {
                assert!(matches!(
                    read_client_frame(&mut stream),
                    ClientFrame::Request {
                        diagnostic_correlation: _,
                        request_id: 1,
                        request: splinterm_protocol::Request::SubscribeTopology
                    }
                ));
                send_response(
                    &mut stream,
                    1,
                    Response::TopologySubscribed {
                        subscription_id: 8,
                        snapshot: reviewed_topology(),
                    },
                );
                stream
                    .write_all(
                        &encode_frame(&ServerFrame::Event {
                            subscription_id: 8,
                            sequence: 1,
                            event: SubscriptionEvent::TopologyResyncRequired {
                                current_revision: TopologyRevision::new(2),
                            },
                        })
                        .unwrap(),
                    )
                    .unwrap();
            }
            ExpectedSubscription::Control => {
                assert!(matches!(
                    read_client_frame(&mut stream),
                    ClientFrame::Request {
                        diagnostic_correlation: _,
                        request_id: 2,
                        request: splinterm_protocol::Request::SubscribeControl { .. }
                    }
                ));
                let status = ControlStatus {
                    splint_id,
                    incarnation: 2,
                    controlled: false,
                    locally_owned: false,
                };
                send_response(
                    &mut stream,
                    2,
                    Response::ControlSubscribed {
                        subscription_id: 9,
                        status,
                    },
                );
                for frame in [
                    ServerFrame::Event {
                        subscription_id: 9,
                        sequence: 1,
                        event: SubscriptionEvent::ControlTransferRequested { transfer_id: 99 },
                    },
                    ServerFrame::Event {
                        subscription_id: 9,
                        sequence: 2,
                        event: SubscriptionEvent::ControlTransferResolved {
                            transfer_id: 99,
                            outcome: splinterm_protocol::ControlTransferOutcome::Granted,
                            controller_id: Some(777),
                        },
                    },
                    ServerFrame::Event {
                        subscription_id: 9,
                        sequence: 4,
                        event: SubscriptionEvent::ControlStatusChanged { status },
                    },
                ] {
                    stream.write_all(&encode_frame(&frame).unwrap()).unwrap();
                }
            }
        }
    });
    (socket, server)
}

fn serve_terminal_action(
    input: bool,
    action: Result<Response, ProtocolError>,
) -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::InspectTopology
            }
        ));
        send_response(
            &mut stream,
            1,
            Response::Topology {
                snapshot: reviewed_topology(),
            },
        );
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 2,
                request: splinterm_protocol::Request::AcquireControl { .. }
            }
        ));
        send_response(
            &mut stream,
            2,
            Response::ControlGranted {
                controller_id: 42,
                lair_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
                dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
            },
        );
        let request = read_client_frame(&mut stream);
        assert!(if input {
            matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 3,
                    request: splinterm_protocol::Request::Input {
                        controller_id: 42,
                        ..
                    }
                }
            )
        } else {
            matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 3,
                    request: splinterm_protocol::Request::Resize {
                        controller_id: 42,
                        columns: 120,
                        rows: 40,
                        ..
                    }
                }
            )
        });
        match action {
            Ok(response) => send_response(&mut stream, 3, response),
            Err(error) => stream
                .write_all(
                    &encode_frame(&ServerFrame::Error {
                        request_id: Some(3),
                        error,
                    })
                    .unwrap(),
                )
                .unwrap(),
        }
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 4,
                request: splinterm_protocol::Request::ReleaseControl { controller_id: 42 }
            }
        ));
        send_response(&mut stream, 4, Response::Acknowledged);
    });
    (socket, server)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpectedMutation {
    Ratio,
    Kill,
    Revoke,
    RestoreSplint,
    RestoreDojo,
}

fn serve_mutation(
    expected: ExpectedMutation,
    result: Response,
) -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        if expected == ExpectedMutation::Revoke {
            assert!(matches!(
                read_client_frame(&mut stream),
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 1,
                    request: splinterm_protocol::Request::RevokeAccess { grant_id: 42 }
                }
            ));
            send_response(&mut stream, 1, result);
            return;
        }
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::InspectTopology
            }
        ));
        send_response(
            &mut stream,
            1,
            Response::Topology {
                snapshot: reviewed_topology(),
            },
        );
        let request = read_client_frame(&mut stream);
        assert!(match expected {
            ExpectedMutation::Ratio => matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 2,
                    request: splinterm_protocol::Request::SetSplitRatio { .. }
                }
            ),
            ExpectedMutation::Kill => matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 2,
                    request: splinterm_protocol::Request::KillSplint { incarnation: 2, .. }
                }
            ),
            ExpectedMutation::Revoke => unreachable!(),
            ExpectedMutation::RestoreSplint => matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 2,
                    request: splinterm_protocol::Request::RestoreSplint { .. }
                }
            ),
            ExpectedMutation::RestoreDojo => matches!(
                request,
                ClientFrame::Request {
                    diagnostic_correlation: _,
                    request_id: 2,
                    request: splinterm_protocol::Request::RestoreDojo { .. }
                }
            ),
        });
        send_response(&mut stream, 2, result);
    });
    (socket, server)
}

fn serve_create() -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::InspectTopology
            }
        ));
        send_response(
            &mut stream,
            1,
            Response::Topology {
                snapshot: TopologySnapshot {
                    revision: TopologyRevision::new(0),
                    topology: Topology::new(),
                    runtimes: Vec::new(),
                },
            },
        );
        let request = read_client_frame(&mut stream);
        assert!(matches!(
            request,
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 2,
                request: splinterm_protocol::Request::CreateLair { .. }
            }
        ));
        let topology = reviewed_topology();
        let dojo = topology.topology.lairs().next().unwrap().clone();
        send_response(
            &mut stream,
            2,
            Response::LairCreated {
                lair: dojo,
                incarnation: 2,
                topology_revision: topology.revision,
            },
        );
    });
    (socket, server)
}

fn serve_one(result: Result<Response, ProtocolError>) -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Hello {
                minimum_version: PROTOCOL_VERSION,
                maximum_version: PROTOCOL_VERSION,
                role: ClientRole::Automation,
                handoff_window: false,
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                    daemon_generation: 1,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::InspectTopology
            }
        ));
        let frame = match result {
            Ok(result) => ServerFrame::Response {
                request_id: 1,
                result,
            },
            Err(error) => ServerFrame::Error {
                request_id: Some(1),
                error,
            },
        };
        stream.write_all(&encode_frame(&frame).unwrap()).unwrap();
    });
    (socket, server)
}

#[test]
fn output_json_reads_use_reviewed_shapes_and_global_flag_placement() {
    let splint_id = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103";
    let cases: &[(&[&str], &str)] = &[
        (&["--output", "json", "list"], "list_lairs"),
        (&["topology", "--output", "json"], "inspect_topology"),
        (
            &[
                "--timeout-ms",
                "1000",
                "inspect",
                splint_id,
                "--output",
                "json",
            ],
            "inspect_splint",
        ),
    ];
    for (arguments, operation) in cases {
        let (socket, server) = serve_one(Ok(Response::Topology {
            snapshot: reviewed_topology(),
        }));
        let output = run_json_ping(arguments, &socket);
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(output.stdout.last(), Some(&b'\n'));
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["request_id"], "1");
        assert_eq!(document["operation"], *operation);
        assert_eq!(document["ok"], true);
        assert_eq!(document["resource"]["topology_revision"], 1);
        assert_eq!(document["truncated"], false);
    }
}

#[test]
fn output_json_focus_uses_only_the_narrow_projection() {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::ReadGraphicalFocus,
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Response {
                    request_id: 1,
                    result: Response::GraphicalFocus {
                        focused_splint_id: Some(splint_id),
                        cwd: Some(PathBuf::from("/home/example/project")),
                    },
                })
                .unwrap(),
            )
            .unwrap();
    });

    let output = run_json_ping(&["--output", "json", "focus"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "focus");
    assert_eq!(document["ok"], true);
    assert_eq!(document["data"]["splint_id"], splint_id.to_string());
    assert_eq!(document["data"]["cwd"], "/home/example/project");
    assert!(document.get("resource").is_none());
    assert_eq!(document["data"].as_object().unwrap().len(), 2);
}

#[test]
fn output_json_focus_rejects_invalid_daemon_projection_with_stable_error() {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::ReadGraphicalFocus,
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Response {
                    request_id: 1,
                    result: Response::GraphicalFocus {
                        focused_splint_id: None,
                        cwd: Some(PathBuf::from("/private/path")),
                    },
                })
                .unwrap(),
            )
            .unwrap();
    });

    let output = run_json_ping(&["--output", "json", "focus"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert_eq!(output.status.code(), Some(70));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["error"]["message"], "focus response was invalid");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("/private/path"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("/private/path"));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one subprocess fixture proves exact terminal provenance and detach cleanup"
)]
fn output_json_snapshot_uses_exact_provenance_and_detaches() {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Hello { .. }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                    daemon_generation: 1,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::InspectTopology
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Response {
                    request_id: 1,
                    result: Response::Topology {
                        snapshot: reviewed_topology(),
                    },
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 2,
                request: splinterm_protocol::Request::Attach {
                    scrollback_rows: 0,
                    ..
                }
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Response {
                    request_id: 2,
                    result: Response::Attached {
                        subscription_id: 7,
                        provenance: reviewed_terminal_provenance(9, 2),
                        snapshot: reviewed_terminal_snapshot(),
                    },
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 3,
                request: splinterm_protocol::Request::Detach { subscription_id: 7 }
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Response {
                    request_id: 3,
                    result: Response::Acknowledged,
                })
                .unwrap(),
            )
            .unwrap();
    });
    let output = run_json_ping(
        &[
            "snapshot",
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
            "--output",
            "json",
        ],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/automation/fixtures/valid/cli-terminal-snapshot.json"
    ))
    .unwrap();
    let mut expected = fixture["document"].clone();
    expected["request_id"] = serde_json::json!("1");
    assert_eq!(document, expected);
}

#[test]
fn expected_incarnation_rejects_retargeting_before_terminal_access() {
    let (socket, server) = serve_one(Ok(Response::Topology {
        snapshot: reviewed_topology(),
    }));
    let output = run_json_ping(
        &[
            "--output",
            "json",
            "snapshot",
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
            "--expected-incarnation",
            "99",
        ],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert_eq!(output.status.code(), Some(5));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "terminal_snapshot");
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "stale_incarnation");
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected incarnation"));
}

#[test]
fn output_json_history_reads_emit_pages_and_resync_without_query_echo() {
    let terminal = reviewed_terminal_snapshot();
    let (socket, server) = serve_history_response(
        false,
        Response::ScrollbackPage {
            provenance: reviewed_terminal_provenance(9, 2),
            page: ScrollbackPage {
                splint_id: terminal.splint_id,
                incarnation: terminal.incarnation,
                terminal_revision: terminal.revision,
                history_generation: terminal.history_generation,
                oldest_available_row_id: Some(1),
                newest_available_row_id: Some(1),
                rows: terminal.visible_rows.clone(),
                has_older: false,
            },
        },
    );
    let output = run_json_ping(
        &[
            "--output",
            "json",
            "scrollback",
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
        ],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "scrollback_page");
    assert_eq!(document["data"]["kind"], "page");
    assert_eq!(document["data"]["continuation_cursor"], Value::Null);

    let (socket, server) = serve_history_response(
        true,
        Response::SearchResyncRequired {
            provenance: reviewed_terminal_provenance(10, 3),
            current_revision: 10,
            history_generation: 3,
        },
    );
    let output = run_json_ping(
        &[
            "search",
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
            "super-secret-query",
            "--output",
            "json",
        ],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("super-secret-query"));
    let document: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(document["operation"], "search_scrollback");
    assert_eq!(document["resource"]["terminal_revision"], 10);
    assert_eq!(document["data"]["reason"], "stale_revision");
}

#[test]
fn output_json_authorization_and_audit_use_reviewed_shapes() {
    let (socket, server) = serve_authorization_status();
    let output = run_json_ping(
        &[
            "authorization",
            "status",
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
            "--output",
            "json",
        ],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/automation/fixtures/valid/cli-authorization-status.json"
    ))
    .unwrap();
    let mut expected = fixture["document"].clone();
    expected["request_id"] = serde_json::json!("1");
    assert_eq!(document, expected);

    let (socket, server) = serve_audit_page();
    let output = run_json_ping(
        &[
            "--output",
            "json",
            "audit",
            "--after",
            "72",
            "--max-records",
            "12",
        ],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "audit_inspect");
    assert_eq!(document["data"]["retention"], "daemon_lifetime");
    assert_eq!(document["data"]["records"], serde_json::json!([]));
    assert!(document.get("resource").is_none());
}

#[test]
fn output_ndjson_subscriptions_emit_initial_state_and_terminate_on_resync() {
    let splint_id = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103";
    for (stream_kind, arguments, initial_type, resync_stream) in [
        (
            ExpectedSubscription::Terminal,
            vec!["--output", "ndjson", "subscribe", "terminal", splint_id],
            "snapshot",
            "terminal",
        ),
        (
            ExpectedSubscription::Topology,
            vec!["subscribe", "topology", "--output", "ndjson"],
            "topology_snapshot",
            "topology",
        ),
        (
            ExpectedSubscription::Control,
            vec!["subscribe", "control", splint_id, "--output", "ndjson"],
            "control_snapshot",
            "control",
        ),
    ] {
        let (socket, server) = serve_subscription(stream_kind);
        let output = run_json_ping(&arguments, &socket);
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let records = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let resync = records.last().unwrap();
        assert_eq!(
            records.len(),
            if resync_stream == "control" { 4 } else { 2 }
        );
        assert_eq!(records[0]["subscription_id"], "1");
        assert_eq!(records[0]["sequence"], 1);
        assert_eq!(records[0]["event_type"], initial_type);
        assert_eq!(resync["sequence"], records.len());
        assert_eq!(resync["event_type"], "resync_required");
        assert_eq!(resync["stream"], resync_stream);
        if resync_stream == "control" {
            assert_eq!(records[1]["data"]["transfer_id"], "1");
            assert_eq!(records[2]["data"]["transfer_id"], "1");
            assert!(
                records
                    .iter()
                    .all(|record| !record.to_string().contains("99"))
            );
            assert!(
                records
                    .iter()
                    .all(|record| !record.to_string().contains("777"))
            );
        }
    }
}

#[test]
fn output_json_terminal_actions_are_atomic_and_do_not_leak_input() {
    let splint_id = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103";
    let acknowledged = || Response::TerminalActionAcknowledged {
        lair_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
        dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
        splint_id: splint_id.parse().unwrap(),
        incarnation: 2,
        terminal_revision: 9,
        history_generation: 3,
    };
    let (socket, server) = serve_terminal_action(true, Ok(acknowledged()));
    let output = run_json_ping(
        &["send", splint_id, "super-secret-input", "--output", "json"],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("super-secret-input"));
    let document: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(document["data"], serde_json::json!({"acknowledged": true}));

    let (socket, server) = serve_terminal_action(false, Ok(acknowledged()));
    let output = run_json_ping(
        &["resize", splint_id, "120", "40", "--output", "json"],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["data"]["columns"], 120);
    assert_eq!(document["data"]["rows"], 40);

    let (socket, server) = serve_terminal_action(
        true,
        Err(ProtocolError::new(ErrorCode::Unauthorized, "input denied")),
    );
    let output = run_json_ping(&["send", splint_id, "hidden", "--output", "json"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("hidden"));
}

#[test]
fn output_json_mutations_enforce_confirmation_before_connecting() {
    let socket = socket_path();
    for (arguments, operation) in [
        (
            vec![
                "close",
                "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
                "--output",
                "json",
            ],
            "close_splint",
        ),
        (
            vec![
                "close-dojo",
                "018f4d8c-2a18-4b31-8c2f-9e7c5de77102",
                "--output",
                "json",
            ],
            "close_dojo",
        ),
        (
            vec!["authorization", "revoke", "42", "--output", "json"],
            "revoke_access",
        ),
    ] {
        let output = run_json_ping(&arguments, &socket);
        assert_eq!(output.status.code(), Some(3));
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["operation"], operation);
        assert_eq!(document["error"]["code"], "confirmation_required");
    }
    assert!(!socket.exists());
}

#[test]
fn output_json_mutations_correlate_topology_kill_and_revoke() {
    let splint_id = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103";
    let (socket, server) = serve_mutation(
        ExpectedMutation::Ratio,
        Response::TopologyCommitted {
            topology_revision: TopologyRevision::new(2),
        },
    );
    let output = run_json_ping(&["ratio", splint_id, "400", "--output", "json"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "set_split_ratio");
    assert_eq!(document["resource"]["topology_revision"], 2);

    let (socket, server) = serve_mutation(
        ExpectedMutation::Kill,
        Response::SplintKilled {
            splint_id: splint_id.parse().unwrap(),
            incarnation: 2,
            exit_status: ProcessExitStatus {
                code: None,
                signal: Some(15),
            },
        },
    );
    let output = run_json_ping(&["kill", splint_id, "--yes", "--output", "json"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "kill_splint");
    assert_eq!(document["data"]["confirmed"], true);

    let grant = AccessGrant {
        grant_id: 42,
        splint_id: splint_id.parse().unwrap(),
        incarnation: 2,
        scopes: vec![AccessScope::Observe],
        requester: "/usr/bin/editor".to_owned(),
        expires_at_unix_seconds: 100,
    };
    let (socket, server) = serve_mutation(
        ExpectedMutation::Revoke,
        Response::AccessRevoked {
            lair_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
            dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
            authorization_revision: 4,
            grant,
        },
    );
    let output = run_json_ping(
        &["authorization", "revoke", "42", "--yes", "--output", "json"],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "revoke_access");
    assert_eq!(document["data"]["revoked_grant_id"], "42");
}

#[test]
fn output_json_restore_consumes_leaf_results_and_rejects_missing_members() {
    let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
    let (socket, server) = serve_mutation(
        ExpectedMutation::RestoreSplint,
        Response::RestoreCompleted {
            topology_revision: TopologyRevision::new(2),
            results: vec![RestoreLeafResult {
                splint_id,
                incarnation: Some(3),
                error: None,
            }],
        },
    );
    let output = run_json_ping(
        &["restore", &splint_id.to_string(), "--output", "json"],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "restore_splint");
    assert_eq!(document["resource"]["incarnation"], 3);

    let dojo_id = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102";
    let (socket, server) = serve_mutation(
        ExpectedMutation::RestoreDojo,
        Response::RestoreCompleted {
            topology_revision: TopologyRevision::new(1),
            results: Vec::new(),
        },
    );
    let output = run_json_ping(&["restore-dojo", dojo_id, "--output", "json"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(!output.status.success());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["error"]["code"], "internal");
}

#[test]
fn output_json_create_uses_atomic_response_without_argv_leak() {
    let (socket, server) = serve_create();
    let output = run_json_ping(
        &[
            "new",
            "main",
            "--output",
            "json",
            "--",
            "printf",
            "secret-argument",
        ],
        &socket,
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("secret-argument"));
    let document: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(document["operation"], "create_lair");
    assert_eq!(document["resource"]["topology_revision"], 1);
    assert_eq!(document["data"]["created"], true);
}

#[test]
fn output_json_read_maps_daemon_errors_without_stdout_noise() {
    let (socket, server) = serve_one(Err(ProtocolError::new(
        ErrorCode::Unauthorized,
        "policy denied topology metadata",
    )));
    let output = run_json_ping(&["list", "--output", "json"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stdout.last(), Some(&b'\n'));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "list_lairs");
    assert_eq!(document["error"]["code"], "unauthorized");
    assert_eq!(document["error"]["retryable"], false);
}

#[test]
fn output_json_ping_keeps_stdout_pristine() {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Hello {
                minimum_version: PROTOCOL_VERSION,
                maximum_version: PROTOCOL_VERSION,
                role: ClientRole::Automation,
                handoff_window: false,
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                    daemon_generation: 1,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
                diagnostic_correlation: _,
                request_id: 1,
                request: splinterm_protocol::Request::Ping
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Response {
                    request_id: 1,
                    result: Response::Pong,
                })
                .unwrap(),
            )
            .unwrap();
    });

    let output = run_json_ping(&["--output", "json", "ping"], &socket);
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.last(), Some(&b'\n'));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema"], "splinterm.cli.v2");
    assert_eq!(document["operation"], "ping");
    assert_eq!(document["data"]["status"], "awake");
}

#[test]
fn output_json_ping_serializes_pre_request_failures() {
    let socket = socket_path();
    let output = run_json_ping(
        &["--output", "json", "--schema-major", "1", "ping"],
        &socket,
    );
    assert_eq!(output.status.code(), Some(4));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["request_id"], "1");
    assert_eq!(document["error"]["code"], "unsupported_schema");

    let output = run_json_ping(&["ping", "--output", "json"], &socket);
    assert_eq!(output.status.code(), Some(4));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "ping");
    assert_eq!(document["error"]["code"], "internal");
}

#[test]
fn output_json_focus_errors_do_not_expose_socket_paths() {
    let socket = socket_path();
    let output = run_json_ping(&["--output", "json", "focus"], &socket);
    assert_eq!(output.status.code(), Some(4));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "focus");
    assert_eq!(document["error"]["message"], "focus service unavailable");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(socket.to_str().unwrap()));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(socket.to_str().unwrap()));
}

#[test]
fn malformed_machine_invocation_has_empty_stdout() {
    for arguments in [
        vec!["--output", "wat", "ping"],
        vec!["--output", "ndjson", "ping"],
        vec!["--output", "json", "subscribe", "topology"],
        vec!["--output", "json", "consent"],
        vec!["--output", "human", "--schema-major", "1", "ping"],
        vec!["--output", "json", "audit", "--max-records", "129"],
        vec!["focus"],
    ] {
        let output = binary().args(arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

const AGENT_SPLINT: &str = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103";
const OTHER_SPLINT: &str = "018f4d8c-2a18-4b31-8c2f-9e7c5de77104";

/// Runs the client without inherited Splint discovery hints, then with exactly
/// the given environment.
fn run_agent_status(
    arguments: &[&str],
    socket: &std::path::Path,
    environment: &[(&str, &str)],
) -> Output {
    binary()
        .env("SPLINTERM_SOCKET", socket)
        .env_remove("SPLINTERM_SPLINT_ID")
        .env_remove("SPLINTERM_SPLINT_INCARNATION")
        .envs(environment.iter().copied())
        .args(arguments)
        .output()
        .unwrap()
}

fn inside_splint() -> [(&'static str, &'static str); 2] {
    [
        ("SPLINTERM_SPLINT_ID", AGENT_SPLINT),
        ("SPLINTERM_SPLINT_INCARNATION", "2"),
    ]
}

/// Serves exactly one automation-role report and checks it without any
/// topology preflight.
fn serve_agent_status_report(
    expected: splinterm_protocol::Request,
    result: Result<Response, ProtocolError>,
) -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Hello {
                role: ClientRole::Automation,
                ..
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                    daemon_generation: 1,
                })
                .unwrap(),
            )
            .unwrap();
        let ClientFrame::Request {
            request_id,
            request,
            ..
        } = read_client_frame(&mut stream)
        else {
            panic!("expected one agent status request");
        };
        assert_eq!(request, expected);
        let frame = match result {
            Ok(result) => ServerFrame::Response { request_id, result },
            Err(error) => ServerFrame::Error {
                request_id: Some(request_id),
                error,
            },
        };
        stream.write_all(&encode_frame(&frame).unwrap()).unwrap();
    });
    (socket, server)
}

fn agent_report(
    splint: &str,
    incarnation: u64,
    status: Option<splinterm_protocol::AgentStatusReport>,
) -> splinterm_protocol::Request {
    splinterm_protocol::Request::ReportAgentStatus {
        splint_id: splint.parse().unwrap(),
        incarnation,
        status,
    }
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/automation/fixtures/valid")
        .join(name);
    serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap()["document"].clone()
}

#[test]
fn agent_status_set_defaults_to_the_splint_environment_without_echoing_detail() {
    use splinterm_protocol::{AgentState, AgentStatusReport};

    let (socket, server) = serve_agent_status_report(
        agent_report(
            AGENT_SPLINT,
            2,
            Some(AgentStatusReport {
                state: AgentState::NeedsInput,
                detail: Some("waiting for approval".into()),
                ttl_seconds: None,
            }),
        ),
        Ok(Response::AgentStatusAccepted {
            sequence: 4,
            expires_at_ms: Some(1_760_630_400_000),
        }),
    );
    let output = run_agent_status(
        &[
            "--output",
            "json",
            "agent-status",
            "set",
            "needs_input",
            "--detail",
            "waiting for approval",
        ],
        &socket,
        &inside_splint(),
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document, fixture("cli-agent-status-set.json"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("waiting for approval"));
}

// A clear that is the first report for an incarnation is a no-op the daemon
// numbers 0; the CLI envelope reports it as the contract fixture shows.
#[test]
fn agent_status_first_clear_reports_sequence_zero() {
    let (socket, server) = serve_agent_status_report(
        agent_report(AGENT_SPLINT, 2, None),
        Ok(Response::AgentStatusAccepted {
            sequence: 0,
            expires_at_ms: None,
        }),
    );
    let output = run_agent_status(
        &["--output", "json", "agent-status", "clear"],
        &socket,
        &inside_splint(),
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document, fixture("cli-agent-status-first-clear.json"));
}

#[test]
fn agent_status_flags_override_the_environment_in_every_mode() {
    use splinterm_protocol::{AgentState, AgentStatusReport};

    let (socket, server) = serve_agent_status_report(
        agent_report(OTHER_SPLINT, 7, None),
        Ok(Response::AgentStatusAccepted {
            sequence: 5,
            expires_at_ms: None,
        }),
    );
    let output = run_agent_status(
        &[
            "agent-status",
            "clear",
            "--splint",
            OTHER_SPLINT,
            "--incarnation",
            "7",
            "--output",
            "json",
        ],
        &socket,
        &inside_splint(),
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(output.status.success());
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "agent_status_clear");
    assert_eq!(document["resource"]["splint_id"], OTHER_SPLINT);
    assert_eq!(document["resource"]["incarnation"], 7);
    assert_eq!(document["data"]["cleared"], true);

    // Human mode reports through the automation role, never the trusted UI.
    let (socket, server) = serve_agent_status_report(
        agent_report(
            AGENT_SPLINT,
            2,
            Some(AgentStatusReport {
                state: AgentState::Done,
                detail: None,
                ttl_seconds: Some(600),
            }),
        ),
        Ok(Response::AgentStatusAccepted {
            sequence: 6,
            expires_at_ms: Some(1_760_630_400_000),
        }),
    );
    let output = run_agent_status(
        &["agent-status", "set", "done", "--ttl", "600"],
        &socket,
        &inside_splint(),
    );
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("Agent status done reported for Splint {AGENT_SPLINT} incarnation 2.\n")
    );
}

#[test]
fn agent_status_outside_a_splint_is_a_distinct_usage_error() {
    let socket = PathBuf::from("/definitely/not/a/daemon.sock");
    for arguments in [
        vec!["--output", "json", "agent-status", "set", "working"],
        vec!["agent-status", "set", "working"],
        vec!["--output", "json", "agent-status", "clear"],
        vec!["agent-status", "get"],
        vec!["--output", "json", "agent-status", "get"],
    ] {
        let output = run_agent_status(&arguments, &socket, &[]);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not inside a Splint"), "{stderr}");
    }
    let partial = run_agent_status(
        &["--output", "json", "agent-status", "set", "idle"],
        &socket,
        &[("SPLINTERM_SPLINT_ID", AGENT_SPLINT)],
    );
    assert_eq!(partial.status.code(), Some(2));
    assert!(partial.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&partial.stderr);
    assert!(stderr.contains("malformed") && !stderr.contains("not inside a Splint"));
    for arguments in [
        vec!["agent-status", "set", "thinking"],
        vec!["agent-status", "set", "working", "--detail", "two\nlines"],
        vec!["agent-status", "set", "working", "--ttl", "5"],
        vec!["agent-status", "set", "working", "--splint", OTHER_SPLINT],
        vec!["--output", "ndjson", "agent-status", "clear"],
        vec!["--remote", "devbox", "agent-status", "set", "working"],
    ] {
        let output = run_agent_status(&arguments, &socket, &inside_splint());
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
    }
}

#[test]
fn agent_status_daemon_refusals_use_documented_exit_codes() {
    for (error, code, exit, retryable) in [
        (ErrorCode::RateLimited, "resource_limit", 5, true),
        (ErrorCode::StaleIncarnation, "stale_incarnation", 5, true),
        (ErrorCode::Unauthorized, "unauthorized", 3, false),
        (ErrorCode::InvalidArgument, "invalid_argument", 5, false),
    ] {
        let (socket, server) = serve_agent_status_report(
            agent_report(AGENT_SPLINT, 2, None),
            Err(ProtocolError::new(error, "agent status refused")),
        );
        let output = run_agent_status(
            &["--output", "json", "agent-status", "clear"],
            &socket,
            &inside_splint(),
        );
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
        assert_eq!(output.status.code(), Some(exit), "{code}");
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["operation"], "agent_status_clear");
        assert_eq!(document["ok"], false);
        assert_eq!(document["error"]["code"], code);
        assert_eq!(document["error"]["retryable"], retryable);
    }
}

#[test]
fn agent_status_get_and_inspect_add_status_only_when_reported() {
    use splinterm_protocol::{AgentState, AgentStatusSummary};

    let mut topology = reviewed_topology();
    topology.runtimes[0].agent_status = Some(AgentStatusSummary {
        state: AgentState::NeedsInput,
        detail: Some("waiting for approval".into()),
        sequence: 4,
        stale: false,
        reported_age_ms: 1_500,
    });
    let run = |arguments: &[&str], snapshot: &TopologySnapshot, environment: &[(&str, &str)]| {
        let (socket, server) = serve_one(Ok(Response::Topology {
            snapshot: snapshot.clone(),
        }));
        let output = run_agent_status(arguments, &socket, environment);
        server.join().unwrap();
        fs::remove_file(socket).unwrap();
        output
    };
    let output = run(
        &["--output", "json", "agent-status", "get"],
        &topology,
        &inside_splint(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document, fixture("cli-agent-status-get.json"));

    let output = run(
        &["--output", "json", "inspect", AGENT_SPLINT],
        &topology,
        &[],
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["data"]["agent_status"]["state"], "needs_input");
    let output = run(&["--output", "json", "topology"], &topology, &[]);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document, fixture("cli-inspect-topology-agent-status.json"));

    let plain = reviewed_topology();
    let output = run(
        &["--output", "json", "agent-status", "get", AGENT_SPLINT],
        &plain,
        &[],
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["data"]["agent_status"], Value::Null);
    let output = run(&["--output", "json", "inspect", AGENT_SPLINT], &plain, &[]);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(document["data"].get("agent_status").is_none());

    let output = run(
        &["--output", "json", "agent-status", "get", OTHER_SPLINT],
        &plain,
        &[],
    );
    assert_eq!(output.status.code(), Some(5));
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["operation"], "agent_status_get");
    assert_eq!(document["error"]["code"], "not_found");
}
