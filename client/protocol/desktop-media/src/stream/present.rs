//! Showing decoded pictures: the video texture, and when each picture goes to the screen.
//!
//! Four pacing modes:
//! - host times (VRR): each picture is shown at the PC's frame time plus a steady delay
//!   ([`PresentClock`]), so the display refreshes at the game's cadence instead of the
//!   network's. Presents are immediate (tearing allowed) at the scheduled time.
//! - immediate: shown as soon as decoded (lowest latency; tears without VRR).
//! - vsync: the newest picture at each refresh, or with the FPS limit and smoothest frame pacings
//!   (`config::FramePacing`) each picture in turn.
//! - beam race: immediate, and a picture still being received is shown slice by slice just ahead
//!   of the display's beam (`stream::race`); chosen once the display is known to allow it.
//!
//! The D3D11 path copies the decoder's surface into the renderer's NV12 texture on the GPU
//! (as moonlight-qt's d3d11va renderer does when it cannot bind decoder surfaces directly);
//! other paths upload planes.

use std::collections::VecDeque;
use std::ffi::c_void;

use rubylight_protocol::present_clock::{Grid, PresentClock, Settings as ClockSettings};
use sdl3_sys::everything::*;

use super::pipeline::Decoded;
use crate::config::FramePacing;
use crate::media::{self, FrameInfo};

mod color;
mod direct_draw;
mod geometry;
mod pacer;
mod planes;
mod upload;
mod video_only;

pub use color::hdr_negotiated;
pub(super) use color::{interleaved_flags, planar_hlg, planar_parameters, vulkan_flags};
// Both Linux and macOS route HLG hardware surfaces through the plane shader (tested everywhere).
#[cfg(any(unix, test))]
pub(super) use color::hlg_through_planes;
use color::*;
pub use geometry::{draws_one_to_one, viewport, whole_pixel_shift};
#[cfg(test)]
pub use geometry::letterbox;
use geometry::{plain, visible_part};
pub use pacer::*;
pub use video_only::{VideoOnly, VideoOnlyFrame, VIDEO_ONLY_ENVIRONMENT};

/// How long the presenter waits for a GPU-decoded picture's fence before the stream ends. The
/// decoder thread has already waited for the decode, so this is only a bound on a broken device.
#[cfg(windows)]
const PLANES_DECODE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

/// What holds the planes of the picture that waits for the back buffer.
#[cfg(windows)]
enum PlanesHold {
    /// A GPU decoder's plane set, once the decode fence reaches `fence_value`.
    Lease { lease: super::gpu_planes::Lease, fence_value: u64 },
    /// The planes the pass uploaded into its own textures (`PlanesPass::upload_frame`).
    Frame,
    /// A 4:4:4 decoder surface bound in the pass (`PlanesPass::bind_surface`): `held` is the
    /// decoder's frame while it is read in place, None for the pass's copy.
    Surface { held: Option<media::Frame> },
}

/// The current picture's planes, waiting for the back buffer or the video texture.
#[cfg(windows)]
struct DirectPlanes {
    source: PlanesHold,
    parameters: [u32; 8],
    /// The video texture holds the picture too (something read it).
    in_texture: bool,
    /// A PyroWave picture's draw was added to the stage times.
    timed: bool,
    /// What the stream's pictures are, for the one line that says which way they are drawn
    /// ("PyroWave", "planar", "4:4:4").
    what: &'static str,
}

/// Which plane-shader configuration owns `VideoTexture.planes` on Linux and macOS: tracked
/// explicitly so transitioning between modes that share identical shader `parameters` (such as
/// `FRAME_VULKAN` and semi-planar `FRAME_NV12`/`FRAME_P010` HLG) recreates the pass instead of
/// mismatching it.
// Linux and macOS (unix): SDL GPU plane-pass mode tag (tested everywhere).
#[cfg(any(unix, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanesMode {
    PyroWave,
    Planar,
    SemiPlanar,
    Vulkan,
}

