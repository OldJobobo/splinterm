//! Window-local topology commands and updates shared across runtime boundaries.

use std::path::PathBuf;

use splinterm_core::{DojoId, LairId, LayoutNode, SplintId, SplitRatio};
use splinterm_protocol::MutationTarget;

use super::{SessionPickerItem, ThemeUpdate, WindowPaneOptions};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowDojoIdentity {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub lair_name: String,
    pub dojo_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorKind {
    Dojo,
    LairDojo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LairDirection {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LairPromptKind {
    Rename,
    Terminate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LairPromptTarget {
    pub lair_id: LairId,
    pub name: String,
    pub targets: Vec<MutationTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WindowTopologyCommand {
    Split {
        dojo_id: DojoId,
        target: SplintId,
        axis: splinterm_core::Axis,
        /// Client-local placeholder already rendered for a remote split.
        pending: Option<SplintId>,
    },
    Close {
        dojo_id: DojoId,
        target: SplintId,
    },
    AdjustRatio {
        dojo_id: DojoId,
        target: SplintId,
        delta: i16,
    },
    SetRatio {
        dojo_id: DojoId,
        target: SplintId,
        ancestor: u16,
        ratio: SplitRatio,
    },
    RequestSessionPicker,
    RequestSelector {
        kind: SelectorKind,
        lair_id: LairId,
    },
    OpenDojo {
        lair_id: LairId,
        dojo_id: DojoId,
    },
    NewLair {
        cwd: PathBuf,
    },
    NewDojo {
        lair_id: LairId,
        cwd: PathBuf,
    },
    NavigateLair {
        current_lair_id: LairId,
        direction: LairDirection,
    },
    RequestLairPrompt {
        lair_id: LairId,
        kind: LairPromptKind,
    },
    RenameLair {
        lair_id: LairId,
        name: String,
    },
    TerminateLair {
        lair_id: LairId,
        targets: Vec<MutationTarget>,
    },
    RenameDojo {
        dojo_id: DojoId,
        name: String,
    },
    TerminateDojo {
        dojo_id: DojoId,
        splints: Vec<(SplintId, u64)>,
    },
    ActivateTab {
        dojo_id: DojoId,
    },
    CloseTab {
        dojo_id: DojoId,
    },
    CloseTabs {
        retain_dojo_id: DojoId,
        dojo_ids: Vec<DojoId>,
    },
}

pub enum WindowTopologyUpdate {
    Apply {
        dojo_id: DojoId,
        layout: LayoutNode,
        added: Vec<WindowPaneOptions>,
        removed: Vec<SplintId>,
        focused: Option<SplintId>,
    },
    OpenTab {
        identity: WindowDojoIdentity,
        layout: LayoutNode,
        panes: Vec<WindowPaneOptions>,
        focused: SplintId,
        acknowledged: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    },
    ActivateTab {
        dojo_id: DojoId,
    },
    RemoveTab {
        dojo_id: DojoId,
        acknowledged: tokio::sync::oneshot::Sender<()>,
    },
    UpdateIdentity(WindowDojoIdentity),
    TabFailed {
        dojo_id: Option<DojoId>,
        message: String,
    },
    ShowSessionPicker {
        items: Vec<SessionPickerItem>,
        targets: Vec<(LairId, DojoId)>,
    },
    ShowSelector {
        kind: SelectorKind,
        items: Vec<SessionPickerItem>,
        targets: Vec<(LairId, DojoId)>,
    },
    ShowLairPrompt {
        kind: LairPromptKind,
        target: LairPromptTarget,
    },
    SessionPickerFailed(String),
    Theme(ThemeUpdate),
    Closed,
    Shutdown(String),
}
