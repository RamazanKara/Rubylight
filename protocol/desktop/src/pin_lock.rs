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
use std::sync::{Arc, Mutex};

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

    /// The state seen from boot `boot`: a lock written on another boot is renewed at its step.
    pub fn on_boot(s: State, now: i64, boot: i64) -> State {
        if s.lock_boot == boot {
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
    /// A number that changes on every start.
    fn boot(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn elapsed(&self) -> i64 {
        uptime_ms()
    }

    fn boot(&self) -> i64 {
        let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        boot_estimate(wall, uptime_ms())
    }
}

#[cfg(windows)]
fn uptime_ms() -> i64 {
    // SAFETY: no arguments; counts since boot, sleep included.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() as i64 }
}

#[cfg(not(windows))]
fn uptime_ms() -> i64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid timespec. CLOCK_BOOTTIME counts suspend too, like Android's elapsedRealtime.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut t) } != 0 {
        panic!("CLOCK_BOOTTIME unavailable: {}", std::io::Error::last_os_error());
    }
    t.tv_sec as i64 * 1000 + t.tv_nsec as i64 / 1_000_000
}

/// Boot time in 10 s steps; a clock change moves it, but then the lock is only renewed, never dropped.
pub fn boot_estimate(wall_clock_ms: i64, elapsed_ms: i64) -> i64 {
    ((wall_clock_ms - elapsed_ms) as f64 / 10_000.0).round() as i64
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
}

/// One try at a time, across every store in the process.
static TRY_LOCK: Mutex<()> = Mutex::new(());

/// The lock's file. Every call reads or writes it, so clones on a worker thread stay in step.
#[derive(Clone)]
pub struct PinStore {
    path: PathBuf,
    clock: Arc<dyn Clock>,
}

impl PinStore {
    pub fn new(dir: &Path, clock: Arc<dyn Clock>) -> Self {
        Self { path: dir.join(FILE), clock }
    }

