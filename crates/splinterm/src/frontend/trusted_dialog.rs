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
    Other,
}

/// How long the upgrade and restart questions ignore input after they first
/// gain focus.
pub(crate) const UPGRADE_ARMING_DELAY: Duration = Duration::from_millis(600);

fn armed(focused_at: Option<Instant>, now: Instant) -> bool {
    focused_at.is_some_and(|focused| now.saturating_duration_since(focused) >= UPGRADE_ARMING_DELAY)
}

impl TrustedDialogKind {
    /// The title the Window maps with.
    pub(crate) const fn window_title(self) -> &'static str {
        match self {
            Self::AccessConsent => "Splinterm — Trusted Access Request",
            Self::Upgrade | Self::Restart => "Splinterm — Upgrade",
        }
    }

    /// Whether key repeats are dropped before they reach the dialog.
    pub(crate) const fn ignores_repeats(self) -> bool {
        matches!(self, Self::Upgrade | Self::Restart)
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
                TrustedDialogKey::R | TrustedDialogKey::Other => None,
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
                    TrustedDialogKey::R | TrustedDialogKey::Other => None,
                }
            }
            Self::Restart => {
                if repeat || !armed(focused_at, now) {
                    return None;
                }
                match key {
                    TrustedDialogKey::R => Some(true),
                    TrustedDialogKey::D | TrustedDialogKey::Enter | TrustedDialogKey::Escape => {
                        Some(false)
                    }
                    TrustedDialogKey::G | TrustedDialogKey::Other => None,
                }
            }
        }
    }

    /// The answer a click in an action area gives. A cancel click always
    /// counts; the upgrade and restart questions' proceed click counts only
    /// once armed.
    pub(crate) fn click_decision(
        self,
        proceed_area: bool,
        focused_at: Option<Instant>,
        now: Instant,
    ) -> Option<bool> {
        if matches!(self, Self::Upgrade | Self::Restart) && proceed_area && !armed(focused_at, now)
        {
            return None;
        }
        Some(proceed_area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [TrustedDialogKey; 6] = [
        TrustedDialogKey::G,
        TrustedDialogKey::D,
        TrustedDialogKey::R,
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
            assert_eq!(decide(TrustedDialogKey::R), None);
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
    fn upgrade_and_restart_ignore_every_key_and_the_proceed_click_until_armed() {
        for kind in [TrustedDialogKind::Upgrade, TrustedDialogKind::Restart] {
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
    fn upgrade_and_restart_ignore_key_repeats_even_when_armed() {
        for kind in [TrustedDialogKind::Upgrade, TrustedDialogKind::Restart] {
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
    }
}
