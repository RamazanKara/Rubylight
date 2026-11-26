//! Unit tests for USB forwarding arbitration: claims and their release paths, the pause rule,
//! and the busid behind a sysfs path.

use std::sync::atomic::AtomicU32;

use super::*;

fn device(busid: &str, vid: u16, pid: u16) -> Forwarded {
    Forwarded { vid, pid, busid: busid.to_owned() }
}

fn pad(vid: u16, pid: u16, busid: Option<&str>) -> PadIdentity {
    PadIdentity { vid, pid, busid: busid.map(str::to_owned) }
}

#[test]
fn a_claim_is_released_once_by_release_or_drop() {
    let registry = Registry::new();
    let start = registry.generation();
    let claim = registry.claim(1, device("1-2", 0x054c, 0x0ce6));
    assert_eq!(registry.forwarded(), vec![device("1-2", 0x054c, 0x0ce6)]);
    assert_eq!(registry.generation(), start + 1);
    claim.release();
    assert!(registry.forwarded().is_empty());
    assert_eq!(registry.generation(), start + 2);
    claim.release();
    drop(claim);
    assert_eq!(registry.generation(), start + 2, "released once: no more changes");

    {
        let _dropped = registry.claim(1, device("1-3", 1, 2));
        assert_eq!(registry.forwarded().len(), 1);
    }
    assert!(registry.forwarded().is_empty(), "a dropped claim lets go");
}

#[test]
fn the_end_of_a_stream_releases_its_claims_and_no_other() {
    let registry = Registry::new();
    let (ours, theirs) = (registry.owner(), registry.owner());
    assert_ne!(ours, theirs);
    let a = registry.claim(ours, device("1-2", 1, 2));
    let _b = registry.claim(ours, device("1-3", 1, 2));
    let _c = registry.claim(theirs, device("2-1", 3, 4));
    registry.release_owner(ours);
    assert_eq!(registry.forwarded(), vec![device("2-1", 3, 4)]);
    let generation = registry.generation();
    a.release();
    registry.release_owner(ours);
    assert_eq!(registry.generation(), generation, "nothing left to release changes nothing");
}

/// A device that counts how often it was closed.
#[derive(Default)]
struct Counted {
    closes: AtomicU32,
}

impl UsbDevice for Counted {
    fn describe(&self) -> wire::Device {
        wire::Device { busid: "1-2".into(), ..Default::default() }
    }

    fn submit(&self, _: Transfer, done: Completion) {
        done.complete(nightjar_core::usb::Reply::failed(wire::EPROTO));
    }

