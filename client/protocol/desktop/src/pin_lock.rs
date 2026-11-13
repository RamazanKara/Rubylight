//! The PIN lock, ported from Nightjar Android's `com.limelight.pin` (PinHasher, PinThrottle,
//! PinStore, PinGate): the same modes, grace time, backoff and stored hash format.
//!
//! Two things can be locked: opening Nightjar (the launcher, until the PIN is entered; again after
//! the window was minimized for longer than the grace time) and starting a stream (every start, or
//! the starts on selected PCs). A stream that reconnects is not a new start and is not gated.
//!
//! The lock keeps its own file, `pin_lock.json`, beside the settings, so resetting settings leaves
//! it alone. Only the salted PBKDF2 hash is kept, never the PIN.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

pub const MIN_DIGITS: usize = 4;
pub const MAX_DIGITS: usize = 8;
pub const DEFAULT_GRACE_MINUTES: u32 = 10;
pub const GRACE_CHOICES: [u32; 5] = [0, 5, 10, 30, 60];
const FILE: &str = "pin_lock.json";

/// Salted PBKDF2-HMAC-SHA256, stored as `v1$iterations$salt$key` with both binary parts in
/// standard Base64, as Android's PinHasher writes it.
pub mod hasher {
    use super::*;
    use std::num::NonZeroU32;
    use ring::pbkdf2;
    use ring::rand::{SecureRandom, SystemRandom};

    pub const KEY_BYTES: usize = 32;
    pub const SALT_BYTES: usize = 16;
    /// The iteration count is measured on this computer so a check costs about this long.
    pub const TARGET_MS: u64 = 150;
    pub const MIN_ITERATIONS: u32 = 20_000;
    pub const MAX_ITERATIONS: u32 = 600_000;
    const PROBE_ITERATIONS: u32 = 10_000;
    /// A stored count above this is corrupt or hostile, not something this module wrote.
    const MAX_STORED_ITERATIONS: u32 = 5_000_000;
    const VERSION: &str = "v1";
    const ALGORITHM: pbkdf2::Algorithm = pbkdf2::PBKDF2_HMAC_SHA256;

    fn derive(pin: &[u8], salt: &[u8], iterations: NonZeroU32, out: &mut [u8]) {
        pbkdf2::derive(ALGORITHM, iterations, salt, pin, out);
    }

    /// A new record for `pin` with a fresh salt.
    pub fn create(pin: &[u8], iterations: u32) -> Result<String, String> {
        if pin.is_empty() {
            return Err("empty PIN".into());
        }
        let iterations = NonZeroU32::new(iterations).ok_or("no iterations")?;
        let mut salt = [0u8; SALT_BYTES];
        SystemRandom::new().fill(&mut salt).map_err(|_| "the system's random source failed".to_owned())?;
        let mut key = [0u8; KEY_BYTES];
        derive(pin, &salt, iterations, &mut key);
        let b64 = base64::engine::general_purpose::STANDARD;
        Ok(format!("{VERSION}${iterations}${}${}", b64.encode(salt), b64.encode(key)))
    }

    /// Whether `pin` matches `stored`, in constant time; false for anything this module did not write.
    pub fn verify(stored: &str, pin: &[u8]) -> bool {
        if pin.is_empty() {
            return false;
        }
        let parts: Vec<&str> = stored.split('$').collect();
        let [version, iterations, salt, key] = parts[..] else { return false };
        if version != VERSION {
            return false;
        }
        let Some(iterations) = iterations.parse::<u32>().ok().filter(|i| *i <= MAX_STORED_ITERATIONS).and_then(NonZeroU32::new) else {
            return false;
        };
        let b64 = base64::engine::general_purpose::STANDARD;
        let (Ok(salt), Ok(key)) = (b64.decode(salt), b64.decode(key)) else { return false };
        if salt.is_empty() || key.is_empty() {
            return false;
        }
        pbkdf2::verify(ALGORITHM, iterations, &salt, pin, &key).is_ok()
    }

    /// Whether `stored` has the shape [`create`] gives it, so a PIN can match it at all.
    pub fn well_formed(stored: &str) -> bool {
        let b64 = base64::engine::general_purpose::STANDARD;
        let parts: Vec<&str> = stored.split('$').collect();
        let [version, iterations, salt, key] = parts[..] else { return false };
        version == VERSION
            && iterations.parse::<u32>().is_ok_and(|i| (1..=MAX_STORED_ITERATIONS).contains(&i))
            && b64.decode(salt).is_ok_and(|s| !s.is_empty())
            && b64.decode(key).is_ok_and(|k| !k.is_empty())
    }

