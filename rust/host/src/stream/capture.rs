//! Latest-frame ownership and notifications shared by capture consumers.
use anyhow::{Result, bail};
use butterpollo_windows::device_loss::DeviceLost;
use butterpollo_windows::timing::{Signal, Timer};
use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

/// Diagnostic identities outlive replacement of the latest frame without
/// keeping the frame or its GPU resources alive. Only TRACE publications add
/// records; expired records are pruned by the next traced publication.
struct PublicationTrace<T> {
    next_id: u64,
    live: Vec<(Weak<T>, u64)>,
}
impl<T> PublicationTrace<T> {
    fn new() -> Self {
        Self {
            next_id: 0,
            live: Vec::new(),
        }
    }
    fn observe(&mut self, image: &Arc<T>) -> u64 {
        self.live.retain(|(image, _)| image.strong_count() > 0);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.live.push((Arc::downgrade(image), self.next_id));
        self.next_id
    }
    fn id(&self, image: &Arc<T>) -> Option<u64> {
        self.live
            .iter()
            .rev()
            .find(|(recorded, _)| recorded.as_ptr() == Arc::as_ptr(image))
            .map(|(_, id)| *id)
    }
}

struct State<T> {
    image: Option<Arc<T>>,
    error: Option<String>,
    captured: Option<Instant>,
    cadence: butterpollo_core::capture_policy::Freshness,
    generation: u64,
    trace: PublicationTrace<T>,
    recovery: super::device_recovery::DeviceRecovery,
}
impl<T> State<T> {
    fn check(&self) -> Result<()> {
        if let Some(error) = &self.error {
            bail!("capture stopped: {error}");
        }
        if self.recovery.active() {
            self.recovery.check(Instant::now())?;
        }
        Ok(())
    }
}
/// Each stream acknowledges a reset only after releasing its encoder, images
/// and filters. Dropping the subscription removes a departing stream's lease.
pub(super) struct Consumer {
    signal: Signal,
    released: AtomicU64,
    session: Weak<crate::state::Session>,
    recovery_pending: AtomicBool,
}
impl Consumer {
    pub(super) fn wake_on_recovery<P, A>(
        self: &Arc<Self>,
        session: &butterpollo_core::session::Session<P, A>,
    ) {
        let wake = Arc::downgrade(self);
        let menu = wake.clone();
        *session.recovery_wake.lock().unwrap() = Some(Box::new(move || {
            if let Some(wake) = wake.upgrade() {
                wake.recovery_pending.store(true, Ordering::Release);
                let _ = wake.set();
            }
        }));
        // A couch menu change re-encodes a still picture the same way.
        session.couch_menu.set_wake(move || {
            if let Some(wake) = menu.upgrade() {
                wake.recovery_pending.store(true, Ordering::Release);
                let _ = wake.set();
            }
        });
    }
    fn prepare_wait(&self) -> Result<bool> {
        self.signal.reset()?;
        // A recovery request before the reset must still bypass the wait.
        Ok(!self.recovery_pending.swap(false, Ordering::AcqRel))
    }
}
impl std::ops::Deref for Consumer {
    type Target = Signal;
    fn deref(&self) -> &Signal {
        &self.signal
    }
}
pub(super) struct Latest<T> {
    state: Mutex<State<T>>,
    changed: Mutex<Vec<Weak<Consumer>>>,
    origin: Instant,
    trace_source_id: u64,
    recovering: AtomicBool,
    /// Running counts of publications and of the pictures among them (see
    /// `Freshness::picture_interval`), for the pipeline frame rates.
    captures: AtomicU64,
    pictures: AtomicU64,
}
impl<T> Latest<T> {
    pub(super) fn new() -> Self {
        static NEXT_SOURCE_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            state: Mutex::new(State {
                image: None,
                error: None,
                captured: None,
                cadence: Default::default(),
                generation: 0,
                trace: PublicationTrace::new(),
                recovery: Default::default(),
            }),
            changed: Mutex::new(Vec::new()),
            origin: Instant::now(),
            trace_source_id: NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed),
            recovering: AtomicBool::new(false),
            captures: AtomicU64::new(0),
            pictures: AtomicU64::new(0),
        }
    }
    /// This slot's identity and its running picture and capture counts, for
    /// `pipeline_rates::Follow`.
    pub(super) fn frame_counts(&self) -> (usize, u64, u64) {
        (
            self.trace_source_id as usize,
            self.pictures.load(Ordering::Relaxed),
            self.captures.load(Ordering::Relaxed),
        )
    }
    pub(super) fn subscribe(&self, session: Weak<crate::state::Session>) -> Result<Arc<Consumer>> {
        let state = self.state.lock().unwrap();
        let signal = Arc::new(Consumer {
            signal: Signal::new()?,
            // A new subscriber owns nothing from an earlier generation.
            released: AtomicU64::new(state.generation),
            session,
            recovery_pending: AtomicBool::new(false),
        });
        let mut waiters = self.changed.lock().unwrap();
        waiters.retain(|waiter| waiter.strong_count() > 0);
        waiters.push(Arc::downgrade(&signal));
        Ok(signal)
    }
    fn notify(&self) -> Result<()> {
        let mut waiters = self.changed.lock().unwrap();
        waiters.retain(|waiter| waiter.strong_count() > 0);
        for waiter in waiters.iter().filter_map(Weak::upgrade) {
            waiter.set()?;
        }
        Ok(())
    }
    pub(super) fn begin_recovery(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.check()?;
        state.generation = state.generation.wrapping_add(1);
        let image = state.image.take();
        state.captured = None;
        state.cadence = Default::default();
        let result = self.notify();
        // GPU resource release can wait inside the driver. Consumers must be
        // able to observe the reset and release their own resources meanwhile.
        drop(state);
        drop(image);
        result
    }
    pub(super) fn device_lost(
        self: &Arc<Self>,
        loss: DeviceLost,
        warnings: &Arc<butterpollo_core::session::Warnings>,
        now: Instant,
    ) -> Result<()>
    where
        T: Send + Sync + 'static,
    {
        let mut state = self.state.lock().unwrap();
        state.check()?;
        if state.recovery.lost(loss, now) {
            warnings.set("gpu_recovery", format!("{loss}. Rebuilding capture and encoder; the picture may freeze for up to 30 seconds. If the adapter remains unavailable (Code 31), reboot Windows before reconnecting."));
            // Vendor teardown can block inside a DLL. End the clients independently
            // of that worker; Windows cannot safely cancel an in-process driver call.
            let latest = Arc::downgrade(self);
            let warnings = warnings.clone();
            let started = state.recovery.started();
            std::thread::Builder::new()
                .name("gpu-recovery".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        let Some(latest) = latest.upgrade() else {
                            break;
                        };
                        let state = latest.state.lock().unwrap();
                        if state.recovery.started() != started || state.error.is_some() {
                            break;
                        }
                        let result = state.recovery.check(Instant::now());
                        drop(state);
                        if let Err(error) = result {
                            warnings.set("gpu_recovery", format!("{error:#}"));
                            let _ = latest.fail(format!("{error:#}"));
                            break;
                        }
                    }
                })?;
        }
        self.recovering.store(true, Ordering::Release);
        let image = state.image.take();
        let result = self.notify();
        drop(state);
        drop(image);
        result
    }
    pub(super) fn take_device_restart(&self) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        state.check()?;
        Ok(std::mem::take(&mut state.recovery.restart))
    }
    pub(super) fn device_recovered(&self, warnings: &butterpollo_core::session::Warnings) {
        if !self.recovering.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.error.is_none()
            && state.recovery.check(Instant::now()).is_ok()
            && state.recovery.recovered()
        {
            self.recovering.store(false, Ordering::Release);
            warnings.clear("gpu_recovery");
            warnings.event("gpu_reset", "The graphics device was reset; capture and encoding have resumed with a keyframe. Check the graphics driver if this repeats.", butterpollo_core::session::EVENT_PERIOD);
            tracing::info!("GPU recovery completed; encoded output resumed");
        }
    }
    /// Call only after releasing every resource belonging to this capture.
    pub(super) fn release_generation(&self, consumer: &Consumer) {
        let state = self.state.lock().unwrap();
        consumer.released.store(state.generation, Ordering::Release);
    }
    pub(super) fn consumers_released(&self) -> bool {
        let state = self.state.lock().unwrap();
        self.changed
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
            .all(|consumer| consumer.released.load(Ordering::Acquire) == state.generation)
    }
    #[cfg(test)]
    pub(super) fn publish_captured(&self, image: Arc<T>, captured: Instant) -> Result<()> {
        self.publish_focus(image, captured, None)
    }
    /// Publish a capture and whether it changed the focus window, `None`
    /// when the capture cannot tell (`Freshness::observe_focus`).
    pub(super) fn publish_focus(
        &self,
        image: Arc<T>,
        captured: Instant,
        focus: Option<bool>,
    ) -> Result<()> {
        let now = Instant::now();
        let nanos =
            |duration: std::time::Duration| duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        let mut state = self.state.lock().unwrap();
        state.check()?;
        if state.recovery.restart {
            return Ok(());
        }
        state.recovery.captured = true;
        let picture = state.cadence.picture_start();
        state.cadence.observe_focus(
            nanos(captured.saturating_duration_since(self.origin)),
            nanos(now.saturating_duration_since(captured)),
            focus,
        );
        self.captures.fetch_add(1, Ordering::Relaxed);
        if state.cadence.picture_start() != picture {
            self.pictures.fetch_add(1, Ordering::Relaxed);
        }
        if tracing::enabled!(target: "pacing", tracing::Level::TRACE) {
            let capture_id = state.trace.observe(&image);
            tracing::trace!(
                target: "pacing",
                capture_id,
                generation = state.generation,
                captured_ns = nanos(captured.saturating_duration_since(self.origin)),
                published_ns = nanos(now.saturating_duration_since(self.origin)),
                age_ns = nanos(now.saturating_duration_since(captured)),
                source_id = self.trace_source_id,
                "publish"
            );
        }
        state.captured = Some(captured);
        let previous = state.image.replace(image);
        drop(state);
        // Releasing an old GPU image can block inside the driver. Wake consumers of the
        // new image first, while both images still have their own resource ownership.
        let notified = self.notify();
        drop(previous);
        notified
    }
    /// Match a claimed frame and the latest publication in one diagnostic snapshot.
    /// Do not clone images here: a diagnostic clone could become the last GPU owner
    /// and delay admission while releasing it. Untraced frames have identity zero.
    pub(super) fn trace_selection(&self, image: &Arc<T>) -> (u64, bool, u64) {
        let state = self.state.lock().unwrap();
        let selected = state.trace.id(image).unwrap_or(0);
        let current = state.image.as_ref();
        (
            selected,
            current.is_some_and(|current| Arc::ptr_eq(current, image)),
            current
                .and_then(|current| state.trace.id(current))
                .unwrap_or(0),
        )
    }
    pub(super) fn trace_source_id(&self) -> u64 {
        self.trace_source_id
    }
    /// The source's recent frame interval, as observed by the capture worker.
    pub(super) fn source_interval(&self) -> Option<std::time::Duration> {
        self.state
            .lock()
            .unwrap()
            .cadence
            .median_interval()
            .map(std::time::Duration::from_nanos)
    }
    /// The source's recent interval between pictures, counting captures less
    /// than 2 ms apart as one picture.
    pub(super) fn picture_interval(&self) -> Option<std::time::Duration> {
        self.state
            .lock()
            .unwrap()
            .cadence
            .picture_interval()
            .map(std::time::Duration::from_nanos)
    }
    /// The mean interval between the source's recent pictures, which a source
    /// alternating between two picture cadences hides from their median.
    pub(super) fn picture_mean(&self) -> Option<std::time::Duration> {
        self.state
            .lock()
            .unwrap()
            .cadence
            .picture_mean_interval()
            .map(std::time::Duration::from_nanos)
    }
    /// When the source's latest picture was first captured, the picture
    /// interval nearly all recent pictures arrived within, and whether the
    /// latest capture only extends the focus window's picture.
    pub(super) fn picture(&self) -> (Option<Instant>, Option<std::time::Duration>, bool) {
        let state = self.state.lock().unwrap();
        (
            state
                .cadence
                .picture_start()
                .map(|nanos| self.origin + std::time::Duration::from_nanos(nanos)),
            state
                .cadence
                .late_picture_interval()
                .map(std::time::Duration::from_nanos),
            state.cadence.extends_picture(),
        )
    }
    /// The source's mean frame interval: its rate, which a source alternating
    /// between two cadences hides from the median.
    pub(super) fn source_rate(&self) -> Option<std::time::Duration> {
        self.state
            .lock()
            .unwrap()
            .cadence
            .mean_interval()
            .map(std::time::Duration::from_nanos)
    }
    pub(super) fn poll_interval(&self, normal: std::time::Duration) -> std::time::Duration {
        let state = self.state.lock().unwrap();
        let Some(captured) = state.captured else {
            return normal;
        };
        let nanos =
            |duration: std::time::Duration| duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        std::time::Duration::from_nanos(
            state
                .cadence
                .poll_wait(nanos(captured.elapsed()), nanos(normal)),
        )
    }
    pub(super) fn fail(&self, error: String) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        for consumer in self
            .changed
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
        {
            if let Some(session) = consumer.session.upgrade() {
                session
                    .launch
                    .warnings
                    .set("stream_failure", format!("Stream ended: {error}"));
                session.fail(format!("the capture failed: {error}"));
            }
        }
        let image = state.image.take();
        state.error = Some(error);
        let result = self.notify();
        drop(state);
        drop(image);
        result
    }
    pub(super) fn current(&self) -> Result<Option<Arc<T>>> {
        let state = self.state.lock().unwrap();
        state.check()?;
        Ok(state.image.clone())
    }
    pub(super) fn check(&self) -> Result<()> {
        self.state.lock().unwrap().check()
    }
    fn wait_if(
        &self,
        timer: &Timer,
        wake: &Consumer,
        deadline: Instant,
        precise: bool,
        extra: Option<&Signal>,
        predicate: impl FnOnce(&State<T>) -> bool,
    ) -> Result<()> {
        let state = self.state.lock().unwrap();
        state.check()?;
        if predicate(&state) && wake.prepare_wait()? {
            drop(state);
            if precise {
                timer.until_or_signal_precise(deadline, wake)?;
            } else {
                timer.until_or_signals(deadline, wake, extra)?;
            }
        }
        Ok(())
    }
    pub(super) fn wait_for_frame(
        &self,
        timer: &Timer,
        wake: &Consumer,
        deadline: Instant,
    ) -> Result<Option<Arc<T>>> {
        self.wait_if(timer, wake, deadline, false, None, |state| {
            // A reset must release the encoder immediately, not wait another
            // frame period while the capture worker waits for its release.
            state.image.is_none() && wake.released.load(Ordering::Acquire) == state.generation
        })?;
        self.current()
    }
    /// Wait while `image` is still the newest picture; `extra`, such as the
    /// encoder's output event, also ends the wait.
    pub(super) fn wait_if_current_or(
        &self,
        timer: &Timer,
        wake: &Consumer,
        image: &Arc<T>,
        deadline: Instant,
        extra: Option<&Signal>,
    ) -> Result<()> {
        self.wait_if(timer, wake, deadline, false, extra, |state| {
            state
                .image
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, image))
        })
    }
    /// Wait for a frame claim with a high-resolution timer, remaining
    /// interruptible by capture and recovery for the entire wait.
    pub(super) fn wait_if_current_precise(
        &self,
        timer: &Timer,
        wake: &Consumer,
        image: &Arc<T>,
        deadline: Instant,
    ) -> Result<()> {
        self.wait_if(timer, wake, deadline, true, None, |state| {
            state
                .image
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, image))
        })
    }
}

#[cfg(test)]
mod tests;
