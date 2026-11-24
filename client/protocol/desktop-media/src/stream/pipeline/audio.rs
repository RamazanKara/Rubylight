//! Stream audio decoding and SDL/WASAPI playback output, with packet-loss concealment
//! driven from the audio device callback.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use nightjar_core::session::{self, AudioRenderer, AudioSetup};
use sdl3_sys::everything::*;
use crate::media;
use super::super::playout::{Fed, Playout};
use super::{lock, Notice, Shared};

/// What the stream's packets are.
pub(in crate::stream) enum AudioDecoder {
    Opus(media::OpusDecoder),
    /// s16le samples, `channels` interleaved.
    Pcm { channels: usize },
}

impl AudioDecoder {
    pub(in crate::stream) fn channels(&self) -> usize {
        match self {
            Self::Opus(decoder) => decoder.channels,
            Self::Pcm { channels } => *channels,
        }
    }

    /// Decodes `packet` into `pcm`, or conceals a lost one for all of `pcm`: Opus with its
    /// concealment, PCM with silence. Returns the samples per channel written.
    pub(in crate::stream) fn decode(&mut self, packet: Option<&[u8]>, pcm: &mut [f32]) -> i32 {
        match self {
            Self::Opus(decoder) => decoder.decode(packet, pcm),
            Self::Pcm { channels } => {
                media::pcm_s16le(packet, *channels, pcm.len() / (*channels).max(1), pcm) as i32
            }
        }
    }
}

/// The passthrough codecs SDL plays here: PCM, which it mixes like decoded Opus to any output.
/// AC3 and E-AC3 need a bitstream output, which SDL has not, so they are never asked for.
/// Mute audio asks for none: Opus is the least to receive for sound nobody hears.
pub fn passthrough_codecs(enabled: bool, mute: bool) -> u8 {
    if enabled && !mute { rubylight_protocol::audio_ext::CODEC_PCM } else { 0 }
}

/// The stream's audio renderer: [`Audio`] on this device's output, or with Mute audio one that
/// opens none and plays nothing (`session::SilentAudio`). The audio is received either way.
pub fn audio_renderer(shared: Arc<Shared>, passthrough: u8, mute: bool) -> Box<dyn AudioRenderer> {
    if mute {
        Box::new(session::SilentAudio)
    } else {
        Box::new(Audio::new(shared, passthrough))
    }
}

/// Opus or PCM to an SDL audio stream, with the queue kept short. What runs dry between packets
/// is concealed from SDL's request callback (see [`Playout`]).
pub struct Audio {
    pub shared: Arc<Shared>,
    playout: Option<Arc<Mutex<Playout>>>,
    /// The passthrough codecs the stream asked for (`passthrough_codecs`).
    passthrough: u8,
    stream: *mut SDL_AudioStream,
    /// 7.1.4's output instead of `stream`: SDL opens at most 8 channels.
    #[cfg(windows)]
    wasapi: Option<super::super::wasapi::Output>,
    /// The callback's reference to `playout`, released after the stream is destroyed.
    callback: *const Mutex<Playout>,
}

// SAFETY: SDL audio streams are thread-safe; the pointers are used by the audio thread and
// SDL's callback only, under the stream's lock.
unsafe impl Send for Audio {}

impl Audio {
    pub fn new(shared: Arc<Shared>, passthrough: u8) -> Self {
        Self {
            shared,
            playout: None,
            passthrough,
            stream: std::ptr::null_mut(),
            #[cfg(windows)]
            wasapi: None,
            callback: std::ptr::null(),
        }
    }

    /// Where `play` puts what it decoded, locked for the call: the output's lock first, as the
    /// output holds it while it asks the playout for concealment.
    fn sink(&self) -> Sink<'_> {
        #[cfg(windows)]
        if let Some(output) = &self.wasapi {
            return Sink::Wasapi(lock(&output.queue));
        }
        if self.stream.is_null() {
            Sink::None
        } else {
            Sink::Sdl { stream: self.stream, _locked: StreamLock::new(self.stream) }
        }
    }

    /// 7.1.4 through WASAPI (`wasapi`); without an output the stream goes on without sound, as
    /// it does on SDL.
    #[cfg(windows)]
    fn init_wasapi(&mut self, decoder: AudioDecoder, packet_ms: u32) {
        match super::super::wasapi::Output::open() {
            Ok(output) => {
                let playout = Arc::new(Mutex::new(Playout::new(decoder, packet_ms, output.period_frames)));
                lock(&output.queue).playout = Some(playout.clone());
                eprintln!("nightjar: audio: 7.1.4 through WASAPI, {} frames a period", output.period_frames);
                self.playout = Some(playout);
                self.wasapi = Some(output);
            }
            Err(e) => {
                eprintln!("nightjar: no 7.1.4 audio output ({e})");
                self.playout = Some(Arc::new(Mutex::new(Playout::new(decoder, packet_ms, 0))));
            }
        }
    }
}

