//! The control worker: ENet control peers, input dispatch, feedback and roaming for every
//! session.
use super::*;

pub(super) fn server_command(
    h: &Shared,
    client: &butterpollo_core::state::Client,
    payload: &[u8],
    authenticated: bool,
    command_at: &mut Option<Instant>,
) -> Option<(String, bool)> {
    // Legacy plaintext control is not proof that the sender owns the stream key.
    if !authenticated || !client.allows(1 << 20) || payload.len() != 1 {
        return None;
    }
    if h.current_app
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|app| !app.allow_client_commands)
        || command_at.is_some_and(|last| last.elapsed() < Duration::from_secs(1))
    {
        return None;
    }
    *command_at = Some(Instant::now());
    let commands: serde_json::Value =
        serde_json::from_str(h.config.read().unwrap().get("server_cmd", "[]")).unwrap_or_default();
    let command = commands.as_array()?.get(payload[0] as usize)?;
    Some((
        command.get("cmd")?.as_str()?.to_owned(),
        command
            .get("elevated")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    ))
}

/// Expire controller sessions once a second (`last` is the last time). Input
/// never waits behind housekeeping: a held app is looked at on the next pass.
pub(super) fn expire_controllers(h: &crate::state::Host, last: &mut Instant) {
    if last.elapsed() >= Duration::from_secs(1)
        && let Some(app_running) = h.try_current_app().map(|current| current.is_some())
    {
        h.controller_sessions.expire(app_running);
        *last = Instant::now();
    }
}

pub(super) const CONTROL_EVENT_LIMIT: usize = 512;

pub(super) fn control_wait_timeout(remaining: Duration, events: usize) -> i32 {
    if events == CONTROL_EVENT_LIMIT {
        0
    } else {
        remaining.as_micros().div_ceil(1000) as i32
    }
}

/// Whether the ENet control worker should see a peer for `session`. A WebRTC
/// session carries control and input on its DataChannels, so it never has one.
pub(super) fn expects_enet_control(session: &Session) -> bool {
    session.launch.transport != butterpollo_core::session::Transport::WebRtc
}

/// Ends the sessions that expect an ENet control peer, have had none for longer than
/// `ping_timeout`, and are not attended now (`attended` says by session id). Drains `sessions`.
/// The log names what `control` received from the client, if anything.
pub(super) fn end_unattended_sessions(
    sessions: &mut Vec<Arc<Session>>,
    attended: impl Fn(&str) -> bool,
    unattended: &mut HashMap<String, Instant>,
    ping_timeout: Duration,
    control: &ReplySocket,
) {
    for s in sessions.drain(..) {
        let id = &s.launch.id;
        if s.stopping() || !expects_enet_control(&s) || attended(id) {
            unattended.remove(id);
            continue;
        }
        let since = match unattended.get(id) {
            Some(since) => *since,
            None => {
                let since = Instant::now();
                unattended.insert(id.clone(), since);
                since
            }
        };
        if since.elapsed() > ping_timeout {
            let received: Vec<_> = control
                .sources_from(s.client_ip())
                .iter()
                .map(|(peer, local)| format!("{peer} on {local}"))
                .collect();
            tracing::warn!(
                client = %s.launch.client.name,
                peer = %s.client_ip(),
                control_datagrams = if received.is_empty() { "none".into() } else { received.join(", ") },
                "session has no control connection; ending it"
            );
            s.fail();
            s.stop();
        }
    }
}

/// ENet's UDP adapter ends the control worker on any error but WouldBlock,
/// and nothing restarted it: a Wi-Fi roam (WSAENETUNREACH), an ICMP reply
/// (WSAENETRESET) or an oversized datagram (WSAEMSGSIZE) left every later
/// session without input or control. These lose one datagram; ENet resends
/// what was reliable.
pub(super) struct ControlSocket {
    /// Answers each client from the address its datagrams arrived on, as
    /// moonlight-common-c's ENet does; shared so the worker can log it.
    pub(super) socket: Arc<ReplySocket>,
    /// Whether datagrams are held back, and those held, in order: ENet
    /// sends a packet's acknowledgement before it returns the packet, and
    /// that sendto delayed every input by its cost.
    holding: bool,
    held: Vec<(SocketAddr, Vec<u8>)>,
    /// Client roaming: what a moved client's datagrams map to.
    pub(super) roaming: roaming::Roaming,
}

impl ControlSocket {
    pub(super) fn new(socket: Arc<ReplySocket>) -> Self {
        Self {
            socket,
            holding: false,
            held: Vec::new(),
            roaming: Default::default(),
        }
    }
    /// Hold back what ENet sends until release().
    pub(super) fn hold(&mut self) {
        self.holding = true;
    }
    /// Send what was held, in order, and stop holding. A datagram that is
    /// lost is logged; any other error ends the worker, as it would have
    /// inside service().
    pub(super) fn release(&mut self) -> std::io::Result<()> {
        self.holding = false;
        for (address, buffer) in self.held.drain(..) {
            let address = self.roaming.wire_of(address);
            if let Err(error) = self.socket.send_to(&buffer, address) {
                if !butterpollo_windows::net::datagram_lost(&error) {
                    return Err(error);
                }
                tracing::debug!(%error, %address, "control datagram dropped");
            }
        }
        Ok(())
    }
}

impl rusty_enet::Socket for ControlSocket {
    type Address = SocketAddr;
    type Error = std::io::Error;
    fn init(&mut self, _options: rusty_enet::SocketOptions) -> std::io::Result<()> {
        // What ENet's own UdpSocket adapter sets.
        self.socket.set_nonblocking(true)?;
        self.socket.set_broadcast(true)
    }
    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> std::io::Result<usize> {
        if self.holding {
            self.held.push((address, buffer.to_vec()));
            return Ok(buffer.len());
        }
        match self.socket.send_to(buffer, self.roaming.wire_of(address)) {
            Err(error) if butterpollo_windows::net::datagram_lost(&error) => Ok(0),
            result => result,
        }
    }
    fn receive(
        &mut self,
        buffer: &mut [u8; rusty_enet::MTU_MAX],
    ) -> std::io::Result<Option<(SocketAddr, rusty_enet::PacketReceived)>> {
        // A flood of roaming datagrams, or of datagrams from an address a client left,
        // must not keep ENet's service from returning.
        for _ in 0..SWALLOWED_PER_RECEIVE {
            let (size, from, local) = match self.socket.receive(buffer) {
                Ok(received) => received,
                Err(error) if butterpollo_windows::net::datagram_lost(&error) => return Ok(None),
                Err(error) => return Err(error),
            };
            if self.roaming.intercepts(&buffer[..size]) {
                self.roaming.on_datagram(
                    &self.socket,
                    Instant::now(),
                    from,
                    local,
                    &buffer[..size],
                );
                continue;
            }
            let peer = match self.roaming.peer_of(from) {
                Some(peer) => peer,
                None if self.roaming.reopens(from, &buffer[..size]) => from,
                None => {
                    self.roaming.dropped(Instant::now(), from);
                    continue;
                }
            };
            // ENet answers before it knows the session: learn from every datagram.
            self.socket.answer_from(from, local);
            return Ok(Some((peer, rusty_enet::PacketReceived::Complete(size))));
        }
        Ok(None)
    }
}

/// Datagrams one `ControlSocket::receive` takes for roaming or drops before it returns
/// none; the rest wait for ENet's next service.
pub(super) const SWALLOWED_PER_RECEIVE: usize = 64;

