//! `SCM_RIGHTS` transport for [`ServerFrame::HandoffPrepare`].
//!
//! The handing-off daemon writes the complete length-prefixed frame with one
//! `sendmsg` whose ancillary data carries exactly
//! [`HANDOFF_PREPARE_DESCRIPTORS`] descriptors in a fixed order: the sealed
//! forward client snapshot, then the client end of the continuation channel,
//! a Unix `SOCK_SEQPACKET` socket. The Unix stream socket the frame travels
//! on attaches ancillary data to the first byte of that write and never
//! merges it into an earlier write, so a receiver that reads
//! each frame header and body with exact-length [`recv_with_descriptors`]
//! calls receives the descriptors with that frame's header read and with no
//! other read. Such a receiver passes them to [`accept_handoff_prepare`]
//! with the decoded frame, and treats descriptors that arrive with a body
//! read as a protocol violation.
//!
//! A receiver that reads with plain `read(2)` never gains the descriptors:
//! the kernel closes them. It then decodes the frame and must reject it as
//! a protocol violation, which is what every current client does.

use std::{
    io::{IoSlice, IoSliceMut},
    mem::MaybeUninit,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
};

use rustix::{
    fs::{FileType, SealFlags, fcntl_get_seals, fstat},
    io::Errno,
    net::{
        AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
        SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketType, recvmsg, sendmsg,
        sockopt::{socket_domain, socket_type},
    },
};

use crate::{
    FrameEncodeError, HANDOFF_PREPARE_DESCRIPTORS, HandoffPrepare, ServerFrame, encode_frame,
};

/// Seals the forward client snapshot must carry: its bytes, size, and seal
/// set can never change again.
pub const REQUIRED_CLIENT_SNAPSHOT_SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);

/// The two descriptors a [`ServerFrame::HandoffPrepare`] frame carries, in
/// wire order. Dropping the value closes both.
#[derive(Debug)]
pub struct HandoffPrepareDescriptors {
    client_snapshot: OwnedFd,
    continuation: OwnedFd,
}

impl HandoffPrepareDescriptors {
    #[must_use]
    pub const fn new(client_snapshot: OwnedFd, continuation: OwnedFd) -> Self {
        Self {
            client_snapshot,
            continuation,
        }
    }

