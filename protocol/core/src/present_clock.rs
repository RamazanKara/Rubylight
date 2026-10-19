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
//! falls slowly when they get earlier. A frame later than the delay is shown at once.
//!
//! Scheduling needs no shared clock: the delay is measured against the frames' own
//! timestamps, so any offset between the two clocks cancels and slow drift is followed
//! by the window. [`ClockOffset`] (fed by clock sync, control message 0x5537) adds the
//! absolute numbers, the host present to client present latency, for statistics.

use crate::control::ClockSyncReply;

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
        // Parts per billion, at most 1000 ppm; quartz is within 100 ppm.
        let drift_ppb = match (older, newer) {
            (Some(a), Some(b)) if b.local_us - a.local_us >= 2_000_000 => {
                ((b.offset_us - a.offset_us) as i128 * 1_000_000_000
                    / (b.local_us - a.local_us) as i128)
                    .clamp(-1_000_000, 1_000_000) as i64
            }
            _ => 0,
        };
        let carried = (local_us - best.local_us) as i128 * drift_ppb as i128 / 1_000_000_000;
        Some(best.offset_us + carried as i64)
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
    /// How fast the delay may fall per frame, in microseconds.
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
    /// Frames shown late since the clock started.
    pub late_frames: u64,
    pub frames: u64,
}

impl Default for PresentClock {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

/// A jump this large between two frames' timestamps is a restarted video clock.
const DISCONTINUITY_TICKS: i64 = 10 * 90_000;

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
            late_frames: 0,
            frames: 0,
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

    fn quantile(&self) -> i64 {
        let mut sorted = [0i64; MAX_WINDOW];
        let values = &mut sorted[..self.len];
        values.copy_from_slice(&self.window[..self.len]);
        let rank = (self.len - 1) * usize::from(self.settings.percentile.min(100)) / 100;
        *values.select_nth_unstable(rank).1
    }

    /// When to show the frame stamped `rtp` that was ready to present at `ready_us`.
    pub fn schedule(&mut self, rtp: u32, ready_us: u64) -> Schedule {
        let ready = ready_us as i64;
        let host = self.host_us(rtp);
        let lateness = ready - host;
        let window = self.settings.window.clamp(1, MAX_WINDOW);
        if self.len > window || self.next >= window {
            // The window was made smaller: keep the newest frames.
            self.len = 0;
            self.next = 0;
        }
        self.window[self.next] = lateness;
        self.next = (self.next + 1) % window;
        self.len = (self.len + 1).min(window);
        let wanted = self.quantile() + self.settings.margin_us;
        let delay = match self.delay_us {
            Some(held) if wanted < held => (held - self.settings.decay_us.max(0)).max(wanted),
            _ => wanted,
        };
        self.delay_us = Some(delay);
        let mut present = host + delay;
        if let Some(last) = self.last_present {
            present = present.max(last + self.settings.min_interval_us);
        }
        let late = present < ready;
        present = present.clamp(ready, ready + self.settings.max_hold_us.max(0));
        self.last_present = Some(present);
        self.frames += 1;
        self.late_frames += u64::from(late);
        Schedule {
            present_us: present as u64,
            late,
            hold_us: present - ready,
            latency_us: self.offset.offset_at(present).map(|offset| present - (host + offset)),
        }
    }
}

#[cfg(test)]
mod tests;
