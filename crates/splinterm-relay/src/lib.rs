#![forbid(unsafe_code)]

//! Byte-transparent, bounded stdio transport for one local Splinterm socket.

use std::{
    env,
    fs::File,
    net::Shutdown,
    os::{
        fd::{AsFd, OwnedFd},
        unix::{
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::UnixStream as StdUnixStream,
        },
    },
    path::{Component, Path, PathBuf},
    sync::mpsc,
    thread,
};

use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
};

const COPY_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

/// Resolves exactly one configured daemon socket.
///
/// # Errors
///
/// Returns an error when neither `SPLINTERM_SOCKET` nor `XDG_RUNTIME_DIR` is set.
pub fn socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SPLINTERM_SOCKET") {
        return Ok(path.into());
    }
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is unset; set SPLINTERM_SOCKET explicitly")?;
    Ok(runtime.join("splinterm/splinterd.sock"))
}

fn validate_socket_path(path: &Path) -> Result<SocketIdentity> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        bail!("daemon socket path must be absolute and normalized");
    }
    let parent = path
        .parent()
        .context("daemon socket path does not have a parent directory")?;
    let canonical_parent = parent
        .canonicalize()
        .context("cannot canonicalize daemon socket directory")?;
    if canonical_parent != parent {
        bail!("daemon socket directory must not contain symlinks");
    }
    let effective_uid = rustix::process::geteuid().as_raw();
    let parent_metadata = parent
        .symlink_metadata()
        .context("cannot inspect daemon socket directory")?;
    if !parent_metadata.file_type().is_dir()
        || parent_metadata.uid() != effective_uid
        || parent_metadata.permissions().mode() & 0o077 != 0
    {
        bail!("daemon socket directory must be owner-only and owned by the current user");
    }
    let metadata = path
        .symlink_metadata()
        .context("cannot inspect daemon socket endpoint")?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        bail!("daemon endpoint must be an owner-only Unix socket owned by the current user");
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[derive(Debug)]
struct ValidatedConnection {
    stream: UnixStream,
    peer_pidfd: OwnedFd,
}

fn peer_exited(pidfd: &OwnedFd) -> Result<bool> {
    let mut descriptor = [nix::poll::PollFd::new(
        pidfd.as_fd(),
        nix::poll::PollFlags::POLLIN,
    )];
    nix::poll::poll(&mut descriptor, nix::poll::PollTimeout::ZERO)
        .context("cannot inspect daemon peer lifetime")?;
    Ok(descriptor[0].any().unwrap_or(true))
}

async fn connect_validated_for(path: &Path, expected_peer: &Path) -> Result<ValidatedConnection> {
    let before = validate_socket_path(path)?;
    let stream = UnixStream::connect(path)
        .await
        .context("cannot connect to the validated daemon socket")?;
    let effective_uid = rustix::process::geteuid().as_raw();
    let peer = stream
        .peer_cred()
        .context("cannot inspect daemon socket peer credentials")?;
    if peer.uid() != effective_uid {
        bail!("daemon socket peer is not owned by the current user");
    }
    let peer_pidfd = nix::sys::socket::getsockopt(&stream, nix::sys::socket::sockopt::PeerPidfd)
        .context("cannot bind daemon socket peer to a pidfd")?;
    if peer_exited(&peer_pidfd)? {
        bail!("daemon socket peer exited during validation");
    }
    let peer_pid = peer
        .pid()
        .context("daemon socket peer PID is unavailable")?;
    let peer_executable = File::open(format!("/proc/{peer_pid}/exe"))
        .context("cannot open daemon socket peer executable")?;
    let expected_executable = File::open(expected_peer)
        .context("cannot open the expected adjacent splinterd executable")?;
    let peer_metadata = peer_executable
        .metadata()
        .context("cannot inspect daemon socket peer executable")?;
    let expected_metadata = expected_executable
        .metadata()
        .context("cannot inspect the expected splinterd executable")?;
    if !peer_metadata.is_file()
        || !expected_metadata.is_file()
        || peer_metadata.dev() != expected_metadata.dev()
        || peer_metadata.ino() != expected_metadata.ino()
    {
        bail!("daemon socket peer is not the exact adjacent splinterd executable");
    }
    if peer_exited(&peer_pidfd)? {
        bail!("daemon socket peer exited during executable validation");
    }
    if validate_socket_path(path)? != before {
        bail!("daemon socket endpoint changed while connecting");
    }
    Ok(ValidatedConnection { stream, peer_pidfd })
}

