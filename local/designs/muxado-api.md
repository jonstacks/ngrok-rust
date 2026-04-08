# Public API Reference — `muxado` Crate

> `muxado` is a standalone Rust implementation of the muxado stream-multiplexing
> protocol. It maps many independent full-duplex byte streams over a single
> `AsyncRead + AsyncWrite` transport. Wire-compatible with the Go reference
> implementation (`golang.ngrok.com/muxado/v2`).

See `specs/muxado/` for the formal specification (Mermaid diagrams, Gherkin scenarios).
See `architecture.md` for how the `ngrok` crate uses this crate internally.

---

## Crate Metadata

```toml
[package]
name = "muxado"
version = "0.1.0"
edition = "2021"
description = "Stream-multiplexing protocol (muxado) for Rust — wire-compatible with golang.ngrok.com/muxado/v2"

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

---

## Module Layout

```
muxado/src/
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
```

---

## Types

### `Session`

The core multiplexer. Wraps a single transport and manages many concurrent streams.

```rust
pub struct Session { /* Arc<SessionInner> */ }

impl Session {
    /// Create a client-role session. Client stream IDs are odd: 3, 5, 7, …
    pub fn client<T>(transport: T, config: SessionConfig) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static;

    /// Create a server-role session. Server stream IDs are even: 2, 4, 6, …
    pub fn server<T>(transport: T, config: SessionConfig) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static;

    /// Open a new locally-initiated stream. The stream ID is allocated automatically.
    /// Returns `Err(MuxadoError::RemoteGoneAway)` if the remote has sent GOAWAY.
    /// Returns `Err(MuxadoError::StreamsExhausted)` if the ID space is full.
    pub async fn open_stream(&self) -> Result<Stream, MuxadoError>;

    /// Accept the next remotely-initiated stream. Blocks until a SYN arrives.
    /// Returns `Err(MuxadoError::SessionClosed)` when the session is shut down.
    pub async fn accept_stream(&self) -> Result<Stream, MuxadoError>;

    /// Send GOAWAY and close all streams. The transport is closed after all
    /// streams have drained.
    pub async fn close(&self) -> Result<(), MuxadoError>;

    /// Wait for the session to terminate (GOAWAY received, transport error, or
    /// explicit close). Returns the termination reason.
    pub async fn wait(&self) -> SessionTermination;

    /// The local network address of the underlying transport, if available.
    pub fn local_addr(&self) -> Option<std::net::SocketAddr>;

    /// The remote network address of the underlying transport, if available.
    pub fn remote_addr(&self) -> Option<std::net::SocketAddr>;
}

impl Clone for Session { /* Arc clone */ }
```

### `SessionConfig`

```rust
pub struct SessionConfig {
    /// Maximum per-stream receive window size in bytes. Default: 262144 (256 KB).
    pub max_window_size: u32,

    /// Maximum number of unaccepted inbound streams. Default: 128.
    pub accept_backlog: usize,

    /// Internal write frame queue depth. Default: 64.
    pub write_queue_depth: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_window_size: 262_144,
            accept_backlog: 128,
            write_queue_depth: 64,
        }
    }
}
```

### `SessionTermination`

```rust
#[derive(Debug)]
pub struct SessionTermination {
    /// The local reason the session ended.
    pub local_error: Option<MuxadoError>,

    /// The ErrorCode from the remote's GOAWAY frame, if one was received.
    pub remote_error: Option<ErrorCode>,

    /// Debug data from the remote's GOAWAY frame, if present (capped at 1 MB).
    pub remote_debug: Option<bytes::Bytes>,
}
```

---

### `Stream`

A single multiplexed stream. Implements `tokio::io::AsyncRead + AsyncWrite`.

```rust
pub struct Stream { /* opaque */ }

impl tokio::io::AsyncRead for Stream { ... }
impl tokio::io::AsyncWrite for Stream { ... }

impl Stream {
    /// The stream's unique identifier.
    pub fn id(&self) -> StreamId;

    /// Half-close the write side (sends a FIN DATA frame). Subsequent writes
    /// return an error. Reads remain open until the remote also closes.
    /// Idempotent: calling on an already half-closed stream is a no-op.
    pub async fn close_write(&mut self) -> Result<(), MuxadoError>;

    /// Reset the stream immediately (sends RST frame with the given error code).
    /// Both read and write halves are closed. The stream remains in the session
    /// registry for 5 seconds to absorb late in-flight frames.
    pub async fn reset(&mut self, code: ErrorCode) -> Result<(), MuxadoError>;

    /// Yield the next inbound DATA frame payload as a zero-copy `bytes::Bytes`.
    ///
    /// Unlike `AsyncRead::poll_read`, this does not copy into a caller-supplied
    /// buffer. The returned `Bytes` shares the transport's receive allocation.
    ///
    /// Returns `None` when the remote has closed the stream (FIN or RST received).
    pub async fn next_frame_payload(&mut self) -> Option<bytes::Bytes>;

