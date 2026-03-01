use std::sync::Arc;

use super::*;

#[test]
fn test_agent_builder_default() {
    let agent = Agent::builder().build().unwrap();
    assert!(agent.auto_connect);
    assert!(agent.event_handlers.is_empty());
}

#[test]
fn test_agent_builder_auto_connect_false() {
    let agent = Agent::builder().auto_connect(false).build().unwrap();
    assert!(!agent.auto_connect);
}

#[test]
fn test_agent_builder_with_event_handler() {
    let handler: EventHandler = Arc::new(|_event| {});
    let agent = Agent::builder().event_handler(handler).build().unwrap();
    assert_eq!(1, agent.event_handlers.len());
}

#[test]
fn test_agent_builder_with_multiple_event_handlers() {
    let handler1: EventHandler = Arc::new(|_event| {});
    let handler2: EventHandler = Arc::new(|_event| {});
    let agent = Agent::builder()
        .event_handler(handler1)
        .event_handler(handler2)
        .build()
        .unwrap();
    assert_eq!(2, agent.event_handlers.len());
}

#[test]
fn test_agent_builder_with_metadata() {
    let _agent = Agent::builder()
        .metadata("test-metadata")
        .build()
        .unwrap();
}

#[test]
fn test_agent_builder_with_authtoken() {
    let _agent = Agent::builder()
        .authtoken("test-token")
        .build()
        .unwrap();
}

#[test]
fn test_agent_builder_with_client_info() {
    let _agent = Agent::builder()
        .client_info("test-client", "1.0.0", Some("test comment"))
        .build()
        .unwrap();
}

#[test]
fn test_upstream_new() {
    let upstream = Upstream::new("http://localhost:8080");
    assert_eq!("http://localhost:8080", upstream.addr);
    assert!(upstream.protocol.is_none());
    assert!(upstream.proxy_proto.is_none());
}

#[test]
fn test_upstream_with_protocol() {
    let upstream = Upstream::new("http://localhost:8080").protocol("http2");
    assert_eq!(Some("http2".to_string()), upstream.protocol);
}

#[test]
fn test_upstream_to_url_full() {
    let upstream = Upstream::new("http://localhost:8080");
    let url = upstream.to_url().unwrap();
    assert_eq!("http", url.scheme());
    assert_eq!(Some("localhost"), url.host_str());
    assert_eq!(Some(8080), url.port());
}

#[test]
fn test_upstream_to_url_port_only() {
    let upstream = Upstream::new("8080");
    let url = upstream.to_url().unwrap();
    assert_eq!(Some("localhost"), url.host_str());
    assert_eq!(Some(8080), url.port());
}

#[test]
fn test_upstream_to_url_host_port() {
    let upstream = Upstream::new("example.com:8080");
    let url = upstream.to_url().unwrap();
    assert_eq!(Some("example.com"), url.host_str());
    assert_eq!(Some(8080), url.port());
}

#[test]
fn test_endpoint_option_with_url() {
    let opts = endpoint_options::EndpointOpts::apply_options(vec![with_url(
        "https://example.ngrok.app",
    )]);
    assert_eq!(Some("https://example.ngrok.app".to_string()), opts.url);
}

#[test]
fn test_endpoint_option_with_metadata() {
    let opts = endpoint_options::EndpointOpts::apply_options(vec![with_metadata("test-meta")]);
    assert_eq!(Some("test-meta".to_string()), opts.metadata);
}

#[test]
fn test_endpoint_option_with_description() {
    let opts =
        endpoint_options::EndpointOpts::apply_options(vec![with_description("my endpoint")]);
    assert_eq!(Some("my endpoint".to_string()), opts.description);
}

