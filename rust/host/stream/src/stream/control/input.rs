//! Input packet decoding, queuing, injector/gamepad readiness, and couch-menu batch
//! application for control peers.

use super::super::*;
use super::haptics::haptics_negotiated;
use super::peer::ControlPeer;
use super::socket::ControlSocket;

/// Decodes and queues an incoming `0x0206` input packet from `peer`.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_input_packet(
    h: &Shared,
    peer: &rusty_enet::Peer<ControlSocket>,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    payload: &[u8],
    encrypted: bool,
    legacy_buf: &mut Vec<u8>,
) {
    let speaker = super::ps_speaker::negotiated(s, peer.channel_count());
    // A 20-byte speaker binding shares the touchpad packet's magic; its length alone routes it.
    if encrypted
        && super::ps_speaker::take_binding(p, s, payload, speaker && s.launch.client.allows(1 << 8))
    {
        return;
    }
    let event = if encrypted {
        input::decode(payload)
    } else {
        p.legacy
            .open_into(&s.launch.key, payload, legacy_buf)
            .and_then(input::decode)
    };
    match event {
        // Say once per kind that the device may not
        // send it: dropped silently, a phone's
        // controller looked undetected.
        Ok(event) if !s.launch.client.allows(required_permission(&event)) => {
            let permission = required_permission(&event);
            if p.denied & permission == 0 {
                p.denied |= permission;
                let (code, message) = input::denied_warning(&event, &s.launch.client.name);
                s.launch.warnings.set(&code, message);
            }
        }
        // A guest's input needs its guest permission, and keys and
        // the mouse go only to their one holder (`multiclient`).
        Ok(event) if !h.device_guests.admits(s, required_permission(&event)) => {}
        Ok(mut event) => {
            super::ps_speaker::input(p, &mut event, speaker);
            p.audio_haptics.input(
                &mut event,
                haptics_negotiated(&s.config, peer.channel_count()),
            );
            if let input::Input::Arrival { capabilities, .. } = &event
                && let Some(hint) = butterpollo_input_policy::input_policy::steam_input_hint(
                    &s.launch.client.name,
                    *capabilities,
                )
            {
                s.launch.warnings.set("input_steam_input", hint);
            }
            if matches!(
                event,
                input::Input::Relative { .. }
                    | input::Input::Absolute { .. }
                    | input::Input::Touch { .. }
                    | input::Input::Pen { .. }
            ) && let Some(window) = p.pointer_gaps.note(Instant::now())
            {
                tracing::info!(
                    client = %s.launch.client.name,
                    moves = window.arrivals,
                    gaps_25_50 = window.gaps_25_50,
                    gaps_50_plus = window.gaps_50_plus,
                    pauses = window.pauses,
                    max_gap_ms = window.max_gap_ms,
                    inject_max_ms = std::mem::take(&mut p.inject_max).as_secs_f32() * 1000.,
                    "pointer arrivals"
                );
            }
            // Frame Warp's input history, for a client that asked:
            // a short queue push before the event is injected.
            if s.frame_warp.enabled() {
                match &event {
                    input::Input::Relative { x, y } => s.frame_warp.on_mouse(*x, *y, p.seen),
                    input::Input::Controller { id, sticks, .. } => {
                        s.frame_warp
                            .on_controller(*id, (sticks[2], sticks[3]), p.seen)
                    }
                    // Frame Warp 2's gyro: only the requested pad's readings are queued.
                    input::Input::Motion { id, kind, xyz } => {
                        s.frame_warp.on_motion(*id, *kind, *xyz, p.seen)
                    }
                    _ => {}
                }
            }
            // Motion and buttons for emulators (`enable_dsu_server`).
            crate::dsu::observe(&p.id, &event);
            if !queue_input(&mut p.inputs, event) {
                // Warnings::set logs it as well.
                s.launch.warnings.set("input_overflow", INPUT_OVERFLOW);
                s.stop();
            }
        }
        Err(e) => {
            if let Some(error) = first_steam_attributes_refusal(&mut p.steam_attributes_refused, &e)
            {
                tracing::warn!(client=%s.launch.client.name, %error, "ignoring Steam Controller attributes");
            } else {
                tracing::debug!(error=%e,"invalid input packet");
            }
        }
    }
}

