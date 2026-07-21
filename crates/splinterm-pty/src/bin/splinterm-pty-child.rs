//! Exec-first child helper for `splinterm-pty`.
//!
//! This process is launched before any session or controlling-terminal work.
//! It is therefore outside the daemon's post-fork interval when it calls the
//! safe rustix wrappers corresponding to Foot 1.27.0's `setsid()` and
//! `TIOCSCTTY` sequence.

use std::{
    env,
    ffi::{OsStr, OsString},
    io::{self, IsTerminal, Write},
    os::{
        linux::net::SocketAddrExt,
        unix::{ffi::OsStrExt, net::SocketAddr, net::UnixStream, process::CommandExt},
    },
    process::{self, Command},
};

fn main() {
    let mut arguments = env::args_os().skip(1);
    let mut exec_status = match connect_exec_status(&mut arguments) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("splinterm-pty-child: {error}");
            process::exit(126);
        }
    };
    if let Err(error) = run(arguments) {
        let _ = exec_status.write_all(&[1]);
        let _ = exec_status.flush();
        eprintln!("splinterm-pty-child: {error}");
        process::exit(126);
    }
}

fn connect_exec_status(
    arguments: &mut impl Iterator<Item = OsString>,
) -> Result<UnixStream, String> {
    if arguments.next().as_deref() != Some(OsStr::new("--exec-status")) {
        return Err("expected --exec-status".into());
    }
    let name = arguments
        .next()
        .ok_or_else(|| "exec status capability is missing".to_owned())?;
    let name = name
        .to_str()
        .ok_or_else(|| "exec status capability is not UTF-8".to_owned())?;
    let address = SocketAddr::from_abstract_name(name.as_bytes())
        .map_err(|error| format!("invalid exec status capability: {error}"))?;
    UnixStream::connect_addr(&address)
        .map_err(|error| format!("connecting exec status socket failed: {error}"))
}

#[allow(
    clippy::unnecessary_debug_formatting,
    reason = "Debug formatting escapes an untrusted executable path in diagnostics"
)]
fn run(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let login = match arguments.next().as_deref() {
        Some(value) if value == OsStr::new("--login") => true,
        Some(value) if value == OsStr::new("--no-login") => false,
        _ => return Err("expected --login or --no-login".into()),
    };
    if arguments.next().as_deref() != Some(OsStr::new("--")) {
        return Err("expected -- before the target command".into());
    }
    let program = arguments
        .next()
        .ok_or_else(|| "target command is missing".to_owned())?;
    let target_arguments = arguments.collect::<Vec<_>>();

    rustix::process::setsid().map_err(|error| format!("setsid failed: {error}"))?;
    rustix::process::ioctl_tiocsctty(io::stdin())
        .map_err(|error| format!("claiming the controlling terminal failed: {error}"))?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() || !io::stderr().is_terminal() {
        return Err("standard streams are not attached to a terminal".into());
    }
    io::stdout()
        .write_all(splinterm_pty::CHILD_READY_MARKER)
        .map_err(|error| format!("writing readiness marker failed: {error}"))?;
    io::stdout()
        .flush()
        .map_err(|error| format!("flushing readiness marker failed: {error}"))?;

    let mut command = Command::new(&program);
    command.args(target_arguments);
    if login {
        command.arg0(login_argv0(&program));
    }
    let error = command.exec();
    Err(format!("executing {program:?} failed: {error}"))
}

fn login_argv0(program: &OsStr) -> OsString {
    let program = program.as_bytes();
    let mut value = Vec::with_capacity(program.len() + 1);
    value.push(b'-');
    value.extend_from_slice(program);
    OsString::from(OsStr::from_bytes(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_argv_prefixes_the_supplied_argv_zero() {
        assert_eq!(login_argv0(OsStr::new("/bin/sh")), OsStr::new("-/bin/sh"));
    }
}
