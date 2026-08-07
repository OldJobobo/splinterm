//! Platform-independent built-in command-palette state.

use splinterm_core::{Axis, DojoId, LairId, SplintId};

use super::WindowTopologyCommand;

const MAX_QUERY_BYTES: usize = 256;
const MAX_QUERY_SCALARS: usize = 128;
pub(crate) const COMMAND_PALETTE_PAGE_ITEMS: usize = 7;
const MAX_DOJO_NAME_BYTES: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DojoActionTarget {
    pub(crate) dojo_id: DojoId,
    pub(crate) name: String,
    pub(crate) pane_count: usize,
    pub(crate) splints: Vec<(SplintId, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenameDojoUi {
    target: DojoActionTarget,
    input: String,
    replace_on_edit: bool,
}

impl RenameDojoUi {
    pub(crate) fn new(target: DojoActionTarget) -> Self {
        Self {
            input: target.name.clone(),
            target,
            replace_on_edit: true,
        }
    }

    pub(crate) const fn target(&self) -> &DojoActionTarget {
        &self.target
    }

    pub(crate) fn input(&self) -> &str {
        &self.input
    }

    pub(crate) fn append_text(&mut self, text: &str) -> bool {
        let mut next = if self.replace_on_edit {
            String::new()
        } else {
            self.input.clone()
        };
        let mut accepted = false;
        for character in text.chars() {
            if character.is_control() || is_bidi_formatting(character) {
                continue;
            }
            if next.len().saturating_add(character.len_utf8()) > MAX_DOJO_NAME_BYTES {
                break;
            }
            next.push(character);
            accepted = true;
        }
        if !accepted {
            return false;
        }
        self.input = next;
        self.replace_on_edit = false;
        true
    }

    pub(crate) fn backspace(&mut self) -> bool {
        if self.replace_on_edit {
            self.input.clear();
            self.replace_on_edit = false;
            return true;
        }
        self.input.pop().is_some()
    }

    pub(crate) fn command(&self) -> Option<WindowTopologyCommand> {
        let name = self.input.trim();
        (!name.is_empty() && name.len() <= MAX_DOJO_NAME_BYTES).then(|| {
            WindowTopologyCommand::RenameDojo {
                dojo_id: self.target.dojo_id,
                name: name.to_owned(),
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TerminationDecision {
    #[default]
    Cancel,
    Terminate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminateDojoUi {
    target: DojoActionTarget,
    decision: TerminationDecision,
}

impl TerminateDojoUi {
    pub(crate) fn new(target: DojoActionTarget) -> Self {
        Self {
            target,
            decision: TerminationDecision::Cancel,
        }
    }

    pub(crate) const fn target(&self) -> &DojoActionTarget {
        &self.target
    }

    pub(crate) const fn decision(&self) -> TerminationDecision {
        self.decision
    }

    pub(crate) fn move_selection(&mut self) -> bool {
        self.decision = match self.decision {
            TerminationDecision::Cancel => TerminationDecision::Terminate,
            TerminationDecision::Terminate => TerminationDecision::Cancel,
        };
        true
    }

    pub(crate) fn select(&mut self, decision: TerminationDecision) -> bool {
        if self.decision == decision {
            return false;
        }
        self.decision = decision;
        true
    }

    pub(crate) fn command(&self) -> Option<WindowTopologyCommand> {
        (self.decision == TerminationDecision::Terminate).then_some(
            WindowTopologyCommand::TerminateDojo {
                dojo_id: self.target.dojo_id,
                splints: self.target.splints.clone(),
            },
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DojoPromptUi {
    Rename(RenameDojoUi),
    Terminate(TerminateDojoUi),
}

impl DojoPromptUi {
    pub(crate) fn rename(
        dojo_id: DojoId,
        name: String,
        pane_count: usize,
        splints: Vec<(SplintId, u64)>,
    ) -> Self {
        Self::Rename(RenameDojoUi::new(DojoActionTarget {
            dojo_id,
            name,
            pane_count,
            splints,
        }))
    }

    pub(crate) fn terminate(
        dojo_id: DojoId,
        name: String,
        pane_count: usize,
        splints: Vec<(SplintId, u64)>,
    ) -> Self {
        Self::Terminate(TerminateDojoUi::new(DojoActionTarget {
            dojo_id,
            name,
            pane_count,
            splints,
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TabMenuActionId {
    RenameTab,
    ActivateTab,
    NewDojo,
    CloseTab,
    CloseOtherTabs,
    TerminateDojo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabMenuContext {
    pub(crate) lair_id: LairId,
    pub(crate) dojo_id: DojoId,
    pub(crate) dojo_name: String,
    pub(crate) pane_count: usize,
    pub(crate) splints: Vec<(SplintId, u64)>,
    pub(crate) active: bool,
    pub(crate) other_dojo_ids: Vec<DojoId>,
}

impl TabMenuContext {
    fn action_target(&self) -> DojoActionTarget {
        DojoActionTarget {
            dojo_id: self.dojo_id,
            name: self.dojo_name.clone(),
            pane_count: self.pane_count,
            splints: self.splints.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TabMenuActionDescriptor {
    pub(crate) id: TabMenuActionId,
    pub(crate) title: &'static str,
}

pub(crate) const TAB_MENU_ACTIONS: [TabMenuActionDescriptor; 6] = [
    TabMenuActionDescriptor {
        id: TabMenuActionId::RenameTab,
        title: "Rename Tab",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::ActivateTab,
        title: "Activate Tab",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::NewDojo,
        title: "New Dojo",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::CloseTab,
        title: "Close Tab",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::CloseOtherTabs,
        title: "Close Other Tabs",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::TerminateDojo,
        title: "Terminate Dojo…",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TabMenuRightPress {
    Retarget(DojoId),
    Dismiss,
}

pub(crate) const fn tab_menu_right_press(target: Option<DojoId>) -> TabMenuRightPress {
    match target {
        Some(dojo_id) => TabMenuRightPress::Retarget(dojo_id),
        None => TabMenuRightPress::Dismiss,
    }
}

pub(crate) fn tab_menu_action_enabled(id: TabMenuActionId, context: &TabMenuContext) -> bool {
    match id {
        TabMenuActionId::ActivateTab => !context.active,
        TabMenuActionId::CloseOtherTabs => !context.other_dojo_ids.is_empty(),
        TabMenuActionId::RenameTab
        | TabMenuActionId::NewDojo
        | TabMenuActionId::CloseTab
        | TabMenuActionId::TerminateDojo => true,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabContextMenuUi {
    context: TabMenuContext,
    selected: usize,
    hovered: Option<TabMenuActionId>,
}

impl TabContextMenuUi {
    pub(crate) fn new(context: TabMenuContext) -> Self {
        let selected = TAB_MENU_ACTIONS
            .iter()
            .position(|action| tab_menu_action_enabled(action.id, &context))
            .unwrap_or(0);
        Self {
            context,
            selected,
            hovered: None,
        }
    }

    pub(crate) fn context(&self) -> TabMenuContext {
        self.context.clone()
    }

    pub(crate) fn action_enabled(&self, action: TabMenuActionId) -> bool {
        tab_menu_action_enabled(action, &self.context)
    }

    pub(crate) const fn selected_action(&self) -> TabMenuActionId {
        TAB_MENU_ACTIONS[self.selected].id
    }

    pub(crate) const fn hovered(&self) -> Option<TabMenuActionId> {
        self.hovered
    }

    pub(crate) fn update_hovered(&mut self, hovered: Option<TabMenuActionId>) -> bool {
        let hovered = hovered.filter(|action| self.action_enabled(*action));
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        true
    }

    pub(crate) fn move_selection(&mut self, delta: isize) -> bool {
        let previous = self.selected;
        let count = TAB_MENU_ACTIONS.len();
        let enabled_count = TAB_MENU_ACTIONS
            .iter()
            .filter(|action| self.action_enabled(action.id))
            .count();
        if enabled_count == 0 || delta == 0 {
            return false;
        }
        let steps = delta.unsigned_abs() % enabled_count;
        for _ in 0..steps {
            for distance in 1..=count {
                let candidate = if delta.is_negative() {
                    self.selected.saturating_add(count).saturating_sub(distance) % count
                } else {
                    self.selected.saturating_add(distance) % count
                };
                if self.action_enabled(TAB_MENU_ACTIONS[candidate].id) {
                    self.selected = candidate;
                    break;
                }
            }
        }
        self.selected != previous
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TabMenuDispatch {
    Topology(WindowTopologyCommand),
    Rename(DojoActionTarget),
    ConfirmTermination(DojoActionTarget),
}

pub(crate) fn tab_menu_dispatch(
    id: TabMenuActionId,
    context: &TabMenuContext,
) -> Option<TabMenuDispatch> {
    if !tab_menu_action_enabled(id, context) {
        return None;
    }
    match id {
        TabMenuActionId::RenameTab => Some(TabMenuDispatch::Rename(context.action_target())),
        TabMenuActionId::ActivateTab => Some(TabMenuDispatch::Topology(
            WindowTopologyCommand::ActivateTab {
                dojo_id: context.dojo_id,
            },
        )),
        TabMenuActionId::NewDojo => {
            Some(TabMenuDispatch::Topology(WindowTopologyCommand::NewDojo {
                lair_id: context.lair_id,
            }))
        }
        TabMenuActionId::CloseTab => {
            Some(TabMenuDispatch::Topology(WindowTopologyCommand::CloseTab {
                dojo_id: context.dojo_id,
            }))
        }
        TabMenuActionId::CloseOtherTabs => Some(TabMenuDispatch::Topology(
            WindowTopologyCommand::CloseTabs {
                retain_dojo_id: context.dojo_id,
                dojo_ids: context.other_dojo_ids.clone(),
            },
        )),
        TabMenuActionId::TerminateDojo => {
            Some(TabMenuDispatch::ConfirmTermination(context.action_target()))
        }
    }
}

pub(crate) fn tab_menu_descriptor(id: TabMenuActionId) -> TabMenuActionDescriptor {
    TAB_MENU_ACTIONS
        .iter()
        .copied()
        .find(|descriptor| descriptor.id == id)
        .expect("tab menu action identity has one descriptor")
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum BuiltInCommandId {
    RecentSessions,
    NewSession,
    RenameCurrentTab,
    NewDojo,
    PreviousDojo,
    NextDojo,
    CloseCurrentTab,
    CloseOtherTabs,
    TerminateCurrentDojo,
    SplitHorizontal,
    SplitVertical,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CloseFocusedPane,
    ResizePaneSmaller,
    ResizePaneLarger,
    SearchScrollback,
    PageUp,
    PageDown,
    ReturnToLive,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    RequestControl,
    ReleaseControl,
    ForceControl,
    RevokeAllAccess,
    AcceptControlTransfer,
    DenyControlTransfer,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CommandCategory {
    Sessions,
    Tabs,
    Panes,
    History,
    View,
    Control,
}

impl CommandCategory {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Sessions => "SESSION",
            Self::Tabs => "TAB",
            Self::Panes => "PANE",
            Self::History => "HISTORY",
            Self::View => "VIEW",
            Self::Control => "CONTROL",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_field_names,
    reason = "the explicit identity suffixes keep domain identifiers and captured destinations unambiguous"
)]
pub(crate) struct CommandPaletteContext {
    pub(crate) lair_id: LairId,
    pub(crate) dojo_id: DojoId,
    pub(crate) dojo_name: String,
    pub(crate) pane_count: usize,
    pub(crate) splint_id: SplintId,
    pub(crate) dojo_splints: Vec<(SplintId, u64)>,
    pub(crate) other_dojo_ids: Vec<DojoId>,
    pub(crate) previous_dojo_id: Option<DojoId>,
    pub(crate) next_dojo_id: Option<DojoId>,
    pub(crate) focus_left: Option<SplintId>,
    pub(crate) focus_right: Option<SplintId>,
    pub(crate) focus_up: Option<SplintId>,
    pub(crate) focus_down: Option<SplintId>,
    pub(crate) viewport_detached: bool,
    pub(crate) controller_active: bool,
    pub(crate) grant_ids: Vec<u64>,
    pub(crate) pending_transfer_id: Option<u64>,
}

impl CommandPaletteContext {
    fn action_target(&self) -> DojoActionTarget {
        DojoActionTarget {
            dojo_id: self.dojo_id,
            name: self.dojo_name.clone(),
            pane_count: self.pane_count,
            splints: self.dojo_splints.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandZoomAction {
    Increase,
    Decrease,
    Reset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandHistoryAction {
    Search,
    PageUp,
    PageDown,
    ReturnToLive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandControlAction {
    Request,
    Release,
    Force,
    Accept(u64),
    Deny(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BuiltInCommandDispatch {
    Topology(WindowTopologyCommand),
    Focus(SplintId),
    Zoom(CommandZoomAction),
    History {
        target: SplintId,
        action: CommandHistoryAction,
    },
    Control {
        target: SplintId,
        action: CommandControlAction,
    },
    RevokeAccess {
        target: SplintId,
        grant_ids: Vec<u64>,
    },
    Rename(DojoActionTarget),
    ConfirmTermination(DojoActionTarget),
    RecentSessions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BuiltInCommandDescriptor {
    pub(crate) id: BuiltInCommandId,
    pub(crate) category: CommandCategory,
    pub(crate) title: &'static str,
    pub(crate) keywords: &'static [&'static str],
    pub(crate) shortcut: &'static str,
}

pub(crate) const BUILT_IN_COMMANDS: [BuiltInCommandDescriptor; 31] = [
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::RecentSessions,
        category: CommandCategory::Sessions,
        title: "Open recent sessions",
        keywords: &["open", "recent", "session", "lair", "dojo", "reopen"],
        shortcut: "Ctrl+Shift+S",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::NewSession,
        category: CommandCategory::Sessions,
        title: "New session",
        keywords: &["new", "session", "lair", "window"],
        shortcut: "",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::RenameCurrentTab,
        category: CommandCategory::Tabs,
        title: "Rename current tab",
        keywords: &["rename", "name", "current", "dojo", "tab"],
        shortcut: "",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::NewDojo,
        category: CommandCategory::Tabs,
        title: "New Dojo",
        keywords: &["new", "dojo", "tab"],
        shortcut: "Ctrl+Shift+D",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::PreviousDojo,
        category: CommandCategory::Tabs,
        title: "Previous Dojo",
        keywords: &["previous", "dojo", "tab", "left"],
        shortcut: "Ctrl+Shift+Tab",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::NextDojo,
        category: CommandCategory::Tabs,
        title: "Next Dojo",
        keywords: &["next", "dojo", "tab", "right"],
        shortcut: "Ctrl+Tab",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::CloseCurrentTab,
        category: CommandCategory::Tabs,
        title: "Close current tab",
        keywords: &["close", "detach", "current", "dojo", "tab"],
        shortcut: "Ctrl+Shift+Q",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::CloseOtherTabs,
        category: CommandCategory::Tabs,
        title: "Close other tabs",
        keywords: &["close", "detach", "other", "dojo", "tabs"],
        shortcut: "",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::TerminateCurrentDojo,
        category: CommandCategory::Tabs,
        title: "Terminate current Dojo…",
        keywords: &["terminate", "kill", "close", "current", "dojo", "panes"],
        shortcut: "",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::SplitHorizontal,
        category: CommandCategory::Panes,
        title: "Split pane horizontally",
        keywords: &["split", "pane", "horizontal", "down"],
        shortcut: "Ctrl+Shift+Enter",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::SplitVertical,
        category: CommandCategory::Panes,
        title: "Split pane vertically",
        keywords: &["split", "pane", "vertical", "right"],
        shortcut: "Ctrl+Shift+\\",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::FocusLeft,
        category: CommandCategory::Panes,
        title: "Focus pane left",
        keywords: &["focus", "pane", "move", "left"],
        shortcut: "Ctrl+Shift+Left",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::FocusRight,
        category: CommandCategory::Panes,
        title: "Focus pane right",
        keywords: &["focus", "pane", "move", "right"],
        shortcut: "Ctrl+Shift+Right",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::FocusUp,
        category: CommandCategory::Panes,
        title: "Focus pane up",
        keywords: &["focus", "pane", "move", "up"],
        shortcut: "Ctrl+Shift+Up",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::FocusDown,
        category: CommandCategory::Panes,
        title: "Focus pane down",
        keywords: &["focus", "pane", "move", "down"],
        shortcut: "Ctrl+Shift+Down",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::CloseFocusedPane,
        category: CommandCategory::Panes,
        title: "Close focused pane",
        keywords: &["close", "kill", "focused", "pane", "shell"],
        shortcut: "Ctrl+Shift+W",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ResizePaneSmaller,
        category: CommandCategory::Panes,
        title: "Resize pane smaller",
        keywords: &["resize", "pane", "ratio", "smaller", "decrease"],
        shortcut: "Ctrl+Shift+[",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ResizePaneLarger,
        category: CommandCategory::Panes,
        title: "Resize pane larger",
        keywords: &["resize", "pane", "ratio", "larger", "increase"],
        shortcut: "Ctrl+Shift+]",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::SearchScrollback,
        category: CommandCategory::History,
        title: "Search scrollback",
        keywords: &["search", "find", "history", "scrollback"],
        shortcut: "Ctrl+Shift+F",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::PageUp,
        category: CommandCategory::History,
        title: "Page up",
        keywords: &["history", "scrollback", "page", "up", "older"],
        shortcut: "Shift+PageUp",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::PageDown,
        category: CommandCategory::History,
        title: "Page down",
        keywords: &["history", "scrollback", "page", "down", "newer"],
        shortcut: "Shift+PageDown",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ReturnToLive,
        category: CommandCategory::History,
        title: "Return to live output",
        keywords: &["history", "scrollback", "return", "live", "bottom"],
        shortcut: "Shift+End",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ZoomIn,
        category: CommandCategory::View,
        title: "Zoom in",
        keywords: &["zoom", "font", "increase", "larger", "view"],
        shortcut: "Ctrl++",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ZoomOut,
        category: CommandCategory::View,
        title: "Zoom out",
        keywords: &["zoom", "font", "decrease", "smaller", "view"],
        shortcut: "Ctrl+-",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ResetZoom,
        category: CommandCategory::View,
        title: "Reset zoom",
        keywords: &["zoom", "font", "reset", "default", "view"],
        shortcut: "Ctrl+0",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::RequestControl,
        category: CommandCategory::Control,
        title: "Request control",
        keywords: &["request", "control", "transfer", "input"],
        shortcut: "Ctrl+Shift+T",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ReleaseControl,
        category: CommandCategory::Control,
        title: "Release control",
        keywords: &["release", "control", "transfer", "input"],
        shortcut: "Ctrl+Shift+L",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::ForceControl,
        category: CommandCategory::Control,
        title: "Force control transfer",
        keywords: &["force", "control", "transfer", "input"],
        shortcut: "Ctrl+Shift+U",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::RevokeAllAccess,
        category: CommandCategory::Control,
        title: "Revoke all access",
        keywords: &["revoke", "all", "access", "grants", "clients"],
        shortcut: "Ctrl+Shift+R",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::AcceptControlTransfer,
        category: CommandCategory::Control,
        title: "Accept pending control transfer",
        keywords: &["accept", "pending", "control", "transfer", "yes"],
        shortcut: "Ctrl+Shift+Y",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::DenyControlTransfer,
        category: CommandCategory::Control,
        title: "Deny pending control transfer",
        keywords: &["deny", "pending", "control", "transfer", "no"],
        shortcut: "Ctrl+Shift+N",
    },
];

fn is_bidi_formatting(character: char) -> bool {
    matches!(
        character,
        '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn descriptor_matches(descriptor: BuiltInCommandDescriptor, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_lowercase();
    descriptor.title.to_lowercase().contains(&query)
        || descriptor.category.label().to_lowercase().contains(&query)
        || descriptor
            .keywords
            .iter()
            .any(|keyword| keyword.contains(&query))
}

pub(crate) fn command_enabled(id: BuiltInCommandId, context: &CommandPaletteContext) -> bool {
    match id {
        BuiltInCommandId::PreviousDojo => context.previous_dojo_id.is_some(),
        BuiltInCommandId::NextDojo => context.next_dojo_id.is_some(),
        BuiltInCommandId::FocusLeft => context.focus_left.is_some(),
        BuiltInCommandId::FocusRight => context.focus_right.is_some(),
        BuiltInCommandId::FocusUp => context.focus_up.is_some(),
        BuiltInCommandId::FocusDown => context.focus_down.is_some(),
        BuiltInCommandId::CloseOtherTabs => !context.other_dojo_ids.is_empty(),
        BuiltInCommandId::ReturnToLive => context.viewport_detached,
        BuiltInCommandId::RequestControl | BuiltInCommandId::ForceControl => {
            !context.controller_active
        }
        BuiltInCommandId::ReleaseControl => context.controller_active,
        BuiltInCommandId::RevokeAllAccess => !context.grant_ids.is_empty(),
        BuiltInCommandId::AcceptControlTransfer | BuiltInCommandId::DenyControlTransfer => {
            context.pending_transfer_id.is_some()
        }
        BuiltInCommandId::RecentSessions
        | BuiltInCommandId::NewSession
        | BuiltInCommandId::RenameCurrentTab
        | BuiltInCommandId::NewDojo
        | BuiltInCommandId::CloseCurrentTab
        | BuiltInCommandId::TerminateCurrentDojo
        | BuiltInCommandId::SplitHorizontal
        | BuiltInCommandId::SplitVertical
        | BuiltInCommandId::CloseFocusedPane
        | BuiltInCommandId::ResizePaneSmaller
        | BuiltInCommandId::ResizePaneLarger
        | BuiltInCommandId::SearchScrollback
        | BuiltInCommandId::PageUp
        | BuiltInCommandId::PageDown
        | BuiltInCommandId::ZoomIn
        | BuiltInCommandId::ZoomOut
        | BuiltInCommandId::ResetZoom => true,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommandPaletteUi {
    context: CommandPaletteContext,
    query: String,
    filtered: Vec<BuiltInCommandId>,
    selected: usize,
    visible_start: usize,
    hovered: Option<BuiltInCommandId>,
}

impl CommandPaletteUi {
    pub(crate) fn new(context: CommandPaletteContext) -> Self {
        Self {
            context,
            query: String::new(),
            filtered: BUILT_IN_COMMANDS.iter().map(|command| command.id).collect(),
            selected: 0,
            visible_start: 0,
            hovered: None,
        }
    }

    pub(crate) fn context(&self) -> CommandPaletteContext {
        self.context.clone()
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(crate) fn filtered(&self) -> &[BuiltInCommandId] {
        &self.filtered
    }

    pub(crate) const fn selected_index(&self) -> usize {
        self.selected
    }

    pub(crate) const fn visible_start(&self) -> usize {
        self.visible_start
    }

    pub(crate) fn selected_command(&self) -> Option<BuiltInCommandId> {
        self.filtered.get(self.selected).copied()
    }

    pub(crate) fn selected_enabled_command(&self) -> Option<BuiltInCommandId> {
        self.selected_command()
            .filter(|command| self.command_enabled(*command))
    }

    pub(crate) fn command_enabled(&self, command: BuiltInCommandId) -> bool {
        command_enabled(command, &self.context)
    }

    pub(crate) const fn hovered(&self) -> Option<BuiltInCommandId> {
        self.hovered
    }

    pub(crate) fn update_hovered(&mut self, hovered: Option<BuiltInCommandId>) -> bool {
        let hovered = hovered.filter(|command| self.command_enabled(*command));
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        true
    }

    pub(crate) fn move_selection(&mut self, delta: isize) -> bool {
        let count = self.filtered.len();
        let enabled_count = self
            .filtered
            .iter()
            .filter(|command| self.command_enabled(**command))
            .count();
        if count == 0 || enabled_count == 0 || delta == 0 {
            return false;
        }
        let previous = self.selected;
        let steps = delta.unsigned_abs() % enabled_count;
        for _ in 0..steps {
            for distance in 1..=count {
                let candidate = if delta.is_negative() {
                    self.selected.saturating_add(count).saturating_sub(distance) % count
                } else {
                    self.selected.saturating_add(distance) % count
                };
                if self.command_enabled(self.filtered[candidate]) {
                    self.selected = candidate;
                    break;
                }
            }
        }
        self.ensure_selected_visible(COMMAND_PALETTE_PAGE_ITEMS);
        self.selected != previous
    }

    pub(crate) fn select_first(&mut self) -> bool {
        let Some(first) = self
            .filtered
            .iter()
            .position(|command| self.command_enabled(*command))
        else {
            return false;
        };
        let changed = self.selected != first || self.visible_start != 0;
        self.selected = first;
        self.visible_start = 0;
        changed
    }

    pub(crate) fn select_last(&mut self) -> bool {
        let Some(last) = self
            .filtered
            .iter()
            .rposition(|command| self.command_enabled(*command))
        else {
            return false;
        };
        let changed = self.selected != last;
        self.selected = last;
        self.ensure_selected_visible(COMMAND_PALETTE_PAGE_ITEMS);
        changed
    }

    pub(crate) fn append_text(&mut self, text: &str) -> bool {
        let before = self.query.clone();
        for character in text
            .chars()
            .filter(|character| !character.is_control() && !is_bidi_formatting(*character))
        {
            if self.query.chars().count() == MAX_QUERY_SCALARS
                || self.query.len().saturating_add(character.len_utf8()) > MAX_QUERY_BYTES
            {
                break;
            }
            self.query.push(character);
        }
        if self.query == before {
            return false;
        }
        self.refilter();
        true
    }

    pub(crate) fn backspace(&mut self) -> bool {
        if self.query.pop().is_none() {
            return false;
        }
        self.refilter();
        true
    }

    fn refilter(&mut self) {
        let selected = self.selected_command();
        self.filtered = BUILT_IN_COMMANDS
            .iter()
            .copied()
            .filter(|descriptor| descriptor_matches(*descriptor, &self.query))
            .map(|descriptor| descriptor.id)
            .collect();
        self.selected = selected
            .filter(|selected| self.command_enabled(*selected))
            .and_then(|selected| {
                self.filtered
                    .iter()
                    .position(|candidate| *candidate == selected)
            })
            .or_else(|| {
                self.filtered
                    .iter()
                    .position(|command| self.command_enabled(*command))
            })
            .unwrap_or(0);
        self.ensure_selected_visible(COMMAND_PALETTE_PAGE_ITEMS);
        self.hovered = None;
    }

    fn ensure_selected_visible(&mut self, capacity: usize) {
        if self.filtered.is_empty() || capacity == 0 {
            self.selected = 0;
            self.visible_start = 0;
            return;
        }
        self.selected = self.selected.min(self.filtered.len() - 1);
        if self.selected < self.visible_start {
            self.visible_start = self.selected;
        } else if self.selected >= self.visible_start.saturating_add(capacity) {
            self.visible_start = self.selected.saturating_add(1).saturating_sub(capacity);
        }
        self.visible_start = self
            .visible_start
            .min(self.filtered.len().saturating_sub(capacity));
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the closed built-in catalog keeps availability and typed dispatch visibly exhaustive"
)]
pub(crate) fn command_dispatch(
    id: BuiltInCommandId,
    context: &CommandPaletteContext,
) -> Option<BuiltInCommandDispatch> {
    if !command_enabled(id, context) {
        return None;
    }
    let dispatch = match id {
        BuiltInCommandId::RecentSessions => BuiltInCommandDispatch::RecentSessions,
        BuiltInCommandId::NewSession => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::NewLair)
        }
        BuiltInCommandId::RenameCurrentTab => {
            BuiltInCommandDispatch::Rename(context.action_target())
        }
        BuiltInCommandId::NewDojo => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::NewDojo {
                lair_id: context.lair_id,
            })
        }
        BuiltInCommandId::PreviousDojo => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::ActivateTab {
                dojo_id: context.previous_dojo_id?,
            })
        }
        BuiltInCommandId::NextDojo => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::ActivateTab {
                dojo_id: context.next_dojo_id?,
            })
        }
        BuiltInCommandId::CloseCurrentTab => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::CloseTab {
                dojo_id: context.dojo_id,
            })
        }
        BuiltInCommandId::CloseOtherTabs => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::CloseTabs {
                retain_dojo_id: context.dojo_id,
                dojo_ids: context.other_dojo_ids.clone(),
            })
        }
        BuiltInCommandId::TerminateCurrentDojo => {
            BuiltInCommandDispatch::ConfirmTermination(context.action_target())
        }
        BuiltInCommandId::SplitHorizontal => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::Split {
                dojo_id: context.dojo_id,
                target: context.splint_id,
                axis: Axis::Horizontal,
            })
        }
        BuiltInCommandId::SplitVertical => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::Split {
                dojo_id: context.dojo_id,
                target: context.splint_id,
                axis: Axis::Vertical,
            })
        }
        BuiltInCommandId::FocusLeft => BuiltInCommandDispatch::Focus(context.focus_left?),
        BuiltInCommandId::FocusRight => BuiltInCommandDispatch::Focus(context.focus_right?),
        BuiltInCommandId::FocusUp => BuiltInCommandDispatch::Focus(context.focus_up?),
        BuiltInCommandId::FocusDown => BuiltInCommandDispatch::Focus(context.focus_down?),
        BuiltInCommandId::CloseFocusedPane => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::Close {
                dojo_id: context.dojo_id,
                target: context.splint_id,
            })
        }
        BuiltInCommandId::ResizePaneSmaller => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::AdjustRatio {
                dojo_id: context.dojo_id,
                target: context.splint_id,
                delta: -50,
            })
        }
        BuiltInCommandId::ResizePaneLarger => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::AdjustRatio {
                dojo_id: context.dojo_id,
                target: context.splint_id,
                delta: 50,
            })
        }
        BuiltInCommandId::SearchScrollback => BuiltInCommandDispatch::History {
            target: context.splint_id,
            action: CommandHistoryAction::Search,
        },
        BuiltInCommandId::PageUp => BuiltInCommandDispatch::History {
            target: context.splint_id,
            action: CommandHistoryAction::PageUp,
        },
        BuiltInCommandId::PageDown => BuiltInCommandDispatch::History {
            target: context.splint_id,
            action: CommandHistoryAction::PageDown,
        },
        BuiltInCommandId::ReturnToLive => BuiltInCommandDispatch::History {
            target: context.splint_id,
            action: CommandHistoryAction::ReturnToLive,
        },
        BuiltInCommandId::ZoomIn => BuiltInCommandDispatch::Zoom(CommandZoomAction::Increase),
        BuiltInCommandId::ZoomOut => BuiltInCommandDispatch::Zoom(CommandZoomAction::Decrease),
        BuiltInCommandId::ResetZoom => BuiltInCommandDispatch::Zoom(CommandZoomAction::Reset),
        BuiltInCommandId::RequestControl => BuiltInCommandDispatch::Control {
            target: context.splint_id,
            action: CommandControlAction::Request,
        },
        BuiltInCommandId::ReleaseControl => BuiltInCommandDispatch::Control {
            target: context.splint_id,
            action: CommandControlAction::Release,
        },
        BuiltInCommandId::ForceControl => BuiltInCommandDispatch::Control {
            target: context.splint_id,
            action: CommandControlAction::Force,
        },
        BuiltInCommandId::RevokeAllAccess => BuiltInCommandDispatch::RevokeAccess {
            target: context.splint_id,
            grant_ids: context.grant_ids.clone(),
        },
        BuiltInCommandId::AcceptControlTransfer => BuiltInCommandDispatch::Control {
            target: context.splint_id,
            action: CommandControlAction::Accept(context.pending_transfer_id?),
        },
        BuiltInCommandId::DenyControlTransfer => BuiltInCommandDispatch::Control {
            target: context.splint_id,
            action: CommandControlAction::Deny(context.pending_transfer_id?),
        },
    };
    Some(dispatch)
}

pub(crate) fn command_descriptor(id: BuiltInCommandId) -> BuiltInCommandDescriptor {
    BUILT_IN_COMMANDS
        .iter()
        .copied()
        .find(|descriptor| descriptor.id == id)
        .expect("built-in command identity has one descriptor")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> CommandPaletteUi {
        let splint_id = SplintId::new();
        CommandPaletteUi::new(CommandPaletteContext {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            dojo_name: "current".to_owned(),
            pane_count: 3,
            splint_id,
            dojo_splints: vec![(splint_id, 1), (SplintId::new(), 2), (SplintId::new(), 3)],
            other_dojo_ids: vec![DojoId::new(), DojoId::new()],
            previous_dojo_id: Some(DojoId::new()),
            next_dojo_id: Some(DojoId::new()),
            focus_left: Some(SplintId::new()),
            focus_right: Some(SplintId::new()),
            focus_up: Some(SplintId::new()),
            focus_down: Some(SplintId::new()),
            viewport_detached: true,
            controller_active: false,
            grant_ids: vec![7, 9],
            pending_transfer_id: Some(42),
        })
    }

    #[test]
    fn rename_prompt_is_prefilled_bounded_sanitized_and_exact() {
        let dojo_id = DojoId::new();
        let target = DojoActionTarget {
            dojo_id,
            name: "terminal".to_owned(),
            pane_count: 3,
            splints: vec![(SplintId::new(), 1)],
        };
        let mut rename = RenameDojoUi::new(target.clone());
        assert_eq!(rename.target(), &target);
        assert_eq!(rename.input(), "terminal");
        assert!(!rename.append_text("\n\u{202e}"));
        assert_eq!(rename.input(), "terminal");
        assert!(rename.append_text("work"));
        assert_eq!(rename.input(), "work");
        assert!(rename.append_text(&"界".repeat(100)));
        assert!(rename.input().len() <= MAX_DOJO_NAME_BYTES);
        assert_eq!(
            rename.command(),
            Some(WindowTopologyCommand::RenameDojo {
                dojo_id,
                name: rename.input().to_owned(),
            })
        );
        while rename.backspace() {}
        assert_eq!(rename.command(), None);
    }

    #[test]
    fn termination_prompt_defaults_to_cancel_and_keeps_exact_target() {
        let target = DojoActionTarget {
            dojo_id: DojoId::new(),
            name: "build".to_owned(),
            pane_count: 4,
            splints: vec![(SplintId::new(), 7), (SplintId::new(), 8)],
        };
        let mut confirmation = TerminateDojoUi::new(target.clone());
        assert_eq!(confirmation.target(), &target);
        assert_eq!(confirmation.decision(), TerminationDecision::Cancel);
        assert_eq!(confirmation.command(), None);
        assert!(confirmation.move_selection());
        assert_eq!(confirmation.decision(), TerminationDecision::Terminate);
        assert_eq!(
            confirmation.command(),
            Some(WindowTopologyCommand::TerminateDojo {
                dojo_id: target.dojo_id,
                splints: target.splints.clone(),
            })
        );
        assert!(confirmation.move_selection());
        assert_eq!(confirmation.command(), None);
    }

    #[test]
    fn right_press_retargets_only_visible_tabs_and_otherwise_dismisses() {
        let dojo_id = DojoId::new();
        assert_eq!(
            tab_menu_right_press(Some(dojo_id)),
            TabMenuRightPress::Retarget(dojo_id)
        );
        assert_eq!(tab_menu_right_press(None), TabMenuRightPress::Dismiss);
    }

    #[test]
    fn tab_menu_skips_disabled_and_dispatches_exact_captured_identity() {
        let other_dojo_ids = vec![DojoId::new(), DojoId::new()];
        let context = TabMenuContext {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            dojo_name: "captured".to_owned(),
            pane_count: 3,
            splints: vec![
                (SplintId::new(), 4),
                (SplintId::new(), 5),
                (SplintId::new(), 6),
            ],
            active: false,
            other_dojo_ids: other_dojo_ids.clone(),
        };
        let mut menu = TabContextMenuUi::new(context.clone());
        assert_eq!(menu.selected_action(), TabMenuActionId::RenameTab);
        assert!(menu.move_selection(-1));
        assert_eq!(menu.selected_action(), TabMenuActionId::TerminateDojo);
        assert_eq!(menu.context(), context);
        assert_eq!(
            tab_menu_dispatch(TabMenuActionId::RenameTab, &context),
            Some(TabMenuDispatch::Rename(DojoActionTarget {
                dojo_id: context.dojo_id,
                name: "captured".to_owned(),
                pane_count: 3,
                splints: context.splints.clone(),
            }))
        );
        assert_eq!(
            tab_menu_dispatch(TabMenuActionId::CloseOtherTabs, &context),
            Some(TabMenuDispatch::Topology(
                WindowTopologyCommand::CloseTabs {
                    retain_dojo_id: context.dojo_id,
                    dojo_ids: other_dojo_ids,
                }
            ))
        );

        assert_eq!(
            tab_menu_dispatch(TabMenuActionId::TerminateDojo, &context),
            Some(TabMenuDispatch::ConfirmTermination(DojoActionTarget {
                dojo_id: context.dojo_id,
                name: "captured".to_owned(),
                pane_count: 3,
                splints: context.splints.clone(),
            }))
        );

        let mut active = TabContextMenuUi::new(TabMenuContext {
            active: true,
            other_dojo_ids: Vec::new(),
            ..context
        });
        assert_eq!(active.selected_action(), TabMenuActionId::RenameTab);
        assert!(!active.action_enabled(TabMenuActionId::ActivateTab));
        assert!(!active.action_enabled(TabMenuActionId::CloseOtherTabs));
        assert!(!active.update_hovered(Some(TabMenuActionId::ActivateTab)));
    }

    #[test]
    fn filtering_is_case_insensitive_stable_category_and_keyword_aware() {
        let mut palette = palette();
        assert_eq!(palette.filtered.len(), BUILT_IN_COMMANDS.len());
        assert!(palette.append_text("SPLIT"));
        assert_eq!(
            palette.filtered,
            vec![
                BuiltInCommandId::SplitHorizontal,
                BuiltInCommandId::SplitVertical
            ]
        );
        assert!(palette.append_text(" horizontal"));
        assert!(palette.filtered.is_empty());
        while palette.backspace() {}
        assert_eq!(palette.filtered.len(), BUILT_IN_COMMANDS.len());
        assert!(palette.append_text("horizontal"));
        assert_eq!(palette.filtered, vec![BuiltInCommandId::SplitHorizontal]);
        while palette.backspace() {}
        assert!(palette.append_text("view"));
        assert_eq!(
            palette.filtered,
            vec![
                BuiltInCommandId::ZoomIn,
                BuiltInCommandId::ZoomOut,
                BuiltInCommandId::ResetZoom
            ]
        );
    }

    #[test]
    fn selection_wraps_skips_disabled_and_preserves_enabled_command() {
        let mut palette = palette();
        assert!(palette.move_selection(-1));
        assert_eq!(
            palette.selected_command(),
            Some(BuiltInCommandId::DenyControlTransfer)
        );
        assert!(palette.append_text("zoom"));
        assert_eq!(palette.selected_command(), Some(BuiltInCommandId::ZoomIn));
        assert!(!palette.select_first());
        assert!(palette.select_last());
        assert_eq!(
            palette.selected_command(),
            Some(BuiltInCommandId::ResetZoom)
        );

        let context = CommandPaletteContext {
            previous_dojo_id: None,
            next_dojo_id: None,
            focus_left: None,
            focus_right: None,
            focus_up: None,
            focus_down: None,
            ..palette.context()
        };
        let mut disabled = CommandPaletteUi::new(context);
        disabled.select_first();
        assert_eq!(
            disabled.selected_command(),
            Some(BuiltInCommandId::RecentSessions)
        );
        disabled.move_selection(1);
        assert_eq!(
            disabled.selected_command(),
            Some(BuiltInCommandId::NewSession)
        );
        assert!(!disabled.command_enabled(BuiltInCommandId::PreviousDojo));
        assert!(!disabled.update_hovered(Some(BuiltInCommandId::FocusLeft)));
    }

    #[test]
    fn query_is_scalar_safe_bounded_and_removes_control_and_bidi() {
        let mut palette = palette();
        assert!(palette.append_text("sp\n\u{202e}lit"));
        assert_eq!(palette.query(), "split");
        while palette.backspace() {}
        assert!(palette.append_text(&"界".repeat(200)));
        assert!(palette.query().len() <= MAX_QUERY_BYTES);
        assert!(palette.query().chars().count() <= MAX_QUERY_SCALARS);
        assert!(palette.backspace());
        assert!(std::str::from_utf8(palette.query().as_bytes()).is_ok());
    }

    #[test]
    fn captured_context_does_not_change_with_query_or_selection() {
        let mut palette = palette();
        let context = palette.context();
        palette.append_text("split");
        palette.move_selection(1);
        assert_eq!(palette.context(), context);
    }

    #[test]
    fn broad_catalog_is_closed_unique_and_category_searchable() {
        let ids = BUILT_IN_COMMANDS
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(BUILT_IN_COMMANDS.len(), 31);
        assert_eq!(ids.len(), 31);
        let mut palette = palette();
        assert!(palette.append_text("control"));
        assert_eq!(palette.filtered().len(), 6);
        while palette.backspace() {}
        assert!(palette.append_text("history"));
        assert_eq!(palette.filtered().len(), 4);
    }

    #[test]
    fn command_dispatch_keeps_exact_captured_identity_and_availability() {
        let context = palette().context();
        for descriptor in BUILT_IN_COMMANDS {
            assert_eq!(
                command_dispatch(descriptor.id, &context).is_some(),
                command_enabled(descriptor.id, &context),
                "availability and dispatch agree for {}",
                descriptor.title
            );
        }
        let controller = CommandPaletteContext {
            controller_active: true,
            ..context.clone()
        };
        assert!(command_dispatch(BuiltInCommandId::ReleaseControl, &controller).is_some());
        assert!(command_dispatch(BuiltInCommandId::RequestControl, &controller).is_none());
        assert_eq!(
            command_dispatch(BuiltInCommandId::AcceptControlTransfer, &context),
            Some(BuiltInCommandDispatch::Control {
                target: context.splint_id,
                action: CommandControlAction::Accept(42),
            })
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::RevokeAllAccess, &context),
            Some(BuiltInCommandDispatch::RevokeAccess {
                target: context.splint_id,
                grant_ids: vec![7, 9],
            })
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::NewDojo, &context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::NewDojo {
                    lair_id: context.lair_id
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::TerminateCurrentDojo, &context),
            Some(BuiltInCommandDispatch::ConfirmTermination(
                DojoActionTarget {
                    dojo_id: context.dojo_id,
                    name: context.dojo_name.clone(),
                    pane_count: context.pane_count,
                    splints: context.dojo_splints.clone(),
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::SplitHorizontal, &context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::Split {
                    dojo_id: context.dojo_id,
                    target: context.splint_id,
                    axis: Axis::Horizontal,
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::PreviousDojo, &context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::ActivateTab {
                    dojo_id: context.previous_dojo_id.unwrap(),
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::FocusRight, &context),
            Some(BuiltInCommandDispatch::Focus(context.focus_right.unwrap()))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::CloseFocusedPane, &context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::Close {
                    dojo_id: context.dojo_id,
                    target: context.splint_id,
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::ZoomOut, &context),
            Some(BuiltInCommandDispatch::Zoom(CommandZoomAction::Decrease))
        );
        assert_eq!(
            command_dispatch(
                BuiltInCommandId::NextDojo,
                &CommandPaletteContext {
                    next_dojo_id: None,
                    ..context.clone()
                }
            ),
            None
        );
    }
}
