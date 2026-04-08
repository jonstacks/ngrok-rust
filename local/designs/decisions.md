# Design Decisions — `ngrok-rust`

---

## Decision 1: Builder Pattern for Agent and Endpoint Configuration

**Status:** Accepted

### Context

ngrok-go uses functional options (`func(*opts)`), which is idiomatic Go. Rust has no
equivalent — closures mutating a private struct would require `pub(crate)` internals.
The primary alternatives are builder structs and config structs.

### Decision

Use the builder pattern (`AgentBuilder`, `EndpointOptionsBuilder`) for all
multi-field configuration. Builders own the data until `.build()` is called.
Terminal methods are `async` where I/O is needed (e.g., `AgentBuilder::build`
dials the control plane).

### Consequences

- Callers get compile-time type safety and IDE autocompletion.
- Builders are `#[must_use]` to catch forgotten `.build()` calls.
- Adding new optional fields is non-breaking (builder methods are additive).
- Slightly more verbose than functional options for one-liners; acceptable.

### Alternatives Considered

- **Config struct**: e.g., `AgentConfig { authtoken: Option<String>, ... }`. Does not
  prevent constructing invalid configs at compile time; requires `Default` impl.
- **Functional options in Rust**: `Vec<Box<dyn FnOnce(&mut AgentOpts)>>`. Awkward API,
  harder to type-check, obscures what fields are set.

---

## Decision 2: `async/await` + `tokio` as the Only Async Runtime

**Status:** Accepted

### Context

Rust has multiple async runtimes (`tokio`, `async-std`, `smol`). The hyper and axum
ecosystems are built on `tokio`. The library needs async I/O for concurrent tunnel
streams.

### Decision

Target `tokio` exclusively. Use `tokio::net`, `tokio::sync`, `tokio::io`, and
`tokio::task` throughout. Do not abstract over runtimes via traits.

### Consequences

- First-class integration with `hyper` and `axum` (both tokio-native) without adapters.
- Users must run under a `tokio` runtime (single-threaded or multi-threaded).
- No `async-std` or `smol` support. This is the dominant choice in the Rust web
  ecosystem and is unlikely to be a blocker.

### Alternatives Considered

- **Runtime-agnostic via `futures` traits**: technically possible but adds significant
  complexity (no `tokio::spawn`, no `tokio::time`, custom executor injection). Not worth
  the maintenance cost for the marginal audience using other runtimes.

---

## Decision 3: `rustls` for TLS, `aws-lc-rs` as an Optional Backend

**Status:** Accepted

### Context

TLS is required for the control-plane connection to `connect.ngrok-agent.com:443`.
`native-tls` links against system OpenSSL/SChannel/SecureTransport, which varies by
platform. `rustls` is a pure-Rust TLS implementation with pluggable crypto backends.

### Decision

Use `rustls` as the TLS library. The default crypto backend is `ring`. A Cargo feature
flag `aws-lc-rs` switches to the `aws-lc-rs` backend (FIPS 140-3 compliant), which is
required for certain regulated environments (US government, financial services).

The ngrok CA certificate is embedded at compile time via `include_bytes!` and added to
a `rustls::RootCertStore`.

### Consequences

- No system OpenSSL dependency; fully cross-compiled.
- `aws-lc-rs` is a build-time opt-in; it requires a C compiler and cmake.
- Callers using custom TLS config pass a `rustls::ClientConfig` directly —
  no wrapped type needed.

### Alternatives Considered

- **`native-tls`**: platform-specific, hard to cross-compile, no FIPS path on Linux.
- **`openssl` crate**: links against system OpenSSL, painful on Windows/macOS.

---

## Decision 4: Implement muxado in Rust (No C FFI)

**Status:** Accepted (updated by Decision 10 — muxado extracted to its own crate)

### Context

muxado is implemented in Go (`golang.ngrok.com/muxado/v2`). Options are: (a) call into
the Go library via CGo/FFI, (b) implement muxado in Rust, (c) replace muxado with an
existing Rust multiplexer (e.g., yamux).

### Decision

Implement muxado in Rust, faithfully following the wire spec in `specs/muxado/`. The
Rust implementation lives in the `muxado` workspace crate (`muxado` on crates.io)
and covers all frame types, stream lifecycle, flow control, `TypedStreamSession`, and
`Heartbeat`. See Decision 10 for the rationale behind extracting it as a standalone
crate and [muxado-api.md](muxado-api.md) for its public API.

