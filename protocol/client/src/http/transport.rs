//! Blocking HTTP/1.1 over TCP, or over TLS with the client certificate and a pinned server
//! certificate, as NvHTTP.java configures OkHttp.

use super::Error;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV6, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A host address: an IP literal (IPv6 may carry a numeric `%scope`) or a host name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Host {
    /// Without brackets or scope.
    pub name: String,
    pub scope_id: u32,
}

impl Host {
    pub fn parse(address: &str) -> Result<Self, Error> {
        let mut text = address.trim();
        if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            text = inner;
        }
        if text.is_empty() {
            return Err(Error::InvalidArgument("empty host address".into()));
        }
        let (name, scope) = match text.split_once('%') {
            Some((name, scope)) if name.contains(':') => (name, Some(scope)),
            _ => (text, None),
        };
        let scope_id = match scope {
            Some(scope) => scope
                .parse()
                .map_err(|_| Error::InvalidArgument(format!("unsupported IPv6 scope: {scope}")))?,
            None => 0,
        };
        // An IPv4-mapped IPv6 literal is used as plain IPv4.
        let name = match name.parse::<Ipv6Addr>() {
            Ok(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => v4.to_string(),
                None => v6.to_string(),
            },
            Err(_) => name.to_owned(),
        };
        Ok(Self { name, scope_id })
    }

    /// The host as it appears in a URL or Host header.
    pub fn url_host(&self) -> String {
        if self.name.contains(':') {
            format!("[{}]", self.name)
        } else {
            self.name.clone()
        }
    }

    fn socket_addrs(&self, port: u16) -> Result<Vec<SocketAddr>, Error> {
        if let Ok(ip) = self.name.parse::<IpAddr>() {
            return Ok(vec![match ip {
                IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, port, 0, self.scope_id)),
                v4 => SocketAddr::new(v4, port),
            }]);
        }
        Ok((self.name.as_str(), port).to_socket_addrs()?.collect())
    }
}

/// Percent-encodes a query component.
pub(crate) fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'*') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A random version 4 UUID, which NvHTTP adds to every request.
pub(crate) fn random_uuid() -> String {
    let mut b = super::crypto::random::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = super::der::hex_encode(&b).to_ascii_lowercase();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

pub(crate) struct Response {
    pub status: u16,
    pub reason: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    pub connect: Duration,
    /// None: no read timeout, for requests that wait on the user or a launching game.
    pub read: Option<Duration>,
}

/// Open sockets, so another thread can abort requests (NvHTTP.cancelPendingRequests).
#[derive(Default)]
pub(crate) struct Cancel {
    cancelled: AtomicBool,
    next: AtomicU64,
    sockets: Mutex<Vec<(u64, TcpStream)>>,
}

impl Cancel {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        for (_, socket) in self.sockets.lock().unwrap().drain(..) {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn register(&self, socket: &TcpStream) -> Result<u64, Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.sockets.lock().unwrap().push((id, socket.try_clone()?));
        if self.is_cancelled() {
            let _ = socket.shutdown(Shutdown::Both);
            self.unregister(id);
            return Err(Error::Cancelled);
        }
        Ok(id)
    }

    fn unregister(&self, id: u64) {
        self.sockets.lock().unwrap().retain(|(i, _)| *i != id);
    }
}

pub(crate) struct Request<'a> {
    pub host: &'a Host,
    pub port: u16,
    /// Path and query, without the leading slash.
    pub target: &'a str,
    pub tls: Option<Arc<ClientConfig>>,
    pub timeouts: Timeouts,
    pub body_limit: usize,
}

pub(crate) fn get(request: &Request<'_>, cancel: &Cancel) -> Result<Response, Error> {
    send(request, cancel, None)
}

pub(crate) fn send(request: &Request<'_>, cancel: &Cancel, body: Option<&[u8]>) -> Result<Response, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let tcp = connect(request.host, request.port, request.timeouts.connect)?;
    let id = cancel.register(&tcp)?;
    let result = exchange(request, tcp, body);
    cancel.unregister(id);
    match result {
        Err(Error::Io(_)) if cancel.is_cancelled() => Err(Error::Cancelled),
        other => other,
    }
}

fn connect(host: &Host, port: u16, timeout: Duration) -> Result<TcpStream, Error> {
    let mut last = None;
    for address in host.socket_addrs(port)? {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::Io(
        last.unwrap_or_else(|| ErrorKind::NotFound.into()),
    ))
}

