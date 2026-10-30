//! Socket and thread priorities, as moonlight-common-c and Nightjar's `latency.c` set them.
//!
//! Three layers, all Linux and Android only (elsewhere they do nothing) except the control
//! socket's marking, which Windows gets through qWAVE:
//!
//! - Socket QoS (`setSocketQos` in `PlatformSockets.c`) is always on: the audio and microphone
//!   sockets get `SO_PRIORITY` 6 and the video socket 5, a local queue hint. They also get DSCP
//!   marking ([`media_tos`]): Expedited Forwarding for audio and microphone, AF41 for video. A
//!   socket the OS refuses it stays unmarked.
//! - The control socket is always marked the way ENet's `ENET_SOCKOPT_QOS` does: `SO_PRIORITY`
//!   6 and DSCP Expedited Forwarding with ECT(1) (`IP_TOS` or `IPV6_TCLASS`); on Windows a
//!   qWAVE control flow, DSCP 56 ([`ControlFlow`]). Routes that drop marked packets are
//!   handled by [`should_drop_control_marking`].
//! - "Prioritize stream traffic" (`AndroidNetworkPriority`) adds `SO_PRIORITY` 6 to the other
//!   sockets of the stream (the RTSP connection) and raises the receive, audio decode and
//!   control threads to nice -4.
//! - The queued video decode thread is always raised to nice -8 (`THREAD_PRIORITY_URGENT_DISPLAY`),
//!   or -4 when that is refused, as the PyroWave submission thread is.
//!
//! The option is a plain argument, never process-global state, so two sessions in one
//! process do not affect each other.

use std::io;

/// `SO_PRIORITY` of the sockets that carry audio and video, and of the others with
/// "prioritize stream traffic" on.
pub const AUDIO_PRIORITY: i32 = 6;
pub const VIDEO_PRIORITY: i32 = 5;
/// The nice value of the stream's threads with "prioritize stream traffic" on.
pub const THREAD_NICE: i32 = -4;
/// The nice value of the queued video decode thread: Android's `THREAD_PRIORITY_URGENT_DISPLAY`.
pub const DECODE_NICE: i32 = -8;
/// The decode thread's fallback when [`DECODE_NICE`] is refused: `THREAD_PRIORITY_DISPLAY`.
pub const DECODE_FALLBACK_NICE: i32 = -4;
/// The control socket's `IP_TOS` / `IPV6_TCLASS`: DSCP Expedited Forwarding (46) plus ECT(1)
/// for L4S, as `enet_socket_set_option(ENET_SOCKOPT_QOS, 1)` sets.
pub const CONTROL_TOS: i32 = 46 << 2 | 0x01;
/// The audio and microphone sockets' `IP_TOS` / `IPV6_TCLASS`: DSCP Expedited Forwarding (46).
pub const AUDIO_TOS: i32 = 46 << 2;
/// The video socket's: DSCP AF41 (34).
pub const VIDEO_TOS: i32 = 34 << 2;
/// Connect attempts lost in a row before the control socket's marking is taken off.
pub const CONTROL_MARKING_LOST_LIMIT: u32 = 3;

/// `SOCK_QOS_TYPE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Qos {
    BestEffort,
    Audio,
    Video,
}

/// The `SO_PRIORITY` a socket of this kind gets, or `None` to leave the OS default.
pub fn socket_priority(qos: Qos, network_priority: bool) -> Option<i32> {
    match qos {
        Qos::Audio => Some(AUDIO_PRIORITY),
        Qos::Video => Some(VIDEO_PRIORITY),
        Qos::BestEffort => network_priority.then_some(AUDIO_PRIORITY),
    }
}

/// Applies [`socket_priority`] to `socket`.
pub fn prioritize_socket<S>(socket: &S, qos: Qos, network_priority: bool) -> io::Result<()>
where
    S: PriorityTarget,
{
    match socket_priority(qos, network_priority) {
        Some(value) => socket.set_priority(value),
        None => Ok(()),
    }
}