### Consequences

- No CGo or FFI overhead; pure Rust call chain.
- Wire-compatible with the Go server: same frame encoding, same stream ID parity,
  same error codes.
- The muxado implementation must be maintained in sync with any protocol changes from
  the ngrok cloud.
- Full test coverage of the protocol is achievable offline via `tokio::io::duplex`.

### Alternatives Considered

- **yamux**: different wire format; not compatible with the ngrok cloud service.
- **tokio-mux / h2**: h2 is HTTP/2 framing, not muxado framing; incompatible.
- **FFI to Go muxado**: extreme build complexity; not cross-compilable; rules out
  mobile/embedded targets.

---

## Decision 5: `thiserror` for Error Types; `#[non_exhaustive]` on All Enums

**Status:** Accepted

### Context

The Rust API guidelines recommend `std::error::Error` impls for library errors.
`thiserror` generates them ergonomically. Public error enums that may gain variants
over time must not force caller recompilation.

### Decision

All public error types use `#[derive(thiserror::Error)]`. All public enums (including
`Error`, `ProxyProtoVersion`, `EndpointKind`, `RpcMethod`) are annotated
`#[non_exhaustive]` so callers must include a wildcard `_` arm. The `muxado` crate's
`MuxadoError` and `ErrorCode` are **not** `#[non_exhaustive]` because they map to a
fixed wire protocol spec and are not expected to gain variants in semver-compatible
releases.

### Consequences

- Callers get stable, composable errors using `?` propagation.
- `ngrok::Error` exposes an `.code()` method for extracting `ERR_NGROK_NNN` strings
  without requiring string parsing.
- New error variants can be added in minor releases without breaking existing `match`
  expressions (callers must already handle `_`).

### Alternatives Considered

- **`anyhow`**: not appropriate for libraries; erases type information.
- **Boxed `dyn Error`**: loses error variant information at the boundary.

---

## Decision 6: `ngrok-testing` as a Separate Crate

**Status:** Accepted

### Context

Offline testing requires an in-process mock ngrok server. This could live in a `#[cfg(test)]`
module, a `testing` Cargo feature, or a separate crate.

### Decision

Ship `ngrok-testing` as a separate crate published to crates.io. It is a `[dev-dependency]`
for `ngrok` tests but a regular `[dependency]` for downstream users who want to write
tests for their own applications.

### Consequences

- Application code that uses ngrok can write offline tests by adding `ngrok-testing`
  as a `[dev-dependency]` — no production cost.
- `ngrok-testing` depends on `ngrok` (as a dev dependency, no circular dependency issue).
- The mock server tracks the ngrok protocol; breaking protocol changes require updating
  both crates.

### Alternatives Considered

- **`testing` feature flag on `ngrok`**: pollutes the main crate with test code; increases
  compile time of production builds.
- **`#[cfg(test)]` only**: downstream applications cannot use the helpers without copying
  the code.

---

## Decision 7: `ngrok-sync` as the Shared FFI Control-Plane Foundation; Adapter Crates Live Externally

**Status:** Accepted (updated — see also Decision 9 for the data plane)

### Context

