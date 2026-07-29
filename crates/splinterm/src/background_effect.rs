//! Pure desired-state reducer for native Wayland background blur.
//!
//! This module owns no Wayland proxies. It validates surface-local geometry and
//! produces ordered actions for the graphical owner to execute in a later slice.

use std::{error::Error, fmt};

/// `ext_background_effect_manager_v1.capability.blur`.
pub const BLUR_CAPABILITY: u32 = 1;

/// A finite surface-local blur region expressed in logical coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalSize {
    width: i32,
    height: i32,
}

impl LogicalSize {
    /// Validates positive dimensions representable by the Wayland region API.
    ///
    /// # Errors
    /// Returns an error for zero, negative, or greater-than-`i32::MAX` values.
    pub fn new(width: i64, height: i64) -> Result<Self, GeometryError> {
        if width <= 0 || height <= 0 {
            return Err(GeometryError::NonPositive { width, height });
        }
        let width = i32::try_from(width)
            .map_err(|_| GeometryError::OutsideProtocolRange { width, height })?;
        let height = i32::try_from(height).map_err(|_| GeometryError::OutsideProtocolRange {
            width: i64::from(width),
            height,
        })?;
        Ok(Self { width, height })
    }

    #[must_use]
    pub const fn width(self) -> i32 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> i32 {
        self.height
    }
}

/// Invalid logical geometry that must not reach a protocol request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeometryError {
    NonPositive { width: i64, height: i64 },
    OutsideProtocolRange { width: i64, height: i64 },
}

impl fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositive { width, height } => {
                write!(
                    formatter,
                    "logical blur size must be positive, got {width}x{height}"
                )
            }
            Self::OutsideProtocolRange { width, height } => write!(
                formatter,
                "logical blur size exceeds the Wayland region range: {width}x{height}"
            ),
        }
    }
}

impl Error for GeometryError {}

/// Client-side ownership of the one allowed surface effect object.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EffectLifecycle {
    #[default]
    Absent,
    Active,
    DestroyPending,
}

/// Why the reducer requires a surface commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitReason {
    Enable,
    Disable,
    Resize,
}

/// Bounded fallback diagnostics emitted by the pure reducer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectDiagnostic {
    MissingManager,
    MissingBlurCapability,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DiagnosticState {
    #[default]
    Clear,
    Reported,
}

/// Ordered requests for the Wayland-owning layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectAction {
    Diagnostic(EffectDiagnostic),
    CreateEffect,
    SetBlurRegion(LogicalSize),
    DestroyEffect,
    CommitSurface(CommitReason),
}

/// Desired and last-applied state for one main `wl_surface`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackgroundEffectState {
    requested_blur: bool,
    background_alpha: u16,
    manager_available: bool,
    capability_flags: u32,
    logical_size: Option<LogicalSize>,
    applied_region: Option<LogicalSize>,
    lifecycle: EffectLifecycle,
    pending_commit: Option<CommitReason>,
    diagnostic_state: DiagnosticState,
    surface_alive: bool,
}

impl Default for BackgroundEffectState {
    fn default() -> Self {
        Self {
            requested_blur: false,
            background_alpha: u16::MAX,
            manager_available: false,
            capability_flags: 0,
            logical_size: None,
            applied_region: None,
            lifecycle: EffectLifecycle::Absent,
            pending_commit: None,
            diagnostic_state: DiagnosticState::Clear,
            surface_alive: true,
        }
    }
}

impl BackgroundEffectState {
    pub fn set_requested_blur(&mut self, requested: bool) {
        self.requested_blur = requested;
    }

    pub fn set_background_alpha(&mut self, alpha: u16) {
        self.background_alpha = alpha;
    }

    pub fn set_manager_available(&mut self, available: bool) {
        self.manager_available = available;
    }

    pub fn set_capability_flags(&mut self, flags: u32) {
        self.capability_flags = flags;
    }

    /// Replaces the desired logical surface size only when it is protocol-safe.
    ///
    /// # Errors
    /// Returns an error without changing the last valid size when either
    /// dimension is zero, negative, or outside the signed 32-bit protocol range.
    pub fn set_logical_size(&mut self, width: i64, height: i64) -> Result<(), GeometryError> {
        self.logical_size = Some(LogicalSize::new(width, height)?);
        Ok(())
    }

    #[must_use]
    pub const fn lifecycle(&self) -> EffectLifecycle {
        self.lifecycle
    }

    #[must_use]
    pub const fn logical_size(&self) -> Option<LogicalSize> {
        self.logical_size
    }

    #[must_use]
    pub const fn capability_flags(&self) -> u32 {
        self.capability_flags
    }

    #[must_use]
    pub const fn commit_required(&self) -> bool {
        self.pending_commit.is_some()
    }

