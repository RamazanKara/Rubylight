//! Reed-Solomon erasure coding over GF(2^8), bit-compatible with nanors as moonlight-common-c
//! uses it for video and audio FEC.
//!
//! Ported from nanors (MIT, Joseph Calderon) as vendored by moonlight-common-c (GPL-3.0,
//! Cameron Gutman and contributors). The field polynomial is 0x11d and the parity matrix is the
//! Cauchy matrix `p[j][i] = 1 / ((parity + i) ^ j)`; Moonlight audio replaces it with a fixed
//! OpenFEC matrix ([`ReedSolomon::with_parity_matrix`]).

use std::fmt;

/// nanors limit on data + parity shards.
pub const MAX_SHARDS: usize = 255;

/// The parity matrix GFE and Sunshine use for the 4+2 audio FEC blocks.
pub const AUDIO_PARITY_MATRIX: [u8; 8] = [0x77, 0x40, 0x38, 0x0e, 0xc7, 0xa7, 0x0d, 0x6c];

const fn exp_log() -> ([u8; 512], [u8; 256]) {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        exp[i] = x as u8;
        exp[i + 255] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x11d;
        }
        i += 1;
    }
    exp[510] = exp[255];
    (exp, log)
}

const EXP_LOG: ([u8; 512], [u8; 256]) = exp_log();

const fn mul_table() -> [[u8; 256]; 256] {
    let (exp, log) = EXP_LOG;
    let mut table = [[0u8; 256]; 256];
    let mut a = 1;
    while a < 256 {
        let mut b = 1;
        while b < 256 {
            table[a][b] = exp[log[a] as usize + log[b] as usize];
            b += 1;
        }
        a += 1;
    }
    table
}

const fn inverse_table() -> [u8; 256] {
    let (exp, log) = EXP_LOG;
    let mut inv = [0u8; 256];
    let mut a = 1;
    while a < 256 {
        inv[a] = exp[255 - log[a] as usize];
        a += 1;
    }
    inv
}

static MUL: [[u8; 256]; 256] = mul_table();
/// `INV[0]` is 0, as in nanors.
static INV: [u8; 256] = inverse_table();

/// Multiplies two field elements.
pub fn gf_mul(a: u8, b: u8) -> u8 {
    MUL[a as usize][b as usize]
}

/// `a ^= u * b`
fn axpy(a: &mut [u8], b: &[u8], u: u8) {
    match u {
        0 => {}
        1 => a.iter_mut().zip(b).for_each(|(a, b)| *a ^= b),
        _ => {
            let row = &MUL[u as usize];
            a.iter_mut()
                .zip(b)
                .for_each(|(a, b)| *a ^= row[*b as usize]);
        }
    }
}

/// `a *= u`
fn scal(a: &mut [u8], u: u8) {
    if u < 2 {
        return;
    }
    let row = &MUL[u as usize];
    a.iter_mut().for_each(|a| *a = row[*a as usize]);
}

/// Mutable `shards[to]` and shared `shards[from]`; `to != from`.
fn pair<'a>(shards: &'a mut [&mut [u8]], to: usize, from: usize) -> (&'a mut [u8], &'a [u8]) {
    debug_assert_ne!(to, from);
    if to < from {
        let (low, high) = shards.split_at_mut(from);
        (&mut *low[to], &*high[0])
    } else {
        let (low, high) = shards.split_at_mut(to);
        (&mut *high[0], &*low[from])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FecError {
    /// Fewer shards or a shorter shard than the code needs.
    BadShards,
    /// More shards are missing than parity shards were received.
    TooFewShards,
}

impl fmt::Display for FecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FecError::BadShards => "invalid shard set",
            FecError::TooFewShards => "too few shards to recover the data",
        })
    }
}

impl std::error::Error for FecError {}

/// A code with fixed data and parity shard counts.
#[derive(Clone)]
pub struct ReedSolomon {
    data: usize,
    parity: usize,
    /// `parity` rows of `data` coefficients.
    matrix: Vec<u8>,
}

impl ReedSolomon {
    /// The nanors code for `data` + `parity` shards; `None` outside nanors' limits
    /// (each count 1..=255, total at most 255).
    pub fn new(data: usize, parity: usize) -> Option<Self> {
        if data == 0 || parity == 0 || data + parity > MAX_SHARDS {
            return None;
        }
        let mut matrix = vec![0; data * parity];
        for (j, row) in matrix.chunks_exact_mut(data).enumerate() {
            for (i, value) in row.iter_mut().enumerate() {
                *value = INV[(parity + i) ^ j];
            }
        }
        Some(Self {
            data,
            parity,
            matrix,
        })
    }

    /// A code with an explicit row-major parity matrix (`parity` rows of `data` entries).
    pub fn with_parity_matrix(data: usize, parity: usize, matrix: &[u8]) -> Option<Self> {
        let mut code = Self::new(data, parity)?;
        if matrix.len() != data * parity {
            return None;
        }
        code.matrix.copy_from_slice(matrix);
        Some(code)
    }

    pub fn data_shards(&self) -> usize {
        self.data
    }

    pub fn parity_shards(&self) -> usize {
        self.parity
    }

    fn check(&self, shards: &[&mut [u8]], len: usize) -> Result<(), FecError> {
        if len == 0
            || shards.len() < self.data + self.parity
            || shards[..self.data + self.parity]
                .iter()
                .any(|s| s.len() < len)
        {
            return Err(FecError::BadShards);
        }
        Ok(())
    }

