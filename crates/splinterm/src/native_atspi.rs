//! Splinterm-owned AT-SPI projection for the bounded navigation tree.
//!
//! AccessKit remains the platform-neutral semantic model. This module owns the
//! Linux transport because the current AccessKit Unix adapter does not expose
//! expanded/current state or distinct navigation actions through AT-SPI.

// zbus interface methods must take `self` and protocol-defined arguments even
// when a particular bounded implementation does not consume them.
#![allow(clippy::unused_self, clippy::used_underscore_binding)]

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
};

use atspi::{
    Action as AtspiAction, CoordType, Granularity, Interface, InterfaceSet, ObjectRef,
    ObjectRefOwned, Politeness, RelationType, Role, ScrollType, State, StateSet,
    events::EventBodyBorrowed, proxy::socket::SocketProxy,
};
use zbus::{
    blocking::{Connection, Proxy, connection::Builder},
    interface,
    names::OwnedUniqueName,
    zvariant::ObjectPath,
};

use crate::accessibility::{
    ROOT_NODE_ID, SEARCH_NODE_ID, STATUS_NODE_ID, SemanticAction, SemanticActionQueue,
    SemanticAvailability, SemanticFocus, SemanticNavigationSnapshot, SemanticNodeId,
    TERMINAL_NODE_ID, TREE_NODE_ID, valid_query_text,
};

const APPLICATION_PATH: &str = "/org/a11y/atspi/accessible/root";
const NODE_PATH_PREFIX: &str = "/org/splinterm/accessibility";
#[cfg(test)]
const NULL_PATH: &str = "/org/a11y/atspi/null";

#[derive(Clone)]
struct SharedTree {
    snapshot: Arc<RwLock<SemanticNavigationSnapshot>>,
    window_focused: Arc<RwLock<bool>>,
    actions: SemanticActionQueue,
    bus_name: Arc<RwLock<Option<OwnedUniqueName>>>,
    desktop: Arc<RwLock<ObjectRefOwned>>,
    application_id: Arc<Mutex<i32>>,
}

impl SharedTree {
    fn new(snapshot: SemanticNavigationSnapshot, actions: SemanticActionQueue) -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            window_focused: Arc::new(RwLock::new(false)),
            actions,
            bus_name: Arc::new(RwLock::new(None)),
            desktop: Arc::new(RwLock::new(ObjectRefOwned::default())),
            application_id: Arc::new(Mutex::new(-1)),
        }
    }

    fn snapshot(&self) -> std::sync::RwLockReadGuard<'_, SemanticNavigationSnapshot> {
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_snapshot(&self, snapshot: SemanticNavigationSnapshot) {
        *self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = snapshot;
    }

    fn node(&self, id: SemanticNodeId) -> Option<NativeNode> {
        let snapshot = self.snapshot();
        if matches!(
            id,
            ROOT_NODE_ID | TERMINAL_NODE_ID | SEARCH_NODE_ID | TREE_NODE_ID | STATUS_NODE_ID
        ) || snapshot.items.iter().any(|item| item.id == id)
        {
            Some(NativeNode {
                tree: self.clone(),
                id,
            })
        } else {
            None
        }
    }

    fn object_ref(&self, id: SemanticNodeId) -> ObjectRefOwned {
        let Some(name) = self
            .bus_name
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return ObjectRefOwned::default();
        };
        ObjectRef::new_owned(name, node_path(id))
    }

    fn application_ref(&self) -> ObjectRefOwned {
        let Some(name) = self
            .bus_name
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return ObjectRefOwned::default();
        };
        ObjectRef::new_owned(
            name,
            ObjectPath::from_static_str_unchecked(APPLICATION_PATH),
        )
    }
}

#[derive(Clone)]
struct NativeNode {
    tree: SharedTree,
    id: SemanticNodeId,
}

