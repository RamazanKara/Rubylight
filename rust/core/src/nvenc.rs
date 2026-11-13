//! Reviewed NVENC driver versions and bounded reference recovery.
use crate::{config::Config, rtsp::Negotiated};
use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion(pub u8, pub u8);
impl ApiVersion {
    pub const COMPILED: Self = Self(13, 0);
    pub const fn packed(self) -> u32 {
        self.0 as u32 | ((self.1 as u32) << 24)
    }
    pub const fn from_driver(value: u32) -> Self {
        Self((value >> 4) as u8, (value & 15) as u8)
    }
    pub const fn structure(self, revision: u32, extended: bool) -> u32 {
        self.packed() | (revision << 16) | (7 << 28) | if extended { 1 << 31 } else { 0 }
    }
    pub fn candidates(codec: u8, maximum: Self) -> impl Iterator<Item = Self> {
        [
            Self(13, 0),
            Self(12, 2),
            Self(12, 1),
            Self(12, 0),
            Self(11, 0),
        ]
        .into_iter()
        // AV1 arrived with API 12.0, whose AV1 configuration and picture
        // structures have the revisions 12.1 kept (`config`, `picture`).
        .filter(move |v| {
            *v <= maximum && *v <= Self::COMPILED && codec <= 2 && (codec != 2 || *v >= Self(12, 0))
        })
    }
    pub const fn modern_depth(self) -> bool {
        self.0 > 12 || (self.0 == 12 && self.1 >= 2)
    }
    pub const fn initialize(self) -> u32 {
        self.structure(
            if self.modern_depth() {
                7
            } else if self.0 == 12 && self.1 == 1 {
                6
            } else {
                5
            },
            true,
        )
    }
    pub const fn config(self) -> u32 {
        self.structure(
            if self.modern_depth() {
                9
            } else if self.0 == 11 {
                7
            } else {
                8
            },
            true,
        )
    }
    pub const fn preset(self) -> u32 {
        self.structure(if self.modern_depth() { 5 } else { 4 }, true)
    }
    pub const fn reconfigure(self) -> u32 {
        self.structure(if self.modern_depth() { 2 } else { 1 }, true)
    }
    pub const fn register(self) -> u32 {
        self.structure(
            if self.modern_depth() {
                5
            } else if self.0 == 11 {
                3
            } else {
                4
            },
            false,
        )
    }
    pub const fn picture(self) -> u32 {
        self.structure(
            if self.modern_depth() {
                7
            } else if self.0 == 11 {
                4
            } else {
                6
            },
            true,
        )
    }
    pub const fn lock(self) -> u32 {
        self.structure(
            if self.0 == 11 || (self.0 == 12 && self.1 == 1) {
                1
            } else {
                2
            },
            self.0 > 12 || (self.0 == 12 && self.1 >= 1),
        )
    }
    pub const fn event(self) -> u32 {
        self.structure(if self.modern_depth() { 2 } else { 1 }, false)
    }
    /// A session on an `ID3D12Device`, with fence points on the input and the
    /// output: API 12.0 (driver 522.25) and newer. SDK 11.1 had the interface
    /// with other structure layouts, and no reviewed candidate uses them.
    pub const fn d3d12(self) -> bool {
        self.0 >= 12
    }
    /// Per-block statistics (`enableOutputStats`, `outputStatsPtr`): API 12.1
    /// (driver 531.61) and newer.
    pub const fn block_stats(self) -> bool {
        self.0 > 12 || (self.0 == 12 && self.1 >= 1)
    }
}

