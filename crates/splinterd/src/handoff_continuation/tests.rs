//! Continuation claims and ticket redemption (Plan 0061 M3.16), against real
//! sockets and processes. The Window a claim succeeds for is this test
//! process: the kernel attaches its credentials to what it sends, its pidfd
//! pins it, and its executable stands in for the pinned snapshot.

use std::{
    fs,
    io::IoSlice,
    mem::MaybeUninit,
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::{MetadataExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    net::{
        AddressFamily, RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags,
        SocketFlags, SocketType, UCred, recv, send, sendmsg, socketpair, sockopt,
    },
    process::{Pid, PidfdFlags, getegid, geteuid, pidfd_open},
};
use splinterm_protocol::handoff_continuation::{HANDOFF_TICKET_GRANT_BYTES, HandoffTicketClaim};

use super::*;
use crate::{
    handoff_manifest::ManifestWindow,
    handoff_window_carry::{assert_end_of_file, process_start_time},
};

const GENERATION: u64 = 2;

fn pidfd_of(pid: u32) -> OwnedFd {
    pidfd_open(
        Pid::from_raw(i32::try_from(pid).unwrap()).unwrap(),
        PidfdFlags::empty(),
    )
    .unwrap()
}

fn own_pid() -> u32 {
    std::process::id()
}

fn own_start_time() -> u64 {
    process_start_time(own_pid()).unwrap()
}

fn own_executable() -> (u64, u64) {
    process_executable(own_pid()).unwrap()
}

fn binding(client_snapshot: Option<(u64, u64)>) -> ContinuationBinding {
    ContinuationBinding {
        daemon_generation: GENERATION,
        client_snapshot,
        uid: geteuid().as_raw(),
    }
}

fn seqpacket_pair(passcred: bool) -> (OwnedFd, OwnedFd) {
    let (daemon, client) = socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap();
    sockopt::set_socket_passcred(&daemon, passcred).unwrap();
    (daemon, client)
}

fn placeholder(window: u8, connection: u64, incarnation: u64) -> ControllerPlaceholder {
    ControllerPlaceholder {
        splint_id: SplintId::new(),
        incarnation,
        window,
        connection,
    }
}

/// A held Window for the process `pid`.
fn window_for(
    index: u8,
    pid: u32,
    continuation: Option<OwnedFd>,
    ticket: Option<IssuedTicket>,
) -> HeldHandoffWindow {
    HeldHandoffWindow {
        record: ManifestWindow {
            index,
            pid,
            start_time: process_start_time(pid).unwrap(),
            client_device: 1,
            client_inode: 2,
            old_connections: vec![11],
        },
        process: pidfd_of(pid),
        continuation,
        ticket,
    }
}

fn issued(bytes: [u8; HANDOFF_RESUME_TICKET_BYTES], client_snapshot: (u64, u64)) -> IssuedTicket {
    IssuedTicket {
        bytes,
        daemon_generation: GENERATION,
        client_snapshot,
        spent: Vec::new(),
    }
}

fn hold_of(
    windows: Vec<HeldHandoffWindow>,
    placeholders: Vec<ControllerPlaceholder>,
    expires_at: Instant,
) -> HandoffWindowHold {
    let hold = HandoffWindowHold::default();
    *hold.held.lock().unwrap() = Some(HeldHandoffWindows {
        windows,
        placeholders,
        expires_at,
    });
    hold
}

fn later() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

/// Waits up to 5 s for one packet on `client` and returns it.
fn read_packet(client: &OwnedFd) -> Option<Vec<u8>> {
    let mut descriptors = [PollFd::new(client, PollFlags::IN)];
    let timeout = Timespec {
        tv_sec: 5,
        tv_nsec: 0,
    };
    if poll(&mut descriptors, Some(&timeout)).unwrap() == 0 {
        return None;
    }
    let mut buffer = [0_u8; 128];
    let (read, _) = recv(client, &mut buffer, RecvFlags::DONTWAIT).ok()?;
    (read > 0).then(|| buffer[..read].to_vec())
}

fn nothing_queued(client: &OwnedFd) -> bool {
    let mut buffer = [0_u8; 128];
    matches!(
        recv(client, &mut buffer, RecvFlags::DONTWAIT),
        Err(rustix::io::Errno::AGAIN)
    )
}

fn claim(generation: u64) -> [u8; 16] {
    HandoffTicketClaim {
        daemon_generation: generation,
    }
    .encode()
}

/// Receives on `daemon` what the Window sent and completes the claim.
fn claim_on(
    hold: &HandoffWindowHold,
    daemon: &OwnedFd,
    binding: &ContinuationBinding,
    now: Instant,
) -> Result<(), ClaimRefusal> {
    let received =
        receive_claim(daemon.as_fd()).and_then(|received| received.ok_or(ClaimRefusal::Receive));
    hold.complete_claim(0, daemon.as_fd(), received, binding, now)
}

/// A child process that is not this one: `sleep`, killed on drop.
struct OtherProcess(Child);

impl OtherProcess {
    fn spawn(program: &Path) -> Self {
        Self(Command::new(program).arg("60").spawn().unwrap())
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for OtherProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn which(program: &str) -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(program))
                .find(|candidate| candidate.is_file())
        })
        .unwrap_or_else(|| panic!("{program} is not on PATH"))
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "splinterd-continuation-{name}-{}-{unique}",
            own_pid()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Copies `source` to a new inode at `path`, renamed over whatever was
