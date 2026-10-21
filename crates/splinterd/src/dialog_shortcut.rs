//! The keyboard path to a waiting trusted dialog (Plan 0053 item 9).
//!
//! Trusted dialogs map without keyboard focus and stay inert until the
//! person engages them. The dialog shortcut, `Ctrl+Alt+G` unless
//! `[integration] dialog-shortcut` says otherwise, engages one from the
//! keyboard the way a click inside does. It never answers: the dialog then
//! arms for its usual delay before `G`, `D`, `Escape` or `Enter` count.
//!
//! Inside a dialog that already has keyboard focus, the chord engages it on
//! any compositor. When Hyprland maps trusted dialogs without focus (the
//! Omarchy integration's dialog rule; the client checks), Splinterm also binds
//! the chord for exactly as long as a dialog waits, so it reaches the dialog
//! from any other Window:
//!
//! * The dialog process registers its own PID before it maps
//!   ([`DialogShortcutLease::register`]). Registration adds the PID to a list
//!   kept in Hyprland's Lua state and, when no Splinterm binding exists yet,
//!   binds the chord with [`BIND_DESCRIPTION`].
//! * The binding is non-consuming and only focuses the newest listed PID
//!   that still has a mapped Window. The chord then reaches that dialog,
//!   which engages because it now has focus and sees the exact chord. When no
//!   listed dialog has a Window, the chord reaches the focused application
//!   unchanged, so a stale binding can never take the chord from it.
//! * The PID leaves the list when the dialog exits, and again when the
//!   process that spawned and reaped it drops its lease, so a killed dialog
//!   cannot leave its entry behind. Every registration and removal also drops
//!   PIDs whose process is gone. The binding is removed with the last entry,
//!   and `hyprctl reload` clears it with the rest of the Lua state.
//! * When the person's own configuration already binds the chord in the
//!   default submap, Splinterm binds nothing and their binding keeps working.
//!
//! This module lives in `splinterd` because the daemon reaps the access
//! consent dialog and the client reaps the upgrade and restart questions;
//! both drop a lease after reaping.

use std::{
    env, fmt,
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// The description Splinterm's binding carries in `hyprctl binds`, which
/// tells it apart from the person's own bindings of the same chord.
pub const BIND_DESCRIPTION: &str = "Splinterm: focus the waiting trusted dialog";

/// The Lua global that holds the waiting dialogs' PIDs and the binding.
const LUA_STATE: &str = "splinterm_trusted_dialog_shortcut";

/// How long one `hyprctl` call may take before it is abandoned.
const HYPRCTL_TIMEOUT: Duration = Duration::from_secs(1);

/// The largest `hyprctl` reply read; a bind list is far smaller.
const MAX_HYPRCTL_OUTPUT: u64 = 4 * 1024 * 1024;

/// Hyprland's modifier mask bits, as `hyprctl -j binds` reports them.
const HYPRLAND_CTRL: u64 = 1 << 2;
const HYPRLAND_ALT: u64 = 1 << 3;
const HYPRLAND_SUPER: u64 = 1 << 6;

/// A dialog shortcut chord: at least one of Ctrl, Alt and Super with one
/// letter or digit. Shift is not allowed, because it changes the key a
/// letter or digit produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogShortcut {
    ctrl: bool,
    alt: bool,
    super_key: bool,
    /// Lowercase ASCII letter or digit.
    key: u8,
}

impl Default for DialogShortcut {
    /// `Ctrl+Alt+G`.
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            super_key: false,
            key: b'g',
        }
    }
}