/// An audio output, locked (`Audio::sink`).
enum Sink<'a> {
    None,
    Sdl { stream: *mut SDL_AudioStream, _locked: StreamLock },
    #[cfg(windows)]
    Wasapi(std::sync::MutexGuard<'a, super::super::wasapi::Queue>),
    #[cfg(not(windows))]
    #[allow(dead_code)]
    Unused(std::marker::PhantomData<&'a ()>),
}

impl Sink<'_> {
    fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Bytes waiting for the device.
    fn queued(&self) -> i32 {
        match self {
            // SAFETY: the stream is open until cleanup, which runs on this thread.
            Self::Sdl { stream, .. } => unsafe { SDL_GetAudioStreamQueued(*stream) }.max(0),
            #[cfg(windows)]
            Self::Wasapi(queue) => (queue.pcm.len() * 4) as i32,
            _ => 0,
        }
    }

    fn clear(&mut self) {
        match self {
            // SAFETY: as above.
            Self::Sdl { stream, .. } => {
                unsafe { SDL_ClearAudioStream(*stream) };
            }
            #[cfg(windows)]
            Self::Wasapi(queue) => queue.pcm.clear(),
            _ => {}
        }
    }

    fn put(&mut self, pcm: &[f32]) {
        match self {
            // SAFETY: as above; pcm is valid for the call.
            Self::Sdl { stream, .. } => {
                unsafe { SDL_PutAudioStreamData(*stream, pcm.as_ptr().cast::<c_void>(), (pcm.len() * 4) as i32) };
            }
            #[cfg(windows)]
            Self::Wasapi(queue) => queue.pcm.extend(pcm),
            _ => {}
        }
    }
}

/// Holds an audio stream's lock (none for a null stream) until dropped.
struct StreamLock(*mut SDL_AudioStream);

impl StreamLock {
    fn new(stream: *mut SDL_AudioStream) -> Self {
        // SAFETY: a non-null stream is open until cleanup, which runs on the same thread.
        if !stream.is_null() && !unsafe { SDL_LockAudioStream(stream) } {
            return Self(std::ptr::null_mut());
        }
        Self(stream)
    }
}

impl Drop for StreamLock {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: locked in new.
            unsafe { SDL_UnlockAudioStream(self.0) };
        }
    }
}

/// SDL wants more than is queued: conceal it. SDL holds the stream's lock, so this never runs
/// while `play` feeds the stream.
unsafe extern "C" fn conceal(userdata: *mut c_void, stream: *mut SDL_AudioStream, additional: i32, _total: i32) {
    if additional <= 0 {
        return;
    }
    // SAFETY: userdata is the `Arc` reference `init` leaked; it outlives the stream.
    let mut playout = lock(unsafe { &*(userdata as *const Mutex<Playout>) });
    let frame_bytes = 4 * playout.channels();
    let pcm = playout.starve(additional as usize / frame_bytes);
    if !pcm.is_empty() {
        // SAFETY: the stream is the one this callback was set on; pcm is valid for the call.
        unsafe { SDL_PutAudioStreamData(stream, pcm.as_ptr().cast::<c_void>(), (pcm.len() * 4) as i32) };
    }
}