    /// The sealed forward client snapshot.
    #[must_use]
    pub fn client_snapshot(&self) -> BorrowedFd<'_> {
        self.client_snapshot.as_fd()
    }

    /// The client end of the continuation channel.
    #[must_use]
    pub fn continuation(&self) -> BorrowedFd<'_> {
        self.continuation.as_fd()
    }

    /// Returns the client snapshot and continuation descriptors.
    #[must_use]
    pub fn into_parts(self) -> (OwnedFd, OwnedFd) {
        (self.client_snapshot, self.continuation)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HandoffDescriptorError {
    #[error("descriptor ancillary data was truncated")]
    Truncated,
    #[error(
        "received {received} descriptors; a handoff prepare frame carries exactly {HANDOFF_PREPARE_DESCRIPTORS}"
    )]
    Count { received: usize },
    #[error("ancillary data held more than one rights record or an unexpected record")]
    UnexpectedAncillary,
    #[error("descriptors arrived with a frame that is not a handoff prepare frame")]
    UnexpectedDescriptors,
    #[error("a handoff prepare frame arrived without its descriptors")]
    MissingDescriptors,
    #[error("handoff prepare frame metadata is invalid")]
    InvalidFrame,
    #[error("the client snapshot descriptor is not the sealed snapshot the frame names")]
    ClientSnapshot,
    #[error("the continuation descriptor is not a Unix sequenced-packet socket")]
    Continuation,
    #[error("handoff prepare frame encoding failed")]
    Encode(#[source] FrameEncodeError),
    #[error("descriptor transport failed")]
    Io(#[source] Errno),
}

/// Encodes one validated [`ServerFrame::HandoffPrepare`] frame.
///
/// # Errors
/// Rejects invalid frame metadata or an encoding failure.
pub fn encode_handoff_prepare(prepare: &HandoffPrepare) -> Result<Vec<u8>, HandoffDescriptorError> {
    prepare
        .validate()
        .map_err(|_| HandoffDescriptorError::InvalidFrame)?;
    encode_frame(&ServerFrame::HandoffPrepare { prepare: *prepare })
        .map_err(HandoffDescriptorError::Encode)
}

/// Writes the start of one encoded handoff prepare frame with its
/// descriptors attached, and returns how many bytes the kernel accepted.
///
/// The caller writes `encoded[sent..]` before any other byte on `socket`.
///
/// # Errors
/// Rejects an empty frame. A `WOULDBLOCK` [`HandoffDescriptorError::Io`] on
/// a nonblocking socket sends nothing, descriptors included, and may be
/// retried.
pub fn send_handoff_prepare(
    socket: BorrowedFd<'_>,
    encoded: &[u8],
    descriptors: &HandoffPrepareDescriptors,
) -> Result<usize, HandoffDescriptorError> {
    if encoded.is_empty() {
        return Err(HandoffDescriptorError::InvalidFrame);
    }
    let rights = [descriptors.client_snapshot(), descriptors.continuation()];
    let mut space =
        [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(HANDOFF_PREPARE_DESCRIPTORS))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    if !ancillary.push(SendAncillaryMessage::ScmRights(&rights)) {
        return Err(HandoffDescriptorError::Truncated);
    }
    sendmsg(
        socket,
        &[IoSlice::new(encoded)],
        &mut ancillary,
        SendFlags::NOSIGNAL,
    )
    .map_err(HandoffDescriptorError::Io)
}

/// Bytes from one [`recv_with_descriptors`] call.
#[derive(Debug)]
pub struct ReceivedBytes {
    /// Bytes written to the front of the buffer; zero at end of stream.
    pub bytes: usize,
    /// Present only when exactly [`HANDOFF_PREPARE_DESCRIPTORS`] arrived.
    pub descriptors: Option<HandoffPrepareDescriptors>,
}

/// Receives bytes and any descriptors attached to them, close-on-exec.
///
/// Room exists for exactly [`HANDOFF_PREPARE_DESCRIPTORS`] descriptors, so a
/// sender passing more is reported as truncated. Every descriptor received
/// by a failing call is closed before it returns.
///
/// # Errors
/// Rejects truncated ancillary data, more than one rights record, any other
/// ancillary record, or a descriptor count other than zero or exactly
/// [`HANDOFF_PREPARE_DESCRIPTORS`]. A `WOULDBLOCK` [`HandoffDescriptorError::Io`]
/// received nothing.
pub fn recv_with_descriptors(
    socket: BorrowedFd<'_>,
    buffer: &mut [u8],
) -> Result<ReceivedBytes, HandoffDescriptorError> {
    let mut space =
        [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(HANDOFF_PREPARE_DESCRIPTORS))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut space);
    let message = recvmsg(
        socket,
        &mut [IoSliceMut::new(buffer)],
        &mut ancillary,
        RecvFlags::CMSG_CLOEXEC,
    )
    .map_err(HandoffDescriptorError::Io)?;
    // Own every delivered descriptor first, so each failure below closes them.
    let mut received = Vec::with_capacity(HANDOFF_PREPARE_DESCRIPTORS);
    let mut records = 0_usize;
    let mut unexpected = false;
    for item in ancillary.drain() {
        match item {
            RecvAncillaryMessage::ScmRights(descriptors) => {
                records += 1;
                received.extend(descriptors);
            }
            _ => unexpected = true,
        }
    }
    if message.flags.contains(ReturnFlags::CTRUNC) {
        return Err(HandoffDescriptorError::Truncated);
    }
    if unexpected || records > 1 {
        return Err(HandoffDescriptorError::UnexpectedAncillary);
    }
    let descriptors = if received.is_empty() {
        None
    } else {
        let [client_snapshot, continuation] =
            <[OwnedFd; HANDOFF_PREPARE_DESCRIPTORS]>::try_from(received).map_err(|received| {
                HandoffDescriptorError::Count {
                    received: received.len(),
                }
            })?;
        Some(HandoffPrepareDescriptors::new(
            client_snapshot,
            continuation,
        ))
    };
    Ok(ReceivedBytes {
        bytes: message.bytes,
        descriptors,
    })
}

