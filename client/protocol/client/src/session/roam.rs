//! Client roaming's sockets: the route poll, the media rebind and the front end's network
//! hints, around the control stream's roaming attempt ([`ControlHandle::network_changed`]).
//! See `docs/protocol/client-roaming.md`.
//!
//! The control, video, audio and microphone sockets are bound to the local address toward
//! the host and move together to new ports on the new one: everything is bound before
//! anything moves, so a failed bind leaves them all where they were; a control socket that
//! fails to follow the media is moved again with them, or the session ends. The host takes media
//! pings only from the address the control stream roamed to, so on Roamed media left
//! anywhere else follow it, or the session fails. The decoders, renderers and the session
//! object stay. The host resets its link rate and pacing and asks for an IDR when the roam
//! commits, and the control stream pings ENet on Roamed, so nothing else is reset here. A
//! roam that fails leaves the stream to the network grace period and the app's recovery.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rubylight_protocol::roaming::SESSION_TAG_BYTES;

use super::net::{self, NextSocket, PingHandle, StopSignal};
use super::{Microphone, Shared};
use crate::control::driver::ControlHandle;
use crate::crypto::Key;
use crate::priority::Qos;
use crate::roaming::{self, LaunchSeq};

/// How often the route toward the host is looked up.
const ROUTE_POLL: Duration = Duration::from_secs(1);
/// With [`RoamingConfig::platform_hints`], how long after a hint the route poll may move.
const HINT_WINDOW: Duration = Duration::from_secs(5);
/// Launch keys whose roaming seqs are kept; a reconnect reuses one of the newest launches.
const LAUNCH_SEQS: usize = 4;

/// The roaming seqs sent under `rikey` by earlier connections of this process. The host
/// keeps the last seq per sender per launch key, so a reconnect under the same key must
/// continue above it or its PROBEs are dropped as replays.
pub(super) fn launch_seq(rikey: &Key) -> LaunchSeq {
    static SEQS: Mutex<VecDeque<([u8; SESSION_TAG_BYTES], LaunchSeq)>> =
        Mutex::new(VecDeque::new());
    // The session tag stands for the key, which stays out of the registry.
    let (_, tag) = roaming::derive(rikey);
    let mut seqs = lock(&SEQS);
    let seq = match seqs.iter().position(|(known, _)| *known == tag) {
        Some(index) => seqs.remove(index).expect("found").1,
        None => LaunchSeq::default(),
    };
    seqs.truncate(LAUNCH_SEQS - 1);
    seqs.push_front((tag, seq.clone()));
    seq
}

/// Client roaming (Experimental): keep the stream across a network change. It acts only
/// when the host advertised roaming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoamingConfig {
    /// "Client roaming", default on.
    pub enabled: bool,
    /// "Continue on mobile data": roam onto a metered network, default off.
    pub metered: bool,
    /// The front end reports every network change with [`super::Session::network_changed`]
    /// and whether it is metered (Android). The route poll then moves only within a few
    /// seconds of a hint that allowed roaming, never onto a network not reported yet.
    pub platform_hints: bool,
}

impl Default for RoamingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            metered: false,
            platform_hints: false,
        }
    }
}

/// What the front end knows about the network the client is on now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkHint {
    /// The network bills by data (mobile data, a metered Wi-Fi).
    pub metered: bool,
}

/// What a network change means for the stream ([`super::Session::network_changed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkChange {
    /// A roam started: the stream moves onto the new network.
    Roamed,
    /// The stream's connection goes on: the route toward the host still leaves from its
    /// address, or there is no route yet (the route poll, or the connection's own timeout,
    /// decides once there is).
    CarriesOn,
    /// The route toward the host now leaves from another address and the stream does not
    /// move there: its connection cannot carry on.
    CannotContinue,
}

impl NetworkChange {
    /// Without roaming: the stream's sockets on `local`, the route toward the host now.
    pub(super) fn standing(local: SocketAddr, route: io::Result<SocketAddr>) -> Self {
        match route {
            Ok(route) if !stays_on(local, route) => Self::CannotContinue,
            _ => Self::CarriesOn,
        }
    }
}

/// One media stream's socket: its pings and its receive thread move together.
pub(super) struct MediaPath {
    pub qos: Qos,
    /// The receive buffer to ask for (0 keeps the default).
    pub buffer: usize,
    pub ping: Option<PingHandle>,
    pub receive: Option<Arc<NextSocket>>,
}