/// The renderer's texture holding the current picture.
pub struct VideoTexture {
    #[cfg(windows)]
    direct_packed: bool,
    pub texture: *mut SDL_Texture,
    width: i32,
    height: i32,
    /// The part of the current decoded picture that is the stream, without an encoder's alignment
    /// padding (`crop::visible_size`); None when the whole texture is the picture.
    visible: Option<(i32, i32)>,
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
    /// Active plane-pass mode (`PlanesMode`), cleared whenever `planes` is dropped.
    // Linux and macOS (unix): plane-pass mode tracking.
    #[cfg(unix)]
    planes_mode: Option<PlanesMode>,
    /// Set during an upload when previously held/sampled planes were released or dropped so
    /// `run_upload` does not restore stale `planes_pending` on failure.
    // Linux and macOS (unix): plane-pass invalidation guard.
    #[cfg(unix)]
    planes_invalidated: bool,
    /// Vulkan video pictures' planes, copied on the GPU, which `planes` draws (dropped after it).
    #[cfg(unix)]
    vulkan: Option<crate::vulkan_video::Planes>,
    /// The current picture's planes, copied for `planes` and not yet drawn, with the parameters to
    /// draw them with: `draw_direct` draws them into the back buffer, `sdl_texture` into the
    /// texture when something reads it (an upscaler, or SDL after a failed direct draw).
    #[cfg(unix)]
    planes_pending: Option<[u32; 8]>,
    /// A PyroWave picture's stage times wait for its deferred conversion (`passes_drawn`).
    #[cfg(unix)]
    pyrowave_stages_open: bool,
    /// Drawing the planes into the back buffer failed: they go through the texture for the rest
    /// of the stream.
    #[cfg(unix)]
    direct_planes_failed: bool,
    /// This frame is video only (`VideoOnly`): the plane pass writes PQ into the window's target.
    #[cfg(target_os = "linux")]
    pq_target: bool,
    #[cfg(target_os = "linux")]
    video_only: VideoOnly,
    /// The render state egui's meshes write PQ with in a video-only frame, once one was asked for.
    #[cfg(target_os = "linux")]
    ui_pq: Option<super::ui_pq::UiPq>,
    /// It could not be made: frames with the overlay go through the back buffer.
    #[cfg(target_os = "linux")]
    ui_pq_failed: bool,
    /// Why the last `upload` failed, when it knows more than the caller can say.
    pub failure: Option<String>,
    /// The last failure written to the log, so a failing picture stream logs each reason once.
    logged_failure: Option<String>,
    /// The last picture of a GPU-decoded or planar stream carried the PQ transfer.
    hdr_frame: bool,
    /// The last picture carried the HLG transfer (ARIB STD-B67).
    hlg_frame: bool,
    /// The last picture's chroma sat midway between luma columns (`media::centred_chroma`).
    centred_chroma: bool,
    /// Measurement: CPU time per PyroWave picture in `pyrowave_gpu`, kept across `destroy`.
    pub pyrowave_stages: super::stage_times::Stages<2>,
    packed_format: Option<[u32; 12]>,
    white_level: f32,
    /// The HDR headroom the texture was made with on its own (`ensure_with`), None for SDL's.
    texture_headroom: Option<f32>,
    hdr_mode: Option<bool>,
    hdr_metadata: Option<nightjar_core::control::HdrMetadata>,
    /// The session negotiated HDR (asked for it and got a 10-bit format), for pictures before or
    /// without the host's HDR mode; a 10-bit SDR stream is not HDR.
    hdr_stream: bool,
    /// Incremented on each `destroy()`, so callers holding a picture's epoch notice texture resets.
    destroy_epoch: u64,
    /// The texture's planes are imported DMA-BUFs (`crate::dmabuf`): its NV12 shader reads the
    /// chroma plane as red-green, where uploaded chroma is luminance-alpha.
    #[cfg(unix)]
    imported: bool,
    #[cfg(unix)]
    importer: Option<crate::dmabuf::Importer>,
    /// The picture whose surface the imported texture samples, kept from the decoder until the
    /// next picture replaces it.
    #[cfg(unix)]
    held: Option<media::Frame>,
    /// A picture could not be imported: the stream's pictures are mapped to memory from then on.
    #[cfg(unix)]
    dmabuf_declined: bool,
    /// The pass that draws D3D11VA pictures into the back buffer, for the device it was made on.
    #[cfg(windows)]
    direct: Option<(usize, super::direct::Direct)>,
    /// The pass failed: SDL draws the pictures for the rest of the stream.
    #[cfg(windows)]
    direct_failed: bool,
    /// The decoder's picture, held instead of copied while the pass reads it in place.
    #[cfg(windows)]
    held: Option<media::Frame>,
    /// The video texture holds the current picture.
    #[cfg(windows)]
    copied: bool,
    /// The current picture's planes, not drawn into the video texture: `draw_direct` draws them into
    /// the back buffer, `materialize_planes` into the texture when something reads it.
    #[cfg(windows)]
    direct_planes: Option<DirectPlanes>,
    /// Drawing the planes into the back buffer failed: they go through the texture for the rest
    /// of the stream.
    #[cfg(windows)]
    direct_planes_failed: bool,
    /// Which way the stream's planes are drawn was logged.
    #[cfg(windows)]
    direct_planes_said: bool,
    /// The display does not place the stream's pictures one texel to one pixel, so they go through
    /// the video texture, which was logged.
    #[cfg(any(windows, unix))]
    direct_planes_scaled_said: bool,
    /// The current picture came from D3D11VA, and its HDR10+ peak in nits.
    #[cfg(windows)]
    d3d11_picture: bool,
    #[cfg(windows)]
    hdr10_plus: Option<f32>,
    /// The current picture's Dolby Vision peak in nits (`media::Frame::dolby_vision`).
    #[cfg(windows)]
    dolby_vision: Option<f32>,
    /// A profile 5 (IPT) RPU was logged: its pictures are drawn as HDR10, once said.
    #[cfg(windows)]
    dolby_vision_ipt_said: bool,
    /// The current picture converted to RGB by the pass, for SDL (`sdl_texture`): 10-bit SDR as
    /// gamma-coded RGB, HLG as linear light (`converted_linear`). The view the pass writes it
    /// through, and whether it holds the current picture.
    #[cfg(windows)]
    converted: *mut SDL_Texture,
    #[cfg(windows)]
    converted_linear: bool,
    #[cfg(windows)]
    converted_view: Option<windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView>,
    #[cfg(windows)]
    converted_current: bool,
    /// Frame Warp's shift for the next draw of the picture into the window (identity without it);
    /// the presenter sets it before each present.
    pub warp: super::frame_warp::WarpDraw,
    /// The triangles of a shifted draw through SDL's renderer, kept across presents.
    pub warp_mesh: super::frame_warp::Mesh,
}

