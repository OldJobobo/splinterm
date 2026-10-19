//! Key and pointer decisions for trusted dialog Windows.
//!
//! The access consent dialog keeps its original mapping exactly: `G` and
//! `Enter` grant, `D` and `Escape` deny, the green right action area grants
//! and the red left one denies, with no arming delay and no repeat filter.
//!
//! The upgrade question (Plan 0061 M3.13d) proceeds only on a deliberate act,
//! because proceeding closes Windows. Only `G` or the green area proceeds;
//! `Enter`, `Escape` and `D` cancel. Every key press and the proceed click are
//! ignored until [`UPGRADE_ARMING_DELAY`] after the Window first gains
//! keyboard focus, and key repeats are always ignored, so typing that was
//! already under way when the dialog appeared cannot answer it.

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
}

/// The keys a trusted dialog tells apart; the adapter maps keysyms to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrustedDialogKey {
    G,
    D,
    Enter,
    Escape,
    Other,
}

/// How long the upgrade question ignores input after it first gains focus.
pub(crate) const UPGRADE_ARMING_DELAY: Duration = Duration::from_millis(600);

fn armed(focused_at: Option<Instant>, now: Instant) -> bool {
    focused_at.is_some_and(|focused| now.saturating_duration_since(focused) >= UPGRADE_ARMING_DELAY)
}

impl TrustedDialogKind {
    /// The title the Window maps with.
    pub(crate) const fn window_title(self) -> &'static str {
        match self {
            Self::AccessConsent => "Splinterm — Trusted Access Request",
            Self::Upgrade => "Splinterm — Upgrade",
        }
    }

    /// Whether key repeats are dropped before they reach the dialog.
    pub(crate) const fn ignores_repeats(self) -> bool {
        matches!(self, Self::Upgrade)
    }

    /// The answer a key gives: `Some(true)` grants or proceeds,
    /// `Some(false)` denies or cancels, `None` is ignored.
    pub(crate) fn key_decision(
        self,
        key: TrustedDialogKey,
        repeat: bool,
        focused_at: Option<Instant>,
        now: Instant,
    ) -> Option<bool> {
        match self {
            Self::AccessConsent => match key {
                TrustedDialogKey::G | TrustedDialogKey::Enter => Some(true),
                TrustedDialogKey::D | TrustedDialogKey::Escape => Some(false),
                TrustedDialogKey::Other => None,
            },
            Self::Upgrade => {
                if repeat || !armed(focused_at, now) {
                    return None;
                }
                match key {
                    TrustedDialogKey::G => Some(true),
                    TrustedDialogKey::D | TrustedDialogKey::Enter | TrustedDialogKey::Escape => {
                        Some(false)
                    }
                    TrustedDialogKey::Other => None,
                }
            }
        }
    }

    /// The answer a click in an action area gives. A cancel click always
    /// counts; the upgrade question's proceed click counts only once armed.
    pub(crate) fn click_decision(
        self,
        proceed_area: bool,
        focused_at: Option<Instant>,
        now: Instant,
    ) -> Option<bool> {
        if self == Self::Upgrade && proceed_area && !armed(focused_at, now) {
            return None;
        }
        Some(proceed_area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [TrustedDialogKey; 5] = [
        TrustedDialogKey::G,
        TrustedDialogKey::D,
        TrustedDialogKey::Enter,
        TrustedDialogKey::Escape,
        TrustedDialogKey::Other,
    ];

    #[test]
    fn consent_keeps_its_mapping_without_arming_or_repeat_filtering() {
        let kind = TrustedDialogKind::AccessConsent;
        let now = Instant::now();
        for (focused_at, repeat) in [(None, false), (Some(now), true), (Some(now), false)] {
            let decide = |key| kind.key_decision(key, repeat, focused_at, now);
            assert_eq!(decide(TrustedDialogKey::G), Some(true));
            assert_eq!(decide(TrustedDialogKey::Enter), Some(true));
            assert_eq!(decide(TrustedDialogKey::D), Some(false));
            assert_eq!(decide(TrustedDialogKey::Escape), Some(false));
            assert_eq!(decide(TrustedDialogKey::Other), None);
            assert_eq!(kind.click_decision(true, focused_at, now), Some(true));
            assert_eq!(kind.click_decision(false, focused_at, now), Some(false));
        }
        assert!(!kind.ignores_repeats());
        assert_eq!(kind, TrustedDialogKind::default());
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
        assert_eq!(decide(TrustedDialogKey::Other), None);
        assert_eq!(kind.click_decision(true, Some(focused), now), Some(true));
        assert_eq!(kind.click_decision(false, Some(focused), now), Some(false));
    }

    #[test]
    fn upgrade_ignores_every_key_and_the_proceed_click_until_armed() {
        let kind = TrustedDialogKind::Upgrade;
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

    #[test]
    fn upgrade_ignores_key_repeats_even_when_armed() {
        let kind = TrustedDialogKind::Upgrade;
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
    }
}
