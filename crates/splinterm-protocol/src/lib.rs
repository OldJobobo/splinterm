//! Versioned, transport-independent messages exchanged over the local socket.
//!
//! Terminal DTOs in this crate are intentionally distinct from the borrowed
//! `splinterm-terminal` and daemon runtime representations.

pub mod handoff_prepare;
pub mod perf_trace;

use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Serialize};
use splinterm_core::{
    Axis, DojoId, Lair, LairId, LairRetention, SplintId, SplitRatio, SplitSide, Topology,
    TopologyRevision,
};

pub const PROTOCOL_VERSION: u16 = 41;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TERMINAL_TRANSACTION_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_TERMINAL_TRANSACTION_CHUNK_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TERMINAL_TRANSACTION_CHUNK_BASE64_BYTES: usize =
    4 * MAX_TERMINAL_TRANSACTION_CHUNK_BYTES.div_ceil(3);
pub const MAX_TERMINAL_TRANSACTION_CHUNKS: usize =
    MAX_TERMINAL_TRANSACTION_BYTES / MAX_TERMINAL_TRANSACTION_CHUNK_BYTES;
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
pub const MAX_PRESET_DOJOS: usize = 32;
pub const MAX_PRESET_PANES_PER_DOJO: usize = 32;
pub const MAX_PRESET_TOTAL_PANES: usize = 128;
pub const MAX_PRESET_DIRECTORY_IDENTITIES: usize = 32;
pub const MAX_PRESET_DEPTH: usize = 32;
pub const MAX_PRESET_KEY_BYTES: usize = 64;
pub const MAX_PRESET_LABEL_BYTES: usize = 128;
pub const MAX_COLUMNS: u16 = 480;
pub const MAX_ROWS: u16 = 128;
pub const MAX_GRID_CELLS: usize = MAX_COLUMNS as usize * MAX_ROWS as usize;
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
pub const MAX_AGENT_STATUS_DETAIL_SCALARS: usize = 80;
pub const MAX_AGENT_STATUS_DETAIL_BYTES: usize = 240;
pub const MIN_AGENT_STATUS_TTL_SECONDS: u32 = 10;
pub const MAX_AGENT_STATUS_TTL_SECONDS: u32 = 900;
pub const DEFAULT_AGENT_STATUS_ACTIVE_TTL_SECONDS: u32 = 120;
/// Bytes in one opaque, single-use, generation-bound handoff resume ticket
/// (ADR 0011). The fixed-size wire array bounds decoding.
pub const HANDOFF_RESUME_TICKET_BYTES: usize = 32;
/// Bytes in one immutable daemon/client pair build identity.
pub const HANDOFF_BUILD_IDENTITY_BYTES: usize = 32;
/// Maximum UTF-8 byte length of a reported daemon build version.
pub const MAX_HANDOFF_BUILD_VERSION_BYTES: usize = 64;
/// Maximum blocking reasons in one handoff report: at most one of each kind.
pub const MAX_HANDOFF_BLOCKING_REASONS: usize = 4;
/// Maximum transient Lair identities one blocking reason names; its count
/// stays exact when more are open.
pub const MAX_HANDOFF_REPORTED_LAIRS: usize = 16;
/// Maximum controllers one handoff control claim restores: one per live
/// Splint a daemon can own.
pub const MAX_HANDOFF_CLAIMED_CONTROLLERS: usize = 256;
/// Exact descriptor count of a [`ServerFrame::HandoffPrepare`] frame: the
/// sealed forward client snapshot, then the client end of the continuation
/// channel.
pub const HANDOFF_PREPARE_DESCRIPTORS: usize = 2;
/// Largest sealed forward client snapshot a handoff may pass.
pub const MAX_HANDOFF_CLIENT_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

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
    PresetMaterialized,
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

/// Maximum UTF-8 byte length of informational daemon hostname metadata.
pub const MAX_DAEMON_HOSTNAME_BYTES: usize = 255;

/// Accept only bounded hostname-like text. Reject controls, whitespace, formatting
/// and bidi characters rather than attempting to repair an untrusted identity.
#[must_use]
pub fn usable_daemon_hostname(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= MAX_DAEMON_HOSTNAME_BYTES
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_')))
    .then_some(value)
}

fn deserialize_daemon_hostname<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Frame decoding is transport-bounded; retain only usable, bounded metadata.
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.filter(|hostname| usable_daemon_hostname(hostname).is_some()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Hello {
        version: u16,
        limits: ServerLimits,
        development_terminal_access: bool,
        /// Informational OS hostname at connection time, never authentication authority.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_daemon_hostname"
        )]
        daemon_hostname: Option<String>,
        /// Generation of the daemon serving this connection. A fresh daemon
        /// start is generation 1; each adopted handoff publishes a later one.
        daemon_generation: u64,
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
    TerminalTransactionStart {
        total_bytes: usize,
        chunks: usize,
    },
    TerminalTransactionChunk {
        index: usize,
        data: String,
    },
    /// Sent by a handing-off daemon to an eligible trusted local Window with
    /// exactly [`HANDOFF_PREPARE_DESCRIPTORS`] descriptors attached through
    /// `SCM_RIGHTS`; see [`handoff_prepare`]. It is valid only inside a
    /// handoff the Window participates in; any other receipt is a protocol
    /// violation.
    HandoffPrepare {
        prepare: HandoffPrepare,
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

impl ServerLimits {
    /// Validates the exact-version terminal transport envelope before a client
    /// uses any advertised dimensions or frame budget.
    ///
    /// # Errors
    /// Returns `InvalidArgument` when the fixed frame limit, input limit, or
    /// endpoint-local grid bounds are outside the protocol's absolute envelope.
    pub fn validate_terminal_transport(self) -> Result<(), ProtocolError> {
        if self.maximum_frame_bytes != MAX_FRAME_BYTES {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "server frame limit does not match the protocol version",
            ));
        }
        if !(1..=MAX_INPUT_BYTES).contains(&self.maximum_input_bytes) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "server input limit is outside the protocol envelope",
            ));
        }
        if !(2..=MAX_COLUMNS).contains(&self.maximum_columns)
            || !(2..=MAX_ROWS).contains(&self.maximum_rows)
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "server terminal dimensions exceed protocol limits",
            ));
        }
        Ok(())
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
pub enum PresetTarget {
    NewLair {
        name: String,
    },
    ExistingLair {
        lair_id: LairId,
        rename: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PresetLayoutLaunch {
    Pane {
        key: String,
        title: String,
        launch: LaunchParameters,
    },
    Split {
        axis: Axis,
        ratio: SplitRatio,
        first: Box<Self>,
        second: Box<Self>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetDojoLaunch {
    pub name: String,
    pub focus_key: String,
    pub root: PresetLayoutLaunch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetDirectoryIdentity {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetPaneIdentity {
    pub dojo_id: DojoId,
    pub key: String,
    pub splint_id: SplintId,
}

/// Where a Splint, Dojo, or Lair created from inside a Window starts.
///
/// Every choice is resolved on the daemon that owns the source Splint, so a
/// remote Window resolves remote paths and the remote owner's home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum LaunchDirectory {
    /// The source Splint's live root-process directory, then its saved launch
    /// directory, then the owner's home.
    Inherit,
    /// The daemon owner's home directory.
    Home,
    /// An absolute directory; falls back to the owner's home when unusable.
    Absolute(PathBuf),
    /// A directory below the owner's home; falls back to the home itself.
    HomeRelative(PathBuf),
}

impl LaunchDirectory {
    /// Validates path shape without touching the filesystem.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for an empty, oversized, NUL-bearing, or
    /// wrongly anchored path, or a home-relative path that leaves the home.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let valid = match self {
            Self::Inherit | Self::Home => true,
            Self::Absolute(path) => bounded_path(path) && path.is_absolute(),
            Self::HomeRelative(path) => {
                bounded_path(path)
                    && path
                        .components()
                        .all(|component| matches!(component, std::path::Component::Normal(_)))
            }
        };
        if !valid {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "launch directory is not a bounded absolute or home-relative path",
            ));
        }
        Ok(())
    }
}

fn bounded_path(path: &std::path::Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    !bytes.is_empty() && !bytes.contains(&0) && bytes.len() <= MAX_CWD_BYTES
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
    /// Resolves where a Window-created Splint, Dojo, or Lair starts, on the
    /// daemon that owns the source Splint.
    ResolveLaunchDirectory {
        source_splint_id: SplintId,
        directory: LaunchDirectory,
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
        /// Set only when Splinterm generated `name`; see [`Request::CreateLair`].
        #[serde(default, skip_serializing_if = "bool_is_false")]
        generated_name: bool,
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
        /// Set only when Splinterm generated `name`; see [`Request::NewDojo`].
        #[serde(default, skip_serializing_if = "bool_is_false")]
        generated_name: bool,
    },
    CreateLair {
        expected_topology_revision: TopologyRevision,
        name: String,
        launch: LaunchParameters,
        /// Set only when Splinterm generated `name` rather than the user
        /// choosing it; an unused close may then remove the Lair with its
        /// last Dojo.
        #[serde(default, skip_serializing_if = "bool_is_false")]
        generated_name: bool,
    },
    CreateTransientLair {
        expected_topology_revision: TopologyRevision,
        name: String,
        launch: LaunchParameters,
        /// Set only when Splinterm generated `name` rather than the user
        /// choosing it; an unused close may then remove the Lair with its
        /// last Dojo.
        #[serde(default, skip_serializing_if = "bool_is_false")]
        generated_name: bool,
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
    SetLairRetention {
        expected_topology_revision: TopologyRevision,
        lair_id: LairId,
        retention: LairRetention,
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
        /// Trusted owner request to atomically make a transient Lair durable.
        #[serde(default, skip_serializing_if = "bool_is_false")]
        promote_transient_lair: bool,
        /// Set only when Splinterm generated `name` rather than the user
        /// choosing it; a chosen name keeps the Dojo from an unused close.
        #[serde(default, skip_serializing_if = "bool_is_false")]
        generated_name: bool,
    },
    MaterializePreset {
        expected_topology_revision: TopologyRevision,
        target: PresetTarget,
        dojos: Vec<PresetDojoLaunch>,
        directory_identities: Vec<PresetDirectoryIdentity>,
    },
    CloseDojo {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
    },
    /// Terminates the Dojo only when it was never used. The daemon decides and
    /// applies this atomically against its current state; a used Dojo is left
    /// unchanged.
    CloseUnusedDojo {
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
        /// Trusted owner request to atomically make a transient Lair durable.
        #[serde(default, skip_serializing_if = "bool_is_false")]
        promote_transient_lair: bool,
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
        /// Set only by the graphical client for the focus in and out reports
        /// it generates under DECSET 1004. Such bytes leave a Dojo unused;
        /// identical bytes sent without this flag count as use.
        #[serde(default, skip_serializing_if = "bool_is_false")]
        focus_report: bool,
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
    /// Explicitly reported display status for one live Splint incarnation.
    /// `None` clears the status. Never inferred from terminal content.
    ReportAgentStatus {
        splint_id: SplintId,
        incarnation: u64,
        status: Option<AgentStatusReport>,
    },
    /// Trusted local graphical UI only. `DryRun` reports whether this daemon
    /// could hand off to its installed build without fencing or capturing
    /// anything; `Execute` starts a handoff, or fails with
    /// [`ErrorCode::HandoffUnavailable`] naming every blocking reason.
    PrepareDaemonHandoff {
        mode: DaemonHandoffMode,
    },
    /// Trusted local graphical UI only. A relaunched Window claims its exact
    /// pre-fence controller disposition with the single-use resume ticket the
    /// adopted daemon issued it (ADR 0011). Every invalid, expired, replayed,
    /// or mismatched ticket fails with [`ErrorCode::InvalidHandoffTicket`].
    ClaimHandoffControl {
        ticket: HandoffResumeTicket,
    },
    /// Trusted local graphical UI only. A Window that received a
    /// [`ServerFrame::HandoffPrepare`] frame acknowledges it once it holds
    /// the descriptors and its resume record, naming the generation the frame
    /// came from. An acknowledgement that matches no pending handoff prepare,
    /// because none was sent, it was cancelled, or the generation differs,
    /// fails with [`ErrorCode::NotFound`] and changes nothing.
    AcknowledgeHandoffPrepare {
        daemon_generation: u64,
    },
}

/// One opaque, single-use, generation-bound handoff resume ticket (ADR 0011).
///
/// On the wire it is exactly [`HANDOFF_RESUME_TICKET_BYTES`] bytes, so
/// decoding rejects any other length. `Debug` never prints the bytes.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HandoffResumeTicket([u8; HANDOFF_RESUME_TICKET_BYTES]);

impl HandoffResumeTicket {
    #[must_use]
    pub const fn new(bytes: [u8; HANDOFF_RESUME_TICKET_BYTES]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; HANDOFF_RESUME_TICKET_BYTES] {
        &self.0
    }
}

impl std::fmt::Debug for HandoffResumeTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "HandoffResumeTicket(<{HANDOFF_RESUME_TICKET_BYTES} bytes redacted>)"
        )
    }
}

