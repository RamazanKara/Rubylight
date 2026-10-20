//! The measurements and summaries Rubylight's C test client prints, computed the same way.

use std::io::Write;

use nightjar_core::video::{Frame, FrameType};

/// `samples=N mean_ms= p50_ms= p95_ms= p99_ms= max_ms=` of `values`, sorted in place.
pub fn distribution(out: &mut impl Write, name: &str, values: &mut [f64]) {
    if values.is_empty() {
        return;
    }
    let count = values.len();
    let sum: f64 = values.iter().sum();
    values.sort_by(f64::total_cmp);
    let _ = writeln!(
        out,
        "{name} samples={count} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
        sum / count as f64,
        values[(count - 1) / 2],
        values[(count - 1) * 95 / 100],
        values[(count - 1) * 99 / 100],
        values[count - 1]
    );
}

/// Per-frame measurements in arrival order.
#[derive(Default)]
pub struct Measurements {
    pub host_latency: Vec<f64>,
    pub decode_ms: Vec<f64>,
    arrivals: Vec<f64>,
    assembly: Vec<f64>,
    picture_age: Vec<f64>,
    picture_frames: Vec<u32>,
}

/// The steady window after the warmup.
pub struct Steady {
    pub host: Vec<f64>,
    pub intervals: Vec<f64>,
    pub ages: Vec<f64>,
    pub late: u32,
    pub visual: u32,
    pub unique: u32,
    pub repeats: u32,
    pub skips: u32,
    pub seconds: f64,
    pub fps: f64,
}

impl Measurements {
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        limit: usize,
        host: f64,
        decode: f64,
        arrival: f64,
        assembly: f64,
        age: f64,
        picture: u32,
    ) {
        if self.host_latency.len() >= limit {
            return;
        }
        self.host_latency.push(host);
        self.decode_ms.push(decode);
        self.arrivals.push(arrival);
        self.assembly.push(assembly);
        self.picture_age.push(age);
        self.picture_frames.push(picture);
    }

    pub fn steady(&self, warmup_seconds: f64, fps: i32) -> Steady {
        let mut s = Steady {
            host: Vec::new(),
            intervals: Vec::new(),
            ages: Vec::new(),
            late: 0,
            visual: 0,
            unique: 0,
            repeats: 0,
            skips: 0,
            seconds: 0.0,
            fps: 0.0,
        };
        let (mut first, mut last) = (0.0, 0.0);
        let mut last_picture = 0u32;
        let start = self.arrivals.first().copied().unwrap_or(0.0) + warmup_seconds * 1000.0;
        for i in 0..self.arrivals.len() {
            if self.arrivals[i] < start {
                continue;
            }
            s.host.push(self.host_latency[i]);
            if first == 0.0 {
                first = self.arrivals[i];
            }
            last = self.arrivals[i];
            if i > 0 {
                let interval = self.assembly[i] - self.assembly[i - 1];
                s.intervals.push(interval);
                if interval > 1500.0 / f64::from(fps) {
                    s.late += 1;
                }
            }
            let picture = self.picture_frames[i];
            if picture != 0 {
                s.visual += 1;
                if self.picture_age[i] >= 0.0 {
                    s.ages.push(self.picture_age[i]);
                }
                if picture == last_picture {
                    s.repeats += 1;
                } else {
                    s.unique += 1;
                    if last_picture != 0 && picture > last_picture + 1 {
                        s.skips += picture - last_picture - 1;
                    }
                }
                last_picture = picture;
            }
        }
        s.seconds = (last - first) / 1000.0;
        s.fps = if s.seconds > 0.0 && s.host.len() > 1 {
            (s.host.len() - 1) as f64 / s.seconds
        } else {
            0.0
        };
        s
    }
}

#[derive(Clone, Copy, Default)]
struct Probe {
    request_us: u64,
    arrival_us: u64,
    decoded_us: u64,
    wire_frame: i64,
}

