//! Reconnecting after a lost daemon connection (Plan 0061 M3.12b).
//!
//! A committed daemon upgrade (handoff) ends every ordinary client
//! connection: the old image execs into the new one, which keeps the
//! listening socket, the process, and every Splint, and serves the next
//! daemon generation. A rollback execs back in the same process and keeps the
//! old generation number. A restart is a new process.
//!
//! These helpers tell a lost connection from other failures, back off between
//! reconnection attempts, and say which daemon answered afterwards. They
//! never replay a request: whether a request in flight took effect is
//! unknown, so callers re-read state and leave mutations to their user.

use std::time::Duration;

use crate::{Connection, DaemonClosedConnection};

/// First pause between reconnection attempts.
pub const RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(100);

/// Longest pause between reconnection attempts.
pub const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(2);

/// The daemon behind one connection: the generation its handshake reported
/// and, for a local socket, its process from `SO_PEERCRED`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DaemonIdentity {
    pub generation: u64,
    pub pid: Option<u32>,
}

impl DaemonIdentity {
    /// The daemon that answered `connection`'s handshake.
    #[must_use]
    pub const fn of(connection: &Connection) -> Self {
        Self {
            generation: connection.daemon_generation(),
            pid: connection.daemon_pid(),
        }
    }

    /// What changed between this daemon and the one that answers `after`.
    ///
    /// Two known, different processes mean a restart. Otherwise a newer
    /// generation means a committed upgrade, the same generation means the
    /// same daemon (no handoff, or a rollback, which keeps the generation
    /// number), and an older one means a restart. Over a relayed transport no
    /// process is known and only the generation decides; a restarted remote
    /// daemon also ends its relay session, so it is rarely reached that way.
    #[must_use]
    pub const fn change_to(self, after: Self) -> DaemonChange {
        if let (Some(before), Some(now)) = (self.pid, after.pid)
            && before != now
        {
            return DaemonChange::Restarted;
        }
        if after.generation > self.generation {
            DaemonChange::Upgraded {
                from: self.generation,
                to: after.generation,
            }
        } else if after.generation == self.generation {
            DaemonChange::Same
        } else {
            DaemonChange::Restarted
        }
    }
}

/// Which daemon answered a reconnection, compared with the one before.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonChange {
    /// The same daemon generation: no handoff committed, or it rolled back.
    /// Its Splints are the ones the lost connection saw.
    Same,
    /// A newer generation of the same daemon: an upgrade committed. Its
    /// Splints carried over; connection-owned IDs and controllers did not.
    Upgraded { from: u64, to: u64 },
    /// A different daemon: splinterd restarted, and the lost connection's
    /// live terminals most likely ended.
    Restarted,
}

/// Whether `error` means the daemon connection itself was lost: the daemon
/// closed it, cut a frame short, or the socket broke. A protocol error, a
/// refusal, or a client-side cancellation is not a loss.
#[must_use]
pub fn is_connection_loss(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if cause.downcast_ref::<DaemonClosedConnection>().is_some()
            || cause
                .downcast_ref::<crate::DaemonConnectionTruncated>()
                .is_some()
        {
            return true;
        }
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
            )
        })
    })
}

/// Exponential backoff between reconnection attempts, from
/// [`RECONNECT_INITIAL_DELAY`] doubling up to [`RECONNECT_MAX_DELAY`].
#[derive(Clone, Debug)]
pub struct ReconnectBackoff {
    next: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            next: RECONNECT_INITIAL_DELAY,
        }
    }
}

impl ReconnectBackoff {
    /// The pause before the next attempt; each call doubles the following one.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(RECONNECT_MAX_DELAY);
        delay
    }
}

