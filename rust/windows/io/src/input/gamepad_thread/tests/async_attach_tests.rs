//! Regression tests for async USB/IP pad attachment audio replay and deferred motion requests.

use super::*;

type WakeSlot = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;
type DeliveredAudio = Arc<Mutex<Vec<(u16, u16, Vec<u8>)>>>;

#[test]
fn controller_audio_status_replays_when_usb_audio_becomes_ready_after_poll() {
    struct DelayedAudioPad {
        plugged: bool,
        audio_ready: u16,
        attach_pending: Arc<AtomicBool>,
        wake: WakeSlot,
        delivered: DeliveredAudio,
        dropped: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Pads for DelayedAudioPad {
        fn apply(&mut self, event: &Event) -> Result<()> {
            if matches!(event, Event::Arrival { .. }) {
                self.plugged = true;
            }
            Ok(())
        }
        fn refresh(&mut self) -> Result<()> {
            if self.attach_pending.swap(false, Ordering::AcqRel) {
                self.audio_ready |= 1;
            }
            Ok(())
        }
        fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
            vec![]
        }
        fn audio(&mut self, id: u16, kind: u16, payload: &[u8]) {
            if self.audio_ready & (1 << id) != 0 {
                self.delivered
                    .lock()
                    .unwrap()
                    .push((id, kind, payload.to_vec()));
            } else {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn audio_ready(&self) -> u16 {
            self.audio_ready
        }
        fn motion_supported(&self, _: u16) -> bool {
            false
        }
        fn plugged(&self) -> bool {
            self.plugged
        }
        fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
            *self.wake.lock().unwrap() = Some(wake);
        }
        fn attach_ready(&self) -> bool {
            self.attach_pending.load(Ordering::Acquire)
        }
        fn release_all(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let attach_pending = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Mutex::new(None));
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pads = GamepadThread::spawn_reported(
        {
            let (attach_pending, wake, delivered, dropped) = (
                attach_pending.clone(),
                wake.clone(),
                delivered.clone(),
                dropped.clone(),
            );
            move || {
                Ok(DelayedAudioPad {
                    plugged: false,
                    audio_ready: 0,
                    attach_pending: attach_pending.clone(),
                    wake: wake.clone(),
                    delivered: delivered.clone(),
                    dropped: dropped.clone(),
                })
            }
        },
        Default::default(),
    )
    .unwrap();

    // Arrival plugs a temporary VHF pad (`plugged() == true`) before USB/IP audio is ready.
    pads.send(Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: haptics::CAPABILITY,
        buttons: 0,
    });
    assert!(pads.wait_ready(1, Duration::from_secs(2)));
    assert!(!pads.controller_audio_ready(0));

    // Client sends 0x5535 status while USB/IP is still enumerating; the 8 ms poll drops it on VHF.
    let status = [1, 1, 0, 0, 7, 0, 0, 0];
    pads.audio(0, controller_audio::CONTROL, &status);
    Gate::default().wait_until(|| dropped.load(Ordering::SeqCst) >= 1);
    assert!(delivered.lock().unwrap().is_empty());

    // Periodic identical status is deduplicated by AudioQueue::push.
    pads.audio(0, controller_audio::CONTROL, &status);

    // USB/IP DualSense finishes attaching and wakes the worker: status must replay immediately.
    attach_pending.store(true, Ordering::Release);
    if let Some(cb) = wake.lock().unwrap().as_ref() {
        cb();
    }
    Gate::default()
        .wait_until(|| pads.controller_audio_ready(0) && !delivered.lock().unwrap().is_empty());
    assert_eq!(
        *delivered.lock().unwrap(),
        vec![(0, controller_audio::CONTROL, status.to_vec())]
    );
}

#[test]
fn motion_is_requested_after_async_attach_completes() {
    struct DelayedMotionPad {
        attached: bool,
        attach_pending: Arc<AtomicBool>,
        wake: WakeSlot,
        pending_motion: Vec<(u8, u16)>,
    }
    impl Pads for DelayedMotionPad {
        fn apply(&mut self, _: &Event) -> Result<()> {
            Ok(())
        }
        fn refresh(&mut self) -> Result<()> {
            if self.attach_pending.swap(false, Ordering::AcqRel) {
                self.attached = true;
                self.pending_motion.push((0, 0x30));
            }
            Ok(())
        }
        fn motion_requests(&mut self) -> Vec<(u8, u16)> {
            std::mem::take(&mut self.pending_motion)
        }
        fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
            vec![]
        }
        fn motion_supported(&self, _: u16) -> bool {
            self.attached
        }
        fn plugged(&self) -> bool {
            self.attached
        }
        fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
            *self.wake.lock().unwrap() = Some(wake);
        }
        fn attach_ready(&self) -> bool {
            self.attach_pending.load(Ordering::Acquire)
        }
        fn release_all(&mut self) -> Result<()> {
            Ok(())
        }
    }

    let attach_pending = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Mutex::new(None));
    let pads = GamepadThread::spawn_reported(
        {
            let (attach_pending, wake) = (attach_pending.clone(), wake.clone());
            move || {
                Ok(DelayedMotionPad {
                    attached: false,
                    attach_pending: attach_pending.clone(),
                    wake: wake.clone(),
                    pending_motion: Vec::new(),
                })
            }
        },
        Default::default(),
    )
    .unwrap();

    // Arrival before USB/IP finishes has no motion-capable pad yet.
    pads.send(Event::Arrival {
        id: 0,
        kind: 3,
        capabilities: 0x30,
        buttons: 0,
    });
    assert!(pads.wait_ready(1, Duration::from_secs(2)));
    assert!(pads.reports().next().is_none());

    // Once background USB/IP attachment finishes, PadReport::Motion is emitted.
    attach_pending.store(true, Ordering::Release);
    if let Some(cb) = wake.lock().unwrap().as_ref() {
        cb();
    }
    let mut got = None;
    Gate::default().wait_until(|| {
        for report in pads.reports() {
            if matches!(report, PadReport::Motion { .. }) {
                got = Some(report);
                return true;
            }
        }
        false
    });
    assert_eq!(
        got,
        Some(PadReport::Motion {
            id: 0,
            capabilities: 0x30
        })
    );
}
