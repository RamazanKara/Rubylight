//! Frame Warp (control messages 0x5543 to 0x5546): the client shifts each picture by the
//! camera turn that its own input has not reached the picture with yet, so a mouse or stick
//! turn shows on the next refresh instead of a round trip and the game's own delay later.
//!
//! The host learns, per game, how far the picture moves per mouse count and per second of
//! right-stick deflection, and how long the game takes to show an input (its lag). It tags
//! each video frame with how much input the frame reflects: the running sums of relative mouse
//! motion it had received, and the count of controller packets it had received for the
//! tracked pad with how long the last one had been held, both taken at the frame's capture
//! time less the game's lag. The client keeps the same sums of what it sent. The difference is
//! the input still in flight or still inside the game, and the model turns it into a shift:
//!
//! ```text
//! shift = mouse_gain * (mouse_sent - tag.mouse) + stick_gain * integral(curve(stick sent), tau..now)
//! ```
//!
//! where `tau` is the client's send time of packet `tag.controller_seq` plus
//! `tag.controller_held_us`. Shifts are in picture pixels and move the content: the client
//! samples the picture at `p - shift`. A HUD mask marks what stays put on screen while the
//! camera turns (crosshair, health, minimap, the weapon); the client does not shift it.
//!
//! A game draws through a perspective camera, so a turn moves its picture's edges farther
//! than its centre. A model with a focal length `f` ([`Model::focal`], picture pixels) says
//! so: its gains are the picture centre's, and the client turns the picture instead of sliding
//! it. With `t = tan(shift / f)` per axis and `q` the output point from the picture's centre
//! in units of `f`, the picture is sampled at
//!
//! ```text
//! (q.x * sqrt(1 + t.y²) - t.x, q.y * sqrt(1 + t.x²) - t.y) / (1 + q.x * t.x + q.y * t.y)
//! ```
//!
//! which is `q - shift / f` at the centre and exact for a turn about one axis.
//!
//! - [`FrameWarpRequest`] (0x5543, client to host) starts or stops it.
//! - [`Model`] (0x5544, host to client, reliable) whenever the learned model changes.
//! - [`Tags`] (0x5545, host to client, unreliable) for each frame as it goes out, with the
//!   previous frames' tags so one lost message costs nothing.
//! - [`Mask`] (0x5546, host to client, reliable) whenever the HUD mask changes.
//!
//! A host lists the ids on its second `a=x-rl-control` line and `FrameWarpVersion` in
//! serverinfo. All integers are little-endian; floats are IEEE 754 binary32.

use alloc::vec::Vec;

/// Client to host: start or stop (a [`FrameWarpRequest`]).
pub const FRAME_WARP_REQUEST_MESSAGE_TYPE: u16 = 0x5543;
/// Host to client: the learned [`Model`].
pub const FRAME_WARP_MODEL_MESSAGE_TYPE: u16 = 0x5544;
/// Host to client: per-frame [`Tags`].
pub const FRAME_WARP_TAGS_MESSAGE_TYPE: u16 = 0x5545;
/// Host to client: the HUD [`Mask`].
pub const FRAME_WARP_MASK_MESSAGE_TYPE: u16 = 0x5546;
/// Serverinfo element: the host learns and sends models, tags and masks.
pub const SERVERINFO_FIELD: &str = "FrameWarpVersion";
/// Format version of every message here.
pub const VERSION: u8 = 1;