    /// The iteration count that makes one check take about `TARGET_MS` here.
    pub fn calibrate() -> u32 {
        let probe = b"1234";
        let salt = [0u8; SALT_BYTES];
        let mut out = [0u8; KEY_BYTES];
        derive(probe, &salt, NonZeroU32::new(1_000).unwrap(), &mut out);
        let start = std::time::Instant::now();
        derive(probe, &salt, NonZeroU32::new(PROBE_ITERATIONS).unwrap(), &mut out);
        scale(start.elapsed().as_nanos() as u64)
    }

    /// Scales the probe run's time to the target and keeps the result in a sane range.
    pub fn scale(probe_nanos: u64) -> u32 {
        let per_iteration = probe_nanos.max(1) as f64 / f64::from(PROBE_ITERATIONS);
        let wanted = TARGET_MS as f64 * 1_000_000.0 / per_iteration;
        wanted.clamp(f64::from(MIN_ITERATIONS), f64::from(MAX_ITERATIONS)) as u32
    }
}

/// Backoff after wrong PINs. Four tries are free; the fifth wrong one locks for 30 s and each
/// further one doubles that, up to 30 min. Times are on the time since boot, which a clock change
/// cannot move; a restart begins that time again, so the stored lock is then renewed at its
/// current step rather than dropped.
pub mod throttle {
    pub const FREE_FAILURES: u32 = 4;
    pub const BASE_MS: i64 = 30_000;
    pub const MAX_MS: i64 = 30 * 60_000;

    /// Wrong tries in a row, when the lock ends and on which boot that time counts.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct State {
        pub fails: u32,
        pub lock_until: i64,
        pub lock_boot: i64,
    }

    /// The wait that follows the `fails`-th wrong try in a row.
    pub fn delay_after(fails: u32) -> i64 {
        if fails <= FREE_FAILURES {
            return 0;
        }
        let doublings = (fails - FREE_FAILURES - 1).min(16);
        MAX_MS.min(BASE_MS << doublings)
    }

    /// The state seen from boot `boot`: a lock written on another boot (`same_boot` false) is
    /// renewed at its step.
    pub fn on_boot(s: State, now: i64, boot: i64, same_boot: bool) -> State {
        if same_boot {
            return s;
        }
        let delay = delay_after(s.fails);
        State { fails: s.fails, lock_until: if delay > 0 { now + delay } else { 0 }, lock_boot: boot }
    }

    /// How long input stays refused; never more than the longest step, whatever was stored.
    pub fn remaining_ms(s: State, now: i64) -> i64 {
        (s.lock_until - now).clamp(0, MAX_MS)
    }

    /// The state after one more wrong (or not yet verified) try.
    pub fn failed(s: State, now: i64, boot: i64) -> State {
        let fails = s.fails.saturating_add(1);
        let delay = delay_after(fails);
        State { fails, lock_until: if delay > 0 { now + delay } else { 0 }, lock_boot: boot }
    }
}

/// When the lock asks for the PIN.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    App,
    Streams,
    Selected,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Off, Mode::App, Mode::Streams, Mode::Selected];
}

/// The clock the lockout and the grace time run on; replaced in tests.
pub trait Clock: Send + Sync {
    /// Milliseconds since the computer started, sleep included.
    fn elapsed(&self) -> i64;
    /// A number for this start of the computer.
    fn boot(&self) -> i64;
    /// Whether `stored`, a [`Self::boot`] saved earlier, was this start of the computer.
    fn is_this_boot(&self, stored: i64) -> bool {
        stored == self.boot()
    }
}

pub struct SystemClock;

/// How far two estimates of the boot time may lie apart and still be the same start (Windows,
/// and Linux without a boot id): the wall clock they come from drifts and is set by NTP.
pub const BOOT_TOLERANCE_MS: i64 = 60_000;

impl Clock for SystemClock {
    fn elapsed(&self) -> i64 {
        uptime_ms()
    }

    fn boot(&self) -> i64 {
        system_boot().0
    }

