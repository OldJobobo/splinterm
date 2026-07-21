use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use splinterm_core::{Axis, LayoutNode, SplintId, SplitRatio, SplitSide, TopologyRevision};
use splinterm_protocol::{
    AccessScope, ClientFrame, ClientRole, ColorSource, ControlTransferDecision,
    ControlTransferOutcome, ErrorCode, LaunchParameters, MAX_FRAME_BYTES, MAX_SUBSCRIPTIONS,
    PROTOCOL_VERSION, ProtocolError, Request, Response, ServerFrame, SplintLifecycle,
    SubscriptionEvent, TerminalSnapshot, TopologyChangeKind, encode_frame,
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
    policy: Option<PathBuf>,
    development_terminal_access: bool,
}

impl Daemon {
    fn spawn_child(runtime: &Path, socket: &Path) -> Child {
        Self::spawn_configured(runtime, socket, None, true)
    }

    fn spawn_configured(
        runtime: &Path,
        socket: &Path,
        policy: Option<&Path>,
        development_terminal_access: bool,
    ) -> Child {
        let stderr = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(runtime.join("daemon.stderr"))
            .unwrap();
        let mut command = Command::new(DAEMON);
        command
            .env("SPLINTERM_SOCKET", socket)
            .env("XDG_STATE_HOME", runtime.join("state"))
            .env("SPLINTERM_DOJO_ID", "caller-supplied-dojo")
            .env("SPLINTERM_WINDOW_ID", "caller-supplied-window")
            .env("SPLINTERM_SPLINT_ID", "caller-supplied-splint")
            .env(
                "SPLINTERM_SPLINT_INCARNATION",
                "caller-supplied-incarnation",
            )
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr);
        if development_terminal_access {
            command.env("SPLINTERM_ENABLE_DEV_ATTACH", "1");
        } else {
            command.env_remove("SPLINTERM_ENABLE_DEV_ATTACH");
        }
        if let Some(policy) = policy {
            command.env("SPLINTERM_POLICY", policy);
        }
        command.spawn().unwrap()
    }

    fn assert_success(&self, status: std::process::ExitStatus) {
        if status.success() {
            return;
        }
        let stderr = fs::read(self.runtime.join("daemon.stderr")).unwrap_or_default();
        let tail = &stderr[stderr.len().saturating_sub(8 * 1024)..];
        panic!(
            "daemon exited as {status:?}; bounded stderr tail:\n{}",
            String::from_utf8_lossy(tail)
        );
    }

    async fn wait_until_ready(socket: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "daemon socket did not appear");
            time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn start() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let runtime =
            std::env::temp_dir().join(format!("splinterm-phase8-{}-{nonce}", std::process::id()));
        fs::create_dir(&runtime).unwrap();
        let socket = runtime.join("splinterd.sock");
        let child = Self::spawn_child(&runtime, &socket);
        Self::wait_until_ready(&socket).await;
        Self {
            child,
            runtime,
            socket,
            policy: None,
            development_terminal_access: true,
        }
    }

    async fn start_with_policy(policy_contents: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let runtime = std::env::temp_dir().join(format!(
            "splinterm-headless-policy-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&runtime).unwrap();
        let policy = runtime.join("policy.json");
        fs::write(&policy, policy_contents).unwrap();
        fs::set_permissions(&policy, fs::Permissions::from_mode(0o600)).unwrap();
        let socket = runtime.join("splinterd.sock");
        let child = Self::spawn_configured(&runtime, &socket, Some(&policy), false);
        Self::wait_until_ready(&socket).await;
        Self {
            child,
            runtime,
            socket,
            policy: Some(policy),
            development_terminal_access: false,
        }
    }

    async fn connect(&self) -> Connection {
        Connection::connect(&self.socket).await
    }

    fn stop_preserving_state(&mut self) {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
        let status = self.child.wait().unwrap();
        self.assert_success(status);
        assert!(!self.socket.exists());
    }

    async fn start_again(&mut self) {
        self.child = Self::spawn_configured(
            &self.runtime,
            &self.socket,
            self.policy.as_deref(),
            self.development_terminal_access,
        );
        Self::wait_until_ready(&self.socket).await;
    }

    fn reload_policy(&self) {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::HUP).unwrap();
    }

    fn shutdown(mut self) {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
        let status = self.child.wait().unwrap();
        self.assert_success(status);
        assert!(!self.socket.exists());
        fs::remove_dir_all(&self.runtime).unwrap();
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
                role: ClientRole::Automation,
            },
        )
        .await;
        assert!(matches!(
            read_frame(&mut stream).await,
            ServerFrame::Hello {
                version: PROTOCOL_VERSION,
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
        self.request_result(request).await.unwrap_or_else(|error| {
            panic!("request failed with {:?}: {}", error.code, error.message)
        })
    }

    async fn topology_revision(&mut self) -> splinterm_core::TopologyRevision {
        let Response::Topology { snapshot } = self.request(Request::InspectTopology).await else {
            panic!("topology response was not returned");
        };
        snapshot.revision
    }

    async fn request_result(&mut self, request: Request) -> Result<Response, ProtocolError> {
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
                } if response_id == request_id => return Ok(result),
                ServerFrame::Error {
                    request_id: Some(response_id),
                    error,
                } if response_id == request_id => return Err(error),
                ServerFrame::Event { .. } => {}
                frame => panic!("unexpected response: {frame:?}"),
            }
        }
    }

    async fn next_event(&mut self, subscription_id: u64) -> (u64, SubscriptionEvent) {
        loop {
            match read_frame(&mut self.stream).await {
                ServerFrame::Event {
                    subscription_id: event_subscription,
                    sequence,
                    event,
                } if event_subscription == subscription_id => return (sequence, event),
                ServerFrame::Event { .. } => {}
                frame => panic!("unexpected subscription frame: {frame:?}"),
            }
        }
    }

    async fn live_incarnation(&mut self, splint_id: SplintId) -> u64 {
        match self.request(Request::InspectSplint { splint_id }).await {
            Response::Splint { runtime } if runtime.splint_id == splint_id => runtime
                .live_incarnation
                .expect("created Splint must have a live incarnation"),
            response => panic!("unexpected targeted identity response: {response:?}"),
        }
    }

    async fn subscribe_control(&mut self, splint_id: SplintId, incarnation: u64) -> u64 {
        match self
            .request(Request::SubscribeControl {
                splint_id,
                incarnation,
            })
            .await
        {
            Response::ControlSubscribed {
                subscription_id,
                status,
            } => {
                status.validate().unwrap();
                subscription_id
            }
            response => panic!("unexpected control subscription response: {response:?}"),
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
        assert!(matches!(
            self.request(Request::Input {
                controller_id,
                splint_id,
                incarnation,
                bytes: bytes.to_vec(),
            })
            .await,
            Response::TerminalActionAcknowledged {
                splint_id: response_id,
                incarnation: response_incarnation,
                ..
            } if response_id == splint_id && response_incarnation == incarnation
        ));
    }

    async fn resize(&mut self, splint_id: SplintId, incarnation: u64, columns: u16, rows: u16) {
        let controller_id = self.acquire_control(splint_id, incarnation).await;
        assert!(matches!(
            self.request(Request::Resize {
                controller_id,
                splint_id,
                incarnation,
                columns,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            })
            .await,
            Response::TerminalActionAcknowledged {
                splint_id: response_id,
                incarnation: response_incarnation,
                ..
            } if response_id == splint_id && response_incarnation == incarnation
        ));
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

async fn read_frame_or_eof(stream: &mut UnixStream) -> Option<ServerFrame> {
    let mut length = [0_u8; 4];
    match time::timeout(TEST_TIMEOUT, stream.read_exact(&mut length))
        .await
        .expect("timed out reading frame length")
    {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return None,
        Err(error) => panic!("failed reading frame length: {error}"),
    }
    let length = u32::from_be_bytes(length) as usize;
    assert!((1..=MAX_FRAME_BYTES).contains(&length));
    let mut body = vec![0_u8; length];
    match time::timeout(TEST_TIMEOUT, stream.read_exact(&mut body))
        .await
        .expect("timed out reading frame body")
    {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return None,
        Err(error) => panic!("failed reading frame body: {error}"),
    }
    Some(serde_json::from_slice(&body).unwrap())
}

async fn read_frame(stream: &mut UnixStream) -> ServerFrame {
    read_frame_or_eof(stream)
        .await
        .expect("connection closed before the expected frame")
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
    let deadline = Instant::now() + Duration::from_secs(10);
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

fn policy_executable_identity() -> (PathBuf, String) {
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let mut file = fs::File::open(&executable).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    (executable, sha256)
}

fn exact_headless_policy(splint: Option<(SplintId, u64)>) -> String {
    let (executable, sha256) = policy_executable_identity();
    let mut scopes = vec![
        "topology_metadata_read",
        "process_spawn",
        "topology_layout_mutate",
        "audit_inspect",
        "topology_subscribe",
    ];
    let mut resources = vec![serde_json::json!({"kind": "lair"})];
    if let Some((splint_id, incarnation)) = splint {
        scopes.extend([
            "terminal_visible_read",
            "terminal_subscribe",
            "controller_acquire",
            "input",
        ]);
        resources.push(serde_json::json!({
            "kind": "splint",
            "splint_id": splint_id,
            "incarnation": incarnation
        }));
    }
    serde_json::json!({
        "schema": "splinterm.policy.v1",
        "rules": [{
            "id": "headless-test",
            "executable": {"path": executable, "sha256": sha256},
            "scopes": scopes,
            "resources": resources,
            "limits": {
                "max_spawn_count": 1,
                "max_results": 16,
                "max_live_subscriptions": 2
            }
        }]
    })
    .to_string()
}

fn parent_snapshot_policy(dojo_id: splinterm_core::DojoId) -> String {
    let (executable, sha256) = policy_executable_identity();
    serde_json::json!({
        "schema": "splinterm.policy.v1",
        "rules": [{
            "id": "parent-snapshot-test",
            "executable": {"path": executable, "sha256": sha256},
            "scopes": [
                "topology_metadata_read",
                "process_spawn",
                "topology_layout_mutate",
                "terminal_visible_read",
                "terminal_subscribe",
                "scrollback_read"
            ],
            "resources": [
                {"kind": "lair"},
                {"kind": "dojo", "dojo_id": dojo_id}
            ],
            "limits": {
                "max_spawn_count": 2,
                "max_results": 16,
                "max_returned_rows": 16,
                "max_live_subscriptions": 1
            }
        }]
    })
    .to_string()
}

async fn assert_connection_closed(connection: &mut Connection, reason: &str) {
    let mut byte = [0_u8; 1];
    let closed = time::timeout(Duration::from_secs(5), connection.stream.read(&mut byte))
        .await
        .unwrap_or_else(|_| panic!("{reason} did not close the existing client"))
        .unwrap();
    assert_eq!(closed, 0);
}

#[allow(
    clippy::too_many_lines,
    reason = "the ordered policy reload, controller cleanup, restart, and process-reap gate is one lifecycle"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headless_policy_reload_fails_closed_and_cleans_up() {
    time::timeout(TEST_TIMEOUT, async {
        let mut daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let marker = daemon.runtime.join("child-pid");
        let mut connection = daemon.connect().await;
        let revision = connection.topology_revision().await;
        let Response::DojoCreated {
            dojo, incarnation, ..
        } = connection
            .request(Request::CreateDojo {
                expected_topology_revision: revision,
                name: "headless".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        format!("printf '%s\\n' $$ > {}; exec sleep 30", marker.display()),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("headless Dojo was not created");
        };
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            panic!("created headless Dojo was not a leaf");
        };
        let splint_id = splint.id;
        let lair_only_denial = connection
            .request_result(Request::Attach {
                splint_id,
                incarnation,
                scrollback_rows: 0,
            })
            .await
            .expect_err("Lair creation authority must not cover the new Dojo descendant");
        assert_eq!(lair_only_denial.code, ErrorCode::Unauthorized);
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(
                Instant::now() < marker_deadline,
                "child PID marker was not written"
            );
            time::sleep(Duration::from_millis(10)).await;
        }
        let child_pid: u32 = fs::read_to_string(&marker).unwrap().trim().parse().unwrap();

        let Response::AuditPage { page } = connection
            .request(Request::AuditInspect {
                after_audit_id: None,
                max_records: 16,
            })
            .await
        else {
            panic!("audit page was not returned");
        };
        let create_audit = page
            .records
            .iter()
            .find(|record| record.operation == splinterm_protocol::AuditOperation::CreateDojo)
            .expect("authorized create audit record was absent");
        let peer_executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let peer_metadata = fs::metadata(&peer_executable).unwrap();
        let mut peer_bytes = Vec::new();
        fs::File::open(&peer_executable)
            .unwrap()
            .read_to_end(&mut peer_bytes)
            .unwrap();
        assert_eq!(create_audit.schema, "splinterm.audit.v1");
        assert_eq!(create_audit.retention, "daemon_lifetime");
        assert_ne!(create_audit.audit_id, 0);
        assert_ne!(create_audit.policy_generation, Some(0));
        assert_eq!(
            create_audit.policy_rule_id.as_deref(),
            Some("headless-test")
        );
        assert_eq!(create_audit.peer.uid, rustix::process::geteuid().as_raw());
        assert_eq!(
            create_audit.peer.executable_path,
            peer_executable.to_string_lossy()
        );
        assert_eq!(
            create_audit.peer.executable_sha256,
            format!("{:x}", Sha256::digest(peer_bytes))
        );
        assert_eq!(create_audit.peer.device, Some(peer_metadata.dev()));
        assert_eq!(create_audit.peer.inode, Some(peer_metadata.ino()));
        assert_eq!(create_audit.resource, None);
        assert_eq!(
            create_audit.requested_scopes,
            vec![
                splinterm_protocol::AutomationScope::ProcessSpawn,
                splinterm_protocol::AutomationScope::TopologyLayoutMutate,
            ]
        );
        assert_eq!(
            create_audit.decision,
            splinterm_protocol::AuditDecision::Matched
        );
        assert_eq!(create_audit.reason, "policy_match");
        assert_eq!(
            create_audit.outcome,
            Some(splinterm_protocol::AuditOutcome::Succeeded)
        );
        assert_eq!(create_audit.argument_count, Some(2));
        assert_eq!(create_audit.executable_basename.as_deref(), Some("sh"));

        let policy = daemon.policy.as_ref().unwrap();
        fs::write(
            policy,
            exact_headless_policy(Some((splint_id, incarnation))),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut connection, "accepted policy reload").await;

        let mut controlled = daemon.connect().await;
        let topology_subscription = match controlled.request(Request::SubscribeTopology).await {
            Response::TopologySubscribed {
                subscription_id, ..
            } => subscription_id,
            response => panic!("unexpected topology subscription response: {response:?}"),
        };
        assert_ne!(topology_subscription, 0);
        let control_subscription = controlled.subscribe_control(splint_id, incarnation).await;
        assert_ne!(control_subscription, 0);
        let controller_id = controlled.acquire_control(splint_id, incarnation).await;
        assert_ne!(controller_id, 0);
        let consent_error = controlled
            .request_result(Request::RequestAccess {
                splint_id,
                incarnation,
                scopes: vec![AccessScope::Resize],
            })
            .await
            .expect_err("under-scoped headless consent request must fail closed");
        assert_eq!(consent_error.code, ErrorCode::ConsentDenied);

        fs::write(policy, r#"{"schema":"wrong","rules":[]}"#).unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut controlled, "rejected policy reload").await;
        let mut denied = daemon.connect().await;
        assert_eq!(
            denied
                .request_result(Request::InspectTopology)
                .await
                .expect_err("rejected reload must install deny-all policy")
                .code,
            ErrorCode::Unauthorized
        );
        fs::write(
            policy,
            exact_headless_policy(Some((splint_id, incarnation))),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut denied, "accepted recovery policy reload").await;
        let mut reauthorized = daemon.connect().await;
        let replacement_controller = reauthorized.acquire_control(splint_id, incarnation).await;
        assert_ne!(replacement_controller, controller_id);
        drop(reauthorized);

        daemon.stop_preserving_state();
        assert!(!Path::new(&format!("/proc/{child_pid}")).exists());
        let marker_before_restart = fs::read_to_string(&marker).unwrap();
        daemon.start_again().await;
        time::sleep(Duration::from_millis(100)).await;
        assert_eq!(fs::read_to_string(&marker).unwrap(), marker_before_restart);
        let mut restarted = daemon.connect().await;
        let Response::Topology { snapshot } = restarted.request(Request::InspectTopology).await
        else {
            panic!("restarted headless topology was not returned");
        };
        assert!(
            snapshot
                .runtimes
                .iter()
                .all(|runtime| runtime.live_incarnation.is_none())
        );
        let Response::AuditPage { page } = restarted
            .request(Request::AuditInspect {
                after_audit_id: None,
                max_records: 16,
            })
            .await
        else {
            panic!("restarted audit page was not returned");
        };
        assert!(
            page.records.iter().all(|record| {
                record.operation != splinterm_protocol::AuditOperation::CreateDojo
            })
        );
        daemon.shutdown();
    })
    .await
    .expect("headless policy integration timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_policy_snapshot_excludes_new_splint_until_reload() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let mut bootstrap = daemon.connect().await;
        let revision = bootstrap.topology_revision().await;
        let Response::DojoCreated { dojo, .. } = bootstrap
            .request(Request::CreateDojo {
                expected_topology_revision: revision,
                name: "snapshot-policy".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec!["/bin/sh".into(), "-c".into(), "exec sleep 30".into()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("snapshot policy Dojo was not created");
        };
        let dojo_id = dojo.id;
        let LayoutNode::Leaf(original) = &dojo.windows[0].root else {
            panic!("snapshot policy Dojo was not a leaf");
        };
        let original_id = original.id;

        let policy = daemon.policy.as_ref().unwrap();
        fs::write(policy, parent_snapshot_policy(dojo_id)).unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut bootstrap, "parent snapshot policy reload").await;

        let mut connection = daemon.connect().await;
        let revision = connection.topology_revision().await;
        let Response::SplintStarted {
            splint_id,
            incarnation,
            ..
        } = connection
            .request(Request::SplitSplint {
                expected_topology_revision: revision,
                target_splint_id: original_id,
                axis: Axis::Horizontal,
                side: SplitSide::Second,
                ratio: SplitRatio::new(500).unwrap(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec!["/bin/sh".into(), "-c".into(), "exec sleep 30".into()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("snapshot policy split was not created");
        };
        let denied = connection
            .request_result(Request::Attach {
                splint_id,
                incarnation,
                scrollback_rows: 1,
            })
            .await
            .expect_err("new descendant must not inherit the published parent snapshot");
        assert_eq!(denied.code, ErrorCode::Unauthorized);

        fs::write(policy, parent_snapshot_policy(dojo_id)).unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut connection, "explicit descendant policy reload").await;

        let mut refreshed = daemon.connect().await;
        let (subscription_id, _) = refreshed.attach(splint_id, incarnation).await;
        assert_eq!(
            refreshed.request(Request::Detach { subscription_id }).await,
            Response::Acknowledged
        );
        daemon.shutdown();
    })
    .await
    .expect("parent policy snapshot integration timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_reaps_signal_resistant_child_and_removes_socket() {
    time::timeout(Duration::from_secs(80), async {
        let daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let marker = daemon.runtime.join("resistant-child-pid");
        let mut connection = daemon.connect().await;
        let revision = connection.topology_revision().await;
        let Response::DojoCreated { .. } = connection
            .request(Request::CreateDojo {
                expected_topology_revision: revision,
                name: "resistant-shutdown".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        format!(
                            "trap '' HUP TERM; printf '%s\\n' $$ > {}; exec sleep 75",
                            marker.display()
                        ),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("signal-resistant Dojo was not created");
        };
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(
                Instant::now() < marker_deadline,
                "signal-resistant child PID marker was not written"
            );
            time::sleep(Duration::from_millis(10)).await;
        }
        let child_pid: u32 = fs::read_to_string(&marker).unwrap().trim().parse().unwrap();
        drop(connection);

        tokio::task::spawn_blocking(move || daemon.shutdown())
            .await
            .unwrap();
        assert!(!Path::new(&format!("/proc/{child_pid}")).exists());
    })
    .await
    .expect("signal-resistant shutdown integration timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one lifecycle scenario correlates create, restore, and new-window context"
)]
async fn new_window_and_restore_inject_exact_current_context() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let restored_marker = daemon.runtime.join("restored-context");
        let window_marker = daemon.runtime.join("window-context");
        let context_launch = |marker: &Path| LaunchParameters {
            cwd: std::env::current_dir().unwrap(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                format!(
                    "printf '%s|%s|%s|%s' \"$SPLINTERM_DOJO_ID\" \"$SPLINTERM_WINDOW_ID\" \"$SPLINTERM_SPLINT_ID\" \"$SPLINTERM_SPLINT_INCARNATION\" > {}",
                    marker.display()
                ),
            ],
            shell: None,
            login_shell: false,
            scrollback_lines: 100,
        };
        let mut connection = daemon.connect().await;
        let Response::DojoCreated {
            dojo,
            incarnation: first_incarnation,
            ..
        } = connection
            .request(Request::CreateDojo {
                expected_topology_revision: TopologyRevision::default(),
                name: "context-lifecycle".into(),
                launch: context_launch(&restored_marker),
            })
            .await
        else {
            panic!("context test Dojo was not created");
        };
        let dojo_id = dojo.id;
        let first_window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(first) = &dojo.windows[0].root else {
            panic!("context test Dojo was not a leaf");
        };
        let first_id = first.id;
        let expected_first =
            format!("{dojo_id}|{first_window_id}|{first_id}|{first_incarnation}");
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(fs::read_to_string(&restored_marker), Ok(ref value) if value == &expected_first) {
            assert!(Instant::now() < marker_deadline, "initial context marker timed out");
            time::sleep(Duration::from_millis(10)).await;
        }
        loop {
            let Response::Splint { runtime } = connection
                .request(Request::InspectSplint { splint_id: first_id })
                .await
            else {
                panic!("context test Splint was not inspected");
            };
            if matches!(runtime.lifecycle, SplintLifecycle::Exited) {
                break;
            }
            time::sleep(Duration::from_millis(10)).await;
        }

        let revision = connection.topology_revision().await;
        let Response::RestoreCompleted { results, .. } = connection
            .request(Request::RestoreSplint {
                expected_topology_revision: revision,
                splint_id: first_id,
            })
            .await
        else {
            panic!("context test Splint was not restored");
        };
        let restored_incarnation = results[0].incarnation.expect("restore must start process");
        assert_ne!(restored_incarnation, first_incarnation);
        let expected_restored =
            format!("{dojo_id}|{first_window_id}|{first_id}|{restored_incarnation}");
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(fs::read_to_string(&restored_marker), Ok(ref value) if value == &expected_restored) {
            assert!(Instant::now() < marker_deadline, "restored context marker timed out");
            time::sleep(Duration::from_millis(10)).await;
        }

        let revision = connection.topology_revision().await;
        let Response::WindowStarted {
            window_id,
            splint_id,
            incarnation,
            ..
        } = connection
            .request(Request::NewWindow {
                expected_topology_revision: revision,
                dojo_id,
                title: "context-window".into(),
                launch: context_launch(&window_marker),
            })
            .await
        else {
            panic!("context test window was not created");
        };
        let expected_window = format!("{dojo_id}|{window_id}|{splint_id}|{incarnation}");
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(fs::read_to_string(&window_marker), Ok(ref value) if value == &expected_window) {
            assert!(Instant::now() < marker_deadline, "window context marker timed out");
            time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!expected_restored.contains("caller-supplied"));
        assert!(!expected_window.contains("caller-supplied"));
        daemon.shutdown();
    })
    .await
    .expect("context lifecycle scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_restores_ids_without_running_saved_commands() {
    time::timeout(TEST_TIMEOUT, async {
        let mut daemon = Daemon::start().await;
        let marker = daemon.runtime.join("launch-count");
        let launch = LaunchParameters {
            cwd: std::env::current_dir().unwrap(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("printf 'run\\n' >> {}; exec sleep 30", marker.display()),
            ],
            shell: None,
            login_shell: false,
            scrollback_lines: 100,
        };
        let mut connection = daemon.connect().await;
        let Response::DojoCreated { dojo, .. } = connection
            .request(Request::CreateDojo {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "durable".into(),
                launch,
            })
            .await
        else {
            panic!("dojo was not created");
        };
        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            panic!("created dojo was not a leaf");
        };
        let splint_id = splint.id;
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(fs::read_to_string(&marker), Ok(contents) if contents == "run\n") {
            assert!(
                Instant::now() < marker_deadline,
                "launch marker did not contain expected output"
            );
            time::sleep(Duration::from_millis(10)).await;
        }
        drop(connection);

        daemon.stop_preserving_state();
        let primary = daemon.runtime.join("state/splinterm/lair.json");
        fs::write(&primary, b"{truncated").unwrap();
        daemon.start_again().await;
        time::sleep(Duration::from_millis(100)).await;
        assert!(
            fs::read_dir(primary.parent().unwrap())
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("lair.invalid-"))
        );
        assert_eq!(fs::read_to_string(&marker).unwrap(), "run\n");
        let mut restored = daemon.connect().await;
        let Response::Topology { snapshot } = restored.request(Request::InspectTopology).await
        else {
            panic!("restored topology was not returned");
        };
        snapshot.validate().unwrap();
        assert_eq!(snapshot.revision.get(), 1);
        let restored_dojo = snapshot.lair.dojos().next().unwrap();
        assert_eq!(restored_dojo.id, dojo_id);
        assert_eq!(restored_dojo.windows[0].id, window_id);
        let LayoutNode::Leaf(restored_splint) = &restored_dojo.windows[0].root else {
            panic!("restored dojo was not a leaf");
        };
        assert_eq!(restored_splint.id, splint_id);
        assert!(matches!(
            restored_splint.state,
            splinterm_core::SplintState::Exited(_)
        ));
        assert_eq!(snapshot.runtimes[0].live_incarnation, None);

        let expected_topology_revision = snapshot.revision;
        let Response::RestoreCompleted { results, .. } = restored
            .request(Request::RestoreSplint {
                expected_topology_revision,
                splint_id,
            })
            .await
        else {
            panic!("restore result was not returned");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].incarnation.is_some());
        assert!(results[0].error.is_none());
        while fs::read_to_string(&marker).unwrap() != "run\nrun\n" {
            time::sleep(Duration::from_millis(10)).await;
        }
        daemon.shutdown();
    })
    .await
    .expect("restart persistence scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one scenario covers restore-one, restore-window, restore-dojo, and partial results"
)]
async fn explicit_restore_scopes_report_per_leaf_results() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let launch = LaunchParameters {
            cwd: std::env::current_dir().unwrap(),
            command: vec!["/bin/sh".into(), "-c".into(), "exec sleep 30".into()],
            shell: None,
            login_shell: false,
            scrollback_lines: 100,
        };
        let mut connection = daemon.connect().await;
        let Response::DojoCreated { dojo, .. } = connection
            .request(Request::CreateDojo {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "restore-scopes".into(),
                launch: launch.clone(),
            })
            .await
        else {
            panic!("dojo was not created");
        };
        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(first) = &dojo.windows[0].root else {
            panic!("created dojo was not a leaf");
        };
        let first_id = first.id;
        let first_incarnation = connection.live_incarnation(first_id).await;
        let Response::SplintStarted {
            splint_id: second_id,
            incarnation: second_incarnation,
            topology_revision,
        } = connection
            .request(Request::SplitSplint {
                expected_topology_revision: splinterm_core::TopologyRevision::new(1),
                target_splint_id: first_id,
                axis: Axis::Horizontal,
                side: SplitSide::Second,
                ratio: SplitRatio::new(500).unwrap(),
                launch: launch.clone(),
            })
            .await
        else {
            panic!("second Splint was not created");
        };
        let Response::WindowStarted {
            splint_id: third_id,
            incarnation: third_incarnation,
            ..
        } = connection
            .request(Request::NewWindow {
                expected_topology_revision: topology_revision,
                dojo_id,
                title: "second".into(),
                launch,
            })
            .await
        else {
            panic!("second window was not created");
        };
        for (splint_id, incarnation) in [
            (first_id, first_incarnation),
            (second_id, second_incarnation),
            (third_id, third_incarnation),
        ] {
            assert!(matches!(
                connection
                    .request(Request::KillSplint {
                        splint_id,
                        incarnation,
                    })
                    .await,
                Response::SplintKilled { .. }
            ));
        }

        let expected_topology_revision = connection.topology_revision().await;
        let Response::RestoreCompleted { results, .. } = connection
            .request(Request::RestoreSplint {
                expected_topology_revision,
                splint_id: first_id,
            })
            .await
        else {
            panic!("single restore failed");
        };
        assert_eq!(results.len(), 1);
        assert!(results[0].incarnation.is_some());

        let expected_topology_revision = connection.topology_revision().await;
        let Response::RestoreCompleted { results, .. } = connection
            .request(Request::RestoreWindow {
                expected_topology_revision,
                window_id,
            })
            .await
        else {
            panic!("window restore failed");
        };
        assert_eq!(results.len(), 2);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.error.is_some())
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| result.incarnation.is_some())
                .count(),
            1
        );

        let expected_topology_revision = connection.topology_revision().await;
        let Response::RestoreCompleted { results, .. } = connection
            .request(Request::RestoreDojo {
                expected_topology_revision,
                dojo_id,
            })
            .await
        else {
            panic!("dojo restore failed");
        };
        assert_eq!(results.len(), 3);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.error.is_some())
                .count(),
            2
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| result.incarnation.is_some())
                .count(),
            1
        );
        daemon.shutdown();
    })
    .await
    .expect("explicit restore scope scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one scenario proves topology CAS, ordered events, and all headless edit types"
)]
async fn topology_cas_stream_and_complete_edits() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let cwd = std::env::current_dir().unwrap();
        let launch = LaunchParameters {
            cwd: cwd.clone(),
            command: vec!["/bin/sh".into(), "-c".into(), "exec sleep 30".into()],
            shell: None,
            login_shell: false,
            scrollback_lines: 100,
        };
        let mut subscriber = daemon.connect().await;
        let Response::TopologySubscribed {
            subscription_id,
            snapshot,
        } = subscriber.request(Request::SubscribeTopology).await
        else {
            panic!("topology subscription was not created");
        };
        assert_eq!(snapshot.revision.get(), 0);

        let mut first = daemon.connect().await;
        let mut second = daemon.connect().await;
        let first_request = Request::CreateDojo {
            expected_topology_revision: snapshot.revision,
            name: "race-first".into(),
            launch: launch.clone(),
        };
        let second_request = Request::CreateDojo {
            expected_topology_revision: snapshot.revision,
            name: "race-second".into(),
            launch: launch.clone(),
        };
        let (first_result, second_result) = tokio::join!(
            first.request_result(first_request),
            second.request_result(second_request)
        );
        let (dojo, stale) = match (first_result, second_result) {
            (Ok(Response::DojoCreated { dojo, .. }), Err(stale))
            | (Err(stale), Ok(Response::DojoCreated { dojo, .. })) => (dojo, stale),
            results => panic!("exactly one racing create must commit: {results:?}"),
        };
        assert_eq!(stale.code, ErrorCode::StaleTopology);
        assert_eq!(
            stale.current_topology_revision,
            Some(splinterm_core::TopologyRevision::new(1))
        );
        let (sequence, event) = subscriber.next_event(subscription_id).await;
        assert_eq!(sequence, 1);
        let SubscriptionEvent::TopologyChanged { change } = &event else {
            panic!("first topology event was not a change");
        };
        change.validate().unwrap();
        assert!(matches!(
            event,
            SubscriptionEvent::TopologyChanged { change }
                if change.revision.get() == 1 && change.kind == TopologyChangeKind::DojoCreated
        ));

        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(initial) = &dojo.windows[0].root else {
            panic!("created dojo was not a leaf");
        };
        let initial_id = initial.id;
        let mut editor = daemon.connect().await;
        let Response::SplintStarted {
            splint_id: sibling_id,
            incarnation: sibling_incarnation,
            topology_revision,
        } = editor
            .request(Request::SplitSplint {
                expected_topology_revision: splinterm_core::TopologyRevision::new(1),
                target_splint_id: initial_id,
                axis: Axis::Horizontal,
                side: SplitSide::Second,
                ratio: SplitRatio::new(500).unwrap(),
                launch: launch.clone(),
            })
            .await
        else {
            panic!("split did not commit");
        };
        assert_eq!(topology_revision.get(), 2);
        assert!(matches!(
            editor
                .request(Request::SetSplitRatio {
                    expected_topology_revision: topology_revision,
                    target_splint_id: sibling_id,
                    ratio: SplitRatio::new(650).unwrap(),
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 3
        ));
        assert!(matches!(
            editor
                .request(Request::RenameDojo {
                    expected_topology_revision: splinterm_core::TopologyRevision::new(3),
                    dojo_id,
                    name: "renamed-dojo".into(),
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 4
        ));
        assert!(matches!(
            editor
                .request(Request::RenameWindow {
                    expected_topology_revision: splinterm_core::TopologyRevision::new(4),
                    window_id,
                    title: "main-window".into(),
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 5
        ));
        assert!(matches!(
            editor
                .request(Request::RenameSplint {
                    expected_topology_revision: splinterm_core::TopologyRevision::new(5),
                    splint_id: initial_id,
                    title: "primary".into(),
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 6
        ));
        let Response::WindowStarted {
            window_id: extra_window,
            splint_id: extra_splint,
            incarnation: extra_incarnation,
            topology_revision,
        } = editor
            .request(Request::NewWindow {
                expected_topology_revision: splinterm_core::TopologyRevision::new(6),
                dojo_id,
                title: "extra-window".into(),
                launch,
            })
            .await
        else {
            panic!("new window did not commit");
        };
        assert_eq!(topology_revision.get(), 7);
        let invalid_focus = editor
            .request_result(Request::SetWindowDefaultFocus {
                expected_topology_revision: topology_revision,
                window_id,
                splint_id: extra_splint,
            })
            .await
            .unwrap_err();
        assert_eq!(invalid_focus.code, ErrorCode::InvalidArgument);
        let Response::TopologyCommitted { topology_revision } = editor
            .request(Request::SetWindowDefaultFocus {
                expected_topology_revision: topology_revision,
                window_id,
                splint_id: sibling_id,
            })
            .await
        else {
            panic!("window focus hint did not commit");
        };
        assert_eq!(topology_revision.get(), 8);
        assert!(matches!(
            editor
                .request(Request::KillSplint {
                    splint_id: extra_splint,
                    incarnation: extra_incarnation,
                })
                .await,
            Response::SplintKilled { .. }
        ));
        assert!(matches!(
            editor
                .request(Request::CloseWindow {
                    expected_topology_revision: topology_revision,
                    window_id: extra_window,
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 9
        ));

        assert!(matches!(
            editor
                .request(Request::KillSplint {
                    splint_id: sibling_id,
                    incarnation: sibling_incarnation,
                })
                .await,
            Response::SplintKilled { .. }
        ));
        assert!(matches!(
            editor
                .request(Request::CloseSplint {
                    expected_topology_revision: splinterm_core::TopologyRevision::new(9),
                    splint_id: sibling_id,
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 10
        ));
        let Response::Topology { snapshot } = editor.request(Request::InspectTopology).await else {
            panic!("topology inspection failed");
        };
        snapshot.validate().unwrap();
        assert_eq!(snapshot.revision.get(), 10);
        assert_eq!(
            snapshot.lair.find_window(window_id).unwrap().default_focus,
            initial_id
        );
        assert_eq!(snapshot.runtimes.len(), 1);
        daemon.shutdown();
    })
    .await
    .expect("topology CAS scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_scrollback_search_pages_and_invalidates_stale_cursors() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let mut client = daemon.connect().await;
        let Response::DojoCreated { dojo, .. } = client
            .request(Request::CreateDojo {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "search".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec!["/bin/sh".into()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("daemon did not create search Splint");
        };
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            panic!("new dojo was not a leaf");
        };
        let splint_id = splint.id;
        let incarnation = client.live_incarnation(splint_id).await;
        client
            .input(
                splint_id,
                incarnation,
                "printf 'Needle one\\nnoise\\nneedle two 界\\n'\n".as_bytes(),
            )
            .await;
        snapshot_until(&mut client, splint_id, incarnation, "needle two").await;
        time::sleep(Duration::from_millis(50)).await;
        let (_, snapshot) = client.attach(splint_id, incarnation).await;
        let search = |cursor| Request::SearchScrollback {
            splint_id,
            incarnation,
            terminal_revision: snapshot.revision,
            history_generation: snapshot.history_generation,
            query: "NEEDLE".into(),
            case_sensitive: false,
            cursor,
            max_results: 1,
        };
        let Response::SearchResults { page: first } = client.request(search(None)).await else {
            panic!("daemon did not return first search page");
        };
        first.validate().unwrap();
        assert_eq!(first.matches.len(), 1);
        let cursor = first.next_cursor.clone().expect("older match cursor");
        let Response::SearchResults { page: second } = client.request(search(Some(cursor))).await
        else {
            panic!("daemon did not return second search page");
        };
        assert_eq!(second.matches.len(), 1);
        assert_ne!(first.matches[0].row_id, second.matches[0].row_id);

        client
            .input(splint_id, incarnation, b"printf 'revision-change\\n'\n")
            .await;
        snapshot_until(&mut client, splint_id, incarnation, "revision-change").await;
        assert!(matches!(
            client.request(search(first.next_cursor)).await,
            Response::SearchResyncRequired { .. }
        ));
        daemon.shutdown();
    })
    .await
    .expect("search scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one connection-level scenario proves denial, acceptance, stale ownership, input, and disconnect"
)]
async fn simultaneous_clients_transfer_control_explicitly() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let mut admin = daemon.connect().await;
        let Response::DojoCreated { dojo, .. } = admin
            .request(Request::CreateDojo {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "control-transfer".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec!["/bin/sh".into()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("daemon did not create transfer Splint");
        };
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            panic!("new dojo was not a leaf");
        };
        let splint_id = splint.id;
        let incarnation = admin.live_incarnation(splint_id).await;

        let mut owner = daemon.connect().await;
        let mut requester = daemon.connect().await;
        let owner_subscription = owner.subscribe_control(splint_id, incarnation).await;
        let requester_subscription = requester.subscribe_control(splint_id, incarnation).await;
        let owner_controller = owner.acquire_control(splint_id, incarnation).await;

        let Response::ControlTransferPending { transfer_id } = requester
            .request(Request::RequestControlTransfer {
                splint_id,
                incarnation,
            })
            .await
        else {
            panic!("control transfer was not queued");
        };
        loop {
            if matches!(
                owner.next_event(owner_subscription).await.1,
                SubscriptionEvent::ControlTransferRequested { transfer_id: seen }
                    if seen == transfer_id
            ) {
                break;
            }
        }
        assert_eq!(
            owner
                .request(Request::DecideControlTransfer {
                    transfer_id,
                    decision: ControlTransferDecision::Deny,
                })
                .await,
            Response::Acknowledged
        );
        assert!(matches!(
            requester.next_event(requester_subscription).await.1,
            SubscriptionEvent::ControlTransferResolved {
                transfer_id: seen,
                outcome: ControlTransferOutcome::Denied,
                controller_id: None,
            } if seen == transfer_id
        ));

        let Response::ControlTransferPending { transfer_id } = requester
            .request(Request::RequestControlTransfer {
                splint_id,
                incarnation,
            })
            .await
        else {
            panic!("second control transfer was not queued");
        };
        loop {
            if matches!(
                owner.next_event(owner_subscription).await.1,
                SubscriptionEvent::ControlTransferRequested { transfer_id: seen }
                    if seen == transfer_id
            ) {
                break;
            }
        }
        assert_eq!(
            owner
                .request(Request::DecideControlTransfer {
                    transfer_id,
                    decision: ControlTransferDecision::Accept,
                })
                .await,
            Response::Acknowledged
        );
        let transferred_controller = loop {
            if let SubscriptionEvent::ControlTransferResolved {
                transfer_id: seen,
                outcome: ControlTransferOutcome::Granted,
                controller_id: Some(controller_id),
            } = requester.next_event(requester_subscription).await.1
            {
                assert_eq!(seen, transfer_id);
                break controller_id;
            }
        };
        let stale = owner
            .request_result(Request::Input {
                controller_id: owner_controller,
                splint_id,
                incarnation,
                bytes: b"stale".to_vec(),
            })
            .await
            .unwrap_err();
        assert_eq!(stale.code, ErrorCode::Unauthorized);
        assert!(matches!(
            requester
                .request(Request::Input {
                    controller_id: transferred_controller,
                    splint_id,
                    incarnation,
                    bytes: b"printf 'transferred-control\\n'\n".to_vec(),
                })
                .await,
            Response::TerminalActionAcknowledged {
                splint_id: response_id,
                incarnation: response_incarnation,
                ..
            } if response_id == splint_id && response_incarnation == incarnation
        ));
        snapshot_until(&mut admin, splint_id, incarnation, "transferred-control").await;

        drop(requester);
        let mut replacement = daemon.connect().await;
        let replacement_controller = replacement.acquire_control(splint_id, incarnation).await;
        assert_ne!(replacement_controller, transferred_controller);
        daemon.shutdown();
    })
    .await
    .expect("control transfer scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one isolated scenario proves split, independent output, kill, relaunch, and stale rejection"
)]
async fn two_splints_spawn_and_preserve_independent_output() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let cwd = std::env::current_dir().unwrap();
        let shell_launch = || LaunchParameters {
            cwd: cwd.clone(),
            command: vec!["/bin/sh".into()],
            shell: None,
            login_shell: false,
            scrollback_lines: 100,
        };
        let mut admin = daemon.connect().await;
        let Response::DojoCreated { dojo, .. } = admin
            .request(Request::CreateDojo {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "multiplex".into(),
                launch: shell_launch(),
            })
            .await
        else {
            panic!("daemon did not create initial Splint");
        };
        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(first) = &dojo.windows[0].root else {
            panic!("new dojo was not a leaf");
        };
        let first_id = first.id;
        let first_incarnation = admin.live_incarnation(first_id).await;

        let Response::SplintStarted {
            splint_id: second_id,
            incarnation: second_incarnation,
            topology_revision,
        } = admin
            .request(Request::SplitSplint {
                expected_topology_revision: splinterm_core::TopologyRevision::new(1),
                target_splint_id: first_id,
                axis: Axis::Horizontal,
                side: SplitSide::Second,
                ratio: SplitRatio::new(500).unwrap(),
                launch: shell_launch(),
            })
            .await
        else {
            panic!("daemon did not split initial Splint");
        };
        assert_eq!(topology_revision.get(), 2);
        assert_ne!(first_id, second_id);

        let Response::Topology { snapshot: before } = admin.request(Request::InspectTopology).await
        else {
            panic!("daemon did not return topology");
        };
        let failed = admin
            .request_result(Request::SplitSplint {
                expected_topology_revision: topology_revision,
                target_splint_id: first_id,
                axis: Axis::Vertical,
                side: SplitSide::First,
                ratio: SplitRatio::new(400).unwrap(),
                launch: LaunchParameters {
                    command: vec!["/definitely/missing/splinterm-command".into()],
                    ..shell_launch()
                },
            })
            .await
            .unwrap_err();
        assert_eq!(failed.code, ErrorCode::Internal);
        let Response::Topology { snapshot: after } = admin.request(Request::InspectTopology).await
        else {
            panic!("daemon did not return topology after failed split");
        };
        assert_eq!(after, before);
        after.validate().unwrap();

        let mut first_client = daemon.connect().await;
        let mut second_client = daemon.connect().await;
        let first_controller = first_client
            .acquire_control(first_id, first_incarnation)
            .await;
        let second_controller = second_client
            .acquire_control(second_id, second_incarnation)
            .await;
        assert_ne!(first_controller, second_controller);
        let context_command = b"printf 'CTX=%s|%s|%s|%s\\n' \"$SPLINTERM_DOJO_ID\" \"$SPLINTERM_WINDOW_ID\" \"$SPLINTERM_SPLINT_ID\" \"$SPLINTERM_SPLINT_INCARNATION\"\n";
        first_client
            .input(first_id, first_incarnation, context_command)
            .await;
        second_client
            .input(second_id, second_incarnation, context_command)
            .await;
        first_client
            .input(first_id, first_incarnation, b"printf 'first-ready\\n'\n")
            .await;
        second_client
            .input(second_id, second_incarnation, b"printf 'second-ready\\n'\n")
            .await;
        first_client
            .resize(first_id, first_incarnation, 90, 30)
            .await;
        second_client
            .resize(second_id, second_incarnation, 100, 40)
            .await;

        let first_snapshot =
            snapshot_until(&mut admin, first_id, first_incarnation, "first-ready").await;
        let second_snapshot =
            snapshot_until(&mut admin, second_id, second_incarnation, "second-ready").await;
        assert_eq!((first_snapshot.columns, first_snapshot.rows), (90, 30));
        assert_eq!((second_snapshot.columns, second_snapshot.rows), (100, 40));
        let first_text = snapshot_text(&first_snapshot);
        let second_text = snapshot_text(&second_snapshot);
        assert!(first_text.contains(&format!(
            "CTX={dojo_id}|{window_id}|{first_id}|{first_incarnation}"
        )));
        assert!(second_text.contains(&format!(
            "CTX={dojo_id}|{window_id}|{second_id}|{second_incarnation}"
        )));
        assert!(!first_text.contains("caller-supplied"));
        assert!(!second_text.contains("caller-supplied"));
        assert!(!first_text.contains("second-ready"));
        assert!(!second_text.contains("first-ready"));

        assert!(matches!(
            admin
                .request(Request::KillSplint {
                    splint_id: first_id,
                    incarnation: first_incarnation,
                })
                .await,
            Response::SplintKilled { .. }
        ));
        second_client
            .input(
                second_id,
                second_incarnation,
                b"printf 'second-after-kill\\n'\n",
            )
            .await;
        snapshot_until(
            &mut admin,
            second_id,
            second_incarnation,
            "second-after-kill",
        )
        .await;

        let expected_topology_revision = admin.topology_revision().await;
        let Response::SplintStarted {
            splint_id: relaunched_id,
            incarnation: relaunched_incarnation,
            topology_revision,
        } = admin
            .request(Request::RelaunchSplint {
                expected_topology_revision,
                splint_id: first_id,
                launch: LaunchParameters {
                    command: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        "printf 'RELAUNCH=%s|%s|%s|%s\\n' \"$SPLINTERM_DOJO_ID\" \"$SPLINTERM_WINDOW_ID\" \"$SPLINTERM_SPLINT_ID\" \"$SPLINTERM_SPLINT_INCARNATION\"; exec sleep 30".into(),
                    ],
                    ..shell_launch()
                },
            })
            .await
        else {
            panic!("daemon did not relaunch exited Splint");
        };
        assert_eq!(relaunched_id, first_id);
        assert_ne!(relaunched_incarnation, first_incarnation);
        assert_eq!(topology_revision.get(), 2);
        let stale = first_client
            .request_result(Request::Input {
                controller_id: first_controller,
                splint_id: first_id,
                incarnation: first_incarnation,
                bytes: b"stale".to_vec(),
            })
            .await
            .unwrap_err();
        assert_eq!(stale.code, ErrorCode::StaleIncarnation);
        let relaunched = snapshot_until(
            &mut admin,
            first_id,
            relaunched_incarnation,
            "RELAUNCH=",
        )
        .await;
        let relaunched_text = snapshot_text(&relaunched);
        assert!(relaunched_text.contains(&format!(
            "RELAUNCH={dojo_id}|{window_id}|{first_id}|{relaunched_incarnation}"
        )));
        assert!(!relaunched_text.contains("caller-supplied"));

        // Both remaining live processes are intentionally left to daemon shutdown,
        // which must drain the registry, reap them, and remove the socket.
        daemon.shutdown();
    })
    .await
    .expect("two-Splint scenario timed out");
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
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "phase8".into(),
                launch: LaunchParameters {
                    cwd: cwd.clone(),
                    command: Vec::new(),
                    shell: None,
                    login_shell: true,
                    scrollback_lines: 1_000,
                },
            })
            .await
        {
            Response::DojoCreated { dojo, .. } => dojo,
            response => panic!("unexpected create response: {response:?}"),
        };
        let LayoutNode::Leaf(model_splint) = &dojo.windows[0].root else {
            unreachable!()
        };
        let splint_id = model_splint.id;
        let incarnation = creator.live_incarnation(splint_id).await;

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
        with_pwd.validate().expect("daemon snapshot identity is valid");
        assert!(with_pwd
            .visible_rows
            .iter()
            .chain(&with_pwd.scrollback_rows)
            .all(|row| row.row_id.is_some_and(|id| id > 0)));
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
        assert!(matches!(
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
            Response::TerminalActionAcknowledged {
                splint_id: response_id,
                incarnation: response_incarnation,
                ..
            } if response_id == splint_id && response_incarnation == incarnation
        ));
        let resized = snapshot_until(&mut creator, splint_id, incarnation, "phase8-initial").await;
        assert_eq!((resized.columns, resized.rows), (100, 30));
        assert!(resized.history_generation > with_pwd.history_generation);

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
        assert!(matches!(
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
            Response::TerminalActionAcknowledged {
                splint_id: response_id,
                incarnation: response_incarnation,
                ..
            } if response_id == splint_id && response_incarnation == incarnation
        ));
        reattached.release_control().await;
        let mut slow = daemon.connect().await;
        for _ in 0..MAX_SUBSCRIPTIONS {
            let (_subscription_id, _) = slow.attach(splint_id, incarnation).await;
        }
        let mut producer = daemon.connect().await;
        producer
            .input(
                splint_id,
                incarnation,
                b"i=0; while [ $i -lt 1000 ]; do printf 'overflow-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' $i; i=$((i+1)); sleep 0.001; done; printf 'overflow-finished\\n'\n",
            )
            .await;
        let _completion_snapshot = snapshot_until(
            &mut reattached,
            splint_id,
            incarnation,
            "overflow-finished",
        )
        .await;

        let mut saw_resync = false;
        let mut disconnected = false;
        for _ in 0..128 {
            let Ok(frame) = time::timeout(
                Duration::from_secs(2),
                read_frame_or_eof(&mut slow.stream),
            )
            .await
            else {
                break;
            };
            let Some(frame) = frame else {
                disconnected = true;
                break;
            };
            if let ServerFrame::Event {
                event: SubscriptionEvent::ResyncRequired { .. },
                ..
            } = frame
            {
                saw_resync = true;
                break;
            }
        }
        assert!(
            saw_resync || disconnected,
            "slow subscriber was neither forced to resynchronize nor disconnected"
        );
        drop(producer);

        let final_snapshot = snapshot_until(
            &mut reattached,
            splint_id,
            incarnation,
            "overflow-finished",
        )
        .await;
        assert!(final_snapshot.revision > detached.revision);

        let mut before_row_id = final_snapshot
            .scrollback_rows
            .first()
            .and_then(|row| row.row_id)
            .expect("overflow produced paged history");
        let mut paged_ids = std::collections::BTreeSet::new();
        for _ in 0..4 {
            let Response::ScrollbackPage { page } = reattached
                .request(Request::ScrollbackPage {
                    splint_id,
                    incarnation,
                    terminal_revision: final_snapshot.revision,
                    history_generation: final_snapshot.history_generation,
                    before_row_id,
                    max_rows: splinterm_protocol::MAX_SCROLLBACK_PAGE_ROWS,
                })
                .await
            else {
                panic!("daemon did not return a scrollback page");
            };
            page.validate().expect("daemon page is valid");
            assert_eq!(page.rows.len(), splinterm_protocol::MAX_SCROLLBACK_PAGE_ROWS);
            assert!(page.rows.iter().all(|row| row.row_id.unwrap() < before_row_id));
            for row_id in page.rows.iter().filter_map(|row| row.row_id) {
                assert!(paged_ids.insert(row_id), "pages must not overlap");
            }
            before_row_id = page.rows.first().and_then(|row| row.row_id).unwrap();
        }
        assert_eq!(paged_ids.len(), 4 * splinterm_protocol::MAX_SCROLLBACK_PAGE_ROWS);

        for (revision, generation) in [
            (
                final_snapshot.revision.saturating_sub(1).max(1),
                final_snapshot.history_generation,
            ),
            (
                final_snapshot.revision,
                final_snapshot.history_generation.saturating_add(1),
            ),
        ] {
            assert!(matches!(
                reattached
                    .request(Request::ScrollbackPage {
                        splint_id,
                        incarnation,
                        terminal_revision: revision,
                        history_generation: generation,
                        before_row_id,
                        max_rows: 1,
                    })
                    .await,
                Response::ScrollbackResyncRequired { .. }
            ));
        }

        reattached
            .input(
                splint_id,
                incarnation,
                b"printf 'paging-binding-changed\\n'\n",
            )
            .await;
        let changed = snapshot_until(
            &mut reattached,
            splint_id,
            incarnation,
            "paging-binding-changed",
        )
        .await;
        assert!(changed.revision > final_snapshot.revision);
        assert!(matches!(
            reattached
                .request(Request::ScrollbackPage {
                    splint_id,
                    incarnation,
                    terminal_revision: final_snapshot.revision,
                    history_generation: final_snapshot.history_generation,
                    before_row_id,
                    max_rows: 1,
                })
                .await,
            Response::ScrollbackResyncRequired { .. }
        ));

        match reattached
            .request(Request::KillSplint {
                splint_id,
                incarnation,
            })
            .await
        {
            Response::SplintKilled { exit_status, .. } => {
                assert!(exit_status.code.is_some() || exit_status.signal.is_some());
            }
            response => panic!("unexpected terminate response: {response:?}"),
        }
        daemon.shutdown();
    })
    .await
    .expect("Phase 8 scenario timed out");
}