/// there, the way a package upgrade replaces a file.
fn install_copy(source: &Path, path: &Path) {
    let staging = path.with_extension("staging");
    fs::copy(source, &staging).unwrap();
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&staging, path).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_valid_claim_gets_one_ticket_then_end_of_file_and_the_ticket_restores() {
    let (daemon, client) = seqpacket_pair(true);
    let target = placeholder(0, 11, 4);
    let hold = hold_of(
        vec![window_for(0, own_pid(), Some(daemon), None)],
        vec![target, placeholder(0, 12, 4)],
        later(),
    );
    assert_eq!(
        serve_continuations(&hold, binding(Some(own_executable()))),
        1
    );
    // The hold no longer owns the end it serves.
    assert!(
        hold.held.lock().unwrap().as_ref().unwrap().windows[0]
            .continuation()
            .is_none()
    );
    assert_eq!(
        send(&client, &claim(GENERATION), SendFlags::empty()).unwrap(),
        16
    );
    let client = Arc::new(client);
    let packet = tokio::task::spawn_blocking({
        let client = Arc::clone(&client);
        move || read_packet(&client)
    })
    .await
    .unwrap()
    .expect("a grant");
    assert_eq!(packet.len(), HANDOFF_TICKET_GRANT_BYTES);
    let grant = HandoffTicketGrant::decode(&packet).unwrap();
    assert_eq!(grant.daemon_generation, GENERATION);
    // Exactly one packet, then the daemon end closes.
    tokio::task::spawn_blocking({
        let client = Arc::clone(&client);
        move || assert_end_of_file(&client)
    })
    .await
    .unwrap();
    assert!(hold.held.lock().unwrap().as_ref().unwrap().windows[0].ticketed());

    let restored = hold
        .redeem(
            &presentation(grant.ticket.as_bytes(), target, 4),
            Instant::now(),
            |placeholder| Some(placeholder.splint_id),
        )
        .unwrap();
    assert_eq!(restored, Redemption::Restored(target.splint_id));
}