/// The `IP_TOS` / `IPV6_TCLASS` a socket of this kind gets, or `None` to leave it unmarked, as
/// Moonlight V+'s moonlight-common-c (qiin2333/moonlight-common-c c7ebb7cb3) marks its audio,
/// microphone and video sockets.
pub fn media_tos(qos: Qos) -> Option<i32> {
    match qos {
        Qos::Audio => Some(AUDIO_TOS),
        Qos::Video => Some(VIDEO_TOS),
        Qos::BestEffort => None,
    }
}

/// Applies [`media_tos`] to `socket`; an `Err` leaves it unmarked.
pub fn mark_media_socket<S>(socket: &S, qos: Qos, ipv6: bool) -> io::Result<()>
where
    S: PriorityTarget,
{
    match media_tos(qos) {
        Some(value) => socket.set_tos(ipv6, value),
        None => Ok(()),
    }
}

/// Marks (or unmarks) the control socket as ENet does: `SO_PRIORITY` 6 and [`CONTROL_TOS`].
/// Every option is tried; the first failure is returned.
pub fn mark_control_socket<S>(socket: &S, ipv6: bool, on: bool) -> io::Result<()>
where
    S: PriorityTarget,
{
    let tos = socket.set_tos(ipv6, if on { CONTROL_TOS } else { 0 });
    let priority = socket.set_priority(if on { AUDIO_PRIORITY } else { 0 });
    tos.and(priority)
}

/// ENet takes the marking off when the first connect attempts get no answer in a row: some
/// networks drop marked packets, so it tries again without. `unanswered` counts the
/// datagrams sent so far without a reply.
pub fn should_drop_control_marking(unanswered: u32) -> bool {
    unanswered >= CONTROL_MARKING_LOST_LIMIT
}

