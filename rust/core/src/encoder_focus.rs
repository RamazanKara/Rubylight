//! Encoder focus: an AMF ROI importance map that gives still blocks (text,
//! HUDs) more bits than motion. This module holds the policy and the CPU
//! references of the GPU map shader (`focus_map_cs` in `color.hlsl`); see
//! `docs/encoder-focus-design.md`.

/// The `encoder_focus` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    Auto,
    Off,
    Static,
    Centre,
}

impl Requested {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            "static" => Ok(Self::Static),
            "centre" | "center" => Ok(Self::Centre),
            other => Err(format!(
                "encoder_focus={other:?} is not one of auto, off, static, centre"
            )),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Static => "static",
            Self::Centre => "centre",
        }
    }
    /// The value of the session atomic.
    pub fn wire(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::Off => 1,
            Self::Static => 2,
            Self::Centre => 3,
        }
    }
    pub fn from_wire(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Auto,
            1 => Self::Off,
            2 => Self::Static,
            3 => Self::Centre,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Static,
    Centre,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Centre => "centre",
        }
    }
    /// The shader's `focusMode` constant.
    pub fn shader(self) -> u32 {
        match self {
            Self::Static => 0,
            Self::Centre => 1,
        }
    }
}

/// `encoder_focus_levels`, a hidden tuning key for measurement only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// Importance at age 0, 0..=10.
    pub moving: u32,
    /// Importance from age `ramp` on, 0..=10.
    pub still: u32,
    /// Frames from `moving` to `still`, 1..=255.
    pub ramp: u32,
    /// Importance `centre` mode adds at most, 0..=10.
    pub centre: u32,
    /// Share of a 64x64 block's 16x16 blocks that must be still, 0..=1.
    pub ctb_still: f32,
    /// Low code bits ignored by the still test, 0..=8; 0 is exact.
    pub noise_bits: u32,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            moving: 2,
            still: 6,
            ramp: 4,
            centre: 4,
            ctb_still: 1.0,
            noise_bits: 0,
        }
    }
}

impl Levels {
    /// Parses `moving=2,still=6,ramp=4,centre=4,ctb_still=1.0,noise_bits=0`;
    /// keys left out keep their defaults and values are clamped.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut levels = Self::default();
        for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, raw) = part
                .split_once('=')
                .ok_or_else(|| format!("encoder_focus_levels: {part:?} has no '='"))?;
            let (key, raw) = (key.trim(), raw.trim());
            let number = || {
                raw.parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("encoder_focus_levels: {key}={raw:?} is not a number"))
            };
            let integer = |max: f32| number().map(|v| v.round().clamp(0.0, max) as u32);
            match key {
                "moving" => levels.moving = integer(10.0)?,
                "still" => levels.still = integer(10.0)?,
                "ramp" => levels.ramp = integer(255.0)?.max(1),
                "centre" | "center" => levels.centre = integer(10.0)?,
                "ctb_still" => levels.ctb_still = number()?.clamp(0.0, 1.0),
                "noise_bits" => levels.noise_bits = integer(8.0)?,
                _ => return Err(format!("encoder_focus_levels: unknown key {key:?}")),
            }
        }
        Ok(levels)
    }
    /// `ctb_still` in the shader's fixed point (x256).
    pub fn ctb_still_256(&self) -> u32 {
        (self.ctb_still.clamp(0.0, 1.0) * 256.0).round() as u32
    }
}

impl std::fmt::Display for Levels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "moving={},still={},ramp={},centre={},ctb_still={},noise_bits={}",
            self.moving, self.still, self.ramp, self.centre, self.ctb_still, self.noise_bits
        )
    }
}

/// Why focus is off; the strings are stable (logs, session warning, API).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Pyrowave,
    CodecUnsupported,
    Packed444,
    D3d12ConversionUnavailable,
    SplitFrame,
    NoAmf,
    NotProbed,
    InvalidValue,
}

