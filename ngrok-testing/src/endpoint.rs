//! Mock endpoint for injecting test connections.

use std::sync::Arc;

use muxado::TypedStreamSession;
use tokio::io::{
    AsyncRead,
    AsyncWrite,
    AsyncWriteExt,
};

/// A mock endpoint representing a bound tunnel on the mock server.
pub struct MockEndpoint {
    /// The endpoint ID.
    pub id: String,
    /// The public URL.
    pub url: String,
    /// The protocol.
    pub proto: String,
    /// The server's typed stream session for injecting proxy connections.
    pub(crate) typed: Arc<TypedStreamSession>,
}

impl MockEndpoint {
    /// Inject a test connection into this endpoint.
    ///
    /// Opens a new muxado stream, writes a ProxyHeader, then pipes the
    /// provided stream to the muxado stream bidirectionally.
    pub async fn inject_connection(
        &self,
        stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
    ) {
        let header = serde_json::json!({
            "Id": self.id,
            "ClientAddr": "127.0.0.1:12345",
            "Proto": self.proto,
            "EdgeType": "cloud",
            "PassthroughTls": false
        });

        let mut typed_stream = match self.typed.open_typed(3).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "failed to open proxy stream");
                return;
            }
        };

        // Write proxy header with 8-byte little-endian length prefix (matches
        // the format expected by handle_proxy in raw_session.rs).
        let header_json = serde_json::to_vec(&header).unwrap();
        let len = header_json.len() as i64;
        if typed_stream.write_all(&len.to_le_bytes()).await.is_err() {
            return;
        }
        if typed_stream.write_all(&header_json).await.is_err() {
            return;
        }
        // Flush to ensure the proxy header is sent through the muxado session
        // before we hand the stream off to the bidirectional copy tasks.
        if typed_stream.flush().await.is_err() {
            return;
        }

        // Bidirectional copy between the injected stream and the muxado stream.
        let (mut stream_read, mut stream_write) = tokio::io::split(stream);
        let (mut mux_read, mut mux_write) = tokio::io::split(typed_stream);

        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stream_read, &mut mux_write).await;
        });
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut mux_read, &mut stream_write).await;
        });
    }
}
