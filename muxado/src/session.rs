//! Muxado session implementation.

use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    task::Waker,
};

use bytes::Bytes;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};
use tracing::{debug, warn};

use crate::{
    error::{ErrorCode, MuxadoError},
    frame::{
        data::DataFlags, read_frame, write_frame, DataFrame, Frame, GoAwayFrame, RstFrame,
        WndIncFrame,
    },
};

/// Initial per-stream send window size (256 KB).
const INITIAL_WINDOW: u32 = 256 * 1024;
/// Capacity of the accept backlog channel.
const ACCEPT_BACKLOG: usize = 64;
/// Capacity of the outbound write channel.
const WRITE_CHAN_CAP: usize = 256;

/// Session role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The side that initiates odd-numbered stream IDs.
    Client,
    /// The side that initiates even-numbered stream IDs.
    Server,
}

/// Per-stream inner state shared between Stream handles.
struct StreamInner {
    /// Stream identifier.
    id: u32,
    /// Buffered inbound data.
    inbound: Mutex<VecDeque<Bytes>>,
    /// True once the remote has closed the write side (FIN received).
    read_closed: AtomicBool,
    /// Waker registered by `poll_read`, woken when data or EOF arrives.
    read_waker: Mutex<Option<Waker>>,
    /// True if the stream has been reset.
    rst: AtomicBool,
    /// Error code from RST, if any.
    rst_code: Mutex<Option<ErrorCode>>,
    /// True once SYN has been sent.
    syn_sent: AtomicBool,
    /// True once FIN has been sent.
    fin_sent: AtomicBool,
    /// Remaining send window in bytes.
    send_window: AtomicU32,
    /// Waker registered by `poll_write`, woken when the window opens.
    write_waker: Mutex<Option<Waker>>,
    /// Channel to queue frames to the session writer task.
    write_tx: mpsc::Sender<Frame>,
}

/// A bidirectional stream within a muxado session.
#[derive(Clone)]
pub struct Stream(Arc<StreamInner>);

impl Stream {
    /// Returns the stream's numeric identifier.
    pub fn stream_id(&self) -> u32 {
        self.0.id
    }

    /// Gracefully close the write side of this stream (send FIN).
    pub async fn close_write(&self) -> Result<(), MuxadoError> {
        if self.0.rst.load(Ordering::Acquire) {
            return Err(MuxadoError::WriteAfterClose);
        }
        if self.0.fin_sent.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let mut flags = DataFlags::FIN;
        if !self.0.syn_sent.swap(true, Ordering::AcqRel) {
            flags |= DataFlags::SYN;
        }
        let frame = Frame::Data(DataFrame {
            stream_id: self.0.id,
            flags,
            payload: Bytes::new(),
        });
        self.0
            .write_tx
            .send(frame)
            .await
            .map_err(|_| MuxadoError::SessionClosed)?;
        Ok(())
    }

    /// Deliver inbound data from the session reader.
    pub(crate) fn push_data(&self, data: Bytes, fin: bool) {
        {
            let mut q = self.0.inbound.lock().unwrap_or_else(|e| e.into_inner());
            if !data.is_empty() {
                q.push_back(data);
            }
        }
        if fin {
            self.0.read_closed.store(true, Ordering::Release);
        }
        if let Some(waker) = self.0.read_waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            waker.wake();
        }
    }

    /// Mark the stream as reset.
    pub(crate) fn set_rst(&self, code: ErrorCode) {
        *self.0.rst_code.lock().unwrap_or_else(|e| e.into_inner()) = Some(code);
        self.0.rst.store(true, Ordering::Release);
        self.0.read_closed.store(true, Ordering::Release);
        if let Some(waker) = self.0.read_waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            waker.wake();
        }
        if let Some(waker) = self.0.write_waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            waker.wake();
        }
    }

    /// Grant additional send window (from WNDINC frame).
    pub(crate) fn add_window(&self, inc: u32) {
        self.0.send_window.fetch_add(inc, Ordering::AcqRel);
        if let Some(waker) = self.0.write_waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            waker.wake();
        }
    }
}

