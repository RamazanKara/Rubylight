//! Frame Warp on the host: learn the running game's camera response, tag each frame with the
//! input it reflects, and find the HUD (`rubylight_protocol::frame_warp` has the idea and the
//! wire formats).
//!
//! One [`FrameWarpHost`] per stream, behind a lock. The control thread hands it the client's
//! request and every relative mouse move and controller packet as it decodes them. The video
//! thread hands it each capture's luma thumbnail and, now and then, the GPU's HUD bits (both a
//! frame or more late), and asks for the tags of each frame as its number is assigned. The
//! control loop sends what [`FrameWarpHost::take_tags`], [`FrameWarpHost::take_model`] and
//! [`FrameWarpHost::take_mask`] return.

pub mod calibrate;
pub mod history;
pub mod motion;

use std::collections::VecDeque;

pub use calibrate::{Calibrator, Prior};
pub use history::InputHistory;
pub use motion::{Motion, THUMBNAIL_WIDTH, Thumbnail, estimate, estimate_around};
pub use rubylight_protocol::frame_warp::{
    FRAME_WARP_MASK_MESSAGE_TYPE, FRAME_WARP_MODEL_MESSAGE_TYPE, FRAME_WARP_REQUEST_MESSAGE_TYPE,
    FRAME_WARP_TAGS_MESSAGE_TYPE, FrameWarpRequest, MAX_TAGS, MODEL_BYTES, Mask, Model,
    SERVERINFO_FIELD, Tag, Tags, stick_curve,
};

/// The camera counts as turning for this long after a confident shift was measured: the GPU's
/// HUD pass only learns while it does.
pub const MOTION_HOLD_US: u64 = 150_000;
/// Smallest shift, in thumbnail pixels, that counts as the camera turning.
pub const MOTION_MIN: f32 = 1.0;
/// Confidence a shift needs to count as the camera turning.
pub const MOTION_CONFIDENCE: f32 = 0.5;
/// Encoded tag messages kept for the control loop; older ones are dropped (the next carries
/// them again).
const PENDING_TAGS: usize = 8;
/// How often a changed HUD mask may be sent.
const MASK_EVERY_US: u64 = 1_000_000;
/// A mask that differs from the last sent in fewer cells than this is not sent.
const MASK_CHANGE: usize = 16;

/// Picture shifts the camera made, as the statistics show them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    pub enabled: bool,
    pub model: Option<Model>,
    pub samples: u64,
    pub mask_cells: usize,
}

#[derive(Debug)]
pub struct FrameWarpHost {
    enabled: bool,
    mask_max_width: u16,
    history: InputHistory,
    calibrator: Calibrator,
    previous: Option<Thumbnail>,
    picture: (u16, u16),
    thumbnail: (usize, usize),
    recent: VecDeque<Tag>,
    tags: VecDeque<Vec<u8>>,
    model: Option<Model>,
    last_model: Option<Model>,
    mask: MaskTracker,
    /// The HUD the GPU found, per thumbnail pixel, left out of the motion measurement.
    hud: Option<Vec<bool>>,
    moving_until_us: Option<u64>,
    samples: u64,
}

impl FrameWarpHost {
    /// Frame Warp for a stream of `picture` pixels; off until a client asks.
    pub fn new(picture: (u16, u16)) -> Self {
        Self {
            enabled: false,
            mask_max_width: 0,
            history: InputHistory::new(None),
            calibrator: Calibrator::new(picture, (THUMBNAIL_WIDTH, thumbnail_height(picture))),
            previous: None,
            picture,
            thumbnail: (THUMBNAIL_WIDTH, thumbnail_height(picture)),
            recent: VecDeque::new(),
            tags: VecDeque::new(),
            model: None,
            last_model: None,
            mask: MaskTracker::default(),
            hud: None,
            moving_until_us: None,
            samples: 0,
        }
    }

