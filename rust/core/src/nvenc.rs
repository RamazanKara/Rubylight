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

/// How NVENC spends bits: a constant bitrate, or a variable one up to 1.5
/// times the stream's rate that aims at `target_quality` (0: the driver
/// picks). The VBR choice and its bounds follow Foundation Sunshine
/// (AlkaidLab/foundation-sunshine, GPL-3.0, src/nvenc/nvenc_base.cpp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateControl {
    Cbr,
    Vbr { target_quality: u8 },
}
impl RateControl {
    /// The peak a VBR stream may reach over its average, in bits per second.
    pub fn max_bitrate(self, average: u32) -> u32 {
        match self {
            Self::Cbr => average,
            Self::Vbr { .. } => (u64::from(average) * 3 / 2).min(u64::from(u32::MAX)) as u32,
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
mod tests {
    use super::*;
    #[test]
    fn reviewed_driver_negotiation_keeps_older_h264_hevc_and_av1_layouts() {
        assert_eq!(ApiVersion::from_driver(0xc1), ApiVersion(12, 1));
        assert_eq!(
            ApiVersion::candidates(1, ApiVersion(12, 1)).collect::<Vec<_>>(),
            vec![ApiVersion(12, 1), ApiVersion(12, 0), ApiVersion(11, 0)]
        );
        // AV1 needs API 12.0 (driver 522.25), not 12.1.
        assert_eq!(
            ApiVersion::candidates(2, ApiVersion(12, 0)).collect::<Vec<_>>(),
            vec![ApiVersion(12, 0)]
        );
        assert_eq!(ApiVersion::candidates(2, ApiVersion(11, 0)).count(), 0);
        assert_eq!(
            ApiVersion::candidates(2, ApiVersion(12, 2)).collect::<Vec<_>>(),
            vec![ApiVersion(12, 2), ApiVersion(12, 1), ApiVersion(12, 0)]
        );
        assert_eq!(
            ApiVersion(12, 0).config(),
            ApiVersion(12, 1).config() & !(1 << 24)
        );
        assert_eq!(
            ApiVersion(12, 0).picture(),
            ApiVersion(12, 1).picture() & !(1 << 24)
        );
        // SDK 12.2 already changed these revisions; using 12.1 silently breaks 10-bit input.
        assert_eq!(ApiVersion(12, 2).initialize(), 0xf007000c | 2 << 24);
        assert_eq!(ApiVersion(12, 2).config(), 0xf009000c | 2 << 24);
        assert_eq!(ApiVersion(12, 1).lock(), 0xf001000c | 1 << 24);
        assert_eq!(ApiVersion(12, 0).lock(), 0x7002000c);
        assert_eq!(ApiVersion(11, 0).picture(), 0xf004000b);
    }
    #[test]
    fn loss_recovery_includes_dependent_frames_is_bounded_and_confirms_once() {
        let mut references = References::default();
        references.completed(16, false);
        assert_eq!(
            references.plan(14, 14, 5, true),
            Recovery::Invalidate {
                first: 14,
                last: 16
            }
        );
        assert_eq!(references.plan(12, 12, 5, true), Recovery::Idr);
        assert_eq!(references.plan(17, 18, 5, true), Recovery::Idr);
        assert_eq!(references.plan(15, 14, 5, true), Recovery::Idr);
        assert_eq!(references.plan(0, 1, 5, true), Recovery::Idr);
        references.applied(14, 16);
        assert_eq!(references.plan(14, 15, 5, true), Recovery::AlreadyApplied);
        assert!(references.confirmation(false));
        assert!(!references.confirmation(false));
        references.applied(14, 16);
        assert!(!references.confirmation(true));
        references.completed(17, true);
        assert_eq!(references.plan(14, 15, 5, true), Recovery::AlreadyApplied);
        assert_eq!(references.plan(17, 17, 5, true), Recovery::Idr);
        references.completed(19, false);
        assert_eq!(references.plan(16, 18, 5, true), Recovery::Idr);
        assert_eq!(
            references.plan(18, 18, 5, true),
            Recovery::Invalidate {
                first: 18,
                last: 19
            }
        );
        assert_eq!(references.plan(14, 15, 5, false), Recovery::Idr);
    }
    fn tuning(text: &str, codec: u8) -> Tuning {
        let stream = Negotiated {
            codec,
            ..Default::default()
        };
        Tuning::new(&Config::parse(text).unwrap(), &stream).unwrap()
    }
    #[test]
    fn defaults_stay_latency_first_with_spatial_aq_and_the_budget_guard_on() {
        let default = tuning("", 1);
        assert_eq!(default.preset, 1);
        assert_eq!(default.multipass, 1);
        assert_eq!(default.split, 0);
        assert_eq!(default.rate_control, RateControl::Cbr);
        assert!(default.spatial_aq);
        assert!(default.frame_budget_guard);
        assert!(!default.temporal_aq && !default.cavlc && !default.filler);
        assert_eq!(default.min_qp, None);
        let off = tuning(
            "nvenc_spatial_aq = false\nnvenc_frame_budget_guard = false\n",
            1,
        );
        assert!(!off.spatial_aq && !off.frame_budget_guard);
    }
    #[test]
    fn foundation_settings_parse_with_codec_bounds_and_fall_back_on_unknown_values() {
        for (value, mode) in [
            ("disabled", SPLIT_DISABLED),
            ("false", SPLIT_DISABLED),
            ("auto", 0),
            ("driver_decides", 0),
            ("enabled", 1),
            ("forced", 1),
            ("two_strips", 2),
            ("three_strips", 3),
            ("four_strips", 4),
            ("sideways", 0),
        ] {
            assert_eq!(
                tuning(&format!("nvenc_split_encode = {value}\n"), 1).split,
                mode,
                "{value}"
            );
        }
        let vbr = "nvenc_rate_control = vbr\nnvenc_target_quality = 70\n";
        assert_eq!(
            tuning(vbr, 1).rate_control,
            RateControl::Vbr { target_quality: 51 }
        );
        assert_eq!(
            tuning(vbr, 2).rate_control,
            RateControl::Vbr { target_quality: 63 }
        );
        assert_eq!(
            tuning("nvenc_rate_control = vbr\n", 0).rate_control,
            RateControl::Vbr { target_quality: 0 }
        );
        assert_eq!(
            tuning("nvenc_rate_control = cq\n", 0).rate_control,
            RateControl::Cbr
        );
        assert_eq!(RateControl::Cbr.max_bitrate(20_000_000), 20_000_000);
        assert_eq!(
            RateControl::Vbr { target_quality: 0 }.max_bitrate(20_000_000),
            30_000_000
        );
        assert_eq!(
            RateControl::Vbr { target_quality: 0 }.max_bitrate(u32::MAX),
            u32::MAX
        );
        assert_eq!(tuning("nvenc_preset = 9\n", 1).preset, 7);
        assert_eq!(tuning("nvenc_twopass = full_res\n", 1).multipass, 2);
        assert_eq!(tuning("nvenc_twopass = disabled\n", 1).multipass, 0);
    }
    #[test]
    fn slice_delivery_follows_the_amf_setting_with_a_slice_per_block() {
        assert_eq!(tuning("", 1).slice_blocks, 0, "auto is off at 1080p");
        let four = tuning("video_slice_delivery = 4\n", 0);
        assert_eq!(four.slice_blocks, 4);
        assert_eq!(four.subframe_slices(1), 4);
        assert_eq!(four.subframe_slices(8), 8);
        assert_eq!(
            tuning("video_slice_delivery = 2\n", 1).subframe_slices(0),
            2
        );
        // AV1 returns tiles, which slice delivery does not stream.
        assert_eq!(tuning("video_slice_delivery = 4\n", 2).slice_blocks, 0);
    }
    #[test]
    fn a_fixed_slice_mode_is_set_only_where_it_cannot_block_split_encoding() {
        let auto = tuning("", 1);
        assert!(!auto.slice_mode(1, ApiVersion(13, 0)));
        assert!(auto.slice_mode(4, ApiVersion(13, 0)));
        // Older drivers have no split encoding to protect.
        assert!(auto.slice_mode(1, ApiVersion(12, 1)));
        let disabled = tuning("nvenc_split_encode = disabled\n", 1);
        assert!(disabled.slice_mode(1, ApiVersion(13, 0)));
    }
    #[test]
    fn the_frame_budget_guard_lowers_slow_presets_only_when_they_miss_the_budget() {
        // 4K120 on one engine: 8.3 MPix in 5.4 ms leaves only P1, and even
        // P1 (5.8 ms) misses.
        let uhd = frame_budget(4, (3840, 2160), 120., 1, true);
        assert_eq!((uhd.configured, uhd.effective), (4, 1));
        assert!(uhd.over_at_floor);
        assert!((uhd.budget_ms - 1000. / 120. * 0.65).abs() < 1e-9);
        // Two engines with split encoding make P5 fit, not P7.
        let split = frame_budget(7, (3840, 2160), 120., 2, true);
        assert_eq!(split.effective, 5);
        assert!(!split.over_at_floor);
        assert!(split.estimated_ms <= split.budget_ms);
        // 1080p60 fits P7.
        let easy = frame_budget(7, (1920, 1080), 60., 1, true);
        assert_eq!(easy.effective, 7);
        assert!(!easy.over_at_floor);
        // The guard off keeps the preset and still reports what it costs.
        let kept = frame_budget(4, (3840, 2160), 120., 1, false);
        assert_eq!(kept.effective, 4);
        assert_eq!(kept.estimated_ms, kept.configured_ms);
        assert!(kept.estimated_ms > kept.budget_ms);
        // P1, the default, is never changed; unknown rates keep the preset.
        assert_eq!(frame_budget(1, (7680, 4320), 240., 1, true).effective, 1);
        assert_eq!(frame_budget(5, (1920, 1080), 0., 1, true).effective, 5);
        let unknown = frame_budget(5, (1920, 1080), f64::NAN, 1, true);
        assert_eq!((unknown.effective, unknown.budget_ms), (5, 0.));
        assert_eq!(frame_budget(5, (1920, 1080), -60., 1, true).effective, 5);
        assert_eq!(frame_budget(0, (1920, 1080), 60., 1, true).configured, 1);
    }
    #[test]
    fn the_d3d12_interface_and_block_statistics_follow_the_api() {
        assert!(!ApiVersion(11, 0).d3d12());
        assert!(ApiVersion(12, 0).d3d12() && ApiVersion(13, 0).d3d12());
        assert!(!ApiVersion(12, 0).block_stats());
        assert!(ApiVersion(12, 1).block_stats() && ApiVersion(13, 0).block_stats());
    }
    #[test]
    fn the_focus_delta_map_moves_qp_between_blocks_around_the_mean() {
        let mut out = Vec::new();
        // H.264: a delta per macroblock, half a QP per importance step.
        focus_deltas(0, (32, 16), &[10, 0], (2, 1), &mut out).unwrap();
        assert_eq!(out, [-3, 3]);
        // A uniform map moves nothing.
        focus_deltas(0, (32, 16), &[7, 7], (2, 1), &mut out).unwrap();
        assert_eq!(out, [0, 0]);
        // HEVC's 32x32 CTBs each take the 64x64 block they lie in.
        focus_deltas(1, (128, 64), &[10, 0], (2, 1), &mut out).unwrap();
        assert_eq!(out, [-3, -3, 3, 3, -3, -3, 3, 3]);
        // A partial CTB at the edge takes the last block.
        focus_deltas(1, (100, 40), &[10, 0], (2, 1), &mut out).unwrap();
        assert_eq!(out.len(), 8);
        assert_eq!(out[3], 3);
        // AV1 is in q-index, four steps per QP.
        focus_deltas(2, (128, 64), &[10, 0], (2, 1), &mut out).unwrap();
        assert_eq!(out, [-10, 10]);
        // Values over 10 count as 10.
        focus_deltas(0, (32, 16), &[30, 0], (2, 1), &mut out).unwrap();
        assert_eq!(out, [-3, 3]);
        assert_eq!(delta_grid(1, 1920, 1080), (60, 34));
        assert_eq!(delta_grid(2, 1920, 1080), (30, 17));
        // A map of another size is refused.
        let error = focus_deltas(0, (32, 16), &[1, 2, 3], (3, 1), &mut out).unwrap_err();
        assert!(error.contains("3x1"), "{error}");
        assert!(focus_deltas(0, (32, 16), &[1], (2, 1), &mut out).is_err());
    }
}
