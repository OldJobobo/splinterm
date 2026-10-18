//! Protocol-level controller-loop regressions; no compositor or daemon required.
use super::*;
use splinterm_core::{DojoId, LairId};
use splinterm_protocol::{ClientFrame, ClientRole, ControlStatus, PROTOCOL_VERSION, ProtocolError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

type Reply = std::result::Result<Response, ErrorCode>;

async fn read_client(reader: &mut (impl AsyncRead + Unpin)) -> ClientFrame {
    let length = reader.read_u32().await.unwrap() as usize;
    assert!(length <= 65536);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn acquire(splint_id: SplintId) -> Request {
    Request::AcquireControl {
        splint_id,
        incarnation: 7,
        modes: vec![ControlMode::Input, ControlMode::Resize],
    }
}

fn transfer(splint_id: SplintId) -> Request {
    Request::RequestControlTransfer {
        splint_id,
        incarnation: 7,
        modes: vec![ControlMode::Input, ControlMode::Resize],
    }
}

fn granted() -> Response {
    Response::ControlGranted {
        controller_id: 2,
        lair_id: LairId::new(),
        dojo_id: DojoId::new(),
    }
}

async fn send_frame(writer: &mut (impl AsyncWrite + Unpin), frame: ServerFrame) {
    writer
        .write_all(&splinterm_protocol::encode_frame(&frame).unwrap())
        .await
        .unwrap();
}

const fn server_hello(limits: ServerLimits) -> ServerFrame {
    ServerFrame::Hello {
        version: PROTOCOL_VERSION,
        limits,
        development_terminal_access: false,
        daemon_hostname: None,
        daemon_generation: 1,
    }
}

const fn control_subscribed(splint_id: SplintId, owned: bool) -> Response {
    Response::ControlSubscribed {
        subscription_id: 9,
        status: ControlStatus {
            splint_id,
            incarnation: 7,
            controlled: owned,
            locally_owned: owned,
        },
    }
}

fn input_acknowledged(splint_id: SplintId) -> Response {
    Response::TerminalActionAcknowledged {
        lair_id: LairId::new(),
        dojo_id: DojoId::new(),
        splint_id,
        incarnation: 7,
        terminal_revision: 10,
        history_generation: 0,
    }
}

async fn run_case(
    splint_id: SplintId,
    owned: bool,
    commands: Vec<WindowCommand>,
    steps: Vec<(Request, Reply)>,
) -> (Result<()>, Vec<WindowUpdate>, usize) {
    let steps = steps
        .into_iter()
        .map(|(request, reply)| (request, reply, Vec::new()))
        .collect();
    run_case_with(splint_id, owned, ServerLimits::default(), commands, steps).await
}

/// Like `run_case`, with negotiated limits and event frames sent before each reply.
async fn run_case_with(
    splint_id: SplintId,
    owned: bool,
    limits: ServerLimits,
    commands: Vec<WindowCommand>,
    steps: Vec<(Request, Reply, Vec<ServerFrame>)>,
) -> (Result<()>, Vec<WindowUpdate>, usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (client, server) = tokio::io::duplex(65536);
        let (finished, finish) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(server);
            let hello = read_client(&mut reader).await;
            assert!(matches!(
                hello,
                ClientFrame::Hello {
                    role: ClientRole::RemoteInteractive,
                    ..
                }
            ));
            send_frame(&mut writer, server_hello(limits)).await;
            let initial = (
                Request::SubscribeControl {
                    splint_id,
                    incarnation: 7,
                },
                Ok(control_subscribed(splint_id, owned)),
                Vec::new(),
            );
            for (expected, reply, events) in std::iter::once(initial).chain(steps) {
                let frame = read_client(&mut reader).await;
                let ClientFrame::Request {
                    request_id,
                    request,
                    ..
                } = frame
                else {
                    panic!("expected request");
                };
                assert_eq!(request, expected);
                for event in events {
                    send_frame(&mut writer, event).await;
                }
                let response = match reply {
                    Ok(result) => ServerFrame::Response { request_id, result },
                    Err(code) => ServerFrame::Error {
                        request_id: Some(request_id),
                        error: ProtocolError::new(code, "scripted rejection"),
                    },
                };
                send_frame(&mut writer, response).await;
            }
            // Do not turn successful script completion into an unrelated EOF.
            let _ = finish.await;
        });
        let (reader, writer) = tokio::io::split(client);
        let connection = Connection::connect_remote_interactive_transport(reader, writer)
            .await
            .unwrap();
        let (sender, receiver) = mpsc::channel(32);
        for command in commands {
            sender.send(command).await.unwrap();
        }
        drop(sender);
        let (updates, mut update_receiver) = mpsc::channel(32);
        let (resyncs, mut resync_receiver) = mpsc::channel(32);
        let result = run_controller(
            connection,
            receiver,
            ControllerOutputs { updates, resyncs },
            owned.then_some(1),
            splint_id,
            7,
            ForcedControlTransfer::Disabled,
            0,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        let _ = finished.send(());
        server.await.unwrap();
        let mut updates = Vec::new();
        while let Ok(update) = update_receiver.try_recv() {
            updates.push(update);
        }
        let mut resyncs = 0;
        while resync_receiver.try_recv().is_ok() {
            resyncs += 1;
        }
        (result, updates, resyncs)
    })
    .await
    .expect("controller protocol case must finish without hanging")
}

