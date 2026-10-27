//! nightjar-core's decoder and audio renderer, forwarded to MoonBridge's `bridgeDr*` and
//! `bridgeAr*` callbacks as `callbacks.c` does (moonlight-common-c's Android binding, GPL-3.0,
//! Cameron Gutman and contributors): parameter sets are submitted on their own, picture data
//! in one buffer, and Opus is decoded here so Java receives PCM. Passthrough PCM goes on as
//! shorts the same way; AC3 and E-AC3 frames go to `bridgeArPlayEncoded` as bytes.

use jni::objects::{GlobalRef, JShortArray};

use nightjar_core::session::{
    self, AudioRenderer, AudioSetup, DecodeStatus, VideoDecoder, VideoSetup,
};
use nightjar_core::video::{BufferType, Frame};

use crate::bridge::{self, byte_array, Bridge};
use crate::extras::audio_configuration_to_int;
use crate::{opus, platform};

const DR_OK: i32 = 0;
const BUFFER_TYPE_PICDATA: i32 = 0;

pub struct Video {
    bridge: &'static Bridge,
    buffer: Option<GlobalRef>,
    capacity: usize,
}

impl Video {
    pub fn new(bridge: &'static Bridge) -> Self {
        Self {
            bridge,
            buffer: None,
            capacity: 0,
        }
    }
}

/// CLOCK_MONOTONIC nanoseconds of a time on the session's microsecond clock.
fn monotonic_ns_of(session_us: u64) -> i64 {
    let age_us = session::now_us().saturating_sub(session_us) as i64;
    platform::monotonic_ns() - age_us * 1000
}

impl VideoDecoder for Video {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        let b = self.bridge;
        let args = [
            bridge::int(setup.format as i32),
            bridge::int(setup.width),
            bridge::int(setup.height),
            bridge::int(setup.fps),
        ];
        let buffer = b.with_env(|env| {
            let packing = [
                bridge::int(setup.chroma_packing as i32),
                bridge::int(setup.logical_width),
                bridge::int(setup.logical_height),
                bridge::int(setup.color_space),
                bridge::int(setup.color_range),
            ];
            let result = b.int(env, b.m.dr_setup_packing, &packing).unwrap_or(-1);
            if result != 0 {
                return Ok(Some(Err(result)));
            }
            let result = b.int(env, b.m.dr_setup, &args).unwrap_or(-1);
            if result != 0 {
                return Ok(Some(Err(result)));
            }
            // 32 KiB to start; it grows with the largest frame.
            let array = env.new_byte_array(32768)?;
            Ok(Some(Ok(env.new_global_ref(array)?)))
        });
        match buffer {
            Some(Ok(buffer)) => {
                self.buffer = Some(buffer);
                self.capacity = 32768;
                Ok(())
            }
            Some(Err(code)) => Err(code),
            None => Err(-1),
        }
    }

    fn start(&mut self) {
        self.bridge.post(self.bridge.m.dr_start, &[]);
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        let b = self.bridge;
        let receive_ns = monotonic_ns_of(frame.receive_time_us);
        let needed = frame.data().len();
        let status = b.with_env(|env| {
            if needed > self.capacity || self.buffer.is_none() {
                let array = env.new_byte_array(needed as i32)?;
                self.buffer = Some(env.new_global_ref(array)?);
                self.capacity = needed;
            }
            let buffer = self.buffer.as_ref().expect("allocated above");
            let array = byte_array(buffer);
            let submit = |env: &mut jni::JNIEnv, length: usize, kind: i32| {
                let args = [
                    bridge::object(buffer.as_obj()),
                    bridge::int(length as i32),
                    bridge::int(kind),
                    bridge::int(frame.number as i32),
                    bridge::int(frame.frame_type as i32),
                    jni::objects::JValue::Char(frame.host_processing_latency).as_jni(),
                    bridge::long(frame.receive_time_us as i64),
                    bridge::long(frame.enqueue_time_us as i64),
                    bridge::long(receive_ns),
                    bridge::long(frame.presentation_time_us as i64),
                ];
                b.int(env, b.m.dr_submit, &args).unwrap_or(DR_OK)
            };
            let mut offset = 0;
            for (kind, bytes) in frame.buffer_slices() {
                if kind == BufferType::PicData {
                    env.set_byte_array_region(array, offset as i32, bridge::jbytes(bytes))?;
                    offset += bytes.len();
                } else {
                    // Parameter sets go to the decoder on their own, from the buffer's start.
                    env.set_byte_array_region(array, 0, bridge::jbytes(bytes))?;
                    let result = submit(env, bytes.len(), kind as i32);
                    if result != DR_OK {
                        return Ok(result);
                    }
                }
            }
            Ok(submit(env, offset, BUFFER_TYPE_PICDATA))
        });
        if status == DR_OK {
            DecodeStatus::Ok
        } else {
            DecodeStatus::NeedIdr
        }
    }

    fn stop(&mut self) {
        self.bridge.post(self.bridge.m.dr_stop, &[]);
    }

    fn cleanup(&mut self) {
        self.buffer = None;
        self.bridge.post(self.bridge.m.dr_cleanup, &[]);
    }
}

