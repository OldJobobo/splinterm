//! Key and pointer decisions for trusted dialog Windows.
//!
//! The access consent dialog (ADR 0014) is default-deny. `Enter`, `Escape`
//! and `D` deny; only `G` or the green right action area grants, and the red
//! left one denies. Every key press and the grant click are ignored until
//! [`UPGRADE_ARMING_DELAY`] has passed since the Window last gained keyboard
//! focus or the selected duration last changed, and key repeats are always
//! ignored. The person chooses the duration with the arrow keys or by
//! clicking its row, never with a letter: "Just this once" is preselected,
//! withheld durations cannot be selected, and selecting never grants. See
//! [`AccessChoice`].
//!
//! The upgrade question (Plan 0061 M3.13d) proceeds only on a deliberate act,
//! because proceeding closes Windows. Only `G` or the green area proceeds;
//! `Enter`, `Escape` and `D` cancel. Every key press and the proceed click are
//! ignored until [`UPGRADE_ARMING_DELAY`] after the Window first gains
//! keyboard focus, and key repeats are always ignored, so typing that was
//! already under way when the dialog appeared cannot answer it.
//!
//! The launcher's restart question (Plan 0037 Milestone 4) ends running
//! shells, so it follows the upgrade question's rules with its own key: only
//! `R` or the green area restarts; `Enter`, `Escape` and `D` answer "Not now",
//! and `G` does nothing.

use std::time::{Duration, Instant};

/// Which trusted dialog a `TrustedConsentUi` Window shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TrustedDialogKind {
    /// The daemon-launched access consent dialog.
    #[default]
    AccessConsent,
    /// The `splinterm upgrade --now` question about Windows that cannot
    /// take part.
    Upgrade,
    /// The desktop launcher's question before restarting a daemon that is
    /// not the installed build and may still run shells.
    Restart,
}

/// The keys a trusted dialog tells apart; the adapter maps keysyms to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrustedDialogKey {
    G,
    D,
    R,
    Enter,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Other,
}

/// How long the trusted dialogs ignore input once armed: the upgrade and
/// restart questions after they first gain focus, the access consent dialog
/// after every focus gain and every duration change.
pub(crate) const UPGRADE_ARMING_DELAY: Duration = Duration::from_millis(600);

fn armed(armed_from: Option<Instant>, now: Instant) -> bool {
    armed_from
        .is_some_and(|armed_from| now.saturating_duration_since(armed_from) >= UPGRADE_ARMING_DELAY)
}

/// What the access consent dialog does with one key press or click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsentAction {
    Deny,
    /// Grant the selected duration.
    Grant,
    /// Select the duration at this index. Selecting never grants.
    Select(usize),
}

/// The access consent dialog's duration choice and arming (ADR 0014).
///
/// Each index is one duration the prompt shows, in order. Index 0, "Just
/// this once", is always offered and preselected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessChoice {
    offered: Vec<bool>,
    selected: usize,
    armed_from: Option<Instant>,
}

impl AccessChoice {
    /// A choice over `offered`, one flag per shown duration, with the first
    /// selected. Unarmed until the Window first gains focus.
    pub(crate) const fn new(offered: Vec<bool>) -> Self {
        Self {
            offered,
            selected: 0,
            armed_from: None,
        }
    }

    pub(crate) const fn selected(&self) -> usize {
        self.selected
    }

    /// The Window gained keyboard focus: the delay starts again.
    pub(crate) const fn note_focus(&mut self, now: Instant) {
        self.armed_from = Some(now);
    }

    /// Selects an offered duration and starts the delay again. A withheld,
    /// unknown, or already selected index changes nothing.
    pub(crate) fn select(&mut self, index: usize, now: Instant) -> bool {
        if !self.offered.get(index).copied().unwrap_or(false) || index == self.selected {
            return false;
        }
        self.selected = index;
        self.armed_from = Some(now);
        true
    }

    fn step(&self, forward: bool) -> Option<usize> {
        if forward {
            (self.selected + 1..self.offered.len()).find(|index| self.offered[*index])
        } else {
            (0..self.selected).rev().find(|index| self.offered[*index])
        }
    }

