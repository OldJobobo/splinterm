//! Native Wayland xdg-shell and shared-memory lifecycle for the graphical client.
//!
//! Foot 1.27.0 `wayland.c`, `shm.c`, and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` are the behavioral reference.
//! The client owns these objects; the daemon remains headless.

use std::{
    collections::{HashMap, HashSet},
    io,
    os::fd::{AsFd, OwnedFd},
    path::PathBuf,
    pin::Pin,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self as std_mpsc, Receiver as StdReceiver, Sender as StdSender},
    },
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use tokio::sync::mpsc::{Receiver, Sender, error::TrySendError};
use unicode_width::UnicodeWidthChar;

use anyhow::{Context, Result};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    data_device_manager::{
        DataDeviceManagerState, WritePipe,
        data_device::{DataDevice, DataDeviceHandler},
        data_offer::{DataOfferHandler, DragOffer, SelectionOffer},
        data_source::{CopyPasteSource, DataSourceHandler},
    },
    delegate_compositor, delegate_data_device, delegate_keyboard, delegate_output,
    delegate_pointer, delegate_primary_selection, delegate_registry, delegate_seat, delegate_shm,
    delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    primary_selection::{
        PrimarySelectionManagerState,
        device::{PrimarySelectionDevice, PrimarySelectionDeviceHandler},
        offer::PrimarySelectionOffer,
        selection::{PrimarySelectionSource, PrimarySelectionSourceHandler},
    },
    reexports::{
        calloop::{
            EventLoop, LoopHandle,
            ping::{Ping, make_ping},
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{BTN_LEFT, BTN_MIDDLE, PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{
        wl_data_device, wl_data_device_manager::DndAction, wl_data_source, wl_keyboard, wl_output,
        wl_pointer, wl_seat, wl_shm, wl_surface,
    },
};

use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::{LayoutNode, SplintId};
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, ControlTransferDecision, ControlTransferOutcome,
    HistoryTransition, MouseTracking, SearchMatch, SearchPage, TerminalCell, TerminalInputModes,
    TerminalRow, TerminalSnapshot, TerminalUpdate, UnderlineStyle,
    perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled},
};

use smithay_client_toolkit::reexports::protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    primary_selection::zv1::client::{
        zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
    },
    text_input::zv3::client::{
        zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        zwp_text_input_v3::{self, ZwpTextInputV3},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use crate::config::{APP_ID, CursorStyle, FrameTitleMode, PaneDividerStyle, ResolvedTheme};
use crate::geometry::{
    OutputDpiObservation, Rect, SurfaceGeometry, WindowGeometry, buffer_to_logical_ceil,
    logical_extent_to_buffer,
};
use crate::pane::{FocusDirection, PaneChrome, PaneDivider, PaneLayout};
use crate::renderer::{
    ChromeText, CursorPresentation, HistoryOverlayStatus, SnapshotFrame, SnapshotOverlays, TextRow,
    configured_background_bgra, history_overlay_layout, paint, paint_box_drawing_cell,
    paint_history_overlay, paint_snapshot_overlays, paint_snapshot_presented,
    paint_snapshot_region_presented, paint_snapshot_rows_presented, scroll_snapshot_pixels,
    set_background_alpha, set_font_zoom_steps, snapshot_row_rect, update_output_dpi, write_ppm,
};
use crate::viewport::ScrollbackViewport;

const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 600;
const TEXT_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const MAX_CLIPBOARD_WORKERS: usize = 4;
const MAX_CACHED_HISTORY_ROWS: usize = 4096;
const MAX_CACHED_HISTORY_BYTES: usize = 16 * 1024 * 1024;
const CLIPBOARD_IO_TIMEOUT: Duration = Duration::from_secs(2);
// Keep application mouse reports at one report per wheel step. Local history
// follows Foot's default three-lines-per-step semantic distance; visual motion
// must be smoothed in pixels rather than by increasing this row multiplier.
const SCROLLBACK_WHEEL_MULTIPLIER: f64 = 3.0;
const WHEEL_VALUE120_STEP: f64 = 120.0;
const SCALE_DENOMINATOR: u32 = 120;
const MIN_SCALE_120: u32 = 120;
const MAX_SCALE_120: u32 = 960;
const MAX_PREEDIT_BYTES: usize = 4 * 1024;
const EVENT_LOOP_TICK_INTERVAL: Duration = Duration::from_millis(50);
const RECEIVER_DRAIN_BUDGET: usize = 8;
const MAX_SHM_BUFFERS: usize = 2;
const BTN_RIGHT: u32 = 0x111;

struct UpdateWake(Ping);

impl Wake for UpdateWake {
    fn wake(self: Arc<Self>) {
        self.0.ping();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.ping();
    }
}

enum ReceiverPoll<T> {
    Item(T),
    Pending,
    Disconnected,
}

struct ReceiverDrain<T> {
    items: Vec<T>,
    disconnected: bool,
}

fn poll_receiver<T>(receiver: &mut Receiver<T>, waker: &Waker) -> ReceiverPoll<T> {
    let mut context = TaskContext::from_waker(waker);
    match Pin::new(receiver).poll_recv(&mut context) {
        Poll::Ready(Some(item)) => ReceiverPoll::Item(item),
        Poll::Ready(None) => ReceiverPoll::Disconnected,
        Poll::Pending => ReceiverPoll::Pending,
    }
}

fn drain_receiver<T>(receiver: &mut Receiver<T>, waker: &Waker) -> ReceiverDrain<T> {
    let mut items = Vec::with_capacity(RECEIVER_DRAIN_BUDGET);
    for _ in 0..RECEIVER_DRAIN_BUDGET {
        match poll_receiver(receiver, waker) {
            ReceiverPoll::Item(item) => items.push(item),
            ReceiverPoll::Pending => {
                return ReceiverDrain {
                    items,
                    disconnected: false,
                };
            }
            ReceiverPoll::Disconnected => {
                return ReceiverDrain {
                    items,
                    disconnected: true,
                };
            }
        }
    }
    if receiver.is_closed() {
        // A closed producer cannot refill the queue, so preserve the previous
        // drain-before-disconnect behavior without risking starvation.
        while let Ok(item) = receiver.try_recv() {
            items.push(item);
        }
        return ReceiverDrain {
            items,
            disconnected: true,
        };
    }
    // Yield to drawing and Wayland dispatch even when a producer refills the
    // bounded channel as quickly as items are consumed. Re-arm this loop so
    // the retained backlog is handled without waiting for the periodic tick.
    waker.wake_by_ref();
    ReceiverDrain {
        items,
        disconnected: false,
    }
}
static ACTIVE_CLIPBOARD_WORKERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct CellPosition {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectionEndpoint {
    active_screen: ActiveScreen,
    history_generation: u64,
    row_id: u64,
    column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Selection {
    anchor: SelectionEndpoint,
    end: SelectionEndpoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PasteTarget {
    Clipboard,
    Primary,
}

#[derive(Clone, Copy, Debug)]
enum PressOwner {
    Application {
        code: u8,
        tracking: MouseTracking,
        sgr: bool,
        modifiers: Modifiers,
    },
    Selection,
    PrimaryPaste,
    Url,
    Ignored,
}

struct ClipboardRead {
    target: PasteTarget,
    bytes: io::Result<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignoffStep {
    WaitHistory,
    LoadSelectionWindow,
    BeginSelection,
    ExtendSelection,
    WaitSelectedOutput,
    FinishSelection,
    LocalWheel,
    LoadClientCache,
    WaitMouseTracking,
    ApplicationWheel,
    ReturnLive,
    Complete,
}

struct SignoffProbe {
    report_path: PathBuf,
    started_at: Instant,
    step: SignoffStep,
    selection_revision: u64,
    cache_window: Option<(usize, u64)>,
    evidence: Vec<serde_json::Value>,
}

impl SignoffProbe {
    fn from_environment(development_bypass: bool) -> Result<Option<Self>> {
        let Some(path) = std::env::var_os("SPLINTERM_SIGNOFF_REPORT") else {
            return Ok(None);
        };
        anyhow::ensure!(
            development_bypass,
            "SPLINTERM_SIGNOFF_REPORT requires SPLINTERM_ENABLE_DEV_ATTACH=1"
        );
        anyhow::ensure!(!path.is_empty(), "SPLINTERM_SIGNOFF_REPORT is empty");
        Ok(Some(Self {
            report_path: PathBuf::from(path),
            started_at: Instant::now(),
            step: SignoffStep::WaitHistory,
            selection_revision: 0,
            cache_window: None,
            evidence: Vec::new(),
        }))
    }
}

struct GraphicalInputProbe {
    target_revisions: usize,
    observed_revisions: HashSet<(SplintId, u64, u64)>,
}

impl GraphicalInputProbe {
    fn from_environment(development_bypass: bool) -> Result<Option<Self>> {
        let Some(value) = std::env::var_os("SPLINTERM_GRAPHICAL_INPUT_AFTER_COMMITS") else {
            return Ok(None);
        };
        anyhow::ensure!(
            development_bypass,
            "SPLINTERM_GRAPHICAL_INPUT_AFTER_COMMITS requires SPLINTERM_ENABLE_DEV_ATTACH=1"
        );
        let value = value
            .to_str()
            .context("SPLINTERM_GRAPHICAL_INPUT_AFTER_COMMITS must be UTF-8")?;
        let remaining_revisions = value
            .parse::<usize>()
            .context("SPLINTERM_GRAPHICAL_INPUT_AFTER_COMMITS must be a positive integer")?;
        anyhow::ensure!(
            (1..=1024).contains(&remaining_revisions),
            "SPLINTERM_GRAPHICAL_INPUT_AFTER_COMMITS must be between 1 and 1024"
        );
        Ok(Some(Self {
            target_revisions: remaining_revisions,
            observed_revisions: HashSet::with_capacity(remaining_revisions),
        }))
    }

    fn observe_commit(&mut self, identity: Option<(SplintId, u64, u64)>) -> bool {
        let Some(identity) = identity else {
            return false;
        };
        self.observed_revisions.insert(identity);
        self.observed_revisions.len() >= self.target_revisions
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum PendingPreedit {
    #[default]
    Unchanged,
    Clear,
    Set(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ImeBatch {
    preedit: PendingPreedit,
    commit: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ImeState {
    entered: bool,
    focused: bool,
    sent_commit_serial: u32,
    visible_preedit: Option<String>,
    pending: ImeBatch,
}

impl ImeState {
    fn set_preedit(&mut self, text: Option<String>) {
        self.pending.preedit = match text {
            Some(text) if text.len() <= MAX_PREEDIT_BYTES => PendingPreedit::Set(text),
            Some(_) => PendingPreedit::Unchanged,
            None => PendingPreedit::Clear,
        };
    }

    fn set_commit(&mut self, text: Option<String>) {
        self.pending.commit = text.filter(|text| text.len() <= MAX_PREEDIT_BYTES);
    }

    fn note_client_commit(&mut self) {
        self.sent_commit_serial = self.sent_commit_serial.wrapping_add(1);
    }

    fn finish(&mut self, serial: u32) -> (bool, Option<String>, Option<String>) {
        let serial_matches = serial == self.sent_commit_serial;
        let commit = self.pending.commit.take();
        match std::mem::take(&mut self.pending.preedit) {
            PendingPreedit::Unchanged if commit.is_some() => self.visible_preedit = None,
            PendingPreedit::Unchanged => {}
            PendingPreedit::Clear => self.visible_preedit = None,
            PendingPreedit::Set(preedit) => self.visible_preedit = Some(preedit),
        }
        (serial_matches, self.visible_preedit.clone(), commit)
    }

    fn composing(&self) -> bool {
        self.visible_preedit.is_some() || matches!(self.pending.preedit, PendingPreedit::Set(_))
    }

    fn clear(&mut self) {
        self.entered = false;
        self.visible_preedit = None;
        self.pending = ImeBatch::default();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WheelUnit {
    Value120,
    Discrete,
    Pixel,
}

#[derive(Debug, Default)]
struct WheelAccumulator {
    unit: Option<WheelUnit>,
    remainder: f64,
}

struct ClipboardWorkerPermit<'a> {
    active: &'a AtomicUsize,
}

impl Drop for ClipboardWorkerPermit<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
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
    },
    Update {
        update: TerminalUpdate,
        image_sources: Option<ImageContentLeaseSet>,
    },
    ScrollbackPages(Vec<splinterm_protocol::ScrollbackPage>),
    ScrollbackResyncRequired,
    Authority(AuthorityStatus),
    Control(bool),
    ControlTransferRequested(u64),
    ControlTransferResolved(ControlTransferOutcome),
    SearchResults(SearchPage),
    SearchResyncRequired,
    Theme(ResolvedTheme),
    Shutdown,
}

/// Bounded Wayland-to-protocol commands for the first interactive slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowCommand {
    Input(Vec<u8>),
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthorityStatus {
    pub grants: Vec<(u64, String)>,
    pub development_bypass: bool,
}

pub struct TrustedConsentUi {
    pub decision: StdSender<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowTopologyCommand {
    Split {
        target: SplintId,
        axis: splinterm_core::Axis,
    },
    Close {
        target: SplintId,
    },
    AdjustRatio {
        target: SplintId,
        delta: i16,
    },
}

pub enum WindowTopologyUpdate {
    Apply {
        layout: LayoutNode,
        added: Vec<WindowPaneOptions>,
        removed: Vec<SplintId>,
    },
    Closed,
    Shutdown(String),
}

pub struct WindowPaneOptions {
    pub snapshot: TerminalSnapshot,
    pub updates: Receiver<WindowUpdate>,
    pub commands: Sender<WindowCommand>,
    pub authority: AuthorityStatus,
    pub controlled: bool,
    pub image_sources: ImageContentLeaseSet,
}

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
    /// Multi-pane input. Empty retains the legacy one-pane fields above.
    pub panes: Vec<WindowPaneOptions>,
    pub layout: Option<LayoutNode>,
    /// Client-local initial focus; never written back implicitly.
    pub active_splint: Option<SplintId>,
    pub topology_updates: Option<Receiver<WindowTopologyUpdate>>,
    pub topology_commands: Option<Sender<WindowTopologyCommand>>,
}

impl WindowOptions {
    fn activate_multi_pane_input(&mut self) -> Result<Vec<WindowPaneOptions>> {
        if self.panes.is_empty() {
            return Ok(Vec::new());
        }
        anyhow::ensure!(
            self.snapshot.is_none() && self.updates.is_none() && self.commands.is_none(),
            "legacy and multi-pane window inputs cannot be mixed"
        );
        let layout = self
            .layout
            .as_ref()
            .context("multi-pane layout is required")?;
        anyhow::ensure!(
            layout.splint_count() == self.panes.len(),
            "layout and pane input counts differ"
        );
        let mut identities = HashSet::new();
        for pane in &self.panes {
            pane.snapshot
                .validate()
                .map_err(|error| anyhow::anyhow!(error.message))?;
            anyhow::ensure!(
                identities.insert(pane.snapshot.splint_id)
                    && layout.find_splint(pane.snapshot.splint_id).is_some(),
                "pane input identity is duplicate or absent from the layout"
            );
        }
        let active = self
            .active_splint
            .unwrap_or_else(|| layout.first_splint_id());
        let index = self
            .panes
            .iter()
            .position(|pane| pane.snapshot.splint_id == active)
            .context("active Splint is absent from pane inputs")?;
        let mut active = self.panes.remove(index);
        apply_theme(&mut active.snapshot, self.theme);
        self.snapshot = Some(active.snapshot);
        self.updates = Some(active.updates);
        self.commands = Some(active.commands);
        self.authority = active.authority;
        self.controlled = active.controlled;
        for pane in &mut self.panes {
            apply_theme(&mut pane.snapshot, self.theme);
        }
        Ok(std::mem::take(&mut self.panes))
    }
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
            panes: Vec::new(),
            layout: None,
            active_splint: None,
            topology_updates: None,
            topology_commands: None,
        }
    }
}

/// Opens the native window and runs until compositor close or an explicit shutdown.
///
/// Q/Escape close shortcuts are available only when `evidence_close_shortcuts` is set.
///
/// # Errors
///
/// Returns an error when font setup, required Wayland globals, shared-memory buffers,
/// keyboard state, capture output, or event dispatch cannot be initialized.
#[allow(
    clippy::too_many_lines,
    reason = "Wayland global binding and application state initialization form one startup transaction"
)]
pub fn run(mut options: WindowOptions) -> Result<()> {
    let inactive_options = options.activate_multi_pane_input()?;
    if let Some(snapshot) = options.snapshot.as_mut() {
        snapshot
            .validate()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        apply_theme(snapshot, options.theme);
    }
    let text_row = options
        .snapshot
        .is_none()
        .then(|| TextRow::load(1))
        .transpose()?;
    let snapshot_frame = options
        .snapshot
        .as_ref()
        .map(|snapshot| {
            SnapshotFrame::load_scaled_with_sources(snapshot, 120, Some(&options.image_sources))
        })
        .transpose()?;
    let (initial_width, initial_height) = snapshot_frame
        .as_ref()
        .map_or(Ok((INITIAL_WIDTH, INITIAL_HEIGHT)), |frame| {
            frame.initial_logical_size(options.initial_columns, options.initial_rows, 120)
        })?;
    let connection = Connection::connect_to_env().context("connect to Wayland compositor")?;
    let (globals, event_queue) =
        registry_queue_init(&connection).context("read Wayland registry")?;
    let queue_handle = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().context("create event loop")?;
    WaylandSource::new(connection.clone(), event_queue)
        .insert(event_loop.handle())
        .context("register Wayland source")?;
    let (update_ping, update_ping_source) = make_ping().context("create update wake source")?;
    event_loop
        .handle()
        .insert_source(update_ping_source, |(), (), _| {})
        .context("register update wake source")?;
    let update_waker = Waker::from(Arc::new(UpdateWake(update_ping)));

    let compositor = CompositorState::bind(&globals, &queue_handle)
        .context("compositor does not provide wl_compositor")?;
    let shell =
        XdgShell::bind(&globals, &queue_handle).context("compositor does not provide xdg-shell")?;
    let shm = Shm::bind(&globals, &queue_handle).context("compositor does not provide wl_shm")?;
    let data_device_manager = DataDeviceManagerState::bind(&globals, &queue_handle)
        .context("compositor does not provide wl_data_device_manager")?;
    let primary_selection_manager =
        PrimarySelectionManagerState::bind(&globals, &queue_handle).ok();
    let fractional_scale_manager = globals
        .bind::<WpFractionalScaleManagerV1, _, _>(&queue_handle, 1..=1, ())
        .ok();
    let viewporter = globals
        .bind::<WpViewporter, _, _>(&queue_handle, 1..=1, ())
        .ok();
    let text_input_manager = globals
        .bind::<ZwpTextInputManagerV3, _, _>(&queue_handle, 1..=1, ())
        .ok();
    let (clipboard_tx, clipboard_rx) = std_mpsc::channel();
    let surface = compositor.create_surface(&queue_handle);
    let window = shell.create_window(surface, WindowDecorations::RequestServer, &queue_handle);
    let fractional_scale = fractional_scale_manager
        .as_ref()
        .zip(viewporter.as_ref())
        .map(|(manager, _)| manager.get_fractional_scale(window.wl_surface(), &queue_handle, ()));
    let viewport = fractional_scale_manager
        .as_ref()
        .zip(viewporter.as_ref())
        .map(|(_, manager)| manager.get_viewport(window.wl_surface(), &queue_handle, ()));
    if let Some(viewport) = &viewport {
        let (width, height) = viewport_destination(initial_width, initial_height)?;
        viewport.set_destination(width, height);
    }
    let controller_active = options.controlled && options.commands.is_some();
    let trusted_consent = options.trusted_consent;
    let title = if trusted_consent.is_some() {
        "Splinterm — Trusted Access Request".to_owned()
    } else {
        window_title(
            options.title.as_deref().or_else(|| {
                options
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.title.as_str())
            }),
            controller_active,
            &options.authority,
            false,
            None,
        )
    };
    window.set_title(title);
    window.set_app_id(APP_ID);
    window
        .set_buffer_scale(1)
        .map_err(|_| anyhow::anyhow!("compositor does not support integer buffer scale"))?;
    window.commit();

    let pool_size = usize::try_from(
        initial_width
            .checked_mul(initial_height)
            .and_then(|pixels| pixels.checked_mul(4))
            .context("initial SHM size overflow")?,
    )
    .context("initial SHM pool size fits usize")?;
    let pool = SlotPool::new(pool_size, &shm).context("create SHM pool")?;
    let signoff = SignoffProbe::from_environment(options.authority.development_bypass)?;
    let graphical_input_probe =
        GraphicalInputProbe::from_environment(options.authority.development_bypass)?;
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &queue_handle),
        output_state: OutputState::new(&globals, &queue_handle),
        data_device_manager,
        primary_selection_manager,
        fractional_scale,
        viewport,
        text_input_manager,
        text_input: None,
        text_input_seat: None,
        ime: ImeState::default(),
        reduced_motion: reduced_motion_requested(),
        keyboard_focused: false,
        shm,
        window,
        pool,
        text_row,
        pane: PaneView {
            snapshot: options.snapshot,
            snapshot_frame,
            image_sources: options.image_sources,
            scrollback_viewport: ScrollbackViewport::default(),
            painted_history_status: None,
            history_page_pending: false,
            history_selection_pin_blocked: false,
            scroll_started_at: None,
            rendered_viewport_offset: 0,
            viewport_dirty: false,
            updates: options.updates,
            commands: options.commands,
            controller_active,
            pending_control_transfer: None,
            search: SearchUiState::default(),
            authority: options.authority,
            last_resize: None,
            prepare_dirty_rows: Vec::new(),
            raster_dirty_rows: Vec::new(),
            surface_dirty_rows: Vec::new(),
            pending_scrolls: Vec::new(),
            selected_text: None,
            selection: None,
            selecting: false,
            pointer_cell: None,
            hovered_url: None,
        },
        inactive_panes: inactive_options
            .into_iter()
            .map(|pane| PaneView::from_options(pane, SCALE_DENOMINATOR))
            .collect::<Result<Vec<_>>>()?,
        layout: options.layout,
        topology_updates: options.topology_updates,
        topology_commands: options.topology_commands,
        signoff,
        graphical_input_probe,
        scroll_trace: std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some(),
        trusted_consent,
        cursor_style: options.cursor_style,
        cursor_blink: options.cursor_blink,
        title_override: options.title,
        theme: options.theme,
        pane_divider_style: options.pane_divider_style,
        frame_title_mode: options.frame_title_mode,
        frame_titles: HashMap::new(),
        evidence_close_shortcuts: options.evidence_close_shortcuts,
        modifiers: Modifiers::default(),
        font_zoom_steps: 0,
        capture: options.capture,
        capture_scale: options.capture_scale,
        buffers: Vec::new(),
        backing: Vec::new(),
        full_redraw: true,
        keyboard: None,
        keyboard_seat: None,
        pointer: None,
        pointer_seat: None,
        data_device: None,
        primary_device: None,
        clipboard_offer: None,
        primary_offer: None,
        clipboard_sources: Vec::new(),
        primary_sources: Vec::new(),
        clipboard_tx,
        clipboard_rx,
        last_pointer_serial: None,
        pressed_buttons: HashMap::new(),
        vertical_wheel: WheelAccumulator::default(),
        scrollback_wheel: WheelAccumulator::default(),
        loop_handle: event_loop.handle(),
        update_waker,
        logical_width: initial_width,
        logical_height: initial_height,
        configured: false,
        exit: false,
        failure: None,
        frame_pending: false,
        redraw_pending: false,
        terminal_redraw_pending: false,
        cursor_blink_visible: true,
        last_cursor_blink: Instant::now(),
        scale_120: SCALE_DENOMINATOR,
        integer_fallback_scale: 1,
        output_count: 0,
        entered_outputs: Vec::new(),
        seat_count: 0,
    };

    while !app.exit {
        app.apply_updates(&queue_handle)?;
        if app.redraw_pending
            && !pending_draw_waits_for_frame(app.frame_pending, app.terminal_redraw_pending)
        {
            if app.terminal_redraw_pending {
                app.schedule_terminal_draw(&queue_handle)?;
            } else {
                app.schedule_draw(&queue_handle)?;
            }
        }
        app.tick_signoff(&queue_handle)?;
        app.apply_clipboard_reads()?;
        app.tick_cursor_blink(&queue_handle)?;
        event_loop
            .dispatch(EVENT_LOOP_TICK_INTERVAL, &mut app)
            .context("dispatch Wayland events")?;
    }
    if let Some(error) = app.failure {
        return Err(error);
    }
    Ok(())
}

fn paint_trusted_consent_chrome(canvas: &mut [u8], width: u32, height: u32) {
    fn fill(canvas: &mut [u8], width: u32, x0: u32, y0: u32, x1: u32, y1: u32, rgb: u32) {
        let [_, red, green, blue] = rgb.to_be_bytes();
        for y in y0.min(y1)..y1 {
            for x in x0.min(x1)..x1 {
                let Ok(index) = usize::try_from((y * width + x) * 4) else {
                    continue;
                };
                if let Some(pixel) = canvas.get_mut(index..index + 4) {
                    pixel.copy_from_slice(&[blue, green, red, 0xff]);
                }
            }
        }
    }
    let border = width.min(height).div_ceil(80).max(4);
    fill(canvas, width, 0, 0, width, border, 0x00e0_a030);
    fill(
        canvas,
        width,
        0,
        height.saturating_sub(border),
        width,
        height,
        0x00e0_a030,
    );
    fill(canvas, width, 0, 0, border, height, 0x00e0_a030);
    fill(
        canvas,
        width,
        width.saturating_sub(border),
        0,
        width,
        height,
        0x00e0_a030,
    );
    let button_top = height.saturating_mul(78) / 100;
    let middle = width / 2;
    fill(
        canvas,
        width,
        border,
        button_top,
        middle.saturating_sub(2),
        height.saturating_sub(border),
        0x0070_2020,
    );
    fill(
        canvas,
        width,
        middle.saturating_add(2),
        button_top,
        width.saturating_sub(border),
        height.saturating_sub(border),
        0x0020_7040,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "bounded box cells require explicit canvas, clip, metrics, color, scale, and direction"
)]
fn paint_box_sequence(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    character: char,
    clip: Rect,
    cell_width: u32,
    cell_height: u32,
    color: u32,
    scale_120: u16,
    horizontal: bool,
) {
    let step = if horizontal { cell_width } else { cell_height };
    if step == 0 {
        return;
    }
    let end = if horizontal {
        clip.x.saturating_add(clip.width)
    } else {
        clip.y.saturating_add(clip.height)
    };
    let mut position = if horizontal { clip.x } else { clip.y };
    while position < end {
        let cell = if horizontal {
            Rect {
                x: position,
                y: clip.y,
                width: cell_width,
                height: cell_height,
            }
        } else {
            Rect {
                x: clip.x,
                y: position,
                width: cell_width,
                height: cell_height,
            }
        };
        paint_box_drawing_cell(
            canvas, width, height, character, cell, clip, color, scale_120,
        );
        position = position.saturating_add(step);
    }
}

fn sanitize_frame_title(title: &str, maximum_cells: u32) -> String {
    let mut output = String::new();
    let mut cells = 0_u32;
    let mut previous_space = false;
    for character in title.chars() {
        let character = if character.is_control() || character.is_whitespace() {
            ' '
        } else {
            character
        };
        if character == ' ' && (previous_space || output.is_empty()) {
            continue;
        }
        let width = u32::try_from(character.width().unwrap_or(0)).unwrap_or(0);
        if width > 0 && cells.saturating_add(width) > maximum_cells {
            break;
        }
        output.push(character);
        cells = cells.saturating_add(width);
        previous_space = character == ' ';
    }
    output.trim_end().to_owned()
}

fn fill_chrome_background(canvas: &mut [u8], width: u32, height: u32, rect: Rect, color: u32) {
    let [_, red, green, blue] = color.to_be_bytes();
    let pixel = configured_background_bgra([red, green, blue]);
    let right = rect.x.saturating_add(rect.width).min(width);
    let bottom = rect.y.saturating_add(rect.height).min(height);
    for y in rect.y.min(height)..bottom {
        for x in rect.x.min(width)..right {
            let Ok(index) = usize::try_from((y * width + x) * 4) else {
                continue;
            };
            if let Some(target) = canvas.get_mut(index..index + 4) {
                target.copy_from_slice(&pixel);
            }
        }
    }
}

fn divider_touches_pane(divider: PaneDivider, pane: Rect) -> bool {
    let divider_right = divider.rect.x.saturating_add(divider.rect.width);
    let divider_bottom = divider.rect.y.saturating_add(divider.rect.height);
    let pane_right = pane.x.saturating_add(pane.width);
    let pane_bottom = pane.y.saturating_add(pane.height);
    match divider.axis {
        splinterm_core::Axis::Horizontal => {
            (pane_right == divider.rect.x || pane.x == divider_right)
                && pane.y < divider_bottom
                && divider.rect.y < pane_bottom
        }
        splinterm_core::Axis::Vertical => {
            (pane_bottom == divider.rect.y || pane.y == divider_bottom)
                && pane.x < divider_right
                && divider.rect.x < pane_right
        }
    }
}

fn divider_junction(first: PaneDivider, second: PaneDivider) -> Option<(char, Rect)> {
    let (vertical, horizontal) = match (first.axis, second.axis) {
        (splinterm_core::Axis::Horizontal, splinterm_core::Axis::Vertical) => (first, second),
        (splinterm_core::Axis::Vertical, splinterm_core::Axis::Horizontal) => (second, first),
        _ => return None,
    };
    let vertical_right = vertical.rect.x.checked_add(vertical.rect.width)?;
    let vertical_bottom = vertical.rect.y.checked_add(vertical.rect.height)?;
    let horizontal_right = horizontal.rect.x.checked_add(horizontal.rect.width)?;
    let horizontal_bottom = horizontal.rect.y.checked_add(horizontal.rect.height)?;
    if horizontal_right == vertical.rect.x
        && horizontal.rect.y < vertical_bottom
        && vertical.rect.y < horizontal_bottom
    {
        return Some((
            '┤',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    if horizontal.rect.x == vertical_right
        && horizontal.rect.y < vertical_bottom
        && vertical.rect.y < horizontal_bottom
    {
        return Some((
            '├',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    if vertical_bottom == horizontal.rect.y
        && vertical.rect.x < horizontal_right
        && horizontal.rect.x < vertical_right
    {
        return Some((
            '┴',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    if vertical.rect.y == horizontal_bottom
        && vertical.rect.x < horizontal_right
        && horizontal.rect.x < vertical_right
    {
        return Some((
            '┬',
            Rect {
                x: vertical.rect.x,
                y: horizontal.rect.y,
                width: vertical.rect.width,
                height: horizontal.rect.height,
            },
        ));
    }
    None
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "line junctions and complete framed panels share one trusted clipped chrome pass"
)]
fn paint_pane_chrome(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    layout: &PaneLayout,
    active_splint: Option<SplintId>,
    theme: ResolvedTheme,
    cell_width: u32,
    cell_height: u32,
    scale_120: u32,
    frame_titles: &HashMap<SplintId, CachedFrameTitle>,
) -> Result<()> {
    let scale = u16::try_from(scale_120).context("pane chrome scale fits u16")?;
    match layout.chrome {
        PaneChrome::None => {}
        PaneChrome::Line { .. } => {
            let active_rect = active_splint.and_then(|id| layout.rect(id));
            for divider in &layout.separators {
                let clip = App::buffer_rect(divider.rect, scale_120)?;
                let active = active_rect.is_some_and(|pane| divider_touches_pane(*divider, pane));
                let color = if active {
                    theme.pane_border_active
                } else {
                    theme.pane_border
                };
                let horizontal = divider.axis == splinterm_core::Axis::Vertical;
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    if horizontal { '─' } else { '│' },
                    clip,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    horizontal,
                );
            }
            for (index, first) in layout.separators.iter().copied().enumerate() {
                for second in layout.separators[index + 1..].iter().copied() {
                    let Some((character, logical_cell)) = divider_junction(first, second) else {
                        continue;
                    };
                    let clip = App::buffer_rect(logical_cell, scale_120)?;
                    let active = active_rect.is_some_and(|pane| {
                        divider_touches_pane(first, pane) || divider_touches_pane(second, pane)
                    });
                    let color = if active {
                        theme.pane_border_active
                    } else {
                        theme.pane_border
                    };
                    paint_box_drawing_cell(
                        canvas,
                        width,
                        height,
                        character,
                        Rect {
                            x: clip.x,
                            y: clip.y,
                            width: cell_width,
                            height: cell_height,
                        },
                        clip,
                        color,
                        scale,
                    );
                }
            }
        }
        PaneChrome::Frame { .. } => {
            for pane in &layout.panes {
                let allocation = App::buffer_rect(pane.allocation, scale_120)?;
                let content = App::buffer_rect(pane.rect, scale_120)?;
                let right = allocation.x.saturating_add(allocation.width);
                let bottom = allocation.y.saturating_add(allocation.height);
                let content_right = content.x.saturating_add(content.width);
                let content_bottom = content.y.saturating_add(content.height);
                let color = if Some(pane.splint_id) == active_splint {
                    theme.pane_border_active
                } else {
                    theme.pane_border
                };
                let top = Rect {
                    x: allocation.x.saturating_add(cell_width),
                    y: allocation.y,
                    width: allocation
                        .width
                        .saturating_sub(cell_width.saturating_mul(2)),
                    height: content.y.saturating_sub(allocation.y),
                };
                let bottom_edge = Rect {
                    x: allocation.x.saturating_add(cell_width),
                    y: content_bottom,
                    width: allocation
                        .width
                        .saturating_sub(cell_width.saturating_mul(2)),
                    height: bottom.saturating_sub(content_bottom),
                };
                let left = Rect {
                    x: allocation.x,
                    y: allocation.y.saturating_add(cell_height),
                    width: content.x.saturating_sub(allocation.x),
                    height: allocation
                        .height
                        .saturating_sub(cell_height.saturating_mul(2)),
                };
                let right_edge = Rect {
                    x: content_right,
                    y: allocation.y.saturating_add(cell_height),
                    width: right.saturating_sub(content_right),
                    height: allocation
                        .height
                        .saturating_sub(cell_height.saturating_mul(2)),
                };
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '─',
                    top,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    true,
                );
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '─',
                    bottom_edge,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    true,
                );
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '│',
                    left,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    false,
                );
                paint_box_sequence(
                    canvas,
                    width,
                    height,
                    '│',
                    right_edge,
                    cell_width,
                    cell_height,
                    color,
                    scale,
                    false,
                );
                let top_left = Rect {
                    x: allocation.x,
                    y: allocation.y,
                    width: cell_width,
                    height: cell_height,
                };
                let top_right = Rect {
                    x: right.saturating_sub(cell_width),
                    y: allocation.y,
                    width: cell_width,
                    height: cell_height,
                };
                let bottom_left = Rect {
                    x: allocation.x,
                    y: bottom.saturating_sub(cell_height),
                    width: cell_width,
                    height: cell_height,
                };
                let bottom_right = Rect {
                    x: right.saturating_sub(cell_width),
                    y: bottom.saturating_sub(cell_height),
                    width: cell_width,
                    height: cell_height,
                };
                for (character, cell) in [
                    ('┌', top_left),
                    ('┐', top_right),
                    ('└', bottom_left),
                    ('┘', bottom_right),
                ] {
                    let clip = cell;
                    paint_box_drawing_cell(
                        canvas, width, height, character, cell, clip, color, scale,
                    );
                }
                if let Some(title) = frame_titles.get(&pane.splint_id) {
                    let clear = Rect {
                        x: allocation.x.saturating_add(cell_width.saturating_mul(2)),
                        y: allocation.y,
                        width: title
                            .text
                            .cells()
                            .saturating_add(2)
                            .saturating_mul(cell_width),
                        height: cell_height,
                    };
                    fill_chrome_background(canvas, width, height, clear, theme.background);
                    title.text.paint(
                        canvas,
                        width,
                        height,
                        (
                            allocation.x.saturating_add(cell_width.saturating_mul(3)),
                            allocation.y,
                        ),
                        clear,
                        color,
                    );
                }
            }
        }
    }
    Ok(())
}

fn apply_theme(snapshot: &mut TerminalSnapshot, theme: ResolvedTheme) {
    if snapshot.palette.len() == 256 {
        snapshot.palette[..16].copy_from_slice(&theme.ansi);
    }
    snapshot.default_colors = [theme.foreground, theme.background, theme.cursor];
}

fn viewport_destination(width: u32, height: u32) -> Result<(i32, i32)> {
    Ok((
        i32::try_from(width).context("viewport width fits i32")?,
        i32::try_from(height).context("viewport height fits i32")?,
    ))
}

fn buffer_dimensions(
    logical_width: u32,
    logical_height: u32,
    scale_120: u32,
) -> Result<(u32, u32, i32)> {
    SurfaceGeometry::new(logical_width, logical_height, scale_120)?.buffer_layout()
}

fn note_output_enter<T: Clone + Eq>(entered: &mut Vec<T>, output: &T) {
    entered.retain(|candidate| candidate != output);
    entered.push(output.clone());
}

fn note_output_leave<T: Eq>(entered: &mut Vec<T>, output: &T) -> bool {
    let was_most_recent = entered.last() == Some(output);
    entered.retain(|candidate| candidate != output);
    was_most_recent
}

#[derive(Clone, Debug, Default)]
struct SearchUiState {
    input: Option<String>,
    query: String,
    matches: Vec<SearchMatch>,
    selected: usize,
    next_cursor: Option<String>,
    pending_reveal: Option<SearchMatch>,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent pane rendering, history, and input flags are not one state machine"
)]
struct PaneView {
    snapshot: Option<TerminalSnapshot>,
    snapshot_frame: Option<SnapshotFrame>,
    image_sources: ImageContentLeaseSet,
    scrollback_viewport: ScrollbackViewport,
    painted_history_status: Option<HistoryOverlayStatus>,
    history_page_pending: bool,
    history_selection_pin_blocked: bool,
    scroll_started_at: Option<Instant>,
    rendered_viewport_offset: usize,
    viewport_dirty: bool,
    updates: Option<Receiver<WindowUpdate>>,
    commands: Option<Sender<WindowCommand>>,
    controller_active: bool,
    pending_control_transfer: Option<u64>,
    search: SearchUiState,
    authority: AuthorityStatus,
    last_resize: Option<(u16, u16, u16, u16)>,
    prepare_dirty_rows: Vec<bool>,
    raster_dirty_rows: Vec<bool>,
    surface_dirty_rows: Vec<bool>,
    pending_scrolls: Vec<splinterm_protocol::TerminalScroll>,
    selected_text: Option<Vec<u8>>,
    selection: Option<Selection>,
    selecting: bool,
    pointer_cell: Option<CellPosition>,
    hovered_url: Option<(CellPosition, CellPosition, String)>,
}

fn rebuild_pane_scaled_frame(pane: &mut PaneView, scale_120: u32) -> Result<bool> {
    let Some(display) = pane.display_snapshot() else {
        return Ok(false);
    };
    pane.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
        &display,
        scale_120,
        Some(&pane.image_sources),
    )?);
    pane.rendered_viewport_offset = pane.scrollback_viewport.offset_from_bottom();
    pane.viewport_dirty = false;
    Ok(true)
}

impl PaneView {
    fn from_options(options: WindowPaneOptions, scale_120: u32) -> Result<Self> {
        let snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
            &options.snapshot,
            scale_120,
            Some(&options.image_sources),
        )?);
        Ok(Self {
            snapshot: Some(options.snapshot),
            snapshot_frame,
            image_sources: options.image_sources,
            scrollback_viewport: ScrollbackViewport::default(),
            painted_history_status: None,
            history_page_pending: false,
            history_selection_pin_blocked: false,
            scroll_started_at: None,
            rendered_viewport_offset: 0,
            viewport_dirty: false,
            updates: Some(options.updates),
            commands: Some(options.commands),
            controller_active: options.controlled,
            pending_control_transfer: None,
            search: SearchUiState::default(),
            authority: options.authority,
            last_resize: None,
            prepare_dirty_rows: Vec::new(),
            raster_dirty_rows: Vec::new(),
            surface_dirty_rows: Vec::new(),
            pending_scrolls: Vec::new(),
            selected_text: None,
            selection: None,
            selecting: false,
            pointer_cell: None,
            hovered_url: None,
        })
    }

    fn clear_local_content_state(&mut self) {
        self.selected_text = None;
        self.selection = None;
        self.selecting = false;
        self.hovered_url = None;
        self.history_selection_pin_blocked = false;
    }

    fn display_snapshot(&self) -> Option<TerminalSnapshot> {
        let snapshot = self.snapshot.as_ref()?;
        if self.scrollback_viewport.is_live() {
            return Some(snapshot.clone());
        }
        let mut display = snapshot.clone();
        let cursor_row = viewport_cursor_row(
            snapshot.cursor_row,
            self.scrollback_viewport.offset_from_bottom(),
            snapshot.rows,
        );
        if cursor_row.is_none() {
            display.input_modes.cursor_visible = false;
        }
        display.cursor_column = cursor_row.map_or(-1, |_| snapshot.cursor_column);
        display.cursor_row = cursor_row.unwrap_or(-1);
        display.cursor_deferred_wrap = false;
        display.visible_rows = self
            .scrollback_viewport
            .visible_rows(snapshot)
            .into_iter()
            .cloned()
            .collect();
        display.oldest_available_scrollback_row_id = None;
        display.newest_available_scrollback_row_id = None;
        display.scrollback_rows.clear();
        display.omitted_oldest_scrollback_rows = display.available_scrollback_rows;
        Some(display)
    }

    fn apply_background_pages(
        &mut self,
        pages: Vec<splinterm_protocol::ScrollbackPage>,
    ) -> Result<bool> {
        self.history_page_pending = false;
        let pinned = self
            .selection
            .map(|selection| [selection.anchor.row_id, selection.end.row_id]);
        let snapshot = self
            .snapshot
            .as_mut()
            .context("scrollback pages arrived before initial pane snapshot")?;
        if pages.iter().any(|page| {
            page.splint_id != snapshot.splint_id
                || page.incarnation != snapshot.incarnation
                || page.terminal_revision != snapshot.revision
                || page.history_generation != snapshot.history_generation
        }) {
            return Ok(false);
        }
        let first_loaded = snapshot
            .scrollback_rows
            .first()
            .and_then(|row| row.row_id)
            .unwrap_or(u64::MAX);
        let existing = snapshot
            .scrollback_rows
            .iter()
            .filter_map(|row| row.row_id)
            .collect::<std::collections::BTreeSet<_>>();
        let metadata = pages
            .first()
            .map(|page| (page.oldest_available_row_id, page.newest_available_row_id));
        let mut older = pages
            .into_iter()
            .rev()
            .flat_map(|page| page.rows)
            .filter(|row| {
                row.row_id
                    .is_some_and(|id| id < first_loaded && !existing.contains(&id))
            })
            .collect::<Vec<_>>();
        if older.is_empty() {
            return Ok(false);
        }
        older.extend(snapshot.scrollback_rows.iter().cloned());
        let Some(older) = bound_history_page_with_pins(older, pinned, &snapshot.visible_rows)
        else {
            self.history_selection_pin_blocked = true;
            return Ok(false);
        };
        snapshot.scrollback_rows = older;
        snapshot.omitted_oldest_scrollback_rows = omitted_rows_before_cache(
            snapshot.oldest_available_scrollback_row_id,
            &snapshot.scrollback_rows,
            snapshot.available_scrollback_rows,
        );
        if let Some((oldest, newest)) = metadata {
            snapshot.oldest_available_scrollback_row_id = oldest;
            snapshot.newest_available_scrollback_row_id = newest;
        }
        Ok(true)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the bounded pane reducer keeps every protocol update transition explicit"
    )]
    fn apply_background_update(
        &mut self,
        update: WindowUpdate,
        theme: ResolvedTheme,
        scale_120: u32,
    ) -> Result<bool> {
        match update {
            WindowUpdate::Snapshot {
                mut snapshot,
                image_sources,
            } => {
                snapshot
                    .validate()
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                apply_theme(&mut snapshot, theme);
                if let Some(current) = self.snapshot.as_ref() {
                    if !snapshot_is_newer(current, &snapshot)? {
                        return Ok(false);
                    }
                }
                let previous_generation = self
                    .snapshot
                    .as_ref()
                    .map_or(snapshot.history_generation, |current| {
                        current.history_generation
                    });
                let previous_rows = self
                    .snapshot
                    .as_ref()
                    .map_or_else(Vec::new, |current| current.scrollback_rows.clone());
                self.scrollback_viewport.observe_history_change(
                    previous_generation,
                    &previous_rows,
                    &snapshot,
                );
                self.clear_local_content_state();
                self.snapshot = Some(snapshot);
                self.image_sources = image_sources;
                let display = self
                    .display_snapshot()
                    .context("background display snapshot")?;
                self.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
                    &display,
                    scale_120,
                    Some(&self.image_sources),
                )?);
                Ok(true)
            }
            WindowUpdate::Update {
                update,
                image_sources,
            } => {
                let changed = terminal_update_changes_visible_content(&update);
                let previous_generation = self
                    .snapshot
                    .as_ref()
                    .map_or(1, |snapshot| snapshot.history_generation);
                let previous_rows = self
                    .snapshot
                    .as_ref()
                    .map_or_else(Vec::new, |snapshot| snapshot.scrollback_rows.clone());
                {
                    let snapshot = self
                        .snapshot
                        .as_mut()
                        .context("terminal update arrived before initial pane snapshot")?;
                    apply_terminal_update(snapshot, update)?;
                    apply_theme(snapshot, theme);
                    self.scrollback_viewport.observe_history_change(
                        previous_generation,
                        &previous_rows,
                        snapshot,
                    );
                }
                if let Some(image_sources) = image_sources {
                    self.image_sources = image_sources;
                }
                let display = self
                    .display_snapshot()
                    .context("background display snapshot")?;
                let frame = SnapshotFrame::load_scaled_with_sources(
                    &display,
                    scale_120,
                    Some(&self.image_sources),
                )?;
                if changed {
                    self.clear_local_content_state();
                }
                self.snapshot_frame = Some(frame);
                Ok(true)
            }
            WindowUpdate::Authority(authority) => {
                self.authority = authority;
                Ok(true)
            }
            WindowUpdate::Control(active) => {
                self.controller_active = active;
                Ok(true)
            }
            WindowUpdate::ControlTransferRequested(transfer_id) => {
                self.pending_control_transfer = Some(transfer_id);
                Ok(true)
            }
            WindowUpdate::ControlTransferResolved(_) => {
                self.pending_control_transfer = None;
                Ok(true)
            }
            WindowUpdate::SearchResults(page) => {
                self.search.matches = page.matches;
                self.search.selected = 0;
                self.search.next_cursor = page.next_cursor;
                self.search.pending_reveal = self.search.matches.first().cloned();
                Ok(true)
            }
            WindowUpdate::SearchResyncRequired => {
                self.search.matches.clear();
                self.search.next_cursor = None;
                self.search.pending_reveal = None;
                Ok(true)
            }
            WindowUpdate::ScrollbackResyncRequired => {
                self.history_page_pending = false;
                self.clear_local_content_state();
                Ok(true)
            }
            WindowUpdate::ScrollbackPages(pages) => self.apply_background_pages(pages),
            WindowUpdate::Theme(_) => Ok(false),
            WindowUpdate::Shutdown => {
                self.controller_active = false;
                self.commands = None;
                self.updates = None;
                Ok(true)
            }
        }
    }
}

