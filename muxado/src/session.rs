use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            AtomicU32,
            Ordering,
        },
    },
};

use bytes::Bytes;
use tokio::{
    io::{
        AsyncRead,
        AsyncReadExt,
        AsyncWrite,
        AsyncWriteExt,
    },
    sync::{
        Mutex,
        Notify,
        mpsc,
    },
};
use tracing::{
    debug,
    trace,
    warn,
};

use crate::{
    config::Config,
    error::{
        ErrorCode,
        MuxadoError,
    },
    frame::{
        DataFlags,
        DataFrame,
        Frame,
        FrameError,
        GoAwayFrame,
        HEADER_SIZE,
        Header,
        RstFrame,
        WndIncFrame,
        decode_frame_payload,
    },
    stream::{
        MuxadoStream,
        StreamCommand,
        StreamInner,
        handle_stream_data,
        handle_stream_rst,
        handle_stream_wndinc,
        new_stream,
    },
};

/// A muxado session that multiplexes streams over a single transport.
pub struct Session {
    inner: Arc<SessionInner>,
    /// Handle to the reader task.
    reader_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the writer task.
    writer_handle: Option<tokio::task::JoinHandle<()>>,
}

struct SessionInner {
    /// Channel to accept incoming streams.
    accept_tx: mpsc::Sender<MuxadoStream>,
    accept_rx: Mutex<mpsc::Receiver<MuxadoStream>>,

    /// Channel for outbound frame commands from streams.
    cmd_tx: mpsc::UnboundedSender<StreamCommand>,

    /// Channel for outbound frames to the writer task.
    frame_tx: mpsc::Sender<Bytes>,

    /// Active streams by ID (shared state for the session reader).
    streams: Mutex<HashMap<u32, Arc<StreamInner>>>,

    /// Next local stream ID (incremented by 2).
    next_stream_id: AtomicU32,

    /// Whether this is the client side (odd IDs) or server side (even IDs).
    is_client: bool,

    /// Session config.
    config: Config,

    /// Whether the session is dead.
    is_dead: AtomicBool,

    /// Notified when session dies.
    dead_notify: Notify,

    /// Whether local side has sent GOAWAY.
    local_gone_away: AtomicBool,

    /// Whether remote side has sent GOAWAY.
    remote_gone_away: AtomicBool,

    /// Highest stream ID initiated by the remote peer (updated on SYN).
    last_remote_stream_id: AtomicU32,
}

