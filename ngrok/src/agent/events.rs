use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

/// Represents the type of event that occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    /// An agent successfully connected to the ngrok service.
    AgentConnectSucceeded,
    /// An agent disconnected from the ngrok service.
    AgentDisconnected,
    /// A heartbeat response was received.
    AgentHeartbeatReceived,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventType::AgentConnectSucceeded => write!(f, "AgentConnectSucceeded"),
            EventType::AgentDisconnected => write!(f, "AgentDisconnected"),
            EventType::AgentHeartbeatReceived => write!(f, "AgentHeartbeatReceived"),
        }
    }
}

/// An event emitted by the agent.
#[derive(Debug, Clone)]
pub enum Event {
    /// Emitted when the agent successfully connects to the ngrok service.
    AgentConnectSucceeded {
        /// When the event occurred.
        timestamp: Instant,
    },
    /// Emitted when the agent disconnects from the ngrok service.
    AgentDisconnected {
        /// When the event occurred.
        timestamp: Instant,
        /// The error that caused the disconnection, if any.
        error: Option<String>,
    },
    /// Emitted when a heartbeat response is received.
    AgentHeartbeatReceived {
        /// When the event occurred.
        timestamp: Instant,
        /// The round-trip latency of the heartbeat.
        latency: Duration,
    },
}

impl Event {
    /// Returns the type of this event.
    pub fn event_type(&self) -> EventType {
        match self {
            Event::AgentConnectSucceeded { .. } => EventType::AgentConnectSucceeded,
            Event::AgentDisconnected { .. } => EventType::AgentDisconnected,
            Event::AgentHeartbeatReceived { .. } => EventType::AgentHeartbeatReceived,
        }
    }

    /// Returns the timestamp when this event occurred.
    pub fn timestamp(&self) -> Instant {
        match self {
            Event::AgentConnectSucceeded { timestamp } => *timestamp,
            Event::AgentDisconnected { timestamp, .. } => *timestamp,
            Event::AgentHeartbeatReceived { timestamp, .. } => *timestamp,
        }
    }
}

/// A callback function for handling agent events.
///
/// Event handlers must not block. If you need to perform blocking operations,
/// send the event to a channel or spawn a task.
///
/// # Example
///
/// ```rust
/// use ngrok::agent::{Event, EventHandler};
/// use std::sync::Arc;
///
/// let handler: EventHandler = Arc::new(|event: Event| {
///     match &event {
///         Event::AgentConnectSucceeded { .. } => {
///             println!("Connected!");
///         }
///         Event::AgentDisconnected { error, .. } => {
///             println!("Disconnected: {:?}", error);
///         }
///         Event::AgentHeartbeatReceived { latency, .. } => {
///             println!("Heartbeat latency: {:?}", latency);
///         }
///     }
/// });
/// ```
pub type EventHandler = Arc<dyn Fn(Event) + Send + Sync>;
