//! The parts the session drives on its own threads: the video decoder, the audio renderer
//! and the connection listener. They hand decoded pictures, notices and control events to
//! the UI thread through [`Shared`].

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nightjar_core::control::ControlEvent;
use nightjar_core::sdp::{VIDEO_FORMAT_MASK_10BIT, VIDEO_FORMAT_MASK_AV1, VIDEO_FORMAT_MASK_H265, VIDEO_FORMAT_MASK_PYROWAVE, VIDEO_FORMAT_MASK_YUV444};
use nightjar_core::session::{
    self, AudioRenderer, AudioSetup, DecodeStatus, Listener, Stage, VideoDecoder, VideoSetup,
};
use nightjar_core::video::Frame;
use sdl3_sys::everything::*;

use crate::hosts::Waker;
use crate::media::{self, Codec, Hardware, Picture, Received};
use crate::strings::trf;
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
    HostAudio(super::menu_extras::AudioAnswer),
    /// The guests of this device's game, read or changed.
    Guests(super::guests::GuestsAnswer),
    Bitrate { requested: u32, result: Result<u32, String> },
    AdaptiveBitrate(Result<(u32, u32), String>),
    /// A Foundation PC turned its own bitrate controller on; None when it has none.
    HostAbr(Result<Option<nightjar_core::http::foundation::AbrConfigured>, String>),
    /// A Foundation PC's answer to a network report; None when the report failed.
    HostAbrAction(Option<nightjar_core::http::foundation::AbrAction>),
    /// A step of the stream doctor's bitrate trial came back.
    HealthTrial(super::doctor::TrialOutcome),
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
    pub slices: Mutex<VecDeque<super::pyrowave::SliceReady>>,
    /// The pacer's queue (`Pacer::with_backlog`), reachable from the decoder thread for `evict_oldest`.
    pub pacer_backlog: Arc<Mutex<VecDeque<super::present::Pending>>>,
    pub notices: Mutex<Vec<Notice>>,
    /// The connect attempt whose listener may still end the stream; changed only with
    /// `notices` locked, so a late termination of an earlier attempt is dropped, not queued.
    attempt: AtomicU32,
    pub decoder_name: Mutex<String>,
    /// The coded size of a packed full chroma stream whose pictures are rebuilt from memory
    /// (`packed_from_memory`), for the stream doctor.
    pub packed_from_memory: Mutex<Option<(i32, i32)>>,
    pub controller_audio: Mutex<Vec<std::sync::Weak<Mutex<crate::controller_audio::Playback>>>>,
    pub counters: Counters,
    /// Most pictures held before the oldest is dropped.
    pub queue_limit: usize,
    /// The pacer schedules by host times ("PC's frame times") and holds pictures until due.
    pub host_times: std::sync::atomic::AtomicBool,
    /// Client roaming's network watcher while a session runs; dropping it stops it.
    pub roam_watch: Mutex<Option<crate::network::RoamWatch>>,
    /// The toast that the microphone runs without the echo cancellation asked for was shown.
    pub echo_cancel_noted: std::sync::atomic::AtomicBool,
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
            packed_from_memory: Mutex::new(None),
            controller_audio: Mutex::new(Vec::new()),
            counters: Counters::default(),
            queue_limit,
            host_times: std::sync::atomic::AtomicBool::new(false),
            roam_watch: Mutex::new(None),
            echo_cancel_noted: std::sync::atomic::AtomicBool::new(false),
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

    /// Queues a picture holding a surface of the hardware decoder's fixed pool, which has spares
    /// for `limit` waiting pictures ([`pool_waiting`]). Should more wait all the same, the pacer's
    /// oldest held picture is made due now, so the presenter shows it early and frees its surface
    /// instead of the decoder failing the next frame; only with no picture in the pacer does the
    /// oldest queued one go.
    fn queue_pooled(&self, decoded: Decoded, limit: usize) {
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
                }
            }
        }
        self.queue(decoded);
    }

    /// The pictures waiting for the presenter, in its queue and in the pacer.
    #[cfg(test)]
    fn waiting(&self) -> usize {
        let frames = lock(&self.frames);
        frames.len() + lock(&self.pacer_backlog).len()
    }

    /// Queues a picture for the presenter, which loses the oldest past the queue's limit, and wakes it.
    fn queue(&self, decoded: Decoded) {
        let mut frames = lock(&self.frames);
        frames.push_back(decoded);
        while frames.len() > self.queue_limit {
            frames.pop_front();
            self.counters.replaced.fetch_add(1, Ordering::Relaxed);
        }
        drop(frames);
        self.wake();
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
    /// The presenter imports VAAPI pictures as DMA-BUFs (`crate::dmabuf`).
    dmabuf: bool,
    /// The pictures go straight onto a display plane (`kms_scanout`), which takes 10-bit too.
    kms_planes: bool,
    /// Measurement: where the first `PYROWAVE_DUMP_FRAMES` envelopes go (`NIGHTJAR_PYROWAVE_DUMP`),
    /// and how many were written.
    dump: Option<(std::path::PathBuf, u32)>,
    /// Frames that failed since the last picture.
    failing: Failing,
    /// The frame being submitted failed for data lost on the way (`Failure::Loss`).
    lost: bool,
    /// Pictures from a hardware pool that may wait for the presenter ([`pool_waiting`]).
    pool_waiting: usize,
    /// The times of the frames sent to the FFmpeg decoder whose pictures have not come out, in
    /// order: an asynchronous decoder's picture belongs to an earlier frame than the one sent.
    in_flight: VecDeque<InFlight>,
    /// The FFmpeg decoder made a picture since it was opened: from then on a frame without one
    /// is a decode failure (synchronous decoders).
    pictured: bool,
    /// A packed full chroma stream's VAAPI pictures are mapped into Vulkan: a failed import in
    /// `receive` drops the map, and the doctor learns the pictures go through memory from then.
    packed_mapped: bool,
    /// The stream's VAAPI pictures are mapped into the GPU renderer's Vulkan images: a failed
    /// import in `receive` drops the map, and Vulkan video takes the stream over where it can
    /// (`vulkan_for_unmapped`).
    vaapi_mapped: bool,
    /// The stream format Vulkan video decodes in place of a VAAPI decoder whose pictures could
    /// not be mapped: should FFmpeg take it to software, VAAPI through memory is next, not software.
    stands_in: Option<u32>,
    /// The backends FFmpeg took to software for a stream format (`VideoSetup::format`), left
    /// out when the decoder is opened again for it.
    refused: Vec<(Hardware, u32)>,
}

