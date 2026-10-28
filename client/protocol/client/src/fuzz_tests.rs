//! Property tests over what the client reads from a host: the control stream and the USB/IP
//! driver's side of a forwarded device here, NvHTTP replies in `http::fuzz_tests` and RTSP in
//! `rtsp::fuzz_tests`. Mutated seeds and arbitrary bytes never panic or allocate
//! without bound, and every writer's output reads back as what was written. The default 256
//! cases each run in CI; `PROPTEST_CASES` raises the budget for a longer local run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use butterpollo_core::packet;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::{select, Index};
use rubylight_protocol::usbip::{self as wire, Base, Header, IsoPacket};

use crate::control::events::{self, CursorShape};
use crate::control::haptics;
use crate::control::negotiation::{encryption, HostCapabilities};
use crate::control::session::{ControlSession, SessionOptions};
use crate::control::wire::{ptype, OpenError, Opener, Sealer};
use crate::crypto::Key;
use crate::usb::{serve, Completion, Direction, Reply, Transfer, Tunnel, UsbDevice};

const KEY: Key = *b"0011223344556677";

/// `seed` with up to four bytes replaced, maybe cut short and maybe with bytes added, or
/// arbitrary bytes one time in five.
pub(crate) fn mutated(seed: Vec<u8>) -> impl Strategy<Value = Vec<u8>> {
    let edited = (
        vec((any::<Index>(), any::<u8>()), 0..4),
        prop_oneof![3 => Just(None), 1 => any::<Index>().prop_map(Some)],
        prop_oneof![3 => Just(Vec::new()), 1 => vec(any::<u8>(), 1..16)],
    )
        .prop_map(move |(edits, cut, tail)| {
            let mut bytes = seed.clone();
            if !bytes.is_empty() {
                for (at, byte) in edits {
                    let at = at.index(bytes.len());
                    bytes[at] = byte;
                }
                if let Some(cut) = cut {
                    bytes.truncate(cut.index(bytes.len()));
                }
            }
            bytes.extend(tail);
            bytes
        });
    prop_oneof![4 => edited, 1 => vec(any::<u8>(), 0..96)]
}

pub(crate) fn text(seed: &str) -> impl Strategy<Value = String> {
    mutated(seed.as_bytes().to_vec()).prop_map(|b| String::from_utf8_lossy(&b).into_owned())
}

// --- The control stream ---

fn rubylight() -> HostCapabilities {
    HostCapabilities {
        app_version: [7, 1, 431, -1],
        feature_flags: 3,
        encryption_enabled: encryption::CONTROL_V2,
        control_messages: vec![
            0x5530, 0x5531, 0x5532, 0x5533, 0x5534, 0x5535, 0x5536, 0x5537, 0x5538, 0x553D, 0x5540,
        ],
        client_roaming: true,
    }
}

/// Every message type the client reads, or any other.
fn message_type() -> impl Strategy<Value = u16> {
    prop_oneof![
        4 => select(vec![
            ptype::HDR_MODE,
            ptype::TERMINATION,
            ptype::RUMBLE,
            ptype::RUMBLE_TRIGGERS,
            ptype::SET_MOTION_EVENT,
            ptype::SET_RGB_LED,
            ptype::ADAPTIVE_TRIGGERS,
            ptype::STEAM_HAPTIC,
            ptype::STEAM_SETTINGS,
            ptype::SERVER_COMMAND,
            ptype::CONTROLLER_HAPTICS,
            ptype::CONTROLLER_SPEAKER,
            ptype::CONTROLLER_MICROPHONE,
            ptype::CONTROLLER_AUDIO_CONTROL,
            ptype::PHASE_LOCK_REPORT,
            ptype::DISPLAY_CAPS,
            ptype::RECONFIGURE,
            ptype::HOST_TIMING,
            ptype::CLOCK_SYNC,
            ptype::LINK_NOTICE,
            ptype::FEC_REPORT,
            ptype::TEXT_INPUT,
            ptype::PIPELINE_RATES,
            ptype::KEYBOARD_LAYOUT,
            ptype::RATE_CHANGE,
            ptype::CURSOR_SHAPE_REQUEST,
            ptype::CURSOR_SHAPE,
            ptype::USB,
        ]),
        1 => any::<u16>(),
    ]
}