impl tokio::io::AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        loop {
            if self.0.rst.load(Ordering::Acquire) {
                return std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "stream reset by remote",
                )));
            }
            {
                let mut q = self.0.inbound.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(chunk) = q.front().cloned() {
                    let n = chunk.len().min(buf.remaining());
                    buf.put_slice(&chunk[..n]);
                    if n == chunk.len() {
                        q.pop_front();
                    } else {
                        let remainder = chunk.slice(n..);
                        *q.front_mut().expect("front exists after clone") = remainder;
                    }
                    return std::task::Poll::Ready(Ok(()));
                }
            }
            if self.0.read_closed.load(Ordering::Acquire) {
                return std::task::Poll::Ready(Ok(())); // EOF
            }
            // Register waker, then re-check to avoid lost-wakeup
            *self.0.read_waker.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(cx.waker().clone());
            {
                let q = self.0.inbound.lock().unwrap_or_else(|e| e.into_inner());
                if !q.is_empty() {
                    continue;
                }
            }
            if self.0.read_closed.load(Ordering::Acquire)
                || self.0.rst.load(Ordering::Acquire)
            {
                continue;
            }
            return std::task::Poll::Pending;
        }
    }
}

impl tokio::io::AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        if self.0.rst.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream reset",
            )));
        }
        if self.0.fin_sent.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write after fin",
            )));
        }
        if buf.is_empty() {
            return std::task::Poll::Ready(Ok(0));
        }
        // Check flow control window
        let window = self.0.send_window.load(Ordering::Acquire);
        if window == 0 {
            *self.0.write_waker.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(cx.waker().clone());
            // Re-check after storing waker
            if self.0.send_window.load(Ordering::Acquire) == 0 {
                return std::task::Poll::Pending;
            }
        }
        let n = buf.len().min(window as usize).min(INITIAL_WINDOW as usize);
        self.0.send_window.fetch_sub(n as u32, Ordering::AcqRel);
        let mut flags = DataFlags::empty();
        if !self.0.syn_sent.swap(true, Ordering::AcqRel) {
            flags |= DataFlags::SYN;
        }
        let payload = Bytes::copy_from_slice(&buf[..n]);
        let frame = Frame::Data(DataFrame {
            stream_id: self.0.id,
            flags,
            payload,
        });
        match self.0.write_tx.try_send(frame) {
            Ok(_) => std::task::Poll::Ready(Ok(n)),
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Restore window and retry later
                self.0.send_window.fetch_add(n as u32, Ordering::AcqRel);
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "session closed",
                )))
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        if self.0.fin_sent.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Ok(()));
        }
        if self.0.rst.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream reset",
            )));
        }
        if self.0.fin_sent.swap(true, Ordering::AcqRel) {
            return std::task::Poll::Ready(Ok(()));
        }
        let mut flags = DataFlags::FIN;
        if !self.0.syn_sent.swap(true, Ordering::AcqRel) {
            flags |= DataFlags::SYN;
        }
        let frame = Frame::Data(DataFrame {
            stream_id: self.0.id,
            flags,
            payload: Bytes::new(),
        });
        match self.0.write_tx.try_send(frame) {
            Ok(_) => std::task::Poll::Ready(Ok(())),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.0.fin_sent.store(false, Ordering::Release);
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "session closed",
                )))
            }
        }
    }
}

/// Shared inner state of a muxado session.
struct SessionInner {
    /// Channel to send frames to the writer task.
    write_tx: mpsc::Sender<Frame>,
    /// Channel to receive incoming streams (tokio mutex for async-safe access).
    accept_rx: tokio::sync::Mutex<mpsc::Receiver<Stream>>,
    /// Map of active streams by ID.
    stream_map: Mutex<HashMap<u32, Stream>>,
    /// Monotonically increasing stream ID counter.
    next_id: AtomicU32,
    /// True once the session is closed.
    closed: AtomicBool,
    /// Termination reason.
    termination: Mutex<Option<MuxadoError>>,
    /// Notified on termination.
    term_notify: tokio::sync::Notify,
}

/// A muxado session.
#[derive(Clone)]
pub struct Session(Arc<SessionInner>);

impl Session {
    /// Create a new client-side session over the given I/O transport.
    pub fn client<T>(transport: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        Self::new(transport, Role::Client)
    }

    /// Create a new server-side session over the given I/O transport.
    pub fn server<T>(transport: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        Self::new(transport, Role::Server)
    }

