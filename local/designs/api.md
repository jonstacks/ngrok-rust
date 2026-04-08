# Public API Reference — `ngrok-rust`

## Overview

`ngrok` exposes an async Rust API built on `tokio`. Configuration uses the builder
pattern throughout. All public types that callers interact with are trait objects or
concrete types returned through opaque handles — internals are not exposed. Error
handling uses `thiserror`-derived `Error` enums and `Result<T, ngrok::Error>`.

The API surface mirrors the semantics of `ngrok-go v2` but is idiomatic Rust:
builder structs replace functional options, `async fn` replaces Go channels,
and Rust traits replace Go interfaces.

> **Note:** The muxado stream-multiplexing layer lives in a separate workspace crate
> (`muxado`). Its public API is documented in [`muxado-api.md`](muxado-api.md).
> The `ngrok` crate does not re-export muxado types — it is an internal dependency.

---

## Types

### `Agent`

The root handle. Manages authentication, the persistent muxado session, and all
open endpoints. At most one `AgentSession` is active at a time.

```rust
pub struct Agent { /* opaque */ }
```

#### Constructor — `AgentBuilder`

```rust
impl Agent {
    pub fn builder() -> AgentBuilder;
}

pub struct AgentBuilder { /* opaque */ }

impl AgentBuilder {
    // Authentication
    pub fn authtoken(self, token: impl Into<String>) -> Self;
    pub fn authtoken_from_env(self) -> Self;             // reads NGROK_AUTHTOKEN

    // Connection
    pub fn connect_url(self, addr: impl Into<String>) -> Self;
    pub fn connect_ca_cert(self, pem: &[u8]) -> Self;    // DER or PEM bytes
    pub fn tls_config(self, cfg: rustls::ClientConfig) -> Self;
    pub fn proxy_url(self, url: impl Into<String>) -> Self;
    pub fn auto_connect(self, auto: bool) -> Self;       // default: true

    // Heartbeats
    pub fn heartbeat_interval(self, d: Duration) -> Self;
    pub fn heartbeat_tolerance(self, d: Duration) -> Self;

    // Identity & Metadata
    pub fn client_info(self, client_type: impl Into<String>, version: impl Into<String>) -> Self;
    pub fn description(self, desc: impl Into<String>) -> Self;
    pub fn metadata(self, meta: impl Into<String>) -> Self;

    // Callbacks
    pub fn on_event(self, handler: impl Fn(Event) + Send + Sync + 'static) -> Self;
    pub fn on_rpc(self, handler: impl Fn(RpcRequest) -> RpcResponse + Send + Sync + 'static) -> Self;

    // Logging
    pub fn with_tracing(self) -> Self;                   // wire tracing events to `tracing` crate

    // Build
    pub async fn build(self) -> Result<Agent, Error>;
}
```

#### Methods on `Agent`

```rust
impl Agent {
    /// Explicitly connect to ngrok cloud. No-op if already connected
    /// and `auto_connect` is true.
    pub async fn connect(&self) -> Result<(), Error>;

    /// Disconnect cleanly. All open endpoints signal completion.
    pub async fn disconnect(&self) -> Result<(), Error>;

    /// Return the active session, if any.
    pub fn session(&self) -> Option<AgentSession>;

    /// Snapshot of all currently open endpoints.
    pub fn endpoints(&self) -> Vec<EndpointInfo>;

    /// Open a new listener endpoint.
    pub async fn listen(
        &self,
        opts: EndpointOptions,
    ) -> Result<EndpointListener, Error>;

    /// Open a new forwarder endpoint that auto-proxies to an upstream.
    pub async fn forward(
        &self,
        upstream: Upstream,
        opts: EndpointOptions,
    ) -> Result<EndpointForwarder, Error>;

    /// Run a connectivity probe without authenticating.
    /// Returns `Err(DiagnoseError::Tcp)`, `Err(DiagnoseError::Tls)`,
    /// or `Err(DiagnoseError::Muxado)` to identify the failing layer.
    pub async fn diagnose(
        &self,
        addr: Option<&str>,
    ) -> Result<DiagnoseResult, DiagnoseError>;
}
```

---

### `AgentSession`

