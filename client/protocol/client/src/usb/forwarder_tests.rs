use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rubylight_protocol::usbip as wire;

use super::*;
use crate::usb::fake::{self, FakeDevice};
use crate::usb::server::tests::{pipe, Driver};
use crate::usb::Blocking;

const TOKEN: [u8; TOKEN_BYTES] = [7; TOKEN_BYTES];

/// A forwarder whose messages, tunnels and changes the test sees.
struct Rig {
    forwarder: Forwarder,
    sent: Arc<Mutex<Vec<UsbMessage>>>,
    drivers: Receiver<Driver>,
    changes: Receiver<()>,
}

fn rig(refuse: Option<SendError>, open_fails: bool) -> Rig {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let (to_test, drivers): (Sender<Driver>, _) = mpsc::channel();
    let (changed, changes) = mpsc::channel();
    let to_test = Mutex::new(to_test);
    let changed = Mutex::new(changed);
    let log = sent.clone();
    let forwarder = Forwarder::new(
        move |message| {
            if let Some(e) = refuse {
                return Err(e);
            }
            match message {
                ClientMessage::Usb(message) => log.lock().unwrap().push(message),
                other => panic!("sent {other:?}"),
            }
            Ok(())
        },
        move |token| {
            assert_eq!(*token, TOKEN);
            if open_fails {
                return Err("403 usb_disabled".into());
            }
            let (tunnel, driver) = pipe();
            to_test.lock().unwrap().send(driver).unwrap();
            Ok(tunnel as Arc<dyn Tunnel>)
        },
        move || {
            let _ = changed.lock().unwrap().send(());
        },
    );
    Rig {
        forwarder,
        sent,
        drivers,
        changes,
    }
}

impl Rig {
    fn sent(&self) -> Vec<UsbMessage> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }

    fn phase(&self, request_id: u32) -> Phase {
        self.forwarder
            .list()
            .into_iter()
            .find(|a| a.request_id == request_id)
            .unwrap()
            .phase
    }

    /// Waits until `request_id` is in `want`.
    fn wait_for(&self, request_id: u32, want: impl Fn(&Phase) -> bool) -> Phase {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let phase = self.phase(request_id);
            if want(&phase) {
                return phase;
            }
            assert!(Instant::now() < deadline, "still {phase:?}");
            let _ = self.changes.recv_timeout(Duration::from_millis(100));
        }
    }

    /// Attaches `device` up to the host's driver importing it.
    fn attach(&self, device: &FakeDevice) -> (u32, Driver) {
        let busid = device.describe_busid();
        let id = self
            .forwarder
            .attach(Arc::new(Blocking::new(device.clone())), "Fake")
            .unwrap();
        self.forwarder.on_message(UsbMessage::TunnelReady {
            request_id: id,
            token: TOKEN,
        });
        let mut driver = self.drivers.recv_timeout(Duration::from_secs(10)).unwrap();
        driver.import(&busid).unwrap();
        self.forwarder.on_message(UsbMessage::Status {
            request_id: id,
            state: UsbState::Attached,
            reason: UsbReason::None,
        });
        assert_eq!(self.phase(id), Phase::Attached);
        (id, driver)
    }
}

impl FakeDevice {
    fn describe_busid(&self) -> String {
        crate::usb::BlockingDevice::describe(self).busid
    }
}

