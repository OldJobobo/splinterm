//! Pure input encoding and shortcut classification.

mod keyboard;
mod pointer;
mod shortcuts;

pub(in crate::wayland) use keyboard::key_input;
pub(in crate::wayland) use pointer::{
    HistoryNavigation, MouseAction, PickerImeReconcile, PressOwner, WheelAccumulator, WheelOutcome,
    application_motion, classify_press, clipboard_read_is_current, history_navigation,
    history_overlay_status, history_return_to_live_hit, mouse_report, picker_ime_reconcile,
    picker_release_activation, pointer_axis_focus_target, reconciled_focus_report,
    take_press_owner,
};
pub(in crate::wayland) use shortcuts::{
    CommandPaletteShortcutAction, FontZoomAction, PaneFocusAction, PaneTopologyAction,
    SessionPickerShortcutAction, TabShortcutAction, command_palette_shortcut_action,
    font_zoom_action, pane_focus_action, pane_topology_action, session_picker_shortcut_action,
    tab_action_dispatch_allowed, tab_shortcut_action,
};
