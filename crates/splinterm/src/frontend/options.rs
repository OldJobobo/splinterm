//! Platform-independent construction options for graphical windows and panes.

use std::{path::PathBuf, sync::mpsc::Sender as StdSender, time::Instant};

use tokio::sync::{
    mpsc::{Receiver, Sender},
    oneshot,
    watch::Sender as WatchSender,
};

use splinterm_automation_client::ImageContentLeaseSet;
use splinterm_core::{LayoutNode, SplintId};
use splinterm_protocol::{ConsentDecision, GrantDuration, MAX_COLUMNS, MAX_ROWS, TerminalSnapshot};

use crate::{
    config::{
        APP_ID, CursorStyle, FrameTitleMode, PaneDividerStyle, ResolvedTheme,
        TopologyReloadSettings,
    },
    keymap::ResolvedKeymap,
};

use super::{
    AuthorityStatus, SessionPickerUi, TrustedDialogKey, TrustedDialogKind, WindowCommand,
    WindowDojoIdentity, WindowTopologyCommand, WindowTopologyUpdate, WindowUpdate,
    trusted_dialog::{AccessChoice, ConsentAction},
};

/// One rendering of the access consent dialog.
pub struct AccessConsentView {
    pub snapshot: TerminalSnapshot,
    /// The snapshot row of each shown duration, in order, so a click on that
    /// row selects it.
    pub duration_rows: Vec<usize>,
}

/// Renders the access consent dialog with the duration at this index
/// selected.
pub type AccessConsentRenderer = Box<dyn Fn(usize) -> AccessConsentView + Send>;

/// What a key press or click did to a trusted dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrustedInput {
    /// The person answered: `true` grants or proceeds.
    Answer(bool),
    /// The selected duration changed; the dialog must be drawn again.
    Redraw,
}

enum TrustedAnswer {
    YesNo(StdSender<bool>),
    Consent(StdSender<ConsentDecision>),
}

struct AccessConsentDialog {
    choice: AccessChoice,
    durations: Vec<GrantDuration>,
    duration_rows: Vec<usize>,
    render: AccessConsentRenderer,
}

/// A trusted dialog Window's answer channel and kind. Build it with
/// [`TrustedConsentUi::access_consent`], [`TrustedConsentUi::upgrade`], or
/// [`TrustedConsentUi::restart`].
pub struct TrustedConsentUi {
    answer: TrustedAnswer,
    pub(crate) kind: TrustedDialogKind,
    /// When the Window first gained keyboard focus; the upgrade question
    /// arms itself from it.
    pub(crate) focused_at: Option<Instant>,
    access: Option<AccessConsentDialog>,
}

impl TrustedConsentUi {
    /// The access consent dialog (ADR 0014): default-deny, armed after every
    /// focus gain and duration change, with the durations the prompt shows
    /// (`true` when offered; the first is "Just this once") and a renderer
    /// for each selection. Returns the dialog and its first rendering.
    #[must_use]
    pub fn access_consent(
        decision: StdSender<ConsentDecision>,
        durations: Vec<(GrantDuration, bool)>,
        render: AccessConsentRenderer,
    ) -> (Self, TerminalSnapshot) {
        let view = render(0);
        let (durations, offered) = durations.into_iter().unzip();
        let dialog = Self {
            answer: TrustedAnswer::Consent(decision),
            kind: TrustedDialogKind::AccessConsent,
            focused_at: None,
            access: Some(AccessConsentDialog {
                choice: AccessChoice::new(offered),
                durations,
                duration_rows: view.duration_rows,
                render,
            }),
        };
        (dialog, view.snapshot)
    }

    /// The upgrade question: only `G` or the green area proceeds, after an
    /// arming delay; `Enter`, `Escape` and `D` cancel.
    #[must_use]
    pub fn upgrade(decision: StdSender<bool>) -> Self {
        Self {
            answer: TrustedAnswer::YesNo(decision),
            kind: TrustedDialogKind::Upgrade,
            focused_at: None,
            access: None,
        }
    }

    /// The launcher's restart question: only `R` or the green area restarts,
    /// after the same arming delay; `Enter`, `Escape` and `D` answer "Not now".
    #[must_use]
    pub fn restart(decision: StdSender<bool>) -> Self {
        Self {
            answer: TrustedAnswer::YesNo(decision),
            kind: TrustedDialogKind::Restart,
            focused_at: None,
            access: None,
        }
    }

    /// Records keyboard focus. The yes/no questions arm from the first
    /// focus; the access consent dialog arms again on every one.
    pub(crate) fn note_focus(&mut self, now: Instant) {
        self.focused_at.get_or_insert(now);
        if let Some(access) = self.access.as_mut() {
            access.choice.note_focus(now);
        }
    }