/// The times of a frame sent to the FFmpeg decoder, for its picture.
#[derive(Clone, Copy, Debug, PartialEq)]
struct InFlight {
    number: u32,
    receive_us: u64,
    enqueue_us: u64,
    input_us: u64,
    host_latency: u16,
}

/// More frames in flight than any decoder holds: the oldest are frames that made no picture.
const IN_FLIGHT_LIMIT: usize = 32;

/// The times of the frame `number` from `in_flight`, dropping those of earlier frames, which
/// made no picture; none when it is not there (then the picture is taken for the latest frame's).
fn take_in_flight(in_flight: &mut VecDeque<InFlight>, number: u32) -> Option<InFlight> {
    while let Some(front) = in_flight.front() {
        // Frame numbers wrap: an earlier frame is less than half the range behind.
        let behind = number.wrapping_sub(front.number) as i32;
        if behind < 0 {
            return None;
        }
        let front = in_flight.pop_front()?;
        if behind == 0 {
            return Some(front);
        }
    }
    None
}

/// What the FFmpeg part of `decode_frame` needs of a decoder, so that a fake one stands in for
/// it in the tests.
trait PictureDecoder {
    type Picture: DecodedPicture;
    fn send(&mut self, data: &[u8], pts: i64) -> i32;
    fn receive(&mut self) -> Received<Self::Picture>;
    fn asynchronous(&self) -> bool;
}

trait DecodedPicture {
    fn pts(&self) -> i64;
    fn corrupt(&self) -> bool;
}

impl PictureDecoder for media::VideoDecoder {
    type Picture = media::Frame;
    fn send(&mut self, data: &[u8], pts: i64) -> i32 {
        media::VideoDecoder::send(self, data, pts)
    }
    fn receive(&mut self) -> Received {
        media::VideoDecoder::receive(self)
    }
    fn asynchronous(&self) -> bool {
        media::VideoDecoder::asynchronous(self)
    }
}

impl DecodedPicture for media::Frame {
    fn pts(&self) -> i64 {
        self.info().pts
    }
    fn corrupt(&self) -> bool {
        media::Frame::corrupt(self)
    }
}

/// What one frame did in the FFmpeg decoder ([`feed`]).
#[derive(Debug, Default, PartialEq)]
struct FedFrame {
    /// The decoder did not take the frame, after its ready pictures were read on EAGAIN.
    refused: bool,
    /// `avcodec_receive_frame` failed.
    failed: bool,
    /// Pictures that came out, this frame's or (asynchronous decoders) earlier ones'.
    pictures: u32,
    /// Of them, pictures FFmpeg flagged as damaged.
    corrupt: u32,
    /// This frame's own picture came out.
    own: bool,
}

/// How often an asynchronous decoder is asked again for the picture of the frame just sent.
const ASYNC_POLL_US: u64 = 250;

/// How long an asynchronous decoder is waited for: half a frame at `fps_millihz`, so the wait
/// never holds up the next frame, and no more than 8 ms.
fn async_wait_us(fps_millihz: u32) -> u64 {
    (500_000_000 / u64::from(fps_millihz.max(1))).min(8_000)
}

