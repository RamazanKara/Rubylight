//! Valve's KeyValues documents: text VDF and the binary `appinfo.vdf`.
use std::collections::{BTreeMap, BTreeSet};

/// A KeyValues document (Valve's text or binary VDF).
#[derive(Clone, Debug, PartialEq)]
pub enum Vdf {
    Text(String),
    Object(Vec<(String, Vdf)>),
}
impl Vdf {
    /// Parse text VDF leniently, as Vibepollo does: a stray brace or a
    /// missing value never stops the rest of the file.
    pub fn parse(text: &str) -> Self {
        let mut lexer = Lexer {
            text: text.as_bytes(),
            at: 0,
        };
        Self::Object(lexer.body(false))
    }
    /// A child by key, ignoring case.
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        self.entries()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    }
    pub fn at(&self, keys: &[&str]) -> Option<&Vdf> {
        keys.iter().try_fold(self, |node, key| node.get(key))
    }
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Object(_) => None,
        }
    }
    pub fn number(&self) -> Option<u64> {
        self.text()?.trim().parse().ok()
    }
    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Self::Object(entries) => entries,
            Self::Text(_) => &[],
        }
    }
}
struct Lexer<'a> {
    text: &'a [u8],
    at: usize,
}
impl Lexer<'_> {
    fn space(&mut self) {
        while let Some(&c) = self.text.get(self.at) {
            if c.is_ascii_whitespace() {
                self.at += 1;
            } else if self.text[self.at..].starts_with(b"//") {
                while self.text.get(self.at).is_some_and(|&c| c != b'\n') {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }
    fn token(&mut self) -> Option<String> {
        self.space();
        match self.text.get(self.at)? {
            b'{' | b'}' => None,
            b'"' => {
                self.at += 1;
                let mut out = vec![];
                while let Some(&c) = self.text.get(self.at) {
                    self.at += 1;
                    match c {
                        b'"' => break,
                        b'\\' if self.at < self.text.len() => {
                            let escaped = self.text[self.at];
                            self.at += 1;
                            out.push(match escaped {
                                b'n' => b'\n',
                                b't' => b'\t',
                                b'r' => b'\r',
                                other => other,
                            });
                        }
                        _ => out.push(c),
                    }
                }
                Some(String::from_utf8_lossy(&out).into_owned())
            }
            _ => {
                let start = self.at;
                while self
                    .text
                    .get(self.at)
                    .is_some_and(|c| !c.is_ascii_whitespace() && !matches!(c, b'{' | b'}'))
                {
                    self.at += 1;
                }
                Some(String::from_utf8_lossy(&self.text[start..self.at]).into_owned())
            }
        }
    }
    fn body(&mut self, nested: bool) -> Vec<(String, Vdf)> {
        let mut entries = vec![];
        loop {
            self.space();
            let Some(&c) = self.text.get(self.at) else {
                return entries;
            };
            if nested && c == b'}' {
                self.at += 1;
                return entries;
            }
            let Some(key) = self.token() else {
                // A stray brace cannot form a pair; skip it.
                self.at += 1;
                continue;
            };
            self.space();
            if self.text.get(self.at) == Some(&b'{') {
                self.at += 1;
                entries.push((key, Vdf::Object(self.body(true))));
            } else if let Some(value) = self.token() {
                entries.push((key, Vdf::Text(value)));
            } else {
                entries.push((key, Vdf::Object(vec![])));
            }
        }
    }
}

/// Read the `common` names and types from Steam's binary app cache
/// (`appcache/appinfo.vdf`, format 28 or 29) for the apps in `wanted`.
pub fn appinfo(data: &[u8], wanted: &BTreeSet<u32>) -> BTreeMap<u32, (String, String)> {
    appinfo_entries(data, wanted)
        .into_iter()
        .map(|(app, info)| {
            let common = |key: &str| {
                info.at(&["common", key])
                    .and_then(Vdf::text)
                    .unwrap_or("")
                    .to_owned()
            };
            (app, (common("name"), common("type")))
        })
        .collect()
}
/// The app cache's `appinfo` object of each app in `wanted`.
pub(super) fn appinfo_entries(data: &[u8], wanted: &BTreeSet<u32>) -> BTreeMap<u32, Vdf> {
    let mut found = BTreeMap::new();
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
    };
    let (keys, end) = match u32_at(0) {
        Some(0x0756_4429) => {
            // Keys are indices into a string table at the end of the file.
            let Some(table) = data
                .get(8..16)
                .and_then(|b| usize::try_from(u64::from_le_bytes(b.try_into().ok()?)).ok())
            else {
                return found;
            };
            let Some(count) = u32_at(table) else {
                return found;
            };
            let mut keys = Vec::with_capacity(count.min(1 << 20) as usize);
            let mut at = table + 4;
            for _ in 0..count {
                let Some(length) = data
                    .get(at..)
                    .and_then(|rest| rest.iter().position(|b| *b == 0))
                else {
                    return found;
                };
                keys.push(String::from_utf8_lossy(&data[at..at + length]).into_owned());
                at += length + 1;
            }
            (Some(keys), table)
        }
        Some(0x0756_4428) => (None, data.len()),
        _ => return found,
    };
    let mut at = if keys.is_some() { 16 } else { 8 };
    while let (Some(app), Some(size)) = (u32_at(at), u32_at(at + 4)) {
        if app == 0 {
            break;
        }
        let entry_end = at + 8 + size as usize;
        if size < 60 || entry_end > end {
            break;
        }
        if wanted.contains(&app) {
            let mut cursor = at + 68;
            if let Some(mut root) = binary_object(data, &mut cursor, entry_end, keys.as_deref()) {
                if let Vdf::Object(entries) = &mut root
                    && let Some(at) = entries.iter().position(|(key, _)| key == "appinfo")
                {
                    root = entries.swap_remove(at).1;
                }
                found.insert(app, root);
            }
        }
        at = entry_end;
    }
    found
}
fn binary_object(data: &[u8], at: &mut usize, end: usize, keys: Option<&[String]>) -> Option<Vdf> {
    let mut entries = vec![];
    let cstring = |at: &mut usize| -> Option<String> {
        let length = data.get(*at..end)?.iter().position(|b| *b == 0)?;
        let text = String::from_utf8_lossy(&data[*at..*at + length]).into_owned();
        *at += length + 1;
        Some(text)
    };
    let fixed = |at: &mut usize, size: usize| -> Option<u64> {
        let bytes = data.get(*at..(*at + size).min(end))?;
        if bytes.len() != size {
            return None;
        }
        *at += size;
        Some(
            bytes
                .iter()
                .rev()
                .fold(0u64, |value, byte| value << 8 | u64::from(*byte)),
        )
    };
    while *at < end {
        let kind = data[*at];
        *at += 1;
        if matches!(kind, 0x08 | 0x0b) {
            return Some(Vdf::Object(entries));
        }
        let key = match keys {
            Some(keys) => keys.get(fixed(at, 4)? as usize)?.clone(),
            None => cstring(at)?,
        };
        let value = match kind {
            0x00 => binary_object(data, at, end, keys)?,
            0x01 => Vdf::Text(cstring(at)?),
            0x02..=0x04 | 0x06 => Vdf::Text(fixed(at, 4)?.to_string()),
            0x07 | 0x0a => Vdf::Text(fixed(at, 8)?.to_string()),
            0x05 => {
                let mut units = vec![];
                loop {
                    let unit = fixed(at, 2)? as u16;
                    if unit == 0 {
                        break;
                    }
                    units.push(unit);
                }
                Vdf::Text(String::from_utf16_lossy(&units))
            }
            _ => return None,
        };
        entries.push((key, value));
    }
    None
}
