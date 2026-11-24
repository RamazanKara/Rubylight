//! GPU PyroWave stream decoder (`GpuStream`), pushed-record bitmap (`BlockSet`), and beam-race
//! slicing state (`Race`).

use std::time::Instant;

use rubylight_protocol::pyrowave::{push_container, Records};

use crate::media::Picture;
use super::super::pyrowave_sdk::{GpuConfig, GpuDecoder};
use super::super::stage_times::Stages;
use super::{pass_records, word, BandReady, Early, Failure, Output, RaceReport, SlicePlan, SliceReady, Source};
#[cfg(windows)]
use super::GPU_DECODE_TIMEOUT;

/// Where a Linux decode call that began at `start` splits into its submit and its wait for the
/// GPU: when the SDK's submit returned, or `start` when that is unknown (the whole call is then
/// the wait, as before the split) or earlier than `start`.
// Unix (Linux and macOS): decode calls split elapsed time at the submit timestamp.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn submit_split(start: Instant, submitted: Option<Instant>) -> Instant {
    submitted.filter(|&at| at >= start).unwrap_or(start)
}

/// The most a frame may have lost (the SDK gate's 90% packet ratio) once its slices were decoded.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) const SLICED_LOSS_LIMIT_PERCENT: f32 = 10.0;
/// Frames in a row that must arrive in row order before slicing starts again after one did not.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) const ORDERED_FRAMES_TO_RESUME: u32 = 8;

/// Whether the final decode of a frame may go ahead. A slice decode marks the SDK's sequence as
/// decoded, and its ready flag is false for the rest of it, so a frame whose slices were decoded
/// is judged on the loss its records cache had to fill in (the gate's own 90% packet ratio)
/// when its chunks were the whole envelope (`finished`). Any other frame asks the SDK.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn final_decode_gate(sliced: bool, finished: bool, loss_percent: f32, sdk_ready: impl FnOnce() -> bool) -> Result<(), &'static str> {
    if sliced && finished {
        if loss_percent > SLICED_LOSS_LIMIT_PERCENT {
            return Err("PyroWave frame lost too much of its data after its slices were decoded");
        }
    } else if !sdk_ready() {
        return Err("PyroWave frame holds no picture data");
    }
    Ok(())
}

const GPU_STAGE_NAMES: [&str; 5] = ["clear", "push", "submit", "gpu", "slices"];
const GPU_CLEAR: usize = 0;
const GPU_PUSH: usize = 1;
const GPU_SUBMIT: usize = 2;
#[cfg_attr(windows, allow(dead_code))]
const GPU_WAIT: usize = 3;
/// The slice decodes made while the frame arrives (beam race), submit and wait together.
#[cfg_attr(not(windows), allow(dead_code))]
const GPU_SLICES: usize = 4;
/// The first word of a record that is the sequence header.
const HEADER_BIT: u32 = 0x8000_0000;

/// Gives one record to the SDK; false if it is a sequence header for another picture than the
/// one negotiated (the CPU path's check, `Decoder::coefficients`) or the SDK refuses it.
fn push_checked(gpu: &mut GpuDecoder, packet: &[u8]) -> bool {
    if word(packet, 0) & HEADER_BIT != 0 {
        let (first, second, config) = (word(packet, 0), word(packet, 4), gpu.config());
        if (first as usize & 0x3fff) + 1 != config.width as usize || ((first >> 14) as usize & 0x3fff) + 1 != config.height as usize
            || (second >> 24) & 3 != 0 || (second & (1 << 26) != 0) != config.chroma444 {
            return false;
        }
    }
    gpu.push_packet(packet).is_ok()
}

/// The block indices of the records a frame gave the SDK early, as a bitmap. A 4K 4:4:4 frame
/// has about 24,000 records, each inserted as it arrives and looked up again when the frame
/// completes; a `HashSet` hashed each of them (SipHash) on the decoder thread, inside the push.
/// Indices are 24 bits (`word >> 8`), so the map is at most 2 MiB; `clear` zeroes only the words
/// the frame touched.
#[derive(Default)]
pub(super) struct BlockSet {
    pub(super) words: Vec<u64>,
    /// One past the highest word set since `clear`.
    used: usize,
    len: usize,
}

impl BlockSet {
    pub(super) fn clear(&mut self) {
        self.words[..self.used].fill(0);
        self.used = 0;
        self.len = 0;
    }

