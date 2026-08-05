use super::super::{
    App, CompositorHandler, Connection, QueueHandle, SCALE_DENOMINATOR, TerminalResizeCause,
    WaylandSurface, Window, WindowConfigure, WindowHandler, note_output_enter, note_output_leave,
    terminal_resize_allowed, viewport_destination, wl_output, wl_surface,
};

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if surface != self.surface.window.wl_surface() || factor <= 0 {
            return;
        }
        let Ok(integer_scale) = u32::try_from(factor) else {
            self.scheduling.fail(anyhow::anyhow!(
                "integer output scale does not fit u32: {factor}"
            ));
            return;
        };
        self.surface.integer_fallback_scale = integer_scale;
        if self.surface.fractional_scale.is_none() {
            let scale_120 = integer_scale.saturating_mul(SCALE_DENOMINATOR);
            if let Err(error) = self.apply_scale(scale_120, queue_handle) {
                self.scheduling.fail(error);
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
        if surface == self.surface.window.wl_surface() {
            self.scheduling.frame_pending = false;
            if self.scheduling.redraw_pending {
                if let Err(error) = self.draw(queue_handle) {
                    self.scheduling.fail(error);
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
        if surface != self.surface.window.wl_surface() {
            return;
        }
        note_output_enter(&mut self.platform.entered_outputs, output);
        self.platform.output_count = self.platform.entered_outputs.len();
        if let Err(error) = self.refresh_output_dpi(output, queue_handle) {
            self.scheduling.fail(error);
        }
    }

    fn surface_leave(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        output: &wl_output::WlOutput,
    ) {
        if surface != self.surface.window.wl_surface() {
            return;
        }
        let was_most_recent = note_output_leave(&mut self.platform.entered_outputs, output);
        self.platform.output_count = self.platform.entered_outputs.len();
        if was_most_recent {
            // With no entered output, retain the last observation as Foot does
            // while temporarily unmapped. Otherwise promote the previous output.
            if let Some(current) = self.platform.entered_outputs.last().cloned() {
                if let Err(error) = self.refresh_output_dpi(&current, queue_handle) {
                    self.scheduling.fail(error);
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
        if self.modal.trusted_consent.is_some() {
            self.decide_consent(false);
        } else {
            self.scheduling.exit = true;
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
            .map_or(self.surface.logical_width, std::num::NonZeroU32::get);
        let height = configure
            .new_size
            .1
            .map_or(self.surface.logical_height, std::num::NonZeroU32::get);
        let resized = (width, height) != (self.surface.logical_width, self.surface.logical_height);
        if resized {
            self.surface.logical_width = width;
            self.surface.logical_height = height;
            self.modal.session_picker_layout = None;
            if let Some(viewport) = &self.surface.viewport {
                match viewport_destination(width, height) {
                    Ok((width, height)) => viewport.set_destination(width, height),
                    Err(error) => {
                        self.scheduling.fail(error);
                        return;
                    }
                }
            }
            self.surface.buffers.clear();
            self.presentation.full_redraw = true;
            self.update_ime_cursor_rectangle();
        }
        let initial_configure = !self.surface.configured;
        self.surface.configured = true;
        if initial_configure || resized {
            debug_assert!(terminal_resize_allowed(
                TerminalResizeCause::SurfaceConfigure,
                self.panes.pane.last_resize.is_some(),
            ));
            let resize = if self.modal.inline_picker_open() {
                self.panes.restored_frontend_needs_resize = true;
                Ok(())
            } else {
                self.emit_resize()
            };
            if let Err(error) = self
                .queue_background_effect_geometry_for_draw()
                .and(resize)
                .and_then(|()| self.schedule_draw(queue_handle))
            {
                self.scheduling.fail(error);
            }
        }
    }
}