struct CachedFrameTitle {
    source: String,
    maximum_cells: u32,
    scale_120: u32,
    text: ChromeText,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent Wayland lifecycle and evidence-mode flags are not one state machine"
)]
struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    data_device_manager: DataDeviceManagerState,
    primary_selection_manager: Option<PrimarySelectionManagerState>,
    fractional_scale: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    text_input_manager: Option<ZwpTextInputManagerV3>,
    text_input: Option<ZwpTextInputV3>,
    text_input_seat: Option<wl_seat::WlSeat>,
    ime: ImeState,
    reduced_motion: bool,
    keyboard_focused: bool,
    shm: Shm,
    window: Window,
    pool: SlotPool,
    text_row: Option<TextRow>,
    pane: PaneView,
    inactive_panes: Vec<PaneView>,
    layout: Option<LayoutNode>,
    topology_updates: Option<Receiver<WindowTopologyUpdate>>,
    topology_commands: Option<Sender<WindowTopologyCommand>>,
    signoff: Option<SignoffProbe>,
    graphical_input_probe: Option<GraphicalInputProbe>,
    scroll_trace: bool,
    trusted_consent: Option<TrustedConsentUi>,
    cursor_style: CursorStyle,
    cursor_blink: bool,
    title_override: Option<String>,
    theme: ResolvedTheme,
    pane_divider_style: PaneDividerStyle,
    frame_title_mode: FrameTitleMode,
    frame_titles: HashMap<SplintId, CachedFrameTitle>,
    evidence_close_shortcuts: bool,
    modifiers: Modifiers,
    font_zoom_steps: i16,
    capture: Option<PathBuf>,
    capture_scale: Option<u32>,
    buffers: Vec<Buffer>,
    backing: Vec<u8>,
    full_redraw: bool,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keyboard_seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    pointer_seat: Option<wl_seat::WlSeat>,
    data_device: Option<DataDevice>,
    primary_device: Option<PrimarySelectionDevice>,
    clipboard_offer: Option<SelectionOffer>,
    primary_offer: Option<PrimarySelectionOffer>,
    clipboard_sources: Vec<(CopyPasteSource, Arc<[u8]>)>,
    primary_sources: Vec<(PrimarySelectionSource, Arc<[u8]>)>,
    clipboard_tx: StdSender<ClipboardRead>,
    clipboard_rx: StdReceiver<ClipboardRead>,
    last_pointer_serial: Option<u32>,
    pressed_buttons: HashMap<u32, PressOwner>,
    vertical_wheel: WheelAccumulator,
    scrollback_wheel: WheelAccumulator,
    loop_handle: LoopHandle<'static, App>,
    update_waker: Waker,
    logical_width: u32,
    logical_height: u32,
    configured: bool,
    exit: bool,
    failure: Option<anyhow::Error>,
    frame_pending: bool,
    redraw_pending: bool,
    terminal_redraw_pending: bool,
    cursor_blink_visible: bool,
    last_cursor_blink: Instant,
    scale_120: u32,
    integer_fallback_scale: u32,
    output_count: usize,
    /// Enter order is significant: the last element is Foot's most-recent output.
    entered_outputs: Vec<wl_output::WlOutput>,
    seat_count: usize,
}

fn snapshot_is_newer(current: &TerminalSnapshot, candidate: &TerminalSnapshot) -> Result<bool> {
    if candidate.splint_id != current.splint_id || candidate.incarnation != current.incarnation {
        anyhow::bail!(
            "live snapshot identity changed from {:?}/{} to {:?}/{}",
            current.splint_id,
            current.incarnation,
            candidate.splint_id,
            candidate.incarnation
        );
    }
    Ok(candidate.revision > current.revision)
}

#[cfg(test)]
fn coalesce_snapshots(
    current: Option<&TerminalSnapshot>,
    pending: impl IntoIterator<Item = TerminalSnapshot>,
) -> Result<Option<TerminalSnapshot>> {
    let mut latest = None;
    for candidate in pending {
        let baseline = latest.as_ref().or(current);
        if match baseline {
            Some(snapshot) => snapshot_is_newer(snapshot, &candidate)?,
            None => true,
        } {
            latest = Some(candidate);
        }
    }
    Ok(latest)
}

fn blank_row(columns: usize) -> TerminalRow {
    TerminalRow {
        row_id: None,
        linebreak: false,
        cells: vec![
            TerminalCell {
                content: String::new(),
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
            };
            columns
        ],
    }
}

fn terminal_row_cache_bytes(row: &TerminalRow) -> usize {
    row.cells.iter().fold(32_usize, |total, cell| {
        total.saturating_add(64).saturating_add(cell.content.len())
    })
}

fn history_cache_bytes(rows: &[TerminalRow]) -> usize {
    rows.iter().map(terminal_row_cache_bytes).sum()
}

fn omitted_rows_before_cache(
    oldest_available_row_id: Option<u64>,
    rows: &[TerminalRow],
    available_rows: usize,
) -> usize {
    oldest_available_row_id
        .zip(rows.first().and_then(|row| row.row_id))
        .and_then(|(oldest, first)| first.checked_sub(oldest))
        .and_then(|omitted| usize::try_from(omitted).ok())
        .map_or_else(
            || available_rows.saturating_sub(rows.len()),
            |omitted| omitted.min(available_rows),
        )
}

fn bound_history_cache(rows: &mut Vec<TerminalRow>, keep_oldest: bool) {
    while rows.len() > MAX_CACHED_HISTORY_ROWS
        || history_cache_bytes(rows) > MAX_CACHED_HISTORY_BYTES
    {
        if rows.is_empty() {
            break;
        }
        if keep_oldest {
            rows.pop();
        } else {
            rows.remove(0);
        }
    }
}

fn bound_history_page_with_pins(
    mut rows: Vec<TerminalRow>,
    pinned_selection_rows: Option<[u64; 2]>,
    visible_rows: &[TerminalRow],
) -> Option<Vec<TerminalRow>> {
    bound_history_cache(&mut rows, true);
    pinned_selection_rows
        .is_none_or(|pins| {
            pins.into_iter().all(|row_id| {
                rows.iter().any(|row| row.row_id == Some(row_id))
                    || visible_rows.iter().any(|row| row.row_id == Some(row_id))
            })
        })
        .then_some(rows)
}

fn terminal_update_changes_visible_content(update: &TerminalUpdate) -> bool {
    !update.rows.is_empty()
        || !update.scrolls.is_empty()
        || update.columns.is_some()
        || update.row_count.is_some()
        || update.palette.is_some()
        || update.default_colors.is_some()
        || update.active_screen.is_some()
        || update.scrollback.is_some()
        || update.images.is_some()
}

fn terminal_update_full_frame_reasons(
    update: &TerminalUpdate,
    current_active_screen: ActiveScreen,
    current_has_images: bool,
) -> u64 {
    u64::from(update.columns.is_some())
        | (u64::from(update.row_count.is_some()) << 1)
        | (u64::from(update.palette.is_some()) << 2)
        | (u64::from(update.default_colors.is_some()) << 3)
        | (u64::from(
            update
                .active_screen
                .is_some_and(|active_screen| active_screen != current_active_screen),
        ) << 4)
        | (u64::from(update.images.is_some()) << 5)
        | (u64::from(current_has_images && !update.scrolls.is_empty()) << 6)
}

#[cfg(test)]
fn terminal_update_requires_full_frame(
    update: &TerminalUpdate,
    current_active_screen: ActiveScreen,
    current_has_images: bool,
) -> bool {
    terminal_update_full_frame_reasons(update, current_active_screen, current_has_images) != 0
}