/// Pairs one decoded frame with the descriptors that arrived with its
/// header, and verifies the descriptors are the ones the frame names.
///
/// Returns `Ok(None)` for an ordinary frame without descriptors. On any
/// error the descriptors are closed.
///
/// # Errors
/// Rejects descriptors beside any other frame, a handoff prepare frame
/// without descriptors or with invalid metadata, a client snapshot that is
/// not the exact sealed regular file named by the frame, and a continuation
/// descriptor that is not a socket.
pub fn accept_handoff_prepare(
    frame: &ServerFrame,
    descriptors: Option<HandoffPrepareDescriptors>,
) -> Result<Option<(HandoffPrepare, HandoffPrepareDescriptors)>, HandoffDescriptorError> {
    match (frame, descriptors) {
        (ServerFrame::HandoffPrepare { prepare }, Some(descriptors)) => {
            prepare
                .validate()
                .map_err(|_| HandoffDescriptorError::InvalidFrame)?;
            verify_descriptors(prepare, &descriptors)?;
            Ok(Some((*prepare, descriptors)))
        }
        (ServerFrame::HandoffPrepare { .. }, None) => {
            Err(HandoffDescriptorError::MissingDescriptors)
        }
        (_, Some(_)) => Err(HandoffDescriptorError::UnexpectedDescriptors),
        (_, None) => Ok(None),
    }
}

