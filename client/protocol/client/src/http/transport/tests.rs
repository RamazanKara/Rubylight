//! Unit tests for HTTP transport wire parsing, probing, hosts, and TLS upgrades.

mod transfers;

use super::super::wire::read_response;
use super::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

fn parse(raw: &[u8]) -> Result<Response, Error> {
    read_response(&mut &raw[..], 1024)
}

#[test]
fn bodies_by_length_chunks_and_close() {
    let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloextra").unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));
    let r = parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;x=1\r\nhello\r\n1\r\n!\r\n0\r\n\r\n").unwrap();
    assert_eq!(r.body, b"hello!");
    // A chunk size is hex digits only, with no sign.
    assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n+5\r\nhello\r\n0\r\n\r\n").is_err());
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
    // The body never comes: the server holds the connection until the client gives up, so
    // only the probe's deadline can end it. The hang guard is far past that deadline.
    let hang_guard = Duration::from_secs(30);
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(hang_guard)).unwrap();
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
    // Long enough that a loaded PC still gets the headers in; 150 ms was not.
    let result = probe(&Request { host: &host, port, target: "probe", tls: None,
        timeouts: Timeouts { connect: Duration::from_secs(1), read: None }, body_limit: 1 },
        &Cancel::default(), Duration::from_secs(2), &mut |_, _, _| { headers_seen = true; Ok(()) });
    assert!(headers_seen, "the headers came after the probe's deadline: {result:?}");
    assert!(result.is_err(), "{result:?}");
    assert!(started.elapsed() < hang_guard / 2, "the probe waited for the server: {:?}", started.elapsed());
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
fn the_user_agent_names_the_app_version() {
    assert_eq!(user_agent_for("3.2.3"), "Nightjar/3.2.3");
    assert_eq!(user_agent_for("3.0.0-rc.3"), "Nightjar/3.0.0-rc.3");
    // A version that would end the header or split the product token is cut down to it.
    assert_eq!(user_agent_for("3.2\r\nX-Injected: 1"), "Nightjar/3.2X-Injected:1");
    assert_eq!(user_agent_for(" "), "Nightjar");
    assert_eq!(user_agent_for(""), "Nightjar");
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
    // A shared folder's upload.
    let head = head_with("PUT", &request, Some(("application/octet-stream", 3)));
    assert!(head.starts_with("PUT /power-actions/v1/sleep?token=ab HTTP/1.1\r\n"), "{head}");
    assert!(head.ends_with("Content-Type: application/octet-stream\r\nContent-Length: 3\r\n\r\n"), "{head}");
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

/// Reads a request head and, like the host, lets an upload that expects it go on.
fn read_head_of(socket: &mut (impl Read + Write)) -> String {
    let head = read_head_only(socket);
    if head.contains("\r\nExpect: 100-continue\r\n") {
        socket.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").unwrap();
        socket.flush().unwrap();
    }
    head
}

fn read_head_only(socket: &mut impl Read) -> String {
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
        if self.after == 0 && let Some(mut trip) = self.trip.take() { trip(); }
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

/// Cancels `transfer` from another thread after `after`.
fn cancel_later(transfer: &Arc<Cancel>, after: Duration) -> std::thread::JoinHandle<()> {
    let transfer = transfer.clone();
    std::thread::spawn(move || { std::thread::sleep(after); transfer.cancel(); })
}

/// A TLS host with the test identity's certificate, and the client config that pins it.
fn tls_host<T: Send + 'static>(serve: impl FnOnce(StreamOwned<rustls::ServerConnection, std::net::TcpStream>) -> T + Send + 'static)
    -> (u16, Arc<ClientConfig>, std::thread::JoinHandle<T>) {
    let identity = crate::http::identity::tests::fixture();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(identity.certificate_der().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.private_key_pkcs8().to_vec()))).unwrap();
    let client = tls_config(identity.certificate_der(), identity.certificate_der(), identity.private_key_pkcs8()).unwrap();
    let (port, server) = host(move |socket| {
        let mut tls = StreamOwned::new(rustls::ServerConnection::new(Arc::new(config)).unwrap(), socket);
        while tls.conn.is_handshaking() { tls.conn.complete_io(&mut tls.sock).unwrap(); }
        serve(tls)
    });
    (port, client, server)
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
fn an_upgraded_connection_carries_writes_larger_than_the_tls_buffer() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    let sent: Vec<u8> = (0..1 << 20).map(|at: usize| (at % 251) as u8).collect();
    let length = sent.len();
    let (port, config, server) = tls_host(move |mut tls| {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            tls.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        tls.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: test/1\r\n\r\n").unwrap();
        let mut got = vec![0; length];
        tls.read_exact(&mut got).unwrap();
        // And back, as large.
        tls.write_all(&got).unwrap();
        tls.flush().unwrap();
        got
    });
    let request = Request { host: &host_name, port, target: "tunnel", tls: Some(config),
        timeouts: Timeouts { connect: Duration::from_secs(1), read: Some(Duration::from_secs(10)) }, body_limit: 1024 };
    let Ok(Ok(upgraded)) = upgrade(&request, &Cancel::default(), "test/1", &[]) else { panic!("no upgrade") };
    upgraded.write_all(&sent).unwrap();
    let mut back = vec![0; length];
    upgraded.read_exact(&mut back).unwrap();
    assert!(server.join().unwrap() == sent, "the host got all of it in order");
    assert!(back == sent, "and the client all of its answer");
}

