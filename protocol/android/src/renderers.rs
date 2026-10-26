//! nightjar-core's decoder and audio renderer, forwarded to MoonBridge's `bridgeDr*` and
//! `bridgeAr*` callbacks as `callbacks.c` does (moonlight-common-c's Android binding, GPL-3.0,
//! Cameron Gutman and contributors): parameter sets are submitted on their own, picture data
//! in one buffer, and Opus is decoded here so Java receives PCM.

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

/// The frame's picture data as one slice, when its picture buffers are adjacent in the frame.
fn contiguous_picture(frame: &Frame) -> Option<&[u8]> {
    let mut range: Option<std::ops::Range<usize>> = None;
    for buffer in frame.buffers() {
        if buffer.kind != BufferType::PicData {
            continue;
        }
        range = match range {
            None => Some(buffer.range.clone()),
            Some(r) if r.end == buffer.range.start => Some(r.start..buffer.range.end),
            Some(_) => return None,
        };
    }
    range.filter(|r| !r.is_empty()).map(|r| &frame.data()[r])
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
            // Picture data that lies in one piece goes to Java in place, as a direct buffer that
            // is only valid during the call: the renderer copies it into the codec (or PyroWave)
            // input without a Java array between.
            let picture = contiguous_picture(frame);
            let mut offset = 0;
            for (kind, bytes) in frame.buffer_slices() {
                if kind == BufferType::PicData {
                    if picture.is_none() {
                        env.set_byte_array_region(array, offset as i32, bridge::jbytes(bytes))?;
                        offset += bytes.len();
                    }
                } else {
                    // Parameter sets go to the decoder on their own, from the buffer's start.
                    env.set_byte_array_region(array, 0, bridge::jbytes(bytes))?;
                    let result = submit(env, bytes.len(), kind as i32);
                    if result != DR_OK {
                        return Ok(result);
                    }
                }
            }
            let Some(picture) = picture else {
                return Ok(submit(env, offset, BUFFER_TYPE_PICDATA));
            };
            // SAFETY: Java only reads the buffer, and only until the call returns; `frame`
            // outlives it.
            let direct = unsafe {
                env.new_direct_byte_buffer(picture.as_ptr().cast_mut(), picture.len())?
            };
            let args = [
                bridge::object(&direct),
                bridge::int(picture.len() as i32),
                bridge::int(BUFFER_TYPE_PICDATA),
                bridge::int(frame.number as i32),
                bridge::int(frame.frame_type as i32),
                jni::objects::JValue::Char(frame.host_processing_latency).as_jni(),
                bridge::long(frame.receive_time_us as i64),
                bridge::long(frame.enqueue_time_us as i64),
                bridge::long(receive_ns),
                bridge::long(frame.presentation_time_us as i64),
            ];
            Ok(b.int(env, b.m.dr_submit_direct, &args).unwrap_or(DR_OK))
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

pub struct Audio {
    bridge: &'static Bridge,
    decoder: Option<opus::Decoder>,
    pcm: Vec<i16>,
    channels: usize,
    samples_per_frame: usize,
    array: Option<GlobalRef>,
}

impl Audio {
    pub fn new(bridge: &'static Bridge) -> Self {
        Self {
            bridge,
            decoder: None,
            pcm: Vec::new(),
            channels: 0,
            samples_per_frame: 0,
            array: None,
        }
    }
}

impl AudioRenderer for Audio {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        let b = self.bridge;
        let opus_config = setup.opus;
        let args = [
            bridge::int(audio_configuration_to_int(setup.configuration)),
            bridge::int(opus_config.sample_rate as i32),
            bridge::int(setup.samples_per_frame as i32),
        ];
        let result = b.with_env(|env| Ok(b.int(env, b.m.ar_init, &args).unwrap_or(-1)));
        if result != 0 {
            return Err(result);
        }
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
        self.decoder = Some(decoder);
        self.array = array;
        Ok(())
    }

    fn start(&mut self) {
        self.bridge.post(self.bridge.m.ar_start, &[]);
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        let (Some(decoder), Some(array)) = (self.decoder.as_mut(), self.array.as_ref()) else {
            return;
        };
        let decoded = decoder.decode(packet, &mut self.pcm, self.samples_per_frame);
        if decoded <= 0 {
            return;
        }
        let samples = (decoded as usize * self.channels).min(self.pcm.len());
        let b = self.bridge;
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
