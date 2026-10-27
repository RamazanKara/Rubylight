//! Dolby Vision profile 8.1: one RPU (reference processing unit) per frame, as
//! the HEVC NAL unit of type 62 that ends the access unit. Profile 8.1 is an
//! HDR10 stream (HEVC Main10, PQ, BT.2020) with an RPU beside every frame, so a
//! decoder without Dolby Vision plays the same bytes as HDR10.
//!
//! The RPU maps the base layer onto itself (an identity polynomial: the picture
//! is the Dolby Vision signal), carries the standard PQ display-management
//! matrices, and three CM v2.9 extension blocks: L1 (the frame's minimum,
//! average and maximum PQ of maxRGB), L2 (neutral trims for the target display)
//! and L6 (the stream's static HDR10 metadata).
//!
//! Syntax, default values and the L1 limits follow the `dolby_vision` crate of
//! dovi_tool (quietvoid/dovi_tool 3.4.0: `rpu/rpu_data_header.rs`,
//! `rpu/rpu_data_mapping.rs`, `rpu/vdr_dm_data.rs`, `rpu/profiles/profile81.rs`,
//! `rpu/extension_metadata/blocks`), Copyright (c) quietvoid, MIT. FFmpeg's
//! `libavcodec/dovi_rpudec.c` reads the same syntax.
use crate::hdr::Metadata;
use crate::hdr_dynamic::{FrameStats, Smoothed, escape, nits_to_pq};

/// HEVC NAL unit type 62 (unspecified), layer 0, TemporalId 0.
const NAL_HEADER: [u8; 2] = [62 << 1, 0x01];
const RPU_PREFIX: u8 = 0x19;
const RPU_FINAL_BYTE: u8 = 0x80;

/// L1 limits of CM v2.9, as dovi_tool clamps measured statistics.
const L1_MIN_PQ_MAX: u16 = 12;
const L1_MAX_PQ_MIN: u16 = 2081;
const L1_AVG_PQ_MIN: u16 = 819;
const PQ12_MAX: u16 = 4095;
/// The average a frame without a measurement gets when the stream names no
/// MaxFALL: HDR reference white (ITU-R BT.2408).
const REFERENCE_WHITE_NITS: f32 = 203.;
/// Neutral L2 trims (no change for the target display).
const L2_NEUTRAL: u16 = 2048;
/// The source display diagonal in inches, as profile 8.1 RPUs carry it.
const SOURCE_DIAGONAL: u16 = 42;

/// YCbCr to RGB (BT.2020, limited range) and RGB to LMS, in the 16-bit fixed
/// point of profile 8.1's display-management data.
const YCC_TO_RGB: [i16; 9] = [9574, 0, 13802, 9574, -1540, -5348, 9574, 17610, 0];
const YCC_TO_RGB_OFFSETS: [u32; 3] = [16_777_216, 134_217_728, 134_217_728];
const RGB_TO_LMS: [i16; 9] = [7222, 8771, 390, 2654, 12430, 1300, 0, 422, 15962];

/// Whether a negotiated stream carries profile 8.1 RPUs: HDR HEVC whose client
/// picked Dolby Vision 8.1 (`x-rl-video[0].hdrFormat`). Stock clients pick no
/// format, so their streams stay byte for byte HDR10.
pub fn negotiated(stream: &crate::rtsp::Negotiated) -> bool {
    stream.hdr && stream.codec == 1 && stream.hdr_format == rubylight_protocol::hdr::DV_81
}

/// A PQ signal value (0..=1) as the 12-bit code RPUs carry.
pub fn pq12(pq: f32) -> u16 {
    if pq.is_nan() {
        return 0;
    }
    (pq.clamp(0., 1.) * f32::from(PQ12_MAX)).round() as u16
}

fn nits_pq12(nits: f32) -> u16 {
    pq12(nits_to_pq(nits))
}

/// One frame's L1 block: minimum, average and maximum PQ of maxRGB, 12 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level1 {
    pub min_pq: u16,
    pub avg_pq: u16,
    pub max_pq: u16,
}

