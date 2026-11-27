//! Unit tests for packed 4:4:4 reprobing on settings saves, stream interruption, and user sign-in waits.

use crate::state::test_support::Fixture;
use butterpollo_config::config::Config;
use butterpollo_media_policy::rtsp::Negotiated;
use butterpollo_win_graphics::codec_probe::{Interrupted, SessionNotReady};
use rubylight_protocol::packed444::mode_bit;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// H.264, HEVC and AV1 found by the startup probe.
const CODECS: u32 = 0x1 | 0x100 | 0x200 | 0x10000 | 0x20000;

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn save(f: &Fixture, key: &str, value: &str, probe: crate::state::PackedProbe) {
    let before = f.host.config.read().unwrap().clone();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert(key.into(), value.into());
    f.host.packed_settings_saved_with(&before, probe);
}
fn offered(f: &Fixture) -> u8 {
    f.host.packed_modes.load(Ordering::Acquire)
}
/// A host with full chroma saved off: it is on by default, and these
/// tests turn it on with a save.
fn off() -> Fixture {
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("packed_444".into(), "false".into());
    f
}

#[test]
fn a_stream_that_starts_stops_the_probe_and_it_runs_again_after() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    static PROBING: AtomicBool = AtomicBool::new(false);
    fn probe(_: &Config, _: &Negotiated, stop: &dyn Fn() -> bool) -> anyhow::Result<()> {
        if CALLS.fetch_add(1, Ordering::AcqRel) > 0 {
            return Ok(());
        }
        PROBING.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !stop() {
            anyhow::ensure!(
                Instant::now() < deadline,
                "the stream did not stop the probe"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(Interrupted.into())
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    save(&f, "packed_444", "true", probe);
    until("the probe started", || PROBING.load(Ordering::Acquire));
    f.host
        .sessions
        .lock()
        .unwrap()
        .teardown
        .insert("stream".into(), Instant::now());
    until("the probe stopped", || {
        f.host.packed_probe_pending.load(Ordering::Acquire)
            == rubylight_protocol::packed444::ALL_MODES
    });
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        CALLS.load(Ordering::Acquire),
        1,
        "no probe while the stream runs"
    );
    assert_eq!(offered(&f), 0);
    f.host.sessions.lock().unwrap().teardown.clear();
    until("the modes are probed after the stream", || {
        offered(&f) == rubylight_protocol::packed444::ALL_MODES
    });
}

/// A save that turns a mode off while the probe runs narrows what it
/// finds instead of throwing it away; one that turns a mode on gets a
/// probe of its own.
#[test]
fn saves_during_the_probe_narrow_its_result_or_probe_again() {
    static PROBING: AtomicBool = AtomicBool::new(false);
    static RELEASE: AtomicBool = AtomicBool::new(false);
    fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        if !PROBING.swap(true, Ordering::AcqRel) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !RELEASE.load(Ordering::Acquire) {
                anyhow::ensure!(Instant::now() < deadline, "never released");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("hevc_mode".into(), "1".into());
    save(&f, "packed_444", "true", probe);
    until("the probe started", || PROBING.load(Ordering::Acquire));
    // AV1 SDR only: the running probe skips 10-bit AV1.
    save(&f, "av1_mode", "2", probe);
    save(&f, "hevc_mode", "0", probe);
    RELEASE.store(true, Ordering::Release);
    let expected = mode_bit(1, false) | mode_bit(1, true) | mode_bit(2, false);
    until("the narrowed and follow-up results are offered", || {
        offered(&f) == expected
    });
    until("the follow-up probe finished", || {
        !f.host.packed_reprobing.load(Ordering::Acquire)
    });
    assert_eq!(offered(&f), expected);
}

/// Turning HEVC on while the host runs offers no packed HEVC: clients
/// see HEVC in ServerCodecModeSupport only after a restart.
#[test]
fn packed_modes_of_a_codec_not_offered_stay_off() {
    static AV1_PROBED: AtomicBool = AtomicBool::new(false);
    fn probe(_: &Config, stream: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        if stream.codec == 2 {
            AV1_PROBED.store(true, Ordering::Release);
        }
        Ok(())
    }
    let f = off();
    // The startup probe found no AV1.
    f.host.codecs.store(0x1 | 0x100 | 0x200, Ordering::Release);
    save(&f, "packed_444", "true", probe);
    until("the HEVC modes are probed", || {
        offered(&f) == mode_bit(1, false) | mode_bit(1, true)
    });
    until("the probe finished", || {
        !f.host.packed_reprobing.load(Ordering::Acquire)
    });
    assert!(!AV1_PROBED.load(Ordering::Acquire));
}

fn logged(path: &std::path::Path, run: impl FnOnce()) -> String {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::sync::Mutex::new(std::fs::File::create(path).unwrap()))
        .finish();
    crate::test_log::capture_during(subscriber, run);
    std::fs::read_to_string(path).unwrap()
}

/// A save that allows only modes of a codec the startup probe did not
/// find probes nothing, keeps what is offered and says once why.
#[test]
fn a_save_that_allows_only_modes_of_a_missing_codec_probes_nothing() {
    fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        panic!("nothing to probe")
    }
    let f = off();
    // HEVC was off at startup: only AV1 was found and offered.
    f.host
        .codecs
        .store(0x1 | 0x10000 | 0x20000, Ordering::Release);
    let av1 = mode_bit(2, false) | mode_bit(2, true);
    {
        let mut config = f.host.config.write().unwrap();
        config.values.insert("packed_444".into(), "true".into());
        config.values.insert("hevc_mode".into(), "1".into());
    }
    f.host.packed_modes.store(av1, Ordering::Release);
    let path = f.host.directory.join("missing.log");
    let log = logged(&path, || {
        save(&f, "hevc_mode", "0", probe);
        save(&f, "hevc_mode", "1", probe);
        save(&f, "hevc_mode", "0", probe);
    });
    assert!(!f.host.packed_reprobing.load(Ordering::Acquire));
    assert_eq!(offered(&f), av1);
    for mode in ["HEVC 8-bit", "HEVC 10-bit"] {
        assert_eq!(log.matches(&format!("mode=\"{mode}\"")).count(), 1, "{log}");
    }
    assert!(
        log.contains("HEVC not found by the startup probe; restart the host"),
        "{log}"
    );
}

