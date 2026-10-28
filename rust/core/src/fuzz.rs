//! Property tests over the host's decoders of client input that live in this crate: RTSP
//! framing and requests, the ANNOUNCE's SDP, control packet encryption and its replay window,
//! the legacy input cipher chain, microphone datagrams, FEC status, controller audio and the
//! session journal. Arbitrary bytes never panic, and what decodes encodes back to the same
//! value. Inputs are valid encodings of random values, edited, cut short or extended, and plain
//! random bytes. `PROPTEST_CASES` sets the cases per property (256 by default); a local fuzz
//! run raises it to about a minute per target. The wire decoders shared with Nightjar are
//! fuzzed in `client/protocol/core/src/fuzz.rs`.

use crate::{
    controller_audio, crypto,
    dualsense::DualSense,
    fec_status, input, mic, packet,
    rtsp::{self, MessageBuffer, Negotiated, Request},
    session_state::Journal,
};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use proptest::sample::{Index, select};
use std::collections::{BTreeSet, VecDeque};
use std::time::Instant;

/// Bytes biased towards what the decoders check: small values, printable ASCII, the RTSP
/// line ends and 0xFF.
fn byte() -> impl Strategy<Value = u8> {
    prop_oneof![
        0u8..8,
        0x20u8..0x7F,
        Just(b'\r'),
        Just(b'\n'),
        Just(0xFF),
        any::<u8>()
    ]
}

/// A message from `seeds`, valid encodings: as it is, with a few bytes changed, cut short or
/// with bytes after it; or random bytes of about its size.
fn message(seeds: impl Strategy<Value = Vec<u8>>) -> impl Strategy<Value = Vec<u8>> {
    let edits = vec((any::<Index>(), byte()), 0..4);
    let cut = prop_oneof![3 => Just(None), 1 => any::<Index>().prop_map(Some)];
    let tail = prop_oneof![3 => Just(Vec::new()), 1 => vec(byte(), 1..16)];
    let edited = (seeds, edits, cut, tail).prop_map(|(mut m, edits, cut, tail)| {
        for (at, b) in edits {
            if !m.is_empty() {
                let at = at.index(m.len());
                m[at] = b;
            }
        }
        if let Some(cut) = cut {
            m.truncate(cut.index(m.len() + 1));
        }
        m.extend(tail);
        m
    });
    prop_oneof![4 => edited, 1 => vec(any::<u8>(), 0..96)]
}

const KEY: [u8; 16] = *b"fuzzed-input-key";

// RTSP.

