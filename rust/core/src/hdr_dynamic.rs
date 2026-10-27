//! Dynamic HDR metadata for HEVC and AV1: SMPTE ST 2094-40 (HDR10+) and CUVA 005.1
//! (HDR Vivid, Experimental) payloads in registered ITU-T T.35 form, their carriage as an
//! HEVC prefix SEI or an AV1 metadata OBU, and the splice that puts them in a frame.
//!
//! Field semantics follow FFmpeg's parsers (`libavutil/hdr_dynamic_metadata.c`,
//! `libavcodec/dynamic_hdr_vivid.c`, `libavcodec/h2645_sei.c`, `libavcodec/libdav1d.c`).
//! Parts are ported from Foundation Sunshine (AlkaidLab/foundation-sunshine, commit
//! 880f1ef): `video_hdr_metadata.h`, `video_hdr_metadata.cpp` and `video_hdr_bitstream.cpp`.
//! Copyright (c) Foundation Sunshine authors, GPL-3.0.

/// The maxRGB percentages HDR10+ carries. [`FrameStats::percentiles_pq`] lines up with it.
pub const PERCENTAGES: [u8; 9] = [1, 5, 10, 25, 50, 75, 90, 95, 99];
const P10: usize = 2;
const P90: usize = 6;
/// ST 2094-40 8.5.4 reserves the 5% and 10% slots in application version 1 at
/// V1 = 0.00000 and V2 = 0.00255. A consumer that finds a nonzero V1 reads it as the frame
/// peak (ST 2094-50, libplacebo), so a real 5th percentile there would report the darkest
/// part of the frame as its peak. V1 = 0 tells it to use maxSCL.
const RESERVED: [(usize, u32); 2] = [(1, 0), (2, 255)];

const PQ_M1: f64 = 2610. / 16384.;
const PQ_M2: f64 = 2523. / 4096. * 128.;
const PQ_C1: f64 = 3424. / 4096.;
const PQ_C2: f64 = 2413. / 4096. * 32.;
const PQ_C3: f64 = 2392. / 4096. * 32.;

/// SMPTE ST 2084 (PQ) signal, 0..=1, for absolute luminance in nits.
pub fn nits_to_pq(nits: f32) -> f32 {
    if nits.is_nan() || nits <= 0. {
        return 0.;
    }
    let y = (f64::from(nits) / 10_000.).min(1.).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y) / (1. + PQ_C3 * y)).powf(PQ_M2) as f32
}

/// Absolute luminance in nits for a PQ signal value.
pub fn pq_to_nits(pq: f32) -> f32 {
    if pq.is_nan() || pq <= 0. {
        return 0.;
    }
    let e = f64::from(pq).min(1.).powf(1. / PQ_M2);
    let y = (e - PQ_C1).max(0.) / (PQ_C2 - PQ_C3 * e);
    (10_000. * y.powf(1. / PQ_M1)) as f32
}

/// One frame's maxRGB statistics, as the GPU analysis hands them to the metadata writers.
///
/// Everything but `mean_nits` is in the PQ signal domain (0..=1), where HDR Vivid's and
/// Dolby Vision's L1 statistics live too; HDR10+ converts back to linear light.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub min_pq: f32,
    /// Mean of PQ(maxRGB).
    pub mean_pq: f32,
    pub max_pq: f32,
    /// Linear-light mean of maxRGB in nits, which HDR10+ carries. It differs widely from
    /// the PQ mean on dark frames with small highlights.
    pub mean_nits: f32,
    /// maxRGB at each of [`PERCENTAGES`], PQ.
    pub percentiles_pq: [f32; 9],
}

