# Usage Examples — `ngrok-rust`

---

## Example 1: Basic HTTPS Listener with Axum

Demonstrates the minimal setup to expose a local axum router at a public HTTPS URL.
Uses the `axum` feature flag for seamless integration.

```toml
# Cargo.toml
[dependencies]
ngrok = { version = "0.1", features = ["axum"] }
axum = "0.7"
tokio = { version = "1", features = ["full"] }
```

```rust
use axum::{routing::get, Router};

#[tokio::main]
async fn main() -> Result<(), ngrok::Error> {
    let listener = ngrok::listen(
        ngrok::EndpointOptions::builder()
            .build(),
    )
    .await?;

    println!("Public URL: {}", listener.url());

    let app = Router::new()
        .route("/", get(|| async { "Hello from ngrok!" }))
        .route("/health", get(|| async { "OK" }));

    axum::serve(listener, app).await.unwrap();
    Ok(())
}
```

---

## Example 2: Explicit Agent Configuration

Demonstrates configuring an `Agent` with a custom authtoken, heartbeat settings,
and a named endpoint with a pinned URL.

```rust
use ngrok::{Agent, EndpointOptions};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), ngrok::Error> {
    let agent = Agent::builder()
        .authtoken(std::env::var("NGROK_AUTHTOKEN").expect("NGROK_AUTHTOKEN not set"))
        .description("production API gateway")
        .metadata("version=1.2.3,env=prod")
        .heartbeat_interval(Duration::from_secs(20))
        .heartbeat_tolerance(Duration::from_secs(60))
        .build()
        .await?;

    let listener = agent
        .listen(
            EndpointOptions::builder()
                .url("https://api.example.ngrok.app")
                .name("production-api")
                .description("Primary API endpoint")
                .traffic_policy(r#"
                    on_http_request:
                      - name: rate-limit
                        actions:
                          - type: rate-limit
                            config:
                              name: per-ip
                              algorithm: sliding_window
                              capacity: 100
                              rate: 60s
                              bucket_key: [conn.client_ip]
                "#)
                .build(),
        )
        .await?;

    println!("Listening at: {}", listener.url());

    // Use as a raw accept loop
    while let Ok(stream) = listener.accept().await {
        tokio::spawn(async move {
            handle_connection(stream).await;
        });
    }

    Ok(())
}

async fn handle_connection(stream: ngrok::NgrokStream) {
    // stream implements tokio::io::AsyncRead + AsyncWrite
    let _ = stream;
}
```

---

## Example 3: Error Handling and Diagnostics

Demonstrates structured error handling using `ngrok::Error` pattern matching, and
running a pre-connection diagnostic probe to identify connectivity issues.

```rust
use ngrok::{Agent, DiagnoseError, Error};

#[tokio::main]
async fn main() {
    // Run diagnostics first
    let agent = Agent::builder()
        .authtoken_from_env()
        .build()
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to build agent: {e}");
            std::process::exit(1);
        });

    match agent.diagnose(None).await {
        Ok(result) => {
            println!(
                "Connection OK: addr={}, region={}, latency={:?}",
                result.addr, result.region, result.latency
            );
        }
        Err(DiagnoseError::Tcp { addr, source }) => {
            eprintln!("TCP unreachable — check firewall rules. addr={addr}: {source}");
            std::process::exit(1);
        }
        Err(DiagnoseError::Tls { addr, source }) => {
            eprintln!("TLS failure — possible MITM or CA mismatch. addr={addr}: {source}");
            std::process::exit(1);
        }
        Err(DiagnoseError::Muxado(msg)) => {
            eprintln!("Muxado protocol error: {msg}");
            std::process::exit(1);
        }
    }

    // Now connect normally
    match agent
        .listen(ngrok::EndpointOptions::builder().build())
        .await
    {
        Ok(listener) => println!("Endpoint: {}", listener.url()),
        Err(Error::Cloud { code, message }) => {
            eprintln!("ngrok error [{code}]: {message}");
        }
        Err(e) => eprintln!("Unexpected error: {e}"),
    }
}
```

---