/// Keyframe requests during the stream (`BUTTERPOLLO_TEST_IDR_PROBE`): each must bring an
/// IDR received after the request and decoded.
pub struct IdrProbe {
    count: usize,
    interval_us: u64,
    warmup_seconds: f64,
    sent: usize,
    /// The slot awaiting its IDR, plus one; 0 when none.
    pending: usize,
    probes: Vec<Probe>,
}

impl IdrProbe {
    pub fn new(count: usize, interval_us: u64, warmup_seconds: f64) -> Self {
        Self {
            count,
            interval_us,
            warmup_seconds,
            sent: 0,
            pending: 0,
            probes: vec![Probe::default(); count],
        }
    }

    /// Whether a request is due now; the slot is published before the request goes out.
    pub fn poll(&mut self, now_us: u64, started_us: u64, decoded_any: bool) -> bool {
        if self.sent >= self.count || self.pending != 0 || !decoded_any {
            return false;
        }
        if ((now_us - started_us) as f64) < self.warmup_seconds * 1_000_000.0 {
            return false;
        }
        if self.sent != 0 && now_us - self.probes[self.sent - 1].request_us < self.interval_us {
            return false;
        }
        self.probes[self.sent].request_us = now_us;
        self.sent += 1;
        self.pending = self.sent;
        true
    }

    pub fn arrived(&mut self, frame: &Frame) {
        if self.pending == 0 || frame.frame_type != FrameType::Idr {
            return;
        }
        let probe = &mut self.probes[self.pending - 1];
        // IDRs already in flight or queued when the request was made do not count.
        if probe.arrival_us != 0 || frame.receive_time_us < probe.request_us {
            return;
        }
        probe.arrival_us = frame.enqueue_time_us;
        probe.wire_frame = i64::from(frame.number);
    }

    pub fn decoded(&mut self, wire_frame: i64, now_us: u64) {
        if self.pending == 0 {
            return;
        }
        let probe = &mut self.probes[self.pending - 1];
        if probe.arrival_us == 0 || wire_frame != probe.wire_frame {
            return;
        }
        probe.decoded_us = now_us;
        self.pending = 0;
    }

    /// Writes the CSV and the `IDR_PROBE` line; true when every request was answered.
    pub fn summarize(&self, out: &mut impl Write, csv: &mut impl Write) -> bool {
        if self.count == 0 {
            return true;
        }
        let mut values = Vec::new();
        let mut decoded = 0;
        let _ = writeln!(csv, "sample,request_ms,arrival_ms,decoded_ms,request_to_arrival_ms,request_to_decoded_ms,wire_frame,status");
        for (i, p) in self.probes.iter().enumerate() {
            let arrival_ms = if p.arrival_us != 0 {
                (p.arrival_us as f64 - p.request_us as f64) / 1000.0
            } else {
                -1.0
            };
            let decoded_ms = if p.decoded_us != 0 {
                (p.decoded_us as f64 - p.request_us as f64) / 1000.0
            } else {
                -1.0
            };
            if p.arrival_us != 0 {
                values.push(arrival_ms);
            }
            if p.decoded_us != 0 {
                decoded += 1;
            }
            let status = if i >= self.sent {
                "not_requested"
            } else if p.arrival_us == 0 {
                "no_idr"
            } else if p.decoded_us == 0 {
                "not_decoded"
            } else {
                "ok"
            };
            let _ = writeln!(
                csv,
                "{},{:.3},{:.3},{:.3},{:.3},{:.3},{},{}",
                i + 1,
                if i < self.sent {
                    p.request_us as f64 / 1000.0
                } else {
                    -1.0
                },
                if p.arrival_us != 0 {
                    p.arrival_us as f64 / 1000.0
                } else {
                    -1.0
                },
                if p.decoded_us != 0 {
                    p.decoded_us as f64 / 1000.0
                } else {
                    -1.0
                },
                arrival_ms,
                decoded_ms,
                if p.arrival_us != 0 { p.wire_frame } else { -1 },
                status
            );
        }
        let count = values.len();
        let sum: f64 = values.iter().sum();
        values.sort_by(f64::total_cmp);
        let pick = |at: usize| if count != 0 { values[at] } else { f64::NAN };
        let _ = writeln!(
            out,
            "IDR_PROBE samples={count} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} max_ms={:.3} requested={} sent={} decoded={decoded}",
            if count != 0 { sum / count as f64 } else { f64::NAN },
            pick(count.saturating_sub(1) / 2),
            pick(count.saturating_sub(1) * 95 / 100),
            pick(count.saturating_sub(1)),
            self.count,
            self.sent
        );
        count == self.count && decoded == self.count
    }
}

