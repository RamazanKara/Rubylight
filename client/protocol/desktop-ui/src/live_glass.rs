//! Live glass on the desktop: the content under the chrome is painted into a render target, run
//! through a linear down and up chain, and sampled by the chrome's meshes in screen space.
//!
//! The chain runs only when a `Boundary` is in the frame, so a playing stream (which has none) pays
//! nothing, and only when what lies under the chrome changed. The tiers, their landing and the probe
//! of the chain's runs are `nightjar_policy::glass_probe`'s, shared with Android: lens (the live
//! chain plus the edge band of `ui/glass_lens.rs`), live, baked and, in high contrast, solid. A
//! chain that does not fit the frame budget steps down a tier, and targets that cannot be made step
//! down to baked; both say so in the log.

use std::any::Any;
use std::hash::Hasher;
use std::time::Instant;

use egui::epaint::{ClippedPrimitive, Mesh, Primitive, TextureId};
use egui::{Context, Id, Rect};

use nightjar_policy::glass_probe::{self as probe, Device, Guard, Probe, Verdict};

/// How glass is drawn on this machine: lens, live, baked or solid (`glass_probe::Tier`).
pub use nightjar_policy::glass_probe::Tier;

mod memory;
pub use memory::{Memory, Stored};

/// The texture chrome samples on the live tier; [`Chain`] (or the screenshot canvas) provides it.
pub const LIVE_BACKDROP: TextureId = TextureId::User(0x676C_6173);

/// What the callback at the content/chrome boundary carries: where the chrome lies, in points.
pub struct Boundary {
    pub regions: Vec<Rect>,
}

fn tier_id() -> Id {
    Id::unique("nightjar-glass-tier")
}

/// Sets the machine's tier. High contrast overrides it to solid, in [`tier`].
pub fn set_tier(ctx: &Context, tier: Tier) {
    ctx.data_mut(|d| d.insert_temp(tier_id(), tier));
}

/// The tier glass draws with this frame.
pub fn tier(ctx: &Context) -> Tier {
    if crate::theme::high_contrast() {
        return Tier::Solid;
    }
    ctx.data(|d| d.get_temp(tier_id())).unwrap_or(Tier::Baked)
}

/// The boundary in a frame's primitives, if it has one: its index and its payload.
pub fn find_boundary(primitives: &[ClippedPrimitive]) -> Option<(usize, &Boundary)> {
    primitives
        .iter()
        .enumerate()
        .find_map(|(at, p)| match &p.primitive {
            Primitive::Callback(callback) => (callback.callback.as_ref() as &dyn Any)
                .downcast_ref::<Boundary>()
                .map(|b| (at, b)),
            Primitive::Mesh(_) => None,
        })
}

/// How far from chrome a change still reaches it through the blur, in points: the chain's coarsest
/// level is 16 pixels to the texel, and its bilinear passes spread one texel each way.
const BLUR_REACH: f32 = 40.0;

/// Non-cryptographic FxHash-style hasher for per-frame vertex fingerprints (avoids SipHash
/// per-frame hashing overhead).
struct FastHasher(u64);

impl FastHasher {
    const K: u64 = 0x517cc1b727220a95;

    fn new() -> Self {
        Self(0)
    }

    #[inline]
    fn mix(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(Self::K);
    }
}

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (chunks, tail) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.mix(u64::from_le_bytes(*chunk));
        }
        if !tail.is_empty() {
            let mut last = [0u8; 8];
            last[..tail.len()].copy_from_slice(tail);
            self.mix(u64::from_le_bytes(last));
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.mix(u64::from(i));
    }

    fn write_i32(&mut self, i: i32) {
        self.mix(u64::from(i as u32));
    }

    fn write_u64(&mut self, i: u64) {
        self.mix(i);
    }
}

