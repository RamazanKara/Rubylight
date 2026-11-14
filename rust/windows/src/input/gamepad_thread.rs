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
    /// Controllers whose client should be asked for motion again, with their
    /// arrival capabilities.
    fn motion_requests(&mut self) -> Vec<(u8, u16)> {
        Vec::new()
    }
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
    fn motion_requests(&mut self) -> Vec<(u8, u16)> {
        Gamepads::motion_requests(self)
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
    /// The back grips no setting maps; pressing one the controller's pad lacks shows a hint.
    unmapped_grips: u32,
    /// The `gamepad` profile and the controllers' identities, for the grips each pad has
    /// ([`butterpollo_core::input_policy::pad_back_grips`]).
    profile: u16,
    identities: Mutex<std::collections::BTreeMap<u16, (u16, u16)>>,
}
impl GamepadThread {
    /// The driver is opened with the first controller event, on the thread.
    pub fn new(
        profile: u16,
        policy: butterpollo_core::input_policy::Policy,
        warnings: Arc<butterpollo_core::session::Warnings>,
    ) -> std::io::Result<Self> {
        let unmapped_grips = policy.unmapped_back_grips();
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
        thread.profile = profile;
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
            profile: 0,
            identities: Mutex::default(),
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
        if let Event::ControllerIdentity(identity) = &event {
            self.identities.lock().unwrap().insert(
                u16::from(identity.controller),
                (identity.vendor, identity.product),
            );
        }
        if let Event::Controller { id, buttons, .. } = &event
            && buttons & self.unmapped_grips != 0
            && buttons
                & self.unmapped_grips
                & !butterpollo_core::input_policy::pad_back_grips(
                    self.profile,
                    self.identities.lock().unwrap().get(id).copied(),
                )
                != 0
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
            for (id, capabilities) in pads.motion_requests() {
                tracing::debug!(
                    controller = id,
                    "controller motion went silent; asking the client again"
                );
                report(PadReport::Motion { id, capabilities });
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
#[path = "gamepad_thread/tests.rs"]
pub(crate) mod tests;