/// Decoded audio: energy and peak over every sample, and the tone's short-window energy
/// range once the decoder has run for two seconds.
pub struct AudioSignal {
    pub samples: u64,
    pub energy: f64,
    pub peak: f64,
    pub tone_blocks: u32,
    pub tone_min: f64,
    pub tone_max: f64,
}

impl Default for AudioSignal {
    fn default() -> Self {
        Self {
            samples: 0,
            energy: 0.0,
            peak: 0.0,
            tone_blocks: 0,
            tone_min: f64::INFINITY,
            tone_max: 0.0,
        }
    }
}

impl AudioSignal {
    /// Adds interleaved samples; returns this packet's tone window energy range.
    pub fn add(&mut self, samples: &[f32], channels: usize, tone: bool) -> (f64, f64) {
        let (mut minimum, mut maximum) = (f64::INFINITY, 0.0f64);
        let count = samples.len() / channels.max(1);
        if tone && self.samples >= 48000 * 2 * channels as u64 {
            // Small windows catch inserted silence even across packet boundaries.
            let mut start = 0;
            while start + 120 <= count {
                let window = &samples[start * channels..(start + 120) * channels];
                let energy = window
                    .iter()
                    .map(|&v| f64::from(v) * f64::from(v))
                    .sum::<f64>()
                    / (120 * channels) as f64;
                minimum = minimum.min(energy);
                maximum = maximum.max(energy);
                self.tone_min = self.tone_min.min(energy);
                self.tone_max = self.tone_max.max(energy);
                self.tone_blocks += 1;
                start += 120;
            }
        }
        for &v in samples {
            let v = f64::from(v);
            self.energy += v * v;
            self.peak = self.peak.max(v.abs());
            self.samples += 1;
        }
        (minimum, maximum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_matches_the_c_percentiles() {
        let mut out = Vec::new();
        let mut values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        distribution(&mut out, "X", &mut values);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "X samples=100 mean_ms=50.500 p50_ms=50.000 p95_ms=95.000 p99_ms=99.000 max_ms=100.000\n"
        );
    }

    #[test]
    fn steady_window_counts_motion() {
        let mut m = Measurements::default();
        // 0..=40 frames 16.67 ms apart; pictures 1,2,2,4,...
        for i in 0..40u32 {
            let t = f64::from(i) * 1000.0 / 60.0;
            let picture = if i == 30 { 30 } else { i + 1 };
            m.push(1000, 1.0, 1.0, t, t, -1.0, picture);
        }
        let s = m.steady(0.25, 60);
        assert_eq!(s.host.len(), 25);
        assert_eq!((s.repeats, s.skips, s.late), (1, 1, 0));
        assert!((s.fps - 60.0).abs() < 0.01);
    }

    #[test]
    fn probe_needs_a_fresh_decoded_idr() {
        let mut p = IdrProbe::new(1, 500_000, 0.0);
        assert!(!p.poll(1_000, 0, false));
        assert!(p.poll(1_000, 0, true));
        let mut frame = Frame::default();
        frame.number = 7;
        frame.frame_type = FrameType::Idr;
        frame.receive_time_us = 500;
        frame.enqueue_time_us = 600;
        p.arrived(&frame);
        frame.receive_time_us = 2_000;
        frame.enqueue_time_us = 3_000;
        p.arrived(&frame);
        p.decoded(7, 4_000);
        let (mut out, mut csv) = (Vec::new(), Vec::new());
        assert!(p.summarize(&mut out, &mut csv));
        assert!(String::from_utf8(out)
            .unwrap()
            .starts_with("IDR_PROBE samples=1 mean_ms=2.000"));
    }
}