struct State {
    /// The local address the media sockets are bound to.
    local: SocketAddr,
    /// The local address toward the host at the last look.
    route: SocketAddr,
    /// A metered network without the opt-in: no roaming until a hint allows it again.
    held: bool,
    /// With platform hints: until when the route poll may move.
    armed_until: Option<Instant>,
    /// A move left some sockets behind: the route poll moves them all again.
    retry: bool,
}

impl State {
    fn new(local: SocketAddr) -> Self {
        Self {
            local,
            route: local,
            held: false,
            armed_until: None,
            retry: false,
        }
    }

    /// A socket move failed; `retrying` says it was the retry of one that moved only some,
    /// `partly` that this one moved only some.
    fn move_failed(&mut self, retrying: bool, partly: bool) -> MoveFailed {
        if retrying {
            MoveFailed::End
        } else if partly {
            self.retry = true;
            MoveFailed::Retry
        } else {
            MoveFailed::Stays
        }
    }

    /// A hint that allows roaming: with platform hints the route poll may move until
    /// [`HINT_WINDOW`] from `now`.
    fn arm(&mut self, now: Instant) {
        self.armed_until = Some(now + HINT_WINDOW);
    }

    /// The route poll found `route`: whether the media move there. Only a route the sockets
    /// do not already leave on ([`stays_on`]) moves, never while held, and with platform
    /// hints only while armed. After a move that left some sockets behind, any route moves.
    fn route_moves(&mut self, route: SocketAddr, platform_hints: bool, now: Instant) -> bool {
        if self.retry {
            self.route = route;
            return true;
        }
        if same_address(route, self.route) {
            return false;
        }
        self.route = route;
        !stays_on(self.local, route)
            && !self.held
            && !(platform_hints && !self.armed_until.is_some_and(|until| now < until))
    }
}

/// The same local address: the port, which a lookup or a socket picks anew, and the IPv6 flow
/// label do not count; the IPv6 scope (the interface of a link-local address) does.
fn same_address(a: SocketAddr, b: SocketAddr) -> bool {
    match (a, b) {
        (SocketAddr::V6(a), SocketAddr::V6(b)) => a.ip() == b.ip() && a.scope_id() == b.scope_id(),
        _ => a.ip() == b.ip(),
    }
}

/// Whether the sockets bound to `local` still carry the stream though the route toward the
/// host now leaves from `route`: the same address, or an IPv6 address the interface rotated
/// (a privacy address in the same /64) while `local` is still assigned. Moving would be a
/// spurious roam: the network did not change.
fn stays_on(local: SocketAddr, route: SocketAddr) -> bool {
    if same_address(local, route) {
        return true;
    }
    let (SocketAddr::V6(old), SocketAddr::V6(new)) = (local, route) else {
        return false;
    };
    old.scope_id() == new.scope_id()
        && old.ip().segments()[..4] == new.ip().segments()[..4]
        && std::net::UdpSocket::bind(net::with_port(local, 0)).is_ok()
}

