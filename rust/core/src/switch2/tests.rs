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
fn controller(buttons: u32, left: u8, right: u8, sticks: [i16; 4]) -> Input {
    Input::Controller {
        id: 0,
        active: 1,
        buttons,
        left_trigger: left,
        right_trigger: right,
        sticks,
    }
}
/// A bulk command as SDL 3 sends it.
fn command(cmd: u8, sub: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![cmd, 0x91, 0x00, sub, 0x00, payload.len() as u8, 0x00, 0x00];
    out.extend_from_slice(payload);
    out
}
/// Sends `out` and reads the whole reply in 64-byte reads, as SDL's
/// `RecvBulkData` does.
fn exchange(pad: &mut Switch2Pro, out: &[u8]) -> Vec<u8> {
    assert!(pad.output(BULK_ENDPOINT, out));
    let mut reply = vec![];
    while let Some(chunk) = pad.transfer_in(BULK_ENDPOINT, 64) {
        let short = chunk.len() < 64;
        reply.extend(chunk);
        if short {
            break;
        }
    }
    reply
}
/// SDL's `ParseStickCalibration`: neutral, travel up, travel down.
fn calibration(d: &[u8]) -> [u16; 6] {
    let a = |lo: u8, hi: u8| u16::from(lo) | (u16::from(hi & 0x0F) << 8);
    let b = |lo: u8, hi: u8| u16::from(lo >> 4) | (u16::from(hi) << 4);
    [
        a(d[0], d[1]),
        b(d[1], d[2]),
        a(d[3], d[4]),
        b(d[4], d[5]),
        a(d[6], d[7]),
        b(d[7], d[8]),
    ]
}
/// SDL's 12-bit stick reads of report 0x05.
fn sticks(r: &[u8]) -> [u16; 4] {
    [
        u16::from(r[11]) | (u16::from(r[12] & 0x0F) << 8),
        u16::from(r[12] >> 4) | (u16::from(r[13]) << 4),
        u16::from(r[14]) | (u16::from(r[15] & 0x0F) << 8),
        u16::from(r[15] >> 4) | (u16::from(r[16]) << 4),
    ]
}
fn word(r: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([r[at], r[at + 1]])
}
fn long(r: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(r[at..at + 4].try_into().unwrap())
}