#[tokio::test]
async fn release_then_request_reacquires_and_keeps_controller_loop_alive() {
    let id = SplintId::new();
    let (result, updates, resyncs) = run_case(
        id,
        true,
        vec![
            WindowCommand::ReleaseControl,
            WindowCommand::RequestControlTransfer,
            WindowCommand::Resynchronize,
        ],
        vec![
            (
                Request::ReleaseControl { controller_id: 1 },
                Ok(Response::Acknowledged),
            ),
            (acquire(id), Ok(granted())),
            (
                Request::ReleaseControl { controller_id: 2 },
                Ok(Response::Acknowledged),
            ),
        ],
    )
    .await;
    result.unwrap();
    assert_eq!(resyncs, 1);
    let control: Vec<_> = updates
        .into_iter()
        .filter_map(|u| {
            if let WindowUpdate::Control(active) = u {
                Some(active)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(control, vec![true, false, true]);
}

#[tokio::test]
async fn reacquire_applies_prepared_resize_before_input_and_releases_new_lease() {
    let id = SplintId::new();
    let ack = || {
        Ok(Response::TerminalActionAcknowledged {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            splint_id: id,
            incarnation: 7,
            terminal_revision: 10,
            history_generation: 0,
        })
    };
    let (result, _, _) = run_case(
        id,
        false,
        vec![
            WindowCommand::PrepareResize {
                columns: 90,
                rows: 30,
                pixel_width: 900,
                pixel_height: 600,
            },
            WindowCommand::RequestControlTransfer,
            WindowCommand::Input(b"marker".to_vec()),
        ],
        vec![
            (acquire(id), Ok(granted())),
            (
                Request::Resize {
                    controller_id: 2,
                    splint_id: id,
                    incarnation: 7,
                    columns: 90,
                    rows: 30,
                    pixel_width: 900,
                    pixel_height: 600,
                },
                ack(),
            ),
            (
                Request::Input {
                    controller_id: 2,
                    splint_id: id,
                    incarnation: 7,
                    bytes: b"marker".to_vec(),
                    focus_report: false,
                },
                ack(),
            ),
            (
                Request::ReleaseControl { controller_id: 2 },
                Ok(Response::Acknowledged),
            ),
        ],
    )
    .await;
    result.unwrap();
}

#[tokio::test]
async fn contested_request_uses_consent_and_duplicate_pending_is_nonfatal() {
    let id = SplintId::new();
    let (result, updates, resyncs) = run_case(
        id,
        false,
        vec![
            WindowCommand::RequestControlTransfer,
            WindowCommand::RequestControlTransfer,
            WindowCommand::Resynchronize,
        ],
        vec![
            (acquire(id), Err(ErrorCode::ControllerUnavailable)),
            (
                transfer(id),
                Ok(Response::ControlTransferPending {
                    transfer_id: 3,
                    lair_id: LairId::new(),
                    dojo_id: DojoId::new(),
                }),
            ),
            (acquire(id), Err(ErrorCode::ControllerUnavailable)),
            (transfer(id), Err(ErrorCode::ControlTransferUnavailable)),
        ],
    )
    .await;
    result.unwrap();
    assert_eq!(resyncs, 1);
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, WindowUpdate::Control(true)))
    );
}

#[tokio::test]
async fn owner_release_race_is_nonfatal_and_later_request_can_acquire() {
    let id = SplintId::new();
    let (result, updates, resyncs) = run_case(
        id,
        false,
        vec![
            WindowCommand::RequestControlTransfer,
            WindowCommand::Resynchronize,
            WindowCommand::RequestControlTransfer,
        ],
        vec![
            (acquire(id), Err(ErrorCode::ControllerUnavailable)),
            (transfer(id), Err(ErrorCode::ControlTransferUnavailable)),
            (acquire(id), Ok(granted())),
            (
                Request::ReleaseControl { controller_id: 2 },
                Ok(Response::Acknowledged),
            ),
        ],
    )
    .await;
    result.unwrap();
    assert_eq!(resyncs, 1);
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, WindowUpdate::Control(true)))
    );
}

