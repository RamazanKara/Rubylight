//! PyroWave-shaped packet pacing without a socket: how long each frame takes
//! on the simulated wire, and how much CPU the pacing thread spends, for three
//! wait strategies. `--load` adds one normal-priority spinning thread per
//! logical CPU in a child process, like a CPU-bound game. `--high` raises this
//! process to HIGH priority class, as a streaming host does.
//!
//! cargo run -p butterpollo-windows --example pace_probe --release -- --load --high
use butterpollo_core::network_pacing::{Pacer, overhead, pyrowave_batch_limit};
use butterpollo_windows::timing::Timer;
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::FILETIME,
    System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetThreadTimes, HIGH_PRIORITY_CLASS, SetPriorityClass,
    },
};

const RATE: u64 = 500_000_000;
const PACKET: usize = 1392;
const FRAME_BYTES: usize = 260_000;
const PERIOD: Duration = Duration::from_micros(6944);
const RUN: Duration = Duration::from_secs(4);

fn cpu() -> Duration {
    let (mut a, mut b, mut kernel, mut user) = Default::default();
    // SAFETY: The pseudo handle is valid and the four outputs outlive the call.
    unsafe {
        GetThreadTimes(GetCurrentThread(), &mut a, &mut b, &mut kernel, &mut user).unwrap();
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

/// The 2.5.1 packet wait: timer to 600 us before, then yield to the deadline.
fn spin_tail(timer: &Timer, deadline: Instant) {
    const TAIL: Duration = Duration::from_micros(600);
    if deadline.saturating_duration_since(Instant::now()) > TAIL + Duration::from_micros(100) {
        timer.until(deadline - TAIL);
    }
    while Instant::now() < deadline {
        std::thread::yield_now();
    }
}

fn run(mode: &str) -> anyhow::Result<()> {
    let _priority = butterpollo_windows::capture::Priority::new();
    let timer = Timer::new()?;
    let packets = FRAME_BYTES.div_ceil(PACKET);
    let ideal = (packets * (PACKET + overhead(false))) as f64 * 8. / RATE as f64 * 1000.;
    let mut pacer = Pacer::new(Instant::now());
    let mut wire = Vec::new();
    let started = Instant::now();
    let cpu_start = cpu();
    let mut next_frame = started;
    while started.elapsed() < RUN {
        timer.until(next_frame);
        let first = Instant::now();
        let mut remaining = packets;
        while remaining > 0 {
            if pacer.due() > Instant::now() {
                match mode {
                    "spin" => spin_tail(&timer, pacer.due()),
                    "timer" => timer.until(pacer.due()),
                    _ => timer.until_paced(pacer.due()),
                }
            }
            let count = (pyrowave_batch_limit(RATE, PACKET, false) / PACKET).min(remaining);
            remaining -= count;
            pacer.sent(Instant::now(), count * PACKET, count, false, RATE);
        }
        let took = first.elapsed();
        wire.push(took.as_secs_f64() * 1000.);
        // Like the sender, a frame that overran its period replaces the next.
        next_frame += PERIOD;
        while next_frame < Instant::now() {
            next_frame += PERIOD;
        }
    }
    let cpu = cpu() - cpu_start;
    let elapsed = started.elapsed();
    wire.sort_by(f64::total_cmp);
    let mean = wire.iter().sum::<f64>() / wire.len() as f64;
    println!(
        "{mode:6} frames {:4} ({:5.1} fps)  wire ms ideal {ideal:.2} mean {mean:.2} p95 {:.2} max {:.2}  cpu {:5.1}% of a core",
        wire.len(),
        wire.len() as f64 / elapsed.as_secs_f64(),
        wire[wire.len() * 95 / 100],
        wire[wire.len() - 1],
        cpu.as_secs_f64() / elapsed.as_secs_f64() * 100.,
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--burn") {
        let threads = std::thread::available_parallelism()?.get();
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                std::thread::spawn(|| {
                    loop {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        for worker in workers {
            let _ = worker.join();
        }
        return Ok(());
    }
    let _timer_resolution = butterpollo_windows::timing::StreamingScope::enter();
    // StreamingScope raises the class; restore normal unless --high is given.
    if !args.iter().any(|a| a == "--high") {
        // SAFETY: The pseudo handle is valid; only this probe's priority changes.
        unsafe {
            SetPriorityClass(
                GetCurrentProcess(),
                windows::Win32::System::Threading::NORMAL_PRIORITY_CLASS,
            )?
        };
    } else {
        // SAFETY: As above.
        unsafe { SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS)? };
    }
    let mut load = args
        .iter()
        .any(|a| a == "--load")
        .then(|| std::process::Command::new(&args[0]).arg("--burn").spawn())
        .transpose()?;
    std::thread::sleep(Duration::from_millis(300));
    for mode in ["spin", "timer", "paced", "spin", "timer", "paced"] {
        std::thread::spawn(move || run(mode)).join().unwrap()?;
    }
    if let Some(child) = load.as_mut() {
        child.kill()?;
    }
    Ok(())
}