impl Session {
    /// Create a client session. Client streams use odd IDs (1, 3, 5, ...).
    pub fn client<T>(transport: T, config: Config) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        Self::new(transport, config, true)
    }

    /// Create a server session. Server streams use even IDs (2, 4, 6, ...).
    pub fn server<T>(transport: T, config: Config) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        Self::new(transport, config, false)
    }

    fn new<T>(transport: T, config: Config, is_client: bool) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (accept_tx, accept_rx) = mpsc::channel(config.accept_backlog);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (frame_tx, frame_rx) = mpsc::channel(config.write_queue_depth);

        let initial_id = if is_client { 1 } else { 2 };
        let role = if is_client { "client" } else { "server" };

        debug!(role, initial_id, "session created");

        let inner = Arc::new(SessionInner {
            accept_tx,
            accept_rx: Mutex::new(accept_rx),
            cmd_tx: cmd_tx.clone(),
            frame_tx: frame_tx.clone(),
            streams: Mutex::new(HashMap::new()),
            next_stream_id: AtomicU32::new(initial_id),
            is_client,
            config: config.clone(),
            is_dead: AtomicBool::new(false),
            dead_notify: Notify::new(),
            local_gone_away: AtomicBool::new(false),
            remote_gone_away: AtomicBool::new(false),
            last_remote_stream_id: AtomicU32::new(0),
        });

        let (read_half, write_half) = tokio::io::split(transport);

        let reader_inner = inner.clone();
        let reader_handle = tokio::spawn(async move {
            reader_loop(reader_inner, read_half).await;
        });

        let writer_inner = inner.clone();
        let writer_handle = tokio::spawn(async move {
            writer_loop(writer_inner, write_half, frame_rx, cmd_rx).await;
        });

        Self {
            inner,
            reader_handle: Some(reader_handle),
            writer_handle: Some(writer_handle),
        }
    }

    /// Open a new locally-initiated stream.
    pub async fn open(&self) -> Result<MuxadoStream, MuxadoError> {
        if self.inner.is_dead.load(Ordering::Acquire) {
            return Err(MuxadoError::SessionClosed);
        }
        if self.inner.local_gone_away.load(Ordering::Acquire) {
            return Err(MuxadoError::SessionClosed);
        }

        let id = self.inner.next_stream_id.fetch_add(2, Ordering::Relaxed);
        if id & 0x8000_0000 != 0 {
            return Err(MuxadoError::StreamsExhausted);
        }

        let (stream, inner) = new_stream(
            id,
            self.inner.config.max_window_size,
            self.inner.cmd_tx.clone(),
            true,
        );

        self.inner.streams.lock().await.insert(id, inner);

        debug!(stream_id = id, "stream opened");
        Ok(stream)
    }

    /// Accept a remotely-initiated stream.
    pub async fn accept(&self) -> Result<MuxadoStream, MuxadoError> {
        let mut rx = self.inner.accept_rx.lock().await;
        match rx.recv().await {
            Some(stream) => {
                debug!(stream_id = stream.id(), "stream accepted");
                Ok(stream)
            }
            None => Err(MuxadoError::SessionClosed),
        }
    }

    /// Gracefully close the session by sending GOAWAY.
    pub async fn close(&self) -> Result<(), MuxadoError> {
        if self.inner.is_dead.load(Ordering::Acquire) {
            return Ok(());
        }
        self.inner.local_gone_away.store(true, Ordering::Release);

        let last_id = self.last_remote_stream_id();
        let goaway = GoAwayFrame::new(last_id, ErrorCode::NoError, Bytes::new());
        let encoded = goaway.encode();
        let _ = self.inner.frame_tx.send(encoded.freeze()).await;
        debug!("session GOAWAY sent");
        Ok(())
    }

    /// Wait for the session to terminate.
    pub async fn wait(&self) {
        self.inner.dead_notify.notified().await;
    }

    /// Returns true if the session is dead.
    pub fn is_dead(&self) -> bool {
        self.inner.is_dead.load(Ordering::Acquire)
    }

    fn last_remote_stream_id(&self) -> u32 {
        self.inner.last_remote_stream_id.load(Ordering::Acquire)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(h) = self.reader_handle.take() {
            h.abort();
        }
        if let Some(h) = self.writer_handle.take() {
            h.abort();
        }
    }
}

/// The reader loop: reads frames from the transport and dispatches them.
async fn reader_loop<R: AsyncRead + Unpin>(inner: Arc<SessionInner>, mut reader: R) {
    let mut header_buf = [0u8; HEADER_SIZE];

    loop {
        if let Err(e) = reader.read_exact(&mut header_buf).await {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                die(&inner, ErrorCode::PeerEOF, "transport EOF").await;
            } else {
                die(&inner, ErrorCode::InternalError, &e.to_string()).await;
            }
            return;
        }

        let header = match Header::decode(&header_buf) {
            Ok(h) => h,
            Err(FrameError::UnknownFrameType { length, .. }) => {
                let mut discard = vec![0u8; length as usize];
                if reader.read_exact(&mut discard).await.is_err() {
                    die(&inner, ErrorCode::PeerEOF, "transport EOF during skip").await;
                    return;
                }
                trace!(length, "skipped unknown frame type");
                continue;
            }
            Err(_) => {
                die(&inner, ErrorCode::ProtocolError, "invalid frame header").await;
                return;
            }
        };

        let mut payload_buf = vec![0u8; header.length as usize];
        if !payload_buf.is_empty() && reader.read_exact(&mut payload_buf).await.is_err() {
            die(&inner, ErrorCode::PeerEOF, "transport EOF during payload").await;
            return;
        }

        // Decode directly from the pre-parsed header + owned payload (zero-copy)
        let frame = match decode_frame_payload(&header, Bytes::from(payload_buf)) {
            Ok(f) => f,
            Err(_) => {
                die(&inner, ErrorCode::ProtocolError, "frame decode error").await;
                return;
            }
        };

        match frame {
            Frame::Data(df) => handle_data_frame(&inner, df).await,
            Frame::Rst(rf) => handle_rst_frame(&inner, rf).await,
            Frame::WndInc(wf) => handle_wndinc_frame(&inner, wf).await,
            Frame::GoAway(_gf) => {
                handle_goaway_frame(&inner).await;
                return;
            }
        }
    }
}