/// Why NVENC as this host builds it cannot encode `codec` in native 4:4:4,
/// before any driver is asked; None for H.264 (High 4:4:4) and HEVC (Rext),
/// which the GPU's `NV_ENC_CAPS_SUPPORT_YUV444_ENCODE` then decides. The SDK
/// headers this host uses (13.0, `rust/windows/include/nvEncodeAPI.h`) say
/// of `NV_ENC_CONFIG_AV1::chromaFormatIDC` "yuv444 input currently not
/// supported" and define no AV1 profile beyond Main, so AV1 4:4:4 is refused
/// rather than configured from a guess.
pub fn sdk_444_refusal(codec: u8) -> Option<&'static str> {
    match codec {
        0 | 1 => None,
        2 => Some(
            "NVENC SDK 13.0, which this host is built with, encodes AV1 in 4:2:0 only. Choose HEVC for YUV 4:4:4",
        ),
        _ => Some("NVENC encodes 4:4:4 in H.264 and HEVC only"),
    }
}

/// The input a native 4:4:4 session reads, chosen as LizardByte Sunshine
/// feeds NVENC (`src/nvenc/nvenc_utils.cpp`, `nvenc_d3d11_native.cpp` and
/// `nvenc_d3d11_on_cuda.cpp` at ec2676b60bb8220a1807fc761bc2157e5e6dba63,
/// GPL-3.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input444 {
    /// 8-bit: one packed AYUV texture on the D3D11 device
    /// (`NV_ENC_BUFFER_FORMAT_AYUV`).
    Ayuv,
    /// 10-bit: the Y, U and V planes stacked in one 16-bit texture three
    /// pictures tall, each code in the upper ten bits, copied through CUDA
    /// and read as `NV_ENC_BUFFER_FORMAT_YUV444_10BIT`. No D3D11 surface
    /// format holds planar 4:4:4, so NVENC cannot register it directly.
    Planar16,
}

/// The native 4:4:4 input for a stream's bit depth.
pub const fn input_444(ten_bit: bool) -> Input444 {
    if ten_bit {
        Input444::Planar16
    } else {
        Input444::Ayuv
    }
}

/// The block edge of NVENC's per-picture QP delta map: a 16x16 macroblock
/// for H.264, the 32x32 CTB for HEVC (the only CTB size NVENC codes) and a
/// 64x64 superblock for AV1.
pub fn delta_block(codec: u8) -> u32 {
    match codec {
        0 => 16,
        1 => 32,
        _ => 64,
    }
}

/// The QP delta map's size in blocks for a coded picture.
pub fn delta_grid(codec: u8, width: u32, height: u32) -> (u32, u32) {
    let block = delta_block(codec);
    (width.div_ceil(block), height.div_ceil(block))
}

/// QP per importance step. AMF's ROI map moved block QP by about half a QP
/// per step on the RX 7900 XT (docs/encoder-focus-design.md, section 2), so
/// the same map weighs blocks alike on both encoders.
pub const FOCUS_QP_PER_STEP: f64 = 0.5;
/// AV1's map is in q-index, about four steps per QP, as AMF's AV1 feedback.
pub const FOCUS_Q_INDEX_SCALE: f64 = 4.0;

/// NVENC's `qpDeltaMap` for an encoder focus importance map (0..=10 per
/// [`crate::encoder_focus::block`], row by row) of a `width` x `height`
/// coded picture, written into `out`. A more important block gets a lower
/// QP. The offsets are centred on the picture's mean importance, so the map
/// moves bits between blocks without moving the frame's QP away from the
/// rate control's choice, which the one-frame VBV holds. HEVC's 32x32 CTBs
/// each take the 64x64 block they lie in.
pub fn focus_deltas(
    codec: u8,
    (width, height): (u32, u32),
    map: &[u32],
    (columns, rows): (u32, u32),
    out: &mut Vec<i8>,
) -> std::result::Result<(), String> {
    let expected = crate::encoder_focus::grid(codec, width, height);
    if (columns, rows) != expected || map.len() != columns as usize * rows as usize {
        return Err(format!(
            "encoder focus map is {columns}x{rows} blocks ({} values), but NVENC codes {width}x{height} in {}x{} focus blocks",
            map.len(),
            expected.0,
            expected.1
        ));
    }
    let (map_block, block) = (crate::encoder_focus::block(codec), delta_block(codec));
    let (wide, high) = delta_grid(codec, width, height);
    let cell = |x: u32, y: u32| {
        let column = (x * block / map_block).min(columns - 1);
        let row = (y * block / map_block).min(rows - 1);
        map[(row * columns + column) as usize].min(10)
    };
    out.clear();
    out.reserve(wide as usize * high as usize);
    let mut sum = 0u64;
    for y in 0..high {
        for x in 0..wide {
            sum += u64::from(cell(x, y));
        }
    }
    let mean = sum as f64 / f64::from((wide * high).max(1));
    let scale = FOCUS_QP_PER_STEP * if codec == 2 { FOCUS_Q_INDEX_SCALE } else { 1. };
    for y in 0..high {
        for x in 0..wide {
            let delta = ((mean - f64::from(cell(x, y))) * scale).round();
            out.push(delta.clamp(-51., 51.) as i8);
        }
    }
    Ok(())
}

