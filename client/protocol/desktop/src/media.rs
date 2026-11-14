//! FFmpeg and Opus through Rust bindings to the platform libraries.

use std::ffi::{c_char, c_int, c_uchar, c_void, CStr};
use std::ptr::{self, NonNull};

use nightjar_core::video::{FrameBytes, FRAME_PADDING};

#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::all
)]
pub(crate) mod ffi {
    include!(concat!(env!("OUT_DIR"), "/media.rs"));
}
use ffi::*;

#[cfg(target_os = "linux")]
use crate::stream::stage_times::Stages;

#[derive(Clone, Copy)]
pub struct FrameInfo {
    pub width: c_int,
    pub height: c_int,
    pub kind: c_int,
    pub ten_bit: c_int,
    pub pts: i64,
    pub data: [*mut u8; 3],
    pub linesize: [c_int; 3],
    pub color_space: c_int,
    pub color_range: c_int,
    pub color_trc: c_int,
}

pub const FRAME_NV12: c_int = 0;
pub const FRAME_P010: c_int = 1;
pub const FRAME_YUV420P: c_int = 2;
/// A VideoToolbox CVPixelBuffer whose NV12 or P010 planes SDL Metal can import.
pub const FRAME_VIDEOTOOLBOX: c_int = 13;
pub const FRAME_D3D11: c_int = 3;
pub const FRAME_WAVELET: c_int = 5;
/// Software-decoded planar pictures: 8-bit 4:4:4, and 10-bit 4:2:0 and 4:4:4 with the samples in
/// the low bits of each 16-bit word. The presenter draws them with the plane shader (`PlanarLayout`).
pub const FRAME_YUV444P: c_int = 6;
pub const FRAME_YUV420P10: c_int = 7;
pub const FRAME_YUV444P10: c_int = 8;
/// A VAAPI surface exported as DMA-BUFs for the presenter to import (`crate::dmabuf`), Linux only.
#[cfg(unix)]
pub const FRAME_DMABUF: c_int = 9;
/// A Vulkan video picture on the GPU renderer's own device (`crate::vulkan_video`), Linux only:
/// two planes, 4:2:0 or (`FRAME_VULKAN444`) 4:4:4, 8- or 10-bit (`ten_bit`). The presenter copies
/// its planes on the GPU and draws them with the plane shader.
#[cfg(unix)]
pub const FRAME_VULKAN: c_int = 10;
#[cfg(unix)]
pub const FRAME_VULKAN444: c_int = 11;
/// A 4:4:4 VAAPI picture mapped into the GPU renderer's device as one image holding Y, Cb and Cr
/// (`vulkan_video::interleaved_444`), Linux only. The presenter copies it whole and the plane
/// shader reads all three from it.
#[cfg(unix)]
pub const FRAME_VULKAN_INTERLEAVED444: c_int = 12;
/// A D3D11VA 4:4:4 picture: an AYUV (8-bit), Y410 or Y416 (10-bit, `ten_bit`) surface, which NVIDIA
/// and Intel decoders make for H.264 High 4:4:4 and HEVC RExt 4:4:4. The presenter copies it and
/// draws it with the plane shader's surface entry (`stream::d3d11::PlanesPass::draw_surface`).
pub const FRAME_D3D11_444: c_int = 13;
/// 10-bit 4:4:4 in three 16-bit planes with the samples in the high bits of each word, as P010's:
/// NVDEC's 10-bit 4:4:4 (`YUV444P16`) copied to memory. The plane upload moves them to the low
/// bits (`stream::planes_gpu`), so it draws as `FRAME_YUV444P10`. Linux only.
pub const FRAME_YUV444P16: c_int = 14;

/// Whether `info` is a `FRAME_VULKAN_INTERLEAVED444` picture.
pub fn interleaved_444(info: &FrameInfo) -> bool {
    #[cfg(unix)]
    return info.kind == FRAME_VULKAN_INTERLEAVED444;
    #[cfg(not(unix))]
    {
        let _ = info;
        false
    }
}

/// The shape of a planar picture's textures: R8 planes, or R16 for 10-bit, with half-size chroma
/// (rounded up, as FFmpeg's) unless the picture is 4:4:4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarLayout {
    pub width: u32,
    pub height: u32,
    pub chroma444: bool,
    pub ten_bit: bool,
}

impl PlanarLayout {
    /// The layout of a `FRAME_YUV444P`, `FRAME_YUV420P10` or `FRAME_YUV444P10` picture, and on
    /// Linux of a `FRAME_YUV444P16` one (its high-bit samples moved to the low bits on upload).
    pub fn of(info: &FrameInfo) -> Option<Self> {
        let (chroma444, ten_bit) = match info.kind {
            FRAME_YUV444P => (true, false),
            FRAME_YUV420P10 => (false, true),
            FRAME_YUV444P10 => (true, true),
            #[cfg(unix)]
            FRAME_YUV444P16 => (true, true),
            _ => return None,
        };
        (info.width > 0 && info.height > 0).then_some(Self { width: info.width as u32, height: info.height as u32, chroma444, ten_bit })
    }

    /// The layout of a `FRAME_D3D11_444` picture as the plane shader's surface entry reads its
    /// copy: one texel holding Y, Cb and Cr, 8-bit (AYUV) or 10-bit (Y410, Y416).
    pub fn of_d3d11(info: &FrameInfo) -> Option<Self> {
        (info.kind == FRAME_D3D11_444 && info.width > 0 && info.height > 0)
            .then_some(Self { width: info.width as u32, height: info.height as u32, chroma444: true, ten_bit: info.ten_bit != 0 })
    }

    /// The 4:2:0 layout an NV12 or P010 picture in memory is drawn in by the plane shader: the
    /// upload splits its interleaved chroma into two planes and moves P010's high-bit samples to
    /// the low bits (`planes_gpu`), so it reads as `FRAME_YUV420P` or `FRAME_YUV420P10`.
    #[cfg(unix)]
    pub fn of_semi_planar(info: &FrameInfo) -> Option<Self> {
        let ten_bit = match info.kind {
            FRAME_NV12 => false,
            FRAME_P010 => true,
            _ => return None,
        };
        (info.width > 0 && info.height > 0).then_some(Self { width: info.width as u32, height: info.height as u32, chroma444: false, ten_bit })
    }

    /// The layout of a `FRAME_VULKAN`, `FRAME_VULKAN444` or `FRAME_VULKAN_INTERLEAVED444`
    /// picture's planes as the plane shader reads them once copied (`vulkan_video::Planes`), its
    /// chroma interleaved.
    #[cfg(unix)]
    pub fn of_vulkan(info: &FrameInfo) -> Option<Self> {
        let chroma444 = match info.kind {
            FRAME_VULKAN => false,
            FRAME_VULKAN444 | FRAME_VULKAN_INTERLEAVED444 => true,
            _ => return None,
        };
        (info.width > 0 && info.height > 0).then_some(Self { width: info.width as u32, height: info.height as u32, chroma444, ten_bit: info.ten_bit != 0 })
    }

    #[cfg(not(unix))]
    pub fn of_vulkan(_: &FrameInfo) -> Option<Self> {
        None
    }

    pub fn plane_size(&self, plane: usize) -> (u32, u32) {
        if plane == 0 || self.chroma444 { (self.width, self.height) } else { (self.width.div_ceil(2), self.height.div_ceil(2)) }
    }
}

extern "C" {
    fn opus_multistream_decoder_create(
        rate: c_int,
        channels: c_int,
        streams: c_int,
        coupled: c_int,
        mapping: *const c_uchar,
        error: *mut c_int,
    ) -> *mut c_void;
    fn opus_multistream_decode_float(
        decoder: *mut c_void,
        data: *const c_uchar,
        length: c_int,
        pcm: *mut f32,
        frame_size: c_int,
        fec: c_int,
    ) -> c_int;
    fn opus_multistream_decoder_destroy(decoder: *mut c_void);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

/// What a stream's pictures are, for a backend that checks it can decode and hand them on before
/// it opens (Vulkan video): their depth and chroma, and whether they carry packed 4:4:4 chroma.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pictures {
    pub ten_bit: bool,
    pub chroma444: bool,
    pub packed: bool,
    /// How many pictures the stream may keep waiting to be shown (`pipeline::pool_waiting`): a
    /// hardware pool gets these and three more beyond FFmpeg's minimum ([`pool_extra`]).
    pub waiting: usize,
    /// Where VAAPI opens. Linux only.
    pub vaapi_node: VaapiNode,
}

/// The device VAAPI opens on (`Pictures::vaapi_node`, [`vaapi_node`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum VaapiNode {
    /// FFmpeg's default device, the first render node.
    #[default]
    Default,
    /// The GPU renderer's DRM render node (`/dev/dri/renderD<minor>`) when its device can take
    /// VAAPI's pictures mapped (`crate::vulkan_video::VaapiMap`): VAAPI decodes on its GPU, and
    /// its pool gets the surfaces the presenter keeps for its copies.
    Mapped(u32),
    /// The render node of the GPU the OpenGL renderer's EGL display runs on, when that is not the
    /// default device (`crate::dmabuf::display_vaapi_node`): its EGL imports the pictures VAAPI
    /// decodes there, and refuses another GPU's tiled ones.
    Display(u32),
}

impl VaapiNode {
    /// The render node's minor; none for the default device.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // VAAPI is Unix's; Windows reads it in tests only.
    pub fn minor(self) -> Option<u32> {
        match self {
            Self::Default => None,
            Self::Mapped(minor) | Self::Display(minor) => Some(minor),
        }
    }
}

