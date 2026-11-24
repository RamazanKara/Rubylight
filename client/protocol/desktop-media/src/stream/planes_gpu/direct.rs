//! Direct back-buffer / swapchain draws, Frame Warp HUD mask upload, and shader uniforms for [`PlanesPass`].

use std::ptr;
use std::sync::Arc;

use sdl3_sys::everything::*;

use super::super::frame_warp;
use super::PlanesPass;

/// The mask texture's size: a row of the widest mask's bytes (`frame_warp::mask_bytes`), and
/// its rows.
pub(super) const MASK_ROW_BYTES: u32 = frame_warp::MASK_EDGE.div_ceil(32) as u32 * 4;
const MASK_ROWS: u32 = frame_warp::MASK_EDGE as u32;

impl PlanesPass {
    /// # Safety
    /// The pass's active render state must be live on the calling thread.
    pub(super) unsafe fn set_uniforms(&mut self, uniforms: &Uniforms) -> Result<(), String> {
        let state = self.active_state();
        if state.is_null() {
            return Err("no picture's planes to draw".into());
        }
        // SAFETY: `state` is a live SDL_GPURenderState on the calling thread and `uniforms` is valid.
        if !unsafe { SDL_SetGPURenderStateFragmentUniforms(state, 0, (uniforms as *const Uniforms).cast(), std::mem::size_of::<Uniforms>() as u32) } {
            return Err(format!("the plane constants cannot be set: {}", crate::sdl::error()));
        }
        Ok(())
    }

    /// Draws the copied planes, converted and scaled, into `destination` of the renderer's current
    /// target (the back buffer): the picture texture's pass and SDL's draw of it in one. `visible`
    /// is the picture's size in the planes; `look` is what SDL's draw would apply (`SdlLook`).
    /// Converting and scaling in the one draw that reaches the swapchain follows moonlight-qt's
    /// and chiaki-ng's Vulkan renderers.
    ///
    /// # Safety
    /// `renderer` must be live and the pass the one made on it, its planes copied for `parameters`.
    #[cfg(test)]
    pub unsafe fn draw_direct(&mut self, renderer: *mut SDL_Renderer, parameters: &[u32; 8], destination: &SDL_FRect, visible: (i32, i32), look: SdlLook) -> Result<(), String> {
        // SAFETY: Caller guarantees `renderer` is live and matches this pass.
        unsafe { self.draw_direct_warped(renderer, parameters, destination, visible, look, &frame_warp::WarpDraw::default()) }
    }

    /// `draw_direct` with the picture shifted by Frame Warp's `warp`, its HUD mask uploaded when
    /// it changed.
    ///
    /// # Safety
    /// As `draw_direct`.
    pub unsafe fn draw_direct_warped(&mut self, renderer: *mut SDL_Renderer, parameters: &[u32; 8], destination: &SDL_FRect, visible: (i32, i32), look: SdlLook,
        warp: &frame_warp::WarpDraw) -> Result<(), String> {
        let mut viewport = SDL_Rect::default();
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        unsafe {
            SDL_GetRenderViewport(renderer, &mut viewport);
            let mut uniforms = direct_uniforms(parameters, destination, (viewport.x, viewport.y), visible, look);
            if !warp.is_identity() {
                let mask = self.ready_mask(renderer, warp);
                uniforms.warp = warp.row(mask);
                uniforms.warp_lens = warp.lens_row();
            }
            self.set_uniforms(&uniforms)?;
            let drawn = SDL_SetGPURenderState(renderer, self.active_state()) && SDL_RenderFillRect(renderer, destination);
            let error = if drawn { String::new() } else { crate::sdl::error() };
            SDL_SetGPURenderState(renderer, ptr::null_mut());
            if drawn { Ok(()) } else { Err(format!("the planes cannot be drawn into the back buffer: {error}")) }
        }
    }

    /// Whether the mask texture holds `warp`'s HUD mask for the shader, uploading it when it
    /// changed. A mask that cannot be made or uploaded is said once and drawn as none from then
    /// on: the HUD moves with the picture.
    unsafe fn ready_mask(&mut self, renderer: *mut SDL_Renderer, warp: &frame_warp::WarpDraw) -> bool {
        let Some(hud) = warp.mask() else { return false };
        if self.mask_failed { return false; }
        if self.mask_made.as_ref().is_some_and(|made| Arc::ptr_eq(made, hud)) { return true; }
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        match unsafe { self.upload_mask(renderer, &hud.mask) } {
            Ok(()) => {
                self.mask_made = Some(hud.clone());
                true
            }
            Err(error) => {
                eprintln!("nightjar: Frame Warp: the HUD mask cannot reach the GPU ({error}); the HUD moves with the picture");
                self.mask_failed = true;
                false
            }
        }
    }

