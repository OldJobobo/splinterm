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
    handoff_status::{
        HandoffBanner, HandoffPresentation, HandoffStatusHub, HandoffTransition, WindowHandoffPhase,
    },
    renderer::SessionPickerTextCache,
};

use super::{
    App, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel, ExitClass, PaneView,
    WindowTopologyCommand, diagnostics, terminal_input_pending_for,
};

/// A daemon connection lost while a handoff was fenced, which kept the
/// Window open instead of ending it.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum HeldFailure {
    /// A pane's update stream ended unexpectedly.
    PaneStream,
    /// The topology manager stopped.
    TopologyManager,
}

/// The frontend's handoff state: its Window's hub, the presented copy, any
/// failure held open, and the banner's text cache.
#[derive(Default)]
pub(super) struct HandoffFrontend {
    hub: Option<HandoffStatusHub>,
    presentation: HandoffPresentation,
    held_failure: Option<HeldFailure>,
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

    /// Folds the hub's view in at `now` and expires an outcome notice;
    /// returns the transition, with `changed` also set by an expiry.
    fn fold(&mut self, now: Instant) -> HandoffTransition {
        let Some(view) = self.hub.as_ref().map(HandoffStatusHub::view) else {
            return HandoffTransition::default();
        };
        let mut transition = self.presentation.apply(view, now);
        transition.changed |= self.presentation.expire(now);
        transition
    }

    /// Whether terminal input, resize, and daemon-mutating actions are paused.
    pub(super) const fn input_paused(&self) -> bool {
        self.presentation.input_paused()
    }

    /// Whether a lost daemon connection keeps the Window open with its
    /// banner rather than ending it: the handoff was fenced, so the loss is
    /// most likely its exec. Read from the hub, not the presented copy,
    /// because a stream can fail before this turn folds in.
    pub(super) fn holds_window_on_disconnect(&self) -> bool {
        self.hub.as_ref().is_some_and(|hub| {
            matches!(
                hub.view().phase,
                WindowHandoffPhase::Fenced | WindowHandoffPhase::AwaitingReconnect
            )
        })
    }

    /// Records a failure the Window was kept open across. If the pause then
    /// ends without a reconnection, the failure takes its original path.
    pub(super) fn hold(&mut self, failure: HeldFailure) {
        self.held_failure = Some(self.held_failure.map_or(failure, |held| held.max(failure)));
    }

