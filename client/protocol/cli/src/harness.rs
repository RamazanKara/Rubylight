//! CLI test harness utilities: clocks, counters, options, mouse-look patterns, and Frame Warp logging.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use nightjar_core::frame_warp::FrameWarp;
use nightjar_core::input::InputError;
use nightjar_core::session::InputHandle;
use nightjar_core::timing_record as record;

use crate::report::{self, IdrProbe, Measurements};

pub(crate) const MAX_MEASUREMENTS: usize = 3600 * 240;
pub(crate) const MAX_IDR_PROBES: i64 = 2000;
pub(crate) const CLOCK_PROBE_INTERVAL_US: u64 = 250_000;
pub(crate) const SERVER_CODEC_MODE_SUPPORT: u32 = 0x30301;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

pub(crate) fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub(crate) fn env_is(name: &str, value: &str) -> bool {
    env(name).as_deref() == Some(value)
}

/// C's atoi: leading whitespace, a sign and digits; anything else ends the number.
pub(crate) fn atoi(text: &str) -> i64 {
    let text = text.trim_start();
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    sign * digits[..end].parse::<i64>().unwrap_or(0)
}

/// C's atof for the plain decimal numbers the harness passes.
pub(crate) fn atof(text: &str) -> f64 {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(text.len(), |(i, _)| i);
    text[..end].parse().unwrap_or(0.0)
}

/// Milliseconds on the clock the motion probe stamps its barcode with: the performance
/// counter on Windows.
#[cfg(windows)]
pub(crate) mod clock {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(count: *mut i64) -> i32;
        fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    }

    pub fn frequency() -> f64 {
        let mut f = 0i64;
        // SAFETY: a valid out pointer.
        unsafe { QueryPerformanceFrequency(&mut f) };
        f as f64
    }

    pub fn ms() -> f64 {
        let mut n = 0i64;
        // SAFETY: a valid out pointer.
        unsafe { QueryPerformanceCounter(&mut n) };
        n as f64 * 1000.0 / frequency()
    }

    /// Whole microseconds on the same counter, converted as the probes convert theirs.
    pub fn us() -> i64 {
        let (mut n, mut f) = (0i64, 0i64);
        // SAFETY: valid out pointers.
        unsafe {
            QueryPerformanceCounter(&mut n);
            QueryPerformanceFrequency(&mut f);
        }
        (i128::from(n) * 1_000_000 / i128::from(f.max(1))) as i64
    }
}

#[cfg(not(windows))]
pub(crate) mod clock {
    pub fn ms() -> f64 {
        static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        EPOCH
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
            * 1000.0
    }

    pub fn us() -> i64 {
        (ms() * 1000.0) as i64
    }
}

/// Counters shared by the decoder, the renderer, the listener and the main thread.
#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) frames: AtomicI32,
    pub(crate) decoded_frames: AtomicI32,
    pub(crate) audio_packets: AtomicI32,
    pub(crate) failures: AtomicI32,
    pub(crate) detailed_frames: AtomicI32,
    pub(crate) ended: AtomicBool,
    pub(crate) hdr_notifications: AtomicI32,
    pub(crate) hdr_enabled_notifications: AtomicI32,
    pub(crate) hdr_disabled_notifications: AtomicI32,
    pub(crate) hdr_control_mismatches: AtomicI32,
    pub(crate) hdr_invalid_metadata: AtomicI32,
    pub(crate) roamed: AtomicI32,
    pub(crate) frame_warp_rows: AtomicU64,
}

impl Counters {
    pub(crate) fn fail(&self) {
        self.failures.fetch_add(1, Ordering::SeqCst);
    }
}

/// The run's settings from the arguments and environment.
#[derive(Clone)]
pub(crate) struct Options {
    pub(crate) requested_format: u32,
    pub(crate) requested_hdr: bool,
    pub(crate) requested_width: i32,
    pub(crate) requested_height: i32,
    pub(crate) decoder_threads: i32,
    pub(crate) hardware: Option<String>,
    pub(crate) hardware_device: Option<String>,
    pub(crate) barcode_bottom: bool,
    pub(crate) barcode_scale: f64,
    pub(crate) barcode_left: f64,
    pub(crate) barcode_bottom_margin: f64,
    pub(crate) expected_hdr_control: i32,
    pub(crate) idr_probe_count: i64,
    pub(crate) first_frame_dump: Option<String>,
    pub(crate) bitstream: Option<String>,
    pub(crate) frame_dump: Option<String>,
    pub(crate) frame_dump_at: i32,
    pub(crate) host: Option<String>,
    pub(crate) audio_tone: bool,
}

pub(crate) struct State {
    pub(crate) options: Options,
    pub(crate) counters: Counters,
    pub(crate) measurements: Mutex<Measurements>,
    pub(crate) probe: Mutex<IdrProbe>,
    pub(crate) audio: Mutex<report::AudioSignal>,
    /// Host timing reports and clock sync for the timing CSV's host columns; `None` without one.
    pub(crate) record: Option<Mutex<record::Record>>,
}

