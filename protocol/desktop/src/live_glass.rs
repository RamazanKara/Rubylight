//! Live glass on the desktop: the content under the chrome is painted into a render target, run
//! through a linear down and up chain, and sampled by the chrome's meshes in screen space.
//!
//! The chain runs only when a `Boundary` is in the frame, so a playing stream (which has none) pays
//! nothing, and only when what lies under the chrome changed. The first runs are timed; a chain
//! that does not fit the frame budget, or targets that cannot be made, step down to the baked tier
//! and say so in the log.

use std::any::Any;
use std::hash::Hasher;

use egui::epaint::{ClippedPrimitive, Primitive, TextureId};
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

/// A hash of everything painted under `regions`: triangle by triangle, so a hover elsewhere in the
/// same batched mesh leaves it alone. Equal hashes mean the chain's last output still holds.
pub fn fingerprint(primitives: &[ClippedPrimitive], regions: &[Rect]) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for clipped in primitives {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        let clip = clipped.clip_rect;
        let near: Vec<Rect> = regions
            .iter()
            .map(|r| r.intersect(clip))
            .filter(|r| r.is_positive())
            .collect();
        if near.is_empty() {
            continue;
        }
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

/// The levels: scene, then 1/2, 1/4, 1/8, 1/16 and back up to 1/4, sampled by the chrome.
const DOWN: [i32; 4] = [2, 4, 8, 16];
const UP: [i32; 2] = [8, 4];

/// The render targets of the chain and what it decided so far.
pub struct Chain {
    size: (i32, i32),
    scene: *mut SDL_Texture,
    levels: Vec<*mut SDL_Texture>,
    print: Option<u64>,
    probe: Option<Probe>,
    /// Set when the chain failed or lost its probe: the caller drops to baked.
    pub failed: bool,
}

impl Default for Chain {
    fn default() -> Self {
        Self {
            size: (0, 0),
            scene: std::ptr::null_mut(),
            levels: Vec::new(),
            print: None,
            probe: None,
            failed: false,
        }
    }
}

impl Chain {
    /// The texture chrome samples, once the chain has run.
    pub fn output(&self) -> Option<*mut SDL_Texture> {
        self.print.and(self.levels.last().copied())
    }

    /// True when the chain must run again for this frame.
    pub fn dirty(&self, print: u64, textures_changed: bool) -> bool {
        textures_changed || self.print != Some(print)
    }

    /// Forgets the output: the device lost the targets' contents.
    pub fn invalidate(&mut self) {
        self.print = None;
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
        self.print = None;
    }

    fn make_target(renderer: *mut SDL_Renderer, w: i32, h: i32) -> *mut SDL_Texture {
        // SAFETY: SDL calls on a live renderer; a null result is checked by the caller.
        unsafe {
            let texture = SDL_CreateTexture(
                renderer,
                SDL_PIXELFORMAT_RGBA32,
                SDL_TEXTUREACCESS_TARGET,
                w.max(1),
                h.max(1),
            );
            if !texture.is_null() {
                SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_NONE);
                SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_LINEAR);
            }
            texture
        }
    }

    fn fail(&mut self, why: &str) {
        eprintln!("nightjar: live glass off, baked glass from here on: {why}");
        self.release();
        self.failed = true;
    }

    /// Points rendering at the scene target, its size matching the output. False when it cannot.
    pub fn begin(&mut self, renderer: *mut SDL_Renderer) -> bool {
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer and two live integers.
        unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
        if w <= 0 || h <= 0 {
            return false;
        }
        if self.size != (w, h) {
            self.release();
            self.scene = Self::make_target(renderer, w, h);
            let sizes = DOWN.iter().chain(&UP);
            self.levels = sizes
                .map(|div| Self::make_target(renderer, w / div, h / div))
                .collect();
            if self.scene.is_null() || self.levels.iter().any(|l| l.is_null()) {
                self.fail("a render target could not be made");
                return false;
            }
            self.size = (w, h);
        }
        // SAFETY: targets we created on this renderer.
        let ok = unsafe {
            SDL_SetRenderClipRect(renderer, std::ptr::null());
            let ok = SDL_SetRenderTarget(renderer, self.scene);
            if ok {
                SDL_RenderClear(renderer);
            }
            ok
        };
        if !ok {
            self.fail("the render target could not be set");
        }
        ok
    }

    /// With the content on the scene target: runs the chain, then puts the scene on the back
    /// buffer. `budget_ms` is one display refresh.
    pub fn finish(&mut self, renderer: *mut SDL_Renderer, print: u64, budget_ms: f32) {
        let probing = self
            .probe
            .get_or_insert_with(|| Probe::new(budget_ms))
            .running();
        // SAFETY: targets we created on this renderer; the null rects mean the whole texture.
        unsafe {
            SDL_SetRenderClipRect(renderer, std::ptr::null());
            let started = if probing {
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
            let mut verdict = None;
            if let Some(started) = started {
                sync(renderer);
                let ms = started.elapsed().as_secs_f32() * 1000.0;
                verdict = self.probe.as_mut().and_then(|p| p.push(ms));
            }
            ok &= SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            ok &= SDL_RenderTexture(renderer, self.scene, std::ptr::null(), std::ptr::null());
            match verdict {
                Some(Verdict::TooSlow { p90_ms }) => {
                    self.fail(&format!("the chain's p90 is {p90_ms:.1} ms against a {budget_ms:.1} ms frame"));
                    return;
                }
                Some(Verdict::Holds { p90_ms }) => eprintln!("nightjar: live glass holds, p90 {p90_ms:.1} ms against a {budget_ms:.1} ms frame"),
                None => {}
            }
            if !ok {
                self.fail("a chain pass failed");
                return;
            }
        }
        self.print = Some(print);
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
