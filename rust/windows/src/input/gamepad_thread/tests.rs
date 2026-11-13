use super::*;

#[test]
fn drained_batches_reuse_both_buffers_and_discard_interrupted_input() {
    let mut queue = Queue::default();
    let mut batch = VecDeque::new();
    // The producer may fill the other buffer while a batch is in the driver.
    for round in 0..8 {
        let expected = [state(0, 1, 1, round), state(0, 1, 0, round)];
        queue.events.extend(expected.clone());
        queue.release = round == 3;
        queue.full = true;
        let release = queue.take_into(&mut batch);
        assert_eq!(release, round == 3);
        assert!(!queue.full);
        assert!(!queue.release);
        assert_eq!(batch.iter().cloned().collect::<Vec<_>>(), expected);
        if round > 0 {
            assert!(
                queue.events.capacity() >= 2,
                "producer lost its spare buffer"
            );
        }
        // An interrupt drops the rest of the batch, as a release request does.
        let capacity = batch.capacity();
        let mut drained = batch.drain(..);
        assert_eq!(drained.next(), Some(expected[0].clone()));
        drop(drained);
        assert!(batch.is_empty());
        assert_eq!(batch.capacity(), capacity);
    }
}

fn state(id: u16, active: u16, buttons: u32, stick: i16) -> Event {
    Event::Controller {
        id,
        active,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [stick, 0, 0, 0],
    }
}
fn motion(id: u8, kind: u8, x: f32) -> Event {
    Event::Motion {
        id,
        kind,
        xyz: [x, 0., 0.],
    }
}

/// Holds every driver call until the test opens it.
#[derive(Default)]
pub(crate) struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
    blocked: AtomicBool,
    /// Calls that reached the driver.
    entered: std::sync::atomic::AtomicUsize,
}
impl Gate {
    fn pass(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.lock().unwrap();
        while !*open {
            self.blocked.store(true, Ordering::SeqCst);
            open = self.opened.wait(open).unwrap();
        }
        self.blocked.store(false, Ordering::SeqCst);
    }
    pub(crate) fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
    fn close(&self) {
        *self.open.lock().unwrap() = false;
    }
    pub(crate) fn wait_until(&self, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "the gamepad thread stopped");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    pub(crate) fn wait_entered(&self, calls: usize) {
        self.wait_until(|| self.entered.load(Ordering::SeqCst) >= calls);
    }
}

/// A driver whose calls wait at a gate, recording what reached it.
struct Fake {
    gate: Arc<Gate>,
    log: Arc<Mutex<Vec<String>>>,
    plugged: bool,
    feedback: Vec<(u16, u16, Vec<u8>)>,
}
impl Pads for Fake {
    fn apply(&mut self, event: &Event) -> Result<()> {
        self.gate.pass();
        self.plugged = true;
        self.log.lock().unwrap().push(format!("{event:?}"));
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        self.gate.pass();
        std::mem::take(&mut self.feedback)
    }
    fn audio(&mut self, id: u16, kind: u16, payload: &[u8]) {
        self.log
            .lock()
            .unwrap()
            .push(format!("audio {id} {kind:#06x} {payload:?}"));
    }
    fn motion_supported(&self, _: u16) -> bool {
        true
    }
    fn plugged(&self) -> bool {
        self.plugged
    }
    fn release_all(&mut self) -> Result<()> {
        self.gate.pass();
        self.log.lock().unwrap().push("released".into());
        Ok(())
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.log.lock().unwrap().push("unplugged".into());
    }
}
/// Pads whose driver calls wait at the gate, which starts closed.
pub(crate) fn fake() -> (GamepadThread, Arc<Gate>, Arc<Mutex<Vec<String>>>) {
    let gate = Arc::new(Gate::default());
    let log = Arc::new(Mutex::new(Vec::new()));
    let pads = GamepadThread::spawn_reported(
        {
            let (gate, log) = (gate.clone(), log.clone());
            move || {
                Ok(Fake {
                    gate: gate.clone(),
                    log: log.clone(),
                    plugged: false,
                    feedback: vec![(0, 1, vec![1, 2, 3, 4, 0, 0, 0, 0])],
                })
            }
        },
        Default::default(),
    )
    .unwrap();
    (pads, gate, log)
}
fn logged(events: &[Event]) -> Vec<String> {
    events.iter().map(|event| format!("{event:?}")).collect()
}