    /// Computes the parity shards `shards[data..data + parity]` from the first `len` bytes of
    /// the data shards.
    pub fn encode(&self, shards: &mut [&mut [u8]], len: usize) -> Result<(), FecError> {
        self.check(shards, len)?;
        let (data, parity) = shards.split_at_mut(self.data);
        for (row, out) in self.matrix.chunks_exact(self.data).zip(parity.iter_mut()) {
            let out = &mut out[..len];
            out.fill(0);
            for (input, &u) in data.iter().zip(row) {
                axpy(out, &input[..len], u);
            }
        }
        Ok(())
    }

    /// Recovers the data shards marked missing in `missing` (one flag per shard, data first)
    /// from the received data and parity shards, like `reed_solomon_decode`. Missing parity
    /// shards are not rebuilt.
    pub fn decode(
        &self,
        shards: &mut [&mut [u8]],
        missing: &[bool],
        len: usize,
    ) -> Result<(), FecError> {
        self.check(shards, len)?;
        let (ds, ts) = (self.data, self.data + self.parity);
        if missing.len() < ts {
            return Err(FecError::BadShards);
        }
        let mut erasures = [0u8; MAX_SHARDS];
        let mut colperm = [0u8; MAX_SHARDS];
        let mut rowperm = [0u8; MAX_SHARDS];
        let mut gaps = 0;
        for (i, _) in missing[..ds].iter().enumerate().filter(|(_, &m)| m) {
            erasures[gaps] = i as u8;
            gaps += 1;
        }
        let mut j = 0;
        for slot in colperm.iter_mut().take(ds - gaps) {
            while missing[j] {
                j += 1;
            }
            *slot = j as u8;
            j += 1;
        }
        for i in 0..gaps {
            colperm[ds - gaps + i] = erasures[i];
        }
        let mut j = ds;
        for i in 0..gaps {
            while j < ts && missing[j] {
                j += 1;
            }
            if j >= ts {
                return Err(FecError::TooFewShards);
            }
            rowperm[i] = (j - ds) as u8;
            let (to, from) = pair(shards, erasures[i] as usize, j);
            to[..len].copy_from_slice(&from[..len]);
            j += 1;
        }
        self.invert(shards, ds - gaps, len, &colperm, &rowperm);
        Ok(())
    }

    /// nanors' `invert_mat`: eliminates the known columns from the copied parity rows, then
    /// solves the remaining `W x W` system in place.
    fn invert(&self, dst: &mut [&mut [u8]], v0: usize, len: usize, c: &[u8], d: &[u8]) {
        let k = self.data;
        let src = &self.matrix;
        let w = k - v0;
        if w == 0 {
            return;
        }
        let mut wrk = vec![0u8; w * w];
        for i in 0..w {
            let dr = d[i] as usize * k;
            for j in 0..w {
                wrk[i * w + j] = src[dr + c[v0 + j] as usize];
            }
        }
        for v in v0..k {
            let dr = d[v - v0] as usize * k;
            for row in 0..v0 {
                let u = src[dr + c[row] as usize];
                let (to, from) = pair(dst, c[v] as usize, c[row] as usize);
                axpy(&mut to[..len], &from[..len], u);
            }
        }
        for x in 0..w {
            let u = INV[wrk[x * w + x] as usize];
            scal(&mut wrk[x * w + x..x * w + w], u);
            scal(&mut dst[c[v0 + x] as usize][..len], u);
            for row in x + 1..w {
                let u = wrk[row * w + x];
                let (pivot, rest) = wrk.split_at_mut(row * w);
                axpy(&mut rest[..w], &pivot[x * w..x * w + w], u);
                let (to, from) = pair(dst, c[v0 + row] as usize, c[v0 + x] as usize);
                axpy(&mut to[..len], &from[..len], u);
            }
        }
        for x in (0..w).rev() {
            for row in 0..x {
                let u = wrk[row * w + x];
                let (to, from) = pair(dst, c[v0 + row] as usize, c[v0 + x] as usize);
                axpy(&mut to[..len], &from[..len], u);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shards(count: usize, len: usize, seed: u32) -> Vec<Vec<u8>> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                (0..len)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (state >> 24) as u8
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn field_tables() {
        assert_eq!(EXP_LOG.0[8], 29);
        assert_eq!(INV[2], 142);
        for a in 1..=255u8 {
            assert_eq!(gf_mul(a, INV[a as usize]), 1);
        }
    }

    #[test]
    fn recovers_any_erasures_up_to_parity() {
        for (data, parity) in [(1, 1), (4, 2), (10, 3), (40, 8), (200, 55)] {
            let code = ReedSolomon::new(data, parity).unwrap();
            let len = 37;
            let mut set = shards(data + parity, len, (data * 31 + parity) as u32);
            let original = set.clone();
            let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
            code.encode(&mut refs, len).unwrap();
            let encoded = set.clone();
            let mut missing = vec![false; data + parity];
            for i in 0..parity {
                let lost = (i * 7 + 3) % (data + parity);
                missing[lost] = true;
                set[lost].fill(0xa5);
            }
            let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
            code.decode(&mut refs, &missing, len).unwrap();
            assert_eq!(set[..data], original[..data]);
            assert_eq!(set[..data], encoded[..data]);
        }
    }

    #[test]
    fn rejects_too_many_erasures() {
        let code = ReedSolomon::new(4, 2).unwrap();
        let mut set = shards(6, 8, 1);
        let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
        let missing = [true, true, false, false, true, false];
        assert_eq!(
            code.decode(&mut refs, &missing, 8),
            Err(FecError::TooFewShards)
        );
    }

    #[test]
    fn limits_match_nanors() {
        assert!(ReedSolomon::new(0, 1).is_none());
        assert!(ReedSolomon::new(1, 0).is_none());
        assert!(ReedSolomon::new(200, 56).is_none());
        assert!(ReedSolomon::new(200, 55).is_some());
        assert!(ReedSolomon::with_parity_matrix(4, 2, &AUDIO_PARITY_MATRIX).is_some());
    }
}
