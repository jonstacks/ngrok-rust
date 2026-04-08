# Architecture — `ngrok-rust`

## Component Map

```mermaid
graph TD
    subgraph "Public API — crate ngrok"
        AB["AgentBuilder\n(builder pattern)"]
        AG["Agent\n(arc-shared state)"]
        AS["AgentSession\n(clone-on-read snapshot)"]
        EL["EndpointListener\n(async Stream of NgrokStream)"]
        EF["EndpointForwarder\n(auto-proxy task)"]
        UP["Upstream\n(value object)"]
        EO["EndpointOptions\n(builder)"]
        EV["Event enum\n(dispatched to handlers)"]
        RPC["RpcRequest / RpcResponse\n(server-sent commands)"]
        DX["Agent::diagnose\n(pre-auth probe)"]
        HY["feature: hyper\nhyper-util TokioIo bridge"]
        AX["feature: axum\nListener impl"]
    end

    subgraph "Tunnel Layer — ngrok/src/tunnel/"
        RS["RawSession\nmuxado + JSON RPC\nauthenticate + bind"]
        RC["ReconnectingSession\nbackoff loop, re-bind"]
        TP["proto module\nAuth/Bind/Proxy messages\n(serde_json)"]
    end

    subgraph "crate muxado (muxado/)"
        MX["Session\nreader task + writer task"]
        ST["Stream\nAsyncRead + AsyncWrite"]
        FR["frame module\nDATA / RST / WNDINC / GOAWAY"]
        HB["Heartbeat\nTypedStream 0xFFFFFFFF"]
        TY["TypedStreamSession\n4-byte type prefix"]
    end

    subgraph "TLS / Network"
        TLS["tokio-rustls\nClientConfig / ServerConfig"]
        NET["tokio::net::TcpStream\nunderlying transport"]
    end

    subgraph "crate ngrok-sync (control plane only)"
        BS["ngrok_sync::Agent\nowned tokio::Runtime\nblock_on wrappers\n+ runtime() accessor"]
        BL["ngrok_sync::EndpointListener\naccept() → ngrok::NgrokStream\n(no blocking I/O wrapper)"]
        BF["ngrok_sync::EndpointForwarder"]
    end

    subgraph "crate ngrok-testing"
        MS["MockNgrokServer\nin-process protocol server"]
        MP["MockEndpoint\naccepts / injects connections"]
    end

    subgraph "External FFI repos (not in this workspace)"
        UF["ngrok-uniffi\nKotlin / Swift / Python / C#\ncontrol: #[uniffi::export]\ndata: rt.spawn() + next_bytes()"]
        RF["ngrok-rustler\nElixir / Erlang NIFs\ncontrol: #[rustler::nif]\ndata: rt.spawn() → send_to_pid()"]
    end

    AB --> AG
    AG --> AS
    AG --> EL
    AG --> EF
    EF --> EL
    EL --> HY
    EL --> AX
    AG --> EV
    AG --> RPC
    AG --> DX
    AG --> RC
    RC --> RS
    RS --> TP
    RS --> TY
    TY --> MX
    MX --> ST
    MX --> FR
    MX --> HB
    MX --> NET
    NET --> TLS

    MS -.->|test double for| RC
    MS -.->|uses muxado server role| MX
    BS -->|block_on| AG
    BL -->|block_on accept| EL
    BF -->|wraps| EF
    UF -.->|depends on published| BS
    RF -.->|depends on published| BS
```

---

## Layers

| Layer | Crate/Module | Responsibility |
|-------|-------------|----------------|
| **Public API** | `ngrok::` root | `Agent`, `EndpointListener`, `EndpointForwarder`, events, options |
| **Integration** | `ngrok::integrations::hyper` / `axum` | Feature-gated trait impls |
| **Tunnel Client** | `ngrok::tunnel::reconnecting` | Reconnect loop, re-bind, backoff |
| **Raw Session** | `ngrok::tunnel::raw_session` | Auth/Bind/Proxy over typed muxado streams |
| **Wire Protocol** | `ngrok::proto` | `Auth`, `Bind`, `Proxy`, `StopTunnel` serde structs |
| **Muxado** | `muxado` (crate) | Full muxado protocol: frames, sessions, streams, heartbeat |
| **TLS / Transport** | `tokio-rustls` + `rustls` | TLS over TCP; `aws-lc-rs` feature for FIPS backend |
| **Blocking API** | `ngrok-sync` | **Control plane only**: owns `tokio::Runtime`; `block_on` wrappers for connect/listen/bind; exposes `runtime()` for data-plane task spawning |
| **Test Helpers** | `ngrok-testing` | In-process mock ngrok server; no authtoken needed |
| **FFI Adapters** | external repos | `ngrok-uniffi`, `ngrok-rustler`, etc. — control plane via `ngrok-sync`; data plane via `rt.spawn()` + `NgrokStream::next_bytes()` |

