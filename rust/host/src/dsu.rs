//! The DSU (cemuhook) motion server (`enable_dsu_server`, off by default):
//! emulators on the PC, or on the network, read each streamed controller's
//! gyroscope, accelerometer, buttons and touch from UDP `dsu_server_port`
//! (26760). The packets and the client registrations are
//! `butterpollo_core::dsu`'s; the setting names and port are Foundation
//! Sunshine's (AlkaidLab/foundation-sunshine, GPL-3.0), and like its server
//! this one listens on every interface.
//!
//! The control thread only updates a slot and wakes the sender; nothing is
//! sent from it, and with the setting off [`observe`] returns at once.
use butterpollo_core::{
    dsu::{self, Clients, Request, SLOTS, Slots},
    input::Input,
    session::Warnings,
};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Console warning code when the port cannot be opened.
pub const WARNING: &str = "dsu_server_port";
/// How long the receiver waits for a packet before it drops stale clients.
const RECEIVE_TIMEOUT: Duration = Duration::from_millis(500);

static SERVER: OnceLock<Arc<Server>> = OnceLock::new();

struct Server {
    socket: UdpSocket,
    /// Random, as every DSU server's: a client sees a restarted server as new.
    id: u32,
    started: Instant,
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    slots: Slots,
    clients: Clients,
    /// Slots whose pad data changed since the sender last ran.
    dirty: [bool; SLOTS],
}

/// Starts the server when `port` is set (`Config::dsu_server_port`). A port
/// that cannot be opened is a console warning; the stream works without it.
pub fn start(port: Option<u16>, warnings: &Warnings) {
    let Some(port) = port else { return };
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)) {
        Ok(socket) => socket,
        Err(error) => {
            tracing::warn!(port, %error, "DSU motion server unavailable");
            warnings.set(
                WARNING,
                format!(
                    "Emulators can't read controller motion: UDP port {port} could not be opened ({error}). Choose another DSU server port or close the program using it, then restart the host."
                ),
            );
            return;
        }
    };
    let receiver = socket.try_clone().and_then(|receiver| {
        receiver.set_read_timeout(Some(RECEIVE_TIMEOUT))?;
        Ok(receiver)
    });
    let receiver = match receiver {
        Ok(receiver) => receiver,
        Err(error) => {
            tracing::warn!(port, %error, "DSU motion server unavailable");
            warnings.set(WARNING, format!("Emulators can't read controller motion: the DSU server could not start ({error})."));
            return;
        }
    };
    warnings.clear(WARNING);
    let server = Arc::new(Server {
        socket,
        id: rand::random(),
        started: Instant::now(),
        state: Mutex::new(State::default()),
        wake: Condvar::new(),
    });
    if SERVER.set(server.clone()).is_err() {
        return;
    }
    tracing::info!(port, "DSU motion server listening on every interface");
    let receiving = server.clone();
    let spawned = std::thread::Builder::new()
        .name("dsu-receive".into())
        .spawn(move || receiving.receive(&receiver))
        .and_then(|_| {
            std::thread::Builder::new()
                .name("dsu-send".into())
                .spawn(move || server.send())
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "DSU motion server threads could not start");
        warnings.set(
            WARNING,
            format!(
                "Emulators can't read controller motion: the DSU server could not start ({error})."
            ),
        );
    }
}

/// A streamed controller's input from `session`: updates its slot.
pub fn observe(session: &str, input: &Input) {
    let Some(server) = SERVER.get() else { return };
    if !matches!(
        input,
        Input::Controller { .. }
            | Input::Arrival { .. }
            | Input::ControllerTouch { .. }
            | Input::Motion { .. }
            | Input::Battery { .. }
    ) {
        return;
    }
    let now = server.started.elapsed().as_micros() as u64;
    let mut state = server.state.lock().unwrap();
    if let Some(slot) = state.slots.apply(session, input, now) {
        state.dirty[usize::from(slot)] = true;
        server.wake.notify_one();
    }
}

/// A session ended: its controllers leave their slots.
pub fn forget(session: &str) {
    let Some(server) = SERVER.get() else { return };
    let mut state = server.state.lock().unwrap();
    for slot in state.slots.forget(session) {
        state.dirty[usize::from(slot)] = true;
    }
    server.wake.notify_one();
}

impl Server {
    /// Answers requests and records subscriptions.
    fn receive(&self, socket: &UdpSocket) {
        let mut buffer = [0u8; 1024];
        loop {
            let (len, from) = match socket.recv_from(&mut buffer) {
                Ok(received) => received,
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) =>
                {
                    self.state.lock().unwrap().clients.prune(Instant::now());
                    continue;
                }
                // Windows reports a client that went away as a reset on the
                // next receive (ICMP port unreachable); the socket still works.
                Err(error) if error.kind() == ErrorKind::ConnectionReset => continue,
                Err(error) => {
                    tracing::warn!(%error, "DSU motion server stopped receiving");
                    return;
                }
            };
            let Some((_, request)) = dsu::parse(&buffer[..len]) else {
                continue;
            };
            match request {
                Request::Version => self.reply(&dsu::version_reply(self.id), from),
                Request::Ports { count, slots } => {
                    let replies: Vec<_> = {
                        let state = self.state.lock().unwrap();
                        slots[..usize::from(count)]
                            .iter()
                            .map(|&slot| dsu::port_info(self.id, slot, state.slots.pad(slot)))
                            .collect()
                    };
                    for reply in replies {
                        self.reply(&reply, from);
                    }
                }
                Request::PadData { flags, slot, mac } => {
                    let mut state = self.state.lock().unwrap();
                    state
                        .clients
                        .register(from, flags, slot, mac, Instant::now());
                    // A new subscriber gets the current state now, not at the
                    // controller's next change.
                    for slot in 0..SLOTS {
                        state.dirty[slot] |= state.slots.pad(slot as u8).is_some();
                    }
                    self.wake.notify_one();
                }
            }
        }
    }

    /// Sends the slots that changed to the clients that want them.
    fn send(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            while !state.dirty.contains(&true) {
                state = self.wake.wait(state).unwrap();
            }
            let now = Instant::now();
            let mut packets = Vec::new();
            for slot in 0..SLOTS as u8 {
                if !std::mem::take(&mut state.dirty[usize::from(slot)]) {
                    continue;
                }
                let Some(pad) = state.slots.pad(slot).cloned() else {
                    continue;
                };
                let subscribers = state.clients.subscribers(slot, now);
                if subscribers.is_empty() {
                    continue;
                }
                let number = state.slots.next_number(slot);
                packets.push((dsu::pad_data(self.id, slot, &pad, number), subscribers));
            }
            drop(state);
            for (packet, subscribers) in &packets {
                for to in subscribers {
                    self.reply(packet, *to);
                }
            }
            state = self.state.lock().unwrap();
        }
    }

    fn reply(&self, packet: &[u8], to: SocketAddr) {
        if let Err(error) = self.socket.send_to(packet, to) {
            tracing::debug!(%error, %to, "DSU packet not sent");
        }
    }
}
