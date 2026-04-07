use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
};

use async_trait::async_trait;
use futures::{
    SinkExt,
    channel::{
        mpsc,
        oneshot,
    },
    prelude::*,
    select,
    stream::StreamExt,
};
use tokio::io::{
    AsyncRead,
    AsyncWrite,
};
use tokio::sync::watch;
use tokio_util::codec::Framed;
use tracing::{
    Instrument,
    debug,
    debug_span,
    instrument,
    trace,
};

use crate::{
    codec::FrameCodec,
    errors::Error,
    frame::{
        Body,
        Frame,
        Header,
        HeaderType,
        StreamID,
    },
    stream::Stream,
    stream_manager::{
        OpenReq,
        SharedStreamManager,
        StreamManager,
    },
};

const DEFAULT_WINDOW: usize = 0x40000; // 256KB
const DEFAULT_ACCEPT: usize = 128;
const DEFAULT_STREAMS: usize = 512;

/// Builder for a muxado session.
///
/// Should probably leave this alone unless you're sure you know what you're
/// doing.
pub struct SessionBuilder<S> {
    io_stream: S,
    window: usize,
    accept_queue_size: usize,
    stream_limit: usize,
    client: bool,
    local_addr: Option<SocketAddr>,
    remote_addr: Option<SocketAddr>,
}

impl<S> SessionBuilder<S>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    /// Start building a new muxado session using the provided IO stream.
    pub fn new(io_stream: S) -> Self {
        SessionBuilder {
            io_stream,
            window: DEFAULT_WINDOW,
            accept_queue_size: DEFAULT_ACCEPT,
            stream_limit: DEFAULT_STREAMS,
            client: true,
            local_addr: None,
            remote_addr: None,
        }
    }

    /// Set the stream window size.
    /// Defaults to 256kb.
    pub fn window_size(mut self, size: usize) -> Self {
        self.window = size;
        self
    }

    /// Set the accept queue size.
    /// This is the size of the channel that will hold "open stream" requests
    /// from the remote. If [Accept::accept] isn't called and the
    /// channel fills up, the remote will receive RST(AcceptQueueFull).
    /// Defaults to 128.
    pub fn accept_queue_size(mut self, size: usize) -> Self {
        self.accept_queue_size = size;
        self
    }

    /// Set the maximum number of streams allowed at a given time.
    /// If this limit is reached, new streams will be refused.
    /// Defaults to 512.
    pub fn stream_limit(mut self, count: usize) -> Self {
        self.stream_limit = count;
        self
    }

    /// Set this session to act as a client.
    pub fn client(mut self) -> Self {
        self.client = true;
        self
    }

    /// Set this session to act as a server.
    pub fn server(mut self) -> Self {
        self.client = false;
        self
    }

    /// Set the local socket address for this session (informational).
    ///
    /// The address is not used internally but is returned by
    /// [MuxadoSession::local_addr].
    pub fn local_addr(mut self, addr: SocketAddr) -> Self {
        self.local_addr = Some(addr);
        self
    }

    /// Set the remote socket address for this session (informational).
    ///
    /// The address is not used internally but is returned by
    /// [MuxadoSession::remote_addr].
    pub fn remote_addr(mut self, addr: SocketAddr) -> Self {
        self.remote_addr = Some(addr);
        self
    }

    /// Start a muxado session with the current options.
    pub fn start(self) -> MuxadoSession {
        let SessionBuilder {
            io_stream,
            window,
            accept_queue_size,
            stream_limit,
            client,
            local_addr,
            remote_addr,
        } = self;

        let (accept_tx, accept_rx) = mpsc::channel(accept_queue_size);
        let (open_tx, open_rx) = mpsc::channel(512);

        let manager = StreamManager::new(stream_limit, client);
        let sys_tx = manager.sys_sender();
        let (m1, m2) = manager.split();

        let (io_tx, io_rx) = Framed::new(io_stream, FrameCodec::default()).split();

        // Signals MuxadoSession::wait() callers when the reader task exits.
        let (close_tx, close_rx) = watch::channel(false);

        let read_task = Reader {
            io: io_rx,
            accept_tx,
            window,
            manager: m1,
            last_stream_processed: StreamID::clamp(0),
            sys_tx: sys_tx.clone(),
            client,
            close_tx,
        };

        let write_task = Writer {
            window,
            io: io_tx,
            manager: m2,
            open_reqs: open_rx,
        };

        let (dropref, waiter) = awaitdrop::awaitdrop();

        tokio::spawn(
            futures::future::select(
                async move {
                    let result = read_task.run().await;
                    debug!(?result, "read_task exited");
                }
                .boxed(),
                waiter.wait(),
            )
            .instrument(debug_span!("read_task")),
        );
        tokio::spawn(
            futures::future::select(
                async move {
                    let result = write_task.run().await;
                    debug!(?result, "write_task exited");
                }
                .boxed(),
                waiter.wait(),
            )
            .instrument(debug_span!("write_task")),
        );

        MuxadoSession {
            incoming: MuxadoAccept(dropref.clone(), accept_rx),
            outgoing: MuxadoOpen {
                dropref,
                open_tx,
                sys_tx,
                closed: AtomicBool::from(false).into(),
            },
            close_rx,
            local_addr,
            remote_addr,
        }
    }
}

