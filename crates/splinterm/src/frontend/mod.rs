//! Platform-independent contracts shared by application orchestration and presentation adapters.

mod action_menu;
mod message;
mod options;
mod picker;
mod topology;

pub(crate) use action_menu::{
    BuiltInCommandDispatch, BuiltInCommandId, COMMAND_PALETTE_PAGE_ITEMS, CommandControlAction,
    CommandHistoryAction, CommandPaletteContext, CommandPaletteUi, CommandZoomAction, DojoPromptUi,
    TAB_MENU_ACTIONS, TabContextMenuUi, TabMenuActionId, TabMenuContext, TabMenuDispatch,
    TabMenuRightPress, TerminationDecision, command_descriptor, command_dispatch,
    tab_menu_descriptor, tab_menu_dispatch, tab_menu_right_press,
};
pub use message::{
    AuthorityStatus, PerfTraceCorrelation, ThemeUpdate, WindowCommand, WindowUpdate,
};
pub use options::{TrustedConsentUi, WindowOptions, WindowPaneOptions};
pub(crate) use picker::PickerHitTarget;
pub use picker::{SessionPickerDecision, SessionPickerItem, SessionPickerUi};
pub use topology::{
    LairDirection, LairPromptKind, LairPromptTarget, SelectorKind, WindowDojoIdentity,
    WindowTopologyCommand, WindowTopologyUpdate,
};
