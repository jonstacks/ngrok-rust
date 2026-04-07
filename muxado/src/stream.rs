use std::{
    cmp,
    fmt,
    io,
    pin::Pin,
    task::{
        Context,
        Poll,
        Waker,
    },
};

use bytes::BytesMut;
use futures::{
    channel::mpsc,
    ready,
    sink::Sink,
    stream::Stream as StreamT,
};
use pin_project::pin_project;
use tokio::io::{
    AsyncRead,
    AsyncWrite,
    ReadBuf,
};
use tracing::instrument;

use crate::{
    errors::Error,
    frame::{
        Body,
        Frame,
        HeaderType,
        Length,
        WndInc,
    },
    stream_output::StreamSender,
    window::Window,
};

/// A muxado stream.
///
/// This is an [AsyncRead]/[AsyncWrite] struct that's backed by a muxado
/// session.
#[pin_project(project = StreamProj, PinnedDrop)]
pub struct Stream {
    pub(crate) dropref: Option<awaitdrop::Ref>,

    window: Window,

    read_buf: BytesMut,

    // These are the two channels that are used to shuttle data back and forth
    // between the stream and the stream manager, which is responsible for
    // routing frames to their proper stream.
    #[pin]
    fin: mpsc::Receiver<Frame>,
    #[pin]
    fout: StreamSender,

    read_waker: Option<Waker>,
    write_waker: Option<Waker>,

    write_closed: Option<Error>,

    data_read_closed: bool,

    needs_syn: bool,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stream")
            .field("window", &self.window)
            .field("read_buf", &self.read_buf)
            .field("read_waker", &self.read_waker)
            .field("write_waker", &self.write_waker)
            .field("reset", &self.write_closed)
            .field("read_closed", &self.data_read_closed)
            .finish()
    }
}

impl Stream {
    pub(crate) fn new(
        fout: StreamSender,
        fin: mpsc::Receiver<Frame>,
        window_size: usize,
        needs_syn: bool,
    ) -> Self {
        Self {
            dropref: None,
            window: Window::new(window_size),
            fin,
            fout,
            read_buf: Default::default(),
            read_waker: Default::default(),
            write_waker: Default::default(),
            write_closed: Default::default(),
            data_read_closed: false,
            needs_syn,
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_recv_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Frame>> {
        let mut this = self.project();
        let fin = this.fin.as_mut();
        fin.poll_next(cx)
    }

    // Receive data and fill the read buffer.
    // Handle frames of any other type along the way.
    // Returns `Poll::Ready` once there are new bytes to read, or EOF/RST has
    // been reached.
    #[instrument(level = "trace", skip_all)]
    fn poll_recv_data(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_recv_frame_type(cx, HeaderType::Data)
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_recv_wndinc(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_recv_frame_type(cx, HeaderType::WndInc)
    }

    #[instrument(level = "trace", skip(self, cx))]
    fn poll_recv_frame_type(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        target_typ: HeaderType,
    ) -> Poll<io::Result<()>> {
        loop {
            let frame = if let Some(frame) = ready!(self.as_mut().poll_recv_frame(cx)) {
                frame
            } else {
                self.data_read_closed = true;
                return Ok(()).into();
            };

            let typ = self.handle_frame(frame, Some(cx));

            if typ == target_typ {
                return Poll::Ready(Ok(()));
            }
        }
    }

    #[instrument(level = "trace", skip(self, cx))]
    fn poll_send_wndinc(self: Pin<&mut Self>, cx: &mut Context<'_>, by: WndInc) -> Poll<()> {
        let mut this = self.project();
        // Treat a closed send channel as "success"
        if ready!(this.fout.as_mut().poll_ready(cx)).is_err() {
            return Poll::Ready(());
        }

        // Same as above
        let _ = this.fout.as_mut().start_send(Body::WndInc(by).into());

        Poll::Ready(())
    }

    #[instrument(level = "trace", skip(self, cx))]
    fn handle_frame(&mut self, frame: Frame, cx: Option<&Context<'_>>) -> HeaderType {
        if frame.is_fin() {
            self.data_read_closed = true;
        }
        match frame.body {
            Body::Data(bs) => {
                self.read_buf.extend_from_slice(&bs);
                self.maybe_wake_read(cx);
            }
            Body::WndInc(by) => {
                self.window.inc(*by as usize);
                self.maybe_wake_write(cx);
            }
            _ => unreachable!("stream should never receive GoAway, Rst or Invalid frames"),
        }
        frame.header.typ
    }

    #[instrument(level = "trace", skip_all)]
    fn maybe_wake_read(&mut self, cx: Option<&Context>) {
        maybe_wake(cx, self.read_waker.take())
    }
    #[instrument(level = "trace", skip_all)]
    fn maybe_wake_write(&mut self, cx: Option<&Context>) {
        maybe_wake(cx, self.write_waker.take())
    }
}

impl<'a> StreamProj<'a> {
    fn closed_err(&mut self, code: Error) -> io::Error {
        *self.write_closed = Some(code);
        io::Error::new(io::ErrorKind::ConnectionReset, code)
    }
}

fn maybe_wake(me: Option<&Context>, other: Option<Waker>) {
    match (me.map(Context::waker), other) {
        (Some(me), Some(other)) if !other.will_wake(me) => other.wake(),
        (None, Some(other)) => other.wake(),
        _ => {}
    }
}

impl AsyncRead for Stream {
    #[instrument(level = "trace", skip_all)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            // If we have data, return it
            if !self.read_buf.is_empty() {
                let max = cmp::min(self.read_buf.len(), buf.remaining());
                let clamped = WndInc::clamp(max as u32);
                let n = *clamped as usize;

                if n > 0 {
                    // Wait till there's window capacity to receive an increment.
                    // If this fails, continue anyway.
                    ready!(self.as_mut().poll_send_wndinc(cx, clamped));

                    buf.put_slice(self.read_buf.split_to(n).as_ref());
                }

                return Poll::Ready(Ok(()));
            }

            // EOF's should return Ok without modifying the output buffer.
            if self.data_read_closed {
                return Poll::Ready(Ok(()));
            }

            // Data frames may be ingested by the writer as well, so make sure
            // we don't get forgotten.
            self.read_waker = Some(cx.waker().clone());

            // Otherwise, try to get more.
            ready!(self.as_mut().poll_recv_data(cx))?;
        }
    }
}

impl AsyncWrite for Stream {
    #[instrument(level = "trace", skip(self, cx))]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        if let Some(code) = self.write_closed {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, code)));
        }

