//! A relaunched Window's controller claim after a daemon upgrade (Plan 0061
//! M3.16; ADR 0011, "Smooth local client recovery").
//!
//! The Window that relaunched into the sealed forward client snapshot has
//! reconnected as trusted UI and read the adopted daemon's authoritative
//! topology (`crate::window_continuation`, M3.15). It then:
//!
//! 1. sends one [`HandoffTicketClaim`] on its inherited continuation
//!    channel, naming the generation it reached, and waits at most
//!    [`TICKET_CLAIM_DEADLINE`] for the [`HandoffTicketGrant`] carrying its
//!    resume ticket ([`claim_resume_ticket`]); the kernel attaches the
//!    process's credentials, which the daemon checks with its pinned pidfd
//!    and the snapshot's identity. The channel then closes;
//! 2. arms its [`HandoffControlRestore`] with the ticket and the Splints of
//!    the tabs it restores, until [`CONTROL_RESTORE_WINDOW`] passes;
//! 3. as each pane attaches and resnapshots, the pane's own control
//!    connection presents the ticket once for its Splint
//!    (`ClaimHandoffControl`), and keeps the controller it gets back.
//!
//! Every wait is bounded. A refusal, a timeout, or a rollback (whose daemon
//! issues no ticket) leaves the panes to take control the ordinary way, with
//! a short notice. Nothing typed before the upgrade is replayed.

use std::{
    collections::HashSet,
    fmt, io,
    os::fd::OwnedFd,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use rustix::net::{RecvFlags, SendFlags};
use splinterm_core::SplintId;
use splinterm_protocol::{
    HandoffResumeTicket,
    handoff_continuation::{HANDOFF_TICKET_GRANT_BYTES, HandoffTicketClaim, HandoffTicketGrant},
};
use tokio::io::{Interest, unix::AsyncFd};

/// How long a relaunched Window waits for its ticket after claiming. The
/// daemon answers at once or closes the channel; this only bounds a daemon
/// that does neither.
pub const TICKET_CLAIM_DEADLINE: Duration = Duration::from_secs(5);
/// How long after the grant a pane may still present the ticket. The daemon
/// holds the Window for 30 s from its commit, which came before the
/// relaunch, so a later claim would be refused anyway.
pub const CONTROL_RESTORE_WINDOW: Duration = Duration::from_secs(30);

/// Why a relaunched Window got no ticket.
#[derive(Debug)]
pub enum TicketClaimError {
    /// The claim could not be sent.
    Send(io::Error),
    /// The daemon closed the channel without a grant: it refused the claim.
    Refused,
    /// No answer within [`TICKET_CLAIM_DEADLINE`].
    TimedOut,
    /// The answer was not a grant.
    Malformed,
    /// The grant names another generation than the one the Window reached.
    Generation,
    /// The channel could not be read.
    Receive(io::Error),
}

impl fmt::Display for TicketClaimError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Send(error) => write!(formatter, "cannot send the claim: {error}"),
            Self::Refused => formatter.write_str("the daemon refused the claim"),
            Self::TimedOut => formatter.write_str("the daemon did not answer the claim"),
            Self::Malformed => formatter.write_str("the daemon's answer is not a ticket"),
            Self::Generation => formatter.write_str("the ticket names another daemon generation"),
            Self::Receive(error) => write!(formatter, "cannot read the answer: {error}"),
        }
    }
}

impl std::error::Error for TicketClaimError {}