An immutable snapshot of a connected session. Cheaply cloneable.

```rust
#[derive(Clone)]
pub struct AgentSession { /* opaque */ }

impl AgentSession {
    /// Server-assigned session ID.
    pub fn id(&self) -> &str;

    /// Non-fatal warnings returned by ngrok cloud during auth.
    pub fn warnings(&self) -> &[String];

    /// Timestamp of successful `connect()`.
    pub fn started_at(&self) -> SystemTime;
}
```

---

### `EndpointListener`

An async stream of inbound connections. Implements `futures::Stream` and the
`hyper`/`axum` integration traits when the respective feature flags are enabled.

```rust
pub struct EndpointListener { /* opaque */ }

impl EndpointListener {
    /// The public URL assigned by ngrok cloud.
    pub fn url(&self) -> &Url;
    pub fn id(&self) -> &str;
    pub fn name(&self) -> &str;
    pub fn metadata(&self) -> &str;
    pub fn protocol(&self) -> &str;

    /// Accept the next inbound connection. Cancellable by dropping the returned future.
    pub async fn accept(&self) -> Result<NgrokStream, Error>;

    /// A future that resolves when this endpoint has been closed.
    pub fn done(&self) -> impl Future<Output = ()> + '_;

    /// Close the endpoint gracefully.
    pub async fn close(self) -> Result<(), Error>;
}

/// Implements Stream<Item = Result<NgrokStream, Error>>
impl futures::Stream for EndpointListener { ... }

/// Feature = "axum"
///
/// `EndpointListener` implements `axum::serve::Listener` so it can be passed
/// directly to `axum::serve()`.
///
/// `Listener::accept()` returns `(Self::Io, Self::Addr)` with no `Result`.
/// Transient accept errors (e.g. a single connection reset mid-handshake) are
/// logged at `WARN` level and retried; fatal errors (endpoint closed) break the
/// accept loop, causing `axum::serve` to return.
///
/// Implementing `axum::serve::Listener` also grants `ListenerExt` methods for
/// free via the blanket impl:
///   - `.limit_connections(n)` — cap concurrent connections
///   - `.tap_io(f)` — inspect / log raw IO on each accepted stream
///   - `.with_service_fn(f)` — wrap with a per-connection service factory
#[cfg(feature = "axum")]
impl axum::serve::Listener for EndpointListener {
    type Io = NgrokStream;
    type Addr = NgrokAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.accept().await {
                Ok(stream) => {
                    let addr = NgrokAddr(stream.remote_addr().to_owned());
                    return (stream, addr);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "transient accept error; retrying");
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(NgrokAddr(self.url().to_string()))
    }
}
```

---

### `NgrokStream`

A single inbound connection, implementing both `AsyncRead` and `AsyncWrite`.
When agent-side TLS termination is configured, this is a `TlsStream<_>`.

```rust
pub struct NgrokStream { /* opaque */ }

impl tokio::io::AsyncRead for NgrokStream { ... }
impl tokio::io::AsyncWrite for NgrokStream { ... }

impl NgrokStream {
    /// Remote address as reported by the ngrok proxy header.
    pub fn remote_addr(&self) -> &str;

    /// Yield the next frame payload from the underlying muxado stream.
    ///
    /// Returns the raw `bytes::Bytes` arc from the muxado `DataFrame` — no copy into a
    /// caller buffer. The returned `Bytes` shares the same allocation as the TLS receive
    /// window; dropping it releases the reference count.
    ///
    /// Returns `None` when the remote peer closes the stream (RST or GOAWAY received).
    ///
    /// # Example
    /// ```rust
    /// while let Some(chunk) = stream.next_bytes().await {
    ///     process(chunk); // chunk is bytes::Bytes — zero-copy arc
    /// }
    /// ```
    pub async fn next_bytes(&mut self) -> Option<bytes::Bytes>;
}

