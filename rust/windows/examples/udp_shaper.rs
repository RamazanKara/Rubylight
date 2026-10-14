//! A loopback bottleneck for stream tests: relays a host's RTSP (TCP) and its
//! video, control and audio ports (UDP) from one loopback address to another,
//! and drains the host-to-client direction of shaped ports at a fixed link
//! rate through a bounded drop-tail queue, as a slow switch port does.
//!
//! The client connects to LISTEN (for example 127.0.0.2); the relay connects to
//! the host at UPSTREAM (127.0.0.1) from that address, so the host sees the
//! same IP for pairing, RTSP and UDP. A packet is admitted when the bytes still
//! queued or on the wire plus its own fit the queue, and leaves when the link
//! has serialized it; each datagram counts its Ethernet, IP and UDP framing.
//! Every 250 ms the counters are written to STATS as JSON.
//!
//! Usage:
//!   udp_shaper LISTEN UPSTREAM RATE_KBPS QUEUE_BYTES STATS PORT[:shape|:tcp]...
//!   udp_shaper --selftest
//! RATE_KBPS 0 relays without shaping (the control).
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::VecDeque,
    io::ErrorKind,
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Ethernet preamble, header, FCS and gap plus IPv4 and UDP, beyond the payload.
const OVERHEAD: u64 = 66;

/// The link: a FIFO that holds at most `capacity` wire bytes, serialized at `bps`.
struct Link {
    bps: u64,
    capacity: u64,
    free_at: Option<Instant>,
}

impl Link {
    fn new(bps: u64, capacity: u64) -> Self {
        Self {
            bps,
            capacity,
            free_at: None,
        }
    }
    fn wire_time(&self, bytes: u64) -> Duration {
        Duration::from_nanos(bytes.saturating_mul(8_000_000_000) / self.bps.max(1))
    }
    /// Wire bytes queued or still being serialized at `now`.
    fn backlog(&self, now: Instant) -> u64 {
        let left = self
            .free_at
            .map_or(Duration::ZERO, |at| at.saturating_duration_since(now));
        (left.as_nanos() * u128::from(self.bps) / 8_000_000_000) as u64
    }
    /// When the datagram has crossed the link, or None when the queue is full.
    fn offer(&mut self, now: Instant, payload: usize) -> Option<Instant> {
        let wire = payload as u64 + OVERHEAD;
        if self.backlog(now) + wire > self.capacity {
            return None;
        }
        let start = self.free_at.map_or(now, |at| at.max(now));
        let done = start + self.wire_time(wire);
        self.free_at = Some(done);
        Some(done)
    }
}

#[derive(Default)]
struct Counters {
    packets_in: AtomicU64,
    bytes_in: AtomicU64,
    packets_dropped: AtomicU64,
    bytes_dropped: AtomicU64,
    packets_out: AtomicU64,
    bytes_out: AtomicU64,
    max_backlog: AtomicU64,
    /// Sum of each forwarded packet's time in the queue and on the link.
    delay_us: AtomicU64,
    max_delay_us: AtomicU64,
    /// Drop events separated by at least 2 ms of admitted traffic.
    drop_bursts: AtomicU64,
}

impl Counters {
    fn json(&self) -> serde_json::Value {
        let get = |value: &AtomicU64| value.load(Ordering::Relaxed);
        let out = get(&self.packets_out);
        serde_json::json!({
            "packets_in": get(&self.packets_in),
            "bytes_in": get(&self.bytes_in),
            "packets_dropped": get(&self.packets_dropped),
            "bytes_dropped": get(&self.bytes_dropped),
            "packets_out": out,
            "bytes_out": get(&self.bytes_out),
            "max_backlog_bytes": get(&self.max_backlog),
            "mean_delay_ms": if out > 0 { get(&self.delay_us) as f64 / out as f64 / 1000. } else { 0. },
            "max_delay_ms": get(&self.max_delay_us) as f64 / 1000.,
            "drop_bursts": get(&self.drop_bursts),
        })
    }
}