    /// Applies a client's request. Returns whether Frame Warp is on, or `None` for a malformed
    /// request, which changes nothing.
    pub fn on_request(&mut self, payload: &[u8]) -> Option<bool> {
        let request = FrameWarpRequest::decode(payload)?;
        let starting = request.enabled && !self.enabled;
        self.enabled = request.enabled;
        self.mask_max_width = request.mask_max_width;
        if starting {
            // Both sides count input from the request that starts Frame Warp; a repeated
            // request keeps the counts.
            self.history = InputHistory::new(request.controller);
            self.calibrator.clear_samples();
            self.recent.clear();
            self.previous = None;
        } else {
            self.history.set_controller(request.controller);
        }
        if self.enabled {
            // The client counts from its request on; so does the model it hears first.
            self.model = Some(self.calibrator.model(self.calibrator_generation()));
            self.mask.resend();
        } else {
            self.tags.clear();
            self.model = None;
            self.mask.clear_pending();
        }
        Some(self.enabled)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn calibrator_generation(&self) -> u16 {
        self.last_model.map_or(0, |m| m.generation)
    }

    /// The stream's picture size changed (a reconfigure).
    pub fn set_picture(&mut self, picture: (u16, u16)) {
        if picture == self.picture {
            return;
        }
        self.picture = picture;
        self.thumbnail = (THUMBNAIL_WIDTH, thumbnail_height(picture));
        self.calibrator.set_sizes(picture, self.thumbnail);
        self.previous = None;
        self.hud = None;
    }

    /// The thumbnail size the GPU should make for this picture.
    pub fn thumbnail_size(&self) -> (usize, usize) {
        self.thumbnail
    }

    pub fn set_prior(&mut self, prior: Prior) {
        self.calibrator.set_prior(prior);
    }

    /// What to remember for this game.
    pub fn prior(&self) -> Option<Prior> {
        self.calibrator.prior()
    }

    /// A relative mouse move, as the control thread decoded it.
    pub fn on_mouse(&mut self, dx: i16, dy: i16, at_us: u64) {
        if self.enabled {
            self.history.on_mouse(dx, dy, at_us);
        }
    }

    /// A controller packet, as the control thread decoded it.
    pub fn on_controller(&mut self, number: u16, right_stick: (i16, i16), at_us: u64) {
        if self.enabled {
            self.history.on_controller(number, right_stick, at_us);
        }
    }

    /// A capture's luma thumbnail: measures the shift from the previous one and learns.
    pub fn on_thumbnail(&mut self, thumbnail: Thumbnail, now_us: u64) {
        if !self.enabled {
            self.previous = None;
            return;
        }
        if (thumbnail.width, thumbnail.height) != self.thumbnail {
            self.thumbnail = (thumbnail.width, thumbnail.height);
            self.calibrator.set_sizes(self.picture, self.thumbnail);
            self.previous = None;
        }
        if let Some(previous) = self.previous.take()
            && thumbnail.at_us > previous.at_us
            && let Some(motion) = estimate_around(&previous, &thumbnail, self.hud.as_deref())
        {
            self.samples += 1;
            if motion.confidence >= MOTION_CONFIDENCE && motion.dx.hypot(motion.dy) >= MOTION_MIN {
                self.moving_until_us = Some(now_us + MOTION_HOLD_US);
            }
            self.calibrator.add(calibrate::Sample {
                t0_us: previous.at_us,
                t1_us: thumbnail.at_us,
                motion,
            });
        }
        self.previous = Some(thumbnail);
        if let Some(model) = self.calibrator.update(&self.history, now_us) {
            self.last_model = Some(model);
            self.model = Some(model);
        }
    }

    /// Whether the camera is turning: the GPU's HUD pass learns only then.
    pub fn camera_moving(&self, now_us: u64) -> bool {
        self.enabled && self.moving_until_us.is_some_and(|until| now_us < until)
    }

    /// The width of the HUD mask the GPU should make, or 0 for none: the client's limit,
    /// halved from the picture until it fits.
    pub fn mask_width(&self) -> u16 {
        if !self.enabled || self.mask_max_width == 0 {
            return 0;
        }
        let mut width = self.picture.0;
        while width > self.mask_max_width {
            width = width.div_ceil(2);
        }
        width
    }

    /// The GPU's HUD bits: one per mask cell, laid out as [`Mask::words`].
    pub fn on_hud(&mut self, mask: Mask, now_us: u64) {
        if self.enabled && self.mask_max_width > 0 {
            self.hud = Some(hud_pixels(&dilate(&mask), self.thumbnail));
            self.mask.offer(mask, self.mask_max_width, now_us);
        }
    }

    /// Tags frame `frame`, captured at `capture_us`, and queues the tags message (this frame's
    /// tag and up to three before it).
    pub fn tag(&mut self, frame: u32, capture_us: u64) {
        if !self.enabled {
            return;
        }
        let mouse_us = capture_us.saturating_sub(self.calibrator.mouse_lag_us());
        let pad_us = capture_us.saturating_sub(self.calibrator.stick_lag_us());
        let tag = self.history.tag(frame, mouse_us, pad_us);
        if self.recent.back().is_some_and(|last| last.frame == frame) {
            self.recent.pop_back();
        }
        self.recent.push_back(tag);
        while self.recent.len() > MAX_TAGS {
            self.recent.pop_front();
        }
        let message = Tags {
            model_generation: self.calibrator_generation(),
            tags: self.recent.iter().copied().collect(),
        };
        if let Some(wire) = message.encode() {
            if self.tags.len() == PENDING_TAGS {
                self.tags.pop_front();
            }
            self.tags.push_back(wire);
        }
    }

    /// The next tags message to send (unreliable).
    pub fn take_tags(&mut self) -> Option<Vec<u8>> {
        self.tags.pop_front()
    }

    /// The model to send (reliable), when it changed.
    pub fn take_model(&mut self) -> Option<[u8; MODEL_BYTES]> {
        self.model.take().and_then(|m| m.encode())
    }

    /// The HUD mask to send (reliable), when it changed.
    pub fn take_mask(&mut self) -> Option<Vec<u8>> {
        self.mask.pending.take()
    }

    pub fn status(&self) -> Status {
        Status {
            enabled: self.enabled,
            model: self.last_model,
            samples: self.samples,
            mask_cells: self.mask.sent.as_ref().map_or(0, Mask::count),
        }
    }
}

/// The thumbnail height for a picture, [`THUMBNAIL_WIDTH`] wide: its aspect, 32 to 256 rows.
pub fn thumbnail_height(picture: (u16, u16)) -> usize {
    let (w, h) = (usize::from(picture.0.max(1)), usize::from(picture.1.max(1)));
    (THUMBNAIL_WIDTH * h / w).clamp(32, 256)
}

/// Decides which HUD masks go out.
#[derive(Debug, Default)]
struct MaskTracker {
    generation: u16,
    sent: Option<Mask>,
    sent_at_us: Option<u64>,
    pending: Option<Vec<u8>>,
}

impl MaskTracker {
    /// Sends the last mask again (a client that just asked has none).
    fn resend(&mut self) {
        if let Some(mask) = &self.sent {
            self.pending = mask.encode();
        }
    }

