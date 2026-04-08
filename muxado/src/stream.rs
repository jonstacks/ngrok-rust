use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            AtomicU8,
            AtomicU32,
            Ordering,
        },
    },
    task::{
        Context,
        Poll,
    },
};

use bytes::Bytes;
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
        ReadBuf,
    },
    sync::mpsc,
};
use tracing::{
    debug,
    trace,
};

use crate::{
    buffer::InboundBuffer,
    error::ErrorCode,
    frame::DataFlags,
    window::FlowWindow,
};

/// Bitmask tracking half-close state.
const HALF_CLOSED_INBOUND: u8 = 0x1;
const HALF_CLOSED_OUTBOUND: u8 = 0x2;
const FULLY_CLOSED: u8 = HALF_CLOSED_INBOUND | HALF_CLOSED_OUTBOUND;

/// Commands sent from a stream to its owning session for frame writes.
#[derive(Debug)]
pub enum StreamCommand {
    SendData {
        stream_id: u32,
        flags: DataFlags,
        data: Bytes,
    },
    SendRst {
        stream_id: u32,
        error_code: ErrorCode,
    },
    SendWndInc {
        stream_id: u32,
        increment: u32,
    },
    /// Notify session that stream is fully closed and can be removed.
    StreamFinished {
        stream_id: u32,
    },
}

/// Shared stream state, wrapped in `Arc` for zero-copy sharing between
/// the session (frame dispatch) and stream (user I/O) sides.
///
/// This design enables FFI-friendly sharing: the `Arc<StreamInner>` can
/// be cheaply passed across boundaries without copying.
pub(crate) struct StreamInner {
    pub id: u32,
    /// Inbound data buffer — session pushes, stream pops.
    pub inbound: InboundBuffer,
    /// Outbound flow control window.
    pub window: FlowWindow,
    /// Channel for sending frame commands to the session writer.
    pub cmd_tx: mpsc::UnboundedSender<StreamCommand>,
    /// Half-close state bitmask.
    pub closed_state: AtomicU8,
    /// Whether SYN has been sent on this stream.
    pub syn_sent: AtomicBool,
    /// Whether this stream has been reset.
    pub reset: AtomicBool,
    /// Error code if reset.
    pub reset_code: AtomicU32,
    /// Max window size for flow control enforcement.
    pub max_window_size: u32,
    /// Current inbound byte count (received but not yet consumed).
    pub inbound_size: AtomicU32,
}

impl StreamInner {
    /// Mark the stream as reset. Closes both inbound buffer and flow window.
    pub fn mark_reset(&self, code: ErrorCode) {
        self.reset.store(true, Ordering::Release);
        self.reset_code.store(code as u32, Ordering::Release);
        self.inbound.close();
        self.window.close();
    }

    /// Mark the inbound (read) side as half-closed.
    pub fn mark_half_closed_inbound(&self) {
        self.closed_state
            .fetch_or(HALF_CLOSED_INBOUND, Ordering::AcqRel);
        self.inbound.close();
    }

    /// Mark the outbound (write) side as half-closed. Returns previous state.
    pub fn mark_half_closed_outbound(&self) -> u8 {
        self.closed_state
            .fetch_or(HALF_CLOSED_OUTBOUND, Ordering::AcqRel)
    }

    /// Returns true if both halves are closed.
    pub fn is_fully_closed(&self) -> bool {
        self.closed_state.load(Ordering::Acquire) == FULLY_CLOSED
    }

    /// Acknowledge `n` bytes consumed from inbound, updating flow control
    /// and sending a WNDINC to the remote peer.
    pub fn ack_bytes(&self, n: u32) {
        self.inbound_size.fetch_sub(n, Ordering::AcqRel);
        let _ = self.cmd_tx.send(StreamCommand::SendWndInc {
            stream_id: self.id,
            increment: n,
        });
    }
}

/// A multiplexed stream over a muxado session.
///
/// Implements `AsyncRead` and `AsyncWrite`. Supports half-close via
/// `close_write()` and forcible reset via `reset()`. When dropped,
/// sends a RST if not fully closed.
///
/// Internally wraps `Arc<StreamInner>` for zero-copy sharing with
/// the session. The `leftover` field is per-instance state for
/// partial reads.
pub struct MuxadoStream {
    pub(crate) inner: Arc<StreamInner>,
    /// Partial chunk leftover from a previous read.
    leftover: Option<Bytes>,
}

impl MuxadoStream {
    /// Returns the stream ID.
    pub fn id(&self) -> u32 {
        self.inner.id
    }

