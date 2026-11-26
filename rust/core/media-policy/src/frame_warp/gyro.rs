//! Frame Warp 2's gyro on the host: what the gyro pad's motion packets said and when, counted
//! as the client counts them, for tagging frames and for fitting the gyro's gain.
//!
//! A version 2 request names a pad whose gyro turns the camera
//! (`rubylight_protocol::frame_warp_v2::RequestV2::gyro_pad`). Every motion packet of that pad
//! with a gyro reading (0x5500_0006, kind 2) counts, in the order it arrived; the client counts
//! the same packets as it sends them. Each reading is kept as two bias-corrected rates, the yaw
//! taken from the packet's y axis and from its z axis
//! (`rubylight_protocol::frame_warp_v2::gyro_rate`, one
//! `rubylight_protocol::frame_warp_v2::GyroBias` each), exactly as the client keeps them, so the
//! host can tell the client which one the game turns by ([`YawChoice`]) and both ends integrate
//! the same rates. A reading is held until the next one arrives.

use std::collections::VecDeque;

use rubylight_protocol::frame_warp::HELD_NONE;
use rubylight_protocol::frame_warp_v2::{
    BIAS_STILL_RATE, GyroAxes, GyroBias, YawSource, gyro_rate,
};

/// A motion packet's kind for a gyro reading.
pub const GYRO_KIND: u8 = 2;
/// History kept: as long as the mouse and stick history ([`super::history::KEEP_US`]).
const KEEP_US: u64 = super::history::KEEP_US;
/// Readings kept at most, whatever their age.
const MAX_SAMPLES: usize = 65_536;
/// How much better the other yaw axis must explain the motion before an automatic choice
/// changes ([`YawChoice`]).
const YAW_SWITCH: f64 = 0.15;
/// An automatic choice needs one axis to explain at least this much of the motion.
const YAW_DECIDE: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    at_us: u64,
    /// The packet's number among the gyro pad's gyro packets (wrapping, from 1).
    seq: u32,
    /// Yaw and pitch rate less the bias, radians per second: `[yaw from y, yaw from z]`.
    rate: [[f32; 2]; 2],
}

/// The gyro pad's readings (see the module documentation).
#[derive(Clone, Debug)]
pub struct GyroHistory {
    pad: Option<u8>,
    axes: GyroAxes,
    seq: u32,
    samples: VecDeque<Sample>,
    bias: [GyroBias; 2],
}

impl GyroHistory {
    /// The history of `pad`'s gyro (none: nothing counts), mapped by `axes`.
    pub fn new(pad: Option<u8>, axes: GyroAxes) -> Self {
        Self {
            pad,
            axes,
            seq: 0,
            samples: VecDeque::new(),
            bias: [GyroBias::default(); 2],
        }
    }

    pub fn pad(&self) -> Option<u8> {
        self.pad
    }

    /// Gyro packets counted so far (wrapping).
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// A motion packet as the control thread decoded it at `at_us`. Returns whether it counted:
    /// a gyro reading of the gyro's pad.
    pub fn on_motion(&mut self, number: u8, kind: u8, xyz: [f32; 3], at_us: u64) -> bool {
        if kind != GYRO_KIND || self.pad != Some(number) {
            return false;
        }
        let axes = |yaw| GyroAxes { yaw, ..self.axes };
        let rate = [
            self.bias[0].observe(gyro_rate(xyz, axes(YawSource::Y), false)),
            self.bias[1].observe(gyro_rate(xyz, axes(YawSource::Z), true)),
        ];
        self.seq = self.seq.wrapping_add(1);
        let at_us = self
            .samples
            .back()
            .map_or(at_us, |last| at_us.max(last.at_us));
        self.samples.push_back(Sample {
            at_us,
            seq: self.seq,
            rate,
        });
        while self.samples.len() > MAX_SAMPLES
            || self.samples.len() > 1 && at_us.saturating_sub(self.samples[1].at_us) > KEEP_US
        {
            self.samples.pop_front();
        }
        true
    }

    /// The index of the reading held at `t` (the last that arrived by then).
    fn held_at(&self, t: u64) -> Option<usize> {
        self.samples
            .partition_point(|s| s.at_us <= t)
            .checked_sub(1)
    }