/// A hash of everything painted under `regions` or within the blur's reach of them: triangle by
/// triangle, so a hover elsewhere in the same batched mesh leaves it alone. Equal hashes mean the
/// chain's last output still holds.
pub fn fingerprint(primitives: &[ClippedPrimitive], regions: &[Rect]) -> u64 {
    let mut hash = FastHasher::new();
    for clipped in primitives {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        let clip = clipped.clip_rect;
        let near: Vec<Rect> = regions
            .iter()
            .map(|r| r.expand(BLUR_REACH).intersect(clip))
            .filter(|r| r.is_positive())
            .collect();
        if near.is_empty() {
            continue;
        }
        let mut clip_written = false;
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [tri[0], tri[1], tri[2]].map(|i| &mesh.vertices[i as usize]);
            let bounds = Rect::from_min_max(
                egui::pos2(
                    v[0].pos.x.min(v[1].pos.x).min(v[2].pos.x),
                    v[0].pos.y.min(v[1].pos.y).min(v[2].pos.y),
                ),
                egui::pos2(
                    v[0].pos.x.max(v[1].pos.x).max(v[2].pos.x),
                    v[0].pos.y.max(v[1].pos.y).max(v[2].pos.y),
                ),
            );
            if near.iter().any(|r| r.intersects(bounds)) {
                if !std::mem::replace(&mut clip_written, true) {
                    for edge in [clip.min, clip.max] {
                        hash.write_u32(edge.x.to_bits());
                        hash.write_u32(edge.y.to_bits());
                    }
                }
                for vertex in v {
                    hash.write_u32(vertex.pos.x.to_bits());
                    hash.write_u32(vertex.pos.y.to_bits());
                    hash.write_u32(vertex.uv.x.to_bits());
                    hash.write_u32(vertex.uv.y.to_bits());
                    hash.write(&vertex.color.to_array());
                }
                match mesh.texture_id {
                    TextureId::Managed(id) | TextureId::User(id) => hash.write_u64(id),
                }
            }
        }
    }
    hash.finish()
}

/// A hash of a whole frame at a scale and size: equal prints look the same on screen, as long as no
/// texture changed under them.
pub fn frame_print(primitives: &[ClippedPrimitive], ppp: f32, size: (i32, i32)) -> u64 {
    let mut hash = FastHasher::new();
    hash.write_u32(ppp.to_bits());
    hash.write_i32(size.0);
    hash.write_i32(size.1);
    for clipped in primitives {
        for edge in [clipped.clip_rect.min, clipped.clip_rect.max] {
            hash.write_u32(edge.x.to_bits());
            hash.write_u32(edge.y.to_bits());
        }
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        match mesh.texture_id {
            TextureId::Managed(id) | TextureId::User(id) => hash.write_u64(id),
        }
        for index in &mesh.indices {
            hash.write_u32(*index);
        }
        for vertex in &mesh.vertices {
            hash.write_u32(vertex.pos.x.to_bits());
            hash.write_u32(vertex.pos.y.to_bits());
            hash.write_u32(vertex.uv.x.to_bits());
            hash.write_u32(vertex.uv.y.to_bits());
            hash.write(&vertex.color.to_array());
        }
    }
    hash.finish()
}

/// Whether a launcher frame has to reach the window: what it shows is not what is on screen, a
/// texture changed, or the window itself did.
pub fn frame_differs(on_screen: Option<u64>, print: u64, must_draw: bool, textures: bool) -> bool {
    must_draw || textures || on_screen != Some(print)
}