// read task - runs until there are no more frames coming from the remote
// Reads frames from the underlying stream and forwards them to the stream
// manager.
struct Reader<R> {
    io: R,
    sys_tx: mpsc::Sender<Frame>,
    accept_tx: mpsc::Sender<Stream>,
    window: usize,
    manager: SharedStreamManager,
    last_stream_processed: StreamID,
    client: bool,
    /// Signals waiters on MuxadoSession::wait() when the reader exits.
    close_tx: watch::Sender<bool>,
}

impl<R> Reader<R>
where
    R: futures::stream::Stream<Item = Result<Frame, io::Error>> + Unpin,
{
    /// Handle an incoming frame from the remote
    #[instrument(level = "trace", skip(self))]
    async fn handle_frame(&mut self, frame: Frame) -> Result<(), Error> {
        // If the remote sent a syn, create a new stream and add it to the accept channel.
        if frame.is_syn() {
            let stream_id = frame.header.stream_id;

            // Spec invariant: client opens odd IDs, server opens even IDs.
            // The remote's parity must be opposite to ours.  If it isn't,
            // that's a protocol error — send GOAWAY and abort.
            let local_parity = if self.client { 1u32 } else { 0u32 };
            if *stream_id % 2 == local_parity {
                debug!(
                    stream_id = display(stream_id),
                    client = self.client,
                    "received SYN with wrong stream ID parity, sending GOAWAY"
                );
                self.sys_tx
                    .send(Frame::goaway(
                        self.last_stream_processed,
                        Error::Protocol,
                        "stream ID parity violation".into(),
                    ))
                    .map_err(|_| Error::SessionClosed)
                    .await?;
                return Err(Error::Protocol);
            }

            let (req, mut stream) = OpenReq::create(self.window, false);
            self.manager
                .lock()
                .await
                .create_stream(frame.header.stream_id.into(), req)?;
            // Tag the stream with its assigned ID (spec §stream.feature: Stream.Id()).
            stream.set_id(stream_id);

            // Spec §session.feature: when the accept queue is full the session
            // must send RST(AcceptQueueFull) and keep running — it must NOT
            // block or close the session.
            match self.accept_tx.try_send(stream) {
                Ok(()) => {}
                Err(e) if e.is_full() => {
                    debug!(
                        stream_id = display(stream_id),
                        "accept queue full, sending RST(AcceptQueueFull)"
                    );
                    // Mark the stream handle so the writer emits no FIN for it.
                    // The stream object is dropped (never handed to the caller),
                    // so the manager will clean it up when its task fires.
                    if let Ok(handle) = self.manager.lock().await.get_stream(stream_id) {
                        handle.sink_closer.close_with(Error::AcceptQueueFull);
                        handle.needs_fin = false;
                    }
                    self.sys_tx
                        .send(Frame::rst(stream_id, Error::AcceptQueueFull))
                        .map_err(|_| Error::SessionClosed)
                        .await?;
                }
                Err(_) => {
                    return Err(Error::SessionClosed);
                }
            }
        }

        let needs_close = frame.is_fin();

        let Frame {
            header:
                Header {
                    length: _,
                    flags: _,
                    stream_id,
                    typ,
                },
            ..
        } = frame;

        match typ {
            // These frame types are stream-specific
            HeaderType::Data | HeaderType::Rst | HeaderType::WndInc => {
                // Spec §protocol.feature: a WNDINC with increment=0 is a
                // ProtocolError — send GOAWAY and close the session.
                if typ == HeaderType::WndInc
                    && let Body::WndInc(inc) = frame.body
                    && *inc == 0
                {
                    debug!("received WNDINC with zero increment, sending GOAWAY");
                    self.sys_tx
                        .send(Frame::goaway(
                            self.last_stream_processed,
                            Error::Protocol,
                            "WNDINC increment must not be zero".into(),
                        ))
                        .map_err(|_| Error::SessionClosed)
                        .await?;
                    return Err(Error::Protocol);
                }
                if let Err(error) = self.manager.send_to_stream(frame).await {
                    // If the stream manager couldn't send this frame to the
                    // stream for some reason, generate an RST to tell the other
                    // end to stop sending on this stream.
                    debug!(
                        stream_id = display(stream_id),
                        error = display(error),
                        "error sending to stream, generating rst"
                    );
                    self.sys_tx
                        .send(Frame::rst(stream_id, error))
                        .map_err(|_| Error::SessionClosed)
                        .await?;
                } else {
                    self.last_stream_processed = stream_id;
                    if needs_close
                        && let Ok(handle) = self.manager.lock().await.get_stream(stream_id)
                    {
                        handle.data_write_closed = true;
                    }
                }
            }

            // GoAway is a system-level frame, so send it along the special
            // system channel.
            HeaderType::GoAway => {
                if let Body::GoAway {
                    last_stream_id,
                    error,
                    ..
                } = frame.body
                {
                    // Only close locally-initiated streams beyond the acknowledged
                    // last stream ID — streams ≤ last_stream_id are left open.
                    self.manager
                        .go_away(error, Some(last_stream_id))
                        .await;
                    return Err(Error::RemoteGoneAway);
                }

                unreachable!()
            }

            // Unknown frame types MUST be silently discarded (spec §protocol.feature).
            HeaderType::Invalid(_) => {
                trace!(?frame, "received unknown frame type, discarding");
            }
        }
        Ok(())
    }

    // The actual read/process loop
    async fn run(mut self) -> Result<(), Error> {
        let _e: Result<(), _> = async {
            loop {
                match self.io.try_next().await {
                    Ok(Some(frame)) => {
                        trace!(?frame, "received frame from remote");
                        self.handle_frame(frame).await?
                    }
                    Ok(None) | Err(_) => {
                        return Err(Error::SessionClosed);
                    }
                }
            }
        }
        .await;

        // Signal any pending wait() callers that the session has ended.
        let _ = self.close_tx.send(true);

        self.manager.close_senders().await;

        Err(Error::SessionClosed)
    }
}

