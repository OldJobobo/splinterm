mod app;

use splinterm::diagnostics::{
    DiagnosticErrorCode, ExitClass, finish_global, global as graphical_diagnostics,
};

#[tokio::main(worker_threads = 2)]
async fn main() {
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
