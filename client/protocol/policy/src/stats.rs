//! The once-a-second stats tick every policy reads.

use std::collections::VecDeque;

use serde_json::Value;

use crate::trace::{Args, Subject};

/// One stats tick, passed by reference to every controller and the stream doctor. A measurement
/// is `None` when unknown, so [`Stats::default`] is a tick at time 0 with nothing measured: no
/// controller reads it as healthy.
///
/// Each policy also reads a negative or NaN measurement as unknown, as its Java original does,
/// but callers pass `None` instead. The trace adapters alone pass Java's -1 and NaN through.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Monotonic time of the tick.
    pub now_ms: u64,
    /// Frames arrived in the last two seconds, so the counters below are current.
    pub recent_video: bool,
    /// The native connection-quality warning.
    pub poor_connection: bool,
    /// The stream's frame rate, 0 when unknown. Only the stream doctor reads it; the controllers
    /// keep the rate they were given.
    pub fps: u32,
    /// Frames lost after FEC, in percent.
    pub loss_percent: Option<f32>,
    pub decode_ms: Option<f32>,
    pub jitter_ms: Option<f32>,
    /// Network queueing delay above its recent minimum.
    pub queue_ms: Option<f32>,
    pub host_p95_ms: Option<f32>,
    pub display_p95_ms: Option<f32>,
    pub received_fps: Option<f32>,
    pub released_fps: Option<f32>,
    pub shown_fps: Option<f32>,
    /// True when `host_p95_ms` runs to the last packet sent, as the host's timing reports measure it.
    pub host_includes_send: bool,
    /// The thermal monitor's level: 0 normal (or no thermal reading), 1 warm, 2 and up hot.
    pub thermal_level: u32,
    /// Frames of this tick that arrived late against the host's clock ([`LateFrames`]).
    pub late_frames: Option<u32>,
}

/// Frames that arrived at least [`late_threshold_us`] later than the host's timestamps put them
/// after the frame before, for [`Stats::late_frames`]: a network (or this device's Wi-Fi)
/// holding frames back and handing them over in a burst. A still desktop, a game that slows down
/// or a capture stall, where the timestamps step as far as the arrivals, is never late.
///
/// The timestamps are taken at capture, so a frame the host itself sent later after its capture
/// than the frame before (a slower encode, a wait for its turn, paced sending after a loss) also
/// arrives later, and that change comes off. The host's timing report for the frame tells it:
/// present to last packet sent less the send stage, i.e. present to the first packet sent, which
/// is when the measured first packet left. Frames and reports are matched by frame number. Without
/// a report for the frame or the one before, the change of the frame header's processing time
/// comes off instead, which covers only packetizing.
///
/// Once the host has sent a report, a frame waits for its own (reports follow the frame's last
/// packet) until frames [`REPORT_WAIT_US`] newer arrive, then counts without it. A late frame
/// counts in the tick it is judged in.
#[derive(Clone, Debug, Default)]
pub struct LateFrames {
    /// The last frame judged.
    last: Option<Arrival>,
    /// Frames waiting for their timing report, oldest first.
    pending: VecDeque<Arrival>,
    /// Reports that came before their frame: frame number, present to first packet sent (µs).
    early: VecDeque<(u32, u32)>,
    /// The host sends timing reports: frames wait for theirs.
    reports: bool,
    count: u32,
}

#[derive(Clone, Copy, Debug)]
struct Arrival {
    frame: u32,
    /// RTP timestamp (90 kHz).
    rtp: u32,
    /// The first packet's arrival (µs).
    receive_us: u64,
    /// The frame header's host processing time (µs, 0 unknown).
    processing_us: u32,
    /// The timing report's present to first packet sent (µs).
    first_sent_us: Option<u32>,
    fps: u32,
}

/// Timestamp steps longer than this start over (a new stream or a long pause).
const MAX_HOST_STEP_US: u64 = 1_000_000;
/// A frame waits for its timing report until a frame this much newer arrives.
pub const REPORT_WAIT_US: u64 = 250_000;
/// Frames waiting for reports past this count without them: a quarter second at 2000 fps.
const MAX_PENDING: usize = 512;
/// Reports kept for frames that have not arrived yet.
const MAX_EARLY: usize = 64;

