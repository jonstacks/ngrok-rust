//! Mock ngrok server for offline testing.

use std::{
    collections::HashMap,
    sync::Arc,
};

use muxado::{
    Session,
    SessionConfig,
    TypedStreamSession,
};
use tokio::{
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
    net::TcpListener,
    sync::{
        Mutex,
        RwLock,
        mpsc,
    },
};
use tokio_rustls::TlsAcceptor;

use crate::{
    endpoint::MockEndpoint,
    fixtures::TestCertificate,
};

/// An in-process mock ngrok server for offline testing.
///
/// Speaks the ngrok muxado + JSON protocol without requiring a real authtoken
/// or network connection.
pub struct MockNgrokServer {
    /// The endpoint receiver.
    endpoint_rx: Mutex<mpsc::Receiver<MockEndpoint>>,
    /// The endpoint sender (for internal use).
    #[allow(dead_code)]
    endpoint_tx: mpsc::Sender<MockEndpoint>,
    /// The active server-side typed stream session.
    typed: Arc<RwLock<Option<Arc<TypedStreamSession>>>>,
    /// Map of RPC method names to stream type codes.
    rpc_types: HashMap<String, u32>,
    /// The TCP listener address.
    #[allow(dead_code)]
    addr: String,
}

impl MockNgrokServer {
    /// Start the mock server.
    ///
    /// Returns `(server, connect_url)` where `connect_url` can be passed to
    /// `AgentBuilder::connect_url()`.
    pub async fn start() -> (Arc<Self>, String) {
        let cert = TestCertificate::generate();

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind mock server");
        let addr = listener.local_addr().unwrap();
        let connect_url = format!("127.0.0.1:{}", addr.port());

        let acceptor = TlsAcceptor::from(Arc::new(cert.server_config));
        let (endpoint_tx, endpoint_rx) = mpsc::channel(32);

        let server = Arc::new(Self {
            endpoint_rx: Mutex::new(endpoint_rx),
            endpoint_tx: endpoint_tx.clone(),
            typed: Arc::new(RwLock::new(None)),
            rpc_types: HashMap::from([
                ("StopAgent".into(), 5),
                ("RestartAgent".into(), 4),
                ("UpdateAgent".into(), 6),
            ]),
            addr: connect_url.clone(),
        });

        let server_clone = server.clone();
        tokio::spawn(async move {
            accept_loop(listener, acceptor, endpoint_tx, server_clone).await;
        });

        (server, connect_url)
    }

    /// Returns the next endpoint that the test agent has bound.
    ///
    /// Blocks until a Bind request arrives.
    pub async fn accept_endpoint(&self) -> MockEndpoint {
        let mut rx = self.endpoint_rx.lock().await;
        rx.recv().await.expect("mock server closed")
    }

    /// Send a server-initiated RPC command.
    pub async fn send_rpc(&self, method: &str) {
        let typed = self.typed.read().await;
        if let Some(ref typed) = *typed {
            let stream_type = self.rpc_types.get(method).copied().unwrap_or(5);
            if let Ok(mut stream) = typed.open_typed(stream_type).await {
                let _ = stream.write_all(b"{}").await;
                stream.close_write();
            }
        }
    }

    /// Simulate a transport drop (tests reconnect logic).
    pub fn drop_session(&self) {
        let typed = self.typed.clone();
        tokio::spawn(async move {
            let mut guard = typed.write().await;
            if let Some(t) = guard.take() {
                let _ = t.close().await;
            }
        });
    }
}

/// Accept loop for the mock server.
async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    endpoint_tx: mpsc::Sender<MockEndpoint>,
    server: Arc<MockNgrokServer>,
) {
    loop {
        let (tcp, _) = match listener.accept().await {
            Ok(c) => c,
            Err(_) => break,
        };

        let tls = match acceptor.accept(tcp).await {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(error = %e, "TLS accept failed in mock server");
                continue;
            }
        };

        // Create muxado server session.
        let session = Session::server(tls, SessionConfig::default());
        let typed = Arc::new(TypedStreamSession::new(session));

        // Store for RPC sending.
        {
            let mut guard = server.typed.write().await;
            *guard = Some(typed.clone());
        }

        let endpoint_tx = endpoint_tx.clone();
        tokio::spawn(async move {
            handle_client(typed, endpoint_tx).await;
        });
    }
}

/// Handle a single client connection on the mock server.
async fn handle_client(typed: Arc<TypedStreamSession>, endpoint_tx: mpsc::Sender<MockEndpoint>) {
    let typed_for_endpoints = typed.clone();

    loop {
        let mut stream = match typed.accept_typed().await {
            Ok(s) => s,
            Err(_) => break,
        };

        let stream_type = stream.stream_type;
        match stream_type {
            // Auth request (type 0)
            0 => {
                let mut buf = Vec::new();
                if stream.read_to_end(&mut buf).await.is_err() {
                    continue;
                }

                let session_id = uuid::Uuid::new_v4().to_string();
                let resp = serde_json::json!({
                    "Version": "3",
                    "Extra": {
                        "Region": "us",
                        "AgentSessionID": session_id,
                        "Cookie": format!("cookie-{}", session_id),
                        "ConnectAddresses": []
                    },
                    "Error": ""
                });

                let resp_json = serde_json::to_vec(&resp).unwrap();
                let _ = stream.write_all(&resp_json).await;
                stream.close_write();
            }

            // Bind request (type 1)
            1 => {
                let mut buf = Vec::new();
                if stream.read_to_end(&mut buf).await.is_err() {
                    continue;
                }

                let bind_req: serde_json::Value = serde_json::from_slice(&buf).unwrap_or_default();

                let endpoint_id = format!("ep_{}", uuid::Uuid::new_v4());
                let proto = bind_req
                    .get("Proto")
                    .and_then(|v| v.as_str())
                    .unwrap_or("https");

                let url = format!("https://{}.mock.ngrok.app", &endpoint_id[3..11]);

                let resp = serde_json::json!({
                    "Id": endpoint_id,
                    "URL": url,
                    "Proto": proto,
                    "Error": ""
                });

                let resp_json = serde_json::to_vec(&resp).unwrap();
                let _ = stream.write_all(&resp_json).await;
                stream.close_write();

                // Create MockEndpoint and send it.
                let mock_ep = MockEndpoint {
                    id: endpoint_id,
                    url: url.to_string(),
                    proto: proto.to_string(),
                    typed: typed_for_endpoints.clone(),
                };
                let _ = endpoint_tx.send(mock_ep).await;
            }

            // SrvInfo request (type 8)
            8 => {
                let mut buf = Vec::new();
                let _ = stream.read_to_end(&mut buf).await;
                let resp = serde_json::json!({ "Region": "us" });
                let resp_json = serde_json::to_vec(&resp).unwrap();
                let _ = stream.write_all(&resp_json).await;
                stream.close_write();
            }

            _ => {
                tracing::debug!(stream_type, "unexpected stream type in mock server");
            }
        }
    }
}
