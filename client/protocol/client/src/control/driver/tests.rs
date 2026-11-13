use super::*;
use crate::roaming::{Rejections, MAX_ATTEMPT};
use std::cell::Cell;

/// A code [`datagram_lost`] takes for packet loss: network unreachable.
const UNREACHABLE: i32 = if cfg!(windows) { 10051 } else { 101 };

thread_local! {
    /// Replaces the result of this thread's control socket sends: a byte count, or a raw
    /// OS error.
    static SEND_RESULT: Cell<Option<Result<usize, i32>>> = const { Cell::new(None) };
}

pub(super) fn send_result() -> Option<Result<usize, i32>> {
    SEND_RESULT.with(Cell::get)
}

fn inject_sends(result: Option<Result<usize, i32>>) {
    SEND_RESULT.with(|injected| injected.set(result));
}

/// A control socket on `socket`, with the warnings it gives.
fn control_socket(
    socket: UdpSocket,
    marking: Option<Marking>,
) -> (ControlSocket, Arc<Mutex<Vec<String>>>) {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let control = ControlSocket {
        socket,
        roam_inbox: VecDeque::new(),
        roaming: false,
        intercept: false,
        held_disconnect: None,
        held_unsequenced: false,
        reinject: None,
        last_receive: None,
        disconnect_handed: None,
        marked: marking.is_some(),
        marking,
        flow: None,
        send_failures: SendFailures::default(),
        warn: Warn::new({
            let warnings = warnings.clone();
            move |m| warnings.lock().unwrap().push(m.to_string())
        }),
    };
    (control, warnings)
}

/// A control socket marked for QoS and watching its first connects, aimed at a sink.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn marked_socket() -> (ControlSocket, UdpSocket, Arc<Mutex<Vec<String>>>) {
    let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    priority::mark_control_socket(&socket, false, true).unwrap();
    let (control, warnings) = control_socket(
        socket,
        Some(Marking {
            ipv6: false,
            unanswered: 0,
        }),
    );
    (control, sink, warnings)
}

/// A socket that takes the type of service but refuses the priority.
#[cfg(any(target_os = "linux", target_os = "android"))]
struct RefusesPriority(UdpSocket);

#[cfg(any(target_os = "linux", target_os = "android"))]
impl priority::PriorityTarget for RefusesPriority {
    fn set_priority(&self, _value: i32) -> io::Result<()> {
        Err(io::ErrorKind::PermissionDenied.into())
    }

