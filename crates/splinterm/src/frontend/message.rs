//! Bounded messages exchanged by the application owner and graphical adapter.

use std::sync::Arc;

use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::SplintId;
use splinterm_protocol::{
    ControlTransferDecision, ControlTransferOutcome, SearchPage, TerminalSnapshot, TerminalUpdate,
};

use crate::{config::ResolvedTheme, renderer::FontGeneration};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthorityStatus {
    pub grants: Vec<(u64, String)>,
    pub development_bypass: bool,
}

/// Body-free identity that correlates one protocol update with graphical work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerfTraceCorrelation {
    pub base_revision: u64,
    pub revision: u64,
    pub subscription_id: u64,
    pub transaction_sequence: u64,
}

/// Bounded protocol-to-Wayland messages for the live snapshot viewer.
#[allow(
    clippy::large_enum_variant,
    reason = "the queue is bounded and owned snapshots avoid a second allocation"
)]
#[derive(Debug)]
pub enum WindowUpdate {
    Snapshot {
        snapshot: TerminalSnapshot,
        image_sources: ImageContentLeaseSet,
        authoritative: bool,
    },
    Update {
        update: TerminalUpdate,
        image_sources: Option<ImageContentLeaseSet>,
        trace: Option<PerfTraceCorrelation>,
    },
    ScrollbackPages(Vec<splinterm_protocol::ScrollbackPage>),
    ScrollbackResyncRequired,
    Authority(AuthorityStatus),
    Control(bool),
    ControlTransferRequested(u64),
    ControlTransferResolved(ControlTransferOutcome),
    SearchResults(SearchPage),
    SearchResyncRequired,
    Theme(ThemeUpdate),
    Font(FontUpdate),
    Exited {
        splint_id: SplintId,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThemeUpdate {
    pub generation: u64,
    pub theme: ResolvedTheme,
}

#[derive(Clone, Debug)]
pub struct FontUpdate {
    pub generation: Arc<FontGeneration>,
}

/// Bounded Wayland-to-protocol commands for the first interactive slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowCommand {
    Input(Vec<u8>),
    Resynchronize,
    Resize {
        columns: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    PrepareResize {
        columns: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    FetchScrollback {
        splint_id: SplintId,
        incarnation: u64,
        terminal_revision: u64,
        history_generation: u64,
        before_row_id: u64,
    },
    RevokeAccess(u64),
    RequestControlTransfer,
    DecideControlTransfer {
        transfer_id: u64,
        decision: ControlTransferDecision,
    },
    ForceControlTransfer,
    Search {
        terminal_revision: u64,
        history_generation: u64,
        query: String,
        case_sensitive: bool,
        cursor: Option<String>,
    },
    ReleaseControl,
}
