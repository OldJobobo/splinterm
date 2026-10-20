//! Pure presentation of explicitly reported agent status.
//!
//! Status is display data from the daemon, never authority and never inferred
//! from terminal content. Every state is distinguishable by icon shape and by
//! text alone; nothing here animates. The free-text detail is Explorer-only
//! and never reaches Dojo tabs.

use std::fmt::Write as _;

use splinterm_protocol::AgentState;
use unicode_width::UnicodeWidthChar;

use crate::navigation_projection::NavigationAgentStatus;
use crate::renderer::{ChromeIcon, ChromeSource};

/// Display priority, highest first. Stale statuses rank below every fresh one.
const PRIORITY: [AgentState; 5] = [
    AgentState::NeedsInput,
    AgentState::Error,
    AgentState::Working,
    AgentState::Done,
    AgentState::Idle,
];

/// Marker after an ASCII glyph for a stale status.
pub(crate) const ASCII_STALE_MARKER: &str = "~";

/// Explorer and accessibility text when endpoint data is stale or disconnected.
pub(crate) const UNAVAILABLE_TEXT: &str = "Agent status unavailable";

/// How status is drawn. Both styles are visual only: accessible text and
/// search always use state words.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AgentGlyphs {
    /// Embedded Tabler icons, two cells each; a clock icon marks stale.
    #[default]
    Icons,
    /// One ASCII character per state; `~` marks stale. Used when the
    /// embedded icon face cannot be loaded, never a replacement glyph.
    Ascii,
}

impl AgentGlyphs {
    pub(crate) const fn select(icons_available: bool) -> Self {
        if icons_available {
            Self::Icons
        } else {
            Self::Ascii
        }
    }

    pub(crate) const fn icon(state: AgentState) -> ChromeIcon {
        match state {
            AgentState::Working => ChromeIcon::Loader2,
            AgentState::NeedsInput => ChromeIcon::MessageQuestion,
            AgentState::Done => ChromeIcon::CircleCheck,
            AgentState::Error => ChromeIcon::AlertCircle,
            AgentState::Idle => ChromeIcon::Point,
        }
    }

    pub(crate) const fn ascii(state: AgentState) -> &'static str {
        match state {
            AgentState::Working => ">",
            AgentState::NeedsInput => "?",
            AgentState::Done => "+",
            AgentState::Error => "!",
            AgentState::Idle => "-",
        }
    }

    /// Cells of the stale marker slot.
    const fn stale_cells(self) -> usize {
        match self {
            Self::Icons => ChromeIcon::CELLS as usize,
            Self::Ascii => 1,
        }
    }

    /// Cells of the fixed Explorer column: the glyph and the stale slot.
    pub(crate) const fn column_cells(self) -> usize {
        match self {
            Self::Icons => 2 * ChromeIcon::CELLS as usize,
            Self::Ascii => 2,
        }
    }

    fn push_glyph(self, source: &mut ChromeSource, state: AgentState) {
        match self {
            Self::Icons => source.push_icon(Self::icon(state)),
            Self::Ascii => source.push_str(Self::ascii(state)),
        };
    }

    fn push_stale(self, source: &mut ChromeSource) {
        match self {
            Self::Icons => source.push_icon(ChromeIcon::Clock),
            Self::Ascii => source.push_str(ASCII_STALE_MARKER),
        };
    }

    /// The fixed-width column for one state, or blank cells without one.
    fn column(self, state: Option<(AgentState, bool)>) -> ChromeSource {
        let mut column = ChromeSource::default();
        match state {
            None => {
                column.push_str(&" ".repeat(self.column_cells()));
            }
            Some((state, is_stale)) => {
                self.push_glyph(&mut column, state);
                if is_stale {
                    self.push_stale(&mut column);
                } else {
                    column.push_str(&" ".repeat(self.stale_cells()));
                }
            }
        }
        column
    }
}

pub(crate) const fn state_text(state: AgentState) -> &'static str {
    match state {
        AgentState::Working => "Working",
        AgentState::NeedsInput => "Needs input",
        AgentState::Done => "Done",
        AgentState::Error => "Error",
        AgentState::Idle => "Idle",
    }
}

const fn priority(state: AgentState) -> usize {
    match state {
        AgentState::NeedsInput => 0,
        AgentState::Error => 1,
        AgentState::Working => 2,
        AgentState::Done => 3,
        AgentState::Idle => 4,
    }
}

