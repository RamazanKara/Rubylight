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

use rubylight_protocol::present_clock::{PresentClock, Settings as ClockSettings};
use sdl3_sys::everything::*;

use super::pipeline::Decoded;
use crate::config::FramePacing;
use crate::media::{self, FrameInfo};

mod direct_draw;
mod pacer;
mod planes;
mod upload;

pub use pacer::*;

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
            pyrowave_stages: super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES),
            packed_format: None,
            white_level: 1.0,
            texture_headroom: None,
            hdr_mode: None,
            hdr_metadata: None,
            hdr_stream: false,
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

/// Whether the session negotiated HDR: the client asked for it and the host chose a 10-bit format.
/// With "10-bit color (SDR)" a 10-bit format alone is SDR.
pub fn hdr_negotiated(asked_hdr: bool, negotiated_format: u32) -> bool {
    asked_hdr && negotiated_format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0
}

/// FFmpeg's AVCOL_TRC_SMPTE2084 (PQ) and AVCOL_TRC_ARIB_STD_B67 (HLG).
const TRC_PQ: i32 = 16;
const TRC_HLG: i32 = 18;

/// Whether a picture is shown as HDR: as the host's HDR mode says once it has, else when it
/// carries the PQ or HLG transfer or the session negotiated HDR. A 10-bit format alone is not HDR.
fn shows_hdr(hdr_mode: Option<bool>, hdr_stream: bool, color_trc: i32) -> bool {
    hdr_mode.unwrap_or(hdr_stream || color_trc == TRC_PQ || color_trc == TRC_HLG)
}

/// The plane shader's constants for a software-decoded planar picture: its colour description is
/// FFmpeg's, read as `colorspace` reads it (the shader's matrix 0 is BT.601, 2 BT.2020, else BT.709).
/// An HDR picture with the HLG transfer gets the shader's HLG flag (128), never PQ, with the
/// display it is drawn for (`planar_hlg`) in place of the wavelet pass's level and operation,
/// and that display's white as the SDR white.
pub(super) fn planar_parameters(info: &FrameInfo, layout: media::PlanarLayout, hdr: bool, white_level: f32, headroom: f32) -> [u32; 8] {
    let matrix = match info.color_space { 5 | 6 => 0, 9 | 10 => 2, _ => 1 };
    let hlg = planar_hlg(info, hdr, white_level, headroom);
    let flags = u32::from(info.color_range == 2) | (u32::from(hdr) << 1) | (matrix << 2) | (u32::from(layout.ten_bit) << 4)
        | (u32::from(layout.chroma444) << 5) | (u32::from(layout.ten_bit) << 6) | (u32::from(hlg.is_some()) << 7);
    match hlg {
        Some(display) => [layout.width, layout.height, layout.width, layout.height, display.peak.to_bits(), display.gamma.to_bits(), flags, (display.white / 80.0).to_bits()],
        None => [layout.width, layout.height, layout.width, layout.height, 0, 3, flags, white_level.to_bits()],
    }
}

/// Whether a packed picture takes the other reconstruction than the one made: from planes for a
/// Vulkan picture (mapped VAAPI or Vulkan video, `vulkan_picture`) where the made one reads memory
/// (no `planes`), or the reverse, as when a new decoder's map fails.
#[cfg(any(unix, test))]
fn packed_path_changed(reconstruction: bool, vulkan_picture: bool, planes: bool) -> bool {
    reconstruction && vulkan_picture != planes
}

/// `planar_parameters` for Vulkan video's copied planes when `vulkan`: flag 256 (the chroma
/// interleaved, 10-bit samples in the high bits) in place of 64 (in the low bits).
pub(super) fn vulkan_flags(mut parameters: [u32; 8], vulkan: bool) -> [u32; 8] {
    if vulkan {
        parameters[6] = parameters[6] & !64 | 256;
    }
    parameters
}

/// `vulkan_flags` for the one copied image of a mapped 4:4:4 VAAPI picture when `interleaved`
/// (`media::FRAME_VULKAN_INTERLEAVED444`): flag 2048 in place of 256, Y, Cb and Cr read from one
/// texel, its 10-bit samples whole.
pub(super) fn interleaved_flags(mut parameters: [u32; 8], interleaved: bool) -> [u32; 8] {
    if interleaved {
        parameters[6] = parameters[6] & !256 | 2048;
    }
    parameters
}

