/// This module contains the definitions for muxado frames.
///
/// The [`codec`] module is responsible converting between their struct and wire
/// format.
use std::{
    cmp,
    fmt,
    ops::{
        Deref,
        RangeInclusive,
    },
};

use bitflags::bitflags;
use bytes::Bytes;

use crate::{
    constrained::*,
    errors::{
        Error,
        InvalidHeader,
    },
};

pub const WNDINC_MAX: u32 = 0x7FFFFFFF;
pub const STREAMID_MAX: u32 = 0x7FFFFFFF;
pub const LENGTH_MAX: u32 = 0x00FFFFFF;
pub const ERROR_MAX: u32 = u32::MAX;

constrained_num!(StreamID, u32, 0..=STREAMID_MAX, clamp, mask);
constrained_num!(WndInc, u32, 0..=WNDINC_MAX, clamp, mask);
constrained_num!(Length, u32, 0..=LENGTH_MAX, clamp);
constrained_num!(ErrorCode, u32, 0..=ERROR_MAX);

bitflags! {
    #[derive(Default)]
    pub struct Flags: u8 {
        const FIN = 0b0001;
        const SYN = 0b0010;
    }
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct Header {
    pub length: Length,
    pub typ: HeaderType,
    pub flags: Flags,
    pub stream_id: StreamID,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Frame {
    pub header: Header,
    pub body: Body,
}

impl From<Body> for Frame {
    fn from(mut other: Body) -> Self {
        let typ = other.header_type();
        other.clamp_len();
        let length = Length::clamp(other.len() as u32);
        Frame {
            header: Header {
                length,
                typ,
                ..Default::default()
            },
            body: other,
        }
    }
}

impl Frame {
    pub fn is_fin(&self) -> bool {
        self.header.flags.contains(Flags::FIN)
    }
    pub fn is_syn(&self) -> bool {
        self.header.flags.contains(Flags::SYN)
    }
    pub fn fin(mut self) -> Frame {
        self.header.flags.set(Flags::FIN, true);
        self
    }
    pub fn syn(mut self) -> Frame {
        self.header.flags.set(Flags::SYN, true);
        self
    }
    pub fn stream_id(mut self, id: StreamID) -> Frame {
        self.header.stream_id = id;
        self
    }

    pub fn rst(stream_id: StreamID, error: Error) -> Frame {
        let length = Length::clamp(4);
        Frame {
            header: Header {
                length,
                stream_id,
                typ: HeaderType::Rst,
                ..Default::default()
            },
            body: Body::Rst(error),
        }
    }
    pub fn goaway(last_stream_id: StreamID, error: Error, mut message: Bytes) -> Frame {
        // GoAway body is 4-byte last_stream_id + 4-byte error_code + variable message.
        // Truncate the message so the total body fits in the 24-bit length field.
        const MAX_MSG_LEN: usize = LENGTH_MAX as usize - 8;
        if message.len() > MAX_MSG_LEN {
            message = message.slice(..MAX_MSG_LEN);
        }
        let length = Length::clamp((8 + message.len()) as u32);
        Frame {
            header: Header {
                length,
                typ: HeaderType::GoAway,
                ..Default::default()
            },
            body: Body::GoAway {
                last_stream_id,
                error,
                message,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderType {
    Rst,
    Data,
    WndInc,
    GoAway,
    Invalid(u8),
}

impl Default for HeaderType {
    fn default() -> Self {
        HeaderType::Invalid(255)
    }
}

impl From<u8> for HeaderType {
    fn from(other: u8) -> HeaderType {
        match other {
            0 => HeaderType::Rst,
            1 => HeaderType::Data,
            2 => HeaderType::WndInc,
            3 => HeaderType::GoAway,
            t => HeaderType::Invalid(t),
        }
    }
}

impl From<HeaderType> for u8 {
    fn from(other: HeaderType) -> u8 {
        match other {
            HeaderType::Rst => 0,
            HeaderType::Data => 1,
            HeaderType::WndInc => 2,
            HeaderType::GoAway => 3,
            HeaderType::Invalid(t) => t,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Body {
    Rst(Error),
    Data(Bytes),
    WndInc(WndInc),
    GoAway {
        last_stream_id: StreamID,
        error: Error,
        message: Bytes,
    },
    Invalid {
        error: InvalidHeader,
        body: Bytes,
    },
}

impl Default for Body {
    fn default() -> Self {
        Body::Data(Default::default())
    }
}

impl Body {
    pub fn header_type(&self) -> HeaderType {
        match self {
            Body::Data(_) => HeaderType::Data,
            Body::GoAway { .. } => HeaderType::GoAway,
            Body::WndInc(_) => HeaderType::WndInc,
            Body::Rst(_) => HeaderType::Rst,
            Body::Invalid { .. } => HeaderType::Invalid(0xff),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Body::Data(bs) => bs.len(),
            Body::GoAway { message, .. } => 8 + message.len(),
            Body::WndInc(_) => 4,
            Body::Rst(_) => 4,
            Body::Invalid { body, .. } => body.len(),
        }
    }

    pub fn clamp_len(&mut self) {
        match self {
            Body::Data(bs) => *bs = bs.slice(0..cmp::min(LENGTH_MAX as usize, bs.len())),
            Body::Invalid { body, .. } => {
                *body = body.slice(0..cmp::min(LENGTH_MAX as usize, body.len()))
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod test {
    use bytes::Bytes;

    use super::*;
    use crate::errors::Error;

    // -------------------------------------------------------------------------
    // Constrained number type tests
    // -------------------------------------------------------------------------

    #[test]
    fn stream_id_clamp_saturates_at_max() {
        // Values above the 31-bit max must be clamped down to STREAMID_MAX.
        assert_eq!(*StreamID::clamp(0x7FFFFFFF), 0x7FFFFFFF);
        assert_eq!(*StreamID::clamp(0x80000000), 0x7FFFFFFF);
        assert_eq!(*StreamID::clamp(u32::MAX), 0x7FFFFFFF);
    }

    #[test]
    fn stream_id_mask_clears_high_bit() {
        // mask() ANDs with the max value (0x7FFFFFFF), so the MSB is cleared.
        assert_eq!(*StreamID::mask(0x80000001), 0x00000001);
        assert_eq!(*StreamID::mask(0xFFFFFFFF), 0x7FFFFFFF);
        assert_eq!(*StreamID::mask(0x7FFFFFFF), 0x7FFFFFFF);
    }

    #[test]
    fn stream_id_zero_is_valid() {
        assert_eq!(*StreamID::clamp(0), 0);
    }

    #[test]
    fn wndinc_clamp_saturates_at_max() {
        assert_eq!(*WndInc::clamp(0x7FFFFFFF), 0x7FFFFFFF);
        assert_eq!(*WndInc::clamp(u32::MAX), 0x7FFFFFFF);
    }

    #[test]
    fn wndinc_mask_clears_high_bit() {
        assert_eq!(*WndInc::mask(0x80000000), 0);
        assert_eq!(*WndInc::mask(0xFFFFFFFF), 0x7FFFFFFF);
    }

    #[test]
    fn length_clamp_saturates_at_max() {
        assert_eq!(*Length::clamp(0x00FFFFFF), 0x00FFFFFF);
        assert_eq!(*Length::clamp(0x01000000), 0x00FFFFFF);
        assert_eq!(*Length::clamp(u32::MAX), 0x00FFFFFF);
    }

    // -------------------------------------------------------------------------
    // Frame builder tests
    // -------------------------------------------------------------------------

    #[test]
    fn frame_fin_sets_flag() {
        let frame = Frame::from(Body::Data(Bytes::new()))
            .stream_id(StreamID::clamp(1))
            .fin();
        assert!(frame.is_fin());
        assert!(!frame.is_syn());
    }

    #[test]
    fn frame_syn_sets_flag() {
        let frame = Frame::from(Body::Data(Bytes::new()))
            .stream_id(StreamID::clamp(1))
            .syn();
        assert!(frame.is_syn());
        assert!(!frame.is_fin());
    }

    #[test]
    fn frame_fin_and_syn_sets_both_flags() {
        let frame = Frame::from(Body::Data(Bytes::new()))
            .stream_id(StreamID::clamp(1))
            .fin()
            .syn();
        assert!(frame.is_fin());
        assert!(frame.is_syn());
    }

    #[test]
    fn frame_rst_has_correct_fields() {
        let id = StreamID::clamp(42);
        let frame = Frame::rst(id, Error::StreamReset);
        assert_eq!(frame.header.typ, HeaderType::Rst);
        assert_eq!(frame.header.stream_id, id);
        assert_eq!(*frame.header.length, 4);
        assert_eq!(frame.body, Body::Rst(Error::StreamReset));
    }

    #[test]
    fn frame_goaway_has_correct_length_with_message() {
        let msg = Bytes::from_static(b"shutdown");
        let frame = Frame::goaway(StreamID::clamp(7), Error::None, msg.clone());
        assert_eq!(
            *frame.header.length as usize,
            8 + msg.len(),
            "GoAway header length should be 8 + message.len()"
        );
        assert_eq!(frame.header.stream_id, StreamID::clamp(0));
        assert_eq!(frame.header.typ, HeaderType::GoAway);
    }

    #[test]
    fn frame_goaway_has_correct_length_without_message() {
        let frame = Frame::goaway(StreamID::clamp(0), Error::RemoteGoneAway, Bytes::new());
        assert_eq!(*frame.header.length, 8);
    }

    // -------------------------------------------------------------------------
    // Body::len() tests
    // -------------------------------------------------------------------------

    #[test]
    fn body_len_data() {
        assert_eq!(Body::Data(Bytes::from_static(b"hello")).len(), 5);
        assert_eq!(Body::Data(Bytes::new()).len(), 0);
    }

    #[test]
    fn body_len_rst() {
        assert_eq!(Body::Rst(Error::None).len(), 4);
    }

    #[test]
    fn body_len_wndinc() {
        assert_eq!(Body::WndInc(WndInc::clamp(100)).len(), 4);
    }

    #[test]
    fn body_len_goaway_empty_message() {
        let body = Body::GoAway {
            last_stream_id: StreamID::clamp(0),
            error: Error::None,
            message: Bytes::new(),
        };
        assert_eq!(body.len(), 8);
    }

    #[test]
    fn body_len_goaway_with_message() {
        let msg = Bytes::from_static(b"error detail");
        let body = Body::GoAway {
            last_stream_id: StreamID::clamp(0),
            error: Error::Protocol,
            message: msg.clone(),
        };
        assert_eq!(body.len(), 8 + msg.len());
    }
}