The library must be callable from multiple language ecosystems with incompatible FFI
mechanisms: UniFFI (C-ABI, Kotlin/Swift/Python/C#), Rustler (Erlang NIF API, Elixir),
napi-rs (Node.js), PyO3 (Python), and others. No single FFI tool covers all targets,
and each has incompatible proc-macros, ABI entry points, and type systems — they cannot
coexist in a single crate.

Every FFI adapter faces the same core problem: calling `async` Rust from a synchronous
FFI boundary for connection management. Without a shared solution, each adapter
independently reinvents Tokio runtime ownership and `block_on` bridging.

### Decision

**`ngrok-sync`** is added to this workspace as a blocking/synchronous wrapper over
`ngrok` — but **only for the control plane**: connecting the agent, authenticating,
opening and closing listeners, managing endpoint lifecycle. These are low-frequency
operations where `block_on` is acceptable.

The **data plane** — reading and writing bytes on `NgrokStream` — is explicitly **not**
wrapped in `block_on`. `EndpointListener::accept()` returns `ngrok::NgrokStream` (the
async type), and `Agent::runtime()` exposes the owned `Arc<tokio::Runtime>` so FFI
adapters can `rt.spawn(...)` data-plane tasks on the shared runtime without additional
overhead. See Decision 9 for the full data-plane strategy.

`ngrok-sync` has no FFI dependencies — it is a plain Rust crate published to crates.io.
FFI adapter crates live in **their own external repositories**, not in this workspace.

```
This repo                    External repos (examples)
────────────────────         ─────────────────────────────────────────
ngrok          async API      ngrok-uniffi     control plane:
ngrok-sync     control-plane──┤                  #[uniffi::export] on ngrok-sync
               blocking API   │               data plane:
ngrok-testing  test helpers   │                  rt.spawn() + next_bytes() via ngrok
                               ngrok-rustler  control plane:
                               (inside Elixir   #[rustler::nif] on ngrok-sync
                                package repo)  data plane:
                                               rt.spawn(async { send_to_pid() })
```

### `ngrok-sync` control-plane API shape

```rust
// ngrok_sync::Agent
pub struct Agent { /* Arc<ngrok::Agent> + Arc<tokio::Runtime> */ }
impl Agent {
    pub fn builder() -> AgentBuilder;
    /// Expose the shared runtime so FFI adapters can spawn data-plane tasks.
    pub fn runtime(&self) -> Arc<tokio::runtime::Runtime>;
    pub fn connect(&self) -> Result<(), Error>;
    pub fn disconnect(&self) -> Result<(), Error>;
    pub fn session(&self) -> Option<AgentSession>;
    pub fn endpoints(&self) -> Vec<EndpointInfo>;
    pub fn listen(&self, opts: EndpointOptions) -> Result<EndpointListener, Error>;
    pub fn forward(&self, upstream: Upstream, opts: EndpointOptions) -> Result<EndpointForwarder, Error>;
}

// ngrok_sync::EndpointListener
// accept() returns the async ngrok::NgrokStream — the FFI adapter decides how to handle it.
pub struct EndpointListener { /* wraps ngrok::EndpointListener + Arc<Runtime> */ }
impl EndpointListener {
    pub fn accept(&self) -> Result<ngrok::NgrokStream, Error>;  // NOTE: async stream, not blocking
    pub fn url(&self) -> &Url;
    pub fn close(self) -> Result<(), Error>;
}
// No ngrok_sync::NgrokStream type — the data plane stays in ngrok's async world.
```

Non-async types (`EndpointOptions`, `Upstream`, `Event`, `Error`, `ProxyProtoVersion`,
`AgentSession`, `EndpointInfo`) are re-exported directly from `ngrok` — no wrapper needed.

### Consequences

- `ngrok` and `ngrok-sync` are FFI-free; zero overhead for pure Rust users.
- The control plane is solved once in `ngrok-sync`; FFI adapters add only proc-macro
  annotations.
- The data plane retains full async performance and zero-copy capability (see Decision 9).
- FFI adapters own the responsibility of bridging `ngrok::NgrokStream` to their host
  event loops — `runtime()` gives them the handle to do so without spawning a second runtime.
- `ngrok-sync` must expose a stable, non-breaking API because external crates depend on it.

### Alternatives Considered

- **Wrap `NgrokStream` in blocking `std::io::Read+Write` inside `ngrok-sync`**: Convenient
  for simple scripts, but blocks host language threads on every read, starving event-driven
  runtimes (BEAM schedulers, libuv, Netty). Also forces a mandatory buffer copy, eliminating
  the zero-copy path from muxado frames. Rejected — see Decision 9.
- **FFI adapters in this workspace**: couples unrelated toolchains; different teams need
  write access; bloats CI.
- **Each adapter wraps `ngrok` directly**: every adapter independently re-implements
  Tokio runtime ownership — duplicated logic, duplicated bugs.
- **Single `ngrok-ffi` crate for all targets**: UniFFI and Rustler are ABI-incompatible.

---

## Decision 8: Semaphore-Based Flow Control in muxado (Not Condvar)

**Status:** Accepted

### Context

The Go muxado uses a `sync.Cond`-based window manager (`condWindow`). Rust's `tokio`
offers `tokio::sync::Semaphore` as a native async primitive for producer/consumer
resource limits.

### Decision

Use `tokio::sync::Semaphore` for outbound flow-control windows. `stream.write()` acquires
`n` permits; receiving a `WNDINC` frame adds `n` permits via `Semaphore::add_permits`.
The semaphore starts with `initial_window_size` permits (default 256 KB).

For inbound buffering, use `tokio::sync::Notify` + `Mutex<VecDeque<Bytes>>` so
`AsyncRead::poll_read` can park efficiently.

### Consequences

- No custom condvar or thread-blocking: fully async, tokio-native.
- `Semaphore::add_permits` is the idiomatic tokio equivalent of `cond.Broadcast`.
- This means muxado streams never block the tokio thread pool; they yield properly.

### Alternatives Considered

- **`std::sync::Condvar`**: blocks the OS thread; incompatible with async tasks.
- **Custom channel-per-stream**: heavier allocation per stream; more complex shutdown.

---

## Decision 9: Split-Plane Data I/O — Async-Native, Zero-Copy Data Plane for FFI

**Status:** Accepted

### Context

All host language runtimes that will consume `ngrok-rust` via FFI are event-driven:
- **Elixir / Erlang BEAM**: uses a fixed pool of Dirty I/O schedulers; blocking a scheduler
  thread for even one connection's read loop exhausts the pool under production load.
- **Node.js / libuv**: single-threaded event loop; one blocking `read()` call stalls the
  entire process.
- **Java (Netty) / .NET (ASP.NET Core)**: use non-blocking selector/IOCP models; blocking
  a pool thread degrades the entire application.

An earlier design had `ngrok-sync` wrap `NgrokStream` in `std::io::Read + Write` via
`self.rt.block_on(self.inner.read(buf))` on every call. This creates two cascading problems:

1. **Thread exhaustion**: the host language's I/O thread physically blocks waiting for the
   Tokio future to resolve. One blocked thread per active connection collapses concurrency.

2. **Mandatory memory copy**: `std::io::Read::read(&mut self, buf: &mut [u8])` requires a
   caller-supplied slice. The muxado `DataFrame` payload (`bytes::Bytes`) — which may be
   a zero-copy arc into the TLS receive buffer — must be copied into that slice. For
   high-throughput traffic this is significant unnecessary CPU and memory pressure.

### Decision

**The data plane is async-native and zero-copy. `ngrok-sync` does not wrap stream I/O.**

#### `NgrokStream::next_bytes()` — zero-copy frame extraction

`ngrok::NgrokStream` gains a single additional method:

```rust
impl NgrokStream {
    /// Yield the next frame payload from the underlying muxado stream.
    ///
    /// Returns the raw `bytes::Bytes` arc directly — no copy into a caller buffer.
    /// Returns `None` when the remote closes the stream.
    pub async fn next_bytes(&mut self) -> Option<bytes::Bytes>;
}
```

The implementation reads the next muxado `DataFrame`, clones (arc-bumps) the payload
`Bytes`, and returns it. The caller decides when to drop the `Bytes` arc, not the
transport layer.

#### FFI adapter data-plane pattern

FFI adapters combine the `runtime()` handle exposed by `ngrok_sync::Agent` with
`NgrokStream::next_bytes()` to bridge the async Rust data plane into their host runtime:

```
// Pseudo-code — actual FFI proc-macros differ by target

// 1. Control plane (ngrok-sync, block_on, FFI-annotated):
let listener: ngrok_sync::EndpointListener = agent.listen(opts)?;
let rt: Arc<Runtime> = agent.runtime();

// 2. Accept (block_on — acceptable; happens once per connection):
let stream: ngrok::NgrokStream = listener.accept()?;

// 3. Data plane (spawn on shared runtime — no new runtime, no block_on):
rt.spawn(async move {
    while let Some(bytes) = stream.next_bytes().await {
        // Zero-copy: `bytes` is an arc into the muxado receive window.
        // Elixir:  send as ErlNifBinary message to the process PID.
        // Node.js: call napi callback with Buffer wrapping the Bytes pointer.
        // UniFFI:  write into pre-negotiated LLFIO shared buffer.
        host_runtime_send(bytes);
    }
    host_runtime_close();
});

// 4. Return immediately to the host scheduler — no thread blocked.
```

This pattern means:
- The Tokio runtime drives the data plane entirely within its own thread pool.
- The host language thread is never blocked waiting for network data.
- `bytes::Bytes` payloads can be wrapped in host-language memory types without copying,
  provided the host FFI supports raw pointer / length pairs (most do).

#### Write path

Writes from the host language go through the same `rt.spawn` pattern:

```rust
rt.spawn(async move {
    stream.write_all(&payload).await?;
    Ok::<_, ngrok::Error>(())
});
```

For unicast write-and-forget from a host language thread, a `tokio::sync::oneshot`
channel can surface the completion/error back to the calling thread without blocking.

### Consequences

- `ngrok-sync` has no `NgrokStream` type — the data plane is purely in `ngrok`.
- Each FFI adapter must implement its own host-event-loop bridge for the data plane;
  there is no universal "plug and play" solution, but the pattern is well-documented and
  short (~30 lines per adapter).
- `NgrokStream::next_bytes()` is an additional public API surface that must be kept
  stable across semver-compatible releases of `ngrok`.
- Host-language binaries that use `block_on` per read (e.g., simple CLI tools) are not
  broken — they can still call `rt.block_on(stream.next_bytes())` themselves.

### Alternatives Considered

- **`std::io::Read + Write` wrappers in `ngrok-sync`**: correct for the control plane,
  catastrophic for the data plane (see Context). Rejected.
- **`tokio::sync::mpsc::Receiver<Bytes>` per stream**: cleanly decouples send/receive but
  adds a channel allocation and an extra arc clone per frame; `next_bytes()` is lighter.
- **Expose `tokio::io::AsyncRead` directly to UniFFI**: UniFFI's generated bindings do not
  understand Tokio futures; this would require a custom async runtime bridge per language.
- **LLFIO / io_uring shared-memory ring buffer**: maximally zero-copy but requires kernel
  5.1+, is Linux-only, and is far more complex than the `bytes::Bytes` arc approach.
  Could be layered on top in a future version.

---

## Decision 10: Extract muxado Into Its Own Workspace Crate

**Status:** Accepted

### Context

Decision 4 established that muxado would be implemented in Rust as an internal module
(`ngrok::muxado`). While this was the simplest starting point, several factors now
motivate extracting it into a standalone crate within the Cargo workspace:

1. **Independent testability.** muxado is a self-contained stream-multiplexing protocol
   with its own wire format, flow control, and lifecycle rules. As an internal module of
   `ngrok`, its tests must be built with all of `ngrok`'s dependencies (rustls, serde,
   hyper, etc.) even though muxado has no inherent TLS or serialization requirements.
   A separate crate compiles and tests in isolation.

