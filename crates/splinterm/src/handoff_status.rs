//! Window-wide daemon handoff (upgrade) status for the graphical client
//! (Plan 0061 M3.12a).
//!
//! The daemon broadcasts `SubscriptionEvent::HandoffStatus` on every control
//! subscription, and each pane of a Window holds one. Every pane's controller
//! reports what its subscription delivers to one [`HandoffStatusHub`] per
//! Window, so the first pane to hear a phase pauses the whole Window, and the
//! hub drops duplicates by daemon generation and phase.
//!
//! The hub keeps each pane's last phase apart, since subscriptions deliver at
//! slightly different times. The Window pauses while any pane reports
//! `Preparing` or `Fenced`, and resumes once every pane that reported the
//! attempt also reported `Cancelled`, so one late pane cannot reopen input
//! early or replay an attempt that already ended.
//!
//! While paused, only the local UI works: terminal input is dropped, not
//! queued, and daemon-mutating actions are disabled. A connection lost while
//! fenced is how a committed handoff looks from here (the daemon execs), so
//! the pause then stays until reconnection; [`HandoffStatusHub::reconnected`]
//! is that hook for M3.12b, and [`HandoffStatusHub::rolled_back`] is the hook
//! for M3.14's continuation outcome.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::Waker,
    time::{Duration, Instant},
};

use splinterm_protocol::{HandoffCancelReason, HandoffStatusPhase};

/// How long an outcome notice, "Upgrade cancelled" or "Rolled back", stays.
pub const HANDOFF_NOTICE_DURATION: Duration = Duration::from_secs(3);

/// The Window-wide handoff phase.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowHandoffPhase {
    /// No handoff: the Window works normally.
    #[default]
    Idle,
    /// A pane heard `Preparing`. Input and daemon mutations are paused.
    Preparing,
    /// A pane heard `Fenced`. Input and daemon mutations are paused.
    Fenced,
    /// A connection was lost while fenced, as when a committed handoff execs.
    /// Paused until M3.12b reconnects.
    AwaitingReconnect,
}

impl WindowHandoffPhase {
    /// Whether terminal input, resize, and daemon mutations are paused.
    #[must_use]
    pub const fn input_paused(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

/// A short-lived outcome shown after an attempt ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffNotice {
    /// The attempt stopped before exec; the same daemon keeps serving.
    Cancelled(HandoffCancelReason),
    /// The new daemon failed and the old one resumed (M3.14).
    RolledBack,
}

/// What the banner shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffBanner {
    Preparing,
    InputPaused,
    Notice(HandoffNotice),
}

impl HandoffBanner {
    /// The banner text, also appended to the Window title.
    #[must_use]
    pub fn text(self) -> String {
        match self {
            Self::Preparing => "Preparing upgrade…".to_owned(),
            Self::InputPaused => "Upgrading — input paused".to_owned(),
            Self::Notice(HandoffNotice::Cancelled(reason)) => {
                format!("Upgrade cancelled — {}", cancel_reason_words(reason))
            }
            Self::Notice(HandoffNotice::RolledBack) => "Rolled back".to_owned(),
        }
    }
}

/// Why an upgrade was cancelled, in plain words.
#[must_use]
pub const fn cancel_reason_words(reason: HandoffCancelReason) -> &'static str {
    match reason {
        HandoffCancelReason::Requested => "stopped on request",
        HandoffCancelReason::Blocked => "something blocked it",
        HandoffCancelReason::Failed => "preparation failed, nothing changed",
        HandoffCancelReason::TimedOut => "a step took too long",
    }
}

/// A snapshot of the hub for the frontend.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HandoffStatusView {
    pub phase: WindowHandoffPhase,
    /// Bumped on every change, so a frontend can skip unchanged views.
    pub revision: u64,
    /// The latest outcome notice with its serial number.
    pub notice: Option<(u64, HandoffNotice)>,
}

/// What one pane's subscription last reported in this generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourcePhase {
    Preparing,
    Fenced,
}

