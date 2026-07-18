//! Native Wayland xdg-shell and shared-memory lifecycle for the graphical client.
//!
//! Foot 1.27.0 `wayland.c`, `shm.c`, and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` are the behavioral reference.
//! The client owns these objects; the daemon remains headless.

use std::{path::PathBuf, time::Duration};

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

use crate::renderer::{TextRow, paint, write_ppm};

const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 600;
const APP_ID: &str = "com.oldjobobo.splinterm";

#[derive(Clone, Debug, Default)]
pub struct WindowOptions {
    pub capture: Option<PathBuf>,
    /// Delay capture until this integer output scale is active.
    pub capture_scale: Option<u32>,
}

/// Opens the 1x deterministic evidence window and runs until compositor close or Q/Escape.
///
/// # Errors
///
/// Returns an error when font setup, required Wayland globals, shared-memory buffers,
/// keyboard state, capture output, or event dispatch cannot be initialized.
pub fn run(options: WindowOptions) -> Result<()> {
    let text_row = TextRow::load(1)?;
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
    window.set_title("Splinterm — Renderer Preview");
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
        capture: options.capture,
        capture_scale: options.capture_scale,
        buffer: None,
        keyboard: None,
        keyboard_seat: None,
        loop_handle: event_loop.handle(),
        logical_width: INITIAL_WIDTH,
        logical_height: INITIAL_HEIGHT,
        configured: false,
        exit: false,
        failure: None,
        frame_pending: false,
        integer_scale: 1,
        output_count: 0,
        seat_count: 0,
    };

    while !app.exit {
        event_loop
            .dispatch(Duration::from_millis(250), &mut app)
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

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    window: Window,
    pool: SlotPool,
    text_row: TextRow,
    capture: Option<PathBuf>,
    capture_scale: Option<u32>,
    buffer: Option<Buffer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keyboard_seat: Option<wl_seat::WlSeat>,
    loop_handle: LoopHandle<'static, App>,
    logical_width: u32,
    logical_height: u32,
    configured: bool,
    exit: bool,
    failure: Option<anyhow::Error>,
    frame_pending: bool,
    integer_scale: u32,
    output_count: usize,
    seat_count: usize,
}

impl App {
    fn fail(&mut self, error: anyhow::Error) {
        self.failure = Some(error);
        self.exit = true;
    }

    fn draw(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
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
        paint(canvas, width, height, &self.text_row);
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
        self.window
            .wl_surface()
            .damage_buffer(0, 0, width_i32, height_i32);
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
            self.text_row = TextRow::load(integer_scale)?;
            self.integer_scale = integer_scale;
            self.buffer = None;
            if self.configured {
                self.draw(queue_handle)?;
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
        _queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if surface == self.window.wl_surface() {
            self.frame_pending = false;
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
        }
        let initial_configure = !self.configured;
        self.configured = true;
        if initial_configure || resized {
            if let Err(error) = self.draw(queue_handle) {
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
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if matches!(event.keysym, Keysym::Escape | Keysym::q | Keysym::Q) {
            self.exit = true;
        }
    }

    fn repeat_key(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
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
        _modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        _layout: u32,
    ) {
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
