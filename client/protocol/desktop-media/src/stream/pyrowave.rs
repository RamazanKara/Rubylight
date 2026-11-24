//! Rust decoder for PyroWave's 186f0393 bitstream.
//! Coefficient ordering and CDF 9/7 lifting follow Themaister/pyrowave a0d2e71 (MIT,
//! Copyright (c) 2026 Hans-Kristian Arntzen; app/src/main/assets/pyrowave_notices.txt).

use rubylight_protocol::pyrowave::rows::{band_rows, slice_ends, RowReport, RowTracker};
use rubylight_protocol::pyrowave::Records;

use crate::media::Picture;

mod cpu;
mod gpu;
mod pack;

pub use cpu::{dump_name, Decoder};
pub use gpu::GpuStream;

/// How long the decoder thread waits for the GPU to finish a frame before the stream ends.
pub(crate) const GPU_DECODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(test)]
use cpu::{decode_block, parse_dump_name};
#[cfg(test)]
use gpu::{final_decode_gate, submit_split, BlockSet, SLICED_LOSS_LIMIT_PERCENT};
#[cfg(all(test, windows))]
use gpu::{Race, ORDERED_FRAMES_TO_RESUME};
#[cfg(test)]
use pack::half;
#[cfg(all(test, target_arch = "x86_64"))]
use pack::pack_half_row;
#[cfg(all(test, any(windows, target_os = "linux")))]
use super::pyrowave_sdk::{GpuConfig, GpuDecoder};

/// Where early decoding of the frame in flight stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Early {
    /// No chunk of this frame was taken: the whole-frame path decodes it.
    Off,
    /// Chunks since the first one are decoded into the planes as their records complete.
    Active,
    /// Something did not parse or decode: the frame decodes whole, from its envelope.
    Failed,
}

/// How the last `coefficients` call got its blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// All in the call, from the finished frame.
    #[default]
    Frame,
    /// The blocks that arrived before the frame were already decoded; the call did the rest.
    Early,
    /// Early chunks were taken but did not match the frame, so the call decoded all of it.
    Fallback,
}

fn word(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

/// One reconstructed frame, ready for the presenter.
pub struct Output {
    pub picture: Picture,
    pub loss_percent: f32,
    pub first_block_us: u64,
    /// The slices decoded for the frame, in order (empty unless the beam race is on).
    pub bands: Vec<BandReady>,
    pub race: RaceReport,
    /// The sequence number of the frame's slices (`SliceReady::seq`); 0 when it had none.
    pub race_seq: u64,
}

/// A slice of a frame still being received, whose decode has finished: rows `band.first_row..
/// band.end_row` can be drawn from the plane set `pin` holds once `band.fence_value` is signalled.
/// The frame's final decode follows in its `Output`.
pub struct SliceReady {
    pub seq: u64,
    pub band: BandReady,
    pub pin: super::gpu_planes::Pin,
}

/// A slice of the picture whose decode has finished: rows `first_row..end_row` of the frame being
/// received are exact in the plane set once `fence_value` is signalled on the decode fence. The
/// last slice is the final decode of the whole frame (its rows run to the end of the picture).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BandReady {
    pub slice: u8,
    pub first_row: u32,
    pub end_row: u32,
    pub fence_value: u64,
    /// When the decode was seen to finish (session clock, us).
    pub ready_us: u64,
}

/// How the beam race went for one frame (the timing CSV's `slices_decoded`, `first_slice_ready_ns`,
/// `last_slice_ready_ns` and `row_ordered`). All zero when the race is off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RaceReport {
    /// Slice decodes for the frame, the final one included.
    pub slices: u8,
    pub first_ready_us: u64,
    pub last_ready_us: u64,
    /// 0 not known, `ORDERED` or `UNORDERED`.
    pub ordered: u8,
}

impl RaceReport {
    pub const ORDERED: u8 = 1;
    pub const UNORDERED: u8 = 2;
}

/// Which slice of the picture is due while the records of a row-ordered frame arrive: the
/// shared row tracker and the slice ends. The last slice is the final decode, so it is never due
/// here.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct SlicePlan {
    tracker: RowTracker,
    /// Slice `i` holds the bands before `ends[i]`.
    ends: Vec<u32>,
    /// The next slice not yet decoded.
    next: usize,
    height: u32,
}

