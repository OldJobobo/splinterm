//! Exhaustive authorization requirements for every daemon request.
//!
//! This module is the single policy vocabulary and request-to-authority table.
//! Runtime authorization evaluates these plans against trusted UI identity,
//! connection-owned capabilities, consent grants, and persistent policy.

pub use splinterm_protocol::AutomationScope as OperationScope;
use splinterm_protocol::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedAuthority {
    PendingTransfer,
    Controller,
    Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalRequirement {
    RequestedAccessScopes,
    RequestedControlModes,
    RequestedControlTakeover,
    AttachScrollback,
    LiveProcessTermination,
    ExpandedLiveProcessTermination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestAuthorization {
    Authenticated,
    TrustedUi,
    Policy {
        required: &'static [OperationScope],
        any_of: &'static [OperationScope],
    },
    PolicyAndOwned {
        required: &'static [OperationScope],
        owned: OwnedAuthority,
    },
    Owned(OwnedAuthority),
    Conditional {
        base: &'static [OperationScope],
        requirement: ConditionalRequirement,
    },
    TrustedUiConsent,
}

impl RequestAuthorization {
    const fn policy(required: &'static [OperationScope]) -> Self {
        Self::Policy {
            required,
            any_of: &[],
        }
    }
}

const fn preflight_authorization(
    mutation: &splinterm_protocol::MutationPreflight,
) -> RequestAuthorization {
    use OperationScope as Scope;

    match mutation {
        splinterm_protocol::MutationPreflight::CreateLair
        | splinterm_protocol::MutationPreflight::SplitSplint { .. }
        | splinterm_protocol::MutationPreflight::NewDojo { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn, Scope::TopologyLayoutMutate])
        }
        splinterm_protocol::MutationPreflight::RelaunchSplint { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn])
        }
        splinterm_protocol::MutationPreflight::RestoreSplint { .. }
        | splinterm_protocol::MutationPreflight::RestoreDojo { .. }
        | splinterm_protocol::MutationPreflight::RestoreLair { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessRestore])
        }
        splinterm_protocol::MutationPreflight::CloseSplint { .. } => {
            RequestAuthorization::Conditional {
                base: &[Scope::TopologyLayoutMutate],
                requirement: ConditionalRequirement::LiveProcessTermination,
            }
        }
        splinterm_protocol::MutationPreflight::CloseDojo { .. }
        | splinterm_protocol::MutationPreflight::TerminateLair { .. } => {
            RequestAuthorization::Conditional {
                base: &[Scope::TopologyLayoutMutate],
                requirement: ConditionalRequirement::ExpandedLiveProcessTermination,
            }
        }
        splinterm_protocol::MutationPreflight::KillSplint { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessTerminate])
        }
        splinterm_protocol::MutationPreflight::SetSplitRatio { .. }
        | splinterm_protocol::MutationPreflight::SetDojoDefaultFocus { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyLayoutMutate])
        }
        splinterm_protocol::MutationPreflight::RenameLair { .. }
        | splinterm_protocol::MutationPreflight::RenameDojo { .. }
        | splinterm_protocol::MutationPreflight::RenameSplint { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyNameMutate])
        }
    }
}

