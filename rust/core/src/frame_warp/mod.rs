//! Frame Warp on the host: learn the running game's camera response, tag each frame with the
//! input it reflects, and find the HUD (`rubylight_protocol::frame_warp` has the idea and the
//! wire formats).
//!
//! One [`FrameWarpHost`] per stream, behind a lock. The control thread hands it the client's
//! request and every relative mouse move and controller packet as it decodes them. The video
//! thread asks for the tags of each frame as its number is assigned, and passes each capture's
//! luma thumbnail and, now and then, the GPU's HUD bits (both a frame or more late) to the
//! frame-warp worker. The worker measures them with its [`Measurer`] and fits a [`FitJob`]
//! outside the lock, handing the host only the results. The control loop sends what
//! [`FrameWarpHost::take_tags`], [`FrameWarpHost::take_model`] and
//! [`FrameWarpHost::take_mask`] return.

pub mod calibrate;
pub mod history;
pub mod lens;
pub mod motion;

use std::borrow::Cow;
use std::collections::VecDeque;

pub use calibrate::{Calibrator, Prior};
pub use history::InputHistory;
pub use lens::Lens;
pub use motion::{
    Motion, Scratch, Still, THUMBNAIL_WIDTH, Thumbnail, estimate, estimate_around, estimate_with,
};
pub use rubylight_protocol::frame_warp::{
    FRAME_WARP_MASK_MESSAGE_TYPE, FRAME_WARP_MODEL_MESSAGE_TYPE, FRAME_WARP_REQUEST_MESSAGE_TYPE,
    FRAME_WARP_TAGS_MESSAGE_TYPE, FrameWarpRequest, MAX_TAGS, MODEL_BYTES, Mask, Model,
    SERVERINFO_FIELD, Tag, Tags, stick_curve,
};

/// The camera counts as turning for this long after the scene was last seen moving: the GPU's
/// HUD pass only learns while it does.
pub const MOTION_HOLD_US: u64 = 150_000;
/// How far, in thumbnail pixels, the scene moves between the two captures the HUD pass compares:
/// over less, a textured scene that moved can look as if it stayed.
pub const MOTION_MIN: f32 = 1.0;
/// The confident shifts of this window add up to how far the scene moved: the camera turns when
/// that is at least [`MOTION_MIN`], and the HUD pass compares captures as far apart as the scene
/// took to move that far. A turn too slow to move a thumbnail pixel between captures at a high
/// frame rate still counts, and noise in the shifts mostly cancels.
pub const MOTION_WINDOW_US: u64 = 100_000;
/// Confidence a shift needs to count toward the scene moving.
pub const MOTION_CONFIDENCE: f32 = 0.5;
/// A move teaches the lens when the player's turning arrived within this of the input the move
/// shows: the game's own camera work (a cutscene, a walk) is not a turn.
const LENS_INPUT_MARGIN_US: u64 = 30_000;
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

/// The model last sent and what to remember for the game ([`FrameWarpHost::learned`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Learned {
    pub model: Option<Model>,
    pub prior: Option<Prior>,
}

