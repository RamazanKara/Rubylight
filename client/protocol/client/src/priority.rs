//! Socket and thread priorities, as moonlight-common-c and Nightjar's `latency.c` set them.
//!
//! Three socket layers, all Linux and Android only (elsewhere they do nothing) except the
//! control socket's marking, which Windows gets through qWAVE, and the thread priorities, which
//! Windows gets through `SetThreadPriority` ([`thread_level`]):
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
//!   or -4 when that is refused, as the PyroWave submission thread is. With direct submission
//!   the receive thread runs the decoder, so it gets that level instead of -4 when "prioritize
//!   stream traffic" is on.
//! - Windows raises the same threads with "prioritize stream traffic" on: the decoding thread
//!   to `THREAD_PRIORITY_HIGHEST`, the others to `THREAD_PRIORITY_ABOVE_NORMAL`.
//! - A Linux process that may not lower its nice value (Flatpak, an unprivileged desktop) asks
//!   the desktop portal and then RealtimeKit over D-Bus instead ([`raise_current_thread`]).
//! - The decode priority switch ([`decode_priority_switch`]) turns the direct submission level
//!   and everything Windows does off. On for the desktop; on Android the Debug setting "Decoder
//!   priority for receiving", off by default: raising VideoRecv to the decode level measured
//!   worse under load on the Retroid. Its environment variable or property wins, for A/B runs.
//!
//! The option is a plain argument, never process-global state, so two sessions in one
//! process do not affect each other.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

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

#[cfg(windows)]
mod qwave;

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

/// The environment variable that turns the decode priority on (`1`) or off (`0`) on the desktop.
pub const DECODE_PRIORITY_ENV: &str = "NIGHTJAR_DECODE_PRIORITY";
/// The Android system property that does the same (`adb shell setprop
/// debug.nightjar.decode_priority 0`); the former C engine's `latency.c` read it too.
pub const DECODE_PRIORITY_PROPERTY: &str = "debug.nightjar.decode_priority";
/// `THREAD_PRIORITY_ABOVE_NORMAL`: the Windows level of [`ThreadLevel::Stream`].
pub const WINDOWS_STREAM_PRIORITY: i32 = 1;
/// `THREAD_PRIORITY_HIGHEST`: the Windows level of [`ThreadLevel::Decode`].
pub const WINDOWS_DECODE_PRIORITY: i32 = 2;

/// A session thread, as far as its priority goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamThread {
    /// VideoRecv; with direct submission it also runs the decoder.
    VideoReceive { direct_submit: bool },
    /// The queued VideoDec thread (no direct submission), which also submits PyroWave frames.
    VideoDecode,
    /// AudioRecv, AudioDec and ControlRecv.
    Other,
}

/// How far a session thread is raised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadLevel {
    /// Left where the OS put it.
    Normal,
    /// [`THREAD_NICE`]; [`WINDOWS_STREAM_PRIORITY`] on Windows.
    Stream,
    /// [`DECODE_NICE`], or [`DECODE_FALLBACK_NICE`] when that is refused;
    /// [`WINDOWS_DECODE_PRIORITY`] on Windows.
    Decode,
}

/// The level of `thread` with "prioritize stream traffic" (`network_priority`) and the decode
/// priority switch ([`decode_priority_switch`]) as given.
pub fn thread_level(
    thread: StreamThread,
    network_priority: bool,
    decode_priority: bool,
) -> ThreadLevel {
    thread_level_on(thread, network_priority, decode_priority, cfg!(windows))
}

fn thread_level_on(
    thread: StreamThread,
    network_priority: bool,
    decode_priority: bool,
    windows: bool,
) -> ThreadLevel {
    // Windows raised no session thread before the switch: off keeps it that way, for A/B runs.
    if windows && !decode_priority {
        return ThreadLevel::Normal;
    }
    match thread {
        // Linux and Android always raised the queued decode thread; Windows only with
        // "prioritize stream traffic" on.
        StreamThread::VideoDecode if network_priority || !windows => ThreadLevel::Decode,
        // With direct submission the receive thread runs the decoder, so it gets the decoder's
        // level.
        StreamThread::VideoReceive {
            direct_submit: true,
        } if network_priority && decode_priority => ThreadLevel::Decode,
        StreamThread::VideoReceive { .. } | StreamThread::Other if network_priority => {
            ThreadLevel::Stream
        }
        _ => ThreadLevel::Normal,
    }
}

