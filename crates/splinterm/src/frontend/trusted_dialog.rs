//! Key and pointer decisions for trusted dialog Windows.
//!
//! Every trusted dialog (the access consent dialog, the upgrade question,
//! and the launcher's restart question) appears without taking keyboard
//! focus, and its keyboard stays inert until the person clicks inside it or,
//! while it has keyboard focus, presses the dialog shortcut (`Ctrl+Alt+G`
//! unless configured otherwise; see `splinterd::dialog_shortcut`). Keyboard
//! focus alone, even when the compositor gives it, never engages a dialog,
//! so typing meant for another Window can never answer one, and the
//! shortcut itself never answers. Any other key with Ctrl, Alt or Super held
//! does nothing. After that click or shortcut the dialog arms: every key
//! press and the proceed or grant click are ignored for
//! [`UPGRADE_ARMING_DELAY`], and the delay starts again
//! whenever the dialog regains focus and, in the access consent dialog,
//! whenever the selected duration changes. Key repeats are always ignored.
//! A click in the deny or cancel area always counts. See [`Engagement`].
//!
//! The access consent dialog (ADR 0014) is default-deny. `Enter`, `Escape`
//! and `D` deny; only `G` or the green right action area grants, and the red
//! left one denies. The person chooses the duration with the arrow keys or
//! by clicking its row, never with a letter: "Just this once" is preselected,
//! withheld durations cannot be selected, and selecting never grants. See
//! [`AccessChoice`].
//!
//! The upgrade question (Plan 0061 M3.13d) proceeds only on a deliberate act,
//! because proceeding closes Windows. Only `G` or the green area proceeds;
//! `Enter`, `Escape` and `D` cancel.
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
    /// The exact dialog shortcut chord: engages the keyboard as a click
    /// inside does, and never answers.
    Engage,
    Other,
}

/// How long a trusted dialog ignores keys and the proceed or grant click
/// after it is engaged, refocused, or (access consent) changes duration.
pub(crate) const UPGRADE_ARMING_DELAY: Duration = Duration::from_millis(600);

/// Whether a trusted dialog's keyboard is engaged, and since when it arms.
///
/// Only a click inside the dialog, or the dialog shortcut pressed while it
/// has focus, engages it. Before that, nothing arms it: a focus gain is
/// ignored, so keys typed for another Window stay inert.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Engagement {
    engaged: bool,
    armed_from: Option<Instant>,
}

impl Engagement {
    /// A click inside the dialog or the dialog shortcut: the first one
    /// engages the keyboard and starts the arming delay; later ones change
    /// nothing.
    pub(crate) const fn engage(&mut self, now: Instant) {
        if !self.engaged {
            self.engaged = true;
            self.armed_from = Some(now);
        }
    }

    /// The dialog gained keyboard focus: an engaged dialog arms again; an
    /// unengaged one stays inert.
    pub(crate) const fn note_focus(&mut self, now: Instant) {
        if self.engaged {
            self.armed_from = Some(now);
        }
    }

    /// Starts the arming delay again, for an engaged dialog.
    const fn rearm(&mut self, now: Instant) {
        if self.engaged {
            self.armed_from = Some(now);
        }
    }

    #[cfg(test)]
    pub(crate) const fn engaged(self) -> bool {
        self.engaged
    }

    /// Whether keys and the proceed or grant click count now.
    pub(crate) fn armed(self, now: Instant) -> bool {
        self.engaged
            && self.armed_from.is_some_and(|armed_from| {
                now.saturating_duration_since(armed_from) >= UPGRADE_ARMING_DELAY
            })
    }
}

/// The share of a trusted dialog's height its red and green action areas
/// take while the text above them leaves room.
pub(crate) const ACTION_AREA_PERCENT: u32 = 22;

/// The shortest the action areas get, in cell rows, when the text needs the
/// rest of the Window.
pub(crate) const MINIMUM_ACTION_ROWS: u32 = 2;