/// How NVENC spends bits: a constant bitrate, or a variable one up to
/// `peak_percent` of the stream's rate (150 unless `nvenc_peak_bitrate_ratio`
/// says otherwise) that aims at `target_quality` (0: the driver picks). The VBR
/// choice and its 1.5 times default follow Foundation Sunshine
/// (AlkaidLab/foundation-sunshine, GPL-3.0, src/nvenc/nvenc_base.cpp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateControl {
    Cbr,
    Vbr {
        target_quality: u8,
        peak_percent: u16,
    },
}
/// The VBR peak over the stream's bitrate when `nvenc_peak_bitrate_ratio` is unset.
pub const VBR_PEAK_PERCENT: u16 = 150;
/// The peaks `nvenc_peak_bitrate_ratio` offers, as multiples of the stream's bitrate.
pub const VBR_PEAKS: [(&str, u16); 5] = [
    ("1", 100),
    ("1.25", 125),
    ("1.5", 150),
    ("2", 200),
    ("3", 300),
];
impl RateControl {
    /// The peak a VBR stream may reach over its average, in bits per second.
    pub fn max_bitrate(self, average: u32) -> u32 {
        match self {
            Self::Cbr => average,
            Self::Vbr { peak_percent, .. } => {
                (u64::from(average) * u64::from(peak_percent) / 100).min(u64::from(u32::MAX)) as u32
            }
        }
    }
}

/// `splitEncodeMode` for split-frame encoding switched off.
pub const SPLIT_DISABLED: u32 = 15;

