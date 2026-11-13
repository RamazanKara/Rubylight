//! Drive the virtual gamepads off the input thread. Blocking VHF driver
//! calls must not delay keyboard, mouse, touch or the control stream's
//! acknowledgements.
use super::{Event, Gamepads};
use anyhow::Result;
use butterpollo_core::input::Batch;
use butterpollo_core::steam_feedback::{SteamHaptic, SteamPads};
use butterpollo_core::{controller_audio, haptics};
use std::{
    collections::VecDeque,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU16, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HANDLE, WAIT_FAILED},
    System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
};

/// How often feedback is polled and the back button emulated while a pad is
/// plugged in, as the control loop and the C++ host's feedback thread did.
const POLL: Duration = Duration::from_millis(8);
/// How long to wait before opening a driver that failed to open again:
/// opening enumerates devices.
const RETRY: Duration = Duration::from_secs(10);
/// Events waiting for a stalled driver beyond this many are dropped.
const QUEUE_LIMIT: usize = 1024;
/// Moonlight's rumble packet, which synthesized Steam Controller rumble uses.
const RUMBLE_DATA: u16 = 0x010b;
/// With `gamepad_feedback_fast_poll`, feedback is polled this often for
/// FAST_SPAN after a driver had some.
const FAST_POLL: Duration = Duration::from_millis(2);
const FAST_SPAN: Duration = Duration::from_millis(250);
/// How often the `gamepad timings` line is logged while pads are plugged.
const STATS_EVERY: Duration = Duration::from_secs(5);

/// What the gamepad thread hands back to the control stream.
#[derive(Debug, PartialEq)]
pub enum PadReport {
    Haptics(haptics::Frame),
    Speaker(haptics::Frame),
    /// New rumble, lights or trigger effects for a controller.
    Feedback {
        id: u16,
        kind: u16,
        data: Vec<u8>,
    },
    /// A controller arrived on a profile with motion sensors; the client's
    /// arrival capabilities say which sensors to ask for.
    Motion {
        id: u8,
        capabilities: u16,
    },
}

/// The pads the thread drives: VHF, or a stand-in in tests.
pub(super) trait Pads {
    fn warnings(&mut self, _warnings: Arc<butterpollo_core::session::Warnings>) {}
    fn apply(&mut self, event: &Event) -> Result<()>;
    fn refresh(&mut self) -> Result<()>;
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)>;
    /// Feedback reports read from the driver since the last call, coalesced
    /// or not.
    fn feedback_drained(&mut self) -> usize {
        0
    }
    /// Events a driver signals when feedback is waiting (ViGEmBus's
    /// notification requests): the thread waits on them beside its queue
    /// and polls at once instead of at the next tick.
    fn wait_handles(&self, _handles: &mut Vec<HANDLE>) {}
    /// Whether one of [`Pads::wait_handles`]' requests has completed and
    /// waits for the next feedback poll.
    fn feedback_ready(&self) -> bool {
        false
    }
    fn haptics(&mut self) -> Vec<haptics::Frame> {
        vec![]
    }
    fn speaker(&mut self) -> Vec<haptics::Frame> {
        vec![]
    }
    /// Haptic reports Steam wrote to a virtual Steam Controller whose client
    /// has no real pads, for synthesized rumble.
    fn steam_haptics(&mut self) -> Vec<SteamHaptic> {
        vec![]
    }
    fn audio(&mut self, _id: u16, _kind: u16, _payload: &[u8]) {}
    /// Controllers with an attached USB microphone endpoint, excluding VHF fallback.
    fn audio_ready(&self) -> u16 {
        0
    }
    fn motion_supported(&self, id: u16) -> bool;
    /// Whether any pad is plugged in, and so feedback needs polling.
    fn plugged(&self) -> bool;
    fn release_all(&mut self) -> Result<()>;
}
impl Pads for Gamepads {
    fn warnings(&mut self, warnings: Arc<butterpollo_core::session::Warnings>) {
        self.set_warnings(warnings);
    }
    fn apply(&mut self, event: &Event) -> Result<()> {
        let applied = Gamepads::apply(self, event);
        self.follow_driver_host();
        applied
    }
    fn refresh(&mut self) -> Result<()> {
        self.check_driver_host();
        Gamepads::refresh(self)
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        Gamepads::feedback(self)
    }
    fn feedback_drained(&mut self) -> usize {
        self.take_feedback_drained()
    }
    fn wait_handles(&self, handles: &mut Vec<HANDLE>) {
        Gamepads::wait_handles(self, handles);
    }
    fn feedback_ready(&self) -> bool {
        Gamepads::feedback_ready(self)
    }
    fn haptics(&mut self) -> Vec<haptics::Frame> {
        Gamepads::haptics(self)
    }
    fn speaker(&mut self) -> Vec<haptics::Frame> {
        Gamepads::speaker(self)
    }
    fn steam_haptics(&mut self) -> Vec<SteamHaptic> {
        self.take_steam_haptics()
    }
    fn audio(&mut self, id: u16, kind: u16, payload: &[u8]) {
        Gamepads::audio(self, id, kind, payload);
    }
    fn audio_ready(&self) -> u16 {
        Gamepads::audio_ready(self)
    }
    fn motion_supported(&self, id: u16) -> bool {
        Gamepads::motion_supported(self, id)
    }
    fn plugged(&self) -> bool {
        !self.active.is_empty()
            || !self.decks.is_empty()
            || !self.dualsenses.is_empty()
            || !self.switch2s.is_empty()
    }
    fn release_all(&mut self) -> Result<()> {
        Gamepads::release_all(self)
    }
}