    /// Half-close the write side. The remote will see EOF after reading
    /// all buffered data. Further writes will fail.
    pub fn close_write(&self) {
        let prev = self.inner.mark_half_closed_outbound();
        if prev & HALF_CLOSED_OUTBOUND != 0 {
            return; // Already half-closed outbound
        }

        let mut flags = DataFlags::FIN;
        if !self.inner.syn_sent.swap(true, Ordering::AcqRel) {
            flags = flags.union(DataFlags::SYN);
        }

        let _ = self.inner.cmd_tx.send(StreamCommand::SendData {
            stream_id: self.inner.id,
            flags,
            data: Bytes::new(),
        });

        debug!(stream_id = self.inner.id, "stream write-side closed");
        self.check_fully_closed();
    }

    /// Forcibly reset the stream with an error code.
    ///
    /// Closes both the inbound buffer and the flow window, then sends
    /// a RST frame to the remote peer.
    pub fn reset(&self, code: ErrorCode) {
        self.inner.mark_reset(code);
        let _ = self.inner.cmd_tx.send(StreamCommand::SendRst {
            stream_id: self.inner.id,
            error_code: code,
        });
        let _ = self.inner.cmd_tx.send(StreamCommand::StreamFinished {
            stream_id: self.inner.id,
        });
        debug!(stream_id = self.inner.id, ?code, "stream reset");
    }

    /// Read the next complete frame payload as a zero-copy `Bytes` chunk.
    ///
    /// Unlike `AsyncRead::poll_read`, this returns whole chunks without
    /// copying into a user buffer, making it ideal for FFI or forwarding.
    /// Returns `None` on EOF or reset.
    pub async fn next_frame_payload(&mut self) -> Option<Bytes> {
        // Drain leftover first
        if let Some(bytes) = self.leftover.take()
            && !bytes.is_empty()
        {
            return Some(bytes);
        }

        if self.inner.reset.load(Ordering::Acquire) {
            return None;
        }

        let bytes = self.inner.inbound.pop().await?;
        self.inner.ack_bytes(bytes.len() as u32);
        Some(bytes)
    }

    fn check_fully_closed(&self) {
        if self.inner.is_fully_closed() {
            let _ = self.inner.cmd_tx.send(StreamCommand::StreamFinished {
                stream_id: self.inner.id,
            });
        }
    }
}

impl AsyncRead for MuxadoStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Handle leftover from a previous partial read
        if let Some(ref mut chunk) = self.leftover {
            let to_copy = chunk.len().min(buf.remaining());
            buf.put_slice(&chunk[..to_copy]);
            if to_copy == chunk.len() {
                self.leftover = None;
            } else {
                *chunk = chunk.slice(to_copy..);
            }
            self.inner.ack_bytes(to_copy as u32);
            return Poll::Ready(Ok(()));
        }

        // Poll for next chunk from the inbound buffer
        match self.inner.inbound.poll_pop(cx) {
            Poll::Ready(Some(chunk)) => {
                let to_copy = chunk.len().min(buf.remaining());
                buf.put_slice(&chunk[..to_copy]);
                if to_copy < chunk.len() {
                    self.leftover = Some(chunk.slice(to_copy..));
                }
                self.inner.ack_bytes(to_copy as u32);
                trace!(stream_id = self.inner.id, bytes = to_copy, "stream read");
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => {
                // Buffer closed — check if reset or clean EOF
                if self.inner.reset.load(Ordering::Acquire) {
                    let code = self.inner.reset_code.load(Ordering::Acquire);
                    let code = ErrorCode::from_u32(code).unwrap_or(ErrorCode::InternalError);
                    Poll::Ready(Err(std::io::Error::other(format!("stream reset: {code}"))))
                } else {
                    Poll::Ready(Ok(())) // Clean EOF
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for MuxadoStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        // Check if outbound is closed
        let state = self.inner.closed_state.load(Ordering::Acquire);
        if state & HALF_CLOSED_OUTBOUND != 0 {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stream write-side is closed",
            )));
        }

        if self.inner.reset.load(Ordering::Acquire) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stream reset",
            )));
        }

        // Acquire flow control permits
        match self.inner.window.poll_acquire(cx, buf.len()) {
            Poll::Ready(Ok(amount)) => {
                let data = Bytes::copy_from_slice(&buf[..amount]);

                let mut flags = DataFlags::NONE;
                if !self.inner.syn_sent.swap(true, Ordering::AcqRel) {
                    flags = flags.union(DataFlags::SYN);
                }

                let _ = self.inner.cmd_tx.send(StreamCommand::SendData {
                    stream_id: self.inner.id,
                    flags,
                    data,
                });

                trace!(stream_id = self.inner.id, bytes = amount, "stream write");
                Poll::Ready(Ok(amount))
            }
            Poll::Ready(Err(())) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "session closed",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.close_write();
        Poll::Ready(Ok(()))
    }
}

