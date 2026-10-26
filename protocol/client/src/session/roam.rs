//! Client roaming's sockets: the route poll, the media rebind and the front end's network
//! hints, around the control stream's roaming attempt ([`ControlHandle::network_changed`]).
//! See `docs/protocol/client-roaming.md`.
//!
//! The control socket is bound to the unspecified address, so it follows the route by
//! itself; the video, audio and microphone sockets are bound to the local address toward
//! the host and move to new ports on the new one. The decoders, renderers and the session
//! object stay. The host resets its link rate and pacing and asks for an IDR when the roam
//! commits, and the control stream pings ENet on Roamed, so nothing else is reset here. A
//! roam that fails leaves the stream to the network grace period and the app's recovery.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::net::{self, NextSocket, PingHandle, StopSignal};
use super::{Microphone, Shared};
use crate::control::driver::ControlHandle;
use crate::priority::Qos;

/// How often the route toward the host is looked up.
const ROUTE_POLL: Duration = Duration::from_secs(1);
/// With [`RoamingConfig::platform_hints`], how long after a hint the route poll may move.
const HINT_WINDOW: Duration = Duration::from_secs(5);

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
            state: Mutex::new(State {
                local,
                route: local,
                held: false,
                armed_until: None,
            }),
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
        if route == state.route {
            return;
        }
        state.route = route;
        if state.held
            || (self.config.platform_hints && !state.armed_until.is_some_and(|until| now < until))
        {
            return;
        }
        self.roam(&mut state, route);
    }

    /// [`super::Session::network_changed`].
    pub fn network_changed(&self, hint: NetworkHint) -> bool {
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
            return false;
        }
        if state.held {
            state.held = false;
            self.control.hold_roaming(false);
        }
        state.armed_until = Some(Instant::now() + HINT_WINDOW);
        match net::local_address_toward(self.remote) {
            Ok(route) => {
                state.route = route;
                if route != state.local {
                    return self.roam(&mut state, route);
                }
            }
            // The new network has no route yet; the route poll moves once it does.
            Err(e) => self
                .shared
                .log(&format!("Roaming: no route toward the host yet: {e}")),
        }
        self.control.network_changed(false)
    }

    /// Moves the media to `local` (warning on failure: the grace period still runs) and
    /// starts the roam.
    fn roam(&self, state: &mut State, local: SocketAddr) -> bool {
        match self.move_media(state, local) {
            Ok(()) => self.shared.log(&format!(
                "Roaming: the network changed, media moved to {}",
                local.ip()
            )),
            Err(e) => self.shared.warn(&format!(
                "Roaming: could not move the media to {}: {e}",
                local.ip()
            )),
        }
        self.control.network_changed(true)
    }

    /// [`super::Session::roam_to`].
    pub fn roam_to(&self, local: Option<IpAddr>) -> io::Result<()> {
        let mut state = lock(&self.state);
        let local = match local {
            Some(ip) => {
                self.control.move_to(ip)?;
                SocketAddr::new(ip, 0)
            }
            None => state.local,
        };
        self.move_media(&mut state, local)?;
        self.shared.log(&format!(
            "Roaming: simulated a network change to {}",
            local.ip()
        ));
        self.control.network_changed(true);
        Ok(())
    }

    /// Binds every media socket anew on `local` and moves the streams there.
    fn move_media(&self, state: &mut State, local: SocketAddr) -> io::Result<()> {
        let warn = |m: &str| self.shared.warn(m);
        for path in &self.paths {
            let socket = net::bind_udp(
                local,
                path.buffer,
                path.qos,
                self.shared.network_priority,
                warn,
            )?;
            if let Some(next) = &path.receive {
                next.set(socket.try_clone()?);
            }
            if let Some(ping) = &path.ping {
                ping.rebind(&socket)?;
            }
        }
        if let Some(mic) = lock(&self.microphone).as_mut() {
            mic.socket = net::bind_udp(local, 0, Qos::Audio, self.shared.network_priority, warn)?;
        }
        state.local = local;
        Ok(())
    }
}
