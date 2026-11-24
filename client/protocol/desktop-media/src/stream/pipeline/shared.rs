//! Shared stream pipeline state exchanged between worker threads and the UI thread:
//! decoded picture queues, UI notices, atomic counters, GPU device handles and session events.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nightjar_core::control::ControlEvent;
use nightjar_core::session::{Listener, Stage, VideoSetup};
use crate::hosts::Waker;
use crate::media::Picture;
use super::{lock, GuardedList};

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
    pub race: super::super::pyrowave::RaceReport,
    /// The slices decoded for the frame, in order, each ready once its fence value is signalled
    /// (empty unless the beam race is on). The last runs to the end of the picture.
    pub bands: Vec<super::super::pyrowave::BandReady>,
    /// The sequence number the frame's slices came with (`SliceReady::seq`), 0 when it had none.
    pub race_seq: u64,
    /// The part of the picture that is the stream, without an encoder's alignment padding, as
    /// `Streaming::pump` works it out when the picture arrives; None for the whole picture.
    pub visible: Option<(i32, i32)>,
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
    /// The microphone runs, without the echo cancellation its setting asked for; the text says why.
    EchoCancelOff(String),
    /// Audio passthrough was asked for, but the PC sends Opus; the stream plays on with a message.
    PassthroughOpus,
    Clipboard(Result<Option<String>, String>),
    /// An image sent to the PC (`None`) or taken from it, ready for the clipboard here; the
    /// error is the text to show.
    ClipboardImage(Result<Option<crate::clipboard::Prepared>, String>),
    ServerCommand(Result<Vec<u8>, String>),
    /// The host's answer to opening its menu; the error is the text to show.
    HostMenu(Result<(), String>),
    /// The PC's night mode and headphone surround, read or changed from the stream controls.
    HostAudio(super::super::menu_extras::AudioAnswer),
    /// The guests of this device's game, read or changed.
    Guests(super::super::guests::GuestsAnswer),
    Bitrate { requested: u32, result: Result<u32, String> },
    AdaptiveBitrate(Result<(u32, u32), String>),
    /// A Foundation PC turned its own bitrate controller on; None when it has none.
    HostAbr(Result<Option<nightjar_core::http::foundation::AbrConfigured>, String>),
    /// A Foundation PC's answer to a network report; None when the report failed.
    HostAbrAction(Option<nightjar_core::http::foundation::AbrAction>),
    /// A step of the stream doctor's bitrate trial came back.
    HealthTrial(super::super::doctor::TrialOutcome),
    /// The network watcher saw this device change networks; true when it came back after the
    /// route toward the PC was gone (`NetworkLost`).
    NetworkChanged(nightjar_core::session::NetworkHint, bool),
    /// The network watcher lost the route toward the PC.
    NetworkLost,
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
    pub slices: Mutex<VecDeque<super::super::pyrowave::SliceReady>>,
    /// The pacer's queue (`Pacer::with_backlog`), reachable from the decoder thread for `evict_oldest`.
    pub pacer_backlog: Arc<Mutex<VecDeque<super::super::present::Pending>>>,
    pub notices: Mutex<Vec<Notice>>,
    /// Why the last failed attempt of an automatic recovery failed, for the message when the
    /// recovery gives up (`connect::recovery_gave_up`).
    pub last_failure: Mutex<Option<String>>,
    /// The connect attempt whose listener may still end the stream; changed only with
    /// `notices` locked, so a late termination of an earlier attempt is dropped, not queued.
    attempt: AtomicU32,
    pub decoder_name: Mutex<String>,
    /// The coded size of a packed full chroma stream whose pictures are rebuilt from memory
    /// (`packed_from_memory`), for the stream doctor.
    pub packed_from_memory: Mutex<Option<(i32, i32)>>,
    pub controller_audio: GuardedList<std::sync::Weak<Mutex<crate::controller_audio::Playback>>>,
    pub counters: Counters,
    /// Most pictures held before the oldest is dropped.
    pub queue_limit: usize,
    /// The pacer schedules by host times ("PC's frame times") and holds pictures until due.
    pub host_times: std::sync::atomic::AtomicBool,
    /// Client roaming's network watcher while a session runs; dropping it stops it.
    pub roam_watch: Mutex<Option<crate::network::RoamWatch>>,
    /// Sessions whose connect attempt stopped for them (a recovery that gave up, a launch that
    /// waited too long for the previous session): the app takes these and joins them before the
    /// renderer goes.
    pub teardowns: Mutex<Vec<super::super::Teardown>>,
    /// The toast that the microphone runs without the echo cancellation asked for was shown.
    pub echo_cancel_noted: std::sync::atomic::AtomicBool,
    /// When the last of the pictures in a row the presenter could not show was ready (session
    /// clock, us; 0 for none): the decoder that made it gives way (`Decoder::presenter_refused`).
    pub present_refused: AtomicU64,
    /// The stream ends into a reconnect (`Streaming::keep_decoder_for_reconnect`): the decoder's
    /// device and codec context are kept for the session that follows it, when they serve it
    /// (`Decoder::cleanup`).
    pub keep_decoder: std::sync::atomic::AtomicBool,
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
            last_failure: Mutex::new(None),
            attempt: AtomicU32::new(0),
            decoder_name: Mutex::new(String::new()),
            packed_from_memory: Mutex::new(None),
            controller_audio: GuardedList::new(),
            counters: Counters::default(),
            queue_limit,
            host_times: std::sync::atomic::AtomicBool::new(false),
            roam_watch: Mutex::new(None),
            teardowns: Mutex::new(Vec::new()),
            echo_cancel_noted: std::sync::atomic::AtomicBool::new(false),
            present_refused: AtomicU64::new(0),
            keep_decoder: std::sync::atomic::AtomicBool::new(false),
            waker,
        })
    }

    /// Frees a GPU plane set when the decoder has none: the oldest picture still waiting for the
    /// presenter goes, first from the pacer (which, scheduling by host times, can hold many
    /// pictures that are not due yet, all older than those in `frames`), then from `frames`. The
    /// frame being decoded is not queued yet, so it is never the one dropped. True if a picture
    /// was dropped.
    pub fn evict_oldest(&self) -> bool {
        let dropped = lock(&self.pacer_backlog).pop_front().is_some() || lock(&self.frames).pop_front().is_some();
        if dropped { self.counters.replaced.fetch_add(1, Ordering::Relaxed); }
        dropped
    }

    /// Counts how the frame's blocks were decoded (see `Decoded::early`); returns the `early` code.
    pub(super) fn count_source(&self, source: super::super::pyrowave::Source) -> u8 {
        match source {
            super::super::pyrowave::Source::Frame => 0,
            super::super::pyrowave::Source::Early => { self.counters.early_frames.fetch_add(1, Ordering::Relaxed); 1 }
            super::super::pyrowave::Source::Fallback => {
                if self.counters.early_fallbacks.fetch_add(1, Ordering::Relaxed) == 0 {
                    eprintln!("nightjar: PyroWave: early chunks did not match the frame, decoding it whole (counted, not repeated)");
                }
                2
            }
        }
    }

    /// Counts a frame's beam race (see `RaceReport`); a frame that arrived out of row order says
    /// so once on stderr and always in the counters and the CSV.
    pub(super) fn count_race(&self, race: &super::super::pyrowave::RaceReport) {
        use super::super::pyrowave::RaceReport;
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

    /// Queues a picture holding a surface of the hardware decoder's fixed pool, which has spares
    /// for `limit` waiting pictures ([`super::ffmpeg::pool_waiting`]). Should more wait all the same, the pacer's
    /// oldest held picture is made due now, so the presenter shows it early and frees its surface
    /// instead of the decoder failing the next frame. With every pacer picture due already, the
    /// pacer's oldest goes; with none in the pacer, the oldest queued one.
    pub(super) fn queue_pooled(&self, decoded: Decoded, limit: usize) {
        {
            // Both locks, so a picture moving from one queue to the other is counted once.
            let mut frames = lock(&self.frames);
            let mut pacer = lock(&self.pacer_backlog);
            if frames.len() + pacer.len() >= limit {
                if pacer.is_empty() {
                    frames.pop_front();
                    self.counters.replaced.fetch_add(1, Ordering::Relaxed);
                } else if let Some(held) = pacer.iter_mut().find(|p| p.present_us > p.decoded.ready_us) {
                    held.present_us = held.decoded.ready_us;
                    held.hold_us = 0;
                } else {
                    // Every pacer picture is due already and the presenter has not caught up:
                    // making one due frees nothing, so the oldest goes.
                    pacer.pop_front();
                    self.counters.replaced.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.queue(decoded);
    }

    /// The pictures waiting for the presenter, in its queue and in the pacer.
    #[cfg(test)]
    pub(super) fn waiting(&self) -> usize {
        let frames = lock(&self.frames);
        frames.len() + lock(&self.pacer_backlog).len()
    }

    /// Queues a picture for the presenter, which loses the oldest past the queue's limit, and wakes
    /// it when the queue was empty. A picture that joins others needs no wake of its own: the
    /// presenter takes the whole queue under this lock (`Streaming::pump`) after each wake it
    /// gets, and the wake of the picture that found the queue empty is sent (below) after that
    /// picture is in it, so it takes this one too. A wake that comes after the presenter already
    /// took the picture only finds a queue to pump that is empty.
    pub(super) fn queue(&self, decoded: Decoded) {
        let mut frames = lock(&self.frames);
        let was_empty = frames.is_empty();
        frames.push_back(decoded);
        while frames.len() > self.queue_limit {
            frames.pop_front();
            self.counters.replaced.fetch_add(1, Ordering::Relaxed);
        }
        drop(frames);
        if was_empty {
            self.wake();
        }
    }

    /// Hands slices that finished decoding to the presenter and wakes it. The queue is bounded: a
    /// presenter that does not keep up loses the oldest, which it would have dropped as stale.
    pub(super) fn push_slices(&self, slices: Vec<super::super::pyrowave::SliceReady>) {
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
