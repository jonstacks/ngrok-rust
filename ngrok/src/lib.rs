//! # ngrok
//!
//! Async Rust SDK for ngrok.

pub mod agent;
pub mod defaults;
pub mod diagnose;
pub mod endpoint;
pub mod error;
pub mod events;
pub mod forwarder;
pub mod listener;
pub mod options;
pub mod proto;
pub mod rpc;
pub mod session;
pub mod upstream;

pub(crate) mod tunnel;

pub mod integrations;

pub use agent::{
    Agent,
    AgentBuilder,
};
pub use defaults::{
    forward,
    listen,
};
pub use diagnose::DiagnoseResult;
pub use endpoint::{
    EndpointInfo,
    EndpointKind,
};
pub use error::{
    DiagnoseError,
    Error,
};
pub use events::{
    AgentConnectSucceededEvent,
    AgentDisconnectedEvent,
    AgentHeartbeatReceivedEvent,
    ConnectionClosedEvent,
    ConnectionOpenedEvent,
    Event,
    HttpRequestCompleteEvent,
};
pub use forwarder::EndpointForwarder;
pub use listener::{
    EndpointListener,
    NgrokAddr,
    NgrokStream,
};
pub use options::{
    EndpointOptions,
    EndpointOptionsBuilder,
};
pub use rpc::{
    RpcMethod,
    RpcRequest,
    RpcResponse,
};
pub use session::AgentSession;
pub use upstream::{
    ProxyProtoVersion,
    Upstream,
};

/// Default ngrok cloud connect address.
pub const DEFAULT_CONNECT_URL: &str = "connect.ngrok-agent.com:443";

/// Minimum TLS version required.
pub const MIN_TLS_VERSION: rustls::ProtocolVersion = rustls::ProtocolVersion::TLSv1_2;

/// Extension trait for `AgentBuilder` to simplify test setup.
///
/// Available when the `testing` feature is enabled or `ngrok-testing` is a dev-dependency.
pub trait AgentTestExt: Sized {
    /// Connect to a `MockNgrokServer` instead of real ngrok cloud.
    fn with_mock_server(self, connect_url: &str) -> Self;

    /// Skip TLS verification (for in-process test servers using self-signed certs).
    fn danger_accept_any_cert(self) -> Self;
}

#[cfg(any(feature = "testing", test))]
impl AgentTestExt for AgentBuilder {
    fn with_mock_server(self, connect_url: &str) -> Self {
        self.connect_url(connect_url).danger_accept_any_cert()
    }

    fn danger_accept_any_cert(self) -> Self {
        self.tls_config(ngrok_testing::danger_accept_any_cert_config())
    }
}
