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
use crate::media::{self, Codec, Hardware, Picture, Received};
use super::playout::{Fed, Playout};
use super::pyrowave::Failure;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// A decoded picture waiting to be shown.
pub struct Decoded {
    pub picture: Picture,
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
    /// How early decode went for this frame: 0 off or not PyroWave, 1 early, 2 fell back to
    /// decoding the whole frame (the timing CSV's `early`).
    pub early: u8,
    /// When its first block was decoded, before the frame completed (session clock, µs); 0
    /// unless `early` is 1.
    pub first_block_us: u64,
    /// How the beam race went (all zero unless it is on): the timing CSV's slice columns.
    pub race: super::pyrowave::RaceReport,
    /// The slices decoded for the frame, in order, each ready once its fence value is signalled
    /// (empty unless the beam race is on). The last runs to the end of the picture.
    pub bands: Vec<super::pyrowave::BandReady>,
    /// The sequence number the frame's slices came with (`SliceReady::seq`), 0 when it had none.
    pub race_seq: u64,
}

/// Something the UI should know about.
pub enum Notice {
    Reconnecting(usize),
    Stage(Stage),
    Started,
    Terminated(i32),
    Control(ControlEvent),
    DecoderFallback(String),
    /// The GPU PyroWave decoder broke, or a decoder kept failing with no picture; the stream
    /// ends with this message (nothing falls back).
    DecoderFailed(String),
    MicrophoneFailed(String),
    /// Audio passthrough was asked for, but the PC sends Opus; the stream plays on with a message.
    PassthroughOpus,
    Clipboard(Result<Option<String>, String>),
    /// An image sent to the PC (`None`) or taken from it, ready for the clipboard here; the
    /// error is the text to show.
    ClipboardImage(Result<Option<crate::clipboard::Prepared>, String>),
    ServerCommand(Result<Vec<u8>, String>),
    /// The host's answer to opening its menu; the error is the text to show.
    HostMenu(Result<(), String>),
    Bitrate { requested: u32, result: Result<u32, String> },
    AdaptiveBitrate(Result<(u32, u32), String>),
    /// A step of the stream doctor's bitrate trial came back.
    HealthTrial(super::doctor::TrialOutcome),
    /// Client roaming: the network watcher saw this device change networks.
    NetworkChanged(nightjar_core::session::NetworkHint),
    /// The host's hardware readings for the overlay, answering poll `generation`.
    HardwareMetrics { generation: u32, result: Result<nightjar_core::http::HardwareMetrics, nightjar_core::http::HostApiError> },
}

#[derive(Default)]
pub struct Counters {
    pub decoded: AtomicU64,
    pub decode_errors: AtomicU64,
    /// Pictures dropped because a newer one was ready before they were shown.
    pub replaced: AtomicU64,
    pub decode_us_total: AtomicU64,
    /// The header's processing times (100 us units) of the received frames that carried one, and
    /// how many did: the overlay's Host processing, counted like Artemis does, before any decode.
    pub host_latency_total: AtomicU64,
    pub host_latency_frames: AtomicU64,
    /// Video payload bytes of the frames handed to the decoder: the overlay's Mbps.
    pub video_bytes: AtomicU64,
    /// PyroWave early decode: frames whose blocks were decoded while they arrived, frames
    /// whose chunks did not match and decoded whole, and the time spent decoding chunks.
    pub early_frames: AtomicU64,
    pub early_fallbacks: AtomicU64,
    pub early_us_total: AtomicU64,
    /// Beam race: frames that were decoded in slices while they arrived, frames that were not
    /// in row order (decoded whole), and the slice decodes made.
    pub race_frames: AtomicU64,
    pub race_unordered: AtomicU64,
    pub race_slices: AtomicU64,
    pub audio_dropped: AtomicU64,
    /// Audio waiting in SDL's stream at the last packet, in microseconds.
    pub audio_queued_us: AtomicU64,
    /// Times the device ran dry and silence was added.
    pub audio_underruns: AtomicU64,
    /// Packets' time filled with Opus concealment: lost, or late for the device.
    pub audio_concealed: AtomicU64,
    /// Output channels, 0 without an audio device.
    pub audio_channels: AtomicU64,
}