impl Media {
    pub(super) fn control(&self, h: Shared) -> Result<()> {
        let socket = Arc::new(ReplySocket::new(crate::network::udp(
            (self.bind, self.control_port).into(),
        )?)?);
        butterpollo_windows::net::configure_udp(&socket)?;
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
            std::sync::mpsc::sync_channel::<(Arc<Session>, String, bool)>(32);
        thread::Builder::new()
            .name("server commands".into())
            .spawn(move || {
                for (session, value, elevated) in pending_commands {
                    if session.stopping() {
                        continue;
                    }
                    if let Err(error) = butterpollo_windows::process::Process::shell_detached(
                        &value,
                        None,
                        elevated,
                        &Default::default(),
                    ) {
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
        let input_timer = butterpollo_windows::timing::Timer::new()?;
        let mut feedback_at = Instant::now();
        let mut controller_cleanup = Instant::now();
        while !h.stop.load(Ordering::Acquire) {
            while self.control_wake_receiver.recv(&mut [0]).is_ok() {}
            expire_controllers(&h, &mut controller_cleanup);
            let (ping_timeout, client_roaming) = {
                let c = h.config.read().unwrap();
                (crate::network::ping_timeout(&c), c.client_roaming())
            };
            host.socket_mut().roaming.set_enabled(client_roaming);
            // Whether a peer went, so roaming forgets its addresses.
            let mut peers_went = false;
            #[cfg(test)]
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
                        let Some(address) = peer.address() else {
                            peer.disconnect_now(0);
                            continue;
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
                        if let Some(l) = launch {
                            if peers.values().any(|p| p.id == l.id) {
                                tracing::warn!(client = %l.client.name, %address, %local, "session already has a control connection; refused another");
                                peer.disconnect_now(0);
                                socket.forget(address);
                                continue;
                            }
                            tracing::info!(client = %l.client.name, %address, %local, "control connected");
                            // ENet's own peer timeout (about 5 s) would end a
                            // stream on a Wi-Fi drop the session itself
                            // tolerates; let ping_timeout decide instead.
                            let timeout =
                                u32::try_from(ping_timeout.as_millis()).unwrap_or(u32::MAX);
                            peer.set_timeout(32, timeout, timeout.max(30_000));
                            // After an outage, what the host queued (termination, rumble, HDR
                            // metadata) goes out with the client's first datagram rather than
                            // up to an outage later (rust/vendor/rusty_enet).
                            peer.set_resend_after_silence(true);
                            peers.insert(
                                peer.id(),
                                ControlPeer {
                                    #[cfg(test)]
                                    _fixture_input: h.reconnect_fixture.as_ref().map(|f| f.input()),
                                    id: l.id.clone(),
                                    injector: None,
                                    injector_tried: false,
                                    coop: None,
                                    coop_tried: false,
                                    sequence: 0,
                                    received: Default::default(),
                                    hdr_metadata: None,
                                    legacy: butterpollo_core::packet::LegacyInput::new(l.key_id),
                                    seen: Instant::now(),
                                    input_released: false,
                                    // The session can finish before the first
                                    // control sweep. Retain it while the active
                                    // map is locked so its termination reason
                                    // survives the worker moving to teardown.
                                    session: sessions.active.get(&l.id).cloned(),
                                    inputs: vec![],
                                    denied: 0,
                                    steam_attributes_refused: false,
                                    controller_audio: [0; 16],
                                    controller_pcm: Default::default(),
                                    rumble: Default::default(),
                                    command_at: None,
                                    roaming: false,
                                    couch: Default::default(),
                                    audio_haptics: Default::default(),
                                    keyboard_layout: Default::default(),
                                    cursor_owner: peer.id().0,
                                    focus_watch: None,
                                    clipboard_sync: Default::default(),
                                },
                            );
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
                        #[cfg(test)]
                        if let Some(fixture) = &h.reconnect_fixture
                            && let Some(connected) = peers.get(&peer.id())
                        {
                            let id = connected.id.clone();
                            drop(sessions);
                            fixture.control_connected(&h, &id);
                        }
                    }
                    Event::Disconnect { peer, .. } => {
                        peers_went = true;
                        if let Some(address) = peer.address() {
                            socket.forget(address);
                        }
                        let id = peer.id();
                        if let Some(p) = peers.remove(&id) {
                            let session = h.sessions.lock().unwrap().active.get(&p.id).cloned();
                            // ENet timed out a client that went quiet: the stream was lost, as the
                            // ping check below says when it sees that first. A pass that runs late
                            // gets ENet's Disconnect before it.
                            if let Some(session) = &session
                                && !session.stopping()
                                && p.seen.elapsed() > ping_timeout
                            {
                                tracing::warn!(client=%session.launch.client.name,"client control stream timed out");
                                session.fail();
                            }
                            let canceled = h.sessions.lock().unwrap().request_stop(Some(&p.id));
                            drop(canceled);
                            // Its forwarded USB devices detach; the client is gone, so nobody
                            // hears.
                            drop(h.usb.end_session(&p.id));
                            if let Some(session) = session
                                && session.launch.role == Role::RemoteMonitor
                                && h.config.read().unwrap().boolean(
                                    "remote_monitor_disconnect_on_client_disconnect",
                                    false,
                                )
                            {
                                crate::remote_display::disconnect(
                                    &h,
                                    Some(&session.launch.client.uuid),
                                );
                            }
                            disconnected.push(p);
                        }
                    }
                    Event::Receive { peer, packet, .. } => {
                        let Some(p) = peers.get_mut(&peer.id()) else {
                            continue;
                        };
                        let s = h.sessions.lock().unwrap().active.get(&p.id).cloned();
                        let Some(s) = s else { continue };
                        p.session = Some(s.clone());
                        let b = packet.data();
                        if b.len() < 2 || b.len() > 65536 {
                            continue;
                        }
                        let encrypted = b[..2] == [1, 0];
                        if s.config.encryption & 1 != 0 && !encrypted {
                            continue;
                        }
                        let parsed = if encrypted {
                            butterpollo_core::packet::decrypt_control(
                                &s.launch.key,
                                b,
                                s.config.encryption & 1 != 0,
                            )
                            .and_then(|(seq, kind, payload)| {
                                if !p.received.accept(seq) {
                                    anyhow::bail!("replayed control message");
                                }
                                Ok((kind, payload))
                            })
                        } else {
                            butterpollo_core::packet::control_header(b)
                                .map(|(kind, payload)| (kind, payload.to_vec()))
                        };
                        let (kind, payload) = match parsed {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::debug!(error=%e,"invalid control packet");
                                continue;
                            }
                        };
                        p.seen = Instant::now();
                        p.input_released = false;
                        match kind {
                            0x0200 => s.record_control_ping(Instant::now(), &payload),
                            controller_audio::CONTROL | controller_audio::MICROPHONE
                                if encrypted
                                    && haptics_negotiated(&s.config, peer.channel_count()) =>
                            {
                                if !s.launch.client.allows(1 << 8) {
                                    continue;
                                }
                                if kind == controller_audio::CONTROL {
                                    if let Some((id, flags)) = controller_audio::status(&payload) {
                                        let Ok(config) = effective_config(&h, &s.launch) else {
                                            continue;
                                        };
                                        let policy = controller_audio::status_policy(
                                            &config,
                                            s.launch.role,
                                            s.launch.client.perm & ((1 << 25) | (1 << 26)) != 0,
                                            flags,
                                        );
                                        if flags & controller_audio::PLAYBACK != 0
                                            && policy.device_flags & controller_audio::PLAYBACK == 0
                                            && config.get("audio_capture_mode", "system") == "game"
                                        {
                                            s.launch.warnings.set("audio_controller_scope", "Game-only audio suppresses dedicated USB controller speaker sound because that endpoint cannot identify its source process. The normal captured game stream still routes to a connected controller headset; haptics and microphone settings remain independent.");
                                        }
                                        let mut state = payload.clone();
                                        state[4] = policy.device_flags;
                                        if let Some(i) = &p.injector {
                                            i.controller_audio(id, kind, &state);
                                        }
                                        p.controller_audio[id as usize] = policy.device_flags;
                                        state[1] = 3; // Acknowledge host microphone policy.
                                        // Auto may have attached a VHF pad without audio.
                                        // The client's periodic status retries after USB
                                        // enumeration; retain its desired device policy.
                                        state[4] = policy.acknowledged_flags(
                                            p.injector
                                                .as_ref()
                                                .is_some_and(|i| i.controller_audio_ready(id)),
                                        );
                                        if let Ok(message) = p.encrypt(&s, kind, &state) {
                                            let _ = peer.send(
                                                FEEDBACK_CHANNEL,
                                                &Packet::new(message, PacketKind::Reliable),
                                            );
                                        }
                                    }
                                } else if let Some((id, _, _)) = controller_audio::pcm(&payload)
                                    && p.controller_audio[id as usize]
                                        & (controller_audio::CAPTURE | controller_audio::MUTED)
                                        == controller_audio::CAPTURE
                                    && let Some(i) = &p.injector
                                {
                                    p.controller_pcm.observe(id, kind, Instant::now());
                                    i.controller_audio(id, kind, &payload);
                                }
                            }
                            0x3000 => {
                                if let Some((value, elevated)) = server_command(
                                    &h,
                                    &s.launch.client,
                                    &payload,
                                    encrypted,
                                    &mut p.command_at,
                                ) && let Err(e) = commands.try_send((s.clone(), value, elevated))
                                {
                                    tracing::warn!(error=%e, "configured server command failed");
                                }
                            }
                            // SS_RFI_REQUEST: first frame, a reserved word, last
                            // frame, reserved words; older clients sent two
                            // 64-bit indices, whose low words sit at the same
                            // offsets. Requiring 8 bytes turned every request
                            // into a keyframe.
                            0x0301 if payload.len() >= 12 => {
                                s.request_invalidation(
                                    u64::from(u32::from_le_bytes(payload[..4].try_into().unwrap())),
                                    u64::from(u32::from_le_bytes(
                                        payload[8..12].try_into().unwrap(),
                                    )),
                                );
                            }
                            0x0301 => s.request_invalidation(0, 0),
                            0x0302 => s.request_client_keyframe(),
                            // This dispatch receives only client-to-host packets;
                            // 0x5502 in the other direction is controller feedback.
                            0x5502 => s.record_fec_status(&payload),
                            butterpollo_core::statistical_fec::FEC_REPORT_MESSAGE_TYPE => {
                                match s.record_fec_report(&payload) {
                                    Ok((report, used)) => tracing::debug!(
                                        seq = report.seq,
                                        data = report.data_shards,
                                        parity = report.parity_shards,
                                        missing = report.missing_shards,
                                        unrecovered = report.unrecovered_blocks,
                                        interval_ms = report.interval_ms,
                                        used,
                                        "fec report"
                                    ),
                                    Err(error) => {
                                        tracing::debug!(%error, "fec report refused")
                                    }
                                }
                            }
                            butterpollo_core::phase_sync::REPORT_MESSAGE_TYPE => {
                                match s
                                    .phase_sync
                                    .lock()
                                    .unwrap()
                                    .on_payload(Instant::now(), &payload)
                                {
                                    Some(report) => tracing::debug!(
                                        lead_ns = report.lead_ns,
                                        spread_ns = report.spread_ns,
                                        period_ns = report.period_ns,
                                        frames = report.frames,
                                        "phase lock report"
                                    ),
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid phase lock report"
                                    ),
                                }
                            }
                            // Answered at once, unreliable: a lost exchange is one sample.
                            butterpollo_core::clock_sync::CLOCK_SYNC_MESSAGE_TYPE => {
                                let epoch = *s.media_epoch.read().unwrap();
                                if let Some(reply) =
                                    butterpollo_core::clock_sync::reply(epoch, p.seen, &payload)
                                    && let Ok(message) = p.encrypt(&s, kind, &reply)
                                {
                                    let _ = peer.send(
                                        FEEDBACK_CHANNEL,
                                        &Packet::new(
                                            message,
                                            PacketKind::AlwaysUnreliable { sequenced: false },
                                        ),
                                    );
                                }
                            }
                            butterpollo_core::host_timing::HOST_TIMING_MESSAGE_TYPE => {
                                match s.host_timings.lock().unwrap().on_request(&payload) {
                                    Some(enabled) => tracing::info!(
                                        client = %s.launch.client.name,
                                        enabled,
                                        "host timing reports"
                                    ),
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid host timing request"
                                    ),
                                }
                            }
                            butterpollo_core::link_notice::LINK_NOTICE_MESSAGE_TYPE => {
                                match s.link_notices.lock().unwrap().on_request(&payload) {
                                    Some(enabled) => tracing::info!(
                                        client = %s.launch.client.name,
                                        enabled,
                                        "link notices"
                                    ),
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid link notice request"
                                    ),
                                }
                            }
                            butterpollo_core::pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE => {
                                let counts = s.pipeline_counts();
                                match s.pipeline_rates.lock().unwrap().on_request(
                                    Instant::now(),
                                    &payload,
                                    counts,
                                ) {
                                    Some(enabled) => tracing::info!(
                                        client = %s.launch.client.name,
                                        enabled,
                                        "pipeline rate reports"
                                    ),
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid pipeline rates request"
                                    ),
                                }
                            }
                            // Keyboard follows focus: only for a device that may type, and
                            // the text around the cursor only over encrypted control.
                            butterpollo_core::text_input::TEXT_INPUT_MESSAGE_TYPE => {
                                let Ok(config) = effective_config(&h, &s.launch) else {
                                    continue;
                                };
                                let offered = crate::extensions::offered(&h, &config).text_input
                                    && s.launch.client.allows(input::KEYBOARD_PERMISSION);
                                let surrounding = encrypted && config.text_input_surrounding_text();
                                let watch = s.text_input.lock().unwrap().on_request(
                                    &payload,
                                    offered,
                                    surrounding,
                                );
                                match watch {
                                    Some(watch) => {
                                        tracing::info!(
                                            client = %s.launch.client.name,
                                            ?watch,
                                            offered,
                                            "text input"
                                        );
                                        // The fixture's tests note the focus themselves.
                                        #[cfg(test)]
                                        if let Some(fixture) = &h.reconnect_fixture {
                                            fixture.text_watches.lock().unwrap().push(watch);
                                            continue;
                                        }
                                        p.watch_focus(&s, watch, &self.control_wake);
                                    }
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid text input request"
                                    ),
                                }
                            }
                            // Clipboard sync from a Nightjar that asked or a Moonlight V+
                            // client, which never asks: its first frame starts the sync.
                            // Encrypted only, as USB: a plain frame anyone on the network
                            // could send would set the PC's clipboard and start sending it.
                            butterpollo_core::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE
                                if encrypted =>
                            {
                                let Ok(config) = effective_config(&h, &s.launch) else {
                                    continue;
                                };
                                let allowed = clipboard_sync::allowed(&config, &s.launch.client);
                                p.clipboard_sync
                                    .receive(&s, &payload, allowed, &self.control_wake);
                            }
                            // The latest report wins; the injector maps keys with it.
                            butterpollo_core::keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE => {
                                let Ok(config) = effective_config(&h, &s.launch) else {
                                    continue;
                                };
                                if !crate::extensions::offered(&h, &config).keyboard_layout {
                                    tracing::debug!("keyboard layout report not offered");
                                    continue;
                                }
                                // Only a device that may type picks the layout keys go with.
                                if !s.launch.client.allows(input::KEYBOARD_PERMISSION) {
                                    tracing::debug!(
                                        "keyboard layout report without the keyboard permission"
                                    );
                                    continue;
                                }
                                match butterpollo_core::keyboard_layout::KeyboardLayout::decode(
                                    &payload,
                                ) {
                                    Some(layout) => {
                                        // Resolved once here; the layout is read on this
                                        // loop, never with input.
                                        let klid = butterpollo_core::keyboard_layout::reported_klid(
                                            &layout,
                                        );
                                        let before = std::mem::replace(
                                            &mut *s.keyboard_layout.lock().unwrap(),
                                            klid,
                                        );
                                        if before != klid {
                                            tracing::info!(
                                                client = %s.launch.client.name,
                                                source = ?layout.source,
                                                reported = format_args!("{:08X}", layout.klid),
                                                tag = %layout.tag,
                                                klid = ?klid.map(|klid| format!("{klid:08X}")),
                                                "keyboard layout"
                                            );
                                        }
                                        p.keyboard_layout.report(
                                            klid,
                                            &s.launch.warnings,
                                            Instant::now(),
                                        );
                                    }
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        "invalid keyboard layout report"
                                    ),
                                }
                            }
                            // The device draws the cursor (0x5540): the stream leaves it
                            // out of the video, and the pass below sends its shapes.
                            butterpollo_core::client_cursor::CURSOR_SHAPE_REQUEST_MESSAGE_TYPE => {
                                let Ok(config) = effective_config(&h, &s.launch) else {
                                    continue;
                                };
                                let offered = crate::extensions::offered(&h, &config).cursor_shape;
                                match s
                                    .client_cursor
                                    .on_request(&payload, offered, p.cursor_owner)
                                {
                                    Some(enabled) => {
                                        tracing::info!(
                                            client = %s.launch.client.name,
                                            enabled,
                                            "client-drawn cursor"
                                        );
                                        let _ = self.control_wake.send(&[1]);
                                    }
                                    None => tracing::debug!(
                                        len = payload.len(),
                                        offered,
                                        "invalid or unoffered cursor shape request"
                                    ),
                                }
                            }
                            // The registry checks the setting and the device's permission and
                            // queues the answer, which the pass below sends.
                            butterpollo_core::usb_forward::USB_MESSAGE_TYPE if encrypted => {
                                match butterpollo_core::usb_forward::UsbMessage::decode(&payload) {
                                    Some(message) => {
                                        let (enabled, limit) = {
                                            let config = h.config.read().unwrap();
                                            (config.usb_forwarding(), config.usb_max_devices())
                                        };
                                        h.usb.on_message(
                                            &p.id,
                                            &s.launch.client,
                                            enabled,
                                            limit,
                                            message,
                                        );
                                    }
                                    None => {
                                        tracing::debug!(len = payload.len(), "invalid USB message")
                                    }
                                }
                            }
                            butterpollo_core::display_caps::DISPLAY_CAPS_MESSAGE_TYPE => {
                                match s.record_display_caps(&payload) {
                                    Some(update) if update.changed => {
                                        tracing::info!(
                                            client = %s.launch.client.name,
                                            hdr = update.caps.hdr,
                                            peak_nits = update.caps.max_nits().unwrap_or(0),
                                            average_nits = update.caps.max_average_nits().unwrap_or(0),
                                            black_decimillinits = update.caps.min_decimillinits,
                                            applied = "diagnostic and HDR10+ target only; source HDR metadata retained",
                                            "display caps"
                                        );
                                    }
                                    Some(_) => tracing::debug!("display caps unchanged"),
                                    None => {
                                        tracing::debug!(len = payload.len(), "invalid display caps")
                                    }
                                }
                            }
                            // The session loop applies it once the client stops sending.
                            butterpollo_core::reconfigure::RECONFIGURE_MESSAGE_TYPE => {
                                let outcome = {
                                    let mut reconfigure = s.reconfigure.lock().unwrap();
                                    let outcome = reconfigure.on_payload(Instant::now(), &payload);
                                    s.rate_changes.lock().unwrap().on_reconfigure(outcome);
                                    outcome
                                };
                                tracing::debug!(
                                    ?outcome,
                                    len = payload.len(),
                                    "reconfigure request"
                                );
                            }
                            // A bitrate applies now, without a keyframe; a rate switches
                            // like a 0x5532 request and is answered once it runs.
                            butterpollo_core::rate_change::RATE_CHANGE_MESSAGE_TYPE => {
                                use butterpollo_core::rate_change;
                                if !matches!(
                                    rate_change::launch_choice(&s.launch.options),
                                    Ok(true)
                                ) {
                                    tracing::debug!("rate change not offered");
                                    continue;
                                }
                                let Some(request) =
                                    rate_change::RateChangeRequest::decode(&payload)
                                else {
                                    tracing::warn!(
                                        len = payload.len(),
                                        "invalid rate change request"
                                    );
                                    continue;
                                };
                                let config = effective_config(&h, &s.launch);
                                let mut reconfigure = s.reconfigure.lock().unwrap();
                                let requested = reconfigure.requested();
                                let current = reconfigure.current().fps_millihz;
                                let bitrate_kbps = s.bitrate.load(Ordering::Acquire);
                                let decision = match config {
                                    Ok(config) => {
                                        let enabled = config.boolean("stream_reconfigure", true);
                                        let mb_rate = h
                                            .encoder_mb_rates
                                            .get(usize::from(s.config.codec))
                                            .map_or(0, |rate| rate.load(Ordering::Acquire));
                                        let stream = rate_change::Stream {
                                            bitrate_kbps,
                                            fps_millihz: current,
                                            requested,
                                            fps_ceiling_millihz: rate_change::fps_ceiling_millihz(
                                                mb_rate,
                                                requested.width,
                                                requested.height,
                                                current,
                                                s.config.fps_millihz(),
                                            ),
                                            fps_refusal: butterpollo_core::reconfigure::refusal(
                                                &s.config, enabled,
                                            )
                                            .map(|_| rate_change::refusal_reason(enabled)),
                                        };
                                        rate_change::decide(&request, &stream, &config)
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %format!("{e:#}"), serial = request.serial, "rate change refused: the device's configuration did not load");
                                        rate_change::refuse(
                                            &request,
                                            bitrate_kbps,
                                            current,
                                            rate_change::Reason::Other,
                                        )
                                    }
                                };
                                s.rate_changes.lock().unwrap().record(
                                    &decision,
                                    &mut reconfigure,
                                    Instant::now(),
                                );
                                drop(reconfigure);
                                // Set before the answer goes out, later in this loop.
                                if let Some(applied) = decision.bitrate_kbps {
                                    butterpollo_core::stream_policy::report_bitrate(
                                        &s.launch.warnings,
                                        request.bitrate_kbps,
                                        applied,
                                        "max_bitrate and the 500 Mbps runtime cap",
                                    );
                                    s.set_client_bitrate(applied);
                                }
                                let result = decision.result;
                                tracing::info!(
                                    client = %s.launch.client.name,
                                    serial = request.serial,
                                    bitrate_kbps = request.bitrate_kbps,
                                    fps_millihz = request.fps_millihz,
                                    applied_kbps = result.bitrate_kbps,
                                    switch_to = ?decision.switch_to.map(|mode| mode.to_string()),
                                    bitrate = ?result.bitrate,
                                    fps = ?result.fps,
                                    "rate change"
                                );
                            }
                            0x0109 => {
                                if encrypted {
                                    s.stop();
                                }
                            }
                            0x0206 => {
                                let raw = if encrypted {
                                    Ok(payload)
                                } else {
                                    p.legacy.open(&s.launch.key, &payload)
                                };
                                let event = raw.and_then(|raw| input::decode(&raw));
                                match event {
                                    // Say once per kind that the device may not
                                    // send it: dropped silently, a phone's
                                    // controller looked undetected.
                                    Ok(event)
                                        if !s.launch.client.allows(event.required_permission()) =>
                                    {
                                        let permission = event.required_permission();
                                        if p.denied & permission == 0 {
                                            p.denied |= permission;
                                            let (code, message) =
                                                event.denied_warning(&s.launch.client.name);
                                            s.launch.warnings.set(&code, message);
                                        }
                                    }
                                    Ok(mut event) => {
                                        p.audio_haptics.input(
                                            &mut event,
                                            haptics_negotiated(&s.config, peer.channel_count()),
                                        );
                                        if let input::Input::Arrival { capabilities, .. } = &event
                                            && let Some(hint) =
                                                butterpollo_core::input_policy::steam_input_hint(
                                                    &s.launch.client.name,
                                                    *capabilities,
                                                )
                                        {
                                            s.launch.warnings.set("input_steam_input", hint);
                                        }
                                        if !queue_input(&mut p.inputs, event) {
                                            // Warnings::set logs it as well.
                                            s.launch.warnings.set("input_overflow", INPUT_OVERFLOW);
                                            s.stop();
                                        }
                                    }
                                    Err(e) => {
                                        if let Some(error) = first_steam_attributes_refusal(
                                            &mut p.steam_attributes_refused,
                                            &e,
                                        ) {
                                            tracing::warn!(client=%s.launch.client.name, %error, "ignoring Steam Controller attributes");
                                        } else {
                                            tracing::debug!(error=%e,"invalid input packet");
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
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
                #[cfg(test)]
                if h.reconnect_fixture.is_some() {
                    p.injector_tried = true;
                }
                let sessions = h.sessions.lock().unwrap();
                let s = sessions
                    .active
                    .get(&p.id)
                    .cloned()
                    .or_else(|| p.session.clone());
                let pending = sessions.pending.contains_key(&p.id);
                drop(sessions);
                if let Some(s) = s {
                    p.session = Some(s.clone());
                    let timed_out = p.seen.elapsed() > ping_timeout;
                    if s.stopping() || timed_out {
                        if timed_out && !s.stopping() {
                            tracing::warn!(client=%s.launch.client.name,"client control stream timed out");
                            s.fail();
                        }
                        host.socket_mut().roaming.set_stopping(&p.id);
                        // Its forwarded USB devices detach, and the client hears so before the
                        // termination.
                        for usb in h.usb.end_session(&p.id) {
                            let sent = usb
                                .encode()
                                .context("the USB message does not encode")
                                .and_then(|payload| {
                                    p.encrypt(
                                        &s,
                                        butterpollo_core::usb_forward::USB_MESSAGE_TYPE,
                                        &payload,
                                    )
                                })
                                .and_then(|message| {
                                    host.peer_mut(*peer_id)
                                        .send(0, &Packet::new(message, PacketKind::Reliable))
                                        .map_err(|e| anyhow::anyhow!("{e}"))
                                });
                            if let Err(e) = sent {
                                tracing::warn!(error = %format!("{e:#}"), ?usb, "USB message not sent");
                            }
                        }
                        let reason = s.termination_reason();
                        if let Ok(message) = p.encrypt(&s, 0x0109, &reason.to_be_bytes()) {
                            let peer = host.peer_mut(*peer_id);
                            let _ = peer.send(0, &Packet::new(message, PacketKind::Reliable));
                            peer.disconnect_later(0);
                        } else {
                            host.peer_mut(*peer_id).disconnect_now(0);
                        }
                        s.stop();
                        remove.push(*peer_id);
                        continue;
                    }
                    // A session takes roaming probes once its stream and ENet peer are up,
                    // and only if its DESCRIBE offered roaming.
                    if !p.roaming
                        && s.launch.roaming_offered.load(Ordering::Acquire)
                        && let Some(address) = host.peer(*peer_id).address()
                    {
                        host.socket_mut()
                            .roaming
                            .insert(&p.id, &s.launch.key, address);
                        p.roaming = true;
                    }
                    if !p.input_released && p.seen.elapsed() >= Duration::from_secs(1) {
                        #[cfg(test)]
                        if let Some(fixture) = &h.reconnect_fixture {
                            fixture.input_releases.fetch_add(1, Ordering::Release);
                        }
                        if let Some(injector) = &mut p.injector {
                            injector.release_all();
                        }
                        p.input_released = true;
                    }
                    // Sent once the encoder has the display's metadata, as
                    // Sunshine does, and again only if it changes. Moonlight
                    // for Xbox sets the TV's HDMI mode on every message, so an
                    // earlier one with placeholder values switched it twice
                    // (issue #11).
                    let metadata = s.hdr_metadata.read().unwrap().map(|m| m.wire(s.config.hdr));
                    if let Some(metadata) = metadata
                        && p.hdr_metadata != Some(metadata)
                        && let Ok(message) = p.encrypt(&s, 0x010e, &metadata)
                        && host
                            .peer_mut(*peer_id)
                            .send(0, &Packet::new(message, PacketKind::Reliable))
                            .is_ok()
                    {
                        p.hdr_metadata = Some(metadata);
                    }
                    // Present-to-sent reports for a client that asked (0x5536).
                    // Unreliable: a lost one costs the client those samples only.
                    loop {
                        let report = s.host_timings.lock().unwrap().take_report();
                        let Some(report) = report else { break };
                        if let Ok(message) = p.encrypt(
                            &s,
                            butterpollo_core::host_timing::HOST_TIMING_MESSAGE_TYPE,
                            &report,
                        ) {
                            let _ = host.peer_mut(*peer_id).send(
                                0,
                                &Packet::new(
                                    message,
                                    PacketKind::AlwaysUnreliable { sequenced: false },
                                ),
                            );
                        }
                    }
                    // Pipeline frame rates for a client that asked (0x553D); unreliable
                    // like the timing reports, as the next one replaces a lost one.
                    let rates = s
                        .pipeline_rates
                        .lock()
                        .unwrap()
                        .take(Instant::now(), || s.pipeline_counts());
                    if let Some(report) = rates
                        && let Ok(message) = p.encrypt(
                            &s,
                            butterpollo_core::pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE,
                            &report,
                        )
                    {
                        let _ = host.peer_mut(*peer_id).send(
                            0,
                            &Packet::new(
                                message,
                                PacketKind::AlwaysUnreliable { sequenced: false },
                            ),
                        );
                    }
                    if poll_feedback && let Some((old, new)) = s.tick_fec(Instant::now()) {
                        tracing::info!(
                            client = %s.launch.client.name,
                            old_percent = old,
                            new_percent = new,
                            "adaptive fec"
                        );
                    }
                    if poll_feedback
                        && let Some((old, new)) = s.tick_automatic_pacing(
                            Instant::now(),
                            host.peer(*peer_id).round_trip_time(),
                        )
                    {
                        tracing::info!(client = %s.launch.client.name, old_bps = old, new_bps = new, "automatic video burst pacing; encoder bitrate unchanged");
                    }
                    if poll_feedback
                        && let Some((old, new)) = s.tick_adaptive_bitrate(
                            Instant::now(),
                            host.peer(*peer_id).round_trip_time(),
                        )
                    {
                        tracing::info!(client = %s.launch.client.name, old_kbps = old, new_kbps = new, "host adaptive bitrate");
                    }
                    if poll_feedback {
                        s.tick_link_rate(Instant::now(), host.peer(*peer_id).round_trip_time());
                    }
                    // The bitrate notice for a client that asked (0x5538): reliable, so
                    // the overlay is not left stale, and at most one a second.
                    let notice = s.link_notices.lock().unwrap().take(Instant::now());
                    if let Some(notice) = notice {
                        match p.encrypt(
                            &s,
                            butterpollo_core::link_notice::LINK_NOTICE_MESSAGE_TYPE,
                            &notice,
                        ) {
                            Ok(message) => {
                                if let Err(e) = host
                                    .peer_mut(*peer_id)
                                    .send(0, &Packet::new(message, PacketKind::Reliable))
                                {
                                    tracing::warn!(error = %e, "link notice not sent");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), "link notice not encrypted");
                            }
                        }
                    }
                    // Rate change answers (0x553C), reliable and in order.
                    loop {
                        let Some(result) = s.rate_changes.lock().unwrap().take() else {
                            break;
                        };
                        match p.encrypt(
                            &s,
                            butterpollo_core::rate_change::RATE_CHANGE_MESSAGE_TYPE,
                            &result.encode(),
                        ) {
                            Ok(message) => {
                                if let Err(e) = host
                                    .peer_mut(*peer_id)
                                    .send(0, &Packet::new(message, PacketKind::Reliable))
                                {
                                    tracing::warn!(error = %e, serial = result.serial, "rate change answer not sent");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), serial = result.serial, "rate change answer not encrypted");
                            }
                        }
                    }
                    // The text field notice for a client that asked (0x553B): reliable,
                    // the newest focus at most every 50 ms.
                    let notice = s.text_input.lock().unwrap().take(Instant::now());
                    if let Some(notice) = notice {
                        match p.encrypt(
                            &s,
                            butterpollo_core::text_input::TEXT_INPUT_MESSAGE_TYPE,
                            &notice,
                        ) {
                            Ok(message) => {
                                if let Err(e) = host
                                    .peer_mut(*peer_id)
                                    .send(0, &Packet::new(message, PacketKind::Reliable))
                                {
                                    tracing::warn!(error = %e, "text input notice not sent");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), "text input notice not encrypted");
                            }
                        }
                    }
                    // The PC's newest copy for a device that syncs the clipboard (0x5508):
                    // reliable.
                    if let Some(frame) = p.clipboard_sync.take(Instant::now()) {
                        match p.encrypt(
                            &s,
                            butterpollo_core::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE,
                            &frame,
                        ) {
                            Ok(message) => {
                                if let Err(e) = host
                                    .peer_mut(*peer_id)
                                    .send(0, &Packet::new(message, PacketKind::Reliable))
                                {
                                    tracing::warn!(error = %e, "clipboard sync frame not sent");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), "clipboard sync frame not encrypted");
                            }
                        }
                    }
                    // The device-drawn cursor's shapes (0x5541): reliable, the newest at
                    // most every 16 ms.
                    let shape = s
                        .client_cursor
                        .take(p.cursor_owner, Instant::now(), |output| {
                            butterpollo_windows::display::output_scale(output)
                        });
                    if let Some(shape) = shape {
                        match p.encrypt(
                            &s,
                            butterpollo_core::client_cursor::CURSOR_SHAPE_MESSAGE_TYPE,
                            &shape,
                        ) {
                            Ok(message) => {
                                if let Err(e) = host
                                    .peer_mut(*peer_id)
                                    .send(0, &Packet::new(message, PacketKind::Reliable))
                                {
                                    tracing::warn!(error = %e, "cursor shape not sent");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), "cursor shape not encrypted");
                            }
                        }
                    }
                    // USB forwarding's answers and detaches (0x553A), reliable and in order.
                    for usb in h.usb.take(&p.id, Instant::now()) {
                        let sent = usb
                            .encode()
                            .context("the USB message does not encode")
                            .and_then(|payload| {
                                p.encrypt(
                                    &s,
                                    butterpollo_core::usb_forward::USB_MESSAGE_TYPE,
                                    &payload,
                                )
                            })
                            .and_then(|message| {
                                host.peer_mut(*peer_id)
                                    .send(0, &Packet::new(message, PacketKind::Reliable))
                                    .map_err(|e| anyhow::anyhow!("{e}"))
                            });
                        if let Err(e) = sent {
                            tracing::warn!(error = %format!("{e:#}"), ?usb, "USB message not sent");
                        }
                    }
                    // A co-op player's input drives its slot's pad and never
                    // the desktop: it gets no injector.
                    if s.launch.nucleus.is_some() && !p.coop_tried {
                        p.coop_tried = true;
                        p.coop = effective_config(&h, &s.launch)
                            .inspect_err(|e| tracing::warn!(error = %format!("{e:#}"), "co-op input configuration unavailable"))
                            .ok()
                            .and_then(|c| crate::nucleus::Player::of(&h, &s.launch, &c));
                        if p.coop.is_none() {
                            tracing::warn!(client = %s.launch.client.name, "co-op input has no Nucleus Co-op session: this player's input is dropped");
                        }
                    }
                    // Made before input comes; after a failure, again when it
                    // does. Making it with the first input held that input up.
                    if s.launch.nucleus.is_none()
                        && p.injector.is_none()
                        && (!p.inputs.is_empty() || !p.injector_tried)
                    {
                        p.injector_tried = true;
                        let c = match effective_config(&h, &s.launch) {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), "input configuration unavailable");
                                p.inputs.clear();
                                continue;
                            }
                        };
                        let output = s.output.read().unwrap().clone();
                        let pads = match h.controller_sessions.acquire(&s.launch, &c) {
                            Ok(pads) => pads,
                            Err(error) => {
                                s.launch.warnings.set("input_initialization", format!("Controller recovery is still in progress ({error:#}); retrying."));
                                drop_uninjectable_input(&mut p.inputs);
                                p.injector_tried = false;
                                continue;
                            }
                        };
                        match Injector::new_options_reported(
                            if output.is_empty() {
                                c.get("output_name", "")
                            } else {
                                &output
                            },
                            c.get("gamepad", "auto"),
                            &c,
                            s.launch.warnings.clone(),
                        ) {
                            Ok(mut i) => {
                                i.use_gamepads(pads);
                                // The client can announce USB audio before input
                                // initialization finishes. Apply its scoped flags
                                // to the acquired (possibly retained) devices now.
                                for (id, flags) in p.controller_audio.iter().copied().enumerate() {
                                    if flags != 0 {
                                        i.controller_audio(id as u16, controller_audio::CONTROL,
                                            &[1, 1, id as u8, 0, flags, 0, 0, 0]);
                                    }
                                }
                                s.launch.warnings.clear("input_initialization");
                                i.set_feedback_waker(self.control_wake.clone());
                                i.set_stream_size(s.config.width, s.config.height);
                                p.injector = Some(i);
                                p.keyboard_layout.refresh();
                            }
                            Err(e) => s.launch.warnings.set("input_initialization", format!("Input initialization failed ({e:#}); controls are unavailable. Check Input settings and the selected display, then reconnect.")),
                        }
                    }
                    // The keyboard layout follows saves of `keyboard_layout` and a coalesced
                    // report; it is read here, and the injector only takes it.
                    p.keyboard_layout.follow(
                        h.config_version.load(Ordering::Acquire),
                        &s.launch.warnings,
                        Instant::now(),
                        || *s.keyboard_layout.lock().unwrap(),
                        || effective_config(&h, &s.launch),
                    );
                    if let Some(i) = &mut p.injector {
                        // The stream's display can be created or renamed after
                        // input began; absolute input follows it.
                        i.set_output(&s.output.read().unwrap());
                        // So does the stream's size, which the client can change (0x5532).
                        let mode = s.stream_mode();
                        i.set_stream_size(mode.width, mode.height);
                        // And keys the device's keyboard layout (0x553E).
                        if let Some(layout) = p.keyboard_layout.take() {
                            i.set_keyboard_layout(layout);
                        }
                        i.follow_keyboard_layout();
                    }
                    p.audio_haptics.follow(
                        h.config_version.load(Ordering::Acquire),
                        &s.launch.warnings,
                        || effective_config(&h, &s.launch),
                    );
                    if p.couch.follow(&s, || effective_config(&h, &s.launch)) {
                        let wake = self.control_wake.clone();
                        s.couch_menu.set_control_wake(move || {
                            let _ = wake.send(&[1]);
                        });
                    }
                    // Nor the couch menu, which the PC's display would show.
                    if s.launch.nucleus.is_some() {
                        let events = std::mem::take(&mut p.inputs);
                        if let Some(player) = &mut p.coop {
                            for event in events {
                                player.input(event, &s.launch.warnings);
                            }
                        }
                    }
                    // The couch menu takes what it needs; the game gets the rest.
                    let batch = crate::couch_menu::apply_batch(
                        &mut p.couch,
                        p.injector.as_mut(),
                        &mut p.inputs,
                        Instant::now(),
                    );
                    if let Some(reason) = batch.refused {
                        tracing::warn!(client = %s.launch.client.name, "{reason}");
                        s.launch.warnings.event(
                            "couch_menu",
                            reason,
                            butterpollo_core::session::EVENT_PERIOD,
                        );
                    }
                    menus.send(&h, &s, batch.nav);
                    for e in batch.errors {
                        s.launch.warnings.event("input_injection", format!("Input injection failed ({e:#}); keyboard, mouse, touch or pen actions may be missing. Unlock the desktop and check Windows input permissions or disable native touch/pen if unsupported."), butterpollo_core::session::EVENT_PERIOD);
                    }
                    p.inputs.clear();
                    if !poll_feedback
                        && let Some(i) = &mut p.injector
                        && let Err(e) = i.due()
                    {
                        s.launch.warnings.event("input_repeat", format!("Input release or repeat failed ({e:#}); held controls may not update. Unlock the desktop or reconnect."), butterpollo_core::session::EVENT_PERIOD);
                    }
                    if poll_feedback
                        && let Some(i) = &mut p.injector
                        && let Err(e) = i.refresh()
                    {
                        s.launch.warnings.event("input_pointer", format!("Touch/pen pointer refresh failed ({e:#}); pointer updates may be missing. Unlock the desktop or disable native touch/pen if Windows does not support it."), butterpollo_core::session::EVENT_PERIOD);
                    }
                    // The gamepad thread polls feedback every 8 ms; pass on
                    // what it found, and the sensors an arrived pad has.
                    let sends = match (&p.injector, &p.coop) {
                        (Some(i), _) => pad_reports(
                            i.gamepad_reports(),
                            |kind| i.feedback_allowed(kind),
                            haptics_negotiated(&s.config, host.peer(*peer_id).channel_count()),
                            &p.controller_audio,
                            &mut p.controller_pcm,
                            &mut p.audio_haptics.arbiter,
                            &mut p.rumble,
                        ),
                        (None, Some(player)) if poll_feedback => pad_reports(
                            player.feedback(&h),
                            |kind| player.feedback_allowed(kind),
                            haptics_negotiated(&s.config, host.peer(*peer_id).channel_count()),
                            &p.controller_audio,
                            &mut p.controller_pcm,
                            &mut p.audio_haptics.arbiter,
                            &mut p.rumble,
                        ),
                        _ => PadSends::default(),
                    };
                    s.reserve_controller_pcm(p.controller_pcm.streams(Instant::now()));
                    for (kind, payload) in sends.waveforms {
                        p.send_waveform(&mut host, *peer_id, &s, kind, &payload);
                    }
                    for (kind, payload) in sends.messages {
                        if let Ok(message) = p.encrypt(&s, kind, &payload) {
                            let _ = host.peer_mut(*peer_id).send(
                                FEEDBACK_CHANNEL,
                                &Packet::new(message, PacketKind::Reliable),
                            );
                        }
                    }
                } else if !pending || p.seen.elapsed() > ping_timeout {
                    drop(h.usb.end_session(&p.id));
                    host.peer_mut(*peer_id).disconnect_now(0);
                    remove.push(*peer_id);
                }
            }
            host.socket_mut().release()?;
            // Release held input now; gamepad workers unplug their pads asynchronously.
            drop(disconnected);
            peers_went |= !remove.is_empty();
            for peer in remove {
                peers.remove(&peer);
            }
            // Haptics from game audio, for the waveform controllers the game
            // leaves without haptics or rumble: after the input was applied
            // and the acks went out, so the analysis delays neither.
            for (peer_id, p) in &mut peers {
                let (Some(s), Some(i)) = (p.session.clone(), &p.injector) else {
                    continue;
                };
                let allowed = i.feedback_allowed(haptics::PACKET_TYPE)
                    && haptics_negotiated(&s.config, host.peer(*peer_id).channel_count());
                let payloads = p.audio_haptics.generate(
                    allowed,
                    &s.audio_haptics,
                    &mut p.controller_pcm,
                    Instant::now(),
                );
                for payload in payloads {
                    p.send_waveform(&mut host, *peer_id, &s, haptics::PACKET_TYPE, &payload);
                }
            }
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
            if peers_went && !self.roam_seqs.is_empty() {
                // An ended launch's key never returns.
                let sessions = h.sessions.lock().unwrap();
                self.roam_seqs.retain_launches(
                    sessions
                        .active
                        .values()
                        .map(|s| &s.launch)
                        .chain(sessions.pending.values())
                        .map(|l| &l.key),
                );
            }
            menus.retain(|id| {
                peers
                    .values()
                    .any(|p| p.id == id && p.session.as_ref().is_some_and(|s| !s.stopping()))
            });
            // A session whose client never connected control, or whose peer
            // this worker lost when it restarted, would otherwise hold the
            // encoder, capture and display forever (Sunshine times it out).
            active_sessions.extend(h.sessions.lock().unwrap().active.values().cloned());
            unattended.retain(|id, _| active_sessions.iter().any(|s| &s.launch.id == id));
            end_unattended_sessions(
                &mut active_sessions,
                |id| peers.values().any(|p| p.id == id),
                &mut unattended,
                ping_timeout,
                &socket,
            );
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
            let timeout = control_wait_timeout(due.saturating_duration_since(now), serviced);
            if butterpollo_windows::net::wait_readable_or_wake(raw_socket, raw_wake, timeout)
                .is_err()
            {
                // A polling error must not spin the loop.
                input_timer.until(now + Duration::from_millis(timeout.max(1) as u64));
            }
        }
        for (peer, p) in peers {
            host.peer_mut(peer).disconnect_now(0);
            drop(p);
        }
        Ok(())
    }
}

