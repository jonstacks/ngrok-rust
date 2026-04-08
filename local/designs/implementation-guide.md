# Implementation Guide — `ngrok-rust`

> This document is written for AI coding agents that will implement `ngrok-rust`.
> Follow every milestone in order. Do not skip ahead. Read each acceptance criterion
> before marking a milestone done.

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust toolchain | stable ≥ 1.78 | `rustup default stable` |
| cargo-llvm-cov | latest | `cargo install cargo-llvm-cov` |
| clippy | bundled with rustup | `rustup component add clippy` |
| rustfmt | bundled with rustup | `rustup component add rustfmt` |

---

## Implementation Order

### Milestone 1: Workspace Scaffold

**Files:** `Cargo.toml` (workspace), `muxado/Cargo.toml`, `muxado/src/lib.rs`,
`ngrok/Cargo.toml`, `ngrok/src/lib.rs`,
`ngrok-sync/Cargo.toml`, `ngrok-sync/src/lib.rs`,
`ngrok-testing/Cargo.toml`, `ngrok-testing/src/lib.rs`

**Goal:** `cargo build --workspace` succeeds with zero warnings.

**Steps:**
1. Create `Cargo.toml` as a Cargo workspace:
   ```toml
   [workspace]
   members = ["muxado", "ngrok", "ngrok-sync", "ngrok-testing"]
   resolver = "2"
   ```
2. Create `muxado/Cargo.toml`:
   ```toml
   [package]
   name = "muxado"
   version = "0.1.0"
   edition = "2021"
   description = "Stream-multiplexing protocol (muxado) for Rust"

   [dependencies]
   tokio = { version = "1", features = ["rt", "io-util", "sync", "time", "macros"] }
   bytes = "1"
   bitflags = "2"
   thiserror = "1"
   tracing = "0.1"
   rand = "0.8"

   [dev-dependencies]
   tokio = { version = "1", features = ["full"] }
   proptest = "1"
   criterion = { version = "0.5", features = ["async_tokio"] }
   ```
3. Create `ngrok/Cargo.toml` with initial dependencies:
   ```toml
   [package]
   name = "ngrok"
   version = "0.1.0"
   edition = "2021"
   
   [features]
   default = ["rustls-native-roots"]
   hyper = ["dep:hyper", "dep:hyper-util", "dep:http-body-util"]
   axum = ["dep:axum", "hyper"]
   aws-lc-rs = ["dep:aws-lc-rs", "rustls/aws-lc-rs"]
   rustls-native-roots = ["dep:rustls-native-roots"]
   online-tests = []
   testing = ["dep:ngrok-testing"]
   
   [dependencies]
   muxado = { path = "../muxado" }
   tokio = { version = "1", features = ["full"] }
   tokio-rustls = "0.26"
   rustls = { version = "0.23", default-features = false, features = ["ring", "tls12"] }
   rustls-native-roots = { version = "0.26", optional = true }
   aws-lc-rs = { version = "1", optional = true }
   futures = "0.3"
   bytes = "1"
   serde = { version = "1", features = ["derive"] }
   serde_json = "1"
   thiserror = "1"
   url = "2"
   tracing = "0.1"
   rand = "0.8"
   hyper = { version = "1", features = ["full"], optional = true }
   hyper-util = { version = "0.1", features = ["full"], optional = true }
   http-body-util = { version = "0.1", optional = true }
   axum = { version = "0.7", optional = true }
   ngrok-testing = { path = "../ngrok-testing", optional = true }
   
   [dev-dependencies]
   ngrok-testing = { path = "../ngrok-testing" }
   tokio = { version = "1", features = ["full"] }
   proptest = "1"
   criterion = { version = "0.5", features = ["async_tokio"] }
   ```
4. Create `ngrok-sync/Cargo.toml`:
   ```toml
   [package]
   name = "ngrok-sync"
   version = "0.1.0"
   edition = "2021"
   
   [dependencies]
   ngrok = { path = "../ngrok" }
   tokio = { version = "1", features = ["full"] }
   
   [dev-dependencies]
   ngrok-testing = { path = "../ngrok-testing" }
   ```
5. Create `ngrok-testing/Cargo.toml`:
   ```toml
   [package]
   name = "ngrok-testing"
   version = "0.1.0"
   edition = "2021"

   [dependencies]
   muxado = { path = "../muxado" }
   tokio = { version = "1", features = ["full"] }
   tokio-rustls = "0.26"
   rustls = { version = "0.23", default-features = false, features = ["ring", "tls12"] }
   rcgen = "0.12"
   bytes = "1"
   serde = { version = "1", features = ["derive"] }
   serde_json = "1"
   uuid = { version = "1", features = ["v4"] }
   ```