impl Reason {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pyrowave => "pyrowave",
            Self::CodecUnsupported => "codec_unsupported",
            Self::Packed444 => "packed_444",
            Self::D3d12ConversionUnavailable => "d3d12_conversion_unavailable",
            Self::SplitFrame => "split_frame",
            Self::NoAmf => "no_amf",
            Self::NotProbed => "not_probed",
            Self::InvalidValue => "invalid_value",
        }
    }
    pub fn from_wire(value: &str) -> Option<Self> {
        [
            Self::Pyrowave,
            Self::CodecUnsupported,
            Self::Packed444,
            Self::D3d12ConversionUnavailable,
            Self::SplitFrame,
            Self::NoAmf,
            Self::NotProbed,
            Self::InvalidValue,
        ]
        .into_iter()
        .find(|reason| reason.wire() == value)
    }
    /// The text for reasons the startup check does not explain itself.
    pub fn message(self) -> &'static str {
        match self {
            Self::Pyrowave => "PyroWave has no ROI map",
            Self::CodecUnsupported => "the driver reports no ROI map",
            Self::Packed444 => "packed 4:4:4 puts chroma in the luma plane",
            Self::D3d12ConversionUnavailable => "the D3D12 colour conversion is unavailable",
            Self::SplitFrame => "split-frame encoding ignores the ROI map",
            Self::NoAmf => "no AMF encoder",
            Self::NotProbed => "the startup encoder check did not test ROI maps",
            Self::InvalidValue => "encoder_focus has an unknown value",
        }
    }
}

/// The startup check's verdict for one codec and bit depth.
#[derive(Clone, Debug, PartialEq)]
pub enum Support {
    /// The ROI half took `delta` QP (q-index for AV1) less than the other.
    Verified {
        delta: f32,
    },
    Unsupported {
        reason: Reason,
        message: String,
    },
    NotProbed,
}

impl Support {
    /// The summary entry: `verified(+4.1)`, `unsupported(...)`, `not_probed`.
    pub fn summary(&self) -> String {
        match self {
            Self::Verified { delta } => format!("verified({delta:+.1})"),
            Self::Unsupported { message, .. } => format!("unsupported({message})"),
            Self::NotProbed => "not_probed".into(),
        }
    }
}

/// Index of a codec and bit depth in the startup check's results, in its
/// order: h264, hevc, hevc10, av1, av1_10.
pub fn support_index(codec: u8, ten_bit: bool) -> Option<usize> {
    match (codec, ten_bit) {
        (0, false) => Some(0),
        (1, false) => Some(1),
        (1, true) => Some(2),
        (2, false) => Some(3),
        (2, true) => Some(4),
        _ => None,
    }
}

pub const SUPPORT_NAMES: [&str; 5] = ["h264", "hevc", "hevc10", "av1", "av1_10"];

/// What a session can do, for [`resolve`].
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub codec: u8,
    pub ten_bit: bool,
    pub pyrowave: bool,
    pub packed_444: bool,
    /// The encoder converts on the D3D12 compute path.
    pub d3d12: bool,
    /// AMF pre-analysis is on.
    pub preanalysis: bool,
    pub support: Support,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resolved {
    On(Mode),
    /// `reason` is set when a mode was wanted but cannot run.
    Off {
        reason: Option<Reason>,
    },
}

/// What `auto` means per codec (0 H.264, 1 HEVC, 2 AV1). Every codec stays
/// off until the measurements in the design's section 9 pass.
pub const AUTO: [(u8, Option<Mode>); 3] = [(0, None), (1, None), (2, None)];

pub fn resolve(requested: Requested, context: &Context) -> Resolved {
    let mode = match requested {
        Requested::Off => return Resolved::Off { reason: None },
        Requested::Static => Mode::Static,
        Requested::Centre => Mode::Centre,
        Requested::Auto => {
            if context.preanalysis {
                return Resolved::Off { reason: None };
            }
            match AUTO.iter().find(|(codec, _)| *codec == context.codec) {
                Some((_, Some(mode))) => *mode,
                _ => return Resolved::Off { reason: None },
            }
        }
    };
    let reason = if context.pyrowave || context.codec == 3 {
        Some(Reason::Pyrowave)
    } else if context.packed_444 {
        Some(Reason::Packed444)
    } else if !context.d3d12 {
        Some(Reason::D3d12ConversionUnavailable)
    } else {
        match &context.support {
            Support::Verified { .. } => None,
            Support::Unsupported { reason, .. } => Some(*reason),
            Support::NotProbed => Some(Reason::NotProbed),
        }
    };
    match reason {
        Some(reason) => Resolved::Off {
            reason: Some(reason),
        },
        None => Resolved::On(mode),
    }
}

/// The codec's ROI block edge in pixels: 16 for H.264, 64 for HEVC and AV1.
pub fn block(codec: u8) -> u32 {
    if codec == 0 { 16 } else { 64 }
}

