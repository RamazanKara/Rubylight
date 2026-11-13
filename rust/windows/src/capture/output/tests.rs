use super::*;
use std::cell::Cell;

struct Fake {
    current: bool,
    fresh_current: bool,
    /// Refreshes whose factory is already stale when they return.
    stale_refreshes: usize,
    candidates: Vec<(Identity, Option<Description>)>,
    enumerations: usize,
    reads: Cell<usize>,
    refresh: u32,
    refresh_reads: Cell<usize>,
}

impl Source for Fake {
    type Output = Option<Description>;
    fn current(&self) -> bool {
        self.current
    }
    fn refresh(&mut self, identity: &Identity) -> Result<Self::Output> {
        self.enumerations += 1;
        self.current = self.fresh_current && self.stale_refreshes == 0;
        self.stale_refreshes = self.stale_refreshes.saturating_sub(1);
        selected(identity, self.candidates.clone())
    }
    fn describe(&self, output: &Self::Output) -> Option<Description> {
        self.reads.set(self.reads.get() + 1);
        *output
    }
    fn refresh_hz(&self, _: &Identity) -> u32 {
        self.refresh_reads.set(self.refresh_reads.get() + 1);
        self.refresh
    }
}

fn identity() -> Identity {
    Identity {
        adapter: (7, 2),
        name: "selected".into(),
        monitor: 19,
    }
}

fn description(hdr: bool, peak: f32) -> Description {
    Description {
        color_space: if hdr {
            DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020
        } else {
            DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
        },
        metadata: butterpollo_core::hdr::Metadata::display(peak, 0.001, 400.),
    }
}

fn cache(now: Instant, initial: Option<Description>) -> Cache<Fake> {
    Cache::new(
        Fake {
            current: true,
            fresh_current: true,
            stale_refreshes: 0,
            candidates: vec![(identity(), initial)],
            enumerations: 0,
            reads: Cell::new(0),
            refresh: 120,
            refresh_reads: Cell::new(0),
        },
        identity(),
        initial,
        now,
    )
}

#[test]
fn initial_binding_refuses_reordered_or_detached_outputs() {
    assert_eq!(
        bind_identity("selected", identity(), true).unwrap(),
        identity()
    );
    assert!(bind_identity("another display", identity(), true).is_err());
    assert!(bind_identity("selected", identity(), false).is_err());
    assert!(
        bind_identity(
            "selected",
            Identity {
                monitor: 0,
                ..identity()
            },
            true
        )
        .is_err()
    );
}

#[test]
fn current_output_is_shared_and_throttled_without_reenumeration() {
    let now = Instant::now();
    let expected = description(true, 1000.);
    let shared = std::sync::Arc::new(std::sync::Mutex::new(cache(now, Some(expected))));
    let encoder = shared.clone();
    assert_eq!(shared.lock().unwrap().poll(now).unwrap(), Some(expected));
    assert_eq!(encoder.lock().unwrap().metadata(now), expected.metadata);
    assert_eq!(
        shared
            .lock()
            .unwrap()
            .poll(now + Duration::from_millis(999))
            .unwrap(),
        Some(expected)
    );
    assert_eq!(shared.lock().unwrap().source.reads.get(), 1);
    shared.lock().unwrap().poll(now + CHECK_INTERVAL).unwrap();
    assert_eq!(shared.lock().unwrap().source.reads.get(), 2);
    assert_eq!(shared.lock().unwrap().source.enumerations, 0);
}

#[test]
fn stale_factory_refreshes_both_hdr_transitions_on_the_selected_output() {
    for (before, after) in [(false, true), (true, false)] {
        let now = Instant::now();
        let old = description(before, 1000.);
        let new = description(after, 600.);
        let mut cache = cache(now, Some(old));
        cache.poll(now).unwrap();
        // Reordered outputs include another GPU with the same desktop name
        // and another monitor reusing that name on the capture adapter.
        cache.source.candidates = vec![
            (
                Identity {
                    adapter: (8, 2),
                    ..identity()
                },
                Some(old),
            ),
            (
                Identity {
                    monitor: 20,
                    ..identity()
                },
                Some(old),
            ),
            (identity(), Some(new)),
        ];
        cache.source.current = false;
        let current = cache.poll(now + CHECK_INTERVAL).unwrap();
        assert_eq!(current, Some(new));
        assert!(mode_changed(Some(old.color_space), current));
        assert_eq!(cache.source.enumerations, 1);
    }
}

