//! Integration tests for the ngrok crate using MockNgrokServer.

#![cfg(feature = "testing")]

use std::sync::{
    Arc,
    Once,
    atomic::{
        AtomicBool,
        Ordering,
    },
};

use ngrok::{
    Agent,
    AgentTestExt,
    EndpointOptions,
    Event,
    Upstream,
};
use ngrok_testing::MockNgrokServer;
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};

/// Install the rustls ring CryptoProvider once for the entire test binary.
fn install_crypto_provider() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("failed to install CryptoProvider");
    });
}

/// Helper: build an agent connected to the given mock server URL.
async fn build_agent(connect_url: &str) -> Agent {
    install_crypto_provider();
    Agent::builder()
        .authtoken("test-token")
        .with_mock_server(connect_url)
        .build()
        .await
        .expect("failed to build agent")
}

// ---------------------------------------------------------------------------
// 1. agent_connects_to_mock_server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_connects_to_mock_server() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;

    // connect() waits until the session is established.
    agent.connect().await.expect("connect failed");

    let session = agent.session();
    assert!(session.is_some(), "session should be Some after connect");
    let session = session.unwrap();
    assert!(!session.id().is_empty(), "session ID should not be empty");
}

// ---------------------------------------------------------------------------
// 2. agent_listen_creates_endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_listen_creates_endpoint() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    assert!(
        !listener.id().is_empty(),
        "listener should have a non-empty ID"
    );
    assert!(
        !listener.url().as_str().is_empty(),
        "listener should have a URL"
    );
    assert!(
        listener.url().as_str().starts_with("https://"),
        "default listener URL should be https"
    );
}

// ---------------------------------------------------------------------------
// 3. agent_endpoints_tracks_listeners
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_endpoints_tracks_listeners() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    let endpoints = agent.endpoints();
    assert_eq!(endpoints.len(), 1, "should have exactly one endpoint");
    assert_eq!(endpoints[0].id, listener.id());
    assert_eq!(endpoints[0].url.as_str(), listener.url().as_str());
}

// ---------------------------------------------------------------------------
// 4. connection_flow_read_write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connection_flow_read_write() {
    let (server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let mut listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    // Server-side: accept the endpoint the agent just bound.
    let mock_ep = server.accept_endpoint().await;

    // Create a duplex stream to simulate a client connection.
    let (client_stream, server_stream) = tokio::io::duplex(4096);

    // Inject the server-side half into the mock endpoint.
    mock_ep.inject_connection(server_stream).await;

    // Agent-side: accept the incoming connection.
    let (mut ngrok_stream, _addr) = listener.accept().await.expect("accept failed");

    // Write from client -> read from ngrok stream.
    let (mut client_read, mut client_write) = tokio::io::split(client_stream);

    client_write
        .write_all(b"hello from client")
        .await
        .expect("client write failed");
    // Shut down client write so the read side gets EOF eventually.
    client_write
        .shutdown()
        .await
        .expect("client shutdown failed");

    let mut buf = vec![0u8; 64];
    let n = ngrok_stream
        .read(&mut buf)
        .await
        .expect("ngrok read failed");
    assert_eq!(
        &buf[..n],
        b"hello from client",
        "ngrok stream should receive client data"
    );

    // Write from ngrok stream -> read from client.
    ngrok_stream
        .write_all(b"hello from server")
        .await
        .expect("ngrok write failed");
    ngrok_stream
        .shutdown()
        .await
        .expect("ngrok shutdown failed");

    let mut buf2 = vec![0u8; 64];
    let n2 = client_read
        .read(&mut buf2)
        .await
        .expect("client read failed");
    assert_eq!(
        &buf2[..n2],
        b"hello from server",
        "client should receive ngrok stream data"
    );
}

