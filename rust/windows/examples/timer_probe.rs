//! How late a high-resolution waitable timer wakes for short waits.
use butterpollo_windows::timing::{Signal, Timer};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--compare") {
        return comparison::run();
    }
    // Pass --streaming to measure with the stream's timer resolution and priority.
    let _scope = std::env::args()
        .any(|a| a == "--streaming")
        .then(butterpollo_windows::timing::StreamingScope::enter);
    let timer = Timer::new()?;
    let signal = Signal::new()?;
    for wait_us in [100u64, 250, 500, 1000, 2000] {
        for (name, mode) in [("until_or_signal", 0), ("until", 1), ("until_precise", 2)] {
            let mut late = Vec::with_capacity(400);
            for _ in 0..400 {
                let deadline = Instant::now() + Duration::from_micros(wait_us);
                match mode {
                    0 => {
                        timer.until_or_signal(deadline, &signal)?;
                    }
                    1 => timer.until(deadline),
                    _ => timer.until_precise(deadline),
                }
                late.push(
                    Instant::now()
                        .saturating_duration_since(deadline)
                        .as_micros() as u64,
                );
            }
            late.sort_unstable();
            let mean = late.iter().sum::<u64>() as f64 / late.len() as f64;
            println!(
                "{name:16} wait {wait_us:5} us: late mean {mean:7.1} p50 {:5} p95 {:5} max {:5} us",
                late[late.len() / 2],
                late[late.len() * 95 / 100],
                late[late.len() - 1]
            );
        }
    }
    Ok(())
}