/// The session's roaming side, shared by the session and the route poll thread.
pub(super) struct Roamer {
    shared: Arc<Shared>,
    control: ControlHandle,
    /// The host, to look up the route toward.
    remote: SocketAddr,
    config: RoamingConfig,
    paths: Vec<MediaPath>,
    microphone: Arc<Mutex<Option<Microphone>>>,
    state: Mutex<State>,
    stop: StopSignal,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

impl Roamer {
    pub fn new(
        shared: Arc<Shared>,
        control: ControlHandle,
        remote: SocketAddr,
        local: SocketAddr,
        config: RoamingConfig,
        paths: Vec<MediaPath>,
        microphone: Arc<Mutex<Option<Microphone>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            shared,
            control,
            remote,
            config,
            paths,
            microphone,
            state: Mutex::new(State::new(local)),
            stop: StopSignal::default(),
        })
    }

    /// Starts the route poll: once a second, off the frame path.
    pub fn start_poll(self: &Arc<Self>) -> io::Result<JoinHandle<()>> {
        let roamer = self.clone();
        std::thread::Builder::new()
            .name("RoamPoll".into())
            .spawn(move || {
                while !roamer.stop.sleep(ROUTE_POLL) {
                    roamer.poll(Instant::now());
                }
            })
    }

    pub fn stop(&self) {
        self.stop.stop();
    }

    /// Looks up the route toward the host and moves the media when it changed.
    fn poll(&self, now: Instant) {
        // No route at all: nothing to move to yet, and the grace period runs.
        let Ok(route) = net::local_address_toward(self.remote) else {
            return;
        };
        let mut state = lock(&self.state);
        if state.route_moves(route, self.config.platform_hints, now) {
            self.roam(&mut state, route);
        }
    }

    /// [`super::Session::network_changed`].
    pub fn network_changed(&self, hint: NetworkHint) -> NetworkChange {
        let mut state = lock(&self.state);
        if hint.metered && !self.config.metered {
            if !state.held {
                self.shared.log(
                    "Roaming: on a metered network without \"Continue on mobile data\"; \
                     not roaming, the network grace period decides",
                );
            }
            state.held = true;
            state.armed_until = None;
            self.control.hold_roaming(true);
            return NetworkChange::standing(state.local, net::local_address_toward(self.remote));
        }
        if state.held {
            state.held = false;
            self.control.hold_roaming(false);
        }
        state.arm(Instant::now());
        match net::local_address_toward(self.remote) {
            Ok(route) => {
                state.route = route;
                if !stays_on(state.local, route) {
                    // A move that fails leaves the sockets where the route no longer goes.
                    return if self.roam(&mut state, route) {
                        NetworkChange::Roamed
                    } else {
                        NetworkChange::CannotContinue
                    };
                }
                // The route toward the host still leaves where the sockets are: nothing
                // moved, and a host that went silent starts its own attempt.
                self.shared.log(&format!(
                    "Roaming: the network changed, still on {}; not roaming",
                    route.ip()
                ));
            }
            // The new network has no route yet; the route poll moves once it does.
            Err(e) => self
                .shared
                .log(&format!("Roaming: no route toward the host yet: {e}")),
        }
        NetworkChange::CarriesOn
    }

    /// Moves the control stream and the media to `local` and starts the roam. A move that
    /// fails starts nothing: the grace period runs ([`Roamer::move_all`]).
    fn roam(&self, state: &mut State, local: SocketAddr) -> bool {
        match self.move_all(state, local) {
            Ok(()) => self.shared.log(&format!(
                "Roaming: the network changed, moved to {}",
                local.ip()
            )),
            Err(e) => {
                self.shared.warn(&format!(
                    "Roaming: could not move to {}: {e}",
                    local.ip()
                ));
                return false;
            }
        }
        self.control.network_changed(true)
    }

    /// [`super::Session::roam_to`].
    pub fn roam_to(&self, local: Option<IpAddr>) -> io::Result<()> {
        let mut state = lock(&self.state);
        let local = local.map_or(state.local, |ip| SocketAddr::new(ip, 0));
        self.move_all(&mut state, local)?;
        self.shared.log(&format!(
            "Roaming: simulated a network change to {}",
            local.ip()
        ));
        self.control.network_changed(true);
        Ok(())
    }

    /// [`super::Session::roam_control_only`].
    pub fn roam_control_only(&self, local: IpAddr) -> io::Result<()> {
        let _state = lock(&self.state);
        self.control.move_to(local)?;
        self.control.network_changed(false);
        Ok(())
    }

    /// The control stream roamed ([`crate::roaming::RoamEvent::Roamed`]). The host now takes
    /// media pings only from the control stream's address: media anywhere else move there,
    /// and if they cannot, the session fails rather than wait out a stream that never comes.
    /// Every media stream pings at once: the host dropped the pings sent before the move
    /// committed, and the next one would be up to an interval away.
    pub fn roamed(&self) {
        let mut state = lock(&self.state);
        let control = net::with_port(self.control.local_addr(), 0);
        let left = state.local.ip();
        if control.ip() == left {
            self.ping_now();
            return;
        }
        match self.move_sockets(&mut state, control, false) {
            Ok(()) => self.shared.warn(&format!(
                "Roaming: the media were left on {left} after the control stream roamed; \
                 moved them to {}",
                control.ip()
            )),
            Err(MoveError { error, .. }) => {
                self.shared.warn(&format!(
                    "Roaming: the media cannot follow the control stream to {}: {error}",
                    control.ip()
                ));
                self.shared.terminate(super::io_code(&error));
            }
        }
    }

    fn ping_now(&self) {
        for ping in self.paths.iter().filter_map(|path| path.ping.as_ref()) {
            ping.ping_now();
        }
    }

    /// Moves the control stream and the media to `local`. A move that left some sockets
    /// behind is made again at the route poll's next look, and if that fails too the session
    /// ends: no stream stays on the old address unnoticed.
    fn move_all(&self, state: &mut State, local: SocketAddr) -> io::Result<()> {
        let retrying = std::mem::take(&mut state.retry);
        let Err(MoveError { error, partly }) = self.move_sockets(state, local, true) else {
            return Ok(());
        };
        match state.move_failed(retrying, partly) {
            MoveFailed::Stays => {}
            MoveFailed::Retry => self.shared.warn(&format!(
                "Roaming: the media moved to {} but the control stream did not: {error}; \
                 moving them all again",
                local.ip()
            )),
            MoveFailed::End => {
                self.shared.warn(&format!(
                    "Roaming: could not move everything to {} again: {error}; ending the \
                     session",
                    local.ip()
                ));
                self.shared.terminate(super::io_code(&error));
            }
        }
        Err(error)
    }

    /// Binds the media sockets, and with `control` the control socket, anew on `local`, then
    /// moves them all there. A bind that fails moves nothing; the control socket moves last.
    fn move_sockets(
        &self,
        state: &mut State,
        local: SocketAddr,
        control: bool,
    ) -> Result<(), MoveError> {
        let warn = |m: &str| self.shared.warn(m);
        let control_socket = control
            .then(|| self.control.bind_move(local.ip()))
            .transpose()?;
        let mut sockets = Vec::with_capacity(self.paths.len());
        for path in &self.paths {
            let socket = net::bind_udp(
                local,
                path.buffer,
                path.qos,
                self.shared.network_priority,
                warn,
            )?;
            let receive = path.receive.as_ref().map(|_| socket.try_clone()).transpose()?;
            sockets.push((socket, receive));
        }
        let mut microphone = lock(&self.microphone);
        let mic_socket = microphone
            .as_ref()
            .map(|_| net::bind_udp(local, 0, Qos::Audio, self.shared.network_priority, warn))
            .transpose()?;

        for (path, (socket, receive)) in self.paths.iter().zip(sockets) {
            if let (Some(next), Some(receive)) = (&path.receive, receive) {
                next.set(receive);
            }
            if let Some(ping) = &path.ping {
                ping.rebind(socket);
            }
        }
        if let (Some(mic), Some(socket)) = (microphone.as_mut(), mic_socket) {
            mic.socket = socket;
        }
        drop(microphone);
        state.local = local;
        match control_socket {
            // The media moved already: the control stream is the one left behind.
            Some(socket) => self
                .control
                .move_to_socket(socket)
                .map_err(|error| MoveError {
                    error,
                    partly: true,
                }),
            None => Ok(()),
        }
    }
}