pub const REQUEST_BYTES: usize = 8;
pub const TAGS_HEADER_BYTES: usize = 4;
pub const TAG_BYTES: usize = 20;
/// Tags per message: the newest frame's and up to three before it.
pub const MAX_TAGS: usize = 4;
/// A model as a host sends it now: with the focal length.
pub const MODEL_BYTES: usize = 52;
/// A model as first defined, without the focal length: the shortest a client takes.
pub const MODEL_FLAT_BYTES: usize = 48;
pub const MASK_HEADER_BYTES: usize = 8;
/// Largest mask width or height.
pub const MAX_MASK_EDGE: u16 = 2048;
/// Largest mask message; a host halves the mask's resolution until it fits.
pub const MAX_MASK_PAYLOAD: usize = 48 * 1024;
/// Controller numbers a request may track, as the input stream's.
pub const MAX_CONTROLLERS: u8 = 16;
/// [`FrameWarpRequest::controller`] on the wire when no pad is tracked.
pub const NO_CONTROLLER: u8 = 0xFF;
/// [`Tag::controller_held_us`] when the host has received no packet from the tracked pad.
pub const HELD_NONE: u32 = u32::MAX;
/// Stick response exponents a [`Model`] may carry: 1 to 3 in steps of a half.
pub const STICK_EXPONENTS: [f32; 5] = [1.0, 1.5, 2.0, 2.5, 3.0];
/// Largest stick dead zone a [`Model`] may carry, as a fraction of full deflection.
pub const MAX_STICK_DEADZONE: f32 = 0.5;
/// Largest gain magnitude, in picture pixels per count or per second of full deflection.
pub const MAX_GAIN: f32 = 100_000.0;
/// Bounds of a [`Model::focal`] other than 0, in picture pixels.
pub const MIN_FOCAL: f32 = 1.0;
pub const MAX_FOCAL: f32 = 1_000_000.0;

const MASK_NONE: u8 = 0;
const MASK_RUNS: u8 = 1;

/// Client to host: start or stop Frame Warp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameWarpRequest {
    /// Learn, tag and mask; false stops all three.
    pub enabled: bool,
    /// The input stream controller number whose right stick turns the camera (0 to 15).
    pub controller: Option<u8>,
    /// Widest HUD mask the client takes; 0 asks for none.
    pub mask_max_width: u16,
    /// The client draws the picture through the model's lens ([`Model::focal`]). Without it
    /// the host sends no focal length and gains that suit a picture drawn at `position - shift`.
    pub lens: bool,
}

impl FrameWarpRequest {
    pub fn encode(&self) -> [u8; REQUEST_BYTES] {
        let width = self.mask_max_width.min(MAX_MASK_EDGE).to_le_bytes();
        let controller = match self.controller {
            Some(number) if number < MAX_CONTROLLERS => number,
            _ => NO_CONTROLLER,
        };
        [
            VERSION,
            u8::from(self.enabled),
            controller,
            u8::from(self.lens),
            width[0],
            width[1],
            0,
            0,
        ]
    }

    /// Parses a request; version 0, an `enabled` other than 0 or 1, a controller over 15 (but
    /// 0xFF) and a mask width over [`MAX_MASK_EDGE`] reject it. Later versions may append, and
    /// the flags' other bits are theirs.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < REQUEST_BYTES || data[0] == 0 || data[1] > 1 {
            return None;
        }
        let controller = match data[2] {
            NO_CONTROLLER => None,
            number if number < MAX_CONTROLLERS => Some(number),
            _ => return None,
        };
        let mask_max_width = u16::from_le_bytes([data[4], data[5]]);
        (mask_max_width <= MAX_MASK_EDGE).then_some(Self {
            enabled: data[1] == 1,
            controller,
            mask_max_width,
            lens: data[3] & 1 != 0,
        })
    }
}

/// How much input one video frame reflects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tag {
    /// The video frame number, as in the video packets.
    pub frame: u32,
    /// Running sums of the relative mouse motion received (wrapping).
    pub mouse_x: i32,
    pub mouse_y: i32,
    /// Controller packets received for the tracked pad (wrapping).
    pub controller_seq: u32,
    /// How long the last of them had been held at the reflection time, in microseconds;
    /// [`HELD_NONE`] when none had arrived.
    pub controller_held_us: u32,
}

impl Tag {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.frame.to_le_bytes());
        out.extend_from_slice(&self.mouse_x.to_le_bytes());
        out.extend_from_slice(&self.mouse_y.to_le_bytes());
        out.extend_from_slice(&self.controller_seq.to_le_bytes());
        out.extend_from_slice(&self.controller_held_us.to_le_bytes());
    }

    fn read(data: &[u8]) -> Self {
        let word = |at: usize| [data[at], data[at + 1], data[at + 2], data[at + 3]];
        Self {
            frame: u32::from_le_bytes(word(0)),
            mouse_x: i32::from_le_bytes(word(4)),
            mouse_y: i32::from_le_bytes(word(8)),
            controller_seq: u32::from_le_bytes(word(12)),
            controller_held_us: u32::from_le_bytes(word(16)),
        }
    }
}