6. Create stub `lib.rs` files with `pub mod` placeholders for each crate.

**Acceptance:** `cargo build --workspace --all-features` exits 0; `cargo clippy --all-features -- -D warnings` exits 0.

---

### Milestone 2: muxado Frame Layer

**Crate:** `muxado`

**Files:** `muxado/src/frame/mod.rs`, `header.rs`, `data.rs`, `rst.rs`,
`wndinc.rs`, `goaway.rs`, `muxado/src/error.rs`

**Goal:** All muxado frames encode and decode correctly; unit tests pass.

**Steps:**
1. Implement the 8-byte frame header in `muxado/src/frame/header.rs`:
   ```
   [0..3] = length (24-bit big-endian) | type (upper nibble byte 3) | flags (lower nibble byte 3)
   [4..8] = stream_id (32-bit big-endian, MSB reserved = 0)
   ```
2. Implement `Frame` enum in `muxado/src/frame/mod.rs`: `Data(DataFrame)`, `Rst(RstFrame)`,
   `WndInc(WndIncFrame)`, `GoAway(GoAwayFrame)`, `Unknown { frame_type: u8, stream_id: u32, payload: Bytes }`.
3. Implement `read_frame(reader: &mut impl AsyncRead) -> Result<Frame, MuxadoError>`:
   - Read exactly 8 bytes for the header.
   - Read exactly `length` bytes for the payload.
4. Implement `write_frame(writer: &mut impl AsyncWrite, frame: &Frame) -> Result<(), MuxadoError>`.
5. `DataFrame` has `SYN` (0x2) and `FIN` (0x1) flags; `GoAwayFrame` has `last_stream_id`,
   `error_code`, and `debug_data` (capped at 1 MB).
6. Implement `MuxadoError` and `ErrorCode` in `muxado/src/error.rs` per `muxado-api.md`.
7. Write unit tests for every frame type: encode → decode → assert equality.
   Use `tokio::io::duplex` or `std::io::Cursor` as the underlying I/O.

**Acceptance:** `cargo test -p muxado -- frame` passes with ≥ 90% line coverage.

---

### Milestone 3: muxado Session and Stream

**Crate:** `muxado`

**Files:** `muxado/src/session.rs`, `session_config.rs`, `stream.rs`, `stream_id.rs`,
`stream_map.rs`, `window.rs`, `buffer.rs`

**Goal:** Two in-process muxado sessions can exchange data over streams bidirectionally.

**Steps:**
1. Implement `Session` in `muxado/src/session.rs`:
   - `client(transport, config)` and `server(transport, config)` constructors.
   - Client starts with `local.last_id = 1`; each open adds 2 → IDs 3, 5, 7, …
   - Server starts with `local.last_id = 0`; each open adds 2 → IDs 2, 4, 6, …
   - Spawn a **reader task** (`tokio::spawn`) that reads frames and dispatches them.
   - Spawn a **writer task** that drains a `mpsc::Sender<Frame>` and calls `write_frame`.
   - Implement `open_stream() -> Result<Stream, MuxadoError>` and
     `accept_stream() -> Result<Stream, MuxadoError>`.
2. Implement `SessionConfig` in `muxado/src/session_config.rs` per `muxado-api.md`.
3. Implement `StreamId` newtype in `muxado/src/stream_id.rs` with `is_client()` / `is_server()`.
4. Implement `Stream` as `AsyncRead + AsyncWrite` in `muxado/src/stream.rs`:
   - Inbound: `Mutex<VecDeque<Bytes>> + Notify` (see Decision 8).
   - Outbound: acquire `n` permits from a `Semaphore` before writing DATA frame.
   - Send SYN flag on first write only (`AtomicBool`).
   - Send FIN when `close_write()` is called.
   - Handle RST by poisoning both read and write halves.
   - Implement `next_frame_payload() -> Option<bytes::Bytes>` for zero-copy reads.
5. Implement `StreamMap`: `RwLock<HashMap<StreamId, Arc<StreamInner>>>` with `drain()`.
6. Handle GOAWAY: send GOAWAY then close all streams with ID > `last_stream_id`.
7. Implement `Session::close() -> Result<(), MuxadoError>` (sends GOAWAY).
8. Implement `Session::wait() -> SessionTermination`.

**Acceptance:**
```rust
#[tokio::test]
async fn muxado_bidirectional_echo() {
    let (client_io, server_io) = tokio::io::duplex(65536);
    let client = muxado::Session::client(client_io, Default::default());
    let server = muxado::Session::server(server_io, Default::default());
    // open stream, send "hello", receive "hello" echoed back
}
```

---

### Milestone 4: TypedStreamSession and Heartbeat

**Crate:** `muxado`

