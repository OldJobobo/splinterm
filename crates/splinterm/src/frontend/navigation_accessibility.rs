//! Owner-local bridge from the real Explorer projection to bounded semantics.

use std::collections::HashMap;

use super::LairExplorerUi;
use crate::accessibility::{
    MAX_ACCESSIBILITY_ITEMS, MAX_ACCESSIBLE_NAME_CHARS, SEARCH_NODE_ID, SemanticAction,
    SemanticActionRequest, SemanticAvailability, SemanticFocus, SemanticNavigationSnapshot,
    SemanticNodeId, SemanticNodeRegistry, SemanticTreeItem, TERMINAL_NODE_ID, TREE_NODE_ID,
    is_bidi_formatting, valid_query_text,
};
use crate::navigation_projection::{NavigationExplorerView, NavigationNodeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent owner authority axes, not one UI mode"
)]
pub(crate) struct NavigationAccessContext {
    pub keyboard_focused: bool,
    pub modal: bool,
    pub permitted: bool,
    pub pending: bool,
}

impl NavigationAccessContext {
    fn enabled(self) -> bool {
        self.keyboard_focused && !self.modal && self.permitted
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct NavigationIdentity {
    endpoint: String,
    endpoint_generation: u64,
    node: NavigationNodeId,
    live_incarnation: Option<u64>,
    last_incarnation: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NavigationAccessAction {
    Terminal,
    Tree,
    Search(String),
    SearchFocus,
    Item(NavigationNodeId),
    Expanded(NavigationNodeId, bool),
    Activate(NavigationNodeId),
}

#[derive(Default)]
pub(crate) struct NavigationAccessibility {
    registry: SemanticNodeRegistry<NavigationIdentity>,
    targets: HashMap<SemanticNodeId, NavigationNodeId>,
    view: Option<NavigationExplorerView>,
    snapshot: Option<SemanticNavigationSnapshot>,
    generation: u64,
}

impl NavigationAccessibility {
    pub(crate) fn refresh(
        &mut self,
        explorer: &LairExplorerUi,
        context: NavigationAccessContext,
    ) -> SemanticNavigationSnapshot {
        let identities = identities(explorer.view());
        self.registry
            .retain(|key| identities.contains_key(&key.node) && identities[&key.node] == *key);
        self.targets.clear();
        let visible = explorer.visible() && !context.modal;
        let enabled = visible && context.enabled();
        let mut items = Vec::new();
        let mut ids = HashMap::new();
        let rows = if visible { explorer.rows() } else { Vec::new() };
        let mut limited = rows.len() > MAX_ACCESSIBILITY_ITEMS;
        if visible {
            for row in rows.into_iter().take(MAX_ACCESSIBILITY_ITEMS) {
                let Some(identity) = identities.get(&row.id) else {
                    continue;
                };
                let Ok(id) = self.registry.id_for(identity.clone()) else {
                    limited = true;
                    break;
                };
                let parent = parent(row.id).and_then(|parent| ids.get(&parent).copied());
                if row.level != 1 && parent.is_none() {
                    continue;
                }
                ids.insert(row.id, id);
                self.targets.insert(id, row.id);
                items.push(SemanticTreeItem {
                    id,
                    parent,
                    name: safe_text(&row.label),
                    status: safe_text(&row.status),
                    level: row.level,
                    expanded: row.expanded,
                    selected: explorer.selected() == Some(row.id),
                    current: row.current,
                    availability: if row.pending {
                        SemanticAvailability::Pending
                    } else if enabled
                        && row.enabled
                        && !context.pending
                        && !explorer.activation_pending()
                    {
                        SemanticAvailability::Enabled
                    } else {
                        SemanticAvailability::Disabled
                    },
                });
            }
        }
        let focus = if !context.keyboard_focused || context.modal || !context.permitted {
            SemanticFocus::None
        } else if visible && explorer.focused() {
            if explorer.search_active() {
                SemanticFocus::Search
            } else {
                explorer
                    .selected()
                    .and_then(|selected| ids.get(&selected).copied())
                    .map_or(SemanticFocus::Tree, SemanticFocus::Item)
            }
        } else {
            SemanticFocus::Terminal
        };
        let mut next = SemanticNavigationSnapshot {
            generation: self.generation,
            visible,
            enabled,
            result_count: items.len(),
            status: visible.then(|| navigation_status(explorer, limited, items.len())),
            items,
            query: if visible {
                explorer.query().to_owned()
            } else {
                String::new()
            },
            focus,
        };
        // Keep exact revision/capability/endpoint evidence privately; no such authority
        // is reconstructed from accessible labels or sent back by an AT client.
        if self.snapshot.as_ref() != Some(&next) || self.view.as_ref() != explorer.view() {
            self.generation = self
                .generation
                .checked_add(1)
                .expect("semantic generation exhausted");
            next.generation = self.generation;
            self.view = explorer.view().cloned();
            self.snapshot = Some(next.clone());
        }
        next
    }

    pub(crate) fn resolve(
        &self,
        request: SemanticActionRequest,
        explorer: &LairExplorerUi,
        context: NavigationAccessContext,
    ) -> Option<NavigationAccessAction> {
        if !context.enabled()
            || !explorer.visible()
            || request.generation != self.generation
            || self.view.as_ref() != explorer.view()
        {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        if !snapshot.enabled {
            return None;
        }
        let target = |id| self.targets.get(&id).copied();
        match request.action {
            SemanticAction::Focus(TERMINAL_NODE_ID) => Some(NavigationAccessAction::Terminal),
            SemanticAction::Focus(SEARCH_NODE_ID) => Some(NavigationAccessAction::SearchFocus),
            SemanticAction::Focus(TREE_NODE_ID) => Some(NavigationAccessAction::Tree),
            SemanticAction::Focus(id) => target(id).map(NavigationAccessAction::Item),
            SemanticAction::SetSearch(query) if valid_query_text(&query).is_ok() => {
                Some(NavigationAccessAction::Search(query))
            }
            SemanticAction::SetExpanded { node, expanded } => {
                snapshot
                    .items
                    .iter()
                    .find(|item| item.id == node && item.expanded.is_some())?;
                target(node).map(|id| NavigationAccessAction::Expanded(id, expanded))
            }
            SemanticAction::Activate(id) => {
                if context.pending || explorer.activation_pending() {
                    return None;
                }
                snapshot.items.iter().find(|item| {
                    item.id == id && item.availability == SemanticAvailability::Enabled
                })?;
                target(id).map(NavigationAccessAction::Activate)
            }
            SemanticAction::SetSearch(_) => None,
        }
    }
}

fn navigation_status(explorer: &LairExplorerUi, limited: bool, count: usize) -> String {
    let status = explorer.status_message().map_or_else(
        || {
            if limited {
                "Navigation item limit reached".to_owned()
            } else {
                format!("{count} results")
            }
        },
        str::to_owned,
    );
    let hint = if explorer.search_active() {
        "Typing searches labels"
    } else if explorer.focused() {
        "F cycles filters"
    } else {
        "Focus Explorer to change filters"
    };
    format!("{} filter · {status} · {hint}", explorer.filter().label())
}

fn identities(
    view: Option<&NavigationExplorerView>,
) -> HashMap<NavigationNodeId, NavigationIdentity> {
    let mut identities = HashMap::new();
    if let Some(view) = view {
        let identity = |node, live_incarnation, last_incarnation| NavigationIdentity {
            endpoint: view.endpoint_namespace.clone(),
            endpoint_generation: view.endpoint_generation,
            node,
            live_incarnation,
            last_incarnation,
        };
        for lair in &view.lairs {
            identities.insert(lair.id, identity(lair.id, None, None));
            for dojo in &lair.dojos {
                identities.insert(dojo.id, identity(dojo.id, None, None));
                for splint in &dojo.splints {
                    identities.insert(
                        splint.id,
                        identity(
                            splint.id,
                            splint.target.live_incarnation,
                            splint.target.last_incarnation,
                        ),
                    );
                }
            }
        }
    }
    identities
}

fn parent(id: NavigationNodeId) -> Option<NavigationNodeId> {
    match id {
        NavigationNodeId::Lair(_) => None,
        NavigationNodeId::Dojo { lair_id, .. } => Some(NavigationNodeId::Lair(lair_id)),
        NavigationNodeId::Splint {
            lair_id, dojo_id, ..
        } => Some(NavigationNodeId::Dojo { lair_id, dojo_id }),
    }
}

fn safe_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() && !is_bidi_formatting(*character))
        .take(MAX_ACCESSIBLE_NAME_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::lair_explorer::tests::view;
    use super::*;
    use crate::frontend::LairExplorerDecision;
    use crate::navigation_projection::{
        NavigationAction, NavigationAvailability, NavigationBlocker,
    };

    fn context() -> NavigationAccessContext {
        NavigationAccessContext {
            keyboard_focused: true,
            modal: false,
            permitted: true,
            pending: false,
        }
    }
    fn explorer() -> LairExplorerUi {
        let mut explorer = LairExplorerUi::default();
        explorer.set_view(view());
        explorer.focus();
        explorer.reveal_current();
        explorer
    }
    fn request(
        snapshot: &SemanticNavigationSnapshot,
        action: SemanticAction,
    ) -> SemanticActionRequest {
        SemanticActionRequest {
            generation: snapshot.generation,
            action,
        }
    }

    #[test]
    fn navigation_accessibility_projects_real_rows_and_independent_current_axes() {
        let mut explorer = explorer();
        let mut accessibility = NavigationAccessibility::default();
        let snapshot = accessibility.refresh(&explorer, context());
        snapshot.tree_update().unwrap();
        assert_eq!(snapshot.items.len(), explorer.rows().len());
        assert_eq!(
            snapshot
                .items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["work", "editor", "shell", "notes"]
        );
        assert_eq!(snapshot.items.iter().filter(|item| item.current).count(), 3);
        assert_eq!(
            snapshot.items.iter().filter(|item| item.selected).count(),
            1
        );
        assert_eq!(snapshot.items[2].parent, Some(snapshot.items[1].id));
        assert_eq!(snapshot.focus, SemanticFocus::Item(snapshot.items[2].id));
        assert_eq!(accessibility.refresh(&explorer, context()), snapshot);
        explorer.begin_search();
        explorer.set_search("no matches".into());
        let empty = accessibility.refresh(&explorer, context());
        assert!(empty.items.is_empty());
        assert_eq!(empty.focus, SemanticFocus::Search);
        assert_eq!(
            empty.status.as_deref(),
            Some("All filter · No matches · Typing searches labels")
        );
        assert_eq!(empty.query, "no matches");
        assert!(
            accessibility
                .resolve(
                    request(&snapshot, SemanticAction::Activate(snapshot.items[2].id)),
                    &explorer,
                    context()
                )
                .is_none()
        );
    }

    #[test]
    fn navigation_accessibility_filter_hint_matches_input_owner() {
        let mut explorer = explorer();
        assert!(super::navigation_status(&explorer, false, 4).contains("F cycles filters"));
        explorer.begin_search();
        let search = super::navigation_status(&explorer, false, 4);
        assert!(search.contains("Typing searches labels"));
        assert!(!search.contains("F cycles filters"));
        explorer.return_to_terminal();
        let terminal = super::navigation_status(&explorer, false, 4);
        assert!(terminal.contains("Focus Explorer"));
        assert!(!terminal.contains("F cycles filters"));
    }

    #[test]
    fn navigation_accessibility_tracks_filter_and_rejects_hidden_targets() {
        use crate::frontend::LairExplorerFilter;
        let mut explorer = explorer();
        let mut accessibility = NavigationAccessibility::default();
        let all = accessibility.refresh(&explorer, context());
        let hidden = all.items[3].id;
        explorer.set_filter(LairExplorerFilter::Saved);
        let saved = accessibility.refresh(&explorer, context());
        assert!(saved.generation > all.generation);
        assert_eq!(saved.items, all.items);
        assert!(saved.status.as_deref().unwrap().starts_with("Saved filter"));
        explorer.set_filter(LairExplorerFilter::Live);
        let live = accessibility.refresh(&explorer, context());
        live.tree_update().unwrap();
        assert_eq!(live.items.len(), 3);
        assert!(live.status.as_deref().unwrap().starts_with("Live filter"));
        assert!(
            accessibility
                .resolve(
                    request(&live, SemanticAction::Activate(hidden)),
                    &explorer,
                    context()
                )
                .is_none()
        );
        assert!(
            accessibility
                .resolve(
                    request(&all, SemanticAction::Activate(all.items[2].id)),
                    &explorer,
                    context()
                )
                .is_none()
        );
        explorer.set_filter(LairExplorerFilter::All);
        let restored = accessibility.refresh(&explorer, context());
        assert_eq!(restored.items, all.items);
    }

    #[test]
    fn navigation_accessibility_rejects_modal_focus_permission_and_hidden_actions() {
        let mut explorer = explorer();
        let mut accessibility = NavigationAccessibility::default();
        let snapshot = accessibility.refresh(&explorer, context());
        let actions = [
            SemanticAction::Focus(TERMINAL_NODE_ID),
            SemanticAction::SetSearch("a".into()),
            SemanticAction::Activate(snapshot.items[2].id),
            SemanticAction::SetExpanded {
                node: snapshot.items[0].id,
                expanded: false,
            },
        ];
        for blocked in [
            NavigationAccessContext {
                modal: true,
                ..context()
            },
            NavigationAccessContext {
                keyboard_focused: false,
                ..context()
            },
            NavigationAccessContext {
                permitted: false,
                ..context()
            },
        ] {
            for action in &actions {
                assert!(
                    accessibility
                        .resolve(request(&snapshot, action.clone()), &explorer, blocked)
                        .is_none()
                );
            }
            let state = accessibility.refresh(&explorer, blocked);
            assert!(!state.enabled);
            if !blocked.permitted {
                assert!(state.visible);
                assert!(!state.items.is_empty());
                assert!(
                    state
                        .items
                        .iter()
                        .all(|item| item.availability == SemanticAvailability::Disabled)
                );
            }
        }
        explorer.toggle_visibility();
        let hidden = accessibility.refresh(&explorer, context());
        assert!(!hidden.visible);
        assert!(hidden.items.is_empty());
        assert!(hidden.query.is_empty());
        assert!(hidden.status.is_none());
    }

    #[test]
    fn navigation_accessibility_never_retargets_incarnation_endpoint_revision_or_removed_identity()
    {
        let mut explorer = explorer();
        let mut accessibility = NavigationAccessibility::default();
        let first = accessibility.refresh(&explorer, context());
        let id = first.items[2].id;
        let mut changed = explorer.view().unwrap().clone();
        changed.lairs[0].dojos[0].splints[0].target.live_incarnation = Some(4);
        explorer.set_view(changed.clone());
        assert!(
            accessibility
                .resolve(
                    request(&first, SemanticAction::Activate(id)),
                    &explorer,
                    context()
                )
                .is_none()
        );
        let next = accessibility.refresh(&explorer, context());
        assert_ne!(next.items[2].id, id);
        assert!(
            accessibility
                .resolve(
                    request(&next, SemanticAction::Activate(id)),
                    &explorer,
                    context()
                )
                .is_none()
        );
        changed.endpoint_generation += 1;
        explorer.set_view(changed.clone());
        let endpoint = accessibility.refresh(&explorer, context());
        assert_ne!(endpoint.items[0].id, next.items[0].id);
        changed.topology_revision = splinterm_core::TopologyRevision::new(99);
        explorer.set_view(changed.clone());
        let revision = accessibility.refresh(&explorer, context());
        assert_eq!(revision.items[2].id, endpoint.items[2].id);
        assert!(
            accessibility
                .resolve(
                    request(&endpoint, SemanticAction::Activate(endpoint.items[2].id)),
                    &explorer,
                    context()
                )
                .is_none()
        );
        let removed = changed.lairs[0].dojos[0].splints.remove(0);
        explorer.set_view(changed.clone());
        accessibility.refresh(&explorer, context());
        changed.lairs[0].dojos[0].splints.push(removed);
        explorer.set_view(changed);
        let reborn = accessibility.refresh(&explorer, context());
        assert_ne!(reborn.items[2].id, revision.items[2].id);
    }

    #[test]
    fn navigation_accessibility_activation_keeps_restore_preview_and_capability_guards() {
        let mut explorer = explorer();
        let mut view = explorer.view().unwrap().clone();
        view.lairs[0].dojos[0].splints[0].target.capability.action =
            NavigationAction::PreviewRestoreSplint;
        explorer.set_view(view.clone());
        let mut accessibility = NavigationAccessibility::default();
        let snapshot = accessibility.refresh(&explorer, context());
        let action = accessibility
            .resolve(
                request(&snapshot, SemanticAction::Activate(snapshot.items[2].id)),
                &explorer,
                context(),
            )
            .unwrap();
        let NavigationAccessAction::Activate(id) = action else {
            panic!("typed activation expected")
        };
        explorer.select(id);
        let Some(LairExplorerDecision::FocusSplint(target)) = explorer.decision() else {
            panic!("existing explorer path expected")
        };
        assert_eq!(
            target.capability.action,
            NavigationAction::PreviewRestoreSplint
        );
        assert!(explorer.set_pending(LairExplorerDecision::FocusSplint(target)));
        let pending = accessibility.refresh(&explorer, context());
        assert_eq!(
            accessibility.resolve(
                request(&pending, SemanticAction::Focus(TERMINAL_NODE_ID)),
                &explorer,
                context()
            ),
            Some(NavigationAccessAction::Terminal)
        );
        assert!(
            accessibility
                .resolve(
                    request(&pending, SemanticAction::Activate(pending.items[2].id)),
                    &explorer,
                    context()
                )
                .is_none()
        );
        explorer.clear_pending();
        view.lairs[0].dojos[0].splints[0]
            .target
            .capability
            .availability = NavigationAvailability::Disabled(NavigationBlocker::PermissionDenied);
        explorer.set_view(view);
        let denied = accessibility.refresh(&explorer, context());
        assert_eq!(denied.items[2].availability, SemanticAvailability::Disabled);
        assert!(
            accessibility
                .resolve(
                    request(&denied, SemanticAction::Activate(denied.items[2].id)),
                    &explorer,
                    context()
                )
                .is_none()
        );
        assert!(explorer.decision().is_none());
    }

    #[test]
    fn navigation_accessibility_pending_activation_preserves_nonmutating_navigation() {
        let explorer = explorer();
        let mut accessibility = NavigationAccessibility::default();
        let pending = NavigationAccessContext {
            pending: true,
            ..context()
        };
        let snapshot = accessibility.refresh(&explorer, pending);
        assert!(snapshot.enabled);
        for action in [
            SemanticAction::Focus(TERMINAL_NODE_ID),
            SemanticAction::Focus(snapshot.items[0].id),
            SemanticAction::SetSearch("work".into()),
            SemanticAction::SetExpanded {
                node: snapshot.items[0].id,
                expanded: false,
            },
        ] {
            assert!(
                accessibility
                    .resolve(request(&snapshot, action), &explorer, pending)
                    .is_some()
            );
        }
        assert!(
            accessibility
                .resolve(
                    request(&snapshot, SemanticAction::Activate(snapshot.items[2].id)),
                    &explorer,
                    pending
                )
                .is_none()
        );
        assert!(
            snapshot
                .items
                .iter()
                .all(|item| item.availability == SemanticAvailability::Disabled)
        );
    }

    #[test]
    fn navigation_accessibility_bounds_metadata_and_registry_churn() {
        assert_eq!(
            safe_text(&format!("\n\u{202e}{}", "x".repeat(1000))),
            "x".repeat(MAX_ACCESSIBLE_NAME_CHARS)
        );
        let mut registry = SemanticNodeRegistry::default();
        let first = registry.id_for(0).unwrap();
        for value in 1..MAX_ACCESSIBILITY_ITEMS * 2 {
            registry.retain(|_| false);
            assert_ne!(registry.id_for(value).unwrap(), first);
        }
    }
}
