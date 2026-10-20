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
    /// The connection was lost while the command ran; `after` is what the
    /// reconnection found.
    Lost { after: Probe },
}

/// What looking for the daemon after a lost connection found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) enum Probe {
    /// A daemon answered; compared with the one before.
    Answered(DaemonChange),
    /// None answered within `window`.
    Unanswered { window: Duration },
    /// The command's deadline left no time to look.
    Skipped,
}

/// A one-shot command that a daemon handoff interrupted; never retried when
/// it may have changed anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) struct InterruptedCommand {
    pub(in crate::app) interruption: Interruption,
    /// The command only reads, so running it again is always safe.
    pub(in crate::app) read_only: bool,
    /// The read was already sent once more after a first lost connection.
    pub(in crate::app) retried: bool,
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

/// A probe window in plain words.
fn window_words(window: Duration) -> String {
    if window.subsec_millis() == 0 {
        format!("{} seconds", window.as_secs())
    } else {
        format!("{:.1} seconds", window.as_secs_f64())
    }
}

impl fmt::Display for InterruptedCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const LOST: &str = "the connection to splinterd was lost while this command ran, as happens during an upgrade";
        let what = match self.interruption {
            Interruption::Refused => {
                return formatter.write_str(
                    "splinterd is upgrading, so it refused this command, which did not run; run it again once the upgrade finishes (`splinterm upgrade` reports the daemon's state)",
                );
            }
            Interruption::Lost {
                after: Probe::Answered(DaemonChange::Upgraded { from, to }),
            } => {
                format!("splinterd was upgraded while this command ran (generation {from} → {to})")
            }
            Interruption::Lost {
                after: Probe::Answered(DaemonChange::Same),
            } => LOST.to_owned(),
            Interruption::Lost {
                after: Probe::Answered(DaemonChange::Restarted),
            } => "splinterd restarted while this command ran".to_owned(),
            Interruption::Lost {
                after: Probe::Unanswered { window },
            } => format!(
                "{LOST}, and no daemon answered within {}",
                window_words(window)
            ),
            Interruption::Lost {
                after: Probe::Skipped,
            } => format!("{LOST}, and its deadline left no time to look for the daemon"),
        };
        if self.read_only && self.retried {
            write!(
                formatter,
                "{what}; the read was retried once and was interrupted again, so run it again"
            )
        } else if self.read_only {
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
/// lost connection (whose probe has not run yet).
#[must_use]
pub(in crate::app) fn interruption_kind(error: &anyhow::Error) -> Option<Interruption> {
    if protocol_error(error).is_some_and(|error| error.code == ErrorCode::HandoffInProgress) {
        Some(Interruption::Refused)
    } else if is_connection_loss(error) {
        Some(Interruption::Lost {
            after: Probe::Skipped,
        })
    } else {
        None
    }
}

/// The interrupted command `error` describes, if any.
#[must_use]
pub(in crate::app) fn interrupted(error: &anyhow::Error) -> Option<InterruptedCommand> {
    error.downcast_ref::<InterruptedCommand>().copied()
}

/// Marks an interrupted read as already retried once.
#[must_use]
pub(in crate::app) fn retried_once(error: anyhow::Error) -> anyhow::Error {
    match interrupted(&error) {
        Some(interrupted) => anyhow::Error::new(InterruptedCommand {
            retried: true,
            ..interrupted
        }),
        None => error,
    }
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

/// What probing for up to `window` finds; `None` skips the probe.
pub(in crate::app) async fn probe<F, Fut>(
    before: DaemonIdentity,
    window: Option<Duration>,
    connect: F,
) -> Probe
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Connection>>,
{
    let Some(window) = window else {
        return Probe::Skipped;
    };
    probe_daemon(before, window, connect)
        .await
        .map_or(Probe::Unanswered { window }, |(_, change)| {
            Probe::Answered(change)
        })
}

/// Turns a command's handoff refusal or lost connection into an
/// [`InterruptedCommand`], probing for up to `window` for the daemon that
/// serves now (`None`: no time left, so no probe); any other error is
/// returned as it is. Nothing is retried here.
pub(in crate::app) async fn explain_interruption<F, Fut>(
    error: anyhow::Error,
    before: DaemonIdentity,
    read_only: bool,
    window: Option<Duration>,
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
            after: probe(before, window, connect).await,
        },
    };
    anyhow::Error::new(InterruptedCommand {
        interruption,
        read_only,
        retried: false,
    })
}