/// A request as a client writes one: CSeq first, then its headers and a Content-Length.
fn request_bytes(
    method: &str,
    target: &str,
    cseq: u32,
    headers: &[(String, String)],
    body: &[u8],
) -> Vec<u8> {
    let mut head = format!("{method} {target} RTSP/1.0\r\nCSeq: {cseq}\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut b = head.into_bytes();
    b.extend_from_slice(body);
    b
}

prop_compose! {
    fn request()(
        method in select(&["OPTIONS", "DESCRIBE", "SETUP", "ANNOUNCE", "PLAY", "TEARDOWN"][..]),
        target in "rtsp://[a-z0-9.]{1,12}(:[0-9]{1,5})?(/[a-z=0-9/]{0,16})?",
        cseq in any::<u32>(),
        // Lowercase names, so no two differ only in case; none the request sets itself.
        headers in btree_map("x-[a-z-]{1,12}", "[!-~]([ -~]{0,20}[!-~])?", 0..5),
        body in vec(any::<u8>(), 0..64),
    ) -> (String, String, u32, Vec<(String, String)>, Vec<u8>) {
        (method.to_owned(), target, cseq, headers.into_iter().collect(), body)
    }
}

fn requests() -> impl Strategy<Value = Vec<u8>> {
    request().prop_map(|(m, t, c, h, b)| request_bytes(m.as_str(), &t, c, &h, &b))
}

/// The request written back out from what was parsed: every header as it was read.
fn reserialized(r: &Request) -> Vec<u8> {
    let mut head = format!("{} {} RTSP/1.0\r\n", r.method, r.target);
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut b = head.into_bytes();
    b.extend_from_slice(&r.body);
    b
}

proptest! {
    #[test]
    fn rtsp_requests_round_trip((method, target, cseq, headers, body) in request()) {
        let wire = request_bytes(&method, &target, cseq, &headers, &body);
        prop_assert_eq!(rtsp::complete_length(&wire).unwrap(), Some(wire.len()));
        let r = Request::parse(&wire).unwrap();
        prop_assert_eq!(&r.method, &method);
        prop_assert_eq!(&r.target, &target);
        prop_assert_eq!(r.cseq, cseq);
        prop_assert_eq!(&r.body, &body);
        for (k, v) in &headers {
            prop_assert_eq!(r.headers.get(k), Some(v));
        }
        prop_assert_eq!(r.headers.len(), headers.len() + 2);
    }

    /// Whatever parses, written back out, parses to the same request.
    #[test]
    fn rtsp_parsing_never_panics_and_is_stable(bytes in message(requests())) {
        let _ = rtsp::complete_length(&bytes);
        if let Ok(r) = Request::parse(&bytes) {
            let again = Request::parse(&reserialized(&r)).unwrap();
            prop_assert_eq!(again.method, r.method);
            prop_assert_eq!(again.target, r.target);
            prop_assert_eq!(again.headers, r.headers);
            prop_assert_eq!(again.body, r.body);
            prop_assert_eq!(again.cseq, r.cseq);
        }
    }

    /// A request fed in pieces frames as `complete_length` frames it whole: complete exactly
    /// at its length, an error for any byte after it, and the same errors.
    #[test]
    fn rtsp_framing_in_pieces_matches_framing_whole(
        bytes in message(requests()),
        cuts in vec(any::<Index>(), 0..6),
    ) {
        let mut cuts: Vec<usize> = cuts.iter().map(|c| c.index(bytes.len() + 1)).collect();
        cuts.push(bytes.len());
        cuts.sort_unstable();
        let mut buffer = MessageBuffer::default();
        let mut at = 0;
        for cut in cuts {
            let got = buffer.append(&bytes[at..cut]);
            at = cut;
            let whole = rtsp::complete_length(&bytes[..at]);
            match (&got, &whole) {
                (Err(_), Err(_)) => {}
                (Ok(false), Ok(None)) => {}
                (Ok(true), Ok(Some(n))) => prop_assert_eq!(*n, at),
                (Err(_), Ok(Some(n))) => prop_assert!(*n < at),
                _ => prop_assert!(false, "{got:?} in pieces, {whole:?} whole, at {at}"),
            }
            if got.is_err() {
                break;
            }
        }
    }

    /// A response frames as one complete message of its own length.
    #[test]
    fn rtsp_responses_frame_as_one_message(
        cseq in any::<u32>(),
        code in 100u16..600,
        reason in "[A-Za-z ]{1,20}",
        headers in btree_map("[A-Z][a-z-]{1,12}", "[!-~]{1,20}", 0..4),
        body in vec(any::<u8>(), 0..64),
    ) {
        let headers: Vec<(&str, String)> = headers
            .iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("content-length"))
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let wire = rtsp::response(cseq, code, &reason, &headers, &body);
        prop_assert_eq!(rtsp::complete_length(&wire).unwrap(), Some(wire.len()));
        prop_assert!(wire.ends_with(&body));
    }
}

// The ANNOUNCE's SDP.

