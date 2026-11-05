//! The pairing link a host shows as a QR code:
//! `rubylight://pair?v=1&host=&port=&otp=&pass=&id=&fp=&name=`.
//!
//! It carries what the existing one-time-PIN pairing needs (the PIN and the passphrase it is
//! hashed with), where to reach the host, and the SHA-256 of the host certificate's DER. A
//! client must refuse to pair when the certificate the host presents does not hash to
//! [`PairingUri::cert_sha256`]. Values are percent-encoded UTF-8.
//!
//! A link can come from any web page, so whatever opens one from outside the app asks first,
//! showing [`PairingUri::shown_name`] and [`PairingUri::shown_address`].

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};

/// Scheme and path of every pairing link.
pub const PREFIX: &str = "rubylight://pair?";
/// The link version this module reads and writes.
pub const VERSION: u32 = 1;
/// The longest link read, in bytes; a real one stays far below it.
pub const MAX_LEN: usize = 4096;
/// The longest address a link may name, as DNS allows.
const MAX_HOST_LEN: usize = 255;
/// How many characters of a link's name are shown.
const MAX_SHOWN_NAME: usize = 64;

/// A host's pairing invitation.
#[derive(Clone, PartialEq, Eq)]
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

/// Leaves out the PIN and the passphrase, so no log can carry them.
impl fmt::Debug for PairingUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingUri")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
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

    /// Parses a link. Another scheme, more than [`MAX_LEN`] bytes, a version other than 1, a
    /// missing or empty host, otp or fp, a host with anything but an address's characters, a port
    /// that is 0 or not a number, a fingerprint that is not 64 hex digits, a bad percent escape,
    /// text that is not UTF-8 and a repeated key reject it. Unknown keys are ignored; `pass`, `id`
    /// and `name` may be absent.
    pub fn parse(uri: &str) -> Option<Self> {
        if uri.len() > MAX_LEN {
            return None;
        }
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
            host: required("host").filter(|host| address_like(host))?,
            port,
            otp: required("otp")?,
            passphrase: take("pass").unwrap_or_default(),
            unique_id: take("id").unwrap_or_default(),
            cert_sha256: fingerprint(&required("fp")?)?,
            name: take("name").unwrap_or_default(),
        })
    }

    /// The link's name for a confirmation: without control characters and the invisible ones that
    /// can reorder or hide text, each run of spaces one space, so that it cannot pass for the
    /// address line, and cut to a length a dialog can show. Empty when none is left.
    pub fn shown_name(&self) -> String {
        let mut out = String::new();
        let mut count = 0;
        let mut space = false;
        for c in self.name.chars().filter(|&c| !hidden_char(c)) {
            if c.is_whitespace() {
                space = true;
                continue;
            }
            if count == MAX_SHOWN_NAME {
                break;
            }
            if space && count > 0 {
                out.push(' ');
                count += 1;
                if count == MAX_SHOWN_NAME {
                    break;
                }
            }
            space = false;
            out.push(c);
            count += 1;
        }
        out
    }

    /// Where the link pairs, for a confirmation: `host:port`, `[v6-address]:port`.
    pub fn shown_address(&self) -> String {
        let mut out = String::new();
        if self.host.contains(':') && !self.host.starts_with('[') {
            let _ = write!(out, "[{}]:{}", self.host, self.port);
        } else {
            let _ = write!(out, "{}:{}", self.host, self.port);
        }
        out
    }
}

/// A name or an IP address (`pc.local`, `192.168.4.2`, `::1`, `[fe80::1%eth0]`): nothing that can
/// end the authority of a URL, or make the address read as something else. A `%` only starts an
/// IPv6 literal's zone, because a URL parser may decode one anywhere else
/// (`192.168.4.2%2Eevil.example`), and then connect elsewhere than the dialog says.
fn address_like(host: &str) -> bool {
    let name_char = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_');
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
    let literal = bare.unwrap_or(host);
    if host.len() > MAX_HOST_LEN || literal.is_empty() {
        return false;
    }
    if !literal.contains(':') {
        return bare.is_none() && literal.bytes().all(name_char);
    }
    let (address, zone) = literal.split_once('%').unwrap_or((literal, "a"));
    address
        .bytes()
        .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
        && !zone.is_empty()
        && zone.bytes().all(name_char)
}