    /// One key press. Repeats and every press before the dialog is armed are
    /// ignored. Letters other than `G` and `D` do nothing, and no letter
    /// changes the duration.
    pub(crate) fn key(
        &self,
        key: TrustedDialogKey,
        repeat: bool,
        now: Instant,
    ) -> Option<ConsentAction> {
        if repeat || !armed(self.armed_from, now) {
            return None;
        }
        match key {
            TrustedDialogKey::Enter | TrustedDialogKey::Escape | TrustedDialogKey::D => {
                Some(ConsentAction::Deny)
            }
            TrustedDialogKey::G => Some(ConsentAction::Grant),
            TrustedDialogKey::Up | TrustedDialogKey::Left => {
                self.step(false).map(ConsentAction::Select)
            }
            TrustedDialogKey::Down | TrustedDialogKey::Right => {
                self.step(true).map(ConsentAction::Select)
            }
            TrustedDialogKey::R | TrustedDialogKey::Other => None,
        }
    }

    /// A click in an action area: the deny area always counts, the grant
    /// area only once armed.
    pub(crate) fn click_action_area(
        &self,
        grant_area: bool,
        now: Instant,
    ) -> Option<ConsentAction> {
        if !grant_area {
            return Some(ConsentAction::Deny);
        }
        armed(self.armed_from, now).then_some(ConsentAction::Grant)
    }

    /// A click on a duration's row selects it when it is offered.
    pub(crate) fn click_duration(&self, index: usize) -> Option<ConsentAction> {
        (self.offered.get(index).copied().unwrap_or(false) && index != self.selected)
            .then_some(ConsentAction::Select(index))
    }
}

