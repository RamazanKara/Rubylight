//! Unit tests for USB forwarding device selection, libusb state transitions, sliced transfers, and labels.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use nightjar_core::usb::wire;
use rusb::TransferType;
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
    Local { busid: busid.into(), vid, pid, name: format!("{vid:04x}:{pid:04x}"), blocked, kernel: false, usbipd: UsbipdOffer::None }
}

#[test]
fn automatic_devices_attach_once_per_plug_in_on_any_port_and_retry_failures() {
    use std::time::{Duration, Instant};
    use super::auto::auto_due;

    assert_eq!(auto_key(0x28de, 0x1304), "28de:1304");
    let auto = vec![auto_key(0x28de, 0x1304), auto_key(0x045e, 0x0719)];
    let puck = plugged("1-2", 0x28de, 0x1304, None);
    let receiver = plugged("1-3", 0x045e, 0x0719, Some(Blocked::NoWinUsb("xusb22".into())));
    let tablet = plugged("1-4", 0x056a, 0x0374, None);
    let mut tried = HashMap::new();
    let local = [puck.clone(), receiver, tablet];
    let t0 = Instant::now();
    // Only the listed one that can be forwarded; not the unlisted tablet or the blocked receiver.
    assert_eq!(auto_due(&local, &auto, &[], |_| false, &mut tried, t0), vec![puck.clone()]);
    // Not again while it stays plugged in and attach succeeded / was user-detached.
    assert!(auto_due(&local, &auto, &[], |_| false, &mut tried, t0 + Duration::from_secs(30)).is_empty());
    // Failed automatic attach retries after cooldown while staying plugged in.
    tried.insert("1-2".into(), Some(t0 + Duration::from_secs(10)));
    assert!(auto_due(&local, &auto, &[], |_| false, &mut tried, t0 + Duration::from_secs(5)).is_empty());
    assert_eq!(auto_due(&local, &auto, &[], |_| false, &mut tried, t0 + Duration::from_secs(10)), vec![puck]);
    // Unplugged, then plugged into another port: attached again immediately.
    assert!(auto_due(&[], &auto, &[], |_| false, &mut tried, t0).is_empty());
    let moved = plugged("2-1", 0x28de, 0x1304, None);
    assert_eq!(auto_due(std::slice::from_ref(&moved), &auto, &[], |_| false, &mut tried, t0), std::slice::from_ref(&moved));
    // One the user attached already is left alone.
    let mut fresh = HashMap::new();
    assert!(auto_due(&[moved], &auto, &[], |busid| busid == "2-1", &mut fresh, t0).is_empty());
}