    /// The saved state. A file that is there but cannot be read keeps the lock on with no PIN that
    /// matches (fail closed): deleting the file is the way out, as the settings page says.
    fn load(&self) -> Saved {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Saved::default(),
            Err(e) => return Self::damaged(&self.path, &e.to_string()),
        };
        serde_json::from_str(&text).unwrap_or_else(|e| Self::damaged(&self.path, &e.to_string()))
    }

    fn damaged(path: &Path, why: &str) -> Saved {
        eprintln!("nightjar: the PIN lock file {} is unreadable ({why}); the app stays locked until it is deleted", path.display());
        Saved { hash: Some(String::new()), mode: Mode::App, grace_min: 0, ..Saved::default() }
    }

    fn save(&self, saved: &Saved) {
        let result = serde_json::to_string_pretty(saved).map_err(std::io::Error::other).and_then(|text| {
            let temporary = self.path.with_extension("json.tmp");
            std::fs::write(&temporary, text)?;
            std::fs::rename(&temporary, &self.path)
        });
        if let Err(e) = result {
            eprintln!("nightjar: saving the PIN lock failed: {e}");
        }
    }

    fn edit(&self, change: impl FnOnce(&mut Saved)) {
        let mut saved = self.load();
        change(&mut saved);
        self.save(&saved);
    }

    pub fn has_pin(&self) -> bool {
        self.load().hash.is_some()
    }

    /// What the settings page shows, read once.
    pub fn view(&self) -> View {
        View { has_pin: self.has_pin(), mode: self.mode(), pcs: self.pcs(), grace_minutes: self.grace_minutes() }
    }

    /// Digits in the PIN, for the dots and the automatic submit; 0 without a PIN.
    pub fn pin_length(&self) -> usize {
        let saved = self.load();
        if saved.hash.is_some() { saved.length.clamp(MIN_DIGITS, MAX_DIGITS) } else { 0 }
    }

    /// Mode in effect: without a PIN nothing is locked, whatever the stored mode says.
    pub fn mode(&self) -> Mode {
        let saved = self.load();
        if saved.hash.is_some() { saved.mode } else { Mode::Off }
    }

    pub fn set_mode(&self, mode: Mode) {
        self.edit(|s| s.mode = mode);
    }

    pub fn pcs(&self) -> Vec<String> {
        self.load().pcs
    }

    pub fn set_pcs(&self, pcs: Vec<String>) {
        self.edit(|s| s.pcs = pcs);
    }

    pub fn grace_minutes(&self) -> u32 {
        let minutes = self.load().grace_min;
        if GRACE_CHOICES.contains(&minutes) { minutes } else { DEFAULT_GRACE_MINUTES }
    }

    pub fn set_grace_minutes(&self, minutes: u32) {
        self.edit(|s| s.grace_min = minutes);
    }

    /// Stores a new PIN under a fresh salt and clears the failure count. Slow: measures the computer.
    pub fn set_pin(&self, pin: &[u8]) -> Result<(), String> {
        self.set_pin_with(pin, hasher::calibrate())
    }

    fn set_pin_with(&self, pin: &[u8], iterations: u32) -> Result<(), String> {
        if !(MIN_DIGITS..=MAX_DIGITS).contains(&pin.len()) || !pin.iter().all(u8::is_ascii_digit) {
            return Err(format!("a PIN has {MIN_DIGITS} to {MAX_DIGITS} digits"));
        }
        let hash = hasher::create(pin, iterations)?;
        self.edit(|s| {
            s.hash = Some(hash);
            s.length = pin.len();
            (s.fails, s.lock_until, s.lock_boot) = (0, 0, 0);
        });
        Ok(())
    }

    /// Removes the PIN and everything that depended on it.
    pub fn remove_pin(&self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("nightjar: removing the PIN lock failed: {e}"),
        }
    }

    fn throttle_state(saved: &Saved) -> throttle::State {
        throttle::State { fails: saved.fails, lock_until: saved.lock_until, lock_boot: saved.lock_boot }
    }

    fn put_throttle(saved: &mut Saved, state: throttle::State) {
        (saved.fails, saved.lock_until, saved.lock_boot) = (state.fails, state.lock_until, state.lock_boot);
    }

    /// The stored state seen from this boot; a lock renewed for a new boot is saved so its countdown moves on.
    fn current(&self, now: i64, boot: i64) -> (Saved, throttle::State) {
        let mut saved = self.load();
        let stored = Self::throttle_state(&saved);
        let state = throttle::on_boot(stored, now, boot);
        if state != stored {
            Self::put_throttle(&mut saved, state);
            self.save(&saved);
        }
        (saved, state)
    }

    /// Milliseconds until the next try is allowed, 0 when it is.
    pub fn locked_for_ms(&self) -> i64 {
        let _one = TRY_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = self.clock.elapsed();
        throttle::remaining_ms(self.current(now, self.clock.boot()).1, now)
    }

    /// One try. Refused while locked out; otherwise the failure is written before the PIN is
    /// checked, so ending the app mid-check cannot get a free guess, and a match clears it again.
    /// Slow. Load, count and save run under one lock, so two PIN screens cannot both count
    /// against the same state.
    pub fn try_pin(&self, pin: &[u8]) -> Outcome {
        self.try_pin_with(pin, hasher::verify)
    }

    fn try_pin_with(&self, pin: &[u8], verify: impl FnOnce(&str, &[u8]) -> bool) -> Outcome {
        let _one = TRY_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = self.clock.elapsed();
        let boot = self.clock.boot();
        let (mut saved, state) = self.current(now, boot);
        let locked = throttle::remaining_ms(state, now);
        if locked > 0 {
            return Outcome::Locked { locked_for_ms: locked };
        }
        let counted = throttle::failed(state, now, boot);
        Self::put_throttle(&mut saved, counted);
        self.save(&saved);
        if !verify(saved.hash.as_deref().unwrap_or_default(), pin) {
            return Outcome::Wrong { locked_for_ms: throttle::remaining_ms(counted, self.clock.elapsed()) };
        }
        Self::put_throttle(&mut saved, throttle::State::default());
        self.save(&saved);
        Outcome::Ok
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
}

