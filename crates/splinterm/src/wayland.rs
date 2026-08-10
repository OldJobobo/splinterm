//! Native Wayland xdg-shell and shared-memory lifecycle for the graphical client.
//!
//! Foot 1.27.0 `wayland.c`, `shm.c`, and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` are the behavioral reference.
//! The client owns these objects; the daemon remains headless.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    pin::Pin,
    process::Command,
    sync::{
        Arc,
        mpsc::{self as std_mpsc, Receiver as StdReceiver, Sender as StdSender},
    },
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use tokio::sync::mpsc::{Receiver, Sender, error::TrySendError};
use unicode_width::UnicodeWidthChar;

use anyhow::{Context, Result};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
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
        pointer::{
            BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, PointerEvent, PointerEventKind, PointerHandler,
        },
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
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::registry_queue_init,
    protocol::{
        wl_data_device, wl_data_device_manager::DndAction, wl_data_source, wl_keyboard, wl_output,
        wl_pointer, wl_seat, wl_shm, wl_surface,
    },
};

use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{
        Capability as BackgroundCapability, Event as BackgroundManagerEvent,
        ExtBackgroundEffectManagerV1,
    },
    ext_background_effect_surface_v1::{self, ExtBackgroundEffectSurfaceV1},
};

use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::{
    Axis, DojoId, LairId, LayoutNode, Splint, SplintId, SplitRatio, TopologyRevision,
};
use splinterm_protocol::{
    ActiveScreen, ControlTransferDecision, MouseTracking, SearchMatch, TerminalCell,
    TerminalInputModes, TerminalRow, TerminalSnapshot,
    perf_trace::{
        PerfTraceEvent, emit_perf_trace, emit_perf_trace_at, monotonic_raw_ns, perf_trace_enabled,
    },
};
#[cfg(test)]
use splinterm_protocol::{HistoryTransition, TerminalUpdate};

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

use crate::background_effect::{
    BackgroundEffectState, CommitReason as BackgroundCommitReason, EffectAction, EffectDiagnostic,
};
use crate::config::{APP_ID, CursorStyle, FrameTitleMode, PaneDividerStyle, ResolvedTheme};
use crate::diagnostics::{
    DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, ExitClass, global as diagnostics,
};
use crate::frontend::{
    AuthorityStatus, BuiltInCommandDispatch, BuiltInCommandId, CommandControlAction,
    CommandHistoryAction, CommandPaletteContext, CommandPaletteUi, CommandZoomAction, DojoPromptUi,
    LairDirection, LairPromptKind, PerfTraceCorrelation, SelectorKind, SessionPickerDecision,
    SessionPickerItem, SessionPickerUi, TabContextMenuUi, TabMenuActionId, TabMenuContext,
    TabMenuDispatch, TabMenuRightPress, TerminationDecision, ThemeUpdate, TrustedConsentUi,
    WindowCommand, WindowDojoIdentity, WindowOptions, WindowPaneOptions, WindowTopologyCommand,
    WindowTopologyUpdate, WindowUpdate, command_dispatch, tab_menu_dispatch, tab_menu_right_press,
};
use crate::geometry::{
    OutputDpiObservation, Rect, SurfaceGeometry, WindowGeometry, buffer_to_logical_ceil,
    logical_extent_to_buffer,
};
#[cfg(test)]
use crate::pane::PaneDivider;
use crate::pane::{
    FocusDirection, PaneChrome, PaneLayout, PaneResizeMetrics, PaneSplit, apply_preview_ratio,
    directional_resize_ratio, split_ratio_at,
};
#[cfg(test)]
use crate::renderer::paint_box_drawing_cell;
use crate::renderer::{
    ChromeText, ChromeTextStyle, CommandPaletteLayout, CommandPaletteTextCache, CursorPresentation,
    DojoPromptLayout, HistoryOverlayStatus, PickerHitTarget, RenderContext,
    SessionPickerOverlayLayout, SessionPickerTextCache, SessionPickerTextItem, SnapshotFrame,
    SnapshotOverlays, TabContextMenuLayout, TextRow, background_bgra, command_palette_hit_test,
    command_palette_layout, dojo_prompt_hit_test, dojo_prompt_layout, fill_rect,
    history_overlay_layout, paint, paint_command_palette, paint_dojo_prompt, paint_history_overlay,
    paint_session_picker_overlay, paint_snapshot_overlays, paint_snapshot_presented,
    paint_snapshot_region_presented, paint_snapshot_rows_presented, paint_tab_context_menu,
    scroll_snapshot_pixels, session_picker_hit_test, session_picker_overlay_layout,
    session_picker_palette, snapshot_row_rect, tab_context_menu_hit_test, tab_context_menu_layout,
    write_ppm,
};
use crate::{
    keymap::{ActionId, KeymapPress, PrefixState, ResolvedKeymap},
    tab::{DojoTab, WindowTabSet, sanitized_tab_label},
    viewport::ScrollbackViewport,
};

mod chrome;
mod clipboard;
mod damage;
mod dispatch;
mod input;
mod selection;
mod tabs;
mod terminal_state;
#[cfg(test)]
use chrome::divider_junction;
use chrome::{paint_pane_chrome, paint_trusted_consent_chrome, sanitize_frame_title};
pub use clipboard::encode_bracketed_paste;
use clipboard::{
    ACTIVE_CLIPBOARD_WORKERS, CLIPBOARD_IO_TIMEOUT, ClipboardRead, PasteTarget, TEXT_MIMES,
    accepted_text_mime, safe_paste, spawn_clipboard_read, try_clipboard_worker,
    write_clipboard_with_deadline,
};
use damage::{
    BackingDamage, pending_draw_waits_for_frame, sync_backing_damage, take_full_surface_damage,
    terminal_draw_waits_for_frame,
};
use input::{
    CommandPaletteShortcutAction, FontZoomAction, HistoryNavigation, ModalPointerFrame,
    MouseAction, PaneFocusAction, PaneTopologyAction, PickerImeReconcile, PressOwner,
    SessionPickerShortcutAction, TabShortcutAction, WheelAccumulator, WheelOutcome,
    application_motion, classify_press, clipboard_read_is_current, command_palette_shortcut_action,
    font_zoom_action, history_overlay_status, history_return_to_live_hit, key_input,
    keymap_press_for, local_selection_owner, mouse_report, pane_focus_action, pane_topology_action,
    pending_selection_drag_anchor, picker_ime_reconcile, picker_release_activation,
    pointer_axis_focus_target, reconciled_focus_report, session_picker_shortcut_action,
    shortcut_action_for, tab_action_dispatch_allowed, tab_shortcut_action, take_press_owner,
};
use selection::{
    CellPosition, Selection, SelectionEndpoint, selection_display_bounds, selection_endpoint,
    selection_is_retained, selection_text, transient_overlay_rows, url_at,
};
use tabs::{
    DojoTabView, TAB_STRIP_LOGICAL_HEIGHT, TabHitTarget, TabsState, tab_context_target,
    tab_strip_hit_test,
};
#[cfg(test)]
use terminal_state::snapshot_is_newer;
use terminal_state::{
    MAX_CACHED_HISTORY_BYTES, MAX_CACHED_HISTORY_ROWS, apply_terminal_update,
    bound_history_page_with_pins, changed_terminal_patch_rows, history_cache_bytes,
    omitted_rows_before_cache, snapshot_replaces, terminal_update_changes_visible_content,
    terminal_update_full_frame_reasons,
};
#[cfg(test)]
use terminal_state::{
    apply_scrollback_update, blank_row, coalesce_snapshots, terminal_update_requires_full_frame,
};

const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 600;
const MAX_DEFERRED_TOPOLOGY_UPDATES: usize = 16;
// Keep application mouse reports at one report per wheel step. Local history
// follows Foot's default three-lines-per-step semantic distance; visual motion
// must be smoothed in pixels rather than by increasing this row multiplier.
const SCROLLBACK_WHEEL_MULTIPLIER: f64 = 3.0;
const SCALE_DENOMINATOR: u32 = 120;
const MIN_SCALE_120: u32 = 120;
const MAX_SCALE_120: u32 = 960;
const MAX_PREEDIT_BYTES: usize = 4 * 1024;
const SIGNOFF_TICK_INTERVAL: Duration = Duration::from_millis(50);
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);
const IDLE_EVENT_LOOP_TIMEOUT: Duration = Duration::from_secs(60);
const RECEIVER_DRAIN_BUDGET: usize = 8;
const MAX_SHM_BUFFERS: usize = 2;

fn translated_rect(mut rect: Rect, origin: Rect) -> Rect {
    rect.x = rect.x.saturating_add(origin.x);
    rect.y = rect.y.saturating_add(origin.y);
    rect
}

fn translate_picker_layout(
    mut layout: SessionPickerOverlayLayout,
    origin: Rect,
) -> SessionPickerOverlayLayout {
    layout.panel = translated_rect(layout.panel, origin);
    layout.header = translated_rect(layout.header, origin);
    layout.action = translated_rect(layout.action, origin);
    layout.list = translated_rect(layout.list, origin);
    layout.footer = translated_rect(layout.footer, origin);
    for row in &mut layout.rows {
        row.rect = translated_rect(row.rect, origin);
        row.surface = translated_rect(row.surface, origin);
        row.title_clip = translated_rect(row.title_clip, origin);
        row.metadata_clip = translated_rect(row.metadata_clip, origin);
    }
    layout
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinalTabRemovalAction {
    Continue,
    Exit,
    ExitAndHandoffPicker,
}

const fn final_tab_removal_action(
    tab_removed: bool,
    remaining_tabs: usize,
    picker_requested: bool,
) -> FinalTabRemovalAction {
    if !tab_removed || remaining_tabs != 0 {
        FinalTabRemovalAction::Continue
    } else if picker_requested {
        FinalTabRemovalAction::ExitAndHandoffPicker
    } else {
        FinalTabRemovalAction::Exit
    }
}

fn session_picker_handoff_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command.arg("sessions");
    command
}

fn spawn_session_picker_handoff() -> Result<()> {
    let executable = std::env::current_exe().context("locate the running Splinterm client")?;
    session_picker_handoff_command(&executable)
        .spawn()
        .context("launch the Recent Sessions picker after final-tab removal")?;
    Ok(())
}

fn rect_contains(rect: Rect, position: (f64, f64)) -> bool {
    position.0 >= f64::from(rect.x)
        && position.1 >= f64::from(rect.y)
        && position.0 < f64::from(rect.x.saturating_add(rect.width))
        && position.1 < f64::from(rect.y.saturating_add(rect.height))
}

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

    fn clear_composition(&mut self) {
        self.visible_preedit = None;
        self.pending = ImeBatch::default();
    }
}

fn pane_stream_has_terminal_notice(updates: &[WindowUpdate]) -> bool {
    updates
        .iter()
        .any(|update| matches!(update, WindowUpdate::Exited { .. } | WindowUpdate::Shutdown))
}

fn enqueue_pending_exited_splints(
    dojo_id: DojoId,
    pending: &mut HashSet<SplintId>,
    commands: &Sender<WindowTopologyCommand>,
) -> bool {
    let targets = pending.iter().copied().collect::<Vec<_>>();
    for target in targets {
        match commands.try_send(WindowTopologyCommand::Close { dojo_id, target }) {
            Ok(()) => {
                pending.remove(&target);
            }
            Err(TrySendError::Full(_)) => return true,
            Err(TrySendError::Closed(_)) => return false,
        }
    }
    true
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
    if let Some(diagnostics) = diagnostics() {
        let dojo_id = options
            .initial_dojo
            .as_ref()
            .map(|identity| identity.dojo_id);
        let splint_id = options
            .active_splint
            .or_else(|| options.snapshot.as_ref().map(|snapshot| snapshot.splint_id));
        diagnostics.ensure_window(dojo_id, splint_id);
    }
    let managed_tabs = options.initial_dojo.is_some();
    let render_context = RenderContext::new(options.theme.background_alpha);
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
            SnapshotFrame::load_scaled_with_sources_and_context(
                snapshot,
                120,
                Some(&options.image_sources),
                &render_context,
            )
        })
        .transpose()?;
    let (initial_width, mut initial_height) = snapshot_frame
        .as_ref()
        .map_or(Ok((INITIAL_WIDTH, INITIAL_HEIGHT)), |frame| {
            frame.initial_logical_size(options.initial_columns, options.initial_rows, 120)
        })?;
    if managed_tabs {
        initial_height = initial_height
            .checked_add(TAB_STRIP_LOGICAL_HEIGHT)
            .context("initial tab strip height overflow")?;
    }
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
    let session_picker = options.session_picker;
    let title = if trusted_consent.is_some() {
        "Splinterm — Trusted Access Request".to_owned()
    } else if session_picker.is_some() {
        "Splinterm — Recent Sessions".to_owned()
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
    let inactive_panes = inactive_options
        .into_iter()
        .map(|pane| {
            PaneView::from_inactive_options_with_context(pane, SCALE_DENOMINATOR, &render_context)
        })
        .collect::<Result<Vec<_>>>()?;
    let initial_identity = options
        .initial_dojo
        .take()
        .unwrap_or_else(|| WindowDojoIdentity {
            topology_revision: TopologyRevision::new(0),
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            lair_name: String::new(),
            dojo_name: String::new(),
        });
    if let Some(diagnostics) = diagnostics() {
        diagnostics.update_topology(
            initial_identity.topology_revision,
            usize::from(managed_tabs),
        );
    }
    let initial_lair_id = initial_identity.lair_id;
    let initial_dojo_id = initial_identity.dojo_id;
    let background_effect_manager = globals
        .bind::<ExtBackgroundEffectManagerV1, _, _>(&queue_handle, 1..=1, ())
        .ok();
    let mut background_effect_state = BackgroundEffectState::default();
    background_effect_state.set_manager_available(background_effect_manager.is_some());
    background_effect_state.set_requested_blur(options.theme.background_blur);
    background_effect_state.set_background_alpha(options.theme.background_alpha);
    let background_effect_trace = std::env::var_os("SPLINTERM_BACKGROUND_EFFECT_TRACE").is_some();
    if background_effect_trace {
        if let Some(manager) = &background_effect_manager {
            eprintln!(
                "splinterm background-effect manager version={} bound",
                manager.version()
            );
        } else {
            eprintln!("splinterm background-effect manager unavailable");
        }
    }
    let mut app = App {
        platform: PlatformState {
            registry_state: RegistryState::new(&globals),
            compositor,
            seat_state: SeatState::new(&globals, &queue_handle),
            output_state: OutputState::new(&globals, &queue_handle),
            data_device_manager,
            primary_selection_manager,
            text_input_manager,
            shm,
            loop_handle: event_loop.handle(),
            update_waker,
            output_count: 0,
            entered_outputs: Vec::new(),
            seat_count: 0,
        },
        surface: SurfaceState {
            fractional_scale,
            viewport,
            background_effect_manager,
            background_effect: None,
            background_effect_state,
            background_effect_deferred_commit: None,
            background_effect_reconcile_schedule: BackgroundEffectReconcileSchedule::default(),
            background_effect_capabilities_received: false,
            background_effect_trace,
            window,
            pool,
            buffers: Vec::new(),
            backing: Vec::new(),
            logical_width: initial_width,
            logical_height: initial_height,
            configured: false,
            scale_120: SCALE_DENOMINATOR,
            integer_fallback_scale: 1,
        },
        presentation: PresentationState {
            render_context,
            text_row,
            renderer_generation: 0,
            theme_generation: 0,
            cursor_style: options.cursor_style,
            cursor_blink: options.cursor_blink,
            title_override: options.title,
            theme: options.theme,
            pane_divider_style: options.pane_divider_style,
            frame_title_mode: options.frame_title_mode,
            frame_titles: HashMap::new(),
            evidence_close_shortcuts: options.evidence_close_shortcuts,
            font_zoom_steps: 0,
            zoomed_splint: None,
            capture: options.capture,
            capture_scale: options.capture_scale,
            full_redraw: true,
        },
        input: InputState {
            keymap: options.keymap,
            prefix_state: PrefixState::Idle,
            prefix_timeout: Duration::from_millis(options.prefix_timeout_ms),
            text_input: None,
            text_input_seat: None,
            ime: ImeState::default(),
            reduced_motion: reduced_motion_requested(),
            keyboard_focused: false,
            graphical_focus: options.graphical_focus,
            forced_control_transfer: options.forced_control_transfer,
            optimistic_remote_splits: options.optimistic_remote_splits,
            input_generation: 0,
            terminal_focus_reported: false,
            ime_generation: 0,
            ime_modal_barrier: false,
            modifiers: Modifiers::default(),
            keyboard: None,
            keyboard_seat: None,
            pointer: None,
            pointer_seat: None,
            last_pointer_serial: None,
            pressed_buttons: HashMap::new(),
            divider_drag: None,
            vertical_wheel: WheelAccumulator::default(),
            scrollback_wheel: WheelAccumulator::default(),
            cursor_blink_visible: true,
            last_cursor_blink: Instant::now(),
        },
        clipboard: ClipboardState {
            data_device: None,
            primary_device: None,
            clipboard_offer: None,
            primary_offer: None,
            clipboard_sources: Vec::new(),
            primary_sources: Vec::new(),
            clipboard_tx,
            clipboard_rx,
        },
        panes: PanesState {
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
                control_release_pending: false,
                pending_focus_report: None,
                pending_control_transfer: None,
                search: SearchUiState::default(),
                authority: options.authority,
                last_resize: None,
                prepare_dirty_rows: Vec::new(),
                raster_dirty_rows: Vec::new(),
                surface_dirty_rows: Vec::new(),
                pending_scrolls: Vec::new(),
                trace_correlation: None,
                trace_pane_role: "focused",
                trace_superseded_revisions: 0,
                selected_text: None,
                selection: None,
                selecting: false,
                pointer_cell: None,
                hovered_url: None,
            },
            inactive_panes,
            layout: options.layout,
            restored_frontend_needs_resize: false,
            pending_exited_splints: HashSet::new(),
            pending_remote_splits: HashMap::new(),
            dirty_inactive_panes: HashSet::new(),
        },
        tab_state: TabsState {
            tabs: WindowTabSet::new(DojoTab::new(initial_lair_id, initial_dojo_id, None)),
            active_identity: initial_identity,
            managed_tabs,
            tab_strip_layout: None,
            tab_strip_pressed: None,
            tab_label_cache: HashMap::new(),
            tab_close_text: None,
            tab_new_text: None,
            topology_updates: options.topology_updates,
            topology_commands: options.topology_commands,
            session_switch_pending: false,
            deferred_topology_updates: Vec::new(),
        },
        modal: ModalState {
            trusted_consent,
            command_palette: None,
            command_palette_layout: None,
            command_palette_pressed: None,
            command_palette_text_cache: CommandPaletteTextCache::default(),
            command_palette_open_focus: None,
            command_palette_reconcile_pending: false,
            dojo_prompt: None,
            dojo_prompt_layout: None,
            dojo_prompt_pressed: None,
            dojo_prompt_text_cache: CommandPaletteTextCache::default(),
            tab_context_menu: None,
            tab_context_menu_anchor: (0, 0),
            tab_context_menu_layout: None,
            tab_context_menu_pressed: None,
            tab_context_menu_retarget: None,
            tab_context_menu_text_cache: CommandPaletteTextCache::default(),
            session_picker,
            selector_kind: None,
            session_picker_targets: Vec::new(),
            session_picker_layout: None,
            session_picker_pressed: None,
            session_picker_text_cache: SessionPickerTextCache::default(),
            session_picker_wheel: WheelAccumulator::default(),
            session_picker_consumed_keys: HashSet::new(),
            session_picker_redraw: false,
            session_picker_reconcile_pending: false,
            session_picker_open_focus: None,
            session_picker_requested: false,
            deferred_picker_theme: None,
        },
        scheduling: SchedulingState {
            signoff,
            graphical_input_probe,
            scroll_trace: std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some(),
            exit: false,
            failure: None,
            frame_pending: false,
            redraw_pending: false,
            terminal_redraw_pending: false,
            next_commit_sequence: 0,
            pending_frame_trace: None,
        },
    };

    let event_loop_result: Result<()> = (|| {
        while !app.scheduling.exit {
            app.apply_updates(&queue_handle)?;
            if app.scheduling.redraw_pending
                && !pending_draw_waits_for_frame(
                    app.scheduling.frame_pending,
                    app.scheduling.terminal_redraw_pending,
                )
            {
                if app.scheduling.terminal_redraw_pending {
                    app.schedule_terminal_draw(&queue_handle)?;
                } else {
                    app.schedule_draw(&queue_handle)?;
                }
            }
            app.tick_signoff(&queue_handle)?;
            app.apply_clipboard_reads()?;
            app.tick_cursor_blink(&queue_handle)?;
            let Some(dispatch_timeout) = app.event_loop_dispatch_timeout() else {
                break;
            };
            event_loop
                .dispatch(dispatch_timeout, &mut app)
                .context("dispatch Wayland events")?;
        }
        Ok(())
    })();
    app.release_background_effect();
    let failure = app.scheduling.failure.take();
    drop(app);
    drop(event_loop);
    let teardown_result = connection
        .roundtrip()
        .context("complete Wayland surface teardown")
        .map(|_| ());
    if (event_loop_result.is_err() || teardown_result.is_err())
        && let Some(diagnostics) = diagnostics()
    {
        diagnostics.request_exit(ExitClass::ErrorWaylandDispatch);
    }
    event_loop_result?;
    if let Some(error) = failure {
        return Err(error);
    }
    teardown_result
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
    control_release_pending: bool,
    pending_focus_report: Option<bool>,
    pending_control_transfer: Option<u64>,
    search: SearchUiState,
    authority: AuthorityStatus,
    last_resize: Option<(u16, u16, u16, u16)>,
    prepare_dirty_rows: Vec<bool>,
    raster_dirty_rows: Vec<bool>,
    surface_dirty_rows: Vec<bool>,
    pending_scrolls: Vec<splinterm_protocol::TerminalScroll>,
    trace_correlation: Option<PerfTraceCorrelation>,
    trace_pane_role: &'static str,
    trace_superseded_revisions: u64,
    selected_text: Option<Vec<u8>>,
    selection: Option<Selection>,
    selecting: bool,
    pointer_cell: Option<CellPosition>,
    hovered_url: Option<(CellPosition, CellPosition, String)>,
}

#[cfg(test)]
fn rebuild_pane_scaled_frame(pane: &mut PaneView, scale_120: u32) -> Result<bool> {
    let Some(display) = pane.display_snapshot() else {
        return Ok(false);
    };
    pane.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
        &display,
        scale_120,
        Some(&pane.image_sources),
    )?);
    finish_rebuilt_pane_frame(pane);
    Ok(true)
}

fn rebuild_pane_scaled_frame_with_context(
    pane: &mut PaneView,
    scale_120: u32,
    context: &RenderContext,
) -> Result<bool> {
    let Some(display) = pane.display_snapshot() else {
        return Ok(false);
    };
    pane.snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources_and_context(
        &display,
        scale_120,
        Some(&pane.image_sources),
        context,
    )?);
    finish_rebuilt_pane_frame(pane);
    Ok(true)
}

fn finish_rebuilt_pane_frame(pane: &mut PaneView) {
    pane.rendered_viewport_offset = pane.scrollback_viewport.offset_from_bottom();
    pane.viewport_dirty = false;
    pane.scroll_started_at = None;
    pane.prepare_dirty_rows.fill(false);
    pane.raster_dirty_rows.fill(false);
    pane.surface_dirty_rows.fill(false);
    pane.pending_scrolls.clear();
}

#[cfg(test)]
fn rebuild_dirty_pane_viewport_frame(pane: &mut PaneView, scale_120: u32) -> Result<bool> {
    if !pane.viewport_dirty {
        return Ok(false);
    }
    rebuild_pane_scaled_frame(pane, scale_120)
}

fn rebuild_dirty_pane_viewport_frame_with_context(
    pane: &mut PaneView,
    scale_120: u32,
    context: &RenderContext,
) -> Result<bool> {
    if !pane.viewport_dirty {
        return Ok(false);
    }
    rebuild_pane_scaled_frame_with_context(pane, scale_120, context)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BackgroundUpdateImpact {
    visual_changed: bool,
    frame_dirty: bool,
}

impl BackgroundUpdateImpact {
    const NONE: Self = Self {
        visual_changed: false,
        frame_dirty: false,
    };
    const VISUAL: Self = Self {
        visual_changed: true,
        frame_dirty: false,
    };
    const FRAME: Self = Self {
        visual_changed: true,
        frame_dirty: true,
    };
}

impl PaneView {
    fn retain_trace_correlation(
        &mut self,
        trace: Option<PerfTraceCorrelation>,
        pane_role: &'static str,
    ) {
        let Some(trace) = trace else { return };
        if self
            .trace_correlation
            .is_some_and(|current| current != trace)
        {
            self.trace_superseded_revisions = self.trace_superseded_revisions.saturating_add(1);
        }
        self.trace_correlation = Some(trace);
        self.trace_pane_role = pane_role;
    }

    fn clear_trace_correlation(&mut self) {
        self.trace_correlation = None;
        self.trace_superseded_revisions = 0;
    }

    fn history_rows_needed_for_viewport_transition(&self) -> Vec<TerminalRow> {
        if self.scrollback_viewport.is_live() {
            Vec::new()
        } else {
            self.snapshot
                .as_ref()
                .map_or_else(Vec::new, |snapshot| snapshot.scrollback_rows.clone())
        }
    }

    fn trace_pane_role(&self, live_role: &'static str) -> &'static str {
        if self.scrollback_viewport.is_live() {
            live_role
        } else {
            "detached-viewport"
        }
    }

    fn pending_commit_trace(&self) -> Option<PaneCommitTrace> {
        let snapshot = self.snapshot.as_ref()?;
        Some(PaneCommitTrace {
            splint_id: snapshot.splint_id,
            incarnation: snapshot.incarnation,
            correlation: self.trace_correlation?,
            pane_role: self.trace_pane_role,
            superseded_revisions: self.trace_superseded_revisions,
        })
    }

    #[cfg(test)]
    fn from_inactive_options(options: WindowPaneOptions, scale_120: u32) -> Result<Self> {
        Self::from_options(options, scale_120)
    }

    fn from_inactive_options_with_context(
        options: WindowPaneOptions,
        scale_120: u32,
        context: &RenderContext,
    ) -> Result<Self> {
        Self::from_options_with_context(options, scale_120, context)
    }

    #[cfg(test)]
    fn from_options(options: WindowPaneOptions, scale_120: u32) -> Result<Self> {
        let snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources(
            &options.snapshot,
            scale_120,
            Some(&options.image_sources),
        )?);
        Ok(Self::from_options_and_frame(options, snapshot_frame))
    }

    fn from_options_with_context(
        options: WindowPaneOptions,
        scale_120: u32,
        context: &RenderContext,
    ) -> Result<Self> {
        let snapshot_frame = Some(SnapshotFrame::load_scaled_with_sources_and_context(
            &options.snapshot,
            scale_120,
            Some(&options.image_sources),
            context,
        )?);
        Ok(Self::from_options_and_frame(options, snapshot_frame))
    }

    fn from_options_and_frame(
        options: WindowPaneOptions,
        snapshot_frame: Option<SnapshotFrame>,
    ) -> Self {
        Self {
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
            control_release_pending: false,
            pending_focus_report: None,
            pending_control_transfer: None,
            search: SearchUiState::default(),
            authority: options.authority,
            last_resize: None,
            prepare_dirty_rows: Vec::new(),
            raster_dirty_rows: Vec::new(),
            surface_dirty_rows: Vec::new(),
            pending_scrolls: Vec::new(),
            trace_correlation: None,
            trace_pane_role: "visible-inactive",
            trace_superseded_revisions: 0,
            selected_text: None,
            selection: None,
            selecting: false,
            pointer_cell: None,
            hovered_url: None,
        }
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
        let cursor_row = viewport_cursor_row(
            snapshot.cursor_row,
            self.scrollback_viewport.offset_from_bottom(),
            snapshot.rows,
        );
        let mut input_modes = snapshot.input_modes;
        if cursor_row.is_none() {
            input_modes.cursor_visible = false;
        }
        Some(TerminalSnapshot {
            splint_id: snapshot.splint_id,
            incarnation: snapshot.incarnation,
            revision: snapshot.revision,
            columns: snapshot.columns,
            rows: snapshot.rows,
            cursor_column: cursor_row.map_or(-1, |_| snapshot.cursor_column),
            cursor_row: cursor_row.unwrap_or(-1),
            cursor_deferred_wrap: false,
            active_screen: snapshot.active_screen,
            input_modes,
            palette: snapshot.palette.clone(),
            default_colors: snapshot.default_colors,
            title: snapshot.title.clone(),
            visible_rows: self
                .scrollback_viewport
                .visible_rows(snapshot)
                .into_iter()
                .cloned()
                .collect(),
            history_generation: snapshot.history_generation,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
            scrollback_rows: Vec::new(),
            available_scrollback_rows: snapshot.available_scrollback_rows,
            omitted_oldest_scrollback_rows: snapshot.available_scrollback_rows,
            images: snapshot.images.clone(),
            exited_code: snapshot.exited_code,
            exited_signal: snapshot.exited_signal,
        })
    }

    fn display_snapshot_cow(&self) -> Option<Cow<'_, TerminalSnapshot>> {
        if self.scrollback_viewport.is_live() {
            self.snapshot.as_ref().map(Cow::Borrowed)
        } else {
            self.display_snapshot().map(Cow::Owned)
        }
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
        pane_role: &'static str,
    ) -> Result<BackgroundUpdateImpact> {
        match update {
            WindowUpdate::Snapshot {
                mut snapshot,
                image_sources,
                authoritative,
            } => {
                snapshot
                    .validate()
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                apply_theme(&mut snapshot, theme);
                if let Some(current) = self.snapshot.as_ref()
                    && !snapshot_replaces(current, &snapshot, authoritative)?
                {
                    return Ok(BackgroundUpdateImpact::NONE);
                }
                let previous_generation = self
                    .snapshot
                    .as_ref()
                    .map_or(snapshot.history_generation, |current| {
                        current.history_generation
                    });
                let previous_rows = self.history_rows_needed_for_viewport_transition();
                self.scrollback_viewport.observe_history_change(
                    previous_generation,
                    &previous_rows,
                    &snapshot,
                );
                self.clear_local_content_state();
                self.snapshot = Some(snapshot);
                self.image_sources = image_sources;
                self.clear_trace_correlation();
                Ok(BackgroundUpdateImpact::FRAME)
            }
            WindowUpdate::Update {
                update,
                image_sources,
                trace,
            } => {
                let apply_started = trace.map(|_| Instant::now());
                let trace_base_revision = update.base_revision;
                let trace_revision = update.revision;
                let trace_rows = update.rows.len();
                let content_changed = terminal_update_changes_visible_content(&update);
                let frame_dirty = content_changed
                    || update.cursor.is_some()
                    || update.input_modes.is_some()
                    || image_sources.is_some();
                let previous_generation = self
                    .snapshot
                    .as_ref()
                    .map_or(1, |snapshot| snapshot.history_generation);
                let previous_rows = self.history_rows_needed_for_viewport_transition();
                let trace_copied_history_bytes =
                    apply_started.map(|_| history_cache_bytes(&previous_rows));
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
                let trace_pane_role = self.trace_pane_role(pane_role);
                if frame_dirty {
                    self.retain_trace_correlation(trace, trace_pane_role);
                }
                if content_changed {
                    self.clear_local_content_state();
                }
                if let (Some(started), Some(trace), Some(snapshot)) =
                    (apply_started, trace, self.snapshot.as_ref())
                {
                    emit_perf_trace(
                        "splinterm",
                        "client_apply",
                        PerfTraceEvent {
                            splint_id: Some(snapshot.splint_id),
                            incarnation: Some(snapshot.incarnation),
                            base_revision: Some(trace_base_revision),
                            revision: Some(trace_revision),
                            subscription_id: Some(trace.subscription_id),
                            transaction_sequence: Some(trace.transaction_sequence),
                            duration_ns: Some(
                                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                            ),
                            rows: Some(u64::try_from(trace_rows).unwrap_or(u64::MAX)),
                            pane_role: Some(trace_pane_role),
                            cached_history_rows: Some(
                                u64::try_from(snapshot.scrollback_rows.len()).unwrap_or(u64::MAX),
                            ),
                            cached_history_bytes: Some(
                                u64::try_from(history_cache_bytes(&snapshot.scrollback_rows))
                                    .unwrap_or(u64::MAX),
                            ),
                            copied_history_rows: Some(
                                u64::try_from(previous_rows.len()).unwrap_or(u64::MAX),
                            ),
                            copied_history_bytes: trace_copied_history_bytes
                                .map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
                            ..PerfTraceEvent::default()
                        },
                    );
                }
                Ok(if frame_dirty {
                    BackgroundUpdateImpact::FRAME
                } else {
                    BackgroundUpdateImpact::VISUAL
                })
            }
            WindowUpdate::Authority(authority) => {
                self.authority = authority;
                Ok(BackgroundUpdateImpact::VISUAL)
            }
            WindowUpdate::Control(active) => {
                self.controller_active = active;
                Ok(BackgroundUpdateImpact::VISUAL)
            }
            WindowUpdate::ControlTransferRequested(transfer_id) => {
                self.pending_control_transfer = Some(transfer_id);
                Ok(BackgroundUpdateImpact::VISUAL)
            }
            WindowUpdate::ControlTransferResolved(_) => {
                self.pending_control_transfer = None;
                Ok(BackgroundUpdateImpact::VISUAL)
            }
            WindowUpdate::SearchResults(page) => {
                self.search.matches = page.matches;
                self.search.selected = 0;
                self.search.next_cursor = page.next_cursor;
                self.search.pending_reveal = self.search.matches.first().cloned();
                Ok(BackgroundUpdateImpact::VISUAL)
            }
            WindowUpdate::SearchResyncRequired => {
                self.search.matches.clear();
                self.search.next_cursor = None;
                self.search.pending_reveal = None;
                Ok(BackgroundUpdateImpact::VISUAL)
            }
            WindowUpdate::ScrollbackResyncRequired => {
                self.history_page_pending = false;
                self.clear_local_content_state();
                Ok(BackgroundUpdateImpact::VISUAL)
            }
            WindowUpdate::ScrollbackPages(pages) => Ok(if self.apply_background_pages(pages)? {
                BackgroundUpdateImpact::FRAME
            } else {
                BackgroundUpdateImpact::NONE
            }),
            WindowUpdate::Theme(_) => Ok(BackgroundUpdateImpact::NONE),
            WindowUpdate::Exited { .. } | WindowUpdate::Shutdown => {
                self.controller_active = false;
                self.commands = None;
                self.updates = None;
                Ok(BackgroundUpdateImpact::VISUAL)
            }
        }
    }
}

struct InactiveUpdateDrain {
    changed: bool,
    theme: Option<ThemeUpdate>,
    dirty_frames: HashSet<SplintId>,
    exited: Vec<SplintId>,
}

fn apply_inactive_update_batch(
    pane: &mut PaneView,
    updates: impl IntoIterator<Item = WindowUpdate>,
    theme: ResolvedTheme,
) -> Result<BackgroundUpdateImpact> {
    let mut batch = BackgroundUpdateImpact::NONE;
    for update in updates {
        let impact = pane.apply_background_update(update, theme, "visible-inactive")?;
        batch.visual_changed |= impact.visual_changed;
        batch.frame_dirty |= impact.frame_dirty;
    }
    Ok(batch)
}

#[cfg(test)]
fn rebuild_inactive_frames(
    panes: &mut [PaneView],
    dirty: &HashSet<SplintId>,
    rebuild_all: bool,
    scale_120: u32,
) -> Result<usize> {
    let mut rebuilt = 0;
    for pane in panes {
        let selected = rebuild_all
            || pane
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| dirty.contains(&snapshot.splint_id));
        if selected && rebuild_pane_scaled_frame(pane, scale_120)? {
            rebuilt += 1;
        }
    }
    Ok(rebuilt)
}

fn rebuild_inactive_frames_with_context(
    panes: &mut [PaneView],
    dirty: &HashSet<SplintId>,
    rebuild_all: bool,
    scale_120: u32,
    context: &RenderContext,
) -> Result<usize> {
    let mut rebuilt = 0;
    for pane in panes {
        let selected = rebuild_all
            || pane
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| dirty.contains(&snapshot.splint_id));
        if selected && rebuild_pane_scaled_frame_with_context(pane, scale_120, context)? {
            rebuilt += 1;
        }
    }
    Ok(rebuilt)
}

