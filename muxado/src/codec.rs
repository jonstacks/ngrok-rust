use std::io::Write;

use bytes::{
    Buf,
    BufMut,
    BytesMut,
};
use tokio_util::codec::{
    Decoder,
    Encoder,
};
use tracing::instrument;

use super::{
    errors::InvalidHeader,
    frame::*,
};

/// Codec for muxado frames.
#[derive(Default, Debug)]
pub struct FrameCodec {
    // the header has to be read to know how big a frame is.
    // We'll decode it once when we have enough bytes, and then wait for the
    // rest, keeping the already-decoded header around in the meantime to avoid
    // decoding it repeatedly.
    input_header: Option<Header>,
}

#[instrument(level = "trace")]
fn decode_header(mut bs: BytesMut) -> Header {
    let length_type_flags = bs.get_u32();
    let length = ((length_type_flags & 0xFFFFFF00) >> 8).try_into().unwrap();
    let type_flags = length_type_flags as u8;

    Header {
        length,
        typ: ((type_flags & 0xF0) >> 4).into(),
        flags: Flags::from_bits_truncate(type_flags & 0x0F),
        stream_id: StreamID::mask(bs.get_u32()),
    }
}

fn expect_zero_stream_id(header: Header) -> Result<(), InvalidHeader> {
    if header.stream_id != StreamID::clamp(0) {
        Err(InvalidHeader::NonZeroStreamID(header.stream_id))
    } else {
        Ok(())
    }
}

fn expect_non_zero_stream_id(header: Header) -> Result<(), InvalidHeader> {
    if header.stream_id == StreamID::clamp(0) {
        Err(InvalidHeader::ZeroStreamID)
    } else {
        Ok(())
    }
}

fn expect_length(header: Header, length: Length) -> Result<(), InvalidHeader> {
    if header.length != length {
        Err(InvalidHeader::Length {
            expected: length,
            actual: header.length,
        })
    } else {
        Ok(())
    }
}

fn expect_min_length(header: Header, length: Length) -> Result<(), InvalidHeader> {
    if header.length < length {
        Err(InvalidHeader::MinLength {
            expected: length,
            actual: header.length,
        })
    } else {
        Ok(())
    }
}

#[instrument(level = "trace")]
fn validate_header(header: Header) -> Result<(), InvalidHeader> {
    match header.typ {
        HeaderType::Rst => {
            expect_non_zero_stream_id(header)?;
            expect_length(header, Length::clamp(4))?;
        }
        HeaderType::Data => {
            expect_non_zero_stream_id(header)?;
        }
        HeaderType::WndInc => {
            expect_non_zero_stream_id(header)?;
            expect_length(header, Length::clamp(4))?;
        }
        HeaderType::GoAway => {
            expect_zero_stream_id(header)?;
            expect_min_length(header, Length::clamp(8))?;
        }
        HeaderType::Invalid(t) => return Err(InvalidHeader::Type(t)),
    }

    Ok(())
}

#[instrument(level = "trace")]
fn decode_frame(header: Header, mut body: BytesMut) -> Frame {
    if let Err(error) = validate_header(header) {
        return Frame {
            header,
            body: Body::Invalid {
                error,
                body: body.freeze(),
            },
        };
    }

    Frame {
        header,
        body: match header.typ {
            HeaderType::Rst => Body::Rst(ErrorCode::mask(body.get_u32()).into()),
            HeaderType::Data => Body::Data(body.freeze()),
            HeaderType::WndInc => Body::WndInc(WndInc::mask(body.get_u32())),
            HeaderType::GoAway => Body::GoAway {
                last_stream_id: StreamID::mask(body.get_u32()),
                error: ErrorCode::mask(body.get_u32()).into(),
                message: body.freeze(),
            },
            HeaderType::Invalid(t) => Body::Invalid {
                error: InvalidHeader::Type(t),
                body: body.freeze(),
            },
        },
    }
}

impl Decoder for FrameCodec {
    type Item = Frame;
    type Error = std::io::Error;

