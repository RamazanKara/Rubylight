//! Frame Warp 2 on the client (`rubylight_protocol::frame_warp_v2` has the wire formats): the
//! gyro's part of the shift, the confidence envelope that fades the shift when the host says a
//! menu, a cutscene or a pointer moves the picture, and the fill hints every renderer's shader
//! takes ([`crate::frame_warp::Warp::v2`]).
//!
//! A [`V2`] lives in every stream's predictor (`crate::frame_warp::Predictor`). It keeps the
//! host's metas by frame, counts the gyro packets of the gyro's pad as they go on the wire (the
//! host counts them as they arrive), and for a picture adds
//!
//! ```text
//! gyro_gain * integral(gyro_rate(packet) - bias, tau_g..now)
//! ```
//!
//! with `tau_g` the send time of the meta's `gyro_seq` plus `gyro_held_us`, then scales the whole
//! shift by the envelope: down with a 40 ms time constant when the host's confidence drops (a
//! pause menu stops the warp within 100 ms), back up along a 180 ms S-curve when it recovers.
//!
//! Motion packets go unreliably, so a lost one leaves the counts apart; whenever the client has
//! sent no gyro packet for [`crate::frame_warp::REBASE_IDLE`] (everything sent has reached the
//! picture), the next meta sets them equal again. A resting pad still sends packets (its noise),
//! so each meta also carries the tag of the reading it names (`reading_tag`), and the client finds
//! that packet among those it sent, nearest where the counts say it is ([`V2::find`]): that sets
//! the counts equal however many packets were lost or are in flight.

use std::collections::VecDeque;

use crate::frame_warp::{EDGE_RAMP, REBASE_IDLE};
use rubylight_protocol::frame_warp::HELD_NONE;
pub use rubylight_protocol::frame_warp_v2::{
    BIAS_STILL_RATE, FRAME_WARP_V2_META_MESSAGE_TYPE, FrameWarpV2Meta, GyroAxes, GyroBias,
    MAX_HUD_GUARD, RequestV2, YawSource, from_unit16, gyro_rate, meta_flags,
};
use rubylight_protocol::frame_warp_v2::{DEFAULT_EDGE_FADE, DEFAULT_HUD_GUARD, reading_tag};

/// Time constant of the envelope's fall, microseconds.
pub const CONFIDENCE_ATTACK_US: f32 = 40_000.0;
/// How long the envelope's rise takes, microseconds.
pub const CONFIDENCE_RELEASE_US: u64 = 180_000;
/// How far the edge fill reflects the picture back from its edge, as a share of how far past
/// the edge a point is ([`edge_v2`]): a quarter keeps a silhouette crossing the edge from
/// appearing twice.
pub const EDGE_V2_REFLECT: f32 = 0.25;
/// The shader row ([`V2::row`]) before the host's first meta: the default fill and HUD probe at
/// full confidence.
pub const DEFAULT_ROW: [f32; 4] = [
    DEFAULT_EDGE_FADE as f32 / 255.0,
    DEFAULT_HUD_GUARD as f32,
    1.0,
    0.0,
];
/// Metas kept, by frame number.
const META_SLOTS: usize = 64;
/// Frames back a missing meta may be taken from.
const MAX_META_AGE: u32 = 4;
/// The most gyro history the shift integrates over, microseconds.
const MAX_GYRO_WINDOW_US: u64 = 400_000;
/// Gyro history kept: the window and a margin for a meta naming a packet sent before it.
const GYRO_HISTORY_US: u64 = MAX_GYRO_WINDOW_US + 600_000;
const GYRO_SAMPLES: usize = 4096;
/// A gyro packet's number among those the host counts (motion packets with a gyro reading).
const GYRO: u8 = 2;
/// How many packets either side of where the counts say it is the client looks for the packet a
/// meta names by its tag ([`V2::find`]).
const MATCH_REACH: u32 = 256;

/// The confidence envelope (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    value: f32,
    at_us: Option<u64>,
    /// While rising: where the rise started, toward what and when.
    rise: Option<(f32, f32, u64)>,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            value: 1.0,
            at_us: None,
            rise: None,
        }
    }
}