/// Carries out a client's validated move (`roaming`): the session's media follow the
/// client to its new address, ENet checks the path at once, and the client hears MIGRATED.
fn follow_roam(
    h: &Shared,
    media: &Media,
    host: &mut Host<ControlSocket>,
    peers: &HashMap<PeerID, ControlPeer>,
    socket: &ReplySocket,
    commit: roaming::Commit,
) {
    let mut stale = Vec::new();
    let session = {
        // The ping thread's lock order. Holding the sessions lock until the session knows
        // its new path keeps a new-path ping from landing between the two: its peer would be
        // dropped below after it took the new path, and with it the keyframe.
        let sessions = h.sessions.lock().unwrap();
        let session = sessions.active.get(&commit.id).cloned();
        if let Some(s) = &session {
            // The old path's media addresses are stale; the first pings from the new path
            // replace them.
            stale.extend(media.peers.lock().unwrap().remove(&commit.id));
            s.roamed(commit.new.ip(), Instant::now());
        }
        session
    };
    for (audio, peer) in stale {
        if audio {
            media.audio.forget(peer);
        } else {
            media.video.forget(peer);
        }
    }
    if let Some((peer_id, _)) = peers.iter().find(|(_, p)| p.id == commit.id) {
        host.peer_mut(*peer_id).ping();
    }
    roaming::send(socket, &commit.migrated, commit.new);
    tracing::info!(
        session = %commit.id,
        client = session.as_ref().map_or("", |s| s.launch.client.name.as_str()),
        media_rebound = commit.media_rebound,
        "client roamed {} -> {}",
        roaming::canonical(commit.old),
        roaming::canonical(commit.new)
    );
}

