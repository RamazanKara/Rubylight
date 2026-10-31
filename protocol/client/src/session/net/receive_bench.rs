//! The receive thread's CPU per video packet at PyroWave's packet rates, one datagram per call
//! against [`BatchReceiver`], with the receiver's lock per datagram or per batch as the video
//! receive thread takes it. Loopback senders at a fixed rate (each receive may wait for its
//! datagram, so the thread pays a wake-up per packet whenever it keeps up), and bursts queued
//! before the receiver drains them (no waits: the cost of the calls alone). Two candidates
//! measured and not taken run beside them: UDP GRO on Linux and Registered I/O on Windows. A
//! measurement, not a check:
//!
//! `cargo test --release -p nightjar-core --lib receive_bench -- --ignored --nocapture --test-threads 1`

use std::sync::atomic::AtomicU64;

use super::*;

/// A 1392-byte video packet with its RTP header.
const DATAGRAM: usize = 1392 + 16;
const ARM: Duration = Duration::from_secs(3);
/// Loopback sends are slow on Windows (about 55k a second per thread): the rate is split.
const SENDERS: u32 = if cfg!(windows) { 4 } else { 2 };
const SOCKET_BUFFER: usize = 32 << 20;

#[derive(Clone, Copy, Debug)]
enum Pattern {
    /// Evenly spaced datagrams.
    Paced { pps: u32 },
    /// Frames of datagrams sent back to back, as an unpaced host sends them.
    Frames { fps: u32, per_frame: u32 },
    /// Bursts queued in full before the receiver takes them.
    Queued,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Arm {
    /// `net::receive` and the receiver's lock per datagram (the receive thread before batches).
    PerDatagram,
    /// [`BatchReceiver`] and the lock per batch.
    Batched,
    /// `UDP_GRO`: one receive of the datagrams the kernel coalesced, split by segment size.
    #[cfg(target_os = "linux")]
    Gro,
    /// Registered I/O ([`super::rio`]) and the lock per batch.
    #[cfg(windows)]
    Rio,
}

const ARMS: &[Arm] = &[
    Arm::PerDatagram,
    Arm::Batched,
    #[cfg(target_os = "linux")]
    Arm::Gro,
    #[cfg(windows)]
    Arm::Rio,
];

#[derive(Default)]
struct Outcome {
    sent: u64,
    received: u64,
    calls: u64,
    cpu: Duration,
    wall: Duration,
    /// Send to hand-over, per datagram, in nanoseconds.
    latencies: Vec<u64>,
}

/// Nanoseconds since the first call, on the clock every thread shares.
fn clock_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// The receive thread's work per datagram, under the receiver's lock: how long since it was
/// sent.
fn arrived(latencies: &mut Vec<u64>, datagram: &[u8]) {
    let sent = u64::from_be_bytes(datagram[8..16].try_into().unwrap());
    latencies.push(clock_ns().saturating_sub(sent));
}

/// The `share` quantile of `sorted`, in microseconds.
fn quantile_us(sorted: &[u64], share: f64) -> f64 {
    sorted
        .get(((sorted.len() as f64 * share) as usize).min(sorted.len().saturating_sub(1)))
        .map_or(0.0, |&ns| ns as f64 / 1000.0)
}

/// Datagrams the socket buffer holds for a queued burst.
fn burst(granted: usize) -> u64 {
    // A datagram takes about twice its size of the buffer on Linux.
    (granted / (2 * DATAGRAM + 512)).min(16384) as u64
}

/// One sender's share of `pattern` to `to` for [`ARM`]: the datagrams sent.
fn send(to: SocketAddr, pattern: Pattern, share: u32) -> u64 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut datagram = vec![0x5au8; DATAGRAM];
    let start = Instant::now();
    let mut sent = 0u64;
    let (interval, count) = match pattern {
        Pattern::Paced { pps } => (Duration::from_secs(1) * share / pps, 1),
        Pattern::Frames { fps, per_frame } => (Duration::from_secs(1) / fps, per_frame / share),
        Pattern::Queued => unreachable!("queued bursts are sent by the receiver's side"),
    };
    let mut due = start;
    while due < start + ARM {
        while Instant::now() < due {
            std::hint::spin_loop();
        }
        for _ in 0..count {
            datagram[..8].copy_from_slice(&sent.to_be_bytes());
            datagram[8..16].copy_from_slice(&clock_ns().to_be_bytes());
            if socket.send_to(&datagram, to).is_ok() {
                sent += 1;
            }
        }
        due += interval;
    }
    sent
}

/// An arm's receive side.
trait Take {
    /// Takes what is queued (waiting up to the poll timeout) under one lock per call as the
    /// arm does: datagrams taken.
    fn take(&mut self, socket: &UdpSocket, lock: &Mutex<Vec<u64>>) -> usize;
}

struct PerDatagram(Vec<u8>);

