//! FFmpeg `VideoDecoder` lifecycle: hardware format negotiation, context creation, flushing,
//! reopening, and teardown.

use std::ffi::{c_int, CStr};
use std::ptr;

use nightjar_core::video::FrameBytes;

#[cfg(target_os = "linux")]
use crate::stream::stage_times::Stages;

#[cfg(windows)]
use super::d3d11;
#[cfg(target_os = "linux")]
use super::decoder_io::stage_summary;
use super::error;
use super::ffi::*;
use super::frame::Frame;
use super::hardware::{first_opened, mapped_extra, pool_extra, Codec, Hardware, Pictures};
// Linux and macOS (unix): VAAPI render node path helper and node kind.
#[cfg(unix)]
use super::hardware::{vaapi_node_path, VaapiNode};

pub(super) struct Format {
    pub(super) hardware: AVPixelFormat,
    required: bool,
    /// `select_format` found no way to decode the stream with `hardware` and gave FFmpeg a
    /// software format instead (`VideoDecoder::refused`).
    pub(super) software: std::cell::Cell<bool>,
    /// D3D11VA: make the decoder's textures readable by shaders (`stream::direct`).
    #[cfg(windows)]
    bind_textures: bool,
    /// Vulkan video: the pictures the device was checked for, whose two-plane format the frames
    /// context is made in (`vulkan_video::frames_context`).
    // Linux and macOS (unix): Vulkan video frame-context parameters.
    #[cfg(unix)]
    vulkan: Option<Pictures>,
}

