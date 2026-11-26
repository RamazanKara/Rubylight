//! Launch warnings: one non-fatal setup fallback that a successful `/launch` or `/resume` reports
//! to the client as `<clientWarning>` (a code) and `<clientWarningMessage>` (plain text). Older
//! clients ignore the tags. The rules match the client's parser
//! (`client/protocol/client/src/http/launch_warning.rs`), so either side can trust the other's
//! output. The element names and codes match the launch-warning response of Vibepollo (Nonary's
//! Sunshine fork, GPL-3.0) and its moonlight-qt client for interoperability; the code is written
//! independently, none is taken from them.

use crate::session::Warning;

/// The longest message, in UTF-8 bytes, after cleaning; a longer one is dropped, not cut.
pub const MAX_MESSAGE_BYTES: usize = 512;
/// Line breaks kept in a message; later ones become spaces.
pub const MAX_LINE_BREAKS: usize = 3;
/// The longest code: lower-case ASCII letters, digits and `_`.
pub const MAX_CODE_BYTES: usize = 64;

/// The virtual display driver was unavailable, so the stream shows a physical display.
pub const VIRTUAL_DISPLAY_UNAVAILABLE: &str = "virtual_display_unavailable";
/// The client asked for the physical display over the host's virtual display setting.
pub const VIRTUAL_DISPLAY_DISABLED_BY_CLIENT: &str = "virtual_display_disabled_by_client";

/// Session warning codes a launch reports, first match wins, with the code sent for each.
const SETUP_WARNINGS: &[(&str, &str)] = &[
    ("display_virtual", VIRTUAL_DISPLAY_UNAVAILABLE),
    ("display_second_screen", "display_second_screen"),
    ("display_scale", "display_scale"),
    ("display_layout", "display_layout"),
];

/// The launch's warning: the first setup fallback among the session's warnings, cleaned.
pub fn pick(warnings: &[Warning]) -> Option<(String, String)> {
    SETUP_WARNINGS.iter().find_map(|(session, wire)| {
        let warning = warnings.iter().find(|w| w.code == *session)?;
        Some((code(wire)?, message(&warning.message)?))
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

/// Plain text fit to show: control and format characters removed (except ZWJ and ZWNJ, which
/// some scripts need), `\r`, tabs and Unicode line and paragraph separators normalised, at most
/// three line breaks, trimmed. Text with markup (`<` or `>`), empty text and text over 512 bytes
/// are rejected.
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
mod tests {
    use super::*;

    fn warning(code: &str, message: &str) -> Warning {
        Warning {
            code: code.into(),
            message: message.into(),
        }
    }

    #[test]
    fn picks_the_first_setup_fallback_in_priority_order() {
        let warnings = [
            warning("audio_route", "not a setup fallback"),
            warning("display_scale", "Scale lowered to 150%."),
            warning("display_virtual", "Using a physical display."),
        ];
        assert_eq!(
            pick(&warnings),
            Some((
                VIRTUAL_DISPLAY_UNAVAILABLE.into(),
                "Using a physical display.".into()
            ))
        );
        assert_eq!(
            pick(&warnings[..2]),
            Some(("display_scale".into(), "Scale lowered to 150%.".into()))
        );
        assert_eq!(pick(&warnings[..1]), None);
        assert_eq!(pick(&[warning("display_virtual", "<b>x</b>")]), None);
    }

    #[test]
    fn codes_are_lower_snake_case_ascii_only() {
        assert_eq!(code(" display_scale ").as_deref(), Some("display_scale"));
        for bad in ["", "Display", "a-b", "a b", "ä", "x<y", &"a".repeat(65)] {
            assert_eq!(code(bad), None, "{bad}");
        }
        assert!(code(&"a".repeat(64)).is_some());
    }

    #[test]
    fn hostile_messages_are_cleaned_or_dropped() {
        assert_eq!(message("  plain  ").as_deref(), Some("plain"));
        assert_eq!(message("<script>alert(1)</script>"), None);
        assert_eq!(message("a > b"), None);
        assert_eq!(message(" \t\r\n\u{200B} "), None);
        assert_eq!(
            message("a\u{0}b\u{7}c\u{1B}[31md").as_deref(),
            Some("abc[31md")
        );
        assert_eq!(
            message("rtl\u{202E}gnp.exe\u{2066}x\u{FEFF}y\u{AD}z").as_deref(),
            Some("rtlgnp.exexyz")
        );
        assert_eq!(
            message("\u{E0041}tag\u{110BD}\u{1D173}").as_deref(),
            Some("tag")
        );
        assert_eq!(
            message("zw\u{200D}j zw\u{200C}nj").as_deref(),
            Some("zw\u{200D}j zw\u{200C}nj"),
            "ZWJ and ZWNJ are kept"
        );
        assert_eq!(message("a\tb").as_deref(), Some("a b"));
        assert_eq!(
            message("1\r\n2\r3\u{2028}4\u{2029}5\n6").as_deref(),
            Some("1\n2\n3\n4 5 6")
        );
        let limit = "é".repeat(MAX_MESSAGE_BYTES / 2);
        assert_eq!(message(&limit).as_deref(), Some(limit.as_str()));
        assert_eq!(message(&format!("{limit}x")), None);
        assert_eq!(message(&"x".repeat(1 << 20)), None);
        assert_eq!(
            message(&format!("{}ok", "\u{200B}".repeat(10_000))).as_deref(),
            Some("ok"),
            "removed characters do not count toward the limit"
        );
    }

    #[test]
    fn control_and_format_ranges_spare_ordinary_text() {
        for c in ['a', 'é', 'ß', 'ア', '😀', '\u{200C}', '\u{200D}', ' '] {
            assert!(!is_control_or_format(c), "{c:?}");
        }
        for c in [
            '\u{0}', '\u{9F}', '\u{200B}', '\u{200F}', '\u{2060}', '\u{FEFF}',
        ] {
            assert!(is_control_or_format(c), "{c:?}");
        }
    }
}
