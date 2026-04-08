//! WNDINC frame.

/// A WNDINC (window increment) frame for per-stream flow control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WndIncFrame {
    /// Stream identifier.
    pub stream_id: u32,
    /// Number of bytes added to the remote's send window.
    pub increment: u32,
}