/// A reference held on the renderer's Direct3D 11 device ([`Hardware::D3d11`]'s, and PyroWave's
/// GPU decoder's), so the device outlives the renderer's use of it while a launch, a decoder or a
/// benchmark still uses it: SDL makes its Direct3D 11 device again after a driver reset, and a
/// decoder opened on the old pointer would use freed memory. Elsewhere it holds nothing.
#[derive(Clone, Default)]
pub struct DeviceRef(#[cfg(windows)] Option<windows::Win32::Graphics::Direct3D11::ID3D11Device>);

impl DeviceRef {
    /// Takes a reference on `hardware`'s device, read from the live renderer on the main thread.
    pub fn of(hardware: Hardware) -> Self {
        match hardware {
            Hardware::D3d11(raw) => Self::of_device(raw),
            _ => Self::default(),
        }
    }

    /// Takes a reference on the Direct3D 11 device at `raw`, just read from the live renderer on
    /// the main thread; none for null.
    pub fn of_device(raw: *mut c_void) -> Self {
        #[cfg(windows)]
        {
            use windows::core::Interface;
            // SAFETY: `raw` is null or the live renderer's device, just read from it.
            Self(unsafe { windows::Win32::Graphics::Direct3D11::ID3D11Device::from_raw_borrowed(&raw) }.cloned())
        }
        #[cfg(not(windows))]
        {
            let _ = raw;
            Self::default()
        }
    }

    /// Whether this holds the device at `raw` (null: whether it holds none). A held device is never
    /// freed, so a device made later cannot take its address. Always true off Windows.
    pub fn holds(&self, raw: *mut c_void) -> bool {
        #[cfg(windows)]
        {
            use windows::core::Interface;
            self.0.as_ref().map_or(std::ptr::null_mut(), Interface::as_raw) == raw
        }
        #[cfg(not(windows))]
        {
            let _ = raw;
            true
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    None,
    D3d11(*mut c_void),
    /// The whole `order` where no single backend is given: on Linux with the GPU renderer's SDL GPU
    /// device for Vulkan video (null when the renderer is another kind) and the render node VAAPI
    /// opens on for the OpenGL renderer (`crate::dmabuf::renderer_vaapi_node`, none for the default
    /// device), and on Windows without a Direct3D 11 renderer.
    Auto(*mut c_void, Option<u32>),
    Vaapi,
    #[cfg(target_os = "macos")]
    VideoToolbox,
    /// FFmpeg's Vulkan video on the GPU renderer's own Vulkan device, this SDL GPU device (or
    /// null, which does not open), its pictures drawn without a copy to memory. Linux only.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Vulkan(*mut c_void),
    /// NVDEC, with CUDA frames copied into the GPU renderer's Vulkan images (`map_to_vulkan`), else
    /// to memory for the presenter.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Cuda,
    /// The V4L2 memory-to-memory decoders of ARM handhelds and boards (H.264 and HEVC).
    V4l2m2m,
}

// The renderer keeps the device alive until the decoder has stopped; FFmpeg owns a COM reference.
unsafe impl Send for Hardware {}

/// The Debug settings "Vulkan video first" (`Settings::vulkan_video_first`) and "Vulkan video
/// recovery" (`Settings::vulkan_rfi`) of the stream being set up, both off by default; their
/// environment variables win over them for test runs.
static VULKAN_VIDEO_FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static VULKAN_RFI: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Takes the Vulkan video Debug settings of the stream about to start, before any decoder order
/// or capability is worked out for it.
pub fn use_vulkan_video_settings(first: bool, rfi: bool) {
    VULKAN_VIDEO_FIRST.store(first, std::sync::atomic::Ordering::Relaxed);
    VULKAN_RFI.store(rfi, std::sync::atomic::Ordering::Relaxed);
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn vulkan_video_first_setting() -> bool {
    VULKAN_VIDEO_FIRST.load(std::sync::atomic::Ordering::Relaxed)
}

/// The "Vulkan video recovery" setting ([`use_vulkan_video_settings`]).
pub fn vulkan_rfi_setting() -> bool {
    VULKAN_RFI.load(std::sync::atomic::Ordering::Relaxed)
}

impl Hardware {
    /// The backends to try, in turn, for this choice. On Linux that is moonlight-qt's and Moonlight
    /// V+'s order: the first-tier hwaccels (VAAPI, then Vulkan video for GPUs without VAAPI), then
    /// the hardware decoders outside FFmpeg's hwaccels (V4L2 M2M), then the second tier (NVDEC);
    /// software decoding comes after them all. VAAPI stays first with the GPU renderer's device
    /// too: its pictures are mapped into the renderer's Vulkan images without a copy, and it keeps
    /// reference frame invalidation. The Debug setting "Vulkan video first"
    /// ([`use_vulkan_video_settings`]) or `NIGHTJAR_VULKAN_VIDEO_GPU_FIRST=1` puts Vulkan video
    /// before VAAPI there, VAAPI next where it does not open or FFmpeg refuses it for the stream;
    /// `NIGHTJAR_VULKAN_VIDEO` puts Vulkan first whatever the renderer or leaves it out. A VA
    /// driver that wraps another API (`crate::linux_video::vaapi_plan`: nvidia-vaapi-driver, the
    /// VDPAU backend) leaves VAAPI out, so NVIDIA decodes with Vulkan video, then NVDEC. The order,
    /// the devices and why go to the log once.
    pub fn order(self) -> Vec<Self> {
        match self {
            Self::None => Vec::new(),
            #[cfg(target_os = "linux")]
            Self::Auto(gpu, _) => {
                use crate::linux_video;
                let vaapi = vaapi_device(vaapi_node(self).minor());
                let plan = linux_video::vaapi_plan(
                    vaapi.as_ref().map(|identity| identity.vendor.as_str()).map_err(String::as_str),
                    crate::stream::env_flag(linux_video::FORCE_VAAPI_ENVIRONMENT) == Some(true),
                );
                if !gpu.is_null() {
                    // SAFETY: The renderer keeps its device alive while streams and probes use it.
                    match unsafe { linux_video::vulkan_identity(gpu.cast()) } {
                        Ok(identity) => linux_video::note(&identity.line()),
                        Err(why) => linux_video::note(&format!("vulkan: the renderer's device is not described: {why}")),
                    }
                }
                let (order, why) = Self::linux_order(
                    gpu,
                    crate::vulkan_video::Choice::from_environment(),
                    crate::vulkan_video::gpu_first(
                        crate::stream::env_flag(crate::vulkan_video::GPU_FIRST_ENVIRONMENT),
                        vulkan_video_first_setting(),
                    ),
                    &plan,
                );
                let labels: Vec<_> = order.iter().map(|backend| backend.label()).collect();
                linux_video::note(&linux_video::order_line(&labels, &why));
                order
            }
            // Vulkan video is Linux's; the rest of the order is tried as before it.
            #[cfg(target_os = "macos")]
            Self::Auto(..) => vec![Self::VideoToolbox],
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            Self::Auto(..) => vec![Self::Vaapi, Self::V4l2m2m, Self::Cuda],
            other => vec![other],
        }
    }

    /// The Linux order for the GPU renderer's device `gpu` (null for another renderer), what
    /// `NIGHTJAR_VULKAN_VIDEO` asks for, "Vulkan video first" and the VA driver's plan, and why.
    #[cfg(any(target_os = "linux", test))]
    fn linux_order(
        gpu: *mut c_void,
        choice: crate::vulkan_video::Choice,
        gpu_first: bool,
        vaapi: &crate::linux_video::VaapiPlan,
    ) -> (Vec<Self>, String) {
        use crate::vulkan_video::Choice;
        let (mut order, why) = match choice {
            Choice::Default if gpu_first && !gpu.is_null() => {
                (vec![Self::Vulkan(gpu), Self::Vaapi, Self::V4l2m2m, Self::Cuda], "Vulkan video first, the Debug setting or NIGHTJAR_VULKAN_VIDEO_GPU_FIRST=1")
            }
            Choice::Default if gpu.is_null() => {
                (vec![Self::Vaapi, Self::Vulkan(gpu), Self::V4l2m2m, Self::Cuda], "VA-API first; Vulkan video needs the GPU renderer")
            }
            Choice::Default => (vec![Self::Vaapi, Self::Vulkan(gpu), Self::V4l2m2m, Self::Cuda], "VA-API first, its pictures mapped into the renderer's Vulkan images"),
            Choice::First => (vec![Self::Vulkan(gpu), Self::Vaapi, Self::V4l2m2m, Self::Cuda], "NIGHTJAR_VULKAN_VIDEO=1"),
            Choice::Off => (vec![Self::Vaapi, Self::V4l2m2m, Self::Cuda], "NIGHTJAR_VULKAN_VIDEO=0"),
        };
        if vaapi.skip {
            order.retain(|&backend| backend != Self::Vaapi);
        }
        let why = match (&vaapi.why, vaapi.skip) {
            (Some(note), true) => note.clone(),
            (Some(note), false) => format!("{why}; {note}"),
            (None, _) => why.to_owned(),
        };
        (order, why)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "software",
            Self::D3d11(_) => "D3D11VA",
            Self::Auto(..) => "hardware",
            Self::Vaapi => "VAAPI",
            #[cfg(target_os = "macos")]
            Self::VideoToolbox => "VideoToolbox",
            Self::Vulkan(_) => "Vulkan",
            Self::Cuda => "NVDEC",
            Self::V4l2m2m => "V4L2 M2M",
        }
    }
}

/// The VA driver on render node `node` (FFmpeg's default device for none), or why VAAPI does not
/// open there: AMD and Intel GPUs open, NVIDIA's driver does not without a VA driver of its own
/// (Vulkan video is used then, `crate::vulkan_video`). Asked once per node; the `vaapi:` line
/// goes to the log then.
#[cfg(target_os = "linux")]
pub fn vaapi_device(node: Option<u32>) -> Result<crate::linux_video::VaapiIdentity, String> {
    type Known = Vec<(Option<u32>, Result<crate::linux_video::VaapiIdentity, String>)>;
    static KNOWN: std::sync::Mutex<Known> = std::sync::Mutex::new(Vec::new());
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, identity)) = known.iter().find(|(asked, _)| *asked == node) {
        return identity.clone();
    }
    let identity = crate::linux_video::vaapi_identity(node);
    match &identity {
        Ok(identity) => crate::linux_video::note(&identity.line()),
        Err(error) => {
            let node = node.map_or_else(|| "the default device".to_owned(), |minor| format!("renderD{minor}"));
            crate::linux_video::note(&format!("vaapi: does not open on {node} ({error})"));
        }
    }
    known.push((node, identity.clone()));
    identity
}

/// Opens the first backend of `order` that opens; `failed` hears about each one that did not, and
/// the error names them all.
fn first_opened<T>(
    order: Vec<Hardware>,
    mut open: impl FnMut(Hardware) -> Result<T, String>,
    mut failed: impl FnMut(Hardware, &str),
) -> Result<T, String> {
    let mut errors = Vec::new();
    for backend in order {
        match open(backend) {
            Ok(opened) => return Ok(opened),
            Err(error) => {
                failed(backend, &error);
                errors.push(format!("{}: {error}", backend.label()));
            }
        }
    }
    if errors.is_empty() {
        return Err("no hardware decoder".into());
    }
    Err(errors.join("; "))
}

struct Format {
    hardware: AVPixelFormat,
    required: bool,
    /// `select_format` found no way to decode the stream with `hardware` and gave FFmpeg a
    /// software format instead (`VideoDecoder::refused`).
    software: std::cell::Cell<bool>,
    /// D3D11VA: make the decoder's textures readable by shaders (`stream::direct`).
    #[cfg(windows)]
    bind_textures: bool,
    /// Vulkan video: the pictures the device was checked for, whose two-plane format the frames
    /// context is made in (`vulkan_video::frames_context`).
    #[cfg(unix)]
    vulkan: Option<Pictures>,
}

/// Surfaces a hardware decoder's pool has at least beyond FFmpeg's minimum, for pictures waiting
/// to be shown, the one on the screen, the one being drawn and one kept after a refused present.
pub const EXTRA_HW_FRAMES: i32 = 6;

/// Pictures holding pool surfaces outside the presenter's queue and the pacer: the shown one, the
/// drawn one and the one kept for the next present when a non-blocking present was refused
/// (`skipped_picture`).
pub const HELD_BEYOND_WAITING: usize = 3;

/// The pool's surfaces beyond FFmpeg's minimum for `waiting` pictures waiting to be shown, plus
/// the [`HELD_BEYOND_WAITING`] ones.
pub fn pool_extra(waiting: usize) -> i32 {
    (waiting.min(64) as i32 + HELD_BEYOND_WAITING as i32).max(EXTRA_HW_FRAMES)
}

/// The device VAAPI opens on for `hardware` (`Pictures::vaapi_node`): the GPU renderer's render
/// node when its device will take VAAPI's pictures mapped (`crate::vulkan_video::map_node`), the
/// OpenGL renderer's when it is not the default device (`Hardware::Auto`'s node), else the default
/// device. The probes and the stream ask the same, so they decode on one GPU; why not the
/// renderer's goes to the log.
pub fn vaapi_node(hardware: Hardware) -> VaapiNode {
    #[cfg(target_os = "linux")]
    if let Hardware::Auto(gpu, display) = hardware {
        if !gpu.is_null() {
            let switched_on = crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref());
            // SAFETY: The renderer keeps its device alive while streams and probes use it.
            match unsafe { crate::vulkan_video::map_node(gpu.cast(), switched_on) } {
                Ok(minor) => return VaapiNode::Mapped(minor),
                Err(why) => crate::vulkan_video::note(&format!("VAAPI opens on the default device: {why}")),
            }
        }
        return display.map_or(VaapiNode::Default, VaapiNode::Display);
    }
    let _ = hardware;
    VaapiNode::Default
}