fn udp(address: SocketAddr) -> Result<UdpSocket> {
    let socket = UdpSocket::bind(address).with_context(|| format!("bind {address}"))?;
    butterpollo_windows::net::configure_udp(&socket)?;
    socket2::SockRef::from(&socket).set_recv_buffer_size(16 << 20)?;
    Ok(socket)
}

fn ignorable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionReset | ErrorKind::TimedOut | ErrorKind::WouldBlock
    )
}

/// Client to host, unshaped: pings, control and input are small.
fn upward(
    listen: Arc<UdpSocket>,
    upstream: Arc<UdpSocket>,
    client: Arc<Mutex<Option<SocketAddr>>>,
) {
    let mut buffer = vec![0; 65536];
    loop {
        match listen.recv_from(&mut buffer) {
            Ok((n, from)) => {
                *client.lock().unwrap() = Some(from);
                let _ = upstream.send(&buffer[..n]);
            }
            Err(error) if ignorable(&error) => {}
            Err(error) => {
                eprintln!("relay {}: {error}", listen.local_addr().unwrap());
                return;
            }
        }
    }
}

fn downward_plain(
    listen: Arc<UdpSocket>,
    upstream: Arc<UdpSocket>,
    client: Arc<Mutex<Option<SocketAddr>>>,
    counters: Arc<Counters>,
) {
    let mut buffer = vec![0; 65536];
    loop {
        match upstream.recv(&mut buffer) {
            Ok(n) => {
                counters.packets_in.fetch_add(1, Ordering::Relaxed);
                counters.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
                let to = *client.lock().unwrap();
                if let Some(to) = to
                    && listen.send_to(&buffer[..n], to).is_ok()
                {
                    counters.packets_out.fetch_add(1, Ordering::Relaxed);
                    counters.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
                }
            }
            Err(error) if ignorable(&error) => {}
            Err(error) => {
                eprintln!("relay {}: {error}", upstream.local_addr().unwrap());
                return;
            }
        }
    }
}

/// Host to client through the link. Spins while packets are queued, so that
/// arrivals are stamped and departures sent within microseconds.
fn downward_shaped(
    listen: Arc<UdpSocket>,
    upstream: Arc<UdpSocket>,
    client: Arc<Mutex<Option<SocketAddr>>>,
    counters: Arc<Counters>,
    mut link: Link,
) -> Result<()> {
    // A switch port is never descheduled: keep the link's clock ahead of
    // builds and other load on the test machine.
    let _priority = butterpollo_windows::capture::Priority::new();
    let mut buffer = vec![0; 65536];
    let mut queue: VecDeque<(Instant, Instant, Vec<u8>)> = VecDeque::new();
    let mut last_drop: Option<Instant> = None;
    let mut last_admit = Instant::now();
    let mut blocking = false;
    loop {
        if queue.is_empty() != blocking {
            blocking = queue.is_empty();
            upstream.set_nonblocking(!blocking)?;
            if blocking {
                upstream.set_read_timeout(Some(Duration::from_millis(50)))?;
            }
        }
        match upstream.recv(&mut buffer) {
            Ok(n) => {
                let now = Instant::now();
                counters.packets_in.fetch_add(1, Ordering::Relaxed);
                counters.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
                match link.offer(now, n) {
                    Some(leaves) => {
                        counters
                            .max_backlog
                            .fetch_max(link.backlog(now), Ordering::Relaxed);
                        queue.push_back((now, leaves, buffer[..n].to_vec()));
                        last_admit = now;
                    }
                    None => {
                        counters.packets_dropped.fetch_add(1, Ordering::Relaxed);
                        counters
                            .bytes_dropped
                            .fetch_add(n as u64, Ordering::Relaxed);
                        if last_drop.is_none_or(|at| last_admit > at + Duration::from_millis(2)) {
                            counters.drop_bursts.fetch_add(1, Ordering::Relaxed);
                        }
                        last_drop = Some(now);
                    }
                }
                // Stamp every waiting arrival before sending.
                continue;
            }
            Err(error) if ignorable(&error) => {}
            Err(error) => return Err(error.into()),
        }
        let now = Instant::now();
        while queue.front().is_some_and(|(_, leaves, _)| *leaves <= now) {
            let (arrived, _, packet) = queue.pop_front().unwrap();
            let to = *client.lock().unwrap();
            if let Some(to) = to
                && listen.send_to(&packet, to).is_ok()
            {
                let delay = now.saturating_duration_since(arrived).as_micros() as u64;
                counters.packets_out.fetch_add(1, Ordering::Relaxed);
                counters
                    .bytes_out
                    .fetch_add(packet.len() as u64, Ordering::Relaxed);
                counters.delay_us.fetch_add(delay, Ordering::Relaxed);
                counters.max_delay_us.fetch_max(delay, Ordering::Relaxed);
            }
        }
        if !queue.is_empty() {
            std::hint::spin_loop();
        }
    }
}

