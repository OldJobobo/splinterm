//! Closed application actions and strictly resolved keymaps.
//!
//! Configuration selects from this action vocabulary; it cannot register
//! callbacks, shell snippets, or other executable behavior.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const BUILT_IN_PROFILE_NAMES: &[&str] = &["splinterm", "omarchy-tmux"];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ActionId {
    CommandPalette,
    RecentSessions,
    NewSession,
    RenameCurrentTab,
    NewDojo,
    PreviousDojo,
    NextDojo,
    CloseCurrentTab,
    CloseOtherTabs,
    TerminateCurrentDojo,
    DojoChooser,
    SelectDojo1,
    SelectDojo2,
    SelectDojo3,
    SelectDojo4,
    SelectDojo5,
    SelectDojo6,
    SelectDojo7,
    SelectDojo8,
    SelectDojo9,
    MoveDojoLeft,
    MoveDojoRight,
    RenameCurrentLair,
    SaveCurrentLair,
    ToggleCurrentLairPin,
    PreviewCurrentLair,
    RestoreCurrentLair,
    TerminateCurrentLair,
    PreviousLair,
    NextLair,
    LairChooser,
    DetachWindow,
    SplitHorizontal,
    SplitVertical,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CloseFocusedPane,
    ResizePaneSmaller,
    ResizePaneLarger,
    ResizePaneLeftFive,
    ResizePaneRightFive,
    ResizePaneUpFive,
    ResizePaneDownFive,
    TogglePaneZoom,
    ToggleTabStrip,
    BindingHelp,
    CopyModeEnter,
    ConfigReload,
    SendPrefix,
    SearchScrollback,
    PageUp,
    PageDown,
    ReturnToLive,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    RequestControl,
    ReleaseControl,
    ForceControl,
    RevokeAllAccess,
    AcceptControlTransfer,
    DenyControlTransfer,
    ClipboardCopy,
    ClipboardPaste,
}

impl ActionId {
    pub const BINDABLE: &[Self] = &[
        Self::CommandPalette,
        Self::RecentSessions,
        Self::NewDojo,
        Self::PreviousDojo,
        Self::NextDojo,
        Self::CloseCurrentTab,
        Self::CloseOtherTabs,
        Self::RenameCurrentTab,
        Self::TerminateCurrentDojo,
        Self::DojoChooser,
        Self::SelectDojo1,
        Self::SelectDojo2,
        Self::SelectDojo3,
        Self::SelectDojo4,
        Self::SelectDojo5,
        Self::SelectDojo6,
        Self::SelectDojo7,
        Self::SelectDojo8,
        Self::SelectDojo9,
        Self::MoveDojoLeft,
        Self::MoveDojoRight,
        Self::NewSession,
        Self::RenameCurrentLair,
        Self::SaveCurrentLair,
        Self::ToggleCurrentLairPin,
        Self::PreviewCurrentLair,
        Self::RestoreCurrentLair,
        Self::TerminateCurrentLair,
        Self::PreviousLair,
        Self::NextLair,
        Self::LairChooser,
        Self::DetachWindow,
        Self::SplitHorizontal,
        Self::SplitVertical,
        Self::FocusLeft,
        Self::FocusRight,
        Self::FocusUp,
        Self::FocusDown,
        Self::CloseFocusedPane,
        Self::ResizePaneSmaller,
        Self::ResizePaneLarger,
        Self::ResizePaneLeftFive,
        Self::ResizePaneRightFive,
        Self::ResizePaneUpFive,
        Self::ResizePaneDownFive,
        Self::TogglePaneZoom,
        Self::ToggleTabStrip,
        Self::BindingHelp,
        Self::CopyModeEnter,
        Self::ConfigReload,
        Self::SendPrefix,
        Self::SearchScrollback,
        Self::PageUp,
        Self::PageDown,
        Self::ReturnToLive,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ResetZoom,
        Self::RequestControl,
        Self::ReleaseControl,
        Self::ForceControl,
        Self::RevokeAllAccess,
        Self::AcceptControlTransfer,
        Self::DenyControlTransfer,
        Self::ClipboardCopy,
        Self::ClipboardPaste,
    ];

    #[must_use]
    pub const fn config_name(self) -> &'static str {
        match self {
            Self::CommandPalette => "app.command-palette",
            Self::RecentSessions => "session.recent",
            Self::NewSession => "lair.new",
            Self::RenameCurrentTab => "dojo.rename",
            Self::NewDojo => "dojo.new",
            Self::PreviousDojo => "dojo.previous",
            Self::NextDojo => "dojo.next",
            Self::CloseCurrentTab => "dojo.close-tab",
            Self::CloseOtherTabs => "dojo.close-other-tabs",
            Self::TerminateCurrentDojo => "dojo.terminate-confirmed",
            Self::DojoChooser => "dojo.choose",
            Self::SelectDojo1 => "dojo.select-1",
            Self::SelectDojo2 => "dojo.select-2",
            Self::SelectDojo3 => "dojo.select-3",
            Self::SelectDojo4 => "dojo.select-4",
            Self::SelectDojo5 => "dojo.select-5",
            Self::SelectDojo6 => "dojo.select-6",
            Self::SelectDojo7 => "dojo.select-7",
            Self::SelectDojo8 => "dojo.select-8",
            Self::SelectDojo9 => "dojo.select-9",
            Self::MoveDojoLeft => "dojo.move-left",
            Self::MoveDojoRight => "dojo.move-right",
            Self::RenameCurrentLair => "lair.rename",
            Self::SaveCurrentLair => "lair.save",
            Self::ToggleCurrentLairPin => "lair.pin-toggle",
            Self::PreviewCurrentLair => "lair.preview",
            Self::RestoreCurrentLair => "lair.restore",
            Self::TerminateCurrentLair => "lair.terminate-confirmed",
            Self::PreviousLair => "lair.previous",
            Self::NextLair => "lair.next",
            Self::LairChooser => "lair.choose",
            Self::DetachWindow => "window.detach",
            Self::SplitHorizontal => "pane.split-below",
            Self::SplitVertical => "pane.split-right",
            Self::FocusLeft => "pane.focus-left",
            Self::FocusRight => "pane.focus-right",
            Self::FocusUp => "pane.focus-up",
            Self::FocusDown => "pane.focus-down",
            Self::CloseFocusedPane => "pane.close",
            Self::ResizePaneSmaller => "pane.resize-smaller",
            Self::ResizePaneLarger => "pane.resize-larger",
            Self::ResizePaneLeftFive => "pane.resize-left-5",
            Self::ResizePaneRightFive => "pane.resize-right-5",
            Self::ResizePaneUpFive => "pane.resize-up-5",
            Self::ResizePaneDownFive => "pane.resize-down-5",
            Self::TogglePaneZoom => "pane.zoom-toggle",
            Self::ToggleTabStrip => "view.toggle-tab-strip",
            Self::BindingHelp => "app.binding-help",
            Self::CopyModeEnter => "copy-mode.enter",
            Self::ConfigReload => "app.config-reload",
            Self::SendPrefix => "terminal.send-prefix",
            Self::SearchScrollback => "history.search",
            Self::PageUp => "history.page-up",
            Self::PageDown => "history.page-down",
            Self::ReturnToLive => "history.return-live",
            Self::ZoomIn => "view.zoom-in",
            Self::ZoomOut => "view.zoom-out",
            Self::ResetZoom => "view.zoom-reset",
            Self::RequestControl => "control.request",
            Self::ReleaseControl => "control.release",
            Self::ForceControl => "control.force",
            Self::RevokeAllAccess => "access.revoke-all",
            Self::AcceptControlTransfer => "control.accept-transfer",
            Self::DenyControlTransfer => "control.deny-transfer",
            Self::ClipboardCopy => "clipboard.copy",
            Self::ClipboardPaste => "clipboard.paste",
        }
    }

    fn parse_bindable(value: &str) -> Result<Self> {
        let action = Self::BINDABLE
            .iter()
            .copied()
            .find(|action| action.config_name() == value)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown or unavailable action '{value}'; run `splinterm keymap show` for bindable actions"
                )
            })?;
        Ok(action)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeymapProfile {
    #[default]
    Splinterm,
    OmarchyTmux,
}

