//! Settings shared by the native encoders. Property names and enum values follow
//! the retained AMF SDK, rather than assuming all codec names share a prefix.
use crate::{config::Config, rtsp::Negotiated};
use anyhow::{Result, bail};

/// Apollo's and Vibepollo's names for AMD's encoder are AMF. Automatic is AMF
/// too: it is the host's only encoder for H.264, HEVC and AV1. Names this
/// host cannot use select automatically; reading the configuration already
/// replaced them (see [`retired`]).
pub fn canonical_name(name: &str) -> &str {
    match name {
        "amdvce" | "amdvce_experimental" | "amdvce_ffmpeg" | "amdvce_legacy" => "amf",
        "" | "auto" | "amf" => name,
        // Chooses PyroWave's own encoder; other codecs select automatically.
        "pyrowave" => "auto",
        other => {
            crate::config::fallback("encoder", other, "auto");
            "auto"
        }
    }
}
/// An `encoder` value naming an encoder this host does not have: NVENC,
/// Quick Sync, software, Media Foundation or anything else. The
/// configuration replaces it with AMF.
pub fn retired(name: &str) -> bool {
    !matches!(
        name,
        "" | "auto"
            | "amf"
            | "pyrowave"
            | "amdvce"
            | "amdvce_experimental"
            | "amdvce_ffmpeg"
            | "amdvce_legacy"
    )
}

