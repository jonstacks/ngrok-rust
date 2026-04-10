use bytes::{
    Buf,
    BufMut,
    Bytes,
    BytesMut,
};

use crate::error::ErrorCode;

/// Size of the frame header in bytes.
pub const HEADER_SIZE: usize = 8;

/// Maximum frame payload length (24-bit).
pub const MAX_PAYLOAD_LENGTH: u32 = 0x00FF_FFFF;

/// Maximum debug data in GOAWAY frames (1 MB).
const MAX_GOAWAY_DEBUG_SIZE: usize = 1024 * 1024;

/// Frame types (4-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Rst = 0x0,
    Data = 0x1,
    WndInc = 0x2,
    GoAway = 0x3,
}

impl FrameType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x0 => Some(Self::Rst),
            0x1 => Some(Self::Data),
            0x2 => Some(Self::WndInc),
            0x3 => Some(Self::GoAway),
            _ => None,
        }
    }
}

/// Flags on DATA frames (4-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataFlags(u8);

impl DataFlags {
    pub const NONE: Self = Self(0);
    /// Half-close: sender will send no more data.
    pub const FIN: Self = Self(0x1);
    /// Stream open: this DATA frame initiates a new stream.
    pub const SYN: Self = Self(0x2);

    pub fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn from_bits(val: u8) -> Self {
        Self(val & 0x0F)
    }
}

/// Common frame header fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub length: u32,
    pub frame_type: FrameType,
    pub flags: u8,
    pub stream_id: u32,
}

impl Header {
    /// Serialize the header into 8 bytes.
    pub fn encode(&self, buf: &mut [u8; HEADER_SIZE]) {
        // Bytes 0-2: length (24-bit big-endian)
        buf[0] = (self.length >> 16) as u8;
        buf[1] = (self.length >> 8) as u8;
        buf[2] = self.length as u8;
        // Byte 3: type (high nibble) | flags (low nibble)
        buf[3] = ((self.frame_type as u8) << 4) | (self.flags & 0x0F);
        // Bytes 4-7: stream ID (31-bit big-endian)
        let id = self.stream_id & 0x7FFF_FFFF;
        buf[4] = (id >> 24) as u8;
        buf[5] = (id >> 16) as u8;
        buf[6] = (id >> 8) as u8;
        buf[7] = id as u8;
    }

    /// Parse a header from 8 bytes.
    pub fn decode(buf: &[u8; HEADER_SIZE]) -> Result<Self, FrameError> {
        let length = ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32);
        let type_val = (buf[3] & 0xF0) >> 4;
        let flags = buf[3] & 0x0F;
        let stream_id = ((buf[4] as u32) << 24)
            | ((buf[5] as u32) << 16)
            | ((buf[6] as u32) << 8)
            | (buf[7] as u32);
        let stream_id = stream_id & 0x7FFF_FFFF;

        let frame_type = FrameType::from_u8(type_val);

        Ok(Self {
            length,
            frame_type: match frame_type {
                Some(ft) => ft,
                None => {
                    return Err(FrameError::UnknownFrameType {
                        type_val,
                        length,
                        stream_id,
                    });
                }
            },
            flags,
            stream_id,
        })
    }
}

/// Parsed frame variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Data(DataFrame),
    Rst(RstFrame),
    WndInc(WndIncFrame),
    GoAway(GoAwayFrame),
}

impl Frame {
    pub fn stream_id(&self) -> u32 {
        match self {
            Self::Data(f) => f.stream_id,
            Self::Rst(f) => f.stream_id,
            Self::WndInc(f) => f.stream_id,
            Self::GoAway(_) => 0,
        }
    }
}

/// DATA frame: carries stream payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFrame {
    pub stream_id: u32,
    pub flags: DataFlags,
    pub body: Bytes,
}

impl DataFrame {
    /// Create a new DATA frame.
    pub fn new(stream_id: u32, flags: DataFlags, body: Bytes) -> Self {
        Self {
            stream_id,
            flags,
            body,
        }
    }

    /// Encode this frame into bytes (header + body).
    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + self.body.len());
        let header = Header {
            length: self.body.len() as u32,
            frame_type: FrameType::Data,
            flags: self.flags.bits(),
            stream_id: self.stream_id,
        };
        let mut hdr = [0u8; HEADER_SIZE];
        header.encode(&mut hdr);
        buf.put_slice(&hdr);
        buf.put_slice(&self.body);
        buf
    }
}