## Example 4: Integration with `hyper` (feature = "hyper")

Demonstrates using `EndpointListener` directly as a `hyper` connection acceptor,
suitable when you need fine-grained control over HTTP/2 settings.

```toml
[dependencies]
ngrok = { version = "0.1", features = ["hyper"] }
hyper = { version = "1", features = ["full"] }
hyper-util = { version = "0.1", features = ["full"] }
http-body-util = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use http_body_util::Full;
use hyper::{body::Bytes, service::service_fn, Request, Response};
use hyper_util::rt::TokioIo;
use ngrok::{Agent, EndpointOptions};

async fn hello_handler(
    _req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    Ok(Response::new(Full::new(Bytes::from("Hello from hyper!"))))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = Agent::builder().authtoken_from_env().build().await?;

    let mut listener = agent
        .listen(EndpointOptions::builder().build())
        .await?;

    println!("URL: {}", listener.url());

    // Manual accept loop with hyper
    loop {
        let stream = listener.accept().await?;
        let io = TokioIo::new(stream);

        tokio::spawn(async move {
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service_fn(hello_handler))
                .await
            {
                eprintln!("Connection error: {e}");
            }
        });
    }
}
```

---

## Example 5: Auto-Forward with Events, RPC Handling, and Multiple Endpoints

Demonstrates a production-grade pattern: forwarder mode with event monitoring,
graceful shutdown via RPC, and two simultaneous endpoints (HTTPS + TCP).

```rust
use ngrok::{Agent, EndpointOptions, Event, RpcMethod, RpcResponse, Upstream};
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<(), ngrok::Error> {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);
    let st = shutdown_tx.clone();

    let agent = Agent::builder()
        .authtoken_from_env()
        .description("multi-endpoint server")
        .on_event(|evt| match &evt {
            Event::AgentConnectSucceeded(e) => {
                println!("[{}] Connected: session={}", e.occurred_at.elapsed().unwrap().as_secs(), e.session.id());
            }
            Event::AgentDisconnected(e) => {
                let reason = e.error.as_deref().unwrap_or("clean");
                println!("Disconnected: {reason}");
            }
            Event::ConnectionOpened(e) => {
                println!("→ conn from {} on {}", e.remote_addr, e.endpoint_id);
            }
            Event::ConnectionClosed(e) => {
                println!(
                    "← conn closed: {}B in, {}B out, {:?}",
                    e.bytes_in, e.bytes_out, e.duration
                );
            }
            Event::HttpRequestComplete(e) => {
                println!("{} {} → {} ({:?})", e.method, e.path, e.status_code, e.duration);
            }
            _ => {}
        })
        .on_rpc(move |req| {
            match req.method {
                RpcMethod::StopAgent => {
                    println!("Received StopAgent RPC — shutting down");
                    let _ = st.send(true);
                    RpcResponse::default()
                }
                RpcMethod::RestartAgent => {
                    println!("Received RestartAgent RPC");
                    RpcResponse::default()
                }
                other => {
                    RpcResponse {
                        error: Some(format!("unhandled RPC: {other:?}")),
                    }
                }
            }
        })
        .build()
        .await?;

    // HTTPS forwarder to local HTTP server
    let https_fwd = agent
        .forward(
            Upstream::new("http://localhost:8080"),
            EndpointOptions::builder()
                .name("http-api")
                .build(),
        )
        .await?;

    // TCP forwarder to local PostgreSQL
    let tcp_fwd = agent
        .forward(
            Upstream::new("tcp://localhost:5432"),
            EndpointOptions::builder()
                .url("tcp://")
                .name("postgres-tunnel")
                .build(),
        )
        .await?;

    println!("HTTPS endpoint: {}", https_fwd.url());
    println!("TCP endpoint:   {}", tcp_fwd.url());
    println!("Agent endpoints: {}", agent.endpoints().len());

    // Wait for shutdown signal
    shutdown_rx.changed().await.ok();
    println!("Shutting down...");

    agent.disconnect().await?;
    Ok(())
}
```

---

## Example 6: Offline Testing with `ngrok-testing`