/// Where a frame is painted: SDL in `gui.rs`, the CPU canvas in `shot.rs`. [`Glass`] decides what
/// goes on it, so the app and the screenshots walk a frame alike.
pub trait Stage {
    /// The output's size in pixels.
    fn size(&self) -> (i32, i32);
    /// Draws one mesh on the current target, sampling the chain's output for [`LIVE_BACKDROP`].
    fn mesh(&mut self, clip: Rect, mesh: &Mesh);
    /// Whether render targets work at all, tried once before anything relies on them.
    fn targets_work(&mut self) -> Result<(), String>;
    /// Points the coming meshes at the scene.
    fn begin_scene(&mut self) -> Result<(), String>;
    /// Blurs the scene into the chain's output and puts the scene on the screen. With `timed`,
    /// returns the blur's own time in ms.
    fn end_scene(&mut self, timed: bool) -> Result<Option<f32>, String>;
    /// Frees the chain's targets.
    fn release(&mut self);
    /// The renderer, its GPU and the GPU's driver as the crash guard's fingerprint names them,
    /// beside the app's build: a GPU or driver update gets a fresh probe.
    fn driver(&self) -> String {
        String::new()
    }
}

/// `NIGHTJAR_GLASS=baked|live|lens`: holds the tier for an A/B run. `live` and `lens` keep a chain
/// over budget, and nothing is remembered.
pub fn forced_by(value: Option<&str>) -> Option<Tier> {
    match value? {
        "baked" => Some(Tier::Baked),
        "live" => Some(Tier::Live),
        "lens" => Some(Tier::Lens),
        other => {
            eprintln!("nightjar: NIGHTJAR_GLASS is {other:?}, not baked, live or lens: ignored");
            None
        }
    }
}

/// The crash guard's fingerprint: this build on this OS, and the stage's renderer and driver.
fn fingerprint_of<S: Stage>(stage: &S) -> String {
    format!("{} {} {}", crate::VERSION, std::env::consts::OS, stage.driver()).trim_end().to_owned()
}

/// The tier's landing and when the chain runs. No SDL in here: a frame without a boundary reaches
/// the stage as its meshes alone, and a frame whose print did not change runs no chain.
///
/// Landing and the probe are `nightjar_policy::glass_probe`, as on Android: the first launcher
/// frame tries the render targets and lands on the best tier (lens, the live chain with its edge
/// band), unless a probe stored for this build and driver stepped it down. Once the screen has
/// settled, the chain's runs are timed against the display's frame (never shorter than a 120 Hz
/// one); a tier that does not hold it steps down once (lens to live, live to baked) and that is
/// stored. The crash guard is armed before the first probed frame and cleared when the probe ends.
pub struct Glass {
    forced: Option<Tier>,
    /// `None` until the first launcher frame tried the render targets: baked meanwhile.
    landing: Option<Tier>,
    over_video: bool,
    refresh_hz: f32,
    probe: Option<Probe>,
    /// The tier whose probe decided against this budget: its runs are not timed any more.
    decided: Option<Tier>,
    /// When the landed screen opened, for the probe's settle time.
    opened: Option<Instant>,
    /// Added to the time since `opened`: the screenshot canvas and tests start settled.
    head_start_ms: u64,
    memory: Memory,
    /// The guard in `memory` is armed by this glass.
    armed: Option<String>,
    /// What the chain's output shows; `None` when it must run.
    print: Option<(u64, (i32, i32))>,
    /// Whether the stage holds targets.
    held: bool,
    /// The tier changed after this frame's pass was laid out: the next pass must follow soon.
    changed: bool,
}

impl Glass {
    pub fn new(forced: Option<Tier>) -> Self {
        Self {
            forced,
            landing: (forced == Some(Tier::Baked)).then_some(Tier::Baked),
            over_video: false,
            refresh_hz: 60.0,
            probe: None,
            decided: None,
            opened: None,
            head_start_ms: 0,
            memory: Memory::default(),
            armed: None,
            print: None,
            held: false,
            changed: false,
        }
    }

    /// Remembers the probe's result and its crash guard in `memory` (the app's settings folder).
    pub fn remembering(mut self, memory: Memory) -> Self {
        self.memory = memory;
        self
    }