pub(super) struct ControlPeer {
    #[cfg(test)]
    _fixture_input: Option<reconnect_tests::Slot>,
    pub(super) id: String,
    injector: Option<Injector>,
    /// Whether making the injector was tried.
    injector_tried: bool,
    /// A co-op player's slot, which takes its input instead of an injector.
    coop: Option<crate::nucleus::Player>,
    /// Whether finding the co-op player was tried.
    coop_tried: bool,
    sequence: u32,
    received: butterpollo_core::packet::ReplayWindow,
    hdr_metadata: Option<[u8; 27]>,
    legacy: butterpollo_core::packet::LegacyInput,
    seen: Instant,
    input_released: bool,
    session: Option<Arc<Session>>,
    inputs: Vec<input::Input>,
    /// Input permissions this device lacked and was warned about.
    denied: u32,
    /// Whether this device was warned about refused Steam Controller attributes.
    steam_attributes_refused: bool,
    controller_audio: [u8; 16],
    controller_pcm: controller_audio::PcmTraffic,
    rumble: RumbleForwarded,
    command_at: Option<Instant>,
    /// Whether the session is in the control socket's roaming table.
    roaming: bool,
    /// The couch menu's filter for this peer's session.
    couch: crate::couch_menu::PeerMenu,
    /// Haptics from game audio for this peer's controllers.
    audio_haptics: audio_haptics::PeerHaptics,
    /// The keyboard layout keys go with (0x553E, `keyboard_layout`).
    keyboard_layout: keyboard_layout::PeerLayout,
    /// This peer's index, which owns a client-drawn cursor session it enabled.
    cursor_owner: usize,
    /// The text field watcher, from the client's first text input request (0x553B); paused
    /// while the client turned it off, and dropped with the peer at session end.
    focus_watch: Option<butterpollo_windows::text_focus::FocusWatch>,
    /// Clipboard sync (0x5508), from the device's first frame; its worker ends with the peer.
    clipboard_sync: clipboard_sync::PeerSync,
}