/// The surfaces a VAAPI pool gets beyond [`pool_extra`] when its pictures will be mapped for the
/// GPU renderer (`VaapiNode::Mapped`): those the presenter keeps until their copies are done.
fn mapped_extra(hardware: Hardware, pictures: Pictures) -> i32 {
    #[cfg(unix)]
    if hardware == Hardware::Vaapi && matches!(pictures.vaapi_node, VaapiNode::Mapped(_)) {
        return crate::vulkan_video::mapped_held();
    }
    let _ = (hardware, pictures);
    0
}

/// The device path of DRM render node `minor`, for VAAPI.
#[cfg(any(unix, test))]
fn vaapi_node_path(minor: u32) -> std::ffi::CString {
    std::ffi::CString::new(format!("/dev/dri/renderD{minor}")).expect("no NUL in a path of digits")
}

unsafe extern "C" fn select_format(
    context: *mut AVCodecContext,
    formats: *const AVPixelFormat,
) -> AVPixelFormat {
    let wanted = &*((*context).opaque.cast::<Format>());
    let mut next = formats;
    while *next != AV_PIX_FMT_NONE {
        if *next == wanted.hardware {
            #[cfg(windows)]
            if wanted.bind_textures && *next == AV_PIX_FMT_D3D11 {
                if let Err(reason) = bind_frames(context, *next) {
                    eprintln!("nightjar: the decoder's textures cannot be made readable ({reason}); its pictures are copied instead");
                }
            }
            #[cfg(unix)]
            if let Some(pictures) = wanted.vulkan {
                if let Err(reason) = crate::vulkan_video::frames_context(context, pictures) {
                    // The decoder then says it was refused, and the stream opens the next backend.
                    eprintln!("nightjar: Vulkan video cannot decode into the presenter's format: {reason}");
                    break;
                }
            }
            return *next;
        }
        next = next.add(1);
    }
    if !wanted.required {
        next = formats;
        while *next != AV_PIX_FMT_NONE {
            let descriptor = av_pix_fmt_desc_get(*next);
            if !descriptor.is_null()
                && (*descriptor).flags & u64::from(AV_PIX_FMT_FLAG_HWACCEL) == 0
            {
                // Decoded in software, or (AV1, which has no software path in this decoder) not
                // at all: never silently under the hardware decoder's name.
                wanted.software.set(true);
                return *next;
            }
            next = next.add(1);
        }
    }
    AV_PIX_FMT_NONE
}

/// Makes the decoder's frames context with shader-readable textures, as Moonlight V+ PC does
/// (d3d11va.cpp, prepareDecoderContextInGetFormat), so `stream::direct` reads its slices in place.
/// On failure the context is left to FFmpeg's default.
#[cfg(windows)]
unsafe fn bind_frames(context: *mut AVCodecContext, format: AVPixelFormat) -> Result<(), String> {
    unsafe extern "C" {
        fn av_hwframe_ctx_init(reference: *mut AVBufferRef) -> c_int;
    }
    let context = &mut *context;
    av_buffer_unref(&mut context.hw_frames_ctx);
    let status = avcodec_get_hw_frames_parameters(context, context.hw_device_ctx, format, &mut context.hw_frames_ctx);
    if status < 0 {
        return Err(format!("frames parameters: {}", error(status)));
    }
    let frames = &mut *(*context.hw_frames_ctx).data.cast::<AVHWFramesContext>();
    let d3d = &mut *frames.hwctx.cast::<AVD3D11VAFramesContext>();
    d3d.BindFlags |= windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE.0 as u32;
    // ff_decode_get_hw_frames_ctx adds these 3 to the minimum FFmpeg asks for.
    if frames.initial_pool_size != 0 {
        frames.initial_pool_size += 3;
    }
    let status = av_hwframe_ctx_init(context.hw_frames_ctx);
    if status < 0 {
        av_buffer_unref(&mut context.hw_frames_ctx);
        return Err(format!("frames context: {}", error(status)));
    }
    Ok(())
}