/// What the stream's packets are, for [`Audio`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioMode {
    Opus,
    /// s16le samples, handed on as Java shorts like decoded Opus.
    Pcm,
    /// Whole AC3 or E-AC3 frames, handed on as bytes to a passthrough AudioTrack.
    Encoded,
}

/// The largest AC3 or E-AC3 frame: E-AC3's 11-bit `frmsiz` counts up to 2048 16-bit words.
const MAX_ENCODED_FRAME: usize = 4096;

pub struct Audio {
    bridge: &'static Bridge,
    decoder: Option<opus::Decoder>,
    mode: AudioMode,
    /// The passthrough codecs this stream asked for, to say so when the stream plays Opus.
    passthrough: u8,
    pcm: Vec<i16>,
    channels: usize,
    samples_per_frame: usize,
    array: Option<GlobalRef>,
    /// An AC3 or E-AC3 frame over [`MAX_ENCODED_FRAME`] was dropped and logged.
    oversized_logged: bool,
}

impl Audio {
    pub fn new(bridge: &'static Bridge, passthrough: u8) -> Self {
        Self {
            bridge,
            decoder: None,
            mode: AudioMode::Opus,
            passthrough,
            pcm: Vec::new(),
            channels: 0,
            samples_per_frame: 0,
            array: None,
            oversized_logged: false,
        }
    }
}

/// The AC3 or E-AC3 frame to play: `None` for a lost frame, or one over [`MAX_ENCODED_FRAME`],
/// which no valid frame is. The first such frame of a stream is logged.
fn playable_frame<'a>(packet: Option<&'a [u8]>, logged: &mut bool) -> Option<&'a [u8]> {
    let frame = packet?;
    if frame.len() <= MAX_ENCODED_FRAME {
        return Some(frame);
    }
    if !*logged {
        *logged = true;
        platform::warn(&format!(
            "Dropped a {}-byte passthrough audio frame, over the {MAX_ENCODED_FRAME}-byte maximum; later ones are dropped unlogged",
            frame.len()
        ));
    }
    None
}

/// A PCM packet (s16le, interleaved) into `pcm`, or silence for a lost one. Returns the samples
/// written: the packet's, capped to `pcm`, or all of `pcm` for a lost packet.
pub fn pcm_s16le(packet: Option<&[u8]>, pcm: &mut [i16]) -> usize {
    match packet {
        Some(bytes) => {
            let samples = (bytes.len() / 2).min(pcm.len());
            for (out, pair) in pcm.iter_mut().zip(bytes.chunks_exact(2)) {
                *out = i16::from_le_bytes([pair[0], pair[1]]);
            }
            samples
        }
        None => {
            pcm.fill(0);
            pcm.len()
        }
    }
}