/// Payloads around the lengths the decoders check, with small leading bytes for versions
/// and kinds.
fn payload() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        3 => (vec(0u8..4, 0..4), vec(any::<u8>(), 0..72)).prop_map(|(mut head, tail)| {
            head.extend(tail);
            head
        }),
        1 => vec(any::<u8>(), 0..1100),
    ]
}

proptest! {
    /// A session takes any run of messages the host sends, and any datagram at all.
    #[test]
    fn control_session_reads_anything(
        messages in vec((message_type(), payload()), 1..12),
        garbage in vec(vec(any::<u8>(), 0..64), 0..3),
        sunshine in any::<bool>(),
        reference_frame_invalidation in any::<bool>(),
    ) {
        let mut caps = rubylight();
        if !sunshine {
            caps.app_version = [7, 1, 431, 0];
        }
        let mut session = ControlSession::new(caps, &KEY, SessionOptions { reference_frame_invalidation });
        let t0 = Instant::now();
        session.start(t0);
        for (seq, (kind, payload)) in messages.iter().enumerate() {
            let now = t0 + Duration::from_millis(seq as u64 * 7);
            if let Ok(datagram) = packet::encrypted_control(&KEY, seq as u32, *kind, payload, true) {
                prop_assert!(session.handle_datagram(now, &datagram).is_ok());
            }
            session.poll(now);
            while session.poll_transmit().is_some() {}
            let _ = session.take_events();
        }
        // Nothing sealed with the key, so refused, unless a termination ended the session.
        let terminated = messages.iter().any(|(kind, _)| *kind == ptype::TERMINATION);
        for datagram in garbage {
            prop_assert!(terminated || session.handle_datagram(t0, &datagram).is_err());
        }
        let _ = session.take_events();
    }

    /// What the host seals opens to the same message; the opener rejects a replay and any
    /// datagram a changed byte broke. The legacy IV (no V2) takes only the sequence's low
    /// byte, as moonlight-common-c's does, so its other three bytes go unauthenticated.
    #[test]
    fn control_wire_round_trips(
        sequence in any::<u32>(),
        kind in any::<u16>(),
        payload in vec(any::<u8>(), 0..256),
        v2 in any::<bool>(),
        edit in prop_oneof![Just(None), (any::<Index>(), 1u8..).prop_map(Some)],
    ) {
        let mut datagram = packet::encrypted_control(&KEY, sequence, kind, &payload, v2).unwrap();
        let mut opener = Opener::new(&KEY, v2);
        match edit {
            None => {
                prop_assert_eq!(opener.open(&datagram), Ok((kind, payload)));
                prop_assert_eq!(opener.open(&datagram), Err(OpenError::Replay(sequence)));
            }
            Some((at, flip)) => {
                let at = at.index(datagram.len());
                datagram[at] ^= flip;
                match opener.open(&datagram) {
                    Err(_) => {}
                    Ok(opened) => {
                        prop_assert!(!v2 && (5..8).contains(&at), "byte {} changed", at);
                        prop_assert_eq!(opened, (kind, payload));
                    }
                }
            }
        }
    }

    /// What the client seals the host opens to the same message.
    #[test]
    fn control_sealer_round_trips(
        messages in vec((any::<u16>(), vec(any::<u8>(), 0..256)), 1..4),
        v2 in any::<bool>(),
    ) {
        let mut sealer = Sealer::new(&KEY, v2);
        for (sequence, (kind, payload)) in messages.into_iter().enumerate() {
            let datagram = sealer.seal(kind, &payload).unwrap();
            let opened = packet::decrypt_control(&KEY, &datagram, v2).unwrap();
            prop_assert_eq!(opened, (sequence as u32, kind, payload));
        }
    }

    #[test]
    fn opener_reads_anything(datagram in vec(any::<u8>(), 0..96), v2 in any::<bool>()) {
        prop_assert!(Opener::new(&KEY, v2).open(&datagram).is_err());
    }

    #[test]
    fn control_payloads_read_anything(p in payload(), saw_frame in any::<bool>()) {
        let _ = CursorShape::decode(&p);
        let _ = events::termination_error(&p, saw_frame);
        if let Some(pcm) = haptics::decode(&p) {
            prop_assert_eq!(haptics::encode(pcm.controller, pcm.sequence, &pcm.samples), Some(p));
        }
    }

    #[test]
    fn haptics_round_trip(controller in 0u16..20, sequence in any::<u32>(), samples in vec(any::<u8>(), 0..1000)) {
        if let Some(p) = haptics::encode(controller, sequence, &samples) {
            let pcm = haptics::decode(&p).unwrap();
            prop_assert_eq!((pcm.controller, pcm.sequence, pcm.samples), (controller, sequence, samples));
        }
    }
}

