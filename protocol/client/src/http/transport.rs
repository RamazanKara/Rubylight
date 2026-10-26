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

#[derive(Debug)]
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
            #[cfg(windows)]
            abort_blocked_io(&socket);
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

/// Windows does not wake a read or write blocked on a socket that was shut down until the peer
/// answers, which a stalled host never does; this aborts it at once, through any handle of the
/// socket. Later calls see the shutdown and fail at once.
#[cfg(windows)]
fn abort_blocked_io(socket: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    extern "system" {
        fn CancelIoEx(file: *mut std::ffi::c_void, overlapped: *mut std::ffi::c_void) -> i32;
    }
    // SAFETY: `socket` keeps the handle open; a null OVERLAPPED cancels all of its I/O.
    unsafe {
        CancelIoEx(socket.as_raw_socket() as *mut std::ffi::c_void, std::ptr::null_mut());
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

/// A POST body and its media type.
#[derive(Clone, Copy)]
pub(crate) struct Body<'a> {
    pub bytes: &'a [u8],
    pub content_type: &'static str,
}

impl<'a> Body<'a> {
    /// The clipboard and the other classic actions send plain text.
    pub fn text(bytes: &'a [u8]) -> Self {
        Self { bytes, content_type: "text/plain; charset=utf-8" }
    }

    /// The native API.
    pub fn json(bytes: &'a [u8]) -> Self {
        Self { bytes, content_type: "application/json" }
    }

    /// A POST without a body or a media type, `Content-Length: 0`: the sleep route refuses
    /// anything else.
    pub fn empty() -> Self {
        Self { bytes: &[], content_type: "" }
    }
}

pub(crate) fn send(request: &Request<'_>, cancel: &Cancel, body: Option<Body<'_>>) -> Result<Response, Error> {
    send_inner(request, cancel, body, None, &mut |_, _, _| Ok(()))
}

type HeaderHook<'a> = dyn FnMut(u16, &[(String, String)], SocketAddr) -> Result<(), Error> + 'a;

pub(crate) fn probe(request: &Request<'_>, cancel: &Cancel, timeout: Duration, headers: &mut HeaderHook<'_>) -> Result<Response, Error> {
    send_inner(request, cancel, None, Some(timeout), headers)
}

fn send_inner(request: &Request<'_>, cancel: &Cancel, body: Option<Body<'_>>, timeout: Option<Duration>, headers: &mut HeaderHook<'_>) -> Result<Response, Error> {
    let deadline = timeout.map(|timeout| std::time::Instant::now() + timeout);
    let head = request_head(request, body);
    connection(request, &[cancel], deadline, Some(write_timeout(request)), &head, |stream, peer| {
        if let Some(body) = body { stream.write_all(body.bytes).map_err(io_tls_error)?; }
        stream.flush().map_err(io_tls_error)?;
        read_response_with_headers(stream, request.body_limit, &mut |status, fields: &[(String, String)]| headers(status, fields, peer))
    })
}

/// How long one write may block on the buffered requests.
fn write_timeout(request: &Request<'_>) -> Duration {
    request.timeouts.connect.max(Duration::from_secs(5))
}

/// A body streamed from a reader: exactly `length` bytes, announced as the Content-Length.
pub(crate) struct Upload<'a> {
    pub reader: &'a mut dyn Read,
    pub length: u64,
    pub content_type: &'static str,
}

/// Bytes moved so far and the total: once at 0, then at most every `PROGRESS_BYTES` and
/// `PROGRESS_INTERVAL`, whichever comes later, and once at the total.
pub(crate) type Progress<'a> = dyn FnMut(u64, u64) + 'a;

const PROGRESS_BYTES: u64 = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Spaces the progress calls, so a fast link does not call into the app for every chunk.
struct Throttle {
    total: u64,
    reported: u64,
    at: std::time::Instant,
}

impl Throttle {
    /// Reports 0 of `total`.
    fn start(progress: &mut Progress<'_>, total: u64) -> Self {
        progress(0, total);
        Self { total, reported: 0, at: std::time::Instant::now() }
    }

    fn update(&mut self, progress: &mut Progress<'_>, done: u64) {
        if self.due(done, std::time::Instant::now()) {
            progress(done, self.total);
        }
    }

    fn due(&mut self, done: u64, now: std::time::Instant) -> bool {
        let due = done > self.reported && (done == self.total
            || (done - self.reported >= PROGRESS_BYTES && now.saturating_duration_since(self.at) >= PROGRESS_INTERVAL));
        if due {
            self.reported = done;
            self.at = now;
        }
        due
    }
}