#[tokio::test]
async fn denied_control_request_remains_observing_without_retry_or_takeover() {
    let id = SplintId::new();
    let (result, updates, resyncs) = run_case(
        id,
        false,
        vec![
            WindowCommand::RequestControlTransfer,
            WindowCommand::Resynchronize,
        ],
        vec![
            (acquire(id), Err(ErrorCode::Unauthorized)),
            (transfer(id), Err(ErrorCode::Unauthorized)),
        ],
    )
    .await;
    result.unwrap();
    assert_eq!(resyncs, 1);
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, WindowUpdate::Control(true)))
    );
}

#[tokio::test]
async fn already_owned_control_request_is_a_noop() {
    let id = SplintId::new();
    let (result, _, resyncs) = run_case(
        id,
        true,
        vec![
            WindowCommand::RequestControlTransfer,
            WindowCommand::Resynchronize,
        ],
        vec![(
            Request::ReleaseControl { controller_id: 1 },
            Ok(Response::Acknowledged),
        )],
    )
    .await;
    result.unwrap();
    assert_eq!(resyncs, 1);
}

#[tokio::test]
async fn unexpected_control_errors_and_responses_are_still_fatal() {
    for transfer_reply in [Err(ErrorCode::InvalidArgument), Ok(Response::Acknowledged)] {
        let id = SplintId::new();
        let (result, _, resyncs) = run_case(
            id,
            false,
            vec![
                WindowCommand::RequestControlTransfer,
                WindowCommand::Resynchronize,
            ],
            vec![
                (acquire(id), Err(ErrorCode::ControllerUnavailable)),
                (transfer(id), transfer_reply),
            ],
        )
        .await;
        assert!(result.is_err());
        assert_eq!(resyncs, 0);
    }
}

fn controlled_input(id: SplintId, bytes: &[u8], focus_report: bool) -> Request {
    Request::Input {
        controller_id: 1,
        splint_id: id,
        incarnation: 7,
        bytes: bytes.to_vec(),
        focus_report,
    }
}

#[tokio::test]
async fn generated_focus_reports_are_flagged_and_typed_bytes_are_not() {
    let id = SplintId::new();
    let (result, _, _) = run_case(
        id,
        true,
        vec![
            WindowCommand::FocusReport(true),
            // The same bytes typed or pasted are ordinary input.
            WindowCommand::Input(b"\x1b[I".to_vec()),
            WindowCommand::FocusReport(false),
        ],
        vec![
            (
                controlled_input(id, b"\x1b[I", true),
                Ok(input_acknowledged(id)),
            ),
            (
                controlled_input(id, b"\x1b[I", false),
                Ok(input_acknowledged(id)),
            ),
            (
                controlled_input(id, b"\x1b[O", true),
                Ok(input_acknowledged(id)),
            ),
            (
                Request::ReleaseControl { controller_id: 1 },
                Ok(Response::Acknowledged),
            ),
        ],
    )
    .await;
    result.unwrap();
}

#[tokio::test]
async fn queued_input_is_coalesced_between_barriers_into_fewer_requests() {
    let id = SplintId::new();
    let (result, _, _) = run_case(
        id,
        true,
        vec![
            WindowCommand::Input(b"ab".to_vec()),
            WindowCommand::Input(b"c".to_vec()),
            WindowCommand::Input(b"d".to_vec()),
            WindowCommand::Resize {
                columns: 90,
                rows: 30,
                pixel_width: 900,
                pixel_height: 600,
            },
            WindowCommand::Input(b"e".to_vec()),
            WindowCommand::Input(Vec::new()),
            WindowCommand::Input(b"f".to_vec()),
            // A generated focus report is a barrier and keeps its flag.
            WindowCommand::FocusReport(true),
            WindowCommand::Input(b"g".to_vec()),
        ],
        vec![
            (
                controlled_input(id, b"abcd", false),
                Ok(input_acknowledged(id)),
            ),
            (
                Request::Resize {
                    controller_id: 1,
                    splint_id: id,
                    incarnation: 7,
                    columns: 90,
                    rows: 30,
                    pixel_width: 900,
                    pixel_height: 600,
                },
                Ok(input_acknowledged(id)),
            ),
            (
                controlled_input(id, b"ef", false),
                Ok(input_acknowledged(id)),
            ),
            (
                controlled_input(id, b"\x1b[I", true),
                Ok(input_acknowledged(id)),
            ),
            (
                controlled_input(id, b"g", false),
                Ok(input_acknowledged(id)),
            ),
            (
                Request::ReleaseControl { controller_id: 1 },
                Ok(Response::Acknowledged),
            ),
        ],
    )
    .await;
    result.unwrap();
}

