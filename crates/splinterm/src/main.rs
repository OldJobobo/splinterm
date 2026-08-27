mod app;

use splinterm::diagnostics::{
    DiagnosticErrorCode, ExitClass, finish_global, global as graphical_diagnostics,
};

fn main() {
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