/// How late a frame must be to count: three quarters of a frame interval, at least 8 ms. At
/// that delay the frame misses its refresh and the one before is shown twice.
pub fn late_threshold_us(fps: u32) -> u64 {
    if fps == 0 {
        return u64::MAX;
    }
    (750_000 / u64::from(fps)).max(8_000)
}

impl LateFrames {
    /// Frame `frame` arrived (its first packet, at `receive_us`) with the RTP timestamp `rtp`
    /// after `processing_us` of host processing as its header says (0 when the host does not
    /// report it).
    pub fn push(&mut self, frame: u32, rtp: u32, processing_us: u32, receive_us: u64, fps: u32) {
        let at = self.early.iter().position(|&(early, _)| early == frame);
        let first_sent_us = at.and_then(|at| self.early.remove(at)).map(|(_, us)| us);
        self.pending.push_back(Arrival {
            frame,
            rtp,
            receive_us,
            processing_us,
            first_sent_us,
            fps,
        });
        self.settle(Some(receive_us));
    }

    /// The host's timing report for frame `frame`: present to last packet sent (`host_us`) and
    /// its send stage, first to last packet (`send_us`).
    pub fn report(&mut self, frame: u32, host_us: u32, send_us: u32) {
        self.reports = true;
        let first_sent_us = host_us.saturating_sub(send_us);
        if let Some(waiting) = self.pending.iter_mut().find(|p| p.frame == frame) {
            waiting.first_sent_us = Some(first_sent_us);
        } else if self
            .last
            .map_or(true, |last| frame.wrapping_sub(last.frame) as i32 > 0)
        {
            if self.early.len() == MAX_EARLY {
                self.early.pop_front();
            }
            self.early.push_back((frame, first_sent_us));
        }
        self.settle(None);
    }

    /// Judges the waiting frames, oldest first, that have their report, have waited until the
    /// arrival at `now_us`, or need none because the host sends no reports.
    fn settle(&mut self, now_us: Option<u64>) {
        while let Some(&next) = self.pending.front() {
            let ready = !self.reports
                || next.first_sent_us.is_some()
                || now_us.is_some_and(|now| now.saturating_sub(next.receive_us) >= REPORT_WAIT_US)
                || self.pending.len() > MAX_PENDING;
            if !ready {
                break;
            }
            self.pending.pop_front();
            self.judge(next);
        }
    }

    fn judge(&mut self, frame: Arrival) {
        if let Some(last) = self.last {
            let ticks = frame.rtp.wrapping_sub(last.rtp) as i32;
            let host_us = u64::from(ticks.max(0) as u32) * 1000 / 90;
            if ticks > 0 && host_us <= MAX_HOST_STEP_US && frame.receive_us >= last.receive_us {
                // Only a change of the host's own time moves the arrival, and only when both are
                // known: from the reports, else from the headers.
                let host_step_us = match (frame.first_sent_us, last.first_sent_us) {
                    (Some(sent), Some(last_sent)) => i64::from(sent) - i64::from(last_sent),
                    _ if frame.processing_us > 0 && last.processing_us > 0 => {
                        i64::from(frame.processing_us) - i64::from(last.processing_us)
                    }
                    _ => 0,
                };
                let late_us =
                    (frame.receive_us - last.receive_us) as i64 - host_us as i64 - host_step_us;
                if late_us >= 0 && late_us as u64 >= late_threshold_us(frame.fps) {
                    self.count += 1;
                }
            }
        }
        self.last = Some(frame);
    }

    /// The late frames judged since the last tick, then starts the next one.
    pub fn tick(&mut self) -> u32 {
        std::mem::take(&mut self.count)
    }