fn apply_scrollback_update(
    snapshot: &mut TerminalSnapshot,
    scrollback: splinterm_protocol::TerminalScrollbackUpdate,
) -> Result<()> {
    match scrollback.transition {
        HistoryTransition::Append { .. }
            if scrollback.history_generation != snapshot.history_generation =>
        {
            anyhow::bail!("history append changed generation");
        }
        HistoryTransition::Clear | HistoryTransition::Reflow
            if scrollback.history_generation <= snapshot.history_generation =>
        {
            anyhow::bail!("history reset did not change generation");
        }
        _ => {}
    }
    let preserve_cached = scrollback.history_generation == snapshot.history_generation
        && matches!(
            scrollback.transition,
            HistoryTransition::Append { .. } | HistoryTransition::Replace
        );
    let first_returned = scrollback.rows.first().and_then(|row| row.row_id);
    let oldest_available = scrollback.oldest_available_row_id;
    let mut rows = if preserve_cached {
        snapshot
            .scrollback_rows
            .iter()
            .filter(|row| {
                row.row_id
                    .zip(oldest_available)
                    .is_some_and(|(id, oldest)| id >= oldest)
                    && row
                        .row_id
                        .zip(first_returned)
                        .is_some_and(|(id, first)| id < first)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    rows.extend(scrollback.rows);
    bound_history_cache(&mut rows, false);
    snapshot.history_generation = scrollback.history_generation;
    snapshot.oldest_available_scrollback_row_id = scrollback.oldest_available_row_id;
    snapshot.newest_available_scrollback_row_id = scrollback.newest_available_row_id;
    snapshot.scrollback_rows = rows;
    snapshot.available_scrollback_rows = scrollback.available_rows;
    snapshot.omitted_oldest_scrollback_rows = snapshot
        .available_scrollback_rows
        .saturating_sub(snapshot.scrollback_rows.len());
    Ok(())
}

fn apply_terminal_update(snapshot: &mut TerminalSnapshot, update: TerminalUpdate) -> Result<()> {
    update
        .validate_against(
            snapshot.revision,
            snapshot.history_generation,
            snapshot.columns,
            snapshot.rows,
        )
        .map_err(|error| anyhow::anyhow!(error.message))?;
    if let Some(columns) = update.columns {
        if columns == 0 || columns > usize::from(splinterm_protocol::MAX_COLUMNS) {
            anyhow::bail!("terminal update columns exceed protocol limits");
        }
        snapshot.columns = columns;
    }
    if let Some(rows) = update.row_count {
        if rows == 0 || rows > usize::from(splinterm_protocol::MAX_ROWS) {
            anyhow::bail!("terminal update rows exceed protocol limits");
        }
        snapshot.rows = rows;
        snapshot
            .visible_rows
            .resize_with(rows, || blank_row(snapshot.columns));
        snapshot.visible_rows.truncate(rows);
    }
    for patch in update.rows {
        if patch.index >= snapshot.rows || patch.row.cells.len() > snapshot.columns {
            anyhow::bail!("terminal row patch exceeds current dimensions");
        }
        snapshot.visible_rows[patch.index] = patch.row;
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
        if palette.len() != 256 {
            anyhow::bail!("terminal update palette must have 256 entries");
        }
        snapshot.palette = palette;
    }
    if let Some(colors) = update.default_colors {
        snapshot.default_colors = colors;
    }
    if let Some(scrollback) = update.scrollback {
        apply_scrollback_update(snapshot, scrollback)?;
    }
    if let Some(images) = update.images {
        snapshot.images = Some(images);
    }
    snapshot.revision = update.revision;
    Ok(())
}

fn modifier_parameter(modifiers: Modifiers) -> u8 {
    1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.ctrl)
}

fn modified_final(final_byte: u8, modifiers: Modifiers, application: bool) -> Vec<u8> {
    let parameter = modifier_parameter(modifiers);
    if parameter == 1 {
        if application {
            vec![0x1b, b'O', final_byte]
        } else {
            vec![0x1b, b'[', final_byte]
        }
    } else {
        format!("\x1b[1;{parameter}{}", char::from(final_byte)).into_bytes()
    }
}

fn modified_tilde(code: u8, modifiers: Modifiers) -> Vec<u8> {
    let parameter = modifier_parameter(modifiers);
    if parameter == 1 {
        format!("\x1b[{code}~").into_bytes()
    } else {
        format!("\x1b[{code};{parameter}~").into_bytes()
    }
}

fn ctrl_utf8(utf8: &str) -> Option<Vec<u8>> {
    if utf8.len() == 1 && utf8.as_bytes()[0] < 0x20 {
        return Some(utf8.as_bytes().to_vec());
    }
    let character = utf8.chars().next()?;
    if utf8.chars().count() != 1 {
        return None;
    }
    let byte = match character {
        '@' | ' ' | '2' => 0,
        'a'..='z' => u8::try_from(u32::from(character) - u32::from('a') + 1).ok()?,
        'A'..='Z' => u8::try_from(u32::from(character) - u32::from('A') + 1).ok()?,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    };
    Some(vec![byte])
}

fn keypad_input(keysym: Keysym) -> Option<u8> {
    Some(match keysym {
        Keysym::KP_0 => b'p',
        Keysym::KP_1 => b'q',
        Keysym::KP_2 => b'r',
        Keysym::KP_3 => b's',
        Keysym::KP_4 => b't',
        Keysym::KP_5 => b'u',
        Keysym::KP_6 => b'v',
        Keysym::KP_7 => b'w',
        Keysym::KP_8 => b'x',
        Keysym::KP_9 => b'y',
        Keysym::KP_Decimal => b'n',
        Keysym::KP_Divide => b'o',
        Keysym::KP_Multiply => b'j',
        Keysym::KP_Subtract => b'm',
        Keysym::KP_Add => b'k',
        Keysym::KP_Separator => b'l',
        Keysym::KP_Equal => b'X',
        _ => return None,
    })
}

fn key_input(
    keysym: Keysym,
    utf8: Option<&str>,
    modifiers: Modifiers,
    modes: TerminalInputModes,
) -> Option<Vec<u8>> {
    let mut alt_is_encoded = false;
    let mut bytes = match keysym {
        Keysym::Return | Keysym::KP_Enter => vec![b'\r'],
        Keysym::BackSpace => vec![0x7f],
        Keysym::ISO_Left_Tab => b"\x1b[Z".to_vec(),
        Keysym::Tab | Keysym::KP_Tab if modifiers.shift => b"\x1b[Z".to_vec(),
        Keysym::Tab | Keysym::KP_Tab => vec![b'\t'],
        Keysym::Escape => vec![0x1b],
        Keysym::Up => {
            alt_is_encoded = true;
            modified_final(b'A', modifiers, modes.application_cursor)
        }
        Keysym::Down => {
            alt_is_encoded = true;
            modified_final(b'B', modifiers, modes.application_cursor)
        }
        Keysym::Right => {
            alt_is_encoded = true;
            modified_final(b'C', modifiers, modes.application_cursor)
        }
        Keysym::Left => {
            alt_is_encoded = true;
            modified_final(b'D', modifiers, modes.application_cursor)
        }
        Keysym::Home => {
            alt_is_encoded = true;
            modified_final(b'H', modifiers, modes.application_cursor)
        }
        Keysym::End => {
            alt_is_encoded = true;
            modified_final(b'F', modifiers, modes.application_cursor)
        }
        Keysym::Insert => {
            alt_is_encoded = true;
            modified_tilde(2, modifiers)
        }
        Keysym::Delete => {
            alt_is_encoded = true;
            modified_tilde(3, modifiers)
        }
        Keysym::Page_Up => {
            alt_is_encoded = true;
            modified_tilde(5, modifiers)
        }
        Keysym::Page_Down => {
            alt_is_encoded = true;
            modified_tilde(6, modifiers)
        }
        Keysym::F1 | Keysym::F2 | Keysym::F3 | Keysym::F4 => {
            alt_is_encoded = true;
            let final_byte = b'P' + u8::try_from(keysym.raw() - Keysym::F1.raw()).ok()?;
            modified_final(final_byte, modifiers, true)
        }
        Keysym::F5
        | Keysym::F6
        | Keysym::F7
        | Keysym::F8
        | Keysym::F9
        | Keysym::F10
        | Keysym::F11
        | Keysym::F12 => {
            alt_is_encoded = true;
            let code = match keysym {
                Keysym::F5 => 15,
                Keysym::F6 => 17,
                Keysym::F7 => 18,
                Keysym::F8 => 19,
                Keysym::F9 => 20,
                Keysym::F10 => 21,
                Keysym::F11 => 23,
                Keysym::F12 => 24,
                _ => unreachable!(),
            };
            modified_tilde(code, modifiers)
        }
        _ if modes.application_keypad && keypad_input(keysym).is_some() => {
            let final_byte = keypad_input(keysym)?;
            alt_is_encoded = true;
            modified_final(final_byte, modifiers, true)
        }
        _ if modifiers.ctrl => ctrl_utf8(utf8?)?,
        _ => utf8.filter(|text| !text.is_empty())?.as_bytes().to_vec(),
    };
    if modifiers.alt && !alt_is_encoded {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// Encodes clipboard bytes for the terminal's current bracketed-paste mode.
///
/// Clipboard acquisition remains Phase 5 client work; this helper only defines
/// the PTY byte boundary used once a user-authorized paste exists.
#[must_use]
pub fn encode_bracketed_paste(bytes: &[u8], enabled: bool) -> Vec<u8> {
    if !enabled {
        return bytes.to_vec();
    }
    let mut encoded = Vec::with_capacity(bytes.len() + 12);
    encoded.extend_from_slice(b"\x1b[200~");
    encoded.extend_from_slice(bytes);
    encoded.extend_from_slice(b"\x1b[201~");
    encoded
}

fn accepted_text_mime(mimes: &[String]) -> Option<String> {
    TEXT_MIMES.iter().find_map(|supported| {
        mimes
            .iter()
            .find(|mime| mime.as_str() == *supported)
            .cloned()
    })
}

fn safe_paste(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() > MAX_CLIPBOARD_BYTES {
        anyhow::bail!("clipboard offer exceeds the 1 MiB limit");
    }
    std::str::from_utf8(bytes).context("clipboard text is not UTF-8")?;
    if bytes
        .iter()
        .any(|byte| matches!(*byte, 0..=8 | 11..=12 | 14..=31 | 127))
    {
        anyhow::bail!("clipboard text contains unsafe control characters");
    }
    Ok(bytes)
}

fn selection_endpoint(
    snapshot: &TerminalSnapshot,
    position: CellPosition,
) -> Option<SelectionEndpoint> {
    let row_id = snapshot.visible_rows.get(position.row)?.row_id?;
    Some(SelectionEndpoint {
        active_screen: snapshot.active_screen,
        history_generation: snapshot.history_generation,
        row_id,
        column: position.column,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectionRange {
    start_row: usize,
    start_column: usize,
    end_row: usize,
    end_column: usize,
}

fn loaded_row_position(snapshot: &TerminalSnapshot, row_id: u64) -> Option<usize> {
    snapshot
        .scrollback_rows
        .iter()
        .chain(&snapshot.visible_rows)
        .position(|row| row.row_id == Some(row_id))
}

fn selection_range(snapshot: &TerminalSnapshot, selection: Selection) -> Option<SelectionRange> {
    if selection.anchor.active_screen != snapshot.active_screen
        || selection.end.active_screen != snapshot.active_screen
        || selection.anchor.history_generation != snapshot.history_generation
        || selection.end.history_generation != snapshot.history_generation
    {
        return None;
    }
    let anchor_row = loaded_row_position(snapshot, selection.anchor.row_id)?;
    let end_row = loaded_row_position(snapshot, selection.end.row_id)?;
    let anchor = (anchor_row, selection.anchor.column);
    let end = (end_row, selection.end.column);
    let (start, end) = if anchor <= end {
        (anchor, end)
    } else {
        (end, anchor)
    };
    Some(SelectionRange {
        start_row: start.0,
        start_column: start.1,
        end_row: end.0,
        end_column: end.1,
    })
}

fn selection_display_bounds(
    snapshot: &TerminalSnapshot,
    display: &TerminalSnapshot,
    selection: Selection,
) -> Option<(CellPosition, CellPosition)> {
    let range = selection_range(snapshot, selection)?;
    let mut selected = display
        .visible_rows
        .iter()
        .enumerate()
        .filter_map(|(display_row, row)| {
            let loaded_row = loaded_row_position(snapshot, row.row_id?)?;
            (loaded_row >= range.start_row && loaded_row <= range.end_row)
                .then_some((display_row, loaded_row))
        });
    let first = selected.next()?;
    let last = selected.next_back().unwrap_or(first);
    Some((
        CellPosition {
            row: first.0,
            column: if first.1 == range.start_row {
                range.start_column
            } else {
                0
            },
        },
        CellPosition {
            row: last.0,
            column: if last.1 == range.end_row {
                range.end_column
            } else {
                snapshot.columns.saturating_sub(1)
            },
        },
    ))
}

fn selection_is_retained(snapshot: &TerminalSnapshot, selection: Selection) -> bool {
    selection_range(snapshot, selection).is_some()
}

fn selection_text(snapshot: &TerminalSnapshot, selection: Selection) -> Option<String> {
    let range = selection_range(snapshot, selection)?;
    let rows = snapshot
        .scrollback_rows
        .iter()
        .chain(&snapshot.visible_rows);
    let mut output = String::new();
    for (row_index, row) in rows
        .enumerate()
        .skip(range.start_row)
        .take(range.end_row.saturating_sub(range.start_row) + 1)
    {
        let first = if row_index == range.start_row {
            range.start_column
        } else {
            0
        };
        let last = if row_index == range.end_row {
            range.end_column
        } else {
            snapshot.columns.saturating_sub(1)
        };
        let mut line = String::new();
        for cell in row.cells.iter().take(last.saturating_add(1)).skip(first) {
            if cell.spacer_remaining.is_none() {
                line.push_str(&cell.content);
            }
        }
        output.push_str(line.trim_end_matches(' '));
        if row_index != range.end_row {
            output.push('\n');
        }
    }
    Some(output)
}

fn url_at(
    snapshot: &TerminalSnapshot,
    position: CellPosition,
) -> Option<(CellPosition, CellPosition, String)> {
    let row = snapshot.visible_rows.get(position.row)?;
    let mut text = String::new();
    let mut columns = Vec::new();
    for (column, cell) in row.cells.iter().take(snapshot.columns).enumerate() {
        if cell.spacer_remaining.is_some() {
            continue;
        }
        for character in cell.content.chars() {
            columns.push(column);
            text.push(character);
        }
    }
    let byte_at = text
        .char_indices()
        .zip(columns.iter().copied())
        .find_map(|((byte, _), column)| (column == position.column).then_some(byte))?;
    let is_url_char = |character: char| {
        !character.is_whitespace() && !matches!(character, '<' | '>' | '"' | '\'')
    };
    let start = text[..byte_at]
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!is_url_char(character)).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    let end = text[byte_at..]
        .char_indices()
        .find_map(|(index, character)| (!is_url_char(character)).then_some(byte_at + index))
        .unwrap_or(text.len());
    let candidate = text[start..end].trim_end_matches(['.', ',', ')', ']', '}', ';', ':']);
    if !(candidate.starts_with("https://") || candidate.starts_with("http://")) {
        return None;
    }
    let start_char = text[..start].chars().count();
    let end_char = start_char + candidate.chars().count().saturating_sub(1);
    Some((
        CellPosition {
            row: position.row,
            column: *columns.get(start_char)?,
        },
        CellPosition {
            row: position.row,
            column: *columns.get(end_char)?,
        },
        candidate.to_owned(),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseAction {
    Press(u8),
    Release(u8),
    Motion(u8),
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WheelOutcome {
    Noop,
    History { before: usize, after: usize },
    Application { reports: usize, bytes: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryNavigation {
    PageUp,
    PageDown,
    ReturnToLive,
}

fn history_navigation(keysym: Keysym, shift: bool, detached: bool) -> Option<HistoryNavigation> {
    if !shift {
        return None;
    }
    match keysym {
        Keysym::Page_Up => Some(HistoryNavigation::PageUp),
        Keysym::Page_Down => Some(HistoryNavigation::PageDown),
        Keysym::End if detached => Some(HistoryNavigation::ReturnToLive),
        _ => None,
    }
}

fn history_overlay_status(
    viewport: &ScrollbackViewport,
    snapshot: Option<&TerminalSnapshot>,
) -> Option<HistoryOverlayStatus> {
    let snapshot = snapshot?;
    (!viewport.is_live()).then_some(HistoryOverlayStatus {
        offset_from_bottom: viewport.offset_from_bottom().min(999),
        available_rows: snapshot.available_scrollback_rows.min(999),
        unseen_rows: viewport.unseen_rows().min(999),
    })
}

fn history_return_to_live_hit(
    position: (f64, f64),
    logical_width: u32,
    logical_height: u32,
    detached: bool,
) -> bool {
    if !detached || !position.0.is_finite() || !position.1.is_finite() {
        return false;
    }
    let Some(layout) = history_overlay_layout(logical_width, logical_height, 120) else {
        return false;
    };
    let (x, y, width, height) = layout.return_to_live;
    position.0 >= f64::from(x)
        && position.1 >= f64::from(y)
        && position.0 < f64::from(x) + f64::from(width)
        && position.1 < f64::from(y) + f64::from(height)
}

fn mouse_button_code(button: u32) -> Option<u8> {
    match button {
        BTN_LEFT => Some(0),
        BTN_MIDDLE => Some(1),
        BTN_RIGHT => Some(2),
        _ => None,
    }
}

fn classify_press(
    button: u32,
    has_position: bool,
    modifiers: Modifiers,
    modes: TerminalInputModes,
    has_hovered_url: bool,
) -> PressOwner {
    if button == BTN_MIDDLE {
        return PressOwner::PrimaryPaste;
    }
    if button == BTN_LEFT && modifiers.ctrl && has_hovered_url {
        return PressOwner::Url;
    }
    if has_position && modes.mouse_tracking != MouseTracking::None && !modifiers.shift {
        return mouse_button_code(button).map_or(PressOwner::Ignored, |code| {
            PressOwner::Application {
                code,
                tracking: modes.mouse_tracking,
                sgr: modes.sgr_mouse,
                modifiers,
            }
        });
    }
    if button == BTN_LEFT && has_position {
        PressOwner::Selection
    } else {
        PressOwner::Ignored
    }
}

fn take_press_owner(pressed: &mut HashMap<u32, PressOwner>, button: u32) -> PressOwner {
    pressed.remove(&button).unwrap_or(PressOwner::Ignored)
}

fn application_motion(owner: &PressOwner) -> Option<(u8, bool, Modifiers)> {
    if let PressOwner::Application {
        code,
        tracking: MouseTracking::Button | MouseTracking::Any,
        sgr,
        modifiers,
    } = owner
    {
        Some((*code, *sgr, *modifiers))
    } else {
        None
    }
}

impl WheelAccumulator {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "Wayland axis values are finite and converted to a whole-line count"
    )]
    fn push(
        &mut self,
        absolute: f64,
        discrete: i32,
        value120: i32,
        cell_height: u32,
    ) -> Option<(MouseAction, usize)> {
        self.push_scaled(absolute, discrete, value120, 1.0, cell_height)
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "Wayland axis values are finite and converted to a whole-line count"
    )]
    fn push_scaled(
        &mut self,
        absolute: f64,
        discrete: i32,
        value120: i32,
        multiplier: f64,
        cell_height: u32,
    ) -> Option<(MouseAction, usize)> {
        let multiplier = if multiplier.is_finite() && multiplier > 0.0 {
            multiplier
        } else {
            1.0
        };
        let (unit, delta, threshold) = if value120 != 0 {
            (
                WheelUnit::Value120,
                f64::from(value120),
                WHEEL_VALUE120_STEP / multiplier,
            )
        } else if discrete != 0 {
            (WheelUnit::Discrete, f64::from(discrete) * multiplier, 1.0)
        } else if absolute != 0.0 && absolute.is_finite() && cell_height > 0 {
            (
                WheelUnit::Pixel,
                absolute,
                f64::from(cell_height) / multiplier,
            )
        } else {
            return None;
        };
        if self.unit != Some(unit) {
            self.unit = Some(unit);
            self.remainder = 0.0;
        }
        self.remainder += delta;
        let count = (self.remainder.abs() / threshold).floor() as usize;
        if count == 0 {
            return None;
        }
        let direction = self.remainder.signum();
        self.remainder -= direction * threshold * count as f64;
        Some((
            if direction.is_sign_negative() {
                MouseAction::WheelUp
            } else {
                MouseAction::WheelDown
            },
            count,
        ))
    }
}

fn mouse_report(
    action: MouseAction,
    position: CellPosition,
    modifiers: Modifiers,
    sgr: bool,
) -> Option<Vec<u8>> {
    let modifier =
        4 * u8::from(modifiers.shift) + 8 * u8::from(modifiers.alt) + 16 * u8::from(modifiers.ctrl);
    let (base, release) = match action {
        MouseAction::Press(button) => (button, false),
        MouseAction::Release(button) => (button, true),
        MouseAction::Motion(button) => (button.saturating_add(32), false),
        MouseAction::WheelUp => (64, false),
        MouseAction::WheelDown => (65, false),
    };
    let code = base.saturating_add(modifier);
    let column = position.column.saturating_add(1);
    let row = position.row.saturating_add(1);
    if sgr {
        Some(
            format!(
                "\x1b[<{code};{column};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes(),
        )
    } else {
        let legacy_code = if release {
            3_u8.saturating_add(modifier)
        } else {
            code
        };
        let column = u8::try_from(column.saturating_add(32)).ok()?;
        let row = u8::try_from(row.saturating_add(32)).ok()?;
        Some(vec![
            0x1b,
            b'[',
            b'M',
            legacy_code.saturating_add(32),
            column,
            row,
        ])
    }
}

fn try_window_command(commands: &Sender<WindowCommand>, command: WindowCommand) -> Result<()> {
    commands.try_send(command).map_err(|error| match error {
        TrySendError::Full(_) => anyhow::anyhow!("Wayland command queue overflow"),
        TrySendError::Closed(_) => anyhow::anyhow!("Wayland command receiver disconnected"),
    })
}

fn apply_ime_preedit(snapshot: &mut TerminalSnapshot, text: Option<&str>) -> Option<usize> {
    let row = usize::try_from(snapshot.cursor_row).ok()?;
    if row >= snapshot.rows {
        return None;
    }
    if let Some(text) = text {
        let mut column = usize::try_from(snapshot.cursor_column.max(0)).unwrap_or(0);
        let mut leader: Option<usize> = None;
        for character in text.chars() {
            let width = UnicodeWidthChar::width(character).unwrap_or(1).min(2);
            if width == 0 {
                if let Some(leader) = leader {
                    if let Some(cell) = snapshot.visible_rows[row].cells.get_mut(leader) {
                        cell.content.push(character);
                    }
                }
                continue;
            }
            if column >= snapshot.columns || column + width > snapshot.columns {
                break;
            }
            if let Some(cell) = snapshot.visible_rows[row].cells.get_mut(column) {
                cell.content = character.to_string();
                cell.spacer_remaining = None;
            }
            leader = Some(column);
            if width == 2 {
                if let Some(spacer) = snapshot.visible_rows[row].cells.get_mut(column + 1) {
                    spacer.content.clear();
                    spacer.spacer_remaining = Some(1);
                }
            }
            column += width;
        }
    }
    Some(row)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaneTopologyAction {
    Split(splinterm_core::Axis),
    Close,
    AdjustRatio(i16),
}

fn pane_topology_action(keysym: Keysym, modifiers: Modifiers) -> Option<PaneTopologyAction> {
    if !modifiers.ctrl || !modifiers.shift || modifiers.alt || modifiers.logo {
        return None;
    }
    match keysym {
        Keysym::Return | Keysym::KP_Enter => {
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Horizontal))
        }
        Keysym::backslash | Keysym::bar => {
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Vertical))
        }
        Keysym::w | Keysym::W => Some(PaneTopologyAction::Close),
        Keysym::bracketleft | Keysym::braceleft => Some(PaneTopologyAction::AdjustRatio(-50)),
        Keysym::bracketright | Keysym::braceright => Some(PaneTopologyAction::AdjustRatio(50)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaneFocusAction {
    Direction(FocusDirection),
    Next { reverse: bool },
}

fn pane_focus_action(keysym: Keysym, modifiers: Modifiers) -> Option<PaneFocusAction> {
    if !modifiers.ctrl || !modifiers.shift || modifiers.alt || modifiers.logo {
        return None;
    }
    match keysym {
        Keysym::Left => Some(PaneFocusAction::Direction(FocusDirection::Left)),
        Keysym::Right => Some(PaneFocusAction::Direction(FocusDirection::Right)),
        Keysym::Up => Some(PaneFocusAction::Direction(FocusDirection::Up)),
        Keysym::Down => Some(PaneFocusAction::Direction(FocusDirection::Down)),
        Keysym::Tab => Some(PaneFocusAction::Next { reverse: false }),
        Keysym::ISO_Left_Tab => Some(PaneFocusAction::Next { reverse: true }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FontZoomAction {
    Increase,
    Decrease,
    Reset,
}

fn font_zoom_action(keysym: Keysym, modifiers: Modifiers) -> Option<FontZoomAction> {
    if !modifiers.ctrl || modifiers.alt || modifiers.logo {
        return None;
    }
    match keysym {
        Keysym::plus | Keysym::equal | Keysym::KP_Add => Some(FontZoomAction::Increase),
        Keysym::minus | Keysym::KP_Subtract => Some(FontZoomAction::Decrease),
        Keysym::_0 | Keysym::KP_0 => Some(FontZoomAction::Reset),
        _ => None,
    }
}

fn cursor_blink_enabled(reduced_motion: bool, focused: bool, modes: TerminalInputModes) -> bool {
    !reduced_motion && focused && modes.cursor_visible && modes.cursor_blink
}

fn offset_cursor_rectangle(
    cursor: (i32, i32, i32, i32),
    pane: Rect,
) -> Option<(i32, i32, i32, i32)> {
    Some((
        cursor.0.checked_add(i32::try_from(pane.x).ok()?)?,
        cursor.1.checked_add(i32::try_from(pane.y).ok()?)?,
        cursor.2,
        cursor.3,
    ))
}

fn resize_changed(previous: Option<(u16, u16, u16, u16)>, candidate: (u16, u16, u16, u16)) -> bool {
    previous != Some(candidate)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalResizeCause {
    SnapshotAvailable,
    SurfaceConfigure,
    OutputDpiChanged,
    CompositorScaleChanged,
}

const fn terminal_resize_allowed(cause: TerminalResizeCause, resize_known: bool) -> bool {
    match cause {
        TerminalResizeCause::SnapshotAvailable => !resize_known,
        TerminalResizeCause::SurfaceConfigure => true,
        TerminalResizeCause::OutputDpiChanged | TerminalResizeCause::CompositorScaleChanged => {
            false
        }
    }
}

fn reduced_motion_requested() -> bool {
    std::env::var_os("SPLINTERM_REDUCED_MOTION")
        .is_some_and(|value| matches!(value.to_str(), Some("1" | "true" | "yes")))
}

fn window_title(
    snapshot_title: Option<&str>,
    controller_active: bool,
    authority: &AuthorityStatus,
    control_transfer_pending: bool,
    search: Option<&SearchUiState>,
) -> String {
    let base = snapshot_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Splinterm");
    let controller = controller_active.then_some("local controller");
    let authority_label = if authority.development_bypass {
        Some("DEVELOPMENT BYPASS")
    } else if authority.grants.is_empty() {
        None
    } else {
        Some("EXTERNAL ACCESS ACTIVE")
    };
    let title = match (controller, authority_label) {
        (Some(controller), Some(authority)) => format!("{base} — {controller} — {authority}"),
        (Some(controller), None) => format!("{base} — {controller}"),
        (None, Some(authority)) => format!("{base} — {authority}"),
        (None, None) => base.to_owned(),
    };
    let title = if control_transfer_pending {
        format!("{title} — CONTROL REQUEST: Ctrl+Shift+Y accept / Ctrl+Shift+N deny")
    } else {
        title
    };
    if let Some(search) = search.filter(|search| search.input.is_some()) {
        let query = search.input.as_deref().unwrap_or_default();
        let query = query
            .chars()
            .filter(|ch| !ch.is_control())
            .take(64)
            .collect::<String>();
        format!(
            "{title} — SEARCH: {query} [{} match(es), Ctrl+N/P]",
            search.matches.len()
        )
    } else {
        title
    }
}

fn terminal_draw_waits_for_frame(frame_pending: bool, buffer_available: bool) -> bool {
    frame_pending && !buffer_available
}

fn pending_draw_waits_for_frame(frame_pending: bool, terminal_priority: bool) -> bool {
    frame_pending && !terminal_priority
}

fn take_full_surface_damage(full_redraw: &mut bool, snapshot_frame_present: bool) -> bool {
    let damage_full_surface = !snapshot_frame_present || *full_redraw;
    *full_redraw = false;
    damage_full_surface
}

const fn deterministic_capture_ready(
    scale_ready: bool,
    minimum_images: usize,
    available_images: usize,
) -> bool {
    scale_ready && available_images >= minimum_images
}

fn capture_minimum_images() -> Result<usize> {
    let explicit = std::env::var("SPLINTERM_CAPTURE_MIN_IMAGES")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()
        .context("SPLINTERM_CAPTURE_MIN_IMAGES must be a nonnegative integer")?;
    Ok(explicit.unwrap_or_else(|| {
        usize::from(std::env::var_os("SPLINTERM_CAPTURE_REQUIRE_IMAGE").is_some())
    }))
}

fn try_clipboard_worker(active: &AtomicUsize) -> Option<ClipboardWorkerPermit<'_>> {
    active
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_CLIPBOARD_WORKERS).then_some(count + 1)
        })
        .ok()
        .map(|_| ClipboardWorkerPermit { active })
}

fn poll_timeout(deadline: Instant) -> Option<Timespec> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    Some(Timespec {
        tv_sec: i64::try_from(remaining.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(remaining.subsec_nanos()),
    })
}

fn wait_for_fd(fd: &impl AsFd, events: PollFlags, deadline: Instant) -> io::Result<bool> {
    let Some(timeout) = poll_timeout(deadline) else {
        return Ok(false);
    };
    let mut descriptor = [PollFd::new(fd, events)];
    let ready = poll(&mut descriptor, Some(&timeout)).map_err(io::Error::from)?;
    if ready == 0 {
        return Ok(false);
    }
    let returned = descriptor[0].revents();
    if returned.intersects(PollFlags::ERR | PollFlags::NVAL) {
        return Err(io::Error::other("clipboard pipe reported an I/O error"));
    }
    Ok(returned.intersects(events | PollFlags::HUP))
}

fn read_clipboard_with_deadline(fd: &OwnedFd, timeout: Duration) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        if !wait_for_fd(fd, PollFlags::IN, deadline)? {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "clipboard read timed out",
            ));
        }
        let remaining = MAX_CLIPBOARD_BYTES
            .saturating_add(1)
            .saturating_sub(bytes.len());
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard offer exceeds byte limit",
            ));
        }
        let chunk_len = remaining.min(chunk.len());
        let read = rustix::io::read(fd, &mut chunk[..chunk_len]).map_err(io::Error::from)?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > MAX_CLIPBOARD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard offer exceeds byte limit",
            ));
        }
    }
}

fn write_clipboard_with_deadline(
    fd: &OwnedFd,
    payload: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut written = 0;
    while written < payload.len() {
        if !wait_for_fd(fd, PollFlags::OUT, deadline)? {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "clipboard write timed out",
            ));
        }
        let end = (written + 4096).min(payload.len());
        let count = rustix::io::write(fd, &payload[written..end]).map_err(io::Error::from)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "clipboard pipe accepted no bytes",
            ));
        }
        written += count;
    }
    Ok(())
}

fn spawn_clipboard_read(fd: OwnedFd, target: PasteTarget, tx: StdSender<ClipboardRead>) {
    let Some(permit) = try_clipboard_worker(&ACTIVE_CLIPBOARD_WORKERS) else {
        let _ = tx.send(ClipboardRead {
            target,
            bytes: Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "clipboard worker limit reached",
            )),
        });
        return;
    };
    std::thread::spawn(move || {
        let _permit = permit;
        let bytes = read_clipboard_with_deadline(&fd, CLIPBOARD_IO_TIMEOUT);
        let _ = tx.send(ClipboardRead { target, bytes });
    });
}

fn viewport_cursor_row(cursor_row: i32, offset: usize, rows: usize) -> Option<i32> {
    if cursor_row < 0 {
        return None;
    }
    i32::try_from(offset)
        .ok()
        .and_then(|offset| cursor_row.checked_add(offset))
        .filter(|row| *row >= 0 && usize::try_from(*row).is_ok_and(|row| row < rows))
}

