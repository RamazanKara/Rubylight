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
    } else {
        without_vrr(settings)
    };
    (mode, vrr)
}

/// The pacing without VRR: V-Sync, or immediate when the settings turn V-Sync off and the frame
/// pacing is the lowest latency (every other frame pacing waits for the refresh).
pub fn without_vrr(settings: &crate::config::Settings) -> PacingMode {
    if settings.vsync || settings.frame_pacing.forces_vsync() { PacingMode::Vsync } else { PacingMode::Immediate }
}

/// The longest a picture waits in the in-order frame pacings before a newer due one replaces it:
/// one frame of the stream, at most 250 ms (Android's `FramePacingPolicy.shouldDrainOutput`).
pub fn frame_us(fps: u32) -> u64 {
    (1_000_000 / u64::from(fps.max(1))).min(250_000)
}

/// The pacing once the display check (`race_probe`) has a verdict: a VRR stream on a display that
/// scans at its own rate whatever is presented tears on nearly every present (99% of them on a
/// 120 Hz laptop panel), so it takes the pacing it would have without VRR (`fallback`, from
/// `without_vrr`): V-Sync, or immediate when the settings turn V-Sync off. Anything else keeps its
/// mode: still checking, VRR seen, or a reason that says nothing about the refresh (a window, no
/// scan line, no restart seen under composition, a stream at the display's own rate).
pub fn after_display_check(mode: PacingMode, verdict: &super::scanline::Verdict, fallback: PacingMode) -> PacingMode {
    match mode {
        PacingMode::Immediate | PacingMode::HostTimes if verdict.fixed_refresh() => fallback,
        mode => mode,
    }
}

/// What a VRR stream's pacing does with the display check's verdict and `NIGHTJAR_VRR_SYNC`, a
/// step on each present (`Streaming::sync_vrr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VrrPacing {
    /// The display check's verdict turns a fixed refresh display to `fallback`.
    pub check: bool,
    pub fallback: PacingMode,
    /// The check changed the pacing.
    pub fell_back: bool,
    pub sync: VrrSync,
    /// The sync interval the renderer was last given.
    pub interval: i32,
}

impl VrrPacing {
    /// `mode`: the pacing the stream starts with, whose sync interval the renderer has.
    pub fn new(check: bool, fallback: PacingMode, sync: VrrSync, mode: PacingMode) -> Self {
        Self { check, fallback, fell_back: false, sync, interval: i32::from(mode == PacingMode::Vsync) }
    }

    /// Folds the display check's verdict (`verdict`, None before the probe runs) into `mode`, and
    /// returns the sync interval to give the renderer when it is not the last one given. It is
    /// given once: a renderer that refuses it refuses it every frame.
    pub fn step(&mut self, mode: &mut PacingMode, verdict: Option<&super::scanline::Verdict>) -> Option<i32> {
        if let (true, Some(verdict)) = (self.check, verdict) {
            let after = after_display_check(*mode, verdict, self.fallback);
            if after != *mode {
                *mode = after;
                self.fell_back = true;
            }
        }
        let interval = sync_interval(*mode, self.sync);
        (interval != self.interval).then(|| {
            self.interval = interval;
            interval
        })
    }
}

/// How a VRR stream presents while the display may still be scanning the last picture
/// (`NIGHTJAR_VRR_SYNC`, a measurement switch). A tearing present then shows the new picture from
/// the beam down: 6-20% of the presents of a 60 fps stream on a 240 Hz VRR panel did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VrrSync {
    /// Present on arrival, with tearing.
    #[default]
    Off,
    /// Sync interval 1: the flip is immediate on an idle display and waits out a scan under way.
    Present1,
    /// Wait out a scan under way on this thread (the beam from the scan line, at most one scan),
    /// then present with tearing.
    Scan,
}