impl FrameStats {
    /// Statistics from a histogram of PQ(maxRGB) in equal bins over 0..=1 (the GPU pass
    /// uses 1024) and the frame's exact minimum and maximum. Means come from bin centres,
    /// percentiles from the upper edge of the bin that reaches them, all kept inside the
    /// minimum and maximum. `None` for an empty histogram or bounds outside 0..=1.
    pub fn from_histogram(bins: &[u32], min_pq: f32, max_pq: f32) -> Option<Self> {
        let total: u64 = bins.iter().map(|&count| u64::from(count)).sum();
        if total == 0 || !(0. ..=1.).contains(&min_pq) || !(min_pq..=1.).contains(&max_pq) {
            return None;
        }
        let width = 1. / bins.len() as f64;
        let (mut pq_sum, mut nits_sum) = (0f64, 0f64);
        for (bin, &count) in bins.iter().enumerate().filter(|(_, count)| **count > 0) {
            let centre = (bin as f64 + 0.5) * width;
            pq_sum += centre * f64::from(count);
            nits_sum += f64::from(pq_to_nits(centre as f32)) * f64::from(count);
        }
        let mut percentiles_pq = [0.; 9];
        let (mut bin, mut below) = (0, 0u64);
        for (slot, &percentage) in PERCENTAGES.iter().enumerate() {
            let rank = (total * u64::from(percentage)).div_ceil(100);
            while below + u64::from(bins[bin]) < rank {
                below += u64::from(bins[bin]);
                bin += 1;
            }
            percentiles_pq[slot] = (((bin + 1) as f64 * width) as f32).clamp(min_pq, max_pq);
        }
        Some(Self {
            min_pq,
            mean_pq: ((pq_sum / total as f64) as f32).clamp(min_pq, max_pq),
            max_pq,
            mean_nits: ((nits_sum / total as f64) as f32)
                .clamp(pq_to_nits(min_pq), pq_to_nits(max_pq)),
            percentiles_pq,
        })
    }

    /// Every value finite and in range: PQ values in 0..=1, `mean_nits` in 0..=10000.
    pub fn is_valid(&self) -> bool {
        let unit = |value: f32| (0. ..=1.).contains(&value);
        unit(self.min_pq)
            && unit(self.mean_pq)
            && unit(self.max_pq)
            && self.percentiles_pq.iter().all(|&value| unit(value))
            && (0. ..=10_000.).contains(&self.mean_nits)
    }
}

/// The peak luminance dynamic metadata targets: the client display's reported peak, else
/// the host display's, else 1000 nits, within 1..=10000.
pub fn target_display_nits(client_peak: Option<f32>, host_peak: Option<f32>) -> u16 {
    [client_peak, host_peak]
        .into_iter()
        .flatten()
        .find(|nits| nits.is_finite() && *nits >= 1.)
        .map_or(1000, |nits| nits.min(10_000.).round() as u16)
}

/// One window of HDR10+ metadata as transmitted.
///
/// Every luminance field is normalized to the 10000-nit PQ peak in units of 1/100000,
/// never to the target display: consumers multiply straight back out by 10000.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hdr10Plus {
    /// targeted_system_display_maximum_luminance, nits.
    pub target_nits: u16,
    /// maxscl, carried for all three components.
    pub maxscl: u32,
    pub average_maxrgb: u32,
    /// distribution_maxrgb percentile at each of [`PERCENTAGES`].
    pub distribution: [u32; 9],
}

impl Hdr10Plus {
    /// `None` for invalid statistics.
    pub fn from_stats(stats: &FrameStats, target_nits: u16) -> Option<Self> {
        if !stats.is_valid() {
            return None;
        }
        let normalize = |nits: f32| ((nits / 10_000.).clamp(0., 1.) * 100_000.).round() as u32;
        let mut distribution = stats.percentiles_pq.map(|pq| normalize(pq_to_nits(pq)));
        let average_maxrgb = normalize(stats.mean_nits);
        // maxSCL is the 99th percentile, not the true maximum: compositor overshoot and a
        // few specular pixels would otherwise pin it near 10000 nits and make every
        // consumer tone-map the whole scene against that. It never sits below the average
        // or a percentile carried beside it; the reserved slots are not measurements.
        let maxscl = (0..PERCENTAGES.len())
            .filter(|slot| RESERVED.iter().all(|(reserved, _)| reserved != slot))
            .map(|slot| distribution[slot])
            .fold(average_maxrgb, u32::max);
        for (slot, value) in RESERVED {
            distribution[slot] = value;
        }
        Some(Self {
            target_nits: target_nits.clamp(1, 10_000),
            maxscl,
            average_maxrgb,
            distribution,
        })
    }

