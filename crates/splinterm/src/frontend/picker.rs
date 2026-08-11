//! Platform-independent Dojo-picker interaction and standalone presentation state.

use std::sync::mpsc::Sender as StdSender;

use splinterm_core::SplintId;
use splinterm_protocol::{
    ActiveScreen, CellAttributes, ColorSource, MouseTracking, TerminalCell, TerminalInputModes,
    TerminalRow, TerminalSnapshot, UnderlineStyle,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum PickerHitTarget {
    New,
    Open(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionPickerItem {
    pub display_title: String,
    pub working_directory: String,
    pub pane_count: usize,
    pub running_pane_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPickerDecision {
    New,
    Open(usize),
}

enum SessionPickerHost {
    Standalone {
        revision: u64,
        synthetic_id: SplintId,
        decision: StdSender<SessionPickerDecision>,
    },
    Inline,
}

pub struct SessionPickerUi {
    items: Vec<SessionPickerItem>,
    selected: usize,
    visible_start: usize,
    hovered: Option<PickerHitTarget>,
    host: SessionPickerHost,
}

const SESSION_PICKER_PAGE_ITEMS: usize = 7;
const SESSION_PICKER_NEW_ROW: usize = 3;
const SESSION_PICKER_FIRST_ITEM_ROW: usize = 5;

impl SessionPickerUi {
    #[must_use]
    pub fn new(items: Vec<SessionPickerItem>, decision: StdSender<SessionPickerDecision>) -> Self {
        Self {
            items,
            selected: 0,
            visible_start: 0,
            hovered: None,
            host: SessionPickerHost::Standalone {
                revision: 0,
                synthetic_id: SplintId::new(),
                decision,
            },
        }
    }

    pub(crate) fn inline(items: Vec<SessionPickerItem>) -> Self {
        Self {
            items,
            selected: 0,
            visible_start: 0,
            hovered: None,
            host: SessionPickerHost::Inline,
        }
    }

    pub(crate) const fn is_inline(&self) -> bool {
        matches!(self.host, SessionPickerHost::Inline)
    }

    pub(crate) fn selected_target(&self) -> PickerHitTarget {
        if self.selected == 0 {
            PickerHitTarget::New
        } else {
            PickerHitTarget::Open(self.selected - 1)
        }
    }

    pub(crate) fn items(&self) -> &[SessionPickerItem] {
        &self.items
    }

    pub(crate) fn layout_state(&self) -> (usize, usize, usize) {
        (self.items.len(), self.selected, self.visible_start)
    }

    pub(crate) fn set_visible_start(&mut self, visible_start: usize) {
        self.visible_start = visible_start;
    }

    pub(crate) const fn hovered(&self) -> Option<PickerHitTarget> {
        self.hovered
    }

    pub(crate) fn update_hovered(&mut self, hovered: Option<PickerHitTarget>) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        true
    }

    pub(crate) fn clear_hovered(&mut self) -> bool {
        self.hovered.take().is_some()
    }

    pub(crate) fn into_standalone_decision(self) -> Option<StdSender<SessionPickerDecision>> {
        match self.host {
            SessionPickerHost::Standalone { decision, .. } => Some(decision),
            SessionPickerHost::Inline => None,
        }
    }

    pub(crate) fn selected_decision(&self) -> SessionPickerDecision {
        if self.selected == 0 {
            SessionPickerDecision::New
        } else {
            SessionPickerDecision::Open(self.selected - 1)
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let count = self.items.len().saturating_add(1);
        let magnitude = delta.unsigned_abs() % count;
        self.selected = if delta.is_negative() {
            self.selected
                .saturating_add(count)
                .saturating_sub(magnitude)
                % count
        } else {
            self.selected.saturating_add(magnitude) % count
        };
        self.ensure_selected_visible(SESSION_PICKER_PAGE_ITEMS);
    }

    pub(crate) fn select_first(&mut self) {
        self.selected = 0;
        self.visible_start = 0;
    }

    pub(crate) fn select_last(&mut self) {
        self.selected = self.items.len();
        self.ensure_selected_visible(SESSION_PICKER_PAGE_ITEMS);
    }

    fn ensure_selected_visible(&mut self, visible_count: usize) {
        if self.items.is_empty() || self.selected == 0 || visible_count == 0 {
            self.visible_start = 0;
            return;
        }
        let selected_item = self.selected - 1;
        if selected_item < self.visible_start {
            self.visible_start = selected_item;
        } else if selected_item >= self.visible_start.saturating_add(visible_count) {
            self.visible_start = selected_item
                .saturating_add(1)
                .saturating_sub(visible_count);
        }
        self.visible_start = self
            .visible_start
            .min(self.items.len().saturating_sub(visible_count));
    }

    pub(crate) fn select_row(&mut self, row: usize) -> Option<SessionPickerDecision> {
        if row == SESSION_PICKER_NEW_ROW || row == SESSION_PICKER_NEW_ROW + 1 {
            self.selected = 0;
            return Some(SessionPickerDecision::New);
        }
        let relative = row.checked_sub(SESSION_PICKER_FIRST_ITEM_ROW)?;
        let slot = relative / 2;
        if slot >= SESSION_PICKER_PAGE_ITEMS {
            return None;
        }
        let item = self.visible_start.checked_add(slot)?;
        if item >= self.items.len() {
            return None;
        }
        self.selected = item + 1;
        Some(SessionPickerDecision::Open(item))
    }

    /// Builds the temporary terminal presentation used only by the standalone
    /// `splinterm dojos` host.
    ///
    /// # Panics
    ///
    /// Panics if called for the inline host, whose presentation is native chrome.
    #[must_use]
    pub fn snapshot(&mut self) -> TerminalSnapshot {
        let SessionPickerHost::Standalone {
            revision,
            synthetic_id,
            ..
        } = &mut self.host
        else {
            panic!("inline Dojo picker does not own a terminal snapshot");
        };
        *revision = revision.saturating_add(1).max(1);
        let marker = |selected| if selected { "› " } else { "  " };
        let mut lines = vec![
            "RECENT DOJOS".to_owned(),
            "Open a running Dojo without restoring or relaunching.".to_owned(),
            String::new(),
            format!("{}New Terminal", marker(self.selected == 0)),
            "    Start a fresh shell".to_owned(),
        ];
        for (index, item) in self
            .items
            .iter()
            .enumerate()
            .skip(self.visible_start)
            .take(SESSION_PICKER_PAGE_ITEMS)
        {
            lines.push(format!(
                "{}{}",
                marker(self.selected == index + 1),
                item.display_title
            ));
            let pane_label = if item.pane_count == 1 {
                "pane"
            } else {
                "panes"
            };
            lines.push(format!(
                "    {} · {} {pane_label} · {} running",
                item.working_directory, item.pane_count, item.running_pane_count
            ));
        }
        while lines.len() < SESSION_PICKER_FIRST_ITEM_ROW + SESSION_PICKER_PAGE_ITEMS * 2 {
            lines.push(String::new());
        }
        lines.extend([
            String::new(),
            "↑/↓ or J/K select · Enter open · N new · Escape cancel".to_owned(),
        ]);
        picker_terminal_snapshot(*synthetic_id, *revision, lines)
    }
}

fn picker_terminal_snapshot(
    splint_id: SplintId,
    revision: u64,
    lines: Vec<String>,
) -> TerminalSnapshot {
    let columns = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1)
        .clamp(72, 120);
    let rows = lines.len().max(24);
    let attributes = CellAttributes {
        bold: false,
        dim: false,
        italic: false,
        underline: UnderlineStyle::None,
        underline_color_source: ColorSource::Default,
        underline_color: 0,
        strikethrough: false,
        blink: false,
        conceal: false,
        reverse: false,
        foreground_source: ColorSource::Default,
        foreground: 0,
        background_source: ColorSource::Default,
        background: 0,
    };
    let mut visible_rows: Vec<_> = lines
        .into_iter()
        .map(|line| TerminalRow {
            row_id: None,
            linebreak: false,
            cells: line
                .chars()
                .take(columns)
                .map(|character| TerminalCell {
                    content: character.to_string(),
                    spacer_remaining: None,
                    attributes,
                })
                .collect(),
        })
        .collect();
    visible_rows.resize_with(rows, || TerminalRow {
        row_id: None,
        linebreak: false,
        cells: Vec::new(),
    });
    for (index, row) in visible_rows.iter_mut().enumerate() {
        row.row_id = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1));
    }
    TerminalSnapshot {
        splint_id,
        incarnation: 1,
        revision,
        columns,
        rows,
        cursor_column: -1,
        cursor_row: -1,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: TerminalInputModes {
            application_cursor: false,
            application_keypad: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor_visible: false,
            cursor_blink: false,
            mouse_tracking: MouseTracking::None,
            sgr_mouse: false,
        },
        palette: vec![0; 256],
        default_colors: [0x00f4_f0e8, 0x0014_1820, 0x00e0_a030],
        title: "Recent Dojos".to_owned(),
        visible_rows,
        history_generation: 1,
        oldest_available_scrollback_row_id: None,
        newest_available_scrollback_row_id: None,
        scrollback_rows: Vec::new(),
        available_scrollback_rows: 0,
        omitted_oldest_scrollback_rows: 0,
        images: None,
        exited_code: None,
        exited_signal: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc as std_mpsc;

    use super::*;

    #[test]
    fn inline_picker_state_has_no_terminal_snapshot_identity() {
        let picker = SessionPickerUi::inline(vec![SessionPickerItem {
            display_title: "work / editor".to_owned(),
            working_directory: "/work".to_owned(),
            pane_count: 2,
            running_pane_count: 2,
        }]);
        assert!(picker.is_inline());
        assert_eq!(picker.selected_target(), PickerHitTarget::New);
        assert!(matches!(picker.host, SessionPickerHost::Inline));
    }

    #[test]
    fn session_picker_wraps_pages_and_maps_visible_rows() {
        let (decision, _receiver) = std_mpsc::channel();
        let items = (0..10)
            .map(|index| SessionPickerItem {
                display_title: format!("session {index}"),
                working_directory: format!("/tmp/{index}"),
                pane_count: 1,
                running_pane_count: 1,
            })
            .collect();
        let mut picker = SessionPickerUi::new(items, decision);
        assert_eq!(picker.selected_decision(), SessionPickerDecision::New);
        picker.move_selection(-1);
        assert_eq!(picker.selected_decision(), SessionPickerDecision::Open(9));
        assert_eq!(picker.visible_start, 3);
        assert_eq!(
            picker.select_row(SESSION_PICKER_FIRST_ITEM_ROW),
            Some(SessionPickerDecision::Open(3))
        );
        assert_eq!(picker.selected_decision(), SessionPickerDecision::Open(3));
        let snapshot = picker.snapshot();
        assert!(snapshot.validate().is_ok());
        assert!(
            snapshot.visible_rows[SESSION_PICKER_FIRST_ITEM_ROW]
                .cells
                .iter()
                .map(|cell| cell.content.as_str())
                .collect::<String>()
                .starts_with('›')
        );
    }

    #[test]
    fn session_picker_adapts_visibility_for_empty_and_large_catalogs() {
        for count in [0, 1, 7, 8, 64, 256] {
            let (decision, _receiver) = std_mpsc::channel();
            let items = (0..count)
                .map(|index| SessionPickerItem {
                    display_title: format!("session {index}"),
                    working_directory: format!("/tmp/{index}"),
                    pane_count: 1,
                    running_pane_count: 1,
                })
                .collect();
            let mut picker = SessionPickerUi::new(items, decision);
            picker.move_selection(-1);
            let expected = if count == 0 {
                SessionPickerDecision::New
            } else {
                SessionPickerDecision::Open(count - 1)
            };
            assert_eq!(picker.selected_decision(), expected);
            picker.ensure_selected_visible(3);
            assert!(picker.visible_start <= count.saturating_sub(3));
            if count > 0 {
                let selected = picker.selected - 1;
                assert!(selected >= picker.visible_start);
                assert!(selected < picker.visible_start + 3.min(count));
            }
            picker.select_first();
            assert_eq!(picker.selected_decision(), SessionPickerDecision::New);
            assert_eq!(picker.visible_start, 0);
            picker.select_last();
            assert_eq!(picker.selected_decision(), expected);
        }
    }
}