/// The decode priority switch: `setting` ([`crate::session::SessionConfig::receive_decode_priority`]:
/// on for the desktop, Android's Debug setting, on by default) unless [`DECODE_PRIORITY_ENV`]
/// (desktop) or [`DECODE_PRIORITY_PROPERTY`] (Android) is `1` or `0`.
pub fn decode_priority_switch(setting: bool) -> bool {
    #[cfg(target_os = "android")]
    let value = android_property(DECODE_PRIORITY_PROPERTY);
    #[cfg(not(target_os = "android"))]
    let value = std::env::var(DECODE_PRIORITY_ENV).ok();
    switch_on(value.as_deref(), setting)
}

/// `1` turns a switch on and `0` off; anything else, or nothing, leaves `setting`.
fn switch_on(value: Option<&str>, setting: bool) -> bool {
    match value.map(str::trim) {
        Some("1") => true,
        Some("0") => false,
        _ => setting,
    }
}

#[cfg(target_os = "android")]
fn android_property(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut value = [0 as libc::c_char; libc::PROP_VALUE_MAX as usize];
    // SAFETY: a NUL-terminated name and a buffer of PROP_VALUE_MAX bytes, as the call requires.
    let length = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    if length <= 0 {
        return None;
    }
    // SAFETY: the call NUL-terminates what it wrote into the buffer.
    let value = unsafe { std::ffi::CStr::from_ptr(value.as_ptr()) };
    Some(value.to_string_lossy().into_owned())
}

/// The `SetThreadPriority` value of `level`, or `None` to leave the thread alone.
pub fn windows_thread_priority(level: ThreadLevel) -> Option<i32> {
    match level {
        ThreadLevel::Normal => None,
        ThreadLevel::Stream => Some(WINDOWS_STREAM_PRIORITY),
        ThreadLevel::Decode => Some(WINDOWS_DECODE_PRIORITY),
    }
}

/// What raised a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaisedBy {
    /// `setpriority` on Linux and Android, `SetThreadPriority` on Windows.
    Os,
    /// xdg-desktop-portal's `org.freedesktop.portal.Realtime` on the session bus, which works
    /// inside Flatpak.
    Portal,
    /// `org.freedesktop.RealtimeKit1` on the system bus.
    RealtimeKit,
}

impl std::fmt::Display for RaisedBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RaisedBy::Os if cfg!(windows) => "SetThreadPriority",
            RaisedBy::Os => "setpriority",
            RaisedBy::Portal => "the desktop portal",
            RaisedBy::RealtimeKit => "RealtimeKit",
        })
    }
}

/// A raised thread: by what, and to which nice value (Windows: thread priority).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raised {
    pub by: RaisedBy,
    pub value: i32,
}

impl std::fmt::Display for Raised {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = if cfg!(windows) { "priority" } else { "nice" };
        write!(f, "{kind} {} by {}", self.value, self.by)
    }
}

/// Raises the calling thread to `level`; `None` when the level leaves it alone. On Linux
/// `setpriority(PRIO_PROCESS, 0)` is the calling thread only, and a process without the right to
/// lower its nice value (a Flatpak, any unprivileged desktop) asks the desktop portal and then
/// RealtimeKit instead ([`raise_nice_with`]). On Windows a relative `SetThreadPriority`, which
/// needs no undoing when the thread ends (MMCSS would, and it throttles the other network traffic
/// while a task is registered). Called once per thread, as it starts.
pub fn raise_current_thread(level: ThreadLevel) -> io::Result<Option<Raised>> {
    #[cfg(windows)]
    let result = match windows_thread_priority(level) {
        None => Ok(None),
        Some(value) => windows_thread::set_current(value).map(|()| {
            Some(Raised {
                by: RaisedBy::Os,
                value,
            })
        }),
    };
    #[cfg(not(windows))]
    let result = {
        let (nice, fallback) = match level {
            ThreadLevel::Normal => return Ok(None),
            ThreadLevel::Stream => (THREAD_NICE, None),
            ThreadLevel::Decode => (DECODE_NICE, Some(DECODE_FALLBACK_NICE)),
        };
        // The id of this thread, not the helper's that asks the services.
        let tid = current_thread_id();
        #[cfg(target_os = "linux")]
        let ask = |nice| {
            realtime::broker()?
                .run(SERVICE_BUDGET, move || {
                    ask_services(nice, tid, realtime::services())
                })
                .and_then(|raised| raised)
        };
        // Android apps have no D-Bus: setpriority only.
        #[cfg(not(target_os = "linux"))]
        let ask = |_| {
            let _ = tid;
            Err(Vec::new())
        };
        raise_nice_with(nice, fallback, set_thread_nice, ask).map(Some)
    };
    result
}