#[test]
fn an_upgraded_connection_reads_while_a_write_waits_for_the_host() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let (port, config, server) = tls_host(move |mut tls| {
        read_head_of(&mut tls);
        tls.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: test/1\r\n\r\n").unwrap();
        tls.flush().unwrap();
        // Reads nothing more, so the client's large write fills the buffers and blocks; once
        // that write has started (its bytes arrive), sends while it waits.
        tls.sock.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let started = std::time::Instant::now();
        while tls.sock.peek(&mut [0]).unwrap_or(0) == 0 {
            assert!(started.elapsed() < Duration::from_secs(3), "the client never wrote");
            std::thread::sleep(Duration::from_millis(5));
        }
        tls.write_all(b"hello").unwrap();
        tls.flush().unwrap();
        let _ = finished.recv_timeout(Duration::from_secs(10));
    });
    let request = Request { host: &host_name, port, target: "tunnel", tls: Some(config),
        timeouts: Timeouts { connect: Duration::from_secs(1), read: Some(Duration::from_secs(10)) }, body_limit: 1024 };
    let Ok(Ok(upgraded)) = upgrade(&request, &Cancel::default(), "test/1", &[]) else { panic!("no upgrade") };
    let upgraded = Arc::new(upgraded);
    let writing = upgraded.clone();
    let writer = std::thread::spawn(move || writing.write_all(&vec![7; 64 << 20]));
    let reading = upgraded.clone();
    let (got, read) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = [0; 5];
        let _ = got.send(reading.read_exact(&mut bytes).map(|()| bytes));
    });
    let read = read.recv_timeout(Duration::from_secs(3));
    upgraded.close();
    let _ = done.send(());
    assert!(writer.join().unwrap().is_err(), "the host never read the write");
    server.join().unwrap();
    assert_eq!(read.expect("the read waited behind the blocked write").unwrap(), *b"hello");
}

#[test]
fn closing_an_upgraded_connection_ends_its_tls_cleanly_and_wakes_a_read() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    let (port, config, server) = tls_host(|mut tls| {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            tls.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        tls.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: test/1\r\n\r\n").unwrap();
        // A host that sees the end as an error tells the client its device failed.
        tls.read(&mut [0; 16]).map_err(|e| e.kind())
    });
    let request = Request { host: &host_name, port, target: "tunnel", tls: Some(config),
        timeouts: Timeouts { connect: Duration::from_secs(1), read: Some(Duration::from_secs(10)) }, body_limit: 1024 };
    let Ok(Ok(upgraded)) = upgrade(&request, &Cancel::default(), "test/1", &[]) else { panic!("no upgrade") };
    let upgraded = Arc::new(upgraded);
    let reading = upgraded.clone();
    let reader = std::thread::spawn(move || reading.read(&mut [0; 16]).map_err(|e| e.kind()));
    std::thread::sleep(Duration::from_millis(100));
    let started = std::time::Instant::now();
    upgraded.close();
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(server.join().unwrap(), Ok(0), "the host reads a close_notify");
    assert!(matches!(reader.join().unwrap(), Ok(0) | Err(_)), "the blocked read returns");
}

