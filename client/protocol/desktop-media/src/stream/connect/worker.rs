//! Worker thread lifecycle, cancellation deadlines, and teardown waiting for
//! desktop stream connection attempts.

use super::*;

/// Why a launch ended without a stream.
pub enum ConnectFailed {
    /// What to tell the user.
    Message(String),
    /// The PC asks the user to confirm closing the running app; the plan launches again with
    /// the answer.
    ConfirmReplace(Box<LaunchPlan>),
    /// A recovery found that the stream moved to `device`; it stops there.
    Moved { host: u64, app: i32, device: String },
    /// The PC refused full chroma after the launch: the plan that asked for it, whose app now runs
    /// on the PC, starts once more without it.
    PackedRefused(Box<LaunchPlan>),
}

/// A launch and session start running on a worker thread.
pub struct Connecting {
    pub shared: Arc<Shared>,
    pub interrupt: Interrupt,
    pub stage: String,
    pub app_name: String,
    pub host_name: String,
    pub host_id: u64,
    pub pacing: PacingMode,
    pub settings: Settings,
    pub(in crate::stream) plan: LaunchPlan,
    pub(in crate::stream) result: Receiver<Result<(Session, Facts, LaunchPlan), ConnectError>>,
    /// The worker's last sign of progress: [`CONNECT_TIMEOUT`] counts from here.
    pub(in crate::stream) progress: Instant,
    /// The worker, joined when the app leaves (`end_all_before`); None if it could not be started.
    pub(in crate::stream) worker: Option<std::thread::JoinHandle<()>>,
}

/// How long leaving waits for a launch in progress to end after cancelling it.
pub const LEAVE_WAIT: Duration = Duration::from_secs(5);

/// How long a launch may go without a sign of progress before the app ends it.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Whether a launch last showed progress at `progress` has waited [`CONNECT_TIMEOUT`] at `now`.
pub fn connect_overdue(progress: Instant, now: Instant) -> bool {
    now.saturating_duration_since(progress) >= CONNECT_TIMEOUT
}

/// Leaving: cancels all launches in progress and tears down once all finish within `wait`.
pub fn end_all_before(attempts: Vec<Connecting>, wait: Duration, teardown: impl FnOnce()) -> bool {
    let deadline = Instant::now() + wait;
    for connecting in &attempts {
        connecting.cancel();
    }
    let mut stopped = true;
    for connecting in attempts {
        let left = deadline.saturating_duration_since(Instant::now());
        if !connecting.finish(left) {
            stopped = false;
        }
    }
    if !stopped {
        eprintln!(
            "nightjar: a connection attempt did not end within {} ms of leaving; the window and its GPU device stay until the process exits, as it may still be using them",
            wait.as_millis()
        );
        return false;
    }
    teardown();
    true
}

/// Leaving with one launch in progress: [`end_all_before`].
#[cfg(test)]
pub fn end_before(connecting: Option<Connecting>, wait: Duration, teardown: impl FnOnce()) -> bool {
    end_all_before(connecting.into_iter().collect(), wait, teardown)
}

/// Waits for sessions still stopping before this attempt's first request.
pub(in crate::stream) fn wait_for_previous(
    previous: Vec<crate::stream::Teardown>,
    shared: &Shared,
    interrupt: &Interrupt,
) {
    let deadline = Instant::now() + crate::stream::LEAVE_WAIT;
    for mut ended in previous {
        let mut stopped = ended.join_until(Instant::now());
        while !stopped && !interrupt.is_interrupted() && Instant::now() < deadline {
            stopped = ended.join_until((Instant::now() + Duration::from_millis(20)).min(deadline));
        }
        if !stopped {
            if !interrupt.is_interrupted() {
                eprintln!(
                    "nightjar: the previous session was still stopping after {} ms; starting while it ends",
                    crate::stream::LEAVE_WAIT.as_millis()
                );
            }
            lock(&shared.teardowns).push(ended);
        }
    }
}

