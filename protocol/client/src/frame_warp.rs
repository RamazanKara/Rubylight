//! Frame Warp on the client: counts the input that leaves, keeps the host's model, tags and
//! HUD mask, and gives the presenter the shift for each picture
//! (`rubylight_protocol::frame_warp` has the wire formats and the idea).
//!
//! One [`FrameWarp`] per stream, shared by three threads: the control thread counts each
//! relative mouse move and controller packet as it goes on the wire ([`FrameWarp::note_mouse`],
//! [`FrameWarp::note_controller`]) and hands over what the host sends; the presenter asks
//! [`FrameWarp::warp`] for each picture it draws, as late as it can, and uploads the HUD mask
//! when [`FrameWarp::mask_since`] has a newer one.
//!
//! The host's sums and the client's start together at the stream's start, but a packet the
//! host never counted (sent before its control stream was up, or held back while it was
//! silent and then dropped) would leave them apart for good. So whenever the client has sent
//! no mouse motion (or no controller packet) for [`REBASE_IDLE`], everything it sent has
//! reached the picture, and the next tag sets the difference to zero again.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub use rubylight_protocol::frame_warp::{
    FrameWarpRequest, Mask, Model, Tag, Tags, FRAME_WARP_MASK_MESSAGE_TYPE,
    FRAME_WARP_MODEL_MESSAGE_TYPE, FRAME_WARP_REQUEST_MESSAGE_TYPE, FRAME_WARP_TAGS_MESSAGE_TYPE,
    SERVERINFO_FIELD,
};
use rubylight_protocol::frame_warp::{stick_curve, HELD_NONE};

/// Input idle this long has reached the picture: the next tag rebases the sums.
pub const REBASE_IDLE: Duration = Duration::from_millis(600);
/// How long a shift takes to fade in when the model locks, and out when it unlocks.
pub const FADE: Duration = Duration::from_millis(150);
/// The most stick history the shift integrates over: more than the game's lag and the
/// network's round trip ever add up to.
pub const MAX_STICK_WINDOW: Duration = Duration::from_millis(400);
/// Frames back a missing tag may be extrapolated from.
const MAX_EXTRAPOLATION: u32 = 4;
/// Tags kept, by frame number.
const TAG_SLOTS: usize = 64;
/// Stick history kept: the shift's window and a margin for a tag naming a packet sent before it
/// (one the host held long, or a present computed for a moment before the newest packet), plus
/// the newest packet before that; at most this many packets.
const STICK_HISTORY: Duration = Duration::from_millis(MAX_STICK_WINDOW.as_millis() as u64 + 600);
const STICK_SAMPLES: usize = 4096;
/// A difference past this many mouse counts is a lost count, not input in flight.
const MOUSE_SANITY: i64 = 200_000;
/// How far past the picture's edge, in fractions of the picture, the edge fill reaches its
/// darkest ([`edge`]).
pub const EDGE_RAMP: f32 = 0.08;
/// How much darker the edge fill gets at most ([`edge`]).
pub const EDGE_DARKEN: f32 = 0.35;

/// One axis of [`edge`]: where the shifted picture coordinate `v` (0 to 1 across the picture)
/// samples, and how bright that axis alone would draw it.
pub fn edge_axis(v: f32) -> (f32, f32) {
    let fade = 1.0 - EDGE_DARKEN * ((-v).max(v - 1.0) / EDGE_RAMP).clamp(0.0, 1.0);
    (v.abs().min(2.0 - v).clamp(0.0, 1.0), fade)
}

/// The edge fill: where a shifted picture point `p` (fractions of the picture, 0 to 1 inside it)
/// samples the picture and how bright it is drawn. Inside the picture, `p` at full brightness.
/// Past an edge the picture is mirrored at that edge (`-p` below 0, `2 - p` above 1, then held
/// inside), so a turn uncovers a reflection rather than streaks of the edge's pixels, darkened by
/// [`EDGE_DARKEN`] times how far past the edge it is (the farther axis) over [`EDGE_RAMP`], at most
/// that. Every presenter's shader draws the same (`frame_warp.hlsl` on the desktop).
pub fn edge(p: [f32; 2]) -> ([f32; 2], f32) {
    let (x, fade_x) = edge_axis(p[0]);
    let (y, fade_y) = edge_axis(p[1]);
    ([x, y], fade_x.min(fade_y))
}

