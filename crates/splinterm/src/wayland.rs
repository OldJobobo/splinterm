//! Native Wayland xdg-shell and shared-memory lifecycle for the graphical client.
//!
//! Foot 1.27.0 `wayland.c`, `shm.c`, and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` are the behavioral reference.
//! The client owns these objects; the daemon remains headless.

use std::{
    collections::HashMap,
    io,
    os::fd::{AsFd, OwnedFd},
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self as std_mpsc, Receiver as StdReceiver, Sender as StdSender},
    },
    time::{Duration, Instant},
};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use tokio::sync::mpsc::{Receiver, Sender, error::TryRecvError, error::TrySendError};
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
        calloop::{EventLoop, LoopHandle},
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

use splinterm_protocol::{
    CellAttributes, ColorSource, MouseTracking, TerminalCell, TerminalInputModes, TerminalRow,
    TerminalSnapshot, TerminalUpdate,
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

use crate::renderer::{
    SnapshotFrame, SnapshotOverlays, TextRow, paint, paint_snapshot, paint_snapshot_overlays,
    paint_snapshot_rows, scroll_snapshot_pixels, snapshot_row_rect, write_ppm,
};

const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 600;
const APP_ID: &str = "com.oldjobobo.splinterm";
const TEXT_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const MAX_CLIPBOARD_WORKERS: usize = 4;
const CLIPBOARD_IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_WHEEL_STEPS_PER_FRAME: usize = 8;
const MAX_BUFFERED_WHEEL_STEPS: f64 = 64.0;
const WHEEL_VALUE120_STEP: f64 = 120.0;
const WHEEL_PIXEL_STEP: f64 = 10.0;
const SCALE_DENOMINATOR: u32 = 120;
const MIN_SCALE_120: u32 = 120;
const MAX_SCALE_120: u32 = 960;
const MAX_PREEDIT_BYTES: usize = 4 * 1024;
const BTN_RIGHT: u32 = 0x111;
static ACTIVE_CLIPBOARD_WORKERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct CellPosition {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Selection {
    anchor: CellPosition,
    end: CellPosition,
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
    Snapshot(TerminalSnapshot),
    Update(TerminalUpdate),
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
}

#[derive(Default)]
pub struct WindowOptions {
    pub capture: Option<PathBuf>,
    /// Initial owned daemon snapshot. `None` retains the deterministic evidence row.
    pub snapshot: Option<TerminalSnapshot>,
    /// Bounded live-update receiver owned by the Wayland thread.
    pub updates: Option<Receiver<WindowUpdate>>,
    /// Bounded command sender from the Wayland thread to the async protocol owner.
    pub commands: Option<Sender<WindowCommand>>,
    /// Retain Q/Escape close shortcuts only for the renderer evidence example.
    pub evidence_close_shortcuts: bool,
    /// Delay capture until this integer output scale is active.
    pub capture_scale: Option<u32>,
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
pub fn run(options: WindowOptions) -> Result<()> {
    let text_row = options
        .snapshot
        .is_none()
        .then(|| TextRow::load(1))
        .transpose()?;
    let snapshot_frame = options
        .snapshot
        .as_ref()
        .map(|snapshot| SnapshotFrame::load(snapshot, 1))
        .transpose()?;
    let connection = Connection::connect_to_env().context("connect to Wayland compositor")?;
    let (globals, event_queue) =
        registry_queue_init(&connection).context("read Wayland registry")?;
    let queue_handle = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().context("create event loop")?;
    WaylandSource::new(connection.clone(), event_queue)
        .insert(event_loop.handle())
        .context("register Wayland source")?;

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
        let (width, height) = viewport_destination(INITIAL_WIDTH, INITIAL_HEIGHT)?;
        viewport.set_destination(width, height);
    }
    let controller_active = options.commands.is_some();
    let title = window_title(
        options
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.title.as_str()),
        controller_active,
    );
    window.set_title(title);
    window.set_app_id(APP_ID);
    window.set_min_size(Some((480, 300)));
    window
        .set_buffer_scale(1)
        .map_err(|_| anyhow::anyhow!("compositor does not support integer buffer scale"))?;
    window.commit();

    let pool_size = usize::try_from(INITIAL_WIDTH * INITIAL_HEIGHT * 4)
        .context("initial SHM pool size fits usize")?;
    let pool = SlotPool::new(pool_size, &shm).context("create SHM pool")?;
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
        snapshot: options.snapshot,
        snapshot_frame,
        updates: options.updates,
        commands: options.commands,
        controller_active,
        evidence_close_shortcuts: options.evidence_close_shortcuts,
        modifiers: Modifiers::default(),
        last_resize: None,
        capture: options.capture,
        capture_scale: options.capture_scale,
        buffer: None,
        backing: Vec::new(),
        prepare_dirty_rows: Vec::new(),
        raster_dirty_rows: Vec::new(),
        surface_dirty_rows: Vec::new(),
        pending_scrolls: Vec::new(),
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
        selected_text: None,
        selection: None,
        selecting: false,
        pointer_cell: None,
        hovered_url: None,
        last_pointer_serial: None,
        pressed_buttons: HashMap::new(),
        vertical_wheel: WheelAccumulator::default(),
        loop_handle: event_loop.handle(),
        logical_width: INITIAL_WIDTH,
        logical_height: INITIAL_HEIGHT,
        configured: false,
        exit: false,
        failure: None,
        frame_pending: false,
        redraw_pending: false,
        cursor_blink_visible: true,
        last_cursor_blink: Instant::now(),
        scale_120: SCALE_DENOMINATOR,
        integer_fallback_scale: 1,
        output_count: 0,
        seat_count: 0,
    };

    while !app.exit {
        app.apply_updates(&queue_handle)?;
        app.apply_clipboard_reads()?;
        app.tick_cursor_blink(&queue_handle)?;
        event_loop
            .dispatch(Duration::from_millis(16), &mut app)
            .context("dispatch Wayland events")?;
    }
    if let Some(error) = app.failure {
        return Err(error);
    }
    Ok(())
}