#[test]
fn luminance_refresh_updates_metadata_without_a_capture_mode_change() {
    let now = Instant::now();
    let old = description(true, 1000.);
    let new = description(true, 650.);
    let mut cache = cache(now, Some(old));
    cache.poll(now).unwrap();
    cache.source.current = false;
    cache.source.candidates = vec![(identity(), Some(new))];
    assert_eq!(cache.metadata(now + CHECK_INTERVAL), new.metadata);
    assert!(!mode_changed(
        Some(old.color_space),
        cache.poll(now + CHECK_INTERVAL).unwrap()
    ));
    assert_eq!(cache.source.enumerations, 1);
}

#[test]
fn removed_output_never_uses_another_monitor_and_refresh_retries() {
    let now = Instant::now();
    let old = description(true, 1000.);
    let mut cache = cache(now, Some(old));
    cache.poll(now).unwrap();
    cache.source.current = false;
    cache.source.candidates = vec![(
        Identity {
            monitor: 20,
            ..identity()
        },
        Some(description(false, 300.)),
    )];
    assert!(cache.poll(now + CHECK_INTERVAL).is_err());
    assert_eq!(cache.metadata(now + CHECK_INTERVAL), old.metadata);
    assert_eq!(cache.source.enumerations, 1);
    // Failed lookup installed a current factory; absence still forces retry.
    assert!(cache.source.current);
    let recovered = description(true, 750.);
    cache.source.candidates = vec![(identity(), Some(recovered))];
    assert_eq!(
        cache.poll(now + CHECK_INTERVAL * 2).unwrap(),
        Some(recovered)
    );
    assert_eq!(cache.source.enumerations, 2);
}

#[test]
fn ambiguous_output_identity_is_refused() {
    assert!(selected(&identity(), [(identity(), 1), (identity(), 2)]).is_err());
}

#[test]
fn already_stale_replacement_is_not_published_and_is_retried() {
    let now = Instant::now();
    let old = description(true, 1000.);
    let new = description(false, 300.);
    let mut cache = cache(now, Some(old));
    cache.poll(now).unwrap();
    cache.source.current = false;
    cache.source.fresh_current = false;
    cache.source.candidates = vec![(identity(), Some(new))];
    assert_eq!(cache.poll(now + CHECK_INTERVAL).unwrap(), Some(old));
    assert_eq!(cache.metadata(now + CHECK_INTERVAL), old.metadata);
    cache.source.fresh_current = true;
    // Retried soon, without waiting another check interval.
    assert_eq!(
        cache.poll(now + CHECK_INTERVAL + REFRESH_RETRY).unwrap(),
        Some(new)
    );
}

#[test]
fn topology_changes_during_a_refresh_do_not_fail_capture() {
    // Another display arriving, or its mode and HDR being set, changes
    // the factory while the selected output is enumerated. Capture opens
    // and its periodic colour check propagate errors, so this must not be
    // one: v2.2.1 tolerated it, and failing restarted or refused capture.
    let now = Instant::now();
    let old = description(true, 1000.);
    let new = description(false, 300.);
    let mut cache = cache(now, Some(old));
    cache.source.current = false;
    cache.source.stale_refreshes = 1;
    cache.source.candidates = vec![(identity(), Some(new))];
    // At open: one more change during enumeration is absorbed in this poll.
    assert_eq!(cache.poll(now).unwrap(), Some(new));
    assert_eq!(cache.source.enumerations, 2);
    // Mid-stream: changes outlasting the poll keep the published state.
    cache.source.current = false;
    cache.source.stale_refreshes = REFRESH_ATTEMPTS;
    cache.source.candidates = vec![(identity(), Some(old))];
    let current = cache.poll(now + CHECK_INTERVAL).unwrap();
    assert_eq!(current, Some(new));
    assert!(!mode_changed(Some(new.color_space), current));
    assert_eq!(cache.metadata, new.metadata);
    assert_eq!(cache.source.enumerations, 2 + REFRESH_ATTEMPTS);
    assert_eq!(
        cache.poll(now + CHECK_INTERVAL + REFRESH_RETRY).unwrap(),
        Some(old)
    );
    assert_eq!(cache.source.enumerations, 3 + REFRESH_ATTEMPTS);
    // A missing output remains an error.
    cache.source.current = false;
    cache.source.candidates.clear();
    assert!(cache.poll(now + CHECK_INTERVAL * 3).is_err());
}

