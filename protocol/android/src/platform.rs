//! Logging to logcat and the CLOCK_MONOTONIC clock that Java's System.nanoTime() and
//! MediaCodec use.

use std::time::Instant;

/// Microseconds of CLOCK_MONOTONIC: the clock of the clock sync probes and of
/// `presentClockSchedule`'s ready times.
pub fn monotonic_us() -> u64 {
    (monotonic_ns() / 1000) as u64
}

#[cfg(unix)]
// time_t and long are 32 bits on armeabi-v7a and x86.
#[allow(clippy::useless_conversion)]
pub fn monotonic_ns() -> i64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    i64::from(now.tv_sec) * 1_000_000_000 + i64::from(now.tv_nsec)
}

#[cfg(not(unix))]
pub fn monotonic_ns() -> i64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as i64
}

/// CLOCK_MONOTONIC microseconds of an earlier `Instant` (Rust's Instant is CLOCK_MONOTONIC
/// on Linux and Android, so only the age is needed).
pub fn monotonic_us_at(instant: Instant) -> u64 {
    monotonic_us().saturating_sub(instant.elapsed().as_micros() as u64)
}

#[derive(Clone, Copy)]
pub enum Level {
    Info = 4,
    Warn = 5,
    Error = 6,
}

#[cfg(target_os = "android")]
pub fn log(level: Level, message: &str) {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};
    extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
    let text = CString::new(message.replace('\0', " ")).unwrap_or_default();
    unsafe { __android_log_write(level as c_int, c"nightjar-core".as_ptr(), text.as_ptr()) };
}

#[cfg(not(target_os = "android"))]
pub fn log(level: Level, message: &str) {
    let _ = level as i32;
    eprintln!("nightjar-core: {message}");
}

pub fn info(message: &str) {
    log(Level::Info, message);
}

pub fn warn(message: &str) {
    log(Level::Warn, message);
}

pub fn error(message: &str) {
    log(Level::Error, message);
}