impl DialogShortcut {
    /// Parses an `[integration] dialog-shortcut` value: `none` disables the
    /// shortcut (`Ok(None)`); otherwise modifiers and one key joined by `+`,
    /// such as `ctrl+alt+g`, in any case.
    ///
    /// # Errors
    /// Returns a reason when the value is not a valid chord.
    pub fn parse_setting(value: &str) -> Result<Option<Self>, String> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        let mut shortcut = Self {
            ctrl: false,
            alt: false,
            super_key: false,
            key: 0,
        };
        let parts: Vec<&str> = value.split('+').map(str::trim).collect();
        let Some((key, modifiers)) = parts.split_last() else {
            return Err("expected modifiers and a key, such as ctrl+alt+g".to_owned());
        };
        for modifier in modifiers {
            let slot = match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut shortcut.ctrl,
                "alt" => &mut shortcut.alt,
                "super" | "logo" => &mut shortcut.super_key,
                "shift" => {
                    return Err("shift is not allowed; use ctrl, alt, or super".to_owned());
                }
                other => return Err(format!("unknown modifier {other:?}")),
            };
            if std::mem::replace(slot, true) {
                return Err(format!("modifier {modifier:?} appears twice"));
            }
        }
        match key.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => shortcut.key = byte.to_ascii_lowercase(),
            _ => return Err(format!("the key must be one letter or digit, not {key:?}")),
        }
        if !(shortcut.ctrl || shortcut.alt || shortcut.super_key) {
            return Err("a dialog shortcut needs ctrl, alt, or super".to_owned());
        }
        Ok(Some(shortcut))
    }

    fn modifier_names(self) -> impl Iterator<Item = (&'static str, &'static str)> {
        [
            (self.ctrl, ("CTRL", "Ctrl")),
            (self.alt, ("ALT", "Alt")),
            (self.super_key, ("SUPER", "Super")),
        ]
        .into_iter()
        .filter_map(|(held, names)| held.then_some(names))
    }

    /// The chord as a Hyprland key string, such as `CTRL + ALT + G`.
    #[must_use]
    pub fn hyprland_keys(self) -> String {
        self.modifier_names()
            .map(|(hyprland, _)| hyprland.to_owned())
            .chain(std::iter::once(
                char::from(self.key.to_ascii_uppercase()).to_string(),
            ))
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// The chord's Hyprland modifier mask.
    #[must_use]
    pub const fn hyprland_modmask(self) -> u64 {
        let mut mask = 0;
        if self.ctrl {
            mask |= HYPRLAND_CTRL;
        }
        if self.alt {
            mask |= HYPRLAND_ALT;
        }
        if self.super_key {
            mask |= HYPRLAND_SUPER;
        }
        mask
    }

    /// The chord as the dialogs name it, such as `Ctrl+Alt+G`.
    #[must_use]
    pub fn label(self) -> String {
        self.modifier_names()
            .map(|(_, label)| label.to_owned())
            .chain(std::iter::once(
                char::from(self.key.to_ascii_uppercase()).to_string(),
            ))
            .collect::<Vec<_>>()
            .join("+")
    }

    /// Whether one key press is exactly this chord: the same Ctrl, Alt and
    /// Super state, no Shift, and the chord's key. `key` is the pressed
    /// keysym's ASCII character, when it has one.
    #[must_use]
    #[allow(
        clippy::fn_params_excessive_bools,
        reason = "one flag per modifier the keyboard reports"
    )]
    pub fn matches(self, ctrl: bool, alt: bool, shift: bool, super_key: bool, key: char) -> bool {
        ctrl == self.ctrl
            && alt == self.alt
            && super_key == self.super_key
            && !shift
            && key.is_ascii()
            && key.to_ascii_lowercase() == char::from(self.key)
    }
}

impl fmt::Display for DialogShortcut {
    /// The `config.ini` form, such as `ctrl+alt+g`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.label().to_ascii_lowercase();
        formatter.write_str(&text)
    }
}

/// `text` as a Lua string literal.
fn lua_string(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for character in text.chars() {
        match character {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            character if character.is_control() => {}
            character => literal.push(character),
        }
    }
    literal.push('"');
    literal
}