/// Rows of a slice that is due.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Due {
    pub slice: usize,
    pub first_row: u32,
    pub end_row: u32,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl SlicePlan {
    pub fn new(width: u32, height: u32, chroma444: bool, slices: u32) -> Self {
        let tracker = RowTracker::new(width, height, chroma444);
        let ends = slice_ends(tracker.bands(), slices);
        Self { tracker, ends, next: 0, height }
    }

    pub fn begin_frame(&mut self) {
        self.tracker.begin_frame();
        self.next = 0;
    }

    /// A packet was fed; the records it completed follow with `record`.
    pub fn packet(&mut self) {
        self.tracker.packet();
    }

    pub fn record(&mut self, index: u32) {
        self.tracker.record(index);
    }

    /// Packets were lost before the one that was just fed: no slice is due for the rest of the frame.
    pub fn gap(&mut self) {
        self.tracker.gap();
    }

    /// The whole picture, for a frame that is decoded once.
    pub fn whole(&self) -> (u32, u32) {
        (0, self.height)
    }

    /// The frame's last packet was fed.
    pub fn finish(&mut self) {
        self.tracker.finish();
    }

    pub fn report(&self) -> RowReport {
        self.tracker.report()
    }

    /// The furthest slice that can be decoded exactly now, if it is one not decoded yet. A frame
    /// that has left the row order, or lost a packet, is never sliced from there on: it decodes whole.
    pub fn due(&mut self) -> Option<Due> {
        let report = self.tracker.report();
        if report.violations > 0 || report.gaps > 0 { return None; }
        let done = self.tracker.complete_bands();
        let bands = self.tracker.bands();
        let target = self.ends.iter().rposition(|&end| end <= done && end < bands)?;
        if target < self.next { return None; }
        let first = self.first_band();
        let (first_row, end_row) = band_rows(self.height, first, self.ends[target]);
        self.next = target + 1;
        Some(Due { slice: target, first_row, end_row })
    }

    /// Rows still to show when the frame completes: after the last slice decoded.
    pub fn rest(&self) -> (u32, u32) {
        band_rows(self.height, self.first_band(), self.tracker.bands())
    }

    fn first_band(&self) -> u32 {
        if self.next == 0 { 0 } else { self.ends[self.next - 1] }
    }
}

/// Why a frame produced no picture.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The frame did not decode (malformed, or the sequence does not match): counted, and the
    /// host is asked for a new IDR, as on the CPU path.
    Frame(String),
    /// The frame lost too much of its data on the way: counted and an IDR asked for as with
    /// `Frame`, but it says nothing about the decoder, so it never ends the stream.
    Loss(String),
    /// Every plane set is held by a picture that has not been shown or dropped yet: the frame is
    /// dropped and counted with the replaced ones.
    NoPlanes,
    /// The GPU decoder itself broke (a failed submit, a decode that never finished, a lost
    /// device): nothing falls back to the CPU, the stream ends with this message.
    Device(String),
}

/// Brings the records cache up to date with a frame that is not decoded, so the frames after it
/// recover what they lose from it and not from an older frame. `early`: every chunk since
/// `begin_frame` was fed and they are as long as `data`, which then only needs finishing if it
/// is the records envelope they belong to.
fn pass_records(records: &mut Records, early: bool, data: &[u8]) {
    let whole_early = early && data.starts_with(&[0, 0, 0, 0]);
    if !(whole_early && records.finish_frame(|_| true)) {
        // Begins the frame anew, and refuses what is not a records envelope.
        records.push_frame(data, |_| true);
    }
}

/// The PyroWave decoder of a stream, on the CPU or on the GPU (`Settings::pyrowave_decoder`).
pub enum Pyrowave {
    Cpu(Box<Decoder>),
    Gpu(Box<GpuStream>),
}

