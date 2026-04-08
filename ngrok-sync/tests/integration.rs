//! Integration tests for the ngrok-sync crate using MockNgrokServer.

use std::{
    sync::{
        Arc,
        Once,
    },
    time::Duration,
};

use ngrok_sync::{
    AgentBuilder,
    EndpointOptions,
};
use ngrok_testing::MockNgrokServer;
use tokio::runtime::Runtime;

/// Install the rustls ring CryptoProvider once for the entire test binary.
fn install_crypto_provider() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("failed to install CryptoProvider");
    });
}

/// Create a shared tokio runtime, start the mock server on it, and return
/// `(runtime, connect_url)`. The runtime keeps the mock server's background
/// tasks alive.
fn setup() -> (Arc<Runtime>, String) {
    install_crypto_provider();
    let rt = Arc::new(Runtime::new().expect("failed to create tokio runtime"));
    let (_server, url) = rt.block_on(MockNgrokServer::start());
    // Leak the server Arc so its background tasks stay alive for the test.
    // (The runtime keeps the spawned accept loop running.)
    std::mem::forget(_server);
    (rt, url)
}

/// Build a connected sync Agent using the given runtime and mock server URL.
fn build_agent(rt: Arc<Runtime>, connect_url: &str) -> ngrok_sync::Agent {
    AgentBuilder::new()
        .authtoken("test-token")
        .connect_url(connect_url)
        .tls_config(ngrok_testing::danger_accept_any_cert_config())
        .runtime(rt)
        .build()
        .expect("failed to build sync agent")
}

// ---------------------------------------------------------------------------
// 1. sync_agent_builder_and_connect
// ---------------------------------------------------------------------------

#[test]
fn sync_agent_builder_and_connect() {
    let (rt, url) = setup();
    let agent = build_agent(rt, &url);

    agent.connect().expect("connect failed");

    let session = agent.session();
    assert!(session.is_some(), "session should be Some after connect");
    let session = session.unwrap();
    assert!(!session.id().is_empty(), "session ID should not be empty");
}

// ---------------------------------------------------------------------------
// 2. sync_agent_listen
// ---------------------------------------------------------------------------

