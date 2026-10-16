mod app;

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
    run_client();
}

#[tokio::main(worker_threads = 2)]
async fn run_client() {
    match app::run().await {
        Ok(()) => {
            if let Some(diagnostics) = graphical_diagnostics() {
                diagnostics.finish(ExitClass::Unknown, None);
            }
        }
        Err(error) => {
            if let Some(not_connected) = error.downcast_ref::<app::RemoteNotConnected>() {
                // The status Window already explained any failure; record its
                // category rather than an internal error.
                if let Some(diagnostics) = graphical_diagnostics() {
                    diagnostics.finish(ExitClass::Unknown, not_connected.last_failure);
                }
                eprintln!("splinterm: {not_connected}");
                std::process::exit(app::RemoteNotConnected::EXIT_CODE);
            }
            if graphical_diagnostics().is_some() {
                finish_global(ExitClass::Unknown, Some(DiagnosticErrorCode::InternalError));
                eprintln!("splinterm graphical client failed");
            } else {
                eprintln!("{error:#}");
            }
            std::process::exit(1);
        }
    }
}
