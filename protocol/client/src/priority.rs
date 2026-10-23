//! Socket and thread priorities, as moonlight-common-c and Nightjar's `latency.c` set them.
//!
//! Three layers, all Linux and Android only (elsewhere they do nothing):
//!
//! - Socket QoS (`setSocketQos` in `PlatformSockets.c`) is always on: the audio and microphone
//!   sockets get `SO_PRIORITY` 6 and the video socket 5. It is a local queue hint, not DSCP
//!   marking that a router might reject.
//! - The control socket is always marked the way ENet's `ENET_SOCKOPT_QOS` does: `SO_PRIORITY`
//!   6 and DSCP Expedited Forwarding with ECT(1) (`IP_TOS` or `IPV6_TCLASS`). Routes that
//!   drop marked packets are handled by [`should_drop_control_marking`].
//! - "Prioritize stream traffic" (`AndroidNetworkPriority`) adds `SO_PRIORITY` 6 to the other
//!   sockets of the stream (the RTSP connection) and raises the receive, audio decode and
//!   control threads to nice -4. The video decode thread is not raised, as in the C engine.
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
/// The control socket's `IP_TOS` / `IPV6_TCLASS`: DSCP Expedited Forwarding (46) plus ECT(1)
/// for L4S, as `enet_socket_set_option(ENET_SOCKOPT_QOS, 1)` sets.
pub const CONTROL_TOS: i32 = 46 << 2 | 0x01;
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

/// Raises the calling thread to [`THREAD_NICE`] when "prioritize stream traffic" is on. On
/// Linux `setpriority(PRIO_PROCESS, 0)` is the calling thread only.
pub fn prioritize_current_thread(network_priority: bool) -> io::Result<()> {
    if !network_priority {
        return Ok(());
    }
    set_thread_nice(THREAD_NICE)
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
    fn the_control_marking_is_dropped_after_three_lost_connects() {
        // The fourth datagram is the first one sent plain.
        assert!(!should_drop_control_marking(2));
        assert!(should_drop_control_marking(3));
    }

    #[test]
    fn the_control_tos_is_expedited_forwarding_with_ect1() {
        assert_eq!(CONTROL_TOS, 0xB9);
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