    /// Set a read deadline. After the deadline, `poll_read` and `next_frame_payload`
    /// return a timeout error.
    pub fn set_read_deadline(&mut self, deadline: Option<tokio::time::Instant>);

    /// Set a write deadline. After the deadline, `poll_write` returns a timeout error.
    pub fn set_write_deadline(&mut self, deadline: Option<tokio::time::Instant>);

    /// The parent session.
    pub fn session(&self) -> &Session;
}
```

---

### `StreamId`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamId(u32);

impl StreamId {
    pub fn new(id: u32) -> Self;
    pub fn value(&self) -> u32;

    /// Returns `true` if this is a client-initiated (odd) stream.
    pub fn is_client(&self) -> bool;

    /// Returns `true` if this is a server-initiated (even) stream.
    pub fn is_server(&self) -> bool;
}

impl std::fmt::Display for StreamId { ... }
```

---

### `TypedStreamSession`

Wraps a `Session` to add a 4-byte big-endian type prefix to every stream. The
type prefix is written/read transparently — callers never see it.

```rust
pub struct TypedStreamSession { /* opaque */ }

impl TypedStreamSession {
    /// Wrap an existing session.
    pub fn new(session: Session) -> Self;

    /// Open a new typed stream with the given type identifier.
    /// The first 4 bytes written to the underlying muxado stream are the
    /// big-endian encoding of `stream_type`.
    pub async fn open_typed_stream(
        &self,
        stream_type: u32,
    ) -> Result<TypedStream, MuxadoError>;

    /// Accept the next remotely-initiated typed stream. Reads the first 4 bytes
    /// to determine the type, then returns the `TypedStream`.
    pub async fn accept_typed_stream(&self) -> Result<TypedStream, MuxadoError>;

    /// Close the underlying session (sends GOAWAY).
    pub async fn close(&self) -> Result<(), MuxadoError>;

    /// Access the underlying session.
    pub fn session(&self) -> &Session;
}
```

### `TypedStream`

```rust
pub struct TypedStream { /* opaque */ }

impl tokio::io::AsyncRead for TypedStream { ... }
impl tokio::io::AsyncWrite for TypedStream { ... }

impl TypedStream {
    /// The 4-byte type identifier for this stream.
    pub fn stream_type(&self) -> u32;

    /// The underlying stream ID.
    pub fn id(&self) -> StreamId;

    /// Half-close the write side. See `Stream::close_write`.
    pub async fn close_write(&mut self) -> Result<(), MuxadoError>;

    /// Yield the next inbound frame payload. See `Stream::next_frame_payload`.
    pub async fn next_frame_payload(&mut self) -> Option<bytes::Bytes>;
}
```

---

### `Heartbeat`

Wraps a `TypedStreamSession` to provide keepalive and latency measurement.
Uses a reserved stream type (`0xFFFF_FFFF` by default) for heartbeat traffic.
Non-heartbeat typed streams are passed through to the application.

```rust
pub struct Heartbeat { /* opaque */ }

impl Heartbeat {
    /// Create a heartbeat wrapper. Does not start heartbeat tasks until `start()`.
    pub fn new(
        session: TypedStreamSession,
        config: HeartbeatConfig,
    ) -> Self;

    /// Start the heartbeat requester and responder tasks. The `callback` is
    /// invoked on each heartbeat result:
    /// - `callback(latency, false)` on a successful heartbeat with the measured RTT.
    /// - `callback(Duration::ZERO, true)` on a timeout (no response within tolerance).
    pub fn start(
        &self,
        callback: impl Fn(std::time::Duration, bool) + Send + Sync + 'static,
    );

    /// Trigger an immediate out-of-band heartbeat. Returns `(latency, success)`.
    pub async fn beat(&self) -> (std::time::Duration, bool);

    /// Update the heartbeat interval atomically. Takes effect on the next tick.
    pub fn set_interval(&self, interval: std::time::Duration);

    /// Update the tolerance atomically. Takes effect on the next check.
    pub fn set_tolerance(&self, tolerance: std::time::Duration);

    /// Open a non-heartbeat typed stream (delegates to inner `TypedStreamSession`).
    pub async fn open_typed_stream(
        &self,
        stream_type: u32,
    ) -> Result<TypedStream, MuxadoError>;

    /// Accept the next non-heartbeat typed stream. Heartbeat streams (type
    /// `0xFFFF_FFFF`) are handled internally and never returned to the caller.
    pub async fn accept_typed_stream(&self) -> Result<TypedStream, MuxadoError>;

    /// Close the heartbeat tasks and the underlying session.
    pub async fn close(&self) -> Result<(), MuxadoError>;
}
```

### `HeartbeatConfig`