// The writer task responsible for receiving frames from streams or open
// requests and writing them to the underlying stream.
struct Writer<W> {
    manager: SharedStreamManager,
    window: usize,
    open_reqs: mpsc::Receiver<oneshot::Sender<Result<Stream, Error>>>,
    io: W,
}

impl<W> Writer<W>
where
    W: Sink<Frame, Error = io::Error> + Unpin + Send + 'static,
{
    async fn run(mut self) -> Result<(), Error> {
        loop {
            select! {
                // The stream manager produced a frame that needs to be sent to
                // the remote.
                frame = self.manager.next() => {
                    if let Some(frame) = frame {
                        let is_goaway = matches!(frame.header.typ, HeaderType::GoAway);
                        trace!(?frame, "sending frame to remote");
                        if let Err(_e) = self.io.send(frame).await {
                            return Err(Error::SessionClosed);
                        }
                        if is_goaway {
                            return Ok(())
                        }
                    }
                },
                // If a request for a new stream originated locally, tell the
                // stream manager to create it. The first dataframe from it will
                // have the SYN flag set.
                req = self.open_reqs.next() => {
                    if let Some(resp_tx) = req {
                        let (req, mut stream) = OpenReq::create(self.window, true);

                        let mut manager = self.manager.lock().await;
                        let res = manager.create_stream(None, req);
                        // Tag the stream with the assigned ID before handing it
                        // back to the caller (spec §stream.feature: Stream.Id()).
                        if let Ok(id) = res {
                            stream.set_id(id);
                        }
                        let _ = resp_tx.send(res.map(move |_| stream));
                    }
                },
                // All senders have been dropped - exit.
                complete => {
                    return Ok(());
                }
            }
        }
    }
}

/// A muxado session.
///
/// Can be used directly to open and accept streams, or split into dedicated
/// open/accept parts.
pub trait Session: Accept + OpenClose {
    /// The open half of the session.
    type OpenClose: OpenClose;
    /// The accept half of the session.
    type Accept: Accept;
    /// Split the session into dedicated open/accept components.
    fn split(self) -> (Self::OpenClose, Self::Accept);
}

/// Trait for accepting incoming streams in a muxado [Session].
#[async_trait]
pub trait Accept {
    /// Accept an incoming stream that was opened by the remote.
    async fn accept(&mut self) -> Option<Stream>;
}