/// Attributes a client sends, with values in, at and past their limits.
fn sdp_attributes() -> impl Strategy<Value = Vec<(&'static str, String)>> {
    let keys = select(
        &[
            "x-nv-video[0].clientViewportWd",
            "x-nv-video[0].clientViewportHt",
            "x-nv-video[0].maxFPS",
            "x-nv-video[0].clientRefreshRateX100",
            "x-nv-vqos[0].bw.maximumBitrateKbps",
            "x-ml-video.configuredBitrateKbps",
            "x-nv-video[0].encoderCscMode",
            "x-nv-vqos[0].bitStreamFormat",
            "x-nv-video[0].dynamicRangeMode",
            "x-ss-video[0].chromaSamplingType",
            "x-nv-video[0].videoEncoderSlicesPerFrame",
            "x-nv-video[0].maxNumReferenceFrames",
            "x-ss-video[0].intraRefresh",
            "x-ss-video[0].pyrowaveFeatures",
            "x-ss-video[0].pyrowaveAdaptiveFec",
            "x-ss-video[0].pyrowaveLinkMbps",
            "x-ss-video[0].pyrowaveRowOrder",
            "x-nv-video[0].packetSize",
            "x-nv-vqos[0].fec.minRequiredFecPackets",
            "x-nv-audio.surround.numChannels",
            "x-nv-aqos.packetDuration",
            "x-ss-general.encryptionEnabled",
            "x-nv-general.featureFlags",
            "x-ml-general.featureFlags",
            crate::packed444::REQUEST_ATTRIBUTE,
            rubylight_protocol::hdr::ANNOUNCE_ATTRIBUTE,
            rubylight_protocol::audio_ext::CODEC_ANNOUNCE_ATTRIBUTE,
        ][..],
    );
    let value = prop_oneof![
        (0u32..20).prop_map(|v| v.to_string()),
        (60u32..5000).prop_map(|v| v.to_string()),
        any::<u32>().prop_map(|v| v.to_string()),
        Just("4294967296".to_owned()),
        Just("-1".to_owned()),
        Just(" 7 ".to_owned()),
        "[ -~]{0,6}",
    ];
    vec((keys, value), 0..12)
}

fn sdp(attributes: &[(&str, String)]) -> Vec<u8> {
    let mut s =
        String::from("v=0\r\no=android 0 14 IN IPv4 127.0.0.1\r\ns=NVIDIA Streaming Client\r\n");
    for (k, v) in attributes {
        s.push_str(&format!("a={k}:{v}\r\n"));
    }
    s.into_bytes()
}

proptest! {
    /// Any ANNOUNCE body parses without a panic, and what it accepts is valid and survives
    /// the session record's serialization.
    #[test]
    fn sdp_parsing_never_panics_and_accepts_only_valid_streams(
        bytes in message(sdp_attributes().prop_map(|a| sdp(&a))),
    ) {
        if let Ok(n) = Negotiated::from_sdp(&bytes) {
            prop_assert!(n.validate().is_ok());
            let json = serde_json::to_value(&n).unwrap();
            let back: Negotiated = serde_json::from_value(json.clone()).unwrap();
            prop_assert_eq!(serde_json::to_value(&back).unwrap(), json);
        }
    }

    /// A plain stream request is taken as written.
    #[test]
    fn sdp_takes_a_valid_mode_as_written(
        width in (32u32..8192).prop_map(|w| w * 2),
        height in (32u32..8192).prop_map(|h| h * 2),
        fps in 1u32..=1000,
        bitrate in 1u32..=2_000_000,
        channels in select(&[2u32, 6, 8][..]),
        packet in 256u32..=1400,
    ) {
        let n = Negotiated::from_sdp(&sdp(&[
            ("x-nv-video[0].clientViewportWd", width.to_string()),
            ("x-nv-video[0].clientViewportHt", height.to_string()),
            ("x-nv-video[0].maxFPS", fps.to_string()),
            ("x-nv-vqos[0].bw.maximumBitrateKbps", bitrate.to_string()),
            ("x-nv-audio.surround.numChannels", channels.to_string()),
            ("x-nv-video[0].packetSize", packet.to_string()),
        ]))
        .unwrap();
        prop_assert_eq!((n.width, n.height, n.fps, n.bitrate_kbps), (width, height, fps, bitrate));
        prop_assert_eq!((u32::from(n.audio_channels), n.packet_size), (channels, packet as usize));
        prop_assert_eq!(n.rate_millihz, fps * 1000);
    }
}

// Control packets.

