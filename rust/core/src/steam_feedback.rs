//! Rumble for a client without the Steam Controller's pads, synthesized from
//! the haptic output reports Steam writes to a virtual Steam Controller, as
//! Vibepollo's `synthesize_steam_rumble` does. Experimental: the gamepad
//! thread folds them in, but no pinned driver has the Steam Controller
//! profile to produce them yet.
pub use rubylight_protocol::steam_controller::SteamHaptic;
use std::time::{Duration, Instant};

/// Sides a report addressed ([`SteamRumble::apply`]).
pub const LEFT: u8 = 1;
pub const RIGHT: u8 = 2;
/// A shorter pulse is lost between two feedback polls, and a motor needs
/// about that long to spin up: Steam's single 400 us UI click becomes this.
pub const MIN_HOLD: Duration = Duration::from_millis(50);
/// Steam re-sends a test-screen click every 100 ms and never an off; the hold
/// outlasts one interval so the clicks read as one buzz, not a stutter.
pub const CLICK_HOLD: Duration = Duration::from_millis(150);
/// The real unit stops a 0x80 rumble when Steam's re-sends (every 50 ms at
/// most) stop coming, so the synthesized one stops too when the sender dies
/// or its zero report is lost.
pub const RUMBLE_TIMEOUT: Duration = Duration::from_millis(200);
const MAX_HOLD: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Motor {
    magnitude: u16,
    /// `None` only while stopped: every non-zero magnitude expires.
    until: Option<Instant>,
}
impl Motor {
    fn at(&self, now: Instant) -> u16 {
        if self.until.is_none_or(|until| now < until) {
            self.magnitude
        } else {
            0
        }
    }
}

/// One controller's synthesized rumble: the left pad drives the low-frequency
/// motor, the right pad the high-frequency one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SteamRumble {
    left: Motor,
    right: Motor,
}
impl SteamRumble {
    /// Folds a haptic report in at `now`. Returns the sides it addressed
    /// ([`LEFT`], [`RIGHT`]), 0 for a report no rumble motor can stand in for
    /// (too short, LFO tone, sweep, script, or a side with no pad); only the
    /// returned sides change.
    pub fn apply(&mut self, haptic: &SteamHaptic, now: Instant) -> u8 {
        let report = haptic.report();
        match report[0] {
            // Rumble: type u8, intensity u16, left {speed u16, gain s8}, right
            // {speed u16, gain s8}. The speeds are already the motor range.
            0x80 if report.len() >= 10 => {
                let until = Some(now + RUMBLE_TIMEOUT);
                self.left = Motor {
                    magnitude: le16(report, 4),
                    until,
                };
                self.right = Motor {
                    magnitude: le16(report, 7),
                    until,
                };
                LEFT | RIGHT
            }
            // Pulse: side u8, on_us u16, off_us u16, repeat u16. The duty cycle
            // is the magnitude and the train's length the hold; a zero-repeat
            // pulse is Steam's stop.
            0x81 if report.len() >= 8 => {
                let (on, off, repeat) = (
                    u64::from(le16(report, 2)),
                    u64::from(le16(report, 4)),
                    u64::from(le16(report, 6)),
                );
                let motor = if repeat == 0 || on == 0 {
                    Motor::default()
                } else {
                    let period = on + off;
                    let train = Duration::from_micros(repeat * period);
                    Motor {
                        magnitude: (on * 65535 / period) as u16,
                        until: Some(now + train.clamp(MIN_HOLD, MAX_HOLD)),
                    }
                };
                self.set(sides(0x81, report[1]), motor)
            }
            // Command: side u8, command u8 (0 off, 1 tick, 2 click, 3 tone,
            // 4 rumble, 5 noise, 6 script, 7 sweep), gain_db s8. Ticks and
            // clicks hold briefly. Tone, rumble and noise run until their off
            // but never past MAX_HOLD from the last re-send, so a lost off
            // cannot leave the motors on. Scripts, sweeps and unknown
            // commands have nothing a motor can stand in for, like 0x84/0x85.
            0x82 if report.len() >= 4 => {
                let hold = match report[2] {
                    0 => None,
                    1 | 2 => Some(CLICK_HOLD),
                    3..=5 => Some(MAX_HOLD),
                    _ => return 0,
                };
                let motor = match hold {
                    None => Motor::default(),
                    Some(hold) => Motor {
                        magnitude: magnitude_from_db(report[3] as i8),
                        until: Some(now + hold),
                    },
                };
                self.set(sides(0x82, report[1]), motor)
            }
            _ => 0,
        }
    }