/// Sends a frame (`pts`, its number in the top 32 bits) to `decoder` and hands each picture that
/// comes out to `picture`. A decoder that says EAGAIN has its ready pictures read and gets the
/// frame again. An asynchronous decoder (V4L2 M2M) is asked again every [`ASYNC_POLL_US`]
/// (`poll` waits that long) until this frame's picture comes out, as moonlight-qt's decode loop
/// does, until `wait_us` after the send by `elapsed_us` (microseconds since the call), one
/// deadline for reading after EAGAIN and for the frame's own picture; what is still in flight
/// then comes out with a later frame.
fn feed<D: PictureDecoder>(
    decoder: &mut D,
    data: &[u8],
    pts: i64,
    wait_us: u64,
    elapsed_us: impl Fn() -> u64,
    mut poll: impl FnMut(),
    mut picture: impl FnMut(D::Picture),
) -> FedFrame {
    let number = (pts >> 32) as u32;
    let asynchronous = decoder.asynchronous();
    let mut fed = FedFrame::default();
    let mut receive = |decoder: &mut D, fed: &mut FedFrame, done: &dyn Fn(&FedFrame) -> bool| {
        loop {
            match decoder.receive() {
                Received::Frame(out) => {
                    // Frame numbers wrap: this frame's picture or a later one.
                    fed.own |= (number.wrapping_sub((out.pts() >> 32) as u32) as i32) <= 0;
                    fed.corrupt += u32::from(out.corrupt());
                    fed.pictures += 1;
                    picture(out);
                }
                Received::None if asynchronous && !done(fed) && elapsed_us() < wait_us => poll(),
                Received::None => return,
                Received::Error => {
                    fed.failed = true;
                    return;
                }
            }
        }
    };
    let mut status = decoder.send(data, pts);
    if status == media::AGAIN {
        // Output first: what is ready goes on (one picture at least from an asynchronous
        // decoder, whose input queue is full), then the frame again.
        receive(decoder, &mut fed, &|fed| fed.pictures > 0);
        if fed.failed {
            return fed;
        }
        status = decoder.send(data, pts);
    }
    if status < 0 {
        fed.refused = true;
        return fed;
    }
    receive(decoder, &mut fed, &|fed| fed.own);
    fed
}

/// What a frame `fed` to a decoder costs the stream. A synchronous decoder (every one but V4L2
/// M2M) makes the picture of each frame it is sent, so once it has made one (`steady`), a frame
/// without a picture failed as much as a damaged picture did: the stream has no periodic key
/// frames, and without recovery the damage stays. An asynchronous one's pictures come later.
fn frame_status(fed: &FedFrame, asynchronous: bool, steady: bool) -> DecodeStatus {
    let damaged = !asynchronous && (fed.corrupt > 0 || (steady && fed.pictures == 0));
    if fed.refused || fed.failed || damaged {
        DecodeStatus::NeedIdr
    } else {
        DecodeStatus::Ok
    }
}

/// Whether the FFmpeg decoder now decoding (`backend`, none for PyroWave) recovers with
/// reference frame invalidation: a launch asks for it only by backends that do
/// (`connect::takes_rfi`), so one a refusal brought in mid-stream (Vulkan video, V4L2 M2M,
/// NVDEC) has the rest of the stream recover with key frames.
fn recovers_with_rfi(backend: Option<Hardware>) -> bool {
    backend.is_none_or(super::connect::takes_rfi)
}

/// FFmpeg took to software the Vulkan video decoder (`refused`) that stood in for a VAAPI decoder
/// whose pictures could not be mapped (`stands_in`, the stream format it did so for): VAAPI
/// decodes that stream, through memory.
pub(super) fn stood_in_for_vaapi(refused: Hardware, stands_in: Option<u32>, format: u32) -> bool {
    matches!(refused, Hardware::Vulkan(_)) && stands_in == Some(format)
}