/// Host to client: the tags of the newest frames, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tags {
    /// The [`Model::generation`] in force when the newest frame was tagged.
    pub model_generation: u16,
    /// 1 to [`MAX_TAGS`] tags, oldest first.
    pub tags: Vec<Tag>,
}

impl Tags {
    /// The wire form, or `None` for no tags or more than [`MAX_TAGS`].
    pub fn encode(&self) -> Option<Vec<u8>> {
        if self.tags.is_empty() || self.tags.len() > MAX_TAGS {
            return None;
        }
        let mut out = Vec::with_capacity(TAGS_HEADER_BYTES + self.tags.len() * TAG_BYTES);
        let generation = self.model_generation.to_le_bytes();
        out.extend_from_slice(&[VERSION, self.tags.len() as u8, generation[0], generation[1]]);
        for tag in &self.tags {
            tag.write(&mut out);
        }
        Some(out)
    }

    /// Parses tags; version 0, a count of 0 or over [`MAX_TAGS`] and a message shorter than
    /// its count reject it. Bytes after the last tag are a later version's and are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < TAGS_HEADER_BYTES || data[0] == 0 {
            return None;
        }
        let count = usize::from(data[1]);
        if count == 0 || count > MAX_TAGS || data.len() < TAGS_HEADER_BYTES + count * TAG_BYTES {
            return None;
        }
        let tags = data[TAGS_HEADER_BYTES..TAGS_HEADER_BYTES + count * TAG_BYTES]
            .chunks_exact(TAG_BYTES)
            .map(Tag::read)
            .collect();
        Some(Self {
            model_generation: u16::from_le_bytes([data[2], data[3]]),
            tags,
        })
    }
}

/// Host to client: what the host has learned about the running game.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    /// Goes up by one with every model the host sends (wrapping).
    pub generation: u16,
    /// The mouse gain fits the game well enough to shift by it.
    pub mouse_locked: bool,
    /// The stick gain and curve fit the game well enough to shift by them.
    pub stick_locked: bool,
    /// The picture size the gains and offsets are in (the stream's video size).
    pub picture_width: u16,
    pub picture_height: u16,
    /// Content shift in picture pixels per relative mouse count, x and y, signed.
    pub mouse_gain: [f32; 2],
    /// Content shift in picture pixels per second of full right-stick deflection after
    /// [`stick_curve`], x and y (the input stream's stick axes, y up), signed.
    pub stick_gain: [f32; 2],
    /// The game's radial stick dead zone as a fraction of full deflection, 0 to 0.5.
    pub stick_deadzone: f32,
    /// The game's stick response exponent, one of [`STICK_EXPONENTS`].
    pub stick_exponent: f32,
    /// Largest shift the client applies, picture pixels, x and y.
    pub max_offset: [f32; 2],
    /// The game's lag the tags allow for, in microseconds (for statistics).
    pub lag_us: u32,
    /// How well the model fits, 0 to 1 (for statistics).
    pub confidence: f32,
    /// The game camera's focal length in picture pixels (half the picture's width over the
    /// tangent of half its horizontal field of view), or 0 when the picture moves alike
    /// everywhere or the host has not learned it. With one, the gains are the picture centre's
    /// and the client turns the picture as a camera does instead of sliding it (the module
    /// documentation has the mapping).
    pub focal: f32,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            generation: 0,
            mouse_locked: false,
            stick_locked: false,
            picture_width: 1,
            picture_height: 1,
            mouse_gain: [0.0; 2],
            stick_gain: [0.0; 2],
            stick_deadzone: 0.0,
            stick_exponent: 1.0,
            max_offset: [0.0; 2],
            lag_us: 0,
            confidence: 0.0,
            focal: 0.0,
        }
    }
}

impl Model {
    pub fn locked(&self) -> bool {
        self.mouse_locked || self.stick_locked
    }

    /// The focal length, when the model has one.
    pub fn lens(&self) -> Option<f32> {
        (self.focal > 0.0).then_some(self.focal)
    }

