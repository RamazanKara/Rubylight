//! Desktop port of Android's AdaptiveBitrateController. Time is session-relative milliseconds.

pub struct Bitrate {
    ceiling: u32,
    current: u32,
    floor: u32,
    frame_ms: f64,
    last_change: u64,
    healthy_since: Option<u64>,
    poor_samples: u32,
    decode_before_reduction: Option<(f64, u32)>,
    decode_insensitive: bool,
    previous_network: Option<(f64, f64)>,
}

impl Bitrate {
    pub fn new(ceiling: u32, current: u32, fps: i32, now: u64) -> Self {
        Self {
            ceiling,
            current,
            floor: current.min(2000),
            frame_ms: 1000.0 / f64::from(fps),
            last_change: now,
            healthy_since: None,
            poor_samples: 0,
            decode_before_reduction: None,
            decode_insensitive: false,
            previous_network: None,
        }
    }

    pub fn pyrowave_sample(&mut self, now: u64, recent: bool, poor: bool, measurements: [f64; 4]) -> Option<u32> {
        let [loss, queue_ms, decode_ms, jitter_ms] = measurements;
        if !recent {
            self.healthy_since = None;
            self.previous_network = None;
            if !poor { self.poor_samples = 0; return None; }
        }
        let rising = self.previous_network.is_some_and(|(previous_loss, previous_queue)| {
            (previous_loss >= 0.0 && loss >= 0.5 && loss - previous_loss >= 0.25)
                || (previous_queue >= 0.0 && queue_ms >= 8.0f64.max(self.frame_ms * 0.75) && queue_ms - previous_queue >= 3.0)
        });
        self.previous_network = Some((loss, queue_ms));
        let congested = poor || (recent && (loss >= 2.0 || decode_ms >= self.frame_ms
            || jitter_ms >= 5.0f64.max(self.frame_ms * 0.75) || queue_ms >= 20.0f64.max(self.frame_ms * 2.0) || rising));
        if congested {
            self.healthy_since = None;
            self.poor_samples += 1;
            if self.poor_samples >= 2 && now - self.last_change >= 3000 {
                let target = self.current.min(10_000).max(self.current * 85 / 100);
                return (target < self.current).then_some(target);
            }
        } else {
            self.poor_samples = 0;
            if !(0.0..0.1).contains(&loss) || !(0.0..4.0f64.max(self.frame_ms / 2.0)).contains(&queue_ms)
                || !(0.0..self.frame_ms * 0.75).contains(&decode_ms) || !(0.0..5.0f64.max(self.frame_ms * 0.25)).contains(&jitter_ms) {
                self.healthy_since = None;
            } else if let Some(since) = self.healthy_since {
                if now - since >= 20_000 && now - self.last_change >= 20_000 {
                    let target = self.ceiling.min(self.current + 250.max(self.current / 50));
                    return (target > self.current).then_some(target);
                }
            } else { self.healthy_since = Some(now); }
        }
        None
    }

    pub fn sample(
        &mut self,
        now: u64,
        recent: bool,
        poor: bool,
        loss: f64,
        decode_ms: f64,
        jitter_ms: f64,
    ) -> Option<u32> {
        if !recent && !poor {
            self.suspend();
            return None;
        }
        let poor =
            poor || (recent && (loss >= 3.0 || jitter_ms >= 5.0f64.max(self.frame_ms * 0.75)));
        if recent && decode_ms >= 0.0 && decode_ms < self.frame_ms * 0.75 {
            self.decode_insensitive = false;
            self.decode_before_reduction = None;
        }
        let mut slow = recent && decode_ms >= self.frame_ms && !self.decode_insensitive;
        if let Some((previous_ms, previous_rate)) = self.decode_before_reduction {
            if slow
                && !poor
                && self.current < previous_rate
                && now - self.last_change >= 15_000
                && decode_ms >= previous_ms * 0.95
            {
                self.decode_insensitive = true;
                slow = false;
            }
        }
        if poor || slow {
            self.healthy_since = None;
            self.poor_samples += 1;
            let floor = if poor {
                self.floor
            } else {
                self.floor.max(self.ceiling / 2)
            };
            if self.poor_samples >= 2 && now - self.last_change >= if poor { 3000 } else { 15_000 }
            {
                let target = floor.max(self.current * 80 / 100);
                if target < self.current {
                    if slow {
                        self.decode_before_reduction = Some((decode_ms, self.current));
                    }
                    return Some(target);
                }
            }
        } else {
            self.poor_samples = 0;
            if !(0.0..0.5).contains(&loss)
                || decode_ms < 0.0
                || (!self.decode_insensitive && decode_ms >= self.frame_ms * 0.75)
                || !(0.0..5.0f64.max(self.frame_ms * 0.25)).contains(&jitter_ms)
            {
                self.healthy_since = None;
            } else if let Some(since) = self.healthy_since {
                if now - since >= 15_000 && now - self.last_change >= 15_000 {
                    let target = self.ceiling.min(self.current + 250.max(self.current / 20));
                    return (target > self.current).then_some(target);
                }
            } else {
                self.healthy_since = Some(now);
            }
        }
        None
    }

