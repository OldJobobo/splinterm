//! Pure, bounded navigation projection shared by picker and explorer policy views.
//!
//! The daemon-owned topology remains authoritative. This module combines one
//! validated snapshot with current-Window presentation facts, but does not own
//! selection, expansion, search, row geometry, or dispatch.

use std::collections::{HashMap, HashSet};

use splinterm_core::{
    Dojo, DojoId, LairId, LairRetention, LayoutNode, MAX_DOJOS_PER_LAIR, MAX_LAYOUT_DEPTH,
    MAX_PERSISTENT_LAIRS, MAX_SPLINTS, SplintId, TopologyRevision,
};
use splinterm_protocol::{
    ProcessExitStatus, SplintLifecycle, SplintRuntimeSummary, TopologySnapshot,
};
use unicode_width::UnicodeWidthChar;

const MAX_LABEL_SCALARS: usize = 256;
const MAX_LABEL_CELLS: usize = 160;
const MAX_CWD_SCALARS: usize = 512;
const MAX_CWD_CELLS: usize = 240;

/// Freshness of the endpoint data from which a projection was derived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointFreshness {
    Current,
    Refreshing,
    Stale,
    Disconnected,
}

/// Stable identity for any row in the containment hierarchy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NavigationNodeId {
    Lair(LairId),
    Dojo {
        lair_id: LairId,
        dojo_id: DojoId,
    },
    Splint {
        lair_id: LairId,
        dojo_id: DojoId,
        splint_id: SplintId,
    },
}

/// Lifecycle state presented without inferring process or agent activity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationLifecycle {
    Starting,
    Running,
    Mixed,
    Exited,
    Restorable,
    Unavailable,
}

/// Relationship between a Dojo and the current Window only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowAttachment {
    Here,
    NotHere,
}

/// Closed set of actions a policy surface may expose for a projected node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationAction {
    ActivateDojo,
    AttachDojo,
    FocusSplint,
    AttachAndFocusSplint,
    PreviewRestoreDojo,
    PreviewRestoreSplint,
    CreateLair,
    CreateDojo,
}

/// Bounded reason that an otherwise relevant capability is unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationBlocker {
    Refreshing,
    Stale,
    Disconnected,
    PermissionDenied,
    TabCapacityReached,
}

impl NavigationBlocker {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Refreshing => "Refreshing topology",
            Self::Stale => "Topology is stale",
            Self::Disconnected => "Endpoint is disconnected",
            Self::PermissionDenied => "Permission denied",
            Self::TabCapacityReached => "Window tab capacity reached",
        }
    }
}

/// Whether a typed capability may execute from this projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationAvailability {
    Enabled,
    Disabled(NavigationBlocker),
}

/// One typed action and its bounded availability state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavigationCapability {
    pub action: NavigationAction,
    pub availability: NavigationAvailability,
}

impl NavigationCapability {
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        matches!(self.availability, NavigationAvailability::Enabled)
    }
}

/// Sanitized identity and display data common to every projected node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationNode {
    pub id: NavigationNodeId,
    pub parent: Option<NavigationNodeId>,
    pub label: String,
    pub lifecycle: NavigationLifecycle,
    pub capabilities: Vec<NavigationCapability>,
}

/// One projected Splint in canonical layout traversal order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationSplint {
    pub node: NavigationNode,
    pub working_directory: String,
    pub live_incarnation: Option<u64>,
    pub last_incarnation: Option<u64>,
    pub exit_status: Option<ProcessExitStatus>,
    pub focused_here: bool,
}

/// One projected Dojo in canonical daemon order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationDojo {
    pub node: NavigationNode,
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub default_focus: SplintId,
    pub working_directory: String,
    pub attachment: WindowAttachment,
    pub active_here: bool,
    pub splints: Vec<NavigationSplint>,
}

/// One persistent projected Lair in canonical daemon order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationLair {
    pub node: NavigationNode,
    pub lair_id: LairId,
    pub retention: LairRetention,
    pub dojos: Vec<NavigationDojo>,
}

/// New-Dojo capability tied to the captured current Lair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavigationDojoCreation {
    pub lair_id: LairId,
    pub capability: NavigationCapability,
}

/// Creation capabilities which are not synthetic navigation rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavigationCreationCapabilities {
    pub new_lair: NavigationCapability,
    pub new_dojo: Option<NavigationDojoCreation>,
}

/// Immutable, bounded source shared by picker and explorer policy views.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationProjection {
    pub topology_revision: TopologyRevision,
    pub endpoint_namespace: String,
    pub endpoint_generation: u64,
    pub freshness: EndpointFreshness,
    pub creation: NavigationCreationCapabilities,
    pub lairs: Vec<NavigationLair>,
}

/// Current-Window facts which are not daemon containment relationships.
#[derive(Clone, Copy, Debug)]
pub struct NavigationWindowState<'a> {
    pub attached_dojos: &'a [DojoId],
    pub active_dojo: Option<DojoId>,
    pub focused_splint: Option<SplintId>,
    pub current_lair: Option<LairId>,
    pub has_tab_capacity: bool,
}

/// Typed permission input used to derive enabled and disabled capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationPermission {
    Allowed,
    Denied,
}

impl NavigationPermission {
    const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// Bounded authority inputs used to derive node capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavigationAuthority {
    pub attach: NavigationPermission,
    pub restore: NavigationPermission,
    pub create_lair: NavigationPermission,
    pub create_dojo: NavigationPermission,
}

/// Inputs that vary independently from daemon topology.
#[derive(Clone, Copy, Debug)]
pub struct NavigationProjectionContext<'a> {
    pub endpoint_namespace: &'a str,
    pub endpoint_generation: u64,
    pub freshness: EndpointFreshness,
    pub window: NavigationWindowState<'a>,
    pub authority: NavigationAuthority,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationProjectionError {
    InvalidSnapshot,
    InvalidEndpointNamespace,
}

impl std::fmt::Display for NavigationProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSnapshot => formatter.write_str("navigation snapshot is invalid"),
            Self::InvalidEndpointNamespace => {
                formatter.write_str("navigation endpoint namespace is invalid")
            }
        }
    }
}

impl std::error::Error for NavigationProjectionError {}

