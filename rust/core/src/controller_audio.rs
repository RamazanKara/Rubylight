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
