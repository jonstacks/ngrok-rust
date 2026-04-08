//! GOAWAY frame.

use bytes::Bytes;

use crate::ErrorCode;

/// Maximum allowed debug data length in a GOAWAY frame (1 MB).
pub const GOAWAY_MAX_DEBUG: usize = 1 << 20;

/// A GOAWAY frame that signals session shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoAwayFrame {
    /// The last stream ID processed by the sender.
    pub last_stream_id: u32,
    /// Error code describing the reason for shutdown.
    pub error_code: ErrorCode,
    /// Optional debug data (capped at 1 MB).
    pub debug_data: Bytes,
}
