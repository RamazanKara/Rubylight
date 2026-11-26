//! Frame Warp 2: the request (0x5543) and the per-frame meta (0x5549), with the model, tags and
//! mask of [`crate::frame_warp`].
//!
//! A host lists [`SERVERINFO_FIELD`] in serverinfo, and the client sends its request as
//! [`RequestV2`] (version 2, twelve bytes): on or off, the stick's pad, the widest mask and the
//! lens, then whose gyro turns the camera, how its axes map and an optional limit for the shift.
//! Frame Warp before 7.0 took an eight-byte version 1 request and listed
//! [`crate::frame_warp::LEGACY_SERVERINFO_FIELD`]; neither end speaks it any more. A host ignores
//! a request that is not version 2, and a client sends nothing to a host without the field, so a
//! client and a host of different generations stream without Frame Warp.
//!
//! The host sends a [`FrameWarpV2Meta`] (0x5549, unreliable) with each frame's tags: how sure it
//! is that the camera, not a menu, a cutscene or a cursor, moves the picture
//! ([`FrameWarpV2Meta::camera_confidence`]), how steady its HUD mask is, the gyro packet the frame
//! reflects and the gyro's gain, and how to fill the strip a shift uncovers. The client adds the gyro's part to the shift,
//!
//! ```text
//! shift += gyro_gain * integral(gyro_rate(packet) - bias, tau_g..now)
//! ```
//!
//! where `tau_g` is the client's send time of gyro packet `gyro_seq` plus `gyro_held_us`,
//! [`gyro_rate`] maps a motion packet (0x5500_0006, degrees per second) to yaw and pitch in
//! radians per second, and both ends estimate `bias` alike from the packets ([`GyroBias`]). It
//! fades the whole shift with the confidence: quickly down, slowly back up.
//!
//! All integers are little-endian; floats are IEEE 754 binary32.

use crate::frame_warp::{FrameWarpRequest, MAX_CONTROLLERS, MAX_GAIN, NO_CONTROLLER};

/// Host to client: a frame's [`FrameWarpV2Meta`] (unreliable).
pub const FRAME_WARP_V2_META_MESSAGE_TYPE: u16 = 0x5549;
/// Serverinfo element: the host runs Frame Warp 2 ([`SERVERINFO_VERSION`]).
pub const SERVERINFO_FIELD: &str = "FrameWarpProtocol";
/// The [`SERVERINFO_FIELD`] value a host that takes [`RequestV2`] lists.
pub const SERVERINFO_VERSION: u32 = 2;
/// Format version of the version 2 request and of [`FrameWarpV2Meta`].
pub const VERSION: u8 = 2;
pub const REQUEST_BYTES: usize = 12;
pub const META_BYTES: usize = 40;
/// Largest [`FrameWarpV2Meta::hud_guard`], in mask cells.
pub const MAX_HUD_GUARD: u8 = 8;
/// [`FrameWarpV2Meta::hud_guard`] a host sends unless it knows better.
pub const DEFAULT_HUD_GUARD: u8 = 2;
/// [`FrameWarpV2Meta::edge_fade_strength`] a host sends unless it knows better (about a third
/// darker at the far end of the fill).
pub const DEFAULT_EDGE_FADE: u8 = 90;
/// The shift's limit as a fraction of the picture when a request gives none.
pub const DEFAULT_MAX_OFFSET: f32 = 0.15;
/// Largest [`RequestV2::max_offset_milli`]: the whole picture.
pub const MAX_OFFSET_MILLI: u16 = 1000;

/// Request flag bits (byte 3).
pub mod request_flags {
    pub const LENS: u8 = 1 << 0;
    pub const GYRO: u8 = 1 << 1;
}

/// [`FrameWarpV2Meta::flags`] bits.
pub mod meta_flags {
    /// The gyro gain fits the game well enough to shift by it.
    pub const GYRO_LOCKED: u8 = 1 << 0;
    /// The picture just cut to another scene.
    pub const SCENE_CUT: u8 = 1 << 1;
    /// A pointer shows on the host: a menu or a pause, not a camera.
    pub const CURSOR_VISIBLE_PAUSE: u8 = 1 << 2;
    /// The HUD mask's edges are soft: the client probes past them ([`super::FrameWarpV2Meta::hud_guard`]).
    pub const HUD_SOFT_EDGE: u8 = 1 << 3;
    /// The model locked from the game's saved prior, confirmed by this stream's first turn.
    pub const PRIOR_WARM_LOCKED: u8 = 1 << 4;
    /// The gyro gain's yaw is the motion packet's z axis, not its y axis: what the host found
    /// for a request that left the axis to it ([`super::YawSource::Auto`]).
    pub const GYRO_YAW_Z: u8 = 1 << 5;
    pub const ALL: u8 = 0x3F;
}

