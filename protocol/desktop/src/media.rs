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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    None,
    D3d11(*mut c_void),
    /// On Linux this stands for the whole `order`: VAAPI first.
    Vaapi,
    /// NVDEC, with CUDA frames copied to memory for the presenter.
    Cuda,
    /// The V4L2 memory-to-memory decoders of ARM handhelds and boards (H.264 and HEVC).
    V4l2m2m,
}

// The renderer keeps the device alive until the decoder has stopped; FFmpeg owns a COM reference.
unsafe impl Send for Hardware {}

impl Hardware {
    /// The backends to try, in turn, for this choice. On Linux that is moonlight-qt's and Moonlight
    /// V+'s order: the first-tier hwaccel (VAAPI), then the hardware decoders outside FFmpeg's
    /// hwaccels (V4L2 M2M), then the second tier (NVDEC); software decoding comes after them all.
    pub fn order(self) -> Vec<Self> {
        match self {
            Self::None => Vec::new(),
            Self::Vaapi => vec![Self::Vaapi, Self::V4l2m2m, Self::Cuda],
            other => vec![other],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "software",
            Self::D3d11(_) => "D3D11VA",
            Self::Vaapi => "VAAPI",
            Self::Cuda => "NVDEC",
            Self::V4l2m2m => "V4L2 M2M",
        }
    }
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
    /// D3D11VA: make the decoder's textures readable by shaders (`stream::direct`).
    #[cfg(windows)]
    bind_textures: bool,
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
    /// VAAPI pictures go to the presenter as DMA-BUFs until the flag says the presenter cannot
    /// import them (`export_dmabuf`).
    #[cfg(unix)]
    dmabuf: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// The size of the pooled packet buffers for a packet of `needed` bytes with its padding: a
/// quarter more, in whole 64 KiB, so a stream whose frames grow slowly does not remake the pool
/// for each one.
fn packet_buffer_size(needed: usize) -> usize {
    (needed + needed / 4).next_multiple_of(64 * 1024)
}

// An AVCodecContext moves between threads but is only accessed by its decoder thread.
unsafe impl Send for VideoDecoder {}

pub enum Received {
    Frame(Frame),
    None,
    Error,
}

impl VideoDecoder {
    /// Opens the first backend of `hardware.order()` that opens, logging each that does not
    /// (`failed`); the error names every one.
    pub fn open_first(
        codec: Codec,
        hardware: Hardware,
        require_hardware: bool,
        failed: impl FnMut(Hardware, &str),
    ) -> Result<Self, String> {
        first_opened(hardware.order(), |backend| Self::open(codec, backend, 1, require_hardware), failed)
    }

    pub fn open(
        codec: Codec,
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
                    #[cfg(windows)]
                    bind_textures: false,
                }),
                name: CStr::from_ptr((*found).name).to_string_lossy().into_owned(),
                hardware,
                #[cfg(unix)]
                dmabuf: None,
            };
            if video.context.is_null() || video.packet.is_null() {
                return Err("Could not allocate the video decoder.".into());
            }
            let context = &mut *video.context;
            context.opaque = (&mut *video._format as *mut Format).cast();
            let device_type = match hardware {
                Hardware::D3d11(_) => Some(AV_HWDEVICE_TYPE_D3D11VA),
                Hardware::Vaapi => Some(AV_HWDEVICE_TYPE_VAAPI),
                Hardware::Cuda => Some(AV_HWDEVICE_TYPE_CUDA),
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
                    Hardware::None | Hardware::V4l2m2m => unreachable!(),
                }
                context.get_format = Some(select_format);
                context.extra_hw_frames = 6;
            }
            context.thread_count = if hardware == Hardware::None {
                threads
            } else {
                1
            };
            context.thread_type = FF_THREAD_SLICE as i32;
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
                Hardware::Cuda => " nvdec",
                Hardware::None | Hardware::V4l2m2m => "",
            });
            Ok(video)
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

    pub fn receive(&mut self) -> Received {
        unsafe {
            let Some(raw) = NonNull::new(av_frame_alloc()) else {
                return Received::Error;
            };
            #[allow(unused_mut)]
            let mut frame = Frame::new(raw);
            let status = avcodec_receive_frame(self.context, raw.as_ptr());
            if status == -11 || status == -541478725 {
                return Received::None;
            }
            if status < 0 {
                return Received::Error;
            }
            let format = (*raw.as_ptr()).format;
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

    pub fn copy_to_d3d11(&self, texture: *mut c_void, width: i32, height: i32) -> bool {
        #[cfg(windows)]
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
        #[cfg(not(windows))]
        {
            let _ = (texture, width, height);
            false
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
            assert_eq!((info.kind, info.ten_bit, info.width, info.height), (FRAME_YUV444P, 0, 64, 64), "{codec:?}");
            // The fixtures are lossless: every plane is the full-size source plane.
            for plane in 0..3 {
                for y in 0..64 {
                    // SAFETY: a 64x64 4:4:4 picture's planes hold 64 bytes per row.
                    let row = unsafe { std::slice::from_raw_parts(info.data[plane].add(y * info.linesize[plane] as usize), 64) };
                    assert_eq!(row, &source[(plane * 64 + y) * 64..][..64], "{codec:?} plane {plane} row {y}");
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
    fn linux_tries_vaapi_then_v4l2_m2m_then_nvdec_and_other_choices_alone() {
        assert_eq!(Hardware::Vaapi.order(), [Hardware::Vaapi, Hardware::V4l2m2m, Hardware::Cuda]);
        assert!(Hardware::None.order().is_empty());
        let device = ptr::dangling_mut::<c_void>();
        assert_eq!(Hardware::D3d11(device).order(), [Hardware::D3d11(device)]);
        assert_eq!(Hardware::Cuda.order(), [Hardware::Cuda]);
    }

    #[test]
    fn the_first_backend_that_opens_is_used_and_each_one_before_it_is_logged_once() {
        let mut logged = Vec::new();
        let mut tried = Vec::new();
        let opened = first_opened(
            Hardware::Vaapi.order(),
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
        let failed: Result<(), String> = first_opened(Hardware::Vaapi.order(), |_| Err("gone".into()), |backend, _| logged.push(backend.label().into()));
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
