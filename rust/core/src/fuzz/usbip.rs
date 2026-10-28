//! The USB/IP server the host exports the Steam Deck controller with, driven over loopback.
//! Any frames a client sends leave it serving the next client; well-formed frames are each
//! answered at most once, with replies that fit what was asked, and leave the connection
//! answering.

use crate::steam_deck::{REPORT_PERIOD, SteamDeck};
use crate::usbip::Export;
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

const CMD_SUBMIT: u32 = 1;
const CMD_UNLINK: u32 = 2;
const RET_SUBMIT: u32 = 3;
const RET_UNLINK: u32 = 4;
const DEVICE: u32 = 0x0001_0002;
/// GET_DESCRIPTOR(device), 18 bytes.
const GET_DEVICE: [u8; 8] = [0x80, 6, 0, 1, 0, 0, 18, 0];

#[derive(Debug, Clone)]
enum Frame {
    Submit {
        inward: bool,
        endpoint: u32,
        length: u32,
        setup: [u8; 8],
        /// Isochronous packets as (offset, length); none for other transfers.
        iso: Option<Vec<(u32, u32)>>,
    },
    /// Unlinks the submit this many frames back, or an unknown one.
    Unlink(usize),
    /// A raw header, then raw bytes.
    Junk([u8; 48], Vec<u8>),
}

fn be(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_be_bytes()).collect()
}

/// The frames' bytes, with seqnums from `first`, and each submit's seqnum and direction.
fn encode(frames: &[Frame], first: u32) -> (Vec<u8>, BTreeMap<u32, bool>) {
    let mut wire = vec![];
    let mut sent = BTreeMap::new();
    for (i, frame) in frames.iter().enumerate() {
        let seqnum = first + i as u32;
        match frame {
            Frame::Submit {
                inward,
                endpoint,
                length,
                setup,
                iso,
            } => {
                let packets = iso.as_ref().map_or(u32::MAX, |p| p.len() as u32);
                wire.extend(be(&[
                    CMD_SUBMIT,
                    seqnum,
                    DEVICE,
                    u32::from(*inward),
                    *endpoint,
                    0,
                    *length,
                    0,
                    packets,
                    0,
                ]));
                wire.extend_from_slice(setup);
                if !inward {
                    wire.resize(wire.len() + *length as usize, 0xa5);
                }
                for &(offset, size) in iso.iter().flatten() {
                    wire.extend(be(&[offset, size, 0, 0]));
                }
                sent.insert(seqnum, *inward);
            }
            Frame::Unlink(back) => {
                let victim = seqnum.wrapping_sub(*back as u32);
                let mut b = be(&[CMD_UNLINK, seqnum, DEVICE, 0, 0, victim]);
                b.resize(48, 0);
                wire.extend(b);
            }
            Frame::Junk(header, rest) => {
                wire.extend_from_slice(header);
                wire.extend_from_slice(rest);
            }
        }
    }
    (wire, sent)
}

fn setup() -> impl Strategy<Value = [u8; 8]> {
    prop_oneof![
        Just(GET_DEVICE),
        // GET_DESCRIPTOR of any type and index, of any length.
        (any::<[u8; 2]>(), any::<u16>()).prop_map(|(v, l)| {
            let l = l.to_le_bytes();
            [0x80, 6, v[0], v[1], 0, 0, l[0], l[1]]
        }),
        // SET_CONFIGURATION, which resets the device's transfers.
        Just([0, 9, 1, 0, 0, 0, 0, 0]),
        any::<[u8; 8]>(),
    ]
}

/// Frames the server should take: no isochronous transfers, endpoints and lengths in range.
fn valid_frame() -> impl Strategy<Value = Frame> {
    prop_oneof![
        4 => (any::<bool>(), prop_oneof![Just(0u32), Just(1), Just(3), 0u32..16], 0u32..512, setup())
            .prop_map(|(inward, endpoint, length, setup)| Frame::Submit {
                inward,
                endpoint,
                length,
                setup,
                iso: None,
            }),
        1 => (0usize..8).prop_map(Frame::Unlink),
    ]
}

