//! Getting a decoded picture into the video texture: making the texture for the picture's format, and
//! the upload, import or GPU copy each decoder path needs.

use super::*;

#[cfg(target_os = "macos")]
#[path = "macos_cache.rs"]
mod macos_cache;

/// Borrowed or movable picture source for `VideoTexture::upload` / `upload_take`.
pub(super) enum FrameSource<'a> {
    Ref(&'a media::Frame),
    Owned(&'a mut media::Picture),
}

impl FrameSource<'_> {
    pub(super) fn frame(&self) -> &media::Frame {
        match self {
            Self::Ref(frame) => frame,
            Self::Owned(picture) => picture.frame().expect("checked Frame variant"),
        }
    }

    /// Moves the frame out when owned (zero `av_frame_clone`), else clones a reference.
    pub(super) fn take_or_ref(&mut self) -> Option<media::Frame> {
        match self {
            Self::Ref(frame) => frame.reference(),
            Self::Owned(picture) => picture.take_frame(),
        }
    }

    /// Moves the frame out when owned, without cloning a borrowed reference.
    #[cfg(windows)]
    pub(super) fn take_owned(&mut self) -> Option<media::Frame> {
        match self {
            Self::Ref(_) => None,
            Self::Owned(picture) => picture.take_frame(),
        }
    }
}

impl VideoTexture {
    pub(super) fn ensure(
        &mut self,
        renderer: *mut SDL_Renderer,
        format: SDL_PixelFormat,
        width: i32,
        height: i32,
        space: SDL_Colorspace,
        white_level: f32,
    ) -> bool {
        self.ensure_with(renderer, format, width, height, space, white_level, None, std::ptr::null_mut())
    }