impl Default for VideoTexture {
    fn default() -> Self {
        Self {
            #[cfg(windows)]
            direct_packed: true,
            texture: std::ptr::null_mut(),
            width: 0,
            height: 0,
            visible: None,
            format: SDL_PIXELFORMAT_UNKNOWN,
            colorspace: SDL_COLORSPACE_UNKNOWN,
            d3d11: std::ptr::null_mut(),
            packed: None,
            pyrowave: None,
            pyrowave_format: None,
            #[cfg(any(windows, unix))]
            planes: None,
            // Linux and macOS (unix): plane-pass mode tracking.
            #[cfg(unix)]
            planes_mode: None,
            // Linux and macOS (unix): plane-pass invalidation guard.
            #[cfg(unix)]
            planes_invalidated: false,
            #[cfg(unix)]
            vulkan: None,
            #[cfg(unix)]
            planes_pending: None,
            #[cfg(unix)]
            pyrowave_stages_open: false,
            #[cfg(unix)]
            direct_planes_failed: false,
            #[cfg(target_os = "linux")]
            pq_target: false,
            #[cfg(target_os = "linux")]
            video_only: VideoOnly::from_environment(),
            #[cfg(target_os = "linux")]
            ui_pq: None,
            #[cfg(target_os = "linux")]
            ui_pq_failed: false,
            failure: None,
            logged_failure: None,
            hdr_frame: false,
            hlg_frame: false,
            centred_chroma: false,
            pyrowave_stages: super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES),
            packed_format: None,
            white_level: 1.0,
            texture_headroom: None,
            hdr_mode: None,
            hdr_metadata: None,
            hdr_stream: false,
            destroy_epoch: 0,
            #[cfg(unix)]
            imported: false,
            #[cfg(unix)]
            importer: None,
            #[cfg(unix)]
            held: None,
            #[cfg(unix)]
            dmabuf_declined: false,
            #[cfg(windows)]
            direct: None,
            #[cfg(windows)]
            direct_failed: false,
            #[cfg(windows)]
            held: None,
            #[cfg(windows)]
            copied: false,
            #[cfg(windows)]
            direct_planes: None,
            #[cfg(windows)]
            direct_planes_failed: false,
            #[cfg(windows)]
            direct_planes_said: false,
            #[cfg(any(windows, unix))]
            direct_planes_scaled_said: false,
            #[cfg(windows)]
            d3d11_picture: false,
            #[cfg(windows)]
            hdr10_plus: None,
            #[cfg(windows)]
            dolby_vision: None,
            #[cfg(windows)]
            dolby_vision_ipt_said: false,
            #[cfg(windows)]
            converted: std::ptr::null_mut(),
            #[cfg(windows)]
            converted_linear: false,
            #[cfg(windows)]
            converted_view: None,
            #[cfg(windows)]
            converted_current: false,
            warp: super::frame_warp::WarpDraw::default(),
            warp_mesh: super::frame_warp::Mesh::default(),
        }
    }
}