fn copy(mut from: TcpStream, mut to: TcpStream) {
    let _ = std::io::copy(&mut from, &mut to);
    let _ = to.shutdown(std::net::Shutdown::Write);
}

fn tcp(listen: SocketAddr, upstream: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(listen).with_context(|| format!("bind {listen}"))?;
    thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let Ok(host) = TcpStream::connect(upstream) else {
                continue;
            };
            let _ = (client.set_nodelay(true), host.set_nodelay(true));
            let (Ok(client_reader), Ok(host_reader)) = (client.try_clone(), host.try_clone())
            else {
                continue;
            };
            thread::spawn(move || copy(client_reader, host));
            thread::spawn(move || copy(host_reader, client));
        }
    });
    Ok(())
}

struct Relay {
    ports: Vec<(u16, bool, Arc<Counters>)>,
}

fn relay(
    listen: IpAddr,
    upstream: IpAddr,
    bps: u64,
    capacity: u64,
    ports: &[String],
) -> Result<Relay> {
    let mut relay = Relay { ports: Vec::new() };
    for spec in ports {
        let (port, kind) = spec.split_once(':').unwrap_or((spec, "udp"));
        let port: u16 = port.parse().with_context(|| format!("port {spec}"))?;
        if kind == "tcp" {
            tcp((listen, port).into(), (upstream, port).into())?;
            continue;
        }
        ensure!(
            matches!(kind, "udp" | "shape"),
            "port kind must be udp, shape or tcp: {spec}"
        );
        let shaped = kind == "shape" && bps > 0;
        let listen_socket = Arc::new(udp((listen, port).into())?);
        let upstream_socket = Arc::new(udp((upstream, 0).into())?);
        upstream_socket.connect((upstream, port))?;
        let client = Arc::new(Mutex::new(None));
        let counters = Arc::new(Counters::default());
        relay.ports.push((port, shaped, counters.clone()));
        let (l, u, c) = (
            listen_socket.clone(),
            upstream_socket.clone(),
            client.clone(),
        );
        thread::spawn(move || upward(l, u, c));
        thread::spawn(move || {
            if shaped {
                if let Err(error) = downward_shaped(
                    listen_socket,
                    upstream_socket,
                    client,
                    counters,
                    Link::new(bps, capacity),
                ) {
                    eprintln!("shaped relay {port}: {error:#}");
                }
            } else {
                downward_plain(listen_socket, upstream_socket, client, counters);
            }
        });
    }
    Ok(relay)
}

