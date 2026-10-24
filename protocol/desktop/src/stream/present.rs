//! Showing decoded pictures: the video texture, and when each picture goes to the screen.
//!
//! Four pacing modes:
//! - host times (VRR): each picture is shown at the PC's frame time plus a steady delay
//!   ([`PresentClock`]), so the display refreshes at the game's cadence instead of the
//!   network's. Presents are immediate (tearing allowed) at the scheduled time.
//! - immediate: shown as soon as decoded (lowest latency; tears without VRR).
//! - vsync: the newest picture at each refresh.
//! - beam race: immediate, and a picture still being received is shown slice by slice just ahead
//!   of the display's beam (`stream::race`); chosen once the display is known to allow it.
//!
//! The D3D11 path copies the decoder's surface into the renderer's NV12 texture on the GPU
//! (as moonlight-qt's d3d11va renderer does when it cannot bind decoder surfaces directly);
//! other paths upload planes.

use std::collections::VecDeque;
use std::ffi::c_void;

use rubylight_protocol::present_clock::{PresentClock, Settings as ClockSettings};
use sdl3_sys::everything::*;

use super::pipeline::Decoded;
use crate::media::{self, FrameInfo};

/// How long the presenter waits for a GPU-decoded picture's fence before the stream ends. The
/// decoder thread has already waited for the decode, so this is only a bound on a broken device.
#[cfg(windows)]
const PLANES_DECODE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacingMode {
    HostTimes,
    Immediate,
    /// Immediate, with the slices of a frame presented ahead of the beam (B10 step 4). Never
    /// chosen by `pacing`: the stream switches to it when the display's probe allows.
    BeamRace,
    Vsync,
}

impl PacingMode {
    pub fn label(self) -> &'static str {
        match self {
            PacingMode::HostTimes => "VRR, PC frame times",
            PacingMode::Immediate => "Immediate",
            PacingMode::BeamRace => "Beam race",
            PacingMode::Vsync => "V-Sync",
        }
    }
}

pub fn pacing(settings: &crate::config::Settings, tearing_supported: bool) -> (PacingMode, bool) {
    let vrr = settings.vrr && tearing_supported;
    let mode = if vrr {
        if settings.present_at_host_times && !(cfg!(windows) && settings.experimental_packed_444) {
            PacingMode::HostTimes
        } else {
            PacingMode::Immediate
        }
    } else if settings.vsync {
        PacingMode::Vsync
    } else {
        PacingMode::Immediate
    };
    (mode, vrr)
}

/// A picture with its present time.
pub struct Pending {
    pub decoded: Decoded,
    pub present_us: u64,
    pub hold_us: i64,
    pub late: bool,
}

pub struct Pacer {
    pub mode: PacingMode,
    pub clock: PresentClock,
    /// Shared with the decoder thread (`Shared::pacer_backlog`): when every GPU plane set is
    /// held by a picture waiting here, the decoder drops the oldest of them instead of the
    /// newest frame.
    pending: std::sync::Arc<std::sync::Mutex<VecDeque<Pending>>>,
}

impl Pacer {
    /// `min_interval_us`: the display's shortest refresh interval.
    #[cfg(test)]
    pub fn new(mode: PacingMode, min_interval_us: i64) -> Self {
        Self::with_backlog(mode, min_interval_us, Default::default())
    }

    pub fn with_backlog(mode: PacingMode, min_interval_us: i64, pending: std::sync::Arc<std::sync::Mutex<VecDeque<Pending>>>) -> Self {
        Self {
            mode,
            clock: PresentClock::new(ClockSettings {
                min_interval_us,
                ..Default::default()
            }),
            pending,
        }
    }

    /// Takes newly decoded pictures, scheduling each once, in order.
    pub fn accept(&mut self, decoded: Decoded) {
        let (present_us, hold_us, late) = match self.mode {
            PacingMode::HostTimes => {
                let s = self.clock.schedule(decoded.rtp, decoded.ready_us);
                (s.present_us, s.hold_us, s.late)
            }
            _ => (decoded.ready_us, 0, false),
        };
        super::pipeline::lock(&self.pending).push_back(Pending {
            decoded,
            present_us,
            hold_us,
            late,
        });
    }

    /// The raced frame number of the next picture and when it is due, if one is waiting.
    #[cfg(any(windows, test))]
    pub fn front_race(&self) -> Option<(u64, u64)> {
        super::pipeline::lock(&self.pending).front().map(|p| (p.decoded.race_seq, p.present_us))
    }

