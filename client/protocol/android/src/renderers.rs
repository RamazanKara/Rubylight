//! nightjar-core's decoder and audio renderer, forwarded to MoonBridge's `bridgeDr*` and
//! `bridgeAr*` callbacks as `callbacks.c` does (moonlight-common-c's Android binding, GPL-3.0,
//! Cameron Gutman and contributors): parameter sets are submitted on their own, picture data
//! in one buffer, and Opus is decoded here so Java receives PCM. Passthrough PCM goes on as
//! shorts the same way; AC3 and E-AC3 frames go to `bridgeArPlayEncoded` as bytes.

use std::sync::{Mutex, MutexGuard, TryLockError};

use jni::objects::{GlobalRef, JShortArray};

use nightjar_core::sdp::VIDEO_FORMAT_MASK_PYROWAVE;
use nightjar_core::session::{
    self, AudioRenderer, AudioSetup, DecodeStatus, VideoDecoder, VideoSetup,
};
use nightjar_core::video::{BufferType, Frame, FrameBuffer};
use rubylight_protocol::pyrowave::PACKET_FRAME_END;

use crate::bridge::{self, byte_array, Bridge};
use crate::extras::audio_configuration_to_int;
use crate::{opus, platform};

const DR_OK: i32 = 0;
const BUFFER_TYPE_PICDATA: i32 = 0;

pub struct Video {
    bridge: &'static Bridge,
    buffer: Option<GlobalRef>,
    capacity: usize,
    /// PyroWave records frames reach the renderer as their packets arrive (early record push).
    early: bool,
    /// Picture data goes to Java in place, as a direct buffer ([`direct_hand_off`]).
    direct: bool,
    /// The "Video data straight to the decoder" setting, for [`direct_hand_off`].
    codec_direct: bool,
    chunks: ChunkBatch,
}

impl Video {
    pub fn new(bridge: &'static Bridge, early: bool, codec_direct: bool) -> Self {
        Self {
            bridge,
            buffer: None,
            capacity: 0,
            early,
            direct: false,
            codec_direct,
            chunks: ChunkBatch::default(),
        }
    }

    /// Hands the collected chunks to `bridgeDrSubmitChunks`, in the frame buffer: Java's renderer
    /// copies them out before it returns, and the frame comes after them on this thread.
    fn flush_chunks(&mut self) {
        if self.chunks.bytes.is_empty() {
            return;
        }
        if self.direct {
            self.flush_chunks_in_place();
            return;
        }
        let b = self.bridge;
        let needed = self.chunks.bytes.len();
        let (chunks, buffer, capacity) = (&self.chunks, &mut self.buffer, &mut self.capacity);
        b.with_env(|env| {
            if needed > *capacity || buffer.is_none() {
                let size = grown_capacity(needed, *capacity);
                let array = env.new_byte_array(size as i32)?;
                *buffer = Some(env.new_global_ref(array)?);
                *capacity = size;
            }
            let buffer = buffer.as_ref().expect("allocated above");
            env.set_byte_array_region(byte_array(buffer), 0, bridge::jbytes(&chunks.bytes))?;
            let args = [
                bridge::object(buffer.as_obj()),
                bridge::int(needed as i32),
                jni::objects::JValue::Bool(chunks.first.into()).as_jni(),
            ];
            b.void(env, b.m.dr_chunks, &args);
            Ok(())
        });
        self.chunks.clear();
    }

    /// The same batch as [`flush_chunks`](Self::flush_chunks), as a direct buffer over the batch's
    /// own memory (`debug.nightjar.pyro_direct`, as for frames): Java's renderer reads it during
    /// the call, and `self.chunks` is not touched until the call returns, so the bytes stay put.
    fn flush_chunks_in_place(&mut self) {
        let b = self.bridge;
        let (bytes, first) = (&self.chunks.bytes, self.chunks.first);
        let length = bytes.len();
        b.with_env(|env| {
            // SAFETY: Java only reads the buffer, and only until the call returns (the contract
            // of bridgeDrSubmitChunksDirect); `bytes` is borrowed from `self.chunks`, which is
            // neither moved nor cleared until after the call, and the local reference goes with
            // with_env's frame.
            let direct = unsafe { env.new_direct_byte_buffer(bytes.as_ptr().cast_mut(), length)? };
            let args = [
                bridge::object(&direct),
                bridge::int(length as i32),
                jni::objects::JValue::Bool(first.into()).as_jni(),
            ];
            b.void(env, b.m.dr_chunks_direct, &args);
            Ok(())
        });
        self.chunks.clear();
    }
}

