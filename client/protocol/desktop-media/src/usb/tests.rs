//! Unit tests for USB forwarding device selection, libusb state transitions, sliced transfers, and labels.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use nightjar_core::usb::wire;
use rusb::constants::{LIBUSB_ERROR_NO_DEVICE, LIBUSB_ERROR_PIPE, LIBUSB_ERROR_TIMEOUT, LIBUSB_SUCCESS};

use super::libusb::*;
use super::*;

#[test]
fn busids_follow_linux() {
    assert_eq!(busid(1, &[2]), "1-2");
    assert_eq!(busid(3, &[1, 4, 2]), "3-1.4.2");
    assert!(wire::valid_busid(&busid(255, &[15, 15, 15, 15, 15, 15, 15])));
}

fn plugged(busid: &str, vid: u16, pid: u16, blocked: Option<Blocked>) -> Local {
    Local { busid: busid.into(), vid, pid, name: format!("{vid:04x}:{pid:04x}"), blocked, kernel: false }
}

#[test]
fn automatic_devices_attach_once_per_plug_in_on_any_port() {
    assert_eq!(auto_key(0x28de, 0x1304), "28de:1304");
    let auto = vec![auto_key(0x28de, 0x1304), auto_key(0x045e, 0x0719)];
    let puck = plugged("1-2", 0x28de, 0x1304, None);
    let receiver = plugged("1-3", 0x045e, 0x0719, Some(Blocked::NoWinUsb("xusb22".into())));
    let tablet = plugged("1-4", 0x056a, 0x0374, None);
    let mut tried = Vec::new();
    let local = [puck.clone(), receiver, tablet];
    // Only the listed one that can be forwarded; not the unlisted tablet or the blocked receiver.
    assert_eq!(auto_due(&local, &auto, |_| false, &mut tried), vec![puck]);
    // Not again while it stays plugged in, whatever became of it.
    assert!(auto_due(&local, &auto, |_| false, &mut tried).is_empty());
    // Unplugged, then plugged into another port: attached again.
    assert!(auto_due(&[], &auto, |_| false, &mut tried).is_empty());
    let moved = plugged("2-1", 0x28de, 0x1304, None);
    assert_eq!(auto_due(std::slice::from_ref(&moved), &auto, |_| false, &mut tried), std::slice::from_ref(&moved));
    // One the user attached already is left alone.
    let mut fresh = Vec::new();
    assert!(auto_due(&[moved], &auto, |busid| busid == "2-1", &mut fresh).is_empty());
}

/// The native answer with these features and the usb permission.
fn native(features: &str, usb: bool) -> NativeCapabilities {
    serde_json::from_str(&format!(
        "{{\"api_version\":1,\"host\":{{\"name\":\"PC\",\"version\":\"3.0.0\",\"unique_id\":\"uid\"}},\"features\":[{features}],\
         \"permissions\":{{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false,\"usb\":{usb}}}}}"
    ))
    .expect("the answer parses")
}

#[test]
fn the_stream_controls_offer_usb_devices_only_when_the_pc_does() {
    let info = |extra: &str| {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>\
             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>\
             <state>SUNSHINE_SERVER_FREE</state>{extra}</root>"
        );
        nightjar_policy::http::ServerInfo::parse(&xml, 47989).unwrap().capabilities
    };
    let caps = info("<RustHostVersion>3.0.0</RustHostVersion><NativeApiVersion>1</NativeApiVersion>");
    let offered = |native| refusal(availability(&caps, Some(native)));
    assert_eq!(offered(Ok(native("\"usb\"", true))), None);
    assert_eq!(
        offered(Ok(native("\"usb\"", false))),
        Some("This PC has not allowed USB devices for this device. On the PC, open the web UI’s Devices page and allow USB devices for it.")
    );
    // The permission alone is no offer: the host turned USB forwarding off.
    assert_eq!(offered(Ok(native("\"files.upload\"", true))), Some("The PC does not forward USB devices."));
    assert_eq!(offered(Err("timed out".into())), Some("Could not ask the PC whether it forwards USB devices."));
    // A host without the native API is not asked, and offers nothing.
    let old = info("<RustHostVersion>2.6.4</RustHostVersion>");
    assert_eq!(refusal(availability(&old, None)), Some("The PC does not forward USB devices."));
    assert!(!old.has(Feature::NativeApi));
}

