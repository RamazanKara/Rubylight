//! Automatic quality: nightjar-policy's bounded resolution/frame-rate ladder (Android's), asked for
//! in the desktop's millihertz requests.

use crate::config::QualityMode;
use nightjar_policy::quality::{AdaptiveQualityController, Bounds, DecodeTiming, Mode, QualityError, Target};
use nightjar_policy::stats::Stats;
use rubylight_protocol::control::Reconfigure;

pub fn target(width: u32, height: u32, fps_millihz: u32) -> Option<Reconfigure> {
    let request = Reconfigure { width: width.try_into().ok()?, height: height.try_into().ok()?, fps_millihz };
    Reconfigure::decode(&request.encode())
}

fn mode(mode: QualityMode) -> Mode {
    match mode {
        QualityMode::Off => Mode::Off,
        QualityMode::Motion => Mode::Motion,
        QualityMode::Detail => Mode::Detail,
        QualityMode::Battery => Mode::Battery,
    }
}

/// The ladder counts whole frames per second; a fractional ceiling such as 59.94 is its rung at 60.
fn hz(fps_millihz: u32) -> u32 { fps_millihz.div_ceil(1000) }

/// One decoder sampling window of the pipeline's [`DecodeWindow`](super::pipeline::DecodeWindow),
/// on the session clock in microseconds. `generation` changes with every requested mode, so a window
/// that spans a change never counts.
pub fn decode_timing(generation: i64, start_us: u64, end_us: u64, fps: u32, window: &super::pipeline::DecodeWindow) -> Option<DecodeTiming> {
    // The early-decode path counts frames without their times; no times, no tail evidence.
    if window.first_us == 0 { return None; }
    let ns = |us: u64| i64::try_from(us.saturating_mul(1000)).unwrap_or(i64::MAX);
    Some(DecodeTiming {
        generation,
        window_start_ns: ns(start_us),
        window_end_ns: ns(end_us),
        first_sample_ns: ns(window.first_us),
        last_sample_ns: ns(window.last_us),
        fps: i32::try_from(fps).unwrap_or(i32::MAX),
        samples: i32::try_from(window.samples).unwrap_or(i32::MAX),
        late_frames: i32::try_from(window.late).unwrap_or(i32::MAX),
    })
}

pub struct Quality {
    policy: AdaptiveQualityController,
    /// The exact request at the top of the ladder.
    ceiling: Reconfigure,
}

impl Quality {
    /// The floors below the protocol's 256 lines are raised to it.
    pub fn new(quality: QualityMode, ceiling: Reconfigure, min_edge: u32, min_fps: u32, now: u64) -> Result<Self, QualityError> {
        let bounds = Bounds { min_short_edge: min_edge.max(256), min_fps, align_width: 2, align_height: 2 };
        let policy = AdaptiveQualityController::new(mode(quality), u32::from(ceiling.width), u32::from(ceiling.height),
            hz(ceiling.fps_millihz), bounds, now)?;
        Ok(Self { policy, ceiling })
    }

    fn request(&self, rung: Target) -> Option<Reconfigure> {
        let fps_millihz = if rung.fps == hz(self.ceiling.fps_millihz) { self.ceiling.fps_millihz } else { rung.fps * 1000 };
        target(rung.width, rung.height, fps_millihz)
    }

    fn rung(request: Reconfigure) -> Target {
        Target { width: u32::from(request.width), height: u32::from(request.height), fps: hz(request.fps_millihz) }
    }

    pub fn current(&self) -> Option<Reconfigure> { self.request(self.policy.current()) }

    /// A display change keeps the reduction level; returns the request at that level.
    pub fn rebase(&mut self, ceiling: Reconfigure, now: u64) -> Result<Option<Reconfigure>, QualityError> {
        self.policy.resize_ceiling(u32::from(ceiling.width), u32::from(ceiling.height), hz(ceiling.fps_millihz), now)?;
        self.ceiling = ceiling;
        Ok(self.current())
    }

    /// `bitrate_has_room` gives automatic bitrate the first answer to network pressure.
    pub fn sample(&mut self, stats: &Stats, bitrate_has_room: bool, timing: Option<&DecodeTiming>) -> Option<Reconfigure> {
        let next = self.policy.sample_timed(stats, bitrate_has_room, timing)?;
        let request = self.request(next);
        if request.is_none() {
            eprintln!("nightjar: automatic quality: {next} is no valid stream request");
        }
        request
    }

    pub fn applied(&mut self, request: Reconfigure, now: u64) -> Result<(), QualityError> {
        self.policy.applied(Self::rung(request), now)
    }

