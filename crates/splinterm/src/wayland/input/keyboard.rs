//! Terminal keyboard sequence encoding.

use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};
use splinterm_protocol::TerminalInputModes;

fn modifier_parameter(modifiers: Modifiers) -> u8 {
    1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.ctrl)
}

fn modified_final(final_byte: u8, modifiers: Modifiers, application: bool) -> Vec<u8> {
    let parameter = modifier_parameter(modifiers);
    if parameter == 1 {
        if application {
            vec![0x1b, b'O', final_byte]
        } else {
            vec![0x1b, b'[', final_byte]
        }
    } else {
        format!("\x1b[1;{parameter}{}", char::from(final_byte)).into_bytes()
    }
}

fn modified_tilde(code: u8, modifiers: Modifiers) -> Vec<u8> {
    let parameter = modifier_parameter(modifiers);
    if parameter == 1 {
        format!("\x1b[{code}~").into_bytes()
    } else {
        format!("\x1b[{code};{parameter}~").into_bytes()
    }
}

fn ctrl_utf8(utf8: &str) -> Option<Vec<u8>> {
    if utf8.len() == 1 && utf8.as_bytes()[0] < 0x20 {
        return Some(utf8.as_bytes().to_vec());
    }
    let character = utf8.chars().next()?;
    if utf8.chars().count() != 1 {
        return None;
    }
    let byte = match character {
        '@' | ' ' | '2' => 0,
        'a'..='z' => u8::try_from(u32::from(character) - u32::from('a') + 1).ok()?,
        'A'..='Z' => u8::try_from(u32::from(character) - u32::from('A') + 1).ok()?,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    };
    Some(vec![byte])
}

fn keypad_input(keysym: Keysym) -> Option<u8> {
    Some(match keysym {
        Keysym::KP_0 => b'p',
        Keysym::KP_1 => b'q',
        Keysym::KP_2 => b'r',
        Keysym::KP_3 => b's',
        Keysym::KP_4 => b't',
        Keysym::KP_5 => b'u',
        Keysym::KP_6 => b'v',
        Keysym::KP_7 => b'w',
        Keysym::KP_8 => b'x',
        Keysym::KP_9 => b'y',
        Keysym::KP_Decimal => b'n',
        Keysym::KP_Divide => b'o',
        Keysym::KP_Multiply => b'j',
        Keysym::KP_Subtract => b'm',
        Keysym::KP_Add => b'k',
        Keysym::KP_Separator => b'l',
        Keysym::KP_Equal => b'X',
        _ => return None,
    })
}

