use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

use rmcp::model::ClientJsonRpcMessage;
use serde::{
    Deserialize, Deserializer,
    de::{Error as _, IgnoredAny, MapAccess, Visitor},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::sync::CancellationToken;

use crate::limits::MAXIMUM_LINE_BYTES;

/// Shared fail-closed state for the stdio reader, writer, and service.
#[derive(Clone)]
pub(crate) struct TransportFailure {
    failed: Arc<AtomicBool>,
    failure_signal: CancellationToken,
    cancellation: CancellationToken,
}

impl TransportFailure {
    pub(crate) fn new(cancellation: CancellationToken) -> Self {
        Self {
            failed: Arc::new(AtomicBool::new(false)),
            failure_signal: CancellationToken::new(),
            cancellation,
        }
    }

    fn cancel(&self) {
        self.cancellation.cancel();
    }

    fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.failure_signal.cancel();
        self.cancel();
    }

    pub(crate) async fn failure_cancelled(&self) {
        self.failure_signal.cancelled().await;
    }

    pub(crate) fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

/// An async reader that buffers and validates one complete bounded NDJSON frame.
///
/// The fixed 256 KiB allocation is made up front. A frame is not exposed to
/// rmcp until its newline has been observed and the complete line has parsed as
/// an MCP JSON-RPC message. A full buffer without a newline is rejected before
/// another byte is read or allocated.
pub struct BoundedLineReader<R> {
    inner: R,
    buffer: Box<[u8]>,
    start: usize,
    end: usize,
    ready_end: Option<usize>,
    eof: bool,
    failed: bool,
    transport_failure: Option<TransportFailure>,
}

impl<R> BoundedLineReader<R> {
    #[must_use]
    pub fn new(inner: R) -> Self {
        Self::with_optional_failure(inner, None)
    }

    pub(crate) fn with_failure(inner: R, failure: TransportFailure) -> Self {
        Self::with_optional_failure(inner, Some(failure))
    }

    fn with_optional_failure(inner: R, transport_failure: Option<TransportFailure>) -> Self {
        Self {
            inner,
            buffer: vec![0; MAXIMUM_LINE_BYTES].into_boxed_slice(),
            start: 0,
            end: 0,
            ready_end: None,
            eof: false,
            failed: false,
            transport_failure,
        }
    }

    fn fail(&mut self, message: &'static str) -> Poll<io::Result<()>> {
        self.failed = true;
        if let Some(failure) = &self.transport_failure {
            failure.fail();
        }
        Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, message)))
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for BoundedLineReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        destination: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.failed {
            return Poll::Ready(Ok(()));
        }

        loop {
            if let Some(ready_end) = this.ready_end {
                if this.start < ready_end {
                    let count = (ready_end - this.start).min(destination.remaining());
                    destination.put_slice(&this.buffer[this.start..this.start + count]);
                    this.start += count;
                    return Poll::Ready(Ok(()));
                }

                this.ready_end = None;
                if this.start == this.end {
                    this.start = 0;
                    this.end = 0;
                } else {
                    this.buffer.copy_within(this.start..this.end, 0);
                    this.end -= this.start;
                    this.start = 0;
                }
            }

            if let Some(offset) = this.buffer[this.start..this.end]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                let ready_end = this.start + offset + 1;
                let mut line = &this.buffer[this.start..ready_end - 1];
                if let Some(without_carriage_return) = line.strip_suffix(b"\r") {
                    line = without_carriage_return;
                }
                if validate_json_rpc_line(line).is_err() {
                    return this.fail("inbound line is not a valid MCP JSON-RPC message");
                }
                this.ready_end = Some(ready_end);
                continue;
            }

            if this.end == MAXIMUM_LINE_BYTES {
                return this.fail("inbound MCP line exceeds 256 KiB");
            }

            if this.eof {
                // MCP stdio requires newline-delimited messages. A nonempty
                // trailing frame is malformed and must terminate fail closed.
                if this.start != this.end {
                    return this.fail("inbound MCP line is missing its newline");
                }
                return Poll::Ready(Ok(()));
            }

            let before = this.end;
            let poll = {
                let mut read_buffer = ReadBuf::new(&mut this.buffer[this.end..]);
                match Pin::new(&mut this.inner).poll_read(context, &mut read_buffer) {
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buffer.filled().len())),
                    Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                    Poll::Pending => Poll::Pending,
                }
            };
            match poll {
                Poll::Ready(Ok(0)) => {
                    this.eof = true;
                    // Cancel the service root before reporting EOF. rmcp drains
                    // handler responses after its receive loop ends, so
                    // cancellation-aware daemon calls can finish promptly and
                    // still return their valid final responses during drain.
                    if let Some(failure) = &this.transport_failure {
                        failure.cancel();
                    }
                }
                Poll::Ready(Ok(count)) => this.end = before + count,
                Poll::Ready(Err(error)) => {
                    this.failed = true;
                    if let Some(failure) = &this.transport_failure {
                        failure.fail();
                    }
                    return Poll::Ready(Err(error));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// An output wrapper that turns any write, flush, or shutdown failure into
/// process-wide service cancellation.
pub(crate) struct FailClosedWriter<W> {
    inner: W,
    failure: TransportFailure,
}

impl<W> FailClosedWriter<W> {
    pub(crate) fn new(inner: W, failure: TransportFailure) -> Self {
        Self { inner, failure }
    }

    fn observe<T>(&self, poll: &Poll<io::Result<T>>) {
        if matches!(poll, Poll::Ready(Err(_))) {
            self.failure.fail();
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for FailClosedWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(context, buffer);
        this.observe(&result);
        result
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_flush(context);
        this.observe(&result);
        result
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_shutdown(context);
        this.observe(&result);
        result
    }
}

fn validate_json_rpc_line(line: &[u8]) -> Result<(), serde_json::Error> {
    let value: serde_json::Value = serde_json::from_slice(line)?;
    if value.get("method").and_then(serde_json::Value::as_str) == Some("initialize") {
        // Parse the raw initialize shape before rmcp's optional capability
        // fields can turn unsupported `null` values or ignored keys into None.
        serde_json::from_slice::<RawInitializeRequest>(line)?;
    }
    serde_json::from_value::<ClientJsonRpcMessage>(value).map(|_| ())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInitializeRequest {
    #[serde(rename = "jsonrpc")]
    _json_rpc: String,
    #[serde(rename = "id")]
    _id: serde_json::Value,
    #[serde(rename = "method")]
    _method: String,
    #[serde(rename = "params")]
    _params: RawInitializeParams,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInitializeParams {
    #[serde(rename = "_meta", default)]
    _meta: Option<serde_json::Value>,
    #[serde(rename = "protocolVersion")]
    _protocol_version: String,
    #[serde(rename = "capabilities")]
    _capabilities: RawClientCapabilities,
    #[serde(rename = "clientInfo")]
    _client_info: serde_json::Value,
}

struct RawClientCapabilities;

impl<'de> Deserialize<'de> for RawClientCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EmptyCapabilitiesVisitor;

        impl<'de> Visitor<'de> for EmptyCapabilitiesVisitor {
            type Value = RawClientCapabilities;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an empty client capabilities object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                if let Some(key) = map.next_key::<String>()? {
                    let _ = map.next_value::<IgnoredAny>()?;
                    return Err(A::Error::custom(format_args!(
                        "unsupported client capability {key:?}"
                    )));
                }
                Ok(RawClientCapabilities)
            }
        }

        deserializer.deserialize_map(EmptyCapabilitiesVisitor)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn oversized_unterminated_line_exposes_no_partial_bytes() {
        let (mut writer, reader) = tokio::io::duplex(4096);
        let write = tokio::spawn(async move {
            writer
                .write_all(&vec![b'x'; MAXIMUM_LINE_BYTES])
                .await
                .unwrap();
        });
        let mut bounded = BoundedLineReader::new(reader);
        let mut exposed = Vec::new();
        let error = bounded
            .read_to_end(&mut exposed)
            .await
            .expect_err("a full buffer without newline must fail");
        write.await.unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(exposed.is_empty());
    }

    #[tokio::test]
    async fn complete_malformed_or_non_rpc_lines_expose_no_bytes() {
        for line in [b"not-json\n".as_slice(), b"{}\n", b"[]\n"] {
            let mut bounded = BoundedLineReader::new(line);
            let mut exposed = Vec::new();
            assert!(bounded.read_to_end(&mut exposed).await.is_err());
            assert!(exposed.is_empty());
        }
    }

    #[test]
    fn raw_initialize_capabilities_allow_exactly_an_empty_object() {
        let valid = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}
            }
        });
        assert!(validate_json_rpc_line(valid.to_string().as_bytes()).is_ok());

        for capabilities in [
            json!(null),
            json!([]),
            json!({"sampling": null}),
            json!({"sampling": {}}),
            json!({"unknown": null}),
        ] {
            let mut invalid = valid.clone();
            invalid["params"]["capabilities"] = capabilities;
            assert!(validate_json_rpc_line(invalid.to_string().as_bytes()).is_err());
        }
        assert!(
            validate_json_rpc_line(
                br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"sampling":null,"sampling":{}},"clientInfo":{"name":"test","version":"1"}}}"#
            )
            .is_err()
        );
    }
}