fn audio_count(log: &Mutex<Vec<String>>) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|entry| entry.starts_with("audio "))
        .count()
}

/// A virtual Steam Controller: the reports the test queues are what Steam
/// wrote to it since the last poll.
struct SteamFake {
    reports: Arc<Mutex<Vec<SteamHaptic>>>,
    plugged: bool,
}
impl Pads for SteamFake {
    fn apply(&mut self, _: &Event) -> Result<()> {
        self.plugged = true;
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        vec![]
    }
    fn steam_haptics(&mut self) -> Vec<SteamHaptic> {
        std::mem::take(&mut self.reports.lock().unwrap())
    }
    fn motion_supported(&self, _: u16) -> bool {
        false
    }
    fn plugged(&self) -> bool {
        self.plugged
    }
    fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
}
fn steam_fake() -> (GamepadThread, Arc<Mutex<Vec<SteamHaptic>>>) {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let pads = GamepadThread::spawn_reported(
        {
            let reports = reports.clone();
            move || {
                Ok(SteamFake {
                    reports: reports.clone(),
                    plugged: false,
                })
            }
        },
        Default::default(),
    )
    .unwrap();
    (pads, reports)
}
/// Steam's 0x82 tone on both pads at full gain, held 10 s unless stopped.
fn steam_tone(reports: &Mutex<Vec<SteamHaptic>>) {
    let tone = SteamHaptic::new(0, &[0x82, 2, 3, 0]).unwrap();
    reports.lock().unwrap().push(tone);
}
const ON: [u8; 4] = [0xff; 4];
const OFF: [u8; 4] = [0; 4];
/// Controller 0's rumble reports, collected until `last` comes or a
/// second passes.
fn rumbles_until(pads: &GamepadThread, last: Option<[u8; 4]>) -> Vec<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut rumbles = Vec::new();
    while Instant::now() < deadline {
        for report in pads.reports() {
            if let PadReport::Feedback {
                id: 0,
                kind: RUMBLE_DATA,
                data,
            } = report
            {
                rumbles.push(data);
            }
        }
        if last.is_some_and(|last| rumbles.last().is_some_and(|data| *data == last)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    rumbles
}
/// No rumble for 100 ms, a dozen feedback polls.
fn quiet(pads: &GamepadThread) -> bool {
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        if pads.reports().any(|report| {
            matches!(
                report,
                PadReport::Feedback {
                    kind: RUMBLE_DATA,
                    ..
                }
            )
        }) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

#[test]
fn steam_rumble_stops_when_its_controller_leaves() {
    let (pads, reports) = steam_fake();
    pads.send(state(0, 1, 0, 0));
    steam_tone(&reports);
    assert_eq!(rumbles_until(&pads, Some(ON)), [ON]);
    pads.send(state(0, 0, 0, 0));
    assert_eq!(rumbles_until(&pads, Some(OFF)), [OFF]);
    // Its held tone does not come back.
    assert!(quiet(&pads));
}

#[test]
fn steam_rumble_follows_the_clients_haptics_setting() {
    let (pads, reports) = steam_fake();
    let mut injector = super::super::Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    injector.gamepads = Arc::new(pads);
    injector.gamepads.send(state(0, 1, 0, 0));
    steam_tone(&reports);
    assert_eq!(rumbles_until(&injector.gamepads, Some(ON)), [ON]);
    injector.apply(&Event::Haptics(false)).unwrap();
    assert_eq!(rumbles_until(&injector.gamepads, Some(OFF)), [OFF]);
    // Reports while haptics are off are dropped, not held for later.
    steam_tone(&reports);
    assert!(quiet(&injector.gamepads));
    injector.apply(&Event::Haptics(true)).unwrap();
    assert!(quiet(&injector.gamepads));
    steam_tone(&reports);
    assert_eq!(rumbles_until(&injector.gamepads, Some(ON)), [ON]);
}

#[test]
fn steam_rumble_does_not_outlive_a_parked_session() {
    let (pads, reports) = steam_fake();
    pads.send(state(0, 1, 0, 0));
    steam_tone(&reports);
    assert_eq!(rumbles_until(&pads, Some(ON)), [ON]);
    // The session ends with the motors running. Its reconnect starts
    // from stopped motors, so the same tone reaches the new client.
    pads.park();
    // The thread takes the release before it polls again.
    Gate::default().wait_until(|| !pads.shared.queue.lock().unwrap().release);
    pads.activate();
    pads.send(state(0, 1, 0, 0));
    steam_tone(&reports);
    let rumbles = rumbles_until(&pads, Some(ON));
    assert_eq!(rumbles.last(), Some(&ON.to_vec()), "{rumbles:?}");
}

#[test]
fn fast_polling_follows_feedback_only_when_switched_on() {
    let now = Instant::now();
    let feedback = Some(now + FAST_SPAN);
    let off = Polling {
        every: POLL,
        fast: false,
    };
    let on = Polling { fast: true, ..off };
    assert_eq!(off.next(now, feedback), now + POLL);
    assert_eq!(on.next(now, None), now + POLL);
    assert_eq!(on.next(now, feedback), now + FAST_POLL);
    // Back to the normal poll once the span has passed.
    assert_eq!(on.next(now + FAST_SPAN, feedback), now + FAST_SPAN + POLL);
}

#[test]
fn gamepad_timings_cover_one_window_then_start_again() {
    let mut stats = Stats::default();
    let start = Instant::now();
    for micros in 1..=100 {
        stats.batch(start, micros, start + Duration::from_micros(micros as u64));
    }
    stats.poll(3, start);
    stats.poll(1, start);
    assert!(stats.take_due(start + Duration::from_secs(4)).is_none());
    let window = stats
        .take_due(start + STATS_EVERY + Duration::from_millis(1))
        .unwrap();
    assert_eq!(window.waits.len(), 100);
    assert_eq!(window.waits[49], Duration::from_micros(50));
    assert_eq!(window.depth_max, 100);
    assert_eq!(
        (window.polls, window.drained, window.drained_max),
        (2, 4, 3)
    );
    // The next window starts with the next record, not the last one.
    assert!(stats.since.is_none() && stats.waits.is_empty());
    assert!(stats.take_due(start + STATS_EVERY * 3).is_none());
}

#[test]
fn a_batchs_wait_counts_from_its_oldest_event() {
    let (pads, gate, _log) = fake();
    // The first state waits at the gate; the second queues behind it and
    // the third folds into the second, keeping its queue time.
    pads.send(state(0, 1, 1, 0));
    gate.wait_entered(1);
    pads.send(state(0, 1, 0, 1));
    let queued = pads.shared.queue.lock().unwrap().oldest.unwrap();
    pads.send(state(0, 1, 0, 2));
    assert_eq!(pads.shared.queue.lock().unwrap().oldest, Some(queued));
    gate.open();
    gate.wait_until(|| pads.shared.queue.lock().unwrap().oldest.is_none());
    pads.release_all();
    assert!(pads.shared.queue.lock().unwrap().oldest.is_none());
}

/// A pad whose driver signals an event when its feedback request
/// completes, as ViGEmBus's notification does.
struct Signalled {
    event: Arc<OwnedHandle>,
    ready: Arc<AtomicBool>,
    plugged: bool,
}
impl Pads for Signalled {
    fn apply(&mut self, _: &Event) -> Result<()> {
        self.plugged = true;
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        // Taking the report re-arms the request.
        if self.ready.swap(false, Ordering::SeqCst) {
            vec![(0, 4, vec![0xff; 8])]
        } else {
            vec![]
        }
    }
    fn wait_handles(&self, handles: &mut Vec<HANDLE>) {
        handles.push(HANDLE(self.event.as_raw_handle()));
    }
    fn feedback_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
    fn motion_supported(&self, _: u16) -> bool {
        false
    }
    fn plugged(&self) -> bool {
        self.plugged
    }
    fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn a_drivers_feedback_event_is_forwarded_without_waiting_for_the_poll() {
    // SAFETY: CreateEventW returned a new manual-reset event, owned here.
    let event = Arc::new(unsafe {
        OwnedHandle::from_raw_handle(CreateEventW(None, true, false, None).unwrap().0)
    });
    let ready = Arc::new(AtomicBool::new(false));
    // No poll comes within the test: only the event can bring the report.
    let pads = GamepadThread::spawn_polled(
        {
            let (event, ready) = (event.clone(), ready.clone());
            move || {
                Ok(Signalled {
                    event: event.clone(),
                    ready: ready.clone(),
                    plugged: false,
                })
            }
        },
        Default::default(),
        Polling {
            every: Duration::from_secs(3600),
            fast: false,
        },
    )
    .unwrap();
    pads.send(state(0, 1, 0, 0));
    // The first tick runs with the first state; wait until it has.
    Gate::default().wait_until(|| pads.shared.queue.lock().unwrap().events.is_empty());
    std::thread::sleep(Duration::from_millis(20));
    for _ in 0..3 {
        ready.store(true, Ordering::SeqCst);
        // SAFETY: the event is alive in `event`.
        unsafe { SetEvent(HANDLE(event.as_raw_handle())).unwrap() };
        let deadline = Instant::now() + Duration::from_secs(5);
        let report = loop {
            if let Some(report) = pads.reports().next() {
                break report;
            }
            assert!(Instant::now() < deadline, "feedback waited for the poll");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(
            report,
            PadReport::Feedback {
                id: 0,
                kind: 4,
                data: vec![0xff; 8]
            }
        );
        // SAFETY: as above; the driver resets it as the next request starts.
        unsafe {
            windows::Win32::System::Threading::ResetEvent(HANDLE(event.as_raw_handle())).unwrap()
        };
    }
}

#[test]
fn controller_audio_readiness_excludes_pending_and_fallback_pads() {
    struct AudioPad {
        gate: Arc<Gate>,
        usb_audio: bool,
        active: u16,
    }
    impl Pads for AudioPad {
        fn apply(&mut self, event: &Event) -> Result<()> {
            self.gate.pass();
            match event {
                Event::Arrival { id, .. } => self.active |= 1 << *id,
                Event::Controller { active, .. } => self.active &= *active,
                _ => {}
            }
            Ok(())
        }
        fn audio_ready(&self) -> u16 {
            if self.usb_audio { self.active } else { 0 }
        }
        fn refresh(&mut self) -> Result<()> {
            Ok(())
        }
        fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
            vec![]
        }
        fn motion_supported(&self, _: u16) -> bool {
            false
        }
        fn plugged(&self) -> bool {
            self.active != 0
        }
        fn release_all(&mut self) -> Result<()> {
            Ok(())
        }
    }
    for usb_audio in [false, true] {
        let gate = Arc::new(Gate::default());
        let pads = GamepadThread::spawn_reported(
            {
                let gate = gate.clone();
                move || {
                    Ok(AudioPad {
                        gate: gate.clone(),
                        usb_audio,
                        active: 0,
                    })
                }
            },
            Default::default(),
        )
        .unwrap();
        pads.send(Event::Arrival {
            id: 0,
            kind: 2,
            capabilities: haptics::CAPABILITY,
            buttons: 0,
        });
        gate.wait_entered(1);
        assert!(!pads.controller_audio_ready(0));
        gate.open();
        assert!(pads.wait_ready(1, Duration::from_secs(1)));
        // Both USB and VHF arrivals are ready for input; only actual USB
        // audio may make the client abandon its ordinary microphone route.
        assert_eq!(pads.controller_audio_ready(0), usb_audio);
        assert!(!pads.controller_audio_ready(1));
        assert!(!pads.controller_audio_ready(16));
        pads.park();
        assert!(!pads.controller_audio_ready(0));
        pads.activate();
        assert_eq!(pads.controller_audio_ready(0), usb_audio);
        pads.send(state(0, 0, 0, 0));
        gate.wait_until(|| pads.shared.ready.load(Ordering::Acquire) == 0);
        assert!(!pads.controller_audio_ready(0));
    }
}

#[test]
fn controller_audio_status_outlives_pcm_and_stalled_arrival() {
    let (pads, gate, log) = fake();
    pads.send(Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    });
    // The USB driver is still attaching when reliable status and PCM arrive.
    gate.wait_entered(1);
    let status = [1, 1, 0, 0, 7, 0, 0, 0];
    pads.audio(0, controller_audio::CONTROL, &status);
    pads.audio(0, controller_audio::MICROPHONE, &[1; 16]);
    for input in &mut pads.shared.audio_input.lock().unwrap().pcm {
        input.at = Instant::now() - haptics::MAX_AGE * 2;
    }
    gate.open();
    gate.wait_until(|| audio_count(&log) == 1);
    assert!(log.lock().unwrap().last().unwrap().contains("0x5535"));

    // The client's periodic identical status must not reset device buffers.
    pads.audio(0, controller_audio::CONTROL, &status);
    gate.wait_entered(gate.entered.load(Ordering::SeqCst) + 2);
    assert_eq!(audio_count(&log), 1);

    // A transient control gap releases held buttons, but the same transport
    // must resume its accepted audio policy after that release completes.
    // Hold the preceding poll in the driver so it cannot consume a replay
    // before the queued release gets its own iteration.
    gate.close();
    gate.wait_until(|| gate.blocked.load(Ordering::SeqCst));
    pads.release_all();
    gate.open();
    gate.wait_until(|| log.lock().unwrap().iter().any(|entry| entry == "released"));
    gate.wait_until(|| audio_count(&log) == 2);
    let entries = log.lock().unwrap();
    let release = entries
        .iter()
        .position(|entry| entry == "released")
        .unwrap();
    assert!(
        entries[release + 1..]
            .iter()
            .any(|entry| entry.starts_with("audio "))
    );
    drop(entries);

    // Disabling audio is durable too, including before another arrival.
    let disabled = [1, 1, 0, 0, 0, 0, 0, 0];
    pads.audio(0, controller_audio::CONTROL, &disabled);
    gate.wait_until(|| audio_count(&log) == 3);
    assert_eq!(
        pads.shared.audio_input.lock().unwrap().status[0],
        Some(disabled)
    );
}

/// #56: a thread stuck in the driver must not hold up whoever stops it
/// (the next Nucleus Co-op launch) beyond the bound.
#[test]
fn stopping_within_a_bound_unplugs_or_gives_up_on_a_stuck_driver() {
    let arrival = Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    };
    let (pads, gate, log) = fake();
    gate.open();
    pads.send(arrival.clone());
    assert!(pads.wait_ready(1, Duration::from_secs(1)));
    pads.stop_within(Duration::from_secs(5)).unwrap();
    assert_eq!(log.lock().unwrap().last().unwrap(), "unplugged");

    let (pads, gate, log) = fake();
    pads.send(arrival.clone());
    gate.wait_entered(1);
    let started = Instant::now();
    let error = pads
        .stop_within(Duration::from_millis(50))
        .unwrap_err()
        .to_string();
    assert!(error.contains("did not unplug"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    // Never waited for again: a later call fails at once.
    let started = Instant::now();
    assert!(pads.stop_within(Duration::from_secs(60)).is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
    // Once it ended, it is done.
    gate.open();
    gate.wait_until(|| pads.stop_within(Duration::ZERO).is_ok());
    assert!(log.lock().unwrap().iter().any(|e| e == "unplugged"));

    // Dropping one still stuck returns at once.
    let (pads, gate, log) = fake();
    pads.send(arrival);
    gate.wait_entered(1);
    assert!(pads.stop_within(Duration::from_millis(50)).is_err());
    drop(pads);
    assert!(!log.lock().unwrap().iter().any(|e| e == "unplugged"));
    gate.open();
    gate.wait_until(|| log.lock().unwrap().iter().any(|e| e == "unplugged"));
}

/// What a Nucleus Co-op launch counts as plugged among the parked pads
/// it releases (#56).
#[test]
fn plugged_pads_count_parked_ones_until_the_client_releases_them() {
    let (pads, gate, _) = fake();
    gate.open();
    assert_eq!(pads.plugged_pads(), 0);
    pads.send(Event::Arrival {
        id: 1,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    });
    assert!(pads.wait_ready(2, Duration::from_secs(1)));
    assert_eq!(pads.plugged_pads(), 1);
    pads.park();
    assert_eq!(pads.plugged_pads(), 1);
    pads.send(state(1, 0, 0, 0));
    gate.wait_until(|| pads.plugged_pads() == 0);
}

#[test]
fn controller_audio_status_does_not_cross_removal_or_park() {
    let (pads, gate, log) = fake();
    gate.open();
    let arrival = Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    };
    pads.send(arrival.clone());
    assert!(pads.wait_ready(1, Duration::from_secs(1)));
    let status = [1, 1, 0, 0, 7, 0, 0, 0];
    pads.audio(0, controller_audio::CONTROL, &status);
    gate.wait_until(|| audio_count(&log) == 1);

    let removed = state(0, 0, 0, 0);
    pads.send(removed.clone());
    gate.wait_until(|| log.lock().unwrap().contains(&format!("{removed:?}")));
    pads.send(arrival.clone());
    assert!(pads.wait_ready(1, Duration::from_secs(1)));
    gate.wait_entered(gate.entered.load(Ordering::SeqCst) + 2);
    assert_eq!(audio_count(&log), 1);

    pads.audio(0, controller_audio::CONTROL, &status);
    gate.wait_until(|| audio_count(&log) == 2);
    pads.park();
    gate.wait_until(|| log.lock().unwrap().iter().any(|entry| entry == "released"));
    // A late status from the departed transport is ignored while parked.
    pads.audio(0, controller_audio::CONTROL, &status);
    pads.activate();
    pads.send(arrival);
    assert!(pads.wait_ready(1, Duration::from_secs(1)));
    gate.wait_entered(gate.entered.load(Ordering::SeqCst) + 2);
    assert_eq!(audio_count(&log), 2);
    assert!(
        pads.shared
            .audio_input
            .lock()
            .unwrap()
            .status
            .iter()
            .all(Option::is_none)
    );
}

#[test]
fn controller_audio_status_rejects_mismatched_ids_and_reserved_flags() {
    let mut queue = AudioQueue::default();
    let now = Instant::now();
    for (id, status) in [
        (16, [1, 1, 16, 0, 7, 0, 0, 0]),
        (0, [1, 1, 1, 0, 7, 0, 0, 0]),
        (0, [1, 1, 0, 0, 16, 0, 0, 0]),
        (0, [1, 1, 0, 0, 7, 1, 0, 0]),
    ] {
        queue.push(id, controller_audio::CONTROL, &status, now);
    }
    assert!(queue.take(now).is_empty());
    queue.arrived(16);
    assert!(queue.status.iter().all(Option::is_none));
}

#[test]
fn controller_audio_status_replays_when_its_controller_arrives_later() {
    let mut queue = AudioQueue::default();
    let now = Instant::now();
    let status = [1, 1, 1, 0, 7, 0, 0, 0];
    queue.push(1, controller_audio::CONTROL, &status, now);
    // Another controller is already polling, but controller 1 does not
    // exist yet, so this initial device delivery cannot apply its policy.
    assert_eq!(queue.take(now)[0].payload, status);
    assert!(queue.take(now).is_empty());
    queue.arrived(1);
    let replay = queue.take(now + Duration::from_secs(6));
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].payload, status);
    assert!(queue.take(now + Duration::from_secs(7)).is_empty());
}

#[test]
fn parking_releases_controls_without_unplugging_and_arrival_becomes_ready() {
    let (pads, gate, log) = fake();
    gate.open();
    let arrival = Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    };
    pads.send(arrival.clone());
    assert!(pads.wait_ready(1, Duration::from_secs(1)));
    pads.park();
    gate.wait_until(|| log.lock().unwrap().iter().any(|s| s == "released"));
    assert!(!log.lock().unwrap().iter().any(|s| s == "unplugged"));
    pads.activate();
    pads.send(arrival);
    assert!(pads.wait_ready(1, Duration::from_secs(1)));
    drop(pads);
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .filter(|s| *s == "unplugged")
            .count(),
        1
    );
}

