//! Planes that skip the video texture: a picture's planes wait for the back buffer and are drawn into
//! it converted and scaled in one pass, or into the texture for what reads it.

use super::*;

impl VideoTexture {
    /// Where the plane pass draws a picture: nowhere yet while the planes can go straight into the
    /// back buffer (`draw_direct`), else the texture.
    #[cfg(unix)]
    pub(super) fn planes_target(&self) -> *mut SDL_Texture {
        if self.direct_planes_failed { self.texture } else { std::ptr::null_mut() }
    }

    /// Notes a picture the plane pass drew into `target` with `parameters`.
    #[cfg(unix)]
    pub(super) fn planes_drawn(&mut self, target: *mut SDL_Texture, parameters: [u32; 8]) {
        self.planes_pending = target.is_null().then_some(parameters);
    }

    /// Draws the copied planes into the texture, once per picture.
    #[cfg(unix)]
    pub(super) fn draw_pending_planes(&mut self, renderer: *mut SDL_Renderer) -> Result<(), String> {
        let Some(parameters) = self.planes_pending.take() else { return Ok(()) };
        let step = std::time::Instant::now();
        let texture = self.texture;
        let pass = self.planes.as_mut().ok_or("the plane pass is gone")?;
        // SAFETY: The pass copied the current picture's planes on this renderer, for `parameters`.
        unsafe { pass.draw_bound(renderer, texture, &parameters) }?;
        self.passes_drawn(step);
        Ok(())
    }

    /// The deferred draw of a PyroWave picture's planes goes to its "passes" stage, once, and
    /// completes the picture's stage times (`upload_planes` leaves them open).
    #[cfg(unix)]
    pub(super) fn passes_drawn(&mut self, step: std::time::Instant) {
        if std::mem::take(&mut self.pyrowave_stages_open) {
            self.pyrowave_stages.add(super::super::pyrowave_gpu::PASSES, step);
            self.pyrowave_stages.commit();
        }
    }

    /// Whether `draw_direct` will draw the current picture with the plane pass (unless the pass
    /// fails then): its planes are pending, the pass is there and has not failed.
    #[cfg(target_os = "linux")]
    pub fn draws_planes_direct(&self) -> bool {
        self.planes_pending.is_some() && self.planes.is_some() && !self.texture.is_null() && !self.direct_planes_failed
    }

    /// Whether SDL settled this frame as video only: `draw_direct` then writes PQ (`VideoOnly`).
    #[cfg(target_os = "linux")]
    pub fn set_pq_target(&mut self, pq_target: bool) {
        self.pq_target = pq_target;
    }

    #[cfg(target_os = "linux")]
    pub fn video_only(&mut self) -> &mut VideoOnly {
        &mut self.video_only
    }

    /// Whether the overlay, a toast and the stream menu can be drawn into an HDR10 swapchain
    /// image (`ui_pq::UiPq`), so a frame with them can be video only too. Makes the render state
    /// the first time; a failure is logged once and holds for the stream.
    ///
    /// # Safety
    /// `renderer` is the live renderer of the presenting thread.
    #[cfg(target_os = "linux")]
    pub unsafe fn ui_draws_pq(&mut self, renderer: *mut SDL_Renderer) -> bool {
        if self.ui_pq.is_none() && !self.ui_pq_failed {
            // SAFETY: Caller guarantees `renderer` is the live renderer of the presenting thread.
            match unsafe { super::super::ui_pq::UiPq::new(renderer) } {
                Ok(ui) => self.ui_pq = Some(ui),
                Err(error) => {
                    self.ui_pq_failed = true;
                    eprintln!("nightjar: HDR frames with the overlay or the menu go through the back buffer and its PQ pass: {error}");
                }
            }
        }
        self.ui_pq.is_some()
    }