#[test]
fn a_cancelled_password_prompt_stops_the_automatic_attach_until_the_user_acts() {
    use std::time::Instant;
    use super::auto::{auto_due, hushed_on_refusal, note_password_refused};

    let quiet = Mutex::new(Vec::new());
    let deck = plugged("3-1", 0x28de, 0x1205, None);
    // Another failure leaves it to the retry cooldown, as before.
    assert!(hushed_on_refusal::<()>(&quiet, &deck, || Err("busy".into())).is_err());
    assert!(quiet.lock().unwrap().is_empty());
    assert!(hushed_on_refusal(&quiet, &deck, || Ok(())).is_ok());
    assert!(quiet.lock().unwrap().is_empty());
    // pkexec's prompt was cancelled during the open: hushed, once.
    for _ in 0..2 {
        let cancelled = hushed_on_refusal::<()>(&quiet, &deck, || {
            note_password_refused();
            Err("cancelled".into())
        });
        assert_eq!(cancelled.unwrap_err(), "cancelled", "the user still hears why");
    }
    assert_eq!(*quiet.lock().unwrap(), vec![auto_key(0x28de, 0x1205)]);
    // The automatic attach leaves it alone, also after the cooldown or another plug-in.
    let hushed = quiet.lock().unwrap().clone();
    let auto = vec![auto_key(0x28de, 0x1205), auto_key(0x28de, 0x1304)];
    let puck = plugged("1-2", 0x28de, 0x1304, None);
    let mut tried = HashMap::new();
    let local = [deck.clone(), puck.clone()];
    assert_eq!(auto_due(&local, &auto, &hushed, |_| false, &mut tried, Instant::now()), vec![puck], "other devices still attach");
    assert!(auto_due(&[plugged("4-2", 0x28de, 0x1205, None)], &auto, &hushed, |_| false, &mut HashMap::new(), Instant::now()).is_empty());
    // A user action (Attach, or Automatic ticked again) takes it off the list.
    quiet.lock().unwrap().retain(|k| *k != auto_key(deck.vid, deck.pid));
    assert_eq!(auto_due(&[deck.clone()], &auto, &quiet.lock().unwrap(), |_| false, &mut HashMap::new(), Instant::now()), vec![deck]);
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

#[test]
fn platform_backend_routing_never_blocks_isochronous_devices() {
    // Windows: WinUSB iso uses inbox winusb.dll; non-iso and libusbK use libusb.
    assert_eq!(select_backend(Platform::Windows { uses_winusb: true }, true), Backend::WinUsbIso);
    assert_eq!(select_backend(Platform::Windows { uses_winusb: true }, false), Backend::Libusb);
    assert_eq!(select_backend(Platform::Windows { uses_winusb: false }, true), Backend::Libusb);
    assert_eq!(select_backend(Platform::Windows { uses_winusb: false }, false), Backend::Libusb);
    // Linux: prefers usbip-host for iso when installed, falls back to libusb iso when missing.
    assert_eq!(select_backend(Platform::Linux { has_kernel_usbip: true }, true), Backend::LinuxKernel);
    assert_eq!(select_backend(Platform::Linux { has_kernel_usbip: false }, true), Backend::Libusb);
    assert_eq!(select_backend(Platform::Linux { has_kernel_usbip: true }, false), Backend::Libusb);
    // macOS: libusb serves both iso and non-iso.
    assert_eq!(select_backend(Platform::MacOs, true), Backend::Libusb);
    assert_eq!(select_backend(Platform::MacOs, false), Backend::Libusb);
}

#[test]
fn iso_plan_validates_packet_counts_lengths_and_out_payloads() {
    use nightjar_core::usb::{Direction, IsoPacket, Transfer};
    let make = |direction, length, iso: Vec<IsoPacket>, data: Vec<u8>| Transfer {
        seqnum: 1,
        endpoint: 1,
        direction,
        flags: 0,
        length,
        start_frame: 0,
        interval: 1,
        setup: [0; 8],
        data,
        iso,
    };
    let pkt = |offset, length| IsoPacket { offset, length, actual_length: 0, status: 0 };

    // Empty, zero-total-length, and over-cap packet lists are rejected.
    assert_eq!(super::iso::iso_plan(&make(Direction::In, 64, vec![], vec![])), Err(wire::EINVAL));
    assert_eq!(super::iso::iso_plan(&make(Direction::In, 64, vec![pkt(0, 0)], vec![])), Err(wire::EINVAL));
    let too_many: Vec<IsoPacket> = (0..super::iso::MAX_ISO_PACKETS + 1).map(|i| pkt(i as u32, 1)).collect();
    assert_eq!(super::iso::iso_plan(&make(Direction::In, 2048, too_many, vec![])), Err(wire::EINVAL));

    // Packet length sum exceeding transfer.length or overflowing is rejected.
    assert_eq!(
        super::iso::iso_plan(&make(Direction::In, 100, vec![pkt(0, 64), pkt(64, 64)], vec![])),
        Err(wire::EINVAL)
    );
    assert_eq!(
        super::iso::iso_plan(&make(Direction::In, u32::MAX, vec![pkt(0, u32::MAX), pkt(0, 1)], vec![])),
        Err(wire::EINVAL)
    );

    // Valid IN plan sums packet lengths.
    assert_eq!(
        super::iso::iso_plan(&make(Direction::In, 192, vec![pkt(0, 64), pkt(64, 128)], vec![])),
        Ok(192)
    );

    // OUT plan rejects truncated data and accepts matching or larger buffer.
    assert_eq!(
        super::iso::iso_plan(&make(Direction::Out, 128, vec![pkt(0, 64), pkt(64, 64)], vec![0; 100])),
        Err(wire::EINVAL)
    );
    assert_eq!(
        super::iso::iso_plan(&make(Direction::Out, 128, vec![pkt(0, 64), pkt(64, 64)], vec![0; 128])),
        Ok(128)
    );
}

#[test]
fn libusb_and_winusb_status_mappings_match_usbip_errnos() {
    use rusb::constants::*;
    assert_eq!(super::iso::libusb_transfer_status(LIBUSB_TRANSFER_COMPLETED), 0);
    assert_eq!(super::iso::libusb_transfer_status(LIBUSB_TRANSFER_STALL), wire::EPIPE);
    assert_eq!(super::iso::libusb_transfer_status(LIBUSB_TRANSFER_NO_DEVICE), wire::ENODEV);
    assert_eq!(super::iso::libusb_transfer_status(LIBUSB_TRANSFER_OVERFLOW), wire::EOVERFLOW);
    assert_eq!(super::iso::libusb_transfer_status(LIBUSB_TRANSFER_TIMED_OUT), wire::ETIMEDOUT);
    assert_eq!(super::iso::libusb_transfer_status(LIBUSB_TRANSFER_CANCELLED), wire::ECONNRESET);
    assert_eq!(super::iso::libusb_transfer_status(LIBUSB_TRANSFER_ERROR), wire::EPROTO);

    assert_eq!(super::winusb::usbd_status_to_errno(0x0000_0000), 0);
    assert_eq!(super::winusb::usbd_status_to_errno(0xC000_0004), wire::EPIPE);
    assert_eq!(super::winusb::usbd_status_to_errno(0xC000_0008), wire::EOVERFLOW);
    assert_eq!(super::winusb::usbd_status_to_errno(0xC000_000C), wire::EOVERFLOW);
    assert_eq!(super::winusb::usbd_status_to_errno(0xC000_6000), wire::ETIMEDOUT);
    assert_eq!(super::winusb::usbd_status_to_errno(0xC000_7000), wire::ENODEV);
    assert_eq!(super::winusb::usbd_status_to_errno(0xC001_0000), wire::ECONNRESET);
    assert_eq!(super::winusb::usbd_status_to_errno(0xC003_0000), wire::EPROTO);

    assert_eq!(super::winusb::win32_error_to_errno(995), wire::ECONNRESET);
    assert_eq!(super::winusb::win32_error_to_errno(121), wire::ETIMEDOUT);
    assert_eq!(super::winusb::win32_error_to_errno(1167), wire::ENODEV);
    assert_eq!(super::winusb::win32_error_to_errno(234), wire::EOVERFLOW);
    assert_eq!(super::winusb::win32_error_to_errno(31), wire::EPIPE);
    assert_eq!(super::winusb::win32_error_to_errno(87), wire::EINVAL);
    assert_eq!(super::winusb::win32_error_to_errno(5), wire::EPROTO);

    // Control transfer recipient routing for composite WinUSB devices.
    assert_eq!(super::winusb::control_target_interface(&[0x21, 0x01, 0, 0, 3, 0, 0, 0]), Some(3));
    assert_eq!(super::winusb::control_target_endpoint(&[0x21, 0x01, 0, 0, 3, 0, 0, 0]), None);
    assert_eq!(super::winusb::control_target_endpoint(&[0x22, 0x01, 0, 0, 0x81, 0, 0, 0]), Some(0x81));
    assert_eq!(super::winusb::control_target_interface(&[0x80, 0x06, 0, 1, 0, 0, 18, 0]), None);
}

#[test]
fn iso_reply_packing_for_libusb_and_winusb_satisfies_usbip_wire_invariants() {
    use nightjar_core::usb::{Direction, IsoPacket};
    use super::iso::{pack_iso_reply, CompletedPacket};
    use super::winusb::{pack_winusb_iso_in, pack_winusb_iso_out, UsbdIsoPacketDescriptor};

    let asked = [
        IsoPacket { offset: 0, length: 4, actual_length: 0, status: 0 },
        IsoPacket { offset: 4, length: 4, actual_length: 0, status: 0 },
        IsoPacket { offset: 8, length: 4, actual_length: 0, status: 0 },
    ];
    let mut buf = vec![0u8; 12];
    buf[0..3].copy_from_slice(&[10, 11, 12]);
    // Packet 1 failed with 0 bytes; packet 2 received 2 bytes starting at offset 8.
    buf[8..10].copy_from_slice(&[20, 21]);

    // 1. libusb IN packing compacts short/failed packets and preserves asked offsets/lengths.
    let completed = [
        CompletedPacket { offset: 0, actual_length: 3, status: 0 },
        CompletedPacket { offset: 4, actual_length: 0, status: wire::EPROTO },
        CompletedPacket { offset: 8, actual_length: 2, status: 0 },
    ];
    let reply = pack_iso_reply(Direction::In, 12, &asked, &completed, &buf);
    assert_eq!(reply.status, 0);
    assert_eq!(reply.error_count, 1);
    assert_eq!(reply.data, [10, 11, 12, 20, 21]);
    assert!(wire::iso_reply_fits(&asked, reply.iso.iter().copied(), reply.data.len() as u32));
    assert_eq!(reply.iso[0].actual_length, 3);
    assert_eq!((reply.iso[1].actual_length, reply.iso[1].status), (0, wire::EPROTO));
    assert_eq!(reply.iso[2].actual_length, 2);

    // 2. WinUSB IN packing respects driver-provided USBD_ISO_PACKET_DESCRIPTOR offsets and clamps overrun.
    let descs = [
        UsbdIsoPacketDescriptor { offset: 0, length: 99, status: 0 },
        UsbdIsoPacketDescriptor { offset: 4, length: 0, status: 0xC003_0000 },
        UsbdIsoPacketDescriptor { offset: 8, length: 2, status: 0 },
    ];
    let win_in = pack_winusb_iso_in(12, &asked, &descs, &buf);
    assert_eq!(win_in.error_count, 1);
    assert_eq!(win_in.data, [10, 11, 12, 0, 20, 21]);
    assert!(wire::iso_reply_fits(&asked, win_in.iso.iter().copied(), win_in.data.len() as u32));

    // 3. WinUSB OUT packing distributes total transferred bytes across asked packets.
    let win_out = pack_winusb_iso_out(12, &asked, 10);
    assert_eq!((win_out.status, win_out.sent, win_out.error_count), (0, 10, 0));
    assert!(win_out.data.is_empty());
    assert!(wire::iso_reply_fits(&asked, win_out.iso.iter().copied(), win_out.sent));
    assert_eq!(
        win_out.iso.iter().map(|p| p.actual_length).collect::<Vec<_>>(),
        vec![4, 4, 2]
    );
}

#[test]
fn winusb_iso_out_packet_table_validation_rejects_unexpressible_layouts() {
    use nightjar_core::usb::IsoPacket;
    use super::winusb::validate_winusb_iso_out_table;

    let pkt = |offset, length| IsoPacket { offset, length, actual_length: 0, status: 0 };

    // Expressible: single packet <= max_bytes_per_interval, or full chunks with optional trailing short packet.
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 128)], 512), Ok(128));
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 512), pkt(512, 512), pkt(1024, 192)], 512), Ok(1216));
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 2048), pkt(2048, 256)], 2048), Ok(2304));

    // Unexpressible: interior short packet (UAC 48kHz audio in 256-byte pipe), zero-length gap,
    // oversized packet, non-contiguous offsets, or unknown pipe interval cap (0).
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 192), pkt(192, 192)], 256), Err(wire::EINVAL));
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 512), pkt(512, 128), pkt(640, 512)], 512), Err(wire::EINVAL));
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 512), pkt(512, 0), pkt(512, 512)], 512), Err(wire::EINVAL));
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 513)], 512), Err(wire::EINVAL));
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 512), pkt(600, 512)], 512), Err(wire::EINVAL));
    assert_eq!(validate_winusb_iso_out_table(&[pkt(0, 512)], 0), Err(wire::EINVAL));
    assert_eq!(validate_winusb_iso_out_table(&[], 512), Err(wire::EINVAL));
}

