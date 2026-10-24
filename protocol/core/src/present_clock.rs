//! Showing frames at the game's own cadence on a variable refresh rate display.
//!
//! With VRR the client's display refreshes when a frame is presented, so the time between
//! two refreshes is the time between two presents. Presenting each frame as soon as it is
//! decoded copies the network's and the decoder's jitter onto the screen: a game running
//! at an even 90 fps arrives, and is shown, at 8, 14, 9, 13 ms intervals.
//!
//! The host stamps every frame with the game's present time (the RTP timestamp, 90 kHz).
//! [`PresentClock`] shows each frame at that time plus a constant delay, so the intervals
//! on screen are the game's intervals. The delay is a high percentile of how late frames
//! have been ready recently, plus a margin; it rises at once when frames get later and
//! falls when they get earlier (by a share of the gap per 100 ms, so a stall is forgotten in
//! seconds). A frame later than the delay is shown at once, and never held more than
//! `max_hold_us` beyond the typical frame.
//!
//! Scheduling needs no shared clock: the delay is measured against the frames' own
//! timestamps, so any offset between the two clocks cancels and slow drift is followed
//! by the window. [`ClockOffset`] (fed by clock sync, control message 0x5537) adds the
//! absolute numbers, the host present to client present latency, for statistics.

use crate::control::ClockSyncReply;

pub mod content_rate;
use content_rate::ContentRate;

/// Clock sync samples kept, about 8 s at four probes a second.
const SYNC_SAMPLES: usize = 32;
/// Most frames the lateness window holds.
pub const MAX_WINDOW: usize = 512;

#[derive(Clone, Copy, Debug, Default)]
struct SyncSample {
    local_us: i64,
    offset_us: i64,
    rtt_us: i64,
}

/// The offset from the host's video clock to the client's clock (`client = host + offset`),
/// from NTP-style clock sync exchanges.
#[derive(Clone, Debug, Default)]
pub struct ClockOffset {
    samples: [SyncSample; SYNC_SAMPLES],
    len: usize,
    next: usize,
}

impl ClockOffset {
    /// Adds the exchange `reply` answered, received at `client_receive_us`. Returns its round
    /// trip, or `None` for an exchange that cannot be right. An offset more than a second
    /// away from the current one means the host restarted its video clock: the older
    /// samples are dropped.
    pub fn add(&mut self, reply: &ClockSyncReply, client_receive_us: u64) -> Option<i64> {
        let t1 = reply.client_us as i64;
        let t2 = reply.host_receive_us as i64;
        let t3 = reply.host_send_us as i64;
        let t4 = client_receive_us as i64;
        let rtt = (t4 - t1) - (t3 - t2);
        if t4 < t1 || !(0..=1_000_000).contains(&rtt) {
            return None;
        }
        let offset = ((t1 - t2) + (t4 - t3)) / 2;
        if self
            .offset_at(t4)
            .is_some_and(|current| (offset - current).abs() > 1_000_000)
        {
            self.clear();
        }
        self.samples[self.next] = SyncSample {
            local_us: t1 + (t4 - t1) / 2,
            offset_us: offset,
            rtt_us: rtt,
        };
        self.next = (self.next + 1) % SYNC_SAMPLES;
        self.len = (self.len + 1).min(SYNC_SAMPLES);
        Some(rtt)
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }

    fn live(&self) -> &[SyncSample] {
        &self.samples[..self.len]
    }

    fn best(samples: impl Iterator<Item = SyncSample>) -> Option<SyncSample> {
        samples.min_by_key(|s| s.rtt_us)
    }

    /// The round trip of the exchange the offset comes from.
    pub fn rtt_us(&self) -> Option<i64> {
        Self::best(self.live().iter().copied()).map(|s| s.rtt_us)
    }