// NgrokStream does NOT directly implement hyper::rt::Read / hyper::rt::Write.
// When using the `hyper` feature, wrap the stream with TokioIo from hyper-util:
//
//   use hyper_util::rt::TokioIo;
//   let io = TokioIo::new(stream);
//   hyper::server::conn::http1::Builder::new()
//       .serve_connection(io, service)
//       .await?;
//
// The `axum` feature does not require this — axum wraps the Listener::Io in
// TokioIo internally before passing it to hyper.
```

---

### `EndpointForwarder`

An auto-proxy handle. The SDK internally accepts connections and forwards them
to the configured upstream; no application code is required for I/O.

```rust
pub struct EndpointForwarder { /* opaque */ }

impl EndpointForwarder {
    pub fn url(&self) -> &Url;
    pub fn id(&self) -> &str;
    pub fn upstream_url(&self) -> &Url;
    pub fn upstream_protocol(&self) -> Option<&str>;
    pub fn proxy_protocol(&self) -> ProxyProtoVersion;

    pub fn done(&self) -> impl Future<Output = ()> + '_;
    pub async fn close(self) -> Result<(), Error>;
}
```

---

### `EndpointOptions`

Built via `EndpointOptionsBuilder`.

```rust
pub struct EndpointOptions { /* opaque */ }

impl EndpointOptions {
    pub fn builder() -> EndpointOptionsBuilder;
}

/// `Default` produces the same result as `EndpointOptions::builder().build()`:
/// an ephemeral endpoint with no pinned URL, no traffic policy, and all defaults.
impl Default for EndpointOptions {
    fn default() -> Self {
        Self::builder().build()
    }
}

pub struct EndpointOptionsBuilder { /* opaque */ }

impl EndpointOptionsBuilder {
    /// Full URL — scheme determines protocol: https://, http://, tcp://, tls://
    pub fn url(self, url: impl Into<String>) -> Self;
    pub fn name(self, name: impl Into<String>) -> Self;
    pub fn description(self, desc: impl Into<String>) -> Self;
    pub fn metadata(self, meta: impl Into<String>) -> Self;

    /// YAML or JSON traffic policy string evaluated at the ngrok edge.
    pub fn traffic_policy(self, policy: impl Into<String>) -> Self;

    /// Agent-side TLS termination. When set, `NgrokStream` connections will be
    /// TLS-terminated by the agent using the provided server config before being
    /// handed to application code. Requires the `rustls` crate, which is a hard
    /// dependency of `ngrok` — no feature flag is needed.
    pub fn agent_tls_termination(self, cfg: rustls::ServerConfig) -> Self;

    pub fn pooling_enabled(self, pool: bool) -> Self;
    pub fn bindings(self, bindings: Vec<String>) -> Self;

    pub fn build(self) -> EndpointOptions;
}
```

---

### `Upstream`

Value object carrying forwarding configuration.

```rust
pub struct Upstream { /* opaque */ }

impl Upstream {
    /// `addr` accepts: `"8080"`, `"host:port"`, `"http://host:port"`,
    /// `"https://host:port"`.
    pub fn new(addr: impl Into<String>) -> Self;

    pub fn protocol(self, proto: impl Into<String>) -> Self;
    pub fn tls_config(self, cfg: rustls::ClientConfig) -> Self;
    pub fn proxy_proto(self, version: ProxyProtoVersion) -> Self;
}
```

---

### `EndpointInfo`

A non-owning snapshot of endpoint metadata. Returned by `Agent::endpoints()`.

```rust
#[derive(Clone, Debug)]
pub struct EndpointInfo {
    pub id: String,
    pub name: String,
    pub url: Url,
    pub protocol: String,
    pub kind: EndpointKind,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum EndpointKind {
    Listener,
    Forwarder { upstream_url: Url },
}
```

---

### `Event`

All events carry a `occurred_at: SystemTime` and are dispatched synchronously to
registered handlers. Handlers **must not block** — spawn a task if work is needed.

```rust
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    AgentConnectSucceeded(AgentConnectSucceededEvent),
    AgentDisconnected(AgentDisconnectedEvent),
    AgentHeartbeatReceived(AgentHeartbeatReceivedEvent),
    ConnectionOpened(ConnectionOpenedEvent),
    ConnectionClosed(ConnectionClosedEvent),
    HttpRequestComplete(HttpRequestCompleteEvent),
}

#[derive(Debug, Clone)]
pub struct AgentConnectSucceededEvent {
    pub occurred_at: SystemTime,
    pub session: AgentSession,
}