#[derive(Clone, Debug)]
pub struct Tuning {
    pub preset: usize,
    pub multipass: u32,
    pub vbv_increase: u32,
    pub weighted_prediction: bool,
    pub spatial_aq: bool,
    pub temporal_aq: bool,
    pub min_qp: Option<u32>,
    pub cavlc: bool,
    pub filler: bool,
    pub split: u32,
    pub rate_control: RateControl,
    /// Lower `preset` when its estimated encode time misses the frame budget
    /// ([`frame_budget`]).
    pub frame_budget_guard: bool,
    /// Blocks a P-frame is sent in as its slices leave the encoder
    /// ([`crate::encoder_policy::slice_delivery`]), the same setting as AMF's.
    pub slice_blocks: u8,
    /// `qpMapMode` is the delta map from the start: the focus probe's own
    /// session. A stream turns it on when focus does.
    pub qp_map: bool,
    /// Per-block QP statistics for `encoder_focus_verify` and the probe.
    pub block_stats: bool,
}
impl Tuning {
    pub fn new(config: &Config, stream: &Negotiated) -> Result<Self> {
        let multipass = match config.get("nvenc_twopass", "quarter_res") {
            "disabled" => 0,
            "quarter_res" => 1,
            "full_res" => 2,
            other => {
                crate::config::fallback("nvenc_twopass", other, "quarter_res");
                1
            }
        };
        let split = match config.get("nvenc_split_encode", "auto") {
            "disabled" | "false" => SPLIT_DISABLED,
            // Vibepollo's name for automatic split encoding.
            "auto" | "driver_decides" => 0,
            "forced" | "enabled" | "true" => 1,
            // Foundation Sunshine's fixed strip counts; the driver encodes
            // fewer strips on a GPU with fewer NVENC engines.
            "two_strips" => 2,
            "three_strips" => 3,
            "four_strips" => 4,
            other => {
                crate::config::fallback("nvenc_split_encode", other, "auto");
                0
            }
        };
        let rate_control = match config.get("nvenc_rate_control", "cbr") {
            "cbr" => RateControl::Cbr,
            "vbr" => RateControl::Vbr {
                target_quality: config
                    .integer("nvenc_target_quality", 0)
                    .clamp(0, if stream.codec == 2 { 63 } else { 51 })
                    as u8,
                peak_percent: {
                    let value = config.get("nvenc_peak_bitrate_ratio", "1.5").trim();
                    match VBR_PEAKS.iter().find(|(name, _)| *name == value) {
                        Some((_, percent)) => *percent,
                        None => {
                            crate::config::fallback("nvenc_peak_bitrate_ratio", value, "1.5");
                            VBR_PEAK_PERCENT
                        }
                    }
                },
            },
            other => {
                crate::config::fallback("nvenc_rate_control", other, "cbr");
                RateControl::Cbr
            }
        };
        let min_qp = config.boolean("nvenc_enable_min_qp", false).then(|| {
            let (key, default) = match stream.codec {
                0 => ("nvenc_min_qp_h264", 19),
                1 => ("nvenc_min_qp_hevc", 23),
                _ => ("nvenc_min_qp_av1", 23),
            };
            config
                .integer(key, default)
                .clamp(0, if stream.codec == 2 { 255 } else { 51 }) as u32
        });
        Ok(Self {
            preset: config.integer("nvenc_preset", 1).clamp(1, 7) as usize,
            multipass,
            vbv_increase: config.integer("nvenc_vbv_increase", 0).clamp(0, 400) as u32,
            weighted_prediction: config.boolean("nvenc_weighted_prediction", false),
            // Spatial AQ costs no latency and gives flat areas the bits they
            // show; NVIDIA's game-streaming note and Foundation Sunshine
            // turn it on.
            spatial_aq: config.boolean("nvenc_spatial_aq", true),
            temporal_aq: config.boolean("nvenc_temporal_aq", false),
            min_qp,
            cavlc: config.boolean("nvenc_h264_cavlc", false),
            filler: config.boolean("nvenc_insert_filler_data", false),
            split,
            rate_control,
            frame_budget_guard: config.boolean("nvenc_frame_budget_guard", true),
            slice_blocks: crate::encoder_policy::slice_delivery(config, stream),
            qp_map: false,
            block_stats: config.boolean("encoder_focus_verify", false),
        })
    }
    /// Slices per frame when NVENC returns them as they finish: at least one
    /// per block, as AMF sets its slice count.
    pub fn subframe_slices(&self, slices: u32) -> u32 {
        u32::from(self.slice_blocks).max(slices).max(2)
    }
    /// H.264 and HEVC set a slice count only for several slices, or with
    /// split encoding off: a fixed slice mode keeps the driver from
    /// splitting a frame across NVENC engines (Foundation Sunshine). Drivers
    /// before API 12.2 always take it.
    pub fn slice_mode(&self, slices: u32, api: ApiVersion) -> bool {
        slices > 1 || self.split == SPLIT_DISABLED || !api.modern_depth()
    }
    pub fn vbv(&self, bitrate_kbps: u32, rate_millihz: u32) -> u32 {
        (u64::from(bitrate_kbps) * 1_000_000 * u64::from(100 + self.vbv_increase)
            / u64::from(rate_millihz.max(1))
            / 100)
            .min(u64::from(u32::MAX)) as u32
    }
}

/// Estimated encode time per megapixel on one NVENC engine, by preset P1 to
/// P7, in milliseconds. Foundation Sunshine's calibration (src/nvenc/
/// frame_budget.cpp), rounded up to stay safe on older GPUs.
const ENCODE_MS_PER_MPIX: [f64; 8] = [0., 0.7, 0.8, 0.9, 1.0, 1.1, 1.2, 1.4];
/// The share of a frame interval encoding may take; capture, the game and
/// the network need the rest.
const BUDGET_SHARE: f64 = 0.65;
/// Throughput when split encoding can spread a frame over several engines.
const MULTI_ENGINE_SPEEDUP: f64 = 1.75;