/// While the startup probe found no standard codec and retries, a
/// save leaves the packed modes to the next pass: no probe, and no
/// restart warning that would use up the one a missing codec gets.
#[test]
fn a_save_before_any_codec_is_found_leaves_the_modes_to_the_next_pass() {
    fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        panic!("the next codec pass probes them")
    }
    let f = off();
    f.host.codecs.store(0, Ordering::Release);
    let path = f.host.directory.join("no-codec.log");
    let log = logged(&path, || save(&f, "packed_444", "true", probe));
    assert!(log.is_empty(), "{log}");
    assert!(!f.host.packed_reprobing.load(Ordering::Acquire));
    assert_eq!(f.host.packed_probe_pending.load(Ordering::Acquire), 0);
    assert_eq!(offered(&f), 0);
    // The next pass finds AV1 but not HEVC: the warning is still due.
    f.host
        .codecs
        .store(0x1 | 0x10000 | 0x20000, Ordering::Release);
    f.host
        .packed_modes
        .store(mode_bit(2, false) | mode_bit(2, true), Ordering::Release);
    save(&f, "hevc_mode", "1", probe);
    let path = f.host.directory.join("hevc-missing.log");
    let log = logged(&path, || save(&f, "hevc_mode", "0", probe));
    assert!(
        log.contains("HEVC not found by the startup probe; restart the host"),
        "{log}"
    );
}

