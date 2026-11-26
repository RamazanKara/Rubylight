//! The endpoint listener against a fake notification source: every callback
//! wakes, nothing stays registered or referenced after the watch drops.

use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use windows::core::{Interface, w};

#[derive(Default)]
struct Fake {
    held: Mutex<Option<IMMNotificationClient>>,
    registers: AtomicUsize,
    unregisters: AtomicUsize,
    refuse: bool,
}

struct Source(Arc<Fake>);

impl EndpointEvents for Source {
    fn register(&self, client: &IMMNotificationClient) -> Result<()> {
        self.0.registers.fetch_add(1, Ordering::SeqCst);
        if self.0.refuse {
            anyhow::bail!("refused");
        }
        *self.0.held.lock().unwrap() = Some(client.clone());
        Ok(())
    }
    fn unregister(&self, client: &IMMNotificationClient) -> Result<()> {
        self.0.unregisters.fetch_add(1, Ordering::SeqCst);
        let held = self.0.held.lock().unwrap().take();
        assert_eq!(
            held.as_ref().map(Interface::as_raw),
            Some(client.as_raw()),
            "unregisters the client it registered"
        );
        Ok(())
    }
}

fn counter() -> (Arc<AtomicUsize>, Wake) {
    let count = Arc::new(AtomicUsize::new(0));
    let wake: Wake = Arc::new({
        let count = count.clone();
        move || {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    (count, wake)
}

#[test]
fn every_endpoint_callback_wakes_the_upkeep() {
    let fake = Arc::new(Fake::default());
    let (count, wake) = counter();
    let watch = EndpointWatch::register(Box::new(Source(fake.clone())), wake).unwrap();
    let client = fake.held.lock().unwrap().clone().unwrap();
    let id = w!("{0.0.0.00000000}.{fake}");
    unsafe {
        client
            .OnDefaultDeviceChanged(eRender, eConsole, id)
            .unwrap();
        client
            .OnDeviceStateChanged(id, DEVICE_STATE_UNPLUGGED)
            .unwrap();
        client.OnDeviceAdded(id).unwrap();
        client.OnDeviceRemoved(id).unwrap();
        client
            .OnPropertyValueChanged(id, PROPERTYKEY::default())
            .unwrap();
    }
    assert_eq!(count.load(Ordering::SeqCst), 5);
    drop(client);
    drop(watch);
}

#[test]
fn dropping_the_watch_unregisters_and_releases_the_listener() {
    let fake = Arc::new(Fake::default());
    let (count, wake) = counter();
    let watch = EndpointWatch::register(Box::new(Source(fake.clone())), wake.clone()).unwrap();
    assert_eq!(fake.registers.load(Ordering::SeqCst), 1);
    assert!(Arc::strong_count(&wake) > 1, "the listener holds the wake");
    drop(watch);
    assert_eq!(fake.unregisters.load(Ordering::SeqCst), 1);
    assert!(fake.held.lock().unwrap().is_none());
    // The last COM reference is gone, so the listener and its wake closure
    // were freed: no leaked reference.
    assert_eq!(Arc::strong_count(&wake), 1);
    assert_eq!(count.load(Ordering::SeqCst), 0, "no wake without an event");
}

#[test]
fn a_refused_registration_leaves_nothing_behind() {
    let fake = Arc::new(Fake {
        refuse: true,
        ..Fake::default()
    });
    let (_, wake) = counter();
    let result = EndpointWatch::register(Box::new(Source(fake.clone())), wake.clone());
    assert!(result.is_err());
    assert_eq!(fake.unregisters.load(Ordering::SeqCst), 0);
    assert_eq!(Arc::strong_count(&wake), 1);
}

#[test]
fn a_wake_ends_the_wait_before_the_poll() {
    let waiter = thread::spawn(|| {
        let started = Instant::now();
        park_until_round(started.checked_sub(MIN_GAP).unwrap_or(started));
        started.elapsed()
    });
    let wake: Wake = Arc::new({
        let thread = waiter.thread().clone();
        move || thread.unpark()
    });
    thread::sleep(Duration::from_millis(20));
    wake();
    let waited = waiter.join().unwrap();
    assert!(waited < POLL, "woke after {waited:?}");
}

#[test]
fn rounds_keep_the_minimum_gap() {
    let last_round = Instant::now();
    thread::current().unpark();
    park_until_round(last_round);
    assert!(last_round.elapsed() >= MIN_GAP);
}