impl NativeNode {
    fn item<'a>(
        &self,
        snapshot: &'a SemanticNavigationSnapshot,
    ) -> Option<&'a crate::accessibility::SemanticTreeItem> {
        snapshot.items.iter().find(|item| item.id == self.id)
    }

    fn name(&self) -> String {
        let snapshot = self.tree.snapshot();
        match self.id {
            ROOT_NODE_ID => "Splinterm".to_owned(),
            TERMINAL_NODE_ID => "Terminal".to_owned(),
            SEARCH_NODE_ID => "Search navigation".to_owned(),
            TREE_NODE_ID => "Lairs".to_owned(),
            STATUS_NODE_ID => snapshot.status.clone().unwrap_or_default(),
            _ => self
                .item(&snapshot)
                .map(|item| item.name.clone())
                .unwrap_or_default(),
        }
    }

    fn description(&self) -> String {
        let snapshot = self.tree.snapshot();
        match self.id {
            SEARCH_NODE_ID => match snapshot.result_count {
                1 => "1 result".to_owned(),
                count => format!("{count} results"),
            },
            _ => self
                .item(&snapshot)
                .map(|item| item.status.clone())
                .unwrap_or_default(),
        }
    }

    fn role(&self) -> Role {
        match self.id {
            ROOT_NODE_ID => Role::Frame,
            TERMINAL_NODE_ID => Role::Terminal,
            SEARCH_NODE_ID => Role::Entry,
            TREE_NODE_ID => Role::Tree,
            STATUS_NODE_ID => Role::StatusBar,
            _ => Role::TreeItem,
        }
    }

    fn localized_role_name(&self) -> &'static str {
        match self.id {
            ROOT_NODE_ID => "window",
            TERMINAL_NODE_ID => "terminal",
            SEARCH_NODE_ID => "search box",
            TREE_NODE_ID => "tree",
            STATUS_NODE_ID => "status bar",
            _ => "tree item",
        }
    }

    fn parent_id(&self) -> Option<SemanticNodeId> {
        let snapshot = self.tree.snapshot();
        match self.id {
            ROOT_NODE_ID => None,
            TERMINAL_NODE_ID | SEARCH_NODE_ID | TREE_NODE_ID | STATUS_NODE_ID => Some(ROOT_NODE_ID),
            _ => self
                .item(&snapshot)
                .map(|item| item.parent.unwrap_or(TREE_NODE_ID)),
        }
    }

    fn child_ids(&self) -> Vec<SemanticNodeId> {
        let snapshot = self.tree.snapshot();
        match self.id {
            ROOT_NODE_ID => vec![
                TERMINAL_NODE_ID,
                SEARCH_NODE_ID,
                TREE_NODE_ID,
                STATUS_NODE_ID,
            ],
            TREE_NODE_ID => snapshot
                .items
                .iter()
                .filter(|item| item.parent.is_none())
                .map(|item| item.id)
                .collect(),
            _ => snapshot
                .items
                .iter()
                .filter(|item| item.parent == Some(self.id))
                .map(|item| item.id)
                .collect(),
        }
    }

    fn index_in_parent(&self) -> i32 {
        let Some(parent) = self.parent_id() else {
            return 0;
        };
        self.tree
            .node(parent)
            .and_then(|parent| {
                parent
                    .child_ids()
                    .iter()
                    .position(|candidate| *candidate == self.id)
            })
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1)
    }

    fn is_focused(&self, snapshot: &SemanticNavigationSnapshot) -> bool {
        let focused = match snapshot.focus {
            SemanticFocus::Terminal => TERMINAL_NODE_ID,
            SemanticFocus::Search => SEARCH_NODE_ID,
            SemanticFocus::Tree => TREE_NODE_ID,
            SemanticFocus::Item(id) => id,
        };
        focused == self.id
            && *self
                .tree
                .window_focused
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn state(&self) -> StateSet {
        let snapshot = self.tree.snapshot();
        if SemanticNodeId::item(self.id.0).is_some() && self.item(&snapshot).is_none() {
            return StateSet::new(State::Defunct);
        }
        let mut states = StateSet::new(State::Visible | State::Showing);
        if self.id == ROOT_NODE_ID
            && *self
                .tree
                .window_focused
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            states.insert(State::Active);
        }
        if matches!(self.id, TERMINAL_NODE_ID | SEARCH_NODE_ID | TREE_NODE_ID)
            || self.item(&snapshot).is_some()
        {
            states.insert(State::Focusable);
        }
        if self.is_focused(&snapshot) {
            states.insert(State::Focused);
        }
        if self.id == SEARCH_NODE_ID {
            states.insert(State::Editable | State::SelectableText | State::SingleLine);
        }
        if let Some(item) = self.item(&snapshot) {
            states.insert(State::Selectable);
            if item.selected {
                states.insert(State::Selected);
            }
            if item.current {
                states.insert(State::Active);
            }
            if let Some(expanded) = item.expanded {
                states.insert(State::Expandable);
                states.insert(if expanded {
                    State::Expanded
                } else {
                    State::Collapsed
                });
            }
            match item.availability {
                SemanticAvailability::Enabled => {
                    states.insert(State::Enabled | State::Sensitive);
                }
                SemanticAvailability::Disabled => {}
                SemanticAvailability::Pending => states.insert(State::Busy),
            }
        } else if self.id != STATUS_NODE_ID {
            states.insert(State::Enabled | State::Sensitive);
        }
        states
    }

    fn attributes(&self) -> HashMap<&'static str, String> {
        let snapshot = self.tree.snapshot();
        let mut attributes = HashMap::new();
        if let Some(item) = self.item(&snapshot) {
            attributes.insert("level", item.level.to_string());
            if item.current {
                attributes.insert("current", "true".to_owned());
            }
            if let Some(expanded) = item.expanded {
                attributes.insert("expanded", expanded.to_string());
            }
            attributes.insert(
                "availability",
                match item.availability {
                    SemanticAvailability::Enabled => "enabled",
                    SemanticAvailability::Disabled => "disabled",
                    SemanticAvailability::Pending => "pending",
                }
                .to_owned(),
            );
        }
        attributes
    }

    fn action_names(&self) -> Vec<&'static str> {
        let snapshot = self.tree.snapshot();
        let mut actions = Vec::new();
        if matches!(self.id, TERMINAL_NODE_ID | SEARCH_NODE_ID | TREE_NODE_ID)
            || self.item(&snapshot).is_some()
        {
            actions.push("focus");
        }
        if let Some(item) = self.item(&snapshot) {
            if let Some(expanded) = item.expanded {
                actions.push(if expanded { "collapse" } else { "expand" });
            }
            if item.availability == SemanticAvailability::Enabled {
                actions.push("activate");
            }
        }
        actions
    }

    fn interfaces(&self) -> InterfaceSet {
        let mut interfaces = InterfaceSet::new(Interface::Accessible);
        if !self.action_names().is_empty() {
            interfaces.insert(Interface::Action);
        }
        if self.id == SEARCH_NODE_ID {
            interfaces.insert(Interface::EditableText);
            interfaces.insert(Interface::Text);
        }
        interfaces
    }

    fn perform_action(&self, index: i32) -> bool {
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        let Some(name) = self.action_names().get(index).copied() else {
            return false;
        };
        let action = match name {
            "focus" => SemanticAction::Focus(self.id),
            "expand" => SemanticAction::SetExpanded {
                node: self.id,
                expanded: true,
            },
            "collapse" => SemanticAction::SetExpanded {
                node: self.id,
                expanded: false,
            },
            "activate" => SemanticAction::Activate(self.id),
            _ => return false,
        };
        self.tree.actions.push(action)
    }
}