---

## Control Plane vs Data Plane

This is the most important architectural boundary in the FFI story. Every operation in
the library falls into exactly one plane:

| Plane | Examples | Frequency | `block_on` ok? | Mechanism |
|-------|----------|-----------|---------------|-----------|
| **Control** | connect, auth, listen, bind, close, config | Low (once per session/endpoint) | ✅ Yes | `ngrok-sync` wrappers |
| **Data** | read stream bytes, write stream bytes | High (every packet) | ❌ No — starves host runtime | `rt.spawn()` + `next_bytes()` |

### FFI Adapter Data-Plane Flow

```mermaid
sequenceDiagram
    participant HL as Host Language Thread<br/>(Elixir scheduler / libuv / etc.)
    participant NS as ngrok_sync::Agent
    participant RT as tokio::Runtime
    participant ST as ngrok::NgrokStream

    HL->>NS: agent.listen(opts)         [block_on — control plane ✅]
    NS-->>HL: EndpointListener

    HL->>NS: listener.accept()          [block_on — control plane ✅]
    NS-->>HL: ngrok::NgrokStream

    HL->>RT: rt.spawn(async move {      [returns immediately ✅]
        loop {
            bytes = stream.next_bytes().await
            host_send(bytes)            [zero-copy arc bump]
        }
    })
    RT-->>HL: JoinHandle (dropped)

    Note over HL: Host thread unblocked immediately
    Note over RT,ST: Tokio drives data plane independently
    RT->>ST: next_bytes().await
    ST-->>RT: Some(bytes::Bytes)
    RT->>HL: host_send(bytes)           [async message / callback]
```

The key insight: `listener.accept()` is a one-time `block_on` (acceptable — it waits for
a connection to arrive, analogous to `TcpListener::accept()`). The stream I/O loop is
entirely within the Tokio runtime; the host thread is never blocked on network data.

### `NgrokStream::next_bytes()` — Zero-Copy Frame Extraction

The muxado layer internally buffers incoming `DataFrame` payloads as `bytes::Bytes` arcs
pointing into the TLS receive window. `next_bytes()` clones (arc-bumps) this `Bytes`
handle and returns it, allowing FFI adapters to wrap the memory in the host language's
native byte type without a deep copy:

```
muxado recv buffer   →  bytes::Bytes (arc)  →  ErlNifBinary (Elixir, zero-copy if pinned)
                     →  bytes::Bytes (arc)  →  napi Buffer  (Node.js, zero-copy)
                     →  bytes::Bytes (arc)  →  Span<byte>   (C#, unsafe pin)
```

---

## Key Data Flows

### Listen Mode: Full Connection Path

```mermaid
sequenceDiagram
    participant App as Application Code
    participant AG as Agent
    participant RC as ReconnectingSession
    participant RS as RawSession
    participant MX as muxado::Session
    participant CLD as ngrok Cloud

    App->>AG: agent.listen(opts).await
    AG->>RC: ensure_connected()
    RC->>CLD: TCP connect :443
    CLD-->>RC: TCP OK
    RC->>CLD: TLS handshake (rustls, ngrok CA)
    CLD-->>RC: TLS OK
    RC->>MX: muxado::Session::client(tls_stream)
    RC->>RS: raw_session.auth(AuthExtra).await
    RS->>MX: open TypedStream(AuthReq=0)
    MX->>CLD: DATA[SYN, type=0, json body]
    CLD-->>MX: DATA[AuthResp json]
    RS-->>RC: SessionID, cookie
    RC->>RS: raw_session.bind(tunnel_config).await
    RS->>MX: open TypedStream(BindReq=1)
    MX->>CLD: DATA[SYN, type=1, json body]
    CLD-->>MX: DATA[BindResp{url}]
    RS-->>AG: TunnelHandle{url, accept_rx}
    AG-->>App: EndpointListener{url}

    loop Per inbound request
        CLD->>MX: DATA[SYN, ProxyReq type=3]
        MX->>RS: dispatch ProxyReq
        RS->>RS: route stream → TunnelHandle.accept_tx
        AG-->>App: NgrokStream via listener.accept()
    end
```

### Forward Mode: Proxy Loop

