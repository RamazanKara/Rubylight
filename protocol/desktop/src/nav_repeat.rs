//! Hold-to-repeat for launcher navigation with a controller: a held D-pad direction or left
//! stick steps again after 400 ms, then every 150 ms, and every 75 ms once held for 1.6 s.
//! The cadence is Moonlight V+'s (SdlGamepadKeyNavigation::axisNavRepeatDelayMs); V+ polls
//! every 50 ms, here the launcher wakes at the next step instead.

use std::time::{Duration, Instant};

/// Where a held direction comes from, so its release (or a live check) finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// A D-pad button (SDL_GamepadButton).
    Button(u8),
    /// The left stick past its threshold on this axis (SDL_GamepadAxis), in this direction.
    Axis(u8, bool),
}

struct Hold {
    pad: u32,
    input: Input,
    key: egui::Key,
    since: Instant,
    last: Instant,
}

/// (held at least, step) tiers, as in V+.
const TIERS: [(Duration, Duration); 3] = [
    (Duration::ZERO, Duration::from_millis(400)),
    (Duration::from_millis(400), Duration::from_millis(150)),
    (Duration::from_millis(1600), Duration::from_millis(75)),
];

/// The step after a direction has been held this long.
pub fn step(held: Duration) -> Duration {
    TIERS.iter().rev().find(|(from, _)| held >= *from).map_or(TIERS[0].1, |t| t.1)
}

#[derive(Default)]
pub struct NavRepeat {
    holds: Vec<Hold>,
}

impl NavRepeat {
    /// A direction went down and was sent once already: it repeats from now while held.
    pub fn hold(&mut self, pad: u32, input: Input, key: egui::Key, now: Instant) {
        self.release(pad, input);
        self.holds.push(Hold { pad, input, key, since: now, last: now });
    }

    pub fn release(&mut self, pad: u32, input: Input) {
        self.holds.retain(|h| !(h.pad == pad && same_source(h.input, input)));
    }

    pub fn clear(&mut self) {
        self.holds.clear();
    }

    /// The keys due by `now`. A hold whose input `held` says is no longer down (a release
    /// that went elsewhere, a controller that left) is dropped without a step.
    pub fn due(&mut self, now: Instant, held: impl Fn(u32, Input) -> bool) -> Vec<egui::Key> {
        self.holds.retain(|h| held(h.pad, h.input));
        let mut keys = Vec::new();
        for h in &mut self.holds {
            if now.saturating_duration_since(h.last) >= step(now.saturating_duration_since(h.since)) {
                h.last = now;
                keys.push(h.key);
            }
        }
        keys
    }

    /// How long until the next step is due, if anything is held.
    pub fn next(&self, now: Instant) -> Option<Duration> {
        self.holds.iter().map(|h| next_step(h.since, h.last).saturating_duration_since(now)).min()
    }
}

/// A stick axis is one source whatever its direction: flipping it replaces the hold.
fn same_source(a: Input, b: Input) -> bool {
    match (a, b) {
        (Input::Axis(a, _), Input::Axis(b, _)) => a == b,
        _ => a == b,
    }
}

/// The first moment `t` with `t - last >= step(t - since)`: the step only shortens as the hold
/// lasts, so the first tier where that falls inside the tier wins.
fn next_step(since: Instant, last: Instant) -> Instant {
    for (i, (from, delay)) in TIERS.iter().enumerate() {
        let t = (last + *delay).max(since + *from);
        if TIERS.get(i + 1).is_none_or(|(until, _)| t < since + *until) {
            return t;
        }
    }
    unreachable!("the last tier has no end")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    /// Steps of one held direction, sampled every millisecond for `ms`.
    fn steps_over(ms: u64) -> Vec<u64> {
        let start = Instant::now();
        let mut nav = NavRepeat::default();
        nav.hold(1, Input::Button(11), egui::Key::ArrowUp, start);
        (1..=ms).filter(|t| !nav.due(start + MS(*t), |_, _| true).is_empty()).collect()
    }

    #[test]
    fn a_held_direction_repeats_at_v_plus_cadence() {
        assert_eq!(step(MS(0)), MS(400));
        assert_eq!(step(MS(399)), MS(400));
        assert_eq!(step(MS(400)), MS(150));
        assert_eq!(step(MS(1599)), MS(150));
        assert_eq!(step(MS(1600)), MS(75));
        let steps = steps_over(2000);
        // 400, then every 150 ms to 1600 (where 75 ms takes over), then every 75 ms.
        assert_eq!(steps, [400, 550, 700, 850, 1000, 1150, 1300, 1450, 1600, 1675, 1750, 1825, 1900, 1975]);
    }

    #[test]
    fn the_launcher_wakes_exactly_when_the_next_step_is_due() {
        let start = Instant::now();
        let mut nav = NavRepeat::default();
        assert_eq!(nav.next(start), None);
        nav.hold(1, Input::Axis(1, true), egui::Key::ArrowDown, start);
        let mut now = start;
        let mut steps = Vec::new();
        while now < start + MS(2000) {
            now += nav.next(now).unwrap();
            assert_eq!(nav.due(now, |_, _| true), [egui::Key::ArrowDown], "woken at {:?}", now - start);
            steps.push((now - start).as_millis() as u64);
        }
        assert_eq!(steps, steps_over(now.duration_since(start).as_millis() as u64));
    }

    #[test]
    fn release_or_a_lost_hold_stops_the_repeat() {
        let start = Instant::now();
        let mut nav = NavRepeat::default();
        nav.hold(1, Input::Button(11), egui::Key::ArrowUp, start);
        nav.hold(2, Input::Button(11), egui::Key::ArrowUp, start);
        nav.release(1, Input::Button(11));
        // Pad 2's button came up while the event went elsewhere: the live check drops it.
        assert!(nav.due(start + MS(500), |pad, _| pad != 2).is_empty());
        assert_eq!(nav.next(start), None);
    }

    #[test]
    fn a_flipped_stick_replaces_its_hold() {
        let start = Instant::now();
        let mut nav = NavRepeat::default();
        nav.hold(1, Input::Axis(0, false), egui::Key::ArrowLeft, start);
        nav.hold(1, Input::Axis(0, true), egui::Key::ArrowRight, start + MS(100));
        assert_eq!(nav.due(start + MS(500), |_, _| true), [egui::Key::ArrowRight]);
        nav.release(1, Input::Axis(0, true));
        assert_eq!(nav.next(start), None);
    }
}