/// The pass for the renderer's device, made on first use or after the device changed.
#[cfg(windows)]
fn direct_pass(direct: &mut Option<(usize, super::direct::Direct)>, renderer: *mut SDL_Renderer) -> Result<&mut super::direct::Direct, String> {
    let device = crate::render_device::live_renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
    if !direct.as_ref().is_some_and(|(made_on, _)| *made_on == device as usize) {
        *direct = None;
        // SAFETY: The renderer's live device, used on the presenting thread.
        let made = unsafe {
            use windows::core::Interface;
            let device = windows::Win32::Graphics::Direct3D11::ID3D11Device::from_raw_borrowed(&device)
                .ok_or("the renderer has no Direct3D 11 device")?;
            super::direct::Direct::new(device)?
        };
        *direct = Some((device as usize, made));
    }
    Ok(&mut direct.as_mut().unwrap().1)
}

/// Runs a submission that reads `texture` under the decoder's lock when it is the held
/// decoder picture's.
#[cfg(windows)]
fn lock_for(held: &Option<media::Frame>, texture: *mut c_void, submit: &mut dyn FnMut()) {
    match held {
        Some(frame) if frame.d3d11_surface().is_some_and(|surface| surface.texture == texture) => frame.with_decoder_lock(submit),
        _ => submit(),
    }
}