    fn new<T>(transport: T, role: Role) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read_half, write_half) = tokio::io::split(transport);
        let (write_tx, write_rx) = mpsc::channel::<Frame>(WRITE_CHAN_CAP);
        let (accept_tx, accept_rx) = mpsc::channel::<Stream>(ACCEPT_BACKLOG);

        let first_id = match role {
            Role::Client => 1u32,
            Role::Server => 2u32,
        };

        let inner = Arc::new(SessionInner {
            write_tx: write_tx.clone(),
            accept_rx: tokio::sync::Mutex::new(accept_rx),
            stream_map: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(first_id),
            closed: AtomicBool::new(false),
            termination: Mutex::new(None),
            term_notify: tokio::sync::Notify::new(),
        });

        let write_inner = inner.clone();
        tokio::spawn(writer_task(write_half, write_rx, write_inner));

        let read_inner = inner.clone();
        tokio::spawn(reader_task(read_half, read_inner, write_tx, accept_tx));

        Session(inner)
    }

    /// Open a new outbound stream.
    pub async fn open_stream(&self) -> Result<Stream, MuxadoError> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(MuxadoError::SessionClosed);
        }
        let id = self.0.next_id.fetch_add(2, Ordering::AcqRel);
        if id > 0x7FFF_FFFF {
            return Err(MuxadoError::StreamsExhausted);
        }
        let stream = make_stream(id, self.0.write_tx.clone());
        self.0
            .stream_map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, stream.clone());
        Ok(stream)
    }

    /// Accept the next inbound stream.
    pub async fn accept_stream(&self) -> Result<Stream, MuxadoError> {
        let mut rx = self.0.accept_rx.lock().await;
        tokio::select! {
            stream = rx.recv() => {
                stream.ok_or(MuxadoError::SessionClosed)
            }
            _ = self.0.term_notify.notified() => {
                Err(MuxadoError::SessionClosed)
            }
        }
    }

    /// Close the session with the given error code.
    pub async fn close(&self, code: ErrorCode) -> Result<(), MuxadoError> {
        self.0.closed.store(true, Ordering::Release);
        let frame = Frame::GoAway(GoAwayFrame {
            last_stream_id: 0,
            error_code: code,
            debug_data: Bytes::new(),
        });
        self.0
            .write_tx
            .send(frame)
            .await
            .map_err(|_| MuxadoError::SessionClosed)?;
        Ok(())
    }

    /// Wait for the session to terminate.
    pub async fn wait(&self) {
        self.0.term_notify.notified().await;
    }
}

fn make_stream(id: u32, write_tx: mpsc::Sender<Frame>) -> Stream {
    Stream(Arc::new(StreamInner {
        id,
        inbound: Mutex::new(VecDeque::new()),
        read_closed: AtomicBool::new(false),
        read_waker: Mutex::new(None),
        rst: AtomicBool::new(false),
        rst_code: Mutex::new(None),
        syn_sent: AtomicBool::new(false),
        fin_sent: AtomicBool::new(false),
        send_window: AtomicU32::new(INITIAL_WINDOW),
        write_waker: Mutex::new(None),
        write_tx,
    }))
}

async fn writer_task<W>(
    mut writer: tokio::io::WriteHalf<W>,
    mut rx: mpsc::Receiver<Frame>,
    inner: Arc<SessionInner>,
) where
    W: AsyncWrite,
{
    while let Some(frame) = rx.recv().await {
        let is_goaway = matches!(frame, Frame::GoAway(_));
        if let Err(e) = write_frame(&mut writer, &frame).await {
            warn!("muxado writer error: {e}");
            terminate_session(&inner, e);
            return;
        }
        if is_goaway {
            break;
        }
    }
}