    pub fn applied(&mut self, requested: u32, applied: u32, now: u64) {
        if applied < requested {
            self.ceiling = self.ceiling.min(applied);
        }
        self.current = applied;
        self.floor = self.floor.min(applied);
        self.last_change = now;
        self.suspend();
    }

    pub fn suspend(&mut self) {
        self.healthy_since = None;
        self.poor_samples = 0;
        self.previous_network = None;
    }

    pub fn has_room(&self) -> bool { self.current > self.floor }

    pub fn set_fps(&mut self, fps_millihz: u32) {
        self.frame_ms = 1_000_000.0 / f64::from(fps_millihz);
        self.decode_before_reduction = None;
        self.decode_insensitive = false;
        self.suspend();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn congestion_requires_sustained_pressure_and_recovery_is_slow() {
        let mut rate = Bitrate::new(20_000, 20_000, 60, 0);
        assert_eq!(rate.sample(1000, true, false, 8.0, 2.0, 1.0), None);
        assert_eq!(rate.sample(2000, true, false, 8.0, 2.0, 1.0), None);
        assert_eq!(rate.sample(3000, true, false, 8.0, 2.0, 1.0), Some(16_000));
        rate.applied(16_000, 16_000, 3000);
        assert_eq!(rate.sample(4000, true, false, 0.0, 2.0, 1.0), None);
        assert_eq!(rate.sample(18_000, true, false, 0.0, 2.0, 1.0), None);
        assert_eq!(
            rate.sample(19_000, true, false, 0.0, 2.0, 1.0),
            Some(16_800)
        );
        rate.suspend();
        assert_eq!(rate.sample(20_000, false, false, 0.0, 2.0, 1.0), None);
        assert_eq!(rate.sample(40_000, true, false, 0.0, 2.0, 1.0), None);
    }

    #[test]
    fn a_host_cap_is_respected_and_fixed_decoder_delay_does_not_keep_cutting() {
        let mut rate = Bitrate::new(20_000, 20_000, 60, 0);
        rate.sample(14_000, true, false, 0.0, 20.0, 1.0);
        assert_eq!(
            rate.sample(15_000, true, false, 0.0, 20.0, 1.0),
            Some(16_000)
        );
        rate.applied(16_000, 15_000, 15_000);
        assert_eq!(rate.sample(30_000, true, false, 0.0, 20.0, 1.0), None);
        assert_eq!(rate.sample(45_000, true, false, 0.0, 20.0, 1.0), None);
    }

    #[test]
    fn pyrowave_reacts_to_rising_loss_and_respects_its_floor() {
        let mut rate = Bitrate::new(40_000, 40_000, 60, 0);
        assert_eq!(rate.pyrowave_sample(1000, true, false, [0.0, 1.0, 2.0, 1.0]), None);
        assert_eq!(rate.pyrowave_sample(2000, true, false, [0.5, 1.0, 2.0, 1.0]), None);
        assert_eq!(rate.pyrowave_sample(3000, true, false, [0.8, 1.0, 2.0, 1.0]), Some(34_000));
        rate.applied(34_000, 34_000, 3000);
        assert_eq!(rate.pyrowave_sample(4000, true, false, [0.0, 1.0, 2.0, 1.0]), None);
        assert_eq!(rate.pyrowave_sample(24_000, true, false, [0.0, 1.0, 2.0, 1.0]), Some(34_680));
        rate.applied(10_000, 10_000, 25_000);
        rate.pyrowave_sample(26_000, true, true, [8.0, 50.0, 20.0, 20.0]);
        assert_eq!(rate.pyrowave_sample(30_000, true, true, [8.0, 50.0, 20.0, 20.0]), None);
    }
}