Demonstrates writing a fully offline integration test using `MockNgrokServer`.
No authtoken or network access required.

```toml
[dev-dependencies]
ngrok-testing = "0.1"
tokio = { version = "1", features = ["full"] }
```

```rust
use ngrok_testing::{AgentTestExt, MockNgrokServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn offline_listen_and_receive_connection() {
    let (server, connect_url) = MockNgrokServer::start().await;

    let agent = ngrok::Agent::builder()
        .authtoken("offline-test-token")
        .with_mock_server(&connect_url)
        .build()
        .await
        .unwrap();

    let listener = agent
        .listen(ngrok::EndpointOptions::builder().build())
        .await
        .unwrap();

    assert!(listener.url().scheme() == "https");

    // Inject a test connection through the mock server
    let endpoint = server.accept_endpoint().await;
    let (client_io, server_io) = tokio::io::duplex(1024);
    endpoint.inject_connection(client_io).await;

    // Application side accepts
    let mut stream = listener.accept().await.unwrap();

    // Inject bytes on the "client" side
    let (mut client_read, mut client_write) = tokio::io::split(server_io);
    client_write.write_all(b"hello ngrok").await.unwrap();

    let mut buf = vec![0u8; 11];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello ngrok");
}
```

---

## Example 7: FFI Data-Plane Pattern (Split-Plane Architecture)

This example shows how an FFI adapter crate (such as `ngrok-rustler` for Elixir or
`ngrok-uniffi` for Kotlin/Swift) should bridge the ngrok data plane into its host
runtime without ever blocking a host-language thread. The pattern applies to any
event-driven environment (BEAM schedulers, libuv, Netty, etc.).

The key rule: use `ngrok-sync` + `block_on` only for the **control plane** (connect,
listen, bind). Use `agent.runtime()` + `rt.spawn()` + `NgrokStream::next_bytes()` for
the **data plane** (stream I/O).