    fn unlink(&self, _: u32) {}

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn closing_or_dropping_a_forwarded_device_releases_its_claim() {
    let registry = Registry::new();
    let counted = Arc::new(Counted::default());
    let device = Arbitrated::new(counted.clone(), registry.claim(1, self::device("1-2", 1, 2)));
    assert_eq!(device.describe().busid, "1-2");
    device.close();
    assert_eq!(counted.closes.load(Ordering::SeqCst), 1, "the device let go");
    assert!(registry.forwarded().is_empty(), "the close released the claim");
    // The forwarder keeps ended attachments' devices: the claim is gone all the same.
    drop(device);

    let held = Arbitrated::new(Arc::new(Counted::default()), registry.claim(1, self::device("1-3", 1, 2)));
    assert_eq!(registry.forwarded().len(), 1);
    drop(held);
    assert!(registry.forwarded().is_empty(), "a device dropped without a close releases its claim too");
}

#[test]
fn waiting_for_the_gamepads_ends_at_once_unwatched_and_at_the_limit_otherwise() {
    let registry = Registry::new();
    let claim = registry.claim(1, device("1-2", 1, 2));
    assert!(claim.wait_applied(Duration::from_secs(5)), "nobody watches: no wait");
    registry.applied(0);
    let started = Instant::now();
    assert!(!claim.wait_applied(Duration::from_millis(30)));
    assert!(started.elapsed() < Duration::from_secs(2));
    registry.applied(registry.generation());
    assert!(claim.wait_applied(Duration::from_millis(30)));
}

#[test]
fn a_pad_pauses_only_while_a_claim_matches_it() {
    let dualsense = (0x054c, 0x0ce6);
    // Known busid: exactly that port.
    let forwarded = [device("1-2", dualsense.0, dualsense.1)];
    let pads = [pad(dualsense.0, dualsense.1, Some("1-2")), pad(dualsense.0, dualsense.1, Some("1-3"))];
    assert_eq!(paused(&pads, &forwarded), vec![true, false]);
    assert_eq!(paused(&pads, &[]), vec![false, false], "no forwarding: nothing paused");
    assert_eq!(paused(&[pad(0x045e, 0x0b12, Some("1-2"))], &forwarded), vec![false], "another model on that port");

    // Unknown busid: only the one pad of the model, with one such device forwarded.
    assert_eq!(paused(&[pad(dualsense.0, dualsense.1, None)], &forwarded), vec![true]);
    let twins = [pad(dualsense.0, dualsense.1, None), pad(dualsense.0, dualsense.1, None)];
    assert_eq!(paused(&twins, &forwarded), vec![false, false], "two of a model: no guess");
    let two_forwarded = [device("1-2", dualsense.0, dualsense.1), device("1-3", dualsense.0, dualsense.1)];
    assert_eq!(paused(&[pad(dualsense.0, dualsense.1, None)], &two_forwarded), vec![false]);
    assert_eq!(paused(&[pad(0, 0, None)], &[device("1-2", 0, 0)]), vec![false], "no vendor or product: no guess");
    // A busid-known pad of the model accounts for the claim; the unknown one stays.
    let mixed = [pad(dualsense.0, dualsense.1, Some("1-2")), pad(dualsense.0, dualsense.1, None)];
    assert_eq!(paused(&mixed, &forwarded), vec![true, false]);
}

#[test]
fn the_plan_pauses_open_pads_and_resumes_paused_ones_when_their_claim_ends() {
    let forwarded = [device("1-2", 1, 2)];
    let open = [(10, pad(1, 2, Some("1-2"))), (11, pad(3, 4, Some("1-3")))];
    assert_eq!(plan(&open, &[], &forwarded), Plan { pause: vec![10], resume: vec![] });
    let paused_pads = [(10, pad(1, 2, Some("1-2")))];
    let still = plan(&open[1..], &paused_pads, &forwarded);
    assert_eq!(still, Plan { pause: vec![], resume: vec![] }, "still forwarded: stays paused");
    assert_eq!(plan(&open[1..], &paused_pads, &[]), Plan { pause: vec![], resume: vec![10] }, "forwarding stopped: reopened");
    assert_eq!(plan(&open[1..], &paused_pads, &[device("1-5", 1, 2)]).resume, vec![10], "another port's claim doesn't hold it");
}

#[test]
fn the_busid_is_the_step_before_the_first_interface() {
    let event = "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2/1-2:1.0/0003:054C:0CE6.0005/input/input20/event5";
    assert_eq!(busid_in_sysfs_path(event).as_deref(), Some("1-2"));
    let behind_hub = "/sys/devices/pci0000:00/0000:00:14.0/usb3/3-1/3-1.4/3-1.4:1.3/0003:054C:0CE6.0007/hidraw/hidraw3";
    assert_eq!(busid_in_sysfs_path(behind_hub).as_deref(), Some("3-1.4"));
    assert_eq!(busid_in_sysfs_path("/sys/devices/virtual/input/input9/event9"), None, "not USB");
    assert_eq!(busid_in_sysfs_path("/sys/devices/platform/i8042/serio0/input/input3/event3"), None);
    assert_eq!(busid_in_sysfs_path("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2"), None, "no interface below");
}