/// A control packet as a client seals it: version 2 numbers its IV with the sequence and
/// "CC", version 1 with the sequence's low byte.
fn client_control(seq: u32, kind: u16, payload: &[u8], v2: bool) -> Vec<u8> {
    let mut plain = kind.to_le_bytes().to_vec();
    plain.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    plain.extend_from_slice(payload);
    let mut iv = vec![0; if v2 { 12 } else { 16 }];
    if v2 {
        iv[..4].copy_from_slice(&seq.to_le_bytes());
        iv[10] = b'C';
        iv[11] = b'C';
    } else {
        iv[0] = seq as u8;
    }
    let (tag, b) = crypto::gcm_seal(&KEY, &iv, &plain).unwrap();
    let mut p = 1u16.to_le_bytes().to_vec();
    p.extend_from_slice(&((4 + 16 + b.len()) as u16).to_le_bytes());
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&tag);
    p.extend_from_slice(&b);
    p
}

prop_compose! {
    fn control_message()(
        seq in any::<u32>(),
        kind in any::<u16>(),
        payload in vec(any::<u8>(), 0..48),
        v2 in any::<bool>(),
    ) -> (u32, u16, Vec<u8>, bool) {
        (seq, kind, payload, v2)
    }
}

proptest! {
    #[test]
    fn control_packets_never_panic(
        bytes in message(control_message().prop_map(|(s, k, p, v2)| client_control(s, k, &p, v2))),
        v2 in any::<bool>(),
    ) {
        let _ = packet::control_header(&bytes);
        let _ = packet::decrypt_control(&KEY, &bytes, v2);
    }

    #[test]
    fn client_control_packets_open_as_sealed((seq, kind, payload, v2) in control_message()) {
        let wire = client_control(seq, kind, &payload, v2);
        prop_assert_eq!(packet::decrypt_control(&KEY, &wire, v2).unwrap(), (seq, kind, payload));
    }

    /// Version 2 tells the directions apart, so a host packet reflected back is refused;
    /// version 1 does not, as in moonlight-common-c.
    #[test]
    fn host_control_packets_open_only_in_version_1((seq, kind, payload, v2) in control_message()) {
        let wire = packet::encrypted_control(&KEY, seq, kind, &payload, v2).unwrap();
        let opened = packet::decrypt_control(&KEY, &wire, v2);
        if v2 {
            prop_assert!(opened.is_err());
        } else {
            prop_assert_eq!(opened.unwrap(), (seq, kind, payload));
        }
    }

    /// Every changed byte is refused, except that version 1 authenticates only the low byte
    /// of the sequence (moonlight-common-c's IV): there the replay window has to tell.
    #[test]
    fn changed_control_packets_are_refused(
        (seq, kind, payload, v2) in control_message(),
        at in any::<Index>(),
        flip in 1u8..,
    ) {
        let mut wire = client_control(seq, kind, &payload, v2);
        let at = at.index(wire.len());
        wire[at] ^= flip;
        let opened = packet::decrypt_control(&KEY, &wire, v2);
        if !v2 && (5..8).contains(&at) {
            let changed = u32::from_le_bytes(wire[4..8].try_into().unwrap());
            prop_assert_eq!(opened.unwrap(), (changed, kind, payload));
        } else {
            prop_assert!(opened.is_err(), "byte {at} changed and the packet still opened");
        }
    }

    /// The replay window takes a sequence once, any newer one, and an older unseen one only
    /// within its 4096.
    #[test]
    fn the_replay_window_takes_each_sequence_once(
        start in any::<u32>(),
        steps in vec(prop_oneof![-5000i64..5000, -64i64..64, Just(0i64)], 1..200),
    ) {
        let mut window = packet::ReplayWindow::default();
        let mut seen = BTreeSet::new();
        let mut newest: Option<u32> = None;
        let mut at = i64::from(start);
        for step in steps {
            at = (at + step).clamp(0, i64::from(u32::MAX));
            let sequence = at as u32;
            let expected = match newest {
                None => true,
                Some(n) if sequence > n => true,
                Some(n) => n - sequence < 4096 && !seen.contains(&sequence),
            };
            prop_assert_eq!(window.accept(sequence), expected, "sequence {}", sequence);
            if expected {
                seen.insert(sequence);
                newest = Some(newest.map_or(sequence, |n| n.max(sequence)));
            }
        }
    }
}

// The legacy input cipher.