impl Take for PerDatagram {
    fn take(&mut self, socket: &UdpSocket, lock: &Mutex<Vec<u64>>) -> usize {
        match receive(socket, &mut self.0).unwrap() {
            Some(length) => {
                arrived(&mut lock.lock().unwrap(), &self.0[..length]);
                1
            }
            None => 0,
        }
    }
}

impl Take for BatchReceiver {
    fn take(&mut self, socket: &UdpSocket, lock: &Mutex<Vec<u64>>) -> usize {
        let count = self.receive(socket).unwrap();
        if count > 0 {
            let mut sum = lock.lock().unwrap();
            for i in 0..count {
                arrived(&mut sum, self.datagram(i));
            }
        }
        count
    }
}

#[cfg(windows)]
impl Take for super::rio::Rio {
    fn take(&mut self, _: &UdpSocket, lock: &Mutex<Vec<u64>>) -> usize {
        let count = self.receive().unwrap();
        if count > 0 {
            let mut sum = lock.lock().unwrap();
            for i in 0..count {
                arrived(&mut sum, self.datagram(i));
            }
        }
        count
    }
}

/// The arm's socket, its granted receive buffer, and its receive side.
fn arm_socket(arm: Arm) -> (UdpSocket, usize, Box<dyn Take>) {
    #[cfg(windows)]
    if arm == Arm::Rio {
        let socket = super::rio::socket(Domain::IPV4).unwrap();
        socket
            .bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())
            .unwrap();
        socket.set_recv_buffer_size(SOCKET_BUFFER).unwrap();
        let socket: UdpSocket = socket.into();
        // As many posted receives as the buffer holds datagrams.
        let take = Box::new(super::rio::Rio::new(&socket, DATAGRAM).unwrap());
        return (socket, SOCKET_BUFFER, take);
    }
    let (socket, buffer) = bind_udp_sized(
        (Ipv4Addr::LOCALHOST, 0).into(),
        SOCKET_BUFFER,
        Qos::Video,
        false,
        |_| {},
    )
    .unwrap();
    let take: Box<dyn Take> = match arm {
        Arm::PerDatagram => Box::new(PerDatagram(vec![0; 65536])),
        Arm::Batched => Box::new(BatchReceiver::new(&socket, DATAGRAM)),
        #[cfg(target_os = "linux")]
        Arm::Gro => Box::new(gro::Receiver::new(&socket)),
        #[cfg(windows)]
        Arm::Rio => unreachable!("made above"),
    };
    (socket, buffer.unwrap().granted, take)
}

fn run(arm: Arm, pattern: Pattern) -> Outcome {
    let (socket, granted, mut taker) = arm_socket(arm);
    let address = socket.local_addr().unwrap();
    let lock = Mutex::new(Vec::with_capacity(1 << 21));
    let mut o = Outcome::default();
    if let Pattern::Queued = pattern {
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let datagram = vec![0x5au8; DATAGRAM];
        let start = Instant::now();
        while start.elapsed() < ARM {
            for _ in 0..burst(granted) {
                sender.send_to(&datagram, address).unwrap();
                o.sent += 1;
            }
            let (wall, cpu) = (Instant::now(), cpu::thread_time());
            let mut left = burst(granted);
            while left > 0 {
                let count = taker.take(&socket, &lock);
                if count == 0 {
                    break;
                }
                left -= count as u64;
                o.received += count as u64;
                o.calls += 1;
            }
            o.cpu += cpu::thread_time() - cpu;
            o.wall += wall.elapsed();
        }
        return o;
    }
    let done = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicU64::new(0));
    let senders: Vec<_> = (0..SENDERS)
        .map(|_| {
            let sent = sent.clone();
            std::thread::spawn(move || {
                sent.fetch_add(send(address, pattern, SENDERS), Ordering::Relaxed);
            })
        })
        .collect();
    let waiter = {
        let done = done.clone();
        std::thread::spawn(move || {
            for sender in senders {
                sender.join().unwrap();
            }
            done.store(true, Ordering::Release);
        })
    };
    let mut started: Option<(Instant, Duration)> = None;
    let mut last = (Instant::now(), cpu::thread_time());
    loop {
        let count = taker.take(&socket, &lock);
        if count == 0 {
            if done.load(Ordering::Acquire) {
                break;
            }
            continue;
        }
        started.get_or_insert_with(|| (Instant::now(), cpu::thread_time()));
        o.received += count as u64;
        o.calls += 1;
        last = (Instant::now(), cpu::thread_time());
    }
    waiter.join().unwrap();
    let (wall, cpu) = started.expect("datagrams arrived");
    o.sent = sent.load(Ordering::Relaxed);
    o.cpu = last.1 - cpu;
    o.wall = last.0 - wall;
    o.latencies = lock.into_inner().unwrap();
    o.latencies.sort_unstable();
    o
}