impl AudioRenderer for Audio {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        use rubylight_protocol::audio_ext::{CODEC_OPUS, CODEC_PCM};
        let opus = &setup.opus;
        let channels = setup.configuration.channel_count;
        let name = nightjar_core::audio::passthrough::codec_name(setup.codec);
        let decoder = match setup.codec {
            CODEC_PCM => AudioDecoder::Pcm { channels: usize::from(channels) },
            CODEC_OPUS => AudioDecoder::Opus(media::OpusDecoder::new(
                opus.channel_count,
                opus.streams,
                opus.coupled_streams,
                &opus.mapping,
            )?),
            _ => {
                // Never asked for (`passthrough_codecs`): refuse rather than play noise.
                eprintln!("nightjar: audio: the stream is {name}, which this client cannot play");
                return Err(-1);
            }
        };
        if setup.codec == CODEC_PCM {
            eprintln!("nightjar: audio: PCM passthrough, {channels} channels, {} ms packets", setup.packet_duration_ms);
        } else if self.passthrough != 0 {
            eprintln!("nightjar: audio: the PC did not offer PCM audio for {channels} channels; playing Opus");
            self.shared.notify(Notice::PassthroughOpus);
        }
        let channels = decoder.channels();
        #[cfg(windows)]
        if channels == super::super::wasapi::CHANNELS {
            self.init_wasapi(decoder, setup.packet_duration_ms);
            self.shared.counters.audio_channels.store(channels as u64, Ordering::Relaxed);
            return Ok(());
        }
        let spec = SDL_AudioSpec {
            format: SDL_AUDIO_F32,
            channels: channels as i32,
            freq: 48000,
        };
        // SAFETY: SDL's audio subsystem is initialised; the spec outlives the call. The
        // callback's reference is released in cleanup, after the stream is destroyed.
        unsafe {
            self.stream = SDL_OpenAudioDeviceStream(
                SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK,
                &spec,
                None,
                std::ptr::null_mut(),
            );
            if self.stream.is_null() {
                // No audio device: keep streaming without sound.
                eprintln!("nightjar: no audio output ({})", crate::sdl::error());
                self.playout = Some(Arc::new(Mutex::new(Playout::new(decoder, setup.packet_duration_ms, 0))));
                return Ok(());
            }
            // How much the device takes at once: the cushion led with must cover it.
            let mut device = std::mem::zeroed::<SDL_AudioSpec>();
            let mut device_frames = 0;
            if !SDL_GetAudioDeviceFormat(SDL_GetAudioStreamDevice(self.stream), &mut device, &mut device_frames) {
                device_frames = 0;
            }
            let playout = Arc::new(Mutex::new(Playout::new(
                decoder,
                setup.packet_duration_ms,
                device_frames.max(0) as usize * 48000 / device.freq.max(1) as usize,
            )));
            self.playout = Some(playout.clone());
            self.callback = Arc::into_raw(playout);
            if !SDL_SetAudioStreamGetCallback(self.stream, Some(conceal), self.callback as *mut c_void) {
                eprintln!("nightjar: late audio plays silence, not concealment ({})", crate::sdl::error());
                lock(&*self.callback).without_requests();
            }
            SDL_ResumeAudioStreamDevice(self.stream);
        }
        self.shared
            .counters
            .audio_channels
            .store(channels as u64, Ordering::Relaxed);
        Ok(())
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        let Some(playout) = self.playout.as_ref() else {
            return;
        };
        // The output's lock first, as SDL takes it before calling `conceal`.
        let mut sink = self.sink();
        let mut playout = lock(playout);
        let channels = playout.channels();
        let bytes_per_sample = 4 * channels as i32;
        let queued = sink.queued();
        let fed = playout.packet(packet, (queued / bytes_per_sample) as usize);
        let counters = &self.shared.counters;
        let bytes_per_us = 48.0 * f64::from(bytes_per_sample) / 1000.0;
        counters.audio_queued_us.store((f64::from(queued) / bytes_per_us) as u64, Ordering::Relaxed);
        counters.audio_concealed.fetch_add(playout.take_concealed(), Ordering::Relaxed);
        if playout.take_bursts_started() {
            eprintln!(
                "nightjar: audio: packets arrive in late bursts, keeping them to grow the cushion ({:.1} ms queued)",
                f64::from(queued) / bytes_per_us / 1000.0
            );
        }
        let lead = match fed {
            Fed::Play { lead, underrun } => {
                if underrun {
                    counters.audio_underruns.fetch_add(1, Ordering::Relaxed);
                }
                lead
            }
            Fed::Dropped => {
                counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Fed::Trimmed => {
                counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "nightjar: audio: clock drift, trimmed a packet with {:.1} ms queued",
                    f64::from(queued) / bytes_per_us / 1000.0
                );
                return;
            }
            Fed::Failed => return,
        };
        let pcm = playout.pcm();
        let mut routed = false;
        if self.shared.controller_audio.has_any() {
            for headphones in lock(&self.shared.controller_audio).iter().filter_map(std::sync::Weak::upgrade) {
                routed |= lock(&headphones).headphones(&pcm[lead * channels..], channels);
            }
        }
        if routed { playout.stop(); sink.clear(); counters.audio_queued_us.store(0, Ordering::Relaxed); return; }
        if sink.is_none() { return; }
        sink.put(pcm);
    }

    fn cleanup(&mut self) {
        // Joins the render thread, which then holds no reference to the playout.
        #[cfg(windows)]
        drop(self.wasapi.take());
        if !self.stream.is_null() {
            // SAFETY: opened in init, destroyed once; the callback cannot run after this.
            unsafe { SDL_DestroyAudioStream(self.stream) };
            self.stream = std::ptr::null_mut();
        }
        if !self.callback.is_null() {
            // SAFETY: leaked by init with Arc::into_raw, released once.
            drop(unsafe { Arc::from_raw(self.callback) });
            self.callback = std::ptr::null();
        }
        self.playout = None;
    }
}
