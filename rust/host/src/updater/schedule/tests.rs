use super::*;

const HOUR: u64 = 3600;
/// Midnight UTC on 2026-03-28, a Saturday.
const SATURDAY: u64 = 20540 * 86400;
/// Central European Summer Time in 2026: from 01:00 UTC on March 29 to
/// 01:00 UTC on October 25.
const SPRING: u64 = 20541 * 86400 + HOUR;
const AUTUMN: u64 = 20751 * 86400 + HOUR;

fn utc(at: u64) -> i64 {
    at as i64
}
/// Berlin's clock.
fn berlin(at: u64) -> i64 {
    at as i64
        + if (SPRING..AUTUMN).contains(&at) {
            7200
        } else {
            3600
        }
}
/// Tokyo's clock: nine hours ahead, so its day turns at 15:00 UTC.
fn tokyo(at: u64) -> i64 {
    at as i64 + 9 * 3600
}
fn clock(hours: u64, minutes: u64) -> u32 {
    (hours * HOUR + minutes * 60) as u32
}
fn daily(time: u32) -> Schedule {
    Schedule {
        interval: 86400,
        time: Some(time),
    }
}
fn due(schedule: Schedule, checked_at: u64, now: u64, wall: &dyn Fn(u64) -> i64) -> bool {
    next_at(schedule, checked_at, 0, now, wall).is_some_and(|at| now >= at)
}

#[test]
fn without_a_time_the_interval_decides_as_before() {
    let t = SATURDAY + 10 * HOUR;
    for interval in [-1, 0, 60, 3600, 86400, 7 * 86400] {
        for checked_at in [0, t - 3 * 86400, t - 7200, t - 100, t + 1, t + 5000] {
            for failures in [0, 1, 5] {
                assert_eq!(
                    next_at(
                        Schedule {
                            interval,
                            time: None
                        },
                        checked_at,
                        failures,
                        t,
                        &berlin
                    ),
                    next_check_at(interval, checked_at, failures, t),
                    "{interval} {checked_at} {failures}"
                );
            }
        }
    }
}

#[test]
fn an_interval_of_zero_turns_the_daily_check_off_too() {
    let t = SATURDAY + 10 * HOUR;
    for interval in [0, -1] {
        let schedule = Schedule {
            interval,
            time: Some(clock(3, 0)),
        };
        assert_eq!(next_at(schedule, t - 3 * 86400, 0, t, &utc), None);
        assert_eq!(next_at(schedule, 0, 0, t, &utc), None);
    }
}

#[test]
fn the_check_is_due_at_the_time_and_not_before() {
    let schedule = daily(clock(3, 0));
    // Checked yesterday at 03:00.
    let checked_at = SATURDAY - 86400 + 3 * HOUR + 5;
    let three = SATURDAY + 3 * HOUR;
    assert!(!due(schedule, checked_at, three - 3 * HOUR, &utc));
    assert!(!due(schedule, checked_at, three - 1, &utc));
    assert_eq!(
        next_at(schedule, checked_at, 0, three - 1, &utc),
        Some(three)
    );
    assert!(due(schedule, checked_at, three, &utc));
    assert!(due(schedule, checked_at, three + 1, &utc));
    // The interval's length does not move it: the time decides.
    for interval in [3600, 7 * 86400] {
        let schedule = Schedule {
            interval,
            ..schedule
        };
        assert!(!due(schedule, checked_at, three - 1, &utc));
        assert!(due(schedule, checked_at, three, &utc));
    }
}

#[test]
fn the_check_runs_once_a_day() {
    let schedule = daily(clock(3, 0));
    let three = SATURDAY + 3 * HOUR;
    // Today's check finished a few seconds after 03:00.
    let checked_at = three + 4;
    for later in [5, 60, HOUR, 12 * HOUR, 24 * HOUR - 1] {
        assert!(!due(schedule, checked_at, three + later, &utc), "{later}");
        assert_eq!(
            next_at(schedule, checked_at, 0, three + later, &utc),
            Some(three + 86400),
            "{later}"
        );
    }
    assert!(due(schedule, checked_at, three + 86400, &utc));
    // A check made by hand in the afternoon counts as today's as well.
    let by_hand = three + 11 * HOUR;
    assert!(!due(schedule, by_hand, three + 12 * HOUR, &utc));
    assert!(due(schedule, by_hand, three + 86400, &utc));
    // One made by hand before the time does not replace the daily check.
    let early = three - HOUR;
    assert!(!due(schedule, early, three - 60, &utc));
    assert!(due(schedule, early, three, &utc));
}