/// Lua that loads the shared state into `s` (`create` makes it when absent)
/// and defines `alive`, which tells whether a PID's process still exists.
fn lua_prelude(create: bool) -> String {
    let state = lua_string(LUA_STATE);
    let load = if create {
        format!(
            "local s = rawget(_G, {state}) if type(s) ~= \"table\" then s = {{ pids = {{}} }} rawset(_G, {state}, s) end"
        )
    } else {
        format!("local s = rawget(_G, {state}) if type(s) ~= \"table\" then return end")
    };
    format!(
        "{load} local function alive(pid) local f = io.open(\"/proc/\" .. pid .. \"/stat\", \"r\") if f then f:close() return true end return false end"
    )
}

/// Lua that drops `pid` (when given) and every PID whose process is gone.
fn lua_prune(pid: Option<u32>) -> String {
    let matches = pid.map_or_else(String::new, |pid| format!("s.pids[i] == {pid} or "));
    format!(
        "for i = #s.pids, 1, -1 do if {matches}not alive(s.pids[i]) then table.remove(s.pids, i) end end"
    )
}

/// The `hyprctl eval` script that registers dialog `pid` and binds
/// `shortcut` unless Splinterm's binding already exists.
///
/// The binding is non-consuming: it focuses the newest listed dialog that
/// still has a mapped Window and lets the chord through to it, or through to
/// the focused application when none has.
#[must_use]
pub fn register_script(pid: u32, shortcut: DialogShortcut) -> String {
    let keys = lua_string(&shortcut.hyprland_keys());
    let description = lua_string(BIND_DESCRIPTION);
    format!(
        "{prelude} {prune} table.insert(s.pids, {pid}) \
         if s.bind ~= nil and (s.bind.enabled == nil or s.keys ~= {keys}) then s.bind:remove() s.bind = nil end \
         if s.bind == nil then s.keys = {keys} s.bind = hl.bind({keys}, function() \
         for i = #s.pids, 1, -1 do local w = \"pid:\" .. s.pids[i] \
         if hl.get_window(w) ~= nil then hl.dispatch(hl.dsp.focus({{ window = w }})) return end end \
         end, {{ non_consuming = true, description = {description} }}) end",
        prelude = lua_prelude(true),
        prune = lua_prune(Some(pid)),
    )
}

/// The `hyprctl eval` script that forgets dialog `pid` (or, with `None`,
/// only dialogs whose process is gone) and removes the binding once no
/// dialog waits.
#[must_use]
pub fn unregister_script(pid: Option<u32>) -> String {
    format!(
        "{prelude} {prune} if #s.pids == 0 then if s.bind ~= nil then s.bind:remove() end s.bind = nil end",
        prelude = lua_prelude(false),
        prune = lua_prune(pid),
    )
}

/// Whether `hyprctl -j binds` output holds a binding of `shortcut` in the
/// default submap that is not Splinterm's. `None` when the output cannot be
/// read, which the caller treats as "do not bind".
#[must_use]
pub fn user_binding_exists(binds_json: &str, shortcut: DialogShortcut) -> Option<bool> {
    let binds: Vec<serde_json::Value> = serde_json::from_str(binds_json).ok()?;
    let key = char::from(shortcut.key).to_string();
    let mask = shortcut.hyprland_modmask();
    Some(binds.iter().any(|bind| {
        let text = |field: &str| bind.get(field).and_then(serde_json::Value::as_str);
        bind.get("modmask").and_then(serde_json::Value::as_u64) == Some(mask)
            && text("key").is_some_and(|bound| bound.eq_ignore_ascii_case(&key))
            && text("submap").is_none_or(str::is_empty)
            && text("description") != Some(BIND_DESCRIPTION)
    }))
}

/// Runs `hyprctl` and returns its standard output.
pub trait Hyprctl {
    /// `None` when Hyprland is unreachable, `hyprctl` fails, or it takes
    /// longer than allowed.
    fn run(&self, arguments: &[&str]) -> Option<String>;
}

