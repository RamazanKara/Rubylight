//! Nonary's Moonlight PCM haptics extension (ControllerHaptics.h).
//! Only actuator channels are transported: 48 kHz, stereo signed 16-bit LE.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub const FEATURE: u32 = 0x04;
pub const CAPABILITY: u16 = 0x8000;
pub const PACKET_TYPE: u16 = 0x5601;
pub const CHANNEL: u8 = 8;
pub const FRAMES: usize = 240;
pub const MAX_AGE: Duration = Duration::from_millis(20);

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub id: u16,
    pub sequence: u32,
    pub created: Instant,
    pub samples: Vec<u8>,
}
impl Frame {
    pub fn payload(&self) -> Option<Vec<u8>> {
        if self.id >= 16
            || self.samples.is_empty()
            || self.samples.len() > FRAMES * 4
            || !self.samples.len().is_multiple_of(4)
        {
            return None;
        }
        let mut bytes = vec![1, 1];
        bytes.extend_from_slice(&self.id.to_le_bytes());
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&((self.samples.len() / 4) as u16).to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&self.samples);
        Some(bytes)
    }
}

/// Separate from reliable controller feedback. A stalled consumer retains
/// at most 20 ms per controller and never replays old audio after resuming.
#[derive(Default)]
pub struct Queue(VecDeque<Frame>);
impl Queue {
    pub fn push(&mut self, frame: Frame, now: Instant) {
        self.0
            .retain(|f| now.saturating_duration_since(f.created) <= MAX_AGE);
        if frame.id >= 16 || now.saturating_duration_since(frame.created) > MAX_AGE {
            return;
        }
        if self.0.iter().filter(|f| f.id == frame.id).count() >= 4 {
            self.0.retain(|f| f.id != frame.id);
        }
        self.0.push_back(frame);
    }
    pub fn retain_controllers(&mut self, mask: u16) {
        self.0.retain(|f| mask & (1 << f.id) != 0);
    }
    pub fn drain(&mut self, now: Instant) -> impl Iterator<Item = Frame> + '_ {
        self.0
            .drain(..)
            .filter(move |f| now.saturating_duration_since(f.created) <= MAX_AGE)
    }
}

#[derive(Default)]
pub struct Packetizer {
    pending: Vec<u8>,
    last: Option<Instant>,
}
impl Packetizer {
    pub fn clear(&mut self) {
        self.pending.clear();
        self.last = None;
    }
    pub fn push(&mut self, stereo: &[u8], now: Instant) -> Vec<Vec<u8>> {
        if !stereo.len().is_multiple_of(4) {
            self.clear();
            return vec![];
        }
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) > MAX_AGE)
        {
            self.pending.clear();
        }
        self.last = Some(now);
        let mut result = vec![];
        for frame in stereo.as_chunks::<4>().0 {
            self.pending.extend_from_slice(frame);
            if self.pending.len() == FRAMES * 4 {
                result.push(std::mem::take(&mut self.pending));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packets_match_the_client_contract_and_partial_audio_expires() {
        let now = Instant::now();
        let mut p = Packetizer::default();
        assert!(p.push(&[9; 192], now).is_empty());
        assert!(
            p.push(&[1; 768], now + Duration::from_millis(30))
                .is_empty()
        );
        let samples = p.push(&[2; 192], now + Duration::from_millis(31)).remove(0);
        let wire = Frame {
            id: 2,
            sequence: 0x12345678,
            created: now,
            samples,
        }
        .payload()
        .unwrap();
        assert_eq!(
            &wire[..12],
            &[1, 1, 2, 0, 0x78, 0x56, 0x34, 0x12, 240, 0, 0, 0]
        );
        assert_eq!(&wire[12..780], &[1; 768]);
        assert_eq!(&wire[780..], &[2; 192]);
    }
    #[test]
    fn slow_consumers_and_slot_reuse_cannot_replay_a_backlog() {
        let now = Instant::now();
        let frame = |id, sequence| Frame {
            id,
            sequence,
            created: now,
            samples: vec![0; 960],
        };
        let mut q = Queue::default();
        q.push(frame(1, 0), now);
        for seq in 0..5 {
            q.push(frame(0, seq), now);
        }
        let frames: Vec<_> = q.drain(now).collect();
        assert_eq!(
            frames
                .iter()
                .map(|f| (f.id, f.sequence))
                .collect::<Vec<_>>(),
            [(1, 0), (0, 4)]
        );
        q.push(frame(0, 5), now);
        q.retain_controllers(2);
        assert_eq!(q.drain(now).count(), 0);
        q.push(frame(1, 1), now);
        assert_eq!(q.drain(now + Duration::from_millis(21)).count(), 0);
    }
}
