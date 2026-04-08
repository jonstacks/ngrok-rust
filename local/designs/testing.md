# Testing Strategy — `ngrok-rust`

## Philosophy

The test suite must provide high confidence that:

1. The **muxado wire protocol** is correctly encoded and decoded.
2. The **ngrok application protocol** (Auth, Bind, Proxy, Heartbeat) correctly maps to
   the wire.
3. The **Agent state machine** transitions correctly through connect / listen /
   disconnect / reconnect.
4. Integration with **hyper** and **axum** is correct and compiles under all
   feature combinations.
5. The library is testable **without a real ngrok authtoken or network connection**
   for all non-integration scenarios.

---

## Coverage Targets

| Module | Target |
|--------|--------|
| `muxado::frame` (frame encoding/decoding) | ≥ 90% |
| `muxado::session` / `stream` / `stream_map` | ≥ 90% |
| `muxado::typed` / `heartbeat` | ≥ 85% |
| `ngrok::proto` (message structs, serialization) | ≥ 90% |
| `ngrok::tunnel::raw_session` | ≥ 85% |
| `ngrok::tunnel::reconnecting` | ≥ 80% |
| `ngrok::agent` (state machine) | ≥ 80% |
| `ngrok::listener` / `ngrok::forwarder` | ≥ 80% |
| `ngrok::events` | ≥ 85% |
| `ngrok::error` | ≥ 95% |
| `ngrok-testing` | ≥ 85% |
| Feature-gated integrations | ≥ 75% |

---

## Test Layers

### Layer 1: Unit Tests (offline, no tokio I/O)

Located in `#[cfg(test)]` modules within each source file or sibling `_test.rs`
files. These tests are pure in-memory: no TCP, no TLS, no network.

**What belongs here:**
- Frame encoding / decoding (`muxado::frame`) — in the `muxado` crate
- Protocol message serialization (`ngrok::proto::msg`) — in the `ngrok` crate
- Error formatting and `Error::code()` extraction
- `AgentBuilder` validation (missing authtoken, invalid URL schemes)
- URL scheme detection logic
- `EndpointOptionsBuilder` defaults
- `MuxadoError` and `ErrorCode` conversions — in the `muxado` crate

### Layer 2: In-Process Integration Tests (offline, real tokio tasks)

Located in `tests/` in each crate. Use `tokio::io::duplex` or the
`ngrok-testing::MockNgrokServer` to wire Agent ↔ mock server entirely in-process.
No real network. No authtoken.

**What belongs here:**
- Full agent connect → listen → accept → close flow
- Session reconnection with simulated transport drops
- Event dispatch: all 6 event types emitted in correct order
- RPC handler invocation (StopAgent, RestartAgent, UpdateAgent)
- Forward mode with a local echo TCP server
- Heartbeat timeout triggering reconnect
- `Diagnose` with mock TLS/muxado server
- hyper / axum integration (with mock listener)

### Layer 3: Online Integration Tests (require `NGROK_AUTHTOKEN`)

Located in `tests/online/`. Gated by `#[cfg_attr(not(feature = "online-tests"), ignore)]`
or a runtime env check.

**What belongs here:**
- Full cloud connect + real endpoint creation
- HTTP traffic round-trip via real ngrok URL
- TCP endpoint with real traffic

### Layer 4: Property Tests

Use `proptest` for (in the `muxado` crate):
- Frame encoding: any valid frame round-trips through encode → decode
- Stream ID parity: client IDs are always odd, server IDs always even
- Flow control: window never goes negative

### Layer 5: Benchmarks

Located in `benches/` in the `muxado` and `ngrok` crates respectively. Use `criterion`.

- `muxado_throughput`: bidirectional copy over a duplex muxado session (in `muxado` crate)
- `frame_encode_decode`: 10 million frame round-trips (in `muxado` crate)
- `agent_connect_listen`: time from `Agent::builder()` to `EndpointListener` with mock server (in `ngrok` crate)

---

## Test Helpers — `ngrok-testing` crate

The `ngrok-testing` crate is the key enabler of offline testing. It implements a
full in-process ngrok server speaking the muxado + ngrok JSON protocol. It depends
directly on the `muxado` crate to speak the server role.

### `MockNgrokServer`

```rust
pub struct MockNgrokServer { /* opaque */ }

impl MockNgrokServer {
    /// Start the mock server. Returns the connect URL to pass to AgentBuilder.
    pub async fn start() -> (Self, String);

    /// Returns the next endpoint that the test agent has bound.
    /// Blocks until a Bind request arrives.
    pub async fn accept_endpoint(&self) -> MockEndpoint;

    /// Inject a raw connection into the named endpoint.
    pub async fn inject(&self, endpoint_id: &str, stream: impl AsyncRead + AsyncWrite + Send + 'static);

    /// Simulate a transport drop (tests reconnect logic).
    pub fn drop_session(&self);

    /// Send a server-initiated RPC command.
    pub async fn send_rpc(&self, method: &str);
}
```

### `MockEndpoint`

```rust
pub struct MockEndpoint {
    pub id: String,
    pub url: String,
    pub proto: String,
}

impl MockEndpoint {
    /// Inject a test connection into this endpoint.
    pub async fn inject_connection(
        &self,
        data: impl AsyncRead + AsyncWrite + Send + 'static,
    );
}
```

### `TestCertificate`