impl Counters {
    /// Counts one received frame's header processing time; frames without one are not counted.
    pub fn count_host_latency(&self, units: u16) {
        if units != 0 {
            self.host_latency_total.fetch_add(u64::from(units), Ordering::Relaxed);
            self.host_latency_frames.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Slices waiting for the raced presenter at most (a frame has a handful).
const MAX_SLICES_QUEUED: usize = 64;

pub struct Shared {
    pub fps_millihz: AtomicU32,
    pub decode_window: Mutex<DecodeWindow>,
    pub host_client: Mutex<Option<Arc<nightjar_core::http::HostClient>>>,
    pub frames: Mutex<VecDeque<Decoded>>,
    /// Slices of frames still arriving, for the raced presenter (beam race only), oldest first.
    pub slices: Mutex<VecDeque<super::pyrowave::SliceReady>>,
    /// The pacer's queue (`Pacer::with_backlog`), reachable from the decoder thread for `evict_oldest`.
    pub pacer_backlog: Arc<Mutex<VecDeque<super::present::Pending>>>,
    pub notices: Mutex<Vec<Notice>>,
    /// The connect attempt whose listener may still end the stream; changed only with
    /// `notices` locked, so a late termination of an earlier attempt is dropped, not queued.
    attempt: AtomicU32,
    pub decoder_name: Mutex<String>,
    pub controller_audio: Mutex<Vec<std::sync::Weak<Mutex<crate::controller_audio::Playback>>>>,
    pub counters: Counters,
    /// Most pictures held before the oldest is dropped.
    pub queue_limit: usize,
    /// Client roaming's network watcher while a session runs; dropping it stops it.
    pub roam_watch: Mutex<Option<crate::network::RoamWatch>>,
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
            slices: Mutex::new(VecDeque::new()),
            pacer_backlog: Default::default(),
            notices: Mutex::new(Vec::new()),
            attempt: AtomicU32::new(0),
            decoder_name: Mutex::new(String::new()),
            controller_audio: Mutex::new(Vec::new()),
            counters: Counters::default(),
            queue_limit,
            roam_watch: Mutex::new(None),
            waker,
        })
    }

    /// Frees a GPU plane set when the decoder has none: the oldest picture still queued for the
    /// presenter goes, first from `frames`, then from the pacer (which, scheduling by host
    /// times, can hold many pictures that are not due yet). The frame being decoded is not queued yet,
    /// so it is never the one dropped. True if a picture was dropped.
    pub fn evict_oldest(&self) -> bool {
        let dropped = lock(&self.frames).pop_front().is_some() || lock(&self.pacer_backlog).pop_front().is_some();
        if dropped { self.counters.replaced.fetch_add(1, Ordering::Relaxed); }
        dropped
    }

    /// Counts how the frame's blocks were decoded (see `Decoded::early`); returns the `early` code.
    fn count_source(&self, source: super::pyrowave::Source) -> u8 {
        match source {
            super::pyrowave::Source::Frame => 0,
            super::pyrowave::Source::Early => { self.counters.early_frames.fetch_add(1, Ordering::Relaxed); 1 }
            super::pyrowave::Source::Fallback => {
                if self.counters.early_fallbacks.fetch_add(1, Ordering::Relaxed) == 0 {
                    eprintln!("nightjar: PyroWave: early chunks did not match the frame, decoding it whole (counted, not repeated)");
                }
                2
            }
        }
    }

    /// Counts a frame's beam race (see `RaceReport`); a frame that arrived out of row order says
    /// so once on stderr and always in the counters and the CSV.
    fn count_race(&self, race: &super::pyrowave::RaceReport) {
        use super::pyrowave::RaceReport;
        match race.ordered {
            RaceReport::ORDERED => {
                self.counters.race_frames.fetch_add(1, Ordering::Relaxed);
                self.counters.race_slices.fetch_add(u64::from(race.slices), Ordering::Relaxed);
            }
            RaceReport::UNORDERED => {
                let first = self.counters.race_unordered.fetch_add(1, Ordering::Relaxed) == 0;
                if first {
                    eprintln!("nightjar: beam race: a frame did not arrive in row order, so it decoded whole (counted, not repeated)");
                }
            }
            _ => {}
        }
    }

    pub fn notify(&self, notice: Notice) {
        lock(&self.notices).push(notice);
        self.wake();
    }

    /// A failed attempt is being retried: its notices are dropped, and so is anything its
    /// listener sends from now on.
    pub fn next_attempt(&self) {
        let mut notices = lock(&self.notices);
        self.attempt.fetch_add(1, Ordering::SeqCst);
        notices.clear();
    }

    /// Queues `notice` if it comes from the current connect attempt; false if it was dropped.
    fn notify_from(&self, attempt: u32, notice: Notice) -> bool {
        {
            let mut notices = lock(&self.notices);
            if self.attempt.load(Ordering::SeqCst) != attempt {
                return false;
            }
            notices.push(notice);
        }
        self.wake();
        true
    }

    pub fn wake(&self) {
        (self.waker)();
    }

    /// Hands slices that finished decoding to the presenter and wakes it. The queue is bounded: a
    /// presenter that does not keep up loses the oldest, which it would have dropped as stale.
    fn push_slices(&self, slices: Vec<super::pyrowave::SliceReady>) {
        if slices.is_empty() { return; }
        let mut queue = lock(&self.slices);
        queue.extend(slices);
        while queue.len() > MAX_SLICES_QUEUED { queue.pop_front(); }
        drop(queue);
        self.wake();
    }
}

/// The renderer's GPU device, lent to the decoder thread: the `ID3D11Device` on Windows (the SDK
/// decoder shares textures with it), the `SDL_GPUDevice` on Linux (a marker that the renderer can
/// draw decoded planes; the SDK decodes on its own Vulkan device). The renderer outlives the
/// stream and D3D11 devices are free-threaded.
#[derive(Clone, Copy)]
pub struct GpuDevice(pub *mut c_void);