/// Lower is more urgent: every fresh state outranks every stale one.
const fn rank(status: &NavigationAgentStatus) -> usize {
    priority(status.state) + if status.stale { PRIORITY.len() } else { 0 }
}

/// One Splint row's status text, e.g. "Needs input · waiting for approval" or
/// "Working · stale (no report for 2 min)".
pub(crate) fn splint_text(status: &NavigationAgentStatus) -> String {
    let mut text = state_text(status.state).to_owned();
    if status.stale {
        let minutes = status.stale_minutes.unwrap_or(0).max(1);
        let _ = write!(text, " · stale (no report for {minutes} min)");
    }
    if let Some(detail) = &status.detail {
        text.push_str(" · ");
        text.push_str(detail);
    }
    text
}

/// Words Explorer search matches for a Splint, e.g. `needs input needs_input`.
pub(crate) fn search_terms(status: &NavigationAgentStatus) -> String {
    let state = state_text(status.state).to_lowercase();
    let mut terms = format!("{state} {}", status.state.as_str());
    if status.stale {
        terms.push_str(" stale");
    }
    terms
}

/// Fixed-width Explorer glyph column: the glyph and a stale marker slot.
pub(crate) fn glyph_column(
    status: Option<&NavigationAgentStatus>,
    glyphs: AgentGlyphs,
) -> ChromeSource {
    glyphs.column(status.map(|status| (status.state, status.stale)))
}

/// Status counts of a set of Splints, ordered by display priority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AgentAggregate {
    /// `(state, stale, count)` groups, most urgent first; empty when none.
    groups: Vec<(AgentState, bool, usize)>,
}