```rust
// This pseudocode represents what the ngrok-rustler or ngrok-uniffi adapter crate
// would look like internally. It is NOT application code for end users.
//
// Cargo.toml for the adapter crate:
//   [dependencies]
//   ngrok-sync = "0.1"   # control plane
//   # ngrok is transitively available via ngrok-sync for NgrokStream + next_bytes()

use std::sync::Arc;
use ngrok_sync::{AgentBuilder, EndpointOptions, NgrokStream};

/// Opaque handle returned to the host language for a running listener.
pub struct ListenerHandle {
    rt: Arc<tokio::runtime::Runtime>,
    url: String,
}

/// Opaque handle returned to the host language for a single connection.
pub struct ConnectionHandle {
    write_tx: tokio::sync::mpsc::UnboundedSender<bytes::Bytes>,
}

/// Type alias for the host-language callback signature (pseudocode).
/// In real Rustler this would be a `LocalPid`; in napi-rs a `JsFunction`.
type HostDataCallback = Arc<dyn Fn(bytes::Bytes) + Send + Sync + 'static>;
type HostCloseCallback = Arc<dyn Fn() + Send + Sync + 'static>;

/// FFI-exported function (annotated with #[rustler::nif] or #[uniffi::export] in the
/// real adapter crate).
pub fn start_listener(
    authtoken: &str,
    on_connection: impl Fn(ConnectionHandle) + Send + Sync + 'static,
) -> Result<ListenerHandle, ngrok_sync::Error> {
    // ── CONTROL PLANE ─────────────────────────────────────────────────────────
    // All of this is block_on — acceptable because it happens once, not per-packet.

    let agent = AgentBuilder::new()
        .authtoken(authtoken)
        .build()?;                              // block_on: connect + auth

    let listener = agent.listen(EndpointOptions::default())?;  // block_on: bind
    let url = listener.url().to_string();
    let rt = agent.runtime();                   // Arc<tokio::Runtime>

    // ── ACCEPT LOOP (spawned — never blocks host thread) ──────────────────────
    let rt_clone = Arc::clone(&rt);
    rt.spawn(async move {
        loop {
            // accept() is a one-time block_on per connection — acceptable.
            // In async context we call the inner async method directly.
            let stream: ngrok::NgrokStream = match listener.inner.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let conn_handle = spawn_data_plane(rt_clone.clone(), stream);
            on_connection(conn_handle);
        }
    });

    Ok(ListenerHandle { rt, url })
}

/// Spawn a Tokio task to drive I/O for a single connection.
/// This is the DATA PLANE — fully async, zero-copy, never blocks host threads.
fn spawn_data_plane(
    rt: Arc<tokio::runtime::Runtime>,
    mut stream: ngrok::NgrokStream,
) -> ConnectionHandle {
    // Channel from host language → Rust write path
    let (write_tx, mut write_rx) = tokio::sync::mpsc::unbounded_channel::<bytes::Bytes>();

    rt.spawn(async move {
        loop {
            tokio::select! {
                // ── Read path: ngrok → host language ──────────────────────────
                maybe_bytes = stream.next_bytes() => {
                    match maybe_bytes {
                        Some(bytes) => {
                            // bytes is a bytes::Bytes arc — zero-copy arc bump.
                            // In Rustler:  rustler::env::send(&pid, OwnedBinary::from(&bytes))
                            // In napi-rs:  callback.call(Buffer::from_data(bytes))
                            // In UniFFI:   deliver_to_generated_callback(bytes.as_ref())
                            deliver_to_host(bytes);
                        }
                        None => break, // remote closed the stream
                    }
                }

                // ── Write path: host language → ngrok ─────────────────────────
                maybe_payload = write_rx.recv() => {
                    match maybe_payload {
                        Some(payload) => {
                            use tokio::io::AsyncWriteExt;
                            if stream.write_all(&payload).await.is_err() {
                                break;
                            }
                        }
                        None => break, // ConnectionHandle dropped
                    }
                }
            }
        }
        // Stream is closed; notify host if needed.
    });

    ConnectionHandle { write_tx }
}

impl ConnectionHandle {
    /// Send bytes from the host language to the ngrok tunnel (non-blocking).
    /// The write is dispatched on the Tokio runtime; this call returns immediately.
    pub fn send(&self, data: bytes::Bytes) {
        let _ = self.write_tx.send(data);
    }
}

// ── What this achieves ────────────────────────────────────────────────────────
//
// Host language thread:  calls start_listener() once → returns ListenerHandle
//                        receives ConnectionHandle via on_connection callback
//                        calls conn.send(bytes) to write (fire-and-forget, no block)
//
// Tokio runtime thread:  drives accept() loop + all stream I/O
//                        calls next_bytes() → delivers to host via callback
//
// Host I/O schedulers:   NEVER blocked on network I/O — zero risk of starvation
// Memory copies:         ZERO between muxado frame and host callback
```

---

## Example 8: Axum `ListenerExt` — Connection Limiting and IO Inspection

`EndpointListener` implements `axum::serve::Listener`, which means it automatically
gains all `axum::serve::ListenerExt` extension methods via a blanket impl. These
require no additional code beyond the `axum` feature.

```toml
# Cargo.toml
[dependencies]
ngrok = { version = "0.1", features = ["axum"] }
axum = "0.7"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
```

```rust
use axum::{routing::get, Router};
use axum::serve::ListenerExt;

#[tokio::main]
async fn main() -> Result<(), ngrok::Error> {
    let listener = ngrok::listen(
        ngrok::EndpointOptions::builder().build(),
    )
    .await?;

    println!("Public URL: {}", listener.url());

    let app = Router::new()
        .route("/", get(|| async { "Hello!" }));

    // limit_connections: cap the number of concurrent connections served.
    // When the limit is reached, new connections are accepted from ngrok but
    // held in the OS accept buffer until a slot opens — no connections are
    // refused.
    //
    // tap_io: called for every accepted stream before it is handed to hyper.
    // Useful for logging, metrics, or per-connection TLS inspection.
    axum::serve(
        listener
            .limit_connections(512)
            .tap_io(|stream| {
                tracing::debug!(remote = %stream.remote_addr(), "new connection");
            }),
        app,
    )
    .await
    .unwrap();

    Ok(())
}
```