async fn reader_task<R>(
    mut reader: tokio::io::ReadHalf<R>,
    inner: Arc<SessionInner>,
    write_tx: mpsc::Sender<Frame>,
    accept_tx: mpsc::Sender<Stream>,
) where
    R: AsyncRead,
{
    loop {
        match read_frame(&mut reader).await {
            Ok(Frame::Data(df)) => {
                let fin = df.flags.contains(DataFlags::FIN);
                let syn = df.flags.contains(DataFlags::SYN);
                if syn {
                    let stream = make_stream(df.stream_id, write_tx.clone());
                    inner
                        .stream_map
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(df.stream_id, stream.clone());
                    stream.push_data(df.payload, fin);
                    if accept_tx.send(stream).await.is_err() {
                        debug!("accept channel closed");
                    }
                } else if let Some(stream) = inner
                    .stream_map
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&df.stream_id)
                    .cloned()
                {
                    stream.push_data(df.payload, fin);
                    if fin {
                        inner
                            .stream_map
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&df.stream_id);
                    }
                }
            }
            Ok(Frame::Rst(rf)) => {
                if let Some(stream) = inner
                    .stream_map
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&rf.stream_id)
                {
                    stream.set_rst(rf.error_code);
                }
            }
            Ok(Frame::WndInc(wf)) => {
                if let Some(stream) = inner
                    .stream_map
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&wf.stream_id)
                    .cloned()
                {
                    stream.add_window(wf.increment);
                }
            }
            Ok(Frame::GoAway(gf)) => {
                terminate_session(&inner, MuxadoError::RemoteGoneAway(gf.error_code));
                return;
            }
            Err(MuxadoError::Io(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                terminate_session(&inner, MuxadoError::SessionClosed);
                return;
            }
            Err(e) => {
                warn!("muxado reader error: {e}");
                terminate_session(&inner, e);
                return;
            }
        }
    }
}

fn terminate_session(inner: &Arc<SessionInner>, err: MuxadoError) {
    inner.closed.store(true, Ordering::Release);
    let streams: Vec<Stream> = inner
        .stream_map
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .map(|(_, s)| s)
        .collect();
    for s in streams {
        s.set_rst(ErrorCode::SessionClosed);
    }
    *inner.termination.lock().unwrap_or_else(|e| e.into_inner()) = Some(err);
    inner.term_notify.notify_waiters();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_bidirectional_echo() {
        let (client_io, server_io) = tokio::io::duplex(65536);

        let client = Session::client(client_io);
        let server = Session::server(server_io);

        let server_task = tokio::spawn(async move {
            let mut stream = server.accept_stream().await.unwrap();
            let mut buf = vec![0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(&buf).await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let mut stream = client.open_stream().await.unwrap();
        stream.write_all(b"hello").await.unwrap();
        stream.shutdown().await.unwrap();

        let mut buf = vec![0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        server_task.await.unwrap();
    }
}


use crate::{
    error::{ErrorCode, MuxadoError},
    frame::{
        data::DataFlags, read_frame, write_frame, DataFrame, Frame, GoAwayFrame, RstFrame,
        WndIncFrame,
    },
};

/// Initial per-stream send window size (256 KB).
const INITIAL_WINDOW: u32 = 256 * 1024;
/// Capacity of the accept backlog channel.
const ACCEPT_BACKLOG: usize = 64;
/// Capacity of the writer channel.
const WRITE_CHAN_CAP: usize = 256;

/// Session role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The side that initiates odd-numbered stream IDs.
    Client,
    /// The side that initiates even-numbered stream IDs.
    Server,
}

/// Per-stream inner state shared between Stream handles.
struct StreamInner {
    /// Stream identifier.
    id: u32,
    /// Buffered inbound data.
    inbound: Mutex<std::collections::VecDeque<Bytes>>,
    /// Notified when new data arrives or the read side closes.
    inbound_notify: Notify,
    /// Semaphore limiting outbound bytes (flow control).
    outbound_window: Arc<Semaphore>,
    /// True once SYN has been sent.
    syn_sent: AtomicBool,
    /// True once FIN has been sent.
    fin_sent: AtomicBool,
    /// True if the stream has been reset.
    rst: AtomicBool,
    /// Error code from RST, if any.
    rst_code: Mutex<Option<ErrorCode>>,
    /// True once the remote has closed the write side (FIN received).
    read_closed: AtomicBool,
    /// Channel to queue frames to the session writer task.
    write_tx: mpsc::Sender<Frame>,
}

/// A bidirectional stream within a muxado session.
#[derive(Clone)]
pub struct Stream(Arc<StreamInner>);

impl Stream {
    /// Returns the stream's numeric identifier.
    pub fn stream_id(&self) -> u32 {
        self.0.id
    }

    /// Gracefully close the write side of this stream (send FIN).
    pub async fn close_write(&self) -> Result<(), MuxadoError> {
        if self.0.rst.load(Ordering::Acquire) {
            return Err(MuxadoError::WriteAfterClose);
        }
        if self.0.fin_sent.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let mut flags = DataFlags::FIN;
        if !self.0.syn_sent.swap(true, Ordering::AcqRel) {
            flags |= DataFlags::SYN;
        }
        let frame = Frame::Data(DataFrame {
            stream_id: self.0.id,
            flags,
            payload: Bytes::new(),
        });
        self.0
            .write_tx
            .send(frame)
            .await
            .map_err(|_| MuxadoError::SessionClosed)?;
        Ok(())
    }

    /// Deliver inbound data from the session reader.
    pub(crate) fn push_data(&self, data: Bytes, fin: bool) {
        {
            let mut q = self.0.inbound.lock().unwrap_or_else(|e| e.into_inner());
            if !data.is_empty() {
                q.push_back(data);
            }
        }
        if fin {
            self.0.read_closed.store(true, Ordering::Release);
        }
        self.0.inbound_notify.notify_waiters();
    }

    /// Mark the stream as reset.
    pub(crate) fn set_rst(&self, code: ErrorCode) {
        *self.0.rst_code.lock().unwrap_or_else(|e| e.into_inner()) = Some(code);
        self.0.rst.store(true, Ordering::Release);
        self.0.read_closed.store(true, Ordering::Release);
        self.0.inbound_notify.notify_waiters();
    }

    /// Grant additional send window (from WNDINC frame).
    pub(crate) fn add_window(&self, inc: u32) {
        self.0.outbound_window.add_permits(inc as usize);
    }
}

impl tokio::io::AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        loop {
            // Check for RST
            if self.0.rst.load(Ordering::Acquire) {
                return std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "stream reset by remote",
                )));
            }
            // Try to pop from inbound buffer
            {
                let mut q = self.0.inbound.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(chunk) = q.front().cloned() {
                    let n = chunk.len().min(buf.remaining());
                    buf.put_slice(&chunk[..n]);
                    if n == chunk.len() {
                        q.pop_front();
                    } else {
                        // Replace front with the remainder
                        let remainder = chunk.slice(n..);
                        *q.front_mut().expect("front exists") = remainder;
                    }
                    return std::task::Poll::Ready(Ok(()));
                }
            }
            // Buffer empty - check if read side is closed (EOF)
            if self.0.read_closed.load(Ordering::Acquire) {
                return std::task::Poll::Ready(Ok(())); // EOF
            }
            // Register waker and wait
            let notify = &self.0.inbound_notify;
            let mut fut = std::pin::pin!(notify.notified());
            match fut.as_mut().poll(cx) {
                std::task::Poll::Ready(_) => continue,
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }
}

