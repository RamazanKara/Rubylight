//! Lock Windows when the last device leaves (`lock_on_last_disconnect`), like
//! Parsec's privacy mode and Apollo's "Auto lock/unlock Windows" recipe: a PC
//! left signed in after a stream ends does not stay open to whoever sits at it.
//!
//! The host locks once each time it goes from "a device is connected" to "no
//! device for [`GRACE`]", so a stream that moves to another device (Continue
//! here) or reconnects after a network drop does not lock the PC under it, and
//! a host that starts with no device never locks. A device unlocks the PC from
//! its stream: the lock screen streams like the desktop.
use std::time::{Duration, Instant};

/// The setting's key; off by default.
pub const KEY: &str = "lock_on_last_disconnect";
/// How long the PC must be without a device before it locks.
pub const GRACE: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
pub struct AutoLock {
    /// A device was connected since the last lock (or since the setting went on).
    seen: bool,
    /// When the last device left.
    left_at: Option<Instant>,
}

impl AutoLock {
    /// One look at the host: whether the setting is on and whether any device
    /// is connected or on its way. True when the PC should lock now.
    pub fn tick(&mut self, enabled: bool, devices: bool, now: Instant) -> bool {
        if !enabled {
            *self = Self::default();
            return false;
        }
        if devices {
            self.seen = true;
            self.left_at = None;
            return false;
        }
        if !self.seen {
            return false;
        }
        let left = *self.left_at.get_or_insert(now);
        if now.saturating_duration_since(left) < GRACE {
            return false;
        }
        *self = Self::default();
        true
    }
    /// When [`AutoLock::tick`] may next return true, for a loop that sleeps.
    pub fn due(&self) -> Option<Instant> {
        self.left_at.map(|left| left + GRACE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_lock_locks_once_after_the_last_device_has_been_gone_for_the_grace() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut lock = AutoLock::default();
        // A host that starts with no device never locks.
        assert!(!lock.tick(true, false, at(0)));
        assert!(!lock.tick(true, false, at(60)));
        assert_eq!(lock.due(), None);
        // A stream, then the device leaves.
        assert!(!lock.tick(true, true, at(61)));
        assert!(!lock.tick(true, false, at(70)));
        assert_eq!(lock.due(), Some(at(70) + GRACE));
        assert!(!lock.tick(true, false, at(79)));
        assert!(lock.tick(true, false, at(80)));
        // Once per departure.
        assert!(!lock.tick(true, false, at(200)));
        assert_eq!(lock.due(), None);
    }

    #[test]
    fn auto_lock_a_device_back_within_the_grace_keeps_the_pc_unlocked() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut lock = AutoLock::default();
        lock.tick(true, true, at(0));
        assert!(!lock.tick(true, false, at(1)));
        // Continue here: the next device arrives before the grace ends.
        assert!(!lock.tick(true, true, at(9)));
        assert!(!lock.tick(true, true, at(30)));
        assert!(!lock.tick(true, false, at(31)));
        assert!(!lock.tick(true, false, at(40)));
        assert!(lock.tick(true, false, at(41)));
    }

    #[test]
    fn auto_lock_off_never_locks_and_forgets_what_it_saw() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut lock = AutoLock::default();
        assert!(!lock.tick(false, true, at(0)));
        assert!(!lock.tick(false, false, at(1)));
        assert!(!lock.tick(false, false, at(100)));
        // Turned on with no device: nothing to lock for.
        assert!(!lock.tick(true, false, at(101)));
        assert!(!lock.tick(true, false, at(200)));
        // Turned off during the grace.
        lock.tick(true, true, at(201));
        lock.tick(true, false, at(202));
        assert!(!lock.tick(false, false, at(205)));
        assert!(!lock.tick(true, false, at(230)));
    }
}