impl Pyrowave {
    pub fn cpu(width: usize, height: usize, chroma444: bool) -> Result<Self, &'static str> {
        Decoder::new(width, height, chroma444).map(|decoder| Self::Cpu(Box::new(decoder)))
    }

    /// Takes one packet of the records envelope of the frame `frame` will get next.
    pub fn chunk(&mut self, first: bool, flags: u32, payload: &[u8]) {
        match self {
            Self::Cpu(decoder) => decoder.chunk(first, flags, payload),
            Self::Gpu(stream) => stream.chunk(first, flags, payload),
        }
    }

    /// Decodes the picture in `slices` slices as the bands of a row-ordered frame complete (the
    /// beam race). Only the GPU decoder does; the CPU decoder says so, loudly, to its caller.
    pub fn set_beam_race(&mut self, slices: u32) -> Result<(), &'static str> {
        let _ = slices;
        match self {
            Self::Cpu(_) => Err("beam racing needs the GPU PyroWave decoder, so frames decode whole"),
            #[cfg(windows)]
            Self::Gpu(stream) => { stream.set_slices(slices); Ok(()) }
            #[cfg(not(windows))]
            Self::Gpu(_) => Err("beam racing is Windows-only so far, so frames decode whole"),
        }
    }

    /// The slices decoded while the frame in flight arrived, since the last call.
    pub fn take_slices(&mut self) -> Vec<SliceReady> {
        match self {
            Self::Cpu(_) => Vec::new(),
            Self::Gpu(stream) => stream.take_slices(),
        }
    }

    /// Where the picture of the last `frame` came from, also after a frame that failed (the
    /// early and fallback counters count those too).
    pub fn source(&self) -> Source {
        match self {
            Self::Cpu(decoder) => decoder.source,
            Self::Gpu(stream) => stream.source,
        }
    }

    /// Reconstructs the frame. `evict` drops the oldest picture still queued for the presenter
    /// and says whether there was one; the GPU decoder calls it while no plane set is free.
    pub fn frame(&mut self, data: &[u8], evict: &mut dyn FnMut() -> bool) -> Result<Output, Failure> {
        match self {
            Self::Cpu(decoder) => {
                decoder.coefficients(data).map_err(|e| Failure::Frame(e.into()))?;
                let frame = decoder.coefficient_frame().map_err(Failure::Frame)?;
                Ok(Output { picture: Picture::Frame(frame), loss_percent: decoder.loss_percent, first_block_us: decoder.first_block_us,
                    bands: Vec::new(), race: RaceReport::default(), race_seq: 0 })
            }
            Self::Gpu(stream) => stream.frame(data, evict),
        }
    }

    /// Whether a frame may be passed over for a newer one ([`Pyrowave::skip`]): every frame
    /// decodes on its own, but the slices of a beam-raced frame are with the presenter before
    /// the frame is complete.
    pub fn skips_frames(&self) -> bool {
        match self {
            Self::Cpu(_) => true,
            Self::Gpu(stream) => stream.race.is_none(),
        }
    }

    /// Passes over the frame `frame` would have decoded: no picture, and no decode time spent.
    /// The records cache takes the frame all the same, for the frames after it.
    pub fn skip(&mut self, data: &[u8]) {
        match self {
            Self::Cpu(decoder) => decoder.skip(data),
            Self::Gpu(stream) => stream.skip(data),
        }
    }

    /// The stream starts over on this decoder (a reconnect that reuses it): drops any frame in
    /// flight, so no picture of the last stream can come out.
    pub fn flush(&mut self) {
        match self {
            Self::Cpu(decoder) => decoder.flush(),
            Self::Gpu(stream) => stream.flush(),
        }
    }

    /// Whether a stream may start over on this decoder: a GPU decoder whose device the watchdog
    /// took for hung is never called again.
    pub fn reusable(&self) -> bool {
        match self {
            Self::Cpu(_) => true,
            // Unix (Linux and macOS): a GPU decoder whose watchdog timed out is never reused.
            #[cfg(unix)]
            Self::Gpu(stream) => !stream.gpu.stalled(),
            // Non-Unix (Windows): D3D11 fence deadlines detect stalls per draw.
            #[cfg(not(unix))]
            Self::Gpu(_) => true,
        }
    }

    pub fn stage_summary(&self) -> Option<String> {
        match self {
            Self::Cpu(decoder) => decoder.stage_summary(),
            Self::Gpu(stream) => stream.stage_summary(),
        }
    }

    /// The log lines a GPU decoder adds at the stream's end: the SDK's GPU time per frame and the
    /// GPU's clocks, each where available.
    pub fn gpu_report(&self) -> Vec<String> {
        match self {
            Self::Cpu(_) => Vec::new(),
            Self::Gpu(stream) => [
                stream.pass_summary().map(|passes| format!("PyroWave GPU passes {passes}")),
                stream.clock_summary().map(|clocks| format!("GPU during the stream: {clocks}")),
            ].into_iter().flatten().collect(),
        }
    }
}

#[cfg(all(test, windows))]
mod gpu_tests;

#[cfg(all(test, target_os = "linux"))]
mod gpu_tests_unix;

#[cfg(test)]
mod race_tests;

#[cfg(test)]
mod tests;
