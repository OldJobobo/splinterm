//! Window-local agent status for Dojo tab badges and the palette clear.
//!
//! Status is display data from the daemon, never authority and never inferred
//! from terminal content. Tabs show one badge per Dojo and never the
//! free-text detail. A fresh working status shows a Braille spinner here and
//! in the Explorer. It steps at most four times a second on the event loop's
//! existing cursor-blink wakeup, only while the Window has keyboard focus and
//! `SPLINTERM_REDUCED_MOTION` is unset, and otherwise holds its first frame.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::{App, DojoId, QueueHandle, SplintId};
use crate::frontend::{AgentAggregate, AgentGlyphs, AgentSpinner};
use crate::navigation_projection::WindowAgentStatus;
use crate::renderer::{ChromeSource, chrome_icons_available};

pub(super) struct TabAgentStatus {
    status: WindowAgentStatus,
    enabled: bool,
    glyphs: AgentGlyphs,
    /// The working spinner's frame, shared with the Explorer.
    spinner: AgentSpinner,
    last_spinner_step: Instant,
}

impl TabAgentStatus {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            status: WindowAgentStatus::default(),
            enabled,
            glyphs: AgentGlyphs::select(chrome_icons_available()),
            spinner: AgentSpinner::STATIC,
            last_spinner_step: Instant::now(),
        }
    }

    /// One tab badge: the aggregate glyph of the Dojo's Splints.
    pub(super) fn badge(&self, dojo_id: DojoId) -> Option<ChromeSource> {
        if !self.enabled {
            return None;
        }
        AgentAggregate::from_statuses(self.status.dojo_statuses(dojo_id))
            .tab_badge(self.glyphs, self.spinner)
    }

    /// Whether a Dojo's tab badge shows a fresh working status. The badge is
    /// the most urgent state, so a working Splint beside one that needs input
    /// shows no spinner.
    fn shows_working(&self, dojo_id: DojoId) -> bool {
        self.enabled
            && AgentAggregate::from_statuses(self.status.dojo_statuses(dojo_id))
                .top()
                .is_some_and(|(state, is_stale, _)| {
                    state == splinterm_protocol::AgentState::Working && !is_stale
                })
    }

    /// Icons, or ASCII when the embedded icon face is unavailable; shared
    /// with the Explorer at startup.
    pub(super) const fn glyphs(&self) -> AgentGlyphs {
        self.glyphs
    }

    /// Live incarnation of a Splint that currently shows a status, which the
    /// palette's "Clear agent status" captures.
    pub(super) fn clearable_incarnation(&self, splint_id: SplintId) -> Option<u64> {
        self.enabled
            .then(|| self.status.splints.get(&splint_id))
            .flatten()
            .map(|splint| splint.incarnation)
    }
}

impl App {
    pub(super) fn set_agent_status_presentation(&mut self, enabled: bool) -> bool {
        let explorer_changed = self.explorer.set_agent_status_enabled(enabled);
        let tabs_changed = self.tab_state.agent.enabled != enabled;
        self.tab_state.agent.enabled = enabled;
        explorer_changed || tabs_changed
    }

    pub(super) fn apply_window_agent_status(&mut self, status: WindowAgentStatus) -> bool {
        if self.tab_state.agent.status == status {
            return false;
        }
        self.tab_state.agent.status = status;
        true
    }

    /// Applies an accepted clear to the matching live incarnation only; the
    /// next topology poll and Explorer refresh supersede it.
    pub(super) fn apply_agent_status_cleared(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
    ) -> bool {
        let tabs = self
            .tab_state
            .agent
            .status
            .apply_change(splint_id, incarnation, None);
        let explorer = self
            .explorer
            .apply_agent_status(splint_id, incarnation, None);
        tabs || explorer
    }

    /// Whether the working spinner moves now: in a focused Window, without
    /// reduced motion, while a drawn tab badge or the visible Explorer shows
    /// a fresh working status.
    fn agent_spinner_animates(&self) -> bool {
        let tabs = || {
            self.tab_state
                .tab_strip_layout
                .as_ref()
                .is_some_and(|layout| {
                    layout
                        .tabs
                        .iter()
                        .any(|tab| self.tab_state.agent.shows_working(tab.dojo_id))
                })
        };
        // Focus and reduced motion first, so an unfocused Window never builds
        // Explorer rows to find out.
        AgentSpinner::animates(self.input.reduced_motion, self.input.keyboard_focused, true)
            && (tabs() || self.explorer.shows_working_status())
    }