/// Claims this Window's resume ticket over its continuation channel, which
/// it consumes: the channel closes on return, whatever the result.
///
/// # Errors
/// [`TicketClaimError`] when no valid grant for `daemon_generation`
/// arrives within `deadline`.
pub async fn claim_resume_ticket(
    continuation: OwnedFd,
    daemon_generation: u64,
    deadline: Duration,
) -> Result<HandoffResumeTicket, TicketClaimError> {
    let claim = HandoffTicketClaim { daemon_generation }.encode();
    let sent = rustix::net::send(
        &continuation,
        &claim,
        SendFlags::NOSIGNAL | SendFlags::DONTWAIT,
    )
    .map_err(|error| TicketClaimError::Send(error.into()))?;
    if sent != claim.len() {
        return Err(TicketClaimError::Send(io::Error::new(
            io::ErrorKind::WriteZero,
            "the claim was not sent whole",
        )));
    }
    let channel = AsyncFd::with_interest(continuation, Interest::READABLE)
        .map_err(TicketClaimError::Receive)?;
    let grant = tokio::time::timeout(deadline, async {
        // Larger than a grant, so a longer packet is seen whole and refused.
        let mut buffer = [0_u8; 2 * HANDOFF_TICKET_GRANT_BYTES];
        loop {
            let mut ready = channel
                .readable()
                .await
                .map_err(TicketClaimError::Receive)?;
            match ready.try_io(|inner| {
                rustix::net::recv(inner.get_ref(), &mut buffer, RecvFlags::DONTWAIT)
                    .map_err(io::Error::from)
            }) {
                Ok(Ok((0, _))) => return Err(TicketClaimError::Refused),
                Ok(Ok((read, _))) => {
                    let read = read.min(buffer.len());
                    return HandoffTicketGrant::decode(&buffer[..read])
                        .map_err(|_| TicketClaimError::Malformed);
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionReset => {
                    return Err(TicketClaimError::Refused);
                }
                Ok(Err(error)) => return Err(TicketClaimError::Receive(error)),
                Err(_would_block) => {}
            }
        }
    })
    .await
    .map_err(|_| TicketClaimError::TimedOut)??;
    if grant.daemon_generation != daemon_generation {
        return Err(TicketClaimError::Generation);
    }
    Ok(grant.ticket)
}

#[derive(Debug)]
struct ArmedRestore {
    ticket: HandoffResumeTicket,
    /// Splints whose pane has not presented the ticket yet.
    pending: HashSet<SplintId>,
    until: tokio::time::Instant,
}

/// The resume ticket a relaunched Window's panes present, one claim per
/// Splint. Shared by every clone of the Window's connection factory; empty,
/// and never armed, in every other Window.
#[derive(Clone, Debug, Default)]
pub struct HandoffControlRestore {
    armed: Arc<Mutex<Option<ArmedRestore>>>,
}

impl HandoffControlRestore {
    /// Arms the restore with `ticket` for `splints`, the Splints of the
    /// restored tabs, until `window` passes.
    pub fn arm(
        &self,
        ticket: HandoffResumeTicket,
        splints: impl IntoIterator<Item = SplintId>,
        window: Duration,
    ) {
        let pending = splints.into_iter().collect::<HashSet<_>>();
        *self.armed.lock().unwrap_or_else(PoisonError::into_inner) =
            (!pending.is_empty()).then(|| ArmedRestore {
                ticket,
                pending,
                until: tokio::time::Instant::now() + window,
            });
    }

    /// The ticket for `splint_id`'s pane, at most once per Splint and only
    /// while armed. The restore disarms once every Splint took it or its
    /// window passed.
    #[must_use]
    pub fn take(&self, splint_id: SplintId) -> Option<HandoffResumeTicket> {
        let mut armed = self.armed.lock().unwrap_or_else(PoisonError::into_inner);
        let current = armed.as_mut()?;
        if tokio::time::Instant::now() >= current.until {
            *armed = None;
            return None;
        }
        if !current.pending.remove(&splint_id) {
            return None;
        }
        let ticket = current.ticket;
        if current.pending.is_empty() {
            *armed = None;
        }
        Some(ticket)
    }

    /// Whether any pane may still present the ticket.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
    use splinterm_protocol::HANDOFF_RESUME_TICKET_BYTES;

    use super::*;

    fn pair() -> (OwnedFd, OwnedFd) {
        socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap()
    }

    fn recv_claim(daemon: &OwnedFd) -> HandoffTicketClaim {
        let mut buffer = [0_u8; 64];
        let (read, _) = rustix::net::recv(daemon, &mut buffer, RecvFlags::empty()).unwrap();
        HandoffTicketClaim::decode(&buffer[..read]).unwrap()
    }

    fn grant(daemon_generation: u64) -> [u8; HANDOFF_TICKET_GRANT_BYTES] {
        HandoffTicketGrant {
            daemon_generation,
            ticket: HandoffResumeTicket::new([0x4e; HANDOFF_RESUME_TICKET_BYTES]),
        }
        .encode()
    }

    #[tokio::test]
    async fn a_claim_names_the_reached_generation_and_takes_the_grant() {
        let (client, daemon) = pair();
        let answering = tokio::task::spawn_blocking(move || {
            assert_eq!(recv_claim(&daemon).daemon_generation, 5);
            rustix::net::send(&daemon, &grant(5), SendFlags::empty()).unwrap();
            daemon
        });
        let ticket = claim_resume_ticket(client, 5, TICKET_CLAIM_DEADLINE)
            .await
            .unwrap();
        assert_eq!(
            ticket,
            HandoffResumeTicket::new([0x4e; HANDOFF_RESUME_TICKET_BYTES])
        );
        // The client's end is closed once the claim returns.
        let daemon = answering.await.unwrap();
        let mut buffer = [0_u8; 8];
        assert_eq!(
            rustix::net::recv(&daemon, &mut buffer, RecvFlags::DONTWAIT)
                .unwrap()
                .0,
            0
        );
    }

    #[tokio::test]
    async fn refusal_garbage_another_generation_or_silence_yield_no_ticket() {
        // Refused: the daemon closes without a grant.
        let (client, daemon) = pair();
        let closing = tokio::task::spawn_blocking(move || {
            recv_claim(&daemon);
            drop(daemon);
        });
        assert!(matches!(
            claim_resume_ticket(client, 5, TICKET_CLAIM_DEADLINE).await,
            Err(TicketClaimError::Refused)
        ));
        closing.await.unwrap();

        // An answer that is not a grant, or names another generation.
        for (answer, expected) in [
            (grant(5)[..20].to_vec(), "malformed"),
            ([grant(5).as_slice(), &[0]].concat(), "malformed"),
            (grant(6).to_vec(), "generation"),
        ] {
            let (client, daemon) = pair();
            let answering = tokio::task::spawn_blocking(move || {
                recv_claim(&daemon);
                rustix::net::send(&daemon, &answer, SendFlags::empty()).unwrap();
                daemon
            });
            let result = claim_resume_ticket(client, 5, TICKET_CLAIM_DEADLINE).await;
            match expected {
                "malformed" => assert!(matches!(result, Err(TicketClaimError::Malformed))),
                _ => assert!(matches!(result, Err(TicketClaimError::Generation))),
            }
            drop(answering.await.unwrap());
        }

        // Silence: bounded by the deadline.
        let (client, _daemon) = pair();
        assert!(matches!(
            claim_resume_ticket(client, 5, Duration::from_millis(50)).await,
            Err(TicketClaimError::TimedOut)
        ));

        // No daemon end at all.
        let (client, daemon) = pair();
        drop(daemon);
        assert!(matches!(
            claim_resume_ticket(client, 5, TICKET_CLAIM_DEADLINE).await,
            Err(TicketClaimError::Send(_))
        ));
    }

    #[tokio::test]
    async fn the_restore_hands_each_splint_the_ticket_once_while_armed() {
        let restore = HandoffControlRestore::default();
        let (first, second, other) = (SplintId::new(), SplintId::new(), SplintId::new());
        assert!(restore.take(first).is_none(), "never armed");
        let ticket = HandoffResumeTicket::new([3; HANDOFF_RESUME_TICKET_BYTES]);
        restore.arm(ticket, [first, second], CONTROL_RESTORE_WINDOW);
        assert!(restore.is_armed());
        assert_eq!(restore.take(first), Some(ticket));
        assert!(restore.take(first).is_none(), "once per Splint");
        assert!(restore.take(other).is_none(), "only restored Splints");
        assert_eq!(restore.take(second), Some(ticket));
        assert!(!restore.is_armed(), "disarmed once every Splint took it");

        restore.arm(ticket, [first], Duration::ZERO);
        assert!(restore.take(first).is_none(), "its window passed");
        assert!(!restore.is_armed());
        restore.arm(ticket, [], CONTROL_RESTORE_WINDOW);
        assert!(!restore.is_armed(), "nothing to restore arms nothing");
    }
}