/// A D-Bus service that raises a thread of an unprivileged process: the desktop portal's
/// `org.freedesktop.portal.Realtime` or `org.freedesktop.RealtimeKit1`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) trait RealtimeService {
    /// The service's `MinNiceLevel` property: the lowest nice value it grants.
    fn min_nice_level(&self) -> Result<i32, String>;
    /// `MakeThreadHighPriorityWithPID` (portal) or `MakeThreadHighPriority` (RealtimeKit).
    fn make_thread_high_priority(&self, tid: u64, nice: i32) -> Result<(), String>;
}

/// `nice` clamped to a service's `MinNiceLevel`; `None` when that is no raise at all.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn clamp_nice(nice: i32, min_nice_level: i32) -> Option<i32> {
    let nice = nice.max(min_nice_level);
    (nice < 0).then_some(nice)
}

/// The longest a stream thread's start waits on the D-Bus services, all of them together.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SERVICE_BUDGET: Duration = Duration::from_millis(500);

/// Tries `set_nice(nice)`, then `set_nice(fallback)`, then, only when the kernel refused for lack
/// of privilege (EPERM, EACCES), `ask(nice)` ([`ask_services`] behind a [`Broker`] on Linux).
/// The error lists every reason.
#[cfg_attr(windows, allow(dead_code))]
fn raise_nice_with(
    nice: i32,
    fallback: Option<i32>,
    set_nice: impl Fn(i32) -> io::Result<()>,
    ask: impl FnOnce(i32) -> Result<Raised, Vec<String>>,
) -> io::Result<Raised> {
    let refused = match set_nice(nice) {
        Ok(()) => {
            return Ok(Raised {
                by: RaisedBy::Os,
                value: nice,
            })
        }
        Err(e) => e,
    };
    if let Some(value) = fallback.filter(|&value| set_nice(value).is_ok()) {
        return Ok(Raised {
            by: RaisedBy::Os,
            value,
        });
    }
    if refused.kind() != io::ErrorKind::PermissionDenied {
        return Err(refused);
    }
    match ask(nice) {
        Ok(raised) => Ok(raised),
        Err(reasons) => {
            let reasons = std::iter::once(format!("setpriority: {refused}")).chain(reasons);
            Err(io::Error::new(
                refused.kind(),
                reasons.collect::<Vec<_>>().join("; "),
            ))
        }
    }
}

/// The services a process asks, in order. Inside a Flatpak sandbox only the portal: it maps the
/// sandbox's thread id to the host's, while RealtimeKit would take that id for one of the host's
/// own, which is another process's thread or none.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn services_asked(sandboxed: bool) -> &'static [RaisedBy] {
    if sandboxed {
        &[RaisedBy::Portal]
    } else {
        &[RaisedBy::Portal, RaisedBy::RealtimeKit]
    }
}

/// A service as [`ask_services`] takes it: what it is, its process-wide "gone" flag, and the
/// service or why it could not be opened.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
type ServiceEntry<'a> = (
    RaisedBy,
    &'a AtomicBool,
    Result<&'a dyn RealtimeService, String>,
);

