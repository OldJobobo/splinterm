//! Window-local topology commands and updates shared across runtime boundaries.

use std::path::PathBuf;

use splinterm_core::{
    DojoId, LairId, LairRetention, LayoutNode, SplintId, SplitRatio, TopologyRevision,
};
use splinterm_protocol::{MutationTarget, PresetDojoLaunch, PresetTarget};

use super::{SessionPickerItem, ThemeUpdate, WindowPaneOptions};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowDojoIdentity {
    pub topology_revision: TopologyRevision,
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub lair_name: String,
    pub lair_retention: LairRetention,
    pub dojo_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorKind {
    Dojo,
    Lair,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LairDirection {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LairPromptKind {
    Rename,
    Preview,
    Restore,
    Terminate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LairPromptTarget {
    pub topology_revision: TopologyRevision,
    pub lair_id: LairId,
    pub dojo_id: Option<DojoId>,
    pub name: String,
    pub retention: LairRetention,
    pub preview: String,
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
    MaterializePreset {
        target: PresetTarget,
        dojos: Vec<PresetDojoLaunch>,
    },
    NavigateLair {
        current_lair_id: LairId,
        direction: LairDirection,
    },
    RequestLairPrompt {
        lair_id: LairId,
        kind: LairPromptKind,
        expected_retention: Option<LairRetention>,
    },
    RequestDojoRestorePrompt {
        dojo_id: DojoId,
    },
    RenameLair {
        lair_id: LairId,
        name: String,
    },
    TerminateLair {
        lair_id: LairId,
        targets: Vec<MutationTarget>,
    },
    SetLairRetention {
        lair_id: LairId,
        expected_retention: LairRetention,
        retention: LairRetention,
    },
    RestoreLair {
        expected_topology_revision: TopologyRevision,
        lair_id: LairId,
    },
    RestoreDojo {
        expected_topology_revision: TopologyRevision,
        dojo_id: DojoId,
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
        topology_revision: TopologyRevision,
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