pub(crate) fn error(code: i32) -> String {
    let mut buffer = [0 as c_char; 128];
    unsafe {
        av_strerror(code, buffer.as_mut_ptr(), buffer.len());
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

pub struct VideoDecoder {
    context: *mut AVCodecContext,
    packet: *mut AVPacket,
    /// Packet buffers of `pool_size` bytes, reused frame to frame: a buffer allocated per frame
    /// costs page faults once frames are large (from about 1 MiB, high bitrates and key frames,
    /// Windows' heap maps a fresh block for each), measured at 0.14 ms per MiB on Windows 11.
    pool: *mut AVBufferPool,
    pool_size: usize,
    /// The bytes of the frame being sent (`lend`): a packet whose data lies in them goes to FFmpeg
    /// by reference instead of through a copy into `pool`.
    lent: Option<FrameBytes>,
    /// Packets that went to FFmpeg by reference rather than copied, for the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    lent_packets: u64,
    _format: Box<Format>,
    pub name: String,
    /// The backend this decoder opened with.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub hardware: Hardware,
    /// HEVC access units go to FFmpeg without the last shard's zero padding (`access_unit`).
    trims_padding: bool,
    /// VAAPI pictures go to the presenter as DMA-BUFs until the flag says the presenter cannot
    /// import them (`export_dmabuf`).
    #[cfg(unix)]
    dmabuf: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Whether VAAPI opened on the renderer's render node (`Pictures::vaapi_node`), or why not.
    #[cfg(unix)]
    vaapi_on_renderer: Result<(), String>,
    /// VAAPI pictures go to the GPU renderer mapped into Vulkan images (`map_to_vulkan`).
    #[cfg(unix)]
    vulkan_map: Option<crate::vulkan_video::VaapiMap>,
    /// NVDEC pictures go to the GPU renderer copied by CUDA into Vulkan images (`map_to_vulkan`).
    #[cfg(unix)]
    cuda_map: Option<crate::vulkan_video::CudaMap>,
    /// Why NVDEC pictures go through memory to the GPU renderer after all (the CUDA copy could not
    /// be set up, or a copy failed), until the stream takes it for its toast (`take_cuda_fallback`).
    #[cfg(unix)]
    cuda_fallback: Option<String>,
    /// The empty frame of the last `receive` that brought no picture, for the next one: each
    /// decoded frame ends with such a call, so this saves an allocation per frame. Pictures
    /// themselves go to the presenter and are freed there.
    spare: Option<Frame>,
    /// A hardware decoder made a picture that is not its hardware format: FFmpeg could not decode
    /// the stream with this backend after all and went to software. `told` once `renamed` has
    /// said so.
    refused: bool,
    told: bool,
    /// A hardware decoder's time per picture in `send`, in FFmpeg's `receive` and in handing the
    /// picture on (DMA-BUF export, Vulkan map, copy to memory), summed up in the log when it
    /// closes after a stream's worth of pictures. Linux only.
    #[cfg(target_os = "linux")]
    stages: Option<Box<Stages<3>>>,
}

/// The stages of `VideoDecoder::stages`.
#[cfg(target_os = "linux")]
const DECODE_STAGES: [&str; 3] = ["send", "receive", "hand-off"];

/// The size of the pooled packet buffers for a packet of `needed` bytes with its padding: a
/// quarter more, in whole 64 KiB, so a stream whose frames grow slowly does not remake the pool
/// for each one.
fn packet_buffer_size(needed: usize) -> usize {
    (needed + needed / 4).next_multiple_of(64 * 1024)
}

unsafe extern "C" {
    /// libavutil/buffer.h (FFmpeg 5 and newer take the size as `size_t`); not in the bindings.
    fn av_buffer_create(
        data: *mut u8,
        size: usize,
        free: Option<unsafe extern "C" fn(*mut c_void, *mut u8)>,
        opaque: *mut c_void,
        flags: c_int,
    ) -> *mut AVBufferRef;
}

/// `AV_BUFFER_FLAG_READONLY`: FFmpeg copies a packet of such a buffer before it would write to it.
const BUFFER_READ_ONLY: c_int = 1;

// A frame's own padding covers what FFmpeg may read past a packet.
const _: () = assert!(FRAME_PADDING >= AV_INPUT_BUFFER_PADDING_SIZE as usize);

/// The free callback of a buffer `lend_packet` made: lets go of its reference to the frame's
/// bytes, on whichever thread FFmpeg releases the last packet holding it (possibly after the
/// decoder and the stream are gone; the reference owns the bytes).
unsafe extern "C" fn release_lent(opaque: *mut c_void, _data: *mut u8) {
    // SAFETY: the reference `lend_packet` turned into this opaque pointer, released once.
    drop(unsafe { FrameBytes::from_raw(opaque.cast_const()) });
}

/// An FFmpeg buffer over a frame's `bytes` that holds a reference to them, for a packet of `data`:
/// when `data` is a non-empty part of those bytes followed there by at least `padding` zero bytes
/// (the frame's padding, after any zeros `access_unit` trimmed). None otherwise, and the packet
/// is copied instead.
fn lend_packet(bytes: &FrameBytes, data: &[u8], padding: usize) -> Option<*mut AVBufferRef> {
    let padded = bytes.padded();
    if data.is_empty() {
        return None;
    }
    let start = (data.as_ptr() as usize).checked_sub(padded.as_ptr() as usize)?;
    let end = start.checked_add(data.len())?;
    let zeros = padded.get(end..end.checked_add(padding)?)?;
    if zeros.iter().any(|&byte| byte != 0) {
        return None;
    }
    let raw = bytes.clone().into_raw();
    // SAFETY: the buffer spans the frame's bytes and padding, which the reference given as its
    // opaque pointer keeps alive and unwritten until `release_lent` lets it go. FFmpeg calls that
    // only for a buffer it made; on failure the reference is taken back here.
    let buffer = unsafe {
        av_buffer_create(padded.as_ptr().cast_mut(), padded.len(), Some(release_lent), raw.cast_mut(), BUFFER_READ_ONLY)
    };
    if buffer.is_null() {
        // SAFETY: from into_raw above; FFmpeg did not take it.
        drop(unsafe { FrameBytes::from_raw(raw) });
        return None;
    }
    Some(buffer)
}

/// An Annex B access unit without the zero bytes after its last NAL unit (trailing_zero_8bits,
/// here the last shard's padding, which no NAL unit ends with). FFmpeg's HEVC decoder reads a
/// Dolby Vision RPU, the access unit's last NAL unit, up to the packet's end, and fails on every
/// frame whose RPU the padding follows (FFmpeg 8.1: "Error parsing DOVI NAL unit"). All zeros stay
/// as they are: an empty packet would start draining the decoder.
fn access_unit(data: &[u8]) -> &[u8] {
    let end = data.iter().rposition(|&byte| byte != 0).map_or(data.len(), |last| last + 1);
    &data[..end]
}

/// Whether a picture of `format` from a decoder opened for the hardware format `hardware` (none
/// for software and V4L2 M2M decoders) says FFmpeg went to software.
fn refused_by(hardware: AVPixelFormat, format: AVPixelFormat) -> bool {
    hardware != AV_PIX_FMT_NONE && format != hardware
}

/// A refused decoder's name: its codec, with the backend that refused it.
fn refused_name(name: &str, hardware: Hardware) -> String {
    let codec = name.rsplit_once(' ').map_or(name, |(codec, _)| codec);
    format!("{codec} (software, {} refused)", hardware.label())
}

// An AVCodecContext moves between threads but is only accessed by its decoder thread.
unsafe impl Send for VideoDecoder {}

pub enum Received<F = Frame> {
    Frame(F),
    None,
    Error,
}

/// The packet timebase a decoder is opened with (`pkt_timebase`, num and den): microseconds for
/// V4L2 M2M, whose buffers carry a pts as a timeval rescaled from `pkt_timebase` (else the
/// decoder's unset `time_base`, 0/1, which turns every pts into 0) to microseconds and back
/// (FFmpeg's v4l2_buffers.c). In microseconds both rescales are exact, so a picture keeps its
/// frame's number. The other decoders hand the pts through untouched.
pub fn packet_timebase(hardware: Hardware) -> Option<(i32, i32)> {
    (hardware == Hardware::V4l2m2m).then_some((1, 1_000_000))
}

/// What FFmpeg's V4L2 M2M decoder makes of a packet's `pts` by the time it is a picture's, for a
/// decoder opened with `timebase` (`packet_timebase`; none leaves `time_base` at 0/1):
/// `v4l2_set_pts` rescales it to microseconds, `v4l2_get_pts` back (`av_rescale_q`, which
/// answers INT64_MIN for a zero denominator).
#[cfg(test)]
pub fn v4l2_pts_round_trip(pts: i64, timebase: Option<(i32, i32)>) -> i64 {
    let (num, den) = timebase.map_or((0, 1), |(num, den)| (i128::from(num), i128::from(den)));
    let rescale = |value: i128, from: (i128, i128), to: (i128, i128)| {
        let (b, c) = (from.0 * to.1, from.1 * to.0);
        if c == 0 { i64::MIN as i128 } else { (value * b + c / 2).div_euclid(c) }
    };
    let microseconds = rescale(i128::from(pts), (num, den), (1, 1_000_000));
    rescale(microseconds, (1, 1_000_000), (num, den)) as i64
}

/// `avcodec_send_packet` and `avcodec_receive_frame`'s AVERROR(EAGAIN): output must be read
/// before more input goes in, or no output is there yet.
#[cfg(unix)]
pub const AGAIN: i32 = -libc::EAGAIN;
#[cfg(windows)]
pub const AGAIN: i32 = -11;

/// Let FFmpeg select the device when this backend does not share one with the renderer.
fn open_default_device(context: &mut AVCodecContext, kind: AVHWDeviceType) -> Result<(), String> {
    // FFmpeg owns the returned reference through the codec context.
    let status = unsafe {
        av_hwdevice_ctx_create(&mut context.hw_device_ctx, kind, ptr::null(), ptr::null_mut(), 0)
    };
    if status < 0 { Err(format!("no device: {}", error(status))) } else { Ok(()) }
}

impl VideoDecoder {
    /// Opens the first backend of `hardware.order()` that opens, logging each that does not
    /// (`failed`); the error names every one.
    pub fn open_first(
        codec: Codec,
        pictures: Pictures,
        hardware: Hardware,
        require_hardware: bool,
        failed: impl FnMut(Hardware, &str),
    ) -> Result<Self, String> {
        Self::open_any(codec, pictures, hardware.order(), require_hardware, failed)
    }

    /// `open_first` for the backends of `order`.
    pub fn open_any(
        codec: Codec,
        pictures: Pictures,
        order: Vec<Hardware>,
        require_hardware: bool,
        failed: impl FnMut(Hardware, &str),
    ) -> Result<Self, String> {
        first_opened(order, |backend| Self::open_for(codec, pictures, backend, 1, require_hardware), failed)
    }

    pub fn open(
        codec: Codec,
        hardware: Hardware,
        threads: i32,
        require_hardware: bool,
    ) -> Result<Self, String> {
        Self::open_for(codec, Pictures::default(), hardware, threads, require_hardware)
    }

    /// `open`, for a stream of `pictures` (which Vulkan video checks it decodes before opening).
    pub fn open_for(
        codec: Codec,
        pictures: Pictures,
        hardware: Hardware,
        threads: i32,
        require_hardware: bool,
    ) -> Result<Self, String> {
        unsafe {
            let id = match codec {
                Codec::H264 => AV_CODEC_ID_H264,
                Codec::Hevc => AV_CODEC_ID_HEVC,
                Codec::Av1 => AV_CODEC_ID_AV1,
            };
            let mut found = if hardware == Hardware::V4l2m2m {
                // The V4L2 M2M decoders are decoders of their own, not hwaccels of FFmpeg's.
                let name = match codec {
                    Codec::H264 => c"h264_v4l2m2m",
                    Codec::Hevc => c"hevc_v4l2m2m",
                    Codec::Av1 => c"av1_v4l2m2m",
                };
                let found = avcodec_find_decoder_by_name(name.as_ptr());
                if found.is_null() {
                    return Err(format!("this FFmpeg has no {} decoder", name.to_string_lossy()));
                }
                found
            } else if codec == Codec::Av1 && hardware != Hardware::None {
                avcodec_find_decoder_by_name(c"av1".as_ptr())
            } else {
                ptr::null()
            };
            if found.is_null() {
                found = avcodec_find_decoder(id);
            }
            if found.is_null() {
                return Err("No decoder for this codec.".into());
            }
            let mut video = Self {
                context: avcodec_alloc_context3(found),
                packet: av_packet_alloc(),
                pool: ptr::null_mut(),
                pool_size: 0,
                lent: None,
                lent_packets: 0,
                _format: Box::new(Format {
                    hardware: AV_PIX_FMT_NONE,
                    required: require_hardware,
                    software: Default::default(),
                    #[cfg(windows)]
                    bind_textures: false,
                    #[cfg(unix)]
                    vulkan: matches!(hardware, Hardware::Vulkan(_)).then_some(pictures),
                }),
                name: CStr::from_ptr((*found).name).to_string_lossy().into_owned(),
                hardware,
                trims_padding: codec == Codec::Hevc,
                #[cfg(unix)]
                dmabuf: None,
                #[cfg(unix)]
                vaapi_on_renderer: Err("VAAPI was not asked to open on the renderer's GPU".into()),
                #[cfg(unix)]
                vulkan_map: None,
                #[cfg(unix)]
                cuda_map: None,
                #[cfg(unix)]
                cuda_fallback: None,
                spare: None,
                refused: false,
                told: false,
                #[cfg(target_os = "linux")]
                stages: (hardware != Hardware::None).then(|| Box::new(Stages::new(DECODE_STAGES))),
            };
            if video.context.is_null() || video.packet.is_null() {
                return Err("Could not allocate the video decoder.".into());
            }
            let context = &mut *video.context;
            context.opaque = (&mut *video._format as *mut Format).cast();
            let device_type = match hardware {
                Hardware::D3d11(_) => Some(AV_HWDEVICE_TYPE_D3D11VA),
                Hardware::Vaapi => Some(AV_HWDEVICE_TYPE_VAAPI),
                #[cfg(target_os = "macos")]
                Hardware::VideoToolbox => Some(AV_HWDEVICE_TYPE_VIDEOTOOLBOX),
                Hardware::Vulkan(_) => Some(AV_HWDEVICE_TYPE_VULKAN),
                Hardware::Cuda => Some(AV_HWDEVICE_TYPE_CUDA),
                Hardware::Auto(..) => return Err("the hardware order is not a backend of its own".into()),
                Hardware::None | Hardware::V4l2m2m => None,
            };
            if let Some(kind) = device_type {
                let mut index = 0;
                loop {
                    let config = avcodec_get_hw_config(found, index);
                    if config.is_null() {
                        return Err(format!("{} has no {} support in this FFmpeg", video.name, hardware.label()));
                    }
                    if (*config).device_type == kind
                        && (*config).methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
                    {
                        video._format.hardware = (*config).pix_fmt;
                        break;
                    }
                    index += 1;
                }
                match hardware {
                    Hardware::D3d11(device) => {
                        #[cfg(windows)]
                        {
                            use windows::core::Interface;
                            use windows::Win32::Graphics::{
                                Direct3D10::ID3D10Multithread, Direct3D11::ID3D11Device,
                            };
                            let device = ID3D11Device::from_raw_borrowed(&device)
                                .ok_or("No D3D11 device.")?;
                            let reference = av_hwdevice_ctx_alloc(kind);
                            if reference.is_null() {
                                return Err("Could not allocate a D3D11 device context.".into());
                            }
                            context.hw_device_ctx = reference;
                            let device_context = (*reference).data.cast::<AVHWDeviceContext>();
                            let d3d =
                                &mut *((*device_context).hwctx.cast::<AVD3D11VADeviceContext>());
                            d3d.device = device.clone().into_raw().cast();
                            // SDL and FFmpeg share the immediate context from different threads.
                            let _ = device
                                .cast::<ID3D10Multithread>()
                                .map_err(|e| format!("D3D11 multithread protection: {e}"))?
                                .SetMultithreadProtected(true);
                            video._format.bind_textures = crate::stream::direct::bind_decoder_textures(device);
                            let status = av_hwdevice_ctx_init(reference);
                            if status < 0 {
                                return Err(error(status));
                            }
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = device;
                            return Err("D3D11VA is only available on Windows.".into());
                        }
                    }
                    #[cfg(unix)]
                    Hardware::Vaapi if pictures.vaapi_node.minor().is_some() => {
                        let node = vaapi_node_path(pictures.vaapi_node.minor().unwrap_or_default());
                        let status = av_hwdevice_ctx_create(&mut context.hw_device_ctx, kind, node.as_ptr(), ptr::null_mut(), 0);
                        if status < 0 {
                            let why = format!("VAAPI does not open on {} ({})", node.to_string_lossy(), error(status));
                            let renderer = if matches!(pictures.vaapi_node, VaapiNode::Mapped(_)) { "GPU" } else { "OpenGL" };
                            eprintln!("nightjar: {why}; it opens on the default device, and its pictures may go through memory to the {renderer} renderer");
                            video.vaapi_on_renderer = Err(why);
                            open_default_device(context, kind)?;
                        } else {
                            video.vaapi_on_renderer = Ok(());
                        }
                    }
                    Hardware::Vaapi | Hardware::Cuda => open_default_device(context, kind)?,
                    #[cfg(target_os = "macos")]
                    Hardware::VideoToolbox => open_default_device(context, kind)?,
                    Hardware::Vulkan(gpu) => {
                        #[cfg(unix)]
                        {
                            context.hw_device_ctx = crate::vulkan_video::device_context(gpu.cast(), codec, pictures)?;
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = (gpu, pictures);
                            return Err("Vulkan video decoding is only used on Linux.".into());
                        }
                    }
                    Hardware::None | Hardware::V4l2m2m | Hardware::Auto(..) => unreachable!(),
                }
                context.get_format = Some(select_format);
                context.extra_hw_frames = pool_extra(pictures.waiting) + mapped_extra(hardware, pictures);
            }
            context.thread_count = if hardware == Hardware::None {
                threads
            } else {
                1
            };
            context.thread_type = FF_THREAD_SLICE as i32;
            if let Some((num, den)) = packet_timebase(hardware) {
                context.pkt_timebase = AVRational { num, den };
            }
            context.flags |= AV_CODEC_FLAG_LOW_DELAY as i32;
            context.flags2 |= AV_CODEC_FLAG2_FAST as i32;
            let mut options = ptr::null_mut();
            if video.name == "libdav1d" {
                av_dict_set(&mut options, c"max_frame_delay".as_ptr(), c"1".as_ptr(), 0);
            }
            let status = avcodec_open2(video.context, found, &mut options);
            av_dict_free(&mut options);
            if status < 0 {
                return Err(format!("Opening the decoder: {}", error(status)));
            }
            video.name.push_str(match hardware {
                Hardware::D3d11(_) => " d3d11va",
                Hardware::Vaapi => " vaapi",
                #[cfg(target_os = "macos")]
                Hardware::VideoToolbox => " videotoolbox",
                Hardware::Vulkan(_) => " vulkan",
                Hardware::Cuda => " nvdec",
                Hardware::None | Hardware::V4l2m2m | Hardware::Auto(..) => "",
            });
            Ok(video)
        }
    }

    /// Whether FFmpeg took this hardware decoder to software: it found no hardware format for the
    /// stream (`select_format`; AV1, which has no software path here, then fails every frame), or
    /// a picture came out that is not the backend's.
    pub fn refused(&self) -> bool {
        self.refused || self._format.software.get()
    }

    /// The decoder's new name, once, when it is no longer the backend it opened with (a hardware
    /// decoder FFmpeg took to software), for the overlay and the log.
    pub fn renamed(&mut self) -> Option<&str> {
        if !self.refused() || std::mem::replace(&mut self.told, true) {
            return None;
        }
        self.name = refused_name(&self.name, self.hardware);
        eprintln!("nightjar: FFmpeg cannot decode this stream with {} after all: {}", self.hardware.label(), self.name);
        Some(&self.name)
    }

    /// Has the decoder act as one FFmpeg took to software, for the tests of what follows.
    #[cfg(test)]
    pub fn refuse(&mut self) {
        self.refused = true;
    }

    /// Has the decoder act as one whose CUDA copy just failed, for the tests of what follows.
    #[cfg(all(test, unix))]
    pub fn fail_cuda_copy(&mut self, why: &str) {
        let mut map = Some(());
        copy_or_drop_map(&mut map, &mut self.cuda_fallback, |_| Err(why.to_owned()));
    }

    /// Whether pictures come out of it on their own time, not the frame they were sent with: the
    /// V4L2 M2M decoders answer "no picture yet" while the driver still takes input.
    pub fn asynchronous(&self) -> bool {
        self.hardware == Hardware::V4l2m2m
    }

    /// Whether its pictures hold a surface of the decoder's fixed pool until they are dropped
    /// (D3D11VA, VAAPI, Vulkan video): only [`EXTRA_HW_FRAMES`] are there beyond what the decoder
    /// itself needs. NVDEC's are copied out, and a refused decoder's are in memory.
    pub fn pooled(&self) -> bool {
        match self.hardware {
            Hardware::D3d11(_) | Hardware::Vaapi | Hardware::Vulkan(_) => !self.refused(),
            _ => false,
        }
    }

    /// Whether VAAPI or NVDEC pictures still go to the GPU renderer as Vulkan images
    /// (`map_to_vulkan`): a failed import or copy in `receive` drops the map for the rest of the
    /// stream.
    #[cfg(unix)]
    pub fn vulkan_mapped(&self) -> bool {
        self.vulkan_map.is_some() || self.cuda_map.is_some()
    }

    #[cfg(not(unix))]
    pub fn vulkan_mapped(&self) -> bool {
        false
    }

    /// Why NVDEC pictures went through memory to the GPU renderer instead of CUDA's copy into its
    /// Vulkan images, once: the stream says so in a toast.
    #[cfg(unix)]
    pub fn take_cuda_fallback(&mut self) -> Option<String> {
        self.cuda_fallback.take()
    }

    #[cfg(not(unix))]
    pub fn take_cuda_fallback(&mut self) -> Option<String> {
        None
    }

    /// Hands VAAPI pictures to the GPU renderer, the SDL GPU device `gpu`, mapped into Vulkan
    /// images on its device instead of through memory (nightjar#80: 2.5 GB/s read back and
    /// uploaded at 4K100 10-bit), unless `NIGHTJAR_VAAPI_VULKAN_MAP` is 0. Says which in the log,
    /// with the reason when they keep going through memory, and returns whether they are mapped
    /// (`vulkan_mapped` after a failed import says they no longer are).
    /// An NVDEC decoder's pictures are copied by CUDA into Vulkan images of that device instead
    /// (`vulkan_video::CudaMap`), where its device was made with the interop extensions.
    /// Only a VAAPI or NVDEC decoder on Linux (and macOS's Unix tests, which build the Vulkan rig).
    #[cfg(all(unix, any(target_os = "linux", test)))]
    pub fn map_to_vulkan(&mut self, gpu: *mut c_void) -> bool {
        if self.hardware == Hardware::Cuda {
            // SAFETY: As below; the codec context's device is the CUDA one this decoder opened.
            return match unsafe { crate::vulkan_video::CudaMap::new(gpu.cast(), (*self.context).hw_device_ctx) } {
                Ok(map) => {
                    eprintln!("nightjar: NVDEC pictures go to the GPU renderer as Vulkan images, copied by CUDA on the GPU");
                    self.cuda_map = Some(map);
                    true
                }
                Err(why) => {
                    eprintln!("nightjar: NVDEC pictures go through memory to the GPU renderer: {why}");
                    self.cuda_fallback = Some(why);
                    false
                }
            };
        }
        if self.hardware != Hardware::Vaapi {
            return false;
        }
        let switched_on = crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref());
        // SAFETY: The renderer keeps its device alive until the decoder has stopped.
        match unsafe { crate::vulkan_video::VaapiMap::new(gpu.cast(), switched_on, self.vaapi_on_renderer.clone()) } {
            Ok(map) => {
                eprintln!("nightjar: VAAPI pictures go to the GPU renderer as mapped Vulkan images");
                self.vulkan_map = Some(map);
                true
            }
            Err(why) => {
                eprintln!("nightjar: VAAPI pictures go through memory to the GPU renderer: {why}");
                false
            }
        }
    }

    /// How this decoder's pictures reach the presenter, for the `video path:` line.
    #[cfg(target_os = "linux")]
    pub fn route(&self) -> &'static str {
        match self.hardware {
            Hardware::Vaapi if self.vulkan_map.is_some() => "mapped into the renderer's Vulkan images",
            Hardware::Vaapi if self.dmabuf.is_some() => "imported as DMA-BUFs",
            Hardware::Vulkan(_) => "Vulkan images on the renderer's device",
            Hardware::D3d11(_) => "D3D11 textures on the renderer's device",
            Hardware::Cuda if self.cuda_map.is_some() => "copied by CUDA into the renderer's Vulkan images",
            Hardware::Cuda => "copied out of CUDA memory",
            Hardware::None => "decoded in memory",
            _ => "through memory",
        }
    }

    /// Hands VAAPI pictures to the presenter as DMA-BUFs instead of CPU mappings, and says whether
    /// it does: only a VAAPI decoder on Linux.
    pub fn export_dmabuf(&mut self) -> bool {
        #[cfg(unix)]
        if self.hardware == Hardware::Vaapi {
            self.dmabuf = Some(Default::default());
            return true;
        }
        false
    }

    pub fn send(&mut self, data: &[u8], pts: i64) -> i32 {
        #[cfg(target_os = "linux")]
        let started = std::time::Instant::now();
        let status = self.send_packet(data, pts);
        #[cfg(target_os = "linux")]
        if let Some(stages) = self.stages.as_mut() {
            stages.add(0, started);
        }
        status
    }

    /// Lends the decoder the bytes of the frame about to be sent, or takes them back (`None`, after
    /// the frame): a `send` whose data lies in them hands FFmpeg a reference to them instead of a
    /// copy. FFmpeg may keep that reference past the call; the bytes stay valid until it lets go.
    pub fn lend(&mut self, bytes: Option<FrameBytes>) {
        self.lent = bytes;
    }

    /// Packets that went to FFmpeg by reference (`lend`) rather than copied.
    #[cfg(test)]
    pub fn lent_packets(&self) -> u64 {
        self.lent_packets
    }

    fn send_packet(&mut self, data: &[u8], pts: i64) -> i32 {
        let data = if self.trims_padding { access_unit(data) } else { data };
        let Ok(length) = i32::try_from(data.len()) else {
            return -1;
        };
        let padding = AV_INPUT_BUFFER_PADDING_SIZE as usize;
        if let Some(buffer) = self.lent.as_ref().and_then(|bytes| lend_packet(bytes, data, padding)) {
            unsafe {
                av_packet_unref(self.packet);
                let packet = &mut *self.packet;
                packet.buf = buffer;
                packet.data = data.as_ptr().cast_mut();
                packet.size = length;
                packet.pts = pts;
                let status = avcodec_send_packet(self.context, self.packet);
                // FFmpeg took its own reference if it kept the packet: ours goes now, so the
                // frame's bytes return to the pool when FFmpeg is done with them.
                av_packet_unref(self.packet);
                self.lent_packets += 1;
                return status;
            }
        }
        unsafe {
            av_packet_unref(self.packet);
            let needed = data.len() + padding;
            if needed > self.pool_size {
                // Buffers the decoder still holds go back to the old pool, which frees itself
                // once they are all back.
                av_buffer_pool_uninit(&mut self.pool);
                self.pool_size = 0;
                let size = packet_buffer_size(needed);
                self.pool = av_buffer_pool_init(size, None);
                if self.pool.is_null() {
                    return -12; // AVERROR(ENOMEM)
                }
                self.pool_size = size;
            }
            let buffer = av_buffer_pool_get(self.pool);
            if buffer.is_null() {
                return -12;
            }
            let packet = &mut *self.packet;
            packet.buf = buffer;
            packet.data = (*buffer).data;
            packet.size = length;
            ptr::copy_nonoverlapping(data.as_ptr(), packet.data, data.len());
            // The buffer can hold an earlier, longer frame, and the decoder reads into the padding.
            ptr::write_bytes(packet.data.add(data.len()), 0, padding);
            packet.pts = pts;
            avcodec_send_packet(self.context, self.packet)
        }
    }

    /// Says the input has ended (an empty packet), so the decoder hands out what it still holds:
    /// a V4L2 M2M decoder keeps a lone frame until more input or this comes.
    pub fn drain(&mut self) -> i32 {
        unsafe { avcodec_send_packet(self.context, ptr::null()) }
    }

    pub fn receive(&mut self) -> Received {
        #[cfg(target_os = "linux")]
        let started = std::time::Instant::now();
        let mut decoded = None;
        let received = self.receive_picture(&mut decoded);
        #[cfg(target_os = "linux")]
        if let Some(stages) = self.stages.as_mut() {
            time_receive(stages, &received, started, decoded);
        }
        received
    }

    /// `receive`, setting `decoded` once FFmpeg has handed over a picture.
    fn receive_picture(&mut self, decoded: &mut Option<std::time::Instant>) -> Received {
        unsafe {
            #[allow(unused_mut)]
            let mut frame = match self.spare.take() {
                Some(frame) => frame,
                None => {
                    let Some(raw) = NonNull::new(av_frame_alloc()) else {
                        return Received::Error;
                    };
                    Frame::new(raw)
                }
            };
            let raw = frame.raw;
            let status = avcodec_receive_frame(self.context, raw.as_ptr());
            if status < 0 {
                // avcodec_receive_frame unreferences the frame first, so it is empty again.
                self.spare = Some(frame);
                return if status == AGAIN || status == -541478725 {
                    Received::None
                } else {
                    Received::Error
                };
            }
            *decoded = Some(std::time::Instant::now());
            let format = (*raw.as_ptr()).format;
            self.refused |= refused_by(self._format.hardware, format);
            if format == AV_PIX_FMT_VAAPI {
                #[cfg(unix)]
                if let Some(decline) = self.dmabuf.as_ref().filter(|d| !d.load(std::sync::atomic::Ordering::Relaxed)) {
                    match crate::dmabuf::export(raw.as_ptr(), decline) {
                        Ok(export) => {
                            frame.dmabuf = Some(Box::new(export));
                            return Received::Frame(frame);
                        }
                        Err(error) => {
                            eprintln!("nightjar: VAAPI pictures cannot be exported as DMA-BUFs ({error}); mapping them to memory instead");
                            decline.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                #[cfg(unix)]
                if let Some(map) = self.vulkan_map.as_mut() {
                    let Some(target) = NonNull::new(av_frame_alloc()) else {
                        return Received::Error;
                    };
                    let mapped = Frame::new(target);
                    match map.map(raw.as_ptr(), target.as_ptr()) {
                        Ok(()) => {
                            // The mapped picture holds the surface; the decoded frame is empty
                            // again for the next `receive`.
                            av_frame_unref(raw.as_ptr());
                            self.spare = Some(frame);
                            return Received::Frame(mapped);
                        }
                        Err(why) => {
                            eprintln!("nightjar: VAAPI pictures go through memory to the GPU renderer from now on: {why}");
                            self.vulkan_map = None;
                        }
                    }
                }
                return frame.map_to_memory().map_or(Received::Error, Received::Frame);
            }
            if format == AV_PIX_FMT_CUDA {
                #[cfg(unix)]
                if self.cuda_map.is_some() {
                    let Some(target) = NonNull::new(av_frame_alloc()) else {
                        return Received::Error;
                    };
                    let copied = Frame::new(target);
                    let source = raw.as_ptr();
                    if copy_or_drop_map(&mut self.cuda_map, &mut self.cuda_fallback, |map| map.copy(source, target.as_ptr())) {
                        // The copy is queued on the decoder's CUDA stream, ahead of any later
                        // use of the CUDA picture; the decoded frame is empty again.
                        av_frame_unref(raw.as_ptr());
                        self.spare = Some(frame);
                        return Received::Frame(copied);
                    }
                }
                // NVDEC's pictures are in CUDA memory, which only CUDA itself can read: copied out.
                let Some(memory) = NonNull::new(av_frame_alloc()) else {
                    return Received::Error;
                };
                let memory = Frame::new(memory);
                if av_hwframe_transfer_data(memory.raw.as_ptr(), raw.as_ptr(), 0) < 0
                    || av_frame_copy_props(memory.raw.as_ptr(), raw.as_ptr()) < 0
                {
                    return Received::Error;
                }
                return Received::Frame(memory);
            }
            Received::Frame(frame)
        }
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(summary) = self.stages.as_ref().and_then(|stages| stage_summary(stages)) {
            eprintln!("nightjar: {} decode stages {summary} (CPU time)", self.name);
        }
        unsafe {
            avcodec_free_context(&mut self.context);
            av_packet_free(&mut self.packet);
            av_buffer_pool_uninit(&mut self.pool);
        }
    }
}

/// A picture's copy with `map` (CUDA's into Vulkan images): true when it was copied. A failed copy
/// drops the map for the rest of the stream, logs why and keeps it in `fallback` for the stream's
/// toast, and the picture goes through memory; without a map nothing is copied.
#[cfg(any(unix, test))]
fn copy_or_drop_map<M>(map: &mut Option<M>, fallback: &mut Option<String>, copy: impl FnOnce(&mut M) -> Result<(), String>) -> bool {
    let Some(mapper) = map.as_mut() else { return false };
    match copy(mapper) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("nightjar: NVDEC pictures go through memory to the GPU renderer from now on: {why}");
            *map = None;
            *fallback = Some(why);
            false
        }
    }
}

/// Times a `receive` that `started` and got `received`, FFmpeg handing over its picture at
/// `decoded`: a picture adds FFmpeg's receive and the hand-off, a poll that finds none (EAGAIN)
/// adds nothing to the next picture, and a failed receive or hand-off drops the picture's stages.
#[cfg(target_os = "linux")]
fn time_receive(stages: &mut Stages<3>, received: &Received, started: std::time::Instant, decoded: Option<std::time::Instant>) {
    match (received, decoded) {
        (Received::Frame(_), Some(at)) => {
            stages.add_between(1, started, at);
            stages.add(2, at);
            stages.commit();
        }
        (Received::None, _) => {}
        _ => stages.reset(),
    }
}

/// The decode stages' summary once they cover a stream's worth of pictures, not a probe's.
#[cfg(target_os = "linux")]
fn stage_summary(stages: &Stages<3>) -> Option<String> {
    (stages.frames() >= crate::stream::stage_times::SUMMARY_FRAMES).then(|| stages.summary()).flatten()
}

/// The presenter's kind (`FRAME_*`) and 10-bit flag of a picture of `format`, whose hardware
/// frames context holds `sw_format` pictures (None without one); `dmabuf`: a VAAPI picture
/// exported as DMA-BUFs. Kind 4 is a picture nothing here shows.
fn frame_kind(format: AVPixelFormat, sw_format: Option<AVPixelFormat>, dmabuf: bool) -> (c_int, c_int) {
    match format {
        AV_PIX_FMT_NV12 => (FRAME_NV12, 0),
        AV_PIX_FMT_P010LE => (FRAME_P010, 1),
        AV_PIX_FMT_YUV420P | AV_PIX_FMT_YUVJ420P => (FRAME_YUV420P, 0),
        AV_PIX_FMT_YUV444P | AV_PIX_FMT_YUVJ444P => (FRAME_YUV444P, 0),
        AV_PIX_FMT_YUV420P10LE => (FRAME_YUV420P10, 1),
        AV_PIX_FMT_YUV444P10LE => (FRAME_YUV444P10, 1),
        // NVDEC's 10-bit 4:4:4 copied to memory: its samples are in the high bits.
        AV_PIX_FMT_YUV444P16LE => (FRAME_YUV444P16, 1),
        // VideoToolbox: only NV12 and P010 surfaces copy into the presenter's textures; a 4:4:4
        // surface is a picture nothing here can show.
        AV_PIX_FMT_VIDEOTOOLBOX => match sw_format {
            Some(AV_PIX_FMT_NV12) => (FRAME_VIDEOTOOLBOX, 0),
            Some(AV_PIX_FMT_P010LE) => (FRAME_VIDEOTOOLBOX, 1),
            _ => (4, 0),
        },
        // NV12 and P010 surfaces copy into the presenter's textures. FFmpeg names the 4:4:4 ones
        // by their packing: AYUV is VUYX, Y410 XV30 and Y416 XV36 (XV48 for 16-bit samples).
        // Without a frames context, NV12.
        AV_PIX_FMT_D3D11 => match sw_format.unwrap_or(AV_PIX_FMT_NV12) {
            AV_PIX_FMT_NV12 => (FRAME_D3D11, 0),
            AV_PIX_FMT_P010LE => (FRAME_D3D11, 1),
            AV_PIX_FMT_VUYX | AV_PIX_FMT_VUYA => (FRAME_D3D11_444, 0),
            AV_PIX_FMT_XV30LE | AV_PIX_FMT_XV36LE => (FRAME_D3D11_444, 1),
            // Only FFmpeg builds as new as Windows' name XV48; D3D11 is Windows' alone.
            #[cfg(windows)]
            AV_PIX_FMT_XV48LE => (FRAME_D3D11_444, 1),
            _ => (4, 0),
        },
        #[cfg(unix)]
        AV_PIX_FMT_VULKAN => match sw_format.unwrap_or(AV_PIX_FMT_NONE) {
            AV_PIX_FMT_NV12 => (FRAME_VULKAN, 0),
            AV_PIX_FMT_P010LE => (FRAME_VULKAN, 1),
            AV_PIX_FMT_NV24 => (FRAME_VULKAN444, 0),
            AV_PIX_FMT_P410LE => (FRAME_VULKAN444, 1),
            // A mapped 4:4:4 VAAPI picture (`vulkan_video::interleaved_444`).
            AV_PIX_FMT_RGBA => (FRAME_VULKAN_INTERLEAVED444, 0),
            AV_PIX_FMT_XV30LE => (FRAME_VULKAN_INTERLEAVED444, 1),
            // Not two planes the presenter copies (`vulkan_video::Planes`).
            _ => (4, 0),
        },
        #[cfg(unix)]
        AV_PIX_FMT_VAAPI if dmabuf => (FRAME_DMABUF, i32::from(sw_format == Some(AV_PIX_FMT_P010LE))),
        _ => {
            let _ = dmabuf;
            (4, 0)
        }
    }
}

pub struct Frame {
    pub(crate) raw: NonNull<AVFrame>,
    wavelet: bool,
    /// The VAAPI surface's DMA-BUFs, for a presenter that imports them.
    #[cfg(unix)]
    dmabuf: Option<Box<crate::dmabuf::Export>>,
}

// An owned AVFrame reference can be handed from the decoder to the renderer.
unsafe impl Send for Frame {}

impl Frame {
    fn new(raw: NonNull<AVFrame>) -> Self {
        Self {
            raw,
            wavelet: false,
            #[cfg(unix)]
            dmabuf: None,
        }
    }

    /// Another reference to the same picture (its surface, for a hardware one), without the
    /// export: what keeps an imported surface from going back to the decoder while it is shown.
    pub fn reference(&self) -> Option<Self> {
        // SAFETY: a live frame; the clone is a new reference to its buffers.
        let mut frame = Self::new(NonNull::new(unsafe { av_frame_clone(self.raw.as_ptr()) })?);
        frame.wavelet = self.wavelet;
        Some(frame)
    }

    /// A VAAPI picture mapped for reading, not transferred: the driver's copy of the surface goes
    /// to the presenter's texture upload as it is, where a transfer copies it once more into a new
    /// frame first. The mapping holds the surface until the picture is dropped, as a D3D11
    /// picture does.
    pub fn map_to_memory(&self) -> Option<Self> {
        // SAFETY: a live frame; the mapping is a new frame of its own.
        unsafe {
            let memory = Self::new(NonNull::new(av_frame_alloc())?);
            (av_hwframe_map(memory.raw.as_ptr(), self.raw.as_ptr(), AV_HWFRAME_MAP_READ as c_int) >= 0
                && av_frame_copy_props(memory.raw.as_ptr(), self.raw.as_ptr()) >= 0)
                .then_some(memory)
        }
    }

    /// Whether FFmpeg says the picture is damaged: flagged corrupt (missing references, concealed
    /// slices) or with decode error flags.
    pub fn corrupt(&self) -> bool {
        // SAFETY: a live frame.
        let frame = unsafe { self.raw.as_ref() };
        frame.flags & AV_FRAME_FLAG_CORRUPT as c_int != 0 || frame.decode_error_flags != 0
    }

    /// The exported DMA-BUFs of a `FRAME_DMABUF` picture.
    #[cfg(unix)]
    pub fn dmabuf(&self) -> Option<&crate::dmabuf::Export> {
        self.dmabuf.as_deref()
    }

    pub fn wavelet(width: usize, height: usize) -> Result<Self, String> {
        unsafe {
            let mut frame = Self::new(NonNull::new(av_frame_alloc()).ok_or("Could not allocate wavelet frame")?);
            frame.wavelet = true;
            let raw = frame.raw.as_mut();
            raw.width = width as i32;
            raw.height = height as i32;
            raw.format = AV_PIX_FMT_RGBA64LE;
            if av_frame_get_buffer(frame.raw.as_ptr(), 32) < 0 { return Err("Could not allocate wavelet coefficients".into()); }
            Ok(frame)
        }
    }

    pub fn info(&self) -> FrameInfo {
        unsafe {
            let frame = self.raw.as_ref();
            let (kind, ten_bit) = if self.wavelet {
                (FRAME_WAVELET, 0)
            } else {
                let sw_format = (!frame.hw_frames_ctx.is_null()).then(|| (*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>())).sw_format);
                #[cfg(unix)]
                let dmabuf = self.dmabuf.is_some();
                #[cfg(not(unix))]
                let dmabuf = false;
                frame_kind(frame.format as AVPixelFormat, sw_format, dmabuf)
            };
            FrameInfo {
                width: frame.width,
                height: frame.height,
                kind,
                ten_bit,
                pts: frame.pts,
                data: [frame.data[0], frame.data[1], frame.data[2]],
                linesize: [frame.linesize[0], frame.linesize[1], frame.linesize[2]],
                color_space: frame.colorspace as i32,
                color_range: frame.color_range as i32,
                color_trc: frame.color_trc as i32,
            }
        }
    }

    pub fn format_name(&self) -> String {
        unsafe {
            let name = av_get_pix_fmt_name(self.raw.as_ref().format as AVPixelFormat);
            if name.is_null() {
                "unknown".into()
            } else {
                CStr::from_ptr(name).to_string_lossy().into_owned()
            }
        }
    }

    #[cfg(windows)]
    pub fn copy_to_d3d11(&self, texture: *mut c_void, width: i32, height: i32) -> bool {
        unsafe {
            use windows::core::Interface;
            use windows::Win32::Graphics::Direct3D11::{
                ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
            };
            let frame = self.raw.as_ref();
            if frame.format != AV_PIX_FMT_D3D11 || frame.hw_frames_ctx.is_null() {
                return false;
            }
            let frames = &*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>());
            let d3d = &*((*frames.device_ctx).hwctx.cast::<AVD3D11VADeviceContext>());
            let context_raw = d3d.device_context.cast();
            let source_raw = frame.data[0].cast();
            let Some(context) = ID3D11DeviceContext::from_raw_borrowed(&context_raw) else {
                return false;
            };
            let Some(source) = ID3D11Texture2D::from_raw_borrowed(&source_raw) else {
                return false;
            };
            let Some(target) = ID3D11Texture2D::from_raw_borrowed(&texture) else {
                return false;
            };
            let region = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: (width & !1) as u32,
                bottom: (height & !1) as u32,
                back: 1,
            };
            (d3d.lock.unwrap())(d3d.lock_ctx);
            context.CopySubresourceRegion(
                target,
                0,
                0,
                0,
                0,
                source,
                frame.data[1] as usize as u32,
                Some(&region),
            );
            (d3d.unlock.unwrap())(d3d.lock_ctx);
            true
        }
    }
}