struct Shared {
    warnings: Mutex<Arc<butterpollo_core::session::Warnings>>,
    queue: Mutex<Queue>,
    /// Signalled with `wake` for the gamepad thread, which waits on it and
    /// the drivers' feedback events together; `wake` stays for
    /// [`GamepadThread::wait_ready`].
    wake_event: OwnedHandle,
    feedback_waker: Mutex<Option<Arc<std::net::UdpSocket>>>,
    haptics: Mutex<haptics::Queue>,
    speaker: Mutex<haptics::Queue>,
    audio_input: Mutex<AudioQueue>,
    wake: Condvar,
    stop: AtomicBool,
    parked: AtomicBool,
    /// The client's Haptics setting, which Steam Controller rumble follows.
    haptics_on: AtomicBool,
    ready: AtomicU16,
    audio_ready: AtomicU16,
}
impl Shared {
    /// Wake the gamepad thread and anyone waiting for its pads.
    fn wake(&self) {
        self.wake.notify_all();
        // SAFETY: `wake_event` is an event handle Shared owns.
        if let Err(error) = unsafe { SetEvent(HANDLE(self.wake_event.as_raw_handle())) } {
            tracing::error!(%error, "could not wake the virtual controllers' thread");
        }
    }
}
struct AudioInput {
    id: u16,
    kind: u16,
    payload: Vec<u8>,
    at: Instant,
}
#[derive(Default)]
struct AudioQueue {
    // Status is reliable transport state, not a 20 ms audio sample. Keep it
    // until the controller leaves so a slow USB attachment can replay it.
    status: [Option<[u8; 8]>; 16],
    pending: u16,
    pcm: VecDeque<AudioInput>,
}
impl AudioQueue {
    fn push(&mut self, id: u16, kind: u16, payload: &[u8], now: Instant) {
        if id >= 16 || payload.len() > 972 {
            return;
        }
        if kind == controller_audio::CONTROL {
            if controller_audio::status(payload).is_some_and(|(index, _)| index == id) {
                let status = Some(payload.try_into().unwrap());
                if self.status[id as usize] != status {
                    self.status[id as usize] = status;
                    self.pending |= 1 << id;
                }
            }
            return;
        }
        self.pcm
            .retain(|input| now.saturating_duration_since(input.at) <= haptics::MAX_AGE);
        if self.pcm.len() >= 64 {
            self.pcm.pop_front();
        }
        self.pcm.push_back(AudioInput {
            id,
            kind,
            payload: payload.to_vec(),
            at: now,
        });
    }
    fn arrived(&mut self, id: u16) {
        if id < 16 && self.status[id as usize].is_some() {
            self.pending |= 1 << id;
        }
    }
    fn retain_controllers(&mut self, active: u16) {
        for (id, status) in self.status.iter_mut().enumerate() {
            if active & (1 << id) == 0 {
                *status = None;
            }
        }
        self.pending &= active;
        self.pcm.retain(|input| active & (1 << input.id) != 0);
    }
    fn release_pcm(&mut self) {
        self.pcm.clear();
    }
    fn replay_status(&mut self) {
        // Releasing held input temporarily suspends the device's audio too.
        // Restore the same transport's status after that release completes.
        for id in 0..16 {
            self.arrived(id);
        }
    }
    fn clear(&mut self) {
        self.status.fill(None);
        self.pending = 0;
        self.pcm.clear();
    }
    fn take(&mut self, now: Instant) -> Vec<AudioInput> {
        let pending = std::mem::take(&mut self.pending);
        let mut inputs = Vec::new();
        for (id, status) in self.status.iter().enumerate() {
            if pending & (1 << id) != 0
                && let Some(payload) = status
            {
                inputs.push(AudioInput {
                    id: id as u16,
                    kind: controller_audio::CONTROL,
                    payload: payload.to_vec(),
                    at: now,
                });
            }
        }
        inputs.extend(
            self.pcm
                .drain(..)
                .filter(|input| now.saturating_duration_since(input.at) <= haptics::MAX_AGE),
        );
        inputs
    }
}
#[derive(Default)]
struct Queue {
    events: VecDeque<Event>,
    release: bool,
    /// Whether events were dropped since the thread last took the queue.
    full: bool,
    /// When the oldest event waiting was queued, for `gamepad timings`.
    oldest: Option<Instant>,
}
impl Queue {
    /// Hand the worker one batch and give producers its empty spare buffer.
    /// Both allocations survive draining, without holding this lock in a driver call.
    fn take_into(&mut self, batch: &mut VecDeque<Event>) -> bool {
        debug_assert!(batch.is_empty());
        std::mem::swap(&mut self.events, batch);
        self.full = false;
        std::mem::take(&mut self.release)
    }
}

