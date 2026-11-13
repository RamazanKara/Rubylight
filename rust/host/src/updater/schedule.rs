//! When the next automatic update check is due: after the interval, or, with
//! a time of day set (`update_check_time`), once a day at that time.
//!
//! With a time set, the check runs at the first moment the host is free at
//! or after that time on this PC's clock, and an automatic installation
//! follows that check as it follows any other. A time that passed while the
//! PC was off, asleep or streaming is caught up at the next opportunity, as a
//! missed interval is; it is not skipped until the next day.
//!
//! Whether a check is due is worked out from the local clock on every tick,
//! never from a wait computed in advance, so daylight saving time, a new time
//! zone and a corrected clock apply at once.
use super::{CLOCK_SLACK, next_check_at, retry_delay};

const DAY: i64 = 86400;

/// The automatic check schedule, from the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// `update_check_interval` in seconds; 0 or less turns automatic checks
    /// off, with or without a time.
    pub interval: i64,
    /// `update_check_time` as seconds after local midnight, when it is set.
    /// The interval then only says whether automatic checks are on.
    pub time: Option<u32>,
}

/// When the next automatic check is due, as Unix seconds: `None` when
/// automatic checks are off, and a moment not after `now` when one is due.
/// `checked_at` is the last check (0 for none) and `failures` how many checks
/// failed in a row. `wall` is what this PC's clock shows at a moment, as Unix
/// seconds as if that clock were UTC.
pub fn next_at(
    schedule: Schedule,
    checked_at: u64,
    failures: u64,
    now: u64,
    wall: &dyn Fn(u64) -> i64,
) -> Option<u64> {
    let Some(time) = schedule.time.filter(|_| schedule.interval > 0) else {
        return next_check_at(schedule.interval, checked_at, failures, now);
    };
    Some(next_daily_at(time, checked_at, failures, now, wall))
}

fn next_daily_at(
    time: u32,
    checked_at: u64,
    failures: u64,
    now: u64,
    wall: &dyn Fn(u64) -> i64,
) -> u64 {
    // Never checked, or the clock was set back well past the last check, as
    // with an interval.
    if checked_at == 0 || checked_at > now + CLOCK_SLACK {
        return 0;
    }
    let time = i64::from(time).min(DAY - 1);
    let local = wall(now);
    // The latest moment this PC's clock showed the time.
    let today = local.div_euclid(DAY) * DAY + time;
    let slot = if today > local { today - DAY } else { today };
    let regular = if wall(checked_at) < slot {
        // Due since then. Compared on the local clock, so an hour that the
        // clock repeats in autumn cannot bring a second check, and a time in
        // the hour it skips in spring is due when the clock jumps past it.
        now.saturating_sub((local - slot) as u64)
    } else {
        moment_of(slot + DAY, now, local, wall)
    };
    if failures == 0 {
        return regular;
    }
    // A failed check is retried soon, as with an interval.
    regular.min(checked_at.saturating_add(retry_delay(failures, DAY as u64)))
}

/// The first moment after `now` at which this PC's clock shows `target` or
/// later, for the console's "Next check". Exact unless `target` is in the
/// hour the clock skips in spring: then it may name a moment up to an hour
/// after the clock jumps, while the check itself is due at the jump.
fn moment_of(target: i64, now: u64, local: i64, wall: &dyn Fn(u64) -> i64) -> u64 {
    let now = now as i64;
    // By the clock's offset now, then by its offset at that moment.
    let first = now + (target - local);
    let second = first - (wall(first.max(0) as u64) - target);
    [first.min(second), first.max(second)]
        .into_iter()
        .find(|&at| at > now && wall(at as u64) >= target)
        .unwrap_or(first)
        .max(now + 1) as u64
}

#[cfg(test)]
mod tests;
