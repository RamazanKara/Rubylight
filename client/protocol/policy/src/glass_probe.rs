//! How real a surface can afford to be, the same rule on Android (`GlassTier`, `GlassProbe`) and on
//! the desktop (`live_glass`): four tiers, where a device lands by what it can do, and a frame
//! probe that steps a live tier down when it does not hold the display's rate.
//!
//! Nothing here reads a device's name. A tier is chosen from capabilities ([`Device`]), then watched
//! over the first [`PROBE_FRAMES`] frames once the screen has settled ([`Probe`]). The crash guard
//! ([`Guard`]) keeps a driver that dies while it allocates the first live target from doing so on
//! every launch.

/// A surface tier, from the most real to the plainest. The order is the step-down order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Live blur plus the refracted edge band.
    Lens,
    /// Live blur of what is really behind the surface.
    Live,
    /// The static ambient sampled in screen space under the fill: a finished look at no GPU cost.
    Baked,
    /// High contrast: an opaque fill and a light rim.
    Solid,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Lens, Tier::Live, Tier::Baked, Tier::Solid];

    /// The name both apps store and log.
    pub fn key(self) -> &'static str {
        match self {
            Tier::Lens => "lens",
            Tier::Live => "live",
            Tier::Baked => "baked",
            Tier::Solid => "solid",
        }
    }

    /// The tier of a stored name; Android's older upper-case names read too.
    pub fn parse(key: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|tier| tier.key().eq_ignore_ascii_case(key))
    }

    /// One tier down, as a failed probe steps. Baked and solid cost nothing and stay.
    pub fn step_down(self) -> Tier {
        match self {
            Tier::Lens => Tier::Live,
            Tier::Live | Tier::Baked => Tier::Baked,
            Tier::Solid => Tier::Solid,
        }
    }

    /// Whether this tier does GPU work a probe has to watch.
    pub fn probed(self) -> bool {
        matches!(self, Tier::Lens | Tier::Live)
    }
}

/// What the device can do right now. Battery saver and heat are read each time and never stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// The lens band can be drawn (Android 13's runtime shaders; the desktop's render targets).
    pub lens: bool,
    /// Live blur can be drawn (Android 12's render effects; the desktop's render targets).
    pub live: bool,
    pub low_ram: bool,
    pub power_save: bool,
    /// The thermal status is severe or worse.
    pub thermal_severe: bool,
    pub high_contrast: bool,
    /// The last probe on this build and driver never finished ([`Guard::tripped`]).
    pub crash_guard_tripped: bool,
}

/// Where a device lands: high contrast is always solid, a device that cannot or should not blur
/// is baked, and anything else takes its best tier unless a probe already stepped it down.
pub fn land(device: &Device, stored: Option<Tier>) -> Tier {
    if device.high_contrast {
        return Tier::Solid;
    }
    if !device.live || device.low_ram || device.power_save || device.thermal_severe || device.crash_guard_tripped {
        return Tier::Baked;
    }
    let top = if device.lens { Tier::Lens } else { Tier::Live };
    match stored {
        // A stored solid is a high-contrast leftover: the probe never writes it.
        Some(stored) if stored > top && stored != Tier::Solid => stored,
        _ => top,
    }
}

/// Frames a probe watches (`tier.probe.frames`).
pub const PROBE_FRAMES: u32 = 60;
/// The share of the frame budget the 90th percentile of the work may take (`tier.probe.p90BudgetFraction`).
pub const P90_BUDGET_FRACTION: f32 = 0.70;
/// What a screen draws as it opens says nothing of how it moves: a first launch compiles its
/// shaders in these frames (`tier.probe.settleMs`).
pub const SETTLE_MS: u64 = 2000;
/// A frame counts as slow only if it also lands more than this share of the budget late
/// (`tier.probe.lateFraction`): a 120 Hz handheld at low clocks goes over its share of work on
/// frames that all land on time.
pub const LATE_FRACTION: f32 = 0.25;
/// The budget never asks for more than this rate (`tier.probe.minBudgetHz`): a faster screen keeps
/// its live surfaces while they fit a 120 Hz frame.
pub const MIN_BUDGET_HZ: f32 = 120.0;