    pub fn suspend(&mut self) { self.policy.suspend(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::pipeline::DecodeWindow;

    fn tick(now_ms: u64) -> Stats {
        Stats { now_ms, recent_video: true, fps: 60, loss_percent: Some(0.0), decode_ms: Some(2.0),
            jitter_ms: Some(1.0), queue_ms: Some(1.0), ..Default::default() }
    }

    /// A healthy one-second window ending at `now_ms`, 60 frames over 980 ms.
    fn window(now_ms: u64, generation: i64, fps: u32) -> Option<DecodeTiming> {
        let end = now_ms * 1000;
        let w = DecodeWindow { samples: 60, first_us: end - 990_000, last_us: end - 10_000, ..Default::default() };
        decode_timing(generation, end - 1_000_000, end, fps, &w)
    }

    fn levels(q: &mut Quality) -> Vec<Reconfigure> {
        // A slow decoder walks every mode down its whole ladder, one rung per 15 s.
        let mut seen = vec![q.current().unwrap()];
        for step in 0..64 {
            let now = 15_000 * (step + 1);
            let mut s = tick(now);
            s.decode_ms = Some(200.0);
            for k in 0..3 { s.now_ms = now - 3000 + k * 1000; assert_eq!(q.sample(&s, false, None), None); }
            s.now_ms = now;
            let Some(next) = q.sample(&s, false, None) else { break };
            q.applied(next, now).unwrap();
            seen.push(next);
        }
        seen
    }

    #[test]
    fn ladders_keep_floors_aspect_priority_and_valid_requests() {
        let top = target(1920, 1080, 144_000).unwrap();
        for quality in [QualityMode::Motion, QualityMode::Detail, QualityMode::Battery] {
            let mut q = Quality::new(quality, top, 720, 30, 0).unwrap();
            let seen = levels(&mut q);
            assert!(seen.len() > 3, "{quality:?}: {seen:?}");
            for t in &seen {
                assert!(t.width <= 1920 && (720..=1080).contains(&t.height));
                assert!((30_000..=144_000).contains(&t.fps_millihz));
                assert!((f64::from(t.width) / f64::from(t.height) - 16.0 / 9.0).abs() < 0.005);
                assert_eq!(Reconfigure::decode(&t.encode()), Some(*t));
            }
            if quality == QualityMode::Motion { assert_eq!(seen[1].fps_millihz, 144_000); }
            if quality == QualityMode::Detail { assert_eq!(seen[1].width, 1920); }
        }
        assert!(target(100_000, 1080, 60_000).is_none());
        assert!(target(1921, 1080, 60_000).is_none());
        // The protocol's 256-line minimum holds even when the setting asks for less.
        let mut small = Quality::new(QualityMode::Motion, target(640, 360, 60_000).unwrap(), 0, 15, 0).unwrap();
        assert!(levels(&mut small).iter().all(|t| t.height >= 256));
    }

    #[test]
    fn a_fractional_ceiling_keeps_its_exact_rate() {
        let top = target(1920, 1080, 59_940).unwrap();
        let mut q = Quality::new(QualityMode::Detail, top, 720, 30, 0).unwrap();
        assert_eq!(q.current(), Some(top));
        let seen = levels(&mut q);
        assert_eq!(seen[1], target(1920, 1080, 45_000).unwrap());
        assert!(seen.iter().filter(|t| t.fps_millihz % 1000 != 0).all(|t| t.fps_millihz == 59_940));
    }

    #[test]
    fn display_changes_preserve_adaptive_reduction() {
        let mut q = Quality::new(QualityMode::Motion, target(1920, 1080, 60_000).unwrap(), 720, 30, 0).unwrap();
        let lower = levels(&mut q)[1];
        q.applied(lower, 15_000).unwrap();
        let mode = q.rebase(target(2560, 1440, 120_000).unwrap(), 20_000).unwrap();
        assert_eq!(mode, target(2048, 1152, 120_000));
        // A rung of the old ladder is foreign now.
        assert_eq!(q.applied(lower, 21_000), Err(QualityError::ForeignTarget));
    }

    #[test]
    fn sustained_pressure_and_fresh_recovery_only() {
        let top = target(1920, 1080, 60_000).unwrap();
        let mut q = Quality::new(QualityMode::Motion, top, 720, 30, 0).unwrap();
        for now in 12..15 {
            let mut s = tick(now * 1000);
            s.decode_ms = Some(40.0);
            assert_eq!(q.sample(&s, false, window(now * 1000, 1, 60).as_ref()), None);
        }
        let mut s = tick(15_000);
        s.decode_ms = Some(40.0);
        let lower = q.sample(&s, false, window(15_000, 1, 60).as_ref()).unwrap();
        assert_eq!(q.current(), Some(top));
        q.applied(lower, 15_000).unwrap();
        // Recovery needs 30 s of fresh healthy windows at the new rate.
        for now in 16..46 { assert_eq!(q.sample(&tick(now * 1000), false, window(now * 1000, 2, 60).as_ref()), None); }
        assert_eq!(q.sample(&tick(46_000), false, window(46_000, 2, 60).as_ref()), Some(top));
        // Without decoder timing, a healthy stream never counts as recovered.
        q.suspend();
        for now in 47..90 { assert_eq!(q.sample(&tick(now * 1000), false, None), None); }
        assert_eq!(q.current(), Some(lower));
    }

    #[test]
    fn bitrate_gets_first_chance_at_network_pressure() {
        let mut q = Quality::new(QualityMode::Detail, target(1920, 1080, 60_000).unwrap(), 720, 30, 0).unwrap();
        let lossy = |now: u64| Stats { loss_percent: Some(8.0), ..tick(now) };
        for now in 15..25 { assert_eq!(q.sample(&lossy(now * 1000), true, None), None); }
        for now in 25..32 { assert_eq!(q.sample(&lossy(now * 1000), false, None), None); }
        assert!(q.sample(&lossy(32_000), false, None).is_some());
    }

    #[test]
    fn decode_windows_without_times_are_no_evidence() {
        let w = DecodeWindow { samples: 60, ..Default::default() };
        assert_eq!(decode_timing(1, 0, 1_000_000, 60, &w), None);
        let t = window(5000, 3, 60).unwrap();
        assert_eq!((t.samples, t.late_frames, t.fps, t.generation), (60, 0, 60, 3));
        assert!(t.first_sample_ns > t.window_start_ns && t.last_sample_ns <= t.window_end_ns);
    }
}
