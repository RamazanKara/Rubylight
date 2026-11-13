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
fn continue_heres_press_leaves_the_grant_to_the_stream_it_starts() {
    let (store, _clock, dir) = store("press");
    store.set_pin_with(b"1234", 1_000).unwrap();
    store.set_mode(Mode::Streams);
    let mut gate = PinGate::new(&store);
    assert!(gate.locks_stream(&store, "pc"), "the press waits for the PIN");
    gate.grant(&store, "pc");
    assert!(!gate.locks_stream(&store, "pc"), "the PIN passed: the press goes on");
    assert!(!gate.locks_stream(&store, "pc"), "and asking did not use it up");
    assert!(!gate.needs_pin_for_stream(&store, "pc"), "the stream after the press does not ask again");
    assert!(gate.locks_stream(&store, "pc"), "the next press asks again");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_press_that_failed_leaves_no_grant() {
    let (store, _clock, dir) = store("grant-forgotten");
    store.set_pin_with(b"1234", 1_000).unwrap();
    store.set_mode(Mode::Streams);
    let mut gate = PinGate::new(&store);
    gate.grant(&store, "pc");
    assert!(!gate.locks_stream(&store, "pc"));
    gate.forget_grant();
    assert!(gate.locks_stream(&store, "pc"), "the next start asks");
    assert!(gate.needs_pin_for_stream(&store, "pc"));
    let _ = std::fs::remove_dir_all(dir);
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

/// The second screen's process, started by an unlocked first, asks for no PIN: the first hands
/// it a token over its input, which only the hash in the file vouches for, briefly.
#[test]
fn a_hand_over_opens_the_second_screens_lock_once_written_and_only_briefly() {
    let (first, clock, dir) = store("hand-over");
    assert_eq!(first.hand_over(), Ok(None), "no PIN, nothing to hand over");
    first.set_pin_with(b"1234", 1_000).unwrap();
    first.set_mode(Mode::Streams);
    first.flush();
    let token = first.hand_over().unwrap().expect("a token");
    assert_eq!(token.len(), 64);
    let file = std::fs::read_to_string(dir.join(FILE)).unwrap();
    assert!(!file.contains(&token), "the file keeps the token's hash, never the token");
    assert_ne!(first.hand_over().unwrap(), Some(token.clone()), "every hand-over has a fresh token");
    let token = first.hand_over().unwrap().unwrap();

    // The second process reads the file as it starts.
    let second = PinStore::new(&dir, clock.clone());
    for wrong in ["", "00", &token[1..], &token.to_uppercase()] {
        assert!(!second.handed_over(wrong), "{wrong:?}");
    }
    assert!(second.handed_over(&token));
    let mut gate = PinGate::new(&second);
    assert!(gate.needs_pin_for_stream(&second, "UID"), "without it the stream asks");
    gate.hand_over();
    second.set_mode(Mode::App);
    assert!(!gate.needs_pin_for_app(&second) && !gate.needs_pin_for_stream(&second, "UID") && !gate.needs_pin_for_stream(&second, "UID"));
    second.flush();
    let before = std::fs::read(dir.join(FILE)).unwrap();
    gate.left(&second);
    second.flush();
    assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), before, "leaving it writes nothing: the first keeps the file");
    clock.now.fetch_add(60 * 60_000, Ordering::SeqCst);
    gate.returned(&second);
    assert!(!gate.needs_pin_for_app(&second), "it ends with the first's stream, never locks over it");
    clock.now.fetch_sub(60 * 60_000, Ordering::SeqCst);

    clock.now.fetch_add(HAND_OVER_MS, Ordering::SeqCst);
    assert!(!PinStore::new(&dir, clock.clone()).handed_over(&token), "too late");
    clock.now.fetch_sub(HAND_OVER_MS, Ordering::SeqCst);
    clock.boot.store(8, Ordering::SeqCst);
    assert!(!PinStore::new(&dir, clock.clone()).handed_over(&token), "another boot");
    clock.boot.store(7, Ordering::SeqCst);
    assert!(PinStore::new(&dir, clock.clone()).handed_over(&token));

    first.end_hand_over();
    first.flush();
    assert!(!PinStore::new(&dir, clock.clone()).handed_over(&token), "the second screen is gone");
    let token = first.hand_over().unwrap().unwrap();
    first.remove_pin();
    first.flush();
    assert!(!PinStore::new(&dir, clock).handed_over(&token), "no PIN, no hand-over");
    std::fs::remove_dir_all(dir).ok();
}
