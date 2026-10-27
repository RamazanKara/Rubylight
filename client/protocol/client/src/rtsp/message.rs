//! RTSP request serialization and response parsing, as Nightjar's `android_rtsp_parser.h`
//! (from moonlight-common-c `RtspParser.c`) writes and reads them.

use super::RtspError;

/// A request; options are written in insertion order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtspRequest {
    pub method: String,
    pub target: String,
    pub options: Vec<(String, String)>,
    pub payload: Vec<u8>,
}

impl RtspRequest {
    pub fn new(method: &str, target: &str) -> Self {
        Self {
            method: method.into(),
            target: target.into(),
            options: Vec::new(),
            payload: Vec::new(),
        }
    }

    /// Adds an option; a repeated name replaces the earlier value in place.
    pub fn option(&mut self, name: &str, value: &str) -> &mut Self {
        match self.options.iter_mut().find(|(n, _)| n == name) {
            Some(existing) => existing.1 = value.into(),
            None => self.options.push((name.into(), value.into())),
        }
        self
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = format!("{} {} RTSP/1.0\r\n", self.method, self.target);
        for (name, value) in &self.options {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        let mut out = out.into_bytes();
        out.extend_from_slice(&self.payload);
        out
    }
}

/// A response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtspResponse {
    pub status_code: i32,
    pub status_text: String,
    pub options: Vec<(String, String)>,
    /// None when nothing follows the header.
    pub payload: Option<Vec<u8>>,
}

impl RtspResponse {
    /// An option's value; the last of repeated options wins, as in the C parser.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn sequence(&self) -> Option<i32> {
        self.option("CSeq").map(atoi)
    }

    pub fn payload_text(&self) -> Option<String> {
        self.payload
            .as_ref()
            .map(|p| String::from_utf8_lossy(p).into_owned())
    }
}

/// C atoi: optional whitespace and sign, then digits.
pub(crate) fn atoi(text: &str) -> i32 {
    strtol(text, 10).0 as i32
}

/// C strtol/strtoul for base 10 or 0 (0x for hex, a leading 0 for octal): the value and
/// whether any digit was read. Overflow saturates.
pub(crate) fn strtol(text: &str, base: u32) -> (i64, bool) {
    let mut rest = text.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let negative = rest.starts_with('-');
    if negative || rest.starts_with('+') {
        rest = &rest[1..];
    }
    let mut radix = base;
    if base == 0 {
        if (rest.starts_with("0x") || rest.starts_with("0X"))
            && rest[2..].starts_with(|c: char| c.is_ascii_hexdigit())
        {
            rest = &rest[2..];
            radix = 16;
        } else if rest.starts_with('0') {
            radix = 8;
        } else {
            radix = 10;
        }
    }
    let mut value: i64 = 0;
    let mut any = false;
    for c in rest.chars() {
        let Some(digit) = c.to_digit(radix) else {
            break;
        };
        any = true;
        value = value
            .saturating_mul(radix as i64)
            .saturating_add(digit as i64);
    }
    (if negative { -value } else { value }, any)
}

