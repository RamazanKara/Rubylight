//! Blocking HTTP/1.1 over TCP, or over TLS with the client certificate and a pinned server
//! certificate, as NvHTTP.java configures OkHttp.

mod tls;
mod transfer;

pub(crate) use tls::{io_tls_error, tls_config, tls_error};
pub(crate) use transfer::{Upload, download, download_optional, upload};
use transfer::{CONTINUE_WAIT, ContinueBy, continue_elapsed};
#[cfg(test)]
use transfer::{PROGRESS_BYTES, Throttle, is_continue_elapsed, send_upload};

use super::wire::{read_response_with_headers, Reader};
use super::Error;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV6, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

static USER_AGENT: OnceLock<String> = OnceLock::new();

/// Names the app's version in every request's `User-Agent`, `Nightjar/<version>`; until it is
/// set (tools and tests) the header is plain `Nightjar`. The first call wins.
pub fn set_app_version(version: &str) {
    let _ = USER_AGENT.set(user_agent_for(version));
}

/// `Nightjar/<version>` with only printable ASCII kept, so a version can never end the header.
fn user_agent_for(version: &str) -> String {
    let version: String = version.chars().filter(|c| c.is_ascii_graphic()).collect();
    if version.is_empty() {
        "Nightjar".into()
    } else {
        format!("Nightjar/{version}")
    }
}

pub(super) fn user_agent() -> &'static str {
    USER_AGENT.get().map_or("Nightjar", String::as_str)
}

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
    sockets: Mutex<Vec<(u64, Arc<Abort>)>>,
}

/// Thread-safe handle for cancelling a [`super::HostClient`]'s pending or future requests.
#[derive(Clone)]
pub struct HostClientCanceller(pub(super) Arc<Cancel>);

impl HostClientCanceller {
    /// Aborts any in-flight socket I/O and rejects later requests on the linked client.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Whether [`Self::cancel`] or [`super::HostClient::cancel_pending_requests`] was triggered.
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

impl Cancel {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let sockets: Vec<_> = self.sockets.lock().unwrap().drain(..).collect();
        for (_, socket) in sockets {
            socket.abort();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(super) fn register(&self, socket: &Arc<Abort>) -> Result<u64, Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.sockets.lock().unwrap().push((id, socket.clone()));
        if self.is_cancelled() {
            socket.abort();
            self.unregister(id);
            return Err(Error::Cancelled);
        }
        Ok(id)
    }