/// What [`Request::PrepareDaemonHandoff`] asks the daemon to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonHandoffMode {
    /// Report compatibility and blocking reasons; change nothing.
    DryRun,
    /// Start the handoff when nothing blocks it.
    Execute,
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
    PresetMaterialized {
        lair_id: LairId,
        dojo_ids: Vec<DojoId>,
        panes: Vec<PresetPaneIdentity>,
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
        /// Point-in-time launch default resolved on the owning daemon, not durable topology.
        resolved_cwd: Option<PathBuf>,
    },
    LaunchDirectoryResolved {
        source_splint_id: SplintId,
        /// Point-in-time usable directory, or `None` when neither the requested
        /// directory nor its fallbacks are usable.
        cwd: Option<PathBuf>,
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
    AgentStatusAccepted {
        /// Daemon-lifetime status sequence current after this report.
        sequence: u64,
        /// Unix milliseconds when an expiring status becomes stale.
        expires_at_ms: Option<u64>,
    },
    UnusedDojoClose {
        dojo_id: DojoId,
        outcome: UnusedDojoCloseOutcome,
    },
    /// Result of a [`DaemonHandoffMode::DryRun`] handoff preparation.
    HandoffReport {
        report: HandoffReport,
    },
    /// Result of a valid [`Request::ClaimHandoffControl`]: the controllers
    /// restored to this connection, one per pre-fence controlled Splint.
    HandoffControlClaimed {
        #[serde(deserialize_with = "deserialize_claimed_controllers")]
        controllers: Vec<HandoffClaimedController>,
    },
}

/// Result of [`Request::CloseUnusedDojo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum UnusedDojoCloseOutcome {
    /// The Dojo was removed and its process is shutting down.
    Closed {
        topology_revision: TopologyRevision,
        /// Its Lair, never named by the user, held no other Dojo and went too.
        #[serde(default, skip_serializing_if = "bool_is_false")]
        lair_removed: bool,
    },
    /// The Dojo counts as used and was left unchanged.
    Used { reason: DojoUse },
}

/// Why a Dojo counts as used, so it is never closed automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DojoUse {
    /// It no longer holds only its original Splint.
    Split,
    /// It did not start in this daemon as a plain default-shell terminal, or
    /// its Splint was relaunched or restored since.
    Launch,
    /// It or its Splint was renamed.
    Renamed,
    /// Its Splint received client input.
    Input,
    /// Another Window is attached to it.
    Attached,
    /// A program other than the shell holds the terminal foreground.
    Foreground,
    /// Its Splint is not running.
    NotRunning,
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
    /// Delivered on topology subscriptions without a topology revision bump.
    /// Clients apply it only to a matching live incarnation; the next topology
    /// snapshot supersedes it.
    AgentStatusChanged {
        splint_id: SplintId,
        incarnation: u64,
        status: Option<AgentStatusSummary>,
        sequence: u64,
    },
    /// Sent while the serving daemon prepares a handoff, without a topology
    /// revision bump. It may arrive on any subscription, terminal, control,
    /// or topology, and every client must tolerate it anywhere: it consumes
    /// one subscription sequence number and changes no subscribed state. The
    /// graphical "input paused" banner (Plan 0061 M3.12) decides which
    /// subscriptions carry it. The outcome after exec reaches a participating
    /// Window through its continuation channel, and every other client
    /// through the next connection's `ServerFrame::Hello::daemon_generation`.
    HandoffStatus {
        /// Generation of the daemon that is handing off.
        daemon_generation: u64,
        phase: HandoffStatusPhase,
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
    PresetMaterialized,
    DojoClosed,
    LairTerminated,
    LairRenamed,
    LairRetentionChanged,
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
            && self.shell.as_ref().is_none_or(|shell| {
                !shell.is_empty()
                    && !shell.contains('\0')
                    && shell.len() <= MAX_LAUNCH_ARGUMENT_BYTES
            })
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

fn valid_preset_label(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= maximum
        && !value.chars().any(char::is_control)
}

fn valid_preset_key(value: &str) -> bool {
    valid_preset_label(value, MAX_PRESET_KEY_BYTES)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl PresetLayoutLaunch {
    fn validate_into(
        &self,
        depth: usize,
        keys: &mut std::collections::HashSet<String>,
    ) -> Result<usize, ProtocolError> {
        if depth > MAX_PRESET_DEPTH {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "preset layout exceeds depth limit",
            ));
        }
        match self {
            Self::Pane { key, title, launch } => {
                if !valid_preset_key(key) || !valid_preset_label(title, MAX_PRESET_LABEL_BYTES) {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "preset pane key or title is invalid",
                    ));
                }
                if !keys.insert(key.clone()) {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "preset pane keys must be unique within a Dojo",
                    ));
                }
                launch.validate()?;
                if !launch.cwd.is_absolute() {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "preset pane cwd must be absolute",
                    ));
                }
                Ok(1)
            }
            Self::Split { first, second, .. } => {
                let first_count = first.validate_into(depth + 1, keys)?;
                let second_count = second.validate_into(depth + 1, keys)?;
                first_count.checked_add(second_count).ok_or_else(|| {
                    ProtocolError::new(ErrorCode::InvalidArgument, "preset pane count overflow")
                })
            }
        }
    }
}

impl PresetDojoLaunch {
    /// Validates one bounded neutral launch tree before daemon materialization.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for malformed labels, keys, launches, or bounds.
    pub fn validate(&self) -> Result<usize, ProtocolError> {
        if !valid_preset_label(&self.name, MAX_PRESET_LABEL_BYTES)
            || !valid_preset_key(&self.focus_key)
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "preset Dojo name or focus key is invalid",
            ));
        }
        let mut keys = std::collections::HashSet::new();
        let pane_count = self.root.validate_into(0, &mut keys)?;
        if pane_count > MAX_PRESET_PANES_PER_DOJO || !keys.contains(&self.focus_key) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "preset Dojo pane count or focus key is invalid",
            ));
        }
        Ok(pane_count)
    }
}

impl PresetDirectoryIdentity {
    /// Validates one no-follow directory identity without reading the filesystem.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for an unsafe or unbounded path or zero identity.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let path = self.path.as_os_str().as_encoded_bytes();
        if !self.path.is_absolute()
            || self.path.to_str().is_none()
            || path.is_empty()
            || path.len() > MAX_INPUT_BYTES
            || path.contains(&0)
            || self.device == 0
            || self.inode == 0
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "preset directory identity is invalid",
            ));
        }
        Ok(())
    }
}

impl PresetLayoutLaunch {
    fn contains_cwd(&self, path: &Path) -> bool {
        match self {
            Self::Pane { launch, .. } => launch.cwd == path,
            Self::Split { first, second, .. } => {
                first.contains_cwd(path) || second.contains_cwd(path)
            }
        }
    }
}

impl PresetTarget {
    /// Validates bounded target naming without resolving daemon topology.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for an invalid new or replacement Lair name.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let valid = match self {
            Self::NewLair { name } => valid_preset_label(name, MAX_PRESET_LABEL_BYTES),
            Self::ExistingLair { rename, .. } => rename
                .as_ref()
                .is_none_or(|name| valid_preset_label(name, MAX_PRESET_LABEL_BYTES)),
        };
        if !valid {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "preset target name is invalid",
            ));
        }
        Ok(())
    }
}

/// Validates one complete atomic preset request before topology mutation.
///
/// # Errors
/// Returns `InvalidArgument` when request-level Dojo or pane bounds are exceeded.
pub fn validate_preset_materialization(
    target: &PresetTarget,
    dojos: &[PresetDojoLaunch],
    directory_identities: &[PresetDirectoryIdentity],
) -> Result<(), ProtocolError> {
    target.validate()?;
    if dojos.is_empty() || dojos.len() > MAX_PRESET_DOJOS {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "preset Dojo count is invalid",
        ));
    }
    let pane_count = dojos.iter().try_fold(0_usize, |total, dojo| {
        total.checked_add(dojo.validate()?).ok_or_else(|| {
            ProtocolError::new(ErrorCode::InvalidArgument, "preset pane count overflow")
        })
    })?;
    if directory_identities.len() > MAX_PRESET_DIRECTORY_IDENTITIES {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "preset directory identity count exceeds limit",
        ));
    }
    let mut identity_paths = std::collections::HashSet::new();
    for identity in directory_identities {
        identity.validate()?;
        if !identity_paths.insert(identity.path.clone())
            || !dojos
                .iter()
                .any(|dojo| dojo.root.contains_cwd(&identity.path))
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "preset directory identities are inconsistent",
            ));
        }
    }
    if pane_count > MAX_PRESET_TOTAL_PANES {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "preset total pane count exceeds limit",
        ));
    }
    Ok(())
}

/// Validates stable pane mappings returned by one atomic preset transaction.
///
/// # Errors
/// Returns `InvalidArgument` for duplicate, missing, or out-of-scope identities.
pub fn validate_preset_materialized(
    dojo_ids: &[DojoId],
    panes: &[PresetPaneIdentity],
) -> Result<(), ProtocolError> {
    if dojo_ids.is_empty()
        || dojo_ids.len() > MAX_PRESET_DOJOS
        || panes.is_empty()
        || panes.len() > MAX_PRESET_TOTAL_PANES
    {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "preset materialization response cardinality is invalid",
        ));
    }
    let dojo_count = dojo_ids.len();
    let dojo_ids = dojo_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    if dojo_ids.len() != dojo_count {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "preset materialization repeats Dojo identities",
        ));
    }
    let mut pane_keys = std::collections::HashSet::new();
    let mut splint_ids = std::collections::HashSet::new();
    if panes.iter().any(|pane| {
        !dojo_ids.contains(&pane.dojo_id)
            || !valid_preset_key(&pane.key)
            || !pane_keys.insert((pane.dojo_id, pane.key.clone()))
            || !splint_ids.insert(pane.splint_id)
    }) || dojo_ids
        .iter()
        .any(|dojo_id| !panes.iter().any(|pane| pane.dojo_id == *dojo_id))
    {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "preset materialization pane mappings are invalid",
        ));
    }
    Ok(())
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
    /// Explicitly reported display status of the live incarnation only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatusSummary>,
}

impl SplintRuntimeSummary {
    #[must_use]
    pub fn validate(&self) -> bool {
        let valid_last = self.last_incarnation.is_none_or(|value| value > 0);
        if let Some(status) = &self.agent_status
            && (self.live_incarnation.is_none() || !status.validate())
        {
            return false;
        }
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

/// Closed vocabulary of explicitly reported agent display states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    NeedsInput,
    Done,
    Error,
    Idle,
}

impl AgentState {
    /// Active states always expire; resting states last until replaced.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Working | Self::NeedsInput)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsInput => "needs_input",
            Self::Done => "done",
            Self::Error => "error",
            Self::Idle => "idle",
        }
    }
}

/// One explicit agent status report. Display data only, never authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStatusReport {
    pub state: AgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u32>,
}