#[test]
fn state_changing_requests_go_to_libusb() {
    assert_eq!(standard(&[0x00, 9, 1, 0, 0, 0, 0, 0]), Some(Standard::SetConfiguration(1)));
    assert_eq!(standard(&[0x01, 11, 1, 0, 2, 0, 0, 0]), Some(Standard::SetInterface { interface: 2, setting: 1 }));
    assert_eq!(standard(&[0x02, 1, 0, 0, 0x81, 0, 0, 0]), Some(Standard::ClearHalt(0x81)));
    assert_eq!(standard(&[0x23, 3, 4, 0, 1, 0, 0, 0]), Some(Standard::PortReset));
    // GET_DESCRIPTOR, a class request, CLEAR_FEATURE and SET_FEATURE of other features pass through.
    assert_eq!(standard(&[0x80, 6, 0, 1, 0, 0, 18, 0]), None);
    assert_eq!(standard(&[0x21, 9, 0, 2, 0, 0, 8, 0]), None);
    assert_eq!(standard(&[0x02, 1, 1, 0, 0x81, 0, 0, 0]), None);
    assert_eq!(standard(&[0x23, 3, 8, 0, 1, 0, 0, 0]), None);
}

/// A handle that logs what it is asked, and fails what the test says.
#[derive(Default)]
struct Fake {
    log: std::cell::RefCell<Vec<String>>,
    set: Option<rusb::Error>,
    reset: Option<rusb::Error>,
}

impl Fake {
    fn note(&self, call: String, result: Option<rusb::Error>) -> rusb::Result<()> {
        self.log.borrow_mut().push(call);
        result.map_or(Ok(()), Err)
    }
    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut self.log.borrow_mut())
    }
}

impl Configure for Fake {
    fn claim_interface(&self, interface: u8) -> rusb::Result<()> { self.note(format!("claim {interface}"), None) }
    fn release_interface(&self, interface: u8) -> rusb::Result<()> { self.note(format!("release {interface}"), None) }
    fn set_auto_detach_kernel_driver(&self, on: bool) -> rusb::Result<()> { self.note(format!("detach {on}"), None) }
    fn set_active_configuration(&self, value: u8) -> rusb::Result<()> { self.note(format!("set {value}"), self.set) }
    fn reset(&self) -> rusb::Result<()> { self.note("reset".into(), self.reset) }
}

/// In configuration 1 (interfaces 0 and 1, endpoint 0x81) with both claimed; 2 has interface 0
/// and endpoint 0x82.
fn configured() -> State {
    let config = |interfaces: &[u8], endpoint| Config { interfaces: interfaces.to_vec(), endpoints: HashMap::from([(endpoint, (TransferType::Interrupt, 8))]) };
    let configs = HashMap::from([(1, config(&[0, 1], 0x81)), (2, config(&[0], 0x82))]);
    State { configuration: 1, active: 1, claimed: vec![0, 1], endpoints: configs[&1].endpoints.clone(), configs, closed: false }
}

#[test]
fn a_configuration_the_device_refuses_leaves_the_old_one_claimed() {
    let (fake, mut state) = (Fake { set: Some(rusb::Error::Pipe), ..Fake::default() }, configured());
    assert_eq!(set_configuration(&fake, &mut state, 2, "1-1").status, wire::EPIPE);
    // The kernel's drivers stay off the interfaces in between.
    assert_eq!(fake.calls(), ["detach false", "release 0", "release 1", "set 2", "detach true", "claim 0", "claim 1"]);
    assert_eq!((state.configuration, state.active, &state.claimed[..]), (1, 1, &[0, 1][..]));
    assert!(state.endpoints.contains_key(&0x81));
}

#[test]
fn a_new_configuration_claims_its_own_interfaces() {
    let (fake, mut state) = (Fake::default(), configured());
    assert_eq!(set_configuration(&fake, &mut state, 2, "1-1").status, 0);
    assert_eq!(fake.calls(), ["detach false", "release 0", "release 1", "set 2", "detach true", "claim 0"]);
    assert_eq!((state.configuration, state.active, &state.claimed[..]), (2, 2, &[0][..]));
    assert!(state.endpoints.contains_key(&0x82) && !state.endpoints.contains_key(&0x81));
    // One the device doesn't have is refused before anything changes.
    assert_eq!(set_configuration(&fake, &mut state, 3, "1-1").status, wire::EINVAL);
    assert!(fake.calls().is_empty());
}

#[test]
fn unconfiguring_hides_the_configuration_until_the_host_sets_it_again() {
    let (fake, mut state) = (Fake::default(), configured());
    assert_eq!(set_configuration(&fake, &mut state, 0, "1-1").status, 0);
    assert_eq!((state.configuration, state.active, &state.claimed[..]), (0, 1, &[0, 1][..]));
    assert!(state.endpoints.is_empty());
    assert_eq!(set_configuration(&fake, &mut state, 1, "1-1").status, 0);
    assert_eq!(state.configuration, 1);
    assert!(state.endpoints.contains_key(&0x81));
    // The device never left configuration 1, and its interfaces stayed claimed.
    assert!(fake.calls().is_empty());
}

