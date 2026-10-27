//! USB forwarding during a stream (`RustBridge.usb*`): Java hands over a device the user
//! picked and allowed, as its usbfs descriptor; the stream's [`Forwarder`] asks the host and
//! serves it there. The forwarder lives as long as the stream: the first attach makes it,
//! [`end`] (the stream stopping) detaches everything. Java hears of every change through
//! `MoonBridge.bridgeClUsbChanged` and reads [`list_json`].

use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};

use nightjar_core::control::SendError;
use nightjar_core::usb::{AttachError, Attachment, Forwarder, Phase, UsbDevice, UsbMessage};
use nightjar_core::usb::{UsbReason, UsbState};

use crate::platform;

/// `RustBridge.USB_*`: what `usbAttach` returns instead of a request id.
pub const NO_STREAM: i32 = -1;
pub const BUSY: i32 = -2;
pub const UNSUPPORTED: i32 = -3;
pub const DEVICE_FAILED: i32 = -4;
pub const INVALID: i32 = -5;
pub const HOST_FAILED: i32 = -6;

static FORWARDER: Mutex<Option<Arc<Forwarder>>> = Mutex::new(None);

fn lock(slot: &Mutex<Option<Arc<Forwarder>>>) -> MutexGuard<'_, Option<Arc<Forwarder>>> {
    slot.lock().unwrap_or_else(|p| p.into_inner())
}

/// The stream's forwarder, made by `make` on first use, or the code to return without one.
fn forwarder(
    slot: &Mutex<Option<Arc<Forwarder>>>,
    streaming: bool,
    make: impl FnOnce() -> Result<Forwarder, String>,
) -> Result<Arc<Forwarder>, i32> {
    let mut slot = lock(slot);
    if !streaming {
        return Err(NO_STREAM);
    }
    if let Some(forwarder) = slot.as_ref() {
        return Ok(forwarder.clone());
    }
    match make() {
        Ok(forwarder) => Ok(slot.insert(Arc::new(forwarder)).clone()),
        Err(e) => {
            platform::error(&format!("USB: no client for the host: {e}"));
            Err(HOST_FAILED)
        }
    }
}

/// Attaches `device`: its request id, or a code. The device is closed when it fails.
fn attach_in(
    slot: &Mutex<Option<Arc<Forwarder>>>,
    streaming: bool,
    make: impl FnOnce() -> Result<Forwarder, String>,
    device: Arc<dyn UsbDevice>,
    name: &str,
) -> i32 {
    // Out of the slot's lock: attaching sends and tells Java, which lists.
    let forwarder = match forwarder(slot, streaming, make) {
        Ok(forwarder) => forwarder,
        Err(code) => {
            device.close();
            return code;
        }
    };
    match forwarder.attach(device.clone(), name) {
        Ok(request_id) => {
            platform::info(&format!(
                "USB: asked the host to attach {name} ({request_id})"
            ));
            request_id as i32
        }
        Err(e) => {
            device.close();
            platform::error(&format!("USB: {name} did not attach: {e}"));
            match e {
                AttachError::Ended | AttachError::Send(SendError::Stopped) => NO_STREAM,
                AttachError::Busy => BUSY,
                AttachError::Send(SendError::Unsupported) => UNSUPPORTED,
                AttachError::Invalid | AttachError::Send(_) => INVALID,
            }
        }
    }
}

fn current(slot: &Mutex<Option<Arc<Forwarder>>>) -> Option<Arc<Forwarder>> {
    lock(slot).clone()
}

/// Attaches `device` to the running stream's host; `make` builds the forwarder the first time.
pub fn attach(
    make: impl FnOnce() -> Result<Forwarder, String>,
    device: Arc<dyn UsbDevice>,
    name: &str,
) -> i32 {
    let streaming = crate::engine::with_session(false, |_| true);
    attach_in(&FORWARDER, streaming, make, device, name)
}

