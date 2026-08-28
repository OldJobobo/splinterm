//! Offline compatibility preflight executed from exact sealed snapshots.
//!
//! The live daemon owns process creation and exact snapshot identities. A fresh
//! private Unix packet channel carries one challenge and one fixed-size report;
//! argv, environment, stdout text, and package paths carry no authority.

use std::{
    ffi::CString,
    io::{self, IoSlice, IoSliceMut},
    os::fd::{AsFd, AsRawFd, OwnedFd},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use nix::{
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::socket::{
        AddressFamily, ControlMessageOwned, MsgFlags, SockFlag, SockType, UnixCredentials, recvmsg,
        sendmsg, setsockopt, socketpair, sockopt::PassCred,
    },
};
use rustix::{
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
    rand::{GetRandomFlags, getrandom},
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch, apply_filter};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    executable_snapshot::{
        HandoffExecutableSnapshots, SealedExecutablePair, SealedExecutableSnapshot,
    },
    handoff_compatibility::{
        BuildIdentity, BuildVersion, ExecutablePairBinding, ExecutablePairPreflight,
        HandoffCapabilities, VersionRange,
    },
    handoff_descriptors::{DescriptorHandoffError, PreparedDescriptorInheritance},
};

const LAUNCHER_ARGUMENT: &str = "--splinterm-internal-preflight-launcher-v1";
const CHILD_ARGUMENT: &str = "--splinterm-internal-preflight-child-v1";
const WIRE_MAGIC: [u8; 8] = *b"SPLTPF01";
const WIRE_VERSION: u16 = 1;
const CHALLENGE_BYTES: usize = 44;
const REPORT_BYTES: usize = 98;
const DEFAULT_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(3);
const HANDOFF_PROTOCOL_VERSION: u16 = 1;
const TERMINAL_CHECKPOINT_VERSION: u16 = 1;
const DESCRIPTOR_MANIFEST_VERSION: u16 = 1;
#[cfg(test)]
const TEST_EXEC_NAME: &str =
    "handoff_preflight::tests::sealed_exec_authenticates_all_four_snapshots";
#[cfg(test)]
const TEST_CLEAN_STAGE: &str = "SPLINTERM_PREFLIGHT_TEST_CLEAN_STAGE";
#[cfg(test)]
const TEST_STAGE: &str = "SPLINTERM_PREFLIGHT_TEST_STAGE";
#[cfg(test)]
const TEST_ROLE: &str = "SPLINTERM_PREFLIGHT_TEST_ROLE";
#[cfg(test)]
const TEST_SECCOMP_NAME: &str =
    "handoff_preflight::tests::sealed_launcher_denies_descendant_creation";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PreflightRole {
    Daemon = 1,
    Client = 2,
}

