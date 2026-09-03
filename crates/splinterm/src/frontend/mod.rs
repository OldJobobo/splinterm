//! Platform-independent contracts shared by application orchestration and presentation adapters.

mod action_menu;
mod binding_help;
mod message;
mod options;
mod picker;
mod text_edit;
mod topology;

pub(crate) use action_menu::{
    BuiltInCommandDispatch, BuiltInCommandId, COMMAND_PALETTE_PAGE_ITEMS, CommandControlAction,
    CommandHistoryAction, CommandPaletteContext, CommandPaletteUi, CommandTabMoveAvailability,
    CommandZoomAction, DojoPromptUi, TAB_MENU_ACTIONS, TabContextMenuUi, TabMenuActionId,
    TabMenuContext, TabMenuDispatch, TabMenuRightPress, TerminationDecision,
    close_other_tabs_command, command_descriptor, command_dispatch, tab_menu_descriptor,
    tab_menu_dispatch, tab_menu_right_press,
};
pub(crate) use binding_help::{BINDING_HELP_PAGE_ITEMS, BindingHelpUi};
pub use message::{
    AuthorityStatus, FontUpdate, PerfTraceCorrelation, ThemeUpdate, WindowCommand, WindowUpdate,
};
pub use options::{TerminalGridLimits, TrustedConsentUi, WindowOptions, WindowPaneOptions};
pub(crate) use picker::PickerHitTarget;
pub use picker::{SessionPickerDecision, SessionPickerItem, SessionPickerUi};
pub(crate) use text_edit::BoundedTextEditor;
pub use topology::{
    LairDirection, LairPromptKind, LairPromptTarget, SelectorKind, WindowDojoIdentity,
    WindowTopologyCommand, WindowTopologyUpdate,
};