    /// Adds `index`; false if it was there already.
    pub(super) fn insert(&mut self, index: u32) -> bool {
        let (word, bit) = (index as usize / 64, 1u64 << (index % 64));
        if word >= self.words.len() { self.words.resize(word + 1, 0); }
        self.used = self.used.max(word + 1);
        let added = self.words[word] & bit == 0;
        self.words[word] |= bit;
        self.len += usize::from(added);
        added
    }

    pub(super) fn contains(&self, index: u32) -> bool {
        self.words.get(index as usize / 64).is_some_and(|word| word & (1u64 << (index % 64)) != 0)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// PyroWave frames decoded by the SDK on the GPU into shared D3D11 planes. The records go to
/// the SDK as they complete (early decode) or when the frame does; the SDK runs the entropy
/// decoding and the inverse wavelets, and the presenter converts the planes to RGB.
pub struct GpuStream {
    pub(super) gpu: GpuDecoder,
    planes: std::sync::Arc<super::super::gpu_planes::Planes>,
    records: Records,
    early: Early,
    missed_start: bool,
    early_bytes: usize,
    first_block_us: u64,
    /// The records already given to the SDK for the frame in flight, by block index.
    pushed: BlockSet,
    /// Where the last frame's records came from (`Pyrowave::source`).
    pub(super) source: Source,
    stages: Stages<5>,
    /// The beam race (`set_slices`); None decodes each frame once, as before.
    pub(super) race: Option<Race>,
    /// The AMD GPU's clocks while the stream runs (None without amdgpu).
    #[cfg(target_os = "linux")]
    clocks: Option<super::super::gpu_clocks::Sampler>,
}

/// The beam race state of the frame in flight.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct Race {
    pub(super) plan: SlicePlan,
    /// The sequence header went to the SDK ahead of the records, so a prefix decodes.
    pub(super) header_pushed: bool,
    /// The plane set the slices decode into, which the final decode then fills (with its `after`).
    pub(super) lease: Option<(super::super::gpu_planes::Lease, u64)>,
    pub(super) bands: Vec<BandReady>,
    /// A slice decode failed: the frame ends with this (a loud presenter error), nothing falls back.
    pub(super) failure: Option<String>,
    /// No plane set was free for the slices: the frame decodes once, whole.
    pub(super) no_plane: bool,
    /// Frames in a row that arrived in row order, up to `ORDERED_FRAMES_TO_RESUME`.
    pub(super) ordered_run: u32,
    /// This frame may be sliced (the last frames were in order).
    pub(super) slicing: bool,
    /// The frame in flight (`race::next_seq`: it grows across decoders; slices and the frame's `Output` carry it).
    pub(super) seq: u64,
    /// Slices decoded and not yet taken by the decoder thread.
    pub(super) events: Vec<SliceReady>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Race {
    pub(super) fn begin_frame(&mut self) {
        self.plan.begin_frame();
        self.seq = super::super::race::next_seq();
        self.events.clear();
        self.header_pushed = false;
        self.lease = None;
        self.bands.clear();
        self.failure = None;
        self.no_plane = false;
        self.slicing = self.ordered_run >= ORDERED_FRAMES_TO_RESUME;
    }

    /// The final decode is the last slice; what the frame's slices were. A frame that was not
    /// received as chunks, or not in row order, has no slices: slices decoded before its order
    /// broke are not exact, so the one entry is the final decode of the whole picture. The same
    /// goes for the one late record `ordered` tolerates, when a slice was decoded before it came:
    /// that slice lacks the record, and the frame counts as unordered, ending the run of ordered
    /// frames. A late record before the first slice costs nothing and still counts as ordered.
    pub(super) fn finish(&mut self, fence_value: u64, ready_us: u64, chunks: bool) -> (RaceReport, Vec<BandReady>) {
        let report = self.plan.report();
        let ordered = chunks && report.ordered();
        let sliced = ordered && report.violations == 0;
        let fell_back = ordered && !sliced && !self.bands.is_empty();
        if chunks {
            self.ordered_run = if ordered && !fell_back { (self.ordered_run + 1).min(ORDERED_FRAMES_TO_RESUME) } else { 0 };
        }
        let (first_row, end_row) = if sliced { self.plan.rest() } else { self.plan.whole() };
        if !sliced { self.bands.clear(); }
        self.bands.push(BandReady { slice: self.bands.len() as u8, first_row, end_row, fence_value, ready_us });
        let bands = std::mem::take(&mut self.bands);
        if !chunks { return (RaceReport::default(), bands); }
        let flag = if ordered && !fell_back { RaceReport::ORDERED } else { RaceReport::UNORDERED };
        (RaceReport { slices: bands.len() as u8, first_ready_us: bands[0].ready_us, last_ready_us: ready_us, ordered: flag }, bands)
    }
}

impl GpuStream {
    /// Creates the SDK decoder on `device` (the renderer's) with `slots` plane sets.
    #[cfg(windows)]
    pub fn new(device: &windows::Win32::Graphics::Direct3D11::ID3D11Device, width: usize, height: usize, chroma444: bool, ten_bit: bool, slots: usize) -> Result<Self, String> {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots };
        GpuDecoder::new(device, config).map(Self::with_decoder)
    }

    /// Creates the SDK decoder with `slots` plane sets, on the Vulkan device of the renderer whose
    /// SDL GPU device is `device` where it can be shared, else on the driver's default device
    /// (`GpuDecoder::for_renderer`).
    #[cfg(target_os = "linux")]
    pub fn new(device: *mut std::ffi::c_void, width: usize, height: usize, chroma444: bool, ten_bit: bool, slots: usize) -> Result<Self, String> {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots };
        GpuDecoder::for_renderer(device, config).map(Self::with_decoder)
    }