impl KeymapProfile {
    /// Parses one packaged profile name.
    ///
    /// # Errors
    /// Returns an error listing available profiles when the name is unknown.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "splinterm" => Ok(Self::Splinterm),
            "omarchy-tmux" => Ok(Self::OmarchyTmux),
            _ => bail!(
                "unknown keymap profile '{value}'; available profiles: {}",
                BUILT_IN_PROFILE_NAMES.join(", ")
            ),
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Splinterm => "splinterm",
            Self::OmarchyTmux => "omarchy-tmux",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyIdentity {
    Character(char),
    Tab,
    Enter,
    Escape,
    Space,
    Slash,
    Backslash,
    BracketLeft,
    BracketRight,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    End,
    Insert,
    Plus,
    Equal,
    Minus,
    Zero,
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ampersand,
}

impl KeyIdentity {
    fn display(self) -> String {
        match self {
            Self::Character(character) => character.to_ascii_uppercase().to_string(),
            Self::Tab => "Tab".to_owned(),
            Self::Enter => "Enter".to_owned(),
            Self::Escape => "Escape".to_owned(),
            Self::Space => "Space".to_owned(),
            Self::Slash => "Slash".to_owned(),
            Self::Backslash => "Backslash".to_owned(),
            Self::BracketLeft => "BracketLeft".to_owned(),
            Self::BracketRight => "BracketRight".to_owned(),
            Self::Left => "Left".to_owned(),
            Self::Right => "Right".to_owned(),
            Self::Up => "Up".to_owned(),
            Self::Down => "Down".to_owned(),
            Self::PageUp => "PageUp".to_owned(),
            Self::PageDown => "PageDown".to_owned(),
            Self::End => "End".to_owned(),
            Self::Insert => "Insert".to_owned(),
            Self::Plus => "Plus".to_owned(),
            Self::Equal => "Equal".to_owned(),
            Self::Minus => "Minus".to_owned(),
            Self::Zero => "0".to_owned(),
            Self::One => "1".to_owned(),
            Self::Two => "2".to_owned(),
            Self::Three => "3".to_owned(),
            Self::Four => "4".to_owned(),
            Self::Five => "5".to_owned(),
            Self::Six => "6".to_owned(),
            Self::Seven => "7".to_owned(),
            Self::Eight => "8".to_owned(),
            Self::Nine => "9".to_owned(),
            Self::Ampersand => "&".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModifierRule {
    Any,
    Required,
    Forbidden,
}

impl ModifierRule {
    const fn matches(self, active: bool) -> bool {
        match self {
            Self::Any => true,
            Self::Required => active,
            Self::Forbidden => !active,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModifierPattern {
    pub ctrl: ModifierRule,
    pub shift: ModifierRule,
    pub alt: ModifierRule,
    pub logo: ModifierRule,
}

impl ModifierPattern {
    const fn matches(self, active: ActiveModifiers) -> bool {
        self.ctrl.matches(active.ctrl)
            && self.shift.matches(active.shift)
            && self.alt.matches(active.alt)
            && self.logo.matches(active.logo)
    }

    const fn exact(active: ActiveModifiers) -> Self {
        const fn rule(active: bool) -> ModifierRule {
            if active {
                ModifierRule::Required
            } else {
                ModifierRule::Forbidden
            }
        }
        Self {
            ctrl: rule(active.ctrl),
            shift: rule(active.shift),
            alt: rule(active.alt),
            logo: rule(active.logo),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the four protocol modifiers are independent keyboard state"
)]
pub struct ActiveModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NormalizedChord {
    pub modifiers: ActiveModifiers,
    pub key: KeyIdentity,
}

impl NormalizedChord {
    #[must_use]
    pub fn display(self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.modifiers.ctrl {
            parts.push("Ctrl".to_owned());
        }
        if self.modifiers.shift {
            parts.push("Shift".to_owned());
        }
        if self.modifiers.alt {
            parts.push("Alt".to_owned());
        }
        if self.modifiers.logo {
            parts.push("Super".to_owned());
        }
        parts.push(self.key.display());
        parts.join("+")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyChord {
    pub modifiers: ModifierPattern,
    pub key: KeyIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BindingSource {
    BuiltIn { profile: &'static str },
    User { path: PathBuf, line: usize },
}

impl BindingSource {
    #[must_use]
    pub fn short_label(&self) -> String {
        match self {
            Self::BuiltIn { profile } => format!("built-in profile {profile}"),
            Self::User { path, line } => format!(
                "{}:{line}",
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned()
                )
            ),
        }
    }
}

impl fmt::Display for BindingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuiltIn { profile } => write!(formatter, "built-in profile {profile}"),
            Self::User { path, line } => write!(formatter, "{}:{line}", path.display()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NormalizedSequence {
    Direct(NormalizedChord),
    Prefix(NormalizedChord),
}

impl NormalizedSequence {
    #[must_use]
    pub fn display(self) -> String {
        match self {
            Self::Direct(chord) => chord.display(),
            Self::Prefix(chord) => format!("Prefix {}", chord.display()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBinding {
    chord: KeyChord,
    sequence: NormalizedSequence,
    action: ActionId,
    display: String,
    source: BindingSource,
}

impl ResolvedBinding {
    #[must_use]
    pub const fn action(&self) -> ActionId {
        self.action
    }

    #[must_use]
    pub fn display(&self) -> &str {
        &self.display
    }

    #[must_use]
    pub const fn source(&self) -> &BindingSource {
        &self.source
    }

    #[must_use]
    pub const fn normalized(&self) -> NormalizedSequence {
        self.sequence
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrefixState {
    #[default]
    Idle,
    Armed {
        raw_code: u32,
        deadline: Instant,
    },
}

impl PrefixState {
    pub fn clear(&mut self) {
        *self = Self::Idle;
    }

    #[must_use]
    pub fn is_armed(self) -> bool {
        matches!(self, Self::Armed { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeymapPress {
    PassThrough,
    PrefixModifier,
    Consumed(Option<ActionId>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedKeymap {
    profile: KeymapProfile,
    prefixes: Vec<NormalizedChord>,
    bindings: Vec<ResolvedBinding>,
}

impl Default for ResolvedKeymap {
    fn default() -> Self {
        built_in_keymap(KeymapProfile::Splinterm)
    }
}

impl ResolvedKeymap {
    #[must_use]
    pub fn action(&self, key: KeyIdentity, modifiers: ActiveModifiers) -> Option<ActionId> {
        self.bindings
            .iter()
            .filter(|binding| matches!(binding.sequence, NormalizedSequence::Direct(_)))
            .find(|binding| binding.chord.key == key && binding.chord.modifiers.matches(modifiers))
            .map(|binding| binding.action)
    }

    #[must_use]
    pub fn press(
        &self,
        state: &mut PrefixState,
        key: KeyIdentity,
        modifiers: ActiveModifiers,
        raw_code: u32,
        now: Instant,
        timeout: Duration,
    ) -> KeymapPress {
        if matches!(*state, PrefixState::Armed { deadline, .. } if now >= deadline) {
            state.clear();
        }
        if state.is_armed() {
            state.clear();
            let action = self
                .bindings
                .iter()
                .filter(|binding| matches!(binding.sequence, NormalizedSequence::Prefix(_)))
                .find(|binding| {
                    binding.chord.key == key && binding.chord.modifiers.matches(modifiers)
                })
                .map(|binding| binding.action);
            return KeymapPress::Consumed(action);
        }
        if self
            .prefixes
            .iter()
            .any(|prefix| prefix.key == key && prefix.modifiers == modifiers)
        {
            *state = PrefixState::Armed {
                raw_code,
                deadline: now + timeout,
            };
            return KeymapPress::Consumed(None);
        }
        self.action(key, modifiers)
            .map_or(KeymapPress::PassThrough, |action| {
                KeymapPress::Consumed(Some(action))
            })
    }

    #[must_use]
    pub fn primary_shortcut(&self, action: ActionId) -> &str {
        self.bindings
            .iter()
            .find(|binding| binding.action == action)
            .map_or("", |binding| binding.display.as_str())
    }

    #[must_use]
    pub const fn profile(&self) -> KeymapProfile {
        self.profile
    }

    #[must_use]
    pub fn prefixes(&self) -> &[NormalizedChord] {
        &self.prefixes
    }

    #[must_use]
    pub fn bindings(&self) -> &[ResolvedBinding] {
        &self.bindings
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeymapResolution {
    pub keymap: ResolvedKeymap,
    pub diagnostics: Vec<String>,
}

const REQUIRED: ModifierRule = ModifierRule::Required;
const FORBIDDEN: ModifierRule = ModifierRule::Forbidden;
const ANY: ModifierRule = ModifierRule::Any;

const CTRL_NO_SHIFT_NO_ALT_LOGO: ModifierPattern = ModifierPattern {
    ctrl: REQUIRED,
    shift: FORBIDDEN,
    alt: FORBIDDEN,
    logo: FORBIDDEN,
};
const CTRL_INSERT_TRANSLATED: ModifierPattern = ModifierPattern {
    ctrl: REQUIRED,
    shift: FORBIDDEN,
    alt: FORBIDDEN,
    logo: ANY,
};
const CTRL_SHIFT_NO_ALT_LOGO: ModifierPattern = ModifierPattern {
    ctrl: REQUIRED,
    shift: REQUIRED,
    alt: FORBIDDEN,
    logo: FORBIDDEN,
};
const CTRL_SHIFT_ANY_ALT_LOGO: ModifierPattern = ModifierPattern {
    ctrl: REQUIRED,
    shift: REQUIRED,
    alt: ANY,
    logo: ANY,
};
const CTRL_ANY_SHIFT_NO_ALT_LOGO: ModifierPattern = ModifierPattern {
    ctrl: REQUIRED,
    shift: ANY,
    alt: FORBIDDEN,
    logo: FORBIDDEN,
};
const SHIFT_ANY_OTHER: ModifierPattern = ModifierPattern {
    ctrl: ANY,
    shift: REQUIRED,
    alt: ANY,
    logo: ANY,
};
const LOGO_NO_OTHER: ModifierPattern = ModifierPattern {
    ctrl: FORBIDDEN,
    shift: FORBIDDEN,
    alt: FORBIDDEN,
    logo: REQUIRED,
};

fn built_in_binding(
    modifiers: ModifierPattern,
    normalized: NormalizedChord,
    key: KeyIdentity,
    action: ActionId,
    display: &str,
) -> ResolvedBinding {
    ResolvedBinding {
        chord: KeyChord { modifiers, key },
        sequence: NormalizedSequence::Direct(normalized),
        action,
        display: display.to_owned(),
        source: BindingSource::BuiltIn {
            profile: "splinterm",
        },
    }
}

fn omarchy_binding(
    sequence: NormalizedSequence,
    action: ActionId,
    display: &str,
) -> ResolvedBinding {
    let normalized = match sequence {
        NormalizedSequence::Direct(chord) | NormalizedSequence::Prefix(chord) => chord,
    };
    ResolvedBinding {
        chord: KeyChord {
            modifiers: ModifierPattern::exact(normalized.modifiers),
            key: normalized.key,
        },
        sequence,
        action,
        display: display.to_owned(),
        source: BindingSource::BuiltIn {
            profile: "omarchy-tmux",
        },
    }
}

fn normalized(ctrl: bool, shift: bool, key: KeyIdentity) -> NormalizedChord {
    normalized_with(
        ActiveModifiers {
            ctrl,
            shift,
            alt: false,
            logo: false,
        },
        key,
    )
}

const fn normalized_with(modifiers: ActiveModifiers, key: KeyIdentity) -> NormalizedChord {
    NormalizedChord { modifiers, key }
}

#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "the closed built-in keymap keeps every default binding visibly auditable"
)]
pub fn built_in_keymap(profile: KeymapProfile) -> ResolvedKeymap {
    match profile {
        KeymapProfile::Splinterm => ResolvedKeymap {
            profile,
            prefixes: Vec::new(),
            bindings: vec![
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('p')),
                    KeyIdentity::Character('p'),
                    ActionId::CommandPalette,
                    "Ctrl+Shift+P",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('s')),
                    KeyIdentity::Character('s'),
                    ActionId::RecentSessions,
                    "Ctrl+Shift+S",
                ),
                built_in_binding(
                    CTRL_NO_SHIFT_NO_ALT_LOGO,
                    normalized(true, false, KeyIdentity::Tab),
                    KeyIdentity::Tab,
                    ActionId::NextDojo,
                    "Ctrl+Tab",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Tab),
                    KeyIdentity::Tab,
                    ActionId::PreviousDojo,
                    "Ctrl+Shift+Tab",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('b')),
                    KeyIdentity::Character('b'),
                    ActionId::ToggleTabStrip,
                    "Ctrl+Shift+B",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('d')),
                    KeyIdentity::Character('d'),
                    ActionId::NewDojo,
                    "Ctrl+Shift+D",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('q')),
                    KeyIdentity::Character('q'),
                    ActionId::CloseCurrentTab,
                    "Ctrl+Shift+Q",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Enter),
                    KeyIdentity::Enter,
                    ActionId::SplitHorizontal,
                    "Ctrl+Shift+Enter",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Backslash),
                    KeyIdentity::Backslash,
                    ActionId::SplitVertical,
                    "Ctrl+Shift+\\",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('w')),
                    KeyIdentity::Character('w'),
                    ActionId::CloseFocusedPane,
                    "Ctrl+Shift+W",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::BracketLeft),
                    KeyIdentity::BracketLeft,
                    ActionId::ResizePaneSmaller,
                    "Ctrl+Shift+[",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::BracketRight),
                    KeyIdentity::BracketRight,
                    ActionId::ResizePaneLarger,
                    "Ctrl+Shift+]",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Left),
                    KeyIdentity::Left,
                    ActionId::FocusLeft,
                    "Ctrl+Shift+Left",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Right),
                    KeyIdentity::Right,
                    ActionId::FocusRight,
                    "Ctrl+Shift+Right",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Up),
                    KeyIdentity::Up,
                    ActionId::FocusUp,
                    "Ctrl+Shift+Up",
                ),
                built_in_binding(
                    CTRL_SHIFT_NO_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Down),
                    KeyIdentity::Down,
                    ActionId::FocusDown,
                    "Ctrl+Shift+Down",
                ),
                built_in_binding(
                    CTRL_ANY_SHIFT_NO_ALT_LOGO,
                    normalized(true, false, KeyIdentity::Plus),
                    KeyIdentity::Plus,
                    ActionId::ZoomIn,
                    "Ctrl++",
                ),
                built_in_binding(
                    CTRL_ANY_SHIFT_NO_ALT_LOGO,
                    normalized(true, false, KeyIdentity::Equal),
                    KeyIdentity::Equal,
                    ActionId::ZoomIn,
                    "Ctrl++",
                ),
                built_in_binding(
                    CTRL_ANY_SHIFT_NO_ALT_LOGO,
                    normalized(true, false, KeyIdentity::Minus),
                    KeyIdentity::Minus,
                    ActionId::ZoomOut,
                    "Ctrl+-",
                ),
                built_in_binding(
                    CTRL_ANY_SHIFT_NO_ALT_LOGO,
                    normalized(true, false, KeyIdentity::Zero),
                    KeyIdentity::Zero,
                    ActionId::ResetZoom,
                    "Ctrl+0",
                ),
                built_in_binding(
                    LOGO_NO_OTHER,
                    normalized_with(
                        ActiveModifiers {
                            logo: true,
                            ..ActiveModifiers::default()
                        },
                        KeyIdentity::Character('c'),
                    ),
                    KeyIdentity::Character('c'),
                    ActionId::ClipboardCopy,
                    "Super+C",
                ),
                built_in_binding(
                    CTRL_INSERT_TRANSLATED,
                    normalized(true, false, KeyIdentity::Insert),
                    KeyIdentity::Insert,
                    ActionId::ClipboardCopy,
                    "Ctrl+Insert",
                ),
                built_in_binding(
                    LOGO_NO_OTHER,
                    normalized_with(
                        ActiveModifiers {
                            logo: true,
                            ..ActiveModifiers::default()
                        },
                        KeyIdentity::Character('v'),
                    ),
                    KeyIdentity::Character('v'),
                    ActionId::ClipboardPaste,
                    "Super+V",
                ),
                built_in_binding(
                    SHIFT_ANY_OTHER,
                    normalized(false, true, KeyIdentity::Insert),
                    KeyIdentity::Insert,
                    ActionId::ClipboardPaste,
                    "Shift+Insert",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('c')),
                    KeyIdentity::Character('c'),
                    ActionId::ClipboardCopy,
                    "Ctrl+Shift+C",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('v')),
                    KeyIdentity::Character('v'),
                    ActionId::ClipboardPaste,
                    "Ctrl+Shift+V",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('f')),
                    KeyIdentity::Character('f'),
                    ActionId::SearchScrollback,
                    "Ctrl+Shift+F",
                ),
                built_in_binding(
                    SHIFT_ANY_OTHER,
                    normalized(false, true, KeyIdentity::PageUp),
                    KeyIdentity::PageUp,
                    ActionId::PageUp,
                    "Shift+PageUp",
                ),
                built_in_binding(
                    SHIFT_ANY_OTHER,
                    normalized(false, true, KeyIdentity::PageDown),
                    KeyIdentity::PageDown,
                    ActionId::PageDown,
                    "Shift+PageDown",
                ),
                built_in_binding(
                    SHIFT_ANY_OTHER,
                    normalized(false, true, KeyIdentity::End),
                    KeyIdentity::End,
                    ActionId::ReturnToLive,
                    "Shift+End",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('t')),
                    KeyIdentity::Character('t'),
                    ActionId::RequestControl,
                    "Ctrl+Shift+T",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('l')),
                    KeyIdentity::Character('l'),
                    ActionId::ReleaseControl,
                    "Ctrl+Shift+L",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('u')),
                    KeyIdentity::Character('u'),
                    ActionId::ForceControl,
                    "Ctrl+Shift+U",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('r')),
                    KeyIdentity::Character('r'),
                    ActionId::RevokeAllAccess,
                    "Ctrl+Shift+R",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('y')),
                    KeyIdentity::Character('y'),
                    ActionId::AcceptControlTransfer,
                    "Ctrl+Shift+Y",
                ),
                built_in_binding(
                    CTRL_SHIFT_ANY_ALT_LOGO,
                    normalized(true, true, KeyIdentity::Character('n')),
                    KeyIdentity::Character('n'),
                    ActionId::DenyControlTransfer,
                    "Ctrl+Shift+N",
                ),
            ],
        },
        KeymapProfile::OmarchyTmux => {
            let mut keymap = built_in_keymap(KeymapProfile::Splinterm);
            keymap.profile = profile;
            let chord = |ctrl, shift, alt, key| {
                normalized_with(
                    ActiveModifiers {
                        ctrl,
                        shift,
                        alt,
                        logo: false,
                    },
                    key,
                )
            };
            keymap.prefixes = vec![
                chord(true, false, false, KeyIdentity::Space),
                chord(true, false, false, KeyIdentity::Character('b')),
            ];
            let direct =
                |ctrl, shift, alt, key| NormalizedSequence::Direct(chord(ctrl, shift, alt, key));
            let prefixed =
                |ctrl, shift, key| NormalizedSequence::Prefix(chord(ctrl, shift, false, key));
            for preferred in ["Ctrl+Shift+V", "Ctrl+Shift+C"] {
                if let Some(index) = keymap
                    .bindings
                    .iter()
                    .position(|binding| binding.display() == preferred)
                {
                    let binding = keymap.bindings.remove(index);
                    keymap.bindings.insert(0, binding);
                }
            }
            keymap.bindings.splice(
                2..2,
                [
                    omarchy_binding(
                        prefixed(true, false, KeyIdentity::Space),
                        ActionId::SendPrefix,
                        "Prefix Ctrl+Space",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Slash),
                        ActionId::BindingHelp,
                        "Prefix ?",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::BracketLeft),
                        ActionId::CopyModeEnter,
                        "Prefix [",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('b')),
                        ActionId::ToggleTabStrip,
                        "Prefix B",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('q')),
                        ActionId::ConfigReload,
                        "Prefix Q",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('c')),
                        ActionId::NewDojo,
                        "Prefix C",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('k')),
                        ActionId::TerminateCurrentDojo,
                        "Prefix K",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Ampersand),
                        ActionId::TerminateCurrentDojo,
                        "Prefix &",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('r')),
                        ActionId::RenameCurrentTab,
                        "Prefix R",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('w')),
                        ActionId::DojoChooser,
                        "Prefix W",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('p')),
                        ActionId::PreviousDojo,
                        "Prefix P",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('n')),
                        ActionId::NextDojo,
                        "Prefix N",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::One),
                        ActionId::SelectDojo1,
                        "Alt+1",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Two),
                        ActionId::SelectDojo2,
                        "Alt+2",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Three),
                        ActionId::SelectDojo3,
                        "Alt+3",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Four),
                        ActionId::SelectDojo4,
                        "Alt+4",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Five),
                        ActionId::SelectDojo5,
                        "Alt+5",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Six),
                        ActionId::SelectDojo6,
                        "Alt+6",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Seven),
                        ActionId::SelectDojo7,
                        "Alt+7",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Eight),
                        ActionId::SelectDojo8,
                        "Alt+8",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Nine),
                        ActionId::SelectDojo9,
                        "Alt+9",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Left),
                        ActionId::PreviousDojo,
                        "Alt+Left",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Right),
                        ActionId::NextDojo,
                        "Alt+Right",
                    ),
                    omarchy_binding(
                        direct(false, true, true, KeyIdentity::Left),
                        ActionId::MoveDojoLeft,
                        "Alt+Shift+Left",
                    ),
                    omarchy_binding(
                        direct(false, true, true, KeyIdentity::Right),
                        ActionId::MoveDojoRight,
                        "Alt+Shift+Right",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('c')),
                        ActionId::NewSession,
                        "Prefix Shift+C",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('k')),
                        ActionId::TerminateCurrentLair,
                        "Prefix Shift+K",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('r')),
                        ActionId::RenameCurrentLair,
                        "Prefix Shift+R",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('s')),
                        ActionId::SaveCurrentLair,
                        "Prefix Shift+S",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('f')),
                        ActionId::ToggleCurrentLairPin,
                        "Prefix Shift+F",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('v')),
                        ActionId::PreviewCurrentLair,
                        "Prefix Shift+V",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('o')),
                        ActionId::RestoreCurrentLair,
                        "Prefix Shift+O",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('p')),
                        ActionId::PreviousLair,
                        "Prefix Shift+P",
                    ),
                    omarchy_binding(
                        prefixed(false, true, KeyIdentity::Character('n')),
                        ActionId::NextLair,
                        "Prefix Shift+N",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Up),
                        ActionId::PreviousLair,
                        "Alt+Up",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Down),
                        ActionId::NextLair,
                        "Alt+Down",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('s')),
                        ActionId::LairChooser,
                        "Prefix S",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('d')),
                        ActionId::DetachWindow,
                        "Prefix D",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Enter),
                        ActionId::SplitHorizontal,
                        "Alt+Enter",
                    ),
                    omarchy_binding(
                        direct(false, true, true, KeyIdentity::Enter),
                        ActionId::SplitVertical,
                        "Alt+Shift+Enter",
                    ),
                    omarchy_binding(
                        direct(false, false, true, KeyIdentity::Escape),
                        ActionId::CloseFocusedPane,
                        "Alt+Escape",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('h')),
                        ActionId::SplitHorizontal,
                        "Prefix H",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('v')),
                        ActionId::SplitVertical,
                        "Prefix V",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('x')),
                        ActionId::CloseFocusedPane,
                        "Prefix X",
                    ),
                    omarchy_binding(
                        prefixed(false, false, KeyIdentity::Character('z')),
                        ActionId::TogglePaneZoom,
                        "Prefix Z",
                    ),
                    omarchy_binding(
                        direct(true, false, true, KeyIdentity::Left),
                        ActionId::FocusLeft,
                        "Ctrl+Alt+Left",
                    ),
                    omarchy_binding(
                        direct(true, false, true, KeyIdentity::Right),
                        ActionId::FocusRight,
                        "Ctrl+Alt+Right",
                    ),
                    omarchy_binding(
                        direct(true, false, true, KeyIdentity::Up),
                        ActionId::FocusUp,
                        "Ctrl+Alt+Up",
                    ),
                    omarchy_binding(
                        direct(true, false, true, KeyIdentity::Down),
                        ActionId::FocusDown,
                        "Ctrl+Alt+Down",
                    ),
                    omarchy_binding(
                        direct(true, true, true, KeyIdentity::Left),
                        ActionId::ResizePaneLeftFive,
                        "Ctrl+Alt+Shift+Left",
                    ),
                    omarchy_binding(
                        direct(true, true, true, KeyIdentity::Right),
                        ActionId::ResizePaneRightFive,
                        "Ctrl+Alt+Shift+Right",
                    ),
                    omarchy_binding(
                        direct(true, true, true, KeyIdentity::Up),
                        ActionId::ResizePaneUpFive,
                        "Ctrl+Alt+Shift+Up",
                    ),
                    omarchy_binding(
                        direct(true, true, true, KeyIdentity::Down),
                        ActionId::ResizePaneDownFive,
                        "Ctrl+Alt+Shift+Down",
                    ),
                ],
            );
            keymap
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeymapDocument {
    version: u16,
    inherits: Option<String>,
    #[serde(default)]
    unbind: Vec<toml::Spanned<UnbindSpec>>,
    #[serde(default)]
    binding: Vec<toml::Spanned<BindingSpec>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnbindSpec {
    sequence: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingSpec {
    sequence: Vec<String>,
    action: String,
}

pub(crate) fn resolve_keymap(
    profile: KeymapProfile,
    file: Option<&Path>,
) -> Result<KeymapResolution> {
    let Some(path) = file else {
        return Ok(KeymapResolution {
            keymap: built_in_keymap(profile),
            diagnostics: Vec::new(),
        });
    };
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    resolve_keymap_text(profile, &text, path)
        .with_context(|| format!("parse keymap {}", path.display()))
}

#[allow(
    clippy::too_many_lines,
    reason = "strict keymap parsing and conflict diagnostics form one validation transaction"
)]
pub(crate) fn resolve_keymap_text(
    selected_profile: KeymapProfile,
    text: &str,
    path: &Path,
) -> Result<KeymapResolution> {
    let document: KeymapDocument = toml::from_str(text)?;
    if document.version != 1 {
        bail!(
            "unsupported keymap version {}; expected version 1",
            document.version
        );
    }
    let inherited_profile = document
        .inherits
        .as_deref()
        .map(KeymapProfile::parse)
        .transpose()?
        .unwrap_or(selected_profile);
    if inherited_profile != selected_profile {
        bail!(
            "keymap inherits '{}' but config.ini selected '{}'",
            inherited_profile.name(),
            selected_profile.name()
        );
    }
    let inherited = built_in_keymap(inherited_profile);
    let prefixes = inherited.prefixes;
    let mut bindings = inherited.bindings;
    let mut diagnostics = Vec::new();
    let mut validation_errors = Vec::new();
    for unbind in document.unbind {
        let line = line_at_byte(text, unbind.span().start);
        let unbind = unbind.into_inner();
        let chord = match parse_sequence(&unbind.sequence) {
            Ok(chord) => chord,
            Err(error) => {
                validation_errors.push(format!(
                    "{}:{line}: invalid unbind: {error:#}",
                    path.display()
                ));
                continue;
            }
        };
        let before = bindings.len();
        bindings.retain(|binding| binding.sequence != chord);
        if bindings.len() == before {
            diagnostics.push(format!(
                "{}:{line}: unbind {} matched no inherited binding",
                path.display(),
                chord.display()
            ));
        }
    }
    for binding in document.binding {
        let line = line_at_byte(text, binding.span().start);
        let binding = binding.into_inner();
        let normalized = match parse_sequence(&binding.sequence) {
            Ok(chord) => chord,
            Err(error) => {
                validation_errors.push(format!(
                    "{}:{line}: invalid binding: {error:#}",
                    path.display()
                ));
                continue;
            }
        };
        let action = match ActionId::parse_bindable(&binding.action) {
            Ok(action) => action,
            Err(error) => {
                validation_errors.push(format!("{}:{line}: {error:#}", path.display()));
                continue;
            }
        };
        if matches!(normalized, NormalizedSequence::Prefix(_)) && prefixes.is_empty() {
            validation_errors.push(format!(
                "{}:{line}: profile '{}' defines no prefix chords",
                path.display(),
                inherited_profile.name()
            ));
            continue;
        }
        let normalized_chord = match normalized {
            NormalizedSequence::Direct(chord) | NormalizedSequence::Prefix(chord) => chord,
        };
        let chord = KeyChord {
            modifiers: ModifierPattern::exact(normalized_chord.modifiers),
            key: normalized_chord.key,
        };
        if matches!(normalized, NormalizedSequence::Direct(_))
            && prefixes.contains(&normalized_chord)
        {
            validation_errors.push(format!(
                "{}:{line}: direct chord {} is reserved as a profile prefix",
                path.display(),
                normalized.display()
            ));
            continue;
        }
        if let Some(existing) = bindings.iter().find(|existing| {
            std::mem::discriminant(&existing.sequence) == std::mem::discriminant(&normalized)
                && chords_overlap(existing.chord, chord)
        }) {
            validation_errors.push(format!(
                "{}:{line}: chord {} for {} conflicts with {} from {}",
                path.display(),
                normalized.display(),
                action.config_name(),
                existing.action.config_name(),
                existing.source
            ));
            continue;
        }
        bindings.push(ResolvedBinding {
            chord,
            sequence: normalized,
            action,
            display: normalized.display(),
            source: BindingSource::User {
                path: path.to_owned(),
                line,
            },
        });
    }
    if !validation_errors.is_empty() {
        bail!(
            "keymap validation failed:\n  - {}",
            validation_errors.join("\n  - ")
        );
    }
    Ok(KeymapResolution {
        keymap: ResolvedKeymap {
            profile: inherited_profile,
            prefixes,
            bindings,
        },
        diagnostics,
    })
}