    /// The render state for egui's meshes in this frame: null unless SDL settled it as video
    /// only, where SDL's own draws would be dropped.
    ///
    /// # Safety
    /// `renderer` is the live renderer of the presenting thread.
    #[cfg(target_os = "linux")]
    pub unsafe fn ui_pq_state(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_GPURenderState {
        match self.ui_pq.as_mut().filter(|_| self.pq_target) {
            // SAFETY: Caller guarantees `renderer` is the live renderer of the presenting thread.
            Some(ui) => unsafe { ui.state(renderer) },
            None => std::ptr::null_mut(),
        }
    }

    /// Draws the current picture's copied planes straight into `destination` of the back buffer,
    /// converted and scaled in one pass, and returns true; false for SDL to draw `texture` (an
    /// upscaler's output, a picture that is not planes, or after a failure, which is logged once).
    /// The pass does what SDL's draw of the RGBA64F texture would: its tone map and colour scale.
    #[cfg(unix)]
    pub fn draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        let Some(parameters) = self.planes_pending.filter(|_| texture == self.texture && !texture.is_null()) else { return false };
        // As on Windows (issue 120): a scaled picture is converted into the texture first and
        // SDL scales it in linear light; the pass would filter its colour codes.
        if !super::draws_one_to_one(destination, self.size()) {
            if !std::mem::replace(&mut self.direct_planes_scaled_said, true) {
                eprintln!("nightjar: the stream's pictures are drawn through the video texture: the display does not place them one texel to one pixel, the only way the plane pass draws them exactly (scaling their colour codes instead of the converted picture shifts the levels)");
            }
            return false;
        }
        let step = std::time::Instant::now();
        // SAFETY: The live renderer and texture of the presenting thread.
        let look = unsafe { super::super::planes_gpu::SdlLook::of(renderer, texture) };
        #[cfg(target_os = "linux")]
        let look = super::super::planes_gpu::SdlLook { pq_target: self.pq_target, ..look };
        let visible = self.size();
        let warp = unix_direct_warp(&self.warp, visible);
        let Some(pass) = self.planes.as_mut() else { return false };
        // SAFETY: The pass copied the current picture's planes on this renderer, for `parameters`.
        match unsafe { pass.draw_direct_warped(renderer, &parameters, destination, visible, look, &warp) } {
            Ok(()) => {
                self.passes_drawn(step);
                true
            }
            Err(error) => {
                eprintln!("nightjar: drawing the picture's planes into the back buffer failed: {error}; they go through the video texture from now on");
                self.direct_planes_failed = true;
                false
            }
        }
    }