impl NavigationProjection {
    /// Builds a projection from one validated topology/runtime snapshot.
    ///
    /// Transient Lairs are intentionally absent. Unknown Window-local identities
    /// are ignored because they can race a newer daemon snapshot; callers retain
    /// those identities separately when they need stale-target reconciliation.
    ///
    /// # Errors
    ///
    /// Returns [`NavigationProjectionError::InvalidSnapshot`] when topology and
    /// runtime identities, incarnations, or revisions are inconsistent, or
    /// [`NavigationProjectionError::InvalidEndpointNamespace`] when endpoint
    /// identity is empty, oversized, or contains unsafe characters.
    pub fn build(
        snapshot: &TopologySnapshot,
        context: NavigationProjectionContext<'_>,
    ) -> Result<Self, NavigationProjectionError> {
        snapshot
            .validate()
            .map_err(|_| NavigationProjectionError::InvalidSnapshot)?;
        if !valid_projection_structure(snapshot, context.window) {
            return Err(NavigationProjectionError::InvalidSnapshot);
        }
        if !valid_endpoint_namespace(context.endpoint_namespace) {
            return Err(NavigationProjectionError::InvalidEndpointNamespace);
        }

        let runtimes: HashMap<_, _> = snapshot
            .runtimes
            .iter()
            .map(|runtime| (runtime.splint_id, runtime))
            .collect();
        let attached: HashSet<_> = context.window.attached_dojos.iter().copied().collect();
        let mut lairs = Vec::new();

        for lair in snapshot
            .topology
            .lairs()
            .filter(|lair| lair.lifetime.is_persistent())
        {
            let lair_id = lair.id;
            let lair_node_id = NavigationNodeId::Lair(lair_id);
            let dojos = lair
                .dojos
                .iter()
                .map(|dojo| {
                    project_dojo(dojo, lair_id, lair_node_id, &attached, &runtimes, context)
                })
                .collect::<Vec<_>>();
            let lifecycle = aggregate_lifecycle(dojos.iter().map(|dojo| dojo.node.lifecycle));
            lairs.push(NavigationLair {
                node: NavigationNode {
                    id: lair_node_id,
                    parent: None,
                    label: bounded_label(&lair.name),
                    lifecycle,
                    capabilities: Vec::new(),
                },
                lair_id,
                retention: lair.retention,
                dojos,
            });
        }

        Ok(Self {
            topology_revision: snapshot.revision,
            endpoint_namespace: context.endpoint_namespace.to_owned(),
            endpoint_generation: context.endpoint_generation,
            freshness: context.freshness,
            creation: project_creation(snapshot, context),
            lairs,
        })
    }
}

fn project_creation(
    snapshot: &TopologySnapshot,
    context: NavigationProjectionContext<'_>,
) -> NavigationCreationCapabilities {
    let new_lair = capability(
        NavigationAction::CreateLair,
        context.freshness,
        context.authority.create_lair.is_allowed(),
        true,
        context.window.has_tab_capacity,
    );
    let new_dojo = context.window.current_lair.and_then(|lair_id| {
        snapshot
            .topology
            .find_lair(lair_id)
            .map(|_| NavigationDojoCreation {
                lair_id,
                capability: capability(
                    NavigationAction::CreateDojo,
                    context.freshness,
                    context.authority.create_dojo.is_allowed(),
                    true,
                    context.window.has_tab_capacity,
                ),
            })
    });
    NavigationCreationCapabilities { new_lair, new_dojo }
}

const fn capability(
    action: NavigationAction,
    freshness: EndpointFreshness,
    permitted: bool,
    needs_capacity: bool,
    has_capacity: bool,
) -> NavigationCapability {
    let blocker = match freshness {
        EndpointFreshness::Current if !permitted => Some(NavigationBlocker::PermissionDenied),
        EndpointFreshness::Current if needs_capacity && !has_capacity => {
            Some(NavigationBlocker::TabCapacityReached)
        }
        EndpointFreshness::Current => None,
        EndpointFreshness::Refreshing => Some(NavigationBlocker::Refreshing),
        EndpointFreshness::Stale => Some(NavigationBlocker::Stale),
        EndpointFreshness::Disconnected => Some(NavigationBlocker::Disconnected),
    };
    NavigationCapability {
        action,
        availability: match blocker {
            Some(blocker) => NavigationAvailability::Disabled(blocker),
            None => NavigationAvailability::Enabled,
        },
    }
}

fn valid_projection_structure(
    snapshot: &TopologySnapshot,
    window: NavigationWindowState<'_>,
) -> bool {
    if window.attached_dojos.len() > crate::tab::MAX_WINDOW_TABS {
        return false;
    }
    let mut attached = HashSet::new();
    if !window
        .attached_dojos
        .iter()
        .all(|dojo_id| attached.insert(*dojo_id))
    {
        return false;
    }

    let mut lair_ids = HashSet::new();
    let mut dojo_ids = HashSet::new();
    let mut splint_ids = HashSet::new();
    let mut persistent_lairs = 0_usize;
    let mut persistent_splints = 0_usize;
    for lair in snapshot.topology.lairs() {
        if !lair_ids.insert(lair.id)
            || !snapshot
                .topology
                .find_lair(lair.id)
                .is_some_and(|resolved| std::ptr::eq(resolved, lair))
        {
            return false;
        }
        if lair.lifetime.is_persistent() {
            persistent_lairs += 1;
            if persistent_lairs > MAX_PERSISTENT_LAIRS || lair.dojos.len() > MAX_DOJOS_PER_LAIR {
                return false;
            }
        }
        for dojo in &lair.dojos {
            if !dojo_ids.insert(dojo.id) || dojo.root.find_splint(dojo.default_focus).is_none() {
                return false;
            }
            if !validate_layout(
                &dojo.root,
                1,
                lair.lifetime.is_persistent(),
                &mut persistent_splints,
                &mut splint_ids,
            ) {
                return false;
            }
        }
    }
    true
}

fn validate_layout(
    layout: &LayoutNode,
    depth: usize,
    persistent: bool,
    persistent_splints: &mut usize,
    splint_ids: &mut HashSet<SplintId>,
) -> bool {
    if depth > MAX_LAYOUT_DEPTH {
        return false;
    }
    match layout {
        LayoutNode::Leaf(splint) => {
            if !splint_ids.insert(splint.id) {
                return false;
            }
            if persistent {
                *persistent_splints += 1;
                *persistent_splints <= MAX_SPLINTS
            } else {
                true
            }
        }
        LayoutNode::Branch { first, second, .. } => {
            validate_layout(first, depth + 1, persistent, persistent_splints, splint_ids)
                && validate_layout(
                    second,
                    depth + 1,
                    persistent,
                    persistent_splints,
                    splint_ids,
                )
        }
    }
}

