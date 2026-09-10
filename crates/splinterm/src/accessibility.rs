//! Bounded semantic accessibility contract for custom-rendered navigation.
//!
//! Rendering and semantics intentionally remain separate. The Unix adapter
//! publishes this tree through AT-SPI over D-Bus; no Wayland protocol object is
//! touched by an accessibility callback thread.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};

use accesskit::{Action, AriaCurrent, Live, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};

use crate::native_atspi::NativeAtspiPublisher;

pub const MAX_ACCESSIBILITY_ITEMS: usize = 4096;
pub const MAX_ACCESSIBLE_NAME_CHARS: usize = 256;
pub const MAX_ACCESSIBLE_QUERY_CHARS: usize = 64;
pub const ACCESSIBILITY_ACTION_QUEUE_CAPACITY: usize = 64;

pub const ROOT_NODE_ID: SemanticNodeId = SemanticNodeId(1);
pub const TERMINAL_NODE_ID: SemanticNodeId = SemanticNodeId(2);
pub const SEARCH_NODE_ID: SemanticNodeId = SemanticNodeId(3);
pub const TREE_NODE_ID: SemanticNodeId = SemanticNodeId(4);
pub const STATUS_NODE_ID: SemanticNodeId = SemanticNodeId(5);
const FIRST_ITEM_NODE_ID: u64 = 16;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SemanticNodeId(pub u64);

impl SemanticNodeId {
    #[must_use]
    pub const fn item(value: u64) -> Option<Self> {
        if value >= FIRST_ITEM_NODE_ID {
            Some(Self(value))
        } else {
            None
        }
    }
}

impl From<SemanticNodeId> for NodeId {
    fn from(value: SemanticNodeId) -> Self {
        Self(value.0)
    }
}

/// Window-local stable semantic IDs. Retiring an entry frees bounded registry
/// space, but its numeric ID is never reused during the Window lifetime.
pub struct SemanticNodeRegistry<K> {
    ids: HashMap<K, SemanticNodeId>,
    next: u64,
}

impl<K> Default for SemanticNodeRegistry<K> {
    fn default() -> Self {
        Self {
            ids: HashMap::new(),
            next: FIRST_ITEM_NODE_ID,
        }
    }
}