fn verify_descriptors(
    prepare: &HandoffPrepare,
    descriptors: &HandoffPrepareDescriptors,
) -> Result<(), HandoffDescriptorError> {
    let expected = prepare.client_snapshot;
    let snapshot =
        fstat(descriptors.client_snapshot()).map_err(|_| HandoffDescriptorError::ClientSnapshot)?;
    let seals = fcntl_get_seals(descriptors.client_snapshot())
        .map_err(|_| HandoffDescriptorError::ClientSnapshot)?;
    if FileType::from_raw_mode(snapshot.st_mode) != FileType::RegularFile
        || snapshot.st_dev != expected.device
        || snapshot.st_ino != expected.inode
        || u64::try_from(snapshot.st_size).ok() != Some(expected.size)
        || !seals.contains(REQUIRED_CLIENT_SNAPSHOT_SEALS)
    {
        return Err(HandoffDescriptorError::ClientSnapshot);
    }
    let continuation =
        fstat(descriptors.continuation()).map_err(|_| HandoffDescriptorError::Continuation)?;
    let domain = socket_domain(descriptors.continuation())
        .map_err(|_| HandoffDescriptorError::Continuation)?;
    let kind = socket_type(descriptors.continuation())
        .map_err(|_| HandoffDescriptorError::Continuation)?;
    // The continuation channel is a bounded message channel (ADR 0011), never
    // a byte stream or a network socket.
    if FileType::from_raw_mode(continuation.st_mode) != FileType::Socket
        || domain != AddressFamily::UNIX
        || kind != SocketType::SEQPACKET
    {
        return Err(HandoffDescriptorError::Continuation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::{fs::MetadataExt as _, net::UnixStream};

    use rustix::fs::{MemfdFlags, fcntl_add_seals, memfd_create};

    use super::*;
    use crate::{HandoffClientSnapshot, MAX_HANDOFF_CLIENT_SNAPSHOT_BYTES, Response};

    const SNAPSHOT_BYTES: &[u8] = b"\x7fELF forward client snapshot";

    fn sealed_snapshot(seals: SealFlags) -> OwnedFd {
        let fd = memfd_create(
            "handoff-client-snapshot",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap();
        assert_eq!(
            rustix::io::write(&fd, SNAPSHOT_BYTES).unwrap(),
            SNAPSHOT_BYTES.len()
        );
        fcntl_add_seals(&fd, seals).unwrap();
        fd
    }

    fn prepare_for(snapshot: &OwnedFd) -> HandoffPrepare {
        let stat = fstat(snapshot).unwrap();
        HandoffPrepare {
            daemon_generation: 1,
            descriptors: HANDOFF_PREPARE_DESCRIPTORS,
            client_snapshot: HandoffClientSnapshot {
                device: stat.st_dev,
                inode: stat.st_ino,
                size: u64::try_from(stat.st_size).unwrap(),
                sha256: [7; 32],
            },
        }
    }

    fn continuation() -> OwnedFd {
        let (local, _peer) = rustix::net::socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        local
    }

    /// Whether this process still holds any descriptor for the open file with
    /// this device and inode. Inodes of memfds and sockets are unique, so
    /// concurrent tests cannot alias them.
    fn still_open(identity: (u64, u64)) -> bool {
        std::fs::read_dir("/proc/self/fd").unwrap().any(|entry| {
            std::fs::metadata(entry.unwrap().path())
                .is_ok_and(|target| (target.dev(), target.ino()) == identity)
        })
    }

    fn identity(descriptor: &OwnedFd) -> (u64, u64) {
        let stat = fstat(descriptor).unwrap();
        (stat.st_dev, stat.st_ino)
    }

    fn send_rights(socket: &UnixStream, bytes: &[u8], rights: &[BorrowedFd<'_>]) {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(8))];
        let mut ancillary = SendAncillaryBuffer::new(&mut space);
        assert!(ancillary.push(SendAncillaryMessage::ScmRights(rights)));
        assert_eq!(
            sendmsg(
                socket,
                &[IoSlice::new(bytes)],
                &mut ancillary,
                SendFlags::NOSIGNAL
            )
            .unwrap(),
            bytes.len()
        );
    }

    /// Reads one frame the way a participating receiver must: an exact
    /// header read, then an exact body read.
    fn receive_frame(
        socket: &UnixStream,
    ) -> Result<(ServerFrame, Option<HandoffPrepareDescriptors>), HandoffDescriptorError> {
        let mut header = [0_u8; 4];
        let received = recv_with_descriptors(socket.as_fd(), &mut header)?;
        assert_eq!(received.bytes, 4);
        let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
        let body_read = recv_with_descriptors(socket.as_fd(), &mut body)?;
        assert_eq!(body_read.bytes, body.len());
        assert!(
            body_read.descriptors.is_none(),
            "descriptors arrive only with the header read"
        );
        Ok((serde_json::from_slice(&body).unwrap(), received.descriptors))
    }

    #[test]
    fn handoff_prepare_round_trips_with_exactly_two_verified_descriptors() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        // An ordinary frame queued first never picks up the later descriptors.
        let ordinary = encode_frame(&ServerFrame::Response {
            request_id: 1,
            result: Response::Pong,
        })
        .unwrap();
        std::io::Write::write_all(&mut &sender, &ordinary).unwrap();
        let snapshot = sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS);
        let prepare = prepare_for(&snapshot);
        let encoded = encode_handoff_prepare(&prepare).unwrap();
        let descriptors = HandoffPrepareDescriptors::new(snapshot, continuation());
        assert_eq!(
            send_handoff_prepare(sender.as_fd(), &encoded, &descriptors).unwrap(),
            encoded.len()
        );
        drop(descriptors);

        let (first, first_descriptors) = receive_frame(&receiver).unwrap();
        assert!(matches!(first, ServerFrame::Response { .. }));
        assert!(
            accept_handoff_prepare(&first, first_descriptors)
                .unwrap()
                .is_none()
        );
        let (frame, attached) = receive_frame(&receiver).unwrap();
        let (accepted, descriptors) = accept_handoff_prepare(&frame, attached)
            .unwrap()
            .expect("handoff prepare frame is accepted");
        assert_eq!(accepted, prepare);
        let mut contents = vec![0_u8; SNAPSHOT_BYTES.len()];
        assert_eq!(
            rustix::io::pread(descriptors.client_snapshot(), &mut contents, 0).unwrap(),
            SNAPSHOT_BYTES.len()
        );
        assert_eq!(contents, SNAPSHOT_BYTES);
        let flags = rustix::io::fcntl_getfd(descriptors.continuation()).unwrap();
        assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
    }

    #[test]
    fn wrong_descriptor_counts_and_records_fail_closed() {
        let snapshot = sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS);
        let encoded = encode_handoff_prepare(&prepare_for(&snapshot)).unwrap();
        let extra = continuation();
        let channel = continuation();

        let (sender, receiver) = UnixStream::pair().unwrap();
        send_rights(&sender, &encoded, &[snapshot.as_fd()]);
        let mut header = [0_u8; 4];
        assert!(matches!(
            recv_with_descriptors(receiver.as_fd(), &mut header),
            Err(HandoffDescriptorError::Count { received: 1 })
        ));

        // The receive buffer's alignment slack may admit a third descriptor;
        // it is then refused by count instead of by truncation.
        let (sender, receiver) = UnixStream::pair().unwrap();
        send_rights(
            &sender,
            &encoded,
            &[snapshot.as_fd(), channel.as_fd(), extra.as_fd()],
        );
        assert!(matches!(
            recv_with_descriptors(receiver.as_fd(), &mut header),
            Err(HandoffDescriptorError::Count { received: 3 } | HandoffDescriptorError::Truncated)
        ));

        let (sender, receiver) = UnixStream::pair().unwrap();
        let many = [snapshot.as_fd(); 8];
        send_rights(&sender, &encoded, &many);
        assert!(matches!(
            recv_with_descriptors(receiver.as_fd(), &mut header),
            Err(HandoffDescriptorError::Truncated)
        ));
    }

    #[test]
    fn descriptors_and_frames_must_pair_exactly() {
        let snapshot = sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS);
        let prepare = prepare_for(&snapshot);
        let ordinary = ServerFrame::Response {
            request_id: 1,
            result: Response::Pong,
        };
        assert!(matches!(
            accept_handoff_prepare(
                &ordinary,
                Some(HandoffPrepareDescriptors::new(
                    sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS),
                    continuation()
                ))
            ),
            Err(HandoffDescriptorError::UnexpectedDescriptors)
        ));
        let frame = ServerFrame::HandoffPrepare { prepare };
        assert!(matches!(
            accept_handoff_prepare(&frame, None),
            Err(HandoffDescriptorError::MissingDescriptors)
        ));

        // Fds attached to an ordinary frame are rejected after decoding.
        let (sender, receiver) = UnixStream::pair().unwrap();
        let channel = continuation();
        send_rights(
            &sender,
            &encode_frame(&ordinary).unwrap(),
            &[snapshot.as_fd(), channel.as_fd()],
        );
        let (decoded, attached) = receive_frame(&receiver).unwrap();
        assert!(matches!(
            accept_handoff_prepare(&decoded, attached),
            Err(HandoffDescriptorError::UnexpectedDescriptors)
        ));
    }

    #[test]
    fn descriptors_must_match_the_named_sealed_snapshot_and_a_socket() {
        let snapshot = sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS);
        let prepare = prepare_for(&snapshot);
        let frame = ServerFrame::HandoffPrepare { prepare };
        let accept = |client_snapshot, continuation| {
            accept_handoff_prepare(
                &frame,
                Some(HandoffPrepareDescriptors::new(
                    client_snapshot,
                    continuation,
                )),
            )
        };

        // A different memfd with the same size is not the named snapshot.
        assert!(matches!(
            accept(
                sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS),
                continuation()
            ),
            Err(HandoffDescriptorError::ClientSnapshot)
        ));
        // A writable snapshot is never accepted.
        let unsealed = sealed_snapshot(SealFlags::empty());
        let mut unsealed_frame = frame.clone();
        if let ServerFrame::HandoffPrepare { prepare } = &mut unsealed_frame {
            *prepare = prepare_for(&unsealed);
        }
        assert!(matches!(
            accept_handoff_prepare(
                &unsealed_frame,
                Some(HandoffPrepareDescriptors::new(unsealed, continuation()))
            ),
            Err(HandoffDescriptorError::ClientSnapshot)
        ));
        // Swapped order fails on the snapshot check.
        assert!(matches!(
            accept(continuation(), snapshot.try_clone().unwrap()),
            Err(HandoffDescriptorError::ClientSnapshot)
        ));
        // The continuation descriptor must be a socket.
        assert!(matches!(
            accept(
                snapshot.try_clone().unwrap(),
                sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS)
            ),
            Err(HandoffDescriptorError::Continuation)
        ));
        assert!(accept(snapshot, continuation()).unwrap().is_some());
    }

    #[test]
    fn continuation_must_be_a_unix_seqpacket_socket_and_rejection_closes_both() {
        let (stream, stream_peer) = UnixStream::pair().unwrap();
        let datagram = rustix::net::socket(AddressFamily::UNIX, SocketType::DGRAM, None).unwrap();
        // A network socket needs no network: an unbound IPv4 datagram socket.
        let network = rustix::net::socket(AddressFamily::INET, SocketType::DGRAM, None).unwrap();
        for wrong in [OwnedFd::from(stream), datagram, network] {
            let snapshot = sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS);
            let frame = ServerFrame::HandoffPrepare {
                prepare: prepare_for(&snapshot),
            };
            let held = [identity(&snapshot), identity(&wrong)];
            assert!(held.iter().all(|descriptor| still_open(*descriptor)));
            assert!(matches!(
                accept_handoff_prepare(
                    &frame,
                    Some(HandoffPrepareDescriptors::new(snapshot, wrong))
                ),
                Err(HandoffDescriptorError::Continuation)
            ));
            assert!(
                held.iter().all(|descriptor| !still_open(*descriptor)),
                "a rejected frame closes both descriptors"
            );
        }
        // The stream's own end is closed, so its peer reads end of stream.
        let mut byte = [0_u8; 1];
        assert_eq!(
            std::io::Read::read(&mut &stream_peer, &mut byte).unwrap(),
            0
        );
    }

    #[test]
    fn invalid_prepare_metadata_is_never_encoded_or_accepted() {
        let snapshot = sealed_snapshot(REQUIRED_CLIENT_SNAPSHOT_SEALS);
        let valid = prepare_for(&snapshot);
        for invalid in [
            HandoffPrepare {
                daemon_generation: 0,
                ..valid
            },
            HandoffPrepare {
                descriptors: 1,
                ..valid
            },
            HandoffPrepare {
                descriptors: 3,
                ..valid
            },
            HandoffPrepare {
                client_snapshot: HandoffClientSnapshot {
                    size: 0,
                    ..valid.client_snapshot
                },
                ..valid
            },
            HandoffPrepare {
                client_snapshot: HandoffClientSnapshot {
                    size: MAX_HANDOFF_CLIENT_SNAPSHOT_BYTES + 1,
                    ..valid.client_snapshot
                },
                ..valid
            },
            HandoffPrepare {
                client_snapshot: HandoffClientSnapshot {
                    sha256: [0; 32],
                    ..valid.client_snapshot
                },
                ..valid
            },
        ] {
            assert!(matches!(
                encode_handoff_prepare(&invalid),
                Err(HandoffDescriptorError::InvalidFrame)
            ));
            assert!(matches!(
                accept_handoff_prepare(
                    &ServerFrame::HandoffPrepare { prepare: invalid },
                    Some(HandoffPrepareDescriptors::new(
                        snapshot.try_clone().unwrap(),
                        continuation()
                    ))
                ),
                Err(HandoffDescriptorError::InvalidFrame)
            ));
        }
        let (sender, _receiver) = UnixStream::pair().unwrap();
        assert!(matches!(
            send_handoff_prepare(
                sender.as_fd(),
                &[],
                &HandoffPrepareDescriptors::new(snapshot, continuation())
            ),
            Err(HandoffDescriptorError::InvalidFrame)
        ));
    }

    #[test]
    fn handoff_prepare_frame_has_an_explicit_wire_shape() {
        let prepare = HandoffPrepare {
            daemon_generation: 3,
            descriptors: HANDOFF_PREPARE_DESCRIPTORS,
            client_snapshot: HandoffClientSnapshot {
                device: 5,
                inode: 9,
                size: 4096,
                sha256: [1; 32],
            },
        };
        let value = serde_json::to_value(ServerFrame::HandoffPrepare { prepare }).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "handoff_prepare",
                "prepare": {
                    "daemon_generation": 3,
                    "descriptors": 2,
                    "client_snapshot": {
                        "device": 5,
                        "inode": 9,
                        "size": 4096,
                        "sha256": vec![1_u8; 32]
                    }
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<ServerFrame>(value).unwrap(),
            ServerFrame::HandoffPrepare { prepare }
        );
    }
}
