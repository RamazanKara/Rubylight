//! Adaptive forward error correction: the share of recovery packets in each
//! video frame follows the client's FEC status reports (control message
//! 0x5502, see [`crate::fec_status`]).
//!
//! The configured `fec_percentage` is the ceiling. A frame the client could not
//! rebuild puts the percentage straight back to it; a frame it rebuilt from
//! parity raises it quickly; several seconds without either let it fall slowly
//! towards [`AdaptiveFec::FLOOR`]. A client that never reports keeps the
//! configured percentage for the whole stream, as does a host with
//! `adaptive_fec` off. The floor stays above zero, so the client's minimum
//! parity per block (`minRequiredFecPackets`, which the packetizer applies to
//! every block whose percentage is not zero) keeps applying.
use std::time::{Duration, Instant};

/// What the client reported for one FEC block of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    /// Every data packet arrived.
    Clean,
    /// `lost` of the block's `data` packets were missing and parity rebuilt them.
    Recovered { lost: u16, data: u16 },
    /// Too few packets arrived to rebuild the block.
    Unrecoverable,
}

#[derive(Debug)]
pub struct AdaptiveFec {
    enabled: bool,
    ceiling: u8,
    floor: u8,
    current: u8,
    /// Set by the first report: until then the percentage never moves.
    active: bool,
    /// The last loss, or the first report: decay starts [`Self::CLEAN_HOLD`] later.
    quiet_since: Option<Instant>,
    last_raise: Option<Instant>,
    last_decay: Option<Instant>,
    logged: u8,
    logged_at: Option<Instant>,
}

impl Default for AdaptiveFec {
    /// Off at the default `fec_percentage` until the stream configures it.
    fn default() -> Self {
        Self::new(false, 20)
    }
}

impl AdaptiveFec {
    /// The lowest percentage a clean link decays to (or the ceiling, when lower).
    pub const FLOOR: u8 = 5;
    /// Percentage points added for a rebuilt block.
    pub const STEP_UP: u8 = 5;
    /// Rebuilt blocks closer together than this raise once, unless their loss
    /// asks for more: one bad frame reports up to four blocks.
    pub const RAISE_COOLDOWN: Duration = Duration::from_millis(250);
    /// How long the link must be clean before the percentage falls.
    pub const CLEAN_HOLD: Duration = Duration::from_secs(5);
    /// One percentage point down per interval once the link is clean.
    pub const DECAY_INTERVAL: Duration = Duration::from_secs(2);
    /// Changes are logged at most this often.
    pub const LOG_PERIOD: Duration = Duration::from_secs(1);

