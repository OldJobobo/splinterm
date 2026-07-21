#![forbid(unsafe_code)]

//! Non-shipping MCP SDK and protocol spike.
//!
//! This crate deliberately contains no daemon client, production tools, network
//! transport, or packaging integration.

mod server;
mod transport;

pub use server::SpikeServer;
pub use transport::{BoundedLineReader, MAXIMUM_LINE_BYTES};

/// Runs the spike server over bounded stdin and stdout.
///
/// # Errors
///
/// Returns an error when MCP initialization or the stdio service fails.
pub async fn run_stdio() -> anyhow::Result<()> {
    use rmcp::ServiceExt as _;

    let input = BoundedLineReader::new(tokio::io::stdin());
    let service = SpikeServer::new()
        .serve((input, tokio::io::stdout()))
        .await?;
    service.waiting().await?;
    Ok(())
}