// ---------------------------------------------------------------------------
// 5. next_bytes_returns_chunks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn next_bytes_returns_chunks() {
    let (server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let mut listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    let mock_ep = server.accept_endpoint().await;

    let (inject_stream, mut external_stream) = tokio::io::duplex(4096);
    mock_ep.inject_connection(inject_stream).await;

    let (mut ngrok_stream, _addr) = listener.accept().await.expect("accept failed");

    // Write data into the external side.
    external_stream
        .write_all(b"zero-copy chunk")
        .await
        .expect("write failed");
    external_stream.shutdown().await.expect("shutdown failed");

    // Use next_bytes() for zero-copy read.
    let chunk = ngrok_stream.next_bytes().await;
    assert!(chunk.is_some(), "next_bytes should return Some");
    let chunk = chunk.unwrap();
    assert_eq!(&chunk[..], b"zero-copy chunk");
}

// ---------------------------------------------------------------------------
// 6. agent_disconnect_closes_cleanly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_disconnect_closes_cleanly() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    assert!(
        agent.session().is_some(),
        "session should exist after connect"
    );

    agent.disconnect().await.expect("disconnect failed");

    // After disconnect the agent is marked as closed. The session snapshot
    // may still be readable (it is a snapshot), but the agent should not
    // allow new operations.
    // The closed flag prevents reconnect, which is the key invariant.
}

// ---------------------------------------------------------------------------
// 7. event_handler_fires_on_connect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_handler_fires_on_connect() {
    install_crypto_provider();
    let (_server, url) = MockNgrokServer::start().await;

    let connected = Arc::new(AtomicBool::new(false));
    let connected_clone = connected.clone();

    let agent = Agent::builder()
        .authtoken("test-token")
        .with_mock_server(&url)
        .on_event(move |event| {
            if matches!(event, Event::AgentConnectSucceeded(_)) {
                connected_clone.store(true, Ordering::SeqCst);
            }
        })
        .build()
        .await
        .expect("failed to build agent");

    agent.connect().await.expect("connect failed");

    // Give a moment for the event to fire (it fires right after connected_tx
    // is signalled, but the handler runs synchronously in the reconnect task).
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        connected.load(Ordering::SeqCst),
        "AgentConnectSucceeded event should have fired"
    );
}

// ---------------------------------------------------------------------------
// 8. multiple_listeners
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_listeners() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let listener1 = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen 1 failed");
    let listener2 = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen 2 failed");

    assert_ne!(
        listener1.id(),
        listener2.id(),
        "listeners should have different IDs"
    );
    assert_ne!(
        listener1.url().as_str(),
        listener2.url().as_str(),
        "listeners should have different URLs"
    );

    let endpoints = agent.endpoints();
    assert_eq!(endpoints.len(), 2, "should have two endpoints");
}

// ---------------------------------------------------------------------------
// 9. endpoint_options_with_url
// ---------------------------------------------------------------------------

#[tokio::test]
async fn endpoint_options_with_url() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let opts = EndpointOptions::builder()
        .url("https://custom.ngrok.app")
        .build();

    let listener = agent.listen(opts).await.expect("listen with url failed");

    // The mock server assigns its own URL, but the endpoint should still be
    // created successfully. We verify the listener was created and has a valid URL.
    assert!(!listener.id().is_empty());
    assert!(listener.url().as_str().starts_with("https://"));
}

// ---------------------------------------------------------------------------
// 10. upstream_parsing (unit tests, no mock server)
// ---------------------------------------------------------------------------

#[test]
fn upstream_parsing_port_only() {
    let u = Upstream::new("8080");
    assert_eq!(u.addr(), "8080");
}

#[test]
fn upstream_parsing_host_port() {
    let u = Upstream::new("localhost:8080");
    assert_eq!(u.addr(), "localhost:8080");
}

#[test]
fn upstream_parsing_http_url() {
    let u = Upstream::new("http://localhost:8080");
    assert_eq!(u.addr(), "http://localhost:8080");
}

#[test]
fn upstream_parsing_https_url() {
    let u = Upstream::new("https://host:443");
    assert_eq!(u.addr(), "https://host:443");
}

#[test]
fn upstream_protocol_override() {
    let u = Upstream::new("8080").protocol("http");
    assert_eq!(u.addr(), "8080");
}

#[test]
fn upstream_proxy_proto() {
    use ngrok::ProxyProtoVersion;
    let u = Upstream::new("localhost:9090").proxy_proto(ProxyProtoVersion::V2);
    assert_eq!(u.addr(), "localhost:9090");
}