#[test]
fn a_time_missed_while_the_pc_was_off_or_streaming_is_caught_up() {
    let schedule = daily(clock(3, 0));
    let three = SATURDAY + 3 * HOUR;
    // Checked yesterday at 03:00; the PC is next free at 18:00 today.
    let checked_at = three - 86400 + 2;
    let evening = three + 15 * HOUR;
    assert!(due(schedule, checked_at, evening, &utc));
    // Due since 03:00, which the console shows as "Shortly".
    assert_eq!(next_at(schedule, checked_at, 0, evening, &utc), Some(three));
    // Caught up at 18:00, the next one is at 03:00 again, not a day later.
    assert_eq!(
        next_at(schedule, evening + 3, 0, evening + 4, &utc),
        Some(three + 86400)
    );
    // Off for a week: one check, not seven.
    let week = three + 7 * 86400 + 5 * HOUR;
    assert!(due(schedule, checked_at, week, &utc));
    assert!(!due(schedule, week + 2, week + 3, &utc));
    // Never checked: at once, as with an interval.
    assert_eq!(next_at(schedule, 0, 0, evening, &utc), Some(0));
}

#[test]
fn the_time_wraps_around_midnight() {
    // Just before midnight.
    let late = daily(clock(23, 30));
    let half_past = SATURDAY + 23 * HOUR + 1800;
    let checked_at = half_past - 86400 + 1;
    assert!(!due(late, checked_at, SATURDAY + 60, &utc));
    assert!(!due(late, checked_at, half_past - 1, &utc));
    assert!(due(late, checked_at, half_past, &utc));
    // Not done before midnight: still due after it, for yesterday's time.
    assert!(due(late, checked_at, half_past + HOUR, &utc));
    assert!(!due(late, half_past + HOUR, half_past + 2 * HOUR, &utc));
    assert_eq!(
        next_at(late, half_past + HOUR, 0, half_past + 2 * HOUR, &utc),
        Some(half_past + 86400)
    );
    // Midnight itself.
    let midnight = daily(clock(0, 0));
    let checked_at = SATURDAY + 1;
    assert!(!due(midnight, checked_at, SATURDAY + 86400 - 1, &utc));
    assert!(due(midnight, checked_at, SATURDAY + 86400, &utc));
    // The local day, not the UTC one: 00:10 in Tokyo is 15:10 UTC.
    let tokyo_night = daily(clock(0, 10));
    let ten_past = SATURDAY + 15 * HOUR + 600;
    let checked_at = ten_past - 86400 + 1;
    assert!(!due(tokyo_night, checked_at, ten_past - 1, &tokyo));
    assert!(due(tokyo_night, checked_at, ten_past, &tokyo));
    assert_eq!(
        next_at(tokyo_night, ten_past + 1, 0, ten_past + 2, &tokyo),
        Some(ten_past + 86400)
    );
}

#[test]
fn the_clock_going_forward_in_spring_keeps_the_local_time() {
    // 03:00 on the Saturday is 02:00 UTC; on the Sunday the clock jumps
    // from 02:00 to 03:00 at 01:00 UTC, so 03:00 is 01:00 UTC.
    let schedule = daily(clock(3, 0));
    let saturday = SATURDAY + 2 * HOUR;
    assert!(due(schedule, saturday - 86400 + 1, saturday, &berlin));
    let checked_at = saturday + 3;
    assert_eq!(
        next_at(schedule, checked_at, 0, saturday + 10 * HOUR, &berlin),
        Some(SPRING)
    );
    assert!(!due(schedule, checked_at, SPRING - 1, &berlin));
    assert!(due(schedule, checked_at, SPRING, &berlin));
    // And on the Monday 03:00 is 23 hours later again, at 01:00 UTC.
    assert_eq!(
        next_at(schedule, SPRING + 2, 0, SPRING + 3, &berlin),
        Some(SPRING + 86400)
    );

    // 02:30 does not exist on that Sunday: the check runs when the clock
    // jumps past it, once.
    let skipped = daily(clock(2, 30));
    let checked_at = SATURDAY + HOUR + 1800 + 2;
    assert!(!due(skipped, checked_at, SPRING - 1, &berlin));
    assert!(due(skipped, checked_at, SPRING, &berlin));
    assert!(!due(skipped, SPRING + 2, SPRING + 3, &berlin));
    assert!(!due(skipped, SPRING + 2, SPRING + 20 * HOUR, &berlin));
    // The next day's 02:30 is 00:30 UTC.
    assert_eq!(
        next_at(skipped, SPRING + 2, 0, SPRING + 3, &berlin),
        Some(SPRING + 86400 - 1800)
    );
    // Before the jump the console's "Next check" is within the hour
    // after it, and never in the past while the check is not due.
    let before = SPRING - 2 * HOUR;
    let shown = next_at(skipped, checked_at, 0, before, &berlin).unwrap();
    assert!((SPRING..=SPRING + HOUR).contains(&shown), "{shown}");
}

