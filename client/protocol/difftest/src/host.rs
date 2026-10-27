//! Test hosts: video and audio packetizers with the wire layout of the Rubylight host
//! (`rust/core/src/packet.rs`), with knobs for the cases the tests need.

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::Aes128Gcm;
use nightjar_core::fec::{ReedSolomon, AUDIO_PARITY_MATRIX};

/// Frame header `frameType` values.
pub const FRAME_P: u8 = 1;
pub const FRAME_IDR: u8 = 2;
pub const FRAME_INTRA_REFRESH: u8 = 4;
pub const FRAME_AFTER_INVALIDATION: u8 = 5;

/// One video frame to send.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub payload: Vec<u8>,
    pub frame_type: u8,
    pub timestamp: u32,
    pub latency_us: u64,
    /// At least this many FEC blocks (1 to 4); more when the frame needs them.
    pub min_blocks: usize,
    /// Marks the extra LTR flag on the frame's packets.
    pub ltr: bool,
    /// Data shards whose `multiFecFlags` carry the PyroWave record-start bit (0x80).
    pub record_starts: Vec<usize>,
    /// Sends the frame's last data packet only up to its payload instead of full size.
    pub trim_last: bool,
}

impl VideoFrame {
    pub fn new(payload: Vec<u8>, frame_type: u8, timestamp: u32) -> Self {
        Self {
            payload,
            frame_type,
            timestamp,
            latency_us: 1234,
            min_blocks: 1,
            ltr: false,
            record_starts: Vec::new(),
            trim_last: false,
        }
    }
}

pub struct VideoHost {
    /// Moonlight's stream packet index; its low 16 bits are the RTP sequence number.
    pub sequence: u32,
    pub frame: u32,
    pub packet_size: usize,
    pub fec_percent: usize,
    pub key: Option<[u8; 16]>,
    pub iv_counter: u64,
    pub ssrc: u32,
    /// The first byte of the frame header (0x01: 8-byte header).
    pub header_byte: u8,
}

impl VideoHost {
    pub fn new(packet_size: usize, fec_percent: usize) -> Self {
        Self {
            sequence: 0,
            frame: 1,
            packet_size,
            fec_percent,
            key: None,
            iv_counter: 0,
            ssrc: 0,
            header_byte: 1,
        }
    }

    /// Data shards per block for `total` shards in at least `min_blocks` blocks, and the
    /// parity percentage each block can carry.
    fn plan(&self, total: usize, min_blocks: usize) -> Vec<(usize, usize)> {
        let per_block = 255 * 100 / (100 + self.fec_percent);
        let blocks = total
            .div_ceil(per_block)
            .max(min_blocks)
            .clamp(1, 4)
            .min(total);
        (0..blocks)
            .map(|b| {
                let count = total / blocks + usize::from(b < total % blocks);
                let mut percent = self.fec_percent;
                if count + (count * percent).div_ceil(100) > 255 {
                    percent = 100 * (255 - count) / count;
                }
                (count, percent)
            })
            .collect()
    }