#[test]
fn missing_driver_is_visible_without_blocking_other_input() {
    let warnings = Arc::new(butterpollo_core::session::Warnings::default());
    let pads = GamepadThread::spawn_reported::<Fake>(
        || anyhow::bail!("test driver missing"),
        warnings.clone(),
    )
    .unwrap();
    pads.send(state(0, 1, 0, 0));
    let deadline = Instant::now() + Duration::from_secs(2);
    while warnings.snapshot().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let entries = warnings.snapshot();
    assert_eq!(entries[0].code, "input_gamepad");
    assert!(entries[0].message.contains("test driver missing"));
    assert!(
        entries[0]
            .message
            .contains("keyboard and mouse remain available")
    );
}

#[test]
fn an_unmapped_back_grip_press_shows_a_hint_and_still_reaches_the_pads() {
    let warnings = Arc::new(butterpollo_core::session::Warnings::default());
    let gate = Arc::new(Gate::default());
    gate.open();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut pads = GamepadThread::spawn_reported(
        {
            let (gate, log) = (gate.clone(), log.clone());
            move || {
                Ok(Fake {
                    gate: gate.clone(),
                    log: log.clone(),
                    plugged: false,
                    feedback: vec![],
                })
            }
        },
        warnings.clone(),
    )
    .unwrap();
    // L4 mapped, R4 not: pressing L4 says nothing.
    pads.unmapped_grips = 0x01_0000;
    pads.send(state(0, 1, 0x02_0000, 0));
    assert!(warnings.snapshot().is_empty());
    // A Switch 2 Pro pad has R4 (GR): no hint for it.
    pads.profile = butterpollo_core::input_policy::USBIP_SWITCH2;
    pads.send(state(0, 1, 0x01_0000, 0));
    assert!(warnings.snapshot().is_empty());
    pads.profile = 0;
    pads.send(state(0, 1, 0x01_0000, 0));
    let entries = warnings.snapshot();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].code, "input_back_grips");
    gate.wait_until(|| log.lock().unwrap().len() == 2);
}