/// One client's virtual gamepads, driven on their own thread. The input
/// thread only queues events and collects reports. Dropping it waits for
/// the thread, which unplugs the pads and frees their slots first, so a
/// later session never sees a stale pad; one [`GamepadThread::stop_within`]
/// gave up on is left running instead.
pub struct GamepadThread {
    shared: Arc<Shared>,
    reports: Mutex<mpsc::Receiver<PadReport>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// [`GamepadThread::stop_within`] timed out: the thread is stuck in the
    /// driver and nothing waits for it again.
    abandoned: AtomicBool,
    /// The back grips no setting maps; pressing one shows a hint.
    unmapped_grips: u32,
}
impl GamepadThread {
    /// The driver is opened with the first controller event, on the thread.
    pub fn new(
        profile: u16,
        policy: butterpollo_core::input_policy::Policy,
        warnings: Arc<butterpollo_core::session::Warnings>,
    ) -> std::io::Result<Self> {
        // The virtual Steam Controller has the grips itself.
        let unmapped_grips = if profile == butterpollo_core::input_policy::VHF_STEAM_CONTROLLER {
            0
        } else {
            policy.unmapped_back_grips()
        };
        let pad_warnings = warnings.clone();
        let polling = Polling {
            every: POLL,
            fast: policy.feedback_fast_poll,
        };
        let mut thread = Self::spawn_polled(
            move || Gamepads::open_options(profile, policy.clone(), pad_warnings.clone()),
            warnings,
            polling,
        )?;
        thread.unmapped_grips = unmapped_grips;
        Ok(thread)
    }
    /// Test pads, polled as the VHF pads are by default.
    #[cfg(test)]
    fn spawn_reported<P: Pads + 'static>(
        open: impl FnMut() -> Result<P> + Send + 'static,
        warnings: Arc<butterpollo_core::session::Warnings>,
    ) -> std::io::Result<Self> {
        Self::spawn_polled(
            open,
            warnings,
            Polling {
                every: POLL,
                fast: false,
            },
        )
    }
    /// The thread with feedback polled as `poll` says; tests stretch it.
    fn spawn_polled<P: Pads + 'static>(
        open: impl FnMut() -> Result<P> + Send + 'static,
        warnings: Arc<butterpollo_core::session::Warnings>,
        poll: Polling,
    ) -> std::io::Result<Self> {
        // SAFETY: CreateEventW returned a new auto-reset event, which only
        // this OwnedHandle owns.
        let wake_event = unsafe {
            OwnedHandle::from_raw_handle(
                CreateEventW(None, false, false, None)
                    .map_err(std::io::Error::other)?
                    .0,
            )
        };
        let shared = Arc::new(Shared {
            warnings: Mutex::new(warnings),
            queue: Mutex::default(),
            wake_event,
            feedback_waker: Mutex::default(),
            haptics: Mutex::default(),
            speaker: Mutex::default(),
            audio_input: Mutex::default(),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            parked: AtomicBool::new(false),
            haptics_on: AtomicBool::new(true),
            ready: AtomicU16::new(0),
            audio_ready: AtomicU16::new(0),
        });
        let (sender, reports) = mpsc::channel();
        let thread = std::thread::Builder::new().name("gamepads".into()).spawn({
            let shared = shared.clone();
            move || run(&shared, open, &sender, poll)
        })?;
        Ok(Self {
            shared,
            reports: Mutex::new(reports),
            thread: Mutex::new(Some(thread)),
            abandoned: AtomicBool::new(false),
            unmapped_grips: 0,
        })
    }
    /// Queue an event for the pads; never waits for the driver.
    pub fn set_warnings(&self, warnings: Arc<butterpollo_core::session::Warnings>) {
        *self.shared.warnings.lock().unwrap() = warnings;
    }
    pub fn send(&self, event: Event) {
        if let Event::Arrival { id, .. } = &event
            && *id < 16
        {
            self.shared.ready.fetch_and(!(1 << *id), Ordering::Release);
        }
        if let Event::Controller { buttons, .. } = &event
            && buttons & self.unmapped_grips != 0
        {
            self.shared.warnings.lock().unwrap().event(
                "input_back_grips",
                butterpollo_core::input_policy::BACK_GRIP_HINT,
                butterpollo_core::session::EVENT_PERIOD,
            );
        }
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.oldest.is_none() {
            queue.oldest = Some(Instant::now());
        }
        if !enqueue(&mut queue.events, event) && !std::mem::replace(&mut queue.full, true) {
            self.shared.warnings.lock().unwrap().event("input_gamepad_queue", "Virtual gamepad driver is not keeping up; controller input was dropped. Check the virtual gamepad driver or choose another supported profile.", butterpollo_core::session::EVENT_PERIOD);
        }
        drop(queue);
        self.shared.wake();
    }
    /// Feedback and motion requests the thread has produced since the last call.
    pub fn reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        let pcm: Vec<_> = self
            .shared
            .haptics
            .lock()
            .unwrap()
            .drain(Instant::now())
            .collect();
        let reports: Vec<_> = self.reports.lock().unwrap().try_iter().collect();
        let speaker: Vec<_> = self
            .shared
            .speaker
            .lock()
            .unwrap()
            .drain(Instant::now())
            .collect();
        reports
            .into_iter()
            .chain(pcm.into_iter().map(PadReport::Haptics))
            .chain(speaker.into_iter().map(PadReport::Speaker))
    }
    pub(super) fn set_feedback_waker(&self, waker: Arc<std::net::UdpSocket>) {
        *self.shared.feedback_waker.lock().unwrap() = Some(waker);
    }
    pub(super) fn release_all(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.events.clear();
        queue.oldest = None;
        queue.release = true;
        self.shared.haptics.lock().unwrap().retain_controllers(0);
        self.shared.speaker.lock().unwrap().retain_controllers(0);
        self.shared.audio_input.lock().unwrap().release_pcm();
        drop(queue);
        self.shared.wake();
    }
    /// Follow the client's Haptics setting: turning it off stops Steam
    /// Controller rumble and drops its pending pulses at the next poll.
    pub(super) fn set_haptics(&self, enabled: bool) {
        self.shared.haptics_on.store(enabled, Ordering::Release);
        drop(self.shared.queue.lock());
        self.shared.wake();
    }
    /// Release controls immediately while keeping the device identity for reconnect.
    pub fn audio(&self, id: u16, kind: u16, payload: &[u8]) {
        let mut queue = self.shared.audio_input.lock().unwrap();
        if !self.shared.parked.load(Ordering::Acquire) {
            queue.push(id, kind, payload, Instant::now());
        }
    }
    pub fn controller_audio_ready(&self, id: u16) -> bool {
        id < 16
            && !self.shared.parked.load(Ordering::Acquire)
            && !self.shared.stop.load(Ordering::Acquire)
            && self.shared.audio_ready.load(Ordering::Acquire) & (1 << id) != 0
    }
    pub fn park(&self) {
        self.shared.parked.store(true, Ordering::Release);
        self.shared.audio_input.lock().unwrap().clear();
        *self.shared.feedback_waker.lock().unwrap() = None;
        self.release_all();
        self.reports.lock().unwrap().try_iter().for_each(drop);
    }
    pub fn activate(&self) {
        self.reports.lock().unwrap().try_iter().for_each(drop);
        self.shared.haptics.lock().unwrap().retain_controllers(0);
        self.shared.speaker.lock().unwrap().retain_controllers(0);
        self.shared.audio_input.lock().unwrap().clear();
        // A new client starts with haptics on, as its injector does.
        self.shared.haptics_on.store(true, Ordering::Release);
        self.shared.parked.store(false, Ordering::Release);
    }
    /// Only the blocking HTTPS launch worker waits here, never the control loop.
    pub fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut queue = self.shared.queue.lock().unwrap();
        while self.shared.ready.load(Ordering::Acquire) & mask != mask {
            let now = Instant::now();
            if now >= end || self.shared.stop.load(Ordering::Acquire) {
                return false;
            }
            queue = self.shared.wake.wait_timeout(queue, end - now).unwrap().0;
        }
        true
    }
    /// How many of its pads are plugged in.
    pub fn plugged_pads(&self) -> u32 {
        self.shared.ready.load(Ordering::Acquire).count_ones()
    }
    /// Tell the thread to end, without waiting: several stopped at once
    /// unplug at once.
    pub fn request_stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        // Taking the lock orders the flag before a wait that checked it.
        drop(self.shared.queue.lock());
        self.shared.wake();
    }
    pub fn stop(&self) -> Option<JoinHandle<()>> {
        self.request_stop();
        self.thread.lock().unwrap().take()
    }
    /// Stop the thread and wait up to `timeout` for it to end, which unplugs
    /// its pads. A thread stuck in the driver is an error; once it timed out
    /// it is never waited for again: a later call fails at once while it
    /// still runs, and is Ok once it ended. Dropping it then doesn't wait.
    pub fn stop_within(&self, timeout: Duration) -> Result<()> {
        self.request_stop();
        let deadline = match self.abandoned.load(Ordering::Acquire) {
            true => Instant::now(),
            false => Instant::now() + timeout,
        };
        loop {
            // The handle stays put while it runs, so callers at the same
            // time all wait for it and all see how it ends.
            let mut slot = self.thread.lock().unwrap();
            let Some(thread) = slot.as_ref() else {
                return Ok(());
            };
            if thread.is_finished() {
                let thread = slot.take().expect("checked above");
                drop(slot);
                if thread.join().is_err() {
                    // Unwinding dropped the pads too.
                    tracing::warn!("the virtual controllers' thread panicked");
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.abandoned.store(true, Ordering::Release);
                anyhow::bail!(
                    "the virtual controllers did not unplug: the virtual gamepad driver isn't responding, so restart Rubylight and start again"
                );
            }
            drop(slot);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for GamepadThread {
    fn drop(&mut self) {
        if let Some(thread) = self.stop() {
            if self.abandoned.load(Ordering::Acquire) && !thread.is_finished() {
                tracing::warn!(
                    "the virtual controllers' thread is stuck in the driver; left running"
                );
                return;
            }
            let _ = thread.join();
        }
    }
}

/// Queue `event`, or fold it into a queued event of the same pad as
/// Input::merge allows: a stick or motion update replaces the queued one, so
/// a stalled driver catches up with the latest state, while every button
/// and active-mask change keeps its place. An update may pass over other
/// pads' events and other kinds of event, never over an arrival. Returns
/// false when the event was dropped: QUEUE_LIMIT events already wait.
fn enqueue(events: &mut VecDeque<Event>, event: Event) -> bool {
    if !matches!(event, Event::Arrival { .. }) {
        for queued in events.iter_mut().rev() {
            if matches!(queued, Event::Arrival { .. }) {
                break;
            }
            let same_kind = std::mem::discriminant(&*queued) == std::mem::discriminant(&event);
            match queued.merge(&event) {
                Batch::Merged => return true,
                Batch::Stop if same_kind => break,
                Batch::Skip | Batch::Stop => {}
            }
        }
    }
    if events.len() >= QUEUE_LIMIT {
        return false;
    }
    events.push_back(event);
    true
}

/// How often the thread polls feedback while pads are plugged.
#[derive(Clone, Copy)]
struct Polling {
    every: Duration,
    /// `gamepad_feedback_fast_poll`: every FAST_POLL for FAST_SPAN after a
    /// driver had feedback.
    fast: bool,
}
impl Polling {
    fn next(&self, now: Instant, fast_until: Option<Instant>) -> Instant {
        let fast = self.fast && fast_until.is_some_and(|until| now < until);
        now + if fast {
            FAST_POLL.min(self.every)
        } else {
            self.every
        }
    }
}

/// What the `gamepad timings` line reports: how long controller events
/// waited from [`GamepadThread::send`] until the driver had them, and how
/// much feedback each poll read.
#[derive(Default)]
struct Stats {
    since: Option<Instant>,
    /// Per batch: the oldest event's time from queued to submitted.
    waits: Vec<Duration>,
    depth_max: usize,
    polls: usize,
    drained: usize,
    drained_max: usize,
}
impl Stats {
    fn batch(&mut self, queued: Instant, depth: usize, now: Instant) {
        self.since.get_or_insert(now);
        if self.waits.len() < 1 << 16 {
            self.waits.push(now.saturating_duration_since(queued));
        }
        self.depth_max = self.depth_max.max(depth);
    }
    fn poll(&mut self, drained: usize, now: Instant) {
        self.since.get_or_insert(now);
        self.polls += 1;
        self.drained += drained;
        self.drained_max = self.drained_max.max(drained);
    }
    /// The line for the window that ended, once STATS_EVERY has passed.
    fn take_due(&mut self, now: Instant) -> Option<Self> {
        let since = self.since?;
        (now.saturating_duration_since(since) >= STATS_EVERY).then(|| std::mem::take(self))
    }
    fn log(mut self) {
        let micros = |d: Option<&Duration>| d.map_or(0, |d| d.as_micros() as u64);
        self.waits.sort_unstable();
        let at = |q: usize| {
            self.waits
                .get((self.waits.len().saturating_sub(1)) * q / 100)
        };
        tracing::info!(
            batches = self.waits.len(),
            queued_p50_us = micros(at(50)),
            queued_p99_us = micros(at(99)),
            queued_max_us = micros(self.waits.last()),
            queue_depth_max = self.depth_max,
            feedback_polls = self.polls,
            feedback_drained = self.drained,
            feedback_drained_max = self.drained_max,
            "gamepad timings"
        );
    }
}

/// Wait without the queue's lock until an event is queued, a driver signals
/// feedback in `handles` (after the wake event at 0), or `timeout` passes.
fn wait<'a>(
    shared: &'a Shared,
    queue: MutexGuard<'a, Queue>,
    handles: &[HANDLE],
    timeout: Option<Duration>,
) -> MutexGuard<'a, Queue> {
    drop(queue);
    // Rounded up: a wait that ends before its deadline would only spin.
    let ms = timeout.map_or(INFINITE, |timeout| {
        u32::try_from(timeout.as_micros().div_ceil(1000))
            .map_or(INFINITE - 1, |ms| ms.min(INFINITE - 1))
    });
    // SAFETY: the wake event lives in `shared`, and the drivers' events in
    // the pads this thread owns and does not touch while it waits.
    if unsafe { WaitForMultipleObjects(handles, false, ms) } == WAIT_FAILED {
        // Only an invalid handle fails this: loud, and no busy loop.
        tracing::error!(
            error = %windows::core::Error::from_thread(),
            "the virtual controllers' thread could not wait for events"
        );
        std::thread::sleep(timeout.unwrap_or(POLL).min(POLL));
    }
    shared.queue.lock().unwrap()
}

fn run<P: Pads>(
    shared: &Shared,
    mut open: impl FnMut() -> Result<P>,
    reports: &mpsc::Sender<PadReport>,
    poll: Polling,
) {
    // Mock workers must not boost their priority or register a Windows task.
    #[cfg(not(test))]
    let _priority = crate::capture::Priority::input();
    let report = |report| {
        if !shared.parked.load(Ordering::Acquire)
            && reports.send(report).is_ok()
            && let Some(waker) = shared.feedback_waker.lock().unwrap().as_ref()
        {
            let _ = waker.send(&[1]);
        }
    };
    // Synthesized Steam Controller rumble, sent whenever a motor changes.
    let mut steam = SteamPads::default();
    let send_steam = |steam: &mut SteamPads| {
        for (id, low, high) in steam.changes(Instant::now()) {
            report(PadReport::Feedback {
                id,
                kind: RUMBLE_DATA,
                data: [low.to_le_bytes(), high.to_le_bytes()].concat(),
            });
        }
    };
    let mut pads: Option<P> = None;
    let mut retry: Option<Instant> = None;
    let mut poll_at = Instant::now();
    // Feedback is polled while a pad is plugged in and once more after the
    // last one leaves, which forgets its last report; otherwise the thread
    // sleeps until an event comes.
    let mut polling = false;
    let mut known_controllers = 0u16;
    let mut events = VecDeque::new();
    let mut handles = Vec::new();
    let mut stats = Stats::default();
    let mut queued = None;
    let mut fast_until = None;
    let stopping = || shared.stop.load(Ordering::Acquire);
    loop {
        let release = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if stopping() {
                    // Dropping the pads unplugs them.
                    return;
                }
                if queue.release || !queue.events.is_empty() {
                    queued = queue.oldest.take();
                    break queue.take_into(&mut events);
                }
                if pads
                    .as_ref()
                    .is_some_and(|pads| (polling || pads.plugged()) && pads.feedback_ready())
                {
                    break false;
                }
                handles.clear();
                handles.push(HANDLE(shared.wake_event.as_raw_handle()));
                if let Some(pads) = &pads {
                    pads.wait_handles(&mut handles);
                }
                if !polling && !pads.as_ref().is_some_and(P::plugged) {
                    queue = wait(shared, queue, &handles, None);
                    continue;
                }
                let now = Instant::now();
                if now >= poll_at {
                    break false;
                }
                queue = wait(shared, queue, &handles, Some(poll_at - now));
            }
        };
        if release && let Some(pads) = &mut pads {
            let released = pads.release_all();
            // The old iteration may already have drained audio when release
            // was queued. Only replay after the device has been suspended.
            shared.audio_input.lock().unwrap().replay_status();
            if let Err(error) = released {
                shared.warnings.lock().unwrap().event("input_gamepad_injection", format!("Controller release failed ({error:#}); held controls may remain active. Check the virtual gamepad driver."), butterpollo_core::session::EVENT_PERIOD);
            }
        }
        if release {
            // Released or parked at session end: no rumble outlives it.
            steam.retain(0);
            send_steam(&mut steam);
        }
        let depth = events.len();
        for event in events.drain(..) {
            if stopping() {
                return;
            }
            if shared.queue.lock().unwrap().release {
                break;
            }
            if pads.is_none() {
                let now = Instant::now();
                if retry.is_some_and(|at| now < at) {
                    continue;
                }
                match open() {
                    Ok(opened) => {
                        shared.warnings.lock().unwrap().clear("input_gamepad");
                        pads = Some(opened);
                    }
                    Err(error) => {
                        shared
                            .warnings
                            .lock()
                            .unwrap()
                            .set("input_gamepad", super::gamepad_backend::unavailable(&error));
                        retry = Some(now + RETRY);
                        continue;
                    }
                }
            }
            let Some(pads) = &mut pads else { continue };
            pads.warnings(shared.warnings.lock().unwrap().clone());
            match &event {
                Event::Arrival { id, .. } if *id < 16 => {
                    shared
                        .haptics
                        .lock()
                        .unwrap()
                        .retain_controllers(!(1 << *id));
                    steam.retain(!(1 << *id));
                    send_steam(&mut steam);
                }
                Event::Controller { active, .. } => {
                    shared.haptics.lock().unwrap().retain_controllers(*active);
                    steam.retain(*active);
                    send_steam(&mut steam);
                }
                _ => {}
            }
            let applied = pads.apply(&event);
            shared
                .audio_ready
                .store(pads.audio_ready(), Ordering::Release);
            match applied {
                Err(error) => shared.warnings.lock().unwrap().event("input_gamepad_injection", format!("Controller input failed ({error:#}); some controls may not work. Check the selected gamepad profile and virtual driver."), butterpollo_core::session::EVENT_PERIOD),
                Ok(()) => {
                    match &event {
                        Event::Arrival { id, .. } if *id < 16 => {
                            known_controllers |= 1 << *id;
                            shared.audio_input.lock().unwrap().arrived(u16::from(*id));
                            shared.ready.fetch_or(1 << *id, Ordering::Release);
                            drop(shared.queue.lock());
                            shared.wake.notify_all();
                        }
                        Event::Controller { id, active, .. } => {
                            let mut audio = shared.audio_input.lock().unwrap();
                            audio.retain_controllers(*active);
                            if *id < 16 && active & (1 << *id) != 0 {
                                if known_controllers & (1 << *id) == 0 {
                                    audio.arrived(*id);
                                }
                                known_controllers |= 1 << *id;
                            }
                            known_controllers &= *active;
                            shared.ready.fetch_and(*active, Ordering::Release);
                        }
                        _ => {}
                    }
                    if let Event::Arrival {
                        id, capabilities, ..
                    } = event
                        && pads.motion_supported(u16::from(id))
                    {
                        report(PadReport::Motion { id, capabilities });
                    }
                }
            }
        }
        let now = Instant::now();
        if let Some(queued) = queued.take()
            && depth > 0
            && pads.is_some()
        {
            stats.batch(queued, depth, now);
        }
        // A driver's completed feedback request is forwarded at once and its
        // next request started, not left to the next tick.
        if let Some(pads) = &mut pads
            && (polling || pads.plugged())
            && (now >= poll_at || pads.feedback_ready())
        {
            if let Err(error) = pads.refresh() {
                tracing::debug!(%error, "controller refresh failed");
            }
            shared
                .audio_ready
                .store(pads.audio_ready(), Ordering::Release);
            let feedback = pads.feedback();
            let drained = pads.feedback_drained();
            if !feedback.is_empty() || drained > 0 {
                fast_until = Some(now + FAST_SPAN);
            }
            poll_at = poll.next(now, fast_until);
            for (id, kind, data) in feedback {
                report(PadReport::Feedback { id, kind, data });
            }
            stats.poll(drained, now);
            if let Some(window) = stats.take_due(now) {
                window.log();
            }
            let steam_haptics = pads.steam_haptics();
            if shared.haptics_on.load(Ordering::Acquire) && !shared.parked.load(Ordering::Acquire) {
                for haptic in &steam_haptics {
                    steam.apply(haptic, now);
                }
            } else {
                steam.retain(0);
            }
            send_steam(&mut steam);
            let audio_input = shared.audio_input.lock().unwrap().take(Instant::now());
            for input in audio_input {
                if !shared.parked.load(Ordering::Acquire) {
                    pads.audio(input.id, input.kind, &input.payload);
                }
            }
            let speaker = pads.speaker();
            if !shared.parked.load(Ordering::Acquire) {
                let mut queue = shared.speaker.lock().unwrap();
                for frame in speaker {
                    queue.push(frame, now);
                }
            }
            let pcm = pads.haptics();
            if !pcm.is_empty() && !shared.parked.load(Ordering::Acquire) {
                let mut queue = shared.haptics.lock().unwrap();
                for frame in pcm {
                    queue.push(frame, now);
                }
                drop(queue);
                if let Some(waker) = shared.feedback_waker.lock().unwrap().as_ref() {
                    let _ = waker.send(&[1]);
                }
            }
            polling = pads.plugged();
            if !polling {
                // The last pad left: the next line covers only the next pads.
                stats = Stats::default();
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
                windows::Win32::System::Threading::ResetEvent(HANDLE(event.as_raw_handle()))
                    .unwrap()
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
}