impl VideoTexture {
    /// `hdr_stream`: the session negotiated HDR.
    pub fn new(_direct_packed: bool, hdr_stream: bool) -> Self {
        Self { #[cfg(windows)] direct_packed: _direct_packed, hdr_stream, ..Default::default() }
    }

    pub fn destroy_epoch(&self) -> u64 {
        self.destroy_epoch
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

    /// Records why a picture cannot be shown: `short` for the caller's message, `detail` once in
    /// the log (a failing stream retries every picture).
    fn fail(&mut self, detail: String, short: String) {
        if self.logged_failure.as_deref() != Some(detail.as_str()) {
            eprintln!("nightjar: {detail}");
            self.logged_failure = Some(detail);
        }
        self.failure = Some(short);
    }

    pub fn is_hdr(&self) -> bool {
        self.hdr_mode.unwrap_or(
            self.hdr_frame
                || self.hlg_frame
                || self
                    .packed_format
                    .is_some_and(|parameters| parameters[7] != 0),
        )
    }

    /// The picture can only be shown on an HDR output. An HLG picture can be shown on an SDR one
    /// too: the draw pass and the plane shader draw it for HLG's reference display, tone mapped.
    #[cfg(test)]
    pub fn needs_hdr_output(&self) -> bool {
        self.is_hdr() && !self.hlg_frame
    }

    /// The host's HDR mastering metadata while its HDR mode is on.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn hdr_metadata(&self) -> Option<nightjar_core::control::HdrMetadata> {
        // HLG carries no mastering metadata and keeps today's path.
        self.hdr_metadata.filter(|_| self.hdr_mode == Some(true) && !self.hlg_frame)
    }

    /// What a display plane (`kms_scanout`) shows `decoded` as: None for SDR, the host's mastering
    /// metadata (empty before it sent any) for an HDR10 picture. HLG and 8-bit HDR pictures stay
    /// with the renderer, which converts them.
    #[cfg(target_os = "linux")]
    pub fn scanout_hdr(&self, decoded: &super::pipeline::Decoded) -> Result<Option<nightjar_core::control::HdrMetadata>, String> {
        let trc = decoded.picture.frame().map_or(0, |frame| frame.info().color_trc);
        if !shows_hdr(self.hdr_mode, self.hdr_stream, trc) {
            return Ok(None);
        }
        if trc == TRC_HLG {
            return Err("HLG pictures are converted by the renderer".into());
        }
        if decoded.setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT == 0 {
            return Err("an 8-bit HDR picture is converted by the renderer".into());
        }
        Ok(Some(self.hdr_metadata.filter(|_| self.hdr_mode == Some(true)).unwrap_or_default()))
    }

    /// The picture's size: the texture's, less the alignment padding of a decoded picture.
    pub fn size(&self) -> (i32, i32) {
        self.visible.unwrap_or((self.width, self.height))
    }

    /// The part of `texture` that holds the picture when it is the video texture with alignment
    /// padding, for SDL to draw; None for the whole texture. It ends half a texel inside the
    /// picture, so bilinear sampling at the edge never reads the first padding texel.
    pub fn source_rect(&self, texture: *mut SDL_Texture) -> Option<SDL_FRect> {
        let (w, h) = self.size();
        (!texture.is_null() && texture == self.texture && (w, h) != (self.width, self.height))
            .then_some(SDL_FRect { x: 0.0, y: 0.0, w: w as f32 - 0.5, h: h as f32 - 0.5 })
    }

    /// The picture AI enhancement can read: a hardware-decoded NV12 or P010 Direct3D 11 texture.
    #[cfg(windows)]
    pub fn enhance_source(&self) -> Option<super::enhance::Source> {
        let ten_bit = match self.format {
            SDL_PIXELFORMAT_NV12 => false,
            SDL_PIXELFORMAT_P010 => true,
            _ => return None,
        };
        (!self.texture.is_null() && !self.d3d11.is_null()).then_some(super::enhance::Source {
            texture: self.texture, d3d11: self.d3d11, picture: self.size(), ten_bit, colorspace: self.colorspace, transfer: self.transfer(),
        })
    }

    /// The texture SDL draws or the upscalers read for the current picture: the video texture
    /// (with a held picture copied in, or on Linux the pending planes drawn into it), or for a
    /// 10-bit SDR or an HLG picture, which SDL's shaders would decode as PQ or gamma, its RGB
    /// conversion by the pass.
    #[cfg(windows)]
    pub fn sdl_texture(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_Texture {
        self.materialize_planes();
        if self.texture.is_null() || !self.d3d11_picture || !sdl_needs_rgb(self.format, self.transfer()) {
            self.copy_held();
            return self.texture;
        }
        if !self.converted_current {
            if let Err(error) = self.convert(renderer) {
                let what = if self.transfer() == super::direct::Transfer::Hlg { "an HLG" } else { "a 10-bit SDR" };
                self.fail(format!("converting {what} picture for SDL failed: {error}"), "the GPU conversion failed".into());
                self.copy_held();
                return self.texture;
            }
            self.converted_current = true;
        }
        self.converted
    }

    #[cfg(unix)]
    pub fn sdl_texture(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_Texture {
        if let Err(error) = self.draw_pending_planes(renderer) {
            self.fail(format!("drawing the picture's planes into the video texture failed: {error}"), error);
        }
        self.texture
    }

    #[cfg(not(any(windows, unix)))]
    pub fn sdl_texture(&mut self, _renderer: *mut SDL_Renderer) -> *mut SDL_Texture {
        self.texture
    }

    #[cfg(windows)]
    fn convert(&mut self, renderer: *mut SDL_Renderer) -> Result<(), String> {
        use windows::core::Interface;
        let picture = self.picture().ok_or("no picture to convert")?;
        // HLG becomes linear light relative to SDR white in a float texture, as the PyroWave and
        // packed pictures SDL draws; 10-bit SDR gamma-coded RGB.
        let linear = picture.transfer == super::direct::Transfer::Hlg;
        // SAFETY: A live texture of this renderer.
        let made_for = (!self.converted.is_null()).then(|| unsafe { ((*self.converted).w, (*self.converted).h) });
        if !self.converted.is_null() && (self.converted_linear != linear || made_for != Some(self.size())) {
            self.converted_view = None;
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.converted) };
            self.converted = std::ptr::null_mut();
        }
        if self.converted.is_null() {
            // SAFETY: SDL calls on the presenting thread's live renderer.
            unsafe {
                let (format, space) = if linear { (SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR) } else { (SDL_PIXELFORMAT_ABGR2101010, SDL_COLORSPACE_SRGB) };
                let props = SDL_CreateProperties();
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, i64::from(SDL_TEXTUREACCESS_TARGET.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(self.size().0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, i64::from(self.size().1));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
                self.converted = SDL_CreateTextureWithProperties(renderer, props);
                self.converted_linear = linear;
                SDL_DestroyProperties(props);
                if self.converted.is_null() { return Err(format!("SDL could not make the RGB texture: {}", crate::sdl::error())); }
                SDL_SetTextureScaleMode(self.converted, SDL_SCALEMODE_LINEAR);
                let raw = SDL_GetPointerProperty(SDL_GetTextureProperties(self.converted), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, std::ptr::null_mut());
                let texture = windows::Win32::Graphics::Direct3D11::ID3D11Texture2D::from_raw_borrowed(&raw).ok_or("the RGB texture has no Direct3D 11 texture")?;
                let mut view = None;
                texture.GetDevice().and_then(|device| device.CreateRenderTargetView(texture, None, Some(&mut view)))
                    .map_err(|e| format!("the RGB texture's view: {e}"))?;
                self.converted_view = view;
            }
        }
        let view = self.converted_view.clone().ok_or("the RGB texture has no view")?;
        let (size, white_nits) = ((self.size().0 as u32, self.size().1 as u32), 80.0 * self.white_level);
        // SAFETY: The live renderer belongs to the presenting thread.
        let headroom = unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0) };
        self.wait_held_d3d11(picture.texture)?;
        let pass = direct_pass(&mut self.direct, renderer)?;
        let held = &self.held;
        // SAFETY: The picture's texture and the view belong to the renderer's device.
        unsafe { pass.convert(&view, size, &picture, white_nits, headroom, |submit| lock_for(held, picture.texture, submit)) }
    }