#[test]
fn test_endpoint_option_with_traffic_policy() {
    let opts = endpoint_options::EndpointOpts::apply_options(vec![with_traffic_policy(
        r#"{"on_http_request": []}"#,
    )]);
    assert_eq!(
        Some(r#"{"on_http_request": []}"#.to_string()),
        opts.traffic_policy
    );
}

#[test]
fn test_endpoint_option_with_bindings() {
    let opts = endpoint_options::EndpointOpts::apply_options(vec![with_bindings(vec![
        "public".to_string(),
    ])]);
    assert_eq!(vec!["public"], opts.bindings);
}

#[test]
fn test_endpoint_option_with_pooling_enabled() {
    let opts = endpoint_options::EndpointOpts::apply_options(vec![with_pooling_enabled(true)]);
    assert_eq!(Some(true), opts.pooling_enabled);
}

#[test]
fn test_endpoint_options_combined() {
    let opts = endpoint_options::EndpointOpts::apply_options(vec![
        with_url("https://example.ngrok.app"),
        with_metadata("test-meta"),
        with_description("my endpoint"),
        with_pooling_enabled(true),
    ]);
    assert_eq!(Some("https://example.ngrok.app".to_string()), opts.url);
    assert_eq!(Some("test-meta".to_string()), opts.metadata);
    assert_eq!(Some("my endpoint".to_string()), opts.description);
    assert_eq!(Some(true), opts.pooling_enabled);
}

#[test]
fn test_endpoint_opts_url_scheme_default() {
    let opts = endpoint_options::EndpointOpts::default();
    assert_eq!("https", opts.url_scheme());
}

#[test]
fn test_endpoint_opts_url_scheme_http() {
    let opts =
        endpoint_options::EndpointOpts::apply_options(vec![with_url("http://example.ngrok.app")]);
    assert_eq!("http", opts.url_scheme());
}

#[test]
fn test_endpoint_opts_url_scheme_tcp() {
    let opts = endpoint_options::EndpointOpts::apply_options(vec![with_url(
        "tcp://1.tcp.ngrok.io:12345",
    )]);
    assert_eq!("tcp", opts.url_scheme());
}

#[test]
fn test_endpoint_opts_url_scheme_tls() {
    let opts =
        endpoint_options::EndpointOpts::apply_options(vec![with_url("tls://example.ngrok.app")]);
    assert_eq!("tls", opts.url_scheme());
}

#[test]
fn test_event_type_display() {
    assert_eq!(
        "AgentConnectSucceeded",
        EventType::AgentConnectSucceeded.to_string()
    );
    assert_eq!(
        "AgentDisconnected",
        EventType::AgentDisconnected.to_string()
    );
    assert_eq!(
        "AgentHeartbeatReceived",
        EventType::AgentHeartbeatReceived.to_string()
    );
}

#[test]
fn test_event_connect_succeeded() {
    let event = Event::AgentConnectSucceeded {
        timestamp: std::time::Instant::now(),
    };
    assert_eq!(EventType::AgentConnectSucceeded, event.event_type());
}

#[test]
fn test_event_disconnected() {
    let event = Event::AgentDisconnected {
        timestamp: std::time::Instant::now(),
        error: Some("test error".to_string()),
    };
    assert_eq!(EventType::AgentDisconnected, event.event_type());
}

#[test]
fn test_event_heartbeat_received() {
    let event = Event::AgentHeartbeatReceived {
        timestamp: std::time::Instant::now(),
        latency: std::time::Duration::from_millis(50),
    };
    assert_eq!(EventType::AgentHeartbeatReceived, event.event_type());
}

#[tokio::test]
async fn test_agent_not_connected_error() {
    let agent = Agent::builder().auto_connect(false).build().unwrap();
    let result = agent.session().await;
    assert!(result.is_none());
}

#[tokio::test]
async fn test_agent_endpoints_empty() {
    let agent = Agent::builder().build().unwrap();
    let endpoints = agent.endpoints().await;
    assert!(endpoints.is_empty());
}

#[tokio::test]
async fn test_agent_disconnect_when_not_connected() {
    let agent = Agent::builder().build().unwrap();
    // Should be a no-op, not an error
    let result = agent.disconnect().await;
    assert!(result.is_ok());
}