/// The lens terms of a picture that moves alike everywhere: [`source`] then only shifts.
pub const FLAT: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
/// The least [`source`] divides by: a point this close to level with the turned camera would
/// sample far outside the picture, and past it from behind the camera.
pub const MIN_DEPTH: f32 = 0.25;

/// What a turn that moves the centre of a picture of `size` pixels by `pixels` does to the
/// rest of it, drawn through a lens of `focal` picture pixels (`Model::focal`): how far the
/// centre moves on the screen, in pixels (`focal * tan(pixels / focal)`: the game turns by an
/// angle), and the lens terms [`source`] takes with that shift as fractions of the picture,
/// `[k.x, k.y, n.x, n.y]`. With `t = tan(pixels / focal)` per axis, `k = t * size / focal`
/// and `n = (sqrt(1 + t.y²), sqrt(1 + t.x²))`.
pub fn lens_terms(pixels: [f32; 2], focal: f32, size: (u16, u16)) -> ([f32; 2], [f32; 4]) {
    if !(focal.is_finite() && focal > 0.0) {
        return (pixels, FLAT);
    }
    let size = [f32::from(size.0.max(1)), f32::from(size.1.max(1))];
    // The host keeps the shift well inside a quarter turn; the bound keeps the tangent finite.
    let t = pixels.map(|p| (p / focal).clamp(-1.4, 1.4).tan());
    (
        [t[0] * focal, t[1] * focal],
        [t[0] * size[0] / focal, t[1] * size[1] / focal, (1.0 + t[1] * t[1]).sqrt(), (1.0 + t[0] * t[0]).sqrt()],
    )
}

/// The horizontal field of view of `model`'s lens in degrees, for logs; `None` for a picture that
/// moves alike everywhere.
pub fn field_of_view(model: &Model) -> Option<f32> {
    model.lens().map(|focal| 2.0 * (f32::from(model.picture_width) * 0.5 / focal).atan().to_degrees())
}

/// Where the output point `p` samples the picture (fractions of the picture, 0 to 1 inside it;
/// the edge fill, [`edge`], comes after): through `lens` ([`lens_terms`], or [`FLAT`]) under
/// the centre's shift `shift`, as a camera turning shows it. From the picture's centre,
///
/// ```text
/// source = (q * lens.zw - shift) / max(1 + dot(q, lens.xy), MIN_DEPTH)
/// ```
///
/// which is `p - shift` for a flat picture and exact for a turn about one axis; a turn about both
/// is off by the product of the two tangents (under a pixel at the corners for a shift of 3% of
/// the picture). Every renderer's shader draws the same (`frame_warp.hlsl` on the desktop).
pub fn source(p: [f32; 2], shift: [f32; 2], lens: [f32; 4]) -> [f32; 2] {
    let q = [p[0] - 0.5, p[1] - 0.5];
    let depth = (1.0 + q[0] * lens[0] + q[1] * lens[1]).max(MIN_DEPTH);
    [0.5 + (q[0] * lens[2] - shift[0]) / depth, 0.5 + (q[1] * lens[3] - shift[1]) / depth]
}

/// The output point that samples the picture at `at` ([`source`] undone), or `None` where no
/// point in front of the camera does. For drawing what stays in place as triangles.
pub fn destination(at: [f32; 2], shift: [f32; 2], lens: [f32; 4]) -> Option<[f32; 2]> {
    let s = [at[0] - 0.5, at[1] - 0.5];
    // s * (1 + q.k) = q * n - shift, two linear equations in q.
    let (a, b, c) = (lens[2] - s[0] * lens[0], -s[0] * lens[1], s[0] + shift[0]);
    let (d, e, f) = (-s[1] * lens[0], lens[3] - s[1] * lens[1], s[1] + shift[1]);
    let det = a * e - b * d;
    if det.abs() < 1e-9 {
        return None;
    }
    let q = [(c * e - b * f) / det, (a * f - c * d) / det];
    (1.0 + q[0] * lens[0] + q[1] * lens[1] >= MIN_DEPTH).then_some([q[0] + 0.5, q[1] + 0.5])
}

