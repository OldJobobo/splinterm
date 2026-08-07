//! Window-local topology commands and updates shared across runtime boundaries.

use splinterm_core::{DojoId, LairId, LayoutNode, SplintId, SplitRatio};

use super::{SessionPickerItem, ThemeUpdate, WindowPaneOptions};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowDojoIdentity {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub lair_name: String,
    pub dojo_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WindowTopologyCommand {
    Split {
        dojo_id: DojoId,
        target: SplintId,
        axis: splinterm_core::Axis,
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
    OpenDojo {
        lair_id: LairId,
        dojo_id: DojoId,
    },
    NewLair,
    NewDojo {
        lair_id: LairId,
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
    SessionPickerFailed(String),
    Theme(ThemeUpdate),
    Closed,
    Shutdown(String),
}