impl App {
    fn focused_splint(&self) -> Option<SplintId> {
        self.pane
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.splint_id)
    }

    fn splint_at_point(&self, point: (f64, f64)) -> Option<SplintId> {
        let layout = self.computed_pane_layout().ok().flatten()?;
        layout.panes.into_iter().find_map(|pane| {
            let right = pane.rect.x.checked_add(pane.rect.width)?;
            let bottom = pane.rect.y.checked_add(pane.rect.height)?;
            (point.0 >= f64::from(pane.rect.x)
                && point.0 < f64::from(right)
                && point.1 >= f64::from(pane.rect.y)
                && point.1 < f64::from(bottom))
            .then_some(pane.splint_id)
        })
    }

    fn focus_splint(&mut self, splint_id: SplintId) -> bool {
        if self.focused_splint() == Some(splint_id) {
            return false;
        }
        let Some(index) = self.inactive_panes.iter().position(|pane| {
            pane.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.splint_id == splint_id)
        }) else {
            return false;
        };
        std::mem::swap(&mut self.pane, &mut self.inactive_panes[index]);
        self.pane.pointer_cell = None;
        self.pane.hovered_url = None;
        self.full_redraw = true;
        true
    }

    fn focus_direction(&mut self, direction: FocusDirection) -> bool {
        let (Some(layout), Some(current)) = (&self.layout, self.focused_splint()) else {
            return false;
        };
        let Ok(layout) = PaneLayout::compute(
            layout,
            Rect {
                x: 0,
                y: 0,
                width: 1_000_000,
                height: 1_000_000,
            },
            1,
            1,
            1,
        ) else {
            return false;
        };
        layout
            .directional(current, direction)
            .is_some_and(|next| self.focus_splint(next))
    }

    fn focus_next(&mut self, reverse: bool) -> bool {
        let (Some(layout), Some(current)) = (&self.layout, self.focused_splint()) else {
            return false;
        };
        let Ok(layout) = PaneLayout::compute(
            layout,
            Rect {
                x: 0,
                y: 0,
                width: 1_000_000,
                height: 1_000_000,
            },
            1,
            1,
            1,
        ) else {
            return false;
        };
        layout
            .next(current, reverse)
            .is_some_and(|next| self.focus_splint(next))
    }

    fn prepare_frame_titles(
        &mut self,
        pane_layout: Option<&PaneLayout>,
        cell_width: u32,
    ) -> Result<()> {
        if self.pane_divider_style != PaneDividerStyle::Frame
            || self.frame_title_mode != FrameTitleMode::Splint
        {
            self.frame_titles.clear();
            return Ok(());
        }
        let (Some(pane_layout), Some(topology)) = (pane_layout, self.layout.as_ref()) else {
            self.frame_titles.clear();
            return Ok(());
        };
        let mut requested = Vec::new();
        for pane in &pane_layout.panes {
            let allocation = Self::buffer_rect(pane.allocation, self.scale_120)?;
            let columns = allocation.width / cell_width.max(1);
            let Some(maximum_cells) = columns.checked_sub(6).filter(|cells| *cells > 0) else {
                continue;
            };
            let Some(splint) = topology.find_splint(pane.splint_id) else {
                continue;
            };
            let title = sanitize_frame_title(&splint.title, maximum_cells);
            if !title.is_empty() {
                requested.push((pane.splint_id, title, maximum_cells));
            }
        }
        let requested_ids = requested
            .iter()
            .map(|(splint_id, _, _)| *splint_id)
            .collect::<HashSet<_>>();
        self.frame_titles
            .retain(|splint_id, _| requested_ids.contains(splint_id));
        for (splint_id, source, maximum_cells) in requested {
            let current = self.frame_titles.get(&splint_id).is_some_and(|cached| {
                cached.source == source
                    && cached.maximum_cells == maximum_cells
                    && cached.scale_120 == self.scale_120
            });
            if !current {
                self.frame_titles.insert(
                    splint_id,
                    CachedFrameTitle {
                        text: ChromeText::load(&source, self.scale_120)?,
                        source,
                        maximum_cells,
                        scale_120: self.scale_120,
                    },
                );
            }
        }
        Ok(())
    }

    fn computed_pane_layout(&self) -> Result<Option<PaneLayout>> {
        self.layout
            .as_ref()
            .map(|layout| {
                let frame = self
                    .pane
                    .snapshot_frame
                    .as_ref()
                    .context("multi-pane layout requires an active snapshot frame")?;
                let cell_width = buffer_to_logical_ceil(frame.cell_width(), self.scale_120)?;
                let cell_height = buffer_to_logical_ceil(frame.cell_height(), self.scale_120)?;
                let chrome = match self.pane_divider_style {
                    PaneDividerStyle::None => PaneChrome::None,
                    PaneDividerStyle::Line => PaneChrome::Line {
                        vertical_width: cell_width,
                        horizontal_height: cell_height,
                    },
                    PaneDividerStyle::Frame => PaneChrome::Frame {
                        vertical_width: cell_width,
                        horizontal_height: cell_height,
                    },
                };
                let minimum_width = cell_width
                    .checked_mul(2)
                    .context("minimum pane width overflow")?;
                let minimum_height = cell_height
                    .checked_mul(2)
                    .context("minimum pane height overflow")?;
                PaneLayout::compute_with_chrome(
                    layout,
                    Rect {
                        x: 0,
                        y: 0,
                        width: self.logical_width,
                        height: self.logical_height,
                    },
                    chrome,
                    minimum_width,
                    minimum_height,
                )
            })
            .transpose()
    }

    fn buffer_rect(rect: Rect, scale_120: u32) -> Result<Rect> {
        let right = rect
            .x
            .checked_add(rect.width)
            .context("pane right overflow")?;
        let bottom = rect
            .y
            .checked_add(rect.height)
            .context("pane bottom overflow")?;
        let x = logical_extent_to_buffer(rect.x, scale_120)?;
        let y = logical_extent_to_buffer(rect.y, scale_120)?;
        Ok(Rect {
            x,
            y,
            width: logical_extent_to_buffer(right, scale_120)?.saturating_sub(x),
            height: logical_extent_to_buffer(bottom, scale_120)?.saturating_sub(y),
        })
    }

    fn pane_geometry(
        pane: &PaneView,
        rect: Rect,
        scale_120: u32,
    ) -> Result<Option<WindowGeometry>> {
        let Some(frame) = pane.snapshot_frame.as_ref() else {
            return Ok(None);
        };
        let geometry = frame.window_geometry(rect.width, rect.height, scale_120)?;
        Ok(Some(geometry.translated(
            logical_extent_to_buffer(rect.x, scale_120)?,
            logical_extent_to_buffer(rect.y, scale_120)?,
        )?))
    }

    fn display_snapshot(&self) -> Option<TerminalSnapshot> {
        self.pane.display_snapshot()
    }

    fn request_older_history(&mut self) -> Result<()> {
        if self.pane.history_page_pending || self.pane.history_selection_pin_blocked {
            return Ok(());
        }
        let Some(snapshot) = self.pane.snapshot.as_ref() else {
            return Ok(());
        };
        if snapshot.omitted_oldest_scrollback_rows == 0 {
            return Ok(());
        }
        let Some(before_row_id) = snapshot.scrollback_rows.first().and_then(|row| row.row_id)
        else {
            return Ok(());
        };
        let Some(commands) = self.pane.commands.as_ref() else {
            return Ok(());
        };
        try_window_command(
            commands,
            WindowCommand::FetchScrollback {
                splint_id: snapshot.splint_id,
                incarnation: snapshot.incarnation,
                terminal_revision: snapshot.revision,
                history_generation: snapshot.history_generation,
                before_row_id,
            },
        )?;
        self.pane.history_page_pending = true;
        Ok(())
    }

    fn scroll_history(&mut self, action: MouseAction, lines: usize) -> Result<bool> {
        let snapshot = self
            .pane
            .snapshot
            .as_ref()
            .context("scroll requires snapshot")?;
        let previous_offset = self.pane.scrollback_viewport.offset_from_bottom();
        match action {
            MouseAction::WheelUp => self.pane.scrollback_viewport.scroll_up(lines, snapshot),
            MouseAction::WheelDown => self.pane.scrollback_viewport.scroll_down(lines, snapshot),
            _ => return Ok(false),
        }
        let moved = self.pane.scrollback_viewport.offset_from_bottom() != previous_offset;
        if action == MouseAction::WheelUp {
            let loaded = snapshot.scrollback_rows.len();
            let remaining =
                loaded.saturating_sub(self.pane.scrollback_viewport.offset_from_bottom());
            let prefetch_distance = snapshot.rows.saturating_mul(2).max(32);
            if remaining <= prefetch_distance {
                self.request_older_history()?;
            }
        }
        if !moved {
            return Ok(false);
        }
        self.pane.scroll_started_at.get_or_insert_with(Instant::now);
        self.invalidate_viewport_local_state();
        self.refresh_ime_preedit()?;
        self.update_ime_cursor_rectangle();
        // Coalesce high-resolution wheel events until the next compositor frame.
        // Re-shaping the entire viewport synchronously for every axis event made
        // fast scrolling stall the Wayland dispatch loop.
        self.pane.viewport_dirty = true;
        Ok(true)
    }

    fn tick_signoff(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let Some(mut probe) = self.signoff.take() else {
            return Ok(());
        };
        let result = self.advance_signoff(&mut probe);
        if let Err(error) = &result {
            self.write_signoff_report(&probe, false, Some(&error.to_string()))?;
        } else {
            self.write_signoff_report(&probe, probe.step == SignoffStep::Complete, None)?;
        }
        self.signoff = Some(probe);
        result?;
        if self.configured && self.pane.viewport_dirty {
            self.schedule_draw(queue_handle)?;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the development-only sign-off state machine is one auditable scenario"
    )]
    fn advance_signoff(&mut self, probe: &mut SignoffProbe) -> Result<()> {
        anyhow::ensure!(
            probe.started_at.elapsed() < Duration::from_secs(60),
            "sign-off probe timed out at {:?}",
            probe.step
        );
        let Some(snapshot) = self.pane.snapshot.as_ref() else {
            return Ok(());
        };
        match probe.step {
            SignoffStep::WaitHistory => {
                if snapshot.available_scrollback_rows >= 5_000 {
                    probe.step = SignoffStep::LoadSelectionWindow;
                }
            }
            SignoffStep::LoadSelectionWindow => {
                self.scroll_history(MouseAction::WheelUp, usize::MAX)?;
                if self
                    .pane
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.scrollback_rows.len() >= 640)
                {
                    probe.step = SignoffStep::BeginSelection;
                }
            }
            SignoffStep::LoadClientCache => {
                self.scroll_history(MouseAction::WheelUp, usize::MAX)?;
                let snapshot = self.pane.snapshot.as_ref().context("sign-off snapshot")?;
                let cache_bytes = history_cache_bytes(&snapshot.scrollback_rows);
                let loaded_rows = snapshot.scrollback_rows.len();
                let first_row_id = snapshot.scrollback_rows.first().and_then(|row| row.row_id);
                let bounded_eviction_observed = probe.cache_window.zip(first_row_id).is_some_and(
                    |((previous_rows, previous_first), current_first)| {
                        loaded_rows <= previous_rows && current_first < previous_first
                    },
                );
                if let Some(first_row_id) = first_row_id {
                    probe.cache_window = Some((loaded_rows, first_row_id));
                }
                let row_capacity_hit = loaded_rows >= MAX_CACHED_HISTORY_ROWS;
                let byte_capacity_hit = cache_bytes >= MAX_CACHED_HISTORY_BYTES;
                if (row_capacity_hit || byte_capacity_hit || bounded_eviction_observed)
                    && self.pane.scrollback_viewport.offset_from_bottom()
                        >= snapshot.scrollback_rows.len().saturating_sub(snapshot.rows)
                {
                    anyhow::ensure!(
                        snapshot.scrollback_rows.len() <= MAX_CACHED_HISTORY_ROWS
                            && cache_bytes <= MAX_CACHED_HISTORY_BYTES,
                        "client history cache exceeded a bound"
                    );
                    probe.evidence.push(serde_json::json!({
                        "check": "client_history_cache",
                        "loaded_rows": loaded_rows,
                        "loaded_bytes": cache_bytes,
                        "available_rows": snapshot.available_scrollback_rows,
                        "omitted_oldest_rows": snapshot.omitted_oldest_scrollback_rows,
                        "offset": self.pane.scrollback_viewport.offset_from_bottom(),
                        "row_capacity_hit": row_capacity_hit,
                        "byte_capacity_hit": byte_capacity_hit,
                        "bounded_eviction_observed": bounded_eviction_observed,
                        "bounded": true,
                    }));
                    probe.step = SignoffStep::WaitMouseTracking;
                }
            }
            SignoffStep::BeginSelection => {
                if self.begin_selection(CellPosition { row: 0, column: 0 }) {
                    probe.selection_revision = self
                        .pane
                        .snapshot
                        .as_ref()
                        .context("selection snapshot")?
                        .revision;
                    probe.step = SignoffStep::ExtendSelection;
                }
            }
            SignoffStep::ExtendSelection => {
                self.scroll_history(MouseAction::WheelDown, 512)?;
                let row = self
                    .display_snapshot()
                    .context("selection display")?
                    .rows
                    .saturating_sub(1);
                anyhow::ensure!(
                    self.extend_selection(CellPosition { row, column: 8 }),
                    "could not extend cross-page selection"
                );
                let selection = self.pane.selection.context("selection exists")?;
                let row_span = selection.anchor.row_id.abs_diff(selection.end.row_id);
                anyhow::ensure!(row_span > 256, "selection did not cross a history page");
                probe.evidence.push(serde_json::json!({
                    "check": "cross_page_selection",
                    "anchor_row_id": selection.anchor.row_id,
                    "end_row_id": selection.end.row_id,
                    "row_id_span": row_span,
                }));
                probe.step = SignoffStep::WaitSelectedOutput;
            }
            SignoffStep::WaitSelectedOutput => {
                if snapshot.revision >= probe.selection_revision.saturating_add(20) {
                    let selection = self.pane.selection.context("selection survived output")?;
                    anyhow::ensure!(
                        selection_is_retained(snapshot, selection),
                        "selection endpoints were not retained during output"
                    );
                    probe.evidence.push(serde_json::json!({
                        "check": "selection_during_detached_output",
                        "revision_before": probe.selection_revision,
                        "revision_after": snapshot.revision,
                        "unseen_rows": self.pane.scrollback_viewport.unseen_rows(),
                        "retained": true,
                    }));
                    probe.step = SignoffStep::FinishSelection;
                }
            }
            SignoffStep::FinishSelection => {
                let copied_bytes = self.finish_selection().map_or(0, <[u8]>::len);
                anyhow::ensure!(copied_bytes > 0, "cross-page copy was empty");
                probe.evidence.push(serde_json::json!({
                    "check": "cross_page_copy",
                    "copied_bytes": copied_bytes,
                    "content_recorded": false,
                }));
                probe.step = SignoffStep::LocalWheel;
            }
            SignoffStep::LocalWheel => {
                let outcome = self.handle_vertical_wheel(None, 0.0, 1, 0)?;
                let WheelOutcome::History { before, after } = outcome else {
                    anyhow::bail!("wheel without mouse tracking did not scroll history");
                };
                anyhow::ensure!(
                    after < before,
                    "local wheel did not move toward live output"
                );
                probe.evidence.push(serde_json::json!({
                    "check": "local_history_wheel",
                    "before": before,
                    "after": after,
                }));
                self.dirty_selection(self.pane.selection);
                self.pane.selection = None;
                self.pane.selected_text = None;
                self.pane.history_selection_pin_blocked = false;
                probe.step = SignoffStep::LoadClientCache;
            }
            SignoffStep::WaitMouseTracking => {
                if snapshot.input_modes.mouse_tracking != MouseTracking::None {
                    probe.step = SignoffStep::ApplicationWheel;
                }
            }
            SignoffStep::ApplicationWheel => {
                let tracking = snapshot.input_modes.mouse_tracking;
                let sgr = snapshot.input_modes.sgr_mouse;
                let outcome = self.handle_vertical_wheel(
                    Some(CellPosition { row: 2, column: 2 }),
                    0.0,
                    0,
                    -120,
                )?;
                let WheelOutcome::Application { reports, bytes } = outcome else {
                    anyhow::bail!("tracked wheel did not emit an application report");
                };
                probe.evidence.push(serde_json::json!({
                    "check": "application_mouse_wheel",
                    "tracking": format!("{tracking:?}"),
                    "sgr": sgr,
                    "reports": reports,
                    "bytes": bytes,
                    "content_recorded": false,
                }));
                probe.step = SignoffStep::ReturnLive;
            }
            SignoffStep::ReturnLive => {
                self.scroll_history(MouseAction::WheelDown, usize::MAX)?;
                if self.pane.scrollback_viewport.is_live() {
                    probe.evidence.push(serde_json::json!({
                        "check": "return_to_live",
                        "offset": 0,
                    }));
                    probe.step = SignoffStep::Complete;
                }
            }
            SignoffStep::Complete => {}
        }
        Ok(())
    }

    fn write_signoff_report(
        &self,
        probe: &SignoffProbe,
        exact: bool,
        error: Option<&str>,
    ) -> Result<()> {
        let snapshot = self.pane.snapshot.as_ref();
        let report = serde_json::json!({
            "schema": "splinterm.signoff.interactions.v1",
            "exact": exact,
            "step": format!("{:?}", probe.step),
            "elapsed_ms": probe.started_at.elapsed().as_millis(),
            "error": error,
            "state": {
                "revision": snapshot.map(|value| value.revision),
                "available_history_rows": snapshot.map(|value| value.available_scrollback_rows),
                "loaded_history_rows": snapshot.map(|value| value.scrollback_rows.len()),
                "loaded_history_bytes": snapshot.map(|value| history_cache_bytes(&value.scrollback_rows)),
                "first_loaded_row_id": snapshot.and_then(|value| value.scrollback_rows.first()).and_then(|row| row.row_id),
                "omitted_oldest_rows": snapshot.map(|value| value.omitted_oldest_scrollback_rows),
                "history_page_pending": self.pane.history_page_pending,
                "viewport_offset": self.pane.scrollback_viewport.offset_from_bottom(),
                "mouse_tracking": snapshot.map(|value| format!("{:?}", value.input_modes.mouse_tracking)),
                "selection_active": self.pane.selection.is_some(),
            },
            "evidence": probe.evidence,
        });
        let temporary = probe.report_path.with_extension("tmp");
        let mut bytes = serde_json::to_vec_pretty(&report)?;
        bytes.push(b'\n');
        std::fs::write(&temporary, bytes).context("write sign-off report")?;
        std::fs::rename(&temporary, &probe.report_path).context("publish sign-off report")
    }

    fn handle_history_key(
        &mut self,
        event: &KeyEvent,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<bool> {
        let Some(navigation) = history_navigation(
            event.keysym,
            self.modifiers.shift,
            !self.pane.scrollback_viewport.is_live(),
        ) else {
            return Ok(false);
        };
        let page = self
            .pane
            .snapshot
            .as_ref()
            .map_or(1, |snapshot| snapshot.rows.saturating_sub(1).max(1));
        match navigation {
            HistoryNavigation::PageUp => {
                if self.scroll_history(MouseAction::WheelUp, page)? && self.configured {
                    self.schedule_draw(queue_handle)?;
                }
            }
            HistoryNavigation::PageDown => {
                if self.scroll_history(MouseAction::WheelDown, page)? && self.configured {
                    self.schedule_draw(queue_handle)?;
                }
            }
            HistoryNavigation::ReturnToLive => {
                if self.scroll_history(MouseAction::WheelDown, usize::MAX)? && self.configured {
                    self.schedule_draw(queue_handle)?;
                }
            }
        }
        Ok(true)
    }

    fn send_input(&mut self, bytes: Vec<u8>) -> Result<()> {
        if let Some(commands) = &self.pane.commands {
            try_window_command(commands, WindowCommand::Input(bytes))?;
        }
        Ok(())
    }

    fn handle_vertical_wheel(
        &mut self,
        position: Option<CellPosition>,
        absolute: f64,
        discrete: i32,
        value120: i32,
    ) -> Result<WheelOutcome> {
        let modes = self.input_modes();
        let cell_height = self
            .pane
            .snapshot_frame
            .as_ref()
            .map_or(1, SnapshotFrame::cell_height);
        if modes.mouse_tracking == MouseTracking::None {
            let before = self.pane.scrollback_viewport.offset_from_bottom();
            let Some((action, count)) = self.scrollback_wheel.push_scaled(
                absolute,
                discrete,
                value120,
                SCROLLBACK_WHEEL_MULTIPLIER,
                cell_height,
            ) else {
                return Ok(WheelOutcome::Noop);
            };
            self.scroll_history(action, count)?;
            return Ok(WheelOutcome::History {
                before,
                after: self.pane.scrollback_viewport.offset_from_bottom(),
            });
        }
        let Some((action, count)) =
            self.vertical_wheel
                .push(absolute, discrete, value120, cell_height)
        else {
            return Ok(WheelOutcome::Noop);
        };
        let Some(position) = position else {
            return Ok(WheelOutcome::Noop);
        };
        let Some(report) = mouse_report(action, position, self.modifiers, modes.sgr_mouse) else {
            return Ok(WheelOutcome::Noop);
        };
        let bytes = report.len().saturating_mul(count);
        let mut batch = Vec::with_capacity(bytes);
        for _ in 0..count {
            batch.extend_from_slice(&report);
        }
        self.send_command(WindowCommand::Input(batch));
        Ok(WheelOutcome::Application {
            reports: count,
            bytes,
        })
    }

    fn begin_selection(&mut self, position: CellPosition) -> bool {
        let Some(snapshot) = self.display_snapshot() else {
            return false;
        };
        let Some(endpoint) = selection_endpoint(&snapshot, position) else {
            return false;
        };
        self.dirty_selection(self.pane.selection);
        let selection = Selection {
            anchor: endpoint,
            end: endpoint,
        };
        self.pane.selection = Some(selection);
        self.pane.selecting = true;
        self.pane.history_selection_pin_blocked = false;
        self.dirty_selection(Some(selection));
        true
    }

    fn extend_selection(&mut self, position: CellPosition) -> bool {
        let (Some(mut selection), Some(snapshot)) = (self.pane.selection, self.display_snapshot())
        else {
            return false;
        };
        let Some(endpoint) = selection_endpoint(&snapshot, position) else {
            return false;
        };
        self.dirty_selection(Some(selection));
        selection.end = endpoint;
        self.pane.selection = Some(selection);
        self.dirty_selection(Some(selection));
        true
    }

    fn finish_selection(&mut self) -> Option<&[u8]> {
        self.pane.selecting = false;
        self.pane.selected_text = self.pane.selection.and_then(|selection| {
            self.pane
                .snapshot
                .as_ref()
                .and_then(|snapshot| selection_text(snapshot, selection).map(String::into_bytes))
        });
        self.pane.selected_text.as_deref()
    }

    fn pointer_cell_at(&self, position: (f64, f64)) -> Option<CellPosition> {
        let frame = self.pane.snapshot_frame.as_ref()?;
        let pane_rect = self.focused_splint().and_then(|splint_id| {
            self.computed_pane_layout()
                .ok()
                .flatten()
                .and_then(|layout| layout.rect(splint_id))
        });
        let (logical_width, logical_height, x, y) = pane_rect.map_or(
            (self.logical_width, self.logical_height, 0.0, 0.0),
            |rect| {
                (
                    rect.width,
                    rect.height,
                    f64::from(rect.x),
                    f64::from(rect.y),
                )
            },
        );
        let geometry = frame
            .window_geometry(logical_width, logical_height, self.scale_120)
            .ok()?;
        let (row, column) = frame.cell_at(position.0 - x, position.1 - y, &geometry)?;
        Some(CellPosition { row, column })
    }

    fn dirty_row(&mut self, row: usize) {
        let rows = self
            .pane
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.rows);
        self.pane.raster_dirty_rows.resize(rows, false);
        self.pane.surface_dirty_rows.resize(rows, false);
        if row < rows {
            self.pane.raster_dirty_rows[row] = true;
            self.pane.surface_dirty_rows[row] = true;
        }
    }

    fn dirty_selection(&mut self, selection: Option<Selection>) {
        let bounds = selection.and_then(|selection| {
            let snapshot = self.pane.snapshot.as_ref()?;
            let display = self.display_snapshot()?;
            selection_display_bounds(snapshot, &display, selection)
        });
        if let Some((start, end)) = bounds {
            for row in start.row..=end.row {
                self.dirty_row(row);
            }
        }
    }

    fn invalidate_viewport_local_state(&mut self) {
        if let Some((start, _, _)) = &self.pane.hovered_url {
            self.dirty_row(start.row);
        }
        self.pane.selected_text = None;
        self.pane.hovered_url = None;
        let selecting = self.pane.selecting;
        self.pressed_buttons.retain(|_, owner| {
            matches!(owner, PressOwner::Application { .. })
                || selecting && matches!(owner, PressOwner::Selection)
        });
    }

    fn invalidate_local_content_state(&mut self) {
        self.dirty_selection(self.pane.selection);
        self.pane.selection = None;
        self.pane.selecting = false;
        self.pane.history_selection_pin_blocked = false;
        self.invalidate_viewport_local_state();
    }

    fn reconcile_selection_after_content_change(&mut self) {
        let retained = self.pane.selection.is_none_or(|selection| {
            self.pane
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| selection_is_retained(snapshot, selection))
        });
        if !retained {
            self.pane.selection = None;
            self.pane.selecting = false;
            self.pane.history_selection_pin_blocked = false;
        }
        self.invalidate_viewport_local_state();
    }

    fn recompute_hovered_url(&mut self) {
        let previous = self.pane.hovered_url.take();
        let display = self.display_snapshot();
        self.pane.hovered_url = self.pane.pointer_cell.and_then(|position| {
            display
                .as_ref()
                .and_then(|snapshot| url_at(snapshot, position))
        });
        if previous != self.pane.hovered_url {
            if let Some((start, _, _)) = previous {
                self.dirty_row(start.row);
            }
            if let Some((start, _, _)) = &self.pane.hovered_url {
                self.dirty_row(start.row);
            }
        }
    }

    fn begin_clipboard_read(&mut self, target: PasteTarget) {
        let tx = self.clipboard_tx.clone();
        match target {
            PasteTarget::Clipboard => {
                let Some(offer) = self.clipboard_offer.clone() else {
                    return;
                };
                let mime = offer.with_mime_types(accepted_text_mime);
                let Some(mime) = mime else { return };
                if let Ok(pipe) = offer.receive(mime) {
                    spawn_clipboard_read(pipe.into(), target, tx);
                }
            }
            PasteTarget::Primary => {
                let Some(offer) = self.primary_offer.clone() else {
                    return;
                };
                let mime = offer.with_mime_types(accepted_text_mime);
                let Some(mime) = mime else { return };
                if let Ok(pipe) = offer.receive(mime) {
                    spawn_clipboard_read(pipe.into(), target, tx);
                }
            }
        }
    }

    fn apply_clipboard_reads(&mut self) -> Result<()> {
        while let Ok(read) = self.clipboard_rx.try_recv() {
            let Ok(bytes) = read.bytes else {
                continue;
            };
            let Ok(bytes) = safe_paste(&bytes) else {
                continue;
            };
            let bracketed = self
                .pane
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.input_modes.bracketed_paste);
            self.send_input(encode_bracketed_paste(bytes, bracketed))?;
            let _ = read.target;
        }
        Ok(())
    }

    fn publish_clipboard(&mut self, qh: &QueueHandle<Self>, serial: u32, primary: bool) {
        let Some(text) = self.pane.selected_text.as_ref() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let payload = Arc::<[u8]>::from(text.clone());
        let mimes: Vec<_> = TEXT_MIMES.iter().map(|mime| (*mime).to_owned()).collect();
        if primary {
            if let (Some(manager), Some(device)) = (
                self.primary_selection_manager.as_ref(),
                self.primary_device.as_ref(),
            ) {
                let source = manager.create_selection_source(qh, mimes);
                source.set_selection(device, serial);
                self.primary_sources.clear();
                self.primary_sources.push((source, payload));
            }
        } else if let Some(device) = self.data_device.as_ref() {
            let source = self.data_device_manager.create_copy_paste_source(qh, mimes);
            source.set_selection(device, serial);
            self.clipboard_sources.clear();
            self.clipboard_sources.push((source, payload));
        }
    }

    fn open_hovered_url(&self) {
        let Some((_, _, url)) = &self.pane.hovered_url else {
            return;
        };
        let _ = Command::new("xdg-open").arg(url).spawn();
    }

    fn fail(&mut self, error: anyhow::Error) {
        eprintln!("Wayland client failure: {error:#}");
        self.failure = Some(error);
        self.exit = true;
    }

    fn send_topology_command(&mut self, command: WindowTopologyCommand) {
        let Some(commands) = &self.topology_commands else {
            return;
        };
        if let Err(error) = commands.try_send(command) {
            self.fail(anyhow::anyhow!("topology command queue failed: {error}"));
        }
    }

    fn send_command(&mut self, command: WindowCommand) {
        let Some(commands) = &self.pane.commands else {
            return;
        };
        if let Err(error) = try_window_command(commands, command) {
            self.fail(error);
        }
    }

    fn send_coalescible_input(&mut self, bytes: Vec<u8>) {
        let Some(commands) = &self.pane.commands else {
            return;
        };
        match commands.try_send(WindowCommand::Input(bytes)) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Closed(_)) => {
                self.fail(anyhow::anyhow!("Wayland command receiver disconnected"));
            }
        }
    }

    fn commit_text_input(&mut self) {
        if let Some(text_input) = &self.text_input {
            text_input.commit();
            self.ime.note_client_commit();
        }
    }

    fn focused_logical_rect(&self) -> Rect {
        self.focused_splint()
            .and_then(|splint_id| {
                self.computed_pane_layout()
                    .ok()
                    .flatten()
                    .and_then(|layout| layout.rect(splint_id))
            })
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                width: self.logical_width,
                height: self.logical_height,
            })
    }

    fn ime_cursor_rectangle(&self) -> Option<(i32, i32, i32, i32)> {
        let frame = self.pane.snapshot_frame.as_ref()?;
        let rect = self.focused_logical_rect();
        let geometry = frame
            .window_geometry(rect.width, rect.height, self.scale_120)
            .ok()?;
        offset_cursor_rectangle(frame.cursor_rectangle(&geometry)?, rect)
    }

    fn update_ime_cursor_rectangle(&mut self) {
        if !self.ime.entered || !self.ime.focused {
            return;
        }
        let Some(text_input) = &self.text_input else {
            return;
        };
        if let Some((x, y, width, height)) = self.ime_cursor_rectangle() {
            text_input.set_cursor_rectangle(x, y, width, height);
            self.commit_text_input();
        }
    }

    fn set_ime_focus(&mut self, focused: bool) {
        self.keyboard_focused = focused;
        self.ime.focused = focused;
        if !focused && self.ime.entered {
            if let Some(text_input) = &self.text_input {
                text_input.disable();
            }
            self.commit_text_input();
            self.clear_ime_preedit();
        } else if focused && self.ime.entered {
            self.enable_text_input();
        }
    }

    fn enable_text_input(&mut self) {
        let Some(text_input) = &self.text_input else {
            return;
        };
        text_input.enable();
        text_input.set_content_type(
            zwp_text_input_v3::ContentHint::None,
            zwp_text_input_v3::ContentPurpose::Terminal,
        );
        if let Some((x, y, width, height)) = self.ime_cursor_rectangle() {
            text_input.set_cursor_rectangle(x, y, width, height);
        }
        self.commit_text_input();
    }

    fn clear_ime_preedit(&mut self) {
        let had_preedit = self.ime.visible_preedit.is_some();
        self.ime.clear();
        if had_preedit {
            let _ = self.refresh_ime_preedit();
        }
    }

    fn refresh_ime_preedit(&mut self) -> Result<()> {
        // The prepared frame represents the display viewport, which may be
        // detached from the live grid. Refreshing it from `self.pane.snapshot`
        // corrupts an unrelated history row when focus loss clears preedit.
        let Some(mut render_snapshot) = self.display_snapshot() else {
            return Ok(());
        };
        let Some(row) =
            apply_ime_preedit(&mut render_snapshot, self.ime.visible_preedit.as_deref())
        else {
            return Ok(());
        };
        let Some(frame) = &mut self.pane.snapshot_frame else {
            return Ok(());
        };
        let mut dirty = vec![false; render_snapshot.rows];
        dirty[row] = true;
        frame.refresh_rows(&render_snapshot, &dirty)?;
        self.pane
            .raster_dirty_rows
            .resize(render_snapshot.rows, false);
        self.pane
            .surface_dirty_rows
            .resize(render_snapshot.rows, false);
        self.pane.raster_dirty_rows[row] = true;
        self.pane.surface_dirty_rows[row] = true;
        Ok(())
    }

    fn input_modes(&self) -> TerminalInputModes {
        self.pane.snapshot.as_ref().map_or(
            TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: true,
                mouse_tracking: MouseTracking::None,
                sgr_mouse: false,
            },
            |snapshot| snapshot.input_modes,
        )
    }

    fn rebuild_scaled_pane_frames(&mut self, scale_120: u32) -> Result<bool> {
        let mut rebuilt = rebuild_pane_scaled_frame(&mut self.pane, scale_120)?;
        for pane in &mut self.inactive_panes {
            rebuilt |= rebuild_pane_scaled_frame(pane, scale_120)?;
        }
        Ok(rebuilt)
    }

    fn apply_font_zoom(
        &mut self,
        action: FontZoomAction,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<bool> {
        let next = match action {
            FontZoomAction::Increase => self.font_zoom_steps.saturating_add(1),
            FontZoomAction::Decrease => self.font_zoom_steps.saturating_sub(1),
            FontZoomAction::Reset => 0,
        };
        if next == self.font_zoom_steps {
            return Ok(true);
        }
        let Some(raster_changed) = set_font_zoom_steps(next, self.scale_120)? else {
            return Ok(true);
        };
        self.font_zoom_steps = next;
        if !raster_changed {
            return Ok(true);
        }
        self.rebuild_scaled_pane_frames(self.scale_120)?;
        self.buffers.clear();
        self.backing.clear();
        self.pane.pending_scrolls.clear();
        self.full_redraw = true;
        self.cursor_blink_visible = true;
        self.last_cursor_blink = Instant::now();
        self.refresh_ime_preedit()?;
        self.update_ime_cursor_rectangle();
        if self.configured {
            self.emit_resize()?;
            self.schedule_draw(queue_handle)?;
        }
        Ok(true)
    }

    fn decide_consent(&mut self, granted: bool) {
        if let Some(consent) = self.trusted_consent.take() {
            let _ = consent.decision.send(granted);
        }
        self.exit = true;
    }

    fn update_window_title(&self) {
        if let Some(snapshot) = &self.pane.snapshot {
            self.window.set_title(window_title(
                self.title_override.as_deref().or(Some(&snapshot.title)),
                self.pane.controller_active,
                &self.pane.authority,
                self.pane.pending_control_transfer.is_some(),
                Some(&self.pane.search),
            ));
        }
    }

    fn reveal_pending_search_match(&mut self) {
        let Some(item) = self.pane.search.pending_reveal.clone() else {
            return;
        };
        let Some(snapshot) = self.pane.snapshot.as_ref() else {
            return;
        };
        if snapshot.active_screen != ActiveScreen::Normal {
            return;
        }
        if self
            .pane
            .scrollback_viewport
            .reveal_row(item.row_id, snapshot)
        {
            let endpoint = |column| SelectionEndpoint {
                active_screen: ActiveScreen::Normal,
                history_generation: snapshot.history_generation,
                row_id: item.row_id,
                column,
            };
            self.pane.selection = Some(Selection {
                anchor: endpoint(item.start_column),
                end: endpoint(item.end_column.saturating_sub(1)),
            });
            self.pane.search.pending_reveal = None;
            self.pane.viewport_dirty = true;
            self.full_redraw = true;
        } else if !self.pane.history_page_pending {
            let before_row_id = snapshot
                .scrollback_rows
                .first()
                .and_then(|row| row.row_id)
                .or_else(|| {
                    snapshot
                        .newest_available_scrollback_row_id
                        .and_then(|id| id.checked_add(1))
                });
            if let Some(before_row_id) = before_row_id {
                self.pane.history_page_pending = true;
                self.send_command(WindowCommand::FetchScrollback {
                    splint_id: snapshot.splint_id,
                    incarnation: snapshot.incarnation,
                    terminal_revision: snapshot.revision,
                    history_generation: snapshot.history_generation,
                    before_row_id,
                });
            }
        }
    }

    fn submit_search(&mut self, cursor: Option<String>) {
        let Some(snapshot) = self.pane.snapshot.as_ref() else {
            return;
        };
        let query = self.pane.search.query.clone();
        if query.is_empty() {
            return;
        }
        self.send_command(WindowCommand::Search {
            terminal_revision: snapshot.revision,
            history_generation: snapshot.history_generation,
            query,
            case_sensitive: false,
            cursor,
        });
    }

    #[allow(
        clippy::too_many_lines,
        reason = "trusted local shortcuts and search editing share one ordered keyboard boundary"
    )]
    fn handle_key(&mut self, event: &KeyEvent) {
        if self.trusted_consent.is_some() {
            match event.keysym {
                Keysym::g | Keysym::G | Keysym::Return | Keysym::KP_Enter => {
                    self.decide_consent(true);
                }
                Keysym::d | Keysym::D | Keysym::Escape => self.decide_consent(false),
                _ => {}
            }
            return;
        }
        if self.pane.search.input.is_some() {
            if self.modifiers.ctrl && matches!(event.keysym, Keysym::n | Keysym::N) {
                if self.pane.search.selected + 1 < self.pane.search.matches.len() {
                    self.pane.search.selected += 1;
                    self.pane.search.pending_reveal = self
                        .pane
                        .search
                        .matches
                        .get(self.pane.search.selected)
                        .cloned();
                    self.reveal_pending_search_match();
                } else if let Some(cursor) = self.pane.search.next_cursor.clone() {
                    self.submit_search(Some(cursor));
                }
                self.update_window_title();
                return;
            }
            if self.modifiers.ctrl && matches!(event.keysym, Keysym::p | Keysym::P) {
                self.pane.search.selected = self.pane.search.selected.saturating_sub(1);
                self.pane.search.pending_reveal = self
                    .pane
                    .search
                    .matches
                    .get(self.pane.search.selected)
                    .cloned();
                self.reveal_pending_search_match();
                self.update_window_title();
                return;
            }
            match event.keysym {
                Keysym::Escape => {
                    self.pane.search = SearchUiState::default();
                    self.pane.selection = None;
                }
                Keysym::Return | Keysym::KP_Enter => {
                    self.pane.search.query = self
                        .pane
                        .search
                        .input
                        .as_deref()
                        .unwrap_or_default()
                        .to_owned();
                    self.submit_search(None);
                }
                Keysym::BackSpace => {
                    if let Some(input) = self.pane.search.input.as_mut() {
                        input.pop();
                    }
                }
                _ if !self.modifiers.ctrl && !self.modifiers.alt => {
                    if let (Some(input), Some(text)) =
                        (self.pane.search.input.as_mut(), event.utf8.as_deref())
                    {
                        for character in text.chars().filter(|character| !character.is_control()) {
                            if input.len() + character.len_utf8()
                                <= splinterm_protocol::MAX_SEARCH_QUERY_BYTES
                            {
                                input.push(character);
                            }
                        }
                    }
                }
                _ => {}
            }
            self.update_window_title();
            return;
        }
        if self.modifiers.ctrl
            && self.modifiers.shift
            && matches!(event.keysym, Keysym::r | Keysym::R)
        {
            let ids: Vec<_> = self
                .pane
                .authority
                .grants
                .iter()
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                self.send_command(WindowCommand::RevokeAccess(id));
            }
            self.pane.authority.grants.clear();
            if let Some(snapshot) = &self.pane.snapshot {
                self.window.set_title(window_title(
                    self.title_override.as_deref().or(Some(&snapshot.title)),
                    self.pane.controller_active,
                    &self.pane.authority,
                    self.pane.pending_control_transfer.is_some(),
                    Some(&self.pane.search),
                ));
            }
            return;
        }
        if self.modifiers.ctrl && self.modifiers.shift {
            match event.keysym {
                Keysym::t | Keysym::T => {
                    self.send_command(WindowCommand::RequestControlTransfer);
                    return;
                }
                Keysym::f | Keysym::F => {
                    self.pane.search.input = Some(String::new());
                    self.pane.search.matches.clear();
                    self.pane.search.next_cursor = None;
                    self.update_window_title();
                    return;
                }
                Keysym::u | Keysym::U => {
                    self.send_command(WindowCommand::ForceControlTransfer);
                    return;
                }
                Keysym::y | Keysym::Y => {
                    if let Some(transfer_id) = self.pane.pending_control_transfer.take() {
                        self.send_command(WindowCommand::DecideControlTransfer {
                            transfer_id,
                            decision: ControlTransferDecision::Accept,
                        });
                    }
                    return;
                }
                Keysym::n | Keysym::N => {
                    if let Some(transfer_id) = self.pane.pending_control_transfer.take() {
                        self.send_command(WindowCommand::DecideControlTransfer {
                            transfer_id,
                            decision: ControlTransferDecision::Deny,
                        });
                    }
                    return;
                }
                _ => {}
            }
        }
        if self.modifiers.ctrl
            && self.modifiers.shift
            && matches!(event.keysym, Keysym::l | Keysym::L)
        {
            self.send_command(WindowCommand::ReleaseControl);
            self.pane.controller_active = false;
            if let Some(snapshot) = &self.pane.snapshot {
                self.window.set_title(window_title(
                    self.title_override.as_deref().or(Some(&snapshot.title)),
                    self.pane.controller_active,
                    &self.pane.authority,
                    self.pane.pending_control_transfer.is_some(),
                    Some(&self.pane.search),
                ));
            }
            return;
        }
        if self.evidence_close_shortcuts
            && matches!(event.keysym, Keysym::Escape | Keysym::q | Keysym::Q)
        {
            self.exit = true;
            return;
        }
        let utf8 = if self.ime.composing() && !self.modifiers.ctrl && !self.modifiers.alt {
            None
        } else {
            event.utf8.as_deref()
        };
        if let Some(bytes) = key_input(event.keysym, utf8, self.modifiers, self.input_modes()) {
            self.send_command(WindowCommand::Input(bytes));
        }
    }

    fn emit_resize(&mut self) -> Result<()> {
        let layout = self.computed_pane_layout()?;
        let active_rect = self
            .focused_splint()
            .and_then(|splint_id| layout.as_ref().and_then(|layout| layout.rect(splint_id)));
        let (active_width, active_height) = active_rect
            .map_or((self.logical_width, self.logical_height), |rect| {
                (rect.width, rect.height)
            });
        let controller_active = self.pane.controller_active;
        Self::emit_pane_resize(
            &mut self.pane,
            active_width,
            active_height,
            self.scale_120,
            controller_active,
        )?;
        for pane in &mut self.inactive_panes {
            let Some(splint_id) = pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id) else {
                continue;
            };
            let Some(rect) = layout.as_ref().and_then(|layout| layout.rect(splint_id)) else {
                continue;
            };
            Self::emit_pane_resize(pane, rect.width, rect.height, self.scale_120, false)?;
        }
        Ok(())
    }

    fn emit_pane_resize(
        pane: &mut PaneView,
        logical_width: u32,
        logical_height: u32,
        scale_120: u32,
        activate: bool,
    ) -> Result<()> {
        let (Some(frame), Some(commands)) = (&pane.snapshot_frame, &pane.commands) else {
            return Ok(());
        };
        let resize = match frame.terminal_size(logical_width, logical_height, scale_120) {
            Ok(resize) => resize,
            Err(error) if error.to_string().contains("SurfaceTooSmall") => return Ok(()),
            Err(error) => return Err(error),
        };
        if !resize_changed(pane.last_resize, resize) {
            return Ok(());
        }
        let resize_command = if activate {
            WindowCommand::Resize {
                columns: resize.0,
                rows: resize.1,
                pixel_width: resize.2,
                pixel_height: resize.3,
            }
        } else {
            WindowCommand::PrepareResize {
                columns: resize.0,
                rows: resize.1,
                pixel_width: resize.2,
                pixel_height: resize.3,
            }
        };
        try_window_command(commands, resize_command)?;
        pane.last_resize = Some(resize);
        Ok(())
    }

    fn apply_topology_updates(&mut self) -> Result<bool> {
        let mut pending = Vec::new();
        if let Some(updates) = &mut self.topology_updates {
            let drained = drain_receiver(updates, &self.update_waker);
            pending = drained.items;
            if drained.disconnected {
                self.topology_updates = None;
            }
        }
        let mut changed = false;
        for update in pending {
            let (layout, added, removed) = match update {
                WindowTopologyUpdate::Apply {
                    layout,
                    added,
                    removed,
                } => (layout, added, removed),
                WindowTopologyUpdate::Closed => {
                    self.exit = true;
                    continue;
                }
                WindowTopologyUpdate::Shutdown(message) => {
                    anyhow::bail!("topology manager stopped: {message}");
                }
            };
            let removed = removed.into_iter().collect::<HashSet<_>>();
            for pane in added {
                self.inactive_panes
                    .push(PaneView::from_options(pane, self.scale_120)?);
            }
            if self
                .focused_splint()
                .is_some_and(|splint_id| removed.contains(&splint_id))
            {
                let fallback = layout.first_splint_id();
                anyhow::ensure!(
                    self.focus_splint(fallback),
                    "topology focus fallback is absent"
                );
            }
            self.inactive_panes.retain(|pane| {
                pane.snapshot
                    .as_ref()
                    .is_none_or(|snapshot| !removed.contains(&snapshot.splint_id))
            });
            let mut identities = self
                .inactive_panes
                .iter()
                .filter_map(|pane| pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id))
                .collect::<HashSet<_>>();
            if let Some(focused) = self.focused_splint() {
                identities.insert(focused);
            }
            anyhow::ensure!(
                identities.len() == layout.splint_count()
                    && identities
                        .iter()
                        .all(|splint_id| layout.find_splint(*splint_id).is_some()),
                "topology update pane identities do not match its layout"
            );
            self.layout = Some(layout);
            self.full_redraw = true;
            changed = true;
        }
        Ok(changed)
    }

    fn apply_inactive_updates(&mut self) -> Result<bool> {
        let mut changed = false;
        let mut next_theme = None;
        for pane in &mut self.inactive_panes {
            let mut pending = Vec::new();
            let mut disconnected = false;
            if let Some(updates) = &mut pane.updates {
                let drained = drain_receiver(updates, &self.update_waker);
                pending = drained.items;
                disconnected = drained.disconnected;
            }
            if disconnected {
                pane.controller_active = false;
                pane.commands = None;
                pane.updates = None;
                changed = true;
            }
            for update in pending {
                if let WindowUpdate::Theme(theme) = update {
                    next_theme = Some(theme);
                } else {
                    changed |= pane.apply_background_update(update, self.theme, self.scale_120)?;
                }
            }
        }
        if let Some(theme) = next_theme {
            set_background_alpha(theme.background_alpha);
            self.theme = theme;
            if let Some(snapshot) = self.pane.snapshot.as_mut() {
                apply_theme(snapshot, theme);
                self.pane.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
                    snapshot,
                    self.scale_120,
                    Some(&self.pane.image_sources),
                )?);
            }
            for pane in &mut self.inactive_panes {
                if let Some(snapshot) = pane.snapshot.as_mut() {
                    apply_theme(snapshot, theme);
                    pane.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
                        snapshot,
                        self.scale_120,
                        Some(&pane.image_sources),
                    )?);
                }
            }
            changed = true;
        }
        if changed {
            self.full_redraw = true;
        }
        Ok(changed)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "bounded update draining and semantic damage coalescing stay adjacent"
    )]
    fn apply_updates(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let topology_changed = self.apply_topology_updates()?;
        if self.exit {
            return Ok(());
        }
        if topology_changed {
            self.emit_resize()?;
        }

        // Topology may promote a newly added pane to focus. Drain and arm the
        // post-topology focused receiver so it cannot fall back to the timed tick.
        let mut pending = Vec::new();
        let mut disconnected = false;
        if let Some(updates) = &mut self.pane.updates {
            let drained = drain_receiver(updates, &self.update_waker);
            pending = drained.items;
            disconnected = drained.disconnected;
        }
        if disconnected {
            self.exit = true;
            return Ok(());
        }
        let mut visual_changed = topology_changed | self.apply_inactive_updates()?;
        let mut title_changed = false;
        let mut full_frame_reload = false;
        for update in pending {
            match update {
                WindowUpdate::Snapshot {
                    mut snapshot,
                    image_sources,
                } => {
                    self.pane.history_page_pending = false;
                    snapshot
                        .validate()
                        .map_err(|error| anyhow::anyhow!(error.message))?;
                    apply_theme(&mut snapshot, self.theme);
                    let accept = match self.pane.snapshot.as_ref() {
                        Some(current) => snapshot_is_newer(current, &snapshot)?,
                        None => true,
                    };
                    if accept {
                        let previous_generation = self
                            .pane
                            .snapshot
                            .as_ref()
                            .map_or(snapshot.history_generation, |current| {
                                current.history_generation
                            });
                        let previous_rows = self
                            .pane
                            .snapshot
                            .as_ref()
                            .map_or_else(Vec::new, |current| current.scrollback_rows.clone());
                        self.pane.scrollback_viewport.observe_history_change(
                            previous_generation,
                            &previous_rows,
                            &snapshot,
                        );
                        self.invalidate_local_content_state();
                        self.pane.snapshot = Some(snapshot);
                        self.pane.image_sources = image_sources;
                        self.full_redraw = true;
                        full_frame_reload = true;
                        visual_changed = true;
                        title_changed = true;
                    }
                }
                WindowUpdate::Update {
                    update,
                    image_sources,
                } => {
                    let apply_started = perf_trace_enabled().then(Instant::now);
                    let trace_base_revision = update.base_revision;
                    let trace_revision = update.revision;
                    let trace_rows = update.rows.len();
                    let old_cursor_row = self.pane.snapshot.as_ref().and_then(|snapshot| {
                        usize::try_from(snapshot.cursor_row)
                            .ok()
                            .filter(|row| *row < snapshot.rows)
                    });
                    let patched_rows: Vec<_> =
                        update.rows.iter().map(|patch| patch.index).collect();
                    let scrolls = update.scrolls.clone();
                    let history_changed = update.scrollback.is_some();
                    let current = self
                        .pane
                        .snapshot
                        .as_ref()
                        .context("terminal update arrived before initial snapshot")?;
                    let full_frame_reasons = terminal_update_full_frame_reasons(
                        &update,
                        current.active_screen,
                        current.images.is_some(),
                    );
                    let mut full = full_frame_reasons != 0;
                    let content_changed = terminal_update_changes_visible_content(&update);
                    let cursor_changed = update.cursor.is_some() || update.input_modes.is_some();
                    title_changed |= update.title.is_some();
                    let previous_generation = self
                        .pane
                        .snapshot
                        .as_ref()
                        .map_or(1, |snapshot| snapshot.history_generation);
                    let previous_rows = self
                        .pane
                        .snapshot
                        .as_ref()
                        .map_or_else(Vec::new, |snapshot| snapshot.scrollback_rows.clone());
                    let snapshot = self
                        .pane
                        .snapshot
                        .as_mut()
                        .context("terminal update arrived before initial snapshot")?;
                    apply_terminal_update(snapshot, update)?;
                    apply_theme(snapshot, self.theme);
                    self.pane.scrollback_viewport.observe_history_change(
                        previous_generation,
                        &previous_rows,
                        snapshot,
                    );
                    if history_changed && !self.pane.scrollback_viewport.is_live() {
                        full = true;
                    }
                    if content_changed {
                        self.reconcile_selection_after_content_change();
                    }
                    let snapshot = self
                        .pane
                        .snapshot
                        .as_ref()
                        .context("updated terminal snapshot exists")?;
                    let rows = snapshot.rows;
                    if let Some(image_sources) = image_sources {
                        self.pane.image_sources = image_sources;
                    }
                    self.pane.prepare_dirty_rows.resize(rows, false);
                    self.pane.raster_dirty_rows.resize(rows, false);
                    self.pane.surface_dirty_rows.resize(rows, false);
                    if full {
                        self.full_redraw = true;
                        full_frame_reload = true;
                    } else {
                        for scroll in &scrolls {
                            for row in scroll.start_row..scroll.end_row.min(rows) {
                                // Rebuilding the bounded semantic scroll region keeps prepared
                                // row geometry correct while pixel movement still uses scroll-copy.
                                self.pane.prepare_dirty_rows[row] = true;
                                self.pane.surface_dirty_rows[row] = true;
                            }
                            let count = scroll
                                .rows
                                .min(scroll.end_row.saturating_sub(scroll.start_row));
                            let exposed = match scroll.direction {
                                splinterm_protocol::ScrollDirection::Forward => {
                                    scroll.end_row.saturating_sub(count)..scroll.end_row
                                }
                                splinterm_protocol::ScrollDirection::Reverse => {
                                    scroll.start_row..scroll.start_row.saturating_add(count)
                                }
                            };
                            for row in exposed.filter(|row| *row < rows) {
                                self.pane.raster_dirty_rows[row] = true;
                            }
                        }
                        for row in patched_rows.into_iter().filter(|row| *row < rows) {
                            self.pane.prepare_dirty_rows[row] = true;
                            let copied = scrolls
                                .iter()
                                .any(|scroll| row >= scroll.start_row && row < scroll.end_row);
                            if !copied {
                                self.pane.raster_dirty_rows[row] = true;
                                self.pane.surface_dirty_rows[row] = true;
                            }
                        }
                        if cursor_changed {
                            if let Some(row) = old_cursor_row {
                                self.pane.raster_dirty_rows[row] = true;
                                self.pane.surface_dirty_rows[row] = true;
                            }
                            if let Ok(row) = usize::try_from(snapshot.cursor_row) {
                                if row < rows {
                                    self.pane.raster_dirty_rows[row] = true;
                                    self.pane.surface_dirty_rows[row] = true;
                                }
                            }
                        }
                        self.pane.pending_scrolls.extend(scrolls);
                    }
                    visual_changed |= full
                        || cursor_changed
                        || self.pane.raster_dirty_rows.iter().any(|dirty| *dirty);
                    if let Some(started) = apply_started {
                        emit_perf_trace(
                            "splinterm",
                            "client_apply",
                            PerfTraceEvent {
                                splint_id: Some(snapshot.splint_id),
                                incarnation: Some(snapshot.incarnation),
                                base_revision: Some(trace_base_revision),
                                revision: Some(trace_revision),
                                duration_ns: Some(
                                    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                                ),
                                rows: Some(u64::try_from(trace_rows).unwrap_or(u64::MAX)),
                                // Bitset: columns, rows, palette, defaults, screen, images,
                                // or an image-bearing scroll in ascending bit order.
                                count: Some(full_frame_reasons),
                                full_reload: Some(full),
                                ..PerfTraceEvent::default()
                            },
                        );
                    }
                }
                WindowUpdate::ScrollbackPages(pages) => {
                    self.pane.history_page_pending = false;
                    let pinned_selection_rows = self
                        .pane
                        .selection
                        .map(|selection| [selection.anchor.row_id, selection.end.row_id]);
                    let snapshot = self
                        .pane
                        .snapshot
                        .as_mut()
                        .context("scrollback pages arrived before initial snapshot")?;
                    if pages.iter().any(|page| {
                        page.splint_id != snapshot.splint_id
                            || page.incarnation != snapshot.incarnation
                            || page.terminal_revision != snapshot.revision
                            || page.history_generation != snapshot.history_generation
                    }) {
                        continue;
                    }
                    let first_loaded = snapshot
                        .scrollback_rows
                        .first()
                        .and_then(|row| row.row_id)
                        .unwrap_or(u64::MAX);
                    let existing = snapshot
                        .scrollback_rows
                        .iter()
                        .filter_map(|row| row.row_id)
                        .collect::<std::collections::BTreeSet<_>>();
                    let metadata = pages
                        .first()
                        .map(|page| (page.oldest_available_row_id, page.newest_available_row_id));
                    let mut older = pages
                        .into_iter()
                        .rev()
                        .flat_map(|page| page.rows)
                        .filter(|row| {
                            row.row_id
                                .is_some_and(|id| id < first_loaded && !existing.contains(&id))
                        })
                        .collect::<Vec<_>>();
                    if !older.is_empty() {
                        older.extend(snapshot.scrollback_rows.iter().cloned());
                        // Keep one contiguous newest history window. If normal bounding
                        // would remove a selected endpoint, reject this older batch so the
                        // existing bounded endpoint window remains pinned and pageable.
                        if let Some(older) = bound_history_page_with_pins(
                            older,
                            pinned_selection_rows,
                            &snapshot.visible_rows,
                        ) {
                            snapshot.scrollback_rows = older;
                            snapshot.omitted_oldest_scrollback_rows = omitted_rows_before_cache(
                                snapshot.oldest_available_scrollback_row_id,
                                &snapshot.scrollback_rows,
                                snapshot.available_scrollback_rows,
                            );
                            if let Some((oldest, newest)) = metadata {
                                snapshot.oldest_available_scrollback_row_id = oldest;
                                snapshot.newest_available_scrollback_row_id = newest;
                            }
                        } else {
                            self.pane.history_selection_pin_blocked = true;
                        }
                    }
                }
                WindowUpdate::ScrollbackResyncRequired => {
                    self.pane.history_page_pending = false;
                    self.invalidate_local_content_state();
                    visual_changed = true;
                }
                WindowUpdate::Authority(authority) => {
                    self.pane.authority = authority;
                    title_changed = true;
                    visual_changed = true;
                    self.full_redraw = true;
                }
                WindowUpdate::Control(active) => {
                    self.pane.controller_active = active;
                    title_changed = true;
                    visual_changed = true;
                    self.full_redraw = true;
                }
                WindowUpdate::ControlTransferRequested(transfer_id) => {
                    self.pane.pending_control_transfer = Some(transfer_id);
                    title_changed = true;
                }
                WindowUpdate::ControlTransferResolved(_) => {
                    self.pane.pending_control_transfer = None;
                    title_changed = true;
                }
                WindowUpdate::SearchResults(page) => {
                    self.pane.search.matches = page.matches;
                    self.pane.search.selected = 0;
                    self.pane.search.next_cursor = page.next_cursor;
                    self.pane.search.pending_reveal = self.pane.search.matches.first().cloned();
                    title_changed = true;
                    visual_changed = true;
                    self.full_redraw = true;
                }
                WindowUpdate::SearchResyncRequired => {
                    self.pane.search.matches.clear();
                    self.pane.search.next_cursor = None;
                    self.pane.search.pending_reveal = None;
                    title_changed = true;
                    visual_changed = true;
                    self.full_redraw = true;
                }
                WindowUpdate::Theme(theme) => {
                    set_background_alpha(theme.background_alpha);
                    self.theme = theme;
                    if let Some(snapshot) = self.pane.snapshot.as_mut() {
                        apply_theme(snapshot, theme);
                    }
                    visual_changed = true;
                    full_frame_reload = true;
                    self.full_redraw = true;
                }
                WindowUpdate::Shutdown => {
                    if self.layout.is_some() {
                        self.pane.controller_active = false;
                        self.pane.commands = None;
                        self.pane.updates = None;
                        title_changed = true;
                        visual_changed = true;
                        self.full_redraw = true;
                    } else {
                        self.exit = true;
                        return Ok(());
                    }
                }
            }
        }
        if self.pane.search.pending_reveal.is_some() {
            self.reveal_pending_search_match();
            visual_changed = true;
        }
        if visual_changed {
            self.cursor_blink_visible = true;
            self.last_cursor_blink = Instant::now();
            let prepare_started = perf_trace_enabled().then(Instant::now);
            let trace_dirty_rows = self
                .pane
                .prepare_dirty_rows
                .iter()
                .filter(|dirty| **dirty)
                .count();
            let live_viewport = self.pane.scrollback_viewport.is_live();
            let display_owned = if live_viewport {
                None
            } else {
                Some(self.display_snapshot().context("updated snapshot exists")?)
            };
            let display = display_owned
                .as_ref()
                .or(self.pane.snapshot.as_ref())
                .context("updated snapshot exists")?;
            if full_frame_reload || self.pane.snapshot_frame.is_none() || !live_viewport {
                self.pane.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
                    display,
                    self.scale_120,
                    Some(&self.pane.image_sources),
                )?);
            } else if let Some(frame) = &mut self.pane.snapshot_frame {
                frame.refresh_rows(display, &self.pane.prepare_dirty_rows)?;
                frame.refresh_images(display, &self.pane.image_sources)?;
                frame.refresh_cursor(display);
            }
            self.pane.rendered_viewport_offset = self.pane.scrollback_viewport.offset_from_bottom();
            self.pane.viewport_dirty = false;
            self.pane.prepare_dirty_rows.fill(false);
            if let Some(started) = prepare_started {
                emit_perf_trace(
                    "splinterm",
                    "frame_prepare",
                    PerfTraceEvent {
                        splint_id: Some(display.splint_id),
                        incarnation: Some(display.incarnation),
                        revision: Some(display.revision),
                        duration_ns: Some(
                            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                        ),
                        rows: Some(u64::try_from(display.rows).unwrap_or(u64::MAX)),
                        cells: Some(
                            u64::try_from(display.rows.saturating_mul(display.columns))
                                .unwrap_or(u64::MAX),
                        ),
                        count: Some(u64::try_from(trace_dirty_rows).unwrap_or(u64::MAX)),
                        full_reload: Some(full_frame_reload),
                        ..PerfTraceEvent::default()
                    },
                );
            }
            self.refresh_ime_preedit()?;
            self.update_ime_cursor_rectangle();
            if terminal_resize_allowed(
                TerminalResizeCause::SnapshotAvailable,
                self.pane.last_resize.is_some(),
            ) {
                self.emit_resize()?;
            }
            if self.configured {
                self.schedule_terminal_draw(queue_handle)?;
            }
        }
        if title_changed {
            let snapshot = self
                .pane
                .snapshot
                .as_ref()
                .context("updated snapshot exists")?;
            self.window.set_title(window_title(
                self.title_override.as_deref().or(Some(&snapshot.title)),
                self.pane.controller_active,
                &self.pane.authority,
                self.pane.pending_control_transfer.is_some(),
                Some(&self.pane.search),
            ));
        }
        Ok(())
    }

    fn output_dpi_observation(&self, output: &wl_output::WlOutput) -> OutputDpiObservation {
        let Some(info) = self.output_state.info(output) else {
            return OutputDpiObservation::unavailable("output-info-pending");
        };
        let current_mode = info
            .modes
            .iter()
            .find(|mode| mode.current)
            .map(|mode| mode.dimensions);
        OutputDpiObservation::from_wayland(info.id, info.name, current_mode, info.physical_size)
    }

    fn refresh_output_dpi(
        &mut self,
        output: &wl_output::WlOutput,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<()> {
        let observation = self.output_dpi_observation(output);
        if !update_output_dpi(observation, self.scale_120)? {
            return Ok(());
        }
        self.rebuild_scaled_pane_frames(self.scale_120)?;
        self.buffers.clear();
        self.backing.clear();
        self.full_redraw = true;
        self.refresh_ime_preedit()?;
        debug_assert!(!terminal_resize_allowed(
            TerminalResizeCause::OutputDpiChanged,
            self.pane.last_resize.is_some(),
        ));
        self.update_ime_cursor_rectangle();
        if self.configured {
            self.schedule_draw(queue_handle)?;
        }
        Ok(())
    }

    fn apply_scale(
        &mut self,
        requested_scale_120: u32,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<()> {
        let scale_120 = requested_scale_120;
        if !(MIN_SCALE_120..=MAX_SCALE_120).contains(&scale_120) || scale_120 == self.scale_120 {
            return Ok(());
        }
        if self.viewport.is_none() && scale_120 % SCALE_DENOMINATOR != 0 {
            return Ok(());
        }
        if self.viewport.is_none() {
            self.window
                .set_buffer_scale(scale_120 / SCALE_DENOMINATOR)
                .map_err(|_| anyhow::anyhow!("compositor rejected integer buffer scale"))?;
        } else {
            self.window
                .set_buffer_scale(1)
                .map_err(|_| anyhow::anyhow!("compositor rejected unit buffer scale"))?;
        }
        if !self.rebuild_scaled_pane_frames(scale_120)? {
            self.text_row = Some(TextRow::load(scale_120.div_ceil(SCALE_DENOMINATOR))?);
        }
        self.scale_120 = scale_120;
        self.buffers.clear();
        self.backing.clear();
        self.full_redraw = true;
        self.refresh_ime_preedit()?;
        debug_assert!(!terminal_resize_allowed(
            TerminalResizeCause::CompositorScaleChanged,
            self.pane.last_resize.is_some(),
        ));
        self.update_ime_cursor_rectangle();
        if self.configured {
            self.schedule_draw(queue_handle)?;
        }
        Ok(())
    }

    fn tick_cursor_blink(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let blinking = self.cursor_blink
            && self.pane.snapshot.as_ref().is_some_and(|snapshot| {
                cursor_blink_enabled(
                    self.reduced_motion,
                    self.keyboard_focused,
                    snapshot.input_modes,
                )
            });
        if blinking && self.last_cursor_blink.elapsed() >= Duration::from_millis(500) {
            self.cursor_blink_visible = !self.cursor_blink_visible;
            self.last_cursor_blink = Instant::now();
            if let Some(snapshot) = &self.pane.snapshot {
                if let Ok(row) = usize::try_from(snapshot.cursor_row) {
                    if row < snapshot.rows {
                        self.pane.raster_dirty_rows.resize(snapshot.rows, false);
                        self.pane.surface_dirty_rows.resize(snapshot.rows, false);
                        self.pane.raster_dirty_rows[row] = true;
                        self.pane.surface_dirty_rows[row] = true;
                    }
                }
            }
            if self.configured {
                self.schedule_draw(queue_handle)?;
            }
        } else if !blinking && !self.cursor_blink_visible {
            self.cursor_blink_visible = true;
            if let Some(snapshot) = &self.pane.snapshot {
                if let Ok(row) = usize::try_from(snapshot.cursor_row) {
                    if row < snapshot.rows {
                        self.pane.raster_dirty_rows.resize(snapshot.rows, false);
                        self.pane.surface_dirty_rows.resize(snapshot.rows, false);
                        self.pane.raster_dirty_rows[row] = true;
                        self.pane.surface_dirty_rows[row] = true;
                    }
                }
            }
            if self.configured {
                self.schedule_draw(queue_handle)?;
            }
        }
        Ok(())
    }

    fn schedule_draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        if self.frame_pending {
            self.redraw_pending = true;
            Ok(())
        } else {
            self.draw(queue_handle)
        }
    }

    fn schedule_terminal_draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        self.terminal_redraw_pending = true;
        let draw_capacity_available = self.buffers.len() < MAX_SHM_BUFFERS
            || self
                .buffers
                .iter()
                .any(|buffer| self.pool.canvas(buffer).is_some());
        if terminal_draw_waits_for_frame(self.frame_pending, draw_capacity_available) {
            self.redraw_pending = true;
            Ok(())
        } else {
            // A released buffer may be committed again even when its earlier frame
            // callback is delayed. Reusing it avoids both callback latency and an
            // unbounded replacement-buffer path.
            self.draw(queue_handle)
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "SHM acquisition, persistent backing updates, damage submission, and commit form one transaction"
    )]
    fn draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        self.redraw_pending = false;
        let terminal_priority = std::mem::take(&mut self.terminal_redraw_pending);
        let draw_started = Instant::now();
        let scroll_started = self.pane.scroll_started_at.take();
        if self.pane.viewport_dirty {
            let display = self.display_snapshot().context("scroll display snapshot")?;
            let current_offset = self.pane.scrollback_viewport.offset_from_bottom();
            let delta = isize::try_from(current_offset)
                .ok()
                .zip(isize::try_from(self.pane.rendered_viewport_offset).ok())
                .map(|(current, rendered)| current - rendered);
            self.pane.prepare_dirty_rows.fill(false);
            self.pane.raster_dirty_rows.fill(false);
            self.pane.surface_dirty_rows.fill(false);
            self.pane.pending_scrolls.clear();
            let incremental = if display.images.is_none() {
                if let (Some(frame), Some(delta)) = (&mut self.pane.snapshot_frame, delta) {
                    let scroll = frame.scroll_viewport_rows(&display, delta)?;
                    frame.refresh_cursor(&display);
                    scroll
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(scroll) = incremental {
                let count = scroll.rows.min(display.rows);
                let exposed = match scroll.direction {
                    splinterm_protocol::ScrollDirection::Forward => {
                        display.rows.saturating_sub(count)..display.rows
                    }
                    splinterm_protocol::ScrollDirection::Reverse => 0..count,
                };
                self.pane.raster_dirty_rows.resize(display.rows, false);
                self.pane.surface_dirty_rows.resize(display.rows, false);
                for row in exposed {
                    self.pane.raster_dirty_rows[row] = true;
                }
                self.pane.surface_dirty_rows.fill(true);
                self.pane.pending_scrolls.push(scroll);
            } else {
                self.pane.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
                    &display,
                    self.scale_120,
                    Some(&self.pane.image_sources),
                )?);
                self.full_redraw = true;
            }
            self.pane.rendered_viewport_offset = current_offset;
            self.pane.viewport_dirty = false;
        }
        let pane_layout = self.computed_pane_layout()?;
        let pane_cell_width = self
            .pane
            .snapshot_frame
            .as_ref()
            .map_or(1, SnapshotFrame::cell_width);
        self.prepare_frame_titles(pane_layout.as_ref(), pane_cell_width)?;
        let active_splint = self.focused_splint();
        let active_rect = active_splint.and_then(|splint_id| {
            pane_layout
                .as_ref()
                .and_then(|layout| layout.rect(splint_id))
        });
        let window_geometry = if let Some(rect) = active_rect {
            Self::pane_geometry(&self.pane, rect, self.scale_120)
        } else {
            self.pane.snapshot_frame.as_ref().map_or(Ok(None), |frame| {
                frame
                    .window_geometry(self.logical_width, self.logical_height, self.scale_120)
                    .map(Some)
            })
        }
        .or_else(|error| {
            if error.to_string().contains("SurfaceTooSmall") {
                Ok(None)
            } else {
                Err(error)
            }
        })?;
        let (width, height, stride) = if pane_layout.is_some() {
            buffer_dimensions(
                self.logical_width.max(1),
                self.logical_height.max(1),
                self.scale_120,
            )?
        } else if let Some(geometry) = window_geometry {
            geometry.buffer_layout()?
        } else {
            buffer_dimensions(
                self.logical_width.max(1),
                self.logical_height.max(1),
                self.scale_120,
            )?
        };
        let width_i32 = i32::try_from(width).context("buffer width fits i32")?;
        let height_i32 = i32::try_from(height).context("buffer height fits i32")?;
        let resolved_selection = self.pane.selection.and_then(|selection| {
            let snapshot = self.pane.snapshot.as_ref()?;
            let display = self.display_snapshot()?;
            selection_display_bounds(snapshot, &display, selection)
                .map(|(start, end)| ((start.row, start.column), (end.row, end.column)))
        });
        let mut buffer_index = None;
        for (index, buffer) in self.buffers.iter().enumerate() {
            if self.pool.canvas(buffer).is_some() {
                buffer_index = Some(index);
                break;
            }
        }
        let buffer_index = if let Some(index) = buffer_index {
            index
        } else if self.buffers.len() < MAX_SHM_BUFFERS {
            let buffer = self
                .pool
                .create_buffer(width_i32, height_i32, stride, wl_shm::Format::Argb8888)
                .context("create bounded SHM buffer")?
                .0;
            self.buffers.push(buffer);
            self.buffers.len() - 1
        } else {
            self.redraw_pending = true;
            self.terminal_redraw_pending = terminal_priority;
            return Ok(());
        };
        let buffer = &self.buffers[buffer_index];
        let canvas = self
            .pool
            .canvas(buffer)
            .context("selected SHM buffer became unavailable")?;

        let backing_len = usize::try_from(
            width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                .context("backing dimensions overflow")?,
        )
        .context("backing size fits usize")?;
        let resized_backing = self.backing.len() != backing_len;
        if resized_backing {
            self.backing.resize(backing_len, 0);
            self.full_redraw = true;
        }
        let capture_minimum_images = capture_minimum_images()?;
        let capture_image_count = self
            .pane
            .snapshot_frame
            .as_ref()
            .map_or(0, SnapshotFrame::image_count)
            .saturating_add(
                self.inactive_panes
                    .iter()
                    .filter_map(|pane| pane.snapshot_frame.as_ref())
                    .map(SnapshotFrame::image_count)
                    .sum(),
            );
        let image_composition_started = (std::env::var_os("SPLINTERM_IMAGE_TRACE").is_some()
            && capture_image_count > 0)
            .then(Instant::now);
        if let (Some(frame), Some(geometry)) = (&self.pane.snapshot_frame, &window_geometry) {
            if self.capture.is_some()
                && capture_minimum_images > 0
                && capture_image_count >= capture_minimum_images
            {
                self.full_redraw = true;
            }
            if self.full_redraw {
                if let Some(layout) = pane_layout.as_ref() {
                    let [_, red, green, blue] = self.theme.background.to_be_bytes();
                    let background = configured_background_bgra([red, green, blue]);
                    for pixel in self.backing.chunks_exact_mut(4) {
                        pixel.copy_from_slice(&background);
                    }
                    for pane in &self.inactive_panes {
                        let Some(splint_id) =
                            pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id)
                        else {
                            continue;
                        };
                        let Some(rect) = layout.rect(splint_id) else {
                            continue;
                        };
                        let (Some(frame), Some(geometry)) = (
                            &pane.snapshot_frame,
                            Self::pane_geometry(pane, rect, self.scale_120)?,
                        ) else {
                            continue;
                        };
                        paint_snapshot_region_presented(
                            &mut self.backing,
                            width,
                            height,
                            frame,
                            &geometry,
                            Self::buffer_rect(rect, self.scale_120)?,
                            self.cursor_blink_visible,
                            self.cursor_style,
                            CursorPresentation::for_keyboard_focus(false),
                        );
                    }
                    paint_snapshot_region_presented(
                        &mut self.backing,
                        width,
                        height,
                        frame,
                        geometry,
                        Self::buffer_rect(
                            active_rect.context("active pane rectangle")?,
                            self.scale_120,
                        )?,
                        self.cursor_blink_visible,
                        self.cursor_style,
                        CursorPresentation::for_keyboard_focus(self.keyboard_focused),
                    );
                } else {
                    paint_snapshot_presented(
                        &mut self.backing,
                        width,
                        height,
                        frame,
                        geometry,
                        self.cursor_blink_visible,
                        self.cursor_style,
                        CursorPresentation::for_keyboard_focus(self.keyboard_focused),
                    );
                }
            } else {
                for scroll in self.pane.pending_scrolls.drain(..) {
                    scroll_snapshot_pixels(&mut self.backing, width, frame, geometry, scroll);
                }
                paint_snapshot_rows_presented(
                    &mut self.backing,
                    width,
                    height,
                    frame,
                    geometry,
                    &self.pane.raster_dirty_rows,
                    self.cursor_blink_visible,
                    self.cursor_style,
                    CursorPresentation::for_keyboard_focus(self.keyboard_focused),
                );
            }
            let selection = resolved_selection;
            let hovered_url = self
                .pane
                .hovered_url
                .as_ref()
                .map(|(start, end, _)| ((start.row, start.column), (end.row, end.column)));
            canvas.copy_from_slice(&self.backing);
            paint_snapshot_overlays(
                canvas,
                width,
                height,
                frame,
                geometry,
                SnapshotOverlays {
                    selection,
                    hovered_url,
                    dirty_rows: None,
                    focused: self.keyboard_focused,
                    selection_color: self.theme.selection,
                    url_color: self.theme.url,
                    accent_color: self.theme.ui_accent,
                },
            );
            if let Some(layout) = pane_layout.as_ref() {
                paint_pane_chrome(
                    canvas,
                    width,
                    height,
                    layout,
                    active_splint,
                    self.theme,
                    frame.cell_width(),
                    frame.cell_height(),
                    self.scale_120,
                    &self.frame_titles,
                )?;
            }
            if let Some(status) =
                history_overlay_status(&self.pane.scrollback_viewport, self.pane.snapshot.as_ref())
            {
                paint_history_overlay(
                    canvas,
                    width,
                    height,
                    self.scale_120,
                    status,
                    self.theme.background,
                    self.theme.ui_accent,
                );
            }
            if self.trusted_consent.is_some() {
                paint_trusted_consent_chrome(canvas, width, height);
            }
        } else if self.pane.snapshot_frame.is_some() {
            let [_, red, green, blue] = self.theme.background.to_be_bytes();
            let background = configured_background_bgra([red, green, blue]);
            for pixel in self.backing.chunks_exact_mut(4) {
                pixel.copy_from_slice(&background);
            }
            canvas.copy_from_slice(&self.backing);
        } else if let Some(row) = &self.text_row {
            paint(canvas, width, height, row);
        } else {
            anyhow::bail!("window has no prepared renderer content");
        }
        if let Some(started) = image_composition_started {
            eprintln!(
                "phase5-image-trace composition_ns={} image_count={capture_image_count}",
                started.elapsed().as_nanos(),
            );
        }
        let capture_scale_ready = self
            .capture_scale
            .is_none_or(|expected| expected.saturating_mul(120) == self.scale_120);
        if deterministic_capture_ready(
            capture_scale_ready,
            capture_minimum_images,
            capture_image_count,
        ) && let Some(path) = self.capture.take()
        {
            write_ppm(&path, canvas, width, height)
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!(
                "Wrote deterministic row capture at {}x scale to {}",
                f64::from(self.scale_120) / 120.0,
                path.display()
            );
        }
        let history_status =
            history_overlay_status(&self.pane.scrollback_viewport, self.pane.snapshot.as_ref());
        let damage_full_surface =
            take_full_surface_damage(&mut self.full_redraw, self.pane.snapshot_frame.is_some());
        if damage_full_surface {
            self.window
                .wl_surface()
                .damage_buffer(0, 0, width_i32, height_i32);
        } else if let Some(geometry) = &window_geometry {
            for (row, dirty) in self.pane.surface_dirty_rows.iter().copied().enumerate() {
                if !dirty {
                    continue;
                }
                if let Some((x, y, row_width, row_height)) = snapshot_row_rect(geometry, row) {
                    self.window
                        .wl_surface()
                        .damage_buffer(x, y, row_width, row_height);
                }
            }
        }
        if history_status != self.pane.painted_history_status {
            if let Some(layout) = history_overlay_layout(width, height, self.scale_120) {
                let (x, y, overlay_width, overlay_height) = layout.panel;
                self.window.wl_surface().damage_buffer(
                    x,
                    y,
                    i32::try_from(overlay_width).unwrap_or(i32::MAX),
                    i32::try_from(overlay_height).unwrap_or(i32::MAX),
                );
            }
        }
        self.pane.painted_history_status = history_status;
        self.pane.raster_dirty_rows.fill(false);
        self.pane.surface_dirty_rows.fill(false);
        self.pane.pending_scrolls.clear();
        if !self.frame_pending {
            self.window
                .wl_surface()
                .frame(queue_handle, self.window.wl_surface().clone());
            self.frame_pending = true;
        }
        buffer
            .attach_to(self.window.wl_surface())
            .context("attach SHM buffer")?;
        self.window.commit();
        let committed_identity = self
            .pane
            .snapshot
            .as_ref()
            .map(|snapshot| (snapshot.splint_id, snapshot.incarnation, snapshot.revision));
        if perf_trace_enabled() {
            let snapshot = self.pane.snapshot.as_ref();
            emit_perf_trace(
                "splinterm",
                "draw_commit",
                PerfTraceEvent {
                    splint_id: snapshot.map(|snapshot| snapshot.splint_id),
                    incarnation: snapshot.map(|snapshot| snapshot.incarnation),
                    revision: snapshot.map(|snapshot| snapshot.revision),
                    duration_ns: Some(
                        u64::try_from(draw_started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                    ),
                    bytes: Some(u64::try_from(backing_len).unwrap_or(u64::MAX)),
                    rows: snapshot.map(|snapshot| u64::try_from(snapshot.rows).unwrap_or(u64::MAX)),
                    full_reload: Some(damage_full_surface),
                    ..PerfTraceEvent::default()
                },
            );
        }
        let inject_graphical_input = self
            .graphical_input_probe
            .as_mut()
            .is_some_and(|probe| probe.observe_commit(committed_identity));
        if inject_graphical_input {
            self.graphical_input_probe = None;
            self.send_input(vec![b'q'])?;
            if perf_trace_enabled() {
                emit_perf_trace(
                    "splinterm",
                    "graphical_input",
                    PerfTraceEvent {
                        splint_id: committed_identity.map(|identity| identity.0),
                        incarnation: committed_identity.map(|identity| identity.1),
                        revision: committed_identity.map(|identity| identity.2),
                        bytes: Some(1),
                        count: Some(1),
                        ..PerfTraceEvent::default()
                    },
                );
            }
        }
        if self.scroll_trace {
            if let Some(scroll_started) = scroll_started {
                eprintln!(
                    "scroll-trace input_to_commit_us={} draw_us={} viewport_offset={} cached_rows={} page_pending={}",
                    scroll_started.elapsed().as_micros(),
                    draw_started.elapsed().as_micros(),
                    self.pane.scrollback_viewport.offset_from_bottom(),
                    self.pane
                        .snapshot
                        .as_ref()
                        .map_or(0, |snapshot| snapshot.scrollback_rows.len()),
                    self.pane.history_page_pending,
                );
            }
        }
        Ok(())
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if surface != self.window.wl_surface() || factor <= 0 {
            return;
        }
        let Ok(integer_scale) = u32::try_from(factor) else {
            self.fail(anyhow::anyhow!(
                "integer output scale does not fit u32: {factor}"
            ));
            return;
        };
        self.integer_fallback_scale = integer_scale;
        if self.fractional_scale.is_none() {
            let scale_120 = integer_scale.saturating_mul(SCALE_DENOMINATOR);
            if let Err(error) = self.apply_scale(scale_120, queue_handle) {
                self.fail(error);
            }
        }
    }

    fn transform_changed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if surface == self.window.wl_surface() {
            self.frame_pending = false;
            if self.redraw_pending {
                if let Err(error) = self.draw(queue_handle) {
                    self.fail(error);
                }
            }
        }
    }

    fn surface_enter(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        if surface != self.window.wl_surface() {
            return;
        }
        note_output_enter(&mut self.entered_outputs, output);
        self.output_count = self.entered_outputs.len();
        if let Err(error) = self.refresh_output_dpi(output, queue_handle) {
            self.fail(error);
        }
    }

    fn surface_leave(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        if surface != self.window.wl_surface() {
            return;
        }
        let was_most_recent = note_output_leave(&mut self.entered_outputs, output);
        self.output_count = self.entered_outputs.len();
        if was_most_recent {
            // With no entered output, retain the last observation as Foot does
            // while temporarily unmapped. Otherwise promote the previous output.
            if let Some(current) = self.entered_outputs.last().cloned() {
                if let Err(error) = self.refresh_output_dpi(&current, queue_handle) {
                    self.fail(error);
                }
            }
        }
    }
}