/// Controls, Unicode's format characters (Cf), and what else draws nothing: combining and variation
/// marks with no shape, Hangul and Khmer fillers, the blank Braille cell and the tag block. `PairingLink`
/// has the same list.
fn hidden_char(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{ad}' | '\u{34f}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}'
            | '\u{890}'..='\u{891}' | '\u{8e2}' | '\u{115f}'..='\u{1160}' | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}' | '\u{2800}' | '\u{3164}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}'
            | '\u{ffa0}' | '\u{fff0}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}')
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

    #[test]
    fn a_long_link_is_refused_at_any_size() {
        let good = invitation().to_uri();
        let fits = format!("{good}&x={}", "a".repeat(MAX_LEN - good.len() - 3));
        assert_eq!(fits.len(), MAX_LEN);
        assert!(PairingUri::parse(&fits).is_some());
        assert_eq!(PairingUri::parse(&format!("{fits}a")), None);
        // Each of these would cost time if it were read at all.
        for hostile in [
            format!("{good}&x={}", "a".repeat(32 * 1024 * 1024)),
            format!("{good}{}", "&k=v".repeat(1024 * 1024)),
            format!("rubylight://pair?{}", "%41".repeat(1024 * 1024)),
        ] {
            assert_eq!(PairingUri::parse(&hostile), None);
        }
    }

    #[test]
    fn a_host_is_only_an_address() {
        for host in [
            "192.168.4.2",
            "pc.local",
            "my_pc-1",
            "[::1]",
            "::1",
            "[fe80::1%eth0]",
        ] {
            let link = PairingUri {
                host: host.into(),
                ..invitation()
            };
            assert_eq!(PairingUri::parse(&link.to_uri()), Some(link), "{host}");
        }
        let long = "a".repeat(MAX_HOST_LEN + 1);
        for host in [
            "a/b",
            "a b",
            "a\nb",
            "a@b",
            "a?b",
            "a#b",
            "a\\b",
            "bank.example\u{202e}cod",
            "pc\u{430}",
            long.as_str(),
            // Decoded by a URL parser, these name somewhere else than the dialog shows.
            "192.168.4.2%2Eevil.example",
            "192.168.4.2%2eevil.example",
            "pc%40evil.example",
            "pc.local%",
            "[pc.local%2Eevil.example]",
            "pc.local:80%2Eevil.example",
            "[fe80::1%]",
            "[fe80::1%25%2Eevil.example]",
            "[]",
            "[pc]",
            "[::1",
            "::1]",
            "pc.local:47989",
        ] {
            let link = PairingUri {
                host: host.into(),
                ..invitation()
            };
            assert_eq!(PairingUri::parse(&link.to_uri()), None, "{host}");
        }
    }

    #[test]
    fn what_a_confirmation_shows_is_plain() {
        let named = |name: &str| {
            PairingUri {
                name: name.into(),
                ..invitation()
            }
            .shown_name()
        };
        assert_eq!(named("Wohnzimmer-PC ä"), "Wohnzimmer-PC ä");
        assert_eq!(named("  Desk\u{202e}cod\n\u{200b}\u{feff} "), "Deskcod");
        assert_eq!(named("\u{202e}\u{200b}"), "");
        for hidden in [
            '\u{34f}', '\u{600}', '\u{6dd}', '\u{70f}', '\u{8e2}', '\u{115f}', '\u{1160}',
            '\u{17b4}', '\u{180b}', '\u{2800}', '\u{3164}', '\u{fe0f}', '\u{ffa0}', '\u{fffb}',
            '\u{110bd}', '\u{13430}', '\u{1bca0}', '\u{1d173}', '\u{e0001}', '\u{e0041}', '\u{e007f}',
            '\u{e0100}',
        ] {
            assert_eq!(named(&format!("Desk{hidden}top")), "Desktop", "{:x}", hidden as u32);
        }
        // A name padded to look like the address line stays a name.
        assert_eq!(
            named("Gaming PC      192.168.4.2:47989"),
            "Gaming PC 192.168.4.2:47989"
        );
        assert_eq!(
            named("Gaming\u{a0}\u{2003}\t\u{3000}PC\u{200b} \u{200b} x"),
            "Gaming PC x"
        );
        assert_eq!(named(&format!("{}  b", "a".repeat(MAX_SHOWN_NAME - 1))), format!("{} ", "a".repeat(MAX_SHOWN_NAME - 1)));
        assert_eq!(
            named(&format!("{} {}", "a".repeat(10), "b".repeat(100))).chars().count(),
            MAX_SHOWN_NAME
        );
        assert_eq!(
            named(&"ä".repeat(MAX_SHOWN_NAME + 1)),
            "ä".repeat(MAX_SHOWN_NAME)
        );
        let address = |host: &str| {
            PairingUri {
                host: host.into(),
                ..invitation()
            }
            .shown_address()
        };
        assert_eq!(address("192.168.4.2"), "192.168.4.2:47989");
        assert_eq!(address("fe80::1"), "[fe80::1]:47989");
        assert_eq!(address("[fe80::1]"), "[fe80::1]:47989");
    }

    #[test]
    fn debug_never_shows_the_pin_or_the_passphrase() {
        let text = format!("{:?}", invitation());
        assert!(text.contains("192.168.4.2"), "{text}");
        assert!(
            !text.contains("4821") && !text.contains("a&b=c d"),
            "{text}"
        );
    }
}
