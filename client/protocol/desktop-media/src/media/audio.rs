//! Opus multistream audio decoding and interleaved s16le PCM conversion.

use std::ffi::{c_int, c_uchar, c_void};
use std::ptr::{self, NonNull};

unsafe extern "C" {
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