    /// macOS: the SDK decoder on a Vulkan device of its own on the bundled KosmicKrisp driver,
    /// its planes read back for the Metal renderer (`GpuDecoder::on_own_vulkan`).
    #[cfg(target_os = "macos")]
    pub fn new(width: usize, height: usize, chroma444: bool, ten_bit: bool, slots: usize) -> Result<Self, String> {
        let config = GpuConfig { width: width as u32, height: height as u32, chroma444, ten_bit, slots };
        GpuDecoder::on_own_vulkan(super::super::pyrowave_sdk::Api::shared()?, config).map(Self::with_decoder)
    }

    pub fn with_decoder(gpu: GpuDecoder) -> Self {
        let config = gpu.config();
        Self {
            planes: super::super::gpu_planes::Planes::new(&gpu),
            records: Records::with_geometry(config.width, config.height, config.chroma444),
            gpu,
            early: Early::Off,
            missed_start: false,
            early_bytes: 0,
            first_block_us: 0,
            pushed: BlockSet::default(),
            source: Source::Frame,
            stages: Stages::new(GPU_STAGE_NAMES),
            race: None,
            #[cfg(target_os = "linux")]
            clocks: super::super::gpu_clocks::Sampler::start(),
        }
    }

    /// The SDK's own GPU time per frame over the stream (`pyrowave_sdk::pass_summary`), for the
    /// log beside the stages, whose "gpu" stage also holds the time the decode waited for the queue.
    pub fn pass_summary(&self) -> Option<String> {
        // Unix (Linux and macOS): a stalled watchdog decoder is never called again.
        #[cfg(unix)]
        if self.gpu.stalled() { return None; }
        super::super::pyrowave_sdk::pass_summary(&self.gpu.performance_stats(false))
    }

