//! Closed application actions and strictly resolved keymaps.
//!
//! Configuration selects from this action vocabulary; it cannot register
//! callbacks, shell snippets, or other executable behavior.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const BUILT_IN_PROFILE_NAMES: &[&str] = &["splinterm"];

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
    SplitHorizontal,
    SplitVertical,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CloseFocusedPane,
    ResizePaneSmaller,
    ResizePaneLarger,
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
        Self::SplitHorizontal,
        Self::SplitVertical,
        Self::FocusLeft,
        Self::FocusRight,
        Self::FocusUp,
        Self::FocusDown,
        Self::CloseFocusedPane,
        Self::ResizePaneSmaller,
        Self::ResizePaneLarger,
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
            Self::SplitHorizontal => "pane.split-below",
            Self::SplitVertical => "pane.split-right",
            Self::FocusLeft => "pane.focus-left",
            Self::FocusRight => "pane.focus-right",
            Self::FocusUp => "pane.focus-up",
            Self::FocusDown => "pane.focus-down",
            Self::CloseFocusedPane => "pane.close",
            Self::ResizePaneSmaller => "pane.resize-smaller",
            Self::ResizePaneLarger => "pane.resize-larger",
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
}

impl KeymapProfile {
    /// Parses one packaged profile name.
    ///
    /// # Errors
    /// Returns an error listing available profiles when the name is unknown.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "splinterm" => Ok(Self::Splinterm),
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
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyIdentity {
    Character(char),
    Tab,
    Enter,
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
    Plus,
    Equal,
    Minus,
    Zero,
}

impl KeyIdentity {
    fn display(self) -> String {
        match self {
            Self::Character(character) => character.to_ascii_uppercase().to_string(),
            Self::Tab => "Tab".to_owned(),
            Self::Enter => "Enter".to_owned(),
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
            Self::Plus => "Plus".to_owned(),
            Self::Equal => "Equal".to_owned(),
            Self::Minus => "Minus".to_owned(),
            Self::Zero => "0".to_owned(),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBinding {
    chord: KeyChord,
    normalized: NormalizedChord,
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
    pub const fn normalized(&self) -> NormalizedChord {
        self.normalized
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedKeymap {
    profile: KeymapProfile,
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
            .find(|binding| binding.chord.key == key && binding.chord.modifiers.matches(modifiers))
            .map(|binding| binding.action)
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

fn built_in_binding(
    modifiers: ModifierPattern,
    normalized: NormalizedChord,
    key: KeyIdentity,
    action: ActionId,
    display: &str,
) -> ResolvedBinding {
    ResolvedBinding {
        chord: KeyChord { modifiers, key },
        normalized,
        action,
        display: display.to_owned(),
        source: BindingSource::BuiltIn {
            profile: "splinterm",
        },
    }
}

fn normalized(ctrl: bool, shift: bool, key: KeyIdentity) -> NormalizedChord {
    NormalizedChord {
        modifiers: ActiveModifiers {
            ctrl,
            shift,
            alt: false,
            logo: false,
        },
        key,
    }
}

#[must_use]
pub fn built_in_keymap(profile: KeymapProfile) -> ResolvedKeymap {
    match profile {
        KeymapProfile::Splinterm => ResolvedKeymap {
            profile,
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
    let mut bindings = built_in_keymap(inherited_profile).bindings;
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
        bindings.retain(|binding| binding.normalized != chord);
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
        let chord = KeyChord {
            modifiers: ModifierPattern::exact(normalized.modifiers),
            key: normalized.key,
        };
        if let Some(existing) = bindings
            .iter()
            .find(|existing| chords_overlap(existing.chord, chord))
        {
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
            normalized,
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

fn parse_sequence(sequence: &[String]) -> Result<NormalizedChord> {
    if sequence.len() != 1 {
        if sequence.first().is_some_and(|part| part == "Prefix") {
            bail!("prefix sequences are not available until the prefix-key milestone");
        }
        bail!("sequence must contain exactly one direct chord");
    }
    parse_chord(&sequence[0])
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
        "plus" => KeyIdentity::Plus,
        "equal" | "=" => KeyIdentity::Equal,
        "minus" | "-" => KeyIdentity::Minus,
        "0" | "kp_0" => KeyIdentity::Zero,
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
        for (index, left) in keymap.bindings().iter().enumerate() {
            for right in &keymap.bindings()[index + 1..] {
                assert!(
                    !chords_overlap(left.chord, right.chord),
                    "overlapping chords: {} and {}",
                    left.display,
                    right.display
                );
            }
        }
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
        assert!(parse_chord("Ctrl+?").is_err());
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
            "Ctrl+?",
            "Ctrl+1",
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
            "version = 1\n[[binding]]\nsequence = [\"Ctrl+?\"]\naction = \"dojo.new\"\n[[binding]]\nsequence = [\"Ctrl+A\"]\naction = \"shell.run\"",
            path,
        )
        .unwrap_err()
        .to_string();
        assert!(multiple.contains("unsupported printable key"));
        assert!(multiple.contains("unknown or unavailable action"));
    }

    #[test]
    fn unmatched_unbind_is_a_diagnostic_and_prefix_is_not_silently_accepted() {
        let path = Path::new("keybindings.toml");
        let unmatched = "version = 1\n[[unbind]]\nsequence = [\"Ctrl+A\"]";
        let resolved = resolve_keymap_text(KeymapProfile::Splinterm, unmatched, path).unwrap();
        assert_eq!(resolved.diagnostics.len(), 1);
        let prefix =
            "version = 1\n[[binding]]\nsequence = [\"Prefix\", \"c\"]\naction = \"dojo.new\"";
        assert!(resolve_keymap_text(KeymapProfile::Splinterm, prefix, path).is_err());
    }
}