/// Where a trusted dialog's action areas begin, in pixels from the top of a
/// Window `height` tall whose text ends `text_bottom` pixels from its top.
///
/// They take the bottom [`ACTION_AREA_PERCENT`] of the Window while the text
/// ends above that. When a small Window (a tiled one, say) leaves the text
/// no room, they shrink to begin where the text ends, down to
/// `minimum_height`, so the actions never cover text that fits. Only text
/// taller than the Window less `minimum_height` is clipped at the actions;
/// the Window asks the compositor for a minimum size that avoids that
/// ([`minimum_dialog_rows`]).
pub(crate) fn action_area_top(height: u32, text_bottom: u32, minimum_height: u32) -> u32 {
    let preferred = height.saturating_mul(100 - ACTION_AREA_PERCENT) / 100;
    let lowest = height.saturating_sub(minimum_height);
    text_bottom.min(lowest).max(preferred)
}

/// How many of a static dialog's `rows` hold text: up to and including the
/// last one with a visible character.
pub(crate) fn dialog_text_rows(rows: &[splinterm_protocol::TerminalRow]) -> usize {
    rows.iter()
        .rposition(|row| row.cells.iter().any(|cell| !cell.content.trim().is_empty()))
        .map_or(0, |last| last + 1)
}

/// The fewest grid rows that show `text_rows` rows of dialog text above the
/// shortest action areas, with one row to spare for the border.
pub(crate) const fn minimum_dialog_rows(text_rows: usize) -> usize {
    text_rows + MINIMUM_ACTION_ROWS as usize + 1
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

/// The access consent dialog's duration choice, engagement, and arming
/// (ADR 0014).
///
/// Each index is one duration the prompt shows, in order. Index 0, "Just
/// this once", is always offered and preselected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessChoice {
    offered: Vec<bool>,
    selected: usize,
    engagement: Engagement,
}

impl AccessChoice {
    /// A choice over `offered`, one flag per shown duration, with the first
    /// selected. Inert until a click inside the dialog engages it.
    pub(crate) const fn new(offered: Vec<bool>) -> Self {
        Self {
            offered,
            selected: 0,
            engagement: Engagement {
                engaged: false,
                armed_from: None,
            },
        }
    }

    pub(crate) const fn selected(&self) -> usize {
        self.selected
    }

    #[cfg(test)]
    pub(crate) const fn engagement(&self) -> Engagement {
        self.engagement
    }

    /// A click inside the dialog engages it (see [`Engagement::engage`]).
    pub(crate) const fn engage(&mut self, now: Instant) {
        self.engagement.engage(now);
    }

    /// The Window gained keyboard focus (see [`Engagement::note_focus`]).
    pub(crate) const fn note_focus(&mut self, now: Instant) {
        self.engagement.note_focus(now);
    }

    /// Selects an offered duration and starts the delay again. A withheld,
    /// unknown, or already selected index changes nothing.
    pub(crate) fn select(&mut self, index: usize, now: Instant) -> bool {
        if !self.offered.get(index).copied().unwrap_or(false) || index == self.selected {
            return false;
        }
        self.selected = index;
        self.engagement.rearm(now);
        true
    }

    fn step(&self, forward: bool) -> Option<usize> {
        if forward {
            (self.selected + 1..self.offered.len()).find(|index| self.offered[*index])
        } else {
            (0..self.selected).rev().find(|index| self.offered[*index])
        }
    }

