//! Video decoding through libavcodec (`ffmpeg_shim.c`) and Opus decoding through
//! libopus, the decoders Rubylight's C test client uses. Without the `ffmpeg` feature the
//! decoders fail to open.

/// Why a frame produced no picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]
pub enum Received {
    Frame,
    /// EAGAIN or EOF.
    Empty,
    /// A hardware surface could not be read back.
    Readback,
    Error(i32),
}

/// What opening the decoder reported.
#[derive(Debug, Default)]
pub struct OpenInfo {
    pub name: String,
    pub hardware_format: String,
    pub active_thread_type: i32,
    pub delay: i32,
}

#[cfg(feature = "ffmpeg")]
mod ffi {
    use std::ffi::{c_char, c_int, c_uint, c_void};

    extern "C" {
        #[allow(clippy::too_many_arguments)]
        pub fn nj_decoder_open(
            codec: c_int,
            threads: c_int,
            hardware: *const c_char,
            device: *const c_char,
            name: *mut c_char,
            name_length: c_int,
            hardware_name: *mut c_char,
            hardware_length: c_int,
            active_thread_type: *mut c_int,
            delay: *mut c_int,
            error: *mut c_int,
        ) -> *mut c_void;
        pub fn nj_decoder_close(decoder: *mut c_void);
        pub fn nj_decoder_send(
            decoder: *mut c_void,
            data: *const u8,
            length: c_int,
            pts: i64,
        ) -> c_int;
        pub fn nj_decoder_receive(decoder: *mut c_void) -> c_int;
        pub fn nj_frame_width(decoder: *const c_void) -> c_int;
        pub fn nj_frame_height(decoder: *const c_void) -> c_int;
        pub fn nj_frame_format(decoder: *const c_void) -> c_int;
        pub fn nj_frame_primaries(decoder: *const c_void) -> c_int;
        pub fn nj_frame_transfer(decoder: *const c_void) -> c_int;
        pub fn nj_frame_pts(decoder: *const c_void) -> i64;
        pub fn nj_frame_crop(decoder: *mut c_void, width: c_int, height: c_int);
        pub fn nj_frame_depth(decoder: *const c_void) -> c_int;
        pub fn nj_frame_format_name(decoder: *const c_void) -> *const c_char;
        pub fn nj_frame_luma(decoder: *const c_void, x: c_int, y: c_int) -> c_uint;
        pub fn nj_frame_dump(decoder: *const c_void, path: *const c_char) -> c_int;

        pub fn opus_multistream_decoder_create(
            rate: i32,
            channels: c_int,
            streams: c_int,
            coupled: c_int,
            mapping: *const u8,
            error: *mut c_int,
        ) -> *mut c_void;
        pub fn opus_multistream_decode_float(
            decoder: *mut c_void,
            data: *const u8,
            length: i32,
            pcm: *mut f32,
            frame_size: c_int,
            decode_fec: c_int,
        ) -> c_int;
        pub fn opus_multistream_decoder_destroy(decoder: *mut c_void);
    }
}

#[cfg(feature = "ffmpeg")]
mod imp {
    use super::{ffi, OpenInfo, Received};
    use std::ffi::{c_void, CStr, CString};