/// Every refusal answers nothing, drops the Window and its placeholders,
/// and leaves the Window's end to read end of file once the daemon end
/// closes.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one table of every continuation misuse case"
)]
fn every_continuation_misuse_is_refused_and_drops_the_window() {
    type Sender = fn(&OwnedFd);
    // A case: its name, the refusal, what the Window sends, the binding,
    // whether the daemon end receives credentials, and another Window PID.
    type Case = (
        &'static str,
        ClaimRefusal,
        Sender,
        ContinuationBinding,
        bool,
        Option<u32>,
    );
    let other = OtherProcess::spawn(&which("sleep"));
    let own = own_executable();
    let send_claim: Sender = |client| {
        send(client, &claim(GENERATION), SendFlags::empty()).unwrap();
    };
    let cases: Vec<Case> = vec![
        (
            "another generation",
            ClaimRefusal::Generation,
            |client| {
                send(client, &claim(GENERATION + 1), SendFlags::empty()).unwrap();
            },
            binding(Some(own)),
            true,
            None,
        ),
        (
            "not a claim",
            ClaimRefusal::Malformed,
            |client| {
                send(client, &[7_u8; 16], SendFlags::empty()).unwrap();
            },
            binding(Some(own)),
            true,
            None,
        ),
        (
            "an excess byte",
            ClaimRefusal::Malformed,
            |client| {
                let mut long = claim(GENERATION).to_vec();
                long.push(0);
                send(client, &long, SendFlags::empty()).unwrap();
            },
            binding(Some(own)),
            true,
            None,
        ),
        (
            "an oversized packet",
            ClaimRefusal::Truncated,
            |client| {
                send(client, &[0_u8; 4096], SendFlags::empty()).unwrap();
            },
            binding(Some(own)),
            true,
            None,
        ),
        (
            "descriptors beside the claim",
            ClaimRefusal::Descriptors,
            |client| {
                let null = fs::File::open("/dev/null").unwrap();
                let rights = [null.as_fd()];
                let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
                let mut control = SendAncillaryBuffer::new(&mut space);
                assert!(control.push(SendAncillaryMessage::ScmRights(&rights)));
                let packet = claim(GENERATION);
                sendmsg(
                    client,
                    &[IoSlice::new(&packet)],
                    &mut control,
                    SendFlags::empty(),
                )
                .unwrap();
            },
            binding(Some(own)),
            true,
            None,
        ),
        (
            "no sender credentials",
            ClaimRefusal::Credentials,
            send_claim,
            binding(Some(own)),
            false,
            None,
        ),
        (
            "another process's Window",
            ClaimRefusal::Process,
            send_claim,
            binding(Some(own)),
            true,
            Some(other.pid()),
        ),
        (
            "another executable",
            ClaimRefusal::Executable,
            send_claim,
            binding(Some((own.0, own.1 ^ 1))),
            true,
            None,
        ),
        (
            "a rollback daemon",
            ClaimRefusal::NoSnapshot,
            send_claim,
            binding(None),
            true,
            None,
        ),
        (
            "another user",
            ClaimRefusal::User,
            send_claim,
            ContinuationBinding {
                uid: geteuid().as_raw().wrapping_add(1),
                ..binding(Some(own))
            },
            true,
            None,
        ),
        (
            "end of file",
            ClaimRefusal::EndOfFile,
            |_| {},
            binding(Some(own)),
            true,
            None,
        ),
    ];
    for (name, expected, send_it, binding, passcred, window_pid) in cases {
        let (daemon, client) = seqpacket_pair(passcred);
        let hold = hold_of(
            vec![window_for(
                0,
                window_pid.unwrap_or_else(own_pid),
                None,
                None,
            )],
            vec![placeholder(0, 11, 4)],
            later(),
        );
        send_it(&client);
        let client = if expected == ClaimRefusal::EndOfFile {
            drop(client);
            None
        } else {
            Some(client)
        };
        assert_eq!(
            claim_on(&hold, &daemon, &binding, Instant::now()),
            Err(expected),
            "{name}"
        );
        assert!(!hold.is_held(), "{name}: the Window is dropped");
        assert!(hold.placeholders().is_empty(), "{name}");
        if let Some(client) = client {
            assert!(nothing_queued(&client), "{name}: nothing is answered");
            drop(daemon);
            assert_end_of_file(&client);
        }
    }

    // Nothing held for the Window, or the hold's deadline passed.
    for (name, expected, placeholders, now) in [
        (
            "nothing held",
            ClaimRefusal::NothingHeld,
            vec![placeholder(1, 13, 4)],
            Instant::now(),
        ),
        (
            "after the deadline",
            ClaimRefusal::Released,
            vec![placeholder(0, 11, 4)],
            Instant::now() + Duration::from_secs(60),
        ),
    ] {
        let (daemon, client) = seqpacket_pair(true);
        let hold = hold_of(
            vec![window_for(0, own_pid(), None, None)],
            placeholders,
            later(),
        );
        send_claim(&client);
        assert_eq!(
            claim_on(&hold, &daemon, &binding(Some(own)), now),
            Err(expected),
            "{name}"
        );
        assert!(!hold.is_held(), "{name}");
        assert!(nothing_queued(&client), "{name}");
    }

    // After the hold was released, nothing remains to claim.
    let (daemon, client) = seqpacket_pair(true);
    let hold = hold_of(
        vec![window_for(0, own_pid(), None, None)],
        vec![placeholder(0, 11, 4)],
        later(),
    );
    assert!(hold.release());
    send_claim(&client);
    assert_eq!(
        claim_on(&hold, &daemon, &binding(Some(own)), Instant::now()),
        Err(ClaimRefusal::Released)
    );
    assert!(nothing_queued(&client));
}

