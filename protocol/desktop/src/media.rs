//! FFmpeg and Opus through Rust bindings to the platform libraries.

use std::ffi::{c_char, c_int, c_uchar, c_void, CStr};
use std::ptr::{self, NonNull};

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
    /// The layout of a `FRAME_YUV444P`, `FRAME_YUV420P10` or `FRAME_YUV444P10` picture.
    pub fn of(info: &FrameInfo) -> Option<Self> {
        let (chroma444, ten_bit) = match info.kind {
            FRAME_YUV444P => (true, false),
            FRAME_YUV420P10 => (false, true),
            FRAME_YUV444P10 => (true, true),
            _ => return None,
        };
        (info.width > 0 && info.height > 0).then_some(Self { width: info.width as u32, height: info.height as u32, chroma444, ten_bit })
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

    /// The layout of a `FRAME_VULKAN` or `FRAME_VULKAN444` picture's planes as the plane shader
    /// reads them once copied (`vulkan_video::Planes`), its chroma interleaved.
    #[cfg(unix)]
    pub fn of_vulkan(info: &FrameInfo) -> Option<Self> {
        let chroma444 = match info.kind {
            FRAME_VULKAN => false,
            FRAME_VULKAN444 => true,
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
    /// hardware pool gets these and two more beyond FFmpeg's minimum ([`pool_extra`]).
    pub waiting: usize,
    /// The GPU renderer's DRM render node (`/dev/dri/renderD<minor>`) when its device can take
    /// VAAPI's pictures mapped (`crate::vulkan_video::VaapiMap`): VAAPI opens there, on its GPU,
    /// and its pool gets the surfaces the presenter keeps for its copies. Linux only.
    pub vaapi_node: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    None,
    D3d11(*mut c_void),
    /// The whole `order` where no single backend is given: on Linux with the GPU renderer's SDL GPU
    /// device for Vulkan video (null when the renderer is another kind), and on Windows without a
    /// Direct3D 11 renderer.
    Auto(*mut c_void),
    Vaapi,
    /// FFmpeg's Vulkan video on the GPU renderer's own Vulkan device, this SDL GPU device (or
    /// null, which does not open), its pictures drawn without a copy to memory. Linux only.
    #[cfg_attr(not(unix), allow(dead_code))]
    Vulkan(*mut c_void),
    /// NVDEC, with CUDA frames copied to memory for the presenter.
    Cuda,
    /// The V4L2 memory-to-memory decoders of ARM handhelds and boards (H.264 and HEVC).
    V4l2m2m,
}

// The renderer keeps the device alive until the decoder has stopped; FFmpeg owns a COM reference.
unsafe impl Send for Hardware {}

impl Hardware {
    /// The backends to try, in turn, for this choice. On Linux that is moonlight-qt's and Moonlight
    /// V+'s order: the first-tier hwaccels (VAAPI, then Vulkan video for GPUs without VAAPI), then
    /// the hardware decoders outside FFmpeg's hwaccels (V4L2 M2M), then the second tier (NVDEC);
    /// software decoding comes after them all. With the GPU renderer's device, Vulkan video goes
    /// before VAAPI: its pictures stay on the renderer's device, where VAAPI's are read back and
    /// uploaded again (all but 8-bit 4:2:0 to the OpenGL renderer go through memory); VAAPI is
    /// next where Vulkan video does not open or FFmpeg refuses it for the stream.
    /// `NIGHTJAR_VULKAN_VIDEO_GPU_FIRST=0` keeps VAAPI first; `NIGHTJAR_VULKAN_VIDEO` puts Vulkan
    /// first whatever the renderer or leaves it out.
    pub fn order(self) -> Vec<Self> {
        match self {
            Self::None => Vec::new(),
            #[cfg(unix)]
            Self::Auto(gpu) => Self::linux_order(
                gpu,
                crate::vulkan_video::Choice::from_environment(),
                crate::vulkan_video::gpu_first(std::env::var_os(crate::vulkan_video::GPU_FIRST_ENVIRONMENT).as_deref()),
            ),
            // Vulkan video is Linux's; the rest of the order is tried as before it.
            #[cfg(not(unix))]
            Self::Auto(_) => vec![Self::Vaapi, Self::V4l2m2m, Self::Cuda],
            other => vec![other],
        }
    }

    #[cfg(any(unix, test))]
    fn linux_order(gpu: *mut c_void, choice: crate::vulkan_video::Choice, gpu_first: bool) -> Vec<Self> {
        use crate::vulkan_video::Choice;
        match choice {
            Choice::Default if gpu_first && !gpu.is_null() => vec![Self::Vulkan(gpu), Self::Vaapi, Self::V4l2m2m, Self::Cuda],
            Choice::Default => vec![Self::Vaapi, Self::Vulkan(gpu), Self::V4l2m2m, Self::Cuda],
            Choice::First => vec![Self::Vulkan(gpu), Self::Vaapi, Self::V4l2m2m, Self::Cuda],
            Choice::Off => vec![Self::Vaapi, Self::V4l2m2m, Self::Cuda],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "software",
            Self::D3d11(_) => "D3D11VA",
            Self::Auto(_) => "hardware",
            Self::Vaapi => "VAAPI",
            Self::Vulkan(_) => "Vulkan",
            Self::Cuda => "NVDEC",
            Self::V4l2m2m => "V4L2 M2M",
        }
    }
}

/// Whether VAAPI opens a device here: AMD and Intel GPUs do, NVIDIA's driver does not without
/// a VAAPI driver of its own (Vulkan video is used then, `crate::vulkan_video`).
#[cfg(unix)]
pub fn vaapi_device() -> Result<(), String> {
    let mut device = ptr::null_mut();
    // SAFETY: FFmpeg makes and frees a device of its own.
    let status = unsafe { av_hwdevice_ctx_create(&mut device, AV_HWDEVICE_TYPE_VAAPI, ptr::null(), ptr::null_mut(), 0) };
    if status < 0 {
        return Err(error(status));
    }
    // SAFETY: The reference made above.
    unsafe { av_buffer_unref(&mut device) };
    Ok(())
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
/// to be shown, the one on the screen and the one being drawn.
pub const EXTRA_HW_FRAMES: i32 = 6;

/// The pool's surfaces beyond FFmpeg's minimum for `waiting` pictures waiting to be shown, plus
/// the shown and the drawn one.
pub fn pool_extra(waiting: usize) -> i32 {
    (waiting.min(64) as i32 + 2).max(EXTRA_HW_FRAMES)
}

/// The surfaces a VAAPI pool gets beyond [`pool_extra`] when its pictures may be mapped for the
/// GPU renderer (`Pictures::vaapi_node`): those the presenter keeps until their copies are done.
fn mapped_extra(hardware: Hardware, pictures: Pictures) -> i32 {
    #[cfg(unix)]
    if hardware == Hardware::Vaapi && pictures.vaapi_node.is_some() {
        return crate::vulkan_video::MAPPED_HELD;
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

fn error(code: i32) -> String {
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
    _format: Box<Format>,
    pub name: String,
    /// The backend this decoder opened with.
    #[cfg_attr(not(unix), allow(dead_code))]
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
    /// The empty frame of the last `receive` that brought no picture, for the next one: each
    /// decoded frame ends with such a call, so this saves an allocation per frame. Pictures
    /// themselves go to the presenter and are freed there.
    spare: Option<Frame>,
    /// A hardware decoder made a picture that is not its hardware format: FFmpeg could not decode
    /// the stream with this backend after all and went to software. `told` once `renamed` has
    /// said so.
    refused: bool,
    told: bool,
}

/// The size of the pooled packet buffers for a packet of `needed` bytes with its padding: a
/// quarter more, in whole 64 KiB, so a stream whose frames grow slowly does not remake the pool
/// for each one.
fn packet_buffer_size(needed: usize) -> usize {
    (needed + needed / 4).next_multiple_of(64 * 1024)
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
pub const AGAIN: i32 = -11;

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
                spare: None,
                refused: false,
                told: false,
            };
            if video.context.is_null() || video.packet.is_null() {
                return Err("Could not allocate the video decoder.".into());
            }
            let context = &mut *video.context;
            context.opaque = (&mut *video._format as *mut Format).cast();
            let device_type = match hardware {
                Hardware::D3d11(_) => Some(AV_HWDEVICE_TYPE_D3D11VA),
                Hardware::Vaapi => Some(AV_HWDEVICE_TYPE_VAAPI),
                Hardware::Vulkan(_) => Some(AV_HWDEVICE_TYPE_VULKAN),
                Hardware::Cuda => Some(AV_HWDEVICE_TYPE_CUDA),
                Hardware::Auto(_) => return Err("the hardware order is not a backend of its own".into()),
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
                    Hardware::Vaapi if pictures.vaapi_node.is_some() => {
                        let node = vaapi_node_path(pictures.vaapi_node.unwrap_or_default());
                        let status = av_hwdevice_ctx_create(&mut context.hw_device_ctx, kind, node.as_ptr(), ptr::null_mut(), 0);
                        if status < 0 {
                            let why = format!("VAAPI does not open on {} ({})", node.to_string_lossy(), error(status));
                            eprintln!("nightjar: {why}; it opens on the default device, and its pictures go through memory to the GPU renderer");
                            video.vaapi_on_renderer = Err(why);
                            let status = av_hwdevice_ctx_create(&mut context.hw_device_ctx, kind, ptr::null(), ptr::null_mut(), 0);
                            if status < 0 {
                                return Err(format!("no device: {}", error(status)));
                            }
                        } else {
                            video.vaapi_on_renderer = Ok(());
                        }
                    }
                    Hardware::Vaapi | Hardware::Cuda => {
                        let status = av_hwdevice_ctx_create(
                            &mut context.hw_device_ctx,
                            kind,
                            ptr::null(),
                            ptr::null_mut(),
                            0,
                        );
                        if status < 0 {
                            return Err(format!("no device: {}", error(status)));
                        }
                    }
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
                    Hardware::None | Hardware::V4l2m2m | Hardware::Auto(_) => unreachable!(),
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
                Hardware::Vulkan(_) => " vulkan",
                Hardware::Cuda => " nvdec",
                Hardware::None | Hardware::V4l2m2m | Hardware::Auto(_) => "",
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

    /// Hands VAAPI pictures to the GPU renderer, the SDL GPU device `gpu`, mapped into Vulkan
    /// images on its device instead of through memory (nightjar#80: 2.5 GB/s read back and
    /// uploaded at 4K100 10-bit), unless `NIGHTJAR_VAAPI_VULKAN_MAP` is 0. Says which in the log,
    /// with the reason when they keep going through memory. Only a VAAPI decoder on Linux.
    #[cfg(unix)]
    pub fn map_to_vulkan(&mut self, gpu: *mut c_void) {
        if self.hardware != Hardware::Vaapi {
            return;
        }
        let switched_on = crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref());
        // SAFETY: The renderer keeps its device alive until the decoder has stopped.
        match unsafe { crate::vulkan_video::VaapiMap::new(gpu.cast(), switched_on, self.vaapi_on_renderer.clone()) } {
            Ok(map) => {
                eprintln!("nightjar: VAAPI pictures go to the GPU renderer as mapped Vulkan images");
                self.vulkan_map = Some(map);
            }
            Err(why) => eprintln!("nightjar: VAAPI pictures go through memory to the GPU renderer: {why}"),
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
        let data = if self.trims_padding { access_unit(data) } else { data };
        let Ok(length) = i32::try_from(data.len()) else {
            return -1;
        };
        let padding = AV_INPUT_BUFFER_PADDING_SIZE as usize;
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
        unsafe {
            avcodec_free_context(&mut self.context);
            av_packet_free(&mut self.packet);
            av_buffer_pool_uninit(&mut self.pool);
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
            let (kind, ten_bit) = if self.wavelet { (FRAME_WAVELET, 0) } else { match frame.format as AVPixelFormat {
                AV_PIX_FMT_NV12 => (FRAME_NV12, 0),
                AV_PIX_FMT_P010LE => (FRAME_P010, 1),
                AV_PIX_FMT_YUV420P | AV_PIX_FMT_YUVJ420P => (FRAME_YUV420P, 0),
                AV_PIX_FMT_YUV444P | AV_PIX_FMT_YUVJ444P => (FRAME_YUV444P, 0),
                AV_PIX_FMT_YUV420P10LE => (FRAME_YUV420P10, 1),
                AV_PIX_FMT_YUV444P10LE => (FRAME_YUV444P10, 1),
                AV_PIX_FMT_D3D11 => {
                    let sw_format = if frame.hw_frames_ctx.is_null() {
                        AV_PIX_FMT_NV12
                    } else {
                        (*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>())).sw_format
                    };
                    // Only NV12 and P010 surfaces copy into the presenter's textures; a 4:4:4
                    // surface (AYUV, Y410) is a picture nothing here can show.
                    match sw_format {
                        AV_PIX_FMT_NV12 => (FRAME_D3D11, 0),
                        AV_PIX_FMT_P010LE => (FRAME_D3D11, 1),
                        _ => (4, 0),
                    }
                }
                #[cfg(unix)]
                AV_PIX_FMT_VULKAN => match crate::vulkan_video::sw_format(frame) {
                    AV_PIX_FMT_NV12 => (FRAME_VULKAN, 0),
                    AV_PIX_FMT_P010LE => (FRAME_VULKAN, 1),
                    AV_PIX_FMT_NV24 => (FRAME_VULKAN444, 0),
                    AV_PIX_FMT_P410LE => (FRAME_VULKAN444, 1),
                    // Not two planes the presenter copies (`vulkan_video::Planes`).
                    _ => (4, 0),
                },
                #[cfg(unix)]
                AV_PIX_FMT_VAAPI if self.dmabuf.is_some() => {
                    let sw_format = if frame.hw_frames_ctx.is_null() {
                        AV_PIX_FMT_NONE
                    } else {
                        (*((*frame.hw_frames_ctx).data.cast::<AVHWFramesContext>())).sw_format
                    };
                    (FRAME_DMABUF, i32::from(sw_format == AV_PIX_FMT_P010LE))
                }
                _ => (4, 0),
            } };
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
            for (out, pair) in pcm.iter_mut().zip(bytes.chunks_exact(2)).take(frames * channels) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Opened without a packet timebase, V4L2 M2M rescaled each pts through the unset 0/1
    /// `time_base`, so no picture kept its frame's number.
    #[test]
    fn a_v4l2_picture_keeps_the_pts_of_its_frame() {
        let timebase = packet_timebase(Hardware::V4l2m2m);
        for number in [0u32, 1, 2, 61, u32::MAX] {
            let pts = (i64::from(number) << 32) | i64::from(number.wrapping_mul(1500));
            assert_eq!(v4l2_pts_round_trip(pts, timebase), pts, "frame {number}");
        }
        assert_ne!(v4l2_pts_round_trip(1 << 32, None), 1 << 32, "the unset timebase loses it");
        assert_eq!(packet_timebase(Hardware::None), None, "the other decoders are left as they were");
        assert_eq!(packet_timebase(Hardware::Vaapi), None);
    }

    #[test]
    fn software_decodes_packed_fixtures_and_preserves_timestamps() {
        for (codec, bytes, ten_bit) in [
            (
                Codec::Hevc,
                include_bytes!("../../../app/src/main/assets/packed444/hevc-8.bin").as_slice(),
                0,
            ),
            (
                Codec::Hevc,
                include_bytes!("../../../app/src/main/assets/packed444/hevc-10.bin").as_slice(),
                1,
            ),
            (
                Codec::Av1,
                include_bytes!("../../../app/src/main/assets/packed444/av1-8.bin").as_slice(),
                0,
            ),
            (
                Codec::Av1,
                include_bytes!("../../../app/src/main/assets/packed444/av1-10.bin").as_slice(),
                1,
            ),
        ] {
            let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
            assert_eq!(decoder.send(bytes, 12345), 0);
            let Received::Frame(frame) = decoder.receive() else {
                panic!("no decoded frame")
            };
            let info = frame.info();
            assert_eq!((info.width, info.height, info.pts), (128, 128, 12345));
            assert!(!info.data[0].is_null());
            if ten_bit == 0 {
                assert_eq!(info.kind, FRAME_YUV420P);
            }
            assert!(matches!(decoder.receive(), Received::None));
        }
    }

    #[test]
    fn access_units_lose_only_their_trailing_zeros() {
        assert_eq!(access_unit(&[0, 0, 0, 1, 0x7c, 0x01, 0x80, 0, 0, 0]), &[0, 0, 0, 1, 0x7c, 0x01, 0x80]);
        assert_eq!(access_unit(&[0, 0, 1, 2]), &[0, 0, 1, 2]);
        assert_eq!(access_unit(&[0, 0]), &[0, 0]);
        assert_eq!(access_unit(&[]), &[] as &[u8]);
    }

    /// A Rubylight host's profile 8.1 RPU (the host's `dovi::tests::GOLDEN`, read back by dovi_tool)
    /// after an HEVC Main10 picture and the last shard's padding: FFmpeg reads it, L1 peak and all.
    #[cfg(windows)]
    #[test]
    fn a_padded_hevc_frame_keeps_its_dolby_vision_rpu() {
        const RPU: &str = concat!(
            "000000017c0119080908406136506f003ff801ffc00fffd0000008000006800000400000340000030200",
            "000301a2566000035ea2566f9fceb1c256644ca00000100000030080000003008000000301c362243018",
            "60a5e308e0514000001a63e5affff000000300000300000300060207d80e15103008005dc32000c02b23",
            "8008008008008004000120c07d00064064001680d76703ed80",
        );
        let mut frame = include_bytes!("../../../app/src/main/assets/packed444/hevc-10.bin").to_vec();
        frame.extend((0..RPU.len()).step_by(2).map(|at| u8::from_str_radix(&RPU[at..at + 2], 16).unwrap()));
        frame.extend([0; 300]);
        let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
        assert_eq!(decoder.send(&frame, 1), 0);
        let Received::Frame(picture) = decoder.receive() else { panic!("no decoded frame") };
        let rpu = picture.dolby_vision().expect("the RPU's metadata on the picture");
        // L1 maximum PQ 3000 of 4095.
        let peak = crate::stream::direct::pq_nits(3000.0 / 4095.0) as f32;
        assert!((rpu.peak.unwrap() - peak).abs() < 1.0, "{:?} against {peak}", rpu.peak);
    }

    /// A VAAPI pool whose pictures may be mapped for the GPU renderer gets the surfaces the
    /// presenter keeps until their copies are done, and opens on the renderer's render node; no
    /// other pool changes (nightjar#80).
    #[test]
    fn a_vaapi_pool_that_maps_keeps_surfaces_for_the_presenters_copies() {
        let mapped = Pictures { vaapi_node: Some(129), ..Pictures::default() };
        assert_eq!(mapped_extra(Hardware::Vaapi, mapped), if cfg!(unix) { crate::vulkan_video::MAPPED_HELD } else { 0 });
        assert_eq!(mapped_extra(Hardware::Vaapi, Pictures::default()), 0);
        assert_eq!(mapped_extra(Hardware::Cuda, mapped), 0);
        assert_eq!(vaapi_node_path(129).to_str(), Ok("/dev/dri/renderD129"));
    }

    #[test]
    fn software_decodes_444_fixtures_with_full_chroma_and_10_bit_as_planar_kinds() {
        let source = include_bytes!("../tests/fixtures/yuv444-8.yuv");
        for (codec, bytes) in [
            (Codec::H264, include_bytes!("../tests/fixtures/h264-444-8.bin").as_slice()),
            (Codec::Hevc, include_bytes!("../tests/fixtures/hevc-444-8.bin").as_slice()),
            (Codec::Av1, include_bytes!("../tests/fixtures/av1-444-8.bin").as_slice()),
        ] {
            let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
            assert_eq!(decoder.send(bytes, 7), 0, "{codec:?}");
            let Received::Frame(frame) = decoder.receive() else { panic!("{codec:?}: no decoded frame") };
            let info = frame.info();
            // 256x256: a 64x64 probe was under NVDEC's minimum size and never offered NVIDIA 4:4:4.
            assert_eq!((info.kind, info.ten_bit, info.width, info.height), (FRAME_YUV444P, 0, 256, 256), "{codec:?}");
            // The fixtures are lossless: every plane is the full-size 64x64 source plane, each
            // sample scaled up to 4x4.
            for plane in 0..3 {
                for y in 0..256 {
                    // SAFETY: a 256x256 4:4:4 picture's planes hold 256 bytes per row.
                    let row = unsafe { std::slice::from_raw_parts(info.data[plane].add(y * info.linesize[plane] as usize), 256) };
                    let expected: Vec<u8> = (0..256).map(|x| source[(plane * 64 + y / 4) * 64 + x / 4]).collect();
                    assert_eq!(row, &expected[..], "{codec:?} plane {plane} row {y}");
                }
            }
        }
        for (codec, bytes) in [
            (Codec::Hevc, include_bytes!("../../../app/src/main/assets/packed444/hevc-10.bin").as_slice()),
            (Codec::Av1, include_bytes!("../../../app/src/main/assets/packed444/av1-10.bin").as_slice()),
        ] {
            let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
            assert_eq!(decoder.send(bytes, 7), 0);
            let Received::Frame(frame) = decoder.receive() else { panic!("{codec:?}: no decoded frame") };
            let info = frame.info();
            assert_eq!((info.kind, info.ten_bit), (FRAME_YUV420P10, 1), "{codec:?} Main10 in software");
        }
    }

    #[test]
    fn packet_buffers_come_from_one_pool_with_zeroed_padding_until_a_frame_outgrows_it() {
        let fixture =
            include_bytes!("../../../app/src/main/assets/packed444/hevc-8.bin").as_slice();
        let padding = AV_INPUT_BUFFER_PADDING_SIZE as usize;
        let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
        assert_eq!(decoder.send(fixture, 1), 0);
        let Received::Frame(frame) = decoder.receive() else {
            panic!("no decoded frame")
        };
        assert_eq!(frame.info().pts, 1);
        let (pool, size) = (decoder.pool, decoder.pool_size);
        assert!(!pool.is_null() && size >= fixture.len() + padding && size % (64 * 1024) == 0);
        // A shorter packet takes a buffer from the same pool (the one the whole frame was in, once
        // the decoder let go of it); the bytes after it are padding again, not the rest of that frame.
        let short = &fixture[..fixture.len() / 2];
        assert!(fixture[short.len()..short.len() + padding]
            .iter()
            .any(|&b| b != 0));
        decoder.send(short, 2);
        assert_eq!((decoder.pool, decoder.pool_size), (pool, size));
        unsafe {
            let packet = &*decoder.packet;
            assert_eq!(packet.size as usize, short.len());
            assert_eq!(std::slice::from_raw_parts(packet.data, short.len()), short);
            assert!(
                std::slice::from_raw_parts(packet.data.add(short.len()), padding)
                    .iter()
                    .all(|&b| b == 0)
            );
        }
        // A packet that does not fit makes a larger pool.
        decoder.send(&vec![0u8; size], 3);
        assert!(
            decoder.pool_size >= size + padding
                && decoder.pool_size == packet_buffer_size(size + padding)
        );
        // Frame after frame from the pool decodes with its own timestamp.
        let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
        for pts in 10..13 {
            assert_eq!(decoder.send(fixture, pts), 0);
            let Received::Frame(frame) = decoder.receive() else {
                panic!("no decoded frame")
            };
            assert_eq!(frame.info().pts, pts);
        }
    }

    #[test]
    fn the_frame_of_a_receive_without_a_picture_holds_the_next_picture() {
        let fixture =
            include_bytes!("../../../app/src/main/assets/packed444/hevc-8.bin").as_slice();
        let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
        for pts in 20..23 {
            assert!(matches!(decoder.receive(), Received::None));
            let spare = decoder.spare.as_ref().expect("the empty frame is kept").raw;
            assert_eq!(decoder.send(fixture, pts), 0);
            let Received::Frame(frame) = decoder.receive() else {
                panic!("no decoded frame")
            };
            assert_eq!(frame.raw, spare, "the picture is decoded into the kept frame");
            assert_eq!(frame.info().pts, pts);
            assert!(decoder.spare.is_none(), "the picture left with its frame");
        }
    }

    /// The fixtures' RPUs are the Rubylight host's (`rust/core/src/dovi.rs`, profiles 8.1 and
    /// 10.1): L1 maximum 3079 (1000 nits), source display 0.005 to 4000 nits.
    #[cfg(windows)]
    #[test]
    fn a_dolby_vision_rpu_reaches_the_frame_as_its_peak() {
        let hevc = include_bytes!("../../../app/src/main/assets/packed444/hevc-10.bin").as_slice();
        let av1 = include_bytes!("../../../app/src/main/assets/packed444/av1-10.bin").as_slice();
        // Profile 8.1: the RPU NAL unit ends the access unit. Profile 10.1: the metadata OBU
        // follows the temporal delimiter.
        let mut hevc_dv = hevc.to_vec();
        hevc_dv.extend_from_slice(include_bytes!("../tests/fixtures/dovi-81-rpu.bin"));
        assert_eq!(&av1[..2], &[0x12, 0], "the AV1 fixture starts with a temporal delimiter");
        let av1_dv = [&av1[..2], include_bytes!("../tests/fixtures/dovi-101-rpu.bin"), &av1[2..]].concat();
        for (codec, plain, with_rpu) in [(Codec::Hevc, hevc, hevc_dv.as_slice()), (Codec::Av1, av1, av1_dv.as_slice())] {
            let decode = |bytes: &[u8]| {
                let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
                assert_eq!(decoder.send(bytes, 1), 0, "{codec:?}");
                let Received::Frame(frame) = decoder.receive() else { panic!("{codec:?}: no decoded frame") };
                frame.dolby_vision()
            };
            assert_eq!(decode(plain), None, "{codec:?} without an RPU");
            let rpu = decode(with_rpu).unwrap_or_else(|| panic!("{codec:?}: no Dolby Vision metadata"));
            assert!(!rpu.ipt, "{codec:?}: an HDR10 base layer");
            let peak = rpu.peak.unwrap();
            assert!((peak - 1000.0).abs() < 2.0, "{codec:?}: L1's 1000 nits, not the source's 4000: {peak}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_dolby_vision_peak_comes_from_l1_else_the_source_display_and_profile_5_is_ipt() {
        let peak = |l1, source| DolbyVision::of(1, 0, l1, source).peak;
        assert!((peak(Some(3079), 3696).unwrap() - 1000.0).abs() < 2.0);
        assert!((peak(None, 3696).unwrap() - 4000.0).abs() < 10.0, "no L1: the source display's maximum");
        assert!((peak(Some(0), 3079).unwrap() - 1000.0).abs() < 2.0, "an empty L1 is no peak");
        assert_eq!(peak(None, 0), None);
        assert!((peak(Some(u16::MAX), 0).unwrap() - 10000.0).abs() < 1.0, "codes past 12 bits are the top");
        // ff_dovi_guess_profile_hevc: RPU profile 0 with a full-range base layer is profile 5.
        assert!(DolbyVision::of(0, 1, Some(3079), 0).ipt);
        assert!(!DolbyVision::of(1, 1, Some(3079), 0).ipt && !DolbyVision::of(0, 0, Some(3079), 0).ipt);
    }

    /// FFmpeg takes a hardware decoder to software without an error (HEVC Main10 on a GPU without
    /// it): a picture that is not the backend's says so for every backend, not only Vulkan.
    #[test]
    fn a_picture_in_memory_from_any_hardware_decoder_says_it_was_refused() {
        for hardware in [AV_PIX_FMT_D3D11, AV_PIX_FMT_VAAPI, AV_PIX_FMT_VULKAN, AV_PIX_FMT_CUDA] {
            assert!(!refused_by(hardware, hardware));
            assert!(refused_by(hardware, AV_PIX_FMT_NV12));
            assert!(refused_by(hardware, AV_PIX_FMT_YUV420P10LE));
        }
        // Software and V4L2 M2M decoders have no hardware format to be refused.
        assert!(!refused_by(AV_PIX_FMT_NONE, AV_PIX_FMT_NV12));
        let device = ptr::dangling_mut::<c_void>();
        assert_eq!(refused_name("hevc d3d11va", Hardware::D3d11(device)), "hevc (software, D3D11VA refused)");
        assert_eq!(refused_name("av1 vaapi", Hardware::Vaapi), "av1 (software, VAAPI refused)");
        assert_eq!(refused_name("hevc vulkan", Hardware::Vulkan(device)), "hevc (software, Vulkan refused)");
        assert_eq!(refused_name("h264 nvdec", Hardware::Cuda), "h264 (software, NVDEC refused)");
    }

    /// A software decoder is synchronous and never refused; a clean picture is not damaged.
    #[test]
    fn a_software_decoder_is_synchronous_and_its_clean_pictures_are_not_damaged() {
        let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
        assert!(!decoder.asynchronous() && !decoder.refused() && !decoder.pooled());
        assert!(decoder.send(include_bytes!("../../../app/src/main/assets/packed444/hevc-8.bin"), 7 << 32) >= 0);
        let Received::Frame(frame) = decoder.receive() else { panic!("no decoded frame") };
        assert!(!frame.corrupt());
        assert_eq!(frame.info().pts, 7 << 32);
        assert!(!decoder.refused() && decoder.renamed().is_none());
    }

    #[test]
    fn packet_buffers_leave_room_to_grow_in_whole_64_kib() {
        assert_eq!(packet_buffer_size(1), 64 * 1024);
        assert_eq!(packet_buffer_size(64 * 1024), 128 * 1024);
        assert_eq!(packet_buffer_size(1 << 20), (1 << 20) + (256 << 10));
        for needed in [100, 65_535, 300_000, 5_000_000] {
            let size = packet_buffer_size(needed);
            assert!(size >= needed + needed / 4 && size % (64 * 1024) == 0);
        }
    }

    #[test]
    fn linux_tries_vaapi_then_vulkan_then_v4l2_m2m_then_nvdec_and_other_choices_alone() {
        use crate::vulkan_video::Choice;
        let device = ptr::dangling_mut::<c_void>();
        let (vaapi, vulkan, v4l2, nvdec) = (Hardware::Vaapi, Hardware::Vulkan(device), Hardware::V4l2m2m, Hardware::Cuda);
        // VAAPI's pictures cross memory to the GPU renderer: with its device Vulkan video goes first.
        assert_eq!(Hardware::linux_order(device, Choice::Default, true), [vulkan, vaapi, v4l2, nvdec]);
        assert_eq!(Hardware::linux_order(device, Choice::Default, false), [vaapi, vulkan, v4l2, nvdec], "NIGHTJAR_VULKAN_VIDEO_GPU_FIRST=0");
        let other_renderer = Hardware::Vulkan(ptr::null_mut());
        assert_eq!(Hardware::linux_order(ptr::null_mut(), Choice::Default, true), [vaapi, other_renderer, v4l2, nvdec], "without the GPU renderer VAAPI stays first");
        for gpu_first in [true, false] {
            assert_eq!(Hardware::linux_order(device, Choice::First, gpu_first), [vulkan, vaapi, v4l2, nvdec], "NIGHTJAR_VULKAN_VIDEO=1");
            assert_eq!(Hardware::linux_order(device, Choice::Off, gpu_first), [vaapi, v4l2, nvdec], "NIGHTJAR_VULKAN_VIDEO=0");
        }
        assert!(crate::vulkan_video::gpu_first(None));
        assert!(crate::vulkan_video::gpu_first(Some("1".as_ref())));
        assert!(!crate::vulkan_video::gpu_first(Some("0".as_ref())));
        assert_eq!(Hardware::Vulkan(device).label(), "Vulkan");
        assert!(Hardware::None.order().is_empty());
        assert_eq!(Hardware::D3d11(device).order(), [Hardware::D3d11(device)]);
        assert_eq!(Hardware::Vaapi.order(), [Hardware::Vaapi]);
        assert_eq!(Hardware::Cuda.order(), [Hardware::Cuda]);
    }

    /// Without the GPU renderer's device, or for packed 4:4:4, Vulkan video does not open and
    /// says why, so the next backend is tried.
    #[test]
    fn vulkan_video_without_the_renderers_device_or_for_packed_chroma_names_the_reason() {
        let error = VideoDecoder::open_for(Codec::Hevc, Pictures::default(), Hardware::Vulkan(ptr::null_mut()), 1, false).err().unwrap();
        if cfg!(unix) {
            assert!(error.contains("does not draw with SDL's GPU API") || error.contains("libavutil"), "{error}");
            let packed = Pictures { packed: true, ..Pictures::default() };
            let error = VideoDecoder::open_for(Codec::Hevc, packed, Hardware::Vulkan(ptr::null_mut()), 1, false).err().unwrap();
            assert!(error.contains("packed 4:4:4"), "{error}");
        } else {
            assert!(error.contains("only used on Linux") || error.contains("no Vulkan support"), "{error}");
        }
        let error = VideoDecoder::open(Codec::H264, Hardware::Auto(ptr::null_mut()), 1, false).err().unwrap();
        assert!(error.contains("not a backend"), "{error}");
    }

    #[test]
    fn the_first_backend_that_opens_is_used_and_each_one_before_it_is_logged_once() {
        let mut logged = Vec::new();
        let mut tried = Vec::new();
        let order = vec![Hardware::Vaapi, Hardware::V4l2m2m, Hardware::Cuda];
        let opened = first_opened(
            order.clone(),
            |backend| {
                tried.push(backend);
                if backend == Hardware::V4l2m2m { Ok("v4l2") } else { Err(format!("no {}", backend.label())) }
            },
            |backend, error| logged.push(format!("{} {error}", backend.label())),
        );
        assert_eq!(opened, Ok("v4l2"));
        assert_eq!(tried, [Hardware::Vaapi, Hardware::V4l2m2m], "NVDEC is not tried once one opened");
        assert_eq!(logged, ["VAAPI no VAAPI"]);

        logged.clear();
        let failed: Result<(), String> = first_opened(order, |_| Err("gone".into()), |backend, _| logged.push(backend.label().into()));
        assert_eq!(failed.unwrap_err(), "VAAPI: gone; V4L2 M2M: gone; NVDEC: gone");
        assert_eq!(logged, ["VAAPI", "V4L2 M2M", "NVDEC"]);
        let none: Result<(), String> = first_opened(Vec::new(), |_| Ok(()), |_, _| panic!("nothing was tried"));
        assert!(none.is_err());
    }

    #[test]
    fn a_held_reference_keeps_the_picture_and_a_software_picture_does_not_map() {
        let fixture = include_bytes!("../../../app/src/main/assets/packed444/hevc-8.bin").as_slice();
        let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
        assert_eq!(decoder.hardware, Hardware::None);
        assert!(!decoder.export_dmabuf(), "only a VAAPI decoder exports");
        assert_eq!(decoder.send(fixture, 42), 0);
        let Received::Frame(frame) = decoder.receive() else { panic!("no decoded frame") };
        let held = frame.reference().expect("a reference");
        let info = frame.info();
        drop(frame);
        let kept = held.info();
        assert_eq!((kept.pts, kept.width, kept.data[0]), (42, info.width, info.data[0]), "the reference is the same picture");
        assert!(held.map_to_memory().is_none(), "only hardware pictures map");
    }

    #[test]
    fn invalid_opus_mapping_is_rejected_before_ffi() {
        assert!(OpusDecoder::new(13, 1, 0, &[0; 13]).is_err());
        assert!(OpusDecoder::new(12, 8, 4, &[0; 11]).is_err());
        assert!(OpusDecoder::new(2, 1, 1, &[0]).is_err());
    }

    /// 7.1.4 in Foundation Sunshine's layouts opens a decoder.
    #[test]
    fn seven_one_four_opens_an_opus_decoder() {
        use rubylight_protocol::audio_ext::{LAYOUT_714, LAYOUT_714_HQ};
        for layout in [LAYOUT_714, LAYOUT_714_HQ] {
            let decoder =
                OpusDecoder::new(12, layout.streams, layout.coupled, &layout.mapping).unwrap();
            assert_eq!(decoder.channels, 12);
        }
    }
}