    /// Uploads `mask` (`frame_warp::mask_bytes`) into the top left of the mask texture, ahead of
    /// the renderer's next draws. The first upload makes the texture and binds it.
    unsafe fn upload_mask(&mut self, renderer: *mut SDL_Renderer, mask: &nightjar_core::frame_warp::Mask) -> Result<(), String> {
        let (row, bytes) = frame_warp::mask_bytes(mask);
        let rows = u32::from(mask.height);
        if bytes.is_empty() || row as u32 > MASK_ROW_BYTES || rows > MASK_ROWS {
            return Err(format!("a {}x{} mask is larger than its texture", mask.width, mask.height));
        }
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        unsafe {
            if self.mask.is_null() {
                self.mask = SDL_CreateGPUTexture(self.device, &SDL_GPUTextureCreateInfo {
                    r#type: SDL_GPU_TEXTURETYPE_2D, format: SDL_GPU_TEXTUREFORMAT_R8_UNORM, usage: SDL_GPU_TEXTUREUSAGE_SAMPLER,
                    width: MASK_ROW_BYTES, height: MASK_ROWS, layer_count_or_depth: 1, num_levels: 1, sample_count: SDL_GPU_SAMPLECOUNT_1, ..Default::default() });
                if self.mask.is_null() {
                    return Err(format!("its texture could not be made: {}", crate::sdl::error()));
                }
                self.rebind_mask().map_err(|error| format!("its texture cannot be bound: {error}"))?;
            }
            // The draws queued so far read the mask before this upload replaces it.
            if !SDL_FlushRenderer(renderer) {
                return Err(format!("the renderer cannot flush: {}", crate::sdl::error()));
            }
            let transfer = SDL_CreateGPUTransferBuffer(self.device, &SDL_GPUTransferBufferCreateInfo {
                usage: SDL_GPU_TRANSFERBUFFERUSAGE_UPLOAD, size: bytes.len() as u32, ..Default::default() });
            if transfer.is_null() {
                return Err(format!("no transfer buffer: {}", crate::sdl::error()));
            }
            let mapped = SDL_MapGPUTransferBuffer(self.device, transfer, false).cast::<u8>();
            let result = if mapped.is_null() {
                Err(format!("the transfer buffer cannot be mapped: {}", crate::sdl::error()))
            } else {
                ptr::copy_nonoverlapping(bytes.as_ptr(), mapped, bytes.len());
                SDL_UnmapGPUTransferBuffer(self.device, transfer);
                let commands = SDL_AcquireGPUCommandBuffer(self.device);
                if commands.is_null() {
                    Err(format!("no GPU command buffer: {}", crate::sdl::error()))
                } else {
                    let copy = SDL_BeginGPUCopyPass(commands);
                    SDL_UploadToGPUTexture(copy,
                        &SDL_GPUTextureTransferInfo { transfer_buffer: transfer, offset: 0, pixels_per_row: row as u32, rows_per_layer: rows },
                        &SDL_GPUTextureRegion { texture: self.mask, w: row as u32, h: rows, d: 1, ..Default::default() },
                        true);
                    SDL_EndGPUCopyPass(copy);
                    if SDL_SubmitGPUCommandBuffer(commands) { Ok(()) } else { Err(format!("the upload cannot be submitted: {}", crate::sdl::error())) }
                }
            };
            // SDL keeps the buffer until the GPU is done with it.
            SDL_ReleaseGPUTransferBuffer(self.device, transfer);
            result
        }
    }
}

/// The plane shader's constants (`Picture` in pyrowave.hlsl, Vulkan build): the pass parameters,
/// then what flag 512 reads.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uniforms {
    pub parameters: [u32; 8],
    pub destination: [f32; 4],
    pub picture: [f32; 4],
    pub output_scale: f32,
    pub tonemap: [f32; 2],
    pub unused: f32,
    /// What flag 8192 reads: the gamma 2.2 range's minimum and 1 / (peak - minimum), in nits.
    pub encoding: [f32; 4],
    /// What flag 512 reads of Frame Warp: `WarpDraw::row`; all zero draws the picture in place.
    pub warp: [f32; 4],
    /// Its lens terms (`WarpDraw::lens_row`), read only beside a row that shifts.
    pub warp_lens: [f32; 4],
}

/// The shader's flag for a draw straight into the target rectangle.
pub const DIRECT_FLAG: u32 = 512;
/// With `DIRECT_FLAG`: the target is sRGB, not scRGB, so the shader encodes and clamps as SDL does.
pub const SRGB_TARGET_FLAG: u32 = 1024;
/// With `DIRECT_FLAG`: the target is the HDR10 swapchain image of a video-only frame (the patched
/// SDL's `SDL.renderer.gpu.video_only`), so the shader finishes with the PQ present pass's encode
/// (`pq_present.hlsl`: BT.709 to BT.2020, 1.0 = 80 nits, ST 2084) of the scRGB it would write.
pub const PQ_TARGET_FLAG: u32 = 4096;
/// With `PQ_TARGET_FLAG`: the swapchain is drawn in the compositor's gamma 2.2 blending space, as
/// the scanout present pass would (`scanout_present.hlsl`, `wl_scanout::Encoding::Gamma22`).
pub const GAMMA22_TARGET_FLAG: u32 = 8192;