    #[instrument(level = "trace")]
    fn decode(&mut self, b: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let header = if let Some(header) = self.input_header {
            header
        } else {
            if b.len() < 8 {
                return Ok(None);
            }

            let header = decode_header(b.split_to(8));
            self.input_header = Some(header);
            header
        };

        if b.len() < *header.length as usize {
            return Ok(None);
        }

        let body_bytes = b.split_to(*header.length as usize);

        // Drop the header to get ready for the next frame.
        self.input_header.take();

        Ok(Some(decode_frame(header, body_bytes)))
    }
}

#[instrument(level = "trace")]
fn encode_header(header: Header, buf: &mut BytesMut) {
    // Pack the type into the upper nibble and flags into the lower.
    let type_flags: u8 = ((u8::from(header.typ) << 4) & 0xF0) | (header.flags.bits() & 0x0F);
    // Pack the 24-bit length and packed type & flags into a u32
    let length_type_flags: u32 = (*header.length << 8 & 0xFFFFFF00) | type_flags as u32;

    buf.put_u32(length_type_flags);
    buf.put_u32(*header.stream_id);
}

#[instrument(level = "trace")]
fn encode_body(body: Body, buf: &mut BytesMut) {
    match body {
        Body::Rst(err) => buf.put_u32(*ErrorCode::from(err)),
        Body::Data(data) => buf.writer().write_all(&data).unwrap(),
        Body::WndInc(inc) => buf.put_u32(*inc),
        Body::GoAway {
            last_stream_id,
            error,
            message,
        } => {
            buf.put_u32(*last_stream_id);
            buf.put_u32(*ErrorCode::from(error));
            buf.writer().write_all(&message).unwrap();
        }
        Body::Invalid { body, .. } => buf.writer().write_all(&body).unwrap(),
    }
}

impl Encoder<Frame> for FrameCodec {
    type Error = std::io::Error;