#[derive(Debug, Clone)]
pub struct AgentDisconnectedEvent {
    pub occurred_at: SystemTime,
    pub session: AgentSession,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentHeartbeatReceivedEvent {
    pub occurred_at: SystemTime,
    pub session: AgentSession,
    pub latency: Duration,
}

#[derive(Debug, Clone)]
pub struct ConnectionOpenedEvent {
    pub occurred_at: SystemTime,
    pub endpoint_id: String,
    pub remote_addr: String,
}

#[derive(Debug, Clone)]
pub struct ConnectionClosedEvent {
    pub occurred_at: SystemTime,
    pub endpoint_id: String,
    pub remote_addr: String,
    pub duration: Duration,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

#[derive(Debug, Clone)]
pub struct HttpRequestCompleteEvent {
    pub occurred_at: SystemTime,
    pub endpoint_id: String,
    pub method: String,
    pub path: String,
    pub status_code: u16,
    pub duration: Duration,
}
```

---

### `RpcRequest` / `RpcResponse`

```rust
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RpcRequest {
    pub method: RpcMethod,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RpcMethod {
    StopAgent,
    RestartAgent,
    UpdateAgent,
}

#[derive(Debug, Default)]
pub struct RpcResponse {
    pub error: Option<String>,
}
```

---

### `ProxyProtoVersion`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ProxyProtoVersion {
    #[default]
    None,
    V1,
    V2,
}
```

---

### `DiagnoseResult`

```rust
#[derive(Debug, Clone)]
pub struct DiagnoseResult {
    pub addr: String,
    pub region: String,
    pub latency: Duration,
}
```

---

### `NgrokAddr`

The address type used as `Listener::Addr` for the axum integration. Wraps the
public ngrok URL string (for `local_addr`) or the client IP reported in the
ngrok proxy header (for per-connection addresses). Implements `Display` only —
it is **not** a `SocketAddr` and does not implement `ToSocketAddrs`, since ngrok
addresses are not directly dialable by the application.

```rust
#[derive(Debug, Clone)]
pub struct NgrokAddr(String);

impl std::fmt::Display for NgrokAddr { ... }
```

---

## Error Types

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("ngrok cloud error [{code}]: {message}")]
    Cloud { code: String, message: String },

    #[error("agent not connected")]
    NotConnected,

    #[error("agent already connected")]
    AlreadyConnected,

    #[error("unsupported URL scheme: {0}")]
    UnsupportedScheme(String),

    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("muxado protocol error: {0}")]
    Muxado(String),  // wraps muxado::MuxadoError via Display

    #[error("authtoken missing — set NGROK_AUTHTOKEN or call .authtoken()")]
    MissingAuthtoken,

    #[error("invalid upstream address: {0}")]
    InvalidUpstream(String),
}

impl Error {
    /// Returns the ngrok error code if this is a `Cloud` variant.
    pub fn code(&self) -> Option<&str> { ... }
}

impl From<muxado::MuxadoError> for Error {
    fn from(e: muxado::MuxadoError) -> Self {
        Error::Muxado(e.to_string())
    }
}