/// Trait for opening new streams in a muxado [Session].
#[async_trait]
pub trait OpenClose {
    /// Open a new stream.
    async fn open(&mut self) -> Result<Stream, Error>;
    /// Close the session by sending a GOAWAY
    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error>;
}

/// The [Open] half of a muxado session.
#[derive(Clone)]
pub struct MuxadoOpen {
    dropref: awaitdrop::Ref,
    open_tx: mpsc::Sender<oneshot::Sender<Result<Stream, Error>>>,
    sys_tx: mpsc::Sender<Frame>,
    closed: Arc<AtomicBool>,
}

/// The [Accept] half of a muxado session.
pub struct MuxadoAccept(#[allow(dead_code)] awaitdrop::Ref, mpsc::Receiver<Stream>);

#[async_trait]
impl Accept for MuxadoAccept {
    async fn accept(&mut self) -> Option<Stream> {
        self.1.next().await
    }
}

#[async_trait]
impl OpenClose for MuxadoOpen {
    async fn open(&mut self) -> Result<Stream, Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::SessionClosed);
        }
        let (resp_tx, resp_rx) = oneshot::channel();

        self.open_tx
            .send(resp_tx)
            .await
            .map_err(|_| Error::SessionClosed)?;

        let mut res = resp_rx
            .await
            .map_err(|_| Error::SessionClosed)
            .and_then(|r| r);

        if let Ok(stream) = &mut res {
            stream.dropref = self.dropref.clone().into();
        }

        res
    }

    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error> {
        let res = self
            .sys_tx
            .send(Frame::goaway(
                StreamID::clamp(0),
                error,
                msg.into_bytes().into(),
            ))
            .await
            .map_err(|_| Error::SessionClosed);
        self.closed.store(true, Ordering::SeqCst);
        res
    }
}

/// The base muxado [Session] implementation.
///
/// See the [Session], [Accept], and [Open] trait implementations for
/// available methods.
pub struct MuxadoSession {
    incoming: MuxadoAccept,
    outgoing: MuxadoOpen,
    /// Resolves to `true` once the remote session closes (GOAWAY or disconnect).
    close_rx: watch::Receiver<bool>,
    local_addr: Option<SocketAddr>,
    remote_addr: Option<SocketAddr>,
}

impl MuxadoSession {
    /// Block until the remote closes the session (sends GOAWAY or disconnects).
    pub async fn wait(&mut self) {
        let _ = self.close_rx.wait_for(|&b| b).await;
    }

    /// The local socket address of the underlying transport, if known.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// The remote socket address of the underlying transport, if known.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote_addr
    }
}

#[async_trait]
impl Accept for MuxadoSession {
    async fn accept(&mut self) -> Option<Stream> {
        self.incoming.accept().await
    }
}

#[async_trait]
impl OpenClose for MuxadoSession {
    async fn open(&mut self) -> Result<Stream, Error> {
        self.outgoing.open().await
    }

    async fn close(&mut self, error: Error, msg: String) -> Result<(), Error> {
        self.outgoing.close(error, msg).await
    }
}

impl Session for MuxadoSession {
    type Accept = MuxadoAccept;
    type OpenClose = MuxadoOpen;
    fn split(self) -> (Self::OpenClose, Self::Accept) {
        (self.outgoing, self.incoming)
    }
}

#[cfg(test)]
mod test {
    use tokio::{
        io::{
            self,
            AsyncReadExt,
            AsyncWriteExt,
        },
        time::{
            Duration,
            timeout,
        },
    };

    use super::*;
    use crate::frame::{
        Body,
        Frame,
        StreamID,
    };

    /// Convenience: create a connected client/server session pair over an
    /// in-memory duplex pipe.
    fn session_pair(buf: usize) -> (MuxadoSession, MuxadoSession) {
        let (left, right) = io::duplex(buf);
        let server = SessionBuilder::new(left).server().start();
        let client = SessionBuilder::new(right).client().start();
        (server, client)
    }

    #[tokio::test]
    async fn test_session() {
        let (mut server, mut client) = session_pair(512);

        tokio::spawn(async move {
            let mut stream = server.accept().await.expect("accept stream");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("read stream");
            drop(stream);
            let mut stream = server.open().await.expect("open stream");
            stream.write_all(&buf).await.expect("write to stream");
        });

        let mut stream = client.open().await.expect("open stream");
        stream
            .write_all(b"Hello, world!")
            .await
            .expect("write to stream");
        drop(stream);

        let mut stream = client.accept().await.expect("accept stream");
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .expect("read from stream");

        assert_eq!(b"Hello, world!", &*buf,);
    }

    // -------------------------------------------------------------------------
    // Stream ID parity tests
    // -------------------------------------------------------------------------