    /// The (low, high) motor magnitudes at `now`.
    pub fn rumble(&self, now: Instant) -> (u16, u16) {
        (self.left.at(now), self.right.at(now))
    }

    /// When after `now` the rumble next changes on its own: poll
    /// [`Self::rumble`] then. `None` once every hold has run out.
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        [self.left, self.right]
            .into_iter()
            .filter(|motor| motor.magnitude != 0)
            .filter_map(|motor| motor.until)
            .filter(|&until| until > now)
            .min()
    }

    fn set(&mut self, sides: u8, motor: Motor) -> u8 {
        if sides & LEFT != 0 {
            self.left = motor;
        }
        if sides & RIGHT != 0 {
            self.right = motor;
        }
        sides
    }
}

/// Every controller's synthesized rumble and the strengths last sent for it,
/// so each change goes out once and a clear stops only motors that ran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SteamPads {
    pads: [SteamRumble; 16],
    sent: [(u16, u16); 16],
}
impl SteamPads {
    /// Folds a report in at `now`; one for a controller past 15 is ignored.
    pub fn apply(&mut self, haptic: &SteamHaptic, now: Instant) {
        if let Some(pad) = self.pads.get_mut(usize::from(haptic.controller)) {
            pad.apply(haptic, now);
        }
    }

    /// Stops every controller outside `active` and drops its pending holds:
    /// one the client removed, or all of them when the client turns haptics
    /// off or the session ends. The stops come with the next [`Self::changes`].
    pub fn retain(&mut self, active: u16) {
        for (id, pad) in self.pads.iter_mut().enumerate() {
            if active & (1 << id) == 0 {
                *pad = SteamRumble::default();
            }
        }
    }

    /// The (controller, low, high) rumble of each controller whose motors
    /// changed at `now` since the last call.
    pub fn changes(&mut self, now: Instant) -> Vec<(u16, u16, u16)> {
        let mut changes = Vec::new();
        for (id, (pad, sent)) in self.pads.iter().zip(&mut self.sent).enumerate() {
            let rumble = pad.rumble(now);
            if rumble != *sent {
                *sent = rumble;
                changes.push((id as u16, rumble.0, rumble.1));
            }
        }
        changes
    }
}

fn le16(report: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([report[at], report[at + 1]])
}

/// The two report families number their sides the opposite way round, as
/// captured from Steam driving the real unit: 0x81 side 1 is the left pad,
/// 0x82 side 0 is; side 2 is both for either. Other sides have no pad.
fn sides(report: u8, side: u8) -> u8 {
    match side {
        2 => LEFT | RIGHT,
        0 | 1 if (report == 0x82) == (side == 0) => LEFT,
        0 | 1 => RIGHT,
        _ => 0,
    }
}

