//! Pure presentation of explicitly reported agent status.
//!
//! Status is display data from the daemon, never inferred from terminal
//! content and never authority. Every state is distinguishable by glyph shape
//! and by text alone; nothing here animates. The free-text detail is
//! Explorer-only and never reaches Dojo tabs.

use std::fmt::Write as _;

use splinterm_protocol::AgentState;
use unicode_width::UnicodeWidthChar;

use crate::navigation_projection::NavigationAgentStatus;

/// Display priority, highest first. Stale statuses rank below every fresh one.
const PRIORITY: [AgentState; 5] = [
    AgentState::NeedsInput,
    AgentState::Error,
    AgentState::Working,
    AgentState::Done,
    AgentState::Idle,
];

/// Marker appended to a stale badge.
pub(crate) const STALE_MARKER: &str = "~";

/// Explorer and accessibility text when endpoint data is stale or disconnected.
pub(crate) const UNAVAILABLE_TEXT: &str = "Agent status unavailable";

/// Status glyphs resolved against the UI font. `?` and `!` are ASCII already.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentGlyphs {
    working: &'static str,
    done: &'static str,
    idle: &'static str,
}

impl Default for AgentGlyphs {
    fn default() -> Self {
        Self::UNICODE
    }
}

impl AgentGlyphs {
    pub(crate) const UNICODE: Self = Self {
        working: "▸",
        done: "✓",
        idle: "·",
    };
    pub(crate) const ASCII: Self = Self {
        working: ">",
        done: "+",
        idle: "-",
    };

    /// Chooses each glyph's ASCII fallback when the font lacks coverage, so
    /// shape never depends on a missing glyph.
    pub(crate) fn with_coverage(covers: impl Fn(&str) -> bool) -> Self {
        let pick = |unicode: &'static str, ascii: &'static str| {
            if covers(unicode) { unicode } else { ascii }
        };
        Self {
            working: pick(Self::UNICODE.working, Self::ASCII.working),
            done: pick(Self::UNICODE.done, Self::ASCII.done),
            idle: pick(Self::UNICODE.idle, Self::ASCII.idle),
        }
    }

