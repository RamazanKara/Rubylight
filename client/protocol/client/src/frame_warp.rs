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
use std::sync::atomic::{AtomicBool, Ordering};
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
/// Stick history kept: two seconds, and at most this many packets.
const STICK_HISTORY: Duration = Duration::from_secs(2);
const STICK_SAMPLES: usize = 4096;
/// A difference past this many mouse counts is a lost count, not input in flight.
const MOUSE_SANITY: i64 = 200_000;

/// The shift for one picture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Warp {
    /// How far to move the picture's content, in picture pixels (x right, y down): sample the
    /// picture at `position - pixels`.
    pub pixels: [f32; 2],
    /// The same as fractions of the picture's width and height: sample at `uv - uv`.
    pub uv: [f32; 2],
    /// Whether to draw with the shift and mask at all. False draws the picture as it is.
    pub active: bool,
    /// The HUD mask to keep unshifted, by generation; `None` for none.
    pub mask_generation: Option<u16>,
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
        })
    }

    /// The request that starts (or stops) this stream's Frame Warp on the host. The host
    /// counts input from the request that starts it, so starting restarts the counts here too;
    /// a repeated start keeps them.
    pub fn request(&self, enabled: bool) -> FrameWarpRequest {
        if self.started.swap(enabled, Ordering::AcqRel) != enabled && enabled {
            *self.predictor() = Predictor::new(self.controller);
            *self.mask.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        FrameWarpRequest {
            enabled,
            controller: self.controller,
            mask_max_width: self.mask_max_width,
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
    }

    /// A controller packet as it goes on the wire.
    pub fn note_controller(&self, number: u16, right_stick: (i16, i16)) {
        let now = self.us(Instant::now());
        self.predictor().note_controller(number, right_stick, now);
    }

    pub fn on_model(&self, model: Model) {
        self.predictor().on_model(model);
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
        self.stick.push_back(StickSample {
            at_us: now_us,
            seq: self.controller_seq,
            stick: right_stick,
        });
        // Keep the newest sample before the window too: it says what was held at its start.
        while self.stick.len() > STICK_SAMPLES
            || self.stick.len() > 1
                && now_us.saturating_sub(self.stick[1].at_us) > STICK_HISTORY.as_micros() as u64
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
            match self.stick.iter().find(|s| s.seq == host_seq) {
                Some(sample) => sample.at_us + u64::from(tag.controller_held_us),
                None => earliest,
            }
        }
        .clamp(earliest, now_us);
        let mut sum = [0.0f32; 2];
        for (i, sample) in self.stick.iter().enumerate() {
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
mod tests {
    use super::*;

    fn model(mouse: bool, stick: bool) -> Model {
        Model {
            generation: 1,
            mouse_locked: mouse,
            stick_locked: stick,
            picture_width: 1000,
            picture_height: 500,
            mouse_gain: [-0.5, -0.25],
            stick_gain: [-1000.0, 500.0],
            stick_deadzone: 0.0,
            stick_exponent: 1.0,
            max_offset: [200.0, 100.0],
            lag_us: 30_000,
            confidence: 0.9,
        }
    }

    fn tags(tags: &[Tag]) -> Tags {
        Tags {
            model_generation: 1,
            tags: tags.to_vec(),
        }
    }

    const SETTLED: u64 = 10_000_000;

    #[test]
    fn mouse_input_the_frame_lacks_shifts_it() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        // The model fades in over 150 ms; settle it first.
        p.warp(0, SETTLED - FADE.as_micros() as u64 - 1);
        p.note_mouse(100, 40, SETTLED);
        p.on_tags(&tags(&[Tag { frame: 5, mouse_x: 60, mouse_y: 40, ..Tag::default() }]), SETTLED);
        let warp = p.warp(5, SETTLED + 1000);
        // 40 counts in x still in flight: -0.5 px per count.
        assert_eq!(warp.pixels, [-20.0, 0.0]);
        assert_eq!(warp.uv, [-0.02, 0.0]);
        assert!(warp.active);
    }

    #[test]
    fn a_peek_shifts_alike_but_counts_nothing() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        p.warp(0, SETTLED - FADE.as_micros() as u64 - 1);
        p.note_mouse(100, 40, SETTLED);
        p.on_tags(&tags(&[Tag { frame: 5, mouse_x: 60, mouse_y: 40, ..Tag::default() }]), SETTLED);
        let before = p.status(SETTLED);
        assert_eq!(p.peek(5, SETTLED + 1000).pixels, [-20.0, 0.0]);
        assert_eq!(p.peek(50, SETTLED + 1000), Warp::default());
        let after = p.status(SETTLED + 1000);
        assert_eq!((after.warped, after.untagged), (before.warped, before.untagged));
        assert_eq!(p.warp(5, SETTLED + 1000).pixels, [-20.0, 0.0]);
        assert_eq!(p.status(SETTLED + 1000).warped, before.warped + 1);
    }

    #[test]
    fn shift_fades_in_over_the_fade_time() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        p.note_mouse(100, 0, 0);
        p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), 0);
        // A rebase does not happen at once: input was just sent.
        let first = p.warp(1, 0);
        assert!(!first.active, "{first:?}");
        let half = p.warp(1, FADE.as_micros() as u64 / 2);
        assert!((half.pixels[0] + 25.0).abs() < 0.5, "{half:?}");
        let full = p.warp(1, FADE.as_micros() as u64 * 2);
        assert_eq!(full.pixels, [-50.0, 0.0]);
    }

    #[test]
    fn idle_input_rebases_lost_counts() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        p.warp(0, 0);
        // The host never counted these 30.
        p.note_mouse(30, 0, 0);
        let idle = REBASE_IDLE.as_micros() as u64;
        p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), idle);
        assert_eq!(p.warp(1, idle + 1).pixels, [0.0, 0.0]);
        // New input counts from the new base.
        p.note_mouse(10, 0, idle + 2);
        p.on_tags(&tags(&[Tag { frame: 2, ..Tag::default() }]), idle + 3);
        assert_eq!(p.warp(2, idle + 4).pixels, [-5.0, 0.0]);
    }

    #[test]
    fn a_missing_tag_is_extrapolated_from_the_two_before() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        p.warp(0, SETTLED - 200_000);
        p.note_mouse(500, 0, SETTLED);
        p.on_tags(
            &tags(&[
                Tag { frame: 10, mouse_x: 100, ..Tag::default() },
                Tag { frame: 11, mouse_x: 200, ..Tag::default() },
            ]),
            SETTLED,
        );
        // Frame 13 would reflect 400 if the rate held, leaving 100 counts.
        let warp = p.warp(13, SETTLED + 1);
        assert_eq!(warp.pixels, [-50.0, 0.0]);
        assert_eq!(p.status(SETTLED + 1).extrapolated, 1);
        // Too far ahead: no tag at all.
        let none = p.warp(30, SETTLED + 2);
        assert!(!none.active);
        assert_eq!(p.status(SETTLED + 2).untagged, 1);
    }

    #[test]
    fn shifts_are_clamped_to_the_model_limit() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        p.warp(0, SETTLED - 200_000);
        p.note_mouse(10_000, -10_000, SETTLED);
        p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), SETTLED);
        assert_eq!(p.warp(1, SETTLED + 1).pixels, [-200.0, 100.0]);
    }

    #[test]
    fn stick_shift_integrates_what_was_held_after_the_tags_packet() {
        let mut p = Predictor::new(Some(0));
        p.on_model(model(false, true));
        p.warp(0, SETTLED - 200_000);
        // Packet 1 centred at t, packet 2 full right 10 ms later, held since.
        p.note_controller(0, (0, 0), SETTLED);
        p.note_controller(0, (32767, 0), SETTLED + 10_000);
        // A packet of another pad is not counted.
        p.note_controller(1, (32767, 32767), SETTLED + 10_500);
        // The frame reflects packet 2 held for 5 ms.
        p.on_tags(
            &tags(&[Tag {
                frame: 1,
                controller_seq: 2,
                controller_held_us: 5_000,
                ..Tag::default()
            }]),
            SETTLED + 20_000,
        );
        // At +40 ms: 25 ms of full right not reflected = 0.025 s * -1000 px/s.
        let warp = p.warp(1, SETTLED + 40_000);
        assert!((warp.pixels[0] + 25.0).abs() < 0.01, "{warp:?}");
        assert!(warp.pixels[1].abs() < 1e-6);
    }

    #[test]
    fn stick_counts_ahead_of_the_client_shift_nothing() {
        let mut p = Predictor::new(Some(0));
        p.on_model(model(false, true));
        p.warp(0, SETTLED - 200_000);
        p.note_controller(0, (32767, 0), SETTLED);
        p.on_tags(
            &tags(&[Tag { frame: 1, controller_seq: 9, controller_held_us: 0, ..Tag::default() }]),
            SETTLED + 1,
        );
        assert_eq!(p.warp(1, SETTLED + 50_000).pixels, [0.0, 0.0]);
    }

    #[test]
    fn unlocking_fades_out_with_the_last_locked_gains() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        p.warp(0, SETTLED - 200_000);
        p.note_mouse(40, 0, SETTLED);
        p.on_tags(&tags(&[Tag { frame: 1, ..Tag::default() }]), SETTLED);
        assert_eq!(p.warp(1, SETTLED + 1).pixels, [-20.0, 0.0]);
        p.on_model(Model { mouse_locked: false, generation: 2, ..model(false, false) });
        let half = p.warp(1, SETTLED + 1 + FADE.as_micros() as u64 / 2);
        assert!((half.pixels[0] + 10.0).abs() < 0.2, "{half:?}");
        assert!(!p.warp(1, SETTLED + 1 + FADE.as_micros() as u64).active);
    }

    #[test]
    fn a_start_request_restarts_the_counts_and_a_repeat_keeps_them() {
        let warp = FrameWarp::new(None, 64);
        warp.note_mouse(50, 0);
        warp.on_model(model(true, false));
        let mut mask = Mask::new(2, 8, 8);
        mask.set(1, 1, true);
        warp.on_mask(mask);
        let request = warp.request(true);
        assert!(request.enabled && request.controller.is_none() && request.mask_max_width == 64);
        assert_eq!(warp.status().model, None);
        assert_eq!(warp.mask_since(None), None);
        warp.on_model(model(true, false));
        warp.request(true);
        assert!(warp.status().model.is_some());
        warp.request(false);
        warp.request(true);
        assert_eq!(warp.status().model, None);
    }

    #[test]
    fn mask_generation_follows_the_mask() {
        let warp = FrameWarp::new(None, 64);
        assert_eq!(warp.mask_since(None), None);
        let mut mask = Mask::new(3, 4, 2);
        mask.set(1, 1, true);
        warp.on_mask(mask.clone());
        assert_eq!(warp.mask_since(None), Some(Some(Arc::new(mask))));
        assert_eq!(warp.mask_since(Some(3)), None);
        warp.on_mask(Mask { generation: 4, ..Mask::default() });
        assert_eq!(warp.mask_since(Some(3)), Some(None));
        assert_eq!(warp.status().mask_cells, 0);
    }

    #[test]
    fn stale_tags_do_not_rebase() {
        let mut p = Predictor::new(None);
        p.on_model(model(true, false));
        p.warp(0, 0);
        p.on_tags(&tags(&[Tag { frame: 9, ..Tag::default() }]), REBASE_IDLE.as_micros() as u64);
        p.note_mouse(50, 0, 2_000_000);
        // An older frame's tag arriving late after an idle spell must not reset the base.
        p.on_tags(&tags(&[Tag { frame: 8, ..Tag::default() }]), 4_000_000);
        p.on_tags(&tags(&[Tag { frame: 10, ..Tag::default() }]), 2_000_001);
        assert_eq!(p.warp(10, 4_000_001).pixels, [-25.0, 0.0]);
    }
}