/// The backends of `order` after `refused`, which FFmpeg took to software: none when it is not
/// in the order.
pub(super) fn backends_after(order: Vec<Hardware>, refused: Hardware) -> Vec<Hardware> {
    match order.iter().position(|&backend| backend == refused) {
        Some(at) => order[at + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// A stream's decoder, for whether Vulkan video takes it over ([`vulkan_for_unmapped`]).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Unmapped {
    /// The stream's decoder is VAAPI's.
    pub vaapi: bool,
    /// Its pictures reach the presenter without crossing memory (mapped, or as DMA-BUFs).
    pub zero_copy: bool,
    /// Packed full chroma, which Vulkan video leaves to the next decoder.
    pub packed: bool,
    /// `NIGHTJAR_VAAPI_VULKAN_MAP` is not 0.
    pub map_switched_on: bool,
    /// `NIGHTJAR_UNMAPPED_VULKAN_VIDEO`, where it is set.
    pub switch: Option<bool>,
}

/// The Vulkan video backend to open in place of a VAAPI decoder whose pictures would be read back
/// and uploaded, one by one (nightjar#80: 2.5 GB/s at 4K100 10-bit; on an Iris Plus 655 a 4K
/// 10-bit picture took 40 ms to decode and map and 9.6 ms to draw that way, against 1.3 ms and
/// 2.4 ms with Vulkan video, rust/PERFORMANCE.md, November 12):
/// the GPU renderer's, where the stream's `order` has it after VAAPI (so not where it was tried
/// first, nor without the GPU renderer, whose device is null) and FFmpeg did not take it to
/// software for this stream before (`refused`). Vulkan video decodes into images of the renderer's
/// device, so nothing is copied; it asks for a key frame where VAAPI invalidates reference frames,
/// unless "Vulkan video recovery" is on. `NIGHTJAR_VAAPI_VULKAN_MAP=0` alone asks for memory.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn vulkan_for_unmapped(now: Unmapped, order: Vec<Hardware>, refused: impl Fn(Hardware) -> bool) -> Option<Hardware> {
    let asked_for_memory = now.switch == Some(false) || (!now.map_switched_on && now.switch != Some(true));
    if !now.vaapi || now.zero_copy || now.packed || asked_for_memory {
        return None;
    }
    backends_after(order, Hardware::Vaapi).into_iter().find(|&backend| matches!(backend, Hardware::Vulkan(gpu) if !gpu.is_null()) && !refused(backend))
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

/// Pictures from a hardware decoder's pool that may wait for the presenter, which the pool gets
/// spare surfaces for ([`media::pool_extra`]). Pacing by host times, the pacer holds every
/// picture until due: the clock raises its hold up to `max_hold_us` (50 ms) on a lateness spike
/// and lowers it over about half a second, 13 pictures at 240 FPS, and the presenter's queue
/// holds up to `queue_limit` more. Otherwise a picture waits a frame at most.
fn pool_waiting(host_times: bool, queue_limit: usize, fps: u32) -> usize {
    let usual = media::EXTRA_HW_FRAMES as usize - 2;
    if !host_times {
        return usual;
    }
    let frame_us = (1_000_000 / i64::from(fps.max(1))).max(1);
    let max_hold_us = rubylight_protocol::present_clock::Settings::default().max_hold_us.max(0);
    let held = (max_hold_us + frame_us - 1) / frame_us + 1;
    (held as usize + queue_limit).max(usual)
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
            dmabuf: false,
            kms_planes: false,
            dump: std::env::var_os("NIGHTJAR_PYROWAVE_DUMP").filter(|dir| !dir.is_empty()).map(|dir| (dir.into(), 0)),
            failing: Failing::default(),
            lost: false,
            pool_waiting: media::EXTRA_HW_FRAMES as usize - 2,
            in_flight: VecDeque::new(),
            pictured: false,
            packed_mapped: false,
            vaapi_mapped: false,
            stands_in: None,
            refused: Vec::new(),
        }
    }

    /// A frame failed with no picture since the last one: after [`FAILING_FOR_US`] of that the
    /// FFmpeg decoder is opened again once. When it still fails, a hardware decoder that made no
    /// picture since gives way to the next decoder; otherwise, or when PyroWave fails, the stream
    /// ends saying why instead of freezing.
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
            Escalation::GiveUp if self.never_pictured_hardware().is_some() => {
                // A hardware decoder that made no picture since it was opened again (V4L2 M2M on
                // an Odin 2 Portal that cannot set up its capture buffers, issue 77): the next
                // decoder, software after the last, takes over instead of the stream ending.
                let backend = self.never_pictured_hardware().expect("checked above");
                eprintln!("nightjar: {} failed {frames} frames in a row over {for_ms} ms after it was opened again, without a picture; trying the next decoder", backend.label());
                self.replace_refused(backend);
                // The next decoder gets its own second and its own reopen.
                self.failing = Failing::default();
            }
            Escalation::GiveUp => {
                eprintln!("nightjar: the video decoder failed {frames} frames in a row over {for_ms} ms with no picture; ending the stream");
                self.shared.notify(Notice::DecoderFailed(format!("The video decoder stopped working: no picture for {} s.", for_ms.div_ceil(1000))));
            }
        }
    }

    /// The stream's hardware decoder when it has made no picture since it was opened.
    fn never_pictured_hardware(&self) -> Option<Hardware> {
        self.decoder.as_ref().map(|decoder| decoder.hardware).filter(|&backend| backend != Hardware::None && !self.pictured)
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

    /// VAAPI pictures go to the presenter as DMA-BUFs, which it imports into its textures.
    pub fn with_dmabuf(mut self, dmabuf: bool) -> Self {
        self.dmabuf = dmabuf;
        self
    }

    /// VAAPI pictures go straight onto a display plane as DMA-BUFs (`kms_scanout`).
    pub fn with_kms_planes(mut self, kms_planes: bool) -> Self {
        self.kms_planes = kms_planes;
        self
    }
}

/// Whether a stream's VAAPI pictures go to the presenter as DMA-BUFs: it imports them, and they
/// are what its import takes (SDL's NV12 texture: 8-bit 4:2:0, not packed). A display plane
/// (`kms_planes`) takes them as they are, P010 included.
fn exports_dmabuf(presenter_imports: bool, kms_planes: bool, setup: &VideoSetup) -> bool {
    let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
    (presenter_imports || kms_planes)
        && (kms_planes || !ten_bit)
        && setup.format & VIDEO_FORMAT_MASK_YUV444 == 0
        && setup.chroma_packing == 0
}