/// A D3D11VA picture: the decoder's texture array and the picture's slice in it.
#[cfg(windows)]
pub struct D3d11Surface {
    pub texture: *mut c_void,
    pub slice: u32,
    /// The textures were made with `D3D11_BIND_SHADER_RESOURCE` (`bind_frames`).
    pub readable: bool,
}

#[cfg(windows)]
impl Frame {
    pub fn d3d11_surface(&self) -> Option<D3d11Surface> {
        // SAFETY: The fields of the live AVFrame and its frames context.
        unsafe {
            let frame = self.raw.as_ref();
            if frame.format != AV_PIX_FMT_D3D11 || frame.hw_frames_ctx.is_null() || frame.data[0].is_null() {
                return None;
            }
            let frames = &*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>());
            let d3d = &*frames.hwctx.cast::<AVD3D11VAFramesContext>();
            let readable = d3d.BindFlags & windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE.0 as u32 != 0;
            Some(D3d11Surface { texture: frame.data[0].cast(), slice: frame.data[1] as usize as u32, readable })
        }
    }

    /// Runs `f` under the D3D11VA device's lock, which FFmpeg holds while it decodes.
    pub fn with_decoder_lock(&self, f: &mut dyn FnMut()) {
        // SAFETY: The device context of the live frame's frames context.
        unsafe {
            let frame = self.raw.as_ref();
            if frame.hw_frames_ctx.is_null() {
                return f();
            }
            let frames = &*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>());
            let d3d = &*((*frames.device_ctx).hwctx.cast::<AVD3D11VADeviceContext>());
            match (d3d.lock, d3d.unlock) {
                (Some(lock), Some(unlock)) => {
                    lock(d3d.lock_ctx);
                    f();
                    unlock(d3d.lock_ctx);
                }
                _ => f(),
            }
        }
    }

    /// The frame's HDR10+ (SMPTE ST 2094-40) scene peak in nits: the largest MaxSCL of its first
    /// processing window, as libplacebo maps it (`10000 * maxscl`). None without HDR10+ or when
    /// the peak is 0.
    pub fn hdr10_plus_peak(&self) -> Option<f32> {
        // SAFETY: Side data of the live AVFrame, sized as FFmpeg made it.
        unsafe {
            let side = av_frame_get_side_data(self.raw.as_ptr(), AV_FRAME_DATA_DYNAMIC_HDR_PLUS);
            if side.is_null() || (*side).size < std::mem::size_of::<AVDynamicHDRPlus>() {
                return None;
            }
            let metadata = &*(*side).data.cast::<AVDynamicHDRPlus>();
            if metadata.num_windows == 0 {
                return None;
            }
            let peak = metadata.params[0]
                .maxscl
                .iter()
                .filter(|r| r.den != 0)
                .map(|r| 10000.0 * f64::from(r.num) / f64::from(r.den))
                .fold(0.0, f64::max);
            (peak > 0.0).then_some(peak as f32)
        }
    }

    /// What the frame's Dolby Vision RPU says, from the metadata FFmpeg's HEVC, AV1 and dav1d
    /// decoders attach to every frame with an RPU (`AV_FRAME_DATA_DOVI_METADATA`). None without one.
    pub fn dolby_vision(&self) -> Option<DolbyVision> {
        // SAFETY: Side data of the live AVFrame, sized as FFmpeg made it; the offsets are FFmpeg's
        // and checked against that size.
        unsafe {
            let side = av_frame_get_side_data(self.raw.as_ptr(), AV_FRAME_DATA_DOVI_METADATA);
            if side.is_null() || (*side).size < std::mem::size_of::<AVDOVIMetadata>() {
                return None;
            }
            let metadata = &*(*side).data.cast::<AVDOVIMetadata>();
            let within = |offset: usize, size: usize| offset.checked_add(size).is_some_and(|end| end <= (*side).size);
            if !within(metadata.header_offset, std::mem::size_of::<AVDOVIRpuDataHeader>())
                || !within(metadata.color_offset, std::mem::size_of::<AVDOVIColorMetadata>())
            {
                return None;
            }
            let header = &*(*side).data.add(metadata.header_offset).cast::<AVDOVIRpuDataHeader>();
            let color = &*(*side).data.add(metadata.color_offset).cast::<AVDOVIColorMetadata>();
            let level1 = av_dovi_find_level(metadata, 1);
            let max_pq = if level1.is_null() { None } else { Some((*level1).__bindgen_anon_1.l1.max_pq) };
            Some(DolbyVision::of(header.vdr_rpu_profile, header.bl_video_full_range_flag, max_pq, color.source_max_pq))
        }
    }
}