/// The user detached it or unplugged it; false when it is not attached.
pub fn detach(request_id: u32) -> bool {
    current(&FORWARDER).is_some_and(|f| f.detach(request_id))
}

/// A USB message from the host, from the control stream.
pub fn on_message(message: UsbMessage) {
    match current(&FORWARDER) {
        Some(forwarder) => forwarder.on_message(message),
        // Nothing attached in this stream: the host has nothing to say about it.
        None => platform::warn(&format!(
            "USB: a message with nothing attached: {message:?}"
        )),
    }
}

/// The stream stopped: everything attached detaches.
pub fn end() {
    end_in(&FORWARDER);
}

fn end_in(slot: &Mutex<Option<Arc<Forwarder>>>) {
    let forwarder = lock(slot).take();
    if let Some(forwarder) = forwarder {
        forwarder.end();
    }
}

/// Every attachment of this stream, as `UsbForwarding.Attachment.parseList` reads it.
pub fn list_json() -> String {
    let attachments = current(&FORWARDER).map(|f| f.list()).unwrap_or_default();
    to_json(&attachments).to_string()
}

fn state_name(state: UsbState) -> &'static str {
    match state {
        UsbState::Attached => "attached",
        UsbState::Detached => "detached",
        UsbState::Refused => "refused",
        UsbState::Failed => "failed",
    }
}

fn reason_name(reason: UsbReason) -> &'static str {
    match reason {
        UsbReason::None => "none",
        UsbReason::Permission => "permission",
        UsbReason::Disabled => "disabled",
        UsbReason::Limit => "limit",
        UsbReason::Driver => "driver",
        UsbReason::TunnelTimeout => "tunnel_timeout",
        UsbReason::DeviceError => "device_error",
        UsbReason::SessionEnd => "session_end",
    }
}

fn to_json(attachments: &[Attachment]) -> Value {
    let list = attachments
        .iter()
        .map(|a| {
            let mut entry = json!({
                "requestId": a.request_id,
                "busid": a.busid,
                "name": a.name,
                "vid": a.vid,
                "pid": a.pid,
            });
            let phase = match &a.phase {
                Phase::Asking => "asking",
                Phase::Opening => "opening",
                Phase::Attached => "attached",
                Phase::Ended {
                    state,
                    reason,
                    error,
                } => {
                    entry["state"] = json!(state_name(*state));
                    entry["reason"] = json!(reason_name(*reason));
                    if let Some(error) = error {
                        entry["error"] = json!(error);
                    }
                    "ended"
                }
            };
            entry["phase"] = json!(phase);
            entry
        })
        .collect();
    Value::Array(list)
}