2. **Clear dependency boundary.** muxado has no logical dependency on ngrok-specific
   types (`Auth`, `Bind`, `ProxyHeader`, `EndpointOptions`, events, etc.). Drawing a
   crate boundary enforces this: the muxado crate cannot accidentally import ngrok
   application types, and ngrok cannot accidentally reach into muxado internals that
   should not be public.

3. **Reuse potential.** The muxado wire protocol is not ngrok-specific — it is a general
   stream multiplexer. A standalone crate allows future use cases (other protocols,
   testing tools, third-party integrations) without pulling in the entire ngrok SDK.

4. **Parallel compilation.** Cargo compiles workspace crates in parallel when they have
   no dependency between them. `muxado` has minimal dependencies (`tokio`, `bytes`,
   `thiserror`), so it can start compiling immediately while heavier ngrok dependencies
   (`rustls`, `serde`, `hyper`) resolve in parallel.

5. **Spec alignment.** The `specs/muxado/` specification suite already treats muxado as a
   standalone protocol with its own system spec, session/stream/protocol/heartbeat
   features, and configuration parameters. A dedicated crate mirrors this spec boundary.

### Decision

Extract the muxado implementation into a workspace crate named `muxado` with crate name
`muxado` (to avoid name collisions on crates.io). The crate lives at `muxado/` in
the workspace root.