impl Level1 {
    /// Within CM v2.9's limits: the minimum at most 12, the maximum at least
    /// 2081 (100 nits), the average from 819 to just below the maximum.
    pub fn clamped(min_pq: u16, avg_pq: u16, max_pq: u16) -> Self {
        let max_pq = max_pq.clamp(L1_MAX_PQ_MIN, PQ12_MAX);
        Self {
            min_pq: min_pq.min(L1_MIN_PQ_MAX),
            avg_pq: avg_pq.clamp(L1_AVG_PQ_MIN, max_pq - 1),
            max_pq,
        }
    }

    /// From one frame's statistics; `None` for invalid ones.
    pub fn from_stats(stats: &FrameStats) -> Option<Self> {
        stats
            .is_valid()
            .then(|| Self::clamped(pq12(stats.min_pq), pq12(stats.mean_pq), pq12(stats.max_pq)))
    }

    /// For frames without a measurement: the stream's MaxCLL as the maximum
    /// (else the mastering peak), its MaxFALL as the average (else reference
    /// white), and the mastering minimum.
    pub fn from_static(metadata: &Metadata) -> Self {
        let max = if metadata.max_cll > 0 {
            metadata.max_cll
        } else {
            metadata.maximum_nits
        };
        let avg = if metadata.max_fall > 0 {
            f32::from(metadata.max_fall)
        } else {
            REFERENCE_WHITE_NITS
        };
        Self::clamped(
            nits_pq12(f32::from(metadata.minimum) / 10_000.),
            nits_pq12(avg),
            nits_pq12(f32::from(max)),
        )
    }
}

/// Everything one frame's RPU says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub level1: Level1,
    /// The target display peak of the L2 trims.
    pub target_nits: u16,
    /// The stream's static HDR10 metadata: the source display and L6.
    pub metadata: Metadata,
    /// The frame starts a scene: the client's display management does not
    /// carry its temporal filtering across it.
    pub scene_refresh: bool,
}

impl Frame {
    /// The RPU payload: prefix, header, mapping, display-management data,
    /// CRC-32 and the final byte, before emulation prevention.
    pub fn rpu(&self) -> Vec<u8> {
        let mut bits = Bits::with_prefix(RPU_PREFIX);
        header(&mut bits);
        mapping(&mut bits);
        self.dm_data(&mut bits);
        bits.align();
        let crc = crc32_mpeg2(&bits.bytes[1..]);
        bits.bytes.extend_from_slice(&crc.to_be_bytes());
        bits.bytes.push(RPU_FINAL_BYTE);
        bits.bytes
    }

    /// The RPU as an Annex B NAL unit (four-byte start code, type 62), with
    /// emulation prevention: what goes after the frame's last VCL NAL unit.
    pub fn nal(&self) -> Vec<u8> {
        let rpu = self.rpu();
        let mut nal = Vec::with_capacity(rpu.len() + rpu.len() / 64 + 6);
        nal.extend_from_slice(&[0, 0, 0, 1]);
        nal.extend_from_slice(&NAL_HEADER);
        escape(&rpu, &mut nal);
        nal
    }

