#[test]
fn the_config_snapshot_is_shared_while_unchanged_and_follows_every_change() {
    use std::sync::Arc;
    let f = super::test_support::Fixture::new();
    let first = f.host.config_snapshot();
    let name = first.get("sunshine_name", "").to_owned();
    assert!(Arc::ptr_eq(&first, &f.host.config_snapshot()));
    assert_eq!(first.values, f.host.config.read().unwrap().values);
    // Any change, also one written straight into the lock, shows at once.
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("sunshine_name".into(), "Den".into());
    let changed = f.host.config_snapshot();
    assert!(!Arc::ptr_eq(&first, &changed));
    assert_eq!(crate::network::host_name(&changed), "Den");
    assert_eq!(changed.values, f.host.config.read().unwrap().values);
    assert!(Arc::ptr_eq(&changed, &f.host.config_snapshot()));
    // What a request still holds stays as it was.
    assert_eq!(first.get("sunshine_name", ""), name);
}
#[test]
fn a_focus_check_error_marks_only_that_codec_unsupported() {
    use butterpollo_core::encoder_focus::{Reason, Support};
    let mut support: [Support; 5] = std::array::from_fn(|_| Support::NotProbed);
    support[0] = focus_verdict(Ok(Support::Verified { delta: 10.0 }));
    support[1] = focus_verdict(Err(anyhow::anyhow!("frame 3 was not encoded")));
    support[3] = focus_verdict(Ok(Support::Verified { delta: 50.0 }));
    assert_eq!(
        support[1],
        Support::Unsupported {
            reason: Reason::CodecUnsupported,
            message: "the focus check failed: frame 3 was not encoded".into(),
        }
    );
    assert_eq!(
        focus_summary(&support),
        "h264=verified(+10.0) hevc=unsupported(the focus check failed: frame 3 was not encoded) hevc10=not_probed av1=verified(+50.0) av1_10=not_probed"
    );
}
#[test]
fn a_timed_out_early_or_crashed_pyrowave_probe_is_probed_again() {
    use butterpollo_windows::codec_probe::{Crashed, SessionNotReady, TimedOut};
    use std::time::Duration;
    // A probe that stalled under a busy CPU left PyroWave unoffered
    // until the host restarted. The load that stalled it, such as a
    // game, is given a minute to pass.
    assert_eq!(
        super::pyrowave_retry_delay(&TimedOut.into()),
        Some(Duration::from_secs(60))
    );
    assert_eq!(
        super::pyrowave_retry_delay(&SessionNotReady::new("no user").into()),
        Some(Duration::from_secs(5))
    );
    // Issue #94: a worker that crashed on half of one PC's starts.
    assert_eq!(
        super::pyrowave_retry_delay(&Crashed::new("exited before replying (0xc0000005)").into()),
        Some(Duration::from_secs(5))
    );
    assert_eq!(
        super::pyrowave_retry_delay(&anyhow::anyhow!("loading libpyrowave-shared-1.dll")),
        None
    );
}

