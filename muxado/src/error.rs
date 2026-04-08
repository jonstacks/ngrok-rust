//! Error types for muxado.

use thiserror::Error;

/// Well-known muxado error codes transmitted in RST and GOAWAY frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ErrorCode {
    /// No error.
    NoError = 0,
    /// Protocol error.
    ProtocolError = 1,
    /// Internal error.
    InternalError = 2,
    /// Flow control error (window exceeded).
    FlowControlError = 3,
    /// Frame too large.
    FrameSizeError = 4,
    /// Stream closed.
    StreamClosed = 5,
    /// Stream refused.
    StreamRefused = 6,
    /// Stream reset.
    StreamReset = 7,
    /// Session closed.
    SessionClosed = 8,
    /// Unknown error code (catch-all).
    Unknown(u32),
}

impl ErrorCode {
    /// Convert from a raw u32.
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::NoError,
            1 => Self::ProtocolError,
            2 => Self::InternalError,
            3 => Self::FlowControlError,
            4 => Self::FrameSizeError,
            5 => Self::StreamClosed,
            6 => Self::StreamRefused,
            7 => Self::StreamReset,
            8 => Self::SessionClosed,
            other => Self::Unknown(other),
        }
    }

    /// Convert to a raw u32.
    pub fn as_u32(self) -> u32 {
        match self {
            Self::NoError => 0,
            Self::ProtocolError => 1,
            Self::InternalError => 2,
            Self::FlowControlError => 3,
            Self::FrameSizeError => 4,
            Self::StreamClosed => 5,
            Self::StreamRefused => 6,
            Self::StreamReset => 7,
            Self::SessionClosed => 8,
            Self::Unknown(v) => v,
        }
    }
}

/// Top-level error type for muxado operations.
#[derive(Debug, Error)]
pub enum MuxadoError {
    /// An I/O error occurred on the underlying transport.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The remote peer sent a GOAWAY frame.
    #[error("remote went away: {0:?}")]
    RemoteGoneAway(ErrorCode),
    /// This session has been closed locally or by the remote.
    #[error("session closed")]
    SessionClosed,
    /// All stream IDs in the local parity space have been used.
    #[error("stream id space exhausted")]
    StreamsExhausted,
    /// A stream was reset by the remote.
    #[error("stream reset: {0:?}")]
    StreamReset(ErrorCode),
    /// A received frame had an unexpected or invalid format.
    #[error("frame decode error: {0}")]
    FrameDecodeError(String),
    /// The debug data in a GOAWAY frame exceeded 1 MB.
    #[error("goaway debug data too large: {0} bytes")]
    GoAwayDebugTooLarge(usize),
    /// A write was attempted on a half-closed stream.
    #[error("stream write side is closed")]
    WriteAfterClose,
    /// The heartbeat response did not match the sent nonce.
    #[error("heartbeat mismatch")]
    HeartbeatMismatch,
    /// The heartbeat response was not received within the tolerance.
    #[error("heartbeat timeout")]
    HeartbeatTimeout,
}