    fn is_this_boot(&self, stored: i64) -> bool {
        match system_boot() {
            (id, true) => stored == id,
            (estimate, false) => same_boot_estimate(stored, estimate),
        }
    }
}

/// This start of the computer, measured once per run so it cannot move while Nightjar runs, and
/// whether it is an exact id (Linux's boot_id) rather than an estimate of the boot time.
fn system_boot() -> (i64, bool) {
    static BOOT: std::sync::OnceLock<(i64, bool)> = std::sync::OnceLock::new();
    *BOOT.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            let mut uuid = [0u8; 37];
            let mut len = uuid.len();
            // kern.bootsessionuuid is a NUL-terminated UUID that changes at boot.
            if unsafe { libc::sysctlbyname(c"kern.bootsessionuuid".as_ptr(), uuid.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } == 0 {
                if let Some(id) = std::ffi::CStr::from_bytes_until_nul(&uuid).ok().and_then(|s| s.to_str().ok()).filter(|s| !s.is_empty()) {
                    return (boot_id_number(id), true);
                }
            }
        }
        #[cfg(target_os = "linux")]
        match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
            Ok(id) if !id.trim().is_empty() => return (boot_id_number(id.trim()), true),
            Ok(_) => eprintln!("nightjar: the kernel's boot id is empty; the PIN lock estimates the boot time instead"),
            Err(e) => eprintln!("nightjar: the kernel's boot id is unreadable ({e}); the PIN lock estimates the boot time instead"),
        }
        let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        (wall - uptime_ms(), false)
    })
}

/// Linux's boot id (a UUID) as a number to store.
#[cfg(not(windows))]
fn boot_id_number(id: &str) -> i64 {
    // FNV-1a: stable across runs and builds, unlike the standard hasher.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    hash as i64
}

/// Two estimates of the boot time (wall clock minus time since boot) belong to the same start
/// when they lie within [`BOOT_TOLERANCE_MS`]: clock drift and NTP corrections move the estimate
/// by seconds, a restart by at least the time the computer ran.
pub fn same_boot_estimate(stored: i64, now: i64) -> bool {
    stored.checked_sub(now).and_then(i64::checked_abs).is_some_and(|d| d < BOOT_TOLERANCE_MS)
}

#[cfg(windows)]
fn uptime_ms() -> i64 {
    // SAFETY: no arguments; counts since boot, sleep included.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() as i64 }
}

#[cfg(not(windows))]
#[allow(clippy::unnecessary_cast, reason = "time_t and c_long are 32-bit on 32-bit Linux")]
fn uptime_ms() -> i64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid timespec. CLOCK_BOOTTIME counts suspend too, like Android's elapsedRealtime.
    #[cfg(target_os = "macos")]
    let clock = libc::CLOCK_MONOTONIC_RAW; // Darwin includes sleep in this clock.
    #[cfg(target_os = "linux")]
    let clock = libc::CLOCK_BOOTTIME;
    if unsafe { libc::clock_gettime(clock, &mut t) } != 0 {
        panic!("boot clock unavailable: {}", std::io::Error::last_os_error());
    }
    t.tv_sec as i64 * 1000 + t.tv_nsec as i64 / 1_000_000
}

/// What is in `pin_lock.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    hash: Option<String>,
    length: usize,
    mode: Mode,
    /// The PCs Selected mode asks for, by their unique id.
    pcs: Vec<String>,
    grace_min: u32,
    fails: u32,
    lock_until: i64,
    lock_boot: i64,
    active_at: i64,
    active_boot: i64,
    /// An open lock handed to the second screen's process ([`PinStore::hand_over`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    hand_over: Option<HandOver>,
}

/// How long a hand-over stays good: the second screen's process reads it as it starts.
pub const HAND_OVER_MS: i64 = 30_000;

/// The SHA-256 of a hand-over's token, which only the started process's input carries, and when
/// it was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct HandOver {
    hash: String,
    at: i64,
    boot: i64,
}

fn token_hash(token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(ring::digest::digest(&ring::digest::SHA256, token.as_bytes()))
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            hash: None,
            length: MAX_DIGITS,
            mode: Mode::Off,
            pcs: Vec::new(),
            grace_min: DEFAULT_GRACE_MINUTES,
            fails: 0,
            lock_until: 0,
            lock_boot: 0,
            active_at: 0,
            active_boot: -1,
            hand_over: None,
        }
    }
}

