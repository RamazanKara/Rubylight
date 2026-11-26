//! Controller feedback, rumble, motion requests, and audio-derived haptics waveform
//! delivery over the control connection.

use super::super::*;
use super::peer::ControlPeer;
use super::socket::ControlSocket;

pub(in crate::stream) const MOTION_EVENT_REQUEST: u16 = 0x5501;

pub const RUMBLE_TRIGGER_DATA: u16 = 0x5500;

pub const RUMBLE_DATA: u16 = 0x010b;

/// Steam Controller (2026) haptic reports, only for a controller whose
/// client announced the real pads (`LI_CCAP_STEAM_HAPTIC`).
pub(crate) const STEAM_HAPTIC_DATA: u16 = 0x5504;

/// Steam Controller settings writes (`LI_CCAP_STEAM_SETTINGS`).
pub(crate) const STEAM_SETTINGS_DATA: u16 = 0x5505;

pub(in crate::stream) const RUMBLE_MARKER: u32 = 0x00c0ffee;

// Channel 0 is available even when an older Moonlight peer negotiates only
// one channel. Sending on channel 1 then fails with InvalidChannel.
pub(in crate::stream) const FEEDBACK_CHANNEL: u8 = 0;

/// What a pass sends for the gamepad thread's reports: feedback messages
/// (reliable) and speaker and haptics waveforms (the haptics channel).
#[derive(Default)]
pub(in crate::stream) struct PadSends {
    pub(in crate::stream) messages: Vec<(u16, Vec<u8>)>,
    pub(in crate::stream) waveforms: Vec<(u16, Vec<u8>)>,
}

/// Polls gamepad feedback reports and sends resulting waveforms and reliable feedback
/// messages to `peer_id`.
pub(super) fn send_pad_feedback(
    h: &Shared,
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
    poll_feedback: bool,
) {
    super::ps_speaker::offer_status(h, p, s);
    // The gamepad thread polls feedback every 8 ms; pass on
    // what it found, and the sensors an arrived pad has.
    let sends = match (&p.injector, &p.coop) {
        (Some(i), _) => pad_reports(
            i.gamepad_reports(),
            |kind| i.feedback_allowed(kind),
            haptics_negotiated(&s.config, host.peer(peer_id).channel_count()),
            &p.controller_audio,
            &mut p.ps_speaker,
            &mut p.controller_pcm,
            &mut p.audio_haptics.arbiter,
            &mut p.rumble,
        ),
        (None, Some(player)) if poll_feedback => pad_reports(
            player.feedback(h),
            |kind| player.feedback_allowed(kind),
            haptics_negotiated(&s.config, host.peer(peer_id).channel_count()),
            &p.controller_audio,
            &mut p.ps_speaker,
            &mut p.controller_pcm,
            &mut p.audio_haptics.arbiter,
            &mut p.rumble,
        ),
        _ => PadSends::default(),
    };
    s.reserve_controller_pcm(p.controller_pcm.streams(Instant::now()));
    for (kind, payload) in sends.waveforms {
        p.send_waveform(host, peer_id, s, kind, &payload);
    }
    for (kind, payload) in sends.messages {
        if let Ok(message) = p.encrypt(s, kind, &payload) {
            let _ = host.peer_mut(peer_id).send(
                FEEDBACK_CHANNEL,
                &Packet::new(message, PacketKind::Reliable),
            );
        }
    }
}

/// Generates and sends audio-derived haptics waveforms for every active peer after input
/// and acks have gone out.
pub(super) fn send_audio_haptics(
    host: &mut Host<ControlSocket>,
    peers: &mut HashMap<PeerID, ControlPeer>,
) {
    for (peer_id, p) in peers {
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
            p.send_waveform(host, *peer_id, &s, haptics::PACKET_TYPE, &payload);
        }
    }
}

/// The game's own haptics and rumble reach `arbiter` here, before the same
/// pass generates haptics from game audio, so it already leaves those
/// controllers alone.
#[allow(clippy::too_many_arguments)]
pub(in crate::stream) fn pad_reports(
    reports: impl IntoIterator<Item = PadReport>,
    feedback_allowed: impl Fn(u16) -> bool,
    negotiated: bool,
    controller_audio: &[u8; 16],
    speakers: &mut butterpollo_input_policy::ps_speaker::Speakers,
    pcm: &mut controller_audio::PcmTraffic,
    arbiter: &mut butterpollo_input_policy::audio_haptics::Arbiter,
    rumble: &mut RumbleForwarded,
) -> PadSends {
    let mut sends = PadSends::default();
    for report in reports {
        match report {
            // A bound controller's speaker goes as Nonary's speaker PCM, never as 0x5533 too.
            PadReport::Speaker(frame) if speakers.bound(frame.id) => {
                if frame.created.elapsed() <= haptics::MAX_AGE
                    && let Some(payload) = speakers.block(frame.id, frame.sequence, &frame.samples)
                {
                    pcm.observe(frame.id, controller_audio::SPEAKER, Instant::now());
                    sends.waveforms.push((
                        butterpollo_input_policy::ps_speaker::PS_SPEAKER_PCM_MESSAGE_TYPE,
                        payload,
                    ));
                }
            }
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
            // Its virtual DualSense's USB audio state is not sent to a client that binds speakers.
            PadReport::Feedback { id, kind, .. }
                if kind == controller_audio::CONTROL && speakers.bound(id) => {}
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
pub(in crate::stream) fn motion_requests(id: u8, capabilities: u16) -> Vec<(u16, Vec<u8>)> {
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

pub(in crate::stream) fn haptics_negotiated(
    config: &butterpollo_media_policy::rtsp::Negotiated,
    channels: usize,
) -> bool {
    config.encryption & 1 != 0
        && config.moonlight_features & haptics::FEATURE != 0
        && channels > usize::from(haptics::CHANNEL)
}

/// Whether each controller's main and trigger motors ran in the last rumble
/// this client was sent.
#[derive(Default)]
pub(in crate::stream) struct RumbleForwarded {
    running: [[bool; 2]; 16],
}

impl RumbleForwarded {
    /// Whether a feedback packet goes out, given whether the injector allows
    /// its kind. With rumble off, a stop still goes out once for motors the
    /// client was last told to run, so they don't keep buzzing; other stops
    /// stay blocked, as VHF repeats them with every lightbar or trigger change.
    pub(in crate::stream) fn forward(&mut self, allowed: bool, kind: u16, payload: &[u8]) -> bool {
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

pub fn feedback_packets(id: u16, kind: u16, data: &[u8]) -> Vec<(u16, Vec<u8>)> {
    if kind == controller_audio::CONTROL && data.len() == 12 {
        return vec![(kind, data.to_vec())];
    }
    // Steam Controller haptics and settings, already the control packets'
    // payloads with the controller in them.
    if (kind == STEAM_HAPTIC_DATA && data.len() == 15)
        || (kind == STEAM_SETTINGS_DATA && data.len() == 31)
    {
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