/// Reproduce the old spin tail beside the production timer without starting a
/// stream, changing process priority, touching displays or creating GPU work.
mod comparison {
    use super::*;
    use windows::{
        Win32::{
            Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0},
            System::Threading::{
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW,
                GetCurrentThread, GetThreadTimes, SetWaitableTimer, TIMER_ALL_ACCESS,
                WaitForMultipleObjects, WaitForSingleObject,
            },
        },
        core::PCWSTR,
    };

    struct LegacyTimer(HANDLE, HANDLE);
    impl LegacyTimer {
        fn new() -> anyhow::Result<Self> {
            // SAFETY: Both are new unnamed objects; the owner closes them in Drop.
            let timer = unsafe {
                CreateWaitableTimerExW(
                    None,
                    PCWSTR::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS.0,
                )
                .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))?
            };
            // SAFETY: Null attributes/name create an owned manual-reset event.
            match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
                Ok(signal) => Ok(Self(timer, signal)),
                Err(error) => {
                    // SAFETY: Creation above succeeded and no other owner exists.
                    unsafe {
                        let _ = CloseHandle(timer);
                    }
                    Err(error.into())
                }
            }
        }
        fn until(&self, deadline: Instant) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining > Duration::from_micros(100) {
                let ticks = -(((remaining - Duration::from_micros(50)).as_nanos() / 100)
                    .min(i64::MAX as u128) as i64);
                // SAFETY: The owner keeps the timer open; ticks is a live local.
                unsafe {
                    if SetWaitableTimer(self.0, &ticks, 0, None, None, false).is_ok() {
                        let _ = WaitForSingleObject(
                            self.0,
                            (remaining.as_millis() + 100).min(u32::MAX as u128) as u32,
                        );
                    } else {
                        std::thread::sleep(remaining);
                    }
                }
            }
            while Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }
        fn precise(&self, deadline: Instant, frame_claim: bool) -> anyhow::Result<()> {
            const SPIN: Duration = Duration::from_micros(600);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining > SPIN + Duration::from_micros(100) {
                if frame_claim {
                    let remaining = (deadline - SPIN).saturating_duration_since(Instant::now());
                    let ticks = -((remaining.as_nanos() / 100).clamp(1, i64::MAX as u128) as i64);
                    // SAFETY: Both handles are owned by self; no callback is registered.
                    unsafe {
                        SetWaitableTimer(self.0, &ticks, 0, None, None, false)?;
                        WaitForMultipleObjects(
                            &[self.0, self.1],
                            false,
                            (remaining.as_millis() + 100) as u32,
                        );
                    }
                } else {
                    self.until(deadline - SPIN);
                }
            }
            while Instant::now() < deadline {
                // SAFETY: The event remains owned; this is the original nonblocking poll.
                if frame_claim && unsafe { WaitForSingleObject(self.1, 0) } == WAIT_OBJECT_0 {
                    break;
                }
                std::thread::yield_now();
            }
            Ok(())
        }
    }
    impl Drop for LegacyTimer {
        fn drop(&mut self) {
            // SAFETY: Both objects have one owner and are closed only here.
            unsafe {
                let _ = CloseHandle(self.0);
                let _ = CloseHandle(self.1);
            }
        }
    }
    struct Resolution;
    impl Drop for Resolution {
        fn drop(&mut self) {
            // SAFETY: Balances this probe's successful timeBeginPeriod request.
            unsafe {
                windows::Win32::Media::timeEndPeriod(1);
            }
        }
    }
    fn cpu_time() -> anyhow::Result<Duration> {
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: The pseudo handle and all four initialized output buffers are valid.
        unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )?;
        }
        let ticks =
            |time: FILETIME| u64::from(time.dwLowDateTime) | (u64::from(time.dwHighDateTime) << 32);
        Ok(Duration::from_nanos(
            (ticks(kernel) + ticks(user)).saturating_mul(100),
        ))
    }
    fn cpu_cycles() -> anyhow::Result<u64> {
        // This single probe export does not need the WindowsProgramming API feature.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            #[link_name = "QueryThreadCycleTime"]
            fn query_cycles(thread: HANDLE, cycles: *mut u64) -> windows::core::BOOL;
        }
        let mut cycles = 0;
        // SAFETY: The declaration matches kernel32's documented signature; the current-thread
        // pseudo handle and initialized output are valid for the call.
        unsafe {
            query_cycles(GetCurrentThread(), &mut cycles).ok()?;
        }
        Ok(cycles)
    }
    pub(super) fn run() -> anyhow::Result<()> {
        // Match media-worker timer handling without entering StreamingScope,
        // which also changes GPU, Wi-Fi and compositor scheduling.
        butterpollo_windows::timing::disable_power_throttling();
        let _priority = butterpollo_windows::capture::Priority::new();
        // SAFETY: A process-local timer-resolution request, balanced by the guard below.
        anyhow::ensure!(
            unsafe { windows::Win32::Media::timeBeginPeriod(1) } == 0,
            "timer resolution unavailable"
        );
        let _resolution = Resolution;
        let legacy = LegacyTimer::new()?;
        let timer = Timer::new()?;
        let signal = Signal::new()?;
        println!(
            "round,workload,method,waits,wall_ms,cpu_ms,cpu_percent,cycles,cycles_per_second,late_mean_us,late_p50_us,late_p95_us,late_p99_us,late_max_us"
        );
        for round in 0..2 {
            for (name, period, claim, idle) in [
                ("packet_batches", Duration::from_micros(500), false, false),
                (
                    "claims_120fps",
                    Duration::from_secs_f64(1. / 120.),
                    true,
                    false,
                ),
                (
                    "claims_240fps",
                    Duration::from_secs_f64(1. / 240.),
                    true,
                    false,
                ),
                ("static_capture", Duration::from_micros(500), false, true),
            ] {
                for candidate in if round == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let mut late = Vec::new();
                    let cpu_start = cpu_time()?;
                    let cycles_start = cpu_cycles()?;
                    let start = Instant::now();
                    let mut deadline = start;
                    while start.elapsed() < Duration::from_secs(3) {
                        let interval = if idle && candidate {
                            butterpollo_core::capture_policy::notification_wait_interval(
                                start.elapsed(),
                                false,
                                period,
                            )
                        } else {
                            period
                        };
                        deadline = if idle {
                            Instant::now() + interval
                        } else {
                            deadline + interval
                        };
                        if candidate {
                            if claim || idle {
                                timer.until_or_signal_precise(deadline, &signal)?;
                            } else {
                                timer.until_precise(deadline);
                            }
                        } else if idle {
                            legacy.until(deadline);
                        } else {
                            legacy.precise(deadline, claim)?;
                        }
                        let now = Instant::now();
                        late.push(
                            now.saturating_duration_since(deadline).as_nanos() as f64 / 1000.,
                        );
                        // Match the packet pacer's bounded catch-up credit.
                        if !claim && !idle {
                            deadline = deadline.max(now - Duration::from_micros(500));
                        }
                    }
                    let wall = start.elapsed();
                    let cpu = cpu_time()?.saturating_sub(cpu_start);
                    let cycles = cpu_cycles()?.saturating_sub(cycles_start);
                    late.sort_by(f64::total_cmp);
                    let n = late.len();
                    println!(
                        "{round},{name},{},{n},{:.3},{:.3},{:.3},{cycles},{:.0},{:.3},{:.3},{:.3},{:.3},{:.3}",
                        if candidate { "timer" } else { "legacy" },
                        wall.as_secs_f64() * 1000.,
                        cpu.as_secs_f64() * 1000.,
                        100. * cpu.as_secs_f64() / wall.as_secs_f64(),
                        cycles as f64 / wall.as_secs_f64(),
                        late.iter().sum::<f64>() / n as f64,
                        late[n / 2],
                        late[n * 95 / 100],
                        late[n * 99 / 100],
                        late[n - 1]
                    );
                }
            }
        }
        Ok(())
    }
}