impl PreflightRole {
    fn from_wire(value: u8) -> Result<Self, PreflightError> {
        match value {
            1 => Ok(Self::Daemon),
            2 => Ok(Self::Client),
            _ => Err(PreflightError::InvalidFrame),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PreflightGeneration {
    Forward = 1,
    Rollback = 2,
}

impl PreflightGeneration {
    fn from_wire(value: u8) -> Result<Self, PreflightError> {
        match value {
            1 => Ok(Self::Forward),
            2 => Ok(Self::Rollback),
            _ => Err(PreflightError::InvalidFrame),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChildReport {
    challenge: [u8; 32],
    role: PreflightRole,
    generation: PreflightGeneration,
    pair_build_identity: BuildIdentity,
    pair_build_version: BuildVersion,
    capabilities: HandoffCapabilities,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SealedPreflightReports {
    pub forward: ExecutablePairPreflight,
    pub rollback: ExecutablePairPreflight,
}

#[derive(Debug, Error)]
pub enum PreflightError {
    #[error("offline preflight is supported only on Linux")]
    UnsupportedPlatform,
    #[error("invalid internal preflight invocation")]
    InvalidInvocation,
    #[error("cannot establish isolated preflight descriptor state: {0}")]
    DescriptorAudit(#[from] DescriptorHandoffError),
    #[error("cannot create preflight transport: {0}")]
    CreateTransport(#[source] io::Error),
    #[error("cannot start sealed preflight process: {0}")]
    Spawn(#[source] io::Error),
    #[error("cannot exchange preflight report: {0}")]
    Transport(#[source] io::Error),
    #[error("preflight report frame is invalid")]
    InvalidFrame,
    #[error("preflight report came from an unexpected process")]
    UnexpectedSender,
    #[error("preflight role or generation does not match its challenge")]
    BindingMismatch,
    #[error("daemon and client preflight reports do not describe one build pair")]
    PairMismatch,
    #[error("sealed preflight process exceeded its deadline")]
    Timeout,
    #[error("sealed preflight process failed with {0}")]
    ChildFailed(ExitStatus),
}

/// Handles the private launcher or child mode before Tokio, Wayland, daemon
/// sockets, diagnostics, or live state are initialized.
///
/// Returns `Ok(true)` only after completing one internal mode. Normal user
/// invocations return `Ok(false)`.
///
/// # Errors
///
/// Returns an error for a malformed challenge, role mismatch, transport
/// failure, or invalid compiled contract while handling an internal mode.
pub fn dispatch_internal_preflight(expected_role: PreflightRole) -> Result<bool, PreflightError> {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments.len() != 2 {
        return Ok(false);
    }
    if arguments[1] == LAUNCHER_ARGUMENT {
        launch_sealed_target();
    }
    if arguments[1] == CHILD_ARGUMENT {
        run_child(expected_role)?;
        return Ok(true);
    }
    Ok(false)
}

/// Executes both roles from both sealed generations and returns authority only
/// after all four reports are authenticated and pair-consistent.
///
/// The caller must quiesce process-wide descriptor creation and closure for the
/// descriptor audit and process-spawn boundary.
///
/// # Errors
///
/// Returns an error unless all four sealed images execute, authenticate one
/// bounded report, exit successfully, and agree within each daemon/client pair.
pub fn preflight_sealed_snapshots(
    snapshots: &HandoffExecutableSnapshots,
) -> Result<SealedPreflightReports, PreflightError> {
    preflight_sealed_snapshots_with_timeout(snapshots, DEFAULT_PREFLIGHT_TIMEOUT)
}

/// Runs one actual binary entrypoint for the cross-package integration tests.
///
/// # Errors
///
/// Returns the same bounded execution and authentication failures as the
/// production four-snapshot boundary.
#[cfg(feature = "integration-test")]
#[doc(hidden)]
pub fn preflight_sealed_snapshot_for_integration_test(
    snapshot: &SealedExecutableSnapshot,
    role: PreflightRole,
    launcher: &Path,
) -> Result<(), PreflightError> {
    preflight_one_with_launcher(
        snapshot,
        role,
        PreflightGeneration::Forward,
        Duration::from_secs(10),
        launcher,
    )
    .map(|_| ())
}

fn preflight_sealed_snapshots_with_timeout(
    snapshots: &HandoffExecutableSnapshots,
    timeout: Duration,
) -> Result<SealedPreflightReports, PreflightError> {
    let forward = preflight_pair(snapshots.forward(), PreflightGeneration::Forward, timeout)?;
    let rollback = preflight_pair(snapshots.rollback(), PreflightGeneration::Rollback, timeout)?;
    Ok(SealedPreflightReports { forward, rollback })
}

fn preflight_pair(
    pair: &SealedExecutablePair,
    generation: PreflightGeneration,
    timeout: Duration,
) -> Result<ExecutablePairPreflight, PreflightError> {
    let daemon = preflight_one(pair.daemon(), PreflightRole::Daemon, generation, timeout)?;
    let client = preflight_one(pair.client(), PreflightRole::Client, generation, timeout)?;
    bind_pair_reports(pair, daemon, client)
}

fn bind_pair_reports(
    pair: &SealedExecutablePair,
    daemon: ChildReport,
    client: ChildReport,
) -> Result<ExecutablePairPreflight, PreflightError> {
    if daemon.role != PreflightRole::Daemon
        || client.role != PreflightRole::Client
        || daemon.generation != client.generation
        || daemon.pair_build_identity != client.pair_build_identity
        || daemon.pair_build_version != client.pair_build_version
        || daemon.capabilities != client.capabilities
    {
        return Err(PreflightError::PairMismatch);
    }
    Ok(ExecutablePairPreflight {
        pair_build_identity: daemon.pair_build_identity,
        pair_build_version: daemon.pair_build_version,
        capabilities: daemon.capabilities,
        snapshots: ExecutablePairBinding::from_snapshots(pair),
    })
}

fn preflight_one(
    snapshot: &SealedExecutableSnapshot,
    role: PreflightRole,
    generation: PreflightGeneration,
    timeout: Duration,
) -> Result<ChildReport, PreflightError> {
    preflight_one_with_launcher(
        snapshot,
        role,
        generation,
        timeout,
        Path::new("/proc/self/exe"),
    )
}

fn preflight_one_with_launcher(
    snapshot: &SealedExecutableSnapshot,
    role: PreflightRole,
    generation: PreflightGeneration,
    timeout: Duration,
    launcher: &Path,
) -> Result<ChildReport, PreflightError> {
    let _audit = PreparedDescriptorInheritance::prepare([])?;
    let (parent, child_endpoint) = socketpair(
        AddressFamily::Unix,
        SockType::SeqPacket,
        None,
        SockFlag::SOCK_CLOEXEC,
    )
    .map_err(errno_io)
    .map_err(PreflightError::CreateTransport)?;
    setsockopt(&parent, PassCred, &true)
        .map_err(errno_io)
        .map_err(PreflightError::CreateTransport)?;

    let target = rustix::io::dup(snapshot.descriptor())
        .map_err(rustix_errno_io)
        .map_err(PreflightError::CreateTransport)?;

    let mut command = Command::new(launcher);
    command.env_clear();
    #[cfg(not(test))]
    command.arg(LAUNCHER_ARGUMENT);
    #[cfg(test)]
    command
        .arg(TEST_EXEC_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(TEST_STAGE, "launcher")
        .env(TEST_ROLE, (role as u8).to_string());
    command
        .stdin(Stdio::from(child_endpoint))
        .stdout(Stdio::null())
        .stderr(Stdio::from(target));
    let mut child = command.spawn().map_err(PreflightError::Spawn)?;
    drop(command);
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let challenge = random_challenge()?;
        let challenge_frame = encode_challenge(challenge, role, generation);
        send_packet(parent.as_raw_fd(), &challenge_frame)?;

        wait_readable(&parent, deadline)?;
        let mut report_frame = [0_u8; REPORT_BYTES];
        let credentials = receive_report_packet(&parent, &mut report_frame)?;
        if credentials.pid()
            != i32::try_from(child.id()).map_err(|_| PreflightError::UnexpectedSender)?
            || credentials.uid() != rustix::process::geteuid().as_raw()
            || credentials.gid() != rustix::process::getegid().as_raw()
        {
            return Err(PreflightError::UnexpectedSender);
        }
        let report = decode_report(&report_frame)?;
        if report.challenge != challenge || report.role != role || report.generation != generation {
            return Err(PreflightError::BindingMismatch);
        }
        let status = wait_until(&mut child, deadline)?;
        if !status.success() {
            return Err(PreflightError::ChildFailed(status));
        }
        wait_readable(&parent, deadline)?;
        ensure_packet_eof(&parent)?;
        Ok(report)
    })();
    if result.is_err() {
        terminate_and_reap(&mut child);
    }
    result
}

fn launch_sealed_target() -> ! {
    let target = std::io::stderr();
    if let Ok(flags) = fcntl_getfd(&target) {
        let _ = fcntl_setfd(&target, flags | FdFlags::CLOEXEC);
    }
    let empty_path = CString::new("").expect("empty path has no NUL");
    #[cfg(not(test))]
    let arguments = vec![
        CString::new("splinterm-sealed-preflight").expect("static argument has no NUL"),
        CString::new(CHILD_ARGUMENT).expect("static argument has no NUL"),
    ];
    #[cfg(not(test))]
    let environment = Vec::<CString>::new();
    #[cfg(test)]
    let arguments = [
        "splinterm-sealed-preflight",
        TEST_EXEC_NAME,
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ]
    .into_iter()
    .map(|value| CString::new(value).expect("static argument has no NUL"))
    .collect::<Vec<_>>();
    #[cfg(test)]
    let environment = [
        format!("{TEST_STAGE}=child"),
        format!(
            "{TEST_ROLE}={}",
            std::env::var(TEST_ROLE).unwrap_or_default()
        ),
    ]
    .into_iter()
    .map(|value| CString::new(value).expect("test environment has no NUL"))
    .collect::<Vec<_>>();
    #[cfg(not(test))]
    if install_no_descendants_filter().is_err() {
        std::process::exit(125);
    }
    let _ = nix::unistd::execveat(
        target.as_fd(),
        &empty_path,
        &arguments,
        &environment,
        nix::fcntl::AtFlags::AT_EMPTY_PATH,
    );
    std::process::exit(126)
}

fn install_no_descendants_filter() -> Result<(), String> {
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    return Err("unsupported seccomp architecture".to_owned());

    #[cfg(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    ))]
    {
        let mut denied = vec![libc::SYS_clone, libc::SYS_clone3];
        #[cfg(target_arch = "x86_64")]
        denied.extend([libc::SYS_fork, libc::SYS_vfork]);
        let architecture =
            TargetArch::try_from(std::env::consts::ARCH).map_err(|error| error.to_string())?;
        let filter: BpfProgram = SeccompFilter::new(
            denied
                .into_iter()
                .map(|syscall| (syscall, Vec::new()))
                .collect(),
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            architecture,
        )
        .map_err(|error| error.to_string())?
        .try_into()
        .map_err(|error: seccompiler::BackendError| error.to_string())?;
        apply_filter(&filter).map_err(|error| error.to_string())
    }
}

fn run_child(expected_role: PreflightRole) -> Result<(), PreflightError> {
    let mut challenge_frame = [0_u8; CHALLENGE_BYTES];
    receive_exact_packet(std::io::stdin().as_raw_fd(), &mut challenge_frame)?;
    let (challenge, role, generation) = decode_challenge(&challenge_frame)?;
    if role != expected_role {
        return Err(PreflightError::BindingMismatch);
    }
    let contract = compiled_contract()?;
    let report = ChildReport {
        challenge,
        role,
        generation,
        pair_build_identity: contract.0,
        pair_build_version: contract.1,
        capabilities: contract.2,
    };
    send_packet(std::io::stdin().as_raw_fd(), &encode_report(report))?;
    Ok(())
}

fn compiled_contract() -> Result<(BuildIdentity, BuildVersion, HandoffCapabilities), PreflightError>
{
    compiled_contract_for_commit(env!("SPLINTERM_BUILD_COMMIT"))
}

fn compiled_contract_for_commit(
    build_commit: &str,
) -> Result<(BuildIdentity, BuildVersion, HandoffCapabilities), PreflightError> {
    if build_commit.len() != 40
        || !build_commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PreflightError::InvalidFrame);
    }
    let mut hasher = Sha256::new();
    hasher.update(b"splinterm-handoff-pair-v1\0");
    hasher.update(build_commit.as_bytes());
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(splinterm_protocol::PROTOCOL_VERSION.to_be_bytes());
    hasher.update(HANDOFF_PROTOCOL_VERSION.to_be_bytes());
    hasher.update(TERMINAL_CHECKPOINT_VERSION.to_be_bytes());
    hasher.update(DESCRIPTOR_MANIFEST_VERSION.to_be_bytes());
    let identity =
        BuildIdentity::new(hasher.finalize().into()).map_err(|_| PreflightError::InvalidFrame)?;
    let version = parse_build_version(env!("CARGO_PKG_VERSION"))?;
    let exact =
        |version| VersionRange::new(version, version).map_err(|_| PreflightError::InvalidFrame);
    Ok((
        identity,
        version,
        HandoffCapabilities {
            private_protocol: exact(splinterm_protocol::PROTOCOL_VERSION)?,
            handoff_protocol: exact(HANDOFF_PROTOCOL_VERSION)?,
            terminal_checkpoint: exact(TERMINAL_CHECKPOINT_VERSION)?,
            descriptor_manifest: exact(DESCRIPTOR_MANIFEST_VERSION)?,
        },
    ))
}

fn parse_build_version(value: &str) -> Result<BuildVersion, PreflightError> {
    let release = value.split_once('-').map_or(value, |(release, _)| release);
    let mut parts = release.split('.');
    let major = parts
        .next()
        .ok_or(PreflightError::InvalidFrame)?
        .parse()
        .map_err(|_| PreflightError::InvalidFrame)?;
    let minor = parts
        .next()
        .ok_or(PreflightError::InvalidFrame)?
        .parse()
        .map_err(|_| PreflightError::InvalidFrame)?;
    let patch = parts
        .next()
        .ok_or(PreflightError::InvalidFrame)?
        .parse()
        .map_err(|_| PreflightError::InvalidFrame)?;
    if parts.next().is_some() {
        return Err(PreflightError::InvalidFrame);
    }
    Ok(BuildVersion::new(major, minor, patch))
}

fn random_challenge() -> Result<[u8; 32], PreflightError> {
    let mut challenge = [0_u8; 32];
    getrandom(&mut challenge, GetRandomFlags::empty())
        .map_err(rustix_errno_io)
        .map_err(PreflightError::Transport)?;
    if challenge == [0; 32] {
        return Err(PreflightError::InvalidFrame);
    }
    Ok(challenge)
}

fn encode_challenge(
    challenge: [u8; 32],
    role: PreflightRole,
    generation: PreflightGeneration,
) -> [u8; CHALLENGE_BYTES] {
    let mut frame = [0_u8; CHALLENGE_BYTES];
    frame[..8].copy_from_slice(&WIRE_MAGIC);
    frame[8..10].copy_from_slice(&WIRE_VERSION.to_be_bytes());
    frame[10] = role as u8;
    frame[11] = generation as u8;
    frame[12..].copy_from_slice(&challenge);
    frame
}

fn decode_challenge(
    frame: &[u8; CHALLENGE_BYTES],
) -> Result<([u8; 32], PreflightRole, PreflightGeneration), PreflightError> {
    if frame[..8] != WIRE_MAGIC || u16::from_be_bytes([frame[8], frame[9]]) != WIRE_VERSION {
        return Err(PreflightError::InvalidFrame);
    }
    let role = PreflightRole::from_wire(frame[10])?;
    let generation = PreflightGeneration::from_wire(frame[11])?;
    let mut challenge = [0_u8; 32];
    challenge.copy_from_slice(&frame[12..]);
    if challenge == [0; 32] {
        return Err(PreflightError::InvalidFrame);
    }
    Ok((challenge, role, generation))
}

fn encode_report(report: ChildReport) -> [u8; REPORT_BYTES] {
    let mut frame = [0_u8; REPORT_BYTES];
    frame[..8].copy_from_slice(&WIRE_MAGIC);
    frame[8..10].copy_from_slice(&WIRE_VERSION.to_be_bytes());
    frame[10] = report.role as u8;
    frame[11] = report.generation as u8;
    frame[12..44].copy_from_slice(&report.challenge);
    frame[44..76].copy_from_slice(&report.pair_build_identity.as_bytes());
    put_u16(&mut frame, 76, report.pair_build_version.major);
    put_u16(&mut frame, 78, report.pair_build_version.minor);
    put_u16(&mut frame, 80, report.pair_build_version.patch);
    let ranges = [
        report.capabilities.private_protocol,
        report.capabilities.handoff_protocol,
        report.capabilities.terminal_checkpoint,
        report.capabilities.descriptor_manifest,
    ];
    for (index, range) in ranges.into_iter().enumerate() {
        put_u16(&mut frame, 82 + index * 4, range.minimum());
        put_u16(&mut frame, 84 + index * 4, range.maximum());
    }
    frame
}

fn decode_report(frame: &[u8; REPORT_BYTES]) -> Result<ChildReport, PreflightError> {
    if frame[..8] != WIRE_MAGIC || u16::from_be_bytes([frame[8], frame[9]]) != WIRE_VERSION {
        return Err(PreflightError::InvalidFrame);
    }
    let mut challenge = [0_u8; 32];
    challenge.copy_from_slice(&frame[12..44]);
    let mut identity = [0_u8; 32];
    identity.copy_from_slice(&frame[44..76]);
    let range = |offset| {
        VersionRange::new(get_u16(frame, offset), get_u16(frame, offset + 2))
            .map_err(|_| PreflightError::InvalidFrame)
    };
    Ok(ChildReport {
        challenge,
        role: PreflightRole::from_wire(frame[10])?,
        generation: PreflightGeneration::from_wire(frame[11])?,
        pair_build_identity: BuildIdentity::new(identity)
            .map_err(|_| PreflightError::InvalidFrame)?,
        pair_build_version: BuildVersion::new(
            get_u16(frame, 76),
            get_u16(frame, 78),
            get_u16(frame, 80),
        ),
        capabilities: HandoffCapabilities {
            private_protocol: range(82)?,
            handoff_protocol: range(86)?,
            terminal_checkpoint: range(90)?,
            descriptor_manifest: range(94)?,
        },
    })
}

fn put_u16(frame: &mut [u8], offset: usize, value: u16) {
    frame[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn get_u16(frame: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([frame[offset], frame[offset + 1]])
}

fn send_packet(fd: i32, bytes: &[u8]) -> Result<(), PreflightError> {
    let written = sendmsg::<()>(fd, &[IoSlice::new(bytes)], &[], MsgFlags::empty(), None)
        .map_err(errno_io)
        .map_err(PreflightError::Transport)?;
    if written != bytes.len() {
        return Err(PreflightError::InvalidFrame);
    }
    Ok(())
}

fn receive_exact_packet(fd: i32, bytes: &mut [u8]) -> Result<(), PreflightError> {
    let expected_bytes = bytes.len();
    let mut iov = [IoSliceMut::new(bytes)];
    let message = recvmsg::<()>(fd, &mut iov, None, MsgFlags::MSG_CMSG_CLOEXEC)
        .map_err(errno_io)
        .map_err(PreflightError::Transport)?;
    if message.bytes != expected_bytes
        || message
            .flags
            .intersects(MsgFlags::MSG_TRUNC | MsgFlags::MSG_CTRUNC)
    {
        return Err(PreflightError::InvalidFrame);
    }
    Ok(())
}

fn receive_report_packet(
    parent: &OwnedFd,
    bytes: &mut [u8; REPORT_BYTES],
) -> Result<UnixCredentials, PreflightError> {
    let mut iov = [IoSliceMut::new(bytes)];
    let mut cmsgspace = nix::cmsg_space!(UnixCredentials);
    let message = recvmsg::<()>(
        parent.as_raw_fd(),
        &mut iov,
        Some(&mut cmsgspace),
        MsgFlags::MSG_CMSG_CLOEXEC,
    )
    .map_err(errno_io)
    .map_err(PreflightError::Transport)?;
    if message.bytes != REPORT_BYTES
        || message
            .flags
            .intersects(MsgFlags::MSG_TRUNC | MsgFlags::MSG_CTRUNC)
    {
        return Err(PreflightError::InvalidFrame);
    }
    let mut credentials = None;
    for message in message
        .cmsgs()
        .map_err(errno_io)
        .map_err(PreflightError::Transport)?
    {
        if let ControlMessageOwned::ScmCredentials(observed) = message
            && credentials.replace(observed).is_some()
        {
            return Err(PreflightError::UnexpectedSender);
        }
    }
    credentials.ok_or(PreflightError::UnexpectedSender)
}

fn ensure_packet_eof(parent: &OwnedFd) -> Result<(), PreflightError> {
    let mut byte = [0_u8; 1];
    let mut iov = [IoSliceMut::new(&mut byte)];
    let message = recvmsg::<()>(
        parent.as_raw_fd(),
        &mut iov,
        None,
        MsgFlags::MSG_CMSG_CLOEXEC,
    )
    .map_err(errno_io)
    .map_err(PreflightError::Transport)?;
    if message.bytes != 0 {
        return Err(PreflightError::InvalidFrame);
    }
    Ok(())
}

fn wait_readable(parent: &OwnedFd, deadline: Instant) -> Result<(), PreflightError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(PreflightError::Timeout);
    }
    let timeout = PollTimeout::try_from(remaining).unwrap_or(PollTimeout::MAX);
    let mut descriptors = [PollFd::new(
        parent.as_fd(),
        PollFlags::POLLIN | PollFlags::POLLHUP,
    )];
    let ready = poll(&mut descriptors, timeout)
        .map_err(errno_io)
        .map_err(PreflightError::Transport)?;
    if ready == 0 {
        return Err(PreflightError::Timeout);
    }
    Ok(())
}

fn wait_until(child: &mut Child, deadline: Instant) -> Result<ExitStatus, PreflightError> {
    loop {
        if let Some(status) = child.try_wait().map_err(PreflightError::Transport)? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            terminate_and_reap(child);
            return Err(PreflightError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn terminate_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn errno_io(error: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(error as i32)
}

fn rustix_errno_io(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::executable_snapshot::{
        ExecutableSnapshotPolicy, ExecutableSourcePair, HandoffExecutableSnapshots,
        RetainedRollbackExecutables,
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        directory: PathBuf,
        snapshots: HandoffExecutableSnapshots,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "splinterd-sealed-preflight-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let executable = std::env::current_exe().unwrap();
            let forward = pair(&directory, "forward", &executable);
            let rollback = pair(&directory, "rollback", &executable);
            let policy = ExecutableSnapshotPolicy {
                expected_owner_uid: rustix::process::geteuid().as_raw(),
            };
            let rollback =
                RetainedRollbackExecutables::capture_declared_for_test(&rollback, policy).unwrap();
            let snapshots =
                HandoffExecutableSnapshots::materialize(&forward, rollback, policy).unwrap();
            Self {
                directory,
                snapshots,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn pair(root: &Path, generation: &str, executable: &Path) -> ExecutableSourcePair {
        let directory = root.join(generation);
        fs::create_dir(&directory).unwrap();
        let daemon = directory.join("splinterd");
        let client = directory.join("splinterm");
        for path in [&daemon, &client] {
            fs::copy(executable, path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        ExecutableSourcePair::new(daemon, client).unwrap()
    }

    fn range(value: u16) -> VersionRange {
        VersionRange::new(value, value).unwrap()
    }

    fn report() -> ChildReport {
        ChildReport {
            challenge: [7; 32],
            role: PreflightRole::Daemon,
            generation: PreflightGeneration::Forward,
            pair_build_identity: BuildIdentity::new([9; 32]).unwrap(),
            pair_build_version: BuildVersion::new(0, 2, 3),
            capabilities: HandoffCapabilities {
                private_protocol: range(35),
                handoff_protocol: range(1),
                terminal_checkpoint: range(2),
                descriptor_manifest: range(3),
            },
        }
    }

    fn clean_test_generation() {
        let status = Command::new("python3")
            .arg("-c")
            .arg(
                "import subprocess, sys; raise SystemExit(subprocess.run(sys.argv[1:], close_fds=True).returncode)",
            )
            .arg(std::env::current_exe().unwrap())
            .arg(TEST_EXEC_NAME)
            .arg("--exact")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(TEST_CLEAN_STAGE, "clean")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn sealed_exec_authenticates_all_four_snapshots() {
        if std::env::var_os(TEST_CLEAN_STAGE).is_none() && std::env::var_os(TEST_STAGE).is_none() {
            clean_test_generation();
            return;
        }
        match std::env::var(TEST_STAGE).as_deref() {
            Ok("launcher") => launch_sealed_target(),
            Ok("child") => {
                let role = std::env::var(TEST_ROLE).unwrap().parse::<u8>().unwrap();
                run_child(PreflightRole::from_wire(role).unwrap()).unwrap();
                std::process::exit(0);
            }
            _ => {}
        }

        let fixture = Fixture::new();
        let reports =
            preflight_sealed_snapshots_with_timeout(&fixture.snapshots, Duration::from_secs(10))
                .unwrap();
        assert_eq!(
            reports.forward.snapshots,
            ExecutablePairBinding::from_snapshots(fixture.snapshots.forward())
        );
        assert_eq!(
            reports.rollback.snapshots,
            ExecutablePairBinding::from_snapshots(fixture.snapshots.rollback())
        );
        assert_eq!(
            reports.forward.pair_build_identity,
            reports.rollback.pair_build_identity
        );
        assert_eq!(
            reports.forward.pair_build_version,
            reports.rollback.pair_build_version
        );
        assert_eq!(reports.forward.capabilities, reports.rollback.capabilities);
    }

    #[test]
    fn sealed_launcher_denies_descendant_creation() {
        if std::env::var(TEST_STAGE).as_deref() == Ok("seccomp-probe") {
            install_no_descendants_filter().unwrap();
            let error = Command::new("/bin/true").status().unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EPERM));
            return;
        }
        let status = Command::new("python3")
            .arg("-c")
            .arg(
                "import subprocess, sys; raise SystemExit(subprocess.run(sys.argv[1:], close_fds=True).returncode)",
            )
            .arg(std::env::current_exe().unwrap())
            .arg(TEST_SECCOMP_NAME)
            .arg("--exact")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(TEST_STAGE, "seccomp-probe")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn challenge_frame_binds_nonzero_nonce_role_and_generation() {
        let frame = encode_challenge(
            [4; 32],
            PreflightRole::Client,
            PreflightGeneration::Rollback,
        );
        assert_eq!(
            decode_challenge(&frame).unwrap(),
            (
                [4; 32],
                PreflightRole::Client,
                PreflightGeneration::Rollback
            )
        );
        for offset in [0, 9, 10, 11] {
            let mut invalid = frame;
            invalid[offset] = 0;
            assert!(decode_challenge(&invalid).is_err());
        }
        let mut zero_nonce = frame;
        zero_nonce[12..].fill(0);
        assert!(decode_challenge(&zero_nonce).is_err());
    }

    #[test]
    fn report_frame_round_trips_and_rejects_invalid_structure() {
        let baseline = encode_report(report());
        assert_eq!(decode_report(&baseline).unwrap(), report());
        for offset in [0, 9, 10, 11, 83, 87, 91, 95] {
            let mut invalid = baseline;
            invalid[offset] = 0;
            assert!(decode_report(&invalid).is_err(), "offset {offset}");
        }
        let mut zero_identity = baseline;
        zero_identity[44..76].fill(0);
        assert!(decode_report(&zero_identity).is_err());
    }

    #[test]
    fn packet_transport_binds_kernel_credentials_and_exact_packet_size() {
        let (receiver, sender) = socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::SOCK_CLOEXEC,
        )
        .unwrap();
        setsockopt(&receiver, PassCred, &true).unwrap();
        send_packet(sender.as_raw_fd(), &encode_report(report())).unwrap();
        let mut frame = [0_u8; REPORT_BYTES];
        let credentials = receive_report_packet(&receiver, &mut frame).unwrap();
        assert_eq!(
            credentials.pid(),
            i32::try_from(std::process::id()).unwrap()
        );
        assert_eq!(credentials.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(decode_report(&frame).unwrap(), report());
        drop(sender);
        ensure_packet_eof(&receiver).unwrap();

        let (receiver, sender) = socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::SOCK_CLOEXEC,
        )
        .unwrap();
        send_packet(sender.as_raw_fd(), b"trailing").unwrap();
        assert!(matches!(
            ensure_packet_eof(&receiver),
            Err(PreflightError::InvalidFrame)
        ));

        for size in [REPORT_BYTES - 1, REPORT_BYTES + 1] {
            let (receiver, sender) = socketpair(
                AddressFamily::Unix,
                SockType::SeqPacket,
                None,
                SockFlag::SOCK_CLOEXEC,
            )
            .unwrap();
            setsockopt(&receiver, PassCred, &true).unwrap();
            send_packet(sender.as_raw_fd(), &vec![1; size]).unwrap();
            let mut frame = [0_u8; REPORT_BYTES];
            assert!(matches!(
                receive_report_packet(&receiver, &mut frame),
                Err(PreflightError::InvalidFrame)
            ));
        }
    }

    #[test]
    fn pair_binding_rejects_mixed_daemon_and_client_contracts() {
        let fixture = Fixture::new();
        let daemon = report();
        let mut client = report();
        client.role = PreflightRole::Client;
        assert!(bind_pair_reports(fixture.snapshots.forward(), daemon, client).is_ok());

        client.pair_build_version.patch += 1;
        assert!(matches!(
            bind_pair_reports(fixture.snapshots.forward(), daemon, client),
            Err(PreflightError::PairMismatch)
        ));
        client = report();
        client.role = PreflightRole::Client;
        client.pair_build_identity = BuildIdentity::new([8; 32]).unwrap();
        assert!(matches!(
            bind_pair_reports(fixture.snapshots.forward(), daemon, client),
            Err(PreflightError::PairMismatch)
        ));
        client = report();
        client.role = PreflightRole::Client;
        client.capabilities.handoff_protocol = range(4);
        assert!(matches!(
            bind_pair_reports(fixture.snapshots.forward(), daemon, client),
            Err(PreflightError::PairMismatch)
        ));
        client = report();
        assert!(matches!(
            bind_pair_reports(fixture.snapshots.forward(), daemon, client),
            Err(PreflightError::PairMismatch)
        ));
        client.role = PreflightRole::Client;
        client.generation = PreflightGeneration::Rollback;
        assert!(matches!(
            bind_pair_reports(fixture.snapshots.forward(), daemon, client),
            Err(PreflightError::PairMismatch)
        ));
    }

    #[test]
    fn unreadable_report_channel_obeys_its_deadline() {
        let (receiver, _sender) = socketpair(
            AddressFamily::Unix,
            SockType::SeqPacket,
            None,
            SockFlag::SOCK_CLOEXEC,
        )
        .unwrap();
        let started = Instant::now();
        assert!(matches!(
            wait_readable(&receiver, started + Duration::from_millis(20)),
            Err(PreflightError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn child_deadline_kills_and_reaps_the_direct_process() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec /bin/sleep 10"])
            .spawn()
            .unwrap();
        let started = Instant::now();
        assert!(matches!(
            wait_until(&mut child, started + Duration::from_millis(20)),
            Err(PreflightError::Timeout)
        ));
        assert!(child.try_wait().unwrap().is_some());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn compiled_pair_contract_is_nonzero_exact_and_stable() {
        let first = compiled_contract().unwrap();
        let second = compiled_contract().unwrap();
        assert_eq!(first, second);
        assert_ne!(first.0.as_bytes(), [0; 32]);
        for range in [
            first.2.private_protocol,
            first.2.handoff_protocol,
            first.2.terminal_checkpoint,
            first.2.descriptor_manifest,
        ] {
            assert_eq!(range.minimum(), range.maximum());
            assert_ne!(range.minimum(), 0);
        }
    }

    #[test]
    fn compiled_pair_identity_is_bound_to_exact_build_commit() {
        let first = compiled_contract_for_commit(&"a".repeat(40)).unwrap();
        let second = compiled_contract_for_commit(&"b".repeat(40)).unwrap();
        assert_ne!(first.0, second.0);
        for invalid in ["", &"a".repeat(39), &"A".repeat(40), &"g".repeat(40)] {
            assert!(matches!(
                compiled_contract_for_commit(invalid),
                Err(PreflightError::InvalidFrame)
            ));
        }
    }
}
