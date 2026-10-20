//! FFmpeg video decoding and Opus audio decoding through `media_shim.c`.

use std::ffi::{c_char, c_int, c_uchar, c_void, CStr};
use std::ptr::NonNull;

#[repr(C)]
struct RawVideo {
    _private: [u8; 0],
}

#[repr(C)]
pub struct RawFrame {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrameInfo {
    pub width: c_int,
    pub height: c_int,
    pub kind: c_int,
    pub ten_bit: c_int,
    pub pts: i64,
    pub data: [*mut u8; 3],
    pub linesize: [c_int; 3],
    pub texture: *mut c_void,
    pub texture_index: c_int,
    pub color_space: c_int,
    pub color_range: c_int,
    pub color_trc: c_int,
    pub color_primaries: c_int,
}

pub const FRAME_NV12: c_int = 0;
pub const FRAME_P010: c_int = 1;
pub const FRAME_YUV420P: c_int = 2;
pub const FRAME_D3D11: c_int = 3;

extern "C" {
    fn nj_video_open(
        codec: c_int,
        hardware: c_int,
        device: *mut c_void,
        threads: c_int,
        require_hardware: c_int,
        name: *mut c_char,
        name_length: c_int,
        error: *mut c_char,
        error_length: c_int,
    ) -> *mut RawVideo;
    fn nj_video_close(video: *mut RawVideo);
    fn nj_video_send(video: *mut RawVideo, data: *const u8, length: c_int, pts: i64) -> c_int;
    fn nj_video_receive(video: *mut RawVideo, status: *mut c_int) -> *mut RawFrame;
    fn nj_frame_free(frame: *mut RawFrame);
    fn nj_frame_describe(frame: *const RawFrame, info: *mut FrameInfo);
    fn nj_pixel_format_name(frame: *const RawFrame) -> *const c_char;
    fn nj_frame_copy_d3d11(
        frame: *const RawFrame,
        destination: *mut c_void,
        width: c_int,
        height: c_int,
    ) -> c_int;
    #[cfg(windows)]
    fn nj_swapchain_allows_tearing(swapchain: *mut c_void) -> c_int;
    fn nj_opus_open(
        rate: c_int,
        channels: c_int,
        streams: c_int,
        coupled: c_int,
        mapping: *const c_uchar,
        error: *mut c_int,
    ) -> *mut c_void;
    fn nj_opus_decode(
        decoder: *mut c_void,
        data: *const c_uchar,
        length: c_int,
        pcm: *mut f32,
        frame_size: c_int,
    ) -> c_int;
    fn nj_opus_close(decoder: *mut c_void);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264 = 0,
    Hevc = 1,
    Av1 = 2,
}

/// Where decoding happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    None,
    /// The renderer's ID3D11Device, shared with the decoder.
    D3d11(*mut c_void),
    Vaapi,
}

// SAFETY: the device pointer is a COM object FFmpeg AddRefs and uses under its own lock.
unsafe impl Send for Hardware {}