pub(in crate::wayland) fn key_input(
    keysym: Keysym,
    utf8: Option<&str>,
    modifiers: Modifiers,
    modes: TerminalInputModes,
) -> Option<Vec<u8>> {
    let mut alt_is_encoded = false;
    let mut bytes = match keysym {
        Keysym::Return | Keysym::KP_Enter => vec![b'\r'],
        Keysym::BackSpace => vec![0x7f],
        Keysym::ISO_Left_Tab => b"\x1b[Z".to_vec(),
        Keysym::Tab | Keysym::KP_Tab if modifiers.shift => b"\x1b[Z".to_vec(),
        Keysym::Tab | Keysym::KP_Tab => vec![b'\t'],
        Keysym::Escape => vec![0x1b],
        Keysym::Up => {
            alt_is_encoded = true;
            modified_final(b'A', modifiers, modes.application_cursor)
        }
        Keysym::Down => {
            alt_is_encoded = true;
            modified_final(b'B', modifiers, modes.application_cursor)
        }
        Keysym::Right => {
            alt_is_encoded = true;
            modified_final(b'C', modifiers, modes.application_cursor)
        }
        Keysym::Left => {
            alt_is_encoded = true;
            modified_final(b'D', modifiers, modes.application_cursor)
        }
        Keysym::Home => {
            alt_is_encoded = true;
            modified_final(b'H', modifiers, modes.application_cursor)
        }
        Keysym::End => {
            alt_is_encoded = true;
            modified_final(b'F', modifiers, modes.application_cursor)
        }
        Keysym::Insert => {
            alt_is_encoded = true;
            modified_tilde(2, modifiers)
        }
        Keysym::Delete => {
            alt_is_encoded = true;
            modified_tilde(3, modifiers)
        }
        Keysym::Page_Up => {
            alt_is_encoded = true;
            modified_tilde(5, modifiers)
        }
        Keysym::Page_Down => {
            alt_is_encoded = true;
            modified_tilde(6, modifiers)
        }
        Keysym::F1 | Keysym::F2 | Keysym::F3 | Keysym::F4 => {
            alt_is_encoded = true;
            let final_byte = b'P' + u8::try_from(keysym.raw() - Keysym::F1.raw()).ok()?;
            modified_final(final_byte, modifiers, true)
        }
        Keysym::F5
        | Keysym::F6
        | Keysym::F7
        | Keysym::F8
        | Keysym::F9
        | Keysym::F10
        | Keysym::F11
        | Keysym::F12 => {
            alt_is_encoded = true;
            let code = match keysym {
                Keysym::F5 => 15,
                Keysym::F6 => 17,
                Keysym::F7 => 18,
                Keysym::F8 => 19,
                Keysym::F9 => 20,
                Keysym::F10 => 21,
                Keysym::F11 => 23,
                Keysym::F12 => 24,
                _ => unreachable!(),
            };
            modified_tilde(code, modifiers)
        }
        _ if modes.application_keypad && keypad_input(keysym).is_some() => {
            let final_byte = keypad_input(keysym)?;
            alt_is_encoded = true;
            modified_final(final_byte, modifiers, true)
        }
        _ if modifiers.ctrl => ctrl_utf8(utf8?)?,
        _ => utf8.filter(|text| !text.is_empty())?.as_bytes().to_vec(),
    };
    if modifiers.alt && !alt_is_encoded {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal_modes() -> TerminalInputModes {
        TerminalInputModes {
            application_cursor: false,
            application_keypad: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor_visible: true,
            cursor_blink: true,
            mouse_tracking: splinterm_protocol::MouseTracking::None,
            sgr_mouse: false,
        }
    }

    fn encoded(keysym: Keysym, utf8: Option<&str>, modifiers: Modifiers) -> Option<Vec<u8>> {
        key_input(keysym, utf8, modifiers, normal_modes())
    }

    #[test]
    fn essential_key_mapping_uses_utf8_modifiers_and_terminal_sequences() {
        let plain = Modifiers::default();
        assert_eq!(encoded(Keysym::a, Some("a"), plain), Some(b"a".to_vec()));
        assert_eq!(encoded(Keysym::Return, None, plain), Some(vec![b'\r']));
        assert_eq!(encoded(Keysym::BackSpace, None, plain), Some(vec![0x7f]));
        assert_eq!(encoded(Keysym::Tab, None, plain), Some(vec![b'\t']));
        assert_eq!(encoded(Keysym::Escape, None, plain), Some(vec![0x1b]));
        assert_eq!(encoded(Keysym::Up, None, plain), Some(b"\x1b[A".to_vec()));
        assert_eq!(encoded(Keysym::Down, None, plain), Some(b"\x1b[B".to_vec()));
        assert_eq!(encoded(Keysym::Left, None, plain), Some(b"\x1b[D".to_vec()));
        assert_eq!(
            encoded(Keysym::Right, None, plain),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(encoded(Keysym::Home, None, plain), Some(b"\x1b[H".to_vec()));
        assert_eq!(encoded(Keysym::End, None, plain), Some(b"\x1b[F".to_vec()));
        assert_eq!(
            encoded(Keysym::Insert, None, plain),
            Some(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Delete, None, plain),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Page_Up, None, plain),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Page_Down, None, plain),
            Some(b"\x1b[6~".to_vec())
        );
        assert_eq!(encoded(Keysym::F1, None, plain), Some(b"\x1bOP".to_vec()));
        assert_eq!(
            encoded(Keysym::F12, None, plain),
            Some(b"\x1b[24~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::ISO_Left_Tab, None, plain),
            Some(b"\x1b[Z".to_vec())
        );

        let alt = Modifiers { alt: true, ..plain };
        assert_eq!(encoded(Keysym::x, Some("x"), alt), Some(b"\x1bx".to_vec()));
        let control = Modifiers {
            ctrl: true,
            ..plain
        };
        assert_eq!(encoded(Keysym::c, Some("c"), control), Some(vec![3]));
        assert_eq!(encoded(Keysym::c, Some("\u{3}"), control), Some(vec![3]));
        assert_eq!(encoded(Keysym::Shift_L, None, plain), None);
        assert_eq!(
            encoded(Keysym::eacute, Some("é"), plain),
            Some("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn mode_and_modifier_key_sequences_match_xterm_conventions() {
        let shift_ctrl = Modifiers {
            shift: true,
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            encoded(Keysym::Up, None, shift_ctrl),
            Some(b"\x1b[1;6A".to_vec())
        );
        assert_eq!(
            encoded(Keysym::Delete, None, shift_ctrl),
            Some(b"\x1b[3;6~".to_vec())
        );
        assert_eq!(
            encoded(Keysym::F2, None, shift_ctrl),
            Some(b"\x1b[1;6Q".to_vec())
        );

        let mut modes = normal_modes();
        modes.application_cursor = true;
        assert_eq!(
            key_input(Keysym::Left, None, Modifiers::default(), modes),
            Some(b"\x1bOD".to_vec())
        );
        modes.application_keypad = true;
        assert_eq!(
            key_input(Keysym::KP_7, Some("7"), Modifiers::default(), modes),
            Some(b"\x1bOw".to_vec())
        );
        assert_eq!(
            key_input(Keysym::colon, Some(":"), Modifiers::default(), modes),
            Some(b":".to_vec()),
            "application keypad mode must not consume Neovim commands"
        );
        assert_eq!(
            key_input(Keysym::space, Some(" "), Modifiers::default(), modes),
            Some(b" ".to_vec()),
            "application keypad mode must not consume Space leader"
        );
        assert_eq!(
            key_input(Keysym::w, Some("w"), Modifiers::default(), modes),
            Some(b"w".to_vec())
        );
    }

    #[test]
    fn repeat_mapping_is_identical_to_press_mapping() {
        let modifiers = Modifiers::default();
        let press = encoded(Keysym::Left, None, modifiers);
        let repeat = encoded(Keysym::Left, None, modifiers);
        assert_eq!(press, repeat);
    }
}
