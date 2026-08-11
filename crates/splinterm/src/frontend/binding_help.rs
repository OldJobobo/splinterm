//! Read-only binding-help rows derived from one resolved keymap.

use crate::keymap::ResolvedKeymap;

pub(crate) const BINDING_HELP_PAGE_ITEMS: usize = 7;
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BindingHelpRow {
    pub(crate) shortcut: String,
    pub(crate) action: String,
    pub(crate) source: String,
    pub(crate) compact: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BindingHelpUi {
    rows: Vec<BindingHelpRow>,
    selected: usize,
    visible_start: usize,
}

impl BindingHelpUi {
    pub(crate) fn new(keymap: &ResolvedKeymap) -> Self {
        let mut rows = keymap
            .bindings()
            .iter()
            .map(|binding| BindingHelpRow {
                shortcut: binding.display().to_owned(),
                action: binding.action().config_name().to_owned(),
                source: binding.source().short_label(),
                compact: format!("{} — {}", binding.display(), binding.action().config_name()),
            })
            .collect::<Vec<_>>();
        rows.extend([
            BindingHelpRow {
                shortcut: "copy: h/j/k/l · arrows · Home/End · PgUp/PgDn".into(),
                action: "move copy cursor".into(),
                source: "copy mode".into(),
                compact: "copy: h/j/k/l · arrows · Home/End · PgUp/PgDn — move".into(),
            },
            BindingHelpRow {
                shortcut: "copy: v".into(),
                action: "begin selection".into(),
                source: "copy mode".into(),
                compact: "copy: v — begin selection".into(),
            },
            BindingHelpRow {
                shortcut: "copy: y / Escape".into(),
                action: "copy and exit / cancel".into(),
                source: "copy mode".into(),
                compact: "copy: y / Escape — copy and exit / cancel".into(),
            },
        ]);
        Self {
            rows,
            selected: 0,
            visible_start: 0,
        }
    }

    pub(crate) fn rows(&self) -> &[BindingHelpRow] {
        &self.rows
    }

    pub(crate) const fn selected_index(&self) -> usize {
        self.selected
    }

    pub(crate) const fn visible_start(&self) -> usize {
        self.visible_start
    }

    pub(crate) fn move_selection(&mut self, delta: isize) -> bool {
        if self.rows.is_empty() || delta == 0 {
            return false;
        }
        let previous = self.selected;
        let magnitude = delta.unsigned_abs().min(self.rows.len().saturating_sub(1));
        self.selected = if delta.is_negative() {
            self.selected.saturating_sub(magnitude)
        } else {
            self.selected
                .saturating_add(magnitude)
                .min(self.rows.len() - 1)
        };
        self.ensure_visible();
        self.selected != previous
    }

    pub(crate) fn select_first(&mut self) -> bool {
        let changed = self.selected != 0 || self.visible_start != 0;
        self.selected = 0;
        self.visible_start = 0;
        changed
    }

    pub(crate) fn select_last(&mut self) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let last = self.rows.len() - 1;
        let changed = self.selected != last;
        self.selected = last;
        self.ensure_visible();
        changed
    }

    fn ensure_visible(&mut self) {
        if self.selected < self.visible_start {
            self.visible_start = self.selected;
        } else if self.selected >= self.visible_start.saturating_add(BINDING_HELP_PAGE_ITEMS) {
            self.visible_start = self
                .selected
                .saturating_add(1)
                .saturating_sub(BINDING_HELP_PAGE_ITEMS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{ActionId, KeymapProfile, built_in_keymap};

    #[test]
    fn help_contains_every_resolved_binding_and_copy_mode_local_keys() {
        let keymap = built_in_keymap(KeymapProfile::OmarchyTmux);
        let help = BindingHelpUi::new(&keymap);
        assert_eq!(help.rows().len(), keymap.bindings().len() + 3);
        for binding in keymap.bindings() {
            assert!(help.rows().iter().any(|row| {
                row.shortcut == binding.display() && row.action == binding.action().config_name()
            }));
        }
        assert!(help.rows().iter().any(|row| {
            row.action == ActionId::BindingHelp.config_name() && row.shortcut == "Prefix ?"
        }));
        assert!(help.rows().iter().any(|row| {
            row.action == ActionId::CopyModeEnter.config_name() && row.shortcut == "Prefix ["
        }));
        assert!(help.rows().iter().any(|row| row.shortcut == "copy: v"));
    }

    #[test]
    fn help_navigation_is_bounded_and_keeps_selection_visible() {
        let keymap = built_in_keymap(KeymapProfile::OmarchyTmux);
        let mut help = BindingHelpUi::new(&keymap);
        assert!(help.move_selection(isize::MAX));
        assert_eq!(help.selected_index(), help.rows().len() - 1);
        assert!(help.visible_start() <= help.selected_index());
        assert!(help.select_first());
        assert_eq!(help.visible_start(), 0);
        assert!(help.select_last());
    }
}
