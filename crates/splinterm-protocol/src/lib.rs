//! Versioned, transport-independent messages exchanged over the local socket.
//!
//! Terminal DTOs in this crate are intentionally distinct from the borrowed
//! `splinterm-terminal` and daemon runtime representations.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use splinterm_core::{
    Axis, Dojo, DojoId, Lair, SplintId, SplitRatio, SplitSide, TopologyRevision, WindowId,
};

pub const PROTOCOL_VERSION: u16 = 17;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SNAPSHOT_SCROLLBACK_ROWS: usize = 16;
pub const MAX_SCROLLBACK_PAGE_ROWS: usize = 16;
pub const MAX_SEARCH_QUERY_BYTES: usize = 256;
pub const MAX_SEARCH_RESULTS: usize = 64;
pub const MAX_AUDIT_PAGE_RECORDS: usize = 128;
pub const MAX_SEARCH_PREVIEW_BYTES: usize = 256;
pub const MAX_SEARCH_CURSOR_BYTES: usize = 32;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_LAUNCH_ARGUMENTS: usize = 64;
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
pub const MAX_ACCESS_SCOPES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Hello {
        minimum_version: u16,
        maximum_version: u16,
    },
    Request {
        request_id: u64,
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
pub struct ServerLimits {
    pub maximum_frame_bytes: usize,
    pub maximum_input_bytes: usize,
    pub maximum_columns: u16,
    pub maximum_rows: u16,
    pub maximum_outstanding_requests: usize,
    pub maximum_subscriptions: usize,
    pub maximum_snapshot_scrollback_rows: usize,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    ListDojos,
    InspectTopology,
    SubscribeTopology,
    InspectSplint {
        splint_id: SplintId,
    },
    RequestAccess {
        splint_id: SplintId,
        incarnation: u64,
        scopes: Vec<AccessScope>,
    },
    AuthorizationStatus {
        splint_id: SplintId,
        incarnation: u64,
    },
    RevokeAccess {
        grant_id: u64,
    },
    CreateDojo {
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
        splint_id: SplintId,
        launch: LaunchParameters,
    },
    RestoreSplint {
        splint_id: SplintId,
    },
    RestoreWindow {
        window_id: WindowId,
    },
    RestoreDojo {
        dojo_id: DojoId,
    },
    CloseSplint {
        expected_topology_revision: TopologyRevision,
        splint_id: SplintId,
    },
    SetSplitRatio {
        expected_topology_revision: TopologyRevision,
        target_splint_id: SplintId,
        ratio: SplitRatio,
    },
    NewWindow {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
        title: String,
        launch: LaunchParameters,
    },
    CloseWindow {
        expected_topology_revision: TopologyRevision,
        window_id: WindowId,
    },
    RenameDojo {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
        name: String,
    },
    RenameWindow {
        expected_topology_revision: TopologyRevision,
        window_id: WindowId,
        title: String,
    },
    SetWindowDefaultFocus {
        expected_topology_revision: TopologyRevision,
        window_id: WindowId,
        splint_id: SplintId,
    },
    RenameSplint {
        expected_topology_revision: TopologyRevision,
        splint_id: SplintId,
        title: String,
    },
    Attach {
        splint_id: SplintId,
        incarnation: u64,
        scrollback_rows: usize,
    },
    ScrollbackPage {
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
        before_row_id: u64,
        max_rows: usize,
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
    },
    SubscribeControl {
        splint_id: SplintId,
        incarnation: u64,
    },
    RequestControlTransfer {
        splint_id: SplintId,
        incarnation: u64,
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
    Dojos {
        dojos: Vec<Dojo>,
    },
    DojoCreated {
        dojo: Dojo,
    },
    SplintStarted {
        splint_id: SplintId,
        incarnation: u64,
        topology_revision: TopologyRevision,
    },
    WindowStarted {
        window_id: WindowId,
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
        runtime: SplintRuntimeSummary,
    },
    AccessGranted {
        grant: AccessGrant,
    },
    AuthorizationStatus {
        grants: Vec<AccessGrant>,
        development_bypass: bool,
    },
    Attached {
        subscription_id: u64,
        snapshot: TerminalSnapshot,
    },
    ScrollbackPage {
        page: ScrollbackPage,
    },
    ScrollbackResyncRequired {
        current_revision: u64,
        history_generation: u64,
    },
    SearchResults {
        page: SearchPage,
    },
    SearchResyncRequired {
        current_revision: u64,
        history_generation: u64,
    },
    ControlGranted {
        controller_id: u64,
    },
    ControlSubscribed {
        subscription_id: u64,
        status: ControlStatus,
    },
    ControlTransferPending {
        transfer_id: u64,
    },
    AuditPage {
        page: AuditPage,
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
    DojoCreated,
    SplintSplit,
    SplintClosed,
    SplitRatioChanged,
    WindowCreated,
    WindowClosed,
    DojoRenamed,
    WindowRenamed,
    WindowDefaultFocusChanged,
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
            && self.cwd.as_os_str().as_encoded_bytes().len() <= MAX_CWD_BYTES
            && self.command.len() <= MAX_LAUNCH_ARGUMENTS
            && self
                .command
                .iter()
                .all(|item| !item.is_empty() && item.len() <= MAX_LAUNCH_ARGUMENT_BYTES)
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
    pub lair: Lair,
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
            .lair
            .dojos()
            .flat_map(|dojo| &dojo.windows)
            .map(|window| window.root.splint_count())
            .sum::<usize>();
        let valid = self.revision == self.lair.revision()
            && runtime_count == self.runtimes.len()
            && self.runtimes.iter().all(|runtime| {
                identities.insert(runtime.splint_id)
                    && self.lair.find_splint(runtime.splint_id).is_some()
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
    pub lifecycle: SplintLifecycle,
    pub exit_status: Option<ProcessExitStatus>,
}

impl SplintRuntimeSummary {
    #[must_use]
    pub fn validate(&self) -> bool {
        match self.lifecycle {
            SplintLifecycle::Starting => {
                self.live_incarnation.is_none_or(|value| value > 0) && self.exit_status.is_none()
            }
            SplintLifecycle::Running => {
                self.live_incarnation.is_some_and(|value| value > 0) && self.exit_status.is_none()
            }
            SplintLifecycle::Exited => self.live_incarnation.is_none(),
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
    AuthorizationStatus,
    RevokeAccess,
    ListDojos,
    InspectTopology,
    SubscribeTopology,
    InspectSplint,
    CreateDojo,
    SplitSplint,
    RelaunchSplint,
    RestoreSplint,
    RestoreWindow,
    RestoreDojo,
    CloseSplint,
    SetSplitRatio,
    NewWindow,
    CloseWindow,
    RenameDojo,
    RenameWindow,
    SetWindowDefaultFocus,
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
    pub dojo_id: Option<DojoId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<WindowId>,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentPrompt {
    pub capability: Vec<u8>,
    pub requester: String,
    pub requester_pid: u32,
    pub requester_uid: u32,
    pub splint_id: SplintId,
    pub incarnation: u64,
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
            || self.terminal_revision == 0
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
            || self.terminal_revision == 0
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
    pub row_id: Option<u64>,
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
    pub content: String,
    pub spacer_remaining: Option<u32>,
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
    clippy::struct_excessive_bools,
    reason = "wire rendition flags are independent terminal semantics"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellAttributes {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    #[serde(default, skip_serializing_if = "underline_is_none")]
    pub underline: UnderlineStyle,
    #[serde(default, skip_serializing_if = "color_source_is_default")]
    pub underline_color_source: ColorSource,
    #[serde(default, skip_serializing_if = "color_value_is_zero")]
    pub underline_color: u32,
    pub strikethrough: bool,
    pub blink: bool,
    pub conceal: bool,
    pub reverse: bool,
    pub foreground_source: ColorSource,
    pub foreground: u32,
    pub background_source: ColorSource,
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
    let body = serde_json::to_vec(value).map_err(FrameEncodeError::Serialize)?;
    if body.len() > MAX_FRAME_BYTES || body.len() > u32::MAX as usize {
        return Err(FrameEncodeError::TooLarge);
    }
    let length = u32::try_from(body.len()).map_err(|_| FrameEncodeError::TooLarge)?;
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&body);
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
        let frame = encode_frame(&ClientFrame::Hello {
            minimum_version: PROTOCOL_VERSION,
            maximum_version: PROTOCOL_VERSION,
        })
        .unwrap();
        assert_eq!(
            u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
            frame.len() - 4
        );
        assert!(
            std::str::from_utf8(&frame[4..])
                .unwrap()
                .contains("\"type\":\"hello\"")
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

        let mut invalid = launch.clone();
        invalid.command = vec!["x".into(); MAX_LAUNCH_ARGUMENTS + 1];
        assert!(invalid.validate().is_err());
        let mut invalid = launch.clone();
        invalid.command.push(String::new());
        assert!(invalid.validate().is_err());
        let mut invalid = launch;
        invalid.scrollback_lines = MAX_SCROLLBACK_LINES + 1;
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

        let request = Request::RequestControlTransfer {
            splint_id,
            incarnation: 7,
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

        let mut lair = Lair::new();
        let dojo = lair
            .create_dojo("main", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let splint_id = match &dojo.windows[0].root {
            splinterm_core::LayoutNode::Leaf(splint) => splint.id,
            splinterm_core::LayoutNode::Branch { .. } => unreachable!(),
        };
        assert!(lair.set_splint_state(splint_id, splinterm_core::SplintState::Exited(0)));
        let runtime = SplintRuntimeSummary {
            splint_id,
            live_incarnation: None,
            lifecycle: SplintLifecycle::Exited,
            exit_status: Some(ProcessExitStatus {
                code: Some(0),
                signal: None,
            }),
        };
        let snapshot = TopologySnapshot {
            revision: lair.revision(),
            lair,
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
        let mut invalid = page.clone();
        invalid.terminal_revision = 0;
        assert!(invalid.validate().is_err());
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