/// The shift for one picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    /// How far to move the content at the picture's centre, in picture pixels (x right, y down).
    pub pixels: [f32; 2],
    /// The same as fractions of the picture's width and height.
    pub uv: [f32; 2],
    /// How the rest of the picture follows ([`lens_terms`]): the output point `p` samples the
    /// picture at [`source`]`(p, uv, lens)`. [`FLAT`] for a picture that moves alike everywhere,
    /// which samples at `p - uv`.
    pub lens: [f32; 4],
    /// Whether to draw with the shift and mask at all. False draws the picture as it is.
    pub active: bool,
    /// The HUD mask to keep unshifted, by generation; `None` for none.
    pub mask_generation: Option<u16>,
}

impl Default for Warp {
    fn default() -> Self {
        Self { pixels: [0.0; 2], uv: [0.0; 2], lens: FLAT, active: false, mask_generation: None }
    }
}

/// What the statistics overlay shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// The host's model, once one has arrived.
    pub model: Option<Model>,
    /// The latest shift, picture pixels.
    pub pixels: [f32; 2],
    /// The largest shift length in the last second, picture pixels.
    pub peak: f32,
    /// Pictures drawn with a shift, and those whose tag had to be extrapolated or was missing.
    pub warped: u64,
    pub extrapolated: u64,
    pub untagged: u64,
    /// Cells set in the HUD mask.
    pub mask_cells: usize,
}

/// Frame Warp for one stream (see the module documentation).
pub struct FrameWarp {
    epoch: Instant,
    controller: Option<u8>,
    mask_max_width: u16,
    predictor: Mutex<Predictor>,
    mask: Mutex<Option<Arc<Mask>>>,
    /// The last request sent was a start.
    started: AtomicBool,
    /// One more for each relative mouse move and each packet of the tracked pad that went on
    /// the wire (wrapping), read without the predictor's lock ([`FrameWarp::input_seq`]).
    input_seq: AtomicU32,
    /// The tracked pad's right stick as last sent, x in the high half and y in the low.
    stick: AtomicU32,
    /// The stick dead zone of the host's latest model (`f32` bits; 0 before one).
    stick_deadzone: AtomicU32,
}