impl VrrSync {
    /// `NIGHTJAR_VRR_SYNC`'s value; anything else is said on stderr and leaves it off.
    pub fn parse(text: Option<&str>) -> Self {
        match text.map(str::trim) {
            None | Some("" | "off" | "0") => Self::Off,
            Some("present1") => Self::Present1,
            Some("scan") => Self::Scan,
            Some(other) => {
                eprintln!("nightjar: NIGHTJAR_VRR_SYNC={other:?} is not present1, scan or off, so it is off");
                Self::Off
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Present1 => "present1",
            Self::Scan => "scan",
        }
    }
}

/// The swap chain's sync interval for a pacing mode: 1 waits for the refresh, 0 presents at once
/// with tearing. Racing slices needs tearing whatever `sync` says.
pub fn sync_interval(mode: PacingMode, sync: VrrSync) -> i32 {
    match (mode, sync) {
        (PacingMode::Vsync, _) => 1,
        (PacingMode::Immediate | PacingMode::HostTimes, VrrSync::Present1) => 1,
        _ => 0,
    }
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
    /// The frame pacing setting, which applies while the mode is V-Sync.
    frame_pacing: FramePacing,
    /// One frame of the stream (`frame_us`).
    frame_us: u64,
    /// The FPS limit's last present slot.
    last_slot_us: Option<u64>,
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
            frame_pacing: FramePacing::Latency,
            frame_us: frame_us(60),
            last_slot_us: None,
        }
    }

    /// The frame pacing setting and the stream's frame rate, for V-Sync.
    pub fn with_frame_pacing(mut self, frame_pacing: FramePacing, fps: u32) -> Self {
        self.frame_pacing = frame_pacing;
        self.frame_us = frame_us(fps);
        self
    }

    /// Whether the frame pacing setting shapes this stream's presents now.
    fn paced(&self, pacing: FramePacing) -> bool {
        self.mode == PacingMode::Vsync && self.frame_pacing == pacing
    }

    /// The FPS limit's slot for a picture ready at `ready_us`: a frame after the last slot, but
    /// never before the picture is ready nor more than a frame after (a late picture starts the
    /// slots again), so it holds a picture at most one frame.
    fn cap_slot(&mut self, ready_us: u64) -> u64 {
        let slot = self.last_slot_us.map_or(ready_us, |last| (last + self.frame_us).clamp(ready_us, ready_us + self.frame_us));
        self.last_slot_us = Some(slot);
        slot
    }

    /// Takes newly decoded pictures, scheduling each once, in order.
    pub fn accept(&mut self, decoded: Decoded) {
        let (present_us, hold_us, late) = match self.mode {
            PacingMode::HostTimes => {
                let s = self.clock.schedule(decoded.rtp, decoded.ready_us);
                (s.present_us, s.hold_us, s.late)
            }
            _ if self.paced(FramePacing::CapFps) => {
                let slot = self.cap_slot(decoded.ready_us);
                (slot, (slot - decoded.ready_us) as i64, false)
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
    /// return's second value). With the FPS limit or the smoothest frame pacing it is the oldest
    /// due picture instead, one a refresh: an older one is dropped only once it is more than a
    /// frame old and a newer one is due.
    pub fn take_due(&mut self, now_us: u64) -> (Option<Pending>, u32) {
        let mut chosen = None;
        let mut dropped = 0;
        let mut pending = super::pipeline::lock(&self.pending);
        if self.paced(FramePacing::CapFps) || self.paced(FramePacing::Smoothness) {
            while pending.get(1).is_some_and(|next| next.present_us <= now_us)
                && pending.front().is_some_and(|p| now_us.saturating_sub(p.decoded.ready_us) > self.frame_us)
            {
                pending.pop_front();
                dropped += 1;
            }
            if pending.front().is_some_and(|p| p.present_us <= now_us) {
                chosen = pending.pop_front();
            }
            return (chosen, dropped);
        }
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
            logged_failure: None,
            hdr_frame: false,
            hlg_frame: false,
            pyrowave_stages: super::stage_times::Stages::new(super::pyrowave_gpu::UPLOAD_AND_PASSES),
            packed_format: None,
            white_level: 1.0,
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
/// An HDR picture with the HLG transfer gets the shader's HLG flag (128), never PQ.
pub(super) fn planar_parameters(info: &FrameInfo, layout: media::PlanarLayout, hdr: bool, white_level: f32) -> [u32; 8] {
    let matrix = match info.color_space { 5 | 6 => 0, 9 | 10 => 2, _ => 1 };
    let flags = u32::from(info.color_range == 2) | (u32::from(hdr) << 1) | (matrix << 2) | (u32::from(layout.ten_bit) << 4)
        | (u32::from(layout.chroma444) << 5) | (u32::from(layout.ten_bit) << 6) | (u32::from(hdr && info.color_trc == TRC_HLG) << 7);
    [layout.width, layout.height, layout.width, layout.height, 0, 3, flags, white_level.to_bits()]
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

    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
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
            texture: self.texture, d3d11: self.d3d11, picture: (self.width, self.height), ten_bit, colorspace: self.colorspace, transfer: self.transfer(),
        })
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
            let metadata = self.hdr_metadata.filter(|_| self.hdr_mode == Some(true));
            // SRGB_LINEAR is the float target of the PyroWave and packed paths, never a PQ picture.
            let hdr = hdr_properties(self.hdr_frame && space != SDL_COLORSPACE_SRGB_LINEAR, white_level, metadata);
            if let Some(white_point) = hdr.white_point {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_SDR_WHITE_POINT_FLOAT, white_point);
            }
            if let Some(headroom) = hdr.headroom {
                SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT, headroom);
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
                    hdr.headroom,
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
        self.hdr_frame = info.color_trc == TRC_PQ;
        self.hlg_frame = info.color_trc == TRC_HLG;
        if let Some(layout) = media::PlanarLayout::of(&info) {
            return self.upload_planar(renderer, &info, layout, white_level);
        }
        #[cfg(unix)]
        if hlg_through_planes(&info) {
            return self.upload_hlg(renderer, frame, &info, white_level);
        }
        let space = colorspace(&info);
        let (w, h) = (info.width, info.height);
        // SAFETY: plane pointers and pitches come from the live AVFrame.
        unsafe {
            match info.kind {
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
                    self.held = None;
                    self.copied = false;
                    // The pass reads a readable decoder texture in place: no copy.
                    if !self.direct_failed && super::direct::enabled()
                        && frame.d3d11_surface().is_some_and(|surface| surface.readable)
                    {
                        self.held = frame.reference();
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
                    let copied = frame.copy_to_d3d11(self.d3d11, w, h);
                    self.copied = copied;
                    if !copied {
                        self.fail(
                            format!("copying a {w}x{h} decoder picture into the {} video texture failed", pixel_format_name(format)),
                            "the GPU copy failed".into(),
                        );
                    }
                    copied
                }
                media::FRAME_NV12 | media::FRAME_P010 => self.upload_nv(renderer, &info, space, white_level),
                #[cfg(unix)]
                media::FRAME_DMABUF => self.upload_dmabuf(renderer, frame, &info, space, white_level),
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
    fn upload_dmabuf(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> bool {
        let Some(export) = frame.dmabuf() else { return false };
        if !self.dmabuf_declined {
            match self.import(renderer, export, info, space, white_level) {
                Ok(()) => {
                    // The texture samples the decoder's surface: it stays out of the decoder's
                    // hands until the next picture replaces it.
                    self.held = frame.reference();
                    return true;
                }
                Err(error) => {
                    eprintln!("nightjar: VAAPI pictures cannot be imported as DMA-BUFs ({error}); mapping them to memory instead");
                    self.dmabuf_declined = true;
                    self.destroy();
                }
            }
        }
        export.decline();
        let Some(mapped) = frame.map_to_memory() else {
            self.fail(format!("mapping a {}x{} VAAPI picture to memory failed", info.width, info.height), "the picture could not be read".into());
            return false;
        };
        self.upload_nv(renderer, &mapped.info(), space, white_level)
    }

    /// Draws an HLG NV12 or P010 picture with the plane shader (`hlg_through_planes`). A VAAPI
    /// surface is mapped to memory for it, this picture alone: the decoder keeps exporting, so a
    /// PQ or SDR picture after it is imported again.
    #[cfg(unix)]
    fn upload_hlg(&mut self, renderer: *mut SDL_Renderer, frame: &media::Frame, info: &FrameInfo, white_level: f32) -> bool {
        let mapped = if info.kind == media::FRAME_DMABUF {
            let Some(mapped) = frame.map_to_memory() else {
                self.fail(format!("mapping a {}x{} VAAPI picture to memory failed", info.width, info.height), "the picture could not be read".into());
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
    fn import(&mut self, renderer: *mut SDL_Renderer, export: &crate::dmabuf::Export, info: &FrameInfo, space: SDL_Colorspace, white_level: f32) -> Result<(), String> {
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
    fn transfer(&self) -> super::direct::Transfer {
        picture_transfer(self.format, self.hdr_mode, self.hdr_stream, self.hdr_frame, self.hlg_frame)
    }

    /// Puts a held decoder picture into the video texture, for what reads it (AI enhancement,
    /// `sdl_texture`). Nothing to do when the picture was copied.
    #[cfg(windows)]
    pub fn copy_held(&mut self) {
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

    /// The texture SDL draws or the upscalers read for the current picture: the video texture
    /// (with a held picture copied in), or for a 10-bit SDR or an HLG picture, which SDL's
    /// shaders would decode as PQ or gamma, its RGB conversion by the pass.
    #[cfg(windows)]
    pub fn sdl_texture(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_Texture {
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

    #[cfg(not(windows))]
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
        if !self.converted.is_null() && self.converted_linear != linear {
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
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(self.width));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, i64::from(self.height));
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
        let size = (self.width as u32, self.height as u32);
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
            texture, slice, size: (self.width as u32, self.height as u32), ten_bit, full_range, matrix, transfer: self.transfer(),
        })
    }

    /// Draws `texture` into the back buffer with `stream::direct` and returns true, or returns
    /// false for SDL to draw it. `texture` is the video texture or AI enhancement's output; the
    /// pass draws the D3D11VA picture of either, and nothing else. A failing pass is logged
    /// once and SDL draws from then on.
    #[cfg(windows)]
    pub fn draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        let drawn = self.try_draw_direct(renderer, destination, texture);
        if !drawn {
            // SDL may resize the back buffer before the pass draws again.
            if let Some((_, pass)) = &mut self.direct { pass.release_back_buffer(); }
        }
        drawn
    }

    #[cfg(windows)]
    fn try_draw_direct(&mut self, renderer: *mut SDL_Renderer, destination: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        if self.direct_failed || texture.is_null() || !super::direct::enabled() {
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
        let tone = super::direct::Tone {
            white_nits,
            dolby_vision: self.dolby_vision,
            hdr10_plus: self.hdr10_plus,
            static_peak: hdr_properties(true, self.white_level, metadata).headroom.map(|headroom| headroom * white_nits),
        };
        let drawn = direct_pass(&mut self.direct, renderer).and_then(|pass| {
            let held = &self.held;
            // SAFETY: The picture's texture and the renderer belong to the pass's device.
            unsafe { pass.present(renderer, destination, &picture, tone, |submit| lock_for(held, picture.texture, submit)) }
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

    /// Draws a software-decoded planar picture (4:4:4, or 10-bit) with the plane shader into an
    /// RGBA64F texture, as the GPU decoder's planes are drawn: SDL has no texture formats for them.
    fn upload_planar(&mut self, renderer: *mut SDL_Renderer, info: &FrameInfo, layout: media::PlanarLayout, white_level: f32) -> bool {
        let hdr = shows_hdr(self.hdr_mode, self.hdr_stream, info.color_trc);
        if hdr && !crate::sdl::hdr_available(renderer) {
            self.destroy();
            self.fail(format!("an HDR {}x{} picture needs HDR output, which the renderer {} does not offer", layout.width, layout.height, crate::sdl::renderer_name(renderer)), "the display has no HDR output".into());
            return false;
        }
        let parameters = planar_parameters(info, layout, hdr, white_level);
        if self.pyrowave_format.is_some_and(|previous| previous != parameters) { self.destroy(); }
        if !self.ensure(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, info.width, info.height, SDL_COLORSPACE_SRGB_LINEAR, white_level) { return false; }
        #[cfg(windows)]
        let drawn = {
            if self.planes.is_none() {
                if self.d3d11.is_null() {
                    self.fail(format!("the RGBA64F video texture has no Direct3D 11 texture (renderer {})", crate::sdl::renderer_name(renderer)), "it needs the Direct3D 11 renderer".into());
                    return false;
                }
                // SAFETY: `d3d11` is the live texture of the renderer's device.
                match unsafe { super::d3d11::PlanesPass::new(self.d3d11) } {
                    Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                    Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
                }
            }
            self.planes.as_mut().unwrap().draw_frame(info, &parameters)
        };
        #[cfg(unix)]
        let drawn = {
            if self.planes.is_none() {
                // SAFETY: The live renderer belongs to the presenting thread.
                match unsafe { super::planes_gpu::PlanesPass::for_frames(renderer, layout) } {
                    Ok(pass) => { self.planes = Some(pass); self.pyrowave_format = Some(parameters); }
                    Err(error) => { self.fail(format!("the plane shader pass cannot be made: {error}"), format!("the plane shader pass cannot be made: {error}")); return false; }
                }
            }
            let texture = self.texture;
            // SAFETY: The pass was made on this renderer for pictures shaped like `info` (a change
            // of shape changes the parameters, which destroys it), and `info` is the live frame's.
            unsafe { self.planes.as_mut().unwrap().draw_frame(renderer, texture, info, &parameters) }
        };
        #[cfg(not(any(windows, unix)))]
        let drawn: Result<(), String> = Err("this platform has no plane shader pass".into());
        match drawn {
            Ok(()) => true,
            Err(error) => { self.fail(format!("drawing a {}x{} planar picture failed: {error}", layout.width, layout.height), error); false }
        }
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
            // The pass outlives the texture; the next pictures are a new scene to it.
            #[cfg(windows)]
            direct: self.direct.take().map(|(device, mut pass)| { pass.reset(); (device, pass) }),
            #[cfg(windows)]
            direct_failed: self.direct_failed,
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

    fn frame(trc: i32, space: i32, range: i32) -> FrameInfo {
        FrameInfo {
            width: 1920,
            height: 1080,
            kind: media::FRAME_D3D11,
            ten_bit: 1,
            pts: 0,
            data: [std::ptr::null_mut(); 3],
            linesize: [0; 3],
            color_space: space,
            color_range: range,
            color_trc: trc,
        }
    }

    #[test]
    fn a_pq_picture_is_a_ycbcr_colorspace_so_a_p010_texture_can_be_made() {
        // The YUV conversion matrix of an RGB colorspace (SDL_COLORSPACE_HDR10) does not exist.
        for (range, expected) in [(1, SDL_COLORSPACE_BT2020_LIMITED), (2, SDL_COLORSPACE_BT2020_FULL), (0, SDL_COLORSPACE_BT2020_LIMITED)] {
            let space = colorspace(&frame(16, 9, range));
            assert_eq!(space.0, expected.0);
            assert_ne!(space.0, SDL_COLORSPACE_HDR10.0);
            assert_eq!((space.0 >> 28) & 0xf, SDL_COLOR_TYPE_YCBCR.0, "a YUV texture needs a YCbCr colorspace");
            assert_eq!((space.0 >> 5) & 0x1f, SDL_TRANSFER_CHARACTERISTICS_PQ.0, "PQ must stay the transfer");
        }
        // A PQ stream whose VUI says nothing about the matrix is still BT.2020.
        assert_eq!(colorspace(&frame(16, 2, 0)).0, SDL_COLORSPACE_BT2020_LIMITED.0);
        assert_eq!(colorspace(&frame(1, 1, 0)).0, SDL_COLORSPACE_BT709_LIMITED.0);
    }

    #[test]
    fn the_texture_gets_the_display_white_and_the_peak_as_headroom() {
        let metadata = nightjar_core::control::HdrMetadata { max_display_luminance: 1000, max_content_light_level: 0, ..Default::default() };
        // White level 2.5 is 200 nits; a 1000-nit peak is 5x that.
        assert_eq!(hdr_properties(true, 2.5, Some(metadata)), HdrProperties { white_point: Some(200.0), headroom: Some(5.0) });
        // SDR streams pass no white point.
        assert_eq!(hdr_properties(false, 1.0, None), HdrProperties { white_point: None, headroom: None });
        // A broken display value (zero, NaN, infinity) is never passed on.
        for white in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(hdr_properties(true, white, Some(metadata)), HdrProperties { white_point: None, headroom: None });
        }
        // No metadata, or an empty one, leaves the headroom to SDL.
        assert_eq!(hdr_properties(true, 1.0, None).headroom, None);
        let empty = nightjar_core::control::HdrMetadata::default();
        assert_eq!(hdr_properties(true, 1.0, Some(empty)).headroom, None);
    }

    #[test]
    fn destroying_the_texture_keeps_what_is_known_about_the_stream() {
        // `ensure` destroys first and reads the PQ flag after; a failing stream logs once, not per picture.
        let mut texture = VideoTexture { hdr_frame: true, ..Default::default() };
        texture.fail("detail".into(), "short".into());
        assert_eq!(texture.failure.as_deref(), Some("short"));
        texture.destroy();
        assert!(texture.hdr_frame, "the PQ flag must survive destroy");
        assert!(texture.is_hdr());
        assert_eq!(texture.logged_failure.as_deref(), Some("detail"));
        assert_eq!(texture.failure.as_deref(), Some("short"));
        assert!(hdr_properties(texture.hdr_frame, 2.5, None).white_point == Some(200.0));
        let mut sdr10 = VideoTexture::new(true, false);
        let mut hdr = VideoTexture::new(true, true);
        sdr10.destroy();
        hdr.destroy();
        assert!(!sdr10.hdr_stream && hdr.hdr_stream, "what the session negotiated must survive destroy");
    }

    #[test]
    fn a_10_bit_sdr_stream_keeps_the_sdr_white_until_the_host_says_hdr() {
        // The bug: every 10-bit picture without the host's HDR mode took the display's SDR white
        // (and HDR conversion), so 10-bit SDR came out too dark or too bright.
        use nightjar_core::sdp::{VIDEO_FORMAT_AV1_MAIN10, VIDEO_FORMAT_H265, VIDEO_FORMAT_H265_MAIN10};
        assert!(!hdr_negotiated(false, VIDEO_FORMAT_H265_MAIN10), "10-bit color (SDR) is not HDR");
        assert!(!hdr_negotiated(false, VIDEO_FORMAT_AV1_MAIN10));
        assert!(hdr_negotiated(true, VIDEO_FORMAT_H265_MAIN10));
        assert!(!hdr_negotiated(true, VIDEO_FORMAT_H265), "HDR asked for, 8-bit chosen");
        let bt709 = 1;
        assert!(!shows_hdr(None, hdr_negotiated(false, VIDEO_FORMAT_H265_MAIN10), bt709));
        assert!(shows_hdr(None, hdr_negotiated(true, VIDEO_FORMAT_H265_MAIN10), bt709));
        // The PQ transfer, and the host's HDR mode once it is known, still decide.
        assert!(shows_hdr(None, false, 16));
        assert!(!shows_hdr(Some(false), true, 16));
        assert!(shows_hdr(Some(true), false, bt709));
    }

    #[test]
    fn planar_pictures_get_the_plane_shader_flags_of_their_colour_description() {
        let info = |kind, width, height, space, range| FrameInfo { kind, width, height, ten_bit: 0, ..frame(1, space, range) };
        let layout = |info: &FrameInfo| media::PlanarLayout::of(info).unwrap();
        // 8-bit 4:4:4, BT.709 limited: matrix 1, 4:4:4 (32).
        let yuv444 = info(media::FRAME_YUV444P, 64, 32, 1, 1);
        assert_eq!(planar_parameters(&yuv444, layout(&yuv444), false, 1.0), [64, 32, 64, 32, 0, 3, 4 | 32, 1.0f32.to_bits()]);
        // 10-bit 4:2:0, BT.601 full: full (1), matrix 0, ten (16), FFmpeg's low-bit samples (64).
        let main10 = info(media::FRAME_YUV420P10, 7, 3, 6, 2);
        assert_eq!(planar_parameters(&main10, layout(&main10), false, 1.0)[6], 1 | 16 | 64);
        // 10-bit 4:4:4 HDR, BT.2020, white 2.5.
        let hdr = info(media::FRAME_YUV444P10, 16, 16, 9, 1);
        assert_eq!(planar_parameters(&hdr, layout(&hdr), true, 2.5)[6..], [2 | 8 | 16 | 32 | 64, 2.5f32.to_bits()]);
        // An HLG one gets the HLG flag (128) beside HDR, so the shader does not read it as PQ;
        // with the host's HDR mode off it is drawn as SDR, without it.
        let hlg = FrameInfo { color_trc: TRC_HLG, ..hdr };
        assert_eq!(planar_parameters(&hlg, layout(&hlg), true, 2.5)[6], 2 | 8 | 16 | 32 | 64 | 128);
        assert_eq!(planar_parameters(&hlg, layout(&hlg), false, 1.0)[6], 8 | 16 | 32 | 64);
        // Odd 4:2:0 pictures round their chroma up, as FFmpeg does.
        assert_eq!([0, 1, 2].map(|p| layout(&main10).plane_size(p)), [(7, 3), (4, 2), (4, 2)]);
        assert_eq!([0, 1, 2].map(|p| layout(&yuv444).plane_size(p)), [(64, 32); 3]);
        // Only the three planar kinds take this path.
        for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_YUV420P, media::FRAME_D3D11, media::FRAME_WAVELET] {
            assert!(media::PlanarLayout::of(&info(kind, 64, 32, 1, 1)).is_none());
        }
        assert!(media::PlanarLayout::of(&info(media::FRAME_YUV444P, 0, 32, 1, 1)).is_none());
    }

    /// Linux hardware decoders (VAAPI, NVDEC, V4L2) hand SDL NV12 and P010 pictures, and SDL read
    /// an HLG P010 picture as PQ. An HLG one goes to the plane shader with the HLG flag; SDR and
    /// PQ ones keep SDL's path (no copy, no added latency).
    #[cfg(unix)]
    #[test]
    fn an_hlg_hardware_picture_is_drawn_as_hlg_on_linux_never_as_pq() {
        let info = |kind, color_trc| FrameInfo { kind, width: 64, height: 32, ten_bit: i32::from(kind == media::FRAME_P010), color_trc, ..frame(1, 9, 1) };
        for kind in [media::FRAME_NV12, media::FRAME_P010, media::FRAME_DMABUF] {
            assert!(hlg_through_planes(&info(kind, TRC_HLG)), "kind {kind}");
            assert!(!hlg_through_planes(&info(kind, TRC_PQ)), "PQ kind {kind} stays SDL's");
            assert!(!hlg_through_planes(&info(kind, 1)), "SDR kind {kind} stays SDL's");
        }
        // Planar pictures already take the plane shader; the others have no NV12 planes.
        for kind in [media::FRAME_YUV420P, media::FRAME_YUV420P10, media::FRAME_WAVELET] {
            assert!(!hlg_through_planes(&info(kind, TRC_HLG)), "kind {kind}");
        }
        // A P010 picture reads as 10-bit 4:2:0 with its samples moved to the low bits (64), and
        // shown as HDR it gets the HLG flag (128), never PQ.
        let p010 = info(media::FRAME_P010, TRC_HLG);
        let layout = media::PlanarLayout::of_semi_planar(&p010).unwrap();
        assert_eq!(layout.plane_size(1), (32, 16));
        assert_eq!(planar_parameters(&p010, layout, true, 2.5)[6], 2 | 8 | 16 | 64 | 128);
        let nv12 = info(media::FRAME_NV12, TRC_HLG);
        assert_eq!(planar_parameters(&nv12, media::PlanarLayout::of_semi_planar(&nv12).unwrap(), true, 2.5)[6], 2 | 8 | 128);
        assert!(media::PlanarLayout::of_semi_planar(&info(media::FRAME_DMABUF, TRC_HLG)).is_none(), "a surface is mapped first");
    }

    /// The report: an HDR stream showed "Cannot show d3d11 pictures". Needs a Direct3D 11 display.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires a Direct3D 11 display"]
    fn sdl_makes_a_p010_texture_in_the_pq_colorspace_we_choose() {
        let _guard = crate::sdl::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: This test owns SDL and all its resources on one thread.
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
            let window = SDL_CreateWindow(c"HDR texture test".as_ptr(), 128, 64, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            let make = |space: SDL_Colorspace| {
                let metadata = nightjar_core::control::HdrMetadata { max_display_luminance: 1000, ..Default::default() };
                let mut texture = VideoTexture { hdr_frame: true, hdr_mode: Some(true), hdr_metadata: Some(metadata), ..Default::default() };
                // A white level of 2.5 is 200 nits, so a 1000-nit peak is 5x headroom.
                let made = texture.ensure(renderer, SDL_PIXELFORMAT_P010, 1920, 1080, space, 2.5);
                let failure = texture.failure.take();
                let properties = if made { SDL_GetTextureProperties(texture.texture) } else { SDL_PropertiesID(0) };
                let white = SDL_GetFloatProperty(properties, SDL_PROP_TEXTURE_SDR_WHITE_POINT_FLOAT, 0.0);
                let headroom = SDL_GetFloatProperty(properties, SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT, 0.0);
                texture.destroy();
                (made, failure, white, headroom)
            };
            let (made, why, white, headroom) = make(colorspace(&frame(16, 9, 1)));
            assert!(made, "{why:?}");
            assert_eq!((white, headroom), (200.0, 5.0), "the texture must keep the display's white and the peak");
            let (made, why, ..) = make(SDL_COLORSPACE_HDR10);
            assert!(!made, "SDL accepts an RGB colorspace for P010 now; the HDR10 workaround is unnecessary");
            assert!(why.is_some_and(|why| why.contains("YUV colorspace")));
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }

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
    fn only_a_display_that_scans_at_its_own_rate_turns_a_vrr_stream_to_v_sync() {
        use super::super::scanline::{Beam, RaceCapability, Reason, Verdict};
        let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
        let off = |reason| Verdict::Off(reason, capability);
        let fixed = off(Reason::NoVrr);
        let vsync = PacingMode::Vsync;
        assert_eq!(after_display_check(PacingMode::Immediate, &fixed, vsync), PacingMode::Vsync, "the laptop's 120 Hz panel");
        assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, vsync), PacingMode::Vsync);
        assert_eq!(after_display_check(PacingMode::Vsync, &fixed, vsync), PacingMode::Vsync);
        // V-Sync turned off in the settings: what the stream would have without VRR.
        let immediate = PacingMode::Immediate;
        assert_eq!(after_display_check(PacingMode::Immediate, &fixed, immediate), PacingMode::Immediate);
        assert_eq!(after_display_check(PacingMode::HostTimes, &fixed, immediate), PacingMode::Immediate);
        // Still checking, VRR seen, and every reason that says nothing about the refresh keep the mode.
        let beam = Beam { rows: 1440, blank_rows: 200, total_rows: 1640, scan_ns: 4_100_000, refresh_hz: 243.9, lines_per_ms: 400.0,
            vblank_fraction: 0.7, scans: 50, idle_from: 2040, longest_idle_ns: 0 };
        let mut kept = vec![Verdict::Checking, Verdict::On(beam, RaceCapability { vrr: true, flip_inferred: true, ..capability })];
        kept.extend([Reason::NoTearing, Reason::NoScanline, Reason::NoScan, Reason::NoRestart, Reason::Windowed, Reason::NoIdle, Reason::RowsDiffer].map(off));
        for verdict in &kept {
            for mode in [PacingMode::Immediate, PacingMode::HostTimes, PacingMode::BeamRace, PacingMode::Vsync] {
                assert_eq!(after_display_check(mode, verdict, PacingMode::Vsync), mode, "{verdict:?}");
            }
        }
    }

    #[test]
    fn the_vrr_sync_switch_sets_the_sync_interval_of_tearing_modes_only() {
        assert_eq!(VrrSync::parse(None), VrrSync::Off);
        assert_eq!(VrrSync::parse(Some("off")), VrrSync::Off);
        assert_eq!(VrrSync::parse(Some(" present1 ")), VrrSync::Present1);
        assert_eq!(VrrSync::parse(Some("scan")), VrrSync::Scan);
        assert_eq!(VrrSync::parse(Some("vsync")), VrrSync::Off, "an unknown value is said and left off");
        assert_eq!(VrrSync::default(), VrrSync::Off, "presents on arrival unless measured otherwise");
        for sync in [VrrSync::Off, VrrSync::Present1, VrrSync::Scan] {
            assert_eq!(sync_interval(PacingMode::Vsync, sync), 1);
            assert_eq!(sync_interval(PacingMode::BeamRace, sync), 0, "slices need tearing");
        }
        assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Off), 0);
        assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Present1), 1);
        assert_eq!(sync_interval(PacingMode::HostTimes, VrrSync::Present1), 1);
        assert_eq!(sync_interval(PacingMode::Immediate, VrrSync::Scan), 0, "waits on this thread, then tears");
    }

    #[test]
    fn a_vrr_streams_pacing_follows_the_display_check_once_and_sets_the_interval_once() {
        use super::super::scanline::{RaceCapability, Reason, Verdict};
        let capability = RaceCapability { tearing: true, vrr: false, flip_inferred: false, scanline: true };
        let fixed = Verdict::Off(Reason::NoVrr, capability);
        let settings = crate::config::Settings::default();
        let (start, _) = pacing(&settings, true);
        assert_eq!(start, PacingMode::Immediate);
        let mut vrr = VrrPacing::new(true, without_vrr(&settings), VrrSync::Off, start);
        let mut mode = start;
        assert_eq!(vrr.step(&mut mode, None), None, "no probe yet: the renderer keeps interval 0");
        assert_eq!(vrr.step(&mut mode, Some(&Verdict::Checking)), None);
        assert_eq!(vrr.step(&mut mode, Some(&fixed)), Some(1), "V-Sync waits for the refresh");
        assert_eq!((mode, vrr.fell_back), (PacingMode::Vsync, true));
        assert_eq!(vrr.step(&mut mode, Some(&fixed)), None, "given once");
        // V-Sync off in the settings: immediate stays, with nothing for the renderer.
        let no_vsync = crate::config::Settings { vsync: false, present_at_host_times: true, ..Default::default() };
        let (start, _) = pacing(&no_vsync, true);
        assert_eq!(start, PacingMode::HostTimes);
        let mut vrr = VrrPacing::new(true, without_vrr(&no_vsync), VrrSync::Off, start);
        let mut mode = start;
        assert_eq!(vrr.step(&mut mode, Some(&fixed)), None);
        assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, true));
        // NIGHTJAR_VRR_CHECK=0: the verdict is not applied; present1 still sets interval 1.
        let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Present1, PacingMode::Immediate);
        let mut mode = PacingMode::Immediate;
        assert_eq!(vrr.step(&mut mode, Some(&fixed)), Some(1));
        assert_eq!((mode, vrr.fell_back), (PacingMode::Immediate, false));
        // A stream that starts on V-Sync has interval 1 already.
        let mut vrr = VrrPacing::new(false, PacingMode::Vsync, VrrSync::Off, PacingMode::Vsync);
        let mut mode = PacingMode::Vsync;
        assert_eq!(vrr.step(&mut mode, None), None);
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

    #[cfg(windows)]
    #[test]
    fn an_hdr_stream_is_pq_without_a_pq_transfer_in_the_bitstream() {
        use super::super::direct::Transfer::{Pq, Sdr};
        // The host's HDR mode decides, not the VUI: a host that leaves the transfer out still streams PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(true), false, false, false), Pq);
        // Before the HDR mode arrives, the negotiated HDR or the PQ transfer decides.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, true, false, false), Pq);
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, false, true, false), Pq);
        // 10-bit SDR, and HDR mode off, are not PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, None, false, false, false), Sdr);
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(false), true, true, false), Sdr);
        // An 8-bit picture is never PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_NV12, Some(true), true, true, false), Sdr);
    }

    #[cfg(windows)]
    #[test]
    fn an_hlg_picture_is_hlg_on_every_path_never_pq() {
        use super::super::direct::Transfer::{Hlg, Pq, Sdr};
        // The HLG transfer in the VUI makes the picture HLG, with or without the host's HDR mode
        // or negotiated HDR, 10-bit or 8-bit.
        for (hdr_mode, hdr_stream) in [(Some(true), true), (None, true), (None, false), (Some(true), false)] {
            assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, hdr_mode, hdr_stream, false, true), Hlg, "{hdr_mode:?} {hdr_stream}");
            assert_eq!(picture_transfer(SDL_PIXELFORMAT_NV12, hdr_mode, hdr_stream, false, true), Hlg, "{hdr_mode:?} {hdr_stream}");
        }
        assert!(shows_hdr(None, false, TRC_HLG), "HLG is HDR");
        // The host's HDR mode off still means SDR, as it does for PQ.
        assert_eq!(picture_transfer(SDL_PIXELFORMAT_P010, Some(false), true, false, true), Sdr);
        // SDL's shaders know no HLG: whatever it is in, SDL gets the pass's conversion.
        assert!(sdl_needs_rgb(SDL_PIXELFORMAT_P010, Hlg) && sdl_needs_rgb(SDL_PIXELFORMAT_NV12, Hlg));
        assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_P010, Pq));
        // And the picture counts as HDR (upscaler gate, HDR output check).
        let texture = VideoTexture { hlg_frame: true, ..Default::default() };
        assert!(texture.is_hdr());
        assert_eq!(texture.transfer(), Sdr, "no picture format yet");
        let texture = VideoTexture { hlg_frame: true, format: SDL_PIXELFORMAT_P010, ..Default::default() };
        assert_eq!(texture.transfer(), Hlg);
    }

    #[cfg(windows)]
    #[test]
    fn sdl_gets_rgb_exactly_when_its_shaders_would_decode_the_picture_wrongly() {
        use super::super::direct::Transfer::{Pq, Sdr};
        // SDL decodes P010 as PQ and NV12 as gamma: every case of the one decision.
        for (format, hdr_mode, hdr_stream, hdr_frame) in [
            (SDL_PIXELFORMAT_P010, Some(true), false, false),
            (SDL_PIXELFORMAT_P010, None, false, false),
            (SDL_PIXELFORMAT_P010, Some(false), true, true),
            (SDL_PIXELFORMAT_NV12, Some(true), true, true),
            (SDL_PIXELFORMAT_NV12, None, false, false),
        ] {
            let transfer = picture_transfer(format, hdr_mode, hdr_stream, hdr_frame, false);
            let sdl_decodes_pq = format == SDL_PIXELFORMAT_P010;
            assert_eq!(sdl_needs_rgb(format, transfer), sdl_decodes_pq != (transfer == Pq), "{} {hdr_mode:?} {hdr_stream} {hdr_frame}", format.0);
        }
        // 10-bit SDR is the case: converted, not shown as PQ.
        assert!(sdl_needs_rgb(SDL_PIXELFORMAT_P010, Sdr));
        assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_P010, Pq));
        assert!(!sdl_needs_rgb(SDL_PIXELFORMAT_NV12, Sdr));
    }

    #[test]
    fn frame_pacing_other_than_lowest_latency_waits_for_the_refresh_without_vrr_only() {
        let mut settings = crate::config::Settings { vrr: false, vsync: false, ..Default::default() };
        assert_eq!(settings.frame_pacing, FramePacing::Latency, "the default keeps today's pacing");
        assert_eq!(pacing(&settings, true).0, PacingMode::Immediate, "lowest latency follows the V-Sync setting");
        for mode in [FramePacing::Balanced, FramePacing::CapFps, FramePacing::Smoothness] {
            settings.frame_pacing = mode;
            assert_eq!(pacing(&settings, true).0, PacingMode::Vsync, "{mode:?}");
        }
        let vrr = crate::config::Settings { frame_pacing: FramePacing::Smoothness, ..Default::default() };
        assert_eq!(pacing(&vrr, true), (PacingMode::Immediate, true), "VRR ignores frame pacing, as on Android");
        assert_eq!(frame_us(60), 16_666);
        assert_eq!(frame_us(0), 250_000, "an unknown rate holds at most 250 ms");
    }

    #[test]
    fn lowest_and_balanced_pacing_show_the_newest_due_picture() {
        for mode in [FramePacing::Latency, FramePacing::Balanced] {
            let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(mode, 60);
            for ready in [1_000, 9_000, 17_000] {
                pacer.accept(decoded(ready));
            }
            let (picture, dropped) = pacer.take_due(17_000);
            assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 2), "{mode:?}");
        }
    }

    #[test]
    fn smoothest_pacing_shows_each_picture_in_turn_and_skips_only_stale_ones() {
        let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
        for ready in [1_000, 9_000, 17_000] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(17_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (1_000, 0), "16 ms old: still within a frame");
        let (picture, dropped) = pacer.take_due(26_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (17_000, 1), "17 ms old with a newer one due: skipped");
        assert!(pacer.next_due().is_none());
        pacer.accept(decoded(30_000));
        let (picture, dropped) = pacer.take_due(60_000);
        assert_eq!((picture.unwrap().decoded.ready_us, dropped), (30_000, 0), "a lone late picture still shows");
        let mut vrr = Pacer::new(PacingMode::Immediate, 8_333).with_frame_pacing(FramePacing::Smoothness, 60);
        vrr.accept(decoded(1_000));
        vrr.accept(decoded(2_000));
        assert_eq!(vrr.take_due(2_000).1, 1, "without V-Sync the setting does not apply");
    }

    #[test]
    fn the_fps_limit_gives_each_picture_its_own_slot_holding_it_at_most_a_frame() {
        let mut pacer = Pacer::new(PacingMode::Vsync, 8_333).with_frame_pacing(FramePacing::CapFps, 60);
        // Two pictures that came 2 ms apart, then one late.
        for ready in [10_000, 12_000, 60_000] {
            pacer.accept(decoded(ready));
        }
        let (picture, dropped) = pacer.take_due(12_000);
        let picture = picture.unwrap();
        assert_eq!((picture.decoded.ready_us, picture.present_us, picture.hold_us, dropped), (10_000, 10_000, 0, 0));
        assert_eq!(pacer.next_due(), Some(26_666), "the early one waits for the next slot");
        assert!(pacer.take_due(20_000).0.is_none());
        let (picture, dropped) = pacer.take_due(26_666);
        let picture = picture.unwrap();
        assert_eq!((picture.decoded.ready_us, picture.hold_us, dropped), (12_000, 14_666, 0));
        assert_eq!(pacer.next_due(), Some(60_000), "a late picture starts the slots again");
        pacer.accept(decoded(60_500));
        pacer.accept(decoded(61_000));
        assert_eq!(pacer.take_due(100_000).1, 2, "pictures more than a frame old give way to the newest due one");
    }
}
