use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, ReadBuf};

/// Maximum inbound newline-delimited MCP frame, including its newline.
pub const MAXIMUM_LINE_BYTES: usize = 256 * 1024;

/// An async reader that buffers one complete, bounded newline-delimited frame.
///
/// The fixed 256 KiB allocation is made up front. A frame is not exposed to
/// rmcp until its newline has been observed, and a full buffer without a
/// newline is rejected before another byte is read or allocated.
pub struct BoundedLineReader<R> {
    inner: R,
    buffer: Box<[u8]>,
    start: usize,
    end: usize,
    ready_end: Option<usize>,
    eof: bool,
    failed: bool,
}

impl<R> BoundedLineReader<R> {
    #[must_use]
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: vec![0; MAXIMUM_LINE_BYTES].into_boxed_slice(),
            start: 0,
            end: 0,
            ready_end: None,
            eof: false,
            failed: false,
        }
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
                this.ready_end = Some(this.start + offset + 1);
                continue;
            }

            if this.end == MAXIMUM_LINE_BYTES {
                this.failed = true;
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "inbound MCP line exceeds 256 KiB",
                )));
            }

            if this.eof {
                // MCP stdio requires newline-delimited messages. Discard an
                // incomplete trailing frame and report clean EOF.
                this.start = 0;
                this.end = 0;
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
                Poll::Ready(Ok(0)) => this.eof = true,
                Poll::Ready(Ok(count)) => this.end = before + count,
                Poll::Ready(Err(error)) => {
                    this.failed = true;
                    return Poll::Ready(Err(error));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