#[test]
fn a_port_reset_resets_the_device_and_claims_its_interfaces_again() {
    let (fake, mut state) = (Fake::default(), configured());
    assert_eq!(reset_port(&fake, &mut state, "1-1").status, 0);
    assert_eq!(fake.calls(), ["reset", "claim 0", "claim 1"]);
    assert_eq!(&state.claimed[..], [0, 1]);
    // A device that came back as another one is gone.
    let fake = Fake { reset: Some(rusb::Error::NotFound), ..Fake::default() };
    assert_eq!(reset_port(&fake, &mut state, "1-1").status, wire::ENODEV);
    assert_eq!(fake.calls(), ["reset"]);
}

#[test]
fn libusb_errors_become_usbip_ones() {
    assert_eq!(errno(LIBUSB_ERROR_PIPE), wire::EPIPE);
    assert_eq!(errno(LIBUSB_ERROR_NO_DEVICE), wire::ENODEV);
    assert_eq!(errno(rusb::constants::LIBUSB_ERROR_IO), wire::EPROTO);
    assert_eq!(rusb_errno(rusb::Error::Pipe), wire::EPIPE);
    assert_eq!(rusb_errno(rusb::Error::Timeout), wire::ETIMEDOUT);
}

#[test]
fn a_read_keeps_what_each_slice_brought() {
    // 64-byte packets: two slices run out after one packet each, the third ends short.
    let mut slices = vec![(LIBUSB_ERROR_TIMEOUT, 64), (LIBUSB_ERROR_TIMEOUT, 64), (LIBUSB_SUCCESS, 10)].into_iter();
    let mut offered = Vec::new();
    let reply = read_sliced(512, &AtomicBool::new(false), |buffer| {
        offered.push(buffer.len());
        let (code, n) = slices.next().unwrap();
        buffer[..n].fill(offered.len() as u8);
        (code, n)
    });
    assert_eq!(offered, [512, 448, 384]);
    assert_eq!(reply.status, 0);
    assert_eq!(reply.data.len(), 138);
    assert_eq!((reply.data[0], reply.data[64], reply.data[137]), (1, 2, 3));
}

#[test]
fn a_waiting_read_ends_when_cancelled_or_failed() {
    let cancelled = AtomicBool::new(false);
    let mut calls = 0;
    let reply = read_sliced(8, &cancelled, |_| {
        calls += 1;
        if calls == 3 { cancelled.store(true, Ordering::SeqCst); }
        (LIBUSB_ERROR_TIMEOUT, 0)
    });
    assert_eq!((reply.status, calls), (wire::ECONNRESET, 3));
    assert_eq!(read_sliced(8, &AtomicBool::new(false), |_| (LIBUSB_ERROR_PIPE, 0)).status, wire::EPIPE);
}

#[test]
fn a_write_goes_on_after_a_slice_and_ends_with_the_zero_packet_asked_for() {
    let mut sent = Vec::new();
    let mut slices = vec![(LIBUSB_ERROR_TIMEOUT, 64), (LIBUSB_SUCCESS, 64), (LIBUSB_SUCCESS, 0)].into_iter();
    let reply = write_sliced(&[7; 128], true, &AtomicBool::new(false), |rest| {
        sent.push(rest.len());
        slices.next().unwrap()
    });
    assert_eq!((reply.status, reply.sent), (0, 128));
    assert_eq!(sent, [128, 64, 0]);
    let mut calls = 0;
    let reply = write_sliced(&[7; 10], false, &AtomicBool::new(false), |rest| { calls += 1; (LIBUSB_SUCCESS, rest.len()) });
    assert_eq!((reply.sent, calls), (10, 1));
    let mut calls = 0;
    let reply = write_sliced(&[], false, &AtomicBool::new(false), |rest| { calls += 1; (LIBUSB_SUCCESS, rest.len()) });
    assert_eq!((reply.sent, calls), (0, 1));
}

#[test]
fn refusals_and_failures_say_why() {
    let ended = |state, reason| Phase::Ended { state, reason, error: None };
    let refused = ended(UsbState::Refused, UsbReason::Permission);
    assert_eq!(phase_label(&refused), "The PC has not allowed USB devices for this client.");
    assert!(phase_failed(&refused));
    assert!(phase_failed(&ended(UsbState::Failed, UsbReason::None)));
    assert!(phase_failed(&ended(UsbState::Detached, UsbReason::DeviceError)));
    let error = Phase::Ended { state: UsbState::Failed, reason: UsbReason::DeviceError, error: Some("the USB tunnel failed".into()) };
    assert_eq!(phase_label(&error), "Failed: the USB tunnel failed");
    // The user's own detach and the stream's end are no failures.
    assert!(!phase_failed(&ended(UsbState::Detached, UsbReason::None)));
    assert!(!phase_failed(&ended(UsbState::Detached, UsbReason::SessionEnd)));
    assert_eq!(attach_error(&AttachError::Send(SendError::Unsupported)), "The PC does not forward USB devices.");
}

#[test]
fn blocked_devices_say_why() {
    assert!(Blocked::NoWinUsb("USBSTOR".into()).message().contains("USBSTOR"));
    assert!(Blocked::Isochronous.message().contains("isochronous"));
}