**Files:** `muxado/src/typed.rs`, `heartbeat.rs`

**Goal:** `TypedStreamSession` correctly prefixes streams with 4-byte type;
`Heartbeat` sends periodic pings and reports latency.

**Steps:**
1. `TypedStreamSession` wraps `Session` in `muxado/src/typed.rs`:
   - `open_typed_stream(stream_type: u32) -> Result<TypedStream>`: writes 4-byte big-endian type before returning.
   - `accept_typed_stream() -> Result<TypedStream>`: reads first 4 bytes, returns type + stream.
2. `Heartbeat` wraps `TypedStreamSession` in `muxado/src/heartbeat.rs`:
   - Uses reserved stream type `0xFFFF_FFFF`.
   - Heartbeat requester task: every `interval`, open a new typed stream, write 4-byte random `u32`, read echo, verify, report RTT via `mpsc`.
   - Heartbeat responder task: `accept_typed_stream`, echo back 4 bytes.
   - If echo mismatch: close stream.
   - If no echo within `tolerance`: call disconnect callback.
3. Implement `HeartbeatConfig` per `muxado-api.md`.

**Acceptance:** `cargo test -p muxado -- heartbeat` — heartbeat latency is
reported, timeout triggers disconnect.

**At this point `cargo test -p muxado` must pass all muxado tests in isolation,
with no dependency on the `ngrok` crate.**

---

### Milestone 5: Wire Protocol Messages

**Crate:** `ngrok`

**Files:** `ngrok/src/proto/msg.rs`

**Goal:** All ngrok JSON wire messages serialize and deserialize correctly.

**Steps:**
1. Implement the following structs with `#[derive(Serialize, Deserialize, Debug, Default)]`:
   - `Auth { version: Vec<String>, client_id: String, extra: AuthExtra }`
   - `AuthExtra { os, arch, authtoken, version, hostname, user_agent, metadata, cookie, heartbeat_interval_ms: i64, heartbeat_tolerance_ms: i64, stop_unsupported_error: Option<String>, restart_unsupported_error: Option<String>, update_unsupported_error: Option<String> }`
   - `AuthResp { version: String, extra: AuthRespExtra, error: String }`
   - `AuthRespExtra { region, agent_session_id, cookie, connect_addresses: Vec<String> }`
   - `BindReq { client_id: String, proto: String, opts: serde_json::Value, forwards: String }`
   - `BindResp { client_id: String, url: String, proto: String, error: String }`
   - `ProxyHeader { id: String, client_addr: String, proto: String, edge_type: String, passthrough_tls: bool }`
   - `StopTunnel { id: String, message: String, error_code: String }`
   - `SrvInfoResp { region: String }`
2. Use `serde(rename = "Id")` for fields where the JSON key differs from the Rust name.
3. `authtoken` in `AuthExtra` must implement `Display` as `"HIDDEN"` (a newtype wrapper).

**Acceptance:** Each message round-trips through `serde_json::to_string` → `serde_json::from_str`.

---

### Milestone 6: RawSession (Tunnel Client)

**Crate:** `ngrok`

**Files:** `ngrok/src/tunnel/raw_session.rs`

**Goal:** `RawSession` can authenticate with `Auth` and create tunnels with `Bind`
over a `muxado::TypedStreamSession`.

**Steps:**
1. `RawSession` holds a `muxado::TypedStreamSession` (from the `muxado` crate).
2. `auth(extra: AuthExtra) -> Result<AuthResp>`:
   - Open typed stream with type `0` (AuthReq).
   - Write JSON-encoded `Auth` then close write half.
   - Read JSON-encoded `AuthResp`.
   - If `resp.error` is non-empty, return `Error::Cloud { code, message }`.
3. `bind(req: BindReq) -> Result<BindResp>`:
   - Open typed stream with type `1` (BindReq).
   - Write JSON-encoded `BindReq`, close write.
   - Read JSON-encoded `BindResp`.
4. `srv_info() -> Result<SrvInfoResp>`:
   - Open typed stream with type `8`.
   - Write empty JSON `{}`, read `SrvInfoResp`.
5. Accept loop task: continuously `accept_typed_stream()` on the `muxado::TypedStreamSession`:
   - Type `3` (ProxyReq): read `ProxyHeader`, route stream to the correct tunnel
     via a `HashMap<String, mpsc::Sender<NgrokStream>>`.
   - Type `5` (StopReq): call registered RPC handler.
   - Type `4` (RestartReq): call RPC handler.
   - Type `6` (UpdateReq): call RPC handler.
   - Type `9` (StopTunnelReq): read `StopTunnel`, send error to corresponding tunnel's sender.

**Acceptance:** Unit test with `MockNgrokServer` completes Auth → Bind → ProxyReq dispatch.

