//! libopus: the multistream decoder for game audio and the encoder for the microphone, with
//! the settings `callbacks.c` and `android_microphone.c` use. The app's prebuilt libopus is
//! linked on Android only (build.rs); host builds get no codec, so the unit tests cover the
//! code around it.

/// A multistream decoder (`OpusMSDecoder`).
pub struct Decoder {
    #[cfg(target_os = "android")]
    raw: *mut ffi::OpusMSDecoder,
}

// libopus state is plain memory, used by one thread at a time.
unsafe impl Send for Decoder {}

/// A mono VoIP encoder (`OpusEncoder`).
pub struct Encoder {
    #[cfg(target_os = "android")]
    raw: *mut ffi::OpusEncoder,
}

unsafe impl Send for Encoder {}

#[cfg(target_os = "android")]
mod ffi {
    use std::os::raw::c_int;

    pub enum OpusMSDecoder {}
    pub enum OpusEncoder {}

    pub const OPUS_APPLICATION_VOIP: c_int = 2048;
    pub const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
    pub const OPUS_SET_INBAND_FEC_REQUEST: c_int = 4012;
    pub const OPUS_SET_PACKET_LOSS_PERC_REQUEST: c_int = 4014;
    pub const OPUS_SET_SIGNAL_REQUEST: c_int = 4024;
    pub const OPUS_RESET_STATE: c_int = 4028;
    pub const OPUS_SIGNAL_VOICE: i32 = 3001;

    extern "C" {
        pub fn opus_multistream_decoder_create(
            fs: i32,
            channels: c_int,
            streams: c_int,
            coupled_streams: c_int,
            mapping: *const u8,
            error: *mut c_int,
        ) -> *mut OpusMSDecoder;
        pub fn opus_multistream_decode(
            st: *mut OpusMSDecoder,
            data: *const u8,
            len: i32,
            pcm: *mut i16,
            frame_size: c_int,
            decode_fec: c_int,
        ) -> c_int;
        pub fn opus_multistream_decoder_destroy(st: *mut OpusMSDecoder);
        pub fn opus_encoder_create(
            fs: i32,
            channels: c_int,
            application: c_int,
            error: *mut c_int,
        ) -> *mut OpusEncoder;
        pub fn opus_encode(
            st: *mut OpusEncoder,
            pcm: *const i16,
            frame_size: c_int,
            data: *mut u8,
            max_data_bytes: i32,
        ) -> i32;
        pub fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
        pub fn opus_encoder_destroy(st: *mut OpusEncoder);
    }
}

/// Microphone settings from `android_microphone.c`: speech at 32 kb/s with in-band FEC.
const MIC_BITRATE: i32 = 32_000;
const MIC_PACKET_LOSS_PERCENT: i32 = 10;

impl Decoder {
    /// `opus_multistream_decoder_create`; the error is libopus's code.
    #[cfg(target_os = "android")]
    pub fn new(
        sample_rate: u32,
        channels: u8,
        streams: u8,
        coupled: u8,
        mapping: &[u8; 12],
    ) -> Result<Self, i32> {
        let mut error = 0;
        let raw = unsafe {
            ffi::opus_multistream_decoder_create(
                sample_rate as i32,
                channels.into(),
                streams.into(),
                coupled.into(),
                mapping.as_ptr(),
                &mut error,
            )
        };
        if raw.is_null() {
            Err(if error != 0 { error } else { -1 })
        } else {
            Ok(Self { raw })
        }
    }

    #[cfg(not(target_os = "android"))]
    pub fn new(_: u32, _: u8, _: u8, _: u8, _: &[u8; 12]) -> Result<Self, i32> {
        Err(-1)
    }

    /// Decodes one packet, or conceals a lost one (`None`), into `pcm`; returns the samples
    /// per channel, or a negative libopus error.
    #[cfg(target_os = "android")]
    pub fn decode(&mut self, packet: Option<&[u8]>, pcm: &mut [i16], frame_size: usize) -> i32 {
        let (data, len) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        unsafe {
            ffi::opus_multistream_decode(
                self.raw,
                data,
                len,
                pcm.as_mut_ptr(),
                frame_size as i32,
                0,
            )
        }
    }

    #[cfg(not(target_os = "android"))]
    pub fn decode(&mut self, _: Option<&[u8]>, _: &mut [i16], _: usize) -> i32 {
        -1
    }
}

#[cfg(target_os = "android")]
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { ffi::opus_multistream_decoder_destroy(self.raw) }
    }
}

impl Encoder {
    /// A 48 kHz mono encoder with the microphone settings.
    #[cfg(target_os = "android")]
    pub fn new_microphone() -> Result<Self, i32> {
        let mut error = 0;
        let raw =
            unsafe { ffi::opus_encoder_create(48_000, 1, ffi::OPUS_APPLICATION_VOIP, &mut error) };
        if raw.is_null() || error != 0 {
            if !raw.is_null() {
                unsafe { ffi::opus_encoder_destroy(raw) };
            }
            return Err(if error != 0 { error } else { -1 });
        }
        unsafe {
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_BITRATE_REQUEST, MIC_BITRATE);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_SIGNAL_REQUEST, ffi::OPUS_SIGNAL_VOICE);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_INBAND_FEC_REQUEST, 1i32);
            ffi::opus_encoder_ctl(
                raw,
                ffi::OPUS_SET_PACKET_LOSS_PERC_REQUEST,
                MIC_PACKET_LOSS_PERCENT,
            );
        }
        Ok(Self { raw })
    }

    #[cfg(not(target_os = "android"))]
    pub fn new_microphone() -> Result<Self, i32> {
        let _ = (MIC_BITRATE, MIC_PACKET_LOSS_PERCENT);
        Err(-1)
    }

    /// Starts afresh, as after a gap in the capture.
    #[cfg(target_os = "android")]
    pub fn reset(&mut self) {
        unsafe { ffi::opus_encoder_ctl(self.raw, ffi::OPUS_RESET_STATE) };
    }

    #[cfg(not(target_os = "android"))]
    pub fn reset(&mut self) {}

    /// Encodes one frame into `out`; returns the packet length or a libopus error.
    #[cfg(target_os = "android")]
    pub fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> i32 {
        unsafe {
            ffi::opus_encode(
                self.raw,
                pcm.as_ptr(),
                pcm.len() as i32,
                out.as_mut_ptr(),
                out.len() as i32,
            )
        }
    }

    #[cfg(not(target_os = "android"))]
    pub fn encode(&mut self, _: &[i16], _: &mut [u8]) -> i32 {
        -1
    }
}

#[cfg(target_os = "android")]
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { ffi::opus_encoder_destroy(self.raw) }
    }
}