fn line_at_byte(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn parse_sequence(sequence: &[String]) -> Result<NormalizedSequence> {
    match sequence {
        [chord] => Ok(NormalizedSequence::Direct(parse_chord(chord)?)),
        [prefix, chord] if prefix.eq_ignore_ascii_case("Prefix") => {
            Ok(NormalizedSequence::Prefix(parse_chord(chord)?))
        }
        [prefix, _] => bail!("unknown sequence start '{prefix}'; expected Prefix"),
        _ => bail!("sequence must contain one direct chord or Prefix plus one chord"),
    }
}

fn parse_chord(value: &str) -> Result<NormalizedChord> {
    let parts = value.split('+').collect::<Vec<_>>();
    if parts.is_empty() || parts.iter().any(|part| part.trim().is_empty()) {
        bail!("chord must contain one key and no empty components");
    }
    let Some((key_name, modifiers)) = parts.split_last() else {
        bail!("chord must contain one key");
    };
    let mut active = ActiveModifiers::default();
    for modifier in modifiers {
        let slot = match modifier.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => &mut active.ctrl,
            "shift" => &mut active.shift,
            "alt" => &mut active.alt,
            "super" | "logo" => &mut active.logo,
            value => bail!("unknown modifier '{value}'"),
        };
        if *slot {
            bail!("duplicate modifier '{}'", modifier.trim());
        }
        *slot = true;
    }
    let key_name = key_name.trim();
    let key = match key_name.to_ascii_lowercase().as_str() {
        "tab" => KeyIdentity::Tab,
        "enter" | "return" | "kp_enter" => KeyIdentity::Enter,
        "escape" | "esc" => KeyIdentity::Escape,
        "space" => KeyIdentity::Space,
        "slash" | "/" => KeyIdentity::Slash,
        "?" => {
            if active.shift {
                bail!("duplicate modifier 'Shift' for '?' alias");
            }
            active.shift = true;
            KeyIdentity::Slash
        }
        "backslash" | "\\" => KeyIdentity::Backslash,
        "bracketleft" | "[" => KeyIdentity::BracketLeft,
        "bracketright" | "]" => KeyIdentity::BracketRight,
        "left" => KeyIdentity::Left,
        "right" => KeyIdentity::Right,
        "up" => KeyIdentity::Up,
        "down" => KeyIdentity::Down,
        "pageup" | "page_up" => KeyIdentity::PageUp,
        "pagedown" | "page_down" => KeyIdentity::PageDown,
        "end" => KeyIdentity::End,
        "insert" => KeyIdentity::Insert,
        "plus" => KeyIdentity::Plus,
        "equal" | "=" => KeyIdentity::Equal,
        "minus" | "-" => KeyIdentity::Minus,
        "0" | "kp_0" => KeyIdentity::Zero,
        "1" => KeyIdentity::One,
        "2" => KeyIdentity::Two,
        "3" => KeyIdentity::Three,
        "4" => KeyIdentity::Four,
        "5" => KeyIdentity::Five,
        "6" => KeyIdentity::Six,
        "7" => KeyIdentity::Seven,
        "8" => KeyIdentity::Eight,
        "9" => KeyIdentity::Nine,
        "&" | "ampersand" => KeyIdentity::Ampersand,
        name if name.chars().count() == 1 => {
            let character = name.chars().next().expect("one-character key");
            if character.is_ascii_alphabetic() {
                KeyIdentity::Character(character.to_ascii_lowercase())
            } else {
                bail!("unsupported printable key '{key_name}'")
            }
        }
        _ => bail!("unknown key '{key_name}'"),
    };
    Ok(NormalizedChord {
        modifiers: active,
        key,
    })
}

