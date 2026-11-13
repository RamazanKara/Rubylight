//! Live glass on the desktop: the content under the chrome is painted into a render target, run
//! through a linear down and up chain, and sampled by the chrome's meshes in screen space.
//!
//! The chain runs only when a `Boundary` is in the frame, so a playing stream (which has none) pays
//! nothing, and only when what lies under the chrome changed. The first runs are timed; a chain
//! that does not fit the frame budget, or targets that cannot be made, step down to the baked tier
//! and say so in the log.

use std::any::Any;
use std::hash::Hasher;

use egui::epaint::{ClippedPrimitive, Mesh, Primitive, TextureId};
use egui::{Context, Id, Rect};
use sdl3_sys::everything::*;

use crate::ui::design_tokens as t;

/// How glass is drawn on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Live blur of what is really behind the chrome.
    Live,
    /// The static ambient sampled in screen space: a complete design of its own.
    Baked,
    /// Opaque surfaces with a light rim: high contrast.
    Solid,
}

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

/// A hash of everything painted under `regions` or within the blur's reach of them: triangle by
/// triangle, so a hover elsewhere in the same batched mesh leaves it alone. Equal hashes mean the
/// chain's last output still holds.
pub fn fingerprint(primitives: &[ClippedPrimitive], regions: &[Rect]) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
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
    let mut hash = std::collections::hash_map::DefaultHasher::new();
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

/// What the probe of the first chain runs decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Holds { p90_ms: f32 },
    TooSlow { p90_ms: f32 },
}

/// Times the chain over its first runs; its p90 against the frame budget lands the tier.
#[derive(Clone, Debug)]
pub struct Probe {
    budget_ms: f32,
    samples: Vec<f32>,
    verdict: Option<Verdict>,
}

impl Probe {
    pub fn new(budget_ms: f32) -> Self {
        Self {
            budget_ms,
            samples: Vec::with_capacity(t::TIER_PROBE_FRAMES as usize),
            verdict: None,
        }
    }

    pub fn running(&self) -> bool {
        self.verdict.is_none()
    }

    /// One run's time. The run that completes the probe returns its verdict.
    pub fn push(&mut self, ms: f32) -> Option<Verdict> {
        if self.verdict.is_some() {
            return None;
        }
        self.samples.push(ms);
        if self.samples.len() < t::TIER_PROBE_FRAMES as usize {
            return None;
        }
        self.samples.sort_by(f32::total_cmp);
        let p90_ms = self.samples[self.samples.len() * 9 / 10];
        let verdict = if p90_ms > self.budget_ms * t::TIER_PROBE_P90_BUDGET_FRACTION {
            Verdict::TooSlow { p90_ms }
        } else {
            Verdict::Holds { p90_ms }
        };
        self.verdict = Some(verdict);
        Some(verdict)
    }
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
}

/// Where the machine's glass landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Landing {
    /// Baked, until the first launcher frame tried the render targets.
    Untried,
    Live,
    Baked,
}

/// What the chain may take on a screen of this rate: a frame, but never less than one at 120 Hz. A
/// fast screen losing a frame now and then while the launcher scrolls is not seen; live glass going
/// for good is.
fn chain_budget_ms(hz: f32) -> f32 {
    (1000.0 / hz).max(1000.0 / 120.0)
}

/// `NIGHTJAR_GLASS=baked|live`: holds the tier for an A/B run. `live` keeps a chain over budget.
pub fn forced_by(value: Option<&str>) -> Option<Tier> {
    match value? {
        "baked" => Some(Tier::Baked),
        "live" => Some(Tier::Live),
        other => {
            eprintln!("nightjar: NIGHTJAR_GLASS is {other:?}, not baked or live: ignored");
            None
        }
    }
}

