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
/// Most erasures a decode solves for: no more than either the data or the parity shards.
const MAX_ERASURES: usize = MAX_SHARDS / 2;

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

/// Per coefficient `u`, the products of every low nibble (`[0][n] = u * n`) and every high
/// nibble (`[1][n] = u * (n << 4)`). Multiplication distributes over XOR, so
/// `u * b == [0][b & 15] ^ [1][b >> 4]`: the 16-byte tables the SIMD kernels shuffle, like
/// nanors' oblas_lite.
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    allow(dead_code)
)]
const fn nibble_tables() -> [[[u8; 16]; 2]; 256] {
    let (exp, log) = EXP_LOG;
    let mut table = [[[0u8; 16]; 2]; 256];
    let mut u = 1;
    while u < 256 {
        let mut n = 1;
        while n < 16 {
            table[u][0][n] = exp[log[u] as usize + log[n] as usize];
            table[u][1][n] = exp[log[u] as usize + log[n << 4] as usize];
            n += 1;
        }
        u += 1;
    }
    table
}

static MUL: [[u8; 256]; 256] = mul_table();
/// `INV[0]` is 0, as in nanors.
static INV: [u8; 256] = inverse_table();
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    allow(dead_code)
)]
static NIBBLES: [[[u8; 16]; 2]; 256] = nibble_tables();

/// Multiplies two field elements.
pub fn gf_mul(a: u8, b: u8) -> u8 {
    MUL[a as usize][b as usize]
}

/// The vector unit `axpy` and `scal` use; the scalar table loop finishes the tail bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kernel {
    /// AArch64 always has NEON; there only the tests run the scalar loop alone.
    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Ssse3,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

impl Kernel {
    /// The widest kernel this CPU runs. std caches the CPUID probe.
    fn best() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                Kernel::Avx2
            } else if is_x86_feature_detected!("ssse3") {
                Kernel::Ssse3
            } else {
                Kernel::Scalar
            }
        }
        // NEON is part of the AArch64 baseline.
        #[cfg(target_arch = "aarch64")]
        {
            Kernel::Neon
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Kernel::Scalar
        }
    }
}

/// `a ^= u * b`
fn axpy(kernel: Kernel, a: &mut [u8], b: &[u8], u: u8) {
    let len = a.len().min(b.len());
    let (a, b) = (&mut a[..len], &b[..len]);
    match u {
        0 => {}
        1 => a.iter_mut().zip(b).for_each(|(a, b)| *a ^= b),
        _ => {
            // SAFETY: `best` (or a test that probed the CPU) picked a kernel this CPU runs, and
            // `a` and `b` have the same length.
            let done = match kernel {
                Kernel::Scalar => 0,
                #[cfg(target_arch = "x86_64")]
                Kernel::Ssse3 => unsafe { x86::axpy_ssse3(a, b, &NIBBLES[u as usize]) },
                #[cfg(target_arch = "x86_64")]
                Kernel::Avx2 => unsafe { x86::axpy_avx2(a, b, &NIBBLES[u as usize]) },
                #[cfg(target_arch = "aarch64")]
                Kernel::Neon => unsafe { neon::axpy(a, b, &NIBBLES[u as usize]) },
            };
            let row = &MUL[u as usize];
            a[done..]
                .iter_mut()
                .zip(&b[done..])
                .for_each(|(a, b)| *a ^= row[*b as usize]);
        }
    }
}

/// `a *= u`
fn scal(kernel: Kernel, a: &mut [u8], u: u8) {
    if u < 2 {
        return;
    }
    // SAFETY: as in `axpy`.
    let done = match kernel {
        Kernel::Scalar => 0,
        #[cfg(target_arch = "x86_64")]
        Kernel::Ssse3 => unsafe { x86::scal_ssse3(a, &NIBBLES[u as usize]) },
        #[cfg(target_arch = "x86_64")]
        Kernel::Avx2 => unsafe { x86::scal_avx2(a, &NIBBLES[u as usize]) },
        #[cfg(target_arch = "aarch64")]
        Kernel::Neon => unsafe { neon::scal(a, &NIBBLES[u as usize]) },
    };
    let row = &MUL[u as usize];
    a[done..].iter_mut().for_each(|a| *a = row[*a as usize]);
}