/// The barcode's age in milliseconds, comparable only on the same Windows machine (both
/// processes read the performance counter): None elsewhere.
pub(crate) fn picture_age(host: Option<&str>, ticks: u64) -> Option<f64> {
    let local = host.is_none_or(|h| h.starts_with("127.") || h == "::1");
    #[cfg(windows)]
    if local {
        return Some(clock::ms() - ticks as f64 * 1000.0 / clock::frequency());
    }
    let _ = (local, ticks);
    None
}

/// The relative mouse counts this client has handed to the input queue, x and y packed in one
/// atomic so a reader never sees one axis moved without the other.
#[derive(Default)]
pub(crate) struct SentMouse(pub(crate) AtomicU64);

impl SentMouse {
    pub(crate) fn get(&self) -> (i32, i32) {
        let packed = self.0.load(Ordering::SeqCst);
        (packed as u32 as i32, (packed >> 32) as u32 as i32)
    }

    /// Adds a move to the counts. One thread sends at a time.
    pub(crate) fn add(&self, dx: i16, dy: i16) {
        let (x, y) = self.get();
        let x = x.wrapping_add(i32::from(dx)) as u32;
        let y = y.wrapping_add(i32::from(dy)) as u32;
        self.0
            .store(u64::from(x) | (u64::from(y) << 32), Ordering::SeqCst);
    }

    /// Queues a relative move and counts it while the queue is held, so the count is what the
    /// queue was handed. A full queue still keeps the move in its pending batch (it adds the
    /// move before offering the flush), so only a stopped one counts nothing.
    pub(crate) fn send(&self, input: &InputHandle, dx: i16, dy: i16) {
        input.input(|q| {
            if q.send_mouse_move(dx, dy) != Err(InputError::Stopped) {
                self.add(dx, dy);
            }
        });
    }
}

/// `NIGHTJAR_CLI_FRAME_WARP`'s CSV header: a row per decoded picture.
pub(crate) const FRAME_WARP_HEADER: &str = "wire_frame,barcode_seq,decode_qpc_us,sent_x,sent_y,active,shift_x,shift_y,locked,mouse_gain_x,mouse_gain_y,lag_ms,focal";

/// `NIGHTJAR_CLI_FRAME_WARP`: the shift Frame Warp gives each decoded picture as if it were
/// drawn at once, with what this client had sent when it asked. `wire_frame` is the picture's
/// frame number (the host's, as the timing CSV's), `barcode_seq` the motion or look probe's
/// sequence (0 without one), `decode_qpc_us` the performance counter in microseconds,
/// `sent_x`/`sent_y` the counts handed to the input queue so far (`SentMouse`), `shift_x`/
/// `shift_y` picture pixels at the picture's centre, `locked` the host's model present with its
/// mouse fit locked, and the model's mouse gain, lag and focal length (picture pixels, 0 for a
/// picture that moves alike everywhere) empty until it has sent one.
pub(crate) struct WarpLog {
    pub(crate) warp: Arc<FrameWarp>,
    pub(crate) sent: Arc<SentMouse>,
    pub(crate) csv: BufWriter<File>,
}

impl WarpLog {
    pub(crate) fn row(&mut self, frame: u32, sequence: u32) -> std::io::Result<()> {
        let (sent_x, sent_y) = self.sent.get();
        let warp = self.warp.warp(frame);
        let decode_us = clock::us();
        let model = self.warp.status().model;
        let locked = model.is_some_and(|m| m.mouse_locked);
        let (gain_x, gain_y, lag_ms, focal) = match model {
            Some(m) => (
                format!("{:.6}", m.mouse_gain[0]),
                format!("{:.6}", m.mouse_gain[1]),
                format!("{:.3}", f64::from(m.lag_us) / 1000.0),
                format!("{:.2}", m.focal),
            ),
            None => Default::default(),
        };
        writeln!(
            self.csv,
            "{frame},{sequence},{decode_us},{sent_x},{sent_y},{},{:.3},{:.3},{},{gain_x},{gain_y},{lag_ms},{focal}",
            u8::from(warp.active),
            warp.pixels[0],
            warp.pixels[1],
            u8::from(locked)
        )
    }
}

/// How often `NIGHTJAR_CLI_LOOK` sends.
pub(crate) const LOOK_STEP: Duration = Duration::from_millis(4);
/// A look segment's x velocities, counts per second.
pub(crate) const LOOK_X_SPEEDS: [i32; 9] = [-2400, -1500, -800, -300, 0, 300, 800, 1500, 2400];
/// A look segment's largest y speed, counts per second.
const LOOK_Y_SPEED: u64 = 300;
/// How far the cumulative y may go from 0, counts.
pub(crate) const LOOK_Y_LIMIT: f64 = 250.0;