impl Drop for ControlPeer {
    fn drop(&mut self) {
        if let Some(injector) = &mut self.injector {
            injector.detach_gamepads();
        }
        // The cursor goes back into the video when its device leaves.
        if let Some(s) = &self.session {
            s.client_cursor.end(self.cursor_owner);
        }
    }
}

impl ControlPeer {
    /// Starts, pauses or resumes the text field watcher after a client's text input request
    /// (0x553B): one thread per peer however often the client toggles. The watcher notes the
    /// newest focus in the session and wakes this loop, which sends the notice; one that was
    /// paused or stopped reports nothing more.
    fn watch_focus(&mut self, s: &Arc<Session>, watch: TextWatch, wake: &Arc<UdpSocket>) {
        if let Some(watcher) = &self.focus_watch {
            watcher.set(watch);
            return;
        }
        let TextWatch::On { max_text } = watch else {
            return;
        };
        let session = Arc::downgrade(s);
        let failed = session.clone();
        let wake = wake.clone();
        let started = butterpollo_windows::text_focus::FocusWatch::start(
            max_text,
            move |focused| {
                if let Some(s) = session.upgrade() {
                    s.text_input.lock().unwrap().note(focused, Instant::now());
                    let _ = wake.send(&[1]);
                }
            },
            move |error| {
                if let Some(s) = failed.upgrade() {
                    focus_watch_failed(&s, &error);
                }
            },
        );
        match started {
            Ok(watcher) => self.focus_watch = Some(watcher),
            Err(error) => focus_watch_failed(s, &error),
        }
    }
    /// Speaker and haptics waveforms go unreliable on their own channel: a
    /// late one is useless.
    fn send_waveform(
        &mut self,
        host: &mut Host<ControlSocket>,
        peer: PeerID,
        s: &Session,
        kind: u16,
        payload: &[u8],
    ) {
        if let Ok(message) = self.encrypt(s, kind, payload) {
            let _ = host.peer_mut(peer).send(
                haptics::CHANNEL,
                &Packet::new(message, PacketKind::AlwaysUnreliable { sequenced: false }),
            );
        }
    }
    fn encrypt(&mut self, s: &Session, kind: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let seq = self.sequence;
        self.sequence = seq.checked_add(1).context("control nonce exhausted")?;
        butterpollo_core::packet::encrypted_control(
            &s.launch.key,
            seq,
            kind,
            payload,
            s.config.encryption & 1 != 0,
        )
    }
}

