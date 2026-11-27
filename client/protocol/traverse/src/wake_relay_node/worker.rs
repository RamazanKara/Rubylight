//! The relay's DHT work: publishing its record for each listed device, and looking for the
//! records of devices that want a wake. Both run on threads of their own, so a slow DHT never
//! holds the relay's loop.

use super::*;

impl WakeRelayPeer {
    pub(super) fn serve_devices(&self) {
        let (public, nat) = {
            let status = self.shared.status.lock().unwrap();
            (status.public.clone(), status.nat)
        };
        let now = self.shared.node.now();
        let window = record::window(unix_now());
        let connected: Vec<[u8; 8]> = self
            .shared
            .paths
            .lock()
            .unwrap()
            .active
            .iter()
            .filter(|(_, path)| matches!(path.puncher.state(), punch::State::Connected(_)))
            .map(|(tag, _)| *tag)
            .collect();
        let mut devices = self.shared.devices.lock().unwrap();
        let publish: Vec<([u8; 8], Keys)> = devices
            .iter()
            .filter(|state| {
                !public.is_empty()
                    && state.publish_retry_at.is_none_or(|at| now >= at)
                    && state
                        .published
                        .as_ref()
                        .is_none_or(|(at_window, at, endpoints)| {
                            *at_window != window
                                || now.saturating_sub(*at) > PUBLISH_EVERY
                                || *endpoints != public
                        })
            })
            .map(|state| (state.tag, state.keys.clone()))
            .collect();
        let busy = devices.iter().filter(|state| state.polling).count();
        let mut due: Vec<usize> = (0..devices.len())
            .filter(|&index| {
                let state = &devices[index];
                !state.polling
                    && !connected.contains(&state.tag)
                    && state
                        .last_poll
                        .is_none_or(|at| now.saturating_sub(at) >= POLL_EVERY)
            })
            .collect();
        due.sort_by_key(|&index| devices[index].last_poll);
        let mut poll = Vec::new();
        for index in due.into_iter().take(MAX_POLLS.saturating_sub(busy)) {
            devices[index].last_poll = Some(now);
            devices[index].polling = true;
            poll.push((devices[index].tag, devices[index].keys.clone()));
        }
        let published = devices
            .iter()
            .filter(|state| state.published.is_some())
            .count();
        drop(devices);
        self.shared.status.lock().unwrap().published = published;
        // Without a bootstrap there is no DHT: a relay on a test's loopback answers punches only.
        if self.shared.config.bootstrap.is_empty() {
            self.done_polling(&poll);
            return;
        }
        for (tag, keys) in poll {
            let polling = self.clone();
            let spawned = thread::Builder::new()
                .name("wake relay poll".into())
                .spawn(move || polling.poll_device(tag, &keys, unix_now()));
            if spawned.is_err() {
                self.done_polling(&[(tag, Keys::new([0; 32]))]);
            }
        }
        if !publish.is_empty() && !self.shared.publishing.swap(true, Ordering::AcqRel) {
            let publishing = self.clone();
            let spawned = thread::Builder::new()
                .name("wake relay publish".into())
                .spawn(move || {
                    publishing.publish(publish, public, nat);
                    publishing.shared.publishing.store(false, Ordering::Release);
                });
            if spawned.is_err() {
                self.shared.publishing.store(false, Ordering::Release);
            }
        }
    }

    fn done_polling(&self, polled: &[([u8; 8], Keys)]) {
        let mut devices = self.shared.devices.lock().unwrap();
        for state in devices.iter_mut() {
            if polled.iter().any(|(tag, _)| *tag == state.tag) {
                state.polling = false;
            }
        }
    }

    fn publish(&self, due: Vec<([u8; 8], Keys)>, public: Vec<SocketAddr>, nat: Nat) {
        let mut failed = false;
        for (tag, keys) in due {
            if self.stopped() {
                return;
            }
            let unix = unix_now();
            let window = record::window(unix);
            let record = Record {
                role: Role::WakeRelay,
                time: unix,
                session: self.shared.session,
                peer_session: [0; 16],
                nat,
                endpoints: public.clone(),
                ports: None,
            };
            let confirmed =
                self.shared
                    .node
                    .put(&record.item(&keys, window, crate::crypto::random()));
            let now = self.shared.node.now();
            let mut devices = self.shared.devices.lock().unwrap();
            if let Some(state) = devices.iter_mut().find(|state| state.tag == tag) {
                if confirmed > 0 {
                    state.published = Some((window, now, public.clone()));
                    state.publish_retry_at = None;
                } else {
                    state.publish_retry_at = Some(now + PUBLISH_RETRY);
                    failed = true;
                }
            }
        }
        self.shared.status.lock().unwrap().dht_error = failed.then(|| {
            "no DHT node stored the relay's record: UDP to the internet may be blocked".to_owned()
        });
    }

    /// Looks for a device's request record; a fresh one naming this relay's session starts
    /// punching toward the device.
    fn poll_device(&self, tag: [u8; 8], keys: &Keys, unix: u64) {
        let window = record::window(unix);
        let get = |window: u64| {
            let (item, _) = self.shared.node.get(
                &keys.public(Role::WakeRequester),
                &keys.salt(Role::WakeRequester, window),
            );
            item.and_then(|item| Record::from_item(keys, Role::WakeRequester, window, &item))
        };
        let mut found = get(window);
        // A record of the previous window can be fresh only in this window's first minutes.
        if found.is_none() && unix % record::WINDOW_SECONDS <= record::CLIENT_FRESH_SECONDS {
            found = get(window.saturating_sub(1));
        }
        self.done_polling(&[(tag, keys.clone())]);
        let Some(record) = found else { return };
        if unix.abs_diff(record.time) > record::CLIENT_FRESH_SECONDS
            || record.peer_session != self.shared.session
        {
            return;
        }
        self.apply_polled_record(tag, &record);
    }

    pub(super) fn apply_polled_record(&self, tag: [u8; 8], record: &Record) {
        let now = self.shared.node.now();
        let candidates = punch::candidates(&record.endpoints, record.nat);
        let mut paths = self.shared.paths.lock().unwrap();
        if paths.is_ended(tag, record.session, now) {
            return;
        }
        let mut devices = self.shared.devices.lock().unwrap();
        let Some(state) = devices.iter_mut().find(|state| state.tag == tag) else {
            return;
        };
        if state.answered == Some(record.session) {
            return;
        }
        match paths.active.get_mut(&tag) {
            Some(path) if path.session == record.session => {
                state.answered = Some(record.session);
                path.puncher.add(&candidates);
            }
            Some(path)
                if matches!(path.puncher.state(), punch::State::Connected(_))
                    || record.time < path.time => {}
            _ => {
                state.answered = Some(record.session);
                let keys = state.keys.path(&self.shared.session, &record.session);
                let first = self.first_seq();
                let mut path = Path::new(state, keys, record.session, record.time, first, now);
                path.puncher = Puncher::new(candidates, now);
                paths.replace(tag, path, now);
            }
        }
    }
}
