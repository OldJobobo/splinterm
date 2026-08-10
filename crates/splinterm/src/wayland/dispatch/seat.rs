use super::super::{App, Capability, Connection, QueueHandle, SeatHandler, SeatState, wl_seat};

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.platform.seat_state
    }

    fn new_seat(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
        self.platform.seat_count += 1;
    }

    fn new_capability(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if self.clipboard.data_device.is_none() {
            self.clipboard.data_device = Some(
                self.platform
                    .data_device_manager
                    .get_data_device(queue_handle, &seat),
            );
            self.clipboard.primary_device = self
                .platform
                .primary_selection_manager
                .as_ref()
                .map(|manager| manager.get_selection_device(queue_handle, &seat));
        }
        if capability == Capability::Keyboard && self.input.keyboard.is_none() {
            match self.platform.seat_state.get_keyboard_with_repeat(
                queue_handle,
                &seat,
                None,
                self.platform.loop_handle.clone(),
                Box::new(|_, _, _| {}),
            ) {
                Ok(keyboard) => {
                    self.input.keyboard = Some(keyboard);
                    self.input.keyboard_seat = Some(seat.clone());
                    if self.input.text_input.is_none()
                        && let Some(manager) = &self.platform.text_input_manager
                    {
                        self.input.text_input = Some(manager.get_text_input(
                            &seat,
                            queue_handle,
                            self.input.ime_generation,
                        ));
                        self.input.text_input_seat = Some(seat.clone());
                    }
                }
                Err(error) => self
                    .scheduling
                    .fail(anyhow::anyhow!("create keyboard: {error}")),
            }
        }
        if capability == Capability::Pointer && self.input.pointer.is_none() {
            match self.platform.seat_state.get_pointer(queue_handle, &seat) {
                Ok(pointer) => {
                    self.input.pointer = Some(pointer);
                    self.input.pointer_seat = Some(seat);
                }
                Err(error) => self
                    .scheduling
                    .fail(anyhow::anyhow!("create pointer: {error}")),
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
        if capability == Capability::Keyboard && self.input.keyboard_seat.as_ref() == Some(&seat) {
            self.input.prefix_state.clear();
            if let Some(keyboard) = self.input.keyboard.take() {
                keyboard.release();
            }
            self.input.keyboard_seat = None;
            if self.input.text_input_seat.as_ref() == Some(&seat) {
                self.input.ime.entered = false;
                self.input.ime_generation = self.input.ime_generation.saturating_add(1);
                self.input.ime_modal_barrier = false;
                self.clear_ime_preedit();
                if let Some(text_input) = self.input.text_input.take() {
                    text_input.disable();
                    text_input.commit();
                    text_input.destroy();
                }
                self.input.text_input_seat = None;
            }
        }
        if capability == Capability::Pointer && self.input.pointer_seat.as_ref() == Some(&seat) {
            if let Some(pointer) = self.input.pointer.take() {
                pointer.release();
            }
            self.input.pointer_seat = None;
            self.panes.pane.pointer_cell = None;
        }
    }

    fn remove_seat(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
    ) {
        if self.input.keyboard_seat.as_ref() == Some(&seat) {
            self.input.prefix_state.clear();
            if let Some(keyboard) = self.input.keyboard.take() {
                keyboard.release();
            }
            self.input.keyboard_seat = None;
            if self.input.text_input_seat.as_ref() == Some(&seat) {
                self.input.ime.entered = false;
                self.input.ime_generation = self.input.ime_generation.saturating_add(1);
                self.input.ime_modal_barrier = false;
                self.clear_ime_preedit();
                if let Some(text_input) = self.input.text_input.take() {
                    text_input.disable();
                    text_input.commit();
                    text_input.destroy();
                }
                self.input.text_input_seat = None;
            }
        }
        if self.input.pointer_seat.as_ref() == Some(&seat) {
            if let Some(pointer) = self.input.pointer.take() {
                pointer.release();
            }
            self.input.pointer_seat = None;
        }
        self.clipboard.data_device = None;
        self.clipboard.primary_device = None;
        self.platform.seat_count = self.platform.seat_count.saturating_sub(1);
    }
}