#[test]
fn winusb_overlapped_cancel_preserves_econnreset_and_error_87_does_not_retry() {
    use std::sync::{Arc, Mutex};
    use super::winusb::iso_coord::{submit_pipelined, IsoEndpointState};
    use super::winusb::overlapped_error_to_errno;

    // R1-DM2: cancel must surface as ECONNRESET even if kernel completed abort with non-995 error.
    assert_eq!(overlapped_error_to_errno(995, None), wire::ECONNRESET);
    assert_eq!(overlapped_error_to_errno(995, Some(wire::ETIMEDOUT)), wire::ETIMEDOUT);
    assert_eq!(overlapped_error_to_errno(31, Some(wire::ECONNRESET)), wire::ECONNRESET);
    assert_eq!(overlapped_error_to_errno(1167, None), wire::ENODEV);
    assert_eq!(overlapped_error_to_errno(31, None), wire::EPIPE);

    // Concurrent pipelined iso submissions start ContinueStream=false then true without gaps.
    let ep = Arc::new(Mutex::new(IsoEndpointState::default()));
    let submitted = Arc::new(Mutex::new(Vec::<bool>::new()));
    let gate = Arc::new(std::sync::Barrier::new(3));
    std::thread::scope(|s| {
        for _ in 0..3 {
            let (ep, submitted, gate) = (ep.clone(), submitted.clone(), gate.clone());
            s.spawn(move || {
                gate.wait();
                let (_, ticket) = submit_pipelined(&ep, |cont| {
                    submitted.lock().unwrap().push(cont);
                    Ok(())
                }).unwrap();
                ep.lock().unwrap().finish(ticket, true);
            });
        }
    });
    let flags = submitted.lock().unwrap();
    assert_eq!(flags.as_slice(), &[false, true, true], "only first submission resets ContinueStream; followers pipeline without gaps");

    // Lapse (21 or 59) retries with ContinueStream=false; ERROR_INVALID_PARAMETER (87) fails immediately (R1-DM1).
    let mut attempts = Vec::new();
    let (_, ticket_21) = submit_pipelined(&ep, |cont| {
        attempts.push(cont);
        if cont { Err((21, wire::EPROTO)) } else { Ok(()) }
    }).unwrap();
    assert_eq!(attempts, vec![true, false]);
    ep.lock().unwrap().finish(ticket_21, true);

    let mut attempts_87 = Vec::new();
    let err = submit_pipelined(&ep, |cont| -> Result<(), (u32, i32)> {
        attempts_87.push(cont);
        Err((87, wire::EINVAL))
    });
    assert_eq!(err, Err(wire::EINVAL));
    assert_eq!(attempts_87, vec![true], "error 87 must not be retried as stream lapse");

    // Stale completion failure from old reset epoch does not clear a reset/restarted stream.
    let (_, stale_ticket) = submit_pipelined(&ep, |_| Ok(())).unwrap();
    ep.lock().unwrap().reset();
    let (_, new_ticket) = submit_pipelined(&ep, |_| Ok(())).unwrap();
    ep.lock().unwrap().finish(stale_ticket, false);
    assert!(ep.lock().unwrap().is_streaming(), "stale completion must not reset active epoch");
    ep.lock().unwrap().finish(new_ticket, false);
    assert!(!ep.lock().unwrap().is_streaming());
}