/// What the frame budget guard decided for a session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub configured: usize,
    pub effective: usize,
    /// The encode time a frame may take.
    pub budget_ms: f64,
    /// The estimate at `effective`, and at `configured`.
    pub estimated_ms: f64,
    pub configured_ms: f64,
    /// Even P1 is estimated over the budget.
    pub over_at_floor: bool,
}

/// Foundation Sunshine's frame budget guard: the slowest preset up to
/// `preset` whose estimated encode time fits [`BUDGET_SHARE`] of the frame
/// interval, P1 at the least. `engines` counts only when split encoding may
/// use them. With the guard off the preset stands and the numbers still say
/// what it costs.
pub fn frame_budget(
    preset: usize,
    (width, height): (u32, u32),
    fps: f64,
    engines: u32,
    guard: bool,
) -> Budget {
    let configured = preset.clamp(1, 7);
    let mut budget = Budget {
        configured,
        effective: configured,
        budget_ms: 0.,
        estimated_ms: 0.,
        configured_ms: 0.,
        over_at_floor: false,
    };
    if fps.is_nan() || fps <= 0. || width == 0 || height == 0 {
        return budget;
    }
    budget.budget_ms = 1000. / fps * BUDGET_SHARE;
    let speedup = if engines > 1 {
        MULTI_ENGINE_SPEEDUP
    } else {
        1.
    };
    let estimate = |preset: usize| {
        ENCODE_MS_PER_MPIX[preset] * f64::from(width) * f64::from(height) / 1e6 / speedup
    };
    budget.configured_ms = estimate(configured);
    let fitting = (1..=configured)
        .rev()
        .find(|&preset| estimate(preset) <= budget.budget_ms)
        .unwrap_or(1);
    budget.over_at_floor = estimate(fitting) > budget.budget_ms;
    if guard {
        budget.effective = fitting;
        budget.estimated_ms = estimate(fitting);
    } else {
        budget.estimated_ms = budget.configured_ms;
    }
    budget
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    AlreadyApplied,
    Invalidate { first: u64, last: u64 },
    Idr,
}
#[derive(Default)]
pub struct References {
    last: u64,
    last_idr: u64,
    covered: Option<(u64, u64)>,
    confirmation: bool,
}
impl References {
    pub fn completed(&mut self, frame: u64, idr: bool) {
        self.last = frame;
        if idr {
            self.last_idr = frame;
            self.covered = None;
            self.confirmation = false;
        }
    }
    pub fn plan(&self, first: u64, last: u64, retained: u32, supported: bool) -> Recovery {
        if !supported || first == 0 || last < first || last > self.last {
            return Recovery::Idr;
        }
        // Delayed feedback for an older GOP cannot poison the current one.
        // A lost IDR itself has no retained independent anchor.
        if last < self.last_idr {
            return Recovery::AlreadyApplied;
        }
        if first <= self.last_idr {
            return Recovery::Idr;
        }
        if self
            .covered
            .is_some_and(|(start, end)| first >= start && last <= end)
        {
            return Recovery::AlreadyApplied;
        }
        if self.last.saturating_sub(first).saturating_add(1) >= u64::from(retained) {
            return Recovery::Idr;
        }
        Recovery::Invalidate {
            first,
            last: self.last,
        }
    }
    pub fn applied(&mut self, first: u64, last: u64) {
        self.covered = Some((first, last));
        self.confirmation = true;
    }
    pub fn confirmation(&mut self, idr: bool) -> bool {
        let pending = std::mem::take(&mut self.confirmation);
        pending && !idr
    }
    /// What [`Self::confirmation`] returns for the next frame, without taking it.
    pub fn confirming(&self, idr: bool) -> bool {
        self.confirmation && !idr
    }
}

#[cfg(test)]
mod tests;