impl Drop for MuxadoStream {
    fn drop(&mut self) {
        let state = self.inner.closed_state.load(Ordering::Acquire);
        if state != FULLY_CLOSED && !self.inner.reset.load(Ordering::Acquire) {
            let _ = self.inner.cmd_tx.send(StreamCommand::SendRst {
                stream_id: self.inner.id,
                error_code: ErrorCode::StreamCancelled,
            });
            let _ = self.inner.cmd_tx.send(StreamCommand::StreamFinished {
                stream_id: self.inner.id,
            });
        }
    }
}

/// Create a new stream and its shared inner state. Called by the session.
pub(crate) fn new_stream(
    id: u32,
    max_window_size: u32,
    cmd_tx: mpsc::UnboundedSender<StreamCommand>,
    locally_initiated: bool,
) -> (MuxadoStream, Arc<StreamInner>) {
    let inner = Arc::new(StreamInner {
        id,
        inbound: InboundBuffer::new(),
        window: FlowWindow::new(max_window_size),
        cmd_tx,
        closed_state: AtomicU8::new(0),
        syn_sent: AtomicBool::new(!locally_initiated),
        reset: AtomicBool::new(false),
        reset_code: AtomicU32::new(0),
        max_window_size,
        inbound_size: AtomicU32::new(0),
    });

    let stream = MuxadoStream {
        inner: inner.clone(),
        leftover: None,
    };

    (stream, inner)
}

/// Handle inbound data for a stream (called by session frame reader).
pub(crate) fn handle_stream_data(
    inner: &StreamInner,
    data: Bytes,
    flags: DataFlags,
) -> Result<(), ErrorCode> {
    let state = inner.closed_state.load(Ordering::Acquire);
    if state & HALF_CLOSED_INBOUND != 0 {
        return Err(ErrorCode::StreamClosed);
    }

    if !data.is_empty() {
        // Flow control: check that adding this data won't exceed the window
        let current = inner.inbound_size.load(Ordering::Acquire);
        if current as usize + data.len() > inner.max_window_size as usize {
            return Err(ErrorCode::FlowControlError);
        }
        inner
            .inbound_size
            .fetch_add(data.len() as u32, Ordering::AcqRel);
        inner.inbound.push(data);
    }

    if flags.contains(DataFlags::FIN) {
        inner.mark_half_closed_inbound();
    }

    Ok(())
}

/// Handle an inbound RST for a stream.
pub(crate) fn handle_stream_rst(inner: &StreamInner, error_code: u32) {
    let code = ErrorCode::from_u32(error_code).unwrap_or(ErrorCode::InternalError);
    inner.mark_reset(code);
}