---

### Milestone 7: ReconnectingSession

**Crate:** `ngrok`

**Files:** `ngrok/src/tunnel/reconnecting.rs`

**Goal:** `ReconnectingSession` dials, authenticates, and re-binds all tunnels after
a transport failure, using exponential backoff.

**Steps:**
1. Hold `Vec<TunnelConfig>` (the bind params for all active tunnels).
2. On session drop: emit `AgentDisconnected` event, begin backoff loop:
   - `initial = 500ms`, `max = 30s`, `factor = 2.0`, no jitter.
   - Use `tokio::time::sleep(backoff_duration)`.
3. After dial success: `Auth` with stored `cookie` from previous `AuthResp`.
4. After auth success: `Bind` all tunnels in `Vec<TunnelConfig>`.
5. After all binds succeed: emit `AgentConnectSucceeded`, reset backoff.
6. If `agent.close()` is called: set an atomic `closed` flag, exit loop on next iteration.

**Acceptance:** Test drops the transport, waits 200ms, verifies re-bind was called.

---

### Milestone 8: Agent and Public API

**Crate:** `ngrok`

**Files:** `ngrok/src/agent.rs`, `session.rs`, `listener.rs`, `forwarder.rs`,
`upstream.rs`, `options.rs`, `events.rs`, `error.rs`, `rpc.rs`, `diagnose.rs`,
`defaults.rs`

**Goal:** The full public API is usable; `examples/basic.rs` compiles and would
connect to real ngrok (not tested in CI).

**Steps:**
1. Implement `AgentBuilder` with all methods from `api.md`. `build()` spawns the
   `ReconnectingSession` task if `auto_connect = true`, or defers until first
   `listen()` / `forward()`.
2. `Agent` is `Arc<AgentInner>`. `AgentInner` contains:
   - `state: tokio::sync::RwLock<AgentState>` (Created / Connecting / Connected / Disconnecting)
   - `reconnecting: Arc<ReconnectingSession>`
   - `endpoints: RwLock<Vec<EndpointHandle>>`
   - `event_handlers: Vec<Box<dyn Fn(Event) + Send + Sync>>`
3. `EndpointListener` wraps `mpsc::Receiver<NgrokStream>`. Implement
   `futures::Stream` by polling the receiver.
4. `EndpointForwarder` spawns a `forward_loop` task. All upstreams (including `http`/`https`)
   use `tokio::io::copy_bidirectional` for raw TCP proxying — ngrok cloud handles all HTTP
   framing and policy enforcement before the bytes reach the agent. The `hyper` feature is
   strictly optional and only needed if the user wants to run their own hyper server on top
   of a `NgrokStream`; it is not required for forwarding.
5. Emit events synchronously from `agent.emit_event(&self, evt: Event)` which iterates
   `event_handlers`. Document that handlers must not block.
6. Implement `AgentBuilder::on_rpc` — stores a single `Box<dyn Fn(RpcRequest) -> RpcResponse>`.
7. Implement `Agent::diagnose` — TCP dial → TLS handshake → `raw_session.srv_info()`.
8. Implement `NgrokStream::next_bytes()`:
   ```rust
   pub async fn next_bytes(&mut self) -> Option<bytes::Bytes> {
       // Delegate to the muxado stream's zero-copy frame extraction.
       // The buffer is a tokio::sync::Notify + Mutex<VecDeque<Bytes>>;
       // we pop the front Bytes arc — zero copy, no intermediate Vec<u8>.
       self.inner.next_frame_payload().await
   }
   ```
   The `next_frame_payload()` method is on the `muxado::Stream` type (see muxado
   Milestone 3). It returns the payload `Bytes` from the head of the inbound
   `VecDeque<Bytes>`. This avoids any `AsyncRead::poll_read` buffer copy.
9. Implement package-level `listen()` and `forward()` in `defaults.rs`:
   reads `NGROK_AUTHTOKEN`, builds agent, calls respective method.

**Acceptance:** `cargo test --all-features -- agent` passes. `NgrokStream::next_bytes()`
is tested with a mock muxado frame injection (using `muxado::Session::server()`
in-process). Compile-test that examples in `examples.md` parse successfully with
`cargo check --example <name>`.

---

### Milestone 9: hyper and axum Integration

**Crate:** `ngrok`

**Files:** `ngrok/src/integrations/hyper.rs`, `axum.rs`

**Goal:** Feature-gated integrations compile; axum integration is end-to-end tested.