/// Ensures `p`'s co-op slot or desktop injector and controllers are initialized; returns
/// `false` if effective configuration could not be loaded (caller skips the rest of this
/// peer's pass).
pub(super) fn ensure_input_ready(
    h: &Shared,
    control_wake: &Arc<UdpSocket>,
    p: &mut ControlPeer,
    s: &Arc<Session>,
) -> bool {
    // A co-op player's input drives its slot's pad and never
    // the desktop: it gets no injector.
    if s.launch.nucleus.is_some() && !p.coop_tried {
        p.coop_tried = true;
        p.coop = effective_config(h, &s.launch)
            .inspect_err(|e| tracing::warn!(error = %format!("{e:#}"), "co-op input configuration unavailable"))
            .ok()
            .and_then(|c| crate::nucleus::Player::of(h, &s.launch, &c));
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
        let c = match effective_config(h, &s.launch) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "input configuration unavailable");
                p.inputs.clear();
                return false;
            }
        };
        let output = s.output.read().unwrap().clone();
        // Keys, clicks and moves need no controller: while the controllers
        // cannot be had, the injector works without them and they follow.
        let pads = crate::controller_sessions::acquire_for(h, &s.launch, &c)
            .inspect_err(|error| controllers_wait(s, error))
            .ok();
        match Injector::new_options_reported(
            if output.is_empty() {
                config::output_name(&c)
            } else {
                &output
            },
            config::gamepad_setting(&c),
            &c,
            s.launch.warnings.clone(),
        ) {
            Ok(mut i) => {
                i.set_feedback_waker(control_wake.clone());
                match pads {
                    Some(pads) => {
                        attach_pads(&mut i, pads, &p.controller_audio);
                        s.launch.warnings.clear("input_initialization");
                    }
                    None => p.pads_wait.failed(Instant::now()),
                }
                i.set_stream_size(s.config.width, s.config.height);
                p.injector = Some(i);
                p.keyboard_layout.refresh();
            }
            Err(e) => s.launch.warnings.set(
                "input_initialization",
                format!(
                    "Input initialization failed ({e:#}); controls are unavailable. Check Input settings and the selected display, then reconnect."
                ),
            ),
        }
    }
    // The controllers' retry, on its own interval rather than every pass.
    if let Some(i) = &mut p.injector
        && let Some(held) = p.pads_wait.retry(Instant::now(), || {
            let pads = effective_config(h, &s.launch)
                .and_then(|c| crate::controller_sessions::acquire_for(h, &s.launch, &c))?;
            attach_pads(i, pads, &p.controller_audio);
            Ok(())
        })
    {
        controllers_back(s);
        // What they missed that the client sends once, before what came since.
        p.inputs.splice(0..0, held);
    }
    true
}

