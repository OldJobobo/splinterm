//! Window-local topology commands and updates shared across runtime boundaries.

use std::path::PathBuf;

use splinterm_core::{
    DojoId, LairId, LairRetention, LayoutNode, SplintId, SplitRatio, TopologyRevision,
};
use splinterm_protocol::{MutationTarget, PresetDojoLaunch, PresetTarget};

use super::{FontUpdate, SessionPickerItem, ThemeUpdate, WindowPaneOptions};
use crate::navigation_projection::{
    NavigationAction, NavigationExplorerSplintTarget, NavigationExplorerView, NavigationNodeId,
};

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
pub struct SessionPickerCreationTarget {
    pub topology_revision: TopologyRevision,
    pub action: NavigationAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionPickerTarget {
    pub topology_revision: TopologyRevision,
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub action: NavigationAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LairExplorerActivationTarget {
    Dojo(SessionPickerTarget),
    Splint(NavigationExplorerSplintTarget),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionPickerCatalog {
    pub items: Vec<SessionPickerItem>,
    pub targets: Vec<SessionPickerTarget>,
    pub creation_enabled: bool,
    pub creation_blocker: Option<&'static str>,
    pub creation_target: Option<SessionPickerCreationTarget>,
    pub initial_row: Option<usize>,
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

/// Exact Explorer row authority, retained through menu and confirmation dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExplorerContextTarget {
    pub topology_revision: TopologyRevision,
    pub node: NavigationNodeId,
    pub live_incarnation: Option<u64>,
}

impl ExplorerContextTarget {
    #[must_use]
    pub fn command(self, command: WindowTopologyCommand) -> WindowTopologyCommand {
        WindowTopologyCommand::ExplorerContext {
            target: self,
            command: Box::new(command),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WindowTopologyCommand {
    ExplorerContext {
        target: ExplorerContextTarget,
        command: Box<WindowTopologyCommand>,
    },
    RequestDojoPrompt {
        dojo_id: DojoId,
        kind: LairPromptKind,
    },
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
    RequestLairExplorer {
        focused_splint: Option<SplintId>,
    },
    RequestSelector {
        kind: SelectorKind,
        lair_id: LairId,
    },
    OpenDojo {
        target: SessionPickerTarget,
        explorer_target: Option<LairExplorerActivationTarget>,
    },
    FocusSplint {
        target: NavigationExplorerSplintTarget,
    },
    NewLair {
        cwd: PathBuf,
    },
    PickerNewLair {
        topology_revision: TopologyRevision,
        cwd: PathBuf,
    },
    NewDojo {
        lair_id: LairId,
        cwd: PathBuf,
    },
    PickerNewDojo {
        topology_revision: TopologyRevision,
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
    ShowExplorerPrompt {
        guard: ExplorerContextTarget,
        kind: LairPromptKind,
        target: LairPromptTarget,
    },
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
        explorer_target: Option<LairExplorerActivationTarget>,
    },
    ActivateTab {
        dojo_id: DojoId,
        explorer_target: Option<LairExplorerActivationTarget>,
    },
    ActivateSplint {
        dojo_id: DojoId,
        splint_id: SplintId,
        live_incarnation: Option<u64>,
        explorer_target: Option<LairExplorerActivationTarget>,
    },
    RemoveTab {
        dojo_id: DojoId,
        acknowledged: tokio::sync::oneshot::Sender<()>,
    },
    UpdateIdentity(WindowDojoIdentity),
    TabFailed {
        dojo_id: Option<DojoId>,
        message: String,
        explorer_target: Option<LairExplorerActivationTarget>,
    },
    ShowSessionPicker {
        catalog: SessionPickerCatalog,
    },
    ShowLairExplorer {
        view: NavigationExplorerView,
    },
    ShowSelector {
        kind: SelectorKind,
        catalog: SessionPickerCatalog,
    },
    ShowLairPrompt {
        kind: LairPromptKind,
        target: LairPromptTarget,
    },
    SessionPickerFailed(String),
    LairExplorerFailed,
    Theme(ThemeUpdate),
    Font(FontUpdate),
    Closed,
    Shutdown(String),
}