impl AudioRenderer for Audio {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_OPUS, CODEC_PCM};
        let b = self.bridge;
        let configuration = bridge::int(audio_configuration_to_int(setup.configuration));
        self.mode = match setup.codec {
            CODEC_OPUS => AudioMode::Opus,
            CODEC_PCM => AudioMode::Pcm,
            CODEC_AC3 | CODEC_EAC3 => AudioMode::Encoded,
            other => {
                platform::error(&format!(
                    "The stream's audio codec {other} is not one this client plays"
                ));
                return Err(-1);
            }
        };
        if self.mode == AudioMode::Opus && self.passthrough != 0 {
            let message = "The PC offered no passthrough format this audio output plays. This stream plays Opus.";
            platform::warn(message);
            b.with_env(|env| {
                let message = env.new_string(message)?;
                b.void(
                    env,
                    b.m.display_message,
                    &[bridge::object(message.as_ref())],
                );
                Ok(())
            });
        }
        if self.mode == AudioMode::Encoded {
            // The Java renderer opens an AC3 or E-AC3 AudioTrack, or refuses and says why.
            let args = [configuration, bridge::int(i32::from(setup.codec))];
            let result = b.with_env(|env| Ok(b.int(env, b.m.ar_init_encoded, &args).unwrap_or(-1)));
            if result != 0 {
                return Err(result);
            }
            let array = b.with_env(|env| {
                let array = env.new_byte_array(MAX_ENCODED_FRAME as i32)?;
                Ok(Some(env.new_global_ref(array)?))
            });
            if array.is_none() {
                b.post(b.m.ar_cleanup, &[]);
                return Err(-1);
            }
            self.array = array;
            return Ok(());
        }
        let opus_config = setup.opus;
        let args = [
            configuration,
            bridge::int(opus_config.sample_rate as i32),
            bridge::int(setup.samples_per_frame as i32),
        ];
        let result = b.with_env(|env| Ok(b.int(env, b.m.ar_init, &args).unwrap_or(-1)));
        if result != 0 {
            return Err(result);
        }
        self.channels = usize::from(setup.configuration.channel_count);
        if self.mode == AudioMode::Opus {
            let decoder = match opus::Decoder::new(
                opus_config.sample_rate,
                opus_config.channel_count,
                opus_config.streams,
                opus_config.coupled_streams,
                &opus_config.mapping,
            ) {
                Ok(decoder) => decoder,
                Err(code) => {
                    platform::error(&format!("Opus decoder creation failed: {code}"));
                    b.post(b.m.ar_cleanup, &[]);
                    return Err(-1);
                }
            };
            self.channels = usize::from(opus_config.channel_count);
            self.decoder = Some(decoder);
        }
        self.samples_per_frame = setup.samples_per_frame as usize;
        self.pcm = vec![0; self.channels * self.samples_per_frame];
        let length = self.pcm.len() as i32;
        let array = b.with_env(|env| {
            let array = env.new_short_array(length)?;
            Ok(Some(env.new_global_ref(array)?))
        });
        if array.is_none() {
            b.post(b.m.ar_cleanup, &[]);
            return Err(-1);
        }
        self.array = array;
        Ok(())
    }

    fn start(&mut self) {
        self.bridge.post(self.bridge.m.ar_start, &[]);
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        let Some(array) = self.array.as_ref() else {
            return;
        };
        let b = self.bridge;
        let samples = match self.mode {
            AudioMode::Encoded => {
                // A lost frame writes nothing: the sink hears a gap, never a broken frame.
                let Some(frame) = playable_frame(packet, &mut self.oversized_logged) else {
                    return;
                };
                b.with_env(|env| {
                    env.set_byte_array_region(byte_array(array), 0, bridge::jbytes(frame))?;
                    b.void(
                        env,
                        b.m.ar_play_encoded,
                        &[
                            bridge::object(array.as_obj()),
                            bridge::int(frame.len() as i32),
                        ],
                    );
                    Ok(())
                });
                return;
            }
            AudioMode::Pcm => pcm_s16le(packet, &mut self.pcm),
            AudioMode::Opus => {
                let Some(decoder) = self.decoder.as_mut() else {
                    return;
                };
                let decoded = decoder.decode(packet, &mut self.pcm, self.samples_per_frame);
                if decoded <= 0 {
                    return;
                }
                (decoded as usize * self.channels).min(self.pcm.len())
            }
        };
        let pcm = &self.pcm[..samples];
        b.with_env(|env| {
            let shorts = <&JShortArray>::from(array.as_obj());
            env.set_short_array_region(shorts, 0, pcm)?;
            b.void(
                env,
                b.m.ar_play,
                &[bridge::object(array.as_obj()), bridge::int(samples as i32)],
            );
            Ok(())
        });
    }

    fn stop(&mut self) {
        self.bridge.post(self.bridge.m.ar_stop, &[]);
    }

    fn cleanup(&mut self) {
        self.decoder = None;
        self.array = None;
        self.bridge.post(self.bridge.m.ar_cleanup, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_packets_become_samples_and_lost_ones_silence() {
        let mut pcm = [7i16; 4];
        assert_eq!(
            pcm_s16le(Some(&[0x01, 0x00, 0xFF, 0xFF, 0x00, 0x80]), &mut pcm),
            3
        );
        assert_eq!(pcm[..3], [1, -1, i16::MIN]);
        assert_eq!(pcm_s16le(None, &mut pcm), 4);
        assert_eq!(pcm, [0; 4]);
        // A packet longer than the buffer is cut to it, never written past it.
        assert_eq!(pcm_s16le(Some(&[1; 12]), &mut pcm), 4);
        assert_eq!(pcm, [0x0101; 4]);
    }

    #[test]
    fn oversized_passthrough_frames_are_dropped_and_logged_once() {
        let mut logged = false;
        let frame = [0u8; MAX_ENCODED_FRAME];
        assert_eq!(playable_frame(Some(&frame), &mut logged), Some(&frame[..]));
        assert_eq!(playable_frame(None, &mut logged), None);
        assert!(!logged);
        let oversized = [0u8; MAX_ENCODED_FRAME + 1];
        assert_eq!(playable_frame(Some(&oversized), &mut logged), None);
        assert!(logged);
        assert_eq!(playable_frame(Some(&oversized), &mut logged), None);
        assert_eq!(playable_frame(Some(&frame), &mut logged), Some(&frame[..]));
    }
}
