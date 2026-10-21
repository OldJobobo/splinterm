//! The keyboard path to a waiting trusted dialog (Plan 0053 item 9).
//!
//! Trusted dialogs map without keyboard focus and stay inert until the
//! person engages them. The dialog shortcut, `Ctrl+Alt+G` unless
//! `[integration] dialog-shortcut` names another `Ctrl+Alt` chord, engages
//! one from the keyboard the way a click inside does. It never answers: the
//! dialog then arms for its usual delay before `G`, `D`, `Escape` or `Enter`
//! count. No Super chord is ever used (operator decision, 2026-10-20).
//!
//! Inside a dialog that already has keyboard focus, the chord engages it on
//! any compositor. When Hyprland maps trusted dialogs without focus (the
//! Omarchy integration's dialog rule; the client checks), Splinterm also
//! enables a binding of the chord for exactly as long as a dialog waits, so
//! it reaches the dialog from any other Window:
//!
//! * The dialog process registers its own PID in the background while it
//!   maps ([`BackgroundRegistration`]). Registration adds the PID to a list
//!   kept in Hyprland's Lua state and enables Splinterm's binding of the
//!   chord, creating it with [`BIND_DESCRIPTION`] the first time.
//! * The binding is non-consuming and only focuses the newest listed PID
//!   whose mapped Window has the trusted dialog app ID. The chord then
//!   reaches that dialog, which engages because it now has focus and sees
//!   the exact chord. When no listed dialog has such a Window, the chord
//!   reaches the focused application unchanged, so a stale entry can neither
//!   take the chord from it nor focus another program that reused the PID.
//! * The PID leaves the list when the dialog has answered, and again when
//!   the process that spawned and reaped it drops its lease, so a killed
//!   dialog cannot leave its entry behind. Every registration and removal
//!   also drops PIDs whose process is gone. With the last entry the binding
//!   is disabled, never removed: Hyprland's removal takes every binding of
//!   the same chord in every submap, the person's own among them. A binding
//!   object that Hyprland has already dropped is only ever recognized by its
//!   string form, because indexing one or calling its methods crashes
//!   Hyprland 0.56. `hyprctl reload` clears the binding with the rest of the
//!   Lua state.
//! * When the person's own configuration binds the chord in any submap,
//!   Splinterm enables nothing and their binding keeps working.
//!
//! This module lives in `splinterd` because the daemon reaps the access
//! consent dialog and the client reaps the upgrade and restart questions;
//! both drop a lease after reaping.

use std::{
    env, fmt,
    io::Read,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// The Wayland app ID of every trusted dialog (access consent, upgrade,
/// restart). The binding focuses only Windows with this app ID.
pub const TRUSTED_DIALOG_APP_ID: &str = "com.oldjobobo.splinterm.trusted-dialog";

/// The description Splinterm's binding carries in `hyprctl binds`, which
/// tells it apart from the person's own bindings of the same chord.
pub const BIND_DESCRIPTION: &str = "Splinterm: focus the waiting trusted dialog";

/// The Lua global that holds the waiting dialogs' PIDs and the bindings.
const LUA_STATE: &str = "splinterm_trusted_dialog_shortcut";

/// How long one `hyprctl` call, its reply included, may take before it is
/// abandoned.
const HYPRCTL_TIMEOUT: Duration = Duration::from_secs(1);

/// The largest `hyprctl` reply read; a bind list is far smaller.
const MAX_HYPRCTL_OUTPUT: u64 = 4 * 1024 * 1024;

/// Hyprland's modifier mask for Ctrl and Alt, as `hyprctl -j binds`
/// reports it (Ctrl is bit 2, Alt bit 3).
const HYPRLAND_CTRL_ALT: u64 = (1 << 2) | (1 << 3);

/// A dialog shortcut chord: Ctrl and Alt with one letter or digit. Shift is
/// not allowed, because it changes the key a letter or digit produces, and
/// Super is not, by operator decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogShortcut {
    /// Lowercase ASCII letter or digit.
    key: u8,
}

impl Default for DialogShortcut {
    /// `Ctrl+Alt+G`.
    fn default() -> Self {
        Self { key: b'g' }
    }
}

