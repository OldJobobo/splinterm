//! Native Wayland/SHM lifecycle spike for Roadmap Phase 2.
//!
//! Foot 1.27.0 `wayland.c`, `shm.c`, and `render.c` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e` are the behavioral reference.
//! This spike validates the selected Rust mechanism before terminal rendering.

use std::time::Duration;

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

const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 600;
const APP_ID: &str = "com.oldjobobo.splinterm.Spike";

fn main() {
    let connection = Connection::connect_to_env().expect("connect to Wayland compositor");
    let (globals, event_queue) = registry_queue_init(&connection).expect("read Wayland registry");
    let queue_handle = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().expect("create event loop");
    WaylandSource::new(connection.clone(), event_queue)
        .insert(event_loop.handle())
        .expect("register Wayland source");

    let compositor = CompositorState::bind(&globals, &queue_handle)
        .expect("compositor does not provide wl_compositor");
    let shell =
        XdgShell::bind(&globals, &queue_handle).expect("compositor does not provide xdg-shell");
    let shm = Shm::bind(&globals, &queue_handle).expect("compositor does not provide wl_shm");
    let surface = compositor.create_surface(&queue_handle);
    let window = shell.create_window(surface, WindowDecorations::RequestServer, &queue_handle);
    window.set_title("Splinterm - Native Wayland Spike");
    window.set_app_id(APP_ID);
    window.set_min_size(Some((480, 300)));
    window.commit();

    let pool = SlotPool::new((INITIAL_WIDTH * INITIAL_HEIGHT * 4) as usize, &shm)
        .expect("create SHM pool");
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &queue_handle),
        output_state: OutputState::new(&globals, &queue_handle),
        shm,
        window,
        pool,
        buffer: None,
        keyboard: None,
        loop_handle: event_loop.handle(),
        width: INITIAL_WIDTH,
        height: INITIAL_HEIGHT,
        configured: false,
        exit: false,
        frame_count: 0,
        scale_factor: 1,
        output_count: 0,
        seat_count: 0,
    };

    while !app.exit {
        event_loop
            .dispatch(Duration::from_millis(250), &mut app)
            .expect("dispatch Wayland events");
    }
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    window: Window,
    pool: SlotPool,
    buffer: Option<Buffer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    loop_handle: LoopHandle<'static, App>,
    width: u32,
    height: u32,
    configured: bool,
    exit: bool,
    frame_count: u32,
    scale_factor: i32,
    output_count: usize,
    seat_count: usize,
}

impl App {
    fn draw(&mut self, queue_handle: &QueueHandle<Self>) {
        let width = self.width.max(1);
        let height = self.height.max(1);
        let stride = i32::try_from(width * 4).expect("window width fits i32");
        let buffer = self.buffer.get_or_insert_with(|| {
            self.pool
                .create_buffer(
                    i32::try_from(width).expect("width fits i32"),
                    i32::try_from(height).expect("height fits i32"),
                    stride,
                    wl_shm::Format::Argb8888,
                )
                .expect("create SHM buffer")
                .0
        });
        let canvas = if let Some(canvas) = self.pool.canvas(buffer) {
            canvas
        } else {
            let (next, canvas) = self
                .pool
                .create_buffer(
                    i32::try_from(width).expect("width fits i32"),
                    i32::try_from(height).expect("height fits i32"),
                    stride,
                    wl_shm::Format::Argb8888,
                )
                .expect("create replacement SHM buffer");
            *buffer = next;
            canvas
        };

        paint(canvas, width, height, self.frame_count);
        self.frame_count = self.frame_count.wrapping_add(1);
        self.window.wl_surface().damage_buffer(
            0,
            0,
            i32::try_from(width).expect("width fits i32"),
            i32::try_from(height).expect("height fits i32"),
        );
        self.window
            .wl_surface()
            .frame(queue_handle, self.window.wl_surface().clone());
        buffer
            .attach_to(self.window.wl_surface())
            .expect("attach SHM buffer");
        self.window.commit();
    }
}

fn paint(canvas: &mut [u8], width: u32, height: u32, frame: u32) {
    let pulse = u8::try_from((frame / 2) % 24).expect("pulse fits u8");
    for (index, pixel) in canvas.chunks_exact_mut(4).enumerate() {
        let x = u32::try_from(index).expect("index fits u32") % width;
        let y = u32::try_from(index).expect("index fits u32") / width;
        let header = y < height / 6;
        let cell_width = (width / 12).max(1);
        let cell_height = (height / 8).max(1);
        let grid_line = x % cell_width < 2 || y % cell_height < 2;
        let accent = x < width / 80 + 8;
        let (red, green, blue) = if header {
            (20, 40 + pulse, 52 + pulse)
        } else if accent {
            (45, 190, 170)
        } else if grid_line {
            (24, 42, 48)
        } else {
            (10, 18, 22)
        };
        pixel.copy_from_slice(&[blue, green, red, 0xff]);
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if surface == self.window.wl_surface() {
            self.scale_factor = factor;
            self.buffer = None;
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
            self.draw(queue_handle);
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
            .map_or(self.width, std::num::NonZeroU32::get);
        let height = configure
            .new_size
            .1
            .map_or(self.height, std::num::NonZeroU32::get);
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.buffer = None;
        }
        if !self.configured {
            self.configured = true;
            self.draw(queue_handle);
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
            self.keyboard = Some(
                self.seat_state
                    .get_keyboard_with_repeat(
                        queue_handle,
                        &seat,
                        None,
                        self.loop_handle.clone(),
                        Box::new(|_, _, _| {}),
                    )
                    .expect("create keyboard"),
            );
        }
    }

    fn remove_capability(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
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