/// Connects once to a strictly validated adjacent `splinterd` process.
///
/// # Errors
///
/// Returns an error when path metadata, connected peer credentials/executable,
/// or the post-connect endpoint identity does not match the owner-only contract.
async fn connect_validated(path: &Path) -> Result<ValidatedConnection> {
    let current = env::current_exe().context("cannot resolve the relay executable")?;
    connect_validated_for(path, &current.with_file_name("splinterd")).await
}

async fn copy_bounded<R, W>(reader: &mut R, writer: &mut W) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = Box::new([0_u8; COPY_BUFFER_BYTES]);
    let mut copied = 0_u64;
    loop {
        let count = reader.read(buffer.as_mut_slice()).await?;
        if count == 0 {
            writer.shutdown().await?;
            return Ok(copied);
        }
        writer.write_all(&buffer[..count]).await?;
        copied = copied.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
}

/// Relays opaque bytes between separate input/output streams and one socket.
///
/// Input EOF half-closes only the socket write side and continues draining
/// daemon output. Daemon EOF cancels any blocked input read by dropping the
/// upstream future. No frame bytes are parsed or rewritten.
///
/// # Errors
///
/// Returns the first transport error from either direction.
pub async fn relay_streams<R, W>(input: R, output: W, stream: UnixStream) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut daemon_reader, mut daemon_writer) = stream.into_split();
    let mut input = input;
    let mut output = output;
    let upstream = copy_bounded(&mut input, &mut daemon_writer);
    let downstream = copy_bounded(&mut daemon_reader, &mut output);
    tokio::pin!(upstream);
    tokio::pin!(downstream);

    tokio::select! {
        result = &mut upstream => {
            result.context("stdin-to-daemon relay failed")?;
            downstream.await.context("daemon-to-stdout relay failed")?;
        }
        result = &mut downstream => {
            result.context("daemon-to-stdout relay failed")?;
        }
    }
    Ok(())
}

