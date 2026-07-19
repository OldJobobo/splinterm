//! Client-owned bounded scrollback viewport state.

use splinterm_protocol::{ActiveScreen, TerminalRow, TerminalSnapshot};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScrollbackViewport {
    offset_from_bottom: usize,
    unseen_rows: usize,
}

impl ScrollbackViewport {
    #[must_use]
    pub fn offset_from_bottom(&self) -> usize {
        self.offset_from_bottom
    }

    #[must_use]
    pub fn unseen_rows(&self) -> usize {
        self.unseen_rows
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        self.offset_from_bottom == 0
    }

    pub fn return_to_live(&mut self) {
        self.offset_from_bottom = 0;
        self.unseen_rows = 0;
    }

    pub fn scroll_up(&mut self, lines: usize, snapshot: &TerminalSnapshot) {
        if snapshot.active_screen == ActiveScreen::Alternate {
            return;
        }
        self.offset_from_bottom = self
            .offset_from_bottom
            .saturating_add(lines)
            .min(snapshot.scrollback_rows.len());
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.offset_from_bottom = self.offset_from_bottom.saturating_sub(lines);
        if self.offset_from_bottom == 0 {
            self.unseen_rows = 0;
        }
    }

    /// Preserves a detached viewport as history grows and clamps it when the
    /// bounded daemon snapshot trims or clears rows.
    pub fn observe_history_change(
        &mut self,
        previous_available: usize,
        previous_rows: &[TerminalRow],
        snapshot: &TerminalSnapshot,
    ) {
        if snapshot.active_screen == ActiveScreen::Alternate {
            self.return_to_live();
            return;
        }
        if self.is_live() {
            return;
        }
        let count_growth = snapshot
            .available_scrollback_rows
            .saturating_sub(previous_available);
        let overlap = (0..=previous_rows.len().min(snapshot.scrollback_rows.len()))
            .rev()
            .find(|count| {
                previous_rows[previous_rows.len() - *count..] == snapshot.scrollback_rows[..*count]
            })
            .unwrap_or(0);
        let window_growth = snapshot.scrollback_rows.len().saturating_sub(overlap);
        let appended = count_growth.max(window_growth);
        self.offset_from_bottom = self
            .offset_from_bottom
            .saturating_add(appended)
            .min(snapshot.scrollback_rows.len());
        self.unseen_rows = self.unseen_rows.saturating_add(appended);
        if snapshot.available_scrollback_rows == 0 {
            self.return_to_live();
        }
    }

    #[must_use]
    pub fn visible_rows<'a>(&self, snapshot: &'a TerminalSnapshot) -> Vec<&'a TerminalRow> {
        if self.is_live() || snapshot.active_screen == ActiveScreen::Alternate {
            return snapshot.visible_rows.iter().collect();
        }
        let mut rows = Vec::with_capacity(
            snapshot
                .scrollback_rows
                .len()
                .saturating_add(snapshot.visible_rows.len()),
        );
        rows.extend(snapshot.scrollback_rows.iter());
        rows.extend(snapshot.visible_rows.iter());
        let viewport_height = snapshot.rows.min(rows.len());
        let end = rows.len().saturating_sub(self.offset_from_bottom);
        let start = end.saturating_sub(viewport_height);
        rows[start..end].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use splinterm_core::SplintId;
    use splinterm_protocol::{TerminalInputModes, TerminalSnapshot};

    use super::*;

    fn snapshot(history: &[&str], visible: &[&str]) -> TerminalSnapshot {
        let row = |text: &&str| TerminalRow {
            linebreak: text.starts_with('h'),
            cells: Vec::new(),
        };
        TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision: 1,
            columns: 1,
            rows: visible.len(),
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: false,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0; 3],
            title: String::new(),
            visible_rows: visible.iter().map(row).collect(),
            scrollback_rows: history.iter().map(row).collect(),
            available_scrollback_rows: history.len(),
            omitted_oldest_scrollback_rows: 0,
            exited_code: None,
            exited_signal: None,
        }
    }

    #[test]
    fn scrolling_clamps_and_returns_to_live() {
        let snapshot = snapshot(&["h1", "h2", "h3"], &["v1", "v2"]);
        let mut viewport = ScrollbackViewport::default();
        viewport.scroll_up(20, &snapshot);
        assert_eq!(viewport.offset_from_bottom(), 3);
        viewport.scroll_down(2);
        assert_eq!(viewport.offset_from_bottom(), 1);
        viewport.scroll_down(1);
        assert!(viewport.is_live());
    }

    #[test]
    fn visible_rows_compose_history_before_live_grid() {
        let snapshot = snapshot(&["h1", "h2"], &["v1", "v2"]);
        let mut viewport = ScrollbackViewport::default();
        viewport.scroll_up(1, &snapshot);
        let rows = viewport.visible_rows(&snapshot);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].linebreak, "oldest displayed row comes from history");
        assert!(
            !rows[1].linebreak,
            "newest displayed row comes from live grid"
        );
    }

    #[test]
    fn detached_viewport_tracks_new_output_and_unseen_rows() {
        let initial = snapshot(&["h1", "h2"], &["v1", "v2"]);
        let mut viewport = ScrollbackViewport::default();
        viewport.scroll_up(1, &initial);
        let next = snapshot(&["h1", "h2", "v1"], &["v2", "v3"]);
        viewport.observe_history_change(
            initial.available_scrollback_rows,
            &initial.scrollback_rows,
            &next,
        );
        assert_eq!(viewport.offset_from_bottom(), 2);
        assert_eq!(viewport.unseen_rows(), 1);
    }

    #[test]
    fn detached_viewport_tracks_ring_rollover_at_constant_capacity() {
        let mut initial = snapshot(&["h1", "h2"], &["v1"]);
        initial.scrollback_rows[1].linebreak = false;
        let mut viewport = ScrollbackViewport::default();
        viewport.scroll_up(1, &initial);
        let mut next = snapshot(&["h2", "h3"], &["v2"]);
        next.scrollback_rows[0].linebreak = false;
        next.scrollback_rows[1].linebreak = true;
        viewport.observe_history_change(
            initial.available_scrollback_rows,
            &initial.scrollback_rows,
            &next,
        );
        assert_eq!(viewport.offset_from_bottom(), 2);
        assert_eq!(viewport.unseen_rows(), 1);
    }

    #[test]
    fn alternate_screen_and_clear_history_return_live() {
        let initial = snapshot(&["h1"], &["v1"]);
        let mut viewport = ScrollbackViewport::default();
        viewport.scroll_up(1, &initial);
        let mut alternate = initial.clone();
        alternate.active_screen = ActiveScreen::Alternate;
        viewport.observe_history_change(1, &initial.scrollback_rows, &alternate);
        assert!(viewport.is_live());

        viewport.scroll_up(1, &initial);
        let cleared = snapshot(&[], &["v1"]);
        viewport.observe_history_change(1, &initial.scrollback_rows, &cleared);
        assert!(viewport.is_live());
    }
}