/// Says on the stream card that the device's keyboard cannot follow text fields.
fn focus_watch_failed(s: &Session, error: &anyhow::Error) {
    tracing::warn!(client = %s.launch.client.name, error = %format!("{error:#}"), "focus watcher failed");
    s.launch.warnings.set(
        "text_input",
        format!(
            "The device's keyboard cannot follow text fields: the focus watcher failed ({error:#}). Type with the device's keyboard button instead."
        ),
    );
}

pub(super) const MOTION_EVENT_REQUEST: u16 = 0x5501;

pub(crate) const RUMBLE_TRIGGER_DATA: u16 = 0x5500;

pub(crate) const RUMBLE_DATA: u16 = 0x010b;

pub(super) const RUMBLE_MARKER: u32 = 0x00c0ffee;

// Channel 0 is available even when an older Moonlight peer negotiates only
// one channel. Sending on channel 1 then fails with InvalidChannel.
pub(super) const FEEDBACK_CHANNEL: u8 = 0;

/// The refused Steam Controller attributes to warn about: only the first per
/// device, since a client that sends them malformed does so on every arrival.
/// Other invalid input stays a debug line.
pub(super) fn first_steam_attributes_refusal(
    warned: &mut bool,
    error: &anyhow::Error,
) -> Option<input::SteamAttributesError> {
    let refusal = *error.downcast_ref::<input::SteamAttributesError>()?;
    (!std::mem::replace(warned, true)).then_some(refusal)
}