/// The session's `hyprctl`, used only inside a Hyprland session
/// (`HYPRLAND_INSTANCE_SIGNATURE` set) and abandoned after one second.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHyprctl;

impl Hyprctl for SystemHyprctl {
    fn run(&self, arguments: &[&str]) -> Option<String> {
        if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none_or(|value| value.is_empty()) {
            return None;
        }
        let mut child = Command::new("hyprctl")
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // The reply is read on its own thread, so a large one cannot fill
        // the pipe and stall the child while this thread waits for it.
        let stdout = child.stdout.take()?;
        let reader = thread::Builder::new()
            .name("hyprctl-reader".to_owned())
            .spawn(move || {
                let mut output = String::new();
                stdout
                    .take(MAX_HYPRCTL_OUTPUT)
                    .read_to_string(&mut output)
                    .ok()
                    .map(|_| output)
            });
        let deadline = Instant::now() + HYPRCTL_TIMEOUT;
        let succeeded = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
            }
        };
        let output = reader.ok()?.join().ok()??;
        succeeded.then_some(output)
    }
}

/// What registering a dialog did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    /// The dialog is listed and the chord is bound.
    Bound,
    /// The person's configuration binds the chord; Splinterm bound nothing.
    UserBinding,
    /// The shortcut is disabled, Hyprland is unreachable, or it refused.
    Unavailable,
}

/// Registers dialog `pid` for `shortcut`, unless the person already binds it.
pub fn register_with(hyprctl: &impl Hyprctl, pid: u32, shortcut: DialogShortcut) -> Registration {
    let user_binding = hyprctl
        .run(&["-j", "binds"])
        .and_then(|binds| user_binding_exists(&binds, shortcut));
    match user_binding {
        None => Registration::Unavailable,
        Some(true) => {
            // Stale Splinterm entries still go.
            let _ = hyprctl.run(&["eval", &unregister_script(None)]);
            Registration::UserBinding
        }
        Some(false) => {
            let reply = hyprctl.run(&["eval", &register_script(pid, shortcut)]);
            if reply.as_deref().map(str::trim) == Some("ok") {
                Registration::Bound
            } else {
                Registration::Unavailable
            }
        }
    }
}

/// Forgets dialog `pid` and every dialog whose process is gone, and removes
/// the binding once none waits.
pub fn unregister_with(hyprctl: &impl Hyprctl, pid: u32) {
    let _ = hyprctl.run(&["eval", &unregister_script(Some(pid))]);
}

/// Removes dialog `pid`'s registration when dropped.
///
/// The dialog holds one for as long as its Window is open; the process that
/// spawned the dialog holds another and drops it only after reaping the
/// dialog, so the entry goes even when the dialog was killed.
#[derive(Debug)]
pub struct DialogShortcutLease<H: Hyprctl = SystemHyprctl> {
    pid: u32,
    hyprctl: H,
}

impl DialogShortcutLease<SystemHyprctl> {
    /// A lease the reaping parent holds for child `pid`.
    #[must_use]
    pub const fn for_child(pid: u32) -> Self {
        Self {
            pid,
            hyprctl: SystemHyprctl,
        }
    }

    /// Registers this process's dialog for `shortcut` (nothing when it is
    /// `None`) and returns the lease that removes it again.
    #[must_use]
    pub fn register(shortcut: Option<DialogShortcut>) -> Self {
        Self::register_with(SystemHyprctl, std::process::id(), shortcut).0
    }
}

impl<H: Hyprctl> DialogShortcutLease<H> {
    /// Registers dialog `pid` through `hyprctl` and returns the lease with
    /// what registration did.
    pub fn register_with(
        hyprctl: H,
        pid: u32,
        shortcut: Option<DialogShortcut>,
    ) -> (Self, Registration) {
        let registration = shortcut.map_or(Registration::Unavailable, |shortcut| {
            register_with(&hyprctl, pid, shortcut)
        });
        (Self { pid, hyprctl }, registration)
    }

