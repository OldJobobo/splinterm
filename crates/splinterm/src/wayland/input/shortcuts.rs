//! Pure shortcut resolution before adapter-side effects.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use crate::{
    keymap::{ActionId, ActiveModifiers, KeyIdentity, KeymapPress, PrefixState, ResolvedKeymap},
    pane::FocusDirection,
};
use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum PaneTopologyAction {
    Split(splinterm_core::Axis),
    Close,
    AdjustRatio(i16),
    ResizeCells(FocusDirection, u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum CommandPaletteShortcutAction {
    Open,
    Consume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum CopyModeDesktopAction {
    CopySelection,
    Consume,
}

pub(in crate::wayland) fn consume_detached_enter_press(
    keysym: Keysym,
    modifiers: Modifiers,
    viewport_detached: bool,
    raw_code: u32,
    consumed_keys: &mut HashSet<u32>,
) -> Option<super::HistoryNavigation> {
    let navigation = (viewport_detached
        && matches!(keysym, Keysym::Return | Keysym::KP_Enter)
        && !modifiers.ctrl
        && !modifiers.shift
        && !modifiers.alt
        && !modifiers.logo)
        .then_some(super::HistoryNavigation::ReturnToLive)?;
    consumed_keys.insert(raw_code);
    Some(navigation)
}

pub(in crate::wayland) fn copy_mode_desktop_action(
    keysym: Keysym,
    modifiers: Modifiers,
) -> Option<CopyModeDesktopAction> {
    if !modifiers.logo || modifiers.ctrl || modifiers.alt || modifiers.shift {
        return None;
    }
    match keysym {
        Keysym::c | Keysym::C => Some(CopyModeDesktopAction::CopySelection),
        Keysym::v | Keysym::V | Keysym::x | Keysym::X | Keysym::z | Keysym::Z => {
            Some(CopyModeDesktopAction::Consume)
        }
        _ => None,
    }
}

fn active_modifiers(modifiers: Modifiers) -> ActiveModifiers {
    ActiveModifiers {
        ctrl: modifiers.ctrl,
        shift: modifiers.shift,
        alt: modifiers.alt,
        logo: modifiers.logo,
    }
}

pub(in crate::wayland) fn shortcut_action_for(
    keymap: &ResolvedKeymap,
    keysym: Keysym,
    modifiers: Modifiers,
) -> Option<ActionId> {
    keymap.action(key_identity(keysym)?, active_modifiers(modifiers))
}

pub(in crate::wayland) fn owned_field_clipboard_action(
    keymap: &ResolvedKeymap,
    keysym: Keysym,
    modifiers: Modifiers,
) -> Option<ActionId> {
    shortcut_action_for(keymap, keysym, modifiers)
        .filter(|action| matches!(action, ActionId::ClipboardCopy | ActionId::ClipboardPaste))
}

pub(in crate::wayland) fn keymap_press_for(
    keymap: &ResolvedKeymap,
    prefix_state: &mut PrefixState,
    keysym: Keysym,
    modifiers: Modifiers,
    raw_code: u32,
    now: Instant,
    timeout: Duration,
) -> KeymapPress {
    let Some(key) = key_identity(keysym) else {
        if prefix_state.is_armed() {
            if is_modifier_key(keysym) {
                return KeymapPress::PrefixModifier;
            }
            prefix_state.clear();
            return KeymapPress::Consumed(None);
        }
        return KeymapPress::PassThrough;
    };
    keymap.press(
        prefix_state,
        key,
        active_modifiers(modifiers),
        raw_code,
        now,
        timeout,
    )
}

pub(in crate::wayland) fn command_palette_shortcut_action(
    action: Option<ActionId>,
    repeat: bool,
    managed_tabs: bool,
    blocked: bool,
) -> Option<CommandPaletteShortcutAction> {
    if !managed_tabs || action != Some(ActionId::CommandPalette) {
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
    action: Option<ActionId>,
    repeat: bool,
    request_pending: bool,
) -> Option<SessionPickerShortcutAction> {
    if action != Some(ActionId::RecentSessions) {
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
    CloseOthers,
    Select(usize),
    Move(isize),
    Rename,
    Terminate,
    Choose,
    Consume,
}

pub(in crate::wayland) fn tab_shortcut_action(
    action: Option<ActionId>,
    repeat: bool,
    managed_tabs: bool,
) -> Option<TabShortcutAction> {
    if !managed_tabs {
        return None;
    }
    let action = match action? {
        ActionId::NextDojo => TabShortcutAction::Next,
        ActionId::PreviousDojo => TabShortcutAction::Previous,
        ActionId::NewDojo => TabShortcutAction::NewDojo,
        ActionId::CloseCurrentTab => TabShortcutAction::Close,
        ActionId::CloseOtherTabs => TabShortcutAction::CloseOthers,
        ActionId::SelectDojo1 => TabShortcutAction::Select(0),
        ActionId::SelectDojo2 => TabShortcutAction::Select(1),
        ActionId::SelectDojo3 => TabShortcutAction::Select(2),
        ActionId::SelectDojo4 => TabShortcutAction::Select(3),
        ActionId::SelectDojo5 => TabShortcutAction::Select(4),
        ActionId::SelectDojo6 => TabShortcutAction::Select(5),
        ActionId::SelectDojo7 => TabShortcutAction::Select(6),
        ActionId::SelectDojo8 => TabShortcutAction::Select(7),
        ActionId::SelectDojo9 => TabShortcutAction::Select(8),
        ActionId::MoveDojoLeft => TabShortcutAction::Move(-1),
        ActionId::MoveDojoRight => TabShortcutAction::Move(1),
        ActionId::RenameCurrentTab => TabShortcutAction::Rename,
        ActionId::TerminateCurrentDojo => TabShortcutAction::Terminate,
        ActionId::DojoChooser => TabShortcutAction::Choose,
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
    action: Option<ActionId>,
) -> Option<PaneTopologyAction> {
    match action? {
        ActionId::SplitHorizontal => {
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Horizontal))
        }
        ActionId::SplitVertical => Some(PaneTopologyAction::Split(splinterm_core::Axis::Vertical)),
        ActionId::CloseFocusedPane => Some(PaneTopologyAction::Close),
        ActionId::ResizePaneSmaller => Some(PaneTopologyAction::AdjustRatio(-50)),
        ActionId::ResizePaneLarger => Some(PaneTopologyAction::AdjustRatio(50)),
        ActionId::ResizePaneLeftFive => {
            Some(PaneTopologyAction::ResizeCells(FocusDirection::Left, 5))
        }
        ActionId::ResizePaneRightFive => {
            Some(PaneTopologyAction::ResizeCells(FocusDirection::Right, 5))
        }
        ActionId::ResizePaneUpFive => Some(PaneTopologyAction::ResizeCells(FocusDirection::Up, 5)),
        ActionId::ResizePaneDownFive => {
            Some(PaneTopologyAction::ResizeCells(FocusDirection::Down, 5))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum PaneFocusAction {
    Direction(FocusDirection),
}

pub(in crate::wayland) fn pane_focus_action(action: Option<ActionId>) -> Option<PaneFocusAction> {
    let direction = match action? {
        ActionId::FocusLeft => FocusDirection::Left,
        ActionId::FocusRight => FocusDirection::Right,
        ActionId::FocusUp => FocusDirection::Up,
        ActionId::FocusDown => FocusDirection::Down,
        _ => return None,
    };
    Some(PaneFocusAction::Direction(direction))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wayland) enum FontZoomAction {
    Increase,
    Decrease,
    Reset,
}

pub(in crate::wayland) fn font_zoom_action(action: Option<ActionId>) -> Option<FontZoomAction> {
    match action? {
        ActionId::ZoomIn => Some(FontZoomAction::Increase),
        ActionId::ZoomOut => Some(FontZoomAction::Decrease),
        ActionId::ResetZoom => Some(FontZoomAction::Reset),
        _ => None,
    }
}

fn is_modifier_key(keysym: Keysym) -> bool {
    matches!(
        keysym,
        Keysym::Shift_L
            | Keysym::Shift_R
            | Keysym::Control_L
            | Keysym::Control_R
            | Keysym::Alt_L
            | Keysym::Alt_R
            | Keysym::Super_L
            | Keysym::Super_R
            | Keysym::Meta_L
            | Keysym::Meta_R
            | Keysym::ISO_Level3_Shift
            | Keysym::Caps_Lock
            | Keysym::Num_Lock
    )
}

fn key_identity(keysym: Keysym) -> Option<KeyIdentity> {
    let key = match keysym {
        Keysym::a | Keysym::A => KeyIdentity::Character('a'),
        Keysym::b | Keysym::B => KeyIdentity::Character('b'),
        Keysym::c | Keysym::C => KeyIdentity::Character('c'),
        Keysym::d | Keysym::D => KeyIdentity::Character('d'),
        Keysym::e | Keysym::E => KeyIdentity::Character('e'),
        Keysym::f | Keysym::F => KeyIdentity::Character('f'),
        Keysym::g | Keysym::G => KeyIdentity::Character('g'),
        Keysym::h | Keysym::H => KeyIdentity::Character('h'),
        Keysym::i | Keysym::I => KeyIdentity::Character('i'),
        Keysym::j | Keysym::J => KeyIdentity::Character('j'),
        Keysym::k | Keysym::K => KeyIdentity::Character('k'),
        Keysym::l | Keysym::L => KeyIdentity::Character('l'),
        Keysym::m | Keysym::M => KeyIdentity::Character('m'),
        Keysym::n | Keysym::N => KeyIdentity::Character('n'),
        Keysym::o | Keysym::O => KeyIdentity::Character('o'),
        Keysym::p | Keysym::P => KeyIdentity::Character('p'),
        Keysym::q | Keysym::Q => KeyIdentity::Character('q'),
        Keysym::r | Keysym::R => KeyIdentity::Character('r'),
        Keysym::s | Keysym::S => KeyIdentity::Character('s'),
        Keysym::t | Keysym::T => KeyIdentity::Character('t'),
        Keysym::u | Keysym::U => KeyIdentity::Character('u'),
        Keysym::v | Keysym::V => KeyIdentity::Character('v'),
        Keysym::w | Keysym::W => KeyIdentity::Character('w'),
        Keysym::x | Keysym::X => KeyIdentity::Character('x'),
        Keysym::y | Keysym::Y => KeyIdentity::Character('y'),
        Keysym::z | Keysym::Z => KeyIdentity::Character('z'),
        Keysym::Tab | Keysym::ISO_Left_Tab => KeyIdentity::Tab,
        Keysym::Return | Keysym::KP_Enter => KeyIdentity::Enter,
        Keysym::Escape => KeyIdentity::Escape,
        Keysym::space => KeyIdentity::Space,
        Keysym::slash | Keysym::question => KeyIdentity::Slash,
        Keysym::backslash | Keysym::bar => KeyIdentity::Backslash,
        Keysym::bracketleft | Keysym::braceleft => KeyIdentity::BracketLeft,
        Keysym::bracketright | Keysym::braceright => KeyIdentity::BracketRight,
        Keysym::Left => KeyIdentity::Left,
        Keysym::Right => KeyIdentity::Right,
        Keysym::Up => KeyIdentity::Up,
        Keysym::Down => KeyIdentity::Down,
        Keysym::Page_Up => KeyIdentity::PageUp,
        Keysym::Page_Down => KeyIdentity::PageDown,
        Keysym::End => KeyIdentity::End,
        Keysym::Insert | Keysym::KP_Insert => KeyIdentity::Insert,
        Keysym::plus | Keysym::KP_Add => KeyIdentity::Plus,
        Keysym::equal => KeyIdentity::Equal,
        Keysym::minus | Keysym::KP_Subtract => KeyIdentity::Minus,
        Keysym::_0 | Keysym::KP_0 => KeyIdentity::Zero,
        Keysym::_1 => KeyIdentity::One,
        Keysym::_2 => KeyIdentity::Two,
        Keysym::_3 => KeyIdentity::Three,
        Keysym::_4 => KeyIdentity::Four,
        Keysym::_5 => KeyIdentity::Five,
        Keysym::_6 => KeyIdentity::Six,
        Keysym::_7 => KeyIdentity::Seven,
        Keysym::_8 => KeyIdentity::Eight,
        Keysym::_9 => KeyIdentity::Nine,
        Keysym::ampersand => KeyIdentity::Ampersand,
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut_action(keysym: Keysym, modifiers: Modifiers) -> Option<ActionId> {
        shortcut_action_for(&ResolvedKeymap::default(), keysym, modifiers)
    }

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn detached_plain_enter_and_keypad_enter_are_consumed_through_release() {
        for keysym in [Keysym::Return, Keysym::KP_Enter] {
            let mut consumed = HashSet::new();
            assert_eq!(
                consume_detached_enter_press(keysym, Modifiers::default(), true, 28, &mut consumed),
                Some(super::super::HistoryNavigation::ReturnToLive)
            );
            assert!(
                consumed.contains(&28),
                "repeat remains consumed after return-live"
            );
            consumed.remove(&28);
            assert!(!consumed.contains(&28), "matching release clears the key");
            assert_eq!(
                consume_detached_enter_press(
                    keysym,
                    Modifiers::default(),
                    false,
                    28,
                    &mut consumed
                ),
                None,
                "the next live press retains terminal encoding"
            );
            assert!(!consumed.contains(&28));
            assert_eq!(
                consume_detached_enter_press(
                    keysym,
                    Modifiers {
                        shift: true,
                        ..Modifiers::default()
                    },
                    true,
                    28,
                    &mut consumed
                ),
                None
            );
        }
        assert_eq!(
            consume_detached_enter_press(
                Keysym::space,
                Modifiers::default(),
                true,
                57,
                &mut HashSet::new()
            ),
            None
        );
    }

    #[test]
    fn copy_mode_desktop_shortcuts_copy_or_consume_without_terminal_routes() {
        let desktop = Modifiers {
            logo: true,
            ..Modifiers::default()
        };
        assert_eq!(
            copy_mode_desktop_action(Keysym::c, desktop),
            Some(CopyModeDesktopAction::CopySelection)
        );
        for keysym in [Keysym::v, Keysym::x, Keysym::z] {
            assert_eq!(
                copy_mode_desktop_action(keysym, desktop),
                Some(CopyModeDesktopAction::Consume)
            );
        }
        assert_eq!(
            copy_mode_desktop_action(
                Keysym::c,
                Modifiers {
                    logo: true,
                    shift: true,
                    ..Modifiers::default()
                }
            ),
            None
        );
    }

    #[test]
    fn omarchy_universal_clipboard_translation_reaches_terminal_actions() {
        for profile in [
            crate::keymap::KeymapProfile::Splinterm,
            crate::keymap::KeymapProfile::OmarchyTmux,
        ] {
            let keymap = crate::keymap::built_in_keymap(profile);
            for logo in [false, true] {
                assert_eq!(
                    shortcut_action_for(
                        &keymap,
                        Keysym::Insert,
                        Modifiers {
                            ctrl: true,
                            logo,
                            ..Modifiers::default()
                        }
                    ),
                    Some(ActionId::ClipboardCopy)
                );
            }
            assert_eq!(
                shortcut_action_for(
                    &keymap,
                    Keysym::Insert,
                    Modifiers {
                        shift: true,
                        ..Modifiers::default()
                    }
                ),
                Some(ActionId::ClipboardPaste)
            );
        }
    }

    #[test]
    fn owned_fields_accept_effective_clipboard_actions_without_other_shortcuts() {
        for profile in [
            crate::keymap::KeymapProfile::Splinterm,
            crate::keymap::KeymapProfile::OmarchyTmux,
        ] {
            let keymap = crate::keymap::built_in_keymap(profile);
            assert_eq!(
                owned_field_clipboard_action(&keymap, Keysym::c, ctrl_shift()),
                Some(ActionId::ClipboardCopy)
            );
            assert_eq!(
                owned_field_clipboard_action(&keymap, Keysym::v, ctrl_shift()),
                Some(ActionId::ClipboardPaste)
            );
            assert_eq!(
                owned_field_clipboard_action(&keymap, Keysym::f, ctrl_shift()),
                None
            );
        }
    }

    #[test]
    fn command_palette_shortcut_is_exact_managed_and_blocked_safely() {
        let action = shortcut_action(Keysym::p, ctrl_shift());
        assert_eq!(
            command_palette_shortcut_action(action, false, true, false),
            Some(CommandPaletteShortcutAction::Open)
        );
        assert_eq!(
            command_palette_shortcut_action(action, true, true, false),
            Some(CommandPaletteShortcutAction::Consume)
        );
        assert_eq!(
            command_palette_shortcut_action(action, false, true, true),
            Some(CommandPaletteShortcutAction::Consume)
        );
        assert_eq!(
            command_palette_shortcut_action(action, false, false, false),
            None
        );
        assert_eq!(shortcut_action(Keysym::p, Modifiers::default()), None);
    }

    #[test]
    fn session_picker_shortcut_is_exact_and_application_owned() {
        let action = shortcut_action(Keysym::s, ctrl_shift());
        assert_eq!(
            session_picker_shortcut_action(action, false, false),
            Some(SessionPickerShortcutAction::Request)
        );
        assert_eq!(
            session_picker_shortcut_action(action, true, false),
            Some(SessionPickerShortcutAction::Consume)
        );
        assert_eq!(
            session_picker_shortcut_action(action, false, true),
            Some(SessionPickerShortcutAction::Consume)
        );
        assert_eq!(shortcut_action(Keysym::s, Modifiers::default()), None);
        assert_eq!(
            shortcut_action(
                Keysym::s,
                Modifiers {
                    alt: true,
                    ..ctrl_shift()
                }
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
        assert_eq!(
            tab_shortcut_action(shortcut_action(Keysym::Tab, ctrl), false, true),
            Some(TabShortcutAction::Next)
        );
        assert_eq!(
            tab_shortcut_action(
                shortcut_action(Keysym::ISO_Left_Tab, ctrl_shift()),
                false,
                true,
            ),
            Some(TabShortcutAction::Previous)
        );
        assert_eq!(
            tab_shortcut_action(shortcut_action(Keysym::d, ctrl_shift()), false, true),
            Some(TabShortcutAction::NewDojo)
        );
        assert_eq!(
            tab_shortcut_action(shortcut_action(Keysym::q, ctrl_shift()), false, true),
            Some(TabShortcutAction::Close)
        );
        assert_eq!(
            tab_shortcut_action(Some(ActionId::CloseOtherTabs), false, true),
            Some(TabShortcutAction::CloseOthers)
        );
        assert_eq!(
            tab_shortcut_action(Some(ActionId::CloseOtherTabs), true, true),
            Some(TabShortcutAction::Consume)
        );
        assert_eq!(
            tab_shortcut_action(shortcut_action(Keysym::Tab, ctrl), true, true),
            Some(TabShortcutAction::Consume)
        );
        assert_eq!(
            tab_shortcut_action(shortcut_action(Keysym::Tab, ctrl), false, false),
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
    fn omarchy_numeric_and_reorder_bindings_dispatch_typed_tab_actions() {
        let keymap = crate::keymap::built_in_keymap(crate::keymap::KeymapProfile::OmarchyTmux);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let alt_shift = Modifiers {
            alt: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            tab_shortcut_action(shortcut_action_for(&keymap, Keysym::_1, alt), false, true,),
            Some(TabShortcutAction::Select(0))
        );
        assert_eq!(
            tab_shortcut_action(
                shortcut_action_for(&keymap, Keysym::Right, alt_shift),
                false,
                true,
            ),
            Some(TabShortcutAction::Move(1))
        );
        assert_eq!(
            shortcut_action_for(&keymap, Keysym::Down, alt),
            Some(ActionId::NextLair)
        );
    }

    #[test]
    fn every_bindable_action_has_an_explicit_runtime_route() {
        for action in ActionId::BINDABLE {
            let route = match action {
                ActionId::CommandPalette => "command palette boundary",
                ActionId::RecentSessions => "Dojo picker boundary",
                ActionId::NewSession
                | ActionId::RenameCurrentLair
                | ActionId::SaveCurrentLair
                | ActionId::ToggleCurrentLairPin
                | ActionId::PreviewCurrentLair
                | ActionId::RestoreCurrentLair
                | ActionId::TerminateCurrentLair
                | ActionId::PreviousLair
                | ActionId::NextLair
                | ActionId::LairChooser => "Lair topology route",
                ActionId::RenameCurrentTab
                | ActionId::NewDojo
                | ActionId::PreviousDojo
                | ActionId::NextDojo
                | ActionId::CloseCurrentTab
                | ActionId::CloseOtherTabs
                | ActionId::TerminateCurrentDojo
                | ActionId::DojoChooser
                | ActionId::SelectDojo1
                | ActionId::SelectDojo2
                | ActionId::SelectDojo3
                | ActionId::SelectDojo4
                | ActionId::SelectDojo5
                | ActionId::SelectDojo6
                | ActionId::SelectDojo7
                | ActionId::SelectDojo8
                | ActionId::SelectDojo9
                | ActionId::MoveDojoLeft
                | ActionId::MoveDojoRight => "Window-local Dojo route",
                ActionId::SplitHorizontal
                | ActionId::SplitVertical
                | ActionId::CloseFocusedPane
                | ActionId::ResizePaneSmaller
                | ActionId::ResizePaneLarger
                | ActionId::ResizePaneLeftFive
                | ActionId::ResizePaneRightFive
                | ActionId::ResizePaneUpFive
                | ActionId::ResizePaneDownFive => "pane topology route",
                ActionId::FocusLeft
                | ActionId::FocusRight
                | ActionId::FocusUp
                | ActionId::FocusDown => "pane focus route",
                ActionId::ZoomIn | ActionId::ZoomOut | ActionId::ResetZoom => "font zoom route",
                ActionId::DetachWindow
                | ActionId::TogglePaneZoom
                | ActionId::ToggleTabStrip
                | ActionId::BindingHelp
                | ActionId::CopyModeEnter
                | ActionId::ConfigReload
                | ActionId::SendPrefix => "typed local application route",
                ActionId::SearchScrollback
                | ActionId::PageUp
                | ActionId::PageDown
                | ActionId::ReturnToLive => "history route",
                ActionId::RequestControl
                | ActionId::ReleaseControl
                | ActionId::ForceControl
                | ActionId::RevokeAllAccess
                | ActionId::AcceptControlTransfer
                | ActionId::DenyControlTransfer => "authority route",
                ActionId::ClipboardCopy | ActionId::ClipboardPaste => "clipboard route",
            };
            assert!(!route.is_empty(), "{}", action.config_name());
        }
    }

    #[test]
    fn pane_bindings_are_typed_and_do_not_capture_plain_keys() {
        assert_eq!(
            pane_focus_action(shortcut_action(Keysym::Left, ctrl_shift())),
            Some(PaneFocusAction::Direction(FocusDirection::Left))
        );
        assert_eq!(
            pane_focus_action(shortcut_action(Keysym::Left, Modifiers::default())),
            None
        );
        assert_eq!(
            pane_topology_action(shortcut_action(Keysym::Return, ctrl_shift())),
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Horizontal))
        );
        assert_eq!(
            pane_topology_action(shortcut_action(Keysym::bar, ctrl_shift())),
            Some(PaneTopologyAction::Split(splinterm_core::Axis::Vertical))
        );
        assert_eq!(
            pane_topology_action(shortcut_action(Keysym::W, ctrl_shift())),
            Some(PaneTopologyAction::Close)
        );
        assert_eq!(
            pane_topology_action(shortcut_action(Keysym::braceleft, ctrl_shift())),
            Some(PaneTopologyAction::AdjustRatio(-50))
        );
    }

    #[test]
    fn font_zoom_preserves_foot_compatible_shift_and_keypad_aliases() {
        let control = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            font_zoom_action(shortcut_action(Keysym::plus, control)),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(shortcut_action(Keysym::equal, control)),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(shortcut_action(Keysym::KP_Add, control)),
            Some(FontZoomAction::Increase)
        );
        assert_eq!(
            font_zoom_action(shortcut_action(Keysym::minus, control)),
            Some(FontZoomAction::Decrease)
        );
        assert_eq!(
            font_zoom_action(shortcut_action(Keysym::_0, control)),
            Some(FontZoomAction::Reset)
        );
        assert_eq!(shortcut_action(Keysym::plus, Modifiers::default()), None);
    }

    #[test]
    fn resolved_overlay_changes_dispatch_without_bypassing_the_adapter() {
        let resolved = crate::keymap::resolve_keymap_text(
            crate::keymap::KeymapProfile::Splinterm,
            r#"
version = 1
[[unbind]]
sequence = ["Ctrl+Shift+P"]
[[binding]]
sequence = ["Ctrl+Alt+P"]
action = "app.command-palette"
"#,
            std::path::Path::new("keybindings.toml"),
        )
        .unwrap();
        assert_eq!(
            shortcut_action_for(
                &resolved.keymap,
                Keysym::p,
                Modifiers {
                    ctrl: true,
                    alt: true,
                    ..Modifiers::default()
                }
            ),
            Some(ActionId::CommandPalette)
        );
        assert_eq!(
            shortcut_action_for(&resolved.keymap, Keysym::p, ctrl_shift()),
            None
        );
    }

    #[test]
    fn strict_prefix_overlay_reaches_the_same_typed_runtime_route() {
        let resolved = crate::keymap::resolve_keymap_text(
            crate::keymap::KeymapProfile::OmarchyTmux,
            r#"
version = 1
inherits = "omarchy-tmux"
[[binding]]
sequence = ["Prefix", "o"]
action = "dojo.close-other-tabs"
"#,
            std::path::Path::new("keybindings.toml"),
        )
        .unwrap();
        let mut state = PrefixState::Idle;
        let now = Instant::now();
        let control = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            keymap_press_for(
                &resolved.keymap,
                &mut state,
                Keysym::space,
                control,
                1,
                now,
                Duration::from_secs(1),
            ),
            KeymapPress::Consumed(None)
        );
        let action = match keymap_press_for(
            &resolved.keymap,
            &mut state,
            Keysym::o,
            Modifiers::default(),
            2,
            now,
            Duration::from_secs(1),
        ) {
            KeymapPress::Consumed(action) => action,
            other => panic!("custom prefix did not dispatch: {other:?}"),
        };
        assert_eq!(action, Some(ActionId::CloseOtherTabs));
        assert_eq!(
            tab_shortcut_action(action, false, true),
            Some(TabShortcutAction::CloseOthers)
        );
    }

    #[test]
    fn omarchy_prefix_resolution_uses_xkb_keys_and_consumes_unknown_seconds() {
        let keymap = crate::keymap::built_in_keymap(crate::keymap::KeymapProfile::OmarchyTmux);
        let mut state = PrefixState::Idle;
        let control = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let now = Instant::now();
        assert_eq!(
            keymap_press_for(
                &keymap,
                &mut state,
                Keysym::space,
                control,
                1,
                now,
                Duration::from_secs(1),
            ),
            KeymapPress::Consumed(None)
        );
        assert_eq!(
            keymap_press_for(
                &keymap,
                &mut state,
                Keysym::Shift_L,
                Modifiers::default(),
                2,
                now,
                Duration::from_secs(1),
            ),
            KeymapPress::PrefixModifier
        );
        assert!(state.is_armed());
        assert_eq!(
            keymap_press_for(
                &keymap,
                &mut state,
                Keysym::question,
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                5,
                now,
                Duration::from_secs(1),
            ),
            KeymapPress::Consumed(Some(ActionId::BindingHelp))
        );
        let _ = keymap_press_for(
            &keymap,
            &mut state,
            Keysym::b,
            control,
            3,
            now,
            Duration::from_secs(1),
        );
        assert_eq!(
            keymap_press_for(
                &keymap,
                &mut state,
                Keysym::F1,
                Modifiers::default(),
                4,
                now,
                Duration::from_secs(1),
            ),
            KeymapPress::Consumed(None)
        );
    }

    #[test]
    fn legacy_permissive_modifier_edges_remain_explicit() {
        let ctrl_shift_alt = Modifiers {
            ctrl: true,
            shift: true,
            alt: true,
            ..Modifiers::default()
        };
        assert_eq!(
            shortcut_action(Keysym::c, ctrl_shift_alt),
            Some(ActionId::ClipboardCopy)
        );
        assert_eq!(
            shortcut_action(
                Keysym::Page_Up,
                Modifiers {
                    shift: true,
                    logo: true,
                    ..Modifiers::default()
                }
            ),
            Some(ActionId::PageUp)
        );
        assert_eq!(shortcut_action(Keysym::p, ctrl_shift_alt), None);
    }
}