    /// Packetizes one frame: an 8-byte frame header, then the payload in
    /// `packet_size - 16` byte shards, with Reed-Solomon parity per block.
    pub fn encode(&mut self, frame: &VideoFrame) -> Vec<Vec<u8>> {
        let block_size = self.packet_size + 16;
        let slice = block_size - 32;
        let total = frame.payload.len() + 8;
        let total_shards = total.div_ceil(slice);
        assert!(total_shards <= 4 * 255, "frame too large for the test host");
        let last = total % slice;
        let mut data = vec![0u8; total_shards * slice];
        data[0] = self.header_byte;
        data[1..3].copy_from_slice(&((frame.latency_us / 100).min(65535) as u16).to_le_bytes());
        data[3] = frame.frame_type;
        data[4..6].copy_from_slice(&((if last == 0 { slice } else { last }) as u16).to_le_bytes());
        data[8..total].copy_from_slice(&frame.payload);

        let plan = self.plan(total_shards, frame.min_blocks);
        let blocks = plan.len();
        let mut out = Vec::new();
        let mut first_shard = 0;
        for (block, &(count, percent)) in plan.iter().enumerate() {
            let parity = (count * percent).div_ceil(100);
            let mut shards = vec![vec![0u8; block_size]; count + parity];
            for (i, s) in shards.iter_mut().take(count).enumerate() {
                let shard = first_shard + i;
                s[32..].copy_from_slice(&data[shard * slice..(shard + 1) * slice]);
                s[16..20]
                    .copy_from_slice(&(self.sequence.wrapping_add(i as u32) << 8).to_le_bytes());
                s[20..24].copy_from_slice(&self.frame.to_le_bytes());
                s[24] = 1 | if i == 0 { 4 } else { 0 } | if i == count - 1 { 2 } else { 0 };
                s[25] = u8::from(frame.ltr);
                s[26] = 0x10
                    | if frame.record_starts.contains(&shard) {
                        0x80
                    } else {
                        0
                    };
                s[27] = ((block as u8) << 4) | (((blocks - 1) as u8) << 6);
            }
            if parity > 0 {
                let rs = ReedSolomon::new(count, parity).expect("valid block");
                let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
                rs.encode(&mut refs, block_size).unwrap();
            }
            for (i, s) in shards.iter_mut().enumerate() {
                s[0] = 0x90;
                s[2..4]
                    .copy_from_slice(&(self.sequence.wrapping_add(i as u32) as u16).to_be_bytes());
                s[4..8].copy_from_slice(&frame.timestamp.to_be_bytes());
                s[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
                s[20..24].copy_from_slice(&self.frame.to_le_bytes());
                s[27] = ((block as u8) << 4) | (((blocks - 1) as u8) << 6);
                let info = ((i as u32) << 12) | ((count as u32) << 22) | ((percent as u32) << 4);
                s[28..32].copy_from_slice(&info.to_le_bytes());
            }
            if frame.trim_last && block == blocks - 1 && last != 0 {
                shards[count - 1].truncate(32 + last);
            }
            for s in shards {
                out.push(self.seal(s));
            }
            self.sequence = self.sequence.wrapping_add((count + parity) as u32);
            first_shard += count;
        }
        self.frame = self.frame.wrapping_add(1);
        out
    }

    fn seal(&mut self, mut packet: Vec<u8>) -> Vec<u8> {
        let Some(key) = self.key else {
            return packet;
        };
        let cipher = Aes128Gcm::new(GenericArray::from_slice(&key));
        let mut iv = [0u8; 12];
        iv[..8].copy_from_slice(&self.iv_counter.to_le_bytes());
        iv[11] = b'V';
        self.iv_counter += 1;
        let tag = cipher
            .encrypt_in_place_detached(GenericArray::from_slice(&iv), &[], &mut packet)
            .unwrap();
        let mut datagram = iv.to_vec();
        datagram.extend_from_slice(&self.frame.to_le_bytes());
        datagram.extend_from_slice(&tag);
        datagram.extend_from_slice(&packet);
        datagram
    }
}

/// Annex B test bitstreams.
pub mod bitstream {
    use crate::net::Rng;

    /// Random bytes that may contain start codes when `raw` is set; otherwise every third
    /// byte is non-zero so none appear.
    pub fn body(rng: &mut Rng, len: usize, raw: bool) -> Vec<u8> {
        (0..len)
            .map(|i| {
                let b = rng.byte();
                if raw || i % 3 != 2 {
                    b
                } else {
                    b | 1
                }
            })
            .collect()
    }

    fn nal(out: &mut Vec<u8>, header: &[u8], body: &[u8], long: bool) {
        out.extend_from_slice(if long { &[0, 0, 0, 1] } else { &[0, 0, 1] });
        out.extend_from_slice(header);
        out.extend_from_slice(body);
    }

    /// An H.264 frame. Key frames carry SPS and PPS, optionally an AUD and SEI first and
    /// zero padding between the parameter sets.
    pub fn h264(rng: &mut Rng, idr: bool, len: usize, extras: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if extras {
            nal(&mut out, &[0x09, 0xf0], &[], true);
            nal(&mut out, &[0x06, 0x05], &body(rng, 12, false), true);
        }
        if idr {
            nal(
                &mut out,
                &[0x67, 0x64, 0x00, 0x28],
                &body(rng, 10, false),
                true,
            );
            if extras {
                out.extend_from_slice(&[0, 0]);
            }
            nal(&mut out, &[0x68, 0xee], &body(rng, 4, false), true);
            nal(&mut out, &[0x65, 0x88], &body(rng, len, extras), true);
        } else {
            if extras && rng.below(2) == 0 {
                nal(&mut out, &[0x68, 0xee], &body(rng, 4, false), true);
            }
            nal(
                &mut out,
                &[0x41, 0x9a],
                &body(rng, len, extras),
                rng.below(2) == 0,
            );
        }
        out
    }

