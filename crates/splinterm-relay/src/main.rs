use std::{fs, io::IsTerminal};

use anyhow::{Result, bail};

const MAX_DIAGNOSTIC_CHARS: usize = 1024;

fn validate_terminal_state(stdin_terminal: bool, stdout_terminal: bool) -> Result<()> {
    if stdin_terminal || stdout_terminal {
        bail!("stdio relay requires non-terminal stdin and stdout; use ssh -T");
    }
    Ok(())
}

fn validate_invocation() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 || arguments[0] != "--stdio" {
        bail!("usage: splinterm-relay --stdio");
    }
    validate_terminal_state(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
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

fn run() -> Result<()> {
    close_inherited_descriptors()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(splinterm_relay::run_stdio())
}

fn main() {
    if let Err(error) = validate_invocation() {
        eprintln!("splinterm-relay: {}", bounded_diagnostic(&error));
        std::process::exit(2);
    }
    if let Err(error) = run() {
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
}