/// Handle an inbound WNDINC for a stream.
pub(crate) fn handle_stream_wndinc(inner: &StreamInner, increment: u32) {
    inner.window.add_permits(increment);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_stream(
        locally_initiated: bool,
    ) -> (
        MuxadoStream,
        Arc<StreamInner>,
        mpsc::UnboundedReceiver<StreamCommand>,
    ) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let id = if locally_initiated { 1 } else { 2 };

        let (stream, inner) = new_stream(id, 256 * 1024, cmd_tx, locally_initiated);

        (stream, inner, cmd_rx)
    }

    #[test]
    fn stream_id() {
        let (stream, _, _rx) = make_test_stream(true);
        assert_eq!(stream.id(), 1);
    }

    #[test]
    fn close_write_sends_fin() {
        let (stream, _, mut rx) = make_test_stream(true);
        stream.close_write();

        match rx.try_recv().unwrap() {
            StreamCommand::SendData {
                stream_id,
                flags,
                data,
            } => {
                assert_eq!(stream_id, 1);
                assert!(flags.contains(DataFlags::FIN));
                assert!(flags.contains(DataFlags::SYN));
                assert!(data.is_empty());
            }
            other => panic!("expected SendData, got {other:?}"),
        }
    }

    #[test]
    fn close_write_idempotent() {
        let (stream, _, mut rx) = make_test_stream(true);
        stream.close_write();
        stream.close_write(); // Should be no-op

        let _first = rx.try_recv().unwrap();
        // Should get StreamFinished but no second FIN
        match rx.try_recv() {
            Ok(StreamCommand::StreamFinished { .. }) => {}
            Ok(other) => panic!("expected StreamFinished or nothing, got {other:?}"),
            Err(_) => {}
        }
    }

    #[tokio::test]
    async fn handle_data_delivers_to_buffer() {
        let (mut stream, inner, _rx) = make_test_stream(false);
        handle_stream_data(&inner, Bytes::from("hello"), DataFlags::NONE).unwrap();
        let mut buf = [0u8; 10];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        assert_eq!(&buf[..n], b"hello");
    }

    #[tokio::test]
    async fn handle_data_with_fin_sets_eof() {
        let (mut stream, inner, _rx) = make_test_stream(false);
        handle_stream_data(&inner, Bytes::from("bye"), DataFlags::FIN).unwrap();

        let mut buf = [0u8; 10];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        assert_eq!(&buf[..n], b"bye");
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap();
        assert_eq!(n, 0); // EOF
    }

    #[test]
    fn handle_data_on_closed_inbound_returns_error() {
        let (_stream, inner, _rx) = make_test_stream(false);
        handle_stream_data(&inner, Bytes::new(), DataFlags::FIN).unwrap();
        let result = handle_stream_data(&inner, Bytes::from("more"), DataFlags::NONE);
        assert_eq!(result, Err(ErrorCode::StreamClosed));
    }

    #[test]
    fn handle_data_overflow_returns_flow_control_error() {
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let (_stream, inner) = new_stream(1, 5, cmd_tx, false);

        let result = handle_stream_data(&inner, Bytes::from("toolong"), DataFlags::NONE);
        assert_eq!(result, Err(ErrorCode::FlowControlError));
    }

    #[test]
    fn handle_rst_closes_stream() {
        let (_stream, inner, _rx) = make_test_stream(false);
        handle_stream_rst(&inner, ErrorCode::StreamReset as u32);
        assert!(inner.reset.load(Ordering::Acquire));
    }

    #[test]
    fn reset_sends_rst_and_finished() {
        let (stream, _, mut rx) = make_test_stream(true);
        stream.reset(ErrorCode::StreamReset);

        match rx.try_recv().unwrap() {
            StreamCommand::SendRst {
                stream_id,
                error_code,
            } => {
                assert_eq!(stream_id, 1);
                assert_eq!(error_code, ErrorCode::StreamReset);
            }
            other => panic!("expected SendRst, got {other:?}"),
        }
        match rx.try_recv().unwrap() {
            StreamCommand::StreamFinished { stream_id } => {
                assert_eq!(stream_id, 1);
            }
            other => panic!("expected StreamFinished, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn next_frame_payload_returns_chunks() {
        let (mut stream, inner, _rx) = make_test_stream(false);
        handle_stream_data(&inner, Bytes::from("chunk1"), DataFlags::NONE).unwrap();
        handle_stream_data(&inner, Bytes::from("chunk2"), DataFlags::NONE).unwrap();

        let c1 = stream.next_frame_payload().await.unwrap();
        assert_eq!(c1, Bytes::from("chunk1"));
        let c2 = stream.next_frame_payload().await.unwrap();
        assert_eq!(c2, Bytes::from("chunk2"));
    }

    #[tokio::test]
    async fn next_frame_payload_returns_none_on_eof() {
        let (mut stream, inner, _rx) = make_test_stream(false);
        handle_stream_data(&inner, Bytes::from("last"), DataFlags::FIN).unwrap();

        let c = stream.next_frame_payload().await.unwrap();
        assert_eq!(c, Bytes::from("last"));
        assert!(stream.next_frame_payload().await.is_none());
    }

    #[tokio::test]
    async fn next_frame_payload_returns_none_on_reset() {
        let (mut stream, inner, _rx) = make_test_stream(false);
        handle_stream_rst(&inner, ErrorCode::StreamReset as u32);
        assert!(stream.next_frame_payload().await.is_none());
    }

    #[test]
    fn drop_sends_rst_if_not_closed() {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
        {
            let (stream, _inner) = new_stream(5, 256 * 1024, cmd_tx, true);
            drop(stream);
        }
        match cmd_rx.try_recv().unwrap() {
            StreamCommand::SendRst {
                stream_id,
                error_code,
            } => {
                assert_eq!(stream_id, 5);
                assert_eq!(error_code, ErrorCode::StreamCancelled);
            }
            other => panic!("expected SendRst, got {other:?}"),
        }
    }

    #[test]
    fn drop_does_not_send_rst_after_reset() {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
        {
            let (stream, _inner) = new_stream(5, 256 * 1024, cmd_tx, true);
            stream.reset(ErrorCode::StreamReset);
            // Drain the RST + StreamFinished from reset()
            let _ = cmd_rx.try_recv();
            let _ = cmd_rx.try_recv();
            drop(stream);
        }
        // Should NOT get another RST from drop
        assert!(cmd_rx.try_recv().is_err());
    }
}