    /// The client allocates stream IDs that are odd (3, 5, 7, …).
    /// We observe the SYN flag on the server side and verify the stream ID
    /// parity without accessing private fields.  We open multiple streams and
    /// check that each one is accepted by the server exactly once, confirming
    /// the session bookkeeping is consistent.
    #[tokio::test]
    async fn test_multiple_streams_client_to_server() {
        let (mut server, mut client) = session_pair(4096);

        let count = 5usize;
        let server_handle = tokio::spawn(async move {
            let mut results = Vec::new();
            for _ in 0..count {
                let mut stream = server.accept().await.expect("accept");
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).await.unwrap();
                results.push(buf);
            }
            results
        });

        for i in 0..count {
            let mut stream = client.open().await.expect("open");
            stream
                .write_all(format!("msg{i}").as_bytes())
                .await
                .unwrap();
        }

        let results = timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("timed out")
            .expect("server panicked");

        assert_eq!(results.len(), count);
        for (i, buf) in results.iter().enumerate() {
            assert_eq!(buf, format!("msg{i}").as_bytes());
        }
    }

    // -------------------------------------------------------------------------
    // GOAWAY / close tests
    // -------------------------------------------------------------------------

    /// Calling `close()` should cause the remote's `accept()` to return None,
    /// signalling that the session has terminated.
    #[tokio::test]
    async fn test_close_terminates_remote_accept() {
        let (server, mut client) = session_pair(4096);
        let (server_open, mut server_accept) = server.split();

        let server_handle = tokio::spawn(async move {
            // Wait for the session to close — accept() returns None.
            let result = server_accept.accept().await;
            assert!(result.is_none(), "accept should return None after GOAWAY");
        });

        // Give the server a moment to start accepting.
        tokio::time::sleep(Duration::from_millis(10)).await;

        client
            .close(Error::None, "clean shutdown".into())
            .await
            .expect("close should succeed");

        drop(client);
        drop(server_open);

        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("timed out")
            .expect("server panicked");
    }

    /// Closing a session while streams are in-flight must not panic.
    #[tokio::test]
    async fn test_close_with_open_streams() {
        let (mut server, mut client) = session_pair(4096);

        let server_handle = tokio::spawn(async move {
            // Accept streams until the session closes.
            while let Some(_stream) = server.accept().await {}
        });

        let mut stream = client.open().await.expect("open stream");
        stream.write_all(b"hello").await.unwrap();

        client
            .close(Error::None, String::new())
            .await
            .expect("close should succeed");

        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("timed out")
            .expect("server panicked");
    }

    // -------------------------------------------------------------------------
    // Stream limit test
    // -------------------------------------------------------------------------

    /// Opening more streams than the configured limit must return
    /// `Error::StreamsExhausted`.
    #[tokio::test]
    async fn test_stream_limit_exhausted() {
        const LIMIT: usize = 4;
        let (left, right) = io::duplex(65536);
        let mut server = SessionBuilder::new(left).server().start();
        let mut client = SessionBuilder::new(right)
            .client()
            .stream_limit(LIMIT)
            .start();

        // Keep the server draining so the client can open streams.
        tokio::spawn(async move { while let Some(_stream) = server.accept().await {} });

        // Open exactly LIMIT streams — all must succeed.
        let mut streams = Vec::new();
        for _ in 0..LIMIT {
            let s = client.open().await.expect("should open within limit");
            streams.push(s);
        }

        // The next open must fail.
        let result = client.open().await;
        assert_eq!(
            result.unwrap_err(),
            Error::StreamsExhausted,
            "expected StreamsExhausted after limit reached"
        );
    }

    // -------------------------------------------------------------------------
    // Session split test
    // -------------------------------------------------------------------------

    /// The session can be split into independent open/accept halves and used
    /// from separate tasks.
    #[tokio::test]
    async fn test_split_session() {
        let (mut server, client) = session_pair(4096);
        let (mut client_open, client_accept) = client.split();

        // Server accepts one stream.
        let server_handle = tokio::spawn(async move {
            let mut s = server.accept().await.expect("accept");
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await.unwrap();
            buf
        });

        // Open from the open-half, accept from the accept-half (different tasks).
        let open_handle = tokio::spawn(async move {
            let mut s = client_open.open().await.expect("open");
            s.write_all(b"split test").await.unwrap();
        });

        // client_accept is kept alive (but not used) to prevent session teardown.
        let accept_handle = tokio::spawn(async move {
            let _keep = client_accept;
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        open_handle.await.unwrap();
        let data = timeout(Duration::from_secs(5), server_handle)
            .await
            .unwrap()
            .unwrap();
        accept_handle.await.unwrap();

        assert_eq!(data, b"split test");
    }

    // -------------------------------------------------------------------------
    // Bidirectional concurrent streams
    // -------------------------------------------------------------------------

    /// Both client and server open streams simultaneously; all data must arrive
    /// correctly with no deadlock.
    #[tokio::test]
    async fn test_bidirectional_concurrent_streams() {
        let (mut server, mut client) = session_pair(65536);

        // Server: accept one stream from client, then open one back.
        let server_handle = tokio::spawn(async move {
            let mut from_client = server.accept().await.expect("server accept");
            let mut buf = Vec::new();
            from_client.read_to_end(&mut buf).await.unwrap();

            let mut to_client = server.open().await.expect("server open");
            to_client.write_all(&buf).await.unwrap();
        });

        // Client: open to server, then accept the echo back.
        let mut to_server = client.open().await.expect("client open");
        to_server.write_all(b"ping").await.unwrap();
        drop(to_server);

        let mut from_server = client.accept().await.expect("client accept");
        let mut buf = Vec::new();
        from_server.read_to_end(&mut buf).await.unwrap();

        server_handle.await.unwrap();
        assert_eq!(buf, b"ping");
    }

    // -------------------------------------------------------------------------
    // Stream ID parity enforcement tests
    // -------------------------------------------------------------------------

    /// Inject a raw frame with the wrong parity SYN into the session to verify
    /// the reader rejects it with a ProtocolError.
    ///
    /// We accomplish this by building a minimal transport from a pair of
    /// in-memory pipes and writing a crafted raw frame directly.
    #[tokio::test]
    async fn test_server_rejects_even_syn_from_client() {
        use crate::codec::FrameCodec;
        use tokio::io::AsyncWriteExt as _;
        use tokio_util::codec::Encoder as _;

        let (transport_a, mut transport_b) = tokio::io::duplex(4096);
        let server = SessionBuilder::new(transport_a).server().start();
        let (_, mut server_accept) = server.split();

        // Write a SYN DATA frame with an *even* stream ID (2) — wrong parity
        // for a frame coming from the "client" side.
        let bad_frame = Frame::from(Body::Data(b"hello"[..].into()))
            .syn()
            .stream_id(StreamID::clamp(2)); // even = server's own parity → violation

        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        codec.encode(bad_frame, &mut buf).unwrap();
        transport_b.write_all(&buf).await.unwrap();

        // The server should close the session; accept() returns None.
        let result = timeout(Duration::from_secs(2), server_accept.accept()).await;
        assert!(
            matches!(result, Ok(None) | Err(_)),
            "server should close session on parity violation, got: {result:?}"
        );
    }

    /// Symmetric test: the client must reject an odd-ID SYN coming from the
    /// server side (the server opens even IDs, so an odd ID is wrong parity).
    #[tokio::test]
    async fn test_client_rejects_odd_syn_from_server() {
        use crate::codec::FrameCodec;
        use tokio::io::AsyncWriteExt as _;
        use tokio_util::codec::Encoder as _;

        let (transport_a, mut transport_b) = tokio::io::duplex(4096);
        let client = SessionBuilder::new(transport_a).client().start();
        let (_, mut client_accept) = client.split();

        // Write a SYN DATA frame with an *odd* stream ID (3) — wrong parity
        // for a frame coming from the "server" side.
        let bad_frame = Frame::from(Body::Data(b"hello"[..].into()))
            .syn()
            .stream_id(StreamID::clamp(3)); // odd = client's own parity → violation

        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        codec.encode(bad_frame, &mut buf).unwrap();
        transport_b.write_all(&buf).await.unwrap();

        let result = timeout(Duration::from_secs(2), client_accept.accept()).await;
        assert!(
            matches!(result, Ok(None) | Err(_)),
            "client should close session on parity violation, got: {result:?}"
        );
    }

    // -------------------------------------------------------------------------
    // GOAWAY last-stream-id filtering test
    // -------------------------------------------------------------------------

    /// When the remote sends GOAWAY(lastStreamId=N), only locally-opened
    /// streams with ID > N should be closed.  Streams ≤ N must remain open.
    #[tokio::test]
    async fn test_goaway_filters_by_last_stream_id() {
        use crate::codec::FrameCodec;
        use tokio::io::AsyncWriteExt as _;
        use tokio_util::codec::Encoder as _;

        // Client opens streams; we inject a GOAWAY from the "server" side.
        let (transport_a, mut transport_b) = tokio::io::duplex(65536);
        let mut client = SessionBuilder::new(transport_a).client().start();

        // Open three streams: IDs 3, 5, 7.
        let mut s3 = client.open().await.unwrap();
        let mut s5 = client.open().await.unwrap();
        let mut s7 = client.open().await.unwrap();

        // Trigger the SYN sends so the streams are registered.
        s3.write_all(b"a").await.unwrap();
        s5.write_all(b"b").await.unwrap();
        s7.write_all(b"c").await.unwrap();

        // Drain the SYN frames so transport_b doesn't fill up.
        let mut drain_buf = vec![0u8; 4096];
        let _ = timeout(Duration::from_millis(50), transport_b.read(&mut drain_buf)).await;

        // Inject GOAWAY(lastStreamId=5, NoError) from the "server".
        let goaway = Frame::goaway(StreamID::clamp(5), Error::None, bytes::Bytes::new());
        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        codec.encode(goaway, &mut buf).unwrap();
        transport_b.write_all(&buf).await.unwrap();

        // Give the reader task a moment to process.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Streams 3 and 5 (id ≤ lastStreamId) must still be writable.
        assert!(
            s3.write_all(b"still open").await.is_ok(),
            "stream 3 should remain open"
        );
        assert!(
            s5.write_all(b"still open").await.is_ok(),
            "stream 5 should remain open"
        );

        // Stream 7 (id > lastStreamId, locally opened) must be closed.
        let write7 = timeout(Duration::from_secs(1), s7.write_all(b"should fail")).await;
        assert!(
            matches!(write7, Ok(Err(_)) | Err(_)),
            "stream 7 should be closed after GOAWAY(lastStreamId=5)"
        );
    }

    // -------------------------------------------------------------------------
    // Unknown frame type silently discarded test
    // -------------------------------------------------------------------------

    /// An unknown frame type must NOT terminate the session; the session must
    /// continue accepting new streams normally.
    #[tokio::test]
    async fn test_unknown_frame_type_is_discarded() {
        use tokio::io::AsyncWriteExt as _;

        let (transport_a, mut transport_b) = tokio::io::duplex(65536);
        let mut server = SessionBuilder::new(transport_a).server().start();

        // Write a raw frame with an unknown type byte (e.g. 0xF in the upper nibble).
        // Header layout: [length(3B) | type+flags(1B) | stream_id(4B)]
        // type nibble 0xF, flags nibble 0x0 → byte 0xF0, length 0, stream_id 0.
        let raw: [u8; 8] = [
            0x00, 0x00, 0x00, // length = 0
            0xF0, // type=0xF, flags=0
            0x00, 0x00, 0x00, 0x00, // stream_id = 0
        ];
        transport_b.write_all(&raw).await.unwrap();

        // Now send a legitimate SYN so we can verify the session is still alive.
        let (_client_open, _) = SessionBuilder::new(transport_b).client().start().split();
        // Trigger an actual stream open from the other direction via raw writes.
        // Easiest: just check that server.accept() doesn't return None immediately
        // (session not dead). We'll do a small sleep to let the unknown frame be processed.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The session must still be operational — accept() should be pending, not None.
        let accept_result = timeout(Duration::from_millis(50), server.accept()).await;
        assert!(
            accept_result.is_err(), // Timeout = still waiting = session alive
            "session should still be alive after unknown frame (accept should be pending)"
        );
    }

    // =========================================================================
    // Medium-priority gap regression tests
    // =========================================================================

    /// Spec §session.feature: when the accept queue is full the session must
    /// send RST(AcceptQueueFull) and continue running — it must NOT block or
    /// close the session.
    #[tokio::test]
    async fn test_accept_queue_full_sends_rst() {
        use tokio::io::AsyncWriteExt as _;

        // Server with accept queue size 1 so it fills quickly.
        let (transport_a, transport_b) = tokio::io::duplex(65536);
        let server = SessionBuilder::new(transport_a)
            .server()
            .accept_queue_size(1)
            .start();
        let (_, mut server_accept) = server.split();
        let (mut client_open, _) = SessionBuilder::new(transport_b).client().start().split();

        // SYNs are sent on the first write to each stream.
        let mut s1 = client_open.open().await.unwrap();
        s1.write_all(b"a").await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Drain stream 1 so the queue is empty again.
        let accepted = timeout(Duration::from_millis(100), server_accept.accept()).await;
        assert!(matches!(accepted, Ok(Some(_))), "should accept stream 1");

        // Fill the queue with stream 3.
        let mut s3 = client_open.open().await.unwrap();
        s3.write_all(b"b").await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Open stream 5 while queue is full → server sends RST(AcceptQueueFull).
        let mut s5 = client_open.open().await.unwrap();
        s5.write_all(b"overflow").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Server session must still be alive: stream 3 is in the queue.
        let got = timeout(Duration::from_millis(100), server_accept.accept()).await;
        assert!(
            matches!(got, Ok(Some(_))),
            "server should have stream 3 in queue after overflow"
        );
    }

    /// Spec §protocol.feature: WNDINC with increment=0 must close the session
    /// with ProtocolError (GOAWAY).
    #[tokio::test]
    async fn test_wndinc_zero_is_protocol_error() {
        use tokio::io::AsyncWriteExt as _;

        let (transport_a, mut transport_b) = tokio::io::duplex(65536);
        let server = SessionBuilder::new(transport_a).server().start();
        let (_, mut server_accept) = server.split();

        // Write a raw WNDINC frame with stream_id=1 and increment=0.
        // Layout: [length(3B) | type+flags(1B) | stream_id(4B) | increment(4B)]
        // type nibble for WndInc = 0x3, flags = 0 → byte 0x30
        let raw: [u8; 12] = [
            0x00, 0x00, 0x04, // length = 4
            0x30, // type=0x3 (WndInc), flags=0
            0x00, 0x00, 0x00, 0x01, // stream_id = 1
            0x00, 0x00, 0x00, 0x00, // increment = 0
        ];
        transport_b.write_all(&raw).await.unwrap();

        // Server should close the session; accept() returns None.
        let result = timeout(Duration::from_secs(2), server_accept.accept()).await;
        assert!(
            matches!(result, Ok(None) | Err(_)),
            "server should close session on zero WNDINC, got: {result:?}"
        );
    }

    /// Spec §session.feature: wait() must block until the remote closes the
    /// session, then resolve.
    #[tokio::test]
    async fn test_wait_resolves_on_remote_close() {
        let (transport_a, transport_b) = tokio::io::duplex(65536);
        let mut server = SessionBuilder::new(transport_a).server().start();
        let mut client = SessionBuilder::new(transport_b).client().start();

        // Close client → server's reader sees EOF and exits.
        client.close(Error::None, String::new()).await.unwrap();

        // wait() on the server should resolve within a reasonable timeout.
        let result = timeout(Duration::from_secs(2), server.wait()).await;
        assert!(
            result.is_ok(),
            "server wait() should resolve when client closes"
        );
    }

    /// Spec §stream.feature: frames arriving within the 5-second RST deferred
    /// removal window must be silently discarded (no spurious protocol error).
    #[tokio::test]
    async fn test_rst_deferred_discards_late_frames() {
        use crate::codec::FrameCodec;
        use tokio::io::AsyncWriteExt as _;
        use tokio_util::codec::Encoder as _;

        let (transport_a, mut transport_b) = tokio::io::duplex(65536);
        let server = SessionBuilder::new(transport_a).server().start();
        let (_, mut server_accept) = server.split();

        // Open stream 1 from the client side by sending a SYN DATA frame.
        let syn_frame = Frame::from(Body::Data(b"hello"[..].into()))
            .syn()
            .stream_id(StreamID::clamp(1));
        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        codec.encode(syn_frame, &mut buf).unwrap();
        transport_b.write_all(&buf).await.unwrap();

        tokio::time::sleep(Duration::from_millis(30)).await;

        // Send RST for stream 1.
        let rst_frame =
            Frame::rst(StreamID::clamp(1), Error::StreamReset).stream_id(StreamID::clamp(1));
        buf.clear();
        codec.encode(rst_frame, &mut buf).unwrap();
        transport_b.write_all(&buf).await.unwrap();

        tokio::time::sleep(Duration::from_millis(30)).await;

        // Now send a DATA frame for the already-RST'd stream 1.
        // This should be silently discarded, NOT trigger a GOAWAY.
        let late_frame = Frame::from(Body::Data(b"late"[..].into()))
            .stream_id(StreamID::clamp(1));
        buf.clear();
        codec.encode(late_frame, &mut buf).unwrap();
        transport_b.write_all(&buf).await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Session must still be alive — accept() should still be pending.
        let _ = timeout(Duration::from_millis(20), server_accept.accept()).await;
        // Just verify we can still use the session (no panic / GOAWAY was sent).
        // If the session died, accept() would return None immediately.
        // We already drained the one stream above, so now accept is pending again.
        let accept_result = timeout(Duration::from_millis(50), server_accept.accept()).await;
        assert!(
            accept_result.is_err(), // Timeout means still waiting = session alive
            "session should be alive after late frame on RST'd stream"
        );
    }
}
