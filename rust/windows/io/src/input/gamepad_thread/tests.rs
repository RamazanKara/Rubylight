//! Unit tests and gated fake pads harness for [`GamepadThread`].

use super::*;

#[path = "tests/async_attach_tests.rs"]
mod async_attach_tests;
#[path = "tests/audio_lifecycle.rs"]
mod audio_lifecycle;
#[path = "tests/refusal_tests.rs"]
mod refusal_tests;

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
        let (release, policy) = queue.take_into(&mut batch);
        assert_eq!(release, round == 3);
        assert!(policy.is_none());
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
    /// Controller 0's pad, once plugged, is the Steam Controller.
    fn steam_pads(&self) -> u16 {
        u16::from(self.plugged)
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
    let pads = Arc::new(pads);
    injector.gamepads = Some(pads.clone());
    pads.send(state(0, 1, 0, 0));
    steam_tone(&reports);
    assert_eq!(rumbles_until(&pads, Some(ON)), [ON]);
    injector.apply(&Event::Haptics(false)).unwrap();
    assert_eq!(rumbles_until(&pads, Some(OFF)), [OFF]);
    // Reports while haptics are off are dropped, not held for later.
    steam_tone(&reports);
    assert!(quiet(&pads));
    injector.apply(&Event::Haptics(true)).unwrap();
    assert!(quiet(&pads));
    steam_tone(&reports);
    assert_eq!(rumbles_until(&pads, Some(ON)), [ON]);
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
        backoff: BACKOFF,
        reopen: REOPEN,
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
fn idle_gamepad_timings_log_at_debug_only() {
    use std::io::Write;
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let capture = |level: tracing::Level, stats: Stats| -> String {
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(level)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || stats.log());
        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
    };

    let start = Instant::now();
    let mut idle = Stats::default();
    idle.poll(0, start);
    assert!(!idle.has_activity());
    assert!(capture(tracing::Level::INFO, idle).is_empty());

    let mut idle_debug = Stats::default();
    idle_debug.poll(0, start);
    let logged = capture(tracing::Level::DEBUG, idle_debug);
    assert!(logged.contains("gamepad timings") && logged.contains("feedback_polls=1"));

    let mut batch_active = Stats::default();
    batch_active.batch(start, 1, start + Duration::from_micros(10));
    assert!(batch_active.has_activity());
    let logged = capture(tracing::Level::INFO, batch_active);
    assert!(logged.contains("gamepad timings") && logged.contains("batches=1"));

    let mut feedback_active = Stats::default();
    feedback_active.poll(2, start);
    assert!(feedback_active.has_activity());
    let logged = capture(tracing::Level::INFO, feedback_active);
    assert!(logged.contains("gamepad timings") && logged.contains("feedback_drained=2"));
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
            backoff: BACKOFF,
            reopen: REOPEN,
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
