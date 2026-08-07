//! Platform-independent contracts shared by application orchestration and presentation adapters.

mod action_menu;
mod message;
mod options;
mod picker;
mod topology;

pub(crate) use action_menu::{
    BuiltInCommandId, COMMAND_PALETTE_PAGE_ITEMS, CommandPaletteContext, CommandPaletteUi,
    TAB_MENU_ACTIONS, TabContextMenuUi, TabMenuActionId, TabMenuContext, TabMenuRightPress,
    command_descriptor, command_topology_command, tab_menu_descriptor, tab_menu_right_press,
    tab_menu_topology_command,
};
pub use message::{
    AuthorityStatus, PerfTraceCorrelation, ThemeUpdate, WindowCommand, WindowUpdate,
};
pub use options::{TrustedConsentUi, WindowOptions, WindowPaneOptions};
pub(crate) use picker::PickerHitTarget;
pub use picker::{SessionPickerDecision, SessionPickerItem, SessionPickerUi};
pub use topology::{WindowDojoIdentity, WindowTopologyCommand, WindowTopologyUpdate};
