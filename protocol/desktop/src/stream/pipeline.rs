//! The parts the session drives on its own threads: the video decoder, the audio renderer
//! and the connection listener. They hand decoded pictures, notices and control events to
//! the UI thread through [`Shared`].

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nightjar_core::control::ControlEvent;
use nightjar_core::sdp::{VIDEO_FORMAT_MASK_AV1, VIDEO_FORMAT_MASK_H265, VIDEO_FORMAT_MASK_PYROWAVE, VIDEO_FORMAT_MASK_YUV444};
use nightjar_core::session::{
    self, AudioRenderer, AudioSetup, DecodeStatus, Listener, Stage, VideoDecoder, VideoSetup,
};
use nightjar_core::video::Frame;
use sdl3_sys::everything::*;

use crate::hosts::Waker;
use crate::media::{self, Codec, Hardware, Received};

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// A decoded picture waiting to be shown.
pub struct Decoded {
    pub frame: media::Frame,
    pub setup: VideoSetup,
    pub rtp: u32,
    /// The host's frame number.
    pub number: u32,
    /// The frame's first packet arrived (session clock, µs).
    pub receive_us: u64,
    /// The frame was complete and queued for decoding.
    pub enqueue_us: u64,
    /// The frame went to the decoder.
    pub input_us: u64,
    /// Decoding finished; the picture can be shown from here on.
    pub ready_us: u64,
    /// Host processing latency in 1/10 ms (0 when the host does not report it).
    pub host_latency: u16,
}

/// Something the UI should know about.
pub enum Notice {
    Reconnecting(usize),
    Stage(Stage),
    Started,
    Terminated(i32),
    Control(ControlEvent),
    DecoderFallback(String),
    MicrophoneFailed(String),
    Clipboard(Result<Option<String>, String>),
    ServerCommand(Result<Vec<u8>, String>),
    Bitrate { requested: u32, result: Result<u32, String> },
    AdaptiveBitrate(Result<(u32, u32), String>),
}

#[derive(Default)]
pub struct Counters {
    pub decoded: AtomicU64,
    pub decode_errors: AtomicU64,
    /// Pictures dropped because a newer one was ready before they were shown.
    pub replaced: AtomicU64,
    pub decode_us_total: AtomicU64,
    pub audio_dropped: AtomicU64,
    /// Audio waiting in SDL's stream at the last packet, in microseconds.
    pub audio_queued_us: AtomicU64,
    /// Times the device ran dry and silence was added.
    pub audio_underruns: AtomicU64,
    /// Output channels, 0 without an audio device.
    pub audio_channels: AtomicU64,
}

pub struct Shared {
    pub fps_millihz: AtomicU32,
    pub decode_window: Mutex<DecodeWindow>,
    pub host_client: Mutex<Option<Arc<nightjar_core::http::HostClient>>>,
    pub frames: Mutex<VecDeque<Decoded>>,
    pub notices: Mutex<Vec<Notice>>,
    pub decoder_name: Mutex<String>,
    pub controller_audio: Mutex<Vec<std::sync::Weak<Mutex<crate::controller_audio::Playback>>>>,
    pub counters: Counters,
    /// Most pictures held before the oldest is dropped.
    pub queue_limit: usize,
    waker: Waker,
}

#[derive(Default)]
pub struct DecodeWindow {
    pub record_loss_milli: u64,
    pub record_samples: u32,
    pub samples: u32,
    pub late: u32,
    pub first_us: u64,
    pub last_us: u64,
    pub queue_us: u64,
}

impl Shared {
    pub fn new(waker: Waker, queue_limit: usize) -> Arc<Self> {
        Arc::new(Self {
            fps_millihz: AtomicU32::new(60_000),
            decode_window: Mutex::new(DecodeWindow::default()),
            host_client: Mutex::new(None),
            frames: Mutex::new(VecDeque::new()),
            notices: Mutex::new(Vec::new()),
            decoder_name: Mutex::new(String::new()),
            controller_audio: Mutex::new(Vec::new()),
            counters: Counters::default(),
            queue_limit,
            waker,
        })
    }

    pub fn notify(&self, notice: Notice) {
        lock(&self.notices).push(notice);
        self.wake();
    }

    pub fn wake(&self) {
        (self.waker)();
    }
}

/// Decodes with FFmpeg on the session's decoder thread.
pub struct Decoder {
    pub shared: Arc<Shared>,
    pub hardware: Hardware,
    pub decoder: Option<media::VideoDecoder>,
    pub software_threads: i32,
    setup: Option<VideoSetup>,
    pyrowave: Option<super::pyrowave::Decoder>,
}

impl Decoder {
    pub fn new(shared: Arc<Shared>, hardware: Hardware) -> Self {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as i32;
        Self {
            shared,
            hardware,
            decoder: None,
            software_threads: threads,
            setup: None,
            pyrowave: None,
        }
    }
}