    fn valid(&self) -> bool {
        let gain = |g: f32| g.is_finite() && g.abs() <= MAX_GAIN;
        self.picture_width > 0
            && self.picture_height > 0
            && self.mouse_gain.iter().chain(&self.stick_gain).all(|g| gain(*g))
            && (0.0..=MAX_STICK_DEADZONE).contains(&self.stick_deadzone)
            && STICK_EXPONENTS.contains(&self.stick_exponent)
            && self
                .max_offset
                .iter()
                .all(|m| m.is_finite() && (0.0..=f32::from(u16::MAX)).contains(m))
            && (0.0..=1.0).contains(&self.confidence)
            && (self.focal == 0.0 || (MIN_FOCAL..=MAX_FOCAL).contains(&self.focal))
    }

    /// The wire form, or `None` for a model out of bounds.
    pub fn encode(&self) -> Option<[u8; MODEL_BYTES]> {
        if !self.valid() {
            return None;
        }
        let mut out = [0u8; MODEL_BYTES];
        out[0] = VERSION;
        out[1] = u8::from(self.mouse_locked) | u8::from(self.stick_locked) << 1;
        out[2..4].copy_from_slice(&self.generation.to_le_bytes());
        out[4..6].copy_from_slice(&self.picture_width.to_le_bytes());
        out[6..8].copy_from_slice(&self.picture_height.to_le_bytes());
        let floats = [
            self.mouse_gain[0],
            self.mouse_gain[1],
            self.stick_gain[0],
            self.stick_gain[1],
            self.stick_deadzone,
            self.stick_exponent,
            self.max_offset[0],
            self.max_offset[1],
        ];
        for (i, value) in floats.iter().enumerate() {
            out[8 + i * 4..12 + i * 4].copy_from_slice(&value.to_le_bytes());
        }
        out[40..44].copy_from_slice(&self.lag_us.to_le_bytes());
        out[44..48].copy_from_slice(&self.confidence.to_le_bytes());
        out[48..52].copy_from_slice(&self.focal.to_le_bytes());
        Some(out)
    }

    /// Parses a model; version 0, unknown flag bits and any value out of bounds reject it. A
    /// model of [`MODEL_FLAT_BYTES`] has no focal length. Bytes after [`MODEL_BYTES`] are a
    /// later version's and are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < MODEL_FLAT_BYTES || data[0] == 0 || data[1] & !0b11 != 0 {
            return None;
        }
        let float = |at: usize| f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let model = Self {
            generation: u16::from_le_bytes([data[2], data[3]]),
            mouse_locked: data[1] & 1 != 0,
            stick_locked: data[1] & 2 != 0,
            picture_width: u16::from_le_bytes([data[4], data[5]]),
            picture_height: u16::from_le_bytes([data[6], data[7]]),
            mouse_gain: [float(8), float(12)],
            stick_gain: [float(16), float(20)],
            stick_deadzone: float(24),
            stick_exponent: float(28),
            max_offset: [float(32), float(36)],
            lag_us: u32::from_le_bytes([data[40], data[41], data[42], data[43]]),
            confidence: float(44),
            focal: if data.len() >= MODEL_BYTES { float(48) } else { 0.0 },
        };
        model.valid().then_some(model)
    }
}

/// The game's stick response that a [`Model`] fits: a radial dead zone, then
/// `((r - deadzone) / (1 - deadzone))^exponent` along the stick's direction. Takes the input
/// stream's axes (-32768 to 32767) and gives each axis from -1 to 1. Host and client both
/// integrate this, so it is defined once here.
pub fn stick_curve(x: i16, y: i16, deadzone: f32, exponent: f32) -> [f32; 2] {
    let vx = (f32::from(x) / 32767.0).clamp(-1.0, 1.0);
    let vy = (f32::from(y) / 32767.0).clamp(-1.0, 1.0);
    let r2 = vx * vx + vy * vy;
    if r2 <= 0.0 {
        return [0.0; 2];
    }
    let r = sqrt(r2);
    let deadzone = deadzone.clamp(0.0, MAX_STICK_DEADZONE);
    if r <= deadzone {
        return [0.0; 2];
    }
    let s = ((r - deadzone) / (1.0 - deadzone)).min(1.0);
    let scaled = power(s, exponent);
    [vx / r * scaled, vy / r * scaled]
}