/// PyroWave probed again after the startup probe: a timeout is tried once
/// more only, a session not ready yet until it is, and a stream that
/// starts stops the probe until it has ended.
mod pyrowave_retry {
    use crate::state::test_support::Fixture;
    use butterpollo_core::config::Config;
    use butterpollo_windows::codec_probe::{Crashed, Interrupted, SessionNotReady, TimedOut};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    const PYROWAVE: u32 = 0x0080_0000;

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_timeout_is_probed_again_exactly_once() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
            CALLS.fetch_add(1, Ordering::AcqRel);
            Err(TimedOut.into())
        }
        let f = Fixture::new();
        let config = f.host.config.read().unwrap().clone();
        assert_eq!(
            f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
            1
        );
        assert_eq!(CALLS.load(Ordering::Acquire), 1);
        assert_eq!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
    }

    /// Issue #94: the startup probe and two more before PyroWave stays
    /// off, and a later one that passes offers it.
    #[test]
    fn a_crashed_worker_is_probed_twice_more() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        fn crashes(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
            CALLS.fetch_add(1, Ordering::AcqRel);
            Err(Crashed::new("exited before replying (0xc0000005)").into())
        }
        let f = Fixture::new();
        let config = f.host.config.read().unwrap().clone();
        assert_eq!(
            f.host
                .retry_pyrowave_probe(&config, Duration::ZERO, crashes),
            2
        );
        assert_eq!(CALLS.load(Ordering::Acquire), 2);
        assert_eq!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);

        static SECOND: AtomicU32 = AtomicU32::new(0);
        fn crashes_once(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
            if SECOND.fetch_add(1, Ordering::AcqRel) == 0 {
                return Err(Crashed::new("exited before replying (0xc0000005)").into());
            }
            Ok(PYROWAVE)
        }
        let f = Fixture::new();
        assert_eq!(
            f.host
                .retry_pyrowave_probe(&config, Duration::ZERO, crashes_once),
            2
        );
        assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
    }

    #[test]
    fn a_session_not_ready_is_probed_until_it_is() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
            if CALLS.fetch_add(1, Ordering::AcqRel) < 2 {
                return Err(SessionNotReady::new("nobody is signed in").into());
            }
            Ok(PYROWAVE)
        }
        let f = Fixture::new();
        let config = f.host.config.read().unwrap().clone();
        assert_eq!(
            f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
            3
        );
        assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
    }

    #[test]
    fn a_stream_that_starts_stops_the_probe_until_it_ends() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        static PROBING: AtomicBool = AtomicBool::new(false);
        fn probe(_: &Config, stop: &dyn Fn() -> bool) -> anyhow::Result<u32> {
            if CALLS.fetch_add(1, Ordering::AcqRel) > 0 {
                return Ok(PYROWAVE);
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
        let f = Fixture::new();
        let config = f.host.config.read().unwrap().clone();
        let host = f.host.clone();
        let retry =
            std::thread::spawn(move || host.retry_pyrowave_probe(&config, Duration::ZERO, probe));
        until("the probe started", || PROBING.load(Ordering::Acquire));
        f.host
            .sessions
            .lock()
            .unwrap()
            .teardown
            .insert("stream".into(), Instant::now());
        until("the stream stopped the probe", || {
            f.host.optional_probe.try_lock().is_ok()
        });
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            CALLS.load(Ordering::Acquire),
            1,
            "no probe while the stream runs"
        );
        f.host.sessions.lock().unwrap().teardown.clear();
        // The stopped probe was no attempt.
        assert_eq!(retry.join().unwrap(), 1);
        assert_eq!(CALLS.load(Ordering::Acquire), 2);
        assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
    }

    /// Streams that stop the probe use up none of its six attempts: one
    /// more stream no longer leaves PyroWave off until a restart.
    #[test]
    fn streams_that_stop_the_probe_use_up_no_attempt() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
            if CALLS.fetch_add(1, Ordering::AcqRel) < 7 {
                return Err(Interrupted.into());
            }
            Ok(PYROWAVE)
        }
        let f = Fixture::new();
        let config = f.host.config.read().unwrap().clone();
        assert_eq!(
            f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
            1
        );
        assert_eq!(CALLS.load(Ordering::Acquire), 8);
        assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
    }

    /// A stream that stops a codec pass's PyroWave probe, or starts
    /// before it, completes the pass with the PyroWave modes found before
    /// and a probe after the stream: the "No video encoder" retry ends
    /// and the standard codecs are not probed again.
    #[test]
    fn a_stream_that_stops_the_pass_probe_keeps_its_modes_and_probes_after() {
        let (flags, retry) = crate::state::pyrowave_pass(Err(Interrupted.into()), PYROWAVE, true);
        assert_eq!((flags, retry), (PYROWAVE, Some(Duration::from_secs(5))));
        let (flags, retry) = crate::state::pyrowave_pass(Ok(PYROWAVE), 0, true);
        assert_eq!((flags, retry), (PYROWAVE, None));
        let (flags, retry) = crate::state::pyrowave_pass(Err(TimedOut.into()), PYROWAVE, true);
        assert_eq!((flags, retry), (0, Some(Duration::from_secs(60))));
        let failed = crate::state::pyrowave_pass(Err(anyhow::anyhow!("no DLL")), PYROWAVE, true);
        assert_eq!(failed, (0, None));
    }
}