```
ngrok-rust/                  ← Cargo workspace
├── muxado/                  ← Stream-multiplexing protocol crate (crates.io: muxado)
├── ngrok/                   ← Async SDK crate (crates.io: ngrok)
├── ngrok-sync/              ← Blocking wrapper crate (crates.io: ngrok-sync)
└── ngrok-testing/           ← Offline test helpers (in-process mock server)
```

The `ngrok` crate depends on `muxado` as a path dependency:

```toml
[dependencies]
muxado = { path = "../muxado" }
```

The `ngrok-testing` crate also depends on `muxado` directly (it needs to speak the
muxado server role for the mock server).

### Public API Surface of the `muxado` Crate

The crate exposes the following public types (see `muxado-api.md` for full signatures):

| Type | Role |
|------|------|
| `Session` | Reader task + writer task; open/accept streams |
| `SessionConfig` | Window size, accept backlog, frame queue depth |
| `Stream` | `AsyncRead + AsyncWrite` bidirectional byte channel |
| `StreamId` | Newtype around `u32` |
| `TypedStreamSession` | 4-byte type-prefix layer on top of `Session` |
| `TypedStream` | `Stream` + `.stream_type() -> u32` |
| `Heartbeat` | Keepalive + RTT measurement over `TypedStreamSession` |
| `HeartbeatConfig` | Interval, tolerance, stream type |
| `MuxadoError` | Error enum: `Io`, `Protocol`, `StreamClosed`, `GoAway`, etc. |
| `ErrorCode` | Wire error codes (`NoError`, `ProtocolError`, `FlowControlError`, …) |
| `frame` module | Public frame types for advanced use (frame-level testing, custom framers) |