    pub(crate) const fn glyph(self, state: AgentState) -> &'static str {
        match state {
            AgentState::Working => self.working,
            AgentState::NeedsInput => "?",
            AgentState::Done => self.done,
            AgentState::Error => "!",
            AgentState::Idle => self.idle,
        }
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
pub(crate) fn glyph_column(status: Option<&NavigationAgentStatus>, glyphs: AgentGlyphs) -> String {
    status.map_or_else(
        || "  ".to_owned(),
        |status| {
            format!(
                "{}{}",
                glyphs.glyph(status.state),
                if status.stale { STALE_MARKER } else { " " }
            )
        },
    )
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

    /// Explorer glyph column for a Dojo or Lair row.
    pub(crate) fn glyph_column(&self, glyphs: AgentGlyphs) -> String {
        self.top().map_or_else(
            || "  ".to_owned(),
            |(state, is_stale, _)| {
                format!(
                    "{}{}",
                    glyphs.glyph(state),
                    if is_stale { STALE_MARKER } else { " " }
                )
            },
        )
    }

    /// Visible counts for a Dojo or Lair row, e.g. "? 1, ▸ 2".
    pub(crate) fn counts_text(&self, glyphs: AgentGlyphs) -> String {
        self.groups
            .iter()
            .map(|(state, is_stale, count)| {
                format!(
                    "{}{} {count}",
                    glyphs.glyph(*state),
                    if *is_stale { STALE_MARKER } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Accessible counts in words, e.g. "1 needs input, 2 working".
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
    /// Splint shares the top state, and the stale marker, e.g. `?2` or `▸~`.
    pub(crate) fn tab_badge(&self, glyphs: AgentGlyphs) -> Option<String> {
        let (top_state, is_stale, count) = self.top()?;
        let mut badge = glyphs.glyph(top_state).to_owned();
        if count > 1 {
            badge.push_str(&count.min(99).to_string());
        }
        if is_stale {
            badge.push_str(STALE_MARKER);
        }
        Some(badge)
    }
}

fn cells(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0).min(2))
        .sum()
}

/// Composes a Dojo tab label within `maximum_cells`. The badge comes first and
/// the label text gives up width before it, so on a narrow tab the badge is
/// the last thing dropped. Without a badge the label is returned unchanged.
pub(crate) fn badged_tab_label(badge: Option<&str>, label: &str, maximum_cells: usize) -> String {
    let Some(badge) = badge else {
        return label.to_owned();
    };
    let badge_cells = cells(badge);
    // One separating space, then at least one label cell and an ellipsis.
    let label_budget = maximum_cells.saturating_sub(badge_cells.saturating_add(1));
    if label.is_empty() || label_budget < 2 {
        return badge.to_owned();
    }
    if cells(label) <= label_budget {
        return format!("{badge} {label}");
    }
    let mut truncated = String::new();
    let mut used = 0_usize;
    for character in label.chars() {
        let width = character.width().unwrap_or(0).min(2);
        if used.saturating_add(width) > label_budget - 1 {
            break;
        }
        truncated.push(character);
        used = used.saturating_add(width);
    }
    format!("{badge} {truncated}…")
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

    #[test]
    fn glyph_and_text_tables_cover_every_state_with_ascii_fallbacks() {
        let table = [
            (AgentState::Working, "▸", ">", "Working"),
            (AgentState::NeedsInput, "?", "?", "Needs input"),
            (AgentState::Done, "✓", "+", "Done"),
            (AgentState::Error, "!", "!", "Error"),
            (AgentState::Idle, "·", "-", "Idle"),
        ];
        for (state, unicode, ascii, text) in table {
            assert_eq!(AgentGlyphs::UNICODE.glyph(state), unicode);
            assert_eq!(AgentGlyphs::ASCII.glyph(state), ascii);
            assert!(ascii.is_ascii());
            assert_eq!(state_text(state), text);
        }
        // Shape alone distinguishes every state in both glyph sets.
        for glyphs in [AgentGlyphs::UNICODE, AgentGlyphs::ASCII] {
            let mut shapes = PRIORITY.map(|state| glyphs.glyph(state)).to_vec();
            shapes.sort_unstable();
            shapes.dedup();
            assert_eq!(shapes.len(), PRIORITY.len());
            assert!(
                PRIORITY
                    .iter()
                    .all(|state| glyphs.glyph(*state).chars().all(|c| c.width() == Some(1)))
            );
        }
    }

    #[test]
    fn missing_coverage_selects_each_ascii_fallback_independently() {
        assert_eq!(AgentGlyphs::with_coverage(|_| true), AgentGlyphs::UNICODE);
        assert_eq!(AgentGlyphs::with_coverage(|_| false), AgentGlyphs::ASCII);
        let partial = AgentGlyphs::with_coverage(|glyph| glyph != "✓");
        assert_eq!(partial.glyph(AgentState::Working), "▸");
        assert_eq!(partial.glyph(AgentState::Done), "+");
        assert_eq!(partial.glyph(AgentState::Idle), "·");
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
            aggregate.counts_text(AgentGlyphs::UNICODE),
            "? 1, ! 1, ▸ 2, ✓ 1, · 1, ?~ 1"
        );
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
            AgentAggregate::default().tab_badge(AgentGlyphs::UNICODE),
            None
        );
    }

    #[test]
    fn tab_badge_counts_only_the_top_state_and_marks_stale() {
        let two_blocked = [
            status(AgentState::NeedsInput, false),
            status(AgentState::NeedsInput, false),
            status(AgentState::Working, false),
        ];
        assert_eq!(
            AgentAggregate::from_statuses(&two_blocked).tab_badge(AgentGlyphs::UNICODE),
            Some("?2".to_owned())
        );
        let stale = [status(AgentState::Working, true)];
        assert_eq!(
            AgentAggregate::from_statuses(&stale).tab_badge(AgentGlyphs::UNICODE),
            Some("▸~".to_owned())
        );
        assert_eq!(
            AgentAggregate::from_statuses(&stale).tab_badge(AgentGlyphs::ASCII),
            Some(">~".to_owned())
        );
        let many = vec![status(AgentState::Done, false); 120];
        assert_eq!(
            AgentAggregate::from_statuses(&many).tab_badge(AgentGlyphs::UNICODE),
            Some("✓99".to_owned())
        );
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
    fn glyph_column_has_a_fixed_width() {
        let columns = [
            glyph_column(None, AgentGlyphs::UNICODE),
            glyph_column(
                Some(&status(AgentState::Working, false)),
                AgentGlyphs::UNICODE,
            ),
            glyph_column(Some(&status(AgentState::Done, true)), AgentGlyphs::ASCII),
            AgentAggregate::default().glyph_column(AgentGlyphs::UNICODE),
            AgentAggregate::from_statuses(&[status(AgentState::Error, false)])
                .glyph_column(AgentGlyphs::UNICODE),
        ];
        assert!(columns.iter().all(|column| cells(column) == 2));
        assert_eq!(columns[2], "+~");
    }

    #[test]
    fn tab_label_width_budget_drops_label_text_before_the_badge() {
        assert_eq!(badged_tab_label(None, "editor", 3), "editor");
        assert_eq!(badged_tab_label(Some("?"), "editor", 20), "? editor");
        assert_eq!(badged_tab_label(Some("?2"), "editor", 9), "?2 editor");
        assert_eq!(badged_tab_label(Some("?2"), "editor", 8), "?2 edit…");
        assert_eq!(badged_tab_label(Some("?2"), "editor", 7), "?2 edi…");
        assert_eq!(badged_tab_label(Some("?2"), "editor", 5), "?2 e…");
        assert_eq!(badged_tab_label(Some("?2"), "editor", 4), "?2");
        assert_eq!(badged_tab_label(Some("?2"), "editor", 1), "?2");
        assert_eq!(badged_tab_label(Some("▸~"), "日本語", 6), "▸~ 日…");
        for maximum in 0..24 {
            let label = badged_tab_label(Some("✓3~"), "a long Dojo name", maximum);
            assert!(label.starts_with("✓3~"));
            assert!(cells(&label) <= maximum.max(3));
        }
    }
}
