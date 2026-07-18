use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use splinterm_core::{LayoutNode, SplintId};
use splinterm_protocol::{
    ClientFrame, ColorSource, MAX_FRAME_BYTES, PROTOCOL_VERSION, Request, Response, ServerFrame,
    SubscriptionEvent, TerminalSnapshot, encode_frame,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time,
};

const DAEMON: &str = env!("CARGO_BIN_EXE_splinterd");
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

struct Daemon {
    child: Child,
    runtime: PathBuf,
    socket: PathBuf,
}

impl Daemon {
    async fn start() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let runtime =
            std::env::temp_dir().join(format!("splinterm-phase8-{}-{nonce}", std::process::id()));
        fs::create_dir(&runtime).unwrap();
        let socket = runtime.join("splinterd.sock");
        let child = Command::new(DAEMON)
            .env("SPLINTERM_SOCKET", &socket)
            .env("SPLINTERM_ENABLE_DEV_ATTACH", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "daemon socket did not appear");
            time::sleep(Duration::from_millis(10)).await;
        }
        Self {
            child,
            runtime,
            socket,
        }
    }

    async fn connect(&self) -> Connection {
        Connection::connect(&self.socket).await
    }

    fn shutdown(mut self) {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
        let status = self.child.wait().unwrap();
        assert!(status.success(), "daemon exited as {status:?}");
        assert!(!self.socket.exists());
        fs::remove_dir(&self.runtime).unwrap();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = fs::remove_dir_all(&self.runtime);
    }
}

struct Connection {
    stream: UnixStream,
    request_id: u64,
    controller_id: Option<u64>,
}

impl Connection {
    async fn connect(socket: &Path) -> Self {
        let mut stream = UnixStream::connect(socket).await.unwrap();
        write_frame(
            &mut stream,
            &ClientFrame::Hello {
                minimum_version: PROTOCOL_VERSION,
                maximum_version: PROTOCOL_VERSION,
            },
        )
        .await;
        assert!(matches!(
            read_frame(&mut stream).await,
            ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                development_terminal_access: true,
                ..
            }
        ));
        Self {
            stream,
            request_id: 1,
            controller_id: None,
        }
    }

    async fn request(&mut self, request: Request) -> Response {
        let request_id = self.request_id;
        self.request_id += 1;
        write_frame(
            &mut self.stream,
            &ClientFrame::Request {
                request_id,
                request,
            },
        )
        .await;
        loop {
            match read_frame(&mut self.stream).await {
                ServerFrame::Response {
                    request_id: response_id,
                    result,
                } if response_id == request_id => return result,
                ServerFrame::Error {
                    request_id: Some(response_id),
                    error,
                } if response_id == request_id => {
                    panic!("request failed with {:?}: {}", error.code, error.message)
                }
                ServerFrame::Event { .. } => {}
                frame => panic!("unexpected response: {frame:?}"),
            }
        }
    }

    async fn live_identity(&mut self) -> (SplintId, u64) {
        match self.request(Request::InspectLiveSplint).await {
            Response::LiveSplint {
                splint_id,
                incarnation,
            } => (splint_id, incarnation),
            response => panic!("unexpected live identity response: {response:?}"),
        }
    }

    async fn acquire_control(&mut self, splint_id: SplintId, incarnation: u64) -> u64 {
        if let Some(controller_id) = self.controller_id {
            return controller_id;
        }
        let Response::ControlGranted { controller_id } = self
            .request(Request::AcquireControl {
                splint_id,
                incarnation,
            })
            .await
        else {
            panic!("control was not granted");
        };
        self.controller_id = Some(controller_id);
        controller_id
    }

    async fn release_control(&mut self) {
        let controller_id = self.controller_id.take().expect("controller owned");
        assert_eq!(
            self.request(Request::ReleaseControl { controller_id })
                .await,
            Response::Acknowledged
        );
    }

    async fn input(&mut self, splint_id: SplintId, incarnation: u64, bytes: &[u8]) {
        let controller_id = self.acquire_control(splint_id, incarnation).await;
        assert_eq!(
            self.request(Request::Input {
                controller_id,
                splint_id,
                incarnation,
                bytes: bytes.to_vec(),
            })
            .await,
            Response::Acknowledged
        );
    }

    async fn attach(&mut self, splint_id: SplintId, incarnation: u64) -> (u64, TerminalSnapshot) {
        match self
            .request(Request::Attach {
                splint_id,
                incarnation,
                scrollback_rows: 16,
            })
            .await
        {
            Response::Attached {
                subscription_id,
                snapshot,
            } => (subscription_id, snapshot),
            response => panic!("unexpected attach response: {response:?}"),
        }
    }
}

async fn write_frame(stream: &mut UnixStream, frame: &ClientFrame) {
    stream
        .write_all(&encode_frame(frame).unwrap())
        .await
        .unwrap();
}