#[test]
#[ignore = "a measurement: run in release with --ignored --nocapture"]
fn receive_bench() {
    let patterns = [
        Pattern::Queued,
        Pattern::Paced { pps: 100_000 },
        Pattern::Paced { pps: 150_000 },
        // 120 fps at about 1.1 Gbps.
        Pattern::Frames {
            fps: 120,
            per_frame: 1000,
        },
    ];
    println!(
        "receive CPU per packet ({}, {} s per arm)",
        std::env::consts::OS,
        ARM.as_secs()
    );
    // Windows measures its counter's rate first, not while a sender runs.
    cpu::thread_time();
    for pattern in patterns {
        for round in 0..2 {
            for &arm in ARMS {
                let o = run(arm, pattern);
                println!(
                    "{pattern:?} round {round} {arm:?}: {:.0} ns/packet, {:.2} packets/call, \
                     thread {:.0}% busy over {:.1} s, received {} of {} ({:.2}% lost), \
                     send to hand-over p50 {:.1} us p99 {:.1} us",
                    o.cpu.as_nanos() as f64 / o.received as f64,
                    o.received as f64 / o.calls as f64,
                    100.0 * o.cpu.as_secs_f64() / o.wall.as_secs_f64(),
                    o.wall.as_secs_f64(),
                    o.received,
                    o.sent,
                    100.0 * (o.sent - o.received) as f64 / o.sent as f64,
                    quantile_us(&o.latencies, 0.5),
                    quantile_us(&o.latencies, 0.99),
                );
            }
        }
    }
}

/// UDP GRO: the kernel hands over datagrams of one size that arrived together as one buffer,
/// with the segment size in a control message (Linux 5.0).
#[cfg(target_os = "linux")]
mod gro {
    use std::net::UdpSocket;
    use std::os::fd::AsRawFd;
    use std::sync::Mutex;

    const UDP_GRO: libc::c_int = 104;

    pub struct Receiver {
        buffer: Vec<u8>,
        control: [u64; 8],
    }

    impl Receiver {
        pub fn new(socket: &UdpSocket) -> Self {
            let on: libc::c_int = 1;
            // SAFETY: a valid option value of its size.
            let set = unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_UDP,
                    UDP_GRO,
                    (&on as *const libc::c_int).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
            assert_eq!(set, 0, "UDP_GRO: {}", std::io::Error::last_os_error());
            Self {
                buffer: vec![0; 65536],
                control: [0; 8],
            }
        }
    }

    impl super::Take for Receiver {
        fn take(&mut self, socket: &UdpSocket, lock: &Mutex<Vec<u64>>) -> usize {
            let mut iovec = libc::iovec {
                iov_base: self.buffer.as_mut_ptr().cast(),
                iov_len: self.buffer.len(),
            };
            // SAFETY: plain data; all zero is an empty header.
            let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
            header.msg_iov = &mut iovec;
            header.msg_iovlen = 1;
            header.msg_control = self.control.as_mut_ptr().cast();
            header.msg_controllen = std::mem::size_of_val(&self.control);
            // SAFETY: the header points at live buffers of their lengths.
            let length = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut header, 0) };
            if length < 0 {
                let e = std::io::Error::last_os_error();
                assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock, "{e}");
                return 0;
            }
            let length = length as usize;
            let mut segment = length;
            // SAFETY: walks the control messages the kernel wrote within msg_controllen.
            unsafe {
                let mut message = libc::CMSG_FIRSTHDR(&header);
                while !message.is_null() {
                    if (*message).cmsg_level == libc::SOL_UDP && (*message).cmsg_type == UDP_GRO {
                        segment = (libc::CMSG_DATA(message) as *const libc::c_int).read_unaligned()
                            as usize;
                    }
                    message = libc::CMSG_NXTHDR(&header, message);
                }
            }
            let mut sum = lock.lock().unwrap();
            let mut count = 0;
            for datagram in self.buffer[..length].chunks(segment.max(1)) {
                super::arrived(&mut sum, datagram);
                count += 1;
            }
            count
        }
    }
}

/// The calling thread's CPU time.
mod cpu {
    use std::time::Duration;

    #[cfg(not(windows))]
    pub fn thread_time() -> Duration {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `now` is a valid timespec.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) },
            0
        );
        Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
    }

    /// From the thread's cycle count, which is exact where GetThreadTimes only samples, in
    /// nanoseconds at the counter's rate measured once.
    #[cfg(windows)]
    pub fn thread_time() -> Duration {
        use std::sync::OnceLock;
        use std::time::Instant;
        static PER_NS: OnceLock<f64> = OnceLock::new();
        let per_ns = *PER_NS.get_or_init(|| {
            let (start, cycles) = (Instant::now(), cycles());
            while start.elapsed() < Duration::from_millis(200) {
                std::hint::spin_loop();
            }
            (self::cycles() - cycles) as f64 / start.elapsed().as_nanos() as f64
        });
        Duration::from_nanos((cycles() as f64 / per_ns) as u64)
    }

    #[cfg(windows)]
    fn cycles() -> u64 {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
        }
        let mut cycles = 0;
        // SAFETY: the current thread's pseudo handle and a valid out pointer.
        assert_ne!(
            unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) },
            0
        );
        cycles
    }
}