    /// One key press. Every key is ignored until a click engages the dialog
    /// and the arming delay passes, and repeats always are. Letters other
    /// than `G` and `D` do nothing, and no letter changes the duration.
    pub(crate) fn key(
        &self,
        key: TrustedDialogKey,
        repeat: bool,
        now: Instant,
    ) -> Option<ConsentAction> {
        if repeat || !self.engagement.armed(now) {
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
            TrustedDialogKey::R | TrustedDialogKey::Engage | TrustedDialogKey::Other => None,
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
        self.engagement.armed(now).then_some(ConsentAction::Grant)
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
    /// proceeds, `Some(false)` denies or cancels, `None` is ignored. Nothing
    /// counts until a click engages the dialog and it arms. The access
    /// consent dialog answers through [`AccessChoice::key`]; here it follows
    /// the same rules for a dialog that offers once.
    pub(crate) fn key_decision(
        self,
        key: TrustedDialogKey,
        repeat: bool,
        engagement: Engagement,
        now: Instant,
    ) -> Option<bool> {
        if repeat || !engagement.armed(now) {
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
        engagement: Engagement,
        now: Instant,
    ) -> Option<bool> {
        if proceed_area && !engagement.armed(now) {
            return None;
        }
        Some(proceed_area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [TrustedDialogKey; 11] = [
        TrustedDialogKey::G,
        TrustedDialogKey::D,
        TrustedDialogKey::R,
        TrustedDialogKey::Enter,
        TrustedDialogKey::Escape,
        TrustedDialogKey::Up,
        TrustedDialogKey::Down,
        TrustedDialogKey::Left,
        TrustedDialogKey::Right,
        TrustedDialogKey::Engage,
        TrustedDialogKey::Other,
    ];

    const KINDS: [TrustedDialogKind; 3] = [
        TrustedDialogKind::AccessConsent,
        TrustedDialogKind::Upgrade,
        TrustedDialogKind::Restart,
    ];

    /// A choice engaged by a click at `clicked`, and the moment it arms.
    fn engaged_choice(offered: Vec<bool>) -> (AccessChoice, Instant) {
        let mut choice = AccessChoice::new(offered);
        let clicked = Instant::now();
        choice.engage(clicked);
        (choice, clicked + UPGRADE_ARMING_DELAY)
    }

    fn engaged_at(clicked: Instant) -> Engagement {
        let mut engagement = Engagement::default();
        engagement.engage(clicked);
        engagement
    }

    #[test]
    fn consent_is_default_deny_and_only_g_grants() {
        let (choice, now) = engaged_choice(vec![true, true, true]);
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

    /// The operator rule: keyboard focus alone never engages a dialog, so a
    /// `G` typed into another application long after the prompt appeared
    /// stays inert, and so do the deny keys.
    #[test]
    fn keys_are_inert_until_a_click_engages_the_dialog_whatever_the_focus() {
        let mut choice = AccessChoice::new(vec![true, true, true]);
        let appeared = Instant::now();
        // The compositor gave it focus anyway, long ago.
        choice.note_focus(appeared);
        let much_later = appeared + Duration::from_secs(60);
        assert!(!choice.engagement().engaged());
        for key in KEYS {
            assert_eq!(choice.key(key, false, much_later), None, "{key:?}");
        }
        assert_eq!(choice.click_action_area(true, much_later), None);
        // Denying by click never waits.
        assert_eq!(
            choice.click_action_area(false, much_later),
            Some(ConsentAction::Deny)
        );
        // A click inside engages it; the keys count once it arms.
        choice.engage(much_later);
        assert!(choice.engagement().engaged());
        assert_eq!(choice.key(TrustedDialogKey::G, false, much_later), None);
        assert_eq!(
            choice.key(
                TrustedDialogKey::G,
                false,
                much_later + UPGRADE_ARMING_DELAY
            ),
            Some(ConsentAction::Grant)
        );
        // A second click does not restart the delay.
        let armed = much_later + UPGRADE_ARMING_DELAY;
        choice.engage(armed);
        assert_eq!(
            choice.key(TrustedDialogKey::D, false, armed),
            Some(ConsentAction::Deny)
        );
    }

    #[test]
    fn consent_ignores_every_key_and_the_grant_click_until_armed() {
        let mut choice = AccessChoice::new(vec![true, true, true]);
        let clicked = Instant::now();
        choice.engage(clicked);
        let early = clicked + Duration::from_millis(599);
        for key in KEYS {
            assert_eq!(choice.key(key, false, early), None, "{key:?}");
        }
        assert_eq!(choice.click_action_area(true, early), None);
        assert_eq!(
            choice.click_action_area(false, early),
            Some(ConsentAction::Deny)
        );
        assert_eq!(
            choice.key(TrustedDialogKey::G, false, clicked + UPGRADE_ARMING_DELAY),
            Some(ConsentAction::Grant)
        );
    }

    #[test]
    fn consent_rearms_on_every_focus_gain_and_duration_change_once_engaged() {
        let (mut choice, armed) = engaged_choice(vec![true, true, true]);
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
        let (choice, now) = engaged_choice(vec![true, true, true]);
        for key in KEYS {
            assert_eq!(choice.key(key, true, now), None, "{key:?}");
        }
    }

    #[test]
    fn only_arrows_and_pointer_change_the_duration_and_skip_withheld_ones() {
        // "While open" withheld: the arrows skip it.
        let (mut choice, now) = engaged_choice(vec![true, false, true]);
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
        let clicked = Instant::now();
        let engagement = engaged_at(clicked);
        let now = clicked + UPGRADE_ARMING_DELAY;
        let decide = |key| kind.key_decision(key, false, engagement, now);
        assert_eq!(decide(TrustedDialogKey::G), Some(true));
        assert_eq!(decide(TrustedDialogKey::Enter), Some(false));
        assert_eq!(decide(TrustedDialogKey::Escape), Some(false));
        assert_eq!(decide(TrustedDialogKey::D), Some(false));
        assert_eq!(decide(TrustedDialogKey::R), None);
        assert_eq!(decide(TrustedDialogKey::Other), None);
        assert_eq!(kind.click_decision(true, engagement, now), Some(true));
        assert_eq!(kind.click_decision(false, engagement, now), Some(false));
    }

    #[test]
    fn restart_confirms_only_on_r_and_enter_g_and_escape_never_restart() {
        let kind = TrustedDialogKind::Restart;
        let clicked = Instant::now();
        let engagement = engaged_at(clicked);
        let now = clicked + UPGRADE_ARMING_DELAY;
        let decide = |key| kind.key_decision(key, false, engagement, now);
        assert_eq!(decide(TrustedDialogKey::R), Some(true));
        assert_eq!(decide(TrustedDialogKey::Enter), Some(false));
        assert_eq!(decide(TrustedDialogKey::Escape), Some(false));
        assert_eq!(decide(TrustedDialogKey::D), Some(false));
        assert_eq!(decide(TrustedDialogKey::G), None);
        assert_eq!(decide(TrustedDialogKey::Other), None);
        assert_eq!(kind.click_decision(true, engagement, now), Some(true));
        assert_eq!(kind.click_decision(false, engagement, now), Some(false));
    }

    /// The same operator rule for every yes/no dialog: focus alone never
    /// engages it, a click does, and it then arms.
    #[test]
    fn yes_no_dialogs_are_inert_until_a_click_then_arm() {
        for kind in KINDS {
            let appeared = Instant::now();
            let mut engagement = Engagement::default();
            engagement.note_focus(appeared);
            let much_later = appeared + Duration::from_secs(60);
            for key in KEYS {
                assert_eq!(
                    kind.key_decision(key, false, engagement, much_later),
                    None,
                    "{kind:?} {key:?}"
                );
            }
            assert_eq!(kind.click_decision(true, engagement, much_later), None);
            assert_eq!(
                kind.click_decision(false, engagement, much_later),
                Some(false),
                "cancelling by click is never delayed"
            );
            engagement.engage(much_later);
            let early = much_later + Duration::from_millis(599);
            for key in KEYS {
                assert_eq!(
                    kind.key_decision(key, false, engagement, early),
                    None,
                    "{kind:?} {key:?}"
                );
            }
            assert_eq!(kind.click_decision(true, engagement, early), None);
            let armed = much_later + UPGRADE_ARMING_DELAY;
            assert_eq!(
                kind.key_decision(TrustedDialogKey::Escape, false, engagement, armed),
                Some(false)
            );
            assert_eq!(kind.click_decision(true, engagement, armed), Some(true));
            // A focus gain after engagement arms again.
            engagement.note_focus(armed);
            assert_eq!(kind.click_decision(true, engagement, armed), None);
        }
    }

    #[test]
    fn yes_no_dialogs_ignore_key_repeats_even_when_armed() {
        for kind in KINDS {
            let clicked = Instant::now();
            let engagement = engaged_at(clicked);
            let now = clicked + Duration::from_secs(5);
            assert!(kind.ignores_repeats());
            for key in KEYS {
                assert_eq!(
                    kind.key_decision(key, true, engagement, now),
                    None,
                    "{key:?}"
                );
            }
        }
    }

    /// The dialog shortcut engages a dialog (see `TrustedConsentUi`) but is
    /// never an answer itself, even once the dialog is armed.
    #[test]
    fn the_dialog_shortcut_never_answers_or_selects() {
        let (choice, armed) = engaged_choice(vec![true, true, true]);
        assert_eq!(choice.key(TrustedDialogKey::Engage, false, armed), None);
        for kind in KINDS {
            let clicked = Instant::now();
            let engagement = engaged_at(clicked);
            assert_eq!(
                kind.key_decision(
                    TrustedDialogKey::Engage,
                    false,
                    engagement,
                    clicked + UPGRADE_ARMING_DELAY
                ),
                None,
                "{kind:?}"
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
        assert_eq!(
            TrustedDialogKind::Restart.window_title(),
            "Splinterm — Upgrade"
        );
        assert_eq!(
            TrustedDialogKind::default(),
            TrustedDialogKind::AccessConsent
        );
    }

    /// The action areas keep the bottom 22% while the text ends above it,
    /// as before.
    #[test]
    fn roomy_dialogs_keep_their_action_areas_at_the_bottom_fifth() {
        assert_eq!(action_area_top(1000, 500, 40), 780);
        assert_eq!(action_area_top(1000, 780, 40), 780);
        assert_eq!(action_area_top(375, 0, 40), 292);
    }

    /// In a small Window the action areas begin where the text ends, so they
    /// never cover it, and keep at least their minimum height. Only text
    /// taller than that leaves is clipped at them.
    #[test]
    fn action_areas_never_cover_text_that_fits_above_their_minimum() {
        // The 2026-10-22 VM case: tiled 621×375, 18 px rows of text.
        let row = 18;
        let minimum = MINIMUM_ACTION_ROWS * row + 5;
        for text_rows in 0..=24 {
            let text_bottom = 4 + text_rows * row;
            let top = action_area_top(375, text_bottom, minimum);
            assert!(top >= 292, "{text_rows}: {top}");
            if text_bottom <= 375 - minimum {
                assert!(top >= text_bottom, "{text_rows}: {top} covers text");
            } else {
                assert_eq!(top, 375 - minimum, "{text_rows}: clipped at the minimum");
            }
        }
        for height in 1..=600 {
            for text_bottom in (0..=height).step_by(7) {
                let top = action_area_top(height, text_bottom, minimum);
                assert!(top <= height);
                assert!(top >= height * 78 / 100);
                assert!(
                    top >= text_bottom || height - top <= minimum.max(height - height * 78 / 100),
                    "{height} {text_bottom}: {top}"
                );
            }
        }
        // A Window too short for even the minimum keeps the bottom 22%.
        assert_eq!(action_area_top(30, 30, 41), 23);
    }

    #[test]
    fn dialog_text_ends_at_its_last_visible_character() {
        fn row(text: &str) -> splinterm_protocol::TerminalRow {
            splinterm_protocol::TerminalRow {
                row_id: None,
                linebreak: false,
                cells: text
                    .chars()
                    .map(|character| splinterm_protocol::TerminalCell {
                        content: character.to_string(),
                        spacer_remaining: None,
                        attributes: splinterm_protocol::CellAttributes::default(),
                    })
                    .collect(),
            }
        }
        assert_eq!(dialog_text_rows(&[]), 0);
        assert_eq!(dialog_text_rows(&[row(""), row("   ")]), 0);
        let rows = [row("TITLE"), row(""), row("text"), row("  "), row("")];
        assert_eq!(dialog_text_rows(&rows), 3);
        assert_eq!(minimum_dialog_rows(3), 3 + 2 + 1);
    }
}
