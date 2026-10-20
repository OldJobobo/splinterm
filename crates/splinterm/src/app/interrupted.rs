//! One-shot commands interrupted by a daemon upgrade (Plan 0061 M3.12b).
//!
//! A one-shot command, in human or machine mode, never retries a mutation:
//! once a request may have reached the daemon, whether it took effect is
//! unknown. A daemon fenced by a handoff refuses requests before running
//! them; a committed handoff ends the connection as the daemon execs. Either
//! way the command fails with an error that names the upgrade and says what
//! to do next. To say which daemon serves afterwards, the client reconnects
//! once (a handshake only, no request) for up to [`PROBE_WINDOW`].
//!
//! Read-only commands whose request is trivially safe to repeat (ping, list,
//! topology, inspect, focus) retry once on that probe connection. Reads with
//! continuation state (snapshot, scrollback, search, subscriptions) do not;
//! they fail with the same clear error.

use std::{fmt, future::Future, time::Duration};

use splinterm::automation::{
    CliErrorCodeV2, Connection, DaemonChange, DaemonIdentity, is_connection_loss, protocol_error,
    retry_until,
};
use splinterm_protocol::ErrorCode;

/// How long a command interrupted by a lost connection looks for the daemon
/// serving afterwards.
pub(in crate::app) const PROBE_WINDOW: Duration = Duration::from_secs(10);

/// What interrupted a command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) enum Interruption {
    /// A handoff fenced the daemon, which refused the request unrun.
    Refused,
    /// The connection was lost while the command ran. `after` is the daemon
    /// answering a reconnection, compared with the one before; `None` when
    /// none answered within [`PROBE_WINDOW`].
    Lost { after: Option<DaemonChange> },
}

/// A one-shot command that a daemon handoff interrupted; never retried when
/// it may have changed anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) struct InterruptedCommand {
    pub(in crate::app) interruption: Interruption,
    /// The command only reads, so running it again is always safe.
    pub(in crate::app) read_only: bool,
}

impl InterruptedCommand {
    /// The public v2 error code and whether running the same command again
    /// is safe as it stands. A refused request did not run. A mutation that
    /// lost its connection may have taken effect, so it is not retryable
    /// until its effect was checked.
    pub(in crate::app) const fn public_code(self) -> (CliErrorCodeV2, bool) {
        match self.interruption {
            Interruption::Refused => (CliErrorCodeV2::ResourceLimit, true),
            Interruption::Lost { .. } => (CliErrorCodeV2::Internal, self.read_only),
        }
    }

    /// The machine-mode exit code: 5 for a refusal, as for other refused
    /// requests, and 4 for a lost daemon connection.
    pub(in crate::app) const fn exit_code(self) -> i32 {
        match self.interruption {
            Interruption::Refused => 5,
            Interruption::Lost { .. } => 4,
        }
    }
}

impl fmt::Display for InterruptedCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.interruption {
            Interruption::Refused => {
                return formatter.write_str(
                    "splinterd is upgrading, so it refused this command, which did not run; run it again once the upgrade finishes (`splinterm upgrade` reports the daemon's state)",
                );
            }
            Interruption::Lost {
                after: Some(DaemonChange::Upgraded { from, to }),
            } => format!("splinterd was upgraded while this command ran (generation {from} → {to})"),
            Interruption::Lost {
                after: Some(DaemonChange::Same),
            } => "the connection to splinterd was lost while this command ran, as happens during an upgrade".to_owned(),
            Interruption::Lost {
                after: Some(DaemonChange::Restarted),
            } => "splinterd restarted while this command ran".to_owned(),
            Interruption::Lost { after: None } => format!(
                "the connection to splinterd was lost while this command ran, as happens during an upgrade, and no daemon answered within {} seconds",
                PROBE_WINDOW.as_secs()
            ),
        };
        if self.read_only {
            write!(
                formatter,
                "{what}; the read was not retried, so run it again"
            )
        } else {
            write!(
                formatter,
                "{what}; the command was not retried and may or may not have taken effect, so check the current state before running it again"
            )
        }
    }
}

impl std::error::Error for InterruptedCommand {}

/// The interruption a command's error shows, if any: a handoff refusal or a
/// lost connection.
#[must_use]
pub(in crate::app) fn interruption_kind(error: &anyhow::Error) -> Option<Interruption> {
    if protocol_error(error).is_some_and(|error| error.code == ErrorCode::HandoffInProgress) {
        Some(Interruption::Refused)
    } else if is_connection_loss(error) {
        Some(Interruption::Lost { after: None })
    } else {
        None
    }
}

/// The interrupted command `error` describes, if any.
#[must_use]
pub(in crate::app) fn interrupted(error: &anyhow::Error) -> Option<InterruptedCommand> {
    error.downcast_ref::<InterruptedCommand>().copied()
}