struct AccessibleInterface(NativeNode);

#[interface(name = "org.a11y.atspi.Accessible")]
impl AccessibleInterface {
    #[zbus(property)]
    fn name(&self) -> String {
        self.0.name()
    }

    #[zbus(property)]
    fn description(&self) -> String {
        self.0.description()
    }

    #[zbus(property)]
    fn parent(&self) -> ObjectRefOwned {
        self.0.parent_id().map_or_else(
            || self.0.tree.application_ref(),
            |id| self.0.tree.object_ref(id),
        )
    }

    #[zbus(property)]
    fn child_count(&self) -> i32 {
        i32::try_from(self.0.child_ids().len()).unwrap_or(i32::MAX)
    }

    #[zbus(property)]
    fn locale(&self) -> &'static str {
        ""
    }

    #[zbus(property)]
    fn accessible_id(&self) -> String {
        format!("splinterm-{}", self.0.id.0)
    }

    fn get_child_at_index(&self, index: i32) -> (ObjectRefOwned,) {
        let child = usize::try_from(index)
            .ok()
            .and_then(|index| self.0.child_ids().get(index).copied())
            .map_or_else(ObjectRefOwned::default, |id| self.0.tree.object_ref(id));
        (child,)
    }

    fn get_children(&self) -> Vec<ObjectRefOwned> {
        self.0
            .child_ids()
            .into_iter()
            .map(|id| self.0.tree.object_ref(id))
            .collect()
    }

    fn get_index_in_parent(&self) -> i32 {
        self.0.index_in_parent()
    }

    fn get_relation_set(&self) -> Vec<(RelationType, Vec<ObjectRefOwned>)> {
        Vec::new()
    }

    fn get_role(&self) -> Role {
        self.0.role()
    }

    fn get_localized_role_name(&self) -> &'static str {
        self.0.localized_role_name()
    }

    fn get_state(&self) -> StateSet {
        self.0.state()
    }

    fn get_attributes(&self) -> HashMap<&'static str, String> {
        self.0.attributes()
    }

    fn get_application(&self) -> (ObjectRefOwned,) {
        (self.0.tree.application_ref(),)
    }

    fn get_interfaces(&self) -> InterfaceSet {
        self.0.interfaces()
    }
}

struct ActionInterface(NativeNode);

#[interface(name = "org.a11y.atspi.Action")]
impl ActionInterface {
    #[zbus(property)]
    fn n_actions(&self) -> i32 {
        i32::try_from(self.0.action_names().len()).unwrap_or(i32::MAX)
    }