#[test]
fn the_clock_going_back_in_autumn_does_not_check_twice() {
    // On October 25 the clock shows 02:00 to 03:00 twice: 02:30 is
    // 00:30 UTC (summer time) and again 01:30 UTC.
    let schedule = daily(clock(2, 30));
    let first = AUTUMN - 1800;
    let second = AUTUMN + 1800;
    let checked_at = first - 86400 + 2;
    assert_eq!(
        next_at(schedule, checked_at, 0, first - 5 * HOUR, &berlin),
        Some(first)
    );
    assert!(!due(schedule, checked_at, first - 1, &berlin));
    assert!(due(schedule, checked_at, first, &berlin));
    let checked_at = first + 2;
    for now in [
        first + 3,
        AUTUMN - 1,
        AUTUMN,
        second - 1,
        second,
        second + HOUR,
    ] {
        assert!(!due(schedule, checked_at, now, &berlin), "{now}");
        let next = next_at(schedule, checked_at, 0, now, &berlin).unwrap();
        assert!(next > now, "{now}: {next}");
    }
    // Monday's 02:30 is winter time: 01:30 UTC, 25 hours after the first.
    assert_eq!(
        next_at(schedule, checked_at, 0, second + HOUR, &berlin),
        Some(second + 86400)
    );
    assert!(due(schedule, checked_at, second + 86400, &berlin));

    // A time after the repeated hour moves an hour later in UTC.
    let later = daily(clock(3, 30));
    let saturday = AUTUMN - 86400 + 1800;
    assert!(due(later, saturday - 86400 + 1, saturday, &berlin));
    assert_eq!(
        next_at(later, saturday + 2, 0, saturday + 3, &berlin),
        Some(AUTUMN + HOUR + 1800)
    );
    assert!(!due(later, saturday + 2, AUTUMN + HOUR + 1799, &berlin));
    assert!(due(later, saturday + 2, AUTUMN + HOUR + 1800, &berlin));
}

#[test]
fn a_changed_clock_or_time_applies_at_the_next_tick() {
    let three = SATURDAY + 3 * HOUR;
    let checked_at = three + 4;
    // The user moves the time from 03:00 to 05:00 at 04:00: the check at
    // 03:00 was before today's new time, so it runs at 05:00 today.
    assert!(!due(daily(clock(5, 0)), checked_at, three + HOUR, &utc));
    assert_eq!(
        next_at(daily(clock(5, 0)), checked_at, 0, three + HOUR, &utc),
        Some(three + 2 * HOUR)
    );
    // To 01:00 instead, which today's check was after: tomorrow at 01:00.
    assert_eq!(
        next_at(daily(clock(1, 0)), checked_at, 0, three + HOUR, &utc),
        Some(three + 86400 - 2 * HOUR)
    );
    // The clock is set a day ahead: due.
    assert!(due(
        daily(clock(3, 0)),
        checked_at,
        three + 86400 + 60,
        &utc
    ));
    // Set back well past the last check: due at once, as with an
    // interval; a step of a few minutes (time sync) is not.
    assert_eq!(
        next_at(daily(clock(3, 0)), checked_at, 0, checked_at - 5000, &utc),
        Some(0)
    );
    assert!(!due(daily(clock(3, 0)), checked_at, checked_at - 200, &utc));
    // The PC moves to another time zone: its new clock decides. 04:00 UTC
    // is 13:00 in Tokyo, where the last check was at 12:00, before 12:30.
    assert!(!due(daily(clock(12, 30)), checked_at, three + HOUR, &utc));
    assert!(due(daily(clock(12, 30)), checked_at, three + HOUR, &tokyo));
}

#[test]
fn a_failed_daily_check_is_retried_within_minutes() {
    let schedule = daily(clock(3, 0));
    let three = SATURDAY + 3 * HOUR;
    let failed_at = three + 30;
    let retry = |failures| next_at(schedule, failed_at, failures, failed_at + 1, &utc).unwrap();
    assert_eq!(retry(1), failed_at + 120);
    assert_eq!(retry(2), failed_at + 240);
    // Backed off no further than the next day's time.
    assert_eq!(retry(20), three + 86400);
    assert_eq!(retry(u64::MAX), three + 86400);
    assert_eq!(retry(0), three + 86400);
}
