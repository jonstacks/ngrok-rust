//! Agent session snapshot.

use std::time::SystemTime;

/// An immutable snapshot of a connected ngrok session.
///
/// Cheaply cloneable.
#[derive(Debug, Clone)]
pub struct AgentSession {
    /// Server-assigned session ID.
    pub(crate) id: String,
    /// Non-fatal warnings returned by ngrok cloud during auth.
    pub(crate) warnings: Vec<String>,
    /// Timestamp of successful `connect()`.
    pub(crate) started_at: SystemTime,
}

impl AgentSession {
    /// Server-assigned session ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Non-fatal warnings returned by ngrok cloud during auth.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Timestamp of successful `connect()`.
    pub fn started_at(&self) -> SystemTime {
        self.started_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_accessors() {
        let now = SystemTime::now();
        let session = AgentSession {
            id: "sess_abc123".into(),
            warnings: vec!["deprecation notice".into()],
            started_at: now,
        };
        assert_eq!(session.id(), "sess_abc123");
        assert_eq!(session.warnings(), &["deprecation notice"]);
        assert_eq!(session.started_at(), now);
    }

    #[test]
    fn session_clone() {
        let session = AgentSession {
            id: "sess_1".into(),
            warnings: Vec::new(),
            started_at: SystemTime::now(),
        };
        let cloned = session.clone();
        assert_eq!(session.id(), cloned.id());
    }

    #[test]
    fn session_empty_warnings() {
        let session = AgentSession {
            id: "s".into(),
            warnings: Vec::new(),
            started_at: SystemTime::now(),
        };
        assert!(session.warnings().is_empty());
    }
}
