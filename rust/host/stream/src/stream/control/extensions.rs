//! Rubylight control extension handlers: Frame Warp, USB forwarding, display capabilities,
//! stream reconfigure, rate changes, and server commands.

use super::super::*;
use super::peer::ControlPeer;
use super::socket::ControlSocket;

pub(in crate::stream) fn server_command(
    h: &Shared,
    client: &butterpollo_session::state::Client,
    payload: &[u8],
    authenticated: bool,
    command_at: &mut Option<Instant>,
) -> Option<crate::process::MenuRun> {
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
    crate::process::pc_command(h, payload[0] as usize, None)
}

/// Dispatches extension control packets (server commands, Frame Warp, USB, display caps,
/// reconfigure, rate change). Returns `true` if `kind` was handled.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_extension_packet(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    commands: &std::sync::mpsc::SyncSender<(Arc<Session>, crate::process::MenuRun)>,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    kind: u16,
    payload: &[u8],
    encrypted: bool,
) -> bool {
    match kind {
        0x3000 => {
            if let Some(run) =
                server_command(h, &s.launch.client, payload, encrypted, &mut p.command_at)
                && let Err(e) = commands.try_send((s.clone(), run))
            {
                tracing::warn!(error=%e, "configured server command failed");
            }
        }
        // Quick Suspend (0x554B): the work runs on a worker of its own.
        rubylight_protocol::quick_suspend::QUICK_SUSPEND_MESSAGE_TYPE => {
            super::quick_suspend::request(h, control_wake, p, s, payload, encrypted);
        }
        // Frame Warp (0x5543): while offered, the host learns the game's
        // camera response and tags every frame for a client that asked;
        // the pass below sends the model, mask, tags and metas. The host runs
        // Frame Warp 2 only: an older Nightjar's version 1 request is ignored
        // and gets nothing back.
        butterpollo_media_policy::frame_warp::FRAME_WARP_REQUEST_MESSAGE_TYPE => {
            let Ok(config) = p.config(h, s) else {
                return true;
            };
            if !crate::extensions::offered(h, &config).frame_warp {
                tracing::debug!(len = payload.len(), "unoffered frame warp request");
                return true;
            }
            let was = s.frame_warp.enabled();
            let wake = control_wake.clone();
            s.frame_warp.set_wake(Arc::new(move || {
                let _ = wake.send(&[1]);
            }));
            match s.frame_warp.on_request(payload) {
                Some(enabled) => {
                    if enabled != was {
                        tracing::info!(
                            client = %s.launch.client.name,
                            enabled,
                            "frame warp"
                        );
                    }
                    let _ = control_wake.send(&[1]);
                }
                None if s.frame_warp.first_ignored() => tracing::info!(
                    client = %s.launch.client.name,
                    len = payload.len(),
                    version = payload.first().copied(),
                    "frame warp request ignored: not Frame Warp 2 (an older Nightjar?)"
                ),
                None => tracing::debug!(len = payload.len(), "invalid frame warp request"),
            }
        }
        // The registry checks the setting and the device's permission and
        // queues the answer, which the pass below sends.
        rubylight_protocol::usb::USB_MESSAGE_TYPE if encrypted => {
            match rubylight_protocol::usb::UsbMessage::decode(payload) {
                Some(message) => {
                    let (enabled, limit) = {
                        let config = h.config.read().unwrap();
                        // Never a guest's but one with keyboard and mouse.
                        (
                            config.usb_forwarding() && h.device_guests.full_access(s),
                            config.usb_max_devices(),
                        )
                    };
                    h.usb
                        .on_message(&p.id, &s.launch.client, enabled, limit, message);
                }
                None => {
                    tracing::debug!(len = payload.len(), "invalid USB message")
                }
            }
        }
        butterpollo_media_policy::display_caps::DISPLAY_CAPS_MESSAGE_TYPE => {
            match s.record_display_caps(payload) {
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
                None => tracing::debug!(len = payload.len(), "invalid display caps"),
            }
        }
        // The session loop applies it once the client stops sending.
        butterpollo_media_policy::reconfigure::RECONFIGURE_MESSAGE_TYPE => {
            let outcome = {
                let mut reconfigure = s.reconfigure.lock().unwrap();
                let outcome = reconfigure.on_payload(Instant::now(), payload);
                s.rate_changes.on_reconfigure(outcome);
                outcome
            };
            tracing::debug!(?outcome, len = payload.len(), "reconfigure request");
        }
        // A bitrate applies now, without a keyframe; a rate switches
        // like a 0x5532 request and is answered once it runs.
        butterpollo_media_policy::rate_change::RATE_CHANGE_MESSAGE_TYPE => {
            handle_rate_change(h, p, s, payload);
        }
        _ => return false,
    }
    true
}