impl tokio::io::AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        if self.0.rst.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream reset",
            )));
        }
        if self.0.fin_sent.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write after fin",
            )));
        }
        let n = buf.len().min(INITIAL_WINDOW as usize);
        if n == 0 {
            return std::task::Poll::Ready(Ok(0));
        }
        // Acquire a permit for each byte we intend to send
        let sem = self.0.outbound_window.clone();
        let mut acquire = std::pin::pin!(sem.acquire_many(n as u32));
        match acquire.as_mut().poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Ok(permit)) => {
                permit.forget();
                let mut flags = DataFlags::empty();
                if !self.0.syn_sent.swap(true, Ordering::AcqRel) {
                    flags |= DataFlags::SYN;
                }
                let payload = Bytes::copy_from_slice(&buf[..n]);
                let frame = Frame::Data(DataFrame {
                    stream_id: self.0.id,
                    flags,
                    payload,
                });
                match self.0.write_tx.try_send(frame) {
                    Ok(_) => std::task::Poll::Ready(Ok(n)),
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // Channel full - add back permits and return pending
                        self.0.outbound_window.add_permits(n);
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        std::task::Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "session closed",
                        )))
                    }
                }
            }
            std::task::Poll::Ready(Err(_)) => std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "semaphore closed",
            ))),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        if self.0.fin_sent.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Ok(()));
        }
        if self.0.rst.load(Ordering::Acquire) {
            return std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream reset",
            )));
        }
        // Send a FIN frame by constructing and queuing it directly
        let mut flags = DataFlags::FIN;
        if !self.0.syn_sent.swap(true, Ordering::AcqRel) {
            flags |= DataFlags::SYN;
        }
        // Mark fin as sent before trying to send to avoid double-FIN races
        if self.0.fin_sent.swap(true, Ordering::AcqRel) {
            return std::task::Poll::Ready(Ok(()));
        }
        let frame = Frame::Data(DataFrame {
            stream_id: self.0.id,
            flags,
            payload: Bytes::new(),
        });
        match self.0.write_tx.try_send(frame) {
            Ok(_) => std::task::Poll::Ready(Ok(())),
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Reset fin_sent so we can retry
                self.0.fin_sent.store(false, Ordering::Release);
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                std::task::Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "session closed",
                )))
            }
        }
    }
}

