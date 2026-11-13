//! Pictures the plane shader draws: the GPU decoder's planes (D3D11, Vulkan video), software-decoded
//! planar ones, and packed 4:4:4 rebuilt from planes.

use super::*;

impl VideoTexture {
    /// Makes the texture and the planes pass ready for pictures of `setup` and returns the pass
    /// parameters, or `None` (with `failure` set when it is a loud one).
    #[cfg(windows)]
    fn prepare_planes(&mut self, renderer: *mut SDL_Renderer, setup: nightjar_core::session::VideoSetup) -> Option<[u32; 8]> {
        let ten = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let hdr = self.hdr_mode.unwrap_or(ten);
        if hdr && !crate::sdl::hdr_available(renderer) { self.destroy(); return None; }
        // SAFETY: The live renderer belongs to the presenting thread.
        let white_level = if hdr { unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0) }.max(0.001) } else { 1.0 };
        let flags = u32::from(setup.color_range != 0) | (u32::from(hdr) << 1) | ((setup.color_space as u32) << 2)
            | (u32::from(ten) << 4) | (u32::from(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_YUV444 != 0) << 5);
        let parameters = [setup.width as u32, setup.height as u32, setup.width as u32, setup.height as u32, 0, 3, flags, white_level.to_bits()];
        if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
        if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, setup.width, setup.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return None; }
        if self.planes.is_none() {
            if self.d3d11.is_null() {
                self.failure = Some("PyroWave GPU decoding needs the Direct3D 11 renderer".into());
                return None;
            }
            // SAFETY: `d3d11` is the live texture of the renderer's device.
            match unsafe { super::super::d3d11::PlanesPass::new(self.d3d11) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.failure = Some(format!("PyroWave GPU decoding cannot draw its planes: {error}")); return None; }
            }
        }
        Some(parameters)
    }

    /// Draws a picture the GPU decoder left in D3D11 planes: waits for its decode on the GPU,
    /// converts the planes (only `rows` of them when given) into the texture and gives the plane
    /// set back.
    #[cfg(windows)]
    pub(super) fn upload_planes(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, lease: &super::super::gpu_planes::Lease, fence_value: u64, rows: Option<(u32, u32)>) -> bool {
        let Some(parameters) = self.prepare_planes(renderer, decoded.setup) else { return false };
        // The picture's set stays with the presenter while the planes wait for the back buffer.
        if self.goes_direct(renderer, rows.is_some()) {
            if let Some(held) = lease.hand_over() {
                self.pyrowave_stages.reset();
                self.hold_direct(DirectPlanes { source: PlanesHold::Lease { lease: held, fence_value }, parameters, in_texture: false, timed: false }, "PyroWave");
                return true;
            }
        }
        self.finish_direct();
        self.say_planes_path(false, "PyroWave");
        let step = std::time::Instant::now();
        let drawn = self.planes.as_mut().is_some_and(|pass| match pass.draw(lease, fence_value, &parameters, PLANES_DECODE_DEADLINE, rows) {
            Ok(()) => true,
            Err(error) => { self.failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if drawn {
            self.pyrowave_stages.add(super::super::pyrowave_gpu::PASSES, step);
            self.pyrowave_stages.commit();
        } else { self.pyrowave_stages.reset(); }
        drawn
    }

    /// Draws `rows` of a slice of the frame being received into the texture (the frame's plane
    /// set stays with it). Returns false when the slice cannot be drawn; `failure` says why when
    /// the stream must end.
    #[cfg(windows)]
    pub fn upload_slice(&mut self, renderer: *mut SDL_Renderer, setup: nightjar_core::session::VideoSetup, pin: &super::super::gpu_planes::Pin, fence_value: u64, rows: (u32, u32)) -> bool {
        let Some(parameters) = self.prepare_planes(renderer, setup) else { return false };
        // The rows of a frame being received go through the texture, which the frame's picture then is.
        self.finish_direct();
        let step = std::time::Instant::now();
        let drawn = self.planes.as_mut().is_some_and(|pass| match pass.draw_slice(pin, fence_value, &parameters, PLANES_DECODE_DEADLINE, rows) {
            Ok(()) => true,
            Err(error) => { self.failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if drawn { self.pyrowave_stages.add(super::super::pyrowave_gpu::PASSES, step); } else { self.pyrowave_stages.reset(); }
        drawn
    }

    /// Draws a GPU-decoded picture: from the decoder's images on the renderer's device, sampled in
    /// place (the pass holds the set until the next picture replaces it) or copied, or from CPU
    /// plane buffers, uploaded; converted into the texture, or at present time into the back
    /// buffer (`draw_direct`).
    #[cfg(unix)]
    pub(super) fn upload_planes(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, lease: &super::super::gpu_planes::Lease) -> bool {
        let setup = decoded.setup;
        let ten = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let hdr = self.hdr_mode.unwrap_or(ten);
        if hdr && !crate::sdl::hdr_available(renderer) { self.destroy(); return false; }
        // SAFETY: The live renderer belongs to the presenting thread.
        let white_level = if hdr { unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0) }.max(0.001) } else { 1.0 };
        let flags = u32::from(setup.color_range != 0) | (u32::from(hdr) << 1) | ((setup.color_space as u32) << 2)
            | (u32::from(ten) << 4) | (u32::from(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_YUV444 != 0) << 5);
        let parameters = [setup.width as u32, setup.height as u32, setup.width as u32, setup.height as u32, 0, 3, flags, white_level.to_bits()];
        if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
        if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, setup.width, setup.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return false; }
        // A pass made for the other way of reading the planes (another decoder's) goes; its held
        // set goes back to that decoder's pool.
        if self.planes.as_ref().is_some_and(|pass| !pass.suits(lease.planes())) { self.planes = None; }
        if self.planes.is_none() {
            // SAFETY: The live renderer belongs to the presenting thread.
            match unsafe { super::super::planes_gpu::PlanesPass::for_planes(renderer, lease.planes()) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.failure = Some(format!("PyroWave GPU decoding cannot draw its planes: {error}")); return false; }
            }
        }
        let texture = self.planes_target();
        let stages = &mut self.pyrowave_stages;
        let failure = &mut self.failure;
        let drawn = self.planes.as_mut().is_some_and(|pass| match unsafe { pass.draw(renderer, texture, lease, &parameters, stages) } {
            Ok(()) => true,
            Err(error) => { *failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if !drawn {
            self.pyrowave_stages.reset();
        } else if texture.is_null() {
            // The conversion runs at the present (`passes_drawn`).
            self.pyrowave_stages_open = true;
        } else {
            self.pyrowave_stages.commit();
        }
        if drawn { self.planes_drawn(texture, parameters); }
        drawn
    }

    /// Draws a D3D11VA 4:4:4 picture (`media::FRAME_D3D11_444`) into the RGBA64F texture as a
    /// software-decoded 4:4:4 one is drawn (same constants: range, matrix, 10-bit SDR, HDR10, HLG):
    /// the plane pass copies the decoder's surface and its surface entry converts it.
    #[cfg(windows)]
    pub(super) fn upload_d3d11_444(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, white_level: f32) -> bool {
        let Some(layout) = media::PlanarLayout::of_d3d11(info) else {
            self.fail(format!("a {}x{} 4:4:4 decoder picture has no size", info.width, info.height), "the 4:4:4 picture cannot be drawn".into());
            return false;
        };
        let Some(parameters) = self.planar_target(renderer, info, layout, white_level, false) else { return false };
        if self.planes.is_none() {
            if self.d3d11.is_null() {
                self.fail(format!("the RGBA64F video texture has no Direct3D 11 texture (renderer {})", crate::sdl::renderer_name(renderer)), "it needs the Direct3D 11 renderer".into());
                return false;
            }
            // SAFETY: `d3d11` is the live texture of the renderer's device.
            match unsafe { super::super::d3d11::PlanesPass::new(self.d3d11) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
            }
        }
        if self.goes_direct(renderer, false) {
            // A readable decoder texture is read in place, its frame held until the next picture.
            let bound = self.planes.as_mut().unwrap().bind_surface(frame, info, &parameters, true);
            match bound {
                Ok(in_place) => {
                    let held = if in_place { frame.reference() } else { None };
                    let held = if in_place && held.is_none() {
                        // No second reference to the frame: the slice is copied instead.
                        match self.planes.as_mut().unwrap().bind_surface(frame, info, &parameters, false) {
                            Ok(_) => None,
                            Err(error) => { self.fail(format!("binding a {}x{} 4:4:4 decoder picture failed: {error}", layout.width, layout.height), error); return false; }
                        }
                    } else { held };
                    self.hold_direct(DirectPlanes { source: PlanesHold::Surface { held }, parameters, in_texture: false, timed: false }, "4:4:4");
                    return true;
                }
                Err(error) => { self.fail(format!("binding a {}x{} 4:4:4 decoder picture failed: {error}", layout.width, layout.height), error); return false; }
            }
        }
        self.finish_direct();
        self.say_planes_path(false, "4:4:4");
        match self.planes.as_mut().unwrap().draw_surface(frame, info, &parameters) {
            Ok(()) => true,
            Err(error) => { self.fail(format!("drawing a {}x{} 4:4:4 decoder picture failed: {error}", layout.width, layout.height), error); false }
        }
    }

    /// Draws a software-decoded planar picture (4:4:4, or 10-bit) with the plane shader into an
    /// RGBA64F texture, as the GPU decoder's planes are drawn: SDL has no texture formats for them.
    pub(super) fn upload_planar(&mut self, renderer: *mut SDL_Renderer, info: &FrameInfo, layout: media::PlanarLayout, white_level: f32) -> bool {
        let Some(parameters) = self.planar_target(renderer, info, layout, white_level, false) else { return false };
        #[cfg(windows)]
        let drawn = {
            if self.planes.is_none() {
                if self.d3d11.is_null() {
                    self.fail(format!("the RGBA64F video texture has no Direct3D 11 texture (renderer {})", crate::sdl::renderer_name(renderer)), "it needs the Direct3D 11 renderer".into());
                    return false;
                }
                // SAFETY: `d3d11` is the live texture of the renderer's device.
                match unsafe { super::super::d3d11::PlanesPass::new(self.d3d11) } {
                    Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                    Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
                }
            }
            if self.goes_direct(renderer, false) {
                match self.planes.as_mut().unwrap().upload_frame(info) {
                    Ok(()) => {
                        self.hold_direct(DirectPlanes { source: PlanesHold::Frame, parameters, in_texture: false, timed: false }, "planar");
                        return true;
                    }
                    Err(error) => Err(error),
                }
            } else {
                self.finish_direct();
                self.say_planes_path(false, "planar");
                self.planes.as_mut().unwrap().draw_frame(info, &parameters)
            }
        };
        #[cfg(unix)]
        let drawn = {
            if self.planes.is_none() {
                // SAFETY: The live renderer belongs to the presenting thread.
                match unsafe { super::super::planes_gpu::PlanesPass::for_frames(renderer, layout) } {
                    Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                    Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
                }
            }
            let texture = self.planes_target();
            // SAFETY: The pass was made on this renderer for pictures shaped like `info` (a change
            // of shape changes the parameters, which destroys it), and `info` is the live frame's.
            let drawn = unsafe { self.planes.as_mut().unwrap().draw_frame(renderer, texture, info, &parameters) };
            if drawn.is_ok() { self.planes_drawn(texture, parameters); }
            drawn
        };
        #[cfg(not(any(windows, unix)))]
        let drawn: Result<(), String> = Err("this platform has no plane shader pass".into());
        match drawn {
            Ok(()) => true,
            Err(error) => { self.fail(format!("drawing a {}x{} planar picture failed: {error}", layout.width, layout.height), error); false }
        }
    }

    /// The plane shader's constants for `info` drawn as `layout`, with the RGBA64F texture made for
    /// them (and a pass made for other constants gone); None, the reason said, when the picture
    /// cannot be drawn. `vulkan`: from Vulkan video's copied planes (flag 256 in place of 64).
    fn planar_target(&mut self, renderer: *mut SDL_Renderer, info: &FrameInfo, layout: media::PlanarLayout, white_level: f32, vulkan: bool) -> Option<[u32; 8]> {
        let hdr = shows_hdr(self.hdr_mode, self.hdr_stream, info.color_trc);
        // An HLG picture needs no HDR output: on an SDR one it is drawn for HLG's reference
        // display and SDL's tone map brings it into SDR (`planar_hlg`).
        let hdr_output = crate::sdl::hdr_available(renderer);
        if hdr && info.color_trc != TRC_HLG && !hdr_output {
            self.destroy();
            self.fail(format!("an HDR {}x{} picture needs HDR output, which the renderer {} does not offer", layout.width, layout.height, crate::sdl::renderer_name(renderer)), "the display has no HDR output".into());
            return None;
        }
        // SAFETY: The live renderer belongs to the presenting thread.
        let headroom = if hdr_output { unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0) } } else { 1.0 };
        let hlg = planar_hlg(info, hdr, white_level, headroom);
        let parameters = interleaved_flags(vulkan_flags(planar_parameters(info, layout, hdr, white_level, headroom), vulkan), vulkan && media::interleaved_444(info));
        if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
        // The HLG picture's own peak over its white, which SDL tone maps down to an output with less.
        let texture_headroom = hlg.map(|display| display.peak / display.white);
        self.ensure_with(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, info.width, info.height, SDL_COLORSPACE_SRGB_LINEAR, white_level, texture_headroom, std::ptr::null_mut()).then_some(parameters)
    }

    /// Draws a Vulkan video picture (`crate::vulkan_video`) as a planar picture of its depth and
    /// chroma: its two planes are copied on the GPU into the presenter's own images (which hands
    /// the picture back to the decoder), and the plane shader draws them with the colour handling
    /// of software-decoded pictures (range, matrix, 10-bit SDR, HDR10, HLG). Nothing goes through
    /// memory.
    #[cfg(unix)]
    pub(super) fn upload_vulkan(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, layout: media::PlanarLayout, white_level: f32) -> bool {
        let Some(parameters) = self.planar_target(renderer, info, layout, white_level, true) else { return false };
        let what = || format!("{}x{} {}-bit Vulkan video picture", layout.width, layout.height, if layout.ten_bit { 10 } else { 8 });
        if self.vulkan.is_none() {
            let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
            // SAFETY: The renderer's live GPU device, which outlives the texture.
            let made = if media::interleaved_444(info) {
                unsafe { crate::vulkan_video::Planes::interleaved(device.cast(), layout.width, layout.height, layout.ten_bit) }
            } else {
                unsafe { crate::vulkan_video::Planes::new(device.cast(), layout.width, layout.height, layout.ten_bit, layout.chroma444) }
            };
            match made {
                Ok(planes) => self.vulkan = Some(planes),
                Err(error) => { self.fail(format!("the planes of a {} cannot be made: {error}", what()), error); return false; }
            }
        }
        if self.planes.is_none() {
            let textures = self.vulkan.as_ref().map_or([std::ptr::null_mut(); 2], |planes| planes.textures());
            // SAFETY: The live renderer of the presenting thread; the textures live in `vulkan`,
            // which is dropped after the pass.
            match unsafe { super::super::planes_gpu::PlanesPass::for_vulkan(renderer, textures) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
            }
        }
        let texture = self.planes_target();
        let (Some(planes), Some(pass)) = (self.vulkan.as_mut(), self.planes.as_mut()) else { return false };
        // SAFETY: A live Vulkan frame of the decoder on this renderer's device (`device_context`);
        // planes and pass made on this renderer for pictures shaped like it (a change of shape
        // changes the parameters, which destroys them).
        let drawn = unsafe {
            // A VA-API picture is sampled where VA-API wrote it when it can be (the route is logged
            // once); any other, or a surface the driver does not sample, is copied.
            #[cfg(target_os = "linux")]
            let sampled = planes.sample(renderer, frame.raw.as_ptr());
            #[cfg(not(target_os = "linux"))]
            let sampled: Result<Option<[*mut SDL_GPUTexture; 2]>, String> = Ok(None);
            match sampled {
                Ok(Some(textures)) => pass.draw_sampled_textures(renderer, texture, textures, &parameters),
                Ok(None) => {
                    pass.use_copy();
                    planes.copy(renderer, frame.raw.as_ptr()).and_then(|()| pass.draw_bound(renderer, texture, &parameters))
                }
                Err(error) => Err(error),
            }
        };
        match drawn {
            Ok(()) => { self.planes_drawn(texture, parameters); true }
            Err(error) => { self.fail(format!("drawing a {} failed: {error}", what()), error); false }
        }
    }

    /// The packed 4:4:4 counterpart of `upload_vulkan`: a packed picture mapped into Vulkan (or
    /// Vulkan video's) is copied into planes of its coded size and rebuilt from them into the
    /// target; nothing goes through memory (#80).
    #[cfg(unix)]
    pub(super) fn draw_packed_planes(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, coded: media::PlanarLayout, parameters: &[u32; 12]) -> bool {
        let what = || format!("{}x{} {}-bit packed full chroma Vulkan picture", coded.width, coded.height, if coded.ten_bit { 10 } else { 8 });
        if self.vulkan.is_none() {
            let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
            // SAFETY: The renderer's live GPU device, which outlives the planes.
            match unsafe { crate::vulkan_video::Planes::new(device.cast(), coded.width, coded.height, coded.ten_bit, coded.chroma444) } {
                Ok(planes) => self.vulkan = Some(planes),
                Err(error) => { self.fail(format!("the planes of a {} cannot be made: {error}", what()), error); return false; }
            }
        }
        if self.packed.is_none() {
            let textures = self.vulkan.as_ref().map_or([std::ptr::null_mut(); 2], |planes| planes.textures());
            // SAFETY: The live renderer of the presenting thread; the textures live in `vulkan`,
            // which `destroy` drops with the reconstruction.
            match unsafe { super::super::packed444::Reconstruction::for_planes(renderer, textures) } {
                Ok(packed) => { self.packed = Some(packed); self.packed_format = Some(*parameters); }
                Err(error) => { self.fail(format!("the packed plane pass cannot be made: {error}"), error); return false; }
            }
        }
        let texture = self.texture;
        let (Some(planes), Some(packed)) = (self.vulkan.as_mut(), self.packed.as_ref()) else { return false };
        // SAFETY: A live Vulkan frame of the decoder on this renderer's device; planes made on this
        // renderer for pictures of its size (another size changes the parameters, which destroys them).
        let drawn = unsafe { planes.copy(renderer, frame.raw.as_ptr()) }.and_then(|()| {
            if packed.draw_planes(renderer, texture, parameters) { Ok(()) } else { Err(crate::sdl::error()) }
        });
        match drawn {
            Ok(()) => true,
            Err(error) => { self.fail(format!("drawing a {} failed: {error}", what()), error); false }
        }
    }

    /// A packed picture was rebuilt from Vulkan planes (`draw_packed_planes`), not from memory.
    #[cfg(all(test, unix))]
    pub(in super::super) fn packed_from_planes(&self) -> bool {
        self.packed.is_some() && self.vulkan.is_some()
    }
}
