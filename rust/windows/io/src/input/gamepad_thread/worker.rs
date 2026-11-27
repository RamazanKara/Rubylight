//! Background worker loop, event wait, and controller setup replay for [`super::GamepadThread`].

use super::*;

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

/// The controller a setup event (identity, arrival, Steam Controller
/// attributes) is for.
fn setup_id(event: &Event) -> Option<u8> {
    match event {
        Event::Arrival { id, .. } => Some(*id),
        Event::ControllerIdentity(identity) => Some(identity.controller),
        Event::SteamAttributes(attributes) => Some(attributes.controller),
        _ => None,
    }
}

/// Keeps each controller's latest identity, arrival and Steam Controller
/// attributes while it stays connected. Pads opened anew get them again
/// before any other event, so every pad comes back as the type it was.
pub(super) fn keep_setup(setup: &mut Vec<Event>, event: &Event) {
    if let Event::Controller { active, .. } = event {
        // Identities stay, as the pads keep them: one comes just before its
        // controller's arrival, which another's state may precede.
        setup.retain(|kept| {
            matches!(kept, Event::ControllerIdentity(_))
                || setup_id(kept).is_some_and(|id| id < 16 && active & (1 << id) != 0)
        });
        return;
    }
    let Some(id) = setup_id(event) else {
        return;
    };
    // In place: the replayed arrivals keep their order, and with it which
    // driver slot, and so which player number, each pad gets.
    match setup.iter_mut().find(|kept| {
        setup_id(kept) == Some(id) && std::mem::discriminant(*kept) == std::mem::discriminant(event)
    }) {
        Some(kept) => *kept = event.clone(),
        None => setup.push(event.clone()),
    }
}

