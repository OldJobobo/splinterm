//! Native Wayland xdg-shell and shared-memory lifecycle for the graphical client.
//!
//! Foot 1.27.0 `wayland.c`, `shm.c`, and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` are the behavioral reference.
//! The client owns these objects; the daemon remains headless.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use tokio::sync::mpsc::{Receiver, Sender, error::TryRecvError, error::TrySendError};

use anyhow::{Context, Result};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_registry, delegate_seat,
    delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{EventLoop, LoopHandle},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
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
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
};

use splinterm_protocol::{
    CellAttributes, ColorSource, TerminalCell, TerminalInputModes, TerminalRow, TerminalSnapshot,
    TerminalUpdate,
};

use crate::renderer::{
    SnapshotFrame, TextRow, paint, paint_snapshot, paint_snapshot_rows, scroll_snapshot_pixels,
    snapshot_row_rect, write_ppm,
};

const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 600;
const APP_ID: &str = "com.oldjobobo.splinterm";

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
    let surface = compositor.create_surface(&queue_handle);
    let window = shell.create_window(surface, WindowDecorations::RequestServer, &queue_handle);
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
        integer_scale: 1,
        output_count: 0,
        seat_count: 0,
    };

    while !app.exit {
        app.apply_updates(&queue_handle)?;
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

fn buffer_dimensions(
    logical_width: u32,
    logical_height: u32,
    integer_scale: u32,
) -> Result<(u32, u32, i32)> {
    if integer_scale == 0 {
        anyhow::bail!("integer scale must be positive");
    }
    let width = logical_width
        .checked_mul(integer_scale)
        .context("scaled buffer width overflow")?;
    let height = logical_height
        .checked_mul(integer_scale)
        .context("scaled buffer height overflow")?;
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
    integer_scale: u32,
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

fn try_window_command(commands: &Sender<WindowCommand>, command: WindowCommand) -> Result<()> {
    commands.try_send(command).map_err(|error| match error {
        TrySendError::Full(_) => anyhow::anyhow!("Wayland command queue overflow"),
        TrySendError::Closed(_) => anyhow::anyhow!("Wayland command receiver disconnected"),
    })
}

fn resize_changed(previous: Option<(u16, u16, u16, u16)>, candidate: (u16, u16, u16, u16)) -> bool {
    previous != Some(candidate)
}

fn window_title(snapshot_title: Option<&str>, controller_active: bool) -> String {
    let base = snapshot_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Splinterm");
    if controller_active {
        format!("{base} — controller")
    } else {
        base.to_owned()
    }
}

impl App {
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

    fn input_modes(&self) -> TerminalInputModes {
        self.snapshot.as_ref().map_or(
            TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: true,
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
        if let Some(bytes) = key_input(
            event.keysym,
            event.utf8.as_deref(),
            self.modifiers,
            self.input_modes(),
        ) {
            self.send_command(WindowCommand::Input(bytes));
        }
    }

    fn emit_resize(&mut self) -> Result<()> {
        let (Some(frame), Some(_)) = (&self.snapshot_frame, &self.commands) else {
            return Ok(());
        };
        let resize =
            frame.terminal_size(self.logical_width, self.logical_height, self.integer_scale)?;
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
                    let cursor_changed = update.cursor.is_some() || update.input_modes.is_some();
                    title_changed |= update.title.is_some();
                    let snapshot = self
                        .snapshot
                        .as_mut()
                        .context("terminal update arrived before initial snapshot")?;
                    apply_terminal_update(snapshot, update)?;
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
                self.snapshot_frame = Some(SnapshotFrame::load(snapshot, self.integer_scale)?);
            } else if let Some(frame) = &mut self.snapshot_frame {
                frame.refresh_rows(snapshot, &self.prepare_dirty_rows)?;
                frame.refresh_cursor(snapshot);
            }
            self.prepare_dirty_rows.fill(false);
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

    fn tick_cursor_blink(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let blinking = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.input_modes.cursor_visible && snapshot.input_modes.cursor_blink
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
        } else if !blinking {
            self.cursor_blink_visible = true;
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
            self.integer_scale,
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
            self.logical_width, self.logical_height, width, height, self.integer_scale
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
            canvas.copy_from_slice(&self.backing);
        } else if let Some(row) = &self.text_row {
            paint(canvas, width, height, row);
        } else {
            anyhow::bail!("window has no prepared renderer content");
        }
        let capture_ready = self
            .capture_scale
            .is_none_or(|expected| expected == self.integer_scale);
        if capture_ready {
            if let Some(path) = self.capture.take() {
                write_ppm(&path, canvas, width, height)
                    .with_context(|| format!("write {}", path.display()))?;
                println!(
                    "Wrote deterministic row capture at {}x scale to {}",
                    self.integer_scale,
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
        if integer_scale == self.integer_scale {
            return;
        }
        let result = (|| -> Result<()> {
            self.window
                .set_buffer_scale(integer_scale)
                .map_err(|_| anyhow::anyhow!("compositor does not support integer buffer scale"))?;
            if let Some(snapshot) = &self.snapshot {
                self.snapshot_frame = Some(SnapshotFrame::load(snapshot, integer_scale)?);
            } else {
                self.text_row = Some(TextRow::load(integer_scale)?);
            }
            self.integer_scale = integer_scale;
            self.buffer = None;
            self.full_redraw = true;
            self.emit_resize()?;
            if self.configured {
                self.schedule_draw(queue_handle)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
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
            self.buffer = None;
            self.full_redraw = true;
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
                    self.keyboard_seat = Some(seat);
                }
                Err(error) => self.fail(anyhow::anyhow!("create keyboard: {error}")),
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
        }
        self.seat_count = self.seat_count.saturating_sub(1);
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        if surface == self.window.wl_surface() && self.input_modes().focus_reporting {
            self.send_command(WindowCommand::Input(b"\x1b[I".to_vec()));
        }
    }

    fn leave(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        if surface == self.window.wl_surface() && self.input_modes().focus_reporting {
            self.send_command(WindowCommand::Input(b"\x1b[O".to_vec()));
        }
    }

    fn press_key(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(&event);
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
    fn duplicate_resize_is_suppressed() {
        let size = (80, 24, 1_120, 720);
        assert!(resize_changed(None, size));
        assert!(!resize_changed(Some(size), size));
        assert!(resize_changed(Some(size), (81, 24, 1_134, 720)));
    }

    #[test]
    fn buffer_dimensions_scale_logical_size_and_stride() {
        assert_eq!(
            buffer_dimensions(960, 600, 1).expect("1x"),
            (960, 600, 3_840)
        );
        assert_eq!(
            buffer_dimensions(960, 600, 2).expect("2x"),
            (1_920, 1_200, 7_680)
        );
    }

    #[test]
    fn buffer_dimensions_reject_zero_scale_and_overflow() {
        assert!(buffer_dimensions(960, 600, 0).is_err());
        assert!(buffer_dimensions(u32::MAX, 1, 2).is_err());
        assert!(buffer_dimensions(1, u32::MAX, 2).is_err());
        assert!(buffer_dimensions(i32::MAX as u32 / 4 + 1, 1, 1).is_err());
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}
