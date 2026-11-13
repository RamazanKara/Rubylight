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
        Upload { reader: &mut reader, length: sent.len() as u64, content_type: "application/octet-stream", method: "POST" },
        &mut recorder(&mut calls), &Cancel::default()).unwrap();
    assert_eq!((response.status, response.body.as_slice()), (201, &b"{}"[..]));
    let (head, body) = server.join().unwrap();
    assert!(head.starts_with("POST /files?name=a HTTP/1.1\r\n"), "{head}");
    assert!(head.contains("Content-Type: application/octet-stream\r\nContent-Length: 5242887\r\n"), "{head}");
    assert!(head.ends_with("\r\nExpect: 100-continue\r\n\r\n"), "{head}");
    assert!(body == sent);
    assert_progress(&calls, sent.len() as u64);
}

#[test]
fn an_upload_refused_by_its_head_sends_no_body_and_returns_the_refusal() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    let refuse = |answer: &'static [u8]| host(move |mut socket| {
        read_head_only(&mut socket);
        socket.write_all(answer).unwrap();
        read_rest(&mut socket).len()
    });
    let send = |port| upload(&transfer(&host_name, port, "files"), &Cancel::default(),
        Upload { reader: &mut &file(1024 * 1024)[..], length: 1024 * 1024, content_type: "image/png", method: "POST" },
        &mut |_, _| {}, &Cancel::default());
    // Interim answers other than 100 are skipped.
    let (port, server) = refuse(b"HTTP/1.1 103 Early Hints\r\n\r\nHTTP/1.1 413 Payload Too Large\r\nContent-Length: 3\r\n\r\nbig");
    let response = send(port).unwrap();
    assert_eq!((response.status, response.body.as_slice()), (413, &b"big"[..]));
    assert_eq!(server.join().unwrap(), 0);
    // A success for a body it never got is not one.
    let (port, server) = refuse(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    let result = send(port);
    assert!(matches!(&result, Err(Error::Malformed(m)) if m.contains("before the upload was sent")), "{result:?}");
    assert_eq!(server.join().unwrap(), 0);
}

#[test]
fn a_host_that_never_says_continue_gets_the_body_after_the_wait() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    let sent = file(300 * 1024);
    let length = sent.len() as u64;
    for tls in [false, true] {
        fn serve(socket: &mut (impl Read + Write), length: usize) -> Vec<u8> {
            let head = read_head_only(socket);
            assert!(head.contains("\r\nExpect: 100-continue\r\n"), "{head}");
            let mut body = vec![0; length];
            socket.read_exact(&mut body).unwrap();
            socket.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\n{}").unwrap();
            socket.flush().unwrap();
            body
        }
        let (port, config, server) = if tls {
            let (port, config, server) = tls_host(move |mut tls| serve(&mut tls, length as usize));
            (port, Some(config), server)
        } else {
            let (port, server) = host(move |mut socket| serve(&mut socket, length as usize));
            (port, None, server)
        };
        let request = Request { tls: config, ..transfer(&host_name, port, "files") };
        let started = std::time::Instant::now();
        let response = upload(&request, &Cancel::default(),
            Upload { reader: &mut &sent[..], length, content_type: "application/octet-stream", method: "PUT" },
            &mut |_, _| {}, &Cancel::default()).unwrap();
        let took = started.elapsed();
        assert_eq!((response.status, response.body.as_slice()), (201, &b"{}"[..]), "tls {tls}");
        assert!(server.join().unwrap() == sent, "tls {tls}");
        // After the wait, and well inside the 10 s read timeout.
        assert!(took >= CONTINUE_WAIT - Duration::from_millis(100) && took < Duration::from_secs(5), "tls {tls}: {took:?}");
    }
}