```rust
pub struct TestCertificate {
    pub server_config: rustls::ServerConfig,
    pub client_config: rustls::ClientConfig,
    pub der: CertificateDer<'static>,
}

impl TestCertificate {
    /// Generate a self-signed certificate for testing agent-side TLS termination.
    pub fn generate() -> Self;
}
```

### `AgentTestExt`

Extension methods on `AgentBuilder` for tests:

```rust
pub trait AgentTestExt: Sized {
    /// Connect to a `MockNgrokServer` instead of real ngrok cloud.
    fn with_mock_server(self, connect_url: &str) -> Self;

    /// Skip TLS verification (for in-process test servers using self-signed certs).
    fn danger_accept_any_cert(self) -> Self;
}

impl AgentTestExt for AgentBuilder { ... }
```

---

## Example Test Cases

### Test 1: Frame Encode / Decode Round-Trip

```rust
// In muxado/src/frame/ tests
#[test]
fn data_frame_syn_fin_round_trip() {
    use muxado::frame::{DataFrame, FrameFlags};
    let frame = DataFrame {
        stream_id: 3,
        flags: FrameFlags::SYN | FrameFlags::FIN,
        payload: Bytes::from_static(b"hello"),
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let decoded = DataFrame::decode(&mut buf).unwrap();
    assert_eq!(decoded.stream_id, 3);
    assert!(decoded.flags.contains(FrameFlags::SYN));
    assert!(decoded.flags.contains(FrameFlags::FIN));
    assert_eq!(decoded.payload, b"hello"[..]);
}
```

### Test 2: Auth Message Serialization

```rust
#[test]
fn auth_extra_serializes_correctly() {
    use ngrok::proto::msg::{Auth, AuthExtra};
    let auth = Auth {
        version: vec!["3".into(), "2".into()],
        client_id: String::new(),
        extra: AuthExtra {
            authtoken: "test-token".into(),
            os: "linux".into(),
            arch: "amd64".into(),
            ..Default::default()
        },
    };
    let json = serde_json::to_string(&auth).unwrap();
    assert!(json.contains("\"3\""));
    assert!(json.contains("test-token"));
}
```

### Test 3: Agent Connect and Listen (In-Process)

```rust
#[tokio::test]
async fn agent_listens_and_returns_url() {
    use ngrok_testing::MockNgrokServer;

    let (server, connect_url) = MockNgrokServer::start().await;

    let agent = ngrok::Agent::builder()
        .authtoken("test-token")
        .with_mock_server(&connect_url)
        .build()
        .await
        .unwrap();

    let listener = agent
        .listen(ngrok::EndpointOptions::builder().build())
        .await
        .unwrap();

    assert!(listener.url().to_string().contains("ngrok"));
    server.drop_session(); // clean up
}
```

### Test 4: Reconnect on Transport Drop

```rust
#[tokio::test]
async fn agent_reconnects_after_transport_drop() {
    use ngrok_testing::MockNgrokServer;
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

    let (server, connect_url) = MockNgrokServer::start().await;
    let connect_count = Arc::new(AtomicU32::new(0));
    let cc = connect_count.clone();

    let agent = ngrok::Agent::builder()
        .authtoken("test-token")
        .with_mock_server(&connect_url)
        .on_event(move |evt| {
            if matches!(evt, ngrok::Event::AgentConnectSucceeded(_)) {
                cc.fetch_add(1, Ordering::Relaxed);
            }
        })
        .build()
        .await
        .unwrap();

    let _listener = agent
        .listen(ngrok::EndpointOptions::builder().build())
        .await
        .unwrap();

    assert_eq!(connect_count.load(Ordering::Relaxed), 1);

    server.drop_session();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Agent should have reconnected
    assert_eq!(connect_count.load(Ordering::Relaxed), 2);
}
```

### Test 5: Error Code Extraction

```rust
#[test]
fn cloud_error_exposes_code() {
    use ngrok::Error;
    let err = Error::Cloud {
        code: "ERR_NGROK_108".into(),
        message: "invalid authtoken".into(),
    };
    assert_eq!(err.code(), Some("ERR_NGROK_108"));
    assert!(err.to_string().contains("ERR_NGROK_108"));
}
```

---

## CI Considerations

```yaml
# .github/workflows/ci.yml (conceptual)
steps:
  - name: cargo clippy
    run: cargo clippy --workspace --all-targets --all-features -- -D warnings

  - name: cargo test muxado crate (no features, fast)
    run: cargo test -p muxado

  - name: cargo test (offline, all feature combos)
    run: |
      cargo test --workspace --no-default-features
      cargo test --workspace --features hyper
      cargo test --workspace --features axum
      cargo test --workspace --features aws-lc-rs
      cargo test --workspace --all-features

  - name: cargo test (online, requires secret)
    if: env.NGROK_AUTHTOKEN != ''
    run: cargo test --features online-tests -- --include-ignored
    env:
      NGROK_AUTHTOKEN: ${{ secrets.NGROK_AUTHTOKEN }}

  - name: cargo bench (smoke only)
    run: cargo bench --workspace -- --test

  - name: coverage gate
    run: cargo llvm-cov --workspace --all-features --lcov --output-path lcov.info
    # fail if total < 80%
```

All tests must pass without `NGROK_AUTHTOKEN`. The `online-tests` Cargo feature
gates online tests; they are `#[ignore]`d by default.
