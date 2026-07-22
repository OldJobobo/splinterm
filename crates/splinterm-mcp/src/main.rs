use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    if splinterm_mcp::run_stdio().await.is_ok() {
        ExitCode::SUCCESS
    } else {
        eprintln!("splinterm-mcp: bounded stdio service failed");
        // Tokio's stdin helper owns a blocking read thread that cannot be
        // cancelled while the peer keeps stdin open. The service and request
        // tokens are already cancelled; do not hang during runtime shutdown.
        std::process::exit(1);
    }
}