/// Decoded input waiting for the next injection pass.
pub(super) const INPUT_QUEUE: usize = 512;

pub(super) const INPUT_OVERFLOW: &str = "Input queue overflow: more than 512 input events waited for injection, so the stream was stopped. Reconnect to continue.";

/// Queues `event` for the next pass, merged into the last one where it can
/// be. False when the queue is full: the caller stops the stream with
/// `INPUT_OVERFLOW`.
pub(super) fn queue_input(inputs: &mut Vec<input::Input>, event: input::Input) -> bool {
    if inputs
        .last_mut()
        .is_some_and(|last| last.merge(&event) == input::Batch::Merged)
    {
        return true;
    }
    if inputs.len() >= INPUT_QUEUE {
        return false;
    }
    inputs.push(event);
    true
}

/// While the controllers can't be acquired there is nothing to inject into:
/// keys, clicks and moves are dropped rather than replayed in one late burst
/// when acquire succeeds. State the client sends once stays: arrivals, so
/// the pads keep the kind the client announced, the haptics switch, Steam
/// attributes, and each pad's latest state, whose active mask removes pads.
pub(super) fn drop_uninjectable_input(inputs: &mut Vec<input::Input>) {
    let mut latest: Vec<(u16, usize)> = Vec::new();
    for (at, event) in inputs.iter().enumerate().rev() {
        if let input::Input::Controller { id, .. } = event
            && !latest.iter().any(|(pad, _)| pad == id)
        {
            latest.push((*id, at));
        }
    }
    let mut at = 0;
    inputs.retain(|event| {
        let keep = match event {
            input::Input::Arrival { .. }
            | input::Input::Haptics(_)
            | input::Input::SteamAttributes(_) => true,
            input::Input::Controller { .. } => latest.iter().any(|(_, kept)| *kept == at),
            _ => false,
        };
        at += 1;
        keep
    });
}

