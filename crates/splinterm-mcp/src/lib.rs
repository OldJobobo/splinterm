#![forbid(unsafe_code)]

//! Bounded stdio MCP adapter for Splinterm.
//!
//! This slice provides bounded lifecycle/discovery plus the reviewed metadata,
//! authorization-status, revocation, and audit tools. Later tool/resource slices
//! remain fail closed.

mod dispatch;
mod dto;
mod limits;
mod server;
mod tools;
mod transport;

pub use limits::{
    MAXIMUM_ACTIVE_REQUESTS, MAXIMUM_ADMITTED_REQUESTS, MAXIMUM_LINE_BYTES,
    MAXIMUM_TOOL_RESPONSE_BYTES,
};
pub use server::SplintermServer;
pub use transport::BoundedLineReader;

/// Runs the MCP server over bounded stdin and stdout.
///
/// # Errors
///
/// Returns an error when MCP initialization, input framing, output, or service
/// shutdown fails.
pub async fn run_stdio() -> anyhow::Result<()> {
    use rmcp::ServiceExt as _;
    use tokio_util::sync::CancellationToken;

    let cancellation = CancellationToken::new();
    let failure = transport::TransportFailure::new(cancellation.clone());
    let input = BoundedLineReader::with_failure(tokio::io::stdin(), failure.clone());
    let output = transport::FailClosedWriter::new(tokio::io::stdout(), failure.clone());
    let service = SplintermServer::new()
        .with_admission()
        .serve_with_ct((input, output), cancellation)
        .await?;
    tokio::select! {
        result = service.waiting() => {
            if failure.has_failed() {
                anyhow::bail!("stdio transport failed closed");
            }
            result?;
            Ok(())
        }
        () = failure.failure_cancelled() => {
            anyhow::bail!("stdio transport failed closed")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use rmcp::{
        ErrorData, RoleServer, ServerHandler, ServiceExt as _,
        model::{InitializeResult, ServerCapabilities},
        service::RequestContext,
    };
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
        sync::Notify,
    };
    use tokio_util::sync::CancellationToken;

    use super::*;

    #[derive(Debug, Clone)]
    struct EofCancellationServer {
        handler_started: Arc<Notify>,
        cancellation_observed: Arc<Notify>,
    }

    impl ServerHandler for EofCancellationServer {
        async fn ping(&self, context: RequestContext<RoleServer>) -> Result<(), ErrorData> {
            self.handler_started.notify_one();
            context.ct.cancelled().await;
            self.cancellation_observed.notify_one();
            Ok(())
        }

        fn get_info(&self) -> InitializeResult {
            InitializeResult::new(ServerCapabilities::default())
        }
    }

    async fn read_message(reader: &mut BufReader<tokio::io::DuplexStream>) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line))
            .await
            .expect("mock service response timed out")
            .expect("mock service response read failed");
        serde_json::from_str(&line).expect("mock service response was not JSON")
    }

    #[tokio::test]
    async fn stdin_eof_cancels_inflight_handler_before_draining_its_response() {
        let (mut client_input, server_input) = tokio::io::duplex(4096);
        let (server_output, client_output) = tokio::io::duplex(4096);
        let mut client_output = BufReader::new(client_output);
        let cancellation = CancellationToken::new();
        let failure = transport::TransportFailure::new(cancellation.clone());
        let input = BoundedLineReader::with_failure(server_input, failure);
        let server = EofCancellationServer {
            handler_started: Arc::new(Notify::new()),
            cancellation_observed: Arc::new(Notify::new()),
        };
        let probe = server.clone();
        let task = tokio::spawn(async move {
            server
                .serve_with_ct((input, server_output), cancellation)
                .await
                .expect("mock service initialization failed")
                .waiting()
                .await
                .expect("mock service task failed");
        });

        client_input
            .write_all(
                format!(
                    "{}\n",
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "initialize",
                        "params": {
                            "protocolVersion": "2025-11-25",
                            "capabilities": {},
                            "clientInfo": {"name": "eof-test", "version": "1"}
                        }
                    })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(read_message(&mut client_output).await["id"], 1);

        client_input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), probe.handler_started.notified())
            .await
            .expect("long-running handler did not start");

        drop(client_input);
        tokio::time::timeout(
            Duration::from_millis(250),
            probe.cancellation_observed.notified(),
        )
        .await
        .expect("stdin EOF did not promptly cancel the in-flight request token");

        let response = read_message(&mut client_output).await;
        assert_eq!(response["id"], 2);
        assert_eq!(response["result"], json!({}));
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("mock service did not finish after EOF")
            .expect("mock service join failed");
    }
}
