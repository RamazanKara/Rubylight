//! Android's bounded resolution/frame-rate ladder, using fresh desktop decoder observations.

use crate::config::QualityMode;
use rubylight_protocol::control::Reconfigure;

pub fn target(width: u32, height: u32, fps_millihz: u32) -> Option<Reconfigure> {
    let request = Reconfigure { width: width.try_into().ok()?, height: height.try_into().ok()?, fps_millihz };
    Reconfigure::decode(&request.encode())
}

pub struct Sample {
    pub now: u64,
    pub recent: bool,
    pub poor: bool,
    pub loss: f64,
    pub decode_ms: f64,
    pub jitter_ms: f64,
    pub queue_ms: f64,
    pub bitrate_has_room: bool,
    pub samples: u32,
    pub late: u32,
    pub span_us: u64,
}

pub struct Quality {
    levels: Vec<Reconfigure>,
    level: usize,
    battery_ceiling: usize,
    last_change: u64,
    healthy_since: Option<u64>,
    last_sample: Option<u64>,
    device_pressure: u32,
    network_pressure: u32,
    tail_pressure: u32,
}

impl Quality {
    pub fn new(mode: QualityMode, ceiling: Reconfigure, min_edge: u32, min_fps: u32, now: u64) -> Self {
        let (width, height) = (u32::from(ceiling.width), u32::from(ceiling.height));
        let edge = width.min(height);
        let floor = min_edge.max(256).min(edge);
        let floor_fps = (min_fps.max(15) * 1000).min(ceiling.fps_millihz);
        let mut sizes = vec![(width, height)];
        let mut next = edge;
        while next > floor {
            next = floor.max(next * 4 / 5);
            let size = ((width * next).div_ceil(edge * 2) * 2, (height * next).div_ceil(edge * 2) * 2);
            if sizes.last() != Some(&size) { sizes.push(size); }
        }
        let mut rates = vec![ceiling.fps_millihz];
        for rate in [120_000, 90_000, 60_000, 45_000, 30_000] {
            if rate < ceiling.fps_millihz && rate > floor_fps { rates.push(rate); }
        }
        if rates.last() != Some(&floor_fps) { rates.push(floor_fps); }
        let mut levels = vec![ceiling];
        let (mut size, mut rate) = (0, 0);
        while size + 1 < sizes.len() || rate + 1 < rates.len() {
            let lower_size = match mode {
                QualityMode::Motion => size + 1 < sizes.len(),
                QualityMode::Detail => rate + 1 >= rates.len(),
                _ => size + 1 < sizes.len() && (rate + 1 >= rates.len() || levels.len() % 2 == 0),
            };
            if lower_size { size += 1; } else { rate += 1; }
            levels.push(target(sizes[size].0, sizes[size].1, rates[rate]).expect("bounded ladder"));
        }
        let battery_ceiling = if mode == QualityMode::Battery {
            levels.iter().position(|t| t.fps_millihz <= floor_fps.max(60_000)
                && u32::from(t.width.min(t.height)) <= floor.max(1080)).unwrap_or(levels.len() - 1)
        } else { 0 };
        Self { levels, level: 0, battery_ceiling, last_change: now, healthy_since: None,
            last_sample: None, device_pressure: 0, network_pressure: 0, tail_pressure: 0 }
    }

    pub fn current(&self) -> Reconfigure { self.levels[self.level] }

    pub fn rebase(&mut self, mode: QualityMode, ceiling: Reconfigure, min_edge: u32, min_fps: u32, now: u64) -> Reconfigure {
        let level = self.level;
        *self = Self::new(mode, ceiling, min_edge, min_fps, now);
        self.level = level.min(self.levels.len() - 1);
        self.current()
    }

    pub fn sample(&mut self, s: Sample) -> Option<Reconfigure> {
        if !s.recent { self.suspend(); return None; }
        if self.last_sample.is_some_and(|last| s.now.saturating_sub(last) > 2000 || s.now <= last) { self.suspend(); }
        self.last_sample = Some(s.now);
        let frame_ms = 1_000_000.0 / f64::from(self.current().fps_millihz);
        let valid_tail = s.samples >= 12 && (500_000..=2_000_000).contains(&s.span_us);
        let tail = valid_tail && s.late >= 3 && u64::from(s.late) * 10 >= u64::from(s.samples);
        let device = s.decode_ms >= frame_ms * 2.0;
        let network = s.poor || s.loss >= 3.0 || s.queue_ms >= 20.0f64.max(frame_ms * 2.0);
        self.device_pressure = if device { self.device_pressure + 1 } else { 0 };
        self.network_pressure = if network && !s.bitrate_has_room { self.network_pressure + 1 } else { 0 };
        self.tail_pressure = if tail { self.tail_pressure + 1 } else { 0 };
        if device || network || tail || self.level < self.battery_ceiling {
            self.healthy_since = None;
            if s.now.saturating_sub(self.last_change) >= 15_000 && self.level + 1 < self.levels.len()
                && (self.device_pressure >= 4 || self.tail_pressure >= 4 || self.network_pressure >= 8 || self.level < self.battery_ceiling) {
                return Some(self.levels[self.level + 1]);
            }
        } else {
            let healthy = valid_tail && u64::from(s.late) * 50 <= u64::from(s.samples)
                && (0.0..0.5).contains(&s.loss) && (0.0..frame_ms * 1.5).contains(&s.decode_ms)
                && (0.0..5.0f64.max(frame_ms * 0.25)).contains(&s.jitter_ms)
                && (0.0..4.0f64.max(frame_ms / 2.0)).contains(&s.queue_ms);
            if !healthy { self.healthy_since = None; }
            else if let Some(since) = self.healthy_since {
                if self.level > self.battery_ceiling && s.now.saturating_sub(since) >= 30_000
                    && s.now.saturating_sub(self.last_change) >= 30_000 { return Some(self.levels[self.level - 1]); }
            } else { self.healthy_since = Some(s.now); }
        }
        None
    }

