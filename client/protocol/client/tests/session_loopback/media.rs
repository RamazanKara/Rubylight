//! Synthetic video and audio packetizers and shared constants for session loopback tests.

use nightjar_core::fec::{ReedSolomon, AUDIO_PARITY_MATRIX};

pub(crate) const KEY: [u8; 16] = *b"session-test-key";
pub(crate) const PING: &[u8; 16] = b"0123456789abcdef";
pub(crate) const PACKET_SIZE: usize = 1024;

/// A single-block video packetizer like the host's (see `video/tests.rs`).
pub(crate) struct Packetizer {
    pub(crate) seq: u16,
    pub(crate) spi: u32,
    pub(crate) frame: u32,
}

impl Packetizer {
    pub(crate) fn frame(&mut self, payload: &[u8], kind: u8) -> Vec<Vec<u8>> {
        let slice = PACKET_SIZE - 16;
        let mut data = vec![1, 0, 0, kind];
        let total = payload.len() + 8;
        let last = if total.is_multiple_of(slice) {
            slice
        } else {
            total % slice
        };
        data.extend_from_slice(&(last as u16).to_le_bytes());
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(payload);
        let count = total.div_ceil(slice);
        let parity = (count * 20).div_ceil(100);
        let mut shards: Vec<Vec<u8>> = (0..count + parity)
            .map(|i| {
                let mut s = vec![0u8; PACKET_SIZE + 16];
                if i < count {
                    s[16..20].copy_from_slice(&((self.spi + i as u32) << 8).to_le_bytes());
                    s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == count - 1 { 2 } else { 0 };
                    s[26] = 0x10;
                    let chunk =
                        &data[(i * slice).min(data.len())..((i + 1) * slice).min(data.len())];
                    s[32..32 + chunk.len()].copy_from_slice(chunk);
                }
                s
            })
            .collect();
        let rs = ReedSolomon::new(count, parity).unwrap();
        let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
        rs.encode(&mut refs, PACKET_SIZE + 16).unwrap();
        for (i, s) in shards.iter_mut().enumerate() {
            s[0] = 0x90;
            s[2..4].copy_from_slice(&self.seq.wrapping_add(i as u16).to_be_bytes());
            s[4..8].copy_from_slice(&(self.frame * 1500).to_be_bytes());
            s[20..24].copy_from_slice(&self.frame.to_le_bytes());
            let info = ((i as u32) << 12) | ((count as u32) << 22) | (20 << 4);
            s[28..32].copy_from_slice(&info.to_le_bytes());
        }
        self.seq = self.seq.wrapping_add((count + parity) as u16);
        self.spi += count as u32;
        self.frame += 1;
        shards
    }
}

pub(crate) fn h264_idr() -> Vec<u8> {
    let mut f = vec![
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80,
    ];
    f.extend_from_slice(&[0, 0, 0, 1, 0x65]);
    f.extend((0..3000u32).map(|i| (i % 251) as u8 | 1));
    f
}

pub(crate) fn h264_p() -> Vec<u8> {
    let mut f = vec![0, 0, 0, 1, 0x41];
    f.extend((0..2000u32).map(|i| (i % 249) as u8 | 1));
    f
}

/// Audio data packets with two parity packets after every fourth (see `audio/tests.rs`).
pub(crate) fn audio_stream(count: u16) -> Vec<Vec<u8>> {
    let rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
    let mut out = Vec::new();
    let mut block: Vec<Vec<u8>> = Vec::new();
    for seq in 0..count {
        let payload: Vec<u8> = (0..60).map(|i| (i as u16 ^ seq) as u8).collect();
        let mut p = vec![0x80, 97];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&(u32::from(seq) * 5).to_be_bytes());
        p.extend_from_slice(&[0; 4]);
        p.extend_from_slice(&payload);
        out.push(p);
        block.push(payload);
        if block.len() == 4 {
            let mut shards = block.clone();
            shards.push(vec![0; 60]);
            shards.push(vec![0; 60]);
            let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
            rs.encode(&mut refs, 60).unwrap();
            for (index, shard) in shards[4..].iter().enumerate() {
                let mut p = vec![0x80, 127];
                p.extend_from_slice(&(seq + 1 + index as u16).to_be_bytes());
                p.extend_from_slice(&[0; 8]);
                p.push(index as u8);
                p.push(97);
                p.extend_from_slice(&(seq - 3).to_be_bytes());
                p.extend_from_slice(&(u32::from(seq - 3) * 5).to_be_bytes());
                p.extend_from_slice(&[0; 4]);
                p.extend_from_slice(shard);
                out.push(p);
            }
            block.clear();
        }
    }
    out
}
