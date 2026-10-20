//! The Window's daemon handoff (upgrade) banner and input gate
//! (Plan 0061 M3.12a).
//!
//! The Window's [`HandoffStatusHub`] is fed by every pane's control
//! subscription. Here the frontend folds its view in once per event-loop
//! turn, pauses the local input paths while the Window is paused, and paints
//! the banner. The controllers drop paused commands too, so input queued
//! before this turn never reaches the daemon either.

use std::{
    task::Waker,
    time::{Duration, Instant},
};

use crate::{
    handoff_status::{HandoffBanner, HandoffPresentation, HandoffStatusHub, WindowHandoffPhase},
    renderer::SessionPickerTextCache,
};

use super::{App, WindowTopologyCommand};

/// The frontend's handoff state: its Window's hub, the presented copy, and
/// the banner's text cache.
#[derive(Default)]
pub(super) struct HandoffFrontend {
    hub: Option<HandoffStatusHub>,
    presentation: HandoffPresentation,
    pub(super) text_cache: SessionPickerTextCache,
}

impl HandoffFrontend {
    /// Follows `hub`, waking the event loop through `waker` on each change.
    pub(super) fn new(hub: Option<HandoffStatusHub>, waker: &Waker) -> Self {
        if let Some(hub) = &hub {
            hub.set_waker(waker.clone());
        }
        Self {
            hub,
            ..Self::default()
        }
    }

    /// Whether terminal input, resize, and daemon-mutating actions are paused.
    pub(super) const fn input_paused(&self) -> bool {
        self.presentation.input_paused()
    }

    /// Whether a lost daemon connection keeps the Window open with its
    /// banner rather than ending it: the handoff was fenced, so the loss is
    /// most likely its exec. M3.12b reconnects from this state and calls
    /// [`HandoffStatusHub::reconnected`]. Read from the hub, not the
    /// presented copy, because a stream can fail before this turn folds in.
    pub(super) fn holds_window_on_disconnect(&self) -> bool {
        self.hub.as_ref().is_some_and(|hub| {
            matches!(
                hub.view().phase,
                WindowHandoffPhase::Fenced | WindowHandoffPhase::AwaitingReconnect
            )
        })
    }

    /// The banner's text, also appended to the Window title.
    pub(super) fn banner_text(&self) -> Option<String> {
        self.presentation.banner().map(HandoffBanner::text)
    }

    /// How long the event loop may sleep before an outcome notice expires.
    pub(super) fn timeout(&self, now: Instant) -> Option<Duration> {
        self.presentation
            .deadline()
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

impl App {
    /// Whether terminal input and daemon mutations are paused for a handoff.
    pub(super) const fn handoff_paused(&self) -> bool {
        self.handoff.input_paused()
    }

    /// Folds the hub's view in; returns whether the banner, the title, or the
    /// pause changed, which repaints the Window.
    pub(super) fn apply_handoff_status(&mut self, now: Instant) -> bool {
        let Some(view) = self.handoff.hub.as_ref().map(HandoffStatusHub::view) else {
            return false;
        };
        let transition = self.handoff.presentation.apply(view, now);
        let expired = self.handoff.presentation.expire(now);
        if transition.pause_changed {
            self.handoff_pause_changed();
        }
        let changed = transition.changed || expired;
        if changed {
            self.update_window_title();
            self.presentation.full_redraw = true;
            for buffer in &mut self.surface.buffers {
                buffer.stale.mark_full();
            }
            if self.surface.configured {
                self.scheduling.redraw_pending = true;
            }
        }
        changed
    }

    fn handoff_pause_changed(&mut self) {
        let paused = self.handoff_paused();
        if paused {
            // Typed, pasted, and composed input is dropped, not queued.
            self.input.pending_terminal_input.clear();
            self.input.terminal_input_saturated = false;
            self.input.prefix_state.clear();
            self.cancel_clipboard_image();
        } else {
            // Resizes were dropped while paused: send every pane's current
            // geometry again, even where it matches the last one sent.
            self.forget_sent_pane_sizes();
            if self.surface.configured
                && let Err(error) = self.emit_resize()
            {
                self.scheduling.fail(error);
            }
        }
        if let Some(palette) = self.modal.command_palette.as_mut()
            && palette.set_mutations_paused(paused)
        {
            self.refresh_command_palette();
        }
        if let Some(menu) = self.modal.tab_context_menu.as_mut()
            && menu.set_mutations_paused(paused)
        {
            self.modal.tab_context_menu_layout = None;
            self.presentation.full_redraw = true;
        }
        // Terminal composition follows the pause; owned fields keep IME.
        if self.input.ime.entered {
            self.enable_text_input_if_allowed();
        }
    }

    /// Drops a topology command the handoff pause holds back, undoing the
    /// local state its entry point set up while awaiting the topology
    /// manager, so no picker, Explorer row, or split placeholder is left
    /// waiting for an answer that never comes.
    pub(super) fn drop_topology_command_for_handoff(&mut self, command: &WindowTopologyCommand) {
        let command = match command {
            WindowTopologyCommand::ExplorerContext { command, .. } => command.as_ref(),
            command => command,
        };
        match command {
            WindowTopologyCommand::Split {
                target,
                pending: Some(pending),
                ..
            } => {
                if let Err(error) = self.rollback_pending_remote_split(*target, *pending) {
                    self.scheduling.fail(error);
                }
            }
            WindowTopologyCommand::RequestLairPrompt { .. }
            | WindowTopologyCommand::RequestDojoPrompt { .. }
            | WindowTopologyCommand::RequestDojoRestorePrompt { .. }
            | WindowTopologyCommand::RequestSelector { .. }
            | WindowTopologyCommand::RequestSessionPicker => {
                self.modal.session_picker_requested = false;
            }
            WindowTopologyCommand::OpenDojo { .. }
            | WindowTopologyCommand::FocusSplint { .. }
            | WindowTopologyCommand::NavigateLair { .. }
            | WindowTopologyCommand::NewLair { .. }
            | WindowTopologyCommand::PickerNewLair { .. }
            | WindowTopologyCommand::NewDojo { .. }
            | WindowTopologyCommand::PickerNewDojo { .. } => {
                self.tab_state.session_switch_pending = false;
                self.explorer.clear_pending();
            }
            _ => {}
        }
        eprintln!("splinterm: action paused while splinterd upgrades");
    }

    fn forget_sent_pane_sizes(&mut self) {
        for pane in
            std::iter::once(&mut self.panes.pane).chain(self.panes.inactive_panes.iter_mut())
        {
            pane.last_resize = None;
        }
        for tab in self.tab_state.tabs.iter_mut() {
            if let Some(view) = tab.value.as_mut() {
                for pane in std::iter::once(&mut view.pane).chain(view.inactive_panes.iter_mut()) {
                    pane.last_resize = None;
                }
            }
        }
    }
}
