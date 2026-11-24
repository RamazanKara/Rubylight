//! Background rendezvous worker loop: STUN discovery, record publishing, and device polling.

use super::*;
use crate::stun;

impl HostService {
    /// Drops the paths that closed. The map is write-locked only when one did, which is rare:
    /// a write lock every second made the receive thread's next datagram wait for it (and for
    /// every datagram in flight before it). The dropped paths go after the lock does.
    pub(super) fn prune_closed(&self) {
        let any = self
            .shared
            .tunnels
            .read()
            .unwrap()
            .values()
            .any(|(_, tunnel)| tunnel.is_closed());
        if !any {
            return;
        }
        let closed: Vec<_> = {
            let mut tunnels = self.shared.tunnels.write().unwrap();
            let tags: Vec<[u8; 8]> = tunnels
                .iter()
                .filter(|(_, (_, tunnel))| tunnel.is_closed())
                .map(|(tag, _)| *tag)
                .collect();
            tags.iter().filter_map(|tag| tunnels.remove(tag)).collect()
        };
        drop(closed);
    }

    pub(super) fn run(&self) {
        let mut next_stun: Millis = 0;
        while !self.stopped() {
            let now = self.shared.node.now();
            if now >= next_stun {
                next_stun = now + STUN_EVERY;
                self.discover();
            }
            self.serve_devices();
            self.prune_closed();
            let now = self.shared.node.now();
            self.shared
                .path_errors
                .lock()
                .unwrap()
                .retain(|_, (_, _, at)| now.saturating_sub(*at) < PATH_ERROR_FOR);
            for _ in 0..10 {
                if self.stopped() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn discover(&self) {
        let answers = self.shared.node.stun(&self.shared.config.stun_servers);
        let local = self.shared.node.local_addr().ok();
        let mut public: Vec<SocketAddr> = Vec::new();
        for answer in &answers {
            if !public.contains(answer) {
                public.push(*answer);
            }
        }
        let nat = local.map_or(Nat::Unknown, |local| {
            Nat::from_mapping(stun::mapping(local, &answers))
        });
        let changed = {
            let mut status = self.shared.status.lock().unwrap();
            status.stun_error = answers.is_empty().then(|| {
                "no STUN server answered: the PC may have no internet connection, or a firewall blocks UDP".to_owned()
            });
            let changed = status.public != public;
            status.public = public;
            status.nat = nat;
            changed
        };
        // A changed address is republished right away.
        if changed {
            for state in self.shared.devices.lock().unwrap().iter_mut() {
                state.published = None;
            }
        }
    }

    /// Starts the publications and the looks for devices' records that are due; both run on
    /// threads of their own, so a slow DHT never holds this loop or another device back.
    pub(super) fn serve_devices(&self) {
        let (public, nat) = {
            let status = self.shared.status.lock().unwrap();
            (status.public.clone(), status.nat)
        };
        let window = record::window(unix_now());
        let (open, healthy): (Vec<[u8; 8]>, Vec<[u8; 8]>) = {
            let tunnels = self.shared.tunnels.read().unwrap();
            let open = tunnels
                .iter()
                .filter(|(_, (_, tunnel))| !tunnel.is_closed())
                .map(|(tag, _)| *tag)
                .collect();
            let healthy = tunnels
                .iter()
                .filter(|(_, (_, tunnel))| tunnel.healthy())
                .map(|(tag, _)| *tag)
                .collect();
            (open, healthy)
        };
        let now = self.shared.node.now();
        let mut devices = self.shared.devices.lock().unwrap();
        let publish: Vec<(String, Keys)> = devices
            .iter()
            .filter(|state| {
                !public.is_empty()
                    && match &state.published {
                        None => true,
                        Some((at_window, at, endpoints)) => {
                            // Saturating: a publication that ended after `now` was taken.
                            *at_window != window
                                || now.saturating_sub(*at) > PUBLISH_EVERY
                                || *endpoints != public
                        }
                    }
            })
            .map(|state| (state.device.id.clone(), state.keys.clone()))
            .collect();
        // The longest waiting first, while threads are free.
        let mut due: Vec<usize> = (0..devices.len())
            .filter(|&index| {
                let state = &devices[index];
                !state.polling
                    && poll_interval(state, &open, &healthy, now).is_some_and(|every| {
                        state
                            .last_poll
                            .is_none_or(|at| now.saturating_sub(at) >= every)
                    })
            })
            .collect();
        due.sort_by_key(|&index| devices[index].last_poll);
        let free = MAX_POLLS.saturating_sub(devices.iter().filter(|state| state.polling).count());
        let mut poll = Vec::new();
        for index in due.into_iter().take(free) {
            // The interval counts from the poll's start, so a slow lookup does not stretch it.
            devices[index].last_poll = Some(now);
            devices[index].polling = true;
            poll.push((
                devices[index].device.id.clone(),
                devices[index].keys.clone(),
            ));
        }
        let published = devices
            .iter()
            .filter(|state| state.published.is_some())
            .count();
        drop(devices);
        self.shared.status.lock().unwrap().published = published;

        for (id, keys) in poll {
            let polling = self.clone();
            let device = id.clone();
            let spawned = thread::Builder::new()
                .name("internet play poll".into())
                .spawn(move || polling.poll_device(&device, &keys, unix_now()));
            if spawned.is_err() {
                let mut devices = self.shared.devices.lock().unwrap();
                if let Some(state) = devices.iter_mut().find(|state| state.device.id == id) {
                    state.polling = false;
                }
            }
        }
        if !publish.is_empty() && !self.shared.publishing.swap(true, Ordering::AcqRel) {
            let publishing = self.clone();
            let spawned = thread::Builder::new()
                .name("internet play publish".into())
                .spawn(move || {
                    publishing.publish(publish, public, nat);
                    publishing.shared.publishing.store(false, Ordering::Release);
                });
            if spawned.is_err() {
                self.shared.publishing.store(false, Ordering::Release);
            }
        }
    }

    /// Publishes the host's record for each device in `due`, one after another.
    fn publish(&self, due: Vec<(String, Keys)>, public: Vec<SocketAddr>, nat: Nat) {
        for (id, keys) in due {
            if self.stopped() {
                return;
            }
            let unix = unix_now();
            let window = record::window(unix);
            let now = self.shared.node.now();
            let record = Record {
                role: Role::Host,
                time: unix,
                session: self.shared.session,
                peer_session: [0; 16],
                nat,
                endpoints: public.clone(),
                ports: Some(self.shared.config.ports),
            };
            let confirmed =
                self.shared
                    .node
                    .put(&record.item(&keys, window, crate::crypto::random()));
            let mut devices = self.shared.devices.lock().unwrap();
            if let Some(state) = devices.iter_mut().find(|state| state.device.id == id)
                && confirmed > 0
            {
                state.published = Some((window, now, public.clone()));
            }
            drop(devices);
            self.shared.status.lock().unwrap().dht_error = (confirmed == 0).then(|| {
                "no DHT node stored the PC's record: UDP to the internet may be blocked".to_owned()
            });
        }
    }

    fn poll_device(&self, id: &str, keys: &Keys, unix: u64) {
        let window = record::window(unix);
        let get = |window: u64| {
            let (item, _) = self
                .shared
                .node
                .get(&keys.public(Role::Client), &keys.salt(Role::Client, window));
            item.and_then(|item| Record::from_item(keys, Role::Client, window, &item))
        };
        // A record of the previous window is older than the seconds this window has run, so it
        // can be fresh only in this window's first CLIENT_FRESH_SECONDS; then both windows are
        // asked at once, so the look takes one lookup's time.
        let fresh_previous = unix % record::WINDOW_SECONDS <= record::CLIENT_FRESH_SECONDS;
        let previous = window.saturating_sub(1);
        let found = if fresh_previous {
            thread::scope(|scope| {
                let earlier = thread::Builder::new()
                    .name("internet play poll".into())
                    .spawn_scoped(scope, || get(previous));
                let current = get(window);
                match earlier {
                    Ok(earlier) => current.or_else(|| earlier.join().ok().flatten()),
                    Err(_) => current.or_else(|| get(previous)),
                }
            })
        } else {
            get(window)
        };
        let now = self.shared.node.now();
        let mut devices = self.shared.devices.lock().unwrap();
        let Some(state) = devices.iter_mut().find(|state| state.device.id == id) else {
            return;
        };
        state.polling = false;
        let Some(record) = found else { return };
        let fresh = unix.abs_diff(record.time) <= record::CLIENT_FRESH_SECONDS;
        if !fresh || record.peer_session != self.shared.session {
            return;
        }
        state.heard = Some(now);
        if state.answered == Some(record.session) {
            return;
        }
        state.answered = Some(record.session);
        let peer = Peer::of(state);
        drop(devices);
        // A punch may be starting this very path: one start per device session, so no two
        // tunnels ever seal under the same keys.
        let claim = (peer.tag, record.session);
        if !self.shared.starting.lock().unwrap().insert(claim) {
            return;
        }
        let candidates = punch::candidates(&record.endpoints, record.nat);
        self.open(&peer, record.session, record.time, candidates);
        self.shared.starting.lock().unwrap().remove(&claim);
    }
}