/// The kernel itself refuses a sender's attempt to name another PID in its
/// credentials (`scm_check_creds`), unless the sender has `CAP_SYS_ADMIN`;
/// credentials it supplies for itself arrive as sent.
#[test]
fn forged_sender_credentials_never_reach_the_daemon() {
    let other = OtherProcess::spawn(&which("sleep"));
    let (daemon, client) = seqpacket_pair(true);
    let send_as = |pid: u32| {
        let credentials = UCred {
            pid: Pid::from_raw(i32::try_from(pid).unwrap()).unwrap(),
            uid: geteuid(),
            gid: getegid(),
        };
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmCredentials(1))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        assert!(control.push(SendAncillaryMessage::ScmCredentials(credentials)));
        let packet = claim(GENERATION);
        sendmsg(
            &client,
            &[IoSlice::new(&packet)],
            &mut control,
            SendFlags::empty(),
        )
    };
    if geteuid().is_root() {
        eprintln!("running as root: the kernel lets this sender forge credentials; skipped");
    } else {
        assert_eq!(send_as(other.pid()), Err(rustix::io::Errno::PERM));
        assert!(receive_claim(daemon.as_fd()).unwrap().is_none());
    }
    assert_eq!(send_as(own_pid()).unwrap(), 16);
    let received = receive_claim(daemon.as_fd()).unwrap().unwrap();
    assert_eq!(received.pid, own_pid());
    assert_eq!(received.uid, geteuid().as_raw());
}

#[test]
fn only_the_live_pinned_process_running_the_snapshot_is_the_window() {
    let scratch = Scratch::new("stale");
    // Named `sleep`, so a multi-call coreutils still dispatches on it.
    let image = scratch.0.join("sleep");
    let sleep = which("sleep");
    install_copy(&sleep, &image);
    let mut child = OtherProcess::spawn(&image);
    let pid = child.pid();
    let running = process_executable(pid).unwrap();
    let window = window_for(0, pid, None, None);
    assert_eq!(verify_window_process(&window, pid, running), Ok(()));
    assert_eq!(
        verify_window_process(&window, own_pid(), running),
        Err(ProcessMismatch::Process)
    );
    // An upgrade replaces the file: the process keeps its stale inode.
    install_copy(&sleep, &image);
    let installed = fs::metadata(&image).unwrap();
    assert_ne!(running, (installed.dev(), installed.ino()));
    assert_eq!(
        verify_window_process(&window, pid, (installed.dev(), installed.ino())),
        Err(ProcessMismatch::Executable)
    );
    // The PID with another start time is another process.
    let mut reused = window_for(0, pid, None, None);
    reused.record.start_time += 1;
    assert_eq!(
        verify_window_process(&reused, pid, running),
        Err(ProcessMismatch::Identity)
    );
    child.0.kill().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !crate::handoff_window_carry::pidfd_exited(window.process.as_fd()).unwrap() {
        assert!(Instant::now() < deadline, "the killed child never exited");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        verify_window_process(&window, pid, running),
        Err(ProcessMismatch::Exited)
    );
}