    /// The offset at client time `local_us`: the exchange with the shortest round trip
    /// (whose error is at most half of it), carried forward by the drift between the best
    /// exchanges of the older and the newer half of the samples.
    pub fn offset_at(&self, local_us: i64) -> Option<i64> {
        let live = self.live();
        let best = Self::best(live.iter().copied())?;
        let (oldest, newest) = live.iter().fold((i64::MAX, i64::MIN), |(lo, hi), s| {
            (lo.min(s.local_us), hi.max(s.local_us))
        });
        let middle = oldest + (newest - oldest) / 2;
        let older = Self::best(live.iter().copied().filter(|s| s.local_us < middle));
        let newer = Self::best(live.iter().copied().filter(|s| s.local_us >= middle));
        // Parts per billion, at most 1000 ppm; quartz is within 100 ppm. No i128: 32-bit
        // Android has no 128-bit division in its runtime. The offsets differ by under a
        // second (a larger jump clears the samples), so the products fit in an i64.
        let drift_ppb = match (older, newer) {
            (Some(a), Some(b)) if b.local_us - a.local_us >= 2_000_000 => {
                ((b.offset_us - a.offset_us).clamp(-1_000_000, 1_000_000) * 1_000_000_000
                    / (b.local_us - a.local_us))
                    .clamp(-1_000_000, 1_000_000)
            }
            _ => 0,
        };
        // A day either way keeps the product within an i64.
        let elapsed = (local_us - best.local_us).clamp(-86_400_000_000, 86_400_000_000);
        Some(best.offset_us + elapsed * drift_ppb / 1_000_000_000)
    }
}

/// How [`PresentClock`] trades latency for even frame times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Frames whose lateness sets the delay (up to [`MAX_WINDOW`]).
    pub window: usize,
    /// Share of frames, in percent, that should be ready before their present time.
    pub percentile: u8,
    /// Added to the delay for the present call itself, in microseconds.
    pub margin_us: i64,
    /// The least the delay falls per frame when it is above the percentile, in microseconds;
    /// it falls faster the further above it is, see [`FALL_TAU_US`].
    pub decay_us: i64,
    /// The longest a frame is held after it is ready, in microseconds.
    pub max_hold_us: i64,
    /// The display's shortest refresh interval, in microseconds; presents are never closer.
    pub min_interval_us: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: 128,
            percentile: 97,
            margin_us: 500,
            decay_us: 25,
            max_hold_us: 50_000,
            min_interval_us: 0,
        }
    }
}

/// When to show one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// Client time to present at; the frame's ready time when it is late.
    pub present_us: u64,
    /// The frame was ready after the time the delay gave it.
    pub late: bool,
    /// How long the frame is held after it is ready.
    pub hold_us: i64,
    /// The host's present of this frame to its present here, when clock sync runs.
    pub latency_us: Option<i64>,
}

/// The display's vsync timeline, for a panel that refreshes on a fixed grid (60, 90, 120 Hz).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Any vsync time, past or future, on the client clock in microseconds.
    pub anchor_us: i64,
    /// The time between vsyncs, in microseconds; zero or less means no grid.
    pub interval_us: i64,
    /// How long before its vsync a frame is released, in microseconds: the compositor's latch
    /// deadline plus a margin. A frame ready later than that is released at once.
    pub lead_us: i64,
    /// A vsync up to this long before the schedule's present time is taken (the frame is
    /// shown a little early). Zero rounds the present time up to the next vsync; half an
    /// interval rounds to the nearest. Never earlier than the first vsync the frame can make.
    pub slack_us: i64,
}

/// When to release one frame so it is shown at a chosen vsync; see [`PresentClock::schedule_on_grid`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSchedule {
    /// Client time to release the frame at: `vsync_us - lead_us`, never before it is ready.
    pub release_us: u64,
    /// The vsync the frame is meant for.
    pub vsync_us: u64,
    /// `vsync_us` as a count of intervals after the grid's anchor (negative before it).
    pub vsync_index: i64,
    /// The frame was ready after its present time (the host time plus the delay): it is
    /// behind the game's cadence.
    pub late: bool,
    /// How far past that present time the frame was ready, in microseconds; zero when on time.
    pub late_by_us: i64,
    /// The frame has the same vsync as the one scheduled before it and would replace it on
    /// screen (or lose to it): content faster than the panel.
    pub shares_vsync: bool,
    /// How long the frame is held after it is ready (`release_us` minus the ready time).
    pub hold_us: i64,
    /// The host's present of this frame to its vsync, when clock sync runs.
    pub latency_us: Option<i64>,
}