// SAFETY: See above; the decoder thread only AddRefs the device.
unsafe impl Send for GpuDevice {}

/// Decodes with FFmpeg on the session's decoder thread.
pub struct Decoder {
    pub shared: Arc<Shared>,
    pub hardware: Hardware,
    pub decoder: Option<media::VideoDecoder>,
    pub software_threads: i32,
    setup: Option<VideoSetup>,
    pyrowave: Option<super::pyrowave::Pyrowave>,
    early: bool,
    /// Slices per frame for the beam race (0 off, `Settings::beam_race`).
    slices: u32,
    /// The renderer's GPU device when PyroWave is to be decoded on the GPU (`pyrowave_decoder = gpu`).
    gpu: Option<GpuDevice>,
    /// Measurement: where the first `PYROWAVE_DUMP_FRAMES` envelopes go (`NIGHTJAR_PYROWAVE_DUMP`),
    /// and how many were written.
    dump: Option<(std::path::PathBuf, u32)>,
    /// Frames that failed since the last picture.
    failing: Failing,
    /// The frame being submitted failed for data lost on the way (`Failure::Loss`).
    lost: bool,
}

/// How long frames may fail with no picture between before the decoder is taken for broken.
/// After a GPU reset or a lost device a hardware decoder fails every frame, and each failed
/// frame only asks for the next IDR, so the picture would freeze without a word.
const FAILING_FOR_US: u64 = 1_000_000;
/// And how many frames, so that one bad frame and the next one after a pause are not enough.
const FAILING_FRAMES: u32 = 3;

/// The frames that failed since the decoder last made a picture.
#[derive(Debug, Default, PartialEq)]
struct Failing {
    since_us: Option<u64>,
    frames: u32,
    /// The decoder was opened again in this run of failures.
    reopened: bool,
    /// The stream was told to end; nothing more is said.
    given_up: bool,
}

/// What a decoder that failed again has to do.
#[derive(Debug, PartialEq)]
enum Escalation {
    /// Ask for an IDR as usual.
    Wait,
    /// Open the decoder again (once).
    Reopen,
    /// End the stream: the decoder failed after it was opened again, or cannot be.
    GiveUp,
}

impl Failing {
    fn decoded(&mut self) {
        *self = Self::default();
    }

    fn failed(&mut self, now_us: u64, reopenable: bool) -> Escalation {
        if self.given_up {
            return Escalation::Wait;
        }
        let since = *self.since_us.get_or_insert(now_us);
        self.frames += 1;
        if self.frames < FAILING_FRAMES || now_us.saturating_sub(since) < FAILING_FOR_US {
            return Escalation::Wait;
        }
        if reopenable && !self.reopened {
            *self = Self { reopened: true, ..Self::default() };
            return Escalation::Reopen;
        }
        self.given_up = true;
        Escalation::GiveUp
    }
}