/// The `--encoder` of the measurement probes. Older repro commands pass
/// `--encoder amf`; it and `auto` still mean the only encoder there is.
pub fn probe_option(value: &str) -> Result<()> {
    match value {
        "amf" | "auto" => Ok(()),
        other => bail!("--encoder {other} is not available: Rubylight 3.0 is AMF-only"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Integer(i64),
    Boolean(bool),
}
#[derive(Debug)]
pub struct Property {
    pub name: String,
    pub value: Value,
    pub required: bool,
}
pub fn tristate(config: &Config, key: &str, default: Option<bool>) -> Option<bool> {
    match config
        .values
        .get(key)
        .map(|s| s.trim().to_ascii_lowercase())
    {
        None => default,
        Some(s) => match s.as_str() {
            "1" | "true" | "yes" | "enable" | "enabled" | "on" => Some(true),
            "0" | "false" | "no" | "disable" | "disabled" | "off" => Some(false),
            _ => None,
        },
    }
}
/// `video_slice_delivery = off | 2 | 4`: how many blocks an H.264 or HEVC
/// P-frame is sent in, each as soon as its slices leave the encoder. 0 is off
/// (the default, and always for AV1): frame output and the whole-frame send.
pub fn slice_delivery(config: &Config, codec: u8) -> u8 {
    const KEY: &str = "video_slice_delivery";
    let blocks = match config.get(KEY, "off") {
        "off" | "0" => 0,
        "2" => 2,
        "4" => 4,
        other => {
            crate::config::fallback(KEY, other, "off");
            0
        }
    };
    if codec > 1 { 0 } else { blocks }
}
/// AVC intra refresh requires two retained references in AMF. A client that
/// negotiated only one must use IDR recovery instead of exceeding its budget.
/// AV1 never refreshes: the stream is configured as one that never asked and
/// recovers from a loss by reference invalidation. On the RX 7900 XT AV1
/// refresh turned long-term references off, so a loss cost a 101 KB keyframe
/// against a 48 KB invalidated frame, inter frames grew from 34 to 51 KB, and
/// refresh alone never decoded again (rust/PERFORMANCE.md, October 27).
pub fn amf_intra_refresh(stream: &Negotiated) -> bool {
    stream.intra_refresh && stream.codec != 2 && !(stream.codec == 0 && stream.references == 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmfHevcIntraRefresh {
    pub gop_frames: i64,
    pub ctbs_per_slot: i64,
    pub fallback_ctbs_per_slot: i64,
}

/// The Xbox HEVC workaround refreshes whole CTB rows with a finite GOP.
/// Derived from Eftimin70's workaround and Nonary/vibeshine's follow-up:
/// https://github.com/Nonary/vibeshine/commit/5671498664f5a8f14e959d4ba05904f11c92525c
/// Apply as a pair after the ordinary policy, retaining that policy if the
/// driver cannot establish both settings. Initial and recovery IDRs remain
/// independently requested on their surfaces.
pub fn amf_hevc_intra_refresh(stream: &Negotiated) -> Option<AmfHevcIntraRefresh> {
    if stream.codec != 1 || !stream.intra_refresh || stream.width == 0 || stream.height == 0 {
        return None;
    }
    let columns = u64::from(stream.width).div_ceil(64);
    let rows = u64::from(stream.height).div_ceil(64);
    let blocks = columns * rows;
    Some(AmfHevcIntraRefresh {
        gop_frames: 120,
        ctbs_per_slot: (columns * rows.div_ceil(120)) as i64,
        fallback_ctbs_per_slot: blocks.div_ceil(blocks.clamp(1, 299)) as i64,
    })
}

/// Long-term references AMF keeps for AV1 when `amd_ltr_frames` is unset.
/// They let a client that lost frames recover with a small frame predicted
/// from an older one instead of a keyframe: under 80 ms air outages at
/// 1440p120 AV1 that frame was 15.5 KB against a 127 KB keyframe, and stalls
/// about 7 ms shorter, with every frame decoded.
pub const AMF_LTR_DEFAULT: i64 = 4;
/// Whether AMF tells clients it recovers by reference invalidation. The offer
/// comes before the client picks a codec, and every AMF codec keeps
/// long-term references for it.
pub fn amf_offers_invalidation(config: &Config) -> bool {
    config.integer("amd_ltr_frames", AMF_LTR_DEFAULT) > 0
}
/// The long-term references `amd_ltr_frames` asks AMF to keep for reference
/// invalidation, before the client's budget: H.264, HEVC and AV1, for every
/// client. H.264 and HEVC need their headers rewritten for a client that
/// dropped frames until the repair (`bitstream::RepairHeaders`): without it,
/// on 2026-10-10 the test client's d3d11va decoder rejected every HEVC
/// picture after an invalidation more than 8 frames on (AMF's four-bit POC),
/// and FFmpeg's hardware H.264 decoders kept a damaged picture after any
/// frame_num gap. With it, on 2026-10-28 every repair from 1 to 130 lost
/// frames decoded identically on software and d3d11va, and the cost estimate
/// (`ltr::References`) repaired only where the repair was the smaller frame.
pub fn amf_ltr_requested(config: &Config, stream: &Negotiated) -> usize {
    if stream.codec > 2 {
        return 0;
    }
    config
        .integer("amd_ltr_frames", AMF_LTR_DEFAULT)
        .clamp(0, 4) as usize
}
/// AMF keeps LTR anchors alongside the rolling short-term reference. The
/// decoder's negotiated budget includes both; zero means it imposed no limit.
pub fn amf_ltr_frames(config: &Config, stream: &Negotiated) -> usize {
    if amf_intra_refresh(stream) {
        return 0;
    }
    let requested = amf_ltr_requested(config, stream);
    if stream.references == 0 {
        requested
    } else {
        requested.min(stream.references.saturating_sub(1) as usize)
    }
}

/// The capability that counts a codec's encoder engines and the property
/// that lets the driver split one frame across them. H.264 has neither.
pub fn amf_multi_instance(codec: u8) -> Option<(&'static str, &'static str)> {
    match codec {
        1 => Some(("HevcNumOfHwInstances", "HevcMultiHwInstanceEncode")),
        2 => Some(("Av1CapNumOfHwInstances", "Av1MultiHwInstanceEncode")),
        _ => None,
    }
}

/// What to write for split-frame encoding (`amd_split_frame`), given the
/// engines the caps report and the value the driver has before Init. AMD
/// documents the property as a hint: the driver still decides whether a frame
/// is split. `auto` asks for it only where the driver left it off, as the
/// original host did. A single engine gets nothing written in any mode, and a
/// driver that rejects the property keeps the stream.
pub fn amf_split_frame(
    config: &Config,
    codec: u8,
    instances: i64,
    current: Option<bool>,
) -> Option<Property> {
    let (_, name) = amf_multi_instance(codec)?;
    if instances <= 1 {
        return None;
    }
    let on = match tristate(config, "amd_split_frame", None) {
        Some(on) => on,
        None if current == Some(true) => return None,
        None => true,
    };
    Some(Property {
        name: name.into(),
        value: Value::Boolean(on),
        required: false,
    })
}

/// Only absent/empty settings or `auto` allow the host's pace-based cap.
pub fn amf_frame_cap_automatic(config: &Config) -> bool {
    config
        .get("amd_max_frame_size", "auto")
        .trim()
        .trim_matches('"')
        .eq_ignore_ascii_case("auto")
}

/// How much under the pace-based frame cap the host writes to AMF, learned
/// from keyframes. #59's rc.4 logs had an AV1 recovery keyframe of 1,908,217 B
/// against a 1,166,893 B cap (1.64 times): AMF does not hold keyframes to the
/// cap, so they left without full FEC. A keyframe over 1.1 times its target
/// lowers the scale to what would have fit (`written / bytes`), never below
/// [`KeyframeCapScale::FLOOR`]; a keyframe that kept to the lower cap and
/// would still fit at the next step up relaxes it by
/// [`KeyframeCapScale::RELAX`], back to 1. The written cap holds every frame,
/// so a driver that ignores it for keyframes must not keep inter frames under
/// it: a keyframe over the target under a lowered cap, no smaller against its
/// target than the one that lowered it, returns the scale to 1 for the
/// encoder's life. Only keyframes feed it (IDR, recovery, a re-encoded cut);
/// a new encoder starts at 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyframeCapScale {
    scale: f64,
    /// The overshoot (bytes over target) of the keyframe that last lowered it.
    lowered_at: Option<f64>,
    /// A lower cap did not shrink keyframes: the driver ignores it.
    ignored: bool,
}
impl Default for KeyframeCapScale {
    fn default() -> Self {
        Self {
            scale: 1.,
            lowered_at: None,
            ignored: false,
        }
    }
}
impl KeyframeCapScale {
    pub const FLOOR: f64 = 0.5;
    /// More than the 10% rise AMF's frame cap filter ignores as jitter, so
    /// each step reaches the driver.
    pub const RELAX: f64 = 1.125;
    pub fn scale(&self) -> f64 {
        self.scale
    }
    /// The cap to write for `target`: scaled, but never under `floor` (two
    /// nominal frames) unless the target itself is.
    pub fn apply(&self, target: u64, floor: u64) -> u64 {
        ((target as f64 * self.scale).round() as u64).clamp(floor.min(target), target)
    }
    /// A keyframe of `bytes` came out against `target`, with `written` the
    /// cap the driver held at its submission. Returns, once, the overshoots
    /// (bytes over target) of the keyframe that lowered the cap and of this
    /// one when the driver proved to ignore the lower cap and the scale went
    /// back to 1 for good.
    pub fn observe(&mut self, bytes: u64, target: u64, written: u64) -> Option<(f64, f64)> {
        if target == 0 || written == 0 || self.ignored {
            return None;
        }
        let (bytes, target, written) = (bytes as f64, target as f64, written as f64);
        let next = (self.scale * Self::RELAX).min(1.);
        if bytes > 1.1 * target {
            let overshoot = bytes / target;
            if let Some(lowered) = self
                .lowered_at
                .filter(|at| written < target && overshoot >= *at)
            {
                self.scale = 1.;
                self.lowered_at = None;
                self.ignored = true;
                return Some((lowered, overshoot));
            }
            self.lowered_at = Some(overshoot);
            self.scale = self.scale.min(written / bytes).max(Self::FLOOR);
        } else if self.scale < 1.
            && bytes >= 0.9 * written
            && bytes * next / self.scale <= 1.1 * target
        {
            // Bound by the lower cap but within it, and a keyframe as much
            // larger as the next step's cap would still fit.
            self.scale = next;
        }
        None
    }
}

/// The cap AMF gets for the pace-based `target`: scaled by `scale` for an
/// automatic cap in frame mode only. An explicit `amd_max_frame_size` is the
/// user's, and slice delivery plans its blocks from the cap itself.
pub fn keyframe_cap_bytes(
    automatic: bool,
    slice_blocks: u8,
    scale: &KeyframeCapScale,
    target: u64,
    floor: u64,
) -> u64 {
    if automatic && slice_blocks == 0 {
        scale.apply(target, floor)
    } else {
        target
    }
}

/// Apply after the frame rate and target bitrate. Start automatic AMF caps at
/// two nominal budgets until the host supplies its current pace before input;
/// an explicit value wins over pacing and zero keeps the driver's limit.
/// Frame caps are best-effort so an older driver can keep streaming;
/// explicitly requested peak and VBV limits stay strict.
pub fn amf_rate_control(config: &Config, stream: &Negotiated) -> Result<Vec<Property>> {
    let prefix = match stream.codec {
        0 => "",
        1 => "Hevc",
        2 => "Av1",
        _ => bail!("invalid AMF codec"),
    };
    let bitrate = f64::from(stream.bitrate_kbps) * 1000.;
    let frame_bits = bitrate * 1000. / f64::from(stream.fps_millihz());
    let mut output = Vec::new();
    for (key, suffix, minimum, maximum, scale) in [
        ("amd_peak_bitrate_ratio", "PeakBitrate", 1., 2., bitrate),
        (
            "amd_vbv_buffer_frames",
            "VBVBufferSize",
            0.5,
            2.,
            frame_bits,
        ),
        (
            "amd_max_frame_size",
            if stream.codec == 2 {
                "MaxCompressedFrameSize"
            } else {
                "MaxAUSize"
            },
            1.,
            8.,
            frame_bits,
        ),
    ] {
        let value = if key == "amd_max_frame_size" && amf_frame_cap_automatic(config) {
            Ok(2.)
        } else {
            config.get(key, "0").trim().trim_matches('"').parse::<f64>()
        };
        let value = match value {
            Ok(0.) => continue,
            Ok(value) if (minimum..=maximum).contains(&value) => value,
            _ => bail!(
                "{key} must be {}0 (driver default) or {minimum}–{maximum}",
                if key == "amd_max_frame_size" {
                    "auto, "
                } else {
                    ""
                }
            ),
        };
        output.push(Property {
            name: format!("{prefix}{suffix}"),
            value: Value::Integer((value * scale).round() as i64),
            required: key != "amd_max_frame_size",
        });
    }
    Ok(output)
}

pub fn amf(config: &Config, stream: &Negotiated) -> Result<Vec<Property>> {
    let codec = stream.codec;
    if codec > 2 {
        bail!("invalid AMF codec");
    }
    let blocks = slice_delivery(config, codec);
    // Refresh stripes and slice output interact; slice delivery wins.
    let intra_refresh = amf_intra_refresh(stream) && blocks == 0;
    let prefix = match codec {
        0 => "",
        1 => "Hevc",
        _ => "Av1",
    };
    let mut output = vec![];
    let mut add = |name: String, value: Value, required: bool| {
        output.push(Property {
            name,
            value,
            required,
        })
    };
    let usage = match config.get("amd_usage", "ultralowlatency") {
        "auto" => None,
        "transcoding" => Some(0),
        "lowlatency" => Some(if codec == 2 { 1 } else { 2 }),
        "webcam" => Some(3),
        "high_quality" => Some(4),
        "lowlatency_high_quality" => Some(5),
        other => {
            crate::config::fallback("amd_usage", other, "ultralowlatency");
            Some(if codec == 2 { 2 } else { 1 })
        }
    };
    if let Some(value) = usage {
        add(format!("{prefix}Usage"), Value::Integer(value), true);
    }
    let quality = match config.get("amd_quality", "speed") {
        "auto" => None,
        "balanced" => Some([0, 5, 70][codec as usize]),
        "quality" => Some([2, 0, 30][codec as usize]),
        other => {
            crate::config::fallback("amd_quality", other, "speed");
            Some([1, 10, 100][codec as usize])
        }
    };
    if let Some(value) = quality {
        add(
            format!("{prefix}QualityPreset"),
            Value::Integer(value),
            true,
        );
    }
    let requested_rc = match config.get("amd_rc", "vbr_latency") {
        "auto" => None,
        "cqp" => Some(0),
        "cbr" => Some(if codec == 0 { 1 } else { 3 }),
        "vbr_peak" => Some(2),
        "qvbr" => Some(4),
        "hqvbr" => Some(5),
        "hqcbr" => Some(6),
        other => {
            crate::config::fallback("amd_rc", other, "vbr_latency");
            Some(if codec == 0 { 3 } else { 1 })
        }
    };
    // AMF's PA accepts NV12, not the P010 used by HDR and ten-bit SDR.
    let rc = if stream.ten_bit() && requested_rc.is_some_and(|r| r >= 4) {
        tracing::warn!(
            "AMF quality rate control requires 8-bit SDR pre-analysis; ten-bit input uses peak VBR instead. Select peak VBR or use 8-bit SDR for quality rate control"
        );
        Some(2)
    } else {
        requested_rc
    };
    if let Some(value) = rc {
        add(
            format!("{prefix}RateControlMethod"),
            Value::Integer(value),
            config.values.contains_key("amd_rc") || value >= 4,
        );
    }
    if stream.ten_bit() && config.boolean("amd_preanalysis", false) {
        tracing::warn!(
            "AMF pre-analysis requires NV12 8-bit SDR input; disabled for ten-bit input. Use 8-bit SDR for pre-analysis, or leave pre-analysis disabled"
        );
    }
    // Quality VBR modes require PA. Keep the original one-frame lookahead.
    let preanalysis = !stream.ten_bit()
        && (config.boolean("amd_preanalysis", false) || rc.is_some_and(|r| r >= 4));
    add(
        format!("{prefix}EnablePreAnalysis"),
        Value::Boolean(preanalysis),
        preanalysis,
    );
    if preanalysis {
        add("PALookAheadBufferDepth".into(), Value::Integer(1), true);
    }
    if rc == Some(4) {
        let q = config.integer("amd_qvbr_quality_level", 0);
        if (1..=51).contains(&q) {
            add(format!("{prefix}QvbrQualityLevel"), Value::Integer(q), true);
        } else if q != 0 {
            crate::config::invalid("amd_qvbr_quality_level", &q.to_string());
        }
    }
    // On by default for HEVC and AV1. For H.264 it cost 1.0 and 9.4 VMAF at
    // 1440p120 and 20 Mbps on two game clips, at equal encode time, so it is
    // off there unless asked for (PERFORMANCE_WORK.md, October 8).
    if let Some(on) = tristate(config, "amd_vbaq", Some(codec != 0)) {
        let on = on && rc != Some(0);
        if codec == 2 {
            add("Av1AQMode".into(), Value::Integer(i64::from(on)), true);
        } else {
            add(format!("{prefix}EnableVBAQ"), Value::Boolean(on), true);
        }
    }
    add(
        format!("{prefix}EnforceHRD"),
        Value::Boolean(config.boolean("amd_enforce_hrd", false)),
        config.values.contains_key("amd_enforce_hrd"),
    );
    // Rate control must not drop a frame to stay on budget: a VRR client
    // shows a dropped frame as a held picture. Vibepollo sets it off too.
    // Keep AV1's existing driver policy until support is checked on hardware.
    if codec < 2 {
        add(
            format!("{prefix}RateControlSkipFrameEnable"),
            Value::Boolean(false),
            false,
        );
    }
    // Only on request, as the original backend does: forcing a queue
    // of one for every VRR client exposed an RDNA4 driver freeze
    // (AlkaidLab/foundation-sunshine#666), video stalling while audio plays.
    let queue = config.integer("amd_input_queue_size", 0).clamp(0, 32);
    if queue > 0 {
        add(
            format!("{prefix}InputQueueSize"),
            Value::Integer(queue),
            true,
        );
    }
    if let Some(on) = tristate(config, "amd_smart_access_video", None) {
        add(
            format!("{prefix}EnableEncoderSmartAccessVideo"),
            Value::Boolean(on),
            on,
        );
    }
    if codec < 2
        && let Some(on) = tristate(config, "amd_lowlatency_mode", None)
    {
        add("LowLatencyInternal".into(), Value::Boolean(on), true);
    }
    if let Some(on) = tristate(config, "amd_high_motion_quality_boost", None) {
        add(
            match codec {
                0 => "HighMotionQualityBoostEnable",
                1 => "HevcHighMotionQualityBoostEnable",
                _ => "Av1HighMotionQualityBoost",
            }
            .into(),
            Value::Boolean(on),
            false,
        );
    }
    if codec == 0 {
        add("Profile".into(), Value::Integer(100), true);
        match config.get("amd_coder", "auto") {
            "cabac" | "ac" => add("CABACEnable".into(), Value::Integer(1), true),
            "cavlc" | "vlc" => add("CABACEnable".into(), Value::Integer(2), true),
            other => crate::config::fallback("amd_coder", other, "auto"),
        }
    }
    if codec == 2 {
        if let Some(on) = tristate(config, "amd_av1_screen_content", None) {
            add("Av1ScreenContentTools".into(), Value::Boolean(on), true);
        }
        let mode = match config.get("amd_av1_latency_mode", "auto") {
            "none" => Some(0),
            "power_saving" => Some(1),
            "realtime" => Some(2),
            "lowest" => Some(3),
            other => {
                crate::config::fallback("amd_av1_latency_mode", other, "auto");
                None
            }
        };
        if let Some(mode) = mode {
            add("Av1EncodingLatencyMode".into(), Value::Integer(mode), true);
        }
        // Experiment, not in the console: whether frames carry their adapted
        // entropy contexts forward. The driver decides unless these are set;
        // the effective values are in the "AMF encoder settings" line.
        if let Some(on) = tristate(config, "amd_av1_cdf_update", None) {
            add("Av1CdfUpdate".into(), Value::Boolean(on), true);
        }
        let cdf_mode = config.integer("amd_av1_cdf_frame_end", -1);
        if cdf_mode >= 0 {
            add(
                "Av1CdfFrameEndUpdateMode".into(),
                Value::Integer(cdf_mode),
                true,
            );
        }
        let tiles = config.integer("amd_av1_tiles", 0);
        if !matches!(tiles, 0 | 1 | 2 | 4) {
            bail!("amd_av1_tiles must be 0, 1, 2 or 4");
        }
        // Tiles derived from the client's slice request are best-effort, like
        // slices below; a tile count the user set must apply.
        if tiles > 0 || stream.slices > 1 {
            add(
                "Av1NumTilesPerFrame".into(),
                Value::Integer(if tiles == 0 {
                    stream.slices.min(4) as i64
                } else {
                    tiles
                }),
                tiles > 0,
            );
        }
    } else if blocks > 0 {
        // Slice delivery needs one slice per block at least, and the driver
        // returning each slice as its own output; without either the stream
        // would send whole frames, so a refusal fails the encoder.
        add(format!("{prefix}OutputMode"), Value::Integer(1), true);
        add(
            format!("{prefix}SlicesPerFrame"),
            Value::Integer(i64::from(blocks).max(i64::from(stream.slices))),
            true,
        );
    } else if stream.slices > 1 {
        // The client's slice request is a decoder hint (Moonlight-Switch asks
        // for four even when it decodes in hardware). A driver that rejects
        // the count keeps its own and the stream starts, instead of failing
        // the encoder.
        add(
            format!("{prefix}SlicesPerFrame"),
            Value::Integer(stream.slices as i64),
            false,
        );
    }
    let references = stream
        .references
        .max(if intra_refresh && codec == 0 { 2 } else { 0 });
    if references > 0 {
        add(
            format!("{prefix}MaxNumRefFrames"),
            Value::Integer(references as i64),
            true,
        );
    }
    if codec != 1 {
        add(
            format!("{prefix}BPicturesPattern"),
            Value::Integer(0),
            false,
        );
    }
    if intra_refresh {
        let block = if codec == 0 { 16 } else { 64 };
        let blocks = stream.width.div_ceil(block) * stream.height.div_ceil(block);
        // HEVC first establishes this existing policy. Its whole-row/GOP
        // override is applied together by the native backend so either
        // unsupported setting can restore this known streaming behavior.
        add(
            if codec == 0 {
                "IntraRefreshMBsNumberPerSlot"
            } else {
                "HevcIntraRefreshCTBsNumberPerSlot"
            }
            .into(),
            Value::Integer(blocks.div_ceil(blocks.clamp(1, 299)) as i64),
            true,
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