/// The most an upload reads from its file at a time.
const UPLOAD_CHUNK: usize = 64 * 1024;

/// POSTs `upload` in chunks without holding the whole body, then reads the answer like `send`.
/// `transfer` cancels only this upload, at once, even while a write blocks; the read timeout
/// also bounds every write, so a host that stops reading fails the upload. A reader that ends
/// before `length` fails loudly.
pub(crate) fn upload(request: &Request<'_>, cancel: &Cancel, upload: Upload<'_>, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    let head = head(request, Some((upload.content_type, upload.length)));
    connection(request, &[cancel, transfer], None, request.timeouts.read, &head, |stream, _| {
        send_upload(stream, upload, request.body_limit, progress, transfer)
    })
}

/// Why an upload stopped sending.
enum Unsent {
    /// The reader failed or ended early, or the transfer was cancelled.
    Local(Error),
    /// The connection failed.
    Write(std::io::Error),
}

fn send_upload(stream: &mut dyn Stream, upload: Upload<'_>, body_limit: usize, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    match write_upload(stream, upload, progress, transfer) {
        Ok(()) => read_response(stream, body_limit),
        Err(Unsent::Local(error)) => Err(error),
        // The host may refuse (413, 403) before it reads the body and close: its answer says
        // more than the failed write. It never accepts a body it did not get whole.
        Err(Unsent::Write(error)) => match read_response(stream, body_limit) {
            Ok(response) if !(200..=299).contains(&response.status) => Ok(response),
            _ => Err(io_tls_error(error)),
        },
    }
}

fn write_upload<W: Write + ?Sized>(stream: &mut W, upload: Upload<'_>, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<(), Unsent> {
    let Upload { reader, length, .. } = upload;
    let mut chunk = vec![0; UPLOAD_CHUNK];
    let mut sent = 0;
    let mut throttle = Throttle::start(progress, length);
    while sent < length {
        if transfer.is_cancelled() {
            return Err(Unsent::Local(Error::Cancelled));
        }
        let want = usize::try_from(length - sent).map_or(UPLOAD_CHUNK, |left| left.min(UPLOAD_CHUNK));
        let read = match reader.read(&mut chunk[..want]) {
            Ok(0) => return Err(Unsent::Local(Error::InvalidArgument(format!(
                "the upload ended after {sent} of {length} bytes")))),
            Ok(read) => read,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(Unsent::Local(Error::Io(e))),
        };
        stream.write_all(&chunk[..read]).map_err(Unsent::Write)?;
        sent += read as u64;
        throttle.update(progress, sent);
    }
    stream.flush().map_err(Unsent::Write)
}

/// GETs a body into `sink` as it arrives. A 2xx must carry a Content-Length and deliver all of
/// it; its `Response` has an empty body. Any other answer is read whole, up to `body_limit`, for
/// its error. `transfer` cancels only this download, at once, even while a read blocks.
pub(crate) fn download(request: &Request<'_>, cancel: &Cancel, sink: &mut dyn Write, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    let head = request_head(request, None);
    connection(request, &[cancel, transfer], None, Some(write_timeout(request)), &head, |stream, _| {
        receive(stream, request.body_limit, sink, progress, transfer)
    })
}

fn receive(stream: &mut dyn Stream, body_limit: usize, sink: &mut dyn Write, progress: &mut Progress<'_>, transfer: &Cancel) -> Result<Response, Error> {
    let mut reader = Reader { stream, buffer: Vec::new(), eof: false };
    let head = read_head(&mut reader)?;
    if !(200..=299).contains(&head.status) {
        let body = read_body(&mut reader, &head, body_limit)?;
        return Ok(head.into_response(body));
    }
    if head.header("Transfer-Encoding").is_some() {
        return Err(Error::Malformed("a download must not be chunked".into()));
    }
    let length = head
        .content_length()?
        .ok_or_else(|| Error::Malformed("a download without a Content-Length".into()))?;
    let mut done = 0;
    let mut throttle = Throttle::start(progress, length);
    while done < length {
        if transfer.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if reader.buffer.is_empty() && !reader.fill()? {
            return Err(Error::Malformed(format!("the download ended after {done} of {length} bytes")));
        }
        let n = usize::try_from(length - done).map_or(reader.buffer.len(), |left| left.min(reader.buffer.len()));
        sink.write_all(&reader.buffer[..n])?;
        reader.buffer.drain(..n);
        done += n as u64;
        throttle.update(progress, done);
    }
    // Bytes past the Content-Length mean the host and this client disagree on the file.
    if !reader.buffer.is_empty() || reader.fill()? {
        return Err(Error::Malformed(format!("the download is longer than its {length} bytes")));
    }
    sink.flush()?;
    Ok(head.into_response(Vec::new()))
}