/// The client side of the legacy input chain as the host reads it: each message under the
/// current IV, and the next IV from the message's last 16 ciphertext bytes once it has them.
struct LegacySender {
    iv: [u8; 16],
}
impl LegacySender {
    fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let (tag, encrypted) = crypto::gcm_seal(&KEY, &self.iv, plain).unwrap();
        let mut packet = ((tag.len() + encrypted.len()) as u32)
            .to_be_bytes()
            .to_vec();
        packet.extend_from_slice(&tag);
        packet.extend_from_slice(&encrypted);
        if encrypted.len() >= 16 {
            self.iv.copy_from_slice(&encrypted[encrypted.len() - 16..]);
        }
        packet
    }
}

#[derive(Debug, Clone)]
enum LegacyStep {
    Message(Vec<u8>),
    /// The next real message with a byte changed.
    Forged(Index, u8),
    Junk(Vec<u8>),
}

fn legacy_steps() -> impl Strategy<Value = Vec<LegacyStep>> {
    let step = prop_oneof![
        4 => vec(any::<u8>(), 0..64).prop_map(LegacyStep::Message),
        1 => (any::<Index>(), 1u8..).prop_map(|(i, f)| LegacyStep::Forged(i, f)),
        1 => message(vec(any::<u8>(), 20..64)).prop_map(LegacyStep::Junk),
    ];
    vec(step, 1..24)
}

proptest! {
    /// A chain of messages opens in order, and forged or junk packets between them neither
    /// open nor move the chain.
    #[test]
    fn the_legacy_input_chain_survives_forgeries(key_id in any::<u32>(), steps in legacy_steps()) {
        let mut iv = [0; 16];
        iv[..4].copy_from_slice(&key_id.to_be_bytes());
        let mut sender = LegacySender { iv };
        let mut host = packet::LegacyInput::new(key_id);
        for step in steps {
            match step {
                LegacyStep::Message(plain) => {
                    let wire = sender.seal(&plain);
                    prop_assert_eq!(host.open(&KEY, &wire).unwrap(), plain);
                }
                LegacyStep::Forged(at, flip) => {
                    let mut probe = LegacySender { iv: sender.iv };
                    let mut wire = probe.seal(b"forged input message");
                    let at = at.index(wire.len());
                    wire[at] ^= flip;
                    prop_assert!(host.open(&KEY, &wire).is_err());
                }
                LegacyStep::Junk(bytes) => {
                    prop_assert!(host.open(&KEY, &bytes).is_err());
                }
            }
        }
    }
}

// Input packets, decoded and applied to a USB DualSense.