**Steps:**
1. `feature = "hyper"`: The only requirement is that `hyper-util` is available as a
   dependency (it is already declared in `Cargo.toml` behind the `hyper` feature flag).
   No trait impls are needed on ngrok types. The `integrations/hyper.rs` file should
   contain a short usage note explaining the integration pattern:
   ```rust
   // hyper 1.x integration
   //
   // There is no trait to implement. Users bridge NgrokStream to hyper's IO
   // layer by wrapping it with TokioIo from hyper-util:
   //
   //   use hyper_util::rt::TokioIo;
   //   let io = TokioIo::new(stream);  // stream: NgrokStream
   //
   // Then pass `io` to hyper's Builder::serve_connection(io, service).
   // NgrokStream already implements tokio::io::AsyncRead + AsyncWrite,
   // which is all TokioIo requires.
   ```
2. `feature = "axum"`: implement `axum::serve::Listener` on `EndpointListener`
   (see `api.md` for the full impl with `accept()` and `local_addr()`). axum
   internally calls `TokioIo::new(stream)` on the `Io` type, so no extra wrapping
   is needed here.
3. Write a compile test for the axum integration (does not need a tokio runtime):
   ```rust
   #[cfg(test)]
   mod compile_tests {
       fn _assert_listener_impl(_: impl axum::serve::Listener) {}
       fn _check(l: super::EndpointListener) { _assert_listener_impl(l); }
   }
   ```

**Acceptance:** `cargo test --features hyper` and `cargo test --features axum` both
pass. `cargo check --features hyper` produces no warnings about unused items in
`integrations/hyper.rs`.

---

### Milestone 10: `ngrok-testing` Mock Server

**Crate:** `ngrok-testing`

**Files:** `ngrok-testing/src/lib.rs`, `server.rs`, `endpoint.rs`, `fixtures.rs`

**Goal:** `MockNgrokServer` handles Auth + Bind + Proxy injection; all offline tests pass.

**Steps:**
1. `MockNgrokServer::start()`:
   - Bind a `tokio::net::TcpListener` on `127.0.0.1:0`.
   - Generate a self-signed TLS certificate (`rcgen` crate).
   - Accept TLS connections with `tokio-rustls`.
   - For each connection: speak muxado server role using `muxado::Session::server()`,
     wrap in `muxado::TypedStreamSession`, handle `TypedStream` calls:
     - Type 0 (Auth): return a canned `AuthResp { version: "3", extra: { agent_session_id: uuid, ... } }`.
     - Type 8 (SrvInfo): return `SrvInfoResp { region: "us" }`.
     - Type 1 (Bind): return `BindResp { url: "https://<random>.mock.ngrok.app", ... }`;
       store tunnel handle in `HashMap<endpoint_id, MockEndpoint>`.
2. `MockNgrokServer::accept_endpoint()` returns the next `MockEndpoint` from a
   `mpsc::Receiver<MockEndpoint>`.
3. `MockEndpoint::inject_connection(stream)`:
   - Open a new muxado stream on the server's `muxado::TypedStreamSession`.
   - Write a `ProxyHeader` JSON with the endpoint ID.
   - Pipe `stream` to the muxado stream using `tokio::io::copy_bidirectional`.
4. `MockNgrokServer::drop_session()`: close the server-side `muxado::Session` to
   simulate a transport failure.
5. `MockNgrokServer::send_rpc(&self, method: &str)`: open a new typed muxado stream with
   the appropriate RPC type code (type 5 = StopReq, type 4 = RestartReq, type 6 = UpdateReq)
   on the server session and write an empty JSON payload, triggering the agent's registered
   RPC handler. Store a `HashMap<&str, u32>` mapping method name → type code to look up the
   correct stream type.
6. Implement `AgentTestExt` on `AgentBuilder` (feature-gated behind `ngrok-testing`):
   - `with_mock_server(url)` → calls `connect_url(url)`.
   - `danger_accept_any_cert()` → installs a `rustls::ClientConfig` that skips certificate
     verification (for the self-signed test cert).

**Acceptance:** All tests in `testing.md` example test cases pass without network access,
including the following additional test for RPC dispatch:

```rust
#[tokio::test]
async fn rpc_handler_invoked_on_stop_agent() {
    use ngrok_testing::MockNgrokServer;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

    let (server, connect_url) = MockNgrokServer::start().await;
    let rpc_called = Arc::new(AtomicBool::new(false));
    let rc = rpc_called.clone();

    let _agent = ngrok::Agent::builder()
        .authtoken("test-token")
        .with_mock_server(&connect_url)
        .on_rpc(move |req| {
            if matches!(req.method, ngrok::RpcMethod::StopAgent) {
                rc.store(true, Ordering::Relaxed);
            }
            ngrok::RpcResponse::default()
        })
        .build()
        .await
        .unwrap();

    server.send_rpc("StopAgent").await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(rpc_called.load(Ordering::Relaxed), "StopAgent RPC handler was not called");
}
```

