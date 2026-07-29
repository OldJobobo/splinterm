//! Slice 0 probe for `ext-background-effect-v1`.
//!
//! The executable binds only the manager and prints its advertised capabilities;
//! it creates no surface or window. `compile_request_signatures` is deliberately
//! not called: its purpose is to keep the generated request signatures checked by
//! `cargo check --example background_effect_protocol_spike`.

use anyhow::{Context, Result, bail};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry::WlRegistry,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{
        Capability, Event as ManagerEvent, ExtBackgroundEffectManagerV1,
    },
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};

#[derive(Default)]
struct ProbeState {
    capability_flags: Option<u32>,
}

impl Dispatch<WlRegistry, GlobalListContents> for ProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: wayland_client::protocol::wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for ProbeState {
    fn event(
        state: &mut Self,
        _proxy: &ExtBackgroundEffectManagerV1,
        event: ManagerEvent,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        if let ManagerEvent::Capabilities { flags } = event {
            state.capability_flags = Some(match flags {
                WEnum::Value(flags) => flags.bits(),
                WEnum::Unknown(flags) => flags,
            });
        }
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, ()> for ProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &ExtBackgroundEffectSurfaceV1,
        _event: wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlRegion, ()> for ProbeState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegion,
        _event: wayland_client::protocol::wl_region::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

#[allow(dead_code)]
fn compile_request_signatures(
    manager: &ExtBackgroundEffectManagerV1,
    compositor: &WlCompositor,
    surface: &WlSurface,
    queue_handle: &QueueHandle<ProbeState>,
) {
    let effect = manager.get_background_effect(surface, queue_handle, ());
    let region = compositor.create_region(queue_handle, ());
    region.add(0, 0, 960, 600);
    effect.set_blur_region(Some(&region));
    region.destroy();
    effect.destroy();
    surface.commit();
}

fn main() -> Result<()> {
    let connection = Connection::connect_to_env().context("connect to Wayland compositor")?;
    let (globals, mut event_queue) =
        registry_queue_init::<ProbeState>(&connection).context("read Wayland registry")?;
    let queue_handle = event_queue.handle();

    let manager = globals
        .bind::<ExtBackgroundEffectManagerV1, _, _>(&queue_handle, 1..=1, ())
        .context("ext_background_effect_manager_v1 version 1 is not advertised")?;

    let mut state = ProbeState::default();
    event_queue
        .roundtrip(&mut state)
        .context("receive background-effect capabilities")?;

    let Some(flags) = state.capability_flags else {
        bail!("manager was bound but sent no capabilities event");
    };
    let blur = flags & Capability::Blur.bits() != 0;
    println!(
        "ext_background_effect_manager_v1 version={} flags={flags:#x} blur={blur}",
        manager.version()
    );
    manager.destroy();
    event_queue
        .roundtrip(&mut state)
        .context("flush manager destroy")?;

    if !blur {
        bail!("compositor does not advertise the blur capability");
    }

    Ok(())
}
