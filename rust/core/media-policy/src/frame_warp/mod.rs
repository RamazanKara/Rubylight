//! Frame Warp on the host: learn the running game's camera response, tag each frame with the
//! input it reflects, and find the HUD (`rubylight_protocol::frame_warp` has the idea and the
//! wire formats).
//!
//! The host runs Frame Warp 2 only: a client asks with a version 2 request
//! (`rubylight_protocol::frame_warp_v2::RequestV2`), and a version 1 request is ignored. On top of
//! the batch calibrator and lens, fast fits lock the model within a second ([`v2`]), and every
//! frame's tags come with a meta (0x5549): the camera confidence, the gyro and the edge fill.
//!
//! One [`FrameWarpHost`] per stream, behind a lock. The control thread hands it the client's
//! request and every relative mouse move, controller packet and motion packet as it decodes
//! them. The video thread asks for the tags of each frame as its number is assigned, says
//! whether the host's pointer shows, and passes each capture's luma thumbnail and, now and then,
//! the GPU's HUD bits (both a frame or more late) to the frame-warp worker. The worker measures
//! them with its [`Measurer`] and fits a [`FitJob`] outside the lock, handing the host only the
//! results. The control loop sends what [`FrameWarpHost::take_tags`],
//! [`FrameWarpHost::take_meta`], [`FrameWarpHost::take_model`] and [`FrameWarpHost::take_mask`]
//! return.

pub mod calibrate;
pub mod confidence;
pub mod gyro;
pub mod history;
pub mod lens;
pub mod lens_fast;
mod mask;
pub mod motion;
pub mod rls;
pub mod v2;

#[cfg(test)]
use std::borrow::Cow;
use std::collections::VecDeque;

