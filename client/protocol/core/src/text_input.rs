//! Text input context: the host tells the client when a text field on the PC gains or loses
//! focus, so the client can raise its own keyboard with the right layout (control message
//! 0x553B).
//!
//! The client asks with a [`TextInputRequest`]; the host then sends a [`TextInputNotice`] on
//! every raise, lower or change of the focused field, numbered by `serial`, and the client
//! keeps only the newest ([`is_newer`]). Typed text still goes back as stock UTF-8 text
//! input. Surrounding text is sent only when the client asked, the host allows it, and the
//! field is not sensitive.
//!
//! Request, little-endian: version u8, flags u8 (bit 0 notices, bit 1 surrounding text),
//! max_text u16 (0-4096 bytes). Notice: version u8, kind u8, input_type u8, flags u8, serial
//! u32, selection_start u16, selection_end u16 (UTF-16 code units, 0xFFFF unknown), text_len
//! u16, reserved u16, then the UTF-8 text.

use alloc::string::String;
use alloc::vec::Vec;

/// Control-stream message type of both directions.
pub const TEXT_INPUT_MESSAGE_TYPE: u16 = 0x553B;
/// Encoded size of a [`TextInputRequest`].
pub const TEXT_INPUT_REQUEST_BYTES: usize = 4;
/// Size of a [`TextInputNotice`] before its text.
pub const TEXT_INPUT_NOTICE_HEADER_BYTES: usize = 16;
/// Most surrounding text a client may ask for, in bytes.
pub const MAX_TEXT_BYTES: u16 = 4096;
/// A selection end the host does not know.
pub const SELECTION_UNKNOWN: u16 = 0xFFFF;
/// Serverinfo element: the host sends notices.
pub const SERVERINFO_FIELD: &str = "TextInputVersion";

const VERSION: u8 = 1;

/// Client to host: start or stop notices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextInputRequest {
    /// Send raise and lower notices; false stops them.
    pub notices: bool,
    /// The client wants the field's surrounding text.
    pub surrounding_text: bool,
    /// Most text per notice, in bytes; the host cuts at a UTF-8 boundary.
    pub max_text: u16,
}

impl TextInputRequest {
    const NOTICES: u8 = 1;
    const SURROUNDING_TEXT: u8 = 2;

    pub fn encode(&self) -> [u8; TEXT_INPUT_REQUEST_BYTES] {
        let flags = if self.notices { Self::NOTICES } else { 0 }
            | if self.surrounding_text {
                Self::SURROUNDING_TEXT
            } else {
                0
            };
        let max = self.max_text.to_le_bytes();
        [VERSION, flags, max[0], max[1]]
    }

    /// Parses a request; version 0 and more than 4096 bytes of text reject it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < TEXT_INPUT_REQUEST_BYTES || data[0] == 0 {
            return None;
        }
        let max_text = u16::from_le_bytes([data[2], data[3]]);
        (max_text <= MAX_TEXT_BYTES).then_some(Self {
            notices: data[1] & Self::NOTICES != 0,
            surrounding_text: data[1] & Self::SURROUNDING_TEXT != 0,
            max_text,
        })
    }
}

/// What happened to the focused field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// No text field has focus; hide the keyboard.
    Lower,
    /// A text field gained focus; show the keyboard.
    Raise,
    /// The raised field's context changed.
    Update,
}

impl NoticeKind {
    fn wire(self) -> u8 {
        match self {
            Self::Lower => 0,
            Self::Raise => 1,
            Self::Update => 2,
        }
    }

    fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Lower),
            1 => Some(Self::Raise),
            2 => Some(Self::Update),
            _ => None,
        }
    }
}

/// The kind of text the field takes, for the keyboard's layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputType {
    Text,
    Number,
    Decimal,
    Phone,
    Email,
    Url,
    Password,
    Search,
    Multiline,
}

impl InputType {
    const ALL: [Self; 9] = [
        Self::Text,
        Self::Number,
        Self::Decimal,
        Self::Phone,
        Self::Email,
        Self::Url,
        Self::Password,
        Self::Search,
        Self::Multiline,
    ];

    fn wire(self) -> u8 {
        Self::ALL.iter().position(|&kind| kind == self).unwrap_or(0) as u8
    }

    fn from_wire(byte: u8) -> Option<Self> {
        Self::ALL.get(usize::from(byte)).copied()
    }
}

