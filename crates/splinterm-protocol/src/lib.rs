//! Versioned, transport-independent messages exchanged over the local socket.
//!
//! Terminal DTOs in this crate are intentionally distinct from the borrowed
//! `splinterm-terminal` and daemon runtime representations.

pub mod perf_trace;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use splinterm_core::{
    Axis, DojoId, Lair, LairId, SplintId, SplitRatio, SplitSide, Topology, TopologyRevision,
};

pub const PROTOCOL_VERSION: u16 = 31;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SNAPSHOT_SCROLLBACK_ROWS: usize = 16;
pub const MAX_SCROLLBACK_PAGE_ROWS: usize = 16;
pub const MAX_SEARCH_QUERY_BYTES: usize = 256;
pub const MAX_SEARCH_RESULTS: usize = 64;
pub const MAX_AUDIT_PAGE_RECORDS: usize = 128;
pub const MAX_SEARCH_PREVIEW_BYTES: usize = 256;
pub const MAX_SEARCH_CURSOR_BYTES: usize = 32;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_LAUNCH_ARGUMENTS: usize = 256;
pub const MAX_LAUNCH_ARGUMENT_BYTES: usize = 4096;
pub const MAX_CWD_BYTES: usize = 4096;
pub const MAX_SCROLLBACK_LINES: usize = 1_000_000;
pub const MAX_COLUMNS: u16 = 240;
pub const MAX_ROWS: u16 = 80;
pub const MAX_OUTSTANDING_REQUESTS: usize = 1;
pub const MAX_SUBSCRIPTIONS: usize = 4;
pub const MAX_UPDATE_ROW_PATCHES: usize = MAX_ROWS as usize;
pub const MAX_UPDATE_SCROLLS: usize = MAX_ROWS as usize;
pub const MAX_CONSENT_FRAME_BYTES: usize = 16 * 1024;
pub const CONSENT_CAPABILITY_BYTES: usize = 32;
pub const MAX_ACCESS_SCOPES: usize = 16;
pub const MAX_IMAGE_CONTENT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_IMAGE_BYTES_PER_SPLINT: usize = 32 * 1024 * 1024;
pub const MAX_IMAGE_BYTES_PER_DAEMON: usize = 64 * 1024 * 1024;
pub const MAX_IMAGE_CONTENTS_PER_SPLINT: usize = 64;
pub const MAX_IMAGE_PLACEMENTS_PER_SPLINT: usize = 256;
pub const MAX_IMAGE_DIMENSION: u32 = 4096;
pub const MAX_IMAGE_PIXELS: usize = 4_194_304;
pub const MAX_IMAGE_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_IMAGE_CHUNK_WINDOW: usize = 4;
pub const MAX_IMAGE_TRANSFERS_PER_SPLINT: usize = 2;
pub const MAX_IMAGE_TRANSFERS_PER_DAEMON: usize = 4;
pub const IMAGE_TRANSFER_TOKEN_BYTES: usize = 32;
pub const IMAGE_TRANSFER_TOKEN_TTL_MILLIS: u32 = 5_000;