```mermaid
sequenceDiagram
    participant App as Application Code
    participant AG as Agent
    participant FW as ForwardLoop task
    participant UP as Upstream (localhost:8080)
    participant CLD as ngrok Cloud

    App->>AG: agent.forward(upstream, opts).await
    AG->>AG: listen() internally
    AG->>FW: spawn forward_loop(listener, upstream)
    AG-->>App: EndpointForwarder{url}

    loop Per connection
        CLD->>FW: NgrokStream via accept()
        FW->>UP: tokio::net::TcpStream::connect(upstream_addr)
        FW->>FW: tokio::io::copy_bidirectional(stream, backend)
        FW->>AG: emit ConnectionClosed event
    end
```

### Reconnection Flow

```mermaid
flowchart TD
    SessionDropped[muxado session error] --> IsClosed{agent\nclosed?}
    IsClosed -->|Yes| PermanentClose([stop loop])
    IsClosed -->|No| EmitDisconnect[emit AgentDisconnected event]
    EmitDisconnect --> Backoff[tokio::time::sleep\nmin=500ms, max=30s, ×2]
    Backoff --> Dial[TCP + TLS dial]
    Dial --> DialOk{success?}
    DialOk -->|No| Backoff
    DialOk -->|Yes| Auth[Auth with stored cookie]
    Auth --> AuthOk{success?}
    AuthOk -->|No| Backoff
    AuthOk -->|Yes| Rebind[Rebind all tunnel configs]
    Rebind --> BindOk{all OK?}
    BindOk -->|No| Backoff
    BindOk -->|Yes| EmitConnect[emit AgentConnectSucceeded]
    EmitConnect --> Running([session active])
```

---

## Module Structure

```
muxado/src/                            ← crate: muxado
├── lib.rs                     ← re-exports all public types: Session, SessionConfig,
│                                 SessionTermination, Stream, StreamId, TypedStreamSession,
│                                 TypedStream, Heartbeat, HeartbeatConfig, MuxadoError,
│                                 ErrorCode, frame module
├── session.rs                 ← Session: reader task + writer task + stream_map
├── session_config.rs          ← SessionConfig: window size, accept backlog, queue depth
├── stream.rs                  ← Stream: AsyncRead + AsyncWrite
├── stream_id.rs               ← StreamId newtype, parity helpers
├── stream_map.rs              ← StreamId → StreamInner registry (internal)
├── window.rs                  ← outbound flow-control window (Semaphore-based, internal)
├── buffer.rs                  ← inbound receive buffer (VecDeque + Notify, internal)
├── typed.rs                   ← TypedStreamSession, TypedStream
├── heartbeat.rs               ← Heartbeat, HeartbeatConfig
├── error.rs                   ← MuxadoError enum, ErrorCode enum
└── frame/
    ├── mod.rs                 ← Frame enum, read_frame / write_frame, re-exports
    ├── header.rs              ← 8-byte header: length (24b) | type (4b) | flags (4b) | stream_id (32b)
    ├── data.rs                ← DataFrame (SYN / FIN flags)
    ├── rst.rs                 ← RstFrame (ErrorCode u32)
    ├── wndinc.rs              ← WndIncFrame (window increment u32)
    └── goaway.rs              ← GoAwayFrame (last_stream_id + error_code + debug)

ngrok/src/                             ← crate: ngrok
├── lib.rs                     ← re-exports, package-level listen/forward helpers
├── agent.rs                   ← AgentBuilder, Agent, AgentState (Arc<Mutex<>>)
├── session.rs                 ← AgentSession
├── endpoint.rs                ← EndpointInfo, EndpointKind, shared metadata
├── listener.rs                ← EndpointListener, NgrokStream, NgrokAddr
├── forwarder.rs               ← EndpointForwarder, forward_loop task
├── upstream.rs                ← Upstream value object, ProxyProtoVersion enum
├── options.rs                 ← EndpointOptions, EndpointOptionsBuilder
├── events.rs                  ← Event enum, all event structs
├── error.rs                   ← Error, DiagnoseError (thiserror)
├── rpc.rs                     ← RpcRequest, RpcMethod, RpcResponse
├── diagnose.rs                ← Agent::diagnose, DiagnoseResult
├── defaults.rs                ← package-level listen/forward using env authtoken
├── proto/
│   ├── mod.rs
│   └── msg.rs                 ← Auth, AuthExtra, BindReq, BindResp, ProxyHeader,
│                                 StopTunnel, SrvInfoResp  (serde::Deserialize/Serialize)
├── tunnel/
│   ├── mod.rs
│   ├── raw_session.rs         ← RawSession: Auth, Bind, accept proxy streams
│   │                            (uses muxado::TypedStreamSession + Heartbeat)
│   └── reconnecting.rs        ← ReconnectingSession: dial loop + backoff + rebind
└── integrations/
    ├── mod.rs
    ├── hyper.rs               ← feature = "hyper": TokioIo bridge usage note
    └── axum.rs                ← feature = "axum": axum::serve::Listener impl

ngrok-testing/src/                     ← crate: ngrok-testing
├── lib.rs                     ← re-exports
├── server.rs                  ← MockNgrokServer (in-process, tokio::io::duplex)
│                                (uses muxado::Session::server() directly)
├── endpoint.rs                ← MockEndpoint (inject / accept test connections)
└── fixtures.rs                ← test certificates, sample traffic policies

ngrok-sync/src/                        ← crate: ngrok-sync
├── lib.rs                     ← re-exports; package-level blocking listen/forward
├── agent.rs                   ← Agent (blocking); owns tokio::Runtime; exposes runtime()
├── listener.rs                ← EndpointListener (blocking); accept() → ngrok::NgrokStream
└── forwarder.rs               ← EndpointForwarder (blocking)
# No stream.rs — NgrokStream is ngrok::NgrokStream; data plane is FFI adapter's responsibility

# FFI adapter crates live in their own external repositories:
#
#   ngrok-uniffi/   (e.g. github.com/ngrok/ngrok-uniffi)
#   ├── src/lib.rs  ← #[uniffi::export] on ngrok-sync control plane
#   │               ← rt.spawn() + next_bytes() for data plane
#   └── ngrok.udl   ← UniFFI definition file
#
#   ngrok-rustler/  (lives inside the Elixir package, e.g. native/ngrok_rustler/)
#   └── src/lib.rs  ← #[rustler::nif] on ngrok-sync control plane
#                   ← rt.spawn() + send_to_pid() for data plane
```

