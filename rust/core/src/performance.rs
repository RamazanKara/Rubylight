//! Bounded per-session measurements. No sampling thread or disk writes.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
#[derive(Clone, Copy)]
struct Frame {
    at: Instant,
    latency: u64,
    processing: u64,
    age: u64,
    sent: u64,
    interval: Option<u64>,
    bytes: u64,
}
/// One second's totals for the history graph.
#[derive(Clone, Copy)]
struct Second {
    fps: f64,
    bitrate_mbps: f64,
    encode_mean_ms: f64,
    encode_max_ms: f64,
    host_processing_mean_ms: f64,
    host_processing_max_ms: f64,
    frame_age_mean_ms: f64,
}
impl Second {
    fn json(&self) -> Value {
        json!({"fps":self.fps,"bitrate_mbps":self.bitrate_mbps,"encode_mean_ms":self.encode_mean_ms,"encode_max_ms":self.encode_max_ms,"host_processing_mean_ms":self.host_processing_mean_ms,"host_processing_max_ms":self.host_processing_max_ms,"frame_age_mean_ms":self.frame_age_mean_ms})
    }
}
struct FecFrame {
    index: u32,
    blocks: u8,
    block_count: u8,
    recovered: bool,
    unrecoverable: bool,
}
/// The wire frames block reports are kept for: the last sent one and those before it.
const FEC_WINDOW: u32 = 1024;
/// The recent wire frames' block reports, one slot per frame index modulo
/// [`FEC_WINDOW`]: the frames inside the window each have a slot of their own,
/// so a report finds its frame without a search.
#[derive(Default)]
struct FecFrames {
    /// Empty until the first report.
    slots: Vec<Option<FecFrame>>,
    len: usize,
    /// The last sent frame the window was last moved to.
    last: Option<u32>,
}
impl FecFrames {
    /// Drops the frames that are not among the [`FEC_WINDOW`] up to `last`, as a
    /// `retain` over all of them would. Only the slots of frames that left the
    /// window since the previous call are looked at; a jump of a window or more,
    /// or back, looks at them all.
    fn advance(&mut self, last: u32) {
        if self.slots.is_empty() {
            self.slots.resize_with(FEC_WINDOW as usize, || None);
        }
        let step = self.last.map(|previous| last.wrapping_sub(previous));
        self.last = Some(last);
        match step {
            Some(0) => {}
            // Every kept frame was inside the previous window, so only the `step`
            // oldest of it can be outside this one.
            Some(step) if step < FEC_WINDOW => {
                let first = last.wrapping_sub(step).wrapping_sub(FEC_WINDOW - 1);
                for offset in 0..step {
                    self.evict(first.wrapping_add(offset) % FEC_WINDOW, last);
                }
            }
            _ => {
                for slot in 0..FEC_WINDOW {
                    self.evict(slot, last);
                }
            }
        }
    }
    fn evict(&mut self, slot: u32, last: u32) {
        let slot = &mut self.slots[slot as usize];
        if slot
            .as_ref()
            .is_some_and(|f| last.wrapping_sub(f.index) >= FEC_WINDOW)
        {
            *slot = None;
            self.len -= 1;
        }
    }
    /// Frame `index`'s record, made with `block_count` blocks if it has none.
    /// `index` is inside the window [`Self::advance`] last moved to.
    fn frame(&mut self, index: u32, block_count: u8) -> &mut FecFrame {
        let slot = &mut self.slots[(index % FEC_WINDOW) as usize];
        // Another frame in this slot is outside the window; `advance` already dropped it.
        if slot.as_ref().is_some_and(|f| f.index != index) {
            *slot = None;
            self.len -= 1;
        }
        if slot.is_none() {
            self.len += 1;
        }
        slot.get_or_insert_with(|| FecFrame {
            index,
            blocks: 0,
            block_count,
            recovered: false,
            unrecoverable: false,
        })
    }
}
/// Microseconds spent on one frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    /// Negotiated frame interval, used to count gaps above twice that interval.
    pub period: Duration,
    /// Claim through completed codec output, including asynchronous work.
    pub encode: u64,
    /// Capture/encode output tick through entry to packetization, as Punktfunk's
    /// GameStream field: excludes the prepared packets' send queue and pacing.
    pub host: u64,
    /// Legacy capture timestamp through the claim. WGC's stamp is not app
    /// Present time and may be clamped to arrival; this is an age estimate.
    /// Moonlight never sees this. Static repeats use a refreshed timestamp.
    pub age: u64,
    /// Claim through sending the final packet, including packetization and pacing.
    pub sent: u64,
}
/// The points one new picture passed on the host, from Windows' present to
/// its last packet. Static repeats have no present and are not recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamps {
    /// The capture's present stamp: Desktop Duplication's LastPresentTime or
    /// WGC's SystemRelativeTime. A stamp in the future or over two seconds
    /// old is the capture's arrival instead, as the pacing clock has it.
    pub presented: Instant,
    /// The host submitted its own copy of the picture, or took a WGC helper
    /// texture to read in place.
    pub acquired: Instant,
    /// Published to the stream threads. The WGC helper can hold a picture
    /// between acquiring and publishing it, to publish on the claim grid.
    pub published: Instant,
    /// The stream thread took the picture to encode.
    pub claimed: Instant,
    /// The encoder accepted the converted picture. An encoder that does not
    /// say when gives the claim, which puts its conversion in `encode`.
    pub submitted: Instant,
    /// The encoded frame was in the host's hands.
    pub encoded: Instant,
    /// Packetization began.
    pub packetized: Instant,
    /// Packetization completed, before the paced sender's queue.
    pub prepared: Instant,
    /// The first packets were handed to the socket.
    pub first_sent: Instant,
    /// The last packet was handed to the socket.
    pub last_sent: Instant,
    /// Set for a frame sent while the encoder still produced it
    /// (`video_slice_delivery`). Its `first_sent` can precede `encoded`.
    pub streamed: Option<Streamed>,
}
/// What a streamed frame adds to its [`Stamps`]: the frame's first blocks left
/// before its last unit was in hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Streamed {
    /// The first unit was in the host's hands. `Stamps::encoded` is the last.
    pub first_unit: Instant,
    /// Blocks sent before the last unit arrived.
    pub early_blocks: u32,
}
/// The stages between consecutive `Stamps`, in their order.
pub const STAGES: [&str; 9] = [
    "capture",
    "hold",
    "claim",
    "convert",
    "encode",
    "queue",
    "packetize",
    "send_queue",
    "send",
];
impl Stamps {
    /// Microseconds spent in each of `STAGES`. A point earlier than the one
    /// before it (a present stamp clamped to arrival) counts as reached with
    /// it, so no stage is negative and the stages add up to `total`.
    pub fn stages(&self) -> [u64; 9] {
        let points = [
            self.presented,
            self.acquired,
            self.published,
            self.claimed,
            self.submitted,
            self.encoded,
            self.packetized,
            self.prepared,
            self.first_sent,
            self.last_sent,
        ];
        let mut reached = points[0];
        let mut stages = [0; 9];
        for (stage, point) in stages.iter_mut().zip(&points[1..]) {
            let next = reached.max(*point);
            *stage = micros(next.duration_since(reached));
            reached = next;
        }
        stages
    }
    /// Present to the last packet sent, in microseconds.
    pub fn total(&self) -> u64 {
        self.stages().iter().sum()
    }
    /// For a streamed frame, how long its first block was on the wire before
    /// its encoder finished (`encoded - first_sent`), in microseconds. Zero
    /// when nothing left early; `None` for a frame that is not streamed.
    pub fn overlap(&self) -> Option<u64> {
        self.streamed?;
        Some(micros(
            self.encoded.saturating_duration_since(self.first_sent),
        ))
    }
    /// These points for a streamed frame; `encoded` is the last unit.
    pub fn with_streamed(mut self, first_unit: Instant, early_blocks: usize) -> Self {
        self.streamed = Some(Streamed {
            first_unit,
            early_blocks: u32::try_from(early_blocks).unwrap_or(u32::MAX),
        });
        self
    }
}
fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}
/// Microseconds from `from` to `to`; zero if `to` is earlier.
fn micros_between(from: Instant, to: Instant) -> u64 {
    micros(to.saturating_duration_since(from))
}
/// A new picture's points up to its encoder submission, kept from its claim
/// until its encoded frame is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    /// The presentation time the encoder returns with this picture's frame.
    pub key: Instant,
    pub presented: Instant,
    pub acquired: Instant,
    pub published: Instant,
    pub claimed: Instant,
    pub submitted: Instant,
}
impl Claim {
    pub fn sent(
        &self,
        encoded: Instant,
        packetized: Instant,
        first_sent: Instant,
        last_sent: Instant,
    ) -> Stamps {
        Stamps {
            presented: self.presented,
            acquired: self.acquired,
            published: self.published,
            claimed: self.claimed,
            submitted: self.submitted,
            encoded,
            packetized,
            // Inline packetization interleaves with sends. Its old packetize
            // stage ends at the first send; it has no separate send queue.
            prepared: first_sent,
            first_sent,
            last_sent,
            streamed: None,
        }
    }
}
/// Claims of new pictures whose encoded frames are not sent yet.
#[derive(Default)]
pub struct Claims(VecDeque<Claim>);
impl Claims {
    /// More than any encoder holds in flight.
    const LIMIT: usize = 16;
    pub fn push(&mut self, claim: Claim) {
        if self.0.len() == Self::LIMIT {
            self.0.pop_front();
        }
        self.0.push_back(claim);
    }
    /// The claim of the picture whose frame the encoder returned with
    /// `presentation`. Encoders return frames in order, so the claims before
    /// it got no frame and are dropped. A static repeat has no claim.
    pub fn take(&mut self, presentation: Option<Instant>) -> Option<Claim> {
        let index = self.0.iter().position(|c| Some(c.key) == presentation)?;
        self.0.drain(..index);
        self.0.pop_front()
    }
}
/// Stage samples are kept as long as the 5 s timing log apart, so each line
/// covers the pictures since the last one.
const STAGE_WINDOW: Duration = Duration::from_secs(5);
/// Bounds the window to 400 new pictures a second.
const STAGE_SAMPLES: usize = 2048;
struct StageSample {
    at: Instant,
    /// `STAGES`, then the total, then Moonlight's host latency field for the
    /// same picture, then the capture's age when the pacer claimed it and when
    /// the encoder took it (from its acquire), in microseconds.
    micros: [u32; STAGES.len() + 4],
}
#[derive(Default)]
pub struct Performance {
    frames: VecDeque<Frame>,
    stages: VecDeque<StageSample>,
    history: VecDeque<Second>,
    bucket: Option<Instant>,
    count: u64,
    bytes: u64,
    latency: u64,
    maximum: u64,
    processing: u64,
    processing_maximum: u64,
    age: u64,
    send_stutters: u64,
    fec_reports: u64,
    fec_invalid_reports: u64,
    fec_duplicate_reports: u64,
    fec_recovered_frames: u64,
    fec_unrecoverable_frames: u64,
    /// Gaps before each reported highest sequence, not unsent trailing parity.
    fec_missing_packets: u64,
    fec_frames: FecFrames,
    /// 0x5539 statistical FEC reports, which `fec_reports` (0x5502) doesn't count.
    fec_loss_reports: u64,
    fec_loss_refused_reports: u64,
}
impl Performance {
    /// Counts one 0x5539 statistical FEC report: taken (also when only logged),
    /// or refused as malformed or out of sequence.
    pub fn record_fec_loss_report(&mut self, taken: bool) {
        let counter = if taken {
            &mut self.fec_loss_reports
        } else {
            &mut self.fec_loss_refused_reports
        };
        *counter = counter.saturating_add(1);
    }
    /// Counts one block report and returns what it says, or `None` for an
    /// invalid, unsent, stale or duplicate report.
    pub fn record_fec_status(
        &mut self,
        payload: &[u8],
        last_sent: Option<u32>,
    ) -> Option<crate::adaptive_fec::Block> {
        self.fec_reports = self.fec_reports.saturating_add(1);
        let status = crate::fec_status::Status::parse(payload)
            .ok()
            .zip(last_sent)
            .filter(|(s, last)| last.wrapping_sub(s.frame) < 1024);
        let Some((status, last)) = status else {
            self.fec_invalid_reports = self.fec_invalid_reports.saturating_add(1);
            return None;
        };
        // One entry per recent wire frame bounds storage to 1024, even when
        // reports arrive out of order or the u32 frame counter wraps.
        self.fec_frames.advance(last);
        let frame = self.fec_frames.frame(status.frame, status.blocks);
        if frame.block_count != status.blocks {
            self.fec_invalid_reports = self.fec_invalid_reports.saturating_add(1);
            return None;
        }
        let block = 1 << status.block;
        if frame.blocks & block != 0 {
            self.fec_duplicate_reports = self.fec_duplicate_reports.saturating_add(1);
            return None;
        }
        frame.blocks |= block;
        self.fec_missing_packets = self
            .fec_missing_packets
            .saturating_add(u64::from(status.missing_before_highest));
        // These best-effort block reports infer recovery, not successful decode.
        // A later failed block makes the whole reported frame unrecoverable.
        let unrecoverable = u32::from(status.received_data) + u32::from(status.received_parity)
            < u32::from(status.data);
        if unrecoverable && !frame.unrecoverable {
            frame.unrecoverable = true;
            self.fec_unrecoverable_frames = self.fec_unrecoverable_frames.saturating_add(1);
            if frame.recovered {
                self.fec_recovered_frames = self.fec_recovered_frames.saturating_sub(1);
            }
        } else if !unrecoverable
            && status.received_data < status.data
            && !frame.recovered
            && !frame.unrecoverable
        {
            frame.recovered = true;
            self.fec_recovered_frames = self.fec_recovered_frames.saturating_add(1);
        }
        Some(if unrecoverable {
            crate::adaptive_fec::Block::Unrecoverable
        } else if status.received_data < status.data {
            crate::adaptive_fec::Block::Recovered {
                lost: status.data - status.received_data,
                data: status.data,
            }
        } else {
            crate::adaptive_fec::Block::Clean
        })
    }
    pub fn record(&mut self, now: Instant, latency: u64, bytes: u64) {
        self.record_timing(
            now,
            Timing {
                period: Duration::ZERO,
                encode: latency,
                host: latency,
                age: 0,
                sent: latency,
            },
            bytes,
        );
    }
    pub fn record_timing(&mut self, now: Instant, timing: Timing, bytes: u64) {
        let Timing {
            period,
            encode: latency,
            host: processing,
            age,
            sent,
        } = timing;
        if let Some(start) = self.bucket
            && now.duration_since(start) >= Duration::from_secs(1)
        {
            let elapsed = now.duration_since(start).as_secs_f64();
            let count = self.count.max(1) as f64;
            self.history.push_back(Second {
                fps: self.count as f64 / elapsed,
                bitrate_mbps: self.bytes as f64 * 8. / elapsed / 1_000_000.,
                encode_mean_ms: self.latency as f64 / count / 1000.,
                encode_max_ms: self.maximum as f64 / 1000.,
                host_processing_mean_ms: self.processing as f64 / count / 1000.,
                host_processing_max_ms: self.processing_maximum as f64 / 1000.,
                frame_age_mean_ms: self.age as f64 / count / 1000.,
            });
            if self.history.len() > 120 {
                self.history.pop_front();
            }
            self.bucket = Some(now);
            self.count = 0;
            self.bytes = 0;
            self.latency = 0;
            self.maximum = 0;
            self.processing = 0;
            self.processing_maximum = 0;
            self.age = 0;
        }
        self.bucket.get_or_insert(now);
        self.count += 1;
        self.bytes += bytes;
        self.latency += latency;
        self.maximum = self.maximum.max(latency);
        self.processing += processing;
        self.processing_maximum = self.processing_maximum.max(processing);
        self.age += age;
        let interval = self.frames.back().map(|frame| {
            now.saturating_duration_since(frame.at)
                .as_micros()
                .min(u128::from(u64::MAX)) as u64
        });
        if !period.is_zero() && interval.is_some_and(|us| u128::from(us) > period.as_micros() * 2) {
            self.send_stutters += 1;
        }
        self.frames.push_back(Frame {
            at: now,
            latency,
            processing,
            age,
            sent,
            interval,
            bytes,
        });
        while self.frames.len() > 1024
            || self
                .frames
                .front()
                .is_some_and(|f| now.duration_since(f.at) > Duration::from_secs(2))
        {
            self.frames.pop_front();
        }
    }
    /// Records where a new picture's time went. `moonlight_host` is the host
    /// latency sent to Moonlight for it, in microseconds.
    pub fn record_stages(&mut self, stamps: &Stamps, moonlight_host: u64) {
        let mut micros = [0; STAGES.len() + 4];
        let stages = stamps.stages();
        // The capture's age when the stream takes it: the wait a fixed encode
        // tick adds, unsynchronised to the content's frame phase.
        let age = |to: Instant| micros_between(stamps.acquired, to);
        for (sample, value) in micros.iter_mut().zip(stages.iter().copied().chain([
            stages.iter().sum(),
            moonlight_host,
            age(stamps.claimed),
            age(stamps.submitted),
        ])) {
            *sample = value.min(u64::from(u32::MAX)) as u32;
        }
        // Each picture, for measurements that need every sample
        // (RUST_LOG=info,stages=trace); off by default.
        tracing::trace!(
            target: "stages",
            capture_us = micros[0],
            hold_us = micros[1],
            claim_us = micros[2],
            convert_us = micros[3],
            encode_us = micros[4],
            queue_us = micros[5],
            packetize_us = micros[6],
            send_queue_us = micros[7],
            send_us = micros[8],
            present_to_sent_us = micros[9],
            moonlight_host_us = micros[10],
            acquire_to_claim_us = micros[11],
            acquire_to_submit_us = micros[12],
            "picture"
        );
        let at = stamps.last_sent;
        self.stages.push_back(StageSample { at, micros });
        while self.stages.len() > STAGE_SAMPLES
            || self
                .stages
                .front()
                .is_some_and(|s| at.saturating_duration_since(s.at) > STAGE_WINDOW)
        {
            self.stages.pop_front();
        }
    }
    /// Copies what `snapshot` reports. The send path records into this under
    /// the same lock, so callers take the window, release the lock, and sort
    /// and build the JSON after.
    pub fn window(&self, now: Instant) -> Window {
        Window {
            now,
            frames: self
                .frames
                .iter()
                .filter(|f| now.duration_since(f.at) <= Duration::from_secs(2))
                .copied()
                .collect(),
            stages: self
                .stages
                .iter()
                .filter(|s| now.saturating_duration_since(s.at) <= STAGE_WINDOW)
                .map(|s| s.micros)
                .collect(),
            history: self.history.iter().copied().collect(),
            send_stutters: self.send_stutters,
            fec_reports: self.fec_reports,
            fec_invalid_reports: self.fec_invalid_reports,
            fec_duplicate_reports: self.fec_duplicate_reports,
            fec_recovered_frames: self.fec_recovered_frames,
            fec_unrecoverable_frames: self.fec_unrecoverable_frames,
            fec_missing_packets: self.fec_missing_packets,
            fec_loss_reports: self.fec_loss_reports,
            fec_loss_refused_reports: self.fec_loss_refused_reports,
        }
    }
    pub fn snapshot(&self, now: Instant) -> Value {
        self.window(now).snapshot()
    }
}
/// The measurements a snapshot covers, copied out of `Performance`.
pub struct Window {
    now: Instant,
    frames: Vec<Frame>,
    stages: Vec<[u32; STAGES.len() + 4]>,
    history: Vec<Second>,
    send_stutters: u64,
    fec_reports: u64,
    fec_invalid_reports: u64,
    fec_duplicate_reports: u64,
    fec_recovered_frames: u64,
    fec_unrecoverable_frames: u64,
    fec_missing_packets: u64,
    fec_loss_reports: u64,
    fec_loss_refused_reports: u64,
}
impl Window {
    /// Mean, p95 and p99 of each stage over the last five seconds' new
    /// pictures, in milliseconds.
    fn stage_snapshot(&self) -> Value {
        let spread = |index: usize| {
            let mut values: Vec<u32> = self.stages.iter().map(|s| s[index]).collect();
            values.sort_unstable();
            let ms = |us: u32| f64::from(us) / 1000.;
            let at = |percent: usize| {
                values
                    .get(values.len().saturating_sub(1) * percent / 100)
                    .copied()
                    .map_or(0., ms)
            };
            json!({
                "mean_ms": values.iter().map(|v| f64::from(*v)).sum::<f64>()
                    / values.len().max(1) as f64 / 1000.,
                "p50_ms": at(50),
                "p95_ms": at(95),
                "p99_ms": at(99),
            })
        };
        let mut stages = serde_json::Map::new();
        stages.insert("frames".into(), self.stages.len().into());
        stages.insert("window_seconds".into(), STAGE_WINDOW.as_secs().into());
        for (index, name) in STAGES.iter().enumerate() {
            stages.insert((*name).into(), spread(index));
        }
        stages.insert("present_to_sent".into(), spread(STAGES.len()));
        stages.insert("moonlight_host".into(), spread(STAGES.len() + 1));
        stages.insert("acquire_to_claim".into(), spread(STAGES.len() + 2));
        stages.insert("acquire_to_submit".into(), spread(STAGES.len() + 3));
        Value::Object(stages)
    }
    pub fn snapshot(&self) -> Value {
        let frames = &self.frames;
        let seconds = frames
            .first()
            .map_or(0., |f| self.now.duration_since(f.at).as_secs_f64());
        let sorted = |value: fn(&Frame) -> u64| {
            let mut values: Vec<_> = frames.iter().map(value).collect();
            values.sort_unstable();
            values
        };
        let latencies = sorted(|f| f.latency);
        let processing = sorted(|f| f.processing);
        let ages = sorted(|f| f.age);
        let present_to_send = sorted(|f| f.age.saturating_add(f.sent));
        let mut intervals: Vec<_> = frames.iter().filter_map(|f| f.interval).collect();
        intervals.sort_unstable();
        let fps = if seconds > 0. {
            frames.len().saturating_sub(1) as f64 / seconds
        } else {
            0.
        };
        let bytes = frames.iter().skip(1).map(|f| f.bytes).sum::<u64>();
        let percentile = |values: &[u64], percent: usize| {
            values
                .get(values.len().saturating_sub(1) * percent / 100)
                .copied()
                .unwrap_or(0) as f64
                / 1000.
        };
        let mean = |values: &[u64]| {
            values.iter().map(|v| *v as f64).sum::<f64>() / values.len().max(1) as f64 / 1000.
        };
        json!({
            "fps":fps,
            "bitrate_mbps":if seconds>0. { bytes as f64*8./seconds/1_000_000. } else { 0. },
            "encode_mean_ms":mean(&latencies),
            "encode_p95_ms":percentile(&latencies,95),
            "encode_p99_ms":percentile(&latencies,99),
            "host_processing_min_ms":percentile(&processing,0),
            "host_processing_mean_ms":mean(&processing),
            "host_processing_max_ms":percentile(&processing,100),
            "host_processing_p95_ms":percentile(&processing,95),
            "host_processing_p99_ms":percentile(&processing,99),
            "frame_age_mean_ms":mean(&ages),
            "frame_age_p95_ms":percentile(&ages,95),
            "frame_age_p99_ms":percentile(&ages,99),
            "present_to_send_mean_ms":mean(&present_to_send),
            "present_to_send_p95_ms":percentile(&present_to_send,95),
            "present_to_send_p99_ms":percentile(&present_to_send,99),
            "send_interval_p95_ms":percentile(&intervals,95),
            "send_interval_p99_ms":percentile(&intervals,99),
            "send_interval_max_ms":percentile(&intervals,100),
            "send_stutters":self.send_stutters,
            "fec_reports":self.fec_reports,
            "fec_invalid_reports":self.fec_invalid_reports,
            "fec_duplicate_reports":self.fec_duplicate_reports,
            "fec_recovered_frames":self.fec_recovered_frames,
            "fec_unrecoverable_frames":self.fec_unrecoverable_frames,
            "fec_missing_packets":self.fec_missing_packets,
            "fec_loss_reports":self.fec_loss_reports,
            "fec_loss_refused_reports":self.fec_loss_refused_reports,
            "stages":self.stage_snapshot(),
            "sample_frames":frames.len(),"history":self.history.iter().map(Second::json).collect::<Vec<_>>()
        })
    }
}
#[cfg(test)]
mod tests;
