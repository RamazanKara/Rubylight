//! The once-a-second stats tick every policy reads.

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
/// after the frame before, for [`Stats::late_frames`]. That is a network (or this device's Wi-Fi)
/// holding frames back and handing them over in a burst: the host sent them on time, so a
/// still desktop, a game that slows down or a capture stall, where the timestamps step as far
/// as the arrivals, is never late. The timestamps are taken at capture, so a frame the host took
/// longer to encode and send than the one before also arrives later: its processing time
/// (the frame header's, as for the queue delay) comes off.
#[derive(Clone, Debug, Default)]
pub struct LateFrames {
    /// The last frame's RTP timestamp (90 kHz), arrival and host processing time (µs, 0 unknown).
    last: Option<(u32, u64, u32)>,
    count: u32,
}

/// Timestamp steps longer than this start over (a new stream or a long pause).
const MAX_HOST_STEP_US: u64 = 1_000_000;

/// How late a frame must be to count: three quarters of a frame interval, at least 8 ms. At
/// that delay the frame misses its refresh and the one before is shown twice.
pub fn late_threshold_us(fps: u32) -> u64 {
    if fps == 0 {
        return u64::MAX;
    }
    (750_000 / u64::from(fps)).max(8_000)
}

impl LateFrames {
    /// A frame arrived (its first packet, at `receive_us`) with the RTP timestamp `rtp` after
    /// `processing_us` of host processing (0 when the host does not report it).
    pub fn push(&mut self, rtp: u32, processing_us: u32, receive_us: u64, fps: u32) {
        if let Some((last_rtp, last_us, last_processing_us)) = self.last {
            let ticks = rtp.wrapping_sub(last_rtp) as i32;
            let host_us = u64::from(ticks.max(0) as u32) * 1000 / 90;
            if ticks > 0 && host_us <= MAX_HOST_STEP_US && receive_us >= last_us {
                // Only a change of processing time moves the arrival, and only when both are known.
                let processing_step_us = if processing_us > 0 && last_processing_us > 0 {
                    i64::from(processing_us) - i64::from(last_processing_us)
                } else {
                    0
                };
                let late_us = (receive_us - last_us) as i64 - host_us as i64 - processing_step_us;
                if late_us >= 0 && late_us as u64 >= late_threshold_us(fps) {
                    self.count += 1;
                }
            }
        }
        self.last = Some((rtp, receive_us, processing_us));
    }

    /// The late frames since the last tick, then starts the next one.
    pub fn tick(&mut self) -> u32 {
        std::mem::take(&mut self.count)
    }

    /// Forgets the last frame, as when the stream changes mode.
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
            "push" => {
                let a = Args::new(args, 4)?;
                self.push(a.int(0)?, a.int(1)?, a.int(2)?, a.int(3)?);
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
        if all.is_empty() {
            return None;
        }
        all.sort_unstable();
        let rank = (all.len() * 95).div_ceil(100);
        Some(all[rank - 1] as f32 / 1000.0)
    }

    /// Forgets every sample, as when the stream changes mode.
    pub fn clear(&mut self) {
        self.previous.clear();
        self.current.clear();
    }
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
            late.push(rtp, 0, at, 60);
        }
        assert_eq!(late.tick(), 1, "only the frame after the hold is late");
        assert_eq!(late.tick(), 0, "a tick starts the count over");
        // A still desktop or a capture stall: the timestamps step as far as the arrivals.
        late.push(7500 + 9000, 0, 174_500, 60);
        // A timestamp from before the last one is not judged; then the RTP clock wraps.
        late.push(u32::MAX - 499, 0, 180_000, 60);
        late.push(1000, 0, 196_700, 60);
        assert_eq!(late.tick(), 0);
        // A new stream's clock is not judged either.
        late.push(1_000_000, 0, 200_000, 60);
        late.push(1_000_000 + 1500, 0, 400_000, 60);
        late.push(1_000_000 + 900_000, 0, 900_000, 60);
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
            late.push(rtp, processing, at, 60);
        }
        assert_eq!(late.tick(), 0, "the host's processing spike alone is no late frame");
        // The same arrivals without a processing time are judged by the timestamps alone.
        late.clear();
        for (rtp, _, at) in frames {
            late.push(rtp, 0, at, 60);
        }
        assert_eq!(late.tick(), 1);
        // A network hold on top of a faster frame still counts: 40 ms late, 5 ms quicker to process.
        late.clear();
        late.push(20_000, 8_000, 300_000, 60);
        late.push(21_500, 3_000, 356_700, 60);
        assert_eq!(late.tick(), 1);
    }
}
