//! What this PC's clock shows at a moment in time.

/// The wall-clock time on this PC at `unix` (Unix seconds), as Unix seconds
/// as if that clock were UTC, by the time zone's rule on that date; the
/// opposite of [`crate::playnite::local_to_utc`]. `None` when Windows cannot
/// convert the moment.
pub fn utc_to_local(unix: u64) -> Option<i64> {
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::{
            FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTimeEx,
        },
    };
    // FILETIME counts 100 ns from 1601.
    const EPOCH: u64 = 11_644_473_600;
    let ticks = unix.checked_add(EPOCH)?.checked_mul(10_000_000)?;
    let file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let (mut utc, mut local, mut out) = (
        SYSTEMTIME::default(),
        SYSTEMTIME::default(),
        FILETIME::default(),
    );
    // SAFETY: every pointer is to a local that outlives the call.
    unsafe {
        FileTimeToSystemTime(&file, &mut utc).ok()?;
        // The Ex form applies the daylight saving rule of the moment's own
        // year, not this year's.
        SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local).ok()?;
        SystemTimeToFileTime(&local, &mut out).ok()?;
    }
    let ticks = (u64::from(out.dwHighDateTime) << 32) | u64::from(out.dwLowDateTime);
    Some(i64::try_from(ticks / 10_000_000).ok()? - EPOCH as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_clock_is_within_a_day_of_utc_and_converts_back() {
        // Noon UTC on 2026-01-15 and 2026-07-15: winter and summer time.
        for unix in [1_768_478_400_u64, 1_784_116_800] {
            let local = utc_to_local(unix).unwrap();
            assert!((local - unix as i64).abs() <= 14 * 3600, "{unix}: {local}");
            // The way back uses this year's daylight saving rule for every
            // year, so allow it an hour.
            let back = crate::playnite::local_to_utc(local).unwrap();
            assert!((back - unix as i64).abs() <= 3600, "{unix}: {back}");
        }
        assert_eq!(utc_to_local(u64::MAX), None);
    }
}