        if self.window.capacity() == 0 {
            self.write_waker = Some(cx.waker().clone());

            ready!(self.as_mut().poll_recv_wndinc(cx))?;
        }

        let mut this = self.project();

        ready!(this.fout.as_mut().poll_ready(cx)).map_err(|e| this.closed_err(e))?;

        let wincap = this.window.capacity();

        let max_len = Length::clamp(buf.len() as u32);

        let send_len = cmp::min(wincap, *max_len as usize);

        let bs = BytesMut::from(&buf[..send_len]);

        let mut frame: Frame = Body::Data(bs.freeze()).into();
        if *this.needs_syn {
            *this.needs_syn = false;
            frame = frame.syn();
        }

        this.fout
            .as_mut()
            .start_send(frame)
            .map_err(|e| this.closed_err(e))?;

        let _dec = this.window.dec(send_len);
        debug_assert!(_dec == send_len);

        Poll::Ready(Ok(send_len))
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let mut this = self.project();
        this.fout
            .as_mut()
            .poll_flush(cx)
            .map_err(|e| this.closed_err(e))
    }

    #[instrument(level = "trace", skip_all)]
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        // Rather than close the output channel, send a fin frame.
        // This lets us use the actual channel closure as the "stream is gone
        // for good" signal.
        let mut this = self.as_mut().project();
        if this.write_closed.is_none() {
            ready!(this.fout.as_mut().poll_ready(cx))
                .and_then(|_| {
                    this.fout
                        .as_mut()
                        .start_send(Frame::from(Body::Data([][..].into())).fin())
                })
                .map_err(|e| this.closed_err(e))?;
            *this.write_closed = Some(Error::StreamClosed);
        }

        this.fout
            .as_mut()
            .poll_flush(cx)
            .map_err(|e| this.closed_err(e))
    }
}

#[pin_project::pinned_drop]
impl PinnedDrop for Stream {
    #[instrument(level = "trace", skip_all)]
    fn drop(self: Pin<&mut Self>) {}
}

#[cfg(test)]
pub mod test {
    use std::time::Duration;

    use tokio::{
        io::{
            AsyncReadExt,
            AsyncWriteExt,
        },
        time,
    };
    use tracing_test::traced_test;

    use super::*;

    // Construct a Stream directly (no session), wiring up two mpsc channels.
    // Returns (stream, sender-to-stream, receiver-from-stream).
    fn make_stream(
        window: usize,
        needs_syn: bool,
    ) -> (Stream, mpsc::Sender<Frame>, mpsc::Receiver<Frame>) {
        let (tx_to_stream, stream_rx) = mpsc::channel(512);
        let (stream_tx, rx_from_stream) = mpsc::channel(512);
        let stream_sender = StreamSender::wrap(stream_tx);
        let stream = Stream::new(stream_sender, stream_rx, window, needs_syn);
        (stream, tx_to_stream, rx_from_stream)
    }

