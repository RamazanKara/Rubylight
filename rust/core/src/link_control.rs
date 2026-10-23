//! What the session adds around [`LinkRate`](crate::link_rate::LinkRate): the
//! lock-free record of what the video sender sent, and the state the host
//! needs to apply the controller's answer.
//!
//! The sender thread must never wait for the controller's lock, so each frame
//! it sends is one store into [`SentLog`], and the control thread drains the
//! log into the controller before it reads a client report or ticks.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::link_rate::LinkRate;

/// Frames kept. The control thread drains every few milliseconds; a drain more
/// than this many frames late (17 s at 60 fps) loses the oldest frames.
const LOG: usize = 1024;

/// One slot per recent wire frame, written by the video sender only.
///
/// A slot is two words, `frame << 32 | packets` and `wire_bytes << 32 | ms`
/// (milliseconds since the log started). `head` counts the frames written and
/// is stored last with release ordering, so a reader that loads it sees whole
/// slots below it.
pub struct SentLog {
    started: Instant,
    head: AtomicU64,
    live: AtomicBool,
    slots: Box<[[AtomicU64; 2]]>,
}

impl SentLog {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            head: AtomicU64::new(0),
            live: AtomicBool::new(false),
            slots: (0..LOG)
                .map(|_| [AtomicU64::new(0), AtomicU64::new(0)])
                .collect(),
        }
    }

    /// Whether a controller reads the log. The sender and the control thread
    /// skip everything while it is off.
    pub fn live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Relaxed);
    }

    /// The frames written so far; a reader starting now sees only later ones.
    pub fn head(&self) -> u64 {
        self.head.load(Ordering::Acquire)
    }

    /// Record a frame the sender sent: its wire frame number, packets (data and
    /// parity) and wire bytes. One store per word, no lock, no allocation.
    /// Call from one thread at a time (the sender's, in frame order).
    pub fn record(&self, frame: u32, packets: u32, wire_bytes: u64, now: Instant) {
        if !self.live() {
            return;
        }
        let n = self.head.load(Ordering::Relaxed);
        let slot = &self.slots[n as usize % LOG];
        let ms = now.saturating_duration_since(self.started).as_millis() as u32;
        slot[0].store(u64::from(frame) << 32 | u64::from(packets), Ordering::Relaxed);
        slot[1].store(
            u64::from(wire_bytes.min(u64::from(u32::MAX)) as u32) << 32 | u64::from(ms),
            Ordering::Relaxed,
        );
        self.head.store(n + 1, Ordering::Release);
    }

    /// Frames recorded since `cursor`, oldest first, as `(frame, packets,
    /// wire_bytes, sent_at)`. Moves the cursor to the head. Frames the sender
    /// has already overwritten are skipped.
    pub fn drain(&self, cursor: &mut u64, mut each: impl FnMut(u32, u32, u32, Instant)) {
        let head = self.head.load(Ordering::Acquire);
        // The slot being written next is the oldest one in a full ring.
        let first = (*cursor).max(head.saturating_sub(LOG as u64 - 2));
        for n in first..head {
            let slot = &self.slots[n as usize % LOG];
            let (a, b) = (
                slot[0].load(Ordering::Relaxed),
                slot[1].load(Ordering::Relaxed),
            );
            if self.head.load(Ordering::Acquire).wrapping_sub(n) >= LOG as u64 - 1 {
                continue;
            }
            each(
                (a >> 32) as u32,
                a as u32,
                (b >> 32) as u32,
                self.started + Duration::from_millis(b & 0xffff_ffff),
            );
        }
        *cursor = head;
    }
}

/// The controller with the session's bookkeeping around it, behind one mutex.
pub struct LinkControl {
    pub rate: LinkRate,
    /// Read position in the [`SentLog`].
    pub cursor: u64,
    /// The encoder bitrate before the controller lowered it, put back when the
    /// controller returns to the requested rate.
    pub restore: Option<u32>,
    /// The controller is below the requested rate.
    pub limited: bool,
    /// The loss share (0..1) behind the last step down, for the warning text.
    pub loss_share: f64,
    /// When the last change was logged at INFO.
    pub logged: Option<Instant>,
    sample: Option<(Instant, u64, u64)>,
    busy: bool,
}

impl Default for LinkControl {
    fn default() -> Self {
        Self::new(LinkRate::default(), 0)
    }
}