#[test]
fn unavailable_description_preserves_unknown_mode_and_last_known_metadata() {
    let now = Instant::now();
    let mut cache = cache(now, None);
    assert_eq!(cache.poll(now).unwrap(), None);
    assert!(!mode_changed(None, None));
    assert_eq!(cache.metadata(now), Default::default());
    let known = description(true, 850.);
    cache.source.current = false;
    cache.source.candidates = vec![(identity(), Some(known))];
    let current = cache.poll(now + CHECK_INTERVAL).unwrap();
    assert!(mode_changed(None, current));
    assert_eq!(cache.metadata(now + CHECK_INTERVAL), known.metadata);
    cache.source.current = false;
    cache.source.candidates = vec![(identity(), None)];
    assert_eq!(cache.poll(now + CHECK_INTERVAL * 2).unwrap(), None);
    assert_eq!(cache.metadata(now + CHECK_INTERVAL * 2), known.metadata);
}

fn watched(now: Instant, initial: Option<Description>) -> Arc<Watched<Fake>> {
    Arc::new(Watched::new(cache(now, initial)))
}

fn reads(watched: &Watched<Fake>) -> usize {
    watched.cache.lock().unwrap().source.reads.get()
}

#[test]
fn hot_reads_take_the_watchers_last_check_without_querying() {
    let now = Instant::now();
    let old = description(true, 1000.);
    let new = description(false, 300.);
    let watched = watched(now, Some(old));
    let mut spawned = 0;
    assert_eq!(
        watched
            .read_with(now, |_| {
                spawned += 1;
                Ok(())
            })
            .0,
        Ok(Some(old))
    );
    assert_eq!(spawned, 1);
    assert_eq!(reads(&watched), 1);
    {
        let mut cache = watched.cache.lock().unwrap();
        cache.source.current = false;
        cache.source.candidates = vec![(identity(), Some(new))];
    }
    // Past the check interval, a read still only takes the published check.
    let later = now + CHECK_INTERVAL * 3 / 2;
    assert_eq!(
        watched.read_with(later, |_| unreachable!()).0,
        Ok(Some(old))
    );
    assert_eq!(reads(&watched), 1);
    // The watcher's check publishes the change, as a poll did.
    assert!(watched.tick(later));
    let (current, metadata, _) = watched.read_with(later, |_| unreachable!());
    assert_eq!(current, Ok(Some(new)));
    assert!(mode_changed(Some(old.color_space), current.unwrap()));
    assert_eq!(metadata, new.metadata);
    assert_eq!(watched.cache.lock().unwrap().source.enumerations, 1);
}

#[test]
fn a_failed_check_is_published_with_the_last_known_metadata() {
    let now = Instant::now();
    let old = description(true, 1000.);
    let watched = watched(now, Some(old));
    let _ = watched.read_with(now, |_| Ok(()));
    {
        let mut cache = watched.cache.lock().unwrap();
        cache.source.current = false;
        cache.source.candidates.clear();
    }
    assert!(watched.tick(now + CHECK_INTERVAL));
    let (current, metadata, _) = watched.read_with(now + CHECK_INTERVAL, |_| unreachable!());
    assert!(current.is_err());
    assert_eq!(metadata, old.metadata);
}

#[test]
fn an_unread_watcher_stops_and_the_next_read_checks_at_once() {
    let now = Instant::now();
    let old = description(true, 1000.);
    let new = description(true, 600.);
    let watched = watched(now, Some(old));
    let _ = watched.read_with(now, |_| Ok(()));
    assert!(watched.tick(now + CHECK_INTERVAL));
    assert!(!watched.tick(now + IDLE));
    {
        let mut cache = watched.cache.lock().unwrap();
        cache.source.current = false;
        cache.source.candidates = vec![(identity(), Some(new))];
    }
    let mut spawned = false;
    let (current, _, _) = watched.read_with(now + IDLE * 2, |_| {
        spawned = true;
        Ok(())
    });
    assert_eq!(current, Ok(Some(new)));
    assert!(spawned);
}