pub(super) fn run<P: Pads>(
    shared: &Arc<Shared>,
    mut open: impl FnMut() -> Result<P>,
    reports: &mpsc::Sender<PadReport>,
    poll: Polling,
) {
    let attach_wake: Arc<dyn Fn() + Send + Sync> = {
        let shared = Arc::clone(shared);
        Arc::new(move || shared.wake())
    };
    // Mock workers must not boost their priority or register a Windows task.
    #[cfg(not(test))]
    let _priority = crate::os_helpers::InputPriority::enter();
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
    // What pads opened anew need to plug each controller as the type it was,
    // and the driver's recent refusals of the pads' handle (#138).
    let mut setup = Vec::new();
    let mut losses = Losses::default();
    let stopping = || shared.stop.load(Ordering::Acquire);
    let sync_audio_ready = |audio_ready: u16| -> bool {
        let previous = shared.audio_ready.swap(audio_ready, Ordering::AcqRel);
        let newly_ready = audio_ready & !previous;
        if newly_ready != 0 {
            shared.audio_input.lock().unwrap().arrived_mask(newly_ready);
            return true;
        }
        false
    };
    loop {
        let (release, pending_policy) = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if stopping() {
                    // Dropping the pads unplugs them.
                    return;
                }
                if queue.release || queue.policy.is_some() || !queue.events.is_empty() {
                    queued = queue.oldest.take();
                    break queue.take_into(&mut events);
                }
                if pads.as_ref().is_some_and(|pads| {
                    pads.attach_ready() || ((polling || pads.plugged()) && pads.feedback_ready())
                }) {
                    break (false, None);
                }
                handles.clear();
                handles.push(HANDLE(shared.wake_event.as_raw_handle()));
                if let Some(pads) = &pads {
                    pads.wait_handles(&mut handles);
                }
                if !polling && !pads.as_ref().is_some_and(P::plugged) {
                    match reopen_at(pads.is_some(), retry, &setup, &losses) {
                        Some(at) => {
                            let now = Instant::now();
                            if now >= at {
                                break (false, None);
                            }
                            queue = wait(shared, queue, &handles, Some(at - now));
                        }
                        None => queue = wait(shared, queue, &handles, None),
                    }
                    continue;
                }
                let now = Instant::now();
                if now >= poll_at {
                    break (false, None);
                }
                queue = wait(shared, queue, &handles, Some(poll_at - now));
            }
        };
        if let Some(policy) = pending_policy
            && let Some(pads_now) = &mut pads
        {
            pads_now.warnings(shared.warnings.lock().unwrap().clone());
            if let Err(error) = pads_now.set_policy(policy)
                && !pads_now.lost()
            {
                shared.warnings.lock().unwrap().event("input_gamepad_injection", format!("Controller policy update failed ({error:#}); some controls may not work. Check the selected gamepad profile and virtual driver."), butterpollo_session::session::EVENT_PERIOD);
            }
            shared
                .steam_pads
                .store(pads_now.steam_pads(), Ordering::Release);
        }
        let fresh_arrival = events
            .iter()
            .any(|event| matches!(event, Event::Arrival { .. }));
        if release && let Some(pads) = &mut pads {
            let released = pads.release_all();
            // The old iteration may already have drained audio when release
            // was queued. Only replay after the device has been suspended.
            shared.audio_input.lock().unwrap().replay_status();
            if let Err(error) = released
                && !pads.lost()
            {
                shared.warnings.lock().unwrap().event("input_gamepad_injection", format!("Controller release failed ({error:#}); held controls may remain active. Check the virtual gamepad driver."), butterpollo_session::session::EVENT_PERIOD);
            }
        }
        if release {
            // Released or parked at session end: no rumble outlives it.
            steam.retain(0);
            send_steam(&mut steam);
        }
        if forget_lost(shared, &mut pads, &mut losses, &mut retry, &poll) {
            (known_controllers, polling) = (0, false);
            steam.retain(0);
            send_steam(&mut steam);
        }
        if shared.fresh.swap(false, Ordering::AcqRel) {
            // A new stream or launch: pads the driver refused, even those
            // opened only seldom, open at its first input, and refusals
            // count anew.
            if pads.is_none() && (losses.count > 0 || losses.open_failures > 0) {
                retry = None;
            }
            // What the refused pads kept still goes to the next ones.
            let memory = losses.memory.take();
            if std::mem::take(&mut losses).warned {
                shared.warnings.lock().unwrap().clear(DRIVER_LOST);
            }
            losses.memory = memory;
        }
        if fresh_arrival && (losses.count > 0 || losses.open_failures > 0 || retry.is_some()) {
            // A controller freshly reconnected (e.g. after AFK/idle disconnect):
            // reset retry/backoff immediately instead of sitting in 60 s SLOW_RETRY (#138).
            if pads.is_none() {
                retry = None;
            }
            if losses.reset_on_arrival() {
                shared.warnings.lock().unwrap().clear(DRIVER_LOST);
            }
        }
        if reopen_at(pads.is_some(), retry, &setup, &losses).is_some_and(|at| Instant::now() >= at)
            && let Some(mut opened) = open_driver(shared, &mut open, &mut losses, &mut retry, &poll)
        {
            opened.set_wake(Arc::clone(&attach_wake));
            // The controllers arrive again without waiting for input.
            pads = Some(opened);
            for kept in std::mem::take(&mut setup).into_iter().rev() {
                events.push_front(kept);
            }
        }
        let depth = events.len();
        while let Some(event) = events.pop_front() {
            if stopping() {
                return;
            }
            if shared.queue.lock().unwrap().release {
                break;
            }
            if pads.is_none() {
                // Stopped, the driver waits `poll.slow_retry()` for input.
                if retry.is_some_and(|at| Instant::now() < at) {
                    keep_setup(&mut setup, &event);
                    continue;
                }
                let Some(mut opened) =
                    open_driver(shared, &mut open, &mut losses, &mut retry, &poll)
                else {
                    keep_setup(&mut setup, &event);
                    continue;
                };
                opened.set_wake(Arc::clone(&attach_wake));
                pads = Some(opened);
                if !setup.is_empty() {
                    // The controllers arrive again first, on pads opened after
                    // the driver refused the last ones' handle or after it
                    // could not be opened. Merge a fresh Arrival in place so the
                    // same controller is not applied twice.
                    if matches!(event, Event::Arrival { .. }) {
                        keep_setup(&mut setup, &event);
                    } else {
                        events.push_front(event);
                    }
                    for kept in std::mem::take(&mut setup).into_iter().rev() {
                        events.push_front(kept);
                    }
                    continue;
                }
            }
            let Some(pads_now) = &mut pads else { continue };
            pads_now.warnings(shared.warnings.lock().unwrap().clone());
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
            let applied = pads_now.apply(&event);
            if sync_audio_ready(pads_now.audio_ready()) {
                poll_at = Instant::now();
            }
            shared
                .steam_pads
                .store(pads_now.steam_pads(), Ordering::Release);
            keep_setup(&mut setup, &event);
            if pads_now.lost() {
                // Not the profile's fault: the pads are closed and the
                // driver opened again, which the log says.
                if forget_lost(shared, &mut pads, &mut losses, &mut retry, &poll) {
                    (known_controllers, polling) = (0, false);
                    steam.retain(0);
                    send_steam(&mut steam);
                }
                continue;
            }
            for (id, capabilities) in pads_now.motion_requests() {
                tracing::debug!(controller = id, "asking the client for controller motion");
                report(PadReport::Motion { id, capabilities });
            }
            match applied {
                Err(error) => shared.warnings.lock().unwrap().event("input_gamepad_injection", format!("Controller input failed ({error:#}); some controls may not work. Check the selected gamepad profile and virtual driver."), butterpollo_session::session::EVENT_PERIOD),
                Ok(()) => {
                    if losses.recovered(Instant::now()) {
                        shared.warnings.lock().unwrap().clear(DRIVER_LOST);
                    }
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
                        && pads_now.motion_supported(u16::from(id))
                    {
                        report(PadReport::Motion { id, capabilities });
                    }
                }
            }
        }
        // Preserve unapplied controller setup (Arrival, Identity, SteamAttributes) when
        // release_all interrupts delivery, discarding only controller state/haptic traffic.
        for event in events.drain(..) {
            keep_setup(&mut setup, &event);
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
            && (pads.attach_ready()
                || ((polling || pads.plugged()) && (now >= poll_at || pads.feedback_ready())))
        {
            if let Err(error) = pads.refresh() {
                tracing::debug!(%error, "controller refresh failed");
            }
            for (id, capabilities) in pads.motion_requests() {
                tracing::debug!(controller = id, "asking the client for controller motion");
                report(PadReport::Motion { id, capabilities });
            }
            sync_audio_ready(pads.audio_ready());
            shared
                .steam_pads
                .store(pads.steam_pads(), Ordering::Release);
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
            let mut wake_feedback = false;
            let speaker = pads.speaker();
            if !speaker.is_empty() && !shared.parked.load(Ordering::Acquire) {
                let mut queue = shared.speaker.lock().unwrap();
                for frame in speaker {
                    queue.push(frame, now);
                }
                wake_feedback = true;
            }
            let pcm = pads.haptics();
            if !pcm.is_empty() && !shared.parked.load(Ordering::Acquire) {
                let mut queue = shared.haptics.lock().unwrap();
                for frame in pcm {
                    queue.push(frame, now);
                }
                wake_feedback = true;
            }
            if wake_feedback && let Some(waker) = shared.feedback_waker.lock().unwrap().as_ref() {
                let _ = waker.send(&[1]);
            }
            polling = pads.plugged();
            if !polling {
                // The last pad left: the next line covers only the next pads.
                stats = Stats::default();
            }
        }
        // A feedback poll the driver refused: no more polls on that handle.
        if forget_lost(shared, &mut pads, &mut losses, &mut retry, &poll) {
            (known_controllers, polling) = (0, false);
            steam.retain(0);
            send_steam(&mut steam);
        }
    }
}