#[test]
fn winusb_port_reset_restores_alt_zero_endpoints_and_resets_streams() {
    use std::sync::{Arc, Mutex};
    use super::winusb::iso_coord::{
        winusb_reset_port, EndpointInfo, IsoEndpointState, ResettableState, WinUsbPortReset,
    };

    struct FakePortReset {
        log: Mutex<Vec<String>>,
        fail_alt: bool,
    }
    impl WinUsbPortReset for FakePortReset {
        fn reset_pipe(&self, interface: u8, endpoint: u8) -> Result<(), i32> {
            self.log.lock().unwrap().push(format!("reset_pipe({interface},{endpoint:#x})"));
            Ok(())
        }
        fn set_alt(&self, interface: u8, setting: u8) -> Result<(), i32> {
            self.log.lock().unwrap().push(format!("set_alt({interface},{setting})"));
            if self.fail_alt { Err(wire::EPIPE) } else { Ok(()) }
        }
    }

    let alt0 = vec![(0x81u8, 64u16, 64u32)];
    let alt1 = vec![(0x81u8, 512u16, 1024u32), (0x02u8, 512u16, 512u32)];
    let resettable = Arc::new(Mutex::new(ResettableState {
        interfaces: vec![0, 1],
        endpoints: HashMap::from([
            (0x81, EndpointInfo { interface: 1, max_packet: 512, max_bytes_per_interval: 1024 }),
            (0x02, EndpointInfo { interface: 1, max_packet: 512, max_bytes_per_interval: 512 }),
        ]),
        alt_endpoints: HashMap::from([((0, 0), alt0.clone()), ((1, 0), vec![]), ((1, 1), alt1)]),
    }));
    let stream = Arc::new(Mutex::new(IsoEndpointState::default()));
    let streams = Mutex::new(HashMap::from([(0x81u8, stream.clone())]));
    let _ = super::winusb::iso_coord::submit_pipelined(&stream, |_| Ok(())).unwrap();
    assert!(stream.lock().unwrap().is_streaming());

    let fake = FakePortReset { log: Mutex::new(Vec::new()), fail_alt: false };
    let reply = winusb_reset_port(&fake, &resettable, &streams);
    assert_eq!(reply.status, 0);
    assert!(!stream.lock().unwrap().is_streaming(), "iso stream cleared on port reset");
    {
        let guard = resettable.lock().unwrap();
        let endpoints = &guard.endpoints;
        assert_eq!(endpoints.get(&0x81).map(|e| (e.interface, e.max_packet)), Some((0, 64)));
        assert!(!endpoints.contains_key(&0x02), "alt > 0 endpoint removed after port reset");
    }
    let log = fake.log.lock().unwrap();
    assert!(log.contains(&"set_alt(0,0)".to_owned()) && log.contains(&"set_alt(1,0)".to_owned()));

    let fail = FakePortReset { log: Mutex::new(Vec::new()), fail_alt: true };
    assert_eq!(winusb_reset_port(&fail, &resettable, &streams).status, wire::EPIPE);
}