impl AgentAggregate {
    pub(crate) fn from_statuses<'a>(
        statuses: impl IntoIterator<Item = &'a NavigationAgentStatus>,
    ) -> Self {
        let mut counts = [0_usize; 2 * PRIORITY.len()];
        for status in statuses {
            counts[rank(status)] += 1;
        }
        let groups = counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(rank, count)| {
                (
                    PRIORITY[rank % PRIORITY.len()],
                    rank >= PRIORITY.len(),
                    *count,
                )
            })
            .collect();
        Self { groups }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The most urgent group, which supplies the aggregate glyph.
    pub(crate) fn top(&self) -> Option<(AgentState, bool, usize)> {
        self.groups.first().copied()
    }

    /// Explorer glyph column for a Dojo or Lair row: the most urgent state.
    pub(crate) fn glyph_column(&self, glyphs: AgentGlyphs) -> ChromeSource {
        glyphs.column(self.top().map(|(state, is_stale, _)| (state, is_stale)))
    }

    /// Counts in words for the Explorer status line and accessibility, e.g.
    /// "1 needs input, 2 working".
    pub(crate) fn counts_words(&self) -> String {
        self.groups
            .iter()
            .map(|(state, is_stale, count)| {
                format!(
                    "{count} {}{}",
                    state_text(*state).to_lowercase(),
                    if *is_stale { " (stale)" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// One Dojo tab badge: the aggregate glyph, a count when more than one
    /// Splint shares the top state, and the stale marker, e.g.
    /// `[message-question]2` or `[loader-2][clock]`; ASCII `?2` or `>~`.
    pub(crate) fn tab_badge(&self, glyphs: AgentGlyphs) -> Option<ChromeSource> {
        let (top_state, is_stale, count) = self.top()?;
        let mut badge = ChromeSource::default();
        glyphs.push_glyph(&mut badge, top_state);
        if count > 1 {
            badge.push_str(&count.min(99).to_string());
        }
        if is_stale {
            glyphs.push_stale(&mut badge);
        }
        Some(badge)
    }
}

fn cells(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0).min(2))
        .sum()
}

/// Blank cells between a Dojo tab badge and its label: two, twice a word
/// space, so a count such as the `2` before "Dojo 2" reads as part of the
/// badge rather than as the first word of the name.
const TAB_BADGE_GAP: &str = "  ";

/// Composes a Dojo tab label within `maximum_cells`. The badge comes first and
/// the label text gives up width before it, so on a narrow tab the badge is
/// the last thing dropped. The gap after the badge is dropped together with
/// the label. Without a badge a label too long for the tab ends in an
/// ellipsis.
pub(crate) fn badged_tab_label(
    badge: Option<&ChromeSource>,
    label: &str,
    maximum_cells: usize,
) -> ChromeSource {
    let Some(badge) = badge else {
        return ChromeSource::from(ellipsized(label, maximum_cells.max(1)));
    };
    let mut composed = badge.clone();
    // The separating gap, then at least one label cell and an ellipsis.
    let label_budget =
        maximum_cells.saturating_sub(badge.cells().saturating_add(cells(TAB_BADGE_GAP)));
    if label.is_empty() || label_budget < 2 {
        return composed;
    }
    composed
        .push_str(TAB_BADGE_GAP)
        .push_str(&ellipsized(label, label_budget));
    composed
}

/// `label` within `maximum_cells`, its last kept cell replaced by `…` when it
/// is longer; `maximum_cells` must be at least one.
fn ellipsized(label: &str, maximum_cells: usize) -> String {
    if cells(label) <= maximum_cells {
        return label.to_owned();
    }
    let mut kept = String::new();
    let mut used = 0_usize;
    for character in label.chars() {
        let width = character.width().unwrap_or(0).min(2);
        if used.saturating_add(width) > maximum_cells - 1 {
            break;
        }
        kept.push(character);
        used = used.saturating_add(width);
    }
    kept.push('…');
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: AgentState, is_stale: bool) -> NavigationAgentStatus {
        NavigationAgentStatus {
            state,
            detail: None,
            stale: is_stale,
            stale_minutes: is_stale.then_some(2),
        }
    }

    fn describe(source: &ChromeSource) -> String {
        source.describe()
    }

    #[test]
    fn icon_and_text_tables_cover_every_state_with_ascii_fallbacks() {
        let table = [
            (AgentState::Working, ChromeIcon::Loader2, ">", "Working"),
            (
                AgentState::NeedsInput,
                ChromeIcon::MessageQuestion,
                "?",
                "Needs input",
            ),
            (AgentState::Done, ChromeIcon::CircleCheck, "+", "Done"),
            (AgentState::Error, ChromeIcon::AlertCircle, "!", "Error"),
            (AgentState::Idle, ChromeIcon::Point, "-", "Idle"),
        ];
        for (state, icon, ascii, text) in table {
            assert_eq!(AgentGlyphs::icon(state), icon);
            assert_eq!(AgentGlyphs::ascii(state), ascii);
            assert!(ascii.is_ascii() && cells(ascii) == 1);
            assert_eq!(state_text(state), text);
        }
        // Shape alone distinguishes every state in both styles, and the stale
        // marker is distinct from every state.
        let mut icons = PRIORITY.map(AgentGlyphs::icon).to_vec();
        icons.push(ChromeIcon::Clock);
        icons.sort_unstable();
        icons.dedup();
        assert_eq!(icons.len(), PRIORITY.len() + 1);
        let mut ascii = PRIORITY.map(AgentGlyphs::ascii).to_vec();
        ascii.push(ASCII_STALE_MARKER);
        ascii.sort_unstable();
        ascii.dedup();
        assert_eq!(ascii.len(), PRIORITY.len() + 1);
    }

    #[test]
    fn ascii_is_used_only_when_icons_are_unavailable() {
        assert_eq!(AgentGlyphs::default(), AgentGlyphs::Icons);
        assert_eq!(AgentGlyphs::select(true), AgentGlyphs::Icons);
        assert_eq!(AgentGlyphs::select(false), AgentGlyphs::Ascii);
        let blocked = [status(AgentState::NeedsInput, true)];
        let aggregate = AgentAggregate::from_statuses(&blocked);
        let ascii = aggregate.tab_badge(AgentGlyphs::Ascii).unwrap();
        assert_eq!(ascii.text(), "?~");
        assert!(ascii.icons().is_empty());
        let column = glyph_column(Some(&blocked[0]), AgentGlyphs::Ascii);
        assert_eq!(column.text(), "?~");
        assert!(column.icons().is_empty());
    }

    #[test]
    fn aggregation_prefers_needs_input_and_ranks_stale_last() {
        let statuses = [
            status(AgentState::Idle, false),
            status(AgentState::Working, false),
            status(AgentState::NeedsInput, true),
            status(AgentState::Done, false),
            status(AgentState::Working, false),
            status(AgentState::Error, false),
            status(AgentState::NeedsInput, false),
        ];
        let aggregate = AgentAggregate::from_statuses(&statuses);
        assert_eq!(aggregate.top(), Some((AgentState::NeedsInput, false, 1)));
        assert_eq!(
            aggregate.counts_words(),
            "1 needs input, 1 error, 2 working, 1 done, 1 idle, 1 needs input (stale)"
        );
        for (higher, lower) in PRIORITY.iter().zip(PRIORITY.iter().skip(1)) {
            let pair = [status(*lower, false), status(*higher, false)];
            assert_eq!(
                AgentAggregate::from_statuses(&pair).top().unwrap().0,
                *higher
            );
            let stale_higher = [status(*lower, false), status(*higher, true)];
            assert_eq!(
                AgentAggregate::from_statuses(&stale_higher).top().unwrap(),
                (*lower, false, 1)
            );
        }
        assert!(AgentAggregate::from_statuses(&[]).is_empty());
        assert_eq!(
            AgentAggregate::default().tab_badge(AgentGlyphs::Icons),
            None
        );
    }

    #[test]
    fn tab_badge_counts_only_the_top_state_and_marks_stale() {
        let badge = |statuses: &[NavigationAgentStatus], glyphs| {
            describe(
                &AgentAggregate::from_statuses(statuses)
                    .tab_badge(glyphs)
                    .unwrap(),
            )
        };
        let two_blocked = [
            status(AgentState::NeedsInput, false),
            status(AgentState::NeedsInput, false),
            status(AgentState::Working, false),
        ];
        assert_eq!(
            badge(&two_blocked, AgentGlyphs::Icons),
            "[message-question]2"
        );
        assert_eq!(badge(&two_blocked, AgentGlyphs::Ascii), "?2");
        let stale = [status(AgentState::Working, true)];
        assert_eq!(badge(&stale, AgentGlyphs::Icons), "[loader-2][clock]");
        assert_eq!(badge(&stale, AgentGlyphs::Ascii), ">~");
        let many = vec![status(AgentState::Done, false); 120];
        assert_eq!(badge(&many, AgentGlyphs::Icons), "[circle-check]99");
        let source = AgentAggregate::from_statuses(&many)
            .tab_badge(AgentGlyphs::Icons)
            .unwrap();
        assert_eq!(source.cells(), 4);
        assert_eq!(source.icons(), &[(0, ChromeIcon::CircleCheck)]);
    }

    #[test]
    fn splint_text_appends_stale_age_and_detail() {
        let mut working = status(AgentState::Working, true);
        working.detail = Some("running tests".to_owned());
        assert_eq!(
            splint_text(&working),
            "Working · stale (no report for 2 min) · running tests"
        );
        working.stale_minutes = Some(0);
        assert!(splint_text(&working).contains("no report for 1 min"));
        let mut blocked = status(AgentState::NeedsInput, false);
        blocked.detail = Some("waiting for approval".to_owned());
        assert_eq!(splint_text(&blocked), "Needs input · waiting for approval");
        assert_eq!(search_terms(&blocked), "needs input needs_input");
        assert_eq!(search_terms(&working), "working working stale");
    }

    #[test]
    fn glyph_column_has_a_fixed_width_per_style() {
        for glyphs in [AgentGlyphs::Icons, AgentGlyphs::Ascii] {
            let columns = [
                glyph_column(None, glyphs),
                glyph_column(Some(&status(AgentState::Working, false)), glyphs),
                glyph_column(Some(&status(AgentState::Done, true)), glyphs),
                AgentAggregate::default().glyph_column(glyphs),
                AgentAggregate::from_statuses(&[status(AgentState::Error, false)])
                    .glyph_column(glyphs),
            ];
            assert!(
                columns
                    .iter()
                    .all(|column| column.cells() == glyphs.column_cells())
            );
        }
        assert_eq!(AgentGlyphs::Icons.column_cells(), 4);
        assert_eq!(
            describe(&glyph_column(
                Some(&status(AgentState::Done, true)),
                AgentGlyphs::Icons
            )),
            "[circle-check][clock]"
        );
        assert_eq!(
            describe(&glyph_column(
                Some(&status(AgentState::NeedsInput, false)),
                AgentGlyphs::Icons
            )),
            "[message-question]  "
        );
        assert_eq!(
            glyph_column(Some(&status(AgentState::Done, true)), AgentGlyphs::Ascii).text(),
            "+~"
        );
    }

    #[test]
    fn tab_label_width_budget_drops_label_text_before_the_badge() {
        let label = |badge: Option<&str>, text: &str, maximum: usize| {
            badged_tab_label(badge.map(ChromeSource::from).as_ref(), text, maximum)
                .text()
                .to_owned()
        };
        assert_eq!(label(None, "editor", 6), "editor");
        assert_eq!(label(None, "editor", 3), "ed…");
        assert_eq!(label(None, "editor", 0), "…");
        assert_eq!(label(Some("?"), "editor", 20), "?  editor");
        assert_eq!(label(Some("?2"), "editor", 10), "?2  editor");
        assert_eq!(label(Some("?2"), "editor", 9), "?2  edit…");
        assert_eq!(label(Some("?2"), "editor", 8), "?2  edi…");
        assert_eq!(label(Some("?2"), "editor", 6), "?2  e…");
        // Too narrow for one label cell and an ellipsis: the gap goes with
        // the label and the badge stays.
        assert_eq!(label(Some("?2"), "editor", 5), "?2");
        assert_eq!(label(Some("?2"), "editor", 1), "?2");
        assert_eq!(label(Some("?2"), "", 20), "?2");
        assert_eq!(label(Some(">~"), "日本語", 7), ">~  日…");
        let mut icon_badge = ChromeSource::default();
        icon_badge.push_icon(ChromeIcon::CircleCheck).push_str("3");
        icon_badge.push_icon(ChromeIcon::Clock);
        for maximum in 0..24 {
            let composed = badged_tab_label(Some(&icon_badge), "a long Dojo name", maximum);
            assert!(composed.describe().starts_with("[circle-check]3[clock]"));
            assert_eq!(composed.icons(), icon_badge.icons());
            assert!(composed.cells() <= maximum.max(5));
        }
        assert_eq!(
            badged_tab_label(Some(&icon_badge), "editor", 13).describe(),
            "[circle-check]3[clock]  editor"
        );
        // A label never contributes icons, even with icon codepoints in it.
        assert!(badged_tab_label(Some(&icon_badge), "\u{ea05}x", 20).icons() == icon_badge.icons());
    }

    /// VM acceptance on 8979fa54: a counted badge before "Dojo 2" read as
    /// "2 Dojo 2", the count one word space from the name. The badge, count
    /// and stale marker included, is now two blank cells from the label in
    /// both glyph styles, whether the label starts with a digit or a letter.
    #[test]
    fn tab_badge_count_is_separated_from_the_label() {
        let counted = [
            status(AgentState::NeedsInput, false),
            status(AgentState::NeedsInput, false),
        ];
        let stale_counted = [
            status(AgentState::Working, true),
            status(AgentState::Working, true),
        ];
        for statuses in [&counted[..], &stale_counted[..]] {
            for glyphs in [AgentGlyphs::Icons, AgentGlyphs::Ascii] {
                let badge = AgentAggregate::from_statuses(statuses)
                    .tab_badge(glyphs)
                    .unwrap();
                for name in ["Dojo 2", "2 builds", "editor"] {
                    let composed = badged_tab_label(Some(&badge), name, 40);
                    let after_badge = &composed.text()[badge.text().len()..];
                    assert_eq!(
                        after_badge,
                        format!("{TAB_BADGE_GAP}{name}"),
                        "{glyphs:?} {}",
                        composed.describe()
                    );
                    assert_eq!(composed.icons(), badge.icons());
                }
            }
        }
        let ascii = AgentAggregate::from_statuses(&counted)
            .tab_badge(AgentGlyphs::Ascii)
            .unwrap();
        assert_eq!(
            badged_tab_label(Some(&ascii), "Dojo 2", 40).text(),
            "?2  Dojo 2"
        );
        let icons = AgentAggregate::from_statuses(&stale_counted)
            .tab_badge(AgentGlyphs::Icons)
            .unwrap();
        assert_eq!(
            badged_tab_label(Some(&icons), "2 builds", 40).describe(),
            "[loader-2]2[clock]  2 builds"
        );
    }
}