impl Relay {
    fn json(&self, started: Instant, bps: u64, capacity: u64) -> serde_json::Value {
        serde_json::json!({
            "seconds": started.elapsed().as_secs_f64(),
            "rate_bps": bps,
            "queue_bytes": capacity,
            "overhead_bytes": OVERHEAD,
            "ports": self.ports.iter().map(|(port, shaped, counters)| {
                let mut value = counters.json();
                value["port"] = (*port).into();
                value["shaped"] = (*shaped).into();
                value
            }).collect::<Vec<_>>(),
        })
    }
}

/// Accuracy check on loopback: a paced sender, the shaper and a receiver.
fn selftest() -> Result<()> {
    struct Case {
        name: &'static str,
        input_bps: u64,
        burst_bytes: usize,
        capacity: u64,
    }
    let link_bps = 100_000_000;
    let payload = 1440usize;
    let cases = [
        Case {
            name: "steady 80 Mbps below the link",
            input_bps: 80_000_000,
            burst_bytes: 0,
            capacity: 65536,
        },
        Case {
            name: "steady 200 Mbps above the link",
            input_bps: 200_000_000,
            burst_bytes: 0,
            capacity: 65536,
        },
        Case {
            name: "250 KB bursts into 16 KiB",
            input_bps: 0,
            burst_bytes: 250_000,
            capacity: 16384,
        },
        Case {
            name: "250 KB bursts into 64 KiB",
            input_bps: 0,
            burst_bytes: 250_000,
            capacity: 65536,
        },
        Case {
            name: "250 KB bursts into 256 KiB",
            input_bps: 0,
            burst_bytes: 250_000,
            capacity: 262144,
        },
    ];
    let mut failures = 0;
    for case in cases {
        // The relay listens on the host's port number at its own address.
        let host = udp("127.0.0.1:0".parse()?)?;
        let host_port = host.local_addr()?.port();
        let relay = relay(
            "127.0.0.2".parse()?,
            "127.0.0.1".parse()?,
            link_bps,
            case.capacity,
            &[format!("{host_port}:shape")],
        )?;
        let client = udp("127.0.0.3:0".parse()?)?;
        client.send_to(b"ping", ("127.0.0.2", host_port))?;
        host.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut buffer = vec![0; 65536];
        let (_, upstream) = host
            .recv_from(&mut buffer)
            .context("relay did not forward the ping")?;
        client.set_read_timeout(Some(Duration::from_millis(300)))?;
        let receiver = thread::spawn(move || {
            let mut arrivals = Vec::new();
            let mut buffer = vec![0; 65536];
            while let Ok(n) = client.recv(&mut buffer) {
                arrivals.push((Instant::now(), n));
            }
            arrivals
        });
        let duration = Duration::from_secs(3);
        let started = Instant::now();
        let mut sent = 0u64;
        let mut stamps = Vec::new();
        let packet = vec![0x5a; payload];
        if let Some(nanos) =
            ((payload as u64 + OVERHEAD) * 8_000_000_000).checked_div(case.input_bps)
        {
            let interval = Duration::from_nanos(nanos);
            let mut due = started;
            while started.elapsed() < duration {
                while Instant::now() < due {
                    std::hint::spin_loop();
                }
                host.send_to(&packet, upstream)?;
                stamps.push(Instant::now());
                sent += 1;
                due += interval;
            }
        } else {
            // A keyframe-sized burst every 100 ms, back to back.
            let mut due = started;
            while started.elapsed() < duration {
                while Instant::now() < due {
                    thread::sleep(Duration::from_millis(1));
                }
                for _ in 0..case.burst_bytes.div_ceil(payload) {
                    host.send_to(&packet, upstream)?;
                    stamps.push(Instant::now());
                    sent += 1;
                }
                due += Duration::from_millis(100);
            }
        }
        let arrivals = receiver.join().unwrap();
        let counters = &relay.ports[0].2;
        let get = |value: &AtomicU64| value.load(Ordering::Relaxed);
        let (input, dropped, out) = (
            get(&counters.packets_in),
            get(&counters.packets_dropped),
            get(&counters.packets_out),
        );
        let received = arrivals.len() as u64;
        // Throughput from the first to the last arrival.
        let busy_bps = if let (Some(first), Some(last)) = (arrivals.first(), arrivals.last()) {
            let bytes: u64 = arrivals[1..]
                .iter()
                .map(|(_, n)| *n as u64 + OVERHEAD)
                .sum();
            bytes as f64 * 8. / last.0.duration_since(first.0).as_secs_f64().max(1e-9)
        } else {
            0.
        };
        let mut gaps: Vec<f64> = arrivals
            .windows(2)
            .map(|pair| pair[1].0.duration_since(pair[0].0).as_secs_f64() * 1e6)
            .collect();
        gaps.sort_by(f64::total_cmp);
        let median_gap_us = gaps.get(gaps.len() / 2).copied().unwrap_or(0.);
        let wire_us = (payload as u64 + OVERHEAD) as f64 * 8. / link_bps as f64 * 1e6;
        // The same link fed with the sender's own send times: the relay's
        // admissions may differ only by loopback delivery jitter. A sender
        // that was descheduled and caught up sent a burst the model drops too.
        let mut model = Link::new(link_bps, case.capacity);
        let expected = stamps
            .iter()
            .filter(|at| model.offer(**at, payload).is_some())
            .count() as u64;
        let mut problems = Vec::new();
        if input != sent {
            problems.push(format!("relay saw {input} of {sent} datagrams"));
        }
        if received != out || input != out + dropped {
            problems.push(format!(
                "in {input}, out {out}, dropped {dropped}, received {received}"
            ));
        }

        // Back-to-back departures are the link's own rate. The average over
        // the run (busy_bps) falls short of it whenever the sender stalled.
        if case.input_bps > link_bps && (median_gap_us / wire_us - 1.).abs() > 0.02 {
            problems.push(format!(
                "median spacing {median_gap_us:.1} us, expected {wire_us:.1} us"
            ));
        }
        if out.abs_diff(expected) > (expected / 50).max(2) {
            problems.push(format!("forwarded {out}, expected about {expected}"));
        }
        println!(
            "{}",
            serde_json::json!({
                "case": case.name, "sent": sent, "relay_in": input, "dropped": dropped,
                "forwarded": out, "received": received, "busy_bps": busy_bps.round(),
                "median_gap_us": (median_gap_us * 10.).round() / 10., "link_wire_us": (wire_us * 10.).round() / 10.,
                "expected_forwarded": expected, "max_backlog_bytes": get(&counters.max_backlog),
                "problems": problems,
            })
        );
        failures += usize::from(!problems.is_empty());
    }
    if failures > 0 {
        bail!("{failures} self-test case(s) outside tolerance");
    }
    println!("SHAPER SELFTEST PASS");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--selftest") {
        return selftest();
    }
    ensure!(
        args.len() >= 6,
        "usage: udp_shaper LISTEN UPSTREAM RATE_KBPS QUEUE_BYTES STATS PORT[:shape|:tcp]..."
    );
    let listen: IpAddr = args[0].parse()?;
    let upstream: IpAddr = args[1].parse()?;
    let bps = args[2].parse::<u64>()? * 1000;
    let capacity: u64 = args[3].parse()?;
    ensure!(
        bps == 0 || capacity >= 1600,
        "the queue must hold a full-size datagram"
    );
    let stats = std::path::PathBuf::from(&args[4]);
    let relay = relay(listen, upstream, bps, capacity, &args[5..])?;
    let started = Instant::now();
    println!(
        "SHAPER ready listen={listen} upstream={upstream} rate_bps={bps} queue_bytes={capacity}"
    );
    let temporary = stats.with_extension("tmp");
    loop {
        thread::sleep(Duration::from_millis(250));
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(&relay.json(started, bps, capacity))?,
        )?;
        std::fs::rename(&temporary, &stats)?;
    }
}