    fn set_tos(&self, ipv6: bool, value: i32) -> io::Result<()> {
        priority::PriorityTarget::set_tos(&self.0, ipv6, value)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn the_marking_is_watched_even_when_the_priority_was_refused() {
    let socket = RefusesPriority(UdpSocket::bind("127.0.0.1:0").unwrap());
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let warn = Warn::new({
        let warnings = warnings.clone();
        move |m| warnings.lock().unwrap().push(m.to_string())
    });
    let marking = mark_for_connect(&socket, false, &warn);
    assert_eq!(marking.unanswered, 0);
    assert_eq!(warnings.lock().unwrap().len(), 1);
    // The type of service took, so the unmarking below has something to undo.
    assert_eq!(tos(&socket.0), priority::CONTROL_TOS);
    let _ = priority::mark_control_socket(&socket, false, false);
    assert_eq!(tos(&socket.0), 0);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn tos(socket: &UdpSocket) -> i32 {
    priority::tests::read_option(socket, libc::IPPROTO_IP, libc::IP_TOS)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn the_marking_comes_off_after_three_unanswered_connects() {
    use rusty_enet::Socket as _;
    let (mut control, sink, warnings) = marked_socket();
    let remote = sink.local_addr().unwrap();
    for _ in 0..3 {
        control.send(remote, b"connect").unwrap();
        assert_eq!(tos(&control.socket), priority::CONTROL_TOS);
    }
    control.send(remote, b"connect").unwrap();
    assert_eq!(tos(&control.socket), 0);
    assert_eq!(
        priority::tests::read_priority(&control.socket),
        0,
        "the priority goes with the marking"
    );
    assert_eq!(warnings.lock().unwrap().len(), 1);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn an_answer_keeps_the_marking() {
    use rusty_enet::Socket as _;
    let (mut control, sink, warnings) = marked_socket();
    let remote = sink.local_addr().unwrap();
    control.send(remote, b"connect").unwrap();
    // The host answers from its address.
    let ours = control.socket.local_addr().unwrap();
    sink.send_to(b"verify", ours).unwrap();
    let mut buffer = [0u8; MTU_MAX];
    let mut received = false;
    for _ in 0..50 {
        if control.receive(&mut buffer).unwrap().is_some() {
            received = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(received);
    for _ in 0..6 {
        control.send(remote, b"data").unwrap();
    }
    assert_eq!(tos(&control.socket), priority::CONTROL_TOS);
    assert!(warnings.lock().unwrap().is_empty());
}

#[test]
fn a_streak_of_failed_sends_is_reported_once() {
    let to: SocketAddr = "192.0.2.1:47999".parse().unwrap();
    let unreachable = || io::Error::from_raw_os_error(UNREACHABLE);
    assert!(datagram_lost(&unreachable()));
    let mut failures = SendFailures::default();
    assert!(failures.failed(to, unreachable()).is_none());
    assert!(failures.failed(to, unreachable()).is_none());
    let warning = failures
        .failed(to, unreachable())
        .expect("the third failure warns");
    assert!(
        warning.starts_with("Control datagrams to 192.0.2.1:47999 are failing: "),
        "{warning}"
    );
    assert!(failures.failed(to, unreachable()).is_none());
    // A send that gets out ends the streak; the next one is reported again.
    failures.sent();
    assert!(failures.failed(to, unreachable()).is_none());
    assert!(failures.failed(to, unreachable()).is_none());
    assert!(failures.failed(to, unreachable()).is_some());
}

#[test]
fn a_connect_whose_sends_all_failed_reports_the_error() {
    let to: SocketAddr = "192.0.2.1:47999".parse().unwrap();
    let mut failures = SendFailures::default();
    assert!(failures.take_if_none_sent().is_none(), "nothing failed");
    failures.failed(to, io::ErrorKind::AddrNotAvailable.into());
    let error = failures.take_if_none_sent().expect("every send failed");
    assert_eq!(error.kind(), io::ErrorKind::AddrNotAvailable);

    let mut failures = SendFailures::default();
    failures.sent();
    failures.failed(to, io::ErrorKind::AddrNotAvailable.into());
    assert!(
        failures.take_if_none_sent().is_none(),
        "a datagram left, so the host was silent"
    );
}

#[test]
fn control_socket_sends_are_counted() {
    use rusty_enet::Socket as _;
    let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    let to = sink.local_addr().unwrap();
    let (mut control, warnings) = control_socket(UdpSocket::bind("127.0.0.1:0").unwrap(), None);
    inject_sends(Some(Err(UNREACHABLE)));
    for _ in 0..3 {
        assert_eq!(
            control.send(to, b"connect").unwrap(),
            0,
            "a loss, not an error"
        );
    }
    assert_eq!(
        *warnings.lock().unwrap(),
        [format!(
            "Control datagrams to {to} are failing: {}",
            io::Error::from_raw_os_error(UNREACHABLE)
        )]
    );
    // A full send buffer (WouldBlock) is not a datagram that left.
    inject_sends(Some(Ok(0)));
    control.send(to, b"connect").unwrap();
    assert!(!control.send_failures.any_sent);
    inject_sends(None);
    assert!(control.send(to, b"connect").unwrap() > 0);
    assert!(control.send_failures.any_sent);
    assert_eq!(control.send_failures.consecutive, 0);
}

/// A non-blocking control socket, as the driver keeps it.
fn nonblocking_control() -> ControlSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    control_socket(socket, None).0
}

fn assert_nonblocking(control: &ControlSocket) {
    let error = control.socket.recv(&mut [0; 1]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn a_queued_datagram_ends_the_wait_at_once() {
    let control = nonblocking_control();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender
        .send_to(&[0], control.socket.local_addr().unwrap())
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let start = Instant::now();
    control.wait(Duration::from_secs(5));
    assert!(start.elapsed() < Duration::from_secs(1));
    control.socket.recv(&mut [0; 1]).unwrap();
    assert_nonblocking(&control);
}

#[test]
fn an_idle_wait_lasts_its_timeout_and_leaves_the_socket_nonblocking() {
    let control = nonblocking_control();
    let start = Instant::now();
    control.wait(Duration::from_millis(100));
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(90), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_nonblocking(&control);
}

#[test]
fn poll_timeouts_round_up_to_whole_milliseconds() {
    assert_eq!(poll_millis(Duration::from_micros(300)), 1);
    assert_eq!(poll_millis(Duration::from_millis(5)), 5);
    assert_eq!(poll_millis(Duration::from_micros(5_001)), 6);
    assert_eq!(poll_millis(Duration::from_secs(u64::MAX)), i32::MAX);
}

#[test]
fn a_datagram_too_short_for_enet_is_not_the_host() {
    use rusty_enet::Socket as _;
    let (mut control, _) = control_socket(UdpSocket::bind("127.0.0.1:0").unwrap(), None);
    control.socket.set_nonblocking(true).unwrap();
    let ours = control.socket.local_addr().unwrap();
    let stray = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut buffer = [0u8; MTU_MAX];
    let mut receive = |control: &mut ControlSocket| {
        for _ in 0..50 {
            if let Some((_, PacketReceived::Complete(length))) =
                control.receive(&mut buffer).unwrap()
            {
                return length;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("nothing arrived");
    };
    // A wake-sized datagram never reaches ENet; the next one does.
    stray.send_to(&[0], ours).unwrap();
    stray.send_to(&[0, 0], ours).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(control.last_receive.is_none());
    assert_eq!(receive(&mut control), 2);
    assert!(control.last_receive.is_some());
}

#[test]
fn a_stop_lingers_only_for_a_host_that_was_heard_lately() {
    let now = Instant::now() + Duration::from_secs(10);
    assert!(!lingers_for_nothing(None, now), "never heard: ENet decides");
    assert!(!lingers_for_nothing(Some(now - Duration::from_millis(200)), now));
    assert!(!lingers_for_nothing(Some(now - Duration::from_millis(1500)), now), "a short blip: a clean stop still reaches the host");
    assert!(lingers_for_nothing(Some(now - SILENT_STOP_AFTER), now));
    assert!(lingers_for_nothing(Some(now - Duration::from_secs(5)), now), "a PC that is gone");
}

#[test]
fn the_hosts_disconnect_is_a_goodbye_any_other_datagram_is_not() {
    use rusty_enet::Socket as _;
    let (mut control, _) = control_socket(UdpSocket::bind("127.0.0.1:0").unwrap(), None);
    let ours = control.socket.local_addr().unwrap();
    let host = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut buffer = [0u8; MTU_MAX];
    let mut deliver = |control: &mut ControlSocket, datagram: &[u8]| {
        host.send_to(datagram, ours).unwrap();
        for _ in 0..50 {
            if control.receive(&mut buffer).unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("nothing arrived");
    };
    deliver(&mut control, &[0x00, 0x00, 0x46, 0xff, 0, 0, 0, 0, 0, 0]);
    assert!(control.disconnect_handed.is_none());
    // enet_peer_disconnect_now, reaching ENet here (no intercept).
    deliver(&mut control, &[0x10, 0x00, 0x44, 0xff, 0, 0, 0, 0, 0, 0]);
    assert!(is_goodbye(control.disconnect_handed, Instant::now()));
}

#[test]
fn only_a_disconnect_event_right_after_the_hosts_disconnect_is_a_goodbye() {
    let now = Instant::now() + Duration::from_secs(30);
    assert!(!is_goodbye(None, now), "a timeout without any disconnect");
    assert!(is_goodbye(Some(now - Duration::from_millis(20)), now), "ENet took the host's disconnect");
    // Another session's disconnect ENet ignored, then the timeout much later: no goodbye.
    assert!(!is_goodbye(Some(now - DISCONNECT_PROCESS_WAIT), now));
    assert!(!is_goodbye(Some(now - Duration::from_secs(12)), now));
}

#[test]
fn roaming_datagrams_are_kept_only_with_roaming_on() {
    use rusty_enet::Socket as _;
    let mut roaming = [0u8; rubylight_protocol::roaming::DATAGRAM_BYTES];
    roaming[..4].copy_from_slice(&rubylight_protocol::roaming::MAGIC);
    for on in [false, true] {
        let (mut control, _) = control_socket(UdpSocket::bind("127.0.0.1:0").unwrap(), None);
        control.roaming = on;
        control.socket.set_nonblocking(true).unwrap();
        let ours = control.socket.local_addr().unwrap();
        let host = UdpSocket::bind("127.0.0.1:0").unwrap();
        host.send_to(&roaming, ours).unwrap();
        host.send_to(&[0, 0], ours).unwrap();
        let mut buffer = [0u8; MTU_MAX];
        let length = (0..50)
            .find_map(|_| match control.receive(&mut buffer).unwrap() {
                Some((_, PacketReceived::Complete(length))) => Some(length),
                _ => {
                    std::thread::sleep(Duration::from_millis(10));
                    None
                }
            })
            .expect("nothing arrived");
        // The roaming datagram never reaches ENet either way.
        assert_eq!(length, 2);
        assert_eq!(control.roam_inbox.len(), usize::from(on), "roaming {on}");
    }
}

#[test]
fn a_connect_none_of_whose_datagrams_left_fails_with_the_error() {
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let config = DriverConfig {
        connect_timeout: Duration::from_millis(300),
        ..DriverConfig::new(silent.local_addr().unwrap())
    };
    let caps = HostCapabilities {
        app_version: [7, 1, 431, -1],
        feature_flags: 3,
        encryption_enabled: super::super::negotiation::encryption::CONTROL_V2,
        control_messages: Vec::new(),
        client_roaming: false,
    };
    inject_sends(Some(Err(UNREACHABLE)));
    let result = connect(config, caps, &[0; 16], SessionOptions::default());
    inject_sends(None);
    match result {
        Err(ConnectError::Io(error)) => assert_eq!(error.raw_os_error(), Some(UNREACHABLE)),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("connected without sending"),
    }
}

/// One pass of [`ControlClient::roam`] on a simulated clock, without sockets.
fn roam_step(
    triggers: &mut RoamTriggers,
    roaming: &mut Roaming,
    now: Instant,
    contact: Instant,
    request: Option<u32>,
) -> Vec<RoamEvent> {
    triggers.update(roaming, now, Some(contact), request, false);
    roaming.poll(now);
    while roaming.poll_transmit().is_some() {}
    let events = roaming.take_events();
    triggers.ended(now, &events);
    events
}

const SILENT: RoamEvent = RoamEvent::Started {
    trigger: RoamTrigger::HostSilent,
};

#[test]
fn a_failed_attempt_rearms_the_silence_trigger_after_a_backoff() {
    let heard = Instant::now();
    let at = |ms| heard + Duration::from_millis(ms);
    let mut roaming = Roaming::new(&[7; 16], MAX_ATTEMPT);
    let mut triggers = RoamTriggers::default();
    let mut step =
        |now, contact, request| roam_step(&mut triggers, &mut roaming, now, contact, request);
    assert!(step(at(999), heard, None).is_empty());
    assert_eq!(step(at(1000), heard, None), [SILENT]);
    assert_eq!(
        step(at(11_000), heard, None),
        [RoamEvent::Failed {
            elapsed: MAX_ATTEMPT,
            rejected: Rejections::default(),
        }]
    );
    // The peer lives on through a long grace period, still silent: another attempt
    // runs, after the backoff.
    assert!(step(at(15_000), heard, None).is_empty());
    assert!(step(at(20_999), heard, None).is_empty());
    assert_eq!(step(at(21_000), heard, None), [SILENT]);
    assert_eq!(
        step(at(21_500), at(21_400), None),
        [RoamEvent::Unneeded {
            elapsed: Duration::from_millis(500)
        }]
    );
    // Heard again, then silent again: one attempt for the new silence.
    assert!(step(at(22_399), at(21_400), None).is_empty());
    assert_eq!(step(at(22_400), at(21_400), None), [SILENT]);
}

#[test]
fn a_failed_attempt_holds_back_silences_but_not_network_changes() {
    let heard = Instant::now();
    let at = |ms| heard + Duration::from_millis(ms);
    let mut roaming = Roaming::new(&[7; 16], MAX_ATTEMPT);
    let mut triggers = RoamTriggers::default();
    let mut step =
        |now, contact, request| roam_step(&mut triggers, &mut roaming, now, contact, request);
    assert_eq!(step(at(1000), heard, None), [SILENT]);
    assert!(matches!(
        step(at(11_000), heard, None)[..],
        [RoamEvent::Failed { .. }]
    ));
    // The host answers for a while, then goes silent within the backoff.
    assert!(step(at(13_000), at(12_000), None).is_empty());
    assert_eq!(
        step(at(14_000), at(12_000), Some(0)),
        [RoamEvent::Started {
            trigger: RoamTrigger::NetworkChanged
        }]
    );
}

#[test]
fn a_metered_hold_is_held_not_failed_and_arms_no_backoff() {
    let heard = Instant::now();
    let at = |ms| heard + Duration::from_millis(ms);
    let mut roaming = Roaming::new(&[7; 16], MAX_ATTEMPT);
    let mut triggers = RoamTriggers::default();
    let mut step = |now, contact, held| {
        triggers.update(&mut roaming, now, Some(contact), None, held);
        roaming.poll(now);
        let sent = std::iter::from_fn(|| roaming.poll_transmit()).count();
        let events = roaming.take_events();
        triggers.ended(now, &events);
        (events, sent)
    };
    assert_eq!(step(at(1000), heard, false), (vec![SILENT], 1));
    // The hold ends the attempt once, as Held, and nothing goes out while it lasts.
    assert_eq!(step(at(1100), heard, true), (vec![RoamEvent::Held], 0));
    assert_eq!(step(at(3000), heard, true), (vec![], 0));
    // Lifted, a silence starts an attempt at once: no failure's backoff holds it back.
    assert_eq!(step(at(3100), heard, false), (vec![SILENT], 1));
}

#[test]
fn a_network_change_during_a_silence_is_not_ended_by_a_straggler() {
    let heard = Instant::now();
    let at = |ms| heard + Duration::from_millis(ms);
    let mut roaming = Roaming::new(&[7; 16], MAX_ATTEMPT);
    let mut triggers = RoamTriggers::default();
    let mut step =
        |now, contact, request| roam_step(&mut triggers, &mut roaming, now, contact, request);
    assert_eq!(step(at(1000), heard, None), [SILENT]);
    assert_eq!(
        step(at(1500), heard, Some(PROBE_FLAG_MEDIA_REBOUND)),
        [RoamEvent::Started {
            trigger: RoamTrigger::NetworkChanged
        }]
    );
    // A datagram the host sent before the change arrives on the old path.
    assert!(step(at(1600), at(1550), None).is_empty());
    assert_eq!(
        step(at(11_500), at(1550), None),
        [RoamEvent::Failed {
            elapsed: MAX_ATTEMPT,
            rejected: Rejections::default(),
        }],
        "timed from the change"
    );
}

#[test]
fn the_waker_aims_at_the_control_socket() {
    let target = |waker: &Waker| waker.socket.peer_addr().unwrap();
    let waker = Waker::new("127.0.0.1:47000".parse().unwrap()).unwrap();
    assert_eq!(target(&waker), "127.0.0.1:47000".parse().unwrap());
    let waker = Waker::new("0.0.0.0:47000".parse().unwrap()).unwrap();
    assert_eq!(target(&waker), "127.0.0.1:47000".parse().unwrap());
    // A moved control socket.
    waker.retarget("0.0.0.0:47001".parse().unwrap()).unwrap();
    assert_eq!(target(&waker), "127.0.0.1:47001".parse().unwrap());
    let Ok(waker) = Waker::new("[::]:47000".parse().unwrap()) else {
        eprintln!("skipped: no IPv6 loopback");
        return;
    };
    assert_eq!(target(&waker), "[::1]:47000".parse().unwrap());
}

/// A roam can take the old local address away: the waker is bound to none, so it
/// reaches the control socket on whichever address it moved to.
#[test]
fn the_waker_is_bound_to_no_local_address() {
    for target in ["192.168.1.20:47000", "[fe80::1]:47000"] {
        let bind = wake_bind(target.parse().unwrap());
        assert!(bind.ip().is_unspecified() && bind.port() == 0, "{bind}");
        assert_eq!(bind.is_ipv4(), target.starts_with('1'));
    }
    let waker = Waker::new("127.0.0.1:47000".parse().unwrap()).unwrap();
    let control = UdpSocket::bind("127.0.0.1:0").unwrap();
    waker.retarget(control.local_addr().unwrap()).unwrap();
    waker.wake();
    control
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buffer = [0; 8];
    assert_eq!(control.recv(&mut buffer).unwrap(), 1, "a 1-byte wake");
}

#[test]
fn disconnect_datagrams_are_recognized() {
    // Reliable disconnect with a sent time: header 4, command 0x84 (acknowledge flag).
    assert_eq!(
        enet_disconnect(&[0x80, 0x00, 0, 1, 0x84, 0xff, 0, 1, 0, 0, 0, 0]),
        Some(false)
    );
    // enet_peer_disconnect_now: unsequenced, no sent time.
    assert_eq!(
        enet_disconnect(&[0x10, 0x00, 0x44, 0xff, 0, 0, 0, 0, 0, 0]),
        Some(true)
    );
    // Addressed to another peer, compressed, another command or another length.
    assert_eq!(
        enet_disconnect(&[0x00, 0x01, 0x44, 0xff, 0, 0, 0, 0, 0, 0]),
        None
    );
    assert_eq!(
        enet_disconnect(&[0x40, 0x00, 0x44, 0xff, 0, 0, 0, 0, 0, 0]),
        None
    );
    assert_eq!(
        enet_disconnect(&[0x00, 0x00, 0x46, 0xff, 0, 0, 0, 0, 0, 0]),
        None
    );
    assert_eq!(
        enet_disconnect(&[0x00, 0x00, 0x44, 0xff, 0, 0, 0, 0, 0]),
        None
    );
}

#[test]
fn an_idle_control_thread_waits_the_cap() {
    let now = Instant::now();
    assert_eq!(service_wait(now, None, [None, None]), MAX_IDLE_WAIT);
    assert_eq!(
        service_wait(now, Some(Duration::from_millis(250)), [None, None]),
        MAX_IDLE_WAIT
    );
}

#[test]
fn enet_sooner_than_the_cap_ends_the_wait() {
    let now = Instant::now();
    assert_eq!(
        service_wait(now, Some(Duration::from_millis(3)), [None, None]),
        Duration::from_millis(3)
    );
    assert_eq!(
        service_wait(now, Some(Duration::from_millis(1)), [None, None]),
        Duration::from_millis(1)
    );
}

#[test]
fn session_and_roam_timers_end_the_wait_and_a_due_one_gives_zero() {
    let now = Instant::now();
    let soon = now + Duration::from_millis(4);
    assert_eq!(
        service_wait(now, None, [Some(soon), None]),
        Duration::from_millis(4)
    );
    assert_eq!(
        service_wait(now, None, [None, Some(soon)]),
        Duration::from_millis(4)
    );
    assert_eq!(service_wait(now, None, [Some(now), None]), Duration::ZERO);
    let later = now + Duration::from_millis(7);
    assert_eq!(service_wait(later, None, [Some(now), None]), Duration::ZERO);
}

#[test]
fn the_earliest_of_every_deadline_wins() {
    let now = Instant::now();
    let timer = now + Duration::from_millis(6);
    assert_eq!(
        service_wait(
            now,
            Some(Duration::from_millis(2)),
            [Some(timer), Some(now + Duration::from_millis(8))]
        ),
        Duration::from_millis(2)
    );
    assert_eq!(
        service_wait(now, Some(Duration::from_millis(9)), [Some(timer), None]),
        Duration::from_millis(6)
    );
}