#[must_use]
pub fn image_content_socket_path(control_socket: &Path) -> PathBuf {
    let mut name = control_socket.as_os_str().to_os_string();
    name.push(".content");
    PathBuf::from(name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientRole {
    TrustedUi,
    RemoteInteractive,
    Automation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticCorrelation {
    pub client_instance_id: uuid::Uuid,
    pub window_id: uuid::Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDiagnosticComponent {
    Splinterd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDiagnosticLevel {
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonDiagnosticEventCode {
    LairCreated,
    SplintSplit,
    DojoCreated,
    SplintClosed,
    DojoClosed,
    LairTerminated,
    LairRenamed,
    DojoRenamed,
    TopologyRestored,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonDiagnosticEvent {
    pub schema_version: u16,
    pub timestamp_unix_ms: u64,
    pub component: DaemonDiagnosticComponent,
    pub event: DaemonDiagnosticEventCode,
    pub level: DaemonDiagnosticLevel,
    pub pid: u32,
    pub client_instance_id: uuid::Uuid,
    pub window_id: uuid::Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topology_revision: Option<TopologyRevision>,
    pub build_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Hello {
        minimum_version: u16,
        maximum_version: u16,
        role: ClientRole,
    },
    Request {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostic_correlation: Option<DiagnosticCorrelation>,
        request: Request,
    },
    Cancel {
        request_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Hello {
        version: u16,
        limits: ServerLimits,
        development_terminal_access: bool,
    },
    Response {
        request_id: u64,
        result: Response,
    },
    Event {
        subscription_id: u64,
        sequence: u64,
        event: SubscriptionEvent,
    },
    Error {
        request_id: Option<u64>,
        error: ProtocolError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageServerCapabilities {
    pub metadata_version: u16,
    pub binary_chunks: bool,
    pub sealed_memfd: bool,
    pub maximum_content_bytes: usize,
    pub maximum_bytes_per_splint: usize,
    pub maximum_bytes_per_daemon: usize,
    pub maximum_contents_per_splint: usize,
    pub maximum_placements_per_splint: usize,
    pub maximum_dimension: u32,
    pub maximum_pixels: usize,
    pub maximum_chunk_bytes: usize,
    pub maximum_chunk_window: usize,
    pub maximum_transfers_per_splint: usize,
    pub maximum_transfers_per_daemon: usize,
}

impl Default for ImageServerCapabilities {
    fn default() -> Self {
        Self {
            metadata_version: 1,
            binary_chunks: true,
            sealed_memfd: cfg!(target_os = "linux"),
            maximum_content_bytes: MAX_IMAGE_CONTENT_BYTES,
            maximum_bytes_per_splint: MAX_IMAGE_BYTES_PER_SPLINT,
            maximum_bytes_per_daemon: MAX_IMAGE_BYTES_PER_DAEMON,
            maximum_contents_per_splint: MAX_IMAGE_CONTENTS_PER_SPLINT,
            maximum_placements_per_splint: MAX_IMAGE_PLACEMENTS_PER_SPLINT,
            maximum_dimension: MAX_IMAGE_DIMENSION,
            maximum_pixels: MAX_IMAGE_PIXELS,
            maximum_chunk_bytes: MAX_IMAGE_CHUNK_BYTES,
            maximum_chunk_window: MAX_IMAGE_CHUNK_WINDOW,
            maximum_transfers_per_splint: MAX_IMAGE_TRANSFERS_PER_SPLINT,
            maximum_transfers_per_daemon: MAX_IMAGE_TRANSFERS_PER_DAEMON,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerLimits {
    pub maximum_frame_bytes: usize,
    pub maximum_input_bytes: usize,
    pub maximum_columns: u16,
    pub maximum_rows: u16,
    pub maximum_outstanding_requests: usize,
    pub maximum_subscriptions: usize,
    pub maximum_snapshot_scrollback_rows: usize,
    pub image: ImageServerCapabilities,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            maximum_frame_bytes: MAX_FRAME_BYTES,
            maximum_input_bytes: MAX_INPUT_BYTES,
            maximum_columns: MAX_COLUMNS,
            maximum_rows: MAX_ROWS,
            maximum_outstanding_requests: MAX_OUTSTANDING_REQUESTS,
            maximum_subscriptions: MAX_SUBSCRIPTIONS,
            maximum_snapshot_scrollback_rows: MAX_SNAPSHOT_SCROLLBACK_ROWS,
            image: ImageServerCapabilities::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationLaunch {
    pub cwd: Option<PathBuf>,
    /// Direct executable plus argv. Empty selects the daemon-owned default shell.
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MutationPreflight {
    CreateLair,
    SplitSplint {
        splint_id: SplintId,
    },
    NewDojo {
        lair_id: LairId,
    },
    RelaunchSplint {
        splint_id: SplintId,
    },
    RestoreSplint {
        splint_id: SplintId,
    },
    RestoreDojo {
        dojo_id: DojoId,
    },
    RestoreLair {
        lair_id: LairId,
    },
    CloseSplint {
        splint_id: SplintId,
    },
    CloseDojo {
        dojo_id: DojoId,
    },
    TerminateLair {
        lair_id: LairId,
    },
    KillSplint {
        splint_id: SplintId,
        incarnation: u64,
    },
    SetSplitRatio {
        splint_id: SplintId,
    },
    RenameLair {
        lair_id: LairId,
    },
    RenameDojo {
        dojo_id: DojoId,
    },
    RenameSplint {
        splint_id: SplintId,
    },
    SetDojoDefaultFocus {
        dojo_id: DojoId,
        splint_id: SplintId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationTarget {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub splint_id: SplintId,
    /// Current incarnation when live, otherwise the last durable incarnation.
    pub incarnation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationPreparation {
    pub topology_revision: TopologyRevision,
    pub lair_id: Option<LairId>,
    pub dojo_id: Option<DojoId>,
    pub splint_id: Option<SplintId>,
    pub incarnation: Option<u64>,
    /// Exact bounded expansion for aggregate restore validation.
    pub targets: Vec<MutationTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    ListLairs,
    InspectTopology,
    SubscribeTopology,
    InspectSplint {
        splint_id: SplintId,
    },
    ReadGraphicalFocus,
    PublishGraphicalFocus {
        focused_splint_id: Option<SplintId>,
    },
    RequestAccess {
        splint_id: SplintId,
        incarnation: u64,
        scopes: Vec<AccessScope>,
    },
    RequestLairAccess {
        lair_id: LairId,
        scopes: Vec<AccessScope>,
    },
    AuthorizationStatus {
        splint_id: SplintId,
        incarnation: Option<u64>,
    },
    RevokeAccess {
        grant_id: u64,
    },
    PrepareMutation {
        mutation: MutationPreflight,
    },
    CreateLairAutomation {
        expected_topology_revision: TopologyRevision,
        name: String,
        launch: AutomationLaunch,
    },
    SplitSplintAutomation {
        expected_topology_revision: TopologyRevision,
        target_splint_id: SplintId,
        axis: Axis,
        side: SplitSide,
        ratio: SplitRatio,
        launch: AutomationLaunch,
    },
    RelaunchSplintAutomation {
        expected_topology_revision: TopologyRevision,
        splint_id: SplintId,
        launch: AutomationLaunch,
    },
    NewDojoAutomation {
        expected_topology_revision: TopologyRevision,
        lair_id: LairId,
        name: String,
        launch: AutomationLaunch,
    },
    CreateLair {
        expected_topology_revision: TopologyRevision,
        name: String,
        launch: LaunchParameters,
    },
    CreateTransientLair {
        expected_topology_revision: TopologyRevision,
        name: String,
        launch: LaunchParameters,
    },
    SplitSplint {
        expected_topology_revision: TopologyRevision,
        target_splint_id: SplintId,
        axis: Axis,
        side: SplitSide,
        ratio: SplitRatio,
        launch: LaunchParameters,
    },
    RelaunchSplint {
        expected_topology_revision: TopologyRevision,
        splint_id: SplintId,
        launch: LaunchParameters,
    },
    RestoreSplint {
        expected_topology_revision: TopologyRevision,
        splint_id: SplintId,
    },
    RestoreDojo {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
    },
    RestoreLair {
        expected_topology_revision: TopologyRevision,
        lair_id: LairId,
    },
    CloseSplint {
        expected_topology_revision: TopologyRevision,
        splint_id: SplintId,
    },
    SetSplitRatio {
        expected_topology_revision: TopologyRevision,
        target_splint_id: SplintId,
        /// Zero selects the immediate parent; larger values select successively
        /// older ancestors of the target leaf.
        ancestor: u16,
        ratio: SplitRatio,
    },
    NewDojo {
        expected_topology_revision: TopologyRevision,
        lair_id: LairId,
        name: String,
        launch: LaunchParameters,
    },
    CloseDojo {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
    },
    TerminateLair {
        expected_topology_revision: TopologyRevision,
        lair_id: LairId,
        /// Exact captured membership and process incarnations for drift rejection.
        targets: Vec<MutationTarget>,
    },
    RenameLair {
        expected_topology_revision: TopologyRevision,
        lair_id: LairId,
        name: String,
    },
    RenameDojo {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
        name: String,
    },
    SetDojoDefaultFocus {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
        splint_id: SplintId,
    },
    RenameSplint {
        expected_topology_revision: TopologyRevision,
        splint_id: SplintId,
        title: String,
    },
    Attach {
        splint_id: SplintId,
        /// Exact incarnation, or `None` to bind the current incarnation before authorization.
        incarnation: Option<u64>,
        scrollback_rows: usize,
    },
    RequestImageContent {
        request: ImageContentRequest,
    },
    StartScrollbackPage {
        splint_id: SplintId,
        /// Exact incarnation, or `None` to bind the current incarnation before authorization.
        incarnation: Option<u64>,
        max_rows: usize,
    },
    ScrollbackPage {
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
        before_row_id: u64,
        max_rows: usize,
    },
    StartSearchScrollback {
        splint_id: SplintId,
        /// Exact incarnation, or `None` to bind the current incarnation before authorization.
        incarnation: Option<u64>,
        query: String,
        case_sensitive: bool,
        max_results: usize,
    },
    SearchScrollback {
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
        query: String,
        case_sensitive: bool,
        cursor: Option<String>,
        max_results: usize,
    },
    AcquireControl {
        splint_id: SplintId,
        incarnation: u64,
        modes: Vec<ControlMode>,
    },
    SubscribeControl {
        splint_id: SplintId,
        incarnation: u64,
    },
    RequestControlTransfer {
        splint_id: SplintId,
        incarnation: u64,
        modes: Vec<ControlMode>,
    },
    DecideControlTransfer {
        transfer_id: u64,
        decision: ControlTransferDecision,
    },
    ForceControlTransfer {
        splint_id: SplintId,
        incarnation: u64,
    },
    ReleaseControl {
        controller_id: u64,
    },
    Input {
        controller_id: u64,
        splint_id: SplintId,
        incarnation: u64,
        bytes: Vec<u8>,
    },
    Resize {
        controller_id: u64,
        splint_id: SplintId,
        incarnation: u64,
        columns: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    Detach {
        subscription_id: u64,
    },
    KillSplint {
        splint_id: SplintId,
        incarnation: u64,
    },
    AuditInspect {
        after_audit_id: Option<u64>,
        max_records: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    MutationPrepared {
        preparation: MutationPreparation,
    },
    Lairs {
        lairs: Vec<Lair>,
        topology_revision: TopologyRevision,
    },
    LairCreated {
        lair: Lair,
        incarnation: u64,
        topology_revision: TopologyRevision,
    },
    SplintStarted {
        splint_id: SplintId,
        incarnation: u64,
        topology_revision: TopologyRevision,
    },
    DojoStarted {
        dojo_id: DojoId,
        splint_id: SplintId,
        incarnation: u64,
        topology_revision: TopologyRevision,
    },
    TopologyCommitted {
        topology_revision: TopologyRevision,
    },
    RestoreCompleted {
        topology_revision: TopologyRevision,
        results: Vec<RestoreLeafResult>,
    },
    Topology {
        snapshot: TopologySnapshot,
    },
    TopologySubscribed {
        subscription_id: u64,
        snapshot: TopologySnapshot,
    },
    Splint {
        lair_id: LairId,
        dojo_id: DojoId,
        title: String,
        topology_revision: TopologyRevision,
        runtime: SplintRuntimeSummary,
    },
    GraphicalFocus {
        focused_splint_id: Option<SplintId>,
        cwd: Option<PathBuf>,
    },
    AccessGranted {
        lair_id: LairId,
        dojo_id: DojoId,
        authorization_revision: u64,
        grant: AccessGrant,
    },
    LairAccessGranted {
        topology_revision: TopologyRevision,
        authorization_revision: u64,
        grant: LairAccessGrant,
    },
    AccessRevoked {
        lair_id: LairId,
        dojo_id: DojoId,
        authorization_revision: u64,
        grant: AccessGrant,
    },
    LairAccessRevoked {
        topology_revision: TopologyRevision,
        authorization_revision: u64,
        grant: LairAccessGrant,
    },
    AuthorizationStatus {
        lair_id: LairId,
        dojo_id: DojoId,
        incarnation: u64,
        topology_revision: TopologyRevision,
        policy_generation: u64,
        grants: Vec<AccessGrant>,
        lair_grants: Vec<LairAccessGrant>,
        persistent: Vec<PersistentAuthorizationStatus>,
        development_bypass: bool,
    },
    Attached {
        subscription_id: u64,
        provenance: TerminalProvenance,
        snapshot: TerminalSnapshot,
    },
    ImageContentReady {
        transfer: ImageContentTransfer,
    },
    ScrollbackPage {
        provenance: TerminalProvenance,
        page: ScrollbackPage,
    },
    ScrollbackResyncRequired {
        provenance: TerminalProvenance,
        current_revision: u64,
        history_generation: u64,
    },
    SearchResults {
        provenance: TerminalProvenance,
        page: SearchPage,
    },
    SearchResyncRequired {
        provenance: TerminalProvenance,
        current_revision: u64,
        history_generation: u64,
    },
    ControlGranted {
        controller_id: u64,
        lair_id: LairId,
        dojo_id: DojoId,
    },
    ControlSubscribed {
        subscription_id: u64,
        status: ControlStatus,
    },
    ControlTransferPending {
        transfer_id: u64,
        lair_id: LairId,
        dojo_id: DojoId,
    },
    ControlTransferDecided {
        outcome: ControlTransferOutcome,
        controller_id: Option<u64>,
    },
    AuditPage {
        page: AuditPage,
    },
    TerminalActionAcknowledged {
        lair_id: LairId,
        dojo_id: DojoId,
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
    },
    Acknowledged,
    SplintKilled {
        splint_id: SplintId,
        incarnation: u64,
        exit_status: ProcessExitStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubscriptionEvent {
    Snapshot {
        snapshot: TerminalSnapshot,
    },
    Update {
        update: TerminalUpdate,
    },
    ResyncRequired {
        current_revision: u64,
    },
    AccessRevoked {
        grant_id: u64,
    },
    ControlStatusChanged {
        status: ControlStatus,
    },
    ControlTransferRequested {
        transfer_id: u64,
    },
    ControlTransferResolved {
        transfer_id: u64,
        outcome: ControlTransferOutcome,
        controller_id: Option<u64>,
    },
    TopologyChanged {
        change: TopologyChange,
    },
    TopologyResyncRequired {
        current_revision: TopologyRevision,
    },
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyChange {
    pub revision: TopologyRevision,
    pub kind: TopologyChangeKind,
    pub snapshot: TopologySnapshot,
}

impl TopologyChange {
    /// Validates the change/snapshot correlation.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when revisions or snapshot identities disagree.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.snapshot.validate()?;
        if self.revision != self.snapshot.revision {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "topology change revision does not match its snapshot",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    Input,
    Resize,
}

/// Validates a nonempty, duplicate-free public controller mode set.
///
/// # Errors
/// Returns `InvalidArgument` when no mode, a duplicate, or more than two modes are present.
pub fn validate_control_modes(modes: &[ControlMode]) -> Result<(), ProtocolError> {
    let valid = (1..=2).contains(&modes.len()) && !(modes.len() == 2 && modes[0] == modes[1]);
    if !valid {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "invalid controller modes",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlTransferDecision {
    Accept,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlTransferOutcome {
    Granted,
    Denied,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlStatus {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub controlled: bool,
    pub locally_owned: bool,
}

impl ControlStatus {
    /// Validates a subscriber-specific control status snapshot.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for malformed identity or ownership state.
    pub fn validate(self) -> Result<(), ProtocolError> {
        if self.incarnation == 0 || (self.locally_owned && !self.controlled) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "invalid control status",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyChangeKind {
    LairCreated,
    SplintSplit,
    SplintClosed,
    SplitRatioChanged,
    DojoCreated,
    DojoClosed,
    LairTerminated,
    LairRenamed,
    DojoRenamed,
    DojoDefaultFocusChanged,
    SplintRenamed,
    RuntimeChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchParameters {
    pub cwd: PathBuf,
    /// Direct executable plus argv. Empty selects `shell` or the default shell.
    pub command: Vec<String>,
    pub shell: Option<String>,
    pub login_shell: bool,
    pub scrollback_lines: usize,
}

impl AutomationLaunch {
    /// Validates bounded structured automation launch input without resolving defaults.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when an explicit path or argv exceeds protocol bounds.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let argv_bytes = self
            .argv
            .iter()
            .try_fold(0_usize, |total, item| total.checked_add(item.len()));
        let valid = self.cwd.as_ref().is_none_or(|cwd| {
            cwd.is_absolute()
                && !cwd.as_os_str().is_empty()
                && !cwd.as_os_str().as_encoded_bytes().contains(&0)
                && cwd.as_os_str().as_encoded_bytes().len() <= MAX_CWD_BYTES
        }) && self.argv.len() <= MAX_LAUNCH_ARGUMENTS
            && self
                .argv
                .iter()
                .all(|item| !item.contains('\0') && item.len() <= MAX_LAUNCH_ARGUMENT_BYTES)
            && self.argv.first().is_none_or(|program| !program.is_empty())
            && argv_bytes.is_some_and(|bytes| bytes <= MAX_INPUT_BYTES);
        if !valid {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "automation launch parameters exceed protocol limits",
            ));
        }
        Ok(())
    }
}

impl LaunchParameters {
    /// Validates wire allocation and launch-policy bounds.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when any field exceeds protocol limits.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let command_bytes = self
            .command
            .iter()
            .try_fold(0_usize, |total, item| total.checked_add(item.len()));
        let valid = !self.cwd.as_os_str().is_empty()
            && !self.cwd.as_os_str().as_encoded_bytes().contains(&0)
            && self.cwd.as_os_str().as_encoded_bytes().len() <= MAX_CWD_BYTES
            && self.command.len() <= MAX_LAUNCH_ARGUMENTS
            && self
                .command
                .iter()
                .all(|item| !item.contains('\0') && item.len() <= MAX_LAUNCH_ARGUMENT_BYTES)
            && self
                .command
                .first()
                .is_none_or(|program| !program.is_empty())
            && command_bytes.is_some_and(|bytes| bytes <= MAX_INPUT_BYTES)
            && self
                .shell
                .as_ref()
                .is_none_or(|shell| !shell.is_empty() && shell.len() <= MAX_LAUNCH_ARGUMENT_BYTES)
            && self.scrollback_lines <= MAX_SCROLLBACK_LINES;
        if !valid {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "launch parameters exceed protocol limits",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologySnapshot {
    pub revision: TopologyRevision,
    pub topology: Topology,
    pub runtimes: Vec<SplintRuntimeSummary>,
}

impl TopologySnapshot {
    /// Validates topology/runtime identity correlation.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for duplicate, stale, or unreachable runtime entries.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let mut identities = std::collections::HashSet::new();
        let runtime_count = self
            .topology
            .lairs()
            .flat_map(|lair| &lair.dojos)
            .map(|dojo| dojo.root.splint_count())
            .sum::<usize>();
        let valid = self.revision == self.topology.revision()
            && runtime_count == self.runtimes.len()
            && self.runtimes.iter().all(|runtime| {
                identities.insert(runtime.splint_id)
                    && self
                        .topology
                        .find_splint(runtime.splint_id)
                        .is_some_and(|splint| splint.last_incarnation == runtime.last_incarnation)
                    && runtime.validate()
            });
        if !valid {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "topology runtime metadata is inconsistent",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreLeafResult {
    pub splint_id: SplintId,
    pub incarnation: Option<u64>,
    pub error: Option<ProtocolError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplintRuntimeSummary {
    pub splint_id: SplintId,
    pub live_incarnation: Option<u64>,
    pub last_incarnation: Option<u64>,
    pub restorable: bool,
    pub lifecycle: SplintLifecycle,
    pub exit_status: Option<ProcessExitStatus>,
}

impl SplintRuntimeSummary {
    #[must_use]
    pub fn validate(&self) -> bool {
        let valid_last = self.last_incarnation.is_none_or(|value| value > 0);
        match self.lifecycle {
            SplintLifecycle::Starting => {
                !self.restorable
                    && valid_last
                    && self.live_incarnation.is_none_or(|value| value > 0)
                    && self.exit_status.is_none()
            }
            SplintLifecycle::Running => {
                !self.restorable
                    && valid_last
                    && self.live_incarnation.is_some_and(|value| value > 0)
                    && self.live_incarnation == self.last_incarnation
                    && self.exit_status.is_none()
            }
            SplintLifecycle::Exited => valid_last && self.live_incarnation.is_none(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplintLifecycle {
    Starting,
    Running,
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessExitStatus {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_topology_revision: Option<TopologyRevision>,
}

impl ProtocolError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            current_topology_revision: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationScope {
    TopologyMetadataRead,
    TopologySubscribe,
    TerminalVisibleRead,
    TerminalSubscribe,
    ScrollbackRead,
    ScrollbackSearch,
    ControllerAcquire,
    ControllerTransfer,
    Input,
    Resize,
    ProcessSpawn,
    ProcessRestore,
    ProcessTerminate,
    TopologyLayoutMutate,
    TopologyNameMutate,
    AuthorizationInspect,
    AuthorizationRevoke,
    AuditInspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOperation {
    Ping,
    RequestAccess,
    RequestLairAccess,
    AuthorizationStatus,
    RevokeAccess,
    ListLairs,
    InspectTopology,
    SubscribeTopology,
    InspectSplint,
    ReadGraphicalFocus,
    PublishGraphicalFocus,
    CreateLair,
    SplitSplint,
    RelaunchSplint,
    RestoreSplint,
    RestoreDojo,
    RestoreLair,
    CloseSplint,
    SetSplitRatio,
    NewDojo,
    CloseDojo,
    TerminateLair,
    RenameLair,
    RenameDojo,
    SetDojoDefaultFocus,
    RenameSplint,
    Attach,
    ScrollbackPage,
    SearchScrollback,
    AcquireControl,
    SubscribeControl,
    RequestControlTransfer,
    DecideControlTransfer,
    ForceControlTransfer,
    ReleaseControl,
    Input,
    Resize,
    Detach,
    KillSplint,
    ProcessExit,
    AuditInspect,
    PolicyReload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecision {
    Allowed,
    Denied,
    Revoked,
    Expired,
    Matched,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditPeer {
    pub uid: u32,
    pub executable_path: String,
    pub executable_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditResource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lair_id: Option<LairId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dojo_id: Option<DojoId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub splint_id: Option<SplintId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incarnation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub schema: String,
    pub retention: String,
    pub audit_id: u64,
    pub unix_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_rule_id: Option<String>,
    pub peer: AuditPeer,
    pub operation: AuditOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<AuditResource>,
    pub requested_scopes: Vec<AutomationScope>,
    pub decision: AuditDecision,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AuditOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_basename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditPage {
    pub records: Vec<AuditRecord>,
    pub retention_gap: bool,
    pub oldest_available_audit_id: Option<u64>,
    pub newest_available_audit_id: Option<u64>,
    pub next_after_audit_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessScope {
    Observe,
    Scrollback,
    Input,
    Resize,
    ClipboardRead,
    ClipboardWrite,
    Terminate,
    ControlTakeover,
    TopologyObserve,
    TopologyLayout,
    TopologyName,
    ProcessSpawn,
    ProcessRestore,
}

impl AccessScope {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Observe => "observe visible terminal",
            Self::Scrollback => "read scrollback",
            Self::Input => "send input",
            Self::Resize => "resize terminal",
            Self::ClipboardRead => "read clipboard metadata",
            Self::ClipboardWrite => "write clipboard metadata",
            Self::Terminate => "terminate process",
            Self::ControlTakeover => "take over terminal control",
            Self::TopologyObserve => "inspect this Lair topology",
            Self::TopologyLayout => "change this Lair layout",
            Self::TopologyName => "rename this Lair and its sessions",
            Self::ProcessSpawn => "start processes in this Lair",
            Self::ProcessRestore => "restore processes in this Lair",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConsentTarget {
    Splint {
        splint_id: SplintId,
        incarnation: u64,
    },
    Lair {
        lair_id: LairId,
        lair_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentPrompt {
    pub capability: Vec<u8>,
    pub requester: String,
    pub requester_pid: u32,
    pub requester_uid: u32,
    pub target: ConsentTarget,
    pub scopes: Vec<AccessScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentReply {
    pub capability: Vec<u8>,
    pub granted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGrant {
    pub grant_id: u64,
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub scopes: Vec<AccessScope>,
    pub requester: String,
    pub expires_at_unix_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessGrantSource {
    Ephemeral,
    PersistentPolicy,
    Development,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LairAccessGrant {
    pub grant_id: u64,
    pub source: AccessGrantSource,
    pub lair_id: LairId,
    pub scopes: Vec<AccessScope>,
    pub requester: String,
    pub expires_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistentAuthorizationStatus {
    pub policy_rule_id: String,
    pub scopes: Vec<AutomationScope>,
    pub expires_at_unix_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AuthenticationFailed,
    HandshakeRequired,
    IncompatibleVersion,
    InvalidFrame,
    FrameTooLarge,
    InvalidRequestId,
    DuplicateRequestId,
    TooManyOutstandingRequests,
    DevelopmentFeatureDisabled,
    UnsupportedOperation,
    ConsentUnavailable,
    ConsentDenied,
    Unauthorized,
    ControllerUnavailable,
    ControlTransferUnavailable,
    StaleTopology,
    NotFound,
    StaleIncarnation,
    ImageContentNotFound,
    StaleImageContent,
    InvalidArgument,
    ResourceLimit,
    Cancelled,
    RequestNotFound,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub row_id: u64,
    pub start_column: usize,
    pub end_column: usize,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub terminal_revision: u64,
    pub history_generation: u64,
    pub matches: Vec<SearchMatch>,
    pub next_cursor: Option<String>,
    pub timed_out: bool,
}

impl SearchPage {
    /// Validates bounded search results and their identity correlation.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for malformed identities, ranges, previews, or cursors.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let valid_matches = self.matches.iter().all(|item| {
            item.row_id > 0
                && item.start_column < item.end_column
                && item.preview.len() <= MAX_SEARCH_PREVIEW_BYTES
        });
        if self.incarnation == 0
            || self.history_generation == 0
            || self.matches.len() > MAX_SEARCH_RESULTS
            || self
                .next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_SEARCH_CURSOR_BYTES)
            || !valid_matches
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "invalid search page",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrollbackPage {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub terminal_revision: u64,
    pub history_generation: u64,
    pub oldest_available_row_id: Option<u64>,
    pub newest_available_row_id: Option<u64>,
    pub rows: Vec<TerminalRow>,
    pub has_older: bool,
}

impl ScrollbackPage {
    /// Validates one bounded history page.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for malformed identity metadata or bounds.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let ids = self
            .rows
            .iter()
            .map(|row| row.row_id)
            .collect::<Option<Vec<_>>>();
        let valid_bounds = match (self.oldest_available_row_id, self.newest_available_row_id) {
            (None, None) => self.rows.is_empty() && !self.has_older,
            (Some(oldest), Some(newest)) if oldest > 0 && oldest <= newest => {
                ids.as_ref().is_some_and(|ids| {
                    ids.iter().all(|id| *id > 0)
                        && ids.windows(2).all(|pair| pair[0] < pair[1])
                        && ids.first().is_none_or(|id| *id >= oldest)
                        && ids.last().is_none_or(|id| *id <= newest)
                        && (!self.has_older || ids.first().is_some_and(|first| *first > oldest))
                })
            }
            _ => false,
        };
        if self.incarnation == 0
            || self.history_generation == 0
            || self.rows.len() > MAX_SCROLLBACK_PAGE_ROWS
            || self
                .rows
                .iter()
                .any(|row| row.cells.len() > usize::from(MAX_COLUMNS))
            || !valid_bounds
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "scrollback page metadata is inconsistent",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageTransferMode {
    BinaryChunks,
    SealedMemfd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContentRequest {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub content_id: u64,
    pub generation: u64,
    pub digest: [u8; 32],
    pub accepted_transfers: Vec<ImageTransferMode>,
}

impl ImageContentRequest {
    /// Validates exact content identity and the bounded transport offer.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when identity or transfer metadata is malformed.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.incarnation == 0
            || self.content_id == 0
            || self.generation == 0
            || self.digest == [0; 32]
            || self.accepted_transfers.is_empty()
            || self.accepted_transfers.len() > 2
            || self
                .accepted_transfers
                .windows(2)
                .any(|pair| pair[0] == pair[1])
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "image content request identity or transfer offer is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContentTransfer {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub content_id: u64,
    pub generation: u64,
    pub digest: [u8; 32],
    pub byte_length: usize,
    pub transfer: ImageTransferMode,
    pub token: [u8; IMAGE_TRANSFER_TOKEN_BYTES],
    pub token_ttl_millis: u32,
}

impl ImageContentTransfer {
    /// Validates a bounded single-use transfer grant.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when identity or transfer metadata is malformed.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.incarnation == 0
            || self.content_id == 0
            || self.generation == 0
            || self.digest == [0; 32]
            || self.byte_length == 0
            || self.byte_length > MAX_IMAGE_CONTENT_BYTES
            || self.token == [0; IMAGE_TRANSFER_TOKEN_BYTES]
            || self.token_ttl_millis == 0
            || self.token_ttl_millis > IMAGE_TRANSFER_TOKEN_TTL_MILLIS
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "image content transfer identity or bounds are invalid",
            ));
        }
        Ok(())
    }

    /// Validates this transfer against the exact request and advertised content.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when identity, length, digest, or negotiated mode differs.
    pub fn validate_for(
        &self,
        request: &ImageContentRequest,
        metadata: &ImageContentMetadata,
    ) -> Result<(), ProtocolError> {
        request.validate()?;
        self.validate()?;
        metadata.validate()?;
        if self.splint_id != request.splint_id
            || self.incarnation != request.incarnation
            || self.content_id != request.content_id
            || self.content_id != metadata.content_id
            || self.generation != request.generation
            || self.generation != metadata.generation
            || self.digest != request.digest
            || self.digest != metadata.digest
            || self.byte_length != metadata.byte_length
            || !request.accepted_transfers.contains(&self.transfer)
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "image transfer does not match its request and content metadata",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSourceFormat {
    Sixel,
    KittyRgb,
    KittyRgba,
    KittyPng,
    Iterm2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageAlphaMode {
    Opaque,
    Premultiplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageRetention {
    WhilePlaced,
    ExplicitDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageErasePolicy {
    TextOverwrite,
    ExplicitDelete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContentMetadata {
    pub content_id: u64,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub source_format: ImageSourceFormat,
    pub alpha_mode: ImageAlphaMode,
    pub digest: [u8; 32],
    pub byte_length: usize,
    pub retention: ImageRetention,
}

impl ImageContentMetadata {
    fn validate(&self) -> Result<(), ProtocolError> {
        let pixels = usize::try_from(self.width)
            .ok()
            .and_then(|width| {
                usize::try_from(self.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .filter(|pixels| *pixels <= MAX_IMAGE_PIXELS);
        let expected = pixels.and_then(|pixels| pixels.checked_mul(4));
        if self.content_id == 0
            || self.generation == 0
            || self.width == 0
            || self.height == 0
            || self.width > MAX_IMAGE_DIMENSION
            || self.height > MAX_IMAGE_DIMENSION
            || self.digest == [0; 32]
            || expected != Some(self.byte_length)
            || self.byte_length > MAX_IMAGE_CONTENT_BYTES
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "image content metadata is invalid or exceeds limits",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePixelSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePlacement {
    pub placement_id: u64,
    pub content_id: u64,
    pub row_id: u64,
    pub column: usize,
    pub source: ImagePixelRect,
    pub destination_columns: usize,
    pub destination_rows: usize,
    pub source_cell_size: Option<ImagePixelSize>,
    pub x_offset: i32,
    pub y_offset: i32,
    pub z_index: i32,
    pub application_image_id: Option<u32>,
    pub application_placement_id: Option<u32>,
    pub creation_order: u64,
    pub erase_policy: ImageErasePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalImagePlane {
    pub screen: ActiveScreen,
    pub contents: Vec<ImageContentMetadata>,
    pub placements: Vec<ImagePlacement>,
}

impl TerminalImagePlane {
    /// Validates bounded image metadata and placement references.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` or `ResourceLimit` for malformed or oversized planes.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.contents.len() > MAX_IMAGE_CONTENTS_PER_SPLINT
            || self.placements.len() > MAX_IMAGE_PLACEMENTS_PER_SPLINT
        {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "terminal image plane exceeds object limits",
            ));
        }
        let mut contents = std::collections::BTreeMap::new();
        let mut total_bytes = 0_usize;
        for content in &self.contents {
            content.validate()?;
            if contents.insert(content.content_id, content).is_some() {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal image plane contains duplicate content identity",
                ));
            }
            total_bytes = total_bytes
                .checked_add(content.byte_length)
                .filter(|bytes| *bytes <= MAX_IMAGE_BYTES_PER_SPLINT)
                .ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::ResourceLimit,
                        "terminal image plane exceeds its byte limit",
                    )
                })?;
        }
        let mut placement_ids = std::collections::BTreeSet::new();
        for placement in &self.placements {
            let Some(content) = contents.get(&placement.content_id) else {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal image placement references unknown content",
                ));
            };
            let source_end_x = placement.source.x.checked_add(placement.source.width);
            let source_end_y = placement.source.y.checked_add(placement.source.height);
            let valid_cell_size = placement.source_cell_size.is_none_or(|size| {
                size.width > 0
                    && size.height > 0
                    && size.width <= MAX_IMAGE_DIMENSION
                    && size.height <= MAX_IMAGE_DIMENSION
            });
            if placement.placement_id == 0
                || placement.row_id == 0
                || placement.creation_order == 0
                || !placement_ids.insert(placement.placement_id)
                || placement.column >= usize::from(MAX_COLUMNS)
                || placement.source.width == 0
                || placement.source.height == 0
                || source_end_x.is_none_or(|end| end > content.width)
                || source_end_y.is_none_or(|end| end > content.height)
                || placement.destination_columns == 0
                || placement.destination_columns > usize::from(MAX_COLUMNS)
                || placement.destination_rows == 0
                || placement.destination_rows > usize::from(MAX_ROWS)
                || !valid_cell_size
            {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal image placement is invalid or exceeds dimensions",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalUpdate {
    pub base_revision: u64,
    pub revision: u64,
    pub rows: Vec<TerminalRowPatch>,
    pub scrolls: Vec<TerminalScroll>,
    pub cursor: Option<TerminalCursor>,
    pub title: Option<String>,
    pub input_modes: Option<TerminalInputModes>,
    pub active_screen: Option<ActiveScreen>,
    pub palette: Option<Vec<u32>>,
    pub default_colors: Option<[u32; 3]>,
    pub columns: Option<usize>,
    pub row_count: Option<usize>,
    pub scrollback: Option<TerminalScrollbackUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Box<TerminalImagePlane>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalScrollbackUpdate {
    pub transition: HistoryTransition,
    pub history_generation: u64,
    pub oldest_available_row_id: Option<u64>,
    pub newest_available_row_id: Option<u64>,
    pub rows: Vec<TerminalRow>,
    pub available_rows: usize,
    pub omitted_oldest_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryTransition {
    Append {
        appended_rows: usize,
        trimmed_rows: usize,
    },
    Clear,
    Reflow,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalRowPatch {
    pub index: usize,
    pub row: TerminalRow,
}

impl TerminalUpdate {
    /// Validates an advancing aggregate revision interval and every bound against
    /// the client's current semantic view.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when the update cannot be applied without an
    /// allocation or index exceeding negotiated protocol limits.
    #[allow(
        clippy::too_many_lines,
        reason = "wire update validation keeps every bounded field check in one transaction"
    )]
    pub fn validate_against(
        &self,
        current_revision: u64,
        current_history_generation: u64,
        current_columns: usize,
        current_rows: usize,
    ) -> Result<(), ProtocolError> {
        if self.base_revision != current_revision || self.revision <= current_revision {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal update revision interval does not advance from the current state",
            ));
        }
        let columns = self.columns.unwrap_or(current_columns);
        let rows = self.row_count.unwrap_or(current_rows);
        if columns == 0 || columns > usize::from(MAX_COLUMNS) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal update columns exceed limits",
            ));
        }
        if rows == 0 || rows > usize::from(MAX_ROWS) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal update rows exceed limits",
            ));
        }
        if self.rows.len() > MAX_UPDATE_ROW_PATCHES {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "terminal update contains too many row patches",
            ));
        }
        if self.scrolls.len() > MAX_UPDATE_SCROLLS {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "terminal update contains too many scroll operations",
            ));
        }
        if let Some(scrollback) = &self.scrollback {
            let generation_valid = match scrollback.transition {
                HistoryTransition::Append { .. } => {
                    scrollback.history_generation == current_history_generation
                }
                HistoryTransition::Clear | HistoryTransition::Reflow => {
                    scrollback.history_generation > current_history_generation
                }
                HistoryTransition::Replace => {
                    scrollback.history_generation >= current_history_generation
                }
            };
            if !generation_valid {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal update history generation transition is invalid",
                ));
            }
            if matches!(
                scrollback.transition,
                HistoryTransition::Append {
                    appended_rows,
                    trimmed_rows,
                } if appended_rows == 0
                    || appended_rows > usize::from(MAX_ROWS)
                    || trimmed_rows > appended_rows
            ) {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal update history transition is invalid",
                ));
            }
            if matches!(scrollback.transition, HistoryTransition::Clear)
                && scrollback.available_rows != 0
            {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal update clear transition retains history",
                ));
            }
            if scrollback.history_generation == 0 {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal update history generation must be non-zero",
                ));
            }
            if scrollback.rows.len() > MAX_SNAPSHOT_SCROLLBACK_ROWS {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "terminal update has too many scrollback rows",
                ));
            }
            if scrollback
                .rows
                .iter()
                .any(|row| row.cells.len() > usize::from(MAX_COLUMNS))
            {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "terminal update scrollback row exceeds column limit",
                ));
            }
            if scrollback.rows.len() > scrollback.available_rows {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal update returns more scrollback rows than available",
                ));
            }
            if !valid_scrollback_identity(
                &scrollback.rows,
                scrollback.available_rows,
                scrollback.omitted_oldest_rows,
                scrollback.oldest_available_row_id,
                scrollback.newest_available_row_id,
            ) {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal update scrollback identity metadata is inconsistent",
                ));
            }
            if scrollback.omitted_oldest_rows
                != scrollback
                    .available_rows
                    .saturating_sub(scrollback.rows.len())
            {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal update scrollback omitted count is inconsistent",
                ));
            }
        }
        let mut seen = vec![false; rows];
        let mut seen_row_ids = std::collections::BTreeSet::new();
        for patch in &self.rows {
            let valid_row_id = patch
                .row
                .row_id
                .is_some_and(|id| id > 0 && seen_row_ids.insert(id));
            if patch.index >= rows
                || !valid_row_id
                || patch.row.cells.len() > columns
                || seen[patch.index]
            {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal row patch is duplicate or exceeds dimensions",
                ));
            }
            seen[patch.index] = true;
        }
        for scroll in &self.scrolls {
            let region_rows = scroll.end_row.saturating_sub(scroll.start_row);
            if scroll.start_row >= scroll.end_row
                || scroll.end_row > rows
                || scroll.rows == 0
                || scroll.rows > region_rows
            {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "terminal scroll exceeds dimensions",
                ));
            }
        }
        if self
            .palette
            .as_ref()
            .is_some_and(|palette| palette.len() != 256)
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal update palette must contain 256 colors",
            ));
        }
        if let Some(images) = &self.images {
            images.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCursor {
    pub column: i32,
    pub row: i32,
    pub deferred_wrap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScroll {
    pub direction: ScrollDirection,
    pub start_row: usize,
    pub end_row: usize,
    pub rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Forward,
    Reverse,
}

/// Scoped public-facing identity for one authorized terminal result.
///
/// This private wire DTO lets non-graphical clients project terminal results
/// without issuing a broader topology request. Terminal revision and history
/// generation must match the adjacent snapshot, page, or resync state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProvenance {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub topology_revision: TopologyRevision,
    pub terminal_revision: u64,
    pub history_generation: u64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub revision: u64,
    pub columns: usize,
    pub rows: usize,
    pub cursor_column: i32,
    pub cursor_row: i32,
    pub cursor_deferred_wrap: bool,
    pub active_screen: ActiveScreen,
    pub input_modes: TerminalInputModes,
    pub palette: Vec<u32>,
    pub default_colors: [u32; 3],
    pub title: String,
    pub visible_rows: Vec<TerminalRow>,
    pub history_generation: u64,
    pub oldest_available_scrollback_row_id: Option<u64>,
    pub newest_available_scrollback_row_id: Option<u64>,
    pub scrollback_rows: Vec<TerminalRow>,
    pub available_scrollback_rows: usize,
    pub omitted_oldest_scrollback_rows: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Box<TerminalImagePlane>>,
    pub exited_code: Option<i32>,
    pub exited_signal: Option<i32>,
}

impl TerminalSnapshot {
    /// Validates all snapshot dimensions and bounded scrollback identity metadata.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when the snapshot is inconsistent or exceeds
    /// negotiated protocol limits.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.columns == 0
            || self.columns > usize::from(MAX_COLUMNS)
            || self.rows == 0
            || self.rows > usize::from(MAX_ROWS)
            || self.visible_rows.len() != self.rows
            || self.palette.len() != 256
            || self.history_generation == 0
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal snapshot dimensions or metadata are invalid",
            ));
        }
        if self
            .visible_rows
            .iter()
            .any(|row| row.cells.len() > self.columns)
            || !valid_visible_identity(&self.visible_rows, &self.scrollback_rows)
            || self.scrollback_rows.len() > MAX_SNAPSHOT_SCROLLBACK_ROWS
            || self
                .scrollback_rows
                .iter()
                .any(|row| row.cells.len() > self.columns)
            || self.omitted_oldest_scrollback_rows
                != self
                    .available_scrollback_rows
                    .saturating_sub(self.scrollback_rows.len())
            || !valid_scrollback_identity(
                &self.scrollback_rows,
                self.available_scrollback_rows,
                self.omitted_oldest_scrollback_rows,
                self.oldest_available_scrollback_row_id,
                self.newest_available_scrollback_row_id,
            )
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal snapshot scrollback metadata is inconsistent",
            ));
        }
        if let Some(images) = &self.images {
            images.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveScreen {
    Normal,
    Alternate,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "wire input modes are independent terminal semantics"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputModes {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
    pub cursor_visible: bool,
    pub cursor_blink: bool,
    pub mouse_tracking: MouseTracking,
    pub sgr_mouse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseTracking {
    None,
    Normal,
    Button,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_id: Option<u64>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub linebreak: bool,
    pub cells: Vec<TerminalCell>,
}

fn valid_visible_identity(visible: &[TerminalRow], scrollback: &[TerminalRow]) -> bool {
    let mut ids = std::collections::BTreeSet::new();
    visible
        .iter()
        .all(|row| row.row_id.is_some_and(|id| id > 0 && ids.insert(id)))
        && scrollback
            .iter()
            .all(|row| row.row_id.is_none_or(|id| !ids.contains(&id)))
}

fn valid_scrollback_identity(
    rows: &[TerminalRow],
    available_rows: usize,
    omitted_oldest_rows: usize,
    oldest: Option<u64>,
    newest: Option<u64>,
) -> bool {
    if available_rows == 0 {
        return rows.is_empty() && oldest.is_none() && newest.is_none();
    }
    if rows.is_empty() {
        return oldest.is_none() && newest.is_none();
    }
    let (Some(oldest), Some(newest)) = (oldest, newest) else {
        return false;
    };
    let ids = rows
        .iter()
        .map(|row| row.row_id)
        .collect::<Option<Vec<_>>>();
    let Some(ids) = ids else {
        return false;
    };
    oldest > 0
        && newest > 0
        && ids.iter().all(|id| *id > 0)
        && ids.windows(2).all(|pair| pair[0] < pair[1])
        && ids.last().copied() == Some(newest)
        && (omitted_oldest_rows > 0 && ids.first().is_some_and(|first| oldest < *first)
            || omitted_oldest_rows == 0 && ids.first().copied() == Some(oldest))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCell {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacer_remaining: Option<u32>,
    #[serde(default, skip_serializing_if = "cell_attributes_are_default")]
    pub attributes: CellAttributes,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip predicate receives a reference"
)]
fn underline_is_none(style: &UnderlineStyle) -> bool {
    *style == UnderlineStyle::None
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip predicate receives a reference"
)]
fn color_source_is_default(source: &ColorSource) -> bool {
    *source == ColorSource::Default
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip predicate receives a reference"
)]
fn color_value_is_zero(value: &u32) -> bool {
    *value == 0
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip predicate receives a reference"
)]
fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn cell_attributes_are_default(attributes: &CellAttributes) -> bool {
    *attributes == CellAttributes::default()
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "wire rendition flags are independent terminal semantics"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellAttributes {
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "underline_is_none")]
    pub underline: UnderlineStyle,
    #[serde(default, skip_serializing_if = "color_source_is_default")]
    pub underline_color_source: ColorSource,
    #[serde(default, skip_serializing_if = "color_value_is_zero")]
    pub underline_color: u32,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub strikethrough: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub blink: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub conceal: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub reverse: bool,
    #[serde(default, skip_serializing_if = "color_source_is_default")]
    pub foreground_source: ColorSource,
    #[serde(default, skip_serializing_if = "color_value_is_zero")]
    pub foreground: u32,
    #[serde(default, skip_serializing_if = "color_source_is_default")]
    pub background_source: ColorSource,
    #[serde(default, skip_serializing_if = "color_value_is_zero")]
    pub background: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSource {
    #[default]
    Default,
    Base16,
    Base256,
    Rgb,
}

/// Encodes one bounded JSON frame with a network-order 32-bit length prefix.
///
/// # Errors
/// Returns a serialization error or [`FrameEncodeError::TooLarge`].
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameEncodeError> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&[0; 4]);
    serde_json::to_writer(&mut frame, value).map_err(FrameEncodeError::Serialize)?;
    let body_len = frame.len() - 4;
    if body_len > MAX_FRAME_BYTES || body_len > u32::MAX as usize {
        return Err(FrameEncodeError::TooLarge);
    }
    let length = u32::try_from(body_len).map_err(|_| FrameEncodeError::TooLarge)?;
    frame[..4].copy_from_slice(&length.to_be_bytes());
    Ok(frame)
}

#[derive(Debug, thiserror::Error)]
pub enum FrameEncodeError {
    #[error("frame exceeds the protocol limit")]
    TooLarge,
    #[error("frame serialization failed")]
    Serialize(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_length_prefixed_and_explicit() {
        let value = ClientFrame::Hello {
            minimum_version: PROTOCOL_VERSION,
            maximum_version: PROTOCOL_VERSION,
            role: ClientRole::Automation,
        };
        let expected_body = serde_json::to_vec(&value).unwrap();
        let frame = encode_frame(&value).unwrap();
        assert_eq!(
            u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
            frame.len() - 4
        );
        assert_eq!(&frame[4..], expected_body);
        assert!(
            std::str::from_utf8(&frame[4..])
                .unwrap()
                .contains("\"type\":\"hello\"")
        );
        let remote = encode_frame(&ClientFrame::Hello {
            minimum_version: PROTOCOL_VERSION,
            maximum_version: PROTOCOL_VERSION,
            role: ClientRole::RemoteInteractive,
        })
        .unwrap();
        assert!(
            std::str::from_utf8(&remote[4..])
                .unwrap()
                .contains("\"role\":\"remote_interactive\"")
        );
    }

    #[test]
    fn request_correlation_is_optional_and_backward_readable() {
        let legacy = serde_json::json!({
            "type": "request",
            "request_id": 7,
            "request": { "type": "ping" }
        });
        assert!(matches!(
            serde_json::from_value::<ClientFrame>(legacy).unwrap(),
            ClientFrame::Request {
                request_id: 7,
                diagnostic_correlation: None,
                request: Request::Ping,
            }
        ));

        let correlation = DiagnosticCorrelation {
            client_instance_id: "71d82a68-11e8-47c4-9193-dd83f4b03f1a".parse().unwrap(),
            window_id: "727b26c3-2b28-4ea2-b94a-2bbfb8ce74f1".parse().unwrap(),
        };
        let frame = ClientFrame::Request {
            request_id: 8,
            diagnostic_correlation: Some(correlation),
            request: Request::Ping,
        };
        assert_eq!(
            serde_json::from_value::<ClientFrame>(serde_json::to_value(frame).unwrap()).unwrap(),
            ClientFrame::Request {
                request_id: 8,
                diagnostic_correlation: Some(correlation),
                request: Request::Ping,
            }
        );
    }

    #[test]
    fn daemon_diagnostic_schema_is_typed_and_contains_no_free_form_error_fields() {
        let event = DaemonDiagnosticEvent {
            schema_version: 1,
            timestamp_unix_ms: 42,
            component: DaemonDiagnosticComponent::Splinterd,
            event: DaemonDiagnosticEventCode::SplintClosed,
            level: DaemonDiagnosticLevel::Info,
            pid: 7,
            client_instance_id: "71d82a68-11e8-47c4-9193-dd83f4b03f1a".parse().unwrap(),
            window_id: "727b26c3-2b28-4ea2-b94a-2bbfb8ce74f1".parse().unwrap(),
            topology_revision: None,
            build_version: "test".to_owned(),
            build_commit: None,
        };
        let value = serde_json::to_value(event).unwrap();
        assert!(value.get("error").is_none());
        assert!(value.get("message").is_none());
        assert!(value.get("name").is_none());
        assert!(value.get("path").is_none());
    }

    #[test]
    fn lair_access_request_and_grant_are_typed_and_bounded() {
        let lair_id = LairId::new();
        let request = Request::RequestLairAccess {
            lair_id,
            scopes: vec![
                AccessScope::Input,
                AccessScope::ControlTakeover,
                AccessScope::TopologyLayout,
            ],
        };
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["type"], "request_lair_access");
        assert_eq!(serde_json::from_value::<Request>(encoded).unwrap(), request);

        let grant = LairAccessGrant {
            grant_id: 7,
            source: AccessGrantSource::Ephemeral,
            lair_id,
            scopes: vec![AccessScope::Input, AccessScope::ControlTakeover],
            requester: "/usr/bin/splinterm-mcp".to_owned(),
            expires_at_unix_seconds: 42,
        };
        let encoded = serde_json::to_value(&grant).unwrap();
        assert_eq!(encoded["lair_id"], lair_id.to_string());
        assert!(encoded.get("splint_id").is_none());
        assert_eq!(
            serde_json::from_value::<LairAccessGrant>(encoded).unwrap(),
            grant
        );
    }

    #[test]
    fn default_terminal_cells_use_compact_backward_readable_json() {
        let empty = TerminalCell {
            content: String::new(),
            spacer_remaining: None,
            attributes: CellAttributes::default(),
        };
        let encoded = serde_json::to_value(&empty).unwrap();
        assert_eq!(encoded, serde_json::json!({}));
        assert_eq!(
            serde_json::from_value::<TerminalCell>(encoded).unwrap(),
            empty
        );

        let space = TerminalCell {
            content: " ".into(),
            ..empty.clone()
        };
        let encoded = serde_json::to_value(&space).unwrap();
        assert_eq!(encoded, serde_json::json!({"content": " "}));
        assert_eq!(
            serde_json::from_value::<TerminalCell>(encoded).unwrap(),
            space
        );

        let mut bold = space;
        bold.attributes.bold = true;
        assert_eq!(
            serde_json::to_value(&bold).unwrap(),
            serde_json::json!({"content": " ", "attributes": {"bold": true}})
        );
    }

    #[test]
    fn first_terminal_read_requests_are_explicit_protocol_v20_shapes() {
        assert_eq!(PROTOCOL_VERSION, 31);
        let splint_id = SplintId::new();
        let attach = Request::Attach {
            splint_id,
            incarnation: None,
            scrollback_rows: 0,
        };
        let scrollback = Request::StartScrollbackPage {
            splint_id,
            incarnation: None,
            max_rows: 16,
        };
        let search = Request::StartSearchScrollback {
            splint_id,
            incarnation: None,
            query: "needle".to_owned(),
            case_sensitive: false,
            max_results: 8,
        };
        let attach_json = serde_json::to_string(&attach).unwrap();
        let scrollback_json = serde_json::to_string(&scrollback).unwrap();
        let search_json = serde_json::to_string(&search).unwrap();
        assert!(attach_json.contains("\"type\":\"attach\""));
        assert!(attach_json.contains("\"incarnation\":null"));
        assert!(scrollback_json.contains("\"type\":\"start_scrollback_page\""));
        assert!(search_json.contains("\"type\":\"start_search_scrollback\""));
        assert_eq!(
            serde_json::from_str::<Request>(&attach_json).unwrap(),
            attach
        );
        assert_eq!(
            serde_json::from_str::<Request>(&scrollback_json).unwrap(),
            scrollback
        );
        assert_eq!(
            serde_json::from_str::<Request>(&search_json).unwrap(),
            search
        );
    }

    #[test]
    fn graphical_focus_messages_are_explicit_protocol_v26_shapes() {
        let splint_id = SplintId::new();
        let read = Request::ReadGraphicalFocus;
        let publish = Request::PublishGraphicalFocus {
            focused_splint_id: Some(splint_id),
        };
        let response = Response::GraphicalFocus {
            focused_splint_id: Some(splint_id),
            cwd: Some(PathBuf::from("/tmp/project")),
        };

        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            serde_json::json!({"type": "read_graphical_focus"})
        );
        assert_eq!(
            serde_json::from_value::<Request>(serde_json::to_value(&publish).unwrap()).unwrap(),
            publish
        );
        assert_eq!(
            serde_json::from_value::<Response>(serde_json::to_value(&response).unwrap()).unwrap(),
            response
        );
    }

    #[test]
    fn mutation_preflight_and_automation_launch_are_explicit_protocol_v21_shapes() {
        let splint_id = SplintId::new();
        let request = Request::PrepareMutation {
            mutation: MutationPreflight::SplitSplint { splint_id },
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("\"type\":\"prepare_mutation\""));
        assert!(encoded.contains("\"operation\":\"split_splint\""));
        assert_eq!(serde_json::from_str::<Request>(&encoded).unwrap(), request);

        let launch = AutomationLaunch {
            cwd: None,
            argv: Vec::new(),
        };
        launch.validate().unwrap();
        let explicit = AutomationLaunch {
            cwd: Some(PathBuf::from("/tmp")),
            argv: vec!["sh".to_owned(), String::new()],
        };
        explicit.validate().unwrap();
        assert!(
            AutomationLaunch {
                cwd: Some(PathBuf::from("relative")),
                argv: Vec::new(),
            }
            .validate()
            .is_err()
        );
        assert!(
            AutomationLaunch {
                cwd: Some(PathBuf::from("/tmp/has\0nul")),
                argv: Vec::new(),
            }
            .validate()
            .is_err()
        );
        assert!(
            AutomationLaunch {
                cwd: None,
                argv: vec!["sh".to_owned(), "has\0nul".to_owned()],
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn server_limits_are_bounded() {
        let limits = ServerLimits::default();
        assert!(limits.maximum_input_bytes < limits.maximum_frame_bytes);
        assert!(limits.maximum_outstanding_requests > 0);
        assert!(limits.maximum_subscriptions > 0);
    }

    #[test]
    fn atomic_lair_termination_has_explicit_revision_and_exact_targets() {
        let lair_id = LairId::new();
        let target = MutationTarget {
            lair_id,
            dojo_id: DojoId::new(),
            splint_id: SplintId::new(),
            incarnation: 7,
        };
        let request = Request::TerminateLair {
            expected_topology_revision: TopologyRevision::new(0),
            lair_id,
            targets: vec![target.clone()],
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["type"], "terminate_lair");
        assert_eq!(value["lair_id"], lair_id.to_string());
        assert_eq!(value["targets"][0]["incarnation"], 7);
        assert_eq!(serde_json::from_value::<Request>(value).unwrap(), request);
        assert_eq!(
            MutationPreflight::TerminateLair { lair_id },
            serde_json::from_value(
                serde_json::to_value(MutationPreflight::TerminateLair { lair_id }).unwrap()
            )
            .unwrap()
        );
    }

    #[test]
    fn launch_parameters_and_lifecycle_requests_are_bounded() {
        let launch = LaunchParameters {
            cwd: PathBuf::from("/tmp"),
            command: vec!["printf".into(), "%s".into()],
            shell: None,
            login_shell: false,
            scrollback_lines: 1_000,
        };
        assert!(launch.validate().is_ok());
        let request = Request::SplitSplint {
            expected_topology_revision: TopologyRevision::default(),
            target_splint_id: SplintId::new(),
            axis: Axis::Horizontal,
            side: SplitSide::Second,
            ratio: SplitRatio::new(500).unwrap(),
            launch: launch.clone(),
        };
        assert!(
            serde_json::to_string(&request)
                .unwrap()
                .contains("split_splint")
        );
        let transient = Request::CreateTransientLair {
            expected_topology_revision: TopologyRevision::new(7),
            name: "xdg".into(),
            launch: launch.clone(),
        };
        let encoded = serde_json::to_vec(&transient).unwrap();
        assert_eq!(
            serde_json::from_slice::<Request>(&encoded).unwrap(),
            transient
        );
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(value["type"], "create_transient_lair");
        assert_eq!(value["launch"]["cwd"], "/tmp");
        assert_eq!(
            value["launch"]["command"],
            serde_json::json!(["printf", "%s"])
        );

        let mut invalid = launch.clone();
        invalid.command = vec!["x".into(); MAX_LAUNCH_ARGUMENTS + 1];
        assert!(invalid.validate().is_err());
        let mut invalid = launch.clone();
        invalid.command[0] = String::new();
        assert!(invalid.validate().is_err());
        let mut invalid = launch.clone();
        invalid.scrollback_lines = MAX_SCROLLBACK_LINES + 1;
        assert!(invalid.validate().is_err());
        let mut invalid = launch.clone();
        invalid.cwd = PathBuf::from("/tmp/has\0nul");
        assert!(invalid.validate().is_err());
        let mut invalid = launch;
        invalid.command.push("has\0nul".to_owned());
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn control_transfer_contract_is_bounded_and_subscriber_specific() {
        let splint_id = SplintId::new();
        let status = ControlStatus {
            splint_id,
            incarnation: 7,
            controlled: true,
            locally_owned: false,
        };
        assert!(status.validate().is_ok());
        assert!(
            ControlStatus {
                locally_owned: true,
                controlled: false,
                ..status
            }
            .validate()
            .is_err()
        );

        assert!(validate_control_modes(&[ControlMode::Input]).is_ok());
        assert!(validate_control_modes(&[ControlMode::Input, ControlMode::Resize]).is_ok());
        assert!(validate_control_modes(&[]).is_err());
        assert!(validate_control_modes(&[ControlMode::Input, ControlMode::Input]).is_err());
        let request = Request::RequestControlTransfer {
            splint_id,
            incarnation: 7,
            modes: vec![ControlMode::Input],
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("request_control_transfer"));
        let event = SubscriptionEvent::ControlTransferResolved {
            transfer_id: 9,
            outcome: ControlTransferOutcome::Granted,
            controller_id: Some(11),
        };
        let encoded = serde_json::to_string(&event).unwrap();
        assert!(encoded.contains("control_transfer_resolved"));
        assert!(encoded.contains("controller_id"));
    }

    #[test]
    fn targeted_inspection_and_topology_runtime_correlation_are_explicit() {
        let splint_id = SplintId::new();
        let request = Request::InspectSplint { splint_id };
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("inspect_splint"));
        assert!(encoded.contains("splint_id"));

        let mut topology = Topology::new();
        let lair = topology
            .create_lair("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let splint_id = match &lair.dojos[0].root {
            splinterm_core::LayoutNode::Leaf(splint) => splint.id,
            splinterm_core::LayoutNode::Branch { .. } => unreachable!(),
        };
        assert!(topology.set_splint_state(splint_id, splinterm_core::SplintState::Exited(0)));
        assert!(topology.set_splint_last_incarnation(splint_id, 1));
        let runtime = SplintRuntimeSummary {
            splint_id,
            live_incarnation: None,
            last_incarnation: Some(1),
            restorable: true,
            lifecycle: SplintLifecycle::Exited,
            exit_status: Some(ProcessExitStatus {
                code: Some(0),
                signal: None,
            }),
        };
        let snapshot = TopologySnapshot {
            revision: topology.revision(),
            topology,
            runtimes: vec![runtime],
        };
        assert!(snapshot.validate().is_ok());

        let mut invalid = snapshot.clone();
        invalid.runtimes.push(invalid.runtimes[0].clone());
        assert!(invalid.validate().is_err());
        let mut invalid = snapshot;
        invalid.runtimes[0].lifecycle = SplintLifecycle::Running;
        assert!(invalid.validate().is_err());
    }

    fn update() -> TerminalUpdate {
        TerminalUpdate {
            base_revision: 4,
            revision: 5,
            rows: vec![TerminalRowPatch {
                index: 1,
                row: TerminalRow {
                    row_id: Some(6),
                    linebreak: false,
                    cells: Vec::new(),
                },
            }],
            scrolls: Vec::new(),
            cursor: None,
            title: None,
            input_modes: None,
            active_screen: None,
            palette: None,
            default_colors: None,
            columns: None,
            row_count: None,
            scrollback: None,
            images: None,
        }
    }

    fn snapshot() -> TerminalSnapshot {
        TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision: 1,
            columns: 1,
            rows: 1,
            cursor_column: 0,
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
            title: String::new(),
            visible_rows: vec![TerminalRow {
                row_id: Some(6),
                linebreak: true,
                cells: Vec::new(),
            }],
            history_generation: 1,
            oldest_available_scrollback_row_id: Some(4),
            newest_available_scrollback_row_id: Some(5),
            scrollback_rows: vec![
                TerminalRow {
                    row_id: Some(4),
                    linebreak: true,
                    cells: Vec::new(),
                },
                TerminalRow {
                    row_id: Some(5),
                    linebreak: true,
                    cells: Vec::new(),
                },
            ],
            available_scrollback_rows: 2,
            omitted_oldest_scrollback_rows: 0,
            images: None,
            exited_code: None,
            exited_signal: None,
        }
    }

    #[test]
    fn terminal_snapshot_validation_enforces_row_identity_scope_and_bounds() {
        assert!(snapshot().validate().is_ok());
        let mut invalid = snapshot();
        invalid.visible_rows[0].row_id = Some(4);
        assert!(invalid.validate().is_err());
        let mut invalid = snapshot();
        invalid.visible_rows[0].row_id = None;
        assert!(invalid.validate().is_err());
        let mut invalid = snapshot();
        invalid.scrollback_rows[1].row_id = Some(4);
        assert!(invalid.validate().is_err());
        let mut invalid = snapshot();
        invalid.scrollback_rows[0].row_id = Some(0);
        assert!(invalid.validate().is_err());
        let mut invalid = snapshot();
        invalid.scrollback_rows.swap(0, 1);
        assert!(invalid.validate().is_err());
        let mut invalid = snapshot();
        invalid.history_generation = 0;
        assert!(invalid.validate().is_err());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one compact identity matrix checks every image transfer binding field"
    )]
    fn image_wire_contract_binds_identity_and_bounds_metadata() {
        let content = ImageContentMetadata {
            content_id: 7,
            generation: 9,
            width: 2,
            height: 1,
            source_format: ImageSourceFormat::Sixel,
            alpha_mode: ImageAlphaMode::Premultiplied,
            digest: [3; 32],
            byte_length: 8,
            retention: ImageRetention::WhilePlaced,
        };
        let plane = TerminalImagePlane {
            screen: ActiveScreen::Normal,
            contents: vec![content.clone()],
            placements: vec![ImagePlacement {
                placement_id: 11,
                content_id: 7,
                row_id: 5,
                column: 1,
                source: ImagePixelRect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                destination_columns: 1,
                destination_rows: 1,
                source_cell_size: Some(ImagePixelSize {
                    width: 8,
                    height: 16,
                }),
                x_offset: 0,
                y_offset: 0,
                z_index: -1,
                application_image_id: None,
                application_placement_id: None,
                creation_order: 13,
                erase_policy: ImageErasePolicy::TextOverwrite,
            }],
        };
        assert!(plane.validate().is_ok());

        let mut invalid = plane.clone();
        invalid.placements[0].content_id = 8;
        assert_eq!(
            invalid.validate().unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        let mut invalid = plane.clone();
        invalid.contents[0].byte_length = MAX_IMAGE_CONTENT_BYTES + 1;
        assert!(invalid.validate().is_err());

        let request = ImageContentRequest {
            splint_id: SplintId::new(),
            incarnation: 2,
            content_id: 7,
            generation: 9,
            digest: [3; 32],
            accepted_transfers: vec![
                ImageTransferMode::SealedMemfd,
                ImageTransferMode::BinaryChunks,
            ],
        };
        assert!(request.validate().is_ok());
        let transfer = ImageContentTransfer {
            splint_id: request.splint_id,
            incarnation: request.incarnation,
            content_id: request.content_id,
            generation: request.generation,
            digest: request.digest,
            byte_length: 8,
            transfer: ImageTransferMode::BinaryChunks,
            token: [4; IMAGE_TRANSFER_TOKEN_BYTES],
            token_ttl_millis: IMAGE_TRANSFER_TOKEN_TTL_MILLIS,
        };
        assert!(transfer.validate().is_ok());
        assert!(transfer.validate_for(&request, &content).is_ok());

        let mut invalid = transfer.clone();
        invalid.splint_id = SplintId::new();
        assert!(invalid.validate_for(&request, &content).is_err());
        let mut invalid = transfer.clone();
        invalid.incarnation += 1;
        assert!(invalid.validate_for(&request, &content).is_err());
        let mut invalid = transfer.clone();
        invalid.content_id += 1;
        assert!(invalid.validate_for(&request, &content).is_err());
        let mut invalid = transfer.clone();
        invalid.generation += 1;
        assert!(invalid.validate_for(&request, &content).is_err());
        let mut invalid = transfer.clone();
        invalid.digest = [5; 32];
        assert!(invalid.validate_for(&request, &content).is_err());
        let mut invalid = transfer.clone();
        invalid.byte_length += 4;
        assert!(invalid.validate_for(&request, &content).is_err());
        let mut invalid = transfer.clone();
        invalid.transfer = ImageTransferMode::SealedMemfd;
        assert!(invalid.validate_for(&request, &content).is_ok());
        let mut binary_only = request.clone();
        binary_only.accepted_transfers = vec![ImageTransferMode::BinaryChunks];
        assert!(invalid.validate_for(&binary_only, &content).is_err());
        invalid = transfer;
        invalid.token = [0; IMAGE_TRANSFER_TOKEN_BYTES];
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn search_page_validation_enforces_identity_ranges_and_bounds() {
        let page = SearchPage {
            splint_id: SplintId::new(),
            incarnation: 2,
            terminal_revision: 3,
            history_generation: 4,
            matches: vec![SearchMatch {
                row_id: 5,
                start_column: 1,
                end_column: 3,
                preview: "hit".into(),
            }],
            next_cursor: Some("0000000000000010".into()),
            timed_out: false,
        };
        assert!(page.validate().is_ok());
        let mut invalid = page.clone();
        invalid.matches[0].end_column = 1;
        assert!(invalid.validate().is_err());
        let mut invalid = page;
        invalid.next_cursor = Some("x".repeat(MAX_SEARCH_CURSOR_BYTES + 1));
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn scrollback_page_validation_enforces_identity_order_and_bounds() {
        let page = ScrollbackPage {
            splint_id: SplintId::new(),
            incarnation: 1,
            terminal_revision: 4,
            history_generation: 2,
            oldest_available_row_id: Some(1),
            newest_available_row_id: Some(9),
            rows: vec![
                TerminalRow {
                    row_id: Some(3),
                    linebreak: false,
                    cells: Vec::new(),
                },
                TerminalRow {
                    row_id: Some(4),
                    linebreak: false,
                    cells: Vec::new(),
                },
            ],
            has_older: true,
        };
        assert!(page.validate().is_ok());

        let mut invalid = page.clone();
        invalid.incarnation = 0;
        assert!(invalid.validate().is_err());
        let mut initial_revision = page.clone();
        initial_revision.terminal_revision = 0;
        assert!(initial_revision.validate().is_ok());
        let mut invalid = page.clone();
        invalid.history_generation = 0;
        assert!(invalid.validate().is_err());
        let mut invalid = page.clone();
        invalid.rows[0].row_id = Some(0);
        assert!(invalid.validate().is_err());
        let mut invalid = page.clone();
        invalid.rows[0].row_id = Some(4);
        assert!(invalid.validate().is_err());
        let mut invalid = page.clone();
        invalid.rows[0].row_id = None;
        assert!(invalid.validate().is_err());
        let mut invalid = page.clone();
        invalid.oldest_available_row_id = Some(10);
        assert!(invalid.validate().is_err());
        let mut invalid = page.clone();
        invalid.newest_available_row_id = Some(2);
        assert!(invalid.validate().is_err());
        let mut invalid = page.clone();
        invalid.rows[1].row_id = Some(10);
        assert!(invalid.validate().is_err());
        let mut invalid = page.clone();
        invalid.rows = vec![invalid.rows[0].clone(); MAX_SCROLLBACK_PAGE_ROWS + 1];
        assert!(invalid.validate().is_err());

        let empty_history = ScrollbackPage {
            oldest_available_row_id: None,
            newest_available_row_id: None,
            rows: Vec::new(),
            has_older: false,
            ..page.clone()
        };
        assert!(empty_history.validate().is_ok());
        let mut invalid = empty_history;
        invalid.has_older = true;
        assert!(invalid.validate().is_err());

        let mut empty_before_oldest = page;
        empty_before_oldest.rows.clear();
        empty_before_oldest.has_older = false;
        assert!(empty_before_oldest.validate().is_ok());
    }

    #[test]
    fn terminal_update_validation_bounds_revisions_rows_scrolls_and_palette() {
        assert!(update().validate_against(4, 1, 80, 24).is_ok());

        let mut aggregated = update();
        aggregated.revision = 6;
        assert!(aggregated.validate_against(4, 1, 80, 24).is_ok());

        let mut invalid = update();
        invalid.revision = 4;
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut invalid = update();
        invalid.rows[0].index = 24;
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut invalid = update();
        invalid.rows[0].row.row_id = None;
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut invalid = update();
        invalid.rows.push(invalid.rows[0].clone());
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut invalid = update();
        invalid.scrolls.push(TerminalScroll {
            direction: ScrollDirection::Forward,
            start_row: 2,
            end_row: 25,
            rows: 1,
        });
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut invalid = update();
        invalid.palette = Some(vec![0; 255]);
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut invalid = update();
        invalid.scrollback = Some(TerminalScrollbackUpdate {
            transition: HistoryTransition::Replace,
            history_generation: 1,
            oldest_available_row_id: None,
            newest_available_row_id: None,
            rows: vec![TerminalRow {
                row_id: None,
                linebreak: false,
                cells: Vec::new(),
            }],
            available_rows: 0,
            omitted_oldest_rows: 0,
        });
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut valid = update();
        valid.scrollback = Some(TerminalScrollbackUpdate {
            transition: HistoryTransition::Append {
                appended_rows: 1,
                trimmed_rows: 0,
            },
            history_generation: 2,
            oldest_available_row_id: Some(7),
            newest_available_row_id: Some(9),
            rows: vec![
                TerminalRow {
                    row_id: Some(7),
                    linebreak: false,
                    cells: Vec::new(),
                },
                TerminalRow {
                    row_id: Some(9),
                    linebreak: false,
                    cells: Vec::new(),
                },
            ],
            available_rows: 2,
            omitted_oldest_rows: 0,
        });
        assert!(valid.validate_against(4, 2, 80, 24).is_ok());

        let mut invalid_ids = valid.clone();
        invalid_ids.scrollback.as_mut().unwrap().rows[0].row_id = Some(0);
        assert!(invalid_ids.validate_against(4, 2, 80, 24).is_err());
        let mut reversed_ids = valid.clone();
        reversed_ids.scrollback.as_mut().unwrap().rows.swap(0, 1);
        assert!(reversed_ids.validate_against(4, 2, 80, 24).is_err());
        let mut duplicate_ids = valid.clone();
        duplicate_ids.scrollback.as_mut().unwrap().rows[1].row_id = Some(7);
        assert!(duplicate_ids.validate_against(4, 2, 80, 24).is_err());

        let mut stale_reset = valid;
        let scrollback = stale_reset.scrollback.as_mut().unwrap();
        scrollback.transition = HistoryTransition::Reflow;
        scrollback.history_generation = 2;
        assert!(stale_reset.validate_against(4, 2, 80, 24).is_err());
        stale_reset.scrollback.as_mut().unwrap().history_generation = 3;
        assert!(stale_reset.validate_against(4, 2, 80, 24).is_ok());

        let mut stale_clear = update();
        stale_clear.scrollback = Some(TerminalScrollbackUpdate {
            transition: HistoryTransition::Clear,
            history_generation: 2,
            oldest_available_row_id: None,
            newest_available_row_id: None,
            rows: Vec::new(),
            available_rows: 0,
            omitted_oldest_rows: 0,
        });
        assert!(stale_clear.validate_against(4, 2, 80, 24).is_err());
        stale_clear.scrollback.as_mut().unwrap().history_generation = 3;
        assert!(stale_clear.validate_against(4, 2, 80, 24).is_ok());
    }
}