/// Which motion packet axis turns the camera left and right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum YawSource {
    /// The y axis: a pad held level.
    #[default]
    Y,
    /// The z axis: a device held upright.
    Z,
    /// The host finds it from how the picture moves ([`meta_flags::GYRO_YAW_Z`]).
    Auto,
}

/// How a gyro's axes map to the camera.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GyroAxes {
    pub invert_yaw: bool,
    pub invert_pitch: bool,
    pub yaw: YawSource,
}

impl GyroAxes {
    fn encode(self) -> u8 {
        let yaw = match self.yaw {
            YawSource::Y => 0,
            YawSource::Z => 1,
            YawSource::Auto => 2,
        };
        u8::from(self.invert_yaw) | u8::from(self.invert_pitch) << 1 | yaw << 2
    }

    /// Yaw source 3 rejects; the high four bits are a later version's.
    fn decode(byte: u8) -> Option<Self> {
        let yaw = match byte >> 2 & 3 {
            0 => YawSource::Y,
            1 => YawSource::Z,
            2 => YawSource::Auto,
            _ => return None,
        };
        Some(Self {
            invert_yaw: byte & 1 != 0,
            invert_pitch: byte & 2 != 0,
            yaw,
        })
    }
}

/// Client to host: start or stop Frame Warp (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestV2 {
    /// On or off, the stick's pad, the widest mask and the lens.
    pub base: FrameWarpRequest,
    /// Turn by the gyro of [`RequestV2::gyro_controller`] too.
    pub gyro: bool,
    /// The pad whose motion packets carry the gyro (0 to 15); `None` is the stick's pad.
    pub gyro_controller: Option<u8>,
    pub axes: GyroAxes,
    /// The client's limit of the shift in thousandths of the picture; 0 leaves it to the host
    /// ([`DEFAULT_MAX_OFFSET`]).
    pub max_offset_milli: u16,
}

impl RequestV2 {
    /// A request with nothing beyond `base`.
    pub fn new(base: FrameWarpRequest) -> Self {
        Self {
            base,
            gyro: false,
            gyro_controller: None,
            axes: GyroAxes::default(),
            max_offset_milli: 0,
        }
    }

    pub fn encode(&self) -> [u8; REQUEST_BYTES] {
        let base = self.base.encode_prefix();
        let mut out = [0u8; REQUEST_BYTES];
        out[..8].copy_from_slice(&base);
        out[0] = VERSION;
        out[3] = u8::from(self.base.lens) | if self.gyro { request_flags::GYRO } else { 0 };
        out[8] = match self.gyro_controller {
            Some(number) if number < MAX_CONTROLLERS => number,
            _ => NO_CONTROLLER,
        };
        out[9] = self.axes.encode();
        out[10..12].copy_from_slice(&self.max_offset_milli.min(MAX_OFFSET_MILLI).to_le_bytes());
        out
    }

    /// Parses a request; `None` for an older client's version 1 request (or anything shorter
    /// than [`REQUEST_BYTES`]) and for anything that rejects: version 0, an `enabled` other than
    /// 0 or 1, a pad over 15 (but 0xFF), a mask width over the largest, yaw source 3 and a limit
    /// over [`MAX_OFFSET_MILLI`]. Later versions may append.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < REQUEST_BYTES || data[0] < VERSION {
            return None;
        }
        let base = FrameWarpRequest::decode_prefix(data)?;
        let gyro_controller = match data[8] {
            NO_CONTROLLER => None,
            number if number < MAX_CONTROLLERS => Some(number),
            _ => return None,
        };
        let max_offset_milli = u16::from_le_bytes([data[10], data[11]]);
        if max_offset_milli > MAX_OFFSET_MILLI {
            return None;
        }
        Some(Self {
            base,
            gyro: data[3] & request_flags::GYRO != 0,
            gyro_controller,
            axes: GyroAxes::decode(data[9])?,
            max_offset_milli,
        })
    }

    /// The pad whose gyro turns the camera, if any.
    pub fn gyro_pad(&self) -> Option<u8> {
        if self.gyro {
            self.gyro_controller.or(self.base.controller)
        } else {
            None
        }
    }

    /// The shift's limit as a fraction of the picture.
    pub fn max_offset(&self) -> f32 {
        match self.max_offset_milli {
            0 => DEFAULT_MAX_OFFSET,
            milli => f32::from(milli.min(MAX_OFFSET_MILLI)) / 1000.0,
        }
    }
}