pub fn parse_response(raw: &[u8]) -> Result<RtspResponse, RtspError> {
    let malformed = |what: &str| RtspError::Malformed(format!("RTSP response {what}"));
    let (head, payload) = match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(at) => (&raw[..at], &raw[at + 4..]),
        // RTSP over ENet and some emulators lose the final CRLF.
        None if raw.ends_with(b"\r\n") => (&raw[..raw.len() - 2], &raw[raw.len()..]),
        None if raw.ends_with(b"\r\n\r") => (&raw[..raw.len() - 3], &raw[raw.len()..]),
        None => return Err(malformed("has no end of header")),
    };
    let head = std::str::from_utf8(head).map_err(|_| malformed("header is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.trim_start().splitn(2, ' ');
    let protocol = parts.next().unwrap_or_default();
    if protocol != "RTSP/1.0" {
        return Err(malformed("is not RTSP/1.0"));
    }
    let rest = parts
        .next()
        .ok_or_else(|| malformed("has no status"))?
        .trim_start();
    let (code, text) = rest.split_once(' ').unwrap_or((rest, ""));
    if code.is_empty() {
        return Err(malformed("has no status"));
    }
    let mut options = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| malformed("has an invalid option"))?;
        let value = value.strip_prefix(' ').unwrap_or(value);
        options.push((name.trim().to_owned(), value.to_owned()));
    }
    Ok(RtspResponse {
        status_code: atoi(code),
        status_text: text.to_owned(),
        options,
        payload: (!payload.is_empty()).then(|| payload.to_vec()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_serialize_like_the_c_client() {
        let mut request = RtspRequest::new("ANNOUNCE", "streamid=control/13/0");
        request
            .option("CSeq", "6")
            .option("X-GS-ClientVersion", "14")
            .option("Host", "10.0.0.2")
            .option("Session", "DEADBEEFCAFE")
            .option("Content-type", "application/sdp")
            .option("Content-length", "3");
        request.payload = b"v=0".to_vec();
        assert_eq!(
            request.serialize(),
            b"ANNOUNCE streamid=control/13/0 RTSP/1.0\r\nCSeq: 6\r\nX-GS-ClientVersion: 14\r\nHost: 10.0.0.2\r\n\
              Session: DEADBEEFCAFE\r\nContent-type: application/sdp\r\nContent-length: 3\r\n\r\nv=0"
        );
        request.option("CSeq", "7");
        assert!(request
            .serialize()
            .starts_with(b"ANNOUNCE streamid=control/13/0 RTSP/1.0\r\nCSeq: 7\r\n"));
    }

    #[test]
    fn responses_from_sunshine_and_rubylight() {
        let r = parse_response(
            b"RTSP/1.0 200 OK\r\nCSeq: 3\r\nSession: DEADBEEFCAFE;timeout = 90\r\nTransport: server_port=48000\r\n\
              X-SS-Ping-Payload: 0123456789ABCDEF\r\n\r\n",
        )
        .unwrap();
        assert_eq!(
            (r.status_code, r.status_text.as_str(), r.sequence()),
            (200, "OK", Some(3))
        );
        assert_eq!(r.option("session"), Some("DEADBEEFCAFE;timeout = 90"));
        assert_eq!(r.option("Transport"), Some("server_port=48000"));
        assert_eq!(r.payload, None);

        let r = parse_response(b"RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: 5\r\n\r\nhello")
            .unwrap();
        assert_eq!(r.payload.as_deref(), Some(&b"hello"[..]));
        let r = parse_response(b"RTSP/1.0 406 Requested codec is unavailable\r\nCSeq: 7\r\n\r\n")
            .unwrap();
        assert_eq!(
            (r.status_code, r.status_text.as_str()),
            (406, "Requested codec is unavailable")
        );
        // Lost final CRLF (ENet, emulators) and a repeated option.
        let r = parse_response(b"RTSP/1.0 200 OK\r\nA: 1\r\nA: 2\r\n").unwrap();
        assert_eq!(r.option("A"), Some("2"));
        assert!(parse_response(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r").is_ok());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n\r\n").is_err());
        assert!(parse_response(b"RTSP/1.0 200 OK\r\nCSeq: 1").is_err());
        assert!(parse_response(b"").is_err());
    }

    #[test]
    fn strtol_follows_c() {
        assert_eq!(strtol("  42abc", 10), (42, true));
        assert_eq!(strtol("0x1F", 0), (31, true));
        assert_eq!(strtol("010", 0), (8, true));
        assert_eq!(strtol("-5", 0), (-5, true));
        assert_eq!(strtol("x", 0), (0, false));
        assert_eq!(strtol("0x", 0), (0, true));
        assert_eq!(atoi("200 OK"), 200);
    }
}