impl DialogShortcut {
    /// Parses an `[integration] dialog-shortcut` value: `none` disables the
    /// shortcut (`Ok(None)`); otherwise `ctrl+alt+` and one letter or digit,
    /// such as `ctrl+alt+g`, in any case and modifier order.
    ///
    /// # Errors
    /// Returns a reason when the value is not a valid chord.
    pub fn parse_setting(value: &str) -> Result<Option<Self>, String> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        let parts: Vec<&str> = value.split('+').map(str::trim).collect();
        let Some((key, modifiers)) = parts.split_last() else {
            return Err("expected ctrl+alt+ and a key, such as ctrl+alt+g".to_owned());
        };
        let (mut ctrl, mut alt) = (false, false);
        for modifier in modifiers {
            let slot = match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut ctrl,
                "alt" => &mut alt,
                "super" | "logo" | "meta" | "win" => {
                    return Err("Super chords are not allowed; use ctrl+alt+ and a key".to_owned());
                }
                "shift" => return Err("shift is not allowed; use ctrl+alt+ and a key".to_owned()),
                other => return Err(format!("unknown modifier {other:?}")),
            };
            if std::mem::replace(slot, true) {
                return Err(format!("modifier {modifier:?} appears twice"));
            }
        }
        if !(ctrl && alt) {
            return Err("a dialog shortcut needs both ctrl and alt, such as ctrl+alt+g".to_owned());
        }
        match key.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => Ok(Some(Self {
                key: byte.to_ascii_lowercase(),
            })),
            _ => Err(format!("the key must be one letter or digit, not {key:?}")),
        }
    }

    fn key_label(self) -> char {
        char::from(self.key.to_ascii_uppercase())
    }

    /// The chord as a Hyprland key string, such as `CTRL + ALT + G`.
    #[must_use]
    pub fn hyprland_keys(self) -> String {
        format!("CTRL + ALT + {}", self.key_label())
    }

    /// The chord's Hyprland modifier mask: Ctrl and Alt.
    #[must_use]
    pub const fn hyprland_modmask(self) -> u64 {
        HYPRLAND_CTRL_ALT
    }

    /// The chord as the dialogs name it, such as `Ctrl+Alt+G`.
    #[must_use]
    pub fn label(self) -> String {
        format!("Ctrl+Alt+{}", self.key_label())
    }

    /// Whether one key press is exactly this chord: Ctrl and Alt held,
    /// neither Shift nor Super, and the chord's key. `key` is the pressed
    /// keysym's ASCII character, when it has one.
    #[must_use]
    #[allow(
        clippy::fn_params_excessive_bools,
        reason = "one flag per modifier the keyboard reports"
    )]
    pub fn matches(self, ctrl: bool, alt: bool, shift: bool, super_key: bool, key: char) -> bool {
        ctrl && alt
            && !shift
            && !super_key
            && key.is_ascii()
            && key.to_ascii_lowercase() == char::from(self.key)
    }
}