fn handle_rate_change(h: &Shared, p: &mut ControlPeer, s: &Arc<Session>, payload: &[u8]) {
    use butterpollo_media_policy::rate_change;
    if !matches!(rate_change::launch_choice(&s.launch.options), Ok(true)) {
        tracing::debug!("rate change not offered");
        return;
    }
    let Some(request) = rate_change::RateChangeRequest::decode(payload) else {
        tracing::warn!(len = payload.len(), "invalid rate change request");
        return;
    };
    let config = p.config(h, s);
    let mut reconfigure = s.reconfigure.lock().unwrap();
    let requested = reconfigure.requested();
    let current = reconfigure.current().fps_millihz;
    // In the client's unit, as its request is: the bitrate its
    // user set, where the launch carried one.
    let bitrate_kbps = s
        .rate_changes
        .client_bitrate_kbps(s.bitrate.load(Ordering::Acquire), &s.config);
    let decision = match config {
        Ok(config) => {
            let enabled = config::stream_reconfigure_enabled(&config);
            let mb_rate = h
                .encoder_mb_rates
                .get(usize::from(s.config.codec))
                .map_or(0, |rate| rate.load(Ordering::Acquire));
            let stream = rate_change::Stream {
                bitrate_kbps,
                launched: &s.config,
                fps_millihz: current,
                requested,
                fps_ceiling_millihz: rate_change::fps_ceiling_millihz(
                    mb_rate,
                    requested.width,
                    requested.height,
                    current,
                    s.config.fps_millihz(),
                ),
                fps_refusal: butterpollo_media_policy::reconfigure::refusal(&s.config, enabled)
                    .map(|_| rate_change::refusal_reason(enabled)),
            };
            rate_change::decide(&request, &stream, &config)
        }
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), serial = request.serial, "rate change refused: the device's configuration did not load");
            rate_change::refuse(&request, bitrate_kbps, current, rate_change::Reason::Other)
        }
    };
    s.rate_changes
        .record(&decision, &mut reconfigure, Instant::now());
    drop(reconfigure);
    // Set before the answer goes out, later in this loop.
    if let Some(encoder_kbps) = decision.bitrate_kbps {
        // Asked and applied in the client's unit: the encoder's
        // share of it is no cap.
        butterpollo_media_policy::stream_policy::report_bitrate(
            &s.launch.warnings,
            request.bitrate_kbps,
            decision.result.bitrate_kbps,
            "max_bitrate and the 500 Mbps runtime cap",
        );
        s.set_client_bitrate(encoder_kbps);
    }
    let result = decision.result;
    if request.is_query() {
        tracing::debug!(
            client = %s.launch.client.name,
            serial = request.serial,
            bitrate_kbps = result.bitrate_kbps,
            fps_millihz = result.fps_millihz,
            "rate query"
        );
    } else {
        tracing::info!(
            client = %s.launch.client.name,
            serial = request.serial,
            bitrate_kbps = request.bitrate_kbps,
            fps_millihz = request.fps_millihz,
            applied_kbps = result.bitrate_kbps,
            encoder_kbps = ?decision.bitrate_kbps,
            switch_to = ?decision.switch_to.map(|mode| mode.to_string()),
            bitrate = ?result.bitrate,
            fps = ?result.fps,
            "rate change"
        );
    }
}

/// Sends queued Frame Warp updates, rate-change answers, and USB forwarding messages for
/// `peer_id`.
pub(super) fn send_extension_updates(
    h: &Shared,
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
) {
    // Frame Warp for a client that asked (0x5543): the model and the HUD
    // mask reliably, then the frames' tags unreliably, as each tags
    // message repeats the three frames before it, and each frame's meta
    // (0x5549) unreliably after its tags. Nothing here waits for the host.
    if s.frame_warp.enabled() {
        let outgoing = s.frame_warp.take_outgoing();
        let reliable = [
            outgoing.model.map(|model| {
                (
                    butterpollo_media_policy::frame_warp::FRAME_WARP_MODEL_MESSAGE_TYPE,
                    model.to_vec(),
                )
            }),
            outgoing.mask.map(|mask| {
                (
                    butterpollo_media_policy::frame_warp::FRAME_WARP_MASK_MESSAGE_TYPE,
                    mask,
                )
            }),
        ];
        for (kind, payload) in reliable.into_iter().flatten() {
            if let Ok(message) = p.encrypt(s, kind, &payload) {
                let _ = host
                    .peer_mut(peer_id)
                    .send(0, &Packet::new(message, PacketKind::Reliable));
            }
        }
        for tags in outgoing.tags {
            if let Ok(message) = p.encrypt(
                s,
                butterpollo_media_policy::frame_warp::FRAME_WARP_TAGS_MESSAGE_TYPE,
                &tags,
            ) {
                let _ = host.peer_mut(peer_id).send(
                    0,
                    &Packet::new(message, PacketKind::AlwaysUnreliable { sequenced: false }),
                );
            }
        }
        for meta in outgoing.metas {
            if let Ok(message) = p.encrypt(
                s,
                butterpollo_media_policy::frame_warp::FRAME_WARP_V2_META_MESSAGE_TYPE,
                &meta,
            ) {
                let _ = host.peer_mut(peer_id).send(
                    0,
                    &Packet::new(message, PacketKind::AlwaysUnreliable { sequenced: false }),
                );
            }
        }
    }
    // Rate change answers (0x553C), reliable and in order.
    if s.rate_changes.queued() {
        while let Some(result) = s.rate_changes.take() {
            match p.encrypt(
                s,
                butterpollo_media_policy::rate_change::RATE_CHANGE_MESSAGE_TYPE,
                &result.encode(),
            ) {
                Ok(message) => {
                    if let Err(e) = host
                        .peer_mut(peer_id)
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
    }
    // USB forwarding's answers and detaches (0x553A), reliable and in order.
    if h.usb.queued() {
        for usb in h.usb.take(&p.id, Instant::now()) {
            let sent = usb
                .encode()
                .context("the USB message does not encode")
                .and_then(|payload| {
                    p.encrypt(s, rubylight_protocol::usb::USB_MESSAGE_TYPE, &payload)
                })
                .and_then(|message| {
                    host.peer_mut(peer_id)
                        .send(0, &Packet::new(message, PacketKind::Reliable))
                        .map_err(|e| anyhow::anyhow!("{e}"))
                });
            if let Err(e) = sent {
                tracing::warn!(error = %format!("{e:#}"), ?usb, "USB message not sent");
            }
        }
    }
}