/// Sockets that take `SO_PRIORITY` and the type-of-service byte.
pub trait PriorityTarget {
    fn set_priority(&self, value: i32) -> io::Result<()>;
    /// `IP_TOS` for IPv4 or `IPV6_TCLASS` for IPv6.
    fn set_tos(&self, ipv6: bool, value: i32) -> io::Result<()>;
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl<S: std::os::fd::AsRawFd> PriorityTarget for S {
    fn set_priority(&self, value: i32) -> io::Result<()> {
        set_int_option(self.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PRIORITY, value)
    }

    fn set_tos(&self, ipv6: bool, value: i32) -> io::Result<()> {
        if ipv6 {
            let result = set_int_option(
                self.as_raw_fd(),
                libc::IPPROTO_IPV6,
                libc::IPV6_TCLASS,
                value,
            );
            // ENet sets both on every socket: IP_TOS is what an IPv4-mapped remote on an
            // IPv6 socket uses. Its failure does not matter.
            let _ = set_int_option(self.as_raw_fd(), libc::IPPROTO_IP, libc::IP_TOS, value);
            result
        } else {
            set_int_option(self.as_raw_fd(), libc::IPPROTO_IP, libc::IP_TOS, value)
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn set_int_option(
    fd: std::os::fd::RawFd,
    level: libc::c_int,
    name: libc::c_int,
    value: i32,
) -> io::Result<()> {
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const i32).cast(),
            std::mem::size_of::<i32>() as libc::socklen_t,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
impl<S> PriorityTarget for S {
    fn set_priority(&self, _value: i32) -> io::Result<()> {
        Ok(())
    }

    fn set_tos(&self, _ipv6: bool, _value: i32) -> io::Result<()> {
        Ok(())
    }
}

/// Windows: the control socket's qWAVE flow, which is how ENet's `win32.c` marks it for
/// `ENET_SOCKOPT_QOS` in moonlight-common-c. Windows ignores `IP_TOS` and has no `SO_PRIORITY`,
/// so [`mark_control_socket`] does nothing there; a `QOSTrafficTypeControl` flow marks the
/// datagrams with DSCP 56 (CS7) instead, without administrator rights. A flow on an unconnected
/// UDP socket needs the destination, so the socket joins it at its first send
/// ([`ControlFlow::add`]). Closing the flow (dropping it) takes the marking off again.
/// `qwave.dll` is loaded at run time, as there: a Windows without it (Windows Server without the
/// qWAVE feature) streams unmarked, and [`ControlFlow::open`] says why.
#[cfg(windows)]
pub use self::qwave::ControlFlow;

/// Elsewhere [`mark_control_socket`] marks the control socket itself: there is no flow.
#[cfg(not(windows))]
pub enum ControlFlow {}

#[cfg(not(windows))]
impl ControlFlow {
    pub fn add<S>(&mut self, _socket: &S, _to: std::net::SocketAddr) -> io::Result<bool> {
        match *self {}
    }

    pub fn remove(&mut self) {
        match *self {}
    }
}

/// The control socket's flow where the OS marks through one (Windows); None elsewhere, or with
/// `warn` hearing why the socket goes unmarked.
pub fn open_control_flow(warn: impl Fn(&str)) -> Option<ControlFlow> {
    #[cfg(windows)]
    let flow = ControlFlow::open()
        .map_err(|e| warn(&format!("Could not mark the control socket for QoS: {e}")))
        .ok();
    #[cfg(not(windows))]
    let flow = {
        let _ = warn;
        None
    };
    flow
}

#[cfg(windows)]
mod qwave {
    use std::ffi::c_void;
    use std::io;
    use std::net::SocketAddr;
    use std::os::windows::io::AsRawSocket;
    use std::sync::OnceLock;

    /// `QOSTrafficTypeControl`: DSCP 56, the highest of the traffic types.
    const TRAFFIC_TYPE_CONTROL: i32 = 5;
    /// `QOS_NON_ADAPTIVE_FLOW`: no network experiments, as ENet asks.
    const NON_ADAPTIVE_FLOW: u32 = 0x2;

    #[repr(C)]
    struct QosVersion {
        major: u16,
        minor: u16,
    }

    type Handle = *mut c_void;
    type CreateHandle = unsafe extern "system" fn(*const QosVersion, *mut Handle) -> i32;
    type CloseHandle = unsafe extern "system" fn(Handle) -> i32;
    type AddSocketToFlow =
        unsafe extern "system" fn(Handle, usize, *const c_void, i32, u32, *mut u32) -> i32;
    type RemoveSocketFromFlow = unsafe extern "system" fn(Handle, usize, u32, u32) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    struct Qwave {
        create: CreateHandle,
        close: CloseHandle,
        add: AddSocketToFlow,
        remove: RemoveSocketFromFlow,
    }

    /// qwave.dll's functions, loaded once and kept for the process.
    fn qwave() -> Result<&'static Qwave, String> {
        static QWAVE: OnceLock<Result<Qwave, String>> = OnceLock::new();
        QWAVE
            .get_or_init(|| {
                let name: Vec<u16> = "qwave.dll\0".encode_utf16().collect();
                // SAFETY: a NUL-terminated name; the module is never freed.
                let module = unsafe { LoadLibraryW(name.as_ptr()) };
                if module.is_null() {
                    return Err(format!("qwave.dll: {}", io::Error::last_os_error()));
                }
                let find = |symbol: &[u8]| {
                    // SAFETY: a NUL-terminated name in a loaded module.
                    let address = unsafe { GetProcAddress(module, symbol.as_ptr()) };
                    (!address.is_null()).then_some(address).ok_or_else(|| {
                        let name = String::from_utf8_lossy(&symbol[..symbol.len() - 1]);
                        format!("qwave.dll has no {name}")
                    })
                };
                // SAFETY: the documented signatures of these qWAVE exports.
                unsafe {
                    Ok(Qwave {
                        create: std::mem::transmute::<*mut c_void, CreateHandle>(find(b"QOSCreateHandle\0")?),
                        close: std::mem::transmute::<*mut c_void, CloseHandle>(find(b"QOSCloseHandle\0")?),
                        add: std::mem::transmute::<*mut c_void, AddSocketToFlow>(find(b"QOSAddSocketToFlow\0")?),
                        remove: std::mem::transmute::<*mut c_void, RemoveSocketFromFlow>(find(
                            b"QOSRemoveSocketFromFlow\0",
                        )?),
                    })
                }
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// One qWAVE handle and the control socket's flow on it.
    pub struct ControlFlow {
        qwave: &'static Qwave,
        handle: Handle,
        /// The socket that joined (or tried to join) the flow, and the flow's id when it did.
        joined: Option<(usize, Option<u32>)>,
    }

    // SAFETY: the handle is used from one thread at a time (the control thread owns it).
    unsafe impl Send for ControlFlow {}

    impl ControlFlow {
        /// `QOSCreateHandle`; Err says why the control socket goes unmarked.
        pub fn open() -> io::Result<Self> {
            let qwave = qwave().map_err(io::Error::other)?;
            let mut handle: Handle = std::ptr::null_mut();
            let version = QosVersion { major: 1, minor: 0 };
            // SAFETY: valid pointers to a version and a handle slot.
            if unsafe { (qwave.create)(&version, &mut handle) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                qwave,
                handle,
                joined: None,
            })
        }

        /// Adds `socket` to the flow toward `to` before its first send; Ok(false) when this
        /// socket already joined or tried to. A refusal is returned once and not retried, as ENet
        /// does: the socket then sends unmarked.
        pub fn add(&mut self, socket: &impl AsRawSocket, to: SocketAddr) -> io::Result<bool> {
            let raw = socket.as_raw_socket() as usize;
            if self.joined.is_some_and(|(joined, _)| joined == raw) {
                return Ok(false);
            }
            self.remove();
            let destination = socket2::SockAddr::from(to);
            let mut flow = 0u32;
            // SAFETY: a live socket, a valid address of its family and a zeroed flow id.
            let added = unsafe {
                (self.qwave.add)(
                    self.handle,
                    raw,
                    destination.as_ptr().cast(),
                    TRAFFIC_TYPE_CONTROL,
                    NON_ADAPTIVE_FLOW,
                    &mut flow,
                )
            } != 0;
            let error = (!added).then(io::Error::last_os_error);
            self.joined = Some((raw, added.then_some(flow)));
            error.map_or(Ok(true), Err)
        }

        /// Takes the socket that joined out of the flow, before it is closed or replaced.
        pub fn remove(&mut self) {
            if let Some((socket, Some(flow))) = self.joined.take() {
                // SAFETY: the socket joined this flow on this handle and is still open.
                unsafe { (self.qwave.remove)(self.handle, socket, flow, 0) };
            }
        }

        /// The flow's id while a socket is in it.
        pub fn flow_id(&self) -> Option<u32> {
            self.joined.and_then(|(_, flow)| flow)
        }
    }

    impl Drop for ControlFlow {
        fn drop(&mut self) {
            self.remove();
            // SAFETY: the handle from QOSCreateHandle, closed once.
            unsafe { (self.qwave.close)(self.handle) };
        }
    }
}

/// Raises the calling thread to [`THREAD_NICE`] when "prioritize stream traffic" is on. On
/// Linux `setpriority(PRIO_PROCESS, 0)` is the calling thread only.
pub fn prioritize_current_thread(network_priority: bool) -> io::Result<()> {
    if !network_priority {
        return Ok(());
    }
    set_thread_nice(THREAD_NICE)
}

/// Raises the calling video decode thread to [`DECODE_NICE`], or [`DECODE_FALLBACK_NICE`]
/// when that is refused. Independent of "prioritize stream traffic": the decoder sits on the
/// frame's critical path, as the PyroWave submission thread does.
pub fn prioritize_decode_thread() -> io::Result<()> {
    set_thread_nice(DECODE_NICE).or_else(|_| set_thread_nice(DECODE_FALLBACK_NICE))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn set_thread_nice(nice: i32) -> io::Result<()> {
    let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn set_thread_nice(_nice: i32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(read_option(&audio, libc::IPPROTO_IP, libc::IP_TOS), AUDIO_TOS);
        let video = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        mark_media_socket(&video, Qos::Video, false).unwrap();
        assert_eq!(read_option(&video, libc::IPPROTO_IP, libc::IP_TOS), VIDEO_TOS);
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
}