    /// An HEVC frame: VPS, SPS, PPS and an IDR_W_RADL slice for key frames.
    pub fn hevc(rng: &mut Rng, idr: bool, len: usize, extras: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if extras {
            nal(&mut out, &[0x46, 0x01, 0x50], &[], true);
            nal(&mut out, &[0x4e, 0x01], &body(rng, 9, false), true);
        }
        if idr {
            nal(&mut out, &[0x40, 0x01], &body(rng, 20, false), true);
            nal(&mut out, &[0x42, 0x01], &body(rng, 30, false), true);
            nal(&mut out, &[0x44, 0x01], &body(rng, 6, false), true);
            nal(&mut out, &[0x26, 0x01], &body(rng, len, extras), true);
        } else {
            nal(&mut out, &[0x02, 0x01], &body(rng, len, extras), true);
        }
        out
    }
}

/// The audio host: Opus packets in RTP, a 4+2 FEC block after every fourth packet.
pub struct AudioHost {
    pub sequence: u16,
    pub timestamp: u32,
    pub packet_ms: u32,
    pub ssrc: u32,
    pub encryption: Option<([u8; 16], u32)>,
    pending: Vec<Vec<u8>>,
}

impl AudioHost {
    pub fn new(packet_ms: u32) -> Self {
        Self {
            sequence: 0,
            timestamp: 0,
            packet_ms,
            ssrc: 0,
            encryption: None,
            pending: Vec::new(),
        }
    }

    /// The data packet for `opus`, followed by the block's two parity packets after every
    /// fourth packet. Parity is computed over the longest payload of the block.
    pub fn encode(&mut self, opus: &[u8]) -> Vec<Vec<u8>> {
        let seq = self.sequence;
        let data = match self.encryption {
            Some((key, key_id)) => {
                use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
                let mut iv = [0u8; 16];
                iv[..4].copy_from_slice(&key_id.wrapping_add(u32::from(seq)).to_be_bytes());
                let cipher = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &iv.into());
                let mut buf = vec![0u8; opus.len() + 16];
                let n = cipher
                    .encrypt_padded_b2b_mut::<Pkcs7>(opus, &mut buf)
                    .unwrap()
                    .len();
                buf.truncate(n);
                buf
            }
            None => opus.to_vec(),
        };
        let mut p = vec![0x80, 97];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&self.timestamp.to_be_bytes());
        p.extend_from_slice(&self.ssrc.to_be_bytes());
        p.extend_from_slice(&data);
        let mut out = vec![p];
        self.pending.push(data);
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(self.packet_ms);
        if self.pending.len() == 4 {
            let len = self.pending.iter().map(Vec::len).max().unwrap();
            let mut shards: Vec<Vec<u8>> = self
                .pending
                .iter()
                .map(|d| {
                    let mut d = d.clone();
                    d.resize(len, 0);
                    d
                })
                .collect();
            shards.extend([vec![0; len], vec![0; len]]);
            if len > 0 {
                let rs = ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).unwrap();
                let mut refs: Vec<&mut [u8]> = shards.iter_mut().map(|s| &mut s[..]).collect();
                rs.encode(&mut refs, len).unwrap();
            }
            for (index, shard) in shards[4..].iter().enumerate() {
                let mut p = vec![0x80, 127];
                p.extend_from_slice(&self.sequence.wrapping_add(index as u16).to_be_bytes());
                p.extend_from_slice(&self.timestamp.to_be_bytes());
                p.extend_from_slice(&self.ssrc.to_be_bytes());
                p.push(index as u8);
                p.push(97);
                p.extend_from_slice(&seq.wrapping_sub(3).to_be_bytes());
                p.extend_from_slice(
                    &self
                        .timestamp
                        .wrapping_sub(4 * self.packet_ms)
                        .to_be_bytes(),
                );
                p.extend_from_slice(&self.ssrc.to_be_bytes());
                p.extend_from_slice(shard);
                out.push(p);
            }
            self.pending.clear();
        }
        out
    }
}