#[must_use]
pub const fn for_request(request: &Request) -> RequestAuthorization {
    use OperationScope as Scope;

    match request {
        Request::Ping | Request::ReadGraphicalFocus => RequestAuthorization::Authenticated,
        Request::PublishGraphicalFocus { .. }
        | Request::CreateTransientLair { .. }
        | Request::MaterializePreset { .. } => RequestAuthorization::TrustedUi,
        Request::ListLairs | Request::InspectTopology | Request::InspectSplint { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyMetadataRead])
        }
        Request::SubscribeTopology => {
            RequestAuthorization::policy(&[Scope::TopologySubscribe, Scope::TopologyMetadataRead])
        }
        Request::RequestAccess { .. } | Request::RequestLairAccess { .. } => {
            RequestAuthorization::Conditional {
                base: &[],
                requirement: ConditionalRequirement::RequestedAccessScopes,
            }
        }
        Request::AuthorizationStatus { .. } => {
            RequestAuthorization::policy(&[Scope::AuthorizationInspect])
        }
        Request::RevokeAccess { .. } => RequestAuthorization::policy(&[Scope::AuthorizationRevoke]),
        Request::PrepareMutation { mutation } => preflight_authorization(mutation),
        Request::CreateLair { .. }
        | Request::CreateLairAutomation { .. }
        | Request::SplitSplint { .. }
        | Request::SplitSplintAutomation { .. }
        | Request::NewDojo { .. }
        | Request::NewDojoAutomation { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn, Scope::TopologyLayoutMutate])
        }
        Request::RelaunchSplint { .. } | Request::RelaunchSplintAutomation { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn])
        }
        Request::RestoreSplint { .. }
        | Request::RestoreDojo { .. }
        | Request::RestoreLair { .. } => RequestAuthorization::policy(&[Scope::ProcessRestore]),
        Request::CloseSplint { .. } => RequestAuthorization::Conditional {
            base: &[Scope::TopologyLayoutMutate],
            requirement: ConditionalRequirement::LiveProcessTermination,
        },
        Request::CloseDojo { .. } | Request::TerminateLair { .. } => {
            RequestAuthorization::Conditional {
                base: &[Scope::TopologyLayoutMutate],
                requirement: ConditionalRequirement::ExpandedLiveProcessTermination,
            }
        }
        Request::SetSplitRatio { .. } | Request::SetDojoDefaultFocus { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyLayoutMutate])
        }
        Request::RenameLair { .. } | Request::RenameDojo { .. } | Request::RenameSplint { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyNameMutate])
        }
        Request::Attach { .. } => RequestAuthorization::Conditional {
            base: &[Scope::TerminalVisibleRead, Scope::TerminalSubscribe],
            requirement: ConditionalRequirement::AttachScrollback,
        },
        Request::StartScrollbackPage { .. } | Request::ScrollbackPage { .. } => {
            RequestAuthorization::policy(&[Scope::TerminalVisibleRead, Scope::ScrollbackRead])
        }
        Request::StartSearchScrollback { .. } | Request::SearchScrollback { .. } => {
            RequestAuthorization::policy(&[
                Scope::TerminalVisibleRead,
                Scope::ScrollbackRead,
                Scope::ScrollbackSearch,
            ])
        }
        Request::AcquireControl { .. } => RequestAuthorization::Conditional {
            base: &[Scope::ControllerAcquire],
            requirement: ConditionalRequirement::RequestedControlModes,
        },
        Request::RequestImageContent { .. } | Request::SubscribeControl { .. } => {
            RequestAuthorization::policy(&[Scope::TerminalVisibleRead])
        }
        Request::RequestControlTransfer { .. } => RequestAuthorization::Conditional {
            base: &[Scope::ControllerTransfer],
            requirement: ConditionalRequirement::RequestedControlModes,
        },
        Request::DecideControlTransfer { .. } => {
            RequestAuthorization::Owned(OwnedAuthority::PendingTransfer)
        }
        Request::ForceControlTransfer { .. } => RequestAuthorization::Conditional {
            base: &[Scope::ControllerAcquire, Scope::ControllerTransfer],
            requirement: ConditionalRequirement::RequestedControlTakeover,
        },
        Request::ReleaseControl { .. } => RequestAuthorization::Owned(OwnedAuthority::Controller),
        Request::Input { .. } => RequestAuthorization::PolicyAndOwned {
            required: &[Scope::Input],
            owned: OwnedAuthority::Controller,
        },
        Request::Resize { .. } => RequestAuthorization::PolicyAndOwned {
            required: &[Scope::Resize],
            owned: OwnedAuthority::Controller,
        },
        Request::Detach { .. } => RequestAuthorization::Owned(OwnedAuthority::Subscription),
        Request::KillSplint { .. } => RequestAuthorization::policy(&[Scope::ProcessTerminate]),
        Request::AuditInspect { .. } => RequestAuthorization::policy(&[Scope::AuditInspect]),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use splinterm_core::{SplintId, TopologyRevision};
    use splinterm_protocol::LaunchParameters;

    #[test]
    fn scope_vocabulary_is_closed_unique_and_schema_sized() {
        let scopes = [
            OperationScope::TopologyMetadataRead,
            OperationScope::TopologySubscribe,
            OperationScope::TerminalVisibleRead,
            OperationScope::TerminalSubscribe,
            OperationScope::ScrollbackRead,
            OperationScope::ScrollbackSearch,
            OperationScope::ControllerAcquire,
            OperationScope::ControllerTransfer,
            OperationScope::Input,
            OperationScope::Resize,
            OperationScope::ProcessSpawn,
            OperationScope::ProcessRestore,
            OperationScope::ProcessTerminate,
            OperationScope::TopologyLayoutMutate,
            OperationScope::TopologyNameMutate,
            OperationScope::AuthorizationInspect,
            OperationScope::AuthorizationRevoke,
            OperationScope::AuditInspect,
        ];
        let json = serde_json::to_value(scopes).unwrap();
        let values = json.as_array().unwrap();
        let unique = values
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(values.len(), 18);
        assert_eq!(unique.len(), values.len());
        assert_eq!(values[0], "topology_metadata_read");
        assert_eq!(values[17], "audit_inspect");
    }

    #[test]
    fn transient_creation_is_trusted_ui_only() {
        assert_eq!(
            for_request(&Request::CreateTransientLair {
                expected_topology_revision: TopologyRevision::new(0),
                name: "transient".into(),
                launch: LaunchParameters {
                    cwd: PathBuf::from("/tmp"),
                    command: vec!["/bin/true".into()],
                    shell: None,
                    login_shell: false,
                    scrollback_lines: 1_000,
                },
            }),
            RequestAuthorization::TrustedUi
        );
        assert_eq!(
            for_request(&Request::MaterializePreset {
                expected_topology_revision: TopologyRevision::new(0),
                target: splinterm_protocol::PresetTarget::NewLair {
                    name: "preset".into(),
                },
                dojos: Vec::new(),
                directory_identities: Vec::new(),
            }),
            RequestAuthorization::TrustedUi
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the exhaustive sensitive-request matrix is intentionally reviewed together"
    )]
    fn sensitive_matrix_keeps_policy_ownership_and_trusted_ui_distinct() {
        let splint_id = SplintId::new();
        assert_eq!(
            for_request(&Request::ReadGraphicalFocus),
            RequestAuthorization::Authenticated
        );
        assert_eq!(
            for_request(&Request::PublishGraphicalFocus {
                focused_splint_id: Some(splint_id),
            }),
            RequestAuthorization::TrustedUi
        );
        assert_eq!(
            for_request(&Request::StartSearchScrollback {
                splint_id,
                incarnation: None,
                query: "needle".into(),
                case_sensitive: false,
                max_results: 1,
            }),
            RequestAuthorization::Policy {
                required: &[
                    OperationScope::TerminalVisibleRead,
                    OperationScope::ScrollbackRead,
                    OperationScope::ScrollbackSearch,
                ],
                any_of: &[],
            }
        );
        assert_eq!(
            for_request(&Request::SearchScrollback {
                splint_id,
                incarnation: 1,
                terminal_revision: 1,
                history_generation: 1,
                query: "needle".into(),
                case_sensitive: false,
                cursor: None,
                max_results: 1,
            }),
            RequestAuthorization::Policy {
                required: &[
                    OperationScope::TerminalVisibleRead,
                    OperationScope::ScrollbackRead,
                    OperationScope::ScrollbackSearch,
                ],
                any_of: &[],
            }
        );
        assert_eq!(
            for_request(&Request::PrepareMutation {
                mutation: splinterm_protocol::MutationPreflight::SplitSplint { splint_id },
            }),
            RequestAuthorization::Policy {
                required: &[
                    OperationScope::ProcessSpawn,
                    OperationScope::TopologyLayoutMutate,
                ],
                any_of: &[],
            }
        );
        assert_eq!(
            for_request(&Request::PrepareMutation {
                mutation: splinterm_protocol::MutationPreflight::CloseSplint { splint_id },
            }),
            RequestAuthorization::Conditional {
                base: &[OperationScope::TopologyLayoutMutate],
                requirement: ConditionalRequirement::LiveProcessTermination,
            }
        );
        assert_eq!(
            for_request(&Request::PrepareMutation {
                mutation: splinterm_protocol::MutationPreflight::RenameSplint { splint_id },
            }),
            RequestAuthorization::Policy {
                required: &[OperationScope::TopologyNameMutate],
                any_of: &[],
            }
        );
        assert_eq!(
            for_request(&Request::Input {
                controller_id: 1,
                splint_id,
                incarnation: 1,
                bytes: vec![],
            }),
            RequestAuthorization::PolicyAndOwned {
                required: &[OperationScope::Input],
                owned: OwnedAuthority::Controller,
            }
        );
        assert_eq!(
            for_request(&Request::ForceControlTransfer {
                splint_id,
                incarnation: 1,
            }),
            RequestAuthorization::Conditional {
                base: &[
                    OperationScope::ControllerAcquire,
                    OperationScope::ControllerTransfer,
                ],
                requirement: ConditionalRequirement::RequestedControlTakeover,
            }
        );
    }
}
