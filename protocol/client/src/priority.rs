//! Socket and thread priorities, as moonlight-common-c and Nightjar's `latency.c` set them.
//!
//! Two layers, both Linux and Android only (elsewhere they do nothing):
//!
//! - Socket QoS (`setSocketQos` in `PlatformSockets.c`) is always on: the audio socket gets
//!   `SO_PRIORITY` 6 and the video socket 5. It is a local queue hint, not DSCP marking that
//!   a router might reject.
//! - "Prioritize stream traffic" (`AndroidNetworkPriority`) adds `SO_PRIORITY` 6 to the other
//!   sockets of the stream (control, microphone) and raises the receive, decode and control
//!   threads to nice -4.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

static NETWORK_PRIORITY: AtomicBool = AtomicBool::new(false);

/// `SO_PRIORITY` of the sockets that carry audio and video, and of the others with
/// "prioritize stream traffic" on.
pub const AUDIO_PRIORITY: i32 = 6;
pub const VIDEO_PRIORITY: i32 = 5;
/// The nice value of the stream's threads with "prioritize stream traffic" on.
pub const THREAD_NICE: i32 = -4;

/// `SOCK_QOS_TYPE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Qos {
    BestEffort,
    Audio,
    Video,
}

/// Turns "prioritize stream traffic" on or off for the sockets and threads created from now
/// on. A session sets it from its config before opening anything.
pub fn set_network_priority(enabled: bool) {
    NETWORK_PRIORITY.store(enabled, Ordering::SeqCst);
}

pub fn network_priority() -> bool {
    NETWORK_PRIORITY.load(Ordering::SeqCst)
}

/// The `SO_PRIORITY` a socket of this kind gets, or `None` to leave the OS default.
pub fn socket_priority(qos: Qos, network_priority: bool) -> Option<i32> {
    match qos {
        Qos::Audio => Some(AUDIO_PRIORITY),
        Qos::Video => Some(VIDEO_PRIORITY),
        Qos::BestEffort => network_priority.then_some(AUDIO_PRIORITY),
    }
}

/// Applies [`socket_priority`] to `socket` (the current global option).
pub fn prioritize_socket<S>(socket: &S, qos: Qos) -> io::Result<()>
where
    S: PriorityTarget,
{
    match socket_priority(qos, network_priority()) {
        Some(value) => socket.set_priority(value),
        None => Ok(()),
    }
}

/// Sockets that take `SO_PRIORITY`.
pub trait PriorityTarget {
    fn set_priority(&self, value: i32) -> io::Result<()>;
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl<S: std::os::fd::AsRawFd> PriorityTarget for S {
    fn set_priority(&self, value: i32) -> io::Result<()> {
        let result = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PRIORITY,
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
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
impl<S> PriorityTarget for S {
    fn set_priority(&self, _value: i32) -> io::Result<()> {
        Ok(())
    }
}

/// Raises the calling thread to [`THREAD_NICE`] when "prioritize stream traffic" is on. On
/// Linux `setpriority(PRIO_PROCESS, 0)` is the calling thread only.
pub fn prioritize_current_thread() -> io::Result<()> {
    if !network_priority() {
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
mod tests {
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

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn read_priority(socket: &std::net::UdpSocket) -> i32 {
        use std::os::fd::AsRawFd;
        let mut value: i32 = -1;
        let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PRIORITY,
                (&mut value as *mut i32).cast(),
                &mut length,
            )
        };
        assert_eq!(result, 0);
        value
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn the_priority_reaches_the_socket() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        assert_eq!(read_priority(&socket), 0);
        socket.set_priority(5).unwrap();
        assert_eq!(read_priority(&socket), 5);
    }
}
