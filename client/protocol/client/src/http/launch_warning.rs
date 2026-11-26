//! A host's launch warning: `<clientWarning>` (a code) and `<clientWarningMessage>` (plain text)
//! in a successful `/launch` or `/resume` reply, read strictly so a hostile host can only show
//! a short plain-text notice. A reply that breaks any rule loses its warning, never its launch.
//!
//! The element names and the known codes match the launch-warning support of Nonary's
//! moonlight-qt client and its Vibepollo host (GPL-3.0) for interoperability; this parser is
//! written independently, no code is taken from them. The Rubylight host applies the same rules
//! before it sends one (`rust/core/session/src/launch_warning.rs`).

use super::xml::Element;

/// A reply longer than this carries no warning.
pub const MAX_REPLY_BYTES: usize = 16 * 1024;
/// The longest message, in UTF-8 bytes, after cleaning; a longer one is dropped, not cut.
pub const MAX_MESSAGE_BYTES: usize = 512;
/// Line breaks kept in a message; later ones become spaces.
pub const MAX_LINE_BREAKS: usize = 3;
/// The longest code: lower-case ASCII letters, digits and `_`.
pub const MAX_CODE_BYTES: usize = 64;

pub const CODE: &str = "clientWarning";
pub const MESSAGE: &str = "clientWarningMessage";
/// The virtual display driver was unavailable, so the stream shows a physical display.
pub const VIRTUAL_DISPLAY_UNAVAILABLE: &str = "virtual_display_unavailable";
/// This device asked for the physical display over the host's virtual display setting.
pub const VIRTUAL_DISPLAY_DISABLED_BY_CLIENT: &str = "virtual_display_disabled_by_client";

/// A warning to show once after the stream connects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchWarning {
    /// The host's code: a known one ([`VIRTUAL_DISPLAY_UNAVAILABLE`]) or any future one.
    pub code: String,
    /// The host's message, cleaned; `None` when the host sent none or an unusable one, and the
    /// app shows its own text for a known code.
    pub message: Option<String>,
}

/// The warning in a successful reply (`raw`, already parsed as `root`), or `None`.
pub fn parse(raw: &str, root: &Element) -> Option<LaunchWarning> {
    if raw.len() > MAX_REPLY_BYTES || raw.contains("<!DOCTYPE") || raw.contains("<!ENTITY") {
        return None;
    }
    let codes: Vec<&Element> = root.elements().filter(|e| e.name == CODE).collect();
    let messages: Vec<&Element> = root.elements().filter(|e| e.name == MESSAGE).collect();
    // A message without a code is ignored; a repeated tag voids the warning.
    let [code] = codes.as_slice() else {
        return None;
    };
    if messages.len() > 1 || code.elements().next().is_some() {
        return None;
    }
    let message = match messages.first() {
        Some(element) if element.elements().next().is_some() => return None,
        Some(element) => self::message(&element.text()),
        None => None,
    };
    Some(LaunchWarning {
        code: self::code(&code.text())?,
        message,
    })
}

/// A code made only of `[a-z0-9_]`, 1 to 64 bytes; anything else is rejected.
pub fn code(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (!raw.is_empty()
        && raw.len() <= MAX_CODE_BYTES
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    .then(|| raw.to_owned())
}

/// Plain text fit to show: control and format characters removed (except ZWJ and ZWNJ),
/// `\r`, tabs and Unicode line and paragraph separators normalised, at most three line breaks,
/// trimmed. Text with markup (`<` or `>`), empty text and text over 512 bytes are rejected.
pub fn message(raw: &str) -> Option<String> {
    if raw.contains(['<', '>']) {
        return None;
    }
    let mut text = String::with_capacity(raw.len().min(MAX_MESSAGE_BYTES * 4));
    let mut breaks = 0;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        let c = match c {
            '\r' if chars.peek() == Some(&'\n') => continue,
            '\r' | '\n' | '\u{2028}' | '\u{2029}' => '\n',
            '\t' => ' ',
            c if is_control_or_format(c) => continue,
            c => c,
        };
        if c == '\n' {
            breaks += 1;
            text.push(if breaks <= MAX_LINE_BREAKS { '\n' } else { ' ' });
        } else {
            text.push(c);
        }
        if text.len() > MAX_MESSAGE_BYTES * 4 {
            return None;
        }
    }
    let text = text.trim();
    (!text.is_empty() && text.len() <= MAX_MESSAGE_BYTES).then(|| text.to_owned())
}

/// Unicode general categories Cc and Cf, except ZWNJ (U+200C) and ZWJ (U+200D).
pub fn is_control_or_format(c: char) -> bool {
    matches!(
        c as u32,
        0x00..=0x1F
            | 0x7F..=0x9F
            | 0xAD
            | 0x600..=0x605
            | 0x61C
            | 0x6DD
            | 0x70F
            | 0x890..=0x891
            | 0x8E2
            | 0x180E
            | 0x200B
            | 0x200E..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

#[cfg(test)]
#[path = "launch_warning_tests.rs"]
mod tests;