```rust
pub struct HeartbeatConfig {
    /// Time between heartbeat probes. Default: 10 seconds.
    pub interval: std::time::Duration,

    /// Maximum time to wait for a heartbeat response before declaring timeout.
    /// Default: 15 seconds.
    pub tolerance: std::time::Duration,

    /// The typed-stream type used for heartbeat traffic.
    /// Default: `0xFFFF_FFFF`.
    pub stream_type: u32,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: std::time::Duration::from_secs(10),
            tolerance: std::time::Duration::from_secs(15),
            stream_type: 0xFFFF_FFFF,
        }
    }
}
```

---

## Error Types

### `MuxadoError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum MuxadoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("stream closed: {0}")]
    StreamClosed(ErrorCode),

    #[error("remote sent GOAWAY: {0}")]
    RemoteGoneAway(ErrorCode),

    #[error("session closed")]
    SessionClosed,

    #[error("stream ID space exhausted")]
    StreamsExhausted,

    #[error("flow control violation on stream {stream_id}")]
    FlowControl { stream_id: StreamId },

    #[error("frame size error: expected {expected} bytes, got {actual}")]
    FrameSize { expected: usize, actual: usize },

    #[error("write deadline exceeded")]
    WriteTimeout,

    #[error("read deadline exceeded")]
    ReadTimeout,

    #[error("accept backlog full")]
    AcceptQueueFull,
}
```

### `ErrorCode`

Wire-level error codes sent in RST and GOAWAY frames.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ErrorCode {
    NoError = 0,
    ProtocolError = 1,
    InternalError = 2,
    FlowControlError = 3,
    StreamClosed = 4,
    StreamRefused = 5,
    StreamCancelled = 6,
    StreamReset = 7,
    FrameSizeError = 8,
    AcceptQueueFull = 9,
    EnhanceYourCalm = 10,
    RemoteGoneAway = 11,
    StreamsExhausted = 12,
    WriteTimeout = 13,
    SessionClosed = 14,
    PeerEOF = 15,
}

impl ErrorCode {
    pub fn from_u32(v: u32) -> Self;
}

impl std::fmt::Display for ErrorCode { ... }
```

---

## Frame Module (`muxado::frame`)

The frame module is public to support advanced use cases (frame-level testing, custom
framers, protocol analysis tools). Normal users interact only with `Session` and
`Stream`.

### `Frame`

```rust
#[derive(Debug, Clone)]
pub enum Frame {
    Data(DataFrame),
    Rst(RstFrame),
    WndInc(WndIncFrame),
    GoAway(GoAwayFrame),
    Unknown { frame_type: u8, stream_id: u32, payload: bytes::Bytes },
}
```

### `DataFrame`

```rust
#[derive(Debug, Clone)]
pub struct DataFrame {
    pub stream_id: u32,
    pub flags: FrameFlags,
    pub payload: bytes::Bytes,
}
```

### `RstFrame`

```rust
#[derive(Debug, Clone)]
pub struct RstFrame {
    pub stream_id: u32,
    pub error_code: ErrorCode,
}
```

### `WndIncFrame`

```rust
#[derive(Debug, Clone)]
pub struct WndIncFrame {
    pub stream_id: u32,
    pub increment: u32,
}
```

### `GoAwayFrame`

```rust
#[derive(Debug, Clone)]
pub struct GoAwayFrame {
    pub last_stream_id: u32,
    pub error_code: ErrorCode,
    pub debug_data: bytes::Bytes,
}
```

### `FrameFlags`

```rust
bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameFlags: u8 {
        const FIN = 0x1;
        const SYN = 0x2;
    }
}
```

### Frame I/O Functions

```rust
/// Read a single frame from the transport. Returns `Err` on I/O error or
/// malformed frame.
pub async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<Frame, MuxadoError>;

/// Write a single frame to the transport.
pub async fn write_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    frame: &Frame,
) -> Result<(), MuxadoError>;
```

---

## Thread Safety

All public types (`Session`, `Stream`, `TypedStreamSession`, `TypedStream`,
`Heartbeat`) are `Send + Sync`. They can be shared across tokio tasks and used
from multiple tasks simultaneously.

---

## Relationship to `ngrok` Crate

The `ngrok` crate depends on `muxado` and uses it through the tunnel layer:

```
ngrok::tunnel::RawSession  → TypedStreamSession (for Auth/Bind/Proxy typed streams)
ngrok::tunnel::RawSession  → Heartbeat          (for keepalive over the control session)
ngrok::NgrokStream         → muxado::Stream      (each proxy connection is a muxado stream)
ngrok_testing::MockServer  → Session::server()   (mock server speaks muxado server role)
```

`NgrokStream::next_bytes()` delegates to `muxado::Stream::next_frame_payload()` for
zero-copy frame extraction.

The `ngrok` crate does **not** re-export muxado types in its public API. Muxado is an
implementation detail of the transport layer. The only user-visible reference is the
`Error::Muxado(String)` variant which wraps `MuxadoError` as a string.