impl FrameWarp {
    /// Tracks the right stick of input stream controller `controller`, if any, and takes HUD
    /// masks up to `mask_max_width` cells wide (0 for none, as a presenter that cannot draw
    /// them asks).
    pub fn new(controller: Option<u8>, mask_max_width: u16) -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            controller,
            mask_max_width,
            predictor: Mutex::new(Predictor::new(controller)),
            mask: Mutex::new(None),
            started: AtomicBool::new(false),
            input_seq: AtomicU32::new(0),
            stick: AtomicU32::new(0),
            stick_deadzone: AtomicU32::new(0.0f32.to_bits()),
        })
    }

    /// The request that starts (or stops) this stream's Frame Warp on the host. The host
    /// counts input from the request that starts it, so starting restarts the counts here too;
    /// a repeated start keeps them. Every renderer here turns the picture through a lens, so the
    /// request asks for the game's.
    pub fn request(&self, enabled: bool) -> FrameWarpRequest {
        if self.started.swap(enabled, Ordering::AcqRel) != enabled && enabled {
            *self.predictor() = Predictor::new(self.controller);
            *self.mask.lock().unwrap_or_else(|e| e.into_inner()) = None;
            self.stick_deadzone.store(0.0f32.to_bits(), Ordering::Release);
        }
        FrameWarpRequest {
            enabled,
            controller: self.controller,
            mask_max_width: self.mask_max_width,
            lens: true,
        }
    }

    fn us(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch).as_micros() as u64
    }

    fn predictor(&self) -> MutexGuard<'_, Predictor> {
        self.predictor.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A relative mouse move as it goes on the wire.
    pub fn note_mouse(&self, dx: i16, dy: i16) {
        let now = self.us(Instant::now());
        self.predictor().note_mouse(dx, dy, now);
        if dx != 0 || dy != 0 {
            self.input_seq.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// A controller packet as it goes on the wire.
    pub fn note_controller(&self, number: u16, right_stick: (i16, i16)) {
        let now = self.us(Instant::now());
        self.predictor().note_controller(number, right_stick, now);
        if self.controller.map(u16::from) == Some(number) {
            let packed = (u32::from(right_stick.0 as u16) << 16) | u32::from(right_stick.1 as u16);
            self.stick.store(packed, Ordering::Release);
            self.input_seq.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// A count of the input that can move the shift: one more for every relative mouse move
    /// and every packet of the tracked pad sent since the stream started (wrapping). Without
    /// the predictor's lock, for a presenter deciding whether to look at the shift again: it
    /// cannot have moved for input while this stays the same.
    pub fn input_seq(&self) -> u32 {
        self.input_seq.load(Ordering::Acquire)
    }

    /// Whether the tracked pad's right stick, as last sent, is outside the dead zone of the
    /// host's latest model (any deflection before a model arrives): the stick's part of the
    /// shift grows on every refresh while it is. False without a tracked pad. Without the
    /// predictor's lock.
    pub fn stick_deflected(&self) -> bool {
        if self.controller.is_none() {
            return false;
        }
        let packed = self.stick.load(Ordering::Acquire);
        let (x, y) = ((packed >> 16) as u16 as i16, packed as u16 as i16);
        let deadzone = f32::from_bits(self.stick_deadzone.load(Ordering::Acquire));
        stick_curve(x, y, deadzone, 1.0) != [0.0; 2]
    }

    pub fn on_model(&self, model: Model) {
        self.predictor().on_model(model);
        self.stick_deadzone.store(model.stick_deadzone.to_bits(), Ordering::Release);
    }

    pub fn on_tags(&self, tags: &Tags) {
        let now = self.us(Instant::now());
        self.predictor().on_tags(tags, now);
    }

    pub fn on_mask(&self, mask: Mask) {
        let cells = mask.count();
        let generation = mask.generation;
        let mask = (!mask.is_clear()).then(|| Arc::new(mask));
        let present = mask.is_some();
        *self.mask.lock().unwrap_or_else(|e| e.into_inner()) = mask;
        self.predictor().on_mask(present.then_some(generation), cells);
    }

    /// The shift for picture `frame` (the video frame number) drawn now.
    pub fn warp(&self, frame: u32) -> Warp {
        self.warp_at(frame, Instant::now())
    }

    /// The shift for picture `frame` drawn at `at`.
    pub fn warp_at(&self, frame: u32, at: Instant) -> Warp {
        let now = self.us(at);
        self.predictor().warp(frame, now)
    }

    /// The shift picture `frame` would get if drawn now, without counting it in the
    /// statistics: for a presenter deciding whether to draw the same picture again.
    pub fn peek(&self, frame: u32) -> Warp {
        self.peek_at(frame, Instant::now())
    }

    /// [`FrameWarp::peek`] at `at`.
    pub fn peek_at(&self, frame: u32, at: Instant) -> Warp {
        let now = self.us(at);
        self.predictor().peek(frame, now)
    }

    /// The HUD mask when its generation is not `known` (`None` asks for whatever there is).
    /// `Some(None)` means the mask was cleared.
    pub fn mask_since(&self, known: Option<u16>) -> Option<Option<Arc<Mask>>> {
        let mask = self.mask.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let generation = mask.as_ref().map(|m| m.generation);
        (generation != known).then_some(mask)
    }

    pub fn status(&self) -> Status {
        let now = self.us(Instant::now());
        self.predictor().status(now)
    }
}

impl std::fmt::Debug for FrameWarp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameWarp")
            .field("controller", &self.controller)
            .field("mask_max_width", &self.mask_max_width)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug)]
struct StickSample {
    at_us: u64,
    /// The packet's number among those sent for the tracked pad (wrapping, from 1).
    seq: u32,
    stick: (i16, i16),
}

/// The pure state behind [`FrameWarp`], on microseconds from the stream's start.
#[derive(Debug)]
pub struct Predictor {
    model: Option<Model>,
    /// The last locked model, which a fade out keeps shifting by.
    fading: Option<Model>,
    weight: f32,
    weight_at_us: Option<u64>,
    tags: [Option<Tag>; TAG_SLOTS],
    newest: Option<u32>,
    mouse: (i32, i32),
    mouse_offset: (i32, i32),
    last_mouse_us: Option<u64>,
    controller: Option<u8>,
    controller_seq: u32,
    seq_offset: u32,
    last_controller_us: Option<u64>,
    stick: VecDeque<StickSample>,
    mask_generation: Option<u16>,
    mask_cells: usize,
    last: [f32; 2],
    peaks: VecDeque<(u64, f32)>,
    warped: u64,
    extrapolated: u64,
    untagged: u64,
}

/// `a` comes after `b` in wrapping frame order.
fn after(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

impl Predictor {
    pub fn new(controller: Option<u8>) -> Self {
        Self {
            model: None,
            fading: None,
            weight: 0.0,
            weight_at_us: None,
            tags: [None; TAG_SLOTS],
            newest: None,
            mouse: (0, 0),
            mouse_offset: (0, 0),
            last_mouse_us: None,
            controller,
            controller_seq: 0,
            seq_offset: 0,
            last_controller_us: None,
            stick: VecDeque::new(),
            mask_generation: None,
            mask_cells: 0,
            last: [0.0; 2],
            peaks: VecDeque::new(),
            warped: 0,
            extrapolated: 0,
            untagged: 0,
        }
    }

    pub fn note_mouse(&mut self, dx: i16, dy: i16, now_us: u64) {
        if dx == 0 && dy == 0 {
            return;
        }
        self.mouse.0 = self.mouse.0.wrapping_add(i32::from(dx));
        self.mouse.1 = self.mouse.1.wrapping_add(i32::from(dy));
        self.last_mouse_us = Some(now_us);
    }

    pub fn note_controller(&mut self, number: u16, right_stick: (i16, i16), now_us: u64) {
        if self.controller.map(u16::from) != Some(number) {
            return;
        }
        self.controller_seq = self.controller_seq.wrapping_add(1);
        self.last_controller_us = Some(now_us);
        // Sample times never go back, so the shift finds its window from the newest end.
        let at_us = self.stick.back().map_or(now_us, |last| now_us.max(last.at_us));
        self.stick.push_back(StickSample {
            at_us,
            seq: self.controller_seq,
            stick: right_stick,
        });
        // Keep the newest sample before the window too: it says what was held at its start.
        while self.stick.len() > STICK_SAMPLES
            || self.stick.len() > 1
                && at_us.saturating_sub(self.stick[1].at_us) > STICK_HISTORY.as_micros() as u64
        {
            self.stick.pop_front();
        }
    }

    pub fn on_model(&mut self, model: Model) {
        if model.locked() {
            self.fading = Some(model);
        }
        self.model = Some(model);
    }

    pub fn on_mask(&mut self, generation: Option<u16>, cells: usize) {
        self.mask_generation = generation;
        self.mask_cells = cells;
    }

    pub fn on_tags(&mut self, tags: &Tags, now_us: u64) {
        let mut newest: Option<Tag> = None;
        for tag in &tags.tags {
            self.tags[tag.frame as usize % TAG_SLOTS] = Some(*tag);
            if newest.map_or(true, |n| after(tag.frame, n.frame)) {
                newest = Some(*tag);
            }
        }
        let Some(tag) = newest else { return };
        if self.newest.is_some_and(|n| !after(tag.frame, n)) {
            return;
        }
        self.newest = Some(tag.frame);
        let idle = |last: Option<u64>| {
            last.map_or(true, |at| now_us.saturating_sub(at) >= REBASE_IDLE.as_micros() as u64)
        };
        if idle(self.last_mouse_us) {
            self.mouse_offset = (
                self.mouse.0.wrapping_sub(tag.mouse_x),
                self.mouse.1.wrapping_sub(tag.mouse_y),
            );
        }
        if idle(self.last_controller_us) {
            self.seq_offset = self.controller_seq.wrapping_sub(tag.controller_seq);
        }
    }

    /// The tag of `frame`: its own, or extrapolated from the two before it within
    /// [`MAX_EXTRAPOLATION`] frames (mouse sums continue at their last rate; the stick keeps
    /// the last tag's packet). The flag says whether it was extrapolated.
    fn tag(&self, frame: u32) -> Option<(Tag, bool)> {
        let slot = |f: u32| self.tags[f as usize % TAG_SLOTS].filter(|t| t.frame == f);
        if let Some(tag) = slot(frame) {
            return Some((tag, false));
        }
        let previous = |before: u32, within: u32| {
            (1..=within).find_map(|back| slot(before.wrapping_sub(back)))
        };
        let a = previous(frame, MAX_EXTRAPOLATION)?;
        let Some(b) = previous(a.frame, MAX_EXTRAPOLATION) else {
            return Some((Tag { frame, ..a }, true));
        };
        let ahead = frame.wrapping_sub(a.frame) as i64;
        let span = a.frame.wrapping_sub(b.frame) as i64;
        let step = |a: i32, b: i32| {
            let rate = i64::from(a.wrapping_sub(b));
            a.wrapping_add((rate * ahead / span) as i32)
        };
        Some((
            Tag {
                frame,
                mouse_x: step(a.mouse_x, b.mouse_x),
                mouse_y: step(a.mouse_y, b.mouse_y),
                ..a
            },
            true,
        ))
    }

    /// Integral of the game's stick curve over the input the tag does not reflect, in seconds
    /// of full deflection.
    fn stick_unreflected(&self, tag: &Tag, model: &Model, now_us: u64) -> [f32; 2] {
        let window = MAX_STICK_WINDOW.as_micros() as u64;
        let earliest = now_us.saturating_sub(window);
        let host_seq = tag.controller_seq.wrapping_add(self.seq_offset);
        let Some(newest) = self.stick.back() else {
            return [0.0; 2];
        };
        // The host counted a packet the client has not sent: the counts disagree until the
        // next rebase.
        if host_seq != newest.seq && after(host_seq, newest.seq) {
            return [0.0; 2];
        }
        let start = if tag.controller_held_us == HELD_NONE || tag.controller_seq == 0 {
            earliest
        } else {
            match self.stick_sample(host_seq) {
                Some(sample) => sample.at_us + u64::from(tag.controller_held_us),
                None => earliest,
            }
        }
        .clamp(earliest, now_us);
        // What came before the newest sample at or before `start` was no longer held by then:
        // the sum starts there, from the newest end, and adds up in the same order as ever.
        let first = self.stick.iter().rposition(|s| s.at_us <= start).unwrap_or(0);
        let mut sum = [0.0f32; 2];
        for (i, sample) in self.stick.iter().enumerate().skip(first) {
            let end = self.stick.get(i + 1).map_or(now_us, |next| next.at_us).min(now_us);
            let from = sample.at_us.max(start);
            if end <= from {
                continue;
            }
            let held = (end - from) as f32 / 1_000_000.0;
            let curve = stick_curve(
                sample.stick.0,
                sample.stick.1,
                model.stick_deadzone,
                model.stick_exponent,
            );
            sum[0] += curve[0] * held;
            sum[1] += curve[1] * held;
        }
        sum
    }

    /// The sample of the tracked pad's packet `seq`, if still kept. The history holds
    /// consecutive packet numbers, newest last, so it is found by its distance from the newest.
    fn stick_sample(&self, seq: u32) -> Option<&StickSample> {
        let back = self.stick.back()?.seq.wrapping_sub(seq) as usize;
        let index = self.stick.len().checked_sub(back.checked_add(1)?)?;
        self.stick.get(index).filter(|sample| sample.seq == seq)
    }

    fn update_weight(&mut self, locked: bool, now_us: u64) {
        let target = if locked { 1.0 } else { 0.0 };
        let elapsed = self.weight_at_us.map_or(0, |at| now_us.saturating_sub(at));
        self.weight_at_us = Some(now_us);
        let step = elapsed as f32 / FADE.as_micros() as f32;
        self.weight = if self.weight < target {
            (self.weight + step).min(target)
        } else {
            (self.weight - step).max(target)
        };
    }

    /// The shift for a picture being drawn: counted in the statistics.
    pub fn warp(&mut self, frame: u32, now_us: u64) -> Warp {
        self.shift(frame, now_us, true)
    }

    /// The shift a picture would get now, for deciding whether to draw it again: not
    /// counted, so the statistics show only what was drawn.
    pub fn peek(&mut self, frame: u32, now_us: u64) -> Warp {
        self.shift(frame, now_us, false)
    }

    fn shift(&mut self, frame: u32, now_us: u64, drawn: bool) -> Warp {
        let locked = self.model.is_some_and(|m| m.locked());
        self.update_weight(locked, now_us);
        let model = if locked { self.model } else { self.fading };
        let Some(model) = model.filter(|_| self.weight > 0.0) else {
            if drawn {
                self.record([0.0; 2], now_us);
            }
            return Warp::default();
        };
        let Some((tag, extrapolated)) = self.tag(frame) else {
            if drawn {
                self.untagged += 1;
                self.record([0.0; 2], now_us);
            }
            return Warp::default();
        };
        if extrapolated && drawn {
            self.extrapolated += 1;
        }
        let mut shift = [0.0f32; 2];
        if model.mouse_locked {
            let x = i64::from(
                self.mouse
                    .0
                    .wrapping_sub(tag.mouse_x)
                    .wrapping_sub(self.mouse_offset.0),
            );
            let y = i64::from(
                self.mouse
                    .1
                    .wrapping_sub(tag.mouse_y)
                    .wrapping_sub(self.mouse_offset.1),
            );
            if x.abs() < MOUSE_SANITY && y.abs() < MOUSE_SANITY {
                shift[0] += model.mouse_gain[0] * x as f32;
                shift[1] += model.mouse_gain[1] * y as f32;
            }
        }
        if model.stick_locked && self.controller.is_some() {
            let seconds = self.stick_unreflected(&tag, &model, now_us);
            shift[0] += model.stick_gain[0] * seconds[0];
            shift[1] += model.stick_gain[1] * seconds[1];
        }
        for (value, limit) in shift.iter_mut().zip(model.max_offset) {
            *value = value.clamp(-limit, limit) * self.weight;
        }
        // Under a lens the picture turns rather than slides, and its centre moves by the tangent.
        let (shift, lens) = match model.lens() {
            Some(focal) => lens_terms(shift, focal, (model.picture_width, model.picture_height)),
            None => (shift, FLAT),
        };
        if drawn {
            self.warped += 1;
            self.record(shift, now_us);
        }
        let active = shift.iter().any(|s| s.abs() >= 0.01);
        Warp {
            pixels: shift,
            uv: [
                shift[0] / f32::from(model.picture_width),
                shift[1] / f32::from(model.picture_height),
            ],
            lens: if active { lens } else { FLAT },
            active,
            mask_generation: self.mask_generation.filter(|_| active),
        }
    }

    fn record(&mut self, shift: [f32; 2], now_us: u64) {
        self.last = shift;
        let length = (shift[0] * shift[0] + shift[1] * shift[1]).sqrt();
        while self
            .peaks
            .front()
            .is_some_and(|(at, _)| now_us.saturating_sub(*at) > 1_000_000)
        {
            self.peaks.pop_front();
        }
        if length > 0.0 {
            self.peaks.push_back((now_us, length));
            if self.peaks.len() > 2048 {
                self.peaks.pop_front();
            }
        }
    }

    pub fn status(&self, now_us: u64) -> Status {
        let peak = self
            .peaks
            .iter()
            .filter(|(at, _)| now_us.saturating_sub(*at) <= 1_000_000)
            .map(|(_, length)| *length)
            .fold(0.0, f32::max);
        Status {
            model: self.model,
            pixels: self.last,
            peak,
            warped: self.warped,
            extrapolated: self.extrapolated,
            untagged: self.untagged,
            mask_cells: self.mask_cells,
        }
    }
}

#[cfg(test)]
mod tests;
