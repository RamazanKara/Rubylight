use super::*;

fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> Setup {
    Setup {
        request_type,
        request,
        value,
        index,
        length,
    }
}

fn relative(report: &[u8]) -> (u8, i16, i16, i8, i8) {
    assert_eq!(report.len(), 8);
    assert_eq!(report[0], 1);
    (
        report[1],
        i16::from_le_bytes([report[2], report[3]]),
        i16::from_le_bytes([report[4], report[5]]),
        report[6] as i8,
        report[7] as i8,
    )
}

/// Walks a report descriptor's short items and returns the bits of each (report type, id):
/// what a HID parser computes the report sizes from.
fn report_bits(descriptor: &[u8]) -> std::collections::BTreeMap<(u8, u8), u32> {
    let mut sizes = std::collections::BTreeMap::new();
    let (mut size, mut count, mut id) = (0u32, 0u32, 0u8);
    let mut stack = vec![];
    let mut depth = 0i32;
    let mut i = 0;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        let len = match prefix & 3 {
            3 => 4,
            n => n as usize,
        };
        let mut data = 0u32;
        for (n, b) in descriptor[i + 1..i + 1 + len].iter().enumerate() {
            data |= u32::from(*b) << (8 * n);
        }
        match prefix & 0xfc {
            0x74 => size = data,
            0x94 => count = data,
            0x84 => id = data as u8,
            0xa4 => stack.push((size, count, id)),
            0xb4 => (size, count, id) = stack.pop().unwrap(),
            0x80 => *sizes.entry((1, id)).or_default() += size * count,
            0xb0 => *sizes.entry((3, id)).or_default() += size * count,
            0xa0 => depth += 1,
            0xc0 => depth -= 1,
            _ => {}
        }
        i += 1 + len;
    }
    assert_eq!(depth, 0, "every collection ends");
    sizes
}

#[test]
fn the_mouse_descriptor_gives_the_reports_the_mouse_sends() {
    let bits = report_bits(MOUSE_REPORT_DESCRIPTOR);
    assert_eq!(bits[&(1, 1)], 7 * 8, "relative: buttons, X, Y, wheel, pan");
    assert_eq!(bits[&(1, 2)], 5 * 8, "absolute: buttons, X, Y");
    assert_eq!(bits[&(3, 3)], 8, "the resolution multiplier");
    let mut mouse = HidMouse::new("1");
    mouse.motion(1, 2);
    assert_eq!(mouse.interrupt(ENDPOINT).unwrap().len(), 8);
    mouse.absolute(1, 2);
    assert_eq!(mouse.interrupt(ENDPOINT).unwrap().len(), 5);
}

#[test]
fn motion_merges_until_polled_and_splits_past_sixteen_bits() {
    let mut mouse = HidMouse::new("1");
    assert!(!mouse.interrupt_ready(ENDPOINT));
    assert!(mouse.interrupt(ENDPOINT).is_none());
    mouse.motion(3, -4);
    mouse.motion(5, 6);
    assert!(mouse.interrupt_ready(ENDPOINT));
    assert!(!mouse.interrupt_ready(2));
    assert_eq!(
        relative(&mouse.interrupt(ENDPOINT).unwrap()),
        (0, 8, 2, 0, 0)
    );
    assert!(!mouse.interrupt_ready(ENDPOINT));

    mouse.motion(40000, -40000);
    assert_eq!(
        relative(&mouse.interrupt(ENDPOINT).unwrap()),
        (0, 32767, -32767, 0, 0)
    );
    assert_eq!(
        relative(&mouse.interrupt(ENDPOINT).unwrap()),
        (0, 7233, -7233, 0, 0)
    );
    assert!(mouse.interrupt(ENDPOINT).is_none());
}