/// Before `p`'s input is applied: the keyboard layout follows saves of `keyboard_layout`
/// and a coalesced report (read here; the injector only takes it), and the injector
/// follows the stream's display and size. Again in the same pass it changes nothing.
pub(super) fn follow_session(h: &Shared, p: &mut ControlPeer, s: &Session) {
    p.keyboard_layout.follow(
        h.config_version.load(Ordering::Acquire),
        &s.launch.warnings,
        Instant::now(),
        || *s.keyboard_layout.lock().unwrap(),
        || effective_config(h, &s.launch),
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
}

/// Applies `p`'s queued input: a co-op player's goes to its slot, and otherwise the
/// couch menu takes what it needs and the game gets the rest. `received` is when the
/// pass read its first datagram; the wait to injection goes to `stream timings`.
pub(super) fn apply_input(
    h: &Shared,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    menus: &mut crate::couch_menu::Workers,
    received: Option<Instant>,
) {
    p.pads_wait.hold(&mut p.inputs);
    let measured = !p.inputs.is_empty() && (p.injector.is_some() || p.coop.is_some());
    // Nor the couch menu, which the PC's display would show.
    if s.launch.nucleus.is_some() {
        let events = std::mem::take(&mut p.inputs);
        if let Some(player) = &mut p.coop {
            for event in events {
                player.input(event, &s.launch.warnings);
            }
        }
    }
    let injecting = Instant::now();
    let batch =
        crate::couch_menu::apply_batch(&mut p.couch, p.injector.as_mut(), &mut p.inputs, injecting);
    let injected = Instant::now();
    p.inject_max = p.inject_max.max(injected - injecting);
    if measured && let Some(received) = received {
        s.stats
            .input_latency
            .lock()
            .unwrap()
            .record(injected.saturating_duration_since(received));
    }
    if let Some(reason) = batch.refused {
        tracing::warn!(client = %s.launch.client.name, "{reason}");
        s.launch.warnings.event(
            "couch_menu",
            reason,
            butterpollo_session::session::EVENT_PERIOD,
        );
    }
    menus.send(h, s, batch.nav);
    for e in batch.errors {
        s.launch.warnings.event("input_injection", format!("Input injection failed ({e:#}); keyboard, mouse, touch or pen actions may be missing. Unlock the desktop and check Windows input permissions or disable native touch/pen if unsupported."), butterpollo_session::session::EVENT_PERIOD);
    }
    p.inputs.clear();
}

/// The refused Steam Controller attributes to warn about: only the first per
/// device, since a client that sends them malformed does so on every arrival.
/// Other invalid input stays a debug line.
pub(in crate::stream) fn first_steam_attributes_refusal(
    warned: &mut bool,
    error: &anyhow::Error,
) -> Option<input::SteamAttributesError> {
    let refusal = *error.downcast_ref::<input::SteamAttributesError>()?;
    (!std::mem::replace(warned, true)).then_some(refusal)
}

/// Decoded input waiting for the next injection pass.
pub(in crate::stream) const INPUT_QUEUE: usize = 512;

pub(in crate::stream) const INPUT_OVERFLOW: &str = "Input queue overflow: more than 512 input events waited for injection, so the stream was stopped. Reconnect to continue.";

/// Queues `event` for the next pass, merged into the last one where it can
/// be. False when the queue is full: the caller stops the stream with
/// `INPUT_OVERFLOW`.
pub(in crate::stream) fn queue_input(inputs: &mut Vec<input::Input>, event: input::Input) -> bool {
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

/// How often an injector made without the controllers tries for them again.
const PADS_RETRY: Duration = Duration::from_millis(100);

/// The controllers could not be had (the previous stream's are still closing): the stream's
/// card says keyboard and mouse work meanwhile.
pub fn controllers_wait(s: &Session, error: &anyhow::Error) {
    tracing::warn!(client = %s.launch.client.name, error = %format!("{error:#}"), "the controllers are not ready yet; keyboard and mouse work meanwhile, and they are tried again");
    s.launch.warnings.set(
        "input_initialization",
        "Controllers reconnect in a moment; keyboard and mouse work now.",
    );
}

/// An injector made without the controllers, because they could not be had yet: when it
/// tries for them again, and their input meanwhile. GameStream's control loop and the WebRTC
/// input worker alike.
#[derive(Default)]
pub struct PadsWait {
    retry: Option<Instant>,
    held: Vec<input::Input>,
}

impl PadsWait {
    /// When it next tries for them, while it waits.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry
    }
    /// They could not be had at `now`: tried again [`PADS_RETRY`] later.
    pub fn failed(&mut self, now: Instant) {
        self.retry = Some(now + PADS_RETRY);
    }
    /// Takes their input out of `inputs` while it waits ([`hold_pad_input`]); nothing once
    /// they are there.
    pub fn hold(&mut self, inputs: &mut Vec<input::Input>) {
        if self.retry.is_some() {
            hold_pad_input(inputs, &mut self.held);
        }
    }
    /// Tries for them when it is time at `now`: `attach` gets and gives them to the
    /// injector. Once they are there, what was held, to go in before anything newer.
    pub fn retry(
        &mut self,
        now: Instant,
        attach: impl FnOnce() -> Result<()>,
    ) -> Option<Vec<input::Input>> {
        if self.retry.is_none_or(|at| now < at) {
            return None;
        }
        match attach() {
            Ok(()) => {
                self.retry = None;
                Some(std::mem::take(&mut self.held))
            }
            Err(_) => {
                self.failed(now);
                None
            }
        }
    }
}

/// The controllers came after [`controllers_wait`].
pub fn controllers_back(s: &Session) {
    s.launch.warnings.clear("input_initialization");
    tracing::info!(client = %s.launch.client.name, "the controllers are back");
}

/// Gives the injector the controller session's pads, and the USB audio the client announced
/// for them before they came.
pub fn attach_pads(
    i: &mut Injector,
    pads: Arc<butterpollo_win_io::input::GamepadThread>,
    announced: &[u8; 16],
) {
    i.use_gamepads(pads);
    for (id, flags) in announced.iter().copied().enumerate() {
        if flags != 0 {
            i.controller_audio(
                id as u16,
                controller_audio::CONTROL,
                &[1, 1, id as u8, 0, flags, 0, 0, 0],
            );
        }
    }
}

/// While the injector waits for the controllers, their input waits in `held` and the rest
/// goes on: keys, clicks and moves need no controller. What waits is trimmed as
/// [`drop_uninjectable_input`] says.
pub(in crate::stream) fn hold_pad_input(
    inputs: &mut Vec<input::Input>,
    held: &mut Vec<input::Input>,
) {
    if !inputs.iter().any(waits_for_pads) {
        return;
    }
    let (pads, rest): (Vec<_>, Vec<_>) =
        std::mem::take(inputs).into_iter().partition(waits_for_pads);
    *inputs = rest;
    held.extend(pads);
    drop_uninjectable_input(held);
}

/// Input only the controllers take.
fn waits_for_pads(event: &input::Input) -> bool {
    matches!(
        event,
        input::Input::Controller { .. }
            | input::Input::SteamAttributes(_)
            | input::Input::ControllerIdentity(_)
            | input::Input::Arrival { .. }
            | input::Input::ControllerTouch { .. }
            | input::Input::Motion { .. }
            | input::Input::Battery { .. }
            | input::Input::Haptics(_)
    )
}

/// While the controllers can't be acquired there is nothing to inject into:
/// keys, clicks and moves are dropped rather than replayed in one late burst
/// when acquire succeeds. State the client sends once stays: arrivals, so
/// the pads keep the kind the client announced, the haptics switch, Steam
/// attributes, and each pad's latest state, whose active mask removes pads.
const MAX_HELD_PAD_INPUTS: usize = 64;

pub(in crate::stream) fn drop_uninjectable_input(inputs: &mut Vec<input::Input>) {
    let mut seen_pads: Vec<u16> = Vec::new();
    let mut seen_setup: Vec<(std::mem::Discriminant<input::Input>, u8)> = Vec::new();
    let mut seen_haptics = false;
    let mut keep = vec![false; inputs.len()];
    let mut kept_count = 0usize;
    for (at, event) in inputs.iter().enumerate().rev() {
        if kept_count >= MAX_HELD_PAD_INPUTS {
            break;
        }
        let take = match event {
            input::Input::Arrival { id, .. } => {
                let key = (std::mem::discriminant(event), *id);
                !seen_setup.contains(&key) && {
                    seen_setup.push(key);
                    true
                }
            }
            input::Input::SteamAttributes(a) => {
                let key = (std::mem::discriminant(event), a.controller);
                !seen_setup.contains(&key) && {
                    seen_setup.push(key);
                    true
                }
            }
            input::Input::ControllerIdentity(c) => {
                let key = (std::mem::discriminant(event), c.controller);
                !seen_setup.contains(&key) && {
                    seen_setup.push(key);
                    true
                }
            }
            input::Input::Haptics(_) => !std::mem::replace(&mut seen_haptics, true),
            input::Input::Controller { id, .. } => {
                !seen_pads.contains(id) && {
                    seen_pads.push(*id);
                    true
                }
            }
            _ => false,
        };
        if take {
            keep[at] = true;
            kept_count += 1;
        }
    }
    let mut at = 0;
    inputs.retain(|_| {
        let k = keep[at];
        at += 1;
        k
    });
}