/// A gain in dB relative to full scale as a motor magnitude; positive gains clip.
fn magnitude_from_db(gain: i8) -> u16 {
    if gain >= 0 {
        return u16::MAX;
    }
    (65535.0 * 10f64.powf(f64::from(gain) / 20.0)).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn haptic(report: &[u8]) -> SteamHaptic {
        SteamHaptic::new(0, report).unwrap()
    }

    #[test]
    fn pulses_and_commands_number_their_sides_the_opposite_way_round() {
        let now = Instant::now();
        // Steam's single 400 us UI pulse, as captured from the wired unit.
        for (report, side, expected) in [
            (0x81, 1, LEFT),
            (0x81, 0, RIGHT),
            (0x81, 2, LEFT | RIGHT),
            (0x82, 0, LEFT),
            (0x82, 1, RIGHT),
            (0x82, 2, LEFT | RIGHT),
        ] {
            let mut rumble = SteamRumble::default();
            let bytes = if report == 0x81 {
                vec![0x81, side, 0x90, 0x01, 0x00, 0x00, 0x01, 0x00]
            } else {
                vec![0x82, side, 0x02, 0x00]
            };
            assert_eq!(rumble.apply(&haptic(&bytes), now), expected);
            let on = |bit| if expected & bit != 0 { u16::MAX } else { 0 };
            assert_eq!(rumble.rumble(now), (on(LEFT), on(RIGHT)));
        }
    }

    #[test]
    fn a_side_without_a_pad_changes_nothing() {
        let now = Instant::now();
        let mut rumble = SteamRumble::default();
        rumble.apply(&haptic(&[0x82, 2, 4, 0]), now);
        let before = rumble;
        for side in [3, 4, 0xff] {
            assert_eq!(rumble.apply(&haptic(&[0x82, side, 0, 0]), now), 0);
            assert_eq!(
                rumble.apply(&haptic(&[0x81, side, 0, 0, 0, 0, 0, 0]), now),
                0
            );
        }
        assert_eq!(rumble, before);
    }

    #[test]
    fn rumble_reports_stop_after_the_units_safety_timeout() {
        let now = Instant::now();
        let mut rumble = SteamRumble::default();
        // type, intensity, left 0x1234 gain 0, right 0xabcd gain 0.
        let report = [0x80, 0, 0xff, 0xff, 0x34, 0x12, 0, 0xcd, 0xab, 0];
        assert_eq!(rumble.apply(&haptic(&report), now), LEFT | RIGHT);
        assert_eq!(rumble.rumble(now), (0x1234, 0xabcd));
        assert_eq!(rumble.deadline(now), Some(now + RUMBLE_TIMEOUT));
        let late = now + RUMBLE_TIMEOUT - Duration::from_millis(1);
        assert_eq!(rumble.rumble(late), (0x1234, 0xabcd));
        assert_eq!(rumble.rumble(now + RUMBLE_TIMEOUT), (0, 0));
        // Steam's re-send keeps it going; its zero report stops it at once.
        rumble.apply(&haptic(&report), late);
        assert_eq!(rumble.rumble(now + RUMBLE_TIMEOUT), (0x1234, 0xabcd));
        rumble.apply(&haptic(&[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0]), late);
        assert_eq!(rumble.rumble(late), (0, 0));
        assert_eq!(rumble.deadline(now), None);
    }

    #[test]
    fn pulse_trains_hold_for_their_length_at_their_duty_cycle() {
        let now = Instant::now();
        let mut rumble = SteamRumble::default();
        // 1 ms on, 3 ms off, 100 times: a quarter for 400 ms on the left pad.
        rumble.apply(&haptic(&[0x81, 1, 0xe8, 0x03, 0xb8, 0x0b, 100, 0]), now);
        let held = now + Duration::from_millis(399);
        assert_eq!(rumble.rumble(held), (16383, 0));
        assert_eq!(rumble.rumble(now + Duration::from_millis(400)), (0, 0));
        // The 400 us UI pulse is held for the minimum a motor can show.
        rumble.apply(&haptic(&[0x81, 0, 0x90, 0x01, 0, 0, 1, 0]), now);
        assert_eq!(rumble.deadline(now), Some(now + MIN_HOLD));
        assert_eq!(rumble.rumble(now + MIN_HOLD), (16383, 0));
        // A zero-repeat stop silences its side only.
        rumble.apply(&haptic(&[0x81, 1, 0x90, 0x01, 0, 0, 0, 0]), now);
        assert_eq!(rumble.rumble(now), (0, u16::MAX));
        // An endless train is capped.
        rumble.apply(&haptic(&[0x81, 1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]), now);
        let capped = now + MAX_HOLD - Duration::from_millis(1);
        assert_eq!(rumble.rumble(capped), (32767, 0));
        assert_eq!(rumble.rumble(now + MAX_HOLD), (0, 0));
    }

    #[test]
    fn ticks_and_clicks_hold_briefly_and_continuous_commands_until_off() {
        let now = Instant::now();
        let mut rumble = SteamRumble::default();
        // Steam's tick (-3 dB) and click (-14 dB) as the wired unit gets them.
        rumble.apply(&haptic(&[0x82, 0, 0x01, 0xfd]), now);
        rumble.apply(&haptic(&[0x82, 1, 0x02, 0xf2]), now);
        assert_eq!(rumble.rumble(now), (46395, 13076));
        assert_eq!(rumble.deadline(now), Some(now + CLICK_HOLD));
        assert_eq!(rumble.rumble(now + CLICK_HOLD), (0, 0));
        // A rumble command (4) at +6 dB clips and runs until its off, each
        // re-send holding it up to MAX_HOLD again.
        rumble.apply(&haptic(&[0x82, 2, 0x04, 0x06]), now);
        let later = now + Duration::from_secs(5);
        rumble.apply(&haptic(&[0x82, 2, 0x04, 0x06]), later);
        assert_eq!(rumble.rumble(now + MAX_HOLD), (u16::MAX, u16::MAX));
        rumble.apply(&haptic(&[0x82, 0, 0x00, 0x06]), later);
        assert_eq!(rumble.rumble(later), (0, u16::MAX));
        assert_eq!(rumble.rumble(later + MAX_HOLD), (0, 0));
    }

    #[test]
    fn deadlines_move_on_once_a_hold_has_run_out() {
        let now = Instant::now();
        let mut rumble = SteamRumble::default();
        // A click on the left pad, a 400 ms pulse train on the right.
        rumble.apply(&haptic(&[0x82, 0, 0x02, 0x00]), now);
        rumble.apply(&haptic(&[0x81, 0, 0xe8, 0x03, 0xb8, 0x0b, 100, 0]), now);
        let train = now + Duration::from_millis(400);
        assert_eq!(rumble.deadline(now), Some(now + CLICK_HOLD));
        assert_eq!(rumble.deadline(now + CLICK_HOLD), Some(train));
        assert_eq!(rumble.rumble(now + CLICK_HOLD), (0, 16383));
        assert_eq!(rumble.deadline(train), None);
        assert_eq!(rumble.rumble(train), (0, 0));
    }

    #[test]
    fn no_command_leaves_the_motors_on_past_the_longest_hold() {
        let now = Instant::now();
        for command in 0..=u8::MAX {
            for side in 0..=2 {
                let mut rumble = SteamRumble::default();
                let sides = rumble.apply(&haptic(&[0x82, side, command, 0x06]), now);
                // Scripts, sweeps and unknown commands change nothing.
                assert_eq!(sides == 0, command > 5, "command {command}");
                if rumble.rumble(now) != (0, 0) {
                    assert!(rumble.deadline(now).is_some(), "command {command}");
                }
                assert_eq!(rumble.rumble(now + MAX_HOLD), (0, 0), "command {command}");
            }
        }
    }

    #[test]
    fn short_and_unrenderable_reports_are_ignored() {
        let now = Instant::now();
        // The codec's golden 0x5504 payload: a 5-byte 0x81 report, too short
        // for a pulse.
        let golden = [
            0x02, 0x01, 5, 0x81, 0x01, 0x20, 0x00, 0x40, 0, 0, 0, 0, 0, 0, 0,
        ];
        let decoded = SteamHaptic::decode(&golden).unwrap();
        assert_eq!(decoded.controller, 0x0102);
        let mut rumble = SteamRumble::default();
        assert_eq!(rumble.apply(&decoded, now), 0);
        for report in [
            &[0x80, 0, 0, 0, 0xff, 0xff, 0, 0xff][..],
            &[0x82, 0, 2],
            &[0x83, 2, 1, 2, 3, 4, 5, 6],
            &[0x84, 2, 1, 2, 3, 4, 5, 6],
            &[0x85, 2, 1],
            &[0x7f, 2, 1, 0],
        ] {
            assert_eq!(rumble.apply(&haptic(report), now), 0);
        }
        assert_eq!(rumble, SteamRumble::default());
    }

    #[test]
    fn cleared_controllers_stop_once_and_drop_their_holds() {
        let now = Instant::now();
        let tone = |controller| SteamHaptic::new(controller, &[0x82, 2, 3, 0]).unwrap();
        let mut pads = SteamPads::default();
        pads.apply(&tone(0), now);
        pads.apply(&tone(1), now);
        pads.apply(&tone(16), now);
        let on = (u16::MAX, u16::MAX);
        assert_eq!(pads.changes(now), [(0, on.0, on.1), (1, on.0, on.1)]);
        assert_eq!(pads.changes(now), []);
        // Removing controller 1 stops it once; controller 0 keeps going.
        pads.retain(1);
        assert_eq!(pads.changes(now), [(1, 0, 0)]);
        pads.retain(1);
        assert_eq!(pads.changes(now), []);
        // Clearing everything drops the 10 s hold: nothing restarts it.
        pads.retain(0);
        assert_eq!(pads.changes(now), [(0, 0, 0)]);
        assert_eq!(pads.changes(now + Duration::from_millis(1)), []);
        assert_eq!(pads, SteamPads::default());
        // A controller that never ran sends no stop.
        pads.retain(0);
        assert_eq!(pads.changes(now), []);
    }
}