    /// `ensure` for a texture with an HDR headroom of its own (an HLG picture's), which SDL tone
    /// maps down to an output with less.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ensure_with(
        &mut self,
        renderer: *mut SDL_Renderer,
        format: SDL_PixelFormat,
        width: i32,
        height: i32,
        space: SDL_Colorspace,
        white_level: f32,
        texture_headroom: Option<f32>,
        pixel_buffer: *mut c_void,
    ) -> bool {
        // Each VideoToolbox picture has its own backing. A later CPU upload must also
        // replace an imported/pooled texture before writing to it.
        #[cfg(target_os = "macos")]
        let reusable = pixel_buffer.is_null() && self.held.is_none() && self.importer.is_none();
        #[cfg(not(target_os = "macos"))]
        let reusable = pixel_buffer.is_null();
        if reusable && !self.texture.is_null()
            && self.format == format
            && self.width == width
            && self.height == height
            && self.colorspace == space
            && self.white_level == white_level
            && self.texture_headroom == texture_headroom
        {
            return true;
        }
        self.destroy();
        // SAFETY: SDL property and texture calls on a live renderer.
        unsafe {
            let props = SDL_CreateProperties();
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER,
                i64::from(format.0),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER,
                i64::from(if format == SDL_PIXELFORMAT_RGBA64_FLOAT {
                    SDL_TEXTUREACCESS_TARGET.0
                } else {
                    SDL_TEXTUREACCESS_STATIC.0
                }),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER,
                i64::from(width),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER,
                i64::from(height),
            );
            SDL_SetNumberProperty(
                props,
                SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER,
                i64::from(space.0),
            );
            let metadata = self.hdr_metadata.filter(|_| self.hdr_mode == Some(true));
            // SRGB_LINEAR is the float target of the PyroWave and packed paths, never a PQ picture.
            let hdr = hdr_properties(self.hdr_frame && space != SDL_COLORSPACE_SRGB_LINEAR, white_level, metadata);
            if let Some(white_point) = hdr.white_point {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT, white_point);
            }
            if let Some(headroom) = texture_headroom.or(hdr.headroom) {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT, headroom);
            }
            if !pixel_buffer.is_null() {
                SDL_SetPointerProperty(props, SDL_PROP_TEXTURE_CREATE_METAL_PIXELBUFFER_POINTER, pixel_buffer);
            }
            self.texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            if self.texture.is_null() {
                let error = crate::sdl::error();
                let detail = format!(
                    "SDL could not create the {} video texture ({width}x{height}, colorspace {:#x}, SDR white point {:?}, HDR headroom {:?}, renderer {}): {error}",
                    pixel_format_name(format),
                    space.0,
                    hdr.white_point,
                    texture_headroom.or(hdr.headroom),
                    crate::sdl::renderer_name(renderer),
                );
                self.fail(detail, error);
                return false;
            }
            SDL_SetTextureScaleMode(self.texture, SDL_SCALEMODE_LINEAR);
            self.d3d11 = SDL_GetPointerProperty(
                SDL_GetTextureProperties(self.texture),
                SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER,
                std::ptr::null_mut(),
            );
        }
        self.format = format;
        self.width = width;
        self.height = height;
        self.colorspace = space;
        self.white_level = white_level;
        self.texture_headroom = texture_headroom;
        true
    }

    /// Pools Metal `SDL_Texture` wrappers per VideoToolbox `IOSurfaceID` (up to 16 entries).
    #[cfg(target_os = "macos")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn ensure_videotoolbox(
        &mut self,
        renderer: *mut SDL_Renderer,
        format: SDL_PixelFormat,
        width: i32,
        height: i32,
        space: SDL_Colorspace,
        white_level: f32,
        pixel_buffer: *mut c_void,
    ) -> bool {
        // SAFETY: Caller passes a live `CVPixelBufferRef` on a live Metal renderer.
        let Some(surface_id) = (unsafe { macos_cache::surface_id(pixel_buffer) }) else {
            return self.ensure_with(renderer, format, width, height, space, white_level, None, pixel_buffer);
        };
        let metadata = self.hdr_metadata.filter(|_| self.hdr_mode == Some(true));
        let hdr = hdr_properties(self.hdr_frame && space != SDL_COLORSPACE_SRGB_LINEAR, white_level, metadata);
        let key = macos_cache::CacheKey {
            renderer: renderer as usize,
            format: format.0 as u32,
            width,
            height,
            space: space.0,
            white_level_bits: white_level.to_bits(),
            white_point_bits: hdr.white_point.map(f32::to_bits),
            headroom_bits: hdr.headroom.map(f32::to_bits),
        };
        let cache_valid = self
            .importer
            .as_mut()
            .and_then(crate::dmabuf::Importer::macos_slot_mut::<macos_cache::TextureCache>)
            .is_some_and(|cache| cache.matches(key, self.texture));
        if cache_valid
            && let Some(texture) = self
                .importer
                .as_mut()
                .and_then(crate::dmabuf::Importer::macos_slot_mut::<macos_cache::TextureCache>)
                .and_then(|cache| cache.activate(surface_id))
        {
            self.texture = texture;
            return true;
        }
        if !cache_valid {
            self.destroy();
        }
        // SAFETY: SDL property and texture calls on a live Metal renderer.
        let texture = unsafe {
            let props = SDL_CreateProperties();
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, i64::from(SDL_TEXTUREACCESS_STATIC.0));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(width));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, i64::from(height));
            SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
            if let Some(white_point) = hdr.white_point {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT, white_point);
            }
            if let Some(headroom) = hdr.headroom {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT, headroom);
            }
            SDL_SetPointerProperty(props, SDL_PROP_TEXTURE_CREATE_METAL_PIXELBUFFER_POINTER, pixel_buffer);
            let texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            if texture.is_null() {
                return false;
            }
            SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_LINEAR);
            texture
        };
        if let Some(cache) = self
            .importer
            .as_mut()
            .and_then(crate::dmabuf::Importer::macos_slot_mut::<macos_cache::TextureCache>)
        {
            // SAFETY: `texture` was created on `renderer` matching `key`.
            unsafe { cache.insert(surface_id, texture) };
        } else {
            self.importer = Some(crate::dmabuf::Importer::macos_slot(macos_cache::TextureCache::new(
                key, surface_id, texture,
            )));
        }
        self.texture = texture;
        self.format = format;
        self.width = width;
        self.height = height;
        self.colorspace = space;
        self.white_level = white_level;
        self.texture_headroom = None;
        true
    }

    /// Puts a decoded picture into the texture; `rows` are the only rows of a GPU-decoded picture
    /// to draw when the rows above them are there already (the raced presenter). A decoded H.264,
    /// HEVC or AV1 picture is shown without its alignment padding (`Decoded::visible`): the texture keeps
    /// the whole picture and what draws it samples only the stream's part, so nothing is copied.
    pub fn upload(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, rows: Option<(u32, u32)>) -> bool {
        let (is_plain, visible, setup) = (plain(decoded), decoded.visible, decoded.setup);
        self.run_upload(visible, is_plain, |this| match &decoded.picture {
            #[cfg(windows)]
            media::Picture::GpuPlanes { lease, fence_value } => this.upload_planes(renderer, setup, lease, *fence_value, rows),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            media::Picture::GpuPlanes { lease, fence_value } => {
                let _ = rows;
                this.upload_planes(renderer, setup, lease, *fence_value)
            }
            #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
            media::Picture::GpuPlanes { .. } => { let _ = rows; false }
            media::Picture::Frame(frame) => this.upload_frame(renderer, setup, FrameSource::Ref(frame)),
            media::Picture::Taken => false,
        })
    }

    /// Like `upload`, but moves hardware frames directly into `self.held` without `av_frame_clone`.
    pub fn upload_take(&mut self, renderer: *mut SDL_Renderer, decoded: &mut Decoded, rows: Option<(u32, u32)>) -> bool {
        let (is_plain, visible, setup) = (plain(decoded), decoded.visible, decoded.setup);
        self.run_upload(visible, is_plain, |this| match &decoded.picture {
            #[cfg(windows)]
            media::Picture::GpuPlanes { lease, fence_value } => this.upload_planes(renderer, setup, lease, *fence_value, rows),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            media::Picture::GpuPlanes { lease, fence_value } => {
                let _ = rows;
                this.upload_planes(renderer, setup, lease, *fence_value)
            }
            #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
            media::Picture::GpuPlanes { .. } => { let _ = rows; false }
            media::Picture::Frame(_) => this.upload_frame(renderer, setup, FrameSource::Owned(&mut decoded.picture)),
            media::Picture::Taken => false,
        })
    }

    fn run_upload(&mut self, visible: Option<(i32, i32)>, is_plain: bool, upload: impl FnOnce(&mut Self) -> bool) -> bool {
        // Planes copied for the last picture are not this one's, unless this one fails without
        // invalidating them: then the last picture stays on screen from its planes while the pass
        // and texture that hold them survive.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let last_planes = {
            self.planes_invalidated = false;
            self.planes_pending.take()
        };
        let uploaded = upload(self);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let held = !self.planes_invalidated && self.planes.is_some() && !self.texture.is_null();
            self.planes_pending = planes_after_upload(uploaded, self.planes_pending, last_planes, held, self.pyrowave_format);
        }
        self.visible = visible_after_upload(self.visible, visible, uploaded, is_plain, (self.width, self.height), !self.texture.is_null());
        uploaded
    }

    fn upload_frame(&mut self, renderer: *mut SDL_Renderer, setup: nightjar_core::session::VideoSetup, mut source: FrameSource<'_>) -> bool {
        // A picture whose conversion never ran is not counted.
        #[cfg(unix)]
        if std::mem::take(&mut self.pyrowave_stages_open) { self.pyrowave_stages.reset(); }
        let info = source.frame().info();
        // Only a D3D11VA picture (below) is drawn by `stream::direct`.
        #[cfg(windows)]
        {
            self.d3d11_picture = false;
            self.held = None;
            self.converted_current = false;
        }
        // SDL's HDR headroom and tone mapping are relative to the display's SDR white.
        let white_level = if shows_hdr(self.hdr_mode, self.hdr_stream, info.color_trc) {
            // SAFETY: The live renderer belongs to the presenting thread.
            unsafe {
                SDL_GetFloatProperty(
                    SDL_GetRendererProperties(renderer),
                    SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT,
                    1.0,
                )
                .max(0.001)
            }
        } else {
            1.0
        };
        if info.kind == media::FRAME_WAVELET {
            let ten = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
            let hdr = self.hdr_mode.unwrap_or(ten);
            let flags = u32::from(setup.color_range != 0) | (u32::from(hdr) << 1) | ((setup.color_space as u32) << 2)
                | (u32::from(ten) << 4) | (u32::from(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_YUV444 != 0) << 5);
            let parameters = [setup.width as u32, setup.height as u32, info.width as u32, info.height as u32, 0, 0, flags, white_level.to_bits()];
            if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
            if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, setup.width, setup.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return false; }
            if self.pyrowave.is_none() {
                self.pyrowave = super::super::pyrowave_gpu::Reconstruction::new(renderer, self.texture, parameters);
                self.pyrowave_format = Some(parameters);
            }
            let drawn = self.pyrowave.as_ref().is_some_and(|gpu| gpu.draw_timed(renderer, self.texture, source.frame(), &mut self.pyrowave_stages));
            if drawn { self.pyrowave_stages.commit(); } else { self.pyrowave_stages.reset(); }
            return drawn;
        }
        if setup.chroma_packing != 0 {
            let Some(layout) = rubylight_protocol::packed444::Geometry::new(
                setup.logical_width as u32,
                setup.logical_height as u32,
            ) else {
                return false;
            };
            if !rubylight_protocol::packed444::supported(setup.chroma_packing)
                || layout.coded_size() != (info.width as u32, info.height as u32)
                || (info.ten_bit != 0)
                    != (setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0)
            {
                self.destroy();
                return false;
            }
            let hdr = self
                .hdr_mode
                .unwrap_or(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0);
            let parameters = [
                layout.width,
                layout.height,
                layout.padded_width,
                layout.padded_height,
                info.ten_bit as u32,
                setup.color_range as u32,
                setup.color_space as u32,
                u32::from(hdr),
                white_level.to_bits(),
                u32::from(setup.chroma_packing),
                0,
                0,
            ];
            // A picture mapped into Vulkan (or Vulkan video's) is rebuilt from its planes; one in
            // memory from a buffer. A new decoder may change which, so the reconstruction goes too.
            #[cfg(unix)]
            let coded = media::PlanarLayout::of_vulkan(&info);
            #[cfg(unix)]
            let other_path = packed_path_changed(self.packed.is_some(), coded.is_some(), self.vulkan.is_some());
            #[cfg(not(unix))]
            let other_path = false;
            if other_path
                || self
                    .packed_format
                    .is_some_and(|previous| previous != parameters)
            {
                self.destroy();
            }
            #[cfg(windows)]
            if self.direct_packed && (!hdr || unsafe {
                let properties = SDL_GetRendererProperties(renderer);
                let peak = self.hdr_metadata.map_or(0, |m| {
                    if m.max_content_light_level != 0 { m.max_content_light_level } else { m.max_display_luminance }
                });
                // Keep SDL's texture path when metadata calls for tone mapping.
                SDL_GetNumberProperty(properties, SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0)
                    == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0)
                    && f32::from(peak) / (80.0 * white_level)
                        <= SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0)
            })
                && !crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER)
                    .is_null()
            {
                if self.packed.is_none() || !self.texture.is_null() {
                    self.destroy();
                    self.packed = super::super::packed444::Reconstruction::new(
                        renderer,
                        std::ptr::null_mut(),
                        &parameters,
                    );
                }
                let success = self
                    .packed
                    .as_mut()
                    .is_some_and(|packed| packed.upload(source.frame()));
                if success {
                    self.width = setup.logical_width;
                    self.height = setup.logical_height;
                    self.packed_format = Some(parameters);
                } else {
                    self.destroy();
                }
                return success;
            }
            if !self.ensure(
                renderer,
                SDL_PIXELFORMAT_RGBA64_FLOAT,
                setup.logical_width,
                setup.logical_height,
                SDL_COLORSPACE_SRGB_LINEAR,
                white_level,
            ) {
                return false;
            }
            #[cfg(unix)]
            if let Some(coded) = coded {
                return self.draw_packed_planes(renderer, source.frame(), coded, &parameters);
            }
            if self.packed.is_none() {
                self.packed =
                    super::super::packed444::Reconstruction::new(renderer, self.texture, &parameters);
                self.packed_format = Some(parameters);
            }
            let success = self
                .packed
                .as_mut()
                .is_some_and(|packed| packed.draw(renderer, self.texture, source.frame(), &parameters));
            if !success {
                self.destroy();
            }
            return success;
        }
        self.hdr_frame = info.color_trc == TRC_PQ;
        self.hlg_frame = info.color_trc == TRC_HLG;
        self.centred_chroma = source.frame().centred_chroma();
        if let Some(layout) = media::PlanarLayout::of(&info) {
            return self.upload_planar(renderer, &info, layout, white_level);
        }
        #[cfg(unix)]
        if let Some(layout) = media::PlanarLayout::of_vulkan(&info) {
            return self.upload_vulkan(renderer, source.frame(), &info, layout, white_level);
        }
        #[cfg(unix)]
        if hlg_through_planes(&info, crate::sdl::renderer_is(renderer, "gpu")) {
            return self.upload_hlg(renderer, source.frame(), &info, white_level);
        }
        let space = colorspace(&info);
        let (w, h) = (info.width, info.height);
        // SAFETY: plane pointers and pitches come from the live AVFrame.
        unsafe {
            match info.kind {
                // AYUV, Y410 or Y416: no SDL texture format, so the plane pass draws its copy.
                #[cfg(windows)]
                media::FRAME_D3D11_444 => self.upload_d3d11_444(renderer, &mut source, &info, white_level),
                #[cfg(windows)]
                media::FRAME_D3D11 => {
                    let format = if info.ten_bit != 0 {
                        SDL_PIXELFORMAT_P010
                    } else {
                        SDL_PIXELFORMAT_NV12
                    };
                    if !self.ensure(renderer, format, w, h, space, white_level) {
                        return false;
                    }
                    self.d3d11_picture = true;
                    let frame = source.frame();
                    self.hdr10_plus = frame.hdr10_plus_peak();
                    self.dolby_vision = match frame.dolby_vision() {
                        Some(rpu) if rpu.ipt => {
                            if !self.dolby_vision_ipt_said {
                                eprintln!("nightjar: the stream's Dolby Vision is profile 5 (IPT), which has no HDR10 base layer; its pictures are drawn as HDR10");
                                self.dolby_vision_ipt_said = true;
                            }
                            None
                        }
                        rpu => rpu.and_then(|rpu| rpu.peak),
                    };
                    let readable = frame.d3d11_surface().is_some_and(|surface| surface.readable);
                    self.held = None;
                    self.copied = false;
                    // The pass reads a readable decoder texture in place: no copy.
                    if !self.direct_failed && super::super::direct::enabled() && readable {
                        self.held = source.take_or_ref();
                        if self.held.is_some() {
                            return true;
                        }
                    }
                    if self.d3d11.is_null() {
                        self.fail(
                            format!(
                                "the {} video texture has no Direct3D 11 texture to copy the decoder's pictures into (renderer {})",
                                pixel_format_name(format),
                                crate::sdl::renderer_name(renderer),
                            ),
                            "the video texture is not a Direct3D 11 texture".into(),
                        );
                        return false;
                    }
                    let copied = source.frame().copy_to_d3d11(self.d3d11, w, h);
                    self.copied = copied;
                    if copied {
                        let _ = source.take_owned();
                    } else {
                        self.fail(
                            format!("copying a {w}x{h} decoder picture into the {} video texture failed", pixel_format_name(format)),
                            "the GPU copy failed".into(),
                        );
                    }
                    copied
                }
                #[cfg(target_os = "macos")]
                media::FRAME_VIDEOTOOLBOX => {
                    if crate::sdl::renderer_is(renderer, "metal") {
                        let format = if info.ten_bit == 0 { SDL_PIXELFORMAT_NV12 } else { SDL_PIXELFORMAT_P010 };
                        // FFmpeg keeps the CVPixelBufferRef in AVFrame.data[3]. Keep the
                        // frame (and thus its pool slot) alive for the displayed texture.
                        let buffer: *mut c_void = source.frame().raw.as_ref().data[3].cast();
                        if !buffer.is_null()
                            && self.ensure_videotoolbox(renderer, format, w, h, space, white_level, buffer)
                            && let Some(held) = source.take_or_ref()
                        {
                            self.held = Some(held);
                            return true;
                        }
                    }
                    // An SDL renderer without IOSurface import can still display the frame.
                    source.frame().map_to_memory().is_some_and(|mapped| self.upload_nv(renderer, &mapped.info(), space, white_level))
                }
                media::FRAME_NV12 | media::FRAME_P010 => self.upload_nv(renderer, &info, space, white_level),
                #[cfg(unix)]
                media::FRAME_DMABUF => self.upload_dmabuf(renderer, &mut source, &info, space, white_level),
                media::FRAME_YUV420P => {
                    self.ensure(renderer, SDL_PIXELFORMAT_IYUV, w, h, space, white_level)
                        && SDL_UpdateYUVTexture(
                            self.texture,
                            std::ptr::null(),
                            info.data[0],
                            info.linesize[0],
                            info.data[1],
                            info.linesize[1],
                            info.data[2],
                            info.linesize[2],
                        )
                }
                _ => false,
            }
        }
    }

    /// Uploads an NV12 or P010 picture from memory.
    fn upload_nv(&mut self, renderer: *mut SDL_Renderer, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> bool {
        // A texture made for imports reads its chroma as red-green, uploads are luminance-alpha.
        #[cfg(unix)]
        if self.imported {
            self.destroy();
        }
        let format = if info.kind == media::FRAME_P010 { SDL_PIXELFORMAT_P010 } else { SDL_PIXELFORMAT_NV12 };
        self.ensure(renderer, format, info.width, info.height, space, white_level)
            // SAFETY: plane pointers and pitches come from the live AVFrame.
            && unsafe {
                SDL_UpdateNVTexture(self.texture, std::ptr::null(), info.data[0], info.linesize[0], info.data[1], info.linesize[1])
            }
    }

    /// Imports a VAAPI picture's DMA-BUFs into the NV12 texture. When that cannot be done it says
    /// why once and maps the picture to memory instead, and the decoder maps the stream's next
    /// ones itself.
    #[cfg(unix)]
    fn upload_dmabuf(&mut self, renderer: *mut SDL_Renderer, source: &mut FrameSource<'_>, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> bool {
        if source.frame().dmabuf().is_none() { return false; }
        if !self.dmabuf_declined {
            let result = {
                let export = source.frame().dmabuf().expect("checked above");
                self.import(renderer, export, info, space, white_level)
            };
            match result {
                Ok(()) => {
                    // The texture samples the decoder's surface: it stays out of the decoder's
                    // hands until the next picture replaces it.
                    self.held = source.take_or_ref();
                    return true;
                }
                Err(error) => {
                    eprintln!("nightjar: VAAPI pictures cannot be imported as DMA-BUFs ({error}); mapping them to memory instead");
                    self.dmabuf_declined = true;
                    self.destroy();
                }
            }
        }
        if let Some(export) = source.frame().dmabuf() {
            export.decline();
        }
        let Some(mapped) = source.frame().map_to_memory() else {
            self.fail(format!("mapping a {}x{} VAAPI picture to memory failed", info.width, info.height), "the picture could not be read".into());
            return false;
        };
        self.upload_nv(renderer, &mapped.info(), space, white_level)
    }

    /// Draws an HLG NV12, P010, VAAPI, or VideoToolbox picture with the plane shader
    /// (`hlg_through_planes`). Hardware surfaces are mapped to memory for this picture alone so a
    /// PQ or SDR picture after it keeps zero-copy import.
    #[cfg(unix)]
    fn upload_hlg(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, white_level: f32) -> bool {
        let mapped = if matches!(info.kind, media::FRAME_DMABUF | media::FRAME_VIDEOTOOLBOX) {
            let Some(mapped) = frame.map_to_memory() else {
                let what = if info.kind == media::FRAME_VIDEOTOOLBOX { "VideoToolbox" } else { "VAAPI" };
                self.fail(format!("mapping a {}x{} {what} picture to memory failed", info.width, info.height), "the picture could not be read".into());
                return false;
            };
            Some(mapped)
        } else {
            None
        };
        let info = mapped.as_ref().map_or(*info, media::Frame::info);
        let Some(layout) = media::PlanarLayout::of_semi_planar(&info) else {
            self.fail(format!("a {}x{} HLG picture of frame kind {} has no planes to draw", info.width, info.height, info.kind), "the HLG picture cannot be drawn".into());
            return false;
        };
        self.upload_planar(renderer, &info, layout, white_level)
    }

    #[cfg(unix)]
    pub(super) fn import(&mut self, renderer: *mut SDL_Renderer, export: &crate::dmabuf::Export, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> Result<(), String> {
        if !self.imported && !self.texture.is_null() {
            self.destroy();
        }
        // The hint is read when SDL makes the texture: only textures for imports get it.
        let hint = c"SDL_RENDER_OPENGL_NV12_RG_SHADER";
        crate::sdl::set_hint(hint.as_ptr(), c"1");
        let made = self.ensure(renderer, SDL_PIXELFORMAT_NV12, info.width, info.height, space, white_level);
        // SAFETY: a hint name.
        unsafe { SDL_ResetHint(hint.as_ptr()) };
        if !made {
            return Err(self.failure.take().unwrap_or_else(|| "SDL made no NV12 texture".into()));
        }
        self.imported = true;
        if self.importer.is_none() {
            self.importer = Some(crate::dmabuf::Importer::new()?);
        }
        let importer = self.importer.as_mut().expect("made above");
        // SAFETY: on the renderer's thread, with its live texture.
        unsafe { importer.import(renderer, self.texture, export, info.width as u32, info.height as u32) }
    }

    /// The current picture's transfer: the one decision of the pass, AI enhancement and what
    /// SDL draws (`picture_transfer`).
    #[cfg(windows)]
    pub(super) fn transfer(&self) -> super::super::direct::Transfer {
        picture_transfer(self.format, self.hdr_mode, self.hdr_stream, self.hdr_frame, self.hlg_frame)
    }

    /// Readable held D3D11 decoder frame not yet copied into SDL's texture, so AI enhancement can
    /// read `SharedSlot::render_tex` directly without an intermediate `copy_held()` copy.
    #[cfg(windows)]
    pub fn enhance_held_frame(&self) -> Option<&media::Frame> {
        self.held.as_ref().filter(|frame| !self.copied && frame.d3d11_surface().is_some_and(|surface| surface.readable))
    }

    /// Puts a held decoder picture into the video texture, for what reads it (AI enhancement,
    /// `sdl_texture`). Nothing to do when the picture was copied.
    #[cfg(windows)]
    pub fn copy_held(&mut self) {
        self.materialize_planes();
        let Some(frame) = &self.held else { return };
        if self.copied || self.d3d11.is_null() { return; }
        self.copied = frame.copy_to_d3d11(self.d3d11, self.width, self.height);
        if !self.copied {
            self.fail(
                format!("copying a {}x{} decoder picture into the {} video texture failed", self.width, self.height, pixel_format_name(self.format)),
                "the GPU copy failed".into(),
            );
        }
    }

    #[cfg(not(windows))]
    pub fn copy_held(&mut self) {}
}