impl fmt::Display for DialogShortcut {
    /// The `config.ini` form, such as `ctrl+alt+g`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ctrl+alt+{}", char::from(self.key))
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
/// and defines `alive`, whether a PID's process still exists, and `live`,
/// whether a binding object still names a binding.
///
/// `live` uses only the object's string form. Hyprland 0.56 checks for an
/// expired binding there, but indexing an expired one or calling its
/// methods dereferences a null pointer and crashes the compositor.
fn lua_prelude(create: bool) -> String {
    let state = lua_string(LUA_STATE);
    let load = if create {
        format!(
            "local s = rawget(_G, {state}) if type(s) ~= \"table\" or type(s.pids) ~= \"table\" or type(s.binds) ~= \"table\" then s = {{ pids = {{}}, binds = {{}} }} rawset(_G, {state}, s) end"
        )
    } else {
        format!(
            "local s = rawget(_G, {state}) if type(s) ~= \"table\" or type(s.pids) ~= \"table\" or type(s.binds) ~= \"table\" then return end"
        )
    };
    format!(
        "{load} local function alive(pid) local f = io.open(\"/proc/\" .. pid .. \"/stat\", \"r\") if f then f:close() return true end return false end \
         local function live(b) return b ~= nil and tostring(b) ~= \"HL.Keybind(expired)\" end"
    )
}

/// Lua that drops `pid` (when given) and every PID whose process is gone.
fn lua_prune(pid: Option<u32>) -> String {
    let matches = pid.map_or_else(String::new, |pid| format!("s.pids[i] == {pid} or "));
    format!(
        "for i = #s.pids, 1, -1 do if {matches}not alive(s.pids[i]) then table.remove(s.pids, i) end end"
    )
}

/// The `hyprctl eval` script that registers dialog `pid` and enables
/// Splinterm's binding of `shortcut`, creating it the first time.
///
/// Bindings of other chords Splinterm made earlier are disabled. The binding
/// is non-consuming: it focuses the newest listed PID whose mapped Window is
/// a trusted dialog and lets the chord through to it, or through to the
/// focused application when there is none.
#[must_use]
pub fn register_script(pid: u32, shortcut: DialogShortcut) -> String {
    let keys = lua_string(&shortcut.hyprland_keys());
    let description = lua_string(BIND_DESCRIPTION);
    let app_id = lua_string(TRUSTED_DIALOG_APP_ID);
    format!(
        "{prelude} {prune} table.insert(s.pids, {pid}) \
         for k, b in pairs(s.binds) do if k ~= {keys} and live(b) then b:set_enabled(false) end end \
         if live(s.binds[{keys}]) then s.binds[{keys}]:set_enabled(true) \
         else s.binds[{keys}] = hl.bind({keys}, function() \
         for i = #s.pids, 1, -1 do local w = \"pid:\" .. s.pids[i] local win = hl.get_window(w) \
         if win ~= nil and win.class == {app_id} then hl.dispatch(hl.dsp.focus({{ window = w }})) return end end \
         end, {{ non_consuming = true, description = {description} }}) end",
        prelude = lua_prelude(true),
        prune = lua_prune(Some(pid)),
    )
}

/// The `hyprctl eval` script that forgets dialog `pid` (or, with `None`,
/// only dialogs whose process is gone) and disables Splinterm's bindings
/// once no dialog waits. It never removes a binding.
#[must_use]
pub fn unregister_script(pid: Option<u32>) -> String {
    format!(
        "{prelude} {prune} if #s.pids == 0 then for _, b in pairs(s.binds) do if live(b) then b:set_enabled(false) end end end",
        prelude = lua_prelude(false),
        prune = lua_prune(pid),
    )
}

/// Whether `hyprctl -j binds` output holds a binding of `shortcut`, in any
/// submap, that is not Splinterm's. `None` when the output cannot be read,
/// which the caller treats as "do not bind".
#[must_use]
pub fn user_binding_exists(binds_json: &str, shortcut: DialogShortcut) -> Option<bool> {
    let binds: Vec<serde_json::Value> = serde_json::from_str(binds_json).ok()?;
    let key = char::from(shortcut.key).to_string();
    let mask = shortcut.hyprland_modmask();
    Some(binds.iter().any(|bind| {
        let text = |field: &str| bind.get(field).and_then(serde_json::Value::as_str);
        bind.get("modmask").and_then(serde_json::Value::as_u64) == Some(mask)
            && text("key").is_some_and(|bound| bound.eq_ignore_ascii_case(&key))
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
/// (`HYPRLAND_INSTANCE_SIGNATURE` set) and abandoned, reply included, after
/// one second.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHyprctl;

impl Hyprctl for SystemHyprctl {
    fn run(&self, arguments: &[&str]) -> Option<String> {
        if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none_or(|value| value.is_empty()) {
            return None;
        }
        let deadline = Instant::now() + HYPRCTL_TIMEOUT;
        let mut child = Command::new("hyprctl")
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // The reply is read on its own thread, so a large one cannot fill
        // the pipe and stall the child while this thread waits for it, and
        // a reply that never ends is abandoned at the deadline.
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        };
        let (sender, reply) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("hyprctl-reader".to_owned())
            .spawn(move || {
                let mut output = String::new();
                let read = stdout
                    .take(MAX_HYPRCTL_OUTPUT)
                    .read_to_string(&mut output)
                    .ok()
                    .map(|_| output);
                let _ = sender.send(read);
            });
        if reader.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
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
        // Whatever still holds the pipe, the reply waits no longer than the
        // deadline; the reader thread then ends on its own.
        let output = reply
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()??;
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

/// Forgets dialog `pid` and every dialog whose process is gone, and disables
/// the binding once none waits.
pub fn unregister_with(hyprctl: &impl Hyprctl, pid: u32) {
    let _ = hyprctl.run(&["eval", &unregister_script(Some(pid))]);
}

/// Removes dialog `pid`'s registration when dropped.
///
/// The process that spawned a dialog holds one and drops it only after
/// reaping the dialog, so the entry goes even when the dialog was killed.
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
}

impl<H: Hyprctl> DialogShortcutLease<H> {
    /// A lease for `pid` that removes its registration through `hyprctl`.
    pub const fn new(hyprctl: H, pid: u32) -> Self {
        Self { pid, hyprctl }
    }

    /// The PID this lease removes.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }
}

impl<H: Hyprctl + Send + 'static> DialogShortcutLease<H> {
    /// Removes the registration on a detached thread, so a caller that has
    /// already reaped the dialog does not wait for `hyprctl`.
    pub fn release_in_background(self) {
        // Without a thread, the removal runs here instead.
        let _ = thread::Builder::new()
            .name("dialog-shortcut-release".to_owned())
            .spawn(move || drop(self));
    }
}

impl<H: Hyprctl> Drop for DialogShortcutLease<H> {
    fn drop(&mut self) {
        unregister_with(&self.hyprctl, self.pid);
    }
}

/// A dialog's own registration, made on a background thread while its
/// Window maps, so registering never delays the dialog; until it is done,
/// a click still engages it.
#[derive(Debug)]
pub struct BackgroundRegistration<H: Hyprctl + Send + 'static = SystemHyprctl> {
    registering: Option<thread::JoinHandle<DialogShortcutLease<H>>>,
}

impl BackgroundRegistration<SystemHyprctl> {
    /// Registers this process's dialog for `shortcut` in the background;
    /// nothing when it is `None`.
    #[must_use]
    pub fn start(shortcut: Option<DialogShortcut>) -> Self {
        Self::start_with(SystemHyprctl, std::process::id(), shortcut)
    }
}

impl<H: Hyprctl + Send + 'static> BackgroundRegistration<H> {
    /// Registers dialog `pid` for `shortcut` through `hyprctl` in the
    /// background; nothing when it is `None`.
    #[must_use]
    pub fn start_with(hyprctl: H, pid: u32, shortcut: Option<DialogShortcut>) -> Self {
        let registering = shortcut.and_then(|shortcut| {
            thread::Builder::new()
                .name("dialog-shortcut-register".to_owned())
                .spawn(move || {
                    let _ = register_with(&hyprctl, pid, shortcut);
                    DialogShortcutLease::new(hyprctl, pid)
                })
                .ok()
        });
        Self { registering }
    }

    /// Ends the registration once the dialog has answered. A finished
    /// registration is removed now. One still running is never waited for:
    /// it removes itself when it ends, unless this process exits first, and
    /// the process that reaps this one removes it in any case.
    pub fn finish(mut self) {
        self.end();
    }

    fn end(&mut self) {
        let Some(registering) = self.registering.take() else {
            return;
        };
        if registering.is_finished()
            && let Ok(lease) = registering.join()
        {
            drop(lease);
        }
    }
}

impl<H: Hyprctl + Send + 'static> Drop for BackgroundRegistration<H> {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A `hyprctl` that records every call and answers from a script.
    #[derive(Clone, Default)]
    struct FakeHyprctl {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        binds: Option<String>,
        eval_reply: Option<String>,
        /// Delays every call, to model a slow compositor.
        delay: Option<Duration>,
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
            self.calls.lock().unwrap().clone()
        }
    }