#[test]
fn a_second_or_concurrent_claim_never_issues_a_second_ticket() {
    let own = own_executable();
    let received = ReceivedClaim {
        claim: HandoffTicketClaim {
            daemon_generation: GENERATION,
        },
        pid: own_pid(),
        uid: geteuid().as_raw(),
    };
    let (daemon, client) = seqpacket_pair(true);
    let daemon = Arc::new(daemon);
    let hold = hold_of(
        vec![window_for(0, own_pid(), None, None)],
        vec![placeholder(0, 11, 4)],
        later(),
    );
    let barrier = Arc::new(Barrier::new(8));
    // Every thread is spawned before any is joined: they meet at the barrier.
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (hold, daemon, barrier) = (hold.clone(), Arc::clone(&daemon), Arc::clone(&barrier));
            thread::spawn(move || {
                barrier.wait();
                hold.complete_claim(
                    0,
                    daemon.as_fd(),
                    Ok(received),
                    &binding(Some(own)),
                    Instant::now(),
                )
            })
        })
        .collect();
    let outcomes = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, Ok(()) | Err(ClaimRefusal::AlreadyClaimed)))
    );
    // The Window keeps its one ticket, and exactly one grant went out.
    assert!(hold.held.lock().unwrap().as_ref().unwrap().windows[0].ticketed());
    assert_eq!(
        read_packet(&client).map(|packet| packet.len()),
        Some(HANDOFF_TICKET_GRANT_BYTES)
    );
    assert!(nothing_queued(&client));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_release_ends_a_waiting_continuation_server() {
    let (daemon, client) = seqpacket_pair(true);
    let hold = hold_of(
        vec![window_for(0, own_pid(), Some(daemon), None)],
        vec![placeholder(0, 11, 4)],
        later(),
    );
    assert_eq!(
        serve_continuations(&hold, binding(Some(own_executable()))),
        1
    );
    tokio::task::yield_now().await;
    assert!(hold.release());
    let client = Arc::new(client);
    tokio::task::spawn_blocking({
        let client = Arc::clone(&client);
        move || assert_end_of_file(&client)
    })
    .await
    .unwrap();
    // A claim sent now reaches nobody.
    assert!(send(&*client, &claim(GENERATION), SendFlags::NOSIGNAL).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_that_closes_its_end_without_claiming_loses_its_placeholders() {
    let (daemon, client) = seqpacket_pair(true);
    let hold = hold_of(
        vec![window_for(0, own_pid(), Some(daemon), None)],
        vec![placeholder(0, 11, 4)],
        later(),
    );
    assert_eq!(
        serve_continuations(&hold, binding(Some(own_executable()))),
        1
    );
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), async {
        while hold.is_held() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the Window is dropped");
    assert!(hold.placeholders().is_empty());
}

fn presentation(
    ticket: &[u8; HANDOFF_RESUME_TICKET_BYTES],
    target: ControllerPlaceholder,
    live_incarnation: u64,
) -> TicketPresentation {
    TicketPresentation {
        ticket: *ticket,
        claimant_pid: own_pid(),
        claimant_start_time: Some(own_start_time()),
        splint_id: target.splint_id,
        incarnation: target.incarnation,
        live_incarnation: Some(live_incarnation),
        daemon_generation: GENERATION,
    }
}

fn ticketed_own_hold(
    ticket: [u8; HANDOFF_RESUME_TICKET_BYTES],
    placeholders: Vec<ControllerPlaceholder>,
) -> HandoffWindowHold {
    hold_of(
        vec![window_for(
            0,
            own_pid(),
            None,
            Some(issued(ticket, own_executable())),
        )],
        placeholders,
        later(),
    )
}

#[test]
fn a_ticket_restores_exactly_the_named_live_unheld_placeholders_once_each() {
    let ticket = [0x3c; HANDOFF_RESUME_TICKET_BYTES];
    let (first, second, third, fourth) = (
        placeholder(0, 11, 4),
        placeholder(0, 12, 4),
        placeholder(0, 13, 4),
        placeholder(0, 14, 4),
    );
    let foreign = placeholder(1, 21, 4);
    let hold = ticketed_own_hold(ticket, vec![first, second, third, fourth, foreign]);
    let restores = AtomicUsize::new(0);
    let restore = |placeholder: &ControllerPlaceholder| {
        restores.fetch_add(1, Ordering::SeqCst);
        Some((placeholder.splint_id, placeholder.incarnation))
    };
    let now = Instant::now();
    assert_eq!(
        hold.redeem(&presentation(&ticket, first, 4), now, restore),
        Ok(Redemption::Restored((first.splint_id, 4)))
    );
    // A replay for the same Splint restores nothing.
    assert_eq!(
        hold.redeem(&presentation(&ticket, first, 4), now, restore),
        Err(TicketRefusal::Replayed)
    );
    // A Splint this Window held nothing for, or another Window's, is skipped.
    assert_eq!(
        hold.redeem(
            &presentation(&ticket, placeholder(0, 11, 4), 4),
            now,
            restore
        ),
        Ok(Redemption::Skipped(ClaimSkip::NotHeld))
    );
    assert_eq!(
        hold.redeem(&presentation(&ticket, foreign, 4), now, restore),
        Ok(Redemption::Skipped(ClaimSkip::NotHeld))
    );
    // The live incarnation changed since the fence: skipped and spent.
    assert_eq!(
        hold.redeem(&presentation(&ticket, second, 5), now, restore),
        Ok(Redemption::Skipped(ClaimSkip::IncarnationChanged))
    );
    // The claimant names another incarnation than the recorded one.
    let mut misnamed = presentation(&ticket, third, 4);
    misnamed.incarnation = 5;
    misnamed.live_incarnation = Some(5);
    assert_eq!(
        hold.redeem(&misnamed, now, restore),
        Ok(Redemption::Skipped(ClaimSkip::IncarnationChanged))
    );
    // A Splint that has a controller now, or a busy connection: `restore`
    // refuses and nothing is forced.
    assert_eq!(
        hold.redeem(&presentation(&ticket, fourth, 4), now, |_| None::<()>),
        Ok(Redemption::Skipped(ClaimSkip::Unavailable))
    );
    assert_eq!(restores.load(Ordering::SeqCst), 1);
    // The ticket ended with the Window's last placeholder.
    assert!(!hold.is_held());
    assert_eq!(
        hold.redeem(&presentation(&ticket, fourth, 4), now, restore),
        Err(TicketRefusal::Unknown)
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one table of every binding mismatch, then a foreign ticket"
)]
fn a_transferred_misbound_or_foreign_ticket_ends_and_restores_nothing() {
    // A case: its name, the refusal, how the presentation changes, and
    // the snapshot identity the ticket is bound to.
    type Case = (
        &'static str,
        TicketRefusal,
        fn(&mut TicketPresentation),
        (u64, u64),
    );
    let ticket = [0x6e; HANDOFF_RESUME_TICKET_BYTES];
    let other = OtherProcess::spawn(&which("sleep"));
    let own = own_executable();
    let target = placeholder(0, 11, 4);
    let cases: Vec<Case> = vec![
        (
            "another process",
            TicketRefusal::Binding(ProcessMismatch::Process),
            |presentation| presentation.claimant_pid = 1,
            own,
        ),
        (
            "a reused PID",
            TicketRefusal::Binding(ProcessMismatch::Process),
            |presentation| {
                presentation.claimant_start_time = presentation.claimant_start_time.map(|t| t + 1);
            },
            own,
        ),
        (
            "an unrecorded start time",
            TicketRefusal::Binding(ProcessMismatch::Process),
            |presentation| presentation.claimant_start_time = None,
            own,
        ),
        (
            "another generation",
            TicketRefusal::Generation,
            |presentation| presentation.daemon_generation = GENERATION + 1,
            own,
        ),
        (
            "not the bound snapshot",
            TicketRefusal::Binding(ProcessMismatch::Executable),
            |_| {},
            (own.0, own.1 ^ 1),
        ),
    ];
    for (name, expected, change, snapshot) in cases {
        let hold = hold_of(
            vec![window_for(
                0,
                own_pid(),
                None,
                Some(issued(ticket, snapshot)),
            )],
            vec![target, placeholder(0, 12, 4)],
            later(),
        );
        let mut presented = presentation(&ticket, target, 4);
        change(&mut presented);
        assert_eq!(
            hold.redeem(&presented, Instant::now(), |_| Some(())),
            Err(expected),
            "{name}"
        );
        // The ticket ended with the Window's placeholders.
        assert!(!hold.is_held(), "{name}");
        assert_eq!(
            hold.redeem(&presentation(&ticket, target, 4), Instant::now(), |_| Some(
                ()
            )),
            Err(TicketRefusal::Unknown),
            "{name}"
        );
    }

    // An unknown ticket changes nothing; another Window's ticket presented
    // by this process ends only that Window's.
    let foreign = [0x7f; HANDOFF_RESUME_TICKET_BYTES];
    let foreign_held = placeholder(1, 21, 4);
    let hold = hold_of(
        vec![
            window_for(0, own_pid(), None, Some(issued(ticket, own))),
            window_for(1, other.pid(), None, Some(issued(foreign, own))),
        ],
        vec![target, foreign_held],
        later(),
    );
    assert_eq!(
        hold.redeem(
            &presentation(&[0x11; HANDOFF_RESUME_TICKET_BYTES], target, 4),
            Instant::now(),
            |_| Some(())
        ),
        Err(TicketRefusal::Unknown)
    );
    assert_eq!(hold.window_count(), 2);
    assert_eq!(
        hold.redeem(
            &presentation(&foreign, foreign_held, 4),
            Instant::now(),
            |_| Some(())
        ),
        Err(TicketRefusal::Binding(ProcessMismatch::Process))
    );
    assert_eq!(hold.window_count(), 1);
    assert_eq!(hold.placeholders(), [target]);
    assert_eq!(
        hold.redeem(&presentation(&ticket, target, 4), Instant::now(), |_| Some(
            ()
        )),
        Ok(Redemption::Restored(()))
    );
}

#[test]
fn a_claim_after_expiry_or_release_restores_nothing_and_releases_once() {
    let ticket = [0x42; HANDOFF_RESUME_TICKET_BYTES];
    let target = placeholder(0, 11, 4);
    let hold = ticketed_own_hold(ticket, vec![target]);
    let deadline = hold.held.lock().unwrap().as_ref().unwrap().expires_at;
    // Past the deadline, before the expiry task ran: the claim releases
    // the hold itself, and the expiry then finds nothing.
    assert_eq!(
        hold.redeem(&presentation(&ticket, target, 4), deadline, |_| Some(())),
        Err(TicketRefusal::Expired)
    );
    assert!(!hold.is_held());
    assert!(!hold.release());

    let hold = ticketed_own_hold(ticket, vec![target]);
    assert!(hold.release());
    assert_eq!(
        hold.redeem(&presentation(&ticket, target, 4), Instant::now(), |_| Some(
            ()
        )),
        Err(TicketRefusal::Unknown)
    );
}

#[test]
fn concurrent_redemptions_restore_a_placeholder_once() {
    let ticket = [0x5d; HANDOFF_RESUME_TICKET_BYTES];
    let target = placeholder(0, 11, 4);
    let hold = ticketed_own_hold(ticket, vec![target, placeholder(0, 12, 4)]);
    let restores = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(16));
    // Every thread is spawned before any is joined: they meet at the barrier.
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let (hold, restores, barrier) =
                (hold.clone(), Arc::clone(&restores), Arc::clone(&barrier));
            thread::spawn(move || {
                barrier.wait();
                hold.redeem(&presentation(&ticket, target, 4), Instant::now(), |_| {
                    restores.fetch_add(1, Ordering::SeqCst);
                    Some(())
                })
            })
        })
        .collect();
    let outcomes = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(restores.load(Ordering::SeqCst), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Ok(Redemption::Restored(())))
            .count(),
        1
    );
    assert!(
        outcomes.iter().all(|outcome| matches!(
            outcome,
            Ok(Redemption::Restored(())) | Err(TicketRefusal::Replayed)
        )),
        "{outcomes:?}"
    );
}