    /// Whether the picture being uploaded waits for the back buffer (`direct_planes::goes_direct`).
    #[cfg(windows)]
    pub(super) fn goes_direct(&self, renderer: *mut SDL_Renderer, partial: bool) -> bool {
        super::super::direct_planes::goes_direct(super::super::direct_planes::Conditions {
            switch: super::super::direct_planes::enabled(),
            failed: self.direct_planes_failed,
            swap_chain: !self.d3d11.is_null() && !crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER).is_null(),
            partial,
        })
    }

    /// Says once which way the stream's `what` pictures are drawn.
    #[cfg(windows)]
    pub(super) fn say_planes_path(&mut self, direct: bool, what: &str) {
        if std::mem::replace(&mut self.direct_planes_said, true) { return; }
        if direct {
            eprintln!("nightjar: {what} pictures are converted and scaled straight into the back buffer in one pass ({}=0 draws them through the video texture)", super::super::direct_planes::SWITCH);
        } else if !super::super::direct_planes::enabled() {
            eprintln!("nightjar: {what} pictures are drawn through the video texture ({}=0)", super::super::direct_planes::SWITCH);
        } else {
            eprintln!("nightjar: {what} pictures are drawn through the video texture");
        }
    }

    /// Makes `picture` the current one, waiting for the back buffer; the one it replaces goes back.
    /// `draw_planes_direct` says which way it is drawn, once it knows the display's scale.
    #[cfg(windows)]
    pub(super) fn hold_direct(&mut self, picture: DirectPlanes) {
        self.finish_direct();
        self.direct_planes = Some(picture);
    }

    /// Lets go of the current picture's planes: a GPU decoder's plane set goes back to the decoder
    /// behind the last draw of it.
    #[cfg(windows)]
    pub(super) fn finish_direct(&mut self) {
        let Some(DirectPlanes { source: PlanesHold::Lease { lease, .. }, .. }) = self.direct_planes.take() else { return };
        if let Some(pass) = &self.planes
            && let Err(error) = pass.give_back(&lease) {
                eprintln!("nightjar: {error}");
            }
    }

    /// Draws the current picture's waiting planes into the video texture, once, for what reads
    /// the texture (an upscaler, AI enhancement, or SDL after a failed or scaled direct draw).
    #[cfg(windows)]
    pub(super) fn materialize_planes(&mut self) {
        let Some(mut picture) = self.direct_planes.take() else { return };
        if !picture.in_texture {
            let parameters = picture.parameters;
            let step = std::time::Instant::now();
            let drawn = match (&picture.source, self.planes.as_mut()) {
                (_, None) => Err("the plane pass is gone".to_string()),
                (PlanesHold::Lease { lease, fence_value }, Some(pass)) => pass.draw_held(lease, *fence_value, &parameters, PLANES_DECODE_DEADLINE),
                (PlanesHold::Frame, Some(pass)) => pass.draw_uploaded_frame(&parameters),
                (PlanesHold::Surface { held, .. }, Some(pass)) => pass.draw_bound_surface(&mut |submit| match held {
                    Some(frame) => frame.with_decoder_lock(submit),
                    None => submit(),
                }),
            };
            match drawn {
                Ok(()) => {
                    picture.in_texture = true;
                    // The conversion is the picture's "passes" stage, as when `upload_planes` draws
                    // it there for a stream that does not go direct.
                    if matches!(picture.source, PlanesHold::Lease { .. }) {
                        self.pyrowave_stages.add(super::super::pyrowave_gpu::PASSES, step);
                        self.pyrowave_stages.commit();
                    }
                }
                Err(error) => self.fail(format!("drawing the picture's planes into the video texture failed: {error}"), error),
            }
        }
        self.direct_planes = Some(picture);
    }

    /// Draws the current picture's planes into `destination` of the back buffer, converted and
    /// scaled in one pass, and returns true; false for SDL to draw `texture` (an upscaler's output,
    /// a picture the display scales, or after a failure, which is logged once and keeps the texture
    /// for the stream).
    #[cfg(windows)]
    fn draw_planes_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        if texture.is_null() || texture != self.texture { return false; }
        let what = self.direct_planes.as_ref().map_or("", |picture| picture.what);
        // Only a picture that lands on the output one texel to one pixel is drawn here: the pass
        // filters the picture's colour codes where SDL filters the converted, linear picture, so a
        // scaled picture would come out dark or bright by the scale (issue 120). It goes through
        // the video texture instead, which `render_flip` draws with SDL.
        if !super::super::direct_planes::draws_one_to_one(destination, self.size()) {
            if !std::mem::replace(&mut self.direct_planes_scaled_said, true) {
                eprintln!("nightjar: {what} pictures are drawn through the video texture: the display does not place them one texel to one pixel, the only way the plane pass draws them exactly (scaling their colour codes instead of the converted picture shifts the levels)");
            }
            return false;
        }
        let step = std::time::Instant::now();
        let swap_chain = crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER);
        if swap_chain.is_null() { return false; }
        let mut output = (0, 0);
        // SAFETY: The live renderer and texture of the presenting thread.
        let (look, flushed) = unsafe {
            SDL_GetCurrentRenderOutputSize(renderer, &mut output.0, &mut output.1);
            let look = super::super::direct_planes::Look::of(renderer, texture);
            // A new output size is resized by SDL's next run of its queue, this flush: let go first.
            if let Some(pass) = &mut self.planes { pass.keep_back_buffer_if(swap_chain as usize, output); }
            (look, SDL_FlushRenderer(renderer))
        };
        let visible = self.size();
        let mut constants = super::super::direct_planes::constants(destination, visible, look);
        let viewport = {
            let x = destination.x.max(0.0);
            let y = destination.y.max(0.0);
            [x, y, (destination.x + destination.w).min(output.0 as f32) - x, (destination.y + destination.h).min(output.1 as f32) - y]
        };
        let (Some(pass), Some(picture)) = (self.planes.as_mut(), self.direct_planes.as_ref()) else { return false };
        if !self.warp.is_identity() {
            // The picture is drawn at its own size, so its shift goes in whole pixels
            // (`packed444.hlsl` rounds it the same way): a fractional one would filter the codes
            // this pass converts afterwards.
            let mut warp = self.warp.clone();
            warp.shift = super::whole_pixel_shift(warp.shift, visible);
            constants.warp = pass.warp_row(&warp);
            constants.warp_lens = warp.lens_row();
            constants.warp_v2 = warp.v2_row();
        }
        let drawn = if !flushed {
            Err(format!("SDL_FlushRenderer: {}", crate::sdl::error()))
        } else {
            // SAFETY: The renderer's swap chain, on the device the pass was made on.
            unsafe { pass.back_buffer(swap_chain, output) }.and_then(|target| match &picture.source {
                PlanesHold::Lease { lease, fence_value } => {
                    let source = super::super::d3d11::DirectSource::Lease { lease, fence_value: *fence_value, deadline: PLANES_DECODE_DEADLINE };
                    pass.draw_direct(&target, viewport, &source, &picture.parameters, &constants, &mut |submit| submit())
                }
                PlanesHold::Frame => pass.draw_direct(&target, viewport, &super::super::d3d11::DirectSource::Frame, &picture.parameters, &constants, &mut |submit| submit()),
                PlanesHold::Surface { held, .. } => pass.draw_direct(&target, viewport, &super::super::d3d11::DirectSource::Surface, &picture.parameters, &constants, &mut |submit| match held {
                    Some(frame) => frame.with_decoder_lock(submit),
                    None => submit(),
                }),
            })
        };
        match drawn {
            Ok(()) => {
                self.say_planes_path(true, what);
                if let Some(picture) = self.direct_planes.as_mut()
                    && matches!(picture.source, PlanesHold::Lease { .. }) && !std::mem::replace(&mut picture.timed, true) {
                        self.pyrowave_stages.add(super::super::pyrowave_gpu::PASSES, step);
                        self.pyrowave_stages.commit();
                    }
                true
            }
            Err(error) => {
                eprintln!("nightjar: drawing the picture's planes into the back buffer failed: {error}; they go through the video texture from now on");
                self.direct_planes_failed = true;
                false
            }
        }
    }

    #[cfg(windows)]
    pub fn draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        if self.direct_planes.is_some() {
            let drawn = self.draw_planes_direct(renderer, destination, texture);
            if !drawn {
                // SDL may resize the back buffer before the pass draws again.
                if let Some(pass) = &mut self.planes { pass.release_back_buffer(); }
            }
            return drawn;
        }
        if let Some(pass) = &mut self.planes { pass.release_back_buffer(); }
        let drawn = self.try_draw_direct(renderer, destination, texture);
        if !drawn {
            // SDL may resize the back buffer before the pass draws again.
            if let Some((_, pass)) = &mut self.direct { pass.release_back_buffer(); }
        }
        drawn
    }

    #[cfg(windows)]
    fn try_draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        if self.direct_failed || texture.is_null() || !super::super::direct::enabled() {
            return false;
        }
        let Some(mut picture) = self.picture() else { return false };
        if texture != self.texture {
            // AMF's HQ Scaler output: the same picture as NV12 or P010 at the output size.
            // SAFETY: A live texture of this renderer.
            let (format, w, h) = unsafe { ((*texture).format, (*texture).w, (*texture).h) };
            if format != self.format {
                return false;
            }
            // SAFETY: A live texture of this renderer.
            picture.texture = unsafe { SDL_GetPointerProperty(SDL_GetTextureProperties(texture), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, std::ptr::null_mut()) };
            if picture.texture.is_null() {
                return false;
            }
            picture.slice = 0;
            picture.size = (w as u32, h as u32);
        }
        let metadata = self.hdr_metadata.filter(|_| self.hdr_mode == Some(true));
        let white_nits = 80.0 * self.white_level;
        let tone = super::super::direct::Tone {
            white_nits,
            dolby_vision: self.dolby_vision,
            hdr10_plus: self.hdr10_plus,
            static_peak: hdr_properties(true, self.white_level, metadata).headroom.map(|headroom| headroom * white_nits),
        };
        let drawn = direct_pass(&mut self.direct, renderer).and_then(|pass| {
            let held = &self.held;
            if let Some(frame) = held
                && frame.d3d11_surface().is_some_and(|s| s.texture == picture.texture)
            {
                frame.wait_d3d11_ready()?;
            }
            // SAFETY: The picture's texture and the renderer belong to the pass's device.
            unsafe { pass.present(renderer, destination, &picture, tone, &self.warp, |submit| lock_for(held, picture.texture, submit)) }
        });
        match drawn {
            Ok(()) => true,
            Err(error) => {
                eprintln!("nightjar: drawing the decoder's pictures directly failed: {error}; SDL draws them from now on");
                self.direct_failed = true;
                self.direct = None;
                false
            }
        }
    }

    /// Waits on the decode fence when `self.held` backs `texture`.
    #[cfg(windows)]
    pub(super) fn wait_held_d3d11(&self, texture: *mut c_void) -> Result<(), String> {
        if let Some(frame) = &self.held
            && frame.d3d11_surface().is_some_and(|s| s.texture == texture)
        {
            frame.wait_d3d11_ready()?;
        }
        Ok(())
    }

    /// Marks held bridged D3D11 frames as flushed after `Present` submits the renderer queue.
    pub fn mark_d3d11_flushed(&self) {
        #[cfg(windows)]
        {
            if let Some(frame) = &self.held {
                frame.mark_d3d11_flushed();
            }
            if let Some(DirectPlanes { source: PlanesHold::Surface { held: Some(frame) }, .. }) = &self.direct_planes {
                frame.mark_d3d11_flushed();
            }
        }
    }
}

/// Frame Warp for the plane pass's draw of a picture at its own size (`draw_direct`): its shift in
/// whole pixels, as on Windows (`draw_planes_direct`), since a fractional one would filter the
/// codes the pass converts afterwards.
#[cfg(unix)]
pub(super) fn unix_direct_warp(warp: &super::super::frame_warp::WarpDraw, visible: (i32, i32)) -> super::super::frame_warp::WarpDraw {
    let mut warp = warp.clone();
    if !warp.is_identity() {
        warp.shift = super::whole_pixel_shift(warp.shift, visible);
    }
    warp
}
