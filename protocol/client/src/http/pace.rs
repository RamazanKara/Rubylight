use super::Error;
use std::collections::BTreeMap;

pub const DURATION_MS: u32 = 2000;
pub const PACKET_SIZE: u32 = 1392;
const STEP: u32 = 50_000;

#[derive(Clone, Debug, Default)]
pub struct Measurement {
    pub expected: u32,
    pub sent: u32,
    pub received: u32,
    pub sender_ms: f64,
    pub loss: f64,
    pub worst_window_loss: f64,
    pub frames: u32,
    pub damaged_frames: u32,
    pub excessive_loss_frames: u32,
    pub missing_tail_frames: u32,
    pub worst_frame_loss: f64,
}

impl Measurement {
    pub fn measured(&self) -> bool {
        self.expected > 0 && self.sent <= self.expected && self.sent as f64 >= self.expected as f64 * 0.98
            && self.received > 0 && self.received <= self.sent && (1960.0..=2040.0).contains(&self.sender_ms)
            && self.loss.is_finite() && self.worst_window_loss.is_finite()
            && self.frames > 0 && self.worst_frame_loss.is_finite()
    }

    pub fn tail_loss(&self) -> f64 { percent(self.missing_tail_frames, self.frames) }

    pub fn qualified(&self) -> bool {
        self.measured() && self.loss < 2.0 && self.worst_window_loss < 5.0
            && self.excessive_loss_frames == 0 && self.worst_frame_loss <= 10.0 && self.tail_loss() < 2.0
    }

    fn ranking(&self) -> [f64; 6] {
        let violations = u32::from(self.loss >= 2.0) + u32::from(self.worst_window_loss >= 5.0)
            + u32::from(self.excessive_loss_frames != 0 || self.worst_frame_loss > 10.0) + u32::from(self.tail_loss() >= 2.0);
        [violations as f64, self.tail_loss(), percent(self.excessive_loss_frames, self.frames),
            self.worst_frame_loss, self.loss, self.worst_window_loss]
    }
}

fn percent(missing: u32, total: u32) -> f64 {
    if total == 0 { 100.0 } else { 100.0 * missing as f64 / total as f64 }
}

pub fn expected_packets(kbps: u32, packet_size: u32) -> u32 {
    (kbps as u64 * DURATION_MS as u64 / (8 * (packet_size as u64 + 134))) as u32
}

pub fn burst_group(pace_kbps: u32, packet_size: u32, ipv6: bool) -> u32 {
    (pace_kbps / 8 / (packet_size + 48 + 8 + if ipv6 { 40 } else { 20 } + 38)).max(1)
}

pub fn summarize(result: &mut Measurement, arrived: &[bool], fps: u32, group: u32) {
    result.frames = 0;
    result.damaged_frames = 0;
    result.excessive_loss_frames = 0;
    result.missing_tail_frames = 0;
    result.worst_frame_loss = 100.0;
    let expected = result.expected as usize;
    let count = fps as usize * DURATION_MS as usize / 1000;
    if expected > 0 && count > 0 {
        result.worst_frame_loss = 0.0;
        for frame in 0..count {
            let begin = expected * frame / count;
            let end = expected * (frame + 1) / count;
            if begin == end { continue; }
            let group = group.max(1) as usize;
            let tail = begin + (end - begin - 1) / group * group;
            let missing = (begin..end).filter(|&seq| !arrived.get(seq).copied().unwrap_or(false)).count() as u32;
            let loss = percent(missing, (end - begin) as u32);
            result.frames += 1;
            result.damaged_frames += u32::from(missing != 0);
            result.excessive_loss_frames += u32::from(loss > 10.0);
            result.missing_tail_frames += u32::from((tail..end).any(|seq| !arrived.get(seq).copied().unwrap_or(false)));
            result.worst_frame_loss = result.worst_frame_loss.max(loss);
        }
    }
    result.received = 0;
    result.worst_window_loss = 0.0;
    for window in 0..20 {
        let begin = expected * window / 20;
        let end = expected * (window + 1) / 20;
        let count = (begin..end).filter(|&seq| arrived.get(seq).copied().unwrap_or(false)).count() as u32;
        result.received += count;
        if end > begin { result.worst_window_loss = result.worst_window_loss.max(percent((end - begin) as u32 - count, (end - begin) as u32)); }
    }
    result.loss = percent(result.expected - result.received, result.expected);
}

pub fn probe_kbps(image_kbps: u32, fps: u32, packet_size: u32, fec: u32, min_parity: u32) -> u32 {
    if fps == 0 || image_kbps == 0 || packet_size <= 16 { return 5000; }
    let parity = if fec == 0 { 0 } else {
        (1..=255u64).map(|critical| (critical, (critical * fec as u64).div_ceil(100).max(min_parity as u64)))
            .filter(|(critical, parity)| critical + parity <= 255).map(|(_, parity)| parity).max().unwrap_or(0)
    };
    let image_bytes = (image_kbps as u64 * 125).div_ceil(fps as u64);
    let data = (image_bytes + 8).div_ceil(packet_size as u64 - 16);
    (((data + parity) * (packet_size as u64 + 134) * fps as u64).div_ceil(125) + 3572).clamp(5000, 3_000_000) as u32
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub pace_kbps: u32,
    pub qualified: bool,
    pub measurement: Measurement,
}

fn measure(probe: &mut impl FnMut(u32) -> Result<Measurement, Error>, measurements: &mut BTreeMap<u32, Measurement>, pace: u32,
    cancelled: &impl Fn() -> bool) -> Result<Measurement, Error> {
    if cancelled() { return Err(Error::Cancelled); }
    let result = probe(pace)?;
    if measurements.get(&pace).map_or(true, |old| !result.measured() || (old.measured() && old.ranking() < result.ranking())) {
        measurements.insert(pace, result.clone());
    }
    Ok(result)
}