/// A packed full chroma stream's coded size where its pictures are rebuilt from memory: on Linux
/// they never go as DMA-BUFs (`exports_dmabuf`), so unless they reach the GPU renderer as Vulkan
/// images (`on_gpu`: mapped VAAPI pictures or Vulkan video's), every picture, twice the stream's
/// height, is copied through the CPU (#80: 3840x4320 at 100 fps fell behind).
fn packed_from_memory(setup: &VideoSetup, on_gpu: bool) -> Option<(i32, i32)> {
    (cfg!(unix) && setup.chroma_packing != 0 && !on_gpu).then_some((setup.width, setup.height))
}

/// The coded size a packed stream's pictures go through memory at once a picture's Vulkan import
/// failed: the decoder dropped the map it had at install (`was_mapped`, not `still_mapped`).
fn packed_map_dropped(setup: &VideoSetup, was_mapped: bool, still_mapped: bool) -> Option<(i32, i32)> {
    if was_mapped && !still_mapped { packed_from_memory(setup, false) } else { None }
}

/// The toast for NVDEC pictures that go through memory to the GPU renderer because CUDA's copy into
/// its Vulkan images could not be set up or failed; the reason, in English, is in the log.
const CUDA_FALLBACK_TOAST: &str = crate::strings::en("NVDEC pictures can’t be copied into the renderer’s images on the graphics card, so each one goes through memory, which takes longer. The log says why.");

/// Says once in a toast when `decoder`'s NVDEC pictures went through memory instead of CUDA's copy.
fn say_cuda_fallback(shared: &Shared, decoder: &mut media::VideoDecoder) {
    if decoder.take_cuda_fallback().is_some() {
        shared.notify(Notice::DecoderFallback(crate::strings::tr(CUDA_FALLBACK_TOAST).to_owned()));
    }
}

/// Maps `decoder`'s VAAPI pictures into the GPU renderer's Vulkan images when it draws with SDL's
/// GPU API (`media::VideoDecoder::map_to_vulkan`); whether they are.
pub(super) fn map_to_vulkan(hardware: Hardware, decoder: &mut media::VideoDecoder) -> bool {
    #[cfg(target_os = "linux")]
    if let Hardware::Auto(gpu, _) = hardware {
        return !gpu.is_null() && decoder.map_to_vulkan(gpu);
    }
    let _ = (hardware, decoder);
    false
}

fn codec_of(setup: &VideoSetup) -> Codec {
    if setup.format & VIDEO_FORMAT_MASK_AV1 != 0 {
        Codec::Av1
    } else if setup.format & VIDEO_FORMAT_MASK_H265 != 0 {
        Codec::Hevc
    } else {
        Codec::H264
    }
}

/// Each backend that does not open is logged as the next one is tried.
fn log_unavailable(backend: Hardware, error: &str) {
    eprintln!("nightjar: {} decoding is unavailable: {error}", backend.label());
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
    #[cfg(target_os = "macos")]
    {
        let _ = (device, setup, slots);
        Err("The PyroWave GPU SDK has no macOS backend.".into())
    }
    #[cfg(target_os = "linux")]
    {
        // The SDK decodes on the renderer's Vulkan device where it can share it, else on its own
        // with the planes crossing through CPU memory (`GpuDecoder::for_renderer`).
        let ten_bit = setup.format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
        let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
        super::pyrowave::GpuStream::new(device.0, setup.width as usize, setup.height as usize, chroma444, ten_bit, slots)
            .map(|stream| super::pyrowave::Pyrowave::Gpu(Box::new(stream)))
    }
}

