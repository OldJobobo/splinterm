//! Pure Window-local state for the optional Lair explorer.

use std::collections::HashSet;

use splinterm_core::{LairRetention, SplintId};

use super::{text_edit::BoundedTextEditor, topology::LairExplorerActivationTarget};
use crate::navigation_projection::{
    EndpointFreshness, NavigationAvailability, NavigationExplorerDojo,
    NavigationExplorerSplintTarget, NavigationExplorerTarget, NavigationExplorerView,
    NavigationLifecycle, NavigationNodeId, WindowAttachment,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LairExplorerRowKind {
    Lair,
    Dojo,
    Splint,
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
    pub(crate) pending: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LairExplorerDecision {
    Toggle(NavigationNodeId),
    OpenDojo(NavigationExplorerTarget),
    FocusSplint(NavigationExplorerSplintTarget),
}

impl LairExplorerDecision {
    pub(crate) const fn returns_focus_to_terminal(self) -> bool {
        matches!(self, Self::OpenDojo(_) | Self::FocusSplint(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LairExplorerPending {
    Dojo(NavigationExplorerTarget),
    Splint(NavigationExplorerSplintTarget),
}

impl LairExplorerPending {
    const fn from_decision(decision: LairExplorerDecision) -> Option<Self> {
        match decision {
            LairExplorerDecision::Toggle(_) => None,
            LairExplorerDecision::OpenDojo(target) => Some(Self::Dojo(target)),
            LairExplorerDecision::FocusSplint(target) => Some(Self::Splint(target)),
        }
    }

    const fn id(self) -> NavigationNodeId {
        match self {
            Self::Dojo(target) => NavigationNodeId::Dojo {
                lair_id: target.lair_id,
                dojo_id: target.dojo_id,
            },
            Self::Splint(target) => NavigationNodeId::Splint {
                lair_id: target.lair_id,
                dojo_id: target.dojo_id,
                splint_id: target.splint_id,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExplorerLoadState {
    Waiting,
    Refreshing,
    Ready,
    Stale,
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
    pending: Option<LairExplorerPending>,
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
            pending: None,
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
            .flat_map(|lair| std::iter::once(lair.id).chain(lair.dojos.iter().map(|dojo| dojo.id)))
            .collect::<HashSet<_>>();
        self.expanded.retain(|id| retained.contains(id));
        let selected_exists = self
            .selected
            .is_some_and(|selected| view_contains(&view, selected));
        let nearest_surviving = self.selected.and_then(|selected| {
            self.view
                .as_ref()
                .and_then(|previous| nearest_surviving_parent(previous, &view, selected))
        });
        if self.pending.is_some_and(|pending| {
            let pending = pending.id();
            !view.lairs.iter().any(|lair| {
                lair.id == pending
                    || lair.dojos.iter().any(|dojo| {
                        dojo.id == pending || dojo.splints.iter().any(|splint| splint.id == pending)
                    })
            })
        }) {
            self.pending = None;
        }
        if !selected_exists {
            self.selected = nearest_surviving.or_else(|| {
                view.current
                    .and_then(|current| {
                        view.lairs
                            .iter()
                            .find(|lair| {
                                lair.dojos.iter().any(|dojo| {
                                    dojo.id == current
                                        || dojo.splints.iter().any(|splint| splint.id == current)
                                })
                            })
                            .map(|lair| lair.id)
                    })
                    .or_else(|| view.lairs.first().map(|lair| lair.id))
            });
        }
        self.load_state = match view.freshness {
            EndpointFreshness::Current => ExplorerLoadState::Ready,
            EndpointFreshness::Refreshing => ExplorerLoadState::Refreshing,
            EndpointFreshness::Stale => ExplorerLoadState::Stale,
            EndpointFreshness::Disconnected => ExplorerLoadState::Disconnected,
        };
        self.view = Some(view);
    }

    pub(crate) fn begin_refresh(&mut self) {
        if let Some(view) = &mut self.view {
            view.freshness = EndpointFreshness::Refreshing;
            for dojo in view.lairs.iter_mut().flat_map(|lair| &mut lair.dojos) {
                dojo.target.capability.availability = NavigationAvailability::Disabled(
                    crate::navigation_projection::NavigationBlocker::Refreshing,
                );
                for splint in &mut dojo.splints {
                    splint.target.capability.availability = NavigationAvailability::Disabled(
                        crate::navigation_projection::NavigationBlocker::Refreshing,
                    );
                }
            }
        }
        self.load_state = ExplorerLoadState::Refreshing;
    }

    pub(crate) fn mark_disconnected(&mut self) {
        self.load_state = ExplorerLoadState::Disconnected;
        self.pending = None;
        if let Some(view) = &mut self.view {
            view.freshness = EndpointFreshness::Disconnected;
            for dojo in view.lairs.iter_mut().flat_map(|lair| &mut lair.dojos) {
                dojo.target.capability.availability = NavigationAvailability::Disabled(
                    crate::navigation_projection::NavigationBlocker::Disconnected,
                );
                for splint in &mut dojo.splints {
                    splint.target.capability.availability = NavigationAvailability::Disabled(
                        crate::navigation_projection::NavigationBlocker::Disconnected,
                    );
                }
            }
        }
    }

    pub(crate) fn set_pending(&mut self, decision: LairExplorerDecision) -> bool {
        let Some(pending) = LairExplorerPending::from_decision(decision) else {
            return false;
        };
        let id = pending.id();
        if self.pending.is_some() || !self.rows().iter().any(|row| row.id == id && row.enabled) {
            return false;
        }
        self.pending = Some(pending);
        true
    }

    /// Retire only the acknowledged request. Return whether terminal focus may resume.
    /// A matching local failure releases pending ownership, never keyboard ownership.
    pub(crate) fn finish_activation(
        &mut self,
        completed: LairExplorerActivationTarget,
        succeeded: bool,
    ) -> bool {
        let matches = self
            .pending
            .is_some_and(|pending| match (pending, completed) {
                (
                    LairExplorerPending::Dojo(pending),
                    LairExplorerActivationTarget::Dojo(completed),
                ) => {
                    pending.topology_revision == completed.topology_revision
                        && pending.lair_id == completed.lair_id
                        && pending.dojo_id == completed.dojo_id
                        && pending.capability.action == completed.action
                }
                (
                    LairExplorerPending::Splint(pending),
                    LairExplorerActivationTarget::Splint(completed),
                ) => pending == completed,
                _ => false,
            });
        if matches {
            if succeeded {
                self.pending = None;
            } else {
                self.fail_pending();
            }
        }
        matches && succeeded
    }

    pub(crate) fn activation_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Unrelated and stale failures cannot release a newer activation's ownership.
    pub(crate) fn fail_activation(
        &mut self,
        completed: Option<LairExplorerActivationTarget>,
    ) -> bool {
        let was_pending = self.activation_pending();
        if let Some(completed) = completed {
            self.finish_activation(completed, false);
        }
        was_pending && !self.activation_pending()
    }

    pub(crate) fn clear_pending(&mut self) -> bool {
        self.pending.take().is_some()
    }

    fn fail_pending(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        if let NavigationNodeId::Splint { splint_id, .. } = pending.id() {
            self.mark_stale_target(splint_id);
        }
        true
    }

    pub(crate) fn mark_stale_target(&mut self, splint_id: SplintId) {
        if let Some(splint) = self
            .view
            .iter_mut()
            .flat_map(|view| &mut view.lairs)
            .flat_map(|lair| &mut lair.dojos)
            .flat_map(|dojo| &mut dojo.splints)
            .find(|splint| {
                matches!(
                    splint.id,
                    NavigationNodeId::Splint {
                        splint_id: id,
                        ..
                    } if id == splint_id
                )
            })
        {
            splint.target.capability.availability = NavigationAvailability::Disabled(
                crate::navigation_projection::NavigationBlocker::Stale,
            );
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        matches!(
            self.load_state,
            ExplorerLoadState::Stale | ExplorerLoadState::Disconnected
        )
    }

    pub(crate) fn status_message(&self) -> Option<&'static str> {
        if self.load_state == ExplorerLoadState::Disconnected {
            Some("Navigation unavailable · R retry")
        } else if self.load_state == ExplorerLoadState::Stale {
            Some("Navigation is stale · R retry")
        } else if self.load_state == ExplorerLoadState::Refreshing && self.view.is_some() {
            Some("Refreshing navigation…")
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
            lair.dojos.iter().find_map(|dojo| {
                if dojo.id == current {
                    Some(format!("{} / {}", lair.label, dojo.label))
                } else {
                    dojo.splints
                        .iter()
                        .find(|splint| splint.id == current)
                        .map(|splint| format!("{} / {} / {}", lair.label, dojo.label, splint.label))
                }
            })
        })
    }

    pub(crate) fn reveal_current(&mut self) -> bool {
        let Some(view) = &self.view else {
            return false;
        };
        let Some(current) = view.current else {
            return false;
        };
        let Some((lair_id, dojo_id)) = view.lairs.iter().find_map(|lair| {
            lair.dojos.iter().find_map(|dojo| {
                (dojo.id == current || dojo.splints.iter().any(|splint| splint.id == current))
                    .then_some((lair.id, dojo.id))
            })
        }) else {
            return false;
        };
        let mut changed = self.expanded.insert(lair_id);
        if matches!(current, NavigationNodeId::Splint { .. }) {
            changed |= self.expanded.insert(dojo_id);
        }
        changed |= self.selected != Some(current);
        self.selected = Some(current);
        changed
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
            let lair_has_descendant_match = lair.dojos.iter().any(|dojo| {
                matches_query(&dojo.label, &query)
                    || dojo
                        .splints
                        .iter()
                        .any(|splint| matches_query(&splint.label, &query))
            });
            if searching && !lair_matches && !lair_has_descendant_match {
                continue;
            }
            let lair_expanded = self.expanded.contains(&lair.id);
            rows.push(LairExplorerRow {
                id: lair.id,
                kind: LairExplorerRowKind::Lair,
                label: lair.label.clone(),
                status: retention_label(lair.retention).to_owned(),
                level: 1,
                expanded: Some(lair_expanded),
                current: lair.current_here,
                enabled: true,
                pending: false,
            });
            if !lair_expanded && !searching {
                continue;
            }
            for dojo in &lair.dojos {
                self.push_dojo_rows(&mut rows, dojo, searching, lair_matches, &query);
            }
        }
        rows
    }

    fn push_dojo_rows(
        &self,
        rows: &mut Vec<LairExplorerRow>,
        dojo: &NavigationExplorerDojo,
        searching: bool,
        lair_matches: bool,
        query: &[String],
    ) {
        let dojo_matches = matches_query(&dojo.label, query);
        let descendant_matches = dojo
            .splints
            .iter()
            .any(|splint| matches_query(&splint.label, query));
        if searching && !lair_matches && !dojo_matches && !descendant_matches {
            return;
        }
        let (enabled, blocker) = availability(dojo.target.capability.availability);
        let attachment = match dojo.attachment {
            WindowAttachment::Here => "Here",
            WindowAttachment::NotHere => "Not here",
        };
        let dojo_expanded = self.expanded.contains(&dojo.id);
        let pending = self.pending.is_some_and(|pending| pending.id() == dojo.id);
        rows.push(LairExplorerRow {
            id: dojo.id,
            kind: LairExplorerRowKind::Dojo,
            label: dojo.label.clone(),
            status: row_status(pending, attachment, blocker),
            level: 2,
            expanded: Some(dojo_expanded),
            current: dojo.active_here,
            enabled: enabled && !pending,
            pending,
        });
        if !dojo_expanded && !searching {
            return;
        }
        for splint in &dojo.splints {
            if searching && !lair_matches && !dojo_matches && !matches_query(&splint.label, query) {
                continue;
            }
            let (enabled, blocker) = availability(splint.target.capability.availability);
            let lifecycle = lifecycle_label(splint.lifecycle);
            let pending = self
                .pending
                .is_some_and(|pending| pending.id() == splint.id);
            rows.push(LairExplorerRow {
                id: splint.id,
                kind: LairExplorerRowKind::Splint,
                label: splint.label.clone(),
                status: row_status(pending, lifecycle, blocker),
                level: 3,
                expanded: None,
                current: splint.focused_here,
                enabled: enabled && !pending,
                pending,
            });
        }
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
        let Some(selected @ (NavigationNodeId::Lair(_) | NavigationNodeId::Dojo { .. })) =
            self.selected
        else {
            return false;
        };
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
        let parent = view.lairs.iter().find_map(|lair| {
            lair.dojos.iter().find_map(|dojo| {
                if dojo.id == selected {
                    Some(lair.id)
                } else if dojo.splints.iter().any(|splint| splint.id == selected) {
                    Some(dojo.id)
                } else {
                    None
                }
            })
        });
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
        let first_child = view.lairs.iter().find_map(|lair| {
            if lair.id == selected {
                lair.dojos.first().map(|dojo| dojo.id)
            } else {
                lair.dojos
                    .iter()
                    .find(|dojo| dojo.id == selected)
                    .and_then(|dojo| dojo.splints.first().map(|splint| splint.id))
            }
        });
        if !matches!(
            selected,
            NavigationNodeId::Lair(_) | NavigationNodeId::Dojo { .. }
        ) {
            return false;
        }
        if !self.expanded.contains(&selected) {
            return self.expanded.insert(selected);
        }
        if let Some(first) = first_child {
            self.selected = Some(first);
            true
        } else {
            false
        }
    }

    pub(crate) fn decision(&mut self) -> Option<LairExplorerDecision> {
        let selected = self.selected?;
        if self.pending.is_some_and(|pending| pending.id() == selected) {
            return None;
        }
        if matches!(selected, NavigationNodeId::Lair(_)) {
            self.toggle_selected();
            return Some(LairExplorerDecision::Toggle(selected));
        }
        let view = self.view.as_ref()?;
        for dojo in view.lairs.iter().flat_map(|lair| &lair.dojos) {
            if dojo.id == selected {
                return dojo
                    .target
                    .capability
                    .is_enabled()
                    .then_some(LairExplorerDecision::OpenDojo(dojo.target));
            }
            if let Some(splint) = dojo.splints.iter().find(|splint| splint.id == selected) {
                return splint
                    .target
                    .capability
                    .is_enabled()
                    .then_some(LairExplorerDecision::FocusSplint(splint.target));
            }
        }
        None
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

fn view_contains(view: &NavigationExplorerView, selected: NavigationNodeId) -> bool {
    view.lairs.iter().any(|lair| {
        lair.id == selected
            || lair.dojos.iter().any(|dojo| {
                dojo.id == selected || dojo.splints.iter().any(|splint| splint.id == selected)
            })
    })
}

fn nearest_surviving_parent(
    previous: &NavigationExplorerView,
    current: &NavigationExplorerView,
    selected: NavigationNodeId,
) -> Option<NavigationNodeId> {
    previous.lairs.iter().find_map(|lair| {
        lair.dojos.iter().find_map(|dojo| {
            let selected_child = dojo.splints.iter().any(|splint| splint.id == selected);
            if selected_child && view_contains(current, dojo.id) {
                Some(dojo.id)
            } else if (selected_child || dojo.id == selected) && view_contains(current, lair.id) {
                Some(lair.id)
            } else {
                None
            }
        })
    })
}

fn row_status(pending: bool, state: &str, blocker: Option<&str>) -> String {
    if pending {
        format!("Pending · {state}")
    } else {
        blocker.map_or_else(|| state.to_owned(), |reason| format!("{state} · {reason}"))
    }
}

fn availability(availability: NavigationAvailability) -> (bool, Option<&'static str>) {
    match availability {
        NavigationAvailability::Enabled => (true, None),
        NavigationAvailability::Disabled(blocker) => (false, Some(blocker.message())),
    }
}

const fn lifecycle_label(lifecycle: NavigationLifecycle) -> &'static str {
    match lifecycle {
        NavigationLifecycle::Starting => "Starting",
        NavigationLifecycle::Running => "Running",
        NavigationLifecycle::Mixed => "Mixed",
        NavigationLifecycle::Exited => "Exited",
        NavigationLifecycle::Restorable => "Restorable",
        NavigationLifecycle::Unavailable => "Unavailable",
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
    use splinterm_core::{DojoId, LairId, SplintId, TopologyRevision};

    use super::*;
    use crate::frontend::topology::SessionPickerTarget;
    use crate::navigation_projection::{
        NavigationAction, NavigationAvailability, NavigationCapability, NavigationExplorerDojo,
        NavigationExplorerLair, NavigationExplorerSplint, NavigationExplorerSplintTarget,
        NavigationExplorerTarget,
    };

    fn view() -> NavigationExplorerView {
        let lair = LairId::new();
        let other_lair = LairId::new();
        let dojo = DojoId::new();
        let other_dojo = DojoId::new();
        let splint = SplintId::new();
        NavigationExplorerView {
            topology_revision: TopologyRevision::new(7),
            freshness: EndpointFreshness::Current,
            current: Some(NavigationNodeId::Splint {
                lair_id: lair,
                dojo_id: dojo,
                splint_id: splint,
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
                        splints: vec![NavigationExplorerSplint {
                            id: NavigationNodeId::Splint {
                                lair_id: lair,
                                dojo_id: dojo,
                                splint_id: splint,
                            },
                            label: "shell".into(),
                            lifecycle: NavigationLifecycle::Running,
                            focused_here: true,
                            target: NavigationExplorerSplintTarget {
                                topology_revision: TopologyRevision::new(7),
                                lair_id: lair,
                                dojo_id: dojo,
                                splint_id: splint,
                                live_incarnation: Some(3),
                                last_incarnation: Some(3),
                                capability: NavigationCapability {
                                    action: NavigationAction::FocusSplint,
                                    availability: NavigationAvailability::Enabled,
                                },
                            },
                        }],
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
                        splints: Vec::new(),
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
    fn terminal_targets_return_focus_while_disclosures_keep_it() {
        let mut explorer = LairExplorerUi::default();
        explorer.set_view(view());
        assert!(explorer.reveal_current());
        assert!(
            explorer
                .decision()
                .is_some_and(LairExplorerDecision::returns_focus_to_terminal)
        );
        assert!(explorer.move_left());
        assert!(
            explorer
                .decision()
                .is_some_and(LairExplorerDecision::returns_focus_to_terminal)
        );
        assert!(explorer.move_left());
        assert!(
            explorer
                .decision()
                .is_some_and(|decision| !decision.returns_focus_to_terminal())
        );
    }

    #[test]
    fn reveal_and_navigation_preserve_stable_hierarchy() {
        let mut explorer = LairExplorerUi::default();
        let view = view();
        let current = view.current;
        explorer.set_view(view);
        assert_eq!(
            explorer.breadcrumb().as_deref(),
            Some("work / editor / shell")
        );
        assert!(explorer.reveal_current());
        assert_eq!(explorer.rows().len(), 4);
        assert_eq!(explorer.selected(), current);
        assert!(explorer.move_left());
        assert!(matches!(
            explorer.selected(),
            Some(NavigationNodeId::Dojo { .. })
        ));
        assert!(explorer.move_left());
        assert!(matches!(
            explorer.selected(),
            Some(NavigationNodeId::Lair(_))
        ));
        assert!(explorer.move_right());
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
        assert!(explorer.retryable());
        assert_eq!(
            explorer.status_message(),
            Some("Navigation unavailable · R retry")
        );
    }

    #[test]
    fn only_the_activated_splint_enters_pending_state() {
        let mut explorer = LairExplorerUi::default();
        let view = view();
        let selected = view.current.unwrap();
        explorer.set_view(view);
        explorer.reveal_current();
        let decision = explorer.decision().unwrap();
        assert!(explorer.set_pending(decision));
        assert!(!explorer.set_pending(decision));
        let rows = explorer.rows();
        let pending = rows.iter().find(|row| row.id == selected).unwrap();
        assert!(pending.pending);
        assert!(!pending.enabled);
        assert_eq!(pending.status, "Pending · Running");
        assert_eq!(rows.iter().filter(|row| row.pending).count(), 1);
        assert_eq!(explorer.decision(), None);
        let LairExplorerDecision::FocusSplint(target) = decision else {
            panic!("expected exact Splint decision");
        };
        let mut wrong = target;
        wrong.topology_revision = TopologyRevision::new(8);
        assert!(!explorer.finish_activation(LairExplorerActivationTarget::Splint(wrong), true));
        wrong = target;
        wrong.live_incarnation = Some(99);
        assert!(!explorer.finish_activation(LairExplorerActivationTarget::Splint(wrong), true));
        assert!(explorer.finish_activation(LairExplorerActivationTarget::Splint(target), true));
        assert!(explorer.decision().is_some());
    }

    #[test]
    fn unrelated_tab_failure_preserves_pending_activation_and_later_focus_ack() {
        for select_dojo in [false, true] {
            let mut explorer = LairExplorerUi::default();
            explorer.set_view(view());
            explorer.reveal_current();
            explorer.focus();
            if select_dojo {
                explorer.move_left();
            }
            let decision = explorer.decision().unwrap();
            let (completed, dojo_id) = match decision {
                LairExplorerDecision::OpenDojo(target) => (
                    LairExplorerActivationTarget::Dojo(SessionPickerTarget {
                        topology_revision: target.topology_revision,
                        lair_id: target.lair_id,
                        dojo_id: target.dojo_id,
                        action: target.capability.action,
                    }),
                    target.dojo_id,
                ),
                LairExplorerDecision::FocusSplint(target) => {
                    (LairExplorerActivationTarget::Splint(target), target.dojo_id)
                }
                LairExplorerDecision::Toggle(_) => panic!("expected activation"),
            };
            assert!(explorer.set_pending(decision));
            let pending = explorer.pending;
            // Even a failure naming the same Dojo is unrelated without an exact
            // Explorer activation target. Exercise the production failure helper.
            let failure = crate::frontend::WindowTopologyUpdate::TabFailed {
                dojo_id: Some(dojo_id),
                message: "queued unrelated topology failure".into(),
                explorer_target: None,
            };
            let crate::frontend::WindowTopologyUpdate::TabFailed {
                explorer_target, ..
            } = failure
            else {
                unreachable!();
            };
            assert!(!explorer.fail_activation(explorer_target));
            assert_eq!(explorer.pending, pending);
            assert!(explorer.activation_pending());
            assert!(explorer.focused());
            assert!(explorer.finish_activation(completed, true));
            explorer.return_to_terminal();
            assert!(!explorer.activation_pending());
            assert!(!explorer.focused());
        }
    }

    #[test]
    fn failed_activation_retires_only_exact_request_and_allows_retry_after_refresh() {
        let mut explorer = LairExplorerUi::default();
        explorer.set_view(view());
        explorer.reveal_current();
        explorer.focus();
        let decision = explorer.decision().unwrap();
        let LairExplorerDecision::FocusSplint(target) = decision else {
            panic!("expected Splint target");
        };
        assert!(explorer.set_pending(decision));

        let mut wrong_lair = target;
        wrong_lair.lair_id = LairId::new();
        let mut wrong_dojo = target;
        wrong_dojo.dojo_id = DojoId::new();
        let mut wrong_splint = target;
        wrong_splint.splint_id = SplintId::new();
        let mut wrong_revision = target;
        wrong_revision.topology_revision = TopologyRevision::new(99);
        let mut wrong_incarnation = target;
        wrong_incarnation.live_incarnation = Some(99);
        for wrong in [
            wrong_lair,
            wrong_dojo,
            wrong_splint,
            wrong_revision,
            wrong_incarnation,
        ] {
            for succeeded in [false, true] {
                assert!(
                    !explorer
                        .finish_activation(LairExplorerActivationTarget::Splint(wrong), succeeded,)
                );
                assert_eq!(explorer.pending, Some(LairExplorerPending::Splint(target)));
                assert!(explorer.focused());
            }
        }

        // The owner could not focus the captured incarnation. Keep keyboard
        // ownership, but release the exact request and disable its stale row.
        assert!(explorer.fail_activation(Some(LairExplorerActivationTarget::Splint(target))));
        assert!(explorer.pending.is_none());
        assert!(explorer.focused());
        assert!(explorer.rows().iter().all(|row| !row.pending));
        assert!(explorer.decision().is_none());

        // A fresh projection retains the same stable ID under a new incarnation.
        let mut refreshed = explorer.view.clone().unwrap();
        let splint = &mut refreshed.lairs[0].dojos[0].splints[0];
        assert_eq!(splint.target.splint_id, target.splint_id);
        splint.target.live_incarnation = Some(99);
        splint.target.last_incarnation = Some(99);
        splint.target.capability.availability = NavigationAvailability::Enabled;
        explorer.set_view(refreshed);
        let retry = explorer.decision().unwrap();
        let LairExplorerDecision::FocusSplint(retry_target) = retry else {
            panic!("expected refreshed Splint target");
        };
        assert!(explorer.set_pending(retry));
        // A delayed old failure or acknowledgement cannot retire the new request.
        assert!(!explorer.fail_activation(Some(LairExplorerActivationTarget::Splint(target))));
        assert!(explorer.activation_pending());
        assert!(!explorer.finish_activation(LairExplorerActivationTarget::Splint(target), true));
        assert_eq!(
            explorer.pending,
            Some(LairExplorerPending::Splint(retry_target))
        );
        assert!(
            explorer.finish_activation(LairExplorerActivationTarget::Splint(retry_target), true)
        );
        explorer.return_to_terminal();
        assert!(!explorer.focused());
        assert!(explorer.pending.is_none());
        assert!(
            !explorer.finish_activation(LairExplorerActivationTarget::Splint(retry_target), true)
        );
    }

    #[test]
    fn stale_refresh_and_lifecycle_states_are_explicit() {
        let mut explorer = LairExplorerUi::default();
        let mut view = view();
        view.freshness = EndpointFreshness::Stale;
        view.lairs[0].dojos[0].splints[0].lifecycle = NavigationLifecycle::Restorable;
        explorer.set_view(view);
        assert!(explorer.retryable());
        assert_eq!(
            explorer.status_message(),
            Some("Navigation is stale · R retry")
        );
        explorer.begin_refresh();
        assert_eq!(explorer.status_message(), Some("Refreshing navigation…"));
        explorer.reveal_current();
        let splint = explorer
            .rows()
            .into_iter()
            .find(|row| row.kind == LairExplorerRowKind::Splint)
            .unwrap();
        assert!(splint.status.contains("Restorable"));
        assert!(splint.status.contains("Refreshing topology"));
        assert!(!splint.enabled);
    }

    #[test]
    fn dojo_decision_retains_exact_revision_and_identity() {
        let mut explorer = LairExplorerUi::default();
        let view = view();
        let target = view.lairs[0].dojos[0].target;
        explorer.set_view(view);
        explorer.reveal_current();
        explorer.move_left();
        let decision = LairExplorerDecision::OpenDojo(target);
        assert_eq!(explorer.decision(), Some(decision));
        assert!(explorer.set_pending(decision));
        let completed = SessionPickerTarget {
            topology_revision: target.topology_revision,
            lair_id: target.lair_id,
            dojo_id: target.dojo_id,
            action: target.capability.action,
        };
        let mut wrong = completed;
        wrong.lair_id = LairId::new();
        assert!(!explorer.finish_activation(LairExplorerActivationTarget::Dojo(wrong), true));
        wrong = completed;
        wrong.topology_revision = TopologyRevision::new(8);
        assert!(!explorer.finish_activation(LairExplorerActivationTarget::Dojo(wrong), true));
        assert!(explorer.finish_activation(LairExplorerActivationTarget::Dojo(completed), true));
    }

    #[test]
    fn restorable_splint_routes_to_the_existing_preview_action() {
        let mut explorer = LairExplorerUi::default();
        let mut view = view();
        let splint = &mut view.lairs[0].dojos[0].splints[0];
        splint.lifecycle = NavigationLifecycle::Restorable;
        splint.target.live_incarnation = None;
        splint.target.capability.action = NavigationAction::PreviewRestoreSplint;
        let target = splint.target;
        explorer.set_view(view);
        explorer.reveal_current();
        assert_eq!(
            explorer.decision(),
            Some(LairExplorerDecision::FocusSplint(target))
        );
    }

    #[test]
    fn splint_selection_survives_reorder_and_falls_back_after_removal() {
        let mut explorer = LairExplorerUi::default();
        let mut view = view();
        let selected = view.lairs[0].dojos[0].splints[0].id;
        let mut sibling = view.lairs[0].dojos[0].splints[0].clone();
        let sibling_id = SplintId::new();
        sibling.id = NavigationNodeId::Splint {
            lair_id: sibling.target.lair_id,
            dojo_id: sibling.target.dojo_id,
            splint_id: sibling_id,
        };
        sibling.target.splint_id = sibling_id;
        sibling.label = "tests".into();
        sibling.focused_here = false;
        view.lairs[0].dojos[0].splints.push(sibling);
        explorer.set_view(view.clone());
        explorer.reveal_current();
        view.lairs[0].dojos[0].splints.swap(0, 1);
        explorer.set_view(view.clone());
        assert_eq!(explorer.selected(), Some(selected));
        let decision = explorer.decision().unwrap();
        assert!(explorer.set_pending(decision));

        view.lairs[0].dojos[0]
            .splints
            .retain(|splint| splint.id != selected);
        view.current = None;
        explorer.set_view(view);
        assert!(matches!(
            explorer.selected(),
            Some(NavigationNodeId::Dojo { .. })
        ));
        assert!(!explorer.clear_pending());
    }

    #[test]
    fn splint_decision_retains_parent_revision_and_incarnation() {
        let mut explorer = LairExplorerUi::default();
        let view = view();
        let target = view.lairs[0].dojos[0].splints[0].target;
        explorer.set_view(view);
        explorer.reveal_current();
        assert_eq!(
            explorer.decision(),
            Some(LairExplorerDecision::FocusSplint(target))
        );
    }
}
