//! Muxado frame types and codec.

pub mod data;
pub mod goaway;
pub mod header;
pub mod rst;
pub mod wndinc;

pub use data::{DataFlags, DataFrame};
pub use goaway::GoAwayFrame;
pub use header::FrameHeader;
pub use rst::RstFrame;
pub use wndinc::WndIncFrame;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::MuxadoError;

/// Numeric frame type for DATA frames.
pub const TYPE_DATA: u8 = 0;
/// Numeric frame type for RST frames.
pub const TYPE_RST: u8 = 1;
/// Numeric frame type for WNDINC frames.
pub const TYPE_WNDINC: u8 = 2;
/// Numeric frame type for GOAWAY frames.
pub const TYPE_GOAWAY: u8 = 3;

/// A decoded muxado frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// DATA frame carrying stream payload.
    Data(DataFrame),
    /// RST frame resetting a stream.
    Rst(RstFrame),
    /// WNDINC frame granting additional send window.
    WndInc(WndIncFrame),
    /// GOAWAY frame shutting down the session.
    GoAway(GoAwayFrame),
}

/// Read a single frame from `reader`.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Frame, MuxadoError> {
    let mut hdr_buf = [0u8; 8];
    reader.read_exact(&mut hdr_buf).await?;
    let hdr = FrameHeader::decode(&hdr_buf);
    let len = hdr.length as usize;

    let payload = if len > 0 {
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        Bytes::from(buf)
    } else {
        Bytes::new()
    };

    match hdr.frame_type {
        TYPE_DATA => {
            let flags = data::DataFlags::from_bits_truncate(hdr.flags);
            Ok(Frame::Data(DataFrame {
                stream_id: hdr.stream_id,
                flags,
                payload,
            }))
        }
        TYPE_RST => {
            if payload.len() < 4 {
                return Err(MuxadoError::FrameDecodeError(
                    "RST frame too short".to_string(),
                ));
            }
            let code = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(Frame::Rst(RstFrame {
                stream_id: hdr.stream_id,
                error_code: crate::ErrorCode::from_u32(code),
            }))
        }
        TYPE_WNDINC => {
            if payload.len() < 4 {
                return Err(MuxadoError::FrameDecodeError(
                    "WNDINC frame too short".to_string(),
                ));
            }
            let inc = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(Frame::WndInc(WndIncFrame {
                stream_id: hdr.stream_id,
                increment: inc,
            }))
        }
        TYPE_GOAWAY => {
            if payload.len() < 8 {
                return Err(MuxadoError::FrameDecodeError(
                    "GOAWAY frame too short".to_string(),
                ));
            }
            let last_stream_id =
                u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let code = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let debug_data = payload.slice(8..);
            if debug_data.len() > goaway::GOAWAY_MAX_DEBUG {
                return Err(MuxadoError::GoAwayDebugTooLarge(debug_data.len()));
            }
            Ok(Frame::GoAway(GoAwayFrame {
                last_stream_id,
                error_code: crate::ErrorCode::from_u32(code),
                debug_data,
            }))
        }
        other => Err(MuxadoError::FrameDecodeError(format!(
            "unknown frame type: {other}"
        ))),
    }
}

/// Write a single frame to `writer`.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &Frame,
) -> Result<(), MuxadoError> {
    match frame {
        Frame::Data(df) => {
            let hdr = FrameHeader {
                length: df.payload.len() as u32,
                frame_type: TYPE_DATA,
                flags: df.flags.bits(),
                stream_id: df.stream_id,
            };
            writer.write_all(&hdr.encode()).await?;
            if !df.payload.is_empty() {
                writer.write_all(&df.payload).await?;
            }
        }
        Frame::Rst(rf) => {
            let payload = rf.error_code.as_u32().to_be_bytes();
            let hdr = FrameHeader {
                length: 4,
                frame_type: TYPE_RST,
                flags: 0,
                stream_id: rf.stream_id,
            };
            writer.write_all(&hdr.encode()).await?;
            writer.write_all(&payload).await?;
        }
        Frame::WndInc(wf) => {
            let payload = wf.increment.to_be_bytes();
            let hdr = FrameHeader {
                length: 4,
                frame_type: TYPE_WNDINC,
                flags: 0,
                stream_id: wf.stream_id,
            };
            writer.write_all(&hdr.encode()).await?;
            writer.write_all(&payload).await?;
        }
        Frame::GoAway(gf) => {
            let mut payload = Vec::with_capacity(8 + gf.debug_data.len());
            payload.extend_from_slice(&gf.last_stream_id.to_be_bytes());
            payload.extend_from_slice(&gf.error_code.as_u32().to_be_bytes());
            payload.extend_from_slice(&gf.debug_data);
            let hdr = FrameHeader {
                length: payload.len() as u32,
                frame_type: TYPE_GOAWAY,
                flags: 0,
                stream_id: 0,
            };
            writer.write_all(&hdr.encode()).await?;
            writer.write_all(&payload).await?;
        }
    }
    writer.flush().await?;
    Ok(())
}