impl AgentStatusReport {
    /// Validates the bounded detail and TTL.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for an out-of-range TTL or a detail that is
    /// empty, oversized, or contains control, bidi-formatting, or line-break
    /// characters. Invalid text is rejected, never stripped.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if let Some(detail) = &self.detail {
            validate_agent_status_detail(detail)?;
        }
        if self.ttl_seconds.is_some_and(|ttl| {
            !(MIN_AGENT_STATUS_TTL_SECONDS..=MAX_AGENT_STATUS_TTL_SECONDS).contains(&ttl)
        }) {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "agent status TTL is outside 10 to 900 seconds",
            ));
        }
        Ok(())
    }

    /// The TTL that applies: the reported one, else the active-state default.
    #[must_use]
    pub fn effective_ttl_seconds(&self) -> Option<u32> {
        self.ttl_seconds.or_else(|| {
            self.state
                .is_active()
                .then_some(DEFAULT_AGENT_STATUS_ACTIVE_TTL_SECONDS)
        })
    }
}

fn is_agent_status_forbidden_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{2028}'..='\u{2029}'
        )
}

/// Validates explicitly reported agent status detail text.
///
/// # Errors
///
/// Returns `InvalidArgument` unless the detail is nonempty, at most 80 Unicode
/// scalars and 240 bytes, and free of control, bidi-formatting, and line-break
/// characters.
pub fn validate_agent_status_detail(detail: &str) -> Result<(), ProtocolError> {
    if detail.is_empty()
        || detail.len() > MAX_AGENT_STATUS_DETAIL_BYTES
        || detail.chars().count() > MAX_AGENT_STATUS_DETAIL_SCALARS
    {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "agent status detail is empty or exceeds 80 characters or 240 bytes",
        ));
    }
    if detail.chars().any(is_agent_status_forbidden_character) {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "agent status detail contains control, bidi-formatting, or line-break characters",
        ));
    }
    Ok(())
}

/// Daemon-held agent status for one live incarnation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStatusSummary {
    pub state: AgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub sequence: u64,
    /// An active state's TTL lapsed; the last state and detail are kept.
    pub stale: bool,
    /// Milliseconds since the last accepted report, at summary time.
    pub reported_age_ms: u64,
}

impl AgentStatusSummary {
    #[must_use]
    pub fn validate(&self) -> bool {
        self.sequence > 0
            && self
                .detail
                .as_deref()
                .is_none_or(|detail| validate_agent_status_detail(detail).is_ok())
    }
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
    /// Every reason a [`ErrorCode::HandoffUnavailable`] handoff was refused;
    /// empty for every other error.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_blocking_reasons"
    )]
    pub handoff_blocking_reasons: Vec<HandoffBlockingReason>,
}

impl ProtocolError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            current_topology_revision: None,
            handoff_blocking_reasons: Vec::new(),
        }
    }

    /// A refused handoff that names every reason it cannot start.
    #[must_use]
    pub fn handoff_unavailable(reasons: Vec<HandoffBlockingReason>) -> Self {
        Self {
            handoff_blocking_reasons: reasons,
            ..Self::new(
                ErrorCode::HandoffUnavailable,
                "daemon handoff is unavailable",
            )
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
    AgentStatusReport,
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
    ResolveLaunchDirectory,
    ReadGraphicalFocus,
    PublishGraphicalFocus,
    CreateLair,
    SplitSplint,
    RelaunchSplint,
    RestoreSplint,
    RestoreDojo,
    RestoreLair,
    SetLairRetention,
    CloseSplint,
    SetSplitRatio,
    NewDojo,
    MaterializePreset,
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
    AgentStatusReport,
    PrepareDaemonHandoff,
    ClaimHandoffControl,
    AcknowledgeHandoffPrepare,
}

impl AuditOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::RequestAccess => "request_access",
            Self::RequestLairAccess => "request_lair_access",
            Self::AuthorizationStatus => "authorization_status",
            Self::RevokeAccess => "revoke_access",
            Self::ListLairs => "list_lairs",
            Self::InspectTopology => "inspect_topology",
            Self::SubscribeTopology => "subscribe_topology",
            Self::InspectSplint => "inspect_splint",
            Self::ResolveLaunchDirectory => "resolve_launch_directory",
            Self::ReadGraphicalFocus => "read_graphical_focus",
            Self::PublishGraphicalFocus => "publish_graphical_focus",
            Self::CreateLair => "create_lair",
            Self::SplitSplint => "split_splint",
            Self::RelaunchSplint => "relaunch_splint",
            Self::RestoreSplint => "restore_splint",
            Self::RestoreDojo => "restore_dojo",
            Self::RestoreLair => "restore_lair",
            Self::SetLairRetention => "set_lair_retention",
            Self::CloseSplint => "close_splint",
            Self::SetSplitRatio => "set_split_ratio",
            Self::NewDojo => "new_dojo",
            Self::MaterializePreset => "materialize_preset",
            Self::CloseDojo => "close_dojo",
            Self::TerminateLair => "terminate_lair",
            Self::RenameLair => "rename_lair",
            Self::RenameDojo => "rename_dojo",
            Self::SetDojoDefaultFocus => "set_dojo_default_focus",
            Self::RenameSplint => "rename_splint",
            Self::Attach => "attach",
            Self::ScrollbackPage => "scrollback_page",
            Self::SearchScrollback => "search_scrollback",
            Self::AcquireControl => "acquire_control",
            Self::SubscribeControl => "subscribe_control",
            Self::RequestControlTransfer => "request_control_transfer",
            Self::DecideControlTransfer => "decide_control_transfer",
            Self::ForceControlTransfer => "force_control_transfer",
            Self::ReleaseControl => "release_control",
            Self::Input => "input",
            Self::Resize => "resize",
            Self::Detach => "detach",
            Self::KillSplint => "kill_splint",
            Self::ProcessExit => "process_exit",
            Self::AuditInspect => "audit_inspect",
            Self::PolicyReload => "policy_reload",
            Self::AgentStatusReport => "agent_status_report",
            Self::PrepareDaemonHandoff => "prepare_daemon_handoff",
            Self::ClaimHandoffControl => "claim_handoff_control",
            Self::AcknowledgeHandoffPrepare => "acknowledge_handoff_prepare",
        }
    }
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
    AgentStatusReport,
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
            Self::AgentStatusReport => "report agent status",
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
    /// A per-target admission rate was exceeded; retry after a short delay.
    RateLimited,
    Cancelled,
    RequestNotFound,
    Internal,
    /// The daemon is handing off: mutations, input, resize, and control
    /// transfer are fenced until the handoff completes or is cancelled.
    HandoffInProgress,
    /// A daemon handoff cannot start; `ProtocolError::handoff_blocking_reasons`
    /// names every reason.
    HandoffUnavailable,
    /// A handoff resume ticket is invalid, expired, replayed, or mismatched.
    InvalidHandoffTicket,
}

/// Deserializes a sequence, failing as soon as it holds more than `MAX`
/// elements, so the resulting `Vec` never grows past the bound.
///
/// This caps the decoded list, not peak decoding memory: the enclosing
/// `#[serde(tag = "type")]` frame enums buffer the whole frame body into
/// serde content before any visitor runs, so peak memory is bounded by
/// [`MAX_FRAME_BYTES`].
fn deserialize_bounded_vec<'de, D, T, const MAX: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct BoundedVec<T, const MAX: usize>(std::marker::PhantomData<T>);

    impl<'de, T: Deserialize<'de>, const MAX: usize> serde::de::Visitor<'de> for BoundedVec<T, MAX> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "a sequence of at most {MAX} elements")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Vec<T>, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(MAX));
            while let Some(value) = sequence.next_element()? {
                if values.len() == MAX {
                    return Err(serde::de::Error::invalid_length(MAX + 1, &self));
                }
                values.push(value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_seq(BoundedVec::<T, MAX>(std::marker::PhantomData))
}

fn deserialize_blocking_reasons<'de, D>(
    deserializer: D,
) -> Result<Vec<HandoffBlockingReason>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bounded_vec::<D, _, MAX_HANDOFF_BLOCKING_REASONS>(deserializer)
}

fn deserialize_reported_lairs<'de, D>(deserializer: D) -> Result<Vec<LairId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bounded_vec::<D, _, MAX_HANDOFF_REPORTED_LAIRS>(deserializer)
}

fn deserialize_claimed_controllers<'de, D>(
    deserializer: D,
) -> Result<Vec<HandoffClaimedController>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bounded_vec::<D, _, MAX_HANDOFF_CLAIMED_CONTROLLERS>(deserializer)
}

/// One inclusive, nonzero version range a daemon build supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffVersionRange {
    pub minimum: u16,
    pub maximum: u16,
}

impl HandoffVersionRange {
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.minimum != 0 && self.minimum <= self.maximum
    }
}

/// How a daemon build was produced, as compiled into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffBuildProvenance {
    Unpackaged,
    LocalCommittedHead,
    ReleaseCandidate,
    NixStore,
}

/// The running daemon build, as it reports itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffBuild {
    /// Package version, such as `0.2.0-dev`; at most
    /// [`MAX_HANDOFF_BUILD_VERSION_BYTES`] of ASCII alphanumerics, `.`, `-`,
    /// and `+`.
    pub version: String,
    /// Immutable daemon/client pair identity, shared with packaging and
    /// sealed preflight. Never all zero.
    pub identity: [u8; HANDOFF_BUILD_IDENTITY_BYTES],
    pub provenance: HandoffBuildProvenance,
}

/// Why a daemon handoff cannot start now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum HandoffBlockingReason {
    /// Transient Lairs never cross a handoff (operator decision D1), so each
    /// must close first. `count` is exact; `lair_ids` names at most
    /// [`MAX_HANDOFF_REPORTED_LAIRS`] of them, so a Window can tell whether
    /// its own transient Lair blocks the upgrade.
    TransientLairsOpen {
        count: usize,
        #[serde(deserialize_with = "deserialize_reported_lairs")]
        lair_ids: Vec<LairId>,
    },
    /// The running daemon is not installed from a package: there is no
    /// installed build to hand off to and no rollback authority.
    NotPackaged,
    /// The running daemon is packaged, but its startup rollback snapshot pair
    /// could not be retained, so a failed handoff could not roll back.
    RollbackUnavailable,
    /// This daemon build cannot perform a live handoff.
    Unsupported,
}

impl HandoffBlockingReason {
    const fn kind(&self) -> u8 {
        match self {
            Self::TransientLairsOpen { .. } => 0,
            Self::NotPackaged => 1,
            Self::RollbackUnavailable => 2,
            Self::Unsupported => 3,
        }
    }
}