fn chords_overlap(left: KeyChord, right: KeyChord) -> bool {
    left.key == right.key
        && (0_u8..16).any(|bits| {
            let active = ActiveModifiers {
                ctrl: bits & 0b0001 != 0,
                shift: bits & 0b0010 != 0,
                alt: bits & 0b0100 != 0,
                logo: bits & 0b1000 != 0,
            };
            left.modifiers.matches(active) && right.modifiers.matches(active)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_chords_resolve_without_semantic_conflicts() {
        let keymap = built_in_keymap(KeymapProfile::Splinterm);
        for binding in keymap.bindings() {
            let active = ActiveModifiers {
                ctrl: binding.chord.modifiers.ctrl == ModifierRule::Required,
                shift: binding.chord.modifiers.shift == ModifierRule::Required,
                alt: binding.chord.modifiers.alt == ModifierRule::Required,
                logo: binding.chord.modifiers.logo == ModifierRule::Required,
            };
            assert_eq!(
                keymap.action(binding.chord.key, active),
                Some(binding.action)
            );
        }
        assert_eq!(
            keymap.primary_shortcut(ActionId::ToggleTabStrip),
            "Ctrl+Shift+B"
        );
        assert_eq!(keymap.primary_shortcut(ActionId::ClipboardCopy), "Super+C");
        assert_eq!(keymap.primary_shortcut(ActionId::ClipboardPaste), "Super+V");
        assert_eq!(
            keymap.action(
                KeyIdentity::Character('x'),
                ActiveModifiers {
                    logo: true,
                    ..ActiveModifiers::default()
                }
            ),
            None,
            "terminal Super+X remains application-owned"
        );
        for (index, left) in keymap.bindings().iter().enumerate() {
            for right in &keymap.bindings()[index + 1..] {
                if std::mem::discriminant(&left.sequence) == std::mem::discriminant(&right.sequence)
                {
                    assert!(
                        !chords_overlap(left.chord, right.chord),
                        "overlapping chords: {} and {}",
                        left.display,
                        right.display
                    );
                }
            }
        }
    }

    #[test]
    fn every_built_in_binding_resolves_to_its_declared_action() {
        for profile in [KeymapProfile::Splinterm, KeymapProfile::OmarchyTmux] {
            let keymap = built_in_keymap(profile);
            for binding in keymap.bindings() {
                let active = ActiveModifiers {
                    ctrl: binding.chord.modifiers.ctrl == ModifierRule::Required,
                    shift: binding.chord.modifiers.shift == ModifierRule::Required,
                    alt: binding.chord.modifiers.alt == ModifierRule::Required,
                    logo: binding.chord.modifiers.logo == ModifierRule::Required,
                };
                let resolved = match binding.sequence {
                    NormalizedSequence::Direct(_) => keymap.action(binding.chord.key, active),
                    NormalizedSequence::Prefix(_) => {
                        let mut state = PrefixState::Armed {
                            raw_code: 1,
                            deadline: Instant::now() + Duration::from_secs(1),
                        };
                        match keymap.press(
                            &mut state,
                            binding.chord.key,
                            active,
                            2,
                            Instant::now(),
                            Duration::from_secs(1),
                        ) {
                            KeymapPress::Consumed(action) => action,
                            other => panic!(
                                "{} binding {} did not consume: {other:?}",
                                profile.name(),
                                binding.display()
                            ),
                        }
                    }
                };
                assert_eq!(
                    resolved,
                    Some(binding.action()),
                    "{} binding {} drifted",
                    profile.name(),
                    binding.display()
                );
            }
        }
    }

    #[test]
    fn shared_desktop_clipboard_aliases_preserve_terminal_cut_and_undo() {
        for profile in [KeymapProfile::Splinterm, KeymapProfile::OmarchyTmux] {
            let keymap = built_in_keymap(profile);
            let desktop = ActiveModifiers {
                logo: true,
                ..ActiveModifiers::default()
            };
            assert_eq!(
                keymap.action(KeyIdentity::Character('c'), desktop),
                Some(ActionId::ClipboardCopy),
                "{} Super+C",
                profile.name()
            );
            assert_eq!(
                keymap.action(KeyIdentity::Character('v'), desktop),
                Some(ActionId::ClipboardPaste),
                "{} Super+V",
                profile.name()
            );
            assert_eq!(keymap.action(KeyIdentity::Character('x'), desktop), None);
            assert_eq!(keymap.action(KeyIdentity::Character('z'), desktop), None);
            assert!(keymap.bindings().iter().any(|binding| {
                binding.action() == ActionId::ClipboardCopy && binding.display() == "Ctrl+Shift+C"
            }));
            assert!(keymap.bindings().iter().any(|binding| {
                binding.action() == ActionId::ClipboardPaste && binding.display() == "Ctrl+Shift+V"
            }));
        }
    }

    #[test]
    fn bindable_action_names_are_closed_unique_and_nonempty() {
        let names = ActionId::BINDABLE
            .iter()
            .map(|action| action.config_name())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(names.len(), ActionId::BINDABLE.len());
        assert!(names.iter().all(|name| !name.trim().is_empty()));
    }

    #[test]
    fn chord_parser_is_canonical_strict_and_case_does_not_imply_shift() {
        assert_eq!(
            parse_chord("Ctrl+Shift+P").unwrap().display(),
            "Ctrl+Shift+P"
        );
        assert_eq!(
            parse_chord("Ctrl+P").unwrap(),
            parse_chord("ctrl+p").unwrap()
        );
        assert_eq!(
            parse_chord("Ctrl+P").unwrap(),
            parse_chord("Ctrl+p").unwrap()
        );
        assert!(parse_chord("Ctrl+Ctrl+P").is_err());
        assert!(parse_chord("Ctrl++").is_err());
        assert!(parse_chord("Hyper+P").is_err());
        assert_eq!(
            parse_chord("Ctrl+?").unwrap(),
            parse_chord("Ctrl+Shift+Slash").unwrap()
        );
        assert!(parse_chord("Ctrl+Shift+?").is_err());
    }

    #[test]
    fn documented_key_and_modifier_spellings_have_golden_normalization() {
        for (input, expected) in [
            ("A", KeyIdentity::Character('a')),
            ("z", KeyIdentity::Character('z')),
            ("Tab", KeyIdentity::Tab),
            ("Enter", KeyIdentity::Enter),
            ("Return", KeyIdentity::Enter),
            ("KP_Enter", KeyIdentity::Enter),
            ("Backslash", KeyIdentity::Backslash),
            ("\\", KeyIdentity::Backslash),
            ("BracketLeft", KeyIdentity::BracketLeft),
            ("[", KeyIdentity::BracketLeft),
            ("BracketRight", KeyIdentity::BracketRight),
            ("]", KeyIdentity::BracketRight),
            ("Left", KeyIdentity::Left),
            ("Right", KeyIdentity::Right),
            ("Up", KeyIdentity::Up),
            ("Down", KeyIdentity::Down),
            ("PageUp", KeyIdentity::PageUp),
            ("Page_Up", KeyIdentity::PageUp),
            ("PageDown", KeyIdentity::PageDown),
            ("Page_Down", KeyIdentity::PageDown),
            ("End", KeyIdentity::End),
            ("Insert", KeyIdentity::Insert),
            ("Plus", KeyIdentity::Plus),
            ("Equal", KeyIdentity::Equal),
            ("=", KeyIdentity::Equal),
            ("Minus", KeyIdentity::Minus),
            ("-", KeyIdentity::Minus),
            ("0", KeyIdentity::Zero),
            ("KP_0", KeyIdentity::Zero),
        ] {
            assert_eq!(parse_chord(input).unwrap().key, expected, "{input}");
        }
        for (input, expected) in [
            (
                "Ctrl+A",
                ActiveModifiers {
                    ctrl: true,
                    ..ActiveModifiers::default()
                },
            ),
            (
                "Control+A",
                ActiveModifiers {
                    ctrl: true,
                    ..ActiveModifiers::default()
                },
            ),
            (
                "Shift+A",
                ActiveModifiers {
                    shift: true,
                    ..ActiveModifiers::default()
                },
            ),
            (
                "Alt+A",
                ActiveModifiers {
                    alt: true,
                    ..ActiveModifiers::default()
                },
            ),
            (
                "Super+A",
                ActiveModifiers {
                    logo: true,
                    ..ActiveModifiers::default()
                },
            ),
            (
                "Logo+A",
                ActiveModifiers {
                    logo: true,
                    ..ActiveModifiers::default()
                },
            ),
        ] {
            assert_eq!(parse_chord(input).unwrap().modifiers, expected, "{input}");
        }
    }

    #[test]
    fn documented_chord_error_classes_are_rejected() {
        for input in [
            "",
            "Ctrl+",
            "+A",
            "Ctrl+Ctrl+A",
            "Ctrl+Control+A",
            "Super+Logo+A",
            "Hyper+A",
            "Ctrl",
            "Ctrl+Shift+?",
            "Ctrl+F1",
        ] {
            assert!(parse_chord(input).is_err(), "{input} should fail");
        }
        assert!(parse_sequence(&[]).is_err());
        assert!(parse_sequence(&["Ctrl+A".to_owned(), "b".to_owned()]).is_err());
    }

    #[test]
    fn overlay_unbinds_then_adds_and_reports_sources() {
        let text = r#"
version = 1
inherits = "splinterm"

[[unbind]] # remove the inherited palette chord
sequence = ["Ctrl+Shift+P"]

[[binding]] # install the local replacement
sequence = ["Ctrl+Alt+P"]
action = "app.command-palette"
"#;
        let path = Path::new("/tmp/keybindings.toml");
        let resolved = resolve_keymap_text(KeymapProfile::Splinterm, text, path).unwrap();
        assert_eq!(
            resolved.keymap.action(
                KeyIdentity::Character('p'),
                ActiveModifiers {
                    ctrl: true,
                    alt: true,
                    ..ActiveModifiers::default()
                }
            ),
            Some(ActionId::CommandPalette)
        );
        assert_eq!(
            resolved.keymap.primary_shortcut(ActionId::CommandPalette),
            "Ctrl+Alt+P"
        );
        assert!(resolved.diagnostics.is_empty());
        assert!(matches!(
            resolved.keymap.bindings().last().unwrap().source(),
            BindingSource::User { line: 8, .. }
        ));
    }

    #[test]
    fn close_other_tabs_is_bindable_without_claiming_a_default_chord() {
        assert!(ActionId::BINDABLE.contains(&ActionId::CloseOtherTabs));
        let path = Path::new("keybindings.toml");
        let text = r#"
version = 1
inherits = "splinterm"
[[binding]]
sequence = ["Ctrl+Alt+O"]
action = "dojo.close-other-tabs"
"#;
        let resolved = resolve_keymap_text(KeymapProfile::Splinterm, text, path).unwrap();
        assert_eq!(
            resolved.keymap.action(
                KeyIdentity::Character('o'),
                ActiveModifiers {
                    ctrl: true,
                    alt: true,
                    ..ActiveModifiers::default()
                }
            ),
            Some(ActionId::CloseOtherTabs)
        );
        assert_eq!(
            resolved.keymap.primary_shortcut(ActionId::CloseOtherTabs),
            "Ctrl+Alt+O"
        );
        assert_eq!(
            built_in_keymap(KeymapProfile::Splinterm).primary_shortcut(ActionId::CloseOtherTabs),
            ""
        );
    }

    #[test]
    fn overlay_rejects_conflicts_unknown_fields_actions_and_versions() {
        let path = Path::new("keybindings.toml");
        let conflict = r#"
version = 1
inherits = "splinterm"
[[binding]]
sequence = ["Ctrl+Shift+C"]
action = "clipboard.paste"
"#;
        let error = resolve_keymap_text(KeymapProfile::Splinterm, conflict, path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("conflicts"));
        assert!(error.contains("built-in profile splinterm"));
        assert!(resolve_keymap_text(KeymapProfile::Splinterm, "version = 2", path).is_err());
        assert!(
            resolve_keymap_text(KeymapProfile::Splinterm, "version = 1\nextra = true", path)
                .is_err()
        );
        assert!(
            resolve_keymap_text(
                KeymapProfile::Splinterm,
                "version = 1\n[[binding]]\nsequence = [\"Ctrl+A\"]\naction = \"shell.run\"",
                path,
            )
            .is_err()
        );
        let multiple = resolve_keymap_text(
            KeymapProfile::Splinterm,
            "version = 1\n[[binding]]\nsequence = [\"Ctrl+*\"]\naction = \"dojo.new\"\n[[binding]]\nsequence = [\"Ctrl+A\"]\naction = \"shell.run\"",
            path,
        )
        .unwrap_err()
        .to_string();
        assert!(multiple.contains("unsupported printable key"));
        assert!(multiple.contains("unknown or unavailable action"));
    }

    #[test]
    fn unmatched_unbind_is_a_diagnostic_and_prefix_requires_a_profile_prefix() {
        let path = Path::new("keybindings.toml");
        let unmatched = "version = 1\n[[unbind]]\nsequence = [\"Ctrl+A\"]";
        let resolved = resolve_keymap_text(KeymapProfile::Splinterm, unmatched, path).unwrap();
        assert_eq!(resolved.diagnostics.len(), 1);
        let prefix =
            "version = 1\n[[binding]]\nsequence = [\"Prefix\", \"c\"]\naction = \"dojo.new\"";
        assert!(resolve_keymap_text(KeymapProfile::Splinterm, prefix, path).is_err());
    }

    #[test]
    fn omarchy_profile_has_two_prefixes_and_direct_pane_bindings() {
        let keymap = built_in_keymap(KeymapProfile::OmarchyTmux);
        assert_eq!(keymap.prefixes().len(), 2);
        assert_eq!(
            keymap.action(
                KeyIdentity::Enter,
                ActiveModifiers {
                    alt: true,
                    ..ActiveModifiers::default()
                }
            ),
            Some(ActionId::SplitHorizontal)
        );
        assert_eq!(
            keymap.action(
                KeyIdentity::Left,
                ActiveModifiers {
                    ctrl: true,
                    shift: true,
                    alt: true,
                    ..ActiveModifiers::default()
                }
            ),
            Some(ActionId::ResizePaneLeftFive)
        );
        assert_eq!(
            keymap.primary_shortcut(ActionId::ToggleTabStrip),
            "Prefix B"
        );
        assert_eq!(keymap.primary_shortcut(ActionId::BindingHelp), "Prefix ?");
        assert_eq!(keymap.primary_shortcut(ActionId::CopyModeEnter), "Prefix [");
        assert_eq!(
            keymap.primary_shortcut(ActionId::ClipboardCopy),
            "Ctrl+Shift+C"
        );
        assert_eq!(
            keymap.primary_shortcut(ActionId::ClipboardPaste),
            "Ctrl+Shift+V"
        );
        assert_eq!(
            keymap.action(
                KeyIdentity::Character('x'),
                ActiveModifiers {
                    logo: true,
                    ..ActiveModifiers::default()
                }
            ),
            None
        );
        assert_eq!(
            keymap.primary_shortcut(ActionId::SplitHorizontal),
            "Alt+Enter"
        );
        assert_eq!(keymap.primary_shortcut(ActionId::SelectDojo1), "Alt+1");
        assert_eq!(keymap.primary_shortcut(ActionId::DojoChooser), "Prefix W");
        assert_eq!(
            keymap.primary_shortcut(ActionId::TerminateCurrentLair),
            "Prefix Shift+K"
        );
        assert_eq!(keymap.primary_shortcut(ActionId::LairChooser), "Prefix S");
        assert_eq!(
            keymap.primary_shortcut(ActionId::SaveCurrentLair),
            "Prefix Shift+S"
        );
        assert_eq!(
            keymap.primary_shortcut(ActionId::ToggleCurrentLairPin),
            "Prefix Shift+F"
        );
        assert_eq!(
            keymap.primary_shortcut(ActionId::PreviewCurrentLair),
            "Prefix Shift+V"
        );
        assert_eq!(
            keymap.primary_shortcut(ActionId::RestoreCurrentLair),
            "Prefix Shift+O"
        );
        assert_eq!(keymap.primary_shortcut(ActionId::DetachWindow), "Prefix D");
    }

    #[test]
    fn prefix_press_timeout_unknown_key_and_send_prefix_are_bounded() {
        let keymap = built_in_keymap(KeymapProfile::OmarchyTmux);
        let mut state = PrefixState::Idle;
        let now = Instant::now();
        let timeout = Duration::from_millis(750);
        assert_eq!(
            keymap.press(
                &mut state,
                KeyIdentity::Space,
                ActiveModifiers {
                    ctrl: true,
                    ..ActiveModifiers::default()
                },
                10,
                now,
                timeout,
            ),
            KeymapPress::Consumed(None)
        );
        assert!(state.is_armed());
        assert_eq!(
            keymap.press(
                &mut state,
                KeyIdentity::Space,
                ActiveModifiers {
                    ctrl: true,
                    ..ActiveModifiers::default()
                },
                11,
                now + Duration::from_millis(1),
                timeout,
            ),
            KeymapPress::Consumed(Some(ActionId::SendPrefix))
        );
        assert!(!state.is_armed());

        let _ = keymap.press(
            &mut state,
            KeyIdentity::Character('b'),
            ActiveModifiers {
                ctrl: true,
                ..ActiveModifiers::default()
            },
            12,
            now,
            timeout,
        );
        assert_eq!(
            keymap.press(
                &mut state,
                KeyIdentity::Character('a'),
                ActiveModifiers::default(),
                13,
                now + Duration::from_millis(751),
                timeout,
            ),
            KeymapPress::PassThrough
        );

        let _ = keymap.press(
            &mut state,
            KeyIdentity::Space,
            ActiveModifiers {
                ctrl: true,
                ..ActiveModifiers::default()
            },
            14,
            now,
            timeout,
        );
        assert_eq!(
            keymap.press(
                &mut state,
                KeyIdentity::Character('a'),
                ActiveModifiers::default(),
                15,
                now + Duration::from_millis(1),
                timeout,
            ),
            KeymapPress::Consumed(None)
        );
    }

    #[test]
    fn strict_overlay_can_bind_every_lair_lifecycle_action() {
        let path = Path::new("keybindings.toml");
        let text = r#"
version = 1
inherits = "splinterm"
[[binding]]
sequence = ["Ctrl+Alt+S"]
action = "lair.save"
[[binding]]
sequence = ["Ctrl+Alt+F"]
action = "lair.pin-toggle"
[[binding]]
sequence = ["Ctrl+Alt+V"]
action = "lair.preview"
[[binding]]
sequence = ["Ctrl+Alt+O"]
action = "lair.restore"
"#;
        let resolved = resolve_keymap_text(KeymapProfile::Splinterm, text, path).unwrap();
        for (action, shortcut) in [
            (ActionId::SaveCurrentLair, "Ctrl+Alt+S"),
            (ActionId::ToggleCurrentLairPin, "Ctrl+Alt+F"),
            (ActionId::PreviewCurrentLair, "Ctrl+Alt+V"),
            (ActionId::RestoreCurrentLair, "Ctrl+Alt+O"),
        ] {
            assert_eq!(resolved.keymap.primary_shortcut(action), shortcut);
        }
    }

    #[test]
    fn omarchy_overlay_can_replace_a_prefix_sequence() {
        let path = Path::new("keybindings.toml");
        let text = r#"
version = 1
inherits = "omarchy-tmux"
[[unbind]]
sequence = ["Prefix", "x"]
[[binding]]
sequence = ["Prefix", "x"]
action = "pane.zoom-toggle"
"#;
        let resolved = resolve_keymap_text(KeymapProfile::OmarchyTmux, text, path).unwrap();
        assert!(resolved.keymap.bindings().iter().any(|binding| {
            binding.action() == ActionId::TogglePaneZoom && binding.display() == "Prefix X"
        }));
    }
}