/// The tier's landing and when the chain runs. No SDL in here: a frame without a boundary reaches
/// the stage as its meshes alone, and a frame whose print did not change runs no chain.
pub struct Glass {
    forced: Option<Tier>,
    landing: Landing,
    over_video: bool,
    budget_ms: f32,
    probe: Option<Probe>,
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
            landing: if forced == Some(Tier::Baked) {
                Landing::Baked
            } else {
                Landing::Untried
            },
            over_video: false,
            budget_ms: 1000.0 / 60.0,
            probe: None,
            print: None,
            held: false,
            changed: false,
        }
    }

    /// A glass already live, for the screenshot canvas.
    #[cfg(test)]
    pub fn live() -> Self {
        let mut glass = Self::new(None);
        glass.landing = Landing::Live;
        glass
    }

    /// One display refresh, what the probe measures the chain against.
    pub fn set_refresh(&mut self, hz: f32) {
        if hz <= 0.0 {
            return;
        }
        let budget_ms = chain_budget_ms(hz);
        // A verdict holds for the budget it was against: another frame time is judged again.
        if budget_ms != self.budget_ms {
            self.budget_ms = budget_ms;
            self.probe = None;
        }
    }

    /// The tier this pass lays out with: never live over a stream's video, which is not repainted
    /// into a scene (the chain would blur an empty target and clear the picture).
    pub fn pass_tier(&self) -> Tier {
        if self.landing == Landing::Live && !self.over_video {
            Tier::Live
        } else {
            Tier::Baked
        }
    }

    /// Whether the frames from here on draw over a stream's video. True when the caller must free
    /// the chain's targets: a stream holds none, whatever it draws or does not draw.
    #[must_use]
    pub fn set_over_video(&mut self, over_video: bool) -> bool {
        self.over_video = over_video;
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
        *self = Self { over_video: self.over_video, budget_ms: self.budget_ms, changed: true, ..Self::new(self.forced) };
    }

    /// Whether the chain's next run may be timed: live, and its probe has no verdict yet. A timed
    /// run reads a pixel back, which the GPU renderer allows only for a frame kept in its back
    /// buffer (`sdl::read_back_frames`), so the frame asks for that before it draws.
    pub fn timing(&self) -> bool {
        self.pass_tier() == Tier::Live && self.probe.as_ref().is_none_or(Probe::running)
    }

    /// Whether the tier changed since last asked.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn step_down(&mut self, why: &str) {
        eprintln!("nightjar: live glass off, baked glass from here on: {why}");
        self.landing = Landing::Baked;
        self.print = None;
        self.changed = true;
    }

    /// Puts a frame on the stage. `textures_changed`: a texture was uploaded for this frame.
    pub fn paint<S: Stage>(
        &mut self,
        stage: &mut S,
        primitives: &[ClippedPrimitive],
        textures_changed: bool,
    ) {
        let found = (self.pass_tier() == Tier::Live)
            .then(|| find_boundary(primitives))
            .flatten();
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
        let budget_ms = self.budget_ms;
        let probe = self.probe.get_or_insert_with(|| Probe::new(budget_ms));
        match stage.end_scene(probe.running()) {
            Err(why) => self.step_down(&why),
            Ok(ms) => {
                self.print = Some(print);
                match ms.and_then(|ms| probe.push(ms)) {
                    Some(Verdict::TooSlow { p90_ms }) if self.forced != Some(Tier::Live) => {
                        self.step_down(&format!(
                            "the chain's p90 is {p90_ms:.1} ms against a {budget_ms:.1} ms frame"
                        ));
                    }
                    Some(Verdict::TooSlow { p90_ms }) => eprintln!("nightjar: live glass held by NIGHTJAR_GLASS, p90 {p90_ms:.1} ms against a {budget_ms:.1} ms frame"),
                    Some(Verdict::Holds { p90_ms }) => eprintln!("nightjar: live glass holds, p90 {p90_ms:.1} ms against a {budget_ms:.1} ms frame"),
                    None => {}
                }
            }
        }
    }

    /// After a frame's meshes: tries the targets on the first launcher frame, and frees them once
    /// the frame that stepped down no longer samples them.
    pub fn settle<S: Stage>(&mut self, stage: &mut S) {
        if self.landing == Landing::Untried && !self.over_video {
            match stage.targets_work() {
                Ok(()) => {
                    self.landing = Landing::Live;
                    self.changed = true;
                }
                Err(why) => self.step_down(&why),
            }
        }
        if self.landing == Landing::Baked && self.held {
            self.held = false;
            stage.release();
        }
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

/// The render targets of the chain.
pub struct Chain {
    size: (i32, i32),
    scene: *mut SDL_Texture,
    levels: Vec<*mut SDL_Texture>,
    ran: bool,
}

impl Default for Chain {
    fn default() -> Self {
        Self {
            size: (0, 0),
            scene: std::ptr::null_mut(),
            levels: Vec::new(),
            ran: false,
        }
    }
}

impl Chain {
    /// The texture chrome samples, once the chain has run.
    pub fn output(&self) -> Option<*mut SDL_Texture> {
        self.levels.last().copied().filter(|_| self.ran)
    }

    /// Frees the targets: the frame has no chrome (a stream plays, or the window closes).
    pub fn release(&mut self) {
        // SAFETY: textures we created, each freed once.
        unsafe {
            if !self.scene.is_null() {
                SDL_DestroyTexture(self.scene);
            }
            for level in self.levels.drain(..) {
                if !level.is_null() {
                    SDL_DestroyTexture(level);
                }
            }
        }
        self.scene = std::ptr::null_mut();
        self.size = (0, 0);
        self.ran = false;
    }

    /// A target in the output's own colour space, so content painted through the scene comes out
    /// as content painted directly: a linear output blends in linear light in both. Null on failure.
    fn make_target(renderer: *mut SDL_Renderer, (w, h): (i32, i32)) -> *mut SDL_Texture {
        // SAFETY: SDL calls on a live renderer; the properties are ours and freed here.
        unsafe {
            let output = SDL_GetNumberProperty(
                SDL_GetRendererProperties(renderer),
                SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER,
                i64::from(SDL_COLORSPACE_SRGB.0),
            );
            let (format, space) = if output == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0) {
                (SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR)
            } else {
                (SDL_PIXELFORMAT_RGBA32, SDL_COLORSPACE_SRGB)
            };
            let props = SDL_CreateProperties();
            let set = |name, value: i64| SDL_SetNumberProperty(props, name, value);
            set(SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
            set(
                SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER,
                i64::from(SDL_TEXTUREACCESS_TARGET.0),
            );
            set(SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(w.max(1)));
            set(SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, i64::from(h.max(1)));
            set(
                SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER,
                i64::from(space.0),
            );
            let texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            if !texture.is_null() {
                SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_NONE);
                SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_LINEAR);
            }
            texture
        }
    }

    /// One small target made, drawn to and freed: can this renderer do it at all?
    pub fn check(renderer: *mut SDL_Renderer) -> Result<(), String> {
        let target = Self::make_target(renderer, (16, 16));
        if target.is_null() {
            return Err(format!(
                "a render target could not be made: {}",
                crate::sdl::error()
            ));
        }
        // SAFETY: a target we just made on this live renderer; the frame's target comes back.
        unsafe {
            let ok = SDL_SetRenderTarget(renderer, target)
                && SDL_RenderClear(renderer)
                && SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            let why = crate::sdl::error();
            SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            SDL_DestroyTexture(target);
            ok.then_some(())
                .ok_or_else(|| format!("the render target could not be set: {why}"))
        }
    }

    /// Points rendering at the scene target, its size matching the output, cleared to the frame's
    /// draw colour as the back buffer was.
    pub fn begin(&mut self, renderer: *mut SDL_Renderer) -> Result<(), String> {
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer and two live integers.
        unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
        if w <= 0 || h <= 0 {
            return Err(format!("the output is {w}x{h}"));
        }
        if self.size != (w, h) {
            self.release();
            self.scene = Self::make_target(renderer, (w, h));
            self.levels = LEVELS
                .iter()
                .map(|&div| Self::make_target(renderer, level_size((w, h), div)))
                .collect();
            if self.scene.is_null() || self.levels.iter().any(|l| l.is_null()) {
                let why = crate::sdl::error();
                self.release();
                return Err(format!("a render target could not be made: {why}"));
            }
            if let Some(&output) = self.levels.last() {
                // Chrome's feathered edges blend over the frame; no pass reads this level.
                // SAFETY: a target we just made.
                unsafe { SDL_SetTextureBlendMode(output, crate::gui::premultiplied_blend()) };
            }
            self.size = (w, h);
        }
        // SAFETY: a target we created on this renderer.
        unsafe {
            if !SDL_SetRenderTarget(renderer, self.scene) {
                return Err(format!(
                    "the render target could not be set: {}",
                    crate::sdl::error()
                ));
            }
            SDL_SetRenderClipRect(renderer, std::ptr::null());
            SDL_RenderClear(renderer);
        }
        Ok(())
    }

    /// With the content on the scene target: runs the chain, then puts the scene on the back buffer.
    pub fn run(&mut self, renderer: *mut SDL_Renderer, timed: bool) -> Result<Option<f32>, String> {
        // SAFETY: targets we created on this renderer; the null rects mean the whole texture.
        unsafe {
            SDL_SetRenderClipRect(renderer, std::ptr::null());
            // Waits for what was queued, so the time below is the chain's own.
            let mut timing = if timed { sync(renderer).map(|()| Some(std::time::Instant::now())) } else { Ok(None) };
            let mut from = self.scene;
            let mut ok = true;
            for &level in &self.levels {
                ok &= SDL_SetRenderTarget(renderer, level);
                ok &= SDL_RenderTexture(renderer, from, std::ptr::null(), std::ptr::null());
                from = level;
            }
            if let Ok(Some(_)) = timing {
                timing = sync(renderer).and(timing);
            }
            let ms = timing.map(|started| started.map(|started| started.elapsed().as_secs_f32() * 1000.0));
            // The scene reaches the screen whether or not the chain could be timed.
            ok &= SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            ok &= SDL_RenderTexture(renderer, self.scene, std::ptr::null(), std::ptr::null());
            if !ok {
                return Err(format!("a chain pass failed: {}", crate::sdl::error()));
            }
            let ms = ms?;
            self.ran = true;
            Ok(ms)
        }
    }
}

/// Blocks until the GPU finished the current target: reading one pixel back waits for it. A
/// read-back that fails waited for nothing, so the chain's time would be wrong: an error, which
/// steps the glass down saying why.
///
/// # Safety
/// Needs a live renderer.
unsafe fn sync(renderer: *mut SDL_Renderer) -> Result<(), String> {
    let one = SDL_Rect {
        x: 0,
        y: 0,
        w: 1,
        h: 1,
    };
    // SAFETY: as the function says; the surface is ours to free.
    unsafe {
        SDL_FlushRenderer(renderer);
        let surface = SDL_RenderReadPixels(renderer, &one);
        if surface.is_null() {
            let why = crate::sdl::error();
            eprintln!("nightjar: live glass: the chain could not be timed, its read-back failed: {why}");
            return Err(format!("the chain could not be timed: {why}"));
        }
        SDL_DestroySurface(surface);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