impl WindowHandler for App {
    fn request_close(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _window: &Window,
    ) {
        if self.trusted_consent.is_some() {
            self.decide_consent(false);
        } else {
            self.exit = true;
        }
    }

    fn configure(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let width = configure
            .new_size
            .0
            .map_or(self.logical_width, std::num::NonZeroU32::get);
        let height = configure
            .new_size
            .1
            .map_or(self.logical_height, std::num::NonZeroU32::get);
        let resized = (width, height) != (self.logical_width, self.logical_height);
        if resized {
            self.logical_width = width;
            self.logical_height = height;
            if let Some(viewport) = &self.viewport {
                match viewport_destination(width, height) {
                    Ok((width, height)) => viewport.set_destination(width, height),
                    Err(error) => {
                        self.fail(error);
                        return;
                    }
                }
            }
            self.buffers.clear();
            self.full_redraw = true;
            self.update_ime_cursor_rectangle();
        }
        let initial_configure = !self.configured;
        self.configured = true;
        if initial_configure || resized {
            debug_assert!(terminal_resize_allowed(
                TerminalResizeCause::SurfaceConfigure,
                self.pane.last_resize.is_some(),
            ));
            if let Err(error) = self
                .emit_resize()
                .and_then(|()| self.schedule_draw(queue_handle))
            {
                self.fail(error);
            }
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
        self.seat_count += 1;
    }

    fn new_capability(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if self.data_device.is_none() {
            self.data_device = Some(
                self.data_device_manager
                    .get_data_device(queue_handle, &seat),
            );
            self.primary_device = self
                .primary_selection_manager
                .as_ref()
                .map(|manager| manager.get_selection_device(queue_handle, &seat));
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard_with_repeat(
                queue_handle,
                &seat,
                None,
                self.loop_handle.clone(),
                Box::new(|_, _, _| {}),
            ) {
                Ok(keyboard) => {
                    self.keyboard = Some(keyboard);
                    self.keyboard_seat = Some(seat.clone());
                    if self.text_input.is_none() {
                        if let Some(manager) = &self.text_input_manager {
                            self.text_input = Some(manager.get_text_input(&seat, queue_handle, ()));
                            self.text_input_seat = Some(seat.clone());
                        }
                    }
                }
                Err(error) => self.fail(anyhow::anyhow!("create keyboard: {error}")),
            }
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(queue_handle, &seat) {
                Ok(pointer) => {
                    self.pointer = Some(pointer);
                    self.pointer_seat = Some(seat);
                }
                Err(error) => self.fail(anyhow::anyhow!("create pointer: {error}")),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard_seat.as_ref() == Some(&seat) {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
            self.keyboard_seat = None;
            if self.text_input_seat.as_ref() == Some(&seat) {
                self.clear_ime_preedit();
                if let Some(text_input) = self.text_input.take() {
                    text_input.disable();
                    text_input.commit();
                    text_input.destroy();
                }
                self.text_input_seat = None;
            }
        }
        if capability == Capability::Pointer && self.pointer_seat.as_ref() == Some(&seat) {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
            self.pointer_seat = None;
            self.pane.pointer_cell = None;
        }
    }

    fn remove_seat(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
    ) {
        if self.keyboard_seat.as_ref() == Some(&seat) {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
            self.keyboard_seat = None;
            if self.text_input_seat.as_ref() == Some(&seat) {
                self.clear_ime_preedit();
                if let Some(text_input) = self.text_input.take() {
                    text_input.disable();
                    text_input.commit();
                    text_input.destroy();
                }
                self.text_input_seat = None;
            }
        }
        if self.pointer_seat.as_ref() == Some(&seat) {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
            self.pointer_seat = None;
        }
        self.data_device = None;
        self.primary_device = None;
        self.seat_count = self.seat_count.saturating_sub(1);
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        if surface == self.window.wl_surface() {
            self.set_ime_focus(true);
            self.full_redraw = true;
            if self.input_modes().focus_reporting {
                self.send_command(WindowCommand::Input(b"\x1b[I".to_vec()));
            }
            if let Err(error) = self.schedule_draw(queue_handle) {
                self.fail(error);
            }
        }
    }

    fn leave(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        if surface == self.window.wl_surface() {
            self.set_ime_focus(false);
            self.full_redraw = true;
            if self.input_modes().focus_reporting {
                self.send_command(WindowCommand::Input(b"\x1b[O".to_vec()));
            }
            if let Err(error) = self.schedule_draw(queue_handle) {
                self.fail(error);
            }
        }
    }

    fn press_key(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        if self.topology_commands.is_some()
            && let Some(action) = pane_topology_action(event.keysym, self.modifiers)
        {
            if let Some(target) = self.focused_splint() {
                let command = match action {
                    PaneTopologyAction::Split(axis) => {
                        WindowTopologyCommand::Split { target, axis }
                    }
                    PaneTopologyAction::Close => WindowTopologyCommand::Close { target },
                    PaneTopologyAction::AdjustRatio(delta) => {
                        WindowTopologyCommand::AdjustRatio { target, delta }
                    }
                };
                self.send_topology_command(command);
            }
            return;
        }
        if let Some(action) = pane_focus_action(event.keysym, self.modifiers) {
            let changed = match action {
                PaneFocusAction::Direction(direction) => self.focus_direction(direction),
                PaneFocusAction::Next { reverse } => self.focus_next(reverse),
            };
            if changed {
                self.update_ime_cursor_rectangle();
                if let Err(error) = self.schedule_draw(queue_handle) {
                    self.fail(error);
                }
            }
            return;
        }
        if let Some(action) = font_zoom_action(event.keysym, self.modifiers) {
            if let Err(error) = self.apply_font_zoom(action, queue_handle) {
                self.fail(error);
            }
            return;
        }
        if self.modifiers.ctrl
            && self.modifiers.shift
            && matches!(event.keysym, Keysym::c | Keysym::C)
        {
            self.publish_clipboard(queue_handle, serial, false);
        } else if self.modifiers.ctrl
            && self.modifiers.shift
            && matches!(event.keysym, Keysym::v | Keysym::V)
        {
            self.begin_clipboard_read(PasteTarget::Clipboard);
        } else {
            match self.handle_history_key(&event, queue_handle) {
                Ok(true) => {}
                Ok(false) => {
                    self.handle_key(&event);
                    if self.full_redraw {
                        if let Err(error) = self.schedule_draw(queue_handle) {
                            self.fail(error);
                        }
                    }
                }
                Err(error) => self.fail(error),
            }
        }
    }

    fn repeat_key(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if let Some(action) = font_zoom_action(event.keysym, self.modifiers) {
            if let Err(error) = self.apply_font_zoom(action, queue_handle) {
                self.fail(error);
            }
            return;
        }
        match self.handle_history_key(&event, queue_handle) {
            Ok(true) => {}
            Ok(false) => self.handle_key(&event),
            Err(error) => self.fail(error),
        }
    }

    fn release_key(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl PointerHandler for App {
    #[allow(
        clippy::too_many_lines,
        reason = "one Wayland pointer frame preserves event order across selection, mouse reporting, paste, and URL gestures"
    )]
    fn pointer_frame(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.window.wl_surface() {
                continue;
            }
            let previous_hover = self.pane.hovered_url.clone();
            let mut cell = self.pointer_cell_at(event.position);
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.last_pointer_serial = Some(serial);
                    self.pane.pointer_cell = cell;
                }
                PointerEventKind::Leave { .. } => {
                    self.pane.pointer_cell = None;
                    self.pane.hovered_url = None;
                }
                PointerEventKind::Motion { .. } => {
                    self.pane.pointer_cell = cell;
                    let display = self.display_snapshot();
                    self.pane.hovered_url = cell.and_then(|position| {
                        display
                            .as_ref()
                            .and_then(|snapshot| url_at(snapshot, position))
                    });
                    if self.pane.selecting {
                        if let Some(position) = cell {
                            self.extend_selection(position);
                        }
                    } else if let Some(position) = cell {
                        let active_press =
                            self.pressed_buttons.values().find_map(application_motion);
                        let report = if let Some((code, sgr, modifiers)) = active_press {
                            mouse_report(MouseAction::Motion(code), position, modifiers, sgr)
                        } else {
                            let modes = self.input_modes();
                            (modes.mouse_tracking == MouseTracking::Any)
                                .then(|| {
                                    mouse_report(
                                        MouseAction::Motion(3),
                                        position,
                                        self.modifiers,
                                        modes.sgr_mouse,
                                    )
                                })
                                .flatten()
                        };
                        if let Some(report) = report {
                            self.send_coalescible_input(report);
                        }
                    }
                }
                PointerEventKind::Press { button, serial, .. } => {
                    self.last_pointer_serial = Some(serial);
                    if let Some(splint_id) = self.splint_at_point(event.position) {
                        if self.focus_splint(splint_id) {
                            cell = self.pointer_cell_at(event.position);
                            self.pane.pointer_cell = cell;
                            self.update_ime_cursor_rectangle();
                            if let Err(error) = self.schedule_draw(queue_handle) {
                                self.fail(error);
                                return;
                            }
                        }
                    }
                    if button == BTN_LEFT
                        && history_return_to_live_hit(
                            event.position,
                            self.logical_width,
                            self.logical_height,
                            !self.pane.scrollback_viewport.is_live(),
                        )
                    {
                        if let Err(error) = self
                            .scroll_history(MouseAction::WheelDown, usize::MAX)
                            .and_then(|moved| {
                                if moved && self.configured {
                                    self.schedule_draw(queue_handle)?;
                                }
                                Ok(())
                            })
                        {
                            self.fail(error);
                        }
                        continue;
                    }
                    if self.trusted_consent.is_some() && button == BTN_LEFT {
                        let (x, y) = event.position;
                        if y >= f64::from(self.logical_height) * 0.78 {
                            self.decide_consent(x >= f64::from(self.logical_width) / 2.0);
                        }
                        continue;
                    }
                    self.pane.pointer_cell = cell;
                    self.recompute_hovered_url();
                    let owner = classify_press(
                        button,
                        cell.is_some(),
                        self.modifiers,
                        self.input_modes(),
                        self.pane.hovered_url.is_some(),
                    );
                    self.pressed_buttons.insert(button, owner);
                    match owner {
                        PressOwner::Application {
                            code,
                            tracking: _,
                            sgr,
                            modifiers,
                        } => {
                            if let Some(position) = cell {
                                if let Some(report) =
                                    mouse_report(MouseAction::Press(code), position, modifiers, sgr)
                                {
                                    self.send_command(WindowCommand::Input(report));
                                }
                            }
                        }
                        PressOwner::Selection => {
                            if let Some(position) = cell {
                                self.begin_selection(position);
                            }
                        }
                        PressOwner::PrimaryPaste => {
                            self.begin_clipboard_read(PasteTarget::Primary);
                        }
                        PressOwner::Url => self.open_hovered_url(),
                        PressOwner::Ignored => {}
                    }
                }
                PointerEventKind::Release { button, serial, .. } => {
                    self.last_pointer_serial = Some(serial);
                    match take_press_owner(&mut self.pressed_buttons, button) {
                        PressOwner::Application {
                            code,
                            tracking: _,
                            sgr,
                            modifiers,
                        } => {
                            if let Some(position) = cell.or(self.pane.pointer_cell) {
                                if let Some(report) = mouse_report(
                                    MouseAction::Release(code),
                                    position,
                                    modifiers,
                                    sgr,
                                ) {
                                    self.send_command(WindowCommand::Input(report));
                                }
                            }
                        }
                        PressOwner::Selection => {
                            self.finish_selection();
                            self.publish_clipboard(queue_handle, serial, true);
                        }
                        PressOwner::PrimaryPaste | PressOwner::Url | PressOwner::Ignored => {}
                    }
                }
                PointerEventKind::Axis {
                    horizontal: _,
                    vertical,
                    ..
                } => {
                    // Xterm's mouse protocol has only vertical wheel button codes 4/5;
                    // Foot does not synthesize horizontal wheel reports into unrelated buttons.
                    if vertical.is_none() {
                        continue;
                    }
                    if let Err(error) = self.handle_vertical_wheel(
                        cell,
                        vertical.absolute,
                        vertical.discrete,
                        vertical.value120,
                    ) {
                        self.fail(error);
                    }
                }
            }
            if previous_hover != self.pane.hovered_url {
                if let Some((start, _, _)) = previous_hover {
                    self.dirty_row(start.row);
                }
                if let Some((start, _, _)) = &self.pane.hovered_url {
                    self.dirty_row(start.row);
                }
            }
        }
        if self.configured
            && (self.pane.viewport_dirty
                || self.pane.raster_dirty_rows.iter().any(|dirty| *dirty)
                || self.pane.surface_dirty_rows.iter().any(|dirty| *dirty))
        {
            if let Err(error) = self.schedule_draw(queue_handle) {
                self.fail(error);
            }
        }
    }
}

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        data_device: &wl_data_device::WlDataDevice,
        _x: f64,
        _y: f64,
        _surface: &wl_surface::WlSurface,
    ) {
        if let Some(offer) = self
            .data_device
            .as_ref()
            .filter(|device| device.inner() == data_device)
            .and_then(|device| device.data().drag_offer())
        {
            offer.accept_mime_type(0, None);
            offer.set_actions(DndAction::empty(), DndAction::empty());
        }
    }

    fn leave(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _data_device: &wl_data_device::WlDataDevice,
    ) {
    }

    fn motion(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _data_device: &wl_data_device::WlDataDevice,
        _x: f64,
        _y: f64,
    ) {
    }

    fn selection(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        data_device: &wl_data_device::WlDataDevice,
    ) {
        self.clipboard_offer = self
            .data_device
            .as_ref()
            .filter(|device| device.inner() == data_device)
            .and_then(|device| device.data().selection_offer())
            .filter(|offer| offer.with_mime_types(accepted_text_mime).is_some());
    }

    fn drop_performed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _data_device: &wl_data_device::WlDataDevice,
    ) {
    }
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        offer: &mut DragOffer,
        _actions: DndAction,
    ) {
        offer.set_actions(DndAction::empty(), DndAction::empty());
    }

    fn selected_action(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _offer: &mut DragOffer,
        _actions: DndAction,
    ) {
    }
}

