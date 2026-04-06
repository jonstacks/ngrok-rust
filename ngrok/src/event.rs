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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_debug() {
        let event = Event::AgentConnectSucceeded;
        let debug = format!("{:?}", event);
        assert!(debug.contains("AgentConnectSucceeded"));
    }

    #[test]
    fn test_event_clone() {
        let event = Event::AgentHeartbeatReceived {
            latency: Duration::from_millis(42),
        };
        let cloned = event.clone();
        if let Event::AgentHeartbeatReceived { latency } = cloned {
            assert_eq!(latency.as_millis(), 42);
        } else {
            panic!("wrong variant");
        }
    }
}