/// Host to client: the focused field's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInputNotice {
    pub kind: NoticeKind,
    pub input_type: InputType,
    /// Never any text for this field.
    pub sensitive: bool,
    pub no_autocorrect: bool,
    pub no_autocapitalize: bool,
    /// Enter submits rather than starting a new line.
    pub enter_action: bool,
    /// One more than the previous notice's.
    pub serial: u32,
    /// UTF-16 code units into `text`, or [`SELECTION_UNKNOWN`].
    pub selection_start: u16,
    pub selection_end: u16,
    /// Surrounding text, empty unless allowed.
    pub text: String,
}

impl TextInputNotice {
    const SENSITIVE: u8 = 1;
    const NO_AUTOCORRECT: u8 = 2;
    const NO_AUTOCAPITALIZE: u8 = 4;
    const ENTER_ACTION: u8 = 8;

    fn text_allowed(&self) -> bool {
        self.text.is_empty()
            || (!self.sensitive
                && self.input_type != InputType::Password
                && self.text.len() <= usize::from(MAX_TEXT_BYTES))
    }

    /// The wire form, or `None` for text in a sensitive or password field or past 4096 bytes.
    pub fn encode(&self) -> Option<Vec<u8>> {
        if !self.text_allowed() {
            return None;
        }
        let flags = [
            (self.sensitive, Self::SENSITIVE),
            (self.no_autocorrect, Self::NO_AUTOCORRECT),
            (self.no_autocapitalize, Self::NO_AUTOCAPITALIZE),
            (self.enter_action, Self::ENTER_ACTION),
        ]
        .iter()
        .filter(|(set, _)| *set)
        .fold(0, |flags, (_, bit)| flags | bit);
        let mut out = Vec::with_capacity(TEXT_INPUT_NOTICE_HEADER_BYTES + self.text.len());
        out.extend_from_slice(&[VERSION, self.kind.wire(), self.input_type.wire(), flags]);
        out.extend_from_slice(&self.serial.to_le_bytes());
        out.extend_from_slice(&self.selection_start.to_le_bytes());
        out.extend_from_slice(&self.selection_end.to_le_bytes());
        out.extend_from_slice(&(self.text.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(self.text.as_bytes());
        Some(out)
    }

    /// Parses a notice. Version 0, an unknown kind or input type, text that is not UTF-8,
    /// past 4096 bytes or in a sensitive or password field, and a short message reject it.
    /// Bytes past the text are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < TEXT_INPUT_NOTICE_HEADER_BYTES || data[0] == 0 {
            return None;
        }
        let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        let text_len = usize::from(u16_at(12));
        let text =
            data.get(TEXT_INPUT_NOTICE_HEADER_BYTES..TEXT_INPUT_NOTICE_HEADER_BYTES + text_len)?;
        let notice = Self {
            kind: NoticeKind::from_wire(data[1])?,
            input_type: InputType::from_wire(data[2])?,
            sensitive: data[3] & Self::SENSITIVE != 0,
            no_autocorrect: data[3] & Self::NO_AUTOCORRECT != 0,
            no_autocapitalize: data[3] & Self::NO_AUTOCAPITALIZE != 0,
            enter_action: data[3] & Self::ENTER_ACTION != 0,
            serial: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            selection_start: u16_at(8),
            selection_end: u16_at(10),
            text: String::from_utf8(text.to_vec()).ok()?,
        };
        notice.text_allowed().then_some(notice)
    }
}