    /// The current D3D11VA picture for the pass: the held decoder slice or the copied texture.
    #[cfg(windows)]
    fn picture(&self) -> Option<super::direct::Picture> {
        let ten_bit = match self.format {
            SDL_PIXELFORMAT_NV12 => false,
            SDL_PIXELFORMAT_P010 => true,
            _ => return None,
        };
        if !self.d3d11_picture || self.texture.is_null() {
            return None;
        }
        let (texture, slice) = match &self.held {
            Some(frame) => frame.d3d11_surface().map(|surface| (surface.texture, surface.slice))?,
            None if self.copied && !self.d3d11.is_null() => (self.d3d11, 0),
            None => return None,
        };
        let (matrix, full_range) = direct_matrix(self.colorspace);
        Some(super::direct::Picture {
            texture, slice, size: (self.size().0 as u32, self.size().1 as u32), ten_bit, full_range, matrix, transfer: self.transfer(),
            centred_chroma: self.centred_chroma,
        })
    }

    /// Draws `texture` into the back buffer with `stream::direct` and returns true, or returns
    /// false for SDL to draw it. `texture` is the video texture or AI enhancement's output; the
    /// pass draws the D3D11VA picture of either, and nothing else. A failing pass is logged
    /// once and SDL draws from then on.
    /// Lets go of the direct passes' back-buffer views, for a frame whose upscaler or AI
    /// enhancement runs SDL's queue before `draw_direct`: SDL resizes there, and a held view
    /// would make its `ResizeBuffers` fail.
    #[cfg(windows)]
    pub fn release_back_buffers(&mut self) {
        if let Some(pass) = &mut self.planes { pass.release_back_buffer(); }
        if let Some((_, pass)) = &mut self.direct { pass.release_back_buffer(); }
    }

    pub fn draw(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect) -> bool {
        #[cfg(windows)]
        if self.texture.is_null()
            && let Some(packed) = &mut self.packed {
                return packed.present_warped(
                    renderer,
                    self.packed_format.as_ref().unwrap(),
                    destination,
                    &self.warp,
                );
            }
        if !self.texture.is_null() {
            let source = self.source_rect(self.texture);
            // SAFETY: The renderer and its video texture are live on this thread.
            unsafe { super::frame_warp::render_texture(renderer, self.texture, source.as_ref(), destination, &self.warp, &mut self.warp_mesh) };
        }
        true
    }