fn wait_closed(device: &FakeDevice) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while device.closes() == 0 {
        assert!(Instant::now() < deadline, "the device stays open");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_attach_asks_once_per_device_and_follows_the_host() {
    let rig = rig(None, false);
    let device = FakeDevice::new("1-4");
    let (id, mut driver) = rig.attach(&device);
    assert_eq!(
        rig.sent(),
        [UsbMessage::AttachRequest {
            request_id: id,
            vid: fake::VENDOR,
            pid: fake::PRODUCT,
            class: 0xff,
            speed: 3,
            busid: "1-4".into(),
            name: "Fake".into(),
        }]
    );
    assert_eq!(
        rig.forwarder
            .attach(Arc::new(Blocking::new(device.clone())), "Again"),
        Err(AttachError::Busy)
    );
    // Served: a transfer goes through.
    let seqnum = driver.submit(0, wire::DIR_IN, 18, crate::usb::server::tests::setup(0x80, 6, 0x0100, 18), &[], &[]);
    let (_, data, _) = driver.reply(true);
    assert_eq!((seqnum, data.as_slice()), (1, &fake::DEVICE_DESCRIPTOR[..]));

    // The host detaches it at the end of its session: the tunnel closes and the device too.
    rig.forwarder.on_message(UsbMessage::Detach { request_id: id });
    rig.forwarder.on_message(UsbMessage::Status {
        request_id: id,
        state: UsbState::Detached,
        reason: UsbReason::SessionEnd,
    });
    assert_eq!(
        rig.phase(id),
        Phase::Ended {
            state: UsbState::Detached,
            reason: UsbReason::SessionEnd,
            error: None
        }
    );
    assert!(driver.closed());
    wait_closed(&device);
    assert!(rig.sent().is_empty(), "the host's detach needs no answer");
    // And it can attach again.
    assert!(rig
        .forwarder
        .attach(Arc::new(Blocking::new(device.clone())), "Fake")
        .is_ok());
}

#[test]
fn the_user_detaching_tells_the_host_and_lets_go() {
    let rig = rig(None, false);
    let device = FakeDevice::new("2-1");
    let (id, mut driver) = rig.attach(&device);
    rig.sent();
    // An interrupt IN waits in the device when the user detaches.
    driver.submit(fake::INTERRUPT as u32, wire::DIR_IN, 8, [0; 8], &[], &[]);
    assert!(rig.forwarder.detach(id));
    assert_eq!(rig.sent(), [UsbMessage::Detach { request_id: id }]);
    assert!(driver.closed());
    wait_closed(&device);
    assert!(!rig.forwarder.detach(id), "once");
    assert!(!rig.forwarder.list()[0].is_active());
}

#[test]
fn the_host_closing_the_tunnel_ends_the_attachment() {
    let rig = rig(None, false);
    let device = FakeDevice::new("2-1");
    let (id, driver) = rig.attach(&device);
    rig.sent();
    drop(driver);
    rig.wait_for(id, |phase| !matches!(phase, Phase::Attached));
    wait_closed(&device);
    assert_eq!(rig.sent(), [UsbMessage::Detach { request_id: id }]);
}

#[test]
fn a_tunnel_that_does_not_open_fails_and_tells_the_host() {
    let rig = rig(None, true);
    let device = FakeDevice::new("1-1");
    let id = rig
        .forwarder
        .attach(Arc::new(Blocking::new(device.clone())), "Fake")
        .unwrap();
    rig.forwarder.on_message(UsbMessage::TunnelReady {
        request_id: id,
        token: TOKEN,
    });
    let phase = rig.wait_for(id, |phase| matches!(phase, Phase::Ended { .. }));
    let Phase::Ended {
        state: UsbState::Failed,
        error: Some(error),
        ..
    } = phase
    else {
        panic!("{phase:?}");
    };
    assert!(error.contains("usb_disabled"), "{error}");
    wait_closed(&device);
    assert_eq!(rig.sent().last(), Some(&UsbMessage::Detach { request_id: id }));
}

#[test]
fn refusals_end_the_attachment_with_the_hosts_reason() {
    let rig = rig(None, false);
    let device = FakeDevice::new("1-1");
    let id = rig
        .forwarder
        .attach(Arc::new(Blocking::new(device.clone())), "Fake")
        .unwrap();
    rig.forwarder.on_message(UsbMessage::Status {
        request_id: id,
        state: UsbState::Refused,
        reason: UsbReason::Permission,
    });
    assert_eq!(
        rig.phase(id),
        Phase::Ended {
            state: UsbState::Refused,
            reason: UsbReason::Permission,
            error: None
        }
    );
    assert_eq!(device.closes(), 1);
    // A late token for it opens nothing.
    rig.forwarder.on_message(UsbMessage::TunnelReady {
        request_id: id,
        token: TOKEN,
    });
    assert!(rig.drivers.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn a_host_that_does_not_forward_usb_leaves_nothing_behind() {
    let rig = rig(Some(SendError::Unsupported), false);
    let device = FakeDevice::new("1-1");
    assert_eq!(
        rig.forwarder
            .attach(Arc::new(Blocking::new(device)), "Fake"),
        Err(AttachError::Send(SendError::Unsupported))
    );
    assert!(rig.forwarder.list().is_empty());
}

#[test]
fn the_session_ending_detaches_everything_and_attaches_nothing_more() {
    let rig = rig(None, false);
    let (first, second) = (FakeDevice::new("1-1"), FakeDevice::new("1-2"));
    let (_, mut first_driver) = rig.attach(&first);
    let (_, mut second_driver) = rig.attach(&second);
    let asking = FakeDevice::new("1-3");
    rig.forwarder
        .attach(Arc::new(Blocking::new(asking.clone())), "Asking")
        .unwrap();
    rig.sent();
    rig.forwarder.end();
    for attachment in rig.forwarder.list() {
        assert_eq!(
            attachment.phase,
            Phase::Ended {
                state: UsbState::Detached,
                reason: UsbReason::SessionEnd,
                error: None
            }
        );
    }
    assert!(first_driver.closed() && second_driver.closed());
    for device in [&first, &second, &asking] {
        wait_closed(device);
    }
    assert!(rig.sent().is_empty(), "the stream is gone; the host detaches its side");
    assert_eq!(
        rig.forwarder
            .attach(Arc::new(Blocking::new(FakeDevice::new("1-5"))), "Late"),
        Err(AttachError::Ended)
    );
}

#[test]
fn dropping_the_forwarder_detaches_too() {
    let rig = rig(None, false);
    let device = FakeDevice::new("1-1");
    let (_, mut driver) = rig.attach(&device);
    drop(rig.forwarder);
    assert!(driver.closed());
    wait_closed(&device);
}
