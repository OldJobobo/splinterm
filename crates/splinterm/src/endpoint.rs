//! Explicit local-versus-remote daemon endpoint capabilities.

use std::sync::Arc;

use anyhow::Result;
use splinterm_automation_client::Connection;

use crate::{remote::RemoteProfile, remote_session::RemoteSession};

/// Whether terminal image bodies may be retrieved for an endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageTransport {
    LocalTrusted,
    Unavailable,
}

/// How graphical compositor focus may be published to the daemon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicalFocusPublication {
    Enabled,
    Disabled,
}

/// Which request family must be used to create processes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchSemantics {
    LocalTrusted,
    RemoteAutomation,
}

/// Whether trusted graphical force-transfer authority is available.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForcedControlTransfer {
    Enabled,
    Disabled,
}

/// Stable behavior carried with every connection factory clone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointCapabilities {
    pub image_transport: ImageTransport,
    pub graphical_focus_publication: GraphicalFocusPublication,
    pub launch_semantics: LaunchSemantics,
    pub forced_control_transfer: ForcedControlTransfer,
    pub recency_namespace: String,
}

#[derive(Debug)]
enum EndpointKind {
    Local,
    Remote(RemoteSession),
}

/// A cloneable factory bound to exactly one local or authenticated remote endpoint.
#[derive(Clone, Debug)]
pub struct ConnectionFactory {
    endpoint: Arc<EndpointKind>,
    capabilities: Arc<EndpointCapabilities>,
}

impl ConnectionFactory {
    /// Creates the unchanged trusted local Unix-socket endpoint.
    #[must_use]
    pub fn local() -> Self {
        Self {
            endpoint: Arc::new(EndpointKind::Local),
            capabilities: Arc::new(EndpointCapabilities {
                image_transport: ImageTransport::LocalTrusted,
                graphical_focus_publication: GraphicalFocusPublication::Enabled,
                launch_semantics: LaunchSemantics::LocalTrusted,
                forced_control_transfer: ForcedControlTransfer::Enabled,
                recency_namespace: "local".to_owned(),
            }),
        }
    }

    /// Authenticates one remote profile and binds all factory clones to it.
    ///
    /// # Errors
    ///
    /// Returns an error when OpenSSH or graphical-relay negotiation fails.
    pub async fn remote(profile: &RemoteProfile) -> Result<Self> {
        let session = RemoteSession::connect(profile).await?;
        Ok(Self::from_remote_session(profile.name(), session))
    }

    #[doc(hidden)]
    #[must_use]
    pub fn from_remote_session(profile_name: &str, session: RemoteSession) -> Self {
        Self {
            endpoint: Arc::new(EndpointKind::Remote(session)),
            capabilities: Arc::new(EndpointCapabilities {
                image_transport: ImageTransport::Unavailable,
                graphical_focus_publication: GraphicalFocusPublication::Disabled,
                launch_semantics: LaunchSemantics::RemoteAutomation,
                forced_control_transfer: ForcedControlTransfer::Disabled,
                recency_namespace: format!("remote-{profile_name}"),
            }),
        }
    }

    /// Opens one protocol connection with the endpoint's fixed role and transport.
    ///
    /// # Errors
    ///
    /// Returns an error when the local socket or remote logical channel cannot be
    /// opened and negotiated.
    pub async fn connect(&self) -> Result<Connection> {
        match self.endpoint.as_ref() {
            EndpointKind::Local => Connection::connect().await,
            EndpointKind::Remote(session) => session.connect_automation().await,
        }
    }

    /// Returns the explicit endpoint behavior contract.
    #[must_use]
    pub fn capabilities(&self) -> &EndpointCapabilities {
        &self.capabilities
    }

    /// Returns true only for the owner-local trusted endpoint.
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self.endpoint.as_ref(), EndpointKind::Local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_capabilities_remain_trusted_and_remote_only_values_are_distinct() {
        let local = ConnectionFactory::local();
        assert!(local.is_local());
        assert_eq!(
            local.capabilities(),
            &EndpointCapabilities {
                image_transport: ImageTransport::LocalTrusted,
                graphical_focus_publication: GraphicalFocusPublication::Enabled,
                launch_semantics: LaunchSemantics::LocalTrusted,
                forced_control_transfer: ForcedControlTransfer::Enabled,
                recency_namespace: "local".to_owned(),
            }
        );
        assert_ne!(
            local.capabilities().image_transport,
            ImageTransport::Unavailable
        );
    }
}