/// The parts of a Dolby Vision RPU the tone map takes.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DolbyVision {
    /// The frame's peak in nits: L1's maximum PQ, else the source display's maximum.
    pub peak: Option<f32>,
    /// Profile 5: IPT pictures without an HDR10 base layer (FFmpeg's `ff_dovi_guess_profile_hevc`:
    /// RPU profile 0 with a full-range base layer).
    pub ipt: bool,
}

#[cfg(windows)]
impl DolbyVision {
    /// From the RPU's profile and base-layer range, and its L1 and source maximum in 12-bit PQ.
    pub fn of(vdr_rpu_profile: u8, full_range_base: u8, l1_max_pq: Option<u16>, source_max_pq: u16) -> Self {
        let nits = |pq: u16| (pq > 0).then(|| crate::stream::direct::pq_nits(f64::from(pq.min(4095)) / 4095.0) as f32);
        Self {
            peak: l1_max_pq.and_then(nits).or_else(|| nits(source_max_pq)),
            ipt: vdr_rpu_profile == 0 && full_range_base != 0,
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe { av_frame_free(&mut self.raw.as_ptr()) };
    }
}

/// What the decoder hands the presenter: an FFmpeg frame, or (PyroWave decoded on the GPU) a set
/// of planes: D3D11 textures with the decode fence value that says when they are final on
/// Windows, CPU buffers (already final) on Linux.
pub enum Picture {
    Frame(Frame),
    GpuPlanes {
        lease: crate::stream::gpu_planes::Lease,
        #[cfg(windows)]
        fence_value: u64,
    },
}

impl Picture {
    /// The FFmpeg frame, for the pictures that are one.
    pub fn frame(&self) -> Option<&Frame> {
        match self {
            Self::Frame(frame) => Some(frame),
            Self::GpuPlanes { .. } => None,
        }
    }

