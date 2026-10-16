//! Native OpenSSH askpass fallback for graphical remote launches (Plan 0055
//! Milestone 4).
//!
//! When a graphical remote launch has no terminal, no ssh-agent key that
//! satisfies the server, and no user `SSH_ASKPASS`, OpenSSH is pointed at this
//! same executable as its askpass program. OpenSSH runs it with the prompt text
//! as the only argument; the helper connects back to the status Window process
//! over a per-attempt Unix socket in a private `0700` directory, presents an
//! unguessable per-attempt token, and prints the answer on stdout for OpenSSH.
//!
//! The broker accepts only peers with this user's uid whose process descends
//! from this attempt's OpenSSH child, handles one prompt at a time, and drops
//! the socket and its directory when the attempt ends. Answers are held in
//! [`Secret`] buffers that never reallocate, are wiped on drop, have redacted
//! `Debug` output, and are never logged, rendered, or put in diagnostics.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, DirBuilder},
    io::{Read, Write},
    mem::MaybeUninit,
    os::{
        fd::OwnedFd,
        unix::{
            ffi::OsStrExt as _,
            fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _},
            net::UnixStream as StdUnixStream,
        },
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

/// Environment variable naming the per-attempt broker socket.
pub const SOCKET_ENV: &str = "SPLINTERM_ASKPASS_SOCKET";
/// Environment variable carrying the per-attempt broker token.
pub const TOKEN_ENV: &str = "SPLINTERM_ASKPASS_TOKEN";
/// Longest prompt text accepted from OpenSSH, in bytes.
pub const MAX_PROMPT_BYTES: usize = 4096;
/// Longest answer accepted. OpenSSH reads at most 1023 bytes plus a newline.
pub const MAX_SECRET_BYTES: usize = 1023;
/// How long one prompt may stay open. The status Window then ends the
/// attempt; a secret prompt is never refused while OpenSSH runs.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Prompt lines shown at most.
pub const MAX_PROMPT_LINES: usize = 8;
/// Characters shown at most per prompt line.
pub const MAX_PROMPT_LINE_CHARS: usize = 100;

const MAGIC: [u8; 4] = *b"SPAK";
const VERSION: u8 = 1;
const TOKEN_BYTES: usize = 32;
const TOKEN_HEX_BYTES: usize = TOKEN_BYTES * 2;
const REQUEST_HEADER_BYTES: usize = MAGIC.len() + 2 + TOKEN_HEX_BYTES + 2;
const REPLY_HEADER_BYTES: usize = MAGIC.len() + 2 + 2;
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
const HELPER_SLACK: Duration = Duration::from_secs(30);
/// How long after [`PROMPT_TIMEOUT`] the broker itself gives up on a prompt.
const BROKER_BACKSTOP_SLACK: Duration = Duration::from_secs(15);
const MAX_ANCESTOR_HOPS: usize = 4;
const REPLY_ANSWER: u8 = 0;
const REPLY_REFUSED: u8 = 1;
/// Socket paths are limited to `sun_path` (108 bytes including the NUL).
const MAX_SOCKET_PATH_BYTES: usize = 107;

/// Overwrites a buffer's initialized bytes and spare capacity with zeros, then
/// empties it. `black_box` keeps the compiler from eliding the stores.
pub fn wipe(bytes: &mut Vec<u8>) {
    bytes.fill(0);
    bytes.spare_capacity_mut().fill(MaybeUninit::new(0));
    std::hint::black_box(&*bytes);
    bytes.clear();
}

/// A bounded credential buffer that never reallocates and is wiped on drop.
///
/// It has no `Clone`, `Display`, or serialization, and its `Debug` output is
/// redacted, so it cannot reach logs, diagnostics, or the screen by accident.
pub struct Secret {
    bytes: Vec<u8>,
}

impl Secret {
    /// An empty buffer with room for [`MAX_SECRET_BYTES`] plus a newline.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(MAX_SECRET_BYTES + 1),
        }
    }

    /// Copies `value` into a new buffer; `None` when it is too long.
    #[must_use]
    pub fn from_bytes(value: &[u8]) -> Option<Self> {
        if value.len() > MAX_SECRET_BYTES {
            return None;
        }
        let mut secret = Self::new();
        secret.bytes.extend_from_slice(value);
        Some(secret)
    }

    /// The secret bytes, for the one place that hands them to OpenSSH.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Wipes and empties the buffer.
    pub fn clear(&mut self) {
        wipe(&mut self.bytes);
    }

    /// Appends one whole UTF-8 sequence; refuses it when it would not fit.
    fn push_sequence(&mut self, sequence: &[u8]) -> bool {
        if self.bytes.len() + sequence.len() > MAX_SECRET_BYTES {
            return false;
        }
        self.bytes.extend_from_slice(sequence);
        true
    }

    /// Removes the last character, zeroing its bytes before truncating.
    fn pop_character(&mut self) {
        while let Some(last) = self.bytes.len().checked_sub(1) {
            let byte = self.bytes[last];
            self.bytes[last] = 0;
            self.bytes.truncate(last);
            if byte & 0b1100_0000 != 0b1000_0000 {
                break;
            }
        }
    }

    /// Fills the buffer from `reader` with exactly `length` bytes, in place.
    fn read_exact_from(&mut self, reader: &mut impl Read, length: usize) -> std::io::Result<()> {
        self.clear();
        if length > MAX_SECRET_BYTES {
            return Err(std::io::Error::other("askpass answer is too long"));
        }
        // Within capacity, so this never reallocates and leaves no copy behind.
        self.bytes.resize(length, 0);
        reader.read_exact(&mut self.bytes)
    }
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.bytes);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

/// What OpenSSH told its askpass program about the prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskpassHint {
    /// `SSH_ASKPASS_PROMPT` unset: a passphrase, password, or OpenSSH question.
    Default,
    /// `SSH_ASKPASS_PROMPT=confirm`: a yes/no permission question.
    Confirm,
    /// `SSH_ASKPASS_PROMPT=none`: a notice such as "touch your security key".
    Notify,
}

impl AskpassHint {
    /// Reads OpenSSH's `SSH_ASKPASS_PROMPT` value.
    #[must_use]
    pub fn from_env_value(value: Option<&OsStr>) -> Self {
        match value.map(OsStr::as_bytes) {
            Some(b"confirm") => Self::Confirm,
            Some(b"none") => Self::Notify,
            _ => Self::Default,
        }
    }

    const fn wire(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::Confirm => 1,
            Self::Notify => 2,
        }
    }

    fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Default),
            1 => Some(Self::Confirm),
            2 => Some(Self::Notify),
            _ => None,
        }
    }
}

