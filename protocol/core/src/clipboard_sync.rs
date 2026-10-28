//! Clipboard sync: what is copied on one side is put on the other side's clipboard, both
//! ways, as control message 0x5508. The frame is Moonlight V+'s and Foundation Sunshine's
//! (moonlight-common-c PR #5 in the V+ fork; `ClipboardSyncManager.kt` and `MoonBridge.java`
//! in https://github.com/qiin2333/moonlight-vplus, GPL-3.0), so a V+ client works with this
//! host unchanged.
//!
//! Frame, little-endian: version u8 (1), kind u8 ([`KIND_TEXT`] UTF-8, [`KIND_PNG`],
//! [`KIND_REF`]), token u32, length u32, then `length` payload bytes; at most
//! [`MAX_FRAME_BYTES`] in all, so it fits one encrypted control packet. The sender picks a
//! fresh token for each frame and drops a frame that comes back with a token it sent
//! recently, so a copy does not bounce. [`KIND_REF`] names a blob on V+'s HTTPS blob routes,
//! which Rubylight does not serve: larger items go by the image clipboard.
//!
//! A Rubylight client asks with the `/launch` and `/resume` query parameter
//! [`LAUNCH_PARAMETER`]`=1`; the host then lists 0x5508 in the stream's `a=x-rl-control`
//! line, and the client starts with [`Frame::hello`] so the host sends the PC's copies from
//! then on. A V+ client sends neither: the host sends to it once its first frame arrives.

use alloc::vec::Vec;

/// Control-stream message type of both directions.
pub const CLIPBOARD_SYNC_MESSAGE_TYPE: u16 = 0x5508;
/// `/launch` and `/resume` query parameter; `1` asks for clipboard sync.
pub const LAUNCH_PARAMETER: &str = "rlClipboardSync";
/// The frame version this crate reads and writes.
pub const VERSION: u8 = 1;
/// Size of a frame before its payload.
pub const HEADER_BYTES: usize = 10;
/// Most bytes in a frame, header included, as V+ caps it.
pub const MAX_FRAME_BYTES: usize = 65500;
/// Most payload bytes in a frame.
pub const MAX_PAYLOAD_BYTES: usize = MAX_FRAME_BYTES - HEADER_BYTES;

/// UTF-8 text.
pub const KIND_TEXT: u8 = 1;
/// A PNG image.
pub const KIND_PNG: u8 = 2;
/// V+'s reference to a blob uploaded over HTTPS, as JSON; not served by Rubylight.
pub const KIND_REF: u8 = 3;

/// Whether a launch parameter value asks for clipboard sync: exactly `1`.
pub fn requested(value: Option<&str>) -> bool {
    value == Some("1")
}

/// One clipboard item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub token: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Empty text: the client syncs from now on, and nothing is copied. V+ never sends one.
    pub fn hello(token: u32) -> Self {
        Self {
            kind: KIND_TEXT,
            token,
            payload: Vec::new(),
        }
    }

    /// Whether this is [`Frame::hello`]'s empty text.
    pub fn is_hello(&self) -> bool {
        self.kind == KIND_TEXT && self.payload.is_empty()
    }

    /// The frame on the wire, or `None` when the payload is over [`MAX_PAYLOAD_BYTES`].
    pub fn encode(&self) -> Option<Vec<u8>> {
        if self.payload.len() > MAX_PAYLOAD_BYTES {
            return None;
        }
        let mut out = Vec::with_capacity(HEADER_BYTES + self.payload.len());
        out.push(VERSION);
        out.push(self.kind);
        out.extend_from_slice(&self.token.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        Some(out)
    }

    /// Parses a frame as V+ does: version 1, and a length within what follows, whose extra
    /// bytes are ignored. Any kind is returned; the caller refuses the ones it can't use.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER_BYTES || data.len() > MAX_FRAME_BYTES || data[0] != VERSION {
            return None;
        }
        let token = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);
        let length = u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
        let payload = data.get(HEADER_BYTES..HEADER_BYTES.checked_add(length)?)?;
        Some(Self {
            kind: data[1],
            token,
            payload: payload.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn frames_match_moonlight_v_plus_byte_for_byte() {
        let frame = Frame {
            kind: KIND_TEXT,
            token: 0x1234_5678,
            payload: b"hi".to_vec(),
        };
        // MoonBridge.sendClipboardData(1, 0x12345678, "hi").
        let wire = [1, 1, 0x78, 0x56, 0x34, 0x12, 2, 0, 0, 0, b'h', b'i'];
        assert_eq!(frame.encode().unwrap(), wire);
        assert_eq!(Frame::decode(&wire), Some(frame));
        assert_eq!(CLIPBOARD_SYNC_MESSAGE_TYPE, 0x5508);
        assert_eq!(MAX_PAYLOAD_BYTES, 65490);
    }

    #[test]
    fn decoding_refuses_what_v_plus_refuses() {
        let png = Frame {
            kind: KIND_PNG,
            token: 7,
            payload: vec![0x89, b'P'],
        };
        let wire = png.encode().unwrap();
        // Short, another version, a length past the end.
        assert_eq!(Frame::decode(&wire[..HEADER_BYTES - 1]), None);
        let mut other = wire.clone();
        other[0] = 2;
        assert_eq!(Frame::decode(&other), None);
        let mut long = wire.clone();
        long[6] = 3;
        assert_eq!(Frame::decode(&long), None);
        let mut huge = wire.clone();
        huge[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Frame::decode(&huge), None);
        // Extra bytes after the payload are ignored, as V+ ignores them.
        let mut padded = wire.clone();
        padded.push(0);
        assert_eq!(Frame::decode(&padded), Some(png));
        // Unknown kinds parse; the receiver decides.
        let mut unknown = wire;
        unknown[1] = 9;
        assert_eq!(Frame::decode(&unknown).unwrap().kind, 9);
    }

    #[test]
    fn the_largest_frame_fits_and_one_byte_more_does_not() {
        let mut frame = Frame {
            kind: KIND_PNG,
            token: 1,
            payload: vec![0; MAX_PAYLOAD_BYTES],
        };
        let wire = frame.encode().unwrap();
        assert_eq!(wire.len(), MAX_FRAME_BYTES);
        assert_eq!(Frame::decode(&wire), Some(frame.clone()));
        frame.payload.push(0);
        assert_eq!(frame.encode(), None);
    }

    #[test]
    fn hello_is_empty_text_and_only_a_one_asks() {
        let hello = Frame::hello(5);
        assert!(hello.is_hello());
        assert_eq!(Frame::decode(&hello.encode().unwrap()), Some(hello));
        assert!(!Frame::decode(&[1, KIND_PNG, 0, 0, 0, 0, 0, 0, 0, 0])
            .unwrap()
            .is_hello());
        assert!(requested(Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some(" 1")] {
            assert!(!requested(value), "{value:?}");
        }
    }
}