/// Packed 4:4:4 probed again after a save, with saves and streams that
/// land while the probe runs.
mod packed_reprobe {
    use crate::state::test_support::Fixture;
    use butterpollo_core::{config::Config, packed444::mode_bit, rtsp::Negotiated};
    use butterpollo_windows::codec_probe::{Interrupted, SessionNotReady};
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
            .insert("experimental_packed_444".into(), "false".into());
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
        save(&f, "experimental_packed_444", "true", probe);
        until("the probe started", || PROBING.load(Ordering::Acquire));
        f.host
            .sessions
            .lock()
            .unwrap()
            .teardown
            .insert("stream".into(), Instant::now());
        until("the probe stopped", || {
            f.host.packed_probe_pending.load(Ordering::Acquire)
                == butterpollo_core::packed444::ALL_MODES
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
            offered(&f) == butterpollo_core::packed444::ALL_MODES
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
        save(&f, "experimental_packed_444", "true", probe);
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
        save(&f, "experimental_packed_444", "true", probe);
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
            config
                .values
                .insert("experimental_packed_444".into(), "true".into());
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
        let log = logged(&path, || save(&f, "experimental_packed_444", "true", probe));
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
            config
                .values
                .insert("experimental_packed_444".into(), "true".into());
            config.values.insert("hevc_mode".into(), "2".into());
        }
        let offered_before = mode_bit(1, false) | mode_bit(2, false) | mode_bit(2, true);
        f.host.packed_modes.store(offered_before, Ordering::Release);
        save(&f, "hevc_mode", "0", probe);
        until("HEVC 10-bit is probed", || {
            offered(&f) == butterpollo_core::packed444::ALL_MODES
        });
        until("the probe finished", || {
            !f.host.packed_reprobing.load(Ordering::Acquire)
        });
        assert!(!AV1_PROBED.load(Ordering::Acquire));
        assert_eq!(offered(&f), butterpollo_core::packed444::ALL_MODES);
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
        save(&f, "experimental_packed_444", "true", probe);
        until("the probe started", || PROBING.load(Ordering::Acquire));
        save(&f, "experimental_packed_444", "false", probe);
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
        config
            .values
            .insert("experimental_packed_444".into(), "true".into());
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
        let all = butterpollo_core::packed444::ALL_MODES;
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
}
#[test]
fn packed_modes_are_logged_by_name() {
    assert_eq!(super::packed_mode_names(0), "none");
    assert_eq!(super::packed_mode_names(1 | 4), "HEVC 8-bit, AV1 8-bit");
    assert_eq!(
        super::packed_mode_names(butterpollo_core::packed444::ALL_MODES),
        "HEVC 8-bit, HEVC 10-bit, AV1 8-bit, AV1 10-bit"
    );
}
#[test]
fn settings_of_removed_encoders_and_limiters_migrate_with_one_warning() {
    use super::test_support::Fixture;
    let directory = Fixture::profile();
    let path = directory.join("sunshine.conf");
    std::fs::write(
        &path,
        "encoder = quicksync\nframe_limiter_provider = nvidia-control-panel\nqsv_preset = slow\nnvenc_preset = 4\nsunshine_name = Host\n",
    )
    .unwrap();
    let first = Fixture::load(directory.clone());
    {
        let config = first.host.config.read().unwrap();
        assert_eq!(config.get("encoder", ""), "auto");
        assert_eq!(config.get("frame_limiter_provider", ""), "auto");
        assert!(!config.values.contains_key("qsv_preset"));
        // NVENC is an encoder again: its settings stay.
        assert_eq!(config.get("nvenc_preset", ""), "4");
        assert_eq!(config.get("sunshine_name", ""), "Host");
    }
    let warnings = first.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "settings_retired");
    assert!(
        warnings[0]
            .message
            .contains("encoder quicksync is now auto")
    );
    assert!(
        warnings[0]
            .message
            .contains("frame_limiter_provider nvidia-control-panel is now auto")
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("encoder = auto"), "{saved}");
    assert!(!saved.contains("qsv"), "{saved}");
    // The next start reads the migrated file; the notice stays until dismissed.
    let second = Fixture::load(directory.clone());
    let warnings = second.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "settings_retired");
    crate::host_warnings::dismiss(&directory, &second.host.warnings, "settings_retired").unwrap();
    assert!(
        Fixture::load(directory.clone())
            .host
            .warnings
            .snapshot()
            .is_empty()
    );
    assert_eq!(
        second.host.config.read().unwrap().get("encoder", ""),
        "auto"
    );
}

