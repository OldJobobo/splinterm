//! Platform-independent built-in command-palette state.

use splinterm_core::{Axis, DojoId, LairId, SplintId};

use super::WindowTopologyCommand;

const MAX_QUERY_BYTES: usize = 256;
const MAX_QUERY_SCALARS: usize = 128;
pub(crate) const COMMAND_PALETTE_PAGE_ITEMS: usize = 7;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TabMenuActionId {
    ActivateTab,
    NewDojo,
    SplitHorizontal,
    SplitVertical,
    CloseTab,
    CloseOtherTabs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabMenuContext {
    pub(crate) lair_id: LairId,
    pub(crate) dojo_id: DojoId,
    pub(crate) focused_splint_id: Option<SplintId>,
    pub(crate) active: bool,
    pub(crate) other_dojo_ids: Vec<DojoId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TabMenuActionDescriptor {
    pub(crate) id: TabMenuActionId,
    pub(crate) title: &'static str,
}

pub(crate) const TAB_MENU_ACTIONS: [TabMenuActionDescriptor; 6] = [
    TabMenuActionDescriptor {
        id: TabMenuActionId::ActivateTab,
        title: "Activate Tab",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::NewDojo,
        title: "New Dojo",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::SplitHorizontal,
        title: "Split Horizontally",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::SplitVertical,
        title: "Split Vertically",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::CloseTab,
        title: "Close Tab",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::CloseOtherTabs,
        title: "Close Other Tabs",
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
        TabMenuActionId::SplitHorizontal | TabMenuActionId::SplitVertical => {
            context.focused_splint_id.is_some()
        }
        TabMenuActionId::CloseOtherTabs => !context.other_dojo_ids.is_empty(),
        TabMenuActionId::NewDojo | TabMenuActionId::CloseTab => true,
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
}

pub(crate) fn tab_menu_dispatch(
    id: TabMenuActionId,
    context: &TabMenuContext,
) -> Option<TabMenuDispatch> {
    if !tab_menu_action_enabled(id, context) {
        return None;
    }
    let command = match id {
        TabMenuActionId::ActivateTab => WindowTopologyCommand::ActivateTab {
            dojo_id: context.dojo_id,
        },
        TabMenuActionId::NewDojo => WindowTopologyCommand::NewDojo {
            lair_id: context.lair_id,
        },
        TabMenuActionId::SplitHorizontal => WindowTopologyCommand::Split {
            dojo_id: context.dojo_id,
            target: context.focused_splint_id?,
            axis: Axis::Horizontal,
        },
        TabMenuActionId::SplitVertical => WindowTopologyCommand::Split {
            dojo_id: context.dojo_id,
            target: context.focused_splint_id?,
            axis: Axis::Vertical,
        },
        TabMenuActionId::CloseTab => WindowTopologyCommand::CloseTab {
            dojo_id: context.dojo_id,
        },
        TabMenuActionId::CloseOtherTabs => WindowTopologyCommand::CloseTabs {
            retain_dojo_id: context.dojo_id,
            dojo_ids: context.other_dojo_ids.clone(),
        },
    };
    Some(TabMenuDispatch::Topology(command))
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
    NewDojo,
    PreviousDojo,
    NextDojo,
    CloseCurrentTab,
    SplitHorizontal,
    SplitVertical,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CloseFocusedPane,
    ZoomIn,
    ZoomOut,
    ResetZoom,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CommandCategory {
    Sessions,
    Tabs,
    Panes,
    View,
}

impl CommandCategory {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Sessions => "SESSION",
            Self::Tabs => "TAB",
            Self::Panes => "PANE",
            Self::View => "VIEW",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_field_names,
    reason = "the explicit identity suffixes keep domain identifiers and captured destinations unambiguous"
)]
pub(crate) struct CommandPaletteContext {
    pub(crate) lair_id: LairId,
    pub(crate) dojo_id: DojoId,
    pub(crate) splint_id: SplintId,
    pub(crate) previous_dojo_id: Option<DojoId>,
    pub(crate) next_dojo_id: Option<DojoId>,
    pub(crate) focus_left: Option<SplintId>,
    pub(crate) focus_right: Option<SplintId>,
    pub(crate) focus_up: Option<SplintId>,
    pub(crate) focus_down: Option<SplintId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandZoomAction {
    Increase,
    Decrease,
    Reset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BuiltInCommandDispatch {
    Topology(WindowTopologyCommand),
    Focus(SplintId),
    Zoom(CommandZoomAction),
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

pub(crate) const BUILT_IN_COMMANDS: [BuiltInCommandDescriptor; 15] = [
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::RecentSessions,
        category: CommandCategory::Sessions,
        title: "Open recent sessions",
        keywords: &["open", "recent", "session", "lair", "dojo", "reopen"],
        shortcut: "Ctrl+Shift+S",
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

pub(crate) const fn command_enabled(id: BuiltInCommandId, context: CommandPaletteContext) -> bool {
    match id {
        BuiltInCommandId::PreviousDojo => context.previous_dojo_id.is_some(),
        BuiltInCommandId::NextDojo => context.next_dojo_id.is_some(),
        BuiltInCommandId::FocusLeft => context.focus_left.is_some(),
        BuiltInCommandId::FocusRight => context.focus_right.is_some(),
        BuiltInCommandId::FocusUp => context.focus_up.is_some(),
        BuiltInCommandId::FocusDown => context.focus_down.is_some(),
        BuiltInCommandId::RecentSessions
        | BuiltInCommandId::NewDojo
        | BuiltInCommandId::CloseCurrentTab
        | BuiltInCommandId::SplitHorizontal
        | BuiltInCommandId::SplitVertical
        | BuiltInCommandId::CloseFocusedPane
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

    pub(crate) const fn context(&self) -> CommandPaletteContext {
        self.context
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

    pub(crate) const fn command_enabled(&self, command: BuiltInCommandId) -> bool {
        command_enabled(command, self.context)
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

pub(crate) const fn command_dispatch(
    id: BuiltInCommandId,
    context: CommandPaletteContext,
) -> Option<BuiltInCommandDispatch> {
    if !command_enabled(id, context) {
        return None;
    }
    let dispatch = match id {
        BuiltInCommandId::RecentSessions => BuiltInCommandDispatch::RecentSessions,
        BuiltInCommandId::NewDojo => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::NewDojo {
                lair_id: context.lair_id,
            })
        }
        BuiltInCommandId::PreviousDojo => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::ActivateTab {
                dojo_id: match context.previous_dojo_id {
                    Some(dojo_id) => dojo_id,
                    None => return None,
                },
            })
        }
        BuiltInCommandId::NextDojo => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::ActivateTab {
                dojo_id: match context.next_dojo_id {
                    Some(dojo_id) => dojo_id,
                    None => return None,
                },
            })
        }
        BuiltInCommandId::CloseCurrentTab => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::CloseTab {
                dojo_id: context.dojo_id,
            })
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
        BuiltInCommandId::FocusLeft => BuiltInCommandDispatch::Focus(match context.focus_left {
            Some(splint_id) => splint_id,
            None => return None,
        }),
        BuiltInCommandId::FocusRight => BuiltInCommandDispatch::Focus(match context.focus_right {
            Some(splint_id) => splint_id,
            None => return None,
        }),
        BuiltInCommandId::FocusUp => BuiltInCommandDispatch::Focus(match context.focus_up {
            Some(splint_id) => splint_id,
            None => return None,
        }),
        BuiltInCommandId::FocusDown => BuiltInCommandDispatch::Focus(match context.focus_down {
            Some(splint_id) => splint_id,
            None => return None,
        }),
        BuiltInCommandId::CloseFocusedPane => {
            BuiltInCommandDispatch::Topology(WindowTopologyCommand::Close {
                dojo_id: context.dojo_id,
                target: context.splint_id,
            })
        }
        BuiltInCommandId::ZoomIn => BuiltInCommandDispatch::Zoom(CommandZoomAction::Increase),
        BuiltInCommandId::ZoomOut => BuiltInCommandDispatch::Zoom(CommandZoomAction::Decrease),
        BuiltInCommandId::ResetZoom => BuiltInCommandDispatch::Zoom(CommandZoomAction::Reset),
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
        CommandPaletteUi::new(CommandPaletteContext {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            splint_id: SplintId::new(),
            previous_dojo_id: Some(DojoId::new()),
            next_dojo_id: Some(DojoId::new()),
            focus_left: Some(SplintId::new()),
            focus_right: Some(SplintId::new()),
            focus_up: Some(SplintId::new()),
            focus_down: Some(SplintId::new()),
        })
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
        let focused_splint_id = SplintId::new();
        let other_dojo_ids = vec![DojoId::new(), DojoId::new()];
        let context = TabMenuContext {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            focused_splint_id: Some(focused_splint_id),
            active: false,
            other_dojo_ids: other_dojo_ids.clone(),
        };
        let mut menu = TabContextMenuUi::new(context.clone());
        assert_eq!(menu.selected_action(), TabMenuActionId::ActivateTab);
        assert!(menu.move_selection(-1));
        assert_eq!(menu.selected_action(), TabMenuActionId::CloseOtherTabs);
        assert_eq!(menu.context(), context);
        assert_eq!(
            tab_menu_dispatch(TabMenuActionId::SplitVertical, &context),
            Some(TabMenuDispatch::Topology(WindowTopologyCommand::Split {
                dojo_id: context.dojo_id,
                target: focused_splint_id,
                axis: Axis::Vertical,
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

        let mut active = TabContextMenuUi::new(TabMenuContext {
            active: true,
            focused_splint_id: None,
            other_dojo_ids: Vec::new(),
            ..context
        });
        assert_eq!(active.selected_action(), TabMenuActionId::NewDojo);
        assert!(!active.action_enabled(TabMenuActionId::ActivateTab));
        assert!(!active.action_enabled(TabMenuActionId::SplitHorizontal));
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
            Some(BuiltInCommandId::ResetZoom)
        );
        assert!(palette.append_text("zoom"));
        assert_eq!(
            palette.selected_command(),
            Some(BuiltInCommandId::ResetZoom)
        );
        assert!(palette.select_first());
        assert_eq!(palette.selected_command(), Some(BuiltInCommandId::ZoomIn));
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
        assert_eq!(disabled.selected_command(), Some(BuiltInCommandId::NewDojo));
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
    fn command_dispatch_keeps_exact_captured_identity_and_availability() {
        let context = palette().context();
        for descriptor in BUILT_IN_COMMANDS {
            assert!(
                command_dispatch(descriptor.id, context).is_some(),
                "fully available context dispatches {}",
                descriptor.title
            );
        }
        assert_eq!(
            command_dispatch(BuiltInCommandId::NewDojo, context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::NewDojo {
                    lair_id: context.lair_id
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::SplitHorizontal, context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::Split {
                    dojo_id: context.dojo_id,
                    target: context.splint_id,
                    axis: Axis::Horizontal,
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::PreviousDojo, context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::ActivateTab {
                    dojo_id: context.previous_dojo_id.unwrap(),
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::FocusRight, context),
            Some(BuiltInCommandDispatch::Focus(context.focus_right.unwrap()))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::CloseFocusedPane, context),
            Some(BuiltInCommandDispatch::Topology(
                WindowTopologyCommand::Close {
                    dojo_id: context.dojo_id,
                    target: context.splint_id,
                }
            ))
        );
        assert_eq!(
            command_dispatch(BuiltInCommandId::ZoomOut, context),
            Some(BuiltInCommandDispatch::Zoom(CommandZoomAction::Decrease))
        );
        assert_eq!(
            command_dispatch(
                BuiltInCommandId::NextDojo,
                CommandPaletteContext {
                    next_dojo_id: None,
                    ..context
                }
            ),
            None
        );
    }
}
