//! Pure shortcut classification before adapter-side effects.

use crate::pane::FocusDirection;
use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum PaneTopologyAction {
    Split(splinterm_core::Axis),
    Close,
    AdjustRatio(i16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum CommandPaletteShortcutAction {
    Open,
    Consume,
}

pub(in crate::wayland) fn command_palette_shortcut_action(
    keysym: Keysym,
    modifiers: Modifiers,
    repeat: bool,
    managed_tabs: bool,
    blocked: bool,
) -> Option<CommandPaletteShortcutAction> {
    if !managed_tabs
        || !modifiers.ctrl
        || !modifiers.shift
        || modifiers.alt
        || modifiers.logo
        || !matches!(keysym, Keysym::p | Keysym::P)
    {
        return None;
    }
    Some(if repeat || blocked {
        CommandPaletteShortcutAction::Consume
    } else {
        CommandPaletteShortcutAction::Open
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum SessionPickerShortcutAction {
    Request,
    Consume,
}

pub(in crate::wayland) fn session_picker_shortcut_action(
    keysym: Keysym,
    modifiers: Modifiers,
    repeat: bool,
    request_pending: bool,
) -> Option<SessionPickerShortcutAction> {
    if !modifiers.ctrl
        || !modifiers.shift
        || modifiers.alt
        || modifiers.logo
        || !matches!(keysym, Keysym::s | Keysym::S)
    {
        return None;
    }
    Some(if repeat || request_pending {
        SessionPickerShortcutAction::Consume
    } else {
        SessionPickerShortcutAction::Request
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum TabShortcutAction {
    Next,
    Previous,
    NewDojo,
    Close,
    Consume,
}

pub(in crate::wayland) fn tab_shortcut_action(
    keysym: Keysym,
    modifiers: Modifiers,
    repeat: bool,
    managed_tabs: bool,
) -> Option<TabShortcutAction> {
    if !managed_tabs || !modifiers.ctrl || modifiers.alt || modifiers.logo {
        return None;
    }
    let action = match keysym {
        Keysym::Tab if !modifiers.shift => TabShortcutAction::Next,
        Keysym::Tab | Keysym::ISO_Left_Tab if modifiers.shift => TabShortcutAction::Previous,
        Keysym::d | Keysym::D if modifiers.shift => TabShortcutAction::NewDojo,
        Keysym::q | Keysym::Q if modifiers.shift => TabShortcutAction::Close,
        _ => return None,
    };
    Some(if repeat {
        TabShortcutAction::Consume
    } else {
        action
    })
}

pub(in crate::wayland) const fn tab_action_dispatch_allowed(blocking_states: [bool; 5]) -> bool {
    !(blocking_states[0]
        || blocking_states[1]
        || blocking_states[2]
        || blocking_states[3]
        || blocking_states[4])
}

pub(in crate::wayland) fn pane_topology_action(
    keysym: Keysym,
    modifiers: Modifiers,
) -> Option<PaneTopologyAction> {
    if !modifiers.ctrl || !modifiers.shift || modifiers.alt || modifiers.logo {
        return None;
    }
    match keysym {
        Keysym::Return | Keysym::KP_Enter => {
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Horizontal))
        }
        Keysym::backslash | Keysym::bar => {
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Vertical))
        }
        Keysym::w | Keysym::W => Some(PaneTopologyAction::Close),
        Keysym::bracketleft | Keysym::braceleft => Some(PaneTopologyAction::AdjustRatio(-50)),
        Keysym::bracketright | Keysym::braceright => Some(PaneTopologyAction::AdjustRatio(50)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum PaneFocusAction {
    Direction(FocusDirection),
}

pub(in crate::wayland) fn pane_focus_action(
    keysym: Keysym,
    modifiers: Modifiers,
) -> Option<PaneFocusAction> {
    if !modifiers.ctrl || !modifiers.shift || modifiers.alt || modifiers.logo {
        return None;
    }
    match keysym {
        Keysym::Left => Some(PaneFocusAction::Direction(FocusDirection::Left)),
        Keysym::Right => Some(PaneFocusAction::Direction(FocusDirection::Right)),
        Keysym::Up => Some(PaneFocusAction::Direction(FocusDirection::Up)),
        Keysym::Down => Some(PaneFocusAction::Direction(FocusDirection::Down)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum FontZoomAction {
    Increase,
    Decrease,
    Reset,
}

pub(in crate::wayland) fn font_zoom_action(
    keysym: Keysym,
    modifiers: Modifiers,
) -> Option<FontZoomAction> {
    if !modifiers.ctrl || modifiers.alt || modifiers.logo {
        return None;
    }
    match keysym {
        Keysym::plus | Keysym::equal | Keysym::KP_Add => Some(FontZoomAction::Increase),
        Keysym::minus | Keysym::KP_Subtract => Some(FontZoomAction::Decrease),
        Keysym::_0 | Keysym::KP_0 => Some(FontZoomAction::Reset),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_palette_shortcut_is_exact_managed_and_blocked_safely() {
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            command_palette_shortcut_action(Keysym::p, modifiers, false, true, false),
            Some(CommandPaletteShortcutAction::Open)
        );
        assert_eq!(
            command_palette_shortcut_action(Keysym::P, modifiers, true, true, false),
            Some(CommandPaletteShortcutAction::Consume)
        );
        assert_eq!(
            command_palette_shortcut_action(Keysym::p, modifiers, false, true, true),
            Some(CommandPaletteShortcutAction::Consume)
        );
        assert_eq!(
            command_palette_shortcut_action(Keysym::p, modifiers, false, false, false),
            None
        );
        assert_eq!(
            command_palette_shortcut_action(Keysym::p, Modifiers::default(), false, true, false),
            None
        );
    }

    #[test]
    fn session_picker_shortcut_is_exact_and_application_owned() {
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            session_picker_shortcut_action(Keysym::s, modifiers, false, false),
            Some(SessionPickerShortcutAction::Request)
        );
        assert_eq!(
            session_picker_shortcut_action(Keysym::S, modifiers, true, false),
            Some(SessionPickerShortcutAction::Consume)
        );
        assert_eq!(
            session_picker_shortcut_action(Keysym::s, modifiers, false, true),
            Some(SessionPickerShortcutAction::Consume)
        );
        assert_eq!(
            session_picker_shortcut_action(Keysym::s, Modifiers::default(), false, false),
            None
        );
        assert_eq!(
            session_picker_shortcut_action(
                Keysym::s,
                Modifiers {
                    alt: true,
                    ..modifiers
                },
                false,
                false,
            ),
            None
        );
        assert_eq!(
            session_picker_shortcut_action(
                Keysym::s,
                Modifiers {
                    logo: true,
                    ..modifiers
                },
                false,
                false,
            ),
            None
        );
    }

    #[test]
    fn tab_shortcuts_are_exact_managed_and_repeat_consumed() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let ctrl_shift = Modifiers {
            shift: true,
            ..ctrl
        };
        assert_eq!(
            tab_shortcut_action(Keysym::Tab, ctrl, false, true),
            Some(TabShortcutAction::Next)
        );
        assert_eq!(
            tab_shortcut_action(Keysym::ISO_Left_Tab, ctrl_shift, false, true),
            Some(TabShortcutAction::Previous)
        );
        assert_eq!(
            tab_shortcut_action(Keysym::d, ctrl_shift, false, true),
            Some(TabShortcutAction::NewDojo)
        );
        assert_eq!(
            tab_shortcut_action(Keysym::q, ctrl_shift, false, true),
            Some(TabShortcutAction::Close)
        );
        assert_eq!(
            tab_shortcut_action(Keysym::Tab, ctrl, true, true),
            Some(TabShortcutAction::Consume)
        );
        assert_eq!(tab_shortcut_action(Keysym::Tab, ctrl, false, false), None);
        assert_eq!(
            tab_shortcut_action(Keysym::Tab, Modifiers::default(), false, true),
            None
        );
        assert!(tab_action_dispatch_allowed([false; 5]));
        for blocked in 0..5 {
            let mut states = [false; 5];
            states[blocked] = true;
            assert!(!tab_action_dispatch_allowed(states));
        }
    }

    #[test]
    fn pane_focus_bindings_are_explicit_and_do_not_capture_plain_arrows() {
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            pane_focus_action(Keysym::Left, modifiers),
            Some(PaneFocusAction::Direction(FocusDirection::Left))
        );
        assert_eq!(pane_focus_action(Keysym::Tab, modifiers), None);
        assert_eq!(pane_focus_action(Keysym::Left, Modifiers::default()), None);
        assert_eq!(
            pane_topology_action(Keysym::Return, modifiers),
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Horizontal))
        );
        assert_eq!(
            pane_topology_action(Keysym::bar, modifiers),
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Vertical))
        );
        assert_eq!(
            pane_topology_action(Keysym::W, modifiers),
            Some(PaneTopologyAction::Close)
        );
        assert_eq!(
            pane_topology_action(Keysym::braceleft, modifiers),
            Some(PaneTopologyAction::AdjustRatio(-50))
        );
        assert_eq!(
            pane_topology_action(Keysym::braceright, modifiers),
            Some(PaneTopologyAction::AdjustRatio(50))
        );
        assert_eq!(pane_topology_action(Keysym::w, Modifiers::default()), None);
    }

    #[test]
    fn foot_font_zoom_bindings_require_control_and_cover_reset() {
        let control = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            font_zoom_action(Keysym::plus, control),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(Keysym::equal, control),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(Keysym::KP_Add, control),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(Keysym::minus, control),
            Some(FontZoomAction::Decrease)
        );
        assert_eq!(
            font_zoom_action(Keysym::_0, control),
            Some(FontZoomAction::Reset)
        );
        assert_eq!(font_zoom_action(Keysym::plus, Modifiers::default()), None);
        assert_eq!(
            font_zoom_action(
                Keysym::plus,
                Modifiers {
                    alt: true,
                    ..control
                }
            ),
            None
        );
    }
}