    /// The complete T.35 payload, from the country code on: one full-frame window, no
    /// tone-mapping curve.
    pub fn to_t35(&self) -> Vec<u8> {
        // Country US, provider 0x003C, provider-oriented code 0x0001, application
        // identifier 4 (ST 2094-40), application version 1.
        let mut bits = BitWriter::with_prefix(&[0xB5, 0x00, 0x3C, 0x00, 0x01, 4, 1]);
        bits.put(2, 1); // num_windows
        bits.put(27, u32::from(self.target_nits.min(10_000)));
        bits.put(1, 0); // targeted_system_display_actual_peak_luminance_flag
        for _ in 0..3 {
            bits.put(17, self.maxscl.min(100_000));
        }
        bits.put(17, self.average_maxrgb.min(100_000));
        bits.put(4, PERCENTAGES.len() as u32);
        for (&percentage, &percentile) in PERCENTAGES.iter().zip(&self.distribution) {
            bits.put(7, u32::from(percentage));
            bits.put(17, percentile.min(100_000));
        }
        bits.put(10, 0); // fraction_bright_pixels
        bits.put(1, 0); // mastering_display_actual_peak_luminance_flag
        bits.put(1, 0); // tone_mapping_flag
        bits.put(1, 0); // color_saturation_mapping_flag
        bits.bytes
    }
}

/// Statistics-only HDR Vivid metadata (one window, no tone-mapping curve), as 12-bit PQ
/// codes over 4095. Experimental: no device here decodes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vivid {
    pub minimum_maxrgb: u16,
    /// The PQ-domain mean: PQ is concave, so PQ(linear mean) would anchor a display's
    /// curve far too high on dark frames with small highlights.
    pub average_maxrgb: u16,
    /// The PQ spread between the 90th and 10th maxRGB percentiles, as Foundation Sunshine
    /// carries it.
    pub variance_maxrgb: u16,
    pub maximum_maxrgb: u16,
}

impl Vivid {
    /// `None` for invalid statistics.
    pub fn from_stats(stats: &FrameStats) -> Option<Self> {
        if !stats.is_valid() {
            return None;
        }
        let code = |pq: f32| (pq.clamp(0., 1.) * 4095.).round() as u16;
        let spread = stats.percentiles_pq[P90] - stats.percentiles_pq[P10];
        Some(Self {
            minimum_maxrgb: code(stats.min_pq),
            average_maxrgb: code(stats.mean_pq),
            variance_maxrgb: code(spread.max(0.)),
            maximum_maxrgb: code(stats.max_pq),
        })
    }

    /// The complete T.35 payload, from the country code on.
    pub fn to_t35(&self) -> Vec<u8> {
        // Country China, provider 0x0004 (CUVA), provider-oriented code 0x0005,
        // system_start_code 1.
        let mut bits = BitWriter::with_prefix(&[0x26, 0x00, 0x04, 0x00, 0x05, 0x01]);
        for code in [
            self.minimum_maxrgb,
            self.average_maxrgb,
            self.variance_maxrgb,
            self.maximum_maxrgb,
        ] {
            bits.put(12, u32::from(code.min(4095)));
        }
        bits.put(1, 0); // tone_mapping_mode_flag
        bits.put(1, 0); // color_saturation_mapping_flag
        bits.bytes
    }
}

/// MSB-first bit writer; the last byte is padded with zeros.
struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    fn with_prefix(prefix: &[u8]) -> Self {
        Self {
            bytes: prefix.to_vec(),
            used: 0,
        }
    }

    fn put(&mut self, count: u32, value: u32) {
        debug_assert!(count < 32 && value >> count == 0, "{value} in {count} bits");
        for shift in (0..count).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= (((value >> shift) & 1) as u8) << (7 - self.used);
            self.used = (self.used + 1) % 8;
        }
    }
}

/// The codecs that carry dynamic HDR metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Hevc,
    Av1,
}

impl Codec {
    /// From the video format index `codec_support` uses (1 HEVC, 2 AV1). H.264 gets none.
    pub fn from_video_format(format: u8) -> Option<Self> {
        match format {
            1 => Some(Self::Hevc),
            2 => Some(Self::Av1),
            _ => None,
        }
    }

    /// One T.35 payload in this codec's carriage.
    pub fn t35_unit(self, t35: &[u8]) -> Vec<u8> {
        match self {
            Self::Hevc => hevc_t35_sei(t35),
            Self::Av1 => av1_t35_obu(t35),
        }
    }
}