/// Whether `serial` came after `than`, across the u32 wrap.
pub fn is_newer(serial: u32, than: u32) -> bool {
    (serial.wrapping_sub(than) as i32) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice() -> TextInputNotice {
        TextInputNotice {
            kind: NoticeKind::Raise,
            input_type: InputType::Email,
            sensitive: false,
            no_autocorrect: true,
            no_autocapitalize: true,
            enter_action: false,
            serial: 41,
            selection_start: 3,
            selection_end: 3,
            text: "zoë@".into(),
        }
    }

    #[test]
    fn requests_round_trip() {
        let request = TextInputRequest {
            notices: true,
            surrounding_text: true,
            max_text: 4096,
        };
        assert_eq!(request.encode(), [1, 3, 0x00, 0x10]);
        assert_eq!(TextInputRequest::decode(&request.encode()), Some(request));
        let stop = TextInputRequest::default();
        assert_eq!(stop.encode(), [1, 0, 0, 0]);
        assert_eq!(
            TextInputRequest::decode(&[2, 1, 0, 1, 7]),
            Some(TextInputRequest {
                notices: true,
                surrounding_text: false,
                max_text: 256,
            })
        );
        let too_much = TextInputRequest {
            max_text: 4097,
            ..request
        };
        assert_eq!(TextInputRequest::decode(&too_much.encode()), None);
        assert_eq!(TextInputRequest::decode(&[0, 1, 0, 0]), None);
        assert_eq!(TextInputRequest::decode(&[1, 1, 0]), None);
    }

    #[test]
    fn notices_round_trip() {
        let wire = notice().encode().unwrap();
        assert_eq!(wire.len(), TEXT_INPUT_NOTICE_HEADER_BYTES + 5);
        assert_eq!(wire[..4], [1, 1, 4, 6]);
        assert_eq!(wire[4..8], 41u32.to_le_bytes());
        assert_eq!(wire[12..16], [5, 0, 0, 0]);
        assert_eq!(TextInputNotice::decode(&wire), Some(notice()));
        let mut longer = wire.clone();
        longer[0] = 2;
        longer.extend_from_slice(b"tail");
        assert_eq!(TextInputNotice::decode(&longer), Some(notice()));
        assert_eq!(TextInputNotice::decode(&wire[..wire.len() - 1]), None);

        for (wire, input_type) in InputType::ALL.into_iter().enumerate() {
            let notice = TextInputNotice {
                input_type,
                text: String::new(),
                ..notice()
            };
            let encoded = notice.encode().unwrap();
            assert_eq!(encoded[2], wire as u8);
            assert_eq!(TextInputNotice::decode(&encoded), Some(notice));
        }
        let lower = TextInputNotice {
            kind: NoticeKind::Lower,
            sensitive: true,
            enter_action: true,
            selection_start: SELECTION_UNKNOWN,
            selection_end: SELECTION_UNKNOWN,
            text: String::new(),
            ..notice()
        };
        let encoded = lower.encode().unwrap();
        assert_eq!(encoded[1], 0);
        assert_eq!(encoded[3], 1 | 2 | 4 | 8);
        assert_eq!(TextInputNotice::decode(&encoded), Some(lower));
    }

    #[test]
    fn private_or_bad_notices_are_refused() {
        let password = TextInputNotice {
            input_type: InputType::Password,
            ..notice()
        };
        let sensitive = TextInputNotice {
            sensitive: true,
            ..notice()
        };
        let long = TextInputNotice {
            text: "a".repeat(4097),
            input_type: InputType::Multiline,
            ..notice()
        };
        for refused in [&password, &sensitive, &long] {
            assert_eq!(refused.encode(), None, "{:?}", refused.input_type);
        }
        assert!(TextInputNotice {
            text: "a".repeat(4096),
            ..notice()
        }
        .encode()
        .is_some());
        // The same refusals on the way in.
        let wire = notice().encode().unwrap();
        let patched = |at: usize, byte: u8| {
            let mut wire = wire.clone();
            wire[at] = byte;
            TextInputNotice::decode(&wire)
        };
        assert_eq!(patched(2, 6), None, "password with text");
        assert_eq!(patched(3, 1), None, "sensitive with text");
        assert_eq!(patched(0, 0), None, "version 0");
        assert_eq!(patched(1, 3), None, "unknown kind");
        assert_eq!(patched(2, 9), None, "unknown input type");
        assert_eq!(patched(16 + 2, 0xFF), None, "not UTF-8");
        let mut huge = vec_of(TEXT_INPUT_NOTICE_HEADER_BYTES + 4097);
        huge[..16].copy_from_slice(&wire[..16]);
        huge[12..14].copy_from_slice(&4097u16.to_le_bytes());
        assert_eq!(TextInputNotice::decode(&huge), None, "past 4096 bytes");
    }

    fn vec_of(len: usize) -> Vec<u8> {
        alloc::vec![b'a'; len]
    }

    #[test]
    fn newer_serials_win_across_the_wrap() {
        assert!(is_newer(2, 1));
        assert!(!is_newer(1, 1));
        assert!(!is_newer(1, 2));
        assert!(is_newer(0, u32::MAX));
        assert!(is_newer(5, u32::MAX - 5));
        assert!(!is_newer(u32::MAX, 0));
    }
}