/// Shared inner state of a muxado session.
struct SessionInner {
    /// Channel to send frames to the writer task.
    write_tx: mpsc::Sender<Frame>,
    /// Channel to receive incoming streams (tokio mutex for async-safe access).
    accept_rx: tokio::sync::Mutex<mpsc::Receiver<Stream>>,
    /// Map of active streams by ID.
    stream_map: Mutex<HashMap<u32, Stream>>,
    /// Monotonically increasing stream ID counter.
    next_id: AtomicU32,
    /// True once the session is closed.
    closed: AtomicBool,
    /// Termination reason.
    termination: Mutex<Option<MuxadoError>>,
    /// Notified on termination.
    term_notify: Notify,
}

/// A muxado session.
#[derive(Clone)]
pub struct Session(Arc<SessionInner>);

impl Session {
    /// Create a new client-side session over the given I/O transport.
    pub fn client<T>(transport: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        Self::new(transport, Role::Client)
    }

    /// Create a new server-side session over the given I/O transport.
    pub fn server<T>(transport: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        Self::new(transport, Role::Server)
    }

    fn new<T>(transport: T, role: Role) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read_half, write_half) = tokio::io::split(transport);
        let (write_tx, write_rx) = mpsc::channel::<Frame>(WRITE_CHAN_CAP);
        let (accept_tx, accept_rx) = mpsc::channel::<Stream>(ACCEPT_BACKLOG);

        let first_id = match role {
            Role::Client => 1u32,
            Role::Server => 2u32,
        };

        let inner = Arc::new(SessionInner {
            write_tx: write_tx.clone(),
            accept_rx: tokio::sync::Mutex::new(accept_rx),
            stream_map: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(first_id),
            closed: AtomicBool::new(false),
            termination: Mutex::new(None),
            term_notify: Notify::new(),
        });

        // Spawn writer task
        let write_inner = inner.clone();
        tokio::spawn(writer_task(write_half, write_rx, write_inner));

        // Spawn reader task
        let read_inner = inner.clone();
        tokio::spawn(reader_task(read_half, read_inner, write_tx, accept_tx));

        Session(inner)
    }

    /// Open a new outbound stream.
    pub async fn open_stream(&self) -> Result<Stream, MuxadoError> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(MuxadoError::SessionClosed);
        }
        let id = self.0.next_id.fetch_add(2, Ordering::AcqRel);
        if id > 0x7FFF_FFFF {
            return Err(MuxadoError::StreamsExhausted);
        }
        let stream = make_stream(id, self.0.write_tx.clone());
        self.0
            .stream_map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, stream.clone());
        Ok(stream)
    }

    /// Accept the next inbound stream.
    pub async fn accept_stream(&self) -> Result<Stream, MuxadoError> {
        let mut rx = self.0.accept_rx.lock().await;
        tokio::select! {
            stream = rx.recv() => {
                stream.ok_or(MuxadoError::SessionClosed)
            }
            _ = self.0.term_notify.notified() => {
                Err(MuxadoError::SessionClosed)
            }
        }
    }

    /// Close the session with the given error code.
    pub async fn close(&self, code: ErrorCode) -> Result<(), MuxadoError> {
        self.0.closed.store(true, Ordering::Release);
        let frame = Frame::GoAway(GoAwayFrame {
            last_stream_id: 0,
            error_code: code,
            debug_data: Bytes::new(),
        });
        self.0
            .write_tx
            .send(frame)
            .await
            .map_err(|_| MuxadoError::SessionClosed)?;
        Ok(())
    }

    /// Wait for the session to terminate.
    pub async fn wait(&self) {
        self.0.term_notify.notified().await;
    }
}

