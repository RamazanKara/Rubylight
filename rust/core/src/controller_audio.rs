//! Rubylight USB controller audio, negotiated independently of Nonary's actuator PCM.
use crate::haptics;
use std::{collections::VecDeque, time::Instant};

pub const SPEAKER: u16 = 0x5533;
pub const MICROPHONE: u16 = 0x5534;
pub const CONTROL: u16 = 0x5535;
pub const PLAYBACK: u8 = 1;
pub const CAPTURE: u8 = 2;
pub const HEADPHONES: u8 = 4;
pub const MUTED: u8 = 8;

/// The USB speaker endpoint has no process provenance. It must not bypass
/// game-only capture, disabled stream sound or an input-only client's role.
pub fn speaker_allowed(
    config: &crate::config::Config,
    role: crate::session::Role,
    viewer: bool,
) -> bool {
    viewer
        && role != crate::session::Role::InputOnly
        && config.boolean("stream_audio", true)
        && !(role == crate::session::Role::RemoteMonitor
            && config.boolean("remote_monitor_mute_audio", false))
        && config.get("audio_capture_mode", "system") == "system"
}

pub struct StatusPolicy {
    pub device_flags: u8,
    pub client_flags: u8,
}
pub fn status_policy(
    config: &crate::config::Config,
    role: crate::session::Role,
    viewer: bool,
    mut flags: u8,
) -> StatusPolicy {
    if !config.boolean("stream_mic", true) {
        flags &= !CAPTURE;
    }
    StatusPolicy {
        device_flags: if speaker_allowed(config, role, viewer) {
            flags
        } else {
            flags & !PLAYBACK
        },
        // PLAYBACK also enables the client's normal Opus stream-to-headset
        // routing. Keep that acknowledgement independent of USB speaker PCM.
        client_flags: flags,
    }
}

/// Stereo S16/48 kHz plus packet/control encryption, ENet and IPv6 overhead.
pub const PCM_WIRE_BPS: u64 = 2_000_000;
const PCM_ACTIVE_FOR: std::time::Duration = std::time::Duration::from_secs(5);

/// Account for only observed PCM paths, including microphone uplink on shared
/// wireless airtime. Silence expires; it cannot permanently suppress recovery.
#[derive(Default)]
pub struct PcmTraffic {
    recent: [[Option<Instant>; 3]; 16],
}
impl PcmTraffic {
    pub fn observe(&mut self, id: u16, kind: u16, now: Instant) {
        let slot = match kind {
            haptics::PACKET_TYPE => 0,
            SPEAKER => 1,
            MICROPHONE => 2,
            _ => return,
        };
        if let Some(paths) = self.recent.get_mut(usize::from(id)) {
            paths[slot] = Some(now);
        }
    }
    pub fn streams(&self, now: Instant) -> usize {
        self.recent
            .iter()
            .flatten()
            .filter(|at| at.is_some_and(|at| now.saturating_duration_since(at) < PCM_ACTIVE_FOR))
            .count()
    }
}

pub fn pcm(payload: &[u8]) -> Option<(u16, u32, &[u8])> {
    if payload.len() < 12 || payload[..2] != [1, 1] || payload[10..12] != [0, 0] {
        return None;
    }
    let id = u16::from_le_bytes(payload[2..4].try_into().ok()?);
    let frames = usize::from(u16::from_le_bytes(payload[8..10].try_into().ok()?));
    if id >= 16 || !(1..=haptics::FRAMES).contains(&frames) || payload.len() != 12 + frames * 4 {
        return None;
    }
    Some((
        id,
        u32::from_le_bytes(payload[4..8].try_into().ok()?),
        &payload[12..],
    ))
}
/// [version, operation=1, controller:u16, flags, zero, zero, zero].
pub fn status(payload: &[u8]) -> Option<(u16, u8)> {
    if payload.len() != 8
        || payload[..2] != [1, 1]
        || payload[4] & !15 != 0
        || payload[5..] != [0; 3]
    {
        return None;
    }
    let id = u16::from_le_bytes([payload[2], payload[3]]);
    (id < 16).then_some((id, payload[4]))
}