impl VideoDecoder for Decoder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.setup = Some(*setup);
        self.shared.fps_millihz.store(setup.fps.max(1) as u32 * 1000, Ordering::Relaxed);
        self.pool_waiting = pool_waiting(self.shared.host_times.load(Ordering::Relaxed), self.shared.queue_limit,
            setup.fps.max(1) as u32);
        if setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            let (width, height) = (setup.width as usize, setup.height as usize);
            let chroma444 = setup.format & VIDEO_FORMAT_MASK_YUV444 != 0;
            let created = match self.gpu {
                Some(device) => gpu_pyrowave(device, setup, {
                    // The presenter holds the shown picture's plane set while it draws the planes into the back buffer.
                    #[cfg(windows)]
                    { super::direct_planes::plane_sets(self.shared.queue_limit) }
                    #[cfg(not(windows))]
                    { self.shared.queue_limit + 2 }
                })
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
        let codec = codec_of(setup);
        let opened = match self.hardware {
            Hardware::None => {
                media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false)
            }
            hardware => {
                // Backends FFmpeg took to software for this stream before are not tried again.
                let all = hardware.order();
                let any = !all.is_empty();
                let order: Vec<_> = all.into_iter().filter(|&backend| !self.refused.contains(&(backend, setup.format))).collect();
                if any && order.is_empty() && setup.chroma_packing == 0 {
                    // Every hardware decoder gave way to software this stream, which was said then.
                    return match media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false) {
                        Ok(decoder) => { self.install(decoder, setup); Ok(()) }
                        Err(e) => { self.shared.notify(Notice::DecoderFallback(format!("No decoder: {e}"))); Err(-1) }
                    };
                }
                media::VideoDecoder::open_any(codec, self.pictures(setup), order, setup.chroma_packing != 0, log_unavailable).or_else(|e| {
                    if setup.chroma_packing != 0 {
                        return Err(e);
                    }
                    self.shared.notify(Notice::DecoderFallback(format!(
                        "Hardware decoding unavailable: {e}"
                    )));
                    media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false)
                })
            }
        };
        match opened {
            Ok(decoder) => {
                self.install(decoder, setup);
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

    fn recovers_with_rfi(&self) -> bool {
        recovers_with_rfi(self.decoder.as_ref().map(|decoder| decoder.hardware))
    }

    fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if let Some(decoder) = &mut self.pyrowave {
            let start = session::now_us();
            decoder.chunk(first, flags, payload);
            self.shared.push_slices(decoder.take_slices());
            self.shared.counters.early_us_total.fetch_add(session::now_us() - start, Ordering::Relaxed);
        }
    }

    /// PyroWave frames decode on their own, so one that a newer frame is already queued behind
    /// can go undecoded: a decoder slower than the stream then shows fewer frames, each of them
    /// fresh, where it would show every frame late.
    fn intra_only(&self) -> bool {
        self.pyrowave.as_ref().is_some_and(|decoder| decoder.skips_frames())
    }

    fn skip(&mut self, frame: &Frame) {
        // The frame arrived: the host's timing and the bitrate count it as they count a decoded one.
        self.shared.counters.count_host_latency(frame.host_processing_latency);
        self.shared.counters.video_bytes.fetch_add(frame.data().len() as u64, Ordering::Relaxed);
        if let Some(decoder) = &mut self.pyrowave { decoder.skip(frame.data()); }
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
        self.in_flight.clear();
        if let Some(summary) = self.pyrowave.as_ref().and_then(|decoder| decoder.stage_summary()) {
            eprintln!("nightjar: PyroWave decode stages {summary}");
        }
        for line in self.pyrowave.as_ref().map(super::pyrowave::Pyrowave::gpu_report).unwrap_or_default() {
            eprintln!("nightjar: {line}");
        }
        self.pyrowave = None;
    }
}

impl Decoder {
    fn pictures(&self, setup: &VideoSetup) -> media::Pictures {
        media::Pictures {
            ten_bit: setup.format & VIDEO_FORMAT_MASK_10BIT != 0,
            chroma444: setup.format & VIDEO_FORMAT_MASK_YUV444 != 0,
            packed: setup.chroma_packing != 0,
            waiting: self.pool_waiting,
            // The device the probes ask, the packed one too: the GPU renderer's when it maps pictures.
            vaapi_node: media::vaapi_node(self.hardware),
        }
    }

    /// Vulkan video opened for a stream whose VAAPI pictures cannot be mapped into the GPU
    /// renderer's Vulkan images (`vulkan_for_unmapped`), or None: VAAPI keeps the stream then, its
    /// pictures through memory. `zero_copy`: they reach the presenter without crossing memory.
    #[cfg(target_os = "linux")]
    fn vulkan_instead(&mut self, vaapi: bool, zero_copy: bool, setup: &VideoSetup) -> Option<media::VideoDecoder> {
        // The order is asked for only where it can matter: it reads the VA driver's name again.
        if !vaapi || zero_copy || !matches!(self.hardware, Hardware::Auto(gpu, _) if !gpu.is_null()) {
            return None;
        }
        let now = Unmapped {
            vaapi,
            zero_copy,
            packed: setup.chroma_packing != 0,
            map_switched_on: crate::vulkan_video::map_switched_on(std::env::var_os(crate::vulkan_video::MAP_ENVIRONMENT).as_deref()),
            switch: super::env_flag(crate::vulkan_video::UNMAPPED_ENVIRONMENT),
        };
        let vulkan = vulkan_for_unmapped(now, self.hardware.order(), |backend| self.refused.contains(&(backend, setup.format)))?;
        match media::VideoDecoder::open_any(codec_of(setup), self.pictures(setup), vec![vulkan], false, log_unavailable) {
            Ok(decoder) => {
                eprintln!("nightjar: VAAPI pictures cannot be mapped into the renderer's Vulkan images: Vulkan video decodes the stream on the renderer's device instead, so no picture is copied through memory");
                self.stands_in = Some(setup.format);
                Some(decoder)
            }
            Err(_) => {
                eprintln!("nightjar: Vulkan video does not decode this stream either: VAAPI keeps it, each picture copied through memory");
                None
            }
        }
    }

    /// Only Linux maps VAAPI pictures and decodes with Vulkan video.
    #[cfg(not(target_os = "linux"))]
    fn vulkan_instead(&mut self, _vaapi: bool, _zero_copy: bool, _setup: &VideoSetup) -> Option<media::VideoDecoder> {
        None
    }