/// Every dynamic-metadata unit for one frame of a PQ stream, concatenated for [`splice`]:
/// HDR10+, and on HEVC also HDR Vivid, which defines a carriage for HEVC but none for AV1.
/// `None` for invalid statistics.
pub fn frame_units(codec: Codec, stats: &FrameStats, target_nits: u16) -> Option<Vec<u8>> {
    let mut units = codec.t35_unit(&Hdr10Plus::from_stats(stats, target_nits)?.to_t35());
    if codec == Codec::Hevc {
        units.extend(codec.t35_unit(&Vivid::from_stats(stats)?.to_t35()));
    }
    Some(units)
}

const HEVC_PREFIX_SEI: u8 = 39;
const HEVC_LAST_VCL: u8 = 31;
const SEI_USER_DATA_REGISTERED_ITU_T_T35: usize = 4;
const OBU_FRAME_HEADER: u8 = 3;
const OBU_METADATA: u8 = 5;
const OBU_TILE_GROUP: u8 = 4;
const OBU_FRAME: u8 = 6;
const OBU_REDUNDANT_FRAME_HEADER: u8 = 7;
const METADATA_TYPE_ITUT_T35: usize = 4;

/// An Annex B prefix SEI NAL unit (four-byte start code, layer 0, TemporalId 0) holding
/// one user_data_registered_itu_t_t35 message, with emulation prevention applied.
pub fn hevc_t35_sei(t35: &[u8]) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(t35.len() + 8);
    push_ff_coded(SEI_USER_DATA_REGISTERED_ITU_T_T35, &mut rbsp);
    push_ff_coded(t35.len(), &mut rbsp);
    rbsp.extend_from_slice(t35);
    rbsp.push(0x80); // rbsp_trailing_bits
    let mut nal = vec![0, 0, 0, 1, HEVC_PREFIX_SEI << 1, 0x01];
    escape(&rbsp, &mut nal);
    nal
}

/// An AV1 metadata OBU (metadata_type ITU-T T.35) with a size field and no extension.
pub fn av1_t35_obu(t35: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(t35.len() + 2);
    push_leb128(METADATA_TYPE_ITUT_T35, &mut payload);
    payload.extend_from_slice(t35);
    payload.push(0x80); // trailing_bits
    let mut obu = vec![(OBU_METADATA << 3) | 0x02];
    push_leb128(payload.len(), &mut obu);
    obu.extend(payload);
    obu
}

/// Where metadata units belong in one encoded frame. HEVC (Annex B): before the first VCL
/// NAL's start code, after any AUD, parameter sets and prefix SEIs. AV1 (OBUs with size
/// fields): before the first frame, frame header or tile group OBU, after the temporal
/// delimiter and sequence header. `None` when the frame has no such point or cannot be
/// walked (no start codes, forbidden bit set, OBUs without sizes, truncated sizes).
pub fn insert_offset(codec: Codec, frame: &[u8]) -> Option<usize> {
    match codec {
        Codec::Hevc => hevc_insert_offset(frame),
        Codec::Av1 => av1_insert_offset(frame),
    }
}

/// Inserts `units` at [`insert_offset`]. Returns false and leaves `frame` unchanged when
/// there is no insertion point or nothing to insert. Callers that would rather not move
/// the frame's bytes can send `frame[..offset]`, the units and `frame[offset..]` instead.
pub fn splice(codec: Codec, frame: &mut Vec<u8>, units: &[u8]) -> bool {
    match insert_offset(codec, frame) {
        Some(at) if !units.is_empty() => {
            frame.splice(at..at, units.iter().copied());
            true
        }
        _ => false,
    }
}

fn hevc_insert_offset(frame: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(code) = find_start_code(frame, from) {
        let header = *frame.get(code + 3)?;
        if header & 0x80 != 0 {
            return None; // forbidden_zero_bit: not an HEVC NAL unit
        }
        if (header >> 1) & 0x3F <= HEVC_LAST_VCL {
            // Leading zero_byte included, so the slice keeps its four-byte start code.
            let mut at = code;
            while at > 0 && frame[at - 1] == 0 {
                at -= 1;
            }
            return Some(at);
        }
        from = code + 4;
    }
    None
}

fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    data.get(from..)?
        .windows(3)
        .position(|window| window == [0, 0, 1])
        .map(|at| from + at)
}