/// `NIGHTJAR_CLI_LOOK=<seed>`'s mouse-look pattern: segments of 250 to 1200 ms, each at an x
/// velocity from [`LOOK_X_SPEEDS`] and a y velocity of 0 to ±300 counts/s, every fourth (the
/// 4th, 8th, ...) a pause, drawn by xorshift from the seed. A segment starting at ±250 counts
/// of y or past them turns y back toward 0, and none goes past them. The counts sent are the
/// exact position rounded less what was sent before, so fractions carry over and the sums
/// track the velocities exactly however late a step comes.
#[derive(Debug)]
pub(crate) struct LookPattern {
    pub(crate) rng: u64,
    /// Segments begun.
    pub(crate) segments: u64,
    pub(crate) start_us: u64,
    pub(crate) end_us: u64,
    /// Counts per second.
    pub(crate) velocity: (f64, f64),
    /// The position at the segment's start.
    pub(crate) origin: (f64, f64),
    pub(crate) sent: (i64, i64),
}

impl LookPattern {
    pub(crate) fn new(seed: u64) -> Self {
        // xorshift never leaves 0.
        let rng = match seed ^ 0x9E37_79B9_7F4A_7C15 {
            0 => 1,
            rng => rng,
        };
        let mut pattern = Self {
            rng,
            segments: 0,
            start_us: 0,
            end_us: 0,
            velocity: (0.0, 0.0),
            origin: (0.0, 0.0),
            sent: (0, 0),
        };
        pattern.begin();
        pattern
    }

    /// xorshift64.
    pub(crate) fn random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// The exact position at `at_us` on the current segment.
    pub(crate) fn position(&self, at_us: u64) -> (f64, f64) {
        let seconds = at_us.saturating_sub(self.start_us) as f64 / 1e6;
        (
            self.origin.0 + self.velocity.0 * seconds,
            self.origin.1 + self.velocity.1 * seconds,
        )
    }

    /// Ends the current segment and begins the next where it ends.
    pub(crate) fn begin(&mut self) {
        let start_us = self.end_us;
        let origin = self.position(start_us);
        let duration_us = (250 + self.random() % 951) * 1000;
        let x = LOOK_X_SPEEDS[(self.random() % LOOK_X_SPEEDS.len() as u64) as usize];
        let speed = (self.random() % (LOOK_Y_SPEED + 1)) as f64;
        let mut y = if self.random() & 1 == 0 {
            speed
        } else {
            -speed
        };
        if origin.1.abs() >= LOOK_Y_LIMIT - 0.5 {
            y = -origin.1.signum() * speed;
        }
        let seconds = duration_us as f64 / 1e6;
        let end_y = (origin.1 + y * seconds).clamp(-LOOK_Y_LIMIT, LOOK_Y_LIMIT);
        self.velocity = if self.segments % 4 == 3 {
            (0.0, 0.0)
        } else {
            (f64::from(x), (end_y - origin.1) / seconds)
        };
        self.origin = origin;
        self.start_us = start_us;
        self.end_us = start_us + duration_us;
        self.segments += 1;
    }

    /// The counts to send at `at_us`, microseconds from the pattern's start (never going back).
    pub(crate) fn step(&mut self, at_us: u64) -> (i16, i16) {
        while at_us >= self.end_us {
            self.begin();
        }
        let (x, y) = self.position(at_us);
        let take = |target: f64, sent: &mut i64| {
            let delta =
                (target.round() as i64 - *sent).clamp(i64::from(i16::MIN), i64::from(i16::MAX));
            *sent += delta;
            delta as i16
        };
        (take(x, &mut self.sent.0), take(y, &mut self.sent.1))
    }
}

/// `--roam-at <seconds> [--roam-ip <address>]`: a simulated roam that many seconds into the
/// stream, the media (and with `--roam-ip` the control) sockets moving to new ports on that
/// address. The run fails unless the roam completes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RoamAt {
    pub(crate) seconds: f64,
    pub(crate) ip: Option<IpAddr>,
}

/// Takes the roam options out of `args`, leaving the positional arguments.
pub(crate) fn take_roam_options(args: &mut Vec<String>) -> Result<Option<RoamAt>, String> {
    let mut take = |name: &str| -> Result<Option<String>, String> {
        let Some(at) = args.iter().position(|a| a == name) else {
            return Ok(None);
        };
        if at + 1 >= args.len() {
            return Err(format!("{name} needs a value"));
        }
        let value = args.remove(at + 1);
        args.remove(at);
        Ok(Some(value))
    };
    let seconds = take("--roam-at")?;
    let ip = take("--roam-ip")?;
    let Some(seconds) = seconds else {
        return match ip {
            Some(_) => Err("--roam-ip needs --roam-at".into()),
            None => Ok(None),
        };
    };
    let seconds = match seconds.parse::<f64>() {
        Ok(s) if s.is_finite() && s >= 0.0 => s,
        _ => return Err(format!("--roam-at must be a number of seconds, not {seconds}")),
    };
    let ip = ip
        .map(|ip| {
            ip.parse::<IpAddr>()
                .map_err(|_| format!("--roam-ip must be an IP address, not {ip}"))
        })
        .transpose()?;
    Ok(Some(RoamAt { seconds, ip }))
}