    /// Makes `decoder` the stream's FFmpeg decoder.
    fn install(&mut self, mut decoder: media::VideoDecoder, setup: &VideoSetup) {
        let exported = exports_dmabuf(self.dmabuf, self.kms_planes, setup) && decoder.export_dmabuf();
        let mut mapped = if exported {
            eprintln!("nightjar: VAAPI pictures go to the renderer as DMA-BUFs");
            false
        } else {
            map_to_vulkan(self.hardware, &mut decoder)
        };
        if let Some(vulkan) = self.vulkan_instead(decoder.hardware == Hardware::Vaapi, exported || mapped, setup) {
            decoder = vulkan;
            mapped = false;
        }
        self.vaapi_mapped = mapped && decoder.hardware == Hardware::Vaapi;
        say_cuda_fallback(&self.shared, &mut decoder);
        // The overlay's decoder name, in the log for runs nobody watches.
        eprintln!("nightjar: video decoder {}", decoder.name);
        #[cfg(target_os = "linux")]
        eprintln!("nightjar: video path: {}, pictures {}", decoder.hardware.label(), decoder.route());
        *lock(&self.shared.decoder_name) = decoder.name.clone();
        let from_memory = packed_from_memory(setup, mapped || matches!(decoder.hardware, Hardware::Vulkan(_)));
        if let Some((width, height)) = from_memory {
            eprintln!("nightjar: full chroma (packed 4:4:4): each {width}x{height} picture is copied through memory and rebuilt on the GPU");
        } else if setup.chroma_packing != 0 {
            eprintln!("nightjar: full chroma (packed 4:4:4): pictures are rebuilt on the GPU from its Vulkan images");
        }
        *lock(&self.shared.packed_from_memory) = from_memory;
        self.packed_mapped = setup.chroma_packing != 0 && mapped;
        self.decoder = Some(decoder);
        self.in_flight.clear();
        self.pictured = false;
    }

    /// FFmpeg took the `refused` backend to software (`media::VideoDecoder::refused`): says so,
    /// and opens the next backend of the order that opens, or the software decoder after them,
    /// so the stream never decodes in software under a hardware decoder's name, nor fails every
    /// frame (AV1) while another decoder could decode it.
    fn replace_refused(&mut self, refused: Hardware) {
        let setup = self.setup.expect("decoder setup precedes frames");
        self.refused.push((refused, setup.format));
        let codec = codec_of(&setup);
        let mut rest: Vec<_> = backends_after(self.hardware.order(), refused).into_iter().filter(|&backend| !self.refused.contains(&(backend, setup.format))).collect();
        // Vulkan video stood in for VAAPI, whose pictures could not be mapped: VAAPI decodes the
        // stream, through memory, before the decoders after both and before software.
        if stood_in_for_vaapi(refused, self.stands_in.take(), setup.format) && !self.refused.contains(&(Hardware::Vaapi, setup.format)) {
            rest.insert(0, Hardware::Vaapi);
        }
        let hardware = if rest.is_empty() {
            Err("no other hardware decoder".to_owned())
        } else {
            media::VideoDecoder::open_any(codec, self.pictures(&setup), rest, false, log_unavailable)
        };
        let (opened, now) = match hardware {
            Ok(decoder) => {
                let now = trf("{} cannot decode this stream. Now decoding with {}.", &[&refused.label(), &decoder.hardware.label()]);
                (Ok(decoder), now)
            }
            Err(error) => {
                eprintln!("nightjar: after {} refused the stream: {error}", refused.label());
                let now = trf("{} cannot decode this stream. Now decoding in software.", &[&refused.label()]);
                (media::VideoDecoder::open(codec, Hardware::None, self.software_threads, false), now)
            }
        };
        self.decoder = None;
        match opened {
            Ok(decoder) => {
                self.shared.notify(Notice::DecoderFallback(now));
                self.install(decoder, &setup);
            }
            Err(error) => {
                self.shared.notify(Notice::DecoderFailed(trf("{} cannot decode this stream, and no other decoder opens: {}", &[&refused.label(), &error])));
            }
        }
    }