    pub fn format_name(&self) -> String {
        match self {
            Self::Frame(frame) => frame.format_name(),
            Self::GpuPlanes { .. } => "PyroWave GPU planes".into(),
        }
    }
}

#[cfg(windows)]
pub fn swapchain_allows_tearing(swapchain: *mut c_void) -> bool {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{IDXGISwapChain1, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING};
    unsafe {
        IDXGISwapChain1::from_raw_borrowed(&swapchain)
            .and_then(|chain| chain.GetDesc1().ok())
            .is_some_and(|desc| desc.Flags & DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32 != 0)
    }
}

pub struct OpusDecoder {
    raw: NonNull<c_void>,
    pub channels: usize,
}

unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    pub fn new(channels: u8, streams: u8, coupled: u8, mapping: &[u8]) -> Result<Self, i32> {
        // Up to 7.1.4's 12 channels.
        if !(1..=12).contains(&channels) || mapping.len() < usize::from(channels) {
            return Err(-1);
        }
        let mut error = 0;
        let raw = unsafe {
            opus_multistream_decoder_create(
                48000,
                i32::from(channels),
                i32::from(streams),
                i32::from(coupled),
                mapping.as_ptr(),
                &mut error,
            )
        };
        NonNull::new(raw)
            .map(|raw| Self {
                raw,
                channels: usize::from(channels),
            })
            .ok_or(if error != 0 { error } else { -1 })
    }