/// `s^exponent` for `s` in 0..=1 and an exponent of [`STICK_EXPONENTS`] (others round down to
/// the nearest half).
fn power(s: f32, exponent: f32) -> f32 {
    let halves = (exponent.clamp(1.0, 3.0) * 2.0) as u32;
    let mut out = 1.0;
    for _ in 0..halves / 2 {
        out *= s;
    }
    if halves % 2 == 1 {
        out *= sqrt(s);
    }
    out
}

/// Square root without std: a bit-level first guess, then Newton steps to full precision.
fn sqrt(value: f32) -> f32 {
    // NaN, zero and negative values give 0; infinity stays.
    if value.is_nan() || value <= 0.0 || !value.is_finite() {
        return if value == f32::INFINITY { value } else { 0.0 };
    }
    let mut guess = f32::from_bits((value.to_bits() >> 1) + 0x1fbd_1df5);
    for _ in 0..4 {
        guess = 0.5 * (guess + value / guess);
    }
    guess
}

/// Host to client: where the HUD is, as one bit per mask cell over the whole picture, rows
/// top to bottom. A set bit stays put while the camera turns. An empty mask (0 by 0) clears
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Mask {
    /// Goes up by one with every mask the host sends (wrapping).
    pub generation: u16,
    pub width: u16,
    pub height: u16,
    /// `height` rows of [`Mask::stride`] words; bit `i` of word `w` is column `32 * w + i`.
    pub words: Vec<u32>,
}

impl Mask {
    /// A clear mask of `width` by `height` cells.
    pub fn new(generation: u16, width: u16, height: u16) -> Self {
        Self {
            generation,
            width,
            height,
            words: alloc::vec![0; Self::stride(width) * usize::from(height)],
        }
    }

    /// Words per row.
    pub fn stride(width: u16) -> usize {
        usize::from(width).div_ceil(32)
    }

    pub fn get(&self, x: u16, y: u16) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let at = usize::from(y) * Self::stride(self.width) + usize::from(x) / 32;
        self.words.get(at).is_some_and(|w| w >> (x % 32) & 1 != 0)
    }

    pub fn set(&mut self, x: u16, y: u16, on: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let at = usize::from(y) * Self::stride(self.width) + usize::from(x) / 32;
        if let Some(word) = self.words.get_mut(at) {
            if on {
                *word |= 1 << (x % 32);
            } else {
                *word &= !(1 << (x % 32));
            }
        }
    }

    /// No cell is set (or the mask has no cells).
    pub fn is_clear(&self) -> bool {
        self.width == 0 || self.height == 0 || self.words.iter().all(|w| *w == 0)
    }

    /// Cells set.
    pub fn count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// The wire form: the header, then per row the lengths of its runs as LEB128, alternating
    /// clear and set and starting with clear (a row that starts set begins with a 0). `None`
    /// when the mask is out of bounds or its runs pass [`MAX_MASK_PAYLOAD`].
    pub fn encode(&self) -> Option<Vec<u8>> {
        let empty = self.width == 0 || self.height == 0;
        if (self.width == 0) != (self.height == 0)
            || self.width > MAX_MASK_EDGE
            || self.height > MAX_MASK_EDGE
            || self.words.len() != Self::stride(self.width) * usize::from(self.height)
        {
            return None;
        }
        let mut out = Vec::with_capacity(MASK_HEADER_BYTES + usize::from(self.height) * 2);
        let generation = self.generation.to_le_bytes();
        let width = self.width.to_le_bytes();
        let height = self.height.to_le_bytes();
        out.extend_from_slice(&[
            VERSION,
            if empty { MASK_NONE } else { MASK_RUNS },
            generation[0],
            generation[1],
            width[0],
            width[1],
            height[0],
            height[1],
        ]);
        for y in 0..self.height {
            let mut x = 0u16;
            let mut on = false;
            while x < self.width {
                let mut run = 0u16;
                while x + run < self.width && self.get(x + run, y) == on {
                    run += 1;
                }
                write_leb128(&mut out, run);
                x += run;
                on = !on;
            }
            if out.len() > MAX_MASK_PAYLOAD {
                return None;
            }
        }
        Some(out)
    }

    /// Parses a mask. Version 0, an unknown kind, a size over [`MAX_MASK_EDGE`], a kind that
    /// does not match the size, a run past its row's end, a zero run anywhere but a row's
    /// start, a row whose runs do not reach its end and a message over [`MAX_MASK_PAYLOAD`]
    /// reject it.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < MASK_HEADER_BYTES || data.len() > MAX_MASK_PAYLOAD || data[0] == 0 {
            return None;
        }
        let generation = u16::from_le_bytes([data[2], data[3]]);
        let width = u16::from_le_bytes([data[4], data[5]]);
        let height = u16::from_le_bytes([data[6], data[7]]);
        if width > MAX_MASK_EDGE || height > MAX_MASK_EDGE || (width == 0) != (height == 0) {
            return None;
        }
        match data[1] {
            MASK_NONE if width == 0 => {
                return Some(Self {
                    generation,
                    ..Self::default()
                })
            }
            MASK_RUNS if width > 0 => {}
            _ => return None,
        }
        let mut mask = Self::new(generation, width, height);
        let mut at = MASK_HEADER_BYTES;
        for y in 0..height {
            let mut x = 0u16;
            let mut on = false;
            let mut first = true;
            while x < width {
                let run = read_leb128(data, &mut at)?;
                if run == 0 && !first {
                    return None;
                }
                first = false;
                let end = x.checked_add(run).filter(|end| *end <= width)?;
                if on {
                    for cell in x..end {
                        mask.set(cell, y, true);
                    }
                }
                x = end;
                on = !on;
            }
        }
        Some(mask)
    }

    /// The mask at half the resolution in both directions: a cell is set when any of the
    /// four it covers is, so the HUD never grows smaller. For a host whose mask does not fit
    /// [`MAX_MASK_PAYLOAD`].
    pub fn halved(&self) -> Self {
        let width = self.width.div_ceil(2);
        let height = self.height.div_ceil(2);
        let mut out = Self::new(self.generation, width, height);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.get(x, y) {
                    out.set(x / 2, y / 2, true);
                }
            }
        }
        out
    }
}