fn project_dojo(
    dojo: &Dojo,
    lair_id: LairId,
    lair_node_id: NavigationNodeId,
    attached: &HashSet<DojoId>,
    runtimes: &HashMap<SplintId, &SplintRuntimeSummary>,
    context: NavigationProjectionContext<'_>,
) -> NavigationDojo {
    let dojo_id = dojo.id;
    let dojo_node_id = NavigationNodeId::Dojo { lair_id, dojo_id };
    let attachment = if attached.contains(&dojo_id) {
        WindowAttachment::Here
    } else {
        WindowAttachment::NotHere
    };
    let active_here =
        attachment == WindowAttachment::Here && context.window.active_dojo == Some(dojo_id);
    let detached_parent_reopenable =
        attachment == WindowAttachment::NotHere && layout_is_fully_running(&dojo.root, runtimes);
    let mut splints = Vec::with_capacity(dojo.root.splint_count());
    collect_splints(
        &dojo.root,
        lair_id,
        dojo_id,
        dojo_node_id,
        attachment,
        active_here,
        detached_parent_reopenable,
        runtimes,
        context,
        &mut splints,
    );
    let lifecycle = aggregate_lifecycle(splints.iter().map(|splint| splint.node.lifecycle));
    let mut capabilities = Vec::new();
    if attachment == WindowAttachment::Here
        && matches!(
            lifecycle,
            NavigationLifecycle::Starting
                | NavigationLifecycle::Running
                | NavigationLifecycle::Mixed
        )
    {
        capabilities.push(capability(
            NavigationAction::ActivateDojo,
            context.freshness,
            true,
            false,
            context.window.has_tab_capacity,
        ));
    } else if attachment == WindowAttachment::NotHere && lifecycle == NavigationLifecycle::Running {
        capabilities.push(capability(
            NavigationAction::AttachDojo,
            context.freshness,
            context.authority.attach.is_allowed(),
            true,
            context.window.has_tab_capacity,
        ));
    }
    if lifecycle == NavigationLifecycle::Restorable {
        capabilities.push(capability(
            NavigationAction::PreviewRestoreDojo,
            context.freshness,
            context.authority.restore.is_allowed(),
            false,
            context.window.has_tab_capacity,
        ));
    }
    let working_directory = dojo
        .root
        .find_splint(dojo.default_focus)
        .or_else(|| dojo.root.find_splint(dojo.root.first_splint_id()))
        .map_or_else(String::new, |splint| {
            bounded_cwd(&splint.cwd.to_string_lossy())
        });
    NavigationDojo {
        node: NavigationNode {
            id: dojo_node_id,
            parent: Some(lair_node_id),
            label: bounded_label(&dojo.name),
            lifecycle,
            capabilities,
        },
        lair_id,
        dojo_id,
        default_focus: dojo.default_focus,
        working_directory,
        attachment,
        active_here,
        splints,
    }
}

