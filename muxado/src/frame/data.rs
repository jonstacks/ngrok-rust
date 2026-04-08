//! DATA frame.

use bitflags::bitflags;
use bytes::Bytes;

bitflags! {
    /// Flags for DATA frames.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DataFlags: u8 {
        /// FIN: half-close the write side of the stream.
        const FIN = 0x1;
        /// SYN: open (create) a new stream.
        const SYN = 0x2;
    }
}

/// A DATA frame carrying stream payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFrame {
    /// Stream identifier.
    pub stream_id: u32,
    /// Frame flags (SYN / FIN).
    pub flags: DataFlags,
    /// Payload bytes.
    pub payload: Bytes,
}