---

### Milestone 11: `ngrok-sync` Control-Plane Wrapper Crate

**Crate:** `ngrok-sync`

**Files:** `ngrok-sync/src/lib.rs`, `ngrok-sync/src/agent.rs`, `ngrok-sync/src/listener.rs`,
`ngrok-sync/src/forwarder.rs`

**Goal:** All `ngrok` control-plane APIs are available as blocking functions;
`EndpointListener::accept()` returns `ngrok::NgrokStream` (no blocking I/O wrapper);
`agent.runtime()` is accessible; `cargo test -p ngrok-sync` passes.

> **Critical constraint:** Do NOT implement `std::io::Read + Write` wrappers on
> `NgrokStream`. Blocking the host thread on per-read `block_on` calls starves
> event-driven runtimes. The data plane is the FFI adapter's responsibility.

**Steps:**

1. Implement `AgentBuilder` and `Agent` state structs in `ngrok-sync/src/agent.rs`:
   ```rust
   use std::sync::Arc;
   use tokio::runtime::Runtime;

   pub struct AgentBuilder { inner: ngrok::AgentBuilder, rt: Option<Arc<Runtime>> }
   pub struct Agent { inner: Arc<ngrok::Agent>, rt: Arc<Runtime> }

   impl AgentBuilder {
       pub fn new() -> Self { Self { inner: ngrok::Agent::builder(), rt: None } }
       pub fn runtime(mut self, rt: Arc<Runtime>) -> Self { self.rt = Some(rt); self }
       pub fn authtoken(mut self, t: impl Into<String>) -> Self {
           self.inner = self.inner.authtoken(t); self
       }
       // ... mirror all AgentBuilder setters ...
       pub fn build(self) -> Result<Agent, ngrok::Error> {
           let rt = self.rt.unwrap_or_else(|| Arc::new(
               Runtime::new().expect("failed to create Tokio runtime")
           ));
           let inner = rt.block_on(self.inner.build())?;
           Ok(Agent { inner: Arc::new(inner), rt })
       }
   }

   impl Agent {
       /// Expose the shared runtime so FFI adapters can spawn data-plane tasks.
       pub fn runtime(&self) -> Arc<Runtime> { Arc::clone(&self.rt) }

       pub fn connect(&self) -> Result<(), ngrok::Error> {
           self.rt.block_on(self.inner.connect())
       }
       pub fn disconnect(&self) -> Result<(), ngrok::Error> {
           self.rt.block_on(self.inner.disconnect())
       }
       pub fn session(&self) -> Option<ngrok::AgentSession> { self.inner.session() }
       pub fn endpoints(&self) -> Vec<ngrok::EndpointInfo> { self.inner.endpoints() }
       pub fn listen(&self, opts: ngrok::EndpointOptions) -> Result<EndpointListener, ngrok::Error> {
           let listener = self.rt.block_on(self.inner.listen(opts))?;
           Ok(EndpointListener { inner: listener, rt: Arc::clone(&self.rt) })
       }
       pub fn forward(&self, upstream: ngrok::Upstream, opts: ngrok::EndpointOptions)
           -> Result<EndpointForwarder, ngrok::Error>
       {
           let fwd = self.rt.block_on(self.inner.forward(upstream, opts))?;
           Ok(EndpointForwarder { inner: fwd, rt: Arc::clone(&self.rt) })
       }
   }
   ```

2. Implement `EndpointListener` in `ngrok-sync/src/listener.rs`:
   ```rust
   pub struct EndpointListener { inner: ngrok::EndpointListener, rt: Arc<Runtime> }

   impl EndpointListener {
       /// Block until the next inbound connection arrives.
       ///
       /// Returns `ngrok::NgrokStream` — the async type. This is a one-time block_on
       /// (acceptable: one call per connection, not one per read). The caller drives
       /// I/O by spawning a task on `agent.runtime()`.
       pub fn accept(&self) -> Result<ngrok::NgrokStream, ngrok::Error> {
           self.rt.block_on(self.inner.accept())
           // No NgrokStream wrapper — return the async stream directly.
       }
       pub fn url(&self) -> &url::Url { self.inner.url() }
       pub fn close(self) -> Result<(), ngrok::Error> {
           self.rt.block_on(self.inner.close())
       }
   }
   ```
   > **No `NgrokStream` wrapper struct in this crate.** Do not create one.

3. Implement `EndpointForwarder` in `ngrok-sync/src/forwarder.rs`:
   ```rust
   pub struct EndpointForwarder { inner: ngrok::EndpointForwarder, rt: Arc<Runtime> }

   impl EndpointForwarder {
       pub fn url(&self) -> &url::Url { self.inner.url() }
       pub fn close(self) -> Result<(), ngrok::Error> {
           self.rt.block_on(self.inner.close())
       }
   }
   ```