#[derive(Debug)]
pub struct FrameWarpHost {
    enabled: bool,
    mask_max_width: u16,
    history: InputHistory,
    calibrator: Calibrator,
    /// The game's field of view, from the moves the player's turns made.
    lens: Lens,
    /// What has cleared the worker's previous thumbnail and HUD pixels ([`Measurer`]).
    resets: Resets,
    /// Bumped by whatever changes what a running fit started from ([`FitJob`]).
    fit_epoch: u64,
    picture: (u16, u16),
    /// Where the capture sits in the picture ([`Placement::content`]).
    content: (u16, u16, u16, u16),
    thumbnail: (usize, usize),
    recent: VecDeque<Tag>,
    tags: VecDeque<Vec<u8>>,
    model: Option<Model>,
    last_model: Option<Model>,
    mask: MaskTracker,
    /// The confident shifts of the last [`MOTION_WINDOW_US`]: capture times and motion.
    shifts: VecDeque<(u64, u64, f32, f32)>,
    moving_until_us: Option<u64>,
    /// While the camera turns, how far apart the captures the HUD pass compares are.
    hud_every_us: u64,
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
            lens: Lens::default(),
            resets: Resets::default(),
            fit_epoch: 0,
            picture,
            content: (0, 0, picture.0, picture.1),
            thumbnail: (THUMBNAIL_WIDTH, thumbnail_height(picture)),
            recent: VecDeque::new(),
            tags: VecDeque::new(),
            model: None,
            last_model: None,
            mask: MaskTracker::default(),
            shifts: VecDeque::new(),
            moving_until_us: None,
            hud_every_us: 0,
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
        self.calibrator.set_lens_drawn(request.lens);
        self.fit_epoch += 1;
        if starting {
            // Both sides count input from the request that starts Frame Warp; a repeated
            // request keeps the counts.
            self.history = InputHistory::new(request.controller);
            self.calibrator.clear_samples();
            self.recent.clear();
            self.shifts.clear();
            self.resets.thumbnail += 1;
        } else if self.history.controller() != request.controller {
            // Another pad: its packets count from zero on both sides (the client's tracker
            // is new too). Tags already made name the old pad's packets, and the measured
            // moves read stick input whose history is gone, so both go; the mouse sums stay
            // (the client rebases them).
            self.history.set_controller(request.controller);
            self.calibrator.clear_samples();
            self.recent.clear();
            self.tags.clear();
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

    /// The stream's picture size changed (a reconfigure), or where the capture sits in it
    /// ([`Placement::content`]): a capture of another shape is letterboxed.
    pub fn set_picture(&mut self, picture: (u16, u16), content: (u16, u16, u16, u16)) {
        if (picture, content) == (self.picture, self.content) {
            return;
        }
        self.picture = picture;
        self.content = content;
        self.thumbnail = (THUMBNAIL_WIDTH, thumbnail_height((content.2, content.3)));
        self.calibrator
            .set_sizes(picture, (content.2, content.3), self.thumbnail);
        self.fit_epoch += 1;
        self.resets.thumbnail += 1;
        self.resets.hud += 1;
    }

    /// The thumbnail size the GPU should make for this picture.
    pub fn thumbnail_size(&self) -> (usize, usize) {
        self.thumbnail
    }

    pub fn set_prior(&mut self, prior: Prior) {
        self.calibrator.set_prior(prior);
        self.lens.set_prior(self.calibrator.lens());
        // What this stream's own turns have already said stands.
        self.apply_lens();
        self.fit_epoch += 1;
    }

    /// Puts the learned lens in force.
    fn apply_lens(&mut self) {
        self.calibrator.set_lens(self.lens.tangent());
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

    /// What has cleared the worker's previous thumbnail and HUD pixels: it catches up after
    /// every hold ([`Measurer::catch_up`]).
    pub fn resets(&self) -> Resets {
        self.resets
    }

    /// A capture's luma thumbnail, measured against the previous one outside the lock: learns
    /// from the shift unless something cleared that previous thumbnail since. Returns whether
    /// the worker keeps this one as the next one's previous.
    pub fn on_measured(&mut self, measured: &Measured, now_us: u64) -> bool {
        if !self.enabled {
            return false;
        }
        if measured.size != self.thumbnail {
            self.thumbnail = measured.size;
            self.calibrator.set_sizes(
                self.picture,
                (self.content.2, self.content.3),
                self.thumbnail,
            );
            self.fit_epoch += 1;
            self.resets.thumbnail += 1;
            self.shifts.clear();
        }
        if measured.resets == self.resets.thumbnail
            && let Some((t0_us, motion)) = measured.shift
        {
            self.samples += 1;
            if self.turning(t0_us, measured.at_us)
                && self.lens.observe(self.calibrator.layout(), &motion)
            {
                self.apply_lens();
                self.fit_epoch += 1;
            }
            let turn = self.calibrator.resolve(&motion);
            if turn.confidence >= MOTION_CONFIDENCE {
                self.add_shift((t0_us, measured.at_us, turn.dx, turn.dy), now_us);
            }
            self.calibrator.add(calibrate::Sample {
                t0_us,
                t1_us: measured.at_us,
                motion,
            });
        }
        true
    }

    /// Whether the player's mouse or stick was turning the camera in the input that the
    /// captures from `t0_us` to `t1_us` show.
    fn turning(&self, t0_us: u64, t1_us: u64) -> bool {
        let span = |lag: u64| {
            (
                t0_us.saturating_sub(lag + LENS_INPUT_MARGIN_US),
                t1_us.saturating_sub(lag) + LENS_INPUT_MARGIN_US,
            )
        };
        let (m0, m1) = span(self.calibrator.mouse_lag_us());
        let (s0, s1) = span(self.calibrator.stick_lag_us());
        self.history.mouse_moved(m0, m1) || self.history.stick_active(s0, s1)
    }

    /// The fit to run outside the lock, when one is due: copies of the calibrator and the
    /// input history the fit reads (from the oldest sample less the largest lag on), 5 times a
    /// second.
    pub fn fit_job(&self, now_us: u64) -> Option<FitJob> {
        (self.enabled && self.calibrator.due(now_us)).then(|| FitJob {
            calibrator: self.calibrator.clone(),
            history: self
                .history
                .since(self.calibrator.earliest_input_us().unwrap_or(now_us)),
            now_us,
            epoch: self.fit_epoch,
        })
    }

    /// A fit's result. A request or a new picture size since the job was taken changed what it
    /// started from, so it goes, and the next thumbnail fits again. Otherwise the calibrator is
    /// as the job copied it (only the worker adds samples, and not while it fits).
    pub fn publish_fit(&mut self, fitted: Fitted) {
        if fitted.epoch != self.fit_epoch {
            return;
        }
        self.calibrator = fitted.calibrator;
        if let Some(model) = fitted.model {
            self.last_model = Some(model);
            self.model = Some(model);
        }
    }

    /// A confident shift between captures at `t0` and `t1`: whether, with those of the last
    /// [`MOTION_WINDOW_US`], the scene moved far enough for the camera to count as turning, and
    /// how long it took to move [`MOTION_MIN`] at that speed.
    fn add_shift(&mut self, shift: (u64, u64, f32, f32), now_us: u64) {
        let (_, t1, ..) = shift;
        self.shifts.push_back(shift);
        while self
            .shifts
            .front()
            .is_some_and(|&(t0, ..)| t0.saturating_add(MOTION_WINDOW_US) < t1)
        {
            self.shifts.pop_front();
        }
        let (dx, dy) = self
            .shifts
            .iter()
            .fold((0.0f32, 0.0f32), |(x, y), &(_, _, dx, dy)| (x + dx, y + dy));
        let distance = dx.hypot(dy);
        let span = self
            .shifts
            .front()
            .map_or(0, |&(t0, ..)| t1.saturating_sub(t0));
        if distance >= MOTION_MIN && span > 0 {
            // Any captures missing from the window only make this longer, never too short.
            self.hud_every_us = (f64::from(MOTION_MIN) * span as f64 / f64::from(distance)) as u64;
            self.moving_until_us = Some(now_us + MOTION_HOLD_US);
        }
    }

    /// Whether the camera is turning: the GPU's HUD pass learns only then.
    pub fn camera_moving(&self, now_us: u64) -> bool {
        self.enabled && self.moving_until_us.is_some_and(|until| now_us < until)
    }

    /// While the camera turns, how far apart the captures are that the GPU's HUD pass compares,
    /// so that the scene moves about [`MOTION_MIN`] between them; `None` while it stands still.
    pub fn hud_every_us(&self, now_us: u64) -> Option<u64> {
        self.camera_moving(now_us).then_some(self.hud_every_us)
    }

    /// The width of the HUD grid the GPU should make over the capture, or 0 for none: as wide
    /// as the capture's part of the mask the client gets ([`Placement::mask`]).
    pub fn mask_width(&self) -> u16 {
        let placement = self.placement();
        if placement.mask.0 == 0 {
            return 0;
        }
        let width = u32::from(placement.mask.0) * u32::from(placement.content.2)
            + u32::from(self.picture.0.max(1)) / 2;
        u16::try_from(width / u32::from(self.picture.0.max(1)))
            .unwrap_or(u16::MAX)
            .clamp(1, placement.mask.0)
    }

    /// How the GPU's HUD grid over the capture lies on the picture the client masks.
    pub fn placement(&self) -> Placement {
        // The client's limit, halved from the picture's width until it fits.
        let mut width = if self.enabled && self.mask_max_width > 0 {
            self.picture.0
        } else {
            0
        };
        while width > self.mask_max_width {
            width = width.div_ceil(2);
        }
        let height = (u32::from(width) * u32::from(self.picture.1)
            + u32::from(self.picture.0.max(1)) / 2)
            / u32::from(self.picture.0.max(1));
        Placement {
            picture: self.picture,
            content: self.content,
            mask: (
                width,
                u16::try_from(height)
                    .unwrap_or(u16::MAX)
                    .max(u16::from(width > 0)),
            ),
        }
    }

    /// The GPU's HUD bits (one per mask cell, laid out as [`Mask::words`]), grown by
    /// [`shape`] and laid over the picture by `placement` ([`Placement::place`]). Returns the
    /// thumbnail size the worker lays the grown bits over for the motion measurement
    /// ([`hud_pixels`]), or `None` while no client wants a HUD or once the picture changed.
    pub fn on_hud(
        &mut self,
        placed: &Mask,
        placement: Placement,
        now_us: u64,
    ) -> Option<(usize, usize)> {
        if self.enabled && self.mask_max_width > 0 && placement == self.placement() {
            self.mask.offer(placed, self.mask_max_width, now_us);
            Some(self.thumbnail)
        } else {
            None
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

    /// What the worker logs and saves after each piece of work.
    pub fn learned(&self) -> Learned {
        Learned {
            model: self.last_model,
            prior: self.prior(),
        }
    }
}

/// The thumbnail height for a picture, [`THUMBNAIL_WIDTH`] wide: its aspect, 32 to 256 rows.
pub fn thumbnail_height(picture: (u16, u16)) -> usize {
    let (w, h) = (usize::from(picture.0.max(1)), usize::from(picture.1.max(1)));
    (THUMBNAIL_WIDTH * h / w).clamp(32, 256)
}

/// What has cleared the worker's measurement state ([`Measurer`]), counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resets {
    /// The previous thumbnail: a starting request or a new picture or thumbnail size.
    thumbnail: u64,
    /// The HUD pixels: a new picture size.
    hud: u64,
}

/// A thumbnail measured against the previous one ([`Measurer::measure`]), for
/// [`FrameWarpHost::on_measured`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    size: (usize, usize),
    at_us: u64,
    /// The shift since the previous thumbnail, and when that one was captured.
    shift: Option<(u64, Motion)>,
    /// [`Resets::thumbnail`] when the previous thumbnail was kept.
    resets: u64,
}

/// The frame-warp worker's side of the motion measurement, kept outside the host's lock: the
/// previous thumbnail, the HUD's thumbnail pixels and the estimate's buffers.
#[derive(Debug, Default)]
pub struct Measurer {
    previous: Option<Thumbnail>,
    /// [`Resets::thumbnail`] as of the last hold; `previous` was kept under it.
    previous_resets: u64,
    /// The HUD the GPU found, per thumbnail pixel, left out of the motion measurement.
    hud: Option<Vec<bool>>,
    /// [`Resets::hud`] as of the last hold.
    hud_resets: u64,
    /// What the measured moves leave standing, left out as well: the rims and plain parts of a
    /// HUD that the GPU's mask leaves in.
    still: Still,
    /// `hud` and `still` together, while both mark something.
    both: Option<Vec<bool>>,
    scratch: Scratch,
}

impl Measurer {
    /// The shift from the previous thumbnail to `thumbnail`: the motion estimate, outside the
    /// lock.
    pub fn measure(&mut self, thumbnail: &Thumbnail) -> Measured {
        let shift = self
            .previous
            .as_ref()
            .filter(|previous| thumbnail.at_us > previous.at_us)
            .and_then(|previous| {
                let left_out = self
                    .both
                    .as_deref()
                    .or(self.still.mask())
                    .or(self.hud.as_deref());
                let motion = estimate_with(previous, thumbnail, left_out, &mut self.scratch)?;
                if self.still.observe(previous, thumbnail, &motion) {
                    self.both = both(self.hud.as_deref(), self.still.mask());
                }
                Some((previous.at_us, motion))
            });
        Measured {
            size: (thumbnail.width, thumbnail.height),
            at_us: thumbnail.at_us,
            shift,
            resets: self.previous_resets,
        }
    }

    /// After the host took `thumbnail`'s measurement: keeps it for the next one when the host
    /// said to ([`FrameWarpHost::on_measured`]).
    pub fn keep(&mut self, thumbnail: Thumbnail, keep: bool, resets: Resets) {
        self.catch_up(resets);
        self.previous = keep.then_some(thumbnail);
    }

    /// After the host took a HUD mask ([`FrameWarpHost::on_hud`]): its pixels, as
    /// [`hud_pixels`] laid them over the thumbnail size the host returned.
    pub fn set_hud(&mut self, hud: Vec<bool>, resets: Resets) {
        self.catch_up(resets);
        self.both = both(Some(&hud), self.still.mask());
        self.hud = Some(hud);
    }

    /// Clears what the host cleared since the last hold.
    pub fn catch_up(&mut self, resets: Resets) {
        if resets.thumbnail != self.previous_resets {
            self.previous = None;
            self.previous_resets = resets.thumbnail;
        }
        if resets.hud != self.hud_resets {
            self.hud = None;
            self.both = None;
            self.hud_resets = resets.hud;
        }
    }
}

/// The pixels either mask marks, when both mark some and are of one size.
fn both(hud: Option<&[bool]>, still: Option<&[bool]>) -> Option<Vec<bool>> {
    let (hud, still) = (hud?, still?);
    (hud.len() == still.len()).then(|| hud.iter().zip(still).map(|(a, b)| *a || *b).collect())
}

/// A fit that runs outside the host's lock ([`FrameWarpHost::fit_job`]).
#[derive(Debug)]
pub struct FitJob {
    calibrator: Calibrator,
    history: InputHistory,
    now_us: u64,
    epoch: u64,
}

impl FitJob {
    pub fn run(mut self) -> Fitted {
        let model = self.calibrator.update(&self.history, self.now_us);
        Fitted {
            calibrator: self.calibrator,
            model,
            epoch: self.epoch,
        }
    }
}

/// A finished [`FitJob`], for [`FrameWarpHost::publish_fit`].
#[derive(Debug)]
pub struct Fitted {
    calibrator: Calibrator,
    model: Option<Model>,
    epoch: u64,
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

    /// A HUD mask, already shaped ([`shape`]).
    fn offer(&mut self, shaped: &Mask, max_width: u16, now_us: u64) {
        let mut mask = Cow::Borrowed(shaped);
        while mask.width > max_width.max(1) {
            mask = Cow::Owned(mask.halved());
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
        let mut mask = mask.into_owned();
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

/// Where the GPU's HUD grid, laid over the capture, lies on the stream's picture, which the
/// client masks: a capture of another shape is letterboxed into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub picture: (u16, u16),
    /// Where the capture sits in the picture, `(x, y, width, height)`
    /// ([`crate::display_policy::letterbox`]); the rest is black bars.
    pub content: (u16, u16, u16, u16),
    /// The size of the mask the client gets, or 0 wide for none: its cells cover the picture.
    pub mask: (u16, u16),
}

impl Placement {
    /// `grid`, over the capture, as the client's mask over the picture: a cell is HUD where a
    /// grid cell whose centre falls in it is (as [`Mask::halved`] would make it), or, when none
    /// does, the grid cell under its centre. The bars count as HUD, so they hold still and the
    /// picture is not shifted in from them. A grid over the whole picture at the mask's size
    /// is already that mask.
    pub fn place<'a>(&self, grid: &'a Mask) -> Cow<'a, Mask> {
        let (x0, y0, cw, ch) = self.content;
        let (mw, mh) = self.mask;
        if (x0, y0, cw, ch) == (0, 0, self.picture.0, self.picture.1)
            && (grid.width, grid.height) == (mw, mh)
        {
            return Cow::Borrowed(grid);
        }
        let mut out = Mask::new(grid.generation, mw, mh);
        if grid.width == 0 || grid.height == 0 || cw == 0 || ch == 0 {
            return Cow::Owned(out);
        }
        let across: Vec<_> = (0..mw)
            .map(|x| grid_cells(x, mw, self.picture.0, x0, cw, grid.width))
            .collect();
        let mut row = vec![false; usize::from(grid.width)];
        for y in 0..mh {
            let Some(rows) = grid_cells(y, mh, self.picture.1, y0, ch, grid.height) else {
                (0..mw).for_each(|x| out.set(x, y, true));
                continue;
            };
            for (gx, hud) in (0..grid.width).zip(row.iter_mut()) {
                *hud = rows.clone().any(|gy| grid.get(gx, gy));
            }
            for (x, columns) in (0..mw).zip(&across) {
                let hud = columns.as_ref().is_none_or(|columns| {
                    row[usize::from(columns.start)..usize::from(columns.end)].contains(&true)
                });
                if hud {
                    out.set(x, y, true);
                }
            }
        }
        Cow::Owned(out)
    }
}

/// Along one axis, the grid cells a mask cell takes ([`Placement::place`]): mask cell `cell` of
/// `cells` over `picture` pixels, and `grid` cells over the `length` pixels from `start`. `None`
/// when the cell reaches past them, into a bar.
fn grid_cells(
    cell: u16,
    cells: u16,
    picture: u16,
    start: u16,
    length: u16,
    grid: u16,
) -> Option<std::ops::Range<u16>> {
    let (cell, cells, picture) = (u64::from(cell), u64::from(cells), u64::from(picture));
    let (start, length, grid) = (u64::from(start), u64::from(length), u64::from(grid));
    let (a, b) = (cell * picture / cells, (cell + 1) * picture / cells);
    if a < start || b > start + length {
        return None;
    }
    // Grid cell k's centre is (2k + 1) * length / (2 * grid) pixels from the start: the first
    // whose centre is at `p` or past it.
    let first = |p: u64| {
        (2 * grid * (p - start))
            .saturating_sub(length)
            .div_ceil(2 * length)
            .min(grid)
    };
    let (low, high) = (first(a), first(b));
    let (low, high) = if low < high {
        (low, high)
    } else {
        let at = ((a + b - 2 * start) * grid / (2 * length)).min(grid - 1);
        (at, at + 1)
    };
    Some(low as u16..high as u16)
}

/// The thumbnail pixels whose centre falls in a HUD cell of `mask`, row by row.
pub fn hud_pixels(mask: &Mask, (width, height): (usize, usize)) -> Vec<bool> {
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

/// HUD pieces up to twice this share of the picture's height apart are joined into one region
/// (16 pixels each way at 1080p). A translucent HUD, grey text on a panel or a ring of icons,
/// changes with the scene behind it, so the GPU finds it only where it is nearly opaque, in
/// pieces; left in pieces, it would break up as the scene shifts between them.
const JOIN_HEIGHT: f32 = 0.015;

/// The mask the client gets from the GPU's HUD cells: pieces close together joined
/// ([`JOIN_HEIGHT`]), then grown by one cell all round, since the HUD's anti-aliased edges change
/// a little with what is behind them and left out they would shift with the scene.
pub fn shape(mask: &Mask) -> Mask {
    let width = usize::from(mask.width);
    let mut cells: Vec<bool> = (0..mask.height)
        .flat_map(|y| (0..mask.width).map(move |x| mask.get(x, y)))
        .collect();
    let join = (f32::from(mask.height) * JOIN_HEIGHT).round() as usize;
    if join > 0 && cells.contains(&true) {
        // A closing: grown, then shrunk back, which fills the gaps it closed.
        grow(&mut cells, width, join, true);
        grow(&mut cells, width, join, false);
    }
    grow(&mut cells, width, 1, true);
    let mut out = Mask::new(mask.generation, mask.width, mask.height);
    for (i, _) in cells.iter().enumerate().filter(|(_, set)| **set) {
        out.set((i % width) as u16, (i / width) as u16, true);
    }
    out
}

/// Spreads `value` from every cell that has it to the cells within `reach` (a square), `width`
/// cells to a row: grows the set cells, or with `false` shrinks them. Nothing spreads from past
/// the edges, so a HUD at an edge keeps its size.
fn grow(cells: &mut [bool], width: usize, reach: usize, value: bool) {
    if width == 0 {
        return;
    }
    let mut line = Vec::with_capacity(width);
    for row in cells.chunks_exact_mut(width) {
        line.clear();
        line.extend_from_slice(row);
        spread(&line, reach, value, |x| row[x] = value);
    }
    // Down the columns a row at a time, which reads memory in order: per column, how many cells
    // of the window [y - reach, y + reach] had `value` before this pass.
    let before = cells.to_vec();
    let height = cells.len() / width;
    let mut counts = vec![0usize; width];
    let count = |counts: &mut [usize], y: usize, add: bool| {
        for (count, &cell) in counts.iter_mut().zip(&before[y * width..(y + 1) * width]) {
            if cell == value {
                if add {
                    *count += 1;
                } else {
                    *count -= 1;
                }
            }
        }
    };
    for y in 0..reach.min(height) {
        count(&mut counts, y, true);
    }
    for y in 0..height {
        if y + reach < height {
            count(&mut counts, y + reach, true);
        }
        if y > reach {
            count(&mut counts, y - reach - 1, false);
        }
        for (cell, &count) in cells[y * width..(y + 1) * width].iter_mut().zip(&counts) {
            if count > 0 {
                *cell = value;
            }
        }
    }
}

/// Calls `put` for each cell of `line` without `value` that has a cell with it within `reach`.
fn spread(line: &[bool], reach: usize, value: bool, mut put: impl FnMut(usize)) {
    let n = line.len();
    // How many cells of the window [i - reach, i + reach] have `value`.
    let mut count = line[..reach.min(n)].iter().filter(|&&v| v == value).count();
    for i in 0..n {
        if i + reach < n && line[i + reach] == value {
            count += 1;
        }
        if i > reach && line[i - reach - 1] == value {
            count -= 1;
        }
        if count > 0 && line[i] != value {
            put(i);
        }
    }
}

#[cfg(test)]
mod tests;
