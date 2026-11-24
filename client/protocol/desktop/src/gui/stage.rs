//! SDL3 mesh and texture stage (`SdlStage`) and tessellated egui painting for [`Gui`].

use std::collections::HashMap;
use std::time::Duration;

use egui::{Rect, TextureId};
use sdl3_sys::everything::*;

use super::{premultiplied_blend, Gui};
use crate::live_glass::{Chain, Stage, LIVE_BACKDROP};

impl Gui {
    pub(super) fn paint_tessellated(
        &mut self,
        renderer: *mut SDL_Renderer,
        mut output: egui::FullOutput,
        primitives: &[egui::ClippedPrimitive],
    ) {
        let textures_changed = !output.textures_delta.set.is_empty();
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.set_texture(renderer, id, &delta);
            }
        }
        let ppp = output.pixels_per_point;
        let mut stage = SdlStage {
            renderer,
            ppp,
            textures: &self.textures,
            xy: &mut self.xy,
            colors: &mut self.colors,
            chain: &mut self.chain,
            no_output_said: &mut self.no_output_said,
            window_state: self.window_state,
        };
        self.glass.paint(&mut stage, primitives, textures_changed);
        self.glass.settle(&mut stage);
        if self.glass.take_changed() {
            // Laid out for the old tier; one more pass lands on the new one, then idle again.
            self.repaint_delay = Duration::ZERO;
        }
        // SAFETY: a live renderer.
        unsafe { SDL_SetRenderClipRect(renderer, std::ptr::null()) };
        for id in output.textures_delta.free.drain() {
            if let Some(texture) = self.textures.remove(&id) {
                // SAFETY: a texture we created.
                unsafe { SDL_DestroyTexture(texture) };
            }
        }
    }

    fn set_texture(
        &mut self,
        renderer: *mut SDL_Renderer,
        id: TextureId,
        delta: &egui::epaint::ImageDelta,
    ) {
        let egui::ImageData::Color(image) = &delta.image;
        let [w, h] = image.size;
        // SAFETY: SDL calls on a live renderer with a buffer of w*h RGBA pixels.
        unsafe {
            let texture = match (delta.pos, self.textures.get(&id)) {
                (Some(_), Some(&texture)) => texture,
                _ => {
                    if let Some(old) = self.textures.remove(&id) {
                        SDL_DestroyTexture(old);
                    }
                    let texture = SDL_CreateTexture(
                        renderer,
                        SDL_PIXELFORMAT_RGBA32,
                        SDL_TEXTUREACCESS_STATIC,
                        w as i32,
                        h as i32,
                    );
                    if texture.is_null() {
                        return;
                    }
                    SDL_SetTextureBlendMode(texture, premultiplied_blend());
                    let mode = if delta.options.magnification == egui::TextureFilter::Nearest {
                        SDL_SCALEMODE_NEAREST
                    } else {
                        SDL_SCALEMODE_LINEAR
                    };
                    SDL_SetTextureScaleMode(texture, mode);
                    self.textures.insert(id, texture);
                    texture
                }
            };
            let [x, y] = delta.pos.unwrap_or([0, 0]);
            let rect = SDL_Rect {
                x: x as i32,
                y: y as i32,
                w: w as i32,
                h: h as i32,
            };
            SDL_UpdateTexture(texture, &rect, image.pixels.as_ptr().cast(), (w * 4) as i32);
        }
    }
}

/// A frame's meshes on the renderer that shows the video.
struct SdlStage<'a> {
    renderer: *mut SDL_Renderer,
    ppp: f32,
    textures: &'a HashMap<TextureId, *mut SDL_Texture>,
    xy: &'a mut Vec<f32>,
    colors: &'a mut Vec<SDL_FColor>,
    chain: &'a mut Chain,
    no_output_said: &'a mut bool,
    window_state: *mut SDL_GPURenderState,
}

impl Stage for SdlStage<'_> {
    fn size(&self) -> (i32, i32) {
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer and two live integers.
        unsafe { SDL_GetCurrentRenderOutputSize(self.renderer, &mut w, &mut h) };
        (w, h)
    }

    fn mesh(&mut self, clip: Rect, mesh: &egui::epaint::Mesh) {
        let ppp = self.ppp;
        let texture = match self.textures.get(&mesh.texture_id) {
            Some(&texture) => texture,
            None if mesh.texture_id == LIVE_BACKDROP => {
                let Some(output) = self.chain.output() else {
                    if !std::mem::replace(self.no_output_said, true) {
                        eprintln!("nightjar: glass sampled the live backdrop with no chain output: a surface without a boundary, or a chain that failed");
                    }
                    return;
                };
                output
            }
            None => return,
        };
        if mesh.indices.is_empty() {
            return;
        }
        let rect = SDL_Rect {
            x: (clip.min.x * ppp).round() as i32,
            y: (clip.min.y * ppp).round() as i32,
            w: ((clip.max.x - clip.min.x) * ppp).round() as i32,
            h: ((clip.max.y - clip.min.y) * ppp).round() as i32,
        };
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        self.xy.clear();
        self.colors.clear();
        for v in &mesh.vertices {
            self.xy.push(v.pos.x * ppp);
            self.xy.push(v.pos.y * ppp);
            let [r, g, b, a] = v.color.to_array();
            self.colors.push(SDL_FColor {
                r: f32::from(r) / 255.0,
                g: f32::from(g) / 255.0,
                b: f32::from(b) / 255.0,
                a: f32::from(a) / 255.0,
            });
        }
        let stride = std::mem::size_of::<egui::epaint::Vertex>() as i32;
        let uv = &mesh.vertices[0].uv.x as *const f32;
        // SAFETY: the arrays hold `vertices.len()` entries; uv points into the vertex
        // array with the vertex stride; indices are u32.
        unsafe {
            // Glass's own targets keep SDL's draw; only the window's target takes the state.
            let state = if SDL_GetRenderTarget(self.renderer).is_null() { self.window_state } else { std::ptr::null_mut() };
            if !state.is_null() {
                SDL_SetGPURenderState(self.renderer, state);
            }
            SDL_SetRenderClipRect(self.renderer, &rect);
            SDL_RenderGeometryRaw(
                self.renderer,
                texture,
                self.xy.as_ptr(),
                8,
                self.colors.as_ptr(),
                std::mem::size_of::<SDL_FColor>() as i32,
                uv,
                stride,
                mesh.vertices.len() as i32,
                mesh.indices.as_ptr().cast(),
                mesh.indices.len() as i32,
                4,
            );
            if !state.is_null() {
                SDL_SetGPURenderState(self.renderer, std::ptr::null_mut());
            }
        }
    }

    fn targets_work(&mut self) -> Result<(), String> {
        Chain::check(self.renderer)
    }

    fn begin_scene(&mut self) -> Result<(), String> {
        self.chain.begin(self.renderer)
    }

    fn end_scene(&mut self, timed: bool) -> Result<Option<f32>, String> {
        self.chain.run(self.renderer, timed)
    }

    fn release(&mut self) {
        self.chain.release();
    }
}