/// Asks each service in turn to raise thread `tid` to `nice` clamped to its `MinNiceLevel`; a
/// service is opened only when the one before it failed. A service that can't be reached, has
/// no `MinNiceLevel` or grants no raise sets its "gone" flag and is skipped from then on, for the
/// whole process. A refused call is per thread and is not remembered.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ask_services<'a>(
    nice: i32,
    tid: u64,
    services: impl IntoIterator<Item = ServiceEntry<'a>>,
) -> Result<Raised, Vec<String>> {
    let mut reasons = Vec::new();
    for (by, gone, service) in services {
        if gone.load(Ordering::Acquire) {
            reasons.push(format!("{by}: skipped, it failed for an earlier thread"));
            continue;
        }
        let raised = service.map_err(|e| (e, true)).and_then(|service| {
            let min = service.min_nice_level().map_err(|e| (e, true))?;
            let value = clamp_nice(nice, min)
                .ok_or_else(|| (format!("grants no raise (MinNiceLevel {min})"), true))?;
            service
                .make_thread_high_priority(tid, value)
                .map_err(|e| (e, false))?;
            Ok(value)
        });
        match raised {
            Ok(value) => return Ok(Raised { by, value }),
            Err((e, remember)) => {
                if remember {
                    gone.store(true, Ordering::Release);
                }
                reasons.push(format!("{by}: {e}"));
            }
        }
    }
    Err(reasons)
}

/// A job for the [`Broker`]'s helper thread.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
type BrokerJob = Box<dyn FnOnce() + Send>;

/// Runs the D-Bus part of a raise on one helper thread, so a bus that hangs (connecting
/// included, which zbus has no timeout for) costs a stream thread at most the budget. After one
/// job ran out of time every later one is refused at once: the helper may still be stuck in it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Broker {
    jobs: mpsc::Sender<BrokerJob>,
    stuck: AtomicBool,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Broker {
    fn new() -> io::Result<Self> {
        let (jobs, queue) = mpsc::channel::<BrokerJob>();
        std::thread::Builder::new()
            .name("RealtimeAsk".into())
            .spawn(move || queue.into_iter().for_each(|job| job()))?;
        Ok(Self {
            jobs,
            stuck: AtomicBool::new(false),
        })
    }

    /// `job`'s result, or why there is none within `budget`.
    fn run<T: Send + 'static>(
        &self,
        budget: Duration,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Vec<String>> {
        if self.stuck.load(Ordering::Acquire) {
            return Err(vec![
                "D-Bus: skipped, an earlier thread got no answer in time".into(),
            ]);
        }
        let (answer, wait) = mpsc::sync_channel(1);
        let abandoned = Arc::new(AtomicBool::new(false));
        let skip = abandoned.clone();
        let job: BrokerJob = Box::new(move || {
            // A job queued behind one that ran out of time has nobody waiting for it any more.
            if !skip.load(Ordering::Acquire) {
                let _ = answer.send(job());
            }
        });
        if self.jobs.send(job).is_err() {
            return Err(vec!["D-Bus: the helper thread is gone".into()]);
        }
        match wait.recv_timeout(budget) {
            Ok(value) => Ok(value),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                abandoned.store(true, Ordering::Release);
                self.stuck.store(true, Ordering::Release);
                Err(vec![format!(
                    "D-Bus: no answer within {} ms",
                    budget.as_millis()
                )])
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(vec!["D-Bus: the helper thread failed".into()])
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn current_thread_id() -> u64 {
    // SAFETY: gettid takes no arguments and cannot fail.
    unsafe { libc::syscall(libc::SYS_gettid) as u64 }
}

#[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
fn current_thread_id() -> u64 {
    0
}

/// The D-Bus services behind [`ask_services`]: one connection per bus, opened on the broker's
/// helper thread when a thread first needs it, and kept (a failure too).
#[cfg(target_os = "linux")]
mod realtime;

#[cfg(windows)]
mod windows_thread {
    use std::io;

    // Handles as isize, as the receive bench declares GetCurrentThread.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        #[cfg(test)]
        fn GetThreadPriority(thread: isize) -> i32;
    }

    pub(super) fn set_current(priority: i32) -> io::Result<()> {
        // SAFETY: the calling thread's pseudo handle, which needs no closing.
        if unsafe { SetThreadPriority(GetCurrentThread(), priority) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    pub(super) fn current() -> i32 {
        // SAFETY: the calling thread's pseudo handle.
        unsafe { GetThreadPriority(GetCurrentThread()) }
    }
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

#[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
fn set_thread_nice(_nice: i32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