impl Saved {
    /// What the lock holds while its file cannot be used: the app locked, no PIN that matches.
    /// Kept in memory only, never saved.
    fn closed() -> Self {
        Saved { hash: Some(String::new()), mode: Mode::App, grace_min: 0, ..Saved::default() }
    }
}

/// The lock's settings as the settings page shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub has_pin: bool,
    pub mode: Mode,
    pub pcs: Vec<String>,
    pub grace_minutes: u32,
}

/// What a try came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// `locked_for_ms` is set when this try started a lockout.
    Wrong { locked_for_ms: i64 },
    Locked { locked_for_ms: i64 },
    /// The lock's file cannot be used ([`PinStore::problem`]); the try was not counted.
    Unavailable,
}

/// Why the lock's file cannot be used. The lock stays closed meanwhile, and the file is read
/// again every [`RETRY_MS`], so deleting it, or whatever held it letting go, opens the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Reading it failed in a way that may pass (another program has it open, ...).
    Unreadable(String),
    /// It is there but not a lock this module wrote; only deleting it removes the PIN lock.
    Damaged(String),
}

/// How often a file with a [`Problem`] is read again.
pub const RETRY_MS: u64 = 1_000;

/// One reader or writer of the lock's file at a time, across every store in the process: loads,
/// changes and saves of the file, and each try's count-check-clear, run under it. Only worker
/// threads take it; the UI thread works on the copy in memory.
static TRY_LOCK: Mutex<()> = Mutex::new(());

fn file_lock() -> std::sync::MutexGuard<'static, ()> {
    TRY_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The lock's state in memory, which every caller reads and changes; the file follows it.
struct Cache {
    saved: Saved,
    problem: Option<Problem>,
    /// When a file with a problem is read next, and whether that read is queued.
    retry_at: Instant,
    reading: bool,
}

/// Work for the store's file thread.
enum Job {
    /// Write what memory holds now (or remove the file when no PIN is set).
    Save,
    /// Read a file that had a problem again.
    Read,
    /// Answers once the jobs before it are done.
    Flush(Sender<()>),
}

/// The lock. Reads and changes work on memory and never wait for the file; a thread of the
/// store's own writes the file behind them. Tries ([`Self::try_pin`]) and [`Self::set_pin`] are
/// slow and run on a worker, where they write the file themselves.
#[derive(Clone)]
pub struct PinStore {
    path: PathBuf,
    clock: Arc<dyn Clock>,
    cache: Arc<Mutex<Cache>>,
    jobs: Sender<Job>,
}

impl PinStore {
    /// Reads the file once, here.
    pub fn new(dir: &Path, clock: Arc<dyn Clock>) -> Self {
        let path = dir.join(FILE);
        let cache = Arc::new(Mutex::new(Cache { saved: Saved::default(), problem: None, retry_at: Instant::now(), reading: false }));
        {
            let _file = file_lock();
            read_into(&path, &cache);
        }
        let (jobs, queue) = channel();
        let (thread_path, thread_cache) = (path.clone(), cache.clone());
        std::thread::Builder::new()
            .name("pin-lock-file".into())
            .spawn(move || file_thread(&thread_path, &thread_cache, &queue))
            .expect("start the PIN lock's file thread");
        Self { path, clock, cache, jobs }
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn queue(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            eprintln!("nightjar: the PIN lock's file thread is gone; the change stays in memory only");
        }
    }

    /// The state as memory holds it. A file with a problem is read again, in the background,
    /// once its retry time has come.
    fn saved(&self) -> Saved {
        let mut memory = self.memory();
        if memory.problem.is_some() && !memory.reading && Instant::now() >= memory.retry_at {
            memory.reading = true;
            drop(memory);
            self.queue(Job::Read);
            return self.memory().saved.clone();
        }
        memory.saved.clone()
    }

    /// Changes the state and has it written. Refused (and said so) while the file has a problem:
    /// the closed state in memory is never written over the user's file.
    fn edit(&self, change: impl FnOnce(&mut Saved)) {
        {
            let mut memory = self.memory();
            if let Some(problem) = &memory.problem {
                eprintln!("nightjar: the PIN lock is not changed while its file cannot be used: {problem:?}");
                return;
            }
            change(&mut memory.saved);
        }
        self.queue(Job::Save);
    }