struct CachedFrameTitle {
    source: String,
    maximum_cells: u32,
    scale_120: u32,
    bold: bool,
    text: ChromeText,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent Wayland lifecycle and evidence-mode flags are not one state machine"
)]
struct ShmFrameBuffer {
    buffer: Buffer,
    stale: BackingDamage,
}

struct PlatformState {
    registry_state: RegistryState,
    compositor: CompositorState,
    seat_state: SeatState,
    output_state: OutputState,
    data_device_manager: DataDeviceManagerState,
    primary_selection_manager: Option<PrimarySelectionManagerState>,
    text_input_manager: Option<ZwpTextInputManagerV3>,
    shm: Shm,
    loop_handle: LoopHandle<'static, App>,
    update_waker: Waker,
    output_count: usize,
    /// Enter order is significant: the last element is Foot's most-recent output.
    entered_outputs: Vec<wl_output::WlOutput>,
    seat_count: usize,
}

struct SurfaceState {
    fractional_scale: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    background_effect_manager: Option<ExtBackgroundEffectManagerV1>,
    background_effect: Option<ExtBackgroundEffectSurfaceV1>,
    background_effect_state: BackgroundEffectState,
    background_effect_deferred_commit: Option<BackgroundCommitReason>,
    background_effect_reconcile_schedule: BackgroundEffectReconcileSchedule,
    background_effect_capabilities_received: bool,
    background_effect_trace: bool,
    window: Window,
    pool: SlotPool,
    buffers: Vec<ShmFrameBuffer>,
    backing: Vec<u8>,
    logical_width: u32,
    logical_height: u32,
    configured: bool,
    scale_120: u32,
    integer_fallback_scale: u32,
}

struct PresentationState {
    render_context: RenderContext,
    text_row: Option<TextRow>,
    renderer_generation: u64,
    theme_generation: u64,
    cursor_style: CursorStyle,
    cursor_blink: bool,
    title_override: Option<String>,
    theme: ResolvedTheme,
    pane_divider_style: PaneDividerStyle,
    frame_title_mode: FrameTitleMode,
    frame_titles: HashMap<SplintId, CachedFrameTitle>,
    evidence_close_shortcuts: bool,
    font_zoom_steps: i16,
    zoomed_splint: Option<SplintId>,
    capture: Option<PathBuf>,
    capture_scale: Option<u32>,
    full_redraw: bool,
}

#[derive(Clone, Copy, Debug)]
struct DividerDrag {
    split: PaneSplit,
    ratio: Option<SplitRatio>,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent keyboard, IME, focus-reporting, and cursor-blink protocol flags"
)]
struct InputState {
    keymap: ResolvedKeymap,
    prefix_state: PrefixState,
    prefix_timeout: Duration,
    text_input: Option<ZwpTextInputV3>,
    text_input_seat: Option<wl_seat::WlSeat>,
    ime: ImeState,
    reduced_motion: bool,
    keyboard_focused: bool,
    graphical_focus: Option<tokio::sync::watch::Sender<Option<SplintId>>>,
    forced_control_transfer: bool,
    optimistic_remote_splits: bool,
    input_generation: u64,
    terminal_focus_reported: bool,
    ime_generation: u64,
    ime_modal_barrier: bool,
    modifiers: Modifiers,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keyboard_seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    pointer_seat: Option<wl_seat::WlSeat>,
    last_pointer_serial: Option<u32>,
    pressed_buttons: HashMap<u32, PressOwner>,
    divider_drag: Option<DividerDrag>,
    vertical_wheel: WheelAccumulator,
    scrollback_wheel: WheelAccumulator,
    cursor_blink_visible: bool,
    last_cursor_blink: Instant,
}

struct ClipboardState {
    data_device: Option<DataDevice>,
    primary_device: Option<PrimarySelectionDevice>,
    clipboard_offer: Option<SelectionOffer>,
    primary_offer: Option<PrimarySelectionOffer>,
    clipboard_sources: Vec<(CopyPasteSource, Arc<[u8]>)>,
    primary_sources: Vec<(PrimarySelectionSource, Arc<[u8]>)>,
    clipboard_tx: StdSender<ClipboardRead>,
    clipboard_rx: StdReceiver<ClipboardRead>,
}

struct PanesState {
    pane: PaneView,
    inactive_panes: Vec<PaneView>,
    layout: Option<LayoutNode>,
    restored_frontend_needs_resize: bool,
    pending_exited_splints: HashSet<SplintId>,
    pending_remote_splits: HashMap<SplintId, SplintId>,
    dirty_inactive_panes: HashSet<SplintId>,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent modal lifecycle and redraw flags span distinct trusted overlays"
)]
struct ModalState {
    trusted_consent: Option<TrustedConsentUi>,
    command_palette: Option<CommandPaletteUi>,
    command_palette_layout: Option<CommandPaletteLayout>,
    command_palette_pressed: Option<BuiltInCommandId>,
    command_palette_text_cache: CommandPaletteTextCache,
    command_palette_open_focus: Option<bool>,
    command_palette_reconcile_pending: bool,
    dojo_prompt: Option<DojoPromptUi>,
    dojo_prompt_layout: Option<DojoPromptLayout>,
    dojo_prompt_pressed: Option<TerminationDecision>,
    dojo_prompt_text_cache: CommandPaletteTextCache,
    tab_context_menu: Option<TabContextMenuUi>,
    tab_context_menu_anchor: (u32, u32),
    tab_context_menu_layout: Option<TabContextMenuLayout>,
    tab_context_menu_pressed: Option<TabMenuActionId>,
    tab_context_menu_retarget: Option<DojoId>,
    tab_context_menu_text_cache: CommandPaletteTextCache,
    session_picker: Option<SessionPickerUi>,
    selector_kind: Option<SelectorKind>,
    session_picker_targets: Vec<(LairId, DojoId)>,
    session_picker_layout: Option<SessionPickerOverlayLayout>,
    session_picker_pressed: Option<PickerHitTarget>,
    session_picker_text_cache: SessionPickerTextCache,
    session_picker_wheel: WheelAccumulator,
    session_picker_consumed_keys: HashSet<u32>,
    session_picker_redraw: bool,
    session_picker_reconcile_pending: bool,
    session_picker_open_focus: Option<bool>,
    session_picker_requested: bool,
    deferred_picker_theme: Option<ThemeUpdate>,
}

#[derive(Clone, Copy, Debug)]
struct PaneCommitTrace {
    splint_id: SplintId,
    incarnation: u64,
    correlation: PerfTraceCorrelation,
    pane_role: &'static str,
    superseded_revisions: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingFrameTrace {
    commit_sequence: u64,
    committed_monotonic_raw_ns: u64,
}

fn pending_pane_commit_traces(focused: &PaneView, inactive: &[PaneView]) -> Vec<PaneCommitTrace> {
    let mut traces = Vec::with_capacity(inactive.len().saturating_add(1));
    traces.extend(focused.pending_commit_trace());
    traces.extend(inactive.iter().filter_map(PaneView::pending_commit_trace));
    traces
}

fn pane_commit_event(trace: PaneCommitTrace, commit_sequence: u64) -> PerfTraceEvent {
    PerfTraceEvent {
        splint_id: Some(trace.splint_id),
        incarnation: Some(trace.incarnation),
        base_revision: Some(trace.correlation.base_revision),
        revision: Some(trace.correlation.revision),
        subscription_id: Some(trace.correlation.subscription_id),
        transaction_sequence: Some(trace.correlation.transaction_sequence),
        commit_sequence: Some(commit_sequence),
        pane_role: Some(trace.pane_role),
        superseded_revisions: Some(trace.superseded_revisions),
        ..PerfTraceEvent::default()
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent lifecycle, frame-callback, redraw-priority, and trace flags"
)]
struct SchedulingState {
    signoff: Option<SignoffProbe>,
    graphical_input_probe: Option<GraphicalInputProbe>,
    scroll_trace: bool,
    exit: bool,
    failure: Option<anyhow::Error>,
    frame_pending: bool,
    redraw_pending: bool,
    terminal_redraw_pending: bool,
    next_commit_sequence: u64,
    pending_frame_trace: Option<PendingFrameTrace>,
}

struct App {
    platform: PlatformState,
    surface: SurfaceState,
    presentation: PresentationState,
    input: InputState,
    clipboard: ClipboardState,
    panes: PanesState,
    tab_state: TabsState,
    modal: ModalState,
    scheduling: SchedulingState,
}

impl PlatformState {
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
}

impl PanesState {
    fn focused_splint(&self) -> Option<SplintId> {
        self.pane
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.splint_id)
    }

    fn display_snapshot(&self) -> Option<TerminalSnapshot> {
        self.pane.display_snapshot()
    }

    fn display_snapshot_cow(&self) -> Option<Cow<'_, TerminalSnapshot>> {
        self.pane.display_snapshot_cow()
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
}

impl ModalState {
    fn inline_picker_open(&self) -> bool {
        self.session_picker
            .as_ref()
            .is_some_and(SessionPickerUi::is_inline)
    }

    fn input_modal_open(&self) -> bool {
        self.inline_picker_open()
            || self.command_palette.is_some()
            || self.dojo_prompt.is_some()
            || self.tab_context_menu.is_some()
    }
}

impl SchedulingState {
    fn request_exit(&mut self, exit_class: ExitClass) {
        if let Some(diagnostics) = diagnostics() {
            diagnostics.request_exit(exit_class);
        }
        self.exit = true;
    }

    fn fail(&mut self, error: anyhow::Error) {
        if let Some(diagnostics) = diagnostics() {
            diagnostics.emit(
                DiagnosticLevel::Error,
                DiagnosticEventCode::WaylandFailure,
                Some(DiagnosticErrorCode::WaylandDispatch),
            );
        }
        eprintln!("splinterm Wayland client failed");
        self.failure = Some(error);
        self.request_exit(ExitClass::ErrorWaylandDispatch);
    }
}

