//! What input the host received and when, for tagging frames and for fitting the model.
//!
//! Relative mouse motion is kept as running sums after each move; the tracked pad's packets
//! as their count and right stick. Times are microseconds on the session's clock, taken when
//! the control thread decoded the packet (injection follows within the same pass).

use std::collections::VecDeque;

use rubylight_protocol::frame_warp::{HELD_NONE, Tag, stick_curve};

/// History kept: longer than the calibrator's window plus its largest lag.
pub const KEEP_US: u64 = 6_000_000;
/// Mouse moves and pad packets kept at most, whatever their age.
const MAX_ENTRIES: usize = 65_536;
/// A stick past this fraction of full deflection is in use.
const STICK_ACTIVE: f32 = 0.08;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MouseEntry {
    at_us: u64,
    /// Running sums after this move, and the move.
    x: i64,
    y: i64,
    dx: i16,
    dy: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PadEntry {
    at_us: u64,
    /// Packets received for the tracked pad, this one included.
    seq: u32,
    stick: (i16, i16),
}

#[derive(Clone, Debug, Default)]
pub struct InputHistory {
    controller: Option<u8>,
    mouse: VecDeque<MouseEntry>,
    mouse_sum: (i64, i64),
    pads: VecDeque<PadEntry>,
    pad_seq: u32,
}

impl InputHistory {
    pub fn new(controller: Option<u8>) -> Self {
        Self {
            controller,
            ..Self::default()
        }
    }

    /// Tracks another pad from now on; its packets count from zero.
    pub fn set_controller(&mut self, controller: Option<u8>) {
        if self.controller != controller {
            self.controller = controller;
            self.pads.clear();
            self.pad_seq = 0;
        }
    }

    pub fn controller(&self) -> Option<u8> {
        self.controller
    }

    pub fn on_mouse(&mut self, dx: i16, dy: i16, at_us: u64) {
        if dx == 0 && dy == 0 {
            return;
        }
        self.mouse_sum.0 += i64::from(dx);
        self.mouse_sum.1 += i64::from(dy);
        let at_us = self
            .mouse
            .back()
            .map_or(at_us, |last| at_us.max(last.at_us));
        self.mouse.push_back(MouseEntry {
            at_us,
            x: self.mouse_sum.0,
            y: self.mouse_sum.1,
            dx,
            dy,
        });
        // Keep the newest entry before the window: it is the sum at the window's start.
        while self.mouse.len() > MAX_ENTRIES
            || self.mouse.len() > 1 && at_us.saturating_sub(self.mouse[1].at_us) > KEEP_US
        {
            self.mouse.pop_front();
        }
    }

    pub fn on_controller(&mut self, number: u16, right_stick: (i16, i16), at_us: u64) {
        if self.controller.map(u16::from) != Some(number) {
            return;
        }
        self.pad_seq = self.pad_seq.wrapping_add(1);
        let at_us = self.pads.back().map_or(at_us, |last| at_us.max(last.at_us));
        self.pads.push_back(PadEntry {
            at_us,
            seq: self.pad_seq,
            stick: right_stick,
        });
        while self.pads.len() > MAX_ENTRIES
            || self.pads.len() > 1 && at_us.saturating_sub(self.pads[1].at_us) > KEEP_US
        {
            self.pads.pop_front();
        }
    }

    /// The running mouse sums as of `at_us`.
    pub fn mouse_at(&self, at_us: u64) -> (i64, i64) {
        let after = self.mouse.partition_point(|e| e.at_us <= at_us);
        match after.checked_sub(1) {
            Some(i) => (self.mouse[i].x, self.mouse[i].y),
            // Before the oldest kept move: its sums less the move itself.
            None => self.mouse.front().map_or(self.mouse_sum, |first| {
                (first.x - i64::from(first.dx), first.y - i64::from(first.dy))
            }),
        }
    }

    /// Mouse counts received in `t0..t1`.
    pub fn mouse_between(&self, t0: u64, t1: u64) -> (i64, i64) {
        let a = self.mouse_at(t0);
        let b = self.mouse_at(t1);
        (b.0 - a.0, b.1 - a.1)
    }

    /// The tag of a frame that reflects mouse input up to `mouse_us` and pad input up to
    /// `pad_us`.
    pub fn tag(&self, frame: u32, mouse_us: u64, pad_us: u64) -> Tag {
        let (x, y) = self.mouse_at(mouse_us);
        let after = self.pads.partition_point(|e| e.at_us <= pad_us);
        let (seq, held) = match after.checked_sub(1) {
            Some(i) => {
                let pad = self.pads[i];
                let held = (pad_us - pad.at_us).min(u64::from(HELD_NONE - 1)) as u32;
                (pad.seq, held)
            }
            None => (
                self.pads
                    .front()
                    .map_or(self.pad_seq, |p| p.seq.wrapping_sub(1)),
                HELD_NONE,
            ),
        };
        Tag {
            frame,
            // The wire carries the sums modulo 2^32; the client takes differences.
            mouse_x: x as i32,
            mouse_y: y as i32,
            controller_seq: seq,
            controller_held_us: if seq == 0 { HELD_NONE } else { held },
        }
    }

    /// The integral of [`stick_curve`] over `t0..t1`, in seconds of full deflection.
    pub fn stick_integral(&self, t0: u64, t1: u64, deadzone: f32, exponent: f32) -> [f64; 2] {
        let mut sum = [0.0f64; 2];
        if t1 <= t0 {
            return sum;
        }
        let start = self
            .pads
            .partition_point(|e| e.at_us <= t0)
            .saturating_sub(1);
        for i in start..self.pads.len() {
            let pad = self.pads[i];
            if pad.at_us >= t1 {
                break;
            }
            let end = self.pads.get(i + 1).map_or(t1, |next| next.at_us).min(t1);
            let from = pad.at_us.max(t0);
            if end <= from {
                continue;
            }
            let seconds = (end - from) as f64 / 1e6;
            let curve = stick_curve(pad.stick.0, pad.stick.1, deadzone, exponent);
            sum[0] += f64::from(curve[0]) * seconds;
            sum[1] += f64::from(curve[1]) * seconds;
        }
        sum
    }

    /// [`Self::stick_integral`] for many spans under one curve: the curve's running integral
    /// over the whole history, built once, so each span costs two binary searches.
    pub fn stick_integrals(&self, deadzone: f32, exponent: f32) -> StickIntegrals<'_> {
        let curve: Vec<[f32; 2]> = self
            .pads
            .iter()
            .map(|pad| stick_curve(pad.stick.0, pad.stick.1, deadzone, exponent))
            .collect();
        let mut running = Vec::with_capacity(curve.len());
        let mut sum = [0.0f64; 2];
        for (i, value) in curve.iter().enumerate() {
            running.push(sum);
            if let Some(next) = self.pads.get(i + 1) {
                let seconds = (next.at_us - self.pads[i].at_us) as f64 / 1e6;
                sum[0] += f64::from(value[0]) * seconds;
                sum[1] += f64::from(value[1]) * seconds;
            }
        }
        StickIntegrals {
            pads: &self.pads,
            curve,
            running,
        }
    }

    /// Whether the tracked pad's right stick was off centre at any time in `t0..t1`.
    pub fn stick_active(&self, t0: u64, t1: u64) -> bool {
        let start = self
            .pads
            .partition_point(|e| e.at_us <= t0)
            .saturating_sub(1);
        self.pads
            .range(start..)
            .take_while(|e| e.at_us < t1)
            .any(|pad| {
                let x = f32::from(pad.stick.0) / 32767.0;
                let y = f32::from(pad.stick.1) / 32767.0;
                x * x + y * y > STICK_ACTIVE * STICK_ACTIVE
            })
    }

    /// When the last mouse move or pad packet arrived.
    pub fn last_input_us(&self) -> Option<u64> {
        let mouse = self.mouse.back().map(|e| e.at_us);
        let pad = self.pads.back().map(|e| e.at_us);
        mouse.max(pad)
    }
}