/// The first vsync at or after `t_us` on the grid through `anchor_us` with the given interval:
/// its time and its index from the anchor (negative before it). Without an interval (zero or
/// less) the grid is every instant: `t_us` itself.
pub fn snap(t_us: i64, anchor_us: i64, interval_us: i64) -> (i64, i64) {
    if interval_us <= 0 {
        return (t_us, 0);
    }
    let index = ceil_div(t_us - anchor_us, interval_us);
    (anchor_us + index * interval_us, index)
}

fn ceil_div(a: i64, b: i64) -> i64 {
    (a + b - 1).div_euclid(b)
}

/// Schedules decoded frames by their host timestamps. See the module documentation.
#[derive(Clone, Debug)]
pub struct PresentClock {
    pub offset: ClockOffset,
    pub settings: Settings,
    /// Ready time minus host time, per frame (an arbitrary offset plus the lateness).
    window: [i64; MAX_WINDOW],
    len: usize,
    next: usize,
    /// The last RTP timestamp and its unwrapped tick count.
    unwrap: Option<(u32, i64)>,
    delay_us: Option<i64>,
    last_present: Option<i64>,
    /// Host time of the first frame and of the previous one, in microseconds.
    start_us: Option<i64>,
    last_host_us: Option<i64>,
    /// Frames shown late since the clock started.
    pub late_frames: u64,
    pub frames: u64,
    /// The vsync the previous grid schedule chose.
    last_vsync_us: Option<i64>,
    /// That frame was moved to the vsync after its first possible one.
    last_bumped: bool,
    rate: ContentRate,
    rate_unwrap: Option<(u32, i64)>,
}

impl Default for PresentClock {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

/// A jump this large between two frames' timestamps is a restarted video clock.
const DISCONTINUITY_TICKS: i64 = 10 * 90_000;
/// Host time after the start (or a restarted clock) in which frames are shown as soon as they
/// are ready and left out of the lateness window. The first pictures wait for the decoder and
/// the GPU to warm up and come in a burst, 90 ms late and falling; with the delay seeded from
/// them and falling at `decay_us` a frame, every picture was held for the longest hold for the
/// first minute of the stream. Counted in host time so the frame rate does not matter.
const WARMUP_US: i64 = 250_000;
/// Time constant of the delay's fall, in host microseconds: the gap between the delay and the
/// lateness percentile shrinks by `dt / TAU` for each `dt` of host time (at least `decay_us`
/// a frame), so a stall's delay is gone in seconds at any frame rate.
const FALL_TAU_US: i64 = 500_000;
/// The longest host time step that counts towards the fall.
const FALL_STEP_MAX_US: i64 = 100_000;
/// The share of frames, in percent, that should be ready before their present time when frames
/// go onto a vsync grid. The grid already holds a frame to the next vsync, and a frame ready
/// after its present time still takes the first vsync it can make, so covering the worst
/// frames of the window ([`Settings::percentile`]) only adds latency to all the others.
const GRID_PERCENTILE: u8 = 75;

impl PresentClock {
    pub fn new(settings: Settings) -> Self {
        Self {
            offset: ClockOffset::default(),
            settings,
            window: [0; MAX_WINDOW],
            len: 0,
            next: 0,
            unwrap: None,
            delay_us: None,
            last_present: None,
            start_us: None,
            last_host_us: None,
            late_frames: 0,
            frames: 0,
            last_vsync_us: None,
            last_bumped: false,
            rate: ContentRate::new(),
            rate_unwrap: None,
        }
    }

