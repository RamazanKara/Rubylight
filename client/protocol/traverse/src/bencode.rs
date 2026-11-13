//! Bencoding (BEP 3), as the DHT's KRPC messages use it. Dictionaries keep their keys sorted,
//! so an encoding is canonical.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

/// Nesting deeper than this is refused: KRPC never needs more than four levels.
const MAX_DEPTH: usize = 16;

impl Value {
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Value::Bytes(bytes.into())
    }
    pub fn dict<'a>(entries: impl IntoIterator<Item = (&'a str, Value)>) -> Self {
        Value::Dict(
            entries
                .into_iter()
                .map(|(key, value)| (key.as_bytes().to_vec(), value))
                .collect(),
        )
    }
    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        match self {
            Value::Dict(entries) => entries.get(key),
            _ => None,
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Value::Int(n) => {
                out.push(b'i');
                out.extend_from_slice(n.to_string().as_bytes());
                out.push(b'e');
            }
            Value::Bytes(bytes) => encode_bytes(bytes, out),
            Value::List(items) => {
                out.push(b'l');
                items.iter().for_each(|item| item.encode_into(out));
                out.push(b'e');
            }
            Value::Dict(entries) => {
                out.push(b'd');
                for (key, value) in entries {
                    encode_bytes(key, out);
                    value.encode_into(out);
                }
                out.push(b'e');
            }
        }
    }
}

pub fn encode_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(bytes.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(bytes);
}

/// Decodes one complete value; trailing bytes are an error.
pub fn decode(input: &[u8]) -> Option<Value> {
    let mut at = 0;
    let value = decode_at(input, &mut at, 0)?;
    (at == input.len()).then_some(value)
}

/// Decodes one value at the start of `input`, and returns the bytes it took: a KRPC reply's
/// `v` is signed as its raw encoding.
pub fn decode_prefix(input: &[u8]) -> Option<(Value, usize)> {
    let mut at = 0;
    decode_at(input, &mut at, 0).map(|value| (value, at))
}

fn number(input: &[u8], at: &mut usize, end: u8) -> Option<i64> {
    let start = *at;
    let stop = start + input.get(start..)?.iter().position(|&byte| byte == end)?;
    let text = std::str::from_utf8(&input[start..stop]).ok()?;
    // Canonical integers only: no leading zeros, no "-0", no plus sign.
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || text == "-0"
    {
        return None;
    }
    *at = stop + 1;
    text.parse().ok()
}

fn decode_at(input: &[u8], at: &mut usize, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    match *input.get(*at)? {
        b'i' => {
            *at += 1;
            number(input, at, b'e').map(Value::Int)
        }
        b'l' => {
            *at += 1;
            let mut items = Vec::new();
            while *input.get(*at)? != b'e' {
                items.push(decode_at(input, at, depth + 1)?);
            }
            *at += 1;
            Some(Value::List(items))
        }
        b'd' => {
            *at += 1;
            let mut entries = BTreeMap::new();
            while *input.get(*at)? != b'e' {
                let key = match decode_at(input, at, depth + 1)? {
                    Value::Bytes(key) => key,
                    _ => return None,
                };
                let value = decode_at(input, at, depth + 1)?;
                entries.insert(key, value);
            }
            *at += 1;
            Some(Value::Dict(entries))
        }
        b'0'..=b'9' => {
            let length = number(input, at, b':')?;
            let length = usize::try_from(length).ok()?;
            let end = at.checked_add(length)?;
            let bytes = input.get(*at..end)?.to_vec();
            *at = end;
            Some(Value::Bytes(bytes))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_canonically() {
        let value = Value::dict([
            ("t", Value::bytes(*b"aa")),
            ("y", Value::bytes(*b"q")),
            ("a", Value::dict([("id", Value::bytes([1u8; 20]))])),
            ("n", Value::Int(-42)),
            (
                "l",
                Value::List(vec![Value::Int(0), Value::bytes(Vec::new())]),
            ),
        ]);
        let encoded = value.encode();
        // Keys come out sorted whatever order they went in.
        assert!(encoded.starts_with(b"d1:ad2:id20:"));
        assert_eq!(decode(&encoded), Some(value));
        assert_eq!(Value::Int(7).encode(), b"i7e");
    }

    #[test]
    fn malformed_input_is_refused() {
        for bad in [
            &b"i01e"[..],
            b"i-0e",
            b"ie",
            b"i1",
            b"5:abc",
            b"d1:ae",
            b"di1ei2ee",
            b"l",
            b"x",
            b"i1ei2e",
            b"99999999999999999999:a",
        ] {
            assert_eq!(decode(bad), None, "{:?}", std::str::from_utf8(bad));
        }
        let deep = "l".repeat(100) + &"e".repeat(100);
        assert_eq!(decode(deep.as_bytes()), None);
    }

    #[test]
    fn a_prefix_reports_its_length() {
        assert_eq!(
            decode_prefix(b"3:abcrest"),
            Some((Value::bytes(*b"abc"), 5))
        );
    }
}