/// Host to client: how far to trust the camera for one frame, and the gyro's part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameWarpV2Meta {
    /// [`meta_flags`] bits.
    pub flags: u8,
    /// The [`crate::frame_warp::Model::generation`] in force when the frame was tagged.
    pub model_generation: u16,
    /// The video frame number, as in [`crate::frame_warp::Tag::frame`].
    pub frame: u32,
    /// How sure the host is that the camera moves the picture, 0 to 65535 for 0 to 1.
    pub camera_confidence: u16,
    /// How steady the HUD mask is, 0 to 65535 for 0 to 1.
    pub hud_stability: u16,
    /// Motion packets with a gyro reading received from the gyro's pad (wrapping), as of the
    /// frame's reflection time; 0 for none.
    pub gyro_seq: u32,
    /// How long the last of them had been held then, microseconds;
    /// [`crate::frame_warp::HELD_NONE`] for none.
    pub gyro_held_us: u32,
    /// Picture centre pixels per radian of yaw and of pitch, signed.
    pub gyro_gain: [f32; 2],
    /// The camera's yaw and pitch rate the host measured, milliradians per second.
    pub omega_mrad_s: [i32; 2],
    /// How dark the uncovered strip's far edge gets, 0 to 255 for none to black.
    pub edge_fade_strength: u8,
    /// Mask cells the client probes back along the shift for a background pixel when a shifted
    /// point lands on the HUD, 0 to [`MAX_HUD_GUARD`].
    pub hud_guard: u8,
    /// Gyro packets the host had received from the gyro's pad when it made this meta (wrapping,
    /// the low 16 bits): the client measures lost packets by it.
    pub gyro_received: u16,
}

impl Default for FrameWarpV2Meta {
    fn default() -> Self {
        Self {
            flags: 0,
            model_generation: 0,
            frame: 0,
            camera_confidence: 0,
            hud_stability: 0,
            gyro_seq: 0,
            gyro_held_us: crate::frame_warp::HELD_NONE,
            gyro_gain: [0.0; 2],
            omega_mrad_s: [0; 2],
            edge_fade_strength: DEFAULT_EDGE_FADE,
            hud_guard: DEFAULT_HUD_GUARD,
            gyro_received: 0,
        }
    }
}

impl FrameWarpV2Meta {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    /// [`FrameWarpV2Meta::camera_confidence`] from 0 to 1.
    pub fn confidence(&self) -> f32 {
        from_unit16(self.camera_confidence)
    }

    fn valid(&self) -> bool {
        self.gyro_gain
            .iter()
            .all(|g| g.is_finite() && g.abs() <= MAX_GAIN)
            && self.hud_guard <= MAX_HUD_GUARD
    }

    /// The wire form, or `None` for a gain out of bounds or a guard over [`MAX_HUD_GUARD`].
    /// Unknown flag bits are left out.
    pub fn encode(&self) -> Option<[u8; META_BYTES]> {
        if !self.valid() {
            return None;
        }
        let mut out = [0u8; META_BYTES];
        out[0] = VERSION;
        out[1] = self.flags & meta_flags::ALL;
        out[2..4].copy_from_slice(&self.model_generation.to_le_bytes());
        out[4..8].copy_from_slice(&self.frame.to_le_bytes());
        out[8..10].copy_from_slice(&self.camera_confidence.to_le_bytes());
        out[10..12].copy_from_slice(&self.hud_stability.to_le_bytes());
        out[12..16].copy_from_slice(&self.gyro_seq.to_le_bytes());
        out[16..20].copy_from_slice(&self.gyro_held_us.to_le_bytes());
        out[20..24].copy_from_slice(&self.gyro_gain[0].to_le_bytes());
        out[24..28].copy_from_slice(&self.gyro_gain[1].to_le_bytes());
        out[28..32].copy_from_slice(&self.omega_mrad_s[0].to_le_bytes());
        out[32..36].copy_from_slice(&self.omega_mrad_s[1].to_le_bytes());
        out[36] = self.edge_fade_strength;
        out[37] = self.hud_guard;
        out[38..40].copy_from_slice(&self.gyro_received.to_le_bytes());
        Some(out)
    }

