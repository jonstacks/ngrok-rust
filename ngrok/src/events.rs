//! Event types dispatched to registered handlers.

use std::time::{
    Duration,
    SystemTime,
};

use crate::session::AgentSession;

/// All events that can be dispatched to registered handlers.
///
/// Handlers registered with `AgentBuilder::on_event` receive these events
/// synchronously. Handlers **must not block** — spawn a task if work is needed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// The agent successfully connected to ngrok cloud.
    AgentConnectSucceeded(AgentConnectSucceededEvent),
    /// The agent disconnected from ngrok cloud.
    AgentDisconnected(AgentDisconnectedEvent),
    /// A heartbeat was received from ngrok cloud.
    AgentHeartbeatReceived(AgentHeartbeatReceivedEvent),
    /// A new connection was opened on an endpoint.
    ConnectionOpened(ConnectionOpenedEvent),
    /// A connection was closed on an endpoint.
    ConnectionClosed(ConnectionClosedEvent),
    /// An HTTP request completed.
    HttpRequestComplete(HttpRequestCompleteEvent),
}

/// Fired when the agent successfully connects to ngrok cloud.
#[derive(Debug, Clone)]
pub struct AgentConnectSucceededEvent {
    /// When the event occurred.
    pub occurred_at: SystemTime,
    /// The agent session that was established.
    pub session: AgentSession,
}

/// Fired when the agent disconnects from ngrok cloud.
#[derive(Debug, Clone)]
pub struct AgentDisconnectedEvent {
    /// When the event occurred.
    pub occurred_at: SystemTime,
    /// The agent session that was disconnected.
    pub session: AgentSession,
    /// The error that caused the disconnect, if any.
    pub error: Option<String>,
}

/// Fired when a heartbeat is received from ngrok cloud.
#[derive(Debug, Clone)]
pub struct AgentHeartbeatReceivedEvent {
    /// When the event occurred.
    pub occurred_at: SystemTime,
    /// The active agent session.
    pub session: AgentSession,
    /// The measured round-trip latency.
    pub latency: Duration,
}

/// Fired when a new connection is opened on an endpoint.
#[derive(Debug, Clone)]
pub struct ConnectionOpenedEvent {
    /// When the event occurred.
    pub occurred_at: SystemTime,
    /// The endpoint ID.
    pub endpoint_id: String,
    /// The remote address.
    pub remote_addr: String,
}

/// Fired when a connection is closed on an endpoint.
#[derive(Debug, Clone)]
pub struct ConnectionClosedEvent {
    /// When the event occurred.
    pub occurred_at: SystemTime,
    /// The endpoint ID.
    pub endpoint_id: String,
    /// The remote address.
    pub remote_addr: String,
    /// How long the connection was open.
    pub duration: Duration,
    /// Bytes received from the remote.
    pub bytes_in: u64,
    /// Bytes sent to the remote.
    pub bytes_out: u64,
}

/// Fired when an HTTP request completes.
#[derive(Debug, Clone)]
pub struct HttpRequestCompleteEvent {
    /// When the event occurred.
    pub occurred_at: SystemTime,
    /// The endpoint ID.
    pub endpoint_id: String,
    /// The HTTP method.
    pub method: String,
    /// The request path.
    pub path: String,
    /// The HTTP status code.
    pub status_code: u16,
    /// How long the request took.
    pub duration: Duration,
}