/// Runs `attempt` until it succeeds or `deadline` passes, pausing with
/// [`ReconnectBackoff`] between attempts; never pauses past `deadline`. An
/// attempt that is still running at the deadline is abandoned. Returns the
/// last error once time is up.
///
/// # Errors
///
/// The last attempt's error, or a timeout when no attempt finished.
pub async fn retry_until<T, F, Fut>(
    deadline: tokio::time::Instant,
    mut attempt: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut backoff = ReconnectBackoff::default();
    loop {
        let error = match tokio::time::timeout_at(deadline, attempt()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error)) => error,
            Err(_) => anyhow::bail!("splinterd did not answer before the reconnection deadline"),
        };
        let resume = tokio::time::Instant::now() + backoff.next_delay();
        if resume >= deadline {
            return Err(error);
        }
        tokio::time::sleep_until(resume).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn identity(generation: u64, pid: Option<u32>) -> DaemonIdentity {
        DaemonIdentity { generation, pid }
    }

    #[test]
    fn a_newer_generation_of_the_same_process_is_an_upgrade() {
        let before = identity(3, Some(40));
        assert_eq!(
            before.change_to(identity(4, Some(40))),
            DaemonChange::Upgraded { from: 3, to: 4 }
        );
        // No handoff, or a rollback, which keeps the generation number.
        assert_eq!(before.change_to(identity(3, Some(40))), DaemonChange::Same);
    }

    #[test]
    fn a_new_process_or_an_older_generation_is_a_restart() {
        let before = identity(3, Some(40));
        assert_eq!(
            before.change_to(identity(3, Some(41))),
            DaemonChange::Restarted
        );
        assert_eq!(
            before.change_to(identity(9, Some(41))),
            DaemonChange::Restarted
        );
        assert_eq!(before.change_to(identity(1, None)), DaemonChange::Restarted);
        // Without process evidence only the generation decides.
        assert_eq!(
            identity(2, None).change_to(identity(3, None)),
            DaemonChange::Upgraded { from: 2, to: 3 }
        );
        assert_eq!(
            identity(2, None).change_to(identity(2, Some(7))),
            DaemonChange::Same
        );
    }

    #[test]
    fn lost_connections_are_told_apart_from_refusals_and_cancellation() {
        assert!(is_connection_loss(&anyhow::Error::new(
            DaemonClosedConnection
        )));
        assert!(is_connection_loss(
            &anyhow::Error::new(crate::DaemonConnectionTruncated("a partial frame"))
                .context("reading")
        ));
        for kind in [
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::BrokenPipe,
        ] {
            assert!(is_connection_loss(&anyhow::Error::new(
                std::io::Error::from(kind)
            )));
        }
        assert!(!is_connection_loss(&anyhow::Error::new(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied)
        )));
        assert!(!is_connection_loss(&crate::response_protocol_error(
            splinterm_protocol::ProtocolError::new(
                splinterm_protocol::ErrorCode::HandoffInProgress,
                "fenced",
            )
        )));
        assert!(!is_connection_loss(&anyhow::anyhow!(
            "splinterd sent invalid JSON"
        )));
    }

    #[test]
    fn backoff_doubles_up_to_its_cap() {
        let mut backoff = ReconnectBackoff::default();
        let delays = (0..7).map(|_| backoff.next_delay()).collect::<Vec<_>>();
        assert_eq!(
            delays,
            [100, 200, 400, 800, 1_600, 2_000, 2_000].map(Duration::from_millis)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retries_back_off_until_an_attempt_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let started = tokio::time::Instant::now();
        let counter = Arc::clone(&attempts);
        let value = retry_until(started + Duration::from_secs(30), move || {
            let counter = Arc::clone(&counter);
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) < 3 {
                    anyhow::bail!("refused")
                }
                Ok(7)
            }
        })
        .await
        .unwrap();
        assert_eq!(value, 7);
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        // 100 + 200 + 400 ms of backoff before the fourth attempt.
        assert_eq!(started.elapsed(), Duration::from_millis(700));
    }

    #[tokio::test(start_paused = true)]
    async fn retries_stop_at_the_deadline_with_the_last_error() {
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_secs(30);
        let result: anyhow::Result<()> =
            retry_until(deadline, || async { anyhow::bail!("still down") }).await;
        let error = result.unwrap_err();
        assert!(error.to_string().contains("still down"));
        assert!(started.elapsed() <= Duration::from_secs(30));
        assert!(started.elapsed() >= Duration::from_secs(28));
        // A hung attempt is abandoned at the deadline.
        let started = tokio::time::Instant::now();
        let error = retry_until(started + Duration::from_secs(5), || {
            std::future::pending::<anyhow::Result<()>>()
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("reconnection deadline"));
        assert_eq!(started.elapsed(), Duration::from_secs(5));
    }
}