fn exchange(request: &Request<'_>, tcp: TcpStream, body: Option<&[u8]>) -> Result<Response, Error> {
    tcp.set_read_timeout(request.timeouts.read)?;
    tcp.set_write_timeout(Some(request.timeouts.connect.max(Duration::from_secs(5))))?;
    tcp.set_nodelay(true)?;
    let mut head = format!(
        "{} /{} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nUser-Agent: Nightjar\r\n",
        if body.is_some() { "POST" } else { "GET" },
        request.target,
        request.host.url_host(),
        request.port
    );
    if let Some(body) = body {
        head.push_str(&format!("Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    match &request.tls {
        None => {
            let mut tcp = tcp;
            tcp.write_all(head.as_bytes())?;
            if let Some(body) = body { tcp.write_all(body)?; }
            read_response(&mut tcp, request.body_limit)
        }
        Some(config) => {
            let name = ServerName::try_from(request.host.name.clone()).map_err(|_| {
                Error::InvalidArgument(format!("invalid host name: {}", request.host.name))
            })?;
            let connection = ClientConnection::new(config.clone(), name).map_err(tls_error)?;
            let mut stream = StreamOwned::new(connection, tcp);
            while stream.conn.is_handshaking() {
                stream
                    .conn
                    .complete_io(&mut stream.sock)
                    .map_err(io_tls_error)?;
            }
            stream.write_all(head.as_bytes()).map_err(io_tls_error)?;
            if let Some(body) = body { stream.write_all(body).map_err(io_tls_error)?; }
            stream.flush().map_err(io_tls_error)?;
            read_response(&mut stream, request.body_limit)
        }
    }
}

fn tls_error(error: rustls::Error) -> Error {
    match error {
        rustls::Error::InvalidCertificate(_) => Error::CertificateMismatch,
        other => Error::Tls(other.to_string()),
    }
}

fn io_tls_error(error: std::io::Error) -> Error {
    let inner = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
        .cloned();
    match inner {
        Some(e) => tls_error(e),
        None => Error::Io(error),
    }
}

/// Reads one response; a peer closing TLS without close_notify still ends the body.
fn read_response(stream: &mut impl Read, body_limit: usize) -> Result<Response, Error> {
    let mut reader = Reader {
        stream,
        buffer: Vec::new(),
        eof: false,
    };
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
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
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
            if body.len() + size > body_limit {
                return Err(too_large());
            }
            body.extend_from_slice(&reader.take(size)?);
            if !reader.line()?.is_empty() {
                return Err(Error::Malformed("invalid chunk terminator".into()));
            }
        }
        body
    } else if let Some(length) = header("Content-Length") {
        let length: usize = length
            .parse()
            .map_err(|_| Error::Malformed("invalid Content-Length".into()))?;
        if length > body_limit {
            return Err(too_large());
        }
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
    Ok(Response {
        status,
        reason,
        content_type: header("Content-Type").map(str::to_owned),
        body,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

struct Reader<'a, R: Read> {
    stream: &'a mut R,
    buffer: Vec<u8>,
    eof: bool,
}

impl<R: Read> Reader<'_, R> {
    /// Reads more; false at the end of the stream.
    fn fill(&mut self) -> Result<bool, Error> {
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

/// Accepts exactly the pinned certificate, whatever its name or issuer.
#[derive(Debug)]
struct PinnedCertificate {
    pinned: Vec<u8>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.pinned.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// TLS that presents the client certificate and trusts only `server_certificate`.
pub(crate) fn tls_config(
    server_certificate: &[u8],
    client_certificate: &[u8],
    client_key_pkcs8: &[u8],
) -> Result<Arc<ClientConfig>, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinnedCertificate {
        pinned: server_certificate.to_vec(),
        provider: provider.clone(),
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(
            vec![CertificateDer::from(client_certificate.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(client_key_pkcs8.to_vec())),
        )
        .map_err(tls_error)?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &[u8]) -> Result<Response, Error> {
        read_response(&mut &raw[..], 1024)
    }

    #[test]
    fn bodies_by_length_chunks_and_close() {
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloextra").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));
        let r = parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;x=1\r\nhello\r\n1\r\n!\r\n0\r\n\r\n").unwrap();
        assert_eq!(r.body, b"hello!");
        let r = parse(b"HTTP/1.0 404 Not Found\r\nX-A: b\r\n\r\nrest").unwrap();
        assert_eq!(
            (r.status, r.reason.as_str(), r.body.as_slice()),
            (404, "Not Found", &b"rest"[..])
        );
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel").is_err());
        assert!(parse(b"garbage\r\n\r\n").is_err());
    }

    #[test]
    fn hosts_and_queries() {
        assert_eq!(
            Host::parse("192.168.1.2").unwrap().url_host(),
            "192.168.1.2"
        );
        assert_eq!(
            Host::parse("[fe80::1%3]").unwrap(),
            Host {
                name: "fe80::1".into(),
                scope_id: 3
            }
        );
        assert_eq!(Host::parse("fe80::1").unwrap().url_host(), "[fe80::1]");
        assert_eq!(
            Host::parse("::ffff:10.0.0.1").unwrap().url_host(),
            "10.0.0.1"
        );
        assert_eq!(Host::parse("pc.local").unwrap().url_host(), "pc.local");
        assert!(Host::parse("fe80::1%wlan0").is_err());
        assert_eq!(encode_query("moto g(60)+"), "moto%20g%2860%29%2B");
        let uuid = random_uuid();
        assert_eq!(
            (uuid.len(), &uuid[14..15], uuid.matches('-').count()),
            (36, "4", 4)
        );
    }

    #[test]
    fn clipboard_response_type_and_unframed_limit_are_preserved() {
        let response = parse(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\n\r\nabc").unwrap();
        assert_eq!(response.content_type.as_deref(), Some("text/plain; charset=utf-8"));
        let mut bytes = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        bytes.extend_from_slice(&[b'x'; 1025]);
        assert!(read_response(&mut bytes.as_slice(), 1024).is_err());
    }
}