    pub fn decode(&mut self, packet: Option<&[u8]>, pcm: &mut [f32]) -> i32 {
        let frame_size = (pcm.len() / self.channels) as i32;
        let (data, length) = packet.map_or((ptr::null(), 0), |p| (p.as_ptr(), p.len()));
        unsafe {
            opus_multistream_decode_float(
                self.raw.as_ptr(),
                data,
                length as i32,
                pcm.as_mut_ptr(),
                frame_size,
                0,
            )
        }
    }
}

/// A PCM packet (s16le, interleaved) as float samples in `pcm`, or `samples` frames of silence
/// for a lost one. Returns the frames written: the packet's, capped to what `pcm` holds.
pub fn pcm_s16le(packet: Option<&[u8]>, channels: usize, samples: usize, pcm: &mut [f32]) -> usize {
    let channels = channels.max(1);
    match packet {
        Some(bytes) => {
            let frames = (bytes.len() / 2 / channels).min(pcm.len() / channels);
            for (out, pair) in pcm.iter_mut().zip(bytes.as_chunks::<2>().0).take(frames * channels) {
                *out = f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32768.0;
            }
            frames
        }
        None => {
            let frames = samples.min(pcm.len() / channels);
            pcm[..frames * channels].fill(0.0);
            frames
        }
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        unsafe { opus_multistream_decoder_destroy(self.raw.as_ptr()) };
    }
}

#[cfg(test)]
mod pcm_tests {
    use super::pcm_s16le;

    #[test]
    fn pcm_packets_become_floats_and_lost_ones_silence() {
        let mut pcm = vec![9.0f32; 8];
        let bytes: Vec<u8> = [0i16, i16::MAX, i16::MIN, -16384].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(pcm_s16le(Some(&bytes), 2, 240, &mut pcm), 2);
        assert_eq!(&pcm[..4], &[0.0, 32767.0 / 32768.0, -1.0, -0.5]);
        assert_eq!(pcm[4], 9.0);
        // A lost packet is its duration of silence, capped to the buffer.
        assert_eq!(pcm_s16le(None, 2, 3, &mut pcm), 3);
        assert!(pcm[..6].iter().all(|&s| s == 0.0) && pcm[6] == 9.0);
        assert_eq!(pcm_s16le(None, 2, 240, &mut pcm), 4);
        // A packet longer than the buffer is cut to it.
        assert_eq!(pcm_s16le(Some(&[1; 40]), 2, 240, &mut pcm), 4);
    }
}

/// The codes of `tests/fixtures/hevc-444-10.bin`, a 256x256 HEVC RExt 4:4:4 10-bit picture
/// (limited range, no colour description) of 4x4 flat 64x64 blocks: block i (row * 4 + column)
/// has Y 65 + 53i, Cb 66 + 57 (5i mod 16) and Cr 67 + 59 (11i mod 16). libx265 (main444-10,
/// lossless, bframes=0) encoded it from exactly these codes.
#[cfg(test)]
pub(crate) fn hevc_444_10_fixture_code(plane: usize, x: usize, y: usize) -> u16 {
    let i = (y / 64 * 4 + x / 64) as u16;
    [65 + 53 * i, 66 + 57 * ((5 * i) % 16), 67 + 59 * ((11 * i) % 16)][plane]
}

#[cfg(test)]
mod tests;