#[test]
fn a_continue_wait_cuts_only_reads_while_armed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    let continue_by = ContinueBy::default();
    let abort = Abort::new(&socket).unwrap();
    let mut tcp = DeadlineSocket { socket, abort, deadline: None, idle: None, read_timeout: Some(Duration::from_secs(10)),
        write_timeout: None, continue_by: Some(continue_by.clone()), capped: false };
    continue_by.set(Some(std::time::Instant::now() + Duration::from_millis(200)));
    let started = std::time::Instant::now();
    let error = tcp.read(&mut [0; 4]).unwrap_err();
    assert!(is_continue_elapsed(&Error::Io(error)), "a wait that ran out is not a timeout");
    assert!(started.elapsed() < Duration::from_secs(5));
    // Disarmed, the read timeout is the socket's again.
    continue_by.set(None);
    peer.write_all(b"late").unwrap();
    let mut bytes = [0; 4];
    tcp.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"late");
    assert!(!tcp.capped);
    assert_eq!(tcp.socket.read_timeout().unwrap(), Some(Duration::from_secs(10)));
}

#[test]
fn a_short_or_stopped_upload_is_an_error_not_a_short_body() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    // The file ends early.
    let (port, server) = host(|mut socket| { read_head_of(&mut socket); read_rest(&mut socket).len() });
    let result = upload(&transfer(&host_name, port, "files"), &Cancel::default(),
        Upload { reader: &mut &file(1000)[..], length: 4000, content_type: "application/octet-stream", method: "POST" },
        &mut |_, _| {}, &Cancel::default());
    assert!(matches!(&result, Err(Error::InvalidArgument(m)) if m == "the upload ended after 1000 of 4000 bytes"), "{result:?}");
    assert_eq!(server.join().unwrap(), 1000);
    // Stopped mid-transfer: no further chunk goes out.
    let (port, server) = host(|mut socket| { read_head_of(&mut socket); read_rest(&mut socket).len() });
    let stop = Cancel::default();
    let data = file(4 * 1024 * 1024);
    let mut reader = Trip::new(&data[..], 1024 * 1024, || stop.cancel());
    let result = upload(&transfer(&host_name, port, "files"), &Cancel::default(),
        Upload { reader: &mut reader, length: 4 * 1024 * 1024, content_type: "application/octet-stream", method: "POST" },
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
        Upload { reader: &mut std::io::repeat(7).take(64 << 20), length: 64 << 20, content_type: "application/octet-stream", method: "POST" },
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

#[test]
fn a_cancel_also_ends_a_write_that_reaches_the_socket_after_it() {
    // A host that reads nothing: the write below blocks once the buffers are full.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    // Bounds the test if the cancel misses the write.
    socket.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let abort = Abort::new(&socket).unwrap();
    let mut tcp = DeadlineSocket { socket, abort: abort.clone(), deadline: None, idle: None, read_timeout: None,
        write_timeout: None, continue_by: None, capped: false };
    let cancel = Arc::new(Cancel::default());
    cancel.register(&abort).unwrap();
    let started = std::time::Instant::now();
    // The write passed its check; the cancel runs before it reaches the socket. The
    // canceller starts inside the guard, so the cancel always races a write in flight.
    let (written, ended, canceller) = abort.guard(|| {
        let canceller = {
            let cancel = cancel.clone();
            std::thread::spawn(move || { cancel.cancel(); std::time::Instant::now() })
        };
        while !abort.aborted.load(Ordering::SeqCst) { std::thread::yield_now(); }
        std::thread::sleep(Duration::from_millis(50));
        Ok(((&tcp.socket).write_all(&vec![7; 64 << 20]), std::time::Instant::now(), canceller))
    }).unwrap();
    let cancelled = canceller.join().unwrap();
    assert!(written.is_err());
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    // On Windows the cancel returns only once the write it raced has ended; elsewhere the
    // shutdown alone ends the write and the cancel does not wait for it.
    #[cfg(windows)]
    assert!(cancelled >= ended);
    #[cfg(not(windows))]
    let _ = (cancelled, ended);
    // Any later call fails before it reaches the socket.
    for error in [tcp.write(b"x").unwrap_err(), tcp.read(&mut [0; 1]).unwrap_err()] {
        assert_eq!(error.kind(), ErrorKind::ConnectionAborted, "{error}");
    }
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
fn a_host_that_stops_reading_fails_an_upload_after_one_timeout() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    let length = 64 << 20;
    // TLS buffers a write that timed out and retries it on the next write and read, a full
    // timeout each; a plain socket would wait once more for an answer.
    for tls in [true, false] {
        let stall = |_| std::thread::sleep(Duration::from_secs(3));
        let (port, config, server) = if tls {
            let (port, config, server) = tls_host(move |mut tls| { read_head_of(&mut tls); stall(tls.sock) });
            (port, Some(config), server)
        } else {
            let (port, server) = host(move |mut socket| { read_head_of(&mut socket); stall(socket) });
            (port, None, server)
        };
        let request = Request { host: &host_name, port, target: "files", tls: config,
            timeouts: Timeouts { connect: Duration::from_secs(1), read: Some(Duration::from_secs(1)) }, body_limit: 1024 };
        let mut last = 0;
        let started = std::time::Instant::now();
        let result = upload(&request, &Cancel::default(),
            Upload { reader: &mut std::io::repeat(7).take(length), length, content_type: "application/octet-stream", method: "POST" },
            &mut |done, _| last = done, &Cancel::default());
        let took = started.elapsed();
        assert!(matches!(&result, Err(Error::Io(e)) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)),
            "tls {tls}: {result:?}");
        assert!(last < length, "tls {tls}: the host took it all");
        // Under two timeouts: 1 s leaves a loaded PC room for the handshake and the rest
        // (500 ms timed the handshake out and left 450 ms, which a busy run overshot).
        assert!(took < Duration::from_millis(1900), "tls {tls}: {took:?}");
        server.join().unwrap();
    }
}

