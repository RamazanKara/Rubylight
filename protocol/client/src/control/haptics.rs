//! The PCM layout shared by Nonary's controller haptics (0x5601) and Rubylight's controller
//! speaker and microphone messages (0x5533, 0x5534), from `ControllerHaptics.h`
//! (Nonary/moonlight-common-c 4ea115e8, GPL-3.0).
//!
//! Little-endian: version 1, format 1 (48 kHz S16LE stereo), controller, sequence, frame
//! count, reserved 0, then interleaved left/right samples.

pub const HEADER_BYTES: usize = 12;
pub const MAX_FRAMES: usize = 240;
pub const FRAME_BYTES: usize = 4;
pub const MAX_PAYLOAD: usize = HEADER_BYTES + MAX_FRAMES * FRAME_BYTES;
/// Controller capability flag (`LI_CCAP_HAPTICS_PCM`) of an arrival event.
pub const CAPABILITY: u16 = 0x8000;

/// One PCM block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pcm {
    pub controller: u16,
    pub sequence: u32,
    /// Interleaved S16LE stereo samples, `frames * 4` bytes.
    pub samples: Vec<u8>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / FRAME_BYTES
    }
}

fn u16_at(p: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([p[at], p[at + 1]])
}

/// `MlHapticsValidate`.
pub fn validate(payload: &[u8]) -> bool {
    if payload.len() < HEADER_BYTES
        || payload[0] != 1
        || payload[1] != 1
        || u16_at(payload, 2) >= 16
        || u16_at(payload, 10) != 0
    {
        return false;
    }
    let frames = usize::from(u16_at(payload, 8));
    (1..=MAX_FRAMES).contains(&frames) && payload.len() == HEADER_BYTES + frames * FRAME_BYTES
}

pub fn decode(payload: &[u8]) -> Option<Pcm> {
    validate(payload).then(|| Pcm {
        controller: u16_at(payload, 2),
        sequence: u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
        samples: payload[HEADER_BYTES..].to_vec(),
    })
}

/// `None` for a controller over 15 or samples that are not 1 to 240 whole frames.
pub fn encode(controller: u16, sequence: u32, samples: &[u8]) -> Option<Vec<u8>> {
    let frames = samples.len() / FRAME_BYTES;
    if controller >= 16 || samples.len() % FRAME_BYTES != 0 || !(1..=MAX_FRAMES).contains(&frames) {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER_BYTES + samples.len());
    out.extend_from_slice(&[1, 1]);
    out.extend_from_slice(&controller.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&(frames as u16).to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(samples);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_frames_decode() {
        let frame = butterpollo_core::haptics::Frame {
            id: 4,
            sequence: 1234,
            created: std::time::Instant::now(),
            samples: vec![7; 16],
        };
        let payload = frame.payload().unwrap();
        let pcm = decode(&payload).unwrap();
        assert_eq!((pcm.controller, pcm.sequence, pcm.frames()), (4, 1234, 4));
        assert_eq!(encode(4, 1234, &[7; 16]).unwrap(), payload);
    }

    #[test]
    fn rejects_malformed_blocks() {
        let good = encode(1, 1, &[0; 8]).unwrap();
        assert!(validate(&good));
        for (at, value) in [(0, 2), (1, 0), (2, 16), (10, 1)] {
            let mut bad = good.clone();
            bad[at] = value;
            assert!(!validate(&bad), "byte {at}");
        }
        assert!(!validate(&good[..good.len() - 4]));
        assert!(encode(16, 0, &[0; 4]).is_none());
        assert!(encode(0, 0, &[]).is_none());
        assert!(encode(0, 0, &[0; 5]).is_none());
        assert!(encode(0, 0, &vec![0; (MAX_FRAMES + 1) * 4]).is_none());
        assert!(encode(0, 0, &vec![0; MAX_FRAMES * 4]).is_some());
    }
}