// --- USB/IP: the client serving a device to the host's driver ---

/// A tunnel over fixed bytes from the driver, keeping what the server writes back.
#[derive(Default)]
struct Script {
    input: Mutex<(Vec<u8>, usize, bool)>,
    output: Mutex<Vec<u8>>,
}

impl Tunnel for Script {
    fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let mut input = self.input.lock().unwrap();
        let (data, at, closed) = &mut *input;
        if *closed {
            return Ok(0);
        }
        let n = bytes.len().min(data.len() - *at).min(61);
        bytes[..n].copy_from_slice(&data[*at..*at + n]);
        *at += n;
        Ok(n)
    }

    fn write_all(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.output.lock().unwrap().extend_from_slice(bytes);
        Ok(())
    }

    fn close(&self) {
        self.input.lock().unwrap().2 = true;
    }
}

/// Answers by seqnum: completes at once, holds the transfer until it is unlinked or the
/// device closes, drops the completion, or answers with more than was asked.
#[derive(Default)]
struct Answering {
    held: Mutex<HashMap<u32, Completion>>,
    /// The transfers still owed an answer, in submit order: seqnum, direction and packets.
    asked: Mutex<Vec<(u32, Direction, usize)>>,
}

fn device() -> wire::Device {
    wire::Device {
        path: "/fuzz/1-2".into(),
        busid: "1-2".into(),
        busnum: 1,
        devnum: 2,
        speed: 3,
        vendor: 0x1234,
        product: 0x5678,
        configurations: 1,
        interfaces: 1,
        ..wire::Device::default()
    }
}

impl UsbDevice for Answering {
    fn describe(&self) -> wire::Device {
        device()
    }

    fn submit(&self, transfer: Transfer, done: Completion) {
        self.asked
            .lock()
            .unwrap()
            .push((transfer.seqnum, transfer.direction, transfer.iso.len()));
        // An isochronous OUT sends every packet whole, an IN reads nothing.
        let out = transfer.direction == Direction::Out;
        let iso: Vec<IsoPacket> = transfer
            .iso
            .iter()
            .map(|p| IsoPacket {
                actual_length: if out { p.length } else { 0 },
                status: 0,
                ..*p
            })
            .collect();
        let fits = match transfer.direction {
            Direction::In if iso.is_empty() => {
                Reply::read(vec![0xA5; transfer.length.min(64) as usize])
            }
            Direction::In => Reply::read(Vec::new()),
            Direction::Out if iso.is_empty() => Reply::sent(transfer.length),
            Direction::Out => Reply::sent(iso.iter().map(|p| p.actual_length).sum()),
        };
        match transfer.seqnum % 5 {
            0 | 1 => done.complete(Reply { iso, ..fits }),
            2 => {
                self.held.lock().unwrap().insert(transfer.seqnum, done);
            }
            3 => drop(done),
            _ => done.complete(Reply::read(vec![0; transfer.length as usize + 1])),
        }
    }

    fn unlink(&self, seqnum: u32) {
        // The server answers no transfer it unlinked.
        self.asked.lock().unwrap().retain(|(s, ..)| *s != seqnum);
        let held = self.held.lock().unwrap().remove(&seqnum);
        drop(held);
    }

    fn close(&self) {
        let held = std::mem::take(&mut *self.held.lock().unwrap());
        drop(held);
    }
}

