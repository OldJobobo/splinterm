use std::{fs, io::IsTerminal};

use anyhow::{Result, bail};

const MAX_DIAGNOSTIC_CHARS: usize = 1024;

fn validate_terminal_state(stdin_terminal: bool, stdout_terminal: bool) -> Result<()> {
    if stdin_terminal || stdout_terminal {
        bail!("stdio relay requires non-terminal stdin and stdout; use ssh -T");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RelayMode {
    Raw,
    Graphical,
}

fn mode_for(arguments: &[std::ffi::OsString]) -> Result<RelayMode> {
    match arguments {
        [argument] if argument == "--stdio" => Ok(RelayMode::Raw),
        [argument] if argument == "--graphical-stdio" => Ok(RelayMode::Graphical),
        _ => bail!("usage: splinterm-relay (--stdio|--graphical-stdio)"),
    }
}

fn validate_invocation() -> Result<RelayMode> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let mode = mode_for(&arguments)?;
    validate_terminal_state(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )?;
    Ok(mode)
}

fn bounded_diagnostic(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    message.chars().take(MAX_DIAGNOSTIC_CHARS).collect()
}

fn close_inherited_descriptors() -> Result<()> {
    let mut descriptors = Vec::new();
    for entry in fs::read_dir("/proc/self/fd")? {
        let entry = entry?;
        if let Ok(descriptor) = entry.file_name().to_string_lossy().parse::<i32>()
            && descriptor > 2
        {
            descriptors.push(descriptor);
        }
    }
    for descriptor in descriptors {
        let _ = nix::unistd::close(descriptor);
    }
    Ok(())
}

fn run(mode: RelayMode) -> Result<()> {
    close_inherited_descriptors()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match mode {
        RelayMode::Raw => runtime.block_on(splinterm_relay::run_stdio()),
        RelayMode::Graphical => runtime.block_on(splinterm_relay::run_graphical_stdio()),
    }
}

fn main() {
    let mode = match validate_invocation() {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("splinterm-relay: {}", bounded_diagnostic(&error));
            std::process::exit(2);
        }
    };
    if let Err(error) = run(mode) {
        eprintln!("splinterm-relay: {}", bounded_diagnostic(&error));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_are_bounded() {
        let error = anyhow::anyhow!("{}", "x".repeat(MAX_DIAGNOSTIC_CHARS + 100));
        assert_eq!(
            bounded_diagnostic(&error).chars().count(),
            MAX_DIAGNOSTIC_CHARS
        );
    }

    #[test]
    fn either_terminal_endpoint_is_rejected() {
        assert!(validate_terminal_state(true, false).is_err());
        assert!(validate_terminal_state(false, true).is_err());
        assert!(validate_terminal_state(true, true).is_err());
        validate_terminal_state(false, false).unwrap();
    }

    #[test]
    fn relay_modes_are_exact_and_distinct() {
        assert_eq!(mode_for(&["--stdio".into()]).unwrap(), RelayMode::Raw);
        assert_eq!(
            mode_for(&["--graphical-stdio".into()]).unwrap(),
            RelayMode::Graphical
        );
        assert!(mode_for(&[]).is_err());
        assert!(mode_for(&["--stdio".into(), "extra".into()]).is_err());
    }
}