#[derive(Debug, Default)]
struct HandoffState {
    /// The newest daemon generation heard; zero before any status.
    generation: u64,
    sources: HashMap<u64, SourcePhase>,
    /// Some pane heard this attempt's `Fenced`. Kept until every pane that
    /// reported the attempt ends it, so a slower pane still at `Preparing`
    /// cannot step the banner back.
    fenced: bool,
    awaiting_reconnect: bool,
    revision: u64,
    notice: Option<(u64, HandoffNotice)>,
    notices: u64,
}

impl HandoffState {
    fn phase(&self) -> WindowHandoffPhase {
        if self.awaiting_reconnect {
            WindowHandoffPhase::AwaitingReconnect
        } else if self.sources.is_empty() {
            WindowHandoffPhase::Idle
        } else if self.fenced {
            WindowHandoffPhase::Fenced
        } else {
            WindowHandoffPhase::Preparing
        }
    }

    fn view(&self) -> HandoffStatusView {
        HandoffStatusView {
            phase: self.phase(),
            revision: self.revision,
            notice: self.notice,
        }
    }

    /// Runs `change` and bumps the revision when the visible state changed.
    fn update(&mut self, change: impl FnOnce(&mut Self)) -> bool {
        let before = (self.phase(), self.notice);
        change(self);
        if self.sources.is_empty() {
            // The attempt ended for every pane; the next one starts unfenced.
            self.fenced = false;
        }
        let changed = before != (self.phase(), self.notice);
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    fn post_notice(&mut self, notice: HandoffNotice) {
        self.notices = self.notices.wrapping_add(1);
        self.notice = Some((self.notices, notice));
    }

    fn observe(&mut self, source: u64, generation: u64, phase: HandoffStatusPhase) -> bool {
        // Generation zero is never valid, and an older daemon's report is
        // stale once a newer one was heard.
        if generation == 0 || generation < self.generation {
            return false;
        }
        self.update(|state| {
            if generation > state.generation {
                // A newer daemon: whatever older generations reported is void.
                state.generation = generation;
                state.sources.clear();
                state.fenced = false;
                state.awaiting_reconnect = false;
            }
            match phase {
                HandoffStatusPhase::Preparing => {
                    let previous = state.sources.insert(source, SourcePhase::Preparing);
                    if previous == Some(SourcePhase::Fenced) {
                        // This pane went on to a next attempt, so it lost the
                        // last one's `Cancelled`; only another pane's report
                        // can keep the Window fenced.
                        state.fenced = state
                            .sources
                            .values()
                            .any(|phase| *phase == SourcePhase::Fenced);
                    }
                }
                HandoffStatusPhase::Fenced => {
                    state.sources.insert(source, SourcePhase::Fenced);
                    state.fenced = true;
                }
                HandoffStatusPhase::Cancelled { reason } => {
                    let reported = state.sources.remove(&source).is_some();
                    // The attempt ends once every pane that reported it has
                    // cancelled. A `Cancelled` also proves the daemon is still
                    // serving, so a connection lost while fenced was not the
                    // exec. A `Cancelled` for an attempt nobody reported, as
                    // replayed after a lag, changes nothing.
                    if state.sources.is_empty() && (reported || state.awaiting_reconnect) {
                        state.awaiting_reconnect = false;
                        state.post_notice(HandoffNotice::Cancelled(reason));
                    }
                }
            }
        })
    }

    /// A pane's subscription ended normally: it was closed or cancelled.
    fn remove_source(&mut self, source: u64) -> bool {
        self.update(|state| {
            state.sources.remove(&source);
        })
    }

    /// A pane's connection failed. While fenced that is the committed
    /// handoff's exec, so the pause stays until reconnection.
    fn source_lost(&mut self, source: u64) -> bool {
        self.update(|state| {
            if state.phase() == WindowHandoffPhase::Fenced {
                state.awaiting_reconnect = true;
            }
            state.sources.remove(&source);
        })
    }

    fn reconnected(&mut self) -> bool {
        self.update(|state| {
            state.sources.clear();
            state.awaiting_reconnect = false;
        })
    }

    fn rolled_back(&mut self) -> bool {
        self.update(|state| {
            state.sources.clear();
            state.awaiting_reconnect = false;
            state.post_notice(HandoffNotice::RolledBack);
        })
    }
}

#[derive(Debug, Default)]
struct HubInner {
    state: Mutex<HandoffState>,
    next_source: AtomicU64,
    waker: Mutex<Option<Waker>>,
}

/// One Window's handoff status, shared by its panes' controllers and its
/// frontend. Clones share the same state.
#[derive(Clone, Debug, Default)]
pub struct HandoffStatusHub {
    inner: Arc<HubInner>,
}

impl HandoffStatusHub {
    /// Registers one pane subscription that reports to this hub.
    #[must_use]
    pub fn source(&self) -> HandoffStatusSource {
        HandoffStatusSource {
            hub: self.clone(),
            id: self.inner.next_source.fetch_add(1, Ordering::Relaxed),
            finished: false,
        }
    }