/// Validates one bounded blocking-reason set: at most one reason of each
/// kind, and an exact, consistent transient Lair reason.
///
/// # Errors
/// Returns `InvalidArgument` for an oversized, duplicated, or inconsistent set.
pub fn validate_handoff_blocking_reasons(
    reasons: &[HandoffBlockingReason],
) -> Result<(), ProtocolError> {
    let invalid = || {
        ProtocolError::new(
            ErrorCode::InvalidArgument,
            "handoff blocking reasons are invalid",
        )
    };
    if reasons.len() > MAX_HANDOFF_BLOCKING_REASONS {
        return Err(invalid());
    }
    let mut kinds = 0_u8;
    for reason in reasons {
        let bit = 1_u8 << reason.kind();
        if kinds & bit != 0 {
            return Err(invalid());
        }
        kinds |= bit;
        if let HandoffBlockingReason::TransientLairsOpen { count, lair_ids } = reason {
            let mut unique = lair_ids.clone();
            unique.sort_unstable();
            unique.dedup();
            if *count == 0
                || lair_ids.len() > MAX_HANDOFF_REPORTED_LAIRS.min(*count)
                || unique.len() != lair_ids.len()
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

/// What a running daemon knows cheaply about a handoff, without opening,
/// sealing, or preflighting an installed candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffReport {
    pub daemon_generation: u64,
    pub build: HandoffBuild,
    pub private_protocol: HandoffVersionRange,
    pub handoff_protocol: HandoffVersionRange,
    pub terminal_checkpoint: HandoffVersionRange,
    pub descriptor_manifest: HandoffVersionRange,
    pub lairs: usize,
    pub transient_lairs: usize,
    pub dojos: usize,
    /// Registered live Splint runtimes, transient Lairs' included.
    pub live_splints: usize,
    /// Whether this daemon retained its startup rollback snapshot pair.
    pub rollback_available: bool,
    /// Every reason a handoff cannot start now; empty only when it could.
    #[serde(deserialize_with = "deserialize_blocking_reasons")]
    pub blocking_reasons: Vec<HandoffBlockingReason>,
}

impl HandoffReport {
    /// Validates identity, ranges, and blocking-reason consistency.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a malformed or self-contradictory report.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_handoff_blocking_reasons(&self.blocking_reasons)?;
        let version = &self.build.version;
        let transient_count = self
            .blocking_reasons
            .iter()
            .find_map(|reason| match reason {
                HandoffBlockingReason::TransientLairsOpen { count, .. } => Some(*count),
                _ => None,
            });
        let rollback_reasons = self
            .blocking_reasons
            .iter()
            .filter(|reason| {
                matches!(
                    reason,
                    HandoffBlockingReason::NotPackaged | HandoffBlockingReason::RollbackUnavailable
                )
            })
            .count();
        if self.daemon_generation == 0
            || version.is_empty()
            || version.len() > MAX_HANDOFF_BUILD_VERSION_BYTES
            || !version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
            || self.build.identity == [0; HANDOFF_BUILD_IDENTITY_BYTES]
            || ![
                self.private_protocol,
                self.handoff_protocol,
                self.terminal_checkpoint,
                self.descriptor_manifest,
            ]
            .into_iter()
            .all(HandoffVersionRange::is_valid)
            || self.transient_lairs > self.lairs
            || transient_count != (self.transient_lairs > 0).then_some(self.transient_lairs)
            // Exactly one rollback reason explains missing rollback authority.
            || rollback_reasons != usize::from(!self.rollback_available)
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "handoff report is invalid",
            ));
        }
        Ok(())
    }
}

/// One phase of a handoff the serving daemon broadcasts before exec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HandoffStatusPhase {
    /// The daemon is building checkpoint and descriptor authority and still
    /// serves normally.
    Preparing,
    /// Mutations, input, resize, and control transfer are paused.
    Fenced,
    /// The handoff stopped before exec. The same generation keeps serving and
    /// the fence is lifted.
    Cancelled { reason: HandoffCancelReason },
}

/// Why a handoff stopped before exec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffCancelReason {
    /// The requesting Window cancelled it.
    Requested,
    /// A blocking condition appeared after it started.
    Blocked,
    /// Preparation failed; nothing changed.
    Failed,
    /// A participant or step missed its deadline.
    TimedOut,
}

/// One controller a valid handoff control claim restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffClaimedController {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub controller_id: u64,
    pub modes: Vec<ControlMode>,
}

/// Validates a bounded claim result with one controller per Splint.
///
/// # Errors
/// Returns `InvalidArgument` for an oversized, duplicated, or malformed set.
pub fn validate_handoff_claimed_controllers(
    controllers: &[HandoffClaimedController],
) -> Result<(), ProtocolError> {
    let invalid = || {
        ProtocolError::new(
            ErrorCode::InvalidArgument,
            "handoff claimed controllers are invalid",
        )
    };
    if controllers.len() > MAX_HANDOFF_CLAIMED_CONTROLLERS {
        return Err(invalid());
    }
    let mut splints = std::collections::HashSet::with_capacity(controllers.len());
    let mut controller_ids = std::collections::HashSet::with_capacity(controllers.len());
    for controller in controllers {
        validate_control_modes(&controller.modes)?;
        if controller.incarnation == 0
            || controller.controller_id == 0
            || !splints.insert(controller.splint_id)
            || !controller_ids.insert(controller.controller_id)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Identity of the sealed forward client snapshot a
/// [`ServerFrame::HandoffPrepare`] frame passes as its first descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffClientSnapshot {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub sha256: [u8; 32],
}

/// Metadata of a [`ServerFrame::HandoffPrepare`] frame. Its descriptors
/// travel beside the frame bytes; see [`handoff_prepare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffPrepare {
    /// Generation of the daemon that is handing off.
    pub daemon_generation: u64,
    /// Attached descriptor count; always [`HANDOFF_PREPARE_DESCRIPTORS`].
    pub descriptors: usize,
    pub client_snapshot: HandoffClientSnapshot,
}

impl HandoffPrepare {
    /// Validates generation, descriptor count, and snapshot bounds.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a malformed or out-of-bounds frame.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.daemon_generation == 0
            || self.descriptors != HANDOFF_PREPARE_DESCRIPTORS
            || !(1..=MAX_HANDOFF_CLIENT_SNAPSHOT_BYTES).contains(&self.client_snapshot.size)
            || self.client_snapshot.sha256 == [0; 32]
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "handoff prepare frame is invalid",
            ));
        }
        Ok(())
    }
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CellAttributes {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: UnderlineStyle,
    pub underline_color_source: ColorSource,
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

const CELL_ATTRIBUTE_FLAG_BOLD: u8 = 1 << 0;
const CELL_ATTRIBUTE_FLAG_DIM: u8 = 1 << 1;
const CELL_ATTRIBUTE_FLAG_ITALIC: u8 = 1 << 2;
const CELL_ATTRIBUTE_FLAG_STRIKETHROUGH: u8 = 1 << 3;
const CELL_ATTRIBUTE_FLAG_BLINK: u8 = 1 << 4;
const CELL_ATTRIBUTE_FLAG_CONCEAL: u8 = 1 << 5;
const CELL_ATTRIBUTE_FLAG_REVERSE: u8 = 1 << 6;
const CELL_ATTRIBUTE_KNOWN_FLAGS: u8 = CELL_ATTRIBUTE_FLAG_BOLD
    | CELL_ATTRIBUTE_FLAG_DIM
    | CELL_ATTRIBUTE_FLAG_ITALIC
    | CELL_ATTRIBUTE_FLAG_STRIKETHROUGH
    | CELL_ATTRIBUTE_FLAG_BLINK
    | CELL_ATTRIBUTE_FLAG_CONCEAL
    | CELL_ATTRIBUTE_FLAG_REVERSE;

type WireCellAttributes = (u8, u8, u8, u32, u8, u32, u8, u32);

impl Serialize for CellAttributes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut flags = 0;
        flags |= u8::from(self.bold) * CELL_ATTRIBUTE_FLAG_BOLD;
        flags |= u8::from(self.dim) * CELL_ATTRIBUTE_FLAG_DIM;
        flags |= u8::from(self.italic) * CELL_ATTRIBUTE_FLAG_ITALIC;
        flags |= u8::from(self.strikethrough) * CELL_ATTRIBUTE_FLAG_STRIKETHROUGH;
        flags |= u8::from(self.blink) * CELL_ATTRIBUTE_FLAG_BLINK;
        flags |= u8::from(self.conceal) * CELL_ATTRIBUTE_FLAG_CONCEAL;
        flags |= u8::from(self.reverse) * CELL_ATTRIBUTE_FLAG_REVERSE;
        (
            flags,
            underline_style_to_wire(self.underline),
            color_source_to_wire(self.underline_color_source),
            self.underline_color,
            color_source_to_wire(self.foreground_source),
            self.foreground,
            color_source_to_wire(self.background_source),
            self.background,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CellAttributes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (
            flags,
            underline,
            underline_source,
            underline_color,
            foreground_source,
            foreground,
            background_source,
            background,
        ) = WireCellAttributes::deserialize(deserializer)?;
        if flags & !CELL_ATTRIBUTE_KNOWN_FLAGS != 0 {
            return Err(serde::de::Error::custom(
                "terminal cell attributes contain unknown flags",
            ));
        }
        Ok(Self {
            bold: flags & CELL_ATTRIBUTE_FLAG_BOLD != 0,
            dim: flags & CELL_ATTRIBUTE_FLAG_DIM != 0,
            italic: flags & CELL_ATTRIBUTE_FLAG_ITALIC != 0,
            underline: underline_style_from_wire(underline).ok_or_else(|| {
                serde::de::Error::custom("terminal cell underline style is invalid")
            })?,
            underline_color_source: color_source_from_wire(underline_source).ok_or_else(|| {
                serde::de::Error::custom("terminal cell underline color source is invalid")
            })?,
            underline_color,
            strikethrough: flags & CELL_ATTRIBUTE_FLAG_STRIKETHROUGH != 0,
            blink: flags & CELL_ATTRIBUTE_FLAG_BLINK != 0,
            conceal: flags & CELL_ATTRIBUTE_FLAG_CONCEAL != 0,
            reverse: flags & CELL_ATTRIBUTE_FLAG_REVERSE != 0,
            foreground_source: color_source_from_wire(foreground_source).ok_or_else(|| {
                serde::de::Error::custom("terminal cell foreground source is invalid")
            })?,
            foreground,
            background_source: color_source_from_wire(background_source).ok_or_else(|| {
                serde::de::Error::custom("terminal cell background source is invalid")
            })?,
            background,
        })
    }
}

const fn underline_style_to_wire(style: UnderlineStyle) -> u8 {
    match style {
        UnderlineStyle::None => 0,
        UnderlineStyle::Single => 1,
        UnderlineStyle::Double => 2,
        UnderlineStyle::Curly => 3,
        UnderlineStyle::Dotted => 4,
        UnderlineStyle::Dashed => 5,
    }
}

const fn underline_style_from_wire(value: u8) -> Option<UnderlineStyle> {
    match value {
        0 => Some(UnderlineStyle::None),
        1 => Some(UnderlineStyle::Single),
        2 => Some(UnderlineStyle::Double),
        3 => Some(UnderlineStyle::Curly),
        4 => Some(UnderlineStyle::Dotted),
        5 => Some(UnderlineStyle::Dashed),
        _ => None,
    }
}

const fn color_source_to_wire(source: ColorSource) -> u8 {
    match source {
        ColorSource::Default => 0,
        ColorSource::Base16 => 1,
        ColorSource::Base256 => 2,
        ColorSource::Rgb => 3,
    }
}