async fn read_frame(stream: &mut UnixStream) -> ServerFrame {
    let mut length = [0_u8; 4];
    time::timeout(TEST_TIMEOUT, stream.read_exact(&mut length))
        .await
        .expect("timed out reading frame length")
        .unwrap();
    let length = u32::from_be_bytes(length) as usize;
    assert!((1..=MAX_FRAME_BYTES).contains(&length));
    let mut body = vec![0_u8; length];
    time::timeout(TEST_TIMEOUT, stream.read_exact(&mut body))
        .await
        .expect("timed out reading frame body")
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn snapshot_text(snapshot: &TerminalSnapshot) -> String {
    snapshot
        .scrollback_rows
        .iter()
        .chain(&snapshot.visible_rows)
        .flat_map(|row| row.cells.iter())
        .map(|cell| cell.content.as_str())
        .collect()
}

async fn snapshot_until(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
    marker: &str,
) -> TerminalSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (subscription_id, snapshot) = connection.attach(splint_id, incarnation).await;
        assert_eq!(
            connection
                .request(Request::Detach { subscription_id })
                .await,
            Response::Acknowledged
        );
        if snapshot_text(&snapshot).contains(marker) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "snapshot never contained {marker}"
        );
        time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "the single scenario intentionally records the complete Phase 8 lifecycle"
)]
async fn phase8_detach_reattach_overflow_resync_and_cleanup() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let mut creator = daemon.connect().await;
        let cwd = std::env::current_dir().unwrap();
        let dojo = match creator
            .request(Request::CreateDojo {
                name: "phase8".into(),
                cwd: cwd.clone(),
            })
            .await
        {
            Response::DojoCreated { dojo } => dojo,
            response => panic!("unexpected create response: {response:?}"),
        };
        let LayoutNode::Leaf(model_splint) = &dojo.windows[0].root else {
            unreachable!()
        };
        let (splint_id, incarnation) = creator.live_identity().await;
        assert_eq!(splint_id, model_splint.id);

        time::sleep(Duration::from_millis(200)).await;
        creator
            .input(
                splint_id,
                incarnation,
                b"clear\nprintf '\\033[31mRED\\033[0m phase8-initial\\n'; pwd\n",
            )
            .await;
        snapshot_until(&mut creator, splint_id, incarnation, "phase8-initial").await;
        let with_pwd = snapshot_until(
            &mut creator,
            splint_id,
            incarnation,
            cwd.to_str().unwrap(),
        )
        .await;
        assert!(snapshot_text(&with_pwd).contains(cwd.to_str().unwrap()));
        assert!(with_pwd
            .visible_rows
            .iter()
            .chain(&with_pwd.scrollback_rows)
            .flat_map(|row| &row.cells)
            .any(|cell| {
                cell.content == "R"
                    && cell.attributes.foreground_source != ColorSource::Default
            }));

        let creator_controller = creator.acquire_control(splint_id, incarnation).await;
        assert_eq!(
            creator
                .request(Request::Resize {
                    controller_id: creator_controller,
                    splint_id,
                    incarnation,
                    columns: 100,
                    rows: 30,
                    pixel_width: 800,
                    pixel_height: 600,
                })
                .await,
            Response::Acknowledged
        );
        let resized = snapshot_until(&mut creator, splint_id, incarnation, "phase8-initial").await;
        assert_eq!((resized.columns, resized.rows), (100, 30));

        drop(creator);
        let mut detached_writer = daemon.connect().await;
        detached_writer
            .input(
                splint_id,
                incarnation,
                b"printf 'while-detached\\n'\n",
            )
            .await;
        drop(detached_writer);
        time::sleep(Duration::from_millis(100)).await;

        let mut reattached = daemon.connect().await;
        let detached = snapshot_until(&mut reattached, splint_id, incarnation, "while-detached").await;
        assert!(detached.revision > resized.revision);

        let reattached_controller = reattached.acquire_control(splint_id, incarnation).await;
        assert_eq!(
            reattached
                .request(Request::Resize {
                    controller_id: reattached_controller,
                    splint_id,
                    incarnation,
                    columns: 40,
                    rows: 10,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .await,
            Response::Acknowledged
        );
        reattached.release_control().await;
        let mut slow = daemon.connect().await;
        let (_subscription_id, _) = slow.attach(splint_id, incarnation).await;
        let mut producer = daemon.connect().await;
        producer
            .input(
                splint_id,
                incarnation,
                b"i=0; while [ $i -lt 300 ]; do printf 'overflow-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' $i; i=$((i+1)); done; printf 'overflow-finished\\n'\n",
            )
            .await;

        let mut saw_resync = false;
        for _ in 0..128 {
            if let ServerFrame::Event {
                event: SubscriptionEvent::ResyncRequired { .. },
                ..
            } = read_frame(&mut slow.stream).await
            {
                saw_resync = true;
                break;
            }
        }
        assert!(saw_resync, "slow subscriber was not forced to resynchronize");

        let final_snapshot = snapshot_until(
            &mut reattached,
            splint_id,
            incarnation,
            "overflow-finished",
        )
        .await;
        assert!(final_snapshot.revision > detached.revision);

        match reattached
            .request(Request::Terminate {
                splint_id,
                incarnation,
            })
            .await
        {
            Response::Terminated { code, signal } => {
                assert!(code.is_some() || signal.is_some());
            }
            response => panic!("unexpected terminate response: {response:?}"),
        }
        daemon.shutdown();
    })
    .await
    .expect("Phase 8 scenario timed out");
}