    /// When the next picture is due, if one is waiting.
    pub fn next_due(&self) -> Option<u64> {
        super::pipeline::lock(&self.pending).front().map(|p| p.present_us)
    }

    /// The newest picture due at `now_us`; older due ones are dropped (counted in the
    /// return's second value).
    pub fn take_due(&mut self, now_us: u64) -> (Option<Pending>, u32) {
        let mut chosen = None;
        let mut dropped = 0;
        let mut pending = super::pipeline::lock(&self.pending);
        while pending.front().is_some_and(|p| p.present_us <= now_us) {
            if chosen.is_some() {
                dropped += 1;
            }
            chosen = pending.pop_front();
        }
        (chosen, dropped)
    }
}

/// The renderer's texture holding the current picture.
pub struct VideoTexture {
    #[cfg(windows)]
    direct_packed: bool,
    pub texture: *mut SDL_Texture,
    width: i32,
    height: i32,
    format: SDL_PixelFormat,
    colorspace: SDL_Colorspace,
    d3d11: *mut c_void,
    packed: Option<super::packed444::Reconstruction>,
    pyrowave: Option<super::pyrowave_gpu::Reconstruction>,
    pyrowave_format: Option<[u32; 8]>,
    /// PyroWave decoded on the GPU: converts the decoder's planes into the texture.
    #[cfg(windows)]
    planes: Option<super::d3d11::PlanesPass>,
    #[cfg(unix)]
    planes: Option<super::planes_gpu::PlanesPass>,
    /// Why the last `upload` failed, when it knows more than the caller can say.
    pub failure: Option<String>,
    /// Measurement: CPU time per PyroWave picture in `pyrowave_gpu`, kept across `destroy`.
    pub pyrowave_stages: super::stage_times::Stages<2>,
    packed_format: Option<[u32; 12]>,
    white_level: f32,
    hdr_mode: Option<bool>,
    hdr_metadata: Option<nightjar_core::control::HdrMetadata>,
}

impl Default for VideoTexture {
    fn default() -> Self {
        Self {
            #[cfg(windows)]
            direct_packed: true,
            texture: std::ptr::null_mut(),
            width: 0,
            height: 0,
            format: SDL_PIXELFORMAT_UNKNOWN,
            colorspace: SDL_COLORSPACE_UNKNOWN,
            d3d11: std::ptr::null_mut(),
            packed: None,
            pyrowave: None,
            pyrowave_format: None,
            #[cfg(any(windows, unix))]
            planes: None,
            failure: None,
            pyrowave_stages: super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES),
            packed_format: None,
            white_level: 1.0,
            hdr_mode: None,
            hdr_metadata: None,
        }
    }
}

/// FFmpeg's colour description as an SDL colorspace.
fn colorspace(info: &FrameInfo) -> SDL_Colorspace {
    // AVCOL_RANGE_JPEG = 2; AVCOL_SPC_BT709 = 1, BT470BG = 5, SMPTE170M = 6, BT2020_NCL = 9;
    // AVCOL_TRC_SMPTE2084 = 16.
    let full = info.color_range == 2;
    if info.color_trc == 16 {
        return SDL_COLORSPACE_HDR10;
    }
    match (info.color_space, full) {
        (9 | 10, false) => SDL_COLORSPACE_BT2020_LIMITED,
        (9 | 10, true) => SDL_COLORSPACE_BT2020_FULL,
        (5 | 6, false) => SDL_COLORSPACE_BT601_LIMITED,
        (5 | 6, true) => SDL_COLORSPACE_BT601_FULL,
        (_, true) => SDL_COLORSPACE_BT709_FULL,
        _ => SDL_COLORSPACE_BT709_LIMITED,
    }
}

