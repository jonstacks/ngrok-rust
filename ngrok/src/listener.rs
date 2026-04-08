//! Endpoint listener and stream types.

use std::{
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use bytes::{
    Buf,
    Bytes,
};
use futures::Stream;
use muxado::TypedStream;
use tokio::{
    io::{
        AsyncRead,
        AsyncWrite,
        ReadBuf,
    },
    sync::mpsc,
};
use url::Url;

use crate::{
    error::Error,
    tunnel::raw_session::ProxyStream,
};

/// An async stream of inbound connections from an ngrok endpoint.
///
/// Implements `futures::Stream` and, with the `axum` feature, `axum::serve::Listener`.
pub struct EndpointListener {
    /// The public URL assigned by ngrok cloud.
    pub(crate) url: Url,
    /// The endpoint ID.
    pub(crate) id: String,
    /// The endpoint name.
    pub(crate) name: String,
    /// The metadata.
    pub(crate) metadata: String,
    /// The protocol.
    pub(crate) protocol: String,
    /// Receiver for incoming proxy streams.
    pub(crate) accept_rx: mpsc::Receiver<ProxyStream>,
    /// Notification when the endpoint is closed.
    pub(crate) done_rx: tokio::sync::watch::Receiver<bool>,
    /// Sender to signal closure.
    pub(crate) done_tx: tokio::sync::watch::Sender<bool>,
}

impl EndpointListener {
    /// The public URL assigned by ngrok cloud.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The endpoint ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The endpoint name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The endpoint metadata.
    pub fn metadata(&self) -> &str {
        &self.metadata
    }

    /// The protocol (e.g., "https", "tcp").
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// Accept the next inbound connection.
    ///
    /// Returns `(NgrokStream, NgrokAddr)` — matching the convention of
    /// `TcpListener::accept() -> (TcpStream, SocketAddr)`.
    ///
    /// Cancellable by dropping the returned future.
    pub async fn accept(&mut self) -> Result<(NgrokStream, NgrokAddr), Error> {
        match self.accept_rx.recv().await {
            Some(proxy) => {
                let addr = NgrokAddr(proxy.header.client_addr.clone());
                let stream = NgrokStream {
                    remote_addr: proxy.header.client_addr,
                    inner: proxy.stream,
                    leftover: None,
                };
                Ok((stream, addr))
            }
            None => Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "endpoint closed",
            ))),
        }
    }

    /// A future that resolves when this endpoint has been closed.
    pub fn done(&self) -> impl Future<Output = ()> + '_ {
        let mut rx = self.done_rx.clone();
        async move {
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
    }

    /// Close the endpoint gracefully.
    pub async fn close(self) -> Result<(), Error> {
        let _ = self.done_tx.send(true);
        Ok(())
    }
}

use std::future::Future;

impl Stream for EndpointListener {
    type Item = Result<(NgrokStream, NgrokAddr), Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.accept_rx.poll_recv(cx) {
            Poll::Ready(Some(proxy)) => {
                let addr = NgrokAddr(proxy.header.client_addr.clone());
                let stream = NgrokStream {
                    remote_addr: proxy.header.client_addr,
                    inner: proxy.stream,
                    leftover: None,
                };
                Poll::Ready(Some(Ok((stream, addr))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A single inbound connection from ngrok, implementing `AsyncRead + AsyncWrite`.
pub struct NgrokStream {
    /// The remote address as reported by the ngrok proxy header.
    remote_addr: String,
    /// The underlying muxado typed stream.
    inner: TypedStream,
    /// Leftover bytes from a previous `next_bytes()` that haven't been consumed via `AsyncRead`.
    leftover: Option<Bytes>,
}

impl NgrokStream {
    /// Remote address as reported by the ngrok proxy header.
    pub fn remote_addr(&self) -> &str {
        &self.remote_addr
    }

    /// Yield the next frame payload from the underlying muxado stream.
    ///
    /// Returns the raw `bytes::Bytes` arc from the muxado `DataFrame` -- no copy into a
    /// caller buffer. The returned `Bytes` shares the same allocation as the TLS receive
    /// window; dropping it releases the reference count.
    ///
    /// Returns `None` when the remote peer closes the stream (RST or GOAWAY received).
    pub async fn next_bytes(&mut self) -> Option<Bytes> {
        self.inner.next_frame_payload().await
    }
}

impl AsyncRead for NgrokStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // First drain any leftover bytes.
        if let Some(ref mut leftover) = self.leftover {
            let to_copy = leftover.len().min(buf.remaining());
            buf.put_slice(&leftover[..to_copy]);
            leftover.advance(to_copy);
            if leftover.is_empty() {
                self.leftover = None;
            }
            return Poll::Ready(Ok(()));
        }

        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for NgrokStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// The address type used as `Listener::Addr` for the axum integration.
///
/// Wraps the public ngrok URL string (for `local_addr`) or the client IP
/// reported in the ngrok proxy header (for per-connection addresses).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NgrokAddr(pub(crate) String);

impl NgrokAddr {
    /// Create a new `NgrokAddr` from a string.
    pub fn new(addr: impl Into<String>) -> Self {
        Self(addr.into())
    }

    /// The address string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NgrokAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for NgrokAddr {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

impl From<String> for NgrokAddr {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for NgrokAddr {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for NgrokAddr {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for NgrokAddr {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for NgrokAddr {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for NgrokAddr {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

// Ensure NgrokStream is Send + Sync.
fn _assert_send_sync() {
    fn _assert<T: Send>() {}
    _assert::<NgrokStream>();
}