    fn get_description(&self, index: i32) -> String {
        self.get_name(index)
    }

    fn get_name(&self, index: i32) -> String {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.0.action_names().get(index).copied())
            .unwrap_or_default()
            .to_owned()
    }

    fn get_localized_name(&self, index: i32) -> String {
        self.get_name(index)
    }

    fn get_key_binding(&self, _index: i32) -> &'static str {
        ""
    }

    fn get_actions(&self) -> Vec<AtspiAction> {
        self.0
            .action_names()
            .into_iter()
            .map(|name| AtspiAction {
                name: name.to_owned(),
                description: name.to_owned(),
                keybinding: String::new(),
            })
            .collect()
    }

    fn do_action(&self, index: i32) -> bool {
        self.0.perform_action(index)
    }
}

struct EditableTextInterface(NativeNode);

#[interface(name = "org.a11y.atspi.EditableText")]
impl EditableTextInterface {
    fn set_text_contents(&self, value: &str) -> bool {
        if valid_query_text(value).is_err() {
            return false;
        }
        self.0
            .tree
            .actions
            .push(SemanticAction::SetSearch(value.to_owned()))
    }

    fn insert_text(&self, _position: i32, _text: &str, _length: i32) -> bool {
        false
    }

    fn copy_text(&self, _start_pos: i32, _end_pos: i32) {}

    fn cut_text(&self, _start_pos: i32, _end_pos: i32) -> bool {
        false
    }

    fn delete_text(&self, _start_pos: i32, _end_pos: i32) -> bool {
        false
    }

    fn paste_text(&self, _position: i32) -> bool {
        false
    }
}

struct TextInterface(NativeNode);

impl TextInterface {
    fn query(&self) -> String {
        self.0.tree.snapshot().query.clone()
    }

    fn character_count_value(&self) -> i32 {
        i32::try_from(self.query().chars().count()).unwrap_or(i32::MAX)
    }

    fn bounded_text(&self, start_offset: i32, end_offset: i32) -> String {
        let query = self.query();
        let count = query.chars().count();
        let start = usize::try_from(start_offset).unwrap_or(0).min(count);
        let end = if end_offset < 0 {
            count
        } else {
            usize::try_from(end_offset).unwrap_or(count).min(count)
        };
        if start >= end {
            String::new()
        } else {
            query.chars().skip(start).take(end - start).collect()
        }
    }
}

#[interface(name = "org.a11y.atspi.Text")]
impl TextInterface {
    #[zbus(property)]
    fn character_count(&self) -> i32 {
        self.character_count_value()
    }

    #[zbus(property)]
    fn caret_offset(&self) -> i32 {
        self.character_count_value()
    }

    fn get_string_at_offset(&self, _offset: i32, _granularity: Granularity) -> (String, i32, i32) {
        let count = self.character_count_value();
        (self.query(), 0, count)
    }

    fn get_text(&self, start_offset: i32, end_offset: i32) -> String {
        self.bounded_text(start_offset, end_offset)
    }

    fn set_caret_offset(&self, _offset: i32) -> bool {
        false
    }

    fn get_attribute_value(&self, _offset: i32, _attribute_name: &str) -> &'static str {
        ""
    }

    fn get_attributes(&self, _offset: i32) -> (HashMap<&'static str, String>, i32, i32) {
        (HashMap::new(), 0, self.character_count_value())
    }

    fn get_default_attributes(&self) -> HashMap<&'static str, String> {
        HashMap::new()
    }

    fn get_character_extents(&self, _offset: i32, _coord_type: CoordType) -> (i32, i32, i32, i32) {
        (-1, -1, -1, -1)
    }

    fn get_offset_at_point(&self, _x: i32, _y: i32, _coord_type: CoordType) -> i32 {
        -1
    }

    fn get_n_selections(&self) -> i32 {
        0
    }

    fn get_selection(&self, _selection_num: i32) -> (i32, i32) {
        (-1, -1)
    }

    fn add_selection(&self, _start_offset: i32, _end_offset: i32) -> bool {
        false
    }

    fn remove_selection(&self, _selection_num: i32) -> bool {
        false
    }

    fn set_selection(&self, _selection_num: i32, _start_offset: i32, _end_offset: i32) -> bool {
        false
    }

    fn get_range_extents(
        &self,
        _start_offset: i32,
        _end_offset: i32,
        _coord_type: CoordType,
    ) -> (i32, i32, i32, i32) {
        (-1, -1, -1, -1)
    }

    fn get_attribute_run(
        &self,
        _offset: i32,
        _include_defaults: bool,
    ) -> (HashMap<&'static str, String>, i32, i32) {
        (HashMap::new(), 0, self.character_count_value())
    }

    fn scroll_substring_to(
        &self,
        _start_offset: i32,
        _end_offset: i32,
        _scroll_type: ScrollType,
    ) -> bool {
        false
    }

    fn scroll_substring_to_point(
        &self,
        _start_offset: i32,
        _end_offset: i32,
        _coord_type: CoordType,
        _x: i32,
        _y: i32,
    ) -> bool {
        false
    }
}