/// SDL 3's USB initialisation of a Switch 2 Pro Controller, as
/// `HIDAPI_DriverSwitch2_InitUSB` sends it.
fn sdl_init(pad: &mut Switch2Pro) {
    for address in [
        0x13000u32, 0x13040, 0x13080, 0x130C0, 0x13100, 0x1FC040, 0x1FC080,
    ] {
        let mut read = command(0x02, 0x01, &[0; 8]);
        read[12..16].copy_from_slice(&address.to_le_bytes());
        let reply = exchange(pad, &read);
        assert_eq!(reply.len(), 0x50, "flash {address:#x}");
    }
    let sequence: [&[u8]; 10] = [
        &command(0x07, 0x01, &[]),
        &command(0x0c, 0x02, &[0x27, 0, 0, 0]),
        &command(0x11, 0x01, &[]),
        &command(
            0x0a,
            0x08,
            &[
                0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x35, 0x00, 0x46, 0, 0, 0, 0,
                0, 0, 0, 0,
            ],
        ),
        &command(0x0c, 0x04, &[0x27, 0, 0, 0]),
        &command(0x01, 0x0c, &[]),
        &command(0x01, 0x01, &[]),
        &command(0x08, 0x02, &[0x01, 0, 0, 0]),
        &command(0x03, 0x0a, &[0x05, 0, 0, 0]),
        &command(0x03, 0x0d, &[0x01, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
    ];
    for out in sequence {
        let reply = exchange(pad, out);
        // Every command is answered, with its command and subcommand.
        assert!(reply.len() >= 8);
        assert_eq!((reply[0], reply[1], reply[3]), (out[0], 0x01, out[3]));
    }
}

#[test]
fn the_device_is_a_switch2_pro_with_a_hid_and_a_winusb_bulk_interface() {
    let mut pad = Switch2Pro::new("BUTTERPOLLO3");
    let description = pad.describe();
    assert_eq!((description.vendor, description.product), (0x057E, 0x2069));
    assert_eq!(description.interfaces, vec![(3, 0, 0), (0xFF, 0, 0)]);
    let device = pad.control(&setup(0x80, 6, 0x0100, 0, 18), &[]).unwrap();
    assert_eq!(&device[8..12], &[0x7E, 0x05, 0x69, 0x20]);
    let config = pad.control(&setup(0x80, 6, 0x0200, 0, 255), &[]).unwrap();
    assert_eq!(
        usize::from(u16::from_le_bytes([config[2], config[3]])),
        config.len()
    );
    // Endpoints: interrupt 0x81 and 0x01, then bulk 0x02 and 0x82.
    let endpoints: Vec<(u8, u8)> = config
        .windows(7)
        .filter(|d| d[0] == 7 && d[1] == 5)
        .map(|d| (d[2], d[3]))
        .collect();
    assert_eq!(endpoints, [(0x81, 3), (0x01, 3), (0x02, 2), (0x82, 2)]);
    let report = pad.control(&setup(0x81, 6, 0x2200, 0, 255), &[]).unwrap();
    assert_eq!(report.len(), 97);
    assert_eq!(report.last(), Some(&0xC0));
    let serial = pad
        .control(&setup(0x80, 6, 0x0303, 0x0409, 255), &[])
        .unwrap();
    assert_eq!(serial, string_descriptor("BUTTERPOLLO3"));
    // Microsoft OS 1.0: the string names vendor code 0x20, which then gives
    // WinUSB for interface 1 and its interface GUID.
    let os = pad.control(&setup(0x80, 6, 0x03EE, 0, 255), &[]).unwrap();
    assert_eq!(os.len(), 18);
    assert_eq!(os[16], 0x20);
    let compatible = pad.control(&setup(0xC0, 0x20, 0, 4, 40), &[]).unwrap();
    assert_eq!((compatible[16], &compatible[18..24]), (1, &b"WINUSB"[..]));
    let properties = pad.control(&setup(0xC1, 0x20, 0, 5, 255), &[]).unwrap();
    assert_eq!(long(&properties, 0) as usize, properties.len());
}

#[test]
fn sdl_init_reads_the_serial_and_full_range_stick_calibration() {
    let mut pad = Switch2Pro::new("BUTTERPOLLO3");
    let mut read = command(0x02, 0x01, &[0; 8]);
    read[12..16].copy_from_slice(&0x13000u32.to_le_bytes());
    let reply = exchange(&mut pad, &read);
    assert_eq!(&reply[18..30], b"BUTTERPOLLO3");
    assert_eq!(long(&reply, 12), 0x13000);
    read[12..16].copy_from_slice(&0x13080u32.to_le_bytes());
    let reply = exchange(&mut pad, &read);
    assert_eq!(
        calibration(&reply[0x10 + 0x28..]),
        [2048, 2048, 2047, 2047, 2048, 2048]
    );
    // The user calibration lacks its magic, so SDL keeps the factory values.
    read[12..16].copy_from_slice(&0x1FC040u32.to_le_bytes());
    let reply = exchange(&mut pad, &read);
    assert_ne!(&reply[0x10..0x12], &[0xb2, 0xa1]);
}

#[test]
fn a_reply_longer_than_the_read_comes_a_transfer_at_a_time() {
    let mut pad = Switch2Pro::new("X");
    let mut read = command(0x02, 0x01, &[0; 8]);
    read[12..16].copy_from_slice(&0x13000u32.to_le_bytes());
    assert!(pad.output(BULK_ENDPOINT, &read));
    assert_eq!(pad.transfer_in(BULK_ENDPOINT, 64).unwrap().len(), 64);
    assert_eq!(pad.transfer_in(BULK_ENDPOINT, 64).unwrap().len(), 16);
    assert_eq!(pad.transfer_in(BULK_ENDPOINT, 64), None);
    // A larger read takes the whole reply at once.
    assert!(pad.output(BULK_ENDPOINT, &read));
    assert_eq!(pad.transfer_in(BULK_ENDPOINT, 512).unwrap().len(), 80);
    assert_eq!(pad.transfer_in(BULK_ENDPOINT, 512), None);
}

#[test]
fn reports_start_only_once_a_driver_starts_them_in_the_format_it_chose() {
    let mut pad = Switch2Pro::new("X");
    assert_eq!(pad.transfer_in(HID_ENDPOINT, 64), None);
    assert!(pad.retries_held());
    sdl_init(&mut pad);
    assert!(pad.reporting());
    let report = pad.transfer_in(HID_ENDPOINT, 64).unwrap();
    assert_eq!((report.len(), report[0]), (64, 0x05));
    assert_eq!(long(&report, 1), 1);
    // The IMU is on (feature 0x04 among 0x27): motion is timestamped.
    assert_ne!(long(&report, 0x2B), 0);
}

#[test]
fn buttons_sticks_and_triggers_land_where_sdl_reads_them() {
    let mut state = State::default();
    // Moonlight's A (south) is Nintendo's B; X (west) is Y.
    assert!(state.apply(&controller(
        0x1000 | 0x4000 | 0x0001 | 0x0010 | 0x0400 | 0x0200,
        255,
        0,
        [32767, 32767, -32768, 0]
    )));
    let r = state.common_report(7, None);
    assert_eq!(long(&r, 1), 7);
    // data[5]: west (Y) 0x01, south (B) 0x04, R 0x40.
    assert_eq!(r[5], 0x01 | 0x04 | 0x40);
    // data[6]: start (+) 0x02, guide (Home) 0x10.
    assert_eq!(r[6], 0x02 | 0x10);
    // data[7]: up 0x02, ZL 0x80 from the left trigger.
    assert_eq!(r[7], 0x02 | 0x80);
    // Full deflection either way: 2048 ± 2047.
    assert_eq!(sticks(&r), [4095, 4095, 1, 2048]);
    assert!(!state.apply(&controller(
        0x1000 | 0x4000 | 0x0001 | 0x0010 | 0x0400 | 0x0200,
        255,
        0,
        [32767, 32767, -32768, 0]
    )));
}

#[test]
fn c_the_grips_and_capture_reach_both_reports() {
    let mut state = State::default();
    state.apply(&controller(
        BUTTON_C | 0x01_0000 | 0x02_0000 | 0x20_0000,
        0,
        31,
        [0; 4],
    ));
    let r = state.common_report(1, None);
    // C 0x40 and Capture 0x20 in data[6]; GR 0x01 and GL 0x02 in data[8].
    assert_eq!((r[6], r[8]), (0x40 | 0x20, 0x01 | 0x02));
    // A trigger below the threshold does not press ZR.
    assert_eq!(r[5], 0);
    let p = state.pro_report(1, true);
    assert_eq!(p[5], 0x02 | 0x04 | 0x08 | 0x10);
    assert_eq!(p[12], 0x38);
    // Full battery on external power: level 9, bit 0.
    assert_eq!(p[2], (9 << 2) | 1);
}

#[test]
fn motion_decodes_in_sdl_back_to_what_the_client_sent() {
    let mut state = State::default();
    state.apply(&Input::Motion {
        id: 0,
        kind: 1,
        xyz: [1.0, 9.80665, -2.0],
    });
    state.apply(&Input::Motion {
        id: 0,
        kind: 2,
        xyz: [90.0, -45.0, 10.0],
    });
    let r = state.common_report(1, Some(4000));
    // SDL: accel = (raw x, raw z, -raw y) * 8 g / 32767.
    let accel_scale = 9.80665 * 8.0 / 32767.0;
    let sdl_accel = [
        f32::from(word(&r, 0x31)) * accel_scale,
        f32::from(word(&r, 0x35)) * accel_scale,
        f32::from(word(&r, 0x33)) * -accel_scale,
    ];
    for (got, want) in sdl_accel.iter().zip([1.0, 9.80665, -2.0]) {
        assert!((got - want).abs() < 0.01, "{sdl_accel:?}");
    }
    // SDL: gyro = (raw x, raw z, -raw y) * 34.8 / 32767 rad/s.
    let gyro_scale = 34.8 / 32767.0 * 180.0 / std::f32::consts::PI;
    let sdl_gyro = [
        f32::from(word(&r, 0x37)) * gyro_scale,
        f32::from(word(&r, 0x3B)) * gyro_scale,
        f32::from(word(&r, 0x39)) * -gyro_scale,
    ];
    for (got, want) in sdl_gyro.iter().zip([90.0, -45.0, 10.0]) {
        assert!((got - want).abs() < 0.2, "{sdl_gyro:?}");
    }
    assert_eq!(long(&r, 0x2B), 4000);
}

#[test]
fn motion_timestamps_step_4_ms_while_sdl_calibrates_then_follow_the_clock() {
    let mut pad = Switch2Pro::new("X");
    sdl_init(&mut pad);
    assert!(!pad.immediate());
    let mut last = 0;
    for n in 1..=CALIBRATION_REPORTS {
        let r = pad.transfer_in(HID_ENDPOINT, 64).unwrap();
        last = long(&r, 0x2B);
        assert_eq!(last, 4000 * n);
    }
    assert!(pad.immediate());
    let next = long(&pad.transfer_in(HID_ENDPOINT, 64).unwrap(), 0x2B);
    assert!(next > last);
    let after = long(&pad.transfer_in(HID_ENDPOINT, 64).unwrap(), 0x2B);
    assert!(after > next);
    // Turning the IMU off and on calibrates again.
    exchange(&mut pad, &command(0x0c, 0x05, &[0x04, 0, 0, 0]));
    assert!(pad.immediate());
    exchange(&mut pad, &command(0x0c, 0x04, &[0x04, 0, 0, 0]));
    assert!(!pad.immediate());
}

#[test]
fn sdl_hd_rumble_becomes_motor_speeds() {
    /// SDL's `EncodeHDRumble`.
    fn encode(high_freq: u16, high_amp: u16, low_freq: u16, low_amp: u16) -> [u8; 5] {
        [
            high_freq as u8,
            (((high_amp >> 4) & 0xfc) as u8) | ((high_freq >> 8) & 0x03) as u8,
            ((high_amp >> 12) as u8) | ((low_freq << 4) as u8),
            ((low_amp & 0xc0) as u8) | (((low_freq >> 4) & 0x3f) as u8),
            (low_amp >> 8) as u8,
        ]
    }
    let mut report = [0u8; 64];
    report[0] = 0x02;
    report[1] = 0x50;
    report[2..7].copy_from_slice(&encode(0x187, 29000, 0x112, 14500));
    report.copy_within(1..7, 0x11);
    let mut pad = Switch2Pro::new("X");
    assert!(pad.output(HID_ENDPOINT, &report));
    let (low, high) = pad.take_rumble().unwrap();
    assert!(high > 65000, "{high}");
    assert!((32000..33500).contains(&low), "{low}");
    assert_eq!(pad.take_rumble(), None);
    // Silence stops both motors; the same report through SET_REPORT works too.
    let mut stop = [0u8; 64];
    stop[0] = 0x02;
    assert!(
        pad.control(&setup(0x21, 0x09, 0x0202, 0, 64), &stop)
            .is_some()
    );
    assert_eq!(pad.take_rumble(), Some((0, 0)));
    // A report of another shape is ignored.
    assert_eq!(rumble(&[0x03; 64]), None);
}

#[test]
fn battery_reports_follow_the_client() {
    let mut state = State::default();
    assert!(state.apply(&Input::Battery {
        id: 0,
        state: 3,
        percent: 50
    }));
    let r = state.common_report(1, None);
    assert_eq!(r[0x22], 0x34);
    assert_eq!(u16::from_le_bytes([r[0x20], r[0x21]]), 3800);
    assert_eq!(state.pro_report(1, false)[2], (4 << 2) | 0b11);
    state.apply(&Input::Battery {
        id: 0,
        state: 2,
        percent: 255,
    });
    assert_eq!(state.pro_report(1, false)[2], 9 << 2);
}

#[test]
fn player_lights_are_kept_for_the_host() {
    let mut pad = Switch2Pro::new("X");
    let reply = exchange(&mut pad, &command(0x09, 0x07, &[0x01, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(reply.len(), 8);
    assert_eq!(pad.take_player_leds(), Some(1));
}
