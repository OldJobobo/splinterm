use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use splinterm_automation_client::{
    Connection as AutomationConnection, protocol_error as automation_protocol_error,
};
use splinterm_core::{
    Axis, DojoId, LairId, LayoutNode, SplintId, SplitRatio, SplitSide, TopologyRevision,
};
use splinterm_protocol::{
    AccessScope, AutomationLaunch, ClientFrame, ClientRole, ColorSource, ControlMode,
    ControlTransferDecision, ControlTransferOutcome, ErrorCode, HistoryTransition,
    LaunchParameters, MAX_FRAME_BYTES, MAX_SUBSCRIPTIONS, MutationPreflight, MutationTarget,
    PROTOCOL_VERSION, ProtocolError, Request, Response, ServerFrame, SplintLifecycle,
    SubscriptionEvent, TerminalProvenance, TerminalSnapshot, TerminalUpdate, TopologyChangeKind,
    encode_frame,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time,
};

const DAEMON: &str = env!("CARGO_BIN_EXE_splinterd");
const TEST_TIMEOUT: Duration = Duration::from_secs(20);
const TEST_SHUTDOWN_GRACE_MS: &str = "1000";

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
        Self::spawn_configured_with_environment(
            runtime,
            socket,
            policy,
            development_terminal_access,
            &[],
        )
    }

    fn spawn_configured_with_environment(
        runtime: &Path,
        socket: &Path,
        policy: Option<&Path>,
        development_terminal_access: bool,
        environment: &[(&str, &str)],
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
            .env("SPLINTERM_LAIR_ID", "caller-supplied-dojo")
            .env("SPLINTERM_DOJO_ID", "caller-supplied-dojo")
            .env("SPLINTERM_SPLINT_ID", "caller-supplied-splint")
            .env(
                "SPLINTERM_SPLINT_INCARNATION",
                "caller-supplied-incarnation",
            )
            // Integration tests preserve the complete HUP -> TERM -> KILL state
            // machine without paying production's two 30-second grace periods.
            // Release daemon builds do not compile support for this override.
            .env(
                "SPLINTERM_TEST_SHUTDOWN_GRACE_MS",
                TEST_SHUTDOWN_GRACE_MS,
            )
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .envs(environment.iter().copied())
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

    async fn start_with_gum_environment(palette: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let runtime = std::env::temp_dir().join(format!(
            "splinterm-gum-environment-{}-{nonce}",
            std::process::id()
        ));
        let theme = runtime.join("state/omarchy/current/theme");
        fs::create_dir_all(&theme).unwrap();
        fs::write(theme.join("gum_env.lua"), palette).unwrap();
        let socket = runtime.join("splinterd.sock");
        let child = Self::spawn_configured_with_environment(
            &runtime,
            &socket,
            None,
            true,
            &[
                ("BACKGROUND", "#1b1b1b"),
                ("GUM_CHOOSE_SELECTED_BACKGROUND", "#808080"),
                ("GUM_OLD_THEME_ONLY", "#f4f4f4"),
            ],
        );
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
                diagnostic_correlation: None,
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
            Response::Splint { runtime, .. } if runtime.splint_id == splint_id => runtime
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
        let Response::ControlGranted { controller_id, .. } = self
            .request(Request::AcquireControl {
                splint_id,
                incarnation,
                modes: vec![ControlMode::Input],
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
        self.attach_with_scrollback(splint_id, incarnation, 16)
            .await
    }

    async fn attach_with_scrollback(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        scrollback_rows: usize,
    ) -> (u64, TerminalSnapshot) {
        match self
            .request(Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows,
            })
            .await
        {
            Response::Attached {
                subscription_id,
                provenance,
                snapshot,
            } => {
                assert_eq!(provenance.splint_id, splint_id);
                assert_eq!(provenance.incarnation, incarnation);
                assert_eq!(provenance.terminal_revision, snapshot.revision);
                assert_eq!(provenance.history_generation, snapshot.history_generation);
                assert_eq!(provenance.title, snapshot.title);
                assert!(provenance.topology_revision.get() > 0);
                (subscription_id, snapshot)
            }
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

fn update_text(update: &TerminalUpdate) -> String {
    update
        .rows
        .iter()
        .map(|patch| &patch.row)
        .chain(
            update
                .scrollback
                .iter()
                .flat_map(|scrollback| scrollback.rows.iter()),
        )
        .flat_map(|row| row.cells.iter())
        .map(|cell| cell.content.as_str())
        .collect()
}

fn apply_terminal_update(snapshot: &mut TerminalSnapshot, update: TerminalUpdate) {
    update
        .validate_against(
            snapshot.revision,
            snapshot.history_generation,
            snapshot.columns,
            snapshot.rows,
        )
        .expect("subscription update validates against reconstructed state");
    assert_eq!(update.columns.unwrap_or(snapshot.columns), snapshot.columns);
    assert_eq!(update.row_count.unwrap_or(snapshot.rows), snapshot.rows);
    for patch in update.rows {
        snapshot.visible_rows[patch.index] = patch.row;
    }
    if let Some(scrollback) = update.scrollback {
        let append = matches!(scrollback.transition, HistoryTransition::Append { .. });
        match scrollback.transition {
            HistoryTransition::Append { trimmed_rows, .. } => {
                snapshot
                    .scrollback_rows
                    .drain(..trimmed_rows.min(snapshot.scrollback_rows.len()));
                snapshot.scrollback_rows.extend(scrollback.rows.clone());
            }
            HistoryTransition::Clear => snapshot.scrollback_rows.clear(),
            HistoryTransition::Reflow | HistoryTransition::Replace => {
                snapshot.scrollback_rows.clone_from(&scrollback.rows);
            }
        }
        snapshot.history_generation = scrollback.history_generation;
        snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
        snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
        snapshot.available_scrollback_rows = scrollback.available_rows;
        snapshot.omitted_oldest_scrollback_rows = if append {
            scrollback
                .available_rows
                .saturating_sub(snapshot.scrollback_rows.len())
        } else {
            scrollback.omitted_oldest_rows
        };
    }
    if let Some(cursor) = update.cursor {
        snapshot.cursor_column = cursor.column;
        snapshot.cursor_row = cursor.row;
        snapshot.cursor_deferred_wrap = cursor.deferred_wrap;
    }
    if let Some(title) = update.title {
        snapshot.title = title;
    }
    if let Some(modes) = update.input_modes {
        snapshot.input_modes = modes;
    }
    if let Some(screen) = update.active_screen {
        snapshot.active_screen = screen;
    }
    if let Some(palette) = update.palette {
        snapshot.palette = palette;
    }
    if let Some(colors) = update.default_colors {
        snapshot.default_colors = colors;
    }
    if let Some(images) = update.images {
        snapshot.images = Some(images);
    }
    snapshot.revision = update.revision;
    snapshot.validate().unwrap_or_else(|error| {
        panic!(
            "reconstructed subscription snapshot remains valid: {error:?}; rows={} available={} omitted={} oldest={:?} newest={:?} revision={}",
            snapshot.scrollback_rows.len(),
            snapshot.available_scrollback_rows,
            snapshot.omitted_oldest_scrollback_rows,
            snapshot.oldest_available_scrollback_row_id,
            snapshot.newest_available_scrollback_row_id,
            snapshot.revision,
        )
    });
}

#[allow(
    clippy::too_many_arguments,
    reason = "terminal provenance is intentionally complete"
)]
fn assert_terminal_provenance(
    provenance: &TerminalProvenance,
    lair_id: LairId,
    dojo_id: DojoId,
    splint_id: SplintId,
    incarnation: u64,
    title: &str,
    terminal_revision: u64,
    history_generation: u64,
) {
    assert_eq!(provenance.lair_id, lair_id);
    assert_eq!(provenance.dojo_id, dojo_id);
    assert_eq!(provenance.splint_id, splint_id);
    assert_eq!(provenance.incarnation, incarnation);
    assert!(provenance.topology_revision.get() > 0);
    assert_eq!(provenance.title, title);
    assert_eq!(provenance.terminal_revision, terminal_revision);
    assert_eq!(provenance.history_generation, history_generation);
}

async fn visible_snapshot_until(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
    marker: &str,
) -> TerminalSnapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (subscription_id, snapshot) = connection
            .attach_with_scrollback(splint_id, incarnation, 0)
            .await;
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
            "visible snapshot never contained {marker}"
        );
        time::sleep(Duration::from_millis(20)).await;
    }
}

async fn snapshot_until(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
    marker: &str,
) -> TerminalSnapshot {
    snapshot_until_with_timeout(
        connection,
        splint_id,
        incarnation,
        marker,
        Duration::from_secs(10),
    )
    .await
}

async fn snapshot_until_with_timeout(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
    marker: &str,
    timeout: Duration,
) -> TerminalSnapshot {
    let deadline = Instant::now() + timeout;
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

async fn stable_snapshot_after_marker(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
    marker: &str,
) -> TerminalSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut snapshot = snapshot_until(connection, splint_id, incarnation, marker).await;
    let mut stable_samples = 0;
    loop {
        time::sleep(Duration::from_millis(20)).await;
        let next = snapshot_until(connection, splint_id, incarnation, marker).await;
        if next.revision == snapshot.revision {
            stable_samples += 1;
            if stable_samples == 5 {
                return next;
            }
        } else {
            stable_samples = 0;
            snapshot = next;
        }
        assert!(
            Instant::now() < deadline,
            "snapshot revision did not become quiescent after {marker}"
        );
    }
}