    #[must_use]
    pub const fn surface_alive(&self) -> bool {
        self.surface_alive
    }

    #[must_use]
    fn eligible(&self) -> bool {
        self.surface_alive
            && self.requested_blur
            && self.background_alpha < u16::MAX
            && self.manager_available
            && self.capability_flags & BLUR_CAPABILITY != 0
            && self.logical_size.is_some()
    }

    fn reconcile_diagnostic(&mut self) -> Option<EffectAction> {
        let requested =
            self.surface_alive && self.requested_blur && self.background_alpha < u16::MAX;
        let supported = self.manager_available && self.capability_flags & BLUR_CAPABILITY != 0;
        if !requested || supported {
            self.diagnostic_state = DiagnosticState::Clear;
            return None;
        }
        if self.diagnostic_state == DiagnosticState::Reported {
            return None;
        }
        self.diagnostic_state = DiagnosticState::Reported;
        Some(EffectAction::Diagnostic(if self.manager_available {
            EffectDiagnostic::MissingBlurCapability
        } else {
            EffectDiagnostic::MissingManager
        }))
    }

    /// Produces the smallest ordered action sequence needed to match desired state.
    ///
    /// A returned commit remains pending until [`Self::surface_committed`] is
    /// called. Reconciliation is a no-op while a commit is pending, which prevents
    /// duplicate objects and region requests.
    #[must_use]
    pub fn reconcile(&mut self) -> Vec<EffectAction> {
        if !self.surface_alive {
            return Vec::new();
        }
        let mut actions = self.reconcile_diagnostic().into_iter().collect::<Vec<_>>();
        if self.pending_commit.is_some() {
            return actions;
        }

        match self.lifecycle {
            EffectLifecycle::Absent if self.eligible() => {
                let Some(size) = self.logical_size else {
                    return actions;
                };
                self.lifecycle = EffectLifecycle::Active;
                self.applied_region = Some(size);
                self.pending_commit = Some(CommitReason::Enable);
                actions.extend([
                    EffectAction::CreateEffect,
                    EffectAction::SetBlurRegion(size),
                    EffectAction::CommitSurface(CommitReason::Enable),
                ]);
            }
            EffectLifecycle::Absent | EffectLifecycle::DestroyPending => {}
            EffectLifecycle::Active if !self.eligible() => {
                self.lifecycle = EffectLifecycle::DestroyPending;
                self.applied_region = None;
                self.pending_commit = Some(CommitReason::Disable);
                actions.extend([
                    EffectAction::DestroyEffect,
                    EffectAction::CommitSurface(CommitReason::Disable),
                ]);
            }
            EffectLifecycle::Active => {
                let Some(size) = self.logical_size else {
                    return actions;
                };
                if self.applied_region == Some(size) {
                    return actions;
                }
                self.applied_region = Some(size);
                self.pending_commit = Some(CommitReason::Resize);
                actions.extend([
                    EffectAction::SetBlurRegion(size),
                    EffectAction::CommitSurface(CommitReason::Resize),
                ]);
            }
        }
        actions
    }

    /// Acknowledges the surface commit that applied the last reducer actions.
    #[must_use]
    pub fn surface_committed(&mut self) -> Option<CommitReason> {
        let reason = self.pending_commit.take()?;
        if self.lifecycle == EffectLifecycle::DestroyPending {
            self.lifecycle = EffectLifecycle::Absent;
        }
        Some(reason)
    }