fn write_selection_payload(write_pipe: WritePipe, payload: Arc<[u8]>) {
    let Some(permit) = try_clipboard_worker(&ACTIVE_CLIPBOARD_WORKERS) else {
        return;
    };
    std::thread::spawn(move || {
        let _permit = permit;
        let fd = OwnedFd::from(write_pipe);
        let _ = write_clipboard_with_deadline(&fd, &payload, CLIPBOARD_IO_TIMEOUT);
    });
}

impl DataSourceHandler for App {
    fn accept_mime(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
        _mime: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &wl_data_source::WlDataSource,
        mime: String,
        write_pipe: WritePipe,
    ) {
        if accepted_text_mime(std::slice::from_ref(&mime)).is_none() {
            return;
        }
        if let Some((_, payload)) = self
            .clipboard_sources
            .iter()
            .find(|(candidate, _)| candidate.inner() == source)
        {
            write_selection_payload(write_pipe, Arc::clone(payload));
        }
    }

    fn cancelled(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &wl_data_source::WlDataSource,
    ) {
        self.clipboard_sources
            .retain(|(candidate, _)| candidate.inner() != source);
    }

    fn dnd_dropped(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
    ) {
    }

    fn dnd_finished(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
    ) {
    }

    fn action(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _source: &wl_data_source::WlDataSource,
        _action: DndAction,
    ) {
    }
}