#[test]
fn a_click_shorter_than_a_poll_is_a_press_and_a_release() {
    let mut mouse = HidMouse::new("1");
    mouse.motion(1, 0);
    assert!(mouse.button(button_bit(1).unwrap(), true));
    assert!(!mouse.button(button::LEFT, true), "already held");
    mouse.motion(2, 0);
    assert!(mouse.button(button::LEFT, false));
    let reports: Vec<_> = std::iter::from_fn(|| mouse.interrupt(ENDPOINT))
        .map(|r| relative(&r))
        .collect();
    assert_eq!(
        reports,
        [(0, 1, 0, 0, 0), (1, 2, 0, 0, 0), (0, 0, 0, 0, 0)],
        "motion after the press stays pressed; the release is its own report"
    );
    assert_eq!(button_bit(3), Some(button::RIGHT));
    assert_eq!(button_bit(2), Some(button::MIDDLE));
    assert_eq!(button_bit(4), Some(button::BACK));
    assert_eq!(button_bit(5), Some(button::FORWARD));
    assert_eq!(button_bit(6), None);
}

#[test]
fn the_wheel_steps_follow_the_multiplier_windows_sets() {
    let mut mouse = HidMouse::new("1");
    // Before Windows sets the multiplier, a step is a whole notch: parts wait.
    assert!(!mouse.scroll(60, false));
    assert!(mouse.scroll(60, false));
    assert_eq!(
        relative(&mouse.interrupt(ENDPOINT).unwrap()),
        (0, 0, 0, 1, 0)
    );

    let feature = setup(0x21, 9, 0x0303, 0, 2);
    assert_eq!(mouse.control(&feature, &[3, 1]), Some(vec![]));
    assert_eq!(mouse.multiplier(), WHEEL_MULTIPLIER);
    assert_eq!(
        mouse.control(&setup(0xa1, 1, 0x0303, 0, 2), &[]),
        Some(vec![3, 1])
    );
    // Now a step is 15: a trackpad's 30 is two steps, a notch eight.
    mouse.scroll(30, false);
    mouse.scroll(-120, true);
    assert_eq!(
        relative(&mouse.interrupt(ENDPOINT).unwrap()),
        (0, 0, 0, 2, -1)
    );
    mouse.scroll(120, false);
    assert_eq!(
        relative(&mouse.interrupt(ENDPOINT).unwrap()),
        (0, 0, 0, 8, 0)
    );
    // A huge turn splits over reports instead of wrapping.
    mouse.scroll(i16::MAX, false);
    let total: i32 = std::iter::from_fn(|| mouse.interrupt(ENDPOINT))
        .map(|r| i32::from(relative(&r).3))
        .sum();
    assert_eq!(total, i32::from(i16::MAX) / 15);
}

#[test]
fn absolute_moves_replace_each_other_and_carry_no_buttons() {
    let mut mouse = HidMouse::new("1");
    mouse.button(button::LEFT, true);
    mouse.absolute(100, 200);
    mouse.absolute(40000, 300);
    let press = mouse.interrupt(ENDPOINT).unwrap();
    assert_eq!(relative(&press).0, button::LEFT);
    assert_eq!(
        mouse.interrupt(ENDPOINT).unwrap(),
        [2, 0, 0xff, 0x7f, 0x2c, 0x01]
    );
    assert!(mouse.interrupt(ENDPOINT).is_none());
    assert!(mouse.release_all());
    assert_eq!(relative(&mouse.interrupt(ENDPOINT).unwrap()).0, 0);
    assert!(!mouse.release_all());
}

#[test]
fn windows_reads_the_mouse_descriptors() {
    let mut mouse = HidMouse::new("BUTTERPOLLO");
    let device = mouse.control(&setup(0x80, 6, 0x100, 0, 18), &[]).unwrap();
    assert_eq!(device.len(), 18);
    assert_eq!(&device[8..12], &[0x09, 0x12, 0xb1, 0x52]);
    let config = mouse.control(&setup(0x80, 6, 0x200, 0, 255), &[]).unwrap();
    assert_eq!(config.len(), 9 + 9 + 9 + 7);
    assert_eq!(
        u16::from_le_bytes([config[2], config[3]]) as usize,
        config.len()
    );
    // A 1 ms interrupt IN endpoint.
    assert_eq!(&config[27..], &[7, 5, 0x81, 3, 16, 0, 1]);
    let report = mouse.control(&setup(0x81, 6, 0x2200, 0, 255), &[]).unwrap();
    assert_eq!(report, MOUSE_REPORT_DESCRIPTOR);
    let serial = mouse
        .control(&setup(0x80, 6, 0x303, 0x409, 255), &[])
        .unwrap();
    assert_eq!(serial[0] as usize, serial.len());
    assert_eq!(serial.len(), 2 + 2 * "BUTTERPOLLO".len());
    assert!(mouse.control(&setup(0x80, 6, 0x600, 0, 10), &[]).is_none());
    assert_eq!(
        mouse.control(&setup(0x21, 0x0a, 0, 0, 0), &[]),
        Some(vec![])
    );
    assert!(
        mouse
            .control(&setup(0x21, 0x09, 0x0201, 0, 1), &[1])
            .is_none()
    );
}