/// The frame budget at `refresh_hz`, in nanoseconds, floored at [`MIN_BUDGET_HZ`].
pub fn budget_ns(refresh_hz: f32) -> u64 {
    let hz = if refresh_hz.is_finite() && refresh_hz >= 1.0 { refresh_hz.min(MIN_BUDGET_HZ) } else { 60.0 };
    (1e9_f64 / f64::from(hz)).round() as u64
}

/// How many slow frames put the 90th percentile of `frames` frames over the line.
pub fn slow_frames_to_fail(frames: u32) -> u32 {
    if frames == 0 {
        return 1;
    }
    let p90_index = (f64::from(frames) * 0.9).ceil() as u32;
    frames - (p90_index.saturating_sub(1)).min(frames - 1)
}

/// What a probe found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The tier holds the rate: keep it for this build and driver.
    Keep(Tier),
    /// It does not: draw this tier from now on and keep that.
    StepDown(Tier),
}

impl Verdict {
    /// The tier to store and draw.
    pub fn tier(self) -> Tier {
        match self {
            Verdict::Keep(tier) | Verdict::StepDown(tier) => tier,
        }
    }
}

/// Watches the frames of one tier on one screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    tier: Tier,
    budget_ns: u64,
    frames: u32,
    count: u32,
    slow: u32,
    verdict: Option<Verdict>,
}

impl Probe {
    /// A probe of `tier` on a screen refreshing at `refresh_hz`; `None` for a tier with nothing to watch.
    pub fn new(tier: Tier, refresh_hz: f32) -> Option<Probe> {
        tier.probed().then(|| Probe { tier, budget_ns: budget_ns(refresh_hz), frames: PROBE_FRAMES, count: 0, slow: 0, verdict: None })
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Whether a screen opened `elapsed_ms` ago has settled.
    pub fn settled(elapsed_ms: u64) -> bool {
        elapsed_ms >= SETTLE_MS
    }

    /// Whether a frame was over its share of the budget in work and also late on screen. Where the
    /// platform cannot tell when a frame landed (`total_ns` is `None`), the work alone decides.
    pub fn slow(&self, work_ns: u64, total_ns: Option<u64>) -> bool {
        let allowed = (self.budget_ns as f64 * f64::from(P90_BUDGET_FRACTION)) as u64;
        let late = (self.budget_ns as f64 * f64::from(1.0 + LATE_FRACTION)) as u64;
        work_ns > allowed && total_ns.is_none_or(|total| total > late)
    }

    /// One real frame, drawn `elapsed_ms` after the screen opened. Returns the verdict once it is
    /// decided: as soon as enough slow frames have settled the 90th percentile, or after
    /// [`PROBE_FRAMES`] frames. Frames before the screen settled, and after the verdict, count nothing.
    pub fn frame(&mut self, elapsed_ms: u64, work_ns: u64, total_ns: Option<u64>) -> Option<Verdict> {
        if self.verdict.is_some() || !Self::settled(elapsed_ms) {
            return None;
        }
        if self.slow(work_ns, total_ns) {
            self.slow += 1;
        }
        self.count += 1;
        let fail = self.slow >= slow_frames_to_fail(self.frames);
        if !fail && self.count < self.frames {
            return None;
        }
        self.verdict = Some(if fail { Verdict::StepDown(self.tier.step_down()) } else { Verdict::Keep(self.tier) });
        self.verdict
    }

    /// Frames counted and how many of them were slow, for the log line.
    pub fn counts(&self) -> (u32, u32) {
        (self.count, self.slow)
    }
}

/// What the log says when the guard trips. Never shown as an error: baked is a finished look.
pub const GUARD_WARNING: &str = "Previous live-surface probe did not finish; using baked surfaces";

/// Unfinished probes in a row on one build and driver before the desktop lands on baked for good.
/// One crash or kill between landing and the first clean frame can be anything's (the app closed
/// by force, a device reset, a stream's crash); a second in a row is taken for the driver's.
pub const GUARD_TRIPS: u32 = 2;

/// The two-phase crash guard around the first live target a build allocates on a driver. The
/// platform stores it: [`Guard::arm`] before the allocation, [`Guard::clear`] after the first
/// frame presented. A launch that finds it still armed for the same build and driver
/// ([`Guard::tripped`]) lands on baked and stores that, so a crashing driver is met once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guard {
    pub in_flight: bool,
    /// The app build and the OS build or GPU driver, as the platform names them.
    pub fingerprint: String,
}