/// How the status Window presents one OpenSSH prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskpassKind {
    /// A password, passphrase, PIN, or server question: masked input.
    Secret,
    /// A yes/no question, including host-key confirmation: Y or N, no text.
    Confirm,
    /// A notice with no answer; it closes when OpenSSH ends the helper.
    Notice,
}

/// Classifies a prompt. OpenSSH routes host-key questions through askpass
/// without a hint, so a `(yes/no` prompt is a confirmation, never a secret.
#[must_use]
pub fn classify_prompt(hint: AskpassHint, prompt: &str) -> AskpassKind {
    match hint {
        AskpassHint::Confirm => AskpassKind::Confirm,
        AskpassHint::Notify => AskpassKind::Notice,
        AskpassHint::Default => {
            if prompt.to_ascii_lowercase().contains("(yes/no") {
                AskpassKind::Confirm
            } else {
                AskpassKind::Secret
            }
        }
    }
}

fn unsafe_prompt_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}'
                | '\u{feff}'
        )
}

/// Bounds and cleans OpenSSH prompt text, which can include server-chosen
/// keyboard-interactive text: control, bidirectional, and zero-width
/// characters become spaces, lines are shortened, and only the newest
/// [`MAX_PROMPT_LINES`] lines are kept, after a leading `…` when cut.
#[must_use]
pub fn sanitize_prompt(raw: &[u8]) -> Vec<String> {
    let raw = &raw[..raw.len().min(MAX_PROMPT_BYTES)];
    let text = String::from_utf8_lossy(raw);
    let mut lines = text
        .split(['\n', '\r'])
        .map(|line| {
            line.chars()
                .map(|character| {
                    if unsafe_prompt_character(character) {
                        ' '
                    } else {
                        character
                    }
                })
                .collect::<String>()
        })
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .map(|line| {
            if line.chars().count() > MAX_PROMPT_LINE_CHARS {
                let mut short = line
                    .chars()
                    .take(MAX_PROMPT_LINE_CHARS - 1)
                    .collect::<String>();
                short.push('…');
                short
            } else {
                line
            }
        })
        .collect::<Vec<_>>();
    if lines.len() > MAX_PROMPT_LINES {
        lines.drain(..=lines.len() - MAX_PROMPT_LINES);
        lines.insert(0, "…".to_owned());
    }
    if lines.is_empty() {
        lines.push("OpenSSH did not say what it needs.".to_owned());
    }
    lines
}

/// The result of applying keyboard input to a masked field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldOutcome {
    Editing,
    Submit,
    Cancel,
}

/// Applies encoded key input to a masked field. Printable text, including
/// pasted text, is appended up to [`MAX_SECRET_BYTES`]; Backspace removes a
/// character, Ctrl+U clears, Enter submits, and Escape or Ctrl+C cancels.
/// Other control bytes and escape sequences (arrows, Alt+key) are ignored.
/// Bytes after a cancel are discarded. A line break submits only when it
/// ends the input; a line break inside pasted text ends the field's input
/// there without submitting, and the rest of the paste is discarded.
pub fn apply_secret_input(field: &mut Secret, input: &[u8]) -> FieldOutcome {
    let mut index = 0;
    while index < input.len() {
        let byte = input[index];
        match byte {
            b'\r' | b'\n' => {
                let rest = &input[index + 1..];
                return if rest.iter().all(|byte| matches!(byte, b'\r' | b'\n')) {
                    FieldOutcome::Submit
                } else {
                    FieldOutcome::Editing
                };
            }
            0x03 => return FieldOutcome::Cancel,
            0x1b => {
                let Some(&next) = input.get(index + 1) else {
                    return FieldOutcome::Cancel;
                };
                index += 2;
                if next == b'[' {
                    while index < input.len() && !(0x40..=0x7e).contains(&input[index]) {
                        index += 1;
                    }
                    index += 1;
                } else if next == b'O' {
                    index += 1;
                }
                continue;
            }
            0x7f | 0x08 => field.pop_character(),
            0x15 => field.clear(),
            0x00..=0x1f => {}
            _ => {
                let width = match byte {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    0x80..=0xbf | 0xf8..=0xff => {
                        index += 1;
                        continue;
                    }
                    _ => 1,
                };
                let end = (index + width).min(input.len());
                let sequence = &input[index..end];
                if sequence.len() == width && std::str::from_utf8(sequence).is_ok() {
                    // A full field silently ignores more text.
                    let _ = field.push_sequence(sequence);
                    index = end;
                } else {
                    // Drop only the stray lead byte; what follows is re-read.
                    index += 1;
                }
                continue;
            }
        }
        index += 1;
    }
    FieldOutcome::Editing
}

/// Maps one key press on a confirmation: Y is yes; N or Escape is no.
#[must_use]
pub fn confirm_key(bytes: &[u8]) -> Option<bool> {
    match bytes {
        b"y" | b"Y" => Some(true),
        b"n" | b"N" | b"\x1b" => Some(false),
        _ => None,
    }
}

/// The status Window's answer to one prompt.
#[derive(Debug)]
pub enum AskpassReply {
    /// The masked answer, handed to OpenSSH and then wiped.
    Submit(Secret),
    /// Yes to a confirmation.
    Confirm,
    /// No to a confirmation, which OpenSSH reads as "no". A secret prompt is
    /// never refused this way: the broker kills OpenSSH first instead, because
    /// OpenSSH would send an empty password and ask again.
    Cancel,
}

/// One OpenSSH prompt delivered to the status Window of the attempt that
/// started it.
#[derive(Debug)]
pub struct AskpassRequest {
    kind: AskpassKind,
    prompt: Vec<String>,
    reply: oneshot::Sender<AskpassReply>,
}

impl AskpassRequest {
    #[must_use]
    pub const fn kind(&self) -> AskpassKind {
        self.kind
    }

    /// Sanitized, bounded prompt lines.
    #[must_use]
    pub fn prompt_lines(&self) -> &[String] {
        &self.prompt
    }

    /// Answers the prompt. A late answer is dropped, and any secret wiped.
    pub fn answer(self, reply: AskpassReply) {
        let _ = self.reply.send(reply);
    }

    /// True once the helper is gone, the prompt timed out, or the attempt ended.
    #[must_use]
    pub fn is_abandoned(&self) -> bool {
        self.reply.is_closed()
    }

    /// Resolves when the prompt is abandoned.
    pub async fn abandoned(&mut self) {
        self.reply.closed().await;
    }

    #[doc(hidden)]
    #[must_use]
    pub fn for_test(kind: AskpassKind, prompt: &[u8]) -> (Self, oneshot::Receiver<AskpassReply>) {
        let (reply, receiver) = oneshot::channel();
        (
            Self {
                kind,
                prompt: sanitize_prompt(prompt),
                reply,
            },
            receiver,
        )
    }
}