    /// Releases reducer-owned effect state before the surface becomes inert.
    ///
    /// An active effect still needs one destroy request. A `DestroyPending`
    /// effect has already emitted that request and must not emit it twice.
    #[must_use]
    pub fn destroy_surface(&mut self) -> Vec<EffectAction> {
        if !self.surface_alive {
            return Vec::new();
        }
        self.surface_alive = false;
        self.logical_size = None;
        self.applied_region = None;
        self.pending_commit = None;
        self.diagnostic_state = DiagnosticState::Clear;
        let actions = if self.lifecycle == EffectLifecycle::Active {
            vec![EffectAction::DestroyEffect]
        } else {
            Vec::new()
        };
        self.lifecycle = EffectLifecycle::Absent;
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protocol_ready() -> BackgroundEffectState {
        let mut state = BackgroundEffectState::default();
        state.set_manager_available(true);
        state.set_capability_flags(BLUR_CAPABILITY);
        state.set_logical_size(960, 600).unwrap();
        state
    }

    fn eligible() -> BackgroundEffectState {
        let mut state = protocol_ready();
        state.set_requested_blur(true);
        state.set_background_alpha(u16::MAX - 1);
        state
    }

    fn enable_actions(size: LogicalSize) -> Vec<EffectAction> {
        vec![
            EffectAction::CreateEffect,
            EffectAction::SetBlurRegion(size),
            EffectAction::CommitSurface(CommitReason::Enable),
        ]
    }

    #[test]
    fn runtime_behavior_table_creates_only_for_full_eligibility() {
        let mut opaque = protocol_ready();
        opaque.set_requested_blur(true);
        assert!(opaque.reconcile().is_empty());

        let mut disabled = protocol_ready();
        disabled.set_background_alpha(u16::MAX - 1);
        assert!(disabled.reconcile().is_empty());

        let mut missing_manager = eligible();
        missing_manager.set_manager_available(false);
        assert_eq!(
            missing_manager.reconcile(),
            vec![EffectAction::Diagnostic(EffectDiagnostic::MissingManager)]
        );
        assert!(missing_manager.reconcile().is_empty());

        let mut missing_capability = eligible();
        missing_capability.set_capability_flags(0x8000_0000);
        assert_eq!(
            missing_capability.reconcile(),
            vec![EffectAction::Diagnostic(
                EffectDiagnostic::MissingBlurCapability
            )]
        );
        assert!(missing_capability.reconcile().is_empty());
        assert_eq!(missing_capability.capability_flags(), 0x8000_0000);

        let mut ready = eligible();
        let size = ready.logical_size().unwrap();
        assert_eq!(ready.reconcile(), enable_actions(size));
        assert_eq!(ready.lifecycle(), EffectLifecycle::Active);
        assert!(ready.commit_required());
    }

    #[test]
    fn fallback_diagnostics_are_one_shot_per_missing_capability_episode() {
        let mut state = eligible();
        state.set_manager_available(false);
        assert_eq!(
            state.reconcile(),
            vec![EffectAction::Diagnostic(EffectDiagnostic::MissingManager)]
        );
        assert!(state.reconcile().is_empty());

        state.set_requested_blur(false);
        assert!(state.reconcile().is_empty());
        state.set_requested_blur(true);
        assert_eq!(
            state.reconcile(),
            vec![EffectAction::Diagnostic(EffectDiagnostic::MissingManager)]
        );

        state.set_manager_available(true);
        state.set_capability_flags(0x8000_0000);
        assert!(state.reconcile().is_empty());
        state.set_manager_available(false);
        assert!(state.reconcile().is_empty());
        state.set_manager_available(true);
        assert!(state.reconcile().is_empty());

        state.set_capability_flags(0x8000_0000 | BLUR_CAPABILITY);
        assert_eq!(
            state.reconcile(),
            enable_actions(state.logical_size().unwrap())
        );
    }

    #[test]
    fn repeated_reconciliation_is_a_no_op_after_state_settles() {
        let mut state = eligible();
        assert_eq!(
            state.reconcile(),
            enable_actions(state.logical_size().unwrap())
        );
        assert!(state.reconcile().is_empty());
        assert_eq!(state.surface_committed(), Some(CommitReason::Enable));
        assert!(!state.commit_required());
        assert!(state.reconcile().is_empty());
    }

    #[test]
    fn capability_loss_and_regain_waits_for_committed_removal() {
        let mut state = eligible();
        assert_eq!(
            state.reconcile(),
            enable_actions(state.logical_size().unwrap())
        );
        assert_eq!(state.surface_committed(), Some(CommitReason::Enable));

        state.set_capability_flags(0x4000_0000);
        assert_eq!(
            state.reconcile(),
            vec![
                EffectAction::Diagnostic(EffectDiagnostic::MissingBlurCapability),
                EffectAction::DestroyEffect,
                EffectAction::CommitSurface(CommitReason::Disable),
            ]
        );
        assert_eq!(state.lifecycle(), EffectLifecycle::DestroyPending);

        state.set_capability_flags(0x4000_0000 | BLUR_CAPABILITY);
        assert!(state.reconcile().is_empty());
        assert_eq!(state.surface_committed(), Some(CommitReason::Disable));
        assert_eq!(state.lifecycle(), EffectLifecycle::Absent);
        assert_eq!(
            state.reconcile(),
            enable_actions(state.logical_size().unwrap())
        );
    }

    #[test]
    fn alpha_and_blur_enable_in_either_order_and_disable_without_duplicates() {
        let mut alpha_first = protocol_ready();
        alpha_first.set_background_alpha(u16::MAX - 1);
        assert!(alpha_first.reconcile().is_empty());
        alpha_first.set_requested_blur(true);
        assert_eq!(
            alpha_first.reconcile(),
            enable_actions(alpha_first.logical_size().unwrap())
        );
        assert_eq!(alpha_first.surface_committed(), Some(CommitReason::Enable));

        alpha_first.set_background_alpha(u16::MAX);
        assert_eq!(
            alpha_first.reconcile(),
            vec![
                EffectAction::DestroyEffect,
                EffectAction::CommitSurface(CommitReason::Disable),
            ]
        );
        alpha_first.set_background_alpha(u16::MAX - 1);
        assert!(alpha_first.reconcile().is_empty());
        assert_eq!(alpha_first.surface_committed(), Some(CommitReason::Disable));
        assert_eq!(
            alpha_first.reconcile(),
            enable_actions(alpha_first.logical_size().unwrap())
        );

        let mut blur_first = protocol_ready();
        blur_first.set_requested_blur(true);
        assert!(blur_first.reconcile().is_empty());
        blur_first.set_background_alpha(u16::MAX - 1);
        assert_eq!(
            blur_first.reconcile(),
            enable_actions(blur_first.logical_size().unwrap())
        );
        assert_eq!(blur_first.surface_committed(), Some(CommitReason::Enable));
        blur_first.set_requested_blur(false);
        assert_eq!(
            blur_first.reconcile(),
            vec![
                EffectAction::DestroyEffect,
                EffectAction::CommitSurface(CommitReason::Disable),
            ]
        );
    }

    #[test]
    fn resize_updates_only_an_active_effect_and_uses_latest_inactive_size() {
        let mut inactive = protocol_ready();
        inactive.set_background_alpha(u16::MAX - 1);
        inactive.set_logical_size(800, 500).unwrap();
        assert!(inactive.reconcile().is_empty());
        inactive.set_requested_blur(true);
        let latest = LogicalSize::new(800, 500).unwrap();
        assert_eq!(inactive.reconcile(), enable_actions(latest));
        assert_eq!(inactive.surface_committed(), Some(CommitReason::Enable));

        inactive.set_logical_size(801, 501).unwrap();
        let resized = LogicalSize::new(801, 501).unwrap();
        assert_eq!(
            inactive.reconcile(),
            vec![
                EffectAction::SetBlurRegion(resized),
                EffectAction::CommitSurface(CommitReason::Resize),
            ]
        );
        assert!(inactive.reconcile().is_empty());
        assert_eq!(inactive.surface_committed(), Some(CommitReason::Resize));
        assert!(inactive.reconcile().is_empty());
    }

    #[test]
    fn invalid_geometry_is_rejected_without_losing_the_last_valid_size() {
        let mut state = BackgroundEffectState::default();
        let valid = LogicalSize::new(960, 600).unwrap();
        state.set_logical_size(960, 600).unwrap();

        for (width, height, expected) in [
            (
                0,
                600,
                GeometryError::NonPositive {
                    width: 0,
                    height: 600,
                },
            ),
            (
                960,
                -1,
                GeometryError::NonPositive {
                    width: 960,
                    height: -1,
                },
            ),
            (
                i64::from(i32::MAX) + 1,
                600,
                GeometryError::OutsideProtocolRange {
                    width: i64::from(i32::MAX) + 1,
                    height: 600,
                },
            ),
            (
                960,
                i64::MAX,
                GeometryError::OutsideProtocolRange {
                    width: 960,
                    height: i64::MAX,
                },
            ),
        ] {
            assert_eq!(state.set_logical_size(width, height), Err(expected));
            assert_eq!(state.logical_size(), Some(valid));
        }
    }

    #[test]
    fn surface_destruction_is_idempotent_in_every_lifecycle() {
        let mut absent = BackgroundEffectState::default();
        assert!(absent.destroy_surface().is_empty());
        assert!(!absent.surface_alive());
        assert!(absent.destroy_surface().is_empty());

        let mut active = eligible();
        assert_eq!(
            active.reconcile(),
            enable_actions(active.logical_size().unwrap())
        );
        assert_eq!(active.destroy_surface(), vec![EffectAction::DestroyEffect]);
        assert_eq!(active.lifecycle(), EffectLifecycle::Absent);
        assert!(!active.commit_required());

        let mut destroy_pending = eligible();
        assert_eq!(
            destroy_pending.reconcile(),
            enable_actions(destroy_pending.logical_size().unwrap())
        );
        assert_eq!(
            destroy_pending.surface_committed(),
            Some(CommitReason::Enable)
        );
        destroy_pending.set_requested_blur(false);
        assert_eq!(
            destroy_pending.reconcile(),
            vec![
                EffectAction::DestroyEffect,
                EffectAction::CommitSurface(CommitReason::Disable),
            ]
        );
        assert_eq!(destroy_pending.lifecycle(), EffectLifecycle::DestroyPending);
        assert!(destroy_pending.destroy_surface().is_empty());
        assert_eq!(destroy_pending.lifecycle(), EffectLifecycle::Absent);
    }
}