/// What SDL's draw of the RGBA64F linear picture texture into the back buffer does to it
/// (`texture_advanced.frag.hlsl`): its tone map, when the texture's HDR headroom is above the
/// output's, then its colour scale (the output's SDR white times the renderer's colour scale),
/// then on an sRGB output (an SDR display outside gamescope) its sRGB encoding and clamp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdlLook {
    pub texture_headroom: f32,
    pub output_headroom: f32,
    pub sdr_white: f32,
    pub color_scale: f32,
    /// The output is scRGB (`SDL_RenderingLinearSpace`), not sRGB.
    pub linear_output: bool,
    /// The frame is video only on a PQ output: the draw writes PQ, not scRGB (`PQ_TARGET_FLAG`).
    pub pq_target: bool,
    /// The encoding the renderer's present writes into its swapchain (`sdl::present_encoding`):
    /// mode 1 makes a `pq_target` draw gamma 2.2 instead (`GAMMA22_TARGET_FLAG`).
    pub target_encoding: [f32; 4],
}

impl SdlLook {
    /// What SDL's draw of `texture` into the renderer's current target (the back buffer, or a
    /// target texture) applies, read as `CalculateAdvancedShaderConstants` and
    /// `SDL_RenderingLinearSpace` read it.
    ///
    /// # Safety
    /// `renderer` and `texture` must be live, on the calling thread.
    pub unsafe fn of(renderer: *mut SDL_Renderer, texture: *mut SDL_Texture) -> Self {
        // SAFETY: Caller guarantees `renderer` and `texture` are live on the calling thread.
        unsafe {
            let target = SDL_GetRenderTarget(renderer);
            let (output, colorspace, sdr_white, output_headroom) = if target.is_null() {
                let props = SDL_GetRendererProperties(renderer);
                (props, SDL_GetNumberProperty(props, SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0),
                    SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT)
            } else {
                let props = SDL_GetTextureProperties(target);
                (props, SDL_GetNumberProperty(props, SDL_PROP_TEXTURE_COLORSPACE_NUMBER, 0),
                    SDL_PROP_TEXTURE_SDR_WHITE_POINT_FLOAT, SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT)
            };
            let mut color_scale = 1.0;
            SDL_GetRenderColorScale(renderer, &mut color_scale);
            Self {
                // SDL leaves a texture's headroom out when it is 0, its default for the linear format.
                texture_headroom: SDL_GetFloatProperty(SDL_GetTextureProperties(texture), SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT, 0.0),
                output_headroom: SDL_GetFloatProperty(output, output_headroom, 0.0),
                sdr_white: SDL_GetFloatProperty(output, sdr_white, 1.0),
                color_scale,
                linear_output: colorspace == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0),
                pq_target: false,
                target_encoding: crate::sdl::present_encoding(renderer),
            }
        }
    }

    /// SDL's TONEMAP_CHROME factors (`CalculateAdvancedShaderConstants`), None without a tone map.
    pub fn tonemap(&self) -> Option<(f32, f32)> {
        (self.texture_headroom > self.output_headroom && self.output_headroom > 0.0)
            .then(|| (self.output_headroom / (self.texture_headroom * self.texture_headroom), 1.0 / self.output_headroom))
    }
}

/// The constants of a `draw_direct` into `destination` (render coordinates) of a viewport at `origin`.
pub fn direct_uniforms(parameters: &[u32; 8], destination: &SDL_FRect, origin: (i32, i32), visible: (i32, i32), look: SdlLook) -> Uniforms {
    let mut parameters = *parameters;
    // A PQ target takes the scRGB a linear one would, encoded: the output colorspace stays scRGB.
    parameters[6] |= DIRECT_FLAG | if look.pq_target { PQ_TARGET_FLAG } else if look.linear_output { 0 } else { SRGB_TARGET_FLAG };
    // The swapchain the present would draw in gamma 2.2 gets the same encode from this draw.
    let [mode, min_nits, inv_range, _] = look.target_encoding;
    let gamma22 = look.pq_target && mode == 1.0 && inv_range > 0.0;
    if gamma22 {
        parameters[6] |= GAMMA22_TARGET_FLAG;
    }
    let (tonemap1, tonemap2) = look.tonemap().unwrap_or((0.0, 0.0));
    Uniforms {
        parameters,
        destination: [destination.x + origin.0 as f32, destination.y + origin.1 as f32, destination.w.max(1.0), destination.h.max(1.0)],
        picture: [visible.0 as f32, visible.1 as f32, 0.0, 0.0],
        output_scale: look.sdr_white * look.color_scale,
        tonemap: [tonemap1, tonemap2],
        unused: 0.0,
        encoding: if gamma22 { [min_nits, inv_range, 0.0, 0.0] } else { [0.0; 4] },
        warp: [0.0; 4],
        warp_lens: nightjar_core::frame_warp::FLAT,
    }
}