/// The size of a new frame array for `needed` bytes when the `capacity` one is too small: half as
/// large again, so frames that creep up in size (as a stream's do after its start) make a new
/// ~1 MB Java array, and garbage of the old one, a few times rather than at every new largest
/// frame. Java reads only the length each call passes.
fn grown_capacity(needed: usize, capacity: usize) -> usize {
    needed.max(capacity.saturating_add(capacity / 2)).min(i32::MAX as usize)
}

/// Early chunks collected before one call into Java: about two dozen packets, so a frame of
/// 800 KB (400 Mbps at 60 fps) takes some 25 calls rather than 600, and the frame's own submit
/// has at most this much left to push.
const CHUNK_BATCH_BYTES: usize = 32 * 1024;

/// Early chunks (`VideoDecoder::chunk`) for one `bridgeDrSubmitChunks` call, in the records
/// envelope's own layout: per packet its byte length, its flags and its payload. The buffer is
/// reused, so once it has grown a frame's chunks allocate nothing.
#[derive(Default)]
struct ChunkBatch {
    bytes: Vec<u8>,
    /// The batch starts with the first packet of a frame.
    first: bool,
}

impl ChunkBatch {
    /// Adds a packet; true when the batch should go now, because the packet ends the frame or
    /// the batch is full. A packet that starts a frame drops what is left of the last one, whose
    /// frame never came (the decoder thread takes chunks and frames in arrival order).
    fn push(&mut self, first: bool, flags: u32, payload: &[u8]) -> bool {
        if first {
            self.bytes.clear();
            self.first = true;
        }
        self.bytes
            .extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(&flags.to_le_bytes());
        self.bytes.extend_from_slice(payload);
        flags & PACKET_FRAME_END != 0 || self.bytes.len() >= CHUNK_BATCH_BYTES
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.first = false;
    }
}

/// Whether a stream of `format` hands its picture data to Java in place, as a direct buffer the
/// renderer reads during the call, instead of copying it into a Java array first. PyroWave reads
/// it in native code, which saves both copies (Java's array and the renderer's own); it is on
/// unless `debug.nightjar.pyro_direct` is 0. MediaCodec copies it straight into the codec's input
/// buffer, one copy fewer; that follows the Debug setting "Video data straight to the decoder"
/// (`setting`, on by default since it measured slightly better on the Retroid), which
/// `debug.nightjar.codec_direct` 1 or 0 overrides.
fn direct_hand_off(format: u32, pyro_direct: &str, codec_direct: &str, setting: bool) -> bool {
    if format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
        pyro_direct != "0"
    } else {
        codec_direct == "1" || (codec_direct != "0" && setting)
    }
}

/// The range of the frame's picture data, when its picture buffers lie back to back.
fn contiguous_picture(buffers: &[FrameBuffer]) -> Option<std::ops::Range<usize>> {
    let mut range: Option<std::ops::Range<usize>> = None;
    for buffer in buffers {
        if buffer.kind != BufferType::PicData {
            continue;
        }
        range = match range {
            None => Some(buffer.range.clone()),
            Some(r) if r.end == buffer.range.start => Some(r.start..buffer.range.end),
            Some(_) => return None,
        };
    }
    range.filter(|r| !r.is_empty())
}

