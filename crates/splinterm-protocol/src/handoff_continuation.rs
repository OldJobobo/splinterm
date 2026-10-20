//! The two packets of a Window's resume-ticket claim on its continuation
//! channel (Plan 0061 M3.16; ADR 0011, "Smooth local client recovery").
//!
//! After the adopted daemon sent its [`crate::handoff_outcome::HandoffOutcome`],
//! the continuation channel carries exactly one exchange, and only after an
//! adoption: the relaunched Window, now executing the sealed forward client
//! snapshot, sends one [`HandoffTicketClaim`]; the daemon checks the kernel's
//! sender credentials, the Window's pinned process, and its executable, and
//! answers with one [`HandoffTicketGrant`] carrying a single-use resume
//! ticket, then closes its end. A refused claim is answered by closing the
//! channel with nothing sent, so the Window reads end of file. Nothing else
//! ever crosses the channel.
//!
//! Both packets have a fixed size and an explicit layout, never native struct
//! layout. A receiver reads into a buffer larger than the packet, so a longer
//! packet is seen as one, and decodes exactly what it read.
//!
//! The claim, [`HANDOFF_TICKET_CLAIM_BYTES`] bytes:
//!
//! | bytes  | field                                             |
//! |--------|---------------------------------------------------|
//! | 0..4   | magic, ASCII `SPHC`                               |
//! | 4      | version, [`HANDOFF_CONTINUATION_VERSION`]         |
//! | 5      | kind, 1                                           |
//! | 6..8   | reserved, zero                                    |
//! | 8..16  | `daemon_generation`, little-endian `u64`, nonzero |
//!
//! The grant, [`HANDOFF_TICKET_GRANT_BYTES`] bytes:
//!
//! | bytes  | field                                             |
//! |--------|---------------------------------------------------|
//! | 0..4   | magic, ASCII `SPHT`                               |
//! | 4      | version, [`HANDOFF_CONTINUATION_VERSION`]         |
//! | 5      | kind, 1                                           |
//! | 6..8   | reserved, zero                                    |
//! | 8..16  | `daemon_generation`, little-endian `u64`, nonzero |
//! | 16..48 | the resume ticket, [`HANDOFF_RESUME_TICKET_BYTES`] bytes |
//!
//! `daemon_generation` is the generation the adopted daemon serves in
//! `Hello`: the Window names the one it reached, and the daemon refuses any
//! other. A new layout takes a new version.

use thiserror::Error;

use crate::{HANDOFF_RESUME_TICKET_BYTES, HandoffResumeTicket};

/// The layout version of both packets.
pub const HANDOFF_CONTINUATION_VERSION: u8 = 1;
/// The exact size of one claim packet.
pub const HANDOFF_TICKET_CLAIM_BYTES: usize = 16;
/// The exact size of one grant packet.
pub const HANDOFF_TICKET_GRANT_BYTES: usize = 16 + HANDOFF_RESUME_TICKET_BYTES;
/// The first four bytes of every claim packet.
pub const HANDOFF_TICKET_CLAIM_MAGIC: [u8; 4] = *b"SPHC";
/// The first four bytes of every grant packet.
pub const HANDOFF_TICKET_GRANT_MAGIC: [u8; 4] = *b"SPHT";
const KIND: u8 = 1;

/// A relaunched Window's one request for its resume ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandoffTicketClaim {
    /// The generation the Window reached after its relaunch; never zero.
    pub daemon_generation: u64,
}

/// The adopted daemon's answer to a valid claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandoffTicketGrant {
    /// The generation the daemon serves; never zero.
    pub daemon_generation: u64,
    /// The single-use resume ticket the Window presents with
    /// [`crate::Request::ClaimHandoffControl`]. `Debug` never prints it.
    pub ticket: HandoffResumeTicket,
}

