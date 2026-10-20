//! Deterministic randomness and network impairments.

/// SplitMix64.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 1.0 < p
    }
}

/// How the network treats a packet stream.
#[derive(Clone, Copy, Debug, Default)]
pub struct Impairment {
    /// Probability of losing each packet.
    pub loss: f64,
    /// Probability of sending a packet a second time, a little later.
    pub duplicate: f64,
    /// Packets are shuffled within windows of this many (0 or 1: in order).
    pub reorder_window: usize,
}

impl Impairment {
    pub fn apply(&self, rng: &mut Rng, packets: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = Vec::with_capacity(packets.len());
        for packet in packets {
            if rng.chance(self.loss) {
                continue;
            }
            if rng.chance(self.duplicate) {
                let at = out.len().saturating_sub(rng.below(3));
                out.insert(at, packet.clone());
            }
            out.push(packet);
        }
        if self.reorder_window > 1 {
            for chunk in out.chunks_mut(self.reorder_window) {
                for i in (1..chunk.len()).rev() {
                    let j = rng.below(i + 1);
                    chunk.swap(i, j);
                }
            }
        }
        out
    }
}
