//! The pairing link a host shows as a QR code:
//! `rubylight://pair?v=1&host=&port=&otp=&pass=&id=&fp=&name=`.
//!
//! It carries what the existing one-time-PIN pairing needs (the PIN and the passphrase it is
//! hashed with), where to reach the host, and the SHA-256 of the host certificate's DER. A
//! client must refuse to pair when the certificate the host presents does not hash to
//! [`PairingUri::cert_sha256`]. Values are percent-encoded UTF-8.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Scheme and path of every pairing link.
pub const PREFIX: &str = "rubylight://pair?";
/// The link version this module reads and writes.
pub const VERSION: u32 = 1;

/// A host's pairing invitation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingUri {
    /// Address the client connects to.
    pub host: String,
    /// The host's HTTP port.
    pub port: u16,
    /// The one-time PIN.
    pub otp: String,
    pub passphrase: String,
    /// The host's `uniqueid`.
    pub unique_id: String,
    /// SHA-256 of the host certificate's DER.
    pub cert_sha256: [u8; 32],
    /// The host's name, for display.
    pub name: String,
}

impl PairingUri {
    pub fn to_uri(&self) -> String {
        let mut out = String::from(PREFIX);
        let _ = write!(out, "v={VERSION}&host=");
        encode(&mut out, &self.host);
        let _ = write!(out, "&port={}&otp=", self.port);
        encode(&mut out, &self.otp);
        out.push_str("&pass=");
        encode(&mut out, &self.passphrase);
        out.push_str("&id=");
        encode(&mut out, &self.unique_id);
        out.push_str("&fp=");
        for byte in self.cert_sha256 {
            let _ = write!(out, "{byte:02x}");
        }
        out.push_str("&name=");
        encode(&mut out, &self.name);
        out
    }

    /// Parses a link. Another scheme, a version other than 1, a missing or empty host, otp or
    /// fp, a port that is 0 or not a number, a fingerprint that is not 64 hex digits, a bad
    /// percent escape, text that is not UTF-8 and a repeated key reject it. Unknown keys are
    /// ignored; `pass`, `id` and `name` may be absent.
    pub fn parse(uri: &str) -> Option<Self> {
        let query = uri.strip_prefix(PREFIX)?;
        let mut pairs: Vec<(&str, String)> = Vec::new();
        for part in query.split('&') {
            let (key, value) = part.split_once('=')?;
            if pairs.iter().any(|(seen, _)| *seen == key) {
                return None;
            }
            pairs.push((key, decode(value)?));
        }
        let take = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.clone())
        };
        let required = |key: &str| take(key).filter(|value| !value.is_empty());
        if required("v")? != "1" {
            return None;
        }
        let port = required("port")?;
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let port = port.parse::<u16>().ok().filter(|&port| port != 0)?;
        Some(Self {
            host: required("host")?,
            port,
            otp: required("otp")?,
            passphrase: take("pass").unwrap_or_default(),
            unique_id: take("id").unwrap_or_default(),
            cert_sha256: fingerprint(&required("fp")?)?,
            name: take("name").unwrap_or_default(),
        })
    }
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
fn encode(out: &mut String, text: &str) {
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let high = hex_digit(*bytes.get(at + 1)?)?;
            let low = hex_digit(*bytes.get(at + 2)?)?;
            out.push((high << 4) | low);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn fingerprint(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0; 32];
    for (index, pair) in bytes.chunks(2).enumerate() {
        out[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(out)
}

fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).map(|digit| digit as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn invitation() -> PairingUri {
        let mut cert_sha256 = [0; 32];
        for (index, byte) in cert_sha256.iter_mut().enumerate() {
            *byte = index as u8 * 8;
        }
        PairingUri {
            host: "192.168.4.2".into(),
            port: 47989,
            otp: "4821".into(),
            passphrase: "a&b=c d".into(),
            unique_id: "0123ABCD".into(),
            cert_sha256,
            name: "Wohnzimmer-PC ä".into(),
        }
    }

    #[test]
    fn links_round_trip_with_encoding() {
        let uri = invitation().to_uri();
        let mut expected = String::from(
            "rubylight://pair?v=1&host=192.168.4.2&port=47989&otp=4821&pass=a%26b%3Dc%20d&id=0123ABCD&fp=",
        );
        for index in 0..32u8 {
            expected.push_str(&format!("{:02x}", index * 8));
        }
        expected.push_str("&name=Wohnzimmer-PC%20%C3%A4");
        assert_eq!(uri, expected);
        assert_eq!(PairingUri::parse(&uri), Some(invitation()));
        let v6 = PairingUri {
            host: "[fe80::1%25eth0]".into(),
            passphrase: String::new(),
            name: "π / 100%".into(),
            ..invitation()
        };
        assert_eq!(PairingUri::parse(&v6.to_uri()), Some(v6));
    }

    #[test]
    fn optional_keys_may_be_missing_and_unknown_ones_are_ignored() {
        let fp = "AB".repeat(32);
        let uri = format!("rubylight://pair?v=1&host=pc&port=1&otp=1234&fp={fp}&extra=x");
        let parsed = PairingUri::parse(&uri).unwrap();
        assert_eq!(parsed.cert_sha256, [0xAB; 32]);
        assert_eq!(
            (
                parsed.passphrase.as_str(),
                parsed.unique_id.as_str(),
                parsed.name.as_str()
            ),
            ("", "", "")
        );
    }

    #[test]
    fn bad_links_are_refused() {
        let good = invitation().to_uri();
        assert!(PairingUri::parse(&good).is_some());
        let fp = format!("fp={}", "00".repeat(32));
        for (from, to) in [
            ("v=1", "v=2"),
            ("v=1&", ""),
            ("otp=4821&", ""),
            ("otp=4821", "otp="),
            ("port=47989", "port=0"),
            ("port=47989", "port=65536"),
            ("port=47989", "port=+80"),
            ("port=47989&", ""),
            ("host=192.168.4.2&", ""),
            ("rubylight://", "https://"),
            ("pass=a%26b", "pass=a%2"),
            ("pass=a%26b", "pass=a%G6b"),
            ("pass=a%26b", "pass=a%FFb"),
            ("&name=", "&otp=1&name="),
        ] {
            let bad = good.replacen(from, to, 1);
            assert_ne!(bad, good, "{from}");
            assert_eq!(PairingUri::parse(&bad), None, "{bad}");
        }
        let fp_start = good.find("fp=").unwrap();
        let without_fp = format!("{}{}", &good[..fp_start], &good[fp_start + 68..]);
        assert_eq!(PairingUri::parse(&without_fp), None, "{without_fp}");
        for bad_fp in [
            "00".repeat(31),
            "00".repeat(33),
            format!("{}zz", "00".repeat(31)),
        ] {
            let bad = format!("{}fp={bad_fp}{}", &good[..fp_start], &good[fp_start + 67..]);
            assert_eq!(PairingUri::parse(&bad), None, "{bad}");
        }
        assert!(PairingUri::parse(&format!(
            "{}{fp}{}",
            &good[..fp_start],
            &good[fp_start + 67..]
        ))
        .is_some());
        assert_eq!(PairingUri::parse("rubylight://pair?v=1&host"), None);
    }
}
