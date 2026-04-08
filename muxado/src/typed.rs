use std::{
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use bytes::Bytes;
use tokio::io::{
    AsyncRead,
    AsyncReadExt,
    AsyncWrite,
    AsyncWriteExt,
    ReadBuf,
};

use crate::{
    error::MuxadoError,
    session::Session,
    stream::MuxadoStream,
};

/// A 4-byte stream type identifier, written as the first bytes of a stream.
pub type StreamType = u32;

/// An accepted stream with its type.
pub struct TypedStream {
    pub stream_type: StreamType,
    pub stream: MuxadoStream,
}

impl TypedStream {
    /// Returns the stream type.
    pub fn stream_type(&self) -> StreamType {
        self.stream_type
    }

    /// Returns the stream ID.
    pub fn id(&self) -> u32 {
        self.stream.id()
    }

    /// Half-close the write side.
    pub fn close_write(&self) {
        self.stream.close_write();
    }

    /// Yield the next frame payload as zero-copy `Bytes`.
    pub async fn next_frame_payload(&mut self) -> Option<Bytes> {
        self.stream.next_frame_payload().await
    }
}

impl AsyncRead for TypedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TypedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// A wrapper around `Session` that prefixes each stream with a 4-byte type.
pub struct TypedStreamSession {
    session: Session,
}

impl TypedStreamSession {
    /// Wrap an existing session to add typed stream support.
    pub fn new(session: Session) -> Self {
        Self { session }
    }

    /// Open a new stream with the given type. The type is written as the
    /// first 4 bytes of the stream payload.
    pub async fn open_typed(&self, stream_type: StreamType) -> Result<MuxadoStream, MuxadoError> {
        let mut stream = self.session.open().await?;
        stream
            .write_all(&stream_type.to_be_bytes())
            .await
            .map_err(MuxadoError::Io)?;
        Ok(stream)
    }

    /// Accept a remotely-initiated typed stream. Reads the first 4 bytes
    /// as the stream type.
    pub async fn accept_typed(&self) -> Result<TypedStream, MuxadoError> {
        let mut stream = self.session.accept().await?;
        let mut type_buf = [0u8; 4];
        stream
            .read_exact(&mut type_buf)
            .await
            .map_err(MuxadoError::Io)?;
        let stream_type = u32::from_be_bytes(type_buf);
        Ok(TypedStream {
            stream_type,
            stream,
        })
    }

    /// Gracefully close the underlying session.
    pub async fn close(&self) -> Result<(), MuxadoError> {
        self.session.close().await
    }

    /// Wait for the session to terminate.
    pub async fn wait(&self) {
        self.session.wait().await;
    }

    /// Returns true if the session is dead.
    pub fn is_dead(&self) -> bool {
        self.session.is_dead()
    }

    /// Access the underlying session.
    pub fn session(&self) -> &Session {
        &self.session
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{
        AsyncReadExt,
        AsyncWriteExt,
    };

    use super::*;
    use crate::config::Config;

    fn make_typed_pair(config: Config) -> (TypedStreamSession, TypedStreamSession) {
        let (c, s) = tokio::io::duplex(64 * 1024);
        let client = TypedStreamSession::new(Session::client(c, config.clone()));
        let server = TypedStreamSession::new(Session::server(s, config));
        (client, server)
    }

    #[tokio::test]
    async fn typed_stream_roundtrip() {
        let (client, server) = make_typed_pair(Config::default());

        let mut c = client.open_typed(0x42).await.unwrap();
        c.write_all(b"typed data").await.unwrap();

        let ts = server.accept_typed().await.unwrap();
        assert_eq!(ts.stream_type, 0x42);
        let mut s = ts.stream;
        let mut buf = vec![0u8; 100];
        let n = s.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"typed data");
    }

    #[tokio::test]
    async fn multiple_typed_streams() {
        let (client, server) = make_typed_pair(Config::default());

        let mut c1 = client.open_typed(1).await.unwrap();
        let mut c2 = client.open_typed(2).await.unwrap();
        c1.write_all(b"type1").await.unwrap();
        c2.write_all(b"type2").await.unwrap();

        let ts1 = server.accept_typed().await.unwrap();
        let ts2 = server.accept_typed().await.unwrap();

        // Match by type
        let (mut s1, mut s2) = if ts1.stream_type == 1 {
            (ts1.stream, ts2.stream)
        } else {
            (ts2.stream, ts1.stream)
        };

        let mut buf = vec![0u8; 100];
        let n = s1.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"type1");
        let n = s2.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"type2");
    }

    #[tokio::test]
    async fn max_stream_type_value() {
        let (client, server) = make_typed_pair(Config::default());

        let mut c = client.open_typed(0xFFFF_FFFF).await.unwrap();
        c.write_all(b"x").await.unwrap();

        let ts = server.accept_typed().await.unwrap();
        assert_eq!(ts.stream_type, 0xFFFF_FFFF);
    }
}