/// One stick curve's running integral over the pad history ([`InputHistory::stick_integrals`]).
#[derive(Debug)]
pub struct StickIntegrals<'a> {
    pads: &'a VecDeque<PadEntry>,
    /// Each packet's curved stick, held until the next packet (the last one indefinitely).
    curve: Vec<[f32; 2]>,
    /// The integral from the first packet to each packet, in seconds of full deflection.
    running: Vec<[f64; 2]>,
}

impl StickIntegrals<'_> {
    /// [`InputHistory::stick_integral`] over `t0..t1` with this curve.
    pub fn between(&self, t0: u64, t1: u64) -> [f64; 2] {
        if t1 <= t0 {
            return [0.0; 2];
        }
        let (a, b) = (self.until(t0), self.until(t1));
        [b[0] - a[0], b[1] - a[1]]
    }

    /// The integral from the first packet to `t`; 0 before it.
    fn until(&self, t: u64) -> [f64; 2] {
        let after = self.pads.partition_point(|e| e.at_us <= t);
        let Some(i) = after.checked_sub(1) else {
            return [0.0; 2];
        };
        let seconds = (t - self.pads[i].at_us) as f64 / 1e6;
        [
            self.running[i][0] + f64::from(self.curve[i][0]) * seconds,
            self.running[i][1] + f64::from(self.curve[i][1]) * seconds,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_sums_are_looked_up_by_time() {
        let mut h = InputHistory::new(None);
        assert_eq!(h.mouse_at(5), (0, 0));
        h.on_mouse(10, -2, 100);
        h.on_mouse(5, 1, 200);
        assert_eq!(h.mouse_at(99), (0, 0));
        assert_eq!(h.mouse_at(100), (10, -2));
        assert_eq!(h.mouse_at(150), (10, -2));
        assert_eq!(h.mouse_at(1000), (15, -1));
        assert_eq!(h.mouse_between(150, 250), (5, 1));
    }

    #[test]
    fn tags_carry_the_sums_and_the_held_pad_packet() {
        let mut h = InputHistory::new(Some(0));
        h.on_mouse(7, 3, 10);
        assert_eq!(h.tag(1, 20, 20).controller_held_us, HELD_NONE);
        h.on_controller(0, (100, 0), 30);
        h.on_controller(2, (9, 9), 35);
        h.on_controller(0, (200, 0), 50);
        let tag = h.tag(4, 40, 60);
        assert_eq!((tag.frame, tag.mouse_x, tag.mouse_y), (4, 7, 3));
        assert_eq!((tag.controller_seq, tag.controller_held_us), (2, 10));
        let early = h.tag(5, 5, 40);
        assert_eq!(
            (
                early.mouse_x,
                early.controller_seq,
                early.controller_held_us
            ),
            (0, 1, 10)
        );
    }

    #[test]
    fn stick_integral_follows_the_held_states() {
        let mut h = InputHistory::new(Some(1));
        h.on_controller(1, (0, 0), 0);
        h.on_controller(1, (32767, 0), 1_000_000);
        h.on_controller(1, (0, -32768), 1_500_000);
        let full = h.stick_integral(1_000_000, 1_500_000, 0.0, 1.0);
        assert!((full[0] - 0.5).abs() < 1e-6 && full[1].abs() < 1e-9);
        let spanning = h.stick_integral(500_000, 2_000_000, 0.0, 1.0);
        assert!((spanning[0] - 0.5).abs() < 1e-6, "{spanning:?}");
        assert!((spanning[1] + 0.5).abs() < 1e-4, "{spanning:?}");
        assert!(h.stick_active(1_200_000, 1_300_000));
        assert!(!h.stick_active(100_000, 900_000));
    }

    #[test]
    fn running_stick_integrals_match_the_direct_sum() {
        let mut seed = 0x9e37_79b9u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut h = InputHistory::new(Some(0));
        let mut t = 50_000u64;
        for _ in 0..3_000 {
            // 0 to 4 ms apart (repeated times included), the stick anywhere, centred often.
            t += u64::from(next() % 4_001) / u64::from(1 + next() % 2);
            let stick = if next() % 4 == 0 {
                (0, 0)
            } else {
                (next() as i16, next() as i16)
            };
            h.on_controller(0, stick, t);
        }
        let end = t;
        for (deadzone, exponent) in [(0.0, 1.0), (0.15, 1.0), (0.2, 1.5), (0.25, 2.0)] {
            let running = h.stick_integrals(deadzone, exponent);
            for _ in 0..2_000 {
                // Spans before the first packet, past the last, empty and reversed included.
                let t0 = u64::from(next()) % (end + 100_000);
                let t1 = match next() % 8 {
                    0 => t0,
                    1 => t0.saturating_sub(u64::from(next() % 10_000)),
                    _ => t0 + u64::from(next() % 230_000),
                };
                let direct = h.stick_integral(t0, t1, deadzone, exponent);
                let fast = running.between(t0, t1);
                for axis in 0..2 {
                    assert!(
                        (direct[axis] - fast[axis]).abs() < 1e-9,
                        "{t0}..{t1} with {deadzone}/{exponent}: {direct:?} vs {fast:?}"
                    );
                }
            }
        }
        // No pad packets: nothing either way.
        let empty = InputHistory::new(Some(0));
        assert_eq!(
            empty.stick_integrals(0.1, 1.0).between(0, 1_000_000),
            [0.0; 2]
        );
    }

    #[test]
    fn old_history_is_dropped() {
        let mut h = InputHistory::new(None);
        h.on_mouse(1, 0, 0);
        h.on_mouse(1, 0, 1);
        h.on_mouse(1, 0, KEEP_US + 10);
        assert_eq!(h.mouse.len(), 2);
        assert_eq!(h.mouse_at(KEEP_US + 10), (3, 0));
    }
}