#[test]
fn only_whole_writes_and_any_read_move_the_idle_deadline() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let idle = Duration::from_secs(60);
    let start = std::time::Instant::now() + Duration::from_millis(10);
    let abort = Abort::new(&socket).unwrap();
    let mut tcp = DeadlineSocket { socket, abort, deadline: Some(start), idle: Some(idle), read_timeout: None, write_timeout: None,
        continue_by: None, capped: false };
    // What Linux returns for a write whose timeout ran out with part of it sent.
    tcp.moved(Ok(7876), 16406).unwrap();
    assert_eq!(tcp.deadline, Some(start));
    tcp.moved(Err(ErrorKind::TimedOut.into()), 1).unwrap_err();
    tcp.moved(Ok(0), 1).unwrap();
    assert_eq!(tcp.deadline, Some(start));
    tcp.moved(Ok(16406), 16406).unwrap();
    assert!(tcp.deadline.unwrap() > start + idle / 2);
    let moved = tcp.deadline;
    tcp.moved(Ok(1), 1).unwrap();
    assert!(tcp.deadline >= moved);
}

#[test]
fn a_slow_but_steady_download_is_not_idle() {
    let host_name = Host::parse("127.0.0.1").unwrap();
    let sent = file(6000);
    let body = sent.clone();
    let (port, server) = host(move |mut socket| {
        read_head_of(&mut socket);
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).unwrap();
        for piece in body.chunks(500) {
            std::thread::sleep(Duration::from_millis(100));
            socket.write_all(piece).unwrap();
        }
    });
    let request = Request { host: &host_name, port, target: "f", tls: None,
        timeouts: Timeouts { connect: Duration::from_secs(1), read: Some(Duration::from_secs(1)) }, body_limit: 1024 };
    let mut sink = Vec::new();
    // 1.2 s or more in all, past the timeout, but a byte every 100 ms: a loaded PC that
    // stretches a gap tenfold still keeps it under the timeout (150 ms against 400 ms did not).
    download(&request, &Cancel::default(), &mut sink, &mut |_, _| {}, &Cancel::default()).unwrap();
    assert!(sink == sent);
    server.join().unwrap();
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
            content_type: "application/octet-stream", method: "POST" }, 1024, &mut |_, _| {}, &Cancel::default())
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
