//! The one outcome record a daemon handoff sends each carried Window
//! (Plan 0061 M3.13c; ADR 0011, "Smooth local client recovery").
//!
//! Before exec, a Window holds the client end of its continuation channel,
//! a Unix `SOCK_SEQPACKET` socket that arrived with
//! [`crate::ServerFrame::HandoffPrepare`]. The daemon that adopts the
//! handed-off state sends it exactly one [`HandoffOutcome`] on that channel,
//! once its adoption has committed: [`HandoffOutcomeKind::Adopted`] from the
//! forward daemon, or [`HandoffOutcomeKind::RolledBack`] from the rollback
//! daemon after a failed forward candidate re-executed it. Nothing is sent
//! before a commit, and a cancelled attempt sends nothing: the Window then
//! reads end of file.
//!
//! The record is one fixed-size packet of [`HANDOFF_OUTCOME_BYTES`] bytes,
//! never native struct layout:
//!
//! | bytes  | field                                             |
//! |--------|---------------------------------------------------|
//! | 0..4   | magic, ASCII `SPHO`                               |
//! | 4      | version, [`HANDOFF_OUTCOME_VERSION`]              |
//! | 5      | kind: 1 `Adopted`, 2 `RolledBack`                 |
//! | 6..8   | reserved, zero                                    |
//! | 8..16  | `daemon_generation`, little-endian `u64`, nonzero |
//!
//! `daemon_generation` is the generation the sending daemon now serves in
//! `Hello`: one past the old generation after adoption, the old generation
//! itself after rollback. A receiver reads the packet into a buffer larger
//! than the record, so a longer packet is seen as one and not truncated
//! silently, and passes exactly what it read to [`HandoffOutcome::decode`],
//! which refuses any other length, magic, version, kind, nonzero reserved
//! byte, or a zero generation. A new layout takes a new version.

use thiserror::Error;

/// The exact size of one outcome packet.
pub const HANDOFF_OUTCOME_BYTES: usize = 16;
/// The first four bytes of every outcome packet.
pub const HANDOFF_OUTCOME_MAGIC: [u8; 4] = *b"SPHO";
/// The layout version this crate writes and reads.
pub const HANDOFF_OUTCOME_VERSION: u8 = 1;

/// Which daemon adopted the handed-off state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffOutcomeKind {
    /// The forward daemon committed its adoption: the Window relaunches into
    /// the sealed forward client snapshot.
    Adopted,
    /// The forward candidate failed before its commit and the rollback
    /// daemon committed instead: the Window closes its handoff artifacts and
    /// reconnects to the old generation.
    RolledBack,
}

impl HandoffOutcomeKind {
    const fn wire(self) -> u8 {
        match self {
            Self::Adopted => 1,
            Self::RolledBack => 2,
        }
    }

    const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Adopted),
            2 => Some(Self::RolledBack),
            _ => None,
        }
    }
}

/// One outcome record; see the module documentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandoffOutcome {
    pub kind: HandoffOutcomeKind,
    /// The generation the sending daemon serves; never zero.
    pub daemon_generation: u64,
}

/// Why bytes are not an outcome record.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HandoffOutcomeError {
    #[error("handoff outcome packet is {0} bytes, not {HANDOFF_OUTCOME_BYTES}")]
    Length(usize),
    #[error("handoff outcome packet has the wrong magic")]
    Magic,
    #[error("handoff outcome packet version {0} is not supported")]
    Version(u8),
    #[error("handoff outcome kind {0} is not known")]
    Kind(u8),
    #[error("handoff outcome packet has nonzero reserved bytes")]
    Reserved,
    #[error("handoff outcome names daemon generation zero")]
    Generation,
}

impl HandoffOutcome {
    /// The packet for this outcome.
    #[must_use]
    pub const fn encode(self) -> [u8; HANDOFF_OUTCOME_BYTES] {
        let generation = self.daemon_generation.to_le_bytes();
        let mut packet = [0_u8; HANDOFF_OUTCOME_BYTES];
        packet[0] = HANDOFF_OUTCOME_MAGIC[0];
        packet[1] = HANDOFF_OUTCOME_MAGIC[1];
        packet[2] = HANDOFF_OUTCOME_MAGIC[2];
        packet[3] = HANDOFF_OUTCOME_MAGIC[3];
        packet[4] = HANDOFF_OUTCOME_VERSION;
        packet[5] = self.kind.wire();
        let mut index = 0;
        while index < 8 {
            packet[8 + index] = generation[index];
            index += 1;
        }
        packet
    }