    /// The lock's file, which the lock screen names when it cannot be used.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Why the file cannot be used, if it cannot.
    pub fn problem(&self) -> Option<Problem> {
        self.saved();
        self.memory().problem.clone()
    }

    pub fn has_pin(&self) -> bool {
        self.saved().hash.is_some()
    }

    /// What the settings page shows, read once.
    pub fn view(&self) -> View {
        let saved = self.saved();
        let has_pin = saved.hash.is_some();
        View { has_pin, mode: if has_pin { saved.mode } else { Mode::Off }, pcs: saved.pcs, grace_minutes: grace_of(saved.grace_min) }
    }

    /// Digits in the PIN, for the dots and the automatic submit; 0 without a PIN.
    pub fn pin_length(&self) -> usize {
        let saved = self.saved();
        if saved.hash.is_some() { saved.length.clamp(MIN_DIGITS, MAX_DIGITS) } else { 0 }
    }

    /// Mode in effect: without a PIN nothing is locked, whatever the stored mode says.
    pub fn mode(&self) -> Mode {
        let saved = self.saved();
        if saved.hash.is_some() { saved.mode } else { Mode::Off }
    }

    pub fn set_mode(&self, mode: Mode) {
        self.edit(|s| s.mode = mode);
    }

    pub fn set_pcs(&self, pcs: Vec<String>) {
        self.edit(|s| s.pcs = pcs);
    }

    pub fn grace_minutes(&self) -> u32 {
        grace_of(self.saved().grace_min)
    }

    pub fn set_grace_minutes(&self, minutes: u32) {
        self.edit(|s| s.grace_min = minutes);
    }

    /// Stores a new PIN under a fresh salt and clears the failure count. Slow (measures the
    /// computer, then writes the file): for a worker thread.
    pub fn set_pin(&self, pin: &[u8]) -> Result<(), String> {
        self.set_pin_with(pin, hasher::calibrate())
    }

    fn set_pin_with(&self, pin: &[u8], iterations: u32) -> Result<(), String> {
        if !(MIN_DIGITS..=MAX_DIGITS).contains(&pin.len()) || !pin.iter().all(u8::is_ascii_digit) {
            return Err(format!("a PIN has {MIN_DIGITS} to {MAX_DIGITS} digits"));
        }
        let hash = hasher::create(pin, iterations)?;
        let _file = file_lock();
        let saved = {
            let mut memory = self.memory();
            if memory.problem.is_some() {
                return Err("the PIN lock's file cannot be used".into());
            }
            memory.saved.hash = Some(hash);
            memory.saved.length = pin.len();
            (memory.saved.fails, memory.saved.lock_until, memory.saved.lock_boot) = (0, 0, 0);
            memory.saved.clone()
        };
        write_file(&self.path, &saved).map_err(|e| format!("saving the PIN failed: {e}"))
    }

    /// Removes the PIN and everything that depended on it.
    pub fn remove_pin(&self) {
        self.edit(|s| *s = Saved::default());
    }

    fn throttle_state(saved: &Saved) -> throttle::State {
        throttle::State { fails: saved.fails, lock_until: saved.lock_until, lock_boot: saved.lock_boot }
    }

    fn put_throttle(saved: &mut Saved, state: throttle::State) {
        (saved.fails, saved.lock_until, saved.lock_boot) = (state.fails, state.lock_until, state.lock_boot);
    }

    /// The lockout seen from this boot. A lock renewed for a new boot is kept (and written, when
    /// `write` asks) so its countdown moves on.
    fn current(&self, memory: &mut Cache, now: i64) -> throttle::State {
        let stored = Self::throttle_state(&memory.saved);
        let state = throttle::on_boot(stored, now, self.clock.boot(), self.clock.is_this_boot(stored.lock_boot));
        if state != stored && memory.problem.is_none() {
            Self::put_throttle(&mut memory.saved, state);
        }
        state
    }

    /// Milliseconds until the next try is allowed, 0 when it is. From memory: no waiting.
    pub fn locked_for_ms(&self) -> i64 {
        let now = self.clock.elapsed();
        let (state, renewed) = {
            let mut memory = self.memory();
            let before = Self::throttle_state(&memory.saved);
            let state = self.current(&mut memory, now);
            (state, Self::throttle_state(&memory.saved) != before)
        };
        if renewed {
            self.queue(Job::Save);
        }
        throttle::remaining_ms(state, now)
    }