/// A connected stream, plain or TLS.
trait Stream: Read + Write {}

impl<T: Read + Write> Stream for T {}

/// Connects, sends `head` and hands the stream to `exchange`, which writes any body and reads
/// the answer; any of `cancels` can abort it from another thread. Whatever fails once one is
/// set (a reset, or a body cut short by the shutdown) is `Error::Cancelled`.
fn connection<T>(request: &Request<'_>, cancels: &[&Cancel], deadline: Option<std::time::Instant>, write_timeout: Option<Duration>,
    head: &str, exchange: impl FnOnce(&mut dyn Stream, SocketAddr) -> Result<T, Error>) -> Result<T, Error> {
    let cancelled = || cancels.iter().any(|cancel| cancel.is_cancelled());
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let result = connect(request.host, request.port, request.timeouts.connect, deadline).and_then(|tcp| {
        let mut ids = Vec::with_capacity(cancels.len());
        let result = cancels
            .iter()
            .try_for_each(|cancel| cancel.register(&tcp).map(|id| ids.push(id)))
            .and_then(|()| open(request, tcp, deadline, write_timeout, head))
            .and_then(|(mut stream, peer)| exchange(&mut *stream, peer));
        for (cancel, id) in cancels.iter().zip(ids) {
            cancel.unregister(id);
        }
        result
    });
    match result {
        Err(_) if cancelled() => Err(Error::Cancelled),
        other => other,
    }
}

fn connect(host: &Host, port: u16, timeout: Duration, deadline: Option<std::time::Instant>) -> Result<TcpStream, Error> {
    connect_each(&host.socket_addrs(port)?, timeout, deadline, TcpStream::connect_timeout)
}

/// Tries the addresses in turn, each for `timeout` but never past `deadline`; a deadline that
/// runs out before an address answers fails with `TimedOut`.
fn connect_each<S>(addresses: &[SocketAddr], timeout: Duration, deadline: Option<std::time::Instant>,
    mut attempt: impl FnMut(&SocketAddr, Duration) -> std::io::Result<S>) -> Result<S, Error> {
    let mut last = None;
    for address in addresses {
        let budget = match deadline {
            None => timeout,
            Some(deadline) => match deadline.checked_duration_since(std::time::Instant::now()).filter(|left| !left.is_zero()) {
                Some(left) => timeout.min(left),
                None => return Err(Error::Io(ErrorKind::TimedOut.into())),
            },
        };
        match attempt(address, budget) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::Io(
        last.unwrap_or_else(|| ErrorKind::NotFound.into()),
    ))
}

/// Sets the timeouts, completes any TLS handshake and writes `head`.
fn open(request: &Request<'_>, tcp: TcpStream, deadline: Option<std::time::Instant>, write_timeout: Option<Duration>,
    head: &str) -> Result<(Box<dyn Stream>, SocketAddr), Error> {
    let peer = tcp.peer_addr()?;
    tcp.set_read_timeout(request.timeouts.read)?;
    tcp.set_write_timeout(write_timeout)?;
    tcp.set_nodelay(true)?;
    let tcp = DeadlineSocket { socket: tcp, deadline, read_timeout: request.timeouts.read, write_timeout };
    let mut stream: Box<dyn Stream> = match &request.tls {
        None => Box::new(tcp),
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
            Box::new(stream)
        }
    };
    stream.write_all(head.as_bytes()).map_err(io_tls_error)?;
    Ok((stream, peer))
}

fn request_head(request: &Request<'_>, body: Option<Body<'_>>) -> String {
    head(request, body.map(|body| (body.content_type, body.bytes.len() as u64)))
}