/// The ROI map's size in blocks for a coded frame.
pub fn grid(codec: u8, coded_width: u32, coded_height: u32) -> (u32, u32) {
    let block = block(codec);
    (coded_width.div_ceil(block), coded_height.div_ceil(block))
}

/// The still-age grid: one entry per conversion thread group of 8x8 threads,
/// each covering 16x16 luma pixels (`yuv420_cs` dispatches `(w/2)/8` groups).
pub fn age_grid(width: u32, height: u32) -> (u32, u32) {
    ((width / 2).div_ceil(8), (height / 2).div_ceil(8))
}

/// A codec block's importance, 0..=10. `centre_weight` is 0..=256 (see
/// [`centre_weight`]) and only counts in `Centre` mode. Mirrors `focus_map_cs`.
pub fn importance(age: u32, centre_weight: u32, mode: Mode, levels: &Levels) -> u32 {
    let ramp = levels.ramp.clamp(1, 255) as i32;
    let moving = levels.moving.min(10) as i32;
    let still = levels.still.min(10) as i32;
    let value =
        (moving + (still - moving) * age.min(ramp as u32) as i32 / ramp).clamp(0, 10) as u32;
    match mode {
        Mode::Static => value,
        Mode::Centre => {
            (value + (levels.centre.min(10) * centre_weight.min(256) + 128) / 256).min(10)
        }
    }
}

/// 256 when the centre of block `(x, y)` lies in the ellipse over the middle
/// half of a `width` x `height` frame or within 128 px of the visible cursor,
/// otherwise 0. Integer math, identical in `focus_map_cs`.
pub fn centre_weight(
    x: u32,
    y: u32,
    block: u32,
    width: u32,
    height: u32,
    cursor: Option<(i32, i32)>,
) -> u32 {
    let cx = (x * block + block / 2) as i32;
    let cy = (y * block + block / 2) as i32;
    // Doubled coordinates keep the frame centre whole; the semi-axes are
    // width/4 and height/4, so width/2 and height/2 when doubled.
    let axis = |doubled: i32, half: u32| -> u32 {
        (doubled.unsigned_abs().min(65_535) * 1024) / half.max(1)
    };
    let nx = axis(2 * cx - width as i32, width / 2);
    let ny = axis(2 * cy - height as i32, height / 2);
    if nx.min(65_535).pow(2).saturating_add(ny.min(65_535).pow(2)) <= 1024 * 1024 {
        return 256;
    }
    if let Some((px, py)) = cursor {
        let dx = (cx - px).unsigned_abs().min(32_767);
        let dy = (cy - py).unsigned_abs().min(32_767);
        if dx * dx + dy * dy <= 128 * 128 {
            return 256;
        }
    }
    0
}

/// A 64x64 block's age from the ages of its 16x16 blocks: the k-th largest,
/// with `k = max(1, ceil(ctb_still * n))`, so the block counts as still for
/// `a` frames once `ctb_still` of its blocks have. Mirrors `focus_map_cs`.
pub fn ctb_age(ages: &[u32], ctb_still_256: u32) -> u32 {
    let n = ages.len() as u32;
    if n == 0 {
        return 0;
    }
    let k = ((ctb_still_256.min(256) * n).div_ceil(256)).max(1) as usize;
    let mut sorted = ages.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    sorted[k - 1]
}

/// Whether a frame gets the map: never on an IDR, an LTR recovery picture,
/// the first frame after an encoder recreate, or with a uniform map.
pub fn attach_for(idr: bool, recover: bool, first_after_create: bool, uniform: bool) -> bool {
    !(idr || recover || first_after_create || uniform)
}

/// The encoder's focus state, which the session publishes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FocusState {
    pub effective: Option<Mode>,
    pub reason: Option<Reason>,
    pub message: Option<String>,
}

