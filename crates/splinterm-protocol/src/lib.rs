//! Versioned, transport-independent messages exchanged over the local socket.
//!
//! Terminal DTOs in this crate are intentionally distinct from the borrowed
//! `splinterm-terminal` and daemon runtime representations.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use splinterm_core::{Dojo, SplintId};

pub const PROTOCOL_VERSION: u16 = 11;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SNAPSHOT_SCROLLBACK_ROWS: usize = 16;
pub const MAX_SCROLLBACK_PAGE_ROWS: usize = 16;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_COLUMNS: u16 = 240;
pub const MAX_ROWS: u16 = 80;
pub const MAX_OUTSTANDING_REQUESTS: usize = 1;
pub const MAX_SUBSCRIPTIONS: usize = 4;
pub const MAX_UPDATE_ROW_PATCHES: usize = MAX_ROWS as usize;
pub const MAX_UPDATE_SCROLLS: usize = MAX_ROWS as usize;
pub const MAX_CONSENT_FRAME_BYTES: usize = 16 * 1024;
pub const CONSENT_CAPABILITY_BYTES: usize = 32;
pub const MAX_ACCESS_SCOPES: usize = 7;

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
    InspectLiveSplint,
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
        name: String,
        cwd: PathBuf,
        /// Direct executable plus argv. Empty selects the configured/default shell.
        command: Vec<String>,
        shell: Option<String>,
        login_shell: bool,
        scrollback_lines: usize,
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
    AcquireControl {
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
    Terminate {
        splint_id: SplintId,
        incarnation: u64,
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
    LiveSplint {
        splint_id: SplintId,
        incarnation: u64,
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
    ControlGranted {
        controller_id: u64,
    },
    Acknowledged,
    Terminated {
        code: Option<i32>,
        signal: Option<i32>,
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
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

impl ProtocolError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
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
    ConsentUnavailable,
    ConsentDenied,
    Unauthorized,
    ControllerUnavailable,
    NotFound,
    StaleIncarnation,
    InvalidArgument,
    ResourceLimit,
    Cancelled,
    RequestNotFound,
    Internal,
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
    /// Validates revision continuity and every collection/index bound against
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
        if self.base_revision != current_revision
            || self.revision != current_revision.saturating_add(1)
        {
            return Err(ProtocolError::new(
                ErrorCode::InvalidArgument,
                "terminal update revision is not contiguous",
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
        for patch in &self.rows {
            if patch.index >= rows
                || patch.row.row_id.is_some()
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
            .any(|row| row.row_id.is_some() || row.cells.len() > self.columns)
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

    fn update() -> TerminalUpdate {
        TerminalUpdate {
            base_revision: 4,
            revision: 5,
            rows: vec![TerminalRowPatch {
                index: 1,
                row: TerminalRow {
                    row_id: None,
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
                row_id: None,
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

        let mut invalid = update();
        invalid.revision = 6;
        assert!(invalid.validate_against(4, 1, 80, 24).is_err());

        let mut invalid = update();
        invalid.rows[0].index = 24;
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
