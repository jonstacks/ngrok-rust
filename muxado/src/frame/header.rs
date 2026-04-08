//! 8-byte muxado frame header.

/// The fixed-size 8-byte header that precedes every muxado frame.
///
/// Layout (all big-endian):
/// - bytes 0..2 : payload length (24-bit)
/// - byte 3     : `(frame_type << 4) | flags` (4-bit type, 4-bit flags)
/// - bytes 4..7 : stream ID (32-bit, MSB reserved = 0)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Payload length in bytes.
    pub length: u32,
    /// Frame type nibble.
    pub frame_type: u8,
    /// Flags nibble.
    pub flags: u8,
    /// Stream identifier.
    pub stream_id: u32,
}

impl FrameHeader {
    /// Encode the header into an 8-byte buffer.
    pub fn encode(&self) -> [u8; 8] {
        let mut buf = [0u8; 8];
        buf[0] = ((self.length >> 16) & 0xFF) as u8;
        buf[1] = ((self.length >> 8) & 0xFF) as u8;
        buf[2] = (self.length & 0xFF) as u8;
        buf[3] = ((self.frame_type & 0xF) << 4) | (self.flags & 0xF);
        let sid = self.stream_id & 0x7FFF_FFFF;
        buf[4] = ((sid >> 24) & 0xFF) as u8;
        buf[5] = ((sid >> 16) & 0xFF) as u8;
        buf[6] = ((sid >> 8) & 0xFF) as u8;
        buf[7] = (sid & 0xFF) as u8;
        buf
    }

    /// Decode an 8-byte buffer into a header.
    pub fn decode(buf: &[u8; 8]) -> Self {
        let length = ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32);
        let type_flags = buf[3];
        let frame_type = (type_flags >> 4) & 0xF;
        let flags = type_flags & 0xF;
        let stream_id = ((buf[4] as u32) << 24)
            | ((buf[5] as u32) << 16)
            | ((buf[6] as u32) << 8)
            | (buf[7] as u32);
        let stream_id = stream_id & 0x7FFF_FFFF;
        Self { length, frame_type, flags, stream_id }
    }
}