impl LinkControl {
    pub fn new(rate: LinkRate, cursor: u64) -> Self {
        Self {
            rate,
            cursor,
            restore: None,
            limited: false,
            loss_share: 0.,
            logged: None,
            sample: None,
            busy: false,
        }
    }

    /// Feed what the sender recorded since the last drain to the controller.
    pub fn drain(&mut self, log: &SentLog) {
        let Self { rate, cursor, .. } = self;
        log.drain(cursor, |frame, packets, bytes, at| {
            rate.on_sent(frame, packets, bytes, at);
        });
    }

    /// Whether video is busy, as the legacy controller defines it: over the
    /// last sample (at least 500 ms, at most 2 s old) at least 60 % of the
    /// frames and bytes the stream should send. A static desktop sends almost
    /// nothing, so a probe sent then proves nothing.
    pub fn busy(
        &mut self,
        now: Instant,
        frames: u64,
        bytes: u64,
        period: Duration,
        kbps: u32,
    ) -> bool {
        let Some((at, before_frames, before_bytes)) = self.sample else {
            self.sample = Some((now, frames, bytes));
            return false;
        };
        let elapsed = now.saturating_duration_since(at);
        if elapsed < Duration::from_millis(500) {
            return self.busy;
        }
        self.sample = Some((now, frames, bytes));
        let seconds = elapsed.as_secs_f64();
        self.busy = elapsed <= Duration::from_secs(2)
            && frames.saturating_sub(before_frames) as f64 >= seconds / period.as_secs_f64() * 0.6
            && bytes.saturating_sub(before_bytes) as f64 * 8. >= f64::from(kbps) * 1000. * seconds * 0.6;
        self.busy
    }

    /// The client set the bitrate itself: the controller stays off and the
    /// bitrate it lowered is no longer ours to put back.
    pub fn takeover(&mut self) {
        self.rate.client_takeover();
        self.restore = None;
        self.limited = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_hands_over_frames_in_order_and_only_once() {
        let started = Instant::now();
        let log = SentLog::new(started);
        log.record(1, 10, 1000, started);
        assert_eq!(log.head(), 0, "nothing is recorded while no controller reads");
        log.set_live(true);
        let mut cursor = 0;
        let mut seen = Vec::new();
        for frame in 1..=3u32 {
            log.record(
                frame,
                frame * 10,
                u64::from(frame) * 1000,
                started + Duration::from_millis(u64::from(frame) * 16),
            );
        }
        log.drain(&mut cursor, |f, p, b, at| {
            seen.push((f, p, b, at.duration_since(started).as_millis()))
        });
        assert_eq!(
            seen,
            [(1, 10, 1000, 16), (2, 20, 2000, 32), (3, 30, 3000, 48)]
        );
        log.drain(&mut cursor, |f, _, _, _| panic!("frame {f} seen twice"));
        log.record(4, 1, u64::MAX, started);
        log.drain(&mut cursor, |f, _, b, _| assert_eq!((f, b), (4, u32::MAX)));
    }

    #[test]
    fn a_reader_that_fell_behind_loses_the_oldest_frames_not_the_newest() {
        let started = Instant::now();
        let log = SentLog::new(started);
        log.set_live(true);
        for frame in 0..3000u32 {
            log.record(frame, 1, 1, started);
        }
        let mut cursor = 0;
        let mut frames = Vec::new();
        log.drain(&mut cursor, |f, _, _, _| frames.push(f));
        assert_eq!(frames.last(), Some(&2999));
        assert!(frames.len() < LOG);
        assert!(frames.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[test]
    fn video_is_busy_only_at_most_of_its_rate() {
        let t0 = Instant::now();
        let period = Duration::from_micros(16_667);
        let mut link = LinkControl::default();
        // 10 Mbps for one second is 1.25 MB.
        assert!(!link.busy(t0, 0, 0, period, 10_000));
        let t1 = t0 + Duration::from_secs(1);
        assert!(link.busy(t1, 60, 1_250_000, period, 10_000));
        let t2 = t1 + Duration::from_secs(1);
        assert!(!link.busy(t2, 62, 1_250_100, period, 10_000), "a static picture");
        // Between samples the last verdict stands.
        assert!(!link.busy(t2 + Duration::from_millis(100), 90, 9_000_000, period, 10_000));
    }
}