4. In `ngrok-sync/src/lib.rs`, re-export types:
   ```rust
   pub use ngrok::{
       EndpointOptions, Upstream, Event, Error, DiagnoseError,
       ProxyProtoVersion, AgentSession, EndpointInfo,
       RpcRequest, RpcResponse,
       NgrokStream,   // re-exported for FFI adapters that want one import
   };
   pub mod agent;     pub use agent::{Agent, AgentBuilder};
   pub mod listener;  pub use listener::EndpointListener;
   pub mod forwarder; pub use forwarder::EndpointForwarder;
   ```

5. Write `ngrok-testing`-backed control-plane tests:
   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;

       #[tokio::test]
       async fn blocking_control_plane_roundtrip() {
           let (server, connect_url) = ngrok_testing::MockNgrokServer::start().await;
           let agent = AgentBuilder::new()
               .connect_url(connect_url)
               .authtoken("test-token")
               .build()
               .unwrap();
           let listener = agent.listen(EndpointOptions::default()).unwrap();
           assert!(listener.url().as_str().contains("ngrok"));
           listener.close().unwrap();
           agent.disconnect().unwrap();
       }

       #[tokio::test]
       async fn accept_returns_async_ngrokstream() {
           let (server, connect_url) = ngrok_testing::MockNgrokServer::start().await;
           let agent = AgentBuilder::new()
               .connect_url(connect_url)
               .authtoken("test-token")
               .build()
               .unwrap();
           let listener = agent.listen(EndpointOptions::default()).unwrap();
           let rt = agent.runtime();

           // Simulate a data-plane task using the shared runtime
           server.inject_connection(listener.url());
           let stream: ngrok::NgrokStream = listener.accept().unwrap();

           let (tx, rx) = std::sync::mpsc::channel();
           rt.spawn(async move {
               let mut s = stream;
               let bytes = s.next_bytes().await;
               tx.send(bytes.is_some()).unwrap();
           });
           assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
       }
   }
   ```

6. Verify no `block_on` calls exist in the data path:
   ```sh
   grep -n "block_on" ngrok-sync/src/listener.rs  # should only be in accept() and close()
   grep -n "NgrokStream" ngrok-sync/src/listener.rs  # return type should be ngrok::NgrokStream
   grep -rn "impl.*Read.*for.*NgrokStream\|impl.*Write.*for.*NgrokStream" ngrok-sync/
   # ↑ must produce no matches
   ```

**Acceptance:**
- `cargo test -p ngrok-sync` passes.
- `cargo doc -p ngrok-sync --no-deps` produces complete docs.
- `grep -rn "impl std::io::Read\|impl std::io::Write" ngrok-sync/src/` produces zero matches.
- `EndpointListener::accept()` return type is `ngrok::NgrokStream` (verified by doc output).
- `Agent::runtime()` returns `Arc<tokio::runtime::Runtime>` (verified by usage in test).
- `AgentBuilder::runtime()` allows injecting a pre-built `Arc<Runtime>`.

---

## Coding Conventions

### Naming
- Types: `UpperCamelCase`
- Functions/methods/fields: `snake_case`
- Constants: `SCREAMING_SNAKE_CASE`
- Acronyms: treat as words — `NgrokStream`, `RpcRequest`, `TlsConfig` (not `TLSConfig`)
- Use `_inner` suffix for `Arc`-wrapped state structs: `AgentInner`, `SessionInner`

### Error Handling
- Never use `.unwrap()` or `.expect()` in library code (only in tests and examples)
- Propagate with `?`; add context with `.map_err(|e| Error::Io(e))` or custom `From` impls
- All `Result` types in `ngrok` public API use `ngrok::Error` or `ngrok::DiagnoseError`
- All `Result` types in `muxado` public API use `muxado::MuxadoError`
- The `ngrok` crate wraps `MuxadoError` into `Error::Muxado(String)` via `Display`

### Documentation
- Every `pub` item must have a `///` doc comment
- Doc comments start with a verb: `/// Opens a new listener endpoint.`
- Include an `# Examples` section for non-trivial public methods
- `// SAFETY:` comment required on every `unsafe` block

### File Organisation
- One primary type per file; helper types in the same file
- `mod.rs` files only re-export; no implementation code
- `integrations/` modules are `#[cfg(feature = "...")]` at the module level

### Formatting / Linting
- `cargo fmt` (no custom settings)
- `cargo clippy --all-targets --all-features -- -D warnings`

---

## Critical Invariants

- **Single transport writer**: only the writer task writes to the muxado transport;
  never write from any other task. This invariant belongs to the `muxado` crate.
