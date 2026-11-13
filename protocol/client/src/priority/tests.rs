use super::*;

#[test]
fn media_sockets_always_get_their_priority() {
    assert_eq!(socket_priority(Qos::Audio, false), Some(6));
    assert_eq!(socket_priority(Qos::Video, false), Some(5));
    assert_eq!(socket_priority(Qos::Audio, true), Some(6));
    assert_eq!(socket_priority(Qos::Video, true), Some(5));
}

#[test]
fn other_sockets_are_prioritized_only_on_request() {
    assert_eq!(socket_priority(Qos::BestEffort, false), None);
    assert_eq!(socket_priority(Qos::BestEffort, true), Some(6));
}

#[test]
fn audio_and_microphone_go_expedited_and_video_af41() {
    assert_eq!(media_tos(Qos::Audio), Some(0xB8));
    assert_eq!(media_tos(Qos::Video), Some(0x88));
    assert_eq!(media_tos(Qos::BestEffort), None);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn media_sockets_are_marked() {
    let audio = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    mark_media_socket(&audio, Qos::Audio, false).unwrap();
    assert_eq!(
        read_option(&audio, libc::IPPROTO_IP, libc::IP_TOS),
        AUDIO_TOS
    );
    let video = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    mark_media_socket(&video, Qos::Video, false).unwrap();
    assert_eq!(
        read_option(&video, libc::IPPROTO_IP, libc::IP_TOS),
        VIDEO_TOS
    );
    let plain = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    mark_media_socket(&plain, Qos::BestEffort, false).unwrap();
    assert_eq!(read_option(&plain, libc::IPPROTO_IP, libc::IP_TOS), 0);
}

#[test]
fn the_control_marking_is_dropped_after_three_lost_connects() {
    // The fourth datagram is the first one sent plain.
    assert!(!should_drop_control_marking(2));
    assert!(should_drop_control_marking(3));
}

#[test]
fn the_control_tos_is_expedited_forwarding_with_ect1() {
    assert_eq!(CONTROL_TOS, 0xB9);
}

const QUEUED: StreamThread = StreamThread::VideoReceive {
    direct_submit: false,
};
const DIRECT: StreamThread = StreamThread::VideoReceive {
    direct_submit: true,
};

#[test]
fn a_direct_submission_receive_thread_gets_the_decode_level() {
    for windows in [false, true] {
        assert_eq!(
            thread_level_on(DIRECT, true, true, windows),
            ThreadLevel::Decode
        );
        assert_eq!(
            thread_level_on(QUEUED, true, true, windows),
            ThreadLevel::Stream
        );
        assert_eq!(
            thread_level_on(StreamThread::VideoDecode, true, true, windows),
            ThreadLevel::Decode
        );
        assert_eq!(
            thread_level_on(StreamThread::Other, true, true, windows),
            ThreadLevel::Stream
        );
    }
}

#[test]
fn stream_priority_off_raises_only_the_queued_decoder_off_windows() {
    for thread in [DIRECT, QUEUED, StreamThread::Other] {
        for windows in [false, true] {
            assert_eq!(
                thread_level_on(thread, false, true, windows),
                ThreadLevel::Normal
            );
        }
    }
    // As before the switch: Linux and Android always raised the queued decode thread.
    assert_eq!(
        thread_level_on(StreamThread::VideoDecode, false, true, false),
        ThreadLevel::Decode
    );
    assert_eq!(
        thread_level_on(StreamThread::VideoDecode, false, true, true),
        ThreadLevel::Normal
    );
}

#[test]
fn the_switch_off_restores_the_old_levels() {
    // Linux and Android: the receive thread is back at the stream level even when it decodes.
    assert_eq!(
        thread_level_on(DIRECT, true, false, false),
        ThreadLevel::Stream
    );
    assert_eq!(
        thread_level_on(QUEUED, true, false, false),
        ThreadLevel::Stream
    );
    assert_eq!(
        thread_level_on(StreamThread::VideoDecode, true, false, false),
        ThreadLevel::Decode
    );
    assert_eq!(
        thread_level_on(StreamThread::Other, true, false, false),
        ThreadLevel::Stream
    );
    // Windows raised nothing.
    for thread in [
        DIRECT,
        QUEUED,
        StreamThread::VideoDecode,
        StreamThread::Other,
    ] {
        assert_eq!(
            thread_level_on(thread, true, false, true),
            ThreadLevel::Normal
        );
    }
}

#[test]
fn windows_raises_decoding_highest_and_the_rest_above_normal() {
    assert_eq!(windows_thread_priority(ThreadLevel::Normal), None);
    assert_eq!(windows_thread_priority(ThreadLevel::Stream), Some(1));
    assert_eq!(windows_thread_priority(ThreadLevel::Decode), Some(2));
}

#[test]
fn the_switch_follows_the_setting_unless_set_to_0_or_1() {
    for setting in [false, true] {
        assert_eq!(switch_on(None, setting), setting);
        assert_eq!(switch_on(Some(""), setting), setting);
        assert_eq!(switch_on(Some("yes"), setting), setting);
        assert!(switch_on(Some("1"), setting));
        assert!(switch_on(Some(" 1\n"), setting));
        assert!(!switch_on(Some("0"), setting));
        assert!(!switch_on(Some(" 0\n"), setting));
    }
}

/// A fake portal or RealtimeKit that records its calls.
struct FakeService {
    min_nice_level: Result<i32, String>,
    answer: Result<(), String>,
    calls: std::cell::RefCell<Vec<(u64, i32)>>,
    min_nice_reads: std::cell::Cell<u32>,
}

impl FakeService {
    fn new(min_nice_level: Result<i32, String>, answer: Result<(), String>) -> Self {
        Self {
            min_nice_level,
            answer,
            calls: Default::default(),
            min_nice_reads: Default::default(),
        }
    }
}

impl RealtimeService for FakeService {
    fn min_nice_level(&self) -> Result<i32, String> {
        self.min_nice_reads.set(self.min_nice_reads.get() + 1);
        self.min_nice_level.clone()
    }

    fn make_thread_high_priority(&self, tid: u64, nice: i32) -> Result<(), String> {
        self.calls.borrow_mut().push((tid, nice));
        self.answer.clone()
    }
}

fn denied(_: i32) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::PermissionDenied))
}