    /// What is remembered, for tests.
    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    /// A glass already landed on `tier` and settled, for the screenshot canvas and tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn landed(tier: Tier) -> Self {
        let mut glass = Self::new(None);
        glass.landing = Some(tier);
        glass.opened = Some(Instant::now());
        glass.head_start_ms = probe::SETTLE_MS;
        glass
    }

    /// A glass already live, for the screenshot canvas.
    #[cfg(any(test, feature = "test-support"))]
    pub fn live() -> Self {
        Self::landed(Tier::Live)
    }

    /// One display refresh, what the probe measures the chain against. Another frame budget
    /// (`glass_probe::budget_ns`) judges the tier again.
    pub fn set_refresh(&mut self, hz: f32) {
        if hz <= 0.0 {
            return;
        }
        if probe::budget_ns(hz) != probe::budget_ns(self.refresh_hz) {
            self.probe = None;
            self.decided = None;
        }
        self.refresh_hz = hz;
    }

    /// The tier this pass lays out with: never lens or live over a stream's video, which is not
    /// repainted into a scene (the chain would blur an empty target and clear the picture).
    pub fn pass_tier(&self) -> Tier {
        match self.landing {
            Some(tier) if tier.probed() && !self.over_video => tier,
            _ => Tier::Baked,
        }
    }

    /// Whether the frames from here on draw over a stream's video. True when the caller must free
    /// the chain's targets: a stream holds none, whatever it draws or does not draw.
    #[must_use]
    pub fn set_over_video(&mut self, over_video: bool) -> bool {
        self.over_video = over_video;
        if over_video {
            // No chain runs over a stream, so whatever ends the app from here is not the glass.
            self.disarm();
        }
        if over_video && self.held {
            self.held = false;
            self.print = None;
            return true;
        }
        false
    }

    /// Forgets the output: the targets' contents are gone, or the targets themselves.
    pub fn invalidate(&mut self) {
        self.print = None;
    }

    /// A new renderer (Linux's switch to the GPU renderer): the targets went with the old one, and
    /// the tier is tried and measured again on this one, as at start.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn renderer_changed(&mut self) {
        // The old renderer went (a device reset, the switch): no crash, so the guard is cleared on
        // disk before the new landing arms it again, and a reset in the first frames is not read
        // as a crash on the next start.
        self.disarm();
        let mut fresh = Self::new(self.forced);
        fresh.over_video = self.over_video;
        fresh.refresh_hz = self.refresh_hz;
        fresh.changed = true;
        fresh.memory = std::mem::take(&mut self.memory);
        *self = fresh;
    }

    fn elapsed_ms(&self) -> u64 {
        self.opened.map_or(0, |at| at.elapsed().as_millis() as u64) + self.head_start_ms
    }

    /// Whether the chain's next run may be timed: lens or live, settled, and its probe has no
    /// verdict yet. A timed run reads a pixel back, which the GPU renderer allows only for a frame
    /// kept in its back buffer (`sdl::read_back_frames`), so the frame asks for that before it draws.
    pub fn timing(&self) -> bool {
        let tier = self.pass_tier();
        tier.probed() && self.decided != Some(tier) && Probe::settled(self.elapsed_ms())
    }

    /// Whether the tier changed since last asked.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Commits the end of a probe: the tier it settled on (unless held by `NIGHTJAR_GLASS`) and
    /// the guard cleared.
    fn finish(&mut self, tier: Option<Tier>) {
        let armed = self.armed.take();
        let mut stored = self.memory.stored().clone();
        if let Some(tier) = tier {
            stored.tier = Some(tier);
        }
        if let Some(fingerprint) = armed {
            stored.guard = Guard::clear(&fingerprint);
            stored.trips = 0;
        }
        self.memory.save(stored);
    }

    /// Clears a guard still armed and keeps what is stored, so a crash later in the session (a
    /// stream's, an abort) is not taken for the glass's.
    fn disarm(&mut self) {
        if let Some(fingerprint) = self.armed.take() {
            self.memory.save(Stored { guard: Guard::clear(&fingerprint), trips: 0, ..self.memory.stored().clone() });
        }
    }

    fn step_down(&mut self, why: &str) {
        eprintln!("nightjar: live glass off, baked glass from here on: {why}");
        self.landing = Some(Tier::Baked);
        self.probe = None;
        self.print = None;
        self.changed = true;
        // A failure is no measurement: nothing new is stored, the guard is cleared.
        self.finish(None);
    }

    /// Puts a frame on the stage. `textures_changed`: a texture was uploaded for this frame.
    pub fn paint<S: Stage>(
        &mut self,
        stage: &mut S,
        primitives: &[ClippedPrimitive],
        textures_changed: bool,
    ) {
        let found = self.pass_tier().probed().then(|| find_boundary(primitives)).flatten();
        let Some((at, boundary)) = found else {
            if self.held {
                self.held = false;
                self.print = None;
                stage.release();
            }
            draw(stage, primitives);
            return;
        };
        // A window with no pixels (minimized) has nothing to draw; that is no failure of the chain.
        let size = stage.size();
        if size.0 <= 0 || size.1 <= 0 {
            return;
        }
        let (content, chrome) = primitives.split_at(at);
        let print = (fingerprint(content, &boundary.regions), size);
        if textures_changed || self.print != Some(print) {
            self.run_chain(stage, content, print);
        } else {
            draw(stage, content);
        }
        draw(stage, chrome);
    }

    fn run_chain<S: Stage>(
        &mut self,
        stage: &mut S,
        content: &[ClippedPrimitive],
        print: (u64, (i32, i32)),
    ) {
        if let Err(why) = stage.begin_scene() {
            self.step_down(&why);
            self.held = true;
            draw(stage, content);
            return;
        }
        self.held = true;
        draw(stage, content);
        let timed = self.timing();
        match stage.end_scene(timed) {
            Err(why) => self.step_down(&why),
            Ok(ms) => {
                self.print = Some(print);
                // The chain ran a frame cleanly: the guard has done its work (design spec 4.3, step 2).
                self.disarm();
                if let Some(ms) = ms.filter(|_| timed) {
                    self.measure(ms);
                }
            }
        }
    }

    /// One timed run of the chain, as the probe's frame: the work is the chain's own time; when it
    /// landed on screen is not known here, so the work alone decides (`Probe::slow`).
    fn measure(&mut self, ms: f32) {
        let tier = self.pass_tier();
        let (hz, elapsed) = (self.refresh_hz, self.elapsed_ms());
        if self.probe.as_ref().is_none_or(|probe| probe.tier() != tier) {
            self.probe = Probe::new(tier, hz);
        }
        let Some(probe) = self.probe.as_mut() else { return };
        let work_ns = (f64::from(ms.max(0.0)) * 1e6) as u64;
        let Some(verdict) = probe.frame(elapsed, work_ns, None) else { return };
        let (frames, slow) = probe.counts();
        self.probe = None;
        self.decided = Some(tier);
        let budget_ms = probe::budget_ns(hz) as f64 / 1e6;
        let measured = format!("{slow} of {frames} runs over {:.0} % of a {budget_ms:.1} ms frame", probe::P90_BUDGET_FRACTION * 100.0);
        match verdict {
            Verdict::StepDown(_) if self.forced.is_some() => {
                eprintln!("nightjar: {} glass held by NIGHTJAR_GLASS, {measured}", tier.key());
            }
            Verdict::StepDown(next) => {
                eprintln!("nightjar: {} glass steps down to {}: {measured}", tier.key(), next.key());
                self.landing = Some(next);
                self.decided = None;
                self.changed = true;
                if next.probed() {
                    // Lens to live keeps the chain and its output; live is probed in turn.
                    self.memory.save(Stored { tier: Some(next), ..self.memory.stored().clone() });
                } else {
                    self.print = None;
                    self.finish(Some(next));
                }
            }
            Verdict::Keep(kept) => {
                eprintln!("nightjar: {} glass holds, {measured}", kept.key());
                if self.forced.is_none() {
                    self.finish(Some(kept));
                }
            }
        }
    }

    /// The first launcher frame: lands the tier. A guard still armed for this build and driver
    /// after `GUARD_TRIPS` unfinished probes in a row lands on baked and stores that; otherwise
    /// the guard is armed, counting an unfinished last probe, before the targets are tried.
    fn land<S: Stage>(&mut self, stage: &mut S) {
        if let Some(forced) = self.forced {
            match stage.targets_work() {
                Ok(()) => self.landed_on(forced),
                Err(why) => self.step_down(&why),
            }
            return;
        }
        let fingerprint = fingerprint_of(stage);
        let stored = self.memory.stored().clone();
        let trips = if stored.guard.tripped(&fingerprint) { stored.trips.saturating_add(1) } else { 0 };
        if trips >= probe::GUARD_TRIPS {
            eprintln!("nightjar: {}", probe::GUARD_WARNING);
            self.landing = Some(Tier::Baked);
            self.changed = true;
            self.memory.save(Stored { tier: Some(Tier::Baked), guard: Guard::clear(&fingerprint), trips: 0 });
            return;
        }
        if trips > 0 {
            eprintln!("nightjar: the last live-surface probe on this build and driver did not finish; trying it once more before baked surfaces");
        }
        let stored_tier = stored.tier.filter(|_| stored.guard.fingerprint == fingerprint);
        if stored_tier == Some(Tier::Baked) {
            // A probe on this build and driver already stepped down to baked: no target is made.
            self.landing = Some(Tier::Baked);
            self.changed = true;
            return;
        }
        self.memory.save(Stored { tier: stored_tier, guard: Guard::arm(&fingerprint), trips });
        self.armed = Some(fingerprint);
        match stage.targets_work() {
            Ok(()) => {
                // Targets are all lens and live need on the desktop; high contrast is applied per
                // frame (`tier`), so leaving it brings the landed tier back.
                let device = Device { lens: true, live: true, ..Device::default() };
                self.landed_on(probe::land(&device, stored_tier));
            }
            Err(why) => self.step_down(&why),
        }
    }

    fn landed_on(&mut self, tier: Tier) {
        self.landing = Some(tier);
        self.opened = Some(Instant::now());
        self.changed = true;
    }

    /// After a frame's meshes: tries the targets on the first launcher frame, and frees them once
    /// the frame that stepped down no longer samples them.
    pub fn settle<S: Stage>(&mut self, stage: &mut S) {
        if self.landing.is_none() && !self.over_video {
            self.land(stage);
        }
        if self.landing == Some(Tier::Baked) && self.held {
            self.held = false;
            stage.release();
        }
    }
}

impl Drop for Glass {
    /// A clean exit is no crash: a probe still running leaves no armed guard behind.
    fn drop(&mut self) {
        self.finish(None);
    }
}

fn draw<S: Stage>(stage: &mut S, primitives: &[ClippedPrimitive]) {
    for clipped in primitives {
        if let Primitive::Mesh(mesh) = &clipped.primitive {
            stage.mesh(clipped.clip_rect, mesh);
        }
    }
}

/// The levels as divisors of the scene: 1/2, 1/4, 1/8, 1/16 and back up to 1/4, the last sampled by
/// the chrome.
pub const LEVELS: [i32; 6] = [2, 4, 8, 16, 8, 4];

/// A level's size in pixels. Every level covers the whole screen, whatever the rounding, so chrome's
/// screen-space UVs land at the same place on it.
pub fn level_size((w, h): (i32, i32), div: i32) -> (i32, i32) {
    ((w / div).max(1), (h / div).max(1))
}

#[cfg(test)]
mod tests;