#[test]
fn a_claim_racing_the_expiry_either_restores_before_it_or_finds_nothing() {
    let ticket = [0x24; HANDOFF_RESUME_TICKET_BYTES];
    for _ in 0..200 {
        // An explicit release (the expiry task) against a timely claim.
        let target = placeholder(0, 11, 4);
        let hold = ticketed_own_hold(ticket, vec![target, placeholder(0, 12, 4)]);
        let barrier = Arc::new(Barrier::new(2));
        let releasing = thread::spawn({
            let (hold, barrier) = (hold.clone(), Arc::clone(&barrier));
            move || {
                barrier.wait();
                hold.release()
            }
        });
        barrier.wait();
        let claimed = hold.redeem(&presentation(&ticket, target, 4), Instant::now(), |_| {
            Some(())
        });
        // The Window kept another placeholder, so the release always found
        // the hold: it released once, whichever ran first.
        assert!(releasing.join().unwrap());
        assert!(
            matches!(
                claimed,
                Ok(Redemption::Restored(())) | Err(TicketRefusal::Unknown)
            ),
            "{claimed:?}"
        );
        assert!(!hold.is_held());

        // A claim arriving at the deadline against the expiry task: exactly
        // one of them releases the hold.
        let hold = ticketed_own_hold(ticket, vec![target]);
        let deadline = hold.held.lock().unwrap().as_ref().unwrap().expires_at;
        let barrier = Arc::new(Barrier::new(2));
        let releasing = thread::spawn({
            let (hold, barrier) = (hold.clone(), Arc::clone(&barrier));
            move || {
                barrier.wait();
                hold.release()
            }
        });
        barrier.wait();
        let late = hold.redeem(&presentation(&ticket, target, 4), deadline, |_| Some(()));
        let released = releasing.join().unwrap();
        assert!(
            matches!(late, Err(TicketRefusal::Expired | TicketRefusal::Unknown)),
            "{late:?}"
        );
        assert_ne!(late == Err(TicketRefusal::Expired), released);
        assert!(!hold.is_held());
    }
}