    #[instrument(level = "trace")]
    fn encode(&mut self, frame: Frame, buf: &mut BytesMut) -> Result<(), std::io::Error> {
        validate_header(frame.header)?;
        encode_header(frame.header, buf);
        encode_body(frame.body, buf);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use bytes::Bytes;

    use super::*;
    use crate::errors::Error;

    fn encode_decode(frame: Frame) -> Frame {
        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        codec
            .encode(frame.clone(), &mut buf)
            .expect("encode should succeed");
        codec
            .decode(&mut buf)
            .expect("decode should not error")
            .expect("should decode a complete frame")
    }

    // -------------------------------------------------------------------------
    // Round-trip tests
    // -------------------------------------------------------------------------

    #[test]
    fn round_trip() {
        let frame = Frame::from(Body::Data(Bytes::from_static(b"Hello, world!")))
            .stream_id(StreamID::clamp(5));
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_data_empty() {
        let frame = Frame::from(Body::Data(Bytes::new())).stream_id(StreamID::clamp(1));
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_data_with_fin() {
        let frame = Frame::from(Body::Data(Bytes::from_static(b"bye")))
            .stream_id(StreamID::clamp(3))
            .fin();
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_data_with_syn() {
        let frame = Frame::from(Body::Data(Bytes::from_static(b"hello")))
            .stream_id(StreamID::clamp(1))
            .syn();
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_data_with_syn_and_fin() {
        // A single frame that opens and immediately half-closes a stream.
        let frame = Frame::from(Body::Data(Bytes::from_static(b"only")))
            .stream_id(StreamID::clamp(1))
            .syn()
            .fin();
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_rst() {
        let frame = Frame::rst(StreamID::clamp(7), Error::StreamClosed);
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_wndinc() {
        let frame = Frame::from(Body::WndInc(WndInc::clamp(4096))).stream_id(StreamID::clamp(9));
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_goaway_empty_message() {
        let frame = Frame::goaway(StreamID::clamp(5), Error::None, Bytes::new());
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_goaway_with_message() {
        let frame = Frame::goaway(
            StreamID::clamp(11),
            Error::Protocol,
            Bytes::from_static(b"invalid frame received"),
        );
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_all_error_codes() {
        use Error::*;
        let errors = [
            None,
            Protocol,
            Internal,
            FlowControl,
            StreamClosed,
            StreamRefused,
            StreamCancelled,
            StreamReset,
            FrameSizeError,
            AcceptQueueFull,
            EnhanceYourCalm,
            RemoteGoneAway,
            StreamsExhausted,
            WriteTimeout,
            SessionClosed,
            PeerEOF,
        ];
        for error in errors {
            let frame = Frame::rst(StreamID::clamp(1), error);
            let decoded = encode_decode(frame.clone());
            assert_eq!(frame, decoded, "round-trip failed for error {:?}", error);
        }
    }

    // -------------------------------------------------------------------------
    // Boundary value tests
    // -------------------------------------------------------------------------

    #[test]
    fn round_trip_max_stream_id() {
        // 0x7FFFFFFF is the max 31-bit stream ID.
        let frame = Frame::from(Body::Data(Bytes::from_static(b"x")))
            .stream_id(StreamID::clamp(0x7FFFFFFF));
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn round_trip_max_wndinc() {
        let frame =
            Frame::from(Body::WndInc(WndInc::clamp(0x7FFFFFFF))).stream_id(StreamID::clamp(1));
        assert_eq!(frame.clone(), encode_decode(frame));
    }

    #[test]
    fn goaway_length_field_is_eight_plus_message() {
        // The length field must equal 8 (fixed fields) + message.len().
        let msg = b"test message";
        let frame = Frame::goaway(StreamID::clamp(3), Error::None, Bytes::from_static(msg));
        assert_eq!(
            *frame.header.length as usize,
            8 + msg.len(),
            "GoAway length should be 8 + message length"
        );
    }

    #[test]
    fn goaway_length_field_is_eight_for_empty_message() {
        let frame = Frame::goaway(StreamID::clamp(0), Error::None, Bytes::new());
        assert_eq!(
            *frame.header.length, 8,
            "GoAway length should be 8 for empty message"
        );
    }

    // -------------------------------------------------------------------------
    // Partial buffer / streaming decode tests
    // -------------------------------------------------------------------------

    #[test]
    fn decode_partial_header_returns_none() {
        let mut codec = FrameCodec::default();
        // Feed only 4 of the 8 header bytes — should return None.
        let mut buf = bytes::BytesMut::from(&[0u8; 4][..]);
        let result = codec.decode(&mut buf).expect("no error");
        assert!(result.is_none(), "should need more data for header");
    }

    #[test]
    fn decode_partial_body_returns_none() {
        // A DATA frame with 10-byte body; supply only 5 body bytes after the header.
        let frame = Frame::from(Body::Data(Bytes::from_static(b"0123456789")))
            .stream_id(StreamID::clamp(1));
        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        codec.encode(frame, &mut buf).unwrap();
        // Remove the last 5 bytes to simulate a partial body.
        let full_len = buf.len();
        buf.truncate(full_len - 5);
        let result = codec.decode(&mut buf).expect("no error");
        assert!(result.is_none(), "should need more data for body");
    }

    #[test]
    fn decode_header_state_preserved_across_calls() {
        // First call with only the 8-byte header should cache it and return None.
        // Second call with the body should complete decoding.
        let frame =
            Frame::from(Body::Data(Bytes::from_static(b"hello"))).stream_id(StreamID::clamp(1));
        let mut full_buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        codec.encode(frame.clone(), &mut full_buf).unwrap();

        // Split into header-only, then body bytes.
        let header_bytes = full_buf.split_to(8);
        let mut header_buf = bytes::BytesMut::from(&header_bytes[..]);
        // After header only: None, but codec has cached the header.
        assert!(codec.decode(&mut header_buf).unwrap().is_none());

        // Now provide the body; codec should complete decoding.
        let mut body_buf = full_buf;
        let decoded = codec.decode(&mut body_buf).unwrap().expect("decoded frame");
        assert_eq!(frame, decoded);
    }

    // -------------------------------------------------------------------------
    // Validation error tests — invalid frames decode to Body::Invalid
    // -------------------------------------------------------------------------

    fn decode_raw(header_bytes: [u8; 8], body_bytes: &[u8]) -> Frame {
        let mut buf = bytes::BytesMut::new();
        buf.extend_from_slice(&header_bytes);
        buf.extend_from_slice(body_bytes);
        let mut codec = FrameCodec::default();
        codec
            .decode(&mut buf)
            .expect("decode should not error")
            .expect("should decode a complete frame")
    }

    fn make_header_bytes(length: u32, typ: u8, flags: u8, stream_id: u32) -> [u8; 8] {
        let length_type_flags: u32 =
            ((length & 0xFFFFFF) << 8) | ((typ as u32) << 4) | (flags as u32);
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&length_type_flags.to_be_bytes());
        out[4..].copy_from_slice(&stream_id.to_be_bytes());
        out
    }

    #[test]
    fn decode_rst_zero_stream_id_gives_invalid() {
        // RST with stream_id=0 violates the protocol.
        let header = make_header_bytes(4, 0 /* RST */, 0, 0);
        let decoded = decode_raw(header, &0u32.to_be_bytes());
        assert!(
            matches!(decoded.body, Body::Invalid { .. }),
            "expected Invalid body, got {:?}",
            decoded.body
        );
    }

    #[test]
    fn decode_rst_wrong_length_gives_invalid() {
        // RST must have exactly 4-byte body; 8 bytes is wrong.
        let header = make_header_bytes(8, 0 /* RST */, 0, 1);
        let decoded = decode_raw(header, &[0u8; 8]);
        assert!(matches!(decoded.body, Body::Invalid { .. }));
    }

    #[test]
    fn decode_wndinc_zero_stream_id_gives_invalid() {
        let header = make_header_bytes(4, 2 /* WndInc */, 0, 0);
        let decoded = decode_raw(header, &0x1000u32.to_be_bytes());
        assert!(matches!(decoded.body, Body::Invalid { .. }));
    }

    #[test]
    fn decode_wndinc_wrong_length_gives_invalid() {
        // WndInc must have exactly 4-byte body.
        let header = make_header_bytes(8, 2 /* WndInc */, 0, 1);
        let decoded = decode_raw(header, &[0u8; 8]);
        assert!(matches!(decoded.body, Body::Invalid { .. }));
    }

    #[test]
    fn decode_goaway_nonzero_stream_id_gives_invalid() {
        // GoAway must have stream_id=0.
        let header = make_header_bytes(8, 3 /* GoAway */, 0, 99);
        let mut body = [0u8; 8];
        body[4..].copy_from_slice(&0u32.to_be_bytes());
        let decoded = decode_raw(header, &body);
        assert!(matches!(decoded.body, Body::Invalid { .. }));
    }

    #[test]
    fn decode_goaway_too_short_gives_invalid() {
        // GoAway body must be at least 8 bytes.
        let header = make_header_bytes(4, 3 /* GoAway */, 0, 0);
        let decoded = decode_raw(header, &0u32.to_be_bytes());
        assert!(matches!(decoded.body, Body::Invalid { .. }));
    }

    #[test]
    fn decode_unknown_frame_type_gives_invalid() {
        // Types 4–14 are not defined; must produce Invalid body.
        for typ in 4u8..=14 {
            let header = make_header_bytes(0, typ, 0, 0);
            let decoded = decode_raw(header, &[]);
            assert!(
                matches!(decoded.body, Body::Invalid { .. }),
                "type {typ} should produce Invalid body"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Encode validation tests — encoding an invalid frame returns an error
    // -------------------------------------------------------------------------

    #[test]
    fn encode_rst_zero_stream_id_errors() {
        // Build an RST with stream_id=0 manually to bypass Frame::rst().
        let frame = Frame {
            header: Header {
                length: Length::clamp(4),
                typ: HeaderType::Rst,
                flags: Flags::empty(),
                stream_id: StreamID::clamp(0),
            },
            body: Body::Rst(Error::None),
        };
        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        assert!(
            codec.encode(frame, &mut buf).is_err(),
            "encoding RST with stream_id=0 should fail"
        );
    }

    #[test]
    fn encode_goaway_empty_message_succeeds() {
        // After the bug fix this must succeed (length=8 >= 8).
        let frame = Frame::goaway(StreamID::clamp(0), Error::None, Bytes::new());
        let mut buf = bytes::BytesMut::new();
        let mut codec = FrameCodec::default();
        assert!(
            codec.encode(frame, &mut buf).is_ok(),
            "encoding GoAway with empty message should succeed"
        );
    }
}