/// Reconnects once, for up to `window`, and says which daemon answered.
/// Only the handshake runs on the returned connection.
pub(in crate::app) async fn probe_daemon<F, Fut>(
    before: DaemonIdentity,
    window: Duration,
    connect: F,
) -> Option<(Connection, DaemonChange)>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Connection>>,
{
    let connection = retry_until(tokio::time::Instant::now() + window, connect)
        .await
        .ok()?;
    let change = before.change_to(DaemonIdentity::of(&connection));
    Some((connection, change))
}

/// Turns a command's handoff refusal or lost connection into an
/// [`InterruptedCommand`], probing for the daemon that serves now; any other
/// error is returned as it is. Nothing is retried here.
pub(in crate::app) async fn explain_interruption<F, Fut>(
    error: anyhow::Error,
    before: DaemonIdentity,
    read_only: bool,
    window: Duration,
    connect: F,
) -> anyhow::Error
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Connection>>,
{
    let interruption = match interruption_kind(&error) {
        None => return error,
        Some(Interruption::Refused) => Interruption::Refused,
        Some(Interruption::Lost { .. }) => Interruption::Lost {
            after: probe_daemon(before, window, connect)
                .await
                .map(|(_, change)| change),
        },
    };
    anyhow::Error::new(InterruptedCommand {
        interruption,
        read_only,
    })
}

/// [`explain_interruption`] for a command's whole result.
pub(in crate::app) async fn explain_result<T, F, Fut>(
    result: anyhow::Result<T>,
    before: DaemonIdentity,
    read_only: bool,
    window: Duration,
    connect: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Connection>>,
{
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(explain_interruption(error, before, read_only, window, connect).await),
    }
}

/// How long a machine command may look for the daemon after losing it:
/// what is left of its deadline, at most [`PROBE_WINDOW`], and never less
/// than a moment so a reconnection is tried at all.
#[must_use]
pub(in crate::app) fn machine_probe_window(remaining: Duration) -> Duration {
    remaining.clamp(Duration::from_millis(250), PROBE_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lost(after: Option<DaemonChange>, read_only: bool) -> InterruptedCommand {
        InterruptedCommand {
            interruption: Interruption::Lost { after },
            read_only,
        }
    }

    #[test]
    fn an_interrupted_mutation_names_the_upgrade_and_is_never_called_done() {
        let text = lost(Some(DaemonChange::Upgraded { from: 3, to: 4 }), false).to_string();
        assert_eq!(
            text,
            "splinterd was upgraded while this command ran (generation 3 → 4); the command was not retried and may or may not have taken effect, so check the current state before running it again"
        );
        let text = lost(None, false).to_string();
        assert!(text.contains("as happens during an upgrade"), "{text}");
        assert!(
            text.contains("no daemon answered within 10 seconds"),
            "{text}"
        );
        assert!(text.contains("was not retried"), "{text}");
        assert!(
            lost(Some(DaemonChange::Restarted), false)
                .to_string()
                .starts_with("splinterd restarted while this command ran;")
        );
        assert_eq!(
            lost(Some(DaemonChange::Same), false).public_code(),
            (CliErrorCodeV2::Internal, false)
        );
    }

    #[test]
    fn an_interrupted_read_says_to_run_it_again() {
        let read = lost(Some(DaemonChange::Upgraded { from: 1, to: 2 }), true);
        assert!(
            read.to_string()
                .ends_with("the read was not retried, so run it again")
        );
        assert_eq!(read.public_code(), (CliErrorCodeV2::Internal, true));
        assert_eq!(read.exit_code(), 4);
    }

    #[test]
    fn a_refused_command_did_not_run() {
        let refused = InterruptedCommand {
            interruption: Interruption::Refused,
            read_only: false,
        };
        assert!(refused.to_string().contains("did not run"));
        assert!(refused.to_string().contains("splinterd is upgrading"));
        assert_eq!(refused.public_code(), (CliErrorCodeV2::ResourceLimit, true));
        assert_eq!(refused.exit_code(), 5);
    }

    #[test]
    fn only_refusals_and_lost_connections_are_interruptions() {
        let refused = splinterm::automation::response_protocol_error(
            splinterm_protocol::ProtocolError::new(ErrorCode::HandoffInProgress, "fenced"),
        );
        assert_eq!(interruption_kind(&refused), Some(Interruption::Refused));
        let closed = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert_eq!(
            interruption_kind(&closed),
            Some(Interruption::Lost { after: None })
        );
        let stale = splinterm::automation::response_protocol_error(
            splinterm_protocol::ProtocolError::new(ErrorCode::StaleTopology, "stale"),
        );
        assert_eq!(interruption_kind(&stale), None);
    }

    #[test]
    fn a_machine_probe_fits_its_deadline_but_always_tries() {
        assert_eq!(
            machine_probe_window(Duration::from_secs(3)),
            Duration::from_secs(3)
        );
        assert_eq!(machine_probe_window(Duration::from_secs(60)), PROBE_WINDOW);
        assert_eq!(
            machine_probe_window(Duration::ZERO),
            Duration::from_millis(250)
        );
    }
}