impl<K: Eq + std::hash::Hash> SemanticNodeRegistry<K> {
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.ids.retain(|key, _| keep(key));
    }

    /// Return one stable ID for a typed domain identity.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticTreeError::TooManyItems`] when the live registry reaches
    /// its fixed bound or the monotonic ID counter is exhausted.
    pub fn id_for(&mut self, key: K) -> Result<SemanticNodeId, SemanticTreeError> {
        if let Some(id) = self.ids.get(&key) {
            return Ok(*id);
        }
        if self.ids.len() == MAX_ACCESSIBILITY_ITEMS {
            return Err(SemanticTreeError::TooManyItems);
        }
        let id = SemanticNodeId(self.next);
        self.next = self
            .next
            .checked_add(1)
            .ok_or(SemanticTreeError::TooManyItems)?;
        self.ids.insert(key, id);
        Ok(id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticAvailability {
    Enabled,
    Disabled,
    Pending,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTreeItem {
    pub id: SemanticNodeId,
    pub parent: Option<SemanticNodeId>,
    pub name: String,
    pub status: String,
    pub level: usize,
    pub expanded: Option<bool>,
    pub selected: bool,
    pub current: bool,
    pub availability: SemanticAvailability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticFocus {
    None,
    Terminal,
    Search,
    Tree,
    Item(SemanticNodeId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticNavigationSnapshot {
    /// Owner-issued authority epoch, captured by native callbacks.
    pub generation: u64,
    pub visible: bool,
    pub enabled: bool,
    pub items: Vec<SemanticTreeItem>,
    pub query: String,
    pub result_count: usize,
    pub status: Option<String>,
    pub focus: SemanticFocus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticTreeError {
    TooManyItems,
    InvalidNodeId,
    DuplicateNodeId,
    InvalidParent,
    InvalidLevel,
    InvalidFocus,
    InvalidState,
    InvalidText,
}

impl std::fmt::Display for SemanticTreeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TooManyItems => "accessibility tree has too many items",
            Self::InvalidNodeId => "accessibility tree item ID is invalid",
            Self::DuplicateNodeId => "accessibility tree item ID is duplicated",
            Self::InvalidParent => "accessibility tree item parent is invalid",
            Self::InvalidLevel => "accessibility tree item level is invalid",
            Self::InvalidFocus => "accessibility focus target is invalid",
            Self::InvalidState => "accessibility item state is invalid",
            Self::InvalidText => "accessibility text is invalid",
        })
    }
}

impl std::error::Error for SemanticTreeError {}

impl SemanticNavigationSnapshot {
    /// Build a complete AccessKit tree. Node-local geometry may be added later;
    /// global window bounds remain unknown because Wayland does not expose the
    /// compositor-relative window position.
    ///
    /// # Errors
    ///
    /// Returns an error when identities, hierarchy, focus, counts, or text
    /// violate this module's closed bounds.
    pub fn tree_update(&self) -> Result<TreeUpdate, SemanticTreeError> {
        self.validate()?;
        let mut child_ids = HashMap::<Option<SemanticNodeId>, Vec<NodeId>>::new();
        for item in &self.items {
            child_ids
                .entry(item.parent)
                .or_default()
                .push(item.id.into());
        }

        let mut root = Node::new(Role::Window);
        root.set_label("Splinterm");
        root.set_children(if self.visible {
            vec![
                TERMINAL_NODE_ID.into(),
                SEARCH_NODE_ID.into(),
                TREE_NODE_ID.into(),
                STATUS_NODE_ID.into(),
            ]
        } else {
            vec![TERMINAL_NODE_ID.into()]
        });

        let mut terminal = Node::new(Role::Terminal);
        terminal.set_label("Terminal");
        terminal.add_action(Action::Focus);

        let mut search = Node::new(Role::SearchInput);
        search.set_label("Search navigation");
        search.set_value(self.query.clone());
        search.set_description(match self.result_count {
            1 => "1 result".to_owned(),
            count => format!("{count} results"),
        });
        search.add_action(Action::Focus);
        search.add_action(Action::SetValue);

        let mut tree = Node::new(Role::Tree);
        tree.set_label("Lairs");
        tree.set_children(child_ids.remove(&None).unwrap_or_default());
        tree.add_action(Action::Focus);

        let mut status = Node::new(Role::Status);
        status.set_live(Live::Polite);
        status.set_live_atomic();
        if let Some(message) = &self.status {
            status.set_label(message.clone());
        }

        if !self.visible {
            search.set_hidden();
            tree.set_hidden();
            status.set_hidden();
        }
        if !self.enabled {
            terminal.set_disabled();
            search.set_disabled();
            tree.set_disabled();
        }
        let mut nodes = vec![
            (ROOT_NODE_ID.into(), root),
            (TERMINAL_NODE_ID.into(), terminal),
            (SEARCH_NODE_ID.into(), search),
            (TREE_NODE_ID.into(), tree),
            (STATUS_NODE_ID.into(), status),
        ];
        for item in &self.items {
            let mut node = Node::new(Role::TreeItem);
            node.set_label(item.name.clone());
            if !item.status.is_empty() {
                node.set_state_description(item.status.clone());
            }
            node.set_level(item.level);
            node.set_selected(item.selected);
            if let Some(expanded) = item.expanded {
                node.set_expanded(expanded);
                node.add_action(if expanded {
                    Action::Collapse
                } else {
                    Action::Expand
                });
            }
            if item.current {
                node.set_aria_current(AriaCurrent::True);
            }
            match item.availability {
                SemanticAvailability::Enabled => node.add_action(Action::Click),
                SemanticAvailability::Disabled => node.set_disabled(),
                SemanticAvailability::Pending => {
                    node.set_disabled();
                    node.set_busy();
                }
            }
            node.set_children(child_ids.remove(&Some(item.id)).unwrap_or_default());
            node.add_action(Action::Focus);
            nodes.push((item.id.into(), node));
        }

        Ok(TreeUpdate {
            nodes,
            tree: Some(TreeInfo {
                root: ROOT_NODE_ID.into(),
                toolkit_name: Some("Splinterm custom renderer".to_owned()),
                toolkit_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            tree_id: TreeId::ROOT,
            focus: self.focus_node().into(),
        })
    }

    fn focus_node(&self) -> SemanticNodeId {
        match self.focus {
            SemanticFocus::None => ROOT_NODE_ID,
            SemanticFocus::Terminal => TERMINAL_NODE_ID,
            SemanticFocus::Search => SEARCH_NODE_ID,
            SemanticFocus::Tree => TREE_NODE_ID,
            SemanticFocus::Item(id) => id,
        }
    }

    fn validate(&self) -> Result<(), SemanticTreeError> {
        if self.items.len() > MAX_ACCESSIBILITY_ITEMS || self.result_count > MAX_ACCESSIBILITY_ITEMS
        {
            return Err(SemanticTreeError::TooManyItems);
        }
        valid_query_text(&self.query)?;
        if let Some(status) = &self.status {
            valid_text(status, MAX_ACCESSIBLE_NAME_CHARS)?;
        }
        let ids = self
            .items
            .iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>();
        if ids.len() != self.items.len() {
            return Err(SemanticTreeError::DuplicateNodeId);
        }
        if self.items.iter().filter(|item| item.selected).count() > 1 {
            return Err(SemanticTreeError::InvalidState);
        }
        for item in &self.items {
            if item.id.0 < FIRST_ITEM_NODE_ID {
                return Err(SemanticTreeError::InvalidNodeId);
            }
            valid_text(&item.name, MAX_ACCESSIBLE_NAME_CHARS)?;
            valid_text(&item.status, MAX_ACCESSIBLE_NAME_CHARS)?;
            if !(1..=3).contains(&item.level) {
                return Err(SemanticTreeError::InvalidLevel);
            }
            if item.availability == SemanticAvailability::Pending && item.status.is_empty() {
                return Err(SemanticTreeError::InvalidState);
            }
            if let Some(parent) = item.parent
                && (!ids.contains(&parent)
                    || self
                        .items
                        .iter()
                        .find(|candidate| candidate.id == parent)
                        .is_none_or(|candidate| candidate.level + 1 != item.level))
            {
                return Err(SemanticTreeError::InvalidParent);
            }
            if item.parent.is_none() && item.level != 1 {
                return Err(SemanticTreeError::InvalidParent);
            }
        }
        if let SemanticFocus::Item(id) = self.focus
            && !ids.contains(&id)
        {
            return Err(SemanticTreeError::InvalidFocus);
        }
        Ok(())
    }
}

pub(crate) fn valid_query_text(text: &str) -> Result<(), SemanticTreeError> {
    valid_text(text, MAX_ACCESSIBLE_QUERY_CHARS)
}

fn valid_text(text: &str, maximum: usize) -> Result<(), SemanticTreeError> {
    if text.chars().count() > maximum
        || text
            .chars()
            .any(|character| character.is_control() || is_bidi_formatting(character))
    {
        return Err(SemanticTreeError::InvalidText);
    }
    Ok(())
}

pub(crate) const fn is_bidi_formatting(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticAction {
    Focus(SemanticNodeId),
    Activate(SemanticNodeId),
    SetExpanded {
        node: SemanticNodeId,
        expanded: bool,
    },
    SetSearch(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticActionRequest {
    pub generation: u64,
    pub action: SemanticAction,
}

#[derive(Default)]
struct SemanticActionQueueState {
    actions: VecDeque<SemanticActionRequest>,
}

#[derive(Clone)]
pub struct SemanticActionQueue {
    state: Arc<Mutex<SemanticActionQueueState>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl SemanticActionQueue {
    #[must_use]
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            state: Arc::new(Mutex::new(SemanticActionQueueState::default())),
            wake: Arc::new(wake),
        }
    }

    /// Queue an AT action without touching calloop- or Wayland-owned state.
    /// Focus and search requests coalesce to their newest value. Other actions
    /// fail closed when the bounded queue is full.
    #[must_use]
    pub fn push(&self, action: SemanticAction) -> bool {
        self.push_at(0, action)
    }

    #[must_use]
    pub fn push_at(&self, generation: u64, action: SemanticAction) -> bool {
        if let SemanticAction::SetSearch(query) = &action
            && valid_query_text(query).is_err()
        {
            return false;
        }
        let request = SemanticActionRequest { generation, action };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(request.action, SemanticAction::Focus(_)) {
            state
                .actions
                .retain(|queued| !matches!(queued.action, SemanticAction::Focus(_)));
        } else if matches!(request.action, SemanticAction::SetSearch(_)) {
            state
                .actions
                .retain(|queued| !matches!(queued.action, SemanticAction::SetSearch(_)));
        } else if state.actions.contains(&request) {
            return true;
        }
        if state.actions.len() == ACCESSIBILITY_ACTION_QUEUE_CAPACITY {
            return false;
        }
        state.actions.push_back(request);
        drop(state);
        (self.wake)();
        true
    }

    #[must_use]
    pub fn drain(&self) -> Vec<SemanticAction> {
        self.drain_requests()
            .into_iter()
            .map(|request| request.action)
            .collect()
    }

    #[must_use]
    pub fn drain_requests(&self) -> Vec<SemanticActionRequest> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .actions
            .drain(..)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticOwnerTurn(pub u64);

#[derive(Default)]
pub struct SemanticUpdateCoalescer {
    published: Option<SemanticNavigationSnapshot>,
    pending: Option<SemanticNavigationSnapshot>,
    published_turn: Option<SemanticOwnerTurn>,
}

impl SemanticUpdateCoalescer {
    pub fn stage(&mut self, snapshot: SemanticNavigationSnapshot) {
        self.pending = Some(snapshot);
    }

    /// Return the newest staged state, suppressing equivalents and validating
    /// the platform-neutral AccessKit tree before native publication.
    ///
    /// # Errors
    ///
    /// Returns an error when the pending semantic snapshot is invalid.
    pub fn take_snapshot(
        &mut self,
        owner_turn: SemanticOwnerTurn,
    ) -> Result<Option<SemanticNavigationSnapshot>, SemanticTreeError> {
        if self.published_turn == Some(owner_turn) {
            return Ok(None);
        }
        let Some(snapshot) = self.pending.take() else {
            return Ok(None);
        };
        if self.published.as_ref() == Some(&snapshot) {
            return Ok(None);
        }
        snapshot.tree_update()?;
        self.published = Some(snapshot.clone());
        self.published_turn = Some(owner_turn);
        Ok(Some(snapshot))
    }
}

/// Native AT-SPI adapter. Construct and update this only on the calloop owner
/// thread. D-Bus callbacks communicate solely through the bounded action queue.
pub struct UnixAccessibilityAdapter {
    publisher: NativeAtspiPublisher,
    updates: SemanticUpdateCoalescer,
}

impl UnixAccessibilityAdapter {
    /// Create an AT-SPI adapter with a complete initial semantic tree.
    ///
    /// # Errors
    ///
    /// Returns an error when the initial semantic snapshot is invalid.
    pub fn new(
        initial: SemanticNavigationSnapshot,
        actions: SemanticActionQueue,
    ) -> Result<Self, SemanticTreeError> {
        initial.tree_update()?;
        let publisher = NativeAtspiPublisher::new(initial.clone(), actions);
        let updates = SemanticUpdateCoalescer {
            published: Some(initial),
            ..SemanticUpdateCoalescer::default()
        };
        Ok(Self { publisher, updates })
    }

    pub fn stage(&mut self, snapshot: SemanticNavigationSnapshot) {
        self.updates.stage(snapshot);
    }

    /// Queue at most one coalesced update for the asynchronous AT-SPI publisher.
    /// A true result means staged for transport, not acknowledged by a consumer.
    ///
    /// # Errors
    ///
    /// Returns an error when the pending semantic snapshot is invalid.
    pub fn publish_pending(
        &mut self,
        owner_turn: SemanticOwnerTurn,
    ) -> Result<bool, SemanticTreeError> {
        let Some(snapshot) = self.updates.take_snapshot(owner_turn)? else {
            return Ok(false);
        };
        self.publisher.publish(&snapshot);
        Ok(true)
    }

    #[must_use]
    pub fn transport_error(&self) -> Option<String> {
        self.publisher.transport_error()
    }

    pub fn update_window_focus_state(&mut self, focused: bool) {
        self.publisher.update_window_focus_state(focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, parent: Option<u64>, level: usize, name: &str) -> SemanticTreeItem {
        SemanticTreeItem {
            id: SemanticNodeId::item(id).unwrap(),
            parent: parent.map(|id| SemanticNodeId::item(id).unwrap()),
            name: name.to_owned(),
            status: String::new(),
            level,
            expanded: None,
            selected: false,
            current: false,
            availability: SemanticAvailability::Enabled,
        }
    }

    fn snapshot() -> SemanticNavigationSnapshot {
        let mut lair = item(16, None, 1, "work");
        lair.expanded = Some(true);
        let mut dojo = item(17, Some(16), 2, "editor");
        dojo.selected = true;
        dojo.current = true;
        let mut splint = item(18, Some(17), 3, "shell");
        splint.status = "Pending".to_owned();
        splint.availability = SemanticAvailability::Pending;
        SemanticNavigationSnapshot {
            generation: 0,
            visible: true,
            enabled: true,
            items: vec![lair, dojo, splint],
            query: String::new(),
            result_count: 3,
            status: Some("3 results".to_owned()),
            focus: SemanticFocus::Item(SemanticNodeId(17)),
        }
    }

    fn node(update: &TreeUpdate, id: SemanticNodeId) -> &Node {
        &update
            .nodes
            .iter()
            .find(|(candidate, _)| *candidate == id.into())
            .unwrap()
            .1
    }

    #[test]
    fn tree_exposes_roles_hierarchy_focus_and_independent_states() {
        let update = snapshot().tree_update().unwrap();
        assert_eq!(node(&update, ROOT_NODE_ID).role(), Role::Window);
        assert_eq!(node(&update, SEARCH_NODE_ID).role(), Role::SearchInput);
        assert_eq!(node(&update, TREE_NODE_ID).role(), Role::Tree);
        assert_eq!(node(&update, STATUS_NODE_ID).live(), Some(Live::Polite));
        assert_eq!(update.focus, NodeId(17));

        let lair = node(&update, SemanticNodeId(16));
        assert_eq!(lair.role(), Role::TreeItem);
        assert_eq!(lair.level(), Some(1));
        assert_eq!(lair.is_expanded(), Some(true));
        let dojo = node(&update, SemanticNodeId(17));
        assert_eq!(dojo.is_selected(), Some(true));
        assert_eq!(dojo.aria_current(), Some(AriaCurrent::True));
        let pending = node(&update, SemanticNodeId(18));
        assert!(pending.is_busy());
        assert!(pending.is_disabled());
        assert!(!pending.supports_action(Action::Click));
    }

    #[test]
    fn stable_ids_survive_reorder_and_invalid_structures_fail_closed() {
        let mut registry = SemanticNodeRegistry::default();
        let work = registry.id_for("work").unwrap();
        let editor = registry.id_for("editor").unwrap();
        assert_eq!(registry.id_for("editor").unwrap(), editor);
        assert_eq!(registry.id_for("work").unwrap(), work);
        assert_ne!(work, editor);

        let mut original = snapshot();
        let original_ids = original
            .tree_update()
            .unwrap()
            .nodes
            .into_iter()
            .map(|(id, _)| id)
            .collect::<HashSet<_>>();
        original.items.reverse();
        let reordered_ids = original
            .tree_update()
            .unwrap()
            .nodes
            .into_iter()
            .map(|(id, _)| id)
            .collect::<HashSet<_>>();
        assert_eq!(original_ids, reordered_ids);

        original.items[0].id = SemanticNodeId(17);
        assert_eq!(
            original.tree_update().unwrap_err(),
            SemanticTreeError::DuplicateNodeId
        );
    }

    #[test]
    fn equivalent_updates_and_bursts_coalesce_to_one_final_tree() {
        let mut coalescer = SemanticUpdateCoalescer::default();
        let first = snapshot();
        coalescer.stage(first.clone());
        assert!(
            coalescer
                .take_snapshot(SemanticOwnerTurn(1))
                .unwrap()
                .is_some()
        );
        coalescer.stage(first.clone());
        assert!(
            coalescer
                .take_snapshot(SemanticOwnerTurn(2))
                .unwrap()
                .is_none()
        );

        let mut intermediate = first.clone();
        intermediate.status = Some("Refreshing".to_owned());
        coalescer.stage(intermediate);
        assert!(
            coalescer
                .take_snapshot(SemanticOwnerTurn(3))
                .unwrap()
                .is_some()
        );
        let mut final_snapshot = first;
        final_snapshot.status = Some("Disconnected".to_owned());
        coalescer.stage(final_snapshot);
        assert!(
            coalescer
                .take_snapshot(SemanticOwnerTurn(3))
                .unwrap()
                .is_none()
        );
        let snapshot = coalescer
            .take_snapshot(SemanticOwnerTurn(4))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.status.as_deref(), Some("Disconnected"));
    }

    #[test]
    fn search_empty_and_error_state_have_one_polite_announcement() {
        let mut empty = snapshot();
        empty.items.clear();
        empty.query = "missing".to_owned();
        empty.result_count = 0;
        empty.status = Some("No matches".to_owned());
        empty.focus = SemanticFocus::Search;
        let update = empty.tree_update().unwrap();
        assert_eq!(
            node(&update, SEARCH_NODE_ID).description(),
            Some("0 results")
        );
        let status = node(&update, STATUS_NODE_ID);
        assert_eq!(status.label(), Some("No matches"));
        assert_eq!(status.live(), Some(Live::Polite));
        assert!(status.is_live_atomic());
    }

    #[test]
    fn action_queue_is_bounded_and_coalesces_focus_and_search() {
        let wakes = Arc::new(Mutex::new(0));
        let wake_counter = Arc::clone(&wakes);
        let queue = SemanticActionQueue::new(move || *wake_counter.lock().unwrap() += 1);
        assert!(queue.push(SemanticAction::Focus(SemanticNodeId(16))));
        assert!(queue.push(SemanticAction::Focus(SemanticNodeId(17))));
        assert!(queue.push(SemanticAction::SetSearch("a".to_owned())));
        assert!(queue.push(SemanticAction::SetSearch("ab".to_owned())));
        assert_eq!(
            queue.drain(),
            vec![
                SemanticAction::Focus(SemanticNodeId(17)),
                SemanticAction::SetSearch("ab".to_owned())
            ]
        );
        assert_eq!(*wakes.lock().unwrap(), 4);
        for id in FIRST_ITEM_NODE_ID
            ..FIRST_ITEM_NODE_ID + u64::try_from(ACCESSIBILITY_ACTION_QUEUE_CAPACITY).unwrap()
        {
            assert!(queue.push(SemanticAction::Activate(SemanticNodeId(id))));
        }
        assert!(!queue.push(SemanticAction::Activate(SemanticNodeId(999))));
    }

    #[test]
    fn text_and_focus_are_bounded_and_never_derive_from_terminal_content() {
        let mut invalid = snapshot();
        invalid.query = "x".repeat(MAX_ACCESSIBLE_QUERY_CHARS + 1);
        assert_eq!(
            invalid.tree_update().unwrap_err(),
            SemanticTreeError::InvalidText
        );
        invalid = snapshot();
        invalid.items[0].name = "unsafe\u{202e}label".to_owned();
        assert_eq!(
            invalid.tree_update().unwrap_err(),
            SemanticTreeError::InvalidText
        );
        invalid = snapshot();
        invalid.focus = SemanticFocus::Item(SemanticNodeId(99));
        assert_eq!(
            invalid.tree_update().unwrap_err(),
            SemanticTreeError::InvalidFocus
        );
    }
}