impl Envelope {
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Moves toward `target` (0 to 1) to `now_us` and returns the value.
    pub fn update(&mut self, target: f32, now_us: u64) -> f32 {
        let target = if target.is_finite() {
            target.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let elapsed = self.at_us.map_or(0, |at| now_us.saturating_sub(at));
        self.at_us = Some(now_us.max(self.at_us.unwrap_or(0)));
        if target < self.value {
            self.rise = None;
            let keep = (-(elapsed as f32) / CONFIDENCE_ATTACK_US).exp();
            self.value = target + (self.value - target) * keep;
            if self.value - target < 1e-4 {
                self.value = target;
            }
        } else if target > self.value {
            let (from, to, start) = match self.rise {
                Some(rise) if rise.1 == target => rise,
                _ => (self.value, target, now_us),
            };
            self.rise = Some((from, to, start));
            let t = (now_us.saturating_sub(start) as f32 / CONFIDENCE_RELEASE_US as f32).min(1.0);
            self.value = from + (to - from) * t * t * (3.0 - 2.0 * t);
            if t >= 1.0 {
                self.value = to;
                self.rise = None;
            }
        }
        self.value
    }
}

/// The edge fill of Frame Warp 2: where a shifted picture point `p` (fractions of the picture, 0
/// to 1 inside it) samples the picture and how bright it is drawn. Inside the picture, `p` at
/// full brightness. Past an edge it samples back inside by [`EDGE_V2_REFLECT`] of how far past
/// the edge it is (a squeezed reflection that reads as the edge stretching, never a second copy of
/// what crosses it), darkened by up to `strength` along a smoothstep over [`EDGE_RAMP`] of the
/// picture. Every renderer's shader draws the same (`frameWarpEdgeV2` in `frame_warp.hlsl`).
pub fn edge_v2(p: [f32; 2], strength: f32) -> ([f32; 2], f32) {
    let axis = |v: f32| {
        let inside = if v < 0.0 {
            -v * EDGE_V2_REFLECT
        } else if v > 1.0 {
            1.0 - (v - 1.0) * EDGE_V2_REFLECT
        } else {
            v
        };
        (inside.clamp(0.0, 1.0), (-v).max(v - 1.0))
    };
    let (x, out_x) = axis(p[0]);
    let (y, out_y) = axis(p[1]);
    let d = (out_x.max(out_y) / EDGE_RAMP).clamp(0.0, 1.0);
    (
        [x, y],
        1.0 - strength.clamp(0.0, 1.0) * d * d * (3.0 - 2.0 * d),
    )
}

/// Where a point whose shifted source `source` (fractions of the picture, before the edge fill)
/// lands on the HUD samples instead: the first of up to `guard` points one mask cell apart back
/// along the shift (`source - k * step`, `step` the shift's direction one cell long) whose fill
/// ([`edge_v2`] with `strength`) is off the HUD, with its brightness; `None` keeps the point's own
/// pixel, as does a shift too short to square (no direction to probe along). `cells` is the mask's
/// size and `hud` reads it. Every renderer's shader probes the same (`frameWarpProbe` in
/// `frame_warp.hlsl` and the Android shaders, which test `dot(shift, shift) == 0.0`).
pub fn hud_probe(
    source: [f32; 2],
    shift: [f32; 2],
    cells: [f32; 2],
    guard: u32,
    strength: f32,
    hud: impl Fn([f32; 2]) -> bool,
) -> Option<([f32; 2], f32)> {
    let squared = shift[0] * shift[0] + shift[1] * shift[1];
    if squared <= 0.0 || cells[0] <= 0.0 || cells[1] <= 0.0 {
        return None;
    }
    let length = squared.sqrt();
    let step = [shift[0] / length / cells[0], shift[1] / length / cells[1]];
    (1..=guard.min(u32::from(MAX_HUD_GUARD))).find_map(|k| {
        let at = [
            source[0] - k as f32 * step[0],
            source[1] - k as f32 * step[1],
        ];
        let (sample, fade) = edge_v2(at, strength);
        (!hud(sample)).then_some((sample, fade))
    })
}

#[derive(Clone, Copy, Debug)]
struct GyroSample {
    at_us: u64,
    /// The packet's number among the gyro packets sent for the gyro's pad (wrapping, from 1).
    seq: u32,
    /// Yaw and pitch rate less the bias, radians per second, with the yaw from y and from z.
    rate: [[f32; 2]; 2],
    /// The reading's `reading_tag`.
    tag: u16,
}

/// What the statistics overlay shows of Frame Warp 2.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// The envelope, 0 to 1.
    pub confidence: f32,
    /// The latest meta, once one has arrived.
    pub meta: Option<FrameWarpV2Meta>,
    /// Gyro packets counted for the gyro's pad.
    pub gyro_packets: u32,
}