    /// One try. Refused while locked out or while the file cannot be used (not counted then);
    /// otherwise the failure is written before the PIN is checked, so ending the app mid-check
    /// cannot get a free guess, and a match clears it again. Slow: for a worker thread. Count,
    /// check and clear run under the file lock, so two PIN screens cannot both count against the
    /// same state.
    pub fn try_pin(&self, pin: &[u8]) -> Outcome {
        self.try_pin_with(pin, hasher::verify)
    }

    fn try_pin_with(&self, pin: &[u8], verify: impl FnOnce(&str, &[u8]) -> bool) -> Outcome {
        let _file = file_lock();
        if self.memory().problem.is_some() {
            // The worker may wait for the disk: read the file now rather than on the next retry.
            read_into(&self.path, &self.cache);
        }
        let now = self.clock.elapsed();
        let boot = self.clock.boot();
        let (counted, hash) = {
            let mut memory = self.memory();
            if memory.problem.is_some() {
                return Outcome::Unavailable;
            }
            let state = self.current(&mut memory, now);
            let locked = throttle::remaining_ms(state, now);
            if locked > 0 {
                return Outcome::Locked { locked_for_ms: locked };
            }
            let counted = throttle::failed(state, now, boot);
            Self::put_throttle(&mut memory.saved, counted);
            (counted, memory.saved.hash.clone().unwrap_or_default())
        };
        self.write_now();
        if !verify(&hash, pin) {
            return Outcome::Wrong { locked_for_ms: throttle::remaining_ms(counted, self.clock.elapsed()) };
        }
        Self::put_throttle(&mut self.memory().saved, throttle::State::default());
        self.write_now();
        Outcome::Ok
    }

    /// Writes memory's state to the file; the caller holds the file lock.
    fn write_now(&self) {
        let saved = self.memory().saved.clone();
        if let Err(e) = write_file(&self.path, &saved) {
            eprintln!("nightjar: saving the PIN lock failed: {e}");
        }
    }

    /// Remembers when the app was left unlocked, for the "ask again after" time.
    pub fn set_active(&self, elapsed: i64, boot: i64) {
        self.edit(|s| (s.active_at, s.active_boot) = (elapsed, boot));
    }

    /// Hands this process's open lock to a process it starts (the second screen's): a fresh
    /// random token for that process's standard input, never a file or a command line. The file
    /// keeps only the token's hash, for [`HAND_OVER_MS`], and is written before this returns.
    /// None without a PIN: there is nothing to hand over.
    pub fn hand_over(&self) -> Result<Option<String>, String> {
        use ring::rand::SecureRandom as _;
        if !self.has_pin() {
            return Ok(None);
        }
        let mut bytes = [0u8; 32];
        ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| "no random numbers for a token".to_owned())?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let entry = HandOver { hash: token_hash(&token), at: self.clock.elapsed(), boot: self.clock.boot() };
        let _file = file_lock();
        let saved = {
            let mut memory = self.memory();
            if memory.problem.is_some() {
                return Err("the PIN lock's file cannot be used".into());
            }
            memory.saved.hand_over = Some(entry);
            memory.saved.clone()
        };
        write_file(&self.path, &saved).map_err(|e| format!("saving the PIN lock failed: {e}"))?;
        Ok(Some(token))
    }

    /// The process the lock was handed to is gone: its token opens nothing any more.
    pub fn end_hand_over(&self) {
        if self.saved().hand_over.is_some() {
            self.edit(|s| s.hand_over = None);
        }
    }

    /// Whether `token`, read from this process's standard input, is a hand-over written on this
    /// boot less than [`HAND_OVER_MS`] ago, by a Nightjar whose lock was open.
    pub fn handed_over(&self, token: &str) -> bool {
        let saved = self.saved();
        let Some(entry) = saved.hand_over.filter(|_| saved.hash.is_some() && !token.is_empty()) else { return false };
        entry.hash == token_hash(token) && self.clock.is_this_boot(entry.boot) && (0..HAND_OVER_MS).contains(&(self.clock.elapsed() - entry.at))
    }

    pub fn elapsed(&self) -> i64 {
        self.clock.elapsed()
    }

    pub fn boot(&self) -> i64 {
        self.clock.boot()
    }

    /// Waits until the file thread has written everything queued before: when Nightjar closes.
    pub fn flush(&self) {
        let (done, wait) = channel();
        self.queue(Job::Flush(done));
        if wait.recv().is_err() {
            eprintln!("nightjar: the PIN lock's file thread ended before it wrote everything");
        }
    }
}

