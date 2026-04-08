/// Error codes used in RST and GOAWAY frames, matching the muxado wire protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ErrorCode {
    /// Clean shutdown, no error.
    NoError = 0,
    /// Fatal protocol violation.
    ProtocolError = 1,
    /// Internal error.
    InternalError = 2,
    /// Flow control window overflow.
    FlowControlError = 3,
    /// Data received on a closed stream.
    StreamClosed = 4,
    /// Stream refused (remote going away).
    StreamRefused = 5,
    /// Stream was cancelled.
    StreamCancelled = 6,
    /// Stream was reset.
    StreamReset = 7,
    /// Invalid frame size.
    FrameSizeError = 8,
    /// Accept backlog overflow.
    AcceptQueueFull = 9,
    /// Rate limiting.
    EnhanceYourCalm = 10,
    /// Remote sent GOAWAY.
    RemoteGoneAway = 11,
    /// All stream IDs exhausted.
    StreamsExhausted = 12,
    /// Write deadline exceeded.
    WriteTimeout = 13,
    /// Session was closed.
    SessionClosed = 14,
    /// Transport reached EOF.
    PeerEOF = 15,
}

impl ErrorCode {
    /// Convert a raw u32 to an ErrorCode, returning None for unknown values.
    pub fn from_u32(val: u32) -> Option<Self> {
        match val {
            0 => Some(Self::NoError),
            1 => Some(Self::ProtocolError),
            2 => Some(Self::InternalError),
            3 => Some(Self::FlowControlError),
            4 => Some(Self::StreamClosed),
            5 => Some(Self::StreamRefused),
            6 => Some(Self::StreamCancelled),
            7 => Some(Self::StreamReset),
            8 => Some(Self::FrameSizeError),
            9 => Some(Self::AcceptQueueFull),
            10 => Some(Self::EnhanceYourCalm),
            11 => Some(Self::RemoteGoneAway),
            12 => Some(Self::StreamsExhausted),
            13 => Some(Self::WriteTimeout),
            14 => Some(Self::SessionClosed),
            15 => Some(Self::PeerEOF),
            _ => None,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Errors that can occur during muxado operations.
#[derive(Debug, thiserror::Error)]
pub enum MuxadoError {
    #[error("protocol error: {code}")]
    Protocol { code: ErrorCode },

    #[error("stream error: {code} on stream {stream_id}")]
    Stream { code: ErrorCode, stream_id: u32 },

    #[error("frame size error: expected {expected}, got {actual}")]
    FrameSize { expected: usize, actual: usize },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("session closed")]
    SessionClosed,

    #[error("streams exhausted")]
    StreamsExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_roundtrip() {
        for val in 0..=15 {
            let code = ErrorCode::from_u32(val).unwrap();
            assert_eq!(code as u32, val);
        }
    }

    #[test]
    fn error_code_unknown_returns_none() {
        assert_eq!(ErrorCode::from_u32(16), None);
        assert_eq!(ErrorCode::from_u32(0xFF), None);
        assert_eq!(ErrorCode::from_u32(u32::MAX), None);
    }

    #[test]
    fn error_code_display() {
        assert_eq!(ErrorCode::NoError.to_string(), "NoError");
        assert_eq!(ErrorCode::ProtocolError.to_string(), "ProtocolError");
    }
}