#[test]
fn queueing_never_waits_for_a_stalled_driver() {
    let (pads, gate, log) = fake();
    // The first state plugs the pad, and the driver does not return.
    pads.send(state(0, 1, 0, 0));
    gate.wait_entered(1);
    // A press, its release and stick motion meanwhile.
    for i in 1..200 {
        pads.send(state(0, 1, u32::from(i == 50), i));
    }
    // Ending the session waits for the call in progress, not for the
    // queue, and the pad is unplugged before the drop returns.
    pads.shared.stop.store(true, Ordering::Release);
    gate.open();
    drop(pads);
    let mut expected = logged(&[state(0, 1, 0, 0)]);
    expected.push("unplugged".into());
    assert_eq!(*log.lock().unwrap(), expected);
}

#[test]
fn keyboard_and_mouse_do_not_wait_for_a_stalled_driver() {
    static INJECTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (pads, gate, _) = fake();
    let mut injector = super::super::Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    injector.gamepads = Arc::new(pads);
    injector.inject = |inputs| {
        INJECTED.fetch_add(inputs.len(), Ordering::Relaxed);
        inputs.len()
    };
    let controller = |buttons| Event::Controller {
        id: 0,
        active: 1,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    injector.apply(&controller(0)).unwrap();
    gate.wait_entered(1);
    // Nothing reaches the desktop, but keyboard and mouse take the
    // production batching path while the driver call cannot return.
    for buttons in 1..20 {
        assert!(
            injector
                .apply_all(&[
                    controller(buttons),
                    Event::Keyboard {
                        key: 0x46,
                        down: true,
                        flags: 0,
                        modifiers: 0,
                    },
                    Event::Keyboard {
                        key: 0x46,
                        down: false,
                        flags: 0,
                        modifiers: 0,
                    },
                    Event::Relative { x: 0, y: 0 },
                ])
                .is_empty()
        );
    }
    let entered = gate.entered.load(Ordering::SeqCst);
    gate.open();
    assert_eq!(entered, 1);
    assert_eq!(INJECTED.load(Ordering::Relaxed), 19 * 3);
}

#[test]
fn disconnect_does_not_wait_for_the_gamepad_driver() {
    let (pads, gate, log) = fake();
    let mut injector = super::super::Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    injector.gamepads = Arc::new(pads);
    injector.gamepads.send(state(0, 1, 1, 42));
    gate.wait_entered(1);
    injector.detach_gamepads();
    drop(injector);
    assert!(log.lock().unwrap().is_empty());
    gate.open();
    gate.wait_until(|| {
        log.lock()
            .unwrap()
            .last()
            .is_some_and(|entry| entry == "unplugged")
    });
}

#[test]
fn silence_releases_the_pads_before_input_resumes() {
    let (pads, gate, log) = fake();
    pads.send(state(0, 1, 1, 42));
    gate.wait_entered(1);
    pads.send(state(0, 1, 2, 43));
    pads.release_all();
    pads.send(state(0, 1, 4, 44));
    gate.open();
    gate.wait_until(|| log.lock().unwrap().len() == 3);
    drop(pads);
    assert_eq!(
        *log.lock().unwrap(),
        [
            format!("{:?}", state(0, 1, 1, 42)),
            "released".into(),
            format!("{:?}", state(0, 1, 4, 44)),
            "unplugged".into(),
        ]
    );
}

#[test]
fn a_stalled_driver_gets_the_latest_state_and_every_press() {
    let (pads, gate, log) = fake();
    // The first state starts a call that waits; the rest queue behind it.
    pads.send(state(0, 1, 0, 0));
    gate.wait_entered(1);
    for stick in 1..=40 {
        pads.send(state(0, 1, 0, stick));
        pads.send(motion(0, 1, f32::from(stick)));
        pads.send(motion(0, 2, f32::from(stick)));
    }
    pads.send(state(0, 1, 1, 41));
    pads.send(state(0, 1, 0, 42));
    for stick in 43..=60 {
        pads.send(state(0, 1, 0, stick));
    }
    gate.open();
    gate.wait_until(|| log.lock().unwrap().len() == 6);
    drop(pads);
    let mut expected = logged(&[
        state(0, 1, 0, 0),
        state(0, 1, 0, 40),
        motion(0, 1, 40.),
        motion(0, 2, 40.),
        state(0, 1, 1, 41),
        state(0, 1, 0, 60),
    ]);
    expected.push("unplugged".into());
    assert_eq!(*log.lock().unwrap(), expected);
}

#[test]
fn feedback_and_motion_are_reported_back() {
    let (pads, gate, _) = fake();
    let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sender.connect(receiver.local_addr().unwrap()).unwrap();
    sender.set_nonblocking(true).unwrap();
    pads.set_feedback_waker(Arc::new(sender));
    gate.open();
    pads.send(Event::Arrival {
        id: 2,
        kind: 3,
        capabilities: 0x30,
        buttons: 0,
    });
    let mut wake = [0];
    receiver.recv(&mut wake).unwrap();
    receiver.recv(&mut wake).unwrap();
    let reports: Vec<_> = pads.reports().collect();
    assert_eq!(
        reports,
        [
            PadReport::Motion {
                id: 2,
                capabilities: 0x30
            },
            PadReport::Feedback {
                id: 0,
                kind: 1,
                data: vec![1, 2, 3, 4, 0, 0, 0, 0]
            },
        ]
    );
}
#[test]
fn merging_keeps_transitions_arrivals_and_other_pads_in_place() {
    let mut events = VecDeque::new();
    for event in [
        state(0, 3, 0, 1),
        state(1, 3, 0, 1),
        motion(0, 1, 1.),
        state(0, 3, 0, 2), // merges past pad 1 and the motion
        state(1, 3, 2, 2), // pad 1's press keeps its place
        motion(0, 1, 2.),  // merges past both states
        state(0, 1, 0, 3), // pad 1 leaves: a new active mask
        state(0, 1, 0, 4), // merges with the state above only
        Event::Arrival {
            id: 1,
            kind: 1,
            capabilities: 0,
            buttons: 0,
        },
        motion(0, 1, 3.), // never passes the arrival
    ] {
        assert!(enqueue(&mut events, event));
    }
    assert_eq!(
        events,
        [
            state(0, 3, 0, 2),
            state(1, 3, 0, 1),
            motion(0, 1, 2.),
            state(1, 3, 2, 2),
            state(0, 1, 0, 4),
            Event::Arrival {
                id: 1,
                kind: 1,
                capabilities: 0,
                buttons: 0,
            },
            motion(0, 1, 3.),
        ]
    );
}

#[test]
fn a_full_queue_drops_new_transitions_but_still_merges() {
    let mut events = VecDeque::new();
    for i in 0..QUEUE_LIMIT {
        assert!(enqueue(&mut events, state(0, 1, i as u32, 0)));
    }
    assert!(!enqueue(&mut events, state(0, 1, 1 << 20, 0)));
    assert!(enqueue(&mut events, state(0, 1, QUEUE_LIMIT as u32 - 1, 5)));
    assert_eq!(events.len(), QUEUE_LIMIT);
    assert_eq!(events.back(), Some(&state(0, 1, QUEUE_LIMIT as u32 - 1, 5)));
}