#[test]
fn sync_agent_listen() {
    let (rt, url) = setup();
    let agent = build_agent(rt, &url);
    agent.connect().expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
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
// 3. sync_agent_listen_and_accept
// ---------------------------------------------------------------------------

#[test]
fn sync_agent_listen_and_accept() {
    let rt_arc = Arc::new(Runtime::new().expect("failed to create runtime"));
    install_crypto_provider();

    let (server, url) = rt_arc.block_on(MockNgrokServer::start());

    let agent = AgentBuilder::new()
        .authtoken("test-token")
        .connect_url(&url)
        .tls_config(ngrok_testing::danger_accept_any_cert_config())
        .runtime(Arc::clone(&rt_arc))
        .build()
        .expect("failed to build sync agent");

    agent.connect().expect("connect failed");

    let mut listener = agent
        .listen(EndpointOptions::default())
        .expect("listen failed");

    // Accept the mock endpoint on the server side and inject a connection.
    let mock_ep = rt_arc.block_on(server.accept_endpoint());
    let (client_stream, server_stream) = tokio::io::duplex(4096);
    rt_arc.block_on(mock_ep.inject_connection(server_stream));

    // Use the sync accept -- this blocks until a connection arrives.
    let (ngrok_stream, addr) = listener
        .accept_timeout(Duration::from_secs(5))
        .expect("accept timed out or failed");
    assert!(
        !addr.as_str().is_empty(),
        "connection should have a remote address"
    );

    // Clean up: write/read through the stream to prove it works.
    use tokio::io::{
        AsyncReadExt,
        AsyncWriteExt,
    };
    rt_arc.block_on(async {
        let (_client_read, mut client_write) = tokio::io::split(client_stream);
        let mut stream = ngrok_stream;

        client_write
            .write_all(b"hello sync")
            .await
            .expect("client write failed");
        client_write.shutdown().await.expect("shutdown failed");

        let mut buf = vec![0u8; 64];
        let n = stream.read(&mut buf).await.expect("read failed");
        assert_eq!(&buf[..n], b"hello sync");
    });
}

// ---------------------------------------------------------------------------
// 4. sync_agent_disconnect
// ---------------------------------------------------------------------------

#[test]
fn sync_agent_disconnect() {
    let (rt, url) = setup();
    let agent = build_agent(rt, &url);

    agent.connect().expect("connect failed");
    assert!(
        agent.session().is_some(),
        "session should exist after connect"
    );

    agent.disconnect().expect("disconnect failed");
}

// ---------------------------------------------------------------------------
// 5. sync_agent_endpoints
// ---------------------------------------------------------------------------

#[test]
fn sync_agent_endpoints() {
    let (rt, url) = setup();
    let agent = build_agent(rt, &url);
    agent.connect().expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
        .expect("listen failed");

    let endpoints = agent.endpoints();
    assert_eq!(endpoints.len(), 1, "should have exactly one endpoint");
    assert_eq!(endpoints[0].id, listener.id());
    assert_eq!(endpoints[0].url.as_str(), listener.url().as_str());
}

// ---------------------------------------------------------------------------
// 6. sync_listener_url_and_id
// ---------------------------------------------------------------------------

#[test]
fn sync_listener_url_and_id() {
    let (rt, url) = setup();
    let agent = build_agent(rt, &url);
    agent.connect().expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
        .expect("listen failed");

    // URL should be a valid https URL assigned by the mock server.
    let listener_url = listener.url();
    assert!(listener_url.scheme() == "https", "scheme should be https");
    assert!(
        listener_url.host_str().is_some(),
        "URL should have a host component"
    );

    // ID should be non-empty and start with the mock server's prefix.
    let id = listener.id();
    assert!(!id.is_empty(), "ID should not be empty");
    assert!(id.starts_with("ep_"), "ID should start with ep_");
}

// ---------------------------------------------------------------------------
// 7. sync_listener_close
// ---------------------------------------------------------------------------

#[test]
fn sync_listener_close() {
    let (rt, url) = setup();
    let agent = build_agent(rt, &url);
    agent.connect().expect("connect failed");

    let listener = agent
        .listen(EndpointOptions::default())
        .expect("listen failed");

    // Close should complete without error.
    listener.close().expect("close failed");
}

// ---------------------------------------------------------------------------
// 8. sync_agent_runtime_exposed
// ---------------------------------------------------------------------------

#[test]
fn sync_agent_runtime_exposed() {
    let (rt, url) = setup();
    let rt_clone = Arc::clone(&rt);
    let agent = build_agent(rt, &url);

    let exposed_rt = agent.runtime();
    // The exposed runtime should be the same Arc we injected.
    assert!(
        Arc::ptr_eq(&exposed_rt, &rt_clone),
        "runtime() should return the injected runtime"
    );
}

// ---------------------------------------------------------------------------
// 9. sync_agent_builder_mirrors_ngrok
// ---------------------------------------------------------------------------

#[test]
fn sync_agent_builder_mirrors_ngrok() {
    let (rt, url) = setup();

    // Chain every builder method to verify they all compile and don't panic.
    let agent = AgentBuilder::new()
        .authtoken("test-token")
        .connect_url(&url)
        .tls_config(ngrok_testing::danger_accept_any_cert_config())
        .heartbeat_interval(Duration::from_secs(5))
        .heartbeat_tolerance(Duration::from_secs(10))
        .client_info("test-client", "0.0.1")
        .description("integration test agent")
        .metadata(r#"{"env":"test"}"#)
        .with_tracing()
        .auto_connect(true)
        .runtime(rt)
        .build()
        .expect("build with all builder methods should succeed");

    agent.connect().expect("connect failed");
    assert!(agent.session().is_some(), "session should exist");
}