fn copy_bounded_blocking<R, W>(reader: &mut R, writer: &mut W) -> std::io::Result<u64>
where
    R: std::io::Read,
    W: std::io::Write,
{
    let mut buffer = Box::new([0_u8; COPY_BUFFER_BYTES]);
    let mut copied = 0_u64;
    loop {
        let count = reader.read(buffer.as_mut_slice())?;
        if count == 0 {
            writer.flush()?;
            return Ok(copied);
        }
        writer.write_all(&buffer[..count])?;
        writer.flush()?;
        copied = copied.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
}

#[derive(Clone, Copy)]
enum CompletedDirection {
    Upstream,
    Downstream,
    Peer,
}

fn relay_process_stdio(stream: StdUnixStream, peer_pidfd: OwnedFd) -> Result<()> {
    let mut upstream_socket = stream
        .try_clone()
        .context("cannot duplicate relay socket")?;
    let (completed_tx, completed_rx) = mpsc::channel();
    let upstream_tx = completed_tx.clone();
    thread::spawn(move || {
        let result = copy_bounded_blocking(&mut std::io::stdin().lock(), &mut upstream_socket)
            .and_then(|copied| {
                upstream_socket.shutdown(Shutdown::Write)?;
                Ok(copied)
            });
        let _ = upstream_tx.send((CompletedDirection::Upstream, result));
    });
    let downstream_tx = completed_tx.clone();
    thread::spawn(move || {
        let result = copy_bounded_blocking(&mut &stream, &mut std::io::stdout().lock());
        let _ = downstream_tx.send((CompletedDirection::Downstream, result));
    });
    thread::spawn(move || {
        let mut descriptor = [nix::poll::PollFd::new(
            peer_pidfd.as_fd(),
            nix::poll::PollFlags::POLLIN,
        )];
        let result = nix::poll::poll(&mut descriptor, nix::poll::PollTimeout::NONE)
            .map(|_| 0_u64)
            .map_err(std::io::Error::from);
        let _ = completed_tx.send((CompletedDirection::Peer, result));
    });

    let (direction, result) = completed_rx
        .recv()
        .context("relay worker exited without reporting completion")?;
    result.context(match direction {
        CompletedDirection::Upstream => "stdin-to-daemon relay failed",
        CompletedDirection::Downstream => "daemon-to-stdout relay failed",
        CompletedDirection::Peer => "daemon peer monitor failed",
    })?;
    if matches!(direction, CompletedDirection::Upstream) {
        let (_, downstream) = completed_rx
            .recv()
            .context("daemon-to-stdout relay worker did not complete")?;
        downstream.context("daemon-to-stdout relay failed")?;
    }
    Ok(())
}

/// Connects to the configured socket and relays process stdin/stdout.
///
/// # Errors
///
/// Returns an error when socket validation, connection, or transport fails.
pub async fn run_stdio() -> Result<()> {
    let path = socket_path()?;
    let connection = connect_validated(&path).await?;
    let stream = connection
        .stream
        .into_std()
        .context("cannot convert the validated daemon socket")?;
    stream
        .set_nonblocking(false)
        .context("cannot configure the relay socket")?;
    tokio::task::spawn_blocking(move || relay_process_stdio(stream, connection.peer_pidfd))
        .await
        .context("relay coordinator task failed")?
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
        time,
    };

    use super::*;

    fn test_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "splinterm-relay-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[tokio::test]
    async fn validated_connect_requires_owner_only_stable_socket() {
        let directory = test_directory("socket");
        let path = directory.join("daemon.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let accepted = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        let expected_peer = env::current_exe().unwrap();
        let connection = connect_validated_for(&path, &expected_peer).await.unwrap();
        assert!(
            rustix::io::fcntl_getfd(&connection.stream)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
        let peer = accepted.await.unwrap();
        drop(connection);
        drop(peer);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(connect_validated_for(&path, &expected_peer).await.is_err());

        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn same_uid_non_daemon_peer_is_rejected() {
        let directory = test_directory("peer");
        let path = directory.join("daemon.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let accepted = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        let error = connect_validated_for(&path, Path::new("/bin/false"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("adjacent splinterd"));
        drop(accepted.await.unwrap());
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn relay_is_byte_transparent_and_preserves_input_half_close() {
        let (mut input_writer, input_reader) = tokio::io::duplex(32 * 1024);
        let (output_writer, mut output_reader) = tokio::io::duplex(32 * 1024);
        let (relay_socket, mut daemon_socket) = UnixStream::pair().unwrap();
        let request = vec![0xa5; 2 * 1024 * 1024];
        let response = vec![0x5a; 2 * 1024 * 1024];
        let expected_request = request.clone();
        let expected_response = response.clone();

        let relay = tokio::spawn(relay_streams(input_reader, output_writer, relay_socket));
        let input = tokio::spawn(async move {
            input_writer.write_all(&request).await.unwrap();
            input_writer.shutdown().await.unwrap();
        });
        let daemon = tokio::spawn(async move {
            let mut received = Vec::new();
            daemon_socket.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, expected_request);
            daemon_socket.write_all(&response).await.unwrap();
            daemon_socket.shutdown().await.unwrap();
        });
        let output = tokio::spawn(async move {
            let mut received = Vec::new();
            output_reader.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, expected_response);
        });

        time::timeout(Duration::from_secs(10), async {
            input.await.unwrap();
            daemon.await.unwrap();
            relay.await.unwrap().unwrap();
            output.await.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn upstream_backpressure_is_bounded_until_daemon_reads() {
        let (mut input_writer, input_reader) = tokio::io::duplex(1024);
        let (output_writer, _output_reader) = tokio::io::duplex(1024);
        let (relay_socket, mut daemon_socket) = UnixStream::pair().unwrap();
        let relay = tokio::spawn(relay_streams(input_reader, output_writer, relay_socket));
        let input = tokio::spawn(async move {
            input_writer
                .write_all(&vec![0x3c; 1024 * 1024])
                .await
                .unwrap();
            input_writer.shutdown().await.unwrap();
        });

        time::sleep(Duration::from_millis(100)).await;
        assert!(!input.is_finished(), "upstream ignored daemon backpressure");
        let daemon = tokio::spawn(async move {
            let mut received = Vec::new();
            daemon_socket.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, vec![0x3c; 1024 * 1024]);
        });
        time::timeout(Duration::from_secs(10), async {
            input.await.unwrap();
            daemon.await.unwrap();
            relay.await.unwrap().unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn daemon_eof_cancels_a_blocked_input_direction() {
        let (_input_writer, input_reader) = tokio::io::duplex(1024);
        let (output_writer, mut output_reader) = tokio::io::duplex(1024);
        let (relay_socket, daemon_socket) = UnixStream::pair().unwrap();
        drop(daemon_socket);

        time::timeout(
            Duration::from_secs(2),
            relay_streams(input_reader, output_writer, relay_socket),
        )
        .await
        .unwrap()
        .unwrap();
        let mut output = Vec::new();
        output_reader.read_to_end(&mut output).await.unwrap();
        assert!(output.is_empty());
    }
}