fn layout_is_fully_running(
    layout: &LayoutNode,
    runtimes: &HashMap<SplintId, &SplintRuntimeSummary>,
) -> bool {
    match layout {
        LayoutNode::Leaf(splint) => runtimes[&splint.id].lifecycle == SplintLifecycle::Running,
        LayoutNode::Branch { first, second, .. } => {
            layout_is_fully_running(first, runtimes) && layout_is_fully_running(second, runtimes)
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "recursive projection keeps authoritative identities and bounded context explicit"
)]
fn collect_splints(
    layout: &LayoutNode,
    lair_id: LairId,
    dojo_id: DojoId,
    parent: NavigationNodeId,
    attachment: WindowAttachment,
    active_here: bool,
    detached_parent_reopenable: bool,
    runtimes: &HashMap<SplintId, &SplintRuntimeSummary>,
    context: NavigationProjectionContext<'_>,
    output: &mut Vec<NavigationSplint>,
) {
    match layout {
        LayoutNode::Leaf(splint) => {
            let runtime = runtimes[&splint.id];
            let lifecycle = splint_lifecycle(runtime);
            let mut capabilities = Vec::new();
            if matches!(
                lifecycle,
                NavigationLifecycle::Starting | NavigationLifecycle::Running
            ) {
                let action = match attachment {
                    WindowAttachment::Here => Some((NavigationAction::FocusSplint, true, false)),
                    WindowAttachment::NotHere if detached_parent_reopenable => Some((
                        NavigationAction::AttachAndFocusSplint,
                        context.authority.attach.is_allowed(),
                        true,
                    )),
                    WindowAttachment::NotHere => None,
                };
                if let Some((action, permitted, needs_capacity)) = action {
                    capabilities.push(capability(
                        action,
                        context.freshness,
                        permitted,
                        needs_capacity,
                        context.window.has_tab_capacity,
                    ));
                }
            }
            if lifecycle == NavigationLifecycle::Restorable {
                capabilities.push(capability(
                    NavigationAction::PreviewRestoreSplint,
                    context.freshness,
                    context.authority.restore.is_allowed(),
                    false,
                    context.window.has_tab_capacity,
                ));
            }
            output.push(NavigationSplint {
                node: NavigationNode {
                    id: NavigationNodeId::Splint {
                        lair_id,
                        dojo_id,
                        splint_id: splint.id,
                    },
                    parent: Some(parent),
                    label: bounded_label(&splint.title),
                    lifecycle,
                    capabilities,
                },
                working_directory: bounded_cwd(&splint.cwd.to_string_lossy()),
                live_incarnation: runtime.live_incarnation,
                last_incarnation: runtime.last_incarnation,
                exit_status: runtime.exit_status,
                focused_here: active_here && context.window.focused_splint == Some(splint.id),
            });
        }
        LayoutNode::Branch { first, second, .. } => {
            collect_splints(
                first,
                lair_id,
                dojo_id,
                parent,
                attachment,
                active_here,
                detached_parent_reopenable,
                runtimes,
                context,
                output,
            );
            collect_splints(
                second,
                lair_id,
                dojo_id,
                parent,
                attachment,
                active_here,
                detached_parent_reopenable,
                runtimes,
                context,
                output,
            );
        }
    }
}

const fn splint_lifecycle(runtime: &SplintRuntimeSummary) -> NavigationLifecycle {
    match runtime.lifecycle {
        SplintLifecycle::Starting => NavigationLifecycle::Starting,
        SplintLifecycle::Running => NavigationLifecycle::Running,
        SplintLifecycle::Exited if runtime.restorable => NavigationLifecycle::Restorable,
        SplintLifecycle::Exited => NavigationLifecycle::Exited,
    }
}

fn aggregate_lifecycle(
    mut lifecycles: impl Iterator<Item = NavigationLifecycle>,
) -> NavigationLifecycle {
    let Some(first) = lifecycles.next() else {
        return NavigationLifecycle::Unavailable;
    };
    let mut all_same = true;
    let mut all_exited = matches!(
        first,
        NavigationLifecycle::Exited | NavigationLifecycle::Restorable
    );
    let mut any_restorable = first == NavigationLifecycle::Restorable;
    for lifecycle in lifecycles {
        all_same &= lifecycle == first;
        all_exited &= matches!(
            lifecycle,
            NavigationLifecycle::Exited | NavigationLifecycle::Restorable
        );
        any_restorable |= lifecycle == NavigationLifecycle::Restorable;
    }
    if all_same {
        first
    } else if all_exited && any_restorable {
        NavigationLifecycle::Restorable
    } else if all_exited {
        NavigationLifecycle::Exited
    } else {
        NavigationLifecycle::Mixed
    }
}

fn valid_endpoint_namespace(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace.len() <= 96
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

const fn is_bidi_formatting(character: char) -> bool {
    matches!(
        character,
        '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn bounded_label(value: &str) -> String {
    bounded_text(value, MAX_LABEL_SCALARS, MAX_LABEL_CELLS)
}

fn bounded_cwd(value: &str) -> String {
    bounded_text(value, MAX_CWD_SCALARS, MAX_CWD_CELLS)
}

fn bounded_text(value: &str, maximum_scalars: usize, maximum_cells: usize) -> String {
    let mut characters = Vec::new();
    let mut cells = 0_usize;
    let mut truncated = false;
    for character in value.chars() {
        if is_bidi_formatting(character) {
            continue;
        }
        let sanitized = if character.is_control() {
            if characters.last() == Some(&' ') {
                continue;
            }
            ' '
        } else {
            character
        };
        let width = UnicodeWidthChar::width(sanitized).unwrap_or(0).min(2);
        if characters.len() == maximum_scalars || cells.saturating_add(width) > maximum_cells {
            truncated = true;
            break;
        }
        characters.push(sanitized);
        cells = cells.saturating_add(width);
    }
    if truncated && maximum_scalars > 0 && maximum_cells > 0 {
        while characters.len() >= maximum_scalars || cells.saturating_add(1) > maximum_cells {
            let Some(character) = characters.pop() else {
                break;
            };
            cells = cells.saturating_sub(UnicodeWidthChar::width(character).unwrap_or(0).min(2));
        }
        characters.push('…');
    }
    characters.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use splinterm_core::{
        Axis, Lair, LairLifetime, LayoutNode, Splint, SplintState, SplitRatio, Topology,
    };
    use splinterm_protocol::SplintRuntimeSummary;

    use super::*;

    fn runtime_for(splint: &splinterm_core::Splint, restorable: bool) -> SplintRuntimeSummary {
        let (live_incarnation, lifecycle, exit_status) = match splint.state {
            SplintState::Starting => (None, SplintLifecycle::Starting, None),
            SplintState::Running => (splint.last_incarnation, SplintLifecycle::Running, None),
            SplintState::Exited(code) => (
                None,
                SplintLifecycle::Exited,
                Some(ProcessExitStatus {
                    code: Some(code),
                    signal: None,
                }),
            ),
        };
        SplintRuntimeSummary {
            splint_id: splint.id,
            live_incarnation,
            last_incarnation: splint.last_incarnation,
            restorable,
            lifecycle,
            exit_status,
        }
    }

    fn collect_runtimes(
        layout: &LayoutNode,
        restorable: &HashSet<SplintId>,
        output: &mut Vec<SplintRuntimeSummary>,
    ) {
        match layout {
            LayoutNode::Leaf(splint) => {
                output.push(runtime_for(splint, restorable.contains(&splint.id)));
            }
            LayoutNode::Branch { first, second, .. } => {
                collect_runtimes(first, restorable, output);
                collect_runtimes(second, restorable, output);
            }
        }
    }

    fn snapshot(lairs: Vec<Lair>, restorable: &[SplintId]) -> TopologySnapshot {
        let mut topology = Topology::new();
        let mut runtimes = Vec::new();
        let restorable: HashSet<_> = restorable.iter().copied().collect();
        for lair in lairs {
            for dojo in &lair.dojos {
                collect_runtimes(&dojo.root, &restorable, &mut runtimes);
            }
            let revision = topology.revision();
            topology.insert_lair_at(revision, lair).unwrap();
        }
        TopologySnapshot {
            revision: topology.revision(),
            topology,
            runtimes,
        }
    }

    fn context(attached_dojos: &[DojoId]) -> NavigationProjectionContext<'_> {
        NavigationProjectionContext {
            endpoint_namespace: "local",
            endpoint_generation: 7,
            freshness: EndpointFreshness::Current,
            window: NavigationWindowState {
                attached_dojos,
                active_dojo: attached_dojos.first().copied(),
                focused_splint: None,
                current_lair: None,
                has_tab_capacity: true,
            },
            authority: NavigationAuthority {
                attach: NavigationPermission::Allowed,
                restore: NavigationPermission::Allowed,
                create_lair: NavigationPermission::Allowed,
                create_dojo: NavigationPermission::Allowed,
            },
        }
    }

    const fn enabled(action: NavigationAction) -> NavigationCapability {
        NavigationCapability {
            action,
            availability: NavigationAvailability::Enabled,
        }
    }

    const fn disabled(
        action: NavigationAction,
        blocker: NavigationBlocker,
    ) -> NavigationCapability {
        NavigationCapability {
            action,
            availability: NavigationAvailability::Disabled(blocker),
        }
    }

    fn running_dojo(name: &str) -> Dojo {
        let mut dojo = Dojo::with_shell(name, PathBuf::from(format!("/{name}")));
        let splint = dojo.root.find_splint_mut(dojo.default_focus).unwrap();
        splint.state = SplintState::Running;
        splint.last_incarnation = Some(1);
        dojo
    }

    fn running_lair(name: &str) -> Lair {
        let mut lair = Lair::new(name, PathBuf::from(format!("/{name}")));
        lair.dojos[0] = running_dojo("Dojo 1");
        lair
    }

    fn running_leaf(cwd: &str) -> LayoutNode {
        let mut splint = Splint::shell(PathBuf::from(cwd));
        splint.state = SplintState::Running;
        splint.last_incarnation = Some(1);
        LayoutNode::Leaf(splint)
    }

    fn deep_running_layout(depth: usize) -> LayoutNode {
        if depth == 1 {
            running_leaf("/deep")
        } else {
            LayoutNode::Branch {
                axis: Axis::Horizontal,
                ratio: SplitRatio::new(500).unwrap(),
                first: Box::new(running_leaf("/deep")),
                second: Box::new(deep_running_layout(depth - 1)),
            }
        }
    }

    fn running_layout_with_leaves(count: usize) -> LayoutNode {
        if count == 1 {
            running_leaf("/wide")
        } else {
            let first_count = count / 2;
            LayoutNode::Branch {
                axis: Axis::Horizontal,
                ratio: SplitRatio::new(500).unwrap(),
                first: Box::new(running_layout_with_leaves(first_count)),
                second: Box::new(running_layout_with_leaves(count - first_count)),
            }
        }
    }

    #[test]
    fn projection_preserves_canonical_containment_and_exact_identity() {
        let mut lair = running_lair("forge");
        lair.retention = LairRetention::Saved;
        let dojo_id = lair.dojos[0].id;
        let first_id = lair.dojos[0].default_focus;
        let first = lair.dojos[0].root.clone();
        let mut second = Splint::shell(PathBuf::from("/forge/tests"));
        second.title = "tests".to_owned();
        second.state = SplintState::Exited(1);
        second.last_incarnation = Some(3);
        let second_id = second.id;
        lair.dojos[0].root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(first),
            second: Box::new(LayoutNode::Leaf(second)),
        };
        let snapshot = snapshot(vec![lair], &[]);
        let projection = NavigationProjection::build(&snapshot, context(&[dojo_id])).unwrap();

        assert_eq!(projection.topology_revision, snapshot.revision);
        assert_eq!(projection.endpoint_namespace, "local");
        assert_eq!(projection.endpoint_generation, 7);
        assert_eq!(projection.lairs.len(), 1);
        let projected_lair = &projection.lairs[0];
        assert_eq!(projected_lair.retention, LairRetention::Saved);
        let dojo = &projected_lair.dojos[0];
        assert_eq!(dojo.attachment, WindowAttachment::Here);
        assert!(dojo.active_here);
        assert_eq!(dojo.node.lifecycle, NavigationLifecycle::Mixed);
        assert_eq!(dojo.node.parent, Some(projected_lair.node.id));
        assert_eq!(dojo.splints.len(), 2);
        assert_eq!(
            dojo.splints
                .iter()
                .map(|splint| splint.node.id)
                .collect::<Vec<_>>(),
            vec![
                NavigationNodeId::Splint {
                    lair_id: projected_lair.lair_id,
                    dojo_id,
                    splint_id: first_id,
                },
                NavigationNodeId::Splint {
                    lair_id: projected_lair.lair_id,
                    dojo_id,
                    splint_id: second_id,
                },
            ]
        );
        assert_eq!(
            dojo.node.capabilities,
            [enabled(NavigationAction::ActivateDojo)]
        );
    }

    #[test]
    fn projection_omits_transient_lairs_and_ignores_unknown_window_ids() {
        let persistent = running_lair("persistent");
        let persistent_id = persistent.dojos[0].id;
        let mut transient = running_lair("transient");
        transient.lifetime = LairLifetime::Transient;
        let unknown = DojoId::new();
        let snapshot = snapshot(vec![transient, persistent], &[]);
        let projection = NavigationProjection::build(&snapshot, context(&[unknown])).unwrap();

        assert_eq!(projection.lairs.len(), 1);
        assert_eq!(projection.lairs[0].node.label, "persistent");
        assert_eq!(
            projection.lairs[0].dojos[0].attachment,
            WindowAttachment::NotHere
        );
        assert_eq!(projection.lairs[0].dojos[0].dojo_id, persistent_id);
    }

    #[test]
    fn current_projection_derives_attach_focus_and_restore_capabilities() {
        let attached = running_lair("attached");
        let attached_dojo = attached.dojos[0].id;
        let attached_splint = attached.dojos[0].default_focus;

        let detached = running_lair("detached");
        let detached_dojo = detached.dojos[0].id;

        let mut restorable = running_lair("restorable");
        let restore_splint = restorable.dojos[0].default_focus;
        let splint = restorable.dojos[0]
            .root
            .find_splint_mut(restore_splint)
            .unwrap();
        splint.state = SplintState::Exited(0);
        splint.last_incarnation = Some(2);

        let restorable_dojo = restorable.dojos[0].id;
        let snapshot = snapshot(vec![attached, detached, restorable], &[restore_splint]);
        let attached_dojos = [attached_dojo];
        let mut context = context(&attached_dojos);
        context.window.focused_splint = Some(attached_splint);
        let projection = NavigationProjection::build(&snapshot, context).unwrap();
        let dojos: HashMap<_, _> = projection
            .lairs
            .iter()
            .flat_map(|lair| &lair.dojos)
            .map(|dojo| (dojo.dojo_id, dojo))
            .collect();

        assert_eq!(
            dojos[&attached_dojo].node.capabilities,
            [enabled(NavigationAction::ActivateDojo)]
        );
        assert_eq!(
            dojos[&attached_dojo].splints[0].node.capabilities,
            [enabled(NavigationAction::FocusSplint)]
        );
        assert!(dojos[&attached_dojo].splints[0].focused_here);
        assert_eq!(
            dojos[&detached_dojo].node.capabilities,
            [enabled(NavigationAction::AttachDojo)]
        );
        assert_eq!(
            dojos[&detached_dojo].splints[0].node.capabilities,
            [enabled(NavigationAction::AttachAndFocusSplint)]
        );
        assert_eq!(dojos[&detached_dojo].splints[0].live_incarnation, Some(1));
        assert_eq!(dojos[&detached_dojo].splints[0].last_incarnation, Some(1));
        assert_eq!(
            dojos[&restorable_dojo].node.capabilities,
            [enabled(NavigationAction::PreviewRestoreDojo)]
        );
        assert_eq!(
            dojos[&restorable_dojo].splints[0].node.capabilities,
            [enabled(NavigationAction::PreviewRestoreSplint)]
        );
        assert_eq!(dojos[&restorable_dojo].splints[0].live_incarnation, None);
        assert_eq!(dojos[&restorable_dojo].splints[0].last_incarnation, Some(2));
    }

    #[test]
    fn attached_exited_dojos_never_advertise_activation() {
        let mut restorable = running_lair("restorable");
        let restorable_dojo = restorable.dojos[0].id;
        let restorable_splint = restorable.dojos[0].default_focus;
        let splint = restorable.dojos[0]
            .root
            .find_splint_mut(restorable_splint)
            .unwrap();
        splint.state = SplintState::Exited(0);
        splint.last_incarnation = Some(2);

        let mut exited = running_lair("exited");
        let exited_dojo = exited.dojos[0].id;
        let exited_splint = exited.dojos[0].default_focus;
        let splint = exited.dojos[0].root.find_splint_mut(exited_splint).unwrap();
        splint.state = SplintState::Exited(1);
        splint.last_incarnation = Some(3);

        let snapshot = snapshot(vec![restorable, exited], &[restorable_splint]);
        let attached = [restorable_dojo, exited_dojo];
        let projection = NavigationProjection::build(&snapshot, context(&attached)).unwrap();
        let dojos: HashMap<_, _> = projection
            .lairs
            .iter()
            .flat_map(|lair| &lair.dojos)
            .map(|dojo| (dojo.dojo_id, dojo))
            .collect();

        assert_eq!(
            dojos[&restorable_dojo].node.capabilities,
            [enabled(NavigationAction::PreviewRestoreDojo)]
        );
        assert!(dojos[&exited_dojo].node.capabilities.is_empty());
    }

    #[test]
    fn detached_starting_and_mixed_dojos_cannot_attach_through_splints() {
        let starting = Lair::new("starting", PathBuf::from("/starting"));
        let starting_dojo = starting.dojos[0].id;

        let mut mixed = running_lair("mixed");
        let mixed_dojo = mixed.dojos[0].id;
        let running = mixed.dojos[0].root.clone();
        let mut exited = Splint::shell(PathBuf::from("/mixed"));
        exited.state = SplintState::Exited(1);
        exited.last_incarnation = Some(2);
        mixed.dojos[0].root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(running),
            second: Box::new(LayoutNode::Leaf(exited)),
        };

        let snapshot = snapshot(vec![starting, mixed], &[]);
        let projection = NavigationProjection::build(&snapshot, context(&[])).unwrap();
        let dojos: HashMap<_, _> = projection
            .lairs
            .iter()
            .flat_map(|lair| &lair.dojos)
            .map(|dojo| (dojo.dojo_id, dojo))
            .collect();

        for dojo_id in [starting_dojo, mixed_dojo] {
            assert!(dojos[&dojo_id].node.capabilities.is_empty());
            assert!(
                dojos[&dojo_id]
                    .splints
                    .iter()
                    .all(|splint| splint.node.capabilities.is_empty())
            );
        }
    }

    #[test]
    fn creation_capabilities_preserve_target_and_disabled_reason() {
        let persistent = running_lair("forge");
        let lair_id = persistent.id;
        let snapshot = snapshot(vec![persistent], &[]);
        let mut current = context(&[]);
        current.window.current_lair = Some(lair_id);
        let projection = NavigationProjection::build(&snapshot, current).unwrap();
        assert_eq!(
            projection.creation.new_lair,
            enabled(NavigationAction::CreateLair)
        );
        assert_eq!(
            projection.creation.new_dojo,
            Some(NavigationDojoCreation {
                lair_id,
                capability: enabled(NavigationAction::CreateDojo),
            })
        );

        let mut blocked = context(&[]);
        blocked.window.current_lair = Some(lair_id);
        blocked.window.has_tab_capacity = false;
        let projection = NavigationProjection::build(&snapshot, blocked).unwrap();
        assert_eq!(
            projection.creation.new_lair,
            disabled(
                NavigationAction::CreateLair,
                NavigationBlocker::TabCapacityReached,
            )
        );
        assert_eq!(
            projection.creation.new_dojo.unwrap().capability,
            disabled(
                NavigationAction::CreateDojo,
                NavigationBlocker::TabCapacityReached,
            )
        );

        let mut denied = context(&[]);
        denied.authority.create_lair = NavigationPermission::Denied;
        denied.authority.create_dojo = NavigationPermission::Denied;
        denied.window.current_lair = Some(lair_id);
        let projection = NavigationProjection::build(&snapshot, denied).unwrap();
        assert_eq!(
            projection.creation.new_lair.availability,
            NavigationAvailability::Disabled(NavigationBlocker::PermissionDenied)
        );
        assert_eq!(
            projection
                .creation
                .new_dojo
                .unwrap()
                .capability
                .availability,
            NavigationAvailability::Disabled(NavigationBlocker::PermissionDenied)
        );
        assert_eq!(
            NavigationBlocker::PermissionDenied.message(),
            "Permission denied"
        );
    }

    #[test]
    fn transient_current_lair_retains_new_dojo_capability_without_tree_row() {
        let mut transient = running_lair("scratch");
        transient.lifetime = LairLifetime::Transient;
        let lair_id = transient.id;
        let snapshot = snapshot(vec![transient], &[]);
        let mut current = context(&[]);
        current.window.current_lair = Some(lair_id);
        let projection = NavigationProjection::build(&snapshot, current).unwrap();

        assert!(projection.lairs.is_empty());
        assert_eq!(projection.creation.new_dojo.unwrap().lair_id, lair_id);
    }

    #[test]
    fn stale_and_capacity_blocked_projections_disable_mutating_actions() {
        let lair = running_lair("forge");
        let dojo_id = lair.dojos[0].id;
        let snapshot = snapshot(vec![lair], &[]);

        let mut blocked = context(&[]);
        blocked.window.has_tab_capacity = false;
        let projection = NavigationProjection::build(&snapshot, blocked).unwrap();
        assert_eq!(
            projection.lairs[0].dojos[0].node.capabilities,
            [disabled(
                NavigationAction::AttachDojo,
                NavigationBlocker::TabCapacityReached,
            )]
        );

        let mut denied = context(&[]);
        denied.authority.attach = NavigationPermission::Denied;
        let projection = NavigationProjection::build(&snapshot, denied).unwrap();
        assert_eq!(
            projection.lairs[0].dojos[0].node.capabilities,
            [disabled(
                NavigationAction::AttachDojo,
                NavigationBlocker::PermissionDenied,
            )]
        );

        let attached_dojos = [dojo_id];
        for freshness in [
            EndpointFreshness::Refreshing,
            EndpointFreshness::Stale,
            EndpointFreshness::Disconnected,
        ] {
            let mut context = context(&attached_dojos);
            context.freshness = freshness;
            let projection = NavigationProjection::build(&snapshot, context).unwrap();
            assert_eq!(projection.freshness, freshness);
            let reason = match freshness {
                EndpointFreshness::Refreshing => NavigationBlocker::Refreshing,
                EndpointFreshness::Stale => NavigationBlocker::Stale,
                EndpointFreshness::Disconnected => NavigationBlocker::Disconnected,
                EndpointFreshness::Current => unreachable!(),
            };
            assert_eq!(
                projection.lairs[0].dojos[0].node.capabilities,
                [disabled(NavigationAction::ActivateDojo, reason)]
            );
            assert_eq!(
                projection.lairs[0].dojos[0].splints[0].node.capabilities,
                [disabled(NavigationAction::FocusSplint, reason)]
            );
        }
    }

    #[test]
    fn focused_marker_requires_the_active_parent_dojo() {
        let mut lair = running_lair("forge");
        let active_dojo = lair.dojos[0].id;
        let mut background = Dojo::with_shell("background", PathBuf::from("/background"));
        let background_splint = background.default_focus;
        let splint = background.root.find_splint_mut(background_splint).unwrap();
        splint.state = SplintState::Running;
        splint.last_incarnation = Some(1);
        let background_dojo = background.id;
        lair.dojos.push(background);
        let snapshot = snapshot(vec![lair], &[]);
        let attached = [active_dojo, background_dojo];
        let mut context = context(&attached);
        context.window.active_dojo = Some(active_dojo);
        context.window.focused_splint = Some(background_splint);
        let projection = NavigationProjection::build(&snapshot, context).unwrap();

        assert!(projection.lairs[0].dojos[0].active_here);
        assert!(!projection.lairs[0].dojos[1].active_here);
        assert!(!projection.lairs[0].dojos[1].splints[0].focused_here);
    }

    #[test]
    fn labels_and_working_directories_are_sanitized_and_bounded() {
        let mut lair = running_lair("forge\n\u{202e}spoof");
        let dojo = &mut lair.dojos[0];
        dojo.name = "edit\u{1b}[31m\u{2066}title".to_owned();
        let splint = dojo.root.find_splint_mut(dojo.default_focus).unwrap();
        splint.title = format!("{}{}", "shell\r\n\u{2066}".repeat(80), "界".repeat(100));
        splint.cwd = PathBuf::from(format!("/tmp/{}\u{200f}", "界".repeat(300)));
        let snapshot = snapshot(vec![lair], &[]);
        let projection = NavigationProjection::build(&snapshot, context(&[])).unwrap();
        let lair = &projection.lairs[0];
        let dojo = &lair.dojos[0];
        let splint = &dojo.splints[0];

        for value in [&lair.node.label, &dojo.node.label, &splint.node.label] {
            assert!(!value.chars().any(char::is_control));
            assert!(!value.chars().any(is_bidi_formatting));
            assert!(value.chars().count() <= MAX_LABEL_SCALARS);
            assert!(
                value
                    .chars()
                    .map(|character| character.width().unwrap_or(0).min(2))
                    .sum::<usize>()
                    <= MAX_LABEL_CELLS
            );
        }
        assert!(dojo.node.label.contains("edit [31mtitle"));
        assert!(splint.node.label.ends_with('…'));
        assert!(splint.working_directory.ends_with('…'));
        assert!(!splint.working_directory.chars().any(is_bidi_formatting));
    }

    #[test]
    fn canonical_order_is_independent_of_runtime_vector_order() {
        let mut first = running_lair("first");
        first.dojos.push(running_dojo("second Dojo"));
        let second = running_lair("second");
        let mut snapshot = snapshot(vec![first, second], &[]);
        let expected = snapshot
            .topology
            .lairs()
            .map(|lair| {
                (
                    lair.id,
                    lair.dojos.iter().map(|dojo| dojo.id).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        snapshot.runtimes.reverse();
        snapshot.validate().unwrap();

        let projection = NavigationProjection::build(&snapshot, context(&[])).unwrap();
        let actual = projection
            .lairs
            .iter()
            .map(|lair| {
                (
                    lair.lair_id,
                    lair.dojos
                        .iter()
                        .map(|dojo| dojo.dojo_id)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn projection_tracks_topology_revision_and_endpoint_generation_independently() {
        let first = snapshot(vec![running_lair("first")], &[]);
        let first_projection = NavigationProjection::build(&first, context(&[])).unwrap();
        let mut next_endpoint = context(&[]);
        next_endpoint.endpoint_generation = 8;
        let regenerated = NavigationProjection::build(&first, next_endpoint).unwrap();
        assert_eq!(
            first_projection.topology_revision,
            regenerated.topology_revision
        );
        assert_ne!(
            first_projection.endpoint_generation,
            regenerated.endpoint_generation
        );

        let second = snapshot(vec![running_lair("first"), running_lair("second")], &[]);
        let advanced = NavigationProjection::build(&second, context(&[])).unwrap();
        assert_ne!(
            first_projection.topology_revision,
            advanced.topology_revision
        );
        assert_eq!(
            first_projection.endpoint_generation,
            advanced.endpoint_generation
        );
    }

    #[test]
    fn structural_validation_rejects_duplicate_dojo_and_invalid_focus() {
        let mut lair = running_lair("forge");
        lair.dojos.push(running_dojo("other"));
        let snapshot = snapshot(vec![lair], &[]);
        let mut duplicate_value = serde_json::to_value(&snapshot).unwrap();
        let lairs = duplicate_value["topology"]["lairs"]
            .as_object_mut()
            .unwrap();
        let dojos = lairs.values_mut().next().unwrap()["dojos"]
            .as_array_mut()
            .unwrap();
        dojos[1]["id"] = dojos[0]["id"].clone();
        let duplicate: TopologySnapshot = serde_json::from_value(duplicate_value).unwrap();
        duplicate.validate().unwrap();
        assert_eq!(
            NavigationProjection::build(&duplicate, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );

        let mut invalid_focus_value = serde_json::to_value(&snapshot).unwrap();
        let lairs = invalid_focus_value["topology"]["lairs"]
            .as_object_mut()
            .unwrap();
        let dojo = &mut lairs.values_mut().next().unwrap()["dojos"][0];
        dojo["default_focus"] = serde_json::to_value(SplintId::new()).unwrap();
        let invalid_focus: TopologySnapshot = serde_json::from_value(invalid_focus_value).unwrap();
        invalid_focus.validate().unwrap();
        assert_eq!(
            NavigationProjection::build(&invalid_focus, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );
    }

    #[test]
    fn structural_validation_rejects_cross_swapped_lair_map_values() {
        let snapshot = snapshot(vec![running_lair("first"), running_lair("second")], &[]);
        let mut value = serde_json::to_value(&snapshot).unwrap();
        let lairs = value["topology"]["lairs"].as_object_mut().unwrap();
        let mut entries = lairs.values_mut().collect::<Vec<_>>();
        let first_id = entries[0]["id"].clone();
        let second_id = entries[1]["id"].clone();
        entries[0]["id"] = second_id;
        entries[1]["id"] = first_id;
        let swapped: TopologySnapshot = serde_json::from_value(value).unwrap();
        swapped.validate().unwrap();

        assert_eq!(
            NavigationProjection::build(&swapped, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );
    }

    #[test]
    fn structural_validation_enforces_window_dojo_and_layout_limits() {
        let baseline = snapshot(vec![running_lair("forge")], &[]);
        let too_many_tabs = vec![DojoId::new(); crate::tab::MAX_WINDOW_TABS + 1];
        assert_eq!(
            NavigationProjection::build(&baseline, context(&too_many_tabs)),
            Err(NavigationProjectionError::InvalidSnapshot)
        );

        let mut too_many_dojos = running_lair("crowded");
        for index in 1..=MAX_DOJOS_PER_LAIR {
            too_many_dojos
                .dojos
                .push(running_dojo(&format!("Dojo {index}")));
        }
        let oversized = snapshot(vec![too_many_dojos], &[]);
        oversized.validate().unwrap();
        assert_eq!(
            NavigationProjection::build(&oversized, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );

        let mut too_deep = running_lair("deep");
        too_deep.dojos[0].root = deep_running_layout(MAX_LAYOUT_DEPTH + 1);
        too_deep.dojos[0].default_focus = too_deep.dojos[0].root.first_splint_id();
        let deep = snapshot(vec![too_deep], &[]);
        deep.validate().unwrap();
        assert_eq!(
            NavigationProjection::build(&deep, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );

        let lairs = (0..=MAX_PERSISTENT_LAIRS)
            .map(|index| running_lair(&format!("Lair {index}")))
            .collect();
        let too_many_lairs = snapshot(lairs, &[]);
        too_many_lairs.validate().unwrap();
        assert_eq!(
            NavigationProjection::build(&too_many_lairs, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );

        let mut too_many_splints = running_lair("wide");
        too_many_splints.dojos[0].root = running_layout_with_leaves(MAX_SPLINTS + 1);
        too_many_splints.dojos[0].default_focus = too_many_splints.dojos[0].root.first_splint_id();
        let too_many_splints = snapshot(vec![too_many_splints], &[]);
        too_many_splints.validate().unwrap();
        assert_eq!(
            NavigationProjection::build(&too_many_splints, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );
    }

    #[test]
    fn incarnation_mismatch_is_rejected() {
        let lair = running_lair("forge");
        let mut snapshot = snapshot(vec![lair], &[]);
        snapshot.runtimes[0].live_incarnation = Some(2);
        assert_eq!(
            NavigationProjection::build(&snapshot, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );
    }

    #[test]
    fn endpoint_namespace_is_bounded_and_keeps_remote_identity_distinct() {
        let lair = running_lair("forge");
        let snapshot = snapshot(vec![lair], &[]);
        let mut remote = context(&[]);
        remote.endpoint_namespace = "ssh-prod.example-22";
        let projection = NavigationProjection::build(&snapshot, remote).unwrap();
        assert_eq!(projection.endpoint_namespace, "ssh-prod.example-22");

        let mut unsafe_context = context(&[]);
        unsafe_context.endpoint_namespace = "remote/../../local";
        assert_eq!(
            NavigationProjection::build(&snapshot, unsafe_context),
            Err(NavigationProjectionError::InvalidEndpointNamespace)
        );
        assert_eq!(
            NavigationProjectionError::InvalidEndpointNamespace.to_string(),
            "navigation endpoint namespace is invalid"
        );
    }

    #[test]
    fn invalid_snapshot_is_rejected_without_exposing_protocol_detail() {
        let lair = running_lair("forge");
        let mut snapshot = snapshot(vec![lair], &[]);
        snapshot.runtimes[0].splint_id = SplintId::new();
        assert_eq!(
            NavigationProjection::build(&snapshot, context(&[])),
            Err(NavigationProjectionError::InvalidSnapshot)
        );
        assert_eq!(
            NavigationProjectionError::InvalidSnapshot.to_string(),
            "navigation snapshot is invalid"
        );
    }

    #[test]
    fn lifecycle_aggregation_keeps_mixed_and_restorable_states_distinct() {
        let aggregate =
            |states: &[NavigationLifecycle]| aggregate_lifecycle(states.iter().copied());
        assert_eq!(aggregate(&[]), NavigationLifecycle::Unavailable);
        assert_eq!(
            aggregate(&[NavigationLifecycle::Starting]),
            NavigationLifecycle::Starting
        );
        assert_eq!(
            aggregate(&[NavigationLifecycle::Running, NavigationLifecycle::Running]),
            NavigationLifecycle::Running
        );
        assert_eq!(
            aggregate(&[NavigationLifecycle::Starting, NavigationLifecycle::Running]),
            NavigationLifecycle::Mixed
        );
        assert_eq!(
            aggregate(&[NavigationLifecycle::Exited, NavigationLifecycle::Exited]),
            NavigationLifecycle::Exited
        );
        assert_eq!(
            aggregate(&[NavigationLifecycle::Exited, NavigationLifecycle::Restorable,]),
            NavigationLifecycle::Restorable
        );
    }

    #[test]
    fn empty_lair_projects_as_unavailable() {
        let mut lair = running_lair("empty");
        lair.dojos.clear();
        let snapshot = snapshot(vec![lair], &[]);
        let projection = NavigationProjection::build(&snapshot, context(&[])).unwrap();
        assert_eq!(
            projection.lairs[0].node.lifecycle,
            NavigationLifecycle::Unavailable
        );
    }
}