    pub fn destroy(&mut self) {
        // While the pass that reads it is still there: the set goes back behind its last draw.
        #[cfg(windows)]
        self.finish_direct();
        #[cfg(any(windows, unix))]
        { self.planes = None; }
        #[cfg(target_os = "linux")]
        { self.ui_pq = None; }
        self.pyrowave = None;
        self.pyrowave_format = None;
        self.packed = None;
        self.packed_format = None;
        if !self.texture.is_null() {
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.texture) };
        }
        #[cfg(windows)]
        if !self.converted.is_null() {
            self.converted_view = None;
            // SAFETY: a texture we created.
            unsafe { SDL_DestroyTexture(self.converted) };
        }
        // The stage timers belong to the whole stream, not to one texture.
        let stages = std::mem::replace(&mut self.pyrowave_stages, super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES));
        let next_epoch = self.destroy_epoch.wrapping_add(1);
        *self = Self {
            #[cfg(windows)]
            direct_packed: self.direct_packed,
            destroy_epoch: next_epoch,
            failure: self.failure.take(),
            hdr_mode: self.hdr_mode,
            hdr_metadata: self.hdr_metadata,
            hdr_stream: self.hdr_stream,
            // Facts about the stream, not the texture: `ensure` calls `destroy` before it reads them.
            hdr_frame: self.hdr_frame,
            hlg_frame: self.hlg_frame,
            centred_chroma: self.centred_chroma,
            #[cfg(windows)]
            dolby_vision_ipt_said: self.dolby_vision_ipt_said,
            logged_failure: self.logged_failure.take(),
            pyrowave_stages: stages,
            #[cfg(unix)]
            dmabuf_declined: self.dmabuf_declined,
            #[cfg(unix)]
            direct_planes_failed: self.direct_planes_failed,
            #[cfg(target_os = "linux")]
            video_only: std::mem::replace(&mut self.video_only, VideoOnly::of(None)),
            // The pass outlives the texture; the next pictures are a new scene to it.
            #[cfg(windows)]
            direct: self.direct.take().map(|(device, mut pass)| { pass.reset(); (device, pass) }),
            #[cfg(windows)]
            direct_failed: self.direct_failed,
            #[cfg(windows)]
            direct_planes_failed: self.direct_planes_failed,
            #[cfg(windows)]
            direct_planes_said: self.direct_planes_said,
            #[cfg(any(windows, unix))]
            direct_planes_scaled_said: self.direct_planes_scaled_said,
            ..Self::default()
        };
    }
}

/// The planes to draw after an upload: the new picture's, or when it failed the last one's while
/// the pass and texture that hold them (`held`) are still there and the pass was made for them
/// (`pass_format`, its parameters: a change of shape makes a new pass, whose images never held
/// them), so the last good picture stays on screen and SDL never draws the texture the planes
/// were never drawn into.
#[cfg_attr(not(unix), allow(dead_code))]
fn planes_after_upload(uploaded: bool, new: Option<[u32; 8]>, last: Option<[u32; 8]>, held: bool, pass_format: Option<[u32; 8]>) -> Option<[u32; 8]> {
    if uploaded || new.is_some() || !held || pass_format != last { new } else { last }
}

/// Whether an existing Linux/macOS `PlanesPass` of `current` mode must be recreated before running
/// `wanted` mode even when shader `parameters` match.
// Linux and macOS (unix): plane-pass mode transition check (tested everywhere).
#[cfg(any(unix, test))]
pub(super) fn planes_mode_mismatch(current: Option<PlanesMode>, wanted: PlanesMode) -> bool {
    current.is_some_and(|mode| mode != wanted)
}

/// Visible crop after an upload attempt: updates from `visible` when `uploaded`, preserves the
/// prior crop when `!uploaded` while the prior texture remains alive, and clears when no texture
/// exists.
pub(super) fn visible_after_upload(
    previous: Option<(i32, i32)>,
    visible: Option<(i32, i32)>,
    uploaded: bool,
    is_plain: bool,
    texture_size: (i32, i32),
    has_texture: bool,
) -> Option<(i32, i32)> {
    if uploaded {
        visible.filter(|_| is_plain).and_then(|v| visible_part(v, texture_size))
    } else if has_texture {
        previous
    } else {
        None
    }
}

#[cfg(all(target_os = "macos", feature = "demo-library"))]
#[path = "macos_video_check.rs"]
pub(super) mod macos_video_check;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(target_os = "macos")]
impl VideoTexture {
    pub(super) fn metalfx_source(&self) -> Option<(*mut c_void, media::FrameInfo)> {
        let frame = self.held.as_ref()?;
        let info = frame.info();
        if info.kind != media::FRAME_VIDEOTOOLBOX || self.is_hdr() { return None; }
        let buffer: *mut c_void = unsafe { frame.raw.as_ref().data[3].cast() };
        (!buffer.is_null()).then_some((buffer, info))
    }
}
