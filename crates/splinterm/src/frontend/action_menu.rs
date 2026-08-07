//! Platform-independent built-in command-palette state.

use splinterm_core::{Axis, DojoId, LairId, SplintId};

use super::WindowTopologyCommand;

const MAX_QUERY_BYTES: usize = 256;
const MAX_QUERY_SCALARS: usize = 128;
pub(crate) const COMMAND_PALETTE_PAGE_ITEMS: usize = 7;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TabMenuActionId {
    NewDojo,
    CloseTab,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TabMenuContext {
    pub(crate) lair_id: LairId,
    pub(crate) dojo_id: DojoId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TabMenuActionDescriptor {
    pub(crate) id: TabMenuActionId,
    pub(crate) title: &'static str,
}

pub(crate) const TAB_MENU_ACTIONS: [TabMenuActionDescriptor; 2] = [
    TabMenuActionDescriptor {
        id: TabMenuActionId::NewDojo,
        title: "New Dojo",
    },
    TabMenuActionDescriptor {
        id: TabMenuActionId::CloseTab,
        title: "Close Tab",
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TabContextMenuUi {
    context: TabMenuContext,
    selected: usize,
    hovered: Option<TabMenuActionId>,
}

impl TabContextMenuUi {
    pub(crate) const fn new(context: TabMenuContext) -> Self {
        Self {
            context,
            selected: 0,
            hovered: None,
        }
    }

    pub(crate) const fn context(&self) -> TabMenuContext {
        self.context
    }

    pub(crate) const fn selected_action(&self) -> TabMenuActionId {
        TAB_MENU_ACTIONS[self.selected].id
    }

    pub(crate) const fn hovered(&self) -> Option<TabMenuActionId> {
        self.hovered
    }

    pub(crate) fn update_hovered(&mut self, hovered: Option<TabMenuActionId>) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        true
    }

    pub(crate) fn move_selection(&mut self, delta: isize) -> bool {
        let previous = self.selected;
        let count = TAB_MENU_ACTIONS.len();
        let magnitude = delta.unsigned_abs() % count;
        self.selected = if delta.is_negative() {
            self.selected
                .saturating_add(count)
                .saturating_sub(magnitude)
                % count
        } else {
            self.selected.saturating_add(magnitude) % count
        };
        self.selected != previous
    }
}

pub(crate) const fn tab_menu_topology_command(
    id: TabMenuActionId,
    context: TabMenuContext,
) -> WindowTopologyCommand {
    match id {
        TabMenuActionId::NewDojo => WindowTopologyCommand::NewDojo {
            lair_id: context.lair_id,
        },
        TabMenuActionId::CloseTab => WindowTopologyCommand::CloseTab {
            dojo_id: context.dojo_id,
        },
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
    NewDojo,
    SplitHorizontal,
    SplitVertical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_field_names,
    reason = "the explicit identity suffixes keep three domain identifiers unambiguous"
)]
pub(crate) struct CommandPaletteContext {
    pub(crate) lair_id: LairId,
    pub(crate) dojo_id: DojoId,
    pub(crate) splint_id: SplintId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BuiltInCommandDescriptor {
    pub(crate) id: BuiltInCommandId,
    pub(crate) title: &'static str,
    pub(crate) keywords: &'static [&'static str],
    pub(crate) shortcut: &'static str,
}

pub(crate) const BUILT_IN_COMMANDS: [BuiltInCommandDescriptor; 3] = [
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::NewDojo,
        title: "New Dojo",
        keywords: &["new", "dojo", "tab"],
        shortcut: "Ctrl+Shift+D",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::SplitHorizontal,
        title: "Split pane horizontally",
        keywords: &["split", "pane", "horizontal", "down"],
        shortcut: "Ctrl+Shift+Enter",
    },
    BuiltInCommandDescriptor {
        id: BuiltInCommandId::SplitVertical,
        title: "Split pane vertically",
        keywords: &["split", "pane", "vertical", "right"],
        shortcut: "Ctrl+Shift+\\",
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
        || descriptor
            .keywords
            .iter()
            .any(|keyword| keyword.contains(&query))
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

    pub(crate) const fn hovered(&self) -> Option<BuiltInCommandId> {
        self.hovered
    }

    pub(crate) fn update_hovered(&mut self, hovered: Option<BuiltInCommandId>) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        true
    }

    pub(crate) fn move_selection(&mut self, delta: isize) -> bool {
        let count = self.filtered.len();
        if count == 0 {
            return false;
        }
        let previous = self.selected;
        let magnitude = delta.unsigned_abs() % count;
        self.selected = if delta.is_negative() {
            self.selected
                .saturating_add(count)
                .saturating_sub(magnitude)
                % count
        } else {
            self.selected.saturating_add(magnitude) % count
        };
        self.ensure_selected_visible(COMMAND_PALETTE_PAGE_ITEMS);
        self.selected != previous
    }

    pub(crate) fn select_first(&mut self) -> bool {
        let changed = self.selected != 0 || self.visible_start != 0;
        self.selected = 0;
        self.visible_start = 0;
        changed
    }

    pub(crate) fn select_last(&mut self) -> bool {
        let Some(last) = self.filtered.len().checked_sub(1) else {
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
            .and_then(|selected| {
                self.filtered
                    .iter()
                    .position(|candidate| *candidate == selected)
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

pub(crate) const fn command_topology_command(
    id: BuiltInCommandId,
    context: CommandPaletteContext,
) -> WindowTopologyCommand {
    match id {
        BuiltInCommandId::NewDojo => WindowTopologyCommand::NewDojo {
            lair_id: context.lair_id,
        },
        BuiltInCommandId::SplitHorizontal => WindowTopologyCommand::Split {
            dojo_id: context.dojo_id,
            target: context.splint_id,
            axis: Axis::Horizontal,
        },
        BuiltInCommandId::SplitVertical => WindowTopologyCommand::Split {
            dojo_id: context.dojo_id,
            target: context.splint_id,
            axis: Axis::Vertical,
        },
    }
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
    fn tab_menu_wraps_and_dispatches_captured_identity() {
        let context = TabMenuContext {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
        };
        let mut menu = TabContextMenuUi::new(context);
        assert_eq!(menu.selected_action(), TabMenuActionId::NewDojo);
        assert!(menu.move_selection(-1));
        assert_eq!(menu.selected_action(), TabMenuActionId::CloseTab);
        assert_eq!(menu.context(), context);
        assert_eq!(
            tab_menu_topology_command(TabMenuActionId::NewDojo, context),
            WindowTopologyCommand::NewDojo {
                lair_id: context.lair_id
            }
        );
        assert_eq!(
            tab_menu_topology_command(TabMenuActionId::CloseTab, context),
            WindowTopologyCommand::CloseTab {
                dojo_id: context.dojo_id
            }
        );
    }

    #[test]
    fn filtering_is_case_insensitive_stable_and_keyword_aware() {
        let mut palette = palette();
        assert_eq!(palette.filtered.len(), 3);
        assert!(palette.append_text("SPLIT"));
        assert_eq!(
            palette.filtered,
            vec![
                BuiltInCommandId::SplitHorizontal,
                BuiltInCommandId::SplitVertical
            ]
        );
        assert!(palette.append_text(" right"));
        assert!(palette.filtered.is_empty());
        while palette.backspace() {}
        assert_eq!(palette.filtered.len(), 3);
        assert!(palette.append_text("right"));
        assert_eq!(palette.filtered, vec![BuiltInCommandId::SplitVertical]);
    }

    #[test]
    fn selection_wraps_and_preserves_command_when_filter_keeps_it() {
        let mut palette = palette();
        assert!(palette.move_selection(-1));
        assert_eq!(
            palette.selected_command(),
            Some(BuiltInCommandId::SplitVertical)
        );
        assert!(palette.append_text("split"));
        assert_eq!(
            palette.selected_command(),
            Some(BuiltInCommandId::SplitVertical)
        );
        assert!(palette.select_first());
        assert_eq!(
            palette.selected_command(),
            Some(BuiltInCommandId::SplitHorizontal)
        );
        assert!(palette.select_last());
        assert_eq!(
            palette.selected_command(),
            Some(BuiltInCommandId::SplitVertical)
        );
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
    fn command_dispatch_keeps_exact_captured_identity() {
        let context = palette().context();
        assert_eq!(
            command_topology_command(BuiltInCommandId::NewDojo, context),
            WindowTopologyCommand::NewDojo {
                lair_id: context.lair_id
            }
        );
        assert_eq!(
            command_topology_command(BuiltInCommandId::SplitHorizontal, context),
            WindowTopologyCommand::Split {
                dojo_id: context.dojo_id,
                target: context.splint_id,
                axis: Axis::Horizontal,
            }
        );
        assert_eq!(
            command_topology_command(BuiltInCommandId::SplitVertical, context),
            WindowTopologyCommand::Split {
                dojo_id: context.dojo_id,
                target: context.splint_id,
                axis: Axis::Vertical,
            }
        );
    }
}