    #[must_use]
    pub fn view(&self) -> HandoffStatusView {
        self.lock().view()
    }

    /// Whether terminal input, resize, and daemon mutations are paused now.
    #[must_use]
    pub fn input_paused(&self) -> bool {
        self.lock().phase().input_paused()
    }

    /// Wakes the frontend's event loop whenever the view changes.
    pub fn set_waker(&self, waker: Waker) {
        *self
            .inner
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker);
    }

    /// M3.12b hook: every connection was replaced by one to the serving
    /// daemon, so the pause held across the lost connection ends.
    pub fn reconnected(&self) {
        self.update(HandoffState::reconnected);
    }

    /// M3.14 hook: the continuation reported a rollback. Ends the pause and
    /// shows "Rolled back" briefly.
    pub fn rolled_back(&self) {
        self.update(HandoffState::rolled_back);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HandoffState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn update(&self, change: impl FnOnce(&mut HandoffState) -> bool) -> bool {
        let changed = change(&mut self.lock());
        if changed
            && let Some(waker) = self
                .inner
                .waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
        {
            waker.wake_by_ref();
        }
        changed
    }
}

/// One pane subscription's report channel into its Window's hub. Dropping it
/// withdraws the pane's report; [`Self::connection_lost`] instead keeps a
/// fenced Window paused.
#[derive(Debug)]
pub struct HandoffStatusSource {
    hub: HandoffStatusHub,
    id: u64,
    finished: bool,
}

impl HandoffStatusSource {
    /// Reports one `HandoffStatus` event; returns whether the Window's view
    /// changed. Duplicates and stale generations change nothing.
    #[allow(
        clippy::must_use_candidate,
        reason = "a controller reports and moves on; the result serves tests and diagnostics"
    )]
    pub fn observe(&self, daemon_generation: u64, phase: HandoffStatusPhase) -> bool {
        let id = self.id;
        self.hub
            .update(|state| state.observe(id, daemon_generation, phase))
    }

    /// Whether the Window is paused, whichever pane heard it first.
    #[must_use]
    pub fn input_paused(&self) -> bool {
        self.hub.input_paused()
    }

    /// The pane's connection failed rather than closing normally.
    pub fn connection_lost(mut self) {
        self.finished = true;
        let id = self.id;
        self.hub.update(|state| state.source_lost(id));
    }
}

impl Drop for HandoffStatusSource {
    fn drop(&mut self) {
        if !self.finished {
            let id = self.id;
            self.hub.update(|state| state.remove_source(id));
        }
    }
}

/// What a hub view changed in the frontend.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HandoffTransition {
    /// The banner, title, or pause changed: redraw.
    pub changed: bool,
    /// Input was paused or resumed.
    pub pause_changed: bool,
}

/// The frontend's copy of the hub view, with the outcome notice's timer.
#[derive(Debug, Default)]
pub struct HandoffPresentation {
    revision: u64,
    phase: WindowHandoffPhase,
    seen_notice: u64,
    notice: Option<(HandoffNotice, Instant)>,
}

impl HandoffPresentation {
    /// Folds a hub view in at `now`.
    pub fn apply(&mut self, view: HandoffStatusView, now: Instant) -> HandoffTransition {
        if view.revision == self.revision {
            return HandoffTransition::default();
        }
        self.revision = view.revision;
        let was_paused = self.phase.input_paused();
        let mut changed = view.phase != self.phase;
        self.phase = view.phase;
        if let Some((serial, notice)) = view.notice
            && serial != self.seen_notice
        {
            self.seen_notice = serial;
            self.notice = Some((notice, now + HANDOFF_NOTICE_DURATION));
            changed = true;
        }
        // A new attempt supersedes the previous one's outcome.
        if self.phase.input_paused() && self.notice.take().is_some() {
            changed = true;
        }
        HandoffTransition {
            changed,
            pause_changed: was_paused != self.phase.input_paused(),
        }
    }