/// A save that newly allows a mode probes only it: a mode already
/// offered is neither probed again nor withdrawn by a failed probe.
#[test]
fn a_save_probes_only_the_modes_it_newly_allows() {
    static AV1_PROBED: AtomicBool = AtomicBool::new(false);
    fn probe(_: &Config, stream: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        if stream.codec == 2 {
            AV1_PROBED.store(true, Ordering::Release);
            anyhow::bail!("timed out under a game");
        }
        Ok(())
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    {
        let mut config = f.host.config.write().unwrap();
        config.values.insert("packed_444".into(), "true".into());
        config.values.insert("hevc_mode".into(), "2".into());
    }
    let offered_before = mode_bit(1, false) | mode_bit(2, false) | mode_bit(2, true);
    f.host.packed_modes.store(offered_before, Ordering::Release);
    save(&f, "hevc_mode", "0", probe);
    until("HEVC 10-bit is probed", || {
        offered(&f) == rubylight_protocol::packed444::ALL_MODES
    });
    until("the probe finished", || {
        !f.host.packed_reprobing.load(Ordering::Acquire)
    });
    assert!(!AV1_PROBED.load(Ordering::Acquire));
    assert_eq!(offered(&f), rubylight_protocol::packed444::ALL_MODES);
}

/// Turning full chroma off while its probe runs stops the probe at
/// once and probes nothing after.
#[test]
fn turning_full_chroma_off_stops_its_running_probe() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    static PROBING: AtomicBool = AtomicBool::new(false);
    static STOPPED: AtomicBool = AtomicBool::new(false);
    fn probe(_: &Config, _: &Negotiated, stop: &dyn Fn() -> bool) -> anyhow::Result<()> {
        CALLS.fetch_add(1, Ordering::AcqRel);
        PROBING.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !stop() {
            anyhow::ensure!(Instant::now() < deadline, "the save did not stop the probe");
            std::thread::sleep(Duration::from_millis(5));
        }
        STOPPED.store(true, Ordering::Release);
        Err(Interrupted.into())
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    save(&f, "packed_444", "true", probe);
    until("the probe started", || PROBING.load(Ordering::Acquire));
    save(&f, "packed_444", "false", probe);
    until("the probe finished", || {
        !f.host.packed_reprobing.load(Ordering::Acquire)
    });
    assert!(STOPPED.load(Ordering::Acquire));
    assert_eq!(CALLS.load(Ordering::Acquire), 1);
    assert_eq!(f.host.packed_probe_pending.load(Ordering::Acquire), 0);
    assert_eq!(offered(&f), 0);
}

fn packed_on(f: &Fixture) -> Config {
    let mut config = f.host.config.write().unwrap();
    config.values.insert("packed_444".into(), "true".into());
    config.clone()
}

/// A codec pass before anyone signs in, as when the host starts with
/// Windows, leaves the packed modes pending instead of failing them,
/// and they are probed and offered once a user is signed in. Other
/// errors stay final.
#[test]
fn a_probe_that_needs_a_signed_in_user_runs_again_after_sign_in() {
    static SIGNED_IN: AtomicBool = AtomicBool::new(false);
    fn probe(_: &Config, stream: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        if !SIGNED_IN.load(Ordering::Acquire) {
            return Err(SessionNotReady::new("helper needs a signed-in user").into());
        }
        anyhow::ensure!(!(stream.codec == 2 && stream.sdr_10bit), "driver refused");
        Ok(())
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    let config = packed_on(&f);
    f.host.pass_packed_probe(&config, CODECS, &|| false, probe);
    let all = rubylight_protocol::packed444::ALL_MODES;
    assert_eq!(f.host.packed_probe_pending.load(Ordering::Acquire), all);
    assert_eq!(offered(&f), 0);
    SIGNED_IN.store(true, Ordering::Release);
    f.host.start_packed_probe(probe);
    until("the modes are probed after sign-in", || {
        !f.host.packed_reprobing.load(Ordering::Acquire)
            && f.host.packed_probe_pending.load(Ordering::Acquire) == 0
    });
    assert_eq!(offered(&f), all & !mode_bit(2, true));
    assert_eq!(f.host.packed_session_waits.load(Ordering::Acquire), 0);
}

/// A launch is ready once the standard codecs are published: it never waits for the packed
/// 4:4:4 probe, which a launch does not ask for. `/serverinfo`, which offers the modes,
/// waits for the probe up to its deadline.
#[test]
fn launch_readiness_does_not_wait_for_the_packed_probe() {
    const PROBE: Duration = Duration::from_millis(1500);
    static SLEPT: AtomicBool = AtomicBool::new(false);
    SLEPT.store(false, Ordering::Release);
    fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        if !SLEPT.swap(true, Ordering::AcqRel) {
            std::thread::sleep(PROBE);
        }
        Ok(())
    }
    let f = off();
    f.host.startup_codecs.send_replace(false);
    f.host.startup_packed.send_replace(false);
    f.host.codecs.store(CODECS, Ordering::Release);
    let config = packed_on(&f);
    let began = Instant::now();
    let pass = {
        let host = f.host.clone();
        std::thread::spawn(move || host.publish_standard_codecs(&config, CODECS, probe))
    };
    f.host
        .wait_for_startup_codecs_blocking(Duration::from_secs(5));
    let launch_ready = began.elapsed();
    assert!(*f.host.startup_codecs.borrow());
    assert!(
        !*f.host.startup_packed.borrow(),
        "the packed probe is still running"
    );
    assert!(launch_ready < PROBE / 2, "a launch waited {launch_ready:?}");
    // A `/serverinfo` that cannot wait that long offers what is known.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let short = Instant::now();
    runtime.block_on(
        f.host
            .wait_for_startup_packed(Instant::now() + Duration::from_millis(100)),
    );
    assert!(short.elapsed() < PROBE / 2);
    assert!(!*f.host.startup_packed.borrow());
    // One that can wait gets every mode once the probe has finished.
    runtime.block_on(
        f.host
            .wait_for_startup_packed(Instant::now() + Duration::from_secs(10)),
    );
    let serverinfo_ready = began.elapsed();
    pass.join().unwrap();
    assert!(serverinfo_ready >= PROBE, "{serverinfo_ready:?}");
    assert_ne!(offered(&f), 0);
    eprintln!("launch ready after {launch_ready:?}, serverinfo after {serverinfo_ready:?}");
}

/// A launch queued while the packed probe runs has the encoder: the probe yields and
/// leaves its modes pending for after the stream.
#[test]
fn the_packed_probe_yields_to_a_launch_that_starts_meanwhile() {
    fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        unreachable!("no mode is probed while a stream starts");
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    let config = packed_on(&f);
    // A stream is starting or ending: the sessions are not idle.
    f.host
        .sessions
        .lock()
        .unwrap()
        .teardown
        .insert("stream".into(), Instant::now());
    f.host.publish_standard_codecs(&config, CODECS, probe);
    assert!(*f.host.startup_codecs.borrow() && *f.host.startup_packed.borrow());
    assert_eq!(
        f.host.packed_probe_pending.load(Ordering::Acquire),
        rubylight_protocol::packed444::ALL_MODES
    );
}

/// Only the modes that need a session wait: a mode that fails
/// otherwise is final at once, and one that passes is offered.
#[test]
fn only_modes_that_need_a_session_stay_pending() {
    fn probe(_: &Config, stream: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        match (stream.codec, stream.sdr_10bit) {
            (1, false) => Err(SessionNotReady::new("helper needs a signed-in user").into()),
            (1, true) => anyhow::bail!("driver refused"),
            _ => Ok(()),
        }
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    let config = packed_on(&f);
    f.host.pass_packed_probe(&config, CODECS, &|| false, probe);
    assert_eq!(
        f.host.packed_probe_pending.load(Ordering::Acquire),
        mode_bit(1, false)
    );
    assert_eq!(offered(&f), mode_bit(2, false) | mode_bit(2, true));
}

/// A session that never becomes ready fails the modes after
/// `PACKED_SESSION_TRIES` probes, loudly, instead of probing forever.
#[test]
fn a_session_that_never_becomes_ready_fails_after_its_tries() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    fn probe(_: &Config, _: &Negotiated, _: &dyn Fn() -> bool) -> anyhow::Result<()> {
        CALLS.fetch_add(1, Ordering::AcqRel);
        Err(SessionNotReady::new("start optional codec probe: no pipe").into())
    }
    let f = off();
    f.host.codecs.store(CODECS, Ordering::Release);
    let config = packed_on(&f);
    let path = f.host.directory.join("never-ready.log");
    // On this thread, which the log capture covers.
    let log = logged(&path, || {
        f.host.pass_packed_probe(&config, CODECS, &|| false, probe);
        for _ in 0..20 {
            if f.host.packed_probe_pending.load(Ordering::Acquire) == 0 {
                break;
            }
            f.host.reprobe_packed_modes(probe);
        }
    });
    assert_eq!(f.host.packed_probe_pending.load(Ordering::Acquire), 0);
    let tries = u32::from(crate::state::PACKED_SESSION_TRIES);
    assert_eq!(CALLS.load(Ordering::Acquire), tries * 4);
    assert_eq!(offered(&f), 0);
    assert_eq!(log.matches("never became ready").count(), 1, "{log}");
}