/// RST frame: forcibly close/reset a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RstFrame {
    pub stream_id: u32,
    pub error_code: u32,
}

impl RstFrame {
    pub fn new(stream_id: u32, error_code: ErrorCode) -> Self {
        Self {
            stream_id,
            error_code: error_code as u32,
        }
    }

    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4);
        let header = Header {
            length: 4,
            frame_type: FrameType::Rst,
            flags: 0,
            stream_id: self.stream_id,
        };
        let mut hdr = [0u8; HEADER_SIZE];
        header.encode(&mut hdr);
        buf.put_slice(&hdr);
        buf.put_u32(self.error_code);
        buf
    }
}

/// WNDINC frame: flow control window increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WndIncFrame {
    pub stream_id: u32,
    pub increment: u32,
}

impl WndIncFrame {
    pub fn new(stream_id: u32, increment: u32) -> Self {
        Self {
            stream_id,
            increment: increment & 0x7FFF_FFFF,
        }
    }

    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4);
        let header = Header {
            length: 4,
            frame_type: FrameType::WndInc,
            flags: 0,
            stream_id: self.stream_id,
        };
        let mut hdr = [0u8; HEADER_SIZE];
        header.encode(&mut hdr);
        buf.put_slice(&hdr);
        buf.put_u32(self.increment);
        buf
    }
}

/// GOAWAY frame: graceful session shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoAwayFrame {
    pub last_stream_id: u32,
    pub error_code: u32,
    pub debug_data: Bytes,
}

impl GoAwayFrame {
    pub fn new(last_stream_id: u32, error_code: ErrorCode, debug_data: Bytes) -> Self {
        Self {
            last_stream_id,
            error_code: error_code as u32,
            debug_data,
        }
    }

    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 8 + self.debug_data.len());
        let header = Header {
            length: (8 + self.debug_data.len()) as u32,
            frame_type: FrameType::GoAway,
            flags: 0,
            stream_id: 0,
        };
        let mut hdr = [0u8; HEADER_SIZE];
        header.encode(&mut hdr);
        buf.put_slice(&hdr);
        buf.put_u32(self.last_stream_id);
        buf.put_u32(self.error_code);
        buf.put_slice(&self.debug_data);
        buf
    }
}

/// Errors that can occur during frame parsing.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("unknown frame type: 0x{type_val:x}")]
    UnknownFrameType {
        type_val: u8,
        length: u32,
        stream_id: u32,
    },

    #[error("frame size error: expected {expected} bytes, got {actual}")]
    FrameSize { expected: usize, actual: usize },

    #[error("invalid stream id: {stream_id} (expected non-zero)")]
    InvalidStreamId { stream_id: u32 },

    #[error("invalid window increment: must be non-zero")]
    InvalidWindowIncrement,

    #[error("goaway debug data too large: {size} bytes (max {MAX_GOAWAY_DEBUG_SIZE})")]
    GoAwayDebugTooLarge { size: usize },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Decode a frame from a pre-parsed header and an owned payload.
///
/// The payload `Bytes` is moved into the resulting frame zero-copy for
/// DATA and GOAWAY frames. Prefer this over `decode_frame` when the
/// header has already been parsed (e.g. in the session reader loop).
pub fn decode_frame_payload(header: &Header, payload: Bytes) -> Result<Frame, FrameError> {
    match header.frame_type {
        FrameType::Data => Ok(Frame::Data(DataFrame {
            stream_id: header.stream_id,
            flags: DataFlags::from_bits(header.flags),
            body: payload,
        })),
        FrameType::Rst => {
            if header.length != 4 {
                return Err(FrameError::FrameSize {
                    expected: 4,
                    actual: header.length as usize,
                });
            }
            let error_code = u32::from_be_bytes(payload[..4].try_into().unwrap());
            Ok(Frame::Rst(RstFrame {
                stream_id: header.stream_id,
                error_code,
            }))
        }
        FrameType::WndInc => {
            if header.length != 4 {
                return Err(FrameError::FrameSize {
                    expected: 4,
                    actual: header.length as usize,
                });
            }
            let increment = u32::from_be_bytes(payload[..4].try_into().unwrap()) & 0x7FFF_FFFF;
            if increment == 0 {
                return Err(FrameError::InvalidWindowIncrement);
            }
            Ok(Frame::WndInc(WndIncFrame {
                stream_id: header.stream_id,
                increment,
            }))
        }
        FrameType::GoAway => {
            if header.length < 8 {
                return Err(FrameError::FrameSize {
                    expected: 8,
                    actual: header.length as usize,
                });
            }
            let mut p = &payload[..];
            let last_stream_id = p.get_u32();
            let error_code = p.get_u32();
            let debug_len = header.length as usize - 8;
            if debug_len > MAX_GOAWAY_DEBUG_SIZE {
                return Err(FrameError::GoAwayDebugTooLarge { size: debug_len });
            }
            let debug_data = payload.slice(8..8 + debug_len);
            Ok(Frame::GoAway(GoAwayFrame {
                last_stream_id,
                error_code,
                debug_data,
            }))
        }
    }
}