impl VideoTexture {
    pub fn new(_direct_packed: bool) -> Self {
        Self { #[cfg(windows)] direct_packed: _direct_packed, ..Default::default() }
    }

    pub fn set_hdr_mode(
        &mut self,
        enabled: bool,
        metadata: Option<nightjar_core::control::HdrMetadata>,
    ) {
        if self.hdr_mode != Some(enabled) || self.hdr_metadata != metadata {
            self.destroy();
            self.hdr_mode = Some(enabled);
            self.hdr_metadata = metadata;
        }
    }

    pub fn is_hdr(&self) -> bool {
        self.hdr_mode.unwrap_or(
            self.colorspace == SDL_COLORSPACE_HDR10
                || self
                    .packed_format
                    .is_some_and(|parameters| parameters[7] != 0),
        )
    }

    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    fn ensure(
        &mut self,
        renderer: *mut SDL_Renderer,
        format: SDL_PixelFormat,
        width: i32,
        height: i32,
        space: SDL_Colorspace,
        white_level: f32,
    ) -> bool {
        if !self.texture.is_null()
            && self.format == format
            && self.width == width
            && self.height == height
            && self.colorspace == space
            && self.white_level == white_level
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
            if space == SDL_COLORSPACE_HDR10 {
                SDL_SetFloatProperty(
                    props,
                    SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT,
                    80.0 * white_level,
                );
            }
            if let Some(metadata) = self.hdr_metadata.filter(|_| self.hdr_mode == Some(true)) {
                let peak = if metadata.max_content_light_level != 0 {
                    metadata.max_content_light_level
                } else {
                    metadata.max_display_luminance
                };
                if peak > 0 {
                    SDL_SetFloatProperty(
                        props,
                        SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT,
                        f32::from(peak) / (80.0 * white_level),
                    );
                }
            }
            self.texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            if self.texture.is_null() {
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
        true
    }

    /// Puts a decoded picture into the texture; `rows` are the only rows of a GPU-decoded picture
    /// to draw when the rows above them are there already (the raced presenter).
    pub fn upload(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, rows: Option<(u32, u32)>) -> bool {
        #[cfg(windows)]
        if let media::Picture::GpuPlanes { lease, fence_value } = &decoded.picture {
            return self.upload_planes(renderer, decoded, lease, *fence_value, rows);
        }
        #[cfg(not(windows))]
        let _ = rows;
        #[cfg(unix)]
        if let media::Picture::GpuPlanes { lease } = &decoded.picture {
            return self.upload_planes(renderer, decoded, lease);
        }
        let Some(frame) = decoded.picture.frame() else { return false };
        let info = frame.info();
        // SDL's HDR headroom and tone mapping are relative to the display's SDR white.
        let white_level = if self.hdr_mode.unwrap_or(
            info.color_trc == 16
                || decoded.setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0,
        ) {
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
            let setup = decoded.setup;
            let ten = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
            let hdr = self.hdr_mode.unwrap_or(ten);
            if hdr && !crate::sdl::hdr_available(renderer) { self.destroy(); return false; }
            let flags = u32::from(setup.color_range != 0) | (u32::from(hdr) << 1) | ((setup.color_space as u32) << 2)
                | (u32::from(ten) << 4) | (u32::from(setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_YUV444 != 0) << 5);
            let parameters = [setup.width as u32, setup.height as u32, info.width as u32, info.height as u32, 0, 0, flags, white_level.to_bits()];
            if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
            if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, setup.width, setup.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return false; }
            if self.pyrowave.is_none() {
                self.pyrowave = super::pyrowave_gpu::Reconstruction::new(renderer, self.texture, parameters);
                self.pyrowave_format = Some(parameters);
            }
            let drawn = self.pyrowave.as_ref().is_some_and(|gpu| gpu.draw_timed(renderer, self.texture, frame, &mut self.pyrowave_stages));
            if drawn { self.pyrowave_stages.commit(); } else { self.pyrowave_stages.reset(); }
            return drawn;
        }
        if decoded.setup.chroma_packing != 0 {
            let setup = decoded.setup;
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
            if hdr && !crate::sdl::hdr_available(renderer) {
                self.destroy();
                return false;
            }
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
            if self
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
                && !crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER)
                    .is_null()
            {
                if self.packed.is_none() || !self.texture.is_null() {
                    self.destroy();
                    self.packed = super::packed444::Reconstruction::new(
                        renderer,
                        std::ptr::null_mut(),
                        &parameters,
                    );
                }
                let success = self
                    .packed
                    .as_mut()
                    .is_some_and(|packed| packed.upload(frame));
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
            if self.packed.is_none() {
                self.packed =
                    super::packed444::Reconstruction::new(renderer, self.texture, &parameters);
                self.packed_format = Some(parameters);
            }
            let success = self
                .packed
                .as_mut()
                .is_some_and(|packed| packed.draw(renderer, self.texture, frame, &parameters));
            if !success {
                self.destroy();
            }
            return success;
        }
        let space = colorspace(&info);
        let (w, h) = (info.width, info.height);
        // SAFETY: plane pointers and pitches come from the live AVFrame.
        unsafe {
            match info.kind {
                media::FRAME_D3D11 => {
                    let format = if info.ten_bit != 0 {
                        SDL_PIXELFORMAT_P010
                    } else {
                        SDL_PIXELFORMAT_NV12
                    };
                    self.ensure(renderer, format, w, h, space, white_level)
                        && !self.d3d11.is_null()
                        && frame.copy_to_d3d11(self.d3d11, w, h)
                }
                media::FRAME_NV12 | media::FRAME_P010 => {
                    let format = if info.kind == media::FRAME_P010 {
                        SDL_PIXELFORMAT_P010
                    } else {
                        SDL_PIXELFORMAT_NV12
                    };
                    self.ensure(renderer, format, w, h, space, white_level)
                        && SDL_UpdateNVTexture(
                            self.texture,
                            std::ptr::null(),
                            info.data[0],
                            info.linesize[0],
                            info.data[1],
                            info.linesize[1],
                        )
                }
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

    pub fn draw(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect) -> bool {
        #[cfg(windows)]
        if self.texture.is_null() {
            if let Some(packed) = &mut self.packed {
                return packed.present(
                    renderer,
                    self.packed_format.as_ref().unwrap(),
                    destination,
                );
            }
        }
        if !self.texture.is_null() {
            // SAFETY: The renderer and its video texture are live on this thread.
            unsafe { SDL_RenderTexture(renderer, self.texture, std::ptr::null(), destination) };
        }
        true
    }

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
            match unsafe { super::d3d11::PlanesPass::new(self.d3d11) } {
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
    fn upload_planes(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, lease: &super::gpu_planes::Lease, fence_value: u64, rows: Option<(u32, u32)>) -> bool {
        let Some(parameters) = self.prepare_planes(renderer, decoded.setup) else { return false };
        let step = std::time::Instant::now();
        let drawn = self.planes.as_mut().is_some_and(|pass| match pass.draw(lease, fence_value, &parameters, PLANES_DECODE_DEADLINE, rows) {
            Ok(()) => true,
            Err(error) => { self.failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if drawn {
            self.pyrowave_stages.add(super::pyrowave_gpu::PASSES, step);
            self.pyrowave_stages.commit();
        } else { self.pyrowave_stages.reset(); }
        drawn
    }

    /// Draws `rows` of a slice of the frame being received into the texture (the frame's plane
    /// set stays with it). Returns false when the slice cannot be drawn; `failure` says why when
    /// the stream must end.
    #[cfg(windows)]
    pub fn upload_slice(&mut self, renderer: *mut SDL_Renderer, setup: nightjar_core::session::VideoSetup, pin: &super::gpu_planes::Pin, fence_value: u64, rows: (u32, u32)) -> bool {
        let Some(parameters) = self.prepare_planes(renderer, setup) else { return false };
        let step = std::time::Instant::now();
        let drawn = self.planes.as_mut().is_some_and(|pass| match pass.draw_slice(pin, fence_value, &parameters, PLANES_DECODE_DEADLINE, rows) {
            Ok(()) => true,
            Err(error) => { self.failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if drawn { self.pyrowave_stages.add(super::pyrowave_gpu::PASSES, step); } else { self.pyrowave_stages.reset(); }
        drawn
    }

    /// Draws a picture the GPU decoder left in CPU plane buffers: uploads the planes to the
    /// renderer's GPU, converts them into the texture and gives the plane set back.
    #[cfg(unix)]
    fn upload_planes(&mut self, renderer: *mut SDL_Renderer, decoded: &Decoded, lease: &super::gpu_planes::Lease) -> bool {
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
        if self.planes.is_none() {
            // SAFETY: The live renderer belongs to the presenting thread.
            match unsafe { super::planes_gpu::PlanesPass::new(renderer, lease.planes().config()) } {
                Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                Err(error) => { self.failure = Some(format!("PyroWave GPU decoding cannot draw its planes: {error}")); return false; }
            }
        }
        let texture = self.texture;
        let stages = &mut self.pyrowave_stages;
        let failure = &mut self.failure;
        let drawn = self.planes.as_mut().is_some_and(|pass| match unsafe { pass.draw(renderer, texture, lease, &parameters, stages) } {
            Ok(()) => true,
            Err(error) => { *failure = Some(format!("PyroWave GPU decoding failed: {error}")); false }
        });
        if drawn { self.pyrowave_stages.commit(); } else { self.pyrowave_stages.reset(); }
        drawn
    }

    pub fn destroy(&mut self) {
        #[cfg(any(windows, unix))]
        { self.planes = None; }
        self.pyrowave = None;
        self.pyrowave_format = None;
        self.packed = None;
        self.packed_format = None;
        if !self.texture.is_null() {
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.texture) };
        }
        // The stage timers belong to the whole stream, not to one texture.
        let stages = std::mem::replace(&mut self.pyrowave_stages, super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES));
        *self = Self {
            #[cfg(windows)]
            direct_packed: self.direct_packed,
            failure: self.failure.take(),
            hdr_mode: self.hdr_mode,
            hdr_metadata: self.hdr_metadata,
            pyrowave_stages: stages,
            ..Self::default()
        };
    }
}

/// The largest rectangle of the picture's aspect ratio inside `area`, centred.
#[cfg(test)]
pub fn letterbox(picture: (i32, i32), area: (i32, i32)) -> SDL_FRect {
    viewport(picture, area, crate::config::VideoScaling::Fit, false)
}

pub fn viewport(picture: (i32, i32), area: (i32, i32), scaling: crate::config::VideoScaling, top: bool) -> SDL_FRect {
    use crate::config::VideoScaling;
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    if picture.0 <= 0 || picture.1 <= 0 || area.0 <= 0 || area.1 <= 0 {
        return SDL_FRect { x: 0.0, y: 0.0, w: aw.max(1.0), h: ah.max(1.0) };
    }
    let (pw, ph) = (picture.0 as f32, picture.1 as f32);
    let (w, h) = match scaling {
        VideoScaling::Stretch => (aw, ah),
        VideoScaling::Native => (pw, ph),
        VideoScaling::Fit | VideoScaling::Fill => {
            let scale = if scaling == VideoScaling::Fit { (aw / pw).min(ah / ph) } else { (aw / pw).max(ah / ph) };
            ((pw * scale).round(), (ph * scale).round())
        }
    };
    SDL_FRect {
        x: ((aw - w) / 2.0).trunc(),
        y: if top && matches!(scaling, VideoScaling::Fit | VideoScaling::Native) { 0.0 } else { ((ah - h) / 2.0).trunc() },
        w,
        h,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_pyrowave_stage_timers_survive_destroying_the_texture() {
        let mut texture = VideoTexture::default();
        texture.pyrowave_stages.add(0, std::time::Instant::now());
        texture.pyrowave_stages.commit();
        texture.destroy();
        assert_eq!(texture.pyrowave_stages.frames(), 1);
        assert!(texture.pyrowave_stages.summary().is_some());
    }

    #[test]
    fn scaling_matches_android_fit_fill_stretch_native_and_top_alignment() {
        use crate::config::VideoScaling::*;
        for (mode, top, expected) in [
            (Fit, false, [0.0, 152.0, 1280.0, 720.0]),
            (Fit, true, [0.0, 0.0, 1280.0, 720.0]),
            (Fill, false, [-270.0, 0.0, 1820.0, 1024.0]),
            (Fill, true, [-270.0, 0.0, 1820.0, 1024.0]),
            (Stretch, true, [0.0, 0.0, 1280.0, 1024.0]),
            (Native, false, [-320.0, -28.0, 1920.0, 1080.0]),
            (Native, true, [-320.0, 0.0, 1920.0, 1080.0]),
        ] {
            let r = viewport((1920, 1080), (1280, 1024), mode, top);
            assert_eq!([r.x, r.y, r.w, r.h], expected);
        }
        let r = viewport((0, 0), (1280, 1024), Native, true);
        assert_eq!([r.x, r.y, r.w, r.h], [0.0, 0.0, 1280.0, 1024.0]);
    }

    #[test]
    fn letterbox_keeps_the_aspect_ratio() {
        let r = letterbox((1920, 1080), (2560, 1600));
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 80.0, 2560.0, 1440.0));
        let r = letterbox((1920, 1080), (1920, 1080));
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 1920.0, 1080.0));
        let r = letterbox((1280, 1024), (1920, 1080));
        assert_eq!((r.w, r.h), (1350.0, 1080.0));
    }

    #[test]
    fn the_default_config_shows_frames_immediately_with_vrr_and_waits_for_vsync_without() {
        // Measured on a 240 Hz tearing display: HostTimes held frames about 7 ms and had the
        // less even present intervals, so it is opt-in (docs/pyrowave-decode.md).
        let settings = crate::config::Settings::default();
        assert!(!settings.present_at_host_times);
        assert_eq!(pacing(&settings, true), (PacingMode::Immediate, true));
        // No tearing support: VRR is not available and the display's refresh is awaited, as before.
        assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
        let no_vrr = crate::config::Settings { vrr: false, ..settings.clone() };
        assert_eq!(pacing(&no_vrr, true), (PacingMode::Vsync, false));
        let no_vsync = crate::config::Settings { vrr: false, vsync: false, ..settings.clone() };
        assert_eq!(pacing(&no_vsync, true), (PacingMode::Immediate, false));
    }

    #[test]
    fn pacing_preserves_vrr_without_host_time_delay() {
        let mut settings = crate::config::Settings { present_at_host_times: true, ..Default::default() };
        assert_eq!(pacing(&settings, true), (PacingMode::HostTimes, true));
        settings.present_at_host_times = false;
        assert_eq!(pacing(&settings, true), (PacingMode::Immediate, true));
        settings.present_at_host_times = true;
        settings.experimental_packed_444 = true;
        let expected = if cfg!(windows) {
            PacingMode::Immediate
        } else {
            PacingMode::HostTimes
        };
        assert_eq!(pacing(&settings, true), (expected, true));
        assert!(settings.present_at_host_times);
    }

    #[test]
    fn pacing_keeps_the_non_vrr_fallback() {
        let mut settings = crate::config::Settings {
            experimental_packed_444: true,
            ..Default::default()
        };
        assert_eq!(pacing(&settings, false), (PacingMode::Vsync, false));
        settings.vrr = false;
        assert_eq!(pacing(&settings, true), (PacingMode::Vsync, false));
        settings.vsync = false;
        assert_eq!(pacing(&settings, true), (PacingMode::Immediate, false));
    }

    pub(crate) fn decoded(ready_us: u64) -> Decoded {
        let mut decoder =
            media::VideoDecoder::open(media::Codec::Hevc, media::Hardware::None, 1, false).unwrap();
        assert_eq!(
            decoder.send(
                include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin"),
                0
            ),
            0
        );
        let media::Received::Frame(frame) = decoder.receive() else {
            panic!("fixture did not decode");
        };
        Decoded {
            picture: media::Picture::Frame(frame),
            setup: nightjar_core::session::VideoSetup {
                format: nightjar_core::sdp::VIDEO_FORMAT_H265,
                width: 128,
                height: 128,
                fps: 60,
                chroma_packing: 1,
                logical_width: 128,
                logical_height: 64,
                color_space: 1,
                color_range: 1,
            },
            rtp: ready_us as u32,
            number: 0,
            receive_us: 0,
            enqueue_us: 0,
            input_us: 0,
            ready_us,
            host_latency: 0,
            early: 0,
            first_block_us: 0,
            race: Default::default(),
            bands: Vec::new(),
            race_seq: 0,
        }
    }

    #[test]
    fn immediate_pacing_takes_the_newest_ready_picture_without_holding_it() {
        let mut pacer = Pacer::new(PacingMode::Immediate, 16_667);
        assert!(pacer.next_due().is_none());
        for ready in [10, 20, 30] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(25);
        let picture = picture.unwrap();
        assert_eq!(picture.decoded.ready_us, 20);
        assert_eq!(picture.present_us, 20);
        assert_eq!(picture.hold_us, 0);
        assert_eq!(dropped, 1);
        assert_eq!(pacer.next_due(), Some(30));
        let (picture, dropped) = pacer.take_due(30);
        assert_eq!(picture.unwrap().decoded.ready_us, 30);
        assert_eq!(dropped, 0);
        assert!(pacer.next_due().is_none());
    }

    #[test]
    fn the_front_picture_says_which_raced_frame_it_is_and_when_it_is_due() {
        let mut pacer = Pacer::new(PacingMode::BeamRace, 16_667);
        assert_eq!(pacer.front_race(), None);
        let mut picture = decoded(40);
        picture.race_seq = 7;
        pacer.accept(picture);
        pacer.accept(decoded(50));
        assert_eq!(pacer.front_race(), Some((7, 40)));
    }
}