/// Write bytes to the transport, killing the session on failure.
async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    inner: &Arc<SessionInner>,
    data: &[u8],
) -> bool {
    if writer.write_all(data).await.is_err() {
        die(inner, ErrorCode::InternalError, "write error").await;
        return false;
    }
    true
}

/// The writer loop: takes frames from the command channel and writes them.
async fn writer_loop<W: AsyncWrite + Unpin>(
    inner: Arc<SessionInner>,
    mut writer: W,
    mut frame_rx: mpsc::Receiver<Bytes>,
    mut cmd_rx: mpsc::UnboundedReceiver<StreamCommand>,
) {
    loop {
        tokio::select! {
            Some(frame_bytes) = frame_rx.recv() => {
                if !write_frame(&mut writer, &inner, &frame_bytes).await {
                    return;
                }
                if writer.flush().await.is_err() {
                    die(&inner, ErrorCode::InternalError, "transport flush error").await;
                    return;
                }
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    StreamCommand::SendData { stream_id, flags, data } => {
                        let encoded = DataFrame::new(stream_id, flags, data).encode();
                        if !write_frame(&mut writer, &inner, &encoded).await { return; }
                        trace!(stream_id, "wrote DATA frame");
                    }
                    StreamCommand::SendRst { stream_id, error_code } => {
                        let encoded = RstFrame::new(stream_id, error_code).encode();
                        if !write_frame(&mut writer, &inner, &encoded).await { return; }
                        trace!(stream_id, ?error_code, "wrote RST frame");
                    }
                    StreamCommand::SendWndInc { stream_id, increment } => {
                        if increment > 0 {
                            let encoded = WndIncFrame::new(stream_id, increment).encode();
                            if !write_frame(&mut writer, &inner, &encoded).await { return; }
                            trace!(stream_id, increment, "wrote WNDINC frame");
                        }
                    }
                    StreamCommand::StreamFinished { stream_id } => {
                        inner.streams.lock().await.remove(&stream_id);
                        trace!(stream_id, "stream removed from session");
                    }
                }
            }
            else => {
                return;
            }
        }
    }
}

async fn handle_data_frame(inner: &Arc<SessionInner>, df: DataFrame) {
    let stream_id = df.stream_id;

    if df.flags.contains(DataFlags::SYN) {
        // New stream from remote
        let expected_parity = if inner.is_client { 0 } else { 1 };
        if stream_id % 2 != expected_parity {
            die(inner, ErrorCode::ProtocolError, "wrong stream ID parity").await;
            return;
        }

        // Track highest remote stream ID for GOAWAY
        inner
            .last_remote_stream_id
            .fetch_max(stream_id, Ordering::AcqRel);

        if inner.local_gone_away.load(Ordering::Acquire) {
            send_rst_async(inner, stream_id, ErrorCode::StreamRefused).await;
            return;
        }

        let (stream, stream_inner) = new_stream(
            stream_id,
            inner.config.max_window_size,
            inner.cmd_tx.clone(),
            false,
        );

        // Deliver initial data
        if let Err(code) = handle_stream_data(&stream_inner, df.body, df.flags) {
            send_rst_async(inner, stream_id, code).await;
            return;
        }

        inner.streams.lock().await.insert(stream_id, stream_inner);

        // Send to accept queue
        if inner.accept_tx.try_send(stream).is_err() {
            send_rst_async(inner, stream_id, ErrorCode::AcceptQueueFull).await;
            inner.streams.lock().await.remove(&stream_id);
        }

        trace!(stream_id, "new remote stream accepted");
    } else {
        // Existing stream
        let streams = inner.streams.lock().await;
        if let Some(stream_inner) = streams.get(&stream_id)
            && let Err(code) = handle_stream_data(stream_inner, df.body, df.flags)
        {
            drop(streams);
            send_rst_async(inner, stream_id, code).await;
        }
    }
}