fn av1_insert_offset(frame: &[u8]) -> Option<usize> {
    let mut at = 0;
    while let Some(&header) = frame.get(at) {
        if header & 0x80 != 0 {
            return None; // obu_forbidden_bit
        }
        if matches!(
            (header >> 3) & 0x0F,
            OBU_FRAME_HEADER | OBU_TILE_GROUP | OBU_FRAME | OBU_REDUNDANT_FRAME_HEADER
        ) {
            return Some(at);
        }
        if header & 0x02 == 0 {
            return None; // no obu_has_size_field: the sizes live in a container
        }
        let mut next = at + 1 + usize::from(header & 0x04 != 0);
        let size = usize::try_from(read_leb128(frame, &mut next)?).ok()?;
        at = next.checked_add(size).filter(|&end| end <= frame.len())?;
    }
    None
}

/// Emulation prevention: an 0x03 after every two zero bytes that precede a byte <= 3.
fn escape(rbsp: &[u8], out: &mut Vec<u8>) {
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros == 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

fn push_ff_coded(mut value: usize, out: &mut Vec<u8>) {
    while value >= 0xFF {
        out.push(0xFF);
        value -= 0xFF;
    }
    out.push(value as u8);
}

fn push_leb128(mut value: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_leb128(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0;
    for index in 0..8 {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7F) << (index * 7);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Smooths the statistics over time so dynamic metadata does not step at every analysis
/// sample and make the picture pump, and snaps to a new scene at a cut. One per stream:
/// a new stream starts fresh.
#[derive(Clone, Debug, Default)]
pub struct StatsSmoother {
    smoothed: Option<FrameStats>,
    last: Option<FrameStats>,
}

/// What [`StatsSmoother::update`] returns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Smoothed {
    pub stats: FrameStats,
    /// The sample started a scene (the first sample, or a cut) and was taken as it is.
    pub scene_cut: bool,
}

impl StatsSmoother {
    /// Weight of each new sample between cuts.
    pub const ALPHA: f32 = 0.15;

    /// Feed one new analysis sample, not one per encoded frame: repeating a sample would
    /// speed the smoothing up. `None` for invalid statistics, which leave the state alone.
    pub fn update(&mut self, raw: &FrameStats) -> Option<Smoothed> {
        if !raw.is_valid() {
            return None;
        }
        let scene_cut = self.last.is_none_or(|last| is_scene_cut(&last, raw));
        let stats = match self.smoothed {
            Some(previous) if !scene_cut => blend(&previous, raw, Self::ALPHA),
            _ => *raw,
        };
        self.last = Some(*raw);
        self.smoothed = Some(stats);
        Some(Smoothed { stats, scene_cut })
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// A cut needs the mean, the low or high end, or the whole distribution to move in PQ
/// (Foundation Sunshine's thresholds). A lone peak is not enough: cursors, subtitles and
/// compositor overshoot all make one.
fn is_scene_cut(previous: &FrameStats, current: &FrameStats) -> bool {
    let delta = |slot: usize| (current.percentiles_pq[slot] - previous.percentiles_pq[slot]).abs();
    let mean = (current.mean_pq - previous.mean_pq).abs();
    let ends = delta(P10).max(delta(P90));
    let (sum, largest) = (0..PERCENTAGES.len())
        .map(delta)
        .fold((0f32, 0f32), |(sum, largest), d| (sum + d, largest.max(d)));
    let spread = sum / PERCENTAGES.len() as f32;
    mean >= 0.10
        || ends >= 0.18
        || (spread >= 0.10 && largest >= 0.14)
        || (mean >= 0.06 && spread >= 0.06)
}

fn blend(previous: &FrameStats, raw: &FrameStats, alpha: f32) -> FrameStats {
    let mix = |old: f32, new: f32| old + alpha * (new - old);
    FrameStats {
        min_pq: mix(previous.min_pq, raw.min_pq),
        mean_pq: mix(previous.mean_pq, raw.mean_pq),
        max_pq: mix(previous.max_pq, raw.max_pq),
        mean_nits: mix(previous.mean_nits, raw.mean_nits),
        percentiles_pq: std::array::from_fn(|slot| {
            mix(previous.percentiles_pq[slot], raw.percentiles_pq[slot])
        }),
    }
}

#[cfg(test)]
mod tests;