/// Where prompts go and which executable OpenSSH runs as askpass.
#[derive(Clone, Debug)]
pub struct NativePrompt {
    program: PathBuf,
    requests: mpsc::Sender<AskpassRequest>,
}

impl NativePrompt {
    /// `program` must be this Splinterm client, which runs the helper when
    /// started with the broker environment.
    #[must_use]
    pub fn new(program: PathBuf, requests: mpsc::Sender<AskpassRequest>) -> Self {
        Self { program, requests }
    }

    /// Uses the running client executable, when it still exists on disk.
    #[must_use]
    pub fn current_executable(requests: mpsc::Sender<AskpassRequest>) -> Option<Self> {
        let program = std::env::current_exe().ok()?;
        program.is_file().then(|| Self::new(program, requests))
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    let mut offset = 0;
    while offset < N {
        offset +=
            rustix::rand::getrandom(&mut bytes[offset..], rustix::rand::GetRandomFlags::empty())
                .context("OS randomness unavailable")?;
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let difference = left
        .iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    std::hint::black_box(difference) == 0
}

fn runtime_base() -> PathBuf {
    let euid = rustix::process::geteuid().as_raw();
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| {
            path.is_absolute()
                && fs::symlink_metadata(path).is_ok_and(|metadata| {
                    let shared = metadata.permissions().mode() & 0o077 != 0;
                    metadata.is_dir() && metadata.uid() == euid && !shared
                })
        })
        .unwrap_or_else(std::env::temp_dir)
}

/// A private directory removed, with its socket, when dropped.
#[derive(Debug)]
struct PrivateDirectory {
    path: PathBuf,
    socket: PathBuf,
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir(&self.path);
    }
}

/// A bound per-attempt broker, before OpenSSH starts.
#[derive(Debug)]
pub struct AskpassBroker {
    directory: PrivateDirectory,
    token: String,
    listener: UnixListener,
    prompt: NativePrompt,
}

impl AskpassBroker {
    /// Creates a fresh `0700` directory with a `0600` socket and a new token.
    ///
    /// # Errors
    ///
    /// Returns an error when randomness, the directory, or the socket is
    /// unavailable.
    pub fn bind(prompt: &NativePrompt) -> Result<Self> {
        Self::bind_in(&runtime_base(), prompt)
    }

    #[doc(hidden)]
    pub fn bind_in(base: &Path, prompt: &NativePrompt) -> Result<Self> {
        // Killing OpenSSH safely before a secret prompt ends needs pidfds.
        open_pidfd(std::process::id()).context("pidfds are unavailable")?;
        let name = format!("splinterm-askpass-{}", hex(&random_bytes::<12>()?));
        let path = base.join(name);
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .with_context(|| format!("create private askpass directory {}", path.display()))?;
        let directory = PrivateDirectory {
            socket: path.join("s"),
            path,
        };
        if directory.socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
            bail!("private askpass socket path is too long");
        }
        let listener =
            UnixListener::bind(&directory.socket).context("bind private askpass socket")?;
        fs::set_permissions(&directory.socket, fs::Permissions::from_mode(0o600))
            .context("restrict private askpass socket")?;
        Ok(Self {
            directory,
            token: hex(&random_bytes::<TOKEN_BYTES>()?),
            listener,
            prompt: prompt.clone(),
        })
    }

    /// Environment for the OpenSSH child: this executable as a forced askpass,
    /// plus the private socket and token.
    #[must_use]
    pub fn environment(&self) -> [(&'static str, OsString); 4] {
        [
            ("SSH_ASKPASS", self.prompt.program.clone().into_os_string()),
            ("SSH_ASKPASS_REQUIRE", OsString::from("force")),
            (SOCKET_ENV, self.directory.socket.clone().into_os_string()),
            (TOKEN_ENV, OsString::from(&self.token)),
        ]
    }

    /// Starts serving prompts from descendants of `ssh_pid`, the OpenSSH
    /// child of this attempt. Dropping the service before
    /// [`AskpassService::release`] SIGKILLs that OpenSSH first.
    #[must_use]
    pub fn serve(self, ssh_pid: u32) -> AskpassService {
        let terminator = Terminator::for_process(ssh_pid);
        self.serve_with(ssh_pid, terminator)
    }

    /// Like [`Self::serve`] with an explicit terminator, so tests whose stand-in
    /// for OpenSSH is the test process itself can record instead of kill.
    #[doc(hidden)]
    #[must_use]
    pub fn serve_with(self, ancestor: u32, terminator: Terminator) -> AskpassService {
        let Self {
            directory,
            token,
            listener,
            prompt,
        } = self;
        let terminator = Arc::new(terminator);
        let prompt_open = Arc::new(watch::Sender::new(false));
        let task = tokio::spawn(serve_prompts(
            listener,
            token.into_bytes(),
            ancestor,
            terminator.clone(),
            prompt.requests,
            prompt_open.clone(),
        ));
        AskpassService {
            task,
            terminator,
            prompt_open,
            armed: true,
            _directory: directory,
        }
    }
}

fn open_pidfd(pid: u32) -> Option<OwnedFd> {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).ok()?)?;
    rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).ok()
}

fn kill_pidfd(pidfd: &OwnedFd) {
    // A pidfd names one process, so a reaped OpenSSH can never be confused
    // with a later process reusing its pid; signalling it then fails harmlessly.
    let _ = rustix::process::pidfd_send_signal(pidfd, rustix::process::Signal::KILL);
}

#[derive(Debug)]
enum TerminatorTarget {
    Process(Option<OwnedFd>),
    Record(Arc<AtomicUsize>),
}

/// Ends OpenSSH before a secret prompt can end without an answer.
///
/// OpenSSH turns any askpass failure into an empty answer and, for password
/// and keyboard-interactive authentication, sends it and asks again. So a
/// secret prompt is never refused: OpenSSH, and the helper's direct parent
/// when that is a nested OpenSSH (`ProxyJump`), are SIGKILLed first, and the
/// helper is then closed without a reply.
#[doc(hidden)]
#[derive(Debug)]
pub struct Terminator {
    target: TerminatorTarget,
    peer_parent: Mutex<Option<OwnedFd>>,
}

impl Terminator {
    fn for_process(pid: u32) -> Self {
        Self {
            target: TerminatorTarget::Process(open_pidfd(pid)),
            peer_parent: Mutex::new(None),
        }
    }

    /// Counts terminations instead of killing anything.
    #[doc(hidden)]
    #[must_use]
    pub fn recording(count: Arc<AtomicUsize>) -> Self {
        Self {
            target: TerminatorTarget::Record(count),
            peer_parent: Mutex::new(None),
        }
    }