impl Guard {
    /// The state to commit before the first lens or live target is allocated.
    pub fn arm(fingerprint: &str) -> Guard {
        Guard { in_flight: true, fingerprint: fingerprint.to_owned() }
    }

    /// The state to commit once the first lens or live frame has presented.
    pub fn clear(fingerprint: &str) -> Guard {
        Guard { in_flight: false, fingerprint: fingerprint.to_owned() }
    }

    /// Whether the last probe on this very build and driver never finished. An update of either
    /// gets a fresh try.
    pub fn tripped(&self, fingerprint: &str) -> bool {
        self.in_flight && self.fingerprint == fingerprint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn able() -> Device {
        Device { lens: true, live: true, low_ram: false, power_save: false, thermal_severe: false, high_contrast: false, crash_guard_tripped: false }
    }

    #[test]
    fn a_device_lands_by_what_it_can_do() {
        assert_eq!(land(&able(), None), Tier::Lens);
        assert_eq!(land(&Device { lens: false, ..able() }, None), Tier::Live);
        assert_eq!(land(&Device { live: false, lens: false, ..able() }, None), Tier::Baked);
        for device in [
            Device { low_ram: true, ..able() },
            Device { power_save: true, ..able() },
            Device { thermal_severe: true, ..able() },
            Device { crash_guard_tripped: true, ..able() },
        ] {
            assert_eq!(land(&device, None), Tier::Baked, "{device:?}");
            assert_eq!(land(&device, Some(Tier::Lens)), Tier::Baked, "a stored tier never outranks the moment");
        }
    }

    #[test]
    fn high_contrast_is_solid_whatever_else_holds() {
        let device = Device { high_contrast: true, power_save: true, ..able() };
        assert_eq!(land(&device, Some(Tier::Lens)), Tier::Solid);
        assert_eq!(land(&Device { high_contrast: true, live: false, ..able() }, None), Tier::Solid);
    }

    #[test]
    fn a_stored_verdict_only_ever_lowers_the_tier() {
        assert_eq!(land(&able(), Some(Tier::Live)), Tier::Live);
        assert_eq!(land(&able(), Some(Tier::Baked)), Tier::Baked);
        assert_eq!(land(&Device { lens: false, ..able() }, Some(Tier::Lens)), Tier::Live, "a stored lens on a device that lost it");
        assert_eq!(land(&able(), Some(Tier::Solid)), Tier::Lens, "leaving high contrast restores the device's tier");
    }

    #[test]
    fn tiers_step_down_one_at_a_time_and_stop_at_baked() {
        assert_eq!(Tier::Lens.step_down(), Tier::Live);
        assert_eq!(Tier::Live.step_down(), Tier::Baked);
        assert_eq!(Tier::Baked.step_down(), Tier::Baked);
        assert_eq!(Tier::Solid.step_down(), Tier::Solid);
        for tier in Tier::ALL {
            assert_eq!(Tier::parse(tier.key()), Some(tier));
            assert_eq!(Tier::parse(&tier.key().to_uppercase()), Some(tier), "Android stored the enum's name");
        }
        assert_eq!(Tier::parse("glass"), None);
    }

    #[test]
    fn the_budget_floors_at_120_hz() {
        assert_eq!(budget_ns(60.0), 16_666_667);
        assert_eq!(budget_ns(120.0), 8_333_333);
        assert_eq!(budget_ns(144.0), 8_333_333);
        assert_eq!(budget_ns(240.0), 8_333_333);
        assert_eq!(budget_ns(0.0), 16_666_667, "an unknown rate reads as 60 Hz");
        assert_eq!(budget_ns(f32::NAN), 16_666_667);
    }

    #[test]
    fn seven_slow_frames_in_sixty_decide_the_p90() {
        assert_eq!(slow_frames_to_fail(60), 7);
        assert_eq!(slow_frames_to_fail(10), 2);
        assert_eq!(slow_frames_to_fail(1), 1);
        assert_eq!(slow_frames_to_fail(0), 1);
    }

    #[test]
    fn frames_before_the_screen_settled_count_nothing() {
        let mut probe = Probe::new(Tier::Lens, 60.0).unwrap();
        for _ in 0..200 {
            assert_eq!(probe.frame(SETTLE_MS - 1, 50_000_000, Some(50_000_000)), None);
        }
        assert_eq!(probe.counts(), (0, 0));
        assert!(Probe::settled(SETTLE_MS));
    }

    #[test]
    fn a_frame_is_slow_only_when_it_is_also_late() {
        let probe = Probe::new(Tier::Live, 120.0).unwrap();
        // 6.5 ms of work is over 70 % of 8.33 ms, but the frame landed on time.
        assert!(!probe.slow(6_500_000, Some(8_400_000)));
        assert!(probe.slow(6_500_000, Some(11_000_000)));
        assert!(!probe.slow(5_000_000, Some(20_000_000)), "late for another reason than this work");
        assert!(probe.slow(6_500_000, None), "no landing time: the work decides");
    }

    #[test]
    fn a_slow_tier_steps_down_as_soon_as_the_p90_is_decided() {
        let mut probe = Probe::new(Tier::Lens, 60.0).unwrap();
        let mut verdict = None;
        let mut frames = 0;
        while verdict.is_none() {
            verdict = probe.frame(SETTLE_MS, 14_000_000, Some(33_000_000));
            frames += 1;
        }
        assert_eq!(verdict, Some(Verdict::StepDown(Tier::Live)));
        assert_eq!(frames, 7);
        assert_eq!(probe.frame(SETTLE_MS, 1, Some(1)), None, "a decided probe counts no more");
    }

    #[test]
    fn a_fast_tier_is_kept_after_sixty_frames() {
        let mut probe = Probe::new(Tier::Live, 60.0).unwrap();
        for frame in 0..PROBE_FRAMES {
            // Six slow frames are still inside the 90th percentile.
            let slow = frame < 6;
            let verdict = probe.frame(SETTLE_MS + u64::from(frame), if slow { 14_000_000 } else { 4_000_000 }, Some(if slow { 33_000_000 } else { 16_000_000 }));
            if frame + 1 < PROBE_FRAMES {
                assert_eq!(verdict, None);
            } else {
                assert_eq!(verdict, Some(Verdict::Keep(Tier::Live)));
                assert_eq!(verdict.unwrap().tier(), Tier::Live);
            }
        }
    }

    #[test]
    fn only_live_tiers_are_probed() {
        assert!(Probe::new(Tier::Baked, 60.0).is_none());
        assert!(Probe::new(Tier::Solid, 60.0).is_none());
        assert_eq!(Probe::new(Tier::Lens, 60.0).unwrap().tier(), Tier::Lens);
    }

    #[test]
    fn the_guard_trips_only_for_the_build_and_driver_that_armed_it() {
        let armed = Guard::arm("v700-driver1");
        assert!(armed.tripped("v700-driver1"));
        assert!(!armed.tripped("v701-driver1"), "an update gets a fresh try");
        assert!(!Guard::clear("v700-driver1").tripped("v700-driver1"));
        assert!(!Guard::default().tripped(""));
        assert!(!GUARD_WARNING.is_empty());
    }
}