- **Stream ID parity**: client allocates odd IDs, server allocates even IDs; the
  implementation must enforce this and reject violating SYN frames.
- **SYN flag exactly once**: a stream's first DATA frame must carry SYN; subsequent
  frames must not.
- **FIN idempotency**: calling `close_write()` on an already half-closed stream must
  be a no-op (not an error).
- **Event handlers never block**: emit events synchronously from the SDK; document that
  handlers blocking will stall the session task.
- **ngrok CA embedded at compile time**: never skip CA verification in production code;
  only `danger_accept_any_cert()` in `ngrok-testing` bypasses this.
- **`#[non_exhaustive]` on all public enums in `ngrok` and `ngrok-sync`**: ensures
  minor-version forward compatibility. The `muxado` crate's `MuxadoError` and
  `ErrorCode` are **not** `#[non_exhaustive]` because they map to a fixed wire protocol.
- **`Arc<>` not `Rc<>`**: all shared state uses `Arc` so it can be sent across `tokio`
  task boundaries (`Send + Sync`).
- **No `.block_on()` in `ngrok` or `muxado` crates**: only `ngrok-sync` may use blocking
  runtime calls. All `ngrok` and `muxado` public APIs are `async`.
- **Reconnect respects `closed` flag**: after `agent.disconnect()`, the reconnect loop
  must not attempt to re-dial.
- **Crate boundary**: the `muxado` crate must not import any types from the `ngrok`
  crate. Dependency flows one way: `ngrok` → `muxado`.

---

## Common Pitfalls

| Pitfall | How to Avoid |
|---------|-------------|
| Calling `tokio::runtime::Runtime::block_on` inside an async context | Never do this in `ngrok` or `muxado`; only in `ngrok-sync` |
| Holding a `RwLock` guard across an `.await` point | Always drop locks before `await`; use `let val = { lock.read().clone() };` pattern |
| Forgetting `#[non_exhaustive]` on new public enums | Clippy lint `clippy::exhaustive_enums` — enable it in `ngrok` and `ngrok-sync` |
| Forgetting `// SAFETY:` on `unsafe` blocks | `cargo clippy` will warn; treat as error |
| Using `std::sync::Mutex` around an `async`-capable type | Use `tokio::sync::Mutex` when the guard must be held across `.await` |
| Implementing `AsyncRead`/`AsyncWrite` with incorrect `Poll` semantics | Use `tokio_util::io::poll_read_buf` / `poll_write_buf` helpers; never spin-poll |
| Stream ID overflow: adding 2 to `u32::MAX - 1` wraps | Assert `next_id <= 0x7FFF_FFFF`; return `MuxadoError::StreamsExhausted` |
| JSON field name mismatches with Go wire format | Use `#[serde(rename = "...")]` for every field where the JSON key differs (e.g., `"Id"` → `id`) |
| Forgetting to handle the `GoAway` frame in the reader task | `GoAway` must drain all pending accepts and close all streams |
| UniFFI `async` bridging deadlock | Always `block_on` on a **separate** runtime, not the current tokio context |
| Accidentally importing `ngrok` types in `muxado` | The `muxado` crate has no dependency on `ngrok` — this is enforced by `Cargo.toml` |

---

## Verification Checklist

Before marking the implementation complete:

- [ ] All `pub` symbols have `///` doc comments (all crates including `muxado`)
- [ ] `cargo test --workspace --all-features` passes
- [ ] `cargo test --workspace --no-default-features` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `cargo llvm-cov --all-features` reports ≥ 80% total line coverage
- [ ] All eight code examples from `examples.md` compile: `cargo check --example <name>`
- [ ] `cargo test -p muxado` — frame round-trips, stream bidirectional exchange, heartbeat, typed streams. All pass in isolation with no `ngrok` crate dependency.
- [ ] `cargo test -p ngrok-testing` — mock server accept/inject passes (uses `muxado` directly)
- [ ] `ngrok-sync` public API contract stable: `cargo doc -p ngrok-sync --no-deps` produces complete docs with no missing items (`ngrok-uniffi` and other FFI adapters live in external repos and are not built here)
- [ ] `cargo doc -p muxado --no-deps` produces complete docs for the muxado crate
- [ ] Feature flag matrix checked:
  - [ ] `--no-default-features`
  - [ ] `--features hyper`
  - [ ] `--features axum`
  - [ ] `--features aws-lc-rs`
  - [ ] `--all-features`
- [ ] Crate publish order verified: `muxado` → `ngrok` → `ngrok-testing` → `ngrok-sync`
- [ ] `CHANGELOG.md` entry created for v0.1.0
- [ ] `README.md` quick-start snippet is copy-paste runnable (offline compile check)