    fn consent_input(
        &mut self,
        action: Option<ConsentAction>,
        now: Instant,
    ) -> Option<TrustedInput> {
        let access = self.access.as_mut()?;
        match action? {
            ConsentAction::Deny => Some(TrustedInput::Answer(false)),
            ConsentAction::Grant => Some(TrustedInput::Answer(true)),
            ConsentAction::Select(index) => access
                .choice
                .select(index, now)
                .then_some(TrustedInput::Redraw),
        }
    }

    pub(crate) fn key_input(
        &mut self,
        key: TrustedDialogKey,
        repeat: bool,
        now: Instant,
    ) -> Option<TrustedInput> {
        if let Some(access) = &self.access {
            let action = access.choice.key(key, repeat, now);
            return self.consent_input(action, now);
        }
        self.kind
            .key_decision(key, repeat, self.focused_at, now)
            .map(TrustedInput::Answer)
    }

    /// A click in the bottom action areas: the right one grants or proceeds.
    pub(crate) fn action_area_input(
        &mut self,
        proceed_area: bool,
        now: Instant,
    ) -> Option<TrustedInput> {
        if let Some(access) = &self.access {
            let action = access.choice.click_action_area(proceed_area, now);
            return self.consent_input(action, now);
        }
        self.kind
            .click_decision(proceed_area, self.focused_at, now)
            .map(TrustedInput::Answer)
    }

    /// A click on a snapshot row: selects the duration shown there.
    pub(crate) fn row_input(&mut self, row: usize, now: Instant) -> Option<TrustedInput> {
        let access = self.access.as_ref()?;
        let index = access
            .duration_rows
            .iter()
            .position(|shown| *shown == row)?;
        let action = access.choice.click_duration(index);
        self.consent_input(action, now)
    }

    /// The access consent dialog rendered for its current selection.
    pub(crate) fn render(&mut self) -> Option<TerminalSnapshot> {
        let access = self.access.as_mut()?;
        let view = (access.render)(access.choice.selected());
        access.duration_rows = view.duration_rows;
        Some(view.snapshot)
    }

