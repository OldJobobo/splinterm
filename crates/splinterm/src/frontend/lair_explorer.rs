//! Pure Window-local state for the optional Lair explorer.

use std::collections::HashSet;

use splinterm_core::LairRetention;

use super::text_edit::BoundedTextEditor;
use crate::navigation_projection::{
    EndpointFreshness, NavigationAvailability, NavigationExplorerTarget, NavigationExplorerView,
    NavigationNodeId, WindowAttachment,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LairExplorerRowKind {
    Lair,
    Dojo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LairExplorerRow {
    pub(crate) id: NavigationNodeId,
    pub(crate) kind: LairExplorerRowKind,
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) level: usize,
    pub(crate) expanded: Option<bool>,
    pub(crate) current: bool,
    pub(crate) enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LairExplorerDecision {
    Toggle(NavigationNodeId),
    OpenDojo(NavigationExplorerTarget),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExplorerLoadState {
    Waiting,
    Ready,
    Disconnected,
}

#[derive(Clone, Debug)]
pub(crate) struct LairExplorerUi {
    visible: bool,
    focused: bool,
    search_active: bool,
    query: BoundedTextEditor,
    expanded: HashSet<NavigationNodeId>,
    selected: Option<NavigationNodeId>,
    view: Option<NavigationExplorerView>,
    load_state: ExplorerLoadState,
}

impl Default for LairExplorerUi {
    fn default() -> Self {
        Self {
            visible: false,
            focused: false,
            search_active: false,
            query: BoundedTextEditor::new(String::new(), 256, 64, true),
            expanded: HashSet::new(),
            selected: None,
            view: None,
            load_state: ExplorerLoadState::Waiting,
        }
    }
}

impl LairExplorerUi {
    pub(crate) const fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) const fn focused(&self) -> bool {
        self.focused
    }

    pub(crate) const fn search_active(&self) -> bool {
        self.search_active
    }

    pub(crate) fn query(&self) -> &str {
        self.query.text()
    }

    pub(crate) fn editor_mut(&mut self) -> &mut BoundedTextEditor {
        &mut self.query
    }

    pub(crate) fn toggle_visibility(&mut self) -> bool {
        self.visible = !self.visible;
        if !self.visible {
            self.focused = false;
            self.search_active = false;
        }
        self.visible
    }

    pub(crate) fn focus(&mut self) {
        self.visible = true;
        self.focused = true;
    }

    pub(crate) fn return_to_terminal(&mut self) {
        self.focused = false;
        self.search_active = false;
    }

    pub(crate) fn set_view(&mut self, view: NavigationExplorerView) {
        let retained = view
            .lairs
            .iter()
            .map(|lair| lair.id)
            .collect::<HashSet<_>>();
        self.expanded.retain(|id| retained.contains(id));
        let selected_exists = self.selected.is_some_and(|selected| {
            view.lairs.iter().any(|lair| {
                lair.id == selected || lair.dojos.iter().any(|dojo| dojo.id == selected)
            })
        });
        if !selected_exists {
            self.selected = view
                .current
                .and_then(|current| {
                    view.lairs
                        .iter()
                        .find(|lair| lair.dojos.iter().any(|dojo| dojo.id == current))
                        .map(|lair| lair.id)
                })
                .or_else(|| view.lairs.first().map(|lair| lair.id));
        }
        self.view = Some(view);
        self.load_state = ExplorerLoadState::Ready;
    }

    pub(crate) fn mark_disconnected(&mut self) {
        self.load_state = ExplorerLoadState::Disconnected;
        if let Some(view) = &mut self.view {
            view.freshness = EndpointFreshness::Disconnected;
            for dojo in view.lairs.iter_mut().flat_map(|lair| &mut lair.dojos) {
                dojo.target.capability.availability = NavigationAvailability::Disabled(
                    crate::navigation_projection::NavigationBlocker::Disconnected,
                );
            }
        }
    }

    pub(crate) fn disconnected(&self) -> bool {
        self.load_state == ExplorerLoadState::Disconnected
    }

    pub(crate) fn status_message(&self) -> Option<&'static str> {
        if self.load_state == ExplorerLoadState::Disconnected {
            Some("Navigation unavailable · R retry")
        } else if self.view.is_none() {
            Some("Loading navigation…")
        } else if self.rows().is_empty() {
            if self.query.text().is_empty() {
                Some("No saved Lairs")
            } else {
                Some("No matches")
            }
        } else {
            None
        }
    }

    pub(crate) fn breadcrumb(&self) -> Option<String> {
        let view = self.view.as_ref()?;
        let current = view.current?;
        view.lairs.iter().find_map(|lair| {
            lair.dojos
                .iter()
                .find(|dojo| dojo.id == current)
                .map(|dojo| format!("{} / {}", lair.label, dojo.label))
        })
    }

    pub(crate) fn reveal_current(&mut self) -> bool {
        let Some(view) = &self.view else {
            return false;
        };
        let Some(current) = view.current else {
            return false;
        };
        let Some(parent) = view
            .lairs
            .iter()
            .find(|lair| lair.dojos.iter().any(|dojo| dojo.id == current))
            .map(|lair| lair.id)
        else {
            return false;
        };
        let expanded = self.expanded.insert(parent);
        let selected = self.selected != Some(current);
        self.selected = Some(current);
        expanded || selected
    }

    pub(crate) fn rows(&self) -> Vec<LairExplorerRow> {
        let Some(view) = &self.view else {
            return Vec::new();
        };
        let query = normalized_query(self.query.text());
        let searching = !query.is_empty();
        let mut rows = Vec::new();
        for lair in &view.lairs {
            let lair_matches = matches_query(&lair.label, &query);
            let matching_dojos = lair
                .dojos
                .iter()
                .filter(|dojo| matches_query(&dojo.label, &query))
                .collect::<Vec<_>>();
            if searching && !lair_matches && matching_dojos.is_empty() {
                continue;
            }
            let expanded = self.expanded.contains(&lair.id);
            rows.push(LairExplorerRow {
                id: lair.id,
                kind: LairExplorerRowKind::Lair,
                label: lair.label.clone(),
                status: retention_label(lair.retention).to_owned(),
                level: 1,
                expanded: Some(expanded),
                current: lair.current_here,
                enabled: true,
            });
            if expanded || searching {
                let dojos: Box<dyn Iterator<Item = _>> = if searching && !lair_matches {
                    Box::new(matching_dojos.into_iter())
                } else {
                    Box::new(lair.dojos.iter())
                };
                rows.extend(dojos.map(|dojo| {
                    let (enabled, blocker) = match dojo.target.capability.availability {
                        NavigationAvailability::Enabled => (true, None),
                        NavigationAvailability::Disabled(blocker) => {
                            (false, Some(blocker.message()))
                        }
                    };
                    let attachment = match dojo.attachment {
                        WindowAttachment::Here => "Here",
                        WindowAttachment::NotHere => "Not here",
                    };
                    LairExplorerRow {
                        id: dojo.id,
                        kind: LairExplorerRowKind::Dojo,
                        label: dojo.label.clone(),
                        status: blocker.map_or_else(
                            || attachment.to_owned(),
                            |reason| format!("{attachment} · {reason}"),
                        ),
                        level: 2,
                        expanded: None,
                        current: dojo.active_here,
                        enabled,
                    }
                }));
            }
        }
        rows
    }

    pub(crate) fn selected(&self) -> Option<NavigationNodeId> {
        self.selected
    }

    pub(crate) fn select(&mut self, id: NavigationNodeId) -> bool {
        if self.rows().iter().any(|row| row.id == id) && self.selected != Some(id) {
            self.selected = Some(id);
            true
        } else {
            false
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) -> bool {
        let rows = self.rows();
        if rows.is_empty() {
            self.selected = None;
            return false;
        }
        let current = self
            .selected
            .and_then(|selected| rows.iter().position(|row| row.id == selected))
            .unwrap_or(0);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta.unsigned_abs())
                .min(rows.len() - 1)
        };
        let changed = self.selected != Some(rows[next].id);
        self.selected = Some(rows[next].id);
        changed
    }

    pub(crate) fn select_edge(&mut self, end: bool) -> bool {
        let rows = self.rows();
        let next = if end { rows.last() } else { rows.first() }.map(|row| row.id);
        let changed = self.selected != next;
        self.selected = next;
        changed
    }

    pub(crate) fn toggle_selected(&mut self) -> bool {
        let Some(NavigationNodeId::Lair(_)) = self.selected else {
            return false;
        };
        let selected = self.selected.expect("matched selected Lair");
        if !self.expanded.remove(&selected) {
            self.expanded.insert(selected);
        }
        self.ensure_selection_visible();
        true
    }

    pub(crate) fn move_left(&mut self) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        if matches!(selected, NavigationNodeId::Lair(_)) && self.expanded.remove(&selected) {
            return true;
        }
        let Some(view) = &self.view else {
            return false;
        };
        let parent = view
            .lairs
            .iter()
            .find(|lair| lair.dojos.iter().any(|dojo| dojo.id == selected))
            .map(|lair| lair.id);
        if parent.is_some() && parent != self.selected {
            self.selected = parent;
            true
        } else {
            false
        }
    }

    pub(crate) fn move_right(&mut self) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        let Some(view) = &self.view else {
            return false;
        };
        let Some(lair) = view.lairs.iter().find(|lair| lair.id == selected) else {
            return false;
        };
        if !self.expanded.contains(&selected) {
            return self.expanded.insert(selected);
        }
        if let Some(first) = lair.dojos.first() {
            self.selected = Some(first.id);
            true
        } else {
            false
        }
    }

    pub(crate) fn decision(&mut self) -> Option<LairExplorerDecision> {
        let selected = self.selected?;
        if matches!(selected, NavigationNodeId::Lair(_)) {
            self.toggle_selected();
            return Some(LairExplorerDecision::Toggle(selected));
        }
        let view = self.view.as_ref()?;
        let target = view
            .lairs
            .iter()
            .flat_map(|lair| &lair.dojos)
            .find(|dojo| dojo.id == selected)?
            .target;
        target
            .capability
            .is_enabled()
            .then_some(LairExplorerDecision::OpenDojo(target))
    }

    pub(crate) fn begin_search(&mut self) -> bool {
        let changed = !self.search_active;
        self.search_active = true;
        changed
    }

    pub(crate) fn append_search(&mut self, text: &str) -> bool {
        if !self.search_active || !self.query.insert(text) {
            return false;
        }
        self.ensure_selection_visible();
        true
    }

    pub(crate) fn backspace_search(&mut self) -> bool {
        if !self.search_active || !self.query.backspace() {
            return false;
        }
        self.ensure_selection_visible();
        true
    }

    pub(crate) fn escape(&mut self) -> bool {
        if self.query.text().is_empty() {
            self.return_to_terminal();
        } else {
            self.query = BoundedTextEditor::new(String::new(), 256, 64, true);
            self.ensure_selection_visible();
        }
        true
    }

    fn ensure_selection_visible(&mut self) {
        let rows = self.rows();
        if !self
            .selected
            .is_some_and(|selected| rows.iter().any(|row| row.id == selected))
        {
            self.selected = rows.first().map(|row| row.id);
        }
    }
}