/// Frame Warp 2's part of a stream's predictor (see the module documentation).
#[derive(Debug)]
pub struct V2 {
    request: RequestV2,
    metas: [Option<FrameWarpV2Meta>; META_SLOTS],
    newest: Option<FrameWarpV2Meta>,
    envelope: Envelope,
    gyro_seq: u32,
    seq_offset: u32,
    /// When the last gyro packet went out.
    last_sent_us: Option<u64>,
    samples: VecDeque<GyroSample>,
    bias: [GyroBias; 2],
}

/// `a` comes after `b` in wrapping order.
fn after(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

impl V2 {
    pub fn new(request: RequestV2) -> Self {
        Self {
            request,
            metas: [None; META_SLOTS],
            newest: None,
            envelope: Envelope::default(),
            gyro_seq: 0,
            seq_offset: 0,
            last_sent_us: None,
            samples: VecDeque::new(),
            bias: [GyroBias::default(); 2],
        }
    }

    pub fn request(&self) -> &RequestV2 {
        &self.request
    }

    /// A motion packet as it goes on the wire: counts it when it is a gyro reading of the
    /// gyro's pad. Returns whether it was counted.
    pub fn note_motion(&mut self, number: u8, kind: u8, xyz: [f32; 3], now_us: u64) -> bool {
        if kind != GYRO || self.request.gyro_pad() != Some(number) {
            return false;
        }
        let axes = |yaw| GyroAxes {
            yaw,
            ..self.request.axes
        };
        let rate = [
            self.bias[0].observe(gyro_rate(xyz, axes(YawSource::Y), false)),
            self.bias[1].observe(gyro_rate(xyz, axes(YawSource::Z), true)),
        ];
        self.last_sent_us = Some(now_us);
        self.gyro_seq = self.gyro_seq.wrapping_add(1);
        let at_us = self
            .samples
            .back()
            .map_or(now_us, |last| now_us.max(last.at_us));
        self.samples.push_back(GyroSample {
            at_us,
            seq: self.gyro_seq,
            rate,
            tag: reading_tag(xyz),
        });
        while self.samples.len() > GYRO_SAMPLES
            || self.samples.len() > 1
                && at_us.saturating_sub(self.samples[1].at_us) > GYRO_HISTORY_US
        {
            self.samples.pop_front();
        }
        true
    }

    /// A meta from the host.
    pub fn on_meta(&mut self, meta: FrameWarpV2Meta, now_us: u64) {
        self.metas[meta.frame as usize % META_SLOTS] = Some(meta);
        if self.newest.is_some_and(|n| !after(meta.frame, n.frame)) {
            return;
        }
        self.newest = Some(meta);
        let idle = self
            .last_sent_us
            .is_none_or(|at| now_us.saturating_sub(at) >= REBASE_IDLE.as_micros() as u64);
        if let Some(seq) = self.find(&meta) {
            self.seq_offset = seq.wrapping_sub(meta.gyro_seq);
        } else if idle {
            // Everything sent has arrived: the counts compare exactly.
            self.seq_offset = self.gyro_seq.wrapping_sub(meta.gyro_seq);
        }
    }

    /// The number of the sent packet `meta` names: of those within [`MATCH_REACH`] of where the
    /// counts put it whose tag is the meta's, the nearest. `None` for a meta without a tag, when
    /// none matches, or when two are as near (a resting sensor can repeat a reading).
    fn find(&self, meta: &FrameWarpV2Meta) -> Option<u32> {
        if !meta.has(meta_flags::GYRO_TAGGED) || meta.gyro_seq == 0 {
            return None;
        }
        let expected = meta.gyro_seq.wrapping_add(self.seq_offset);
        let mut best: Option<(u32, u32)> = None;
        let mut tie = false;
        for sample in self.samples.iter().filter(|s| s.tag == meta.gyro_tag) {
            let distance = sample
                .seq
                .wrapping_sub(expected)
                .min(expected.wrapping_sub(sample.seq));
            if distance > MATCH_REACH {
                continue;
            }
            match best {
                Some((_, nearest)) if distance > nearest => {}
                Some((_, nearest)) if distance == nearest => tie = true,
                _ => {
                    best = Some((sample.seq, distance));
                    tie = false;
                }
            }
        }
        best.filter(|_| !tie).map(|(seq, _)| seq)
    }

    /// The meta of `frame`, or the newest one before it within a few frames.
    fn meta(&self, frame: u32) -> Option<FrameWarpV2Meta> {
        (0..=MAX_META_AGE).find_map(|back| {
            let f = frame.wrapping_sub(back);
            self.metas[f as usize % META_SLOTS].filter(|m| m.frame == f)
        })
    }

    /// Whether the host's gyro gain fits well enough to shift by it.
    pub fn gyro_locked(&self) -> bool {
        self.request.gyro_pad().is_some()
            && self.newest.is_some_and(|m| m.has(meta_flags::GYRO_LOCKED))
    }

    /// The gyro's part of picture `frame`'s shift at the picture's centre, picture pixels.
    pub fn gyro_shift(&self, frame: u32, now_us: u64) -> [f32; 2] {
        if !self.gyro_locked() {
            return [0.0; 2];
        }
        let Some(meta) = self.meta(frame).or(self.newest) else {
            return [0.0; 2];
        };
        let Some(newest) = self.samples.back() else {
            return [0.0; 2];
        };
        let host_seq = meta.gyro_seq.wrapping_add(self.seq_offset);
        if host_seq != newest.seq && after(host_seq, newest.seq) {
            return [0.0; 2];
        }
        let earliest = now_us.saturating_sub(MAX_GYRO_WINDOW_US);
        let start = if meta.gyro_held_us == HELD_NONE || meta.gyro_seq == 0 {
            earliest
        } else {
            let back = newest.seq.wrapping_sub(host_seq) as usize;
            match self
                .samples
                .len()
                .checked_sub(back + 1)
                .and_then(|i| self.samples.get(i))
            {
                Some(sample) if sample.seq == host_seq => {
                    sample.at_us + u64::from(meta.gyro_held_us)
                }
                _ => earliest,
            }
        }
        .clamp(earliest, now_us);
        let axis = usize::from(meta.has(meta_flags::GYRO_YAW_Z));
        let first = self
            .samples
            .iter()
            .rposition(|s| s.at_us <= start)
            .unwrap_or(0);
        let mut angle = [0.0f32; 2];
        for (i, sample) in self.samples.iter().enumerate().skip(first) {
            let end = self
                .samples
                .get(i + 1)
                .map_or(now_us, |next| next.at_us)
                .min(now_us);
            let from = sample.at_us.max(start);
            if end > from {
                let held = (end - from) as f32 / 1_000_000.0;
                angle[0] += sample.rate[axis][0] * held;
                angle[1] += sample.rate[axis][1] * held;
            }
        }
        [meta.gyro_gain[0] * angle[0], meta.gyro_gain[1] * angle[1]]
    }

    /// The envelope at `now_us`, toward the newest meta's confidence (1 before any).
    pub fn confidence(&mut self, now_us: u64) -> f32 {
        let target = self.newest.map_or(1.0, |meta| meta.confidence());
        self.envelope.update(target, now_us)
    }

    /// The shader row for picture `frame`: the edge fill's strength (0 to 1, 0 holds the edge's
    /// pixels), the HUD probe's reach in mask cells (0 probes nothing), the envelope and the
    /// meta's flags; [`DEFAULT_ROW`]'s fill and probe before the host's first meta.
    pub fn row(&self, frame: u32) -> [f32; 4] {
        let Some(meta) = self.meta(frame).or(self.newest) else {
            return [DEFAULT_ROW[0], DEFAULT_ROW[1], self.envelope.value(), DEFAULT_ROW[3]];
        };
        [
            f32::from(meta.edge_fade_strength) / 255.0,
            f32::from(meta.hud_guard.min(MAX_HUD_GUARD)),
            self.envelope.value(),
            f32::from(meta.flags),
        ]
    }

    pub fn status(&self) -> Status {
        Status {
            confidence: self.envelope.value(),
            meta: self.newest,
            gyro_packets: self.gyro_seq,
        }
    }
}

#[cfg(test)]
mod tests;