    impl Hyprctl for FakeHyprctl {
        fn run(&self, arguments: &[&str]) -> Option<String> {
            if let Some(delay) = self.delay {
                thread::sleep(delay);
            }
            self.calls.lock().unwrap().push(
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

    fn wait_for_calls(hyprctl: &FakeHyprctl, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while hyprctl.calls().len() < count && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }

    const LOAD_CREATE: &str = "local s = rawget(_G, \"splinterm_trusted_dialog_shortcut\") if type(s) ~= \"table\" or type(s.pids) ~= \"table\" or type(s.binds) ~= \"table\" then s = { pids = {}, binds = {} } rawset(_G, \"splinterm_trusted_dialog_shortcut\", s) end";
    const LOAD_EXISTING: &str = "local s = rawget(_G, \"splinterm_trusted_dialog_shortcut\") if type(s) ~= \"table\" or type(s.pids) ~= \"table\" or type(s.binds) ~= \"table\" then return end";
    const HELPERS: &str = "local function alive(pid) local f = io.open(\"/proc/\" .. pid .. \"/stat\", \"r\") if f then f:close() return true end return false end \
         local function live(b) return b ~= nil and tostring(b) ~= \"HL.Keybind(expired)\" end";

    #[test]
    fn the_setting_takes_only_ctrl_alt_chords_and_none() {
        assert_eq!(
            DialogShortcut::parse_setting("ctrl+alt+g"),
            Ok(Some(DialogShortcut::default()))
        );
        assert_eq!(
            DialogShortcut::parse_setting(" ALT + Control + G "),
            Ok(Some(DialogShortcut::default()))
        );
        assert_eq!(DialogShortcut::parse_setting("none"), Ok(None));
        assert_eq!(DialogShortcut::parse_setting("NONE"), Ok(None));
        let seven = DialogShortcut::parse_setting("ctrl+alt+7")
            .unwrap()
            .unwrap();
        assert_eq!(seven.hyprland_keys(), "CTRL + ALT + 7");
        assert_eq!(seven.label(), "Ctrl+Alt+7");
        assert_eq!(seven.hyprland_modmask(), 12);
        assert_eq!(seven.to_string(), "ctrl+alt+7");
        for invalid in [
            "",
            "g",
            "ctrl+c",
            "alt+g",
            "ctrl+g",
            "super+g",
            "super+7",
            "ctrl+alt+super+g",
            "logo+ctrl+alt+g",
            "shift+ctrl+alt+g",
            "ctrl+alt+shift+g",
            "ctrl+",
            "ctrl+alt",
            "ctrl+alt+",
            "ctrl+alt+gg",
            "ctrl+alt+F1",
            "ctrl+alt+é",
            "ctrl+ctrl+alt+g",
            "hyper+ctrl+alt+g",
            "ctrl+alt+g+h",
            "ctrl alt g",
        ] {
            assert!(
                DialogShortcut::parse_setting(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
        assert!(
            DialogShortcut::parse_setting("super+g")
                .unwrap_err()
                .contains("Super chords are not allowed")
        );
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
    fn the_register_script_lists_the_dialog_and_enables_one_non_consuming_focus() {
        assert_eq!(
            register_script(4242, DialogShortcut::default()),
            format!(
                "{LOAD_CREATE} {HELPERS} \
                 for i = #s.pids, 1, -1 do if s.pids[i] == 4242 or not alive(s.pids[i]) then table.remove(s.pids, i) end end \
                 table.insert(s.pids, 4242) \
                 for k, b in pairs(s.binds) do if k ~= \"CTRL + ALT + G\" and live(b) then b:set_enabled(false) end end \
                 if live(s.binds[\"CTRL + ALT + G\"]) then s.binds[\"CTRL + ALT + G\"]:set_enabled(true) \
                 else s.binds[\"CTRL + ALT + G\"] = hl.bind(\"CTRL + ALT + G\", function() \
                 for i = #s.pids, 1, -1 do local w = \"pid:\" .. s.pids[i] local win = hl.get_window(w) \
                 if win ~= nil and win.class == \"com.oldjobobo.splinterm.trusted-dialog\" then hl.dispatch(hl.dsp.focus({{ window = w }})) return end end \
                 end, {{ non_consuming = true, description = \"Splinterm: focus the waiting trusted dialog\" }}) end"
            )
        );
    }

    #[test]
    fn the_unregister_script_forgets_the_dialog_prunes_and_disables_when_empty() {
        let tail = "if #s.pids == 0 then for _, b in pairs(s.binds) do if live(b) then b:set_enabled(false) end end end";
        assert_eq!(
            unregister_script(Some(4242)),
            format!(
                "{LOAD_EXISTING} {HELPERS} \
                 for i = #s.pids, 1, -1 do if s.pids[i] == 4242 or not alive(s.pids[i]) then table.remove(s.pids, i) end end \
                 {tail}"
            )
        );
        assert_eq!(
            unregister_script(None),
            format!(
                "{LOAD_EXISTING} {HELPERS} \
                 for i = #s.pids, 1, -1 do if not alive(s.pids[i]) then table.remove(s.pids, i) end end \
                 {tail}"
            )
        );
    }

    /// F1: Hyprland 0.56 crashes when an expired binding object is indexed
    /// or has a method called. Every use of a stored binding goes through
    /// `live`, which uses only `tostring`, and nothing is ever removed.
    #[test]
    fn scripts_never_touch_a_binding_object_that_may_have_expired() {
        let register = register_script(1, DialogShortcut::default());
        let unregister = unregister_script(Some(1));
        for script in [&register, &unregister] {
            assert!(!script.contains(":remove("), "{script}");
            assert!(!script.contains(":unbind("), "{script}");
            assert!(!script.contains("hl.unbind"), "{script}");
            assert!(!script.contains(".enabled"), "{script}");
            assert!(!script.contains("s.bind "), "{script}");
            assert!(!script.contains("s.bind."), "{script}");
        }
        // Each method call on a stored binding is guarded by `live` in the
        // same statement.
        assert_eq!(register.matches(":set_enabled(").count(), 2);
        assert!(
            register.contains("if k ~= \"CTRL + ALT + G\" and live(b) then b:set_enabled(false)")
        );
        assert!(register.contains(
            "if live(s.binds[\"CTRL + ALT + G\"]) then s.binds[\"CTRL + ALT + G\"]:set_enabled(true)"
        ));
        assert_eq!(unregister.matches(":set_enabled(").count(), 1);
        assert!(unregister.contains("if live(b) then b:set_enabled(false)"));
        assert!(HELPERS.contains("tostring(b) ~= \"HL.Keybind(expired)\""));
    }

    /// F3: the binding focuses only a trusted dialog, never another program
    /// that reused a stale PID.
    #[test]
    fn the_binding_focuses_only_windows_with_the_trusted_dialog_app_id() {
        let script = register_script(1, DialogShortcut::default());
        let check = "local win = hl.get_window(w) if win ~= nil and win.class == \"com.oldjobobo.splinterm.trusted-dialog\" then hl.dispatch(";
        assert!(script.contains(check), "{script}");
        assert_eq!(script.matches("hl.dispatch(").count(), 1);
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
    fn a_user_binding_of_the_chord_in_any_submap_is_detected_and_splinterms_own_is_not() {
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
            user_binding_exists(&bind(12, "G", "resize", ""), chord),
            Some(true),
            "a binding in another submap is the person's too"
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
            r#"[{"modmask": 12, "submap": "resize", "key": "G", "description": ""}]"#,
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
    fn a_finished_background_registration_is_removed_when_the_dialog_answers() {
        let hyprctl = FakeHyprctl::with_binds("[]");
        let chord = DialogShortcut::default();
        let registration = BackgroundRegistration::start_with(hyprctl.clone(), 9, Some(chord));
        wait_for_calls(&hyprctl, 2);
        // The registering thread may not have returned yet.
        thread::sleep(Duration::from_millis(50));
        registration.finish();
        assert_eq!(
            hyprctl.calls(),
            vec![
                binds(),
                eval(register_script(9, chord)),
                eval(unregister_script(Some(9)))
            ]
        );
    }

    /// F5: the answer never waits for a slow registration; the reaping
    /// parent's lease removes it instead.
    #[test]
    fn an_unfinished_background_registration_never_delays_the_answer() {
        let hyprctl = FakeHyprctl {
            delay: Some(Duration::from_millis(300)),
            ..FakeHyprctl::with_binds("[]")
        };
        let started = Instant::now();
        let registration =
            BackgroundRegistration::start_with(hyprctl.clone(), 9, Some(DialogShortcut::default()));
        registration.finish();
        assert!(started.elapsed() < Duration::from_millis(250));
        // A dialog exits right after answering, ending the thread, and its
        // reaper removes the entry; a process that lives on removes it when
        // the detached registration ends.
        wait_for_calls(&hyprctl, 3);
        assert_eq!(
            hyprctl.calls().last(),
            Some(&eval(unregister_script(Some(9))))
        );
    }

    #[test]
    fn a_disabled_shortcut_registers_nothing() {
        let hyprctl = FakeHyprctl::with_binds("[]");
        BackgroundRegistration::start_with(hyprctl.clone(), 9, None).finish();
        assert!(hyprctl.calls().is_empty());
    }

    #[test]
    fn a_parent_lease_unregisters_its_child_on_drop_or_in_the_background() {
        let hyprctl = FakeHyprctl::default();
        let lease = DialogShortcutLease::new(hyprctl.clone(), 31337);
        assert_eq!(lease.pid(), 31337);
        drop(lease);
        assert_eq!(hyprctl.calls(), vec![eval(unregister_script(Some(31337)))]);
        let background = FakeHyprctl::default();
        DialogShortcutLease::new(background.clone(), 5).release_in_background();
        wait_for_calls(&background, 1);
        assert_eq!(background.calls(), vec![eval(unregister_script(Some(5)))]);
    }

    #[test]
    fn outside_hyprland_the_system_hyprctl_never_runs() {
        if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
            assert_eq!(SystemHyprctl.run(&["version"]), None);
        }
    }
}