#[test]
fn tls_server_name_falls_back_to_peer_ip_for_non_dns_lan_names() {
    let v4_peer: SocketAddr = "127.0.0.1:47984".parse().unwrap();
    let v6_peer: SocketAddr = "[::1]:47984".parse().unwrap();
    for bad in ["-bad_host", "GAMING PC", "bad..host", "host_name-"] {
        let host = Host::parse(bad).unwrap();
        assert_eq!(
            tls_server_name(&host, v4_peer),
            ServerName::from(v4_peer.ip()),
            "host {bad}"
        );
        assert_eq!(
            tls_server_name(&host, v6_peer),
            ServerName::from(v6_peer.ip()),
            "host {bad}"
        );
    }
    // Valid DNS names and IP literals keep their direct ServerName.
    let dns = Host::parse("gaming-pc.local").unwrap();
    assert_eq!(
        tls_server_name(&dns, v4_peer),
        ServerName::try_from("gaming-pc.local".to_owned()).unwrap()
    );

    // Real TLS handshake succeeds via `open` even when `request.host.name` is rejected as a DNS name.
    let invalid_dns = Host::parse("-bad_host").unwrap();
    let (port, config, server) = tls_host(|mut tls| {
        let head = read_head_of(&mut tls);
        tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .unwrap();
        tls.flush().unwrap();
        head
    });
    let tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let abort = Abort::new(&tcp).unwrap();
    let request = Request {
        host: &invalid_dns,
        port,
        target: "serverinfo",
        tls: Some(config),
        timeouts: Timeouts {
            connect: Duration::from_secs(2),
            read: Some(Duration::from_secs(2)),
        },
        body_limit: 1024,
    };
    let head = request_head(&request, None);
    let (mut stream, _peer) = open(
        &request,
        tcp,
        abort,
        Deadline::None,
        Some(Duration::from_secs(2)),
        &head,
        None,
    )
    .unwrap();
    stream.flush().unwrap();
    let response = read_response(&mut *stream, 1024).unwrap();
    assert_eq!((response.status, response.body.as_slice()), (200, &b"ok"[..]));
    assert!(server.join().unwrap().contains("Host: -bad_host:"));
}

#[test]
fn cancellable_connect_handles_loopback_refusal_and_cancellation() {
    // 1. Live loopback listener connects cleanly and produces a working blocking socket.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        sock.write_all(b"hi").unwrap();
    });
    let host = Host::parse("127.0.0.1").unwrap();
    let cancel = Cancel::default();
    let mut tcp = connect_with_cancels(&host, port, Duration::from_secs(2), None, &[&cancel]).unwrap();
    let mut buf = [0u8; 2];
    tcp.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"hi");
    server.join().unwrap();

    // 2. Pre-cancelled token aborts immediately without blocking.
    cancel.cancel();
    let err = connect_with_cancels(&host, port, Duration::from_secs(5), None, &[&cancel]).unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");

    // 3. Closed loopback port fails with ConnectionRefused (not TimedOut) and can be cancelled
    //    mid-SYN well before Windows' ~2 s loopback SYN retry finishes.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
    let mid_cancel = Arc::new(Cancel::default());
    let trigger = cancel_later(&mid_cancel, Duration::from_millis(50));
    let started = std::time::Instant::now();
    let err = connect_with_cancels(&host, closed_port, Duration::from_secs(5), None, &[&mid_cancel])
        .unwrap_err();
    trigger.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "mid-connect cancellation took too long: {:?}",
        started.elapsed()
    );
    assert!(
        matches!(err, Error::Cancelled)
            || matches!(&err, Error::Io(e) if e.kind() == ErrorKind::ConnectionRefused || e.kind() == ErrorKind::Interrupted),
        "unexpected error on cancelled connect: {err:?}"
    );

    let fresh = Cancel::default();
    let started = std::time::Instant::now();
    let err = connect_with_cancels(&host, closed_port, Duration::from_secs(5), None, &[&fresh])
        .unwrap_err();
    assert!(
        matches!(&err, Error::Io(e) if e.kind() == ErrorKind::ConnectionRefused),
        "expected ConnectionRefused on closed port, got {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "closed port connect timed out instead of detecting refusal: {:?}",
        started.elapsed()
    );
}