#[test]
fn tickets_are_compared_in_full_and_never_printed() {
    let ticket = [0x81; HANDOFF_RESUME_TICKET_BYTES];
    for index in 0..HANDOFF_RESUME_TICKET_BYTES {
        let mut other = ticket;
        other[index] ^= 1;
        assert!(!same_ticket(&ticket, &other));
    }
    assert!(same_ticket(&ticket, &ticket));
    let first = random_ticket().unwrap();
    assert_ne!(first, random_ticket().unwrap());
    let debug = format!(
        "{:?} {:?}",
        issued(ticket, (1, 2)),
        presentation(&ticket, placeholder(0, 1, 1), 1)
    );
    assert!(!debug.contains("129"), "{debug}");
}

/// The kernel reports PID 0 for a sender outside the daemon's PID
/// namespace; nix decodes it as a plain value, which is refused, as is any
/// negative PID, before the credentials are used for anything.
#[test]
fn credentials_naming_no_visible_pid_are_refused() {
    for pid in [0, -1, libc::pid_t::MIN] {
        assert_eq!(sender_pid(pid), Err(ClaimRefusal::Credentials), "{pid}");
    }
    assert_eq!(sender_pid(1), Ok(1));
    assert_eq!(
        sender_pid(libc::pid_t::try_from(own_pid()).unwrap()),
        Ok(own_pid())
    );
}