    /// What the AMD GPU's clocks did during the stream (`gpu_clocks`), where they can be read:
    /// on Linux.
    pub fn clock_summary(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            self.clocks.as_ref().and_then(super::super::gpu_clocks::Sampler::summary)
        }
        #[cfg(not(target_os = "linux"))]
        None
    }

    /// Decodes the picture in `slices` slices while a row-ordered frame arrives (0 turns it off).
    /// Frames that do not arrive in row order are decoded once, whole, and say so in their report.
    #[cfg(windows)]
    pub fn set_slices(&mut self, slices: u32) {
        let config = self.gpu.config();
        self.race = (slices > 0).then(|| Race {
            plan: SlicePlan::new(config.width, config.height, config.chroma444, slices),
            header_pushed: false,
            lease: None,
            bands: Vec::new(),
            failure: None,
            no_plane: false,
            ordered_run: ORDERED_FRAMES_TO_RESUME,
            slicing: true,
            seq: 0,
            events: Vec::new(),
        });
    }

    #[cfg(all(test, not(target_os = "macos")))]
    pub fn planes(&self) -> &std::sync::Arc<super::super::gpu_planes::Planes> {
        &self.planes
    }

    /// The stream starts over on this decoder (a reconnect that reuses it): drops the packets and
    /// any slice of a frame still in flight, and any plane set they pinned, so no picture of the
    /// last stream can come out.
    pub fn flush(&mut self) {
        self.gpu.clear();
        let config = self.gpu.config();
        // A fresh recovery cache: no block of the last stream's frames can stand in for one the
        // new stream loses.
        self.records = Records::with_geometry(config.width, config.height, config.chroma444);
        self.early = Early::Off;
        self.missed_start = false;
        self.early_bytes = 0;
        self.first_block_us = 0;
        self.pushed.clear();
        self.source = Source::Frame;
        if let Some(race) = &mut self.race {
            race.lease = None;
            race.events.clear();
            race.header_pushed = false;
            race.bands.clear();
            race.failure = None;
            race.no_plane = false;
            // A new stream's first frames are sliced as a fresh decoder's are, whatever order the
            // last stream's frames arrived in.
            race.ordered_run = ORDERED_FRAMES_TO_RESUME;
            race.slicing = true;
        }
    }

    pub fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        if first {
            let step = Instant::now();
            self.stages.reset();
            self.gpu.clear();
            self.records.begin_frame();
            self.pushed.clear();
            self.early = Early::Active;
            self.early_bytes = 4;
            self.first_block_us = 0;
            self.missed_start = false;
            if let Some(race) = &mut self.race { race.begin_frame(); }
            self.stages.add(GPU_CLEAR, step);
        } else if self.early == Early::Off {
            self.missed_start = true;
            return;
        }
        if self.early != Early::Active { return; }
        let step = Instant::now();
        self.early_bytes += 8 + payload.len();
        if !self.records.feed_packet(flags, payload) { self.early = Early::Failed; return; }
        let Self { records, gpu, pushed, race, .. } = self;
        let mut failed = false;
        if let Some(race) = race.as_mut() {
            // A packet that is not the first and does not continue the one before it follows a loss.
            if !first && flags & rubylight_protocol::pyrowave::PACKET_CONTIGUOUS == 0 { race.plan.gap(); }
            race.plan.packet();
            if flags & rubylight_protocol::pyrowave::PACKET_FRAME_START != 0 && payload.len() >= 16 {
                // The sequence header first, so a slice decode knows the picture. `finish_frame`
                // pushes it again with the final record count, which the SDK takes for the same sequence.
                if push_checked(gpu, &payload[8..16]) { race.header_pushed = true; } else { failed = true; }
            }
            if flags & rubylight_protocol::pyrowave::PACKET_FRAME_END != 0 { race.plan.finish(); }
        }
        records.drain_new(|index, record| {
            if failed { return; }
            if !push_checked(gpu, record) { failed = true; return; }
            pushed.insert(index);
            if let Some(race) = race.as_mut() { race.plan.record(index); }
        });
        if failed { self.early = Early::Failed; }
        if !self.pushed.is_empty() && self.first_block_us == 0 { self.first_block_us = nightjar_core::session::now_us(); }
        self.stages.add(GPU_PUSH, step);
        self.decode_due_slice();
    }

    /// Decodes the slice that the records so far complete, if there is one: the prefix of the
    /// frame, missing blocks as zero (the SDK needs no `is_ready` for that, which a prefix never
    /// satisfies), into the plane set the final decode will use. Waits for it, because the next
    /// decode needs the SDK's payload buffer free.
    #[cfg(windows)]
    fn decode_due_slice(&mut self) {
        if self.early != Early::Active { return; }
        let Self { race: Some(race), gpu, planes, stages, .. } = self else { return };
        if !race.slicing || !race.header_pushed || race.failure.is_some() || race.no_plane { return; }
        let Some(due) = race.plan.due() else { return };
        let step = Instant::now();
        if race.lease.is_none() {
            match planes.acquire() {
                Some(lease) => race.lease = Some(lease),
                None => { race.no_plane = true; return; }
            }
        }
        let Some((lease, after)) = &race.lease else { return };
        // The slices already drawn from the set (by the presenter) are waited for as well. A slice
        // that is ready but not drawn yet is not: this decode rewrites its rows while the draw may read
        // them. That is safe for two reasons. The rewrite is the same bytes: a slice's rows depend only
        // on the records of the steps it was cut at, all of which this prefix has, so every decode of
        // the frame from that slice on gives those rows the final decode's values (the ignored GPU
        // test `a_slice_decode_is_exact_on_its_rows_on_real_row_ordered_frames` holds each slice
        // to them; this relies on the SDK's output planes being written by its last inverse wavelet
        // pass, which that test checks only through the values). And the set is acquired from
        // `QUEUE_FAMILY_EXTERNAL` once it holds a slice (`decode_again`): the SDK then records GENERAL
        // to GENERAL, where acquiring from IGNORED records UNDEFINED to GENERAL and lets the
        // driver discard the held rows. The rows below the slice, which this decode does change, are
        // outside the draw's scissor.
        let wait = (*after).max(planes.wait_for(lease.set()));
        let decoded = if race.bands.is_empty() { gpu.decode(lease.set(), wait) } else { gpu.decode_again(lease.set(), wait) };
        match decoded.and_then(|fence| gpu.wait(fence, GPU_DECODE_TIMEOUT).map(|()| fence)) {
            Ok(fence_value) => {
                let band = BandReady {
                    slice: due.slice as u8, first_row: due.first_row, end_row: due.end_row, fence_value, ready_us: nightjar_core::session::now_us(),
                };
                race.bands.push(band);
                race.events.push(SliceReady { seq: race.seq, band, pin: lease.pin() });
            }
            Err(error) => race.failure = Some(error.to_string()),
        }
        stages.add(GPU_SLICES, step);
    }

    /// The slices decoded since the last call, for the presenter.
    pub fn take_slices(&mut self) -> Vec<SliceReady> {
        self.race.as_mut().map(|race| std::mem::take(&mut race.events)).unwrap_or_default()
    }

    #[cfg(not(windows))]
    fn decode_due_slice(&mut self) {}

    /// Passes over a frame that a newer one replaces: nothing is decoded and no plane set is
    /// taken, and the records cache takes the frame as `frame` would have. The records its
    /// chunks pushed to the SDK go with the next frame, which clears them either way.
    pub fn skip(&mut self, data: &[u8]) {
        let early = std::mem::replace(&mut self.early, Early::Off);
        self.missed_start = false;
        self.source = Source::Frame;
        self.first_block_us = 0;
        pass_records(&mut self.records, early == Early::Active && self.early_bytes == data.len(), data);
    }

    pub fn frame(&mut self, data: &[u8], evict: &mut dyn FnMut() -> bool) -> Result<Output, Failure> {
        let early = std::mem::replace(&mut self.early, Early::Off);
        let missed_start = std::mem::take(&mut self.missed_start);
        if let Some(race) = &mut self.race
            && let Some(failure) = race.failure.take() {
                race.lease = None;
                return Err(Failure::Device(failure));
            }
        if early == Early::Off { self.stages.reset(); }
        let records_envelope = data.starts_with(&[0, 0, 0, 0]);
        let sliced = self.race.as_ref().is_some_and(|race| race.lease.is_some());
        let whole_early = early == Early::Active && records_envelope && self.early_bytes == data.len();
        // The slices decoded into a plane set already; the final decode fills that one.
        let mut slot = self.race.as_mut().and_then(|race| race.lease.take()).or_else(|| self.planes.acquire());
        while slot.is_none() && evict() { slot = self.planes.acquire(); }
        let Some((lease, after)) = slot else {
            // Keep the records cache current, so the frames after this one recover from it.
            if whole_early { self.records.finish_frame(|_| true); } else if records_envelope { self.records.push_frame(data, |_| true); }
            return Err(Failure::NoPlanes);
        };
        if data.len() > rubylight_protocol::pyrowave::MAX_FRAME_BYTES {
            return Err(Failure::Frame("PyroWave frame exceeds the packet budget".into()));
        }
        let mut finished = false;
        let mut source = Source::Frame;
        if early != Early::Off {
            // The chunks are the frame only if they add up to its envelope; the frame decides.
            if whole_early {
                let step = Instant::now();
                let Self { records, gpu, pushed, .. } = self;
                finished = records.finish_frame(|packet| {
                    if word(packet, 0) & HEADER_BIT != 0 || !pushed.contains(word(packet, 4) >> 8) { push_checked(gpu, packet) } else { true }
                });
                self.stages.add(GPU_PUSH, step);
            }
            source = if finished { Source::Early } else { Source::Fallback };
        } else if missed_start {
            source = Source::Fallback;
        }
        self.source = source;
        if !finished {
            let step = Instant::now();
            self.gpu.clear();
            let Self { records, gpu, .. } = self;
            let push = |packet: &[u8]| push_checked(gpu, packet);
            let valid = if records_envelope { records.push_frame(data, push) } else { push_container(data, push) };
            self.stages.add(GPU_PUSH, step);
            if !valid { return Err(Failure::Frame("Malformed PyroWave frame container, or a sequence that does not match the negotiated picture".into())); }
        }
        let loss_percent = if records_envelope { self.records.loss_percent } else { 0.0 };
        // A slice decode marks the SDK's sequence as decoded, and `is_ready` is then false for the
        // rest of it. The slices' frame is complete if its chunks were the envelope (`finished`);
        // what the cache had to fill in is the loss the gate would have judged.
        // A frame the cache had to fill in failed for what the network lost, not for the decoder.
        final_decode_gate(sliced, finished, loss_percent, || self.gpu.is_ready(true))
            .map_err(|why| if loss_percent > 0.0 { Failure::Loss(why.into()) } else { Failure::Frame(why.into()) })?;
        let step = Instant::now();
        #[cfg(windows)]
        let fence_value = {
            // Draws of the frame's slices from this set come first (none without the race).
            let after = if self.race.is_some() { after.max(self.planes.wait_for(lease.set())) } else { after };
            // The set of a sliced frame holds its slices (see `decode_due_slice`); a set that holds nothing is overwritten.
            let decoded = if sliced { self.gpu.decode_again(lease.set(), after) } else { self.gpu.decode(lease.set(), after) };
            let fence_value = decoded.map_err(|e| Failure::Device(e.to_string()))?;
            self.stages.add(GPU_SUBMIT, step);
            fence_value
        };
        // Unix (Linux and macOS): zero-copy Vulkan hands `(lease, decode_value)` directly to the
        // presenter (GPU timeline semaphore orders sampling); CPU readback blocks until the
        // readback buffer is filled.
        #[cfg(unix)]
        let fence_value = {
            let decoded = if self.gpu.images().is_some() {
                self.gpu.decode_into(lease.set(), after)
            } else {
                let gpu = &mut self.gpu;
                lease.with_buffers(|[y, cb, cr]| gpu.decode([y, cb, cr]))
                    .unwrap_or_else(|| Err("the plane set has no CPU buffers".into()))
                    .map(|()| 0)
            };
            let fence_value = match decoded {
                Ok(value) => value,
                Err(e) => {
                    // A decode that missed its deadline may still write into this set once the
                    // device comes back: the set is never given back, so nothing reads or reuses it.
                    if self.gpu.stalled() { std::mem::forget(lease); }
                    return Err(Failure::Device(e.to_string()));
                }
            };
            let submitted = submit_split(step, self.gpu.submitted_at());
            self.stages.add_between(GPU_SUBMIT, step, submitted);
            self.stages.add(GPU_WAIT, submitted);
            fence_value
        };
        self.stages.commit();
        let first_block_us = if source == Source::Early { self.first_block_us } else { 0 };
        #[cfg(windows)]
        let (race, bands) = match &mut self.race {
            Some(race) => race.finish(fence_value, nightjar_core::session::now_us(), source == Source::Early),
            None => (RaceReport::default(), Vec::new()),
        };
        #[cfg(not(windows))]
        let (race, bands) = (RaceReport::default(), Vec::new());
        let race_seq = if race.slices > 0 { self.race.as_ref().map_or(0, |race| race.seq) } else { 0 };
        Ok(Output {
            picture: Picture::GpuPlanes { lease, fence_value },
            loss_percent, first_block_us, bands, race, race_seq,
        })
    }

    /// The stage times; the slices stage only when the beam race is on.
    pub fn stage_summary(&self) -> Option<String> {
        self.stages.summary_of(if self.race.is_some() { GPU_STAGE_NAMES.len() } else { GPU_SLICES })
    }
}