impl VideoDecoder for Decoder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.setup = Some(*setup);
        self.shared.fps_millihz.store(setup.fps.max(1) as u32 * 1000, Ordering::Relaxed);
        if setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            self.pyrowave = match super::pyrowave::Decoder::new(setup.width as usize, setup.height as usize, setup.format & VIDEO_FORMAT_MASK_YUV444 != 0) {
                Ok(decoder) => Some(decoder),
                Err(error) => { self.shared.notify(Notice::DecoderFallback(error.into())); return Err(-1); }
            };
            *lock(&self.shared.decoder_name) = "PyroWave (Rust, GPU wavelets)".into();
            return Ok(());
        }
        if setup.chroma_packing != 0 && self.hardware == Hardware::None {
            self.shared.notify(Notice::DecoderFallback(
                "Packed 4:4:4 requires hardware decoding".into(),
            ));
            return Err(-1);
        }
        let codec = if setup.format & VIDEO_FORMAT_MASK_AV1 != 0 {
            Codec::Av1
        } else if setup.format & VIDEO_FORMAT_MASK_H265 != 0 {
            Codec::Hevc
        } else {
            Codec::H264
        };
        let opened = match self.hardware {
            Hardware::None => {
                media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false)
            }
            hardware => media::VideoDecoder::open(codec, hardware, 1, setup.chroma_packing != 0)
                .or_else(|e| {
                    if setup.chroma_packing != 0 {
                        return Err(e);
                    }
                    self.shared.notify(Notice::DecoderFallback(format!(
                        "Hardware decoding unavailable: {e}"
                    )));
                    media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false)
                }),
        };
        match opened {
            Ok(decoder) => {
                *lock(&self.shared.decoder_name) = decoder.name.clone();
                self.decoder = Some(decoder);
                Ok(())
            }
            Err(e) => {
                self.shared
                    .notify(Notice::DecoderFallback(format!("No decoder: {e}")));
                Err(-1)
            }
        }
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        if let Some(decoder) = &mut self.pyrowave {
            let start = session::now_us();
            let setup = self.setup.expect("decoder setup precedes frames");
            let result = decoder.coefficients(frame.data()).map_err(str::to_owned).and_then(|_| decoder.coefficient_frame());
            let picture = match result {
                Ok(picture) => picture,
                Err(error) => { eprintln!("nightjar: PyroWave: {error}"); self.shared.counters.decode_errors.fetch_add(1, Ordering::Relaxed); return DecodeStatus::NeedIdr; }
            };
            let ready_us = session::now_us();
            let counters = &self.shared.counters;
            counters.decoded.fetch_add(1, Ordering::Relaxed);
            counters.decode_us_total.fetch_add(ready_us - start, Ordering::Relaxed);
            let mut timing = lock(&self.shared.decode_window);
            timing.samples += 1;
            timing.queue_us += start.saturating_sub(frame.enqueue_time_us);
            if decoder.loss_percent >= 0.0 {
                timing.record_loss_milli += (decoder.loss_percent * 1000.0) as u64;
                timing.record_samples += 1;
            }
            drop(timing);
            let decoded = Decoded { frame: picture, setup, rtp: frame.rtp_timestamp, number: frame.number, receive_us: frame.receive_time_us,
                enqueue_us: frame.enqueue_time_us, input_us: start, ready_us, host_latency: frame.host_processing_latency };
            let mut frames = lock(&self.shared.frames);
            frames.push_back(decoded);
            while frames.len() > self.shared.queue_limit { frames.pop_front(); counters.replaced.fetch_add(1, Ordering::Relaxed); }
            drop(frames);
            self.shared.wake();
            return DecodeStatus::Ok;
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return DecodeStatus::Ok;
        };
        let started = session::now_us();
        let pts = (i64::from(frame.number) << 32) | i64::from(frame.rtp_timestamp);
        if decoder.send(frame.data(), pts) < 0 {
            self.shared.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
            return DecodeStatus::NeedIdr;
        }
        let mut status = DecodeStatus::Ok;
        loop {
            match decoder.receive() {
                Received::Frame(picture) => {
                    let ready = session::now_us();
                    let pts = picture.info().pts;
                    let counters = &self.shared.counters;
                    counters.decoded.fetch_add(1, Ordering::Relaxed);
                    counters
                        .decode_us_total
                        .fetch_add(ready.saturating_sub(started), Ordering::Relaxed);
                    let mut timing = lock(&self.shared.decode_window);
                    if timing.samples == 0 { timing.first_us = ready; }
                    timing.last_us = ready;
                    timing.samples += 1;
                    timing.queue_us += started.saturating_sub(frame.enqueue_time_us);
                    let budget_us = 2_000_000_000 / u64::from(self.shared.fps_millihz.load(Ordering::Relaxed).max(1));
                    timing.late += u32::from(ready.saturating_sub(frame.enqueue_time_us) >= budget_us);
                    drop(timing);
                    let decoded = Decoded {
                        frame: picture,
                        setup: self.setup.expect("decoder setup precedes frames"),
                        rtp: pts as u32,
                        number: frame.number,
                        receive_us: frame.receive_time_us,
                        enqueue_us: frame.enqueue_time_us,
                        input_us: started,
                        ready_us: ready,
                        host_latency: frame.host_processing_latency,
                    };
                    let mut frames = lock(&self.shared.frames);
                    frames.push_back(decoded);
                    while frames.len() > self.shared.queue_limit {
                        frames.pop_front();
                        counters.replaced.fetch_add(1, Ordering::Relaxed);
                    }
                    drop(frames);
                    (self.shared.waker)();
                }
                Received::None => break,
                Received::Error => {
                    self.shared.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                    status = DecodeStatus::NeedIdr;
                    break;
                }
            }
        }
        status
    }

    fn cleanup(&mut self) {
        lock(&self.shared.frames).clear();
        self.decoder = None;
        self.pyrowave = None;
    }
}

