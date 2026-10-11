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

async fn run_case(
    splint_id: SplintId,
    owned: bool,
    commands: Vec<WindowCommand>,
    steps: Vec<(Request, Reply)>,
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
            send_frame(
                &mut writer,
                ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                },
            )
            .await;
            let initial = (
                Request::SubscribeControl {
                    splint_id,
                    incarnation: 7,
                },
                Ok(Response::ControlSubscribed {
                    subscription_id: 9,
                    status: ControlStatus {
                        splint_id,
                        incarnation: 7,
                        controlled: owned,
                        locally_owned: owned,
                    },
                }),
            );
            for (expected, reply) in std::iter::once(initial).chain(steps) {
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