#[test]
fn without_a_watcher_every_read_checks_as_before() {
    let now = Instant::now();
    let watched = watched(now, Some(description(true, 1000.)));
    let mut spawns = 0;
    for step in 0..3 {
        let _ = watched.read_with(now + CHECK_INTERVAL * step, |_| {
            spawns += 1;
            Err(std::io::Error::other("no threads"))
        });
    }
    // Told once; the cache's own interval throttles the checks.
    assert_eq!(spawns, 1);
    assert_eq!(reads(&watched), 3);
    assert!(watched.published.lock().unwrap().snapshot.is_none());
}

#[test]
fn the_watcher_thread_ends_with_the_last_reader() {
    let now = Instant::now();
    let watched = watched(now, Some(description(true, 1000.)));
    let mut handle = None;
    let _ = watched.read_with(now, |weak| {
        handle = Some(thread::spawn(move || watch(weak)));
        Ok(())
    });
    // Waiting for its next check, the watcher holds no reference.
    thread::sleep(Duration::from_millis(200));
    assert!(!handle.as_ref().unwrap().is_finished());
    assert_eq!(Arc::strong_count(&watched), 1);
    drop(watched);
    let handle = handle.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(handle.is_finished());
    handle.join().expect("the watcher ends without a panic");
}

#[test]
fn a_refresh_change_is_published_by_the_watcher_and_found_by_name() {
    let now = Instant::now();
    let watched = watched(now, Some(description(false, 300.)));
    assert_eq!(watched.read_with(now, |_| Ok(())).2, 120);
    // A game's mode switch: hot reads keep the published rate until the watcher checks.
    watched.cache.lock().unwrap().source.refresh = 60;
    assert_eq!(watched.read_with(now, |_| unreachable!()).2, 120);
    assert!(watched.tick(now + CHECK_INTERVAL));
    assert_eq!(
        watched
            .read_with(now + CHECK_INTERVAL, |_| unreachable!())
            .2,
        60
    );
    // The stamper finds it by output name while a device holds it.
    let name = r"\.\REFRESH-TEST";
    register(name, &watched);
    assert!(published_refresh_hz(name).is_some());
    drop(watched);
    assert_eq!(published_refresh_hz(name), None);
}

#[test]
fn another_device_on_the_same_output_does_not_hide_the_stream_s() {
    // A GPU probe opens its own device on the stream's display after the
    // stream did, and stops mid-stream.
    let now = Instant::now();
    let name = r"\.\REFRESH-PROBE";
    let stream = watched(now, None);
    let probe = watched(now, None);
    probe.cache.lock().unwrap().source.refresh = 60;
    register(name, &stream);
    register(name, &probe);
    assert!(published_refresh_hz(name).is_some());
    drop(probe);
    assert_eq!(published_refresh_hz(name), Some(120));
    drop(stream);
    assert_eq!(published_refresh_hz(name), None);
}

#[test]
fn the_refresh_is_read_at_the_check_interval_even_without_a_watcher() {
    let now = Instant::now();
    let watched = watched(now, None);
    for ms in [0, 10, 500, 999] {
        let _ = watched.read_with(now + Duration::from_millis(ms), |_| {
            Err(std::io::Error::other("no threads"))
        });
    }
    assert_eq!(watched.cache.lock().unwrap().source.refresh_reads.get(), 1);
    let _ = watched.read_with(now + CHECK_INTERVAL, |_| unreachable!());
    assert_eq!(watched.cache.lock().unwrap().source.refresh_reads.get(), 2);
}

#[test]
fn the_display_is_in_hdr_by_its_color_space_not_its_frame_format() {
    assert_eq!(in_hdr(Some(description(true, 850.))), Some(true));
    assert_eq!(in_hdr(Some(description(false, 80.))), Some(false));
    // An output that cannot say is not assumed to be in HDR.
    assert_eq!(in_hdr(None), None);
}