    /// Drops an outcome notice whose time is up; returns whether it did.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.notice.is_some_and(|(_, until)| now >= until) {
            self.notice = None;
            return true;
        }
        false
    }

    #[must_use]
    pub const fn phase(&self) -> WindowHandoffPhase {
        self.phase
    }

    #[must_use]
    pub const fn input_paused(&self) -> bool {
        self.phase.input_paused()
    }

    #[must_use]
    pub fn banner(&self) -> Option<HandoffBanner> {
        match self.phase {
            WindowHandoffPhase::Preparing => Some(HandoffBanner::Preparing),
            WindowHandoffPhase::Fenced | WindowHandoffPhase::AwaitingReconnect => {
                Some(HandoffBanner::InputPaused)
            }
            WindowHandoffPhase::Idle => {
                self.notice.map(|(notice, _)| HandoffBanner::Notice(notice))
            }
        }
    }

    /// When the outcome notice expires, for the event loop's timeout.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.notice.map(|(_, until)| until)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::task::Wake;

    use super::*;

    const GENERATION: u64 = 4;

    fn cancelled(reason: HandoffCancelReason) -> HandoffStatusPhase {
        HandoffStatusPhase::Cancelled { reason }
    }

    #[test]
    fn one_pane_walks_preparing_fenced_cancelled() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Preparing));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Preparing);
        assert!(hub.input_paused(), "input pauses from Preparing on");
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Fenced);
        assert!(pane.observe(GENERATION, cancelled(HandoffCancelReason::TimedOut)));
        let view = hub.view();
        assert_eq!(view.phase, WindowHandoffPhase::Idle);
        assert!(!hub.input_paused(), "Cancelled resumes input");
        assert!(matches!(
            view.notice,
            Some((_, HandoffNotice::Cancelled(HandoffCancelReason::TimedOut)))
        ));
    }

    #[test]
    fn duplicates_change_nothing_and_keep_the_revision() {
        let hub = HandoffStatusHub::default();
        let first = hub.source();
        let second = hub.source();
        assert!(first.observe(GENERATION, HandoffStatusPhase::Preparing));
        let revision = hub.view().revision;
        assert!(!first.observe(GENERATION, HandoffStatusPhase::Preparing));
        // Another pane hearing the same phase leaves the Window unchanged.
        assert!(!second.observe(GENERATION, HandoffStatusPhase::Preparing));
        assert_eq!(hub.view().revision, revision);
        assert!(first.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert!(!second.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Fenced);
    }

    #[test]
    fn the_first_pane_pauses_and_the_last_pane_resumes() {
        let hub = HandoffStatusHub::default();
        let fast = hub.source();
        let slow = hub.source();
        assert!(fast.observe(GENERATION, HandoffStatusPhase::Preparing));
        assert!(
            slow.input_paused(),
            "a pane that heard nothing is paused too"
        );
        assert!(fast.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert!(!slow.observe(GENERATION, HandoffStatusPhase::Preparing));
        // The fast pane's Cancelled arrives while the slow pane still
        // reports the attempt: the Window stays paused, and the banner does
        // not step back to Preparing.
        assert!(!fast.observe(GENERATION, cancelled(HandoffCancelReason::Requested)));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Fenced);
        assert!(hub.view().notice.is_none());
        assert!(!slow.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert!(slow.observe(GENERATION, cancelled(HandoffCancelReason::Requested)));
        let view = hub.view();
        assert_eq!(view.phase, WindowHandoffPhase::Idle);
        assert!(matches!(
            view.notice,
            Some((_, HandoffNotice::Cancelled(HandoffCancelReason::Requested)))
        ));
    }

    #[test]
    fn out_of_order_and_unreported_cancels_change_nothing() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        // A Cancelled for an attempt this Window never heard of, as a lagged
        // subscription replays it, neither pauses nor shows a notice.
        assert!(!pane.observe(GENERATION, cancelled(HandoffCancelReason::Failed)));
        assert_eq!(hub.view(), HandoffStatusView::default());
        // A Fenced without Preparing still pauses.
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Fenced);
        // A next attempt's Preparing after a lost Cancelled replaces the
        // stale Fenced rather than adding to it.
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Preparing));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Preparing);
    }

    #[test]
    fn generations_filter_stale_reports_and_reset_on_a_newer_daemon() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        assert!(
            !pane.observe(0, HandoffStatusPhase::Fenced),
            "generation zero"
        );
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Fenced));
        // An older generation's report is stale.
        assert!(!pane.observe(GENERATION - 1, cancelled(HandoffCancelReason::Failed)));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Fenced);
        // A newer daemon voids the older generation's pause.
        let other = hub.source();
        assert!(other.observe(GENERATION + 1, HandoffStatusPhase::Preparing));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Preparing);
        assert!(other.observe(GENERATION + 1, cancelled(HandoffCancelReason::Blocked)));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
        // The newer generation sticks.
        assert!(!pane.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
    }

    #[test]
    fn a_closed_pane_withdraws_its_report() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Preparing));
        drop(pane);
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
        assert!(hub.view().notice.is_none(), "closing a pane is no cancel");
    }

    #[test]
    fn a_connection_lost_while_fenced_stays_paused_until_reconnected() {
        let hub = HandoffStatusHub::default();
        let first = hub.source();
        let second = hub.source();
        assert!(first.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert!(!second.observe(GENERATION, HandoffStatusPhase::Fenced));
        first.connection_lost();
        assert_eq!(hub.view().phase, WindowHandoffPhase::AwaitingReconnect);
        drop(second);
        assert_eq!(
            hub.view().phase,
            WindowHandoffPhase::AwaitingReconnect,
            "the committed handoff's exec closes every connection"
        );
        assert!(hub.input_paused());
        hub.reconnected();
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
        assert!(!hub.input_paused());
    }

    #[test]
    fn a_connection_lost_while_preparing_or_idle_does_not_stick() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Preparing));
        pane.connection_lost();
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
        hub.source().connection_lost();
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
    }

    #[test]
    fn a_cancel_from_a_live_pane_ends_a_lost_connection_pause() {
        let hub = HandoffStatusHub::default();
        let lost = hub.source();
        let live = hub.source();
        assert!(lost.observe(GENERATION, HandoffStatusPhase::Fenced));
        assert!(!live.observe(GENERATION, HandoffStatusPhase::Fenced));
        lost.connection_lost();
        assert!(live.observe(GENERATION, cancelled(HandoffCancelReason::Failed)));
        assert_eq!(hub.view().phase, WindowHandoffPhase::Idle);
    }

    #[test]
    fn rolled_back_ends_the_pause_with_a_notice() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        assert!(pane.observe(GENERATION, HandoffStatusPhase::Fenced));
        pane.connection_lost();
        hub.rolled_back();
        let view = hub.view();
        assert_eq!(view.phase, WindowHandoffPhase::Idle);
        assert!(matches!(view.notice, Some((_, HandoffNotice::RolledBack))));
    }

    struct CountingWake(AtomicUsize);

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn the_frontend_is_woken_only_for_changes() {
        let hub = HandoffStatusHub::default();
        let wakes = Arc::new(CountingWake(AtomicUsize::new(0)));
        hub.set_waker(Waker::from(Arc::clone(&wakes)));
        let pane = hub.source();
        pane.observe(GENERATION, HandoffStatusPhase::Preparing);
        pane.observe(GENERATION, HandoffStatusPhase::Preparing);
        pane.observe(GENERATION, HandoffStatusPhase::Fenced);
        assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn banner_text_per_state() {
        assert_eq!(HandoffBanner::Preparing.text(), "Preparing upgrade…");
        assert_eq!(
            HandoffBanner::InputPaused.text(),
            "Upgrading — input paused"
        );
        assert_eq!(
            HandoffBanner::Notice(HandoffNotice::Cancelled(HandoffCancelReason::Requested)).text(),
            "Upgrade cancelled — stopped on request"
        );
        assert_eq!(
            HandoffBanner::Notice(HandoffNotice::Cancelled(HandoffCancelReason::Blocked)).text(),
            "Upgrade cancelled — something blocked it"
        );
        assert_eq!(
            HandoffBanner::Notice(HandoffNotice::Cancelled(HandoffCancelReason::Failed)).text(),
            "Upgrade cancelled — preparation failed, nothing changed"
        );
        assert_eq!(
            HandoffBanner::Notice(HandoffNotice::Cancelled(HandoffCancelReason::TimedOut)).text(),
            "Upgrade cancelled — a step took too long"
        );
        assert_eq!(
            HandoffBanner::Notice(HandoffNotice::RolledBack).text(),
            "Rolled back"
        );
    }

    #[test]
    fn presentation_shows_each_phase_then_a_timed_cancel_notice() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        let mut presentation = HandoffPresentation::default();
        let start = Instant::now();
        assert_eq!(
            presentation.apply(hub.view(), start),
            HandoffTransition::default()
        );
        assert_eq!(presentation.banner(), None);
        pane.observe(GENERATION, HandoffStatusPhase::Preparing);
        assert_eq!(
            presentation.apply(hub.view(), start),
            HandoffTransition {
                changed: true,
                pause_changed: true,
            }
        );
        assert_eq!(presentation.banner(), Some(HandoffBanner::Preparing));
        pane.observe(GENERATION, HandoffStatusPhase::Fenced);
        assert_eq!(
            presentation.apply(hub.view(), start),
            HandoffTransition {
                changed: true,
                pause_changed: false,
            }
        );
        assert_eq!(presentation.banner(), Some(HandoffBanner::InputPaused));
        pane.observe(GENERATION, cancelled(HandoffCancelReason::Blocked));
        let cancelled_at = start + Duration::from_secs(1);
        assert_eq!(
            presentation.apply(hub.view(), cancelled_at),
            HandoffTransition {
                changed: true,
                pause_changed: true,
            }
        );
        assert!(!presentation.input_paused());
        assert_eq!(
            presentation.banner(),
            Some(HandoffBanner::Notice(HandoffNotice::Cancelled(
                HandoffCancelReason::Blocked
            )))
        );
        assert_eq!(
            presentation.deadline(),
            Some(cancelled_at + HANDOFF_NOTICE_DURATION)
        );
        assert!(!presentation.expire(cancelled_at + Duration::from_millis(2_999)));
        assert!(presentation.expire(cancelled_at + HANDOFF_NOTICE_DURATION));
        assert_eq!(presentation.banner(), None);
        assert_eq!(presentation.deadline(), None);
    }

    #[test]
    fn presentation_keeps_the_paused_banner_after_a_lost_connection() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        let mut presentation = HandoffPresentation::default();
        pane.observe(GENERATION, HandoffStatusPhase::Fenced);
        presentation.apply(hub.view(), Instant::now());
        pane.connection_lost();
        assert_eq!(
            presentation.apply(hub.view(), Instant::now()),
            HandoffTransition {
                changed: true,
                pause_changed: false,
            }
        );
        assert_eq!(presentation.phase(), WindowHandoffPhase::AwaitingReconnect);
        assert_eq!(presentation.banner(), Some(HandoffBanner::InputPaused));
    }

    #[test]
    fn a_new_attempt_replaces_the_previous_outcome_notice() {
        let hub = HandoffStatusHub::default();
        let pane = hub.source();
        let mut presentation = HandoffPresentation::default();
        let now = Instant::now();
        pane.observe(GENERATION, HandoffStatusPhase::Preparing);
        pane.observe(GENERATION, cancelled(HandoffCancelReason::Failed));
        presentation.apply(hub.view(), now);
        assert!(matches!(
            presentation.banner(),
            Some(HandoffBanner::Notice(_))
        ));
        pane.observe(GENERATION, HandoffStatusPhase::Preparing);
        presentation.apply(hub.view(), now);
        assert_eq!(presentation.banner(), Some(HandoffBanner::Preparing));
        assert_eq!(presentation.deadline(), None);
    }
}
