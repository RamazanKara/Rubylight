//! Shared capture worker thread, capture key deduplication, device recovery, and
//! exclusive-fullscreen fallback for streaming sessions.

mod state;

use super::*;
pub(super) use state::CaptureKey;
use state::{ClaimGridTracker, Targets};

pub(super) struct Source {
    pub(super) warnings: Arc<butterpollo_session::session::Warnings>,
    pub(super) latest: Arc<Latest>,
    /// The capture backend that opened, after every reopen: `auto` is planned
    /// as WGC, so the plan cannot say whether frames come from WGC or the
    /// display driver.
    pub(super) backend: Arc<Mutex<&'static str>>,
    pub(super) grid: Arc<Mutex<butterpollo_win_graphics::capture::ClaimGrid>>,
    claim_grids: ClaimGridTracker,
    targets: Arc<Targets>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) thread: Option<thread::JoinHandle<()>>,
}

pub(super) fn finish_worker(worker: thread::JoinHandle<()>, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if worker.is_finished() {
        let _ = worker.join();
    } else {
        tracing::error!(
            worker = name,
            "teardown did not return within five seconds; detaching the driver worker"
        );
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            finish_worker(worker, "capture");
        }
    }
}

impl Source {
    pub(super) fn backend(&self) -> &'static str {
        *self.backend.lock().unwrap()
    }

    pub(super) fn attach_target(&self, prepared: &Arc<crate::display_session::Ready>) {
        self.targets.attach(prepared);
    }

    pub(super) fn update_claim_grid(
        &self,
        consumer: &Arc<capture::Consumer>,
        period: Duration,
        anchor: Instant,
    ) {
        self.claim_grids
            .update(&self.grid, consumer, period, anchor);
    }
}

impl std::ops::Deref for Source {
    type Target = Latest;
    fn deref(&self) -> &Latest {
        &self.latest
    }
}

pub(super) fn capture_config(config: &Config) -> Config {
    let mut capture = config.clone();
    // Keep the effective interval choice with the shared capture and its helper.
    // A 1 ms request skipped updates in the 120 FPS motion comparison; use
    // explicit zero at every rate unless the user opts into the legacy limit.
    capture.values.insert(
        "wgc_high_rate_capture".into(),
        config::wgc_high_rate_capture(config).to_string(),
    );
    capture
}

