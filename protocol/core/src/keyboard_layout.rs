//! The client's keyboard layout, so the host can switch the PC's input language to match
//! (control message 0x553E, client to host).
//!
//! Sent after the stream starts and again whenever the layout changes; the newest wins.
//! Little-endian: version u8, source u8, tag_len u8 (0-64), reserved u8, klid u32 (0 when
//! unknown), then the ASCII tag.

use alloc::string::String;
use alloc::vec::Vec;

/// Control-stream message type of a [`KeyboardLayout`] (client to host).
pub const KEYBOARD_LAYOUT_MESSAGE_TYPE: u16 = 0x553E;
/// Size of a [`KeyboardLayout`] before its tag.
pub const KEYBOARD_LAYOUT_HEADER_BYTES: usize = 8;
/// Longest tag, in bytes.
pub const MAX_TAG_BYTES: usize = 64;
/// Serverinfo element: the host takes layouts.
pub const SERVERINFO_FIELD: &str = "KeyboardLayoutVersion";

const VERSION: u8 = 1;

/// What the tag names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutSource {
    Unknown,
    /// A Windows keyboard layout identifier; `klid` holds it.
    Klid,
    /// A BCP-47 language tag such as `de-DE`.
    Bcp47,
    /// An XKB layout such as `de(nodeadkeys)`.
    Xkb,
}

impl LayoutSource {
    fn wire(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Klid => 1,
            Self::Bcp47 => 2,
            Self::Xkb => 3,
        }
    }

    fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Unknown),
            1 => Some(Self::Klid),
            2 => Some(Self::Bcp47),
            3 => Some(Self::Xkb),
            _ => None,
        }
    }
}

/// The layout the client's keyboard types with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardLayout {
    pub source: LayoutSource,
    /// The Windows layout, such as 0x00000407 for German; 0 when unknown.
    pub klid: u32,
    /// At most 64 printable ASCII bytes.
    pub tag: String,
}

impl KeyboardLayout {
    /// The wire form, or `None` for a tag past 64 bytes or with anything but printable ASCII.
    pub fn encode(&self) -> Option<Vec<u8>> {
        if !tag_valid(self.tag.as_bytes()) {
            return None;
        }
        let mut out = Vec::with_capacity(KEYBOARD_LAYOUT_HEADER_BYTES + self.tag.len());
        out.extend_from_slice(&[VERSION, self.source.wire(), self.tag.len() as u8, 0]);
        out.extend_from_slice(&self.klid.to_le_bytes());
        out.extend_from_slice(self.tag.as_bytes());
        Some(out)
    }

    /// Parses a layout. Version 0, an unknown source, a tag past 64 bytes or not printable
    /// ASCII, and a short message reject it. Bytes past the tag are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < KEYBOARD_LAYOUT_HEADER_BYTES || data[0] == 0 {
            return None;
        }
        let tag = data.get(
            KEYBOARD_LAYOUT_HEADER_BYTES..KEYBOARD_LAYOUT_HEADER_BYTES + usize::from(data[2]),
        )?;
        if !tag_valid(tag) {
            return None;
        }
        Some(Self {
            source: LayoutSource::from_wire(data[1])?,
            klid: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            tag: String::from_utf8(tag.to_vec()).ok()?,
        })
    }
}

fn tag_valid(tag: &[u8]) -> bool {
    tag.len() <= MAX_TAG_BYTES && tag.iter().all(u8::is_ascii_graphic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_round_trip() {
        let german = KeyboardLayout {
            source: LayoutSource::Klid,
            klid: 0x0000_0407,
            tag: "de-DE".into(),
        };
        let wire = german.encode().unwrap();
        assert_eq!(wire[..8], [1, 1, 5, 0, 0x07, 0x04, 0, 0]);
        assert_eq!(&wire[8..], b"de-DE");
        assert_eq!(KeyboardLayout::decode(&wire), Some(german.clone()));
        let mut longer = wire.clone();
        longer[0] = 2;
        longer.extend_from_slice(b"\0\0");
        assert_eq!(KeyboardLayout::decode(&longer), Some(german));
        for layout in [
            KeyboardLayout {
                source: LayoutSource::Xkb,
                klid: 0,
                tag: "de(nodeadkeys)".into(),
            },
            KeyboardLayout {
                source: LayoutSource::Bcp47,
                klid: 0,
                tag: "a".repeat(64),
            },
            KeyboardLayout {
                source: LayoutSource::Unknown,
                klid: 0,
                tag: String::new(),
            },
        ] {
            let wire = layout.encode().unwrap();
            assert_eq!(KeyboardLayout::decode(&wire), Some(layout));
        }
    }

    #[test]
    fn bad_layouts_are_refused() {
        for tag in ["a".repeat(65), "de DE".into(), "dé".into(), "de\n".into()] {
            let layout = KeyboardLayout {
                source: LayoutSource::Bcp47,
                klid: 0,
                tag,
            };
            assert_eq!(layout.encode(), None, "{:?}", layout.tag);
        }
        let wire = KeyboardLayout {
            source: LayoutSource::Bcp47,
            klid: 0,
            tag: "en-US".into(),
        }
        .encode()
        .unwrap();
        let patched = |at: usize, byte: u8| {
            let mut wire = wire.clone();
            wire[at] = byte;
            KeyboardLayout::decode(&wire)
        };
        assert_eq!(patched(0, 0), None, "version 0");
        assert_eq!(patched(1, 4), None, "unknown source");
        assert_eq!(patched(2, 6), None, "tag past the message");
        assert_eq!(patched(8, 0x80), None, "not ASCII");
        assert_eq!(patched(8, b' '), None, "not printable");
        let mut long = alloc::vec![b'a'; KEYBOARD_LAYOUT_HEADER_BYTES + 65];
        long[..8].copy_from_slice(&[1, 2, 65, 0, 0, 0, 0, 0]);
        assert_eq!(KeyboardLayout::decode(&long), None, "tag past 64 bytes");
        assert_eq!(KeyboardLayout::decode(&wire[..7]), None);
    }
}