/// How many real PyroWave envelopes `NIGHTJAR_PYROWAVE_DUMP` keeps.
const PYROWAVE_DUMP_FRAMES: u32 = 120;

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
            early: false,
            slices: 0,
            gpu: None,
            dump: std::env::var_os("NIGHTJAR_PYROWAVE_DUMP").filter(|dir| !dir.is_empty()).map(|dir| (dir.into(), 0)),
            failing: Failing::default(),
            lost: false,
        }
    }

    /// A frame failed with no picture since the last one: after [`FAILING_FOR_US`] of that the
    /// FFmpeg decoder is opened again once, and when it still fails, or PyroWave does, the
    /// stream ends saying why instead of freezing.
    fn frame_failed(&mut self, now_us: u64) {
        let frames = self.failing.frames + 1;
        let for_ms = self.failing.since_us.map_or(0, |since| now_us.saturating_sub(since) / 1000);
        match self.failing.failed(now_us, self.decoder.is_some()) {
            Escalation::Wait => {}
            Escalation::Reopen => {
                eprintln!("nightjar: the video decoder failed {frames} frames in a row over {for_ms} ms with no picture; opening it again");
                let setup = self.setup.expect("decoder setup precedes frames");
                self.decoder = None;
                if self.setup(&setup).is_err() || self.decoder.is_none() {
                    self.failing.given_up = true;
                    self.shared.notify(Notice::DecoderFailed("The video decoder stopped working and could not be opened again.".into()));
                }
            }
            Escalation::GiveUp => {
                eprintln!("nightjar: the video decoder failed {frames} frames in a row over {for_ms} ms with no picture; ending the stream");
                self.shared.notify(Notice::DecoderFailed(format!("The video decoder stopped working: no picture for {} s.", for_ms.div_ceil(1000))));
            }
        }
    }

    /// Counts a submitted frame towards [`Failing`]: a picture starts over, and a frame that failed
    /// in the decoder counts. One that only lost too much of its data on the way does not.
    fn after_frame(&mut self, decoded: bool, need_idr: bool, now_us: u64) {
        if decoded {
            self.failing.decoded();
        } else if need_idr && !self.lost {
            self.frame_failed(now_us);
        }
    }

    /// Writes a PyroWave envelope as `frame-<w>x<h>-<420|444>-<n>.bin`, which
    /// `cpu_costs_of_a_pyrowave_frame` reads back through `NIGHTJAR_PYROWAVE_FRAMES`.
    fn dump_envelope(&mut self, setup: &VideoSetup, data: &[u8]) {
        let Some((dir, written)) = &mut self.dump else { return };
        if *written >= PYROWAVE_DUMP_FRAMES { return; }
        let path = dir.join(super::pyrowave::dump_name(setup.width as usize, setup.height as usize, setup.format & VIDEO_FORMAT_MASK_YUV444 != 0, *written));
        let result = std::fs::create_dir_all(&*dir).and_then(|_| std::fs::write(&path, data));
        match result {
            Ok(()) => *written += 1,
            Err(e) => { eprintln!("nightjar: PyroWave dump to {} failed: {e}", path.display()); *written = PYROWAVE_DUMP_FRAMES; }
        }
        if *written == PYROWAVE_DUMP_FRAMES { eprintln!("nightjar: PyroWave dump: {PYROWAVE_DUMP_FRAMES} envelopes in {}", dir.display()); }
    }

    /// What a frame the PyroWave decoder could not turn into a picture costs the stream.
    fn failed(&mut self, failure: Failure) -> DecodeStatus {
        let counters = &self.shared.counters;
        match failure {
            Failure::Frame(error) => {
                eprintln!("nightjar: PyroWave: {error}");
                counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                DecodeStatus::NeedIdr
            }
            Failure::Loss(error) => {
                eprintln!("nightjar: PyroWave: {error}");
                counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                self.lost = true;
                DecodeStatus::NeedIdr
            }
            Failure::NoPlanes => {
                if counters.replaced.fetch_add(1, Ordering::Relaxed) == 0 { eprintln!("nightjar: PyroWave: every GPU plane set is waiting to be shown, dropping the frame (counted, not repeated)"); }
                DecodeStatus::Ok
            }
            Failure::Device(error) => {
                eprintln!("nightjar: PyroWave: {error}");
                counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                self.shared.notify(Notice::DecoderFailed(error));
                DecodeStatus::NeedIdr
            }
        }
    }

    /// PyroWave decodes on the GPU with the SDK, into planes on this device.
    pub fn with_gpu_decoder(mut self, device: Option<GpuDevice>) -> Self {
        self.gpu = device;
        self
    }

    /// PyroWave decodes the blocks of a frame while they arrive (`Settings::early_decode`).
    pub fn with_early_decode(mut self, early: bool) -> Self {
        self.early = early;
        self
    }

    /// PyroWave decodes a row-ordered frame in `slices` slices while it arrives (0 off).
    pub fn with_beam_race(mut self, slices: u32) -> Self {
        self.slices = slices;
        self
    }
}

/// The SDK's decoder (on the renderer's adapter on Windows), with a plane set per picture that can be waiting.
fn gpu_pyrowave(device: GpuDevice, setup: &VideoSetup, slots: usize) -> Result<super::pyrowave::Pyrowave, String> {
    #[cfg(windows)]
    {
        use windows::core::Interface;
        // SAFETY: The renderer owns the device for the whole stream; this only borrows it to AddRef.
        let device = unsafe { windows::Win32::Graphics::Direct3D11::ID3D11Device::from_raw_borrowed(&device.0) }
            .ok_or("the renderer has no Direct3D 11 device")?;
        let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
        super::pyrowave::GpuStream::new(device, setup.width as usize, setup.height as usize, chroma444, ten_bit, slots)
            .map(|stream| super::pyrowave::Pyrowave::Gpu(Box::new(stream)))
    }
    #[cfg(unix)]
    {
        // The SDK decodes on its own Vulkan device and the planes cross through CPU memory, so
        // the renderer's device is only the proof that the presenter can draw them.
        let _ = device;
        let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
        super::pyrowave::GpuStream::new(setup.width as usize, setup.height as usize, chroma444, ten_bit, slots)
            .map(|stream| super::pyrowave::Pyrowave::Gpu(Box::new(stream)))
    }
}