/// Errors returned by Agent::diagnose.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiagnoseError {
    #[error("TCP connection to {addr} failed: {source}")]
    Tcp { addr: String, #[source] source: std::io::Error },

    #[error("TLS handshake with {addr} failed: {source}")]
    Tls { addr: String, #[source] source: rustls::Error },

    #[error("muxado SrvInfo exchange failed: {0}")]
    Muxado(String),
}
```

---

## Constants

```rust
pub mod rpc {
    pub const STOP_AGENT: &str = "StopAgent";
    pub const RESTART_AGENT: &str = "RestartAgent";
    pub const UPDATE_AGENT: &str = "UpdateAgent";
}

pub const DEFAULT_CONNECT_URL: &str = "connect.ngrok-agent.com:443";
pub const MIN_TLS_VERSION: rustls::ProtocolVersion = rustls::ProtocolVersion::TLSv1_2;
```

---

## Package-Level Convenience Functions

```rust
/// Listen using NGROK_AUTHTOKEN from the environment.
/// Equivalent to `Agent::builder().authtoken_from_env().build().await?.listen(opts).await`.
pub async fn listen(opts: EndpointOptions) -> Result<EndpointListener, Error>;

/// Forward using NGROK_AUTHTOKEN from the environment.
pub async fn forward(upstream: Upstream, opts: EndpointOptions) -> Result<EndpointForwarder, Error>;
```

---

# `ngrok-sync` Crate API

`ngrok-sync` is the **control-plane** blocking wrapper over the async `ngrok` crate.
It owns a `tokio::Runtime` and exposes `block_on`-wrapped equivalents of the connection
management APIs: building an agent, connecting, opening listeners, and managing endpoint
lifecycle. It does **not** wrap stream I/O in blocking wrappers — the data plane stays
async (see "Data Plane Pattern" below).

All non-async types (`EndpointOptions`, `Upstream`, `Event`, `Error`, etc.) are
**re-exported unchanged** from `ngrok` — no conversion needed.

> **Control plane vs data plane**: `ngrok-sync` covers everything up to and including
> `EndpointListener::accept()`. After that, the returned `ngrok::NgrokStream` is handled
> by the FFI adapter's data-plane strategy (`rt.spawn()` + `next_bytes()`).

## `ngrok_sync::AgentBuilder`

```rust
pub struct AgentBuilder { /* private */ }

impl AgentBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self;

    /// Inject a pre-built `Arc<Runtime>` (advanced — for FFI adapters that share a runtime).
    pub fn runtime(self, rt: Arc<tokio::runtime::Runtime>) -> Self;

    // All ngrok::AgentBuilder setters are mirrored here with identical signatures:

    // Authentication
    pub fn authtoken(self, token: impl Into<String>) -> Self;
    pub fn authtoken_from_env(self) -> Self;             // reads NGROK_AUTHTOKEN

    // Connection
    pub fn connect_url(self, url: impl Into<String>) -> Self;
    pub fn connect_ca_cert(self, pem: &[u8]) -> Self;    // DER or PEM bytes
    pub fn tls_config(self, cfg: rustls::ClientConfig) -> Self;
    pub fn proxy_url(self, url: impl Into<String>) -> Self;
    pub fn auto_connect(self, auto: bool) -> Self;       // default: true

    // Heartbeats
    pub fn heartbeat_interval(self, d: Duration) -> Self;
    pub fn heartbeat_tolerance(self, d: Duration) -> Self;

    // Identity & Metadata
    pub fn client_info(self, client_type: impl Into<String>, version: impl Into<String>) -> Self;
    pub fn description(self, desc: impl Into<String>) -> Self;
    pub fn metadata(self, meta: impl Into<String>) -> Self;

    // Callbacks
    pub fn on_event(self, handler: impl Fn(Event) + Send + Sync + 'static) -> Self;
    pub fn on_rpc(self, handler: impl Fn(RpcRequest) -> RpcResponse + Send + Sync + 'static) -> Self;

    // Logging
    pub fn with_tracing(self) -> Self;                   // wire tracing events to `tracing` crate

    /// Build the blocking `Agent`. Creates and owns a `tokio::Runtime` unless one was
    /// provided via `runtime()`.
    pub fn build(self) -> Result<Agent, Error>;
}
```

## `ngrok_sync::Agent`

```rust
pub struct Agent { /* Arc<ngrok::Agent> + Arc<tokio::Runtime> */ }

impl Agent {
    pub fn builder() -> AgentBuilder;

    /// Expose the shared runtime so FFI adapters can spawn data-plane tasks on it.
    ///
    /// Typical use:
    /// ```rust
    /// let rt = agent.runtime();
    /// rt.spawn(async move {
    ///     while let Some(bytes) = stream.next_bytes().await {
    ///         host_runtime_send(bytes);
    ///     }
    /// });
    /// ```
    pub fn runtime(&self) -> Arc<tokio::runtime::Runtime>;

    /// Connect the agent to ngrok (blocking).
    pub fn connect(&self) -> Result<(), Error>;

    /// Gracefully disconnect (blocking).
    pub fn disconnect(&self) -> Result<(), Error>;

    /// Return the current session info, if connected.
    pub fn session(&self) -> Option<AgentSession>;    // re-export of ngrok::AgentSession

    /// List currently active endpoints.
    pub fn endpoints(&self) -> Vec<EndpointInfo>;     // re-export of ngrok::EndpointInfo

    /// Open a new listener endpoint (blocking).
    pub fn listen(&self, opts: EndpointOptions) -> Result<EndpointListener, Error>;

    /// Start forwarding traffic to an upstream service (blocking).
    pub fn forward(&self, upstream: Upstream, opts: EndpointOptions) -> Result<EndpointForwarder, Error>;
}
```

## `ngrok_sync::EndpointListener`

```rust
pub struct EndpointListener { /* wraps ngrok::EndpointListener + Arc<Runtime> */ }

impl EndpointListener {
    /// Block until the next inbound connection arrives.
    ///
    /// Returns `ngrok::NgrokStream` — the async stream type. This is a one-time
    /// `block_on` (acceptable, analogous to `TcpListener::accept()`). The caller is
    /// responsible for driving I/O on the returned stream; use `agent.runtime()` to
    /// spawn a data-plane task.
    pub fn accept(&self) -> Result<ngrok::NgrokStream, Error>;

    /// The public URL for this endpoint.
    pub fn url(&self) -> &Url;

    /// Close the listener and release the endpoint (blocking).
    pub fn close(self) -> Result<(), Error>;
}
```

## `ngrok_sync::EndpointForwarder`

```rust
pub struct EndpointForwarder { /* wraps ngrok::EndpointForwarder + Arc<Runtime> */ }

impl EndpointForwarder {
    /// The public URL for this forwarded endpoint.
    pub fn url(&self) -> &Url;

    /// Stop forwarding and release the endpoint (blocking).
    pub fn close(self) -> Result<(), Error>;
}
```

> **Note:** There is no `ngrok_sync::NgrokStream` type. Stream I/O is handled by
> `ngrok::NgrokStream` directly. This is by design — wrapping every `read()` in
> `block_on` would starve event-driven host runtimes under load (see Decision 9).

## Data Plane Pattern for FFI Adapters

After calling `listener.accept()` to get a `ngrok::NgrokStream`, FFI adapters use the
shared runtime to drive the data plane without ever blocking a host-language thread:

```rust
// Step 1: control plane — blocking, via ngrok-sync
let listener = agent.listen(EndpointOptions::default())?;
let rt = agent.runtime();            // Arc<tokio::Runtime>

// Step 2: accept — one-time block_on; acceptable
let stream = listener.accept()?;     // ngrok::NgrokStream (async)

// Step 3: data plane — spawn on the shared runtime, return immediately to host
rt.spawn(async move {
    while let Some(bytes) = stream.next_bytes().await {
        // bytes is bytes::Bytes — zero-copy arc from the muxado receive buffer
        // Elixir/Rustler:  rustler::env::send(&pid, bytes.as_ref())
        // Node.js/napi-rs: callback.call(None, &[Env::create_buffer_copy(bytes)])
        // UniFFI/C#:       copy into pre-pinned Span<byte> or use unsafe pointer
        host_runtime_deliver(bytes);
    }
    host_runtime_close();
});
// ↑ returns immediately; host scheduler thread is never blocked on network I/O
```

Write path (from host language to ngrok tunnel):

```rust
// Writes are also dispatched via spawn to avoid blocking the host thread.
// A oneshot channel surfaces the result back to the caller if needed.
let (tx, rx) = tokio::sync::oneshot::channel();
rt.spawn(async move {
    let result = stream.write_all(&payload).await;
    let _ = tx.send(result);
});
// Host thread can poll rx or ignore it (fire-and-forget).
```

## Re-exported Types

The following types are re-exported directly from `ngrok` — no conversion or wrapper:

```rust
pub use ngrok::{
    EndpointOptions,
    Upstream,
    Event,
    Error,
    DiagnoseError,
    ProxyProtoVersion,
    AgentSession,
    EndpointInfo,
    RpcRequest,
    RpcResponse,
    NgrokStream,   // re-exported so FFI adapters can name it without depending on ngrok directly
};
```
```