fn raise_through(
    set_nice: impl Fn(i32) -> io::Result<()>,
    portal: Result<&dyn RealtimeService, String>,
    kit: Result<&dyn RealtimeService, String>,
) -> io::Result<Raised> {
    let gone = [AtomicBool::new(false), AtomicBool::new(false)];
    raise_nice_with(DECODE_NICE, Some(DECODE_FALLBACK_NICE), set_nice, |nice| {
        let services = [
            (RaisedBy::Portal, &gone[0], portal),
            (RaisedBy::RealtimeKit, &gone[1], kit),
        ];
        ask_services(nice, 42, services)
    })
}

#[test]
fn a_service_that_failed_is_skipped_by_later_threads() {
    let gone = [AtomicBool::new(false), AtomicBool::new(false)];
    let unreadable = FakeService::new(Err("ServiceUnknown".into()), Ok(()));
    let refusing = FakeService::new(Ok(-15), Err("AccessDenied".into()));
    for _ in 0..2 {
        let services = [
            (
                RaisedBy::Portal,
                &gone[0],
                Ok(&unreadable as &dyn RealtimeService),
            ),
            (
                RaisedBy::RealtimeKit,
                &gone[1],
                Ok(&refusing as &dyn RealtimeService),
            ),
        ];
        assert!(ask_services(DECODE_NICE, 42, services).is_err());
    }
    // The missing portal was asked once; RealtimeKit refused a thread, which is per thread.
    assert_eq!(unreadable.min_nice_reads.get(), 1);
    assert_eq!(refusing.calls.borrow().len(), 2);
    // A bus that could not be reached is remembered as well.
    let gone = AtomicBool::new(false);
    for _ in 0..2 {
        let services = [(
            RaisedBy::Portal,
            &gone,
            Err("session bus: none".to_string()),
        )];
        let reasons = ask_services(DECODE_NICE, 42, services).unwrap_err();
        assert_eq!(reasons.len(), 1);
    }
    assert!(gone.load(Ordering::Acquire));
}

#[test]
fn a_hung_bus_costs_a_thread_at_most_the_budget_once() {
    let broker = Broker::new().unwrap();
    let budget = Duration::from_millis(100);
    // A connect that never returns: the job waits on a channel nobody sends to.
    let (release, hung) = mpsc::channel::<()>();
    let start = std::time::Instant::now();
    let reasons = broker
        .run(budget, move || {
            let _ = hung.recv();
        })
        .unwrap_err();
    let waited = start.elapsed();
    assert!(waited < budget + Duration::from_millis(400), "{waited:?}");
    assert!(
        reasons[0].contains("no answer within 100 ms"),
        "{reasons:?}"
    );
    // The next thread is refused at once and its job never runs.
    let ran = Arc::new(AtomicBool::new(false));
    let job_ran = ran.clone();
    let start = std::time::Instant::now();
    assert!(broker
        .run(budget, move || job_ran.store(true, Ordering::Release))
        .is_err());
    assert!(start.elapsed() < Duration::from_millis(50));
    drop(release);
    assert!(!ran.load(Ordering::Acquire));
}

#[test]
fn a_broker_answers_within_the_budget() {
    let broker = Broker::new().unwrap();
    assert_eq!(broker.run(SERVICE_BUDGET, || 7).unwrap(), 7);
    assert_eq!(broker.run(SERVICE_BUDGET, || 8).unwrap(), 8);
}