    /// The failure held open, taken once the pause ends. M3.12b reconnects
    /// and resnapshots the dead pieces instead, clearing this first.
    fn take_held_failure(&mut self) -> Option<HeldFailure> {
        self.held_failure.take()
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

/// The focus report a pane's terminal hears when a handoff pause ends: the
/// reports sent while paused were dropped, so a terminal that asked for them
/// hears its current focus again.
pub(super) const fn focus_report_on_resume(focus_reporting: bool, focused: bool) -> Option<bool> {
    if focus_reporting { Some(focused) } else { None }
}

impl App {
    /// Whether terminal input and daemon mutations are paused for a handoff.
    pub(super) const fn handoff_paused(&self) -> bool {
        self.handoff.input_paused()
    }

    /// Folds the hub's view in; returns whether the banner, the title, or the
    /// pause changed, which repaints the Window.
    pub(super) fn apply_handoff_status(&mut self, now: Instant) -> bool {
        let transition = self.handoff.fold(now);
        if transition.pause_changed {
            self.handoff_pause_changed();
        }
        if transition.changed {
            self.update_window_title();
            self.presentation.full_redraw = true;
            for buffer in &mut self.surface.buffers {
                buffer.stale.mark_full();
            }
            if self.surface.configured {
                self.scheduling.redraw_pending = true;
            }
        }
        transition.changed
    }

    fn handoff_pause_changed(&mut self) {
        let paused = self.handoff_paused();
        if paused {
            // Typed, pasted, and composed input is dropped, not queued.
            self.input.pending_terminal_input.clear();
            self.input.terminal_input_saturated = false;
            self.input.prefix_state.clear();
            self.cancel_clipboard_image();
        } else if let Some(failure) = self.handoff.take_held_failure() {
            // The handoff was cancelled, yet a connection lost meanwhile was
            // not a committed exec and nothing reconnected it.
            self.end_window_for_held_failure(failure);
            return;
        } else {
            self.ready_panes_for_handoff_resume();
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

    /// Takes the exit the held failure would have taken when it happened.
    fn end_window_for_held_failure(&mut self, failure: HeldFailure) {
        let (event, error, exit) = match failure {
            HeldFailure::PaneStream => (
                DiagnosticEventCode::PaneStreamFailure,
                DiagnosticErrorCode::PaneStream,
                ExitClass::ErrorPaneStream,
            ),
            HeldFailure::TopologyManager => (
                DiagnosticEventCode::TopologyFailure,
                DiagnosticErrorCode::TopologyManager,
                ExitClass::ErrorTopologyManager,
            ),
        };
        if let Some(diagnostics) = diagnostics() {
            diagnostics.emit(DiagnosticLevel::Error, event, Some(error));
        }
        if failure == HeldFailure::TopologyManager {
            self.scheduling.failure = Some(anyhow::anyhow!("topology manager stopped"));
        }
        self.scheduling.request_exit(exit);
    }

    /// Resizes and focus reports were dropped while paused: every pane sends
    /// its geometry again, even where it matches the last one sent, and every
    /// terminal that asked for focus reports hears its current focus.
    fn ready_panes_for_handoff_resume(&mut self) {
        let pending = &self.input.pending_terminal_input;
        let active_focus = self.input.terminal_focus_reported;
        Self::ready_pane_for_handoff_resume(&mut self.panes.pane, active_focus, pending);
        for pane in &mut self.panes.inactive_panes {
            Self::ready_pane_for_handoff_resume(pane, false, pending);
        }
        for tab in self.tab_state.tabs.iter_mut() {
            if let Some(view) = tab.value.as_mut() {
                for pane in std::iter::once(&mut view.pane).chain(view.inactive_panes.iter_mut()) {
                    Self::ready_pane_for_handoff_resume(pane, false, pending);
                }
            }
        }
    }

    /// Readies one pane for the end of a handoff pause; `focused` is the
    /// focus its terminal should believe.
    pub(super) fn ready_pane_for_handoff_resume(
        pane: &mut PaneView,
        focused: bool,
        pending: &std::collections::VecDeque<super::PendingTerminalInput>,
    ) {
        pane.last_resize = None;
        let focus_reporting = pane
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.input_modes.focus_reporting);
        if let Some(focused) = focus_report_on_resume(focus_reporting, focused) {
            let input_pending = terminal_input_pending_for(pending, pane.commands.as_ref());
            Self::request_pane_focus_report(pane, focused, input_pending);
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::task::Wake;

    use splinterm_protocol::{HandoffCancelReason, HandoffStatusPhase};

    use super::*;

    struct NoWake;

    impl Wake for NoWake {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    fn a_failure_held_through_a_cancelled_handoff_is_taken_on_resume() {
        let hub = HandoffStatusHub::default();
        let mut frontend = HandoffFrontend::new(Some(hub.clone()), &Waker::from(Arc::new(NoWake)));
        let lost = hub.source();
        let live = hub.source();
        lost.observe(1, HandoffStatusPhase::Fenced);
        live.observe(1, HandoffStatusPhase::Fenced);
        assert!(frontend.fold(Instant::now()).pause_changed);
        assert!(frontend.holds_window_on_disconnect());
        // A pane stream and then the topology manager fail while fenced.
        frontend.hold(HeldFailure::PaneStream);
        frontend.hold(HeldFailure::TopologyManager);
        frontend.hold(HeldFailure::PaneStream);
        lost.connection_lost();
        assert!(!frontend.fold(Instant::now()).pause_changed);
        // A live pane then hears Cancelled: the daemon kept serving, so the
        // held failure was no exec and now ends the Window.
        live.observe(
            1,
            HandoffStatusPhase::Cancelled {
                reason: HandoffCancelReason::Failed,
            },
        );
        let transition = frontend.fold(Instant::now());
        assert!(transition.pause_changed && !frontend.input_paused());
        assert_eq!(
            frontend.take_held_failure(),
            Some(HeldFailure::TopologyManager)
        );
        assert_eq!(frontend.take_held_failure(), None);
    }

    #[test]
    fn a_terminal_hears_its_focus_again_only_when_it_asked_for_reports() {
        assert_eq!(focus_report_on_resume(true, true), Some(true));
        assert_eq!(focus_report_on_resume(true, false), Some(false));
        assert_eq!(focus_report_on_resume(false, true), None);
    }
}