impl VideoDecoder for Decoder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.setup = Some(*setup);
        self.shared.fps_millihz.store(setup.fps.max(1) as u32 * 1000, Ordering::Relaxed);
        if setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            let (width, height) = (setup.width as usize, setup.height as usize);
            let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
            let created = match self.gpu {
                Some(device) => gpu_pyrowave(device, setup, self.shared.queue_limit + 2)
                    .map(|decoder| (decoder, "PyroWave (GPU decode, SDK)"))
                    .map_err(|error| Notice::DecoderFailed(format!("PyroWave GPU decoding is unavailable: {error}"))),
                None => super::pyrowave::Pyrowave::cpu(width, height, chroma444)
                    .map(|decoder| (decoder, "PyroWave (Rust, GPU wavelets)"))
                    .map_err(|error| Notice::DecoderFallback(error.into())),
            };
            return match created {
                Ok((mut decoder, name)) => {
                    if self.slices > 0 {
                        if let Err(error) = decoder.set_beam_race(self.slices) { eprintln!("nightjar: beam race: {error}"); }
                    }
                    self.pyrowave = Some(decoder);
                    *lock(&self.shared.decoder_name) = name.into();
                    Ok(())
                }
                Err(notice) => { self.shared.notify(notice); Err(-1) }
            };
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

    fn wants_early_chunks(&self) -> bool {
        self.early
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if let Some(decoder) = &mut self.pyrowave {
            let start = session::now_us();
            decoder.chunk(first, flags, payload);
            self.shared.push_slices(decoder.take_slices());
            self.shared.counters.early_us_total.fetch_add(session::now_us() - start, Ordering::Relaxed);
        }
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        let pictures = self.shared.counters.decoded.load(Ordering::Relaxed);
        self.lost = false;
        let status = self.decode_frame(frame);
        let decoded = self.shared.counters.decoded.load(Ordering::Relaxed) != pictures;
        self.after_frame(decoded, status == DecodeStatus::NeedIdr, session::now_us());
        status
    }

    fn cleanup(&mut self) {
        lock(&self.shared.frames).clear();
        self.decoder = None;
        if let Some(summary) = self.pyrowave.as_ref().and_then(|decoder| decoder.stage_summary()) {
            eprintln!("nightjar: PyroWave decode stages {summary}");
        }
        self.pyrowave = None;
    }
}