impl PrimarySelectionDeviceHandler for App {
    fn selection(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        device: &ZwpPrimarySelectionDeviceV1,
    ) {
        self.primary_offer = self
            .primary_device
            .as_ref()
            .filter(|candidate| candidate.inner() == device)
            .and_then(|candidate| candidate.data().selection_offer())
            .filter(|offer| offer.with_mime_types(accepted_text_mime).is_some());
    }
}

impl PrimarySelectionSourceHandler for App {
    fn send_request(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
        mime: String,
        write_pipe: WritePipe,
    ) {
        if accepted_text_mime(std::slice::from_ref(&mime)).is_none() {
            return;
        }
        if let Some((_, payload)) = self
            .primary_sources
            .iter()
            .find(|(candidate, _)| candidate.inner() == source)
        {
            write_selection_payload(write_pipe, Arc::clone(payload));
        }
    }

    fn cancelled(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
    ) {
        self.primary_sources
            .retain(|(candidate, _)| candidate.inner() != source);
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
    fn update_output(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self.entered_outputs.last() == Some(&output) {
            if let Err(error) = self.refresh_output_dpi(&output, queue_handle) {
                self.fail(error);
            }
        }
    }
    fn output_destroyed(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let was_most_recent = note_output_leave(&mut self.entered_outputs, &output);
        self.output_count = self.entered_outputs.len();
        if was_most_recent {
            if let Some(current) = self.entered_outputs.last().cloned() {
                if let Err(error) = self.refresh_output_dpi(&current, queue_handle) {
                    self.fail(error);
                }
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WpFractionalScaleManagerV1,
        _event: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewporter, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTextInputManagerV3, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpTextInputManagerV3,
        _event: <ZwpTextInputManagerV3 as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            if let Err(error) = state.apply_scale(scale, queue_handle) {
                state.fail(error);
            }
        }
    }
}

impl Dispatch<ZwpTextInputV3, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { surface } => {
                if surface == *state.window.wl_surface() {
                    state.ime.entered = true;
                    if state.keyboard_focused {
                        state.enable_text_input();
                    }
                }
            }
            zwp_text_input_v3::Event::Leave { surface } => {
                if surface == *state.window.wl_surface() {
                    state.ime.entered = false;
                    state.clear_ime_preedit();
                    if state.configured {
                        if let Err(error) = state.schedule_draw(queue_handle) {
                            state.fail(error);
                        }
                    }
                }
            }
            zwp_text_input_v3::Event::PreeditString { text, .. } => {
                state.ime.set_preedit(text);
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                state.ime.set_commit(text);
            }
            zwp_text_input_v3::Event::Done { serial } => {
                let (_serial_matches, _, commit) = state.ime.finish(serial);
                if let Some(commit) = commit {
                    state.send_command(WindowCommand::Input(commit.into_bytes()));
                }
                if let Err(error) = state.refresh_ime_preedit() {
                    state.fail(error);
                    return;
                }
                if state.configured {
                    if let Err(error) = state.schedule_draw(queue_handle) {
                        state.fail(error);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use splinterm_core::{Axis, Splint, SplitRatio};
    use splinterm_protocol::ActiveScreen;

    fn update_test_waker() -> (EventLoop<'static, usize>, Waker) {
        let event_loop = EventLoop::<usize>::try_new().unwrap();
        let (ping, source) = make_ping().unwrap();
        event_loop
            .handle()
            .insert_source(source, |(), _, wake_count| *wake_count += 1)
            .unwrap();
        (event_loop, Waker::from(Arc::new(UpdateWake(ping))))
    }

    #[test]
    fn terminal_draw_bypasses_delayed_frame_only_with_a_released_buffer() {
        assert!(!terminal_draw_waits_for_frame(false, false));
        assert!(!terminal_draw_waits_for_frame(false, true));
        assert!(!terminal_draw_waits_for_frame(true, true));
        assert!(terminal_draw_waits_for_frame(true, false));
    }

    #[test]
    fn terminal_priority_retries_pending_draw_before_delayed_frame() {
        assert!(!pending_draw_waits_for_frame(false, false));
        assert!(!pending_draw_waits_for_frame(false, true));
        assert!(pending_draw_waits_for_frame(true, false));
        assert!(!pending_draw_waits_for_frame(true, true));
    }

    #[test]
    fn graphical_input_probe_counts_only_distinct_committed_revisions() {
        let splint_id = SplintId::new();
        let mut probe = GraphicalInputProbe {
            target_revisions: 3,
            observed_revisions: HashSet::with_capacity(3),
        };
        assert!(!probe.observe_commit(None));
        assert!(!probe.observe_commit(Some((splint_id, 1, 7))));
        assert!(!probe.observe_commit(Some((splint_id, 1, 8))));
        assert!(!probe.observe_commit(Some((splint_id, 1, 7))));
        assert!(probe.observe_commit(Some((splint_id, 1, 9))));
    }

    #[test]
    fn pending_update_receiver_wakes_calloop_and_coalesces_pings() {
        let mut event_loop = EventLoop::<usize>::try_new().unwrap();
        let (ping, source) = make_ping().unwrap();
        event_loop
            .handle()
            .insert_source(source, |(), _, wake_count| *wake_count += 1)
            .unwrap();
        let waker = Waker::from(Arc::new(UpdateWake(ping)));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);

        assert!(matches!(
            poll_receiver(&mut receiver, &waker),
            ReceiverPoll::Pending
        ));
        sender.try_send(1).unwrap();
        sender.try_send(2).unwrap();

        let mut wake_count = 0;
        event_loop
            .dispatch(Duration::ZERO, &mut wake_count)
            .unwrap();
        assert_eq!(wake_count, 1);
        assert!(matches!(
            poll_receiver(&mut receiver, &waker),
            ReceiverPoll::Item(1)
        ));
        assert!(matches!(
            poll_receiver(&mut receiver, &waker),
            ReceiverPoll::Item(2)
        ));
        assert!(matches!(
            poll_receiver(&mut receiver, &waker),
            ReceiverPoll::Pending
        ));
    }

    #[test]
    fn receiver_drain_yields_at_budget_and_rearms_calloop() {
        let mut event_loop = EventLoop::<usize>::try_new().unwrap();
        let (ping, source) = make_ping().unwrap();
        event_loop
            .handle()
            .insert_source(source, |(), _, wake_count| *wake_count += 1)
            .unwrap();
        let waker = Waker::from(Arc::new(UpdateWake(ping)));
        let item_count = RECEIVER_DRAIN_BUDGET * 2;
        let (sender, mut receiver) = tokio::sync::mpsc::channel(item_count);
        for item in 0..item_count {
            sender.try_send(item).unwrap();
        }

        let first = drain_receiver(&mut receiver, &waker);
        assert_eq!(first.items, (0..RECEIVER_DRAIN_BUDGET).collect::<Vec<_>>());
        assert!(!first.disconnected);
        assert_eq!(receiver.len(), RECEIVER_DRAIN_BUDGET);

        let mut wake_count = 0;
        event_loop
            .dispatch(Duration::ZERO, &mut wake_count)
            .unwrap();
        assert_eq!(wake_count, 1);

        let second = drain_receiver(&mut receiver, &waker);
        assert_eq!(
            second.items,
            (RECEIVER_DRAIN_BUDGET..item_count).collect::<Vec<_>>()
        );
        assert!(!second.disconnected);
    }

    #[test]
    fn receiver_drain_reports_boundary_disconnect_after_finite_tail() {
        let (_event_loop, waker) = update_test_waker();
        let item_count = RECEIVER_DRAIN_BUDGET + 4;
        let (sender, mut receiver) = tokio::sync::mpsc::channel(item_count);
        for item in 0..item_count {
            sender.try_send(item).unwrap();
        }
        drop(sender);

        let drained = drain_receiver(&mut receiver, &waker);
        assert_eq!(drained.items, (0..item_count).collect::<Vec<_>>());
        assert!(drained.disconnected);
    }

    #[test]
    fn receiver_drain_yields_to_a_concurrently_refilling_producer() {
        let event_loop = EventLoop::<usize>::try_new().unwrap();
        let (ping, source) = make_ping().unwrap();
        event_loop
            .handle()
            .insert_source(source, |(), _, wake_count| *wake_count += 1)
            .unwrap();
        let waker = Waker::from(Arc::new(UpdateWake(ping)));
        let item_count = RECEIVER_DRAIN_BUDGET * 8;
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let producer = std::thread::spawn(move || {
            for item in 0..item_count {
                sender.blocking_send(item).unwrap();
            }
        });
        let mut items_seen = Vec::with_capacity(item_count);
        let mut disconnected = false;
        while !disconnected {
            let drained = drain_receiver(&mut receiver, &waker);
            assert!(drained.items.len() <= RECEIVER_DRAIN_BUDGET);
            items_seen.extend(drained.items);
            disconnected = drained.disconnected;
            std::thread::yield_now();
        }
        producer.join().unwrap();
        assert_eq!(items_seen, (0..item_count).collect::<Vec<_>>());
    }

    #[test]
    fn full_redraw_damage_survives_redraw_state_reset() {
        let mut full_redraw = true;
        assert!(take_full_surface_damage(&mut full_redraw, true));
        assert!(!full_redraw);

        assert!(!take_full_surface_damage(&mut full_redraw, true));
        assert!(take_full_surface_damage(&mut full_redraw, false));
    }

    #[test]
    fn deterministic_capture_waits_for_required_images_and_scale() {
        assert!(deterministic_capture_ready(true, 0, 0));
        assert!(!deterministic_capture_ready(false, 0, 2));
        assert!(!deterministic_capture_ready(true, 2, 1));
        assert!(deterministic_capture_ready(true, 2, 2));
    }

    fn snapshot(splint_id: SplintId, incarnation: u64, revision: u64) -> TerminalSnapshot {
        TerminalSnapshot {
            splint_id,
            incarnation,
            revision,
            columns: 0,
            rows: 0,
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
                cursor_blink: true,
                mouse_tracking: MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
            title: String::new(),
            visible_rows: Vec::new(),
            history_generation: 1,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            images: None,
            exited_code: None,
            exited_signal: None,
        }
    }

    fn valid_snapshot(splint_id: SplintId) -> TerminalSnapshot {
        let mut snapshot = snapshot(splint_id, 1, 1);
        snapshot.columns = 1;
        snapshot.rows = 1;
        let mut row = blank_row(1);
        row.row_id = Some(1);
        snapshot.visible_rows = vec![row];
        snapshot
    }

    fn pane_options(splint_id: SplintId) -> WindowPaneOptions {
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, _command_receiver) = tokio::sync::mpsc::channel(1);
        WindowPaneOptions {
            snapshot: valid_snapshot(splint_id),
            updates: update_receiver,
            commands,
            authority: AuthorityStatus::default(),
            controlled: false,
            image_sources: ImageContentLeaseSet::default(),
        }
    }

    #[test]
    fn frame_corner_cells_contain_only_corner_masks() {
        let splint = Splint::shell(PathBuf::from("/tmp"));
        let splint_id = splint.id;
        let layout = PaneLayout::compute_with_chrome(
            &LayoutNode::Leaf(splint),
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 64,
            },
            PaneChrome::Frame {
                vertical_width: 8,
                horizontal_height: 16,
            },
            2,
            2,
        )
        .unwrap();
        let mut actual = vec![0; 80 * 64 * 4];
        let theme = ResolvedTheme::default();
        paint_pane_chrome(
            &mut actual,
            80,
            64,
            &layout,
            Some(splint_id),
            theme,
            8,
            16,
            120,
            &HashMap::new(),
        )
        .unwrap();

        let corner = Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let mut expected = vec![0; 80 * 64 * 4];
        paint_box_drawing_cell(
            &mut expected,
            80,
            64,
            '┌',
            corner,
            corner,
            theme.pane_border_active,
            120,
        );
        let region = |canvas: &[u8]| {
            (0_usize..16)
                .flat_map(|y| {
                    let start = y * 80 * 4;
                    canvas[start..start + 8 * 4].to_vec()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(region(&actual), region(&expected));
    }

    #[test]
    fn frame_titles_sanitize_controls_collapse_space_and_keep_complete_widths() {
        assert_eq!(
            sanitize_frame_title("  editor\n\t logs  ", 20),
            "editor logs"
        );
        assert_eq!(sanitize_frame_title("界 shell", 4), "界 s");
        assert_eq!(sanitize_frame_title("e\u{301}ditor", 1), "e\u{301}");
        assert_eq!(sanitize_frame_title("ignored", 0), "");
    }

    #[test]
    fn nested_line_dividers_resolve_all_tee_orientations() {
        let vertical = PaneDivider {
            axis: Axis::Horizontal,
            rect: Rect {
                x: 40,
                y: 0,
                width: 8,
                height: 100,
            },
        };
        let from_right = PaneDivider {
            axis: Axis::Vertical,
            rect: Rect {
                x: 48,
                y: 30,
                width: 52,
                height: 16,
            },
        };
        let from_left = PaneDivider {
            rect: Rect {
                x: 0,
                width: 40,
                ..from_right.rect
            },
            ..from_right
        };
        assert_eq!(divider_junction(vertical, from_right).unwrap().0, '├');
        assert_eq!(divider_junction(vertical, from_left).unwrap().0, '┤');

        let horizontal = PaneDivider {
            axis: Axis::Vertical,
            rect: Rect {
                x: 0,
                y: 40,
                width: 100,
                height: 16,
            },
        };
        let from_bottom = PaneDivider {
            axis: Axis::Horizontal,
            rect: Rect {
                x: 30,
                y: 56,
                width: 8,
                height: 44,
            },
        };
        let from_top = PaneDivider {
            rect: Rect {
                y: 0,
                height: 40,
                ..from_bottom.rect
            },
            ..from_bottom
        };
        assert_eq!(divider_junction(horizontal, from_bottom).unwrap().0, '┬');
        assert_eq!(divider_junction(horizontal, from_top).unwrap().0, '┴');
    }

    #[test]
    fn multi_pane_inputs_select_local_focus_and_reject_identity_mismatch() {
        let first = Splint::shell(PathBuf::from("/tmp"));
        let first_id = first.id;
        let second = Splint::shell(PathBuf::from("/tmp"));
        let second_id = second.id;
        let layout = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(LayoutNode::Leaf(first)),
            second: Box::new(LayoutNode::Leaf(second)),
        };
        let mut options = WindowOptions {
            panes: vec![pane_options(first_id), pane_options(second_id)],
            layout: Some(layout.clone()),
            active_splint: Some(second_id),
            ..WindowOptions::default()
        };
        let inactive = options.activate_multi_pane_input().unwrap();
        assert_eq!(
            options.snapshot.as_ref().map(|snapshot| snapshot.splint_id),
            Some(second_id)
        );
        assert_eq!(inactive.len(), 1);
        assert_eq!(inactive[0].snapshot.splint_id, first_id);

        let mut invalid = WindowOptions {
            panes: vec![pane_options(first_id), pane_options(first_id)],
            layout: Some(layout),
            ..WindowOptions::default()
        };
        assert!(invalid.activate_multi_pane_input().is_err());
    }

    #[test]
    fn pane_focus_bindings_are_explicit_and_do_not_capture_plain_arrows() {
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            pane_focus_action(Keysym::Left, modifiers),
            Some(PaneFocusAction::Direction(FocusDirection::Left))
        );
        assert_eq!(
            pane_focus_action(Keysym::Tab, modifiers),
            Some(PaneFocusAction::Next { reverse: false })
        );
        assert_eq!(pane_focus_action(Keysym::Left, Modifiers::default()), None);
        assert_eq!(
            pane_topology_action(Keysym::Return, modifiers),
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Horizontal))
        );
        assert_eq!(
            pane_topology_action(Keysym::bar, modifiers),
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Vertical))
        );
        assert_eq!(
            pane_topology_action(Keysym::W, modifiers),
            Some(PaneTopologyAction::Close)
        );
        assert_eq!(
            pane_topology_action(Keysym::braceleft, modifiers),
            Some(PaneTopologyAction::AdjustRatio(-50))
        );
        assert_eq!(
            pane_topology_action(Keysym::braceright, modifiers),
            Some(PaneTopologyAction::AdjustRatio(50))
        );
        assert_eq!(pane_topology_action(Keysym::w, Modifiers::default()), None);
        assert_eq!(
            offset_cursor_rectangle(
                (7, 11, 8, 16),
                Rect {
                    x: 100,
                    y: 200,
                    width: 400,
                    height: 300,
                },
            ),
            Some((107, 211, 8, 16))
        );
    }

    #[test]
    fn inactive_pane_reducer_applies_only_contiguous_matching_updates() {
        let splint_id = SplintId::new();
        let mut pane = PaneView::from_options(pane_options(splint_id), SCALE_DENOMINATOR).unwrap();
        let mut update = empty_update();
        update.title = Some("background pane".into());
        assert!(
            pane.apply_background_update(
                WindowUpdate::Update {
                    update,
                    image_sources: None,
                },
                ResolvedTheme::default(),
                SCALE_DENOMINATOR,
            )
            .unwrap()
        );
        assert_eq!(pane.snapshot.as_ref().unwrap().revision, 2);
        assert_eq!(pane.snapshot.as_ref().unwrap().title, "background pane");

        let mut stale = empty_update();
        stale.base_revision = 1;
        stale.revision = 3;
        assert!(
            pane.apply_background_update(
                WindowUpdate::Update {
                    update: stale,
                    image_sources: None,
                },
                ResolvedTheme::default(),
                SCALE_DENOMINATOR,
            )
            .is_err()
        );
    }

    #[test]
    fn inactive_detached_pane_stays_anchored_across_new_output() {
        let splint_id = SplintId::new();
        let mut initial = valid_snapshot(splint_id);
        initial.visible_rows[0].row_id = Some(3);
        initial.visible_rows[0].cells[0].content = "visible-three".into();
        initial.scrollback_rows = vec![history_row(1, 0), history_row(2, 0)];
        initial.available_scrollback_rows = 2;
        initial.oldest_available_scrollback_row_id = Some(1);
        initial.newest_available_scrollback_row_id = Some(2);
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, _command_receiver) = tokio::sync::mpsc::channel(1);
        let mut pane = PaneView::from_options(
            WindowPaneOptions {
                snapshot: initial,
                updates: update_receiver,
                commands,
                authority: AuthorityStatus::default(),
                controlled: false,
                image_sources: ImageContentLeaseSet::default(),
            },
            SCALE_DENOMINATOR,
        )
        .unwrap();
        pane.scrollback_viewport
            .scroll_up(1, pane.snapshot.as_ref().unwrap());

        let mut next = pane.snapshot.as_ref().unwrap().clone();
        next.revision = 2;
        next.scrollback_rows.push(next.visible_rows[0].clone());
        next.available_scrollback_rows = 3;
        next.newest_available_scrollback_row_id = Some(3);
        next.visible_rows[0].row_id = Some(4);
        next.visible_rows[0].cells[0].content = "live-four".into();
        pane.apply_background_update(
            WindowUpdate::Snapshot {
                snapshot: next,
                image_sources: ImageContentLeaseSet::default(),
            },
            ResolvedTheme::default(),
            SCALE_DENOMINATOR,
        )
        .unwrap();

        assert!(!pane.scrollback_viewport.is_live());
        let display = pane.display_snapshot().unwrap();
        assert_eq!(display.visible_rows[0].row_id, Some(2));
        assert_ne!(display.visible_rows[0].cells[0].content, "live-four");
    }

    #[test]
    fn active_and_inactive_pane_frames_rebuild_at_fractional_scale() {
        let mut active = PaneView::from_options(pane_options(SplintId::new()), 120).unwrap();
        let mut inactive = PaneView::from_options(pane_options(SplintId::new()), 120).unwrap();
        assert!(rebuild_pane_scaled_frame(&mut active, 150).unwrap());
        assert!(rebuild_pane_scaled_frame(&mut inactive, 150).unwrap());
        assert_eq!(active.snapshot_frame.as_ref().unwrap().scale_120(), 150);
        assert_eq!(inactive.snapshot_frame.as_ref().unwrap().scale_120(), 150);
    }

    #[test]
    fn pane_resize_uses_its_local_rectangle_and_suppresses_duplicates() {
        let splint_id = SplintId::new();
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, mut command_receiver) = tokio::sync::mpsc::channel(2);
        let mut pane = PaneView::from_options(
            WindowPaneOptions {
                snapshot: valid_snapshot(splint_id),
                updates: update_receiver,
                commands,
                authority: AuthorityStatus::default(),
                controlled: false,
                image_sources: ImageContentLeaseSet::default(),
            },
            SCALE_DENOMINATOR,
        )
        .unwrap();
        App::emit_pane_resize(&mut pane, 320, 240, SCALE_DENOMINATOR, true).unwrap();
        let first = command_receiver.try_recv().unwrap();
        assert!(matches!(first, WindowCommand::Resize { .. }));
        App::emit_pane_resize(&mut pane, 320, 240, SCALE_DENOMINATOR, true).unwrap();
        assert!(command_receiver.try_recv().is_err());
    }

    fn history_row(id: u64, content_bytes: usize) -> TerminalRow {
        let mut row = blank_row(1);
        row.row_id = Some(id);
        row.cells[0].content = "x".repeat(content_bytes);
        row
    }

    #[test]
    fn history_cache_enforces_row_budget_from_either_edge() {
        let mut newest = (1..=u64::try_from(MAX_CACHED_HISTORY_ROWS + 4).unwrap())
            .map(|id| history_row(id, 0))
            .collect::<Vec<_>>();
        bound_history_cache(&mut newest, false);
        assert_eq!(newest.len(), MAX_CACHED_HISTORY_ROWS);
        assert_eq!(newest.first().and_then(|row| row.row_id), Some(5));

        let mut oldest = (1..=u64::try_from(MAX_CACHED_HISTORY_ROWS + 4).unwrap())
            .map(|id| history_row(id, 0))
            .collect::<Vec<_>>();
        bound_history_cache(&mut oldest, true);
        assert_eq!(oldest.len(), MAX_CACHED_HISTORY_ROWS);
        assert_eq!(oldest.last().and_then(|row| row.row_id), Some(4096));
    }

    #[test]
    fn history_cache_enforces_byte_budget_and_preserves_order() {
        let mut rows = (1..=20)
            .map(|id| history_row(id, 1024 * 1024))
            .collect::<Vec<_>>();
        bound_history_cache(&mut rows, false);
        assert!(history_cache_bytes(&rows) <= MAX_CACHED_HISTORY_BYTES);
        assert!(rows.windows(2).all(|pair| pair[0].row_id < pair[1].row_id));
        assert_eq!(rows.last().and_then(|row| row.row_id), Some(20));
    }

    #[test]
    fn omitted_history_tracks_the_stable_cache_window_position() {
        let rows = |first| {
            (first..first + 100)
                .map(|id| history_row(id, 0))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            omitted_rows_before_cache(Some(100), &rows(1_000), 1_000),
            900
        );
        assert_eq!(omitted_rows_before_cache(Some(100), &rows(500), 1_000), 400);
        assert_eq!(omitted_rows_before_cache(Some(100), &rows(100), 1_000), 0);
        assert_eq!(omitted_rows_before_cache(None, &rows(1_000), 1_000), 900);
    }

    #[test]
    fn same_generation_trim_discards_cached_rows_before_daemon_oldest() {
        let mut current = snapshot(SplintId::new(), 1, 10);
        current.columns = 1;
        current.rows = 1;
        current.visible_rows = vec![blank_row(1)];
        current.scrollback_rows = (1..=4).map(|id| history_row(id, 0)).collect();
        current.available_scrollback_rows = 4;
        current.oldest_available_scrollback_row_id = Some(1);
        current.newest_available_scrollback_row_id = Some(4);

        apply_scrollback_update(
            &mut current,
            splinterm_protocol::TerminalScrollbackUpdate {
                transition: HistoryTransition::Append {
                    appended_rows: 2,
                    trimmed_rows: 2,
                },
                history_generation: 1,
                oldest_available_row_id: Some(3),
                newest_available_row_id: Some(6),
                rows: vec![history_row(5, 0), history_row(6, 0)],
                available_rows: 4,
                omitted_oldest_rows: 2,
            },
        )
        .unwrap();

        assert_eq!(
            current
                .scrollback_rows
                .iter()
                .filter_map(|row| row.row_id)
                .collect::<Vec<_>>(),
            vec![3, 4, 5, 6]
        );
        assert_eq!(current.omitted_oldest_scrollback_rows, 0);
    }

    #[test]
    fn semantic_update_applies_exact_row_cursor_and_title_revision() {
        let mut current = snapshot(SplintId::new(), 7, 10);
        current.columns = 2;
        current.rows = 1;
        current.visible_rows = vec![blank_row(2)];
        let row = TerminalRow {
            row_id: Some(8),
            linebreak: true,
            cells: vec![TerminalCell {
                content: "x".into(),
                spacer_remaining: None,
                attributes: current.visible_rows[0].cells[0].attributes,
            }],
        };
        apply_terminal_update(
            &mut current,
            TerminalUpdate {
                base_revision: 10,
                revision: 11,
                rows: vec![splinterm_protocol::TerminalRowPatch {
                    index: 0,
                    row: row.clone(),
                }],
                scrolls: Vec::new(),
                cursor: Some(splinterm_protocol::TerminalCursor {
                    column: 1,
                    row: 0,
                    deferred_wrap: true,
                }),
                title: Some("revision eleven".into()),
                input_modes: None,
                active_screen: None,
                palette: None,
                default_colors: None,
                columns: None,
                row_count: None,
                scrollback: Some(splinterm_protocol::TerminalScrollbackUpdate {
                    transition: splinterm_protocol::HistoryTransition::Reflow,
                    history_generation: 2,
                    oldest_available_row_id: Some(7),
                    newest_available_row_id: Some(7),
                    rows: vec![TerminalRow {
                        row_id: Some(7),
                        linebreak: true,
                        cells: Vec::new(),
                    }],
                    available_rows: 1,
                    omitted_oldest_rows: 0,
                }),
                images: Some(Box::new(splinterm_protocol::TerminalImagePlane {
                    screen: splinterm_protocol::ActiveScreen::Normal,
                    contents: Vec::new(),
                    placements: Vec::new(),
                })),
            },
        )
        .expect("contiguous semantic update");
        assert_eq!(current.revision, 11);
        assert_eq!(current.visible_rows[0], row);
        assert_eq!((current.cursor_column, current.cursor_row), (1, 0));
        assert!(current.cursor_deferred_wrap);
        assert_eq!(current.title, "revision eleven");
        assert_eq!(current.history_generation, 2);
        assert_eq!(current.oldest_available_scrollback_row_id, Some(7));
        assert_eq!(current.newest_available_scrollback_row_id, Some(7));
        assert_eq!(current.available_scrollback_rows, 1);
        assert_eq!(current.scrollback_rows[0].row_id, Some(7));
        assert!(current.images.is_some());
    }

    #[test]
    fn snapshot_order_accepts_only_newer_matching_identity() {
        let splint_id = SplintId::new();
        let current = snapshot(splint_id, 7, 10);
        assert!(snapshot_is_newer(&current, &snapshot(splint_id, 7, 11)).expect("matching"));
        assert!(!snapshot_is_newer(&current, &snapshot(splint_id, 7, 10)).expect("duplicate"));
        assert!(!snapshot_is_newer(&current, &snapshot(splint_id, 7, 9)).expect("stale"));
        assert!(snapshot_is_newer(&current, &snapshot(SplintId::new(), 7, 11)).is_err());
        assert!(snapshot_is_newer(&current, &snapshot(splint_id, 8, 11)).is_err());
    }

    #[test]
    fn pending_snapshots_coalesce_to_newest_revision() {
        let splint_id = SplintId::new();
        let current = snapshot(splint_id, 2, 10);
        let latest = coalesce_snapshots(
            Some(&current),
            [
                snapshot(splint_id, 2, 11),
                snapshot(splint_id, 2, 13),
                snapshot(splint_id, 2, 12),
            ],
        )
        .expect("matching snapshots")
        .expect("newer snapshot");
        assert_eq!(latest.revision, 13);
        assert!(coalesce_snapshots(Some(&current), [snapshot(SplintId::new(), 2, 14)]).is_err());
    }

    fn normal_modes() -> TerminalInputModes {
        TerminalInputModes {
            application_cursor: false,
            application_keypad: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor_visible: true,
            cursor_blink: true,
            mouse_tracking: MouseTracking::None,
            sgr_mouse: false,
        }
    }

    fn empty_update() -> TerminalUpdate {
        TerminalUpdate {
            base_revision: 1,
            revision: 2,
            rows: Vec::new(),
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

    fn encoded(keysym: Keysym, utf8: Option<&str>, modifiers: Modifiers) -> Option<Vec<u8>> {
        key_input(keysym, utf8, modifiers, normal_modes())
    }

    #[test]
    fn viewport_cursor_moves_with_content_until_it_leaves_the_grid() {
        assert_eq!(viewport_cursor_row(2, 0, 6), Some(2));
        assert_eq!(viewport_cursor_row(2, 3, 6), Some(5));
        assert_eq!(viewport_cursor_row(2, 4, 6), None);
        assert_eq!(viewport_cursor_row(-1, 3, 6), None);
    }

    #[test]
    fn local_scrollback_uses_foot_default_wheel_multiplier() {
        let mut wheel = WheelAccumulator::default();
        assert_eq!(
            wheel.push_scaled(0.0, 0, -40, SCROLLBACK_WHEEL_MULTIPLIER, 29),
            Some((MouseAction::WheelUp, 1))
        );
        assert_eq!(
            wheel.push_scaled(0.0, 1, 0, SCROLLBACK_WHEEL_MULTIPLIER, 29),
            Some((MouseAction::WheelDown, 3))
        );
    }

    #[test]
    fn history_navigation_requires_shift_and_detached_end() {
        assert_eq!(
            history_navigation(Keysym::Page_Up, true, false),
            Some(HistoryNavigation::PageUp)
        );
        assert_eq!(
            history_navigation(Keysym::Page_Down, true, true),
            Some(HistoryNavigation::PageDown)
        );
        assert_eq!(history_navigation(Keysym::Page_Up, false, true), None);
        assert_eq!(history_navigation(Keysym::End, true, false), None);
        assert_eq!(
            history_navigation(Keysym::End, true, true),
            Some(HistoryNavigation::ReturnToLive)
        );
    }

    #[test]
    fn foot_font_zoom_bindings_require_control_and_cover_reset() {
        let control = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            font_zoom_action(Keysym::plus, control),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(Keysym::equal, control),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(Keysym::KP_Add, control),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(Keysym::minus, control),
            Some(FontZoomAction::Decrease)
        );
        assert_eq!(
            font_zoom_action(Keysym::_0, control),
            Some(FontZoomAction::Reset)
        );
        assert_eq!(font_zoom_action(Keysym::plus, Modifiers::default()), None);
        assert_eq!(
            font_zoom_action(
                Keysym::plus,
                Modifiers {
                    alt: true,
                    ..control
                }
            ),
            None
        );
    }

    #[test]
    fn essential_key_mapping_uses_utf8_modifiers_and_terminal_sequences() {
        let plain = Modifiers::default();
        assert_eq!(encoded(Keysym::a, Some("a"), plain), Some(b"a".to_vec()));
        assert_eq!(encoded(Keysym::Return, None, plain), Some(vec![b'\r']));
        assert_eq!(encoded(Keysym::BackSpace, None, plain), Some(vec![0x7f]));
        assert_eq!(encoded(Keysym::Tab, None, plain), Some(vec![b'\t']));
        assert_eq!(encoded(Keysym::Escape, None, plain), Some(vec![0x1b]));
        assert_eq!(encoded(Keysym::Up, None, plain), Some(b"\x1b[A".to_vec()));
        assert_eq!(encoded(Keysym::Down, None, plain), Some(b"\x1b[B".to_vec()));
        assert_eq!(encoded(Keysym::Left, None, plain), Some(b"\x1b[D".to_vec()));
        assert_eq!(
            encoded(Keysym::Right, None, plain),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(encoded(Keysym::Home, None, plain), Some(b"\x1b[H".to_vec()));
        assert_eq!(encoded(Keysym::End, None, plain), Some(b"\x1b[F".to_vec()));
        assert_eq!(
            encoded(Keysym::Insert, None, plain),
            Some(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Delete, None, plain),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Page_Up, None, plain),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Page_Down, None, plain),
            Some(b"\x1b[6~".to_vec())
        );
        assert_eq!(encoded(Keysym::F1, None, plain), Some(b"\x1bOP".to_vec()));
        assert_eq!(
            encoded(Keysym::F12, None, plain),
            Some(b"\x1b[24~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::ISO_Left_Tab, None, plain),
            Some(b"\x1b[Z".to_vec())
        );

        let alt = Modifiers { alt: true, ..plain };
        assert_eq!(encoded(Keysym::x, Some("x"), alt), Some(b"\x1bx".to_vec()));
        let control = Modifiers {
            ctrl: true,
            ..plain
        };
        assert_eq!(encoded(Keysym::c, Some("c"), control), Some(vec![3]));
        assert_eq!(encoded(Keysym::c, Some("\u{3}"), control), Some(vec![3]));
        assert_eq!(encoded(Keysym::Shift_L, None, plain), None);
        assert_eq!(
            encoded(Keysym::eacute, Some("é"), plain),
            Some("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn mode_and_modifier_key_sequences_match_xterm_conventions() {
        let shift_ctrl = Modifiers {
            shift: true,
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            encoded(Keysym::Up, None, shift_ctrl),
            Some(b"\x1b[1;6A".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Delete, None, shift_ctrl),
            Some(b"\x1b[3;6~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::F2, None, shift_ctrl),
            Some(b"\x1b[1;6Q".to_vec())
        );

        let mut modes = normal_modes();
        modes.application_cursor = true;
        assert_eq!(
            key_input(Keysym::Left, None, Modifiers::default(), modes),
            Some(b"\x1bOD".to_vec())
        );
        modes.application_keypad = true;
        assert_eq!(
            key_input(Keysym::KP_7, Some("7"), Modifiers::default(), modes),
            Some(b"\x1bOw".to_vec())
        );
        assert_eq!(
            key_input(Keysym::colon, Some(":"), Modifiers::default(), modes),
            Some(b":".to_vec()),
            "application keypad mode must not consume Neovim commands"
        );
        assert_eq!(
            key_input(Keysym::space, Some(" "), Modifiers::default(), modes),
            Some(b" ".to_vec()),
            "application keypad mode must not consume Space leader"
        );
        assert_eq!(
            key_input(Keysym::w, Some("w"), Modifiers::default(), modes),
            Some(b"w".to_vec())
        );
    }

    #[test]
    fn clipboard_policy_filters_mime_size_utf8_and_controls() {
        assert_eq!(
            accepted_text_mime(&[
                "image/png".to_owned(),
                "text/plain".to_owned(),
                "text/plain;charset=utf-8".to_owned(),
            ]),
            Some("text/plain;charset=utf-8".to_owned())
        );
        assert_eq!(accepted_text_mime(&["image/png".to_owned()]), None);
        assert_eq!(
            safe_paste(b"line one\nline two\t").expect("safe text"),
            b"line one\nline two\t"
        );
        assert!(safe_paste(b"unsafe\x1bsequence").is_err());
        assert!(safe_paste(&[0xff]).is_err());
        assert!(safe_paste(&vec![b'x'; MAX_CLIPBOARD_BYTES + 1]).is_err());
    }

    #[test]
    fn trusted_history_return_target_is_half_open_and_detached_only() {
        let layout = history_overlay_layout(960, 600, 120).expect("overlay layout");
        let (x, y, width, height) = layout.return_to_live;
        let inside = (f64::from(x) + 1.0, f64::from(y) + 1.0);
        assert!(history_return_to_live_hit(inside, 960, 600, true));
        assert!(!history_return_to_live_hit(inside, 960, 600, false));
        assert!(!history_return_to_live_hit(
            (f64::from(x) + f64::from(width), f64::from(y) + 1.0),
            960,
            600,
            true,
        ));
        assert!(!history_return_to_live_hit(
            (f64::from(x) + 1.0, f64::from(y) + f64::from(height)),
            960,
            600,
            true,
        ));
    }

    #[test]
    fn trusted_title_surfaces_control_decision_and_bounded_search_state() {
        let authority = AuthorityStatus::default();
        let mut search = SearchUiState {
            input: Some("needle\nspoof".into()),
            ..SearchUiState::default()
        };
        search.matches.push(SearchMatch {
            row_id: 1,
            start_column: 0,
            end_column: 2,
            preview: "needle".into(),
        });
        let title = window_title(Some("shell"), true, &authority, true, Some(&search));
        assert!(title.contains("local controller"));
        assert!(title.contains("CONTROL REQUEST"));
        assert!(title.contains("SEARCH: needlespoof [1 match(es)"));
        assert!(!title.contains('\n'));
    }

    #[test]
    fn history_overlay_status_is_detached_and_display_bounded() {
        let mut state = snapshot(SplintId::new(), 1, 1);
        state.rows = 1;
        state.scrollback_rows.push(TerminalRow {
            row_id: Some(1),
            linebreak: false,
            cells: Vec::new(),
        });
        state.available_scrollback_rows = 5_000;
        let mut viewport = ScrollbackViewport::default();
        assert_eq!(history_overlay_status(&viewport, Some(&state)), None);
        viewport.scroll_up(1, &state);
        assert_eq!(
            history_overlay_status(&viewport, Some(&state)),
            Some(HistoryOverlayStatus {
                offset_from_bottom: 1,
                available_rows: 999,
                unseen_rows: 0,
            })
        );
    }

    #[test]
    fn selection_text_orders_endpoints_and_skips_wide_spacers() {
        let mut view = snapshot(SplintId::new(), 1, 1);
        view.columns = 4;
        view.rows = 2;
        view.visible_rows = vec![blank_row(4), blank_row(4)];
        view.visible_rows[0].row_id = Some(1);
        view.visible_rows[1].row_id = Some(2);
        view.visible_rows[0].cells[0].content = "A".to_owned();
        view.visible_rows[0].cells[1].content = "界".to_owned();
        view.visible_rows[0].cells[2].spacer_remaining = Some(0);
        view.visible_rows[0].cells[3].content = " ".to_owned();
        view.visible_rows[1].cells[0].content = "B".to_owned();
        view.visible_rows[1].cells[1].content = "C".to_owned();
        let selection = Selection {
            anchor: SelectionEndpoint {
                active_screen: ActiveScreen::Normal,
                history_generation: 1,
                row_id: 2,
                column: 1,
            },
            end: SelectionEndpoint {
                active_screen: ActiveScreen::Normal,
                history_generation: 1,
                row_id: 1,
                column: 0,
            },
        };
        assert_eq!(selection_text(&view, selection).as_deref(), Some("A界\nBC"));
        let stale = Selection {
            anchor: SelectionEndpoint {
                history_generation: 2,
                ..selection.anchor
            },
            ..selection
        };
        assert_eq!(selection_text(&view, stale), None);
    }

    #[test]
    fn selection_identity_survives_live_to_history_and_rejects_reset_or_trim() {
        let mut state = snapshot(SplintId::new(), 1, 1);
        state.columns = 1;
        state.rows = 2;
        state.visible_rows = vec![blank_row(1), blank_row(1)];
        state.visible_rows[0].row_id = Some(10);
        state.visible_rows[1].row_id = Some(11);
        let selection = Selection {
            anchor: SelectionEndpoint {
                active_screen: ActiveScreen::Normal,
                history_generation: 1,
                row_id: 10,
                column: 0,
            },
            end: SelectionEndpoint {
                active_screen: ActiveScreen::Normal,
                history_generation: 1,
                row_id: 11,
                column: 0,
            },
        };
        assert!(selection_is_retained(&state, selection));

        let moved = state.visible_rows.remove(0);
        state.scrollback_rows.push(moved);
        state.rows = 1;
        assert!(selection_is_retained(&state, selection));
        state.history_generation = 2;
        assert!(!selection_is_retained(&state, selection));
        state.history_generation = 1;
        state.scrollback_rows.clear();
        assert!(!selection_is_retained(&state, selection));
        state.active_screen = ActiveScreen::Alternate;
        assert!(!selection_is_retained(&state, selection));
    }

    #[test]
    fn selection_copy_spans_three_loaded_pages_by_row_identity() {
        let mut state = snapshot(SplintId::new(), 1, 1);
        state.columns = 1;
        state.rows = 8;
        let rows = (1..=48)
            .map(|row_id| {
                let mut row = blank_row(1);
                row.row_id = Some(row_id);
                row.cells[0].content = "x".to_owned();
                row
            })
            .collect::<Vec<_>>();
        state.scrollback_rows = rows[..40].to_vec();
        state.visible_rows = rows[40..].to_vec();
        let selection = Selection {
            anchor: SelectionEndpoint {
                active_screen: ActiveScreen::Normal,
                history_generation: 1,
                row_id: 1,
                column: 0,
            },
            end: SelectionEndpoint {
                active_screen: ActiveScreen::Normal,
                history_generation: 1,
                row_id: 48,
                column: 0,
            },
        };
        let copied = selection_text(&state, selection).expect("loaded endpoints resolve");
        assert_eq!(copied.lines().count(), 48);

        let mut display = state.clone();
        display.scrollback_rows.clear();
        display.visible_rows = rows[19..27].to_vec();
        assert_eq!(
            selection_display_bounds(&state, &display, selection),
            Some((
                CellPosition { row: 0, column: 0 },
                CellPosition { row: 7, column: 0 },
            ))
        );
    }

    #[test]
    fn page_bounding_rejects_eviction_of_selected_endpoint() {
        let rows = (1..=u64::try_from(MAX_CACHED_HISTORY_ROWS + 1).unwrap())
            .map(|row_id| {
                let mut row = blank_row(1);
                row.row_id = Some(row_id);
                row
            })
            .collect::<Vec<_>>();
        let oldest = bound_history_page_with_pins(rows.clone(), Some([1, 2]), &[])
            .expect("older paging retains selected oldest endpoints");
        assert_eq!(oldest.len(), MAX_CACHED_HISTORY_ROWS);
        assert_eq!(oldest.first().and_then(|row| row.row_id), Some(1));
        let newest = u64::try_from(MAX_CACHED_HISTORY_ROWS + 1).unwrap();
        assert!(
            bound_history_page_with_pins(rows, Some([newest - 1, newest]), &[]).is_none(),
            "older paging rejects a batch rather than evict selected newest endpoints"
        );
    }

    #[test]
    fn url_detection_is_visible_http_only_and_trims_punctuation() {
        let mut view = snapshot(SplintId::new(), 1, 1);
        let text = "see https://example.com/path). now";
        view.columns = text.chars().count();
        view.rows = 1;
        let mut row = blank_row(view.columns);
        for (cell, character) in row.cells.iter_mut().zip(text.chars()) {
            cell.content = character.to_string();
        }
        view.visible_rows = vec![row];
        let column = text.find("example").expect("URL column");
        let (_, _, url) = url_at(&view, CellPosition { row: 0, column }).expect("visible URL");
        assert_eq!(url, "https://example.com/path");
        assert!(url_at(&view, CellPosition { row: 0, column: 0 }).is_none());
    }

    #[test]
    fn visible_content_updates_invalidate_local_overlays_but_metadata_does_not() {
        let mut cursor_only = empty_update();
        cursor_only.cursor = Some(splinterm_protocol::TerminalCursor {
            column: 1,
            row: 1,
            deferred_wrap: false,
        });
        assert!(!terminal_update_changes_visible_content(&cursor_only));

        let mut row = empty_update();
        row.rows.push(splinterm_protocol::TerminalRowPatch {
            index: 0,
            row: blank_row(1),
        });
        assert!(terminal_update_changes_visible_content(&row));
        let mut scroll = empty_update();
        scroll.scrolls.push(splinterm_protocol::TerminalScroll {
            direction: splinterm_protocol::ScrollDirection::Forward,
            start_row: 0,
            end_row: 2,
            rows: 1,
        });
        assert!(terminal_update_changes_visible_content(&scroll));
        assert!(!terminal_update_requires_full_frame(
            &scroll,
            ActiveScreen::Normal,
            false
        ));
        assert!(terminal_update_requires_full_frame(
            &scroll,
            ActiveScreen::Normal,
            true
        ));

        let mut screen = empty_update();
        screen.active_screen = Some(ActiveScreen::Normal);
        assert!(!terminal_update_requires_full_frame(
            &screen,
            ActiveScreen::Normal,
            false
        ));
        screen.active_screen = Some(ActiveScreen::Alternate);
        assert!(terminal_update_requires_full_frame(
            &screen,
            ActiveScreen::Normal,
            false
        ));

        let mut image_update = empty_update();
        image_update.images = Some(Box::new(splinterm_protocol::TerminalImagePlane {
            screen: ActiveScreen::Normal,
            contents: Vec::new(),
            placements: Vec::new(),
        }));
        assert!(terminal_update_requires_full_frame(
            &image_update,
            ActiveScreen::Normal,
            false
        ));
        let mut colors = empty_update();
        colors.default_colors = Some([1, 2, 3]);
        assert!(terminal_update_changes_visible_content(&colors));
    }

    #[test]
    fn stale_url_disappears_when_visible_row_is_replaced() {
        let mut view = snapshot(SplintId::new(), 1, 1);
        let text = "https://example.com";
        view.columns = text.len();
        view.rows = 1;
        let mut row = blank_row(view.columns);
        for (cell, character) in row.cells.iter_mut().zip(text.chars()) {
            cell.content = character.to_string();
        }
        view.visible_rows = vec![row];
        let position = CellPosition { row: 0, column: 10 };
        assert!(url_at(&view, position).is_some());
        view.visible_rows[0] = blank_row(view.columns);
        assert!(url_at(&view, position).is_none());
    }

    #[test]
    fn press_ownership_pairs_application_and_local_releases() {
        let position_present = true;
        let mut modes = normal_modes();
        modes.mouse_tracking = MouseTracking::Normal;
        let app = classify_press(
            BTN_LEFT,
            position_present,
            Modifiers::default(),
            modes,
            false,
        );
        assert!(matches!(
            app,
            PressOwner::Application {
                code: 0,
                tracking: MouseTracking::Normal,
                ..
            }
        ));
        assert!(application_motion(&app).is_none());
        modes.mouse_tracking = MouseTracking::Button;
        let button_motion = classify_press(
            BTN_LEFT,
            position_present,
            Modifiers::default(),
            modes,
            false,
        );
        assert!(application_motion(&button_motion).is_some());
        let primary = classify_press(
            BTN_MIDDLE,
            position_present,
            Modifiers::default(),
            modes,
            false,
        );
        assert!(matches!(primary, PressOwner::PrimaryPaste));
        let url = classify_press(
            BTN_LEFT,
            position_present,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            modes,
            true,
        );
        assert!(matches!(url, PressOwner::Url));

        let mut pressed = HashMap::from([(BTN_MIDDLE, primary), (BTN_LEFT, url)]);
        assert!(matches!(
            take_press_owner(&mut pressed, BTN_MIDDLE),
            PressOwner::PrimaryPaste
        ));
        assert!(matches!(
            take_press_owner(&mut pressed, BTN_LEFT),
            PressOwner::Url
        ));
        assert!(matches!(
            take_press_owner(&mut pressed, BTN_RIGHT),
            PressOwner::Ignored
        ));
    }

    #[test]
    fn axis_accumulates_partial_steps_with_foot_thresholds() {
        let mut wheel = WheelAccumulator::default();
        assert_eq!(wheel.push(0.0, 0, -60, 10), None);
        assert_eq!(wheel.push(0.0, 0, -59, 10), None);
        assert_eq!(wheel.push(0.0, 0, -1, 10), Some((MouseAction::WheelUp, 1)));
        assert_eq!(wheel.push(0.0, 0, 119, 10), None);
        assert_eq!(wheel.push(0.0, 0, 1, 10), Some((MouseAction::WheelDown, 1)));

        assert_eq!(
            wheel.push(0.0, 20, 0, 10),
            Some((MouseAction::WheelDown, 20))
        );
        assert_eq!(
            wheel.push(0.0, 0, 0, 10),
            None,
            "zero frames do not flush a different source implicitly"
        );
        assert_eq!(wheel.push(0.0, 1, 0, 10), Some((MouseAction::WheelDown, 1)));

        assert_eq!(wheel.push(-4.0, 0, 0, 10), None);
        assert_eq!(wheel.push(-6.0, 0, 0, 10), Some((MouseAction::WheelUp, 1)));
    }

    #[test]
    fn mouse_reports_cover_sgr_legacy_modifiers_motion_and_wheel() {
        let position = CellPosition { row: 4, column: 9 };
        assert_eq!(
            mouse_report(MouseAction::Press(0), position, Modifiers::default(), true,),
            Some(b"\x1b[<0;10;5M".to_vec())
        );
        assert_eq!(
            mouse_report(
                MouseAction::Release(0),
                position,
                Modifiers::default(),
                true,
            ),
            Some(b"\x1b[<0;10;5m".to_vec())
        );
        let modifiers = Modifiers {
            shift: true,
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            mouse_report(MouseAction::Motion(0), position, modifiers, true),
            Some(b"\x1b[<52;10;5M".to_vec())
        );
        assert_eq!(
            mouse_report(
                MouseAction::WheelUp,
                CellPosition { row: 0, column: 0 },
                Modifiers::default(),
                false,
            ),
            Some(vec![0x1b, b'[', b'M', 96, 33, 33])
        );
        assert!(
            mouse_report(
                MouseAction::WheelDown,
                CellPosition {
                    row: 500,
                    column: 500
                },
                Modifiers::default(),
                false,
            )
            .is_none()
        );
    }

    #[test]
    fn bracketed_paste_wraps_only_when_mode_is_enabled() {
        assert_eq!(encode_bracketed_paste(b"hello", false), b"hello");
        assert_eq!(
            encode_bracketed_paste(b"hello", true),
            b"\x1b[200~hello\x1b[201~"
        );
    }

    #[test]
    fn repeat_mapping_is_identical_to_press_mapping() {
        let modifiers = Modifiers::default();
        let press = encoded(Keysym::Left, None, modifiers);
        let repeat = encoded(Keysym::Left, None, modifiers);
        assert_eq!(press, repeat);
    }

    #[test]
    fn bounded_command_queue_reports_overflow_and_disconnect() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        try_window_command(&sender, WindowCommand::Input(vec![1])).expect("first command");
        let error = try_window_command(&sender, WindowCommand::Input(vec![2]))
            .expect_err("bounded overflow");
        assert!(error.to_string().contains("overflow"));
        assert!(receiver.try_recv().is_ok());
        drop(receiver);
        let error = try_window_command(&sender, WindowCommand::Input(vec![3]))
            .expect_err("disconnected receiver");
        assert!(error.to_string().contains("disconnected"));
    }

    #[test]
    fn clipboard_worker_budget_is_strict_and_released() {
        let active = AtomicUsize::new(0);
        let permits: Vec<_> = (0..MAX_CLIPBOARD_WORKERS)
            .map(|_| try_clipboard_worker(&active).expect("worker slot"))
            .collect();
        assert!(try_clipboard_worker(&active).is_none());
        drop(permits);
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert!(try_clipboard_worker(&active).is_some());
    }

    #[test]
    fn clipboard_read_deadline_expires_without_a_writer() {
        use std::os::unix::net::UnixStream;

        let (reader, _writer) = UnixStream::pair().expect("socket pair");
        let fd = OwnedFd::from(reader);
        let error = read_clipboard_with_deadline(&fd, Duration::from_millis(5))
            .expect_err("idle peer times out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn duplicate_resize_is_suppressed() {
        let size = (80, 24, 1_120, 720);
        assert!(resize_changed(None, size));
        assert!(!resize_changed(Some(size), size));
        assert!(resize_changed(Some(size), (81, 24, 1_134, 720)));
    }

    #[test]
    fn output_lifecycle_preserves_grid_until_surface_configure() {
        for cause in [
            TerminalResizeCause::OutputDpiChanged,
            TerminalResizeCause::CompositorScaleChanged,
        ] {
            assert!(!terminal_resize_allowed(cause, false));
            assert!(!terminal_resize_allowed(cause, true));
        }
        assert!(terminal_resize_allowed(
            TerminalResizeCause::SurfaceConfigure,
            true
        ));
        assert!(terminal_resize_allowed(
            TerminalResizeCause::SurfaceConfigure,
            false
        ));
    }

    #[test]
    fn startup_snapshot_emits_only_the_first_grid_resize() {
        assert!(terminal_resize_allowed(
            TerminalResizeCause::SnapshotAvailable,
            false
        ));
        assert!(!terminal_resize_allowed(
            TerminalResizeCause::SnapshotAvailable,
            true
        ));
    }

    #[test]
    fn output_entry_order_selects_most_recent_and_retains_when_unmapped() {
        let mut entered = Vec::new();
        note_output_enter(&mut entered, &1);
        note_output_enter(&mut entered, &2);
        note_output_enter(&mut entered, &1);
        assert_eq!(entered, vec![2, 1]);
        assert!(!note_output_leave(&mut entered, &2));
        assert_eq!(entered.last(), Some(&1));
        assert!(note_output_leave(&mut entered, &1));
        assert!(entered.is_empty());
        // App deliberately leaves renderer's last DPI observation unchanged here.
    }

    #[test]
    fn buffer_dimensions_scale_logical_size_and_stride() {
        assert_eq!(
            buffer_dimensions(960, 600, 120).expect("1x"),
            (960, 600, 3_840)
        );
        assert_eq!(
            buffer_dimensions(960, 600, 240).expect("2x"),
            (1_920, 1_200, 7_680)
        );
    }

    #[test]
    fn fractional_buffer_dimensions_cover_phase_six_scale_matrix() {
        assert_eq!(buffer_dimensions(801, 601, 120).unwrap(), (801, 601, 3_204));
        assert_eq!(
            buffer_dimensions(801, 601, 150).unwrap(),
            (1_002, 752, 4_008)
        );
        assert_eq!(
            buffer_dimensions(801, 601, 180).unwrap(),
            (1_202, 902, 4_808)
        );
        assert_eq!(
            buffer_dimensions(801, 601, 240).unwrap(),
            (1_602, 1_202, 6_408)
        );
    }

    #[test]
    fn ime_batches_are_bounded_replace_commits_and_clear_committed_preedit() {
        let mut ime = ImeState {
            entered: true,
            focused: true,
            ..ImeState::default()
        };
        assert!(!ime.composing());
        let decomposed = format!("e{}", '\u{301}');
        ime.set_preedit(Some(decomposed.clone()));
        assert!(ime.composing());
        ime.note_client_commit();
        let (_, visible, commit) = ime.finish(1);
        assert_eq!(visible.as_deref(), Some(decomposed.as_str()));
        assert!(commit.is_none());

        ime.set_commit(Some("first".into()));
        ime.set_commit(Some("final".into()));
        let (serial_matches, visible, commit) = ime.finish(1);
        assert!(serial_matches);
        assert_eq!(visible, None);
        assert_eq!(commit.as_deref(), Some("final"));

        ime.set_preedit(Some("x".repeat(MAX_PREEDIT_BYTES + 1)));
        assert!(!ime.composing());
    }

    #[test]
    fn detached_ime_repaint_targets_the_display_cursor_row_only() {
        let mut display = snapshot(SplintId::new(), 1, 1);
        display.columns = 2;
        display.rows = 3;
        display.cursor_column = 0;
        display.cursor_row = 1;
        display.visible_rows = ["a0", "b1", "c2"]
            .map(|text| {
                let mut row = blank_row(2);
                for (cell, character) in row.cells.iter_mut().zip(text.chars()) {
                    cell.content = character.to_string();
                }
                row
            })
            .into();

        assert_eq!(apply_ime_preedit(&mut display, Some("Z")), Some(1));
        let contents = display
            .visible_rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| &cell.content)
                    .cloned()
                    .collect()
            })
            .collect::<Vec<String>>();
        assert_eq!(contents, ["a0", "Z1", "c2"]);

        display.cursor_row = -1;
        assert_eq!(apply_ime_preedit(&mut display, None), None);
        assert_eq!(
            display.visible_rows[0].cells[0].content, "a",
            "a hidden live cursor must not replace a visible history row"
        );
    }

    #[test]
    fn reduced_motion_and_focus_suppress_cursor_blink() {
        let modes = normal_modes();
        assert!(cursor_blink_enabled(false, true, modes));
        assert!(!cursor_blink_enabled(true, true, modes));
        assert!(!cursor_blink_enabled(false, false, modes));
    }

    #[test]
    fn ime_done_applies_events_even_when_client_state_serial_is_stale() {
        let mut ime = ImeState::default();
        ime.set_commit(Some("界".into()));
        let (serial_matches, _, commit) = ime.finish(9);
        assert!(!serial_matches);
        assert_eq!(commit.as_deref(), Some("界"));
    }

    #[test]
    fn buffer_dimensions_reject_zero_scale_and_overflow() {
        assert!(buffer_dimensions(960, 600, 0).is_err());
        assert!(buffer_dimensions(u32::MAX, 1, 240).is_err());
        assert!(buffer_dimensions(1, u32::MAX, 240).is_err());
        assert!(buffer_dimensions(i32::MAX as u32 / 4 + 1, 1, 120).is_err());
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_data_device!(App);
delegate_primary_selection!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}