#[test]
fn winusb_iso_pool_rejects_stale_slots_across_set_interface_and_reset_port() {
    use super::winusb::iso_coord::IsoPoolState;

    let mut pool = IsoPoolState::<&'static str>::default();
    let (ep0_gen, missing) = pool.take(0x81, 0, |_| true);
    assert_eq!(missing, None);
    let (ep1_gen, _) = pool.take(0x82, 1, |_| true);

    // Concurrent `set_interface(1)` invalidates interface 1 before & after updating alt endpoints.
    assert!(pool.release(0x81, 0, ep0_gen, "iface0-idle", 8).is_none());
    let (inflight_gen, _) = pool.take(0x82, 1, |_| true);
    assert!(pool.invalidate(Some(1)).is_empty());
    assert!(pool.invalidate(Some(1)).is_empty());

    // Pre-alt-switch slot returned after alt-0 repopulated `0x82` on interface 1 is rejected.
    assert_eq!(pool.release(0x82, 1, inflight_gen, "stale-pre-set-alt", 8), Some("stale-pre-set-alt"));
    assert_eq!(pool.release(0x82, 1, ep1_gen, "older-stale", 8), Some("older-stale"));
    // Unaffected interface 0 kept its idle slot until `reset_port`.
    assert_eq!(pool.take(0x81, 0, |_| true), (ep0_gen, Some("iface0-idle")));

    // Re-insert `0x81` before `reset_port` bumps the global epoch across every interface.
    let (before_reset_gen, _) = pool.take(0x81, 0, |_| true);
    assert!(pool.release(0x81, 0, before_reset_gen, "idle-before-reset", 8).is_none());
    let (inflight_reset_gen, taken) = pool.take(0x81, 0, |_| true);
    assert_eq!(taken, Some("idle-before-reset"));
    assert!(pool.invalidate(None).is_empty());
    assert!(pool.invalidate(None).is_empty());
    assert_eq!(pool.release(0x81, 0, inflight_reset_gen, "stale-pre-reset", 8), Some("stale-pre-reset"));

    let (post_reset_gen, empty) = pool.take(0x81, 0, |_| true);
    assert!(post_reset_gen > inflight_reset_gen);
    assert_eq!(empty, None);
    assert!(pool.release(0x81, 0, post_reset_gen, "fresh-post-reset", 8).is_none());
    assert_eq!(pool.take(0x81, 0, |_| true), (post_reset_gen, Some("fresh-post-reset")));
}