    /// Forgets the frames seen so far, for example after the stream was reconfigured. The
    /// clock sync samples stay.
    pub fn reset(&mut self) {
        self.len = 0;
        self.next = 0;
        self.unwrap = None;
        self.delay_us = None;
        self.last_present = None;
        self.start_us = None;
        self.last_host_us = None;
        self.last_vsync_us = None;
        self.last_bumped = false;
        self.rate.reset();
        self.rate_unwrap = None;
    }

    /// The current delay from host present to client present, beyond the clocks' offset.
    pub fn delay_us(&self) -> Option<i64> {
        self.delay_us
    }

    /// Host microseconds of an RTP timestamp, unwrapped against the previous one.
    fn host_us(&mut self, rtp: u32) -> i64 {
        let ticks = match self.unwrap {
            Some((last, ticks)) => {
                let step = i64::from(rtp.wrapping_sub(last) as i32);
                if step.abs() > DISCONTINUITY_TICKS {
                    self.reset();
                    i64::from(rtp)
                } else {
                    ticks + step
                }
            }
            None => i64::from(rtp),
        };
        self.unwrap = Some((rtp, ticks));
        ticks * 100 / 9
    }

    fn quantile(&self, percentile: u8) -> i64 {
        let mut sorted = [0i64; MAX_WINDOW];
        let values = &mut sorted[..self.len];
        values.copy_from_slice(&self.window[..self.len]);
        let rank = (self.len - 1) * usize::from(percentile.min(100)) / 100;
        *values.select_nth_unstable(rank).1
    }

    /// When to show the frame stamped `rtp` that was ready to present at `ready_us`.
    pub fn schedule(&mut self, rtp: u32, ready_us: u64) -> Schedule {
        self.schedule_frame(rtp, ready_us, self.settings.percentile).0
    }

    /// Notes a frame with RTP timestamp `rtp` for the content rate. Call it for every frame in
    /// presentation order as it arrives, whether or not frames are scheduled.
    pub fn observe(&mut self, rtp: u32) {
        let ticks = match self.rate_unwrap {
            Some((last, ticks)) => {
                let step = i64::from(rtp.wrapping_sub(last) as i32);
                if step.abs() > DISCONTINUITY_TICKS {
                    self.rate.reset();
                    i64::from(rtp)
                } else {
                    ticks + step
                }
            }
            None => i64::from(rtp),
        };
        self.rate_unwrap = Some((rtp, ticks));
        self.rate.observe(ticks * 100 / 9);
    }

    /// The rate the game presents at, in millihertz, once it held steady for two seconds.
    pub fn content_rate_mhz(&self) -> Option<u32> {
        self.rate.rate_mhz()
    }