/// Decode a complete frame from a buffer. The buffer must contain at least
/// the header + payload bytes. Returns the frame and the number of bytes consumed.
pub fn decode_frame(buf: &[u8]) -> Result<(Frame, usize), FrameError> {
    if buf.len() < HEADER_SIZE {
        return Err(FrameError::FrameSize {
            expected: HEADER_SIZE,
            actual: buf.len(),
        });
    }

    let header_bytes: [u8; HEADER_SIZE] = buf[..HEADER_SIZE].try_into().unwrap();
    let header = Header::decode(&header_bytes)?;
    let total_size = HEADER_SIZE + header.length as usize;

    if buf.len() < total_size {
        return Err(FrameError::FrameSize {
            expected: total_size,
            actual: buf.len(),
        });
    }

    let payload = Bytes::copy_from_slice(&buf[HEADER_SIZE..total_size]);
    let frame = decode_frame_payload(&header, payload)?;
    Ok((frame, total_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Header encode/decode ────────────────────────────────────────────

    #[test]
    fn header_roundtrip() {
        let header = Header {
            length: 0x00ABCD,
            frame_type: FrameType::Data,
            flags: 0x03,
            stream_id: 0x12345678,
        };
        let mut buf = [0u8; HEADER_SIZE];
        header.encode(&mut buf);
        let decoded = Header::decode(&buf).unwrap();
        assert_eq!(decoded, header);
    }

    #[test]
    fn header_encodes_correct_bytes() {
        let header = Header {
            length: 0x000005,
            frame_type: FrameType::Data,
            flags: 0x01, // FIN
            stream_id: 1,
        };
        let mut buf = [0u8; HEADER_SIZE];
        header.encode(&mut buf);

        // Length: 0x000005 -> [0x00, 0x00, 0x05]
        assert_eq!(buf[0], 0x00);
        assert_eq!(buf[1], 0x00);
        assert_eq!(buf[2], 0x05);
        // Type(0x1) << 4 | Flags(0x1) = 0x11
        assert_eq!(buf[3], 0x11);
        // Stream ID: 1 -> [0x00, 0x00, 0x00, 0x01]
        assert_eq!(&buf[4..8], &[0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn header_masks_stream_id_top_bit() {
        let header = Header {
            length: 0,
            frame_type: FrameType::Rst,
            flags: 0,
            stream_id: 0xFFFF_FFFF,
        };
        let mut buf = [0u8; HEADER_SIZE];
        header.encode(&mut buf);
        let decoded = Header::decode(&buf).unwrap();
        assert_eq!(decoded.stream_id, 0x7FFF_FFFF);
    }

    #[test]
    fn header_max_length() {
        let header = Header {
            length: MAX_PAYLOAD_LENGTH,
            frame_type: FrameType::Data,
            flags: 0,
            stream_id: 1,
        };
        let mut buf = [0u8; HEADER_SIZE];
        header.encode(&mut buf);
        let decoded = Header::decode(&buf).unwrap();
        assert_eq!(decoded.length, MAX_PAYLOAD_LENGTH);
    }

    #[test]
    fn header_unknown_type_returns_error() {
        // Encode manually with type = 0xF
        let buf = [0x00, 0x00, 0x00, 0xF0, 0x00, 0x00, 0x00, 0x01];
        let result = Header::decode(&buf);
        assert!(result.is_err());
        match result.unwrap_err() {
            FrameError::UnknownFrameType { type_val, .. } => {
                assert_eq!(type_val, 0xF);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ── DATA frame ──────────────────────────────────────────────────────

    #[test]
    fn data_frame_encode_decode() {
        let frame = DataFrame::new(1, DataFlags::SYN, Bytes::from("hello"));
        let encoded = frame.encode();
        let (decoded, consumed) = decode_frame(&encoded).unwrap();
        assert_eq!(consumed, HEADER_SIZE + 5);
        match decoded {
            Frame::Data(df) => {
                assert_eq!(df.stream_id, 1);
                assert!(df.flags.contains(DataFlags::SYN));
                assert!(!df.flags.contains(DataFlags::FIN));
                assert_eq!(df.body, Bytes::from("hello"));
            }
            other => panic!("expected Data frame, got {other:?}"),
        }
    }

    #[test]
    fn data_frame_empty_body_with_fin() {
        let frame = DataFrame::new(3, DataFlags::FIN, Bytes::new());
        let encoded = frame.encode();
        let (decoded, consumed) = decode_frame(&encoded).unwrap();
        assert_eq!(consumed, HEADER_SIZE);
        match decoded {
            Frame::Data(df) => {
                assert_eq!(df.stream_id, 3);
                assert!(df.flags.contains(DataFlags::FIN));
                assert!(!df.flags.contains(DataFlags::SYN));
                assert!(df.body.is_empty());
            }
            other => panic!("expected Data frame, got {other:?}"),
        }
    }

    #[test]
    fn data_frame_syn_and_fin() {
        let flags = DataFlags::SYN.union(DataFlags::FIN);
        let frame = DataFrame::new(5, flags, Bytes::from("x"));
        let encoded = frame.encode();
        let (decoded, _) = decode_frame(&encoded).unwrap();
        match decoded {
            Frame::Data(df) => {
                assert!(df.flags.contains(DataFlags::SYN));
                assert!(df.flags.contains(DataFlags::FIN));
            }
            other => panic!("expected Data frame, got {other:?}"),
        }
    }

    // ── RST frame ───────────────────────────────────────────────────────

    #[test]
    fn rst_frame_encode_decode() {
        let frame = RstFrame::new(7, ErrorCode::StreamReset);
        let encoded = frame.encode();
        let (decoded, consumed) = decode_frame(&encoded).unwrap();
        assert_eq!(consumed, HEADER_SIZE + 4);
        match decoded {
            Frame::Rst(rf) => {
                assert_eq!(rf.stream_id, 7);
                assert_eq!(rf.error_code, ErrorCode::StreamReset as u32);
            }
            other => panic!("expected Rst frame, got {other:?}"),
        }
    }

    #[test]
    fn rst_frame_wrong_size_rejected() {
        // Manually craft RST header with length=3 (should be 4)
        let header = Header {
            length: 3,
            frame_type: FrameType::Rst,
            flags: 0,
            stream_id: 1,
        };
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 3);
        let mut hdr = [0u8; HEADER_SIZE];
        header.encode(&mut hdr);
        buf.put_slice(&hdr);
        buf.put_slice(&[0x00, 0x00, 0x00]);
        let result = decode_frame(&buf);
        assert!(matches!(result, Err(FrameError::FrameSize { .. })));
    }

    // ── WNDINC frame ───────────────────────────────────────────────────

    #[test]
    fn wndinc_frame_encode_decode() {
        let frame = WndIncFrame::new(9, 65536);
        let encoded = frame.encode();
        let (decoded, consumed) = decode_frame(&encoded).unwrap();
        assert_eq!(consumed, HEADER_SIZE + 4);
        match decoded {
            Frame::WndInc(wf) => {
                assert_eq!(wf.stream_id, 9);
                assert_eq!(wf.increment, 65536);
            }
            other => panic!("expected WndInc frame, got {other:?}"),
        }
    }

    #[test]
    fn wndinc_masks_top_bit() {
        let frame = WndIncFrame::new(1, 0xFFFF_FFFF);
        assert_eq!(frame.increment, 0x7FFF_FFFF);
    }

    #[test]
    fn wndinc_zero_increment_rejected() {
        // Manually craft WNDINC with increment=0
        let header = Header {
            length: 4,
            frame_type: FrameType::WndInc,
            flags: 0,
            stream_id: 1,
        };
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4);
        let mut hdr = [0u8; HEADER_SIZE];
        header.encode(&mut hdr);
        buf.put_slice(&hdr);
        buf.put_u32(0);
        let result = decode_frame(&buf);
        assert!(matches!(result, Err(FrameError::InvalidWindowIncrement)));
    }

    // ── GOAWAY frame ────────────────────────────────────────────────────

    #[test]
    fn goaway_frame_encode_decode() {
        let frame = GoAwayFrame::new(10, ErrorCode::NoError, Bytes::from("bye"));
        let encoded = frame.encode();
        let (decoded, consumed) = decode_frame(&encoded).unwrap();
        assert_eq!(consumed, HEADER_SIZE + 8 + 3);
        match decoded {
            Frame::GoAway(gf) => {
                assert_eq!(gf.last_stream_id, 10);
                assert_eq!(gf.error_code, ErrorCode::NoError as u32);
                assert_eq!(gf.debug_data, Bytes::from("bye"));
            }
            other => panic!("expected GoAway frame, got {other:?}"),
        }
    }

    #[test]
    fn goaway_frame_no_debug_data() {
        let frame = GoAwayFrame::new(0, ErrorCode::InternalError, Bytes::new());
        let encoded = frame.encode();
        let (decoded, _) = decode_frame(&encoded).unwrap();
        match decoded {
            Frame::GoAway(gf) => {
                assert_eq!(gf.last_stream_id, 0);
                assert_eq!(gf.error_code, ErrorCode::InternalError as u32);
                assert!(gf.debug_data.is_empty());
            }
            other => panic!("expected GoAway frame, got {other:?}"),
        }
    }

    #[test]
    fn goaway_frame_too_short_body() {
        // GOAWAY requires at least 8 bytes body
        let header = Header {
            length: 4,
            frame_type: FrameType::GoAway,
            flags: 0,
            stream_id: 0,
        };
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + 4);
        let mut hdr = [0u8; HEADER_SIZE];
        header.encode(&mut hdr);
        buf.put_slice(&hdr);
        buf.put_u32(0);
        let result = decode_frame(&buf);
        assert!(matches!(result, Err(FrameError::FrameSize { .. })));
    }

    // ── DataFlags ───────────────────────────────────────────────────────

    #[test]
    fn data_flags_none() {
        assert!(!DataFlags::NONE.contains(DataFlags::FIN));
        assert!(!DataFlags::NONE.contains(DataFlags::SYN));
    }

    #[test]
    fn data_flags_union() {
        let flags = DataFlags::FIN.union(DataFlags::SYN);
        assert!(flags.contains(DataFlags::FIN));
        assert!(flags.contains(DataFlags::SYN));
        assert_eq!(flags.bits(), 0x3);
    }

    // ── Incomplete buffer ───────────────────────────────────────────────

    #[test]
    fn decode_too_short_for_header() {
        let result = decode_frame(&[0x00, 0x00]);
        assert!(matches!(result, Err(FrameError::FrameSize { .. })));
    }

    #[test]
    fn decode_too_short_for_payload() {
        let frame = DataFrame::new(1, DataFlags::NONE, Bytes::from("hello"));
        let encoded = frame.encode();
        // Truncate the payload
        let result = decode_frame(&encoded[..HEADER_SIZE + 2]);
        assert!(matches!(result, Err(FrameError::FrameSize { .. })));
    }

    // ── Frame stream_id helper ──────────────────────────────────────────

    #[test]
    fn frame_stream_id() {
        let data = Frame::Data(DataFrame::new(1, DataFlags::NONE, Bytes::new()));
        assert_eq!(data.stream_id(), 1);

        let rst = Frame::Rst(RstFrame {
            stream_id: 5,
            error_code: 0,
        });
        assert_eq!(rst.stream_id(), 5);

        let wndinc = Frame::WndInc(WndIncFrame {
            stream_id: 3,
            increment: 100,
        });
        assert_eq!(wndinc.stream_id(), 3);

        let goaway = Frame::GoAway(GoAwayFrame {
            last_stream_id: 10,
            error_code: 0,
            debug_data: Bytes::new(),
        });
        assert_eq!(goaway.stream_id(), 0);
    }
}
