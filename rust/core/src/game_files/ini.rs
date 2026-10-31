//! Finding and replacing one key's value in an INI file's text, UTF-8 or UTF-16,
//! every other byte left as it was.
use super::check_text;
use anyhow::{Result, anyhow, bail};
use std::ops::Range;

/// A code unit of a text file: a byte (UTF-8, or another encoding that keeps ASCII as it is)
/// or a UTF-16 unit.
trait Unit: Copy {
    fn code(self) -> u32;
    fn text(units: &[Self]) -> String;
}
impl Unit for u8 {
    fn code(self) -> u32 {
        u32::from(self)
    }
    fn text(units: &[Self]) -> String {
        String::from_utf8_lossy(units).into_owned()
    }
}
impl Unit for u16 {
    fn code(self) -> u32 {
        u32::from(self)
    }
    fn text(units: &[Self]) -> String {
        String::from_utf16_lossy(units)
    }
}

fn blank(code: u32) -> bool {
    code == u32::from(b' ') || code == u32::from(b'\t')
}

fn trim<T: Unit>(text: &[T], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && blank(text[range.start].code()) {
        range.start += 1;
    }
    while range.end > range.start && blank(text[range.end - 1].code()) {
        range.end -= 1;
    }
    range
}

fn find<T: Unit>(text: &[T], range: Range<usize>, wanted: u8) -> Option<usize> {
    range
        .into_iter()
        .find(|&at| text[at].code() == u32::from(wanted))
}

/// Where the value of `key` in `[section]` is: the first such key, blanks around the value
/// left out. An empty section stands for the keys before the first header.
fn locate<T: Unit>(text: &[T], section: &str, key: &str) -> Option<Range<usize>> {
    let (section, key) = (section.trim(), key.trim());
    let mut inside = section.is_empty();
    let mut start = 0;
    while start < text.len() {
        let newline = find(text, start..text.len(), b'\n').unwrap_or(text.len());
        let mut end = newline;
        if end > start && text[end - 1].code() == u32::from(b'\r') {
            end -= 1;
        }
        let line = trim(text, start..end);
        if !line.is_empty() {
            let first = text[line.start].code();
            if first == u32::from(b';') || first == u32::from(b'#') {
                // A comment.
            } else if first == u32::from(b'[') {
                if let Some(close) = find(text, line.start + 1..line.end, b']') {
                    let name = trim(text, line.start + 1..close);
                    inside = T::text(&text[name]).eq_ignore_ascii_case(section);
                }
            } else if inside && let Some(equals) = find(text, line.clone(), b'=') {
                let name = trim(text, line.start..equals);
                if T::text(&text[name]).eq_ignore_ascii_case(key) {
                    return Some(trim(text, equals + 1..line.end));
                }
            }
        }
        start = newline + 1;
    }
    None
}

pub(super) const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Whether the file is UTF-16 by its byte order mark, and which byte order.
fn wide(bytes: &[u8]) -> Option<bool> {
    match bytes {
        [0xFF, 0xFE, ..] => Some(false),
        [0xFE, 0xFF, ..] => Some(true),
        _ => None,
    }
}

fn units16(bytes: &[u8], big_endian: bool) -> Result<Vec<u16>> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        bail!("the file is not whole UTF-16 text");
    }
    Ok(pairs
        .iter()
        .map(|&pair| {
            if big_endian {
                u16::from_be_bytes(pair)
            } else {
                u16::from_le_bytes(pair)
            }
        })
        .collect())
}

/// The value of `key` in `[section]` of a file's `bytes`, or `None` when it has no such key.
pub fn value_of(bytes: &[u8], section: &str, key: &str) -> Result<Option<String>> {
    match wide(bytes) {
        None => {
            let skip = if bytes.starts_with(UTF8_BOM) { 3 } else { 0 };
            let Some(range) = locate(&bytes[skip..], section, key) else {
                return Ok(None);
            };
            let value = &bytes[skip + range.start..skip + range.end];
            std::str::from_utf8(value)
                .map(|value| Some(value.to_owned()))
                .map_err(|_| anyhow!("the value of {key} is not UTF-8 text"))
        }
        Some(big_endian) => {
            let units = units16(&bytes[2..], big_endian)?;
            let Some(range) = locate(&units, section, key) else {
                return Ok(None);
            };
            String::from_utf16(&units[range])
                .map(Some)
                .map_err(|_| anyhow!("the value of {key} is not UTF-16 text"))
        }
    }
}

/// `bytes` with the value of `key` in `[section]` replaced by `value`, every other byte as it
/// was; `None` when the file has no such key.
pub fn with_value(bytes: &[u8], section: &str, key: &str, value: &str) -> Result<Option<Vec<u8>>> {
    check_text(value, "value")?;
    set_value(bytes, section, key, value)
}

/// [`with_value`] for a value read from the file before: an original goes back as it was, even
/// one longer than a value an entry may set.
pub(super) fn set_value(
    bytes: &[u8],
    section: &str,
    key: &str,
    value: &str,
) -> Result<Option<Vec<u8>>> {
    match wide(bytes) {
        None => {
            let skip = if bytes.starts_with(UTF8_BOM) { 3 } else { 0 };
            let Some(range) = locate(&bytes[skip..], section, key) else {
                return Ok(None);
            };
            let mut out = Vec::with_capacity(bytes.len() + value.len());
            out.extend_from_slice(&bytes[..skip + range.start]);
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(&bytes[skip + range.end..]);
            Ok(Some(out))
        }
        Some(big_endian) => {
            let units = units16(&bytes[2..], big_endian)?;
            let Some(range) = locate(&units, section, key) else {
                return Ok(None);
            };
            let mut out = bytes[..2 + range.start * 2].to_vec();
            for unit in value.encode_utf16() {
                out.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            out.extend_from_slice(&bytes[2 + range.end * 2..]);
            Ok(Some(out))
        }
    }
}