#[test]
fn setpriority_comes_first_and_no_service_is_asked() {
    let portal = FakeService::new(Ok(-15), Ok(()));
    let raised = raise_through(|_| Ok(()), Ok(&portal), Err("unused".into())).unwrap();
    assert_eq!((raised.by, raised.value), (RaisedBy::Os, DECODE_NICE));
    // A kernel that allows -4 but not -8 (RLIMIT_NICE) keeps the old fallback.
    let raised = raise_through(
        |nice| {
            if nice == DECODE_NICE {
                denied(nice)
            } else {
                Ok(())
            }
        },
        Ok(&portal),
        Err("unused".into()),
    )
    .unwrap();
    assert_eq!(
        (raised.by, raised.value),
        (RaisedBy::Os, DECODE_FALLBACK_NICE)
    );
    assert!(portal.calls.borrow().is_empty());
}

#[test]
fn a_refused_setpriority_asks_the_portal_then_realtimekit() {
    let portal = FakeService::new(Ok(-15), Ok(()));
    let kit = FakeService::new(Ok(-15), Ok(()));
    let raised = raise_through(denied, Ok(&portal), Ok(&kit)).unwrap();
    assert_eq!((raised.by, raised.value), (RaisedBy::Portal, DECODE_NICE));
    assert_eq!(*portal.calls.borrow(), [(42, DECODE_NICE)]);
    assert!(kit.calls.borrow().is_empty());

    let refusing = FakeService::new(Ok(-15), Err("AccessDenied".into()));
    let raised = raise_through(denied, Ok(&refusing), Ok(&kit)).unwrap();
    assert_eq!(
        (raised.by, raised.value),
        (RaisedBy::RealtimeKit, DECODE_NICE)
    );
    // No session bus (or no portal) at all.
    let raised = raise_through(denied, Err("session bus: none".into()), Ok(&kit)).unwrap();
    assert_eq!(raised.by, RaisedBy::RealtimeKit);
    // The label says "priority" where the value is a Windows thread priority.
    let kind = if cfg!(windows) { "priority" } else { "nice" };
    assert_eq!(raised.to_string(), format!("{kind} -8 by RealtimeKit"));
}

/// Nightjar issue 86: inside a Flatpak sandbox RealtimeKit was handed the sandbox's thread
/// id, which on the host names another thread or none.
#[test]
fn a_sandboxed_process_asks_only_the_portal() {
    assert_eq!(services_asked(true), [RaisedBy::Portal]);
    assert_eq!(
        services_asked(false),
        [RaisedBy::Portal, RaisedBy::RealtimeKit]
    );
}

#[test]
fn a_service_raise_is_clamped_to_its_min_nice_level() {
    assert_eq!(clamp_nice(-8, -15), Some(-8));
    assert_eq!(clamp_nice(-8, -5), Some(-5));
    assert_eq!(clamp_nice(-8, 0), None);
    let portal = FakeService::new(Ok(-5), Ok(()));
    let raised = raise_through(denied, Ok(&portal), Err("unused".into())).unwrap();
    assert_eq!((raised.by, raised.value), (RaisedBy::Portal, -5));
    assert_eq!(*portal.calls.borrow(), [(42, -5)]);
    // A service that grants nothing is skipped without a call.
    let stingy = FakeService::new(Ok(0), Ok(()));
    let kit = FakeService::new(Ok(-11), Ok(()));
    let raised = raise_through(denied, Ok(&stingy), Ok(&kit)).unwrap();
    assert_eq!((raised.by, raised.value), (RaisedBy::RealtimeKit, -8));
    assert!(stingy.calls.borrow().is_empty());
}

#[test]
fn every_reason_is_said_when_nothing_raises() {
    let unreadable = FakeService::new(Err("UnknownProperty".into()), Ok(()));
    let e = raise_through(denied, Ok(&unreadable), Err("system bus: none".into())).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    let message = e.to_string();
    for part in [
        "setpriority:",
        "the desktop portal: UnknownProperty",
        "RealtimeKit: system bus: none",
    ] {
        assert!(message.contains(part), "{message}");
    }
    assert!(unreadable.calls.borrow().is_empty());
}

#[test]
fn only_a_privilege_refusal_asks_a_service() {
    let portal = FakeService::new(Ok(-15), Ok(()));
    let e = raise_through(
        |_| Err(io::Error::from(io::ErrorKind::InvalidInput)),
        Ok(&portal),
        Err("unused".into()),
    )
    .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    assert!(portal.calls.borrow().is_empty());
}

