//! Control message, termination, grace period, reliable window, jitter, and multi-homed tests.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nightjar_core::control::driver::{connect, ConnectError, DriverConfig, Outcome, Warn};
use nightjar_core::control::{ClientMessage, ControlEvent, SessionOptions};
use rubylight_protocol::control::{ClockSyncReply, Reconfigure};
use rubylight_wire::input::{decode, Input};
use rusty_enet::{PacketReceived, MTU_MAX};

use crate::host::{caps, config, connect_to, TestHost, CONNECT_DATA, KEY};

#[test]
fn messages_flow_both_ways_and_stop_disconnects_gracefully() {
    let mut host = TestHost::new();
    let (client, handle) = connect_to(&mut host);
    let (events_tx, events) = mpsc::channel();
    let running = thread::spawn(move || client.run(move |event| events_tx.send(event).unwrap()));

    host.until("start and ping", |h| {
        h.has(0x0302) && h.has(0x0307) && h.has(0x0200)
    });
    let first: Vec<u16> = host.received.iter().map(|m| m.1).take(2).collect();
    assert_eq!(first, [0x0302, 0x0307]);

    handle
        .with_input(|input| input.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    handle
        .send(ClientMessage::Reconfigure(Reconfigure {
            width: 1920,
            height: 1080,
            fps_millihz: 120_000,
        }))
        .unwrap();
    host.until("input and reconfigure", |h| h.has(0x0206) && h.has(0x5532));
    let inputs: Vec<Input> = host
        .received
        .iter()
        .filter(|m| m.1 == 0x0206)
        .map(|m| decode(&m.2).unwrap())
        .collect();
    assert_eq!(inputs[0], Input::Haptics(true));
    assert!(inputs.contains(&Input::Keyboard {
        key: 0x41,
        modifiers: 0,
        flags: 0,
        down: true
    }));
    let key_channel = host
        .received
        .iter()
        .find(|m| m.1 == 0x0206 && m.2.len() == 14)
        .unwrap()
        .0;
    assert_eq!(key_channel, 0x02);
    let reconfigure = host.received.iter().find(|m| m.1 == 0x5532).unwrap();
    assert_eq!(Reconfigure::decode(&reconfigure.2).unwrap().width, 1920);

    host.send(0x010b, &[0, 0, 0, 0, 2, 0, 0x00, 0x80, 0xff, 0xff]);
    let reply = ClockSyncReply {
        sequence: 1,
        client_us: 5,
        host_receive_us: 10,
        host_send_us: 12,
    };
    host.send(0x5537, &reply.encode());
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.len() < 2 {
        assert!(Instant::now() < deadline, "events: {seen:?}");
        host.pump();
        if let Ok(event) = events.recv_timeout(Duration::from_millis(5)) {
            seen.push(event);
        }
    }
    assert_eq!(
        seen[0],
        ControlEvent::Rumble {
            controller: 2,
            low_frequency: 0x8000,
            high_frequency: 0xffff
        }
    );
    assert!(matches!(seen[1], ControlEvent::ClockSync { reply: r, .. } if r == reply));
    assert!(handle.round_trip_time().is_some());

    // A key release queued right before stop still reaches the host.
    handle
        .with_input(|input| input.send_keyboard(0x41, false, 0, 0))
        .unwrap();
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    let released = host.received.iter().filter(|m| m.1 == 0x0206).any(|m| {
        decode(&m.2).unwrap()
            == Input::Keyboard {
                key: 0x41,
                modifiers: 0,
                flags: 0,
                down: false,
            }
    });
    assert!(released, "the release was drained before the disconnect");
    assert!(handle.send(ClientMessage::RequestIdrFrame).is_err());
}

#[test]
fn a_remote_monitor_connection_sends_no_input_at_all() {
    let mut host = TestHost::new();
    let config = DriverConfig {
        input_disabled: true,
        ..config(&host)
    };
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()).unwrap());
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = connecting.join().unwrap();
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start and ping", |h| h.has(0x0302) && h.has(0x0200));

    assert!(handle
        .with_input(|input| input.send_keyboard(0x41, true, 0, 0))
        .is_err());
    assert!(handle
        .with_input(|input| input.send_mouse_move(1, 1))
        .is_err());
    // Control traffic still flows (start and ping arrived); only input is refused.
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    assert!(
        !host.has(0x0206),
        "no input packet, not even the haptics enable"
    );
}

