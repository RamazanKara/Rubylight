//! Main ENet control worker event loop, peer connect/disconnect lifecycle, and top-level
//! control packet dispatch.

use super::super::*;
use super::audio::handle_controller_audio;
use super::clipboard::{handle_clipboard_and_focus_packet, send_clipboard_and_focus_updates};
use super::extensions::{handle_extension_packet, send_extension_updates};
use super::haptics::{haptics_negotiated, send_audio_haptics, send_pad_feedback};
use super::input::{apply_input, ensure_input_ready, follow_session, handle_input_packet};
use super::peer::{ControlPeer, ReceiveScratch};
use super::qos::{handle_qos_packet, send_clock_syncs, tick_and_send_qos};
use super::socket::{
    CONTROL_EVENT_LIMIT, ControlSocket, NOTICE_ACK_WAIT, UnattendedSweep, control_wait_timeout,
    end_unattended_sessions, expire_controllers, follow_roam, tell_client_stopping,
};

impl Media {
    pub(in crate::stream) fn control(&self, h: Shared) -> Result<()> {
        let socket = Arc::new(ReplySocket::new(crate::network::udp(
            (self.bind, self.control_port).into(),
        )?)?);
        butterpollo_win_io::net::configure_udp(&socket)?;
        // Input from every client arrives here; the Windows default buffer is small.
        if let Err(error) = socket2::SockRef::from(&*socket).set_recv_buffer_size(1 << 20) {
            tracing::warn!(%error, "control socket receive buffer unchanged");
        }
        let raw_socket = std::os::windows::io::AsRawSocket::as_raw_socket(&*socket);
        let raw_wake =
            std::os::windows::io::AsRawSocket::as_raw_socket(&self.control_wake_receiver);
        let mut control_socket = ControlSocket::new(socket.clone());
        control_socket.roaming = roaming::Roaming::remembering(self.roam_seqs.clone());
        let mut host = Host::new(
            control_socket,
            HostSettings {
                peer_limit: 32,
                // Moonlight opens 48 channels (gamepads from 0x10, motion
                // from 0x20) and folds those above the host's limit onto
                // channel 0, where one lost ping holds up all controller
                // input. The previous host allowed the protocol maximum.
                channel_limit: 255,
                ..Default::default()
            },
        )?;
        let _com = ComGuard::new()?;
        let mut priority = None;
        let (commands, pending_commands) =
            std::sync::mpsc::sync_channel::<(Arc<Session>, crate::process::MenuRun)>(32);
        thread::Builder::new()
            .name("server commands".into())
            .spawn(move || {
                for (session, run) in pending_commands {
                    if session.stopping() {
                        continue;
                    }
                    if let Err(error) = run.start() {
                        tracing::warn!(%error, "configured server command failed");
                    }
                }
            })?;
        let mut peers: HashMap<PeerID, ControlPeer> = HashMap::new();
        // Active sessions without a control peer, since when.
        let mut unattended: HashMap<String, Instant> = HashMap::new();
        // Keep the snapshot's allocation across high-rate input wakes. Sessions
        // are keyed by their immutable launch ID, so the IDs need no copies.
        let mut active_sessions: Vec<Arc<Session>> = Vec::new();
        // The couch menu workers, one per session.
        let mut menus = crate::couch_menu::Workers::default();
        let input_timer = butterpollo_win_graphics::timing::Timer::new()?;
        let mut feedback_at = Instant::now();
        let mut controller_cleanup = Instant::now();
        let mut unattended_sweep = UnattendedSweep::new(Instant::now());
        let mut scratch: ReceiveScratch = Default::default();
        while !h.stop.load(Ordering::Acquire) {
            while self.control_wake_receiver.recv(&mut [0]).is_ok() {}
            // The host stops before `h.stop` is set (Ctrl+C): every client hears it, as it
            // would if the maintenance loop had set the flag.
            if self.shutdown_requested.load(Ordering::Acquire) {
                self.tell_clients_of_shutdown(&h, &mut host, &mut peers);
                // The clients told above have only `disconnect_later` queued, and a peer
                // whose session had already stopped was told nothing: end every peer now,
                // as the normal exit does, and send what telling them queued.
                for (peer, p) in std::mem::take(&mut peers) {
                    host.peer_mut(peer).disconnect_now(0);
                    drop(p);
                }
                host.flush();
                return Ok(());
            }
            expire_controllers(&h, &mut controller_cleanup);
            let (ping_timeout, client_roaming) = {
                let c = h.config.read().unwrap();
                (crate::network::ping_timeout(&c), c.client_roaming())
            };
            host.socket_mut().roaming.set_enabled(client_roaming);
            // Whether a peer went, so roaming forgets its addresses.
            let mut peers_went = false;
            // Whether a peer came, so the unattended sweep runs at once.
            let mut peers_came = false;
            #[cfg(any(test, feature = "test-support"))]
            if let Some(fixture) = &h.reconnect_fixture {
                fixture.hold_control();
                fixture.reset_peer(&mut host, &peers);
            }
            let mut disconnected = Vec::new();
            // Acknowledgements and anything else ENet sends wait until this
            // pass's input is applied.
            host.socket_mut().hold();
            // Bound each pass so a busy input peer cannot starve cleanup or feedback.
            let mut serviced = 0;
            for _ in 0..CONTROL_EVENT_LIMIT {
                let Some(event) = host.service()? else { break };
                serviced += 1;
                match event {
                    Event::Connect { peer, data } => {
                        if handle_connect(
                            &h,
                            &socket,
                            ping_timeout,
                            &self.control_seqs,
                            &mut peers,
                            peer,
                            data,
                        ) {
                            peers_came = true;
                        }
                    }
                    Event::Disconnect { peer, .. } => {
                        peers_went = true;
                        if let Some(p) =
                            handle_disconnect(&h, &socket, ping_timeout, &mut peers, peer)
                        {
                            disconnected.push(p);
                        }
                    }
                    Event::Receive { peer, packet, .. } => {
                        handle_receive(
                            &h,
                            &self.control_wake,
                            &commands,
                            &mut peers,
                            peer,
                            packet.data(),
                            &mut scratch,
                        );
                    }
                }
            }
            // Any guest that lost keyboard and mouse lets go before the new holder's input lands.
            for p in peers.values_mut() {
                let Some(s) = &p.session else { continue };
                if let Some(i) = &mut p.injector
                    && h.device_guests.take_release(s)
                {
                    i.release_keyboard_mouse();
                }
            }
            // Input first, for every peer: none waits for another peer's housekeeping
            // below (feedback, notices, cursor shapes, USB answers).
            let received = host.socket().first_received;
            for p in peers.values_mut() {
                let Some(s) = p.session.clone() else { continue };
                let ready = match s.launch.nucleus {
                    Some(_) => p.coop.is_some(),
                    None => p.injector.is_some(),
                };
                if p.inputs.is_empty() || !ready || s.stopping() {
                    continue;
                }
                follow_session(&h, p, &s);
                apply_input(&h, p, &s, &mut menus, received);
            }
            for commit in host.socket_mut().roaming.take_commits() {
                follow_roam(&h, self, &mut host, &peers, &socket, commit);
            }
            let poll_feedback = Instant::now() >= feedback_at;
            if poll_feedback {
                feedback_at = Instant::now() + Duration::from_millis(8);
            }
            let mut remove = vec![];
            for (peer_id, p) in &mut peers {
                if !poll_peer(
                    &h,
                    &self.control_wake,
                    &commands,
                    &mut host,
                    *peer_id,
                    p,
                    ping_timeout,
                    poll_feedback,
                    received,
                    &mut menus,
                    &mut scratch,
                ) {
                    remove.push(*peer_id);
                }
            }
            host.socket_mut().release()?;
            if !scratch.0.is_empty() {
                send_clock_syncs(&mut host, &mut peers, &mut scratch.0);
                host.flush();
            }
            // Release held input now; gamepad workers unplug their pads asynchronously.
            drop(disconnected);
            peers_went |= !remove.is_empty();
            for peer in remove {
                if let Some(p) = peers.remove(&peer) {
                    crate::dsu::forget(&p.id);
                }
            }
            // Haptics from game audio, for the waveform controllers the game
            // leaves without haptics or rumble: after the input was applied
            // and the acks went out, so the analysis delays neither.
            send_audio_haptics(&mut host, &mut peers);
            if peers_went && host.socket().roaming.has_peers() {
                let live: Vec<SocketAddr> = host
                    .peers()
                    .filter(|peer| peer.state() != PeerState::Disconnected)
                    .filter_map(|peer| peer.address())
                    .collect();
                host.socket_mut()
                    .roaming
                    .retain_peers(&socket, |peer| live.contains(&peer));
            }
            let sweep_due = unattended_sweep.due(Instant::now(), peers_came || peers_went);
            if (peers_went || sweep_due)
                && (!self.roam_seqs.is_empty() || !self.control_seqs.is_empty())
            {
                // An ended launch never returns; sweep both on peer exit and periodically after
                // video teardown finishes so stopping launches do not stay pinned.
                let sessions = h.sessions.lock().unwrap();
                let roam_keys: Vec<[u8; 16]> = sessions
                    .active
                    .values()
                    .map(|s| s.launch.key)
                    .chain(sessions.pending.values().map(|l| l.key))
                    .collect();
                self.roam_seqs.retain_launches(roam_keys.iter());
                let control_live: Vec<(String, [u8; 16])> = sessions
                    .active
                    .values()
                    .filter(|s| !s.stopping())
                    .map(|s| (s.launch.id.clone(), s.launch.key))
                    .chain(sessions.pending.values().map(|l| (l.id.clone(), l.key)))
                    .collect();
                self.control_seqs
                    .retain_launches(control_live.iter().map(|(id, key)| (id.as_str(), key)));
            }
            menus.retain(|id| {
                peers
                    .values()
                    .any(|p| p.id == id && p.session.as_ref().is_some_and(|s| !s.stopping()))
            });
            // A session whose client never connected control, or whose peer
            // this worker lost when it restarted, would otherwise hold the
            // encoder, capture and display forever (Sunshine times it out).
            // Its timeout is seconds long: input passes leave it to the timer.
            if sweep_due {
                active_sessions.extend(h.sessions.lock().unwrap().active.values().cloned());
                unattended.retain(|id, _| active_sessions.iter().any(|s| &s.launch.id == id));
                end_unattended_sessions(
                    &mut active_sessions,
                    |id| peers.values().any(|p| p.id == id),
                    &mut unattended,
                    ping_timeout,
                    &socket,
                );
            }
            host.flush();
            if peers.is_empty() {
                priority = None;
            } else if priority.is_none() {
                priority = Some(Priority::input());
            }
            let now = Instant::now();
            let due = if peers.is_empty() {
                now + Duration::from_millis(250)
            } else {
                peers
                    .values()
                    .flat_map(|p| {
                        [
                            p.injector.as_ref().and_then(Injector::next_due),
                            p.couch.next_due(),
                            p.audio_haptics.arbiter.next_due(),
                            p.session.as_ref().and_then(|s| s.client_cursor.next_due()),
                        ]
                    })
                    .flatten()
                    .fold(feedback_at, Instant::min)
            };
            // And for the unattended sweep's timer.
            let due = due.min(unattended_sweep.next());
            let timeout = control_wait_timeout(due.saturating_duration_since(now), serviced);
            if butterpollo_win_io::net::wait_readable_or_wake(raw_socket, raw_wake, timeout)
                .is_err()
            {
                // A polling error must not spin the loop.
                input_timer.until(now + Duration::from_millis(timeout.max(1) as u64));
            }
        }
        // The host stops: every client hears its forwarded USB devices detach and the
        // 0x0109 termination through the path a normal stop uses, before the control
        // socket closes. `Media::clients_told_of_shutdown` waits a second for this.
        self.tell_clients_of_shutdown(&h, &mut host, &mut peers);
        for (peer, p) in peers {
            host.peer_mut(peer).disconnect_now(0);
            drop(p);
        }
        Ok(())
    }

