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
    AttachScrollback,
    LiveProcessTermination,
    ExpandedLiveProcessTermination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestAuthorization {
    Authenticated,
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
        splinterm_protocol::MutationPreflight::CreateDojo
        | splinterm_protocol::MutationPreflight::SplitSplint { .. }
        | splinterm_protocol::MutationPreflight::NewWindow { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn, Scope::TopologyLayoutMutate])
        }
        splinterm_protocol::MutationPreflight::RelaunchSplint { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn])
        }
        splinterm_protocol::MutationPreflight::RestoreSplint { .. }
        | splinterm_protocol::MutationPreflight::RestoreWindow { .. }
        | splinterm_protocol::MutationPreflight::RestoreDojo { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessRestore])
        }
        splinterm_protocol::MutationPreflight::CloseSplint { .. } => {
            RequestAuthorization::Conditional {
                base: &[Scope::TopologyLayoutMutate],
                requirement: ConditionalRequirement::LiveProcessTermination,
            }
        }
        splinterm_protocol::MutationPreflight::CloseWindow { .. } => {
            RequestAuthorization::Conditional {
                base: &[Scope::TopologyLayoutMutate],
                requirement: ConditionalRequirement::ExpandedLiveProcessTermination,
            }
        }
        splinterm_protocol::MutationPreflight::KillSplint { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessTerminate])
        }
        splinterm_protocol::MutationPreflight::SetSplitRatio { .. }
        | splinterm_protocol::MutationPreflight::SetWindowDefaultFocus { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyLayoutMutate])
        }
        splinterm_protocol::MutationPreflight::RenameDojo { .. }
        | splinterm_protocol::MutationPreflight::RenameWindow { .. }
        | splinterm_protocol::MutationPreflight::RenameSplint { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyNameMutate])
        }
    }
}

#[must_use]
pub const fn for_request(request: &Request) -> RequestAuthorization {
    use OperationScope as Scope;

    match request {
        Request::Ping => RequestAuthorization::Authenticated,
        Request::ListDojos | Request::InspectTopology | Request::InspectSplint { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyMetadataRead])
        }
        Request::SubscribeTopology => {
            RequestAuthorization::policy(&[Scope::TopologySubscribe, Scope::TopologyMetadataRead])
        }
        Request::RequestAccess { .. } => RequestAuthorization::Conditional {
            base: &[],
            requirement: ConditionalRequirement::RequestedAccessScopes,
        },
        Request::AuthorizationStatus { .. } => {
            RequestAuthorization::policy(&[Scope::AuthorizationInspect])
        }
        Request::RevokeAccess { .. } => RequestAuthorization::policy(&[Scope::AuthorizationRevoke]),
        Request::PrepareMutation { mutation } => preflight_authorization(mutation),
        Request::CreateDojo { .. }
        | Request::CreateDojoAutomation { .. }
        | Request::SplitSplint { .. }
        | Request::SplitSplintAutomation { .. }
        | Request::NewWindow { .. }
        | Request::NewWindowAutomation { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn, Scope::TopologyLayoutMutate])
        }
        Request::RelaunchSplint { .. } | Request::RelaunchSplintAutomation { .. } => {
            RequestAuthorization::policy(&[Scope::ProcessSpawn])
        }
        Request::RestoreSplint { .. }
        | Request::RestoreWindow { .. }
        | Request::RestoreDojo { .. } => RequestAuthorization::policy(&[Scope::ProcessRestore]),
        Request::CloseSplint { .. } => RequestAuthorization::Conditional {
            base: &[Scope::TopologyLayoutMutate],
            requirement: ConditionalRequirement::LiveProcessTermination,
        },
        Request::CloseWindow { .. } => RequestAuthorization::Conditional {
            base: &[Scope::TopologyLayoutMutate],
            requirement: ConditionalRequirement::ExpandedLiveProcessTermination,
        },
        Request::SetSplitRatio { .. } | Request::SetWindowDefaultFocus { .. } => {
            RequestAuthorization::policy(&[Scope::TopologyLayoutMutate])
        }
        Request::RenameDojo { .. }
        | Request::RenameWindow { .. }
        | Request::RenameSplint { .. } => {
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
        Request::ForceControlTransfer { .. } => RequestAuthorization::TrustedUiConsent,
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
    use super::*;
    use splinterm_core::SplintId;

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
    fn sensitive_matrix_keeps_policy_ownership_and_trusted_ui_distinct() {
        let splint_id = SplintId::new();
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
            RequestAuthorization::TrustedUiConsent
        );
    }
}
