use std::time::Duration;

/// Events emitted by the ngrok agent.
#[derive(Debug, Clone)]
pub enum Event {
    /// The agent successfully connected to the ngrok service.
    AgentConnectSucceeded,
    /// The agent disconnected from the ngrok service.
    AgentDisconnected {
        /// The error that caused the disconnection, if any.
        error: Option<String>,
    },
    /// A heartbeat response was received from the ngrok service.
    AgentHeartbeatReceived {
        /// The round-trip latency of the heartbeat.
        latency: Duration,
    },
}
