use super::*;
use std::time::Duration;
#[test]
fn pending_idr_is_served_within_the_short_poll_instead_of_a_stream_period() -> Result<()> {
    use butterpollo_core::{
        rtsp::Negotiated,
        session::{Launch, Role, Session},
        state::Client,
    };
    let session = Session::<(), ()>::new(
        Launch {
            id: "recovery".into(),
            client: Client {
                name: "fixture".into(),
                cert: String::new(),
                uuid: "client".into(),
                perm: u32::MAX,
                enabled: true,
                extra: Default::default(),
            },
            peer: "127.0.0.1".parse().unwrap(),
            app_id: 1,
            background: None,
            key: [0; 16],
            key_id: 1,
            ping: "ping".into(),
            connect_data: 1,
            role: Role::Stream,
            created: Instant::now(),
            rtsp_encrypted: true,
            rtsp_counter: Default::default(),
            rtsp_received: Default::default(),
            preparation: Default::default(),
            vrr_requested: false,
            host_audio: false,
            requested_rate: 0,
            options: Default::default(),
            audio_preparation: Default::default(),
            preparing: Default::default(),
            transport: Default::default(),
            roaming_offered: Default::default(),
            claimed_until: None,
            nucleus: None,
            held: None,
            warnings: Default::default(),
        },
        Negotiated::default(),
    );
    session.idr.store(false, Ordering::Release);
    let latest = Latest::new();
    let image = Arc::new(1u8);
    latest.publish_captured(image.clone(), Instant::now())?;
    let wake = latest.subscribe(Weak::new())?;
    wake.wake_on_recovery(&session);
    assert!(wake.prepare_wait()?);

    session.request_idr();
    // Model the wait without scheduler jitter: resetting the capture event
    // must not postpone a pending IDR until the static screen's next repeat.
    let now = Instant::now();
    let period = Duration::from_secs_f64(1. / 120.);
    let served = if wake.prepare_wait()? {
        now + period
    } else {
        now
    };
    assert!(served <= now + super::super::OUTPUT_POLL);
    assert!(session.idr.swap(false, Ordering::AcqRel));
    assert!(Arc::ptr_eq(&latest.current()?.unwrap(), &image));
    assert!(wake.prepare_wait()?);

    session.request_invalidation(4, 6);
    assert!(!wake.prepare_wait()?);
    assert_eq!(session.invalidation.lock().unwrap().take(), Some((4, 6)));
    assert!(wake.prepare_wait()?);

    // A request after the event reset must also interrupt the next wait.
    let timer = Timer::new()?;
    session.request_idr();
    assert!(timer.until_or_signal(Instant::now() + Duration::from_secs(1), &wake)?);
    Ok(())
}
#[test]
fn publication_trace_matches_frames_still_owned_after_replacement() {
    let mut trace = PublicationTrace::new();
    let old = Arc::new(1u8);
    let old_id = trace.observe(&old);
    let current = Arc::new(2u8);
    let current_id = trace.observe(&current);
    assert!(current_id > old_id);
    assert_eq!(trace.id(&old), Some(old_id));
    assert_eq!(trace.id(&current), Some(current_id));
    assert_eq!(trace.id(&Arc::new(1u8)), None);
}
#[test]
fn publication_trace_does_not_keep_capture_resources_alive() {
    let mut trace = PublicationTrace::new();
    let frame = Arc::new(1u8);
    let released = Arc::downgrade(&frame);
    let old_id = trace.observe(&frame);
    assert_eq!(Arc::strong_count(&frame), 1);
    drop(frame);
    assert!(released.upgrade().is_none());
    let replacement = Arc::new(2u8);
    let new_id = trace.observe(&replacement);
    assert!(new_id > old_id);
    assert_eq!(trace.live.len(), 1);
    assert_eq!(trace.id(&replacement), Some(new_id));
}
#[test]
fn replacing_the_latest_frame_releases_resources_without_holding_the_state_lock() -> Result<()> {
    struct Image {
        latest: Weak<Latest<Image>>,
        released: Arc<AtomicBool>,
    }
    impl Drop for Image {
        fn drop(&mut self) {
            if let Some(latest) = self.latest.upgrade() {
                assert!(latest.state.try_lock().is_ok());
                self.released.store(true, Ordering::Release);
            }
        }
    }
    let latest = Arc::new(Latest::new());
    let released = Arc::new(AtomicBool::new(false));
    latest.publish_captured(
        Arc::new(Image {
            latest: Arc::downgrade(&latest),
            released: released.clone(),
        }),
        Instant::now(),
    )?;
    latest.publish_captured(
        Arc::new(Image {
            latest: Weak::new(),
            released: released.clone(),
        }),
        Instant::now(),
    )?;
    assert!(released.load(Ordering::Acquire));
    Ok(())
}
#[test]
fn new_frame_notification_does_not_wait_for_previous_image_release() -> Result<()> {
    use std::sync::mpsc;

    struct ReleaseGate {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    struct Image {
        value: u8,
        gate: Option<ReleaseGate>,
    }
    impl Drop for Image {
        fn drop(&mut self) {
            if let Some(gate) = &self.gate {
                let _ = gate.entered.send(());
                // Model a vendor resource release that blocks, without a GPU or an
                // unbounded test thread if publication fails unexpectedly.
                let _ = gate
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
            }
        }
    }

    let latest = Arc::new(Latest::new());
    let consumer = latest.subscribe(Weak::new())?;
    let timer = Timer::new()?;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    latest.publish_captured(
        Arc::new(Image {
            value: 1,
            gate: Some(ReleaseGate {
                entered: entered_tx,
                release: Mutex::new(release_rx),
            }),
        }),
        Instant::now(),
    )?;
    consumer.reset()?;
    let worker = latest.clone();
    let publisher = std::thread::spawn(move || {
        worker.publish_captured(
            Arc::new(Image {
                value: 2,
                gate: None,
            }),
            Instant::now(),
        )
    });

    let releasing = entered_rx.recv_timeout(Duration::from_secs(2));
    let notified = timer.until_or_signal(Instant::now() + Duration::from_millis(20), &consumer);
    let current = latest.current();
    // Release and join before assertions, including on the old behavior's failure.
    let _ = release_tx.send(());
    let published = publisher.join().expect("publication thread panicked");
    releasing.expect("previous image release did not begin");
    published?;
    assert_eq!(current?.unwrap().value, 2);
    assert!(
        notified?,
        "a new image's notification waited for the previous image's resource release"
    );
    Ok(())
}
#[test]
fn capture_reset_notifies_consumers_before_releasing_the_latest_resources() -> Result<()> {
    struct Image {
        latest: Weak<Latest<Image>>,
        consumer: Arc<Consumer>,
        released: Arc<AtomicBool>,
    }
    impl Drop for Image {
        fn drop(&mut self) {
            let latest = self.latest.upgrade().unwrap();
            let state = latest.state.try_lock().expect("reset held the state lock");
            assert!(state.image.is_none());
            assert_ne!(
                state.generation,
                self.consumer.released.load(Ordering::Acquire)
            );
            drop(state);
            let timer = Timer::new().unwrap();
            assert!(
                timer
                    .until_or_signal(Instant::now() + Duration::from_millis(20), &self.consumer,)
                    .unwrap()
            );
            latest.release_generation(&self.consumer);
            self.released.store(true, Ordering::Release);
        }
    }
    let latest = Arc::new(Latest::new());
    let consumer = latest.subscribe(Weak::new())?;
    let released = Arc::new(AtomicBool::new(false));
    latest.publish_captured(
        Arc::new(Image {
            latest: Arc::downgrade(&latest),
            consumer: consumer.clone(),
            released: released.clone(),
        }),
        Instant::now(),
    )?;
    consumer.reset()?;
    latest.begin_recovery()?;
    assert!(released.load(Ordering::Acquire));
    assert!(latest.consumers_released());
    Ok(())
}
#[test]
fn publications_before_wait_and_independent_consumers_keep_the_latest_frame() -> Result<()> {
    let latest = Latest::new();
    let timer = Timer::new()?;
    let first = latest.subscribe(Weak::new())?;
    let second = latest.subscribe(Weak::new())?;
    let old = Arc::new(1u8);
    latest.publish_captured(old.clone(), Instant::now())?;
    latest.wait_if_current_or(
        &timer,
        &first,
        &old,
        Instant::now() + Duration::from_millis(2),
        None,
    )?;
    assert!(timer.until_or_signal(Instant::now() + Duration::from_secs(1), &second)?);
    let new = Arc::new(2u8);
    latest.publish_captured(new.clone(), Instant::now())?;
    latest.wait_if_current_or(
        &timer,
        &first,
        &old,
        Instant::now() + Duration::from_secs(1),
        None,
    )?;
    assert!(Arc::ptr_eq(
        &latest
            .wait_for_frame(&timer, &first, Instant::now())?
            .unwrap(),
        &new
    ));
    latest.begin_recovery()?;
    assert!(
        latest
            .wait_for_frame(&timer, &first, Instant::now() + Duration::from_millis(2))?
            .is_none()
    );
    Ok(())
}
#[test]
fn an_encoder_output_event_ends_the_wait_for_a_new_picture() -> Result<()> {
    let latest = Latest::new();
    let timer = Timer::new()?;
    let wake = latest.subscribe(Weak::new())?;
    let image = Arc::new(1u8);
    latest.publish_captured(image.clone(), Instant::now())?;
    // The wait resets the capture signal: only the output event ends it.
    let output = Signal::new()?;
    output.set()?;
    let started = Instant::now();
    latest.wait_if_current_or(
        &timer,
        &wake,
        &image,
        started + Duration::from_secs(2),
        Some(&output),
    )?;
    assert!(started.elapsed() < Duration::from_secs(1));
    Ok(())
}
#[test]
fn terminal_capture_failure_wakes_waiters_and_cannot_return_a_stale_frame() -> Result<()> {
    let latest = Arc::new(Latest::<u8>::new());
    let timer = Timer::new()?;
    let wake = latest.subscribe(Weak::new())?;
    let worker = latest.clone();
    let failed = std::thread::spawn(move || worker.fail("device lost".into()).unwrap());
    let error = latest
        .wait_for_frame(&timer, &wake, Instant::now() + Duration::from_secs(1))
        .unwrap_err();
    failed.join().unwrap();
    assert!(error.to_string().contains("device lost"));
    assert!(latest.current().is_err());
    assert!(
        latest
            .publish_captured(Arc::new(3), Instant::now())
            .is_err()
    );
    let late = latest.subscribe(Weak::new())?;
    assert!(
        latest
            .wait_for_frame(&timer, &late, Instant::now())
            .is_err()
    );
    Ok(())
}

#[test]
fn recovery_waits_for_all_owners_and_departing_consumers_release_their_lease() -> Result<()> {
    let latest = Latest::new();
    let first = latest.subscribe(Weak::new())?;
    let second = latest.subscribe(Weak::new())?;
    latest.publish_captured(Arc::new(1u8), Instant::now())?;
    let held = latest.current()?.unwrap();
    let old = Arc::downgrade(&held);
    latest.begin_recovery()?;
    assert!(latest.current()?.is_none());
    assert!(!latest.consumers_released());
    drop(held);
    latest.release_generation(&first);
    assert!(old.upgrade().is_none());
    assert!(!latest.consumers_released());
    // A stream joining during recovery has no old resources to release.
    let joining = latest.subscribe(Weak::new())?;
    drop(second);
    assert!(latest.consumers_released());
    latest.publish_captured(Arc::new(2), Instant::now())?;
    latest.begin_recovery()?;
    assert!(!latest.consumers_released());
    latest.release_generation(&first);
    assert!(!latest.consumers_released());
    latest.release_generation(&joining);
    assert!(latest.consumers_released());
    Ok(())
}

#[test]
fn reset_wakes_each_waiter_and_skips_waiting_until_resources_are_released() -> Result<()> {
    let latest = Arc::new(Latest::<u8>::new());
    let consumer = latest.subscribe(Weak::new())?;
    let worker = latest.clone();
    let timer = Timer::new()?;
    let reset = std::thread::spawn(move || worker.begin_recovery().unwrap());
    let start = Instant::now();
    assert!(
        latest
            .wait_for_frame(&timer, &consumer, start + Duration::from_secs(2))?
            .is_none()
    );
    reset.join().unwrap();
    // An unacknowledged reset must also bypass waits started after notification.
    assert!(
        latest
            .wait_for_frame(&timer, &consumer, start + Duration::from_secs(2))?
            .is_none()
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!latest.consumers_released());
    latest.release_generation(&consumer);
    assert!(latest.consumers_released());
    let frame = Arc::new(2);
    latest.publish_captured(frame.clone(), Instant::now())?;
    assert!(Arc::ptr_eq(
        &frame,
        &latest
            .wait_for_frame(&timer, &consumer, Instant::now())?
            .unwrap()
    ));
    Ok(())
}
