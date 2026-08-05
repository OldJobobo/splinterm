use super::super::{
    App, BackgroundCapability, BackgroundEffectCommitMode, BackgroundManagerEvent, Connection,
    Dispatch, ExtBackgroundEffectManagerV1, ExtBackgroundEffectSurfaceV1, Proxy, QueueHandle,
    WaylandSurface, WindowCommand, WpFractionalScaleManagerV1, WpFractionalScaleV1, WpViewport,
    WpViewporter, ZwpTextInputManagerV3, ZwpTextInputV3, background_effect_capability_bits,
    ext_background_effect_surface_v1, wp_fractional_scale_v1, zwp_text_input_v3,
};

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for App {
    fn event(
        state: &mut Self,
        _proxy: &ExtBackgroundEffectManagerV1,
        event: BackgroundManagerEvent,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let BackgroundManagerEvent::Capabilities { flags } = event {
            let flags = background_effect_capability_bits(flags);
            state.surface.background_effect_capabilities_received = true;
            state
                .surface
                .background_effect_state
                .set_capability_flags(flags);
            if state.surface.background_effect_trace {
                eprintln!(
                    "splinterm background-effect capabilities={flags:#x} blur={}",
                    flags & BackgroundCapability::Blur.bits() != 0
                );
            }
            if state
                .surface
                .background_effect_reconcile_schedule
                .capability_reconciles_immediately()
                && let Err(error) = state.execute_background_effect_actions(
                    queue_handle,
                    BackgroundEffectCommitMode::Immediate,
                )
            {
                state.scheduling.fail(error);
            }
        }
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, ()> for App {
    fn event(
        _state: &mut Self,
        _proxy: &ExtBackgroundEffectSurfaceV1,
        _event: ext_background_effect_surface_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
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
                state.scheduling.fail(error);
            }
        }
    }
}

impl Dispatch<ZwpTextInputV3, u64> for App {
    fn event(
        state: &mut Self,
        _proxy: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        generation: &u64,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if *generation != state.input.ime_generation {
            return;
        }
        let ime_batch_event = matches!(
            &event,
            zwp_text_input_v3::Event::PreeditString { .. }
                | zwp_text_input_v3::Event::CommitString { .. }
                | zwp_text_input_v3::Event::Done { .. }
        );
        if ime_batch_event && (state.modal.inline_picker_open() || state.input.ime_modal_barrier) {
            state.input.ime.clear_composition();
            return;
        }
        match event {
            zwp_text_input_v3::Event::Enter { surface } => {
                if surface == *state.surface.window.wl_surface() {
                    state.input.ime.entered = true;
                    if state.input.keyboard_focused && !state.modal.inline_picker_open() {
                        state.enable_text_input();
                    }
                }
            }
            zwp_text_input_v3::Event::Leave { surface } => {
                if surface == *state.surface.window.wl_surface() {
                    state.input.ime.entered = false;
                    state.clear_ime_preedit();
                    if state.surface.configured {
                        if let Err(error) = state.schedule_draw(queue_handle) {
                            state.scheduling.fail(error);
                        }
                    }
                }
            }
            zwp_text_input_v3::Event::PreeditString { text, .. } => {
                state.input.ime.set_preedit(text);
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                state.input.ime.set_commit(text);
            }
            zwp_text_input_v3::Event::Done { serial } => {
                let (_serial_matches, _, commit) = state.input.ime.finish(serial);
                if let Some(commit) = commit {
                    state.send_command(WindowCommand::Input(commit.into_bytes()));
                }
                if let Err(error) = state.refresh_ime_preedit() {
                    state.scheduling.fail(error);
                    return;
                }
                if state.surface.configured {
                    if let Err(error) = state.schedule_draw(queue_handle) {
                        state.scheduling.fail(error);
                    }
                }
            }
            _ => {}
        }
    }
}