    /// Decodes exactly one packet as received.
    ///
    /// # Errors
    /// Refuses a packet of any other length, magic, version, kind, nonzero
    /// reserved bytes, or a zero generation.
    pub fn decode(packet: &[u8]) -> Result<Self, HandoffOutcomeError> {
        let packet: &[u8; HANDOFF_OUTCOME_BYTES] = packet
            .try_into()
            .map_err(|_| HandoffOutcomeError::Length(packet.len()))?;
        if packet[..4] != HANDOFF_OUTCOME_MAGIC {
            return Err(HandoffOutcomeError::Magic);
        }
        if packet[4] != HANDOFF_OUTCOME_VERSION {
            return Err(HandoffOutcomeError::Version(packet[4]));
        }
        let kind =
            HandoffOutcomeKind::from_wire(packet[5]).ok_or(HandoffOutcomeError::Kind(packet[5]))?;
        if packet[6..8] != [0, 0] {
            return Err(HandoffOutcomeError::Reserved);
        }
        let mut generation = [0_u8; 8];
        generation.copy_from_slice(&packet[8..]);
        let daemon_generation = u64::from_le_bytes(generation);
        if daemon_generation == 0 {
            return Err(HandoffOutcomeError::Generation);
        }
        Ok(Self {
            kind,
            daemon_generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_layout_is_fixed() {
        let adopted = HandoffOutcome {
            kind: HandoffOutcomeKind::Adopted,
            daemon_generation: 0x0102_0304_0506_0708,
        }
        .encode();
        assert_eq!(
            adopted,
            [
                b'S', b'P', b'H', b'O', 1, 1, 0, 0, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01
            ]
        );
        let rolled_back = HandoffOutcome {
            kind: HandoffOutcomeKind::RolledBack,
            daemon_generation: 1,
        }
        .encode();
        assert_eq!(
            rolled_back,
            [b'S', b'P', b'H', b'O', 1, 2, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn every_outcome_round_trips() {
        for kind in [HandoffOutcomeKind::Adopted, HandoffOutcomeKind::RolledBack] {
            for daemon_generation in [1, 2, u64::MAX] {
                let outcome = HandoffOutcome {
                    kind,
                    daemon_generation,
                };
                assert_eq!(HandoffOutcome::decode(&outcome.encode()), Ok(outcome));
            }
        }
    }

    #[test]
    fn malformed_packets_are_refused() {
        let valid = HandoffOutcome {
            kind: HandoffOutcomeKind::Adopted,
            daemon_generation: 2,
        }
        .encode();
        assert_eq!(
            HandoffOutcome::decode(&valid[..15]),
            Err(HandoffOutcomeError::Length(15))
        );
        let mut long = valid.to_vec();
        long.push(0);
        assert_eq!(
            HandoffOutcome::decode(&long),
            Err(HandoffOutcomeError::Length(17))
        );
        assert_eq!(
            HandoffOutcome::decode(&[]),
            Err(HandoffOutcomeError::Length(0))
        );
        let mutate = |index: usize, value: u8| {
            let mut packet = valid;
            packet[index] = value;
            HandoffOutcome::decode(&packet)
        };
        assert_eq!(mutate(0, b'X'), Err(HandoffOutcomeError::Magic));
        assert_eq!(mutate(4, 2), Err(HandoffOutcomeError::Version(2)));
        assert_eq!(mutate(5, 0), Err(HandoffOutcomeError::Kind(0)));
        assert_eq!(mutate(5, 3), Err(HandoffOutcomeError::Kind(3)));
        assert_eq!(mutate(6, 1), Err(HandoffOutcomeError::Reserved));
        assert_eq!(mutate(7, 1), Err(HandoffOutcomeError::Reserved));
        let zero = HandoffOutcome {
            kind: HandoffOutcomeKind::RolledBack,
            daemon_generation: 0,
        }
        .encode();
        assert_eq!(
            HandoffOutcome::decode(&zero),
            Err(HandoffOutcomeError::Generation)
        );
    }
}