    /// `configured` is the `fec_percentage` setting (clamped to 0-100), and the ceiling.
    pub fn new(enabled: bool, configured: usize) -> Self {
        let ceiling = configured.min(100) as u8;
        Self {
            enabled,
            ceiling,
            floor: Self::FLOOR.min(ceiling),
            current: ceiling,
            active: false,
            quiet_since: None,
            last_raise: None,
            last_decay: None,
            logged: ceiling,
            logged_at: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The percentage for the next frame.
    pub fn percent(&self) -> usize {
        usize::from(self.current)
    }

    /// Applies one block report.
    pub fn on_block(&mut self, now: Instant, block: Block) {
        if !self.enabled {
            return;
        }
        self.active = true;
        self.quiet_since.get_or_insert(now);
        match block {
            Block::Clean | Block::Recovered { lost: 0, .. } => {}
            Block::Recovered { lost, data } => {
                // Twice the block's loss rate, so the same loss again leaves parity to spare.
                let loss = (u32::from(lost) * 100).div_ceil(u32::from(data.max(1)));
                let target = (loss * 2).min(100) as u8;
                let stepped = if self
                    .last_raise
                    .is_some_and(|at| now.saturating_duration_since(at) < Self::RAISE_COOLDOWN)
                {
                    self.current
                } else {
                    self.current.saturating_add(Self::STEP_UP)
                };
                self.set(stepped.max(target));
                self.loss(now);
            }
            Block::Unrecoverable => {
                self.set(self.ceiling);
                self.loss(now);
            }
        }
    }

    /// Lets the percentage fall on a clean link. Call regularly; a client that
    /// reports nothing about a frame had nothing to report.
    pub fn tick(&mut self, now: Instant) {
        if !self.enabled || !self.active || self.current <= self.floor {
            return;
        }
        let clean = self
            .quiet_since
            .is_some_and(|at| now.saturating_duration_since(at) >= Self::CLEAN_HOLD);
        let due = self
            .last_decay
            .is_none_or(|at| now.saturating_duration_since(at) >= Self::DECAY_INTERVAL);
        if clean && due {
            self.set(self.current - 1);
            self.last_decay = Some(now);
        }
    }

    /// `(old, new)` when the percentage changed since it was last logged, at
    /// most once per [`Self::LOG_PERIOD`].
    pub fn log_due(&mut self, now: Instant) -> Option<(usize, usize)> {
        if self.current == self.logged
            || self
                .logged_at
                .is_some_and(|at| now.saturating_duration_since(at) < Self::LOG_PERIOD)
        {
            return None;
        }
        let old = std::mem::replace(&mut self.logged, self.current);
        self.logged_at = Some(now);
        Some((usize::from(old), usize::from(self.current)))
    }

    fn loss(&mut self, now: Instant) {
        self.last_raise = Some(now);
        self.quiet_since = Some(now);
        self.last_decay = None;
    }

    fn set(&mut self, percent: u8) {
        self.current = percent.clamp(self.floor, self.ceiling);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECOVERED: Block = Block::Recovered { lost: 1, data: 50 };

    /// Ticks every 100 ms for `seconds`, returning the time after them.
    fn run(fec: &mut AdaptiveFec, from: Instant, seconds: u64) -> Instant {
        let mut now = from;
        for _ in 0..seconds * 10 {
            now += Duration::from_millis(100);
            fec.tick(now);
        }
        now
    }

    #[test]
    fn a_client_that_never_reports_keeps_the_configured_percentage() {
        let start = Instant::now();
        for configured in [0, 1, 5, 20, 100] {
            let mut fec = AdaptiveFec::new(true, configured);
            assert_eq!(fec.percent(), configured);
            run(&mut fec, start, 120);
            assert_eq!(fec.percent(), configured);
            assert_eq!(fec.log_due(start + Duration::from_secs(200)), None);
        }
    }

    #[test]
    fn off_keeps_the_configured_percentage_whatever_the_client_reports() {
        let start = Instant::now();
        let mut fec = AdaptiveFec::new(false, 20);
        fec.on_block(start, Block::Clean);
        let now = run(&mut fec, start, 60);
        assert_eq!(fec.percent(), 20);
        fec.on_block(now, RECOVERED);
        fec.on_block(now, Block::Unrecoverable);
        run(&mut fec, now, 60);
        assert_eq!(fec.percent(), 20);
        assert!(!fec.enabled());
        assert_eq!(fec.log_due(now), None);
    }

    #[test]
    fn a_clean_link_decays_slowly_to_the_floor() {
        let start = Instant::now();
        let mut fec = AdaptiveFec::new(true, 20);
        fec.on_block(start, Block::Clean);
        // Nothing moves for the first five seconds.
        fec.tick(start + AdaptiveFec::CLEAN_HOLD - Duration::from_millis(1));
        assert_eq!(fec.percent(), 20);
        fec.tick(start + AdaptiveFec::CLEAN_HOLD);
        assert_eq!(fec.percent(), 19);
        // Then one point every two seconds: not at the floor after 20 s...
        let now = run(&mut fec, start + AdaptiveFec::CLEAN_HOLD, 20);
        assert!(fec.percent() > usize::from(AdaptiveFec::FLOOR));
        // ...and there, without going below it, after a minute.
        run(&mut fec, now, 60);
        assert_eq!(fec.percent(), usize::from(AdaptiveFec::FLOOR));
    }

    #[test]
    fn loss_raises_quickly_and_an_unrecoverable_frame_jumps_to_the_ceiling() {
        let start = Instant::now();
        let mut fec = AdaptiveFec::new(true, 20);
        fec.on_block(start, Block::Clean);
        let mut now = run(&mut fec, start, 120);
        assert_eq!(fec.percent(), 5);
        // A rebuilt block: five points up at once.
        fec.on_block(now, RECOVERED);
        assert_eq!(fec.percent(), 10);
        // The frame's other blocks in the same burst do not stack...
        fec.on_block(now + Duration::from_millis(1), RECOVERED);
        assert_eq!(fec.percent(), 10);
        // ...unless their loss needs more: 4 of 50 packets asks for 16 %.
        fec.on_block(now, Block::Recovered { lost: 4, data: 50 });
        assert_eq!(fec.percent(), 16);
        // The next loss after the cooldown raises again.
        now += AdaptiveFec::RAISE_COOLDOWN;
        fec.on_block(now, RECOVERED);
        assert_eq!(fec.percent(), 20);
        // Losses restart the clean period.
        now = run(&mut fec, now, 30);
        assert!(fec.percent() < 20);
        fec.on_block(now, Block::Recovered { lost: 0, data: 50 });
        let low = fec.percent();
        fec.on_block(now, Block::Unrecoverable);
        assert_eq!(fec.percent(), 20);
        assert!(low < 20);
        fec.tick(now + AdaptiveFec::CLEAN_HOLD - Duration::from_millis(1));
        assert_eq!(fec.percent(), 20);
    }

    #[test]
    fn stays_within_the_floor_and_the_configured_ceiling() {
        let start = Instant::now();
        for configured in [0, 3, 5, 20, 100] {
            let mut fec = AdaptiveFec::new(true, configured);
            let mut now = start;
            for round in 0..200 {
                now += Duration::from_millis(300);
                let block = match round % 7 {
                    0 => Block::Unrecoverable,
                    1 => Block::Recovered {
                        lost: 255,
                        data: 255,
                    },
                    2 => Block::Recovered { lost: 1, data: 1 },
                    _ => Block::Clean,
                };
                fec.on_block(now, block);
                now = run(&mut fec, now, (round % 3) * 10);
                let floor = configured.min(usize::from(AdaptiveFec::FLOOR));
                assert!(
                    (floor..=configured).contains(&fec.percent()),
                    "{configured}: {}",
                    fec.percent()
                );
            }
        }
        // Settings above 100 % are clamped like the packetizer's.
        assert_eq!(AdaptiveFec::new(true, 500).percent(), 100);
        // A zero setting stays zero: no parity at all, as before.
        let mut off = AdaptiveFec::new(true, 0);
        off.on_block(start, Block::Unrecoverable);
        assert_eq!(off.percent(), 0);
    }

    #[test]
    fn changes_are_logged_at_most_once_a_second() {
        let start = Instant::now();
        let mut fec = AdaptiveFec::new(true, 20);
        fec.on_block(start, Block::Clean);
        fec.tick(start + AdaptiveFec::CLEAN_HOLD);
        let at = start + AdaptiveFec::CLEAN_HOLD;
        assert_eq!(fec.log_due(at), Some((20, 19)));
        assert_eq!(fec.log_due(at), None);
        fec.on_block(at, RECOVERED);
        assert_eq!(fec.log_due(at + Duration::from_millis(500)), None);
        assert_eq!(fec.log_due(at + Duration::from_secs(1)), Some((19, 20)));
    }
}