struct ApplicationAccessibleInterface(SharedTree);

#[interface(name = "org.a11y.atspi.Accessible")]
impl ApplicationAccessibleInterface {
    #[zbus(property)]
    fn name(&self) -> &'static str {
        "splinterm"
    }

    #[zbus(property)]
    fn description(&self) -> &'static str {
        ""
    }

    #[zbus(property)]
    fn parent(&self) -> ObjectRefOwned {
        self.0
            .desktop
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[zbus(property)]
    fn child_count(&self) -> i32 {
        1
    }

    #[zbus(property)]
    fn locale(&self) -> &'static str {
        ""
    }

    #[zbus(property)]
    fn accessible_id(&self) -> &'static str {
        "splinterm"
    }

    fn get_child_at_index(&self, index: i32) -> (ObjectRefOwned,) {
        (if index == 0 {
            self.0.object_ref(ROOT_NODE_ID)
        } else {
            ObjectRefOwned::default()
        },)
    }

    fn get_children(&self) -> Vec<ObjectRefOwned> {
        vec![self.0.object_ref(ROOT_NODE_ID)]
    }

    fn get_index_in_parent(&self) -> i32 {
        -1
    }

    fn get_relation_set(&self) -> Vec<(RelationType, Vec<ObjectRefOwned>)> {
        Vec::new()
    }

    fn get_role(&self) -> Role {
        Role::Application
    }

    fn get_localized_role_name(&self) -> &'static str {
        "application"
    }

    fn get_state(&self) -> StateSet {
        StateSet::new(State::Enabled | State::Sensitive | State::Visible | State::Showing)
    }

    fn get_attributes(&self) -> HashMap<&'static str, String> {
        HashMap::new()
    }

    fn get_application(&self) -> (ObjectRefOwned,) {
        (self.0.application_ref(),)
    }

    fn get_interfaces(&self) -> InterfaceSet {
        InterfaceSet::new(Interface::Accessible | Interface::Application)
    }
}

struct ApplicationInterface(SharedTree);

