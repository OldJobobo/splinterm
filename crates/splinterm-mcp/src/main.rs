use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match splinterm_mcp::run_stdio().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("splinterm-mcp spike: {error:#}");
            ExitCode::FAILURE
        }
    }
}