    /// The angle the gyro turned over `t0..t1`, radians of yaw and pitch, with the yaw from the
    /// packets' z axis when `yaw_z` (else their y axis): each reading held until the next.
    pub fn angle(&self, t0: u64, t1: u64, yaw_z: bool) -> [f64; 2] {
        let mut sum = [0.0f64; 2];
        if t1 <= t0 {
            return sum;
        }
        let axis = usize::from(yaw_z);
        let start = self.held_at(t0).unwrap_or(0);
        for i in start..self.samples.len() {
            let sample = self.samples[i];
            if sample.at_us >= t1 {
                break;
            }
            let end = self
                .samples
                .get(i + 1)
                .map_or(t1, |next| next.at_us)
                .min(t1);
            let from = sample.at_us.max(t0);
            if end > from {
                let seconds = (end - from) as f64 / 1e6;
                sum[0] += f64::from(sample.rate[axis][0]) * seconds;
                sum[1] += f64::from(sample.rate[axis][1]) * seconds;
            }
        }
        sum
    }

    /// Whether the gyro turned faster than a pad lying still at any time in `t0..t1`.
    pub fn active(&self, t0: u64, t1: u64) -> bool {
        let start = self.held_at(t0).unwrap_or(0);
        self.samples
            .range(start..)
            .take_while(|s| s.at_us < t1)
            .any(|s| s.rate.iter().flatten().any(|r| r.abs() >= BIAS_STILL_RATE))
    }

    /// The gyro packet a frame reflecting the gyro up to `at_us` shows, and how long it had
    /// been held then: `(gyro_seq, gyro_held_us)` of
    /// `rubylight_protocol::frame_warp_v2::FrameWarpV2Meta` (0 and
    /// [`HELD_NONE`] for none), as [`super::history::InputHistory::tag`] names the pad's packet.
    pub fn tag(&self, at_us: u64) -> (u32, u32) {
        let (seq, held) = match self.held_at(at_us) {
            Some(i) => {
                let sample = self.samples[i];
                let held = (at_us - sample.at_us).min(u64::from(HELD_NONE - 1)) as u32;
                (sample.seq, held)
            }
            None => (
                self.samples
                    .front()
                    .map_or(self.seq, |s| s.seq.wrapping_sub(1)),
                HELD_NONE,
            ),
        };
        (seq, if seq == 0 { HELD_NONE } else { held })
    }
}

/// Which motion packet axis turns the camera left and right: the request's, or for one that
/// leaves it to the host ([`YawSource::Auto`]), whichever of the two candidate gyro fits
/// explains the measured motion better, kept until the other explains clearly more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct YawChoice {
    source: YawSource,
    /// The automatic choice so far: the z axis.
    yaw_z: Option<bool>,
}

impl YawChoice {
    pub fn new(source: YawSource) -> Self {
        Self {
            source,
            yaw_z: None,
        }
    }

    /// Takes how much of the measured motion the gyro explains with the yaw from y and from z
    /// (`None` where the fit has too little to say). Returns whether the yaw is z.
    pub fn observe(&mut self, fits: [Option<f64>; 2]) -> bool {
        if self.source == YawSource::Auto {
            let [y, z] = fits.map(|f| f.unwrap_or(0.0));
            let best_z = z > y;
            let (best, other) = if best_z { (z, y) } else { (y, z) };
            match self.yaw_z {
                None if best >= YAW_DECIDE && best - other >= YAW_SWITCH => {
                    self.yaw_z = Some(best_z);
                }
                Some(current) if current != best_z && best - other >= YAW_SWITCH => {
                    self.yaw_z = Some(best_z);
                }
                _ => {}
            }
        }
        self.yaw_z()
    }

    /// Whether the yaw is the packets' z axis: the request's, or the automatic choice (y until
    /// one is made).
    pub fn yaw_z(&self) -> bool {
        match self.source {
            YawSource::Y => false,
            YawSource::Z => true,
            YawSource::Auto => self.yaw_z.unwrap_or(false),
        }
    }

    /// Starts an automatic choice from what an earlier stream of the game found.
    pub fn prefer(&mut self, yaw_z: bool) {
        if self.source == YawSource::Auto && self.yaw_z.is_none() {
            self.yaw_z = Some(yaw_z);
        }
    }

    /// Whether the yaw axis is known: given by the request or chosen.
    pub fn decided(&self) -> bool {
        self.source != YawSource::Auto || self.yaw_z.is_some()
    }
}

#[cfg(test)]
mod tests;