---

## Concurrency & State

| Component | Ownership / Sync Primitive | Notes |
|-----------|--------------------------|-------|
| `Agent` | `Arc<AgentInner>` | Cheaply cloneable handle |
| `AgentInner.state` | `tokio::sync::RwLock<AgentState>` | State machine: Created / Connecting / Connected / Disconnecting |
| `AgentInner.endpoints` | `tokio::sync::RwLock<Vec<EndpointHandle>>` | Snapshot-copy on `endpoints()` |
| `AgentInner.event_handlers` | `Vec<Box<dyn Fn(Event)+Send+Sync>>` | Written once at build time; read-only during operation |
| `ReconnectingSession` | `Arc<ReconnectInner>` | Shared between Agent and reconnect task |
| `muxado::Session` | `Arc<SessionInner>` | Reader task + writer task both hold Arc |
| `muxado::StreamMap` | `std::sync::RwLock<HashMap<StreamId, StreamInner>>` | Fine-grained: locked only for insert/remove |
| `muxado::Stream.inbound` | `Mutex<VecDeque<Bytes>> + Notify` | `AsyncRead` wakes blocked readers |
| `muxado::Stream.window` | `tokio::sync::Semaphore` | `AsyncWrite` acquires permits; `WNDINC` adds them |
| `EndpointListener.accept_rx` | `tokio::sync::mpsc::Receiver<NgrokStream>` | One receiver per endpoint |

Thread safety: all public types are `Send + Sync`. The library is safe to use from
multiple tokio tasks simultaneously.

---

## Extension Points

| Extension | How |
|-----------|-----|
| Custom TLS backend | Pass `rustls::ClientConfig` to `AgentBuilder::tls_config`; enable `aws-lc-rs` feature for FIPS |
| Custom TCP dialer | Not directly exposed on `AgentBuilder` in v1; planned via `tower::Service` layer |
| Upstream TLS | `Upstream::tls_config(rustls::ClientConfig)` |
| Event monitoring | `AgentBuilder::on_event(handler)` |
| RPC handling | `AgentBuilder::on_rpc(handler)` |
| Test transport | `ngrok_testing::MockNgrokServer` — replaces the real cloud endpoint; uses `muxado::Session::server()` directly |
| Custom muxado framing | Import `muxado::frame` module for frame-level access (advanced) |
| Cross-language control plane (UniFFI) | Add `ngrok-uniffi` in its own repo; annotate `ngrok-sync` types with `#[uniffi::export]` |
| Cross-language control plane (Rustler) | Add Rust NIF code inside the Elixir package; annotate `ngrok-sync` types with `#[rustler::nif]` |
| Cross-language data plane (any target) | Call `agent.runtime()` to get `Arc<Runtime>`; `rt.spawn(async { stream.next_bytes().await })` per connection |
| Any other FFI target | Create a new adapter crate; depend on `ngrok-sync` for control plane; use `next_bytes()` for data plane |
