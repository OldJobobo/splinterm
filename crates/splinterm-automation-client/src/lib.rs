#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    reason = "the extracted internal API preserves concise compatibility docs; errors remain typed"
)]

//! Internal daemon client boundary for non-Wayland automation.
//!
//! This Rust API is reusable by Splinterm clients, but is not part of the
//! supported JSON/NDJSON compatibility contract.

use std::{
    collections::{HashMap, VecDeque},
    env,
    io::{self, ErrorKind, IoSliceMut, Write as _},
    os::fd::{AsFd, OwnedFd},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, recvmsg};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use splinterm_core::{DojoId, LayoutNode, SplintId, TopologyRevision, WindowId};
use splinterm_filemap::ReadOnlyFileMap;
use splinterm_protocol::{
    AccessGrant, AccessScope, AuditDecision, AuditOperation, AuditOutcome, AuditPage,
    AutomationScope, ClientFrame, ClientRole, ControlStatus, ControlTransferOutcome, ErrorCode,
    ImageContentMetadata, ImageContentRequest, ImageTransferMode, MAX_FRAME_BYTES,
    MAX_IMAGE_CHUNK_BYTES, MAX_IMAGE_CHUNK_WINDOW, PROTOCOL_VERSION, PersistentAuthorizationStatus,
    Request, Response, RestoreLeafResult, ScrollbackPage, SearchPage, ServerFrame, ServerLimits,
    SplintLifecycle, SplintRuntimeSummary, TerminalRow, TerminalSnapshot, TerminalUpdate,
    TopologyChangeKind, TopologySnapshot, encode_frame, image_content_socket_path,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use tokio_util::sync::CancellationToken;

const READ_CHUNK_BYTES: usize = 16 * 1024;
const MAX_BUFFERED_BYTES: usize = MAX_FRAME_BYTES + 4 + READ_CHUNK_BYTES;
const MAX_QUEUED_EVENTS: usize = 64;
const MAX_QUEUED_EVENT_BYTES: usize = MAX_FRAME_BYTES + 4;
const CLI_SCHEMA_V1: &str = "splinterm.cli.v1";
const CLI_EVENT_SCHEMA_V1: &str = "splinterm.cli.event.v1";

fn decimal_id(value: u64, label: &str) -> Result<String> {
    if value == 0 {
        bail!("{label} must be nonzero");
    }
    Ok(value.to_string())
}

/// Stable public v1 symbolic error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CliErrorCodeV1 {
    AuthenticationFailed,
    HandshakeRequired,
    IncompatibleVersion,
    InvalidRequest,
    UnsupportedSchema,
    ConsentUnavailable,
    ConsentDenied,
    Unauthorized,
    ConfirmationRequired,
    ControllerUnavailable,
    ControlTransferUnavailable,
    StaleTopology,
    NotFound,
    StaleIncarnation,
    InvalidArgument,
    ResourceLimit,
    Cancelled,
    Timeout,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CliErrorV1 {
    code: CliErrorCodeV1,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_topology_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_terminal_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct GenericEnvelopeDataV1 {
    schema: &'static str,
    request_id: String,
    operation: &'static str,
    ok: bool,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<CliErrorV1>,
}

/// Opaque explicit v1 envelope for reviewed operation projections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CliEnvelopeV1(GenericEnvelopeDataV1);

impl CliEnvelopeV1 {
    /// Creates a successful operation envelope from an explicit public projection.
    fn success(
        operation: &'static str,
        resource: Option<Value>,
        data: Value,
        truncated: bool,
    ) -> Result<Self> {
        Ok(Self(GenericEnvelopeDataV1 {
            schema: CLI_SCHEMA_V1,
            request_id: decimal_id(1, "request ID")?,
            operation,
            ok: true,
            truncated,
            resource,
            data: Some(data),
            error: None,
        }))
    }

    /// Creates a failed operation envelope with no terminal or query body.
    pub fn failure(
        operation: &'static str,
        code: CliErrorCodeV1,
        message: impl Into<String>,
        retryable: bool,
    ) -> Result<Self> {
        let message = message.into();
        if message.is_empty() || message.chars().count() > 1024 {
            bail!("public error message length is outside 1..=1024 characters");
        }
        Ok(Self(GenericEnvelopeDataV1 {
            schema: CLI_SCHEMA_V1,
            request_id: decimal_id(1, "request ID")?,
            operation,
            ok: false,
            truncated: false,
            resource: None,
            data: None,
            error: Some(CliErrorV1 {
                code,
                message,
                retryable,
                current_topology_revision: None,
                current_terminal_revision: None,
            }),
        }))
    }

    /// Creates a failed operation envelope preserving reviewed revision evidence.
    pub fn protocol_failure(
        operation: &'static str,
        error: &splinterm_protocol::ProtocolError,
        message: impl Into<String>,
    ) -> Result<Self> {
        let message = message.into();
        if message.is_empty() || message.chars().count() > 1024 {
            bail!("public error message length is outside 1..=1024 characters");
        }
        let (code, retryable) = public_error_code(error.code);
        Ok(Self(GenericEnvelopeDataV1 {
            schema: CLI_SCHEMA_V1,
            request_id: decimal_id(1, "request ID")?,
            operation,
            ok: false,
            truncated: false,
            resource: None,
            data: None,
            error: Some(CliErrorV1 {
                code,
                message,
                retryable,
                current_topology_revision: error
                    .current_topology_revision
                    .map(TopologyRevision::get),
                current_terminal_revision: None,
            }),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DojoSummaryV1 {
    dojo_id: String,
    name: String,
    window_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct WindowSummaryV1 {
    dojo_id: String,
    window_id: String,
    title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SplintLifecycleV1 {
    Running,
    Exited,
    Restorable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SplintSummaryV1 {
    dojo_id: String,
    window_id: String,
    splint_id: String,
    title: String,
    lifecycle: SplintLifecycleV1,
    current_incarnation: Option<u64>,
    last_incarnation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ListDojosDataV1 {
    dojos: Vec<DojoSummaryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct InspectTopologyDataV1 {
    dojos: Vec<DojoSummaryV1>,
    windows: Vec<WindowSummaryV1>,
    splints: Vec<SplintSummaryV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct InspectSplintResourceV1 {
    dojo_id: String,
    window_id: String,
    splint_id: String,
    topology_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct InspectSplintDataV1 {
    title: String,
    lifecycle: SplintLifecycleV1,
    current_incarnation: Option<u64>,
    last_incarnation: Option<u64>,
    exit_code: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalReadResourceV1 {
    dojo_id: String,
    window_id: String,
    splint_id: String,
    incarnation: u64,
    terminal_revision: u64,
    history_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalReadProvenanceV1 {
    pub dojo_id: DojoId,
    pub window_id: WindowId,
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub terminal_revision: u64,
    pub history_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalActionDataV1 {
    acknowledged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalReadRowV1 {
    row_id: Option<u64>,
    linebreak: bool,
    cells: Vec<ProjectedTerminalCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalCursorV1 {
    column: usize,
    row: usize,
    visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalSnapshotDataV1 {
    content_encoding: &'static str,
    columns: usize,
    rows: Vec<TerminalReadRowV1>,
    cursor: TerminalCursorV1,
    continuation_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ScrollbackPageDataV1 {
    kind: &'static str,
    content_encoding: &'static str,
    rows: Vec<TerminalReadRowV1>,
    has_older: bool,
    continuation_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SearchMatchV1 {
    row_id: u64,
    start_column: usize,
    end_column: usize,
    preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SearchResultsDataV1 {
    kind: &'static str,
    matches: Vec<SearchMatchV1>,
    timed_out: bool,
    continuation_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadResyncReasonV1 {
    StaleRevision,
    HistoryReplaced,
    RetentionGap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReadResyncDataV1 {
    kind: &'static str,
    reason: ReadResyncReasonV1,
    continuation_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalContinuationV1 {
    Scrollback {
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
        before_row_id: u64,
    },
    Search {
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
        daemon_cursor: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuthorizationResourceV1 {
    dojo_id: String,
    window_id: String,
    splint_id: String,
    incarnation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuthorizationGrantV1 {
    #[serde(skip_serializing_if = "Option::is_none")]
    grant_id: Option<String>,
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_rule_id: Option<String>,
    scopes: Vec<&'static str>,
    expires_unix_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuthorizationStatusDataV1 {
    grants: Vec<AuthorizationGrantV1>,
    development_bypass: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuditPeerV1 {
    uid: u32,
    executable_path: String,
    executable_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inode: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuditResourceV1 {
    #[serde(skip_serializing_if = "Option::is_none")]
    dojo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    splint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    incarnation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuditRecordV1 {
    schema: &'static str,
    retention: &'static str,
    audit_id: String,
    unix_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_rule_id: Option<String>,
    peer: AuditPeerV1,
    operation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<AuditResourceV1>,
    requested_scopes: Vec<&'static str>,
    decision: &'static str,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    argument_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executable_basename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AuditDataV1 {
    retention: &'static str,
    records: Vec<AuditRecordV1>,
    retention_gap: bool,
    oldest_available_audit_id: Option<String>,
    newest_available_audit_id: Option<String>,
    next_after_audit_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MutationResourceV1 {
    #[serde(skip_serializing_if = "Option::is_none")]
    dojo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    splint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    topology_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    incarnation: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct CreatedResultV1 {
    created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct MutationResultV1 {
    committed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct ProcessStartedV1 {
    started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RestoreLeafV1 {
    splint_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    incarnation: Option<u64>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<CliErrorCodeV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RestoreManyV1 {
    results: Vec<RestoreLeafV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct KillResultV1 {
    terminated: bool,
    confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RevokeResultV1 {
    revoked_grant_id: String,
    confirmed: bool,
}

fn checked_public_text(value: &str, label: &str) -> Result<String> {
    if value.chars().count() > 255 {
        bail!("{label} exceeds public v1 bounds");
    }
    Ok(value.to_owned())
}

fn project_terminal_cells(
    row: &splinterm_protocol::TerminalRow,
) -> Result<Vec<ProjectedTerminalCell>> {
    let mut cells = Vec::new();
    let mut index = 0;
    while index < row.cells.len() {
        let cell = &row.cells[index];
        if cell.spacer_remaining.is_some() {
            index += 1;
            continue;
        }
        let mut width = 1_usize;
        while index + width < row.cells.len()
            && row.cells[index + width]
                .spacer_remaining
                .is_some_and(|remaining| remaining > 0)
        {
            width += 1;
        }
        if width > 2 || cell.content.chars().count() > 64 {
            bail!("terminal cell exceeds public v1 bounds");
        }
        cells.push(ProjectedTerminalCell {
            text: cell.content.clone(),
            width: u8::try_from(width)?,
        });
        index += width;
    }
    Ok(cells)
}

/// Projects semantic terminal rows into the shared bounded public shape.
///
/// Spacer cells are collapsed into their leading cell, preserving a display width
/// of one or two columns without exposing terminal attributes or private row IDs.
pub fn project_terminal_rows(
    rows: &[splinterm_protocol::TerminalRow],
) -> Result<Vec<ProjectedTerminalRow>> {
    rows.iter()
        .map(|row| {
            Ok(ProjectedTerminalRow {
                linebreak: row.linebreak,
                cells: project_terminal_cells(row)?,
            })
        })
        .collect()
}

fn blank_terminal_row(columns: usize) -> TerminalRow {
    TerminalRow {
        row_id: None,
        linebreak: false,
        cells: Vec::with_capacity(columns),
    }
}

/// Applies one validated aggregate update to a retained non-Wayland snapshot.
///
/// History replacement, clear, and reflow require an explicit resnapshot and are
/// rejected rather than presenting retained terminal contents as current.
pub fn apply_terminal_update(
    snapshot: &mut TerminalSnapshot,
    update: TerminalUpdate,
) -> Result<()> {
    update
        .validate_against(
            snapshot.revision,
            snapshot.history_generation,
            snapshot.columns,
            snapshot.rows,
        )
        .map_err(|error| anyhow::anyhow!(error.message))?;
    if update.scrollback.as_ref().is_some_and(|scrollback| {
        !matches!(
            scrollback.transition,
            splinterm_protocol::HistoryTransition::Append { .. }
        )
    }) {
        bail!("terminal history replacement requires resynchronization");
    }

    // Keep the retained state intact unless every dimension, identity, and
    // projection invariant accepts the complete aggregate update.
    let mut candidate = snapshot.clone();
    if let Some(columns) = update.columns {
        candidate.columns = columns;
        for row in &mut candidate.visible_rows {
            row.cells.truncate(columns);
        }
    }
    if let Some(rows) = update.row_count {
        candidate.rows = rows;
        candidate
            .visible_rows
            .resize_with(rows, || blank_terminal_row(candidate.columns));
        candidate.visible_rows.truncate(rows);
    }
    for patch in update.rows {
        if patch.index >= candidate.rows || patch.row.cells.len() > candidate.columns {
            bail!("terminal row patch exceeds current dimensions");
        }
        candidate.visible_rows[patch.index] = patch.row;
    }
    if let Some(cursor) = update.cursor {
        candidate.cursor_column = cursor.column;
        candidate.cursor_row = cursor.row;
        candidate.cursor_deferred_wrap = cursor.deferred_wrap;
    }
    if let Some(title) = update.title {
        candidate.title = title;
    }
    if let Some(modes) = update.input_modes {
        candidate.input_modes = modes;
    }
    if let Some(screen) = update.active_screen {
        candidate.active_screen = screen;
    }
    if let Some(palette) = update.palette {
        candidate.palette = palette;
    }
    if let Some(colors) = update.default_colors {
        candidate.default_colors = colors;
    }
    if let Some(scrollback) = update.scrollback {
        candidate.history_generation = scrollback.history_generation;
        candidate.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
        candidate.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
        candidate.scrollback_rows.clear();
        candidate.available_scrollback_rows = scrollback.available_rows;
        candidate.omitted_oldest_scrollback_rows = scrollback.available_rows;
    }
    candidate.revision = update.revision;
    candidate
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    *snapshot = candidate;
    Ok(())
}

fn public_rows(rows: &[splinterm_protocol::TerminalRow]) -> Result<Vec<TerminalReadRowV1>> {
    rows.iter()
        .zip(project_terminal_rows(rows)?)
        .map(|(row, projected)| {
            if projected
                .cells
                .iter()
                .any(|cell| cell.text.chars().count() > 32)
            {
                bail!("terminal cell exceeds public v1 bounds");
            }
            Ok(TerminalReadRowV1 {
                row_id: row.row_id,
                linebreak: projected.linebreak,
                cells: projected.cells,
            })
        })
        .collect()
}

fn base64url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(char::from(ALPHABET[((bits >> 18) & 63) as usize]));
        output.push(char::from(ALPHABET[((bits >> 12) & 63) as usize]));
        if chunk.len() > 1 {
            output.push(char::from(ALPHABET[((bits >> 6) & 63) as usize]));
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[(bits & 63) as usize]));
        }
    }
    output
}

fn base64url_decode(input: &str) -> Result<Vec<u8>> {
    if !(16..=256).contains(&input.len()) {
        bail!("continuation cursor length is outside public v1 bounds");
    }
    let value = |byte| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    };
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    for byte in input.bytes() {
        accumulator = (accumulator << 6)
            | u32::from(value(byte).context("continuation cursor is not base64url")?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(u8::try_from(accumulator >> bits)?);
            accumulator &= (1_u32 << bits).wrapping_sub(1);
        }
    }
    if bits >= 6 || accumulator != 0 || base64url_encode(&output) != input {
        bail!("continuation cursor is not canonical base64url");
    }
    Ok(output)
}

fn push_u64(output: &mut Vec<u8>, value: u64, label: &str) -> Result<()> {
    if value == 0 {
        bail!("{label} must be nonzero");
    }
    output.extend_from_slice(&value.to_be_bytes());
    Ok(())
}

fn read_cursor_u64(input: &[u8], offset: &mut usize) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .context("continuation cursor overflow")?;
    let bytes: [u8; 8] = input
        .get(*offset..end)
        .context("continuation cursor is truncated")?
        .try_into()?;
    *offset = end;
    Ok(u64::from_be_bytes(bytes))
}

fn read_u64(input: &[u8], offset: &mut usize, label: &str) -> Result<u64> {
    let value = read_cursor_u64(input, offset)?;
    if value == 0 {
        bail!("continuation cursor {label} must be nonzero");
    }
    Ok(value)
}

/// Encodes private paging provenance into an opaque public v1 cursor.
pub fn encode_terminal_cursor(cursor: &TerminalContinuationV1) -> Result<String> {
    let (kind, splint_id, incarnation, revision, generation) = match cursor {
        TerminalContinuationV1::Scrollback {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            ..
        } => (
            1,
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
        ),
        TerminalContinuationV1::Search {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            ..
        } => (
            2,
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
        ),
    };
    let mut bytes = vec![1, kind];
    bytes.extend_from_slice(splint_id.to_string().as_bytes());
    push_u64(&mut bytes, *incarnation, "incarnation")?;
    bytes.extend_from_slice(&revision.to_be_bytes());
    push_u64(&mut bytes, *generation, "history generation")?;
    match cursor {
        TerminalContinuationV1::Scrollback { before_row_id, .. } => {
            push_u64(&mut bytes, *before_row_id, "before row ID")?;
        }
        TerminalContinuationV1::Search { daemon_cursor, .. } => {
            let length = u8::try_from(daemon_cursor.len())?;
            if length == 0 || length > 32 || !daemon_cursor.is_ascii() {
                bail!("daemon search cursor exceeds public v1 bounds");
            }
            bytes.push(length);
            bytes.extend_from_slice(daemon_cursor.as_bytes());
        }
    }
    let encoded = base64url_encode(&bytes);
    if encoded.len() > 256 {
        bail!("continuation cursor exceeds public v1 bounds");
    }
    Ok(encoded)
}

/// Decodes and validates an opaque public v1 terminal cursor.
pub fn decode_terminal_cursor(encoded: &str) -> Result<TerminalContinuationV1> {
    let bytes = base64url_decode(encoded)?;
    let (&version, rest) = bytes
        .split_first()
        .context("continuation cursor is empty")?;
    let (&kind, rest) = rest
        .split_first()
        .context("continuation cursor omits kind")?;
    if version != 1 || rest.len() < 60 {
        bail!("unsupported or truncated continuation cursor");
    }
    let splint_id = std::str::from_utf8(&rest[..36])?.parse()?;
    let mut offset = 36;
    let incarnation = read_u64(rest, &mut offset, "incarnation")?;
    let terminal_revision = read_cursor_u64(rest, &mut offset)?;
    let history_generation = read_u64(rest, &mut offset, "history generation")?;
    match kind {
        1 => {
            let before_row_id = read_u64(rest, &mut offset, "before row ID")?;
            if offset != rest.len() {
                bail!("scrollback cursor has trailing data");
            }
            Ok(TerminalContinuationV1::Scrollback {
                splint_id,
                incarnation,
                terminal_revision,
                history_generation,
                before_row_id,
            })
        }
        2 => {
            let length = usize::from(*rest.get(offset).context("search cursor omits length")?);
            offset += 1;
            let daemon_cursor = std::str::from_utf8(
                rest.get(offset..offset + length)
                    .context("search cursor is truncated")?,
            )?
            .to_owned();
            if length == 0 || offset + length != rest.len() {
                bail!("search cursor has invalid length");
            }
            Ok(TerminalContinuationV1::Search {
                splint_id,
                incarnation,
                terminal_revision,
                history_generation,
                daemon_cursor,
            })
        }
        _ => bail!("continuation cursor kind is unsupported"),
    }
}

fn public_lifecycle(runtime: &SplintRuntimeSummary) -> SplintLifecycleV1 {
    match runtime.lifecycle {
        SplintLifecycle::Starting | SplintLifecycle::Running => SplintLifecycleV1::Running,
        SplintLifecycle::Exited if runtime.restorable => SplintLifecycleV1::Restorable,
        SplintLifecycle::Exited => SplintLifecycleV1::Exited,
    }
}

fn runtime_for(snapshot: &TopologySnapshot, splint_id: SplintId) -> Result<&SplintRuntimeSummary> {
    snapshot
        .runtimes
        .iter()
        .find(|runtime| runtime.splint_id == splint_id)
        .context("validated topology omitted Splint runtime")
}

fn append_splint_summaries(
    node: &LayoutNode,
    snapshot: &TopologySnapshot,
    dojo_id: splinterm_core::DojoId,
    window_id: splinterm_core::WindowId,
    summaries: &mut Vec<SplintSummaryV1>,
) -> Result<()> {
    match node {
        LayoutNode::Leaf(splint) => {
            let runtime = runtime_for(snapshot, splint.id)?;
            summaries.push(SplintSummaryV1 {
                dojo_id: dojo_id.to_string(),
                window_id: window_id.to_string(),
                splint_id: splint.id.to_string(),
                title: checked_public_text(&splint.title, "Splint title")?,
                lifecycle: public_lifecycle(runtime),
                current_incarnation: runtime.live_incarnation,
                last_incarnation: runtime.last_incarnation,
            });
        }
        LayoutNode::Branch { first, second, .. } => {
            append_splint_summaries(first, snapshot, dojo_id, window_id, summaries)?;
            append_splint_summaries(second, snapshot, dojo_id, window_id, summaries)?;
        }
    }
    Ok(())
}

fn topology_projection(snapshot: &TopologySnapshot) -> Result<InspectTopologyDataV1> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let mut dojos = Vec::new();
    let mut windows = Vec::new();
    let mut splints = Vec::new();
    for dojo in snapshot.lair.dojos() {
        dojos.push(DojoSummaryV1 {
            dojo_id: dojo.id.to_string(),
            name: checked_public_text(&dojo.name, "Dojo name")?,
            window_count: dojo.windows.len(),
        });
        for window in &dojo.windows {
            windows.push(WindowSummaryV1 {
                dojo_id: dojo.id.to_string(),
                window_id: window.id.to_string(),
                title: checked_public_text(&window.title, "window title")?,
            });
            append_splint_summaries(&window.root, snapshot, dojo.id, window.id, &mut splints)?;
        }
    }
    if dojos.len() > 256 || windows.len() > 1_024 || splints.len() > 4_096 {
        bail!("topology exceeds public v1 collection bounds");
    }
    Ok(InspectTopologyDataV1 {
        dojos,
        windows,
        splints,
    })
}

/// Converts a validated private topology snapshot to the reviewed `list_dojos` envelope.
pub fn list_dojos_envelope(snapshot: &TopologySnapshot) -> Result<CliEnvelopeV1> {
    let projection = topology_projection(snapshot)?;
    CliEnvelopeV1::success(
        "list_dojos",
        Some(serde_json::to_value(TopologyResourceV1 {
            topology_revision: snapshot.revision.get(),
        })?),
        serde_json::to_value(ListDojosDataV1 {
            dojos: projection.dojos,
        })?,
        false,
    )
}

/// Converts a validated private topology snapshot to the reviewed topology envelope.
pub fn inspect_topology_envelope(snapshot: &TopologySnapshot) -> Result<CliEnvelopeV1> {
    CliEnvelopeV1::success(
        "inspect_topology",
        Some(serde_json::to_value(TopologyResourceV1 {
            topology_revision: snapshot.revision.get(),
        })?),
        serde_json::to_value(topology_projection(snapshot)?)?,
        false,
    )
}

/// Converts one Splint in a validated private topology snapshot to the reviewed envelope.
pub fn inspect_splint_envelope(
    snapshot: &TopologySnapshot,
    splint_id: SplintId,
) -> Result<CliEnvelopeV1> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    for dojo in snapshot.lair.dojos() {
        for window in &dojo.windows {
            if let Some(splint) = window.root.find_splint(splint_id) {
                let runtime = runtime_for(snapshot, splint_id)?;
                let exit_code = runtime.exit_status.and_then(|status| status.code);
                if exit_code.is_some_and(|code| !(0..=255).contains(&code)) {
                    bail!("Splint exit code exceeds public v1 bounds");
                }
                return CliEnvelopeV1::success(
                    "inspect_splint",
                    Some(serde_json::to_value(InspectSplintResourceV1 {
                        dojo_id: dojo.id.to_string(),
                        window_id: window.id.to_string(),
                        splint_id: splint_id.to_string(),
                        topology_revision: snapshot.revision.get(),
                    })?),
                    serde_json::to_value(InspectSplintDataV1 {
                        title: checked_public_text(&splint.title, "Splint title")?,
                        lifecycle: public_lifecycle(runtime),
                        current_incarnation: runtime.live_incarnation,
                        last_incarnation: runtime.last_incarnation,
                        exit_code: exit_code.map(u8::try_from).transpose()?,
                    })?,
                    false,
                );
            }
        }
    }
    bail!("validated topology omitted requested Splint")
}

/// Converts a validated private terminal snapshot to the reviewed one-shot projection.
pub fn terminal_snapshot_envelope(
    dojo_id: DojoId,
    window_id: WindowId,
    snapshot: &TerminalSnapshot,
) -> Result<CliEnvelopeV1> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let column =
        usize::try_from(snapshot.cursor_column).context("terminal cursor column is negative")?;
    let row = usize::try_from(snapshot.cursor_row).context("terminal cursor row is negative")?;
    if column > snapshot.columns || row >= snapshot.rows {
        bail!("terminal cursor exceeds public v1 bounds");
    }
    CliEnvelopeV1::success(
        "terminal_snapshot",
        Some(serde_json::to_value(TerminalReadResourceV1 {
            dojo_id: dojo_id.to_string(),
            window_id: window_id.to_string(),
            splint_id: snapshot.splint_id.to_string(),
            incarnation: snapshot.incarnation,
            terminal_revision: snapshot.revision,
            history_generation: snapshot.history_generation,
        })?),
        serde_json::to_value(TerminalSnapshotDataV1 {
            content_encoding: "unicode_scalars",
            columns: snapshot.columns,
            rows: public_rows(&snapshot.visible_rows)?,
            cursor: TerminalCursorV1 {
                column,
                row,
                visible: snapshot.input_modes.cursor_visible,
            },
            continuation_cursor: None,
        })?,
        false,
    )
}

fn terminal_read_resource(
    dojo_id: DojoId,
    window_id: WindowId,
    splint_id: SplintId,
    incarnation: u64,
    terminal_revision: u64,
    history_generation: u64,
) -> TerminalReadResourceV1 {
    TerminalReadResourceV1 {
        dojo_id: dojo_id.to_string(),
        window_id: window_id.to_string(),
        splint_id: splint_id.to_string(),
        incarnation,
        terminal_revision,
        history_generation,
    }
}

/// Converts a validated private scrollback page to the reviewed public projection.
pub fn scrollback_page_envelope(
    dojo_id: DojoId,
    window_id: WindowId,
    page: &ScrollbackPage,
) -> Result<CliEnvelopeV1> {
    page.validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let continuation_cursor = if page.has_older {
        Some(encode_terminal_cursor(
            &TerminalContinuationV1::Scrollback {
                splint_id: page.splint_id,
                incarnation: page.incarnation,
                terminal_revision: page.terminal_revision,
                history_generation: page.history_generation,
                before_row_id: page.rows.first().and_then(|row| row.row_id).context(
                    "validated scrollback page with older rows omitted its first row identity",
                )?,
            },
        )?)
    } else {
        None
    };
    CliEnvelopeV1::success(
        "scrollback_page",
        Some(serde_json::to_value(terminal_read_resource(
            dojo_id,
            window_id,
            page.splint_id,
            page.incarnation,
            page.terminal_revision,
            page.history_generation,
        ))?),
        serde_json::to_value(ScrollbackPageDataV1 {
            kind: "page",
            content_encoding: "unicode_scalars",
            rows: public_rows(&page.rows)?,
            has_older: page.has_older,
            continuation_cursor,
        })?,
        page.has_older,
    )
}

/// Converts a validated private search page to the reviewed public projection.
pub fn search_page_envelope(
    dojo_id: DojoId,
    window_id: WindowId,
    page: &SearchPage,
) -> Result<CliEnvelopeV1> {
    page.validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let continuation_cursor = page
        .next_cursor
        .as_ref()
        .map(|daemon_cursor| {
            encode_terminal_cursor(&TerminalContinuationV1::Search {
                splint_id: page.splint_id,
                incarnation: page.incarnation,
                terminal_revision: page.terminal_revision,
                history_generation: page.history_generation,
                daemon_cursor: daemon_cursor.clone(),
            })
        })
        .transpose()?;
    let matches = page
        .matches
        .iter()
        .map(|item| SearchMatchV1 {
            row_id: item.row_id,
            start_column: item.start_column,
            end_column: item.end_column,
            preview: item.preview.clone(),
        })
        .collect();
    CliEnvelopeV1::success(
        "search_scrollback",
        Some(serde_json::to_value(terminal_read_resource(
            dojo_id,
            window_id,
            page.splint_id,
            page.incarnation,
            page.terminal_revision,
            page.history_generation,
        ))?),
        serde_json::to_value(SearchResultsDataV1 {
            kind: "results",
            matches,
            timed_out: page.timed_out,
            continuation_cursor,
        })?,
        page.next_cursor.is_some(),
    )
}

/// Creates a successful scrollback/search resynchronization envelope.
pub fn read_resync_envelope(
    operation: &'static str,
    provenance: TerminalReadProvenanceV1,
    reason: ReadResyncReasonV1,
) -> Result<CliEnvelopeV1> {
    if !matches!(operation, "scrollback_page" | "search_scrollback") {
        bail!("unsupported read resynchronization operation");
    }
    CliEnvelopeV1::success(
        operation,
        Some(serde_json::to_value(terminal_read_resource(
            provenance.dojo_id,
            provenance.window_id,
            provenance.splint_id,
            provenance.incarnation,
            provenance.terminal_revision,
            provenance.history_generation,
        ))?),
        serde_json::to_value(ReadResyncDataV1 {
            kind: "resync_required",
            reason,
            continuation_cursor: None,
        })?,
        false,
    )
}

fn automation_scope_name(scope: AutomationScope) -> &'static str {
    match scope {
        AutomationScope::TopologyMetadataRead => "topology_metadata_read",
        AutomationScope::TopologySubscribe => "topology_subscribe",
        AutomationScope::TerminalVisibleRead => "terminal_visible_read",
        AutomationScope::TerminalSubscribe => "terminal_subscribe",
        AutomationScope::ScrollbackRead => "scrollback_read",
        AutomationScope::ScrollbackSearch => "scrollback_search",
        AutomationScope::ControllerAcquire => "controller_acquire",
        AutomationScope::ControllerTransfer => "controller_transfer",
        AutomationScope::Input => "input",
        AutomationScope::Resize => "resize",
        AutomationScope::ProcessSpawn => "process_spawn",
        AutomationScope::ProcessRestore => "process_restore",
        AutomationScope::ProcessTerminate => "process_terminate",
        AutomationScope::TopologyLayoutMutate => "topology_layout_mutate",
        AutomationScope::TopologyNameMutate => "topology_name_mutate",
        AutomationScope::AuthorizationInspect => "authorization_inspect",
        AutomationScope::AuthorizationRevoke => "authorization_revoke",
        AutomationScope::AuditInspect => "audit_inspect",
    }
}

fn access_scope_name(scope: AccessScope) -> Option<&'static str> {
    match scope {
        AccessScope::Observe => Some("terminal_visible_read"),
        AccessScope::Scrollback => Some("scrollback_read"),
        AccessScope::Input => Some("input"),
        AccessScope::Resize => Some("resize"),
        AccessScope::Terminate => Some("process_terminate"),
        AccessScope::ControlTakeover => Some("controller_transfer"),
        AccessScope::ClipboardRead | AccessScope::ClipboardWrite => None,
    }
}

fn valid_rule_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Converts a correlated private terminal action acknowledgement to public v1.
pub fn terminal_action_envelope(
    operation: &'static str,
    provenance: TerminalReadProvenanceV1,
    dimensions: Option<(u16, u16)>,
) -> Result<CliEnvelopeV1> {
    if provenance.incarnation == 0
        || provenance.history_generation == 0
        || !matches!(operation, "input" | "resize")
        || (operation == "input" && dimensions.is_some())
        || (operation == "resize"
            && !dimensions.is_some_and(|(columns, rows)| columns > 0 && rows > 0))
    {
        bail!("terminal action acknowledgement exceeds public v1 bounds");
    }
    let (columns, rows) =
        dimensions.map_or((None, None), |(columns, rows)| (Some(columns), Some(rows)));
    CliEnvelopeV1::success(
        operation,
        Some(serde_json::to_value(terminal_read_resource(
            provenance.dojo_id,
            provenance.window_id,
            provenance.splint_id,
            provenance.incarnation,
            provenance.terminal_revision,
            provenance.history_generation,
        ))?),
        serde_json::to_value(TerminalActionDataV1 {
            acknowledged: true,
            columns,
            rows,
        })?,
        false,
    )
}

/// Converts private ephemeral and persistent authority to the reviewed status projection.
pub fn authorization_status_envelope(
    dojo_id: DojoId,
    window_id: WindowId,
    splint_id: SplintId,
    incarnation: u64,
    grants: &[AccessGrant],
    persistent: &[PersistentAuthorizationStatus],
    development_bypass: bool,
) -> Result<CliEnvelopeV1> {
    if incarnation == 0 || grants.len() + persistent.len() > 64 {
        bail!("authorization status exceeds public v1 bounds");
    }
    let mut projected = Vec::with_capacity(grants.len() + persistent.len());
    for grant in grants {
        if grant.splint_id != splint_id
            || grant.incarnation != incarnation
            || grant.expires_at_unix_seconds == 0
        {
            bail!("authorization grant has inconsistent identity or expiry");
        }
        let mut scopes = grant
            .scopes
            .iter()
            .copied()
            .filter_map(access_scope_name)
            .collect::<Vec<_>>();
        scopes.sort_unstable();
        scopes.dedup();
        projected.push(AuthorizationGrantV1 {
            grant_id: Some(decimal_id(grant.grant_id, "grant ID")?),
            source: "grant_once",
            policy_rule_id: None,
            scopes,
            expires_unix_seconds: Some(grant.expires_at_unix_seconds),
        });
    }
    for grant in persistent {
        if !valid_rule_id(&grant.policy_rule_id)
            || grant.expires_at_unix_seconds == Some(0)
            || grant.scopes.len() > 18
        {
            bail!("persistent authorization status exceeds public v1 bounds");
        }
        let mut scopes = grant
            .scopes
            .iter()
            .copied()
            .map(automation_scope_name)
            .collect::<Vec<_>>();
        scopes.sort_unstable();
        if scopes.windows(2).any(|pair| pair[0] == pair[1]) {
            bail!("persistent authorization status contains duplicate scopes");
        }
        projected.push(AuthorizationGrantV1 {
            grant_id: None,
            source: "persistent_policy",
            policy_rule_id: Some(grant.policy_rule_id.clone()),
            scopes,
            expires_unix_seconds: grant.expires_at_unix_seconds,
        });
    }
    CliEnvelopeV1::success(
        "authorization_status",
        Some(serde_json::to_value(AuthorizationResourceV1 {
            dojo_id: dojo_id.to_string(),
            window_id: window_id.to_string(),
            splint_id: splint_id.to_string(),
            incarnation,
        })?),
        serde_json::to_value(AuthorizationStatusDataV1 {
            grants: projected,
            development_bypass,
        })?,
        false,
    )
}

fn audit_operation_name(operation: AuditOperation) -> &'static str {
    match operation {
        AuditOperation::Ping => "ping",
        AuditOperation::RequestAccess => "request_access",
        AuditOperation::AuthorizationStatus => "authorization_status",
        AuditOperation::RevokeAccess => "revoke_access",
        AuditOperation::ListDojos => "list_dojos",
        AuditOperation::InspectTopology => "inspect_topology",
        AuditOperation::SubscribeTopology => "subscribe_topology",
        AuditOperation::InspectSplint => "inspect_splint",
        AuditOperation::CreateDojo => "create_dojo",
        AuditOperation::SplitSplint => "split_splint",
        AuditOperation::RelaunchSplint => "relaunch_splint",
        AuditOperation::RestoreSplint => "restore_splint",
        AuditOperation::RestoreWindow => "restore_window",
        AuditOperation::RestoreDojo => "restore_dojo",
        AuditOperation::CloseSplint => "close_splint",
        AuditOperation::SetSplitRatio => "set_split_ratio",
        AuditOperation::NewWindow => "new_window",
        AuditOperation::CloseWindow => "close_window",
        AuditOperation::RenameDojo => "rename_dojo",
        AuditOperation::RenameWindow => "rename_window",
        AuditOperation::SetWindowDefaultFocus => "set_window_default_focus",
        AuditOperation::RenameSplint => "rename_splint",
        AuditOperation::Attach => "attach",
        AuditOperation::ScrollbackPage => "scrollback_page",
        AuditOperation::SearchScrollback => "search_scrollback",
        AuditOperation::AcquireControl => "acquire_control",
        AuditOperation::SubscribeControl => "subscribe_control",
        AuditOperation::RequestControlTransfer => "request_control_transfer",
        AuditOperation::DecideControlTransfer => "decide_control_transfer",
        AuditOperation::ForceControlTransfer => "force_control_transfer",
        AuditOperation::ReleaseControl => "release_control",
        AuditOperation::Input => "input",
        AuditOperation::Resize => "resize",
        AuditOperation::Detach => "detach",
        AuditOperation::KillSplint => "kill_splint",
        AuditOperation::ProcessExit => "process_exit",
        AuditOperation::AuditInspect => "audit_inspect",
        AuditOperation::PolicyReload => "policy_reload",
    }
}

fn audit_decision_name(decision: AuditDecision) -> &'static str {
    match decision {
        AuditDecision::Allowed => "allowed",
        AuditDecision::Denied => "denied",
        AuditDecision::Revoked => "revoked",
        AuditDecision::Expired => "expired",
        AuditDecision::Matched => "matched",
        AuditDecision::Rejected => "rejected",
    }
}

fn audit_outcome_name(outcome: AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Succeeded => "succeeded",
        AuditOutcome::Failed => "failed",
        AuditOutcome::Cancelled => "cancelled",
    }
}

fn audit_record(record: &splinterm_protocol::AuditRecord) -> Result<AuditRecordV1> {
    if record.schema != "splinterm.audit.v1"
        || record.retention != "daemon_lifetime"
        || record.unix_seconds == 0
        || record.policy_generation == Some(0)
        || record
            .policy_rule_id
            .as_deref()
            .is_some_and(|value| !valid_rule_id(value))
        || record.peer.executable_path.is_empty()
        || record.peer.executable_path.chars().count() > 4096
        || record.peer.executable_sha256.len() != 64
        || !record
            .peer
            .executable_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || record.requested_scopes.len() > 18
        || record.reason.is_empty()
        || record.reason.len() > 64
        || !record.reason.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte == b'_' || index > 0 && byte.is_ascii_digit()
        })
        || record.argument_count.is_some_and(|count| count > 64)
        || record.executable_basename.as_deref().is_some_and(|value| {
            value.is_empty() || value.chars().count() > 255 || value.contains('/')
        })
    {
        bail!("audit record exceeds public v1 bounds");
    }
    let mut requested_scopes = record
        .requested_scopes
        .iter()
        .copied()
        .map(automation_scope_name)
        .collect::<Vec<_>>();
    requested_scopes.sort_unstable();
    if requested_scopes.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("audit record contains duplicate requested scopes");
    }
    let resource = record.resource.as_ref().map(|resource| AuditResourceV1 {
        dojo_id: resource.dojo_id.map(|id| id.to_string()),
        window_id: resource.window_id.map(|id| id.to_string()),
        splint_id: resource.splint_id.map(|id| id.to_string()),
        incarnation: resource.incarnation,
    });
    if resource.as_ref().is_some_and(|resource| {
        resource.dojo_id.is_none()
            && resource.window_id.is_none()
            && resource.splint_id.is_none()
            && resource.incarnation.is_none()
    }) {
        bail!("audit resource must identify at least one field");
    }
    Ok(AuditRecordV1 {
        schema: "splinterm.audit.v1",
        retention: "daemon_lifetime",
        audit_id: decimal_id(record.audit_id, "audit ID")?,
        unix_seconds: record.unix_seconds,
        policy_generation: record.policy_generation,
        policy_rule_id: record.policy_rule_id.clone(),
        peer: AuditPeerV1 {
            uid: record.peer.uid,
            executable_path: record.peer.executable_path.clone(),
            executable_sha256: record.peer.executable_sha256.clone(),
            device: record.peer.device,
            inode: record.peer.inode,
        },
        operation: audit_operation_name(record.operation),
        resource,
        requested_scopes,
        decision: audit_decision_name(record.decision),
        reason: record.reason.clone(),
        outcome: record.outcome.map(audit_outcome_name),
        argument_count: record.argument_count,
        executable_basename: record.executable_basename.clone(),
    })
}

/// Converts a private bounded audit page to the reviewed public projection.
pub fn audit_page_envelope(page: &AuditPage) -> Result<CliEnvelopeV1> {
    if page.records.len() > 128
        || page.oldest_available_audit_id == Some(0)
        || page.newest_available_audit_id == Some(0)
        || page.next_after_audit_id == Some(0)
        || page
            .records
            .windows(2)
            .any(|pair| pair[0].audit_id >= pair[1].audit_id)
    {
        bail!("audit page exceeds public v1 bounds");
    }
    let records = page
        .records
        .iter()
        .map(audit_record)
        .collect::<Result<_>>()?;
    CliEnvelopeV1::success(
        "audit_inspect",
        None,
        serde_json::to_value(AuditDataV1 {
            retention: "daemon_lifetime",
            records,
            retention_gap: page.retention_gap,
            oldest_available_audit_id: page
                .oldest_available_audit_id
                .map(|id| decimal_id(id, "oldest audit ID"))
                .transpose()?,
            newest_available_audit_id: page
                .newest_available_audit_id
                .map(|id| decimal_id(id, "newest audit ID"))
                .transpose()?,
            next_after_audit_id: page
                .next_after_audit_id
                .map(|id| decimal_id(id, "next audit ID"))
                .transpose()?,
        })?,
        page.next_after_audit_id.is_some(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationIdentityV1 {
    pub dojo_id: Option<DojoId>,
    pub window_id: Option<WindowId>,
    pub splint_id: Option<SplintId>,
    pub topology_revision: Option<u64>,
    pub incarnation: Option<u64>,
}

fn mutation_resource(identity: MutationIdentityV1) -> Result<MutationResourceV1> {
    if identity.topology_revision == Some(0) || identity.incarnation == Some(0) {
        bail!("mutation provenance must be nonzero");
    }
    Ok(MutationResourceV1 {
        dojo_id: identity.dojo_id.map(|id| id.to_string()),
        window_id: identity.window_id.map(|id| id.to_string()),
        splint_id: identity.splint_id.map(|id| id.to_string()),
        topology_revision: identity.topology_revision,
        incarnation: identity.incarnation,
    })
}

/// Creates a reviewed create/split/new-window success envelope.
pub fn created_mutation_envelope(
    operation: &'static str,
    identity: MutationIdentityV1,
) -> Result<CliEnvelopeV1> {
    if !matches!(operation, "create_dojo" | "split_splint" | "new_window")
        || identity.dojo_id.is_none()
        || identity.window_id.is_none()
        || identity.splint_id.is_none()
        || identity.topology_revision.is_none()
        || identity.incarnation.is_none()
    {
        bail!("created mutation identity does not match its operation");
    }
    CliEnvelopeV1::success(
        operation,
        Some(serde_json::to_value(mutation_resource(identity)?)?),
        serde_json::to_value(CreatedResultV1 { created: true })?,
        false,
    )
}

/// Creates a reviewed topology-commit success envelope.
pub fn committed_mutation_envelope(
    operation: &'static str,
    identity: MutationIdentityV1,
    confirmed: bool,
) -> Result<CliEnvelopeV1> {
    let destructive = matches!(operation, "close_splint" | "close_window");
    let valid = match operation {
        "close_splint" | "set_split_ratio" | "rename_splint" | "set_window_default_focus" => {
            identity.dojo_id.is_some()
                && identity.window_id.is_some()
                && identity.splint_id.is_some()
                && identity.topology_revision.is_some()
                && identity.incarnation.is_none()
        }
        "close_window" | "rename_window" => {
            identity.dojo_id.is_some()
                && identity.window_id.is_some()
                && identity.splint_id.is_none()
                && identity.topology_revision.is_some()
                && identity.incarnation.is_none()
        }
        "rename_dojo" => {
            identity.dojo_id.is_some()
                && identity.window_id.is_none()
                && identity.splint_id.is_none()
                && identity.topology_revision.is_some()
                && identity.incarnation.is_none()
        }
        _ => false,
    };
    if !valid || destructive != confirmed {
        bail!("committed mutation identity or confirmation is invalid");
    }
    CliEnvelopeV1::success(
        operation,
        Some(serde_json::to_value(mutation_resource(identity)?)?),
        serde_json::to_value(MutationResultV1 {
            committed: true,
            confirmed: destructive.then_some(true),
        })?,
        false,
    )
}

/// Creates a reviewed relaunch/restore-one success envelope.
pub fn process_started_envelope(
    operation: &'static str,
    identity: MutationIdentityV1,
) -> Result<CliEnvelopeV1> {
    if !matches!(operation, "relaunch_splint" | "restore_splint")
        || identity.dojo_id.is_none()
        || identity.window_id.is_none()
        || identity.splint_id.is_none()
        || identity.topology_revision.is_none()
        || identity.incarnation.is_none()
    {
        bail!("process start identity does not match its operation");
    }
    CliEnvelopeV1::success(
        operation,
        Some(serde_json::to_value(mutation_resource(identity)?)?),
        serde_json::to_value(ProcessStartedV1 { started: true })?,
        false,
    )
}

fn restore_leaf(result: &RestoreLeafResult) -> Result<RestoreLeafV1> {
    match (&result.error, result.incarnation) {
        (None, Some(incarnation)) if incarnation > 0 => Ok(RestoreLeafV1 {
            splint_id: result.splint_id.to_string(),
            incarnation: Some(incarnation),
            status: "restored",
            error_code: None,
        }),
        (Some(error), None) => Ok(RestoreLeafV1 {
            splint_id: result.splint_id.to_string(),
            incarnation: None,
            status: "failed",
            error_code: Some(public_error_code(error.code).0),
        }),
        _ => bail!("restore result identity and outcome are inconsistent"),
    }
}

/// Creates a reviewed restore-window/restore-Dojo aggregate envelope.
pub fn restore_many_envelope(
    operation: &'static str,
    identity: MutationIdentityV1,
    results: &[RestoreLeafResult],
) -> Result<CliEnvelopeV1> {
    let valid = match operation {
        "restore_window" => identity.dojo_id.is_some() && identity.window_id.is_some(),
        "restore_dojo" => identity.dojo_id.is_some() && identity.window_id.is_none(),
        _ => false,
    } && identity.splint_id.is_none()
        && identity.topology_revision.is_some()
        && identity.incarnation.is_none()
        && results.len() <= 4096;
    if !valid {
        bail!("aggregate restore identity does not match its operation");
    }
    let results = results.iter().map(restore_leaf).collect::<Result<_>>()?;
    CliEnvelopeV1::success(
        operation,
        Some(serde_json::to_value(mutation_resource(identity)?)?),
        serde_json::to_value(RestoreManyV1 { results })?,
        false,
    )
}

/// Creates a reviewed confirmed process-termination envelope.
pub fn kill_envelope(
    dojo_id: DojoId,
    window_id: WindowId,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<CliEnvelopeV1> {
    CliEnvelopeV1::success(
        "kill_splint",
        Some(serde_json::to_value(AuthorizationResourceV1 {
            dojo_id: dojo_id.to_string(),
            window_id: window_id.to_string(),
            splint_id: splint_id.to_string(),
            incarnation,
        })?),
        serde_json::to_value(KillResultV1 {
            terminated: true,
            confirmed: true,
        })?,
        false,
    )
}

/// Creates a reviewed confirmed ephemeral-grant revocation envelope.
pub fn revoke_envelope(
    dojo_id: DojoId,
    window_id: WindowId,
    grant: &AccessGrant,
) -> Result<CliEnvelopeV1> {
    if grant.incarnation == 0 || grant.expires_at_unix_seconds == 0 {
        bail!("revoked grant provenance is invalid");
    }
    CliEnvelopeV1::success(
        "revoke_access",
        Some(serde_json::to_value(AuthorizationResourceV1 {
            dojo_id: dojo_id.to_string(),
            window_id: window_id.to_string(),
            splint_id: grant.splint_id.to_string(),
            incarnation: grant.incarnation,
        })?),
        serde_json::to_value(RevokeResultV1 {
            revoked_grant_id: decimal_id(grant.grant_id, "grant ID")?,
            confirmed: true,
        })?,
        false,
    )
}

#[derive(Debug)]
struct DaemonProtocolFailure(splinterm_protocol::ProtocolError);

impl std::fmt::Display for DaemonProtocolFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.code == ErrorCode::DevelopmentFeatureDisabled {
            return write!(
                formatter,
                "splinterd: {} (restart with SPLINTERM_ENABLE_DEV_ATTACH=1)",
                self.0.message
            );
        }
        write!(
            formatter,
            "splinterd [{}]: {}",
            format!("{:?}", self.0.code).to_lowercase(),
            self.0.message
        )
    }
}

impl std::error::Error for DaemonProtocolFailure {}

/// The reason an in-flight daemon request was cancelled by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestCancellation {
    /// The request exceeded its client-side deadline.
    DeadlineElapsed,
    /// The caller's cancellation token was cancelled.
    Cancelled,
}

/// Typed cancellation retained through the crate's `anyhow` boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestCancelled {
    reason: RequestCancellation,
}

impl RequestCancelled {
    /// Returns why the request was cancelled.
    #[must_use]
    pub const fn reason(self) -> RequestCancellation {
        self.reason
    }
}

impl std::fmt::Display for RequestCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            RequestCancellation::DeadlineElapsed => {
                write!(formatter, "splinterd request timed out")
            }
            RequestCancellation::Cancelled => write!(formatter, "splinterd request cancelled"),
        }
    }
}

impl std::error::Error for RequestCancelled {}

/// Extracts typed client-side request cancellation from the `anyhow` boundary.
#[must_use]
pub fn request_cancellation(error: &anyhow::Error) -> Option<RequestCancellation> {
    error
        .downcast_ref::<RequestCancelled>()
        .map(|cancelled| cancelled.reason())
}

/// Extracts a daemon protocol error retained through the anyhow boundary.
#[must_use]
pub fn protocol_error(error: &anyhow::Error) -> Option<&splinterm_protocol::ProtocolError> {
    error
        .downcast_ref::<DaemonProtocolFailure>()
        .map(|failure| &failure.0)
}

/// Wraps an embedded daemon operation error for stable public error mapping.
#[must_use]
pub fn response_protocol_error(error: splinterm_protocol::ProtocolError) -> anyhow::Error {
    anyhow::Error::new(DaemonProtocolFailure(error))
}

/// Maps every private protocol error to a stable public v1 category.
#[must_use]
pub const fn public_error_code(code: ErrorCode) -> (CliErrorCodeV1, bool) {
    match code {
        ErrorCode::AuthenticationFailed => (CliErrorCodeV1::AuthenticationFailed, false),
        ErrorCode::HandshakeRequired => (CliErrorCodeV1::HandshakeRequired, false),
        ErrorCode::IncompatibleVersion => (CliErrorCodeV1::IncompatibleVersion, false),
        ErrorCode::InvalidFrame
        | ErrorCode::FrameTooLarge
        | ErrorCode::InvalidRequestId
        | ErrorCode::DuplicateRequestId
        | ErrorCode::TooManyOutstandingRequests
        | ErrorCode::UnsupportedOperation
        | ErrorCode::DevelopmentFeatureDisabled
        | ErrorCode::RequestNotFound => (CliErrorCodeV1::InvalidRequest, false),
        ErrorCode::ConsentUnavailable => (CliErrorCodeV1::ConsentUnavailable, true),
        ErrorCode::ConsentDenied => (CliErrorCodeV1::ConsentDenied, false),
        ErrorCode::Unauthorized => (CliErrorCodeV1::Unauthorized, false),
        ErrorCode::ControllerUnavailable => (CliErrorCodeV1::ControllerUnavailable, true),
        ErrorCode::ControlTransferUnavailable => (CliErrorCodeV1::ControlTransferUnavailable, true),
        ErrorCode::StaleTopology => (CliErrorCodeV1::StaleTopology, true),
        ErrorCode::NotFound | ErrorCode::ImageContentNotFound => (CliErrorCodeV1::NotFound, false),
        ErrorCode::StaleIncarnation | ErrorCode::StaleImageContent => {
            (CliErrorCodeV1::StaleIncarnation, true)
        }
        ErrorCode::InvalidArgument => (CliErrorCodeV1::InvalidArgument, false),
        ErrorCode::ResourceLimit => (CliErrorCodeV1::ResourceLimit, true),
        ErrorCode::Cancelled => (CliErrorCodeV1::Cancelled, true),
        ErrorCode::Internal => (CliErrorCodeV1::Internal, true),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PingSuccessV1 {
    status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum PingEnvelopeDataV1 {
    Success {
        schema: &'static str,
        request_id: String,
        operation: &'static str,
        ok: bool,
        data: PingSuccessV1,
        truncated: bool,
    },
    Failure {
        schema: &'static str,
        request_id: String,
        operation: &'static str,
        ok: bool,
        error: CliErrorV1,
        truncated: bool,
    },
}

/// An opaque schema-conforming public v1 response for `ping`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PingEnvelopeV1(PingEnvelopeDataV1);

impl PingEnvelopeV1 {
    /// Creates a successful v1 ping envelope.
    pub fn success(request_id: u64) -> Result<Self> {
        Ok(Self(PingEnvelopeDataV1::Success {
            schema: CLI_SCHEMA_V1,
            request_id: decimal_id(request_id, "request ID")?,
            operation: "ping",
            ok: true,
            data: PingSuccessV1 { status: "awake" },
            truncated: false,
        }))
    }

    /// Creates a failed v1 ping envelope with a bounded public message.
    pub fn failure(
        request_id: u64,
        code: CliErrorCodeV1,
        message: impl Into<String>,
        retryable: bool,
    ) -> Result<Self> {
        let message = message.into();
        if message.is_empty() || message.chars().count() > 1024 {
            bail!("public error message length is outside 1..=1024 characters");
        }
        Ok(Self(PingEnvelopeDataV1::Failure {
            schema: CLI_SCHEMA_V1,
            request_id: decimal_id(request_id, "request ID")?,
            operation: "ping",
            ok: false,
            error: CliErrorV1 {
                code,
                message,
                retryable,
                current_topology_revision: None,
                current_terminal_revision: None,
            },
            truncated: false,
        }))
    }
}

/// Writes one compact JSON document plus a newline and flushes it.
pub fn write_json_document(value: &impl Serialize) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value).context("failed to encode machine output")?;
    output
        .write_all(b"\n")
        .context("failed to write machine output")?;
    output.flush().context("failed to flush machine output")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalResourceV1 {
    splint_id: String,
    incarnation: u64,
    terminal_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TopologyResourceV1 {
    topology_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ControlResourceV1 {
    splint_id: String,
    incarnation: u64,
}

/// One bounded semantic cell in a public terminal projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectedTerminalCell {
    pub text: String,
    pub width: u8,
}

/// One bounded semantic row in a public terminal projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectedTerminalRow {
    pub linebreak: bool,
    pub cells: Vec<ProjectedTerminalCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalSnapshotV1 {
    content_encoding: &'static str,
    columns: usize,
    rows: usize,
    title: String,
    visible_rows: Vec<ProjectedTerminalRow>,
}

impl TerminalSnapshotV1 {
    fn try_from_protocol(snapshot: &TerminalSnapshot) -> Result<Self> {
        snapshot
            .validate()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        if snapshot.title.chars().count() > 1_024 {
            bail!("terminal title exceeds public event bounds");
        }
        let visible_rows = project_terminal_rows(&snapshot.visible_rows)?;
        Ok(Self {
            content_encoding: "unicode_scalars",
            columns: snapshot.columns,
            rows: snapshot.rows,
            title: snapshot.title.clone(),
            visible_rows,
        })
    }
}

#[allow(
    clippy::struct_field_names,
    reason = "field names are fixed by the reviewed public v1 JSON schema"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TopologySnapshotV1 {
    dojo_count: usize,
    window_count: usize,
    splint_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ControlSnapshotV1 {
    controlled: bool,
    locally_owned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TerminalUpdateV1 {
    content_encoding: &'static str,
    changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TopologyChangeV1 {
    kind: &'static str,
    dojo_count: usize,
    window_count: usize,
    splint_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TransferRequestedV1 {
    transfer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TransferResolvedV1 {
    transfer_id: String,
    outcome: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessRevokedV1 {
    grant_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ExitedV1 {
    code: Option<i32>,
    signal: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReasonV1 {
    SubscriberStalled,
    RevisionGap,
    HistoryReplaced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ResyncV1 {
    reason: ResyncReasonV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct EmptyDataV1 {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum CliEventDataV1 {
    TerminalSnapshot {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: TerminalResourceV1,
        data: TerminalSnapshotV1,
        truncated: bool,
    },
    TopologySnapshot {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: TopologyResourceV1,
        data: TopologySnapshotV1,
        truncated: bool,
    },
    ControlSnapshot {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: ControlResourceV1,
        data: ControlSnapshotV1,
        truncated: bool,
    },
    TerminalUpdate {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: TerminalResourceV1,
        data: TerminalUpdateV1,
        truncated: bool,
    },
    TopologyChanged {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: TopologyResourceV1,
        data: TopologyChangeV1,
        truncated: bool,
    },
    ControlStatusChanged {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: ControlResourceV1,
        data: ControlSnapshotV1,
        truncated: bool,
    },
    ControlTransferRequested {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: ControlResourceV1,
        data: TransferRequestedV1,
        truncated: bool,
    },
    ControlTransferResolved {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: ControlResourceV1,
        data: TransferResolvedV1,
        truncated: bool,
    },
    AccessRevoked {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: ControlResourceV1,
        data: AccessRevokedV1,
        truncated: bool,
    },
    Exited {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        resource: ControlResourceV1,
        data: ExitedV1,
        truncated: bool,
    },
    TerminalResync {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        stream: &'static str,
        resource: TerminalResourceV1,
        data: EmptyDataV1,
        truncated: bool,
        resync: ResyncV1,
    },
    TopologyResync {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        stream: &'static str,
        resource: TopologyResourceV1,
        data: EmptyDataV1,
        truncated: bool,
        resync: ResyncV1,
    },
    ControlResync {
        schema: &'static str,
        subscription_id: String,
        sequence: u64,
        event_type: &'static str,
        stream: &'static str,
        resource: ControlResourceV1,
        data: EmptyDataV1,
        truncated: bool,
        resync: ResyncV1,
    },
}

/// Opaque public v1 initial-state and resynchronization event record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CliEventV1(CliEventDataV1);

impl CliEventV1 {
    fn common(subscription_id: u64, sequence: u64) -> Result<(String, u64)> {
        if sequence == 0 {
            bail!("event sequence must be nonzero");
        }
        Ok((decimal_id(subscription_id, "subscription ID")?, sequence))
    }

    /// Converts a private terminal snapshot into an explicit public initial event.
    pub fn terminal_snapshot(
        subscription_id: u64,
        sequence: u64,
        snapshot: &TerminalSnapshot,
        truncated: bool,
    ) -> Result<Self> {
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::TerminalSnapshot {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "snapshot",
            resource: TerminalResourceV1 {
                splint_id: snapshot.splint_id.to_string(),
                incarnation: snapshot.incarnation,
                terminal_revision: snapshot.revision,
                history_generation: None,
            },
            data: TerminalSnapshotV1::try_from_protocol(snapshot)?,
            truncated,
        }))
    }

    /// Converts a private topology snapshot into a public count-only initial event.
    pub fn topology_snapshot(
        subscription_id: u64,
        sequence: u64,
        snapshot: &TopologySnapshot,
    ) -> Result<Self> {
        snapshot
            .validate()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        let dojo_count = snapshot.lair.dojos().len();
        let window_count = snapshot.lair.dojos().map(|dojo| dojo.windows.len()).sum();
        let splint_count = snapshot
            .lair
            .dojos()
            .flat_map(|dojo| &dojo.windows)
            .map(|window| window.root.splint_count())
            .sum();
        Ok(Self(CliEventDataV1::TopologySnapshot {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "topology_snapshot",
            resource: TopologyResourceV1 {
                topology_revision: snapshot.revision.get(),
            },
            data: TopologySnapshotV1 {
                dojo_count,
                window_count,
                splint_count,
            },
            truncated: false,
        }))
    }

    /// Converts a private controller status into a public initial event.
    pub fn control_snapshot(
        subscription_id: u64,
        sequence: u64,
        status: ControlStatus,
    ) -> Result<Self> {
        status
            .validate()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::ControlSnapshot {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "control_snapshot",
            resource: ControlResourceV1 {
                splint_id: status.splint_id.to_string(),
                incarnation: status.incarnation,
            },
            data: ControlSnapshotV1 {
                controlled: status.controlled,
                locally_owned: status.locally_owned,
            },
            truncated: false,
        }))
    }

    /// Emits a bounded terminal revision update without private bytes.
    pub fn terminal_update(
        subscription_id: u64,
        sequence: u64,
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
    ) -> Result<Self> {
        if incarnation == 0 || history_generation == 0 {
            bail!("terminal update incarnation and history generation must be nonzero");
        }
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::TerminalUpdate {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "update",
            resource: TerminalResourceV1 {
                splint_id: splint_id.to_string(),
                incarnation,
                terminal_revision,
                history_generation: Some(history_generation),
            },
            data: TerminalUpdateV1 {
                content_encoding: "unicode_scalars",
                changed: true,
            },
            truncated: false,
        }))
    }

    /// Emits reviewed topology counts for one topology change.
    pub fn topology_changed(
        subscription_id: u64,
        sequence: u64,
        kind: TopologyChangeKind,
        snapshot: &TopologySnapshot,
    ) -> Result<Self> {
        snapshot
            .validate()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        let dojo_count = snapshot.lair.dojos().len();
        let window_count = snapshot.lair.dojos().map(|dojo| dojo.windows.len()).sum();
        let splint_count = snapshot
            .lair
            .dojos()
            .flat_map(|dojo| &dojo.windows)
            .map(|window| window.root.splint_count())
            .sum();
        let kind = match kind {
            TopologyChangeKind::DojoCreated => "dojo_created",
            TopologyChangeKind::SplintSplit => "splint_split",
            TopologyChangeKind::SplintClosed => "splint_closed",
            TopologyChangeKind::SplitRatioChanged => "split_ratio_changed",
            TopologyChangeKind::WindowCreated => "window_created",
            TopologyChangeKind::WindowClosed => "window_closed",
            TopologyChangeKind::DojoRenamed => "dojo_renamed",
            TopologyChangeKind::WindowRenamed => "window_renamed",
            TopologyChangeKind::WindowDefaultFocusChanged => "window_default_focus_changed",
            TopologyChangeKind::SplintRenamed => "splint_renamed",
            TopologyChangeKind::RuntimeChanged => "runtime_changed",
        };
        Ok(Self(CliEventDataV1::TopologyChanged {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "topology_changed",
            resource: TopologyResourceV1 {
                topology_revision: snapshot.revision.get(),
            },
            data: TopologyChangeV1 {
                kind,
                dojo_count,
                window_count,
                splint_count,
            },
            truncated: false,
        }))
    }

    /// Emits one reviewed controller-status change.
    pub fn control_status_changed(
        subscription_id: u64,
        sequence: u64,
        status: ControlStatus,
    ) -> Result<Self> {
        status
            .validate()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::ControlStatusChanged {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "control_status_changed",
            resource: ControlResourceV1 {
                splint_id: status.splint_id.to_string(),
                incarnation: status.incarnation,
            },
            data: ControlSnapshotV1 {
                controlled: status.controlled,
                locally_owned: status.locally_owned,
            },
            truncated: false,
        }))
    }

    /// Emits a controller-transfer request without exposing controller IDs.
    pub fn control_transfer_requested(
        subscription_id: u64,
        sequence: u64,
        splint_id: SplintId,
        incarnation: u64,
        transfer_id: u64,
    ) -> Result<Self> {
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::ControlTransferRequested {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "control_transfer_requested",
            resource: ControlResourceV1 {
                splint_id: splint_id.to_string(),
                incarnation,
            },
            data: TransferRequestedV1 {
                transfer_id: decimal_id(transfer_id, "transfer ID")?,
            },
            truncated: false,
        }))
    }

    /// Emits a controller-transfer outcome without exposing controller IDs.
    pub fn control_transfer_resolved(
        subscription_id: u64,
        sequence: u64,
        splint_id: SplintId,
        incarnation: u64,
        transfer_id: u64,
        outcome: ControlTransferOutcome,
    ) -> Result<Self> {
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        let outcome = match outcome {
            ControlTransferOutcome::Granted => "granted",
            ControlTransferOutcome::Denied => "denied",
            ControlTransferOutcome::TimedOut => "timed_out",
            ControlTransferOutcome::Cancelled => "cancelled",
        };
        Ok(Self(CliEventDataV1::ControlTransferResolved {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "control_transfer_resolved",
            resource: ControlResourceV1 {
                splint_id: splint_id.to_string(),
                incarnation,
            },
            data: TransferResolvedV1 {
                transfer_id: decimal_id(transfer_id, "transfer ID")?,
                outcome,
            },
            truncated: false,
        }))
    }

    /// Emits grant revocation without private capability material.
    pub fn access_revoked(
        subscription_id: u64,
        sequence: u64,
        splint_id: SplintId,
        incarnation: u64,
        grant_id: u64,
    ) -> Result<Self> {
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::AccessRevoked {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "access_revoked",
            resource: ControlResourceV1 {
                splint_id: splint_id.to_string(),
                incarnation,
            },
            data: AccessRevokedV1 {
                grant_id: decimal_id(grant_id, "grant ID")?,
            },
            truncated: false,
        }))
    }

    /// Emits a terminal process exit status.
    pub fn exited(
        subscription_id: u64,
        sequence: u64,
        splint_id: SplintId,
        incarnation: u64,
        code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<Self> {
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::Exited {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "exited",
            resource: ControlResourceV1 {
                splint_id: splint_id.to_string(),
                incarnation,
            },
            data: ExitedV1 { code, signal },
            truncated: false,
        }))
    }

    /// Creates a terminal-stream resynchronization record.
    pub fn terminal_resync(
        subscription_id: u64,
        sequence: u64,
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: Option<u64>,
        reason: ResyncReasonV1,
    ) -> Result<Self> {
        if incarnation == 0 {
            bail!("terminal resync incarnation must be nonzero");
        }
        if reason == ResyncReasonV1::HistoryReplaced && history_generation.is_none() {
            bail!("history replacement requires a history generation");
        }
        if history_generation == Some(0) {
            bail!("history generation must be nonzero");
        }
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::TerminalResync {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "resync_required",
            stream: "terminal",
            resource: TerminalResourceV1 {
                splint_id: splint_id.to_string(),
                incarnation,
                terminal_revision,
                history_generation,
            },
            data: EmptyDataV1 {},
            truncated: false,
            resync: ResyncV1 { reason },
        }))
    }

    /// Creates a topology-stream resynchronization record.
    pub fn topology_resync(
        subscription_id: u64,
        sequence: u64,
        revision: TopologyRevision,
        reason: ResyncReasonV1,
    ) -> Result<Self> {
        if reason == ResyncReasonV1::HistoryReplaced {
            bail!("topology resync cannot report replaced terminal history");
        }
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::TopologyResync {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "resync_required",
            stream: "topology",
            resource: TopologyResourceV1 {
                topology_revision: revision.get(),
            },
            data: EmptyDataV1 {},
            truncated: false,
            resync: ResyncV1 { reason },
        }))
    }

    /// Creates a control-stream resynchronization record.
    pub fn control_resync(
        subscription_id: u64,
        sequence: u64,
        splint_id: SplintId,
        incarnation: u64,
        reason: ResyncReasonV1,
    ) -> Result<Self> {
        if incarnation == 0 || reason == ResyncReasonV1::HistoryReplaced {
            bail!("invalid control resync provenance or reason");
        }
        let (subscription_id, sequence) = Self::common(subscription_id, sequence)?;
        Ok(Self(CliEventDataV1::ControlResync {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id,
            sequence,
            event_type: "resync_required",
            stream: "control",
            resource: ControlResourceV1 {
                splint_id: splint_id.to_string(),
                incarnation,
            },
            data: EmptyDataV1 {},
            truncated: false,
            resync: ResyncV1 { reason },
        }))
    }
}

const IMAGE_CONTENT_HEADER_BYTES: usize = 53;
const IMAGE_MEMFD_HEADER_BYTES: usize = 45;

async fn receive_image_content(
    stream: &mut UnixStream,
    metadata: &ImageContentMetadata,
) -> Result<Vec<u8>> {
    let mut header = [0_u8; IMAGE_CONTENT_HEADER_BYTES];
    stream.read_exact(&mut header).await?;
    if &header[0..5] != b"SPIM\x01"
        || header[13..45] != metadata.digest
        || usize::try_from(u64::from_be_bytes(header[5..13].try_into().unwrap())).ok()
            != Some(metadata.byte_length)
        || usize::try_from(u32::from_be_bytes(header[45..49].try_into().unwrap())).ok()
            != Some(MAX_IMAGE_CHUNK_BYTES)
        || usize::try_from(u32::from_be_bytes(header[49..53].try_into().unwrap())).ok()
            != Some(MAX_IMAGE_CHUNK_WINDOW)
    {
        bail!("image content header does not match negotiated metadata");
    }
    let mut pixels = Vec::with_capacity(metadata.byte_length);
    let mut chunks_in_window = 0_usize;
    while pixels.len() < metadata.byte_length {
        let mut chunk_header = [0_u8; 12];
        stream.read_exact(&mut chunk_header).await?;
        let offset = usize::try_from(u64::from_be_bytes(chunk_header[0..8].try_into().unwrap()))
            .context("image chunk offset exceeds usize")?;
        let length = usize::try_from(u32::from_be_bytes(chunk_header[8..12].try_into().unwrap()))
            .expect("u32 image chunk length fits usize");
        if offset != pixels.len()
            || length == 0
            || length > MAX_IMAGE_CHUNK_BYTES
            || offset
                .checked_add(length)
                .is_none_or(|end| end > metadata.byte_length)
        {
            bail!("image content chunk is out of window or exceeds bounds");
        }
        let start = pixels.len();
        pixels.resize(start + length, 0);
        stream.read_exact(&mut pixels[start..]).await?;
        chunks_in_window += 1;
        if chunks_in_window == MAX_IMAGE_CHUNK_WINDOW || pixels.len() == metadata.byte_length {
            let mut acknowledgement = [0_u8; 9];
            acknowledgement[0] = 1;
            acknowledgement[1..9].copy_from_slice(
                &u64::try_from(pixels.len())
                    .context("image acknowledgement offset exceeds u64")?
                    .to_be_bytes(),
            );
            stream.write_all(&acknowledgement).await?;
            chunks_in_window = 0;
        }
    }
    if Sha256::digest(&pixels).as_slice() != metadata.digest {
        bail!("image content digest does not match metadata");
    }
    Ok(pixels)
}

async fn receive_image_memfd(
    stream: &mut UnixStream,
    metadata: &ImageContentMetadata,
) -> Result<ReadOnlyFileMap> {
    let descriptor = loop {
        stream.readable().await?;
        let mut marker = [0_u8; 1];
        let mut iov = [IoSliceMut::new(&mut marker)];
        let mut ancillary_space =
            [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut ancillary_space);
        let message = match recvmsg(
            stream.as_fd(),
            &mut iov,
            &mut ancillary,
            RecvFlags::CMSG_CLOEXEC,
        ) {
            Ok(message) => message,
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => continue,
            Err(error) => return Err(error.into()),
        };
        if message.bytes != 1 || marker != [b'F'] || message.flags.contains(ReturnFlags::CTRUNC) {
            bail!("image descriptor message is malformed or truncated");
        }
        let mut descriptor: Option<OwnedFd> = None;
        for item in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(mut descriptors) = item {
                if descriptor.is_some() {
                    bail!("image descriptor message contains multiple rights records");
                }
                descriptor = descriptors.next();
                if descriptors.next().is_some() {
                    bail!("image descriptor message contains multiple descriptors");
                }
            }
        }
        break descriptor.context("image descriptor message contains no descriptor")?;
    };
    let mut header = [0_u8; IMAGE_MEMFD_HEADER_BYTES];
    stream.read_exact(&mut header).await?;
    if &header[0..5] != b"SPIF\x01"
        || header[13..45] != metadata.digest
        || usize::try_from(u64::from_be_bytes(header[5..13].try_into().unwrap())).ok()
            != Some(metadata.byte_length)
    {
        bail!("image descriptor header does not match negotiated metadata");
    }
    let mapping = ReadOnlyFileMap::from_sealed_fd(descriptor, metadata.byte_length)
        .context("image descriptor is not exactly sized and immutable")?;
    if Sha256::digest(&*mapping).as_slice() != metadata.digest {
        bail!("mapped image content digest does not match metadata");
    }
    stream.write_all(&[1]).await?;
    Ok(mapping)
}

pub const MAX_IMAGE_SOURCE_CACHE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ImageCacheKey {
    content_id: u64,
    generation: u64,
    digest: [u8; 32],
}

impl From<&ImageContentMetadata> for ImageCacheKey {
    fn from(metadata: &ImageContentMetadata) -> Self {
        Self {
            content_id: metadata.content_id,
            generation: metadata.generation,
            digest: metadata.digest,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImageContentCacheMetrics {
    pub bytes: usize,
    pub entries: usize,
    pub high_water_bytes: usize,
    pub high_water_entries: usize,
}

#[derive(Clone, Debug)]
pub enum ImageContentSource {
    Buffered(Arc<[u8]>),
    Mapped(Arc<ReadOnlyFileMap>),
}

impl ImageContentSource {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Buffered(bytes) => bytes,
            Self::Mapped(mapping) => mapping,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }
}

#[derive(Debug)]
pub struct ImageContentCache {
    entries: HashMap<ImageCacheKey, ImageContentSource>,
    order: VecDeque<ImageCacheKey>,
    maximum_bytes: usize,
    metrics: ImageContentCacheMetrics,
}

impl Default for ImageContentCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            maximum_bytes: MAX_IMAGE_SOURCE_CACHE_BYTES,
            metrics: ImageContentCacheMetrics::default(),
        }
    }
}

impl ImageContentCache {
    pub fn with_maximum_bytes(maximum_bytes: usize) -> Result<Self> {
        if maximum_bytes == 0 || maximum_bytes > MAX_IMAGE_SOURCE_CACHE_BYTES {
            bail!("image source cache byte limit is invalid");
        }
        Ok(Self {
            maximum_bytes,
            ..Self::default()
        })
    }

    #[must_use]
    pub fn contains(&self, metadata: &ImageContentMetadata) -> bool {
        self.entries.contains_key(&ImageCacheKey::from(metadata))
    }

    pub fn insert(
        &mut self,
        metadata: &ImageContentMetadata,
        pixels: Vec<u8>,
    ) -> Result<ImageContentSource> {
        self.insert_source(metadata, ImageContentSource::Buffered(pixels.into()))
    }

    pub fn insert_source(
        &mut self,
        metadata: &ImageContentMetadata,
        source: ImageContentSource,
    ) -> Result<ImageContentSource> {
        if source.len() != metadata.byte_length
            || source.len() > self.maximum_bytes
            || Sha256::digest(source.as_bytes())[..] != metadata.digest
        {
            bail!("image content does not match cache metadata");
        }
        let key = ImageCacheKey::from(metadata);
        if let Some(existing) = self.entries.get(&key) {
            return Ok(existing.clone());
        }
        while self
            .metrics
            .bytes
            .checked_add(source.len())
            .is_none_or(|bytes| bytes > self.maximum_bytes)
        {
            let oldest = self
                .order
                .pop_front()
                .context("image cache accounting has no evictable entry")?;
            let removed = self
                .entries
                .remove(&oldest)
                .context("image cache order references a missing entry")?;
            self.metrics.bytes -= removed.len();
        }
        self.metrics.bytes += source.len();
        self.entries.insert(key, source.clone());
        self.order.push_back(key);
        self.metrics.entries = self.entries.len();
        self.metrics.high_water_bytes = self.metrics.high_water_bytes.max(self.metrics.bytes);
        self.metrics.high_water_entries = self.metrics.high_water_entries.max(self.metrics.entries);
        Ok(source)
    }

    #[must_use]
    pub fn get(&self, metadata: &ImageContentMetadata) -> Option<ImageContentSource> {
        self.entries.get(&ImageCacheKey::from(metadata)).cloned()
    }

    #[must_use]
    pub const fn metrics(&self) -> ImageContentCacheMetrics {
        self.metrics
    }
}

/// A negotiated connection to the private local daemon protocol.
///
/// This type is an internal implementation boundary. Public automation
/// compatibility is defined only by the checked-in JSON/NDJSON schemas.
#[derive(Debug)]
pub struct Connection {
    stream: Option<UnixStream>,
    next_request: u64,
    read_buffer: Vec<u8>,
    queued_events: VecDeque<(ServerFrame, usize)>,
    queued_event_bytes: usize,
    limits: ServerLimits,
    socket_path: Option<PathBuf>,
    trusted_ui: bool,
    unusable: bool,
}

/// Drop guard ensuring an abandoned request cannot leave a correlated response
/// or connection-owned temporary subscription behind.
struct InFlightRequest<'a> {
    connection: &'a mut Connection,
    request_id: u64,
    armed: bool,
}

impl InFlightRequest<'_> {
    async fn perform(&mut self, request: Request) -> Result<Response> {
        self.connection
            .perform_request(self.request_id, request)
            .await
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for InFlightRequest<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.connection.cancel_and_close(self.request_id);
        }
    }
}

impl Connection {
    /// Connects as the graphical client and negotiates the current private protocol.
    pub async fn connect() -> Result<Self> {
        Self::connect_role(ClientRole::TrustedUi).await
    }

    /// Connects without the trusted graphical-client authorization bypass.
    pub async fn connect_automation() -> Result<Self> {
        Self::connect_role(ClientRole::Automation).await
    }

    /// Connects as automation to one explicit socket for isolated integration tests.
    #[doc(hidden)]
    pub async fn connect_automation_at(socket: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .await
            .with_context(|| format!("cannot connect to splinterd at {}", socket.display()))?;
        Self::connect_stream_at(stream, ClientRole::Automation, Some(socket.to_owned())).await
    }

    async fn connect_role(role: ClientRole) -> Result<Self> {
        let socket = socket_path()?;
        let stream = UnixStream::connect(&socket)
            .await
            .with_context(|| format!("cannot connect to splinterd at {}", socket.display()))?;
        Self::connect_stream_at(stream, role, Some(socket)).await
    }

    #[cfg(test)]
    async fn connect_stream(stream: UnixStream, role: ClientRole) -> Result<Self> {
        Self::connect_stream_at(stream, role, None).await
    }

    async fn connect_stream_at(
        mut stream: UnixStream,
        role: ClientRole,
        socket_path: Option<PathBuf>,
    ) -> Result<Self> {
        write_frame(
            &mut stream,
            &ClientFrame::Hello {
                minimum_version: PROTOCOL_VERSION,
                maximum_version: PROTOCOL_VERSION,
                role,
            },
        )
        .await?;
        let limits = match read_frame(&mut stream).await? {
            ServerFrame::Hello {
                version, limits, ..
            } if version == PROTOCOL_VERSION => limits,
            ServerFrame::Error { error, .. } => {
                return Err(anyhow::Error::new(DaemonProtocolFailure(error)));
            }
            _ => bail!("splinterd sent an invalid handshake"),
        };
        Ok(Self {
            stream: Some(stream),
            next_request: 1,
            read_buffer: Vec::new(),
            queued_events: VecDeque::new(),
            queued_event_bytes: 0,
            limits,
            socket_path,
            trusted_ui: role == ClientRole::TrustedUi,
            unusable: false,
        })
    }

    /// Returns the bounds negotiated during the daemon handshake.
    #[must_use]
    pub const fn limits(&self) -> ServerLimits {
        self.limits
    }

    /// Retrieves one exact missing image body using the preferred negotiated source.
    pub async fn image_content_source(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        metadata: &ImageContentMetadata,
        cancellation: &CancellationToken,
    ) -> Result<ImageContentSource> {
        if !self.trusted_ui || (!self.limits.image.binary_chunks && !self.limits.image.sealed_memfd)
        {
            bail!("image content transport was not negotiated");
        }
        let mut accepted_transfers = Vec::with_capacity(2);
        if self.limits.image.sealed_memfd {
            accepted_transfers.push(ImageTransferMode::SealedMemfd);
        }
        if self.limits.image.binary_chunks {
            accepted_transfers.push(ImageTransferMode::BinaryChunks);
        }
        let request = ImageContentRequest {
            splint_id,
            incarnation,
            content_id: metadata.content_id,
            generation: metadata.generation,
            digest: metadata.digest,
            accepted_transfers,
        };
        let response = self
            .request_with_cancellation(
                Request::RequestImageContent {
                    request: request.clone(),
                },
                Duration::from_secs(5),
                cancellation,
            )
            .await?;
        let Response::ImageContentReady { transfer } = response else {
            bail!("splinterd returned an invalid image content response");
        };
        transfer
            .validate_for(&request, metadata)
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let control_socket = self
            .socket_path
            .as_deref()
            .context("image content socket path is unavailable")?;
        let mut stream = UnixStream::connect(image_content_socket_path(control_socket))
            .await
            .context("cannot connect to the image content socket")?;
        stream.write_all(&transfer.token).await?;
        let outcome = {
            let receive = async {
                match transfer.transfer {
                    ImageTransferMode::BinaryChunks => receive_image_content(&mut stream, metadata)
                        .await
                        .map(|bytes| ImageContentSource::Buffered(bytes.into())),
                    ImageTransferMode::SealedMemfd => receive_image_memfd(&mut stream, metadata)
                        .await
                        .map(|mapping| ImageContentSource::Mapped(Arc::new(mapping))),
                }
            };
            tokio::pin!(receive);
            let timeout =
                tokio::time::sleep(Duration::from_millis(u64::from(transfer.token_ttl_millis)));
            tokio::pin!(timeout);
            tokio::select! {
                result = &mut receive => Ok(Some(result)),
                () = cancellation.cancelled() => Ok(None),
                () = &mut timeout => Err(anyhow::anyhow!("image content transfer timed out")),
            }
        }?;
        if let Some(result) = outcome {
            if result.is_err() && transfer.transfer == ImageTransferMode::SealedMemfd {
                let _ = stream.write_all(&[2]).await;
            }
            result
        } else {
            if transfer.transfer == ImageTransferMode::BinaryChunks {
                let mut cancel = [0_u8; 9];
                cancel[0] = 2;
                let _ = stream.write_all(&cancel).await;
            } else {
                let _ = stream.write_all(&[2]).await;
            }
            bail!("image content transfer was cancelled")
        }
    }

    /// Retrieves one exact missing image body into a bounded owned buffer.
    pub async fn image_content(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        metadata: &ImageContentMetadata,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>> {
        Ok(self
            .image_content_source(splint_id, incarnation, metadata, cancellation)
            .await?
            .as_bytes()
            .to_vec())
    }

    /// Sends one request and waits for its correlated response.
    ///
    /// Dropping the returned future while the request is in flight sends at
    /// most one best-effort daemon cancellation and disposes the connection.
    pub async fn request(&mut self, request: Request) -> Result<Response> {
        let request_id = self.reserve_request_id()?;
        let mut in_flight = InFlightRequest {
            connection: self,
            request_id,
            armed: true,
        };
        let result = in_flight.perform(request).await;
        in_flight.disarm();
        result
    }

    /// Sends one request with a client-side deadline.
    ///
    /// On timeout, one best-effort `Cancel` frame is sent and the connection is
    /// closed. The connection is permanently unusable because a late response
    /// could otherwise make request correlation ambiguous.
    pub async fn request_with_deadline(
        &mut self,
        request: Request,
        deadline: Duration,
    ) -> Result<Response> {
        let cancellation = CancellationToken::new();
        self.request_with_cancellation(request, deadline, &cancellation)
            .await
    }

    /// Sends one request with a deadline and caller-owned cancellation token.
    ///
    /// Deadline expiry, token cancellation, or dropping this future after
    /// dispatch sends at most one best-effort daemon `Cancel`, closes the
    /// connection, and discards buffered late frames. A token already cancelled
    /// at entry or a zero deadline returns before reserving an ID or writing.
    /// Cancellation cannot roll back a daemon mutation that has already committed.
    pub async fn request_with_cancellation(
        &mut self,
        request: Request,
        deadline: Duration,
        cancellation: &CancellationToken,
    ) -> Result<Response> {
        if cancellation.is_cancelled() {
            return Err(anyhow::Error::new(RequestCancelled {
                reason: RequestCancellation::Cancelled,
            }));
        }
        if deadline.is_zero() {
            return Err(anyhow::Error::new(RequestCancelled {
                reason: RequestCancellation::DeadlineElapsed,
            }));
        }
        let request_id = self.reserve_request_id()?;
        let mut in_flight = InFlightRequest {
            connection: self,
            request_id,
            armed: true,
        };
        let result = tokio::select! {
            biased;
            result = in_flight.perform(request) => Some(result),
            () = cancellation.cancelled() => None,
            () = tokio::time::sleep(deadline) => {
                return Err(anyhow::Error::new(RequestCancelled {
                    reason: RequestCancellation::DeadlineElapsed,
                }));
            }
        };
        let Some(result) = result else {
            return Err(anyhow::Error::new(RequestCancelled {
                reason: RequestCancellation::Cancelled,
            }));
        };
        in_flight.disarm();
        result
    }

    fn reserve_request_id(&mut self) -> Result<u64> {
        self.ensure_usable()?;
        let request_id = self.next_request;
        self.next_request = self
            .next_request
            .checked_add(1)
            .context("splinterd request ID space exhausted")?;
        Ok(request_id)
    }

    async fn perform_request(&mut self, request_id: u64, request: Request) -> Result<Response> {
        if let Err(error) = self
            .write_client_frame(&ClientFrame::Request {
                request_id,
                request,
            })
            .await
        {
            self.mark_unusable();
            return Err(error);
        }
        loop {
            let frame = match self.read_server_frame().await {
                Ok(frame) => frame,
                Err(error) => {
                    self.mark_unusable();
                    return Err(error);
                }
            };
            match frame {
                ServerFrame::Response {
                    request_id: response_id,
                    result,
                } if response_id == request_id => return Ok(result),
                ServerFrame::Error {
                    request_id: Some(response_id),
                    error,
                } if response_id == request_id => {
                    return Err(anyhow::Error::new(DaemonProtocolFailure(error)));
                }
                event @ ServerFrame::Event { .. } => {
                    if let Err(error) = self.queue_event(event) {
                        self.mark_unusable();
                        return Err(error);
                    }
                }
                _ => {
                    self.mark_unusable();
                    bail!("splinterd sent a response with the wrong request id")
                }
            }
        }
    }

    /// Returns the next queued or newly received server frame.
    pub async fn next_server_frame(&mut self) -> Result<ServerFrame> {
        self.ensure_usable()?;
        if let Some((event, encoded_bytes)) = self.queued_events.pop_front() {
            self.queued_event_bytes = self.queued_event_bytes.saturating_sub(encoded_bytes);
            return Ok(event);
        }
        let result = self.read_server_frame().await;
        if result.is_err() {
            self.mark_unusable();
        }
        result
    }

    fn queue_event(&mut self, event: ServerFrame) -> Result<()> {
        let encoded_bytes = encode_frame(&event)?.len();
        let total_bytes = self
            .queued_event_bytes
            .checked_add(encoded_bytes)
            .context("splinterd queued-event byte count overflowed")?;
        if self.queued_events.len() >= MAX_QUEUED_EVENTS || total_bytes > MAX_QUEUED_EVENT_BYTES {
            bail!("splinterd sent too many events while a request was pending");
        }
        self.queued_events.push_back((event, encoded_bytes));
        self.queued_event_bytes = total_bytes;
        Ok(())
    }

    async fn read_server_frame(&mut self) -> Result<ServerFrame> {
        self.ensure_usable()?;
        loop {
            if self.read_buffer.len() >= 4 {
                let length = u32::from_be_bytes(
                    self.read_buffer[..4]
                        .try_into()
                        .expect("four-byte frame prefix"),
                ) as usize;
                if length == 0 || length > MAX_FRAME_BYTES {
                    bail!("splinterd sent an invalid frame length: {length} bytes");
                }
                let frame_length = length + 4;
                if self.read_buffer.len() >= frame_length {
                    let frame = serde_json::from_slice(&self.read_buffer[4..frame_length])
                        .context("splinterd sent invalid JSON")?;
                    self.read_buffer.drain(..frame_length);
                    return Ok(frame);
                }
            }
            let mut chunk = Box::new([0_u8; READ_CHUNK_BYTES]);
            let read = self
                .stream
                .as_mut()
                .context("splinterd connection is closed")?
                .read(chunk.as_mut_slice())
                .await?;
            if read == 0 {
                bail!("splinterd closed a partial frame");
            }
            self.read_buffer.extend_from_slice(&chunk[..read]);
            if self.read_buffer.len() > MAX_BUFFERED_BYTES {
                bail!("splinterd sent an invalid frame length: buffered data exceeds limit");
            }
        }
    }

    async fn write_client_frame(&mut self, frame: &ClientFrame) -> Result<()> {
        self.ensure_usable()?;
        write_frame(
            self.stream
                .as_mut()
                .context("splinterd connection is closed")?,
            frame,
        )
        .await
    }

    fn cancel_and_close(&mut self, request_id: u64) {
        self.unusable = true;
        self.queued_events.clear();
        self.queued_event_bytes = 0;
        self.read_buffer.clear();
        if let Some(stream) = self.stream.take()
            && let Ok(frame) = encode_frame(&ClientFrame::Cancel { request_id })
        {
            let _ = stream.try_write(&frame);
        }
    }

    fn ensure_usable(&self) -> Result<()> {
        if self.unusable || self.stream.is_none() {
            bail!("splinterd connection cannot be reused after cancellation or protocol failure");
        }
        Ok(())
    }

    fn mark_unusable(&mut self) {
        self.unusable = true;
        self.stream.take();
        self.queued_events.clear();
        self.queued_event_bytes = 0;
        self.read_buffer.clear();
    }

    /// Reads the current daemon topology revision.
    pub async fn topology_revision(&mut self) -> Result<TopologyRevision> {
        match self.request(Request::InspectTopology).await? {
            Response::Topology { snapshot } => Ok(snapshot.revision),
            _ => bail!("splinterd did not return topology"),
        }
    }

    /// Resolves the current live incarnation of a selected Splint.
    pub async fn live_incarnation(&mut self, splint_id: SplintId) -> Result<u64> {
        match self.request(Request::InspectSplint { splint_id }).await? {
            Response::Splint { runtime, .. } if runtime.splint_id == splint_id => runtime
                .live_incarnation
                .context("selected Splint does not have a live process"),
            _ => bail!("splinterd did not return the selected Splint identity"),
        }
    }

    /// Acquires a connection-owned controller lease.
    pub async fn acquire_control(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        modes: Vec<splinterm_protocol::ControlMode>,
    ) -> Result<u64> {
        match self
            .request(Request::AcquireControl {
                splint_id,
                incarnation,
                modes,
            })
            .await?
        {
            Response::ControlGranted { controller_id, .. } if controller_id != 0 => {
                Ok(controller_id)
            }
            _ => bail!("splinterd did not grant a controller lease"),
        }
    }

    /// Releases a controller lease owned by this connection.
    pub async fn release_control(&mut self, controller_id: u64) -> Result<()> {
        if matches!(
            self.request(Request::ReleaseControl { controller_id })
                .await?,
            Response::Acknowledged
        ) {
            Ok(())
        } else {
            bail!("splinterd did not release the controller lease")
        }
    }
}

/// Resolves the configured private daemon socket path.
pub fn socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SPLINTERM_SOCKET") {
        return Ok(path.into());
    }
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is unset; set SPLINTERM_SOCKET explicitly")?;
    Ok(runtime.join("splinterm/splinterd.sock"))
}

async fn write_frame(stream: &mut UnixStream, frame: &ClientFrame) -> Result<()> {
    stream.write_all(&encode_frame(frame)?).await?;
    Ok(())
}

async fn read_frame(stream: &mut UnixStream) -> Result<ServerFrame> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        bail!("splinterd sent an invalid frame length: {length} bytes");
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await.map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            anyhow::anyhow!("splinterd sent a truncated frame")
        } else {
            error.into()
        }
    })?;
    serde_json::from_slice(&body).context("splinterd sent invalid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_core::{Dojo, DojoId, Lair, Splint, SplintState, Window, WindowId};
    use splinterm_protocol::{
        CellAttributes, ColorSource, ProtocolError, SearchMatch, SubscriptionEvent, TerminalCell,
        TerminalRow, UnderlineStyle,
    };

    #[test]
    fn image_source_cache_deduplicates_exact_identity_and_tracks_bounds() {
        let pixels = vec![1_u8, 2, 3, 255];
        let metadata = ImageContentMetadata {
            content_id: 1,
            generation: 2,
            width: 1,
            height: 1,
            source_format: splinterm_protocol::ImageSourceFormat::Sixel,
            alpha_mode: splinterm_protocol::ImageAlphaMode::Opaque,
            digest: Sha256::digest(&pixels).into(),
            byte_length: pixels.len(),
            retention: splinterm_protocol::ImageRetention::WhilePlaced,
        };
        let mut cache = ImageContentCache::default();
        let first = cache.insert(&metadata, pixels.clone()).unwrap();
        let second = cache.insert(&metadata, pixels).unwrap();
        assert!(matches!(
            (&first, &second),
            (ImageContentSource::Buffered(first), ImageContentSource::Buffered(second))
                if Arc::ptr_eq(first, second)
        ));
        assert!(cache.contains(&metadata));
        assert_eq!(cache.get(&metadata).unwrap().as_bytes(), first.as_bytes());
        assert_eq!(
            cache.metrics(),
            ImageContentCacheMetrics {
                bytes: 4,
                entries: 1,
                high_water_bytes: 4,
                high_water_entries: 1,
            }
        );
    }

    #[test]
    fn image_source_cache_evicts_derived_entries_before_exceeding_its_limit() {
        let metadata = |content_id: u64, pixels: &[u8]| ImageContentMetadata {
            content_id,
            generation: 1,
            width: 1,
            height: 1,
            source_format: splinterm_protocol::ImageSourceFormat::Sixel,
            alpha_mode: splinterm_protocol::ImageAlphaMode::Opaque,
            digest: Sha256::digest(pixels).into(),
            byte_length: pixels.len(),
            retention: splinterm_protocol::ImageRetention::WhilePlaced,
        };
        let first_pixels = vec![1, 2, 3, 255];
        let second_pixels = vec![4, 5, 6, 255];
        let third_pixels = vec![7, 8, 9, 255];
        let first = metadata(1, &first_pixels);
        let second = metadata(2, &second_pixels);
        let third = metadata(3, &third_pixels);
        let mut cache = ImageContentCache::with_maximum_bytes(8).unwrap();
        cache.insert(&first, first_pixels).unwrap();
        cache.insert(&second, second_pixels).unwrap();
        cache.insert(&third, third_pixels).unwrap();
        assert!(!cache.contains(&first));
        assert!(cache.contains(&second));
        assert!(cache.contains(&third));
        assert_eq!(
            cache.metrics(),
            ImageContentCacheMetrics {
                bytes: 8,
                entries: 2,
                high_water_bytes: 8,
                high_water_entries: 2,
            }
        );
    }

    #[tokio::test]
    async fn sealed_memfd_receiver_validates_and_maps_immutable_content() {
        use rustix::{
            fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create},
            io::write,
            net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg},
        };

        let pixels = [1_u8, 2, 3, 255];
        let digest: [u8; 32] = Sha256::digest(pixels).into();
        let metadata = ImageContentMetadata {
            content_id: 1,
            generation: 2,
            width: 1,
            height: 1,
            source_format: splinterm_protocol::ImageSourceFormat::Sixel,
            alpha_mode: splinterm_protocol::ImageAlphaMode::Opaque,
            digest,
            byte_length: pixels.len(),
            retention: splinterm_protocol::ImageRetention::WhilePlaced,
        };
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let sender = tokio::spawn(async move {
            let fd = memfd_create(
                "splinterm-client-memfd-test",
                MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
            )
            .unwrap();
            write(&fd, &pixels).unwrap();
            fcntl_add_seals(
                &fd,
                SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
            )
            .unwrap();
            server.writable().await.unwrap();
            let descriptors = [fd.as_fd()];
            let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut ancillary = SendAncillaryBuffer::new(&mut space);
            assert!(ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)));
            assert_eq!(
                sendmsg(
                    server.as_fd(),
                    &[std::io::IoSlice::new(b"F")],
                    &mut ancillary,
                    SendFlags::empty(),
                )
                .unwrap(),
                1
            );
            let mut header = [0_u8; IMAGE_MEMFD_HEADER_BYTES];
            header[0..5].copy_from_slice(b"SPIF\x01");
            header[5..13].copy_from_slice(&4_u64.to_be_bytes());
            header[13..45].copy_from_slice(&digest);
            server.write_all(&header).await.unwrap();
            let mut acknowledgement = [0_u8; 1];
            server.read_exact(&mut acknowledgement).await.unwrap();
            assert_eq!(acknowledgement, [1]);
        });
        let mapping = receive_image_memfd(&mut client, &metadata).await.unwrap();
        assert_eq!(&*mapping, &pixels);
        sender.await.unwrap();
    }

    #[tokio::test]
    async fn binary_image_content_receiver_bounds_chunks_and_verifies_digest() {
        let pixels = [1_u8, 2, 3, 255];
        let digest: [u8; 32] = Sha256::digest(pixels).into();
        let metadata = ImageContentMetadata {
            content_id: 1,
            generation: 2,
            width: 1,
            height: 1,
            source_format: splinterm_protocol::ImageSourceFormat::Sixel,
            alpha_mode: splinterm_protocol::ImageAlphaMode::Opaque,
            digest,
            byte_length: pixels.len(),
            retention: splinterm_protocol::ImageRetention::WhilePlaced,
        };
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let sender = tokio::spawn(async move {
            let mut header = [0_u8; IMAGE_CONTENT_HEADER_BYTES];
            header[0..5].copy_from_slice(b"SPIM\x01");
            header[5..13].copy_from_slice(&4_u64.to_be_bytes());
            header[13..45].copy_from_slice(&digest);
            header[45..49]
                .copy_from_slice(&u32::try_from(MAX_IMAGE_CHUNK_BYTES).unwrap().to_be_bytes());
            header[49..53]
                .copy_from_slice(&u32::try_from(MAX_IMAGE_CHUNK_WINDOW).unwrap().to_be_bytes());
            server.write_all(&header).await.unwrap();
            server.write_all(&0_u64.to_be_bytes()).await.unwrap();
            server.write_all(&4_u32.to_be_bytes()).await.unwrap();
            server.write_all(&pixels).await.unwrap();
            let mut acknowledgement = [0_u8; 9];
            server.read_exact(&mut acknowledgement).await.unwrap();
            assert_eq!(acknowledgement[0], 1);
            assert_eq!(
                u64::from_be_bytes(acknowledgement[1..9].try_into().unwrap()),
                4
            );
        });
        assert_eq!(
            receive_image_content(&mut client, &metadata).await.unwrap(),
            pixels
        );
        sender.await.unwrap();
    }

    async fn read_client_frame(stream: &mut UnixStream) -> ClientFrame {
        let mut length = [0_u8; 4];
        stream.read_exact(&mut length).await.unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn established(stream: UnixStream) -> Connection {
        Connection {
            stream: Some(stream),
            next_request: 1,
            read_buffer: Vec::new(),
            queued_events: VecDeque::new(),
            queued_event_bytes: 0,
            limits: ServerLimits::default(),
            socket_path: None,
            trusted_ui: false,
            unusable: false,
        }
    }

    fn fixture_document(raw: &str) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(raw).unwrap()["document"].clone()
    }

    fn serialized(value: &impl Serialize) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }

    fn reviewed_row() -> TerminalRow {
        TerminalRow {
            row_id: Some(2),
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
        }
    }

    #[test]
    fn shared_terminal_projection_preserves_semantic_cells_and_bounds() {
        let attributes = reviewed_row().cells[0].attributes;
        let row = TerminalRow {
            row_id: Some(9),
            linebreak: true,
            cells: vec![
                TerminalCell {
                    content: "e\u{301}".to_owned(),
                    spacer_remaining: None,
                    attributes,
                },
                TerminalCell {
                    content: "界".to_owned(),
                    spacer_remaining: None,
                    attributes,
                },
                TerminalCell {
                    content: String::new(),
                    spacer_remaining: Some(1),
                    attributes,
                },
                TerminalCell {
                    content: "\u{fffd}".to_owned(),
                    spacer_remaining: None,
                    attributes,
                },
            ],
        };
        assert_eq!(
            project_terminal_rows(&[row]).unwrap(),
            [ProjectedTerminalRow {
                linebreak: true,
                cells: vec![
                    ProjectedTerminalCell {
                        text: "e\u{301}".to_owned(),
                        width: 1,
                    },
                    ProjectedTerminalCell {
                        text: "界".to_owned(),
                        width: 2,
                    },
                    ProjectedTerminalCell {
                        text: "\u{fffd}".to_owned(),
                        width: 1,
                    },
                ],
            }]
        );

        let mut oversized = reviewed_row();
        oversized.cells[0].content = "x".repeat(65);
        assert!(project_terminal_rows(&[oversized]).is_err());
    }

    fn reviewed_topology() -> TopologySnapshot {
        let dojo_id: DojoId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap();
        let window_id: WindowId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap();
        let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
        let mut splint = Splint::shell(PathBuf::from("/tmp"));
        splint.id = splint_id;
        splint.title = "build".to_owned();
        splint.last_incarnation = Some(2);
        splint.state = SplintState::Running;
        let window = Window {
            id: window_id,
            title: "terminal".to_owned(),
            default_focus: splint_id,
            root: LayoutNode::Leaf(splint),
        };
        let dojo = Dojo {
            id: dojo_id,
            name: "main".to_owned(),
            windows: vec![window],
        };
        let mut lair = Lair::new();
        lair.insert_dojo_at(TopologyRevision::new(0), dojo).unwrap();
        TopologySnapshot {
            revision: lair.revision(),
            lair,
            runtimes: vec![SplintRuntimeSummary {
                splint_id,
                live_incarnation: Some(2),
                last_incarnation: Some(2),
                restorable: false,
                lifecycle: SplintLifecycle::Running,
                exit_status: None,
            }],
        }
    }

    #[test]
    fn public_v1_read_dtos_match_golden_fixtures() {
        let snapshot = reviewed_topology();
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-list-dojos.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        expected["resource"]["topology_revision"] = serde_json::json!(1);
        assert_eq!(
            serialized(&list_dojos_envelope(&snapshot).unwrap()),
            expected
        );

        let empty = TopologySnapshot {
            revision: TopologyRevision::new(0),
            lair: Lair::new(),
            runtimes: Vec::new(),
        };
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-inspect-topology.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        expected["resource"]["topology_revision"] = serde_json::json!(0);
        assert_eq!(
            serialized(&inspect_topology_envelope(&empty).unwrap()),
            expected
        );

        let splint_id = snapshot.runtimes[0].splint_id;
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-inspect-splint.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        expected["resource"]["topology_revision"] = serde_json::json!(1);
        assert_eq!(
            serialized(&inspect_splint_envelope(&snapshot, splint_id).unwrap()),
            expected
        );
    }

    #[test]
    fn public_v1_history_dtos_match_golden_fixtures() {
        let dojo_id: DojoId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap();
        let window_id: WindowId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap();
        let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
        let page = ScrollbackPage {
            splint_id,
            incarnation: 2,
            terminal_revision: 9,
            history_generation: 3,
            oldest_available_row_id: Some(1),
            newest_available_row_id: Some(2),
            rows: vec![reviewed_row()],
            has_older: true,
        };
        let actual = serialized(&scrollback_page_envelope(dojo_id, window_id, &page).unwrap());
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-scrollback-page.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        expected["data"]["rows"][0]["row_id"] = serde_json::json!(2);
        expected["data"]["continuation_cursor"] = actual["data"]["continuation_cursor"].clone();
        assert_eq!(actual, expected);

        let search = SearchPage {
            splint_id,
            incarnation: 2,
            terminal_revision: 9,
            history_generation: 3,
            matches: vec![SearchMatch {
                row_id: 1,
                start_column: 0,
                end_column: 2,
                preview: "ok".to_owned(),
            }],
            next_cursor: None,
            timed_out: false,
        };
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-search-results.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(&search_page_envelope(dojo_id, window_id, &search).unwrap()),
            expected
        );

        for (operation, reason, fixture) in [
            (
                "scrollback_page",
                ReadResyncReasonV1::HistoryReplaced,
                include_str!("../../../tests/automation/fixtures/valid/cli-scrollback-resync.json"),
            ),
            (
                "search_scrollback",
                ReadResyncReasonV1::StaleRevision,
                include_str!("../../../tests/automation/fixtures/valid/cli-search-resync.json"),
            ),
        ] {
            let mut expected = fixture_document(fixture);
            expected["request_id"] = serde_json::json!("1");
            assert_eq!(
                serialized(
                    &read_resync_envelope(
                        operation,
                        TerminalReadProvenanceV1 {
                            dojo_id,
                            window_id,
                            splint_id,
                            incarnation: 2,
                            terminal_revision: 9,
                            history_generation: 3,
                        },
                        reason,
                    )
                    .unwrap()
                ),
                expected
            );
        }
    }

    #[test]
    fn public_v1_terminal_cursors_round_trip_and_reject_tampering() {
        let splint_id = SplintId::new();
        for cursor in [
            TerminalContinuationV1::Scrollback {
                splint_id,
                incarnation: 2,
                terminal_revision: 0,
                history_generation: 3,
                before_row_id: 8,
            },
            TerminalContinuationV1::Search {
                splint_id,
                incarnation: 2,
                terminal_revision: 9,
                history_generation: 3,
                daemon_cursor: "next".to_owned(),
            },
        ] {
            let encoded = encode_terminal_cursor(&cursor).unwrap();
            assert!((16..=256).contains(&encoded.len()));
            assert_eq!(decode_terminal_cursor(&encoded).unwrap(), cursor);
            assert!(decode_terminal_cursor(&format!("{encoded}A")).is_err());
        }
    }

    #[test]
    fn public_v1_terminal_action_dtos_match_golden_fixtures() {
        let provenance = TerminalReadProvenanceV1 {
            dojo_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
            window_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
            splint_id: "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap(),
            incarnation: 2,
            terminal_revision: 9,
            history_generation: 3,
        };
        for (operation, dimensions, fixture) in [
            (
                "input",
                None,
                include_str!("../../../tests/automation/fixtures/valid/cli-input.json"),
            ),
            (
                "resize",
                Some((120, 40)),
                include_str!("../../../tests/automation/fixtures/valid/cli-resize.json"),
            ),
        ] {
            let mut expected = fixture_document(fixture);
            expected["request_id"] = serde_json::json!("1");
            assert_eq!(
                serialized(&terminal_action_envelope(operation, provenance, dimensions).unwrap()),
                expected
            );
        }
    }

    fn mutation_ids() -> (DojoId, WindowId, SplintId) {
        (
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap(),
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap(),
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap(),
        )
    }

    fn mutation_identity() -> MutationIdentityV1 {
        let (dojo_id, window_id, splint_id) = mutation_ids();
        MutationIdentityV1 {
            dojo_id: Some(dojo_id),
            window_id: Some(window_id),
            splint_id: Some(splint_id),
            topology_revision: Some(7),
            incarnation: Some(2),
        }
    }

    #[test]
    fn public_v1_mutation_dtos_match_golden_fixtures() {
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-create-dojo.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(&created_mutation_envelope("create_dojo", mutation_identity()).unwrap()),
            expected
        );

        let mut identity = mutation_identity();
        identity.incarnation = None;
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-layout-mutation.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(&committed_mutation_envelope("set_split_ratio", identity, false).unwrap()),
            expected
        );

        identity.splint_id = None;
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-confirmed-close.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(&committed_mutation_envelope("close_window", identity, true).unwrap()),
            expected
        );

        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-relaunch-splint.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(&process_started_envelope("relaunch_splint", mutation_identity()).unwrap()),
            expected
        );
    }

    #[test]
    fn public_v1_restore_kill_and_revoke_dtos_match_golden_fixtures() {
        let (dojo_id, window_id, splint_id) = mutation_ids();
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-restore-dojo.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(
                &restore_many_envelope(
                    "restore_dojo",
                    MutationIdentityV1 {
                        dojo_id: Some(dojo_id),
                        window_id: None,
                        splint_id: None,
                        topology_revision: Some(7),
                        incarnation: None,
                    },
                    &[RestoreLeafResult {
                        splint_id,
                        incarnation: Some(2),
                        error: None,
                    }],
                )
                .unwrap()
            ),
            expected
        );

        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-kill.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(&kill_envelope(dojo_id, window_id, splint_id, 2).unwrap()),
            expected
        );

        let grant = AccessGrant {
            grant_id: 42,
            splint_id,
            incarnation: 2,
            scopes: vec![AccessScope::Observe],
            requester: "/usr/bin/editor".to_owned(),
            expires_at_unix_seconds: 1,
        };
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-revoke-access.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(&revoke_envelope(dojo_id, window_id, &grant).unwrap()),
            expected
        );
    }

    #[test]
    fn public_v1_authorization_and_audit_dtos_match_golden_fixtures() {
        let dojo_id: DojoId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77101".parse().unwrap();
        let window_id: WindowId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102".parse().unwrap();
        let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103".parse().unwrap();
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-authorization-status.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(
            serialized(
                &authorization_status_envelope(
                    dojo_id,
                    window_id,
                    splint_id,
                    2,
                    &[],
                    &[PersistentAuthorizationStatus {
                        policy_rule_id: "editor".to_owned(),
                        scopes: vec![AutomationScope::TerminalVisibleRead],
                        expires_at_unix_seconds: None,
                    }],
                    false,
                )
                .unwrap()
            ),
            expected
        );

        let page = AuditPage {
            records: vec![splinterm_protocol::AuditRecord {
                schema: "splinterm.audit.v1".to_owned(),
                retention: "daemon_lifetime".to_owned(),
                audit_id: 73,
                unix_seconds: 1_721_760_000,
                policy_generation: None,
                policy_rule_id: None,
                peer: splinterm_protocol::AuditPeer {
                    uid: 1_000,
                    executable_path: "/usr/bin/editor".to_owned(),
                    executable_sha256: "a".repeat(64),
                    device: None,
                    inode: None,
                },
                operation: AuditOperation::Input,
                resource: Some(splinterm_protocol::AuditResource {
                    dojo_id: None,
                    window_id: None,
                    splint_id: Some(splint_id),
                    incarnation: Some(2),
                }),
                requested_scopes: vec![AutomationScope::Input],
                decision: AuditDecision::Allowed,
                reason: "policy_match".to_owned(),
                outcome: Some(AuditOutcome::Succeeded),
                argument_count: None,
                executable_basename: None,
            }],
            retention_gap: false,
            oldest_available_audit_id: Some(1),
            newest_available_audit_id: Some(73),
            next_after_audit_id: None,
        };
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/cli-audit-inspect.json"
        ));
        expected["request_id"] = serde_json::json!("1");
        assert_eq!(serialized(&audit_page_envelope(&page).unwrap()), expected);
    }

    #[test]
    fn public_v1_protocol_failures_preserve_topology_revision() {
        let error = splinterm_protocol::ProtocolError {
            code: ErrorCode::StaleTopology,
            message: "topology revision is stale".to_owned(),
            current_topology_revision: Some(TopologyRevision::new(9)),
        };
        let document = serialized(
            &CliEnvelopeV1::protocol_failure(
                "set_split_ratio",
                &error,
                "splinterd [staletopology]: topology revision is stale",
            )
            .unwrap(),
        );
        assert_eq!(document["error"]["code"], "stale_topology");
        assert_eq!(document["error"]["current_topology_revision"], 9);
        assert_eq!(document["truncated"], false);
    }

    #[test]
    fn public_v1_ping_dtos_match_golden_fixtures() {
        let success = PingEnvelopeV1::success(1).unwrap();
        assert_eq!(
            serialized(&success),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/cli-ping-success.json"
            ))
        );
        let failure =
            PingEnvelopeV1::failure(1, CliErrorCodeV1::Timeout, "request deadline elapsed", true)
                .unwrap();
        assert_eq!(
            serialized(&failure),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/cli-ping-timeout.json"
            ))
        );
        assert!(PingEnvelopeV1::success(0).is_err());
        assert!(PingEnvelopeV1::failure(1, CliErrorCodeV1::Internal, "", false).is_err());
    }

    #[test]
    fn public_v1_initial_event_dtos_match_golden_fixtures() {
        let splint_id = "018f4d8c-2a18-4b31-8c2f-9e7c5de77110";
        let terminal = CliEventV1(CliEventDataV1::TerminalSnapshot {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id: "1".to_owned(),
            sequence: 1,
            event_type: "snapshot",
            resource: TerminalResourceV1 {
                splint_id: splint_id.to_owned(),
                incarnation: 3,
                terminal_revision: 42,
                history_generation: None,
            },
            data: TerminalSnapshotV1 {
                content_encoding: "unicode_scalars",
                columns: 2,
                rows: 1,
                title: "build".to_owned(),
                visible_rows: vec![ProjectedTerminalRow {
                    linebreak: false,
                    cells: vec![
                        ProjectedTerminalCell {
                            text: "A".to_owned(),
                            width: 1,
                        },
                        ProjectedTerminalCell {
                            text: "界".to_owned(),
                            width: 2,
                        },
                    ],
                }],
            },
            truncated: false,
        });
        assert_eq!(
            serialized(&terminal),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-terminal-snapshot.json"
            ))
        );

        let topology = CliEventV1(CliEventDataV1::TopologySnapshot {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id: "1".to_owned(),
            sequence: 1,
            event_type: "topology_snapshot",
            resource: TopologyResourceV1 {
                topology_revision: 7,
            },
            data: TopologySnapshotV1 {
                dojo_count: 1,
                window_count: 2,
                splint_count: 3,
            },
            truncated: false,
        });
        assert_eq!(
            serialized(&topology),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-topology-snapshot.json"
            ))
        );

        let control = CliEventV1(CliEventDataV1::ControlSnapshot {
            schema: CLI_EVENT_SCHEMA_V1,
            subscription_id: "1".to_owned(),
            sequence: 1,
            event_type: "control_snapshot",
            resource: ControlResourceV1 {
                splint_id: splint_id.to_owned(),
                incarnation: 3,
            },
            data: ControlSnapshotV1 {
                controlled: true,
                locally_owned: false,
            },
            truncated: false,
        });
        assert_eq!(
            serialized(&control),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-control-snapshot.json"
            ))
        );
    }

    #[test]
    fn public_v1_update_event_dtos_match_closed_fixtures() {
        let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77110".parse().unwrap();
        assert_eq!(
            serialized(&CliEventV1::terminal_update(1, 2, splint_id, 3, 43, 2).unwrap()),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-terminal-update.json"
            ))
        );
        assert_eq!(
            serialized(
                &CliEventV1::control_transfer_resolved(
                    1,
                    2,
                    splint_id,
                    3,
                    9,
                    ControlTransferOutcome::Granted,
                )
                .unwrap()
            ),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-control-transfer-resolved.json"
            ))
        );
        let snapshot = reviewed_topology();
        let actual = serialized(
            &CliEventV1::topology_changed(1, 2, TopologyChangeKind::SplintSplit, &snapshot)
                .unwrap(),
        );
        let mut expected = fixture_document(include_str!(
            "../../../tests/automation/fixtures/valid/subscription-topology-changed.json"
        ));
        expected["resource"]["topology_revision"] = serde_json::json!(1);
        expected["data"]["window_count"] = serde_json::json!(1);
        expected["data"]["splint_count"] = serde_json::json!(1);
        assert_eq!(actual, expected);
    }

    #[test]
    fn public_v1_resync_dtos_match_golden_fixtures() {
        let splint_id: SplintId = "018f4d8c-2a18-4b31-8c2f-9e7c5de77110".parse().unwrap();
        let terminal = CliEventV1::terminal_resync(
            1,
            4,
            splint_id,
            3,
            45,
            Some(2),
            ResyncReasonV1::HistoryReplaced,
        )
        .unwrap();
        assert_eq!(
            serialized(&terminal),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-terminal-resync.json"
            ))
        );
        let topology = CliEventV1::topology_resync(
            1,
            4,
            TopologyRevision::new(9),
            ResyncReasonV1::RevisionGap,
        )
        .unwrap();
        assert_eq!(
            serialized(&topology),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-topology-resync.json"
            ))
        );
        let control =
            CliEventV1::control_resync(1, 4, splint_id, 3, ResyncReasonV1::SubscriberStalled)
                .unwrap();
        assert_eq!(
            serialized(&control),
            fixture_document(include_str!(
                "../../../tests/automation/fixtures/valid/subscription-control-resync.json"
            ))
        );
        assert!(
            CliEventV1::control_resync(1, 4, splint_id, 3, ResyncReasonV1::HistoryReplaced)
                .is_err()
        );
    }

    #[tokio::test]
    async fn negotiation_stores_limits_and_rejects_error() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let expected_limits = ServerLimits {
            maximum_input_bytes: 17,
            ..ServerLimits::default()
        };
        let server_task = tokio::spawn(async move {
            assert!(matches!(
                read_client_frame(&mut server).await,
                ClientFrame::Hello { .. }
            ));
            let frame = ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                limits: expected_limits,
                development_terminal_access: false,
            };
            server
                .write_all(&encode_frame(&frame).unwrap())
                .await
                .unwrap();
        });
        let connection = Connection::connect_stream(client, ClientRole::Automation)
            .await
            .unwrap();
        assert_eq!(connection.limits(), expected_limits);
        server_task.await.unwrap();

        let (client, mut server) = UnixStream::pair().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = read_client_frame(&mut server).await;
            let frame = ServerFrame::Error {
                request_id: None,
                error: ProtocolError::new(ErrorCode::IncompatibleVersion, "wrong version"),
            };
            server
                .write_all(&encode_frame(&frame).unwrap())
                .await
                .unwrap();
        });
        let error = Connection::connect_stream(client, ClientRole::Automation)
            .await
            .unwrap_err();
        assert_eq!(
            protocol_error(&error).map(|error| error.code),
            Some(ErrorCode::IncompatibleVersion)
        );
        server_task.await.unwrap();

        let (client, mut server) = UnixStream::pair().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = read_client_frame(&mut server).await;
            let frame = ServerFrame::Hello {
                version: PROTOCOL_VERSION - 1,
                limits: ServerLimits::default(),
                development_terminal_access: false,
            };
            server
                .write_all(&encode_frame(&frame).unwrap())
                .await
                .unwrap();
        });
        let error = Connection::connect_stream(client, ClientRole::Automation)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid handshake"));
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn buffered_reader_rejects_oversized_truncated_and_eof_frames() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        server
            .write_all(&u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_be_bytes())
            .await
            .unwrap();
        assert!(
            connection
                .next_server_frame()
                .await
                .unwrap_err()
                .to_string()
                .contains("invalid frame length")
        );

        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        server.write_all(&10_u32.to_be_bytes()).await.unwrap();
        server.write_all(b"{}").await.unwrap();
        drop(server);
        assert!(
            connection
                .next_server_frame()
                .await
                .unwrap_err()
                .to_string()
                .contains("partial frame")
        );

        let (client, server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        drop(server);
        assert!(
            connection
                .next_server_frame()
                .await
                .unwrap_err()
                .to_string()
                .contains("partial frame")
        );
    }

    #[tokio::test]
    async fn request_queues_events_and_rejects_mismatched_response_ids() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        let server_task = tokio::spawn(async move {
            assert!(matches!(
                read_client_frame(&mut server).await,
                ClientFrame::Request { request_id: 1, .. }
            ));
            let event = ServerFrame::Event {
                subscription_id: 4,
                sequence: 1,
                event: SubscriptionEvent::AccessRevoked { grant_id: 9 },
            };
            server
                .write_all(&encode_frame(&event).unwrap())
                .await
                .unwrap();
            server
                .write_all(
                    &encode_frame(&ServerFrame::Response {
                        request_id: 1,
                        result: Response::Pong,
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
        });
        assert_eq!(
            connection.request(Request::Ping).await.unwrap(),
            Response::Pong
        );
        assert!(matches!(
            connection.next_server_frame().await.unwrap(),
            ServerFrame::Event {
                subscription_id: 4,
                ..
            }
        ));
        server_task.await.unwrap();

        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        let server_task = tokio::spawn(async move {
            let _ = read_client_frame(&mut server).await;
            server
                .write_all(
                    &encode_frame(&ServerFrame::Response {
                        request_id: 2,
                        result: Response::Pong,
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
        });
        assert!(
            connection
                .request(Request::Ping)
                .await
                .unwrap_err()
                .to_string()
                .contains("wrong request id")
        );
        assert!(connection.request(Request::Ping).await.is_err());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn request_rejects_unbounded_event_queue() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        let server_task = tokio::spawn(async move {
            let _ = read_client_frame(&mut server).await;
            for sequence in 1..=MAX_QUEUED_EVENTS + 1 {
                let event = ServerFrame::Event {
                    subscription_id: 4,
                    sequence: u64::try_from(sequence).unwrap(),
                    event: SubscriptionEvent::AccessRevoked { grant_id: 9 },
                };
                server
                    .write_all(&encode_frame(&event).unwrap())
                    .await
                    .unwrap();
            }
        });
        let error = connection.request(Request::Ping).await.unwrap_err();
        assert!(error.to_string().contains("too many events"));
        assert!(connection.request(Request::Ping).await.is_err());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn pre_cancelled_and_zero_deadline_requests_emit_no_frame() {
        for (cancellation, deadline, expected) in [
            {
                let cancellation = CancellationToken::new();
                cancellation.cancel();
                (
                    cancellation,
                    Duration::from_secs(1),
                    RequestCancellation::Cancelled,
                )
            },
            (
                CancellationToken::new(),
                Duration::ZERO,
                RequestCancellation::DeadlineElapsed,
            ),
        ] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let mut connection = established(client);
            let error = connection
                .request_with_cancellation(Request::Ping, deadline, &cancellation)
                .await
                .unwrap_err();
            assert_eq!(request_cancellation(&error), Some(expected));
            let mut byte = [0_u8; 1];
            assert!(
                tokio::time::timeout(Duration::from_millis(20), server.read(&mut byte))
                    .await
                    .is_err(),
                "preflight rejection emitted a frame or closed the connection"
            );
            assert_eq!(connection.next_request, 1);
            assert!(connection.stream.is_some());
        }
    }

    #[tokio::test]
    async fn cancellation_token_sends_one_cancel_and_discards_queued_frames() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        let cancellation = CancellationToken::new();
        let cancel_request = cancellation.clone();
        let server_task = tokio::spawn(async move {
            assert!(matches!(
                read_client_frame(&mut server).await,
                ClientFrame::Request { request_id: 1, .. }
            ));
            let event = ServerFrame::Event {
                subscription_id: 7,
                sequence: 1,
                event: SubscriptionEvent::AccessRevoked { grant_id: 9 },
            };
            server
                .write_all(&encode_frame(&event).unwrap())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_request.cancel();
            assert_eq!(
                read_client_frame(&mut server).await,
                ClientFrame::Cancel { request_id: 1 }
            );
            let mut byte = [0_u8; 1];
            assert_eq!(server.read(&mut byte).await.unwrap(), 0);
        });
        let error = connection
            .request_with_cancellation(Request::Ping, Duration::from_secs(1), &cancellation)
            .await
            .unwrap_err();
        assert_eq!(
            request_cancellation(&error),
            Some(RequestCancellation::Cancelled)
        );
        assert!(connection.queued_events.is_empty());
        assert!(connection.read_buffer.is_empty());
        assert!(connection.stream.is_none());
        assert!(connection.request(Request::Ping).await.is_err());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn dropped_request_future_closes_connection_owned_subscription() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        let topology = reviewed_topology();
        let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            assert!(matches!(
                read_client_frame(&mut server).await,
                ClientFrame::Request {
                    request_id: 1,
                    request: Request::SubscribeTopology,
                }
            ));
            server
                .write_all(
                    &encode_frame(&ServerFrame::Response {
                        request_id: 1,
                        result: Response::TopologySubscribed {
                            subscription_id: 7,
                            snapshot: topology,
                        },
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            assert!(matches!(
                read_client_frame(&mut server).await,
                ClientFrame::Request { request_id: 2, .. }
            ));
            request_seen_tx.send(()).unwrap();
            assert_eq!(
                read_client_frame(&mut server).await,
                ClientFrame::Cancel { request_id: 2 }
            );
            // A response racing cancellation is never observed because the
            // client has already disposed the complete connection.
            let _ = server
                .write_all(
                    &encode_frame(&ServerFrame::Response {
                        request_id: 2,
                        result: Response::Pong,
                    })
                    .unwrap(),
                )
                .await;
            let mut byte = [0_u8; 1];
            assert_eq!(server.read(&mut byte).await.unwrap(), 0);
        });
        assert!(matches!(
            connection
                .request(Request::SubscribeTopology)
                .await
                .unwrap(),
            Response::TopologySubscribed {
                subscription_id: 7,
                ..
            }
        ));

        let mut request = Box::pin(connection.request(Request::Ping));
        tokio::select! {
            result = &mut request => panic!("request unexpectedly completed: {result:?}"),
            result = request_seen_rx => result.unwrap(),
        }
        drop(request);

        assert!(connection.stream.is_none());
        assert!(connection.queued_events.is_empty());
        assert!(connection.read_buffer.is_empty());
        assert!(connection.request(Request::Ping).await.is_err());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn timeout_sends_cancel_closes_and_prevents_reuse() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut connection = established(client);
        let server_task = tokio::spawn(async move {
            assert!(matches!(
                read_client_frame(&mut server).await,
                ClientFrame::Request { request_id: 1, .. }
            ));
            assert_eq!(
                read_client_frame(&mut server).await,
                ClientFrame::Cancel { request_id: 1 }
            );
            let mut byte = [0_u8; 1];
            assert_eq!(server.read(&mut byte).await.unwrap(), 0);
        });
        let error = connection
            .request_with_deadline(Request::Ping, Duration::from_millis(10))
            .await
            .unwrap_err();
        assert_eq!(
            request_cancellation(&error),
            Some(RequestCancellation::DeadlineElapsed)
        );
        assert!(error.to_string().contains("timed out"));
        assert!(
            connection
                .request(Request::Ping)
                .await
                .unwrap_err()
                .to_string()
                .contains("cannot be reused")
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn buffered_read_resumes_after_outer_cancellation() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let expected = ServerFrame::Hello {
            version: PROTOCOL_VERSION,
            limits: ServerLimits::default(),
            development_terminal_access: true,
        };
        let encoded = encode_frame(&expected).unwrap();
        let (prefix_sent, prefix_received) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            server.write_all(&encoded[..4]).await.unwrap();
            prefix_sent.send(()).unwrap();
            resume_rx.await.unwrap();
            server.write_all(&encoded[4..]).await.unwrap();
        });
        prefix_received.await.unwrap();
        let mut connection = established(client);

        assert!(
            tokio::time::timeout(Duration::from_millis(10), connection.next_server_frame())
                .await
                .is_err()
        );
        assert_eq!(connection.read_buffer.len(), 4);
        resume_tx.send(()).unwrap();
        assert_eq!(connection.next_server_frame().await.unwrap(), expected);
        assert!(connection.read_buffer.is_empty());
        writer.await.unwrap();
    }
}