/// Counters for the 5 s `encoder focus` line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub maps: u64,
    pub uniform: u64,
    pub stale: u64,
    pub skipped_recovery: u64,
    /// Mean QP of low- minus high-importance blocks, with `encoder_focus_verify`.
    pub qp_delta: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(codec: u8, support: Support) -> Context {
        Context {
            codec,
            ten_bit: false,
            pyrowave: codec == 3,
            packed_444: false,
            d3d12: true,
            preanalysis: false,
            support,
        }
    }

    #[test]
    fn requested_parses_and_round_trips() {
        assert_eq!(Requested::parse(""), Ok(Requested::Auto));
        assert_eq!(Requested::parse(" Auto "), Ok(Requested::Auto));
        assert_eq!(Requested::parse("off"), Ok(Requested::Off));
        assert_eq!(Requested::parse("static"), Ok(Requested::Static));
        assert_eq!(Requested::parse("center"), Ok(Requested::Centre));
        assert!(Requested::parse("bogus").is_err());
        for r in [
            Requested::Auto,
            Requested::Off,
            Requested::Static,
            Requested::Centre,
        ] {
            assert_eq!(Requested::from_wire(r.wire()), Some(r));
            assert_eq!(Requested::parse(r.as_str()), Ok(r));
        }
        assert_eq!(Requested::from_wire(4), None);
    }

    #[test]
    fn levels_parse_defaults_and_clamps() {
        assert_eq!(Levels::parse(""), Ok(Levels::default()));
        let text = Levels::default().to_string();
        assert_eq!(Levels::parse(&text), Ok(Levels::default()));
        let levels =
            Levels::parse("moving=-3, still=12,ramp=0,center=5,ctb_still=1.5,noise_bits=9")
                .unwrap();
        assert_eq!(
            levels,
            Levels {
                moving: 0,
                still: 10,
                ramp: 1,
                centre: 5,
                ctb_still: 1.0,
                noise_bits: 8
            }
        );
        assert_eq!(Levels::parse("ctb_still=0.5").unwrap().ctb_still_256(), 128);
        assert!(Levels::parse("moving").is_err());
        assert!(Levels::parse("moving=x").is_err());
        assert!(Levels::parse("speed=1").is_err());
    }

    #[test]
    fn reasons_have_stable_wire_strings() {
        let all = [
            (Reason::Pyrowave, "pyrowave"),
            (Reason::CodecUnsupported, "codec_unsupported"),
            (Reason::Packed444, "packed_444"),
            (
                Reason::D3d12ConversionUnavailable,
                "d3d12_conversion_unavailable",
            ),
            (Reason::SplitFrame, "split_frame"),
            (Reason::NoAmf, "no_amf"),
            (Reason::NotProbed, "not_probed"),
            (Reason::InvalidValue, "invalid_value"),
        ];
        for (reason, wire) in all {
            assert_eq!(reason.wire(), wire);
            assert_eq!(Reason::from_wire(wire), Some(reason));
        }
        assert_eq!(Reason::from_wire("other"), None);
    }

    #[test]
    fn support_summaries_and_indices() {
        assert_eq!(
            Support::Verified { delta: 4.06 }.summary(),
            "verified(+4.1)"
        );
        let unsupported = Support::Unsupported {
            reason: Reason::CodecUnsupported,
            message: "no block QP map".into(),
        };
        assert_eq!(unsupported.summary(), "unsupported(no block QP map)");
        assert_eq!(Support::NotProbed.summary(), "not_probed");
        let indices: Vec<_> = [(0, false), (1, false), (1, true), (2, false), (2, true)]
            .into_iter()
            .map(|(c, t)| support_index(c, t))
            .collect();
        assert_eq!(indices, [Some(0), Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(support_index(0, true), None);
        assert_eq!(support_index(3, false), None);
    }

    #[test]
    fn auto_is_off_for_every_codec() {
        let verified = Support::Verified { delta: 4.0 };
        for codec in 0..=2 {
            assert_eq!(
                resolve(Requested::Auto, &context(codec, verified.clone())),
                Resolved::Off { reason: None }
            );
        }
        assert!(AUTO.iter().all(|(_, mode)| mode.is_none()));
    }

    #[test]
    fn explicit_modes_run_only_where_verified() {
        let verified = Support::Verified { delta: 4.0 };
        assert_eq!(
            resolve(Requested::Off, &context(1, verified.clone())),
            Resolved::Off { reason: None }
        );
        assert_eq!(
            resolve(Requested::Static, &context(1, verified.clone())),
            Resolved::On(Mode::Static)
        );
        assert_eq!(
            resolve(Requested::Centre, &context(0, verified.clone())),
            Resolved::On(Mode::Centre)
        );
        let off = |reason| Resolved::Off {
            reason: Some(reason),
        };
        assert_eq!(
            resolve(Requested::Static, &context(3, verified.clone())),
            off(Reason::Pyrowave)
        );
        let mut packed = context(1, verified.clone());
        packed.packed_444 = true;
        assert_eq!(resolve(Requested::Static, &packed), off(Reason::Packed444));
        let mut d3d11 = context(1, verified.clone());
        d3d11.d3d12 = false;
        assert_eq!(
            resolve(Requested::Static, &d3d11),
            off(Reason::D3d12ConversionUnavailable)
        );
        let unsupported = Support::Unsupported {
            reason: Reason::SplitFrame,
            message: "x".into(),
        };
        assert_eq!(
            resolve(Requested::Static, &context(2, unsupported)),
            off(Reason::SplitFrame)
        );
        assert_eq!(
            resolve(Requested::Static, &context(2, Support::NotProbed)),
            off(Reason::NotProbed)
        );
        // Pre-analysis turns auto off, but an explicit mode still runs.
        let mut pa = context(1, verified);
        pa.preanalysis = true;
        assert_eq!(
            resolve(Requested::Auto, &pa),
            Resolved::Off { reason: None }
        );
        assert_eq!(resolve(Requested::Static, &pa), Resolved::On(Mode::Static));
    }

    #[test]
    fn grids_follow_the_codec_block() {
        assert_eq!(block(0), 16);
        assert_eq!(block(1), 64);
        assert_eq!(grid(0, 1920, 1088), (120, 68));
        assert_eq!(grid(1, 1920, 1088), (30, 17));
        assert_eq!(grid(2, 3840, 2160), (60, 34));
        assert_eq!(age_grid(1920, 1080), (120, 68));
        assert_eq!(age_grid(1366, 768), (86, 48));
    }

    #[test]
    fn importance_ramps_from_moving_to_still() {
        let levels = Levels::default();
        let ages: Vec<_> = (0..7)
            .map(|age| importance(age, 0, Mode::Static, &levels))
            .collect();
        assert_eq!(ages, [2, 3, 4, 5, 6, 6, 6]);
        assert_eq!(importance(255, 256, Mode::Static, &levels), 6);
        assert_eq!(importance(0, 256, Mode::Centre, &levels), 6);
        assert_eq!(importance(9, 256, Mode::Centre, &levels), 10);
        assert_eq!(importance(9, 0, Mode::Centre, &levels), 6);
        let falling = Levels {
            moving: 8,
            still: 1,
            ramp: 3,
            ..levels
        };
        let ages: Vec<_> = (0..4)
            .map(|age| importance(age, 0, Mode::Static, &falling))
            .collect();
        assert_eq!(ages, [8, 6, 4, 1]);
        let wild = Levels {
            moving: 40,
            still: 40,
            ramp: 0,
            centre: 40,
            ..levels
        };
        assert_eq!(importance(0, 256, Mode::Centre, &wild), 10);
    }

    #[test]
    fn centre_weight_covers_the_middle_and_the_cursor() {
        // 1920x1080 at 16 px: the middle block is in, the corner block out.
        assert_eq!(centre_weight(60, 33, 16, 1920, 1080, None), 256);
        assert_eq!(centre_weight(0, 0, 16, 1920, 1080, None), 0);
        // The ellipse reaches width/4 sideways: x = 1440 is its edge.
        assert_eq!(centre_weight(1424 / 16, 33, 16, 1920, 1080, None), 256);
        assert_eq!(centre_weight(1472 / 16, 33, 16, 1920, 1080, None), 0);
        // A cursor in the corner pulls in blocks within 128 px.
        assert_eq!(centre_weight(0, 0, 16, 1920, 1080, Some((40, 40))), 256);
        assert_eq!(centre_weight(10, 0, 16, 1920, 1080, Some((8, 8))), 0);
        assert_eq!(centre_weight(0, 0, 64, 3840, 2160, Some((-5000, -5000))), 0);
    }

    #[test]
    fn ctb_age_follows_ctb_still() {
        let ages = [9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 0];
        assert_eq!(ctb_age(&ages, 256), 0, "1.0 needs every block still");
        assert_eq!(ctb_age(&ages, 240), 9, "15 of 16 still is enough at 0.9375");
        assert_eq!(ctb_age(&ages, 0), 9);
        assert_eq!(ctb_age(&[3, 7], 128), 7);
        assert_eq!(ctb_age(&[3, 7], 129), 3);
        assert_eq!(ctb_age(&[], 256), 0);
    }

    #[test]
    fn maps_skip_keyframes_recovery_first_frames_and_uniform_maps() {
        assert!(attach_for(false, false, false, false));
        assert!(!attach_for(true, false, false, false));
        assert!(!attach_for(false, true, false, false));
        assert!(!attach_for(false, false, true, false));
        assert!(!attach_for(false, false, false, true));
    }
}