    fn dm_data(&self, bits: &mut Bits) {
        let metadata = &self.metadata;
        bits.ue(0); // affected_dm_metadata_id
        bits.ue(0); // current_dm_metadata_id
        bits.ue(u64::from(self.scene_refresh));
        for coefficient in YCC_TO_RGB {
            bits.signed(16, coefficient.into());
        }
        for offset in YCC_TO_RGB_OFFSETS {
            bits.put(32, offset.into());
        }
        for coefficient in RGB_TO_LMS {
            bits.signed(16, coefficient.into());
        }
        bits.put(16, 65535); // signal_eotf: PQ
        bits.put(16, 0); // signal_eotf_param0
        bits.put(16, 0); // signal_eotf_param1
        bits.put(32, 0); // signal_eotf_param2
        bits.put(5, 12); // signal_bit_depth
        bits.put(2, 0); // signal_color_space: YCbCr
        bits.put(2, 0); // signal_chroma_format: 4:2:0
        bits.put(2, 1); // signal_full_range_flag
        bits.put(12, nits_pq12(f32::from(metadata.minimum) / 10_000.).into()); // source_min_pq
        bits.put(12, nits_pq12(f32::from(metadata.maximum_nits)).into()); // source_max_pq
        bits.put(10, SOURCE_DIAGONAL.into());

        // CM v2.9 extension blocks, in level order.
        bits.ue(3); // num_ext_blocks
        bits.align(); // dm_alignment_zero_bit
        let l1 = self.level1;
        extension_block(bits, 1, 5, |bits| {
            bits.put(12, l1.min_pq.into());
            bits.put(12, l1.max_pq.into());
            bits.put(12, l1.avg_pq.into());
        });
        let target_pq = nits_pq12(f32::from(self.target_nits.max(1)));
        extension_block(bits, 2, 11, |bits| {
            bits.put(12, target_pq.into());
            for _ in 0..5 {
                // trim slope, offset, power, chroma weight, saturation gain
                bits.put(12, L2_NEUTRAL.into());
            }
            bits.signed(13, L2_NEUTRAL.into()); // ms_weight
        });
        extension_block(bits, 6, 8, |bits| {
            bits.put(16, metadata.maximum_nits.min(10_000).into());
            bits.put(16, metadata.minimum.min(10_000).into());
            bits.put(16, metadata.max_cll.min(10_000).into());
            bits.put(16, metadata.max_fall.min(10_000).into());
        });
    }
}

/// rpu_data_header for profile 8: one 10-bit base layer, no enhancement
/// layer (residual disabled), display-management data present.
fn header(bits: &mut Bits) {
    bits.put(6, 2); // rpu_type
    bits.put(11, 18); // rpu_format
    bits.put(4, 1); // vdr_rpu_profile
    bits.put(4, 0); // vdr_rpu_level
    bits.put(1, 1); // vdr_seq_info_present_flag
    bits.put(1, 0); // chroma_resampling_explicit_filter_flag
    bits.put(2, 0); // coefficient_data_type: fixed point
    bits.ue(COEFFICIENT_LOG2_DENOM.into());
    bits.put(2, 1); // vdr_rpu_normalized_idc
    bits.put(1, 0); // bl_video_full_range_flag
    bits.ue(2); // bl_bit_depth_minus8
    bits.ue(2); // el_bit_depth_minus8, ext_mapping_idc 0
    bits.ue(4); // vdr_bit_depth_minus8
    bits.put(1, 0); // spatial_resampling_filter_flag
    bits.put(3, 0); // reserved_zero_3bits
    bits.put(1, 0); // el_spatial_resampling_filter_flag
    bits.put(1, 1); // disable_residual_flag
    bits.put(1, 1); // vdr_dm_metadata_present_flag
    bits.put(1, 0); // use_prev_vdr_rpu_flag
}

const COEFFICIENT_LOG2_DENOM: u8 = 23;
const BL_BIT_DEPTH: u32 = 10;

/// rpu_data_mapping: per component one piece over the whole 10-bit range,
/// the first-order polynomial 0 + 1x.
fn mapping(bits: &mut Bits) {
    bits.ue(0); // vdr_rpu_id
    bits.ue(0); // mapping_color_space
    bits.ue(0); // mapping_chroma_format_idc
    for _ in 0..3 {
        bits.ue(0); // num_pivots_minus2
        bits.put(BL_BIT_DEPTH, 0);
        bits.put(BL_BIT_DEPTH, (1 << BL_BIT_DEPTH) - 1);
    }
    bits.ue(0); // num_x_partitions_minus1
    bits.ue(0); // num_y_partitions_minus1
    for _ in 0..3 {
        bits.ue(0); // mapping_idc: polynomial
        bits.ue(0); // poly_order_minus1
        bits.put(1, 0); // linear_interp_flag
        for coefficient in [0, 1] {
            bits.se(coefficient); // poly_coef_int
            bits.put(COEFFICIENT_LOG2_DENOM.into(), 0); // poly_coef
        }
    }
}

