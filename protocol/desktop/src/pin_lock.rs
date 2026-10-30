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
        #[cfg(not(windows))]
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
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut t) } != 0 {
        panic!("CLOCK_BOOTTIME unavailable: {}", std::io::Error::last_os_error());
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
        Self { unlocked_app, grant: None, left_at: None }
    }

    /// The app is locked and nothing has unlocked it.
    pub fn needs_pin_for_app(&self, store: &PinStore) -> bool {
        app_needed(store.has_pin(), store.mode(), self.unlocked_app)
    }

    /// Whether starting a stream under `key` ([`stream_key`]) must first go through the PIN
    /// screen. A yes answer from the PIN screen ([`Self::grant`]) is used up by the start it was
    /// given for.
    pub fn needs_pin_for_stream(&mut self, store: &PinStore, key: &str) -> bool {
        let now = store.elapsed();
        let granted = self.grant.as_ref().is_some_and(|(pc, at)| !key.is_empty() && pc.eq_ignore_ascii_case(key) && (0..GRANT_MS).contains(&(now - at)));
        let view = store.view();
        let needed = stream_needed(view.has_pin, view.mode, &view.pcs, key, self.unlocked_app, granted);
        if !needed && granted {
            self.grant = None;
        }
        needed
    }

    /// The PIN was entered for starting a stream under `key`.
    pub fn grant(&mut self, store: &PinStore, key: &str) {
        self.grant = Some((key.to_owned(), store.elapsed()));
    }

    /// The PIN was entered (or has just been set): the app stays open until it is left for the grace time.
    pub fn unlock_app(&mut self) {
        self.unlocked_app = true;
        self.left_at = None;
    }

    /// The window went out of sight (minimized or hidden), or the app is closing.
    pub fn left(&mut self, store: &PinStore) {
        if self.left_at.is_some() {
            return;
        }
        let now = store.elapsed();
        self.left_at = Some(now);
        if self.unlocked_app && store.has_pin() {
            store.set_active(now, store.boot());
        }
    }

    /// The window is in view again: the app locks when it was away longer than the grace time.
    pub fn returned(&mut self, store: &PinStore) {
        let Some(left_at) = self.left_at.take() else { return };
        if self.unlocked_app && store.mode() == Mode::App && !within_grace(true, left_at, store.elapsed(), store.grace_minutes()) {
            self.unlocked_app = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct FakeClock {
        now: AtomicI64,
        boot: AtomicI64,
    }

    impl Clock for FakeClock {
        fn elapsed(&self) -> i64 {
            self.now.load(Ordering::SeqCst)
        }
        fn boot(&self) -> i64 {
            self.boot.load(Ordering::SeqCst)
        }
    }

    fn store(tag: &str) -> (PinStore, Arc<FakeClock>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("nightjar-pin-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let clock = Arc::new(FakeClock { now: AtomicI64::new(1_000_000), boot: AtomicI64::new(7) });
        (PinStore::new(&dir, clock.clone()), clock, dir)
    }

    #[test]
    fn a_hash_verifies_its_pin_only_and_keeps_androids_format() {
        let record = hasher::create(b"2580", 1_000).unwrap();
        let parts: Vec<&str> = record.split('$').collect();
        assert_eq!(parts.len(), 4, "{record}");
        assert_eq!((parts[0], parts[1]), ("v1", "1000"));
        let b64 = base64::engine::general_purpose::STANDARD;
        assert_eq!(b64.decode(parts[2]).unwrap().len(), hasher::SALT_BYTES);
        assert_eq!(b64.decode(parts[3]).unwrap().len(), hasher::KEY_BYTES);
        assert!(!record.contains("2580"), "the PIN is never stored");
        assert!(hasher::verify(&record, b"2580"));
        for wrong in [&b"2581"[..], b"258", b"25800", b""] {
            assert!(!hasher::verify(&record, wrong), "{wrong:?}");
        }
        assert_ne!(record, hasher::create(b"2580", 1_000).unwrap(), "every PIN gets a fresh salt");
    }

    #[test]
    fn a_record_androids_pin_hasher_wrote_verifies() {
        // PBKDF2-HMAC-SHA256 of "1234", salt 16 zero bytes, 1000 iterations: the same derivation
        // Android's PinHasher runs over javax.crypto.Mac, so a record moves between the two.
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut key = [0u8; 32];
        ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, std::num::NonZeroU32::new(1000).unwrap(), &[0; 16], b"1234", &mut key);
        let record = format!("v1$1000${}${}", b64.encode([0u8; 16]), b64.encode(key));
        assert!(hasher::verify(&record, b"1234"));
        // RFC 7914's PBKDF2-HMAC-SHA256 vector (P "passwd", S "salt", c 1) checks the derivation itself.
        let mut out = [0u8; 64];
        ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, std::num::NonZeroU32::new(1).unwrap(), b"salt", b"passwd", &mut out);
        assert_eq!(&out[..8], &[0x55, 0xac, 0x04, 0x6e, 0x56, 0xe3, 0x08, 0x9f]);
    }

    #[test]
    fn records_that_were_not_written_here_never_verify() {
        let good = hasher::create(b"1234", 1_000).unwrap();
        let (_, rest) = good.split_once('$').unwrap();
        for bad in [
            String::new(),
            format!("v2${rest}"),
            good.replacen("1000", "0", 1),
            good.replacen("1000", "99999999", 1),
            good.replacen("1000", "x", 1),
            "v1$1000$$".into(),
            "v1$1000$!!$!!".into(),
            format!("{good}$extra"),
        ] {
            assert!(!hasher::verify(&bad, b"1234"), "{bad}");
        }
    }

    #[test]
    fn calibration_aims_at_the_target_and_stays_in_range() {
        assert_eq!(hasher::scale(15_000_000), 100_000, "1.5 µs an iteration: 150 ms is 100k");
        assert_eq!(hasher::scale(1), hasher::MAX_ITERATIONS);
        assert_eq!(hasher::scale(u64::MAX), hasher::MIN_ITERATIONS);
        let measured = hasher::calibrate();
        assert!((hasher::MIN_ITERATIONS..=hasher::MAX_ITERATIONS).contains(&measured));
    }

    #[test]
    fn four_wrong_tries_are_free_then_the_wait_doubles_to_half_an_hour() {
        let delays: Vec<i64> = (1..=12).map(throttle::delay_after).collect();
        assert_eq!(delays, [0, 0, 0, 0, 30_000, 60_000, 120_000, 240_000, 480_000, 960_000, 1_800_000, 1_800_000]);
        assert_eq!(throttle::delay_after(u32::MAX), throttle::MAX_MS);
        let locked = throttle::State { fails: 5, lock_until: 100_000, lock_boot: 1 };
        assert_eq!(throttle::remaining_ms(locked, 90_000), 10_000);
        assert_eq!(throttle::remaining_ms(locked, 200_000), 0);
        let hostile = throttle::State { lock_until: i64::MAX, ..locked };
        assert_eq!(throttle::remaining_ms(hostile, 0), throttle::MAX_MS, "never longer than the longest step");
        // A restart renews the lock at its step instead of dropping it.
        assert_eq!(throttle::on_boot(locked, 5, 2, false), throttle::State { fails: 5, lock_until: 30_005, lock_boot: 2 });
        assert_eq!(throttle::on_boot(locked, 5, 1, true), locked, "same boot: as stored");
    }

    #[test]
    fn the_store_locks_out_after_wrong_tries_and_a_right_one_clears_it() {
        let (store, clock, dir) = store("lockout");
        assert!(!store.has_pin() && store.mode() == Mode::Off);
        store.set_pin_with(b"4321", 1_000).unwrap();
        store.set_mode(Mode::App);
        assert_eq!((store.pin_length(), store.mode()), (4, Mode::App));
        store.flush();
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!text.contains("4321"), "only the hash is saved: {text}");
        for _ in 0..4 {
            assert_eq!(store.try_pin(b"0000"), Outcome::Wrong { locked_for_ms: 0 });
        }
        assert_eq!(store.try_pin(b"0000"), Outcome::Wrong { locked_for_ms: 30_000 });
        assert_eq!(store.try_pin(b"4321"), Outcome::Locked { locked_for_ms: 30_000 }, "even the right PIN waits");
        clock.now.fetch_add(10_000, Ordering::SeqCst);
        assert_eq!(store.locked_for_ms(), 20_000);
        // A restart of the computer renews the lock at its step.
        clock.boot.store(8, Ordering::SeqCst);
        assert_eq!(store.locked_for_ms(), 30_000);
        clock.now.fetch_add(30_000, Ordering::SeqCst);
        assert_eq!(store.try_pin(b"0000"), Outcome::Wrong { locked_for_ms: 60_000 }, "the sixth wrong try doubles");
        clock.now.fetch_add(60_000, Ordering::SeqCst);
        assert_eq!(store.try_pin(b"4321"), Outcome::Ok);
        assert_eq!(store.locked_for_ms(), 0);
        for _ in 0..4 {
            assert_eq!(store.try_pin(b"1111"), Outcome::Wrong { locked_for_ms: 0 }, "the count started over");
        }
        // A new PIN clears the count too.
        store.set_pin_with(b"98765", 1_000).unwrap();
        assert_eq!(store.try_pin(b"1111"), Outcome::Wrong { locked_for_ms: 0 });
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_try_is_counted_before_the_check_runs() {
        let (store, _, dir) = store("counted");
        store.set_pin_with(b"1234", 1_000).unwrap();
        let on_disk = || serde_json::from_slice::<Saved>(&std::fs::read(dir.join(FILE)).unwrap()).unwrap().fails;
        let fails_during_check = std::cell::Cell::new(0);
        let outcome = store.try_pin_with(b"1234", |hash, pin| {
            fails_during_check.set(on_disk());
            hasher::verify(hash, pin)
        });
        assert_eq!(outcome, Outcome::Ok);
        assert_eq!(fails_during_check.get(), 1, "a kill during the check cannot give a free guess");
        assert_eq!(on_disk(), 0, "and a match clears it");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_pin_needs_four_to_eight_digits_and_removing_it_turns_the_lock_off() {
        let (store, _, dir) = store("rules");
        for bad in [&b"123"[..], b"123456789", b"12a4", b""] {
            assert!(store.set_pin_with(bad, 1_000).is_err(), "{bad:?}");
        }
        assert!(!store.has_pin());
        store.set_pin_with(b"12345678", 1_000).unwrap();
        store.set_mode(Mode::Streams);
        store.set_grace_minutes(30);
        store.set_pcs(vec!["UID".into()]);
        store.remove_pin();
        assert!(!store.has_pin());
        assert_eq!((store.mode(), store.grace_minutes(), store.view().pcs.len()), (Mode::Off, DEFAULT_GRACE_MINUTES, 0), "everything that depended on it goes");
        std::fs::remove_dir_all(dir).ok();
    }

    /// Has a file with a problem read again on the next call instead of a second later.
    fn retry_now(store: &PinStore) {
        store.memory().retry_at = Instant::now();
    }

    #[test]
    fn a_damaged_file_keeps_the_app_locked_counts_no_tries_and_is_never_overwritten() {
        for (n, damaged) in ["{ not json", r#"{"hash":"not a hash","mode":"off"}"#].into_iter().enumerate() {
            let (_, clock, dir) = store(&format!("damaged{n}"));
            std::fs::write(dir.join(FILE), damaged).unwrap();
            let store = PinStore::new(&dir, clock.clone());
            assert!(matches!(store.problem(), Some(Problem::Damaged(_))), "{damaged}");
            assert!(store.has_pin() && store.mode() == Mode::App);
            let gate = PinGate::new(&store);
            assert!(gate.needs_pin_for_app(&store));
            for _ in 0..8 {
                assert_eq!(store.try_pin(b"1234"), Outcome::Unavailable, "no PIN opens it, and no try counts");
            }
            assert_eq!(store.locked_for_ms(), 0);
            store.set_mode(Mode::Off);
            store.set_grace_minutes(60);
            store.flush();
            assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), damaged, "the closed state is never saved over it");
            // Deleting it, as the lock screen says, removes the lock without a restart.
            std::fs::remove_file(dir.join(FILE)).unwrap();
            retry_now(&store);
            store.problem();
            store.flush();
            assert_eq!(store.problem(), None);
            assert!(!store.has_pin() && !gate.needs_pin_for_app(&store));
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_now_locks_until_it_can_and_keeps_the_pin() {
        let (store, clock, dir) = store("unreadable");
        store.set_pin_with(b"1234", 1_000).unwrap();
        store.set_mode(Mode::Streams);
        store.flush();
        let good = std::fs::read(dir.join(FILE)).unwrap();
        drop(store);
        // A read that fails for another reason than a missing file (here: a folder in its place).
        std::fs::remove_file(dir.join(FILE)).unwrap();
        std::fs::create_dir(dir.join(FILE)).unwrap();
        let store = PinStore::new(&dir, clock.clone());
        assert!(matches!(store.problem(), Some(Problem::Unreadable(_))));
        assert!(store.has_pin() && store.mode() == Mode::App, "locked meanwhile");
        store.set_mode(Mode::Off);
        store.flush();
        assert!(dir.join(FILE).is_dir(), "nothing was written in its place");
        // It can be read again: the next read takes it, PIN and all.
        std::fs::remove_dir(dir.join(FILE)).unwrap();
        std::fs::write(dir.join(FILE), &good).unwrap();
        assert_eq!(store.try_pin(b"1234"), Outcome::Ok, "a try reads it again first");
        assert_eq!((store.problem(), store.mode()), (None, Mode::Streams));
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn a_file_another_program_holds_is_read_again_not_replaced() {
        use std::os::windows::fs::OpenOptionsExt;
        let (store, clock, dir) = store("shared");
        store.set_pin_with(b"1234", 1_000).unwrap();
        drop(store);
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(dir.join(FILE)).unwrap();
        let store = PinStore::new(&dir, clock.clone());
        assert!(matches!(store.problem(), Some(Problem::Unreadable(_))), "a sharing violation is not damage");
        drop(held);
        retry_now(&store);
        store.problem();
        store.flush();
        assert_eq!(store.problem(), None);
        assert_eq!(store.pin_length(), 4);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_ui_never_waits_for_the_file() {
        let (store, _, dir) = store("nowait");
        store.set_pin_with(b"1234", 1_000).unwrap();
        // A try or a write holds the file lock; the UI's calls answer from memory meanwhile.
        let held = file_lock();
        let (done, wait) = channel();
        let ui = store.clone();
        std::thread::spawn(move || {
            ui.set_mode(Mode::App);
            ui.set_grace_minutes(30);
            let answer = (ui.locked_for_ms(), ui.has_pin(), ui.mode(), ui.view().grace_minutes, ui.problem());
            let _ = done.send(answer);
        });
        let answer = wait.recv_timeout(Duration::from_secs(5)).expect("answered without the file lock");
        assert_eq!(answer, (0, true, Mode::App, 30, None));
        drop(held);
        store.flush();
        let saved: Saved = serde_json::from_slice(&std::fs::read(dir.join(FILE)).unwrap()).unwrap();
        assert_eq!((saved.mode, saved.grace_min), (Mode::App, 30), "the file thread wrote it after");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_boot_estimate_survives_clock_drift_but_not_a_restart() {
        let boot = 1_700_000_000_000;
        assert!(same_boot_estimate(boot, boot + 59_000), "NTP moved the clock by a minute at most");
        assert!(same_boot_estimate(boot, boot - 59_000));
        assert!(!same_boot_estimate(boot, boot + 10 * 60_000), "a restart ten minutes on");
        assert!(!same_boot_estimate(i64::MIN, 0) && !same_boot_estimate(-1, boot), "nothing stored");
        let clock = SystemClock;
        assert!(clock.is_this_boot(clock.boot()), "this run's own boot");
        assert_eq!(clock.boot(), clock.boot(), "measured once per run");
        #[cfg(not(windows))]
        assert_ne!(boot_id_number("a"), boot_id_number("b"));
    }

    #[test]
    fn the_gate_decides_as_androids_table_does() {
        let pcs = vec!["AbC".to_owned()];
        // No PIN: nothing is locked.
        assert!(!app_needed(false, Mode::App, false));
        assert!(!stream_needed(false, Mode::Streams, &pcs, "abc", false, false));
        // App lock: the app and its streams, until unlocked.
        assert!(app_needed(true, Mode::App, false) && !app_needed(true, Mode::App, true));
        assert!(stream_needed(true, Mode::App, &pcs, "x", false, false));
        assert!(!stream_needed(true, Mode::App, &pcs, "x", true, false));
        // Every stream, unless just granted.
        assert!(stream_needed(true, Mode::Streams, &pcs, "x", true, false));
        assert!(!stream_needed(true, Mode::Streams, &pcs, "x", true, true));
        // Selected PCs, matched without case.
        assert!(stream_needed(true, Mode::Selected, &pcs, "abc", true, false));
        assert!(!stream_needed(true, Mode::Selected, &pcs, "other", true, false));
        assert!(!stream_needed(true, Mode::Selected, &pcs, "", true, false));
        assert!(!stream_needed(true, Mode::Off, &pcs, "abc", true, false));
        // Grace: same boot, inside the minutes, never with 0.
        assert!(within_grace(true, 1000, 1000 + 9 * 60_000, 10));
        assert!(!within_grace(true, 1000, 1000 + 10 * 60_000, 10));
        assert!(!within_grace(false, 1000, 1001, 10));
        assert!(!within_grace(true, 1000, 1001, 0));
        assert!(!within_grace(true, 1000, 999, 10), "a clock that went back");
    }

    #[test]
    fn a_grant_is_used_up_by_one_start_and_expires() {
        let (store, clock, dir) = store("grant");
        store.set_pin_with(b"1234", 1_000).unwrap();
        store.set_mode(Mode::Streams);
        let mut gate = PinGate::new(&store);
        assert!(gate.needs_pin_for_stream(&store, "pc"));
        gate.grant(&store, "PC");
        assert!(!gate.needs_pin_for_stream(&store, "pc"), "granted");
        assert!(gate.needs_pin_for_stream(&store, "pc"), "used up");
        gate.grant(&store, "pc");
        assert!(gate.needs_pin_for_stream(&store, "other"), "for that PC only");
        clock.now.fetch_add(GRANT_MS, Ordering::SeqCst);
        assert!(gate.needs_pin_for_stream(&store, "pc"), "expired");
        // A PC that gave no unique id is asked and granted under Nightjar's id for it.
        let (no_id, other) = (stream_key("", 5), stream_key("", 6));
        assert_eq!(stream_key("UID", 5), "UID");
        assert!(gate.needs_pin_for_stream(&store, &no_id));
        gate.grant(&store, &no_id);
        assert!(gate.needs_pin_for_stream(&store, &other), "not for another PC without one");
        gate.grant(&store, &no_id);
        assert!(!gate.needs_pin_for_stream(&store, &no_id), "the right PIN lets it through");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn leaving_the_window_for_longer_than_the_grace_time_locks_the_app() {
        let (store, clock, dir) = store("grace");
        store.set_pin_with(b"1234", 1_000).unwrap();
        store.set_mode(Mode::App);
        let mut gate = PinGate::new(&store);
        assert!(gate.needs_pin_for_app(&store), "locked at start");
        gate.unlock_app();
        assert!(!gate.needs_pin_for_app(&store));
        gate.left(&store);
        clock.now.fetch_add(9 * 60_000, Ordering::SeqCst);
        gate.returned(&store);
        assert!(!gate.needs_pin_for_app(&store), "back within ten minutes");
        gate.left(&store);
        // A restart of Nightjar within the grace time stays unlocked, as on Android.
        assert!(!PinGate::new(&store).needs_pin_for_app(&store));
        clock.now.fetch_add(10 * 60_000, Ordering::SeqCst);
        assert!(PinGate::new(&store).needs_pin_for_app(&store), "but not after it");
        gate.returned(&store);
        assert!(gate.needs_pin_for_app(&store), "away ten minutes: locked");
        // Every time Nightjar is left.
        store.set_grace_minutes(0);
        gate.unlock_app();
        gate.left(&store);
        gate.returned(&store);
        assert!(gate.needs_pin_for_app(&store));
        std::fs::remove_dir_all(dir).ok();
    }
}
