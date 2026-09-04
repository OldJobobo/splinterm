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
    pub breadcrumb: String,
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
    filtered_indices: Vec<usize>,
    query: String,
    search_active: bool,
    selected: usize,
    visible_start: usize,
    hovered: Option<PickerHitTarget>,
    new_enabled: bool,
    new_blocker: Option<&'static str>,
    host: SessionPickerHost,
}

const SESSION_PICKER_PAGE_ITEMS: usize = 7;
const SESSION_PICKER_NEW_ROW: usize = 3;
const SESSION_PICKER_FIRST_ITEM_ROW: usize = 5;

impl SessionPickerUi {
    #[must_use]
    pub fn new(items: Vec<SessionPickerItem>, decision: StdSender<SessionPickerDecision>) -> Self {
        let selected = usize::from(!items.is_empty());
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            query: String::new(),
            search_active: false,
            selected,
            visible_start: 0,
            hovered: None,
            new_enabled: true,
            new_blocker: None,
            host: SessionPickerHost::Standalone {
                revision: 0,
                synthetic_id: SplintId::new(),
                decision,
            },
        }
    }

    pub(crate) fn inline(
        items: Vec<SessionPickerItem>,
        new_enabled: bool,
        new_blocker: Option<&'static str>,
        initial_row: Option<usize>,
    ) -> Self {
        let filtered_indices = (0..items.len()).collect::<Vec<_>>();
        let selected = initial_row
            .filter(|index| *index < items.len())
            .map_or_else(|| usize::from(!items.is_empty()), |index| index + 1);
        Self {
            selected,
            items,
            filtered_indices,
            query: String::new(),
            search_active: false,
            visible_start: 0,
            hovered: None,
            new_enabled,
            new_blocker,
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

    pub(crate) fn items(&self) -> impl Iterator<Item = &SessionPickerItem> {
        self.filtered_indices
            .iter()
            .filter_map(|index| self.items.get(*index))
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(crate) const fn search_active(&self) -> bool {
        self.search_active
    }

    pub(crate) fn begin_search(&mut self) -> bool {
        if self.search_active {
            return false;
        }
        self.search_active = true;
        true
    }

    pub(crate) fn clear_search(&mut self) -> bool {
        if !self.search_active && self.query.is_empty() {
            return false;
        }
        self.search_active = false;
        self.query.clear();
        self.apply_search();
        true
    }

    pub(crate) fn append_search(&mut self, text: &str) -> bool {
        let remaining = 64_usize.saturating_sub(self.query.chars().count());
        let addition = text
            .chars()
            .filter(|character| !character.is_control())
            .take(remaining)
            .collect::<String>();
        if addition.is_empty() {
            return false;
        }
        self.search_active = true;
        self.query.push_str(&addition);
        self.apply_search();
        true
    }

    pub(crate) fn backspace_search(&mut self) -> bool {
        if self.query.pop().is_none() {
            return false;
        }
        self.apply_search();
        true
    }

    fn apply_search(&mut self) {
        let needle = self.query.to_lowercase();
        let tokens = needle.split_whitespace().collect::<Vec<_>>();
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let labels = format!(
                    "{} {}",
                    item.display_title.to_lowercase(),
                    item.breadcrumb.to_lowercase()
                );
                tokens
                    .iter()
                    .all(|token| labels.contains(token))
                    .then_some(index)
            })
            .collect();
        self.selected = usize::from(!self.filtered_indices.is_empty());
        self.visible_start = 0;
        self.hovered = None;
    }

    pub(crate) const fn new_enabled(&self) -> bool {
        self.new_enabled
    }

    pub(crate) const fn new_blocker(&self) -> Option<&'static str> {
        self.new_blocker
    }

    pub(crate) fn layout_state(&self) -> (usize, usize, usize) {
        (
            self.filtered_indices.len(),
            self.selected,
            self.visible_start,
        )
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

    pub(crate) fn selected_decision(&self) -> Option<SessionPickerDecision> {
        if self.selected == 0 {
            self.new_enabled.then_some(SessionPickerDecision::New)
        } else {
            self.filtered_indices
                .get(self.selected - 1)
                .copied()
                .map(SessionPickerDecision::Open)
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let first = usize::from(!self.new_enabled && !self.filtered_indices.is_empty());
        let count = self
            .filtered_indices
            .len()
            .saturating_add(1)
            .saturating_sub(first);
        if count == 0 {
            return;
        }
        let current = self.selected.saturating_sub(first).min(count - 1);
        let magnitude = delta.unsigned_abs() % count;
        let next = if delta.is_negative() {
            current.saturating_add(count).saturating_sub(magnitude) % count
        } else {
            current.saturating_add(magnitude) % count
        };
        self.selected = next.saturating_add(first);
        self.ensure_selected_visible(SESSION_PICKER_PAGE_ITEMS);
    }

    pub(crate) fn select_first(&mut self) {
        self.selected = usize::from(!self.new_enabled && !self.filtered_indices.is_empty());
        self.visible_start = 0;
    }

    pub(crate) fn select_last(&mut self) {
        self.selected = self.filtered_indices.len();
        self.ensure_selected_visible(SESSION_PICKER_PAGE_ITEMS);
    }

    fn ensure_selected_visible(&mut self, visible_count: usize) {
        if self.filtered_indices.is_empty() || self.selected == 0 || visible_count == 0 {
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
            .min(self.filtered_indices.len().saturating_sub(visible_count));
    }

    pub(crate) fn select_row(&mut self, row: usize) -> Option<SessionPickerDecision> {
        if row == SESSION_PICKER_NEW_ROW || row == SESSION_PICKER_NEW_ROW + 1 {
            if !self.new_enabled {
                return None;
            }
            self.selected = 0;
            return Some(SessionPickerDecision::New);
        }
        let relative = row.checked_sub(SESSION_PICKER_FIRST_ITEM_ROW)?;
        let slot = relative / 2;
        if slot >= SESSION_PICKER_PAGE_ITEMS {
            return None;
        }
        let item = self.visible_start.checked_add(slot)?;
        let original_index = *self.filtered_indices.get(item)?;
        self.selected = item + 1;
        Some(SessionPickerDecision::Open(original_index))
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
        for (visible_index, original_index) in self
            .filtered_indices
            .iter()
            .enumerate()
            .skip(self.visible_start)
            .take(SESSION_PICKER_PAGE_ITEMS)
        {
            let Some(item) = self.items.get(*original_index) else {
                continue;
            };
            let selected = self.selected == visible_index + 1;
            lines.push(format!("{}{}", marker(selected), item.display_title));
            let pane_label = if item.pane_count == 1 {
                "pane"
            } else {
                "panes"
            };
            let detail = if selected && !item.working_directory.is_empty() {
                format!("{} · {}", item.breadcrumb, item.working_directory)
            } else {
                item.breadcrumb.clone()
            };
            lines.push(format!(
                "    {detail} · {} {pane_label} · {} running",
                item.pane_count, item.running_pane_count
            ));
        }
        while lines.len() < SESSION_PICKER_FIRST_ITEM_ROW + SESSION_PICKER_PAGE_ITEMS * 2 {
            lines.push(String::new());
        }
        lines.extend([
            String::new(),
            "/ search · ↑/↓ or J/K select · Enter open · Ctrl+N new · Escape cancel".to_owned(),
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
        let picker = SessionPickerUi::inline(
            vec![SessionPickerItem {
                display_title: "editor".to_owned(),
                breadcrumb: "work / editor".to_owned(),
                working_directory: "/work".to_owned(),
                pane_count: 2,
                running_pane_count: 2,
            }],
            true,
            None,
            Some(0),
        );
        assert!(picker.is_inline());
        assert_eq!(picker.selected_target(), PickerHitTarget::Open(0));
        assert!(matches!(picker.host, SessionPickerHost::Inline));
    }

    #[test]
    fn session_picker_wraps_pages_and_maps_visible_rows() {
        let (decision, _receiver) = std_mpsc::channel();
        let items = (0..10)
            .map(|index| SessionPickerItem {
                display_title: format!("session {index}"),
                breadcrumb: format!("work / session {index}"),
                working_directory: format!("/tmp/{index}"),
                pane_count: 1,
                running_pane_count: 1,
            })
            .collect();
        let mut picker = SessionPickerUi::new(items, decision);
        assert_eq!(
            picker.selected_decision(),
            Some(SessionPickerDecision::Open(0))
        );
        picker.move_selection(-2);
        assert_eq!(
            picker.selected_decision(),
            Some(SessionPickerDecision::Open(9))
        );
        assert_eq!(picker.visible_start, 3);
        assert_eq!(
            picker.select_row(SESSION_PICKER_FIRST_ITEM_ROW),
            Some(SessionPickerDecision::Open(3))
        );
        assert_eq!(
            picker.selected_decision(),
            Some(SessionPickerDecision::Open(3))
        );
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
    fn inline_picker_search_is_stable_bounded_and_excludes_cwd() {
        let items = vec![
            SessionPickerItem {
                display_title: "Éditor".to_owned(),
                breadcrumb: "work / Éditor".to_owned(),
                working_directory: "/secret/match".to_owned(),
                pane_count: 1,
                running_pane_count: 1,
            },
            SessionPickerItem {
                display_title: "Éditor".to_owned(),
                breadcrumb: "other / Éditor".to_owned(),
                working_directory: "/tmp".to_owned(),
                pane_count: 1,
                running_pane_count: 1,
            },
        ];
        let mut picker = SessionPickerUi::inline(items, false, Some("Unavailable"), Some(0));
        assert!(picker.begin_search());
        assert!(picker.append_search("ÉDITOR"));
        assert_eq!(picker.items().count(), 2);
        assert_eq!(
            picker.selected_decision(),
            Some(SessionPickerDecision::Open(0))
        );
        picker.move_selection(1);
        assert_eq!(
            picker.selected_decision(),
            Some(SessionPickerDecision::Open(1))
        );
        assert!(picker.clear_search());
        assert!(!picker.search_active());
        assert!(picker.append_search("match"));
        assert_eq!(picker.items().count(), 0);
        assert_eq!(picker.selected_decision(), None);
        assert!(picker.clear_search());
        assert!(picker.append_search(&"x".repeat(100)));
        assert_eq!(picker.query().chars().count(), 64);
    }

    #[test]
    fn session_picker_adapts_visibility_for_empty_and_large_catalogs() {
        for count in [0, 1, 7, 8, 64, 256] {
            let (decision, _receiver) = std_mpsc::channel();
            let items = (0..count)
                .map(|index| SessionPickerItem {
                    display_title: format!("session {index}"),
                    breadcrumb: format!("work / session {index}"),
                    working_directory: format!("/tmp/{index}"),
                    pane_count: 1,
                    running_pane_count: 1,
                })
                .collect();
            let mut picker = SessionPickerUi::new(items, decision);
            picker.select_first();
            picker.move_selection(-1);
            let expected = if count == 0 {
                SessionPickerDecision::New
            } else {
                SessionPickerDecision::Open(count - 1)
            };
            assert_eq!(picker.selected_decision(), Some(expected));
            picker.ensure_selected_visible(3);
            assert!(picker.visible_start <= count.saturating_sub(3));
            if count > 0 {
                let selected = picker.selected - 1;
                assert!(selected >= picker.visible_start);
                assert!(selected < picker.visible_start + 3.min(count));
            }
            picker.select_first();
            assert_eq!(picker.selected_decision(), Some(SessionPickerDecision::New));
            assert_eq!(picker.visible_start, 0);
            picker.select_last();
            assert_eq!(picker.selected_decision(), Some(expected));
        }
    }
}