fn contact(event: u8, id: u32, x: f32, y: f32) -> Contact {
    Contact { event, id, x, y }
}

/// One finger of a touch report: confidence, tip, slot, X and Y.
fn finger(report: &[u8], n: usize) -> (bool, bool, u8, u16, u16) {
    let at = 1 + n * 5;
    (
        report[at] & 1 != 0,
        report[at] & 2 != 0,
        report[at] >> 2 & 7,
        u16::from_le_bytes([report[at + 1], report[at + 2]]),
        u16::from_le_bytes([report[at + 3], report[at + 4]]),
    )
}
fn scan(report: &[u8]) -> u16 {
    u16::from_le_bytes([report[26], report[27]])
}
fn count(report: &[u8]) -> u8 {
    report[28]
}
fn click(report: &[u8]) -> bool {
    report[29] & 1 != 0
}

fn active(width_mm: u16, height_mm: u16) -> (PrecisionTouchpad, Instant) {
    let epoch = Instant::now();
    let mut pad = PrecisionTouchpad::new(width_mm, height_mm, "1", epoch);
    assert!(!pad.active());
    assert_eq!(
        pad.control(&setup(0x21, 9, 0x0303, 0, 2), &[3, 3]),
        Some(vec![])
    );
    assert!(pad.active());
    (pad, epoch)
}

#[test]
fn the_touchpad_descriptor_matches_its_reports() {
    for (w, h) in [(105, 70), (0, 0), (300, 200), (5, 4000)] {
        let bits = report_bits(&touchpad_report_descriptor(w, h));
        assert_eq!(
            bits[&(1, 4)],
            29 * 8,
            "five fingers, scan time, count, button"
        );
        assert_eq!(bits[&(3, 5)], 8, "contact count maximum and pad type");
        assert_eq!(bits[&(3, 6)], 256 * 8, "certification status");
        assert_eq!(bits[&(3, 3)], 8, "input mode");
        assert_eq!(bits[&(3, 7)], 8, "surface and button switches");
    }
}

#[test]
fn windows_reads_the_touchpad_features_and_switches_it_on() {
    let epoch = Instant::now();
    let mut pad = PrecisionTouchpad::new(120, 80, "1", epoch);
    let config = pad.control(&setup(0x80, 6, 0x200, 0, 255), &[]).unwrap();
    assert_eq!(&config[27..], &[7, 5, 0x81, 3, 64, 0, 1]);
    let device = pad.control(&setup(0x80, 6, 0x100, 0, 18), &[]).unwrap();
    assert_eq!(&device[8..12], &[0x09, 0x12, 0xb2, 0x52]);
    let report = pad.control(&setup(0x81, 6, 0x2200, 0, 1024), &[]).unwrap();
    assert_eq!(report, touchpad_report_descriptor(120, 80));
    // X's logical maximum: 1200 tenths of a millimetre.
    assert!(report.windows(3).any(|w| w == [0x26, 0xb0, 0x04]));
    assert_eq!(
        pad.control(&setup(0xa1, 1, 0x0305, 0, 2), &[]),
        Some(vec![5, 5])
    );
    let certification = pad.control(&setup(0xa1, 1, 0x0306, 0, 257), &[]).unwrap();
    assert_eq!(certification.len(), 257);
    assert_eq!(certification[0], 6);
    assert_eq!(&certification[1..], &PTP_CERTIFICATION_BLOB);
    assert_eq!(
        pad.control(&setup(0xa1, 1, 0x0303, 0, 2), &[]),
        Some(vec![3, 0])
    );

    // Contacts before Windows switches the device to touchpad input are counted, not sent.
    assert!(!pad.apply(&[contact(1, 7, 0.5, 0.5)], 0, epoch));
    assert_eq!(pad.early_contacts, 1);
    assert!(!pad.interrupt_ready(ENDPOINT));
    pad.control(&setup(0x21, 9, 0x0303, 0, 2), &[3, 3]);
    assert!(pad.active());
    assert_eq!(
        pad.control(&setup(0xa1, 1, 0x0303, 0, 2), &[]),
        Some(vec![3, 3])
    );
    assert!(pad.apply(&[contact(3, 7, 0.25, 0.75)], 0, epoch));
    let report = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(report.len(), 30);
    assert_eq!(finger(&report, 0), (true, true, 0, 300, 600));
}

