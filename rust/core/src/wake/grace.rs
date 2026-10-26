//! Wake grace: after a resume, the host holds the PC awake while a paired
//! client connects, before Windows' unattended-sleep timeout puts it back to
//! sleep (`docs/wake-and-audio-design.md`, A.4).
use super::idle::resumed;
use std::time::{Duration, Instant};

/// How long after a resume the grace lasts at most.
pub const WINDOW: Duration = Duration::from_secs(5 * 60);
/// How recent a paired client's request must be.
pub const CLIENT: Duration = Duration::from_secs(60);

/// When the PC last resumed, from the sleep clock.
#[derive(Clone, Debug, Default)]
pub struct WakeGrace {
    /// The last good reading of how long the PC has slept.
    slept: Option<Duration>,
    /// The resume the grace runs for, until a stream starts or the window ends.
    resumed: Option<Instant>,
}

impl WakeGrace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether to hold the PC awake `now`. `slept` is `None` when the clock
    /// could not be read, `client` the time since the last paired client's
    /// request, and `streaming` whether a stream is active or starting.
    pub fn tick(
        &mut self,
        now: Instant,
        slept: Option<Duration>,
        client: Option<Duration>,
        streaming: bool,
    ) -> bool {
        if let Some(slept) = slept
            && resumed(&mut self.slept, slept)
        {
            self.resumed = Some(now);
        }
        let Some(resumed) = self.resumed else {
            return false;
        };
        // A started stream holds the PC by itself; the grace is over.
        if streaming || now.saturating_duration_since(resumed) >= WINDOW {
            self.resumed = None;
            return false;
        }
        client.is_some_and(|since| since < CLIENT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);
    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn only_a_resume_with_a_recent_client_holds_the_pc() {
        let start = Instant::now();
        let mut grace = WakeGrace::new();
        // The first reading is no resume, however long the PC slept before.
        assert!(!grace.tick(start, Some(60 * MINUTE), Some(SECOND), false));
        // Clock jitter isn't one either.
        assert!(!grace.tick(
            start + MINUTE,
            Some(60 * MINUTE + 5 * SECOND),
            Some(SECOND),
            false
        ));
        // Resumed, nobody connecting yet.
        let resume = start + 2 * MINUTE;
        assert!(!grace.tick(resume, Some(120 * MINUTE), None, false));
        assert!(grace.tick(
            resume + 30 * SECOND,
            Some(120 * MINUTE),
            Some(SECOND),
            false
        ));
        // 60 s without a request ends the hold, a new request starts it again.
        assert!(!grace.tick(resume + 2 * MINUTE, Some(120 * MINUTE), Some(CLIENT), false));
        assert!(grace.tick(resume + 3 * MINUTE, Some(120 * MINUTE), Some(SECOND), false));
        // The window ends 5 minutes after the resume, requests or not.
        assert!(!grace.tick(resume + WINDOW, Some(120 * MINUTE), Some(SECOND), false));
        assert!(!grace.tick(
            resume + WINDOW + SECOND,
            Some(120 * MINUTE),
            Some(SECOND),
            false
        ));
    }

    #[test]
    fn a_stream_ends_it_and_an_unreadable_clock_never_starts_it() {
        let start = Instant::now();
        let mut grace = WakeGrace::new();
        grace.tick(start, Some(Duration::ZERO), None, false);
        let resume = start + MINUTE;
        assert!(grace.tick(resume, Some(30 * MINUTE), Some(SECOND), false));
        assert!(!grace.tick(resume + SECOND, Some(30 * MINUTE), Some(SECOND), true));
        // The stream ended: the grace stays over for this resume.
        assert!(!grace.tick(resume + 2 * SECOND, Some(30 * MINUTE), Some(SECOND), false));
        // A failed reading is no resume; the next good one still sees it.
        assert!(!grace.tick(resume + MINUTE, None, Some(SECOND), false));
        let next = resume + 40 * MINUTE;
        assert!(grace.tick(next, Some(60 * MINUTE), Some(SECOND), false));
    }
}
