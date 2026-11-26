//! Controller USB audio control and microphone PCM packet handling.

use super::super::*;
use super::haptics::FEEDBACK_CHANNEL;
use super::peer::ControlPeer;
use super::socket::ControlSocket;

/// Handles incoming `controller_audio::CONTROL` and `controller_audio::MICROPHONE` packets.
pub(super) fn handle_controller_audio(
    h: &Shared,
    peer: &mut rusty_enet::Peer<ControlSocket>,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    kind: u16,
    payload: &[u8],
) {
    if !s.launch.client.allows(1 << 8) {
        return;
    }
    if kind == controller_audio::CONTROL {
        if let Some((id, flags)) = controller_audio::status(payload) {
            // A controller bound to speaker PCM (0x5602) never takes USB controller audio too.
            if p.ps_speaker.bound(id) {
                return;
            }
            let Ok(config) = p.config(h, s) else {
                return;
            };
            let policy = controller_audio::status_policy(
                &config,
                s.launch.role.into(),
                s.launch.client.perm & ((1 << 25) | (1 << 26)) != 0,
                flags,
            );
            if flags & controller_audio::PLAYBACK != 0
                && policy.device_flags & controller_audio::PLAYBACK == 0
                && config::audio_capture_mode_is_game(&config)
            {
                s.launch.warnings.set("audio_controller_scope", "Game-only audio suppresses dedicated USB controller speaker sound because that endpoint cannot identify its source process. The normal captured game stream still routes to a connected controller headset; haptics and microphone settings remain independent.");
            }
            let mut state = payload.to_vec();
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
            if let Ok(message) = p.encrypt(s, kind, &state) {
                let _ = peer.send(
                    FEEDBACK_CHANNEL,
                    &Packet::new(message, PacketKind::Reliable),
                );
            }
        }
    } else if let Some((id, _, _)) = controller_audio::pcm(payload)
        && p.controller_audio[id as usize] & (controller_audio::CAPTURE | controller_audio::MUTED)
            == controller_audio::CAPTURE
        && let Some(i) = &p.injector
    {
        p.controller_pcm.observe(id, kind, Instant::now());
        i.controller_audio(id, kind, payload);
    }
}