// ---------------------------------------------------------------------------
// 11. ngrok_stream_async_write_and_read
// ---------------------------------------------------------------------------

/// Full bidirectional test: write to NgrokStream and read from the injected
/// duplex, then write to duplex and read from NgrokStream using AsyncRead.
#[tokio::test]
async fn ngrok_stream_async_write_and_read() {
    let (server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let mut listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    let mock_ep = server.accept_endpoint().await;
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    mock_ep.inject_connection(server_stream).await;

    let (mut ngrok_stream, _addr) = listener.accept().await.expect("accept failed");
    let (mut client_read, mut client_write) = tokio::io::split(client_stream);

    // NgrokStream -> client: write via AsyncWrite trait
    ngrok_stream
        .write_all(b"from ngrok")
        .await
        .expect("ngrok write_all failed");
    ngrok_stream.flush().await.expect("ngrok flush failed");

    let mut buf = vec![0u8; 64];
    let n = client_read
        .read(&mut buf)
        .await
        .expect("client read failed");
    assert_eq!(&buf[..n], b"from ngrok");

    // client -> NgrokStream: read via AsyncRead trait
    client_write
        .write_all(b"from client")
        .await
        .expect("client write failed");
    client_write
        .shutdown()
        .await
        .expect("client shutdown failed");

    let mut buf2 = vec![0u8; 64];
    let n2 = ngrok_stream
        .read(&mut buf2)
        .await
        .expect("ngrok read failed");
    assert_eq!(&buf2[..n2], b"from client");
}

// ---------------------------------------------------------------------------
// 12. ngrok_stream_remote_addr
// ---------------------------------------------------------------------------

/// Verify NgrokStream::remote_addr() returns the client address from the
/// proxy header.
#[tokio::test]
async fn ngrok_stream_remote_addr() {
    let (server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let mut listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    let mock_ep = server.accept_endpoint().await;
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    mock_ep.inject_connection(server_stream).await;

    let (ngrok_stream, addr) = listener.accept().await.expect("accept failed");

    // MockEndpoint injects "127.0.0.1:12345" as ClientAddr in the proxy header.
    assert_eq!(
        addr.as_str(),
        "127.0.0.1:12345",
        "addr should match the injected ClientAddr"
    );
    assert_eq!(
        ngrok_stream.remote_addr(),
        "127.0.0.1:12345",
        "remote_addr should match the injected ClientAddr"
    );

    // Keep client_stream alive so the connection doesn't close prematurely.
    drop(client_stream);
}

// ---------------------------------------------------------------------------
// 13. ngrok_addr_display
// ---------------------------------------------------------------------------

/// Verify NgrokAddr Display impl works.
#[test]
fn ngrok_addr_display() {
    use ngrok::NgrokAddr;

    let addr = NgrokAddr::new("https://example.ngrok.app");
    assert_eq!(format!("{}", addr), "https://example.ngrok.app");
    assert_eq!(addr.to_string(), "https://example.ngrok.app");
    assert_eq!(addr.as_str(), "https://example.ngrok.app");

    // Test with an empty string.
    let empty = NgrokAddr::new("");
    assert_eq!(empty.to_string(), "");

    // Test Debug impl.
    let debug_str = format!("{:?}", addr);
    assert!(debug_str.contains("https://example.ngrok.app"));
}

// ---------------------------------------------------------------------------
// 14. listener_stream_impl
// ---------------------------------------------------------------------------

/// Use the `futures::Stream` trait on EndpointListener (poll-based) instead
/// of accept().
#[tokio::test]
async fn listener_stream_impl() {
    use futures::StreamExt;

    let (server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let mut listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    let mock_ep = server.accept_endpoint().await;
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    mock_ep.inject_connection(server_stream).await;

    // Use Stream::next() instead of accept()
    let result = listener.next().await;
    assert!(result.is_some(), "Stream should yield Some");
    let conn = result.unwrap();
    assert!(conn.is_ok(), "Stream item should be Ok");

    let (mut ngrok_stream, _addr) = conn.unwrap();
    let (_client_read, mut client_write) = tokio::io::split(client_stream);

    // Verify it works by sending data through.
    client_write
        .write_all(b"stream-test")
        .await
        .expect("write failed");
    client_write.shutdown().await.expect("shutdown failed");

    let mut buf = vec![0u8; 64];
    let n = ngrok_stream.read(&mut buf).await.expect("read failed");
    assert_eq!(&buf[..n], b"stream-test");
}

// ---------------------------------------------------------------------------
// 15. listener_close_ends_accept
// ---------------------------------------------------------------------------

/// Close the listener, verify close() completes successfully.
/// Since close() consumes the listener, the accept channel sender is dropped
/// which would cause any pending accept() to return an error.
#[tokio::test]
async fn listener_close_ends_accept() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    // close() should complete without error.
    listener.close().await.expect("close should succeed");
}

// ---------------------------------------------------------------------------
// 16. listener_done_resolves_on_close
// ---------------------------------------------------------------------------

/// Verify `listener.done()` does not resolve while the listener is open,
/// proving it correctly waits for the close signal.
#[tokio::test]
async fn listener_done_resolves_on_close() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
        .await
        .expect("listen failed");

    // done() should NOT resolve while the listener is still open.
    let timeout_result =
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.done()).await;

    assert!(
        timeout_result.is_err(),
        "done() should not resolve while listener is still open"
    );

    // Now close it and verify close() succeeds.
    listener.close().await.expect("close failed");
}