#[interface(name = "org.a11y.atspi.Application")]
impl ApplicationInterface {
    #[zbus(property)]
    fn toolkit_name(&self) -> &'static str {
        "Splinterm custom renderer"
    }

    #[zbus(property)]
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    #[zbus(property)]
    fn atspi_version(&self) -> &'static str {
        "2.1"
    }

    #[zbus(property)]
    fn id(&self) -> i32 {
        *self
            .0
            .application_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[zbus(property)]
    fn set_id(&mut self, id: i32) {
        *self
            .0
            .application_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = id;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeStateEvent {
    id: SemanticNodeId,
    state: &'static str,
    enabled: bool,
}

fn focus_events(
    previous: &SemanticNavigationSnapshot,
    next: &SemanticNavigationSnapshot,
    window_focused: bool,
) -> Vec<NativeStateEvent> {
    if !window_focused || focus_id(previous) == focus_id(next) {
        return Vec::new();
    }
    vec![
        NativeStateEvent {
            id: focus_id(previous),
            state: "focused",
            enabled: false,
        },
        NativeStateEvent {
            id: focus_id(next),
            state: "focused",
            enabled: true,
        },
    ]
}

fn state_events(
    previous: &SemanticNavigationSnapshot,
    next: &SemanticNavigationSnapshot,
) -> Vec<NativeStateEvent> {
    let previous = previous
        .items
        .iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<_, _>>();
    let mut events = Vec::new();
    for item in &next.items {
        let Some(old) = previous.get(&item.id) else {
            continue;
        };
        let mut changed = |state, before, after| {
            if before != after {
                events.push(NativeStateEvent {
                    id: item.id,
                    state,
                    enabled: after,
                });
            }
        };
        changed("selected", old.selected, item.selected);
        changed("active", old.current, item.current);
        changed(
            "expanded",
            old.expanded == Some(true),
            item.expanded == Some(true),
        );
        changed(
            "busy",
            old.availability == SemanticAvailability::Pending,
            item.availability == SemanticAvailability::Pending,
        );
        changed(
            "enabled",
            old.availability == SemanticAvailability::Enabled,
            item.availability == SemanticAvailability::Enabled,
        );
        changed(
            "sensitive",
            old.availability == SemanticAvailability::Enabled,
            item.availability == SemanticAvailability::Enabled,
        );
    }
    events
}

struct NativeConnection {
    connection: Connection,
    registered: HashSet<SemanticNodeId>,
}

impl NativeConnection {
    fn is_enabled(session: &Connection) -> zbus::Result<bool> {
        Proxy::new(session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Status")?
            .get_property("IsEnabled")
    }

    fn connect(session: &Connection, tree: &SharedTree) -> zbus::Result<Option<Self>> {
        if !Self::is_enabled(session)? {
            return Ok(None);
        }
        let bus = Proxy::new(session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus")?;
        let address: String = bus.call("GetAddress", &())?;
        let address = zbus::Address::try_from(address.as_str())?;
        let connection = Builder::address(address)?.build()?;
        let unique_name = connection
            .unique_name()
            .cloned()
            .ok_or_else(|| zbus::Error::Failure("AT-SPI bus has no unique name".to_owned()))?;
        *tree
            .bus_name
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(unique_name.clone());
        connection.object_server().at(
            APPLICATION_PATH,
            ApplicationAccessibleInterface(tree.clone()),
        )?;
        connection
            .object_server()
            .at(APPLICATION_PATH, ApplicationInterface(tree.clone()))?;
        let mut native = Self {
            connection,
            registered: HashSet::new(),
        };
        native.register_current_nodes(tree)?;
        let socket = zbus::block_on(SocketProxy::new(native.connection.inner()))?;
        let root = ObjectPath::from_static_str_unchecked(APPLICATION_PATH);
        let desktop = zbus::block_on(socket.embed(&(unique_name.as_str(), root)))?;
        *tree
            .desktop
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = desktop;
        Ok(Some(native))
    }

    fn register_current_nodes(&mut self, tree: &SharedTree) -> zbus::Result<()> {
        let snapshot = tree.snapshot();
        let mut ids = vec![
            ROOT_NODE_ID,
            TERMINAL_NODE_ID,
            SEARCH_NODE_ID,
            TREE_NODE_ID,
            STATUS_NODE_ID,
        ];
        ids.extend(snapshot.items.iter().map(|item| item.id));
        drop(snapshot);
        for id in ids {
            if self.registered.contains(&id) {
                continue;
            }
            let node = tree.node(id).expect("current semantic node must exist");
            let path = node_path(id);
            self.connection
                .object_server()
                .at(path.clone(), AccessibleInterface(node.clone()))?;
            if !node.action_names().is_empty() {
                self.connection
                    .object_server()
                    .at(path.clone(), ActionInterface(node.clone()))?;
            }
            if id == SEARCH_NODE_ID {
                self.connection
                    .object_server()
                    .at(path.clone(), EditableTextInterface(node.clone()))?;
                self.connection
                    .object_server()
                    .at(path, TextInterface(node))?;
            }
            self.registered.insert(id);
        }
        Ok(())
    }

    fn emit_announcement(&self, message: &str) -> zbus::Result<()> {
        let mut body = EventBodyBorrowed::default();
        body.detail1 = Politeness::Polite as i32;
        body.any_data = message.into();
        self.connection.emit_signal(
            Option::<&str>::None,
            node_path(STATUS_NODE_ID),
            "org.a11y.atspi.Event.Object",
            "Announcement",
            &body,
        )
    }

    fn emit_state(&self, id: SemanticNodeId, state: &str, enabled: bool) -> zbus::Result<()> {
        let mut body = EventBodyBorrowed::default();
        body.kind = state;
        body.detail1 = i32::from(enabled);
        body.any_data = enabled.into();
        self.connection.emit_signal(
            Option::<&str>::None,
            node_path(id),
            "org.a11y.atspi.Event.Object",
            "StateChanged",
            &body,
        )
    }

    fn emit_window_focus(&self, focused: bool) -> zbus::Result<()> {
        let mut body = EventBodyBorrowed::default();
        body.any_data = "Splinterm".into();
        self.connection.emit_signal(
            Option::<&str>::None,
            node_path(ROOT_NODE_ID),
            "org.a11y.atspi.Event.Window",
            if focused { "Activate" } else { "Deactivate" },
            &body,
        )
    }

    fn publish_update(
        &mut self,
        tree: &SharedTree,
        previous: &SemanticNavigationSnapshot,
        next: &SemanticNavigationSnapshot,
    ) -> zbus::Result<()> {
        self.register_current_nodes(tree)?;
        if previous.status != next.status
            && let Some(message) = next.status.as_deref()
        {
            self.emit_announcement(message)?;
        }
        for event in state_events(previous, next) {
            self.emit_state(event.id, event.state, event.enabled)?;
        }
        let window_focused = *tree
            .window_focused
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for event in focus_events(previous, next, window_focused) {
            self.emit_state(event.id, event.state, event.enabled)?;
        }
        Ok(())
    }
}

pub(crate) struct NativeAtspiPublisher {
    session: Option<Connection>,
    native: Option<NativeConnection>,
    tree: SharedTree,
    transport_error: Option<String>,
}

impl NativeAtspiPublisher {
    pub(crate) fn new(snapshot: SemanticNavigationSnapshot, actions: SemanticActionQueue) -> Self {
        let tree = SharedTree::new(snapshot, actions);
        let session = Connection::session().ok();
        let (native, transport_error) = session.as_ref().map_or_else(
            || (None, Some("session bus is unavailable".to_owned())),
            |session| match NativeConnection::connect(session, &tree) {
                Ok(native) => (native, None),
                Err(error) => (None, Some(error.to_string())),
            },
        );
        Self {
            session,
            native,
            tree,
            transport_error,
        }
    }

    pub(crate) fn publish(&mut self, snapshot: &SemanticNavigationSnapshot) {
        let previous = self.tree.snapshot().clone();
        self.tree.set_snapshot(snapshot.clone());
        if self.session.is_none() {
            self.session = Connection::session().ok();
        }
        let Some(session) = &self.session else {
            return;
        };
        match NativeConnection::is_enabled(session) {
            Ok(false) => {
                self.native = None;
                self.transport_error = None;
                *self
                    .tree
                    .bus_name
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                return;
            }
            Ok(true) => {}
            Err(error) => {
                self.transport_error = Some(error.to_string());
                return;
            }
        }
        let newly_connected = self.native.is_none();
        if newly_connected {
            match NativeConnection::connect(session, &self.tree) {
                Ok(native) => {
                    self.native = native;
                    self.transport_error = None;
                }
                Err(error) => {
                    self.transport_error = Some(error.to_string());
                    return;
                }
            }
        }
        if let Some(native) = &mut self.native {
            let result = if newly_connected {
                native.register_current_nodes(&self.tree)
            } else {
                native.publish_update(&self.tree, &previous, snapshot)
            };
            if let Err(error) = result {
                self.transport_error = Some(error.to_string());
            } else {
                self.transport_error = None;
            }
        }
    }

    pub(crate) fn transport_error(&self) -> Option<&str> {
        self.transport_error.as_deref()
    }

    pub(crate) fn update_window_focus_state(&mut self, focused: bool) {
        let mut window_focused = self
            .tree
            .window_focused
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *window_focused == focused {
            return;
        }
        *window_focused = focused;
        drop(window_focused);
        if let Some(native) = &self.native {
            let result = native.emit_window_focus(focused).and_then(|()| {
                native.emit_state(focus_id(&self.tree.snapshot()), "focused", focused)
            });
            if let Err(error) = result {
                self.transport_error = Some(error.to_string());
            } else {
                self.transport_error = None;
            }
        }
    }
}

fn focus_id(snapshot: &SemanticNavigationSnapshot) -> SemanticNodeId {
    match snapshot.focus {
        SemanticFocus::Terminal => TERMINAL_NODE_ID,
        SemanticFocus::Search => SEARCH_NODE_ID,
        SemanticFocus::Tree => TREE_NODE_ID,
        SemanticFocus::Item(id) => id,
    }
}

fn node_path(id: SemanticNodeId) -> ObjectPath<'static> {
    ObjectPath::try_from(format!("{NODE_PATH_PREFIX}/{}", id.0))
        .expect("semantic node ID creates a valid object path")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::{MAX_ACCESSIBLE_QUERY_CHARS, SemanticTreeItem};

    fn snapshot() -> SemanticNavigationSnapshot {
        let mut lair = item(16, None, 1, "work");
        lair.expanded = Some(true);
        let mut dojo = item(17, Some(16), 2, "editor");
        dojo.expanded = Some(false);
        dojo.selected = true;
        dojo.current = true;
        dojo.status = "Here, 2 of 2 running".to_owned();
        let mut splint = item(18, Some(17), 3, "shell");
        splint.availability = SemanticAvailability::Pending;
        splint.status = "Starting".to_owned();
        SemanticNavigationSnapshot {
            items: vec![lair, dojo, splint],
            query: String::new(),
            result_count: 3,
            status: Some("3 results".to_owned()),
            focus: SemanticFocus::Item(SemanticNodeId(17)),
        }
    }

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

    #[test]
    fn native_projection_preserves_states_hierarchy_and_actions() {
        let actions = SemanticActionQueue::new(|| {});
        let tree = SharedTree::new(snapshot(), actions.clone());
        *tree.window_focused.write().unwrap() = true;
        let dojo = tree.node(SemanticNodeId(17)).unwrap();
        assert_eq!(dojo.parent_id(), Some(SemanticNodeId(16)));
        assert_eq!(dojo.child_ids(), vec![SemanticNodeId(18)]);
        assert_eq!(dojo.role(), Role::TreeItem);
        assert_eq!(dojo.description(), "Here, 2 of 2 running");
        let state = dojo.state();
        assert!(state.contains(State::Focused));
        assert!(state.contains(State::Selected));
        assert!(state.contains(State::Active));
        assert!(state.contains(State::Expandable));
        assert!(state.contains(State::Collapsed));
        assert_eq!(
            dojo.attributes().get("level").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            dojo.attributes().get("current").map(String::as_str),
            Some("true")
        );
        assert_eq!(dojo.action_names(), vec!["focus", "expand", "activate"]);
        assert!(dojo.perform_action(1));
        assert_eq!(
            actions.drain(),
            vec![SemanticAction::SetExpanded {
                node: SemanticNodeId(17),
                expanded: true,
            }]
        );

        let pending = tree.node(SemanticNodeId(18)).unwrap();
        assert!(pending.state().contains(State::Busy));
        assert!(!pending.state().contains(State::Enabled));
        assert_eq!(pending.action_names(), vec!["focus"]);
    }

    #[test]
    fn native_state_events_report_only_changed_independent_axes() {
        let previous = snapshot();
        let mut next = previous.clone();
        next.items[0].expanded = Some(false);
        next.items[1].selected = false;
        next.items[1].current = false;
        next.items[2].availability = SemanticAvailability::Enabled;
        assert_eq!(
            state_events(&previous, &next),
            vec![
                NativeStateEvent {
                    id: SemanticNodeId(16),
                    state: "expanded",
                    enabled: false,
                },
                NativeStateEvent {
                    id: SemanticNodeId(17),
                    state: "selected",
                    enabled: false,
                },
                NativeStateEvent {
                    id: SemanticNodeId(17),
                    state: "active",
                    enabled: false,
                },
                NativeStateEvent {
                    id: SemanticNodeId(18),
                    state: "busy",
                    enabled: false,
                },
                NativeStateEvent {
                    id: SemanticNodeId(18),
                    state: "enabled",
                    enabled: true,
                },
                NativeStateEvent {
                    id: SemanticNodeId(18),
                    state: "sensitive",
                    enabled: true,
                },
            ]
        );

        next.focus = SemanticFocus::Terminal;
        assert!(focus_events(&previous, &next, false).is_empty());
        assert_eq!(
            focus_events(&previous, &next, true),
            vec![
                NativeStateEvent {
                    id: SemanticNodeId(17),
                    state: "focused",
                    enabled: false,
                },
                NativeStateEvent {
                    id: TERMINAL_NODE_ID,
                    state: "focused",
                    enabled: true,
                },
            ]
        );
    }

    #[test]
    fn native_search_edit_is_bounded_and_typed() {
        let actions = SemanticActionQueue::new(|| {});
        let tree = SharedTree::new(snapshot(), actions.clone());
        let search = tree.node(SEARCH_NODE_ID).unwrap();
        assert!(search.interfaces().contains(Interface::Text));
        assert!(search.interfaces().contains(Interface::EditableText));
        let text = TextInterface(search.clone());
        assert_eq!(text.character_count(), 0);
        assert_eq!(text.get_text(0, -1), "");
        assert_eq!(
            text.get_character_extents(0, CoordType::Screen),
            (-1, -1, -1, -1)
        );
        let interface = EditableTextInterface(search);
        assert!(interface.set_text_contents("edit"));
        assert!(!interface.set_text_contents(&"x".repeat(MAX_ACCESSIBLE_QUERY_CHARS + 1)));
        assert!(!interface.set_text_contents("line\nbreak"));
        assert!(!interface.set_text_contents("unsafe\u{202e}query"));
        assert_eq!(
            actions.drain(),
            vec![SemanticAction::SetSearch("edit".to_owned())]
        );
    }

    #[test]
    fn removed_ids_are_never_retargeted() {
        let actions = SemanticActionQueue::new(|| {});
        let tree = SharedTree::new(snapshot(), actions);
        assert!(tree.node(SemanticNodeId(18)).is_some());
        let mut next = snapshot();
        next.items.pop();
        tree.set_snapshot(next);
        assert!(tree.node(SemanticNodeId(18)).is_none());
        assert_eq!(
            node_path(SemanticNodeId(18)).as_str(),
            "/org/splinterm/accessibility/18"
        );
    }

    #[test]
    fn null_path_is_the_atspi_null_object() {
        assert_eq!(ObjectRefOwned::default().path_as_str(), NULL_PATH);
    }
}