#[tokio::test]
async fn control_loss_between_coalesced_input_batches_is_observed_before_the_next_batch() {
    let id = SplintId::new();
    let limits = ServerLimits {
        maximum_input_bytes: 8,
        ..ServerLimits::default()
    };
    let lost = ServerFrame::Event {
        subscription_id: 9,
        sequence: 1,
        event: SubscriptionEvent::ControlStatusChanged {
            status: ControlStatus {
                splint_id: id,
                incarnation: 7,
                controlled: true,
                locally_owned: false,
            },
        },
    };
    // Each input exceeds half the bound, so the second is a coalescing barrier.
    // Control is lost while the first round trip is in flight. The barrier must
    // wait for that event and reacquire, never reuse the stale controller 1.
    let (result, updates, _) = run_case_with(
        id,
        true,
        limits,
        vec![
            WindowCommand::Input(b"abcde".to_vec()),
            WindowCommand::Input(b"fghij".to_vec()),
        ],
        vec![
            (
                Request::Input {
                    controller_id: 1,
                    splint_id: id,
                    incarnation: 7,
                    bytes: b"abcde".to_vec(),
                    focus_report: false,
                },
                Ok(input_acknowledged(id)),
                vec![lost],
            ),
            (
                acquire(id),
                Err(ErrorCode::ControllerUnavailable),
                Vec::new(),
            ),
        ],
    )
    .await;
    result.unwrap();
    let control: Vec<_> = updates
        .into_iter()
        .filter_map(|update| match update {
            WindowUpdate::Control(active) => Some(active),
            _ => None,
        })
        .collect();
    assert_eq!(control, vec![true, false, false]);
}

#[tokio::test]
async fn cancellation_stops_a_chain_of_coalescing_barriers() {
    let id = SplintId::new();
    let limits = ServerLimits {
        maximum_input_bytes: 8,
        ..ServerLimits::default()
    };
    let (client, server) = tokio::io::duplex(65536);
    let cancellation = tokio_util::sync::CancellationToken::new();
    let server_cancellation = cancellation.clone();
    let server = tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(server);
        read_client(&mut reader).await;
        send_frame(&mut writer, server_hello(limits)).await;
        let mut inputs = 0;
        loop {
            let ClientFrame::Request {
                request_id,
                request,
                ..
            } = read_client(&mut reader).await
            else {
                panic!("expected request");
            };
            let result = match request {
                Request::SubscribeControl { .. } => control_subscribed(id, true),
                Request::Input { .. } => {
                    inputs += 1;
                    // The pane closes while the first batch is in flight.
                    server_cancellation.cancel();
                    input_acknowledged(id)
                }
                Request::ReleaseControl { .. } => {
                    send_frame(
                        &mut writer,
                        ServerFrame::Response {
                            request_id,
                            result: Response::Acknowledged,
                        },
                    )
                    .await;
                    return inputs;
                }
                other => panic!("unexpected request {other:?}"),
            };
            send_frame(&mut writer, ServerFrame::Response { request_id, result }).await;
        }
    });
    let (reader, writer) = tokio::io::split(client);
    let connection = Connection::connect_remote_interactive_transport(reader, writer)
        .await
        .unwrap();
    let (sender, receiver) = mpsc::channel(32);
    for bytes in [b"abcde", b"fghij", b"klmno"] {
        sender
            .send(WindowCommand::Input(bytes.to_vec()))
            .await
            .unwrap();
    }
    let (updates, _update_receiver) = mpsc::channel(32);
    let (resyncs, _resync_receiver) = mpsc::channel(32);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        run_controller(
            connection,
            receiver,
            ControllerOutputs { updates, resyncs },
            Some(1),
            id,
            7,
            ForcedControlTransfer::Disabled,
            0,
            cancellation,
        ),
    )
    .await
    .expect("cancelled controller must not keep draining queued input");
    result.unwrap();
    drop(sender);
    assert_eq!(
        server.await.unwrap(),
        1,
        "only the in-flight batch was sent"
    );
}
