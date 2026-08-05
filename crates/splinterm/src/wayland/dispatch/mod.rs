mod clipboard;
mod keyboard;
mod output;
mod pointer;
mod protocol;
mod seat;
mod window;

use super::{
    App, OutputState, ProvidesRegistryState, RegistryState, SeatState, delegate_compositor,
    delegate_data_device, delegate_keyboard, delegate_output, delegate_pointer,
    delegate_primary_selection, delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell,
    delegate_xdg_window, registry_handlers,
};

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
        &mut self.platform.registry_state
    }

    registry_handlers![OutputState, SeatState];
}