/// Track 8B: libusb refused permission (`EACCES`, no udev rule) hands the device to Linux's own
/// USB/IP server when it can run here; the udev advice stays when it can't, and follows the
/// fallback's own failure. Any other libusb answer never starts the fallback.
#[test]
fn a_refused_libusb_open_falls_back_to_the_kernel_server_and_keeps_the_udev_advice() {
    use std::cell::Cell;

    let access = open_error(rusb::Error::Access);
    let tried = Cell::new(0);
    let kernel_ok = || { tried.set(tried.get() + 1); Some(Ok("kernel")) };
    assert_eq!(open_or_kernel(Err(rusb::Error::Access), kernel_ok), Ok("kernel"));
    assert_eq!(tried.get(), 1, "the fallback runs once");

    assert_eq!(open_or_kernel::<&str>(Err(rusb::Error::Access), || None), Err(access.clone()), "nothing to fall back to: the advice alone");

    let failed = open_or_kernel::<&str>(Err(rusb::Error::Access), || Some(Err("The prompt was cancelled.".to_owned()))).unwrap_err();
    assert!(failed.starts_with("The prompt was cancelled."), "{failed}");
    assert!(failed.ends_with(&access), "the advice follows the fallback's failure: {failed}");

    let never = || -> Option<Result<&str, String>> { panic!("only EACCES falls back") };
    assert_eq!(open_or_kernel(Ok("libusb"), never), Ok("libusb"));
    assert_eq!(open_or_kernel::<&str>(Err(rusb::Error::Busy), never), Err(open_error(rusb::Error::Busy)));
    assert_eq!(open_or_kernel::<&str>(Err(rusb::Error::NoDevice), never), Err(open_error(rusb::Error::NoDevice)));
}