impl Connecting {
    /// What this start was asked to do: a failed start's card makes it again.
    pub fn plan(&self) -> &LaunchPlan {
        &self.plan
    }

    /// Sessions this attempt set aside to stop.
    pub fn take_teardowns(&self) -> Vec<crate::stream::Teardown> {
        std::mem::take(&mut *lock(&self.shared.teardowns))
    }

    /// The game this stream plays, for the recent list.
    pub fn recent_game(&self) -> Option<config::Recent> {
        recent_game(&self.plan)
    }

    pub fn start(
        mut plan: LaunchPlan,
        waker: Waker,
        previous: Vec<crate::stream::Teardown>,
    ) -> Self {
        #[cfg(target_os = "macos")]
        unsafe {
            if SDL_WasInit(SDL_INIT_AUDIO).0 == 0 && !SDL_InitSubSystem(SDL_INIT_AUDIO) {
                eprintln!(
                    "nightjar: stream audio could not start: {}",
                    crate::sdl::error()
                );
            }
        }
        if let Some(recovery) = &mut plan.recovery {
            recovery.begin(recovery::now_ms());
        }
        let limit = if plan.pacing == PacingMode::HostTimes {
            8
        } else {
            2
        };
        let shared = Shared::new(waker.clone(), limit);
        shared.host_times.store(
            plan.pacing == PacingMode::HostTimes,
            std::sync::atomic::Ordering::Relaxed,
        );
        let interrupt = Interrupt::default();
        let (sender, result) = channel();
        let app_name = plan.app.name.clone();
        let host_name = plan.host_name.clone();
        let (host_id, pacing, settings) = (plan.host_id, plan.pacing, plan.settings.clone());
        let worker_shared = shared.clone();
        let worker_interrupt = interrupt.clone();
        let worker_plan = plan.clone();
        let stopping = Arc::new(std::sync::Mutex::new(previous));
        let worker_stopping = stopping.clone();
        let worker = std::thread::Builder::new()
            .name("connect".into())
            .spawn(move || {
                let previous = std::mem::take(&mut *lock(&worker_stopping));
                drop(worker_stopping);
                wait_for_previous(previous, &worker_shared, &worker_interrupt);
                // Scope `worker_plan` (and any `DeviceRef` / `Arc<Shared>`) so they are dropped
                // strictly before `sender.send(result)` can unblock teardown on the UI thread.
                let result: Result<(Session, Facts, LaunchPlan), ConnectError> = {
                    let mut worker_plan = worker_plan;
                    match connect_with_recovery(&mut worker_plan, worker_shared, &worker_interrupt)
                    {
                        Ok((session, _)) if worker_interrupt.is_interrupted() => {
                            session.stop();
                            Err("Cancelled.".into())
                        }
                        Ok((session, facts)) => Ok((session, facts, worker_plan)),
                        Err(e) => Err(e),
                    }
                };
                let _ = sender.send(result);
                waker();
            })
            .inspect_err(|e| eprintln!("nightjar: the connect thread cannot start: {e}"))
            .ok();
        if worker.is_none() {
            lock(&shared.teardowns).append(&mut lock(&stopping));
        }
        Self {
            shared,
            interrupt,
            stage: initial_stage_text().into(),
            app_name,
            host_name,
            host_id,
            pacing,
            settings,
            plan,
            result,
            progress: Instant::now(),
            worker,
        }
    }