    /// Sends the answer: for the access consent dialog, a grant of the
    /// selected duration or a deny.
    pub(crate) fn answer(self, proceed: bool) {
        match self.answer {
            TrustedAnswer::YesNo(decision) => {
                let _ = decision.send(proceed);
            }
            TrustedAnswer::Consent(decision) => {
                let duration = self
                    .access
                    .as_ref()
                    .and_then(|access| access.durations.get(access.choice.selected()).copied());
                let _ = decision.send(match (proceed, duration) {
                    (true, Some(duration)) => ConsentDecision::Grant { duration },
                    _ => ConsentDecision::Deny,
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalGridLimits {
    pub maximum_columns: u16,
    pub maximum_rows: u16,
}

impl Default for TerminalGridLimits {
    fn default() -> Self {
        Self {
            maximum_columns: MAX_COLUMNS,
            maximum_rows: MAX_ROWS,
        }
    }
}

pub struct WindowPaneOptions {
    pub snapshot: TerminalSnapshot,
    pub terminal_grid_limits: TerminalGridLimits,
    pub updates: Receiver<WindowUpdate>,
    pub commands: Sender<WindowCommand>,
    pub authority: AuthorityStatus,
    pub controlled: bool,
    pub image_sources: ImageContentLeaseSet,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent renderer, input, capture, and endpoint capability flags"
)]
pub struct WindowOptions {
    pub capture: Option<PathBuf>,
    /// Initial owned daemon snapshot. `None` retains the deterministic evidence row.
    pub snapshot: Option<TerminalSnapshot>,
    /// Exact renderer leases paired with the legacy initial snapshot.
    pub image_sources: ImageContentLeaseSet,
    /// Bounded live-update receiver owned by the Wayland thread.
    pub updates: Option<Receiver<WindowUpdate>>,
    /// Bounded command sender from the Wayland thread to the async protocol owner.
    pub commands: Option<Sender<WindowCommand>>,
    /// Retain Q/Escape close shortcuts only for the renderer evidence example.
    pub evidence_close_shortcuts: bool,
    /// Delay capture until this integer output scale is active.
    pub capture_scale: Option<u32>,
    /// Trusted application-owned consent mode. Terminal content cannot enable it.
    pub trusted_consent: Option<TrustedConsentUi>,
    /// Application-owned recent-Dojo picker. Terminal content cannot enable it.
    pub session_picker: Option<SessionPickerUi>,
    /// Trusted authority state rendered in persistent application chrome.
    pub authority: AuthorityStatus,
    /// Whether the legacy single-pane command channel already owns control.
    pub controlled: bool,
    /// Whether the title names a controlling Window "local controller". Only
    /// the title: input still follows `controlled`. The remote connection
    /// status Window turns it off because it controls no terminal.
    pub controller_title: bool,
    /// Endpoint-local terminal limits for the legacy or activated focused pane.
    pub terminal_grid_limits: TerminalGridLimits,
    /// Initial terminal dimensions from the supported configuration subset.
    pub initial_columns: u16,
    pub initial_rows: u16,
    /// Configured cursor presentation policy.
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    /// Optional fixed user title; terminal OSC titles remain active when absent.
    pub title: Option<String>,
    /// Client-local Wayland identity. Only private XDG launch may override the default.
    pub app_id: String,
    /// Current project-owned Omarchy role mapping.
    pub theme: ResolvedTheme,
    pub pane_divider_style: PaneDividerStyle,
    pub frame_title_mode: FrameTitleMode,
    /// Fully validated client-local keymap for this Window.
    pub keymap: ResolvedKeymap,
    /// Prefix sequence timeout from the validated client configuration.
    pub prefix_timeout_ms: u64,
    /// Explicit opt-in destination; saves also require a local endpoint.
    pub clipboard_image_directory: Option<PathBuf>,
    /// Fixed endpoint authority, not inferred from a UI capability or terminal data.
    pub local_endpoint: bool,
    /// Multi-pane input. Empty retains the legacy one-pane fields above.
    pub panes: Vec<WindowPaneOptions>,
    pub layout: Option<LayoutNode>,
    /// Client-local initial focus; never written back implicitly.
    pub active_splint: Option<SplintId>,
    pub topology_updates: Option<Receiver<WindowTopologyUpdate>>,
    pub topology_commands: Option<Sender<WindowTopologyCommand>>,
    /// Where Reload configuration publishes the settings the topology manager
    /// reads; shared state, so a busy command queue cannot drop a reload.
    pub topology_settings: Option<WatchSender<TopologyReloadSettings>>,
    /// Coalesced ephemeral keyboard/pane focus publication for the supported adapter API.
    pub graphical_focus: Option<WatchSender<Option<SplintId>>>,
    /// Whether trusted graphical force-transfer actions may be offered or dispatched.
    pub forced_control_transfer: bool,
    /// Render a client-local placeholder before dispatching a remote split.
    pub optimistic_remote_splits: bool,
    /// Stable identity for the initial managed Dojo; absent for legacy/evidence windows.
    pub initial_dojo: Option<WindowDojoIdentity>,
    /// Initial tab-strip policy for managed Dojo windows.
    pub initial_tab_strip_visible: bool,
    /// Remote daemon identity from this Window's initial connection; never terminal content.
    pub remote_display_identity: Option<crate::endpoint::RemoteDisplayIdentity>,
    /// `[explorer] agent-status`: show explicitly reported agent status.
    pub agent_status_presentation: bool,
    /// Whether the Window binds the text-input (IME) protocol. The remote
    /// connection status Window turns it off so an input method never sees or
    /// draws a typed password as composing text.
    pub input_method: bool,
    /// Fired once, after the first buffer commit maps the Window. Dropped unfired
    /// when the Window ends without mapping.
    pub mapped: Option<oneshot::Sender<()>>,
    /// The Window's daemon handoff status, fed by its panes' control
    /// subscriptions; drives the upgrade banner and input pause (Plan 0061
    /// M3.12a). `None` for Windows that hold no daemon panes.
    pub handoff_status: Option<crate::handoff_status::HandoffStatusHub>,
    /// Where a local managed Window publishes its tabs and focus each
    /// event-loop turn, for the resume record its handoff anchor seals
    /// (Plan 0061 M3.14). `None` for every other Window.
    pub layout_ledger: Option<crate::window_continuation::WindowLayoutLedger>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            capture: None,
            snapshot: None,
            image_sources: ImageContentLeaseSet::default(),
            updates: None,
            commands: None,
            evidence_close_shortcuts: false,
            capture_scale: None,
            trusted_consent: None,
            session_picker: None,
            authority: AuthorityStatus::default(),
            controlled: true,
            controller_title: true,
            terminal_grid_limits: TerminalGridLimits::default(),
            initial_columns: 80,
            initial_rows: 24,
            cursor_style: CursorStyle::Block,
            cursor_blink: true,
            title: None,
            app_id: APP_ID.to_owned(),
            theme: ResolvedTheme::default(),
            pane_divider_style: PaneDividerStyle::Line,
            frame_title_mode: FrameTitleMode::Splint,
            keymap: ResolvedKeymap::default(),
            prefix_timeout_ms: 1_000,
            clipboard_image_directory: None,
            local_endpoint: false,
            panes: Vec::new(),
            layout: None,
            active_splint: None,
            topology_updates: None,
            topology_commands: None,
            topology_settings: None,
            graphical_focus: None,
            forced_control_transfer: true,
            optimistic_remote_splits: false,
            initial_dojo: None,
            initial_tab_strip_visible: true,
            remote_display_identity: None,
            agent_status_presentation: true,
            input_method: true,
            mapped: None,
            handoff_status: None,
            layout_ledger: None,
        }
    }
}