#[test]
fn nvidia_settings_a_2_6_host_left_pending_are_reported_at_start() {
    use super::test_support::Fixture;
    let directory = Fixture::profile();
    std::fs::write(
        directory.join("frame-limiter-recovery.json"),
        r#"{"version":1,"rtss":null,"nvidia":[{"id":277041154,"before":{"effective":0,"override_value":null},"applied":60}]}"#,
    )
    .unwrap();
    let first = Fixture::load(directory.clone());
    let warnings = first.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "nvidia_limiter_retired");
    assert!(warnings[0].message.contains("NVIDIA Control Panel"));
    // Shown at each start until dismissed.
    let second = Fixture::load(directory.clone());
    let warnings = second.host.warnings.snapshot();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "nvidia_limiter_retired");
    crate::host_warnings::dismiss(&directory, &second.host.warnings, "nvidia_limiter_retired")
        .unwrap();
    assert!(Fixture::load(directory).host.warnings.snapshot().is_empty());
}

#[test]
fn a_rejoin_in_the_same_mode_reuses_the_clients_own_display_untouched() {
    use super::take_retained;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Instant;
    let paused = Some(Instant::now());
    let mine = Arc::new((2560, 1440, 120_000, true));
    let theirs = Arc::new((2560, 1440, 120_000, true));
    let mut displays = BTreeMap::from([
        ("me".to_string(), (mine.clone(), paused)),
        ("other".to_string(), (theirs.clone(), None)),
    ]);
    let same = |lease: &Arc<(u32, u32, u32, bool)>| **lease == (2560, 1440, 120_000, true);

    // The same client in the same mode gets the very display it had, and
    // the retained entry stays as it was: nothing is created or released.
    let reused = take_retained(&mut displays, "me", same).unwrap();
    assert!(Arc::ptr_eq(&reused, &mine));
    assert_eq!(displays.len(), 2);
    assert!(Arc::ptr_eq(&displays["me"].0, &mine));
    assert_eq!(displays["me"].1, paused);
    drop(reused);
    // Rejoining again still finds it.
    assert!(Arc::ptr_eq(
        &take_retained(&mut displays, "me", same).unwrap(),
        &mine
    ));

    // A client with nothing retained never takes another client's display.
    assert_eq!(take_retained(&mut displays, "new", same), Err(None));
    assert!(Arc::ptr_eq(&displays["other"].0, &theirs));
    assert!(Arc::ptr_eq(&displays["me"].0, &mine));
}
#[test]
fn a_rejoin_in_another_mode_releases_only_the_clients_own_display() {
    use super::take_retained;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    let mine = Arc::new((2560, 1440, 120_000, true));
    let theirs = Arc::new((2560, 1440, 120_000, true));
    let mut displays = BTreeMap::from([
        ("me".to_string(), (mine.clone(), None)),
        ("other".to_string(), (theirs.clone(), None)),
    ]);
    let sdr = |lease: &Arc<(u32, u32, u32, bool)>| **lease == (2560, 1440, 120_000, false);
    let released = take_retained(&mut displays, "me", sdr)
        .unwrap_err()
        .unwrap();
    assert!(Arc::ptr_eq(&released, &mine));
    assert!(!displays.contains_key("me"));
    assert!(Arc::ptr_eq(&displays["other"].0, &theirs));
    assert_eq!(take_retained(&mut displays, "me", sdr), Err(None));
}
#[test]
fn a_sign_in_set_while_the_host_runs_is_taken_up_and_not_written_back() {
    use super::test_support::Fixture;
    use butterpollo_core::state::{Credentials, load_json, write_json};
    let f = Fixture::new();
    let h = &f.host;
    // As in Apollo and Sunshine profiles, the sign-in shares the
    // paired-state file.
    assert_eq!(h.credentials_path, h.paired_path);
    let old = Credentials::new("apollo".into(), "old-password").unwrap();
    h.save_credentials(&old).unwrap();
    *h.credentials.write().unwrap() = Some(old);
    h.new_web_session(
        "apollo".into(),
        false,
        String::new(),
        "127.0.0.1".into(),
        None,
    )
    .unwrap();
    let tokens = |h: &Host| h.aliases.lock().unwrap()["root"]["api_tokens"].clone();
    {
        let mut aliases = h.aliases.lock().unwrap();
        aliases["root"]["api_tokens"] = serde_json::json!([{"hash": "made with old-password"}]);
        write_json(&h.aliases_path, &*aliases).unwrap();
    }
    // The console's own write is no change from outside.
    h.reload_credentials();
    assert_eq!(h.web_sessions.lock().unwrap().len(), 1);
    assert_eq!(tokens(h).as_array().map(Vec::len), Some(1));
    // --creds writes the file while the host runs.
    let set = |credentials: Option<&Credentials>| {
        let mut document = load_json(&h.credentials_path, serde_json::json!({})).unwrap();
        let document = document.as_object_mut().unwrap();
        for key in ["username", "password", "salt"] {
            document.remove(key);
        }
        if let Some(c) = credentials {
            document.extend(
                serde_json::to_value(c)
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .clone(),
            );
        }
        write_json(&h.credentials_path, &document).unwrap();
    };
    let new = Credentials::new("Ramazan K".into(), "new-password").unwrap();
    set(Some(&new));
    h.paired.write().unwrap().save(&h.paired_path).unwrap();
    h.reload_credentials();
    let current = h.credentials.read().unwrap().clone().unwrap();
    assert!(current.verifies("ramazan k", "new-password"));
    assert!(!current.verifies("apollo", "old-password"));
    assert!(h.web_sessions.lock().unwrap().is_empty());
    // So are the API tokens made while the old password was in force.
    assert_eq!(tokens(h), serde_json::json!([]));
    assert_eq!(
        load_json(&h.aliases_path, serde_json::json!({})).unwrap()["root"]["api_tokens"],
        serde_json::json!([])
    );
    // A later save of the paired state keeps the new sign-in.
    let paired = h.paired.read().unwrap();
    paired.save(&h.paired_path).unwrap();
    drop(paired);
    let saved = Credentials::load(&h.credentials_path).unwrap().unwrap();
    assert!(saved.verifies("Ramazan K", "new-password"));
    assert!(h.paired.read().unwrap().document["root"]["named_devices"].is_array());
    // A sign-in removed from the file offers the first-run setup.
    set(None);
    h.reload_credentials();
    assert!(h.credentials.read().unwrap().is_none());
    assert!(h.paired.read().unwrap().document.get("username").is_none());
}
#[test]
fn host_load_reuses_the_shared_virtual_display_guid_without_replacing_an_existing_one() {
    use super::test_support::Fixture;
    let f = Fixture::new();
    let imported = "f773d31b-43da-470c-80d5-02e777a6d993";
    let existing = "15ff84bd-9db8-45ef-bfde-e60337744358";
    butterpollo_core::state::write_json(
        &f.host.aliases_path,
        &serde_json::json!({"root":{"shared_virtual_display_guid":imported}}),
    )
    .unwrap();
    let load = || super::Host::load(f.host.directory.clone(), f.host.assets.clone(), None).unwrap();
    let host = load();
    assert_eq!(
        host.paired.read().unwrap().document["root"]["shared_virtual_display_guid"],
        imported
    );
    let mut state = host.paired.write().unwrap();
    state.document["root"]["shared_virtual_display_guid"] = existing.into();
    state.save(&host.paired_path).unwrap();
    drop(state);
    assert_eq!(
        load().paired.read().unwrap().document["root"]["shared_virtual_display_guid"],
        existing
    );
}
use super::*;
#[test]
#[ignore = "requires SYSTEM in a console session with nobody signed in"]
fn standard_codec_retries_do_not_wait_for_windows_sign_in() {
    assert!(butterpollo_windows::process::is_system());
    assert!(!butterpollo_windows::process::user_signed_in());
    let f = test_support::Fixture::new();
    let now = Instant::now() - CODEC_RETRY_MIN;
    let mut retry = CodecRetry::new(now);
    retry.completed(0, Some("GPU not ready at startup"), &f.host.warnings, now);
    let host = f.host.clone();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        send.send(host.wait_for_codec_retry(&retry)).unwrap();
    });
    let result = receive.recv_timeout(Duration::from_secs(2));
    f.host
        .stop
        .store(true, std::sync::atomic::Ordering::Release);
    worker.join().unwrap();
    assert!(result.unwrap());
}
#[test]
fn codecs_keep_retrying_with_capped_backoff_until_the_probe_recovers() {
    let mut now = Instant::now();
    let mut retry = CodecRetry::new(now);
    let warnings = Warnings::default();
    let mut attempts = 0;
    let mut probe = || {
        attempts += 1;
        if attempts <= 100 {
            (0, Some("AMF error 1"))
        } else {
            (0x101, None)
        }
    };
    for delay in [5, 10, 20, 40]
        .into_iter()
        .chain(std::iter::repeat_n(60, 96))
    {
        let (flags, error) = probe();
        assert!(retry.completed(flags, error, &warnings, now));
        assert_eq!(
            warnings.snapshot()[0].message,
            "No video encoder available: AMF error 1. Retrying."
        );
        now += Duration::from_secs(delay);
        assert!(!retry.due(now - Duration::from_millis(1), false));
        assert!(retry.due(now, false));
    }
    let (flags, error) = probe();
    assert!(!retry.completed(flags, error, &warnings, now));
    assert!(warnings.snapshot().is_empty());
    assert!(!retry.due(now + Duration::from_secs(600), true));
    assert_eq!(attempts, 101);
}
#[test]
fn client_requests_bypass_backoff_but_never_the_minimum_interval() {
    let mut now = Instant::now();
    let mut retry = CodecRetry::new(now);
    let warnings = Warnings::default();
    let mut calls = 0;
    let mut probe = || {
        calls += 1;
        0
    };
    for _ in 0..10 {
        assert!(retry.completed(probe(), Some("driver unavailable"), &warnings, now));
        for milliseconds in [0, 1, 250, 4999] {
            assert!(!retry.due(now + Duration::from_millis(milliseconds), true));
        }
        now += CODEC_RETRY_MIN;
        assert!(retry.due(now, true));
    }
    assert_eq!(calls, 10);
    assert!(!retry.due(now, false));
    assert!(retry.due(now + Duration::from_secs(55), false));
}
#[test]
fn codec_failure_reminders_are_limited_and_recovery_resets_the_delay() {
    let now = Instant::now();
    let mut retry = CodecRetry::new(now);
    assert!(retry.warn(now));
    for seconds in 1..60 {
        assert!(!retry.warn(now + Duration::from_secs(seconds)));
    }
    assert!(retry.warn(now + Duration::from_secs(60)));
    let warnings = Warnings::default();
    for _ in 0..4 {
        assert!(retry.completed(0, None, &warnings, now));
    }
    assert!(!retry.due(now + Duration::from_secs(39), false));
    assert!(!retry.completed(1, None, &warnings, now));
    assert!(retry.completed(0, Some("COM initialization failed"), &warnings, now));
    assert_eq!(
        warnings.snapshot()[0].message,
        "No video encoder available: COM initialization failed. Retrying."
    );
    assert!(!retry.due(now + Duration::from_secs(4), false));
    assert!(retry.due(now + Duration::from_secs(5), false));
}
#[test]
fn codec_modifiers_do_not_count_as_video_and_optional_codecs_keep_standard_retries() {
    let now = Instant::now();
    let mut retry = CodecRetry::new(now);
    let warnings = Warnings::default();
    assert!(retry.completed(0x40000000, None, &warnings, now));
    assert_eq!(
        warnings.snapshot()[0].message,
        "No video encoder available: the encoder produced no packets. Retrying."
    );
    assert!(retry.completed(0x800000, None, &warnings, now));
    assert!(warnings.snapshot().is_empty());
    assert!(retry.completed(0, Some("AMF error 1"), &warnings, now));
    assert_eq!(warnings.snapshot().len(), 1);
    assert!(!retry.completed(0x10000, None, &warnings, now));
    assert!(warnings.snapshot().is_empty());
}
#[test]
fn an_invalid_library_starts_empty_and_is_kept() {
    let dir = std::env::temp_dir().join(format!("butterpollo-apps-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("apps.json");
    let default = json!({"apps":[{"name":"Desktop"}]});
    assert_eq!(load_library(&path, default.clone()).unwrap(), default);
    std::fs::write(&path, b"{\"apps\":[{\"name\":\"Game\",}]}").unwrap();
    assert_eq!(
        load_library(&path, default.clone()).unwrap()["apps"],
        json!([])
    );
    assert_eq!(
        std::fs::read(dir.join("apps.json.invalid")).unwrap(),
        b"{\"apps\":[{\"name\":\"Game\",}]}"
    );
    std::fs::write(&path, b"{\"apps\":[{\"name\":\"Game\"}]}").unwrap();
    assert_eq!(
        load_library(&path, default.clone()).unwrap()["apps"][0]["name"],
        "Game"
    );
    // Notepad's byte order mark, and a blank file Vibepollo reads as missing.
    std::fs::write(&path, b"\xef\xbb\xbf{\"apps\":[{\"name\":\"Noted\"}]}").unwrap();
    assert_eq!(
        load_library(&path, default.clone()).unwrap()["apps"][0]["name"],
        "Noted"
    );
    std::fs::write(&path, b"\r\n").unwrap();
    assert_eq!(load_library(&path, default.clone()).unwrap(), default);
    std::fs::remove_dir_all(&dir).ok();
}
