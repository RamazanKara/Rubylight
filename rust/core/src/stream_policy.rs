//! Wire budget and encoder cadence used by the previous Moonlight host.
use crate::{config::Config, rtsp::Negotiated};
use std::time::{Duration, Instant};

/// A frame slot is consumed by a submission, not by checking an unchanged image.
pub struct Cadence {
    due: Instant,
    period: Duration,
    smooth: bool,
}
impl Cadence {
    pub fn new(now: Instant, period: Duration, smooth: bool) -> Self {
        Self {
            due: now,
            period,
            smooth,
        }
    }
    pub fn deadline(&self) -> Instant {
        self.due
    }
    pub fn submitted(&mut self, now: Instant) {
        self.submitted_after(now, self.period);
    }
    /// Like [`Self::submitted`], with the next slot `interval` away instead of
    /// one stream period: phase lock follows the client's display this way.
    pub fn submitted_after(&mut self, now: Instant, interval: Duration) {
        let anchored = self.due + interval;
        // After a static interval, start a new cadence without a catch-up burst.
        self.due = if self.smooth && anchored > now {
            anchored
        } else {
            now + interval
        };
    }
}
/// How the encoder chooses when to claim a captured frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// Claim each new frame as it arrives, at most at the stream rate. A frame
    /// never waits for a slot that is unrelated to when Windows presented it.
    Arrival,
    /// Claim the newest frame on a fixed grid at the stream rate.
    Grid,
}
impl Pacing {
    pub fn from_config(config: &Config) -> Self {
        match config
            .get("frame_pacing", "arrival")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "grid" | "fixed" => Self::Grid,
            "arrival" | "" => Self::Arrival,
            other => {
                crate::config::fallback("frame_pacing", other, "arrival");
                Self::Arrival
            }
        }
    }
}
/// What the encoder should do with the newest unclaimed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    Claim,
    /// Wait for a fresher frame or this deadline, whichever comes first.
    WaitUntil(Instant),
}
/// Rolling phase of a stable source close to the requested stream rate, or
/// close to a whole multiple of it.
/// Capture can publish extra compositions between real source updates. Those
/// extra timestamps must not become the anchor for predicting the next update.
struct SourcePhase {
    /// The stream period the phase was learned for.
    period_ns: u64,
    /// The source's updates per stream period: phases fold over
    /// `period_ns / multiple` (`multiple_from`).
    multiple: u64,
    /// The most updates per stream period the picture interval allows
    /// (`limit_to`), and when it last allowed as many.
    limit: u64,
    limit_at: Option<Instant>,
    origin: Option<Instant>,
    last: Option<Instant>,
    samples: [u64; 64],
    elapsed_ns: [u64; 64],
    count: usize,
    next: usize,
    center: Option<u64>,
    last_phase: Option<Instant>,
}
impl SourcePhase {
    fn new(period: Duration) -> Self {
        Self {
            period_ns: period.as_nanos().clamp(1, u128::from(u64::MAX)) as u64,
            multiple: 1,
            limit: 1,
            limit_at: None,
            origin: None,
            last: None,
            samples: [0; 64],
            elapsed_ns: [0; 64],
            count: 0,
            next: 0,
            center: None,
            last_phase: None,
        }
    }
    /// The interval phases fold over: one source update.
    fn cycle_ns(&self) -> u64 {
        (self.period_ns / self.multiple).max(1)
    }
    /// Fold the samples over one update of a source updating `multiple`
    /// times a stream period.
    fn fold(&mut self, multiple: u64) {
        self.multiple = multiple;
        let cycle = self.cycle_ns();
        for index in 0..self.count {
            self.samples[index] = self.elapsed_ns[index] % cycle;
        }
    }
    /// A source updating 2 to 4 times a stream period (a 120 Hz game at
    /// 60 fps) shows one phase per update; folded over the stream period
    /// they are as many peaks, none with enough support, and the phase was
    /// never learned. Another window's updates then started pictures of
    /// their own: claimed once allowed, each took the game's previous frame
    /// a few milliseconds before its next, and the claims after it waited
    /// for the credit it spent. The largest multiple whose peak has an update
    /// in 80 % of its cycles is the source's, counted for every update of the
    /// period on its own (`cycle_share`): another window's updates on a
    /// 240 Hz display's other refreshes, 50 to 70 a second, filled 80 % of a
    /// 120 Hz game's quarter-period cycles together, but only the game's own
    /// two updates of the period fill them all. The multiple comes from the
    /// captures, with the picture interval only as the limit: other windows'
    /// updates and late second compositions split pictures, and the
    /// interval's median flickered between 2, 3 and 1 for a 120 Hz game.
    /// Tries the multiples from the limit down to `least`; leaves the samples
    /// folded over the last one tried.
    fn multiple_from(&mut self, least: u64) -> Option<u64> {
        for multiple in (least.max(2)..=self.limit.min(4)).rev() {
            self.fold(multiple);
            let (center, _) = self.peak();
            let (matched, complete) = self.cycle_share(center);
            if complete > 0 && matched * 100 >= complete * 80 {
                return Some(center);
            }
        }
        None
    }
    /// A lower limit takes over once the higher one has not shown for this
    /// long: the picture interval's median flickers.
    const LIMIT_HOLD: Duration = Duration::from_millis(250);
    /// The whole source updates per period the picture interval allows at
    /// `now`. With focus labels the picture interval is the focus window's:
    /// another window at 240 Hz beside a 60 Hz game must not become the
    /// phase, and a multiple above the limit is learned again.
    fn limit_to(&mut self, limit: u64, now: Instant) {
        if limit >= self.limit
            || self
                .limit_at
                .is_none_or(|at| now.saturating_duration_since(at) >= Self::LIMIT_HOLD)
        {
            self.limit = limit;
            self.limit_at = Some(now);
        }
    }
    /// The center of the densest radius-wide window of samples, and how many
    /// samples it holds.
    fn peak(&self) -> (u64, usize) {
        let cycle = self.cycle_ns();
        let mut bins = [0usize; 64];
        for &sample in &self.samples[..self.count] {
            bins[(u128::from(sample) * 64 / u128::from(cycle)) as usize] += 1;
        }
        // The bins within the radius on either side: 4 of 64 at a multiple
        // of 1.
        let half = (u128::from(self.radius_ns()) * 64 / u128::from(cycle)) as usize;
        let population = |bin: usize| {
            (0..=2 * half)
                .map(|offset| bins[(bin + offset + 64 - half) % 64])
                .sum::<usize>()
        };
        let peak = (0..64).max_by_key(|&bin| population(bin)).unwrap();
        (
            ((peak as u128 * 2 + 1) * u128::from(cycle) / 128) as u64,
            population(peak),
        )
    }
    fn offset(&self, at: Instant) -> u64 {
        (at.saturating_duration_since(self.origin.unwrap_or(at))
            .as_nanos()
            % u128::from(self.cycle_ns())) as u64
    }
    fn distance(&self, a: u64, b: u64) -> u64 {
        let delta = a.abs_diff(b);
        delta.min(self.cycle_ns() - delta)
    }
    /// How far from the center an update still counts as in phase: 9/128 of
    /// the stream period, whatever the multiple (a game's frame times and a
    /// composition's second capture spread its updates as much), at most a
    /// quarter of an update.
    fn radius_ns(&self) -> u64 {
        ((self.period_ns / 128) * 9).min(self.cycle_ns() / 4)
    }
    fn has_surplus(&self) -> bool {
        if self.count < 32 {
            return false;
        }
        let oldest = if self.count == self.samples.len() {
            self.next
        } else {
            0
        };
        let newest = (self.next + self.samples.len() - 1) % self.samples.len();
        let span = self.elapsed_ns[newest].saturating_sub(self.elapsed_ns[oldest]);
        // Phase alignment only addresses surplus compositions. Near 1:1 and
        // slower sources keep normal pacing, even if their phase is stable.
        // Use observed timestamp intervals (not sample count / elapsed time)
        // so a finite window does not overestimate a healthy source's rate.
        span > 0
            && (self.count as u128 - 1) * u128::from(self.period_ns) * 10 >= u128::from(span) * 11
    }
    fn covers_cycles(&self, center: u64) -> bool {
        let (matched, complete) = self.cycle_share(center);
        complete > 0 && matched * 100 >= complete * 65
    }
    /// The complete cycles within the samples, and how many of them have a
    /// sample near `center`: of the update of the stream period (the cycle's
    /// index modulo the multiple) with the fewest.
    fn cycle_share(&self, center: u64) -> (u128, u128) {
        let oldest = if self.count == self.samples.len() {
            self.next
        } else {
            0
        };
        let newest = (self.next + self.samples.len() - 1) % self.samples.len();
        let period = u128::from(self.cycle_ns());
        let cycle = |elapsed: u64| {
            (u128::from(elapsed) + period - u128::from(center) + period / 2) / period
        };
        let first = cycle(self.elapsed_ns[oldest]);
        let last = cycle(self.elapsed_ns[newest]);
        let complete = last.saturating_sub(first + 1);
        let updates = u128::from(self.multiple.clamp(1, 4));
        let mut matched = [0u128; 4];
        let mut previous = None;
        for offset in 0..self.count {
            let index = (oldest + offset) % self.samples.len();
            let cycle = cycle(self.elapsed_ns[index]);
            if cycle > first
                && cycle < last
                && previous != Some(cycle)
                && self.distance(self.samples[index], center) <= self.radius_ns()
            {
                matched[(cycle % updates) as usize] += 1;
                previous = Some(cycle);
            }
        }
        // The complete cycles first + 1 ..= last - 1 of each update.
        let start = (first + 1) % updates;
        (0..updates)
            .map(|update| {
                let extra = u128::from((update + updates - start) % updates < complete % updates);
                (matched[update as usize], complete / updates + extra)
            })
            .min_by(|a, b| (a.0 * b.1).cmp(&(b.0 * a.1)).then(a.1.cmp(&b.1)))
            .unwrap_or((0, 0))
    }
    fn observe(&mut self, at: Instant) {
        if self.last == Some(at) {
            return;
        }
        if self.last.is_some_and(|last| {
            at < last || at.duration_since(last) > Duration::from_nanos(self.period_ns) * 4
        }) {
            self.restart();
        }
        self.origin.get_or_insert(at);
        let mut phase = self.offset(at);
        if self
            .center
            .is_some_and(|center| self.distance(phase, center) <= self.radius_ns())
        {
            self.last_phase = Some(at);
        }
        // A phase jump or irregular source must not keep a stale prediction.
        // Extra compositions can outnumber source updates without a phase jump.
        if self
            .last_phase
            .is_some_and(|last| at.duration_since(last) >= Duration::from_nanos(self.period_ns) * 3)
        {
            self.restart();
            self.origin = Some(at);
            phase = 0;
        }
        self.last = Some(at);
        self.samples[self.next] = phase;
        self.elapsed_ns[self.next] = at
            .duration_since(self.origin.unwrap())
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        self.next = (self.next + 1) % self.samples.len();
        self.count = (self.count + 1).min(self.samples.len());
        let previous = self.center.take();
        self.last_phase = None;
        self.learn(previous);
    }
    /// Start learning from scratch, for the same stream period and multiple.
    fn restart(&mut self) {
        let multiple = self.multiple;
        *self = Self::new(Duration::from_nanos(self.period_ns));
        self.multiple = multiple;
    }
    /// Find the center from the samples, keeping `previous` while the source
    /// still updates near it.
    fn learn(&mut self, previous: Option<u64>) {
        if self.count < 32 {
            return;
        }
        // Retain a learned source while it still updates in most stream cycles;
        // counting all captures would let extra compositions dilute its support.
        // A larger multiple the limit allows replaces it.
        let retained =
            previous.filter(|&center| self.multiple <= self.limit && self.covers_cycles(center));
        let multiple = self.multiple;
        let least = if retained.is_some() { multiple + 1 } else { 2 };
        let center = match (self.multiple_from(least), retained) {
            (Some(center), _) => center,
            (None, Some(center)) => {
                self.fold(multiple);
                center
            }
            (None, None) => {
                // The phase over the whole period, as before.
                self.fold(1);
                let (center, population) = self.peak();
                if population * 100 < self.count * 65 {
                    return;
                }
                center
            }
        };
        let cycle = self.cycle_ns();
        let half = i128::from(cycle / 2);
        let period = i128::from(cycle);
        let radius = i128::from(self.radius_ns());
        let mut sum = 0i128;
        let mut count = 0i128;
        for &sample in &self.samples[..self.count] {
            let delta = (i128::from(sample) - i128::from(center) + half).rem_euclid(period) - half;
            if delta.abs() <= radius {
                sum += delta;
                count += 1;
            }
        }
        if count > 0 {
            let center = (i128::from(center) + sum / count).rem_euclid(period) as u64;
            self.center = Some(center);
            self.last_phase = self.samples[..self.count]
                .iter()
                .zip(&self.elapsed_ns[..self.count])
                .filter(|&(sample, _)| self.distance(*sample, center) <= self.radius_ns())
                .map(|(_, &elapsed)| self.origin.unwrap() + Duration::from_nanos(elapsed))
                .max();
        }
    }
}
/// Claims frames when they arrive, without exceeding the stream rate on average.
///
/// A source faster than the stream (the 2x virtual display, a 240 Hz monitor)
/// delivers frames the stream cannot use. Claiming the first one that may be
/// claimed would let it age until the claim; instead the pacer waits for the
/// next source frame when it is expected shortly after the claim becomes
/// allowed. A source at or below the stream rate is claimed on arrival.
pub struct Pacer {
    period: Duration,
    credit: f64,
    credit_at: Instant,
    last_claim: Option<Instant>,
    prediction: bool,
    /// The shortest source interval a prediction waits for.
    prediction_floor: Duration,
    source_phase: Option<SourcePhase>,
    /// The least time between two claims, in periods.
    spacing: f64,
    /// The source presents faster than the stream rate.
    faster: bool,
    /// The source's mean frame interval, when the capture knows it.
    source_rate: Option<Duration>,
    /// When the source's latest picture was first captured.
    picture: Option<Instant>,
    /// The picture interval nearly all recent pictures arrived within.
    late_picture: Option<Duration>,
    /// The newest capture only extends the focus window's latest picture.
    extends: bool,
    /// The first capture of the picture the latest claim took.
    claimed_picture: Option<Instant>,
    /// Time after which an unchanged picture is encoded again.
    static_period: Duration,
    /// Credit a faster source may be lent, in frames, to repay a deficit.
    grace: f64,
    grace_at: Instant,
}
impl Pacer {
    /// Credit above one frame absorbs arrival jitter of a source at the stream rate.
    const CREDIT_CAP: f64 = 1.5;
    pub fn new(now: Instant, period: Duration) -> Self {
        Self {
            period,
            credit: Self::CREDIT_CAP,
            credit_at: now,
            last_claim: None,
            prediction: true,
            prediction_floor: Duration::ZERO,
            source_phase: None,
            spacing: 0.75,
            faster: false,
            source_rate: None,
            picture: None,
            late_picture: None,
            extends: false,
            claimed_picture: None,
            static_period: Duration::from_millis(50),
            grace: Self::GRACE_CAP,
            grace_at: now,
        }
    }
    /// When an unchanged picture is encoded again (`Rates::static_period`):
    /// another window's update next to a focus picture already claimed waits
    /// as long, unless the focus window changes first.
    pub fn with_static_period(mut self, period: Duration) -> Self {
        self.static_period = period;
        self
    }
    /// Whether the newest capture only extends the focus window's latest
    /// picture (`Freshness::extends_picture`).
    pub fn observe_extension(&mut self, extends: bool) {
        self.extends = extends;
    }
    /// VRR: the display follows each frame, so a game's uneven frame times
    /// should reach it as they are. The average stays capped at the stream
    /// rate; 3/4 of a period between claims distorted that cadence by up to
    /// 3 ms, half a period by about 1 ms.
    /// Phase lock: claims follow the client's refresh period instead of the
    /// stream's own. The source phase keeps its period; the two differ by a
    /// few parts in ten thousand.
    pub fn set_period(&mut self, period: Duration) {
        self.period = period;
    }
    pub fn with_spacing(mut self, periods: f64) -> Self {
        self.spacing = periods;
        self
    }
    /// Diagnostic comparison: keep the same rate and burst limits, but claim
    /// at the earliest allowed slot instead of waiting for a predicted frame.
    /// Prediction remains enabled unless explicitly disabled.
    pub fn with_prediction(mut self, prediction: bool) -> Self {
        self.prediction = prediction;
        self
    }
    /// Predictive waits only for a source whose frames are at least this far
    /// apart. VRR runs the virtual display at 1000 Hz, whose frame interval is
    /// the display's own refresh, not content: a wait for the next one would
    /// hold every claim up to a quarter period for nothing. A game or desktop
    /// on that display (120 or 240 Hz) shows its own, longer interval.
    pub fn with_prediction_floor(mut self, floor: Duration) -> Self {
        self.prediction_floor = floor;
        self
    }
    /// Learn the dominant phase from observed capture
    /// timestamps. Apply it only when at least 32 recent observations show
    /// capture updates arriving at least 10% faster than the requested rate.
    /// The host enables this for WGC; ordinary arrival pacing is retained
    /// whenever the observed source does not meet those conditions.
    pub fn with_source_phase(mut self, enabled: bool) -> Self {
        self.source_phase = enabled.then(|| SourcePhase::new(self.period));
        self
    }
    /// Observe each newest image seen by the stream, including images replaced
    /// before a claim. Re-reading the same captured timestamp is deduplicated.
    pub fn observe_source(&mut self, presented: Instant) {
        if let Some(phase) = &mut self.source_phase {
            phase.observe(presented);
        }
    }
    /// Turn source-phase learning on or off for the capture that is open now;
    /// a change starts it from scratch, the same state keeps what was learned.
    pub fn set_source_phase(&mut self, enabled: bool) {
        if enabled != self.source_phase.is_some() {
            self.source_phase = enabled.then(|| SourcePhase::new(self.period));
        }
    }
    /// The source's mean frame interval over its recent frames. Whether it is
    /// faster than the stream follows its rate, not its median interval: a
    /// 240 Hz virtual display alternating between one and two refreshes per
    /// picture has an 8 ms median at about 160 frames a second. Judged by the
    /// median, it refilled credit 1% fast, so claims ran at 121 a second off
    /// the source's arrivals and each took a picture up to 4 ms old.
    pub fn observe_source_rate(&mut self, mean_interval: Option<Duration>) {
        self.source_rate = mean_interval;
    }
    /// When the source's latest picture was first captured, and the picture
    /// interval nearly all recent pictures arrived within
    /// (`Freshness::picture_start`, `Freshness::late_picture_interval`). A
    /// prediction counts from the picture's first capture: from a second
    /// composition of the same picture, 1 ms later, it expected the next frame
    /// 1 ms late. A claim waiting for the next picture keeps waiting until the
    /// late interval, at most a quarter period past the median: a 120 Hz
    /// game's frames under VRR came 7.3 to 9.3 ms apart, so a wait that gave up
    /// 1 ms after the median prediction still took 0.5-2 % of frames 8 ms old.
    pub fn observe_picture(&mut self, start: Option<Instant>, late: Option<Duration>) {
        self.picture = start;
        self.late_picture = late;
    }
    /// Capture recovery or replacement invalidates the learned source phase.
    pub fn reset_source_phase(&mut self) {
        if self.source_phase.is_some() {
            self.source_phase = Some(SourcePhase::new(self.period));
        }
    }
    /// Credit refills slightly faster than the stream rate. A source at exactly
    /// the stream rate refills what each claim spends, so a deficit left by a
    /// startup burst would otherwise delay every following claim for the rest
    /// of the session; with the margin it is repaid within about a second.
    /// A faster source always has a frame for the next slot, so it refills at
    /// exactly the stream rate: with the margin, a 240 Hz display streamed at
    /// 120 fps was claimed 121.2 times a second.
    const REFILL: f64 = 1.01;
    fn credit(&self, now: Instant) -> f64 {
        let refill = if self.faster { 1. } else { Self::REFILL };
        let earned = now.saturating_duration_since(self.credit_at).as_secs_f64() * refill
            / self.period.as_secs_f64();
        (self.credit + earned).min(Self::CREDIT_CAP)
    }
    /// A faster source refills at exactly the stream rate, so a deficit
    /// would last the whole session: every claim waited the same fraction of
    /// a frame for credit while a fresh frame was in hand (2-3 ms at 60 fps on
    /// the 2x virtual display). Such a frame may borrow the shortfall from a
    /// grace that refills at a tenth of a frame a second, a quarter frame at
    /// most, so the claim rate stays within 0.1 frames a second of the stream.
    const GRACE_CAP: f64 = 0.25;
    const GRACE_PER_SECOND: f64 = 0.1;
    fn lend_grace(&mut self, now: Instant) {
        let grace = (self.grace
            + now.saturating_duration_since(self.grace_at).as_secs_f64() * Self::GRACE_PER_SECOND)
            .min(Self::GRACE_CAP);
        self.grace_at = now;
        let spaced = self
            .last_claim
            .is_none_or(|claim| now >= claim + self.period.mul_f64(self.spacing));
        let credit = self.credit(now);
        let lent = if spaced {
            (Self::CLAIM_CREDIT - credit).clamp(0., grace)
        } else {
            0.
        };
        self.grace = grace - lent;
        if lent > 0. {
            self.credit = credit + lent;
            self.credit_at = now;
        }
    }
    /// A claim needs this much credit. A frame at the stream rate that arrives
    /// a little before the previous claim's slot (because that claim was itself
    /// late) is claimed at once; waiting would carry the lateness into every
    /// following frame. Each claim still costs a whole frame of credit.
    const CLAIM_CREDIT: f64 = 7. / 8.;
    /// How long after its latest predicted present a frame the claim waits
    /// for gets to be noticed, even when that runs past the prediction budget.
    /// Cut off at the budget, a frame due just before it was noticed just
    /// after: the claim waited the whole budget, then took the frame one
    /// source interval old, and each such claim set the next budget one period
    /// later, so the miss repeated. A missed prediction holds a frame at most
    /// this long past the budget, plus the late picture interval's lead over
    /// the median (`observe_picture`), at most a quarter period.
    const PREDICTED_NOTICE: Duration = Duration::from_millis(1);
    /// The earliest instant the next frame may be claimed: no two claims closer
    /// than three quarters of a period, and no more than the stream rate overall.
    pub fn allowed_at(&self, now: Instant) -> Instant {
        let spaced = self
            .last_claim
            .map_or(now, |claim| claim + self.period.mul_f64(self.spacing));
        let credit = self.credit(now);
        let funded = if credit >= Self::CLAIM_CREDIT {
            now
        } else {
            now + self.period.mul_f64(Self::CLAIM_CREDIT - credit)
        };
        spaced.max(funded)
    }
    /// Give an imminent fresh capture a bounded chance to replace an unchanged
    /// image before a minimum-rate repeat consumes its pacing credit. `due`
    /// must be the original repeat deadline, not the current polling time or a
    /// previously deferred deadline; the result is fixed for the same inputs.
    /// A stale prediction adds no wait. This only delays repeats, so it also
    /// protects fresh frames with Desktop Duplication and VRR arrival pacing.
    pub fn repeat_deadline(
        &self,
        due: Instant,
        presented: Instant,
        source_interval: Option<Duration>,
    ) -> Instant {
        let Some(interval) =
            source_interval.filter(|interval| *interval >= self.period.mul_f64(0.875))
        else {
            return due;
        };
        let next = presented + interval;
        let detection = self.period / 4;
        let latest = due + detection + Duration::from_micros(500);
        let detected = next + detection;
        if detected > due && next <= latest {
            detected.min(latest)
        } else {
            due
        }
    }
    /// Decide for the newest unclaimed frame, presented at `presented`, given
    /// the source's recent frame interval as seen by the capture worker.
    pub fn decide(
        &mut self,
        now: Instant,
        presented: Instant,
        source_interval: Option<Duration>,
    ) -> Pace {
        self.faster = self
            .source_rate
            .or(source_interval)
            .is_some_and(|i| i < self.period.mul_f64(0.95));
        if self.faster {
            self.lend_grace(now);
        }
        let allowed = self.allowed_at(now);
        // Another window's update next to a focus picture already claimed
        // carries no new focus frame: claimed once credit allowed, it showed
        // that frame twice and the next claim skipped one. It waits like an
        // unchanged picture unless the focus window changes first.
        if self.extends
            && self.claimed_picture.is_some()
            && self.picture.filter(|start| *start <= presented) == self.claimed_picture
        {
            let repeat = self
                .last_claim
                .map_or(allowed, |last| last + self.static_period)
                .max(allowed);
            return if now >= repeat {
                Pace::Claim
            } else {
                Pace::WaitUntil(repeat)
            };
        }
        if let Some(phase) = &mut self.source_phase {
            let limit = source_interval.map_or(1, |interval| {
                let interval = interval.as_nanos().max(1);
                ((u128::from(phase.period_ns) + interval / 2) / interval).clamp(1, 4) as u64
            });
            phase.limit_to(limit, now);
        }
        if let Some(phase) = &self.source_phase
            && let Some(center) = phase.center
            && phase.has_surplus()
        {
            let cycle = u128::from(phase.cycle_ns());
            let offset = phase.offset(presented);
            let mut advance = (u128::from(center) + cycle - u128::from(offset)) % cycle;
            // A capture near the learned phase belongs to that update, even
            // when jitter puts it just before the center. Its next update is
            // one cycle later; an old extra composition must not skip this wait.
            if advance <= u128::from(phase.radius_ns()) {
                advance += cycle;
            }
            let next = presented + Duration::from_nanos(advance as u64);
            // A compositor update can arrive half a stream period late.
            // Keep the fallback anchored to this capture, not the polling time.
            let detection = self.period / 2 + Duration::from_micros(500);
            // A source updating several times a period: an in-phase capture's
            // next update is at least a cycle less a radius away, and the
            // slack stays a radius short of that. Past it (a 240 Hz source at
            // 60 fps), a fresh in-phase capture waited for the next one, and
            // every claim ran a cycle late. Up to it, an update half a 120 Hz
            // game's cycle off its phase (another window's on the 240 Hz
            // display's other refresh) waits for the game's next frame, which
            // came 4.1 to 5.3 ms later: a quarter period was too short.
            let slack = if phase.multiple > 1 {
                Duration::from_nanos(phase.cycle_ns().saturating_sub(2 * phase.radius_ns()))
            } else {
                self.period / 4 + Duration::from_micros(500)
            };
            if next + detection > allowed && next <= allowed + slack {
                // A new picture can precede the learned phase. Waiting for
                // another composition must not stretch delivery to 1.5 periods.
                // An update due right at that bound still gets
                // `PREDICTED_NOTICE` to be noticed in: a 120 Hz game frame
                // that missed one 240 Hz refresh came at exactly 1.25 periods,
                // and a cut there took the frame before it, 12.5 ms old.
                let deadline = self.last_claim.map_or(next + detection, |last| {
                    (next + detection)
                        .min(last + self.period + self.period / 4 + Self::PREDICTED_NOTICE)
                });
                if deadline > allowed {
                    return Pace::WaitUntil(deadline);
                }
            }
        }
        if now >= allowed {
            return Pace::Claim;
        }
        // A long pause is a static desktop, not the source cadence.
        if self.prediction
            && let Some(interval) = source_interval.filter(|interval| {
                *interval >= self.prediction_floor && *interval <= self.period * 4
            })
        {
            // Count from the first capture of the picture `presented` belongs
            // to. A start after it is a newer capture this claim has not seen.
            let since = self
                .picture
                .filter(|start| {
                    *start <= presented
                        && (self.extends || presented.duration_since(*start) < interval / 2)
                })
                .unwrap_or(presented);
            let next = since + interval;
            let slack = self.period / 4 + Duration::from_micros(500);
            // `next` is when Windows will present the fresher frame; the
            // capture worker notices it up to a detection delay later, which
            // can reach a millisecond with a polled capture. A frame presented
            // just before the claim is allowed may still arrive after it.
            let detection = self.period / 4;
            // The next picture can also come as late as the late interval.
            let late = since
                + self.late_picture.map_or(interval, |late| {
                    late.clamp(interval, interval + self.period / 4)
                });
            if late + detection > allowed && next <= allowed + slack {
                // Detection shares the prediction budget: adding it after
                // the latest expected arrival held an available frame for
                // another quarter-period when that prediction was missed.
                // A frame due at the end of the budget still gets
                // `PREDICTED_NOTICE` to be noticed in.
                let noticed = late + Self::PREDICTED_NOTICE;
                return Pace::WaitUntil((late + detection).min((allowed + slack).max(noticed)));
            }
        }
        Pace::WaitUntil(allowed)
    }
    pub fn claimed(&mut self, now: Instant) {
        // The picture of the newest capture, which every claim encodes.
        self.claimed_picture = self.picture;
        self.credit = (self.credit(now) - 1.).max(-1.);
        self.credit_at = now;
        self.last_claim = Some(now);
    }
}
/// When arrival pacing encodes an unchanged picture again. A static repeat
/// is due at `repeat_due`. For a keyframe or reference invalidation the
/// client asked for (`recovery`), a moving source's next new picture carries
/// it: the picture is held for two stream periods or one and a half of the
/// source's own frame intervals after it was presented, whichever is longer,
/// which leaves room for uneven frame times and capture detection. A picture
/// already unchanged that long (a still or slow screen) is encoded again at
/// once. Encoding a moving picture again at once sent an extra keyframe and
/// delayed the game's next frame behind it.
pub fn reencode_at(
    recovery: bool,
    presented: Instant,
    period: Duration,
    source_interval: Option<Duration>,
    repeat_due: Instant,
) -> Instant {
    if recovery {
        let hold = (period * 2).max(source_interval.unwrap_or(period).mul_f64(1.5));
        (presented + hold).min(repeat_due)
    } else {
        repeat_due
    }
}
/// Whether an encode spends arrival pacing credit. New pictures and static
/// repeats do, so repeats stay within the stream rate. A picture encoded
/// again only for the client's recovery request does not: it would hold the
/// game's next frame back by up to a period, and leave every frame for about
/// a second after it late while the credit is repaid.
/// Whether claims wait for the capture's learned phase, for the backend that
/// opened. Only WGC's frames arrive on a composition beat the wait can align
/// with: Rubylight's display driver pushes every composed frame, and a wait for
/// the next one under GPU load held claims up to 1.25 periods. Capture `auto`
/// is planned as WGC before it opens, so the plan cannot decide this.
/// `frame_pacing_source_phase` set either way wins.
pub fn source_phase(config: &Config, backend: &str) -> bool {
    config.boolean("frame_pacing_source_phase", backend == "wgc")
}