    /// Cancels the launch and waits up to `wait` for the worker to end, stopping a session it
    /// started.
    pub fn finish(self, wait: Duration) -> bool {
        self.cancel();
        let deadline = Instant::now() + wait;
        let mut teardown = crate::stream::Teardown::none();
        match self.result.recv_timeout(wait) {
            Ok(Ok((session, _, _))) => {
                lock(&self.shared.roam_watch).take();
                teardown = crate::stream::reap_session(session);
            }
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return false,
        }
        let mut worker_ended = true;
        if let Some(worker) = self.worker {
            while !worker.is_finished() {
                if Instant::now() >= deadline {
                    worker_ended = false;
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            if worker_ended && worker.join().is_err() {
                eprintln!("nightjar: the connect thread panicked");
            }
        }
        let mut teardown = teardown;
        let mut stopped = teardown.join_until(deadline);
        for mut ended in std::mem::take(&mut *lock(&self.shared.teardowns)) {
            stopped &= ended.join_until(deadline);
        }
        if !stopped {
            eprintln!(
                "nightjar: a session did not stop within {} ms of leaving; the renderer stays until the process exits",
                wait.as_millis()
            );
        }
        worker_ended && stopped
    }

    /// The stream presents for a VRR display.
    pub fn vrr(&self) -> bool {
        self.plan.vrr
    }

    /// Ok(Some) once streaming, Err on failure.
    pub fn poll(&mut self) -> Result<Option<(Session, Facts)>, ConnectFailed> {
        let expired = self
            .plan
            .recovery
            .as_ref()
            .is_some_and(|r| r.remaining(recovery::now_ms()) == 0);
        if expired {
            self.cancel();
        }
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        let mut keep = Vec::new();
        let mut decoder_failed = None;
        for notice in notices {
            match notice {
                Notice::Stage(stage) => {
                    self.stage = stage_text(stage).into();
                    self.progress = Instant::now();
                }
                Notice::Reconnecting(attempt) => {
                    keep.clear();
                    decoder_failed = None;
                    self.stage = reconnecting_stage_text(attempt);
                    self.progress = Instant::now();
                }
                notice @ (Notice::DecoderFailed(_) | Notice::RenderDeviceLost(_)) => {
                    if let Notice::DecoderFailed(text) | Notice::RenderDeviceLost(text) = &notice {
                        decoder_failed.get_or_insert_with(|| text.clone());
                    }
                    keep.push(notice);
                }
                other => keep.push(other),
            }
        }
        if !keep.is_empty() {
            let mut queue = lock(&self.shared.notices);
            keep.append(&mut queue);
            *queue = keep;
        }
        match self.result.try_recv() {
            Ok(Ok((session, facts, plan))) => {
                if expired {
                    lock(&self.shared.roam_watch).take();
                    lock(&self.shared.teardowns).push(crate::stream::reap_session(session));
                    Err(ConnectFailed::Message(recovery_gave_up(
                        lock(&self.shared.last_failure).as_deref(),
                    )))
                } else {
                    self.plan = plan;
                    Ok(Some((session, facts)))
                }
            }
            Ok(Err(e)) => Err(if let Some(device) = e.moved_to {
                ConnectFailed::Moved {
                    host: self.host_id,
                    app: self.plan.app.id,
                    device,
                }
            } else if expired {
                ConnectFailed::Message(recovery_gave_up(lock(&self.shared.last_failure).as_deref()))
            } else if e.asks_replace {
                ConnectFailed::ConfirmReplace(Box::new(self.plan.clone()))
            } else if e.packed_refused {
                ConnectFailed::PackedRefused(Box::new(self.plan.clone()))
            } else {
                ConnectFailed::Message(decoder_failed.unwrap_or(e.message))
            }),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(ConnectFailed::Message(
                "The connection attempt ended.".into(),
            )),
        }
    }

    pub fn cancel(&self) {
        self.interrupt.interrupt();
        if let Some(client) = &*lock(&self.shared.host_client) {
            client.cancel_pending_requests();
        }
    }
}

pub(in crate::stream) struct ConnectError {
    pub(in crate::stream) message: String,
    pub(in crate::stream) retryable: bool,
    pub(in crate::stream) asks_replace: bool,
    pub(in crate::stream) moved_to: Option<String>,
    pub(in crate::stream) packed_refused: bool,
    pub(in crate::stream) cause: Option<&'static str>,
}

impl From<String> for ConnectError {
    fn from(message: String) -> Self {
        Self {
            message,
            retryable: false,
            asks_replace: false,
            moved_to: None,
            packed_refused: false,
            cause: None,
        }
    }
}

impl From<&str> for ConnectError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
