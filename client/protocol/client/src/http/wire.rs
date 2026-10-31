//! Reading one HTTP/1.1 response: the status line, the header fields and the body, framed by
//! Content-Length, chunked encoding or the end of the stream.

use super::transport::{io_tls_error, Response};
use super::Error;
use std::io::{ErrorKind, Read};

/// Reads one response; a peer closing TLS without close_notify still ends the body.
pub(super) fn read_response<R: Read + ?Sized>(stream: &mut R, body_limit: usize) -> Result<Response, Error> {
    read_response_with_headers(stream, body_limit, &mut |_, _| Ok(()))
}

pub(super) fn read_response_with_headers<R: Read + ?Sized>(stream: &mut R, body_limit: usize,
    on_headers: &mut impl FnMut(u16, &[(String, String)]) -> Result<(), Error>) -> Result<Response, Error> {
    let mut reader = Reader {
        stream,
        buffer: Vec::new(),
        eof: false,
    };
    let head = read_head(&mut reader)?;
    on_headers(head.status, &head.headers)?;
    let body = read_body(&mut reader, &head, body_limit)?;
    Ok(head.into_response(body))
}

/// A response's status line and header fields.
pub(super) struct Head {
    pub(super) status: u16,
    reason: String,
    pub(super) headers: Vec<(String, String)>,
}

impl Head {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The Content-Length: digits only and a single field, since a repeated or signed one
    /// could frame the body differently for this client than for a proxy in between.
    pub(super) fn content_length(&self) -> Result<Option<u64>, Error> {
        let invalid = || Error::Malformed("invalid Content-Length".into());
        let mut fields = self.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case("Content-Length"));
        let Some((_, value)) = fields.next() else {
            return Ok(None);
        };
        if fields.next().is_some() {
            return Err(Error::Malformed("more than one Content-Length".into()));
        }
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        value.parse().map(Some).map_err(|_| invalid())
    }

    pub(super) fn into_response(self, body: Vec<u8>) -> Response {
        Response {
            content_type: self.header("Content-Type").map(str::to_owned),
            status: self.status,
            reason: self.reason,
            body,
        }
    }
}

pub(super) fn read_head<R: Read + ?Sized>(reader: &mut Reader<'_, R>) -> Result<Head, Error> {
    let head_end = loop {
        if let Some(at) = find(&reader.buffer, b"\r\n\r\n") {
            break at;
        }
        if reader.buffer.len() > 64 * 1024 {
            return Err(Error::Malformed("HTTP header too large".into()));
        }
        if !reader.fill()? {
            return Err(Error::Malformed(
                "connection closed before the HTTP header ended".into(),
            ));
        }
    };
    let head = String::from_utf8_lossy(&reader.buffer[..head_end]).into_owned();
    reader.buffer.drain(..head_end + 4);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    let status = parts
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|_| version.starts_with("HTTP/1."))
        .ok_or_else(|| Error::Malformed(format!("invalid HTTP status line: {status_line}")))?;
    let reason = parts.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Ok(Head { status, reason, headers })
}

pub(super) fn read_body<R: Read + ?Sized>(reader: &mut Reader<'_, R>, head: &Head, body_limit: usize) -> Result<Vec<u8>, Error> {
    let header = |name: &str| head.header(name);
    let too_large =
        || Error::Malformed(format!("host response exceeds the {body_limit} byte limit"));
    let body = if header("Transfer-Encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        let mut body = Vec::new();
        loop {
            let line = reader.line()?;
            let size_text = line.split(';').next().unwrap_or_default().trim();
            let size = usize::from_str_radix(size_text, 16)
                .map_err(|_| Error::Malformed("invalid chunk size".into()))?;
            if size == 0 {
                break;
            }
            if size > body_limit.saturating_sub(body.len()) {
                return Err(too_large());
            }
            body.extend_from_slice(&reader.take(size)?);
            if !reader.line()?.is_empty() {
                return Err(Error::Malformed("invalid chunk terminator".into()));
            }
        }
        body
    } else if let Some(length) = head.content_length()? {
        let length = usize::try_from(length).ok().filter(|length| *length <= body_limit).ok_or_else(too_large)?;
        reader.take(length)?
    } else {
        while reader.fill()? {
            if reader.buffer.len() > body_limit {
                return Err(too_large());
            }
        }
        if reader.buffer.len() > body_limit {
            return Err(too_large());
        }
        std::mem::take(&mut reader.buffer)
    };
    Ok(body)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub(super) struct Reader<'a, R: Read + ?Sized> {
    pub(super) stream: &'a mut R,
    pub(super) buffer: Vec<u8>,
    pub(super) eof: bool,
}

impl<R: Read + ?Sized> Reader<'_, R> {
    /// Reads more; false at the end of the stream.
    pub(super) fn fill(&mut self) -> Result<bool, Error> {
        if self.eof {
            return Ok(false);
        }
        let mut chunk = [0; 16 * 1024];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(n) => {
                    self.buffer.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    self.eof = true;
                    return Ok(false);
                }
                Err(e) => return Err(io_tls_error(e)),
            }
        }
    }

    fn take(&mut self, n: usize) -> Result<Vec<u8>, Error> {
        while self.buffer.len() < n {
            if !self.fill()? {
                return Err(Error::Malformed(
                    "connection closed before the HTTP body ended".into(),
                ));
            }
        }
        Ok(self.buffer.drain(..n).collect())
    }

    fn line(&mut self) -> Result<String, Error> {
        loop {
            if let Some(at) = find(&self.buffer, b"\r\n") {
                let line = String::from_utf8_lossy(&self.buffer[..at]).into_owned();
                self.buffer.drain(..at + 2);
                return Ok(line);
            }
            if self.buffer.len() > 4096 || !self.fill()? {
                return Err(Error::Malformed("invalid chunked body".into()));
            }
        }
    }
}