    fn clear_pending(&mut self) {
        self.pending = None;
    }

    fn offer(&mut self, mask: Mask, max_width: u16, now_us: u64) {
        let mut mask = dilate(&mask);
        while mask.width > max_width.max(1) {
            mask = mask.halved();
        }
        if let Some(sent) = &self.sent {
            if self
                .sent_at_us
                .is_some_and(|at| now_us.saturating_sub(at) < MASK_EVERY_US)
            {
                return;
            }
            if sent.width == mask.width && sent.height == mask.height {
                let changed: usize = sent
                    .words
                    .iter()
                    .zip(&mask.words)
                    .map(|(a, b)| (a ^ b).count_ones() as usize)
                    .sum();
                if changed < MASK_CHANGE {
                    return;
                }
            }
        } else if mask.is_clear() {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        mask.generation = self.generation;
        let mut wire = mask.encode();
        while wire.is_none() && mask.width > 1 {
            mask = mask.halved();
            wire = mask.encode();
        }
        let Some(wire) = wire else { return };
        self.pending = Some(wire);
        self.sent = Some(mask);
        self.sent_at_us = Some(now_us);
    }
}

/// The thumbnail pixels whose centre falls in a HUD cell of `mask`, row by row.
fn hud_pixels(mask: &Mask, (width, height): (usize, usize)) -> Vec<bool> {
    let (mw, mh) = (usize::from(mask.width), usize::from(mask.height));
    let mut out = vec![false; width * height];
    if mw == 0 || mh == 0 {
        return out;
    }
    for y in 0..height {
        let cy = ((2 * y + 1) * mh / (2 * height)).min(mh - 1);
        for x in 0..width {
            let cx = ((2 * x + 1) * mw / (2 * width)).min(mw - 1);
            out[y * width + x] = mask.get(cx as u16, cy as u16);
        }
    }
    out
}

/// Grows the HUD by one cell all round: its anti-aliased edges change a little with what is
/// behind them, and left out they would shift with the scene.
fn dilate(mask: &Mask) -> Mask {
    let mut out = Mask::new(mask.generation, mask.width, mask.height);
    let stride = Mask::stride(mask.width);
    for y in 0..mask.height {
        for word in 0..stride {
            let bits = mask.words[usize::from(y) * stride + word];
            if bits == 0 {
                continue;
            }
            for bit in 0..32u16 {
                if bits >> bit & 1 == 0 {
                    continue;
                }
                let x = word as u16 * 32 + bit;
                for ny in y.saturating_sub(1)..=(y + 1).min(mask.height - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(mask.width - 1) {
                        out.set(nx, ny, true);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use motion::tests::scene;

    fn enabled(picture: (u16, u16)) -> FrameWarpHost {
        let mut host = FrameWarpHost::new(picture);
        let request = FrameWarpRequest {
            enabled: true,
            controller: Some(0),
            mask_max_width: 480,
        };
        assert_eq!(host.on_request(&request.encode()), Some(true));
        host
    }

    #[test]
    fn nothing_happens_until_a_client_asks() {
        let mut host = FrameWarpHost::new((1920, 1080));
        host.on_mouse(5, 5, 10);
        host.tag(1, 100);
        assert_eq!(host.take_tags(), None);
        assert_eq!(host.mask_width(), 0);
        assert_eq!(host.on_request(&[0, 1]), None);
    }

    #[test]
    fn hud_cells_cover_the_thumbnail_pixels_under_them() {
        let mut mask = Mask::new(0, 8, 4);
        mask.set(7, 0, true);
        mask.set(0, 3, true);
        let hud = hud_pixels(&mask, (16, 8));
        let set: Vec<(usize, usize)> = (0..16 * 8)
            .filter(|&i| hud[i])
            .map(|i| (i % 16, i / 16))
            .collect();
        assert_eq!(
            set,
            [
                (14, 0),
                (15, 0),
                (14, 1),
                (15, 1),
                (0, 6),
                (1, 6),
                (0, 7),
                (1, 7)
            ]
        );
    }

    #[test]
    fn a_start_request_restarts_the_counts_and_a_repeat_keeps_them() {
        let newest = |host: &mut FrameWarpHost| {
            let mut last = None;
            while let Some(wire) = host.take_tags() {
                last = Some(Tags::decode(&wire).unwrap());
            }
            *last.unwrap().tags.last().unwrap()
        };
        let request = |enabled| {
            FrameWarpRequest {
                enabled,
                controller: Some(0),
                mask_max_width: 480,
            }
            .encode()
        };
        let mut host = enabled((1920, 1080));
        host.on_mouse(30, 0, 1_000);
        host.on_controller(0, (0, 0), 1_000);
        host.tag(1, 100_000);
        let tag = newest(&mut host);
        assert_eq!((tag.mouse_x, tag.controller_seq), (30, 1));
        // A repeated start keeps the counts.
        host.on_request(&request(true));
        host.tag(3, 100_000);
        let tag = newest(&mut host);
        assert_eq!((tag.mouse_x, tag.controller_seq), (30, 1));
        // A stop and a new start restart them.
        host.on_request(&request(false));
        host.on_request(&request(true));
        host.tag(4, 200_000);
        let tag = newest(&mut host);
        assert_eq!((tag.mouse_x, tag.controller_seq), (0, 0));
    }

    #[test]
    fn tags_carry_the_newest_frames_and_the_input_before_the_lag() {
        let mut host = enabled((1920, 1080));
        // The model sent on request first.
        let first = Model::decode(&host.take_model().unwrap()).unwrap();
        assert!(!first.locked());
        host.on_mouse(10, 0, 1_000);
        host.on_mouse(10, 0, 100_000);
        for frame in 1..=6 {
            host.tag(frame, 100_000 + u64::from(frame) * 1_000);
        }
        let mut last = None;
        while let Some(wire) = host.take_tags() {
            last = Some(Tags::decode(&wire).unwrap());
        }
        let tags = last.unwrap();
        assert_eq!(
            tags.tags.iter().map(|t| t.frame).collect::<Vec<_>>(),
            [3, 4, 5, 6]
        );
        // With the default 40 ms lag, a capture at 106 ms reflects input up to 66 ms.
        assert_eq!(tags.tags[3].mouse_x, 10);
    }

    #[test]
    fn learns_from_thumbnails_and_flags_the_camera_moving() {
        let mut host = enabled((1920, 1080));
        host.take_model();
        let h = thumbnail_height((1920, 1080));
        // The camera pans 2 thumbnail pixels per 8 ms frame, for 40 ms of mouse input each.
        let mut t = 0u64;
        let mut offset = 0.0;
        let mut sum = 0;
        while t < 3_000_000 {
            host.on_mouse(16, 0, t);
            sum += 16;
            let shown = (sum as f32 - 16.0 * 5.0).max(0.0);
            offset = shown * 0.125;
            host.on_thumbnail(scene(256, h, offset, 0.0, t), t);
            t += 8_000;
        }
        assert!(offset > 100.0);
        assert!(host.camera_moving(t - 8_000));
        assert!(!host.camera_moving(t + MOTION_HOLD_US));
        let model = host.status().model.expect("a model");
        assert!(model.mouse_locked, "{model:?}");
        // The content moves left 0.125 thumbnail px per count: -0.9375 picture px.
        assert!((model.mouse_gain[0] + 0.9375).abs() < 0.08, "{model:?}");
    }

    #[test]
    fn hud_masks_are_dilated_halved_to_fit_and_rate_limited() {
        let mut host = enabled((1920, 1080));
        assert_eq!(host.mask_width(), 480);
        let mut mask = Mask::new(0, 480, 270);
        mask.set(10, 10, true);
        host.on_hud(mask.clone(), 0);
        let sent = Mask::decode(&host.take_mask().unwrap()).unwrap();
        assert_eq!(sent.count(), 9);
        assert_eq!(sent.generation, 1);
        // The same mask again, or a changed one too soon: nothing.
        host.on_hud(mask.clone(), 2_000_000);
        assert_eq!(host.take_mask(), None);
        for x in 200..205 {
            for y in 100..105 {
                mask.set(x, y, true);
            }
        }
        host.on_hud(mask.clone(), 2_100_000);
        assert!(host.take_mask().is_some());
        host.on_hud(Mask::new(0, 480, 270), 2_200_000);
        assert_eq!(host.take_mask(), None);
        // A client asking again gets the last one.
        let again = FrameWarpRequest {
            enabled: true,
            controller: None,
            mask_max_width: 480,
        };
        host.on_request(&again.encode());
        assert!(host.take_mask().is_some());
    }

    #[test]
    fn a_narrow_client_gets_a_halved_mask() {
        let mut host = FrameWarpHost::new((1920, 1080));
        let request = FrameWarpRequest {
            enabled: true,
            controller: None,
            mask_max_width: 300,
        };
        host.on_request(&request.encode());
        assert_eq!(host.mask_width(), 240);
        let mut mask = Mask::new(0, 960, 540);
        mask.set(100, 100, true);
        host.on_hud(mask, 0);
        let sent = Mask::decode(&host.take_mask().unwrap()).unwrap();
        assert_eq!((sent.width, sent.height), (240, 135));
    }

    #[test]
    fn thumbnail_height_keeps_the_aspect() {
        assert_eq!(thumbnail_height((1920, 1080)), 144);
        assert_eq!(thumbnail_height((2560, 1600)), 160);
        assert_eq!(thumbnail_height((3440, 1440)), 107);
        assert_eq!(thumbnail_height((1080, 1920)), 256);
    }
}