pub use calibrate::{Calibrator, Prior};
use confidence::{Scene, scene_change};
pub use history::InputHistory;
pub use lens::Lens;
use mask::MaskTracker;
pub use mask::{Placement, hud_pixels, shape};
pub use motion::{
    Motion, Scratch, Still, THUMBNAIL_WIDTH, Thumbnail, estimate, estimate_around, estimate_with,
};
pub use rubylight_protocol::frame_warp::{
    FRAME_WARP_MASK_MESSAGE_TYPE, FRAME_WARP_MODEL_MESSAGE_TYPE, FRAME_WARP_REQUEST_MESSAGE_TYPE,
    FRAME_WARP_TAGS_MESSAGE_TYPE, FrameWarpRequest, MAX_TAGS, MODEL_BYTES, Mask, Model, Tag, Tags,
    stick_curve,
};
pub use rubylight_protocol::frame_warp_v2::{
    FRAME_WARP_V2_META_MESSAGE_TYPE, FrameWarpV2Meta, GyroAxes, META_BYTES, REQUEST_BYTES,
    RequestV2, SERVERINFO_FIELD, SERVERINFO_VERSION, YawSource, meta_flags,
};
pub use v2::{PriorV2, V2State};

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
    pub prior: Option<PriorV2>,
    /// The gyro's fast fit is locked: the stream shifts by the gyro even while the model's
    /// mouse and stick fits are not.
    pub gyro_locked: bool,
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
    /// The calibrator's model last published.
    last_model: Option<Model>,
    /// Frame Warp 2's state, while Frame Warp is on ([`v2`]).
    v2: Option<Box<V2State>>,
    /// The gyro's part of the game's saved prior.
    v2_prior: Option<v2::GyroPrior>,
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
            last_model: None,
            v2: None,
            v2_prior: None,
            mask: MaskTracker::default(),
            shifts: VecDeque::new(),
            moving_until_us: None,
            hud_every_us: 0,
            samples: 0,
        }
    }

    /// Applies a client's request. Returns whether Frame Warp is on, or `None` for a request
    /// that is not version 2 (a version 1 client's) or is malformed, which changes nothing.
    pub fn on_request(&mut self, payload: &[u8]) -> Option<bool> {
        let full = RequestV2::decode(payload)?;
        let request = full.base;
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
        // The client counts from its request on; so does the model it hears first.
        self.v2_request(full, starting);
        if self.enabled {
            self.mask.resend(self.mask_max_width);
        } else {
            self.tags.clear();
            self.mask.clear_pending();
        }
        Some(self.enabled)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
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
        self.v2_resized();
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
            self.v2_resized();
        }
        if measured.resets != self.resets.thumbnail {
            return true;
        }
        let motion = measured.shift.as_ref().map(|(_, motion)| motion);
        self.v2_scene(measured.scene, motion, measured.at_us);
        if let Some((t0_us, motion)) = measured.shift {
            self.samples += 1;
            if self.turning(t0_us, measured.at_us) || self.v2_turning(t0_us, measured.at_us) {
                if self.lens.observe(self.calibrator.layout(), &motion) {
                    self.apply_lens();
                    self.fit_epoch += 1;
                }
                self.v2_lens(&motion);
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
            self.v2_turn((t0_us, measured.at_us), turn, now_us);
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
            self.v2_hud(placed);
            Some(self.thumbnail)
        } else {
            None
        }
    }

    /// Tags frame `frame`, captured at `capture_us`, and queues the tags message (this frame's
    /// tag and up to three before it) and the frame's meta. The tags count by the lags of the
    /// model the client was sent last.
    pub fn tag(&mut self, frame: u32, capture_us: u64) {
        if !self.enabled {
            return;
        }
        let (mouse_lag, stick_lag) = self.v2_lags().unwrap_or((
            self.calibrator.mouse_lag_us(),
            self.calibrator.stick_lag_us(),
        ));
        let mouse_us = capture_us.saturating_sub(mouse_lag);
        let pad_us = capture_us.saturating_sub(stick_lag);
        let tag = self.history.tag(frame, mouse_us, pad_us);
        if self.recent.back().is_some_and(|last| last.frame == frame) {
            self.recent.pop_back();
        }
        self.recent.push_back(tag);
        while self.recent.len() > MAX_TAGS {
            self.recent.pop_front();
        }
        let message = Tags {
            model_generation: self.wire_generation(),
            tags: self.recent.iter().copied().collect(),
        };
        if let Some(wire) = message.encode() {
            if self.tags.len() == PENDING_TAGS {
                self.tags.pop_front();
            }
            self.tags.push_back(wire);
        }
        self.v2_tag(frame, capture_us);
    }

    /// The next tags message to send (unreliable).
    pub fn take_tags(&mut self) -> Option<Vec<u8>> {
        self.tags.pop_front()
    }

    /// The model to send (reliable), when it changed ([`v2`] composes it).
    pub fn take_model(&mut self) -> Option<[u8; MODEL_BYTES]> {
        self.v2_take_model()
    }

    /// The HUD mask to send (reliable), when it changed.
    pub fn take_mask(&mut self) -> Option<Vec<u8>> {
        self.mask.pending.take()
    }

    pub fn status(&self) -> Status {
        Status {
            enabled: self.enabled,
            model: self.v2_model().or(self.last_model),
            samples: self.samples,
            mask_cells: self.mask.sent.as_ref().map_or(0, Mask::count),
        }
    }

    /// What the worker logs and saves after each piece of work.
    pub fn learned(&self) -> Learned {
        Learned {
            model: self.v2_model().or(self.last_model),
            prior: self.prior_v2(),
            gyro_locked: self.v2().is_some_and(|v2| v2.locked_fits()[2].is_some()),
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
    /// How alike the previous thumbnail and this one are ([`confidence::scene_change`]).
    scene: Option<Scene>,
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
        let scene = self
            .previous
            .as_ref()
            .filter(|previous| thumbnail.at_us > previous.at_us)
            .and_then(|previous| scene_change(previous, thumbnail));
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
            scene,
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

#[cfg(test)]
mod tests;
