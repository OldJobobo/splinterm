mod app;
mod handoff_acquire;

use splinterd::child_stderr::CLIENT_FAILURE_PREFIX;
use splinterm::diagnostics::{
    DiagnosticErrorCode, ExitClass, finish_global, global as graphical_diagnostics,
};

fn main() {
    // OpenSSH runs this executable as its askpass program, with the prompt as
    // the only argument, when a graphical remote launch needs a credential.
    // Checked first so prompt text is never parsed as a client argument.
    if let Some(code) = splinterm::remote_askpass::dispatch_internal_askpass() {
        std::process::exit(code);
    }
    match splinterd::handoff_preflight::dispatch_internal_preflight(
        splinterd::handoff_preflight::PreflightRole::Client,
    ) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("sealed preflight failed: {error}");
            std::process::exit(1);
        }
    }
    // A Window relaunched across a daemon upgrade claims its two inherited
    // descriptors here, before Tokio or any other thread exists (Plan 0061
    // M3.15). Without its activation flags this reads only argv.
    let window_resume = match handoff_acquire::acquire_before_tokio() {
        Ok(window_resume) => window_resume,
        Err(error) => {
            eprintln!("splinterm: {error:#}");
            std::process::exit(1);
        }
    };
    run_client(window_resume);
}

#[tokio::main(worker_threads = 2)]
async fn run_client(window_resume: Option<handoff_acquire::AcquiredWindowResume>) {
    let result = match window_resume {
        Some(window_resume) => app::run_resumed(window_resume).await,
        None => app::run().await,
    };
    match result {
        Ok(()) => {
            if let Some(diagnostics) = graphical_diagnostics() {
                diagnostics.finish(ExitClass::Unknown, None);
            }
        }
        Err(error) => {
            if let Some(not_connected) = error.downcast_ref::<app::RemoteNotConnected>() {
                // The status Window already explained any failure; record its
                // category rather than an internal error. Closing that Window
                // queued a clean exit, which this unclean result overrides.
                finish_global(ExitClass::Unknown, not_connected.last_failure);
                eprintln!("splinterm: {not_connected}");
                std::process::exit(app::RemoteNotConnected::EXIT_CODE);
            }
            if graphical_diagnostics().is_some() {
                finish_global(ExitClass::Unknown, Some(DiagnosticErrorCode::InternalError));
                eprintln!("{}", graphical_failure(&error, app::is_dialog_child()));
            } else {
                eprintln!("{error:#}");
            }
            std::process::exit(1);
        }
    }
}

/// The line a failed graphical client prints. An ordinary graphical client
/// prints only that it failed, never arbitrary error text, because its
/// standard error can reach the desktop session's journal; the diagnostics
/// log records its category. A trusted dialog child's standard error is a
/// private pipe to the command that asked, which reports it bounded
/// (`splinterd::child_stderr`), so the child adds its error chain: without
/// it, a dialog that fails before it maps leaves no reason anywhere.
fn graphical_failure(error: &anyhow::Error, dialog_child: bool) -> String {
    if dialog_child {
        format!("{CLIENT_FAILURE_PREFIX}: {error:#}")
    } else {
        CLIENT_FAILURE_PREFIX.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::graphical_failure;

    #[test]
    fn only_a_dialog_child_reports_its_error_chain() {
        let error = anyhow::anyhow!("terminal snapshot metadata is invalid")
            .context("show the trusted dialog");
        assert_eq!(
            graphical_failure(&error, true),
            "splinterm graphical client failed: show the trusted dialog: terminal snapshot metadata is invalid"
        );
        assert_eq!(
            graphical_failure(&error, false),
            "splinterm graphical client failed"
        );
    }
}
