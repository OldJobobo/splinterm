//! Terminal modes owned by the renderer-independent kernel.
//!
//! Derived from Foot 1.27.0 `terminal.h` and DEC mode handling in `csi.c` at
//! commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`.

/// Currently selected screen buffer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActiveScreen {
    /// Normal screen with scrollback.
    #[default]
    Normal,
    /// Alternate screen without persistent scrollback.
    Alternate,
}

/// Mouse reporting trigger mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MouseTracking {
    #[default]
    None,
    Normal,
    Button,
    Any,
}

/// Core ANSI and DEC modes needed by terminal output and future input encoding.
#[allow(
    clippy::struct_excessive_bools,
    reason = "these independent DEC and ANSI modes mirror terminal protocol state"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalModes {
    pub insert: bool,
    pub origin: bool,
    pub auto_margin: bool,
    pub reverse_wrap: bool,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub reverse_video: bool,
    pub cursor_visible: bool,
    pub cursor_blink: bool,
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
    pub mouse_tracking: MouseTracking,
    pub sgr_mouse: bool,
}

impl Default for TerminalModes {
    fn default() -> Self {
        Self {
            insert: false,
            origin: false,
            auto_margin: true,
            reverse_wrap: false,
            application_cursor: false,
            application_keypad: false,
            reverse_video: false,
            cursor_visible: true,
            cursor_blink: true,
            focus_reporting: false,
            bracketed_paste: false,
            mouse_tracking: MouseTracking::None,
            sgr_mouse: false,
        }
    }
}