/// Why bytes are not a continuation claim or grant.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HandoffContinuationError {
    #[error("handoff continuation packet is {0} bytes, not the expected size")]
    Length(usize),
    #[error("handoff continuation packet has the wrong magic")]
    Magic,
    #[error("handoff continuation packet version {0} is not supported")]
    Version(u8),
    #[error("handoff continuation packet kind {0} is not known")]
    Kind(u8),
    #[error("handoff continuation packet has nonzero reserved bytes")]
    Reserved,
    #[error("handoff continuation packet names daemon generation zero")]
    Generation,
}

fn header(magic: [u8; 4], daemon_generation: u64) -> [u8; 16] {
    let mut packet = [0_u8; 16];
    packet[..4].copy_from_slice(&magic);
    packet[4] = HANDOFF_CONTINUATION_VERSION;
    packet[5] = KIND;
    packet[8..16].copy_from_slice(&daemon_generation.to_le_bytes());
    packet
}

fn decode_header(packet: &[u8], magic: [u8; 4]) -> Result<u64, HandoffContinuationError> {
    if packet[..4] != magic {
        return Err(HandoffContinuationError::Magic);
    }
    if packet[4] != HANDOFF_CONTINUATION_VERSION {
        return Err(HandoffContinuationError::Version(packet[4]));
    }
    if packet[5] != KIND {
        return Err(HandoffContinuationError::Kind(packet[5]));
    }
    if packet[6..8] != [0, 0] {
        return Err(HandoffContinuationError::Reserved);
    }
    let mut generation = [0_u8; 8];
    generation.copy_from_slice(&packet[8..16]);
    match u64::from_le_bytes(generation) {
        0 => Err(HandoffContinuationError::Generation),
        generation => Ok(generation),
    }
}

impl HandoffTicketClaim {
    /// The packet for this claim.
    #[must_use]
    pub fn encode(self) -> [u8; HANDOFF_TICKET_CLAIM_BYTES] {
        header(HANDOFF_TICKET_CLAIM_MAGIC, self.daemon_generation)
    }

    /// Decodes exactly one packet as received.
    ///
    /// # Errors
    /// Refuses any other length, magic, version, kind, nonzero reserved
    /// bytes, or a zero generation.
    pub fn decode(packet: &[u8]) -> Result<Self, HandoffContinuationError> {
        if packet.len() != HANDOFF_TICKET_CLAIM_BYTES {
            return Err(HandoffContinuationError::Length(packet.len()));
        }
        Ok(Self {
            daemon_generation: decode_header(packet, HANDOFF_TICKET_CLAIM_MAGIC)?,
        })
    }
}

impl HandoffTicketGrant {
    /// The packet for this grant.
    #[must_use]
    pub fn encode(self) -> [u8; HANDOFF_TICKET_GRANT_BYTES] {
        let mut packet = [0_u8; HANDOFF_TICKET_GRANT_BYTES];
        packet[..16].copy_from_slice(&header(HANDOFF_TICKET_GRANT_MAGIC, self.daemon_generation));
        packet[16..].copy_from_slice(self.ticket.as_bytes());
        packet
    }