fn frame() -> impl Strategy<Value = Frame> {
    let iso = proptest::collection::vec((0u32..600, 0u32..600), 1..6);
    prop_oneof![
        4 => valid_frame(),
        1 => (any::<bool>(), 0u32..16, 0u32..512, setup(), iso).prop_map(
            |(inward, endpoint, length, setup, iso)| Frame::Submit {
                inward,
                endpoint,
                length,
                setup,
                iso: Some(iso),
            }
        ),
        1 => (any::<[u8; 48]>(), proptest::collection::vec(any::<u8>(), 0..64))
            .prop_map(|(h, rest)| Frame::Junk(h, rest)),
        // Valid commands with a bad direction, endpoint, length or packet count.
        1 => (0usize..4, any::<u32>()).prop_map(|(field, value)| {
            let mut h = be(&[CMD_SUBMIT, 1, DEVICE, 1, 0, 0, 18, 0, u32::MAX, 0]);
            h.extend_from_slice(&GET_DEVICE);
            let at = [12, 16, 24, 32][field];
            h[at..at + 4].copy_from_slice(&value.to_be_bytes());
            Frame::Junk(h.try_into().unwrap(), vec![])
        }),
    ]
}

fn import(port: u16) -> TcpStream {
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = be(&[0x0111_8003, 0]);
    request.extend_from_slice(b"1-1");
    request.resize(8 + 32, 0);
    client.write_all(&request).unwrap();
    let mut reply = [0u8; 8 + 312];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[..8], &be(&[0x0111_0003, 0])[..]);
    client
}

/// Reads replies until `want` is answered, checking each against what was sent; returns the
/// answer's status and data.
fn answer(
    client: &mut TcpStream,
    sent: &BTreeMap<u32, bool>,
    lengths: &BTreeMap<u32, u32>,
    want: u32,
) -> Result<(i32, Vec<u8>), TestCaseError> {
    let mut answered = BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        prop_assert!(Instant::now() < deadline, "no answer to {}", want);
        let mut h = [0u8; 48];
        client.read_exact(&mut h).unwrap();
        let word = |at: usize| u32::from_be_bytes(h[at..at + 4].try_into().unwrap());
        let (command, seqnum, status) = (word(0), word(4), word(20) as i32);
        prop_assert!(
            answered.insert((command, seqnum), ()).is_none(),
            "{} answered twice",
            seqnum
        );
        match command {
            RET_SUBMIT => {
                let inward = sent.get(&seqnum).copied();
                prop_assert!(inward.is_some(), "a reply to {}, never submitted", seqnum);
                prop_assert_eq!(word(32), u32::MAX, "not isochronous");
                let length = word(24);
                prop_assert!(length <= lengths[&seqnum], "more than asked for");
                let mut data = vec![
                    0;
                    if inward == Some(true) {
                        length as usize
                    } else {
                        0
                    }
                ];
                client.read_exact(&mut data).unwrap();
                if seqnum == want {
                    return Ok((status, data));
                }
            }
            RET_UNLINK => prop_assert!(!sent.contains_key(&seqnum)),
            _ => prop_assert!(false, "unknown reply {}", command),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever a client sends, the server drops at most that client and serves the next.
    #[test]
    fn any_frames_leave_the_device_attachable(frames in proptest::collection::vec(frame(), 0..24)) {
        let export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
        let mut client = import(export.port());
        let (wire, _) = encode(&frames, 1);
        // The server may close the connection part way.
        let _ = client.write_all(&wire);
        drop(client);

        let mut client = import(export.port());
        let (wire, sent) = encode(&[Frame::Submit { inward: true, endpoint: 0, length: 18, setup: GET_DEVICE, iso: None }], 1);
        client.write_all(&wire).unwrap();
        let (status, data) = answer(&mut client, &sent, &BTreeMap::from([(1, 18)]), 1)?;
        prop_assert_eq!(status, 0);
        prop_assert_eq!(&data[8..12], &[0xde, 0x28, 0x05, 0x12]);
    }

    /// Well-formed frames keep the connection: each is answered at most once and within what
    /// it asked for, and a descriptor request after them is answered.
    #[test]
    fn well_formed_frames_keep_the_connection(frames in proptest::collection::vec(valid_frame(), 0..32)) {
        let export = Export::listen(SteamDeck::new("TEST"), REPORT_PERIOD).unwrap();
        let mut client = import(export.port());
        let mut frames = frames;
        frames.push(Frame::Submit { inward: true, endpoint: 0, length: 18, setup: GET_DEVICE, iso: None });
        let (wire, sent) = encode(&frames, 1);
        let lengths = frames
            .iter()
            .enumerate()
            .filter_map(|(i, f)| match f {
                Frame::Submit { length, .. } => Some((1 + i as u32, *length)),
                _ => None,
            })
            .collect();
        client.write_all(&wire).unwrap();
        let want = frames.len() as u32;
        let (status, data) = answer(&mut client, &sent, &lengths, want)?;
        prop_assert_eq!(status, 0);
        prop_assert_eq!(&data[8..12], &[0xde, 0x28, 0x05, 0x12]);
        prop_assert!(export.imported());
    }
}