#[cfg(windows)]
#[test]
fn a_windows_thread_takes_its_level() {
    let levels = std::thread::spawn(|| {
        let before = windows_thread::current();
        raise_current_thread(ThreadLevel::Normal).unwrap();
        let normal = windows_thread::current();
        raise_current_thread(ThreadLevel::Stream).unwrap();
        let stream = windows_thread::current();
        raise_current_thread(ThreadLevel::Decode).unwrap();
        (before, normal, stream, windows_thread::current())
    })
    .join()
    .unwrap();
    assert_eq!(
        levels,
        (0, 0, WINDOWS_STREAM_PRIORITY, WINDOWS_DECODE_PRIORITY)
    );
}

// Windows ignores IP_TOS: the control socket is marked by joining a qWAVE control flow, once
// per socket, at its first send. qWAVE needs a route through an interface (loopback has none,
// ERROR_NOT_FOUND), so the flow points at TEST-NET-1 over the default route; nothing is sent.
#[cfg(windows)]
#[test]
fn the_windows_control_socket_joins_a_qwave_flow_once_per_socket() {
    let to: std::net::SocketAddr = "192.0.2.1:47999".parse().unwrap();
    let routed = |socket: &std::net::UdpSocket| socket.connect(to).is_ok();
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    if !routed(&probe) {
        eprintln!("skipped: no default route");
        return;
    }
    let mut flow = open_control_flow(|m| panic!("{m}")).expect("qWAVE on Windows");
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    assert!(flow.add(&socket, to).expect("joins the flow"));
    assert!(flow.flow_id().is_some());
    assert!(!flow.add(&socket, to).unwrap(), "once per socket");
    // A moved control socket leaves the flow, and the new one joins at its first send.
    flow.remove();
    assert_eq!(flow.flow_id(), None);
    let moved = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    assert!(flow.add(&moved, to).expect("the moved socket joins"));
    assert!(flow.flow_id().is_some());
    // A refusal is said once, and not retried for the same socket.
    let loopback = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let sink = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    assert!(flow.add(&loopback, sink.local_addr().unwrap()).is_err());
    assert!(!flow.add(&loopback, sink.local_addr().unwrap()).unwrap());
    assert_eq!(flow.flow_id(), None);
}

#[cfg(not(windows))]
#[test]
fn only_windows_marks_through_a_flow() {
    assert!(open_control_flow(|m| panic!("{m}")).is_none());
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn read_option<S: std::os::fd::AsRawFd>(socket: &S, level: i32, name: i32) -> i32 {
    let mut value: i32 = -1;
    let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            level,
            name,
            (&mut value as *mut i32).cast(),
            &mut length,
        )
    };
    assert_eq!(result, 0);
    value
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn read_priority<S: std::os::fd::AsRawFd>(socket: &S) -> i32 {
    read_option(socket, libc::SOL_SOCKET, libc::SO_PRIORITY)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn the_priority_reaches_the_socket() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    assert_eq!(read_priority(&socket), 0);
    socket.set_priority(5).unwrap();
    assert_eq!(read_priority(&socket), 5);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn the_control_socket_is_marked_and_unmarked() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    mark_control_socket(&socket, false, true).unwrap();
    assert_eq!(read_priority(&socket), 6);
    assert_eq!(
        read_option(&socket, libc::IPPROTO_IP, libc::IP_TOS),
        CONTROL_TOS
    );
    mark_control_socket(&socket, false, false).unwrap();
    assert_eq!(read_priority(&socket), 0);
    assert_eq!(read_option(&socket, libc::IPPROTO_IP, libc::IP_TOS), 0);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn an_ipv6_control_socket_gets_its_traffic_class() {
    // A machine without IPv6 loopback has nothing to test.
    let Ok(socket) = std::net::UdpSocket::bind("[::1]:0") else {
        return;
    };
    mark_control_socket(&socket, true, true).unwrap();
    assert_eq!(
        read_option(&socket, libc::IPPROTO_IPV6, libc::IPV6_TCLASS),
        CONTROL_TOS
    );
    // IPv4-mapped remotes use IP_TOS on the same socket, so it is marked too.
    assert_eq!(
        read_option(&socket, libc::IPPROTO_IP, libc::IP_TOS),
        CONTROL_TOS
    );
    mark_control_socket(&socket, true, false).unwrap();
    assert_eq!(
        read_option(&socket, libc::IPPROTO_IPV6, libc::IPV6_TCLASS),
        0
    );
    assert_eq!(read_option(&socket, libc::IPPROTO_IP, libc::IP_TOS), 0);
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_tcp_socket_takes_the_stream_priority_only_on_request() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let plain = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    prioritize_socket(&plain, Qos::BestEffort, false).unwrap();
    assert_eq!(read_priority(&plain), 0);
    let raised = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    prioritize_socket(&raised, Qos::BestEffort, true).unwrap();
    assert_eq!(read_priority(&raised), 6);
}