/// [`explain_interruption`] for a command's whole result.
pub(in crate::app) async fn explain_result<T, F, Fut>(
    result: anyhow::Result<T>,
    before: DaemonIdentity,
    read_only: bool,
    window: Option<Duration>,
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
/// what is left of its deadline, at most [`PROBE_WINDOW`]. `None` when no
/// time is left, so the reconnection never outlasts `--timeout-ms`.
#[must_use]
pub(in crate::app) fn machine_probe_window(remaining: Duration) -> Option<Duration> {
    (!remaining.is_zero()).then(|| remaining.min(PROBE_WINDOW))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lost(after: Probe, read_only: bool) -> InterruptedCommand {
        InterruptedCommand {
            interruption: Interruption::Lost { after },
            read_only,
            retried: false,
        }
    }

    fn upgraded(from: u64, to: u64) -> Probe {
        Probe::Answered(DaemonChange::Upgraded { from, to })
    }

    #[test]
    fn an_interrupted_mutation_names_the_upgrade_and_is_never_called_done() {
        let text = lost(upgraded(3, 4), false).to_string();
        assert_eq!(
            text,
            "splinterd was upgraded while this command ran (generation 3 → 4); the command was not retried and may or may not have taken effect, so check the current state before running it again"
        );
        let text = lost(
            Probe::Unanswered {
                window: PROBE_WINDOW,
            },
            false,
        )
        .to_string();
        assert!(text.contains("as happens during an upgrade"), "{text}");
        assert!(
            text.contains("no daemon answered within 10 seconds"),
            "{text}"
        );
        assert!(text.contains("was not retried"), "{text}");
        assert!(
            lost(Probe::Answered(DaemonChange::Restarted), false)
                .to_string()
                .starts_with("splinterd restarted while this command ran;")
        );
        assert_eq!(
            lost(Probe::Answered(DaemonChange::Same), false).public_code(),
            (CliErrorCodeV2::Internal, false)
        );
    }

    #[test]
    fn an_interrupted_read_says_whether_it_was_already_retried() {
        let read = lost(upgraded(1, 2), true);
        assert!(
            read.to_string()
                .ends_with("the read was not retried, so run it again")
        );
        assert_eq!(read.public_code(), (CliErrorCodeV2::Internal, true));
        assert_eq!(read.exit_code(), 4);
        // Review S2: a read retried once says so.
        let error = retried_once(anyhow::Error::new(read));
        assert!(
            error
                .to_string()
                .ends_with("the read was retried once and was interrupted again, so run it again"),
            "{error}"
        );
        assert!(interrupted(&error).unwrap().retried);
        // Anything else passes through unchanged.
        assert_eq!(retried_once(anyhow::anyhow!("other")).to_string(), "other");
    }

    #[test]
    fn a_refused_command_did_not_run() {
        let refused = InterruptedCommand {
            interruption: Interruption::Refused,
            read_only: false,
            retried: false,
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
        assert!(matches!(
            interruption_kind(&closed),
            Some(Interruption::Lost { .. })
        ));
        let stale = splinterm::automation::response_protocol_error(
            splinterm_protocol::ProtocolError::new(ErrorCode::StaleTopology, "stale"),
        );
        assert_eq!(interruption_kind(&stale), None);
    }

    /// Review S3: the reconnection fits what is left of `--timeout-ms`, and
    /// with nothing left there is no probe at all.
    #[test]
    fn a_machine_probe_never_outlasts_the_deadline() {
        assert_eq!(
            machine_probe_window(Duration::from_secs(3)),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            machine_probe_window(Duration::from_millis(40)),
            Some(Duration::from_millis(40))
        );
        assert_eq!(
            machine_probe_window(Duration::from_secs(60)),
            Some(PROBE_WINDOW)
        );
        assert_eq!(machine_probe_window(Duration::ZERO), None);
    }

    #[tokio::test]
    async fn with_no_time_left_the_daemon_is_not_looked_for() {
        let before = DaemonIdentity {
            generation: 1,
            pid: None,
        };
        let error = explain_interruption(
            anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
            before,
            false,
            None,
            || async { panic!("no reconnection without time left") },
        )
        .await;
        let interrupted = interrupted(&error).unwrap();
        assert_eq!(
            interrupted.interruption,
            Interruption::Lost {
                after: Probe::Skipped
            }
        );
        assert!(
            error
                .to_string()
                .contains("its deadline left no time to look for the daemon"),
            "{error}"
        );
        let text = lost(
            Probe::Unanswered {
                window: Duration::from_millis(1_500),
            },
            true,
        )
        .to_string();
        assert!(
            text.contains("no daemon answered within 1.5 seconds"),
            "{text}"
        );
    }
}