    fn set_peer_parent(&self, pidfd: Option<OwnedFd>) {
        if let Ok(mut peer_parent) = self.peer_parent.lock() {
            *peer_parent = pidfd;
        }
    }

    fn terminate(&self) {
        let peer_parent = self
            .peer_parent
            .lock()
            .ok()
            .and_then(|mut peer_parent| peer_parent.take());
        match &self.target {
            TerminatorTarget::Process(ssh) => {
                if let Some(parent) = &peer_parent {
                    kill_pidfd(parent);
                }
                if let Some(ssh) = ssh {
                    kill_pidfd(ssh);
                }
            }
            TerminatorTarget::Record(count) => {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

/// A running per-attempt broker.
#[derive(Debug)]
pub struct AskpassService {
    task: JoinHandle<()>,
    terminator: Arc<Terminator>,
    /// True while a secret or question waits on the user in the status Window.
    prompt_open: Arc<watch::Sender<bool>>,
    armed: bool,
    _directory: PrivateDirectory,
}

impl AskpassService {
    /// Watches whether a secret or question from this attempt is waiting on
    /// the user, so the negotiation deadline can stop running meanwhile. A
    /// notice takes no answer and does not count.
    #[must_use]
    pub fn prompt_open(&self) -> watch::Receiver<bool> {
        self.prompt_open.subscribe()
    }

    /// Stops serving after OpenSSH authenticated, without killing it.
    pub fn release(mut self) {
        self.armed = false;
    }
}

impl Drop for AskpassService {
    fn drop(&mut self) {
        // Kill first: aborting closes any open helper connection, and the
        // helper's exit would otherwise reach OpenSSH as an empty answer.
        if self.armed {
            self.terminator.terminate();
        }
        // Aborting drops any open prompt's reply channel, so the status Window
        // sees it abandoned and wipes its field.
        self.task.abort();
    }
}

async fn serve_prompts(
    listener: UnixListener,
    token: Vec<u8>,
    ancestor: u32,
    terminator: Arc<Terminator>,
    requests: mpsc::Sender<AskpassRequest>,
    prompt_open: Arc<watch::Sender<bool>>,
) {
    // One connection at a time: OpenSSH asks sequentially, and a second
    // helper waits in the backlog until the first prompt is answered.
    while let Ok((stream, _)) = listener.accept().await {
        let _ = serve_connection(
            stream,
            &token,
            ancestor,
            &terminator,
            &requests,
            &prompt_open,
        )
        .await;
        terminator.set_peer_parent(None);
    }
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces or parentheses; fields resume after
    // the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

fn descends_from(pid: u32, ancestor: u32) -> bool {
    let mut current = pid;
    for _ in 0..MAX_ANCESTOR_HOPS {
        let Some(parent) = parent_pid(current) else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        if parent <= 1 {
            return false;
        }
        current = parent;
    }
    false
}

/// Accepts a same-user peer descending from `ancestor`. Returns a pidfd for
/// the peer's direct parent, the OpenSSH that will read the answer, when that
/// is not `ancestor` itself (a nested `ProxyJump` OpenSSH).
fn verify_peer(stream: &UnixStream, ancestor: u32) -> Result<Option<OwnedFd>> {
    let credentials = stream
        .peer_cred()
        .context("read askpass peer credentials")?;
    if credentials.uid() != rustix::process::geteuid().as_raw() {
        bail!("askpass peer belongs to another user");
    }
    let pid = credentials
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .context("askpass peer pid is unavailable")?;
    if !descends_from(pid, ancestor) {
        bail!("askpass peer was not started by this attempt's OpenSSH");
    }
    let parent = parent_pid(pid).context("askpass peer parent is unavailable")?;
    if parent == ancestor {
        return Ok(None);
    }
    let pidfd = open_pidfd(parent);
    // Still the parent after opening, so the pidfd names that live process.
    Ok(pidfd.filter(|_| parent_pid(pid) == Some(parent)))
}

struct WireRequest {
    hint: AskpassHint,
    token: [u8; TOKEN_HEX_BYTES],
    prompt: Vec<u8>,
}

async fn read_request(reader: &mut (impl AsyncRead + Unpin)) -> Result<WireRequest> {
    let mut header = [0_u8; REQUEST_HEADER_BYTES];
    reader.read_exact(&mut header).await?;
    let (magic, rest) = header.split_at(MAGIC.len());
    if magic != MAGIC || rest[0] != VERSION {
        bail!("askpass request has the wrong version");
    }
    let hint = AskpassHint::from_wire(rest[1]).context("askpass hint is invalid")?;
    let mut token = [0_u8; TOKEN_HEX_BYTES];
    token.copy_from_slice(&rest[2..2 + TOKEN_HEX_BYTES]);
    let length = usize::from(u16::from_be_bytes([
        rest[2 + TOKEN_HEX_BYTES],
        rest[3 + TOKEN_HEX_BYTES],
    ]));
    if length > MAX_PROMPT_BYTES {
        bail!("askpass prompt is too long");
    }
    let mut prompt = vec![0_u8; length];
    reader.read_exact(&mut prompt).await?;
    Ok(WireRequest {
        hint,
        token,
        prompt,
    })
}

async fn write_reply(writer: &mut (impl AsyncWrite + Unpin), reply: AskpassReply) -> Result<()> {
    let (status, secret) = match reply {
        AskpassReply::Submit(secret) => (REPLY_ANSWER, Some(secret)),
        AskpassReply::Confirm => (REPLY_ANSWER, Secret::from_bytes(b"yes")),
        AskpassReply::Cancel => (REPLY_REFUSED, None),
    };
    let body = secret.as_ref().map_or(&[][..], Secret::expose);
    let length = u16::try_from(body.len()).context("askpass answer is too long")?;
    let mut header = [0_u8; REPLY_HEADER_BYTES];
    header[..4].copy_from_slice(&MAGIC);
    header[4] = VERSION;
    header[5] = status;
    header[6..].copy_from_slice(&length.to_be_bytes());
    writer.write_all(&header).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    // `secret` drops here and wipes its buffer.
    Ok(())
}

/// Marks a secret or question open until dropped, on every exit path
/// including the serving task being aborted.
struct PromptOpen<'a>(&'a watch::Sender<bool>);

impl<'a> PromptOpen<'a> {
    fn new(prompt_open: &'a watch::Sender<bool>) -> Self {
        prompt_open.send_replace(true);
        Self(prompt_open)
    }
}

impl Drop for PromptOpen<'_> {
    fn drop(&mut self) {
        self.0.send_replace(false);
    }
}

async fn serve_connection(
    stream: UnixStream,
    token: &[u8],
    ancestor: u32,
    terminator: &Terminator,
    requests: &mpsc::Sender<AskpassRequest>,
    prompt_open: &watch::Sender<bool>,
) -> Result<()> {
    let peer_parent = verify_peer(&stream, ancestor)?;
    let (mut reader, mut writer) = stream.into_split();
    let request = tokio::time::timeout(REQUEST_READ_TIMEOUT, read_request(&mut reader))
        .await
        .context("askpass request timed out")??;
    if !constant_time_eq(&request.token, token) {
        bail!("askpass token was rejected");
    }
    terminator.set_peer_parent(peer_parent);
    // Classify the whole prompt, not the shortened lines shown on screen.
    let kind = classify_prompt(request.hint, &String::from_utf8_lossy(&request.prompt));
    let prompt = sanitize_prompt(&request.prompt);
    // From here until this prompt ends, the user, not OpenSSH, is the wait.
    let _open = (kind != AskpassKind::Notice).then(|| PromptOpen::new(prompt_open));
    let (reply, mut answer) = oneshot::channel();
    // The status Window times prompts out itself and kills the attempt; this
    // later deadline is only a backstop.
    let backstop = tokio::time::Instant::now() + PROMPT_TIMEOUT + BROKER_BACKSTOP_SLACK;
    let delivered = matches!(
        tokio::time::timeout_at(
            backstop,
            requests.send(AskpassRequest {
                kind,
                prompt,
                reply,
            }),
        )
        .await,
        Ok(Ok(()))
    );
    let outcome = if delivered {
        let mut probe = [0_u8; 1];
        tokio::select! {
            answer = &mut answer => answer.ok(),
            // Any byte, EOF, or error means the helper is gone; dropping
            // `answer` tells the status Window to remove the prompt.
            _ = reader.read(&mut probe) => return Ok(()),
            () = tokio::time::sleep_until(backstop) => None,
        }
    } else {
        None
    };
    drop(answer);
    match (kind, outcome) {
        (_, Some(reply @ (AskpassReply::Submit(_) | AskpassReply::Confirm))) => {
            write_reply(&mut writer, reply).await
        }
        // OpenSSH reads a refused question as "no", which is safe.
        (AskpassKind::Confirm, _) => write_reply(&mut writer, AskpassReply::Cancel).await,
        // A notice expects no answer; OpenSSH ignores how its helper ends.
        (AskpassKind::Notice, _) => Ok(()),
        // Never refuse a secret: kill OpenSSH first, then close silently.
        (AskpassKind::Secret, _) => {
            terminator.terminate();
            Ok(())
        }
    }
}

/// Runs the internal askpass helper when OpenSSH started this executable with
/// the broker environment, returning the process exit status. Returns `None`
/// for every ordinary launch.
#[must_use]
pub fn dispatch_internal_askpass() -> Option<i32> {
    let socket = std::env::var_os(SOCKET_ENV).filter(|value| !value.is_empty())?;
    let token = std::env::var_os(TOKEN_ENV).filter(|value| !value.is_empty())?;
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let hint = AskpassHint::from_env_value(std::env::var_os("SSH_ASKPASS_PROMPT").as_deref());
    // Keep the answer out of core dumps and away from same-user ptrace. Only
    // the short-lived helper process does this; the status Window does not.
    let _ = rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable);
    let stdout = std::io::stdout();
    let code = match run_helper(&socket, &token, &arguments, hint, &mut stdout.lock()) {
        Ok(code) => code,
        Err(error) => {
            // Fixed text only: never the prompt or an answer.
            eprintln!("splinterm askpass: {error}");
            1
        }
    };
    Some(code)
}

/// The helper side of one prompt. Writes the answer and a newline to `output`
/// and returns 0, or returns 1 when the prompt was refused.
///
/// # Errors
///
/// Returns an error with fixed text when the arguments, environment, socket,
/// or reply are unusable.
#[doc(hidden)]
pub fn run_helper(
    socket: &OsStr,
    token: &OsStr,
    arguments: &[OsString],
    hint: AskpassHint,
    output: &mut impl Write,
) -> Result<i32> {
    let [prompt] = arguments else {
        bail!("expected exactly one prompt argument");
    };
    let token = token.as_bytes();
    if token.len() != TOKEN_HEX_BYTES || !token.iter().all(u8::is_ascii_hexdigit) {
        bail!("the private prompt token is invalid");
    }
    let prompt = prompt.as_bytes();
    let prompt = &prompt[..prompt.len().min(MAX_PROMPT_BYTES)];
    let mut stream = StdUnixStream::connect(socket)
        .map_err(|_| anyhow::anyhow!("the Splinterm prompt is unavailable"))?;
    let server = rustix::net::sockopt::socket_peercred(&stream)
        .map_err(|_| anyhow::anyhow!("cannot identify the Splinterm prompt"))?;
    if server.uid != rustix::process::geteuid() {
        bail!("the Splinterm prompt belongs to another user");
    }
    stream.set_write_timeout(Some(REQUEST_READ_TIMEOUT))?;
    stream.set_read_timeout(Some(PROMPT_TIMEOUT + HELPER_SLACK))?;
    let length = u16::try_from(prompt.len()).context("prompt is too long")?;
    let mut request = Vec::with_capacity(REQUEST_HEADER_BYTES + prompt.len());
    request.extend_from_slice(&MAGIC);
    request.push(VERSION);
    request.push(hint.wire());
    request.extend_from_slice(token);
    request.extend_from_slice(&length.to_be_bytes());
    request.extend_from_slice(prompt);
    stream
        .write_all(&request)
        .map_err(|_| anyhow::anyhow!("the Splinterm prompt is unavailable"))?;
    let mut header = [0_u8; REPLY_HEADER_BYTES];
    if stream.read_exact(&mut header).is_err() {
        // Closed without an answer: abandoned, timed out, or a notice ended.
        return Ok(1);
    }
    if header[..4] != MAGIC || header[4] != VERSION {
        bail!("the Splinterm prompt replied with the wrong version");
    }
    let length = usize::from(u16::from_be_bytes([header[6], header[7]]));
    match header[5] {
        REPLY_ANSWER => {
            let mut answer = Secret::new();
            answer
                .read_exact_from(&mut stream, length)
                .map_err(|_| anyhow::anyhow!("the Splinterm prompt reply was cut short"))?;
            // Capacity reserves room for the newline OpenSSH expects.
            answer.bytes.push(b'\n');
            let written = output
                .write_all(answer.expose())
                .and_then(|()| output.flush());
            drop(answer);
            written.map_err(|_| anyhow::anyhow!("cannot hand the answer to OpenSSH"))?;
            Ok(0)
        }
        REPLY_REFUSED => Ok(1),
        _ => bail!("the Splinterm prompt reply is invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(input: &[u8]) -> (Secret, FieldOutcome) {
        let mut secret = Secret::new();
        let outcome = apply_secret_input(&mut secret, input);
        (secret, outcome)
    }

    #[test]
    fn prompts_are_classified_by_hint_then_by_question_shape() {
        assert_eq!(
            classify_prompt(AskpassHint::Default, "case@host's password:"),
            AskpassKind::Secret
        );
        assert_eq!(
            classify_prompt(
                AskpassHint::Default,
                "Enter passphrase for key '/home/case/.ssh/id_ed25519':"
            ),
            AskpassKind::Secret
        );
        assert_eq!(
            classify_prompt(
                AskpassHint::Default,
                "The authenticity of host 'h' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?"
            ),
            AskpassKind::Confirm
        );
        assert_eq!(
            classify_prompt(AskpassHint::Default, "Accept updated hostkeys? (YES/NO):"),
            AskpassKind::Confirm
        );
        assert_eq!(
            classify_prompt(AskpassHint::Confirm, "Allow use of key id_ed25519?"),
            AskpassKind::Confirm
        );
        assert_eq!(
            classify_prompt(AskpassHint::Notify, "Confirm user presence for key"),
            AskpassKind::Notice
        );
        assert_eq!(
            AskpassHint::from_env_value(Some(OsStr::new("confirm"))),
            AskpassHint::Confirm
        );
        assert_eq!(
            AskpassHint::from_env_value(Some(OsStr::new("none"))),
            AskpassHint::Notify
        );
        assert_eq!(
            AskpassHint::from_env_value(Some(OsStr::new("other"))),
            AskpassHint::Default
        );
        assert_eq!(AskpassHint::from_env_value(None), AskpassHint::Default);
        for hint in [
            AskpassHint::Default,
            AskpassHint::Confirm,
            AskpassHint::Notify,
        ] {
            assert_eq!(AskpassHint::from_wire(hint.wire()), Some(hint));
        }
        assert_eq!(AskpassHint::from_wire(9), None);
    }

    #[test]
    fn prompt_text_is_sanitized_and_bounded() {
        assert_eq!(
            sanitize_prompt(b"case@host's password: "),
            ["case@host's password:"]
        );
        assert_eq!(
            sanitize_prompt("evil\u{202e}txt\u{200b}\u{1b}[31m\u{7}ok\r\nnext".as_bytes()),
            ["evil txt  [31m ok", "next"]
        );
        assert_eq!(
            sanitize_prompt(b"\n \n"),
            ["OpenSSH did not say what it needs."]
        );
        assert_eq!(sanitize_prompt(b"\xff\xfepass:"), ["\u{fffd}\u{fffd}pass:"]);
        let long = (0..20)
            .map(|index| format!("{index} {}", "x".repeat(150)))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = sanitize_prompt(long.as_bytes());
        assert_eq!(lines.len(), MAX_PROMPT_LINES);
        assert_eq!(lines[0], "…");
        assert!(lines.last().unwrap().starts_with("19 "));
        assert!(
            lines
                .iter()
                .all(|line| line.chars().count() <= MAX_PROMPT_LINE_CHARS)
        );
        let huge = vec![b'a'; MAX_PROMPT_BYTES * 4];
        let lines = sanitize_prompt(&huge);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn masked_field_edits_submits_cancels_and_bounds() {
        let (secret, outcome) = field(b"hunter2\r");
        assert_eq!(
            (secret.expose(), outcome),
            (&b"hunter2"[..], FieldOutcome::Submit)
        );
        let (secret, outcome) = field(b"abc\x7f\x7fd");
        assert_eq!(
            (secret.expose(), outcome),
            (&b"ad"[..], FieldOutcome::Editing)
        );
        let (secret, outcome) = field("pässw\u{f6}rd\x7f\x7f".as_bytes());
        assert_eq!(secret.expose(), "pässwö".as_bytes());
        assert_eq!(outcome, FieldOutcome::Editing);
        let (secret, _) = field(b"abc\x15xy");
        assert_eq!(secret.expose(), b"xy");
        // Arrow keys, Alt+key, and other controls are not text.
        let (secret, outcome) = field(b"a\x1b[A\x1b[1;5Cb\x1bOPc\x1bxd\x01\t");
        assert_eq!(
            (secret.expose(), outcome),
            (&b"abcd"[..], FieldOutcome::Editing)
        );
        assert_eq!(field(b"abc\x1b").1, FieldOutcome::Cancel);
        assert_eq!(field(b"abc\x03def").1, FieldOutcome::Cancel);
        // A line break inside a paste stops input there without submitting;
        // the rest is discarded. A trailing line break submits.
        let (secret, outcome) = field(b"first\nsecond");
        assert_eq!(
            (secret.expose(), outcome),
            (&b"first"[..], FieldOutcome::Editing)
        );
        let (secret, outcome) = field(b"first\r\n");
        assert_eq!(
            (secret.expose(), outcome),
            (&b"first"[..], FieldOutcome::Submit)
        );
        // Invalid UTF-8 is dropped rather than half-stored.
        let (secret, _) = field(b"a\xc3b\x80c");
        assert_eq!(secret.expose(), b"abc");

        let mut secret = Secret::new();
        let capacity = secret.bytes.capacity();
        let long = vec![b'x'; MAX_SECRET_BYTES + 50];
        apply_secret_input(&mut secret, &long);
        assert_eq!(secret.len(), MAX_SECRET_BYTES);
        apply_secret_input(&mut secret, "é".as_bytes());
        assert_eq!(secret.len(), MAX_SECRET_BYTES);
        assert_eq!(
            secret.bytes.capacity(),
            capacity,
            "the field never reallocates"
        );
        assert!(Secret::from_bytes(&long).is_none());
    }

    #[test]
    fn secrets_are_redacted_and_wiped() {
        let secret = Secret::from_bytes(b"hunter2").unwrap();
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        let reply = AskpassReply::Submit(secret);
        assert!(!format!("{reply:?}").contains("hunter2"));

        let mut buffer = Vec::with_capacity(16);
        buffer.extend_from_slice(b"hunter2-secret");
        buffer.truncate(6);
        let capacity = buffer.capacity();
        wipe(&mut buffer);
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), capacity, "wiping keeps the allocation");
        let mut secret = Secret::from_bytes(b"ab").unwrap();
        secret.pop_character();
        assert_eq!(secret.expose(), b"a");
        let mut secret = Secret::from_bytes(b"pw").unwrap();
        secret.clear();
        assert!(secret.is_empty());
    }

    #[test]
    fn confirm_keys_require_an_explicit_choice() {
        assert_eq!(confirm_key(b"y"), Some(true));
        assert_eq!(confirm_key(b"Y"), Some(true));
        for no in [&b"n"[..], b"N", b"\x1b"] {
            assert_eq!(confirm_key(no), Some(false));
        }
        for other in [&b"\r"[..], b"yes", b"", b"x"] {
            assert_eq!(confirm_key(other), None);
        }
    }

    #[test]
    fn tokens_compare_in_constant_time_and_ancestry_is_read_from_proc() {
        assert!(constant_time_eq(b"abcd", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abce"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        let token = hex(&random_bytes::<TOKEN_BYTES>().unwrap());
        assert_eq!(token.len(), TOKEN_HEX_BYTES);
        assert_ne!(token, hex(&random_bytes::<TOKEN_BYTES>().unwrap()));
        let me = std::process::id();
        let parent = parent_pid(me).unwrap();
        assert!(descends_from(me, parent));
        assert!(!descends_from(me, me));
        assert!(!descends_from(u32::MAX, parent));
    }

    struct Broker {
        service: AskpassService,
        environment: [(&'static str, OsString); 4],
        requests: mpsc::Receiver<AskpassRequest>,
        directory: PathBuf,
        /// Times the broker would have killed OpenSSH.
        kills: Arc<AtomicUsize>,
    }

    impl Broker {
        fn token(&self) -> OsString {
            env_value(&self.environment, TOKEN_ENV).to_owned()
        }

        fn kills(&self) -> usize {
            self.kills.load(Ordering::SeqCst)
        }
    }

    /// The test process's parent stands in for OpenSSH, so terminations are
    /// recorded, never delivered.
    fn start_broker(ancestor: u32) -> Broker {
        let (requests, receiver) = mpsc::channel(4);
        let prompt = NativePrompt::new(PathBuf::from("/nonexistent/splinterm"), requests);
        let broker = AskpassBroker::bind_in(&std::env::temp_dir(), &prompt).unwrap();
        let environment = broker.environment();
        let directory = broker.directory.path.clone();
        let metadata = fs::metadata(&directory).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        let socket = fs::metadata(&broker.directory.socket).unwrap();
        assert_eq!(socket.permissions().mode() & 0o777, 0o600);
        let kills = Arc::new(AtomicUsize::new(0));
        Broker {
            service: broker.serve_with(ancestor, Terminator::recording(kills.clone())),
            environment,
            requests: receiver,
            directory,
            kills,
        }
    }

    fn openssh_stand_in() -> u32 {
        parent_pid(std::process::id()).unwrap()
    }

    fn env_value<'a>(environment: &'a [(&'static str, OsString); 4], key: &str) -> &'a OsStr {
        &environment.iter().find(|(name, _)| *name == key).unwrap().1
    }

    /// Runs the helper on a thread of this test process, so the peer pid is
    /// this process and its parent stands in for the attempt's OpenSSH.
    async fn helper(
        environment: &[(&'static str, OsString); 4],
        token: &OsStr,
        prompt: &str,
        hint: AskpassHint,
    ) -> (Result<i32>, Vec<u8>) {
        let socket = env_value(environment, SOCKET_ENV).to_owned();
        let token = token.to_owned();
        let prompt = OsString::from(prompt);
        tokio::task::spawn_blocking(move || {
            let mut output = Vec::new();
            let result = run_helper(&socket, &token, &[prompt], hint, &mut output);
            (result, output)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn round_trip_answers_and_cleanup_remove_the_socket() {
        let mut broker = start_broker(openssh_stand_in());
        assert_eq!(
            env_value(&broker.environment, "SSH_ASKPASS_REQUIRE"),
            OsStr::new("force")
        );
        assert_eq!(
            env_value(&broker.environment, "SSH_ASKPASS"),
            OsStr::new("/nonexistent/splinterm")
        );
        let token = broker.token();
        let mut requests = std::mem::replace(&mut broker.requests, mpsc::channel(1).1);
        let answering = tokio::spawn(async move {
            let request = requests.recv().await.unwrap();
            assert_eq!(request.kind(), AskpassKind::Secret);
            assert_eq!(request.prompt_lines(), ["case@host's password:"]);
            request.answer(AskpassReply::Submit(
                Secret::from_bytes(b"hunter2").unwrap(),
            ));
            let request = requests.recv().await.unwrap();
            assert_eq!(request.kind(), AskpassKind::Confirm);
            request.answer(AskpassReply::Confirm);
            let request = requests.recv().await.unwrap();
            assert_eq!(request.kind(), AskpassKind::Confirm);
            request.answer(AskpassReply::Cancel);
        });
        let environment = &broker.environment;
        let (code, output) = helper(
            environment,
            &token,
            "case@host's password: ",
            AskpassHint::Default,
        )
        .await;
        assert_eq!((code.unwrap(), output.as_slice()), (0, &b"hunter2\n"[..]));
        let (code, output) = helper(environment, &token, "Allow key?", AskpassHint::Confirm).await;
        assert_eq!((code.unwrap(), output.as_slice()), (0, &b"yes\n"[..]));
        // "No" to a question is a refusal, which OpenSSH reads as "no".
        let (code, output) = helper(
            environment,
            &token,
            "Accept updated hostkeys? (yes/no):",
            AskpassHint::Default,
        )
        .await;
        assert_eq!((code.unwrap(), output.as_slice()), (1, &b""[..]));
        answering.await.unwrap();
        assert_eq!(
            broker.kills(),
            0,
            "answers and questions never kill OpenSSH"
        );
        let Broker {
            service,
            directory,
            kills,
            ..
        } = broker;
        service.release();
        assert_eq!(kills.load(Ordering::SeqCst), 0, "release does not kill");
        assert!(!directory.exists(), "the private directory is removed");
    }

    #[tokio::test]
    async fn a_secret_prompt_without_an_answer_kills_openssh_before_closing() {
        // The status Window drops the prompt without answering.
        let mut broker = start_broker(openssh_stand_in());
        let token = broker.token();
        let mut requests = std::mem::replace(&mut broker.requests, mpsc::channel(1).1);
        let dropping = tokio::spawn(async move {
            let request = requests.recv().await.unwrap();
            assert_eq!(request.kind(), AskpassKind::Secret);
            drop(request);
            requests
        });
        let (code, output) = helper(
            &broker.environment,
            &token,
            "password:",
            AskpassHint::Default,
        )
        .await;
        assert_eq!((code.unwrap(), output.as_slice()), (1, &b""[..]));
        assert_eq!(
            broker.kills(),
            1,
            "OpenSSH was killed before the helper exited"
        );
        let mut requests = dropping.await.unwrap();

        // An explicit Cancel on a secret is treated the same way.
        let answering = tokio::spawn(async move {
            requests.recv().await.unwrap().answer(AskpassReply::Cancel);
            requests
        });
        let (code, _) = helper(
            &broker.environment,
            &token,
            "password:",
            AskpassHint::Default,
        )
        .await;
        assert_eq!(code.unwrap(), 1);
        assert_eq!(broker.kills(), 2);
        drop(answering.await.unwrap());

        // With the status Window gone, the prompt cannot even be delivered.
        let (code, _) = helper(
            &broker.environment,
            &token,
            "password:",
            AskpassHint::Default,
        )
        .await;
        assert_eq!(code.unwrap(), 1);
        assert_eq!(broker.kills(), 3);
        broker.service.release();
    }

    #[tokio::test]
    async fn wrong_tokens_and_foreign_processes_are_refused_without_a_prompt() {
        let mut broker = start_broker(openssh_stand_in());
        let wrong = OsString::from("0".repeat(TOKEN_HEX_BYTES));
        let (code, output) = helper(
            &broker.environment,
            &wrong,
            "password:",
            AskpassHint::Default,
        )
        .await;
        assert_eq!((code.unwrap(), output.as_slice()), (1, &b""[..]));
        let (code, _) = helper(
            &broker.environment,
            OsStr::new("not-a-token"),
            "password:",
            AskpassHint::Default,
        )
        .await;
        assert!(code.is_err());
        assert!(
            broker.requests.try_recv().is_err(),
            "no prompt reached the Window"
        );
        assert_eq!(
            broker.kills(),
            0,
            "an impostor cannot make Splinterm kill OpenSSH"
        );
        broker.service.release();

        // A correct token from a process outside the attempt's OpenSSH tree.
        let mut broker = start_broker(u32::MAX);
        let token = broker.token();
        let (code, _) = helper(
            &broker.environment,
            &token,
            "password:",
            AskpassHint::Default,
        )
        .await;
        assert_eq!(code.unwrap(), 1);
        assert!(broker.requests.try_recv().is_err());
        assert_eq!(broker.kills(), 0);
        broker.service.release();
    }

    #[tokio::test]
    async fn secrets_and_questions_pause_negotiation_but_notices_do_not() {
        let mut broker = start_broker(openssh_stand_in());
        let token = broker.token();
        let mut open = broker.service.prompt_open();
        assert!(!*open.borrow_and_update());
        for (prompt, hint) in [
            ("password:", AskpassHint::Default),
            ("Allow key?", AskpassHint::Confirm),
        ] {
            let pending = {
                let environment = broker.environment.clone();
                let token = token.clone();
                tokio::spawn(async move { helper(&environment, &token, prompt, hint).await })
            };
            let request = broker.requests.recv().await.unwrap();
            assert!(*open.borrow_and_update(), "{prompt} waits on the user");
            request.answer(if hint == AskpassHint::Confirm {
                AskpassReply::Confirm
            } else {
                AskpassReply::Submit(Secret::from_bytes(b"hunter2").unwrap())
            });
            assert_eq!(pending.await.unwrap().0.unwrap(), 0);
            tokio::time::timeout(Duration::from_secs(5), open.wait_for(|open| !*open))
                .await
                .unwrap()
                .unwrap();
        }

        let socket = env_value(&broker.environment, SOCKET_ENV).to_owned();
        let client = tokio::task::spawn_blocking(move || {
            let mut stream = StdUnixStream::connect(socket).unwrap();
            let mut request = Vec::new();
            request.extend_from_slice(&MAGIC);
            request.extend_from_slice(&[VERSION, AskpassHint::Notify.wire()]);
            request.extend_from_slice(token.as_bytes());
            request.extend_from_slice(&5_u16.to_be_bytes());
            request.extend_from_slice(b"touch");
            stream.write_all(&request).unwrap();
            stream
        });
        let request = broker.requests.recv().await.unwrap();
        assert_eq!(request.kind(), AskpassKind::Notice);
        assert!(!*open.borrow(), "a notice takes no answer");
        drop((client.await.unwrap(), request));
        assert_eq!(broker.kills(), 0);
        broker.service.release();
    }

    #[tokio::test]
    async fn an_abandoned_helper_withdraws_its_prompt() {
        let mut broker = start_broker(openssh_stand_in());
        let socket = env_value(&broker.environment, SOCKET_ENV).to_owned();
        let token = broker.token();
        // A raw client that sends a request and then disconnects.
        let client = tokio::task::spawn_blocking(move || {
            let mut stream = StdUnixStream::connect(socket).unwrap();
            let mut request = Vec::new();
            request.extend_from_slice(&MAGIC);
            request.extend_from_slice(&[VERSION, AskpassHint::Notify.wire()]);
            request.extend_from_slice(token.as_bytes());
            request.extend_from_slice(&5_u16.to_be_bytes());
            request.extend_from_slice(b"touch");
            stream.write_all(&request).unwrap();
            stream
        });
        let mut request = broker.requests.recv().await.unwrap();
        assert_eq!(request.kind(), AskpassKind::Notice);
        assert!(!request.is_abandoned());
        drop(client.await.unwrap());
        tokio::time::timeout(Duration::from_secs(5), request.abandoned())
            .await
            .unwrap();
        assert!(request.is_abandoned());
        assert_eq!(broker.kills(), 0);
        broker.service.release();
    }

    #[tokio::test]
    async fn dropping_an_armed_service_kills_openssh_then_abandons_the_prompt() {
        let mut broker = start_broker(openssh_stand_in());
        let token = broker.token();
        let pending = {
            let environment = broker.environment.clone();
            tokio::spawn(async move {
                helper(&environment, &token, "password:", AskpassHint::Default).await
            })
        };
        let mut request = broker.requests.recv().await.unwrap();
        let kills = broker.kills.clone();
        drop(broker.service);
        assert_eq!(kills.load(Ordering::SeqCst), 1, "killed before the abort");
        tokio::time::timeout(Duration::from_secs(5), request.abandoned())
            .await
            .unwrap();
        let (code, output) = pending.await.unwrap();
        assert_eq!((code.unwrap(), output.as_slice()), (1, &b""[..]));
        assert_eq!(kills.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn yes_no_questions_are_classified_on_the_full_prompt() {
        // The question is past the shown lines and line-length limits.
        let mut prompt = "x".repeat(MAX_PROMPT_LINE_CHARS * 2);
        prompt.push_str(" Continue (yes/no)?");
        let lines = sanitize_prompt(prompt.as_bytes());
        assert!(!lines.join("\n").contains("(yes/no"));
        assert_eq!(
            classify_prompt(AskpassHint::Default, &prompt),
            AskpassKind::Confirm
        );
    }
}