fn retention_label(retention: LairRetention) -> &'static str {
    match retention {
        LairRetention::Disposable => "Disposable",
        LairRetention::Saved => "Saved",
        LairRetention::Pinned => "Pinned",
    }
}

fn normalized_query(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

fn matches_query(label: &str, query: &[String]) -> bool {
    let label = label.to_lowercase();
    query.iter().all(|token| label.contains(token))
}

#[cfg(test)]
mod tests {
    use splinterm_core::{DojoId, LairId, TopologyRevision};

    use super::*;
    use crate::navigation_projection::{
        NavigationAction, NavigationAvailability, NavigationCapability, NavigationExplorerDojo,
        NavigationExplorerLair, NavigationExplorerTarget,
    };

    fn view() -> NavigationExplorerView {
        let lair = LairId::new();
        let other_lair = LairId::new();
        let dojo = DojoId::new();
        let other_dojo = DojoId::new();
        NavigationExplorerView {
            topology_revision: TopologyRevision::new(7),
            freshness: EndpointFreshness::Current,
            current: Some(NavigationNodeId::Dojo {
                lair_id: lair,
                dojo_id: dojo,
            }),
            lairs: vec![
                NavigationExplorerLair {
                    id: NavigationNodeId::Lair(lair),
                    label: "work".into(),
                    retention: LairRetention::Saved,
                    current_here: true,
                    dojos: vec![NavigationExplorerDojo {
                        id: NavigationNodeId::Dojo {
                            lair_id: lair,
                            dojo_id: dojo,
                        },
                        label: "editor".into(),
                        attachment: WindowAttachment::Here,
                        active_here: true,
                        target: NavigationExplorerTarget {
                            topology_revision: TopologyRevision::new(7),
                            lair_id: lair,
                            dojo_id: dojo,
                            capability: NavigationCapability {
                                action: NavigationAction::ActivateDojo,
                                availability: NavigationAvailability::Enabled,
                            },
                        },
                    }],
                },
                NavigationExplorerLair {
                    id: NavigationNodeId::Lair(other_lair),
                    label: "notes".into(),
                    retention: LairRetention::Pinned,
                    current_here: false,
                    dojos: vec![NavigationExplorerDojo {
                        id: NavigationNodeId::Dojo {
                            lair_id: other_lair,
                            dojo_id: other_dojo,
                        },
                        label: "draft".into(),
                        attachment: WindowAttachment::NotHere,
                        active_here: false,
                        target: NavigationExplorerTarget {
                            topology_revision: TopologyRevision::new(7),
                            lair_id: other_lair,
                            dojo_id: other_dojo,
                            capability: NavigationCapability {
                                action: NavigationAction::AttachDojo,
                                availability: NavigationAvailability::Enabled,
                            },
                        },
                    }],
                },
            ],
        }
    }

    #[test]
    fn explorer_is_default_hidden_and_focus_actions_are_independent() {
        let mut explorer = LairExplorerUi::default();
        assert!(!explorer.visible());
        explorer.toggle_visibility();
        assert!(explorer.visible());
        assert!(!explorer.focused());
        explorer.focus();
        assert!(explorer.focused());
        explorer.return_to_terminal();
        assert!(explorer.visible());
        assert!(!explorer.focused());
    }

    #[test]
    fn reveal_and_navigation_preserve_stable_hierarchy() {
        let mut explorer = LairExplorerUi::default();
        let view = view();
        let current = view.current;
        explorer.set_view(view);
        assert_eq!(explorer.breadcrumb().as_deref(), Some("work / editor"));
        assert!(explorer.reveal_current());
        assert_eq!(explorer.rows().len(), 3);
        assert_eq!(explorer.selected(), current);
        assert!(explorer.move_left());
        assert!(matches!(
            explorer.selected(),
            Some(NavigationNodeId::Lair(_))
        ));
        assert!(explorer.move_right());
        assert_eq!(explorer.selected(), current);
    }

    #[test]
    fn search_is_bounded_label_only_and_keeps_parent_context() {
        let mut explorer = LairExplorerUi::default();
        explorer.set_view(view());
        explorer.begin_search();
        assert!(explorer.append_search("draft"));
        let rows = explorer.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "notes");
        assert_eq!(rows[1].label, "draft");
        assert!(!explorer.append_search("\n\u{202e}"));
    }

    #[test]
    fn empty_and_disconnected_states_are_explicit() {
        let mut explorer = LairExplorerUi::default();
        assert_eq!(explorer.status_message(), Some("Loading navigation…"));
        explorer.mark_disconnected();
        assert!(explorer.disconnected());
        assert_eq!(
            explorer.status_message(),
            Some("Navigation unavailable · R retry")
        );
    }

    #[test]
    fn dojo_decision_retains_exact_revision_and_identity() {
        let mut explorer = LairExplorerUi::default();
        let view = view();
        let target = view.lairs[0].dojos[0].target;
        explorer.set_view(view);
        explorer.reveal_current();
        assert_eq!(
            explorer.decision(),
            Some(LairExplorerDecision::OpenDojo(target))
        );
    }
}