#[derive(Default)]
pub struct Capture {
    chunks: VecDeque<(Instant, Vec<u8>, usize)>,
    sequence: Option<u32>,
    bytes: usize,
}
impl Capture {
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.bytes = 0;
        self.sequence = None;
    }
    pub fn push(&mut self, sequence: u32, bytes: &[u8], now: Instant) {
        if bytes.is_empty() || bytes.len() > 960 || !bytes.len().is_multiple_of(4) {
            return;
        }
        if self
            .sequence
            .is_some_and(|old| sequence.wrapping_sub(old) as i32 <= 0)
        {
            return;
        }
        if self
            .sequence
            .is_some_and(|old| sequence != old.wrapping_add(1))
            || self.bytes + bytes.len() > 3840
        {
            self.clear();
        }
        self.sequence = Some(sequence);
        self.bytes += bytes.len();
        self.chunks.push_back((now, bytes.to_vec(), 0));
    }
    pub fn read(&mut self, length: usize, now: Instant) -> Vec<u8> {
        let mut result = vec![0; length];
        let mut written = 0;
        while written < length {
            let Some((at, bytes, offset)) = self.chunks.front_mut() else {
                break;
            };
            if now.saturating_duration_since(*at) > haptics::MAX_AGE {
                self.bytes -= bytes.len() - *offset;
                self.chunks.pop_front();
                continue;
            }
            let n = (length - written).min(bytes.len() - *offset);
            result[written..written + n].copy_from_slice(&bytes[*offset..*offset + n]);
            *offset += n;
            written += n;
            self.bytes -= n;
            if *offset == bytes.len() {
                self.chunks.pop_front();
            }
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unscoped_usb_sound_cannot_bypass_game_only_or_muted_streams() {
        use crate::session::Role;
        for (text, role, viewer, allowed) in [
            ("", Role::Stream, true, true),
            ("audio_capture_mode=game", Role::Stream, true, false),
            ("stream_audio=false", Role::Stream, true, false),
            (
                "remote_monitor_mute_audio=true",
                Role::RemoteMonitor,
                true,
                false,
            ),
            ("", Role::InputOnly, true, false),
            ("", Role::Stream, false, false),
        ] {
            assert_eq!(
                speaker_allowed(&crate::config::Config::parse(text).unwrap(), role, viewer),
                allowed
            );
        }
        let config = crate::config::Config::parse("audio_capture_mode=game").unwrap();
        let policy = status_policy(&config, Role::Stream, true, PLAYBACK | CAPTURE | HEADPHONES);
        assert_eq!(policy.device_flags, CAPTURE | HEADPHONES);
        assert_eq!(policy.client_flags, PLAYBACK | CAPTURE | HEADPHONES);
        let config =
            crate::config::Config::parse("audio_capture_mode=game\nstream_mic=false").unwrap();
        let policy = status_policy(&config, Role::Stream, true, PLAYBACK | CAPTURE | HEADPHONES);
        assert_eq!(policy.device_flags, HEADPHONES);
        assert_eq!(policy.client_flags, PLAYBACK | HEADPHONES);
    }
    #[test]
    fn pcm_reservations_are_unique_bounded_and_expire() {
        let now = Instant::now();
        let mut traffic = PcmTraffic::default();
        for id in 0..16 {
            for kind in [haptics::PACKET_TYPE, SPEAKER, MICROPHONE] {
                traffic.observe(id, kind, now);
                traffic.observe(id, kind, now);
            }
        }
        traffic.observe(u16::MAX, SPEAKER, now);
        traffic.observe(0, CONTROL, now);
        assert_eq!(traffic.streams(now), 48);
        assert_eq!(traffic.streams(now + PCM_ACTIVE_FOR), 0);
        traffic.observe(0, SPEAKER, now + PCM_ACTIVE_FOR);
        assert_eq!(traffic.streams(now + PCM_ACTIVE_FOR), 1);
    }
    #[test]
    fn microphone_drops_old_reordered_and_stale_audio() {
        let now = Instant::now();
        let mut capture = Capture::default();
        capture.push(u32::MAX, &[1; 8], now);
        capture.push(0, &[2; 8], now);
        capture.push(u32::MAX, &[3; 8], now);
        assert_eq!(
            capture.read(20, now),
            [vec![1; 8], vec![2; 8], vec![0; 4]].concat()
        );
        capture.push(1, &[4; 8], now);
        assert_eq!(
            capture.read(8, now + std::time::Duration::from_millis(21)),
            vec![0; 8]
        );
        capture.push(2, &[5; 8], now);
        capture.push(4, &[6; 4], now);
        assert_eq!(capture.read(8, now), [6, 6, 6, 6, 0, 0, 0, 0]);
    }
    #[test]
    fn protocol_rejects_reserved_bits_lengths_and_controller_overflow() {
        assert_eq!(status(&[1, 1, 15, 0, 15, 0, 0, 0]), Some((15, 15)));
        assert!(status(&[1, 1, 16, 0, 1, 0, 0, 0]).is_none());
        assert!(status(&[1, 1, 0, 0, 16, 0, 0, 0]).is_none());
        let frame = haptics::Frame {
            id: 0,
            sequence: 5,
            created: Instant::now(),
            samples: vec![1; 16],
        };
        let mut bytes = frame.payload().unwrap();
        assert!(pcm(&bytes).is_some());
        bytes.push(0);
        assert!(pcm(&bytes).is_none());
    }
}
