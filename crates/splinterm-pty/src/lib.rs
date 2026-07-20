//! Linux PTY and child-process ownership for Splinterm.
//!
//! The behavioral reference is Foot 1.27.0 at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`, primarily `slave.c`,
//! `terminal.c`, `render.c`, and `reaper.c`.
//!
//! The daemon never installs a Rust `pre_exec` callback. It starts the
//! `splinterm-pty-child` helper with the PTY slave on standard input, output,
//! and error. That freshly executed single-threaded helper creates the session,
//! claims the controlling terminal, and immediately executes the target.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(not(target_os = "linux"))]
compile_error!("splinterm-pty currently supports Linux only");

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixListener, UnixStream},
    },
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fs::{self, Mode, OFlags},
    process::{self, Pid},
    pty::{self, OpenptFlags},
    termios::{self, Winsize},
};
use thiserror::Error;

#[doc(hidden)]
pub const CHILD_READY_MARKER: &[u8] = b"\0splinterm-pty-ready\0";

const FOREIGN_TERMINAL_ENV: &[&str] = &[
    "ALACRITTY_LOG",
    "ALACRITTY_SOCKET",
    "ALACRITTY_WINDOW_ID",
    "CONTOUR_PROFILE",
    "GHOSTTY_BIN_DIR",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_SHELL_INTEGRATION_NO_SUDO",
    "GNOME_TERMINAL_SCREEN",
    "GNOME_TERMINAL_SERVICE",
    "KITTY_INSTALLATION_DIR",
    "KITTY_PID",
    "KITTY_PUBLIC_KEY",
    "KITTY_WINDOW_ID",
    "MLTERM",
    "TERMINAL_NAME",
    "TERMINAL_VERSION_STRING",
    "TERMINAL_VERSION_TRIPLE",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "VTE_VERSION",
    "WEZTERM_CONFIG_DIR",
    "WEZTERM_CONFIG_FILE",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_EXECUTABLE_DIR",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "XTERM_LOCALE",
    "XTERM_SHELL",
    "XTERM_VERSION",
    "ZUTTY_VERSION",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtySize {
    pub rows: u16,
    pub columns: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl PtySize {
    #[must_use]
    pub const fn cells(columns: u16, rows: u16) -> Self {
        Self {
            rows,
            columns,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    fn winsize(self) -> Winsize {
        Winsize {
            ws_row: self.rows,
            ws_col: self.columns,
            ws_xpixel: self.pixel_width,
            ws_ypixel: self.pixel_height,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PtySignal {
    Hangup,
    Terminate,
    Kill,
}

impl PtySignal {
    const fn rustix(self) -> process::Signal {
        match self {
            Self::Hangup => process::Signal::HUP,
            Self::Terminate => process::Signal::TERM,
            Self::Kill => process::Signal::KILL,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtyCommand {
    program: OsString,
    arguments: Vec<OsString>,
    cwd: PathBuf,
    environment: BTreeMap<OsString, Option<OsString>>,
    inherit_environment: bool,
    login_shell: bool,
    term: OsString,
}

impl PtyCommand {
    pub fn new(program: impl Into<OsString>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            cwd: cwd.into(),
            environment: BTreeMap::new(),
            inherit_environment: true,
            login_shell: false,
            term: OsString::from("xterm-256color"),
        }
    }

    #[must_use]
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    #[must_use]
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(name.into(), Some(value.into()));
        self
    }

    #[must_use]
    pub fn env_remove(mut self, name: impl Into<OsString>) -> Self {
        self.environment.insert(name.into(), None);
        self
    }

    #[must_use]
    pub fn inherit_environment(mut self, inherit: bool) -> Self {
        self.inherit_environment = inherit;
        self
    }

    #[must_use]
    pub fn login_shell(mut self, login_shell: bool) -> Self {
        self.login_shell = login_shell;
        self
    }

    #[must_use]
    pub fn term(mut self, term: impl Into<OsString>) -> Self {
        self.term = term.into();
        self
    }
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("PTY operation {operation} failed")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("child process id is outside the supported pid range")]
    InvalidChildId,
    #[error("PTY child helper did not complete session setup")]
    HelperHandshake,
    #[error("target command could not be executed")]
    TargetExec,
}

impl PtyError {
    fn io(operation: &'static str, source: impl Into<io::Error>) -> Self {
        Self::Io {
            operation,
            source: source.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, PtyError>;

pub trait PtySession {
    /// Updates the kernel PTY window size.
    ///
    /// # Errors
    /// Returns an error when the PTY descriptor no longer accepts resize operations.
    fn resize(&mut self, size: PtySize) -> Result<()>;

    /// Writes bytes to the PTY master without blocking.
    ///
    /// # Errors
    /// Returns an error, including `WouldBlock`, from the PTY master.
    fn write(&mut self, bytes: &[u8]) -> Result<usize>;

    /// Polls the child without blocking.
    ///
    /// # Errors
    /// Returns an error when the operating system cannot query the child.
    fn try_wait(&mut self) -> Result<Option<ExitStatus>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxPtyBackend {
    helper: PathBuf,
}

impl LinuxPtyBackend {
    #[must_use]
    pub fn new(helper: impl Into<PathBuf>) -> Self {
        Self {
            helper: helper.into(),
        }
    }

    /// Locates an installed helper beside the running executable.
    ///
    /// # Errors
    /// Returns an error when the current executable path has no parent directory.
    pub fn installed() -> Result<Self> {
        let executable = std::env::current_exe()
            .map_err(|error| PtyError::io("locate current executable", error))?;
        let parent = executable.parent().ok_or_else(|| {
            PtyError::io(
                "locate PTY helper",
                io::Error::new(io::ErrorKind::NotFound, "executable has no parent"),
            )
        })?;
        Ok(Self::new(parent.join("splinterm-pty-child")))
    }

    /// Allocates a PTY and starts the configured command through the exec-first helper.
    ///
    /// # Errors
    /// Returns an error when PTY setup, slave configuration, or helper spawn fails.
    pub fn spawn(&self, command: &PtyCommand, size: PtySize) -> Result<LinuxPtySession> {
        let master = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
            .map_err(|error| PtyError::io("open master", error))?;
        pty::grantpt(&master).map_err(|error| PtyError::io("grant slave", error))?;
        pty::unlockpt(&master).map_err(|error| PtyError::io("unlock slave", error))?;
        termios::tcsetwinsize(&master, size.winsize())
            .map_err(|error| PtyError::io("set initial window size", error))?;

        let slave_name = pty::ptsname(&master, Vec::new())
            .map_err(|error| PtyError::io("resolve slave name", error))?;
        let slave = fs::open(
            slave_name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| PtyError::io("open slave", error))?;
        enable_utf8_input(&slave)?;

        let stdin = duplicate_file(&slave)?;
        let stdout = duplicate_file(&slave)?;
        let stderr = File::from(slave);
        let (status_listener, status_name) = exec_status_listener()?;
        let mut child_command = Command::new(&self.helper);
        child_command
            .arg("--exec-status")
            .arg(status_name)
            .arg(if command.login_shell {
                "--login"
            } else {
                "--no-login"
            })
            .arg("--")
            .arg(&command.program)
            .args(&command.arguments)
            .current_dir(&command.cwd)
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        configure_environment(&mut child_command, command);

        let flags =
            fs::fcntl_getfl(&master).map_err(|error| PtyError::io("read master flags", error))?;
        fs::fcntl_setfl(&master, flags | OFlags::NONBLOCK)
            .map_err(|error| PtyError::io("set master nonblocking", error))?;

        let mut child = child_command
            .spawn()
            .map_err(|error| PtyError::io("spawn PTY helper", error))?;
        let Some(process_group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PtyError::InvalidChildId);
        };
        let Ok(mut exec_status) = accept_exec_status(&status_listener) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PtyError::HelperHandshake);
        };
        let mut master = File::from(master);
        if !wait_for_child_ready(&mut master) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PtyError::HelperHandshake);
        }
        if !target_exec_succeeded(&mut exec_status) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PtyError::TargetExec);
        }

        Ok(LinuxPtySession {
            master,
            child,
            process_group,
        })
    }
}

#[derive(Debug)]
pub struct LinuxPtySession {
    master: File,
    child: Child,
    process_group: Pid,
}

impl LinuxPtySession {
    /// Clones the nonblocking master descriptor for independent read integration.
    ///
    /// # Errors
    /// Returns an error when the descriptor cannot be duplicated.
    pub fn try_clone_reader(&self) -> Result<File> {
        self.master
            .try_clone()
            .map_err(|error| PtyError::io("clone master reader", error))
    }

    #[must_use]
    pub fn child_id(&self) -> u32 {
        self.child.id()
    }

    #[must_use]
    pub fn master_raw_fd(&self) -> i32 {
        self.master.as_raw_fd()
    }

    /// Sends a lifecycle signal to the original child process group.
    ///
    /// # Errors
    /// Returns an error when the process group does not exist or cannot be signaled.
    pub fn signal_process_group(&self, signal: PtySignal) -> Result<()> {
        process::kill_process_group(self.process_group, signal.rustix())
            .map_err(|error| PtyError::io("signal process group", error))
    }

    /// Waits for and reaps the child process.
    ///
    /// # Errors
    /// Returns an error when waiting for the child fails.
    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.child
            .wait()
            .map_err(|error| PtyError::io("wait for child", error))
    }

    /// Reads bytes from the nonblocking PTY master.
    ///
    /// # Errors
    /// Returns an error, including `WouldBlock`, from the PTY master.
    pub fn read(&mut self, bytes: &mut [u8]) -> Result<usize> {
        self.master
            .read(bytes)
            .map_err(|error| PtyError::io("read master", error))
    }
}

impl PtySession for LinuxPtySession {
    fn resize(&mut self, size: PtySize) -> Result<()> {
        termios::tcsetwinsize(&self.master, size.winsize())
            .map_err(|error| PtyError::io("resize PTY", error))
    }

    fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        self.master
            .write(bytes)
            .map_err(|error| PtyError::io("write master", error))
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .map_err(|error| PtyError::io("poll child", error))
    }
}

fn exec_status_listener() -> Result<(UnixListener, String)> {
    let mut nonce = [0_u8; 16];
    rustix::rand::getrandom(&mut nonce, rustix::rand::GetRandomFlags::empty())
        .map_err(|error| PtyError::io("generate exec status capability", error))?;
    let mut name = String::from("splinterm-pty-");
    for byte in nonce {
        std::fmt::Write::write_fmt(&mut name, format_args!("{byte:02x}")).map_err(|error| {
            PtyError::io("format exec status capability", io::Error::other(error))
        })?;
    }
    let address = SocketAddr::from_abstract_name(name.as_bytes())
        .map_err(|error| PtyError::io("create exec status address", error))?;
    let listener = UnixListener::bind_addr(&address)
        .map_err(|error| PtyError::io("bind exec status socket", error))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| PtyError::io("configure exec status socket", error))?;
    Ok((listener, name))
}

fn accept_exec_status(listener: &UnixListener) -> io::Result<UnixStream> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "exec status helper connection timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn target_exec_succeeded(status: &mut UnixStream) -> bool {
    if status
        .set_read_timeout(Some(Duration::from_secs(5)))
        .is_err()
    {
        return false;
    }
    let mut marker = [0_u8; 1];
    loop {
        match status.read(&mut marker) {
            Ok(0) => return true,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Ok(_) | Err(_) => return false,
        }
    }
}