#[test]
fn two_fingers_go_down_move_and_lift_on_their_own_slots() {
    let (mut pad, epoch) = active(100, 50);
    let t = |ms: u64| epoch + Duration::from_millis(ms);
    pad.apply(
        &[contact(1, 40, 0.1, 0.2), contact(1, 41, 0.9, 0.8)],
        0,
        t(1),
    );
    let down = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(count(&down), 2);
    assert_eq!(finger(&down, 0), (true, true, 0, 100, 100));
    assert_eq!(finger(&down, 1), (true, true, 1, 900, 400));
    assert_eq!(scan(&down), 10);

    // A frame naming one finger keeps the other where it was.
    pad.apply(&[contact(3, 41, 0.5, 0.5)], 0, t(2));
    let moved = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(count(&moved), 2);
    assert_eq!(finger(&moved, 0), (true, true, 0, 100, 100));
    assert_eq!(finger(&moved, 1), (true, true, 1, 500, 250));
    assert_eq!(scan(&moved), 20);

    // Lifting one reports it once with the tip off, then only the other.
    pad.apply(&[contact(2, 40, 0.1, 0.2)], 0, t(3));
    let lifted = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(count(&lifted), 2);
    assert_eq!(finger(&lifted, 0), (true, false, 0, 100, 100));
    assert_eq!(finger(&lifted, 1).2, 1);
    pad.apply(&[contact(3, 41, 0.6, 0.5)], 0, t(4));
    assert_eq!(count(&pad.interrupt(ENDPOINT).unwrap()), 1);
    assert_eq!(pad.fingers(), 1);

    // A new finger takes the free slot.
    pad.apply(&[contact(1, 99, 0.0, 0.0)], 0, t(5));
    let report = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(finger(&report, 0).2, 0);
    assert_eq!(finger(&report, 1).2, 1);
}

#[test]
fn a_cancel_is_not_a_touch_and_cancel_all_lifts_everything() {
    let (mut pad, epoch) = active(100, 50);
    pad.apply(
        &[contact(1, 1, 0.5, 0.5), contact(1, 2, 0.6, 0.6)],
        0,
        epoch,
    );
    pad.interrupt(ENDPOINT);
    pad.apply(&[contact(4, 1, 0.5, 0.5)], 0, epoch);
    let report = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(finger(&report, 0), (false, false, 0, 500, 250));
    assert!(pad.release_all(epoch));
    let report = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(count(&report), 1);
    assert!(!finger(&report, 0).1);
    assert_eq!(pad.fingers(), 0);
    assert!(!pad.release_all(epoch), "nothing left to lift");
}

#[test]
fn the_click_rides_on_the_fingers_and_a_button_only_change_still_reports() {
    let (mut pad, epoch) = active(100, 50);
    pad.apply(&[contact(1, 1, 0.5, 0.5)], 1, epoch);
    let pressed = pad.interrupt(ENDPOINT).unwrap();
    assert!(click(&pressed));
    // A button-only packet (event 5) releases the click with the finger still down.
    pad.apply(&[contact(5, 1, 0.5, 0.5)], 0, epoch);
    let released = pad.interrupt(ENDPOINT).unwrap();
    assert!(!click(&released));
    assert_eq!(count(&released), 1);
    // The same scan time twice in a row would read as one frame.
    assert_ne!(scan(&pressed), scan(&released));
}