/// Opus to an SDL audio stream, with the queue kept short.
pub struct Audio {
    pub shared: Arc<Shared>,
    decoder: Option<media::OpusDecoder>,
    stream: *mut SDL_AudioStream,
    pcm: Vec<f32>,
    /// Most audio queued in SDL before packets are dropped, in bytes.
    max_queued: i32,
    started: bool,
}

// SAFETY: SDL audio streams are thread-safe; the pointer is used by the audio thread only.
unsafe impl Send for Audio {}

impl Audio {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            decoder: None,
            stream: std::ptr::null_mut(),
            pcm: vec![0.0; 5760 * 8],
            max_queued: 0,
            started: false,
        }
    }
}

impl AudioRenderer for Audio {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        let opus = &setup.opus;
        self.decoder = Some(media::OpusDecoder::new(
            opus.channel_count,
            opus.streams,
            opus.coupled_streams,
            &opus.mapping,
        )?);
        let spec = SDL_AudioSpec {
            format: SDL_AUDIO_F32,
            channels: i32::from(opus.channel_count),
            freq: 48000,
        };
        // SAFETY: SDL's audio subsystem is initialised; the spec outlives the call.
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
                return Ok(());
            }
            SDL_ResumeAudioStreamDevice(self.stream);
        }
        self.shared
            .counters
            .audio_channels
            .store(u64::from(opus.channel_count), Ordering::Relaxed);
        // 40 ms: enough to ride out scheduling jitter, short enough to stay in sync.
        let bytes_per_ms = 48 * 4 * i32::from(opus.channel_count);
        self.max_queued = bytes_per_ms * (40 + setup.packet_duration_ms as i32);
        Ok(())
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        let samples = decoder.decode(packet, &mut self.pcm);
        if samples <= 0 {
            return;
        }
        let mut routed = false;
        for playback in lock(&self.shared.controller_audio).iter().filter_map(std::sync::Weak::upgrade) {
            routed |= lock(&playback).headphones(&self.pcm[..samples as usize * decoder.channels], decoder.channels);
        }
        if routed {
            if !self.stream.is_null() { unsafe { SDL_ClearAudioStream(self.stream) }; }
            self.started = false;
            self.shared.counters.audio_queued_us.store(0, Ordering::Relaxed);
            return;
        }
        if self.stream.is_null() { return; }
        let bytes = samples as usize * decoder.channels * 4;
        // SAFETY: the stream is open; pcm holds `bytes` bytes.
        unsafe {
            let queued = SDL_GetAudioStreamQueued(self.stream);
            let bytes_per_us = 48.0 * 4.0 * decoder.channels as f64 / 1000.0;
            self.shared
                .counters
                .audio_queued_us
                .store((f64::from(queued.max(0)) / bytes_per_us) as u64, Ordering::Relaxed);
            if queued > self.max_queued {
                self.shared.counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if queued == 0 {
                // Empty at the start or after an underrun: lead with 10 ms of silence so
                // the next packet's jitter does not starve the device again.
                if self.started {
                    self.shared.counters.audio_underruns.fetch_add(1, Ordering::Relaxed);
                }
                self.started = true;
                let silence = vec![0f32; 480 * decoder.channels];
                SDL_PutAudioStreamData(
                    self.stream,
                    silence.as_ptr().cast::<c_void>(),
                    (silence.len() * 4) as i32,
                );
            }
            SDL_PutAudioStreamData(self.stream, self.pcm.as_ptr().cast::<c_void>(), bytes as i32);
        }
    }

    fn cleanup(&mut self) {
        if !self.stream.is_null() {
            // SAFETY: opened in init, destroyed once.
            unsafe { SDL_DestroyAudioStream(self.stream) };
            self.stream = std::ptr::null_mut();
        }
        self.decoder = None;
    }
}

pub struct Events {
    pub shared: Arc<Shared>,
}

impl Listener for Events {
    fn stage_starting(&self, stage: Stage) {
        self.shared.notify(Notice::Stage(stage));
    }

    fn connection_started(&self) {
        self.shared.notify(Notice::Started);
    }

    fn connection_terminated(&self, error: i32) {
        self.shared.notify(Notice::Terminated(error));
    }

    fn control_event(&self, event: ControlEvent) {
        self.shared.notify(Notice::Control(event));
    }

    fn log(&self, message: &str) {
        eprintln!("nightjar: {message}");
    }
}