fn wait_for_child_ready(master: &mut File) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut received = Vec::with_capacity(CHILD_READY_MARKER.len());
    while received.len() < CHILD_READY_MARKER.len() && Instant::now() < deadline {
        let mut buffer = [0_u8; 32];
        let remaining = CHILD_READY_MARKER.len() - received.len();
        let read_length = remaining.min(buffer.len());
        match master.read(&mut buffer[..read_length]) {
            Ok(0) => thread::sleep(Duration::from_millis(1)),
            Ok(count) => {
                received.extend_from_slice(&buffer[..count]);
                if !CHILD_READY_MARKER.starts_with(&received) {
                    return false;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    received == CHILD_READY_MARKER
}

fn duplicate_file(fd: &OwnedFd) -> Result<File> {
    rustix::io::dup(fd)
        .map(File::from)
        .map_err(|error| PtyError::io("duplicate slave", error))
}

fn enable_utf8_input(fd: &OwnedFd) -> Result<()> {
    let mut settings =
        termios::tcgetattr(fd).map_err(|error| PtyError::io("read slave termios", error))?;
    settings.input_modes.insert(termios::InputModes::IUTF8);
    termios::tcsetattr(fd, termios::OptionalActions::Now, &settings)
        .map_err(|error| PtyError::io("enable slave UTF-8 input", error))
}

fn configure_environment(command: &mut Command, spec: &PtyCommand) {
    if !spec.inherit_environment {
        command.env_clear();
    }
    for name in FOREIGN_TERMINAL_ENV {
        command.env_remove(name);
    }
    command
        .env("TERM", &spec.term)
        .env("COLORTERM", "truecolor")
        .env("PWD", &spec.cwd);
    for (name, value) in &spec.environment {
        if let Some(value) = value {
            command.env(name, value);
        } else {
            command.env_remove(name);
        }
    }
    if is_valid_shell(&spec.program) {
        command.env("SHELL", &spec.program);
    }
}

fn is_valid_shell(program: &OsString) -> bool {
    let Some(program) = program.to_str() else {
        return false;
    };
    std::fs::read_to_string("/etc/shells").is_ok_and(|shells| {
        shells
            .lines()
            .map(str::trim)
            .any(|line| !line.starts_with('#') && line == program)
    })
}

#[must_use]
pub fn default_shell() -> OsString {
    std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_environment(spec: &PtyCommand) -> BTreeMap<OsString, Option<OsString>> {
        let mut command = Command::new("ignored");
        configure_environment(&mut command, spec);
        command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect()
    }

    use std::ffi::OsStr;

    #[test]
    fn default_terminal_type_matches_the_supported_keyboard_contract() {
        let spec = PtyCommand::new("/bin/sh", "/tmp");
        assert_eq!(spec.term, OsStr::new("xterm-256color"));
    }

    #[test]
    fn environment_cleanup_and_last_override_are_explicit() {
        let spec = PtyCommand::new("/not/a/shell", "/tmp")
            .env("TERM", "override")
            .env_remove("COLORTERM");
        let environment = configured_environment(&spec);
        assert_eq!(
            environment.get(OsStr::new("TERM")),
            Some(&Some(OsString::from("override")))
        );
        assert_eq!(environment.get(OsStr::new("COLORTERM")), Some(&None));
        assert_eq!(environment.get(OsStr::new("TERM_PROGRAM")), Some(&None));
    }

    #[test]
    fn valid_shell_updates_shell_after_environment_overrides() {
        if is_valid_shell(&OsString::from("/bin/sh")) {
            let spec = PtyCommand::new("/bin/sh", "/tmp").env("SHELL", "/wrong");
            assert_eq!(
                configured_environment(&spec).get(OsStr::new("SHELL")),
                Some(&Some(OsString::from("/bin/sh")))
            );
        }
    }
}