    /// A lease for `pid` that registers nothing.
    pub const fn new(hyprctl: H, pid: u32) -> Self {
        Self { pid, hyprctl }
    }
}

impl<H: Hyprctl> Drop for DialogShortcutLease<H> {
    fn drop(&mut self) {
        unregister_with(&self.hyprctl, self.pid);
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    /// A `hyprctl` that records every call and answers from a script.
    #[derive(Clone, Default)]
    struct FakeHyprctl {
        calls: Rc<RefCell<Vec<Vec<String>>>>,
        binds: Option<String>,
        eval_reply: Option<String>,
    }

    impl FakeHyprctl {
        fn with_binds(binds: &str) -> Self {
            Self {
                binds: Some(binds.to_owned()),
                eval_reply: Some("ok".to_owned()),
                ..Self::default()
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.borrow().clone()
        }
    }

    impl Hyprctl for FakeHyprctl {
        fn run(&self, arguments: &[&str]) -> Option<String> {
            self.calls.borrow_mut().push(
                arguments
                    .iter()
                    .map(|&argument| argument.to_owned())
                    .collect(),
            );
            match arguments {
                ["-j", "binds"] => self.binds.clone(),
                ["eval", _] => self.eval_reply.clone(),
                _ => None,
            }
        }
    }

    fn eval(script: String) -> Vec<String> {
        vec!["eval".to_owned(), script]
    }

    fn binds() -> Vec<String> {
        vec!["-j".to_owned(), "binds".to_owned()]
    }

    const PRELUDE: &str = "local function alive(pid) local f = io.open(\"/proc/\" .. pid .. \"/stat\", \"r\") if f then f:close() return true end return false end";

    #[test]
    fn the_setting_parses_chords_and_none() {
        assert_eq!(
            DialogShortcut::parse_setting("ctrl+alt+g"),
            Ok(Some(DialogShortcut::default()))
        );
        assert_eq!(
            DialogShortcut::parse_setting(" Control + ALT + G "),
            Ok(Some(DialogShortcut::default()))
        );
        assert_eq!(DialogShortcut::parse_setting("none"), Ok(None));
        assert_eq!(DialogShortcut::parse_setting("NONE"), Ok(None));
        let super_seven = DialogShortcut::parse_setting("super+7").unwrap().unwrap();
        assert_eq!(super_seven.hyprland_keys(), "SUPER + 7");
        assert_eq!(super_seven.label(), "Super+7");
        assert_eq!(super_seven.hyprland_modmask(), 64);
        assert_eq!(super_seven.to_string(), "super+7");
        for invalid in [
            "",
            "g",
            "ctrl+",
            "ctrl+alt",
            "ctrl+alt+gg",
            "ctrl+alt+F1",
            "ctrl+alt+é",
            "shift+ctrl+g",
            "ctrl+ctrl+g",
            "hyper+g",
            "ctrl+alt+g+h",
            "ctrl alt g",
        ] {
            assert!(
                DialogShortcut::parse_setting(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn the_default_chord_is_ctrl_alt_g_in_every_form() {
        let chord = DialogShortcut::default();
        assert_eq!(chord.hyprland_keys(), "CTRL + ALT + G");
        assert_eq!(chord.hyprland_modmask(), 12);
        assert_eq!(chord.label(), "Ctrl+Alt+G");
        assert_eq!(chord.to_string(), "ctrl+alt+g");
    }

    #[test]
    fn only_the_exact_chord_matches_a_key_press() {
        let chord = DialogShortcut::default();
        assert!(chord.matches(true, true, false, false, 'g'));
        assert!(chord.matches(true, true, false, false, 'G'), "caps lock");
        assert!(!chord.matches(false, false, false, false, 'g'));
        assert!(!chord.matches(true, false, false, false, 'g'));
        assert!(!chord.matches(false, true, false, false, 'g'));
        assert!(!chord.matches(true, true, true, false, 'g'), "shift");
        assert!(!chord.matches(true, true, false, true, 'g'), "super");
        assert!(!chord.matches(true, true, false, false, 'd'));
        assert!(!chord.matches(true, true, false, false, 'ĝ'));
    }

    #[test]
    fn the_register_script_lists_the_dialog_and_binds_a_non_consuming_focus() {
        assert_eq!(
            register_script(4242, DialogShortcut::default()),
            format!(
                "local s = rawget(_G, \"splinterm_trusted_dialog_shortcut\") if type(s) ~= \"table\" then s = {{ pids = {{}} }} rawset(_G, \"splinterm_trusted_dialog_shortcut\", s) end \
                 {PRELUDE} \
                 for i = #s.pids, 1, -1 do if s.pids[i] == 4242 or not alive(s.pids[i]) then table.remove(s.pids, i) end end \
                 table.insert(s.pids, 4242) \
                 if s.bind ~= nil and (s.bind.enabled == nil or s.keys ~= \"CTRL + ALT + G\") then s.bind:remove() s.bind = nil end \
                 if s.bind == nil then s.keys = \"CTRL + ALT + G\" s.bind = hl.bind(\"CTRL + ALT + G\", function() \
                 for i = #s.pids, 1, -1 do local w = \"pid:\" .. s.pids[i] \
                 if hl.get_window(w) ~= nil then hl.dispatch(hl.dsp.focus({{ window = w }})) return end end \
                 end, {{ non_consuming = true, description = \"Splinterm: focus the waiting trusted dialog\" }}) end"
            )
        );
    }

    #[test]
    fn the_unregister_script_forgets_the_dialog_prunes_and_unbinds_when_empty() {
        let tail =
            "if #s.pids == 0 then if s.bind ~= nil then s.bind:remove() end s.bind = nil end";
        assert_eq!(
            unregister_script(Some(4242)),
            format!(
                "local s = rawget(_G, \"splinterm_trusted_dialog_shortcut\") if type(s) ~= \"table\" then return end \
                 {PRELUDE} \
                 for i = #s.pids, 1, -1 do if s.pids[i] == 4242 or not alive(s.pids[i]) then table.remove(s.pids, i) end end \
                 {tail}"
            )
        );
        assert_eq!(
            unregister_script(None),
            format!(
                "local s = rawget(_G, \"splinterm_trusted_dialog_shortcut\") if type(s) ~= \"table\" then return end \
                 {PRELUDE} \
                 for i = #s.pids, 1, -1 do if not alive(s.pids[i]) then table.remove(s.pids, i) end end \
                 {tail}"
            )
        );
    }

    #[test]
    fn scripts_are_one_line_and_lua_strings_are_escaped() {
        for script in [
            register_script(1, DialogShortcut::default()),
            unregister_script(Some(1)),
            unregister_script(None),
        ] {
            assert!(!script.contains('\n'), "{script}");
        }
        assert_eq!(lua_string("a\"b\\c\nd\u{7}"), "\"a\\\"b\\\\c\\nd\"");
    }

    #[test]
    fn a_user_binding_of_the_chord_is_detected_and_splinterms_own_is_not() {
        let chord = DialogShortcut::default();
        let bind = |modmask: u64, key: &str, submap: &str, description: &str| {
            format!(
                r#"[{{"modmask": {modmask}, "submap": "{submap}", "key": "{key}", "keycode": 0, "description": "{description}", "dispatcher": "__lua", "arg": "7"}}]"#
            )
        };
        assert_eq!(
            user_binding_exists(&bind(12, "G", "", ""), chord),
            Some(true)
        );
        assert_eq!(
            user_binding_exists(&bind(12, "g", "", "Mine"), chord),
            Some(true)
        );
        assert_eq!(
            user_binding_exists(&bind(12, "G", "", BIND_DESCRIPTION), chord),
            Some(false),
            "Splinterm's own binding is not the person's"
        );
        assert_eq!(
            user_binding_exists(&bind(4, "G", "", ""), chord),
            Some(false)
        );
        assert_eq!(
            user_binding_exists(&bind(76, "G", "", ""), chord),
            Some(false)
        );
        assert_eq!(
            user_binding_exists(&bind(12, "H", "", ""), chord),
            Some(false)
        );
        assert_eq!(
            user_binding_exists(&bind(12, "G", "resize", ""), chord),
            Some(false),
            "another submap's binding is not active with the default one"
        );
        assert_eq!(user_binding_exists("[]", chord), Some(false));
        assert_eq!(user_binding_exists("not json", chord), None);
    }

    #[test]
    fn registering_checks_for_a_user_binding_then_binds() {
        let hyprctl = FakeHyprctl::with_binds("[]");
        let chord = DialogShortcut::default();
        assert_eq!(register_with(&hyprctl, 77, chord), Registration::Bound);
        assert_eq!(
            hyprctl.calls(),
            vec![binds(), eval(register_script(77, chord))]
        );
    }

    #[test]
    fn a_user_binding_wins_and_only_stale_entries_are_cleaned() {
        let hyprctl = FakeHyprctl::with_binds(
            r#"[{"modmask": 12, "submap": "", "key": "G", "description": ""}]"#,
        );
        assert_eq!(
            register_with(&hyprctl, 77, DialogShortcut::default()),
            Registration::UserBinding
        );
        assert_eq!(
            hyprctl.calls(),
            vec![binds(), eval(unregister_script(None))]
        );
    }

    #[test]
    fn nothing_is_bound_when_the_binds_are_unreadable_or_hyprland_refuses() {
        let unreachable = FakeHyprctl::default();
        assert_eq!(
            register_with(&unreachable, 77, DialogShortcut::default()),
            Registration::Unavailable
        );
        assert_eq!(unreachable.calls(), vec![binds()]);
        let refusing = FakeHyprctl {
            eval_reply: Some("error: no".to_owned()),
            ..FakeHyprctl::with_binds("[]")
        };
        assert_eq!(
            register_with(&refusing, 77, DialogShortcut::default()),
            Registration::Unavailable
        );
    }

    #[test]
    fn a_lease_registers_on_creation_and_unregisters_on_drop() {
        let hyprctl = FakeHyprctl::with_binds("[]");
        let chord = DialogShortcut::default();
        let (lease, registration) =
            DialogShortcutLease::register_with(hyprctl.clone(), 9, Some(chord));
        assert_eq!(registration, Registration::Bound);
        assert_eq!(hyprctl.calls().len(), 2);
        drop(lease);
        assert_eq!(
            hyprctl.calls(),
            vec![
                binds(),
                eval(register_script(9, chord)),
                eval(unregister_script(Some(9)))
            ]
        );
    }

    #[test]
    fn a_disabled_shortcut_binds_nothing_but_still_cleans_up() {
        let hyprctl = FakeHyprctl::with_binds("[]");
        let (lease, registration) = DialogShortcutLease::register_with(hyprctl.clone(), 9, None);
        assert_eq!(registration, Registration::Unavailable);
        assert!(hyprctl.calls().is_empty());
        drop(lease);
        assert_eq!(hyprctl.calls(), vec![eval(unregister_script(Some(9)))]);
    }

    #[test]
    fn a_parent_lease_unregisters_its_child_on_drop() {
        let hyprctl = FakeHyprctl::default();
        drop(DialogShortcutLease::new(hyprctl.clone(), 31337));
        assert_eq!(hyprctl.calls(), vec![eval(unregister_script(Some(31337)))]);
    }

    #[test]
    fn outside_hyprland_the_system_hyprctl_never_runs() {
        if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
            assert_eq!(SystemHyprctl.run(&["version"]), None);
        }
    }
}