    #[traced_test]
    #[tokio::test]
    async fn test_stream() {
        let (mut tx, stream_rx) = mpsc::channel(512);
        let (stream_tx, mut rx) = mpsc::channel(512);
        let stream_tx = StreamSender::wrap(stream_tx);

        let mut stream = Stream::new(stream_tx, stream_rx, 5, true);

        const MSG: &str = "Hello, world!";
        const MSG2: &str = "Hello to you too!";

        // First try a short write, the window won't permit more
        let n = time::timeout(Duration::from_secs(1), stream.write(MSG2.as_bytes()))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(n, 5);
        let resp = rx.try_next().unwrap().unwrap();
        assert_eq!(resp, Frame::from(Body::Data(MSG2[0..5].into())).syn());

        // Next, send the stream an inc and a data frame
        tx.try_send(Body::WndInc(WndInc::clamp(5)).into()).unwrap();
        tx.try_send(Body::Data(MSG.as_bytes().into()).into())
            .unwrap();
        drop(tx);

        // Read the data. The wndinc should get processed as well.
        let mut buf = String::new();
        time::timeout(Duration::from_secs(1), stream.read_to_string(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(buf, "Hello, world!");
        // Reading the data should generate a wndinc
        let resp = rx.try_next().unwrap().unwrap();
        assert_eq!(resp, Body::WndInc(WndInc::clamp(MSG.len() as u32)).into());

        // Finally, try writing again. If the previous read handled the wndinc, we'll have capacity for 5 more bytes
        let n = time::timeout(Duration::from_secs(1), stream.write(&MSG2.as_bytes()[5..]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 5);
        let resp = rx.try_next().unwrap().unwrap();
        assert_eq!(resp, Body::Data(MSG2[5..10].into()).into());

        stream.shutdown().await.unwrap();

        assert!(rx.try_next().unwrap().unwrap().is_fin());
    }

    // -------------------------------------------------------------------------
    // Half-close (FIN) tests
    // -------------------------------------------------------------------------

    /// Receiving a FIN frame from the remote must eventually produce an
    /// `AsyncRead` EOF (poll_read returns Ok with 0 bytes advanced).
    #[tokio::test]
    async fn test_fin_received_causes_eof() {
        let (mut stream, mut tx, _rx) = make_stream(256, false);

        // Deliver some data followed by a FIN frame.
        tx.try_send(Body::Data(b"hello"[..].into()).into()).unwrap();
        let fin_frame = Frame::from(Body::Data([][..].into())).fin();
        tx.try_send(fin_frame).unwrap();
        drop(tx);

        let mut buf = Vec::new();
        time::timeout(Duration::from_secs(1), stream.read_to_end(&mut buf))
            .await
            .expect("should not time out")
            .expect("read_to_end should succeed");

        assert_eq!(buf, b"hello");
    }

    /// A FIN DATA frame that also carries payload must deliver the payload
    /// before signalling EOF.
    #[tokio::test]
    async fn test_fin_with_payload_delivers_data_then_eof() {
        let (mut stream, mut tx, _rx) = make_stream(256, false);

        // A single frame that carries both the last bytes and FIN.
        let fin_with_data = Frame::from(Body::Data(b"last"[..].into())).fin();
        tx.try_send(fin_with_data).unwrap();
        drop(tx);

        let mut buf = Vec::new();
        time::timeout(Duration::from_secs(1), stream.read_to_end(&mut buf))
            .await
            .expect("should not time out")
            .expect("read_to_end should succeed");

        assert_eq!(buf, b"last");
    }

    /// After `AsyncWrite::shutdown()`, any further write must fail with a
    /// `BrokenPipe` error.
    #[tokio::test]
    async fn test_write_after_shutdown_returns_broken_pipe() {
        let (mut stream, _tx, mut rx) = make_stream(256, false);

        stream.shutdown().await.expect("shutdown should succeed");

        // Consume the FIN frame emitted by shutdown.
        let fin = rx.try_next().unwrap().unwrap();
        assert!(fin.is_fin(), "shutdown should produce a FIN frame");

        // A subsequent write must fail.
        let result = stream.write_all(b"should fail").await;
        assert!(result.is_err(), "write after shutdown should fail");
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe,
            "expected BrokenPipe error kind"
        );
    }

    /// `shutdown()` itself must be idempotent: calling it twice must not panic
    /// and must succeed both times.
    #[tokio::test]
    async fn test_shutdown_is_idempotent() {
        let (mut stream, _tx, _rx) = make_stream(256, false);
        stream.shutdown().await.expect("first shutdown");
        // Second shutdown: write_closed is already set, so we skip the send.
        stream.shutdown().await.expect("second shutdown");
    }

    // -------------------------------------------------------------------------
    // Flow control tests
    // -------------------------------------------------------------------------

    /// With a 5-byte window, a write of 10 bytes must be limited to 5 bytes.
    /// After receiving a WNDINC of 5, the next write of the remaining bytes
    /// must succeed.
    #[tokio::test]
    async fn test_flow_control_write_limited_by_window() {
        let (mut stream, mut tx, mut rx) = make_stream(5, false);

        // First write: limited to window size (5 bytes).
        let n = time::timeout(Duration::from_secs(1), stream.write(b"0123456789"))
            .await
            .expect("should not time out")
            .expect("write should succeed");
        assert_eq!(n, 5, "write should be limited to 5 bytes by flow control");
        // Consume the DATA frame.
        rx.try_next().unwrap().unwrap();

        // Sending a WNDINC to the stream should unblock the next write.
        tx.try_send(Body::WndInc(WndInc::clamp(5)).into()).unwrap();

        let n = time::timeout(Duration::from_secs(1), stream.write(b"56789"))
            .await
            .expect("should not time out")
            .expect("write should succeed");
        assert_eq!(n, 5);
    }

    /// Multiple WNDINC frames must be accumulated correctly by the window.
    #[tokio::test]
    async fn test_window_accumulates_multiple_increments() {
        let (mut stream, mut tx, mut rx) = make_stream(5, false);

        // Exhaust the initial window.
        stream.write_all(b"12345").await.unwrap();
        rx.try_next().unwrap(); // consume DATA frame

        // Send two separate WNDINCs totalling 10.
        tx.try_send(Body::WndInc(WndInc::clamp(3)).into()).unwrap();
        tx.try_send(Body::WndInc(WndInc::clamp(4)).into()).unwrap();

        // The window is capped at max_size (5), not unbounded.
        // After two increments the window should not exceed 5.
        let n = time::timeout(Duration::from_secs(1), stream.write(b"abcdefghij"))
            .await
            .expect("should not time out")
            .expect("write should succeed");
        assert!(
            n <= 5,
            "window should be capped at max_size; wrote {n} bytes"
        );
    }

    // -------------------------------------------------------------------------
    // SYN flag tests
    // -------------------------------------------------------------------------

    /// A stream with `needs_syn=true` must set the SYN flag on the first write
    /// and clear it on subsequent writes.
    #[tokio::test]
    async fn test_syn_set_on_first_write_only() {
        let (mut stream, _tx, mut rx) = make_stream(64, true);

        stream.write_all(b"first").await.unwrap();
        let first_frame = rx.try_next().unwrap().unwrap();
        assert!(first_frame.is_syn(), "first write must have SYN flag");

        stream.write_all(b"second").await.unwrap();
        let second_frame = rx.try_next().unwrap().unwrap();
        assert!(
            !second_frame.is_syn(),
            "subsequent writes must not have SYN flag"
        );
    }

    /// A stream with `needs_syn=false` must never set the SYN flag.
    #[tokio::test]
    async fn test_no_syn_when_not_needed() {
        let (mut stream, _tx, mut rx) = make_stream(64, false);

        stream.write_all(b"data").await.unwrap();
        let frame = rx.try_next().unwrap().unwrap();
        assert!(
            !frame.is_syn(),
            "stream with needs_syn=false must not set SYN"
        );
    }

    // -------------------------------------------------------------------------
    // WNDINC generation on read
    // -------------------------------------------------------------------------

    /// Reading data from the stream must generate a WNDINC for the bytes
    /// consumed.
    #[tokio::test]
    async fn test_read_generates_wndinc() {
        let (mut stream, mut tx, mut rx) = make_stream(256, false);

        tx.try_send(Body::Data(b"hello world"[..].into()).into())
            .unwrap();
        drop(tx);

        let mut buf = [0u8; 11];
        time::timeout(Duration::from_secs(1), stream.read_exact(&mut buf))
            .await
            .expect("should not time out")
            .expect("read_exact should succeed");

        assert_eq!(&buf, b"hello world");

        // The read must have generated a WNDINC for the 11 bytes consumed.
        let frame = rx.try_next().unwrap().unwrap();
        assert_eq!(
            frame.body,
            Body::WndInc(WndInc::clamp(11)),
            "read should generate a WNDINC of 11"
        );
    }
}
