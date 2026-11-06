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
        for tri in mesh.indices.chunks_exact(3) {
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
        if hz > 0.0 {
            self.budget_ms = 1000.0 / hz;
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
        let (content, chrome) = primitives.split_at(at);
        let print = (fingerprint(content, &boundary.regions), stage.size());
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
            let started = if timed {
                // Waits for what was queued, so the time below is the chain's own.
                sync(renderer);
                Some(std::time::Instant::now())
            } else {
                None
            };
            let mut from = self.scene;
            let mut ok = true;
            for &level in &self.levels {
                ok &= SDL_SetRenderTarget(renderer, level);
                ok &= SDL_RenderTexture(renderer, from, std::ptr::null(), std::ptr::null());
                from = level;
            }
            let ms = started.map(|started| {
                sync(renderer);
                started.elapsed().as_secs_f32() * 1000.0
            });
            ok &= SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            ok &= SDL_RenderTexture(renderer, self.scene, std::ptr::null(), std::ptr::null());
            if !ok {
                return Err(format!("a chain pass failed: {}", crate::sdl::error()));
            }
            self.ran = true;
            Ok(ms)
        }
    }
}

/// Blocks until the GPU finished the current target: reading one pixel back waits for it.
///
/// # Safety
/// Needs a live renderer.
unsafe fn sync(renderer: *mut SDL_Renderer) {
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
        if !surface.is_null() {
            SDL_DestroySurface(surface);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    use egui::{pos2, Color32};

    fn mesh(rects: &[(Rect, Color32)]) -> ClippedPrimitive {
        let mut m = Mesh::default();
        for (rect, color) in rects {
            let base = m.vertices.len() as u32;
            for pos in [
                rect.left_top(),
                rect.right_top(),
                rect.right_bottom(),
                rect.left_bottom(),
            ] {
                m.vertices.push(Vertex {
                    pos,
                    uv: WHITE_UV,
                    color: *color,
                });
            }
            m.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Mesh(m),
        }
    }

    #[test]
    fn only_what_lies_under_the_chrome_changes_the_print() {
        let chrome = [Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))];
        let under = Rect::from_min_max(pos2(50.0, 40.0), pos2(90.0, 90.0));
        let below = Rect::from_min_max(pos2(50.0, 300.0), pos2(90.0, 400.0));
        let print = |under_color, below_color| {
            fingerprint(
                &[mesh(&[(under, under_color), (below, below_color)])],
                &chrome,
            )
        };
        let base = print(Color32::RED, Color32::RED);
        assert_eq!(
            base,
            print(Color32::RED, Color32::BLUE),
            "a hover far below runs no chain"
        );
        assert_ne!(
            base,
            print(Color32::BLUE, Color32::RED),
            "a change under the chrome does"
        );
    }

    #[test]
    fn scrolling_content_under_the_chrome_changes_the_print() {
        let chrome = [Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))];
        let at = |y: f32| {
            fingerprint(
                &[mesh(&[(
                    Rect::from_min_max(pos2(50.0, y), pos2(90.0, y + 100.0)),
                    Color32::RED,
                )])],
                &chrome,
            )
        };
        assert_ne!(at(10.0), at(11.0));
        assert_eq!(at(200.0), at(300.0), "nothing under the chrome both times");
    }

    #[test]
    fn a_change_just_outside_the_chrome_reaches_it_through_the_blur() {
        let chrome = [Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))];
        let beside = Rect::from_min_max(pos2(50.0, 80.0), pos2(90.0, 90.0));
        let print = |color| fingerprint(&[mesh(&[(beside, color)])], &chrome);
        assert_ne!(print(Color32::RED), print(Color32::BLUE));
    }

    #[test]
    fn a_clip_change_on_content_under_the_chrome_changes_the_print() {
        let chrome = [bar()];
        let print = |bottom: f32| {
            let mut content = mesh(&[(bar(), Color32::RED)]);
            content.clip_rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, bottom));
            fingerprint(&[content], &chrome)
        };
        assert_ne!(print(60.0), print(30.0), "the same triangles, less of them showing");
    }

    /// What reached the stage, in order.
    #[derive(Debug, PartialEq, Clone, Copy)]
    enum Call {
        Mesh,
        Begin,
        End { timed: bool },
        Release,
        Try,
    }

    #[derive(Default)]
    struct Recorder {
        calls: Vec<Call>,
        size: (i32, i32),
        targets_fail: bool,
        begin_fails: bool,
        end_fails: bool,
        ms: f32,
    }

    impl Stage for Recorder {
        fn size(&self) -> (i32, i32) {
            self.size
        }
        fn mesh(&mut self, _: Rect, _: &Mesh) {
            self.calls.push(Call::Mesh);
        }
        fn targets_work(&mut self) -> Result<(), String> {
            self.calls.push(Call::Try);
            if self.targets_fail {
                Err("no targets".into())
            } else {
                Ok(())
            }
        }
        fn begin_scene(&mut self) -> Result<(), String> {
            self.calls.push(Call::Begin);
            if self.begin_fails {
                Err("no scene".into())
            } else {
                Ok(())
            }
        }
        fn end_scene(&mut self, timed: bool) -> Result<Option<f32>, String> {
            self.calls.push(Call::End { timed });
            if self.end_fails {
                Err("no pass".into())
            } else {
                Ok(timed.then_some(self.ms))
            }
        }
        fn release(&mut self) {
            self.calls.push(Call::Release);
        }
    }

    fn boundary_at(regions: Vec<Rect>) -> ClippedPrimitive {
        ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Callback(egui::epaint::PaintCallback {
                rect: Rect::EVERYTHING,
                callback: std::sync::Arc::new(Boundary { regions }),
            }),
        }
    }

    fn bar() -> Rect {
        Rect::from_min_max(pos2(0.0, 20.0), pos2(1000.0, 68.0))
    }

    /// Content under the chrome in `color`, the boundary, one chrome mesh.
    fn chrome_frame(color: Color32) -> Vec<ClippedPrimitive> {
        vec![
            mesh(&[(bar(), color)]),
            boundary_at(vec![bar()]),
            mesh(&[(bar(), Color32::WHITE)]),
        ]
    }

    fn stage() -> Recorder {
        Recorder {
            size: (1000, 800),
            ms: 1.0,
            ..Default::default()
        }
    }

    #[test]
    fn a_stream_frame_reaches_the_stage_as_its_meshes_alone() {
        let frame = [
            mesh(&[(bar(), Color32::RED)]),
            mesh(&[(bar(), Color32::BLUE)]),
        ];
        // On the machine's every landing: nothing but the two draws today's path makes.
        for mut glass in [
            Glass::new(None),
            Glass::new(Some(Tier::Baked)),
            Glass::live(),
        ] {
            let mut stage = stage();
            glass.paint(&mut stage, &frame, true);
            assert_eq!(stage.calls, [Call::Mesh, Call::Mesh]);
        }
    }

    #[test]
    fn over_a_streams_video_there_is_no_chain_even_with_a_boundary() {
        let mut glass = Glass::live();
        assert!(!glass.set_over_video(true), "nothing held, nothing to free");
        assert_eq!(glass.pass_tier(), Tier::Baked, "so no boundary is laid out");
        let mut stage = stage();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), true);
        glass.settle(&mut stage);
        assert_eq!(
            stage.calls,
            [Call::Mesh, Call::Mesh],
            "the video stays on the back buffer"
        );
    }

    #[test]
    fn a_stream_starting_frees_the_targets_even_if_it_never_draws_an_overlay() {
        let mut glass = Glass::live();
        let mut stage = stage();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), true);
        assert!(
            glass.set_over_video(true),
            "the launcher's targets go at the stream's first frame"
        );
        assert!(!glass.set_over_video(true), "and only once");
        // Back on the launcher the chain is built anew.
        assert!(!glass.set_over_video(false));
        assert_eq!(glass.pass_tier(), Tier::Live);
        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
        assert_eq!(stage.calls[0], Call::Begin);
    }

    #[test]
    fn a_page_without_chrome_frees_the_targets_once() {
        let mut glass = Glass::live();
        let mut stage = stage();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), true);
        stage.calls.clear();
        let plain = [mesh(&[(bar(), Color32::RED)])];
        glass.paint(&mut stage, &plain, false);
        glass.paint(&mut stage, &plain, false);
        assert_eq!(stage.calls, [Call::Release, Call::Mesh, Call::Mesh]);
    }

    #[test]
    fn content_goes_through_the_scene_and_chrome_on_top_of_it() {
        let mut glass = Glass::live();
        let mut stage = stage();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
        assert_eq!(
            stage.calls,
            [
                Call::Begin,
                Call::Mesh,
                Call::End { timed: true },
                Call::Mesh
            ]
        );
    }

    #[test]
    fn nothing_changed_under_the_chrome_runs_no_chain() {
        let mut glass = Glass::live();
        let mut stage = stage();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
        assert_eq!(stage.calls, [Call::Mesh, Call::Mesh], "the same frame again");

        stage.calls.clear();
        let mut hover_far_below = chrome_frame(Color32::RED);
        hover_far_below.insert(
            0,
            mesh(&[(
                Rect::from_min_max(pos2(0.0, 500.0), pos2(90.0, 600.0)),
                Color32::BLUE,
            )]),
        );
        glass.paint(&mut stage, &hover_far_below, false);
        assert!(!stage.calls.contains(&Call::Begin), "{:?}", stage.calls);

        for (change, frame_color, size) in [
            ("content under it", Color32::BLUE, (1000, 800)),
            ("the window's size", Color32::BLUE, (900, 800)),
        ] {
            stage.calls.clear();
            stage.size = size;
            glass.paint(&mut stage, &chrome_frame(frame_color), false);
            assert_eq!(stage.calls[0], Call::Begin, "{change}");
        }

        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(Color32::BLUE), true);
        assert_eq!(stage.calls[0], Call::Begin, "an uploaded texture");

        glass.invalidate();
        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(Color32::BLUE), false);
        assert_eq!(stage.calls[0], Call::Begin, "targets reset");
    }

    #[test]
    fn a_chain_over_budget_steps_down_once_keeping_the_frame_it_decided_on() {
        let mut glass = Glass::live();
        let mut stage = stage();
        stage.ms = 30.0;
        for n in 0..t::TIER_PROBE_FRAMES {
            assert_eq!(glass.pass_tier(), Tier::Live, "run {n}");
            let color = Color32::from_gray(n as u8);
            stage.calls.clear();
            glass.paint(&mut stage, &chrome_frame(color), false);
            assert_eq!(stage.calls[2], Call::End { timed: true });
        }
        assert_eq!(glass.pass_tier(), Tier::Baked);
        assert!(glass.take_changed(), "the next pass follows at once");
        assert!(!glass.take_changed(), "once");
        assert!(
            !stage.calls.contains(&Call::Release),
            "chrome of the last frame sampled the output"
        );
        stage.calls.clear();
        glass.settle(&mut stage);
        assert_eq!(stage.calls, [Call::Release]);
        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(Color32::GREEN), true);
        glass.settle(&mut stage);
        assert_eq!(stage.calls, [Call::Mesh, Call::Mesh], "and never comes back");
    }

    #[test]
    fn a_chain_within_budget_stays_live_and_stops_timing() {
        let mut glass = Glass::live();
        let mut stage = stage();
        for n in 0..t::TIER_PROBE_FRAMES {
            glass.paint(&mut stage, &chrome_frame(Color32::from_gray(n as u8)), false);
        }
        assert_eq!(glass.pass_tier(), Tier::Live);
        stage.calls.clear();
        glass.paint(&mut stage, &chrome_frame(Color32::GREEN), false);
        assert_eq!(stage.calls[2], Call::End { timed: false }, "no more waits");
    }

    #[test]
    fn a_failing_chain_lands_on_baked_with_the_content_still_drawn() {
        for fails in ["begin", "end"] {
            let mut glass = Glass::live();
            let mut stage = stage();
            stage.begin_fails = fails == "begin";
            stage.end_fails = fails == "end";
            glass.paint(&mut stage, &chrome_frame(Color32::RED), false);
            assert_eq!(
                stage.calls.iter().filter(|c| **c == Call::Mesh).count(),
                2,
                "{fails}: content and chrome"
            );
            assert_eq!(glass.pass_tier(), Tier::Baked, "{fails}");
            assert!(glass.take_changed(), "{fails}");
            glass.settle(&mut stage);
            assert_eq!(stage.calls.last(), Some(&Call::Release), "{fails}");
        }
    }

    #[test]
    fn the_targets_are_tried_once_on_the_first_launcher_frame() {
        let mut glass = Glass::new(None);
        assert_eq!(glass.pass_tier(), Tier::Baked, "baked until they worked");
        let mut stage = stage();
        assert!(!glass.set_over_video(true));
        glass.settle(&mut stage);
        assert!(stage.calls.is_empty(), "a stream never tries them");
        assert!(!glass.set_over_video(false));
        glass.settle(&mut stage);
        assert_eq!(stage.calls, [Call::Try]);
        assert_eq!(glass.pass_tier(), Tier::Live);
        assert!(glass.take_changed(), "the next pass lays out live");
        glass.settle(&mut stage);
        assert_eq!(stage.calls, [Call::Try], "once");
        assert!(!glass.take_changed(), "and idle stays idle");

        let mut glass = Glass::new(None);
        let mut stage = self::stage();
        stage.targets_fail = true;
        glass.settle(&mut stage);
        assert_eq!(glass.pass_tier(), Tier::Baked);
        glass.settle(&mut stage);
        assert_eq!(stage.calls, [Call::Try], "a failure is not retried");
    }

    #[test]
    fn nightjar_glass_holds_the_tier_for_an_a_b_run() {
        assert_eq!(forced_by(None), None);
        assert_eq!(forced_by(Some("nonsense")), None);
        let mut baked = Glass::new(forced_by(Some("baked")));
        let mut stage = stage();
        baked.settle(&mut stage);
        assert!(stage.calls.is_empty(), "baked never tries the targets");
        assert_eq!(baked.pass_tier(), Tier::Baked);

        let mut live = Glass::new(forced_by(Some("live")));
        live.settle(&mut stage);
        stage.ms = 30.0;
        for n in 0..t::TIER_PROBE_FRAMES {
            live.paint(&mut stage, &chrome_frame(Color32::from_gray(n as u8)), false);
        }
        assert_eq!(live.pass_tier(), Tier::Live, "over budget and held");
    }

    #[test]
    fn a_frame_without_a_boundary_has_none_to_find() {
        // Stream frames: the overlay's meshes alone.
        assert!(find_boundary(&[mesh(&[(Rect::ZERO, Color32::RED)])]).is_none());
    }

    #[test]
    fn the_boundary_is_found_at_its_place() {
        let boundary = ClippedPrimitive {
            clip_rect: Rect::EVERYTHING,
            primitive: Primitive::Callback(egui::epaint::PaintCallback {
                rect: Rect::EVERYTHING,
                callback: std::sync::Arc::new(Boundary {
                    regions: vec![Rect::ZERO],
                }),
            }),
        };
        let frame = [
            mesh(&[(Rect::ZERO, Color32::RED)]),
            boundary,
            mesh(&[(Rect::ZERO, Color32::RED)]),
        ];
        assert_eq!(
            find_boundary(&frame).map(|(at, b)| (at, b.regions.len())),
            Some((1, 1))
        );
    }

    #[test]
    fn a_chain_over_its_budget_steps_down_and_one_within_it_stays() {
        let run = |ms: f32| {
            let mut probe = Probe::new(16.6);
            let mut verdict = None;
            for _ in 0..t::TIER_PROBE_FRAMES {
                assert!(verdict.is_none(), "one verdict, at the last sample");
                verdict = probe.push(ms);
            }
            assert!(!probe.running());
            verdict.expect("the last sample decides")
        };
        assert_eq!(run(2.0), Verdict::Holds { p90_ms: 2.0 });
        assert_eq!(run(14.0), Verdict::TooSlow { p90_ms: 14.0 });
    }

    #[test]
    fn a_few_slow_runs_do_not_decide_the_tier() {
        let mut probe = Probe::new(16.6);
        let mut verdict = None;
        for n in 0..t::TIER_PROBE_FRAMES {
            verdict = probe.push(if n % 20 == 0 { 40.0 } else { 2.0 });
        }
        assert!(
            matches!(verdict, Some(Verdict::Holds { .. })),
            "{verdict:?}"
        );
    }

    #[test]
    fn the_tier_is_baked_until_set_and_solid_in_high_contrast() {
        crate::theme::own_palette();
        let ctx = Context::default();
        assert_eq!(tier(&ctx), Tier::Baked);
        set_tier(&ctx, Tier::Live);
        assert_eq!(tier(&ctx), Tier::Live);
        crate::theme::set_high_contrast(true);
        assert_eq!(tier(&ctx), Tier::Solid);
        crate::theme::set_high_contrast(false);
    }
}