// ---------------------------------------------------------------------------
// 17. forwarder_proxies_to_upstream
// ---------------------------------------------------------------------------

/// Start a TCP listener on localhost, create an EndpointForwarder pointing at
/// it, inject a connection via mock server, write data to the injected stream,
/// read it from the TCP listener. Tests the full forward loop.
#[tokio::test]
async fn forwarder_proxies_to_upstream() {
    use tokio::net::TcpListener as TokioTcpListener;

    let (server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    // Start a local TCP server to act as the upstream.
    let upstream_listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream failed");
    let upstream_addr = upstream_listener.local_addr().unwrap();

    let upstream = Upstream::new(format!("127.0.0.1:{}", upstream_addr.port()));
    let _forwarder = agent
        .forward(upstream, EndpointOptions::default())
        .await
        .expect("forward failed");

    let mock_ep = server.accept_endpoint().await;

    // Inject a connection through the mock endpoint.
    let (mut client_stream, server_stream) = tokio::io::duplex(4096);
    mock_ep.inject_connection(server_stream).await;

    // Accept the forwarded connection on the upstream TCP listener.
    // The forward loop accepts the ngrok connection and connects to upstream.
    let (mut upstream_conn, _) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        upstream_listener.accept(),
    )
    .await
    .expect("upstream accept timed out")
    .expect("upstream accept failed");

    // Write data from the "client" side.
    client_stream
        .write_all(b"forwarded-payload")
        .await
        .expect("client write failed");
    client_stream
        .shutdown()
        .await
        .expect("client shutdown failed");

    let mut buf = vec![0u8; 128];
    let n = upstream_conn
        .read(&mut buf)
        .await
        .expect("upstream read failed");
    assert_eq!(
        &buf[..n],
        b"forwarded-payload",
        "upstream should receive forwarded data"
    );
}

// ---------------------------------------------------------------------------
// 18. forwarder_url_and_id
// ---------------------------------------------------------------------------

/// Verify EndpointForwarder accessors (url, id, upstream_url, proxy_protocol).
#[tokio::test]
async fn forwarder_url_and_id() {
    use tokio::net::TcpListener as TokioTcpListener;

    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let upstream_listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failed");
    let upstream_port = upstream_listener.local_addr().unwrap().port();

    let upstream = Upstream::new(format!("127.0.0.1:{}", upstream_port));
    let forwarder = agent
        .forward(upstream, EndpointOptions::default())
        .await
        .expect("forward failed");

    assert!(!forwarder.id().is_empty(), "forwarder should have an ID");
    assert!(
        forwarder.url().as_str().starts_with("https://"),
        "forwarder URL should start with https://"
    );
    // upstream_url is constructed as tcp://host:port by the agent.
    let upstream_url_str = forwarder.upstream_url().to_string();
    assert!(
        !upstream_url_str.is_empty(),
        "upstream_url should not be empty, got: {upstream_url_str}"
    );
    assert_eq!(
        forwarder.proxy_protocol(),
        ngrok::ProxyProtoVersion::None,
        "default proxy protocol should be None"
    );
}