/// How long the PIN screen's approval of one PC's stream start stays valid.
pub const GRANT_MS: i64 = 60_000;

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

pub fn stream_needed(pin_set: bool, mode: Mode, pcs: &[String], uuid: &str, unlocked: bool, grant_valid: bool) -> bool {
    if app_needed(pin_set, mode, unlocked) {
        return true;
    }
    if !pin_set {
        return false;
    }
    let asks = mode == Mode::Streams || (mode == Mode::Selected && contains(pcs, uuid));
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
        let saved = store.load();
        let unlocked_app = saved.hash.is_some()
            && saved.mode == Mode::App
            && within_grace(saved.active_boot == store.boot(), saved.active_at, store.elapsed(), store.grace_minutes());
        Self { unlocked_app, grant: None, left_at: None }
    }

    /// The app is locked and nothing has unlocked it.
    pub fn needs_pin_for_app(&self, store: &PinStore) -> bool {
        app_needed(store.has_pin(), store.mode(), self.unlocked_app)
    }

    /// Whether starting a stream on `uuid` must first go through the PIN screen. A yes answer
    /// from the PIN screen ([`Self::grant`]) is used up by the start it was given for.
    pub fn needs_pin_for_stream(&mut self, store: &PinStore, uuid: &str) -> bool {
        let now = store.elapsed();
        let granted = self.grant.as_ref().is_some_and(|(pc, at)| !uuid.is_empty() && pc.eq_ignore_ascii_case(uuid) && (0..GRANT_MS).contains(&(now - at)));
        let needed = stream_needed(store.has_pin(), store.mode(), &store.pcs(), uuid, self.unlocked_app, granted);
        if !needed && granted {
            self.grant = None;
        }
        needed
    }

    /// The PIN was entered for starting a stream on `uuid`.
    pub fn grant(&mut self, store: &PinStore, uuid: &str) {
        self.grant = Some((uuid.to_owned(), store.elapsed()));
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
        assert_eq!(throttle::on_boot(locked, 5, 2), throttle::State { fails: 5, lock_until: 30_005, lock_boot: 2 });
        assert_eq!(throttle::on_boot(locked, 5, 1), locked, "same boot: as stored");
    }

    #[test]
    fn the_store_locks_out_after_wrong_tries_and_a_right_one_clears_it() {
        let (store, clock, dir) = store("lockout");
        assert!(!store.has_pin() && store.mode() == Mode::Off);
        store.set_pin_with(b"4321", 1_000).unwrap();
        store.set_mode(Mode::App);
        assert_eq!((store.pin_length(), store.mode()), (4, Mode::App));
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
        let fails_during_check = std::cell::Cell::new(0);
        let outcome = store.try_pin_with(b"1234", |hash, pin| {
            fails_during_check.set(store.load().fails);
            hasher::verify(hash, pin)
        });
        assert_eq!(outcome, Outcome::Ok);
        assert_eq!(fails_during_check.get(), 1, "a kill during the check cannot give a free guess");
        assert_eq!(store.load().fails, 0, "and a match clears it");
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
        assert_eq!((store.mode(), store.grace_minutes(), store.pcs().len()), (Mode::Off, DEFAULT_GRACE_MINUTES, 0), "everything that depended on it goes");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_damaged_file_keeps_the_app_locked() {
        let (store, _, dir) = store("damaged");
        std::fs::write(dir.join(FILE), "{ not json").unwrap();
        assert!(store.has_pin() && store.mode() == Mode::App);
        let gate = PinGate::new(&store);
        assert!(gate.needs_pin_for_app(&store));
        assert!(matches!(store.try_pin(b"1234"), Outcome::Wrong { .. }), "no PIN opens it");
        std::fs::remove_dir_all(dir).ok();
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