impl TrustedDialogKind {
    /// The title the Window maps with.
    pub(crate) const fn window_title(self) -> &'static str {
        match self {
            Self::AccessConsent => "Splinterm — Trusted Access Request",
            Self::Upgrade | Self::Restart => "Splinterm — Upgrade",
        }
    }

    /// Whether key repeats are dropped before they reach the dialog: always,
    /// so a held key never answers a trusted question.
    pub(crate) const fn ignores_repeats(self) -> bool {
        matches!(self, Self::AccessConsent | Self::Upgrade | Self::Restart)
    }

    /// The answer a key gives to a yes/no question: `Some(true)` grants or
    /// proceeds, `Some(false)` denies or cancels, `None` is ignored. The
    /// access consent dialog answers through [`AccessChoice::key`]; here it
    /// follows the same default-deny rules for a dialog that offers once.
    pub(crate) fn key_decision(
        self,
        key: TrustedDialogKey,
        repeat: bool,
        focused_at: Option<Instant>,
        now: Instant,
    ) -> Option<bool> {
        if repeat || !armed(focused_at, now) {
            return None;
        }
        let proceed = match self {
            Self::AccessConsent | Self::Upgrade => TrustedDialogKey::G,
            Self::Restart => TrustedDialogKey::R,
        };
        match key {
            key if key == proceed => Some(true),
            TrustedDialogKey::D | TrustedDialogKey::Enter | TrustedDialogKey::Escape => Some(false),
            _ => None,
        }
    }

    /// The answer a click in an action area gives. A cancel click always
    /// counts; the proceed click counts only once armed.
    #[allow(
        clippy::unused_self,
        reason = "every yes/no kind answers clicks by one rule; call sites keep naming the kind"
    )]
    pub(crate) fn click_decision(
        self,
        proceed_area: bool,
        focused_at: Option<Instant>,
        now: Instant,
    ) -> Option<bool> {
        if proceed_area && !armed(focused_at, now) {
            return None;
        }
        Some(proceed_area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [TrustedDialogKey; 10] = [
        TrustedDialogKey::G,
        TrustedDialogKey::D,
        TrustedDialogKey::R,
        TrustedDialogKey::Enter,
        TrustedDialogKey::Escape,
        TrustedDialogKey::Up,
        TrustedDialogKey::Down,
        TrustedDialogKey::Left,
        TrustedDialogKey::Right,
        TrustedDialogKey::Other,
    ];

    fn armed_choice(offered: Vec<bool>) -> (AccessChoice, Instant) {
        let mut choice = AccessChoice::new(offered);
        let focused = Instant::now();
        choice.note_focus(focused);
        (choice, focused + UPGRADE_ARMING_DELAY)
    }

    #[test]
    fn consent_is_default_deny_and_only_g_grants() {
        let (choice, now) = armed_choice(vec![true, true, true]);
        assert_eq!(choice.selected(), 0, "just this once is preselected");
        let decide = |key| choice.key(key, false, now);
        assert_eq!(decide(TrustedDialogKey::G), Some(ConsentAction::Grant));
        assert_eq!(decide(TrustedDialogKey::Enter), Some(ConsentAction::Deny));
        assert_eq!(decide(TrustedDialogKey::Escape), Some(ConsentAction::Deny));
        assert_eq!(decide(TrustedDialogKey::D), Some(ConsentAction::Deny));
        assert_eq!(decide(TrustedDialogKey::R), None);
        assert_eq!(decide(TrustedDialogKey::Other), None);
        assert_eq!(
            choice.click_action_area(true, now),
            Some(ConsentAction::Grant)
        );
        assert_eq!(
            choice.click_action_area(false, now),
            Some(ConsentAction::Deny)
        );
        assert!(TrustedDialogKind::AccessConsent.ignores_repeats());
    }

    #[test]
    fn consent_ignores_every_key_and_the_grant_click_until_armed() {
        let mut choice = AccessChoice::new(vec![true, true, true]);
        let focused = Instant::now();
        // Before any focus nothing but a deny click counts.
        for key in KEYS {
            assert_eq!(choice.key(key, false, focused), None, "{key:?}");
        }
        assert_eq!(choice.click_action_area(true, focused), None);
        assert_eq!(
            choice.click_action_area(false, focused),
            Some(ConsentAction::Deny)
        );
        choice.note_focus(focused);
        let early = focused + Duration::from_millis(599);
        for key in KEYS {
            assert_eq!(choice.key(key, false, early), None, "{key:?}");
        }
        assert_eq!(choice.click_action_area(true, early), None);
        let armed = focused + UPGRADE_ARMING_DELAY;
        assert_eq!(
            choice.key(TrustedDialogKey::G, false, armed),
            Some(ConsentAction::Grant)
        );
    }

    #[test]
    fn consent_rearms_on_every_focus_gain_and_duration_change() {
        let (mut choice, armed) = armed_choice(vec![true, true, true]);
        assert_eq!(
            choice.key(TrustedDialogKey::G, false, armed),
            Some(ConsentAction::Grant)
        );
        // Focus returns: the delay starts again.
        choice.note_focus(armed);
        assert_eq!(choice.key(TrustedDialogKey::G, false, armed), None);
        assert_eq!(choice.click_action_area(true, armed), None);
        let rearmed = armed + UPGRADE_ARMING_DELAY;
        // Changing the duration starts it again too, and never grants.
        assert_eq!(
            choice.key(TrustedDialogKey::Down, false, rearmed),
            Some(ConsentAction::Select(1))
        );
        assert!(choice.select(1, rearmed));
        assert_eq!(choice.selected(), 1);
        assert_eq!(choice.key(TrustedDialogKey::G, false, rearmed), None);
        assert_eq!(choice.click_action_area(true, rearmed), None);
        assert_eq!(
            choice.key(TrustedDialogKey::G, false, rearmed + UPGRADE_ARMING_DELAY),
            Some(ConsentAction::Grant)
        );
    }

    #[test]
    fn consent_ignores_key_repeats_even_when_armed() {
        let (choice, now) = armed_choice(vec![true, true, true]);
        for key in KEYS {
            assert_eq!(choice.key(key, true, now), None, "{key:?}");
        }
    }

    #[test]
    fn only_arrows_and_pointer_change_the_duration_and_skip_withheld_ones() {
        // "While open" withheld: the arrows skip it.
        let (mut choice, now) = armed_choice(vec![true, false, true]);
        assert_eq!(
            choice.key(TrustedDialogKey::Right, false, now),
            Some(ConsentAction::Select(2))
        );
        assert_eq!(choice.key(TrustedDialogKey::Up, false, now), None);
        assert!(choice.select(2, now));
        let now = now + UPGRADE_ARMING_DELAY;
        assert_eq!(
            choice.key(TrustedDialogKey::Left, false, now),
            Some(ConsentAction::Select(0))
        );
        assert_eq!(choice.key(TrustedDialogKey::Down, false, now), None);
        // No letter ever selects a duration.
        for key in [
            TrustedDialogKey::G,
            TrustedDialogKey::D,
            TrustedDialogKey::R,
            TrustedDialogKey::Other,
        ] {
            assert!(
                !matches!(choice.key(key, false, now), Some(ConsentAction::Select(_))),
                "{key:?}"
            );
        }
        // A click selects an offered row, never a withheld or unknown one,
        // and selecting a withheld index is refused.
        assert_eq!(choice.click_duration(0), Some(ConsentAction::Select(0)));
        assert_eq!(choice.click_duration(1), None);
        assert_eq!(choice.click_duration(2), None, "already selected");
        assert_eq!(choice.click_duration(9), None);
        assert!(!choice.select(1, now));
        assert_eq!(choice.selected(), 2);
    }

    #[test]
    fn upgrade_proceeds_only_on_g_and_enter_cancels() {
        let kind = TrustedDialogKind::Upgrade;
        let focused = Instant::now();
        let now = focused + UPGRADE_ARMING_DELAY;
        let decide = |key| kind.key_decision(key, false, Some(focused), now);
        assert_eq!(decide(TrustedDialogKey::G), Some(true));
        assert_eq!(decide(TrustedDialogKey::Enter), Some(false));
        assert_eq!(decide(TrustedDialogKey::Escape), Some(false));
        assert_eq!(decide(TrustedDialogKey::D), Some(false));
        assert_eq!(decide(TrustedDialogKey::R), None);
        assert_eq!(decide(TrustedDialogKey::Other), None);
        assert_eq!(kind.click_decision(true, Some(focused), now), Some(true));
        assert_eq!(kind.click_decision(false, Some(focused), now), Some(false));
    }

    #[test]
    fn restart_confirms_only_on_r_and_enter_g_and_escape_never_restart() {
        let kind = TrustedDialogKind::Restart;
        let focused = Instant::now();
        let now = focused + UPGRADE_ARMING_DELAY;
        let decide = |key| kind.key_decision(key, false, Some(focused), now);
        assert_eq!(decide(TrustedDialogKey::R), Some(true));
        assert_eq!(decide(TrustedDialogKey::Enter), Some(false));
        assert_eq!(decide(TrustedDialogKey::Escape), Some(false));
        assert_eq!(decide(TrustedDialogKey::D), Some(false));
        assert_eq!(decide(TrustedDialogKey::G), None);
        assert_eq!(decide(TrustedDialogKey::Other), None);
        assert_eq!(kind.click_decision(true, Some(focused), now), Some(true));
        assert_eq!(kind.click_decision(false, Some(focused), now), Some(false));
    }

    #[test]
    fn yes_no_dialogs_ignore_every_key_and_the_proceed_click_until_armed() {
        for kind in [
            TrustedDialogKind::AccessConsent,
            TrustedDialogKind::Upgrade,
            TrustedDialogKind::Restart,
        ] {
            let focused = Instant::now();
            let early = focused + Duration::from_millis(599);
            for key in KEYS {
                // Before any focus, and inside the arming delay.
                assert_eq!(kind.key_decision(key, false, None, early), None, "{key:?}");
                assert_eq!(
                    kind.key_decision(key, false, Some(focused), early),
                    None,
                    "{key:?}"
                );
            }
            assert_eq!(kind.click_decision(true, None, early), None);
            assert_eq!(kind.click_decision(true, Some(focused), early), None);
            // Cancelling by click is never delayed.
            assert_eq!(kind.click_decision(false, None, early), Some(false));
            assert_eq!(
                kind.click_decision(false, Some(focused), early),
                Some(false)
            );
        }
    }

    #[test]
    fn yes_no_dialogs_ignore_key_repeats_even_when_armed() {
        for kind in [
            TrustedDialogKind::AccessConsent,
            TrustedDialogKind::Upgrade,
            TrustedDialogKind::Restart,
        ] {
            let focused = Instant::now();
            let now = focused + Duration::from_secs(5);
            assert!(kind.ignores_repeats());
            for key in KEYS {
                assert_eq!(
                    kind.key_decision(key, true, Some(focused), now),
                    None,
                    "{key:?}"
                );
            }
        }
    }

    #[test]
    fn each_kind_has_its_own_title() {
        assert_eq!(
            TrustedDialogKind::AccessConsent.window_title(),
            "Splinterm — Trusted Access Request"
        );
        assert_eq!(
            TrustedDialogKind::Upgrade.window_title(),
            "Splinterm — Upgrade"
        );
        assert_eq!(
            TrustedDialogKind::Restart.window_title(),
            "Splinterm — Upgrade"
        );
        assert_eq!(
            TrustedDialogKind::default(),
            TrustedDialogKind::AccessConsent
        );
    }
}