fn grace_of(minutes: u32) -> u32 {
    if GRACE_CHOICES.contains(&minutes) { minutes } else { DEFAULT_GRACE_MINUTES }
}

/// The store's file thread: saves and reads queued from the UI, coalesced, under the file lock.
fn file_thread(path: &Path, cache: &Mutex<Cache>, queue: &Receiver<Job>) {
    while let Ok(first) = queue.recv() {
        let (mut save, mut read) = (false, false);
        let mut flushes = Vec::new();
        for job in std::iter::once(first).chain(queue.try_iter()) {
            match job {
                Job::Save => save = true,
                Job::Read => read = true,
                Job::Flush(done) => flushes.push(done),
            }
        }
        {
            let _file = file_lock();
            if read {
                read_into(path, cache);
            }
            if save {
                let saved = {
                    let memory = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Never write the closed state over the user's file.
                    memory.problem.is_none().then(|| memory.saved.clone())
                };
                if let Some(saved) = saved {
                    if let Err(e) = write_file(path, &saved) {
                        eprintln!("nightjar: saving the PIN lock failed: {e}");
                    }
                }
            }
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

/// Reads the file into memory; the caller holds the file lock. A missing file is no lock. A file
/// that cannot be read, or is not one this module wrote, keeps the lock closed (fail closed)
/// without touching the file, and is read again after [`RETRY_MS`].
fn read_into(path: &Path, cache: &Mutex<Cache>) {
    let read = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Saved>(&bytes).map_err(|e| Problem::Damaged(e.to_string())).and_then(|saved| match &saved.hash {
            Some(hash) if !hasher::well_formed(hash) => Err(Problem::Damaged("the stored PIN is not a hash this version wrote".into())),
            _ => Ok(saved),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Saved::default()),
        Err(e) => Err(Problem::Unreadable(e.to_string())),
    };
    let mut memory = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    memory.reading = false;
    memory.retry_at = Instant::now() + Duration::from_millis(RETRY_MS);
    match read {
        Ok(saved) => {
            if memory.problem.take().is_some() {
                eprintln!("nightjar: the PIN lock file {} is usable again", path.display());
            }
            memory.saved = saved;
        }
        Err(problem) => {
            if memory.problem.as_ref() != Some(&problem) {
                match &problem {
                    Problem::Unreadable(why) => eprintln!("nightjar: the PIN lock file {} cannot be read now ({why}); the app stays locked and reads it again", path.display()),
                    Problem::Damaged(why) => eprintln!("nightjar: the PIN lock file {} is damaged ({why}); the app stays locked until it is deleted", path.display()),
                }
            }
            memory.problem = Some(problem);
            memory.saved = Saved::closed();
        }
    }
}

/// Writes the state, or removes the file when no PIN is set (nothing else is worth keeping then).
fn write_file(path: &Path, saved: &Saved) -> std::io::Result<()> {
    if saved.hash.is_none() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_vec_pretty(saved).map_err(std::io::Error::other)?;
    crate::config::write_private(path, &text)
}

/// How long the PIN screen's approval of one PC's stream start stays valid.
pub const GRANT_MS: i64 = 60_000;

/// The key a stream start on a PC is asked and granted under: its unique id, or, for a PC that
/// gave none, Nightjar's own id for it, so the right PIN still lets that start through.
pub fn stream_key(unique_id: &str, host_id: u64) -> String {
    if unique_id.is_empty() { format!("host:{host_id}") } else { unique_id.to_owned() }
}

/// Decides when the PIN is asked for: Android's PinGate without its activity bookkeeping. The
/// window being minimized or hidden is leaving the app.
pub struct PinGate {
    unlocked_app: bool,
    grant: Option<(String, i64)>,
    /// When the window went out of sight, None while it is in view.
    left_at: Option<i64>,
    /// The second screen's process, handed the first's open lock ([`PinStore::handed_over`]).
    handed_over: bool,
}

pub fn app_needed(pin_set: bool, mode: Mode, unlocked: bool) -> bool {
    pin_set && mode == Mode::App && !unlocked
}

pub fn stream_needed(pin_set: bool, mode: Mode, pcs: &[String], key: &str, unlocked: bool, grant_valid: bool) -> bool {
    if app_needed(pin_set, mode, unlocked) {
        return true;
    }
    if !pin_set {
        return false;
    }
    let asks = mode == Mode::Streams || (mode == Mode::Selected && contains(pcs, key));
    asks && !grant_valid
}

pub fn contains(pcs: &[String], uuid: &str) -> bool {
    !uuid.is_empty() && pcs.iter().any(|pc| pc.eq_ignore_ascii_case(uuid))
}

/// Whether the app was left unlocked recently enough on this boot.
pub fn within_grace(same_boot: bool, left_at: i64, now: i64, grace_minutes: u32) -> bool {
    let away = now - left_at;
    same_boot && grace_minutes > 0 && away >= 0 && away < i64::from(grace_minutes) * 60_000
}

impl PinGate {
    /// After a restart the app counts as unlocked if it was left unlocked a moment ago.
    pub fn new(store: &PinStore) -> Self {
        let saved = store.saved();
        let unlocked_app = saved.hash.is_some()
            && saved.mode == Mode::App
            && within_grace(store.clock.is_this_boot(saved.active_boot), saved.active_at, store.elapsed(), grace_of(saved.grace_min));
        Self { unlocked_app, grant: None, left_at: None, handed_over: false }
    }

    /// The app is locked and nothing has unlocked it.
    pub fn needs_pin_for_app(&self, store: &PinStore) -> bool {
        app_needed(store.has_pin(), store.mode(), self.unlocked_app)
    }

    /// Whether starting a stream under `key` ([`stream_key`]) must first go through the PIN
    /// screen. A yes answer from the PIN screen ([`Self::grant`]) is used up by the start it was
    /// given for.
    pub fn needs_pin_for_stream(&mut self, store: &PinStore, key: &str) -> bool {
        let needed = self.locks_stream(store, key);
        if !needed && self.granted(store, key) {
            self.grant = None;
        }
        needed
    }

    /// The same answer without using up a grant: Continue here's press asks this, and the stream
    /// start that follows the press uses the grant.
    pub fn locks_stream(&self, store: &PinStore, key: &str) -> bool {
        if self.handed_over {
            return false;
        }
        let view = store.view();
        stream_needed(view.has_pin, view.mode, &view.pcs, key, self.unlocked_app, self.granted(store, key))
    }

    fn granted(&self, store: &PinStore, key: &str) -> bool {
        let now = store.elapsed();
        self.grant.as_ref().is_some_and(|(pc, at)| !key.is_empty() && pc.eq_ignore_ascii_case(key) && (0..GRANT_MS).contains(&(now - at)))
    }

    /// The PIN was entered for starting a stream under `key`.
    pub fn grant(&mut self, store: &PinStore, key: &str) {
        self.grant = Some((key.to_owned(), store.elapsed()));
    }

    /// The start a grant was left for will not come (Continue here's press failed, or a launch
    /// was busy): it must not open the next start, whichever that is.
    pub fn forget_grant(&mut self) {
        self.grant = None;
    }

    /// The PIN was entered (or has just been set): the app stays open until it is left for the grace time.
    pub fn unlock_app(&mut self) {
        self.unlocked_app = true;
        self.left_at = None;
    }

    /// This is the second screen's process, which the first's open lock was handed to: it streams
    /// the first's game and ends with it, and asks for no PIN.
    pub fn hand_over(&mut self) {
        self.handed_over = true;
        self.unlocked_app = true;
    }

    /// The window went out of sight (minimized or hidden), or the app is closing.
    pub fn left(&mut self, store: &PinStore) {
        if self.left_at.is_some() {
            return;
        }
        let now = store.elapsed();
        self.left_at = Some(now);
        // The second screen's process writes nothing: its lock was the first's, which keeps the file.
        if self.unlocked_app && !self.handed_over && store.has_pin() {
            store.set_active(now, store.boot());
        }
    }

    /// The window is in view again: the app locks when it was away longer than the grace time.
    pub fn returned(&mut self, store: &PinStore) {
        let Some(left_at) = self.left_at.take() else { return };
        if self.unlocked_app && !self.handed_over && store.mode() == Mode::App && !within_grace(true, left_at, store.elapsed(), store.grace_minutes()) {
            self.unlocked_app = false;
        }
    }
}

#[cfg(test)]
mod tests;