async fn handle_rst_frame(inner: &Arc<SessionInner>, rf: RstFrame) {
    let streams = inner.streams.lock().await;
    if let Some(stream_inner) = streams.get(&rf.stream_id) {
        handle_stream_rst(stream_inner, rf.error_code);
        trace!(stream_id = rf.stream_id, "RST received");
    }
}

/// Handle WNDINC frame — now properly async (no more lost increments).
async fn handle_wndinc_frame(inner: &Arc<SessionInner>, wf: WndIncFrame) {
    let streams = inner.streams.lock().await;
    if let Some(stream_inner) = streams.get(&wf.stream_id) {
        handle_stream_wndinc(stream_inner, wf.increment);
        trace!(
            stream_id = wf.stream_id,
            increment = wf.increment,
            "WNDINC received"
        );
    }
}

async fn handle_goaway_frame(inner: &Arc<SessionInner>) {
    inner.remote_gone_away.store(true, Ordering::Release);
    debug!("GOAWAY received from remote");
    die(inner, ErrorCode::RemoteGoneAway, "remote sent GOAWAY").await;
}

async fn send_rst_async(inner: &Arc<SessionInner>, stream_id: u32, code: ErrorCode) {
    let frame = RstFrame::new(stream_id, code);
    let encoded = frame.encode();
    let _ = inner.frame_tx.send(encoded.freeze()).await;
}