/// A GET, or a POST of `(media type, length)`.
fn head(request: &Request<'_>, body: Option<(&str, u64)>) -> String {
    let mut head = format!(
        "{} /{} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nUser-Agent: Nightjar\r\n",
        if body.is_some() { "POST" } else { "GET" },
        request.target,
        request.host.url_host(),
        request.port
    );
    if let Some((content_type, length)) = body {
        if !content_type.is_empty() {
            head.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    head.push_str("\r\n");
    head
}

struct DeadlineSocket {
    socket: TcpStream,
    deadline: Option<std::time::Instant>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
}

impl DeadlineSocket {
    fn remaining(&self) -> std::io::Result<Option<Duration>> {
        self.deadline.map(|deadline| deadline.checked_duration_since(std::time::Instant::now())
            .filter(|duration| !duration.is_zero()).ok_or_else(|| ErrorKind::TimedOut.into())).transpose()
    }
}

impl Read for DeadlineSocket {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if let Some(left) = self.remaining()? {
            self.socket.set_read_timeout(Some(self.read_timeout.map_or(left, |timeout| timeout.min(left))))?;
        }
        self.socket.read(bytes)
    }
}

impl Write for DeadlineSocket {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(left) = self.remaining()? {
            self.socket.set_write_timeout(Some(self.write_timeout.map_or(left, |timeout| timeout.min(left))))?;
        }
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.socket.flush() }
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
fn read_response<R: Read + ?Sized>(stream: &mut R, body_limit: usize) -> Result<Response, Error> {
    read_response_with_headers(stream, body_limit, &mut |_, _| Ok(()))
}

fn read_response_with_headers<R: Read + ?Sized>(stream: &mut R, body_limit: usize,
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
struct Head {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The Content-Length: digits only and a single field, since a repeated or signed one
    /// could frame the body differently for this client than for a proxy in between.
    fn content_length(&self) -> Result<Option<u64>, Error> {
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

    fn into_response(self, body: Vec<u8>) -> Response {
        Response {
            content_type: self.header("Content-Type").map(str::to_owned),
            status: self.status,
            reason: self.reason,
            body,
        }
    }
}

fn read_head<R: Read + ?Sized>(reader: &mut Reader<'_, R>) -> Result<Head, Error> {
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

fn read_body<R: Read + ?Sized>(reader: &mut Reader<'_, R>, head: &Head, body_limit: usize) -> Result<Vec<u8>, Error> {
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

struct Reader<'a, R: Read + ?Sized> {
    stream: &'a mut R,
    buffer: Vec<u8>,
    eof: bool,
}

impl<R: Read + ?Sized> Reader<'_, R> {
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
    fn a_content_length_is_digits_and_given_once() {
        let r = parse(b"HTTP/1.1 200 OK\r\ncontent-length: 05\r\n\r\nhello").unwrap();
        assert_eq!(r.body, b"hello");
        for length in ["+5", "-1", "", " ", "5, 5", "0x5", "5 5", "99999999999999999999999"] {
            let raw = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\nhello");
            let result = parse(raw.as_bytes());
            assert!(matches!(&result, Err(Error::Malformed(m)) if m == "invalid Content-Length"), "{length:?}: {result:?}");
        }
        for lengths in ["Content-Length: 5\r\nContent-Length: 5", "Content-Length: 5\r\ncontent-length: 3"] {
            let raw = format!("HTTP/1.1 200 OK\r\n{lengths}\r\n\r\nhello");
            let result = parse(raw.as_bytes());
            assert!(matches!(&result, Err(Error::Malformed(m)) if m == "more than one Content-Length"), "{lengths:?}: {result:?}");
        }
    }

    #[test]
    fn probe_deadline_closes_a_stalled_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0]; socket.read_exact(&mut byte).unwrap(); head.push(byte[0]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n").unwrap();
            let _ = socket.read(&mut [0]);
        });
        let host = Host::parse("127.0.0.1").unwrap();
        let mut headers_seen = false;
        let started = std::time::Instant::now();
        let result = probe(&Request { host: &host, port, target: "probe", tls: None,
            timeouts: Timeouts { connect: Duration::from_secs(1), read: None }, body_limit: 1 },
            &Cancel::default(), Duration::from_millis(150), &mut |_, _, _| { headers_seen = true; Ok(()) });
        assert!(headers_seen && result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        server.join().unwrap();
    }

    #[test]
    fn connect_attempts_share_the_probe_deadline() {
        // Two addresses that never answer: each attempt spends its whole budget.
        let addresses: [SocketAddr; 2] = ["192.0.2.1:47984".parse().unwrap(), "192.0.2.2:47984".parse().unwrap()];
        let mut budgets = Vec::new();
        let started = std::time::Instant::now();
        let deadline = started + Duration::from_millis(150);
        let result = connect_each(&addresses, Duration::from_secs(3), Some(deadline), |_, budget| -> std::io::Result<()> {
            budgets.push(budget);
            std::thread::sleep(budget);
            Err(ErrorKind::TimedOut.into())
        });
        assert!(matches!(&result, Err(Error::Io(e)) if e.kind() == ErrorKind::TimedOut), "{result:?}");
        assert!(budgets.iter().sum::<Duration>() <= Duration::from_millis(150), "{budgets:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
        // Without a deadline every address gets the connect timeout.
        let mut budgets = Vec::new();
        let result = connect_each(&addresses, Duration::from_secs(3), None, |_, budget| -> std::io::Result<()> {
            budgets.push(budget);
            Err(ErrorKind::ConnectionRefused.into())
        });
        assert!(matches!(&result, Err(Error::Io(e)) if e.kind() == ErrorKind::ConnectionRefused));
        assert_eq!(budgets, [Duration::from_secs(3); 2]);
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
    fn an_empty_post_has_a_zero_length_and_no_media_type() {
        let host = Host::parse("127.0.0.1").unwrap();
        let request = Request { host: &host, port: 47984, target: "power-actions/v1/sleep?token=ab", tls: None,
            timeouts: Timeouts { connect: Duration::from_secs(1), read: None }, body_limit: 1 };
        assert_eq!(request_head(&request, Some(Body::empty())),
            "POST /power-actions/v1/sleep?token=ab HTTP/1.1\r\nHost: 127.0.0.1:47984\r\nConnection: close\r\n\
             User-Agent: Nightjar\r\nContent-Length: 0\r\n\r\n");
        let head = request_head(&request, Some(Body::json(b"{}")));
        assert!(head.ends_with("Content-Type: application/json\r\nContent-Length: 2\r\n\r\n"), "{head}");
        assert!(request_head(&request, None).starts_with("GET /power-actions/v1/sleep?token=ab HTTP/1.1\r\n"));
    }

    #[test]
    fn clipboard_response_type_and_unframed_limit_are_preserved() {
        let response = parse(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\n\r\nabc").unwrap();
        assert_eq!(response.content_type.as_deref(), Some("text/plain; charset=utf-8"));
        let mut bytes = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        bytes.extend_from_slice(&[b'x'; 1025]);
        assert!(read_response(&mut bytes.as_slice(), 1024).is_err());
    }

    /// Bytes no chunk size divides.
    fn file(length: usize) -> Vec<u8> {
        (0..length).map(|i| (i * 7 % 251) as u8).collect()
    }

    /// A plain-TCP host that hands its one connection to `serve`, which returns what it got.
    fn host<T: Send + 'static>(serve: impl FnOnce(std::net::TcpStream) -> T + Send + 'static) -> (u16, std::thread::JoinHandle<T>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        (port, std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            serve(socket)
        }))
    }

    fn read_head_of(socket: &mut std::net::TcpStream) -> String {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0]; socket.read_exact(&mut byte).unwrap(); head.push(byte[0]);
        }
        String::from_utf8(head).unwrap()
    }

    /// Reads a request body until the client stops sending.
    fn read_rest(socket: &mut std::net::TcpStream) -> Vec<u8> {
        let mut body = Vec::new();
        let mut chunk = [0; 64 * 1024];
        while let Ok(n @ 1..) = socket.read(&mut chunk) { body.extend_from_slice(&chunk[..n]); }
        body
    }

    fn transfer<'a>(host: &'a Host, port: u16, target: &'a str) -> Request<'a> {
        Request { host, port, target, tls: None,
            timeouts: Timeouts { connect: Duration::from_secs(1), read: Some(Duration::from_secs(10)) }, body_limit: 1024 }
    }

    /// Records every progress call.
    fn recorder(calls: &mut Vec<(u64, u64)>) -> impl FnMut(u64, u64) + '_ {
        |done, total| calls.push((done, total))
    }

    /// From 0 to the total, rising by at least `PROGRESS_BYTES` between the ends.
    fn assert_progress(calls: &[(u64, u64)], total: u64) {
        assert_eq!(calls.first(), Some(&(0, total)));
        assert_eq!(calls.last(), Some(&(total, total)));
        assert!(calls.iter().all(|call| call.1 == total), "{calls:?}");
        assert!(calls.windows(2).all(|w| w[0].0 < w[1].0), "{calls:?}");
        assert!(calls[1..calls.len() - 1].windows(2).all(|w| w[1].0 - w[0].0 >= PROGRESS_BYTES), "{calls:?}");
    }

    #[test]
    fn progress_waits_for_both_bytes_and_time_and_always_reports_the_end() {
        let mut calls = Vec::new();
        let mut throttle = Throttle::start(&mut recorder(&mut calls), 1024 * 1024);
        let start = throttle.at;
        let ms = |ms| start + Duration::from_millis(ms);
        // Enough bytes, too soon; then enough time, too few bytes.
        assert!(!throttle.due(PROGRESS_BYTES, ms(50)));
        assert!(!throttle.due(PROGRESS_BYTES - 1, ms(150)));
        assert!(throttle.due(PROGRESS_BYTES, ms(150)));
        // Both again, counted from the last report.
        assert!(!throttle.due(2 * PROGRESS_BYTES - 1, ms(400)));
        assert!(!throttle.due(3 * PROGRESS_BYTES, ms(249)));
        assert!(throttle.due(3 * PROGRESS_BYTES, ms(250)));
        // The end goes out at once, and only once.
        assert!(throttle.due(1024 * 1024, ms(251)));
        assert!(!throttle.due(1024 * 1024, ms(900)));
        assert_eq!(calls, [(0, 1024 * 1024)]);
        // An empty transfer reports only its start.
        let mut calls = Vec::new();
        let mut throttle = Throttle::start(&mut recorder(&mut calls), 0);
        assert!(!throttle.due(0, std::time::Instant::now()));
        assert_eq!(calls, [(0, 0)]);
    }

    /// Passes bytes through and calls `trip` once `after` of them went by.
    struct Trip<T, F: FnMut()> {
        inner: T,
        after: usize,
        trip: Option<F>,
    }

    impl<T, F: FnMut()> Trip<T, F> {
        fn new(inner: T, after: usize, trip: F) -> Self { Self { inner, after, trip: Some(trip) } }

        fn count(&mut self, n: usize) {
            self.after = self.after.saturating_sub(n);
            if self.after == 0 { if let Some(mut trip) = self.trip.take() { trip(); } }
        }
    }

    impl<T: Read, F: FnMut()> Read for Trip<T, F> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(bytes)?;
            self.count(n);
            Ok(n)
        }
    }

    impl<T: Write, F: FnMut()> Write for Trip<T, F> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let n = self.inner.write(bytes)?;
            self.count(n);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
    }

    #[test]
    fn an_upload_streams_exactly_its_length_in_chunks() {
        let sent = file(5 * 1024 * 1024 + 7);
        let (port, server) = host(|mut socket| {
            let head = read_head_of(&mut socket);
            let length: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
            let mut body = vec![0; length];
            socket.read_exact(&mut body).unwrap();
            socket.write_all(b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").unwrap();
            (head, body)
        });
        let host_name = Host::parse("127.0.0.1").unwrap();
        let mut calls = Vec::new();
        // A reader with more than `length`: only `length` bytes go out.
        let mut reader = std::io::Cursor::new([sent.as_slice(), b"not sent"].concat());
        let response = upload(&transfer(&host_name, port, "files?name=a"), &Cancel::default(),
            Upload { reader: &mut reader, length: sent.len() as u64, content_type: "application/octet-stream" },
            &mut recorder(&mut calls), &Cancel::default()).unwrap();
        assert_eq!((response.status, response.body.as_slice()), (201, &b"{}"[..]));
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /files?name=a HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("Content-Type: application/octet-stream\r\nContent-Length: 5242887\r\n"), "{head}");
        assert!(body == sent);
        assert_progress(&calls, sent.len() as u64);
    }

    #[test]
    fn a_short_or_stopped_upload_is_an_error_not_a_short_body() {
        let host_name = Host::parse("127.0.0.1").unwrap();
        // The file ends early.
        let (port, server) = host(|mut socket| { read_head_of(&mut socket); read_rest(&mut socket).len() });
        let result = upload(&transfer(&host_name, port, "files"), &Cancel::default(),
            Upload { reader: &mut &file(1000)[..], length: 4000, content_type: "application/octet-stream" },
            &mut |_, _| {}, &Cancel::default());
        assert!(matches!(&result, Err(Error::InvalidArgument(m)) if m == "the upload ended after 1000 of 4000 bytes"), "{result:?}");
        assert_eq!(server.join().unwrap(), 1000);
        // Stopped mid-transfer: no further chunk goes out.
        let (port, server) = host(|mut socket| { read_head_of(&mut socket); read_rest(&mut socket).len() });
        let stop = Cancel::default();
        let data = file(4 * 1024 * 1024);
        let mut reader = Trip::new(&data[..], 1024 * 1024, || stop.cancel());
        let result = upload(&transfer(&host_name, port, "files"), &Cancel::default(),
            Upload { reader: &mut reader, length: 4 * 1024 * 1024, content_type: "application/octet-stream" },
            &mut |_, _| {}, &stop);
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        // The cancel also shuts the socket, so the chunk in flight may not get out either.
        assert!(server.join().unwrap() <= 1024 * 1024);
    }

    /// Cancels `transfer` from another thread after `after`.
    fn cancel_later(transfer: &Arc<Cancel>, after: Duration) -> std::thread::JoinHandle<()> {
        let transfer = transfer.clone();
        std::thread::spawn(move || { std::thread::sleep(after); transfer.cancel(); })
    }

    #[test]
    fn a_cancel_ends_a_stalled_transfer_at_once_and_only_that_transfer() {
        let host_name = Host::parse("127.0.0.1").unwrap();
        let client = Cancel::default();
        // A download whose host sends half the body, then nothing: the read blocks until the
        // cancel shuts the socket, and the body cut short is a cancel, not a malformed answer.
        let (port, server) = host(|mut socket| {
            read_head_of(&mut socket);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2000\r\n\r\n").unwrap();
            socket.write_all(&file(1000)).unwrap();
            read_rest(&mut socket);
        });
        let job = Arc::new(Cancel::default());
        let canceller = cancel_later(&job, Duration::from_millis(200));
        let started = std::time::Instant::now();
        let result = download(&transfer(&host_name, port, "f"), &client, &mut Vec::new(), &mut |_, _| {}, &job);
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        canceller.join().unwrap();
        server.join().unwrap();

        // An upload to a host that stops reading: the write blocks until the cancel.
        let (port, server) = host(|mut socket| {
            read_head_of(&mut socket);
            std::thread::sleep(Duration::from_secs(3));
        });
        let job = Arc::new(Cancel::default());
        let canceller = cancel_later(&job, Duration::from_millis(200));
        let started = std::time::Instant::now();
        let result = upload(&transfer(&host_name, port, "files"), &client,
            Upload { reader: &mut std::io::repeat(7).take(64 << 20), length: 64 << 20, content_type: "application/octet-stream" },
            &mut |_, _| {}, &job);
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        canceller.join().unwrap();
        server.join().unwrap();

        // The client's own cancel is untouched: its next request goes through.
        assert!(!client.is_cancelled());
        let (port, server) = serve_download("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n".into(), b"abc".to_vec());
        let mut sink = Vec::new();
        download(&transfer(&host_name, port, "f"), &client, &mut sink, &mut |_, _| {}, &Cancel::default()).unwrap();
        assert_eq!(sink, b"abc");
        server.join().unwrap();
        // And a client-wide cancel still stops a transfer.
        let job = Cancel::default();
        client.cancel();
        let result = download(&transfer(&host_name, port, "f"), &client, &mut Vec::new(), &mut |_, _| {}, &job);
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    }

    /// Takes `accepted` bytes, then fails every write like a reset connection.
    struct Refusing {
        accepted: usize,
        answer: std::io::Cursor<Vec<u8>>,
    }

    impl Read for Refusing {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> { self.answer.read(bytes) }
    }

    impl Write for Refusing {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.accepted { return Err(ErrorKind::ConnectionReset.into()); }
            self.accepted -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    #[test]
    fn a_refusal_sent_before_the_body_was_read_wins_over_the_failed_write() {
        let send = |answer: &[u8]| {
            let mut stream = Refusing { accepted: 100 * 1024, answer: std::io::Cursor::new(answer.to_vec()) };
            send_upload(&mut stream, Upload { reader: &mut &file(1024 * 1024)[..], length: 1024 * 1024,
                content_type: "application/octet-stream" }, 1024, &mut |_, _| {}, &Cancel::default())
        };
        let error = br#"{"error":{"code":"too_large","message":""}}"#;
        let refusal = [format!("HTTP/1.1 413 Payload Too Large\r\nContent-Length: {}\r\n\r\n", error.len()).as_bytes(), error].concat();
        let response = send(&refusal).unwrap();
        assert_eq!(response.status, 413);
        assert!(response.body.starts_with(br#"{"error":{"code":"too_large""#));
        // No answer, or a success the host cannot have meant: the write error stands.
        assert!(matches!(send(b""), Err(Error::Io(e)) if e.kind() == ErrorKind::ConnectionReset));
        assert!(matches!(send(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n"), Err(Error::Io(e)) if e.kind() == ErrorKind::ConnectionReset));
    }

    /// A host that answers a GET with `head` and then `body` in uneven pieces.
    fn serve_download(head: String, body: Vec<u8>) -> (u16, std::thread::JoinHandle<String>) {
        host(move |mut socket| {
            let request = read_head_of(&mut socket);
            socket.write_all(head.as_bytes()).unwrap();
            for piece in body.chunks(9973) {
                if socket.write_all(piece).is_err() { break; }
            }
            request
        })
    }

    #[test]
    fn a_download_streams_into_the_sink_and_must_be_whole() {
        let host_name = Host::parse("127.0.0.1").unwrap();
        let sent = file(3 * 1024 * 1024 + 11);
        let (port, server) = serve_download(format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n", sent.len()), sent.clone());
        let mut sink = Vec::new();
        let mut calls = Vec::new();
        let response = download(&transfer(&host_name, port, "files/content?name=a"), &Cancel::default(), &mut sink,
            &mut recorder(&mut calls), &Cancel::default()).unwrap();
        assert_eq!((response.status, response.body.len()), (200, 0));
        assert!(sink == sent);
        assert_progress(&calls, sent.len() as u64);
        assert!(server.join().unwrap().starts_with("GET /files/content?name=a HTTP/1.1\r\n"));

        // The host closes early.
        let (port, server) = serve_download("HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n".into(), file(500));
        let result = download(&transfer(&host_name, port, "f"), &Cancel::default(), &mut Vec::new(), &mut |_, _| {}, &Cancel::default());
        assert!(matches!(&result, Err(Error::Malformed(m)) if m == "the download ended after 500 of 1000 bytes"), "{result:?}");
        server.join().unwrap();

        // The host sends more than it announced, by a lot or by one byte.
        for body in [file(1500), [file(1000), file(1)].concat()] {
            let (port, server) = serve_download("HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n".into(), body);
            let result = download(&transfer(&host_name, port, "f"), &Cancel::default(), &mut Vec::new(), &mut |_, _| {}, &Cancel::default());
            assert!(matches!(&result, Err(Error::Malformed(m)) if m == "the download is longer than its 1000 bytes"), "{result:?}");
            server.join().unwrap();
        }

        // No length to check against, or one that is not plain digits.
        for head in ["HTTP/1.1 200 OK\r\n\r\n", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: +3\r\n\r\n", "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 3\r\n\r\n"] {
            let (port, server) = serve_download(head.into(), b"3\r\nabc\r\n0\r\n\r\n".to_vec());
            let mut sink = Vec::new();
            let result = download(&transfer(&host_name, port, "f"), &Cancel::default(), &mut sink, &mut |_, _| {}, &Cancel::default());
            assert!(matches!(result, Err(Error::Malformed(_))) && sink.is_empty(), "{head}: {result:?}");
            server.join().unwrap();
        }

        // A refusal is read whole for its error and never reaches the sink.
        let error = br#"{"error":{"code":"not_found","message":"No such file."}}"#;
        let (port, server) = serve_download(format!("HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n", error.len()), error.to_vec());
        let mut sink = Vec::new();
        let response = download(&transfer(&host_name, port, "f"), &Cancel::default(), &mut sink, &mut |_, _| {}, &Cancel::default()).unwrap();
        assert_eq!((response.status, response.body.as_slice(), sink.len()), (404, &error[..], 0));
        server.join().unwrap();
    }

    #[test]
    fn a_stopped_download_ends_mid_transfer() {
        let host_name = Host::parse("127.0.0.1").unwrap();
        let length = 16 * 1024 * 1024;
        let (port, server) = serve_download(format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n"), file(length));
        let stop = Cancel::default();
        let mut sink = Trip::new(Vec::new(), 1024 * 1024, || stop.cancel());
        let result = download(&transfer(&host_name, port, "f"), &Cancel::default(), &mut sink, &mut |_, _| {}, &stop);
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        // It stops before the next piece it reads.
        let got = sink.inner.len();
        assert!((1024 * 1024..1024 * 1024 + 16 * 1024).contains(&got), "{got}");
        server.join().unwrap();
    }
}