    /// Parses a meta; version 0, a message shorter than [`META_BYTES`], a gain that is not
    /// finite or past [`MAX_GAIN`] and a guard over [`MAX_HUD_GUARD`] reject it. Unknown flag
    /// bits and bytes after [`META_BYTES`] are a later version's and are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < META_BYTES || data[0] == 0 {
            return None;
        }
        let word = |at: usize| [data[at], data[at + 1], data[at + 2], data[at + 3]];
        let meta = Self {
            flags: data[1] & meta_flags::ALL,
            model_generation: u16::from_le_bytes([data[2], data[3]]),
            frame: u32::from_le_bytes(word(4)),
            camera_confidence: u16::from_le_bytes([data[8], data[9]]),
            hud_stability: u16::from_le_bytes([data[10], data[11]]),
            gyro_seq: u32::from_le_bytes(word(12)),
            gyro_held_us: u32::from_le_bytes(word(16)),
            gyro_gain: [f32::from_le_bytes(word(20)), f32::from_le_bytes(word(24))],
            omega_mrad_s: [i32::from_le_bytes(word(28)), i32::from_le_bytes(word(32))],
            edge_fade_strength: data[36],
            hud_guard: data[37],
            gyro_received: u16::from_le_bytes([data[38], data[39]]),
        };
        meta.valid().then_some(meta)
    }
}

/// `value` from 0 to 1 as 0 to 65535 (rounded; outside and NaN clamp).
pub fn to_unit16(value: f32) -> u16 {
    if value.is_nan() {
        return 0;
    }
    (value.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

/// 0 to 65535 as 0 to 1.
pub fn from_unit16(value: u16) -> f32 {
    f32::from(value) / 65535.0
}

/// Radians per degree.
pub const RADIANS_PER_DEGREE: f32 = core::f32::consts::PI / 180.0;

/// A motion packet's gyro reading (x, y, z in degrees per second, as moonlight-common-c sends
/// it: x pitch, y yaw, z roll) as the camera's yaw and pitch rate in radians per second, by
/// `axes` with the yaw from z when `yaw_z` (else y; [`YawSource::Auto`] is the host's to
/// resolve). A reading that is not finite gives 0.
pub fn gyro_rate(xyz_deg_s: [f32; 3], axes: GyroAxes, yaw_z: bool) -> [f32; 2] {
    let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
    let yaw_z = match axes.yaw {
        YawSource::Y => false,
        YawSource::Z => true,
        YawSource::Auto => yaw_z,
    };
    let yaw = finite(if yaw_z { xyz_deg_s[2] } else { xyz_deg_s[1] }) * RADIANS_PER_DEGREE;
    let pitch = finite(xyz_deg_s[0]) * RADIANS_PER_DEGREE;
    [
        if axes.invert_yaw { -yaw } else { yaw },
        if axes.invert_pitch { -pitch } else { pitch },
    ]
}

/// A gyro rate this slow on both axes, radians per second (about 3 degrees per second), counts
/// as the pad lying still.
pub const BIAS_STILL_RATE: f32 = 0.05;
/// Consecutive still packets before the bias learns from them.
pub const BIAS_STILL_PACKETS: u32 = 20;
/// How much of the way to each still reading the bias moves.
pub const BIAS_RATE: f32 = 0.02;

/// The gyro's zero-rate bias, learned from the readings of a pad lying still. It counts
/// packets, not time, so the client (on what it sent) and the host (on what it received) learn
/// the same bias from the same readings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GyroBias {
    bias: [f32; 2],
    still: u32,
}

impl GyroBias {
    /// Takes a reading ([`gyro_rate`]) and returns it less the bias.
    pub fn observe(&mut self, rate: [f32; 2]) -> [f32; 2] {
        let raw = [rate[0] - self.bias[0], rate[1] - self.bias[1]];
        if raw[0].abs() < BIAS_STILL_RATE && raw[1].abs() < BIAS_STILL_RATE {
            self.still = self.still.saturating_add(1);
            if self.still >= BIAS_STILL_PACKETS {
                self.bias[0] += (rate[0] - self.bias[0]) * BIAS_RATE;
                self.bias[1] += (rate[1] - self.bias[1]) * BIAS_RATE;
            }
        } else {
            self.still = 0;
        }
        raw
    }

    pub fn bias(&self) -> [f32; 2] {
        self.bias
    }
}

#[cfg(test)]
mod tests;