async fn die(inner: &Arc<SessionInner>, code: ErrorCode, msg: &str) {
    if inner
        .is_dead
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    warn!(?code, msg, "session died");
    inner.dead_notify.notify_waiters();

    let mut streams = inner.streams.lock().await;
    for (_, stream_inner) in streams.drain() {
        stream_inner.mark_reset(ErrorCode::SessionClosed);
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{
        AsyncReadExt,
        AsyncWriteExt,
        DuplexStream,
    };

    use super::*;

    fn make_pipe_pair() -> (DuplexStream, DuplexStream) {
        tokio::io::duplex(64 * 1024)
    }

    fn make_session_pair(config: Config) -> (Session, Session) {
        let (client_transport, server_transport) = make_pipe_pair();
        let client = Session::client(client_transport, config.clone());
        let server = Session::server(server_transport, config);
        (client, server)
    }

    #[tokio::test]
    async fn open_and_accept_stream() {
        let (client, server) = make_session_pair(Config::default());

        let mut client_stream = client.open().await.unwrap();
        assert_eq!(client_stream.id(), 1);

        client_stream.write_all(b"hello from client").await.unwrap();

        let mut server_stream = server.accept().await.unwrap();
        assert_eq!(server_stream.id(), 1);

        let mut buf = vec![0u8; 100];
        let n = server_stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello from client");
    }

    #[tokio::test]
    async fn bidirectional_communication() {
        let (client, server) = make_session_pair(Config::default());

        let mut client_stream = client.open().await.unwrap();
        client_stream.write_all(b"ping").await.unwrap();

        let mut server_stream = server.accept().await.unwrap();
        let mut buf = vec![0u8; 100];
        let n = server_stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"ping");

        // Server responds
        server_stream.write_all(b"pong").await.unwrap();
        let n = client_stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"pong");
    }

    #[tokio::test]
    async fn multiple_streams() {
        let (client, server) = make_session_pair(Config::default());

        let mut c1 = client.open().await.unwrap();
        let mut c2 = client.open().await.unwrap();
        assert_eq!(c1.id(), 1);
        assert_eq!(c2.id(), 3);

        c1.write_all(b"stream1").await.unwrap();
        c2.write_all(b"stream2").await.unwrap();

        let s1 = server.accept().await.unwrap();
        let s2 = server.accept().await.unwrap();

        let mut buf = vec![0u8; 100];

        let (mut s1, mut s2) = if s1.id() == 1 { (s1, s2) } else { (s2, s1) };

        let n = s1.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"stream1");

        let n = s2.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"stream2");
    }

    #[tokio::test]
    async fn client_stream_ids_are_odd() {
        let (client, _server) = make_session_pair(Config::default());
        for i in 0..5 {
            let s = client.open().await.unwrap();
            assert_eq!(s.id(), 2 * i + 1);
            assert_eq!(s.id() % 2, 1);
        }
    }

    #[tokio::test]
    async fn server_can_open_streams() {
        let (client, server) = make_session_pair(Config::default());

        let mut server_stream = server.open().await.unwrap();
        assert_eq!(server_stream.id(), 2);
        assert_eq!(server_stream.id() % 2, 0);

        server_stream.write_all(b"from server").await.unwrap();

        let mut client_stream = client.accept().await.unwrap();
        let mut buf = vec![0u8; 100];
        let n = client_stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"from server");
    }

    #[tokio::test]
    async fn half_close_write() {
        let (client, server) = make_session_pair(Config::default());

        let mut client_stream = client.open().await.unwrap();
        client_stream.write_all(b"data").await.unwrap();
        client_stream.close_write();

        let mut server_stream = server.accept().await.unwrap();
        let mut buf = vec![0u8; 100];
        let n = server_stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"data");

        // Next read should return 0 (EOF)
        let n = server_stream.read(&mut buf).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn close_session_gracefully() {
        let (client, server) = make_session_pair(Config::default());

        let mut c = client.open().await.unwrap();
        c.write_all(b"hi").await.unwrap();

        let mut s = server.accept().await.unwrap();
        let mut buf = [0u8; 10];
        let n = s.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hi");

        client.close().await.unwrap();
    }

    #[tokio::test]
    async fn open_after_close_fails() {
        let (client, _server) = make_session_pair(Config::default());
        client.close().await.unwrap();

        let result = client.open().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn large_payload() {
        let (client, server) = make_session_pair(Config::default());

        let data = vec![0xAB; 100_000];
        let mut c = client.open().await.unwrap();
        c.write_all(&data).await.unwrap();
        c.close_write();

        let mut s = server.accept().await.unwrap();
        let mut received = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
        }
        assert_eq!(received.len(), 100_000);
        assert!(received.iter().all(|&b| b == 0xAB));
    }

    #[tokio::test]
    async fn drop_stream_sends_rst() {
        let (client, server) = make_session_pair(Config::default());

        {
            let mut c = client.open().await.unwrap();
            c.write_all(b"brief").await.unwrap();
        }

        let mut s = server.accept().await.unwrap();
        let mut buf = [0u8; 100];

        let mut got_data = false;
        tokio::time::timeout(std::time::Duration::from_millis(500), async {
            loop {
                match s.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if &buf[..n] == b"brief" {
                            got_data = true;
                        }
                    }
                    Err(_) => break, // RST received
                }
            }
        })
        .await
        .unwrap();

        assert!(got_data, "should have received 'brief' before RST");
    }

    #[tokio::test]
    async fn stream_reset() {
        let (client, server) = make_session_pair(Config::default());

        let mut c = client.open().await.unwrap();
        c.write_all(b"before reset").await.unwrap();
        c.reset(ErrorCode::StreamReset);

        let mut s = server.accept().await.unwrap();
        let mut buf = [0u8; 100];

        // Should eventually get an error or see data then error
        tokio::time::timeout(std::time::Duration::from_millis(500), async {
            loop {
                match s.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(_) => break, // RST received
                }
            }
        })
        .await
        .unwrap();
    }
}