    /// How long the event loop may sleep before the spinner's next frame;
    /// `None` while it rests.
    pub(super) fn agent_spinner_timeout(&self) -> Option<Duration> {
        AgentSpinner::remaining(
            self.agent_spinner_animates(),
            self.tab_state.agent.last_spinner_step.elapsed(),
        )
    }

    /// Steps the spinner when its frame is due, or returns it to its first
    /// frame once it stops, and redraws the chrome that shows it. Runs beside
    /// the cursor blink on the same event-loop wakeup.
    pub(super) fn tick_agent_spinner(&mut self, queue_handle: &QueueHandle<Self>) -> Result<()> {
        let agent = &self.tab_state.agent;
        let Some(next) = agent.spinner.tick(
            self.agent_spinner_animates(),
            agent.last_spinner_step.elapsed(),
        ) else {
            return Ok(());
        };
        self.tab_state.agent.spinner = next;
        self.tab_state.agent.last_spinner_step = Instant::now();
        self.explorer.set_agent_spinner(next);
        if self.surface.configured {
            self.schedule_draw(queue_handle)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use splinterm_protocol::AgentState;

    use super::*;
    use crate::navigation_projection::{NavigationAgentStatus, WindowSplintAgentStatus};

    fn status(dojo_id: DojoId, incarnation: u64, state: AgentState) -> WindowSplintAgentStatus {
        WindowSplintAgentStatus {
            dojo_id,
            incarnation,
            status: NavigationAgentStatus {
                state,
                detail: Some("never shown in tabs".to_owned()),
                stale: false,
                stale_minutes: None,
            },
        }
    }

    #[test]
    fn tab_badges_aggregate_per_dojo_and_never_carry_detail() {
        let (first, second) = (DojoId::new(), DojoId::new());
        let mut agent = TabAgentStatus::new(true);
        let blocked = SplintId::new();
        agent.status.splints = [
            (blocked, status(first, 3, AgentState::NeedsInput)),
            (SplintId::new(), status(first, 1, AgentState::Working)),
            (SplintId::new(), status(second, 2, AgentState::Done)),
        ]
        .into_iter()
        .collect();
        let badge = |agent: &TabAgentStatus, dojo| agent.badge(dojo).map(|badge| badge.describe());
        // The embedded icon face is always available in tests.
        assert_eq!(agent.glyphs(), AgentGlyphs::Icons);
        assert_eq!(badge(&agent, first).as_deref(), Some("[message-question]"));
        assert_eq!(badge(&agent, second).as_deref(), Some("[circle-check]"));
        assert_eq!(agent.badge(DojoId::new()), None);
        assert!(
            [first, second]
                .into_iter()
                .filter_map(|dojo| agent.badge(dojo))
                .all(|badge| !badge.text().contains("shown"))
        );
        assert_eq!(agent.clearable_incarnation(blocked), Some(3));
        assert_eq!(agent.clearable_incarnation(SplintId::new()), None);

        // Needs input outranks working, so the spinner is not shown yet.
        assert!(!agent.shows_working(first));
        assert!(!agent.shows_working(second));
        assert!(agent.status.apply_change(blocked, 3, None));
        assert_eq!(badge(&agent, first).as_deref(), Some("⠋"));
        assert!(agent.shows_working(first));
        assert!(!agent.shows_working(second));
        // The badge shows the spinner's current frame in both glyph styles.
        agent.spinner = AgentSpinner::STATIC.next();
        assert_eq!(badge(&agent, first).as_deref(), Some("⠙"));
        agent.glyphs = AgentGlyphs::Ascii;
        assert_eq!(badge(&agent, first).as_deref(), Some("⠙"));

        agent.enabled = false;
        assert_eq!(agent.badge(second), None);
        assert!(!agent.shows_working(first));
        assert_eq!(agent.clearable_incarnation(blocked), None);
    }

    #[test]
    fn clears_apply_only_to_the_matching_live_incarnation() {
        let dojo = DojoId::new();
        let splint = SplintId::new();
        let mut status_set = WindowAgentStatus::default();
        status_set
            .splints
            .insert(splint, status(dojo, 4, AgentState::Error));
        assert!(!status_set.apply_change(splint, 3, None));
        assert!(!status_set.apply_change(SplintId::new(), 4, None));
        assert!(status_set.splints.contains_key(&splint));
        assert!(status_set.apply_change(splint, 4, None));
        assert!(!status_set.apply_change(splint, 4, None));
    }
}
