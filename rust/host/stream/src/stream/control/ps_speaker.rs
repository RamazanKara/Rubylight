//! The control loop's side of Nonary's controller speaker PCM (0x5602): speaker bindings taken
//! from the input stream before decoding, PlayStation arrivals scoped to them, and the virtual
//! DualSense of a bound controller told to play its speaker. The rules are
//! [`butterpollo_input_policy::ps_speaker`]'s; blocks go out in `pad_reports`.

use super::super::*;
use super::peer::ControlPeer;
use butterpollo_input_policy::ps_speaker;

/// How often a bound controller's speaker status is offered to its virtual DualSense again;
/// the gamepad thread keeps it and replays it once a slow USB attachment finishes.
const STATUS_EVERY: Duration = Duration::from_secs(1);

/// Whether this session takes speaker bindings and gets speaker blocks.
pub(super) fn negotiated(s: &Session, channels: usize) -> bool {
    ps_speaker::negotiated(
        s.config.moonlight_features,
        s.config.encryption,
        s.config.reliable_control,
        channels,
    )
}

/// Takes `payload`, an encrypted input packet, when it is a speaker binding by length and
/// magic (`ps_speaker::is_binding`); `false` leaves it to the input decoder, which reads the
/// 40-byte touchpad packet of the same magic as before.
pub(super) fn take_binding(
    p: &mut ControlPeer,
    s: &Session,
    payload: &[u8],
    negotiated: bool,
) -> bool {
    if !ps_speaker::is_binding(payload) {
        return false;
    }
    match p.ps_speaker.bind(payload, negotiated) {
        Ok(slot) => {
            tracing::debug!(client = %s.launch.client.name, slot, "controller speaker binding")
        }
        Err(refused) => {
            tracing::debug!(client = %s.launch.client.name, ?refused, "ignoring controller speaker binding")
        }
    }
    true
}

/// Scopes an arrival's 0x4000 to a bound PlayStation controller and notes which controllers
/// remain, before the event reaches the injector.
pub(super) fn input(p: &mut ControlPeer, event: &mut input::Input, negotiated: bool) {
    match event {
        input::Input::Arrival {
            id,
            kind,
            capabilities,
            ..
        } => {
            let (scoped, bound) = p.ps_speaker.arrival(*id, *kind, *capabilities, negotiated);
            *capabilities = scoped;
            if bound {
                // The controller plays speaker blocks: Rubylight's USB controller audio state
                // a client may have set for the slot no longer applies to it.
                p.controller_audio[usize::from(*id)] = 0;
                p.ps_speaker_status_at = None;
            }
        }
        input::Input::Controller { active, .. } => p.ps_speaker.controllers(*active),
        _ => {}
    }
}

/// Tells the virtual DualSense of each bound controller to play its speaker, as a Rubylight
/// client's 0x5535 status does, within the host's controller speaker policy.
pub(super) fn offer_status(h: &Shared, p: &mut ControlPeer, s: &Session) {
    let bound = p.ps_speaker.bound_mask();
    if bound == 0
        || p.injector.is_none()
        || p.ps_speaker_status_at
            .is_some_and(|at| at.elapsed() < STATUS_EVERY)
    {
        return;
    }
    let Ok(config) = p.config(h, s) else {
        return;
    };
    p.ps_speaker_status_at = Some(Instant::now());
    let policy = controller_audio::status_policy(
        &config,
        s.launch.role.into(),
        s.launch.client.perm & ((1 << 25) | (1 << 26)) != 0,
        controller_audio::PLAYBACK,
    );
    if policy.device_flags & controller_audio::PLAYBACK == 0
        && config::audio_capture_mode_is_game(&config)
    {
        s.launch.warnings.set("audio_controller_scope", "Game-only audio suppresses dedicated USB controller speaker sound because that endpoint cannot identify its source process. The normal captured game stream still routes to a connected controller headset; haptics and microphone settings remain independent.");
    }
    let Some(injector) = &p.injector else {
        return;
    };
    for id in (0..16u16).filter(|id| bound & (1 << id) != 0) {
        let [low, high] = id.to_le_bytes();
        let status = [1, 1, low, high, policy.device_flags, 0, 0, 0];
        injector.controller_audio(id, controller_audio::CONTROL, &status);
    }
}