    /// Decodes exactly one packet as received.
    ///
    /// # Errors
    /// Refuses any other length, magic, version, kind, nonzero reserved
    /// bytes, or a zero generation.
    pub fn decode(packet: &[u8]) -> Result<Self, HandoffContinuationError> {
        if packet.len() != HANDOFF_TICKET_GRANT_BYTES {
            return Err(HandoffContinuationError::Length(packet.len()));
        }
        let daemon_generation = decode_header(packet, HANDOFF_TICKET_GRANT_MAGIC)?;
        let mut ticket = [0_u8; HANDOFF_RESUME_TICKET_BYTES];
        ticket.copy_from_slice(&packet[16..]);
        Ok(Self {
            daemon_generation,
            ticket: HandoffResumeTicket::new(ticket),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_layouts_are_fixed() {
        assert_eq!(
            HandoffTicketClaim {
                daemon_generation: 0x0102_0304_0506_0708,
            }
            .encode(),
            [
                b'S', b'P', b'H', b'C', 1, 1, 0, 0, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01
            ]
        );
        let grant = HandoffTicketGrant {
            daemon_generation: 2,
            ticket: HandoffResumeTicket::new([0xa5; HANDOFF_RESUME_TICKET_BYTES]),
        }
        .encode();
        assert_eq!(
            grant[..16],
            [b'S', b'P', b'H', b'T', 1, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(grant[16..], [0xa5; HANDOFF_RESUME_TICKET_BYTES]);
    }

    #[test]
    fn both_packets_round_trip_and_the_grant_never_prints_its_ticket() {
        for daemon_generation in [1, 2, u64::MAX] {
            let claim = HandoffTicketClaim { daemon_generation };
            assert_eq!(HandoffTicketClaim::decode(&claim.encode()), Ok(claim));
            let grant = HandoffTicketGrant {
                daemon_generation,
                ticket: HandoffResumeTicket::new([0x5c; HANDOFF_RESUME_TICKET_BYTES]),
            };
            assert_eq!(HandoffTicketGrant::decode(&grant.encode()), Ok(grant));
            let debug = format!("{grant:?}");
            assert!(debug.contains("redacted"), "{debug}");
            assert!(!debug.contains("92"), "{debug}");
        }
    }

    #[test]
    fn malformed_packets_are_refused() {
        let claim = HandoffTicketClaim {
            daemon_generation: 2,
        }
        .encode();
        let grant = HandoffTicketGrant {
            daemon_generation: 2,
            ticket: HandoffResumeTicket::new([1; HANDOFF_RESUME_TICKET_BYTES]),
        }
        .encode();
        // Lengths, including each packet read as the other.
        assert_eq!(
            HandoffTicketClaim::decode(&claim[..15]),
            Err(HandoffContinuationError::Length(15))
        );
        assert_eq!(
            HandoffTicketClaim::decode(&grant),
            Err(HandoffContinuationError::Length(HANDOFF_TICKET_GRANT_BYTES))
        );
        assert_eq!(
            HandoffTicketGrant::decode(&claim),
            Err(HandoffContinuationError::Length(HANDOFF_TICKET_CLAIM_BYTES))
        );
        let mut long = grant.to_vec();
        long.push(0);
        assert_eq!(
            HandoffTicketGrant::decode(&long),
            Err(HandoffContinuationError::Length(49))
        );
        assert_eq!(
            HandoffTicketClaim::decode(&[]),
            Err(HandoffContinuationError::Length(0))
        );
        // A claim's header under a grant's length is not a grant.
        let mut swapped = grant;
        swapped[..4].copy_from_slice(&HANDOFF_TICKET_CLAIM_MAGIC);
        assert_eq!(
            HandoffTicketGrant::decode(&swapped),
            Err(HandoffContinuationError::Magic)
        );
        let mutate = |index: usize, value: u8| {
            let mut packet = claim;
            packet[index] = value;
            HandoffTicketClaim::decode(&packet)
        };
        assert_eq!(mutate(0, b'X'), Err(HandoffContinuationError::Magic));
        assert_eq!(mutate(4, 2), Err(HandoffContinuationError::Version(2)));
        assert_eq!(mutate(5, 0), Err(HandoffContinuationError::Kind(0)));
        assert_eq!(mutate(5, 2), Err(HandoffContinuationError::Kind(2)));
        assert_eq!(mutate(6, 1), Err(HandoffContinuationError::Reserved));
        assert_eq!(mutate(7, 1), Err(HandoffContinuationError::Reserved));
        assert_eq!(
            HandoffTicketClaim::decode(
                &HandoffTicketClaim {
                    daemon_generation: 0
                }
                .encode()
            ),
            Err(HandoffContinuationError::Generation)
        );
        let mut zero = grant;
        zero[8..16].fill(0);
        assert_eq!(
            HandoffTicketGrant::decode(&zero),
            Err(HandoffContinuationError::Generation)
        );
    }
}
