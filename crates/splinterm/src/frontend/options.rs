//! Platform-independent construction options for graphical windows and panes.

use std::{path::PathBuf, sync::mpsc::Sender as StdSender};

use tokio::sync::{
    mpsc::{Receiver, Sender},
    watch::Sender as WatchSender,
};

use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::{LayoutNode, SplintId};
use splinterm_protocol::TerminalSnapshot;

use crate::{
    config::{CursorStyle, FrameTitleMode, PaneDividerStyle, ResolvedTheme},
    keymap::ResolvedKeymap,
};

use super::{
    AuthorityStatus, SessionPickerUi, WindowCommand, WindowDojoIdentity, WindowTopologyCommand,
    WindowTopologyUpdate, WindowUpdate,
};

pub struct TrustedConsentUi {
    pub decision: StdSender<bool>,
}

pub struct WindowPaneOptions {
    pub snapshot: TerminalSnapshot,
    pub updates: Receiver<WindowUpdate>,
    pub commands: Sender<WindowCommand>,
    pub authority: AuthorityStatus,
    pub controlled: bool,
    pub image_sources: ImageContentLeaseSet,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent renderer, input, capture, and endpoint capability flags"
)]
pub struct WindowOptions {
    pub capture: Option<PathBuf>,
    /// Initial owned daemon snapshot. `None` retains the deterministic evidence row.
    pub snapshot: Option<TerminalSnapshot>,
    /// Exact renderer leases paired with the legacy initial snapshot.
    pub image_sources: ImageContentLeaseSet,
    /// Bounded live-update receiver owned by the Wayland thread.
    pub updates: Option<Receiver<WindowUpdate>>,
    /// Bounded command sender from the Wayland thread to the async protocol owner.
    pub commands: Option<Sender<WindowCommand>>,
    /// Retain Q/Escape close shortcuts only for the renderer evidence example.
    pub evidence_close_shortcuts: bool,
    /// Delay capture until this integer output scale is active.
    pub capture_scale: Option<u32>,
    /// Trusted application-owned consent mode. Terminal content cannot enable it.
    pub trusted_consent: Option<TrustedConsentUi>,
    /// Application-owned recent-session picker. Terminal content cannot enable it.
    pub session_picker: Option<SessionPickerUi>,
    /// Trusted authority state rendered in persistent application chrome.
    pub authority: AuthorityStatus,
    /// Whether the legacy single-pane command channel already owns control.
    pub controlled: bool,
    /// Initial terminal dimensions from the supported configuration subset.
    pub initial_columns: u16,
    pub initial_rows: u16,
    /// Configured cursor presentation policy.
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    /// Optional fixed user title; terminal OSC titles remain active when absent.
    pub title: Option<String>,
    /// Current project-owned Omarchy role mapping.
    pub theme: ResolvedTheme,
    pub pane_divider_style: PaneDividerStyle,
    pub frame_title_mode: FrameTitleMode,
    /// Fully validated client-local keymap for this Window.
    pub keymap: ResolvedKeymap,
    /// Multi-pane input. Empty retains the legacy one-pane fields above.
    pub panes: Vec<WindowPaneOptions>,
    pub layout: Option<LayoutNode>,
    /// Client-local initial focus; never written back implicitly.
    pub active_splint: Option<SplintId>,
    pub topology_updates: Option<Receiver<WindowTopologyUpdate>>,
    pub topology_commands: Option<Sender<WindowTopologyCommand>>,
    /// Coalesced ephemeral keyboard/pane focus publication for the supported adapter API.
    pub graphical_focus: Option<WatchSender<Option<SplintId>>>,
    /// Whether trusted graphical force-transfer actions may be offered or dispatched.
    pub forced_control_transfer: bool,
    /// Render a client-local placeholder before dispatching a remote split.
    pub optimistic_remote_splits: bool,
    /// Stable identity for the initial managed Dojo; absent for legacy/evidence windows.
    pub initial_dojo: Option<WindowDojoIdentity>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            capture: None,
            snapshot: None,
            image_sources: ImageContentLeaseSet::default(),
            updates: None,
            commands: None,
            evidence_close_shortcuts: false,
            capture_scale: None,
            trusted_consent: None,
            session_picker: None,
            authority: AuthorityStatus::default(),
            controlled: true,
            initial_columns: 80,
            initial_rows: 24,
            cursor_style: CursorStyle::Block,
            cursor_blink: true,
            title: None,
            theme: ResolvedTheme::default(),
            pane_divider_style: PaneDividerStyle::Line,
            frame_title_mode: FrameTitleMode::Splint,
            keymap: ResolvedKeymap::default(),
            panes: Vec::new(),
            layout: None,
            active_splint: None,
            topology_updates: None,
            topology_commands: None,
            graphical_focus: None,
            forced_control_transfer: true,
            optimistic_remote_splits: false,
            initial_dojo: None,
        }
    }
}