    /// When to release the frame stamped `rtp`, ready at `ready_us`, so it is shown at a vsync of
    /// `grid`. The frame's present time is [`schedule`](Self::schedule)'s (the delay is learned
    /// the same way); it is then moved onto the grid:
    ///
    /// - the vsync is the first one at or after the present time (less `slack_us`), but never
    ///   before the first vsync at or after the ready time, which is the earliest the panel
    ///   could show the frame whatever is done; so a frame is never held past the vsync it
    ///   would have had if released at once, unless the one before it has that vsync;
    /// - a frame that would share its vsync with the previous one goes to the next vsync if that
    ///   is one after its first possible one and the previous frame was not moved either,
    ///   otherwise it shares and says so;
    /// - release is `lead_us` before the vsync, and at once for a frame ready later than that.
    ///
    /// On a grid the delay covers [`GRID_PERCENTILE`] of the window, not `Settings::percentile`.
    ///
    /// Without an interval this is [`schedule`](Self::schedule) with the extra fields.
    pub fn schedule_on_grid(&mut self, rtp: u32, ready_us: u64, grid: Grid) -> GridSchedule {
        let percentile = if grid.interval_us > 0 {
            GRID_PERCENTILE.min(self.settings.percentile)
        } else {
            self.settings.percentile
        };
        let (base, late_by_us, host) = self.schedule_frame(rtp, ready_us, percentile);
        let ready = ready_us as i64;
        let present = base.present_us as i64;
        if grid.interval_us <= 0 {
            return GridSchedule {
                release_us: base.present_us,
                vsync_us: base.present_us,
                vsync_index: 0,
                late: base.late,
                late_by_us,
                shares_vsync: false,
                hold_us: base.hold_us,
                latency_us: base.latency_us,
            };
        }
        let interval = grid.interval_us;
        let first = ceil_div(ready - grid.anchor_us, interval);
        let mut index = first.max(ceil_div(present - grid.slack_us.max(0) - grid.anchor_us, interval));
        let mut shares = false;
        let mut bumped = false;
        if let Some(last) = self.last_vsync_us {
            // The previous frame's vsync as an index on this call's grid (the anchor moves).
            let taken = (last - grid.anchor_us + interval / 2).div_euclid(interval);
            if index <= taken {
                if taken == first && !self.last_bumped {
                    // One vsync past its first possible one is the most a frame is moved, and
                    // not two frames in a row: content faster than the panel would then run
                    // one vsync behind for good.
                    index = first + 1;
                    bumped = true;
                } else {
                    index = taken;
                    shares = true;
                }
            }
        }
        let vsync = grid.anchor_us + index * interval;
        self.last_vsync_us = Some(vsync);
        self.last_bumped = bumped;
        let release = (vsync - grid.lead_us.max(0)).max(ready);
        GridSchedule {
            release_us: release as u64,
            vsync_us: vsync as u64,
            vsync_index: index,
            late: base.late,
            late_by_us,
            shares_vsync: shares,
            hold_us: release - ready,
            latency_us: self.offset.offset_at(vsync).map(|offset| vsync - (host + offset)),
        }
    }

    /// [`schedule`](Self::schedule), with how far past its present time the frame was ready and
    /// its host time.
    fn schedule_frame(&mut self, rtp: u32, ready_us: u64, percentile: u8) -> (Schedule, i64, i64) {
        let ready = ready_us as i64;
        let host = self.host_us(rtp);
        let lateness = ready - host;
        let start = *self.start_us.get_or_insert(host);
        let step = self.last_host_us.replace(host).map_or(0, |last| host - last);
        let mut present = if host - start < WARMUP_US {
            ready + self.settings.margin_us
        } else {
            let window = self.settings.window.clamp(1, MAX_WINDOW);
            if self.len > window || self.next >= window {
                // The window was made smaller: keep the newest frames.
                self.len = 0;
                self.next = 0;
            }
            self.window[self.next] = lateness;
            self.next = (self.next + 1) % window;
            self.len = (self.len + 1).min(window);
            let wanted = self.quantile(percentile) + self.settings.margin_us;
            let mut delay = match self.delay_us {
                Some(held) if wanted < held => {
                    let dt = step.clamp(0, FALL_STEP_MAX_US);
                    let fall = (held - wanted) * dt / FALL_TAU_US;
                    let fall = fall.max(self.settings.decay_us.max(0));
                    (held - fall).max(wanted)
                }
                _ => wanted,
            };
            // However long the frames were late, a frame is held at most `max_hold_us` beyond
            // the typical one.
            delay = delay.min(self.quantile(50) + self.settings.max_hold_us.max(0));
            self.delay_us = Some(delay);
            host + delay
        };
        if let Some(last) = self.last_present {
            present = present.max(last + self.settings.min_interval_us);
        }
        let late = present < ready;
        let late_by = (ready - present).max(0);
        present = present.clamp(ready, ready + self.settings.max_hold_us.max(0));
        self.last_present = Some(present);
        self.frames += 1;
        self.late_frames += u64::from(late);
        let schedule = Schedule {
            present_us: present as u64,
            late,
            hold_us: present - ready,
            latency_us: self.offset.offset_at(present).map(|offset| present - (host + offset)),
        };
        (schedule, late_by, host)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod grid_tests;