fn make_stream(id: u32, write_tx: mpsc::Sender<Frame>) -> Stream {
    Stream(Arc::new(StreamInner {
        id,
        inbound: Mutex::new(std::collections::VecDeque::new()),
        inbound_notify: Notify::new(),
        outbound_window: Arc::new(Semaphore::new(INITIAL_WINDOW as usize)),
        syn_sent: AtomicBool::new(false),
        fin_sent: AtomicBool::new(false),
        rst: AtomicBool::new(false),
        rst_code: Mutex::new(None),
        read_closed: AtomicBool::new(false),
        write_tx,
    }))
}

async fn writer_task<W: AsyncWrite + Unpin>(
    mut writer: WriteHalf<W>,
    mut rx: mpsc::Receiver<Frame>,
    inner: Arc<SessionInner>,
) {
    while let Some(frame) = rx.recv().await {
        let is_goaway = matches!(frame, Frame::GoAway(_));
        if let Err(e) = write_frame(&mut writer, &frame).await {
            warn!("muxado writer error: {e}");
            terminate_session(&inner, MuxadoError::Io(e));
            return;
        }
        if is_goaway {
            break;
        }
    }
}

async fn reader_task<R: AsyncRead + Unpin>(
    mut reader: ReadHalf<R>,
    inner: Arc<SessionInner>,
    write_tx: mpsc::Sender<Frame>,
    accept_tx: mpsc::Sender<Stream>,
) {
    loop {
        match read_frame(&mut reader).await {
            Ok(Frame::Data(df)) => {
                let fin = df.flags.contains(DataFlags::FIN);
                let syn = df.flags.contains(DataFlags::SYN);
                if syn {
                    let stream = make_stream(df.stream_id, write_tx.clone());
                    inner
                        .stream_map
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(df.stream_id, stream.clone());
                    stream.push_data(df.payload, fin);
                    if accept_tx.send(stream).await.is_err() {
                        debug!("accept channel closed");
                    }
                } else if let Some(stream) = inner
                    .stream_map
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&df.stream_id)
                    .cloned()
                {
                    stream.push_data(df.payload, fin);
                    if fin {
                        inner
                            .stream_map
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&df.stream_id);
                    }
                }
            }
            Ok(Frame::Rst(rf)) => {
                if let Some(stream) = inner
                    .stream_map
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&rf.stream_id)
                {
                    stream.set_rst(rf.error_code);
                }
            }
            Ok(Frame::WndInc(wf)) => {
                if let Some(stream) = inner
                    .stream_map
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&wf.stream_id)
                    .cloned()
                {
                    stream.add_window(wf.increment);
                }
            }
            Ok(Frame::GoAway(gf)) => {
                terminate_session(&inner, MuxadoError::RemoteGoneAway(gf.error_code));
                return;
            }
            Err(MuxadoError::Io(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                terminate_session(&inner, MuxadoError::SessionClosed);
                return;
            }
            Err(e) => {
                warn!("muxado reader error: {e}");
                terminate_session(&inner, e);
                return;
            }
        }
    }
}

fn terminate_session(inner: &Arc<SessionInner>, err: MuxadoError) {
    inner.closed.store(true, Ordering::Release);
    let streams: Vec<Stream> = inner
        .stream_map
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .map(|(_, s)| s)
        .collect();
    for s in streams {
        s.set_rst(ErrorCode::SessionClosed);
    }
    *inner.termination.lock().unwrap_or_else(|e| e.into_inner()) = Some(err);
    inner.term_notify.notify_waiters();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_bidirectional_echo() {
        let (client_io, server_io) = tokio::io::duplex(65536);

        let client = Session::client(client_io);
        let server = Session::server(server_io);

        let server_task = tokio::spawn(async move {
            let mut stream = server.accept_stream().await.unwrap();
            let mut buf = vec![0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(&buf).await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let mut stream = client.open_stream().await.unwrap();
        stream.write_all(b"hello").await.unwrap();
        stream.shutdown().await.unwrap();

        let mut buf = vec![0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        server_task.await.unwrap();
    }
}