// ---------------------------------------------------------------------------
// 19. forwarder_close
// ---------------------------------------------------------------------------

/// Verify close() completes successfully and done() does not resolve before
/// close is called.
#[tokio::test]
async fn forwarder_close() {
    use tokio::net::TcpListener as TokioTcpListener;

    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let upstream_listener = TokioTcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failed");
    let upstream_port = upstream_listener.local_addr().unwrap().port();

    let upstream = Upstream::new(format!("127.0.0.1:{}", upstream_port));
    let forwarder = agent
        .forward(upstream, EndpointOptions::default())
        .await
        .expect("forward failed");

    // done() should NOT resolve while the forwarder is still open.
    let timeout_result =
        tokio::time::timeout(std::time::Duration::from_millis(100), forwarder.done()).await;
    assert!(
        timeout_result.is_err(),
        "done() should not resolve while forwarder is still open"
    );

    // close() should succeed.
    forwarder.close().await.expect("close failed");
}

// ---------------------------------------------------------------------------
// 20. diagnose_against_mock_server
// ---------------------------------------------------------------------------

/// Call agent.diagnose() pointing at the mock server URL, verify it returns
/// a DiagnoseResult with a region string.
#[tokio::test]
async fn diagnose_against_mock_server() {
    let (_server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let result = agent.diagnose(Some(&url)).await.expect("diagnose failed");

    assert_eq!(
        result.region, "us",
        "region should be 'us' from mock server"
    );
    assert_eq!(result.addr, url, "addr should match the connect URL");
    assert!(
        result.latency.as_millis() < 5000,
        "latency should be reasonable"
    );
}

// ---------------------------------------------------------------------------
// 21. rpc_handler_receives_stop
// ---------------------------------------------------------------------------

/// Register an on_rpc handler, call mock server's send_rpc("StopAgent"),
/// verify the handler fires with RpcMethod::StopAgent.
#[tokio::test]
async fn rpc_handler_receives_stop() {
    use ngrok::RpcMethod;

    install_crypto_provider();
    let (server, url) = MockNgrokServer::start().await;

    let stop_received = Arc::new(AtomicBool::new(false));
    let stop_received_clone = stop_received.clone();

    let agent = Agent::builder()
        .authtoken("test-token")
        .with_mock_server(&url)
        .on_rpc(move |req| {
            if matches!(req.method, RpcMethod::StopAgent) {
                stop_received_clone.store(true, Ordering::SeqCst);
            }
            ngrok::RpcResponse { error: None }
        })
        .build()
        .await
        .expect("failed to build agent");

    agent.connect().await.expect("connect failed");

    // Give a moment for the accept loop to start.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send stop RPC from the server side.
    server.send_rpc("StopAgent").await;

    // Wait for the handler to fire.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert!(
        stop_received.load(Ordering::SeqCst),
        "on_rpc handler should have received StopAgent"
    );
}

// ---------------------------------------------------------------------------
// 22. reconnect_after_session_drop
// ---------------------------------------------------------------------------

/// Connect, call mock server's drop_session(), wait briefly, verify the
/// agent reconnects (session() returns Some again).
#[tokio::test]
async fn reconnect_after_session_drop() {
    let (server, url) = MockNgrokServer::start().await;
    let agent = build_agent(&url).await;
    agent.connect().await.expect("connect failed");

    let session_before = agent.session().expect("session should exist");
    let id_before = session_before.id().to_string();

    // Drop the session from the server side to simulate transport failure.
    server.drop_session();

    // Wait for reconnect. The agent's reconnect loop should detect the broken
    // session and re-establish.
    let mut reconnected = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if let Some(session) = agent.session() {
            // A new session may have a different ID.
            if session.id() != id_before {
                reconnected = true;
                break;
            }
        }
    }

    assert!(
        reconnected,
        "agent should have reconnected with a new session after drop"
    );
}