    /// Tells every client whose stream is live that the host stops, through the path a
    /// normal stop uses, waits briefly for them to acknowledge it, and records the count and
    /// time for [`Media::clients_told_of_shutdown`]. A session already stopping was told by
    /// the normal path, and its control peer may have gone with it.
    fn tell_clients_of_shutdown(
        &self,
        h: &Shared,
        host: &mut Host<ControlSocket>,
        peers: &mut HashMap<PeerID, ControlPeer>,
    ) {
        let started = Instant::now();
        let mut told = 0;
        for (peer_id, p) in peers.iter_mut() {
            let s = {
                let sessions = h.sessions.lock().unwrap();
                sessions.active.get(&p.id).cloned()
            }
            .or_else(|| p.session.clone());
            let Some(s) = s else { continue };
            if s.stopping() {
                continue;
            }
            if tell_client_stopping(h, host, *peer_id, p, &s) {
                told += 1;
            }
        }
        if told > 0 {
            host.flush();
            // ENet sends the disconnect `tell_client_stopping` queued only once the
            // termination is acknowledged. Wait for that (or a short deadline), so each
            // client dispatches the notice before the caller ends its peer: ending a peer
            // whose notice it has not dispatched drops the notice with its queue.
            let deadline = Instant::now() + NOTICE_ACK_WAIT;
            while Instant::now() < deadline
                && peers
                    .keys()
                    .any(|peer| host.peer(*peer).state() == PeerState::DisconnectLater)
            {
                let _ = host.service();
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.shutdown_told.store(told, Ordering::Release);
        self.shutdown_elapsed_ms
            .store(started.elapsed().as_millis() as u64, Ordering::Release);
        self.shutdown_done.store(true, Ordering::Release);
    }
}

fn handle_connect(
    h: &Shared,
    socket: &ReplySocket,
    ping_timeout: Duration,
    control_seqs: &super::super::ControlSeqMemory,
    peers: &mut HashMap<PeerID, ControlPeer>,
    peer: &mut rusty_enet::Peer<ControlSocket>,
    data: u32,
) -> bool {
    let Some(address) = peer.address() else {
        peer.disconnect_now(0);
        return false;
    };
    let sessions = h.sessions.lock().unwrap();
    let ip = address.ip().to_canonical();
    // A session's client may have roamed since its launch.
    let candidates: Vec<_> = sessions
        .active
        .values()
        .filter(|s| !s.stopping() && s.client_ip() == ip)
        .map(|s| &s.launch)
        .chain(sessions.pending.values().filter(|l| l.peer == ip))
        .collect();
    let launch = candidates
        .iter()
        .find(|l| data == l.connect_data)
        .copied()
        .or_else(|| {
            if data == 0 && candidates.len() == 1 {
                Some(candidates[0])
            } else {
                None
            }
        });
    let local = describe_local(socket.source(address));
    let mut accepted = false;
    if let Some(l) = launch {
        if peers.values().any(|p| p.id == l.id) {
            tracing::warn!(client = %l.client.name, %address, %local, "session already has a control connection; refused another");
            peer.disconnect_now(0);
            socket.forget(address);
            return false;
        }
        tracing::info!(client = %l.client.name, %address, %local, "control connected");
        // ENet's own peer timeout (about 5 s) would end a
        // stream on a Wi-Fi drop the session itself
        // tolerates; let ping_timeout decide instead.
        let timeout = u32::try_from(ping_timeout.as_millis()).unwrap_or(u32::MAX);
        peer.set_timeout(32, timeout, timeout.max(30_000));
        // After an outage, what the host queued (termination, rumble, HDR
        // metadata) goes out with the client's first datagram rather than
        // up to an outage later (client/protocol/vendor/rusty_enet).
        peer.set_resend_after_silence(true);
        // The session can finish before the first
        // control sweep. Retain it while the active
        // map is locked so its termination reason
        // survives the worker moving to teardown.
        let active = sessions.active.get(&l.id).cloned();
        peers.insert(
            peer.id(),
            ControlPeer::new(h, l, peer.id(), active, control_seqs),
        );
        accepted = true;
    } else {
        tracing::warn!(
            %address,
            %local,
            sessions = candidates.len(),
            "control connection matches no session from this address; refused"
        );
        peer.disconnect_now(0);
        socket.forget(address);
    }
    #[cfg(any(test, feature = "test-support"))]
    if let Some(fixture) = &h.reconnect_fixture
        && let Some(connected) = peers.get(&peer.id())
    {
        let id = connected.id.clone();
        drop(sessions);
        fixture.control_connected(h, &id);
    }
    accepted
}

fn handle_disconnect(
    h: &Shared,
    socket: &ReplySocket,
    ping_timeout: Duration,
    peers: &mut HashMap<PeerID, ControlPeer>,
    peer: &mut rusty_enet::Peer<ControlSocket>,
) -> Option<ControlPeer> {
    if let Some(address) = peer.address() {
        socket.forget(address);
    }
    let id = peer.id();
    let p = peers.remove(&id)?;
    crate::dsu::forget(&p.id);
    let session = h.sessions.lock().unwrap().active.get(&p.id).cloned();
    // ENet timed out a client that went quiet: the stream was lost, as the
    // ping check below says when it sees that first. A pass that runs late
    // gets ENet's Disconnect before it.
    if let Some(session) = &session
        && !session.stopping()
        && p.seen.elapsed() > ping_timeout
    {
        tracing::warn!(client=%session.launch.client.name,"client control stream timed out");
        session.fail("the client's control stream timed out");
    }
    let canceled = h.sessions.lock().unwrap().request_stop(Some(&p.id));
    drop(canceled);
    // Its forwarded USB devices detach; the client is gone, so nobody
    // hears.
    drop(h.usb.end_session(&p.id));
    if let Some(session) = session
        && session.launch.role == Role::RemoteMonitor
        && h.config.read().unwrap().remove_remote_monitor()
            == butterpollo_config::config::RemoveRemoteMonitor::Disconnect
    {
        // Laying the displays out again takes long enough to hold up
        // every other client's input, so it runs beside this loop.
        let host = h.clone();
        let client = session.launch.client.uuid.clone();
        let started = thread::Builder::new()
            .name("remote monitor end".into())
            .spawn(move || crate::remote_display::disconnect(&host, Some(&client)));
        if let Err(error) = started {
            tracing::warn!(%error, "remote monitor end thread did not start; ended inline");
            crate::remote_display::disconnect(h, Some(&session.launch.client.uuid));
        }
    }
    Some(p)
}

#[allow(clippy::too_many_arguments)]
fn handle_receive(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    commands: &std::sync::mpsc::SyncSender<(Arc<Session>, crate::process::MenuRun)>,
    peers: &mut HashMap<PeerID, ControlPeer>,
    peer: &mut rusty_enet::Peer<ControlSocket>,
    data: &[u8],
    scratch: &mut ReceiveScratch,
) {
    let Some(p) = peers.get_mut(&peer.id()) else {
        return;
    };
    let s = match &p.session {
        Some(s) if s.stopping() => return,
        Some(s) => s.clone(),
        None => {
            if let Some(s) = h.sessions.lock().unwrap().active.get(&p.id).cloned() {
                p.session = Some(s.clone());
                s
            } else {
                if (2..=65536).contains(&data.len()) {
                    p.seen = Instant::now();
                    if p.pre_active_packets.len() < 64 {
                        p.pre_active_packets.push_back(data.to_vec());
                    }
                }
                return;
            }
        }
    };
    while let Some(buffered) = p.pre_active_packets.pop_front() {
        if s.stopping() {
            p.pre_active_packets.clear();
            break;
        }
        handle_control_packet(h, control_wake, commands, peer, p, &s, &buffered, scratch);
    }
    if !s.stopping() {
        handle_control_packet(h, control_wake, commands, peer, p, &s, data, scratch);
    }
}

/// Polls one peer per control-loop pass; returns `false` when the peer should be removed.
#[allow(clippy::too_many_arguments)]
fn poll_peer(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    commands: &std::sync::mpsc::SyncSender<(Arc<Session>, crate::process::MenuRun)>,
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    ping_timeout: Duration,
    poll_feedback: bool,
    received: Option<Instant>,
    menus: &mut crate::couch_menu::Workers,
    scratch: &mut ReceiveScratch,
) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if h.reconnect_fixture.is_some() {
        p.injector_tried = true;
    }
    // The sessions lock only while the peer has no session: a
    // launch id names one session, so the map cannot hold another
    // under this peer's id while it has one, live or going. A peer
    // without a session watches for its launch starting, and the
    // pending flag below keeps it until one does or it times out.
    let (s, pending) = match &p.session {
        Some(s) => (Some(s.clone()), true),
        None => {
            let sessions = h.sessions.lock().unwrap();
            let s = sessions.active.get(&p.id).cloned();
            let pending = sessions.pending.contains_key(&p.id);
            (s, pending)
        }
    };
    let Some(s) = s else {
        if !pending || p.seen.elapsed() > ping_timeout {
            drop(h.usb.end_session(&p.id));
            host.peer_mut(peer_id).disconnect_now(0);
            return false;
        }
        return true;
    };
    if p.session.is_none() {
        p.session = Some(s.clone());
        while let Some(buffered) = p.pre_active_packets.pop_front() {
            if s.stopping() {
                p.pre_active_packets.clear();
                break;
            }
            handle_control_packet(
                h,
                control_wake,
                commands,
                host.peer_mut(peer_id),
                p,
                &s,
                &buffered,
                scratch,
            );
        }
    }
    let timed_out = p.seen.elapsed() > ping_timeout;
    if s.stopping() || timed_out {
        if timed_out && !s.stopping() {
            tracing::warn!(client=%s.launch.client.name,"client control stream timed out");
            s.fail("the client's control stream timed out");
        }
        tell_client_stopping(h, host, peer_id, p, &s);
        return false;
    }
    // A session takes roaming probes once its stream and ENet peer are up,
    // and only if its DESCRIBE offered roaming.
    if !p.roaming
        && s.launch.roaming_offered.load(Ordering::Acquire)
        && let Some(address) = host.peer(peer_id).address()
    {
        host.socket_mut()
            .roaming
            .insert(&p.id, &s.launch.key, address);
        p.roaming = true;
    }
    if !p.input_released && p.seen.elapsed() >= Duration::from_secs(1) {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(fixture) = &h.reconnect_fixture {
            fixture.input_releases.fetch_add(1, Ordering::Release);
        }
        if let Some(injector) = &mut p.injector {
            injector.release_all();
        }
        if let Some(player) = &p.coop {
            player.release_all();
        }
        p.input_released = true;
    }
    tick_and_send_qos(host, peer_id, p, &s, poll_feedback);
    send_extension_updates(h, host, peer_id, p, &s);
    super::quick_suspend::send(h, control_wake, host, peer_id, p, &s);
    send_clipboard_and_focus_updates(h, control_wake, host, peer_id, p, &s);
    if !ensure_input_ready(h, control_wake, p, &s) {
        return true;
    }
    follow_session(h, p, &s);
    // A guest that lost the keyboard and mouse lets go of what it holds.
    if let Some(i) = &mut p.injector
        && h.device_guests.take_release(&s)
    {
        i.release_keyboard_mouse();
    }
    p.audio_haptics.follow(
        h.config_version.load(Ordering::Acquire),
        &s.launch.warnings,
        || effective_config(h, &s.launch),
    );
    if p.couch.follow(&s, || effective_config(h, &s.launch)) {
        let wake = control_wake.clone();
        s.couch_menu.set_control_wake(move || {
            let _ = wake.send(&[1]);
        });
    }
    // What the pass above left: input of a peer whose injector or
    // co-op player was made just now, and the couch menu's timers.
    apply_input(h, p, &s, menus, received);
    if !poll_feedback
        && let Some(i) = &mut p.injector
        && let Err(e) = i.flush_virtual_left_release().and_then(|()| i.due())
    {
        s.launch.warnings.event("input_repeat", format!("Input release or repeat failed ({e:#}); held controls may not update. Unlock the desktop or reconnect."), butterpollo_session::session::EVENT_PERIOD);
    }
    if poll_feedback
        && let Some(i) = &mut p.injector
        && let Err(e) = i.refresh()
    {
        s.launch.warnings.event("input_pointer", format!("Touch/pen pointer refresh failed ({e:#}); pointer updates may be missing. Unlock the desktop or disable native touch/pen if Windows does not support it."), butterpollo_session::session::EVENT_PERIOD);
    }
    send_pad_feedback(h, host, peer_id, p, &s, poll_feedback);
    true
}

#[allow(clippy::too_many_arguments)]
fn handle_control_packet(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    commands: &std::sync::mpsc::SyncSender<(Arc<Session>, crate::process::MenuRun)>,
    peer: &mut rusty_enet::Peer<ControlSocket>,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    b: &[u8],
    scratch: &mut ReceiveScratch,
) {
    if b.len() < 2 || b.len() > 65536 {
        return;
    }
    let encrypted = b[..2] == [1, 0];
    if s.config.encryption & 1 != 0 && !encrypted {
        return;
    }
    let (kind, payload) = match if encrypted {
        p.ciphers
            .get(&s.launch.key, s.config.encryption & 1 != 0)
            .open_into(b, &mut scratch.1)
            .and_then(|(seq, kind, payload)| {
                anyhow::ensure!(
                    p.seq_state.lock().unwrap().received.accept(seq),
                    "replayed control message"
                );
                Ok((kind, payload))
            })
    } else {
        butterpollo_media_policy::packet::control_header(b)
    } {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error=%e,"invalid control packet");
            return;
        }
    };
    p.seen = Instant::now();
    p.input_released = false;
    match kind {
        controller_audio::CONTROL | controller_audio::MICROPHONE
            if encrypted && haptics_negotiated(&s.config, peer.channel_count()) =>
        {
            handle_controller_audio(h, peer, p, s, kind, payload);
        }
        0x0109 => {
            if encrypted {
                s.stop();
            }
        }
        0x0206 => {
            handle_input_packet(h, peer, p, s, payload, encrypted, &mut scratch.2);
        }
        _ => {
            if handle_qos_packet(peer, p, s, kind, payload, &mut scratch.0) {
                return;
            }
            if handle_clipboard_and_focus_packet(h, control_wake, p, s, kind, payload, encrypted) {
                return;
            }
            handle_extension_packet(h, control_wake, commands, p, s, kind, payload, encrypted);
        }
    }
}