    pub fn applied(&mut self, request: Reconfigure, now: u64) {
        self.level = self.levels.iter().position(|r| *r == request).expect("target from this ladder");
        self.last_change = now;
        self.suspend();
    }

    pub fn suspend(&mut self) {
        self.healthy_since = None;
        self.last_sample = None;
        self.device_pressure = 0;
        self.network_pressure = 0;
        self.tail_pressure = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(now: u64) -> Sample {
        Sample { now, recent: true, poor: false, loss: 0.0, decode_ms: 2.0, jitter_ms: 1.0,
            queue_ms: 1.0, bitrate_has_room: false, samples: 60, late: 0, span_us: 980_000 }
    }

    #[test]
    fn ladders_keep_floors_aspect_and_priority() {
        let top = target(1920, 1080, 144_000).unwrap();
        for mode in [QualityMode::Motion, QualityMode::Detail, QualityMode::Battery] {
            let q = Quality::new(mode, top, 720, 30, 0);
            for t in &q.levels {
                assert!(t.width <= 1920 && (720..=1080).contains(&t.height));
                assert!((30_000..=144_000).contains(&t.fps_millihz));
                assert!((f64::from(t.width) / f64::from(t.height) - 16.0 / 9.0).abs() < 0.005);
                assert_eq!(Reconfigure::decode(&t.encode()), Some(*t));
            }
            if mode == QualityMode::Motion { assert_eq!(q.levels[1].fps_millihz, 144_000); }
            if mode == QualityMode::Detail { assert_eq!(q.levels[1].width, 1920); }
        }
        assert!(target(100_000, 1080, 60_000).is_none());
        assert!(target(1921, 1080, 60_000).is_none());
    }

    #[test]
    fn display_changes_preserve_adaptive_reduction() {
        let mut quality = Quality::new(QualityMode::Motion, target(1920, 1080, 60_000).unwrap(), 720, 30, 0);
        quality.applied(quality.levels[1], 15_000);
        let mode = quality.rebase(QualityMode::Motion, target(2560, 1440, 120_000).unwrap(), 720, 30, 20_000);
        assert_eq!(mode, target(2048, 1152, 120_000).unwrap());
        assert_eq!(quality.level, 1);
    }

    #[test]
    fn sustained_pressure_and_fresh_recovery_only() {
        let top = target(1920, 1080, 60_000).unwrap();
        let mut q = Quality::new(QualityMode::Motion, top, 720, 30, 0);
        for now in 12..15 { let mut s = sample(now * 1000); s.decode_ms = 40.0; assert_eq!(q.sample(s), None); }
        let mut s = sample(15_000); s.decode_ms = 40.0;
        let lower = q.sample(s).unwrap();
        assert_eq!(q.current(), top);
        q.applied(lower, 15_000);
        for now in 16..46 { assert_eq!(q.sample(sample(now * 1000)), None); }
        assert_eq!(q.sample(sample(46_000)), Some(top));
        q.suspend();
        let mut stale = sample(80_000); stale.span_us = 0;
        assert_eq!(q.sample(stale), None);
        assert_eq!(q.current(), lower);
    }

    #[test]
    fn bitrate_gets_first_chance_at_network_pressure() {
        let mut q = Quality::new(QualityMode::Detail, target(1920, 1080, 60_000).unwrap(), 720, 30, 0);
        for now in 15..25 { let mut s = sample(now * 1000); s.loss = 8.0; s.bitrate_has_room = true; assert_eq!(q.sample(s), None); }
        for now in 25..32 { let mut s = sample(now * 1000); s.loss = 8.0; assert_eq!(q.sample(s), None); }
        let mut s = sample(32_000); s.loss = 8.0;
        assert!(q.sample(s).is_some());
    }
}
