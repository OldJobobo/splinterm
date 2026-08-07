use super::super::{
    App, CommandPaletteShortcutAction, Connection, KeyEvent, KeyboardHandler, Keysym, Modifiers,
    PaneFocusAction, PaneTopologyAction, PasteTarget, QueueHandle, RawModifiers,
    SessionPickerShortcutAction, TabShortcutAction, WaylandSurface, WindowCommand,
    WindowTopologyCommand, command_palette_shortcut_action, font_zoom_action, pane_focus_action,
    pane_topology_action, session_picker_shortcut_action, tab_action_dispatch_allowed,
    tab_shortcut_action, wl_keyboard, wl_surface,
};

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
        if surface == self.surface.window.wl_surface() {
            self.set_ime_focus(true);
            self.presentation.full_redraw = true;
            if self.panes.input_modes().focus_reporting && !self.modal.input_modal_open() {
                self.send_command(WindowCommand::Input(b"\x1b[I".to_vec()));
                self.input.terminal_focus_reported = true;
            }
            if let Err(error) = self.schedule_draw(queue_handle) {
                self.scheduling.fail(error);
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
        if surface == self.surface.window.wl_surface() {
            self.set_ime_focus(false);
            self.presentation.full_redraw = true;
            if self.panes.input_modes().focus_reporting && !self.modal.input_modal_open() {
                self.send_command(WindowCommand::Input(b"\x1b[O".to_vec()));
                self.input.terminal_focus_reported = false;
            }
            if let Err(error) = self.schedule_draw(queue_handle) {
                self.scheduling.fail(error);
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn press_key(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        serial: u32,
        event: KeyEvent,
    ) {
        if self.modal.command_palette.is_some()
            || self.modal.dojo_prompt.is_some()
            || self.modal.tab_context_menu.is_some()
        {
            self.modal
                .session_picker_consumed_keys
                .insert(event.raw_code);
            self.handle_key(&event, queue_handle);
            if self.presentation.full_redraw
                && let Err(error) = self.schedule_draw(queue_handle)
            {
                self.scheduling.fail(error);
            }
            return;
        }
        if self.modal.session_picker.is_some() {
            self.modal
                .session_picker_consumed_keys
                .insert(event.raw_code);
        }
        if let Some(action) = command_palette_shortcut_action(
            event.keysym,
            self.input.modifiers,
            false,
            self.tab_state.managed_tabs,
            !self.command_palette_available(),
        ) {
            self.modal
                .session_picker_consumed_keys
                .insert(event.raw_code);
            if action == CommandPaletteShortcutAction::Open {
                if let Err(error) = self.show_command_palette() {
                    eprintln!("splinterm command palette: {error:#}");
                } else if let Err(error) = self.schedule_draw(queue_handle) {
                    self.scheduling.fail(error);
                }
            }
            return;
        }
        if let Some(action) = tab_shortcut_action(
            event.keysym,
            self.input.modifiers,
            false,
            self.tab_state.managed_tabs,
        ) {
            self.modal
                .session_picker_consumed_keys
                .insert(event.raw_code);
            if !tab_action_dispatch_allowed([
                self.modal.session_picker.is_some(),
                self.modal.trusted_consent.is_some(),
                self.modal.session_picker_requested,
                self.tab_state.session_switch_pending,
                self.modal.session_picker_reconcile_pending,
            ]) {
                return;
            }
            let target = match action {
                TabShortcutAction::Next => self
                    .tab_state
                    .tabs
                    .next()
                    .map(|dojo_id| WindowTopologyCommand::ActivateTab { dojo_id }),
                TabShortcutAction::Previous => self
                    .tab_state
                    .tabs
                    .previous()
                    .map(|dojo_id| WindowTopologyCommand::ActivateTab { dojo_id }),
                TabShortcutAction::NewDojo => Some(WindowTopologyCommand::NewDojo {
                    lair_id: self.tab_state.active_identity.lair_id,
                }),
                TabShortcutAction::Close => Some(WindowTopologyCommand::CloseTab {
                    dojo_id: self.tab_state.active_dojo_id(),
                }),
                TabShortcutAction::Consume => None,
            };
            if let Some(target) = target
                && let Err(error) = self.send_topology_command(target)
            {
                self.scheduling.fail(error);
            }
            return;
        }
        if let Some(action) = session_picker_shortcut_action(
            event.keysym,
            self.input.modifiers,
            false,
            self.modal.session_picker.is_some()
                || self.modal.trusted_consent.is_some()
                || self.modal.session_picker_requested
                || self.tab_state.session_switch_pending
                || self.modal.session_picker_reconcile_pending
                || self.modal.command_palette_reconcile_pending,
        ) {
            if action == SessionPickerShortcutAction::Request {
                if self.tab_state.topology_commands.is_some() {
                    self.modal.session_picker_requested = true;
                    if let Err(error) =
                        self.send_topology_command(WindowTopologyCommand::RequestSessionPicker)
                    {
                        self.modal.session_picker_requested = false;
                        eprintln!("splinterm session picker request: {error:#}");
                    }
                } else {
                    eprintln!("splinterm session picker is unavailable for this attachment");
                }
            }
            return;
        }
        if self.modal.session_picker.is_some() || self.modal.trusted_consent.is_some() {
            self.handle_key(&event, queue_handle);
            if (self.presentation.full_redraw || self.modal.session_picker_redraw)
                && let Err(error) = self.schedule_draw(queue_handle)
            {
                self.scheduling.fail(error);
            }
            return;
        }
        if self.tab_state.topology_commands.is_some()
            && let Some(action) = pane_topology_action(event.keysym, self.input.modifiers)
        {
            if let Some(target) = self.panes.focused_splint() {
                let dojo_id = self.tab_state.active_dojo_id();
                let command = match action {
                    PaneTopologyAction::Split(axis) => WindowTopologyCommand::Split {
                        dojo_id,
                        target,
                        axis,
                    },
                    PaneTopologyAction::Close => WindowTopologyCommand::Close { dojo_id, target },
                    PaneTopologyAction::AdjustRatio(delta) => WindowTopologyCommand::AdjustRatio {
                        dojo_id,
                        target,
                        delta,
                    },
                };
                if let Err(error) = self.send_topology_command(command) {
                    self.scheduling.fail(error);
                }
            }
            return;
        }
        if let Some(action) = pane_focus_action(event.keysym, self.input.modifiers) {
            let changed = match action {
                PaneFocusAction::Direction(direction) => self.focus_direction(direction),
            };
            if changed {
                self.update_ime_cursor_rectangle();
                if let Err(error) = self.schedule_draw(queue_handle) {
                    self.scheduling.fail(error);
                }
            }
            return;
        }
        if let Some(action) = font_zoom_action(event.keysym, self.input.modifiers) {
            if let Err(error) = self.apply_font_zoom(action, queue_handle) {
                self.scheduling.fail(error);
            }
            return;
        }
        if self.input.modifiers.ctrl
            && self.input.modifiers.shift
            && matches!(event.keysym, Keysym::c | Keysym::C)
        {
            self.publish_clipboard(queue_handle, serial, false);
        } else if self.input.modifiers.ctrl
            && self.input.modifiers.shift
            && matches!(event.keysym, Keysym::v | Keysym::V)
        {
            self.begin_clipboard_read(PasteTarget::Clipboard);
        } else {
            match self.handle_history_key(&event, queue_handle) {
                Ok(true) => {}
                Ok(false) => {
                    self.handle_key(&event, queue_handle);
                    if self.presentation.full_redraw {
                        if let Err(error) = self.schedule_draw(queue_handle) {
                            self.scheduling.fail(error);
                        }
                    }
                }
                Err(error) => self.scheduling.fail(error),
            }
        }
    }

    fn repeat_key(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if self.modal.command_palette.is_some()
            || self.modal.dojo_prompt.is_some()
            || self.modal.tab_context_menu.is_some()
        {
            self.handle_key(&event, queue_handle);
            if self.presentation.full_redraw
                && let Err(error) = self.schedule_draw(queue_handle)
            {
                self.scheduling.fail(error);
            }
            return;
        }
        if self.modal.session_picker.is_some() {
            self.handle_key(&event, queue_handle);
            if (self.presentation.full_redraw || self.modal.session_picker_redraw)
                && let Err(error) = self.schedule_draw(queue_handle)
            {
                self.scheduling.fail(error);
            }
            return;
        }
        if self
            .modal
            .session_picker_consumed_keys
            .contains(&event.raw_code)
        {
            return;
        }
        if command_palette_shortcut_action(
            event.keysym,
            self.input.modifiers,
            true,
            self.tab_state.managed_tabs,
            !self.command_palette_available(),
        )
        .is_some()
        {
            self.modal
                .session_picker_consumed_keys
                .insert(event.raw_code);
            return;
        }
        if tab_shortcut_action(
            event.keysym,
            self.input.modifiers,
            true,
            self.tab_state.managed_tabs,
        )
        .is_some()
        {
            self.modal
                .session_picker_consumed_keys
                .insert(event.raw_code);
            return;
        }
        if session_picker_shortcut_action(
            event.keysym,
            self.input.modifiers,
            true,
            self.modal.session_picker.is_some()
                || self.modal.trusted_consent.is_some()
                || self.modal.session_picker_requested
                || self.tab_state.session_switch_pending
                || self.modal.session_picker_reconcile_pending
                || self.modal.command_palette_reconcile_pending,
        )
        .is_some()
        {
            return;
        }
        if self.modal.trusted_consent.is_some() {
            self.handle_key(&event, queue_handle);
            return;
        }
        if let Some(action) = font_zoom_action(event.keysym, self.input.modifiers) {
            if let Err(error) = self.apply_font_zoom(action, queue_handle) {
                self.scheduling.fail(error);
            }
            return;
        }
        match self.handle_history_key(&event, queue_handle) {
            Ok(true) => {}
            Ok(false) => self.handle_key(&event, queue_handle),
            Err(error) => self.scheduling.fail(error),
        }
    }

    fn release_key(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.modal
            .session_picker_consumed_keys
            .remove(&event.raw_code);
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
        self.input.modifiers = modifiers;
    }
}
