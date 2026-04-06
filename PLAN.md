# ngrok-rust v2: Complete API Redesign with FFI-First Architecture

## Your Mission

Build a new version of the `ngrok` Rust crate that redesigns the public API to match the philosophy of [ngrok-go v2](https://github.com/ngrok/ngrok-go) while being an excellent embedding target for language SDK wrappers (JavaScript/Node.js, Python, Ruby, Elixir, and others). The current crate (v0.18.0) mirrors ngrok-go v1's design — protocol-specific builders, programmatic middleware, and a large public surface. Your job is to collapse that into a small, clean API (~8 core types, ~30 builder methods) and provide a concrete FFI module that makes writing language wrappers trivial.

**Validate your work** by creating working FFI wrapper examples for JavaScript (NAPI-RS) and Elixir (Rustler) in an `examples/ffi-validation/` directory.

---

## Repository Context

The repo is a Cargo workspace:

```
Cargo.toml              (workspace root)
ngrok/                   (main crate — this is what you're redesigning)
  Cargo.toml
  src/
    lib.rs
    internals/           (rpc.rs, proto.rs, raw_session.rs — low-level wire protocol)
    config/              (v1 protocol-specific builders + middleware — being replaced)
    session.rs           (v1 Session/SessionBuilder — being wrapped by Agent)
    tunnel.rs            (v1 Tunnel types — being wrapped by EndpointListener)
    conn.rs              (Conn type — stays public)
    forwarder.rs         (v1 Forwarder — being wrapped by EndpointForwarder)
    tunnel_ext.rs        (axum/hyper integration)
    proxy_proto.rs
    online_tests.rs
  examples/
muxado/                  (internal muxado crate — DO NOT TOUCH)
```

**Do not modify** `muxado/` or `ngrok/src/internals/`. These are the internal wire protocol and transport — they stay as-is. Your new types wrap them.

---

## Internal Architecture: How the Current Crate Works (Read This First)

Before building anything, you need to understand the internal types you'll be wrapping. **Your new v2 types are thin wrappers around these existing internals.** Do not reimplement session management, tunnel creation, or the wire protocol — wrap it.

### `Session` and `SessionBuilder` (in `ngrok/src/session.rs`)

`Session` is the central connection object. It's `Clone + Send + Sync` (it wraps an `Arc` internally). It auto-reconnects on network failures.

```rust
// Creating a session (the v1 pattern you're wrapping):
pub struct Session { /* private fields, Arc-based, Clone */ }

impl Session {
    pub fn builder() -> SessionBuilder;

    // These create protocol-specific builders — the key dispatch points:
    pub fn http_endpoint(&self) -> HttpTunnelBuilder;
    pub fn tcp_endpoint(&self) -> TcpTunnelBuilder;
    pub fn tls_endpoint(&self) -> TlsTunnelBuilder;
    pub fn labeled_tunnel(&self) -> LabeledTunnelBuilder;

    pub fn id(&self) -> String;
    pub async fn close_tunnel(&self, id: impl AsRef<str>) -> Result<(), RpcError>;
    pub async fn close(&mut self) -> Result<(), RpcError>;
}
```

`SessionBuilder` has these methods (all `&mut self -> &mut Self`):
- `authtoken(impl Into<String>)`, `authtoken_from_env()`
- `server_addr(impl Into<String>) -> Result<&mut Self, InvalidServerAddr>`
- `root_cas(impl Into<String>) -> Result<&mut Self, io::Error>`, `ca_cert(Bytes)`
- `tls_config(ClientConfig)` — takes a `rustls::ClientConfig`
- `connector(impl Connector)` — custom connection logic
- `proxy_url(Url) -> Result<&mut Self, ProxyUnsupportedError>` — NOTE: takes `url::Url`, not a string
- `metadata(impl Into<String>)`
- `heartbeat_interval(Duration) -> Result<&mut Self, InvalidHeartbeatInterval>`
- `heartbeat_tolerance(Duration) -> Result<&mut Self, InvalidHeartbeatTolerance>`
- `client_info(impl Into<String>, impl Into<String>, Option<impl Into<String>>)`
- `handle_stop_command(impl CommandHandler<Stop>)` — async callback
- `handle_restart_command(impl CommandHandler<Restart>)` — async callback
- `handle_update_command(impl CommandHandler<Update>)` — async callback
- `handle_heartbeat(impl HeartbeatHandler)` — called on every heartbeat with latency
- `async connect(&self) -> Result<Session, ConnectError>` — the terminal method

Key traits:
```rust
// CommandHandler is an async callback trait:
pub trait CommandHandler<T>: Send + Sync + 'static {
    // Returns a future — the handler is called when ngrok dashboard sends a command
    // T is one of: Stop, Restart, Update
}

pub trait HeartbeatHandler: Send + Sync + 'static {
    // Called with latency Duration on each heartbeat
}

pub trait Connector: Send + Sync + 'static {
    // Establishes the raw TCP/TLS connection to the ngrok service
}

pub trait IoStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
```

### Tunnel Types (in `ngrok/src/tunnel.rs`)

There are four concrete tunnel types, all implementing common traits:

```rust
pub struct HttpTunnel { /* private */ }
pub struct TcpTunnel { /* private */ }
pub struct TlsTunnel { /* private */ }
pub struct LabeledTunnel { /* private */ }
```

Common traits they implement:
```rust
pub trait TunnelInfo {
    fn id(&self) -> &str;
    fn forwards_to(&self) -> &str;
    fn metadata(&self) -> &str;
}

pub trait EndpointInfo {  // for Http/Tcp/Tls tunnels (not Labeled)
    fn proto(&self) -> &str;    // "https", "tcp", "tls"
    fn url(&self) -> &str;      // the full public URL
}

pub trait EdgeInfo {  // for LabeledTunnel only
    fn labels(&self) -> &HashMap<String, String>;
}

pub trait TunnelCloser {
    async fn close(&mut self) -> Result<(), RpcError>;
}

// The main Tunnel trait — note it IS a Stream
pub trait Tunnel:
    TunnelInfo
    + TunnelCloser
    + Stream<Item = Result<EndpointConn, AcceptError>>
    + Send  // All tunnel types are Send
{}
```

All four tunnel types implement `Stream<Item = Result<EndpointConn, AcceptError>>` — you iterate them with `.next().await` to accept connections. `EndpointConn` is a type alias for `Conn<EndpointConnInfo>`.

### Config / Tunnel Builders (in `ngrok/src/config/`)

Each builder is constructed from a `Session` and produces a tunnel:

```rust
// HttpTunnelBuilder has the most methods (~25+):
pub struct HttpTunnelBuilder { /* contains Session + options */ }

impl HttpTunnelBuilder {
    // Endpoint identity:
    pub fn domain(self, domain: impl Into<String>) -> Self;
    pub fn url(self, url: impl Into<String>) -> Self;
    pub fn scheme(self, scheme: Scheme) -> Self;

    // Middleware (ALL of these are being replaced by traffic_policy):
    pub fn compression(self) -> Self;
    pub fn circuit_breaker(self, threshold: f64) -> Self;
    pub fn websocket_tcp_conversion(self) -> Self;
    pub fn host_header_rewrite(self, rewrite: bool) -> Self;
    pub fn request_header(self, name: impl Into<String>, value: impl Into<String>) -> Self;
    pub fn response_header(self, name: impl Into<String>, value: impl Into<String>) -> Self;
    pub fn remove_request_header(self, name: impl Into<String>) -> Self;
    pub fn remove_response_header(self, name: impl Into<String>) -> Self;
    pub fn basic_auth(self, username: impl Into<String>, password: impl Into<String>) -> Self;
    pub fn oauth(self, opts: OauthOptions) -> Self;
    pub fn oidc(self, opts: OidcOptions) -> Self;
    pub fn mutual_tlsca(self, ca: Bytes) -> Self;
    pub fn webhook_verification(self, provider: impl Into<String>, secret: impl Into<String>) -> Self;
    pub fn allow_user_agent(self, pattern: impl Into<String>) -> Self;
    pub fn deny_user_agent(self, pattern: impl Into<String>) -> Self;
    pub fn allow_cidr(self, cidr: impl Into<String>) -> Self;
    pub fn deny_cidr(self, cidr: impl Into<String>) -> Self;
    pub fn proxy_proto(self, version: ProxyProto) -> Self;

    // Common options (these survive in v2):
    pub fn metadata(self, meta: impl Into<String>) -> Self;
    pub fn traffic_policy(self, policy: impl Into<String>) -> Self;
    pub fn forwards_to(self, addr: impl Into<String>) -> Self;
    pub fn app_protocol(self, proto: impl Into<String>) -> Self;
    pub fn binding(self, binding: impl Into<String>) -> Self;
    pub fn pooling_enabled(self, enabled: bool) -> Self;
    pub fn verify_upstream_tls(self, verify: bool) -> Self;
}

// TunnelBuilder and ForwarderBuilder traits — the terminal methods:
pub trait TunnelBuilder: From<Session> {
    type Tunnel: Tunnel;
    async fn listen(&self) -> Result<Self::Tunnel, RpcError>;
}

pub trait ForwarderBuilder: TunnelBuilder {
    async fn listen_and_forward(&self, url: Url) -> Result<Forwarder<Self::Tunnel>, RpcError>;
}

// TcpTunnelBuilder has fewer methods:
// remote_addr(), allow_cidr(), deny_cidr(), proxy_proto(),
// traffic_policy(), metadata(), binding(), forwards_to(), pooling_enabled()

// TlsTunnelBuilder: like Tcp plus domain(), mutual_tlsca(), tls_termination()

// LabeledTunnelBuilder: label(key, value), app_protocol(), metadata(), forwards_to()
```

### Forwarder (in `ngrok/src/forwarder.rs`)

```rust
pub struct Forwarder<T: Tunnel> {
    // Holds the tunnel + a join handle for the forwarding task
}

// Created by listen_and_forward() — accepts connections and forwards them to a URL
```

### Conn (in `ngrok/src/conn.rs`)

```rust
pub trait Conn: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    // ConnInfo access
}

pub struct EndpointConn; // type alias: Conn with endpoint info
pub struct EdgeConn;      // type alias: Conn with edge info
```

### Error Types (in `ngrok/src/session.rs` and `ngrok/src/internals/proto.rs`)

```rust
pub enum RpcError {
    // Wire-protocol RPC errors — this is the INTERNAL error type
    // that must NOT appear in your v2 public API
}

pub enum ConnectError {
    // Errors from SessionBuilder::connect()
}

// The public Error trait (currently on the crate root):
pub trait Error: std::error::Error {
    fn error_code(&self) -> Option<&str>;  // ngrok error code like "ERR_NGROK_108"
    fn msg(&self) -> String;
}
```

### How to Wire v2 → v1 Internals

Here's the exact mapping for how your new types call into the existing code:

**`AgentBuilder::build()` → stores config, does NOT call `SessionBuilder::connect()`**
```rust
// In AgentBuilder::build():
// 1. Validate config
// 2. Store all SessionBuilder params in AgentConfig
// 3. Return Agent { config: Arc<AgentConfig>, session: Arc<Mutex<Option<Session>>> }
// Do NOT connect yet — auto_connect defers to first listen/forward
```

**`Agent::connect()` → calls `SessionBuilder::connect()`**
```rust
// In Agent::connect():
// 1. Build a SessionBuilder from stored config
// 2. Apply all stored params: authtoken, server_addr, ca_cert, etc.
// 3. Wire event_handler into handle_heartbeat (emit Event::AgentHeartbeatReceived)
// 4. Wire rpc_handler into handle_stop/restart/update (dispatch by method string)
// 5. Call session_builder.connect().await
// 6. Store resulting Session in self.session
```

**`EndpointListenBuilder::start()` → URL-scheme dispatch**
```rust
// This is the CRITICAL translation layer. In start():
async fn start(self) -> Result<EndpointListener, NgrokError> {
    let session = self.agent.ensure_connected().await?;  // auto-connect if needed

    // Parse URL to determine protocol
    let tunnel: Box<dyn Tunnel> = match url_scheme {
        "https" | "http" | None => {
            let mut b = session.http_endpoint();
            if let Some(domain) = parsed_domain { b = b.domain(domain); }
            if let Some(tp) = &self.opts.traffic_policy { b = b.traffic_policy(tp); }
            if let Some(meta) = &self.opts.metadata { b = b.metadata(meta); }
            if let Some(binding) = &self.opts.bindings.first() { b = b.binding(binding); }
            if let Some(pe) = self.opts.pooling_enabled { b = b.pooling_enabled(pe); }
            Box::new(b.listen().await?)
        },
        "tcp" => {
            let mut b = session.tcp_endpoint();
            if let Some(addr) = parsed_addr { b = b.remote_addr(addr); }
            // ... apply common opts
            Box::new(b.listen().await?)
        },
        "tls" => {
            let mut b = session.tls_endpoint();
            if let Some(domain) = parsed_domain { b = b.domain(domain); }
            // ... apply common opts
            Box::new(b.listen().await?)
        },
        other => return Err(NgrokError::UnsupportedScheme { scheme: other.into() }),
    };

    Ok(EndpointListener { inner: tunnel, /* stored opts for getters */ })
}
```

**`EndpointListener` wraps a `Box<dyn Tunnel>`**
```rust
pub struct EndpointListener {
    inner: Box<dyn Tunnel>,
    // Store these from EndpointOptions so Endpoint trait getters work:
    traffic_policy: String,
    description: String,
    bindings: Vec<String>,
    pooling_enabled: bool,
    // Shutdown signal:
    closed_tx: Arc<watch::Sender<bool>>,
    closed_rx: watch::Receiver<bool>,
}

// Endpoint trait delegates to inner:
impl Endpoint for EndpointListener {
    fn id(&self) -> &str { self.inner.id() }
    fn url(&self) -> &str { self.inner.url() }       // EndpointInfo::url()
    fn protocol(&self) -> &str { self.inner.proto() } // EndpointInfo::proto()
    fn metadata(&self) -> &str { self.inner.metadata() }
    fn traffic_policy(&self) -> &str { &self.traffic_policy } // stored from opts
    // ...
}

// Stream delegates to inner:
impl Stream for EndpointListener {
    type Item = Result<EndpointConn, AcceptError>;
    fn poll_next(...) { self.inner.poll_next(...) }
}
```

**`EndpointForwarder` wraps a `Forwarder<T>`**
```rust
// Created by listen_and_forward() internally:
async fn start(self) -> Result<EndpointForwarder, NgrokError> {
    let session = self.agent.ensure_connected().await?;
    let upstream_url = Url::parse(&self.upstream.addr)?;

    // Dispatch same as listener, but call listen_and_forward instead:
    let forwarder = match url_scheme {
        "https" | "http" | None => {
            let mut b = session.http_endpoint();
            // ... apply opts
            b.listen_and_forward(upstream_url).await?
        },
        // ... tcp, tls
    };

    Ok(EndpointForwarder { /* wrap forwarder, store opts */ })
}
```

---

## The v2 Design: What to Build

### Design Principles

1. **Terminology alignment**: `Session` → `Agent` + `AgentSession`. `Tunnel` → `EndpointListener`. `Forwarder` → `EndpointForwarder`. These match ngrok's platform terminology.

2. **Traffic Policy replaces all middleware**: Instead of 25+ programmatic middleware methods (`.oauth()`, `.compression()`, `.basic_auth()`, `.allow_cidr()`, etc.), there is a single `.traffic_policy(yaml_string)` method. Traffic Policy is a YAML/JSON-based rules engine that decouples the SDK from ngrok's feature releases. This is the primary driver of API surface reduction.

3. **URL-scheme-based protocol selection**: Instead of `session.http_endpoint()` / `session.tcp_endpoint()` / `session.tls_endpoint()`, there is one builder with `.url("https://...")` / `.url("tcp://...")` / `.url("tls://...")`. The scheme determines the protocol.

4. **Builder pattern** (not Go's functional options): Idiomatic Rust uses builders with fluent methods.

5. **Keep `tracing`** for logging (Rust ecosystem standard — do not switch to `slog` or `log`).

6. **Keep `axum` and `hyper` feature-flag integrations**.

7. **Async-first**: All I/O methods are `async`. `EndpointListener` implements `Stream`, not a blocking `Accept()`.

### Core Types (~8 total)

#### `Agent` — replaces `Session`

Long-lived agent identity and configuration. Manages reconnection internally. Separates configuration (Agent) from connection state (AgentSession).

```rust
pub struct Agent { /* wraps Arc<AgentInner> — Clone is cheap */ }

impl Agent {
    pub fn builder() -> AgentBuilder;
    pub async fn connect(&self) -> Result<(), NgrokError>;
    pub async fn disconnect(&self) -> Result<(), NgrokError>;
    pub fn session(&self) -> Option<AgentSession>;
    pub fn endpoints(&self) -> Vec<EndpointInfo>;
    pub fn listen(&self) -> EndpointListenBuilder<'_>;
    pub fn forward(&self, upstream: Upstream) -> EndpointForwardBuilder<'_>;
    pub async fn forward_to(&self, addr: &str, opts: EndpointOptions) -> Result<EndpointForwarder, NgrokError>;
}
```

#### `AgentBuilder` — replaces `SessionBuilder`

```rust
pub struct AgentBuilder { /* config fields */ }

impl AgentBuilder {
    // Auth
    pub fn authtoken(self, token: impl Into<String>) -> Self;
    pub fn authtoken_from_env(self) -> Self;

    // Connectivity
    pub fn connect_url(self, url: impl Into<String>) -> Self;
    pub fn connect_cas(self, pem: impl Into<Vec<u8>>) -> Self;
    pub fn tls_config(self, config: rustls::ClientConfig) -> Self;
    pub fn connector(self, connector: /* existing connector type */) -> Self;
    pub fn proxy_url(self, url: impl Into<String>) -> Self;

    // Behavior
    pub fn auto_connect(self, auto: bool) -> Self; // default: true
    pub fn metadata(self, meta: impl Into<String>) -> Self;
    pub fn description(self, desc: impl Into<String>) -> Self;
    pub fn heartbeat_interval(self, interval: Duration) -> Self;
    pub fn heartbeat_tolerance(self, tolerance: Duration) -> Self;
    pub fn client_info(self, client_type: impl Into<String>, version: impl Into<String>, comments: Option<String>) -> Self;

    // Unified event handler (replaces separate connect/disconnect/heartbeat callbacks)
    pub fn event_handler(self, handler: impl Fn(Event) + Send + Sync + 'static) -> Self;

    // Unified RPC handler (replaces separate on_stop/on_restart/on_update)
    pub fn rpc_handler(self, handler: impl Fn(RpcRequest) -> Result<Vec<u8>, String> + Send + Sync + 'static) -> Self;

    pub fn build(self) -> Result<Agent, NgrokError>;
}
```

#### `AgentSession` — represents an active connection

```rust
pub struct AgentSession { /* connection state */ }

impl AgentSession {
    pub fn warnings(&self) -> &[String];
    pub fn started_at(&self) -> Instant;
    pub fn agent(&self) -> &Agent;
}
```

#### `Endpoint` trait — common interface for all endpoints

```rust
pub trait Endpoint: Send + Sync {
    fn id(&self) -> &str;
    fn url(&self) -> &str;
    fn protocol(&self) -> &str;        // sugar for url scheme
    fn metadata(&self) -> &str;
    fn description(&self) -> &str;
    fn traffic_policy(&self) -> &str;
    fn bindings(&self) -> &[String];
    fn pooling_enabled(&self) -> bool;
    fn agent(&self) -> &Agent;
    fn close(&mut self) -> impl Future<Output = Result<(), NgrokError>> + Send;
    fn closed(&self) -> impl Future<Output = ()> + Send;  // resolves when endpoint stops
}
```

#### `EndpointListener` — replaces `HttpTunnel`/`TcpTunnel`/`TlsTunnel`

Implements `Endpoint` + `Stream<Item = Result<Conn, NgrokError>>`. Under `#[cfg(feature = "axum")]`, also implements the axum listener trait.

#### `EndpointForwarder` — replaces `Forwarder<T>`

Implements `Endpoint` plus: `upstream_url()`, `upstream_protocol()`, `proxy_protocol()`.

#### `Upstream` — forwarding target configuration

```rust
pub struct Upstream {
    addr: String,
    protocol: Option<String>,
    tls_config: Option<rustls::ClientConfig>,
    proxy_proto: Option<ProxyProtoVersion>,
}

impl Upstream {
    pub fn new(addr: impl Into<String>) -> Self;
    pub fn protocol(self, proto: impl Into<String>) -> Self;
    pub fn tls_config(self, config: rustls::ClientConfig) -> Self;
    pub fn proxy_proto(self, version: ProxyProtoVersion) -> Self;
}
```

#### `Event` enum — unified events

```rust
#[derive(Debug, Clone)]
pub enum Event {
    AgentConnectSucceeded,
    AgentDisconnected { error: Option<NgrokError> },
    AgentHeartbeatReceived { latency: Duration },
}
```

### Endpoint Builder (unified — replaces 4 protocol-specific builders)

```rust
pub struct EndpointListenBuilder<'a> { /* agent ref + options */ }

impl<'a> EndpointListenBuilder<'a> {
    pub fn url(self, url: impl Into<String>) -> Self;
    pub fn traffic_policy(self, policy: impl Into<String>) -> Self;
    pub fn metadata(self, meta: impl Into<String>) -> Self;
    pub fn description(self, desc: impl Into<String>) -> Self;
    pub fn bindings(self, bindings: impl IntoIterator<Item = impl Into<String>>) -> Self;
    pub fn pooling_enabled(self, enabled: bool) -> Self;
    pub fn agent_tls_termination(self, config: rustls::ServerConfig) -> Self;
    pub async fn start(self) -> Result<EndpointListener, NgrokError>;
}
```

The `start()` method internally dispatches based on URL scheme:
- `https://` or `http://` or no URL → internal HTTP endpoint builder
- `tcp://` → internal TCP endpoint builder
- `tls://` → internal TLS endpoint builder

`EndpointForwardBuilder` has the same options plus returns `EndpointForwarder`.

### Default Agent and Top-Level Convenience Functions

```rust
/// Process-global default agent. Reads NGROK_AUTHTOKEN from env.
pub fn default_agent() -> Agent;
pub fn set_default_agent(agent: Agent);
pub fn reset_default_agent();

/// Convenience: listen using default agent
pub fn listen() -> EndpointListenBuilder<'static>;

/// Convenience: forward using default agent
pub fn forward(upstream: Upstream) -> EndpointForwardBuilder<'static>;

/// Convenience: forward to address using default agent (FFI-friendly single call)
pub async fn forward_to(addr: &str, opts: EndpointOptions) -> Result<EndpointForwarder, NgrokError>;
```

**CRITICAL**: `default_agent()` must be safe to call from inside a tokio runtime. Use `std::sync::OnceLock` + `std::sync::Mutex`, NOT `tokio::sync::Mutex` with `blocking_lock()` (which panics in async contexts).

### Error Type

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NgrokError {
    #[error("{message}")]
    Api { code: Option<String>, message: String },

    #[error("agent not connected")]
    NotConnected,

    #[error("unsupported URL scheme '{scheme}'")]
    UnsupportedScheme { scheme: String },

    #[error("invalid URL '{url}': {reason}")]
    InvalidUrl { url: String, reason: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    // ... other variants as needed
}

impl NgrokError {
    pub fn code(&self) -> Option<&str>;
}
```

**The internal `RpcError` wire type must NEVER appear in any public API signature.** Implement `From<RpcError> for NgrokError`.

### What to Remove from the Public API

All of these v1 items become `pub(crate)` or are deleted:

- **Protocol-specific builders**: `HttpTunnelBuilder`, `TcpTunnelBuilder`, `TlsTunnelBuilder`, `LabeledTunnelBuilder`
- **All middleware methods**: `.compression()`, `.circuit_breaker()`, `.basic_auth()`, `.oauth()`, `.oidc()`, `.allow_cidr()`, `.deny_cidr()`, `.mutual_tlsca()`, `.webhook_verification()`, `.allow_user_agent()`, `.deny_user_agent()`, `.request_header()`, `.response_header()`, `.remove_request_header()`, `.remove_response_header()`, `.host_header_rewrite()`, `.websocket_tcp_conversion()`, `.scheme()`
- **Middleware types**: `OauthOptions`, `OidcOptions`, `Policy`, `Rule`, `Action`
- **Old core types from public API**: `Session`, `SessionBuilder`, `Tunnel` trait, `TunnelBuilder` trait, `ForwarderBuilder` trait, `TunnelInfo`, `EndpointInfo`, `EdgeInfo`, `TunnelCloser`, `EdgeType`, `EdgeConn`, `EndpointConn`
- **Labeled tunnels**: `LabeledTunnelBuilder`, `LabeledTunnel` — removed entirely (edges were deprecated Dec 2025)
- **Merged into URL**: `.domain()`, `.remote_addr()`, `.scheme()`

The internal machinery behind these (the actual `Session`, tunnel builders, etc.) stays as `pub(crate)` because the new v2 types wrap them.

---

## The FFI Module: What Makes This an Embedding Target

### The Problem

Rust closures, async traits, and generic bounds cannot cross FFI boundaries. NAPI-RS, PyO3, Rustler, and magnus all need **concrete, non-generic structs** with simple method signatures. Each language wrapper currently has to write hundreds of lines of glue code to bridge these gaps.

### The Solution: `ffi` Feature Flag + `ffi` Module

Create `ngrok/src/ffi.rs` gated behind `#[cfg(feature = "ffi")]` (off by default):

```rust
#[cfg(feature = "ffi")]
pub mod ffi {
    use std::sync::Arc;

    /// Concrete, FFI-safe Agent — all generics erased.
    /// Clone is cheap (Arc).
    pub struct FfiAgent { /* Arc<Agent> */ }

    impl FfiAgent {
        pub fn new(agent: Agent) -> Self;

        pub async fn listen(&self, opts: FfiEndpointOptions) -> Result<FfiEndpointListener, FfiError>;
        pub async fn forward(&self, upstream: FfiUpstream, opts: FfiEndpointOptions) -> Result<FfiEndpointForwarder, FfiError>;
        pub async fn forward_to(&self, addr: &str, opts: FfiEndpointOptions) -> Result<FfiEndpointForwarder, FfiError>;

        /// Channel-based event delivery — poll from foreign runtime instead of registering closures.
        pub fn event_receiver(&self) -> tokio::sync::broadcast::Receiver<Event>;

        /// Channel-based RPC — foreign runtime reads requests, sends responses.
        pub fn rpc_receiver(&self) -> tokio::sync::mpsc::Receiver<FfiRpcRequest>;
    }

    /// Concrete endpoint options — no generics, no Into<>, all String fields.
    pub struct FfiEndpointOptions {
        pub url: Option<String>,
        pub traffic_policy: Option<String>,
        pub metadata: Option<String>,
        pub description: Option<String>,
        pub bindings: Vec<String>,
        pub pooling_enabled: Option<bool>,
    }

    /// Concrete upstream — no generics.
    pub struct FfiUpstream {
        pub addr: String,
        pub protocol: Option<String>,
        pub verify_upstream_tls: bool,
    }

    /// Error type that serializes cleanly across any FFI boundary.
    pub struct FfiError {
        pub code: Option<String>,
        pub message: String,
    }

    impl From<NgrokError> for FfiError { /* ... */ }

    pub struct FfiEndpointListener { /* wraps EndpointListener */ }

    impl FfiEndpointListener {
        pub fn id(&self) -> &str;
        pub fn url(&self) -> &str;
        pub fn protocol(&self) -> &str;
        pub fn metadata(&self) -> &str;
        pub async fn close(&mut self) -> Result<(), FfiError>;
        /// One-shot receiver — easier to bridge than impl Future.
        pub fn closed_receiver(&self) -> tokio::sync::oneshot::Receiver<()>;
    }

    pub struct FfiEndpointForwarder { /* wraps EndpointForwarder */ }

    impl FfiEndpointForwarder {
        pub fn id(&self) -> &str;
        pub fn url(&self) -> &str;
        pub fn upstream_url(&self) -> &str;
        pub async fn close(&mut self) -> Result<(), FfiError>;
        pub fn closed_receiver(&self) -> tokio::sync::oneshot::Receiver<()>;
    }

    /// RPC request with a response channel — foreign runtime reads method, sends response back.
    pub struct FfiRpcRequest {
        pub method: String,
        pub respond: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
    }
}
```

### Why This Design

| FFI Problem | Solution |
|---|---|
| Closures can't cross FFI | Channel-based `event_receiver()` and `rpc_receiver()` — wrapper polls in its own runtime |
| Generics/async traits not FFI-transparent | All `Ffi*` types are concrete structs, no type parameters |
| Two-step `Upstream` construction awkward for flat-config languages | `forward_to(addr, opts)` convenience method inlines it |
| `Drop`-based cleanup doesn't work with GC'd languages | `closed()` / `closed_receiver()` gives explicit shutdown signal |
| Error types require per-wrapper marshaling | `FfiError { code, message }` maps trivially to any language |
| Process-global `DefaultAgent` unsafe for multi-runtime hosts (Node workers, Python sub-interpreters) | `set_default_agent()` / `reset_default_agent()` for lifecycle control |

### Concurrency Safety Requirements

Every `Ffi*` type must be:
- **`Send + Sync`**: Safe to share across threads (Node.js worker threads, Python threads, BEAM schedulers)
- **`Clone` where appropriate**: `FfiAgent` should be `Clone` (cheap, via `Arc`)
- **No `&mut self` on shared types**: Use interior mutability (`Arc<Mutex<>>` or `Arc<RwLock<>>`) where mutation is needed
- **All async methods return concrete futures**: Not `impl Future` with lifetime parameters — use `BoxFuture<'static, _>` at the FFI boundary if needed

---

## Validation: FFI Wrapper Examples

Create `examples/ffi-validation/` at the workspace root with two complete wrapper skeletons.

### JavaScript / NAPI-RS (`examples/ffi-validation/javascript/`)

Create a minimal NAPI-RS crate that wraps `ngrok::ffi::*` types and exposes them to Node.js. This validates:

1. `FfiAgent` construction from a JS options object — no closures, no generics
2. `forward()` as a single async call — no `Upstream` object in JS
3. `listen()` returning endpoint info via simple getters
4. `FfiError` mapping to JS exceptions via `.to_string()`
5. Top-level convenience functions (`ngrokListen`, `ngrokForward`) that match the current `ngrok-javascript` API shape

**Files to create:**
- `Cargo.toml` — depends on `napi`, `napi-derive`, `ngrok` (with `ffi` feature), `tokio`
- `build.rs` — standard NAPI-RS build
- `src/lib.rs` — the wrapper (~150 lines of Rust)
- `index.d.ts` — expected TypeScript declarations (the contract)
- `test.mjs` — smoke test script

**The JS-facing API should look like this:**

```typescript
// Agent-based
const agent = NapiAgent.create({ authtokenFromEnv: true });
const fwd = await agent.forward({ addr: "localhost:8080", url: "https://app.ngrok.app" });
console.log(fwd.url);

// Top-level convenience
const listener = await ngrokListen({ url: "https://app.ngrok.app" });
console.log(listener.url);
```

**Target: under 150 lines of Rust, zero `unsafe`, zero manual error marshaling.**

### Elixir / Rustler (`examples/ffi-validation/elixir/`)

Create a minimal Rustler NIF crate that wraps `ngrok::ffi::*` types. This validates:

1. `FfiAgent` held as a NIF resource (opaque reference)
2. Async bridging via `DirtyIo` scheduler NIFs
3. `FfiError` mapping to `{:error, message}` tuples
4. Endpoint info returned as Elixir structs

**Files to create:**
- `native/ngrok_nif/Cargo.toml` — depends on `rustler`, `ngrok` (with `ffi` feature), `tokio`
- `native/ngrok_nif/src/lib.rs` — the NIF wrapper (~120 lines of Rust)
- `lib/ngrok/native.ex` — Elixir NIF stubs
- `lib/ngrok.ex` — high-level Elixir API wrapping the NIFs

**The Elixir-facing API should look like this:**

```elixir
{:ok, agent} = Ngrok.Native.create_agent("my-authtoken")
{:ok, fwd} = Ngrok.forward("localhost:8080", url: "https://app.ngrok.app")
IO.puts(fwd.url)
```

**Target: under 120 lines of Rust, zero `unsafe` beyond Rustler's generated code.**

---

## Implementation Phases

### Phase 1: New Types (Additive)

Create the new v2 source files alongside existing code. Nothing is deleted yet.

1. `ngrok/src/error.rs` — `NgrokError` enum with `From<RpcError>` conversion
2. `ngrok/src/agent.rs` — `Agent`, `AgentBuilder`, `AgentSession`
3. `ngrok/src/endpoint.rs` — `Endpoint` trait, `EndpointListener`, `EndpointForwarder`
4. `ngrok/src/endpoint_builder.rs` — `EndpointOptions`, `EndpointListenBuilder`, `EndpointForwardBuilder`
5. `ngrok/src/upstream.rs` — `Upstream` struct
6. `ngrok/src/event.rs` — `Event` enum
7. `ngrok/src/rpc_handler.rs` — `RpcRequest` trait, method constants
8. `ngrok/src/default_agent.rs` — `default_agent()`, `listen()`, `forward()`, `forward_to()`
9. `ngrok/src/ffi.rs` — all `Ffi*` types (behind `#[cfg(feature = "ffi")]`)

### Phase 2: Wire Internals

Connect new types to existing `Session`/`Tunnel`/`Forwarder` machinery:

1. `AgentBuilder::build()` stores config; `Agent::connect()` creates internal `Session`
2. `EndpointListenBuilder::start()` dispatches URL scheme → internal protocol builder
3. `EndpointListener` wraps internal tunnel types (enum or trait object)
4. `EndpointForwarder` wraps internal forwarder
5. Wire `event_handler` to session heartbeat/reconnect callbacks
6. Wire `rpc_handler` to session `on_stop`/`on_restart`/`on_update`
7. Implement `closed()` using `tokio::sync::watch` channel
8. Wire FFI channel-based alternatives

### Phase 3: Update Exports

1. Update `lib.rs`: add new module declarations and `pub use` for v2 types
2. Make v1 types `pub(crate)`: `Session`, `SessionBuilder`, all tunnel builders, `Forwarder<T>`
3. Update prelude to export only v2 types
4. Add `ffi` feature flag to `Cargo.toml`

### Phase 4: Update Examples

1. Delete `examples/labeled.rs`
2. Rewrite all examples to v2 API patterns
3. Include traffic policy YAML examples (as documentation replacement for removed middleware)
4. Include event handler and RPC handler demonstrations

### Phase 5: FFI Validation Examples

1. Create `examples/ffi-validation/javascript/` (NAPI-RS)
2. Create `examples/ffi-validation/elixir/` (Rustler)
3. Verify both compile: `cargo check -p ngrok-napi-validation` and `cargo check -p ngrok_nif`

### Phase 6: Tests

1. Unit tests for `NgrokError`, URL parsing, `Upstream` builder, `AgentBuilder` config
2. Test `default_agent()` doesn't panic in async context
3. Integration tests (in `online_tests.rs`) for v2 API paths
4. Test `closed()` future resolves on close

### Phase 7: Verification

1. `cargo clippy` — zero warnings
2. `cargo doc` — all public items documented
3. `cargo test`
4. Feature flag matrix: `cargo check`, `cargo check --features axum`, `cargo check --features hyper`, `cargo check --features ffi`, `cargo check --features axum,hyper,ffi`, `cargo check --no-default-features --features ring`
5. FFI examples compile
6. `muxado/` has zero changes

---

## Key Gotchas to Avoid

1. **Do NOT use `blocking_lock()` anywhere that might run inside a tokio runtime.** The `default_agent()` function is called from `listen()` and `forward()` which are async. Use `std::sync::OnceLock` + `std::sync::Mutex` for the global agent.

2. **Do NOT expose `RpcError` (the internal wire protocol error type) in any public API signature.** Every public method returns `Result<T, NgrokError>`.

3. **Do NOT use `#[allow(dead_code)]` broadly on entire modules.** If internal code becomes unused after making v1 types `pub(crate)`, either suppress on specific items with a comment, or remove truly dead code.

4. **Do NOT forget to store `EndpointOptions` fields on `EndpointListener`/`EndpointForwarder`.** The `Endpoint` trait has getters for `traffic_policy()`, `description()`, `bindings()`, `pooling_enabled()` — these values must be preserved after the tunnel is created, not discarded.

5. **Do NOT create `EndpointForwarder` with two copies of the tunnel inner.** Use a single shared reference or store info fields separately.

6. **The `ffi` module types must be `Send + Sync`.** This is non-negotiable for concurrent access from foreign runtimes.

7. **Keep `oidc` and `oauth` modules accessible as `pub(crate)`** — the internal HTTP builder still needs them for URL-scheme dispatch.

---

## Success Criteria

- [ ] `cargo check` passes with zero errors
- [ ] `cargo check --features ffi` passes
- [ ] `cargo clippy` has zero warnings
- [ ] `cargo test` passes (unit tests for all new types)
- [ ] The public API surface is ~8 core types and ~30 builder methods
- [ ] No internal types (`RpcError`, `Session`, protocol-specific builders) leak into the public API
- [ ] `default_agent()` is safe to call from async contexts
- [ ] Every `Endpoint` trait method returns real data (not empty strings)
- [ ] `closed()` future resolves when an endpoint is closed
- [ ] FFI validation example (JavaScript) compiles: `cargo check -p ngrok-napi-validation`
- [ ] FFI validation example (Elixir) compiles: `cargo check -p ngrok_nif`
- [ ] Both FFI wrappers are under 150 lines of Rust each
- [ ] `muxado/` crate has zero modifications