unsafe extern "C" fn select_format(
    context: *mut AVCodecContext,
    formats: *const AVPixelFormat,
) -> AVPixelFormat {
    // SAFETY: FFmpeg passes valid `context` (with our `Format` in `opaque`) and `AV_PIX_FMT_NONE`-terminated `formats`.
    unsafe {
        let wanted = &*((*context).opaque.cast::<Format>());
        let mut next = formats;
        while *next != AV_PIX_FMT_NONE {
            if *next == wanted.hardware {
                #[cfg(windows)]
                if wanted.bind_textures && *next == AV_PIX_FMT_D3D11
                    && let Err(reason) = bind_frames(context, *next) {
                        eprintln!("nightjar: the decoder's textures cannot be made readable ({reason}); its pictures are copied instead");
                    }
                // Linux and macOS (unix): configure Vulkan video frames context during get_format.
                #[cfg(unix)]
                if let Some(pictures) = wanted.vulkan
                    && let Err(reason) = crate::vulkan_video::frames_context(context, pictures)
                {
                    // The decoder then says it was refused, and the stream opens the next backend.
                    eprintln!("nightjar: Vulkan video cannot decode into the presenter's format: {reason}");
                    break;
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
}

/// Makes the decoder's frames context with shader-readable textures, as Moonlight V+ PC does
/// (d3d11va.cpp, prepareDecoderContextInGetFormat), so `stream::direct` reads its slices in place.
/// On failure the context is left to FFmpeg's default.
#[cfg(windows)]
unsafe fn bind_frames(context: *mut AVCodecContext, format: AVPixelFormat) -> Result<(), String> {
    unsafe extern "C" {
        fn av_hwframe_ctx_init(reference: *mut AVBufferRef) -> c_int;
    }
    // SAFETY: caller guarantees `context` is an active FFmpeg codec context.
    unsafe {
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
}

pub struct VideoDecoder {
    pub(super) context: *mut AVCodecContext,
    /// The codec `context` was opened for: kept to open a new context after a failure, on the
    /// same hardware device (`reopen`).
    found: *const AVCodec,
    /// The `AVCodecContext::extra_hw_frames` the decoder was opened with (`reopen`).
    extra_hw_frames: i32,
    pub(super) packet: *mut AVPacket,
    /// Packet buffers of `pool_size` bytes, reused frame to frame: a buffer allocated per frame
    /// costs page faults once frames are large (from about 1 MiB, high bitrates and key frames,
    /// Windows' heap maps a fresh block for each), measured at 0.14 ms per MiB on Windows 11.
    pub(super) pool: *mut AVBufferPool,
    pub(super) pool_size: usize,
    /// The bytes of the frame being sent (`lend`): a packet whose data lies in them goes to FFmpeg
    /// by reference instead of through a copy into `pool`.
    pub(super) lent: Option<FrameBytes>,
    /// Packets that went to FFmpeg by reference rather than copied, for the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) lent_packets: u64,
    pub(super) _format: Box<Format>,
    pub name: String,
    /// The backend this decoder opened with.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub hardware: Hardware,
    /// HEVC access units go to FFmpeg without the last shard's zero padding (`access_unit`).
    pub(super) trims_padding: bool,
    /// VAAPI pictures go to the presenter as DMA-BUFs until the flag says the presenter cannot
    /// import them (`export_dmabuf`).
    // Linux and macOS (unix): DMA-BUF export opt-out flag.
    #[cfg(unix)]
    pub(super) dmabuf: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Whether VAAPI opened on the renderer's render node (`Pictures::vaapi_node`), or why not.
    // Linux and macOS (unix): tracks whether VAAPI opened on the renderer GPU.
    #[cfg(unix)]
    pub(super) vaapi_on_renderer: Result<(), String>,
    /// VAAPI pictures go to the GPU renderer mapped into Vulkan images (`map_to_vulkan`).
    // Linux and macOS (unix): VAAPI-to-Vulkan mapper state.
    #[cfg(unix)]
    pub(super) vulkan_map: Option<crate::vulkan_video::VaapiMap>,
    /// NVDEC pictures go to the GPU renderer copied by CUDA into Vulkan images (`map_to_vulkan`).
    // Linux and macOS (unix): CUDA-to-Vulkan mapper state.
    #[cfg(unix)]
    pub(super) cuda_map: Option<crate::vulkan_video::CudaMap>,
    /// Why NVDEC pictures go through memory to the GPU renderer after all (the CUDA copy could not
    /// be set up, or a copy failed), until the stream takes it for its toast (`take_cuda_fallback`).
    // Linux and macOS (unix): CUDA fallback diagnostic reason.
    #[cfg(unix)]
    pub(super) cuda_fallback: Option<String>,
    /// The empty frame of the last `receive` that brought no picture, for the next one: each
    /// decoded frame ends with such a call, so this saves an allocation per frame. Pictures
    /// themselves go to the presenter and are freed there.
    pub(super) spare: Option<Frame>,
    /// A hardware decoder made a picture that is not its hardware format: FFmpeg could not decode
    /// the stream with this backend after all and went to software. `told` once `renamed` has
    /// said so.
    pub(super) refused: bool,
    pub(super) told: bool,
    /// Separate `D3D11_CREATE_DEVICE_VIDEO_SUPPORT` decode device bridged to SDL3's renderer via
    /// NT-handle shared textures and monitored `ID3D11Fence`s. None when decoding in software,
    /// running a probe-only device, or falling back to SDL3's shared renderer device.
    #[cfg(windows)]
    pub(super) d3d11_bridge: Option<std::sync::Arc<d3d11::DecodeBridge>>,
    /// A hardware decoder's time per picture in `send`, in FFmpeg's `receive` and in handing the
    /// picture on (DMA-BUF export, Vulkan map, copy to memory), summed up in the log when it
    /// closes after a stream's worth of pictures. Linux only.
    #[cfg(target_os = "linux")]
    pub(super) stages: Option<Box<Stages<3>>>,
}

/// The stages of `VideoDecoder::stages`.
#[cfg(target_os = "linux")]
pub(super) const DECODE_STAGES: [&str; 3] = ["send", "receive", "hand-off"];

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

/// The size a V4L2 M2M decoder is opened with (`Pictures::width`/`height`), when it is known.
/// FFmpeg copies it into its output and capture queues at open (`v4l2_m2m_dec.c`, the
/// `avctx->coded_width`/`coded_height` assignments), so the driver's first
/// `V4L2_EVENT_SOURCE_CHANGE` finds the size it already has and only starts decoding; without it,
/// the same event counts as a resolution change and FFmpeg reinitialises the capture buffers there
/// (Nightjar issue 77). Only V4L2 M2M: FFmpeg's other decoders take a preset size as a hint, and
/// their bitstreams set it anyway.
pub(super) fn v4l2_open_size(hardware: Hardware, pictures: Pictures) -> Option<(c_int, c_int)> {
    (hardware == Hardware::V4l2m2m && pictures.width > 0 && pictures.height > 0)
        .then_some((pictures.width as c_int, pictures.height as c_int))
}

/// Dimensions preserved across `VideoDecoder::reopen` for V4L2 M2M so `avcodec_open2` receives
/// the same preset size it opened with.
pub(super) fn reopen_size(
    hardware: Hardware,
    width: c_int,
    height: c_int,
    coded_width: c_int,
    coded_height: c_int,
) -> Option<(c_int, c_int, c_int, c_int)> {
    (hardware == Hardware::V4l2m2m && width > 0 && height > 0).then_some((
        width,
        height,
        if coded_width > 0 { coded_width } else { width },
        if coded_height > 0 { coded_height } else { height },
    ))
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
// Unix (Linux and macOS): POSIX EAGAIN value from libc.
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
                found,
                extra_hw_frames: 0,
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
                    // Linux and macOS (unix): Vulkan video format negotiation parameters.
                    #[cfg(unix)]
                    vulkan: matches!(hardware, Hardware::Vulkan(_)).then_some(pictures),
                }),
                name: CStr::from_ptr((*found).name).to_string_lossy().into_owned(),
                hardware,
                trims_padding: codec == Codec::Hevc,
                // Linux and macOS (unix): platform DMA-BUF / Vulkan / CUDA mapper state.
                #[cfg(unix)]
                dmabuf: None,
                // Linux and macOS (unix): renderer render-node VAAPI status.
                #[cfg(unix)]
                vaapi_on_renderer: Err("VAAPI was not asked to open on the renderer's GPU".into()),
                // Linux and macOS (unix): VAAPI-to-Vulkan mapper state.
                #[cfg(unix)]
                vulkan_map: None,
                // Linux and macOS (unix): CUDA-to-Vulkan mapper state.
                #[cfg(unix)]
                cuda_map: None,
                // Linux and macOS (unix): CUDA fallback reason.
                #[cfg(unix)]
                cuda_fallback: None,
                spare: None,
                refused: false,
                told: false,
                #[cfg(windows)]
                d3d11_bridge: None,
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
                            let (bridge, bind) = d3d11::open_device_context(context, kind, device, pictures)?;
                            video._format.bind_textures = bind;
                            video.d3d11_bridge = bridge;
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = device;
                            return Err("D3D11VA is only available on Windows.".into());
                        }
                    }
                    // Linux and macOS (unix): open VAAPI on the renderer's DRM render node when known.
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
                        // Linux and macOS (unix): share SDL's Vulkan GPU device with FFmpeg.
                        #[cfg(unix)]
                        {
                            context.hw_device_ctx = crate::vulkan_video::device_context(gpu.cast(), codec, pictures)?;
                        }
                        // Non-Unix platforms (Windows): Vulkan video decode is not used.
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
                video.extra_hw_frames = context.extra_hw_frames;
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
            let v4l2_size = v4l2_open_size(hardware, pictures);
            if let Some((width, height)) = v4l2_size {
                context.width = width;
                context.height = height;
                context.coded_width = width;
                context.coded_height = height;
            }
            let status = avcodec_open2(video.context, found, &mut options);
            av_dict_free(&mut options);
            if status < 0 {
                return Err(format!("Opening the decoder: {}", error(status)));
            }
            if let Some((width, height)) = v4l2_size {
                eprintln!("nightjar: V4L2 M2M {} opened for {width}x{height}", video.name);
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

    /// Empties the decoder for a stream that starts over on it (a reconnect reusing it): no
    /// picture of the last stream can come out, and the maps to the renderer are made again for
    /// the stream's own (`pipeline::Decoder::install`).
    pub fn flush(&mut self) {
        // SAFETY: the context is allocated and opened; flushing it drops its queued packets and
        // reference pictures.
        unsafe { avcodec_flush_buffers(self.context) };
        self.lent = None;
        self.spare = None;
        self.refused = false;
        self.told = false;
        self._format.software.set(false);
        // Linux and macOS (unix): clear DMA-BUF, Vulkan, and CUDA mappers across stream restart.
        #[cfg(unix)]
        {
            self.dmabuf = None;
            self.vulkan_map = None;
            self.cuda_map = None;
            self.cuda_fallback = None;
        }
    }

    /// Opens the codec context again for the same stream parameters, on the same hardware device:
    /// after a decoder failed, a new context is what recovers, but a VA-API, Vulkan, D3D11VA or
    /// VideoToolbox device costs what the stream's start did to make. The pictures' pool and any
    /// frame in flight go with the old context.
    pub fn reopen(&mut self) -> Result<(), String> {
        unsafe {
            let mut previous = self.context;
            let before = &mut *previous;
            let device = std::mem::replace(&mut before.hw_device_ctx, ptr::null_mut());
            let (threads, timebase) = (before.thread_count, before.pkt_timebase);
            let dimensions = reopen_size(self.hardware, before.width, before.height, before.coded_width, before.coded_height);
            let fresh = avcodec_alloc_context3(self.found);
            if fresh.is_null() {
                before.hw_device_ctx = device;
                return Err("Could not allocate the video decoder again.".into());
            }
            self.context = fresh;
            avcodec_free_context(&mut previous);
            self.lent = None;
            self.spare = None;
            self.refused = false;
            self.told = false;
            let context = &mut *fresh;
            context.opaque = (&mut *self._format as *mut Format).cast();
            context.hw_device_ctx = device;
            self._format.software.set(false);
            if self.extra_hw_frames != 0 {
                context.extra_hw_frames = self.extra_hw_frames;
                context.get_format = Some(select_format);
            }
            context.thread_count = threads;
            context.thread_type = FF_THREAD_SLICE as i32;
            context.pkt_timebase = timebase;
            context.flags |= AV_CODEC_FLAG_LOW_DELAY as i32;
            context.flags2 |= AV_CODEC_FLAG2_FAST as i32;
            if let Some((width, height, coded_width, coded_height)) = dimensions {
                context.width = width;
                context.height = height;
                context.coded_width = coded_width;
                context.coded_height = coded_height;
            }
            let mut options = ptr::null_mut();
            if self.name == "libdav1d" {
                av_dict_set(&mut options, c"max_frame_delay".as_ptr(), c"1".as_ptr(), 0);
            }
            let status = avcodec_open2(self.context, self.found, &mut options);
            av_dict_free(&mut options);
            if status < 0 {
                return Err(format!("Opening the decoder again: {}", error(status)));
            }
            Ok(())
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
