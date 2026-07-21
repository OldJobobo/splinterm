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
    Dojo, DojoId, Lair, LayoutNode, Splint, SplintId, SplintState, TopologyRevision, Window,
    WindowId,
};
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen, AuditPage, AutomationScope, CellAttributes,
    ClientFrame, ClientRole, ColorSource, ControlStatus, ErrorCode, MouseTracking,
    PROTOCOL_VERSION, PersistentAuthorizationStatus, ProcessExitStatus, ProtocolError, Response,
    RestoreLeafResult, ScrollbackPage, ServerFrame, ServerLimits, SplintLifecycle,
    SplintRuntimeSummary, SubscriptionEvent, TerminalCell, TerminalInputModes, TerminalRow,
    TerminalSnapshot, TopologySnapshot, UnderlineStyle, encode_frame,
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
    let dojo_id: DojoId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap();
    let window_id: WindowId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap();
    let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
    let mut splint = Splint::shell(PathBuf::from("/tmp"));
    splint.id = splint_id;
    "build".clone_into(&mut splint.title);
    splint.state = SplintState::Running;
    let dojo = Dojo {
        id: dojo_id,
        name: "main".to_owned(),
        windows: vec![Window {
            id: window_id,
            title: "terminal".to_owned(),
            default_focus: splint_id,
            root: LayoutNode::Leaf(splint),
        }],
    };
    let mut lair = Lair::new();
    lair.insert_dojo_at(TopologyRevision::new(0), dojo).unwrap();
    TopologySnapshot {
        revision: lair.revision(),
        lair,
        runtimes: vec![SplintRuntimeSummary {
            splint_id,
            live_incarnation: Some(2),
            lifecycle: SplintLifecycle::Running,
            exit_status: None,
        }],
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
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
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
                request_id: 2,
                request: splinterm_protocol::Request::Attach { .. }
            }
        ));
        send_response(
            &mut stream,
            2,
            Response::Attached {
                subscription_id: 7,
                snapshot: reviewed_terminal_snapshot(),
            },
        );
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
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
                    request_id: 4,
                    request: splinterm_protocol::Request::SearchScrollback { .. }
                }
            )
        } else {
            matches!(
                request,
                ClientFrame::Request {
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
                request_id: 2,
                request: splinterm_protocol::Request::AuthorizationStatus { .. }
            }
        ));
        send_response(
            &mut stream,
            2,
            Response::AuthorizationStatus {
                grants: Vec::new(),
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
                        request_id: 1,
                        request: splinterm_protocol::Request::InspectSplint { .. }
                    }
                ));
                send_response(
                    &mut stream,
                    1,
                    Response::Splint {
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
                        request_id: 2,
                        request: splinterm_protocol::Request::Attach { .. }
                    }
                ));
                send_response(
                    &mut stream,
                    2,
                    Response::Attached {
                        subscription_id: 7,
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
                request_id: 2,
                request: splinterm_protocol::Request::AcquireControl { .. }
            }
        ));
        send_response(
            &mut stream,
            2,
            Response::ControlGranted { controller_id: 42 },
        );
        let request = read_client_frame(&mut stream);
        assert!(if input {
            matches!(
                request,
                ClientFrame::Request {
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
                request_id: 4,
                request: splinterm_protocol::Request::ReleaseControl { controller_id: 42 }
            }
        ));
        send_response(&mut stream, 4, Response::Acknowledged);
    });
    (socket, server)
}

enum ExpectedMutation {
    Ratio,
    Kill,
    Revoke,
    RestoreSplint,
    RestoreWindow,
}

fn serve_mutation(
    expected: ExpectedMutation,
    result: Response,
) -> (PathBuf, thread::JoinHandle<()>) {
    let socket = socket_path();
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let mut stream = accept_client(&listener);
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
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
                    request_id: 2,
                    request: splinterm_protocol::Request::SetSplitRatio { .. }
                }
            ),
            ExpectedMutation::Kill => matches!(
                request,
                ClientFrame::Request {
                    request_id: 2,
                    request: splinterm_protocol::Request::KillSplint { incarnation: 2, .. }
                }
            ),
            ExpectedMutation::Revoke => matches!(
                request,
                ClientFrame::Request {
                    request_id: 2,
                    request: splinterm_protocol::Request::RevokeAccess { grant_id: 42 }
                }
            ),
            ExpectedMutation::RestoreSplint => matches!(
                request,
                ClientFrame::Request {
                    request_id: 2,
                    request: splinterm_protocol::Request::RestoreSplint { .. }
                }
            ),
            ExpectedMutation::RestoreWindow => matches!(
                request,
                ClientFrame::Request {
                    request_id: 2,
                    request: splinterm_protocol::Request::RestoreWindow { .. }
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
                    lair: Lair::new(),
                    runtimes: Vec::new(),
                },
            },
        );
        let request = read_client_frame(&mut stream);
        assert!(matches!(
            request,
            ClientFrame::Request {
                request_id: 2,
                request: splinterm_protocol::Request::CreateDojo { .. }
            }
        ));
        let topology = reviewed_topology();
        let dojo = topology.lair.dojos().next().unwrap().clone();
        send_response(
            &mut stream,
            2,
            Response::DojoCreated {
                dojo,
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
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
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
        (&["--output", "json", "list"], "list_dojos"),
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
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
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
                        snapshot: reviewed_terminal_snapshot(),
                    },
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
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
        dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
        window_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
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
                "close-window",
                "018f4d8c-2a18-4b31-8c2f-9e7c5de77102",
                "--output",
                "json",
            ],
            "close_window",
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
    let (socket, server) =
        serve_mutation(ExpectedMutation::Revoke, Response::AccessRevoked { grant });
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

    let window_id = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102";
    let (socket, server) = serve_mutation(
        ExpectedMutation::RestoreWindow,
        Response::RestoreCompleted {
            topology_revision: TopologyRevision::new(1),
            results: Vec::new(),
        },
    );
    let output = run_json_ping(&["restore-window", window_id, "--output", "json"], &socket);
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
    assert_eq!(document["operation"], "create_dojo");
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
    assert_eq!(document["operation"], "list_dojos");
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
            }
        ));
        stream
            .write_all(
                &encode_frame(&ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            read_client_frame(&mut stream),
            ClientFrame::Request {
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
    assert_eq!(document["schema"], "splinterm.cli.v1");
    assert_eq!(document["operation"], "ping");
    assert_eq!(document["data"]["status"], "awake");
}

#[test]
fn output_json_ping_serializes_pre_request_failures() {
    let socket = socket_path();
    let output = run_json_ping(
        &["--output", "json", "--schema-major", "2", "ping"],
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
fn malformed_machine_invocation_has_empty_stdout() {
    for arguments in [
        vec!["--output", "wat", "ping"],
        vec!["--output", "ndjson", "ping"],
        vec!["--output", "json", "subscribe", "topology"],
        vec!["--output", "json", "consent"],
        vec!["--output", "human", "--schema-major", "1", "ping"],
        vec!["--output", "json", "audit", "--max-records", "129"],
    ] {
        let output = binary().args(arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}
