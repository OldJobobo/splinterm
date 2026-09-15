//! Platform-independent contracts shared by application orchestration and presentation adapters.

mod action_menu;
mod binding_help;
mod lair_explorer;
mod message;
mod navigation_accessibility;
mod options;
mod picker;
mod text_edit;
mod topology;

pub(crate) use action_menu::{
    BuiltInCommandDispatch, BuiltInCommandId, COMMAND_PALETTE_PAGE_ITEMS, CommandControlAction,
    CommandHistoryAction, CommandPaletteContext, CommandPaletteUi, CommandTabMoveAvailability,
    CommandZoomAction, DojoPromptUi, TabContextMenuUi, TabMenuActionDescriptor, TabMenuActionId,
    TabMenuContext, TabMenuDispatch, TabMenuRightPress, TerminationDecision,
    close_other_tabs_command, command_descriptor, command_dispatch, tab_menu_right_press,
};
#[cfg(test)]
pub(crate) use action_menu::{TAB_MENU_ACTIONS, tab_menu_descriptor};
pub(crate) use binding_help::{BINDING_HELP_PAGE_ITEMS, BindingHelpUi};
#[cfg(test)]
pub(crate) use lair_explorer::LairExplorerRowKind;
pub(crate) use lair_explorer::{
    LairExplorerDecision, LairExplorerFilter, LairExplorerRow, LairExplorerUi,
};
pub use message::{
    AuthorityStatus, FontUpdate, PerfTraceCorrelation, ThemeUpdate, WindowCommand, WindowUpdate,
};
pub(crate) use navigation_accessibility::{
    NavigationAccessAction, NavigationAccessContext, NavigationAccessibility,
};
pub use options::{TerminalGridLimits, TrustedConsentUi, WindowOptions, WindowPaneOptions};
pub(crate) use picker::PickerHitTarget;
pub use picker::{SessionPickerDecision, SessionPickerItem, SessionPickerUi};
pub(crate) use text_edit::BoundedTextEditor;
pub use topology::{
    ExplorerContextTarget, LairDirection, LairExplorerActivationTarget, LairPromptKind,
    LairPromptTarget, SelectorKind, SessionPickerCatalog, SessionPickerCreationTarget,
    SessionPickerTarget, WindowDojoIdentity, WindowTopologyCommand, WindowTopologyUpdate,
};