/// The Java array a frame needs: all of it, or with its picture in place only the largest
/// parameter set, which still goes through the array.
fn array_bytes(buffers: &[FrameBuffer], total: usize, picture_in_place: bool) -> usize {
    if !picture_in_place {
        return total;
    }
    buffers
        .iter()
        .filter(|b| b.kind != BufferType::PicData)
        .map(|b| b.range.len())
        .max()
        .unwrap_or(0)
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
                self.direct = direct_hand_off(
                    setup.format,
                    &platform::system_property(c"debug.nightjar.pyro_direct"),
                    &platform::system_property(c"debug.nightjar.codec_direct"),
                    self.codec_direct,
                );
                platform::info(&format!(
                    "Picture data goes to the renderer {}",
                    if self.direct {
                        "in place"
                    } else {
                        "through a Java array"
                    }
                ));
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
        // Chunks still collected go first: the frame's end packet, which sends a batch, was lost.
        self.flush_chunks();
        let b = self.bridge;
        let receive_ns = monotonic_ns_of(frame.receive_time_us);
        // Picture data in one piece goes to Java in place, as a direct buffer over the frame
        // that is only valid during the call; split picture data takes the array as before.
        let picture = if self.direct {
            contiguous_picture(frame.buffers())
        } else {
            None
        };
        let needed = array_bytes(frame.buffers(), frame.data().len(), picture.is_some());
        let status = b.with_env(|env| {
            if needed > self.capacity || self.buffer.is_none() {
                let size = grown_capacity(needed, self.capacity);
                let array = env.new_byte_array(size as i32)?;
                self.buffer = Some(env.new_global_ref(array)?);
                self.capacity = size;
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
            let bytes = &frame.data()[picture];
            // SAFETY: Java only reads the buffer, and only until the call returns (the
            // contract of bridgeDrSubmitDirectDecodeUnit); `frame` outlives the call, and the
            // local reference goes with with_env's frame.
            let direct =
                unsafe { env.new_direct_byte_buffer(bytes.as_ptr().cast_mut(), bytes.len())? };
            let args = [
                bridge::object(&direct),
                bridge::int(bytes.len() as i32),
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

    fn wants_early_chunks(&self) -> bool {
        self.early
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if self.chunks.push(first, flags, payload) {
            self.flush_chunks();
        }
    }

    fn stop(&mut self) {
        self.bridge.post(self.bridge.m.dr_stop, &[]);
    }

    fn cleanup(&mut self) {
        self.buffer = None;
        self.chunks.clear();
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

/// The stream's Opus decoder and its channel count. The decode thread decodes with it; the
/// AAudio callback conceals a stall with it ([`conceal`]) while that thread waits for packets.
static DECODER: Mutex<Option<(opus::Decoder, usize)>> = Mutex::new(None);

fn shared_decoder() -> MutexGuard<'static, Option<(opus::Decoder, usize)>> {
    DECODER.lock().unwrap_or_else(|p| p.into_inner())
}

/// Opus packet loss concealment of `frames` stereo frames into `pcm`, for the AAudio callback
/// when the network stalls: the frames concealed, or 0 when there is no stereo decoder or the
/// decode thread holds it (the callback must not wait). `frames` is a multiple of 2.5 ms.
///
/// # Safety
/// `pcm` holds `frames * 2` samples.
pub unsafe extern "C" fn conceal(pcm: *mut i16, frames: i32) -> i32 {
    let mut guard = match DECODER.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return 0,
    };
    let Some((decoder, 2)) = guard.as_mut() else {
        return 0;
    };
    if pcm.is_null() || frames <= 0 {
        return 0;
    }
    let pcm = std::slice::from_raw_parts_mut(pcm, frames as usize * 2);
    decoder.decode(None, pcm, frames as usize).max(0)
}

pub struct Audio {
    bridge: &'static Bridge,
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
        *shared_decoder() = None;
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
            *shared_decoder() = Some((decoder, self.channels));
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
                let decoded = match shared_decoder().as_mut() {
                    Some((decoder, _)) => {
                        decoder.decode(packet, &mut self.pcm, self.samples_per_frame)
                    }
                    None => return,
                };
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
        *shared_decoder() = None;
        self.array = None;
        self.bridge.post(self.bridge.m.ar_cleanup, &[]);
    }
}

#[cfg(test)]
mod tests;
