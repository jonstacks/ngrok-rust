use std::io;

use thiserror::Error;

use crate::frame::{
    ErrorCode,
    Length,
    StreamID,
};

/// The muxado error type.
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error("No Error")]
    None = 0x00,
    #[error("Protocol Error")]
    Protocol = 0x01,
    #[error("Internal Error")]
    Internal = 0x02,
    #[error("Flow Control Error")]
    FlowControl = 0x03,
    #[error("Stream Closed")]
    StreamClosed = 0x04,
    #[error("Stream Refused")]
    StreamRefused = 0x05,
    #[error("Stream Cancelled")]
    StreamCancelled = 0x06,
    #[error("Stream Reset")]
    StreamReset = 0x07,
    #[error("Frame Size Error")]
    FrameSizeError = 0x08,
    #[error("Accept Queue Full")]
    AcceptQueueFull = 0x09,
    #[error("Enhance Your Calm")]
    EnhanceYourCalm = 0x0A,
    #[error("Remote Gone Away")]
    RemoteGoneAway = 0x0B,
    #[error("Streams Exhausted")]
    StreamsExhausted = 0x0C,
    #[error("Write Timeout")]
    WriteTimeout = 0x0D,
    #[error("Session Closed")]
    SessionClosed = 0x0E,
    #[error("Peer EOF")]
    PeerEOF = 0x0F,

    #[error("Unknown Error")]
    ErrorUnknown = u32::MAX,
}

impl From<Error> for ErrorCode {
    fn from(other: Error) -> ErrorCode {
        ErrorCode::mask(other as u32)
    }
}

impl From<ErrorCode> for Error {
    fn from(other: ErrorCode) -> Error {
        use Error::*;
        match *other {
            0x00 => None,
            0x01 => Protocol,
            0x02 => Internal,
            0x03 => FlowControl,
            0x04 => StreamClosed,
            0x05 => StreamRefused,
            0x06 => StreamCancelled,
            0x07 => StreamReset,
            0x08 => FrameSizeError,
            0x09 => AcceptQueueFull,
            0x0A => EnhanceYourCalm,
            0x0B => RemoteGoneAway,
            0x0C => StreamsExhausted,
            0x0D => WriteTimeout,
            0x0E => SessionClosed,
            0x0F => PeerEOF,

            _ => ErrorUnknown,
        }
    }
}

#[derive(Copy, Clone, Debug, Error, PartialEq, Eq)]
pub enum InvalidHeader {
    #[error("StreamID should be non-zero")]
    ZeroStreamID,
    #[error("StreamID should be zero, got {0}")]
    NonZeroStreamID(StreamID),
    #[error("Length should be {expected}, got {actual}")]
    Length { expected: Length, actual: Length },
    #[error("Length should be at least {expected}, got {actual}")]
    MinLength { expected: Length, actual: Length },
    #[error("Invalid frame type: {0}")]
    Type(u8),
}

impl From<InvalidHeader> for io::Error {
    fn from(other: InvalidHeader) -> io::Error {
        io::Error::other(other)
    }
}

impl From<InvalidHeader> for Error {
    fn from(_: InvalidHeader) -> Error {
        Error::Protocol
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::frame::ErrorCode;

    #[test]
    fn all_known_error_codes_round_trip() {
        use Error::*;
        let known = [
            (0x00u32, None),
            (0x01, Protocol),
            (0x02, Internal),
            (0x03, FlowControl),
            (0x04, StreamClosed),
            (0x05, StreamRefused),
            (0x06, StreamCancelled),
            (0x07, StreamReset),
            (0x08, FrameSizeError),
            (0x09, AcceptQueueFull),
            (0x0A, EnhanceYourCalm),
            (0x0B, RemoteGoneAway),
            (0x0C, StreamsExhausted),
            (0x0D, WriteTimeout),
            (0x0E, SessionClosed),
            (0x0F, PeerEOF),
        ];

        for (code, variant) in known {
            let ec = ErrorCode::mask(code);
            let err: Error = ec.into();
            assert_eq!(err, variant, "code {code:#04x} should map to {variant:?}");

            let ec_back = ErrorCode::from(variant);
            assert_eq!(
                *ec_back, code,
                "{variant:?} should encode back to {code:#04x}"
            );
        }
    }

    #[test]
    fn unknown_error_codes_map_to_error_unknown() {
        // Codes 0x10 through 0xFE are not defined and must map to ErrorUnknown.
        for code in [0x10u32, 0xFF, 0x1234, 0xDEAD_BEEF, u32::MAX - 1] {
            let err: Error = ErrorCode::mask(code).into();
            assert_eq!(
                err,
                Error::ErrorUnknown,
                "code {code:#010x} should map to ErrorUnknown"
            );
        }
    }

    #[test]
    fn error_unknown_encodes_as_u32_max() {
        let ec = ErrorCode::from(Error::ErrorUnknown);
        assert_eq!(*ec, u32::MAX, "ErrorUnknown should encode as u32::MAX");
    }

    #[test]
    fn invalid_header_converts_to_protocol_error() {
        let variants: &[InvalidHeader] = &[
            InvalidHeader::ZeroStreamID,
            InvalidHeader::NonZeroStreamID(StreamID::clamp(1)),
            InvalidHeader::Length {
                expected: Length::clamp(4),
                actual: Length::clamp(8),
            },
            InvalidHeader::MinLength {
                expected: Length::clamp(8),
                actual: Length::clamp(0),
            },
            InvalidHeader::Type(0xFF),
        ];
        for v in variants {
            let err: Error = (*v).into();
            assert_eq!(
                err,
                Error::Protocol,
                "InvalidHeader::{v:?} should map to Protocol"
            );
        }
    }
}