/// The display a plane shader picture shown as HDR with the HLG transfer is drawn for, as the
/// Windows draw pass draws it (`hlg::hlg_display`): an HDR output's own peak (`headroom` over the
/// SDR white of `white_level` x 80 nits), else HLG's 1000-nit reference display with its
/// reference white at SDR white, which SDL's tone map brings into SDR. None for other pictures.
pub(super) fn planar_hlg(info: &FrameInfo, hdr: bool, white_level: f32, headroom: f32) -> Option<super::hlg::HlgDisplay> {
    (hdr && info.color_trc == TRC_HLG).then(|| super::hlg::hlg_display(headroom, 80.0 * white_level))
}

/// Whether a picture SDL would show (NV12 or P010 from memory, or an imported VAAPI surface) goes
/// to the plane shader instead: SDL reads P010 as PQ and knows no HLG, so an HLG one is drawn as a
/// 4:2:0 planar picture with the shader's HLG flag. SDR and PQ pictures keep SDL's upload or
/// import as they are.
#[cfg(unix)]
pub(super) fn hlg_through_planes(info: &FrameInfo) -> bool {
    info.color_trc == TRC_HLG && matches!(info.kind, media::FRAME_NV12 | media::FRAME_P010 | media::FRAME_DMABUF)
}

/// FFmpeg's colour description as an SDL colorspace.
fn colorspace(info: &FrameInfo) -> SDL_Colorspace {
    // AVCOL_RANGE_JPEG = 2; AVCOL_SPC_BT709 = 1, BT470BG = 5, SMPTE170M = 6, BT2020_NCL = 9;
    // AVCOL_TRC_SMPTE2084 = 16.
    let full = info.color_range == 2;
    // A PQ picture is YCbCr BT.2020 (SDL's BT2020_* spaces carry the PQ transfer). SDL_COLORSPACE_HDR10
    // is an RGB space with the identity matrix: SDL's D3D11 renderer cannot make a P010 texture in it
    // ("Unsupported YUV colorspace"), which showed as "Cannot show d3d11 pictures".
    if info.color_trc == 16 {
        return if full { SDL_COLORSPACE_BT2020_FULL } else { SDL_COLORSPACE_BT2020_LIMITED };
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

/// The YCbCr matrix (0 BT.601, 1 BT.709, 2 BT.2020) and full range of an SDL colorspace from
/// `colorspace`, for `stream::direct`.
#[cfg(windows)]
fn direct_matrix(space: SDL_Colorspace) -> (u32, bool) {
    match space {
        SDL_COLORSPACE_BT601_LIMITED => (0, false),
        SDL_COLORSPACE_BT601_FULL => (0, true),
        SDL_COLORSPACE_BT2020_LIMITED => (2, false),
        SDL_COLORSPACE_BT2020_FULL => (2, true),
        SDL_COLORSPACE_BT709_FULL => (1, true),
        _ => (1, false),
    }
}

/// A decoded picture's transfer, one decision for every path that shows it. When the stream
/// shows HDR (`shows_hdr`: the host's HDR mode, else the PQ or HLG transfer or the negotiated
/// HDR), a picture with the HLG transfer is HLG and any other P010 picture PQ, even when the
/// bitstream does not signal PQ. Everything else is SDR: an NV12 picture is never PQ.
#[cfg(windows)]
fn picture_transfer(format: SDL_PixelFormat, hdr_mode: Option<bool>, hdr_stream: bool, hdr_frame: bool, hlg_frame: bool) -> super::direct::Transfer {
    use super::direct::Transfer;
    let trc = if hlg_frame { TRC_HLG } else if hdr_frame { TRC_PQ } else { 0 };
    if !matches!(format, SDL_PIXELFORMAT_P010 | SDL_PIXELFORMAT_NV12) || !shows_hdr(hdr_mode, hdr_stream, trc) {
        Transfer::Sdr
    } else if hlg_frame {
        Transfer::Hlg
    } else if format == SDL_PIXELFORMAT_P010 {
        Transfer::Pq
    } else {
        Transfer::Sdr
    }
}

/// SDL's D3D11 shaders decode every P010 texture as PQ and NV12 as gamma, and know no HLG: a
/// picture whose `picture_transfer` differs reaches SDL converted to RGB by the pass.
#[cfg(windows)]
fn sdl_needs_rgb(format: SDL_PixelFormat, transfer: super::direct::Transfer) -> bool {
    use super::direct::Transfer;
    let sdl_reads = if format == SDL_PIXELFORMAT_P010 { Transfer::Pq } else { Transfer::Sdr };
    sdl_reads != transfer
}

/// The pass for the renderer's device, made on first use or after the device changed.
#[cfg(windows)]
fn direct_pass(direct: &mut Option<(usize, super::direct::Direct)>, renderer: *mut SDL_Renderer) -> Result<&mut super::direct::Direct, String> {
    let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
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

/// The HDR properties of a texture request.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HdrProperties {
    /// `SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT`, in nits (HDR10 textures only).
    white_point: Option<f32>,
    /// `SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT`, relative to the SDR white.
    headroom: Option<f32>,
}

/// The white point and headroom a texture is created with. `white_level` is the renderer's SDR
/// white as a multiple of 80 nits; a value that is not a positive finite number is not passed on.
fn hdr_properties(hdr10: bool, white_level: f32, metadata: Option<nightjar_core::control::HdrMetadata>) -> HdrProperties {
    let white_nits = 80.0 * white_level;
    if !white_nits.is_finite() || white_nits <= 0.0 {
        return HdrProperties { white_point: None, headroom: None };
    }
    let headroom = metadata.and_then(|metadata| {
        let peak = if metadata.max_content_light_level != 0 {
            metadata.max_content_light_level
        } else {
            metadata.max_display_luminance
        };
        (peak > 0).then(|| f32::from(peak) / white_nits)
    });
    HdrProperties { white_point: hdr10.then_some(white_nits), headroom }
}

fn pixel_format_name(format: SDL_PixelFormat) -> String {
    let name = SDL_GetPixelFormatName(format);
    if name.is_null() {
        return format!("format {:#x}", format.0);
    }
    // SAFETY: checked non-null above.
    unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned()
}

impl VideoTexture {
    /// `hdr_stream`: the session negotiated HDR.
    pub fn new(_direct_packed: bool, hdr_stream: bool) -> Self {
        Self { #[cfg(windows)] direct_packed: _direct_packed, hdr_stream, ..Default::default() }
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
        let size = (self.size().0 as u32, self.size().1 as u32);
        // SAFETY: The live renderer belongs to the presenting thread.
        let headroom = unsafe { SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0) };
        let white_nits = 80.0 * self.white_level;
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
        if self.texture.is_null() {
            if let Some(packed) = &mut self.packed {
                return packed.present_warped(
                    renderer,
                    self.packed_format.as_ref().unwrap(),
                    destination,
                    &self.warp,
                );
            }
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
        *self = Self {
            #[cfg(windows)]
            direct_packed: self.direct_packed,
            failure: self.failure.take(),
            hdr_mode: self.hdr_mode,
            hdr_metadata: self.hdr_metadata,
            hdr_stream: self.hdr_stream,
            // Facts about the stream, not the texture: `ensure` calls `destroy` before it reads them.
            hdr_frame: self.hdr_frame,
            hlg_frame: self.hlg_frame,
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

/// `0` keeps every HDR frame on the PQ pass through the back buffer, as before video-only frames.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const VIDEO_ONLY_ENVIRONMENT: &str = "NIGHTJAR_PQ_VIDEO_ONLY";

/// How long the first frame that lost draws holds video-only frames off; each further one doubles it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VIDEO_ONLY_HOLD_US: u64 = 2_000_000;

/// After this many frames that lost draws, a stream draws no more video-only frames.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const VIDEO_ONLY_MISSES: u32 = 4;

/// What a frame on the window's target will draw, for whether it can be video only.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoOnlyFrame {
    /// The renderer presents through the patched SDL's PQ pass (`handheld::Output::Pq`).
    pub pq_output: bool,
    /// The plane pass draws the picture straight into the window's target (`draws_planes_direct`),
    /// with no upscaler or AI enhancement drawing it instead.
    pub planes_direct: bool,
    /// Nothing else draws there that SDL would draw itself: no Remote Input screen, and the
    /// overlay, a toast or the stream menu only as meshes that write PQ (`ui_pq::UiPq`).
    pub nothing_else: bool,
    /// A read-back of the frame is asked for (a screenshot), which reads the back buffer as scRGB.
    pub read_back: bool,
}

/// HDR frames that draw only the picture, through the patched SDL's `SDL.renderer.gpu.video_only`
/// (`vendor/sdl3-src/README.md`): the plane pass writes PQ into the HDR10 swapchain image itself,
/// and SDL skips the RGBA16F back buffer and its full-screen PQ pass. SDL drops its own draws in
/// such a frame and counts them; a frame that lost any (something drew that `VideoOnlyFrame` did
/// not foresee) holds video-only frames off for a while, longer each time, and after
/// `VIDEO_ONLY_MISSES` for the rest of the stream.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug)]
pub struct VideoOnly {
    switched_on: bool,
    /// The path was logged.
    said: bool,
    /// SDL's count of the draws it dropped, as last read.
    dropped_seen: i64,
    misses: u32,
    held_until_us: u64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl VideoOnly {
    pub fn of(value: Option<&std::ffi::OsStr>) -> Self {
        Self { switched_on: !value.is_some_and(|value| value == "0"), said: false, dropped_seen: 0, misses: 0, held_until_us: 0 }
    }

    pub fn from_environment() -> Self {
        Self::of(std::env::var_os(VIDEO_ONLY_ENVIRONMENT).as_deref())
    }

    /// Whether to ask SDL for a video-only frame, and a line to log the first time one could be
    /// asked for while the environment switches them off.
    pub fn wanted(&mut self, frame: VideoOnlyFrame, now_us: u64) -> (bool, Option<&'static str>) {
        let could = frame.pq_output && frame.planes_direct && frame.nothing_else && !frame.read_back;
        let line = (could && !self.switched_on && !std::mem::replace(&mut self.said, true))
            .then_some("nightjar: HDR frames go through the back buffer and its PQ pass (NIGHTJAR_PQ_VIDEO_ONLY=0)");
        (could && self.switched_on && self.misses < VIDEO_ONLY_MISSES && now_us >= self.held_until_us, line)
    }

    /// What SDL settled for a frame asked to be video only; a line to log the first time.
    pub fn settled(&mut self, video_only: bool) -> Option<&'static str> {
        (!std::mem::replace(&mut self.said, true)).then_some(if video_only {
            "nightjar: HDR frames with only the picture take PQ straight from the plane pass, without the back buffer and its PQ pass"
        } else {
            "nightjar: SDL did not take a video-only frame; HDR frames go through the back buffer and its PQ pass"
        })
    }

    /// SDL's count of the draws it dropped in video-only frames, after a present; a line to log
    /// when this frame lost any.
    pub fn presented(&mut self, dropped: i64, now_us: u64) -> Option<String> {
        // A renderer made again counts from 0.
        let lost = if dropped < self.dropped_seen { 0 } else { dropped - self.dropped_seen };
        self.dropped_seen = dropped;
        if lost == 0 {
            return None;
        }
        self.misses += 1;
        if self.misses >= VIDEO_ONLY_MISSES {
            return Some(format!("nightjar: SDL dropped {lost} draws of a video-only HDR frame; HDR frames go through the PQ pass for the rest of the stream"));
        }
        let hold_us = VIDEO_ONLY_HOLD_US << (self.misses - 1);
        self.held_until_us = now_us + hold_us;
        Some(format!("nightjar: SDL dropped {lost} draws of a video-only HDR frame; HDR frames go through the PQ pass for {} s", hold_us / 1_000_000))
    }
}

/// A decoded H.264, HEVC or AV1 picture, which may carry an encoder's alignment padding; not a
/// PyroWave or packed 4:4:4 one, whose size the setup gives.
fn plain(decoded: &Decoded) -> bool {
    decoded.setup.chroma_packing == 0 && decoded.setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_PYROWAVE == 0
        && decoded.picture.frame().is_some()
}

/// The `visible` part of a `texture`-sized texture to show, when it is less than the texture;
/// None when the whole texture is, or the part would reach past it.
fn visible_part(visible: (i32, i32), texture: (i32, i32)) -> Option<(i32, i32)> {
    (visible != texture && visible.0 <= texture.0 && visible.1 <= texture.1).then_some(visible)
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

#[cfg(all(target_os = "macos", feature = "demo-library"))]
#[path = "macos_video_check.rs"]
pub(super) mod macos_video_check;

#[cfg(test)]
pub(crate) mod tests;