impl Media {
    pub(super) fn capture(
        &self,
        hdr: bool,
        config: &Config,
        rate: butterpollo_media_policy::framegen::Rate,
        phase: &str,
        prepared: Arc<crate::display_session::Ready>,
    ) -> Result<Arc<Source>> {
        // One read of the stream's policy: the method and the repeat mode
        // must agree, or a capture is keyed under one and opened as the other.
        let (kind, constant_rate) = prepared.capture_method();
        let output = prepared.output();
        let aligned = config::wgc_slot_aligned_publish(config);
        // The user's Capture method, not the resolved backend: an explicit method is
        // never moved to another backend (`capture_policy::automatic_capture`).
        let automatic = butterpollo_media_policy::capture_policy::automatic_capture(
            config::capture_setting(config),
        );
        let capture_config = capture_config(config);
        let key = CaptureKey::new(&kind, &output, hdr, &capture_config, phase, constant_rate);
        {
            let mut captures = self.captures.lock().unwrap();
            captures.retain(|_, source| source.strong_count() > 0);
            if let Some(existing) = captures.get(&key).and_then(Weak::upgrade)
                && existing.check().is_ok()
            {
                drop(captures);
                existing.attach_target(&prepared);
                return Ok(existing);
            }
        }
        let targets = Arc::new(Targets::new(&prepared));
        let worker_targets = targets.clone();
        let latest = Arc::new(Latest::new());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = latest.clone();
        let warnings = Arc::new(butterpollo_session::session::Warnings::default());
        let capture_warnings = warnings.clone();
        let grid = Arc::new(Mutex::new(butterpollo_win_graphics::capture::ClaimGrid {
            anchor: Instant::now(),
            period: rate.period(),
        }));
        let worker_grid = grid.clone();
        let backend = Arc::new(Mutex::new(""));
        let worker_backend = backend.clone();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        // Before the wait below begins: the capture's tries end within it.
        let started = Instant::now();
        let thread = thread::Builder::new().name("capture".into()).spawn(move || {
            let result = (|| -> Result<()> {
                let _com = ComGuard::new()?;
                let _priority = Priority::new();
                let _display_awake = butterpollo_win_graphics::timing::DisplayAwake::enter()
                    .inspect_err(
                        |error| tracing::warn!(%error, "capture cannot keep the display awake"),
                    )
                    .ok();
                let timer = butterpollo_win_graphics::timing::Timer::new()?;
                // Duplicate the desktop that is showing, including the secure
                // desktop of a UAC prompt or the lock screen.
                butterpollo_win_io::input::follow_input_desktop();
                let opened = open_first(
                    || worker_targets.capture_target(),
                    |target| {
                        wb::open_capture(
                            &target.0,
                            &kind,
                            hdr,
                            &capture_config,
                            capture_warnings.clone(),
                        )
                    },
                    |tried| {
                        let replaced = worker_targets.capture_target() != *tried;
                        wait_to_open_again(started, &worker_stop, replaced)
                    },
                );
                let (mut capture, mut target) = match opened {
                    Ok(opened) => opened,
                    Err(e) => {
                        let _ = started_tx.send(Err(format!("{e:#}")));
                        return Ok(());
                    }
                };
                // The stream's display (and its revision) when the capture last opened on a
                // new device: a window target switches on that device only while it stays.
                let mut opened_display = worker_targets.display_target();
                capture.set_claim_grid(worker_grid.clone(), aligned);
                *worker_backend.lock().unwrap() = capture.backend();
                tracing::info!(requested=%kind, backend=capture.backend(), output=%target.0, constant_rate = constant_rate, "capture backend opened");
                // Re-create the capture on a new device. Windows keeps handing
                // out the stale adapter while anything holds the old device, so
                // every duplication made then loses access at once (a display
                // arriving for another client does this). The old capture is
                // dropped and consumers acknowledge releasing their frames
                // and encoders before the new device is made.
                // The capture, whether it shows the primary display in
                // place of the stream's, which is gone, and the stream's
                // display it was opened for. The display can be recreated
                // while capture opens; the next check then sees that.
                // `chosen` is normally the stream's own backend; a display whose
                // Windows Graphics Capture picture stopped asks for Desktop
                // Duplication instead. A refused switch keeps the working one.
                let reopen = |lost: Capture,
                              target: &(String, u64),
                              chosen: &str|
                 -> Result<(Capture, bool, (String, u64))> {
                    let recovery_started = Instant::now();
                    // Desktop Duplication reports the pointer's shape only
                    // when it changes; the new capture starts from this one.
                    let mut pointer = lost.pointer();
                    worker.begin_recovery()?;
                    drop(lost);
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while !worker.consumers_released() {
                        worker.check()?;
                        if worker_stop.load(Ordering::Acquire) {
                            bail!("capture stopped while releasing resources");
                        }
                        anyhow::ensure!(
                            Instant::now() < deadline,
                            "streams did not release the lost capture device"
                        );
                        timer.until(Instant::now() + Duration::from_millis(2));
                    }
                    let release_ms = recovery_started.elapsed().as_millis();
                    let mut missing_since = None;
                    loop {
                        worker.check()?;
                        anyhow::ensure!(
                            Instant::now() < deadline,
                            "capture did not recover within 30 seconds"
                        );
                        if worker_stop.load(Ordering::Acquire) {
                            bail!("capture stopped while recovering");
                        }
                        let next = worker_targets.capture_target();
                        // A UAC prompt or the lock screen switches the input
                        // desktop; duplication must be made on that desktop.
                        butterpollo_win_io::input::follow_input_desktop();
                        let background = butterpollo_session::background::enabled(&capture_config);
                        let on = reopen_on(
                            butterpollo_win_graphics::capture::display_present(&next.0),
                            background,
                            &mut missing_since,
                            Instant::now(),
                        );
                        let opened = match on {
                            ReopenOn::Wait => {
                                if background {
                                    capture_warnings.set("capture_display", "The background display disappeared. Waiting for it to recover; the PC's monitor is not captured.");
                                }
                                thread::sleep(RECOVERY_RETRY);
                                continue;
                            }
                            ReopenOn::Stream => match wb::open_capture(
                                &next.0,
                                chosen,
                                hdr,
                                &capture_config,
                                capture_warnings.clone(),
                            ) {
                                Ok(opened) => Ok(opened),
                                // Keep the backend that was working: Desktop
                                // Duplication cannot capture this display (another
                                // vendor's GPU drives it, or it is turned), and a
                                // frozen picture is worse than a picture. The
                                // warning says so after the reopen: opening the
                                // stream's own backend clears it otherwise.
                                Err(error) if chosen != kind.as_str() => {
                                    match wb::open_capture(
                                        &next.0,
                                        &kind,
                                        hdr,
                                        &capture_config,
                                        capture_warnings.clone(),
                                    ) {
                                        Ok(opened) => {
                                            if opened.backend() == "wgc" {
                                                capture_warnings.set("capture_backend", format!("Capturing with Windows Graphics Capture: Desktop Duplication cannot capture this display ({error:#})."));
                                            }
                                            Ok(opened)
                                        }
                                        Err(failure) => Err(failure),
                                    }
                                }
                                Err(error) => Err(error),
                            },
                            // By name: the WGC helper checks it opened the display asked for.
                            ReopenOn::Primary => {
                                butterpollo_win_graphics::capture::primary_display()
                                    .context("no display is on the desktop")
                                    .and_then(|primary| {
                                        wb::open_capture(
                                            &primary,
                                            &kind,
                                            hdr,
                                            &capture_config,
                                            capture_warnings.clone(),
                                        )
                                    })
                            }
                        };
                        match opened {
                            Ok(mut recovered) => {
                                if on == ReopenOn::Primary {
                                    capture_warnings.set("capture_display", format!("The stream's display {} disappeared and did not return within {} s; showing the primary display until it returns. A game or Windows changed the display layout; reconnect if the picture does not return.", next.0, DISPLAY_RETURN_WAIT.as_secs()));
                                } else {
                                    capture_warnings.clear("capture_display");
                                }
                                if next != *target {
                                    tracing::info!(output = %next.0, "capture moved to the recreated display");
                                }
                                if let Some(pointer) = pointer.take()
                                    && let Err(error) = recovered.resume_pointer(pointer)
                                {
                                    tracing::warn!(%error, "the pointer appears once it moves or changes");
                                }
                                tracing::info!(release_ms, elapsed_ms=recovery_started.elapsed().as_millis(), backend=recovered.backend(), "capture reopened after resource release");
                                return Ok((recovered, on == ReopenOn::Primary, next));
                            }
                            Err(error) if Instant::now() >= deadline => return Err(error),
                            Err(_) => thread::sleep(RECOVERY_RETRY),
                        }
                    }
                };
                // A stream that stopped waiting for its capture has none to feed.
                if started_tx.send(Ok(())).is_err() {
                    return Ok(());
                }
                let progress = crate::stall_watch::register("capture", &output, None);
                let mut check_target = Instant::now();
                let mut user_desktop = kind == "wgc"
                    && butterpollo_win_graphics::capture::wgc_desktop_available();
                let mut on_primary = false;
                let poll_interval =
                    Duration::from_micros(config::capture_poll_interval_us(&capture_config));
                let mut last_frame = Instant::now();
                // When to ask again whether a frozen display's picture is one
                // Windows Graphics Capture cannot see; see the `Ok(None)` arm.
                let mut stall_check = Instant::now();
                // One attempt per capture worker, and whether it moved the stream
                // to Desktop Duplication for good: from then on it reopens there.
                let mut duplication_tried = false;
                let mut switched_to_duplication = false;
                // Constant-rate capture (`capture = wgcc`): one picture per
                // stream slot, the last one again when nothing new was
                // composed for it. `origin` names the content of a fresh
                // picture so its repeats reuse the converted picture.
                let mut constant = constant_rate
                    .then(butterpollo_media_policy::capture_policy::ConstantRate::default);
                let mut origin = 0u64;
                while !worker_stop.load(Ordering::Acquire) {
                    if Instant::now() >= check_target {
                        check_target = Instant::now() + Duration::from_millis(100);
                        let next = worker_targets.capture_target();
                        let available = kind == "wgc"
                            && butterpollo_win_graphics::capture::wgc_desktop_available();
                        let return_to_wgc = returns_to_wgc(
                            available,
                            user_desktop,
                            capture.backend(),
                            &kind,
                            switched_to_duplication,
                        );
                        user_desktop = available;
                        // Back from the primary display once the stream's returns.
                        let returned = on_primary
                            && butterpollo_win_graphics::capture::display_present(&next.0);
                        let restart = worker.take_device_restart()?;
                        // The app's window found, minimised or restored on the same display
                        // and not recreated: the new target opens on the capture's own device
                        // and the encoder keeps going. A device problem, another display or
                        // backend reopens on a new device below, releasing the encoder.
                        let in_place = !restart
                            && !return_to_wgc
                            && !returned
                            && next != target
                            && worker_targets.display_target() == opened_display;
                        if in_place
                            && let Some(switched) = wb::switch_capture(
                                &mut capture,
                                &next.0,
                                &kind,
                                hdr,
                                &capture_config,
                                capture_warnings.clone(),
                            )
                        {
                            match switched {
                                Ok(switched) => {
                                    drop(std::mem::replace(&mut capture, switched));
                                    tracing::info!(from = %target.0, to = %next.0, "capture switched target on its device");
                                    target = next.clone();
                                    capture.set_claim_grid(worker_grid.clone(), aligned);
                                    *worker_backend.lock().unwrap() = capture.backend();
                                    last_frame = Instant::now();
                                    continue;
                                }
                                Err(error) => tracing::info!(error = %format!("{error:#}"), to = %next.0, "the capture could not switch target on its device; reopening it"),
                            }
                        }
                        if restart || next != target || return_to_wgc || returned {
                            let lost = std::mem::replace(&mut capture, Capture::Closed);
                            progress.mark(crate::stall_watch::Phase::Reopen);
                            (capture, on_primary, target) = match reopen(
                                lost,
                                &next,
                                reopen_backend(&kind, switched_to_duplication),
                            ) {
                                Ok(capture) => capture,
                                Err(_) if worker_stop.load(Ordering::Acquire) => return Ok(()),
                                Err(error) => return Err(error),
                            };
                            if switched_to_duplication && capture.backend() == "ddx" {
                                capture_warnings.set("capture_backend", DUPLICATION_WARNING);
                            }
                            opened_display = worker_targets.display_target();
                            capture.set_claim_grid(worker_grid.clone(), aligned);
                            *worker_backend.lock().unwrap() = capture.backend();
                            last_frame = Instant::now();
                        }
                    }
                    // Reset before the helper's announcements are read: one
                    // that comes after them ends the wait below.
                    if let Some(signal) = capture.frame_signal() {
                        signal.reset()?;
                    }
                    progress.mark(crate::stall_watch::Phase::Capture);
                    // The publication identity this picture carries when the
                    // capture repeats (`capture = wgcc`): the worker's
                    // counter, which a repeat keeps. 0 when the capture
                    // does not repeat; a picture the capture did not return
                    // must not spend one.
                    let publication = constant
                        .as_ref()
                        .map(|_| origin.wrapping_add(1).max(1))
                        .unwrap_or(0);
                    let captured = (|| {
                        #[cfg(debug_assertions)]
                        crate::soak_fault::check("DXGI_ERROR_ACCESS_LOST")?;
                        capture.next_gpu(publication)
                    })();
                    match captured {
                        Ok(Some(image)) => {
                            last_frame = Instant::now();
                            capture_warnings.clear("capture_recovery");
                            let (captured, focus) = (image.captured, image.focus);
                            if let Some(rate) = &mut constant {
                                origin = publication;
                                rate.published(image.published);
                            }
                            worker.publish_focus(image, captured, focus)?;
                        }
                        Ok(None) => {
                            // Windows Graphics Capture reads the compositor's output. An
                            // app in exclusive fullscreen presents past it, so the picture
                            // stops with no error while the display keeps changing; Desktop
                            // Duplication reads what the display shows (issue #123). Only
                            // Automatic moves: an explicit Capture method is the user's,
                            // including for the one-reconnect comparison.
                            let now = Instant::now();
                            let idle = now.saturating_duration_since(last_frame);
                            let display = butterpollo_win_graphics::capture::WindowTarget::parse(
                                &target.0,
                            )
                            .ok()
                            .flatten()
                            .is_none();
                            // The shell's own answer first, at most twice a second: enumerating
                            // the windows and the displays behind it is the expensive part of
                            // the probe, and most still pictures have no exclusive-fullscreen
                            // app.
                            if automatic
                                && !duplication_tried
                                && capture.backend() == "wgc"
                                && display
                                && idle
                                    >= butterpollo_media_policy::capture_policy::FULLSCREEN_STALL
                                && fullscreen_probe_due(now, &mut stall_check)
                                && butterpollo_win_graphics::foreground::exclusive_fullscreen()
                            {
                                // The display's rectangle as it is now: a game that
                                // changed its mode after the capture opened (issue #123)
                                // would not match the rectangle the capture opened on.
                                let in_front =
                                    butterpollo_win_graphics::foreground::in_front(&target.0);
                                if butterpollo_media_policy::capture_policy::duplication_for_fullscreen(
                                    automatic,
                                    capture.backend(),
                                    display,
                                    idle,
                                    in_front,
                                    true,
                                ) {
                                    duplication_tried = true;
                                    tracing::info!(idle_ms = idle.as_millis(), output = %target.0, "an exclusive-fullscreen app is in front and no picture came; capturing the display with Desktop Duplication");
                                    let lost = std::mem::replace(&mut capture, Capture::Closed);
                                    progress.mark(crate::stall_watch::Phase::Reopen);
                                    (capture, on_primary, target) = match reopen(lost, &target, "ddx") {
                                        Ok(capture) => capture,
                                        Err(_) if worker_stop.load(Ordering::Acquire) => {
                                            return Ok(());
                                        }
                                        Err(error) => return Err(error),
                                    };
                                    if capture.backend() == "ddx" {
                                        switched_to_duplication = true;
                                        capture_warnings.set("capture_backend", DUPLICATION_WARNING);
                                    }
                                    opened_display = worker_targets.display_target();
                                    capture.set_claim_grid(worker_grid.clone(), aligned);
                                    *worker_backend.lock().unwrap() = capture.backend();
                                    last_frame = Instant::now();
                                    continue;
                                }
                            }
                            // The slot's period is the fastest stream using
                            // this capture, so a rate change and a shared
                            // capture both follow it; the grid's period is
                            // the fallback before a stream declares one.
                            let constant_period = constant.as_ref().map(|_| {
                                let fallback = worker_grid.lock().unwrap().period;
                                worker.repeat_period(fallback)
                            });
                            // Nothing composed for this slot: publish the last
                            // picture again, stamped now, so the encoder keeps a
                            // steady cadence instead of repeating a still one.
                            if let (Some(rate), Some(period)) =
                                (constant.as_mut(), constant_period)
                            {
                                let now = Instant::now();
                                if matches!(
                                    rate.decide(now, period, false),
                                    Some(butterpollo_media_policy::capture_policy::Publish::Repeat)
                                ) {
                                    match worker.current()? {
                                        Some(picture) => {
                                            let mut repeat = (*picture).clone();
                                            repeat.captured = now;
                                            repeat.acquired = now;
                                            repeat.published = now;
                                            repeat.wgc_stamp = None;
                                            repeat.focus = None;
                                            repeat.repeat = true;
                                            repeat.origin = picture.origin;
                                            // A refused repeat (a capture
                                            // restart) leaves the slot for the
                                            // next try.
                                            if worker.publish_repeat(Arc::new(repeat), now)? {
                                                rate.repeated(now, period);
                                            }
                                        }
                                        // No picture to repeat (the capture is
                                        // being recreated): wait a period for
                                        // the next slot instead of spinning on
                                        // this one.
                                        None => rate.published(now),
                                    }
                                }
                            }
                            let interval = if config::capture_predictive_poll(&capture_config)
                                && capture.backend() == "ddx"
                            {
                                worker.poll_interval(poll_interval)
                            } else {
                                poll_interval
                            };
                            let interval = capture.wait_interval(last_frame.elapsed(), interval);
                            let now = Instant::now();
                            let deadline = now + interval;
                            // Never sleep past the next constant-rate slot.
                            // One already due could not be published (a
                            // capture restart): the next poll tries again.
                            let due = constant_period.and_then(|period| {
                                constant.as_ref().and_then(|rate| rate.due(period))
                            });
                            let until = capture
                                .publication_deadline()
                                .unwrap_or(deadline)
                                .min(deadline);
                            let until = due
                                .filter(|due| *due > now)
                                .map_or(until, |due| until.min(due));
                            match capture.frame_signal() {
                                // Frame announcements interrupt both active polling
                                // and the longer wait used by a static source.
                                Some(signal) => {
                                    timer.until_or_signal(until, signal)?;
                                }
                                None => timer.until(until),
                            }
                        }
                        Err(e) => {
                            // The app's window shown over the display was minimised or closed:
                            // its worker lays the display back within its check, and the next
                            // target check switches to it on this device, without releasing
                            // the encoder. Any other failure reopens on a new device.
                            if DeviceLost::from_error(&e).is_none()
                                && target != worker_targets.display_target()
                                && butterpollo_win_graphics::capture::WindowTarget::parse(&target.0)
                                    .ok()
                                    .flatten()
                                    .is_some()
                            {
                                let deadline = Instant::now() + WINDOW_TARGET_WAIT;
                                while worker_targets.capture_target() == target
                                    && Instant::now() < deadline
                                    && !worker_stop.load(Ordering::Acquire)
                                {
                                    timer.until(Instant::now() + Duration::from_millis(10));
                                }
                                if worker_targets.capture_target() != target {
                                    tracing::info!(error = %format!("{e:#}"), "the app's window is unavailable; the capture follows its target");
                                    check_target = Instant::now();
                                    continue;
                                }
                            }
                            if let Some(loss) = DeviceLost::from_error(&e) {
                                worker.device_lost(loss, &capture_warnings, Instant::now())?;
                                worker.take_device_restart()?;
                            } else {
                                capture_warnings.set("capture_recovery", format!("Capture interrupted ({e:#}); reopening capture, with a frozen picture until frames resume. If this repeats, keep the display mode stable and check the WGC helper and graphics driver."));
                            }
                            let lost = std::mem::replace(&mut capture, Capture::Closed);
                            progress.mark(crate::stall_watch::Phase::Reopen);
                            (capture, on_primary, target) = match reopen(
                                lost,
                                &target,
                                reopen_backend(&kind, switched_to_duplication),
                            ) {
                                Ok(capture) => capture,
                                Err(_) if worker_stop.load(Ordering::Acquire) => return Ok(()),
                                Err(error) => return Err(error),
                            };
                            if switched_to_duplication && capture.backend() == "ddx" {
                                capture_warnings.set("capture_backend", DUPLICATION_WARNING);
                            }
                            opened_display = worker_targets.display_target();
                            capture.set_claim_grid(worker_grid.clone(), aligned);
                            *worker_backend.lock().unwrap() = capture.backend();
                            last_frame = Instant::now();
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                if DeviceLost::from_error(&e).is_some() {
                    capture_warnings.set("gpu_recovery", format!("{e:#}"));
                }
                let _ = worker.fail(format!("{e:#}"));
                tracing::error!(error=%format!("{e:#}"),"capture worker stopped");
            }
        })?;
        let latest = Arc::new(Source {
            warnings,
            latest,
            backend,
            grid,
            claim_grids: ClaimGridTracker::default(),
            targets,
            stop,
            thread: Some(thread),
        });
        if let Err(error) = started_rx
            .recv_timeout(CAPTURE_START_WAIT)
            .map_err(anyhow::Error::from)
            .and_then(|res| res.map_err(|e| anyhow::anyhow!(e)))
        {
            drop(latest);
            return Err(error);
        }
        let existing = {
            let mut captures = self.captures.lock().unwrap();
            captures.retain(|_, source| source.strong_count() > 0);
            if let Some(existing) = captures.get(&key).and_then(Weak::upgrade)
                && existing.check().is_ok()
            {
                Some(existing)
            } else {
                captures.insert(key, Arc::downgrade(&latest));
                None
            }
        };
        if let Some(existing) = existing {
            drop(latest);
            existing.attach_target(&prepared);
            return Ok(existing);
        }
        Ok(latest)
    }
}
