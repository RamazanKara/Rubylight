use super::*;
use std::net::{Ipv4Addr, SocketAddrV4};

// Reference packets, built byte by byte from the layout of DS4Windows' DSU
// server (UdpServer.cs) with an independent CRC-32 (zlib's).
const VERSION_REQUEST: [u8; 20] = [
    0x44, 0x53, 0x55, 0x43, 0xe9, 0x03, 0x04, 0x00, 0xc9, 0xbc, 0x7b, 0xb7, 0x78, 0x56, 0x34, 0x12,
    0x00, 0x00, 0x10, 0x00,
];
const PORTS_REQUEST: [u8; 26] = [
    0x44, 0x53, 0x55, 0x43, 0xe9, 0x03, 0x0a, 0x00, 0x06, 0x75, 0xc8, 0xe2, 0x78, 0x56, 0x34, 0x12,
    0x01, 0x00, 0x10, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
];
const PAD_DATA_REQUEST: [u8; 28] = [
    0x44, 0x53, 0x55, 0x43, 0xe9, 0x03, 0x0c, 0x00, 0x38, 0x37, 0x64, 0x71, 0x78, 0x56, 0x34, 0x12,
    0x02, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const PAD_DATA_SLOT_REQUEST: [u8; 28] = [
    0x44, 0x53, 0x55, 0x43, 0xe9, 0x03, 0x0c, 0x00, 0x8f, 0x26, 0x51, 0x2a, 0x78, 0x56, 0x34, 0x12,
    0x02, 0x00, 0x10, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const VERSION_REPLY: [u8; 24] = [
    0x44, 0x53, 0x55, 0x53, 0xe9, 0x03, 0x08, 0x00, 0x6a, 0x05, 0xe9, 0xce, 0xd4, 0xc3, 0xb2, 0xa1,
    0x00, 0x00, 0x10, 0x00, 0xe9, 0x03, 0x00, 0x00,
];
/// Slot 1: connected, full gyro, USB, MAC 00:00:00:00:00:02, charging.
const PORT_INFO_SLOT1: [u8; 32] = [
    0x44, 0x53, 0x55, 0x53, 0xe9, 0x03, 0x10, 0x00, 0xbc, 0xd3, 0xac, 0x1f, 0xd4, 0xc3, 0xb2, 0xa1,
    0x01, 0x00, 0x10, 0x00, 0x01, 0x02, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xee, 0x00,
];
const PORT_INFO_EMPTY_SLOT3: [u8; 32] = [
    0x44, 0x53, 0x55, 0x53, 0xe9, 0x03, 0x10, 0x00, 0x90, 0x3a, 0x0c, 0x7b, 0xd4, 0xc3, 0xb2, 0xa1,
    0x01, 0x00, 0x10, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// Slot 0, packet 7: Cross, Up and L1 held, right trigger full, left stick
/// up and right, one touch at (0.5, 0.25), lying flat, gyro (10, -20, 30)
/// deg/s at 123456 us, battery 80 % discharging.
const PAD_DATA: [u8; 100] = [
    0x44, 0x53, 0x55, 0x53, 0xe9, 0x03, 0x54, 0x00, 0x78, 0x23, 0x00, 0x2c, 0xd4, 0xc3, 0xb2, 0xa1,
    0x02, 0x00, 0x10, 0x00, 0x00, 0x02, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01,
    0x07, 0x00, 0x00, 0x00, 0x10, 0x46, 0x00, 0x00, 0xff, 0xff, 0x80, 0x80, 0x00, 0x00, 0x00, 0xff,
    0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x01, 0x00, 0xc0, 0x03, 0xec, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x40, 0xe2, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
    0x00, 0x00, 0x80, 0xbf, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x20, 0x41, 0x00, 0x00, 0xa0, 0xc1,
    0x00, 0x00, 0xf0, 0x41,
];

const CLIENT: u32 = 0x1234_5678;
const SERVER: u32 = 0xA1B2_C3D4;

fn address(port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

/// A client packet with `payload` after the header, its CRC set.
fn request(payload: &[u8]) -> Vec<u8> {
    let mut out = b"DSUC".to_vec();
    out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&CLIENT.to_le_bytes());
    out.extend_from_slice(payload);
    let crc = checksum(&out);
    out[8..12].copy_from_slice(&crc.to_le_bytes());
    out
}

#[test]
fn client_requests_decode_from_the_reference_packets() {
    assert_eq!(parse(&VERSION_REQUEST), Some((CLIENT, Request::Version)));
    assert_eq!(
        parse(&PORTS_REQUEST),
        Some((
            CLIENT,
            Request::Ports {
                count: 2,
                slots: [0, 1, 0, 0]
            }
        ))
    );
    assert_eq!(
        parse(&PAD_DATA_REQUEST),
        Some((
            CLIENT,
            Request::PadData {
                flags: 0,
                slot: 0,
                mac: [0; 6]
            }
        ))
    );
    assert_eq!(
        parse(&PAD_DATA_SLOT_REQUEST),
        Some((
            CLIENT,
            Request::PadData {
                flags: 1,
                slot: 2,
                mac: [0; 6]
            }
        ))
    );
    // Bytes after the stated length are ignored, as DS4Windows does.
    let mut longer = VERSION_REQUEST.to_vec();
    longer.extend_from_slice(&[0xAA; 8]);
    assert_eq!(parse(&longer), Some((CLIENT, Request::Version)));
    // An older protocol version is answered too.
    let mut older = VERSION_REQUEST;
    older[4] = 0xE8;
    let crc = checksum(&older);
    older[8..12].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(parse(&older), Some((CLIENT, Request::Version)));
}

#[test]
fn packets_a_dsu_server_drops_are_refused() {
    let mut crc = VERSION_REQUEST;
    crc[9] ^= 1;
    assert_eq!(parse(&crc), None, "bad CRC");
    let mut body = VERSION_REQUEST;
    body[19] ^= 0x40;
    assert_eq!(parse(&body), None, "the CRC no longer matches");
    assert_eq!(parse(&VERSION_REPLY), None, "a server's packet");
    let mut newer = VERSION_REQUEST;
    newer[4] = 0xEA;
    let crc = checksum(&newer);
    newer[8..12].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(parse(&newer), None, "protocol 1002");
    assert_eq!(parse(&VERSION_REQUEST[..19]), None, "short");
    assert_eq!(
        parse(&PORTS_REQUEST[..25]),
        None,
        "the length says 10 bytes follow"
    );
    // Five ports, a slot past the four, or a list longer than the packet.
    for (count, slots) in [
        (5i32, vec![0u8, 1, 2, 3, 0]),
        (1, vec![4]),
        (3, vec![0, 1]),
        (-1, vec![]),
    ] {
        let mut payload = MSG_PORTS.to_le_bytes().to_vec();
        payload.extend_from_slice(&count.to_le_bytes());
        payload.extend_from_slice(&slots);
        assert_eq!(parse(&request(&payload)), None, "{count} {slots:?}");
    }
    let mut short = MSG_PAD_DATA.to_le_bytes().to_vec();
    short.extend_from_slice(&[0; 7]);
    assert_eq!(
        parse(&request(&short)),
        None,
        "pad data request without a whole MAC"
    );
    assert_eq!(
        parse(&request(&0x20_0000u32.to_le_bytes())),
        None,
        "message 0x200000"
    );
}

#[test]
fn server_replies_match_the_reference_packets() {
    assert_eq!(version_reply(SERVER), VERSION_REPLY);
    let charging = Pad {
        battery: (3, 50),
        ..Pad::default()
    };
    assert_eq!(port_info(SERVER, 1, Some(&charging)), PORT_INFO_SLOT1);
    assert_eq!(port_info(SERVER, 3, None), PORT_INFO_EMPTY_SLOT3);
}

#[test]
fn pad_data_matches_the_reference_packet() {
    let mut pad = Pad::default();
    let inputs = [
        Input::Controller {
            id: 0,
            active: 1,
            buttons: button::A | button::UP | button::LB,
            left_trigger: 0,
            right_trigger: 255,
            sticks: [32767, 32767, 0, 0],
        },
        Input::ControllerTouch {
            id: 0,
            event: 1,
            touchpad: 0,
            pointer: 9,
            x: 0.5,
            y: 0.25,
            pressure: 1.,
        },
        Input::Motion {
            id: 0,
            kind: 1,
            xyz: [0., 9.806_65, 0.],
        },
        Input::Motion {
            id: 0,
            kind: 2,
            xyz: [10., 20., -30.],
        },
        Input::Battery {
            id: 0,
            state: 2,
            percent: 80,
        },
    ];
    for input in &inputs {
        pad.apply(input, 123_456);
    }
    let data = pad_data(SERVER, 0, &pad, 7);
    assert_eq!(data.len(), PAD_DATA_BYTES);
    assert_eq!(data, PAD_DATA);
}

#[test]
fn motion_reaches_dsu_axes_and_units() {
    let mut pad = Pad::default();
    // Lying flat: SDL reads +1 g on Y, DSU -1 g.
    assert_eq!(pad.dsu_accel(), [-0., -1., -0.]);
    pad.accel = [9.806_65, 0., -9.806_65];
    assert_eq!(pad.dsu_accel(), [-1., 0., 1.]);
    pad.gyro = [90., -45., 180.];
    assert_eq!(pad.dsu_gyro(), [90., 45., -180.]);
    // A motion reading stamps the pad; other input leaves the stamp.
    pad.apply(
        &Input::Motion {
            id: 0,
            kind: 2,
            xyz: [0.; 3],
        },
        42,
    );
    pad.apply(
        &Input::Battery {
            id: 0,
            state: 2,
            percent: 50,
        },
        99,
    );
    assert_eq!(pad.motion_us, 42);
}

#[test]
fn battery_levels_follow_dsu() {
    let level = |state, percent| {
        Pad {
            battery: (state, percent),
            ..Pad::default()
        }
        .battery_status()
    };
    assert_eq!(level(0, 0xFF), 0);
    assert_eq!(level(1, 50), 0, "no battery");
    assert_eq!(level(2, 0xFF), 0);
    assert_eq!(level(2, 5), 1);
    assert_eq!(level(2, 30), 2);
    assert_eq!(level(4, 60), 3);
    assert_eq!(level(2, 99), 4);
    assert_eq!(level(2, 100), 5);
    assert_eq!(level(3, 10), 0xEE);
    assert_eq!(level(5, 100), 0xEF);
}

#[test]
fn touches_keep_their_contact_and_take_new_ids() {
    let mut pad = Pad::default();
    let touch = |pad: &mut Pad, event, pointer, x| {
        pad.apply(
            &Input::ControllerTouch {
                id: 0,
                event,
                touchpad: 0,
                pointer,
                x,
                y: 0.5,
                pressure: 1.,
            },
            0,
        )
    };
    assert!(touch(&mut pad, 1, 5, 0.1));
    assert!(touch(&mut pad, 1, 6, 0.9));
    assert!(!touch(&mut pad, 1, 7, 0.5), "a third contact has no place");
    assert!(touch(&mut pad, 3, 5, 0.2));
    assert_eq!(pad.touches[0].map(|t| (t.id, t.x)), Some((0, 0.2)));
    assert!(touch(&mut pad, 2, 5, 0.));
    assert!(touch(&mut pad, 1, 8, 0.3));
    assert_eq!(pad.touches[0].map(|t| t.id), Some(2));
    assert!(touch(&mut pad, 7, 0, 0.));
    assert_eq!(pad.touches, [None, None]);
}

#[test]
fn controllers_take_free_slots_and_leave_them() {
    let mut slots = Slots::default();
    let arrival = |id| Input::Arrival {
        id,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    };
    let state = |id: u16, active: u16| Input::Controller {
        id,
        active,
        buttons: button::B,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    assert_eq!(slots.apply("phone", &arrival(0), 0), Some(0));
    assert_eq!(slots.apply("laptop", &arrival(0), 0), Some(1));
    assert_eq!(slots.apply("phone", &state(1, 0b11), 0), Some(2));
    assert_eq!(slots.pad(2).unwrap().buttons, button::B);
    assert_eq!(slots.apply("phone", &state(1, 0b11), 0), None, "no change");
    assert_eq!(slots.apply("phone", &Input::Haptics(true), 0), None);
    // Controller 1 left the phone's active mask: its slot is free again.
    assert_eq!(slots.apply("phone", &state(1, 0b01), 0), Some(2));
    assert_eq!(slots.pad(2), None);
    assert_eq!(
        slots.apply("phone", &state(1, 0b01), 0),
        None,
        "already gone"
    );
    assert_eq!(slots.apply("tv", &arrival(3), 0), Some(2));
    assert_eq!(slots.apply("tv", &arrival(2), 0), Some(3));
    assert_eq!(slots.apply("tv", &arrival(1), 0), None, "all four taken");
    assert_eq!(slots.forget("tv"), vec![2, 3]);
    assert_eq!(slots.pad(3), None);
    assert!(slots.pad(1).is_some());
    assert_eq!((slots.next_number(1), slots.next_number(1)), (1, 2));
}

#[test]
fn clients_get_what_they_asked_for_until_they_stop_asking() {
    let mut clients = Clients::default();
    let t0 = Instant::now();
    clients.register(address(1), 0, 0, [0; 6], t0);
    clients.register(address(2), 1, 2, [0; 6], t0);
    clients.register(address(3), 2, 0, mac(3), t0);
    clients.register(address(4), 1, 9, [0; 6], t0);
    let sorted = |mut v: Vec<SocketAddr>| {
        v.sort();
        v
    };
    assert_eq!(sorted(clients.subscribers(0, t0)), vec![address(1)]);
    assert_eq!(
        sorted(clients.subscribers(2, t0)),
        vec![address(1), address(2)]
    );
    assert_eq!(
        sorted(clients.subscribers(3, t0)),
        vec![address(1), address(3)]
    );
    // Slot 9 does not exist: that client registered nothing and was dropped.
    assert!(!clients.subscribers(1, t0).contains(&address(4)));
    let later = t0 + Duration::from_secs(3);
    clients.register(address(2), 1, 2, [0; 6], later);
    let expired = t0 + CLIENT_TIMEOUT;
    assert_eq!(clients.subscribers(2, expired), vec![address(2)]);
    assert!(clients.subscribers(0, expired).is_empty());
    clients.prune(later + CLIENT_TIMEOUT);
    assert!(clients.is_empty());
}