/// Split-nibble `pshufb` kernels. Each handles whole 16- or 32-byte blocks and returns how many
/// bytes it did; callers guarantee the CPU feature and `a.len() == b.len()`.
#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;

    #[inline]
    #[target_feature(enable = "ssse3")]
    unsafe fn mul128(x: __m128i, lo: __m128i, hi: __m128i) -> __m128i {
        let mask = _mm_set1_epi8(0x0f);
        let l = _mm_and_si128(x, mask);
        let h = _mm_and_si128(_mm_srli_epi64::<4>(x), mask);
        _mm_xor_si128(_mm_shuffle_epi8(lo, l), _mm_shuffle_epi8(hi, h))
    }

    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn mul256(x: __m256i, lo: __m256i, hi: __m256i) -> __m256i {
        let mask = _mm256_set1_epi8(0x0f);
        let l = _mm256_and_si256(x, mask);
        let h = _mm256_and_si256(_mm256_srli_epi64::<4>(x), mask);
        _mm256_xor_si256(_mm256_shuffle_epi8(lo, l), _mm256_shuffle_epi8(hi, h))
    }

    #[target_feature(enable = "ssse3")]
    pub unsafe fn axpy_ssse3(a: &mut [u8], b: &[u8], t: &[[u8; 16]; 2]) -> usize {
        let lo = _mm_loadu_si128(t[0].as_ptr().cast());
        let hi = _mm_loadu_si128(t[1].as_ptr().cast());
        let end = a.len() & !15;
        let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
        let mut i = 0;
        while i < end {
            let x = _mm_loadu_si128(pb.add(i).cast());
            let y = _mm_loadu_si128(pa.add(i).cast());
            _mm_storeu_si128(pa.add(i).cast(), _mm_xor_si128(y, mul128(x, lo, hi)));
            i += 16;
        }
        end
    }

    #[target_feature(enable = "ssse3")]
    pub unsafe fn scal_ssse3(a: &mut [u8], t: &[[u8; 16]; 2]) -> usize {
        let lo = _mm_loadu_si128(t[0].as_ptr().cast());
        let hi = _mm_loadu_si128(t[1].as_ptr().cast());
        let end = a.len() & !15;
        let pa = a.as_mut_ptr();
        let mut i = 0;
        while i < end {
            let x = _mm_loadu_si128(pa.add(i).cast());
            _mm_storeu_si128(pa.add(i).cast(), mul128(x, lo, hi));
            i += 16;
        }
        end
    }

    /// 32-byte blocks, then one 16-byte block.
    #[target_feature(enable = "avx2")]
    pub unsafe fn axpy_avx2(a: &mut [u8], b: &[u8], t: &[[u8; 16]; 2]) -> usize {
        let lo = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[0].as_ptr().cast()));
        let hi = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[1].as_ptr().cast()));
        let end = a.len() & !31;
        let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
        let mut i = 0;
        while i < end {
            let x = _mm256_loadu_si256(pb.add(i).cast());
            let y = _mm256_loadu_si256(pa.add(i).cast());
            _mm256_storeu_si256(pa.add(i).cast(), _mm256_xor_si256(y, mul256(x, lo, hi)));
            i += 32;
        }
        end + axpy_ssse3(&mut a[end..], &b[end..], t)
    }

    /// 32-byte blocks, then one 16-byte block.
    #[target_feature(enable = "avx2")]
    pub unsafe fn scal_avx2(a: &mut [u8], t: &[[u8; 16]; 2]) -> usize {
        let lo = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[0].as_ptr().cast()));
        let hi = _mm256_broadcastsi128_si256(_mm_loadu_si128(t[1].as_ptr().cast()));
        let end = a.len() & !31;
        let pa = a.as_mut_ptr();
        let mut i = 0;
        while i < end {
            let x = _mm256_loadu_si256(pa.add(i).cast());
            _mm256_storeu_si256(pa.add(i).cast(), mul256(x, lo, hi));
            i += 32;
        }
        end + scal_ssse3(&mut a[end..], t)
    }
}

/// Split-nibble `tbl` kernels on 16-byte blocks; same contract as the x86 ones.
#[cfg(target_arch = "aarch64")]
mod neon {
    use std::arch::aarch64::*;

    #[inline]
    unsafe fn mul(x: uint8x16_t, lo: uint8x16_t, hi: uint8x16_t) -> uint8x16_t {
        let l = vandq_u8(x, vdupq_n_u8(0x0f));
        let h = vshrq_n_u8::<4>(x);
        veorq_u8(vqtbl1q_u8(lo, l), vqtbl1q_u8(hi, h))
    }

    pub unsafe fn axpy(a: &mut [u8], b: &[u8], t: &[[u8; 16]; 2]) -> usize {
        let lo = vld1q_u8(t[0].as_ptr());
        let hi = vld1q_u8(t[1].as_ptr());
        let end = a.len() & !15;
        let (pa, pb) = (a.as_mut_ptr(), b.as_ptr());
        let mut i = 0;
        while i < end {
            let x = vld1q_u8(pb.add(i));
            let y = vld1q_u8(pa.add(i));
            vst1q_u8(pa.add(i), veorq_u8(y, mul(x, lo, hi)));
            i += 16;
        }
        end
    }