/// What a pass sends for the gamepad thread's reports: feedback messages
/// (reliable) and speaker and haptics waveforms (the haptics channel).
#[derive(Default)]
pub(super) struct PadSends {
    pub(super) messages: Vec<(u16, Vec<u8>)>,
    pub(super) waveforms: Vec<(u16, Vec<u8>)>,
}

/// The game's own haptics and rumble reach `arbiter` here, before the same
/// pass generates haptics from game audio, so it already leaves those
/// controllers alone.
pub(super) fn pad_reports(
    reports: impl IntoIterator<Item = PadReport>,
    feedback_allowed: impl Fn(u16) -> bool,
    negotiated: bool,
    controller_audio: &[u8; 16],
    pcm: &mut controller_audio::PcmTraffic,
    arbiter: &mut butterpollo_core::audio_haptics::Arbiter,
    rumble: &mut RumbleForwarded,
) -> PadSends {
    let mut sends = PadSends::default();
    for report in reports {
        match report {
            PadReport::Speaker(frame) => {
                if controller_audio
                    .get(usize::from(frame.id))
                    .is_some_and(|flags| flags & controller_audio::PLAYBACK != 0)
                    && frame.created.elapsed() <= haptics::MAX_AGE
                    && let Some(payload) = frame.payload()
                {
                    pcm.observe(frame.id, controller_audio::SPEAKER, Instant::now());
                    sends.waveforms.push((controller_audio::SPEAKER, payload));
                }
            }
            PadReport::Haptics(mut frame) => {
                if feedback_allowed(haptics::PACKET_TYPE)
                    && negotiated
                    && frame.created.elapsed() <= haptics::MAX_AGE
                    && arbiter.game_frame(&mut frame, Instant::now())
                    && let Some(payload) = frame.payload()
                {
                    pcm.observe(frame.id, haptics::PACKET_TYPE, Instant::now());
                    sends.waveforms.push((haptics::PACKET_TYPE, payload));
                }
            }
            PadReport::Feedback { id, kind, data } => {
                let packets = feedback_packets(id, kind, &data);
                for (kind, payload) in &packets {
                    arbiter.game_feedback(*kind, payload, Instant::now());
                }
                sends
                    .messages
                    .extend(packets.into_iter().filter(|(kind, payload)| {
                        rumble.forward(feedback_allowed(*kind), *kind, payload)
                    }))
            }
            PadReport::Motion { id, capabilities } => {
                sends.messages.extend(motion_requests(id, capabilities))
            }
        }
    }
    sends
}

/// Ask the client for an arrived pad's accelerometer (kind 1) and gyroscope
/// (kind 2) at 200 Hz, when it has them.
pub(super) fn motion_requests(id: u8, capabilities: u16) -> Vec<(u16, Vec<u8>)> {
    [(0x10, 1), (0x20, 2)]
        .into_iter()
        .filter(|(sensor, _)| capabilities & sensor != 0)
        .map(|(_, kind)| {
            let mut payload = u16::from(id).to_le_bytes().to_vec();
            payload.extend_from_slice(&200u16.to_le_bytes());
            payload.push(kind);
            (MOTION_EVENT_REQUEST, payload)
        })
        .collect()
}

pub(super) fn haptics_negotiated(
    config: &butterpollo_core::rtsp::Negotiated,
    channels: usize,
) -> bool {
    config.encryption & 1 != 0
        && config.moonlight_features & haptics::FEATURE != 0
        && channels > usize::from(haptics::CHANNEL)
}

/// Whether each controller's main and trigger motors ran in the last rumble
/// this client was sent.
#[derive(Default)]
pub(super) struct RumbleForwarded {
    running: [[bool; 2]; 16],
}

impl RumbleForwarded {
    /// Whether a feedback packet goes out, given whether the injector allows
    /// its kind. With rumble off, a stop still goes out once for motors the
    /// client was last told to run, so they don't keep buzzing; other stops
    /// stay blocked, as VHF repeats them with every lightbar or trigger change.
    pub(super) fn forward(&mut self, allowed: bool, kind: u16, payload: &[u8]) -> bool {
        let (motor, id, strengths) = match kind {
            RUMBLE_DATA if payload.len() == 10 => (0, &payload[4..6], &payload[6..]),
            RUMBLE_TRIGGER_DATA if payload.len() == 6 => (1, &payload[..2], &payload[2..]),
            _ => return allowed,
        };
        let id = usize::from(u16::from_le_bytes([id[0], id[1]]));
        let Some(running) = self.running.get_mut(id).map(|motors| &mut motors[motor]) else {
            return allowed;
        };
        let runs = strengths.iter().any(|byte| *byte != 0);
        if allowed {
            *running = runs;
            true
        } else if !runs && *running {
            *running = false;
            true
        } else {
            false
        }
    }
}

pub(crate) fn feedback_packets(id: u16, kind: u16, data: &[u8]) -> Vec<(u16, Vec<u8>)> {
    if kind == controller_audio::CONTROL && data.len() == 12 {
        return vec![(kind, data.to_vec())];
    }
    if (kind == RUMBLE_DATA && data.len() == 4)
        || (kind == 0x5502 && data.len() == 3)
        || (kind == 0x5503 && data.len() == 23)
    {
        let mut payload = if kind == RUMBLE_DATA {
            RUMBLE_MARKER.to_le_bytes().to_vec()
        } else {
            vec![]
        };
        payload.extend_from_slice(&id.to_le_bytes());
        payload.extend_from_slice(data);
        return vec![(kind, payload)];
    }
    let mut result = vec![];
    if matches!(kind, 4 | 5) && data.len() >= 8 {
        let mut rumble = RUMBLE_MARKER.to_le_bytes().to_vec();
        rumble.extend_from_slice(&id.to_le_bytes());
        rumble.extend_from_slice(&data[..4]);
        result.push((RUMBLE_DATA, rumble));
        if kind == 4 {
            result.push((
                RUMBLE_TRIGGER_DATA,
                [&id.to_le_bytes()[..], &data[4..8]].concat(),
            ));
        } else if kind == 5 && data[7] & 1 != 0 {
            result.push((0x5502, [&id.to_le_bytes()[..], &data[4..7]].concat()));
        }
        if kind == 5 && data.len() == 32 && data[7] & 2 != 0 {
            // moonlight-common-c: id, event flags (0x08 left, 0x04 right),
            // left type, right type, then each trigger's ten parameters.
            // The driver reports both triggers at once.
            let mut trigger = id.to_le_bytes().to_vec();
            trigger.extend_from_slice(&[0x0c, data[10], data[21]]);
            trigger.extend_from_slice(&data[11..21]);
            trigger.extend_from_slice(&data[22..32]);
            result.push((0x5503, trigger));
        }
    }
    result
}