    /// Forgets every frame and report, as when the stream changes mode.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

pub(crate) fn traced_late(args: &Value) -> Result<Box<dyn Subject>, String> {
    Args::new(args, 0)?;
    Ok(Box::new(LateFrames::default()))
}

/// Android replays these through `LateFramesTrace`, which turns the RTP timestamp into its
/// microseconds as the decoder gets them.
impl Subject for LateFrames {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            // A frame without its number, as from a host that sends no timing reports.
            "push" => {
                let a = Args::new(args, 4)?;
                self.push(0, a.int(0)?, a.int(1)?, a.int(2)?, a.int(3)?);
                Ok(Value::Null)
            }
            "push_frame" => {
                let a = Args::new(args, 5)?;
                self.push(a.int(0)?, a.int(1)?, a.int(2)?, a.int(3)?, a.int(4)?);
                Ok(Value::Null)
            }
            "report" => {
                let a = Args::new(args, 3)?;
                self.report(a.int(0)?, a.int(1)?, a.int(2)?);
                Ok(Value::Null)
            }
            "tick" => {
                Args::new(args, 0)?;
                Ok(self.tick().into())
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

/// A measured value: at least 0, which also drops NaN.
pub(crate) fn known(value: Option<f32>) -> Option<f32> {
    value.filter(|v| *v >= 0.0)
}

/// The 95th percentile of a stage over the last two stats ticks, for [`Stats::host_p95_ms`] and
/// [`Stats::display_p95_ms`]. Android's `FrameLatencyStats` summarizes the last two seconds the
/// same way: the sample at rank ceil(0.95 n) of the sorted values. Values go in as microseconds.
#[derive(Clone, Debug, Default)]
pub struct RecentP95 {
    previous: Vec<u32>,
    current: Vec<u32>,
}

impl RecentP95 {
    /// Samples kept per tick: a second of a 1000 fps stream, plus slack.
    pub const MAX_SAMPLES: usize = 1024;

    /// Records one sample of the current tick; samples past [`Self::MAX_SAMPLES`] are dropped.
    pub fn push(&mut self, us: u64) {
        if self.current.len() < Self::MAX_SAMPLES {
            self.current.push(u32::try_from(us).unwrap_or(u32::MAX));
        }
    }

    /// Whether this tick or the previous one has a sample.
    pub fn has_samples(&self) -> bool {
        !self.previous.is_empty() || !self.current.is_empty()
    }

    /// The p95 of this tick's and the previous tick's samples in ms, `None` without any, then
    /// starts the next tick.
    pub fn tick(&mut self) -> Option<f32> {
        let mut all: Vec<u32> = self.previous.iter().chain(&self.current).copied().collect();
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        p95_ms(&mut all)
    }

    /// [`tick`](Self::tick), but no more than this tick's own p95, and `None` when this tick has
    /// no sample: a spike counts in the tick it fell in, not again in the next one. Sustained
    /// slowness is in both ticks and counts in each.
    pub fn tick_once(&mut self) -> Option<f32> {
        let own = p95_ms(&mut self.current.clone());
        let both = self.tick();
        own.zip(both).map(|(own, both)| own.min(both))
    }

    /// Forgets every sample, as when the stream changes mode.
    pub fn clear(&mut self) {
        self.previous.clear();
        self.current.clear();
    }
}

/// The nearest-rank p95 of `values` (microseconds) in ms, `None` without any.
fn p95_ms(values: &mut [u32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let rank = (values.len() * 95).div_ceil(100);
    Some(values[rank - 1] as f32 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p95_is_the_nearest_rank_over_two_ticks() {
        let mut p95 = RecentP95::default();
        assert_eq!(p95.tick(), None);
        assert!(!p95.has_samples());
        // 1..=10 ms in the first tick, 11..=20 ms in the second: rank ceil(19) of 20 is 19 ms.
        for ms in 1..=10 {
            p95.push(ms * 1000);
        }
        assert_eq!(p95.tick(), Some(10.0));
        for ms in (11..=20).rev() {
            p95.push(ms * 1000);
        }
        assert_eq!(p95.tick(), Some(19.0));
        // The first tick has aged out; the second still counts once.
        assert!(p95.has_samples());
        assert_eq!(p95.tick(), Some(20.0));
        assert!(!p95.has_samples());
        assert_eq!(p95.tick(), None);
    }

    // Regression (nightjar#80): one startup spike in the two-tick window gave two HOST votes.
    #[test]
    fn tick_once_counts_a_spike_in_its_own_tick_only() {
        let mut p95 = RecentP95::default();
        for _ in 0..99 {
            p95.push(3_000);
        }
        p95.push(80_000);
        for _ in 0..19 {
            p95.push(80_000);
        }
        assert_eq!(p95.tick_once(), Some(80.0), "the spike's own tick");
        for _ in 0..100 {
            p95.push(3_000);
        }
        assert_eq!(p95.tick_once(), Some(3.0), "not again in the next tick");
        assert_eq!(p95.tick_once(), None, "a tick without samples says nothing");
        // Sustained: in every tick.
        for _ in 0..2 {
            for _ in 0..100 {
                p95.push(40_000);
            }
            assert_eq!(p95.tick_once(), Some(40.0));
        }
    }

    #[test]
    fn one_sample_is_its_own_p95_and_a_tick_is_bounded() {
        let mut p95 = RecentP95::default();
        p95.push(2_500);
        assert_eq!(p95.tick(), Some(2.5));
        p95.clear();
        assert!(!p95.has_samples());
        for _ in 0..RecentP95::MAX_SAMPLES + 10 {
            p95.push(1_000);
        }
        p95.push(u64::MAX);
        assert_eq!(p95.current.len(), RecentP95::MAX_SAMPLES);
        assert_eq!(p95.tick(), Some(1.0));
    }

    #[test]
    fn a_frame_is_late_when_it_arrives_later_than_the_host_sent_it() {
        let mut late = LateFrames::default();
        // 60 fps: 1500 ticks of 90 kHz apart. Arrivals on time, then a 40 ms hold and two frames
        // back to back behind it.
        let frames = [(0u32, 0u64), (1500, 16_700), (3000, 33_300), (4500, 73_300), (6000, 73_900), (7500, 74_500)];
        for (rtp, at) in frames {
            late.push(0, rtp, 0, at, 60);
        }
        assert_eq!(late.tick(), 1, "only the frame after the hold is late");
        assert_eq!(late.tick(), 0, "a tick starts the count over");
        // A still desktop or a capture stall: the timestamps step as far as the arrivals.
        late.push(0, 7500 + 9000, 0, 174_500, 60);
        // A timestamp from before the last one is not judged; then the RTP clock wraps.
        late.push(0, u32::MAX - 499, 0, 180_000, 60);
        late.push(0, 1000, 0, 196_700, 60);
        assert_eq!(late.tick(), 0);
        // A new stream's clock is not judged either.
        late.push(0, 1_000_000, 0, 200_000, 60);
        late.push(0, 1_000_000 + 1500, 0, 400_000, 60);
        late.push(0, 1_000_000 + 900_000, 0, 900_000, 60);
        assert_eq!(late.tick(), 1, "only the 183 ms hold of one frame interval");
        assert_eq!(late_threshold_us(60), 12_500);
        assert_eq!(late_threshold_us(120), 8_000);
        assert_eq!(late_threshold_us(0), u64::MAX);
    }

    #[test]
    fn a_frame_the_host_took_longer_to_process_is_not_late() {
        let mut late = LateFrames::default();
        // 60 fps on a clean network, but the host's processing time jumps from 3 to 40 ms: that
        // frame arrives 37 ms after its step, the two behind it waited for it and come right after.
        let frames = [
            (0u32, 3_000u32, 3_000u64),
            (1500, 3_000, 19_700),
            (3000, 40_000, 73_300),
            (4500, 24_000, 74_000),
            (6000, 8_000, 74_700),
            (7500, 3_000, 86_300),
        ];
        for (rtp, processing, at) in frames {
            late.push(0, rtp, processing, at, 60);
        }
        assert_eq!(late.tick(), 0, "the host's processing spike alone is no late frame");
        // The same arrivals without a processing time are judged by the timestamps alone.
        late.clear();
        for (rtp, _, at) in frames {
            late.push(0, rtp, 0, at, 60);
        }
        assert_eq!(late.tick(), 1);
        // A network hold on top of a faster frame still counts: 40 ms late, 5 ms quicker to process.
        late.clear();
        late.push(0, 20_000, 8_000, 300_000, 60);
        late.push(0, 21_500, 3_000, 356_700, 60);
        assert_eq!(late.tick(), 1);
    }
}
