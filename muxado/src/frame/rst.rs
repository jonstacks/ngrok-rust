//! RST frame.

use crate::ErrorCode;

/// An RST frame that resets a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RstFrame {
    /// Stream identifier.
    pub stream_id: u32,
    /// Error code explaining the reset.
    pub error_code: ErrorCode,
}