    fn text(buffer: &[u8]) -> String {
        CStr::from_bytes_until_nul(buffer)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub struct VideoDecoder(*mut c_void);

    // The decoder is used from one thread at a time.
    unsafe impl Send for VideoDecoder {}

    impl VideoDecoder {
        /// codec: 0 H.264, 1 HEVC, 2 AV1.
        pub fn open(
            codec: i32,
            threads: i32,
            hardware: Option<&str>,
            device: Option<&str>,
        ) -> (Option<Self>, OpenInfo, i32) {
            let hardware = hardware.and_then(|h| CString::new(h).ok());
            let device = device.and_then(|d| CString::new(d).ok());
            let mut name = [0u8; 64];
            let mut hardware_name = [0u8; 64];
            let (mut active, mut delay, mut error) = (0, 0, 0);
            // SAFETY: valid pointers and lengths for the duration of the call.
            let decoder = unsafe {
                ffi::nj_decoder_open(
                    codec,
                    threads,
                    hardware.as_ref().map_or(std::ptr::null(), |h| h.as_ptr()),
                    device.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
                    name.as_mut_ptr().cast(),
                    name.len() as i32,
                    hardware_name.as_mut_ptr().cast(),
                    hardware_name.len() as i32,
                    &mut active,
                    &mut delay,
                    &mut error,
                )
            };
            let info = OpenInfo {
                name: text(&name),
                hardware_format: text(&hardware_name),
                active_thread_type: active,
                delay,
            };
            ((!decoder.is_null()).then_some(Self(decoder)), info, error)
        }

        pub fn send(&mut self, data: &[u8], pts: i64) -> i32 {
            // SAFETY: the decoder is open; data is valid for its length.
            unsafe { ffi::nj_decoder_send(self.0, data.as_ptr(), data.len() as i32, pts) }
        }

        pub fn receive(&mut self) -> Received {
            // SAFETY: the decoder is open.
            match unsafe { ffi::nj_decoder_receive(self.0) } {
                0 => Received::Frame,
                1 => Received::Empty,
                2 => Received::Readback,
                error => Received::Error(error),
            }
        }

        pub fn width(&self) -> i32 {
            unsafe { ffi::nj_frame_width(self.0) }
        }

        pub fn height(&self) -> i32 {
            unsafe { ffi::nj_frame_height(self.0) }
        }

        pub fn format(&self) -> i32 {
            unsafe { ffi::nj_frame_format(self.0) }
        }

        pub fn primaries(&self) -> i32 {
            unsafe { ffi::nj_frame_primaries(self.0) }
        }

        pub fn transfer(&self) -> i32 {
            unsafe { ffi::nj_frame_transfer(self.0) }
        }

        pub fn pts(&self) -> i64 {
            unsafe { ffi::nj_frame_pts(self.0) }
        }

        pub fn depth(&self) -> i32 {
            unsafe { ffi::nj_frame_depth(self.0) }
        }

        pub fn crop(&mut self, width: i32, height: i32) {
            unsafe { ffi::nj_frame_crop(self.0, width, height) }
        }

        pub fn format_name(&self) -> String {
            // SAFETY: the shim returns a static string.
            unsafe { CStr::from_ptr(ffi::nj_frame_format_name(self.0)) }
                .to_string_lossy()
                .into_owned()
        }

        pub fn luma(&self, x: i32, y: i32) -> u32 {
            unsafe { ffi::nj_frame_luma(self.0, x, y) }
        }

        pub fn dump(&self, path: &str) -> bool {
            let Ok(path) = CString::new(path) else {
                return false;
            };
            unsafe { ffi::nj_frame_dump(self.0, path.as_ptr()) == 0 }
        }
    }

    impl Drop for VideoDecoder {
        fn drop(&mut self) {
            unsafe { ffi::nj_decoder_close(self.0) }
        }
    }

    pub struct OpusDecoder(*mut c_void);

    unsafe impl Send for OpusDecoder {}

    impl OpusDecoder {
        pub fn new(
            channels: i32,
            streams: i32,
            coupled: i32,
            mapping: &[u8; 12],
        ) -> Result<Self, i32> {
            let mut error = 0;
            // SAFETY: mapping holds at least `channels` (at most 12) entries.
            let decoder = unsafe {
                ffi::opus_multistream_decoder_create(
                    48000,
                    channels,
                    streams,
                    coupled,
                    mapping.as_ptr(),
                    &mut error,
                )
            };
            if decoder.is_null() || error != 0 {
                return Err(if error != 0 { error } else { -1 });
            }
            Ok(Self(decoder))
        }

        /// Decodes a packet, or conceals a lost one with `None`; returns samples per channel
        /// or a negative error.
        pub fn decode(&mut self, packet: Option<&[u8]>, pcm: &mut [f32], frame_size: i32) -> i32 {
            let (data, length) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len()));
            // SAFETY: pcm holds frame_size samples for every channel (checked by the caller).
            unsafe {
                ffi::opus_multistream_decode_float(
                    self.0,
                    data,
                    length as i32,
                    pcm.as_mut_ptr(),
                    frame_size,
                    0,
                )
            }
        }
    }

    impl Drop for OpusDecoder {
        fn drop(&mut self) {
            unsafe { ffi::opus_multistream_decoder_destroy(self.0) }
        }
    }
}

#[cfg(not(feature = "ffmpeg"))]
mod imp {
    use super::{OpenInfo, Received};

    pub struct VideoDecoder;

    impl VideoDecoder {
        pub fn open(
            _codec: i32,
            _threads: i32,
            _hardware: Option<&str>,
            _device: Option<&str>,
        ) -> (Option<Self>, OpenInfo, i32) {
            eprintln!("nightjar-cli was built without the ffmpeg feature");
            (None, OpenInfo::default(), -1)
        }

        pub fn send(&mut self, _data: &[u8], _pts: i64) -> i32 {
            -1
        }

        pub fn receive(&mut self) -> Received {
            Received::Empty
        }

        pub fn width(&self) -> i32 {
            0
        }

        pub fn height(&self) -> i32 {
            0
        }

        pub fn format(&self) -> i32 {
            -1
        }

        pub fn primaries(&self) -> i32 {
            0
        }

        pub fn transfer(&self) -> i32 {
            0
        }

        pub fn pts(&self) -> i64 {
            0
        }

        pub fn depth(&self) -> i32 {
            0
        }

        pub fn crop(&mut self, _width: i32, _height: i32) {}

        pub fn format_name(&self) -> String {
            "unknown".into()
        }

        pub fn luma(&self, _x: i32, _y: i32) -> u32 {
            0
        }

        pub fn dump(&self, _path: &str) -> bool {
            false
        }
    }

    pub struct OpusDecoder;

    impl OpusDecoder {
        pub fn new(
            _channels: i32,
            _streams: i32,
            _coupled: i32,
            _mapping: &[u8; 12],
        ) -> Result<Self, i32> {
            Err(-1)
        }

        pub fn decode(
            &mut self,
            _packet: Option<&[u8]>,
            _pcm: &mut [f32],
            _frame_size: i32,
        ) -> i32 {
            -1
        }
    }
}

pub use imp::{OpusDecoder, VideoDecoder};