/// One extension block of `length` bytes, zero-padded after `payload`.
fn extension_block(bits: &mut Bits, level: u8, length: u64, payload: impl FnOnce(&mut Bits)) {
    bits.ue(length);
    bits.put(8, level.into());
    let start = bits.position();
    payload(bits);
    let used = bits.position() - start;
    debug_assert!(used <= length * 8, "L{level} uses {used} bits");
    for _ in used..length * 8 {
        bits.put(1, 0); // ext_dm_alignment_zero_bit
    }
}

/// CRC-32/MPEG-2 (polynomial 0x04C11DB7, initial value all ones, unreflected,
/// no final XOR), which RPUs carry over everything after their prefix byte.
pub fn crc32_mpeg2(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// One stream's RPUs: every frame gets one, from the newest smoothed
/// statistics, or from the static metadata before the first (and on a
/// stream without statistics).
#[derive(Clone, Debug)]
pub struct Writer {
    metadata: Metadata,
    target_nits: u16,
    measured: Option<Level1>,
    /// The next frame starts a scene: the first one, and after a cut.
    refresh: bool,
}

impl Writer {
    pub fn new(target_nits: u16) -> Self {
        Self {
            metadata: Metadata::default(),
            target_nits,
            measured: None,
            refresh: true,
        }
    }

    pub fn target_nits(&self) -> u16 {
        self.target_nits
    }

    pub fn set_target_nits(&mut self, nits: u16) {
        self.target_nits = nits;
    }

    /// The stream's static HDR10 metadata, for the source display and L6.
    pub fn set_metadata(&mut self, metadata: Metadata) {
        self.metadata = metadata;
    }

    /// One smoothed analysis sample ([`crate::hdr_dynamic::Writer::sample`]).
    pub fn sample(&mut self, smoothed: &Smoothed) {
        if let Some(level1) = Level1::from_stats(&smoothed.stats) {
            self.measured = Some(level1);
            self.refresh |= smoothed.scene_cut;
        }
    }

    /// Whether frames carry measured L1 statistics yet.
    pub fn measured(&self) -> bool {
        self.measured.is_some()
    }

    /// The measurements ended: frames go back to the static light levels,
    /// from a new scene.
    pub fn forget_measurements(&mut self) {
        if self.measured.take().is_some() {
            self.refresh = true;
        }
    }

    /// What the next frame's RPU says. A keyframe refreshes the scene too, so
    /// a client that starts there does not filter against what it never saw.
    pub fn frame(&mut self, keyframe: bool) -> Frame {
        let frame = Frame {
            level1: self
                .measured
                .unwrap_or_else(|| Level1::from_static(&self.metadata)),
            target_nits: self.target_nits,
            metadata: self.metadata,
            scene_refresh: self.refresh || keyframe,
        };
        self.refresh = false;
        frame
    }
}

/// MSB-first bit writer; [`Bits::align`] pads the last byte with zeros.
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn with_prefix(prefix: u8) -> Self {
        let mut bytes = Vec::with_capacity(160);
        bytes.push(prefix);
        Self { bytes, used: 0 }
    }

    fn put(&mut self, count: u32, value: u64) {
        debug_assert!(count <= 64 && (count == 64 || value >> count == 0));
        for shift in (0..count).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= (((value >> shift) & 1) as u8) << (7 - self.used);
            self.used = (self.used + 1) % 8;
        }
    }

    /// Two's complement in `count` bits.
    fn signed(&mut self, count: u32, value: i64) {
        self.put(count, (value as u64) & ((1 << count) - 1));
    }

    /// Exp-Golomb ue(v).
    fn ue(&mut self, value: u64) {
        let coded = value + 1;
        let length = 64 - coded.leading_zeros();
        self.put(length - 1, 0);
        self.put(length, coded);
    }

    /// Exp-Golomb se(v).
    fn se(&mut self, value: i64) {
        self.ue(if value > 0 {
            (2 * value - 1) as u64
        } else {
            (-2 * value) as u64
        });
    }

    fn align(&mut self) {
        self.used = 0;
    }

    fn position(&self) -> u64 {
        let full = self.bytes.len() as u64 - u64::from(self.used != 0);
        full * 8 + u64::from(self.used)
    }
}

#[cfg(test)]
mod tests;