#[test]
fn a_sixth_finger_has_no_slot_and_a_lost_down_is_recovered() {
    let (mut pad, epoch) = active(0, 0);
    let fingers: Vec<_> = (0..5).map(|n| contact(1, n, 0.1, 0.1)).collect();
    pad.apply(&fingers, 0, epoch);
    pad.interrupt(ENDPOINT);
    pad.apply(&[contact(1, 50, 0.9, 0.9)], 0, epoch);
    assert_eq!(pad.fingers(), 5);
    let report = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(count(&report), 5);
    assert!(
        (0..5).all(|n| finger(&report, n).3 == 105),
        "default 105 mm surface"
    );

    let (mut pad, epoch) = active(100, 50);
    pad.apply(&[contact(3, 9, 1.5, -1.0)], 0, epoch);
    assert_eq!(
        finger(&pad.interrupt(ENDPOINT).unwrap(), 0),
        (true, true, 0, 1000, 0)
    );
}

#[test]
fn the_surface_switch_off_sends_only_the_button() {
    let (mut pad, epoch) = active(100, 50);
    assert_eq!(
        pad.control(&setup(0x21, 9, 0x0307, 0, 2), &[7, 0b10]),
        Some(vec![])
    );
    assert!(!pad.apply(&[contact(1, 1, 0.5, 0.5)], 0, epoch));
    assert!(pad.apply(&[contact(3, 1, 0.5, 0.5)], 1, epoch));
    let report = pad.interrupt(ENDPOINT).unwrap();
    assert_eq!(count(&report), 0);
    assert!(click(&report));
    // Windows leaving touchpad mode drops what it has not read.
    pad.apply(&[contact(3, 1, 0.5, 0.5)], 0, epoch);
    pad.control(&setup(0x21, 9, 0x0303, 0, 2), &[3, 0]);
    assert!(!pad.interrupt_ready(ENDPOINT));
}

#[test]
fn the_mouse_reports_over_usbip_as_soon_as_it_moves() {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpStream};
    let export = crate::usbip::Export::listen(HidMouse::new("1"), POLL).unwrap();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, export.port())).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    // OP_REQ_IMPORT of bus 1-1.
    let mut request = vec![0x01, 0x11, 0x80, 0x03, 0, 0, 0, 0];
    let mut busid = [0u8; 32];
    busid[..3].copy_from_slice(b"1-1");
    request.extend_from_slice(&busid);
    client.write_all(&request).unwrap();
    let mut imported = [0u8; 8 + 312];
    client.read_exact(&mut imported).unwrap();
    assert_eq!(&imported[8 + 300..8 + 304], &[0x12, 0x09, 0x52, 0xb1]);
    // CMD_SUBMIT: an 8-byte interrupt IN transfer on endpoint 1.
    let mut submit = vec![];
    for n in [1u32, 1, 0x0001_0002, 1, 1, 0, 8, 0, 0, 0] {
        submit.extend_from_slice(&n.to_be_bytes());
    }
    submit.extend_from_slice(&[0; 8]);
    client.write_all(&submit).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(30)))
        .unwrap();
    let mut byte = [0u8; 1];
    assert!(
        client.peek(&mut byte).is_err(),
        "an idle mouse sends nothing"
    );
    export.with(|mouse| mouse.motion(-5, 7));
    export.changed();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut header = [0u8; 48];
    client.read_exact(&mut header).unwrap();
    assert_eq!(&header[..4], &3u32.to_be_bytes(), "RET_SUBMIT");
    assert_eq!(&header[24..28], &8u32.to_be_bytes());
    let mut data = [0u8; 8];
    client.read_exact(&mut data).unwrap();
    assert_eq!(relative(&data), (0, -5, 7, 0, 0));
}