fn viewport_destination(width: u32, height: u32) -> Result<(i32, i32)> {
    Ok((
        i32::try_from(width).context("viewport width fits i32")?,
        i32::try_from(height).context("viewport height fits i32")?,
    ))
}

fn scaled_dimension(logical: u32, scale_120: u32) -> Result<u32> {
    if !(MIN_SCALE_120..=MAX_SCALE_120).contains(&scale_120) {
        anyhow::bail!("scale must be between 1x and 8x");
    }
    logical
        .checked_mul(scale_120)
        .and_then(|value| value.checked_add(SCALE_DENOMINATOR - 1))
        .map(|value| value / SCALE_DENOMINATOR)
        .context("scaled dimension overflow")
}

fn buffer_dimensions(
    logical_width: u32,
    logical_height: u32,
    scale_120: u32,
) -> Result<(u32, u32, i32)> {
    let width = scaled_dimension(logical_width, scale_120)?;
    let height = scaled_dimension(logical_height, scale_120)?;
    let stride = i32::try_from(width.checked_mul(4).context("buffer stride overflow")?)
        .context("buffer stride fits i32")?;
    i32::try_from(width).context("buffer width fits i32")?;
    i32::try_from(height).context("buffer height fits i32")?;
    Ok((width, height, stride))
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
    snapshot: Option<TerminalSnapshot>,
    snapshot_frame: Option<SnapshotFrame>,
    updates: Option<Receiver<WindowUpdate>>,
    commands: Option<Sender<WindowCommand>>,
    controller_active: bool,
    evidence_close_shortcuts: bool,
    modifiers: Modifiers,
    last_resize: Option<(u16, u16, u16, u16)>,
    capture: Option<PathBuf>,
    capture_scale: Option<u32>,
    buffer: Option<Buffer>,
    backing: Vec<u8>,
    prepare_dirty_rows: Vec<bool>,
    raster_dirty_rows: Vec<bool>,
    surface_dirty_rows: Vec<bool>,
    pending_scrolls: Vec<splinterm_protocol::TerminalScroll>,
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
    selected_text: Option<Vec<u8>>,
    selection: Option<Selection>,
    selecting: bool,
    pointer_cell: Option<CellPosition>,
    hovered_url: Option<(CellPosition, CellPosition, String)>,
    last_pointer_serial: Option<u32>,
    pressed_buttons: HashMap<u32, PressOwner>,
    vertical_wheel: WheelAccumulator,
    loop_handle: LoopHandle<'static, App>,
    logical_width: u32,
    logical_height: u32,
    configured: bool,
    exit: bool,
    failure: Option<anyhow::Error>,
    frame_pending: bool,
    redraw_pending: bool,
    cursor_blink_visible: bool,
    last_cursor_blink: Instant,
    scale_120: u32,
    integer_fallback_scale: u32,
    output_count: usize,
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
        linebreak: false,
        cells: vec![
            TerminalCell {
                content: String::new(),
                spacer_remaining: None,
                attributes: CellAttributes {
                    bold: false,
                    dim: false,
                    italic: false,
                    underline: false,
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

fn terminal_update_changes_visible_content(update: &TerminalUpdate) -> bool {
    !update.rows.is_empty()
        || !update.scrolls.is_empty()
        || update.columns.is_some()
        || update.row_count.is_some()
        || update.palette.is_some()
        || update.default_colors.is_some()
        || update.active_screen.is_some()
}

fn apply_terminal_update(snapshot: &mut TerminalSnapshot, update: TerminalUpdate) -> Result<()> {
    update
        .validate_against(snapshot.revision, snapshot.columns, snapshot.rows)
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
        _ if modes.application_keypad => {
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

fn selection_bounds(selection: Selection) -> (CellPosition, CellPosition) {
    if selection.anchor <= selection.end {
        (selection.anchor, selection.end)
    } else {
        (selection.end, selection.anchor)
    }
}

fn selection_text(snapshot: &TerminalSnapshot, selection: Selection) -> String {
    let (start, end) = selection_bounds(selection);
    let mut output = String::new();
    for row_index in start.row..=end.row.min(snapshot.rows.saturating_sub(1)) {
        let Some(row) = snapshot.visible_rows.get(row_index) else {
            continue;
        };
        let first = if row_index == start.row {
            start.column
        } else {
            0
        };
        let last = if row_index == end.row {
            end.column
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
        if row_index != end.row {
            output.push('\n');
        }
    }
    output
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
        reason = "the finite accumulated value and emitted count are clamped to eight wheel steps"
    )]
    fn push(
        &mut self,
        absolute: f64,
        discrete: i32,
        value120: i32,
    ) -> Option<(MouseAction, usize)> {
        let (unit, delta, threshold) = if value120 != 0 {
            (
                WheelUnit::Value120,
                f64::from(value120),
                WHEEL_VALUE120_STEP,
            )
        } else if discrete != 0 {
            (WheelUnit::Discrete, f64::from(discrete), 1.0)
        } else if absolute != 0.0 && absolute.is_finite() {
            (WheelUnit::Pixel, absolute, WHEEL_PIXEL_STEP)
        } else {
            return None;
        };
        if self.unit != Some(unit) {
            self.unit = Some(unit);
            self.remainder = 0.0;
        }
        let limit = threshold * MAX_BUFFERED_WHEEL_STEPS;
        self.remainder = (self.remainder + delta).clamp(-limit, limit);
        let available = (self.remainder.abs() / threshold).floor() as usize;
        let count = available.min(MAX_WHEEL_STEPS_PER_FRAME);
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

fn cursor_blink_enabled(reduced_motion: bool, focused: bool, modes: TerminalInputModes) -> bool {
    !reduced_motion && focused && modes.cursor_visible && modes.cursor_blink
}

fn resize_changed(previous: Option<(u16, u16, u16, u16)>, candidate: (u16, u16, u16, u16)) -> bool {
    previous != Some(candidate)
}

fn reduced_motion_requested() -> bool {
    std::env::var_os("SPLINTERM_REDUCED_MOTION")
        .is_some_and(|value| matches!(value.to_str(), Some("1" | "true" | "yes")))
}

fn window_title(snapshot_title: Option<&str>, controller_active: bool) -> String {
    let base = snapshot_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Splinterm");
    if controller_active {
        format!("{base} — local controller")
    } else {
        base.to_owned()
    }
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

impl App {
    fn send_input(&mut self, bytes: Vec<u8>) -> Result<()> {
        if let Some(commands) = &self.commands {
            try_window_command(commands, WindowCommand::Input(bytes))?;
        }
        Ok(())
    }

    fn pointer_cell_at(&self, position: (f64, f64)) -> Option<CellPosition> {
        let (row, column) =
            self.snapshot_frame
                .as_ref()?
                .cell_at(position.0, position.1, self.scale_120)?;
        Some(CellPosition { row, column })
    }

    fn dirty_row(&mut self, row: usize) {
        let rows = self.snapshot.as_ref().map_or(0, |snapshot| snapshot.rows);
        self.raster_dirty_rows.resize(rows, false);
        self.surface_dirty_rows.resize(rows, false);
        if row < rows {
            self.raster_dirty_rows[row] = true;
            self.surface_dirty_rows[row] = true;
        }
    }

    fn dirty_selection(&mut self, selection: Option<Selection>) {
        if let Some(selection) = selection {
            let (start, end) = selection_bounds(selection);
            for row in start.row..=end.row {
                self.dirty_row(row);
            }
        }
    }

    fn invalidate_local_content_state(&mut self) {
        self.dirty_selection(self.selection);
        if let Some((start, _, _)) = &self.hovered_url {
            self.dirty_row(start.row);
        }
        self.selection = None;
        self.selected_text = None;
        self.selecting = false;
        self.hovered_url = None;
        self.pressed_buttons
            .retain(|_, owner| matches!(owner, PressOwner::Application { .. }));
    }

    fn recompute_hovered_url(&mut self) {
        let previous = self.hovered_url.take();
        self.hovered_url = self.pointer_cell.and_then(|position| {
            self.snapshot
                .as_ref()
                .and_then(|snapshot| url_at(snapshot, position))
        });
        if previous != self.hovered_url {
            if let Some((start, _, _)) = previous {
                self.dirty_row(start.row);
            }
            if let Some((start, _, _)) = &self.hovered_url {
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
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.input_modes.bracketed_paste);
            self.send_input(encode_bracketed_paste(bytes, bracketed))?;
            let _ = read.target;
        }
        Ok(())
    }

    fn publish_clipboard(&mut self, qh: &QueueHandle<Self>, serial: u32, primary: bool) {
        let Some(text) = self.selected_text.as_ref() else {
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
        let Some((_, _, url)) = &self.hovered_url else {
            return;
        };
        let _ = Command::new("xdg-open").arg(url).spawn();
    }

    fn fail(&mut self, error: anyhow::Error) {
        eprintln!("Wayland client failure: {error:#}");
        self.failure = Some(error);
        self.exit = true;
    }

    fn send_command(&mut self, command: WindowCommand) {
        let Some(commands) = &self.commands else {
            return;
        };
        if let Err(error) = try_window_command(commands, command) {
            self.fail(error);
        }
    }

    fn send_coalescible_input(&mut self, bytes: Vec<u8>) {
        let Some(commands) = &self.commands else {
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

    fn update_ime_cursor_rectangle(&mut self) {
        if !self.ime.entered || !self.ime.focused {
            return;
        }
        let (Some(text_input), Some(frame)) = (&self.text_input, &self.snapshot_frame) else {
            return;
        };
        if let Some((x, y, width, height)) = frame.cursor_rectangle(self.scale_120) {
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
        if let Some(frame) = &self.snapshot_frame {
            if let Some((x, y, width, height)) = frame.cursor_rectangle(self.scale_120) {
                text_input.set_cursor_rectangle(x, y, width, height);
            }
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
        let (Some(snapshot), Some(frame)) = (&self.snapshot, &mut self.snapshot_frame) else {
            return Ok(());
        };
        let Ok(row) = usize::try_from(snapshot.cursor_row) else {
            return Ok(());
        };
        if row >= snapshot.rows {
            return Ok(());
        }
        let mut render_snapshot = snapshot.clone();
        if let Some(text) = self.ime.visible_preedit.as_deref() {
            let mut column = usize::try_from(snapshot.cursor_column.max(0)).unwrap_or(0);
            let mut leader: Option<usize> = None;
            for character in text.chars() {
                let width = UnicodeWidthChar::width(character).unwrap_or(1).min(2);
                if width == 0 {
                    if let Some(leader) = leader {
                        if let Some(cell) = render_snapshot.visible_rows[row].cells.get_mut(leader)
                        {
                            cell.content.push(character);
                        }
                    }
                    continue;
                }
                if column >= render_snapshot.columns || column + width > render_snapshot.columns {
                    break;
                }
                if let Some(cell) = render_snapshot.visible_rows[row].cells.get_mut(column) {
                    cell.content = character.to_string();
                    cell.spacer_remaining = None;
                }
                leader = Some(column);
                if width == 2 {
                    if let Some(spacer) =
                        render_snapshot.visible_rows[row].cells.get_mut(column + 1)
                    {
                        spacer.content.clear();
                        spacer.spacer_remaining = Some(1);
                    }
                }
                column += width;
            }
        }
        let mut dirty = vec![false; snapshot.rows];
        dirty[row] = true;
        frame.refresh_rows(&render_snapshot, &dirty)?;
        self.raster_dirty_rows.resize(snapshot.rows, false);
        self.surface_dirty_rows.resize(snapshot.rows, false);
        self.raster_dirty_rows[row] = true;
        self.surface_dirty_rows[row] = true;
        Ok(())
    }

    fn input_modes(&self) -> TerminalInputModes {
        self.snapshot.as_ref().map_or(
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

    fn handle_key(&mut self, event: &KeyEvent) {
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
        let (Some(frame), Some(_)) = (&self.snapshot_frame, &self.commands) else {
            return Ok(());
        };
        let resize =
            frame.terminal_size(self.logical_width, self.logical_height, self.scale_120)?;
        if !resize_changed(self.last_resize, resize) {
            return Ok(());
        }
        self.send_command(WindowCommand::Resize {
            columns: resize.0,
            rows: resize.1,
            pixel_width: resize.2,
            pixel_height: resize.3,
        });
        if !self.exit {
            self.last_resize = Some(resize);
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "bounded update draining and semantic damage coalescing stay adjacent"
    )]
    fn apply_updates(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let mut pending = Vec::new();
        let mut disconnected = false;
        if let Some(updates) = &mut self.updates {
            loop {
                match updates.try_recv() {
                    Ok(update) => pending.push(update),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.exit = true;
            return Ok(());
        }
        let mut visual_changed = false;
        let mut title_changed = false;
        let mut full_frame_reload = false;
        for update in pending {
            match update {
                WindowUpdate::Snapshot(snapshot) => {
                    if self
                        .snapshot
                        .as_ref()
                        .is_none_or(|current| snapshot_is_newer(current, &snapshot).unwrap_or(true))
                    {
                        self.invalidate_local_content_state();
                        self.snapshot = Some(snapshot);
                        self.full_redraw = true;
                        full_frame_reload = true;
                        visual_changed = true;
                        title_changed = true;
                    }
                }
                WindowUpdate::Update(update) => {
                    let old_cursor_row = self.snapshot.as_ref().and_then(|snapshot| {
                        usize::try_from(snapshot.cursor_row)
                            .ok()
                            .filter(|row| *row < snapshot.rows)
                    });
                    let patched_rows: Vec<_> =
                        update.rows.iter().map(|patch| patch.index).collect();
                    let scrolls = update.scrolls.clone();
                    let full = update.columns.is_some()
                        || update.row_count.is_some()
                        || update.palette.is_some()
                        || update.default_colors.is_some()
                        || update.active_screen.is_some();
                    let content_changed = terminal_update_changes_visible_content(&update);
                    let cursor_changed = update.cursor.is_some() || update.input_modes.is_some();
                    title_changed |= update.title.is_some();
                    let snapshot = self
                        .snapshot
                        .as_mut()
                        .context("terminal update arrived before initial snapshot")?;
                    apply_terminal_update(snapshot, update)?;
                    if content_changed {
                        self.invalidate_local_content_state();
                    }
                    let snapshot = self
                        .snapshot
                        .as_ref()
                        .context("updated terminal snapshot exists")?;
                    let rows = snapshot.rows;
                    self.prepare_dirty_rows.resize(rows, false);
                    self.raster_dirty_rows.resize(rows, false);
                    self.surface_dirty_rows.resize(rows, false);
                    if full {
                        self.full_redraw = true;
                        full_frame_reload = true;
                    } else {
                        for scroll in &scrolls {
                            for row in scroll.start_row..scroll.end_row.min(rows) {
                                // Rebuilding the bounded semantic scroll region keeps prepared
                                // row geometry correct while pixel movement still uses scroll-copy.
                                self.prepare_dirty_rows[row] = true;
                                self.surface_dirty_rows[row] = true;
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
                                self.raster_dirty_rows[row] = true;
                            }
                        }
                        for row in patched_rows.into_iter().filter(|row| *row < rows) {
                            self.prepare_dirty_rows[row] = true;
                            let copied = scrolls
                                .iter()
                                .any(|scroll| row >= scroll.start_row && row < scroll.end_row);
                            if !copied {
                                self.raster_dirty_rows[row] = true;
                                self.surface_dirty_rows[row] = true;
                            }
                        }
                        if cursor_changed {
                            if let Some(row) = old_cursor_row {
                                self.raster_dirty_rows[row] = true;
                                self.surface_dirty_rows[row] = true;
                            }
                            if let Ok(row) = usize::try_from(snapshot.cursor_row) {
                                if row < rows {
                                    self.raster_dirty_rows[row] = true;
                                    self.surface_dirty_rows[row] = true;
                                }
                            }
                        }
                        self.pending_scrolls.extend(scrolls);
                    }
                    visual_changed |=
                        full || cursor_changed || self.raster_dirty_rows.iter().any(|dirty| *dirty);
                }
                WindowUpdate::Shutdown => {
                    self.exit = true;
                    return Ok(());
                }
            }
        }
        if visual_changed {
            self.cursor_blink_visible = true;
            self.last_cursor_blink = Instant::now();
            let snapshot = self.snapshot.as_ref().context("updated snapshot exists")?;
            if full_frame_reload || self.snapshot_frame.is_none() {
                self.snapshot_frame = Some(SnapshotFrame::load_scaled(snapshot, self.scale_120)?);
            } else if let Some(frame) = &mut self.snapshot_frame {
                frame.refresh_rows(snapshot, &self.prepare_dirty_rows)?;
                frame.refresh_cursor(snapshot);
            }
            self.prepare_dirty_rows.fill(false);
            self.refresh_ime_preedit()?;
            self.update_ime_cursor_rectangle();
            self.emit_resize()?;
            if self.configured {
                self.schedule_draw(queue_handle)?;
            }
        }
        if title_changed {
            let snapshot = self.snapshot.as_ref().context("updated snapshot exists")?;
            self.window
                .set_title(window_title(Some(&snapshot.title), self.controller_active));
        }
        Ok(())
    }

    fn apply_scale(&mut self, scale_120: u32, queue_handle: &QueueHandle<Self>) -> Result<()> {
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
        if let Some(snapshot) = &self.snapshot {
            self.snapshot_frame = Some(SnapshotFrame::load_scaled(snapshot, scale_120)?);
        } else {
            self.text_row = Some(TextRow::load(scale_120.div_ceil(SCALE_DENOMINATOR))?);
        }
        self.scale_120 = scale_120;
        self.buffer = None;
        self.backing.clear();
        self.full_redraw = true;
        self.last_resize = None;
        self.refresh_ime_preedit()?;
        self.emit_resize()?;
        self.update_ime_cursor_rectangle();
        if self.configured {
            self.schedule_draw(queue_handle)?;
        }
        Ok(())
    }

    fn tick_cursor_blink(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let blinking = self.snapshot.as_ref().is_some_and(|snapshot| {
            cursor_blink_enabled(
                self.reduced_motion,
                self.keyboard_focused,
                snapshot.input_modes,
            )
        });
        if blinking && self.last_cursor_blink.elapsed() >= Duration::from_millis(500) {
            self.cursor_blink_visible = !self.cursor_blink_visible;
            self.last_cursor_blink = Instant::now();
            if let Some(snapshot) = &self.snapshot {
                if let Ok(row) = usize::try_from(snapshot.cursor_row) {
                    if row < snapshot.rows {
                        self.raster_dirty_rows.resize(snapshot.rows, false);
                        self.surface_dirty_rows.resize(snapshot.rows, false);
                        self.raster_dirty_rows[row] = true;
                        self.surface_dirty_rows[row] = true;
                    }
                }
            }
            if self.configured {
                self.schedule_draw(queue_handle)?;
            }
        } else if !blinking && !self.cursor_blink_visible {
            self.cursor_blink_visible = true;
            if let Some(snapshot) = &self.snapshot {
                if let Ok(row) = usize::try_from(snapshot.cursor_row) {
                    if row < snapshot.rows {
                        self.raster_dirty_rows.resize(snapshot.rows, false);
                        self.surface_dirty_rows.resize(snapshot.rows, false);
                        self.raster_dirty_rows[row] = true;
                        self.surface_dirty_rows[row] = true;
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

    #[allow(
        clippy::too_many_lines,
        reason = "SHM acquisition, persistent backing updates, damage submission, and commit form one transaction"
    )]
    fn draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        self.redraw_pending = false;
        let (width, height, stride) = buffer_dimensions(
            self.logical_width.max(1),
            self.logical_height.max(1),
            self.scale_120,
        )?;
        let width_i32 = i32::try_from(width).context("buffer width fits i32")?;
        let height_i32 = i32::try_from(height).context("buffer height fits i32")?;
        let buffer = if let Some(buffer) = self.buffer.as_mut() {
            buffer
        } else {
            let buffer = self
                .pool
                .create_buffer(width_i32, height_i32, stride, wl_shm::Format::Argb8888)
                .context("create SHM buffer")?
                .0;
            self.buffer.insert(buffer)
        };
        let canvas = if let Some(canvas) = self.pool.canvas(buffer) {
            canvas
        } else {
            let (next, canvas) = self
                .pool
                .create_buffer(width_i32, height_i32, stride, wl_shm::Format::Argb8888)
                .context("create replacement SHM buffer")?;
            *buffer = next;
            canvas
        };

        println!(
            "Presenting logical={}x{} buffer={}x{} scale={}x stride={stride}",
            self.logical_width,
            self.logical_height,
            width,
            height,
            f64::from(self.scale_120) / 120.0
        );
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
        if let Some(frame) = &self.snapshot_frame {
            if self.full_redraw {
                paint_snapshot(
                    &mut self.backing,
                    width,
                    height,
                    frame,
                    self.cursor_blink_visible,
                );
            } else {
                for scroll in self.pending_scrolls.drain(..) {
                    scroll_snapshot_pixels(&mut self.backing, width, frame, scroll);
                }
                paint_snapshot_rows(
                    &mut self.backing,
                    width,
                    height,
                    frame,
                    &self.raster_dirty_rows,
                    self.cursor_blink_visible,
                );
            }
            let selection = self
                .selection
                .map(selection_bounds)
                .map(|(start, end)| ((start.row, start.column), (end.row, end.column)));
            let hovered_url = self
                .hovered_url
                .as_ref()
                .map(|(start, end, _)| ((start.row, start.column), (end.row, end.column)));
            canvas.copy_from_slice(&self.backing);
            paint_snapshot_overlays(
                canvas,
                width,
                height,
                frame,
                SnapshotOverlays {
                    selection,
                    hovered_url,
                    dirty_rows: None,
                    focused: self.keyboard_focused,
                },
            );
        } else if let Some(row) = &self.text_row {
            paint(canvas, width, height, row);
        } else {
            anyhow::bail!("window has no prepared renderer content");
        }
        let capture_ready = self
            .capture_scale
            .is_none_or(|expected| expected.saturating_mul(120) == self.scale_120);
        if capture_ready {
            if let Some(path) = self.capture.take() {
                write_ppm(&path, canvas, width, height)
                    .with_context(|| format!("write {}", path.display()))?;
                println!(
                    "Wrote deterministic row capture at {}x scale to {}",
                    f64::from(self.scale_120) / 120.0,
                    path.display()
                );
            }
        }
        if self.snapshot_frame.is_none() || self.full_redraw {
            self.window
                .wl_surface()
                .damage_buffer(0, 0, width_i32, height_i32);
        } else if let Some(frame) = &self.snapshot_frame {
            for (row, dirty) in self.surface_dirty_rows.iter().copied().enumerate() {
                if !dirty {
                    continue;
                }
                if let Some((x, y, row_width, row_height)) = snapshot_row_rect(frame, row) {
                    self.window
                        .wl_surface()
                        .damage_buffer(x, y, row_width, row_height);
                }
            }
        }
        self.full_redraw = false;
        self.raster_dirty_rows.fill(false);
        self.surface_dirty_rows.fill(false);
        self.pending_scrolls.clear();
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
        _queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        if surface == self.window.wl_surface() {
            self.output_count += 1;
        }
    }

    fn surface_leave(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        if surface == self.window.wl_surface() {
            self.output_count = self.output_count.saturating_sub(1);
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
        self.exit = true;
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
            self.buffer = None;
            self.full_redraw = true;
            self.update_ime_cursor_rectangle();
        }
        let initial_configure = !self.configured;
        self.configured = true;
        if initial_configure || resized {
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
            self.pointer_cell = None;
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
            self.handle_key(&event);
        }
    }

    fn repeat_key(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(&event);
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
            let previous_hover = self.hovered_url.clone();
            let cell = self.pointer_cell_at(event.position);
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.last_pointer_serial = Some(serial);
                    self.pointer_cell = cell;
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_cell = None;
                    self.hovered_url = None;
                }
                PointerEventKind::Motion { .. } => {
                    self.pointer_cell = cell;
                    self.hovered_url = cell.and_then(|position| {
                        self.snapshot
                            .as_ref()
                            .and_then(|snapshot| url_at(snapshot, position))
                    });
                    if self.selecting {
                        if let (Some(mut selection), Some(position)) = (self.selection, cell) {
                            self.dirty_selection(Some(selection));
                            selection.end = position;
                            self.selection = Some(selection);
                            self.dirty_selection(Some(selection));
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
                    self.pointer_cell = cell;
                    self.recompute_hovered_url();
                    let owner = classify_press(
                        button,
                        cell.is_some(),
                        self.modifiers,
                        self.input_modes(),
                        self.hovered_url.is_some(),
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
                                self.dirty_selection(self.selection);
                                let selection = Selection {
                                    anchor: position,
                                    end: position,
                                };
                                self.selection = Some(selection);
                                self.selecting = true;
                                self.dirty_selection(Some(selection));
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
                            if let Some(position) = cell.or(self.pointer_cell) {
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
                            self.selecting = false;
                            self.selected_text = self.selection.and_then(|selection| {
                                self.snapshot.as_ref().map(|snapshot| {
                                    selection_text(snapshot, selection).into_bytes()
                                })
                            });
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
                    let Some(position) = cell else { continue };
                    let modes = self.input_modes();
                    if modes.mouse_tracking == MouseTracking::None {
                        self.vertical_wheel = WheelAccumulator::default();
                        continue;
                    }
                    if vertical.is_none() {
                        continue;
                    }
                    if let Some((action, count)) = self.vertical_wheel.push(
                        vertical.absolute,
                        vertical.discrete,
                        vertical.value120,
                    ) {
                        if let Some(report) =
                            mouse_report(action, position, self.modifiers, modes.sgr_mouse)
                        {
                            let mut batch = Vec::with_capacity(report.len().saturating_mul(count));
                            for _ in 0..count {
                                batch.extend_from_slice(&report);
                            }
                            self.send_command(WindowCommand::Input(batch));
                        }
                    }
                }
            }
            if previous_hover != self.hovered_url {
                if let Some((start, _, _)) = previous_hover {
                    self.dirty_row(start.row);
                }
                if let Some((start, _, _)) = &self.hovered_url {
                    self.dirty_row(start.row);
                }
            }
        }
        if self.configured
            && (self.raster_dirty_rows.iter().any(|dirty| *dirty)
                || self.surface_dirty_rows.iter().any(|dirty| *dirty))
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
        _queue_handle: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
    fn output_destroyed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
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
    use super::*;
    use splinterm_core::SplintId;
    use splinterm_protocol::ActiveScreen;

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
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            exited_code: None,
            exited_signal: None,
        }
    }

    #[test]
    fn semantic_update_applies_exact_row_cursor_and_title_revision() {
        let mut current = snapshot(SplintId::new(), 7, 10);
        current.columns = 2;
        current.rows = 1;
        current.visible_rows = vec![blank_row(2)];
        let row = TerminalRow {
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
            },
        )
        .expect("contiguous semantic update");
        assert_eq!(current.revision, 11);
        assert_eq!(current.visible_rows[0], row);
        assert_eq!((current.cursor_column, current.cursor_row), (1, 0));
        assert!(current.cursor_deferred_wrap);
        assert_eq!(current.title, "revision eleven");
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
        }
    }

    fn encoded(keysym: Keysym, utf8: Option<&str>, modifiers: Modifiers) -> Option<Vec<u8>> {
        key_input(keysym, utf8, modifiers, normal_modes())
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
    fn selection_text_orders_endpoints_and_skips_wide_spacers() {
        let mut view = snapshot(SplintId::new(), 1, 1);
        view.columns = 4;
        view.rows = 2;
        view.visible_rows = vec![blank_row(4), blank_row(4)];
        view.visible_rows[0].cells[0].content = "A".to_owned();
        view.visible_rows[0].cells[1].content = "界".to_owned();
        view.visible_rows[0].cells[2].spacer_remaining = Some(0);
        view.visible_rows[0].cells[3].content = " ".to_owned();
        view.visible_rows[1].cells[0].content = "B".to_owned();
        view.visible_rows[1].cells[1].content = "C".to_owned();
        let selection = Selection {
            anchor: CellPosition { row: 1, column: 1 },
            end: CellPosition { row: 0, column: 0 },
        };
        assert_eq!(selection_text(&view, selection), "A界\nBC");
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
    fn axis_accumulates_partial_steps_preserves_remainder_and_caps_frames() {
        let mut wheel = WheelAccumulator::default();
        assert_eq!(wheel.push(0.0, 0, -60), None);
        assert_eq!(wheel.push(0.0, 0, -59), None);
        assert_eq!(wheel.push(0.0, 0, -1), Some((MouseAction::WheelUp, 1)));
        assert_eq!(wheel.push(0.0, 0, 119), None);
        assert_eq!(wheel.push(0.0, 0, 1), Some((MouseAction::WheelDown, 1)));

        assert_eq!(
            wheel.push(0.0, 20, 0),
            Some((MouseAction::WheelDown, MAX_WHEEL_STEPS_PER_FRAME))
        );
        assert_eq!(
            wheel.push(0.0, 0, 0),
            None,
            "zero frames do not flush a different source implicitly"
        );
        assert_eq!(
            wheel.push(0.0, 1, 0),
            Some((MouseAction::WheelDown, MAX_WHEEL_STEPS_PER_FRAME))
        );

        assert_eq!(wheel.push(-4.0, 0, 0), None);
        assert_eq!(wheel.push(-6.0, 0, 0), Some((MouseAction::WheelUp, 1)));
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