/// A socket move that failed.
struct MoveError {
    error: io::Error,
    /// Some sockets moved and some did not.
    partly: bool,
}

impl From<io::Error> for MoveError {
    fn from(error: io::Error) -> Self {
        Self {
            error,
            partly: false,
        }
    }
}

/// What a failed socket move leaves to do.
#[derive(Debug, PartialEq, Eq)]
enum MoveFailed {
    /// Nothing moved: the stream stays where it was and the grace period runs.
    Stays,
    /// Some sockets moved: the route poll moves them all again at its next look.
    Retry,
    /// The retry failed too: the session ends rather than keep streams on two addresses.
    End,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_key_keeps_its_seqs_among_the_newest_launches() {
        // Keys of their own: the registry is shared by the whole test process.
        let key = |n: u8| [0xA0, n, 0x5E, 0x9C, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let now = Instant::now();
        let mut first = roaming::Roaming::with_seq(&key(0), HINT_WINDOW, launch_seq(&key(0)));
        first.start(now, roaming::RoamTrigger::HostSilent, 0, 1);
        first.poll(now);
        assert_eq!(launch_seq(&key(0)).last(), 1);
        // Newer launches push the oldest out.
        for n in 1..=LAUNCH_SEQS as u8 {
            assert_eq!(launch_seq(&key(n)).last(), 0);
        }
        assert_eq!(launch_seq(&key(0)).last(), 0);
    }

    #[test]
    fn with_platform_hints_the_route_poll_moves_only_soon_after_a_hint() {
        let at = |n: u8| SocketAddr::from(([10, 0, 0, n], 0));
        let now = Instant::now();
        let seconds = |s: u64| now + Duration::from_secs(s);

        // Without platform hints any new route moves.
        let mut state = State::new(at(1));
        assert!(!state.route_moves(at(1), false, now));
        assert!(state.route_moves(at(2), false, now));

        // With them, not before a hint, then within HINT_WINDOW of one and no later.
        let mut state = State::new(at(1));
        assert!(!state.route_moves(at(2), true, now));
        state.arm(seconds(1));
        assert!(state.route_moves(at(3), true, seconds(5)));
        assert!(!state.route_moves(at(4), true, seconds(6)));
        state.arm(seconds(10));
        assert!(state.route_moves(at(5), true, seconds(10)));

        // A route back to where the sockets are moves nothing.
        state.arm(seconds(15));
        assert!(!state.route_moves(at(1), true, seconds(15)));

        // A metered network without the opt-in holds it even within the window.
        state.arm(seconds(20));
        state.held = true;
        assert!(!state.route_moves(at(6), true, seconds(21)));
    }

    #[test]
    fn a_move_that_left_sockets_behind_is_made_again_then_ends_the_session() {
        let at = |n: u8| SocketAddr::from(([10, 0, 0, n], 0));
        let now = Instant::now();
        let mut state = State::new(at(1));
        // Nothing moved: the stream stays, and the same route does not move.
        assert_eq!(state.move_failed(false, false), MoveFailed::Stays);
        assert!(!state.route_moves(at(1), true, now));
        // Some sockets moved: the next look moves them all again, held, unarmed and on the
        // same route.
        assert_eq!(state.move_failed(false, true), MoveFailed::Retry);
        state.held = true;
        assert!(state.route_moves(at(1), true, now));
        // That retry failing too, partly or not, ends the session.
        let retrying = std::mem::take(&mut state.retry);
        assert_eq!(state.move_failed(retrying, false), MoveFailed::End);
        let mut state = State::new(at(1));
        state.move_failed(false, true);
        let retrying = std::mem::take(&mut state.retry);
        assert_eq!(state.move_failed(retrying, true), MoveFailed::End);
    }

    #[test]
    fn a_new_port_or_a_rotated_privacy_address_is_not_a_network_change() {
        let now = Instant::now();
        let v4 = |n: u8, port: u16| SocketAddr::from(([10, 0, 0, n], port));
        let mut state = State::new(v4(1, 0));
        assert!(!state.route_moves(v4(1, 40000), false, now));
        assert!(state.route_moves(v4(2, 0), false, now));

        let Ok(probe) = std::net::UdpSocket::bind("[::1]:0") else {
            eprintln!("skipped: no IPv6 loopback");
            return;
        };
        let v6 = |ip: &str| SocketAddr::new(ip.parse().unwrap(), 0);
        // ::1 is assigned: another address of its /64 is a rotation; another /64 moves.
        let local = net::with_port(probe.local_addr().unwrap(), 0);
        assert!(stays_on(local, v6("::2")));
        assert!(!stays_on(local, v6("2001:db8::1")));
        // An address the interface no longer has moves even within its /64.
        assert!(!stays_on(v6("2001:db8::1"), v6("2001:db8::2")));
        let mut state = State::new(local);
        assert!(!state.route_moves(v6("::2"), false, now));
        assert!(state.route_moves(v6("2001:db8::1"), false, now));
    }

    // A Wi-Fi blip that the stream does not roam across: a network back on the same address
    // (DHCP gave it again) or with no route yet keeps the connection; only another address
    // strands it.
    #[test]
    fn without_a_roam_the_connection_carries_on_unless_the_address_changed() {
        let at = |n: u8, port: u16| SocketAddr::from(([192, 168, 1, n], port));
        let standing = NetworkChange::standing;
        assert_eq!(standing(at(20, 0), Ok(at(20, 51000))), NetworkChange::CarriesOn);
        let no_route = io::Error::from(io::ErrorKind::NetworkUnreachable);
        assert_eq!(standing(at(20, 0), Err(no_route)), NetworkChange::CarriesOn);
        assert_eq!(standing(at(20, 0), Ok(at(21, 0))), NetworkChange::CannotContinue);
    }
}