fn confirmed(probe: &mut impl FnMut(u32) -> Result<Measurement, Error>, measurements: &mut BTreeMap<u32, Measurement>, pace: u32,
    cancelled: &impl Fn() -> bool) -> Result<Option<Outcome>, Error> {
    let first = measure(probe, measurements, pace, cancelled)?;
    if !first.qualified() { return Ok(None); }
    let second = measure(probe, measurements, pace, cancelled)?;
    Ok(second.qualified().then_some(Outcome { pace_kbps: pace, qualified: true, measurement: second }))
}

pub fn search(link_kbps: u32, floor_kbps: u32, mut probe: impl FnMut(u32) -> Result<Measurement, Error>,
    cancelled: impl Fn() -> bool) -> Result<Option<Outcome>, Error> {
    let step_down = |n: u32| n / STEP * STEP;
    let cap = step_down(link_kbps.min(9_999_999));
    if cap < STEP { return Ok(None); }
    let floor = (floor_kbps.min(cap).div_ceil(STEP) * STEP).clamp(STEP, cap);
    let mut measurements = BTreeMap::new();
    if measure(&mut probe, &mut measurements, cap, &cancelled)?.qualified() {
        if let Some(outcome) = confirmed(&mut probe, &mut measurements, cap, &cancelled)? { return Ok(Some(outcome)); }
    }
    let (mut low, mut high) = (0, cap);
    if floor < cap {
        if measure(&mut probe, &mut measurements, floor, &cancelled)?.qualified() { low = floor; }
        for _ in 0..16 {
            if low == 0 || high - low <= STEP { break; }
            let next = (low + STEP).max(step_down((low + high) / 2));
            if next >= high { break; }
            if measure(&mut probe, &mut measurements, next, &cancelled)?.qualified() { low = next; } else { high = next; }
        }
    }
    if low != 0 {
        let mut pace = floor.max(step_down(low * 95 / 100));
        loop {
            if let Some(outcome) = confirmed(&mut probe, &mut measurements, pace, &cancelled)? { return Ok(Some(outcome)); }
            if pace == floor { break; }
            pace = floor.max(step_down(pace * 90 / 100));
        }
    }
    // Loss need not be monotonic: a failed floor does not rule out faster paces.
    for pace in (floor..cap).step_by(STEP as usize).rev() {
        let passes = match measurements.get(&pace) {
            Some(previous) => previous.qualified(),
            None => measure(&mut probe, &mut measurements, pace, &cancelled)?.qualified(),
        };
        if passes {
            if let Some(outcome) = confirmed(&mut probe, &mut measurements, pace, &cancelled)? { return Ok(Some(outcome)); }
        }
    }
    let best = measurements.iter().filter(|(_, result)| result.measured()).min_by(|(pace_a, a), (pace_b, b)| {
        a.ranking().partial_cmp(&b.ranking()).unwrap().then_with(|| pace_b.cmp(pace_a))
    }).map(|(&pace, _)| pace);
    let Some(pace) = best else { return Ok(None); };
    let first = measure(&mut probe, &mut measurements, pace, &cancelled)?;
    let second = measure(&mut probe, &mut measurements, pace, &cancelled)?;
    if !first.measured() || !second.measured() { return Ok(None); }
    if cancelled() { return Err(Error::Cancelled); }
    Ok(Some(Outcome { pace_kbps: pace, qualified: first.qualified() && second.qualified(),
        measurement: if first.ranking() > second.ranking() { first } else { second } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(tail_loss: bool) -> Measurement {
        let mut arrived = vec![true; 24_000];
        if tail_loss { for i in 0..24 { arrived[i * 1000 + 99] = false; } }
        let mut result = Measurement { expected: 24_000, sent: 24_000, sender_ms: 2000.0, ..Default::default() };
        summarize(&mut result, &arrived, 120, 10);
        result
    }

    #[test]
    fn frame_tails_and_sender_duration_match_android() {
        let mut result = probe(false);
        assert_eq!(result.frames, 240);
        assert!(result.qualified());
        result.sender_ms = 1500.0;
        assert!(!result.measured());
        let result = probe(true);
        assert_eq!(result.missing_tail_frames, 24);
        assert!(result.loss < 2.0);
        assert!(!result.qualified());
        assert_eq!(expected_packets(1_000_000, 1392), 163826);
        assert_eq!(burst_group(1_000_000, 1392, false), 1_000_000 / 8 / (1440 + 8 + 20 + 38));
        assert!((1_000_000..1_300_000).contains(&probe_kbps(1_000_000, 120, 1392, 20, 2)));
        assert_eq!(probe_kbps(5_000_000, 60, 1392, 20, 2), 3_000_000);
    }

    #[test]
    fn search_confirms_twice_and_keeps_the_same_margin_and_fallback() {
        let mut tested = Vec::new();
        let outcome = search(2_500_000, 300_000, |pace| { tested.push(pace); Ok(probe(false)) }, || false).unwrap().unwrap();
        assert!(outcome.qualified);
        assert_eq!(tested, [2_500_000; 3]);
        let outcome = search(2_500_000, 300_000, |pace| Ok(probe(pace > 2_200_000)), || false).unwrap().unwrap();
        assert_eq!(outcome.pace_kbps, 2_050_000);
        assert!(outcome.qualified);
        let outcome = search(500_000, 100_000, |_| Ok(probe(true)), || false).unwrap().unwrap();
        assert_eq!(outcome.pace_kbps, 500_000);
        assert!(!outcome.qualified);
        assert!(matches!(search(500_000, 100_000, |_| panic!("cancelled probe ran"), || true), Err(Error::Cancelled)));
    }
}