/// One message from the driver: a submit with its OUT data and packet descriptors, an
/// unlink, or bytes that are neither.
fn driver_message() -> impl Strategy<Value = Vec<u8>> {
    let submit = (
        0u32..12,
        prop_oneof![9 => 0u32..2, 1 => any::<u32>()],
        prop_oneof![9 => 0u32..4, 1 => 0u32..20],
        prop_oneof![8 => 0u32..200, 1 => Just(wire::MAX_TRANSFER), 1 => any::<u32>()],
        prop_oneof![4 => Just(wire::NOT_ISO), 2 => Just(0), 3 => 1u32..5, 1 => any::<u32>()],
        any::<[u8; 8]>(),
        any::<Index>(),
    )
        .prop_map(|(seqnum, direction, ep, length, packets, setup, short)| {
            let header = Header::Submit {
                base: Base {
                    seqnum,
                    devid: 0x10002,
                    direction,
                    ep,
                },
                flags: 0,
                length,
                start_frame: 0,
                packets,
                interval: 1,
                setup,
            };
            let mut out = header.encode().to_vec();
            if let Some(payload) = wire::submit_payload(direction, length, packets) {
                let packets = wire::iso_packets(packets) as usize;
                // At most 4 KiB of the data: past that the stream ends inside the message.
                let data = payload - packets * wire::ISO_PACKET_BYTES;
                out.extend(std::iter::repeat_n(0x5A, data.min(4096)));
                let each = length as usize / packets.max(1);
                for i in 0..packets {
                    let descriptor = IsoPacket {
                        offset: (i * each) as u32,
                        length: each as u32,
                        actual_length: 0,
                        status: 0,
                    };
                    out.extend(descriptor.encode());
                }
                // Now and then the stream ends inside a message.
                if short.index(16) == 0 {
                    out.truncate(short.index(out.len()));
                }
            }
            out
        });
    let unlink = (0u32..12, 0u32..12).prop_map(|(seqnum, target)| {
        Header::Unlink {
            base: Base {
                seqnum: seqnum | 0x100,
                devid: 0x10002,
                direction: 0,
                ep: 0,
            },
            unlink: target,
        }
        .encode()
        .to_vec()
    });
    prop_oneof![12 => submit, 3 => unlink, 1 => vec(any::<u8>(), 0..64)]
}

/// The replies after the import: whole messages, each with as many bytes as it says.
fn replies_are_framed(
    mut out: &[u8],
    mut asked: Vec<(u32, Direction, usize)>,
) -> Result<(), String> {
    while !out.is_empty() {
        let head: [u8; wire::HEADER_BYTES] = out
            .get(..wire::HEADER_BYTES)
            .and_then(|h| h.try_into().ok())
            .ok_or("a cut header")?;
        out = &out[wire::HEADER_BYTES..];
        let follows = match Header::decode(&head) {
            Some(Header::RetSubmit { base, length, .. }) => {
                // A seqnum is never in flight twice, so the earliest is the one answered.
                let at = asked
                    .iter()
                    .position(|(s, ..)| *s == base.seqnum)
                    .ok_or("an answer to nothing")?;
                let (_, direction, packets) = asked.remove(at);
                let data = if direction == Direction::In {
                    length as usize
                } else {
                    0
                };
                data + packets * wire::ISO_PACKET_BYTES
            }
            Some(Header::RetUnlink { .. }) => 0,
            other => return Err(format!("{other:?} sent")),
        };
        out = out.get(follows..).ok_or("a cut reply")?;
    }
    Ok(())
}

proptest! {
    #[test]
    fn usbip_server_reads_anything(
        import in prop_oneof![
            6 => Just("1-2".to_owned()),
            1 => "[0-9a-z.:-]{1,31}",
        ],
        messages in vec(driver_message(), 0..8),
        mutate in prop_oneof![3 => Just(None), 1 => (any::<Index>(), any::<u8>()).prop_map(Some)],
    ) {
        let mut input = wire::import_request(&import).unwrap().to_vec();
        input.extend(messages.concat());
        if let Some((at, byte)) = mutate {
            let at = at.index(input.len());
            input[at] = byte;
        }
        let script = Arc::new(Script {
            input: Mutex::new((input, 0, false)),
            ..Script::default()
        });
        let answering = Arc::new(Answering::default());
        let _ = serve(script.clone(), answering.clone());
        let out = script.output.lock().unwrap().clone();
        let accepted = wire::import_reply(0);
        if out.starts_with(&accepted) {
            prop_assert_eq!(&out[accepted.len()..][..wire::DEVICE_BYTES], &device().encode().unwrap()[..]);
            let asked = answering.asked.lock().unwrap().clone();
            let framed = replies_are_framed(&out[accepted.len() + wire::DEVICE_BYTES..], asked);
            prop_assert!(framed.is_ok(), "{:?}", framed);
        } else {
            prop_assert_eq!(out, wire::import_reply(1).to_vec());
        }
        prop_assert!(answering.held.lock().unwrap().is_empty(), "held past close");
    }
}