    /// Decodes `frame`, PyroWave or FFmpeg, and queues its picture for the presenter.
    fn decode_frame(&mut self, frame: &Frame) -> DecodeStatus {
        self.shared.counters.count_host_latency(frame.host_processing_latency);
        self.shared.counters.video_bytes.fetch_add(frame.data().len() as u64, Ordering::Relaxed);
        if self.pyrowave.is_some() {
            let start = session::now_us();
            let setup = self.setup.expect("decoder setup precedes frames");
            if self.dump.is_some() { self.dump_envelope(&setup, frame.data()); }
            let Some(decoder) = self.pyrowave.as_mut() else { return DecodeStatus::Ok };
            let shared = &self.shared;
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
                early, first_block_us: output.first_block_us, race: output.race, bands: output.bands, race_seq: output.race_seq, visible: None };
            self.shared.queue(decoded);
            return DecodeStatus::Ok;
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return DecodeStatus::Ok;
        };
        let started = session::now_us();
        let pts = (i64::from(frame.number) << 32) | i64::from(frame.rtp_timestamp);
        let sent = InFlight {
            number: frame.number,
            receive_us: frame.receive_time_us,
            enqueue_us: frame.enqueue_time_us,
            input_us: started,
            host_latency: frame.host_processing_latency,
        };
        if self.in_flight.len() >= IN_FLIGHT_LIMIT {
            self.in_flight.pop_front();
        }
        self.in_flight.push_back(sent);
        let setup = self.setup.expect("decoder setup precedes frames");
        let asynchronous = decoder.asynchronous();
        // Pictures before the first of a refused decoder are its pool's; the first is in memory
        // and goes to the queue's pool limit too, which only evicts sooner.
        let pooled = decoder.pooled();
        let fps_millihz = self.shared.fps_millihz.load(Ordering::Relaxed).max(1);
        let (shared, in_flight, pool_waiting) = (&self.shared, &mut self.in_flight, self.pool_waiting);
        let sent_at = std::time::Instant::now();
        // FFmpeg takes the frame's bytes by reference, not copied; ours goes back after the frame,
        // so they return to the receiver's pool once FFmpeg lets go too.
        decoder.lend(Some(frame.shared_data()));
        let fed = feed(decoder, frame.data(), pts, async_wait_us(fps_millihz),
            || sent_at.elapsed().as_micros() as u64,
            || std::thread::sleep(std::time::Duration::from_micros(ASYNC_POLL_US)),
            |picture| {
                let ready = session::now_us();
                // An asynchronous decoder's picture is an earlier frame's: its own times, by pts.
                let pts = picture.info().pts;
                let own = take_in_flight(in_flight, (pts >> 32) as u32).unwrap_or(sent);
                let counters = &shared.counters;
                counters.decoded.fetch_add(1, Ordering::Relaxed);
                counters.decode_us_total.fetch_add(ready.saturating_sub(own.input_us), Ordering::Relaxed);
                let mut timing = lock(&shared.decode_window);
                if timing.samples == 0 { timing.first_us = ready; }
                timing.last_us = ready;
                timing.samples += 1;
                timing.queue_us += own.input_us.saturating_sub(own.enqueue_us);
                let budget_us = 2_000_000_000 / u64::from(fps_millihz);
                timing.late += u32::from(ready.saturating_sub(own.enqueue_us) >= budget_us);
                drop(timing);
                let decoded = Decoded {
                    picture: Picture::Frame(picture),
                    setup,
                    rtp: pts as u32,
                    number: own.number,
                    receive_us: own.receive_us,
                    enqueue_us: own.enqueue_us,
                    input_us: own.input_us,
                    ready_us: ready,
                    host_latency: own.host_latency,
                    early: 0,
                    first_block_us: 0,
                    race: Default::default(),
                    bands: Vec::new(),
                    race_seq: 0,
                    visible: None,
                };
                if pooled { shared.queue_pooled(decoded, pool_waiting) } else { shared.queue(decoded) }
            });
        decoder.lend(None);
        say_cuda_fallback(&self.shared, decoder);
        if let Some((width, height)) = packed_map_dropped(&setup, self.packed_mapped, decoder.vulkan_mapped()) {
            eprintln!("nightjar: full chroma (packed 4:4:4): the Vulkan map failed, so each {width}x{height} picture is copied through memory from now on");
            *lock(&self.shared.packed_from_memory) = Some((width, height));
            self.packed_mapped = false;
        }
        let status = frame_status(&fed, asynchronous, self.pictured);
        self.pictured |= fed.pictures > 0;
        if status == DecodeStatus::NeedIdr {
            self.shared.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
            if !(fed.refused || fed.failed) {
                eprintln!("nightjar: frame {} {}; asking for a key frame", frame.number,
                    if fed.corrupt > 0 { "decoded damaged" } else { "made no picture" });
            }
        }
        // FFmpeg took a hardware decoder to software: said, and the next decoder opened, which
        // starts from a key frame.
        let backend = decoder.hardware;
        let unmapped = self.vaapi_mapped && !decoder.vulkan_mapped();
        if let Some(name) = decoder.renamed().map(str::to_owned) {
            *lock(&self.shared.decoder_name) = name;
            self.replace_refused(backend);
            return DecodeStatus::NeedIdr;
        }
        // A picture's import failed and the map is gone: Vulkan video takes the stream over where
        // it can, from a key frame, before every picture goes through memory.
        if unmapped {
            self.vaapi_mapped = false;
            if let Some(vulkan) = self.vulkan_instead(true, false, &setup) {
                self.decoder = None;
                self.install(vulkan, &setup);
                return DecodeStatus::NeedIdr;
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
    wasapi: Option<super::wasapi::Output>,
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
        match super::wasapi::Output::open() {
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
    Wasapi(MutexGuard<'a, super::wasapi::Queue>),
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
        if channels == super::wasapi::CHANNELS {
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
        for headphones in lock(&self.shared.controller_audio).iter().filter_map(std::sync::Weak::upgrade) {
            routed |= lock(&headphones).headphones(&pcm[lead * channels..], channels);
        }
        if routed {
            playout.stop();
            sink.clear();
            counters.audio_queued_us.store(0, Ordering::Relaxed);
            return;
        }
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
mod tests;