#[test]
fn termination_followed_by_disconnect_now_keeps_the_reason() {
    let mut host = TestHost::new();
    let (client, handle) = connect_to(&mut host);
    handle.with_session(|session| session.frame_seen(1, Instant::now()));
    let (events_tx, events) = mpsc::channel();
    let running = thread::spawn(move || client.run(move |event| events_tx.send(event).unwrap()));
    host.until("start", |h| h.has(0x0307));

    // Sunshine forks send their end message and disconnect at once.
    host.send(0x0109, &0x8003_0023u32.to_be_bytes());
    let peer = host.peer.unwrap();
    host.host.peer_mut(peer).disconnect_now(0);
    host.host.flush();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !running.is_finished() {
        assert!(Instant::now() < deadline, "client did not end");
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(running.join().unwrap(), Outcome::Terminated(0));
    let all: Vec<ControlEvent> = events.try_iter().collect();
    assert_eq!(all, [ControlEvent::Terminated { error: 0 }]);
}

#[test]
fn host_disconnect_without_reason_terminates() {
    let mut host = TestHost::new();
    let (client, _handle) = connect_to(&mut host);
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start", |h| h.has(0x0307));
    let peer = host.peer.unwrap();
    host.host.peer_mut(peer).disconnect(0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !running.is_finished() {
        assert!(Instant::now() < deadline, "client did not end");
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(running.join().unwrap(), Outcome::Terminated(-1));
}

#[test]
fn input_is_held_while_the_host_is_silent_during_a_grace_period() {
    let mut host = TestHost::new();
    let config = DriverConfig {
        network_grace: Duration::from_secs(20),
        ..config(&host)
    };
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()).unwrap());
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = connecting.join().unwrap();
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start", |h| h.has(0x0200));

    // The host goes quiet: no acknowledgements, so the client stops hearing it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.contact_lost() {
        assert!(Instant::now() < deadline, "contact never lost");
        thread::sleep(Duration::from_millis(20));
    }
    handle.with_input(|input| {
        input.send_keyboard(0x42, true, 0, 0).unwrap();
        input.send_keyboard(0x42, false, 0, 0).unwrap();
        input.send_keyboard(0x43, true, 0, 0).unwrap();
        input.send_mouse_move(5, 5).unwrap();
    });
    thread::sleep(Duration::from_millis(50));
    assert!(handle.with_input(|input| input.is_holding()));

    let keys = |h: &TestHost| -> Vec<Input> {
        h.received
            .iter()
            .filter(|m| m.1 == 0x0206)
            .map(|m| decode(&m.2).unwrap())
            .filter(|i| matches!(i, Input::Keyboard { .. } | Input::Relative { .. }))
            .collect()
    };
    host.until("held input", |h| keys(h).len() >= 2);
    assert_eq!(
        keys(&host),
        [
            Input::Keyboard {
                key: 0x42,
                modifiers: 0,
                flags: 0,
                down: false
            },
            Input::Keyboard {
                key: 0x43,
                modifiers: 0,
                flags: 0,
                down: true
            },
        ]
    );
    assert!(!handle.contact_lost());
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

/// nightjar#97: while ENet's reliable window is full, controller states merge in the input
/// queue instead of waiting inside ENet, so the host gets the newest state once the window
/// opens rather than every stale one in a burst.
#[test]
fn controller_states_merge_while_the_reliable_window_is_full() {
    let mut host = TestHost::with_small_window();
    let (client, handle) = connect_to(&mut host);
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start and ping", |h| h.has(0x0307) && h.has(0x0200));

    // The host stops acknowledging (a round trip far longer than the input interval) while the
    // stick moves: each state, sent apart from the last, would be its own reliable packet.
    const STATES: i16 = 400;
    for x in 1..=STATES {
        handle
            .with_input(|input| input.send_controller(0, 1, 0, 0, 0, (x, 0), (0, 0)))
            .unwrap();
        thread::sleep(Duration::from_micros(500));
    }
    let sticks = |h: &TestHost| -> Vec<i16> {
        h.received
            .iter()
            .filter(|m| m.1 == 0x0206)
            .filter_map(|m| match decode(&m.2).unwrap() {
                Input::Controller { sticks, .. } => Some(sticks[0]),
                _ => None,
            })
            .collect()
    };
    host.until("the newest state", |h| sticks(h).last() == Some(&STATES));
    let sent = sticks(&host);
    assert!(sent.windows(2).all(|w| w[0] < w[1]), "in order: {sent:?}");
    // A 4096-byte window carries about 45 states; the rest merged in the queue.
    assert!(
        sent.len() < usize::from(STATES as u16) / 2,
        "{} of {STATES} states went out one by one",
        sent.len()
    );
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

/// A host socket whose datagrams leave 10 ms late until `loaded` is set, then 40 to 60 ms
/// late with a 150 ms stall every 400 ms, in order, released while the host services: a
/// mobile link through a VPN whose round trip rose after the stream started.
struct Jittery {
    socket: UdpSocket,
    seed: u32,
    loaded: Option<Instant>,
    queued: std::collections::VecDeque<(Instant, SocketAddr, Vec<u8>)>,
}

impl rusty_enet::Socket for Jittery {
    type Address = SocketAddr;
    type Error = io::Error;

    fn init(&mut self, _: rusty_enet::SocketOptions) -> io::Result<()> {
        self.socket.set_nonblocking(true)
    }

    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> io::Result<usize> {
        // xorshift32: the same delays every run.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        let now = Instant::now();
        let mut due = now + Duration::from_millis(10);
        if let Some(since) = self.loaded {
            due = now + Duration::from_millis(40 + u64::from(self.seed % 21));
            let phase = now.duration_since(since).as_millis() % 400;
            if phase < 150 {
                due = due.max(now + Duration::from_millis((150 - phase) as u64));
            }
        }
        if let Some(back) = self.queued.back() {
            due = due.max(back.0);
        }
        self.queued.push_back((due, address, buffer.to_vec()));
        Ok(buffer.len())
    }

    fn receive(
        &mut self,
        buffer: &mut [u8; MTU_MAX],
    ) -> io::Result<Option<(SocketAddr, PacketReceived)>> {
        let now = Instant::now();
        while self.queued.front().is_some_and(|q| q.0 <= now) {
            let (_, to, datagram) = self.queued.pop_front().unwrap();
            self.socket.send_to(&datagram, to)?;
        }
        match self.socket.recv_from(buffer) {
            Ok((length, from)) => Ok(Some((from, PacketReceived::Complete(length)))),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// nightjar#97: round trip jitter does not shrink the reliable window, so on a jittery link
/// each controller state still leaves on its own instead of waiting for an acknowledgement.
#[test]
fn round_trip_jitter_keeps_the_reliable_window_open() {
    let mut host = TestHost::on(Jittery {
        socket: UdpSocket::bind("127.0.0.1:0").unwrap(),
        seed: 0x9E37_79B9,
        loaded: None,
        queued: Default::default(),
    });
    let config = DriverConfig {
        connect_data: CONNECT_DATA,
        ..DriverConfig::new(host.host.socket().socket.local_addr().unwrap())
    };
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()).unwrap());
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = connecting.join().unwrap();
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start", |h| h.has(0x0307));
    let mut x = 0i16;
    let mut state = || {
        x += 1;
        handle
            .with_input(|input| input.send_controller(0, 1, 0, 0, 0, (x, 0), (0, 0)))
            .unwrap();
        x
    };
    // A state every 2 ms.
    let mut steer = |host: &mut TestHost<Jittery>, until: Instant| {
        let mut last = 0;
        while Instant::now() < until {
            last = state();
            host.pump();
            thread::sleep(Duration::from_millis(2));
        }
        last
    };
    steer(&mut host, Instant::now() + Duration::from_millis(500));
    // The round trip rises well past the one ENet's throttle measures against (the lowest of
    // its first five seconds), and stalls. Stock ENet sinks the throttle, cutting the window
    // to one MTU (about 15 states) while a stalled round trip carries about 90.
    host.host.socket_mut().loaded = Some(Instant::now());
    let first = steer(&mut host, Instant::now() + Duration::from_millis(500)) + 1;
    let last = steer(&mut host, Instant::now() + Duration::from_millis(2_500));
    let sticks = |h: &TestHost<Jittery>| -> Vec<i16> {
        h.received
            .iter()
            .filter(|m| m.1 == 0x0206)
            .filter_map(|m| match decode(&m.2).unwrap() {
                Input::Controller { sticks, .. } if sticks[0] >= first => Some(sticks[0]),
                _ => None,
            })
            .collect()
    };
    host.until("the newest state", |h| sticks(h).last() == Some(&last));
    let sent = usize::from((last - first + 1) as u16);
    let arrived = sticks(&host).len();
    assert!(
        arrived * 100 >= sent * 85,
        "only {arrived} of {sent} states left on their own"
    );
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
}

#[test]
fn connect_times_out_without_a_host() {
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let config = DriverConfig {
        connect_timeout: Duration::from_millis(300),
        ..DriverConfig::new(silent.local_addr().unwrap())
    };
    let started = Instant::now();
    let result = connect(config, caps(), &KEY, SessionOptions::default());
    assert!(matches!(result, Err(ConnectError::TimedOut)));
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// nightjar#38: connects to a host that replies from `routed` although the client sent to
/// `reached`, and keeps hearing it.
fn connect_to_a_multi_homed_host(reached: &str, routed: &str) {
    let Some(mut host) = TestHost::multi_homed(reached, routed) else {
        eprintln!("skipped: cannot bind {reached} and {routed}");
        return;
    };
    let warnings = Arc::new(Mutex::new(Vec::<String>::new()));
    let config = DriverConfig {
        connect_data: CONNECT_DATA,
        connect_timeout: Duration::from_secs(4),
        // With a grace period, contact_lost() says whether the replies count as the host's.
        network_grace: Duration::from_secs(20),
        warn: Warn::new({
            let warnings = warnings.clone();
            move |m| warnings.lock().unwrap().push(m.to_string())
        }),
        ..DriverConfig::new(host.address())
    };
    let connecting =
        thread::spawn(move || connect(config, caps(), &KEY, SessionOptions::default()));
    while !connecting.is_finished() {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    let (client, handle) = match connecting.join().unwrap() {
        Ok(connected) => connected,
        Err(e) => panic!("the control stream connects to a multi-homed host: {e:?}"),
    };
    host.until("connect", |h| h.peer.is_some());
    let running = thread::spawn(move || client.run(|_| {}));
    host.until("start and ping", |h| h.has(0x0302) && h.has(0x0200));
    // The host keeps acknowledging for longer than INPUT_CONTACT_LOST_AFTER.
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        host.pump();
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !handle.contact_lost(),
        "input held back although the host answers"
    );
    // The host goes quiet: its replies were what kept the contact.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.contact_lost() {
        assert!(
            Instant::now() < deadline,
            "contact never lost, so the host's replies never counted"
        );
        thread::sleep(Duration::from_millis(20));
    }
    handle.stop();
    host.until("disconnect", |h| h.disconnected);
    assert_eq!(running.join().unwrap(), Outcome::Stopped);
    let warnings = warnings.lock().unwrap();
    assert!(
        !warnings
            .iter()
            .any(|w| w.starts_with("No answer to the first control connects")),
        "{warnings:?}"
    );
}

#[test]
fn connects_to_a_host_that_replies_from_another_port() {
    connect_to_a_multi_homed_host("127.0.0.1:0", "127.0.0.1:0");
}

#[test]
fn connects_to_a_host_that_replies_from_another_ip() {
    connect_to_a_multi_homed_host("127.0.0.1:0", "127.0.0.2:0");
}