fn text(buffer: &[c_char]) -> String {
    // SAFETY: the shim always NUL-terminates within the buffer.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

pub struct VideoDecoder {
    raw: NonNull<RawVideo>,
    pub name: String,
}

// SAFETY: an AVCodecContext may move between threads; it is used by one thread at a time.
unsafe impl Send for VideoDecoder {}

pub enum Received {
    Frame(Frame),
    None,
    Error,
}

impl VideoDecoder {
    pub fn open(
        codec: Codec,
        hardware: Hardware,
        threads: i32,
        require_hardware: bool,
    ) -> Result<Self, String> {
        let (kind, device) = match hardware {
            Hardware::None => (0, std::ptr::null_mut()),
            Hardware::D3d11(device) => (1, device),
            Hardware::Vaapi => (2, std::ptr::null_mut()),
        };
        let mut name = [0 as c_char; 96];
        let mut error = [0 as c_char; 256];
        // SAFETY: valid buffers with their lengths.
        let raw = unsafe {
            nj_video_open(
                codec as c_int,
                kind,
                device,
                threads,
                i32::from(require_hardware),
                name.as_mut_ptr(),
                name.len() as c_int,
                error.as_mut_ptr(),
                error.len() as c_int,
            )
        };
        match NonNull::new(raw) {
            Some(raw) => Ok(Self {
                raw,
                name: text(&name),
            }),
            None => Err(text(&error)),
        }
    }

    pub fn send(&mut self, data: &[u8], pts: i64) -> i32 {
        // SAFETY: the decoder is open; data is valid for its length.
        unsafe { nj_video_send(self.raw.as_ptr(), data.as_ptr(), data.len() as c_int, pts) }
    }

    pub fn receive(&mut self) -> Received {
        let mut status = 0;
        // SAFETY: the decoder is open.
        let frame = unsafe { nj_video_receive(self.raw.as_ptr(), &mut status) };
        match NonNull::new(frame) {
            Some(raw) => Received::Frame(Frame { raw }),
            None if status == 1 => Received::None,
            None => Received::Error,
        }
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        // SAFETY: opened by nj_video_open, closed once.
        unsafe { nj_video_close(self.raw.as_ptr()) }
    }
}

/// A decoded picture (an AVFrame reference).
pub struct Frame {
    pub(crate) raw: NonNull<RawFrame>,
}

// SAFETY: AVFrames are reference counted and safe to hand to another thread.
unsafe impl Send for Frame {}

impl Frame {
    pub fn info(&self) -> FrameInfo {
        let mut info = std::mem::MaybeUninit::<FrameInfo>::uninit();
        // SAFETY: the frame is valid and the shim fills every field.
        unsafe {
            nj_frame_describe(self.raw.as_ptr(), info.as_mut_ptr());
            info.assume_init()
        }
    }

    pub fn format_name(&self) -> String {
        // SAFETY: the shim returns a static string.
        unsafe { CStr::from_ptr(nj_pixel_format_name(self.raw.as_ptr())) }
            .to_string_lossy()
            .into_owned()
    }

    /// Copies a D3D11 surface into `texture` (same device, NV12 or P010).
    pub fn copy_to_d3d11(&self, texture: *mut c_void, width: i32, height: i32) -> bool {
        // SAFETY: the frame is valid; the texture belongs to the decoder's device.
        unsafe { nj_frame_copy_d3d11(self.raw.as_ptr(), texture, width, height) == 0 }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: owned reference, freed once.
        unsafe { nj_frame_free(self.raw.as_ptr()) }
    }
}

#[cfg(windows)]
pub fn swapchain_allows_tearing(swapchain: *mut c_void) -> bool {
    // SAFETY: a DXGI swap chain from SDL's renderer properties, or null.
    unsafe { nj_swapchain_allows_tearing(swapchain) != 0 }
}

pub struct OpusDecoder {
    raw: NonNull<c_void>,
    pub channels: usize,
}

// SAFETY: used by one thread at a time.
unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    pub fn new(channels: u8, streams: u8, coupled: u8, mapping: &[u8]) -> Result<Self, i32> {
        let mut error = 0;
        let mut map = [0u8; 8];
        let n = mapping.len().min(8);
        map[..n].copy_from_slice(&mapping[..n]);
        // SAFETY: the mapping has at least `channels` entries.
        let raw = unsafe {
            nj_opus_open(
                48000,
                c_int::from(channels),
                c_int::from(streams),
                c_int::from(coupled),
                map.as_ptr(),
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

    /// Decodes a packet (None conceals a lost one) into `pcm`; samples per channel.
    pub fn decode(&mut self, packet: Option<&[u8]>, pcm: &mut [f32]) -> i32 {
        let frame_size = (pcm.len() / self.channels.max(1)) as c_int;
        let (data, length) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len()));
        // SAFETY: pcm holds frame_size samples for every channel.
        unsafe {
            nj_opus_decode(
                self.raw.as_ptr(),
                data,
                length as c_int,
                pcm.as_mut_ptr(),
                frame_size,
            )
        }
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        // SAFETY: created by nj_opus_open, destroyed once.
        unsafe { nj_opus_close(self.raw.as_ptr()) }
    }
}