    pub(super) fn unregister(&self, id: u64) {
        self.sockets.lock().unwrap().retain(|(i, _)| *i != id);
    }
}

/// A connection's socket as its cancels hold it. The abort is sticky: every read and write
/// through [`DeadlineSocket`] checks it before it starts, so none begins blocking after a cancel.
pub(crate) struct Abort {
    socket: TcpStream,
    aborted: AtomicBool,
    /// While a read or write may block on the socket.
    busy: AtomicBool,
}

/// The most [`Abort::abort`] waits for a call it aborted to come back.
#[cfg(windows)]
const ABORT_WAIT: Duration = Duration::from_secs(1);

impl Abort {
    pub(crate) fn new(socket: &TcpStream) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self { socket: socket.try_clone()?, aborted: AtomicBool::new(false), busy: AtomicBool::new(false) }))
    }

    /// Shuts the socket, so any later call fails at once, and aborts the call in progress. A
    /// call that passed its check just before the abort may reach the socket only after
    /// `CancelIoEx` ran; it is aborted again until it returns.
    pub(crate) fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        let _ = self.socket.shutdown(Shutdown::Both);
        #[cfg(windows)]
        {
            let until = std::time::Instant::now() + ABORT_WAIT;
            loop {
                abort_blocked_io(&self.socket);
                if !self.busy.load(Ordering::SeqCst) || std::time::Instant::now() >= until {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// Runs `call` on the socket unless it was aborted; `abort` sees it as busy meanwhile.
    pub(crate) fn guard<T>(&self, call: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
        self.busy.store(true, Ordering::SeqCst);
        let result = if self.aborted.load(Ordering::SeqCst) {
            Err(std::io::Error::new(ErrorKind::ConnectionAborted, "the request was cancelled"))
        } else {
            call()
        };
        self.busy.store(false, Ordering::SeqCst);
        result
    }
}

/// Windows does not wake a read or write blocked on a socket that was shut down until the peer
/// answers, which a stalled host never does; this aborts it at once, through any handle of the
/// socket.
#[cfg(windows)]
pub(super) fn abort_blocked_io(socket: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    unsafe extern "system" {
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

pub(crate) fn probe_stream(
    request: &Request<'_>,
    cancel: &Cancel,
    timeout: Duration,
    headers: &mut HeaderHook<'_>,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), Error>,
) -> Result<u64, Error> {
    let deadline = Deadline::At(std::time::Instant::now() + timeout);
    let head_str = request_head(request, None);
    let too_large = || Error::Malformed(format!("host response exceeds the {} byte limit", request.body_limit));
    connection(request, &[cancel], deadline, Some(write_timeout(request)), &head_str, None, |stream, peer| {
        stream.flush().map_err(io_tls_error)?;
        let mut reader = Reader { stream, buffer: Vec::new(), eof: false };
        let head = super::wire::read_final_head(&mut reader)?;
        headers(head.status, &head.headers, peer)?;
        let mut total = 0usize;
        if head.header("Transfer-Encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
            loop {
                let line = reader.line()?;
                let size_text = line.split(';').next().unwrap_or_default().trim();
                let mut left = size_text.bytes().all(|b| b.is_ascii_hexdigit())
                    .then(|| usize::from_str_radix(size_text, 16).ok()).flatten()
                    .ok_or_else(|| Error::Malformed("invalid chunk size".into()))?;
                if left == 0 { break; }
                if left > request.body_limit.saturating_sub(total) { return Err(too_large()); }
                while left > 0 {
                    if reader.buffer.is_empty() && !reader.fill()? {
                        return Err(Error::Malformed("connection closed before the HTTP body ended".into()));
                    }
                    let take = left.min(reader.buffer.len());
                    on_chunk(&reader.buffer[..take])?;
                    reader.buffer.drain(..take);
                    total += take;
                    left -= take;
                }
                if !reader.line()?.is_empty() { return Err(Error::Malformed("invalid chunk terminator".into())); }
            }
        } else if let Some(len) = head.content_length()? {
            let len = usize::try_from(len).ok().filter(|l| *l <= request.body_limit).ok_or_else(too_large)?;
            while total < len {
                if reader.buffer.is_empty() && !reader.fill()? {
                    return Err(Error::Malformed("connection closed before the HTTP body ended".into()));
                }
                let take = (len - total).min(reader.buffer.len());
                on_chunk(&reader.buffer[..take])?;
                reader.buffer.drain(..take);
                total += take;
            }
        } else {
            loop {
                if !reader.buffer.is_empty() {
                    let take = reader.buffer.len();
                    if take > request.body_limit.saturating_sub(total) { return Err(too_large()); }
                    let chunk = std::mem::take(&mut reader.buffer);
                    on_chunk(&chunk)?;
                    total += take;
                }
                if !reader.fill()? { break; }
            }
        }
        Ok(total as u64)
    })
}

fn send_inner(request: &Request<'_>, cancel: &Cancel, body: Option<Body<'_>>, timeout: Option<Duration>, headers: &mut HeaderHook<'_>) -> Result<Response, Error> {
    let deadline = timeout
        .or_else(|| {
            request
                .timeouts
                .read
                .map(|read| request.timeouts.connect.saturating_add(read))
        })
        .map_or(Deadline::None, |limit| {
            Deadline::At(std::time::Instant::now() + limit)
        });
    let head = request_head(request, body);
    connection(request, &[cancel], deadline, Some(write_timeout(request)), &head, None, |stream, peer| {
        if let Some(body) = body { stream.write_all(body.bytes).map_err(io_tls_error)?; }
        stream.flush().map_err(io_tls_error)?;
        read_response_with_headers(stream, request.body_limit, &mut |status, fields: &[(String, String)]| headers(status, fields, peer))
    })
}

/// How long one write may block on the buffered requests.
pub(super) fn write_timeout(request: &Request<'_>) -> Duration {
    request.timeouts.connect.max(Duration::from_secs(5))
}

/// A connected stream, plain or TLS.
trait Stream: Read + Write {}

impl<T: Read + Write> Stream for T {}

/// How long a request may take, beyond the timeout of each call.
#[derive(Clone, Copy)]
enum Deadline {
    None,
    /// The whole request, connecting included, ends by then.
    At(std::time::Instant),
    /// A transfer fails once nothing moved for this long, whichever calls waited.
    Idle(Duration),
}

impl Deadline {
    /// The read timeout, as the most a transfer may stall.
    fn idle(request: &Request<'_>) -> Self {
        request.timeouts.read.map_or(Deadline::None, Deadline::Idle)
    }
}

/// Connects, sends `head` and hands the stream to `exchange`, which writes any body and reads
/// the answer; any of `cancels` can abort it from another thread. Whatever fails once one is
/// set (a reset, or a body cut short by the shutdown) is `Error::Cancelled`.
fn connection<T>(request: &Request<'_>, cancels: &[&Cancel], deadline: Deadline, write_timeout: Option<Duration>,
    head: &str, continue_by: Option<&ContinueBy>, exchange: impl FnOnce(&mut dyn Stream, SocketAddr) -> Result<T, Error>) -> Result<T, Error> {
    let cancelled = || cancels.iter().any(|cancel| cancel.is_cancelled());
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let connect_by = match deadline { Deadline::At(at) => Some(at), _ => None };
    let result = connect_with_cancels(request.host, request.port, request.timeouts.connect, connect_by, cancels).and_then(|tcp| {
        let abort = Abort::new(&tcp)?;
        let mut ids = Vec::with_capacity(cancels.len());
        let result = cancels
            .iter()
            .try_for_each(|cancel| cancel.register(&abort).map(|id| ids.push(id)))
            .and_then(|()| open(request, tcp, abort, deadline, write_timeout, head, continue_by))
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

pub(crate) fn connect(host: &Host, port: u16, timeout: Duration, deadline: Option<std::time::Instant>) -> Result<TcpStream, Error> {
    connect_with_cancels(host, port, timeout, deadline, &[])
}

pub(crate) fn connect_with_cancels(host: &Host, port: u16, timeout: Duration, deadline: Option<std::time::Instant>, cancels: &[&Cancel]) -> Result<TcpStream, Error> {
    connect_each_with(&host.socket_addrs(port)?, timeout, deadline, cancels, |address, budget| {
        connect_timeout_cancellable(address, budget, cancels)
    })
}

/// Non-blocking TCP connect that checks `cancels` every ~15 ms so cancelling a poll or request
/// never blocks for the full connect timeout on a silent/blackholed address.
fn connect_timeout_cancellable(
    address: &SocketAddr,
    timeout: Duration,
    cancels: &[&Cancel],
) -> std::io::Result<TcpStream> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    let is_cancelled = || cancels.iter().any(|cancel| cancel.is_cancelled());
    if is_cancelled() {
        return Err(std::io::Error::new(ErrorKind::Interrupted, "cancelled"));
    }
    let socket = Socket::new(Domain::for_address(*address), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_nonblocking(true)?;
    match socket.connect(&SockAddr::from(*address)) {
        Ok(()) => {}
        Err(err) if is_connect_in_progress(&err) => {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if is_cancelled() {
                    return Err(std::io::Error::new(ErrorKind::Interrupted, "cancelled"));
                }
                let Some(remaining) = deadline
                    .checked_duration_since(std::time::Instant::now())
                    .filter(|left| !left.is_zero())
                else {
                    return Err(ErrorKind::TimedOut.into());
                };
                match poll_connect_ready(&socket, remaining.min(Duration::from_millis(15)))? {
                    ConnectPoll::Pending => continue,
                    ConnectPoll::Ready => {
                        if let Some(err) = socket.take_error()? {
                            return Err(err);
                        }
                        break;
                    }
                    ConnectPoll::Failed => {
                        return Err(socket
                            .take_error()?
                            .unwrap_or_else(|| ErrorKind::ConnectionRefused.into()));
                    }
                }
            }
        }
        Err(err) => return Err(err),
    }
    if is_cancelled() {
        return Err(std::io::Error::new(ErrorKind::Interrupted, "cancelled"));
    }
    socket.set_nonblocking(false)?;
    Ok(socket.into())
}

enum ConnectPoll {
    Pending,
    Ready,
    Failed,
}

fn is_connect_in_progress(err: &std::io::Error) -> bool {
    if err.kind() == ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Networking::WinSock::{WSAEINPROGRESS, WSAEWOULDBLOCK};
        matches!(err.raw_os_error(), Some(code) if code == WSAEWOULDBLOCK || code == WSAEINPROGRESS)
    }
    #[cfg(unix)]
    {
        err.raw_os_error() == Some(libc::EINPROGRESS)
    }
    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

#[cfg(windows)]
fn poll_connect_ready(socket: &socket2::Socket, timeout: Duration) -> std::io::Result<ConnectPoll> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{FD_SET, SOCKET_ERROR, TIMEVAL, select};
    let raw = socket.as_raw_socket() as usize;
    let mut write_set = FD_SET {
        fd_count: 1,
        fd_array: [0; 64],
    };
    write_set.fd_array[0] = raw;
    let mut except_set = write_set;
    let tv = TIMEVAL {
        tv_sec: timeout.as_secs().min(i32::MAX as u64) as i32,
        tv_usec: timeout.subsec_micros() as i32,
    };
    // SAFETY: `write_set`, `except_set` and `tv` are valid locals; `socket` stays open.
    let rc = unsafe {
        select(
            0,
            std::ptr::null_mut(),
            &mut write_set,
            &mut except_set,
            &tv,
        )
    };
    if rc == SOCKET_ERROR {
        Err(std::io::Error::last_os_error())
    } else if rc == 0 {
        Ok(ConnectPoll::Pending)
    } else if except_set.fd_count > 0 {
        Ok(ConnectPoll::Failed)
    } else {
        Ok(ConnectPoll::Ready)
    }
}

#[cfg(unix)]
fn poll_connect_ready(socket: &socket2::Socket, timeout: Duration) -> std::io::Result<ConnectPoll> {
    use std::os::unix::io::AsRawFd;
    let mut pfd = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let ms = timeout.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
    // SAFETY: `pfd` points to one valid entry and `socket` remains open during the call.
    let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == ErrorKind::Interrupted {
            Ok(ConnectPoll::Pending)
        } else {
            Err(err)
        }
    } else if rc == 0 {
        Ok(ConnectPoll::Pending)
    } else if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        Ok(ConnectPoll::Failed)
    } else {
        Ok(ConnectPoll::Ready)
    }
}

#[cfg(not(any(windows, unix)))]
fn poll_connect_ready(_socket: &socket2::Socket, timeout: Duration) -> std::io::Result<ConnectPoll> {
    std::thread::sleep(timeout);
    Ok(ConnectPoll::Ready)
}

/// Server name for rustls: uses `host.name` when it is a valid DNS name or IP literal, and
/// falls back to the connected `peer.ip()` for LAN/NetBIOS names (such as `GAMING_PC`) that
/// rustls rejects as DNS names. Certificate identity is always enforced by the pinned cert.
pub(super) fn tls_server_name(host: &Host, peer: SocketAddr) -> ServerName<'static> {
    ServerName::try_from(host.name.clone()).unwrap_or_else(|_| ServerName::from(peer.ip()))
}

/// Tries the addresses in turn, each for `timeout` but never past `deadline`; a deadline that
/// runs out before an address answers fails with `TimedOut`.
#[cfg_attr(not(test), allow(dead_code))]
fn connect_each<S>(addresses: &[SocketAddr], timeout: Duration, deadline: Option<std::time::Instant>,
    attempt: impl FnMut(&SocketAddr, Duration) -> std::io::Result<S>) -> Result<S, Error> {
    connect_each_with(addresses, timeout, deadline, &[], attempt)
}

fn connect_each_with<S>(addresses: &[SocketAddr], timeout: Duration, deadline: Option<std::time::Instant>, cancels: &[&Cancel],
    mut attempt: impl FnMut(&SocketAddr, Duration) -> std::io::Result<S>) -> Result<S, Error> {
    let mut last = None;
    for address in addresses {
        if cancels.iter().any(|cancel| cancel.is_cancelled()) {
            return Err(Error::Cancelled);
        }
        let budget = match deadline {
            None => timeout,
            Some(deadline) => match deadline.checked_duration_since(std::time::Instant::now()).filter(|left| !left.is_zero()) {
                Some(left) => timeout.min(left),
                None => return Err(Error::Io(ErrorKind::TimedOut.into())),
            },
        };
        match attempt(address, budget) {
            Ok(stream) => return Ok(stream),
            Err(_) if cancels.iter().any(|cancel| cancel.is_cancelled()) => {
                return Err(Error::Cancelled);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(Error::Io(
        last.unwrap_or_else(|| ErrorKind::NotFound.into()),
    ))
}

/// Sets the timeouts, completes any TLS handshake, writes `head` and arms `continue_by`.
fn open(request: &Request<'_>, tcp: TcpStream, abort: Arc<Abort>, deadline: Deadline, write_timeout: Option<Duration>,
    head: &str, continue_by: Option<&ContinueBy>) -> Result<(Box<dyn Stream>, SocketAddr), Error> {
    let peer = tcp.peer_addr()?;
    tcp.set_read_timeout(request.timeouts.read)?;
    tcp.set_write_timeout(write_timeout)?;
    tcp.set_nodelay(true)?;
    let (deadline, idle) = match deadline {
        Deadline::None => (None, None),
        Deadline::At(at) => (Some(at), None),
        Deadline::Idle(idle) => (Some(std::time::Instant::now() + idle), Some(idle)),
    };
    let tcp = DeadlineSocket { socket: tcp, abort, deadline, idle, read_timeout: request.timeouts.read, write_timeout,
        continue_by: continue_by.cloned(), capped: false };
    let mut stream: Box<dyn Stream> = match &request.tls {
        None => Box::new(tcp),
        Some(config) => {
            let name = tls_server_name(request.host, peer);
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
    if let Some(continue_by) = continue_by {
        continue_by.set(Some(std::time::Instant::now() + CONTINUE_WAIT));
    }
    Ok((stream, peer))
}

fn request_head(request: &Request<'_>, body: Option<Body<'_>>) -> String {
    head(request, body.map(|body| (body.content_type, body.bytes.len() as u64)))
}

/// A GET, or a POST of `(media type, length)`.
fn head(request: &Request<'_>, body: Option<(&str, u64)>) -> String {
    head_with(if body.is_some() { "POST" } else { "GET" }, request, body)
}

fn head_with(method: &str, request: &Request<'_>, body: Option<(&str, u64)>) -> String {
    let mut head = format!(
        "{method} /{} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nUser-Agent: {}\r\n",
        request.target,
        request.host.url_host(),
        request.port,
        user_agent()
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
    /// Every read and write goes through it, so a cancel ends them all.
    abort: Arc<Abort>,
    deadline: Option<std::time::Instant>,
    /// Moves `deadline` this far ahead after every byte.
    idle: Option<Duration>,
    read_timeout: Option<Duration>,
    write_timeout: Option<Duration>,
    /// While armed, reads end by then with [`ContinueElapsed`].
    continue_by: Option<ContinueBy>,
    /// Whether the last read had its timeout cut short by `continue_by`.
    capped: bool,
}

impl DeadlineSocket {
    fn remaining(&self) -> std::io::Result<Option<Duration>> {
        self.deadline.map(|deadline| deadline.checked_duration_since(std::time::Instant::now())
            .filter(|duration| !duration.is_zero()).ok_or_else(|| ErrorKind::TimedOut.into())).transpose()
    }

    /// Moves the idle deadline once a call moved at least `whole` bytes: any read, but only a
    /// write the socket took whole. A blocking write comes back short only when its timeout ran
    /// out with part of it sent, as Linux reports it (Windows fails it). The space freed during
    /// that wait then takes the next writes at once: counting the short write would let them
    /// restart the timeout, again and again, for a host that reads nothing.
    fn moved(&mut self, result: std::io::Result<usize>, whole: usize) -> std::io::Result<usize> {
        if let (Ok(moved), Some(idle)) = (&result, self.idle)
            && *moved > 0
            && *moved >= whole
        {
            self.deadline = Some(std::time::Instant::now() + idle);
        }
        result
    }
}

impl Read for DeadlineSocket {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let left = self.remaining()?;
        let wait = self.continue_by.as_ref().and_then(|by| by.get())
            .map(|by| by.saturating_duration_since(std::time::Instant::now()));
        if wait.is_some_and(|wait| wait.is_zero()) {
            return Err(continue_elapsed());
        }
        let limit = [self.read_timeout, left].into_iter().flatten().min();
        let capped = wait.filter(|wait| limit.is_none_or(|limit| *wait < limit));
        if left.is_some() || capped.is_some() || self.capped {
            self.socket.set_read_timeout(capped.or(limit))?;
            self.capped = capped.is_some();
        }
        let read = self.abort.guard(|| self.socket.read(bytes));
        if capped.is_some() && matches!(&read, Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)) {
            return Err(continue_elapsed());
        }
        self.moved(read, 1)
    }
}

impl Write for DeadlineSocket {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(left) = self.remaining()? {
            self.socket.set_write_timeout(Some(self.write_timeout.map_or(left, |timeout| timeout.min(left))))?;
        }
        let written = self.abort.guard(|| self.socket.write(bytes));
        self.moved(written, bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { self.socket.flush() }
}

pub use super::upgrade::Upgraded;
pub(crate) use super::upgrade::upgrade;

#[cfg(test)]
mod tests;