impl Decoder {
    /// Decodes `frame`, PyroWave or FFmpeg, and queues its picture for the presenter.
    fn decode_frame(&mut self, frame: &Frame) -> DecodeStatus {
        self.shared.counters.count_host_latency(frame.host_processing_latency);
        self.shared.counters.video_bytes.fetch_add(frame.data().len() as u64, Ordering::Relaxed);
        if self.pyrowave.is_some() {
            let start = session::now_us();
            let setup = self.setup.expect("decoder setup precedes frames");
            if self.dump.is_some() { self.dump_envelope(&setup, frame.data()); }
            let Some(decoder) = self.pyrowave.as_mut() else { return DecodeStatus::Ok };
            let shared = self.shared.clone();
            // The presenter's queue and the pacer hold plane sets: when none is free the oldest picture goes.
            let mut evict = || shared.evict_oldest();
            let result = decoder.frame(frame.data(), &mut evict);
            // Counted for frames that failed too, as before the GPU decoder.
            let early = self.shared.count_source(decoder.source());
            let output = match result {
                Ok(output) => output,
                Err(failure) => return self.failed(failure),
            };
            let ready_us = session::now_us();
            let counters = &self.shared.counters;
            counters.decoded.fetch_add(1, Ordering::Relaxed);
            counters.decode_us_total.fetch_add(ready_us - start, Ordering::Relaxed);
            let mut timing = lock(&self.shared.decode_window);
            timing.samples += 1;
            timing.queue_us += start.saturating_sub(frame.enqueue_time_us);
            if output.loss_percent >= 0.0 {
                timing.record_loss_milli += (output.loss_percent * 1000.0) as u64;
                timing.record_samples += 1;
            }
            drop(timing);
            self.shared.count_race(&output.race);
            let decoded = Decoded { picture: output.picture, setup, rtp: frame.rtp_timestamp, number: frame.number, receive_us: frame.receive_time_us,
                enqueue_us: frame.enqueue_time_us, input_us: start, ready_us, host_latency: frame.host_processing_latency,
                early, first_block_us: output.first_block_us, race: output.race, bands: output.bands, race_seq: output.race_seq };
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
                        picture: Picture::Frame(picture),
                        setup: self.setup.expect("decoder setup precedes frames"),
                        rtp: pts as u32,
                        number: frame.number,
                        receive_us: frame.receive_time_us,
                        enqueue_us: frame.enqueue_time_us,
                        input_us: started,
                        ready_us: ready,
                        host_latency: frame.host_processing_latency,
                        early: 0,
                        first_block_us: 0,
                        race: Default::default(),
                        bands: Vec::new(),
                        race_seq: 0,
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
}

/// What the stream's packets are.
pub(super) enum AudioDecoder {
    Opus(media::OpusDecoder),
    /// s16le samples, `channels` interleaved.
    Pcm { channels: usize },
}

impl AudioDecoder {
    pub(super) fn channels(&self) -> usize {
        match self {
            Self::Opus(decoder) => decoder.channels,
            Self::Pcm { channels } => *channels,
        }
    }

    /// Decodes `packet` into `pcm`, or conceals a lost one for all of `pcm`: Opus with its
    /// concealment, PCM with silence. Returns the samples per channel written.
    pub(super) fn decode(&mut self, packet: Option<&[u8]>, pcm: &mut [f32]) -> i32 {
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
pub fn passthrough_codecs(enabled: bool) -> u8 {
    if enabled { rubylight_protocol::audio_ext::CODEC_PCM } else { 0 }
}

/// Opus or PCM to an SDL audio stream, with the queue kept short. What runs dry between packets
/// is concealed from SDL's request callback (see [`Playout`]).
pub struct Audio {
    pub shared: Arc<Shared>,
    playout: Option<Arc<Mutex<Playout>>>,
    /// The passthrough codecs the stream asked for (`passthrough_codecs`).
    passthrough: u8,
    stream: *mut SDL_AudioStream,
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
            callback: std::ptr::null(),
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
        let stream = self.stream;
        // The stream's lock first, as SDL takes it before calling `conceal`.
        let _locked = StreamLock::new(stream);
        let mut playout = lock(playout);
        let channels = playout.channels();
        let bytes_per_sample = 4 * channels as i32;
        // SAFETY: as above.
        let queued = if stream.is_null() { 0 } else { unsafe { SDL_GetAudioStreamQueued(stream) }.max(0) };
        let fed = playout.packet(packet, (queued / bytes_per_sample) as usize);
        let counters = &self.shared.counters;
        let bytes_per_us = 48.0 * f64::from(bytes_per_sample) / 1000.0;
        counters.audio_queued_us.store((f64::from(queued) / bytes_per_us) as u64, Ordering::Relaxed);
        counters.audio_concealed.fetch_add(playout.take_concealed(), Ordering::Relaxed);
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
            Fed::Failed => return,
        };
        let pcm = playout.pcm();
        let mut routed = false;
        for headphones in lock(&self.shared.controller_audio).iter().filter_map(std::sync::Weak::upgrade) {
            routed |= lock(&headphones).headphones(&pcm[lead * channels..], channels);
        }
        if routed {
            playout.stop();
            if !stream.is_null() { unsafe { SDL_ClearAudioStream(stream) }; }
            counters.audio_queued_us.store(0, Ordering::Relaxed);
            return;
        }
        if stream.is_null() { return; }
        // SAFETY: the stream is open and locked; pcm is valid for the call.
        unsafe { SDL_PutAudioStreamData(stream, pcm.as_ptr().cast::<c_void>(), (pcm.len() * 4) as i32) };
    }

    fn cleanup(&mut self) {
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

pub struct Events {
    shared: Arc<Shared>,
    /// The connect attempt this listener belongs to.
    attempt: u32,
}

impl Events {
    /// The listener of the current connect attempt.
    pub fn new(shared: Arc<Shared>) -> Self {
        let attempt = shared.attempt.load(Ordering::SeqCst);
        Self { shared, attempt }
    }
}

impl Listener for Events {
    fn stage_starting(&self, stage: Stage) {
        self.shared.notify(Notice::Stage(stage));
    }

    fn connection_started(&self) {
        self.shared.notify(Notice::Started);
    }

    fn connection_terminated(&self, error: i32) {
        if !self.shared.notify_from(self.attempt, Notice::Terminated(error)) {
            eprintln!("nightjar: dropped the end ({error}) of an earlier connection attempt");
        }
    }

    fn control_event(&self, event: ControlEvent) {
        self.shared.notify(Notice::Control(event));
    }

    fn log(&self, message: &str) {
        eprintln!("nightjar: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoder() -> (Decoder, Arc<Shared>) {
        let shared = Shared::new(Arc::new(|| {}), 8);
        (Decoder::new(shared.clone(), Hardware::None), shared)
    }

    fn notices(shared: &Shared) -> Vec<Notice> {
        std::mem::take(&mut *lock(&shared.notices))
    }

    #[test]
    fn a_late_termination_of_a_failed_attempt_does_not_end_the_retry() {
        let shared = Shared::new(Arc::new(|| {}), 8);
        let failed = Events::new(shared.clone());
        shared.next_attempt();
        let retry = Events::new(shared.clone());
        // The failed attempt's termination thread runs after the retry began.
        failed.connection_terminated(5);
        assert!(notices(&shared).is_empty(), "a stale termination is dropped");
        retry.connection_terminated(6);
        assert!(matches!(notices(&shared).as_slice(), [Notice::Terminated(6)]));
    }

    #[test]
    fn every_received_frame_with_a_header_value_counts_for_the_host_processing_average() {
        // No decoder is set up, so nothing is decoded or shown: the count must not depend on that.
        let (mut decoder, shared) = decoder();
        for units in [5u16, 0, 6, 4] {
            let mut frame = Frame::default();
            frame.host_processing_latency = units;
            assert!(matches!(decoder.submit(&frame), DecodeStatus::Ok));
        }
        assert_eq!(shared.counters.host_latency_total.load(Ordering::Relaxed), 15);
        assert_eq!(shared.counters.host_latency_frames.load(Ordering::Relaxed), 3);
        assert_eq!(shared.counters.decoded.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_decoder_without_the_beam_race_hands_over_no_slices_and_wakes_nobody() {
        let woken = Arc::new(AtomicU64::new(0));
        let counter = woken.clone();
        let shared = Shared::new(Arc::new(move || { counter.fetch_add(1, Ordering::Relaxed); }), 8);
        let decoder = Decoder::new(shared.clone(), Hardware::None);
        assert_eq!(decoder.slices, 0, "the race is off unless a stream asks for slices");
        shared.push_slices(Vec::new());
        assert!(lock(&shared.slices).is_empty());
        assert_eq!(woken.load(Ordering::Relaxed), 0, "no slices, no wake-up");
    }

    #[test]
    fn chunks_through_a_decoder_with_the_beam_race_off_queue_no_slice_and_wake_nobody() {
        let woken = Arc::new(AtomicU64::new(0));
        let counter = woken.clone();
        let shared = Shared::new(Arc::new(move || { counter.fetch_add(1, Ordering::Relaxed); }), 8);
        let mut decoder = Decoder::new(shared.clone(), Hardware::None).with_early_decode(true);
        decoder.pyrowave = Some(super::super::pyrowave::Pyrowave::cpu(256, 128, false).expect("the CPU decoder"));
        let packet = [0u8; 64];
        for n in 0..8 {
            decoder.chunk(n == 0, 0, &packet);
        }
        assert!(lock(&shared.slices).is_empty(), "no slice is queued for the presenter");
        assert_eq!(woken.load(Ordering::Relaxed), 0, "and the presenter is not woken for one");
        assert!(decoder.pyrowave.as_mut().unwrap().take_slices().is_empty());
    }

    #[test]
    fn a_gpu_device_failure_ends_the_stream_with_its_message() {
        let (mut decoder, shared) = decoder();
        let status = decoder.failed(Failure::Device("PyroWave GPU decode failed: device removed".into()));
        assert!(matches!(status, DecodeStatus::NeedIdr));
        assert_eq!(shared.counters.decode_errors.load(Ordering::Relaxed), 1);
        let notices = notices(&shared);
        let [Notice::DecoderFailed(text)] = notices.as_slice() else { panic!("expected exactly the DecoderFailed notice") };
        assert_eq!(text, "PyroWave GPU decode failed: device removed");
        // The stream shows it as the reason it ended and does not reconnect on its own.
        assert!(!crate::stream::Exit::Ended(Some(text.clone())).retryable());
    }

    #[test]
    fn a_bad_frame_asks_for_an_idr_and_a_full_ring_drops_the_frame_without_ending_the_stream() {
        let (mut decoder, shared) = decoder();
        assert!(matches!(decoder.failed(Failure::Frame("bad".into())), DecodeStatus::NeedIdr));
        assert!(matches!(decoder.failed(Failure::NoPlanes), DecodeStatus::Ok));
        assert!(matches!(decoder.failed(Failure::NoPlanes), DecodeStatus::Ok));
        assert_eq!(shared.counters.decode_errors.load(Ordering::Relaxed), 1);
        assert_eq!(shared.counters.replaced.load(Ordering::Relaxed), 2);
        assert!(notices(&shared).is_empty(), "only a broken GPU decoder ends the stream");
    }

    #[test]
    fn early_and_fallback_frames_are_counted_whatever_the_frame_result() {
        // `submit` counts before it looks at the result, so a frame that failed after its blocks
        // were decoded early (or whole, after a mismatch) still shows in the counters, as in the
        // CPU path before the GPU decoder existed.
        use super::super::pyrowave::Source;
        let (_, shared) = decoder();
        assert_eq!(shared.count_source(Source::Early), 1);
        assert_eq!(shared.count_source(Source::Fallback), 2);
        assert_eq!(shared.count_source(Source::Fallback), 2);
        assert_eq!(shared.count_source(Source::Frame), 0);
        assert_eq!(shared.counters.early_frames.load(Ordering::Relaxed), 1);
        assert_eq!(shared.counters.early_fallbacks.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn evicting_drops_queued_pictures_oldest_first_then_the_pacers_so_the_decoder_is_never_starved() {
        use super::super::present::{tests::decoded, Pending};
        let (_, shared) = decoder();
        let pending = |present_us| Pending { decoded: decoded(present_us), present_us, hold_us: 0, late: false };
        assert!(!shared.evict_oldest(), "nothing queued: the caller reports NoPlanes");
        // HostTimes: the presenter's queue is empty and the pacer holds every picture.
        lock(&shared.pacer_backlog).extend([pending(1), pending(2), pending(3)]);
        lock(&shared.frames).push_back(decoded(10));
        assert!(shared.evict_oldest(), "the presenter's queue goes first");
        assert!(lock(&shared.frames).is_empty());
        assert_eq!(lock(&shared.pacer_backlog).len(), 3);
        assert!(shared.evict_oldest() && shared.evict_oldest());
        let left: Vec<u64> = lock(&shared.pacer_backlog).iter().map(|p| p.present_us).collect();
        assert_eq!(left, [3], "the oldest pacer pictures went, the newest stays");
        assert_eq!(shared.counters.replaced.load(Ordering::Relaxed), 3);
        // Even the last queued picture is older than the frame being decoded, so it may go.
        assert!(shared.evict_oldest() && !shared.evict_oldest());
    }

    #[test]
    fn frames_failing_for_a_second_open_the_decoder_again_once_then_end_the_stream() {
        let mut failing = Failing::default();
        assert_eq!(failing.failed(0, true), Escalation::Wait);
        assert_eq!(failing.failed(400_000, true), Escalation::Wait);
        assert_eq!(failing.failed(1_000_000, true), Escalation::Reopen);
        assert_eq!(failing.failed(1_100_000, true), Escalation::Wait, "the reopened decoder gets its own second");
        assert_eq!(failing.failed(1_600_000, true), Escalation::Wait);
        assert_eq!(failing.failed(2_100_000, true), Escalation::GiveUp);
        assert_eq!(failing.failed(3_000_000, true), Escalation::Wait, "said once");
        // A picture between failures starts over, and two failures a pause apart are not enough.
        let mut failing = Failing::default();
        failing.failed(0, true);
        failing.failed(500_000, true);
        failing.decoded();
        assert_eq!(failing.failed(1_200_000, true), Escalation::Wait);
        assert_eq!(failing.failed(6_000_000, true), Escalation::Wait);
        // A decoder that cannot be opened again ends the stream at once.
        let mut failing = Failing::default();
        failing.failed(0, false);
        failing.failed(500_000, false);
        assert_eq!(failing.failed(1_000_000, false), Escalation::GiveUp);
    }

    #[test]
    fn a_decoder_that_keeps_failing_is_opened_again_and_then_ends_the_stream_saying_why() {
        let (mut decoder, shared) = decoder();
        let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
        decoder.setup(&setup).unwrap();
        for at in [0, 500_000, 1_000_000] {
            decoder.frame_failed(at);
        }
        assert!(decoder.decoder.is_some(), "opened again");
        assert!(decoder.failing.reopened);
        assert!(notices(&shared).is_empty(), "the stream goes on");
        for at in [1_100_000, 1_600_000, 2_200_000] {
            decoder.frame_failed(at);
        }
        let ended = notices(&shared);
        let [Notice::DecoderFailed(text)] = ended.as_slice() else { panic!("expected exactly the DecoderFailed notice") };
        assert!(text.contains("stopped working"), "{text}");
        decoder.frame_failed(3_000_000);
        assert!(notices(&shared).is_empty(), "said once");
    }

    #[test]
    fn a_pyrowave_decoder_that_keeps_failing_ends_the_stream() {
        let (mut decoder, shared) = decoder();
        let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_PYROWAVE, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
        decoder.setup(&setup).unwrap();
        for at in [0, 500_000, 1_000_000] {
            decoder.frame_failed(at);
        }
        assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFailed(_)]));
    }

    #[test]
    fn pyrowave_frames_that_lost_their_data_never_end_the_stream() {
        let (mut decoder, shared) = decoder();
        let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_PYROWAVE, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
        decoder.setup(&setup).unwrap();
        // A loss burst of seconds on a bad link: an IDR is asked for each time, nothing more.
        for at in [0, 500_000, 1_000_000, 2_000_000, 5_000_000] {
            decoder.lost = false;
            assert!(matches!(decoder.failed(Failure::Loss("lost".into())), DecodeStatus::NeedIdr));
            decoder.after_frame(false, true, at);
        }
        assert!(notices(&shared).is_empty(), "the stream goes on");
        assert_eq!(decoder.failing, Failing::default());
        // Frames the decoder itself rejects still end it.
        for at in [6_000_000, 6_500_000, 7_000_000] {
            decoder.lost = false;
            decoder.failed(Failure::Frame("bad".into()));
            decoder.after_frame(false, true, at);
        }
        assert!(matches!(notices(&shared).as_slice(), [Notice::DecoderFailed(_)]));
    }

    #[test]
    fn gpu_mode_without_a_device_never_builds_a_cpu_decoder() {
        // No renderer device (Windows) or no SDK library next to the test executable (Linux, where
        // the planes cross through the CPU and the device only proves the renderer): the GPU
        // decoder cannot be built, and setup reports that instead of decoding on the CPU.
        let (mut decoder, shared) = decoder();
        decoder = decoder.with_gpu_decoder(Some(GpuDevice(std::ptr::null_mut())));
        let setup = VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_PYROWAVE, width: 128, height: 128, fps: 60, chroma_packing: 0, logical_width: 128, logical_height: 128, color_space: 1, color_range: 0 };
        assert!(decoder.setup(&setup).is_err());
        assert!(decoder.pyrowave.is_none());
        let notices = notices(&shared);
        assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("PyroWave GPU decoding is unavailable")));
        #[cfg(unix)]
        assert!(matches!(notices.as_slice(), [Notice::DecoderFailed(text)] if text.contains("libpyrowave-shared.so not found")), "the reason is the missing library");
    }
}