/// Opens the device Java's `UsbDeviceConnection` has open at `path`; `fd` stays Java's.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn open(fd: i32, path: &str) -> Result<Arc<dyn UsbDevice>, String> {
    use nightjar_core::usb::usbfs::{linux::Usbfs, UsbfsDevice};
    let kernel = Usbfs::from_fd(fd).map_err(|e| format!("its descriptor did not copy: {e}"))?;
    Ok(Arc::new(UsbfsDevice::open(kernel, path)?))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn open(_: i32, _: &str) -> Result<Arc<dyn UsbDevice>, String> {
    Err("usbfs is Linux's".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::control::ClientMessage;
    use nightjar_core::usb::fake::FakeDevice;
    use nightjar_core::usb::Blocking;

    fn sent_to(log: Arc<Mutex<Vec<UsbMessage>>>, refuse: Option<SendError>) -> Forwarder {
        Forwarder::new(
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
            |_| Err("no tunnel in this test".into()),
            || {},
        )
    }

    fn device(busid: &str) -> (FakeDevice, Arc<dyn UsbDevice>) {
        let fake = FakeDevice::new(busid);
        (fake.clone(), Arc::new(Blocking::new(fake)))
    }

    #[test]
    fn without_a_stream_nothing_attaches_and_the_device_closes() {
        let slot = Mutex::new(None);
        let (fake, device) = device("1-5");
        let made = std::cell::Cell::new(false);
        let code = attach_in(
            &slot,
            false,
            || {
                made.set(true);
                Err("unused".into())
            },
            device,
            "Pad",
        );
        assert_eq!(code, NO_STREAM);
        assert!(!made.get());
        assert_eq!(fake.closes(), 1);
        assert!(lock(&slot).is_none());
    }

    #[test]
    fn attaching_asks_the_host_once_per_device_on_one_forwarder() {
        let slot = Mutex::new(None);
        let log = Arc::new(Mutex::new(Vec::new()));
        let (_, first) = device("1-5");
        let id = attach_in(&slot, true, || Ok(sent_to(log.clone(), None)), first, "Pad");
        assert_eq!(id, 1);
        assert!(matches!(
            log.lock().unwrap()[..],
            [UsbMessage::AttachRequest { request_id: 1, ref busid, .. }] if busid == "1-5"
        ));
        // The same device again is busy, and a second forwarder is never made.
        let (fake, again) = device("1-5");
        let code = attach_in(&slot, true, || panic!("made twice"), again, "Pad");
        assert_eq!((code, fake.closes()), (BUSY, 1));
        let (_, other) = device("1-6");
        assert_eq!(
            attach_in(&slot, true, || panic!("made twice"), other, "Key"),
            2
        );

        let listed = to_json(&lock(&slot).as_ref().unwrap().list());
        assert_eq!(listed[0]["requestId"], 1);
        assert_eq!(listed[0]["busid"], "1-5");
        assert_eq!(listed[0]["phase"], "asking");
        assert_eq!(listed[1]["name"], "Key");

        // The stream ends: both end, and the next stream makes its own.
        let forwarder = lock(&slot).clone().unwrap();
        end_in(&slot);
        assert!(lock(&slot).is_none());
        let ended = to_json(&forwarder.list());
        assert_eq!(ended[0]["phase"], "ended");
        assert_eq!(ended[0]["state"], "detached");
        assert_eq!(ended[0]["reason"], "session_end");
        assert!(ended[0].get("error").is_none());
    }

    #[test]
    fn a_host_without_usb_forwarding_says_so_and_the_device_closes() {
        let slot = Mutex::new(None);
        let log = Arc::new(Mutex::new(Vec::new()));
        let (fake, device) = device("1-5");
        let make = || Ok(sent_to(log.clone(), Some(SendError::Unsupported)));
        assert_eq!(attach_in(&slot, true, make, device, "Pad"), UNSUPPORTED);
        assert_eq!(fake.closes(), 1);
        assert!(lock(&slot).as_ref().unwrap().list().is_empty());

        let slot = Mutex::new(None);
        let (fake, device) = self::device("2-1");
        assert_eq!(
            attach_in(&slot, true, || Err("bad certificate".into()), device, "Pad"),
            HOST_FAILED
        );
        assert_eq!(fake.closes(), 1);
        assert!(lock(&slot).is_none());
    }

    #[test]
    fn a_refused_attachment_lists_its_reason() {
        let slot = Mutex::new(None);
        let log = Arc::new(Mutex::new(Vec::new()));
        let (fake, device) = device("1-5");
        let id = attach_in(
            &slot,
            true,
            || Ok(sent_to(log.clone(), None)),
            device,
            "Pad",
        );
        let forwarder = lock(&slot).clone().unwrap();
        forwarder.on_message(UsbMessage::Status {
            request_id: id as u32,
            state: UsbState::Refused,
            reason: UsbReason::Permission,
        });
        let listed = to_json(&forwarder.list());
        assert_eq!(listed[0]["phase"], "ended");
        assert_eq!(listed[0]["state"], "refused");
        assert_eq!(listed[0]["reason"], "permission");
        assert_eq!(fake.closes(), 1);
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    #[test]
    fn off_linux_a_descriptor_does_not_open() {
        assert!(open(3, "/dev/bus/usb/001/005").is_err());
    }
}