    pub unsafe fn scal(a: &mut [u8], t: &[[u8; 16]; 2]) -> usize {
        let lo = vld1q_u8(t[0].as_ptr());
        let hi = vld1q_u8(t[1].as_ptr());
        let end = a.len() & !15;
        let pa = a.as_mut_ptr();
        let mut i = 0;
        while i < end {
            let x = vld1q_u8(pa.add(i));
            vst1q_u8(pa.add(i), mul(x, lo, hi));
            i += 16;
        }
        end
    }
}

/// Mutable `shards[to]` and shared `shards[from]`; `to != from`.
fn pair<S: AsMut<[u8]> + AsRef<[u8]>>(
    shards: &mut [S],
    to: usize,
    from: usize,
) -> (&mut [u8], &[u8]) {
    debug_assert_ne!(to, from);
    if to < from {
        let (low, high) = shards.split_at_mut(from);
        (low[to].as_mut(), high[0].as_ref())
    } else {
        let (low, high) = shards.split_at_mut(to);
        (high[0].as_mut(), low[from].as_ref())
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
        let mut code = Self {
            data: 0,
            parity: 0,
            matrix: Vec::new(),
        };
        code.renew(data, parity).then_some(code)
    }

    /// Makes this the nanors code for `data` + `parity` shards, in the memory it has. False,
    /// and unchanged, outside nanors' limits.
    pub fn renew(&mut self, data: usize, parity: usize) -> bool {
        if data == 0 || parity == 0 || data + parity > MAX_SHARDS {
            return false;
        }
        self.data = data;
        self.parity = parity;
        self.matrix.clear();
        self.matrix.resize(data * parity, 0);
        for (j, row) in self.matrix.chunks_exact_mut(data).enumerate() {
            for (i, value) in row.iter_mut().enumerate() {
                *value = INV[(parity + i) ^ j];
            }
        }
        true
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

    fn check<S: AsRef<[u8]>>(&self, shards: &[S], len: usize) -> Result<(), FecError> {
        if len == 0
            || shards.len() < self.data + self.parity
            || shards[..self.data + self.parity]
                .iter()
                .any(|s| s.as_ref().len() < len)
        {
            return Err(FecError::BadShards);
        }
        Ok(())
    }

    /// Computes the parity shards `shards[data..data + parity]` from the first `len` bytes of
    /// the data shards.
    pub fn encode(&self, shards: &mut [&mut [u8]], len: usize) -> Result<(), FecError> {
        self.check(shards, len)?;
        let kernel = Kernel::best();
        let (data, parity) = shards.split_at_mut(self.data);
        for (row, out) in self.matrix.chunks_exact(self.data).zip(parity.iter_mut()) {
            let out = &mut out[..len];
            out.fill(0);
            for (input, &u) in data.iter().zip(row) {
                axpy(kernel, out, &input[..len], u);
            }
        }
        Ok(())
    }

    /// Recovers the data shards marked missing in `missing` (one flag per shard, data first)
    /// from the received data and parity shards, like `reed_solomon_decode`. Missing parity
    /// shards are not rebuilt. Allocates nothing.
    pub fn decode<S: AsMut<[u8]> + AsRef<[u8]>>(
        &self,
        shards: &mut [S],
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
        self.invert(Kernel::best(), shards, ds - gaps, len, &colperm, &rowperm);
        Ok(())
    }

    /// nanors' `invert_mat`: eliminates the known columns from the copied parity rows, then
    /// solves the remaining `W x W` system in place.
    fn invert<S: AsMut<[u8]> + AsRef<[u8]>>(
        &self,
        kernel: Kernel,
        dst: &mut [S],
        v0: usize,
        len: usize,
        c: &[u8],
        d: &[u8],
    ) {
        let k = self.data;
        let src = &self.matrix;
        let w = k - v0;
        if w == 0 {
            return;
        }
        // Each erasure took a parity shard, so `w` is at most the smaller shard count.
        debug_assert!(w <= MAX_ERASURES);
        let mut work = [0u8; MAX_ERASURES * MAX_ERASURES];
        let wrk = &mut work[..w * w];
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
                axpy(kernel, &mut to[..len], &from[..len], u);
            }
        }
        for x in 0..w {
            let u = INV[wrk[x * w + x] as usize];
            scal(kernel, &mut wrk[x * w + x..x * w + w], u);
            scal(kernel, &mut dst[c[v0 + x] as usize].as_mut()[..len], u);
            for row in x + 1..w {
                let u = wrk[row * w + x];
                let (pivot, rest) = wrk.split_at_mut(row * w);
                axpy(kernel, &mut rest[..w], &pivot[x * w..x * w + w], u);
                let (to, from) = pair(dst, c[v0 + row] as usize, c[v0 + x] as usize);
                axpy(kernel, &mut to[..len], &from[..len], u);
            }
        }
        for x in (0..w).rev() {
            for row in 0..x {
                let u = wrk[row * w + x];
                let (to, from) = pair(dst, c[v0 + row] as usize, c[v0 + x] as usize);
                axpy(kernel, &mut to[..len], &from[..len], u);
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

    /// Every vector kernel this CPU runs.
    fn simd_kernels() -> Vec<Kernel> {
        #[cfg(target_arch = "x86_64")]
        {
            let mut kernels = Vec::new();
            if is_x86_feature_detected!("ssse3") {
                kernels.push(Kernel::Ssse3);
            }
            if is_x86_feature_detected!("avx2") {
                kernels.push(Kernel::Avx2);
            }
            kernels
        }
        #[cfg(target_arch = "aarch64")]
        {
            vec![Kernel::Neon]
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Vec::new()
        }
    }

    /// Runs `axpy` and `scal` with `kernel` and with the scalar table loop on the same inputs.
    fn assert_matches_scalar(kernel: Kernel, a: &[u8], b: &[u8], u: u8) {
        let (mut want, mut got) = (a.to_vec(), a.to_vec());
        axpy(Kernel::Scalar, &mut want, b, u);
        axpy(kernel, &mut got, b, u);
        assert_eq!(got, want, "{kernel:?} axpy u {u} len {}", a.len());
        let (mut want, mut got) = (a.to_vec(), a.to_vec());
        scal(Kernel::Scalar, &mut want, u);
        scal(kernel, &mut got, u);
        assert_eq!(got, want, "{kernel:?} scal u {u} len {}", a.len());
    }

    #[test]
    fn nibble_tables_split_every_product() {
        for u in 0..=255usize {
            for b in 0..=255usize {
                let split = NIBBLES[u][0][b & 15] ^ NIBBLES[u][1][b >> 4];
                assert_eq!(split, MUL[u][b], "u {u} b {b}");
            }
        }
    }

    #[test]
    fn simd_matches_scalar_for_every_coefficient() {
        let kernels = simd_kernels();
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        assert!(!kernels.is_empty());
        let set = shards(2, 1392 + 3, 11);
        for kernel in kernels {
            for u in 0..=255u8 {
                for len in [0, 1, 15, 16, 17, 31, 32, 33, 47, 48, 63, 64, 65, 1392] {
                    for offset in [0, 3] {
                        let a = &set[0][offset..offset + len];
                        let b = &set[1][3 - offset..3 - offset + len];
                        assert_matches_scalar(kernel, a, b, u);
                    }
                }
            }
        }
    }

    #[test]
    fn simd_matches_scalar_for_every_length() {
        let set = shards(2, 4100 + 3, 23);
        let mut state = 5u32;
        for kernel in simd_kernels() {
            for len in 0..=4100 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let u = (state >> 24) as u8;
                let (offset_a, offset_b) = (len % 4, (len / 4) % 3);
                let a = &set[0][offset_a..offset_a + len];
                let b = &set[1][offset_b..offset_b + len];
                assert_matches_scalar(kernel, a, b, u);
            }
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

    /// Microseconds per recovery of video-sized shards (1392 bytes, 20 % parity). Run in release:
    /// `cargo test --release -p nightjar-core --lib fec::tests::bench_recovery -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_recovery() {
        const LEN: usize = 1392;
        for (data, erasures) in [(100, 5), (100, 20), (200, 5), (200, 20)] {
            let parity = data / 5;
            let code = ReedSolomon::new(data, parity).unwrap();
            let mut set = shards(data + parity, LEN, data as u32);
            let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
            code.encode(&mut refs, LEN).unwrap();
            let encoded = set.clone();
            let mut missing = vec![false; data + parity];
            for i in 0..erasures {
                missing[i * data / erasures] = true;
            }
            let lost: Vec<usize> = (0..data).filter(|&i| missing[i]).collect();
            let iterations = 4_000_000 / (data * erasures);
            let mut best = f64::MAX;
            for _ in 0..5 {
                let start = std::time::Instant::now();
                for _ in 0..iterations {
                    for &i in &lost {
                        set[i].fill(0);
                    }
                    code.decode(&mut set, &missing, LEN).unwrap();
                }
                let us = start.elapsed().as_secs_f64() * 1e6 / iterations as f64;
                best = best.min(us);
                assert_eq!(set, encoded);
            }
            println!("data {data:3} erasures {erasures:2}: {best:8.1} us per recovery");
        }
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