Types that remain internal (not `pub`):
- `StreamMap` / `StreamInner` — implementation detail of `Session`
- `Window` / `Buffer` — internal stream state
- `ReaderTask` / `WriterTask` — spawned tasks

### Dependency Graph After Extraction

```
muxado                    tokio, bytes, thiserror
  ↑
ngrok                     muxado, tokio, rustls, serde, serde_json, ...
  ↑                 ↑
ngrok-sync          ngrok-testing    (also depends on muxado directly)
```

### Consequences

- The `ngrok` crate no longer has a `pub(crate) mod muxado`; it depends on the `muxado`
  crate like any other external dependency.
- All references in other design documents change from `ngrok::muxado::*` to `muxado::*`
  (or `muxado::*` as the Rust import name).
- The muxado crate has its own `Cargo.toml`, `README.md`, unit tests, property tests,
  and benchmarks.
- The implementation guide gains a dedicated "Milestone 1.5: muxado Crate Scaffold"
  step and the muxado milestones (frame layer, session, heartbeat) move into the
  muxado crate context.
- `ngrok-testing::MockNgrokServer` depends on `muxado` directly to speak the server
  role, rather than reaching into `ngrok::muxado` internals.
- Publishing order on crates.io: `muxado` first, then `ngrok`, then `ngrok-sync`
  and `ngrok-testing`.

### Alternatives Considered

- **Keep muxado as `pub(crate) mod`:** Simpler workspace, but loses all five benefits
  listed above. The module grows large (8+ source files, frame sub-module) and its
  tests pollute the `ngrok` crate's test binary.
- **Publish muxado as a fully independent repo/crate:** Maximises reuse but introduces
  cross-repo version coordination. A workspace crate gets the isolation benefits while
  keeping the version lockstep and single-PR workflow.
- **Use `[lib] path` tricks to share source between crates:** Fragile and confusing;
  standard workspace members are the idiomatic Rust solution.

---

## Open Questions / Future Decisions

These items are deferred and not part of the v1 scope. They should be revisited before
or during v2 planning.

### Open Question 1: Custom TCP Dialer

**Context:** `AgentBuilder::build()` always dials `connect.ngrok-agent.com:443` via
`tokio::net::TcpStream::connect`. Architecture.md notes: *"Not directly exposed on
`AgentBuilder` in v1; planned via `tower::Service` layer."*

**Why deferred:** A `tower::Service` integration requires deciding whether `ngrok`
takes a direct `tower` dependency (and at which version), or exposes a simpler
`FnMut(addr) -> Future<TcpStream>` callback. Neither option has been fully designed.

**Impact:** Users with strict egress routing requirements (corporate proxies, VPCs)
cannot override the dial path. The existing `proxy_url` builder method covers the
HTTP/SOCKS proxy case; this question is specifically about replacing the raw TCP
dial function.

**Resolution criteria before v2:** Agree on the API shape (tower vs callback), add to
`AgentBuilder`, document in api.md, and cover in a test using `MockNgrokServer`.

---

### Open Question 2: LLFIO / io_uring Zero-Copy Data Plane

**Context:** Decision 9 documents `NgrokStream::next_bytes()` as the zero-copy data
plane. An alternative noted there — LLFIO / io_uring shared-memory ring buffers —
is maximally zero-copy but Linux-only and requires kernel 5.1+.

**Why deferred:** The `bytes::Bytes` arc approach satisfies all current FFI targets.
The ring-buffer approach would require a platform-specific implementation path and
significant additional testing infrastructure.

**Resolution criteria before consideration:** Demonstrate a measurable throughput or
latency regression with `next_bytes()` in a real FFI target at production load.
Only then is the additional complexity justified.