fn try_window_command(commands: &Sender<WindowCommand>, command: WindowCommand) -> Result<()> {
    commands.try_send(command).map_err(|error| match error {
        TrySendError::Full(_) => anyhow::anyhow!("Wayland command queue overflow"),
        TrySendError::Closed(_) => anyhow::anyhow!("Wayland command receiver disconnected"),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlReleaseOutcome {
    Queued,
    Retry,
    Disconnected,
}

fn try_queue_control_release(commands: &Sender<WindowCommand>) -> ControlReleaseOutcome {
    match commands.try_send(WindowCommand::ReleaseControl) {
        Ok(()) => ControlReleaseOutcome::Queued,
        Err(TrySendError::Full(_)) => ControlReleaseOutcome::Retry,
        Err(TrySendError::Closed(_)) => ControlReleaseOutcome::Disconnected,
    }
}

fn try_queue_focus_report(
    commands: &Sender<WindowCommand>,
    focused: bool,
) -> ControlReleaseOutcome {
    let report = if focused { b"\x1b[I" } else { b"\x1b[O" };
    match commands.try_send(WindowCommand::Input(report.to_vec())) {
        Ok(()) => ControlReleaseOutcome::Queued,
        Err(TrySendError::Full(_)) => ControlReleaseOutcome::Retry,
        Err(TrySendError::Closed(_)) => ControlReleaseOutcome::Disconnected,
    }
}

fn terminal_update_has_visual_damage(
    full: bool,
    cursor_changed: bool,
    raster_dirty_rows: &[bool],
    surface_dirty_rows: &[bool],
) -> bool {
    full || cursor_changed
        || raster_dirty_rows.iter().any(|dirty| *dirty)
        || surface_dirty_rows.iter().any(|dirty| *dirty)
}

fn propagate_raster_damage_through_scroll(
    dirty_rows: &mut [bool],
    scroll: &splinterm_protocol::TerminalScroll,
) {
    let end = scroll.end_row.min(dirty_rows.len());
    let count = scroll.rows.min(end.saturating_sub(scroll.start_row));
    if count == 0 || scroll.start_row >= end || count >= end - scroll.start_row {
        return;
    }
    match scroll.direction {
        splinterm_protocol::ScrollDirection::Forward => {
            for row in scroll.start_row + count..end {
                if dirty_rows[row] {
                    dirty_rows[row - count] = true;
                }
            }
        }
        splinterm_protocol::ScrollDirection::Reverse => {
            for row in (scroll.start_row..end - count).rev() {
                if dirty_rows[row] {
                    dirty_rows[row + count] = true;
                }
            }
        }
    }
}

fn request_return_live_resync(
    commands: Option<&Sender<WindowCommand>>,
    previous_offset: usize,
    current_offset: usize,
) -> Result<bool> {
    if previous_offset == 0 || current_offset != 0 {
        return Ok(false);
    }
    let Some(commands) = commands else {
        return Ok(false);
    };
    try_window_command(commands, WindowCommand::Resynchronize)?;
    Ok(true)
}

fn try_topology_command(
    commands: &Sender<WindowTopologyCommand>,
    command: WindowTopologyCommand,
) -> Result<()> {
    commands.try_send(command).map_err(|error| match error {
        TrySendError::Full(_) => anyhow::anyhow!("topology command queue is full"),
        TrySendError::Closed(_) => anyhow::anyhow!("topology command queue is closed"),
    })
}

fn try_topology_command_with_rollback(
    commands: Option<&Sender<WindowTopologyCommand>>,
    command: WindowTopologyCommand,
    rollback: impl FnOnce(SplintId, SplintId) -> Result<()>,
) -> Result<()> {
    let pending_split = match &command {
        WindowTopologyCommand::Split {
            target,
            pending: Some(pending),
            ..
        } => Some((*target, *pending)),
        _ => None,
    };
    let result = commands
        .context("topology command queue is unavailable")
        .and_then(|commands| try_topology_command(commands, command));
    if let Err(error) = result {
        if let Some((target, pending)) = pending_split {
            rollback(target, pending)
                .context("failed to roll back unsent remote split placeholder")?;
        }
        return Err(error);
    }
    Ok(())
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
                if let Some(leader) = leader
                    && let Some(cell) = snapshot.visible_rows[row].cells.get_mut(leader)
                {
                    cell.content.push(character);
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
            if width == 2
                && let Some(spacer) = snapshot.visible_rows[row].cells.get_mut(column + 1)
            {
                spacer.content.clear();
                spacer.spacer_remaining = Some(1);
            }
            column += width;
        }
    }
    Some(row)
}

const fn presented_cursor_visible(inline_picker_open: bool, blink_phase_visible: bool) -> bool {
    inline_picker_open || blink_phase_visible
}

fn restart_cursor_blink(
    focused_visual_changed: bool,
    visible: &mut bool,
    last_blink: &mut Instant,
) -> bool {
    if !focused_visual_changed {
        return false;
    }
    *visible = true;
    *last_blink = Instant::now();
    true
}

fn cursor_blink_enabled(reduced_motion: bool, focused: bool, modes: TerminalInputModes) -> bool {
    !reduced_motion && focused && modes.cursor_visible && modes.cursor_blink
}

fn event_loop_timeout(
    exiting: bool,
    signoff_active: bool,
    cursor_blink_elapsed: Option<Duration>,
) -> Option<Duration> {
    if exiting {
        None
    } else if signoff_active {
        Some(SIGNOFF_TICK_INTERVAL)
    } else if let Some(elapsed) = cursor_blink_elapsed {
        Some(CURSOR_BLINK_INTERVAL.saturating_sub(elapsed))
    } else {
        Some(IDLE_EVENT_LOOP_TIMEOUT)
    }
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

fn viewport_cursor_row(cursor_row: i32, offset: usize, rows: usize) -> Option<i32> {
    if cursor_row < 0 {
        return None;
    }
    i32::try_from(offset)
        .ok()
        .and_then(|offset| cursor_row.checked_add(offset))
        .filter(|row| *row >= 0 && usize::try_from(*row).is_ok_and(|row| row < rows))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackgroundEffectCommitMode {
    Immediate,
    DeferToDraw,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ThemeUpdateImpact {
    rebuild_pixels: bool,
    reconcile_effect: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BackgroundEffectReconcileSchedule {
    on_draw: bool,
}

impl BackgroundEffectReconcileSchedule {
    fn queue_update(
        &mut self,
        effect_changed: bool,
        visual_commit_queued: bool,
        configured: bool,
    ) -> bool {
        if !effect_changed {
            return false;
        }
        if self.on_draw || visual_commit_queued && configured {
            self.on_draw = true;
            false
        } else {
            true
        }
    }

    fn queue_geometry(&mut self) {
        self.on_draw = true;
    }

    fn capability_reconciles_immediately(self) -> bool {
        !self.on_draw
    }

    fn take_for_draw(&mut self) -> bool {
        std::mem::take(&mut self.on_draw)
    }

    fn clear(&mut self) {
        self.on_draw = false;
    }
}

const fn reported_graphical_focus(
    keyboard_focused: bool,
    focused_splint_id: Option<SplintId>,
) -> Option<SplintId> {
    if keyboard_focused {
        focused_splint_id
    } else {
        None
    }
}

fn update_graphical_focus_watch(
    sender: Option<&tokio::sync::watch::Sender<Option<SplintId>>>,
    keyboard_focused: bool,
    focused_splint_id: Option<SplintId>,
) {
    let focused = reported_graphical_focus(keyboard_focused, focused_splint_id);
    if let Some(sender) = sender {
        sender.send_if_modified(|current| {
            if *current == focused {
                false
            } else {
                *current = focused;
                true
            }
        });
    }
}

fn retain_newest_theme(slot: &mut Option<ThemeUpdate>, update: ThemeUpdate) {
    if slot
        .as_ref()
        .is_none_or(|current| update.generation > current.generation)
    {
        *slot = Some(update);
    }
}

fn classify_theme_update(current: ResolvedTheme, next: ResolvedTheme) -> ThemeUpdateImpact {
    let reconcile_effect = current.background_alpha != next.background_alpha
        || current.background_blur != next.background_blur;
    let mut current_pixels = current;
    let mut next_pixels = next;
    current_pixels.background_blur = false;
    next_pixels.background_blur = false;
    ThemeUpdateImpact {
        rebuild_pixels: current_pixels != next_pixels,
        reconcile_effect,
    }
}

fn background_effect_capability_bits(flags: WEnum<BackgroundCapability>) -> u32 {
    match flags {
        WEnum::Value(flags) => flags.bits(),
        WEnum::Unknown(flags) => flags,
    }
}

fn background_effect_diagnostic_message(diagnostic: EffectDiagnostic) -> &'static str {
    match diagnostic {
        EffectDiagnostic::MissingManager => {
            "splinterm background blur requested, but ext-background-effect-v1 is unavailable"
        }
        EffectDiagnostic::MissingBlurCapability => {
            "splinterm background blur requested, but the compositor advertises no blur capability"
        }
    }
}

fn background_effect_trace_line(action: EffectAction) -> Option<String> {
    match action {
        EffectAction::Diagnostic(_) => None,
        EffectAction::CreateEffect => Some("splinterm background-effect create".to_owned()),
        EffectAction::SetBlurRegion(size) => Some(format!(
            "splinterm background-effect region={}x{}",
            size.width(),
            size.height()
        )),
        EffectAction::DestroyEffect => Some("splinterm background-effect destroy".to_owned()),
        EffectAction::CommitSurface(reason) => {
            Some(format!("splinterm background-effect commit={reason:?}"))
        }
    }
}

fn remote_split_can_begin(pending: &HashMap<SplintId, SplintId>) -> bool {
    pending.is_empty()
}

fn is_pending_remote_splint(pending: &HashMap<SplintId, SplintId>, splint_id: SplintId) -> bool {
    pending.values().any(|candidate| *candidate == splint_id)
}

fn insert_pending_split(
    node: &mut LayoutNode,
    target: SplintId,
    pending: Splint,
    axis: Axis,
) -> bool {
    match node {
        LayoutNode::Leaf(splint) if splint.id == target => {
            let current = LayoutNode::Leaf(splint.clone());
            *node = LayoutNode::Branch {
                axis,
                ratio: SplitRatio::new(500).expect("fixed pending split ratio is valid"),
                first: Box::new(current),
                second: Box::new(LayoutNode::Leaf(pending)),
            };
            true
        }
        LayoutNode::Leaf(_) => false,
        LayoutNode::Branch { first, second, .. } => {
            if first.find_splint(target).is_some() {
                insert_pending_split(first, target, pending, axis)
            } else {
                insert_pending_split(second, target, pending, axis)
            }
        }
    }
}

fn remove_pending_split(node: LayoutNode, pending: SplintId) -> (Option<LayoutNode>, bool) {
    match node {
        LayoutNode::Leaf(splint) if splint.id == pending => (None, true),
        leaf @ LayoutNode::Leaf(_) => (Some(leaf), false),
        LayoutNode::Branch {
            axis,
            ratio,
            first,
            second,
        } => {
            let (first, removed_first) = remove_pending_split(*first, pending);
            let (second, removed_second) = remove_pending_split(*second, pending);
            let removed = removed_first || removed_second;
            let node = match (first, second) {
                (Some(first), Some(second)) => Some(LayoutNode::Branch {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            };
            (node, removed)
        }
    }
}

fn pending_remote_snapshot(splint_id: SplintId, columns: usize, rows: usize) -> TerminalSnapshot {
    let message_cells = "Opening remote pane…"
        .chars()
        .take(columns)
        .map(|character| TerminalCell {
            content: character.to_string(),
            spacer_remaining: None,
            attributes: splinterm_protocol::CellAttributes::default(),
        })
        .collect::<Vec<_>>();
    let visible_rows = (0..rows)
        .map(|index| TerminalRow {
            row_id: u64::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1)),
            linebreak: false,
            cells: if index == 0 {
                message_cells.clone()
            } else {
                Vec::new()
            },
        })
        .collect();
    TerminalSnapshot {
        splint_id,
        incarnation: 1,
        revision: 1,
        columns,
        rows,
        cursor_column: 0,
        cursor_row: 0,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: TerminalInputModes {
            application_cursor: false,
            application_keypad: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor_visible: false,
            cursor_blink: false,
            mouse_tracking: MouseTracking::None,
            sgr_mouse: false,
        },
        palette: vec![0; 256],
        default_colors: [0xff_d8dee9, 0xff_2e3440, 0xff_d8dee9],
        title: "Opening remote pane…".to_owned(),
        visible_rows,
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

impl App {
    fn content_rect(&self) -> Rect {
        let y = if self.tab_state.managed_tabs {
            TAB_STRIP_LOGICAL_HEIGHT.min(self.surface.logical_height)
        } else {
            0
        };
        Rect {
            x: 0,
            y,
            width: self.surface.logical_width,
            height: self.surface.logical_height.saturating_sub(y),
        }
    }

    fn request_pane_control_release(pane: &mut PaneView) {
        if !pane.controller_active {
            return;
        }
        let outcome = pane.commands.as_ref().map_or(
            ControlReleaseOutcome::Disconnected,
            try_queue_control_release,
        );
        match outcome {
            ControlReleaseOutcome::Queued => {
                pane.controller_active = false;
                pane.control_release_pending = false;
            }
            ControlReleaseOutcome::Retry => pane.control_release_pending = true,
            ControlReleaseOutcome::Disconnected => {
                pane.controller_active = false;
                pane.control_release_pending = false;
                pane.commands = None;
            }
        }
    }

    fn retry_pane_control_release(pane: &mut PaneView) {
        if pane.control_release_pending {
            Self::request_pane_control_release(pane);
        }
    }

    fn release_tab_controllers(view: &mut DojoTabView) {
        for pane in std::iter::once(&mut view.pane).chain(view.inactive_panes.iter_mut()) {
            Self::request_pane_control_release(pane);
        }
    }

    fn request_pane_focus_report(pane: &mut PaneView, focused: bool) {
        let outcome = pane
            .commands
            .as_ref()
            .map_or(ControlReleaseOutcome::Disconnected, |commands| {
                try_queue_focus_report(commands, focused)
            });
        match outcome {
            ControlReleaseOutcome::Queued => pane.pending_focus_report = None,
            ControlReleaseOutcome::Retry => pane.pending_focus_report = Some(focused),
            ControlReleaseOutcome::Disconnected => {
                pane.controller_active = false;
                pane.pending_focus_report = None;
                pane.commands = None;
            }
        }
    }

    fn retry_pane_focus_report(pane: &mut PaneView) {
        if let Some(focused) = pane.pending_focus_report {
            Self::request_pane_focus_report(pane, focused);
        }
    }

    fn activate_tab(&mut self, dojo_id: DojoId) -> Result<bool> {
        let started = Instant::now();
        let previous_id = self.tab_state.active_dojo_id();
        if previous_id == dojo_id {
            return Ok(false);
        }
        anyhow::ensure!(
            self.tab_state
                .tabs
                .get(dojo_id)
                .is_some_and(|tab| tab.value.is_some()),
            "activated Dojo tab has no hidden frontend"
        );
        self.settle_terminal_presses_for_picker();
        self.input.prefix_state.clear();
        self.presentation.zoomed_splint = None;
        self.input.input_generation = self.input.input_generation.saturating_add(1);
        if self.input.terminal_focus_reported {
            Self::request_pane_focus_report(&mut self.panes.pane, false);
            self.input.terminal_focus_reported = false;
        }
        for pane in
            std::iter::once(&mut self.panes.pane).chain(self.panes.inactive_panes.iter_mut())
        {
            Self::request_pane_control_release(pane);
        }
        let next = self
            .tab_state
            .tabs
            .get_mut(dojo_id)
            .and_then(|tab| tab.value.take())
            .context("validated Dojo tab frontend disappeared")?;
        let previous = DojoTabView {
            identity: std::mem::replace(&mut self.tab_state.active_identity, next.identity),
            pane: std::mem::replace(&mut self.panes.pane, next.pane),
            inactive_panes: std::mem::replace(&mut self.panes.inactive_panes, next.inactive_panes),
            layout: std::mem::replace(&mut self.panes.layout, next.layout),
            pending_exited_splints: std::mem::replace(
                &mut self.panes.pending_exited_splints,
                next.pending_exited_splints,
            ),
            pending_remote_splits: std::mem::replace(
                &mut self.panes.pending_remote_splits,
                next.pending_remote_splits,
            ),
            frame_titles: std::mem::replace(&mut self.presentation.frame_titles, next.frame_titles),
            dirty_inactive_panes: std::mem::replace(
                &mut self.panes.dirty_inactive_panes,
                next.dirty_inactive_panes,
            ),
        };
        self.tab_state
            .tabs
            .get_mut(previous_id)
            .context("previous Dojo tab disappeared")?
            .value = Some(previous);
        anyhow::ensure!(
            self.tab_state.tabs.activate(dojo_id),
            "activated Dojo tab is absent"
        );
        self.sync_graphical_focus();
        rebuild_pane_scaled_frame_with_context(
            &mut self.panes.pane,
            self.surface.scale_120,
            &self.presentation.render_context,
        )?;
        for pane in &mut self.panes.inactive_panes {
            rebuild_pane_scaled_frame_with_context(
                pane,
                self.surface.scale_120,
                &self.presentation.render_context,
            )?;
        }
        self.panes.dirty_inactive_panes.clear();
        self.panes.restored_frontend_needs_resize = true;
        self.input.cursor_blink_visible = true;
        self.input.last_cursor_blink = Instant::now();
        self.presentation.full_redraw = true;
        self.update_window_title();
        self.clear_ime_preedit();
        self.update_ime_cursor_rectangle();
        if self.input.keyboard_focused && self.panes.input_modes().focus_reporting {
            Self::request_pane_focus_report(&mut self.panes.pane, true);
            self.input.terminal_focus_reported = true;
        }
        if perf_trace_enabled() {
            emit_perf_trace(
                "splinterm",
                "tab_switch",
                PerfTraceEvent {
                    splint_id: self.panes.focused_splint(),
                    duration_ns: Some(
                        u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                    ),
                    count: Some(u64::try_from(self.tab_state.tabs.len()).unwrap_or(u64::MAX)),
                    ..PerfTraceEvent::default()
                },
            );
        }
        Ok(true)
    }

    fn trace_background_effect_action(&self, action: EffectAction) {
        if self.surface.background_effect_trace
            && let Some(line) = background_effect_trace_line(action)
        {
            eprintln!("{line}");
        }
    }

    fn execute_background_effect_actions(
        &mut self,
        queue_handle: &QueueHandle<Self>,
        commit_mode: BackgroundEffectCommitMode,
    ) -> Result<()> {
        for action in self.surface.background_effect_state.reconcile() {
            self.trace_background_effect_action(action);
            match action {
                EffectAction::Diagnostic(diagnostic) => {
                    eprintln!("{}", background_effect_diagnostic_message(diagnostic));
                }
                EffectAction::CreateEffect => {
                    anyhow::ensure!(
                        self.surface.background_effect.is_none(),
                        "background effect reducer attempted duplicate object creation"
                    );
                    let manager = self
                        .surface
                        .background_effect_manager
                        .as_ref()
                        .context("background effect manager disappeared before creation")?;
                    self.surface.background_effect = Some(manager.get_background_effect(
                        self.surface.window.wl_surface(),
                        queue_handle,
                        (),
                    ));
                }
                EffectAction::SetBlurRegion(size) => {
                    let effect = self
                        .surface
                        .background_effect
                        .as_ref()
                        .context("background effect region requested without an object")?;
                    let region = Region::new(&self.platform.compositor)
                        .context("create finite background effect region")?;
                    region.add(0, 0, size.width(), size.height());
                    effect.set_blur_region(Some(region.wl_region()));
                    drop(region);
                }
                EffectAction::DestroyEffect => {
                    let effect = self
                        .surface
                        .background_effect
                        .take()
                        .context("background effect destroy requested without an object")?;
                    effect.destroy();
                }
                EffectAction::CommitSurface(reason) => match commit_mode {
                    BackgroundEffectCommitMode::Immediate => {
                        self.surface.window.commit();
                        anyhow::ensure!(
                            self.surface.background_effect_state.surface_committed()
                                == Some(reason),
                            "background effect commit did not match reducer state"
                        );
                    }
                    BackgroundEffectCommitMode::DeferToDraw => {
                        anyhow::ensure!(
                            self.surface
                                .background_effect_deferred_commit
                                .replace(reason)
                                .is_none(),
                            "background effect already has a deferred surface commit"
                        );
                    }
                },
            }
        }
        Ok(())
    }

    fn complete_background_effect_draw_commit(&mut self) -> Result<()> {
        let Some(expected) = self.surface.background_effect_deferred_commit.take() else {
            return Ok(());
        };
        anyhow::ensure!(
            self.surface.background_effect_state.surface_committed() == Some(expected),
            "draw commit did not match deferred background effect state"
        );
        Ok(())
    }

    fn reconcile_background_effect(
        &mut self,
        queue_handle: &QueueHandle<Self>,
        commit_mode: BackgroundEffectCommitMode,
    ) -> Result<()> {
        if self.surface.background_effect_manager.is_none()
            || self.surface.background_effect_capabilities_received
        {
            self.execute_background_effect_actions(queue_handle, commit_mode)?;
        }
        Ok(())
    }

    fn queue_background_effect_geometry_for_draw(&mut self) -> Result<()> {
        self.surface.background_effect_state.set_logical_size(
            i64::from(self.surface.logical_width),
            i64::from(self.surface.logical_height),
        )?;
        self.surface
            .background_effect_reconcile_schedule
            .queue_geometry();
        Ok(())
    }

    fn release_background_effect(&mut self) {
        for action in self.surface.background_effect_state.destroy_surface() {
            self.trace_background_effect_action(action);
            if action == EffectAction::DestroyEffect
                && let Some(effect) = self.surface.background_effect.take()
            {
                effect.destroy();
            }
        }
        self.surface.background_effect_deferred_commit = None;
        self.surface.background_effect_reconcile_schedule.clear();
        if let Some(manager) = self.surface.background_effect_manager.take() {
            manager.destroy();
        }
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
        if is_pending_remote_splint(&self.panes.pending_remote_splits, splint_id) {
            return false;
        }
        if self.panes.focused_splint() == Some(splint_id) {
            return false;
        }
        let Some(index) = self.panes.inactive_panes.iter().position(|pane| {
            pane.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.splint_id == splint_id)
        }) else {
            return false;
        };
        if self.presentation.zoomed_splint.is_some() {
            self.presentation.zoomed_splint = None;
            self.panes.restored_frontend_needs_resize = true;
        }
        std::mem::swap(&mut self.panes.pane, &mut self.panes.inactive_panes[index]);
        self.panes.pane.pointer_cell = None;
        self.panes.pane.hovered_url = None;
        self.presentation.full_redraw = true;
        self.sync_graphical_focus();
        true
    }

    fn directional_splint(&self, direction: FocusDirection) -> Option<SplintId> {
        let (Some(layout), Some(current)) = (&self.panes.layout, self.panes.focused_splint())
        else {
            return None;
        };
        PaneLayout::compute(
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
        )
        .ok()?
        .directional(current, direction)
    }

    fn focus_direction(&mut self, direction: FocusDirection) -> bool {
        if self.presentation.zoomed_splint.is_some() {
            return false;
        }
        self.directional_splint(direction)
            .is_some_and(|next| self.focus_splint(next))
    }

    fn toggle_pane_zoom(&mut self) -> bool {
        let Some(focused) = self.panes.focused_splint() else {
            return false;
        };
        self.presentation.zoomed_splint = if self.presentation.zoomed_splint == Some(focused) {
            None
        } else {
            Some(focused)
        };
        self.panes.restored_frontend_needs_resize = true;
        self.presentation.full_redraw = true;
        self.update_ime_cursor_rectangle();
        true
    }

    fn directional_resize_command(
        &self,
        direction: FocusDirection,
        cells: u16,
    ) -> Result<Option<WindowTopologyCommand>> {
        let (Some(root), Some(layout), Some(current), Some(frame)) = (
            self.panes.layout.as_ref(),
            self.computed_pane_layout()?,
            self.panes.focused_splint(),
            self.panes.pane.snapshot_frame.as_ref(),
        ) else {
            return Ok(None);
        };
        let cell_width = buffer_to_logical_ceil(frame.cell_width(), self.surface.scale_120)?;
        let cell_height = buffer_to_logical_ceil(frame.cell_height(), self.surface.scale_120)?;
        let minimum_width = cell_width
            .checked_mul(2)
            .context("minimum pane width overflow")?;
        let minimum_height = cell_height
            .checked_mul(2)
            .context("minimum pane height overflow")?;
        let Some((target, ancestor, ratio)) = directional_resize_ratio(
            root,
            &layout,
            current,
            direction,
            cells,
            PaneResizeMetrics {
                cell_width,
                cell_height,
                minimum_width,
                minimum_height,
            },
        ) else {
            return Ok(None);
        };
        Ok(Some(WindowTopologyCommand::SetRatio {
            dojo_id: self.tab_state.active_dojo_id(),
            target,
            ancestor,
            ratio,
        }))
    }

    fn prepare_frame_titles(
        &mut self,
        pane_layout: Option<&PaneLayout>,
        cell_width: u32,
    ) -> Result<()> {
        if self.presentation.pane_divider_style != PaneDividerStyle::Frame
            || self.presentation.frame_title_mode != FrameTitleMode::Splint
        {
            self.presentation.frame_titles.clear();
            return Ok(());
        }
        let (Some(pane_layout), Some(topology)) = (pane_layout, self.panes.layout.as_ref()) else {
            self.presentation.frame_titles.clear();
            return Ok(());
        };
        let mut requested = Vec::new();
        for pane in &pane_layout.panes {
            let allocation = Self::buffer_rect(pane.allocation, self.surface.scale_120)?;
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
        self.presentation
            .frame_titles
            .retain(|splint_id, _| requested_ids.contains(splint_id));
        for (splint_id, source, maximum_cells) in requested {
            let current = self
                .presentation
                .frame_titles
                .get(&splint_id)
                .is_some_and(|cached| {
                    cached.source == source
                        && cached.maximum_cells == maximum_cells
                        && cached.scale_120 == self.surface.scale_120
                });
            if !current {
                self.presentation.frame_titles.insert(
                    splint_id,
                    CachedFrameTitle {
                        text: ChromeText::load_with_context(
                            &source,
                            self.surface.scale_120,
                            &self.presentation.render_context,
                        )?,
                        source,
                        maximum_cells,
                        scale_120: self.surface.scale_120,
                        bold: false,
                    },
                );
            }
        }
        Ok(())
    }

    fn computed_pane_layout(&self) -> Result<Option<PaneLayout>> {
        let Some(topology) = self.panes.layout.as_ref() else {
            return Ok(None);
        };
        let zoomed = self
            .presentation
            .zoomed_splint
            .and_then(|splint_id| topology.find_splint(splint_id).cloned())
            .map(LayoutNode::Leaf);
        let layout = zoomed.as_ref().unwrap_or(topology);
        let frame = self
            .panes
            .pane
            .snapshot_frame
            .as_ref()
            .context("multi-pane layout requires an active snapshot frame")?;
        let cell_width = buffer_to_logical_ceil(frame.cell_width(), self.surface.scale_120)?;
        let cell_height = buffer_to_logical_ceil(frame.cell_height(), self.surface.scale_120)?;
        let chrome = match self.presentation.pane_divider_style {
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
            self.content_rect(),
            chrome,
            minimum_width,
            minimum_height,
        )
        .map(Some)
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

    fn request_older_history(&mut self) -> Result<()> {
        if self.panes.pane.history_page_pending || self.panes.pane.history_selection_pin_blocked {
            return Ok(());
        }
        let Some(snapshot) = self.panes.pane.snapshot.as_ref() else {
            return Ok(());
        };
        if snapshot.omitted_oldest_scrollback_rows == 0 {
            return Ok(());
        }
        let Some(before_row_id) = snapshot.scrollback_rows.first().and_then(|row| row.row_id)
        else {
            return Ok(());
        };
        let Some(commands) = self.panes.pane.commands.as_ref() else {
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
        self.panes.pane.history_page_pending = true;
        Ok(())
    }

    fn scroll_history(&mut self, action: MouseAction, lines: usize) -> Result<bool> {
        let snapshot = self
            .panes
            .pane
            .snapshot
            .as_ref()
            .context("scroll requires snapshot")?;
        let previous_offset = self.panes.pane.scrollback_viewport.offset_from_bottom();
        match action {
            MouseAction::WheelUp => self
                .panes
                .pane
                .scrollback_viewport
                .scroll_up(lines, snapshot),
            MouseAction::WheelDown => self
                .panes
                .pane
                .scrollback_viewport
                .scroll_down(lines, snapshot),
            _ => return Ok(false),
        }
        let current_offset = self.panes.pane.scrollback_viewport.offset_from_bottom();
        let moved = current_offset != previous_offset;
        request_return_live_resync(
            self.panes.pane.commands.as_ref(),
            previous_offset,
            current_offset,
        )?;
        if action == MouseAction::WheelUp {
            let loaded = snapshot.scrollback_rows.len();
            let remaining =
                loaded.saturating_sub(self.panes.pane.scrollback_viewport.offset_from_bottom());
            let prefetch_distance = snapshot.rows.saturating_mul(2).max(32);
            if remaining <= prefetch_distance {
                self.request_older_history()?;
            }
        }
        if !moved {
            return Ok(false);
        }
        self.panes
            .pane
            .scroll_started_at
            .get_or_insert_with(Instant::now);
        self.invalidate_viewport_local_state();
        self.refresh_ime_preedit()?;
        self.update_ime_cursor_rectangle();
        // Coalesce high-resolution wheel events until the next compositor frame.
        // Re-shaping the entire viewport synchronously for every axis event made
        // fast scrolling stall the Wayland dispatch loop.
        self.panes.pane.viewport_dirty = true;
        Ok(true)
    }

    fn tick_signoff(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let Some(mut probe) = self.scheduling.signoff.take() else {
            return Ok(());
        };
        let result = self.advance_signoff(&mut probe);
        if let Err(error) = &result {
            self.write_signoff_report(&probe, false, Some(&error.to_string()))?;
        } else {
            self.write_signoff_report(&probe, probe.step == SignoffStep::Complete, None)?;
        }
        self.scheduling.signoff = Some(probe);
        result?;
        if self.surface.configured && self.panes.pane.viewport_dirty {
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
        let Some(snapshot) = self.panes.pane.snapshot.as_ref() else {
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
                    .panes
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
                let snapshot = self
                    .panes
                    .pane
                    .snapshot
                    .as_ref()
                    .context("sign-off snapshot")?;
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
                    && self.panes.pane.scrollback_viewport.offset_from_bottom()
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
                        "offset": self.panes.pane.scrollback_viewport.offset_from_bottom(),
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
                        .panes
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
                    .panes
                    .display_snapshot()
                    .context("selection display")?
                    .rows
                    .saturating_sub(1);
                anyhow::ensure!(
                    self.extend_selection(CellPosition { row, column: 8 }),
                    "could not extend cross-page selection"
                );
                let selection = self.panes.pane.selection.context("selection exists")?;
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
                    let selection = self
                        .panes
                        .pane
                        .selection
                        .context("selection survived output")?;
                    anyhow::ensure!(
                        selection_is_retained(snapshot, selection),
                        "selection endpoints were not retained during output"
                    );
                    probe.evidence.push(serde_json::json!({
                        "check": "selection_during_detached_output",
                        "revision_before": probe.selection_revision,
                        "revision_after": snapshot.revision,
                        "unseen_rows": self.panes.pane.scrollback_viewport.unseen_rows(),
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
                self.dirty_selection(self.panes.pane.selection);
                self.panes.pane.selection = None;
                self.panes.pane.selected_text = None;
                self.panes.pane.history_selection_pin_blocked = false;
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
                if self.panes.pane.scrollback_viewport.is_live() {
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
        let snapshot = self.panes.pane.snapshot.as_ref();
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
                "history_page_pending": self.panes.pane.history_page_pending,
                "viewport_offset": self.panes.pane.scrollback_viewport.offset_from_bottom(),
                "mouse_tracking": snapshot.map(|value| format!("{:?}", value.input_modes.mouse_tracking)),
                "selection_active": self.panes.pane.selection.is_some(),
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
        let navigation =
            match shortcut_action_for(&self.input.keymap, event.keysym, self.input.modifiers) {
                Some(ActionId::PageUp) => HistoryNavigation::PageUp,
                Some(ActionId::PageDown) => HistoryNavigation::PageDown,
                Some(ActionId::ReturnToLive) if !self.panes.pane.scrollback_viewport.is_live() => {
                    HistoryNavigation::ReturnToLive
                }
                _ => return Ok(false),
            };
        let page = self
            .panes
            .pane
            .snapshot
            .as_ref()
            .map_or(1, |snapshot| snapshot.rows.saturating_sub(1).max(1));
        match navigation {
            HistoryNavigation::PageUp => {
                if self.scroll_history(MouseAction::WheelUp, page)? && self.surface.configured {
                    self.schedule_draw(queue_handle)?;
                }
            }
            HistoryNavigation::PageDown => {
                if self.scroll_history(MouseAction::WheelDown, page)? && self.surface.configured {
                    self.schedule_draw(queue_handle)?;
                }
            }
            HistoryNavigation::ReturnToLive => {
                if self.scroll_history(MouseAction::WheelDown, usize::MAX)?
                    && self.surface.configured
                {
                    self.schedule_draw(queue_handle)?;
                }
            }
        }
        Ok(true)
    }

    fn send_input(&mut self, bytes: Vec<u8>) -> Result<()> {
        if self.modal.input_modal_open() {
            return Ok(());
        }
        if let Some(commands) = &self.panes.pane.commands {
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
        let modes = self.panes.input_modes();
        let cell_height = self
            .panes
            .pane
            .snapshot_frame
            .as_ref()
            .map_or(1, SnapshotFrame::cell_height);
        if modes.mouse_tracking == MouseTracking::None {
            let before = self.panes.pane.scrollback_viewport.offset_from_bottom();
            let Some((action, count)) = self.input.scrollback_wheel.push_scaled(
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
                after: self.panes.pane.scrollback_viewport.offset_from_bottom(),
            });
        }
        let Some((action, count)) =
            self.input
                .vertical_wheel
                .push(absolute, discrete, value120, cell_height)
        else {
            return Ok(WheelOutcome::Noop);
        };
        let Some(position) = position else {
            return Ok(WheelOutcome::Noop);
        };
        let Some(report) = mouse_report(action, position, self.input.modifiers, modes.sgr_mouse)
        else {
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

    fn clear_selection(&mut self) {
        let selection = self.panes.pane.selection.take();
        self.dirty_selection(selection);
        self.panes.pane.selected_text = None;
        self.panes.pane.selecting = false;
        self.panes.pane.history_selection_pin_blocked = false;
    }

    fn begin_selection(&mut self, position: CellPosition) -> bool {
        let Some(snapshot) = self.panes.display_snapshot_cow() else {
            return false;
        };
        let Some(endpoint) = selection_endpoint(&snapshot, position) else {
            return false;
        };
        self.dirty_selection(self.panes.pane.selection);
        let selection = Selection {
            anchor: endpoint,
            end: endpoint,
        };
        self.panes.pane.selection = Some(selection);
        self.panes.pane.selecting = true;
        self.panes.pane.history_selection_pin_blocked = false;
        self.dirty_selection(Some(selection));
        true
    }

    fn extend_selection(&mut self, position: CellPosition) -> bool {
        let (Some(mut selection), Some(snapshot)) =
            (self.panes.pane.selection, self.panes.display_snapshot_cow())
        else {
            return false;
        };
        let Some(endpoint) = selection_endpoint(&snapshot, position) else {
            return false;
        };
        if endpoint == selection.end {
            return false;
        }
        self.dirty_selection(Some(selection));
        selection.end = endpoint;
        self.panes.pane.selection = Some(selection);
        self.dirty_selection(Some(selection));
        true
    }

    fn finish_selection(&mut self) -> Option<&[u8]> {
        self.panes.pane.selecting = false;
        self.panes.pane.selected_text =
            self.panes.pane.selection.and_then(|selection| {
                self.panes.pane.snapshot.as_ref().and_then(|snapshot| {
                    selection_text(snapshot, selection).map(String::into_bytes)
                })
            });
        self.panes.pane.selected_text.as_deref()
    }

    fn pointer_cell_at(&self, position: (f64, f64)) -> Option<CellPosition> {
        let frame = self.panes.pane.snapshot_frame.as_ref()?;
        let pane_rect = self.panes.focused_splint().and_then(|splint_id| {
            self.computed_pane_layout()
                .ok()
                .flatten()
                .and_then(|layout| layout.rect(splint_id))
        });
        let content = self.content_rect();
        let (logical_width, logical_height, x, y) = pane_rect.map_or(
            (
                content.width,
                content.height,
                f64::from(content.x),
                f64::from(content.y),
            ),
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
            .window_geometry(logical_width, logical_height, self.surface.scale_120)
            .ok()?;
        let (row, column) = frame.cell_at(position.0 - x, position.1 - y, &geometry)?;
        Some(CellPosition { row, column })
    }

    fn dirty_row(&mut self, row: usize) {
        let rows = self
            .panes
            .pane
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.rows);
        self.panes.pane.raster_dirty_rows.resize(rows, false);
        self.panes.pane.surface_dirty_rows.resize(rows, false);
        if row < rows {
            self.panes.pane.raster_dirty_rows[row] = true;
            self.panes.pane.surface_dirty_rows[row] = true;
        }
    }

    fn dirty_selection(&mut self, selection: Option<Selection>) {
        let bounds = selection.and_then(|selection| {
            let snapshot = self.panes.pane.snapshot.as_ref()?;
            let display = self.panes.display_snapshot_cow()?;
            selection_display_bounds(snapshot, &display, selection)
        });
        if let Some((start, end)) = bounds {
            let rows = self
                .panes
                .pane
                .snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.rows);
            self.panes.pane.surface_dirty_rows.resize(rows, false);
            for row in start.row..=end.row.min(rows.saturating_sub(1)) {
                self.panes.pane.surface_dirty_rows[row] = true;
            }
        }
    }

    fn invalidate_viewport_local_state(&mut self) {
        if let Some((start, _, _)) = &self.panes.pane.hovered_url {
            self.dirty_row(start.row);
        }
        self.panes.pane.selected_text = None;
        self.panes.pane.hovered_url = None;
        let selecting = self.panes.pane.selecting;
        self.input.pressed_buttons.retain(|_, owner| {
            matches!(owner, PressOwner::Application { .. })
                || selecting && matches!(owner, PressOwner::Selection)
        });
    }

    fn invalidate_local_content_state(&mut self) {
        self.dirty_selection(self.panes.pane.selection);
        self.panes.pane.selection = None;
        self.panes.pane.selecting = false;
        self.panes.pane.history_selection_pin_blocked = false;
        self.invalidate_viewport_local_state();
    }

    fn reconcile_selection_after_content_change(&mut self) {
        let retained = self.panes.pane.selection.is_none_or(|selection| {
            self.panes
                .pane
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| selection_is_retained(snapshot, selection))
        });
        if !retained {
            self.panes.pane.selection = None;
            self.panes.pane.selecting = false;
            self.panes.pane.history_selection_pin_blocked = false;
        }
        self.invalidate_viewport_local_state();
    }

    fn recompute_hovered_url(&mut self) {
        let previous = self.panes.pane.hovered_url.take();
        let display = self.panes.display_snapshot();
        self.panes.pane.hovered_url = self.panes.pane.pointer_cell.and_then(|position| {
            display
                .as_ref()
                .and_then(|snapshot| url_at(snapshot, position))
        });
        if previous != self.panes.pane.hovered_url {
            if let Some((start, _, _)) = previous {
                self.dirty_row(start.row);
            }
            if let Some((start, _, _)) = &self.panes.pane.hovered_url {
                self.dirty_row(start.row);
            }
        }
    }

    fn begin_clipboard_read(&mut self, target: PasteTarget) {
        if self.modal.input_modal_open() {
            return;
        }
        let tx = self.clipboard.clipboard_tx.clone();
        let waker = self.platform.update_waker.clone();
        let input_generation = self.input.input_generation;
        match target {
            PasteTarget::Clipboard => {
                let Some(offer) = self.clipboard.clipboard_offer.clone() else {
                    return;
                };
                let mime = offer.with_mime_types(accepted_text_mime);
                let Some(mime) = mime else { return };
                if let Ok(pipe) = offer.receive(mime) {
                    spawn_clipboard_read(pipe.into(), target, input_generation, tx, waker);
                }
            }
            PasteTarget::Primary => {
                let Some(offer) = self.clipboard.primary_offer.clone() else {
                    return;
                };
                let mime = offer.with_mime_types(accepted_text_mime);
                let Some(mime) = mime else { return };
                if let Ok(pipe) = offer.receive(mime) {
                    spawn_clipboard_read(pipe.into(), target, input_generation, tx, waker);
                }
            }
        }
    }

    fn apply_clipboard_reads(&mut self) -> Result<()> {
        while let Ok(read) = self.clipboard.clipboard_rx.try_recv() {
            if !clipboard_read_is_current(
                self.modal.input_modal_open(),
                self.input.input_generation,
                read.input_generation,
            ) {
                continue;
            }
            let Ok(bytes) = read.bytes else {
                continue;
            };
            let Ok(bytes) = safe_paste(&bytes) else {
                continue;
            };
            let bracketed = self
                .panes
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
        let Some(text) = self.panes.pane.selected_text.as_ref() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let payload = Arc::<[u8]>::from(text.clone());
        let mimes: Vec<_> = TEXT_MIMES.iter().map(|mime| (*mime).to_owned()).collect();
        if primary {
            if let (Some(manager), Some(device)) = (
                self.platform.primary_selection_manager.as_ref(),
                self.clipboard.primary_device.as_ref(),
            ) {
                let source = manager.create_selection_source(qh, mimes);
                source.set_selection(device, serial);
                self.clipboard.primary_sources.clear();
                self.clipboard.primary_sources.push((source, payload));
            }
        } else if let Some(device) = self.clipboard.data_device.as_ref() {
            let source = self
                .platform
                .data_device_manager
                .create_copy_paste_source(qh, mimes);
            source.set_selection(device, serial);
            self.clipboard.clipboard_sources.clear();
            self.clipboard.clipboard_sources.push((source, payload));
        }
    }

    fn open_hovered_url(&self) {
        let Some((_, _, url)) = &self.panes.pane.hovered_url else {
            return;
        };
        let _ = Command::new("xdg-open").arg(url).spawn();
    }

    fn settle_terminal_presses_for_picker(&mut self) {
        let pressed = std::mem::take(&mut self.input.pressed_buttons);
        for owner in pressed.into_values() {
            if let PressOwner::Application {
                code,
                sgr,
                modifiers,
                ..
            } = owner
                && let Some(position) = self.panes.pane.pointer_cell
                && let Some(report) =
                    mouse_report(MouseAction::Release(code), position, modifiers, sgr)
            {
                self.send_command(WindowCommand::Input(report));
            }
        }
        if self.panes.pane.selecting {
            let selection = self.panes.pane.selection.take();
            self.dirty_selection(selection);
            self.panes.pane.selecting = false;
            self.panes.pane.selected_text = None;
            self.panes.pane.history_selection_pin_blocked = false;
        }
        self.panes.pane.pointer_cell = None;
        self.panes.pane.hovered_url = None;
    }

    fn reconcile_terminal_focus_report(&mut self, modal_focus_changed: bool) {
        if !modal_focus_changed {
            return;
        }
        let Some(_) = reconciled_focus_report(
            self.panes.input_modes().focus_reporting,
            self.input.terminal_focus_reported,
            self.input.keyboard_focused,
        ) else {
            return;
        };
        Self::request_pane_focus_report(&mut self.panes.pane, self.input.keyboard_focused);
        self.input.terminal_focus_reported = self.input.keyboard_focused;
    }

    fn command_palette_available(&self) -> bool {
        self.tab_state.managed_tabs
            && self.tab_state.topology_commands.is_some()
            && self.modal.command_palette.is_none()
            && self.modal.dojo_prompt.is_none()
            && self.modal.tab_context_menu.is_none()
            && self.modal.session_picker.is_none()
            && self.modal.trusted_consent.is_none()
            && !self.modal.session_picker_requested
            && !self.modal.session_picker_reconcile_pending
            && !self.modal.command_palette_reconcile_pending
            && !self.tab_state.session_switch_pending
            && self.panes.pane.search.input.is_none()
            && self.input.divider_drag.is_none()
    }

    fn focused_cwd(&self) -> Result<std::path::PathBuf> {
        let focused = self
            .panes
            .focused_splint()
            .context("focused cwd requires a focused Splint")?;
        self.panes
            .layout
            .as_ref()
            .and_then(|layout| layout.find_splint(focused))
            .map(|splint| splint.cwd.clone())
            .context("focused Splint is absent from authoritative topology")
    }

    fn show_command_palette(&mut self) -> Result<()> {
        self.input.prefix_state.clear();
        anyhow::ensure!(
            self.command_palette_available(),
            "command palette is unavailable"
        );
        let splint_id = self
            .panes
            .focused_splint()
            .context("command palette requires a focused Splint")?;
        self.settle_terminal_presses_for_picker();
        self.input.input_generation = self.input.input_generation.saturating_add(1);
        self.input.ime_modal_barrier = self.input.ime.entered && self.input.text_input.is_some();
        if self.input.ime_modal_barrier {
            if let Some(text_input) = &self.input.text_input {
                text_input.disable();
            }
            self.commit_text_input();
        }
        self.clear_ime_preedit();
        self.modal.session_picker_wheel = WheelAccumulator::default();
        let multiple_tabs = self.tab_state.tabs.len() > 1;
        let active_dojo_id = self.tab_state.active_dojo_id();
        self.modal.command_palette = Some(CommandPaletteUi::new(CommandPaletteContext {
            lair_id: self.tab_state.active_identity.lair_id,
            focused_cwd: self.focused_cwd()?,
            dojo_id: active_dojo_id,
            dojo_name: self.tab_state.active_identity.dojo_name.clone(),
            pane_count: 1_usize.saturating_add(self.panes.inactive_panes.len()),
            splint_id,
            dojo_splints: std::iter::once(&self.panes.pane)
                .chain(self.panes.inactive_panes.iter())
                .filter_map(|pane| {
                    pane.snapshot
                        .as_ref()
                        .map(|snapshot| (snapshot.splint_id, snapshot.incarnation))
                })
                .collect(),
            other_dojo_ids: self
                .tab_state
                .tabs
                .iter()
                .filter_map(|tab| (tab.dojo_id != active_dojo_id).then_some(tab.dojo_id))
                .collect(),
            previous_dojo_id: multiple_tabs
                .then(|| self.tab_state.tabs.previous())
                .flatten(),
            next_dojo_id: multiple_tabs.then(|| self.tab_state.tabs.next()).flatten(),
            focus_left: self.directional_splint(FocusDirection::Left),
            focus_right: self.directional_splint(FocusDirection::Right),
            focus_up: self.directional_splint(FocusDirection::Up),
            focus_down: self.directional_splint(FocusDirection::Down),
            viewport_detached: !self.panes.pane.scrollback_viewport.is_live(),
            controller_active: self.panes.pane.controller_active,
            forced_control_transfer: self.input.forced_control_transfer,
            grant_ids: self
                .panes
                .pane
                .authority
                .grants
                .iter()
                .map(|(id, _)| *id)
                .collect(),
            pending_transfer_id: self.panes.pane.pending_control_transfer,
        }));
        self.modal.command_palette_layout = None;
        self.modal.command_palette_pressed = None;
        self.modal.command_palette_text_cache.clear();
        self.modal.command_palette_open_focus = Some(self.input.keyboard_focused);
        self.modal.command_palette_reconcile_pending = false;
        self.surface.window.set_title("Splinterm — Commands");
        self.presentation.full_redraw = true;
        Ok(())
    }

    fn close_command_palette(&mut self) -> bool {
        if self.modal.command_palette.take().is_none() {
            return false;
        }
        self.modal.command_palette_layout = None;
        self.modal.command_palette_pressed = None;
        self.modal.command_palette_text_cache.clear();
        self.modal.command_palette_reconcile_pending = true;
        self.presentation.full_redraw = true;
        true
    }

    fn reconcile_command_palette_close(&mut self, queue_handle: &QueueHandle<Self>) {
        if !std::mem::take(&mut self.modal.command_palette_reconcile_pending) {
            return;
        }
        self.update_window_title();
        match picker_ime_reconcile(
            self.input.ime_modal_barrier,
            self.input.keyboard_focused,
            self.input.ime.entered,
        ) {
            PickerImeReconcile::Renew => self.renew_text_input(queue_handle),
            PickerImeReconcile::Enable => self.enable_text_input(),
            PickerImeReconcile::None => {}
        }
        let modal_focus_changed = self
            .modal
            .command_palette_open_focus
            .take()
            .is_some_and(|focused| focused != self.input.keyboard_focused);
        self.reconcile_terminal_focus_report(modal_focus_changed);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the closed palette catalog routes each typed application action at one modal boundary"
    )]
    fn execute_command_palette(
        &mut self,
        command: BuiltInCommandId,
        queue_handle: &QueueHandle<Self>,
    ) {
        let Some(dispatch) = self
            .modal
            .command_palette
            .as_ref()
            .and_then(|palette| command_dispatch(command, &palette.context()))
        else {
            return;
        };
        let transitions_to_prompt = matches!(
            &dispatch,
            BuiltInCommandDispatch::Rename(_) | BuiltInCommandDispatch::ConfirmTermination(_)
        );
        self.close_command_palette();
        if !transitions_to_prompt {
            self.reconcile_command_palette_close(queue_handle);
        }
        let result = match dispatch {
            BuiltInCommandDispatch::Topology(mut command) => (|| -> Result<()> {
                let pending_started = if self.input.optimistic_remote_splits
                    && let WindowTopologyCommand::Split {
                        target,
                        axis,
                        pending,
                        ..
                    } = &mut command
                    && pending.is_none()
                {
                    match self.begin_pending_remote_split(*target, *axis)? {
                        Some(pending_id) => {
                            *pending = Some(pending_id);
                            true
                        }
                        None => return Ok(()),
                    }
                } else {
                    false
                };
                self.send_topology_command(command)?;
                if pending_started && self.surface.configured {
                    self.schedule_draw(queue_handle)?;
                }
                Ok(())
            })(),
            BuiltInCommandDispatch::Focus(splint_id) => {
                if self.focus_splint(splint_id) {
                    self.update_ime_cursor_rectangle();
                }
                Ok(())
            }
            BuiltInCommandDispatch::Zoom(action) => self
                .apply_font_zoom(
                    match action {
                        CommandZoomAction::Increase => FontZoomAction::Increase,
                        CommandZoomAction::Decrease => FontZoomAction::Decrease,
                        CommandZoomAction::Reset => FontZoomAction::Reset,
                    },
                    queue_handle,
                )
                .map(|_| ()),
            BuiltInCommandDispatch::History { target, action } => (|| -> Result<()> {
                anyhow::ensure!(
                    self.panes.focused_splint() == Some(target),
                    "captured history target is no longer focused"
                );
                match action {
                    CommandHistoryAction::Search => {
                        self.panes.pane.search.input = Some(String::new());
                        self.panes.pane.search.matches.clear();
                        self.panes.pane.search.next_cursor = None;
                        self.update_window_title();
                    }
                    CommandHistoryAction::PageUp | CommandHistoryAction::PageDown => {
                        let page = self
                            .panes
                            .pane
                            .snapshot
                            .as_ref()
                            .map_or(1, |snapshot| snapshot.rows.saturating_sub(1).max(1));
                        let direction = if action == CommandHistoryAction::PageUp {
                            MouseAction::WheelUp
                        } else {
                            MouseAction::WheelDown
                        };
                        if self.scroll_history(direction, page)? && self.surface.configured {
                            self.schedule_draw(queue_handle)?;
                        }
                    }
                    CommandHistoryAction::ReturnToLive => {
                        if self.scroll_history(MouseAction::WheelDown, usize::MAX)?
                            && self.surface.configured
                        {
                            self.schedule_draw(queue_handle)?;
                        }
                    }
                }
                Ok(())
            })(),
            BuiltInCommandDispatch::Control { target, action } => (|| -> Result<()> {
                anyhow::ensure!(
                    self.panes.focused_splint() == Some(target),
                    "captured control target is no longer focused"
                );
                match action {
                    CommandControlAction::Request => {
                        self.send_command(WindowCommand::RequestControlTransfer);
                    }
                    CommandControlAction::Release => {
                        Self::request_pane_control_release(&mut self.panes.pane);
                    }
                    CommandControlAction::Force if self.input.forced_control_transfer => {
                        self.send_command(WindowCommand::ForceControlTransfer);
                    }
                    CommandControlAction::Force => {}
                    CommandControlAction::Accept(transfer_id)
                    | CommandControlAction::Deny(transfer_id) => {
                        anyhow::ensure!(
                            self.panes.pane.pending_control_transfer == Some(transfer_id),
                            "captured control transfer is no longer pending"
                        );
                        self.panes.pane.pending_control_transfer = None;
                        self.send_command(WindowCommand::DecideControlTransfer {
                            transfer_id,
                            decision: if matches!(action, CommandControlAction::Accept(_)) {
                                ControlTransferDecision::Accept
                            } else {
                                ControlTransferDecision::Deny
                            },
                        });
                    }
                }
                self.update_window_title();
                Ok(())
            })(),
            BuiltInCommandDispatch::RevokeAccess { target, grant_ids } => (|| -> Result<()> {
                anyhow::ensure!(
                    self.panes.focused_splint() == Some(target),
                    "captured access target is no longer focused"
                );
                for id in &grant_ids {
                    self.send_command(WindowCommand::RevokeAccess(*id));
                }
                self.panes
                    .pane
                    .authority
                    .grants
                    .retain(|(id, _)| !grant_ids.contains(id));
                self.update_window_title();
                Ok(())
            })(),
            BuiltInCommandDispatch::Rename(target) => {
                self.show_dojo_prompt(DojoPromptUi::rename(
                    target.dojo_id,
                    target.name,
                    target.pane_count,
                    target.splints,
                ));
                Ok(())
            }
            BuiltInCommandDispatch::ConfirmTermination(target) => {
                self.show_dojo_prompt(DojoPromptUi::terminate(
                    target.dojo_id,
                    target.name,
                    target.pane_count,
                    target.splints,
                ));
                Ok(())
            }
            BuiltInCommandDispatch::RecentSessions => {
                self.modal.session_picker_requested = true;
                self.send_topology_command(WindowTopologyCommand::RequestSessionPicker)
                    .inspect_err(|_| self.modal.session_picker_requested = false)
            }
        };
        if result.is_err() {
            eprintln!("splinterm command palette action failed");
        }
    }

    fn refresh_command_palette(&mut self) {
        self.modal.command_palette_layout = None;
        self.presentation.full_redraw = true;
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite pointer coordinates are clamped to the logical surface"
    )]
    fn show_tab_context_menu(&mut self, dojo_id: DojoId, anchor: (f64, f64)) -> Result<()> {
        self.input.prefix_state.clear();
        anyhow::ensure!(
            self.tab_state.managed_tabs,
            "tab menu requires managed tabs"
        );
        anyhow::ensure!(
            self.tab_state.topology_commands.is_some(),
            "tab menu topology commands are unavailable"
        );
        anyhow::ensure!(
            self.modal.command_palette.is_none()
                && self.modal.dojo_prompt.is_none()
                && self.modal.session_picker.is_none()
                && self.modal.trusted_consent.is_none()
                && !self.modal.session_picker_requested
                && !self.modal.session_picker_reconcile_pending
                && !self.modal.command_palette_reconcile_pending
                && !self.tab_state.session_switch_pending
                && self.panes.pane.search.input.is_none()
                && self.panes.pane.pending_control_transfer.is_none()
                && self.input.divider_drag.is_none(),
            "tab menu is unavailable"
        );
        let identity = self
            .tab_state
            .tab_identity(dojo_id)
            .cloned()
            .context("tab menu target is unavailable")?;
        let active = self.tab_state.active_dojo_id() == dojo_id;
        let splints = if active {
            std::iter::once(&self.panes.pane)
                .chain(self.panes.inactive_panes.iter())
                .filter_map(|pane| {
                    pane.snapshot
                        .as_ref()
                        .map(|snapshot| (snapshot.splint_id, snapshot.incarnation))
                })
                .collect::<Vec<_>>()
        } else {
            self.tab_state
                .tabs
                .get(dojo_id)
                .and_then(|tab| tab.value.as_ref())
                .map_or_else(Vec::new, |view| {
                    std::iter::once(&view.pane)
                        .chain(view.inactive_panes.iter())
                        .filter_map(|pane| {
                            pane.snapshot
                                .as_ref()
                                .map(|snapshot| (snapshot.splint_id, snapshot.incarnation))
                        })
                        .collect()
                })
        };
        let pane_count = splints.len();
        let other_dojo_ids = self
            .tab_state
            .tabs
            .iter()
            .filter_map(|tab| (tab.dojo_id != dojo_id).then_some(tab.dojo_id))
            .collect();
        self.settle_terminal_presses_for_picker();
        self.input.input_generation = self.input.input_generation.saturating_add(1);
        self.input.ime_modal_barrier = self.input.ime.entered && self.input.text_input.is_some();
        if self.input.ime_modal_barrier {
            if let Some(text_input) = &self.input.text_input {
                text_input.disable();
            }
            self.commit_text_input();
        }
        self.clear_ime_preedit();
        let logical = |value: f64, maximum: u32| {
            if value.is_finite() && value > 0.0 {
                value.floor().min(f64::from(maximum)) as u32
            } else {
                0
            }
        };
        self.modal.tab_context_menu = Some(TabContextMenuUi::new(TabMenuContext {
            lair_id: identity.lair_id,
            focused_cwd: self.focused_cwd()?,
            dojo_id,
            dojo_name: identity.dojo_name,
            pane_count,
            splints,
            active,
            other_dojo_ids,
        }));
        self.modal.tab_context_menu_anchor = (
            logical(anchor.0, self.surface.logical_width),
            logical(anchor.1, self.surface.logical_height),
        );
        self.modal.tab_context_menu_layout = None;
        self.modal.tab_context_menu_pressed = None;
        self.modal.tab_context_menu_retarget = None;
        self.modal.tab_context_menu_text_cache.clear();
        self.modal.command_palette_open_focus = Some(self.input.keyboard_focused);
        self.modal.command_palette_reconcile_pending = false;
        self.presentation.full_redraw = true;
        Ok(())
    }

    fn close_tab_context_menu(&mut self) -> bool {
        if self.modal.tab_context_menu.take().is_none() {
            return false;
        }
        self.modal.tab_context_menu_layout = None;
        self.modal.tab_context_menu_pressed = None;
        self.modal.tab_context_menu_retarget = None;
        self.modal.tab_context_menu_text_cache.clear();
        self.modal.command_palette_reconcile_pending = true;
        self.presentation.full_redraw = true;
        true
    }

    fn execute_tab_context_menu(&mut self, action: TabMenuActionId) {
        let Some(dispatch) = self
            .modal
            .tab_context_menu
            .as_ref()
            .and_then(|menu| tab_menu_dispatch(action, &menu.context()))
        else {
            return;
        };
        self.close_tab_context_menu();
        let result = match dispatch {
            TabMenuDispatch::Topology(command) => self.send_topology_command(command),
            TabMenuDispatch::Rename(target) => {
                self.show_dojo_prompt(DojoPromptUi::rename(
                    target.dojo_id,
                    target.name,
                    target.pane_count,
                    target.splints,
                ));
                Ok(())
            }
            TabMenuDispatch::ConfirmTermination(target) => {
                self.show_dojo_prompt(DojoPromptUi::terminate(
                    target.dojo_id,
                    target.name,
                    target.pane_count,
                    target.splints,
                ));
                Ok(())
            }
        };
        if result.is_err() {
            eprintln!("splinterm tab menu action failed");
        }
    }

    fn show_current_dojo_prompt(&mut self, terminate: bool) {
        let dojo_id = self.tab_state.active_dojo_id();
        let splints = std::iter::once(&self.panes.pane)
            .chain(self.panes.inactive_panes.iter())
            .filter_map(|pane| {
                pane.snapshot
                    .as_ref()
                    .map(|snapshot| (snapshot.splint_id, snapshot.incarnation))
            })
            .collect::<Vec<_>>();
        let name = self.tab_state.active_identity.dojo_name.clone();
        let pane_count = splints.len();
        let prompt = if terminate {
            DojoPromptUi::terminate(dojo_id, name, pane_count, splints)
        } else {
            DojoPromptUi::rename(dojo_id, name, pane_count, splints)
        };
        self.show_dojo_prompt(prompt);
    }

    fn show_dojo_prompt(&mut self, prompt: DojoPromptUi) {
        self.input.prefix_state.clear();
        let title = match &prompt {
            DojoPromptUi::Rename(_) => "Splinterm — Rename Tab",
            DojoPromptUi::Terminate(_) => "Splinterm — Confirm Dojo Termination",
            DojoPromptUi::RenameLair(_) => "Splinterm — Rename Lair",
            DojoPromptUi::TerminateLair(_) => "Splinterm — Confirm Lair Termination",
        };
        self.modal.dojo_prompt = Some(prompt);
        self.modal.dojo_prompt_layout = None;
        self.modal.dojo_prompt_pressed = None;
        self.modal.dojo_prompt_text_cache.clear();
        self.modal.command_palette_reconcile_pending = false;
        self.surface.window.set_title(title);
        self.presentation.full_redraw = true;
    }

    fn close_dojo_prompt(&mut self) -> bool {
        if self.modal.dojo_prompt.take().is_none() {
            return false;
        }
        self.modal.dojo_prompt_layout = None;
        self.modal.dojo_prompt_pressed = None;
        self.modal.dojo_prompt_text_cache.clear();
        self.modal.command_palette_reconcile_pending = true;
        self.presentation.full_redraw = true;
        true
    }

    fn execute_dojo_prompt(&mut self) {
        let command = match self.modal.dojo_prompt.as_ref() {
            Some(DojoPromptUi::Rename(prompt)) => prompt.command(),
            Some(DojoPromptUi::Terminate(prompt)) => prompt.command(),
            Some(DojoPromptUi::RenameLair(prompt)) => prompt.command(),
            Some(DojoPromptUi::TerminateLair(prompt)) => prompt.command(),
            None => None,
        };
        if command.is_none()
            && self
                .modal
                .dojo_prompt
                .as_ref()
                .is_some_and(DojoPromptUi::is_rename)
        {
            return;
        }
        self.close_dojo_prompt();
        if let Some(command) = command
            && self.send_topology_command(command).is_err()
        {
            eprintln!("splinterm Dojo prompt action failed");
        }
    }

    fn refresh_dojo_prompt(&mut self) {
        self.modal.dojo_prompt_layout = None;
        self.presentation.full_redraw = true;
    }

    fn refresh_tab_context_menu(&mut self) {
        self.modal.tab_context_menu_layout = None;
        self.presentation.full_redraw = true;
    }

    fn show_embedded_session_picker(
        &mut self,
        items: Vec<SessionPickerItem>,
        targets: Vec<(LairId, DojoId)>,
        selector_kind: Option<SelectorKind>,
    ) -> Result<()> {
        self.input.prefix_state.clear();
        anyhow::ensure!(
            self.modal.session_picker.is_none(),
            "session picker is already open"
        );
        anyhow::ensure!(
            items.len() == targets.len(),
            "session picker targets differ"
        );
        self.settle_terminal_presses_for_picker();
        self.input.input_generation = self.input.input_generation.saturating_add(1);
        self.modal.session_picker_wheel = WheelAccumulator::default();
        self.input.ime_modal_barrier = self.input.ime.entered && self.input.text_input.is_some();
        if self.input.ime_modal_barrier {
            if let Some(text_input) = &self.input.text_input {
                text_input.disable();
            }
            self.commit_text_input();
        }
        self.clear_ime_preedit();
        self.modal.session_picker = Some(SessionPickerUi::inline(items));
        self.modal.selector_kind = selector_kind;
        self.modal.session_picker_targets = targets;
        self.modal.session_picker_layout = None;
        self.modal.session_picker_pressed = None;
        self.modal.session_picker_text_cache.clear();
        self.modal.session_picker_redraw = false;
        self.modal.session_picker_reconcile_pending = false;
        self.modal.session_picker_open_focus = Some(self.input.keyboard_focused);
        self.modal.session_picker_requested = false;
        self.surface.window.set_title(match selector_kind {
            Some(SelectorKind::Dojo) => "Splinterm — Dojos",
            Some(SelectorKind::LairDojo) => "Splinterm — Lairs and Dojos",
            None => "Splinterm — Recent Sessions",
        });
        self.presentation.full_redraw = true;
        Ok(())
    }

    fn close_inline_session_picker(&mut self) -> bool {
        if !self.modal.inline_picker_open() {
            return false;
        }
        self.modal.session_picker = None;
        self.modal.selector_kind = None;
        self.modal.session_picker_targets.clear();
        self.modal.session_picker_layout = None;
        self.modal.session_picker_pressed = None;
        self.modal.session_picker_text_cache.clear();
        self.modal.session_picker_wheel = WheelAccumulator::default();
        self.modal.session_picker_redraw = false;
        self.modal.session_picker_reconcile_pending = true;
        self.modal.session_picker_requested = false;
        true
    }

    fn send_topology_command(&mut self, command: WindowTopologyCommand) -> Result<()> {
        let commands = self.tab_state.topology_commands.clone();
        try_topology_command_with_rollback(commands.as_ref(), command, |target, pending| {
            self.rollback_pending_remote_split(target, pending)
        })
    }

    fn send_command(&mut self, command: WindowCommand) {
        if self.modal.input_modal_open() && matches!(command, WindowCommand::Input(_)) {
            return;
        }
        let Some(commands) = &self.panes.pane.commands else {
            return;
        };
        if let Err(error) = try_window_command(commands, command) {
            self.scheduling.fail(error);
        }
    }

    fn send_coalescible_input(&mut self, bytes: Vec<u8>) {
        if self.modal.input_modal_open() {
            return;
        }
        let Some(commands) = &self.panes.pane.commands else {
            return;
        };
        match commands.try_send(WindowCommand::Input(bytes)) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Closed(_)) => {
                self.scheduling
                    .fail(anyhow::anyhow!("Wayland command receiver disconnected"));
            }
        }
    }

    fn commit_text_input(&mut self) {
        if let Some(text_input) = &self.input.text_input {
            text_input.commit();
            self.input.ime.note_client_commit();
        }
    }

    fn focused_logical_rect(&self) -> Rect {
        self.panes
            .focused_splint()
            .and_then(|splint_id| {
                self.computed_pane_layout()
                    .ok()
                    .flatten()
                    .and_then(|layout| layout.rect(splint_id))
            })
            .unwrap_or_else(|| self.content_rect())
    }

    fn ime_cursor_rectangle(&self) -> Option<(i32, i32, i32, i32)> {
        let frame = self.panes.pane.snapshot_frame.as_ref()?;
        let rect = self.focused_logical_rect();
        let geometry = frame
            .window_geometry(rect.width, rect.height, self.surface.scale_120)
            .ok()?;
        offset_cursor_rectangle(frame.cursor_rectangle(&geometry)?, rect)
    }

    fn update_ime_cursor_rectangle(&mut self) {
        if !self.input.ime.entered || !self.input.ime.focused {
            return;
        }
        let Some(text_input) = &self.input.text_input else {
            return;
        };
        if let Some((x, y, width, height)) = self.ime_cursor_rectangle() {
            text_input.set_cursor_rectangle(x, y, width, height);
            self.commit_text_input();
        }
    }

    fn sync_graphical_focus(&self) {
        update_graphical_focus_watch(
            self.input.graphical_focus.as_ref(),
            self.input.keyboard_focused,
            self.panes.focused_splint(),
        );
    }

    fn set_ime_focus(&mut self, focused: bool) {
        self.input.keyboard_focused = focused;
        self.input.ime.focused = focused;
        self.sync_graphical_focus();
        if !focused && self.input.ime.entered {
            if let Some(text_input) = &self.input.text_input {
                text_input.disable();
            }
            self.commit_text_input();
            self.clear_ime_preedit();
        } else if focused && self.input.ime.entered && !self.modal.input_modal_open() {
            self.enable_text_input();
        }
    }

    fn renew_text_input(&mut self, queue_handle: &QueueHandle<Self>) {
        if let Some(text_input) = self.input.text_input.take() {
            text_input.destroy();
        }
        self.input.ime_generation = self.input.ime_generation.saturating_add(1);
        self.input.ime.entered = false;
        self.input.ime.clear_composition();
        self.input.ime_modal_barrier = false;
        if let (Some(manager), Some(seat)) = (
            &self.platform.text_input_manager,
            &self.input.text_input_seat,
        ) {
            self.input.text_input =
                Some(manager.get_text_input(seat, queue_handle, self.input.ime_generation));
        }
    }

    fn enable_text_input(&mut self) {
        let Some(text_input) = &self.input.text_input else {
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
        let had_preedit = self.input.ime.visible_preedit.is_some();
        self.input.ime.clear_composition();
        if had_preedit {
            let _ = self.refresh_ime_preedit();
        }
    }

    fn refresh_ime_preedit(&mut self) -> Result<()> {
        // The prepared frame represents the display viewport, which may be
        // detached from the live grid. Refreshing it from `self.panes.pane.snapshot`
        // corrupts an unrelated history row when focus loss clears preedit.
        let Some(mut render_snapshot) = self.panes.display_snapshot() else {
            return Ok(());
        };
        let Some(row) = apply_ime_preedit(
            &mut render_snapshot,
            self.input.ime.visible_preedit.as_deref(),
        ) else {
            return Ok(());
        };
        let Some(frame) = &mut self.panes.pane.snapshot_frame else {
            return Ok(());
        };
        let mut dirty = vec![false; render_snapshot.rows];
        dirty[row] = true;
        frame.refresh_rows_with_context(
            &render_snapshot,
            &dirty,
            &self.presentation.render_context,
        )?;
        self.panes
            .pane
            .raster_dirty_rows
            .resize(render_snapshot.rows, false);
        self.panes
            .pane
            .surface_dirty_rows
            .resize(render_snapshot.rows, false);
        self.panes.pane.raster_dirty_rows[row] = true;
        self.panes.pane.surface_dirty_rows[row] = true;
        Ok(())
    }

    fn rebuild_scaled_pane_frames(&mut self, scale_120: u32) -> Result<bool> {
        let mut rebuilt = rebuild_pane_scaled_frame_with_context(
            &mut self.panes.pane,
            scale_120,
            &self.presentation.render_context,
        )?;
        for pane in &mut self.panes.inactive_panes {
            rebuilt |= rebuild_pane_scaled_frame_with_context(
                pane,
                scale_120,
                &self.presentation.render_context,
            )?;
        }
        Ok(rebuilt)
    }

    fn apply_font_zoom(
        &mut self,
        action: FontZoomAction,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<bool> {
        let next = match action {
            FontZoomAction::Increase => self.presentation.font_zoom_steps.saturating_add(1),
            FontZoomAction::Decrease => self.presentation.font_zoom_steps.saturating_sub(1),
            FontZoomAction::Reset => 0,
        };
        if next == self.presentation.font_zoom_steps {
            return Ok(true);
        }
        let Some(raster_changed) = self
            .presentation
            .render_context
            .set_font_zoom_steps(next, self.surface.scale_120)?
        else {
            return Ok(true);
        };
        self.presentation.font_zoom_steps = next;
        self.presentation.renderer_generation =
            self.presentation.renderer_generation.saturating_add(1);
        self.modal.session_picker_text_cache.clear();
        self.tab_state.tab_label_cache.clear();
        self.tab_state.tab_close_text = None;
        self.tab_state.tab_new_text = None;
        self.modal.session_picker_layout = None;
        if !raster_changed {
            if self.modal.inline_picker_open() && self.surface.configured {
                self.modal.session_picker_redraw = true;
                self.schedule_draw(queue_handle)?;
            }
            return Ok(true);
        }
        self.rebuild_scaled_pane_frames(self.surface.scale_120)?;
        self.surface.buffers.clear();
        self.surface.backing.clear();
        self.panes.pane.pending_scrolls.clear();
        self.presentation.full_redraw = true;
        self.input.cursor_blink_visible = true;
        self.input.last_cursor_blink = Instant::now();
        self.refresh_ime_preedit()?;
        self.update_ime_cursor_rectangle();
        if self.surface.configured {
            if self.modal.inline_picker_open() {
                self.panes.restored_frontend_needs_resize = true;
            } else {
                self.emit_resize()?;
            }
            self.schedule_draw(queue_handle)?;
        }
        Ok(true)
    }

    fn decide_consent(&mut self, granted: bool) {
        if let Some(consent) = self.modal.trusted_consent.take() {
            let _ = consent.decision.send(granted);
        }
        self.scheduling.request_exit(ExitClass::CleanUserClose);
    }

    fn refresh_session_picker(&mut self) -> Result<()> {
        let Some(picker) = self.modal.session_picker.as_mut() else {
            return Ok(());
        };
        if picker.is_inline() {
            picker.clear_hovered();
            self.modal.session_picker_layout = None;
            self.modal.session_picker_pressed = None;
            self.modal.session_picker_redraw = true;
            return Ok(());
        }
        let mut snapshot = picker.snapshot();
        apply_theme(&mut snapshot, self.presentation.theme);
        self.panes.pane.snapshot = Some(snapshot);
        rebuild_pane_scaled_frame_with_context(
            &mut self.panes.pane,
            self.surface.scale_120,
            &self.presentation.render_context,
        )?;
        self.surface.buffers.clear();
        self.surface.backing.clear();
        self.presentation.full_redraw = true;
        Ok(())
    }

    fn move_session_picker(&mut self, delta: isize) {
        if let Some(picker) = self.modal.session_picker.as_mut() {
            picker.move_selection(delta);
        }
        if let Err(error) = self.refresh_session_picker() {
            self.scheduling.fail(error);
        }
    }

    fn handle_pane_divider_pointer(&mut self, event: &PointerEvent) -> Result<bool> {
        if let Some(mut drag) = self.input.divider_drag {
            match event.kind {
                PointerEventKind::Motion { .. } | PointerEventKind::Enter { .. } => {
                    let Some(ratio) = split_ratio_at(drag.split, event.position, 1) else {
                        return Ok(true);
                    };
                    if drag.ratio == Some(ratio) {
                        return Ok(true);
                    }
                    let Some(mut candidate) = self.panes.layout.clone() else {
                        self.input.divider_drag = None;
                        return Ok(false);
                    };
                    if !apply_preview_ratio(
                        &mut candidate,
                        drag.split.target,
                        drag.split.ancestor,
                        ratio,
                    ) {
                        self.input.divider_drag = None;
                        return Ok(false);
                    }
                    let previous = self.panes.layout.replace(candidate);
                    if self.computed_pane_layout().is_err() {
                        self.panes.layout = previous;
                        return Ok(true);
                    }
                    drag.ratio = Some(ratio);
                    self.input.divider_drag = Some(drag);
                    self.panes.pane.pointer_cell = None;
                    self.panes.pane.hovered_url = None;
                    self.presentation.full_redraw = true;
                    Ok(true)
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    self.input.divider_drag = None;
                    if let Some(ratio) = drag.ratio {
                        self.send_topology_command(WindowTopologyCommand::SetRatio {
                            dojo_id: self.tab_state.active_identity.dojo_id,
                            target: drag.split.target,
                            ancestor: drag.split.ancestor,
                            ratio,
                        })?;
                    }
                    Ok(true)
                }
                PointerEventKind::Leave { .. }
                | PointerEventKind::Press { .. }
                | PointerEventKind::Axis { .. }
                | PointerEventKind::Release { .. } => Ok(true),
            }
        } else if matches!(
            event.kind,
            PointerEventKind::Press {
                button: BTN_LEFT,
                ..
            }
        ) {
            let split = self
                .computed_pane_layout()?
                .and_then(|layout| layout.split_at(event.position, 6));
            if let Some(split) = split {
                self.input.divider_drag = Some(DividerDrag { split, ratio: None });
                self.panes.pane.pointer_cell = None;
                self.panes.pane.hovered_url = None;
                return Ok(true);
            }
            Ok(false)
        } else {
            Ok(false)
        }
    }

    fn handle_tab_strip_pointer(&mut self, event: &PointerEvent) -> Result<bool> {
        let Some(layout) = self.tab_state.tab_strip_layout.as_ref() else {
            return Ok(false);
        };
        let target = tab_strip_hit_test(layout, event.position);
        let in_strip = rect_contains(layout.rect, event.position);
        match event.kind {
            PointerEventKind::Press { button, .. }
                if in_strip && matches!(button, BTN_LEFT | BTN_MIDDLE | BTN_RIGHT) =>
            {
                let target = if button == BTN_RIGHT {
                    target
                        .and_then(tab_context_target)
                        .map(TabHitTarget::Activate)
                } else {
                    target
                };
                if let Some(target) = target {
                    self.tab_state.tab_strip_pressed = Some((button, target));
                }
                self.panes.pane.pointer_cell = None;
                self.panes.pane.hovered_url = None;
                Ok(true)
            }
            PointerEventKind::Release { button, .. } => {
                let Some((pressed_button, _)) = self.tab_state.tab_strip_pressed else {
                    return Ok(false);
                };
                if pressed_button != button {
                    return Ok(false);
                }
                let (_, pressed_target) = self
                    .tab_state
                    .tab_strip_pressed
                    .take()
                    .expect("matching chrome press remains present");
                let release_target = if button == BTN_RIGHT {
                    target
                        .and_then(tab_context_target)
                        .map(TabHitTarget::Activate)
                } else {
                    target
                };
                if Some(pressed_target) == release_target {
                    if let (BTN_RIGHT, TabHitTarget::Activate(dojo_id)) = (button, pressed_target) {
                        self.show_tab_context_menu(dojo_id, event.position)?;
                        return Ok(true);
                    }
                    let command = match (button, pressed_target) {
                        (BTN_LEFT, TabHitTarget::Activate(dojo_id)) => {
                            Some(WindowTopologyCommand::ActivateTab { dojo_id })
                        }
                        (BTN_LEFT | BTN_MIDDLE, TabHitTarget::Close(dojo_id))
                        | (BTN_MIDDLE, TabHitTarget::Activate(dojo_id)) => {
                            Some(WindowTopologyCommand::CloseTab { dojo_id })
                        }
                        (BTN_LEFT, TabHitTarget::New) => Some(WindowTopologyCommand::NewDojo {
                            lair_id: self.tab_state.active_identity.lair_id,
                            cwd: self.focused_cwd()?,
                        }),
                        _ => None,
                    };
                    if let Some(command) = command {
                        self.send_topology_command(command)?;
                    }
                }
                Ok(true)
            }
            PointerEventKind::Leave { .. } => {
                self.tab_state.tab_strip_pressed = None;
                Ok(in_strip)
            }
            _ => Ok(in_strip || self.tab_state.tab_strip_pressed.is_some()),
        }
    }

    fn handle_command_palette_pointer(
        &mut self,
        event: &PointerEvent,
        queue_handle: &QueueHandle<Self>,
    ) -> bool {
        let Some(committed_layout) = self.modal.command_palette_layout.as_ref() else {
            return false;
        };
        let target = command_palette_hit_test(committed_layout, event.position);
        let inside_panel = rect_contains(committed_layout.panel, event.position);
        let mut changed = false;
        let mut execute = None;
        match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                if let Some(palette) = self.modal.command_palette.as_mut() {
                    changed |= palette.update_hovered(target);
                }
            }
            PointerEventKind::Leave { .. } => {
                if let Some(palette) = self.modal.command_palette.as_mut() {
                    changed |= palette.update_hovered(None);
                }
            }
            PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                if inside_panel {
                    let next = target.filter(|command| {
                        self.modal
                            .command_palette
                            .as_ref()
                            .is_some_and(|palette| palette.command_enabled(*command))
                    });
                    if self.modal.command_palette_pressed != next {
                        self.modal.command_palette_pressed = next;
                        changed = true;
                    }
                } else {
                    changed |= self.close_command_palette();
                }
            }
            PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                let pressed = self.modal.command_palette_pressed.take();
                changed |= pressed.is_some();
                if pressed.is_some() && pressed == target {
                    execute = target;
                }
            }
            PointerEventKind::Axis { vertical, .. } if !vertical.is_none() => {
                if let Some((direction, count)) = self.modal.session_picker_wheel.push(
                    vertical.absolute,
                    vertical.discrete,
                    vertical.value120,
                    44,
                ) && let Some(palette) = self.modal.command_palette.as_mut()
                {
                    let count = isize::try_from(count).unwrap_or(isize::MAX);
                    changed |= palette.move_selection(match direction {
                        MouseAction::WheelUp => -count,
                        MouseAction::WheelDown => count,
                        _ => 0,
                    });
                    palette.update_hovered(None);
                }
            }
            _ => {}
        }
        if let Some(command) = execute {
            self.execute_command_palette(command, queue_handle);
            changed = true;
        }
        if changed {
            self.refresh_command_palette();
        }
        changed
    }

    fn handle_dojo_prompt_pointer(&mut self, event: &PointerEvent) -> bool {
        let Some(committed_layout) = self.modal.dojo_prompt_layout.as_ref() else {
            return false;
        };
        let target = dojo_prompt_hit_test(committed_layout, event.position);
        let inside_panel = rect_contains(committed_layout.panel, event.position);
        let mut changed = false;
        let mut execute = false;
        match event.kind {
            PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                if inside_panel {
                    if self.modal.dojo_prompt_pressed != target {
                        self.modal.dojo_prompt_pressed = target;
                        changed = true;
                    }
                    if let Some(decision) = target {
                        match self.modal.dojo_prompt.as_mut() {
                            Some(DojoPromptUi::Terminate(confirmation)) => {
                                changed |= confirmation.select(decision);
                            }
                            Some(DojoPromptUi::TerminateLair(confirmation)) => {
                                changed |= confirmation.select(decision);
                            }
                            Some(DojoPromptUi::Rename(_) | DojoPromptUi::RenameLair(_)) | None => {}
                        }
                    }
                } else {
                    changed |= self.close_dojo_prompt();
                }
            }
            PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                let pressed = self.modal.dojo_prompt_pressed.take();
                changed |= pressed.is_some();
                execute = pressed.is_some() && pressed == target;
            }
            _ => {}
        }
        if execute {
            self.execute_dojo_prompt();
            changed = true;
        }
        if changed && self.modal.dojo_prompt.is_some() {
            self.refresh_dojo_prompt();
        }
        changed
    }

    fn handle_tab_context_menu_pointer(&mut self, event: &PointerEvent) -> bool {
        let Some(committed_layout) = self.modal.tab_context_menu_layout.as_ref() else {
            return false;
        };
        let target = tab_context_menu_hit_test(committed_layout, event.position);
        let inside_panel = rect_contains(committed_layout.panel, event.position);
        let tab_target = self
            .tab_state
            .tab_strip_layout
            .as_ref()
            .and_then(|layout| tab_strip_hit_test(layout, event.position))
            .and_then(tab_context_target);
        let mut changed = false;
        let mut execute = None;
        let mut retarget = None;
        match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                if let Some(menu) = self.modal.tab_context_menu.as_mut() {
                    changed |= menu.update_hovered(target);
                }
            }
            PointerEventKind::Leave { .. } => {
                if let Some(menu) = self.modal.tab_context_menu.as_mut() {
                    changed |= menu.update_hovered(None);
                }
            }
            PointerEventKind::Press {
                button: BTN_RIGHT, ..
            } => match tab_menu_right_press(tab_target) {
                TabMenuRightPress::Retarget(dojo_id) => {
                    if self.modal.tab_context_menu_retarget != Some(dojo_id) {
                        self.modal.tab_context_menu_retarget = Some(dojo_id);
                        changed = true;
                    }
                }
                TabMenuRightPress::Dismiss => {
                    self.modal.tab_context_menu_retarget = None;
                    changed |= self.close_tab_context_menu();
                }
            },
            PointerEventKind::Release {
                button: BTN_RIGHT, ..
            } => {
                let pressed = self.modal.tab_context_menu_retarget.take();
                changed |= pressed.is_some();
                if pressed.is_some() && pressed == tab_target {
                    retarget = pressed;
                }
            }
            PointerEventKind::Press {
                button: BTN_LEFT, ..
            } => {
                if inside_panel {
                    let target = target.filter(|action| {
                        self.modal
                            .tab_context_menu
                            .as_ref()
                            .is_some_and(|menu| menu.action_enabled(*action))
                    });
                    if self.modal.tab_context_menu_pressed != target {
                        self.modal.tab_context_menu_pressed = target;
                        changed = true;
                    }
                } else {
                    changed |= self.close_tab_context_menu();
                }
            }
            PointerEventKind::Release {
                button: BTN_LEFT, ..
            } => {
                let pressed = self.modal.tab_context_menu_pressed.take();
                changed |= pressed.is_some();
                if pressed.is_some() && pressed == target {
                    execute = pressed;
                }
            }
            _ => {}
        }
        if let Some(dojo_id) = retarget {
            if self.show_tab_context_menu(dojo_id, event.position).is_err() {
                eprintln!("splinterm tab menu retarget failed");
            }
            changed = true;
        } else if let Some(action) = execute {
            self.execute_tab_context_menu(action);
            changed = true;
        }
        if changed {
            self.refresh_tab_context_menu();
        }
        changed
    }

    fn handle_session_picker_pointer(&mut self, event: &PointerEvent) -> bool {
        let target = self
            .modal
            .session_picker_layout
            .as_ref()
            .and_then(|layout| session_picker_hit_test(layout, event.position));
        let mut changed = false;
        let mut activate = None;
        match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                if let Some(picker) = self.modal.session_picker.as_mut() {
                    changed |= picker.update_hovered(target);
                }
            }
            PointerEventKind::Leave { .. } => {
                if let Some(picker) = self.modal.session_picker.as_mut() {
                    changed |= picker.clear_hovered();
                }
            }
            PointerEventKind::Press { button, .. } => {
                let next = (button == BTN_LEFT).then_some(target).flatten();
                if self.modal.session_picker_pressed != next {
                    self.modal.session_picker_pressed = next;
                    changed = true;
                }
            }
            PointerEventKind::Release { button, .. } => {
                if button == BTN_LEFT {
                    let pressed = self.modal.session_picker_pressed.take();
                    changed |= pressed.is_some();
                    activate = picker_release_activation(pressed, target);
                }
            }
            PointerEventKind::Axis { vertical, .. } => {
                if !vertical.is_none()
                    && let Some((direction, count)) = self.modal.session_picker_wheel.push(
                        vertical.absolute,
                        vertical.discrete,
                        vertical.value120,
                        44,
                    )
                {
                    let count = isize::try_from(count).unwrap_or(isize::MAX);
                    let delta = match direction {
                        MouseAction::WheelUp => -count,
                        MouseAction::WheelDown => count,
                        _ => 0,
                    };
                    self.move_session_picker(delta);
                    self.modal.session_picker_layout = None;
                    if let Some(picker) = self.modal.session_picker.as_mut() {
                        picker.clear_hovered();
                    }
                    changed = true;
                }
            }
        }
        if let Some(target) = activate {
            let decision = match target {
                PickerHitTarget::New => SessionPickerDecision::New,
                PickerHitTarget::Open(index) => SessionPickerDecision::Open(index),
            };
            self.decide_session_picker(decision);
            changed = true;
        }
        if changed && self.modal.inline_picker_open() {
            self.modal.session_picker_redraw = true;
        }
        changed
    }

    fn cancel_session_picker(&mut self) {
        if !self.close_inline_session_picker() {
            self.scheduling
                .request_exit(ExitClass::CleanSessionPickerDecision);
        }
    }

    fn decide_session_picker(&mut self, decision: SessionPickerDecision) {
        if self.modal.inline_picker_open() {
            let selector_kind = self.modal.selector_kind;
            let command = match decision {
                SessionPickerDecision::New => {
                    let cwd = match self.focused_cwd() {
                        Ok(cwd) => cwd,
                        Err(error) => {
                            self.scheduling.fail(error);
                            return;
                        }
                    };
                    match selector_kind {
                        Some(SelectorKind::Dojo) => WindowTopologyCommand::NewDojo {
                            lair_id: self.tab_state.active_identity.lair_id,
                            cwd,
                        },
                        Some(SelectorKind::LairDojo) | None => {
                            WindowTopologyCommand::NewLair { cwd }
                        }
                    }
                }
                SessionPickerDecision::Open(index) => {
                    let Some((lair_id, dojo_id)) =
                        self.modal.session_picker_targets.get(index).copied()
                    else {
                        self.scheduling
                            .fail(anyhow::anyhow!("session picker selected an invalid target"));
                        return;
                    };
                    WindowTopologyCommand::OpenDojo { lair_id, dojo_id }
                }
            };
            self.close_inline_session_picker();
            self.tab_state.session_switch_pending = true;
            if self.send_topology_command(command).is_err() {
                self.tab_state.session_switch_pending = false;
                eprintln!("splinterm session switch failed");
            }
            return;
        }
        if let Some(sender) = self
            .modal
            .session_picker
            .take()
            .and_then(SessionPickerUi::into_standalone_decision)
        {
            let _ = sender.send(decision);
        }
        self.scheduling
            .request_exit(ExitClass::CleanSessionPickerDecision);
    }

    fn update_window_title(&self) {
        if let Some(snapshot) = &self.panes.pane.snapshot {
            self.surface.window.set_title(window_title(
                self.presentation
                    .title_override
                    .as_deref()
                    .or(Some(&snapshot.title)),
                self.panes.pane.controller_active,
                &self.panes.pane.authority,
                self.panes.pane.pending_control_transfer.is_some(),
                Some(&self.panes.pane.search),
            ));
        }
    }

    fn reveal_pending_search_match(&mut self) {
        let Some(item) = self.panes.pane.search.pending_reveal.clone() else {
            return;
        };
        let Some(snapshot) = self.panes.pane.snapshot.as_ref() else {
            return;
        };
        if snapshot.active_screen != ActiveScreen::Normal {
            return;
        }
        if self
            .panes
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
            self.panes.pane.selection = Some(Selection {
                anchor: endpoint(item.start_column),
                end: endpoint(item.end_column.saturating_sub(1)),
            });
            self.panes.pane.search.pending_reveal = None;
            self.panes.pane.viewport_dirty = true;
            self.presentation.full_redraw = true;
        } else if !self.panes.pane.history_page_pending {
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
                self.panes.pane.history_page_pending = true;
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
        let Some(snapshot) = self.panes.pane.snapshot.as_ref() else {
            return;
        };
        let query = self.panes.pane.search.query.clone();
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
    fn handle_key(&mut self, event: &KeyEvent, queue_handle: &QueueHandle<Self>) {
        if self.modal.dojo_prompt.is_some() {
            let mut execute = false;
            let mut close = false;
            let mut changed = false;
            if let Some(prompt) = self.modal.dojo_prompt.as_mut() {
                match prompt {
                    DojoPromptUi::Rename(rename) => match event.keysym {
                        Keysym::Return | Keysym::KP_Enter => execute = true,
                        Keysym::Escape => close = true,
                        Keysym::BackSpace => changed = rename.backspace(),
                        _ if !self.input.modifiers.ctrl && !self.input.modifiers.alt => {
                            if let Some(text) = event.utf8.as_deref() {
                                changed = rename.append_text(text);
                            }
                        }
                        _ => {}
                    },
                    DojoPromptUi::Terminate(confirmation) => match event.keysym {
                        Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down | Keysym::Tab => {
                            changed = confirmation.move_selection();
                        }
                        Keysym::Return | Keysym::KP_Enter => execute = true,
                        Keysym::Escape => close = true,
                        _ => {}
                    },
                    DojoPromptUi::RenameLair(rename) => match event.keysym {
                        Keysym::Return | Keysym::KP_Enter => execute = true,
                        Keysym::Escape => close = true,
                        Keysym::BackSpace => changed = rename.backspace(),
                        _ if !self.input.modifiers.ctrl && !self.input.modifiers.alt => {
                            if let Some(text) = event.utf8.as_deref() {
                                changed = rename.append_text(text);
                            }
                        }
                        _ => {}
                    },
                    DojoPromptUi::TerminateLair(confirmation) => match event.keysym {
                        Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down | Keysym::Tab => {
                            changed = confirmation.move_selection();
                        }
                        Keysym::Return | Keysym::KP_Enter => execute = true,
                        Keysym::Escape => close = true,
                        _ => {}
                    },
                }
            }
            if execute {
                self.execute_dojo_prompt();
            } else if close {
                self.close_dojo_prompt();
            } else if changed {
                self.refresh_dojo_prompt();
            }
            return;
        }
        if self.modal.tab_context_menu.is_some() {
            let mut execute = None;
            let mut close = false;
            let mut changed = false;
            if let Some(menu) = self.modal.tab_context_menu.as_mut() {
                match event.keysym {
                    Keysym::Up => changed = menu.move_selection(-1),
                    Keysym::Down => changed = menu.move_selection(1),
                    Keysym::Return | Keysym::KP_Enter => execute = Some(menu.selected_action()),
                    Keysym::Escape => close = true,
                    _ => {}
                }
            }
            if let Some(action) = execute {
                self.execute_tab_context_menu(action);
            } else if close {
                self.close_tab_context_menu();
            } else if changed {
                self.refresh_tab_context_menu();
            }
            return;
        }
        if self.modal.command_palette.is_some() {
            let mut execute = None;
            let mut close = false;
            let mut changed = false;
            if let Some(palette) = self.modal.command_palette.as_mut() {
                match event.keysym {
                    Keysym::Up => changed = palette.move_selection(-1),
                    Keysym::Down => changed = palette.move_selection(1),
                    Keysym::Home => changed = palette.select_first(),
                    Keysym::End => changed = palette.select_last(),
                    Keysym::Return | Keysym::KP_Enter => {
                        execute = palette.selected_enabled_command();
                    }
                    Keysym::Escape => close = true,
                    Keysym::BackSpace => changed = palette.backspace(),
                    _ if !self.input.modifiers.ctrl && !self.input.modifiers.alt => {
                        if let Some(text) = event.utf8.as_deref() {
                            changed = palette.append_text(text);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(command) = execute {
                self.execute_command_palette(command, queue_handle);
            } else if close {
                self.close_command_palette();
            } else if changed {
                self.refresh_command_palette();
            }
            return;
        }
        if self.modal.session_picker.is_some() {
            match event.keysym {
                Keysym::Up | Keysym::k | Keysym::K => self.move_session_picker(-1),
                Keysym::Down | Keysym::j | Keysym::J => self.move_session_picker(1),
                Keysym::Home => {
                    if let Some(picker) = self.modal.session_picker.as_mut() {
                        picker.select_first();
                    }
                    if let Err(error) = self.refresh_session_picker() {
                        self.scheduling.fail(error);
                    }
                }
                Keysym::End => {
                    if let Some(picker) = self.modal.session_picker.as_mut() {
                        picker.select_last();
                    }
                    if let Err(error) = self.refresh_session_picker() {
                        self.scheduling.fail(error);
                    }
                }
                Keysym::Return | Keysym::KP_Enter => {
                    if let Some(decision) = self
                        .modal
                        .session_picker
                        .as_ref()
                        .map(SessionPickerUi::selected_decision)
                    {
                        self.decide_session_picker(decision);
                    }
                }
                Keysym::n | Keysym::N => {
                    self.decide_session_picker(SessionPickerDecision::New);
                }
                Keysym::Escape => self.cancel_session_picker(),
                _ => {}
            }
            return;
        }
        if self.modal.trusted_consent.is_some() {
            match event.keysym {
                Keysym::g | Keysym::G | Keysym::Return | Keysym::KP_Enter => {
                    self.decide_consent(true);
                }
                Keysym::d | Keysym::D | Keysym::Escape => self.decide_consent(false),
                _ => {}
            }
            return;
        }
        if self.panes.pane.search.input.is_some() {
            if self.input.modifiers.ctrl && matches!(event.keysym, Keysym::n | Keysym::N) {
                if self.panes.pane.search.selected + 1 < self.panes.pane.search.matches.len() {
                    self.panes.pane.search.selected += 1;
                    self.panes.pane.search.pending_reveal = self
                        .panes
                        .pane
                        .search
                        .matches
                        .get(self.panes.pane.search.selected)
                        .cloned();
                    self.reveal_pending_search_match();
                } else if let Some(cursor) = self.panes.pane.search.next_cursor.clone() {
                    self.submit_search(Some(cursor));
                }
                self.update_window_title();
                return;
            }
            if self.input.modifiers.ctrl && matches!(event.keysym, Keysym::p | Keysym::P) {
                self.panes.pane.search.selected = self.panes.pane.search.selected.saturating_sub(1);
                self.panes.pane.search.pending_reveal = self
                    .panes
                    .pane
                    .search
                    .matches
                    .get(self.panes.pane.search.selected)
                    .cloned();
                self.reveal_pending_search_match();
                self.update_window_title();
                return;
            }
            match event.keysym {
                Keysym::Escape => {
                    self.panes.pane.search = SearchUiState::default();
                    self.panes.pane.selection = None;
                }
                Keysym::Return | Keysym::KP_Enter => {
                    self.panes.pane.search.query = self
                        .panes
                        .pane
                        .search
                        .input
                        .as_deref()
                        .unwrap_or_default()
                        .to_owned();
                    self.submit_search(None);
                }
                Keysym::BackSpace => {
                    if let Some(input) = self.panes.pane.search.input.as_mut() {
                        input.pop();
                    }
                }
                _ if !self.input.modifiers.ctrl && !self.input.modifiers.alt => {
                    if let (Some(input), Some(text)) =
                        (self.panes.pane.search.input.as_mut(), event.utf8.as_deref())
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
        match shortcut_action_for(&self.input.keymap, event.keysym, self.input.modifiers) {
            Some(ActionId::RevokeAllAccess) => {
                let ids: Vec<_> = self
                    .panes
                    .pane
                    .authority
                    .grants
                    .iter()
                    .map(|(id, _)| *id)
                    .collect();
                for id in ids {
                    self.send_command(WindowCommand::RevokeAccess(id));
                }
                self.panes.pane.authority.grants.clear();
                if let Some(snapshot) = &self.panes.pane.snapshot {
                    self.surface.window.set_title(window_title(
                        self.presentation
                            .title_override
                            .as_deref()
                            .or(Some(&snapshot.title)),
                        self.panes.pane.controller_active,
                        &self.panes.pane.authority,
                        self.panes.pane.pending_control_transfer.is_some(),
                        Some(&self.panes.pane.search),
                    ));
                }
                return;
            }
            Some(ActionId::RequestControl) => {
                self.send_command(WindowCommand::RequestControlTransfer);
                return;
            }
            Some(ActionId::SearchScrollback) => {
                self.panes.pane.search.input = Some(String::new());
                self.panes.pane.search.matches.clear();
                self.panes.pane.search.next_cursor = None;
                self.update_window_title();
                return;
            }
            Some(ActionId::ForceControl) => {
                if self.input.forced_control_transfer {
                    self.send_command(WindowCommand::ForceControlTransfer);
                }
                return;
            }
            Some(ActionId::AcceptControlTransfer) => {
                if let Some(transfer_id) = self.panes.pane.pending_control_transfer.take() {
                    self.send_command(WindowCommand::DecideControlTransfer {
                        transfer_id,
                        decision: ControlTransferDecision::Accept,
                    });
                }
                return;
            }
            Some(ActionId::DenyControlTransfer) => {
                if let Some(transfer_id) = self.panes.pane.pending_control_transfer.take() {
                    self.send_command(WindowCommand::DecideControlTransfer {
                        transfer_id,
                        decision: ControlTransferDecision::Deny,
                    });
                }
                return;
            }
            Some(ActionId::ReleaseControl) => {
                Self::request_pane_control_release(&mut self.panes.pane);
                if let Some(snapshot) = &self.panes.pane.snapshot {
                    self.surface.window.set_title(window_title(
                        self.presentation
                            .title_override
                            .as_deref()
                            .or(Some(&snapshot.title)),
                        self.panes.pane.controller_active,
                        &self.panes.pane.authority,
                        self.panes.pane.pending_control_transfer.is_some(),
                        Some(&self.panes.pane.search),
                    ));
                }
                return;
            }
            _ => {}
        }
        if self.presentation.evidence_close_shortcuts
            && matches!(event.keysym, Keysym::Escape | Keysym::q | Keysym::Q)
        {
            self.scheduling.request_exit(ExitClass::CleanUserClose);
            return;
        }
        let utf8 = if self.input.ime.composing()
            && !self.input.modifiers.ctrl
            && !self.input.modifiers.alt
        {
            None
        } else {
            event.utf8.as_deref()
        };
        if let Some(bytes) = key_input(
            event.keysym,
            utf8,
            self.input.modifiers,
            self.panes.input_modes(),
        ) {
            self.send_command(WindowCommand::Input(bytes));
        }
    }

    fn emit_resize(&mut self) -> Result<()> {
        let layout = self.computed_pane_layout()?;
        let active_rect = self
            .panes
            .focused_splint()
            .and_then(|splint_id| layout.as_ref().and_then(|layout| layout.rect(splint_id)));
        let content = self.content_rect();
        let (active_width, active_height) = active_rect
            .map_or((content.width, content.height), |rect| {
                (rect.width, rect.height)
            });
        Self::emit_active_pane_resize(
            &mut self.panes.pane,
            active_width,
            active_height,
            self.surface.scale_120,
        )?;
        for pane in &mut self.panes.inactive_panes {
            let Some(splint_id) = pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id) else {
                continue;
            };
            let Some(rect) = layout.as_ref().and_then(|layout| layout.rect(splint_id)) else {
                continue;
            };
            Self::emit_inactive_pane_resize(pane, rect.width, rect.height, self.surface.scale_120)?;
        }
        Ok(())
    }

    fn emit_active_pane_resize(
        pane: &mut PaneView,
        logical_width: u32,
        logical_height: u32,
        scale_120: u32,
    ) -> Result<()> {
        Self::emit_pane_resize(pane, logical_width, logical_height, scale_120, true)
    }

    fn emit_inactive_pane_resize(
        pane: &mut PaneView,
        logical_width: u32,
        logical_height: u32,
        scale_120: u32,
    ) -> Result<()> {
        // Every visible pane must resize immediately. Deferring an uncontrolled
        // pane leaves its reflowed history stale until later input claims control.
        Self::emit_pane_resize(pane, logical_width, logical_height, scale_120, true)
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

    fn begin_pending_remote_split(
        &mut self,
        target: SplintId,
        axis: Axis,
    ) -> Result<Option<SplintId>> {
        if !remote_split_can_begin(&self.panes.pending_remote_splits) {
            return Ok(None);
        }
        let target_snapshot = std::iter::once(&self.panes.pane)
            .chain(self.panes.inactive_panes.iter())
            .find_map(|pane| {
                pane.snapshot
                    .as_ref()
                    .filter(|snapshot| snapshot.splint_id == target)
            })
            .context("pending split target has no frontend pane")?;
        let pending_id = SplintId::new();
        let mut pending_splint = Splint::shell(PathBuf::from("/"));
        pending_splint.id = pending_id;
        "Opening remote pane…".clone_into(&mut pending_splint.title);
        let mut layout = self
            .panes
            .layout
            .clone()
            .context("pending split requires a managed layout")?;
        anyhow::ensure!(
            insert_pending_split(&mut layout, target, pending_splint, axis),
            "pending split target is absent from the managed layout"
        );
        let mut snapshot =
            pending_remote_snapshot(pending_id, target_snapshot.columns, target_snapshot.rows);
        apply_theme(&mut snapshot, self.presentation.theme);
        let (update_sender, updates) = tokio::sync::mpsc::channel(1);
        let (commands, command_receiver) = tokio::sync::mpsc::channel(1);
        let options = WindowPaneOptions {
            snapshot,
            updates,
            commands,
            authority: AuthorityStatus::default(),
            controlled: false,
            image_sources: ImageContentLeaseSet::default(),
        };
        let mut pane = PaneView::from_inactive_options_with_context(
            options,
            self.surface.scale_120,
            &self.presentation.render_context,
        )?;
        // Placeholder channels never cross the trust boundary and must not emit
        // terminal input, resize, focus, or disconnect events.
        pane.updates = None;
        pane.commands = None;
        drop(update_sender);
        drop(command_receiver);
        self.panes.inactive_panes.push(pane);
        self.panes.layout = Some(layout);
        self.panes.pending_remote_splits.insert(target, pending_id);
        self.presentation.full_redraw = true;
        Ok(Some(pending_id))
    }

    fn rollback_pending_remote_split(&mut self, target: SplintId, pending: SplintId) -> Result<()> {
        anyhow::ensure!(
            self.panes.pending_remote_splits.get(&target) == Some(&pending),
            "remote split placeholder reservation changed before rollback"
        );
        let layout = self
            .panes
            .layout
            .take()
            .context("remote split placeholder has no managed layout")?;
        let (layout, removed) = remove_pending_split(layout, pending);
        anyhow::ensure!(
            removed,
            "remote split placeholder is absent from its layout"
        );
        let layout = layout.context("remote split placeholder consumed the complete layout")?;
        self.panes.inactive_panes.retain(|pane| {
            pane.snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.splint_id != pending)
        });
        self.panes.pending_remote_splits.remove(&target);
        self.presentation.frame_titles.remove(&pending);
        self.panes.layout = Some(layout);
        let _ = self.focus_splint(target);
        anyhow::ensure!(
            self.panes.focused_splint() == Some(target),
            "remote split rollback could not restore target focus"
        );
        self.presentation.full_redraw = true;
        Ok(())
    }

    fn apply_topology_replacement(
        &mut self,
        layout: LayoutNode,
        added: Vec<WindowPaneOptions>,
        removed: Vec<SplintId>,
        focused: Option<SplintId>,
    ) -> Result<()> {
        anyhow::ensure!(
            !self.modal.inline_picker_open(),
            "topology replacement arrived while the session picker was open"
        );
        let removed = removed.into_iter().collect::<HashSet<_>>();
        let mut prepared = Vec::with_capacity(added.len());
        for mut pane in added {
            apply_theme(&mut pane.snapshot, self.presentation.theme);
            prepared.push(PaneView::from_inactive_options_with_context(
                pane,
                self.surface.scale_120,
                &self.presentation.render_context,
            )?);
        }
        let prepared_ids = prepared
            .iter()
            .filter_map(|pane| pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id));
        let mut identities = std::iter::once(&self.panes.pane)
            .chain(self.panes.inactive_panes.iter())
            .filter_map(|pane| pane.snapshot.as_ref().map(|snapshot| snapshot.splint_id))
            .filter(|splint_id| !removed.contains(splint_id))
            .chain(prepared_ids)
            .collect::<HashSet<_>>();
        anyhow::ensure!(
            identities.len() == layout.splint_count()
                && identities
                    .iter()
                    .all(|splint_id| layout.find_splint(*splint_id).is_some()),
            "topology update pane identities do not match its layout"
        );
        let next_focus = focused
            .or_else(|| {
                self.panes
                    .focused_splint()
                    .filter(|splint_id| !removed.contains(splint_id))
            })
            .unwrap_or_else(|| layout.first_splint_id());
        anyhow::ensure!(
            identities.remove(&next_focus),
            "topology update focus is absent"
        );

        self.panes
            .pending_exited_splints
            .retain(|splint_id| !removed.contains(splint_id));
        self.panes.inactive_panes.extend(prepared);
        let _ = self.focus_splint(next_focus);
        anyhow::ensure!(
            self.panes.focused_splint() == Some(next_focus),
            "topology update focus could not be applied"
        );
        self.panes.inactive_panes.retain(|pane| {
            pane.snapshot
                .as_ref()
                .is_none_or(|snapshot| !removed.contains(&snapshot.splint_id))
        });
        self.panes
            .pending_remote_splits
            .retain(|_, pending| layout.find_splint(*pending).is_some());
        self.panes.layout = Some(layout);
        self.presentation.full_redraw = true;
        Ok(())
    }

    fn apply_targeted_topology_replacement(
        &mut self,
        dojo_id: DojoId,
        layout: LayoutNode,
        added: Vec<WindowPaneOptions>,
        removed: Vec<SplintId>,
        focused: Option<SplintId>,
    ) -> Result<bool> {
        if dojo_id == self.tab_state.active_dojo_id() {
            self.apply_topology_replacement(layout, added, removed, focused)?;
            return Ok(true);
        }
        let theme = self.presentation.theme;
        let scale_120 = self.surface.scale_120;
        self.tab_state
            .tabs
            .get_mut(dojo_id)
            .and_then(|tab| tab.value.as_mut())
            .context("updated Dojo tab has no hidden frontend")?
            .apply_topology(
                layout,
                added,
                removed,
                focused,
                theme,
                scale_120,
                &self.presentation.render_context,
            )?;
        Ok(false)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "bounded topology draining, tab reconciliation, and deferred picker updates remain one transaction"
    )]
    fn apply_topology_updates(&mut self) -> Result<(bool, Option<ThemeUpdate>)> {
        let mut pending = VecDeque::new();
        if let Some(updates) = &mut self.tab_state.topology_updates {
            let drained = drain_receiver(updates, &self.platform.update_waker);
            pending = drained.items.into();
            if drained.disconnected {
                self.tab_state.topology_updates = None;
            }
        }
        if !self.modal.inline_picker_open() && !self.tab_state.deferred_topology_updates.is_empty()
        {
            let mut deferred = VecDeque::from(std::mem::take(
                &mut self.tab_state.deferred_topology_updates,
            ));
            deferred.append(&mut pending);
            pending = deferred;
        }
        let mut changed = false;
        let mut next_theme = None;
        while let Some(update) = pending.pop_front() {
            let defer_for_picker = self.modal.inline_picker_open()
                && matches!(
                    &update,
                    WindowTopologyUpdate::Apply { .. }
                        | WindowTopologyUpdate::OpenTab { .. }
                        | WindowTopologyUpdate::ActivateTab { .. }
                        | WindowTopologyUpdate::RemoveTab { .. }
                        | WindowTopologyUpdate::UpdateIdentity(_)
                );
            if defer_for_picker {
                if self.tab_state.deferred_topology_updates.len() < MAX_DEFERRED_TOPOLOGY_UPDATES {
                    self.tab_state.deferred_topology_updates.push(update);
                    continue;
                }
                self.close_inline_session_picker();
                eprintln!("splinterm session picker: cancelled to apply queued topology updates");
                let mut replay = VecDeque::from(std::mem::take(
                    &mut self.tab_state.deferred_topology_updates,
                ));
                replay.push_back(update);
                replay.append(&mut pending);
                pending = replay;
                changed = true;
                continue;
            }
            match update {
                WindowTopologyUpdate::Apply {
                    topology_revision,
                    dojo_id,
                    layout,
                    added,
                    removed,
                    focused,
                } => {
                    if self.modal.inline_picker_open() {
                        let update = WindowTopologyUpdate::Apply {
                            topology_revision,
                            dojo_id,
                            layout,
                            added,
                            removed,
                            focused,
                        };
                        if self.tab_state.deferred_topology_updates.len()
                            < MAX_DEFERRED_TOPOLOGY_UPDATES
                        {
                            self.tab_state.deferred_topology_updates.push(update);
                        } else {
                            self.close_inline_session_picker();
                            eprintln!(
                                "splinterm session picker: cancelled to apply queued topology updates"
                            );
                            let mut replay = VecDeque::from(std::mem::take(
                                &mut self.tab_state.deferred_topology_updates,
                            ));
                            replay.push_back(update);
                            replay.append(&mut pending);
                            pending = replay;
                            changed = true;
                        }
                    } else {
                        changed |= self.apply_targeted_topology_replacement(
                            dojo_id, layout, added, removed, focused,
                        )?;
                        if let Some(diagnostics) = diagnostics() {
                            diagnostics
                                .update_topology(topology_revision, self.tab_state.tabs.len());
                        }
                    }
                }
                WindowTopologyUpdate::OpenTab {
                    identity,
                    layout,
                    panes,
                    focused,
                    acknowledged,
                } => {
                    let topology_revision = identity.topology_revision;
                    let dojo_id = identity.dojo_id;
                    let lair_id = identity.lair_id;
                    let view = match DojoTabView::from_open(
                        identity,
                        layout,
                        panes,
                        focused,
                        self.presentation.theme,
                        self.surface.scale_120,
                        &self.presentation.render_context,
                    ) {
                        Ok(view) => view,
                        Err(error) => {
                            self.tab_state.session_switch_pending = false;
                            let message = format!("{error:#}");
                            let _ = acknowledged.send(Err(message));
                            eprintln!("splinterm Dojo tab failed to open");
                            continue;
                        }
                    };
                    let previous_id = self.tab_state.active_dojo_id();
                    self.tab_state
                        .tabs
                        .open_or_activate(DojoTab::new(lair_id, dojo_id, Some(view)))
                        .map_err(anyhow::Error::from)?;
                    anyhow::ensure!(
                        self.tab_state.tabs.activate(previous_id),
                        "active Dojo tab disappeared while opening another"
                    );
                    self.tab_state.session_switch_pending = false;
                    changed |= self.activate_tab(dojo_id)?;
                    if let Some(diagnostics) = diagnostics() {
                        diagnostics.update_topology(topology_revision, self.tab_state.tabs.len());
                    }
                    let _ = acknowledged.send(Ok(()));
                }
                WindowTopologyUpdate::ActivateTab { dojo_id } => {
                    self.tab_state.session_switch_pending = false;
                    changed |= self.activate_tab(dojo_id)?;
                }
                WindowTopologyUpdate::RemoveTab {
                    dojo_id,
                    acknowledged,
                } => {
                    let was_active = self.tab_state.active_dojo_id() == dojo_id;
                    if was_active {
                        if let Some(selected) = self.tab_state.tabs.selection_after_close(dojo_id) {
                            changed |= self.activate_tab(selected)?;
                        } else {
                            for pane in std::iter::once(&mut self.panes.pane)
                                .chain(self.panes.inactive_panes.iter_mut())
                            {
                                Self::request_pane_control_release(pane);
                            }
                        }
                    }
                    let mut removed = self.tab_state.tabs.close(dojo_id);
                    let removal_action = final_tab_removal_action(
                        removed.is_some(),
                        self.tab_state.tabs.len(),
                        self.modal.session_picker_requested,
                    );
                    if let Some(removed) = removed.as_mut() {
                        if let Some(view) = removed.value.as_mut() {
                            Self::release_tab_controllers(view);
                        }
                        self.tab_state.tab_label_cache.clear();
                        self.presentation.full_redraw = true;
                        changed = true;
                    }
                    match removal_action {
                        FinalTabRemovalAction::Continue => {}
                        FinalTabRemovalAction::Exit => self
                            .scheduling
                            .request_exit(ExitClass::CleanFinalTabRemoved),
                        FinalTabRemovalAction::ExitAndHandoffPicker => {
                            self.modal.session_picker_requested = false;
                            if spawn_session_picker_handoff().is_err() {
                                eprintln!("splinterm session picker handoff failed");
                            }
                            self.scheduling
                                .request_exit(ExitClass::CleanFinalTabRemoved);
                        }
                    }
                    let _ = acknowledged.send(());
                }
                WindowTopologyUpdate::UpdateIdentity(identity) => {
                    if let Some(diagnostics) = diagnostics() {
                        diagnostics
                            .update_topology(identity.topology_revision, self.tab_state.tabs.len());
                    }
                    let dojo_id = identity.dojo_id;
                    if dojo_id == self.tab_state.active_dojo_id() {
                        self.tab_state.active_identity = identity;
                    } else if let Some(view) = self
                        .tab_state
                        .tabs
                        .get_mut(dojo_id)
                        .and_then(|tab| tab.value.as_mut())
                    {
                        view.identity = identity;
                    }
                    self.tab_state.tab_label_cache.clear();
                    self.presentation.full_redraw = true;
                    changed = true;
                }
                WindowTopologyUpdate::TabFailed {
                    dojo_id: _,
                    message: _,
                } => {
                    self.close_inline_session_picker();
                    self.modal.session_picker_requested = false;
                    self.tab_state.session_switch_pending = false;
                    eprintln!("splinterm Dojo tab failed");
                }
                WindowTopologyUpdate::ShowSessionPicker { items, targets } => {
                    if self.modal.session_picker_requested
                        && self.modal.session_picker.is_none()
                        && !self.tab_state.session_switch_pending
                        && !self.modal.session_picker_reconcile_pending
                        && !self.modal.command_palette_reconcile_pending
                    {
                        self.show_embedded_session_picker(items, targets, None)?;
                        changed = true;
                    }
                }
                WindowTopologyUpdate::ShowSelector {
                    kind,
                    items,
                    targets,
                } => {
                    if self.modal.session_picker_requested
                        && self.modal.session_picker.is_none()
                        && !self.tab_state.session_switch_pending
                        && !self.modal.session_picker_reconcile_pending
                        && !self.modal.command_palette_reconcile_pending
                    {
                        self.show_embedded_session_picker(items, targets, Some(kind))?;
                        changed = true;
                    }
                }
                WindowTopologyUpdate::ShowLairPrompt { kind, target } => {
                    self.modal.session_picker_requested = false;
                    let prompt = match kind {
                        LairPromptKind::Rename => DojoPromptUi::rename_lair(target),
                        LairPromptKind::Terminate => DojoPromptUi::terminate_lair(target),
                    };
                    self.show_dojo_prompt(prompt);
                    changed = true;
                }
                WindowTopologyUpdate::SessionPickerFailed(_) => {
                    self.close_inline_session_picker();
                    self.modal.session_picker_requested = false;
                    self.tab_state.session_switch_pending = false;
                    eprintln!("splinterm session picker failed");
                }
                WindowTopologyUpdate::Theme(update) => {
                    if self.modal.inline_picker_open() {
                        retain_newest_theme(&mut self.modal.deferred_picker_theme, update);
                    } else {
                        retain_newest_theme(&mut next_theme, update);
                    }
                }
                WindowTopologyUpdate::Closed => self
                    .scheduling
                    .request_exit(ExitClass::CleanFinalTabRemoved),
                WindowTopologyUpdate::Shutdown(_) => {
                    if let Some(diagnostics) = diagnostics() {
                        diagnostics.emit(
                            DiagnosticLevel::Error,
                            DiagnosticEventCode::TopologyFailure,
                            Some(DiagnosticErrorCode::TopologyManager),
                        );
                    }
                    self.scheduling
                        .request_exit(ExitClass::ErrorTopologyManager);
                    anyhow::bail!("topology manager stopped");
                }
            }
        }
        self.sync_graphical_focus();
        Ok((changed, next_theme))
    }

    fn apply_resolved_theme(&mut self, update: ThemeUpdate) -> ThemeUpdateImpact {
        if update.generation <= self.presentation.theme_generation {
            return ThemeUpdateImpact::default();
        }
        self.presentation.theme_generation = update.generation;
        let theme = update.theme;
        self.surface
            .background_effect_state
            .set_requested_blur(theme.background_blur);
        self.surface
            .background_effect_state
            .set_background_alpha(theme.background_alpha);
        let impact = classify_theme_update(self.presentation.theme, theme);
        self.presentation
            .render_context
            .set_background_alpha(theme.background_alpha);
        self.presentation.theme = theme;
        if !impact.rebuild_pixels {
            return impact;
        }
        if let Some(snapshot) = self.panes.pane.snapshot.as_mut() {
            apply_theme(snapshot, theme);
        }
        for pane in &mut self.panes.inactive_panes {
            if let Some(snapshot) = pane.snapshot.as_mut() {
                apply_theme(snapshot, theme);
            }
        }
        for tab in self.tab_state.tabs.iter_mut() {
            if let Some(view) = tab.value.as_mut() {
                let mut dirty = Vec::new();
                for pane in std::iter::once(&mut view.pane).chain(view.inactive_panes.iter_mut()) {
                    if let Some(snapshot) = pane.snapshot.as_mut() {
                        apply_theme(snapshot, theme);
                        dirty.push(snapshot.splint_id);
                    }
                }
                view.dirty_inactive_panes.extend(dirty);
            }
        }
        self.tab_state.tab_label_cache.clear();
        self.presentation.full_redraw = true;
        impact
    }

    fn apply_inactive_updates(&mut self) -> Result<InactiveUpdateDrain> {
        let mut changed = false;
        let mut next_theme = None;
        let mut dirty_frames = HashSet::new();
        let mut exited = Vec::new();
        for pane in &mut self.panes.inactive_panes {
            let mut pending = Vec::new();
            let mut disconnected = false;
            if let Some(updates) = &mut pane.updates {
                let drained = drain_receiver(updates, &self.platform.update_waker);
                pending = drained.items;
                disconnected = drained.disconnected;
            }
            if disconnected {
                pane.controller_active = false;
                pane.commands = None;
                pane.updates = None;
                changed = true;
            }
            let mut terminal_updates = Vec::with_capacity(pending.len());
            for update in pending {
                match update {
                    WindowUpdate::Theme(update) => {
                        retain_newest_theme(&mut next_theme, update);
                    }
                    WindowUpdate::Exited { splint_id } => {
                        anyhow::ensure!(
                            pane.snapshot
                                .as_ref()
                                .is_some_and(|snapshot| snapshot.splint_id == splint_id),
                            "inactive pane exit identity does not match its snapshot"
                        );
                        pane.controller_active = false;
                        pane.commands = None;
                        pane.updates = None;
                        exited.push(splint_id);
                        changed = true;
                    }
                    update => terminal_updates.push(update),
                }
            }
            let impact =
                apply_inactive_update_batch(pane, terminal_updates, self.presentation.theme)?;
            changed |= impact.visual_changed;
            if impact.frame_dirty
                && let Some(snapshot) = pane.snapshot.as_ref()
            {
                dirty_frames.insert(snapshot.splint_id);
            }
        }
        Ok(InactiveUpdateDrain {
            changed,
            theme: next_theme,
            dirty_frames,
            exited,
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "bounded update draining and semantic damage coalescing stay adjacent"
    )]
    fn apply_updates(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let (topology_changed, topology_theme) = self.apply_topology_updates()?;
        let theme = self.presentation.theme;
        let waker = self.platform.update_waker.clone();
        let topology_commands = self.tab_state.topology_commands.clone();
        for tab in self.tab_state.tabs.iter_mut() {
            if let Some(view) = tab.value.as_mut() {
                view.drain_hidden_updates(&waker, theme)?;
                for pane in std::iter::once(&mut view.pane).chain(view.inactive_panes.iter_mut()) {
                    Self::retry_pane_focus_report(pane);
                }
                Self::release_tab_controllers(view);
                if let Some(commands) = &topology_commands {
                    enqueue_pending_exited_splints(
                        tab.dojo_id,
                        &mut view.pending_exited_splints,
                        commands,
                    );
                }
            }
        }
        if self.scheduling.exit {
            return Ok(());
        }
        Self::retry_pane_focus_report(&mut self.panes.pane);
        Self::retry_pane_control_release(&mut self.panes.pane);
        let inline_picker_open = self.modal.inline_picker_open();
        let picker_reconcile_pending = self.modal.session_picker_reconcile_pending;
        let mut next_theme = (!inline_picker_open)
            .then(|| self.modal.deferred_picker_theme.take())
            .flatten();
        if let Some(update) = topology_theme {
            retain_newest_theme(&mut next_theme, update);
        }
        let restored_frontend = std::mem::take(&mut self.panes.restored_frontend_needs_resize);
        let final_resize_reconciliation = topology_changed || restored_frontend;
        if inline_picker_open && restored_frontend {
            self.panes.restored_frontend_needs_resize = true;
        } else if final_resize_reconciliation && !picker_reconcile_pending {
            self.emit_resize()?;
        }
        if restored_frontend && !inline_picker_open && !picker_reconcile_pending {
            self.update_ime_cursor_rectangle();
        }

        // Topology may promote a newly added pane to focus. Drain and arm the
        // post-topology focused receiver so it cannot fall back to the timed tick.
        let mut pending = Vec::new();
        let mut disconnected = false;
        if let Some(updates) = &mut self.panes.pane.updates {
            let drained = drain_receiver(updates, &self.platform.update_waker);
            pending = drained.items;
            disconnected = drained.disconnected;
        }
        if disconnected && !pane_stream_has_terminal_notice(&pending) {
            if let Some(diagnostics) = diagnostics() {
                diagnostics.emit(
                    DiagnosticLevel::Error,
                    DiagnosticEventCode::PaneStreamFailure,
                    Some(DiagnosticErrorCode::PaneStream),
                );
            }
            self.scheduling.request_exit(ExitClass::ErrorPaneStream);
            return Ok(());
        }
        let receiver_batch_size = pending.len();
        let inactive = self.apply_inactive_updates()?;
        self.panes.pending_exited_splints.extend(inactive.exited);
        if let Some(update) = inactive.theme {
            retain_newest_theme(&mut next_theme, update);
        }
        let mut visual_changed = topology_changed | inactive.changed;
        let mut focused_visual_changed = topology_changed;
        let mut title_changed = false;
        let mut full_frame_reload = false;
        let mut rebuild_all_inactive = false;
        let mut effect_desired_changed = false;
        if let Some(update) = next_theme {
            if inline_picker_open {
                retain_newest_theme(&mut self.modal.deferred_picker_theme, update);
            } else {
                let impact = self.apply_resolved_theme(update);
                visual_changed |= impact.rebuild_pixels;
                focused_visual_changed |= impact.rebuild_pixels;
                full_frame_reload |= impact.rebuild_pixels;
                rebuild_all_inactive |= impact.rebuild_pixels;
                effect_desired_changed |= impact.reconcile_effect;
            }
        }
        for update in pending {
            match update {
                WindowUpdate::Snapshot {
                    mut snapshot,
                    image_sources,
                    authoritative,
                } => {
                    self.panes.pane.history_page_pending = false;
                    snapshot
                        .validate()
                        .map_err(|error| anyhow::anyhow!(error.message))?;
                    apply_theme(&mut snapshot, self.presentation.theme);
                    let accept = match self.panes.pane.snapshot.as_ref() {
                        Some(current) => snapshot_replaces(current, &snapshot, authoritative)?,
                        None => true,
                    };
                    if accept {
                        let previous_generation = self
                            .panes
                            .pane
                            .snapshot
                            .as_ref()
                            .map_or(snapshot.history_generation, |current| {
                                current.history_generation
                            });
                        let previous_rows = self
                            .panes
                            .pane
                            .history_rows_needed_for_viewport_transition();
                        self.panes.pane.scrollback_viewport.observe_history_change(
                            previous_generation,
                            &previous_rows,
                            &snapshot,
                        );
                        self.invalidate_local_content_state();
                        self.panes.pane.snapshot = Some(snapshot);
                        self.panes.pane.image_sources = image_sources;
                        self.panes.pane.clear_trace_correlation();
                        self.presentation.full_redraw = true;
                        full_frame_reload = true;
                        visual_changed = true;
                        focused_visual_changed = true;
                        title_changed = true;
                    }
                }
                WindowUpdate::Update {
                    update,
                    image_sources,
                    trace,
                } => {
                    let apply_started = perf_trace_enabled().then(Instant::now);
                    let trace_base_revision = update.base_revision;
                    let trace_revision = update.revision;
                    let trace_rows = update.rows.len();
                    let trace_image_changed = image_sources.is_some();
                    let old_cursor_row = self.panes.pane.snapshot.as_ref().and_then(|snapshot| {
                        usize::try_from(snapshot.cursor_row)
                            .ok()
                            .filter(|row| *row < snapshot.rows)
                    });
                    let scrolls = update.scrolls.clone();
                    let history_changed = update.scrollback.is_some();
                    let current = self
                        .panes
                        .pane
                        .snapshot
                        .as_ref()
                        .context("terminal update arrived before initial snapshot")?;
                    let patched_rows = changed_terminal_patch_rows(&update, current);
                    let full_frame_reasons = terminal_update_full_frame_reasons(&update, current);
                    let mut full = full_frame_reasons != 0;
                    let content_changed = terminal_update_changes_visible_content(&update);
                    let cursor_changed = update.cursor.is_some() || update.input_modes.is_some();
                    title_changed |= update.title.is_some();
                    if content_changed {
                        self.dirty_selection(self.panes.pane.selection);
                    }
                    let previous_generation = self
                        .panes
                        .pane
                        .snapshot
                        .as_ref()
                        .map_or(1, |snapshot| snapshot.history_generation);
                    let previous_rows = self
                        .panes
                        .pane
                        .history_rows_needed_for_viewport_transition();
                    let trace_copied_history_bytes =
                        apply_started.map(|_| history_cache_bytes(&previous_rows));
                    let snapshot = self
                        .panes
                        .pane
                        .snapshot
                        .as_mut()
                        .context("terminal update arrived before initial snapshot")?;
                    apply_terminal_update(snapshot, update)?;
                    apply_theme(snapshot, self.presentation.theme);
                    self.panes.pane.scrollback_viewport.observe_history_change(
                        previous_generation,
                        &previous_rows,
                        snapshot,
                    );
                    if history_changed && !self.panes.pane.scrollback_viewport.is_live() {
                        full = true;
                    }
                    let trace_pane_role = self.panes.pane.trace_pane_role("focused");
                    if content_changed {
                        self.reconcile_selection_after_content_change();
                    }
                    let snapshot = self
                        .panes
                        .pane
                        .snapshot
                        .as_ref()
                        .context("updated terminal snapshot exists")?;
                    let rows = snapshot.rows;
                    if let Some(image_sources) = image_sources {
                        self.panes.pane.image_sources = image_sources;
                    }
                    self.panes.pane.prepare_dirty_rows.resize(rows, false);
                    self.panes.pane.raster_dirty_rows.resize(rows, false);
                    self.panes.pane.surface_dirty_rows.resize(rows, false);
                    if full {
                        self.presentation.full_redraw = true;
                        full_frame_reload = true;
                    } else {
                        if !scrolls.is_empty()
                            && let Some(row) = old_cursor_row
                        {
                            // The cursor is part of the persistent backing pixels. Seed its old
                            // row as dirty so scroll-copy damage follows the copied cursor block.
                            self.panes.pane.raster_dirty_rows[row] = true;
                        }
                        for scroll in &scrolls {
                            propagate_raster_damage_through_scroll(
                                &mut self.panes.pane.raster_dirty_rows,
                                scroll,
                            );
                            for row in scroll.start_row..scroll.end_row.min(rows) {
                                // Rebuilding the bounded semantic scroll region keeps prepared
                                // row geometry correct while pixel movement still uses scroll-copy.
                                self.panes.pane.prepare_dirty_rows[row] = true;
                                self.panes.pane.surface_dirty_rows[row] = true;
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
                                self.panes.pane.raster_dirty_rows[row] = true;
                            }
                        }
                        for row in patched_rows.into_iter().filter(|row| *row < rows) {
                            self.panes.pane.prepare_dirty_rows[row] = true;
                            self.panes.pane.raster_dirty_rows[row] = true;
                            self.panes.pane.surface_dirty_rows[row] = true;
                        }
                        if cursor_changed {
                            if let Some(row) = old_cursor_row {
                                self.panes.pane.raster_dirty_rows[row] = true;
                                self.panes.pane.surface_dirty_rows[row] = true;
                            }
                            if let Ok(row) = usize::try_from(snapshot.cursor_row)
                                && row < rows
                            {
                                self.panes.pane.raster_dirty_rows[row] = true;
                                self.panes.pane.surface_dirty_rows[row] = true;
                            }
                        }
                        self.panes.pane.pending_scrolls.extend(scrolls);
                    }
                    let update_visual_changed = terminal_update_has_visual_damage(
                        full,
                        cursor_changed,
                        &self.panes.pane.raster_dirty_rows,
                        &self.panes.pane.surface_dirty_rows,
                    );
                    visual_changed |= update_visual_changed;
                    focused_visual_changed |= update_visual_changed;
                    let trace_draw_expected = full || update_visual_changed || trace_image_changed;
                    if let Some(started) = apply_started {
                        emit_perf_trace(
                            "splinterm",
                            "client_apply",
                            PerfTraceEvent {
                                splint_id: Some(snapshot.splint_id),
                                incarnation: Some(snapshot.incarnation),
                                base_revision: Some(trace_base_revision),
                                revision: Some(trace_revision),
                                subscription_id: trace.map(|value| value.subscription_id),
                                transaction_sequence: trace.map(|value| value.transaction_sequence),
                                duration_ns: Some(
                                    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                                ),
                                rows: Some(u64::try_from(trace_rows).unwrap_or(u64::MAX)),
                                pane_role: Some(trace_pane_role),
                                pane_count: Some(
                                    u64::try_from(
                                        self.panes.inactive_panes.len().saturating_add(1),
                                    )
                                    .unwrap_or(u64::MAX),
                                ),
                                cached_history_rows: Some(
                                    u64::try_from(snapshot.scrollback_rows.len())
                                        .unwrap_or(u64::MAX),
                                ),
                                cached_history_bytes: Some(
                                    u64::try_from(history_cache_bytes(&snapshot.scrollback_rows))
                                        .unwrap_or(u64::MAX),
                                ),
                                copied_history_rows: Some(
                                    u64::try_from(previous_rows.len()).unwrap_or(u64::MAX),
                                ),
                                copied_history_bytes: trace_copied_history_bytes
                                    .map(|value| u64::try_from(value).unwrap_or(u64::MAX)),
                                receiver_batch_size: Some(
                                    u64::try_from(receiver_batch_size).unwrap_or(u64::MAX),
                                ),
                                // Bitset: columns, rows, palette, defaults, screen, images,
                                // or an image-bearing scroll in ascending bit order.
                                count: Some(full_frame_reasons),
                                full_reload: Some(full),
                                ..PerfTraceEvent::default()
                            },
                        );
                    }
                    if trace_draw_expected {
                        self.panes
                            .pane
                            .retain_trace_correlation(trace, trace_pane_role);
                    }
                }
                WindowUpdate::ScrollbackPages(pages) => {
                    self.panes.pane.history_page_pending = false;
                    let pinned_selection_rows = self
                        .panes
                        .pane
                        .selection
                        .map(|selection| [selection.anchor.row_id, selection.end.row_id]);
                    let snapshot = self
                        .panes
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
                            self.panes.pane.history_selection_pin_blocked = true;
                        }
                    }
                }
                WindowUpdate::ScrollbackResyncRequired => {
                    self.panes.pane.history_page_pending = false;
                    self.invalidate_local_content_state();
                    visual_changed = true;
                    focused_visual_changed = true;
                }
                WindowUpdate::Authority(authority) => {
                    self.panes.pane.authority = authority;
                    title_changed = true;
                    visual_changed = true;
                    focused_visual_changed = true;
                    self.presentation.full_redraw = true;
                }
                WindowUpdate::Control(active) => {
                    self.panes.pane.controller_active = active;
                    title_changed = true;
                    visual_changed = true;
                    focused_visual_changed = true;
                    self.presentation.full_redraw = true;
                }
                WindowUpdate::ControlTransferRequested(transfer_id) => {
                    self.panes.pane.pending_control_transfer = Some(transfer_id);
                    title_changed = true;
                }
                WindowUpdate::ControlTransferResolved(_) => {
                    self.panes.pane.pending_control_transfer = None;
                    title_changed = true;
                }
                WindowUpdate::SearchResults(page) => {
                    self.panes.pane.search.matches = page.matches;
                    self.panes.pane.search.selected = 0;
                    self.panes.pane.search.next_cursor = page.next_cursor;
                    self.panes.pane.search.pending_reveal =
                        self.panes.pane.search.matches.first().cloned();
                    title_changed = true;
                    visual_changed = true;
                    focused_visual_changed = true;
                    self.presentation.full_redraw = true;
                }
                WindowUpdate::SearchResyncRequired => {
                    self.panes.pane.search.matches.clear();
                    self.panes.pane.search.next_cursor = None;
                    self.panes.pane.search.pending_reveal = None;
                    title_changed = true;
                    visual_changed = true;
                    focused_visual_changed = true;
                    self.presentation.full_redraw = true;
                }
                WindowUpdate::Theme(update) => {
                    if self.modal.inline_picker_open() {
                        retain_newest_theme(&mut self.modal.deferred_picker_theme, update);
                    } else {
                        let impact = self.apply_resolved_theme(update);
                        visual_changed |= impact.rebuild_pixels;
                        focused_visual_changed |= impact.rebuild_pixels;
                        full_frame_reload |= impact.rebuild_pixels;
                        rebuild_all_inactive |= impact.rebuild_pixels;
                        effect_desired_changed |= impact.reconcile_effect;
                    }
                }
                WindowUpdate::Exited { splint_id } => {
                    anyhow::ensure!(
                        self.panes.focused_splint() == Some(splint_id),
                        "focused pane exit identity does not match its snapshot"
                    );
                    if self.tab_state.topology_commands.is_some() {
                        self.panes.pending_exited_splints.insert(splint_id);
                        self.panes.pane.controller_active = false;
                        self.panes.pane.commands = None;
                        self.panes.pane.updates = None;
                        title_changed = true;
                        visual_changed = true;
                        focused_visual_changed = true;
                        self.presentation.full_redraw = true;
                    } else {
                        self.scheduling
                            .request_exit(ExitClass::CleanFinalTabRemoved);
                        return Ok(());
                    }
                }
                WindowUpdate::Shutdown => {
                    if self.panes.layout.is_some() {
                        self.panes.pane.controller_active = false;
                        self.panes.pane.commands = None;
                        self.panes.pane.updates = None;
                        title_changed = true;
                        visual_changed = true;
                        focused_visual_changed = true;
                        self.presentation.full_redraw = true;
                    } else {
                        self.scheduling.request_exit(ExitClass::ErrorPaneStream);
                        return Ok(());
                    }
                }
            }
        }
        if !self.panes.pending_exited_splints.is_empty() {
            let topology_queue_open =
                self.tab_state
                    .topology_commands
                    .as_ref()
                    .is_some_and(|commands| {
                        enqueue_pending_exited_splints(
                            self.tab_state.active_identity.dojo_id,
                            &mut self.panes.pending_exited_splints,
                            commands,
                        )
                    });
            if !topology_queue_open {
                self.tab_state.topology_commands = None;
            }
        }
        let rebuilt_inactive = rebuild_inactive_frames_with_context(
            &mut self.panes.inactive_panes,
            &inactive.dirty_frames,
            rebuild_all_inactive,
            self.surface.scale_120,
            &self.presentation.render_context,
        )?;
        visual_changed |= rebuilt_inactive > 0;
        if rebuilt_inactive > 0 {
            self.panes
                .dirty_inactive_panes
                .extend(inactive.dirty_frames.iter().copied());
        }
        if rebuild_all_inactive {
            self.presentation.full_redraw = true;
        } else if rebuilt_inactive == 0 && inactive.changed {
            // Rare inactive metadata changes do not identify a terminal frame
            // region, so retain the conservative whole-window fallback.
            self.presentation.full_redraw = true;
        }
        if self.panes.pane.search.pending_reveal.is_some() {
            self.reveal_pending_search_match();
            visual_changed = true;
            focused_visual_changed = true;
        }
        if self
            .surface
            .background_effect_reconcile_schedule
            .queue_update(
                effect_desired_changed,
                visual_changed,
                self.surface.configured,
            )
        {
            self.reconcile_background_effect(queue_handle, BackgroundEffectCommitMode::Immediate)?;
        }
        if restart_cursor_blink(
            focused_visual_changed,
            &mut self.input.cursor_blink_visible,
            &mut self.input.last_cursor_blink,
        ) {
            let prepare_started = perf_trace_enabled().then(Instant::now);
            let trace_dirty_rows = self
                .panes
                .pane
                .prepare_dirty_rows
                .iter()
                .filter(|dirty| **dirty)
                .count();
            let live_viewport = self.panes.pane.scrollback_viewport.is_live();
            let display_owned = if live_viewport {
                None
            } else {
                Some(
                    self.panes
                        .display_snapshot()
                        .context("updated snapshot exists")?,
                )
            };
            let display = display_owned
                .as_ref()
                .or(self.panes.pane.snapshot.as_ref())
                .context("updated snapshot exists")?;
            if full_frame_reload || self.panes.pane.snapshot_frame.is_none() || !live_viewport {
                self.panes.pane.snapshot_frame =
                    Some(SnapshotFrame::load_scaled_with_sources_and_context(
                        display,
                        self.surface.scale_120,
                        Some(&self.panes.pane.image_sources),
                        &self.presentation.render_context,
                    )?);
            } else if let Some(frame) = &mut self.panes.pane.snapshot_frame {
                frame.refresh_rows_with_context(
                    display,
                    &self.panes.pane.prepare_dirty_rows,
                    &self.presentation.render_context,
                )?;
                frame.refresh_images(display, &self.panes.pane.image_sources)?;
                frame.refresh_cursor(display);
            }
            self.panes.pane.rendered_viewport_offset =
                self.panes.pane.scrollback_viewport.offset_from_bottom();
            self.panes.pane.viewport_dirty = false;
            self.panes.pane.prepare_dirty_rows.fill(false);
            if let Some(started) = prepare_started {
                emit_perf_trace(
                    "splinterm",
                    "frame_prepare",
                    PerfTraceEvent {
                        splint_id: Some(display.splint_id),
                        incarnation: Some(display.incarnation),
                        base_revision: self
                            .panes
                            .pane
                            .trace_correlation
                            .map(|value| value.base_revision),
                        revision: Some(
                            self.panes
                                .pane
                                .trace_correlation
                                .map_or(display.revision, |value| value.revision),
                        ),
                        subscription_id: self
                            .panes
                            .pane
                            .trace_correlation
                            .map(|value| value.subscription_id),
                        transaction_sequence: self
                            .panes
                            .pane
                            .trace_correlation
                            .map(|value| value.transaction_sequence),
                        duration_ns: Some(
                            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                        ),
                        rows: Some(u64::try_from(display.rows).unwrap_or(u64::MAX)),
                        cells: Some(
                            u64::try_from(display.rows.saturating_mul(display.columns))
                                .unwrap_or(u64::MAX),
                        ),
                        count: Some(u64::try_from(trace_dirty_rows).unwrap_or(u64::MAX)),
                        pane_role: Some(self.panes.pane.trace_pane_role),
                        pane_count: Some(
                            u64::try_from(self.panes.inactive_panes.len().saturating_add(1))
                                .unwrap_or(u64::MAX),
                        ),
                        columns: Some(u64::try_from(display.columns).unwrap_or(u64::MAX)),
                        cached_history_rows: Some(
                            u64::try_from(display.scrollback_rows.len()).unwrap_or(u64::MAX),
                        ),
                        dirty_rows: Some(u64::try_from(trace_dirty_rows).unwrap_or(u64::MAX)),
                        prepared_rows: Some(u64::try_from(display.rows).unwrap_or(u64::MAX)),
                        prepared_cells: Some(
                            u64::try_from(display.rows.saturating_mul(display.columns))
                                .unwrap_or(u64::MAX),
                        ),
                        superseded_revisions: Some(self.panes.pane.trace_superseded_revisions),
                        full_reload: Some(full_frame_reload),
                        ..PerfTraceEvent::default()
                    },
                );
            }
            self.refresh_ime_preedit()?;
            self.update_ime_cursor_rectangle();
            if !self.modal.inline_picker_open()
                && !self.modal.session_picker_reconcile_pending
                && terminal_resize_allowed(
                    TerminalResizeCause::SnapshotAvailable,
                    self.panes.pane.last_resize.is_some(),
                )
            {
                self.emit_resize()?;
            }
        }
        if visual_changed && self.surface.configured && !self.modal.session_picker_reconcile_pending
        {
            self.schedule_terminal_draw(queue_handle)?;
        }
        if title_changed
            && !self.modal.input_modal_open()
            && !self.modal.session_picker_reconcile_pending
        {
            let snapshot = self
                .panes
                .pane
                .snapshot
                .as_ref()
                .context("updated snapshot exists")?;
            self.surface.window.set_title(window_title(
                self.presentation
                    .title_override
                    .as_deref()
                    .or(Some(&snapshot.title)),
                self.panes.pane.controller_active,
                &self.panes.pane.authority,
                self.panes.pane.pending_control_transfer.is_some(),
                Some(&self.panes.pane.search),
            ));
        }
        if picker_reconcile_pending {
            self.modal.session_picker_reconcile_pending = false;
            if final_resize_reconciliation {
                self.emit_resize()?;
                self.update_ime_cursor_rectangle();
            }
            self.update_window_title();
            match picker_ime_reconcile(
                self.input.ime_modal_barrier,
                self.input.keyboard_focused,
                self.input.ime.entered,
            ) {
                PickerImeReconcile::Renew => self.renew_text_input(queue_handle),
                PickerImeReconcile::Enable => self.enable_text_input(),
                PickerImeReconcile::None => {}
            }
            let modal_focus_changed = self
                .modal
                .session_picker_open_focus
                .take()
                .is_some_and(|focused| focused != self.input.keyboard_focused);
            self.reconcile_terminal_focus_report(modal_focus_changed);
            self.presentation.full_redraw = true;
            if self.surface.configured {
                self.schedule_draw(queue_handle)?;
            }
        }
        Ok(())
    }

    fn refresh_output_dpi(
        &mut self,
        output: &wl_output::WlOutput,
        queue_handle: &QueueHandle<Self>,
    ) -> Result<()> {
        let observation = self.platform.output_dpi_observation(output);
        let raster_changed = self
            .presentation
            .render_context
            .update_output_dpi(observation, self.surface.scale_120)?;
        self.presentation.renderer_generation =
            self.presentation.renderer_generation.saturating_add(1);
        self.modal.session_picker_text_cache.clear();
        self.tab_state.tab_label_cache.clear();
        self.tab_state.tab_close_text = None;
        self.tab_state.tab_new_text = None;
        self.modal.session_picker_layout = None;
        if !raster_changed {
            if self.modal.inline_picker_open() && self.surface.configured {
                self.modal.session_picker_redraw = true;
                self.schedule_draw(queue_handle)?;
            }
            return Ok(());
        }
        if self.modal.inline_picker_open() {
            self.panes.restored_frontend_needs_resize = true;
        }
        self.rebuild_scaled_pane_frames(self.surface.scale_120)?;
        self.surface.buffers.clear();
        self.surface.backing.clear();
        self.presentation.full_redraw = true;
        self.refresh_ime_preedit()?;
        debug_assert!(!terminal_resize_allowed(
            TerminalResizeCause::OutputDpiChanged,
            self.panes.pane.last_resize.is_some(),
        ));
        self.update_ime_cursor_rectangle();
        if self.surface.configured {
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
        if !(MIN_SCALE_120..=MAX_SCALE_120).contains(&scale_120)
            || scale_120 == self.surface.scale_120
        {
            return Ok(());
        }
        if self.surface.viewport.is_none() && !scale_120.is_multiple_of(SCALE_DENOMINATOR) {
            return Ok(());
        }
        if self.surface.viewport.is_none() {
            self.surface
                .window
                .set_buffer_scale(scale_120 / SCALE_DENOMINATOR)
                .map_err(|_| anyhow::anyhow!("compositor rejected integer buffer scale"))?;
        } else {
            self.surface
                .window
                .set_buffer_scale(1)
                .map_err(|_| anyhow::anyhow!("compositor rejected unit buffer scale"))?;
        }
        if !self.rebuild_scaled_pane_frames(scale_120)? {
            self.presentation.text_row =
                Some(TextRow::load(scale_120.div_ceil(SCALE_DENOMINATOR))?);
        }
        self.surface.scale_120 = scale_120;
        self.presentation.renderer_generation =
            self.presentation.renderer_generation.saturating_add(1);
        self.modal.session_picker_text_cache.clear();
        self.tab_state.tab_label_cache.clear();
        self.tab_state.tab_close_text = None;
        self.tab_state.tab_new_text = None;
        self.modal.session_picker_layout = None;
        if self.modal.inline_picker_open() {
            self.panes.restored_frontend_needs_resize = true;
        }
        self.surface.buffers.clear();
        self.surface.backing.clear();
        self.presentation.full_redraw = true;
        self.refresh_ime_preedit()?;
        debug_assert!(!terminal_resize_allowed(
            TerminalResizeCause::CompositorScaleChanged,
            self.panes.pane.last_resize.is_some(),
        ));
        self.update_ime_cursor_rectangle();
        if self.surface.configured {
            self.schedule_draw(queue_handle)?;
        }
        Ok(())
    }

    fn cursor_is_blinking(&self) -> bool {
        !self.modal.inline_picker_open()
            && self.presentation.cursor_blink
            && self.panes.pane.snapshot.as_ref().is_some_and(|snapshot| {
                cursor_blink_enabled(
                    self.input.reduced_motion,
                    self.input.keyboard_focused,
                    snapshot.input_modes,
                )
            })
    }

    fn event_loop_dispatch_timeout(&self) -> Option<Duration> {
        event_loop_timeout(
            self.scheduling.exit,
            self.scheduling.signoff.is_some(),
            self.cursor_is_blinking()
                .then(|| self.input.last_cursor_blink.elapsed()),
        )
    }

    fn tick_cursor_blink(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let blinking = self.cursor_is_blinking();
        if blinking && self.input.last_cursor_blink.elapsed() >= CURSOR_BLINK_INTERVAL {
            self.input.cursor_blink_visible = !self.input.cursor_blink_visible;
            self.input.last_cursor_blink = Instant::now();
            if let Some(snapshot) = &self.panes.pane.snapshot
                && let Ok(row) = usize::try_from(snapshot.cursor_row)
                && row < snapshot.rows
            {
                self.panes
                    .pane
                    .raster_dirty_rows
                    .resize(snapshot.rows, false);
                self.panes
                    .pane
                    .surface_dirty_rows
                    .resize(snapshot.rows, false);
                self.panes.pane.raster_dirty_rows[row] = true;
                self.panes.pane.surface_dirty_rows[row] = true;
            }
            if self.surface.configured {
                self.schedule_draw(queue_handle)?;
            }
        } else if !blinking && !self.input.cursor_blink_visible {
            self.input.cursor_blink_visible = true;
            if let Some(snapshot) = &self.panes.pane.snapshot
                && let Ok(row) = usize::try_from(snapshot.cursor_row)
                && row < snapshot.rows
            {
                self.panes
                    .pane
                    .raster_dirty_rows
                    .resize(snapshot.rows, false);
                self.panes
                    .pane
                    .surface_dirty_rows
                    .resize(snapshot.rows, false);
                self.panes.pane.raster_dirty_rows[row] = true;
                self.panes.pane.surface_dirty_rows[row] = true;
            }
            if self.surface.configured {
                self.schedule_draw(queue_handle)?;
            }
        }
        Ok(())
    }

    fn schedule_draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        if self.scheduling.frame_pending {
            self.scheduling.redraw_pending = true;
            Ok(())
        } else {
            self.draw(queue_handle)
        }
    }

    fn schedule_terminal_draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        self.scheduling.terminal_redraw_pending = true;
        let draw_capacity_available = self.surface.buffers.len() < MAX_SHM_BUFFERS
            || self
                .surface
                .buffers
                .iter()
                .any(|buffer| self.surface.pool.canvas(&buffer.buffer).is_some());
        if terminal_draw_waits_for_frame(self.scheduling.frame_pending, draw_capacity_available) {
            self.scheduling.redraw_pending = true;
            Ok(())
        } else {
            // Terminal damage is coalesced while a compositor frame callback is
            // pending so bursty PTY redraws cannot render obsolete intermediate
            // states faster than they can be presented.
            self.draw(queue_handle)
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "SHM acquisition, persistent backing updates, damage submission, and commit form one transaction"
    )]
    fn draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        self.scheduling.redraw_pending = false;
        self.reconcile_command_palette_close(queue_handle);
        let terminal_priority = std::mem::take(&mut self.scheduling.terminal_redraw_pending);
        let draw_started = Instant::now();
        let scroll_started = self.panes.pane.scroll_started_at.take();
        if self.panes.pane.viewport_dirty {
            let display = self
                .panes
                .display_snapshot()
                .context("scroll display snapshot")?;
            let current_offset = self.panes.pane.scrollback_viewport.offset_from_bottom();
            let delta = isize::try_from(current_offset)
                .ok()
                .zip(isize::try_from(self.panes.pane.rendered_viewport_offset).ok())
                .map(|(current, rendered)| current - rendered);
            self.panes.pane.prepare_dirty_rows.fill(false);
            self.panes.pane.raster_dirty_rows.fill(false);
            self.panes.pane.surface_dirty_rows.fill(false);
            self.panes.pane.pending_scrolls.clear();
            let incremental = if display.images.is_none() {
                if let (Some(frame), Some(delta)) = (&mut self.panes.pane.snapshot_frame, delta) {
                    let scroll = frame.scroll_viewport_rows_with_context(
                        &display,
                        delta,
                        &self.presentation.render_context,
                    )?;
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
                self.panes
                    .pane
                    .raster_dirty_rows
                    .resize(display.rows, false);
                self.panes
                    .pane
                    .surface_dirty_rows
                    .resize(display.rows, false);
                for row in exposed {
                    self.panes.pane.raster_dirty_rows[row] = true;
                }
                self.panes.pane.surface_dirty_rows.fill(true);
                self.panes.pane.pending_scrolls.push(scroll);
            } else {
                self.panes.pane.snapshot_frame =
                    Some(SnapshotFrame::load_scaled_with_sources_and_context(
                        &display,
                        self.surface.scale_120,
                        Some(&self.panes.pane.image_sources),
                        &self.presentation.render_context,
                    )?);
                self.presentation.full_redraw = true;
            }
            self.panes.pane.rendered_viewport_offset = current_offset;
            self.panes.pane.viewport_dirty = false;
        }
        for pane in &mut self.panes.inactive_panes {
            if rebuild_dirty_pane_viewport_frame_with_context(
                pane,
                self.surface.scale_120,
                &self.presentation.render_context,
            )? {
                self.presentation.full_redraw = true;
            }
        }
        let pane_layout = self.computed_pane_layout()?;
        let pane_cell_width = self
            .panes
            .pane
            .snapshot_frame
            .as_ref()
            .map_or(1, SnapshotFrame::cell_width);
        self.prepare_frame_titles(pane_layout.as_ref(), pane_cell_width)?;
        let active_splint = self.panes.focused_splint();
        let active_rect = active_splint.and_then(|splint_id| {
            pane_layout
                .as_ref()
                .and_then(|layout| layout.rect(splint_id))
        });
        let window_geometry = if let Some(rect) = active_rect {
            Self::pane_geometry(&self.panes.pane, rect, self.surface.scale_120)
        } else {
            let content = self.content_rect();
            self.panes
                .pane
                .snapshot_frame
                .as_ref()
                .map_or(Ok(None), |frame| {
                    frame
                        .window_geometry(content.width, content.height, self.surface.scale_120)?
                        .translated(
                            logical_extent_to_buffer(content.x, self.surface.scale_120)?,
                            logical_extent_to_buffer(content.y, self.surface.scale_120)?,
                        )
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
        let (width, height, stride) = if pane_layout.is_some() || self.tab_state.managed_tabs {
            buffer_dimensions(
                self.surface.logical_width.max(1),
                self.surface.logical_height.max(1),
                self.surface.scale_120,
            )?
        } else if let Some(geometry) = window_geometry {
            geometry.buffer_layout()?
        } else {
            buffer_dimensions(
                self.surface.logical_width.max(1),
                self.surface.logical_height.max(1),
                self.surface.scale_120,
            )?
        };
        let width_i32 = i32::try_from(width).context("buffer width fits i32")?;
        let height_i32 = i32::try_from(height).context("buffer height fits i32")?;
        let resolved_selection = self.panes.pane.selection.and_then(|selection| {
            let snapshot = self.panes.pane.snapshot.as_ref()?;
            let display = self.panes.display_snapshot_cow()?;
            selection_display_bounds(snapshot, &display, selection)
                .map(|(start, end)| ((start.row, start.column), (end.row, end.column)))
        });
        let inline_picker_open = self.modal.inline_picker_open();
        let command_palette_open = self.modal.command_palette.is_some();
        let dojo_prompt_open = self.modal.dojo_prompt.is_some();
        let tab_context_menu_open = self.modal.tab_context_menu.is_some();
        let picker_layout = if inline_picker_open {
            let content = self.content_rect();
            let picker = self
                .modal
                .session_picker
                .as_mut()
                .context("inline picker exists")?;
            let (item_count, selected, visible_start) = picker.layout_state();
            let layout = session_picker_overlay_layout(
                content.width,
                content.height,
                self.surface.scale_120,
                item_count,
                selected,
                visible_start,
            )
            .map(|layout| translate_picker_layout(layout, content));
            if let Some(layout) = &layout {
                picker.set_visible_start(layout.visible_range.start);
            }
            layout
        } else {
            None
        };
        let command_palette_layout = self.modal.command_palette.as_ref().and_then(|palette| {
            command_palette_layout(
                self.content_rect(),
                palette.filtered(),
                palette.selected_index(),
                palette.visible_start(),
            )
        });
        let dojo_prompt_layout = self
            .modal
            .dojo_prompt
            .as_ref()
            .and_then(|prompt| dojo_prompt_layout(self.content_rect(), prompt));
        let tab_context_menu_layout = self.modal.tab_context_menu.as_ref().and_then(|_| {
            tab_context_menu_layout(
                Rect {
                    x: 0,
                    y: 0,
                    width: self.surface.logical_width,
                    height: self.surface.logical_height,
                },
                self.modal.tab_context_menu_anchor,
            )
        });
        let tab_layout = self.current_tab_strip_layout();
        let content_rect = self.content_rect();
        let content_buffer_rect = Self::buffer_rect(content_rect, self.surface.scale_120)?;
        let active_dojo = self.tab_state.active_dojo_id();
        if let Some(layout) = &tab_layout {
            self.prepare_tab_strip_text(layout)?;
        }
        self.tab_state.tab_strip_layout.clone_from(&tab_layout);
        let terminal_cursor_blink = presented_cursor_visible(
            inline_picker_open || command_palette_open || dojo_prompt_open || tab_context_menu_open,
            self.input.cursor_blink_visible,
        );
        let terminal_keyboard_focused = self.input.keyboard_focused
            && !inline_picker_open
            && !command_palette_open
            && !dojo_prompt_open
            && !tab_context_menu_open;
        let mut buffer_index = None;
        for (index, buffer) in self.surface.buffers.iter().enumerate() {
            if self.surface.pool.canvas(&buffer.buffer).is_some() {
                buffer_index = Some(index);
                break;
            }
        }
        let buffer_index = if let Some(index) = buffer_index {
            index
        } else if self.surface.buffers.len() < MAX_SHM_BUFFERS {
            let buffer = self
                .surface
                .pool
                .create_buffer(width_i32, height_i32, stride, wl_shm::Format::Argb8888)
                .context("create bounded SHM buffer")?
                .0;
            self.surface.buffers.push(ShmFrameBuffer {
                buffer,
                stale: BackingDamage::Full,
            });
            self.surface.buffers.len() - 1
        } else {
            self.scheduling.redraw_pending = true;
            self.scheduling.terminal_redraw_pending = terminal_priority;
            return Ok(());
        };
        let canvas = self
            .surface
            .pool
            .canvas(&self.surface.buffers[buffer_index].buffer)
            .context("selected SHM buffer became unavailable")?;

        let backing_len = usize::try_from(
            width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                .context("backing dimensions overflow")?,
        )
        .context("backing size fits usize")?;
        let resized_backing = self.surface.backing.len() != backing_len;
        if resized_backing {
            self.surface.backing.resize(backing_len, 0);
            self.presentation.full_redraw = true;
            for buffer in &mut self.surface.buffers {
                buffer.stale.mark_full();
            }
        }
        let mut copied_backing_bytes = 0;
        let mut inactive_damage_regions = Vec::new();
        let backing_scroll_changed = !self.panes.pane.pending_scrolls.is_empty();
        let capture_minimum_images = capture_minimum_images()?;
        let capture_image_count = self
            .panes
            .pane
            .snapshot_frame
            .as_ref()
            .map_or(0, SnapshotFrame::image_count)
            .saturating_add(
                self.panes
                    .inactive_panes
                    .iter()
                    .filter_map(|pane| pane.snapshot_frame.as_ref())
                    .map(SnapshotFrame::image_count)
                    .sum(),
            );
        let image_composition_started = (std::env::var_os("SPLINTERM_IMAGE_TRACE").is_some()
            && capture_image_count > 0)
            .then(Instant::now);
        if let (Some(frame), Some(geometry)) = (&self.panes.pane.snapshot_frame, &window_geometry) {
            if self.presentation.capture.is_some()
                && capture_minimum_images > 0
                && capture_image_count >= capture_minimum_images
            {
                self.presentation.full_redraw = true;
            }
            if self.presentation.full_redraw {
                if let Some(layout) = pane_layout.as_ref() {
                    let [_, red, green, blue] = self.presentation.theme.background.to_be_bytes();
                    let background = background_bgra(
                        [red, green, blue],
                        self.presentation.render_context.background_alpha(),
                    );
                    for pixel in self.surface.backing.chunks_exact_mut(4) {
                        pixel.copy_from_slice(&background);
                    }
                    for pane in &self.panes.inactive_panes {
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
                            Self::pane_geometry(pane, rect, self.surface.scale_120)?,
                        ) else {
                            continue;
                        };
                        paint_snapshot_region_presented(
                            &mut self.surface.backing,
                            width,
                            height,
                            frame,
                            &geometry,
                            Self::buffer_rect(rect, self.surface.scale_120)?,
                            terminal_cursor_blink,
                            self.presentation.cursor_style,
                            CursorPresentation::INACTIVE_PANE,
                        );
                    }
                    paint_snapshot_region_presented(
                        &mut self.surface.backing,
                        width,
                        height,
                        frame,
                        geometry,
                        Self::buffer_rect(
                            active_rect.context("active pane rectangle")?,
                            self.surface.scale_120,
                        )?,
                        terminal_cursor_blink,
                        self.presentation.cursor_style,
                        CursorPresentation::for_keyboard_focus(terminal_keyboard_focused),
                    );
                } else {
                    paint_snapshot_presented(
                        &mut self.surface.backing,
                        width,
                        height,
                        frame,
                        geometry,
                        terminal_cursor_blink,
                        self.presentation.cursor_style,
                        CursorPresentation::for_keyboard_focus(terminal_keyboard_focused),
                    );
                }
            } else {
                for scroll in self.panes.pane.pending_scrolls.drain(..) {
                    scroll_snapshot_pixels(
                        &mut self.surface.backing,
                        width,
                        frame,
                        geometry,
                        scroll,
                    );
                }
                paint_snapshot_rows_presented(
                    &mut self.surface.backing,
                    width,
                    height,
                    frame,
                    geometry,
                    &self.panes.pane.raster_dirty_rows,
                    terminal_cursor_blink,
                    self.presentation.cursor_style,
                    CursorPresentation::for_keyboard_focus(terminal_keyboard_focused),
                );
                if let Some(layout) = pane_layout.as_ref() {
                    for pane in &self.panes.inactive_panes {
                        let Some(splint_id) = pane
                            .snapshot
                            .as_ref()
                            .map(|snapshot| snapshot.splint_id)
                            .filter(|splint_id| {
                                self.panes.dirty_inactive_panes.contains(splint_id)
                            })
                        else {
                            continue;
                        };
                        let Some(rect) = layout.rect(splint_id) else {
                            continue;
                        };
                        let (Some(frame), Some(geometry)) = (
                            &pane.snapshot_frame,
                            Self::pane_geometry(pane, rect, self.surface.scale_120)?,
                        ) else {
                            continue;
                        };
                        let region = Self::buffer_rect(rect, self.surface.scale_120)?;
                        paint_snapshot_region_presented(
                            &mut self.surface.backing,
                            width,
                            height,
                            frame,
                            &geometry,
                            region,
                            terminal_cursor_blink,
                            self.presentation.cursor_style,
                            CursorPresentation::INACTIVE_PANE,
                        );
                        inactive_damage_regions.push(region);
                    }
                }
            }
            let selection = resolved_selection;
            let hovered_url = self
                .panes
                .pane
                .hovered_url
                .as_ref()
                .map(|(start, end, _)| ((start.row, start.column), (end.row, end.column)));
            let history_status = history_overlay_status(
                &self.panes.pane.scrollback_viewport,
                self.panes.pane.snapshot.as_ref(),
            );
            // Pane chrome is deterministically repainted after synchronization, so
            // row scanline copies may overwrite it without contaminating reuse.
            let overlay_rows = transient_overlay_rows(
                self.panes
                    .pane
                    .snapshot
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.rows),
                selection,
                hovered_url,
            );
            let full_transient_canvas_content = history_status.is_some()
                || self.modal.trusted_consent.is_some()
                || inline_picker_open
                || command_palette_open
                || dojo_prompt_open
                || tab_context_menu_open;
            let full_backing_sync =
                self.presentation.full_redraw || capture_image_count > 0 || backing_scroll_changed;
            for buffer in &mut self.surface.buffers {
                if full_backing_sync {
                    buffer.stale.mark_full();
                } else {
                    buffer.stale.mark_rows(&self.panes.pane.raster_dirty_rows);
                    buffer.stale.mark_regions(&inactive_damage_regions);
                }
            }
            let stale = std::mem::replace(
                &mut self.surface.buffers[buffer_index].stale,
                BackingDamage::Clean,
            );
            copied_backing_bytes = sync_backing_damage(
                canvas,
                &self.surface.backing,
                width,
                height,
                window_geometry.as_ref(),
                &stale,
            );
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
                    focused: terminal_keyboard_focused,
                    selection_color: self.presentation.theme.selection,
                    url_color: self.presentation.theme.url,
                    accent_color: self.presentation.theme.ui_accent,
                },
            );
            if let Some(layout) = pane_layout.as_ref() {
                paint_pane_chrome(
                    canvas,
                    width,
                    height,
                    layout,
                    active_splint,
                    self.presentation.theme,
                    frame.cell_width(),
                    frame.cell_height(),
                    self.surface.scale_120,
                    &self.presentation.frame_titles,
                )?;
            }
            if let Some(status) = history_overlay_status(
                &self.panes.pane.scrollback_viewport,
                self.panes.pane.snapshot.as_ref(),
            ) {
                paint_history_overlay(
                    canvas,
                    width,
                    height,
                    content_buffer_rect,
                    self.surface.scale_120,
                    status,
                    self.presentation.theme.background,
                    self.presentation.theme.ui_accent,
                );
            }
            if self.modal.trusted_consent.is_some() {
                paint_trusted_consent_chrome(canvas, width, height);
            }
            if full_transient_canvas_content {
                self.surface.buffers[buffer_index].stale.mark_full();
            } else {
                self.surface.buffers[buffer_index]
                    .stale
                    .mark_rows(&overlay_rows);
            }
        } else if self.panes.pane.snapshot_frame.is_some() {
            let [_, red, green, blue] = self.presentation.theme.background.to_be_bytes();
            let background = background_bgra(
                [red, green, blue],
                self.presentation.render_context.background_alpha(),
            );
            for pixel in self.surface.backing.chunks_exact_mut(4) {
                pixel.copy_from_slice(&background);
            }
            for buffer in &mut self.surface.buffers {
                buffer.stale.mark_full();
            }
            let stale = std::mem::replace(
                &mut self.surface.buffers[buffer_index].stale,
                BackingDamage::Clean,
            );
            copied_backing_bytes =
                sync_backing_damage(canvas, &self.surface.backing, width, height, None, &stale);
        } else if let Some(row) = &self.presentation.text_row {
            paint(canvas, width, height, row);
            self.surface.buffers[buffer_index].stale.mark_full();
        } else {
            anyhow::bail!("window has no prepared renderer content");
        }
        if let Some(layout) = tab_layout.as_ref() {
            Self::paint_tab_strip(
                canvas,
                width,
                height,
                layout,
                self.surface.scale_120,
                self.presentation.theme,
                active_dojo,
                &self.tab_state.tab_label_cache,
                self.tab_state.tab_close_text.as_ref().map(|(_, text)| text),
                self.tab_state.tab_new_text.as_ref().map(|(_, text)| text),
            )?;
            self.surface.buffers[buffer_index].stale.mark_full();
        }
        if let (Some(layout), Some(picker)) =
            (picker_layout.as_ref(), self.modal.session_picker.as_ref())
        {
            let items = picker
                .items()
                .iter()
                .map(|item| SessionPickerTextItem {
                    display_title: &item.display_title,
                    working_directory: &item.working_directory,
                    pane_count: item.pane_count,
                    running_pane_count: item.running_pane_count,
                })
                .collect::<Vec<_>>();
            paint_session_picker_overlay(
                &mut self.modal.session_picker_text_cache,
                &self.presentation.render_context,
                canvas,
                width,
                height,
                content_buffer_rect,
                self.surface.scale_120,
                self.presentation.renderer_generation,
                layout,
                session_picker_palette(self.presentation.theme),
                &items,
                picker.selected_target(),
                picker.hovered(),
                self.modal.session_picker_pressed,
                self.input.keyboard_focused,
            )?;
            self.surface.buffers[buffer_index].stale.mark_full();
        }
        if let (Some(layout), Some(palette)) = (
            command_palette_layout.as_ref(),
            self.modal.command_palette.as_ref(),
        ) {
            paint_command_palette(
                &mut self.modal.command_palette_text_cache,
                &self.presentation.render_context,
                canvas,
                width,
                height,
                content_rect,
                self.surface.scale_120,
                self.presentation.renderer_generation,
                layout,
                session_picker_palette(self.presentation.theme),
                palette,
                &self.input.keymap,
                self.modal.command_palette_pressed,
                self.input.keyboard_focused,
            )?;
            self.surface.buffers[buffer_index].stale.mark_full();
        }
        self.modal.command_palette_layout = command_palette_layout;
        if let (Some(layout), Some(prompt)) =
            (dojo_prompt_layout.as_ref(), self.modal.dojo_prompt.as_ref())
        {
            paint_dojo_prompt(
                &mut self.modal.dojo_prompt_text_cache,
                &self.presentation.render_context,
                canvas,
                width,
                height,
                content_rect,
                self.surface.scale_120,
                self.presentation.renderer_generation,
                layout,
                session_picker_palette(self.presentation.theme),
                prompt,
                self.input.keyboard_focused,
            )?;
            self.surface.buffers[buffer_index].stale.mark_full();
        }
        self.modal.dojo_prompt_layout = dojo_prompt_layout;
        if let (Some(layout), Some(menu)) = (
            tab_context_menu_layout.as_ref(),
            self.modal.tab_context_menu.as_ref(),
        ) {
            paint_tab_context_menu(
                &mut self.modal.tab_context_menu_text_cache,
                &self.presentation.render_context,
                canvas,
                width,
                height,
                self.surface.scale_120,
                self.presentation.renderer_generation,
                layout,
                session_picker_palette(self.presentation.theme),
                menu,
                self.modal.tab_context_menu_pressed,
                self.input.keyboard_focused,
            )?;
            self.surface.buffers[buffer_index].stale.mark_full();
        }
        self.modal.tab_context_menu_layout = tab_context_menu_layout;
        if let Some(started) = image_composition_started {
            eprintln!(
                "phase5-image-trace composition_ns={} image_count={capture_image_count}",
                started.elapsed().as_nanos(),
            );
        }
        let capture_scale_ready = self
            .presentation
            .capture_scale
            .is_none_or(|expected| expected.saturating_mul(120) == self.surface.scale_120);
        if deterministic_capture_ready(
            capture_scale_ready,
            capture_minimum_images,
            capture_image_count,
        ) && let Some(path) = self.presentation.capture.take()
        {
            write_ppm(&path, canvas, width, height)
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!(
                "Wrote deterministic row capture at {}x scale to {}",
                f64::from(self.surface.scale_120) / 120.0,
                path.display()
            );
        }
        let history_status = history_overlay_status(
            &self.panes.pane.scrollback_viewport,
            self.panes.pane.snapshot.as_ref(),
        );
        let picker_damage_full_surface = std::mem::take(&mut self.modal.session_picker_redraw);
        let damage_full_surface = picker_damage_full_surface
            || take_full_surface_damage(
                &mut self.presentation.full_redraw,
                self.panes.pane.snapshot_frame.is_some(),
            );
        if damage_full_surface {
            self.surface
                .window
                .wl_surface()
                .damage_buffer(0, 0, width_i32, height_i32);
        } else {
            if let Some(geometry) = &window_geometry {
                for (row, dirty) in self
                    .panes
                    .pane
                    .surface_dirty_rows
                    .iter()
                    .copied()
                    .enumerate()
                {
                    if !dirty {
                        continue;
                    }
                    if let Some((x, y, row_width, row_height)) = snapshot_row_rect(geometry, row) {
                        self.surface
                            .window
                            .wl_surface()
                            .damage_buffer(x, y, row_width, row_height);
                    }
                }
            }
            for region in &inactive_damage_regions {
                self.surface.window.wl_surface().damage_buffer(
                    i32::try_from(region.x).unwrap_or(i32::MAX),
                    i32::try_from(region.y).unwrap_or(i32::MAX),
                    i32::try_from(region.width).unwrap_or(i32::MAX),
                    i32::try_from(region.height).unwrap_or(i32::MAX),
                );
            }
        }
        if history_status != self.panes.pane.painted_history_status {
            let content = Self::buffer_rect(self.content_rect(), self.surface.scale_120)?;
            if let Some(layout) =
                history_overlay_layout(content.width, content.height, self.surface.scale_120)
            {
                let (x, y, overlay_width, overlay_height) = layout.panel;
                self.surface.window.wl_surface().damage_buffer(
                    x.saturating_add(i32::try_from(content.x).unwrap_or(i32::MAX)),
                    y.saturating_add(i32::try_from(content.y).unwrap_or(i32::MAX)),
                    i32::try_from(overlay_width).unwrap_or(i32::MAX),
                    i32::try_from(overlay_height).unwrap_or(i32::MAX),
                );
            }
        }
        self.panes.pane.painted_history_status = history_status;
        self.panes.pane.raster_dirty_rows.fill(false);
        self.panes.pane.surface_dirty_rows.fill(false);
        self.panes.pane.pending_scrolls.clear();
        if self
            .surface
            .background_effect_reconcile_schedule
            .take_for_draw()
        {
            self.reconcile_background_effect(
                queue_handle,
                BackgroundEffectCommitMode::DeferToDraw,
            )?;
        }
        let committed_identity = self
            .panes
            .pane
            .snapshot
            .as_ref()
            .map(|snapshot| (snapshot.splint_id, snapshot.incarnation, snapshot.revision));
        let pane_commit_traces = if perf_trace_enabled() {
            pending_pane_commit_traces(&self.panes.pane, &self.panes.inactive_panes)
        } else {
            Vec::new()
        };
        let commit_sequence = perf_trace_enabled().then(|| {
            let sequence = self.scheduling.next_commit_sequence;
            self.scheduling.next_commit_sequence = sequence.saturating_add(1);
            sequence
        });
        let requests_frame_callback = !self.scheduling.frame_pending;
        if requests_frame_callback {
            self.surface
                .window
                .wl_surface()
                .frame(queue_handle, self.surface.window.wl_surface().clone());
            self.scheduling.frame_pending = true;
        }
        self.surface.buffers[buffer_index]
            .buffer
            .attach_to(self.surface.window.wl_surface())
            .context("attach SHM buffer")?;
        self.surface.window.commit();
        if let Some(diagnostics) = diagnostics() {
            diagnostics.mark_window_mapped();
        }
        let draw_duration_ns = u64::try_from(draw_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let committed_monotonic_raw_ns = commit_sequence.map(|_| monotonic_raw_ns());
        if let (Some(commit_sequence), Some(committed_monotonic_raw_ns)) =
            (commit_sequence, committed_monotonic_raw_ns)
        {
            let snapshot = self.panes.pane.snapshot.as_ref();
            emit_perf_trace_at(
                "splinterm",
                "draw_commit",
                PerfTraceEvent {
                    commit_sequence: Some(commit_sequence),
                    duration_ns: Some(draw_duration_ns),
                    bytes: Some(u64::try_from(copied_backing_bytes).unwrap_or(u64::MAX)),
                    rows: snapshot.map(|snapshot| u64::try_from(snapshot.rows).unwrap_or(u64::MAX)),
                    pane_count: Some(
                        u64::try_from(self.panes.inactive_panes.len().saturating_add(1))
                            .unwrap_or(u64::MAX),
                    ),
                    backing_copy_bytes: Some(
                        u64::try_from(copied_backing_bytes).unwrap_or(u64::MAX),
                    ),
                    full_reload: Some(damage_full_surface),
                    ..PerfTraceEvent::default()
                },
                committed_monotonic_raw_ns,
            );
            for trace in &pane_commit_traces {
                emit_perf_trace_at(
                    "splinterm",
                    "pane_commit",
                    pane_commit_event(*trace, commit_sequence),
                    committed_monotonic_raw_ns,
                );
            }
            if requests_frame_callback {
                self.scheduling.pending_frame_trace = Some(PendingFrameTrace {
                    commit_sequence,
                    committed_monotonic_raw_ns,
                });
            }
        }
        if self.panes.pane.trace_correlation.is_some() {
            self.panes.pane.clear_trace_correlation();
        }
        for pane in &mut self.panes.inactive_panes {
            if pane.trace_correlation.is_some() {
                pane.clear_trace_correlation();
            }
        }
        self.panes.dirty_inactive_panes.clear();
        self.modal.session_picker_layout = picker_layout;
        self.complete_background_effect_draw_commit()?;
        let inject_graphical_input = self
            .scheduling
            .graphical_input_probe
            .as_mut()
            .is_some_and(|probe| probe.observe_commit(committed_identity));
        if inject_graphical_input {
            self.scheduling.graphical_input_probe = None;
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
        if self.scheduling.scroll_trace
            && let Some(scroll_started) = scroll_started
        {
            eprintln!(
                "scroll-trace input_to_commit_us={} draw_us={} viewport_offset={} cached_rows={} page_pending={}",
                scroll_started.elapsed().as_micros(),
                draw_started.elapsed().as_micros(),
                self.panes.pane.scrollback_viewport.offset_from_bottom(),
                self.panes
                    .pane
                    .snapshot
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.scrollback_rows.len()),
                self.panes.pane.history_page_pending,
            );
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf};

    use super::*;
    use sha2::{Digest, Sha256};
    use splinterm_automation_client::{ImageContentSource, SharedImageContentCache};
    use splinterm_core::{Axis, Splint, SplitRatio};
    use splinterm_protocol::{
        ActiveScreen, ImageAlphaMode, ImageContentMetadata, ImageRetention, ImageSourceFormat,
    };

    fn update_test_waker() -> (EventLoop<'static, usize>, Waker) {
        let event_loop = EventLoop::<usize>::try_new().unwrap();
        let (ping, source) = make_ping().unwrap();
        event_loop
            .handle()
            .insert_source(source, |(), (), wake_count| *wake_count += 1)
            .unwrap();
        (event_loop, Waker::from(Arc::new(UpdateWake(ping))))
    }

    #[test]
    fn final_tab_removal_hands_off_one_pending_picker_request() {
        assert_eq!(
            final_tab_removal_action(true, 0, true),
            FinalTabRemovalAction::ExitAndHandoffPicker
        );
        assert_eq!(
            final_tab_removal_action(true, 0, false),
            FinalTabRemovalAction::Exit
        );
        assert_eq!(
            final_tab_removal_action(true, 1, true),
            FinalTabRemovalAction::Continue
        );
        assert_eq!(
            final_tab_removal_action(false, 0, true),
            FinalTabRemovalAction::Continue
        );

        let command = session_picker_handoff_command(Path::new("/opt/splinterm"));
        assert_eq!(command.get_program(), "/opt/splinterm");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["sessions"]);
    }

    #[test]
    fn one_pending_remote_split_blocks_more_splits_and_cannot_receive_focus() {
        let target = SplintId::new();
        let pending = SplintId::new();
        let mut splits = HashMap::new();
        assert!(remote_split_can_begin(&splits));
        assert!(!is_pending_remote_splint(&splits, target));

        splits.insert(target, pending);
        assert!(!remote_split_can_begin(&splits));
        assert!(is_pending_remote_splint(&splits, pending));
        assert!(!is_pending_remote_splint(&splits, target));
    }

    #[test]
    fn pending_remote_snapshot_is_valid_and_identifiable() {
        let splint_id = SplintId::new();
        let snapshot = pending_remote_snapshot(splint_id, 80, 24);

        snapshot.validate().unwrap();
        assert_eq!(snapshot.splint_id, splint_id);
        assert_eq!(snapshot.title, "Opening remote pane…");
        assert_eq!(
            snapshot.visible_rows[0]
                .cells
                .iter()
                .map(|cell| cell.content.as_str())
                .collect::<String>(),
            "Opening remote pane…"
        );
        assert!(!snapshot.input_modes.cursor_visible);
    }

    #[test]
    fn pending_split_inserts_second_leaf_without_changing_target_identity() {
        let target = Splint::shell(PathBuf::from("/tmp"));
        let target_id = target.id;
        let mut pending = Splint::shell(PathBuf::from("/"));
        let pending_id = pending.id;
        "Opening remote pane…".clone_into(&mut pending.title);
        let mut layout = LayoutNode::Leaf(target);

        assert!(insert_pending_split(
            &mut layout,
            target_id,
            pending,
            Axis::Horizontal,
        ));
        assert_eq!(layout.splint_count(), 2);
        assert!(layout.find_splint(target_id).is_some());
        assert_eq!(
            layout
                .find_splint(pending_id)
                .map(|splint| splint.title.as_str()),
            Some("Opening remote pane…")
        );
    }

    #[test]
    fn pending_split_rollback_collapses_only_the_placeholder_leaf() {
        let target = Splint::shell(PathBuf::from("/tmp"));
        let target_id = target.id;
        let sibling = Splint::shell(PathBuf::from("/tmp"));
        let sibling_id = sibling.id;
        let pending = Splint::shell(PathBuf::from("/"));
        let pending_id = pending.id;
        let mut split = LayoutNode::Leaf(target);
        assert!(insert_pending_split(
            &mut split,
            target_id,
            pending,
            Axis::Horizontal,
        ));
        let layout = LayoutNode::Branch {
            axis: Axis::Vertical,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(split),
            second: Box::new(LayoutNode::Leaf(sibling)),
        };

        let (restored, removed) = remove_pending_split(layout, pending_id);
        let restored = restored.unwrap();
        assert!(removed);
        assert_eq!(restored.splint_count(), 2);
        assert!(restored.find_splint(target_id).is_some());
        assert!(restored.find_splint(sibling_id).is_some());
        assert!(restored.find_splint(pending_id).is_none());
    }

    #[test]
    fn cursor_rectangle_offsets_into_pane() {
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
    fn picker_translation_moves_visual_surfaces_with_hit_slots_and_text() {
        let layout = session_picker_overlay_layout(960, 600, 120, 3, 1, 0).unwrap();
        let origin = Rect {
            x: 7,
            y: TAB_STRIP_LOGICAL_HEIGHT,
            width: 960,
            height: 600,
        };
        let translated = translate_picker_layout(layout.clone(), origin);
        for (source, moved) in layout.rows.iter().zip(&translated.rows) {
            assert_eq!(moved.rect, translated_rect(source.rect, origin));
            assert_eq!(moved.surface, translated_rect(source.surface, origin));
            assert_eq!(moved.title_clip, translated_rect(source.title_clip, origin));
            assert_eq!(
                moved.metadata_clip,
                translated_rect(source.metadata_clip, origin)
            );
        }
    }

    #[test]
    fn blur_only_theme_updates_reconcile_immediately_without_queuing_pixel_work() {
        let current = ResolvedTheme::default();
        let mut blur_only = current;
        blur_only.background_blur = true;
        let impact = classify_theme_update(current, blur_only);
        assert_eq!(
            impact,
            ThemeUpdateImpact {
                rebuild_pixels: false,
                reconcile_effect: true,
            }
        );
        let mut schedule = BackgroundEffectReconcileSchedule::default();
        assert!(schedule.queue_update(impact.reconcile_effect, impact.rebuild_pixels, true));
        assert!(!schedule.on_draw);

        let mut alpha = blur_only;
        alpha.background_alpha = u16::MAX - 1;
        assert_eq!(
            classify_theme_update(blur_only, alpha),
            ThemeUpdateImpact {
                rebuild_pixels: true,
                reconcile_effect: true,
            }
        );

        let mut palette = blur_only;
        palette.foreground ^= 0x00ff_ffff;
        assert_eq!(
            classify_theme_update(blur_only, palette),
            ThemeUpdateImpact {
                rebuild_pixels: true,
                reconcile_effect: false,
            }
        );
        assert_eq!(
            classify_theme_update(current, current),
            ThemeUpdateImpact::default()
        );
    }

    #[test]
    fn draw_bound_effect_reconciliation_survives_later_updates_and_capabilities() {
        let mut schedule = BackgroundEffectReconcileSchedule::default();
        assert!(!schedule.queue_update(true, true, true));
        assert!(schedule.on_draw);

        assert!(!schedule.queue_update(true, false, true));
        assert!(schedule.on_draw);
        assert!(!schedule.capability_reconciles_immediately());

        assert!(schedule.take_for_draw());
        assert!(!schedule.on_draw);
        assert!(schedule.capability_reconciles_immediately());
        assert!(!schedule.take_for_draw());

        schedule.queue_geometry();
        assert!(schedule.on_draw);
        schedule.clear();
        assert!(!schedule.on_draw);
    }

    #[test]
    fn background_effect_dispatch_preserves_known_and_unknown_capability_bits() {
        assert_eq!(
            background_effect_capability_bits(WEnum::Value(BackgroundCapability::Blur)),
            crate::background_effect::BLUR_CAPABILITY
        );
        assert_eq!(
            background_effect_capability_bits(WEnum::Unknown(0x8000_0000)),
            0x8000_0000
        );
    }

    #[test]
    fn background_effect_diagnostics_and_trace_are_bounded_metadata_only() {
        let size = crate::background_effect::LogicalSize::new(960, 600).unwrap();
        assert_eq!(
            background_effect_trace_line(EffectAction::SetBlurRegion(size)).as_deref(),
            Some("splinterm background-effect region=960x600")
        );
        assert_eq!(
            background_effect_trace_line(EffectAction::CommitSurface(
                crate::background_effect::CommitReason::Enable
            ))
            .as_deref(),
            Some("splinterm background-effect commit=Enable")
        );
        assert!(
            background_effect_trace_line(EffectAction::Diagnostic(
                EffectDiagnostic::MissingManager
            ))
            .is_none()
        );
        for diagnostic in [
            EffectDiagnostic::MissingManager,
            EffectDiagnostic::MissingBlurCapability,
        ] {
            let message = background_effect_diagnostic_message(diagnostic);
            assert!(message.len() < 128);
            assert!(message.starts_with("splinterm background blur requested"));
        }
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
            .insert_source(source, |(), (), wake_count| *wake_count += 1)
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
            .insert_source(source, |(), (), wake_count| *wake_count += 1)
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
    fn terminal_stream_notices_distinguish_clean_exit_from_unexpected_disconnect() {
        let splint_id = SplintId::new();
        assert!(pane_stream_has_terminal_notice(&[WindowUpdate::Exited {
            splint_id
        }]));
        assert!(pane_stream_has_terminal_notice(&[WindowUpdate::Shutdown]));
        assert!(!pane_stream_has_terminal_notice(&[WindowUpdate::Control(
            false
        )]));
    }

    #[test]
    fn pending_exited_splints_retry_full_and_closed_topology_queues() {
        let first = SplintId::new();
        let second = SplintId::new();
        let mut pending = HashSet::from([first, second]);
        let dojo_id = DojoId::new();
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);

        assert!(enqueue_pending_exited_splints(
            dojo_id,
            &mut pending,
            &commands
        ));
        assert_eq!(pending.len(), 1);
        let WindowTopologyCommand::Close {
            dojo_id: accepted_dojo,
            target: accepted,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected automatic close command");
        };
        assert_eq!(accepted_dojo, dojo_id);
        assert!(!pending.contains(&accepted));

        assert!(enqueue_pending_exited_splints(
            dojo_id,
            &mut pending,
            &commands
        ));
        assert!(pending.is_empty());
        let WindowTopologyCommand::Close {
            dojo_id: retained_dojo,
            target: retained,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected retried automatic close command");
        };
        assert_eq!(retained_dojo, dojo_id);
        drop(receiver);
        pending.insert(retained);
        assert!(!enqueue_pending_exited_splints(
            dojo_id,
            &mut pending,
            &commands
        ));
        assert_eq!(pending, HashSet::from([retained]));
    }

    #[test]
    fn receiver_drain_yields_to_a_concurrently_refilling_producer() {
        let event_loop = EventLoop::<usize>::try_new().unwrap();
        let (ping, source) = make_ping().unwrap();
        event_loop
            .handle()
            .insert_source(source, |(), (), wake_count| *wake_count += 1)
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
            assert!(
                drained.disconnected || drained.items.len() <= RECEIVER_DRAIN_BUDGET,
                "an active producer drain exceeded its cooperative budget"
            );
            items_seen.extend(drained.items);
            disconnected = drained.disconnected;
            std::thread::yield_now();
        }
        producer.join().unwrap();
        assert_eq!(items_seen, (0..item_count).collect::<Vec<_>>());
    }

    #[test]
    fn inactive_only_visual_changes_preserve_focused_cursor_blink_phase() {
        let original = Instant::now()
            .checked_sub(Duration::from_millis(250))
            .unwrap();
        let mut last_blink = original;
        let mut visible = false;
        assert!(!restart_cursor_blink(false, &mut visible, &mut last_blink));
        assert!(!visible);
        assert_eq!(last_blink, original);

        assert!(restart_cursor_blink(true, &mut visible, &mut last_blink));
        assert!(visible);
        assert!(last_blink > original);
    }

    #[test]
    fn transient_overlays_only_stale_the_rows_they_touch() {
        assert_eq!(
            transient_overlay_rows(5, Some(((1, 3), (3, 7))), Some(((4, 0), (4, 8)))),
            vec![false, true, true, true, true]
        );
        assert_eq!(transient_overlay_rows(3, None, None), vec![false; 3]);
        assert!(transient_overlay_rows(0, Some(((0, 0), (2, 0))), None).is_empty());
        assert_eq!(
            transient_overlay_rows(3, Some(((7, 0), (9, 0))), None),
            vec![false; 3]
        );
    }

    #[test]
    fn backing_damage_accumulates_independently_across_reused_buffers() {
        let first_region = Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        };
        let second_region = Rect {
            x: 5,
            y: 6,
            width: 7,
            height: 8,
        };
        let mut buffers = [BackingDamage::Clean, BackingDamage::Clean];
        for damage in &mut buffers {
            damage.mark_rows(&[true, false, false]);
            damage.mark_regions(&[first_region, first_region]);
        }
        let first = std::mem::replace(&mut buffers[0], BackingDamage::Clean);
        assert_eq!(
            first,
            BackingDamage::Partial {
                dirty_rows: vec![true, false, false],
                dirty_regions: vec![first_region],
            }
        );

        for damage in &mut buffers {
            damage.mark_rows(&[false, false, true]);
            damage.mark_regions(&[second_region]);
        }
        assert_eq!(
            buffers[0],
            BackingDamage::Partial {
                dirty_rows: vec![false, false, true],
                dirty_regions: vec![second_region],
            }
        );
        assert_eq!(
            buffers[1],
            BackingDamage::Partial {
                dirty_rows: vec![true, false, true],
                dirty_regions: vec![first_region, second_region],
            }
        );
        buffers[1].mark_full();
        buffers[1].mark_rows(&[false, true, false]);
        assert_eq!(buffers[1], BackingDamage::Full);
    }

    #[test]
    fn backing_damage_copies_only_stale_row_scanlines() {
        let cell = crate::geometry::CellGeometry::from_metrics(2, 2, 1, 1, 1).unwrap();
        let geometry = WindowGeometry::for_grid(
            2,
            3,
            cell,
            crate::geometry::TerminalPadding::uniform(0),
            120,
        )
        .unwrap();
        let (width, height, _) = geometry.buffer_layout().unwrap();
        let len = usize::try_from(width * height * 4).unwrap();
        let backing = (0..len)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect::<Vec<_>>();
        let mut canvas = vec![0xff; len];
        let copied = sync_backing_damage(
            &mut canvas,
            &backing,
            width,
            height,
            Some(&geometry),
            &BackingDamage::Partial {
                dirty_rows: vec![false, true, false],
                dirty_regions: Vec::new(),
            },
        );
        let stride = usize::try_from(width).unwrap() * 4;
        assert_eq!(copied, stride * 2);
        assert!(canvas[..stride * 2].iter().all(|byte| *byte == 0xff));
        assert_eq!(
            &canvas[stride * 2..stride * 4],
            &backing[stride * 2..stride * 4]
        );
        assert!(canvas[stride * 4..].iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn backing_damage_copies_only_stale_pane_regions() {
        let width = 4;
        let height = 3;
        let len = usize::try_from(width * height * 4).unwrap();
        let backing = (0..len)
            .map(|index| u8::try_from(index).unwrap())
            .collect::<Vec<_>>();
        let mut canvas = vec![0xff; len];
        let region = Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 1,
        };
        let copied = sync_backing_damage(
            &mut canvas,
            &backing,
            width,
            height,
            None,
            &BackingDamage::Partial {
                dirty_rows: Vec::new(),
                dirty_regions: vec![region],
            },
        );
        let stride = usize::try_from(width).unwrap() * 4;
        assert_eq!(copied, 8);
        assert!(canvas[..stride + 4].iter().all(|byte| *byte == 0xff));
        assert_eq!(
            &canvas[stride + 4..stride + 12],
            &backing[stride + 4..stride + 12]
        );
        assert!(canvas[stride + 12..].iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn alternating_backing_buffers_repair_every_missed_row_update() {
        let cell = crate::geometry::CellGeometry::from_metrics(2, 2, 1, 1, 1).unwrap();
        let geometry = WindowGeometry::for_grid(
            2,
            3,
            cell,
            crate::geometry::TerminalPadding::uniform(0),
            120,
        )
        .unwrap();
        let (width, height, _) = geometry.buffer_layout().unwrap();
        let stride = usize::try_from(width).unwrap() * 4;
        let mut backing = vec![0; stride * usize::try_from(height).unwrap()];
        let mut canvases = [backing.clone(), backing.clone()];
        let mut damage = [BackingDamage::Clean, BackingDamage::Clean];

        backing[..stride * 2].fill(1);
        for stale in &mut damage {
            stale.mark_rows(&[true, false, false]);
        }
        let first = std::mem::replace(&mut damage[0], BackingDamage::Clean);
        sync_backing_damage(
            &mut canvases[0],
            &backing,
            width,
            height,
            Some(&geometry),
            &first,
        );
        assert_eq!(canvases[0], backing);
        assert_ne!(canvases[1], backing);

        backing[stride * 4..stride * 6].fill(2);
        for stale in &mut damage {
            stale.mark_rows(&[false, false, true]);
        }
        let second = std::mem::replace(&mut damage[1], BackingDamage::Clean);
        sync_backing_damage(
            &mut canvases[1],
            &backing,
            width,
            height,
            Some(&geometry),
            &second,
        );
        assert_eq!(canvases[1], backing);
        assert_ne!(canvases[0], backing);

        let first = std::mem::replace(&mut damage[0], BackingDamage::Clean);
        sync_backing_damage(
            &mut canvases[0],
            &backing,
            width,
            height,
            Some(&geometry),
            &first,
        );
        assert_eq!(canvases[0], backing);
        assert_eq!(canvases[1], backing);
    }

    #[test]
    fn alternating_backing_buffers_repair_every_missed_pane_region() {
        let width = 4;
        let height = 2;
        let stride = usize::try_from(width).unwrap() * 4;
        let mut backing = vec![0; stride * usize::try_from(height).unwrap()];
        let mut canvases = [backing.clone(), backing.clone()];
        let mut damage = [BackingDamage::Clean, BackingDamage::Clean];
        let left = Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let right = Rect {
            x: 2,
            y: 1,
            width: 2,
            height: 1,
        };

        backing[..8].fill(1);
        for stale in &mut damage {
            stale.mark_regions(&[left]);
        }
        let first = std::mem::replace(&mut damage[0], BackingDamage::Clean);
        sync_backing_damage(&mut canvases[0], &backing, width, height, None, &first);
        assert_eq!(canvases[0], backing);
        assert_ne!(canvases[1], backing);

        backing[stride + 8..stride + 16].fill(2);
        for stale in &mut damage {
            stale.mark_regions(&[right]);
        }
        let second = std::mem::replace(&mut damage[1], BackingDamage::Clean);
        sync_backing_damage(&mut canvases[1], &backing, width, height, None, &second);
        assert_eq!(canvases[1], backing);
        assert_ne!(canvases[0], backing);

        let first = std::mem::replace(&mut damage[0], BackingDamage::Clean);
        sync_backing_damage(&mut canvases[0], &backing, width, height, None, &first);
        assert_eq!(canvases[0], backing);
        assert_eq!(canvases[1], backing);
    }

    #[test]
    fn backing_full_damage_restores_transient_canvas_bytes() {
        let backing = vec![7; 32];
        let mut canvas = vec![9; 32];
        assert_eq!(
            sync_backing_damage(&mut canvas, &backing, 4, 2, None, &BackingDamage::Full),
            backing.len()
        );
        assert_eq!(canvas, backing);
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
    fn hidden_tab_updates_cache_without_rebuild_resize_or_control_claim() {
        let (_event_loop, waker) = update_test_waker();
        let splint = Splint::shell(PathBuf::from("/tmp"));
        let splint_id = splint.id;
        let (updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, mut command_receiver) = tokio::sync::mpsc::channel(1);
        let mut view = DojoTabView::from_open(
            WindowDojoIdentity {
                topology_revision: TopologyRevision::new(1),
                lair_id: LairId::new(),
                dojo_id: DojoId::new(),
                lair_name: "hidden lair".to_owned(),
                dojo_name: "hidden dojo".to_owned(),
            },
            LayoutNode::Leaf(splint),
            vec![WindowPaneOptions {
                snapshot: valid_snapshot(splint_id),
                updates: update_receiver,
                commands,
                authority: AuthorityStatus::default(),
                controlled: false,
                image_sources: ImageContentLeaseSet::default(),
            }],
            splint_id,
            ResolvedTheme::default(),
            SCALE_DENOMINATOR,
            &RenderContext::new(u16::MAX),
        )
        .unwrap();
        view.pane.snapshot_frame = None;
        let mut revised = valid_snapshot(splint_id);
        revised.revision = 2;
        revised.visible_rows[0].cells[0].content = "hidden update".to_owned();
        updates
            .try_send(WindowUpdate::Snapshot {
                snapshot: revised,
                image_sources: ImageContentLeaseSet::default(),
                authoritative: false,
            })
            .unwrap();

        view.drain_hidden_updates(&waker, ResolvedTheme::default())
            .unwrap();

        assert_eq!(view.pane.snapshot.as_ref().unwrap().revision, 2);
        assert!(view.dirty_inactive_panes.contains(&splint_id));
        assert!(view.pane.snapshot_frame.is_none());
        assert!(command_receiver.try_recv().is_err());
    }

    #[test]
    fn hidden_tab_controller_release_covers_every_pane() {
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
        let (_first_updates, first_update_receiver) = tokio::sync::mpsc::channel(1);
        let (first_commands, mut first_command_receiver) = tokio::sync::mpsc::channel(1);
        let (_second_updates, second_update_receiver) = tokio::sync::mpsc::channel(1);
        let (second_commands, mut second_command_receiver) = tokio::sync::mpsc::channel(1);
        let mut view = DojoTabView::from_open(
            WindowDojoIdentity {
                topology_revision: TopologyRevision::new(1),
                lair_id: LairId::new(),
                dojo_id: DojoId::new(),
                lair_name: "hidden lair".to_owned(),
                dojo_name: "hidden dojo".to_owned(),
            },
            layout,
            vec![
                WindowPaneOptions {
                    snapshot: valid_snapshot(first_id),
                    updates: first_update_receiver,
                    commands: first_commands,
                    authority: AuthorityStatus::default(),
                    controlled: true,
                    image_sources: ImageContentLeaseSet::default(),
                },
                WindowPaneOptions {
                    snapshot: valid_snapshot(second_id),
                    updates: second_update_receiver,
                    commands: second_commands,
                    authority: AuthorityStatus::default(),
                    controlled: true,
                    image_sources: ImageContentLeaseSet::default(),
                },
            ],
            first_id,
            ResolvedTheme::default(),
            SCALE_DENOMINATOR,
            &RenderContext::new(u16::MAX),
        )
        .unwrap();

        App::release_tab_controllers(&mut view);

        assert!(!view.pane.controller_active);
        assert!(
            view.inactive_panes
                .iter()
                .all(|pane| !pane.controller_active)
        );
        assert_eq!(
            first_command_receiver.try_recv().unwrap(),
            WindowCommand::ReleaseControl
        );
        assert_eq!(
            second_command_receiver.try_recv().unwrap(),
            WindowCommand::ReleaseControl
        );
    }

    #[test]
    fn closed_tab_drops_renderer_image_leases() {
        let pixels = vec![1_u8, 2, 3, 255];
        let metadata = ImageContentMetadata {
            content_id: 1,
            generation: 1,
            width: 1,
            height: 1,
            source_format: ImageSourceFormat::Sixel,
            alpha_mode: ImageAlphaMode::Opaque,
            digest: Sha256::digest(&pixels).into(),
            byte_length: pixels.len(),
            retention: ImageRetention::WhilePlaced,
        };
        let cache = SharedImageContentCache::with_maximum_bytes(4).unwrap();
        cache
            .insert_source(&metadata, ImageContentSource::Buffered(Arc::from(pixels)))
            .unwrap();
        let splint = Splint::shell(PathBuf::from("/tmp"));
        let splint_id = splint.id;
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, _command_receiver) = tokio::sync::mpsc::channel(1);
        let lair_id = LairId::new();
        let dojo_id = DojoId::new();
        let view = DojoTabView::from_open(
            WindowDojoIdentity {
                topology_revision: TopologyRevision::new(1),
                lair_id,
                dojo_id,
                lair_name: "image lair".to_owned(),
                dojo_name: "image dojo".to_owned(),
            },
            LayoutNode::Leaf(splint),
            vec![WindowPaneOptions {
                snapshot: valid_snapshot(splint_id),
                updates: update_receiver,
                commands,
                authority: AuthorityStatus::default(),
                controlled: false,
                image_sources: cache.lease(std::slice::from_ref(&metadata)).unwrap(),
            }],
            splint_id,
            ResolvedTheme::default(),
            SCALE_DENOMINATOR,
            &RenderContext::new(u16::MAX),
        )
        .unwrap();
        let mut tabs = WindowTabSet::new(DojoTab::new(lair_id, dojo_id, Some(view)));
        let replacement_pixels = vec![4_u8, 5, 6, 255];
        let mut replacement = metadata.clone();
        replacement.content_id = 2;
        replacement.digest = Sha256::digest(&replacement_pixels).into();
        assert!(
            cache
                .insert_source(
                    &replacement,
                    ImageContentSource::Buffered(Arc::from(replacement_pixels.clone())),
                )
                .is_err()
        );

        drop(tabs.close(dojo_id));

        cache
            .insert_source(
                &replacement,
                ImageContentSource::Buffered(Arc::from(replacement_pixels)),
            )
            .unwrap();
        assert!(!cache.contains(&metadata).unwrap());
        assert!(cache.contains(&replacement).unwrap());
    }

    #[derive(Clone, Copy)]
    enum ReducerBenchMode {
        FocusedRole,
        InactiveBatch,
    }

    #[derive(Clone, Copy)]
    struct ReducerBenchCase {
        history_rows: usize,
        detached: bool,
        batch_size: usize,
        mode: ReducerBenchMode,
    }

    fn reducer_bench_cases(smoke: bool) -> Vec<ReducerBenchCase> {
        let history = if smoke {
            vec![(0, false), (MAX_CACHED_HISTORY_ROWS, false)]
        } else {
            vec![
                (0, false),
                (1_000, false),
                (MAX_CACHED_HISTORY_ROWS, false),
                (MAX_CACHED_HISTORY_ROWS, true),
            ]
        };
        let batches = if smoke { vec![1, 16] } else { vec![1, 16, 64] };
        history
            .into_iter()
            .flat_map(|(history_rows, detached)| {
                batches.iter().copied().flat_map(move |batch_size| {
                    [
                        ReducerBenchMode::FocusedRole,
                        ReducerBenchMode::InactiveBatch,
                    ]
                    .into_iter()
                    .map(move |mode| ReducerBenchCase {
                        history_rows,
                        detached,
                        batch_size,
                        mode,
                    })
                })
            })
            .collect()
    }

    fn reducer_bench_pane(case: ReducerBenchCase) -> PaneView {
        let splint_id = SplintId::new();
        let mut options = pane_options(splint_id);
        options.snapshot.scrollback_rows = (1..=u64::try_from(case.history_rows).unwrap())
            .map(|row_id| {
                let mut row = blank_row(1);
                row.row_id = Some(row_id);
                row
            })
            .collect();
        options.snapshot.available_scrollback_rows = case.history_rows;
        options.snapshot.oldest_available_scrollback_row_id = (case.history_rows > 0).then_some(1);
        options.snapshot.newest_available_scrollback_row_id =
            (case.history_rows > 0).then_some(u64::try_from(case.history_rows).unwrap());
        options.snapshot.visible_rows[0].row_id = Some(10_000_000);
        let mut pane = PaneView::from_options(options, SCALE_DENOMINATOR).unwrap();
        if case.detached {
            pane.scrollback_viewport
                .scroll_up(1, pane.snapshot.as_ref().unwrap());
            assert!(!pane.scrollback_viewport.is_live());
        }
        pane
    }

    fn reducer_bench_updates(snapshot: &TerminalSnapshot, batch_size: usize) -> Vec<WindowUpdate> {
        let mut projected = snapshot.clone();
        (0..batch_size)
            .map(|index| {
                let mut update = empty_update();
                update.base_revision = projected.revision;
                update.revision = projected.revision.saturating_add(1);
                let mut row = projected.visible_rows[0].clone();
                row.cells[0].content = format!("{index:02x}");
                update
                    .rows
                    .push(splinterm_protocol::TerminalRowPatch { index: 0, row });
                apply_terminal_update(&mut projected, update.clone()).unwrap();
                WindowUpdate::Update {
                    update,
                    image_sources: None,
                    trace: None,
                }
            })
            .collect()
    }

    fn measure_reducer_bench_case(case: ReducerBenchCase) -> u64 {
        let mut pane = reducer_bench_pane(case);
        let updates = reducer_bench_updates(pane.snapshot.as_ref().unwrap(), case.batch_size);
        let started = Instant::now();
        match case.mode {
            ReducerBenchMode::FocusedRole => {
                for update in updates {
                    pane.apply_background_update(update, ResolvedTheme::default(), "focused")
                        .unwrap();
                }
            }
            ReducerBenchMode::InactiveBatch => {
                apply_inactive_update_batch(&mut pane, updates, ResolvedTheme::default()).unwrap();
            }
        }
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        std::hint::black_box((
            pane.snapshot.as_ref().unwrap().revision,
            pane.scrollback_viewport.offset_from_bottom(),
        ));
        elapsed
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
    fn graphical_focus_watch_tracks_keyboard_and_selected_pane() {
        let first = SplintId::new();
        let second = SplintId::new();
        let (sender, receiver) = tokio::sync::watch::channel(None);

        update_graphical_focus_watch(Some(&sender), true, Some(first));
        assert_eq!(*receiver.borrow(), Some(first));
        update_graphical_focus_watch(Some(&sender), true, Some(second));
        assert_eq!(*receiver.borrow(), Some(second));
        update_graphical_focus_watch(Some(&sender), false, Some(second));
        assert_eq!(*receiver.borrow(), None);
        update_graphical_focus_watch(Some(&sender), true, None);
        assert_eq!(*receiver.borrow(), None);
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
    fn live_viewport_skips_previous_history_but_detached_retains_transition_rows() {
        let live_case = ReducerBenchCase {
            history_rows: MAX_CACHED_HISTORY_ROWS,
            detached: false,
            batch_size: 1,
            mode: ReducerBenchMode::FocusedRole,
        };
        let live = reducer_bench_pane(live_case);
        assert!(
            live.history_rows_needed_for_viewport_transition()
                .is_empty()
        );

        let detached = reducer_bench_pane(ReducerBenchCase {
            detached: true,
            ..live_case
        });
        let rows = detached.history_rows_needed_for_viewport_transition();
        assert_eq!(rows.len(), MAX_CACHED_HISTORY_ROWS);
        assert_eq!(rows.first().and_then(|row| row.row_id), Some(1));
        assert_eq!(
            rows.last().and_then(|row| row.row_id),
            Some(u64::try_from(MAX_CACHED_HISTORY_ROWS).unwrap())
        );
        assert_eq!(live.trace_pane_role("focused"), "focused");
        assert_eq!(detached.trace_pane_role("focused"), "detached-viewport");
    }

    #[test]
    fn pane_reducer_benchmark_contract_is_bounded_and_role_explicit() {
        let smoke = reducer_bench_cases(true);
        let full = reducer_bench_cases(false);
        assert_eq!(smoke.len(), 8);
        assert_eq!(full.len(), 24);
        assert!(full.iter().all(|case| {
            [0, 1_000, MAX_CACHED_HISTORY_ROWS].contains(&case.history_rows)
                && [1, 16, 64].contains(&case.batch_size)
        }));
        assert!(full.iter().any(|case| case.detached));
        assert!(
            full.iter()
                .any(|case| { matches!(case.mode, ReducerBenchMode::FocusedRole) })
        );
        assert!(
            full.iter()
                .any(|case| { matches!(case.mode, ReducerBenchMode::InactiveBatch) })
        );
        for case in smoke {
            assert!(measure_reducer_bench_case(case) > 0);
        }
    }

    #[test]
    #[ignore = "manual release timing harness; writes the requested JSON report"]
    fn pane_reducer_timing_harness() {
        let smoke = env::var_os("SPLINTERM_PANE_REDUCER_SMOKE").is_some();
        let warmup_runs = if smoke { 0 } else { 5 };
        let sample_runs = if smoke { 1 } else { 30 };
        let cases = reducer_bench_cases(smoke);
        let mut durations = vec![Vec::with_capacity(sample_runs); cases.len()];
        for round in 0..warmup_runs + sample_runs {
            let rotation = if cases.is_empty() {
                0
            } else {
                (round * 7) % cases.len()
            };
            for offset in 0..cases.len() {
                let index = (rotation + offset) % cases.len();
                let duration = measure_reducer_bench_case(cases[index]);
                if round >= warmup_runs {
                    durations[index].push(duration);
                }
            }
        }
        let records = cases
            .iter()
            .enumerate()
            .map(|(index, case)| {
                let mode = match case.mode {
                    ReducerBenchMode::FocusedRole => "focused-role",
                    ReducerBenchMode::InactiveBatch => "inactive-batch",
                };
                serde_json::json!({
                    "name": format!(
                        "{}-h{}-{}-b{}",
                        mode,
                        case.history_rows,
                        if case.detached { "detached" } else { "live" },
                        case.batch_size,
                    ),
                    "mode": mode,
                    "history_rows": case.history_rows,
                    "viewport": if case.detached { "detached" } else { "live" },
                    "batch_size": case.batch_size,
                    "duration_ns": durations[index],
                })
            })
            .collect::<Vec<_>>();
        let report = serde_json::json!({
            "schema": "splinterm.performance.pane-reducer.v1",
            "clock": "std::time::Instant monotonic process clock",
            "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "warmup_runs": warmup_runs,
            "sample_runs": sample_runs,
            "history_capacity_rows": MAX_CACHED_HISTORY_ROWS,
            "smoke": smoke,
            "focused_role_scope": "PaneView semantic reducer only; not the full App::apply_updates active path",
            "cases": records,
        });
        let path = env::var_os("SPLINTERM_PANE_REDUCER_REPORT").map_or_else(
            || PathBuf::from("/tmp/splinterm-pane-reducer-report.json"),
            PathBuf::from,
        );
        let temporary = path.with_file_name(format!(
            ".{}.tmp",
            path.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&temporary, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        fs::rename(temporary, path).unwrap();
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
                    trace: None,
                },
                ResolvedTheme::default(),
                "visible-inactive",
            )
            .unwrap()
            .visual_changed
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
                    trace: None,
                },
                ResolvedTheme::default(),
                "visible-inactive",
            )
            .is_err()
        );
    }

    #[test]
    fn authoritative_equal_revision_snapshot_rebuilds_a_diverged_pane() {
        let splint_id = SplintId::new();
        let mut pane = PaneView::from_options(pane_options(splint_id), SCALE_DENOMINATOR).unwrap();
        let revision = pane.snapshot.as_ref().unwrap().revision;
        let mut authoritative = pane.snapshot.as_ref().unwrap().clone();
        authoritative.visible_rows[0].cells[0].content = "authoritative-live".into();
        pane.snapshot_frame = None;

        let impact = apply_inactive_update_batch(
            &mut pane,
            [WindowUpdate::Snapshot {
                snapshot: authoritative,
                image_sources: ImageContentLeaseSet::default(),
                authoritative: true,
            }],
            ResolvedTheme::default(),
        )
        .unwrap();

        assert_eq!(impact, BackgroundUpdateImpact::FRAME);
        assert_eq!(pane.snapshot.as_ref().unwrap().revision, revision);
        assert_eq!(
            pane.snapshot.as_ref().unwrap().visible_rows[0].cells[0].content,
            "authoritative-live"
        );
        assert_eq!(
            rebuild_inactive_frames(
                std::slice::from_mut(&mut pane),
                &HashSet::from([splint_id]),
                false,
                SCALE_DENOMINATOR,
            )
            .unwrap(),
            1
        );
        assert!(pane.snapshot_frame.is_some());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one regression preserves the complete inactive-pane burst and rebuild lifecycle"
    )]
    fn inactive_pane_batch_defers_one_rebuild_until_after_contiguous_burst() {
        let splint_id = SplintId::new();
        let mut pane = PaneView::from_options(pane_options(splint_id), SCALE_DENOMINATOR).unwrap();
        let mut first = empty_update();
        let mut first_row = pane.snapshot.as_ref().unwrap().visible_rows[0].clone();
        first_row.cells[0].content = "first".into();
        first.rows.push(splinterm_protocol::TerminalRowPatch {
            index: 0,
            row: first_row,
        });
        let mut second = empty_update();
        second.base_revision = 2;
        second.revision = 3;
        let mut second_row = pane.snapshot.as_ref().unwrap().visible_rows[0].clone();
        second_row.cells[0].content = "second".into();
        second.rows.push(splinterm_protocol::TerminalRowPatch {
            index: 0,
            row: second_row,
        });
        pane.snapshot_frame = None;

        let impact = apply_inactive_update_batch(
            &mut pane,
            [
                WindowUpdate::Update {
                    update: first,
                    image_sources: None,
                    trace: Some(PerfTraceCorrelation {
                        base_revision: 1,
                        revision: 2,
                        subscription_id: 9,
                        transaction_sequence: 1,
                    }),
                },
                WindowUpdate::Update {
                    update: second,
                    image_sources: None,
                    trace: Some(PerfTraceCorrelation {
                        base_revision: 2,
                        revision: 3,
                        subscription_id: 9,
                        transaction_sequence: 2,
                    }),
                },
            ],
            ResolvedTheme::default(),
        )
        .unwrap();
        let mut metadata_only = empty_update();
        metadata_only.base_revision = 3;
        metadata_only.revision = 4;
        metadata_only.title = Some("metadata-only".into());
        assert_eq!(
            pane.apply_background_update(
                WindowUpdate::Update {
                    update: metadata_only,
                    image_sources: None,
                    trace: Some(PerfTraceCorrelation {
                        base_revision: 3,
                        revision: 4,
                        subscription_id: 9,
                        transaction_sequence: 3,
                    }),
                },
                ResolvedTheme::default(),
                "visible-inactive",
            )
            .unwrap(),
            BackgroundUpdateImpact::VISUAL
        );
        assert_eq!(impact, BackgroundUpdateImpact::FRAME);
        assert!(pane.snapshot_frame.is_none());
        assert_eq!(
            pane.trace_correlation,
            Some(PerfTraceCorrelation {
                base_revision: 2,
                revision: 3,
                subscription_id: 9,
                transaction_sequence: 2,
            })
        );
        assert_eq!(pane.trace_superseded_revisions, 1);
        let pending_commit = pane.pending_commit_trace().unwrap();
        assert_eq!(pending_commit.correlation.revision, 3);
        assert_eq!(pending_commit.pane_role, "visible-inactive");
        let commit_event = pane_commit_event(pending_commit, 7);
        assert_eq!(commit_event.revision, Some(3));
        assert_eq!(commit_event.transaction_sequence, Some(2));
        assert_eq!(commit_event.commit_sequence, Some(7));
        assert_eq!(
            rebuild_inactive_frames(
                std::slice::from_mut(&mut pane),
                &HashSet::from([splint_id]),
                false,
                SCALE_DENOMINATOR,
            )
            .unwrap(),
            1
        );
        let snapshot = pane.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.revision, 4);
        assert_eq!(snapshot.visible_rows[0].cells[0].content, "second");

        let other_id = SplintId::new();
        let mut newly_focused =
            PaneView::from_options(pane_options(other_id), SCALE_DENOMINATOR).unwrap();
        newly_focused.retain_trace_correlation(
            Some(PerfTraceCorrelation {
                base_revision: 1,
                revision: 2,
                subscription_id: 10,
                transaction_sequence: 99,
            }),
            "hidden",
        );
        std::mem::swap(&mut pane, &mut newly_focused);
        let traces = pending_pane_commit_traces(&pane, &[newly_focused]);
        assert_eq!(traces.len(), 2);
        assert!(
            traces
                .iter()
                .any(|trace| trace.correlation.transaction_sequence == 2)
        );
        assert!(
            traces
                .iter()
                .any(|trace| trace.correlation.transaction_sequence == 99)
        );
    }

    #[test]
    fn bounded_detached_display_matches_legacy_snapshot_semantics() {
        let mut initial = valid_snapshot(SplintId::new());
        initial.rows = 2;
        initial.visible_rows = vec![history_row(4, 0), history_row(5, 0)];
        initial.scrollback_rows = vec![history_row(1, 0), history_row(2, 0), history_row(3, 0)];
        initial.available_scrollback_rows = 3;
        initial.oldest_available_scrollback_row_id = Some(1);
        initial.newest_available_scrollback_row_id = Some(3);
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
            .scroll_up(2, pane.snapshot.as_ref().unwrap());

        let snapshot = pane.snapshot.as_ref().unwrap();
        let mut expected = snapshot.clone();
        let cursor_row = viewport_cursor_row(
            snapshot.cursor_row,
            pane.scrollback_viewport.offset_from_bottom(),
            snapshot.rows,
        );
        if cursor_row.is_none() {
            expected.input_modes.cursor_visible = false;
        }
        expected.cursor_column = cursor_row.map_or(-1, |_| snapshot.cursor_column);
        expected.cursor_row = cursor_row.unwrap_or(-1);
        expected.cursor_deferred_wrap = false;
        expected.visible_rows = pane
            .scrollback_viewport
            .visible_rows(snapshot)
            .into_iter()
            .cloned()
            .collect();
        expected.oldest_available_scrollback_row_id = None;
        expected.newest_available_scrollback_row_id = None;
        expected.scrollback_rows.clear();
        expected.omitted_oldest_scrollback_rows = expected.available_scrollback_rows;

        assert_eq!(pane.display_snapshot().unwrap(), expected);
        assert_eq!(snapshot.scrollback_rows.len(), 3);
    }

    #[test]
    fn inactive_detached_pane_batches_snapshot_and_theme_into_one_anchored_frame() {
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
        pane.snapshot_frame = None;
        let impact = apply_inactive_update_batch(
            &mut pane,
            [WindowUpdate::Snapshot {
                snapshot: next,
                image_sources: ImageContentLeaseSet::default(),
                authoritative: false,
            }],
            ResolvedTheme::default(),
        )
        .unwrap();
        assert_eq!(impact, BackgroundUpdateImpact::FRAME);
        assert!(pane.snapshot_frame.is_none());
        let theme = ResolvedTheme {
            background: 0x12_34_56,
            ..ResolvedTheme::default()
        };
        apply_theme(pane.snapshot.as_mut().unwrap(), theme);
        let expected_offset = pane.scrollback_viewport.offset_from_bottom();
        assert_eq!(
            rebuild_inactive_frames(
                std::slice::from_mut(&mut pane),
                &HashSet::new(),
                true,
                SCALE_DENOMINATOR,
            )
            .unwrap(),
            1
        );

        assert!(!pane.scrollback_viewport.is_live());
        assert_eq!(pane.rendered_viewport_offset, expected_offset);
        let display = pane.display_snapshot().unwrap();
        assert_eq!(display.visible_rows[0].row_id, Some(2));
        assert_ne!(display.visible_rows[0].cells[0].content, "live-four");
    }

    #[test]
    fn dirty_viewport_frame_rebuilds_after_pane_becomes_inactive() {
        let mut initial = valid_snapshot(SplintId::new());
        initial.visible_rows[0].row_id = Some(2);
        initial.scrollback_rows = vec![history_row(1, 0)];
        initial.available_scrollback_rows = 1;
        initial.oldest_available_scrollback_row_id = Some(1);
        initial.newest_available_scrollback_row_id = Some(1);
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, _command_receiver) = tokio::sync::mpsc::channel(1);
        let mut formerly_active = PaneView::from_options(
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
        formerly_active
            .scrollback_viewport
            .scroll_up(1, formerly_active.snapshot.as_ref().unwrap());
        formerly_active.viewport_dirty = true;

        let mut newly_active =
            PaneView::from_options(pane_options(SplintId::new()), SCALE_DENOMINATOR).unwrap();
        std::mem::swap(&mut formerly_active, &mut newly_active);
        assert!(newly_active.viewport_dirty);
        assert_eq!(newly_active.rendered_viewport_offset, 0);
        newly_active.scroll_started_at = Some(Instant::now());
        newly_active
            .pending_scrolls
            .push(splinterm_protocol::TerminalScroll {
                direction: splinterm_protocol::ScrollDirection::Reverse,
                start_row: 0,
                end_row: 1,
                rows: 1,
            });

        assert!(rebuild_dirty_pane_viewport_frame(&mut newly_active, SCALE_DENOMINATOR).unwrap());
        assert!(!newly_active.viewport_dirty);
        assert_eq!(newly_active.rendered_viewport_offset, 1);
        assert!(newly_active.scroll_started_at.is_none());
        assert!(newly_active.pending_scrolls.is_empty());

        newly_active.scrollback_viewport.return_to_live();
        newly_active.viewport_dirty = true;
        assert!(rebuild_dirty_pane_viewport_frame(&mut newly_active, SCALE_DENOMINATOR).unwrap());
        assert_eq!(newly_active.rendered_viewport_offset, 0);
        assert!(newly_active.scrollback_viewport.is_live());
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
    fn uncontrolled_opened_tab_claims_control_for_every_active_resize() {
        let splint = Splint::shell(PathBuf::from("/tmp"));
        let splint_id = splint.id;
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, mut command_receiver) = tokio::sync::mpsc::channel(2);
        let view = DojoTabView::from_open(
            WindowDojoIdentity {
                topology_revision: TopologyRevision::new(1),
                lair_id: LairId::new(),
                dojo_id: DojoId::new(),
                lair_name: "test lair".to_owned(),
                dojo_name: "test dojo".to_owned(),
            },
            LayoutNode::Leaf(splint),
            vec![WindowPaneOptions {
                snapshot: valid_snapshot(splint_id),
                updates: update_receiver,
                commands,
                authority: AuthorityStatus::default(),
                controlled: false,
                image_sources: ImageContentLeaseSet::default(),
            }],
            splint_id,
            ResolvedTheme::default(),
            SCALE_DENOMINATOR,
            &RenderContext::new(u16::MAX),
        )
        .unwrap();
        let mut pane = view.pane;

        App::emit_active_pane_resize(&mut pane, 320, 240, SCALE_DENOMINATOR).unwrap();
        assert!(matches!(
            command_receiver.try_recv().unwrap(),
            WindowCommand::Resize { .. }
        ));

        App::emit_active_pane_resize(&mut pane, 640, 480, SCALE_DENOMINATOR).unwrap();
        assert!(matches!(
            command_receiver.try_recv().unwrap(),
            WindowCommand::Resize { .. }
        ));
    }

    #[test]
    fn controlled_inactive_pane_applies_resize_after_layout_change() {
        let splint_id = SplintId::new();
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, mut command_receiver) = tokio::sync::mpsc::channel(1);
        let mut pane = PaneView::from_options(
            WindowPaneOptions {
                snapshot: valid_snapshot(splint_id),
                updates: update_receiver,
                commands,
                authority: AuthorityStatus::default(),
                controlled: true,
                image_sources: ImageContentLeaseSet::default(),
            },
            SCALE_DENOMINATOR,
        )
        .unwrap();

        App::emit_inactive_pane_resize(&mut pane, 320, 240, SCALE_DENOMINATOR).unwrap();
        assert!(matches!(
            command_receiver.try_recv().unwrap(),
            WindowCommand::Resize { .. }
        ));
    }

    #[test]
    fn uncontrolled_visible_inactive_pane_claims_control_for_every_resize() {
        let splint_id = SplintId::new();
        let (_updates, update_receiver) = tokio::sync::mpsc::channel(1);
        let (commands, mut command_receiver) = tokio::sync::mpsc::channel(2);
        let mut pane = PaneView::from_inactive_options(
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

        App::emit_inactive_pane_resize(&mut pane, 320, 240, SCALE_DENOMINATOR).unwrap();
        assert!(matches!(
            command_receiver.try_recv().unwrap(),
            WindowCommand::Resize { .. }
        ));

        App::emit_inactive_pane_resize(&mut pane, 640, 480, SCALE_DENOMINATOR).unwrap();
        assert!(matches!(
            command_receiver.try_recv().unwrap(),
            WindowCommand::Resize { .. }
        ));
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
    fn bounded_history_replace_applies_visible_marker_without_resync() {
        let mut current = snapshot(SplintId::new(), 7, 10);
        current.columns = 1;
        current.rows = 1;
        current.visible_rows = vec![blank_row(1)];
        current.scrollback_rows = (1..=4).map(|id| history_row(id, 0)).collect();
        current.available_scrollback_rows = 4;
        current.oldest_available_scrollback_row_id = Some(1);
        current.newest_available_scrollback_row_id = Some(4);
        let marker = TerminalRow {
            row_id: Some(21),
            linebreak: true,
            cells: vec![TerminalCell {
                content: "SPLINTERBENCH_DONE".into(),
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
                    row: marker.clone(),
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
                scrollback: Some(splinterm_protocol::TerminalScrollbackUpdate {
                    transition: HistoryTransition::Replace,
                    history_generation: 1,
                    oldest_available_row_id: Some(1),
                    newest_available_row_id: Some(20),
                    rows: vec![history_row(19, 0), history_row(20, 0)],
                    available_rows: 20,
                    omitted_oldest_rows: 18,
                }),
                images: None,
            },
        )
        .expect("bounded replace update");

        assert_eq!(current.revision, 11);
        assert_eq!(current.visible_rows[0], marker);
        assert_eq!(current.newest_available_scrollback_row_id, Some(20));
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
    fn semantic_scroll_survives_return_to_live_frame_rebuild() {
        let splint_id = SplintId::new();
        let mut initial = valid_snapshot(splint_id);
        initial.rows = 4;
        initial.scrollback_rows = vec![history_row(1, 0)];
        initial.available_scrollback_rows = 1;
        initial.oldest_available_scrollback_row_id = Some(1);
        initial.newest_available_scrollback_row_id = Some(1);
        initial.visible_rows = (2..=5)
            .zip(["a", "b", "c", "d"])
            .map(|(row_id, content)| {
                let mut row = history_row(row_id, 0);
                row.cells[0].content = content.into();
                row
            })
            .collect();
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
        let mut exposed = history_row(6, 0);
        exposed.cells[0].content = "e".into();

        apply_terminal_update(
            pane.snapshot.as_mut().unwrap(),
            TerminalUpdate {
                base_revision: 1,
                revision: 2,
                rows: vec![splinterm_protocol::TerminalRowPatch {
                    index: 3,
                    row: exposed,
                }],
                scrolls: vec![splinterm_protocol::TerminalScroll {
                    direction: splinterm_protocol::ScrollDirection::Forward,
                    start_row: 0,
                    end_row: 4,
                    rows: 1,
                }],
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
            },
        )
        .unwrap();
        pane.scrollback_viewport
            .scroll_up(1, pane.snapshot.as_ref().unwrap());
        pane.viewport_dirty = true;
        assert!(rebuild_dirty_pane_viewport_frame(&mut pane, SCALE_DENOMINATOR).unwrap());

        pane.scrollback_viewport.return_to_live();
        pane.viewport_dirty = true;
        assert!(rebuild_dirty_pane_viewport_frame(&mut pane, SCALE_DENOMINATOR).unwrap());

        assert!(pane.scrollback_viewport.is_live());
        assert_eq!(pane.rendered_viewport_offset, 0);
        let live = pane.display_snapshot().unwrap();
        assert_eq!(
            live.visible_rows
                .iter()
                .map(|row| row.cells[0].content.as_str())
                .collect::<Vec<_>>(),
            ["b", "c", "d", "e"]
        );
    }

    #[test]
    fn snapshot_order_accepts_only_newer_matching_identity() {
        let splint_id = SplintId::new();
        let current = snapshot(splint_id, 7, 10);
        assert!(snapshot_is_newer(&current, &snapshot(splint_id, 7, 11)).expect("matching"));
        let equal = snapshot(splint_id, 7, 10);
        assert!(!snapshot_is_newer(&current, &equal).expect("duplicate"));
        assert!(!snapshot_replaces(&current, &equal, false).expect("ordinary duplicate"));
        assert!(snapshot_replaces(&current, &equal, true).expect("authoritative duplicate"));
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
    #[allow(
        clippy::too_many_lines,
        reason = "one regression compares every local overlay invalidation class"
    )]
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
        let current_rows = valid_snapshot(SplintId::new());
        let mut repeated = empty_update();
        repeated.rows.push(splinterm_protocol::TerminalRowPatch {
            index: 0,
            row: current_rows.visible_rows[0].clone(),
        });
        assert!(changed_terminal_patch_rows(&repeated, &current_rows).is_empty());
        repeated.rows[0].row.row_id = Some(999);
        assert!(changed_terminal_patch_rows(&repeated, &current_rows).is_empty());
        repeated.rows[0].row.cells[0].content = "changed".into();
        assert_eq!(changed_terminal_patch_rows(&repeated, &current_rows), [0]);

        let mut scroll = empty_update();
        scroll.scrolls.push(splinterm_protocol::TerminalScroll {
            direction: splinterm_protocol::ScrollDirection::Forward,
            start_row: 0,
            end_row: 2,
            rows: 1,
        });
        assert!(terminal_update_changes_visible_content(&scroll));
        let mut current = snapshot(SplintId::new(), 1, 1);
        current.columns = 1;
        current.rows = 2;
        current.visible_rows.resize_with(2, || blank_row(1));
        current.visible_rows[0].cells[0].content = "top".into();
        current.visible_rows[1].cells[0].content = "bottom".into();
        let mut projected_scroll = scroll.clone();
        projected_scroll.rows = vec![
            splinterm_protocol::TerminalRowPatch {
                index: 0,
                row: current.visible_rows[1].clone(),
            },
            splinterm_protocol::TerminalRowPatch {
                index: 1,
                row: blank_row(1),
            },
        ];
        assert!(changed_terminal_patch_rows(&projected_scroll, &current).is_empty());
        projected_scroll.rows[0].row.cells[0].content = "changed after scroll".into();
        assert_eq!(
            changed_terminal_patch_rows(&projected_scroll, &current),
            [0]
        );
        projected_scroll.scrolls[0].rows = 0;
        assert_eq!(
            changed_terminal_patch_rows(&projected_scroll, &current),
            [0, 1]
        );
        projected_scroll.scrolls[0].rows = 1;
        projected_scroll.scrolls[0].end_row = projected_scroll.scrolls[0].start_row;
        assert_eq!(
            changed_terminal_patch_rows(&projected_scroll, &current),
            [0, 1]
        );
        let mut repeated_metadata = empty_update();
        repeated_metadata.columns = Some(current.columns);
        repeated_metadata.row_count = Some(current.rows);
        let mut repeated_palette = current.palette.clone();
        repeated_palette[0] ^= 0x00ff_ffff;
        repeated_metadata.palette = Some(repeated_palette);
        repeated_metadata.default_colors = Some([1, 2, 3]);
        repeated_metadata.active_screen = Some(current.active_screen);
        assert!(!terminal_update_requires_full_frame(
            &repeated_metadata,
            &current
        ));
        repeated_metadata.palette.as_mut().unwrap()[16] ^= 0x00ff_ffff;
        assert!(terminal_update_requires_full_frame(
            &repeated_metadata,
            &current
        ));
        repeated_metadata.palette = Some(current.palette.clone());
        assert!(!terminal_update_requires_full_frame(&scroll, &current));
        current.images = Some(Box::new(splinterm_protocol::TerminalImagePlane {
            screen: ActiveScreen::Normal,
            contents: Vec::new(),
            placements: Vec::new(),
        }));
        assert!(!terminal_update_requires_full_frame(&scroll, &current));
        current
            .images
            .as_mut()
            .unwrap()
            .placements
            .push(splinterm_protocol::ImagePlacement {
                placement_id: 1,
                content_id: 1,
                row_id: 1,
                column: 0,
                source: splinterm_protocol::ImagePixelRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                destination_columns: 1,
                destination_rows: 1,
                source_cell_size: None,
                x_offset: 0,
                y_offset: 0,
                z_index: 0,
                application_image_id: None,
                application_placement_id: None,
                creation_order: 1,
                erase_policy: splinterm_protocol::ImageErasePolicy::TextOverwrite,
            });
        assert!(terminal_update_requires_full_frame(&scroll, &current));
        let mut identity_update = empty_update();
        let mut identity_row = current.visible_rows[0].clone();
        identity_row.row_id = Some(999);
        identity_update
            .rows
            .push(splinterm_protocol::TerminalRowPatch {
                index: 0,
                row: identity_row,
            });
        assert_eq!(changed_terminal_patch_rows(&identity_update, &current), [0]);
        current.images = None;
        let mut resize_with_scroll = scroll.clone();
        resize_with_scroll.columns = Some(80);
        resize_with_scroll.row_count = Some(24);
        assert!(terminal_update_requires_full_frame(
            &resize_with_scroll,
            &current
        ));

        let mut screen = empty_update();
        screen.active_screen = Some(ActiveScreen::Normal);
        assert!(!terminal_update_requires_full_frame(&screen, &current));
        screen.active_screen = Some(ActiveScreen::Alternate);
        assert!(terminal_update_requires_full_frame(&screen, &current));

        let mut image_update = empty_update();
        image_update.images = Some(Box::new(splinterm_protocol::TerminalImagePlane {
            screen: ActiveScreen::Normal,
            contents: Vec::new(),
            placements: Vec::new(),
        }));
        assert!(terminal_update_requires_full_frame(&image_update, &current));
        current.images.clone_from(&image_update.images);
        assert!(!terminal_update_requires_full_frame(
            &image_update,
            &current
        ));
        let mut colors = empty_update();
        colors.default_colors = Some([1, 2, 3]);
        assert!(terminal_update_changes_visible_content(&colors));

        assert!(terminal_update_has_visual_damage(
            false,
            false,
            &[false, false],
            &[false, true]
        ));
        assert!(!terminal_update_has_visual_damage(
            false,
            false,
            &[false, false],
            &[false, false]
        ));
    }

    #[test]
    fn raster_damage_follows_pixels_copied_by_queued_scrolls() {
        let forward = splinterm_protocol::TerminalScroll {
            direction: splinterm_protocol::ScrollDirection::Forward,
            start_row: 0,
            end_row: 6,
            rows: 1,
        };
        let mut dirty = [false; 6];
        dirty[5] = true;
        propagate_raster_damage_through_scroll(&mut dirty, &forward);
        assert_eq!(dirty, [false, false, false, false, true, true]);

        // A second coalesced scroll moves the still-unpainted backing pixels again.
        propagate_raster_damage_through_scroll(&mut dirty, &forward);
        assert_eq!(dirty, [false, false, false, true, true, true]);

        let reverse = splinterm_protocol::TerminalScroll {
            direction: splinterm_protocol::ScrollDirection::Reverse,
            start_row: 0,
            end_row: 6,
            rows: 2,
        };
        let mut dirty = [true, false, false, false, false, false];
        propagate_raster_damage_through_scroll(&mut dirty, &reverse);
        assert_eq!(dirty, [true, false, true, false, false, false]);
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
    fn picker_activation_requires_matching_press_and_release_targets() {
        assert_eq!(
            picker_release_activation(Some(PickerHitTarget::New), Some(PickerHitTarget::New)),
            Some(PickerHitTarget::New)
        );
        assert_eq!(
            picker_release_activation(Some(PickerHitTarget::Open(1)), None),
            None
        );
        assert_eq!(
            picker_release_activation(
                Some(PickerHitTarget::Open(1)),
                Some(PickerHitTarget::Open(2))
            ),
            None
        );
    }

    #[test]
    fn modal_input_generations_reject_stale_clipboard_and_reconcile_focus_once() {
        assert!(clipboard_read_is_current(false, 4, 4));
        assert!(!clipboard_read_is_current(true, 4, 4));
        assert!(!clipboard_read_is_current(false, 5, 4));
        assert_eq!(
            reconciled_focus_report(true, false, true),
            Some(b"\x1b[I".to_vec())
        );
        assert_eq!(
            reconciled_focus_report(true, true, false),
            Some(b"\x1b[O".to_vec())
        );
        assert_eq!(reconciled_focus_report(true, true, true), None);
        assert_eq!(reconciled_focus_report(false, false, true), None);
        assert_eq!(
            picker_ime_reconcile(true, true, true),
            PickerImeReconcile::Renew
        );
        assert_eq!(
            picker_ime_reconcile(false, true, true),
            PickerImeReconcile::Enable
        );
        assert_eq!(
            picker_ime_reconcile(false, false, true),
            PickerImeReconcile::None
        );
    }

    #[test]
    fn theme_updates_coalesce_by_shared_generation() {
        let mut pending = None;
        let theme = ResolvedTheme::default();
        retain_newest_theme(
            &mut pending,
            ThemeUpdate {
                generation: 8,
                theme,
            },
        );
        retain_newest_theme(
            &mut pending,
            ThemeUpdate {
                generation: 3,
                theme,
            },
        );
        retain_newest_theme(
            &mut pending,
            ThemeUpdate {
                generation: 13,
                theme,
            },
        );
        assert_eq!(pending.map(|update| update.generation), Some(13));
    }

    #[test]
    fn return_to_live_queues_one_authoritative_resynchronization() {
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);

        assert!(!request_return_live_resync(Some(&commands), 0, 0).unwrap());
        assert!(!request_return_live_resync(Some(&commands), 4, 2).unwrap());
        assert!(request_return_live_resync(Some(&commands), 2, 0).unwrap());
        assert_eq!(receiver.try_recv().unwrap(), WindowCommand::Resynchronize);
    }

    #[test]
    fn bounded_command_queue_reports_overflow_and_disconnect() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        try_window_command(&sender, WindowCommand::Input(vec![1])).expect("first command");
        let error = try_window_command(&sender, WindowCommand::Input(vec![2]))
            .expect_err("bounded overflow");
        assert!(error.to_string().contains("overflow"));
        assert_eq!(
            try_queue_control_release(&sender),
            ControlReleaseOutcome::Retry
        );
        assert_eq!(
            try_queue_focus_report(&sender, false),
            ControlReleaseOutcome::Retry
        );
        assert!(receiver.try_recv().is_ok());
        assert_eq!(
            try_queue_focus_report(&sender, false),
            ControlReleaseOutcome::Queued
        );
        assert_eq!(
            receiver.try_recv().unwrap(),
            WindowCommand::Input(b"\x1b[O".to_vec())
        );
        assert_eq!(
            try_queue_control_release(&sender),
            ControlReleaseOutcome::Queued
        );
        assert_eq!(receiver.try_recv().unwrap(), WindowCommand::ReleaseControl);
        drop(receiver);
        let error = try_window_command(&sender, WindowCommand::Input(vec![3]))
            .expect_err("disconnected receiver");
        assert!(error.to_string().contains("disconnected"));
        assert_eq!(
            try_queue_control_release(&sender),
            ControlReleaseOutcome::Disconnected
        );
        assert_eq!(
            try_queue_focus_report(&sender, true),
            ControlReleaseOutcome::Disconnected
        );

        let (topology_sender, mut topology_receiver) = tokio::sync::mpsc::channel(1);
        try_topology_command(
            &topology_sender,
            WindowTopologyCommand::RequestSessionPicker,
        )
        .expect("first topology command");
        let error = try_topology_command(
            &topology_sender,
            WindowTopologyCommand::NewLair { cwd: "/tmp".into() },
        )
        .expect_err("bounded topology overflow");
        assert!(error.to_string().contains("full"));
        assert!(topology_receiver.try_recv().is_ok());
        drop(topology_receiver);
        let error = try_topology_command(
            &topology_sender,
            WindowTopologyCommand::RequestSessionPicker,
        )
        .expect_err("disconnected topology receiver");
        assert!(error.to_string().contains("closed"));

        let target = SplintId::new();
        let pending = SplintId::new();
        let pending_command = || WindowTopologyCommand::Split {
            dojo_id: DojoId::new(),
            target,
            axis: Axis::Horizontal,
            pending: Some(pending),
        };
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        try_topology_command(
            &sender,
            WindowTopologyCommand::NewLair { cwd: "/tmp".into() },
        )
        .unwrap();
        let mut rolled_back = None;
        let error = try_topology_command_with_rollback(
            Some(&sender),
            pending_command(),
            |rollback_target, rollback_pending| {
                rolled_back = Some((rollback_target, rollback_pending));
                Ok(())
            },
        )
        .expect_err("full pending split queue");
        assert!(error.to_string().contains("full"));
        assert_eq!(rolled_back, Some((target, pending)));

        assert!(receiver.try_recv().is_ok());
        drop(receiver);
        rolled_back = None;
        let error = try_topology_command_with_rollback(
            Some(&sender),
            pending_command(),
            |rollback_target, rollback_pending| {
                rolled_back = Some((rollback_target, rollback_pending));
                Ok(())
            },
        )
        .expect_err("closed pending split queue");
        assert!(error.to_string().contains("closed"));
        assert_eq!(rolled_back, Some((target, pending)));
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
        ime.set_preedit(Some("modal".into()));
        ime.clear_composition();
        assert!(!ime.composing());
        assert!(ime.entered);
        assert!(ime.focused);
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
        assert!(presented_cursor_visible(true, false));
        assert!(!presented_cursor_visible(false, false));
    }

    #[test]
    fn event_loop_timeout_is_event_driven_except_for_active_deadlines() {
        assert_eq!(
            event_loop_timeout(false, false, None),
            Some(IDLE_EVENT_LOOP_TIMEOUT)
        );
        assert_eq!(
            event_loop_timeout(false, false, Some(Duration::from_millis(125))),
            Some(Duration::from_millis(375))
        );
        assert_eq!(
            event_loop_timeout(false, false, Some(Duration::from_secs(1))),
            Some(Duration::ZERO)
        );
        assert_eq!(
            event_loop_timeout(false, true, Some(Duration::ZERO)),
            Some(SIGNOFF_TICK_INTERVAL)
        );
        assert_eq!(event_loop_timeout(true, false, None), None);
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