const fn color_source_from_wire(value: u8) -> Option<ColorSource> {
    match value {
        0 => Some(ColorSource::Default),
        1 => Some(ColorSource::Base16),
        2 => Some(ColorSource::Base256),
        3 => Some(ColorSource::Rgb),
        _ => None,
    }
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

fn terminal_frame_transaction_eligible(value: &ServerFrame) -> bool {
    matches!(
        value,
        ServerFrame::Response {
            result: Response::Attached { .. },
            ..
        } | ServerFrame::Event {
            event: SubscriptionEvent::Snapshot { .. } | SubscriptionEvent::Update { .. },
            ..
        }
    )
}

/// Encodes one server frame directly or as one bounded, contiguous terminal
/// transaction when its exact JSON body exceeds the individual frame limit.
///
/// # Errors
/// Returns `TooLarge` when the complete body exceeds the aggregate transaction
/// ceiling, or `Serialize` when the server frame cannot be encoded.
pub fn encode_server_frame_transaction(
    value: &ServerFrame,
) -> Result<Vec<Vec<u8>>, FrameEncodeError> {
    let body = serde_json::to_vec(value).map_err(FrameEncodeError::Serialize)?;
    if body.len() <= MAX_FRAME_BYTES {
        return encode_frame(value).map(|frame| vec![frame]);
    }
    if !terminal_frame_transaction_eligible(value) || body.len() > MAX_TERMINAL_TRANSACTION_BYTES {
        return Err(FrameEncodeError::TooLarge);
    }
    let chunks = body.len().div_ceil(MAX_TERMINAL_TRANSACTION_CHUNK_BYTES);
    if !(2..=MAX_TERMINAL_TRANSACTION_CHUNKS).contains(&chunks) {
        return Err(FrameEncodeError::TooLarge);
    }
    let mut frames = Vec::with_capacity(chunks + 1);
    frames.push(encode_frame(&ServerFrame::TerminalTransactionStart {
        total_bytes: body.len(),
        chunks,
    })?);
    for (index, chunk) in body
        .chunks(MAX_TERMINAL_TRANSACTION_CHUNK_BYTES)
        .enumerate()
    {
        frames.push(encode_frame(&ServerFrame::TerminalTransactionChunk {
            index,
            data: BASE64_STANDARD.encode(chunk),
        })?);
    }
    Ok(frames)
}

#[derive(Debug, Default)]
pub struct ServerFrameTransactionAssembler {
    pending: Option<PendingServerFrameTransaction>,
}

#[derive(Debug)]
struct PendingServerFrameTransaction {
    total_bytes: usize,
    chunks: usize,
    next_index: usize,
    body: Vec<u8>,
}

impl ServerFrameTransactionAssembler {
    #[must_use]
    pub const fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Accepts one ordered wire frame and publishes a logical server frame only
    /// after a complete transaction has been validated and decoded.
    ///
    /// # Errors
    /// Returns `InvalidArgument` or `ResourceLimit` for nested, interrupted,
    /// reordered, malformed, or oversized transactions. Any failure discards
    /// the pending aggregate.
    pub fn push(&mut self, frame: ServerFrame) -> Result<Option<ServerFrame>, ProtocolError> {
        let result = self.push_inner(frame);
        if result.is_err() {
            self.pending = None;
        }
        result
    }

    fn push_inner(&mut self, frame: ServerFrame) -> Result<Option<ServerFrame>, ProtocolError> {
        match frame {
            ServerFrame::TerminalTransactionStart {
                total_bytes,
                chunks,
            } => {
                if self.pending.is_some()
                    || !(MAX_FRAME_BYTES + 1..=MAX_TERMINAL_TRANSACTION_BYTES)
                        .contains(&total_bytes)
                    || !(2..=MAX_TERMINAL_TRANSACTION_CHUNKS).contains(&chunks)
                    || total_bytes.div_ceil(MAX_TERMINAL_TRANSACTION_CHUNK_BYTES) != chunks
                {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "terminal frame transaction start is invalid",
                    ));
                }
                self.pending = Some(PendingServerFrameTransaction {
                    total_bytes,
                    chunks,
                    next_index: 0,
                    body: Vec::with_capacity(total_bytes),
                });
                Ok(None)
            }
            ServerFrame::TerminalTransactionChunk { index, data } => {
                let pending = self.pending.as_mut().ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "terminal frame transaction chunk has no start",
                    )
                })?;
                if index != pending.next_index || index >= pending.chunks {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "terminal frame transaction chunks are out of order",
                    ));
                }
                if data.len() > MAX_TERMINAL_TRANSACTION_CHUNK_BASE64_BYTES {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceLimit,
                        "terminal frame transaction base64 chunk exceeds limits",
                    ));
                }
                let decoded = BASE64_STANDARD.decode(data).map_err(|_| {
                    ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "terminal frame transaction chunk is not valid base64",
                    )
                })?;
                if decoded.is_empty()
                    || decoded.len() > MAX_TERMINAL_TRANSACTION_CHUNK_BYTES
                    || pending.body.len().saturating_add(decoded.len()) > pending.total_bytes
                {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceLimit,
                        "terminal frame transaction chunk exceeds limits",
                    ));
                }
                pending.body.extend_from_slice(&decoded);
                pending.next_index += 1;
                if pending.next_index < pending.chunks {
                    return Ok(None);
                }
                if pending.body.len() != pending.total_bytes {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "terminal frame transaction length is inconsistent",
                    ));
                }
                let completed = self.pending.take().expect("pending transaction exists");
                let decoded =
                    serde_json::from_slice::<ServerFrame>(&completed.body).map_err(|_| {
                        ProtocolError::new(
                            ErrorCode::InvalidArgument,
                            "terminal frame transaction body is invalid",
                        )
                    })?;
                if !terminal_frame_transaction_eligible(&decoded) {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidArgument,
                        "terminal frame transaction body is not eligible",
                    ));
                }
                Ok(Some(decoded))
            }
            _frame if self.pending.is_some() => Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal frame transaction was interrupted",
            )),
            frame => Ok(Some(frame)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_directories_are_bounded_and_anchored() {
        for valid in [
            LaunchDirectory::Inherit,
            LaunchDirectory::Home,
            LaunchDirectory::Absolute(PathBuf::from("/srv/projects")),
            LaunchDirectory::HomeRelative(PathBuf::from("src/work")),
        ] {
            assert!(valid.validate().is_ok(), "{valid:?}");
            let request = Request::ResolveLaunchDirectory {
                source_splint_id: SplintId::new(),
                directory: valid,
            };
            let round_trip: Request =
                serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
            assert_eq!(round_trip, request);
        }
        for invalid in [
            LaunchDirectory::Absolute(PathBuf::from("relative")),
            LaunchDirectory::Absolute(PathBuf::new()),
            LaunchDirectory::Absolute(PathBuf::from(format!("/{}", "x".repeat(MAX_CWD_BYTES)))),
            LaunchDirectory::HomeRelative(PathBuf::from("/absolute")),
            LaunchDirectory::HomeRelative(PathBuf::from("../escape")),
            LaunchDirectory::HomeRelative(PathBuf::from("./a")),
            LaunchDirectory::HomeRelative(PathBuf::new()),
        ] {
            assert_eq!(
                invalid.validate().unwrap_err().code,
                ErrorCode::InvalidArgument,
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn audit_operation_name_matches_its_wire_representation() {
        for operation in [
            AuditOperation::Ping,
            AuditOperation::RequestAccess,
            AuditOperation::RequestLairAccess,
            AuditOperation::AuthorizationStatus,
            AuditOperation::RevokeAccess,
            AuditOperation::ListLairs,
            AuditOperation::InspectTopology,
            AuditOperation::SubscribeTopology,
            AuditOperation::InspectSplint,
            AuditOperation::ResolveLaunchDirectory,
            AuditOperation::ReadGraphicalFocus,
            AuditOperation::PublishGraphicalFocus,
            AuditOperation::CreateLair,
            AuditOperation::SplitSplint,
            AuditOperation::RelaunchSplint,
            AuditOperation::RestoreSplint,
            AuditOperation::RestoreDojo,
            AuditOperation::RestoreLair,
            AuditOperation::SetLairRetention,
            AuditOperation::CloseSplint,
            AuditOperation::SetSplitRatio,
            AuditOperation::NewDojo,
            AuditOperation::MaterializePreset,
            AuditOperation::CloseDojo,
            AuditOperation::TerminateLair,
            AuditOperation::RenameLair,
            AuditOperation::RenameDojo,
            AuditOperation::SetDojoDefaultFocus,
            AuditOperation::RenameSplint,
            AuditOperation::Attach,
            AuditOperation::ScrollbackPage,
            AuditOperation::SearchScrollback,
            AuditOperation::AcquireControl,
            AuditOperation::SubscribeControl,
            AuditOperation::RequestControlTransfer,
            AuditOperation::DecideControlTransfer,
            AuditOperation::ForceControlTransfer,
            AuditOperation::ReleaseControl,
            AuditOperation::Input,
            AuditOperation::Resize,
            AuditOperation::Detach,
            AuditOperation::KillSplint,
            AuditOperation::ProcessExit,
            AuditOperation::AuditInspect,
            AuditOperation::PolicyReload,
            AuditOperation::AgentStatusReport,
            AuditOperation::PrepareDaemonHandoff,
            AuditOperation::ClaimHandoffControl,
            AuditOperation::AcknowledgeHandoffPrepare,
        ] {
            assert_eq!(
                serde_json::to_string(&operation).unwrap(),
                format!("\"{}\"", operation.as_str())
            );
        }
    }

    #[test]
    fn hello_hostname_is_additive_and_old_clients_ignore_it() {
        #[derive(serde::Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum OldServerFrame {
            Hello {
                version: u16,
                limits: ServerLimits,
                development_terminal_access: bool,
            },
        }
        let old = ServerFrame::Hello {
            version: PROTOCOL_VERSION,
            limits: ServerLimits::default(),
            development_terminal_access: false,
            daemon_hostname: None,
            daemon_generation: 1,
        };
        let old_json = serde_json::to_value(&old).unwrap();
        assert!(old_json.get("daemon_hostname").is_none());
        assert_eq!(
            serde_json::from_value::<ServerFrame>(old_json.clone()).unwrap(),
            old
        );
        let mut new_json = old_json;
        new_json["daemon_hostname"] = serde_json::json!("daemon-actual.example");
        let decoded: ServerFrame = serde_json::from_value(new_json.clone()).unwrap();
        assert!(
            matches!(decoded, ServerFrame::Hello { daemon_hostname: Some(ref host), .. } if host == "daemon-actual.example")
        );
        let OldServerFrame::Hello {
            version,
            limits,
            development_terminal_access,
        } = serde_json::from_value(new_json).unwrap();
        assert_eq!(version, PROTOCOL_VERSION);
        assert_eq!(limits, ServerLimits::default());
        assert!(!development_terminal_access);
    }

    #[test]
    fn hello_hostname_rejects_unusable_metadata_without_losing_the_handshake() {
        for hostname in [
            String::new(),
            "a".repeat(MAX_DAEMON_HOSTNAME_BYTES + 1),
            "bad\nname".to_owned(),
            "bad\0name".to_owned(),
            "bad\u{202e}name".to_owned(),
            "bad\u{2066}name".to_owned(),
            "bad\u{200b}name".to_owned(),
            "white space".to_owned(),
        ] {
            let frame = ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                limits: ServerLimits::default(),
                development_terminal_access: false,
                daemon_hostname: Some(hostname),
                daemon_generation: 1,
            };
            let decoded: ServerFrame =
                serde_json::from_slice(&serde_json::to_vec(&frame).unwrap()).unwrap();
            assert!(matches!(
                decoded,
                ServerFrame::Hello {
                    daemon_hostname: None,
                    ..
                }
            ));
        }
        assert_eq!(
            usable_daemon_hostname("node-界.example"),
            Some("node-界.example")
        );
        assert!(usable_daemon_hostname(&"a".repeat(MAX_DAEMON_HOSTNAME_BYTES)).is_some());
    }

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

    fn decode_test_server_frame(frame: &[u8]) -> ServerFrame {
        let length = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(length, frame.len() - 4);
        serde_json::from_slice(&frame[4..]).unwrap()
    }

    #[test]
    fn oversized_terminal_frames_use_one_atomic_bounded_transaction() {
        let mut large = snapshot();
        large.columns = usize::from(MAX_COLUMNS);
        large.rows = usize::from(MAX_ROWS);
        large.visible_rows = (1..=u64::from(MAX_ROWS))
            .map(|row_id| TerminalRow {
                row_id: Some(row_id),
                linebreak: false,
                cells: vec![
                    TerminalCell {
                        content: "x".repeat(300),
                        spacer_remaining: None,
                        attributes: CellAttributes::default(),
                    };
                    usize::from(MAX_COLUMNS)
                ],
            })
            .collect();
        large.scrollback_rows.clear();
        large.available_scrollback_rows = 0;
        large.omitted_oldest_scrollback_rows = 0;
        large.oldest_available_scrollback_row_id = None;
        large.newest_available_scrollback_row_id = None;
        let logical = ServerFrame::Event {
            subscription_id: 7,
            sequence: 9,
            event: SubscriptionEvent::Snapshot { snapshot: large },
        };
        let frames = encode_server_frame_transaction(&logical).unwrap();
        assert!(frames.len() >= 3);
        assert!(
            frames
                .iter()
                .all(|frame| frame.len() - 4 <= MAX_FRAME_BYTES)
        );

        let mut assembler = ServerFrameTransactionAssembler::default();
        let mut published = None;
        for frame in frames {
            let next = assembler.push(decode_test_server_frame(&frame)).unwrap();
            assert!(published.is_none());
            published = next;
        }
        assert_eq!(published, Some(logical));
    }

    #[test]
    fn oversized_terminal_updates_use_the_same_atomic_transaction_boundary() {
        let cell = TerminalCell {
            content: "x".repeat(300),
            spacer_remaining: None,
            attributes: CellAttributes::default(),
        };
        let mut large = update();
        large.columns = Some(usize::from(MAX_COLUMNS));
        large.row_count = Some(usize::from(MAX_ROWS));
        large.rows = (0..usize::from(MAX_ROWS))
            .map(|index| TerminalRowPatch {
                index,
                row: TerminalRow {
                    row_id: Some(u64::try_from(index + 1).unwrap()),
                    linebreak: false,
                    cells: vec![cell.clone(); usize::from(MAX_COLUMNS)],
                },
            })
            .collect();
        large
            .validate_against(
                large.base_revision,
                1,
                usize::from(MAX_COLUMNS),
                usize::from(MAX_ROWS),
            )
            .unwrap();
        let logical = ServerFrame::Event {
            subscription_id: 7,
            sequence: 10,
            event: SubscriptionEvent::Update { update: large },
        };
        let frames = encode_server_frame_transaction(&logical).unwrap();
        assert!(frames.len() >= 3);
        let mut assembler = ServerFrameTransactionAssembler::default();
        let mut published = None;
        for frame in frames {
            published = assembler.push(decode_test_server_frame(&frame)).unwrap();
        }
        assert_eq!(published, Some(logical));
    }

    #[test]
    fn terminal_frame_transactions_fail_closed_on_order_and_interruption() {
        let start = ServerFrame::TerminalTransactionStart {
            total_bytes: MAX_FRAME_BYTES + 1,
            chunks: 3,
        };
        let mut assembler = ServerFrameTransactionAssembler::default();
        assert_eq!(assembler.push(start.clone()).unwrap(), None);
        assert!(
            assembler
                .push(ServerFrame::Response {
                    request_id: 1,
                    result: Response::Pong,
                })
                .is_err()
        );
        assert_eq!(assembler.push(start).unwrap(), None);
        assert!(
            assembler
                .push(ServerFrame::TerminalTransactionChunk {
                    index: 1,
                    data: BASE64_STANDARD.encode([1]),
                })
                .is_err()
        );
        assert_eq!(
            assembler
                .push(ServerFrame::Response {
                    request_id: 1,
                    result: Response::Pong,
                })
                .unwrap(),
            Some(ServerFrame::Response {
                request_id: 1,
                result: Response::Pong,
            })
        );
    }

    #[test]
    fn terminal_frame_transactions_reject_corrupt_duplicate_and_inconsistent_chunks() {
        let start = ServerFrame::TerminalTransactionStart {
            total_bytes: MAX_FRAME_BYTES + 1,
            chunks: 3,
        };
        let chunk = |index, data: &str| ServerFrame::TerminalTransactionChunk {
            index,
            data: data.to_owned(),
        };
        let mut assembler = ServerFrameTransactionAssembler::default();

        assert_eq!(assembler.push(start.clone()).unwrap(), None);
        assert!(assembler.push(chunk(0, "not base64")).is_err());

        assert_eq!(assembler.push(start.clone()).unwrap(), None);
        assert!(
            assembler
                .push(chunk(
                    0,
                    &"A".repeat(MAX_TERMINAL_TRANSACTION_CHUNK_BASE64_BYTES + 1),
                ))
                .is_err()
        );

        assert_eq!(assembler.push(start.clone()).unwrap(), None);
        assert_eq!(assembler.push(chunk(0, "AQ==")).unwrap(), None);
        assert!(assembler.push(chunk(0, "AQ==")).is_err());

        assert_eq!(assembler.push(start).unwrap(), None);
        assert_eq!(assembler.push(chunk(0, "AQ==")).unwrap(), None);
        assert_eq!(assembler.push(chunk(1, "AQ==")).unwrap(), None);
        assert!(assembler.push(chunk(2, "AQ==")).is_err());

        assert!(
            assembler
                .push(ServerFrame::TerminalTransactionStart {
                    total_bytes: MAX_TERMINAL_TRANSACTION_BYTES + 1,
                    chunks: MAX_TERMINAL_TRANSACTION_CHUNKS + 1,
                })
                .is_err()
        );
        assert!(!assembler.has_pending());
    }

    #[test]
    fn transaction_wrapped_nonterminal_frames_are_rejected_after_reassembly() {
        let logical = ServerFrame::Response {
            request_id: 1,
            result: Response::Pong,
        };
        let mut body = serde_json::to_vec(&logical).unwrap();
        body.resize(MAX_FRAME_BYTES + 1, b' ');
        let chunks = body.len().div_ceil(MAX_TERMINAL_TRANSACTION_CHUNK_BYTES);
        let mut assembler = ServerFrameTransactionAssembler::default();
        assert_eq!(
            assembler
                .push(ServerFrame::TerminalTransactionStart {
                    total_bytes: body.len(),
                    chunks,
                })
                .unwrap(),
            None
        );
        for (index, bytes) in body
            .chunks(MAX_TERMINAL_TRANSACTION_CHUNK_BYTES)
            .enumerate()
        {
            let result = assembler.push(ServerFrame::TerminalTransactionChunk {
                index,
                data: BASE64_STANDARD.encode(bytes),
            });
            if index + 1 == chunks {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), None);
            }
        }
        assert!(!assembler.has_pending());
    }

    #[test]
    fn nonterminal_frames_cannot_expand_into_aggregate_transactions() {
        let oversized = ServerFrame::Error {
            request_id: None,
            error: ProtocolError::new(ErrorCode::Internal, "x".repeat(MAX_FRAME_BYTES)),
        };
        assert!(matches!(
            encode_server_frame_transaction(&oversized),
            Err(FrameEncodeError::TooLarge)
        ));
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
    #[allow(
        clippy::too_many_lines,
        reason = "one protocol fixture covers request, response, mapping, and launch bounds"
    )]
    fn atomic_preset_materialization_is_typed_and_bounded() {
        let lair_id = LairId::new();
        let dojo_id = DojoId::new();
        let splint_id = SplintId::new();
        let dojo = PresetDojoLaunch {
            name: "review".into(),
            focus_key: "editor".into(),
            root: PresetLayoutLaunch::Split {
                axis: Axis::Horizontal,
                ratio: SplitRatio::new(650).unwrap(),
                first: Box::new(PresetLayoutLaunch::Pane {
                    key: "editor".into(),
                    title: "editor".into(),
                    launch: LaunchParameters {
                        cwd: PathBuf::from("/tmp"),
                        command: vec!["printf".into(), "$HOME;literal".into()],
                        shell: None,
                        login_shell: false,
                        scrollback_lines: 1000,
                    },
                }),
                second: Box::new(PresetLayoutLaunch::Pane {
                    key: "shell".into(),
                    title: "shell".into(),
                    launch: LaunchParameters {
                        cwd: PathBuf::from("/tmp"),
                        command: Vec::new(),
                        shell: Some("/bin/sh".into()),
                        login_shell: true,
                        scrollback_lines: 1000,
                    },
                }),
            },
        };
        let target = PresetTarget::ExistingLair {
            lair_id,
            rename: None,
        };
        assert!(validate_preset_materialization(&target, std::slice::from_ref(&dojo), &[]).is_ok());
        let directory = PresetDirectoryIdentity {
            path: PathBuf::from("/tmp"),
            device: 1,
            inode: 2,
        };
        assert!(
            validate_preset_materialization(
                &target,
                std::slice::from_ref(&dojo),
                std::slice::from_ref(&directory),
            )
            .is_ok()
        );
        assert!(
            validate_preset_materialization(
                &target,
                std::slice::from_ref(&dojo),
                &[directory.clone(), directory.clone()],
            )
            .is_err()
        );
        let request = Request::MaterializePreset {
            expected_topology_revision: TopologyRevision::new(7),
            target,
            dojos: vec![dojo.clone()],
            directory_identities: Vec::new(),
        };
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["type"], "materialize_preset");
        assert_eq!(serde_json::from_value::<Request>(encoded).unwrap(), request);

        let response = Response::PresetMaterialized {
            lair_id,
            dojo_ids: vec![dojo_id],
            panes: vec![PresetPaneIdentity {
                dojo_id,
                key: "editor".into(),
                splint_id,
            }],
            topology_revision: TopologyRevision::new(8),
        };
        assert_eq!(
            serde_json::from_value::<Response>(serde_json::to_value(&response).unwrap()).unwrap(),
            response
        );
        assert!(
            validate_preset_materialized(
                &[dojo_id],
                &[PresetPaneIdentity {
                    dojo_id,
                    key: "editor".into(),
                    splint_id,
                }]
            )
            .is_ok()
        );
        assert!(validate_preset_materialized(&[dojo_id, dojo_id], &[]).is_err());

        let mut invalid = dojo;
        invalid.focus_key = "missing".into();
        assert!(
            validate_preset_materialization(
                &PresetTarget::NewLair { name: "new".into() },
                &[invalid],
                &[]
            )
            .is_err()
        );
        assert!(
            validate_preset_materialization(
                &PresetTarget::NewLair { name: "new".into() },
                &[],
                &[]
            )
            .is_err()
        );
        assert!(
            LaunchParameters {
                cwd: PathBuf::from("/tmp/\0invalid"),
                command: Vec::new(),
                shell: Some("/bin/sh".into()),
                login_shell: true,
                scrollback_lines: 1_000,
            }
            .validate()
            .is_err()
        );
        assert!(
            LaunchParameters {
                cwd: PathBuf::from("/tmp"),
                command: Vec::new(),
                shell: Some("/bin/\0sh".into()),
                login_shell: true,
                scrollback_lines: 1_000,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn default_terminal_cells_use_compact_protocol_v35_json() {
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
            serde_json::json!({"content": " ", "attributes": [1, 0, 0, 0, 0, 0, 0, 0]})
        );
        assert_eq!(
            serde_json::from_value::<TerminalCell>(serde_json::json!({
                "content": " ",
                "attributes": [1, 0, 0, 0, 0, 0, 0, 0]
            }))
            .unwrap(),
            bold
        );
        assert!(
            serde_json::from_value::<TerminalCell>(serde_json::json!({
                "attributes": [128, 0, 0, 0, 0, 0, 0, 0]
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<TerminalCell>(serde_json::json!({
                "attributes": [0, 6, 0, 0, 0, 0, 0, 0]
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<TerminalCell>(serde_json::json!({
                "attributes": [0, 0, 4, 0, 0, 0, 0, 0]
            }))
            .is_err()
        );

        let styled = TerminalCell {
            attributes: CellAttributes {
                bold: true,
                dim: true,
                italic: true,
                underline: UnderlineStyle::Curly,
                underline_color_source: ColorSource::Rgb,
                underline_color: 0x00ff_ffff,
                strikethrough: true,
                blink: true,
                conceal: true,
                reverse: true,
                foreground_source: ColorSource::Rgb,
                foreground: 0x00ff_ffff,
                background_source: ColorSource::Rgb,
                background: 0x00ff_ffff,
            },
            ..TerminalCell {
                content: "x".into(),
                spacer_remaining: None,
                attributes: CellAttributes::default(),
            }
        };
        let encoded = serde_json::to_value(&styled).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "content": "x",
                "attributes": [127, 3, 3, 0x00ff_ffff, 3, 0x00ff_ffff, 3, 0x00ff_ffff]
            })
        );
        assert_eq!(
            serde_json::from_value::<TerminalCell>(encoded).unwrap(),
            styled
        );
    }

    #[test]
    fn first_terminal_read_requests_are_explicit_protocol_v20_shapes() {
        assert_eq!(PROTOCOL_VERSION, 41);
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
        assert_eq!(MAX_GRID_CELLS, 61_440);
        limits.validate_terminal_transport().unwrap();

        let valid_smaller_grid = ServerLimits {
            maximum_columns: 240,
            maximum_rows: 80,
            ..limits
        };
        valid_smaller_grid.validate_terminal_transport().unwrap();

        for invalid in [
            ServerLimits {
                maximum_frame_bytes: 0,
                ..limits
            },
            ServerLimits {
                maximum_frame_bytes: MAX_FRAME_BYTES - 1,
                ..limits
            },
            ServerLimits {
                maximum_frame_bytes: MAX_FRAME_BYTES + 1,
                ..limits
            },
            ServerLimits {
                maximum_input_bytes: 0,
                ..limits
            },
            ServerLimits {
                maximum_input_bytes: MAX_INPUT_BYTES + 1,
                ..limits
            },
            ServerLimits {
                maximum_columns: 1,
                ..limits
            },
            ServerLimits {
                maximum_columns: MAX_COLUMNS + 1,
                ..limits
            },
            ServerLimits {
                maximum_rows: 1,
                ..limits
            },
            ServerLimits {
                maximum_rows: MAX_ROWS + 1,
                ..limits
            },
        ] {
            assert!(invalid.validate_terminal_transport().is_err());
        }
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
            generated_name: false,
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

        let promotion = Request::NewDojo {
            expected_topology_revision: TopologyRevision::new(8),
            lair_id: LairId::new(),
            name: "work".into(),
            launch: launch.clone(),
            promote_transient_lair: true,
            generated_name: false,
        };
        let mut promotion_value = serde_json::to_value(&promotion).unwrap();
        assert_eq!(promotion_value["promote_transient_lair"], true);
        assert_eq!(
            serde_json::from_value::<Request>(promotion_value.clone()).unwrap(),
            promotion
        );
        promotion_value
            .as_object_mut()
            .unwrap()
            .remove("promote_transient_lair");
        assert!(matches!(
            serde_json::from_value::<Request>(promotion_value).unwrap(),
            Request::NewDojo {
                promote_transient_lair: false,
                ..
            }
        ));
        let ordinary_rename = serde_json::to_value(Request::RenameDojo {
            expected_topology_revision: TopologyRevision::new(9),
            dojo_id: DojoId::new(),
            name: "ordinary".into(),
            promote_transient_lair: false,
        })
        .unwrap();
        assert!(ordinary_rename.get("promote_transient_lair").is_none());

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
            agent_status: None,
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
    fn unused_dojo_close_has_explicit_wire_shapes() {
        let dojo_id = DojoId::new();
        let request = Request::CloseUnusedDojo { dojo_id };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({"type": "close_unused_dojo", "dojo_id": dojo_id})
        );
        let used = Response::UnusedDojoClose {
            dojo_id,
            outcome: UnusedDojoCloseOutcome::Used {
                reason: DojoUse::Foreground,
            },
        };
        assert_eq!(
            serde_json::to_value(&used).unwrap(),
            serde_json::json!({
                "type": "unused_dojo_close",
                "dojo_id": dojo_id,
                "outcome": {"outcome": "used", "reason": "foreground"},
            })
        );
        let closed = Response::UnusedDojoClose {
            dojo_id,
            outcome: UnusedDojoCloseOutcome::Closed {
                topology_revision: TopologyRevision::default(),
                lair_removed: false,
            },
        };
        assert_eq!(
            serde_json::to_value(&closed).unwrap()["outcome"],
            serde_json::json!({"outcome": "closed", "topology_revision": 0})
        );
        let lair_removed = Response::UnusedDojoClose {
            dojo_id,
            outcome: UnusedDojoCloseOutcome::Closed {
                topology_revision: TopologyRevision::default(),
                lair_removed: true,
            },
        };
        assert_eq!(
            serde_json::to_value(&lair_removed).unwrap()["outcome"]["lair_removed"],
            serde_json::json!(true)
        );
        for response in [used, closed, lair_removed] {
            assert_eq!(
                serde_json::from_value::<Response>(serde_json::to_value(&response).unwrap())
                    .unwrap(),
                response
            );
        }
        assert_eq!(
            serde_json::from_value::<Request>(serde_json::to_value(&request).unwrap()).unwrap(),
            request
        );
    }

    #[test]
    fn input_marks_focus_reports_only_when_flagged() {
        let splint_id = SplintId::new();
        let input = |focus_report| Request::Input {
            controller_id: 3,
            splint_id,
            incarnation: 4,
            bytes: b"\x1b[I".to_vec(),
            focus_report,
        };
        let ordinary = serde_json::to_value(input(false)).unwrap();
        // Ordinary input keeps the encoding older peers send and accept.
        assert!(ordinary.get("focus_report").is_none());
        assert_eq!(
            serde_json::from_value::<Request>(ordinary).unwrap(),
            input(false)
        );
        let flagged = serde_json::to_value(input(true)).unwrap();
        assert_eq!(flagged["focus_report"], true);
        assert_eq!(
            serde_json::from_value::<Request>(flagged).unwrap(),
            input(true)
        );
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

    #[test]
    fn agent_status_report_wire_shapes_are_closed_and_additive() {
        let splint_id = SplintId::new();
        let request = Request::ReportAgentStatus {
            splint_id,
            incarnation: 3,
            status: Some(AgentStatusReport {
                state: AgentState::NeedsInput,
                detail: Some("waiting for approval".into()),
                ttl_seconds: None,
            }),
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["type"], "report_agent_status");
        assert_eq!(value["status"]["state"], "needs_input");
        assert!(value["status"].get("ttl_seconds").is_none());
        assert_eq!(serde_json::from_value::<Request>(value).unwrap(), request);
        let clear: Request = serde_json::from_value(serde_json::json!({
            "type": "report_agent_status",
            "splint_id": splint_id,
            "incarnation": 3,
            "status": null
        }))
        .unwrap();
        assert!(matches!(
            clear,
            Request::ReportAgentStatus { status: None, .. }
        ));
        assert!(
            serde_json::from_value::<Request>(serde_json::json!({
                "type": "report_agent_status",
                "splint_id": splint_id,
                "incarnation": 3,
                "status": {"state": "thinking"}
            }))
            .is_err(),
            "unknown states are rejected by the closed vocabulary"
        );
        assert!(
            serde_json::from_value::<AgentStatusReport>(serde_json::json!({
                "state": "done",
                "progress": 50
            }))
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(Response::AgentStatusAccepted {
                sequence: 4,
                expires_at_ms: None,
            })
            .unwrap(),
            serde_json::json!({
                "type": "agent_status_accepted",
                "sequence": 4,
                "expires_at_ms": null
            })
        );
        let event = SubscriptionEvent::AgentStatusChanged {
            splint_id,
            incarnation: 3,
            status: None,
            sequence: 5,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "agent_status_changed");
        assert_eq!(
            serde_json::from_value::<SubscriptionEvent>(value).unwrap(),
            event
        );
        assert_eq!(
            serde_json::to_value(AutomationScope::AgentStatusReport).unwrap(),
            "agent_status_report"
        );
        assert_eq!(
            serde_json::to_value(AccessScope::AgentStatusReport).unwrap(),
            "agent_status_report"
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::RateLimited).unwrap(),
            "rate_limited"
        );
        assert_eq!(
            AuditOperation::AgentStatusReport.as_str(),
            serde_json::to_value(AuditOperation::AgentStatusReport).unwrap()
        );
        for state in [
            AgentState::Working,
            AgentState::NeedsInput,
            AgentState::Done,
            AgentState::Error,
            AgentState::Idle,
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
    }

    #[test]
    fn runtime_summary_agent_status_is_optional_and_live_only() {
        let splint_id = SplintId::new();
        let mut runtime = SplintRuntimeSummary {
            splint_id,
            live_incarnation: Some(2),
            last_incarnation: Some(2),
            restorable: false,
            lifecycle: SplintLifecycle::Running,
            exit_status: None,
            agent_status: None,
        };
        let value = serde_json::to_value(&runtime).unwrap();
        assert!(value.get("agent_status").is_none());
        assert_eq!(
            serde_json::from_value::<SplintRuntimeSummary>(value).unwrap(),
            runtime
        );
        runtime.agent_status = Some(AgentStatusSummary {
            state: AgentState::Working,
            detail: Some("compiling".into()),
            sequence: 1,
            stale: true,
            reported_age_ms: 121_000,
        });
        assert!(runtime.validate());
        let mut invalid = runtime.clone();
        invalid.agent_status.as_mut().unwrap().sequence = 0;
        assert!(!invalid.validate());
        let mut exited = runtime;
        exited.live_incarnation = None;
        exited.lifecycle = SplintLifecycle::Exited;
        exited.restorable = true;
        assert!(!exited.validate(), "exited incarnations carry no status");
    }

    #[test]
    fn agent_status_detail_and_ttl_are_bounded_and_rejected_not_stripped() {
        let report = |detail: &str, ttl_seconds| AgentStatusReport {
            state: AgentState::Working,
            detail: Some(detail.to_owned()),
            ttl_seconds,
        };
        assert!(report("waiting for approval", None).validate().is_ok());
        assert!(report(&"x".repeat(80), Some(10)).validate().is_ok());
        assert!(report(&"界".repeat(80), Some(900)).validate().is_ok());
        assert!(report(&"x".repeat(81), None).validate().is_err());
        // 61 four-byte scalars stay under 80 scalars but exceed 240 bytes.
        assert!(report(&"🦀".repeat(61), None).validate().is_err());
        assert!(report(&"🦀".repeat(60), None).validate().is_ok());
        assert!(report("", None).validate().is_err());
        for forbidden in [
            "line\nbreak",
            "carriage\rreturn",
            "tab\there",
            "nel\u{85}",
            "line\u{2028}separator",
            "para\u{2029}separator",
            "rlo\u{202e}",
            "isolate\u{2066}",
            "mark\u{200f}",
            "arabic\u{061c}",
            "escape\u{1b}[31m",
        ] {
            let error = report(forbidden, None).validate().unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{forbidden:?}");
        }
        assert!(report("ok", Some(9)).validate().is_err());
        assert!(report("ok", Some(901)).validate().is_err());
        let working = AgentStatusReport {
            state: AgentState::Working,
            detail: None,
            ttl_seconds: None,
        };
        assert_eq!(working.effective_ttl_seconds(), Some(120));
        let done = AgentStatusReport {
            state: AgentState::Done,
            detail: None,
            ttl_seconds: None,
        };
        assert_eq!(done.effective_ttl_seconds(), None);
        let timed_done = AgentStatusReport {
            ttl_seconds: Some(30),
            ..done
        };
        assert_eq!(timed_done.effective_ttl_seconds(), Some(30));
    }

    fn handoff_report() -> HandoffReport {
        HandoffReport {
            daemon_generation: 1,
            build: HandoffBuild {
                version: "0.2.0-dev".into(),
                identity: [9; HANDOFF_BUILD_IDENTITY_BYTES],
                provenance: HandoffBuildProvenance::Unpackaged,
            },
            private_protocol: HandoffVersionRange {
                minimum: PROTOCOL_VERSION,
                maximum: PROTOCOL_VERSION,
            },
            handoff_protocol: HandoffVersionRange {
                minimum: 1,
                maximum: 1,
            },
            terminal_checkpoint: HandoffVersionRange {
                minimum: 2,
                maximum: 2,
            },
            descriptor_manifest: HandoffVersionRange {
                minimum: 1,
                maximum: 1,
            },
            lairs: 3,
            transient_lairs: 1,
            dojos: 4,
            live_splints: 5,
            rollback_available: false,
            blocking_reasons: vec![
                HandoffBlockingReason::TransientLairsOpen {
                    count: 1,
                    lair_ids: vec![LairId::new()],
                },
                HandoffBlockingReason::NotPackaged,
                HandoffBlockingReason::Unsupported,
            ],
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "every handoff request shape and decoding bound is reviewed together"
    )]
    fn daemon_handoff_requests_and_hello_generation_have_explicit_wire_shapes() {
        assert_eq!(
            serde_json::to_value(Request::PrepareDaemonHandoff {
                mode: DaemonHandoffMode::DryRun,
            })
            .unwrap(),
            serde_json::json!({"type": "prepare_daemon_handoff", "mode": "dry_run"})
        );
        let execute: Request = serde_json::from_value(
            serde_json::json!({"type": "prepare_daemon_handoff", "mode": "execute"}),
        )
        .unwrap();
        assert_eq!(
            execute,
            Request::PrepareDaemonHandoff {
                mode: DaemonHandoffMode::Execute,
            }
        );
        assert!(
            serde_json::from_value::<Request>(
                serde_json::json!({"type": "prepare_daemon_handoff", "mode": "force"})
            )
            .is_err()
        );

        let claim = Request::ClaimHandoffControl {
            ticket: HandoffResumeTicket::new([0xa5; HANDOFF_RESUME_TICKET_BYTES]),
        };
        let value = serde_json::to_value(&claim).unwrap();
        assert_eq!(value["type"], "claim_handoff_control");
        assert_eq!(
            value["ticket"],
            serde_json::json!(vec![0xa5_u8; HANDOFF_RESUME_TICKET_BYTES]),
            "the newtype keeps the plain fixed-size array wire shape"
        );
        // Neither the ticket nor a request holding it prints the bytes.
        for debug in [
            format!(
                "{:?}",
                HandoffResumeTicket::new([0xa5; HANDOFF_RESUME_TICKET_BYTES])
            ),
            format!("{claim:?}"),
            format!("{claim:#?}"),
        ] {
            assert!(debug.contains("32 bytes redacted"), "{debug}");
            assert!(!debug.contains("165"), "{debug}");
            assert!(!debug.to_ascii_lowercase().contains("a5"), "{debug}");
        }
        assert_eq!(serde_json::from_value::<Request>(value).unwrap(), claim);
        for length in [
            0,
            HANDOFF_RESUME_TICKET_BYTES - 1,
            HANDOFF_RESUME_TICKET_BYTES + 1,
            4096,
        ] {
            assert!(
                serde_json::from_value::<Request>(serde_json::json!({
                    "type": "claim_handoff_control",
                    "ticket": vec![1_u8; length],
                }))
                .is_err(),
                "a {length}-byte ticket is rejected while decoding"
            );
        }

        let acknowledge = Request::AcknowledgeHandoffPrepare {
            daemon_generation: 3,
        };
        let value = serde_json::to_value(&acknowledge).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "acknowledge_handoff_prepare", "daemon_generation": 3})
        );
        assert_eq!(
            serde_json::from_value::<Request>(value).unwrap(),
            acknowledge
        );

        let hello = ServerFrame::Hello {
            version: PROTOCOL_VERSION,
            limits: ServerLimits::default(),
            development_terminal_access: false,
            daemon_hostname: None,
            daemon_generation: 7,
        };
        let mut value = serde_json::to_value(&hello).unwrap();
        assert_eq!(value["daemon_generation"], 7);
        assert_eq!(
            serde_json::from_value::<ServerFrame>(value.clone()).unwrap(),
            hello
        );
        value.as_object_mut().unwrap().remove("daemon_generation");
        assert!(
            serde_json::from_value::<ServerFrame>(value).is_err(),
            "protocol 41 hello always names the daemon generation"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "every report invariant and decoding bound is reviewed together"
    )]
    fn handoff_report_is_bounded_and_self_consistent() {
        let report = handoff_report();
        report.validate().unwrap();
        let response = Response::HandoffReport {
            report: report.clone(),
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["type"], "handoff_report");
        assert_eq!(value["report"]["build"]["provenance"], "unpackaged");
        assert_eq!(
            value["report"]["blocking_reasons"][0]["reason"],
            "transient_lairs_open"
        );
        assert_eq!(
            value["report"]["blocking_reasons"][1],
            serde_json::json!({"reason": "not_packaged"})
        );
        assert_eq!(
            value["report"]["blocking_reasons"][2],
            serde_json::json!({"reason": "unsupported"})
        );
        assert_eq!(serde_json::from_value::<Response>(value).unwrap(), response);

        let ready = HandoffReport {
            transient_lairs: 0,
            rollback_available: true,
            build: HandoffBuild {
                provenance: HandoffBuildProvenance::ReleaseCandidate,
                ..report.build.clone()
            },
            blocking_reasons: Vec::new(),
            ..report
        };
        ready.validate().unwrap();

        let invalid = |mutate: fn(&mut HandoffReport)| {
            let mut candidate = handoff_report();
            mutate(&mut candidate);
            candidate.validate().unwrap_err().code
        };
        for mutate in [
            |report: &mut HandoffReport| report.daemon_generation = 0,
            |report: &mut HandoffReport| report.build.identity = [0; HANDOFF_BUILD_IDENTITY_BYTES],
            |report: &mut HandoffReport| report.build.version = String::new(),
            |report: &mut HandoffReport| report.build.version = "0.2.0 dev".into(),
            |report: &mut HandoffReport| report.build.version = "0.2.0\u{202e}".into(),
            |report: &mut HandoffReport| {
                report.build.version = "1".repeat(MAX_HANDOFF_BUILD_VERSION_BYTES + 1);
            },
            |report: &mut HandoffReport| {
                report.handoff_protocol = HandoffVersionRange {
                    minimum: 2,
                    maximum: 1,
                };
            },
            |report: &mut HandoffReport| {
                report.private_protocol = HandoffVersionRange {
                    minimum: 0,
                    maximum: 1,
                };
            },
            |report: &mut HandoffReport| report.transient_lairs = 2,
            |report: &mut HandoffReport| report.lairs = 0,
            |report: &mut HandoffReport| {
                report.blocking_reasons.remove(0);
            },
            |report: &mut HandoffReport| report.rollback_available = true,
            |report: &mut HandoffReport| {
                report
                    .blocking_reasons
                    .push(HandoffBlockingReason::RollbackUnavailable);
            },
            |report: &mut HandoffReport| {
                report
                    .blocking_reasons
                    .push(HandoffBlockingReason::Unsupported);
            },
            |report: &mut HandoffReport| {
                report.blocking_reasons[0] = HandoffBlockingReason::TransientLairsOpen {
                    count: 1,
                    lair_ids: vec![LairId::new(), LairId::new()],
                };
            },
            |report: &mut HandoffReport| {
                let lair_id = LairId::new();
                report.transient_lairs = 2;
                report.blocking_reasons[0] = HandoffBlockingReason::TransientLairsOpen {
                    count: 2,
                    lair_ids: vec![lair_id, lair_id],
                };
            },
        ] {
            assert_eq!(invalid(mutate), ErrorCode::InvalidArgument);
        }

        // Many transient Lairs keep an exact count but name a bounded subset.
        let mut many = handoff_report();
        many.lairs = 40;
        many.transient_lairs = 40;
        many.blocking_reasons[0] = HandoffBlockingReason::TransientLairsOpen {
            count: 40,
            lair_ids: (0..MAX_HANDOFF_REPORTED_LAIRS)
                .map(|_| LairId::new())
                .collect(),
        };
        many.validate().unwrap();

        // Decoding rejects a list one element past its bound. (The whole frame
        // is still parsed first; `MAX_FRAME_BYTES` bounds that.)
        let mut value = serde_json::to_value(&many).unwrap();
        value["blocking_reasons"][0]["lair_ids"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(LairId::new()).unwrap());
        assert!(serde_json::from_value::<HandoffReport>(value).is_err());
        let mut value = serde_json::to_value(handoff_report()).unwrap();
        let reasons = value["blocking_reasons"].as_array_mut().unwrap();
        reasons.push(serde_json::json!({"reason": "rollback_unavailable"}));
        reasons.push(serde_json::json!({"reason": "unsupported"}));
        assert_eq!(reasons.len(), MAX_HANDOFF_BLOCKING_REASONS + 1);
        assert!(serde_json::from_value::<HandoffReport>(value).is_err());
    }

    #[test]
    fn handoff_errors_and_status_events_have_explicit_wire_shapes() {
        for (code, name) in [
            (ErrorCode::HandoffInProgress, "handoff_in_progress"),
            (ErrorCode::HandoffUnavailable, "handoff_unavailable"),
            (ErrorCode::InvalidHandoffTicket, "invalid_handoff_ticket"),
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), name);
        }
        let plain = ProtocolError::new(ErrorCode::HandoffInProgress, "daemon handoff in progress");
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("handoff_blocking_reasons")
                .is_none()
        );
        let refused = ProtocolError::handoff_unavailable(vec![
            HandoffBlockingReason::NotPackaged,
            HandoffBlockingReason::Unsupported,
        ]);
        assert_eq!(refused.code, ErrorCode::HandoffUnavailable);
        let value = serde_json::to_value(ServerFrame::Error {
            request_id: Some(4),
            error: refused.clone(),
        })
        .unwrap();
        assert_eq!(
            value["error"]["handoff_blocking_reasons"],
            serde_json::json!([{"reason": "not_packaged"}, {"reason": "unsupported"}])
        );
        assert!(matches!(
            serde_json::from_value::<ServerFrame>(value).unwrap(),
            ServerFrame::Error { error, .. } if error == refused
        ));
        let oversized = serde_json::json!({
            "code": "handoff_unavailable",
            "message": "daemon handoff is unavailable",
            "handoff_blocking_reasons": vec![serde_json::json!({"reason": "unsupported"}); 5],
        });
        assert!(serde_json::from_value::<ProtocolError>(oversized).is_err());

        for (phase, expected) in [
            (
                HandoffStatusPhase::Preparing,
                serde_json::json!({"state": "preparing"}),
            ),
            (
                HandoffStatusPhase::Fenced,
                serde_json::json!({"state": "fenced"}),
            ),
            (
                HandoffStatusPhase::Cancelled {
                    reason: HandoffCancelReason::TimedOut,
                },
                serde_json::json!({"state": "cancelled", "reason": "timed_out"}),
            ),
        ] {
            let event = SubscriptionEvent::HandoffStatus {
                daemon_generation: 2,
                phase,
            };
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(
                value,
                serde_json::json!({
                    "type": "handoff_status",
                    "daemon_generation": 2,
                    "phase": expected,
                })
            );
            assert_eq!(
                serde_json::from_value::<SubscriptionEvent>(value).unwrap(),
                event
            );
        }
    }

    #[test]
    fn handoff_claimed_controllers_are_bounded_and_unique() {
        let controller = |controller_id| HandoffClaimedController {
            splint_id: SplintId::new(),
            incarnation: 1,
            controller_id,
            modes: vec![ControlMode::Input, ControlMode::Resize],
        };
        let response = Response::HandoffControlClaimed {
            controllers: vec![controller(1), controller(2)],
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["type"], "handoff_control_claimed");
        assert_eq!(
            value["controllers"][0]["modes"],
            serde_json::json!(["input", "resize"])
        );
        assert_eq!(serde_json::from_value::<Response>(value).unwrap(), response);
        let Response::HandoffControlClaimed { controllers } = response else {
            unreachable!();
        };
        validate_handoff_claimed_controllers(&controllers).unwrap();
        validate_handoff_claimed_controllers(&[]).unwrap();

        let duplicate_splint = HandoffClaimedController {
            controller_id: 3,
            ..controllers[0].clone()
        };
        for invalid in [
            vec![controller(0)],
            vec![HandoffClaimedController {
                incarnation: 0,
                ..controller(1)
            }],
            vec![HandoffClaimedController {
                modes: Vec::new(),
                ..controller(1)
            }],
            vec![controller(1), controller(1)],
            vec![controllers[0].clone(), duplicate_splint],
        ] {
            assert_eq!(
                validate_handoff_claimed_controllers(&invalid)
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidArgument
            );
        }
        let oversized = serde_json::json!({
            "type": "handoff_control_claimed",
            "controllers": (1..=MAX_HANDOFF_CLAIMED_CONTROLLERS as u64 + 1)
                .map(|id| serde_json::to_value(controller(id)).unwrap())
                .collect::<Vec<_>>(),
        });
        assert!(serde_json::from_value::<Response>(oversized).is_err());
    }
}