async fn wait_for_pid_marker(marker: &Path, label: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = fs::read_to_string(marker)
            && let Ok(pid) = contents.trim().parse()
        {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "{label} PID marker was not completed"
        );
        time::sleep(Duration::from_millis(10)).await;
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
    let mut resources = vec![serde_json::json!({"kind": "daemon"})];
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
        "schema": "splinterm.policy.v2",
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

fn current_metadata_policy(splint_id: SplintId) -> String {
    let (executable, sha256) = policy_executable_identity();
    serde_json::json!({
        "schema": "splinterm.policy.v2",
        "rules": [{
            "id": "restorable-metadata-test",
            "executable": {"path": executable, "sha256": sha256},
            "scopes": ["topology_metadata_read", "audit_inspect"],
            "resources": [
                {"kind": "daemon"},
                {
                    "kind": "splint",
                    "splint_id": splint_id,
                    "incarnation": "current"
                }
            ],
            "limits": {"max_returned_bytes": 65536, "max_results": 16}
        }]
    })
    .to_string()
}

fn scoped_authorization_policy(splint_id: SplintId, incarnation: u64, scopes: &[&str]) -> String {
    let (executable, sha256) = policy_executable_identity();
    let mut resources = vec![serde_json::json!({
        "kind": "splint",
        "splint_id": splint_id,
        "incarnation": incarnation
    })];
    if scopes.contains(&"audit_inspect") {
        resources.push(serde_json::json!({"kind": "daemon"}));
    }
    serde_json::json!({
        "schema": "splinterm.policy.v2",
        "rules": [{
            "id": "authorization-only-test",
            "executable": {"path": executable, "sha256": sha256},
            "scopes": scopes,
            "resources": resources,
            "limits": {
                "max_returned_rows": 16,
                "max_results": 16,
                "max_returned_bytes": 1_048_576,
                "max_live_subscriptions": 2,
                "max_spawn_count": 1,
                "deadline_ms": 10
            }
        }]
    })
    .to_string()
}

fn parent_snapshot_policy(lair_id: splinterm_core::LairId) -> String {
    let (executable, sha256) = policy_executable_identity();
    serde_json::json!({
        "schema": "splinterm.policy.v2",
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
                {"kind": "daemon"},
                {"kind": "lair", "lair_id": lair_id}
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
    let mut notified = false;
    for _ in 0..16 {
        let frame = time::timeout(
            Duration::from_secs(5),
            read_frame_or_eof(&mut connection.stream),
        )
        .await
        .unwrap_or_else(|_| panic!("{reason} did not notify the existing client"));
        match frame {
            Some(ServerFrame::Error {
                request_id: None,
                error:
                    ProtocolError {
                        code: ErrorCode::Unauthorized,
                        ref message,
                        ..
                    },
            }) if message == "persistent policy reloaded; reconnect required" => {
                notified = true;
                break;
            }
            Some(ServerFrame::Event { .. }) => {}
            frame => panic!("{reason} returned an unexpected reload frame: {frame:?}"),
        }
    }
    assert!(notified, "{reason} omitted the policy reload diagnostic");
    let closed = time::timeout(
        Duration::from_secs(5),
        read_frame_or_eof(&mut connection.stream),
    )
    .await
    .unwrap_or_else(|_| panic!("{reason} did not close the existing client"));
    assert!(closed.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_roles_require_their_exact_installed_graphical_processes() {
    time::timeout(Duration::from_secs(30), async {
        let daemon = Daemon::start().await;
        for (role, expected) in [
            (
                ClientRole::TrustedUi,
                "trusted UI role requires the installed graphical client",
            ),
            (
                ClientRole::RemoteInteractive,
                "remote interactive role requires the installed graphical relay",
            ),
        ] {
            let mut stream = UnixStream::connect(&daemon.socket).await.unwrap();
            write_frame(
                &mut stream,
                &ClientFrame::Hello {
                    minimum_version: PROTOCOL_VERSION,
                    maximum_version: PROTOCOL_VERSION,
                    role,
                },
            )
            .await;
            assert!(matches!(
                read_frame(&mut stream).await,
                ServerFrame::Error {
                    request_id: None,
                    error: ProtocolError {
                        code: ErrorCode::Unauthorized,
                        ref message,
                        ..
                    },
                } if message == expected
            ));
        }
        let mut automation = daemon.connect().await;
        assert!(matches!(
            automation.request(Request::Ping).await,
            Response::Pong
        ));
        let revision = automation.topology_revision().await;
        let denied = automation
            .request_result(Request::CreateTransientLair {
                expected_topology_revision: revision,
                name: "automation-must-not-own-transient".into(),
                launch: LaunchParameters {
                    cwd: daemon.runtime.clone(),
                    command: vec!["/bin/true".into()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 1_000,
                },
            })
            .await
            .unwrap_err();
        assert_eq!(denied.code, ErrorCode::Unauthorized);
        let denied = automation
            .request_result(Request::MaterializePreset {
                expected_topology_revision: revision,
                target: splinterm_protocol::PresetTarget::NewLair {
                    name: "automation-must-not-materialize-preset".into(),
                },
                dojos: Vec::new(),
                directory_identities: Vec::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(denied.code, ErrorCode::Unauthorized);
        let Response::Topology { snapshot } = automation.request(Request::InspectTopology).await
        else {
            panic!("topology response was not returned");
        };
        assert!(snapshot.topology.lairs().next().is_none());
        daemon.shutdown();
    })
    .await
    .expect("human-role identity validation timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one isolated scenario proves the complete mutation preflight authority boundary"
)]
async fn mutation_preflight_preserves_exact_scope_cas_and_descendant_denial() {
    time::timeout(Duration::from_secs(60), async {
        let daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let mut setup = daemon.connect().await;
        let revision = setup.topology_revision().await;
        let Response::LairCreated {
            lair: dojo,
            incarnation,
            ..
        } = setup
            .request(Request::CreateLair {
                expected_topology_revision: revision,
                name: "mutation-preflight".to_owned(),
                launch: LaunchParameters {
                    cwd: daemon.runtime.clone(),
                    command: vec![
                        "/bin/sh".to_owned(),
                        "-c".to_owned(),
                        "sleep 3600".to_owned(),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("mutation setup did not create a Lair")
        };
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            panic!("mutation setup root was not a leaf")
        };
        let splint_id = splint.id;
        let policy = daemon.policy.as_ref().unwrap();
        fs::write(
            policy,
            scoped_authorization_policy(splint_id, incarnation, &["topology_layout_mutate"]),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut setup, "missing process-spawn policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut missing_scope = daemon.connect().await;
        assert_eq!(
            missing_scope
                .request_result(Request::PrepareMutation {
                    mutation: MutationPreflight::SplitSplint { splint_id },
                })
                .await
                .expect_err("split preflight without process-spawn must fail")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(splint_id, incarnation, &["process_spawn"]),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut missing_scope, "missing layout scope policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut missing_scope = daemon.connect().await;
        assert_eq!(
            missing_scope
                .request_result(Request::PrepareMutation {
                    mutation: MutationPreflight::SplitSplint { splint_id },
                })
                .await
                .expect_err("split preflight without layout scope must fail")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &["process_spawn", "topology_layout_mutate", "audit_inspect"],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut missing_scope, "mutation policy reload").await;
        time::sleep(Duration::from_millis(50)).await;

        let mut scoped = daemon.connect().await;
        assert_eq!(
            scoped
                .request_result(Request::InspectTopology)
                .await
                .expect_err("mutation-only policy must not permit topology inspection")
                .code,
            ErrorCode::Unauthorized
        );
        let Response::AuditPage { page: baseline } = scoped
            .request(Request::AuditInspect {
                after_audit_id: None,
                max_records: 16,
            })
            .await
        else {
            panic!("audit baseline was not returned")
        };
        let baseline_audit_id = baseline.records.last().map(|record| record.audit_id);

        // A completed preflight followed by client cancellation/disconnect is not
        // a committed mutation and must leave no successful mutation audit.
        let mut cancelled = daemon.connect().await;
        assert!(matches!(
            cancelled
                .request(Request::PrepareMutation {
                    mutation: MutationPreflight::SplitSplint { splint_id },
                })
                .await,
            Response::MutationPrepared { .. }
        ));
        drop(cancelled);

        let Response::MutationPrepared { preparation } = scoped
            .request(Request::PrepareMutation {
                mutation: MutationPreflight::SplitSplint { splint_id },
            })
            .await
        else {
            panic!("split preflight did not return scoped preparation")
        };
        assert_eq!(preparation.splint_id, Some(splint_id));
        assert_eq!(preparation.incarnation, Some(incarnation));
        assert!(preparation.targets.is_empty());
        let Response::SplintStarted {
            splint_id: child_id,
            topology_revision,
            ..
        } = scoped
            .request(Request::SplitSplintAutomation {
                expected_topology_revision: preparation.topology_revision,
                target_splint_id: splint_id,
                axis: Axis::Horizontal,
                side: SplitSide::Second,
                ratio: SplitRatio::new(500).unwrap(),
                launch: AutomationLaunch {
                    cwd: Some(daemon.runtime.clone()),
                    argv: vec![
                        "/bin/sh".to_owned(),
                        "-c".to_owned(),
                        "sleep 3600".to_owned(),
                    ],
                },
            })
            .await
        else {
            panic!("authorized split did not commit")
        };
        assert_eq!(
            topology_revision.get(),
            preparation.topology_revision.get() + 1
        );
        assert_eq!(
            scoped
                .request_result(Request::PrepareMutation {
                    mutation: MutationPreflight::SetSplitRatio {
                        splint_id: child_id
                    },
                })
                .await
                .expect_err("new descendant must remain outside publication snapshot")
                .code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            scoped
                .request_result(Request::SetSplitRatio {
                    expected_topology_revision: preparation.topology_revision,
                    target_splint_id: splint_id,
                    ancestor: 0,
                    ratio: SplitRatio::new(400).unwrap(),
                })
                .await
                .expect_err("stale mutation CAS must fail")
                .code,
            ErrorCode::StaleTopology
        );
        let Response::AuditPage { page } = scoped
            .request(Request::AuditInspect {
                after_audit_id: baseline_audit_id,
                max_records: 16,
            })
            .await
        else {
            panic!("post-mutation audit page was not returned")
        };
        let split_records = page
            .records
            .iter()
            .filter(|record| record.operation == splinterm_protocol::AuditOperation::SplitSplint)
            .collect::<Vec<_>>();
        assert_eq!(split_records.len(), 1, "preflight polluted mutation audit");
        assert_eq!(
            split_records[0].outcome,
            Some(splinterm_protocol::AuditOutcome::Succeeded)
        );
        assert_eq!(
            split_records[0].decision,
            splinterm_protocol::AuditDecision::Matched
        );
        daemon.shutdown();
    })
    .await
    .expect("mutation preflight scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one real-daemon lifecycle proves both runtime-only MCP mutation revision contracts"
)]
async fn runtime_relaunch_and_restore_preserve_topology_revision() {
    time::timeout(Duration::from_secs(60), async {
        let daemon = Daemon::start().await;
        let mut client = daemon.connect().await;
        let revision = client.topology_revision().await;
        let Response::LairCreated {
            lair: dojo,
            incarnation: first_incarnation,
            topology_revision,
        } = client
            .request(Request::CreateLair {
                expected_topology_revision: revision,
                name: "runtime-relaunch".to_owned(),
                launch: LaunchParameters {
                    cwd: daemon.runtime.clone(),
                    command: vec!["/bin/sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("runtime relaunch setup did not create a Lair")
        };
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            panic!("runtime relaunch setup was not a leaf")
        };
        let splint_id = splint.id;
        assert_eq!(topology_revision.get(), revision.get() + 1);

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let Response::Splint { runtime, .. } =
                client.request(Request::InspectSplint { splint_id }).await
            else {
                panic!("runtime status was not returned")
            };
            if runtime.live_incarnation.is_none() && runtime.restorable {
                break;
            }
            assert!(Instant::now() < deadline, "initial process did not exit");
            time::sleep(Duration::from_millis(20)).await;
        }

        let relaunch = splinterm_mcp::dispatch_mutation_for_integration_test(
            "splinterm.relaunch_splint",
            &serde_json::json!({
                "splint_id": splint_id,
                "cwd": daemon.runtime.to_string_lossy(),
                "argv": ["/bin/sh", "-c", "exit 0"]
            }),
            &daemon.socket,
        )
        .await
        .expect("MCP runtime relaunch did not commit");
        assert_eq!(relaunch["ok"], true);
        assert_eq!(
            relaunch["resource"]["topology_revision"],
            topology_revision.get()
        );
        let second_incarnation = relaunch["resource"]["incarnation"]
            .as_u64()
            .expect("MCP relaunch omitted incarnation");
        assert!(second_incarnation > first_incarnation);

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let Response::Splint { runtime, .. } =
                client.request(Request::InspectSplint { splint_id }).await
            else {
                panic!("relaunched runtime status was not returned")
            };
            if runtime.live_incarnation.is_none() && runtime.restorable {
                break;
            }
            assert!(Instant::now() < deadline, "relaunched process did not exit");
            time::sleep(Duration::from_millis(20)).await;
        }

        let restore = splinterm_mcp::dispatch_mutation_for_integration_test(
            "splinterm.restore_splint",
            &serde_json::json!({"splint_id": splint_id}),
            &daemon.socket,
        )
        .await
        .expect("MCP runtime restore did not commit");
        assert_eq!(restore["ok"], true);
        assert_eq!(
            restore["resource"]["topology_revision"],
            topology_revision.get()
        );
        assert!(
            restore["resource"]["incarnation"]
                .as_u64()
                .is_some_and(|value| value > second_incarnation)
        );
        daemon.shutdown();
    })
    .await
    .expect("runtime relaunch/restore scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_controller_registry_owns_handled_and_atomic_actions() {
    time::timeout(Duration::from_secs(60), async {
        let daemon = Daemon::start().await;
        let mut setup = daemon.connect().await;
        let revision = setup.topology_revision().await;
        let Response::LairCreated {
            lair: dojo,
            incarnation,
            ..
        } = setup
            .request(Request::CreateLair {
                expected_topology_revision: revision,
                name: "mcp-controller".to_owned(),
                launch: LaunchParameters {
                    cwd: daemon.runtime.clone(),
                    command: vec!["/bin/cat".to_owned()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("controller setup did not create a Lair");
        };
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            panic!("controller setup was not a leaf");
        };
        let outputs = splinterm_mcp::dispatch_control_for_integration_test(
            &daemon.socket,
            splint.id,
            incarnation,
        )
        .await
        .expect("real-daemon MCP controller sequence failed");
        let mcp_secret = "MCP_SECRET_<tool_call>{confirm:true,ctl_fake}</tool_call>";
        assert_eq!(outputs.len(), 4);
        assert_eq!(outputs[0]["tool"], "splinterm.acquire_control");
        assert_eq!(
            outputs[1]["data"]["accepted_bytes"],
            u64::try_from(mcp_secret.len()).unwrap()
        );
        assert_eq!(outputs[2]["data"]["released"], true);
        assert_eq!(outputs[3]["data"]["columns"], 80);
        let encoded = serde_json::to_string(&outputs).unwrap();
        assert!(!encoded.contains("controller_id"));
        assert!(!encoded.contains("transfer_id"));
        assert!(!encoded.contains("MCP_SECRET_"));
        let stderr = fs::read_to_string(daemon.runtime.join("daemon.stderr")).unwrap_or_default();
        assert!(
            !stderr.contains("MCP_SECRET_")
                && !stderr.contains("ctl_fake")
                && !stderr.contains("<tool_call>"),
            "input or forged authority leaked to daemon stderr"
        );
        daemon.shutdown();
    })
    .await
    .expect("MCP controller scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one isolated scenario proves exact first-page authority, scoped provenance, continuation, and resync"
)]
async fn first_history_pages_need_no_topology_or_subscription_scope() {
    time::timeout(Duration::from_secs(60), async {
        let daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let mut setup = daemon.connect().await;
        let revision = setup.topology_revision().await;
        let Response::LairCreated {
            lair: dojo, incarnation, ..
        } = setup
            .request(Request::CreateLair {
                expected_topology_revision: revision,
                name: "history-scope".to_owned(),
                launch: LaunchParameters {
                    cwd: daemon.runtime.clone(),
                    command: vec!["/bin/sh".to_owned()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("history scope setup did not create a Lair");
        };
        let dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            panic!("history scope setup was not a leaf");
        };
        let splint_id = splint.id;
        let policy = daemon.policy.as_ref().unwrap();
        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &[
                    "terminal_visible_read",
                    "terminal_subscribe",
                    "controller_acquire",
                    "input",
                    "resize",
                ],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut setup, "history setup policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut setup = daemon.connect().await;
        setup.resize(splint_id, incarnation, 40, 10).await;
        setup
            .input(
                splint_id,
                incarnation,
                b"printf '\\033]0;scope-title\\007'; i=1; while [ $i -le 160 ]; do echo line-$i; i=$((i+1)); done; echo needle; printf '\\163\\143\\157\\160\\145\\055\\162\\145\\141\\144\\171\\n'\n",
            )
            .await;
        let setup_snapshot =
            visible_snapshot_until(&mut setup, splint_id, incarnation, "scope-ready").await;
        assert_eq!(setup_snapshot.rows, 10);
        assert!(
            setup_snapshot.available_scrollback_rows > 0,
            "setup snapshot did not retain history: {setup_snapshot:?}"
        );
        let Response::Attached {
            subscription_id,
            provenance: attached_provenance,
            snapshot: attached_snapshot,
        } = setup
            .request(Request::Attach {
                splint_id,
                incarnation: None,
                scrollback_rows: 0,
            })
            .await
        else {
            panic!("current-incarnation attach did not return a snapshot");
        };
        assert_terminal_provenance(
            &attached_provenance,
            dojo.id,
            dojo_id,
            splint_id,
            incarnation,
            "scope-title",
            attached_snapshot.revision,
            attached_snapshot.history_generation,
        );
        assert_eq!(
            setup.request(Request::Detach { subscription_id }).await,
            Response::Acknowledged
        );

        fs::write(
            policy,
            scoped_authorization_policy(splint_id, incarnation, &["scrollback_read"]),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut setup, "missing-visible-read policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut missing_scope = daemon.connect().await;
        assert_eq!(
            missing_scope
                .request_result(Request::StartScrollbackPage {
                    splint_id,
                    incarnation: None,
                    max_rows: 8,
                })
                .await
                .expect_err("scrollback without terminal-visible scope must fail")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(splint_id, incarnation, &["terminal_visible_read"]),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut missing_scope, "missing-scrollback policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut missing_scope = daemon.connect().await;
        assert_eq!(
            missing_scope
                .request_result(Request::StartScrollbackPage {
                    splint_id,
                    incarnation: None,
                    max_rows: 8,
                })
                .await
                .expect_err("terminal-visible without scrollback scope must fail")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &["terminal_visible_read", "scrollback_read"],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut missing_scope, "history-read policy reload").await;
        time::sleep(Duration::from_millis(50)).await;

        let mut scoped = daemon.connect().await;
        assert_eq!(
            scoped
                .request_result(Request::InspectTopology)
                .await
                .expect_err("history-only policy must not permit topology reads")
                .code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            scoped
                .request_result(Request::Attach {
                    splint_id,
                    incarnation: None,
                    scrollback_rows: 0,
                })
                .await
                .expect_err("history-only policy must not permit terminal subscriptions")
                .code,
            ErrorCode::Unauthorized
        );

        let Response::ScrollbackPage { provenance, page } = scoped
            .request(Request::StartScrollbackPage {
                splint_id,
                incarnation: None,
                max_rows: 8,
            })
            .await
        else {
            panic!("scoped first scrollback page was not returned");
        };
        page.validate().unwrap();
        assert_terminal_provenance(
            &provenance,
            dojo.id,
            dojo_id,
            splint_id,
            incarnation,
            "scope-title",
            page.terminal_revision,
            page.history_generation,
        );
        assert!(!page.rows.is_empty(), "first page was empty: {page:?}");

        let before_row_id = page.rows.first().and_then(|row| row.row_id).unwrap();
        let Response::ScrollbackPage {
            provenance: continued_provenance,
            page: continued,
        } = scoped
            .request(Request::ScrollbackPage {
                splint_id,
                incarnation: provenance.incarnation,
                terminal_revision: provenance.terminal_revision,
                history_generation: provenance.history_generation,
                before_row_id,
                max_rows: 4,
            })
            .await
        else {
            panic!("scoped scrollback continuation was not returned");
        };
        continued.validate().unwrap();
        assert_terminal_provenance(
            &continued_provenance,
            dojo.id,
            dojo_id,
            splint_id,
            provenance.incarnation,
            "scope-title",
            continued.terminal_revision,
            continued.history_generation,
        );

        let Response::ScrollbackResyncRequired {
            provenance: scrollback_resync,
            current_revision,
            history_generation,
        } = scoped
            .request(Request::ScrollbackPage {
                splint_id,
                incarnation: provenance.incarnation,
                terminal_revision: provenance.terminal_revision.saturating_add(1),
                history_generation: provenance.history_generation,
                before_row_id,
                max_rows: 4,
            })
            .await
        else {
            panic!("stale scrollback page did not return resync provenance");
        };
        assert_terminal_provenance(
            &scrollback_resync,
            dojo.id,
            dojo_id,
            splint_id,
            provenance.incarnation,
            "scope-title",
            current_revision,
            history_generation,
        );

        assert_eq!(
            scoped
                .request_result(Request::StartSearchScrollback {
                    splint_id,
                    incarnation: None,
                    query: "needle".to_owned(),
                    case_sensitive: false,
                    max_results: 4,
                })
                .await
                .expect_err("scrollback-only policy must not permit search")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &[
                    "terminal_visible_read",
                    "scrollback_read",
                    "scrollback_search",
                ],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "history-search policy reload").await;
        time::sleep(Duration::from_millis(50)).await;

        let mut scoped = daemon.connect().await;
        let Response::SearchResults {
            provenance: search_provenance,
            page: search,
        } = scoped
            .request(Request::StartSearchScrollback {
                splint_id,
                incarnation: None,
                query: "needle".to_owned(),
                case_sensitive: false,
                max_results: 4,
            })
            .await
        else {
            panic!("scoped first search page was not returned");
        };
        search.validate().unwrap();
        assert_terminal_provenance(
            &search_provenance,
            dojo.id,
            dojo_id,
            splint_id,
            incarnation,
            "scope-title",
            search.terminal_revision,
            search.history_generation,
        );
        assert!(search.matches.iter().any(|item| item.preview.contains("needle")));

        let Response::SearchResyncRequired {
            provenance: resync,
            current_revision,
            history_generation,
        } = scoped
            .request(Request::SearchScrollback {
                splint_id,
                incarnation: search_provenance.incarnation,
                terminal_revision: search_provenance.terminal_revision.saturating_add(1),
                history_generation: search_provenance.history_generation,
                query: "needle".to_owned(),
                case_sensitive: false,
                cursor: None,
                max_results: 4,
            })
            .await
        else {
            panic!("stale scoped search did not return resync provenance");
        };
        assert_terminal_provenance(
            &resync,
            dojo.id,
            dojo_id,
            splint_id,
            search_provenance.incarnation,
            "scope-title",
            current_revision,
            history_generation,
        );
        daemon.shutdown();
    })
    .await
    .expect("scoped first history page test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one isolated scenario proves scoped status, grant metadata, revocation, and stale-incarnation behavior"
)]
async fn scoped_authorization_status_needs_no_topology_permission() {
    time::timeout(Duration::from_secs(180), async {
        let mut daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let mut connection = daemon.connect().await;
        let revision = connection.topology_revision().await;
        let Response::LairCreated {
            lair: dojo,
            incarnation,
            ..
        } = connection
            .request(Request::CreateLair {
                expected_topology_revision: revision,
                name: "authorization-scope".to_owned(),
                launch: LaunchParameters {
                    cwd: daemon.runtime.clone(),
                    command: vec![
                        "/bin/sh".to_owned(),
                        "-c".to_owned(),
                        "sleep 300".to_owned(),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("authorization scope setup did not create a Lair");
        };
        let dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            panic!("authorization scope setup was not a leaf");
        };
        let splint_id = splint.id;
        let policy = daemon.policy.as_ref().unwrap();
        fs::write(policy, exact_headless_policy(None)).unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut connection, "wrong-resource setup policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut connection = daemon.connect().await;
        let second_revision = connection.topology_revision().await;
        let Response::LairCreated {
            lair: other_dojo,
            incarnation: other_incarnation,
            ..
        } = connection
            .request(Request::CreateLair {
                expected_topology_revision: second_revision,
                name: "authorization-other".to_owned(),
                launch: LaunchParameters {
                    cwd: daemon.runtime.clone(),
                    command: vec![
                        "/bin/sh".to_owned(),
                        "-c".to_owned(),
                        "sleep 300".to_owned(),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("authorization wrong-resource setup did not create a Lair");
        };
        let LayoutNode::Leaf(other_splint) = &other_dojo.dojos[0].root else {
            panic!("authorization wrong-resource setup was not a leaf");
        };
        let other_splint_id = other_splint.id;

        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &[
                    "authorization_inspect",
                    "authorization_revoke",
                    "terminal_visible_read",
                ],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut connection, "authorization-only policy reload").await;
        time::sleep(Duration::from_millis(50)).await;

        let mut scoped = daemon.connect().await;
        let Response::AuthorizationStatus {
            lair_id,
            dojo_id: response_dojo_id,
            incarnation: response_incarnation,
            topology_revision,
            policy_generation,
            persistent,
            development_bypass,
            ..
        } = scoped
            .request(Request::AuthorizationStatus {
                splint_id,
                incarnation: None,
            })
            .await
        else {
            panic!("scoped authorization status response was not returned");
        };
        assert_eq!(lair_id, dojo.id);
        assert_eq!(response_dojo_id, dojo_id);
        assert_eq!(response_incarnation, incarnation);
        assert!(topology_revision.get() > 0);
        assert!(policy_generation > 1);
        assert_eq!(persistent.len(), 1);
        assert!(!development_bypass);

        let Response::AccessGranted {
            lair_id: granted_lair_id,
            dojo_id: granted_dojo_id,
            authorization_revision: granted_revision,
            grant,
        } = scoped
            .request(Request::RequestAccess {
                splint_id,
                incarnation,
                scopes: vec![AccessScope::Observe],
            })
            .await
        else {
            panic!("exact scoped access grant was not returned");
        };
        assert_eq!(granted_lair_id, dojo.id);
        assert_eq!(granted_dojo_id, dojo_id);
        assert_eq!(granted_revision, 1);
        assert!(grant.grant_id > 0);
        assert_eq!(grant.splint_id, splint_id);
        assert_eq!(grant.incarnation, incarnation);

        fs::write(
            policy,
            scoped_authorization_policy(splint_id, incarnation, &["authorization_revoke"]),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "revoke-only policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut scoped = daemon.connect().await;

        let Response::AccessRevoked {
            lair_id: revoked_lair_id,
            dojo_id: revoked_dojo_id,
            authorization_revision: revoked_revision,
            grant: revoked,
        } = scoped
            .request(Request::RevokeAccess {
                grant_id: grant.grant_id,
            })
            .await
        else {
            panic!("exact scoped access revocation was not returned");
        };
        assert_eq!(revoked_lair_id, dojo.id);
        assert_eq!(revoked_dojo_id, dojo_id);
        assert_eq!(revoked_revision, granted_revision + 1);
        assert_eq!(revoked, grant);
        assert_eq!(
            scoped
                .request_result(Request::AuthorizationStatus {
                    splint_id,
                    incarnation: None,
                })
                .await
                .expect_err("missing authorization_inspect must deny status")
                .code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            scoped
                .request_result(Request::InspectTopology)
                .await
                .expect_err("revoke-only policy must not permit topology inspection")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &["authorization_inspect", "authorization_revoke"],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "missing requested-access scope reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut scoped = daemon.connect().await;
        assert_eq!(
            scoped
                .request_result(Request::RequestAccess {
                    splint_id,
                    incarnation,
                    scopes: vec![AccessScope::Observe],
                })
                .await
                .expect_err("missing requested terminal access scope must deny")
                .code,
            ErrorCode::ConsentDenied
        );

        fs::write(
            policy,
            scoped_authorization_policy(
                other_splint_id,
                other_incarnation,
                &[
                    "authorization_inspect",
                    "authorization_revoke",
                    "terminal_visible_read",
                ],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "wrong-resource policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut scoped = daemon.connect().await;
        assert_eq!(
            scoped
                .request_result(Request::AuthorizationStatus {
                    splint_id,
                    incarnation: None,
                })
                .await
                .expect_err("another Splint policy must not authorize this Splint")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &["authorization_inspect", "terminal_visible_read"],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "grant setup policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut scoped = daemon.connect().await;
        let Response::AccessGranted {
            grant: unrevokeable,
            authorization_revision: 3,
            ..
        } = scoped
            .request(Request::RequestAccess {
                splint_id,
                incarnation,
                scopes: vec![AccessScope::Observe],
            })
            .await
        else {
            panic!("missing-revoke setup grant was not returned");
        };
        fs::write(
            policy,
            scoped_authorization_policy(splint_id, incarnation, &["authorization_inspect"]),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "missing authorization_revoke reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut scoped = daemon.connect().await;
        assert_eq!(
            scoped
                .request_result(Request::RevokeAccess {
                    grant_id: unrevokeable.grant_id,
                })
                .await
                .expect_err("missing authorization_revoke must deny revocation")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(policy, r#"{"schema":"splinterm.policy.v2","rules":[]}"#).unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "deny-all policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut scoped = daemon.connect().await;
        assert_eq!(
            scoped
                .request_result(Request::AuthorizationStatus {
                    splint_id,
                    incarnation: None,
                })
                .await
                .expect_err("no-policy status must deny")
                .code,
            ErrorCode::Unauthorized
        );

        fs::write(
            policy,
            scoped_authorization_policy(
                splint_id,
                incarnation,
                &[
                    "authorization_inspect",
                    "authorization_revoke",
                    "terminal_visible_read",
                ],
            ),
        )
        .unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        assert_connection_closed(&mut scoped, "stale-incarnation policy reload").await;
        time::sleep(Duration::from_millis(50)).await;
        let mut scoped = daemon.connect().await;
        assert_eq!(
            scoped
                .request_result(Request::RequestAccess {
                    splint_id,
                    incarnation: incarnation + 1,
                    scopes: vec![AccessScope::Observe],
                })
                .await
                .expect_err("stale exact incarnation must fail closed before consent")
                .code,
            ErrorCode::Unauthorized
        );

        daemon.stop_preserving_state();
        daemon.start_again().await;
        time::sleep(Duration::from_millis(50)).await;
        let mut restarted = daemon.connect().await;
        assert_eq!(
            restarted
                .request_result(Request::AuthorizationStatus {
                    splint_id,
                    incarnation: None,
                })
                .await
                .expect_err("restarted restorable Splint has no current authorization target")
                .code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            restarted
                .request_result(Request::RequestAccess {
                    splint_id,
                    incarnation,
                    scopes: vec![AccessScope::Observe],
                })
                .await
                .expect_err("exited restorable Splint must deny requested access")
                .code,
            ErrorCode::Unauthorized
        );
        daemon.shutdown();
    })
    .await
    .expect("scoped authorization policy test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automation_client_preserves_accepted_and_rejected_reload_diagnostics() {
    time::timeout(Duration::from_secs(60), async {
        let daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let policy = daemon.policy.as_ref().unwrap();

        for candidate in [
            exact_headless_policy(None),
            r#"{"schema":"wrong","rules":[]}"#.to_owned(),
        ] {
            let mut connection = AutomationConnection::connect_automation_at(&daemon.socket)
                .await
                .unwrap();
            assert!(matches!(
                connection
                    .request(Request::SubscribeTopology)
                    .await
                    .unwrap(),
                Response::TopologySubscribed { .. }
            ));
            fs::write(policy, candidate).unwrap();
            fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
            daemon.reload_policy();

            let error = connection.next_server_frame().await.unwrap_err();
            assert_eq!(
                automation_protocol_error(&error).map(|error| (error.code, error.message.as_str())),
                Some((
                    ErrorCode::Unauthorized,
                    "persistent policy reloaded; reconnect required"
                ))
            );
            let unusable = connection.request(Request::Ping).await.unwrap_err();
            assert!(unusable.to_string().contains("cannot be reused"));
        }

        daemon.shutdown();
    })
    .await
    .expect("automation reload diagnostic scenario timed out");
}

#[allow(
    clippy::too_many_lines,
    reason = "the ordered policy reload, controller cleanup, restart, and process-reap gate is one lifecycle"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headless_policy_reload_fails_closed_and_cleans_up() {
    time::timeout(Duration::from_secs(60), async {
        let mut daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let marker = daemon.runtime.join("child-pid");
        let mut connection = daemon.connect().await;
        let revision = connection.topology_revision().await;
        let Response::LairCreated {
            lair: dojo,
            incarnation,
            ..
        } = connection
            .request(Request::CreateLair {
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
            panic!("headless Lair was not created");
        };
        let dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            panic!("created headless Lair was not a leaf");
        };
        let splint_id = splint.id;
        let splint_title = splint.title.clone();
        let lair_only_denial = connection
            .request_result(Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 0,
            })
            .await
            .expect_err("Topology creation authority must not cover the new Lair descendant");
        assert_eq!(lair_only_denial.code, ErrorCode::Unauthorized);
        let child_pid = wait_for_pid_marker(&marker, "child").await;

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
            .find(|record| record.operation == splinterm_protocol::AuditOperation::CreateLair)
            .expect("authorized create audit record was absent");
        let peer_executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let peer_metadata = fs::metadata(&peer_executable).unwrap();
        let mut peer_bytes = Vec::new();
        fs::File::open(&peer_executable)
            .unwrap()
            .read_to_end(&mut peer_bytes)
            .unwrap();
        assert_eq!(create_audit.schema, "splinterm.audit.v2");
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
        fs::write(policy, current_metadata_policy(splint_id)).unwrap();
        fs::set_permissions(policy, fs::Permissions::from_mode(0o600)).unwrap();
        daemon.reload_policy();
        time::sleep(Duration::from_millis(50)).await;

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
        let restarted_runtime = snapshot
            .runtimes
            .iter()
            .find(|runtime| runtime.splint_id == splint_id)
            .expect("persisted Splint runtime metadata was omitted after restart");
        assert_eq!(restarted_runtime.live_incarnation, None);
        assert_eq!(restarted_runtime.last_incarnation, Some(incarnation));
        assert!(restarted_runtime.restorable);
        let restarted_dojo = snapshot.topology.lairs().next().unwrap();
        assert_eq!(restarted_dojo.id, dojo.id);
        let restarted_window = &restarted_dojo.dojos[0];
        assert_eq!(restarted_window.id, dojo_id);
        let LayoutNode::Leaf(restarted_splint) = &restarted_window.root else {
            panic!("restarted persisted topology was not the exact leaf");
        };
        assert_eq!(restarted_splint.id, splint_id);
        assert_eq!(restarted_splint.title, splint_title);
        assert_eq!(restarted_splint.last_incarnation, Some(incarnation));
        assert!(matches!(
            restarted_splint.state,
            splinterm_core::SplintState::Exited(_)
        ));
        let Response::Splint {
            lair_id: inspected_lair_id,
            dojo_id: inspected_dojo_id,
            title: inspected_title,
            topology_revision: inspected_revision,
            runtime: inspected_runtime,
        } = restarted
            .request(Request::InspectSplint { splint_id })
            .await
        else {
            panic!("restorable Splint targeted inspection was not returned");
        };
        assert_eq!(inspected_lair_id, dojo.id);
        assert_eq!(inspected_dojo_id, dojo_id);
        assert_eq!(inspected_title, splint_title);
        assert_eq!(inspected_revision, snapshot.revision);
        assert_eq!(inspected_runtime, *restarted_runtime);
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
                record.operation != splinterm_protocol::AuditOperation::CreateLair
            })
        );
        daemon.shutdown();
    })
    .await
    .expect("headless policy integration timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_policy_snapshot_excludes_new_splint_until_reload() {
    time::timeout(Duration::from_secs(60), async {
        let daemon = Daemon::start_with_policy(&exact_headless_policy(None)).await;
        let mut bootstrap = daemon.connect().await;
        let revision = bootstrap.topology_revision().await;
        let Response::LairCreated { lair: dojo, .. } = bootstrap
            .request(Request::CreateLair {
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
            panic!("snapshot policy Lair was not created");
        };
        let lair_id = dojo.id;
        let LayoutNode::Leaf(original) = &dojo.dojos[0].root else {
            panic!("snapshot policy Lair was not a leaf");
        };
        let original_id = original.id;

        let policy = daemon.policy.as_ref().unwrap();
        fs::write(policy, parent_snapshot_policy(lair_id)).unwrap();
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
                incarnation: Some(incarnation),
                scrollback_rows: 1,
            })
            .await
            .expect_err("new descendant must not inherit the published parent snapshot");
        assert_eq!(denied.code, ErrorCode::Unauthorized);

        fs::write(policy, parent_snapshot_policy(lair_id)).unwrap();
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
        let Response::LairCreated { .. } = connection
            .request(Request::CreateLair {
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
            panic!("signal-resistant Lair was not created");
        };
        let child_pid = wait_for_pid_marker(&marker, "signal-resistant child").await;
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
    reason = "one lifecycle scenario compares existing and post-theme-change PTY environments"
)]
async fn new_splint_refreshes_the_active_omarchy_gum_environment() {
    time::timeout(TEST_TIMEOUT, async {
        let old_palette = r##"
hl.env("FOREGROUND", "#f4f4f4")
hl.env("BACKGROUND", "#1b1b1b")
hl.env("BORDER_FOREGROUND", "#808080")
hl.env("BORDER_BACKGROUND", "#1b1b1b")
hl.env("GUM_CHOOSE_SELECTED_BACKGROUND", "#808080")
hl.env("GUM_OLD_THEME_ONLY", "#f4f4f4")
"##;
        let current_palette = r##"
hl.env("FOREGROUND", "#afaaa2")
hl.env("BACKGROUND", "#0c1928")
hl.env("BORDER_FOREGROUND", "#d4bda2")
hl.env("BORDER_BACKGROUND", "#0c1928")
hl.env("GUM_CHOOSE_SELECTED_BACKGROUND", "#d4bda2")
"##;
        let daemon = Daemon::start_with_gum_environment(old_palette).await;
        let existing_ready = daemon.runtime.join("existing-gum-ready");
        let existing_trigger = daemon.runtime.join("existing-gum-trigger");
        let existing_marker = daemon.runtime.join("existing-gum-environment");
        let refreshed_marker = daemon.runtime.join("refreshed-gum-environment");
        let malformed_marker = daemon.runtime.join("malformed-gum-environment");
        let mut connection = daemon.connect().await;
        let Response::LairCreated { .. } = connection
            .request(Request::CreateLair {
                expected_topology_revision: TopologyRevision::default(),
                name: "old-gum-environment".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        format!(
                            "touch {}; while [ ! -e {} ]; do sleep 0.01; done; printf '%s|%s|%s' \"$BACKGROUND\" \"$GUM_CHOOSE_SELECTED_BACKGROUND\" \"${{GUM_OLD_THEME_ONLY-unset}}\" > {}",
                            existing_ready.display(),
                            existing_trigger.display(),
                            existing_marker.display()
                        ),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("existing Gum environment test Lair was not created");
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !existing_ready.exists() {
            assert!(
                Instant::now() < deadline,
                "existing Gum environment did not become ready"
            );
            time::sleep(Duration::from_millis(10)).await;
        }

        let theme = daemon.runtime.join("state/omarchy/current/theme");
        let replacement = theme.join("gum_env.lua.next");
        fs::write(&replacement, current_palette).unwrap();
        fs::rename(replacement, theme.join("gum_env.lua")).unwrap();
        let revision = connection.topology_revision().await;
        let Response::LairCreated { .. } = connection
            .request(Request::CreateLair {
                expected_topology_revision: revision,
                name: "current-gum-environment".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        format!(
                            "printf '%s|%s|%s' \"$BACKGROUND\" \"$GUM_CHOOSE_SELECTED_BACKGROUND\" \"${{GUM_OLD_THEME_ONLY-unset}}\" > {}",
                            refreshed_marker.display()
                        ),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("refreshed Gum environment test Lair was not created");
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !refreshed_marker.exists() {
            assert!(
                Instant::now() < deadline,
                "refreshed Gum environment marker timed out"
            );
            time::sleep(Duration::from_millis(10)).await;
        }

        let malformed = format!(
            "{current_palette}\nhl.env(\"GUM_FILTER_INDICATOR\", \"#ffffff\""
        );
        let replacement = theme.join("gum_env.lua.next");
        fs::write(&replacement, malformed).unwrap();
        fs::rename(replacement, theme.join("gum_env.lua")).unwrap();
        let revision = connection.topology_revision().await;
        let Response::LairCreated { .. } = connection
            .request(Request::CreateLair {
                expected_topology_revision: revision,
                name: "malformed-gum-environment".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        format!(
                            "printf '%s|%s|%s' \"$BACKGROUND\" \"$GUM_CHOOSE_SELECTED_BACKGROUND\" \"${{GUM_OLD_THEME_ONLY-unset}}\" > {}",
                            malformed_marker.display()
                        ),
                    ],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        else {
            panic!("malformed Gum environment test Lair was not created");
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !malformed_marker.exists() {
            assert!(
                Instant::now() < deadline,
                "malformed Gum environment marker timed out"
            );
            time::sleep(Duration::from_millis(10)).await;
        }

        fs::write(existing_trigger, b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !existing_marker.exists() {
            assert!(
                Instant::now() < deadline,
                "existing Gum environment marker timed out"
            );
            time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(
            fs::read_to_string(refreshed_marker).unwrap(),
            "#0c1928|#d4bda2|unset"
        );
        assert_eq!(
            fs::read_to_string(existing_marker).unwrap(),
            "#1b1b1b|#808080|#f4f4f4"
        );
        assert_eq!(
            fs::read_to_string(malformed_marker).unwrap(),
            "#1b1b1b|#808080|#f4f4f4"
        );
        daemon.shutdown();
    })
    .await
    .expect("Gum environment integration timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "one lifecycle scenario correlates create, restore, and new-Dojo context"
)]
async fn new_dojo_and_restore_inject_exact_current_context() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let restored_marker = daemon.runtime.join("restored-context");
        let dojo_marker = daemon.runtime.join("window-context");
        let context_launch = |marker: &Path| LaunchParameters {
            cwd: std::env::current_dir().unwrap(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                format!(
                    "printf '%s|%s|%s|%s' \"$SPLINTERM_LAIR_ID\" \"$SPLINTERM_DOJO_ID\" \"$SPLINTERM_SPLINT_ID\" \"$SPLINTERM_SPLINT_INCARNATION\" > {}",
                    marker.display()
                ),
            ],
            shell: None,
            login_shell: false,
            scrollback_lines: 100,
        };
        let mut connection = daemon.connect().await;
        let Response::LairCreated {
            lair: dojo,
            incarnation: first_incarnation,
            ..
        } = connection
            .request(Request::CreateLair {
                expected_topology_revision: TopologyRevision::default(),
                name: "context-lifecycle".into(),
                launch: context_launch(&restored_marker),
            })
            .await
        else {
            panic!("context test Lair was not created");
        };
        let lair_id = dojo.id;
        let first_dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(first) = &dojo.dojos[0].root else {
            panic!("context test Lair was not a leaf");
        };
        let first_id = first.id;
        let expected_first =
            format!("{lair_id}|{first_dojo_id}|{first_id}|{first_incarnation}");
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(fs::read_to_string(&restored_marker), Ok(ref value) if value == &expected_first) {
            assert!(Instant::now() < marker_deadline, "initial context marker timed out");
            time::sleep(Duration::from_millis(10)).await;
        }
        loop {
            let Response::Splint { runtime, .. } = connection
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
            format!("{lair_id}|{first_dojo_id}|{first_id}|{restored_incarnation}");
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(fs::read_to_string(&restored_marker), Ok(ref value) if value == &expected_restored) {
            assert!(Instant::now() < marker_deadline, "restored context marker timed out");
            time::sleep(Duration::from_millis(10)).await;
        }

        let revision = connection.topology_revision().await;
        let Response::DojoStarted {
            dojo_id,
            splint_id,
            incarnation,
            ..
        } = connection
            .request(Request::NewDojo {
                expected_topology_revision: revision,
                lair_id,
                name: "context-dojo".into(),
                launch: context_launch(&dojo_marker),
                promote_transient_lair: false,
            })
            .await
        else {
            panic!("context test Dojo was not created");
        };
        let expected_window = format!("{lair_id}|{dojo_id}|{splint_id}|{incarnation}");
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(fs::read_to_string(&dojo_marker), Ok(ref value) if value == &expected_window) {
            assert!(Instant::now() < marker_deadline, "Dojo context marker timed out");
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
        let Response::LairCreated { lair: dojo, .. } = connection
            .request(Request::CreateLair {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "durable".into(),
                launch,
            })
            .await
        else {
            panic!("dojo was not created");
        };
        let lair_id = dojo.id;
        let dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
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
        let primary = daemon.runtime.join("state/splinterm/topology.json");
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
                    .starts_with("topology.invalid-"))
        );
        assert_eq!(fs::read_to_string(&marker).unwrap(), "run\n");
        let mut restored = daemon.connect().await;
        let Response::Topology { snapshot } = restored.request(Request::InspectTopology).await
        else {
            panic!("restored topology was not returned");
        };
        snapshot.validate().unwrap();
        assert_eq!(snapshot.revision.get(), 1);
        let restored_dojo = snapshot.topology.lairs().next().unwrap();
        assert_eq!(restored_dojo.id, lair_id);
        assert_eq!(restored_dojo.dojos[0].id, dojo_id);
        let LayoutNode::Leaf(restored_splint) = &restored_dojo.dojos[0].root else {
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
    reason = "one scenario covers restore-one, restore-Dojo, restore-Lair, and partial results"
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
        let Response::LairCreated { lair: dojo, .. } = connection
            .request(Request::CreateLair {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "restore-scopes".into(),
                launch: launch.clone(),
            })
            .await
        else {
            panic!("dojo was not created");
        };
        let lair_id = dojo.id;
        let dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(first) = &dojo.dojos[0].root else {
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
        let Response::DojoStarted {
            splint_id: third_id,
            incarnation: third_incarnation,
            ..
        } = connection
            .request(Request::NewDojo {
                expected_topology_revision: topology_revision,
                lair_id,
                name: "second".into(),
                launch,
                promote_transient_lair: false,
            })
            .await
        else {
            panic!("second Dojo was not created");
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
            .request(Request::RestoreDojo {
                expected_topology_revision,
                dojo_id,
            })
            .await
        else {
            panic!("Dojo restore failed");
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
            .request(Request::RestoreLair {
                expected_topology_revision,
                lair_id,
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
        let first_request = Request::CreateLair {
            expected_topology_revision: snapshot.revision,
            name: "race-first".into(),
            launch: launch.clone(),
        };
        let second_request = Request::CreateLair {
            expected_topology_revision: snapshot.revision,
            name: "race-second".into(),
            launch: launch.clone(),
        };
        let (first_result, second_result) = tokio::join!(
            first.request_result(first_request),
            second.request_result(second_request)
        );
        let (dojo, stale) = match (first_result, second_result) {
            (Ok(Response::LairCreated { lair: dojo, .. }), Err(stale))
            | (Err(stale), Ok(Response::LairCreated { lair: dojo, .. })) => (dojo, stale),
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
                if change.revision.get() == 1 && change.kind == TopologyChangeKind::LairCreated
        ));

        let lair_id = dojo.id;
        let dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(initial) = &dojo.dojos[0].root else {
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
                    ancestor: 0,
                    ratio: SplitRatio::new(650).unwrap(),
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 3
        ));
        assert!(matches!(
            editor
                .request(Request::RenameLair {
                    expected_topology_revision: splinterm_core::TopologyRevision::new(3),
                    lair_id,
                    name: "renamed-dojo".into(),
                })
                .await,
            Response::TopologyCommitted { topology_revision } if topology_revision.get() == 4
        ));
        assert!(matches!(
            editor
                .request(Request::RenameDojo {
                    expected_topology_revision: splinterm_core::TopologyRevision::new(4),
                    dojo_id,
                    name: "main-dojo".into(),
                    promote_transient_lair: false,
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
        let Response::DojoStarted {
            dojo_id: extra_window,
            splint_id: extra_splint,
            incarnation: extra_incarnation,
            topology_revision,
        } = editor
            .request(Request::NewDojo {
                expected_topology_revision: splinterm_core::TopologyRevision::new(6),
                lair_id,
                name: "extra-dojo".into(),
                launch,
                promote_transient_lair: false,
            })
            .await
        else {
            panic!("new Dojo did not commit");
        };
        assert_eq!(topology_revision.get(), 7);
        let invalid_focus = editor
            .request_result(Request::SetDojoDefaultFocus {
                expected_topology_revision: topology_revision,
                dojo_id,
                splint_id: extra_splint,
            })
            .await
            .unwrap_err();
        assert_eq!(invalid_focus.code, ErrorCode::InvalidArgument);
        let Response::TopologyCommitted { topology_revision } = editor
            .request(Request::SetDojoDefaultFocus {
                expected_topology_revision: topology_revision,
                dojo_id,
                splint_id: sibling_id,
            })
            .await
        else {
            panic!("Dojo focus hint did not commit");
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
                .request(Request::CloseDojo {
                    expected_topology_revision: topology_revision,
                    dojo_id: extra_window,
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
            snapshot.topology.find_dojo(dojo_id).unwrap().default_focus,
            initial_id
        );
        assert_eq!(snapshot.runtimes.len(), 1);
        daemon.shutdown();
    })
    .await
    .expect("topology CAS scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn atomic_lair_termination_rejects_drift_and_removes_exact_runtime_set() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let mut client = daemon.connect().await;
        let launch = LaunchParameters {
            cwd: std::env::current_dir().unwrap(),
            command: vec!["/bin/sh".into(), "-c".into(), "exec sleep 30".into()],
            shell: None,
            login_shell: false,
            scrollback_lines: 100,
        };
        let Response::LairCreated {
            lair,
            incarnation,
            topology_revision,
        } = client
            .request(Request::CreateLair {
                expected_topology_revision: TopologyRevision::default(),
                name: "terminate-exact".into(),
                launch,
            })
            .await
        else {
            panic!("test Lair was not created");
        };
        let dojo_id = lair.dojos[0].id;
        let splint_id = lair.dojos[0].default_focus;
        let target = MutationTarget {
            lair_id: lair.id,
            dojo_id,
            splint_id,
            incarnation,
        };
        let mut stale = target.clone();
        stale.incarnation = stale.incarnation.saturating_add(1);
        let failure = client
            .request_result(Request::TerminateLair {
                expected_topology_revision: topology_revision,
                lair_id: lair.id,
                targets: vec![stale],
            })
            .await
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::InvalidArgument);
        let Response::Topology { snapshot } = client.request(Request::InspectTopology).await else {
            panic!("topology inspection failed");
        };
        assert_eq!(snapshot.revision, topology_revision);
        assert!(
            snapshot
                .topology
                .lairs()
                .any(|current| current.id == lair.id)
        );
        assert!(matches!(
            client
                .request(Request::TerminateLair {
                    expected_topology_revision: topology_revision,
                    lair_id: lair.id,
                    targets: vec![target],
                })
                .await,
            Response::TopologyCommitted { topology_revision: committed }
                if committed.get() == topology_revision.get() + 1
        ));
        let Response::Topology { snapshot } = client.request(Request::InspectTopology).await else {
            panic!("topology inspection failed");
        };
        assert!(
            snapshot
                .topology
                .lairs()
                .all(|current| current.id != lair.id)
        );
        assert!(
            snapshot
                .runtimes
                .iter()
                .all(|runtime| runtime.splint_id != splint_id)
        );
        daemon.shutdown();
    })
    .await
    .expect("atomic Lair termination scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_scrollback_search_pages_and_invalidates_stale_cursors() {
    time::timeout(TEST_TIMEOUT, async {
        let daemon = Daemon::start().await;
        let mut client = daemon.connect().await;
        let Response::LairCreated { lair: dojo, .. } = client
            .request(Request::CreateLair {
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
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
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
        let Response::SearchResults { page: first, .. } = client.request(search(None)).await else {
            panic!("daemon did not return first search page");
        };
        first.validate().unwrap();
        assert_eq!(first.matches.len(), 1);
        let cursor = first.next_cursor.clone().expect("older match cursor");
        let Response::SearchResults { page: second, .. } =
            client.request(search(Some(cursor))).await
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
        let Response::LairCreated { lair: dojo, .. } = admin
            .request(Request::CreateLair {
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
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            panic!("new dojo was not a leaf");
        };
        let splint_id = splint.id;
        let incarnation = admin.live_incarnation(splint_id).await;

        let mut owner = daemon.connect().await;
        let mut requester = daemon.connect().await;
        let owner_subscription = owner.subscribe_control(splint_id, incarnation).await;
        let requester_subscription = requester.subscribe_control(splint_id, incarnation).await;
        let owner_controller = owner.acquire_control(splint_id, incarnation).await;

        let Response::ControlTransferPending { transfer_id, .. } = requester
            .request(Request::RequestControlTransfer {
                splint_id,
                incarnation,
                modes: vec![ControlMode::Input, ControlMode::Resize],
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
            Response::ControlTransferDecided {
                outcome: ControlTransferOutcome::Denied,
                controller_id: None,
            }
        );
        assert!(matches!(
            requester.next_event(requester_subscription).await.1,
            SubscriptionEvent::ControlTransferResolved {
                transfer_id: seen,
                outcome: ControlTransferOutcome::Denied,
                controller_id: None,
            } if seen == transfer_id
        ));

        let Response::ControlTransferPending { transfer_id, .. } = requester
            .request(Request::RequestControlTransfer {
                splint_id,
                incarnation,
                modes: vec![ControlMode::Input, ControlMode::Resize],
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
        assert!(matches!(
            owner
                .request(Request::DecideControlTransfer {
                    transfer_id,
                    decision: ControlTransferDecision::Accept,
                })
                .await,
            Response::ControlTransferDecided {
                outcome: ControlTransferOutcome::Granted,
                controller_id: Some(_),
            }
        ));
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
        let Response::LairCreated { lair: dojo, .. } = admin
            .request(Request::CreateLair {
                expected_topology_revision: splinterm_core::TopologyRevision::default(),
                name: "multiplex".into(),
                launch: shell_launch(),
            })
            .await
        else {
            panic!("daemon did not create initial Splint");
        };
        let lair_id = dojo.id;
        let dojo_id = dojo.dojos[0].id;
        let LayoutNode::Leaf(first) = &dojo.dojos[0].root else {
            panic!("new dojo was not a leaf");
        };
        let first_id = first.id;
        let first_incarnation = admin.live_incarnation(first_id).await;
        admin.resize(first_id, first_incarnation, 91, 43).await;
        admin.release_control().await;

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
        let mut initial_size_probe = daemon.connect().await;
        let (_, second_initial) = initial_size_probe
            .attach(second_id, second_incarnation)
            .await;
        assert_eq!((second_initial.columns, second_initial.rows), (91, 43));
        assert!(
            second_initial.cursor_row <= 1,
            "new shell prompt started at row {} of {}",
            second_initial.cursor_row,
            second_initial.rows
        );
        drop(initial_size_probe);

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
        let context_command = b"printf 'CTX=%s|%s|%s|%s\\n' \"$SPLINTERM_LAIR_ID\" \"$SPLINTERM_DOJO_ID\" \"$SPLINTERM_SPLINT_ID\" \"$SPLINTERM_SPLINT_INCARNATION\"\n";
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
            "CTX={lair_id}|{dojo_id}|{first_id}|{first_incarnation}"
        )));
        assert!(second_text.contains(&format!(
            "CTX={lair_id}|{dojo_id}|{second_id}|{second_incarnation}"
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
                        "printf 'RELAUNCH=%s|%s|%s|%s\\n' \"$SPLINTERM_LAIR_ID\" \"$SPLINTERM_DOJO_ID\" \"$SPLINTERM_SPLINT_ID\" \"$SPLINTERM_SPLINT_INCARNATION\"; exec sleep 30".into(),
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
            "RELAUNCH={lair_id}|{dojo_id}|{first_id}|{relaunched_incarnation}"
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
async fn mixed_clear_subscription_reconstructs_exact_final_snapshot_without_resync() {
    time::timeout(Duration::from_secs(60), async {
        let daemon = Daemon::start().await;
        let mut creator = daemon.connect().await;
        let script = r"i=0; while [ $i -lt 5000 ]; do if [ $i -gt 0 ] && [ $((i % 500)) -eq 0 ]; then printf '\033[2J\033[H'; fi; case $((i % 3)) in 0) printf 'plan0043-%08d plain xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n' $i;; 1) printf '\033[3%dmplan0043-%08d ansi xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\033[0m\n' $((i % 8)) $i;; 2) printf 'plan0043-%08d unicode-naive-cafe-lambda-emoji\n' $i;; esac; i=$((i+1)); done; printf 'PLAN0043_%s\n' FINAL; sleep 30";
        let lair = match creator
            .request(Request::CreateLair {
                expected_topology_revision: TopologyRevision::default(),
                name: "plan0043-reconstruction".into(),
                launch: LaunchParameters {
                    cwd: std::env::current_dir().unwrap(),
                    command: vec!["/bin/sh".into()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 100,
                },
            })
            .await
        {
            Response::LairCreated { lair, .. } => lair,
            response => panic!("unexpected create response: {response:?}"),
        };
        let LayoutNode::Leaf(splint) = &lair.dojos[0].root else {
            unreachable!()
        };
        let splint_id = splint.id;
        let incarnation = creator.live_incarnation(splint_id).await;
        let mut subscriber = daemon.connect().await;
        let (subscription_id, mut reconstructed) = subscriber
            .attach_with_scrollback(splint_id, incarnation, 100)
            .await;

        let mut workload = script.as_bytes().to_vec();
        workload.push(b'\n');
        creator.input(splint_id, incarnation, &workload).await;
        let mut expected_sequence = 1_u64;
        let mut updates = 0_usize;
        loop {
            let (sequence, event) = subscriber.next_event(subscription_id).await;
            assert_eq!(sequence, expected_sequence);
            expected_sequence += 1;
            match event {
                SubscriptionEvent::Update { update } => {
                    apply_terminal_update(&mut reconstructed, update);
                    updates += 1;
                    if snapshot_text(&reconstructed).contains("PLAN0043_FINAL") {
                        break;
                    }
                }
                SubscriptionEvent::Snapshot { snapshot } => reconstructed = snapshot,
                SubscriptionEvent::ResyncRequired { current_revision } => {
                    panic!("fast subscriber required resync at revision {current_revision}")
                }
                SubscriptionEvent::Exited { .. } => {
                    panic!("terminal exited before final reconstruction")
                }
                event => panic!("unexpected terminal event: {event:?}"),
            }
        }
        assert!(updates > 1, "workload must cross publication boundaries");

        let mut observer = daemon.connect().await;
        let (observer_subscription, authoritative) = observer
            .attach_with_scrollback(splint_id, incarnation, 100)
            .await;
        assert_eq!(
            observer
                .request(Request::Detach {
                    subscription_id: observer_subscription,
                })
                .await,
            Response::Acknowledged
        );
        assert_eq!(reconstructed, authoritative);
        assert_eq!(
            subscriber
                .request(Request::Detach { subscription_id })
                .await,
            Response::Acknowledged
        );
        daemon.shutdown();
    })
    .await
    .expect("Plan 0043 reconstruction scenario timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "the single scenario intentionally records the complete Phase 8 lifecycle"
)]
async fn phase8_detach_reattach_overflow_resync_and_cleanup() {
    time::timeout(Duration::from_secs(120), async {
        let daemon = Daemon::start().await;
        let mut creator = daemon.connect().await;
        let cwd = std::env::current_dir().unwrap();
        let dojo = match creator
            .request(Request::CreateLair {
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
            Response::LairCreated { lair: dojo, .. } => dojo,
            response => panic!("unexpected create response: {response:?}"),
        };
        let LayoutNode::Leaf(model_splint) = &dojo.dojos[0].root else {
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
        let mut fast = daemon.connect().await;
        let (fast_subscription, _) = fast.attach(splint_id, incarnation).await;
        let fast_drain = tokio::spawn(async move {
            loop {
                match fast.next_event(fast_subscription).await.1 {
                    SubscriptionEvent::Update { update }
                        if update_text(&update).contains("overflow-finished") =>
                    {
                        break;
                    }
                    SubscriptionEvent::Snapshot { snapshot }
                        if snapshot_text(&snapshot).contains("overflow-finished") =>
                    {
                        break;
                    }
                    SubscriptionEvent::ResyncRequired { .. } => {
                        panic!("drained subscriber required resynchronization")
                    }
                    SubscriptionEvent::Exited { .. } => {
                        panic!("terminal exited before the overflow marker")
                    }
                    _ => {}
                }
            }
        });
        let mut producer = daemon.connect().await;
        // First prove that an actively drained subscriber keeps up with paced
        // producer frames. Slow-runner scheduling must not be conflated with the
        // separate unread-connection overflow proof below.
        producer
            .input(
                splint_id,
                incarnation,
                b"i=0; while [ $i -lt 2000 ]; do limit=$((i+20)); while [ $i -lt $limit ]; do printf 'paced-%05d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' $i; i=$((i+1)); done; sleep 0.01; done; printf 'overflow-finished\\n'\n",
            )
            .await;
        let _completion_snapshot = snapshot_until_with_timeout(
            &mut reattached,
            splint_id,
            incarnation,
            "overflow-finished",
            Duration::from_secs(60),
        )
        .await;
        time::timeout(Duration::from_secs(30), fast_drain)
            .await
            .expect("drained subscriber did not observe the overflow marker")
            .expect("drained subscriber task failed");
        let _pre_pressure_snapshot = stable_snapshot_after_marker(
            &mut reattached,
            splint_id,
            incarnation,
            "overflow-finished",
        )
        .await;

        // The actively drained client has exited. Create a fresh unread
        // MAX_SUBSCRIPTIONS connection so the following unpaced pressure stream
        // must resolve through exact coalesced delivery, resynchronization, or
        // disconnection.
        let mut slow = daemon.connect().await;
        nix::sys::socket::setsockopt(
            &slow.stream,
            nix::sys::socket::sockopt::RcvBuf,
            &4096,
        )
        .unwrap();
        let mut slow_subscriptions = std::collections::BTreeMap::new();
        for _ in 0..MAX_SUBSCRIPTIONS {
            let (subscription_id, snapshot) = slow.attach(splint_id, incarnation).await;
            assert!(slow_subscriptions
                .insert(subscription_id, (snapshot, 1_u64))
                .is_none());
        }
        producer
            .input(
                splint_id,
                incarnation,
                b"i=0; while [ $i -lt 30000 ]; do printf 'pressure-%05d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n' $i; i=$((i+1)); done; printf 'pressure-finished\\n'\n",
            )
            .await;
        let _pressure_snapshot = snapshot_until_with_timeout(
            &mut reattached,
            splint_id,
            incarnation,
            "pressure-finished",
            Duration::from_secs(60),
        )
        .await;
        let final_snapshot = stable_snapshot_after_marker(
            &mut reattached,
            splint_id,
            incarnation,
            "pressure-finished",
        )
        .await;

        let mut caught_up = false;
        let mut saw_resync = false;
        let mut disconnected = false;
        let slow_read_deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < slow_read_deadline {
            let Ok(frame) = time::timeout(
                slow_read_deadline.saturating_duration_since(Instant::now()),
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
            let ServerFrame::Event {
                subscription_id,
                sequence,
                event,
            } = frame
            else {
                continue;
            };
            let (reconstructed, expected_sequence) = slow_subscriptions
                .get_mut(&subscription_id)
                .expect("slow connection emitted an event for an unknown subscription");
            match event {
                SubscriptionEvent::Update { update } => {
                    assert_eq!(sequence, *expected_sequence);
                    *expected_sequence += 1;
                    apply_terminal_update(reconstructed, update);
                    if reconstructed.revision == final_snapshot.revision
                        && snapshot_text(reconstructed).contains("pressure-finished")
                    {
                        assert_eq!(&*reconstructed, &final_snapshot);
                        caught_up = true;
                        break;
                    }
                }
                SubscriptionEvent::Snapshot { snapshot } => {
                    assert_eq!(sequence, *expected_sequence);
                    *expected_sequence += 1;
                    snapshot.validate().expect("slow subscriber snapshot is valid");
                    assert_eq!(snapshot.splint_id, splint_id);
                    assert_eq!(snapshot.incarnation, incarnation);
                    *reconstructed = snapshot;
                    if reconstructed.revision == final_snapshot.revision
                        && snapshot_text(reconstructed).contains("pressure-finished")
                    {
                        assert_eq!(&*reconstructed, &final_snapshot);
                        caught_up = true;
                        break;
                    }
                }
                SubscriptionEvent::ResyncRequired { .. } => {
                    assert!(sequence >= *expected_sequence);
                    saw_resync = true;
                    break;
                }
                _ => {
                    assert_eq!(sequence, *expected_sequence);
                    *expected_sequence += 1;
                }
            }
        }
        assert!(
            caught_up || saw_resync || disconnected,
            "slow subscriber neither received final state, required resync, nor disconnected"
        );
        drop(producer);
        assert!(final_snapshot.revision > detached.revision);

        let mut before_row_id = final_snapshot
            .scrollback_rows
            .first()
            .and_then(|row| row.row_id)
            .expect("overflow produced paged history");
        let mut paged_ids = std::collections::BTreeSet::new();
        for _ in 0..4 {
            let response = reattached
                .request(Request::ScrollbackPage {
                    splint_id,
                    incarnation,
                    terminal_revision: final_snapshot.revision,
                    history_generation: final_snapshot.history_generation,
                    before_row_id,
                    max_rows: splinterm_protocol::MAX_SCROLLBACK_PAGE_ROWS,
                })
                .await;
            let Response::ScrollbackPage { page, .. } = response else {
                panic!("daemon did not return a scrollback page: {response:?}");
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
