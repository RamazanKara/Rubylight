//! The text of sunshine.conf: its encoding, comments and bracketed values.

/// The text of sunshine.conf. Editors save it as UTF-8 with or without a
/// byte order mark, or as UTF-16 with one; anything else is read as the
/// Windows ANSI code page (Windows-1252) rather than refused.
pub(super) fn decode(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| unit(pair))
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return utf16(rest, u16::from_be_bytes);
    }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    tracing::warn!("the configuration is not UTF-8; it is read as Windows-1252");
    // Windows-1252 differs from Latin-1 only in 0x80-0x9F; its five unused
    // codes map to the same control characters, as browsers read them.
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}',
        '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}',
        '\u{178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9f => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}
/// The line without its comment. Quotes protect `#` only when they are
/// balanced on the line; otherwise `#` always starts a comment, as in Vibepollo.
pub(super) fn strip_comment(line: &str) -> String {
    let mut quote = false;
    let mut escaped = false;
    let mut cut = None;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quote => escaped = true,
            '"' => quote = !quote,
            '#' if !quote && cut.is_none() => cut = Some(i),
            _ => {}
        }
    }
    let cut = if quote { line.find('#') } else { cut };
    line[..cut.unwrap_or(line.len())].to_owned()
}
/// Net bracket depth of a line, ignoring brackets in balanced quotes.
pub(super) fn bracket_depth(line: &str) -> i32 {
    let balanced = line.matches('"').count().is_multiple_of(2);
    let (mut depth, mut quote, mut escaped) = (0, false, false);
    for c in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quote => escaped = true,
            '"' if balanced => quote = !quote,
            '[' | '{' if !quote => depth += 1,
            ']' | '}' if !quote => depth -= 1,
            _ => {}
        }
    }
    depth
}