fn write_leb128(out: &mut Vec<u8>, mut value: u16) {
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

/// A run length: at most three bytes and at most [`MAX_MASK_EDGE`].
fn read_leb128(data: &[u8], at: &mut usize) -> Option<u16> {
    let mut value = 0u32;
    for shift in [0u32, 7, 14] {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return (value <= u32::from(MAX_MASK_EDGE)).then_some(value as u16);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn request_round_trips_and_rejects_bad_fields() {
        let request = FrameWarpRequest {
            enabled: true,
            controller: Some(3),
            mask_max_width: 960,
            lens: false,
        };
        assert_eq!(request.encode(), [1, 1, 3, 0, 0xC0, 0x03, 0, 0]);
        assert_eq!(FrameWarpRequest::decode(&request.encode()), Some(request));
        // A client that draws the lens says so in the flags; their other bits are ignored.
        let lens = FrameWarpRequest {
            lens: true,
            ..request
        };
        assert_eq!(lens.encode(), [1, 1, 3, 1, 0xC0, 0x03, 0, 0]);
        assert_eq!(FrameWarpRequest::decode(&lens.encode()), Some(lens));
        assert_eq!(
            FrameWarpRequest::decode(&[1, 1, 3, 0xFE, 0xC0, 0x03, 0, 0]),
            Some(request)
        );
        let none = FrameWarpRequest {
            enabled: false,
            controller: None,
            mask_max_width: 0,
            lens: false,
        };
        assert_eq!(FrameWarpRequest::decode(&none.encode()), Some(none));
        assert_eq!(FrameWarpRequest::decode(&[0, 1, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(FrameWarpRequest::decode(&[1, 2, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(FrameWarpRequest::decode(&[1, 1, 16, 0, 0, 0, 0, 0]), None);
        assert_eq!(FrameWarpRequest::decode(&[1, 1, 0, 0, 0x01, 0x08, 0, 0]), None);
        assert_eq!(FrameWarpRequest::decode(&[1, 1, 0, 0, 0, 0, 0]), None);
        // A later version may append.
        assert!(FrameWarpRequest::decode(&[2, 1, 0, 0, 0, 0, 0, 0, 9]).is_some());
    }

    #[test]
    fn tags_round_trip_oldest_first() {
        let tags = Tags {
            model_generation: 7,
            tags: vec![
                Tag {
                    frame: 10,
                    mouse_x: -5,
                    mouse_y: i32::MAX,
                    controller_seq: 3,
                    controller_held_us: 1234,
                },
                Tag {
                    frame: 11,
                    mouse_x: 1,
                    mouse_y: i32::MIN,
                    controller_seq: 4,
                    controller_held_us: HELD_NONE,
                },
            ],
        };
        let wire = tags.encode().unwrap();
        assert_eq!(wire.len(), TAGS_HEADER_BYTES + 2 * TAG_BYTES);
        assert_eq!(Tags::decode(&wire), Some(tags.clone()));
        let mut longer = wire.clone();
        longer.push(0);
        assert_eq!(Tags::decode(&longer), Some(tags));
        assert_eq!(Tags::decode(&wire[..wire.len() - 1]), None);
        assert!(Tags { model_generation: 0, tags: vec![] }.encode().is_none());
        assert!(Tags { model_generation: 0, tags: vec![Tag::default(); 5] }.encode().is_none());
        let mut bad = wire;
        bad[1] = 0;
        assert_eq!(Tags::decode(&bad), None);
    }

    fn model() -> Model {
        Model {
            generation: 3,
            mouse_locked: true,
            stick_locked: false,
            picture_width: 1920,
            picture_height: 1080,
            mouse_gain: [-0.75, 0.5],
            stick_gain: [-900.0, 400.0],
            stick_deadzone: 0.15,
            stick_exponent: 1.5,
            max_offset: [230.0, 130.0],
            lag_us: 32_000,
            confidence: 0.93,
            focal: 960.0,
        }
    }

    #[test]
    fn model_round_trips_and_rejects_out_of_bounds() {
        let wire = model().encode().unwrap();
        assert_eq!(wire[1], 1);
        assert_eq!(wire.len(), MODEL_BYTES);
        assert_eq!(Model::decode(&wire), Some(model()));
        assert_eq!(model().lens(), Some(960.0));
        let mut extended = wire.to_vec();
        extended.extend_from_slice(&[1, 2, 3]);
        assert_eq!(Model::decode(&extended), Some(model()));
        // A model as first defined, and one cut inside the focal length, have no lens.
        let flat = Model { focal: 0.0, ..model() };
        assert_eq!(Model::decode(&wire[..MODEL_FLAT_BYTES]), Some(flat));
        assert_eq!(Model::decode(&wire[..MODEL_BYTES - 1]), Some(flat));
        assert_eq!(flat.lens(), None);
        assert_eq!(Model::decode(&wire[..MODEL_FLAT_BYTES - 1]), None);
        for broken in [
            Model { mouse_gain: [f32::NAN, 0.0], ..model() },
            Model { stick_gain: [0.0, f32::INFINITY], ..model() },
            Model { stick_deadzone: 0.6, ..model() },
            Model { stick_exponent: 1.2, ..model() },
            Model { max_offset: [-1.0, 0.0], ..model() },
            Model { confidence: 1.5, ..model() },
            Model { picture_width: 0, ..model() },
            Model { focal: -1.0, ..model() },
            Model { focal: 0.5, ..model() },
            Model { focal: f32::NAN, ..model() },
            Model { focal: 2_000_000.0, ..model() },
        ] {
            assert!(broken.encode().is_none(), "{broken:?}");
        }
        let mut flags = wire;
        flags[1] = 4;
        assert_eq!(Model::decode(&flags), None);
    }

    #[test]
    fn stick_curve_has_a_radial_dead_zone_and_exponent() {
        assert_eq!(stick_curve(0, 0, 0.1, 1.0), [0.0, 0.0]);
        assert_eq!(stick_curve(3000, 0, 0.1, 1.0), [0.0, 0.0]);
        let full = stick_curve(32767, 0, 0.1, 2.0);
        assert!((full[0] - 1.0).abs() < 1e-5 && full[1] == 0.0);
        let left = stick_curve(-32768, 0, 0.0, 1.0);
        assert!((left[0] + 1.0).abs() < 1e-5);
        // Half way out with no dead zone and exponent 1.5: 0.5^1.5.
        let half = stick_curve(16384, 0, 0.0, 1.5);
        assert!((half[0] - 0.353_6).abs() < 1e-3, "{half:?}");
        // Diagonal keeps its direction.
        let diagonal = stick_curve(20000, 20000, 0.0, 1.0);
        assert!((diagonal[0] - diagonal[1]).abs() < 1e-6);
        assert!(diagonal[0] > 0.6 && diagonal[0] <= 0.7072);
        // Corners saturate at length 1.
        let corner = stick_curve(32767, 32767, 0.0, 1.0);
        let length = sqrt(corner[0] * corner[0] + corner[1] * corner[1]);
        assert!((length - 1.0).abs() < 1e-5);
    }

    #[test]
    fn sqrt_is_exact_enough() {
        for value in [1e-6f32, 0.01, 0.25, 0.5, 1.0, 2.0, 1234.5] {
            let root = sqrt(value);
            assert!((root * root - value).abs() <= value * 1e-6, "{value}");
        }
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(-1.0), 0.0);
    }

    #[test]
    fn mask_round_trips_through_runs() {
        let mut mask = Mask::new(9, 70, 3);
        for x in 0..5 {
            mask.set(x, 0, true);
        }
        mask.set(69, 1, true);
        for x in 30..40 {
            mask.set(x, 2, true);
        }
        let wire = mask.encode().unwrap();
        // Row 0 starts set: a zero clear run, then 5 set, then 65 clear.
        assert_eq!(&wire[MASK_HEADER_BYTES..MASK_HEADER_BYTES + 3], &[0, 5, 65]);
        assert_eq!(Mask::decode(&wire), Some(mask.clone()));
        assert_eq!(mask.count(), 16);
        let empty = Mask { generation: 4, ..Mask::default() };
        let wire = empty.encode().unwrap();
        assert_eq!(wire.len(), MASK_HEADER_BYTES);
        assert_eq!(Mask::decode(&wire), Some(empty));
    }

    #[test]
    fn mask_decode_refuses_broken_runs() {
        let header = |kind: u8, w: u16, h: u16| {
            let mut out = vec![1, kind, 0, 0];
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out
        };
        // Runs past the row's end.
        let mut long = header(1, 4, 1);
        long.extend_from_slice(&[2, 3]);
        assert_eq!(Mask::decode(&long), None);
        // A zero run after the first.
        let mut zero = header(1, 4, 1);
        zero.extend_from_slice(&[2, 0, 2]);
        assert_eq!(Mask::decode(&zero), None);
        // A row that stops short.
        let mut short = header(1, 4, 1);
        short.push(2);
        assert_eq!(Mask::decode(&short), None);
        // Kinds that do not match the size.
        assert_eq!(Mask::decode(&header(0, 4, 1)), None);
        assert_eq!(Mask::decode(&header(1, 0, 0)), None);
        assert_eq!(Mask::decode(&header(1, 0, 4)), None);
        assert_eq!(Mask::decode(&header(1, MAX_MASK_EDGE + 1, 1)), None);
        // A run over the largest edge.
        let mut huge = header(1, 4, 1);
        huge.extend_from_slice(&[0xFF, 0xFF, 0x7F]);
        assert_eq!(Mask::decode(&huge), None);
    }

    #[test]
    fn halved_mask_keeps_every_set_cell() {
        let mut mask = Mask::new(1, 5, 3);
        mask.set(4, 2, true);
        mask.set(1, 0, true);
        let half = mask.halved();
        assert_eq!((half.width, half.height), (3, 2));
        assert!(half.get(2, 1) && half.get(0, 0));
        assert_eq!(half.count(), 2);
    }

    #[test]
    fn a_full_size_checkerboard_does_not_fit_and_its_halves_do() {
        let mut mask = Mask::new(1, 1920, 1080);
        for y in 0..1080 {
            for x in (y % 2..1920).step_by(2) {
                mask.set(x, y, true);
            }
        }
        assert!(mask.encode().is_none());
        let mut smaller = mask.halved();
        while smaller.encode().is_none() {
            smaller = smaller.halved();
        }
        assert!(smaller.width <= 960);
    }
}