/// An input packet's framing around random fields: the length, then a known magic.
fn input_packet() -> impl Strategy<Value = Vec<u8>> {
    let magic = select(
        &[
            3u32,
            4,
            5,
            6,
            7,
            8,
            9,
            10,
            12,
            13,
            23,
            0x5500_0001,
            0x5500_0002,
            0x5500_0003,
            0x5500_0004,
            0x5500_0005,
            0x5500_0006,
            0x5500_0007,
            0x5500_0008,
            0x5500_000B,
        ][..],
    );
    (magic, vec(byte(), 0..40)).prop_map(|(magic, body)| {
        let mut b = ((body.len() + 4) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(&magic.to_le_bytes());
        b.extend(body);
        b
    })
}

proptest! {
    #[test]
    fn input_packets_never_panic(bytes in message(input_packet())) {
        let mut pad = DualSense::new(0, 0);
        if let Ok(event) = input::decode(&bytes) {
            pad.apply(&event);
        }
    }
}

// Microphone datagrams.

proptest! {
    #[test]
    fn microphone_datagrams_never_panic(
        bytes in message((any::<u16>(), any::<u32>(), vec(any::<u8>(), 1..80))
            .prop_map(|(s, t, o)| mic::seal(&KEY, 7, s, t, &o))),
    ) {
        if let Some(packet) = mic::parse(&bytes) {
            let _ = mic::open(&KEY, 7, &packet);
        }
    }

    /// What a client seals opens to the same packet, except one that is whole blocks and
    /// ends in what looks like padding: the host cannot tell (see `mic`).
    #[test]
    fn microphone_packets_round_trip(
        key_id in any::<u32>(),
        sequence in any::<u16>(),
        timestamp in any::<u32>(),
        opus in prop_oneof![vec(any::<u8>(), 1..200), vec(0u8..4, 15..18)],
    ) {
        let wire = mic::seal(&KEY, key_id, sequence, timestamp, &opus);
        let packet = mic::parse(&wire).unwrap();
        prop_assert_eq!((packet.sequence, packet.timestamp), (sequence, timestamp));
        let opened = mic::open(&KEY, key_id, &packet).unwrap();
        let last = *opus.last().unwrap();
        let looks_padded = opus.len() % 16 == 0
            && (1..16).contains(&last)
            && opus[opus.len() - usize::from(last)..].iter().all(|&b| b == last);
        if looks_padded {
            prop_assert_eq!(&opened[..], &opus[..opus.len() - usize::from(last)]);
        } else {
            prop_assert_eq!(opened, opus);
        }
    }
}

// FEC status.

fn fec_status_bytes(s: &fec_status::Status) -> Vec<u8> {
    let mut b = s.frame.to_be_bytes().to_vec();
    for word in [
        s.highest_received,
        s.next_contiguous,
        s.missing_before_highest,
        s.data,
        s.parity,
        s.received_data,
        s.received_parity,
    ] {
        b.extend_from_slice(&word.to_be_bytes());
    }
    b.extend_from_slice(&[s.percentage, s.block, s.blocks]);
    b
}

prop_compose! {
    /// Mostly consistent reports: the counts the client's FEC block could have.
    fn fec_status()(
        frame in any::<u32>(),
        data in 1u16..=250,
        percentage in 0u8..=100,
        blocks in 1u8..=4,
    )(
        frame in Just(frame),
        data in Just(data),
        percentage in Just(percentage),
        parity in Just((u32::from(data) * u32::from(percentage)).div_ceil(100) as u16),
        received_data in 0..=data,
        blocks in Just(blocks),
        block in 0..blocks,
        highest in any::<u16>(),
        span in 0u16..8,
        missing in 0u16..4,
    ) -> Vec<u8> {
        fec_status_bytes(&fec_status::Status {
            frame,
            highest_received: highest,
            next_contiguous: highest.wrapping_add(1).wrapping_sub(span),
            missing_before_highest: missing,
            data,
            parity,
            received_data,
            received_parity: parity / 2,
            percentage,
            block,
            blocks,
        })
    }
}

proptest! {
    /// Every byte of a FEC status is a field: what parses writes back to the same bytes.
    #[test]
    fn fec_status_round_trips(bytes in message(fec_status())) {
        if let Ok(status) = fec_status::Status::parse(&bytes) {
            prop_assert_eq!(fec_status_bytes(&status), bytes);
        }
    }
}

// Controller audio.

fn pcm_bytes(id: u16, sequence: u32, samples: &[u8]) -> Vec<u8> {
    let mut b = vec![1, 1];
    b.extend_from_slice(&id.to_le_bytes());
    b.extend_from_slice(&sequence.to_le_bytes());
    b.extend_from_slice(&((samples.len() / 4) as u16).to_le_bytes());
    b.extend_from_slice(&[0, 0]);
    b.extend_from_slice(samples);
    b
}

fn status_bytes(id: u16, flags: u8) -> Vec<u8> {
    let mut b = vec![1, 1];
    b.extend_from_slice(&id.to_le_bytes());
    b.extend_from_slice(&[flags, 0, 0, 0]);
    b
}

fn controller_audio_message() -> impl Strategy<Value = (u16, Vec<u8>)> {
    let pcm = (
        0u16..18,
        any::<u32>(),
        (1usize..=12).prop_flat_map(|f| vec(any::<u8>(), f * 4)),
    )
        .prop_map(|(id, s, b)| pcm_bytes(id, s, &b));
    let status = (0u16..18, 0u8..20).prop_map(|(id, f)| status_bytes(id, f));
    prop_oneof![
        message(pcm).prop_map(|b| (controller_audio::MICROPHONE, b)),
        message(status).prop_map(|b| (controller_audio::CONTROL, b)),
        message(vec(any::<u8>(), 0..32)).prop_map(|b| (controller_audio::SPEAKER, b)),
    ]
}

proptest! {
    #[test]
    fn controller_audio_round_trips(id in 0u16..16, sequence in any::<u32>(), frames in 1usize..=240, flags in 0u8..16) {
        let samples: Vec<u8> = (0..frames * 4).map(|i| i as u8).collect();
        let wire = pcm_bytes(id, sequence, &samples);
        prop_assert_eq!(controller_audio::pcm(&wire), Some((id, sequence, &samples[..])));
        prop_assert_eq!(controller_audio::status(&status_bytes(id, flags)), Some((id, flags)));
    }

    /// Any controller audio a client sends a USB DualSense never panics it.
    #[test]
    fn controller_audio_never_panics(messages in vec(controller_audio_message(), 1..24)) {
        let mut pad = DualSense::new(1, 1);
        for (kind, payload) in messages {
            if let Some((id, sequence, samples)) = controller_audio::pcm(&payload) {
                prop_assert_eq!(pcm_bytes(id, sequence, samples), payload.clone());
            }
            if let Some((id, flags)) = controller_audio::status(&payload) {
                prop_assert_eq!(status_bytes(id, flags), payload.clone());
            }
            pad.audio(kind, &payload);
            let _ = pad.take_speaker();
            let _ = pad.take_pcm();
        }
    }

    /// The controller microphone's capture queue plays back what arrived in order, restarts
    /// on a gap, drops what is late and pads a short read with silence.
    #[test]
    fn controller_capture_follows_its_model(
        pushes in vec((0u32..12, (0usize..260).prop_flat_map(|n| vec(any::<u8>(), n)), 0usize..1200), 1..32),
    ) {
        let now = Instant::now();
        let mut capture = controller_audio::Capture::default();
        let mut model: VecDeque<u8> = VecDeque::new();
        let mut last: Option<u32> = None;
        for (sequence, bytes, read) in pushes {
            let valid = !bytes.is_empty() && bytes.len() <= 960 && bytes.len() % 4 == 0;
            if valid && last.is_none_or(|old| sequence.wrapping_sub(old) as i32 > 0) {
                if last.is_some_and(|old| sequence != old.wrapping_add(1)) || model.len() + bytes.len() > 3840 {
                    model.clear();
                }
                last = Some(sequence);
                model.extend(&bytes);
            }
            capture.push(sequence, &bytes, now);
            let mut expected: Vec<u8> = model.drain(..read.min(model.len())).collect();
            expected.resize(read, 0);
            prop_assert_eq!(capture.read(read, now), expected);
        }
    }
}

// The session journal.

fn journal() -> impl Strategy<Value = Journal> {
    use crate::native_api::session::{SessionApp, State};
    let state = select(
        &[
            State::Preparing,
            State::Launching,
            State::Streaming,
            State::Suspended,
            State::Stopping,
            State::Unknown,
        ][..],
    );
    let app = proptest::option::of(("[ -~]{0,12}", any::<u32>(), "\\PC{0,16}").prop_map(
        |(install_id, app_id, title)| SessionApp {
            install_id,
            app_id,
            title,
        },
    ));
    (
        any::<u32>(),
        state,
        proptest::option::of("\\PC{0,16}"),
        app,
        any::<u64>(),
    )
        .prop_map(|(version, state, owner, app, since_unix_ms)| Journal {
            version,
            state,
            owner,
            app,
            since_unix_ms,
        })
}

proptest! {
    #[test]
    fn the_session_journal_round_trips(journal in journal()) {
        prop_assert_eq!(Journal::parse(&journal.to_bytes()), Some(journal));
    }

    /// A damaged journal is ignored or read as some journal, never a panic, and what it reads
    /// as writes back to the same journal.
    #[test]
    fn a_damaged_session_journal_never_panics(bytes in message(journal().prop_map(|j| j.to_bytes()))) {
        if let Some(journal) = Journal::parse(&bytes) {
            prop_assert_eq!(Journal::parse(&journal.to_bytes()), Some(journal));
        }
    }
}