pub fn counts_toward_rate(fresh: bool, recovery: bool) -> bool {
    fresh || !recovery
}
/// Packets of one frame the host itself failed to send, against what its FEC
/// can repair. On a host whose Wi-Fi drops out, the socket refuses packets;
/// once a block loses more than its parity, the client cannot decode the frame
/// or any frame after it until a keyframe. Moonlight asks for one only after
/// its next complete frame and a round trip; the host knows at once.
pub struct SendLoss {
    /// Data and parity shards of each FEC block, in send order.
    blocks: Vec<(usize, usize)>,
    /// Upper bound of lost packets in each block.
    lost: Vec<usize>,
    /// Packets handed to the socket so far, and how many it refused.
    offered: usize,
    refused: usize,
}
impl SendLoss {
    pub fn new(blocks: Vec<(usize, usize)>) -> Self {
        let lost = vec![0; blocks.len()];
        Self {
            blocks,
            lost,
            offered: 0,
            refused: 0,
        }
    }
    /// A frame whose blocks are not all known yet, as when it is sent while
    /// the encoder still produces it. Add each with [`Self::push_block`]
    /// before its packets are recorded; the result then matches `new` with
    /// the whole layout.
    pub fn streamed() -> Self {
        Self::new(Vec::new())
    }
    /// The next block of a streamed frame: its data and parity shards.
    pub fn push_block(&mut self, data: usize, parity: usize) {
        self.blocks.push((data, parity));
        self.lost.push(0);
    }
    /// Record a batch of the next `packets` packets, of which `dropped` were
    /// refused. Which ones is not known, so each block covered by the batch
    /// is charged as if they all fell in it. Returns whether the frame is
    /// beyond repair.
    pub fn sent(&mut self, packets: usize, dropped: usize) -> bool {
        let (first, last) = (self.offered, self.offered + packets);
        self.offered = last;
        self.refused += dropped.min(packets);
        if dropped > 0 {
            let mut start = 0;
            for ((data, parity), lost) in self.blocks.iter().zip(self.lost.iter_mut()) {
                let end = start + data + parity;
                let overlap = last.min(end).saturating_sub(first.max(start));
                *lost = (*lost + dropped.min(overlap)).min(data + parity);
                start = end;
            }
        }
        self.unrecoverable()
    }
    /// Whether the socket took any packet of this frame.
    pub fn reached_socket(&self) -> bool {
        self.refused < self.offered
    }
    pub fn unrecoverable(&self) -> bool {
        self.blocks
            .iter()
            .zip(&self.lost)
            .any(|((_, parity), lost)| lost > parity)
    }
}
/// Whether a frame lost on the host's own send makes the next frame a
/// keyframe. A frame refused from its first packet (the radio is out)
/// always does: a keyframe costs the link nothing while it is refused, and
/// the first frame through afterwards must be one. A frame lost after part
/// of it got in (a congested link) does too, unless a keyframe started this
/// way within the hold has not yet been sent whole: a keyframe every frame
/// would only add its size to the backlog. The client then asks for one
/// itself once a frame gets through, as Moonlight does to avoid congestion
/// collapse.
#[derive(Default)]
pub struct SendLossRecovery {
    /// When the last keyframe started this way was asked for, until one is
    /// sent whole.
    pending: Option<Instant>,
}
impl SendLossRecovery {
    pub fn lost(&mut self, now: Instant, period: Duration, reached_socket: bool) -> bool {
        let hold = (period * 4).max(Duration::from_millis(40));
        if reached_socket
            && self
                .pending
                .is_some_and(|at| now.saturating_duration_since(at) < hold)
        {
            return false;
        }
        self.pending = Some(now);
        true
    }
    /// A frame went out whole; a keyframe ends the hold.
    pub fn delivered(&mut self, keyframe: bool) {
        if keyframe {
            self.pending = None;
        }
    }
}
/// Test-only radio outage on the host's video socket, from
/// `BUTTERPOLLO_TEST_SEND_OUTAGE=every_ms:length_ms[:air]`: for `length` of
/// every `every`, starting one `every` into the stream, each video datagram is
/// refused as by a full socket. Loopback never loses packets, so this is how
/// a host A/B exercises recovery from a host-side Wi-Fi dropout. With `:air`
/// the datagrams are lost after the socket instead, as on a client's Wi-Fi:
/// the host counts them sent and only the client notices. With `:stall` there
/// is no schedule: a large frame meets a network adapter that stops draining
/// (see [`AdapterStall`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendOutage {
    every: Duration,
    length: Duration,
    air: bool,
    stall: bool,
}
impl SendOutage {
    pub fn parse(value: &str) -> Option<Self> {
        let mut fields = value.trim().split(':').map(str::trim);
        let every = Duration::from_millis(fields.next()?.parse().ok()?);
        let length = Duration::from_millis(fields.next()?.parse().ok()?);
        let (air, stall) = match fields.next() {
            None => (false, false),
            Some("air") => (true, false),
            Some("stall") => (false, true),
            Some(_) => return None,
        };
        (fields.next().is_none() && length > Duration::ZERO && every > length).then_some(Self {
            every,
            length,
            air,
            stall,
        })
    }
    /// Lost in the air: the host sent it and does not know it was lost.
    pub fn in_air(&self) -> bool {
        self.air
    }
    /// The adapter stall a large frame starts, for `:stall`.
    pub fn stall(&self) -> Option<AdapterStall> {
        self.stall.then_some(AdapterStall {
            every: self.every,
            length: self.length,
            started: None,
            left: 0,
        })
    }
    pub fn from_env() -> Option<Self> {
        let outage = Self::parse(&std::env::var("BUTTERPOLLO_TEST_SEND_OUTAGE").ok()?);
        if let Some(outage) = outage {
            tracing::warn!(
                every_ms = outage.every.as_millis(),
                length_ms = outage.length.as_millis(),
                air = outage.air,
                stall = outage.stall,
                "BUTTERPOLLO_TEST_SEND_OUTAGE drops video datagrams on a schedule; unset it outside tests"
            );
        }
        outage
    }
    /// Whether the schedule refuses datagrams now; never for `:stall`.
    pub fn active(&self, started: Instant, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(started);
        !self.stall
            && elapsed >= self.every
            && Duration::from_nanos((elapsed.as_nanos() % self.every.as_nanos()) as u64)
                < self.length
    }
}
/// The video socket's send buffer (`SO_SNDBUF`, `butterpollo_windows::net`).
pub const VIDEO_SOCKET_BUFFER: usize = 1 << 20;
/// Test-only adapter stall, from `BUTTERPOLLO_TEST_SEND_OUTAGE=every:length:stall`:
/// the #59 field event, a large frame meeting a network adapter that stopped
/// draining while a game started. At most once every `every`, a frame with more
/// than [`VIDEO_SOCKET_BUFFER`] on the wire starts a `length` stall. The socket
/// buffer takes the first [`VIDEO_SOCKET_BUFFER`] bytes sent in it; after that
/// the socket is full until the stall ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterStall {
    every: Duration,
    length: Duration,
    started: Option<Instant>,
    /// Bytes handed to the socket since the stall started.
    left: usize,
}
impl AdapterStall {
    /// A frame with `wire_bytes` on the wire so far is being sent: it starts
    /// a stall when it overflows the socket buffer and none started within
    /// `every`. Returns whether it did.
    pub fn frame(&mut self, wire_bytes: usize, now: Instant) -> bool {
        if wire_bytes <= VIDEO_SOCKET_BUFFER
            || self
                .started
                .is_some_and(|at| now.saturating_duration_since(at) < self.every)
        {
            return false;
        }
        self.started = Some(now);
        self.left = 0;
        true
    }
    /// A batch of `bytes` about to be sent at `now`: `None` when the socket
    /// takes it, or the stall's end while the socket is full.
    pub fn full_until(&mut self, bytes: usize, now: Instant) -> Option<Instant> {
        let end = self.started? + self.length;
        if now >= end {
            return None;
        }
        if self.left + bytes <= VIDEO_SOCKET_BUFFER {
            self.left += bytes;
            return None;
        }
        Some(end)
    }
}
/// Test-only Wi-Fi stalls on the host's video socket, from
/// `BUTTERPOLLO_TEST_SEND_JITTER=every_ms:max_ms[:seed]`: a stall starts
/// `every` times U(0.5, 1.5) after the one before (the first that far into
/// the stream) and lasts U(max/4, max), drawn from a generator seeded with
/// `seed` (1 unless given), so two runs stall alike. `max` may not exceed
/// `every`: longer stalls would outrun the gaps and hold the link for good.
/// While one lasts, video
/// datagrams wait in a [`JitterLine`] and go out in order as a burst when it
/// ends, as from an access point that held them. A wired link, or an access
/// point that isolates flows, never does that, so this is how a host A/B
/// gives a client a jittery link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendJitter {
    every: Duration,
    max: Duration,
    state: u64,
    /// The current or next stall, from the stream's start.
    from: Duration,
    until: Duration,
}
impl SendJitter {
    pub fn parse(value: &str) -> Option<Self> {
        let mut fields = value.trim().split(':').map(str::trim);
        let every = Duration::from_millis(fields.next()?.parse().ok()?);
        let max = Duration::from_millis(fields.next()?.parse().ok()?);
        let state = match fields.next() {
            None => 1,
            Some(seed) => seed.parse().ok()?,
        };
        if fields.next().is_some() || every.is_zero() || max.is_zero() || max > every {
            return None;
        }
        let mut jitter = Self {
            every,
            max,
            state,
            from: Duration::ZERO,
            until: Duration::ZERO,
        };
        jitter.advance();
        Some(jitter)
    }
    /// The schedule in `BUTTERPOLLO_TEST_SEND_JITTER`, if any. A value that
    /// does not parse is warned about, not silently ignored.
    pub fn from_env() -> Option<Self> {
        let value = std::env::var("BUTTERPOLLO_TEST_SEND_JITTER").ok()?;
        let jitter = Self::parse(&value);
        if jitter.is_none() {
            tracing::warn!(
                value,
                "BUTTERPOLLO_TEST_SEND_JITTER is not every_ms:max_ms[:seed] with 0 < max_ms <= every_ms; ignored"
            );
        }
        jitter
    }
    /// Warns that the knob is set: it stalls the paced video sender, or, with
    /// `ignored_by` naming another send path, nothing at all.
    pub fn warn(&self, ignored_by: Option<&str>) {
        match ignored_by {
            None => tracing::warn!(
                every_ms = self.every.as_millis(),
                max_ms = self.max.as_millis(),
                "BUTTERPOLLO_TEST_SEND_JITTER holds video datagrams on a schedule; unset it outside tests"
            ),
            Some(path) => tracing::warn!(
                path,
                "BUTTERPOLLO_TEST_SEND_JITTER only stalls the paced video sender; this stream sends another way and ignores it"
            ),
        }
    }
    /// The end of the stall `elapsed` into the stream, or `None` outside one.
    /// Times must not go backwards.
    pub fn stalled_until(&mut self, elapsed: Duration) -> Option<Duration> {
        while elapsed >= self.until {
            self.advance();
        }
        (elapsed >= self.from).then_some(self.until)
    }
    fn advance(&mut self) {
        let every = self.every.as_nanos() as u64;
        let gap = every / 2 + split_mix(&mut self.state) % (every + 1);
        let max = self.max.as_nanos() as u64;
        let length = max / 4 + split_mix(&mut self.state) % (max - max / 4 + 1);
        // Back to back when a stall outlasts the gap to the next.
        self.from = (self.from + Duration::from_nanos(gap)).max(self.until);
        self.until = self.from + Duration::from_nanos(length);
    }
}
/// [`SendJitter`] as the access point that holds what reaches it during a
/// stall: sends while one lasts wait here and all go out, in the order they
/// came, the moment it ends (back-to-back stalls hold them through both).
/// The sender counts them as sent, so its pace, backlog and claims carry on
/// through the stall as they would on the real link.
#[derive(Clone, Debug)]
pub struct JitterLine<T> {
    jitter: SendJitter,
    held: std::collections::VecDeque<T>,
    /// When the held sends go out: the end of the stall that holds them.
    release: Duration,
}
impl<T> JitterLine<T> {
    pub fn new(jitter: SendJitter) -> Self {
        Self {
            jitter,
            held: Default::default(),
            release: Duration::ZERO,
        }
    }
    /// Whether a send `elapsed` into the stream has to wait: a stall lasts,
    /// or sends held before it have not gone yet. Times must not go
    /// backwards.
    pub fn holds(&mut self, elapsed: Duration) -> bool {
        self.stalled(elapsed) || !self.held.is_empty()
    }
    /// Queue a send that [`Self::holds`] said has to wait.
    pub fn hold(&mut self, send: T) {
        self.held.push_back(send);
    }
    /// The held sends, in order, once no stall lasts `elapsed` into the
    /// stream; none while one does.
    pub fn release(&mut self, elapsed: Duration) -> std::collections::vec_deque::Drain<'_, T> {
        let over = self.held.is_empty() || !self.stalled(elapsed);
        let end = if over { self.held.len() } else { 0 };
        self.held.drain(..end)
    }
    /// When the held sends go out, while any wait.
    pub fn release_at(&self) -> Option<Duration> {
        (!self.held.is_empty()).then_some(self.release)
    }
    fn stalled(&mut self, elapsed: Duration) -> bool {
        let stalled = self.jitter.stalled_until(elapsed);
        if let Some(end) = stalled {
            self.release = end;
        }
        stalled.is_some()
    }
}
/// SplitMix64, the test knobs' seeded generator.
fn split_mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
/// Test-only random loss on the host's video socket, from
/// `BUTTERPOLLO_TEST_SEND_LOSS=percent:seed[:air]`: each video datagram is
/// lost with that probability (in percent, fractions allowed), decided by a
/// generator seeded with `seed`, so two runs lose the same datagrams in the
/// same order. Like [`SendOutage`], lost datagrams are refused as by a full
/// socket, or with `:air` counted sent and lost after the socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendDrop {
    /// Loss probability in millionths.
    millionths: u64,
    state: u64,
    air: bool,
}
impl SendDrop {
    pub fn parse(value: &str) -> Option<Self> {
        let mut fields = value.trim().split(':').map(str::trim);
        let percent: f64 = fields.next()?.parse().ok()?;
        let seed: u64 = fields.next()?.parse().ok()?;
        let air = match fields.next() {
            None => false,
            Some("air") => true,
            Some(_) => return None,
        };
        (fields.next().is_none() && percent > 0. && percent <= 100.).then_some(Self {
            millionths: (percent * 10_000.).round() as u64,
            state: seed,
            air,
        })
    }
    pub fn from_env() -> Option<Self> {
        let loss = Self::parse(&std::env::var("BUTTERPOLLO_TEST_SEND_LOSS").ok()?);
        if let Some(loss) = &loss {
            tracing::warn!(
                loss_millionths = loss.millionths,
                air = loss.air,
                "BUTTERPOLLO_TEST_SEND_LOSS drops video datagrams at random; unset it outside tests"
            );
        }
        loss
    }
    /// Lost in the air: the host sent it and does not know it was lost.
    pub fn in_air(&self) -> bool {
        self.air
    }
    /// Whether the next datagram is lost.
    pub fn drops(&mut self) -> bool {
        split_mix(&mut self.state) % 1_000_000 < self.millionths
    }
}
/// `prefer_sdr_10bit` streams an HDR request as ten-bit SDR, without the HDR
/// format the client picked.
pub fn apply_color(stream: &mut Negotiated, config: &Config) {
    if stream.hdr && config.boolean("prefer_sdr_10bit", false) {
        stream_sdr_10bit(stream);
    }
}
/// Why a packed 4:4:4 stream never carries Dolby Vision, for the log, the
/// session warning and the setting's note in the console.
pub const PACKED_DOLBY_VISION: &str = "Dolby Vision is off for packed 4:4:4 streams: the device unpacks the packed picture itself, so a Dolby Vision decoder would tone-map the packed layout instead of the picture. The stream carries static HDR10 instead";
/// A packed 4:4:4 stream whose client picked Dolby Vision streams static
/// HDR10, from the control channel: the client decodes the packed
/// picture as plain sample data and unpacks it in its own shader, and the
/// RPU would not survive the unpack. The log and the session's warnings say
/// so; any other stream clears the warning. Returns whether it dropped it.
pub fn packed_drops_dolby_vision(
    stream: &mut Negotiated,
    warnings: &crate::session::Warnings,
) -> bool {
    let dropped = stream.chroma_packing != 0
        && stream.hdr_format & rubylight_protocol::hdr::DOLBY_VISION != 0;
    if dropped {
        tracing::warn!(hdr_format = stream.hdr_format, "{PACKED_DOLBY_VISION}");
        warnings.set("hdr_dolby_vision", PACKED_DOLBY_VISION);
        stream.hdr_format = 0;
    } else {
        warnings.clear("hdr_dolby_vision");
    }
    dropped
}
/// Why a packed 4:4:4 stream never carries HLG, for the log and the session
/// warning.
pub const PACKED_HLG: &str = "HLG is off for packed 4:4:4 streams: the device unpacks the packed picture itself and shows it as HDR10. The stream carries static HDR10 instead";
/// A packed 4:4:4 stream whose client picked HLG streams HDR10, like one that
/// picked Dolby Vision ([`packed_drops_dolby_vision`], which drops 8.4 as
/// Dolby Vision first): the client's unpack shader shows the picture as PQ.
/// The log and the session's warnings say so; any other stream clears the
/// warning. Returns whether it dropped it.
pub fn packed_drops_hlg(stream: &mut Negotiated, warnings: &crate::session::Warnings) -> bool {
    let dropped = stream.chroma_packing != 0 && stream.hdr_format == rubylight_protocol::hdr::HLG;
    if dropped {
        tracing::warn!(hdr_format = stream.hdr_format, "{PACKED_HLG}");
        warnings.set("hdr_hlg", PACKED_HLG);
        stream.hdr_format = 0;
    } else {
        warnings.clear("hdr_hlg");
    }
    dropped
}
/// An HDR request streamed as ten-bit SDR: its HDR format pick goes with it.
fn stream_sdr_10bit(stream: &mut Negotiated) {
    stream.sdr_10bit = true;
    stream.hdr = false;
    stream.hdr_format = 0;
}
/// The protocol has no flag for ten-bit SDR: a client that wants it
/// negotiates a ten-bit format in RTSP (`dynamicRangeMode=1`) while its launch
/// did not ask for HDR. `sdr_requested` is what `Launch::ten_bit_sdr_requested`
/// decides from the query. Such a stream is ten-bit SDR, so the display stays
/// as launched. Returns whether it was inferred.
pub fn infer_sdr_10bit(stream: &mut Negotiated, sdr_requested: bool) -> bool {
    let inferred = stream.hdr && sdr_requested;
    if inferred {
        stream_sdr_10bit(stream);
    }
    inferred
}
pub fn apply(stream: &mut Negotiated, launch_millihz: u32, config: &Config) {
    // The stream runs at the rate its request asks for, as in Vibepollo,
    // where `limit_framerate` only aligns encoder timestamps to the launch
    // rate. Turned on here, the stream is encoded at the launch rate.
    let limit = config.boolean("limit_framerate", false);
    if limit && (1..=4_000_000).contains(&launch_millihz) {
        stream.rate_millihz = launch_millihz;
    }
    let configured = u64::from(stream.configured_bitrate_kbps);
    let requested = if configured > 0 {
        configured
    } else {
        u64::from(stream.bitrate_kbps)
    };
    // A stream request at a multiple of the launch rate (Artemis's Warp modes
    // ask for 2x or 4x) keeps the client's bitrate. Vibepollo multiplies it
    // by that factor, which sent two to four times the bitrate the client set:
    // more than an Onn 4K Pro's 100 Mbps port carries. Each frame gets a half
    // or a quarter of Vibepollo's budget instead.
    let ceiling = config.integer("max_bitrate", 0);
    let mut budget = requested.min(if ceiling > 0 {
        ceiling as u64
    } else {
        i32::MAX as u64 / 1000
    });
    if configured > 0 {
        let fec = config.integer("fec_percentage", 20);
        if stream.codec != 3 && (0..=80).contains(&fec) {
            budget = budget * (100 - fec as u64) / 100;
        }
        let audio = u64::from(stream.audio_channels) * if stream.audio_quality { 256 } else { 96 };
        if stream.codec == 3 {
            // PyroWave's bitrate is the stream's whole wire budget, as
            // Vibeshine takes it: audio with its 50% parity and control come
            // off in full, uncapped, so a calibrated total holds.
            budget = budget.saturating_sub((audio * 3).div_ceil(2) + 500);
        } else {
            budget -= audio.min(budget / 5);
            budget -= 500.min(budget / 10);
        }
    }
    stream.bitrate_kbps = budget.clamp(1, 2_000_000) as u32;
}
/// Video packets always use the size the device asked for. Moonlight receives into a
/// buffer of that size, so a larger packet is cut short (#52: 1200 bytes against the 1184
/// a remote IPv6 Moonlight asks for, a green picture with no loss), and it rebuilds
/// FEC-recovered packets at that size, so a smaller one corrupts every repaired frame.
/// A `packetsize` left in the config is ignored and reported.
pub fn report_packet_size(warnings: &crate::session::Warnings, config: &Config) {
    let packet_size = config.integer("packetsize", 0);
    if packet_size != 0 {
        warnings.set("packet_size", format!("packetsize = {packet_size} is ignored: video packets use the size the device asks for. Remove it from the config."));
    } else {
        warnings.clear("packet_size");
    }
}
/// The encoder bitrate [`apply`] would choose without `max_bitrate`.
pub fn uncapped_bitrate_kbps(stream: &Negotiated, launch_millihz: u32, config: &Config) -> u32 {
    let mut uncapped = stream.clone();
    let mut config = config.clone();
    config.values.remove("max_bitrate");
    apply(&mut uncapped, launch_millihz, &config);
    uncapped.bitrate_kbps
}
pub fn report_bitrate(
    warnings: &crate::session::Warnings,
    requested: u32,
    applied: u32,
    reason: &str,
) {
    if applied < requested {
        warnings.set("network_bitrate", format!("Encoder bitrate reduced from {requested} to {applied} Kbps: {reason}. Picture detail may be lower; check Maximum bitrate and the client bitrate, leaving room for audio and FEC."));
    } else {
        warnings.clear("network_bitrate");
    }
}

/// The bitrate a client may set during a stream: `max_bitrate` caps it, and
/// 500 Mbps keeps it inside the encoders' rate fields, as in Vibepollo.
pub fn runtime_bitrate_kbps(config: &Config, requested: u32) -> u32 {
    let ceiling = config.integer("max_bitrate", 0);
    let applied = requested.min(500_000);
    if ceiling > 0 {
        applied.min(ceiling.min(i64::from(u32::MAX)) as u32)
    } else {
        applied
    }
}
/// How long an encoder holding a full backlog may return nothing before the
/// stream recreates it. The first stall waits 250 ms; each recreation that
/// brings no frame back doubles the wait, up to 2 s. A GPU saturated by a
/// game (99% in an RX 9070 XT report) can hold the encoder's input for well
/// over a frame; recreating it then only adds an encoder start and a large
/// keyframe to a GPU that is already behind, and a stream that should have
/// turned choppy ended instead. A fresh encoder's first 4K keyframe on the
/// RX 9070 XT's single VCN can outlast a short limit too.
pub fn encoder_stall_limit(recreations: u32) -> Duration {
    Duration::from_millis(250 << recreations.min(3))
}
#[cfg(test)]
mod tests;
