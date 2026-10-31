use super::super::sdp::testing::{answer, answer_for, client, host_offer, spec, surround_spec};
use super::super::sdp::{VideoCodec, accept_answer};
use super::*;
use std::collections::HashMap;
use std::sync::Mutex;
use str0m::media::{KeyframeRequestKind, Mid};
use str0m::rtp::RawPacket;

const HOST: &str = "192.168.1.10:8001";
const CLIENT: &str = "192.168.1.20:50000";

#[derive(Clone, Debug)]
struct Rtp {
    pt: u8,
    marker: bool,
    timestamp: u32,
    seq: u16,
    playout: Option<(MediaTime, MediaTime)>,
}

#[derive(Debug)]
struct Received {
    pt: u8,
    data: Vec<u8>,
    time: MediaTime,
}

/// A host Engine against a plain str0m client, joined by an in-memory link and a virtual clock.
struct Link {
    host: Engine,
    client: Rtc,
    client_video: Option<Mid>,
    now: Instant,
    start: Instant,
    authenticated: Arc<AtomicBool>,
    rtp: Vec<Rtp>,
    media: Vec<Received>,
    host_events: Vec<TransportEvent>,
    idr: Vec<IdrReason>,
    channel_data: Vec<(String, Vec<u8>)>,
    labels: HashMap<ChannelId, String>,
    closed_by_host: Vec<String>,
    /// Drop the n-th host media datagram (counting from zero).
    drop_rtp: Option<usize>,
    rtp_seen: usize,
    /// Header extension ids on the wire, from the unencrypted part of every media datagram.
    wire_ext_ids: std::collections::BTreeSet<u8>,
    /// Drop every datagram in both directions (the path is down).
    partition: bool,
    /// The host's ICE username fragment, for forging STUN.
    host_ufrag: String,
}

/// The ids of the one-byte header extensions in an RTP packet (RFC 8285).
fn extension_ids(packet: &[u8]) -> Vec<u8> {
    let mut ids = Vec::new();
    if packet[0] & 0x10 == 0 {
        return ids;
    }
    let at = 12 + 4 * usize::from(packet[0] & 0x0f);
    if packet[at..at + 2] != [0xbe, 0xde] {
        return ids;
    }
    let words = usize::from(u16::from_be_bytes([packet[at + 2], packet[at + 3]]));
    let block = &packet[at + 4..at + 4 + 4 * words];
    let mut i = 0;
    while i < block.len() {
        if block[i] == 0 {
            i += 1;
            continue;
        }
        ids.push(block[i] >> 4);
        i += 1 + usize::from(block[i] & 0x0f) + 1;
    }
    ids
}

/// The client's answer for a surround request: str0m's own client only knows stereo Opus, so
/// it answers that and its Opus line is munged to multiopus the way Chromium would send it.
fn multiopus_answer(spec: &OfferSpec, offer: &str, client: &mut Rtc) -> String {
    answer_for(client, offer, spec)
}

impl Link {
    fn new(spec: OfferSpec) -> Self {
        let host_addr: SocketAddr = HOST.parse().unwrap();
        let (mut rtc, offer) = host_offer(&spec, host_addr);
        let host_ufrag = offer
            .sdp
            .lines()
            .find_map(|l| l.strip_prefix("a=ice-ufrag:"))
            .expect("the offer has an ice-ufrag")
            .trim()
            .to_string();
        let mut client = client(&spec, CLIENT.parse().unwrap());
        let answer_text = multiopus_answer(&spec, &offer.sdp, &mut client);
        let (ids, _) = accept_answer(&mut rtc, offer, &answer_text, &spec).unwrap();
        let now = Instant::now();
        let authenticated = Arc::new(AtomicBool::new(false));
        let host = Engine::new(rtc, ids, &spec, host_addr, now, authenticated.clone());
        Self {
            host,
            client,
            client_video: None,
            now,
            start: now,
            authenticated,
            rtp: Vec::new(),
            media: Vec::new(),
            host_events: Vec::new(),
            idr: Vec::new(),
            channel_data: Vec::new(),
            labels: HashMap::new(),
            closed_by_host: Vec::new(),
            drop_rtp: None,
            rtp_seen: 0,
            wire_ext_ids: Default::default(),
            partition: false,
            host_ufrag,
        }
    }

    fn connected(spec: OfferSpec) -> Self {
        let mut link = Self::new(spec);
        for _ in 0..3000 {
            link.step(Duration::from_millis(1));
            let open = |l: &str| {
                link.host_events
                    .contains(&TransportEvent::ChannelOpen(l.into()))
            };
            if link.host.is_connected() && open("control") && open("gamepad") {
                return link;
            }
        }
        panic!("never connected: {:?}", link.host_events);
    }

    /// Move the clock `dt` and carry datagrams until the link is quiet.
    fn step(&mut self, dt: Duration) {
        self.now += dt;
        let (host_addr, client_addr): (SocketAddr, SocketAddr) =
            (HOST.parse().unwrap(), CLIENT.parse().unwrap());
        for _ in 0..200 {
            let mut moved = false;
            self.host.advance(self.now).unwrap();
            self.client.handle_input(Input::Timeout(self.now)).unwrap();
            self.collect_host();
            for out in self.host.take_outgoing() {
                moved = true;
                if out.paced {
                    self.wire_ext_ids.extend(extension_ids(&out.data));
                    let n = self.rtp_seen;
                    self.rtp_seen += 1;
                    if self.drop_rtp == Some(n) {
                        continue;
                    }
                }
                if self.partition {
                    continue;
                }
                let recv = Receive::new(Protocol::Udp, host_addr, client_addr, &out.data).unwrap();
                self.client
                    .handle_input(Input::Receive(self.now, recv))
                    .unwrap();
            }
            loop {
                match self.client.poll_output().unwrap() {
                    Output::Timeout(_) => break,
                    Output::Transmit(t) => {
                        moved = true;
                        let data: Vec<u8> = t.contents.into();
                        if !self.partition {
                            let fate = self.host.handle_datagram(self.now, client_addr, &data);
                            assert_eq!(fate, Datagram::Handled, "the real client's datagram");
                        }
                    }
                    Output::Event(e) => self.on_client_event(e),
                }
            }
            self.collect_host();
            if !moved {
                break;
            }
        }
    }

    fn run(&mut self, total: Duration) {
        for _ in 0..total.as_millis() {
            self.step(Duration::from_millis(1));
        }
    }

    fn collect_host(&mut self) {
        self.host_events.extend(self.host.take_events());
        self.idr.extend(self.host.take_idr_requests());
    }

    fn on_client_event(&mut self, e: Event) {
        match e {
            Event::MediaAdded(added) if added.kind.is_video() => {
                self.client_video = Some(added.mid)
            }
            Event::MediaData(d) => self.media.push(Received {
                pt: *d.pt,
                data: d.data.to_vec(),
                time: d.time,
            }),
            Event::RawPacket(raw) => {
                if let RawPacket::RtpRx(h, _) = *raw {
                    self.rtp.push(Rtp {
                        pt: *h.payload_type,
                        marker: h.marker,
                        timestamp: h.timestamp,
                        seq: h.sequence_number,
                        playout: h.ext_vals.play_delay_min.zip(h.ext_vals.play_delay_max),
                    });
                }
            }
            Event::ChannelOpen(id, label) => {
                self.labels.insert(id, label);
            }
            Event::ChannelData(d) => {
                let label = self.labels.get(&d.id).cloned().unwrap_or_default();
                self.channel_data.push((label, d.data));
            }
            Event::ChannelClose(id) => {
                if let Some(label) = self.labels.get(&id) {
                    self.closed_by_host.push(label.clone());
                }
            }
            _ => {}
        }
    }

    fn at(&self, ms: u64) -> Instant {
        self.start + Duration::from_millis(ms)
    }

    fn video(&mut self, data: Vec<u8>, at_ms: u64, keyframe: bool) {
        let frame = VideoFrame {
            data,
            capture: self.at(at_ms),
            keyframe,
        };
        assert!(
            self.host.write_video(self.now, frame).unwrap(),
            "not connected"
        );
    }

    fn audio(&mut self, data: Vec<u8>, at_ms: u64, position: u64) {
        let packet = AudioPacket {
            data,
            capture: self.at(at_ms),
            position,
        };
        assert!(
            self.host.write_audio(self.now, packet).unwrap(),
            "not connected"
        );
    }

    fn client_channel(&self, label: &str) -> ChannelId {
        *self
            .labels
            .iter()
            .find(|(_, l)| l.as_str() == label)
            .unwrap_or_else(|| panic!("no {label} channel"))
            .0
    }

    fn client_writes(&mut self, label: &str, data: &[u8]) {
        let id = self.client_channel(label);
        self.client_writes_to(id, data);
    }

    fn client_writes_to(&mut self, id: ChannelId, data: &[u8]) {
        assert!(self.client.channel(id).unwrap().write(true, data).unwrap());
    }

    fn client_opens(&mut self, label: &str) -> ChannelId {
        let mut api = self.client.sdp_api();
        let id = api.add_channel(label.into());
        assert!(
            api.apply().is_none(),
            "opening a channel should not renegotiate"
        );
        id
    }
}

/// Bytes that can't contain a start code or a valid-looking OBU boundary by accident.
fn filler(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8 % 200).wrapping_add(seed).max(1) | 0x01)
        .collect()
}

fn annex_b(nals: &[Vec<u8>]) -> Vec<u8> {
    nals.iter()
        .flat_map(|n| [&[0, 0, 0, 1][..], n.as_slice()].concat())
        .collect()
}

fn h264_frame(keyframe: bool, size: usize) -> Vec<u8> {
    if keyframe {
        annex_b(&[
            [&[0x67u8][..], &filler(20, 3)].concat(),
            [&[0x68u8][..], &filler(6, 5)].concat(),
            [&[0x65u8][..], &filler(size, 7)].concat(),
        ])
    } else {
        annex_b(&[[&[0x41u8][..], &filler(size, 9)].concat()])
    }
}

fn hevc_frame(keyframe: bool, size: usize) -> Vec<u8> {
    if keyframe {
        annex_b(&[
            [&[0x40u8, 0x01][..], &filler(20, 3)].concat(),
            [&[0x42u8, 0x01][..], &filler(30, 4)].concat(),
            [&[0x44u8, 0x01][..], &filler(6, 5)].concat(),
            [&[0x26u8, 0x01][..], &filler(size, 7)].concat(),
        ])
    } else {
        annex_b(&[[&[0x02u8, 0x01][..], &filler(size, 9)].concat()])
    }
}

fn obu(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![(kind << 3) | 0x02];
    let mut size = payload.len();
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        if size == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(payload);
    out
}

const TD: [u8; 2] = [0x12, 0x00];

/// A temporal delimiter, then (for a keyframe) a sequence header, then the frame OBU.
fn av1_frame(keyframe: bool, size: usize) -> Vec<u8> {
    let mut out = TD.to_vec();
    if keyframe {
        out.extend(obu(1, &filler(12, 3)));
    }
    out.extend(obu(6, &filler(size, 9)));
    out
}

fn without_td(frame: &[u8]) -> Vec<u8> {
    frame[TD.len()..].to_vec()
}

fn diff(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(i) => format!(
            "first difference at byte {i} (lengths {} and {})",
            a.len(),
            b.len()
        ),
        None if a.len() != b.len() => {
            format!("same prefix, lengths {} and {}", a.len(), b.len())
        }
        None => "identical".into(),
    }
}

#[test]
fn video_arrives_byte_identical_for_every_codec() {
    type Frame = fn(bool, usize) -> Vec<u8>;
    let cases: [(VideoCodec, Frame, bool); 4] = [
        (VideoCodec::H264, h264_frame, false),
        (VideoCodec::Hevc, hevc_frame, false),
        (VideoCodec::Hevc, hevc_frame, true),
        (VideoCodec::Av1, av1_frame, false),
    ];
    for (codec, make, hdr) in cases {
        let mut link = Link::connected(spec(codec, hdr));
        let sent = [make(true, 30_000), make(false, 4_000), make(false, 900)];
        for (i, frame) in sent.iter().enumerate() {
            link.video(frame.clone(), 16 * i as u64, i == 0);
            link.step(Duration::from_millis(16));
        }
        link.run(Duration::from_millis(50));
        let expected: Vec<Vec<u8>> = sent
            .iter()
            .map(|f| {
                if codec == VideoCodec::Av1 {
                    without_td(f)
                } else {
                    f.clone()
                }
            })
            .collect();
        let video: Vec<&Received> = link.media.iter().filter(|m| m.pt == 96).collect();
        assert_eq!(video.len(), expected.len(), "{codec:?}: frames received");
        for (i, (got, want)) in video.iter().zip(&expected).enumerate() {
            assert!(
                got.data == *want,
                "{codec:?} frame {i}: {}",
                diff(&got.data, want)
            );
        }
    }
}

#[test]
fn marker_bit_is_on_the_last_packet_of_each_frame_only() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 30_000), 0, true);
    link.step(Duration::from_millis(16));
    link.video(h264_frame(false, 5_000), 16, false);
    link.run(Duration::from_millis(50));
    let video: Vec<&Rtp> = link.rtp.iter().filter(|p| p.pt == 96).collect();
    let mut frames: Vec<Vec<&Rtp>> = Vec::new();
    for p in video {
        match frames.last_mut() {
            Some(f) if f[0].timestamp == p.timestamp => f.push(p),
            _ => frames.push(vec![p]),
        }
    }
    assert_eq!(frames.len(), 2);
    assert!(
        frames[0].len() > 20,
        "a 30 KB keyframe is many packets: {}",
        frames[0].len()
    );
    for f in &frames {
        let (last, rest) = f.split_last().unwrap();
        assert!(last.marker, "the last packet of a frame carries the marker");
        assert!(rest.iter().all(|p| !p.marker), "no other packet does");
        assert!(
            f.windows(2).all(|w| w[1].seq == w[0].seq.wrapping_add(1)),
            "sequence numbers are gapless"
        );
    }
}

#[test]
fn rtp_timestamps_follow_the_capture_instant() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // Captured 0, 7 and 24 ms after the session start, but all handed over in one go.
    for (ms, key) in [(0, true), (7, false), (24, false)] {
        link.video(h264_frame(key, 2_000), ms, key);
    }
    link.run(Duration::from_millis(50));
    let mut stamps: Vec<u32> = Vec::new();
    for p in link.rtp.iter().filter(|p| p.pt == 96) {
        if stamps.last() != Some(&p.timestamp) {
            stamps.push(p.timestamp);
        }
    }
    assert_eq!(stamps.len(), 3);
    assert_eq!(
        stamps[1].wrapping_sub(stamps[0]),
        7 * 90,
        "7 ms is 630 ticks of 90 kHz"
    );
    assert_eq!(stamps[2].wrapping_sub(stamps[1]), 17 * 90);
    let times: Vec<u64> = link
        .media
        .iter()
        .filter(|m| m.pt == 96)
        .map(|m| m.time.numer())
        .collect();
    assert_eq!(times.len(), 3);
    assert_eq!(times[2] - times[0], 24 * 90);
}

#[test]
fn every_video_packet_asks_for_no_playout_delay() {
    let mut link = Link::connected(spec(VideoCodec::Hevc, true));
    link.video(hevc_frame(true, 10_000), 0, true);
    link.run(Duration::from_millis(30));
    let video: Vec<&Rtp> = link.rtp.iter().filter(|p| p.pt == 96).collect();
    assert!(!video.is_empty());
    for p in video {
        let (min, max) = p
            .playout
            .expect("the playout-delay extension is on every packet");
        assert!(min.is_zero() && max.is_zero());
    }
}

#[test]
fn hdr_colour_space_is_never_sent() {
    let mut link = Link::connected(spec(VideoCodec::Hevc, true));
    link.video(hevc_frame(true, 10_000), 0, true);
    link.run(Duration::from_millis(30));
    // Offered (the SDP test) but str0m has no serializer for it, and the playout delay and mid
    // extensions are there, so the block parsing is real.
    assert!(
        link.wire_ext_ids.contains(&6) && link.wire_ext_ids.contains(&4),
        "{:?}",
        link.wire_ext_ids
    );
    assert!(!link.wire_ext_ids.contains(&9), "{:?}", link.wire_ext_ids);
}

#[test]
fn nothing_is_written_before_the_peer_is_connected() {
    let mut link = Link::new(spec(VideoCodec::H264, false));
    let frame = VideoFrame {
        data: h264_frame(true, 1_000),
        capture: link.at(0),
        keyframe: true,
    };
    assert!(!link.host.write_video(link.now, frame).unwrap());
    let packet = AudioPacket {
        data: vec![1, 2, 3],
        capture: link.at(0),
        position: 0,
    };
    assert!(!link.host.write_audio(link.now, packet).unwrap());
}

#[test]
fn connecting_asks_for_a_keyframe() {
    let link = Link::connected(spec(VideoCodec::H264, false));
    assert!(link.idr.contains(&IdrReason::Connected), "{:?}", link.idr);
}

#[test]
fn pli_and_fir_reach_the_idr_callback() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 3_000), 0, true);
    link.run(Duration::from_millis(30));
    link.idr.clear();
    let mid = link.client_video.expect("video negotiated");
    for kind in [KeyframeRequestKind::Pli, KeyframeRequestKind::Fir] {
        link.client
            .direct_api()
            .stream_rx_by_mid(mid, None)
            .expect("receiving video")
            .request_keyframe(kind);
        link.run(Duration::from_millis(30));
    }
    assert!(link.idr.contains(&IdrReason::Pli), "{:?}", link.idr);
    assert!(link.idr.contains(&IdrReason::Fir), "{:?}", link.idr);
}

#[test]
fn opus_packets_pass_through_untouched() {
    for spec in [
        spec(VideoCodec::H264, false),
        surround_spec(6),
        surround_spec(8),
    ] {
        let channels = spec.audio.channels;
        let mut link = Link::connected(spec);
        // Not valid Opus: the transport must not look inside.
        let packets: Vec<Vec<u8>> = [120, 300, 640]
            .iter()
            .enumerate()
            .map(|(i, n)| filler(*n, 20 + i as u8))
            .collect();
        for (i, p) in packets.iter().enumerate() {
            link.audio(p.clone(), 10 * i as u64, 480 * i as u64);
            link.step(Duration::from_millis(10));
        }
        link.run(Duration::from_millis(40));
        let got: Vec<&Vec<u8>> = link
            .media
            .iter()
            .filter(|m| m.pt == 111)
            .map(|m| &m.data)
            .collect();
        assert_eq!(got.len(), packets.len(), "{channels} channels");
        for (g, p) in got.iter().zip(&packets) {
            assert!(*g == p, "{channels} channels: {}", diff(g, p));
        }
    }
}

/// The RTP times of the Opus packets the client has received so far.
fn audio_times(link: &Link) -> Vec<u64> {
    link.media
        .iter()
        .filter(|m| m.pt == 111)
        .map(|m| m.time.numer())
        .collect()
}

#[test]
fn audio_packets_stamped_with_one_capture_instant_still_advance_the_rtp_clock() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // 5 ms CELT packets (240 samples) read in pairs, as the audio thread does from 10 ms WASAPI chunks.
    let packet = |n: u8| vec![0xE8, n, n];
    for i in 0..8u64 {
        link.audio(packet(i as u8), 10 * (i / 2), 240 * i);
        link.step(Duration::from_millis(5));
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 8);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], 240, "{times:?}");
    }
}

#[test]
fn a_late_wake_that_drops_nothing_delivers_its_packets_contiguously() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let packet = |n: u8| vec![0xE8, n, n];
    link.audio(packet(0), 0, 0);
    link.step(Duration::from_millis(5));
    // The thread wakes 25 ms late, under the capture's 30 ms trim limit, and reads the five
    // packets that built up at once: one capture instant, five consecutive positions.
    for i in 1..=5u64 {
        link.audio(packet(i as u8), 30, 240 * i);
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 6);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], 240, "{times:?}");
    }
}

#[test]
fn a_late_wake_that_trimmed_its_backlog_is_a_gap_as_long_as_the_trim() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let packet = |n: u8| vec![0xE8, n, n];
    link.audio(packet(0), 0, 0);
    link.step(Duration::from_millis(5));
    // 100 ms late: the capture trims 90 ms (4320 samples) of the 100 ms waiting and keeps two
    // packets. The audio thread adds the trim to the position, so the receiver conceals it,
    // and the RTP time is back in step with the wall (105 ms in, 5040 ticks).
    link.audio(packet(1), 105, 240 + 4320);
    link.audio(packet(2), 105, 240 + 4320 + 240);
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 3);
    assert_eq!(times[1] - times[0], 240 + 4320, "{times:?}");
    assert_eq!(times[2] - times[1], 240, "{times:?}");
}

#[test]
fn lost_audio_is_a_gap_of_exactly_its_length_at_any_size() {
    // Around the old 40 ms wall-time threshold: every gap, long or short, is what was lost.
    for lost in [0u64, 1, 1919, 1920, 1921, 9600] {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let packet = |n: u8| vec![0xE8, n, n];
        link.audio(packet(0), 0, 0);
        link.step(Duration::from_millis(5));
        link.audio(packet(1), 5, 240 + lost);
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len(), 2, "{lost}");
        assert_eq!(times[1] - times[0], 240 + lost, "{lost}: {times:?}");
    }
}

#[test]
fn a_packet_dropped_upstream_still_advances_the_rtp_clock() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let packet = |n: u8| vec![0xE8, n, n];
    // The queue was full, or the peer not yet connected, for the packets at 240 and 480:
    // the audio thread numbered them anyway.
    link.audio(packet(0), 0, 0);
    link.step(Duration::from_millis(5));
    link.audio(packet(3), 20, 720);
    link.step(Duration::from_millis(5));
    link.audio(packet(4), 25, 960);
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 3);
    assert_eq!(times[1] - times[0], 720, "{times:?}");
    assert_eq!(times[2] - times[1], 240, "{times:?}");
}

#[test]
fn packets_the_host_cannot_measure_do_not_repeat_a_timestamp() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // A one-byte code 3 packet has no frame count: nothing in it says how long it is.
    for i in 0..4u64 {
        link.audio(vec![0xEB], 5 * i, 240 * i);
        link.step(Duration::from_millis(5));
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len(), 4);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], 240, "{times:?}");
    }
}

#[test]
fn a_slow_device_clock_over_an_hour_never_jumps_the_rtp_clock() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    // The device delivers 48 kHz samples 0.05% slower than wall time (1.8 s an hour), with
    // one packet sampled every 3 s of audio, ten times what the old wall-time rule
    // tolerated before resyncing. RTP time must differ by exactly the sample position.
    let step = 144_000u64;
    let packets = 1200u64;
    for i in 0..packets {
        let position = i * step;
        let wall_ms = position / 48 * 10_005 / 10_000;
        link.audio(vec![0xE8, 1, 2], wall_ms, position);
        for _ in 0..60 {
            link.step(Duration::from_millis(50));
        }
    }
    link.run(Duration::from_millis(40));
    let times = audio_times(&link);
    assert_eq!(times.len() as u64, packets);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], step, "{pair:?}");
    }
}

#[test]
fn control_and_gamepad_channels_carry_messages_both_ways() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    assert!(
        link.host
            .send_control(b"{\"type\":\"hello\"}", false)
            .unwrap()
    );
    assert!(link.host.send_gamepad(&[1, 2, 3, 4], true).unwrap());
    link.run(Duration::from_millis(30));
    assert!(
        link.channel_data
            .contains(&("control".into(), b"{\"type\":\"hello\"}".to_vec()))
    );
    assert!(
        link.channel_data
            .contains(&("gamepad".into(), vec![1, 2, 3, 4]))
    );

    link.client_writes("control", b"{\"type\":\"idr\"}");
    link.client_writes("gamepad", &[9, 8, 7]);
    link.run(Duration::from_millis(30));
    assert!(link.host_events.contains(&TransportEvent::Control {
        data: b"{\"type\":\"idr\"}".to_vec(),
        binary: true
    }));
    assert!(link.host_events.contains(&TransportEvent::Gamepad {
        data: vec![9, 8, 7],
        binary: true
    }));
}

#[test]
fn the_input_channel_is_accepted_only_when_authenticated() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.client_opens("input");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelRejected("input".into())),
        "{:?}",
        link.host_events
    );
    assert!(
        !link
            .host_events
            .contains(&TransportEvent::ChannelOpen("input".into()))
    );
    assert!(
        link.closed_by_host.contains(&"input".to_string()),
        "the refused channel is closed"
    );

    link.authenticated.store(true, Ordering::Release);
    let input = link.client_opens("input");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelOpen("input".into())),
        "{:?}",
        link.host_events
    );
    link.client_writes_to(input, &[5, 6, 7]);
    link.run(Duration::from_millis(30));
    assert!(link.host_events.contains(&TransportEvent::Input {
        data: vec![5, 6, 7],
        binary: true
    }));

    // Revoking the session closes the channel on the next message.
    link.authenticated.store(false, Ordering::Release);
    link.client_writes_to(input, &[1]);
    link.run(Duration::from_millis(30));
    assert!(!link.host_events.contains(&TransportEvent::Input {
        data: vec![1],
        binary: true
    }));
    let rejected = link
        .host_events
        .iter()
        .filter(|e| **e == TransportEvent::ChannelRejected("input".into()))
        .count();
    assert_eq!(rejected, 2, "the revoked channel is refused and closed");
}

#[test]
fn unknown_channel_labels_are_closed() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.authenticated.store(true, Ordering::Release);
    link.client_opens("shell");
    // A second channel that claims a host label is refused too.
    link.client_opens("control");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelRejected("shell".into())),
        "{:?}",
        link.host_events
    );
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelRejected("control".into())),
        "{:?}",
        link.host_events
    );
    assert!(link.closed_by_host.contains(&"shell".to_string()));
}

fn nack_run(rtx: bool) -> (Vec<Rtp>, usize) {
    let mut s = spec(VideoCodec::H264, false);
    s.rtx = rtx;
    let mut link = Link::connected(s);
    link.drop_rtp = Some(link.rtp_seen + 3);
    link.video(h264_frame(true, 20_000), 0, true);
    link.run(Duration::from_millis(600));
    let frames = link.media.iter().filter(|m| m.pt == 96).count();
    (link.rtp, frames)
}

#[test]
fn a_lost_packet_is_resent_only_when_rtx_is_negotiated() {
    let (with_rtx, _) = nack_run(true);
    assert!(
        with_rtx.iter().any(|p| p.pt == 97),
        "with RTX the NACK is answered"
    );

    // Without an RTX payload type str0m marks every packet non-nackable, so a NACK would be
    // ignored; the offer doesn't even negotiate NACK, so receivers send a PLI instead.
    let (without, frames) = nack_run(false);
    assert!(
        without.iter().all(|p| p.pt == 96),
        "no retransmission without RTX"
    );
    let mut seqs: Vec<u16> = without.iter().map(|p| p.seq).collect();
    let total = seqs.len();
    seqs.dedup();
    assert_eq!(seqs.len(), total, "no duplicate sequence numbers");
    assert_eq!(frames, 0, "the frame with the hole is never completed");
}

#[test]
fn av1_temporal_delimiters_are_removed_and_the_rest_kept() {
    let strip = |frame: &[u8]| strip_av1_temporal_delimiters(frame.to_vec()).unwrap();
    let keyframe = av1_frame(true, 300);
    assert_eq!(strip(&keyframe), &keyframe[2..]);
    let plain = obu(6, &filler(200, 1));
    assert_eq!(strip(&plain), plain);
    // A TD in the middle (two temporal units in one buffer) goes too.
    let two = [plain.clone(), TD.to_vec(), plain.clone()].concat();
    assert_eq!(strip(&two), [plain.clone(), plain].concat());
    // An OBU without a size field takes the rest.
    let tail = [obu(1, &[1, 2, 3]), vec![(6 << 3), 9, 9, 9]].concat();
    assert_eq!(strip(&tail), tail);
    // Malformed input is an error, not a guess.
    assert!(strip_av1_temporal_delimiters(vec![0x32, 0x40, 1, 2]).is_err());
    assert!(strip_av1_temporal_delimiters(vec![0x80, 0]).is_err());
}

#[test]
fn av1_frames_keep_their_buffer_when_delimiters_go() {
    // The frame moves on to str0m in the buffer the encoder filled: no copy.
    for frame in [av1_frame(true, 4_000), obu(6, &filler(4_000, 1))] {
        let at = frame.as_ptr();
        let stripped = strip_av1_temporal_delimiters(frame).unwrap();
        assert_eq!(stripped.as_ptr(), at);
    }
}

fn rtp_packet(len: usize) -> Outgoing {
    let mut data = vec![0x60u8; len];
    data[0] = 0x80;
    Outgoing {
        destination: CLIENT.parse().unwrap(),
        data,
        paced: true,
    }
}

fn control_packet() -> Outgoing {
    Outgoing {
        destination: CLIENT.parse().unwrap(),
        data: vec![0x16; 100],
        paced: false,
    }
}

const PERIOD: Duration = Duration::from_micros(16_667);

/// Paced at a configured cap, in kbps, on a route of unknown speed.
fn capped(kbps: i64) -> Pacing {
    Pacing::Paced {
        configured_kbps: kbps,
        link_bps: 0,
        wireless: false,
    }
}

#[test]
fn the_loss_switch_drops_only_video_and_about_the_share_asked_for() {
    let queue = || SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()).with_loss(10);
    let (mut lossy, mut clean) = (
        queue(),
        SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()),
    );
    let video = rtp_packet(1200);
    let lost = (0..100_000)
        .filter(|_| lossy.lost_in_the_air(&video))
        .count();
    assert!((800..=1200).contains(&lost), "{lost} of 100000 at 1%");
    assert!((0..1000).all(|_| !lossy.lost_in_the_air(&control_packet())));
    assert!((0..1000).all(|_| !clean.lost_in_the_air(&video)));
    // The seed is fixed: a second queue loses the same packets.
    let mut second = queue();
    let again = (0..100_000)
        .filter(|_| second.lost_in_the_air(&video))
        .count();
    assert_eq!(again, lost);
}

#[test]
fn simulated_losses_are_counted_and_reported_only_with_the_switch_on() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let (handle, _rx, _idr) = detached(TransportOptions {
        loss_permille: 500,
        ..TransportOptions::default()
    });
    let mut queue =
        SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()).with_loss(500);
    let to = receiver.local_addr().unwrap();
    for _ in 0..200 {
        queue.push(Outgoing {
            destination: to,
            ..rtp_packet(100)
        });
    }
    for _ in 0..20 {
        queue.push(Outgoing {
            destination: to,
            ..control_packet()
        });
    }
    send_due(&socket, &mut queue, &mut Batch::default(), &handle.shared);
    let stats = handle.stats();
    assert!(queue.is_empty());
    assert!(
        (50..=150).contains(&stats.simulated_losses),
        "{} of 200 video datagrams at 50%",
        stats.simulated_losses
    );
    assert_eq!(stats.simulated_losses + stats.datagrams_sent, 220);
    assert_eq!(handle.simulated_losses(), Some(stats.simulated_losses));
    let (clean, _rx, _idr) = detached(TransportOptions::default());
    assert_eq!(clean.simulated_losses(), None);
}

#[test]
fn the_pacer_sends_one_millisecond_of_media_per_group() {
    let now = Instant::now();
    // 12 Mbps is 1500 bytes of wire traffic a millisecond: one 1100-byte datagram (1166 on the
    // wire) fits, a second does not.
    let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, now);
    for _ in 0..5 {
        queue.push(rtp_packet(1100));
    }
    queue.push(control_packet());
    let first = queue.next_batch(now);
    assert_eq!(
        first.packets.len(),
        2,
        "the control datagram and one media datagram"
    );
    assert!(!first.packets[0].paced && first.packets[1].paced);
    queue.sent(now, &first);
    assert!(
        queue.next_batch(now).packets.is_empty(),
        "the next group waits"
    );
    let due = queue.due(now).unwrap();
    assert!(
        due > now && due <= now + Duration::from_millis(1),
        "{:?}",
        due - now
    );
    let second = queue.next_batch(due);
    assert_eq!(second.paced_packets, 1);
}

#[test]
fn a_large_frame_raises_the_wireless_pacing_rate_to_what_it_needs() {
    let now = Instant::now();
    let wireless = Pacing::Paced {
        configured_kbps: 0,
        link_bps: 0,
        wireless: true,
    };
    // A 1 Mbps stream is paced at 2 Mbps (one datagram a group) until a frame needs more.
    let packets = |note: Option<usize>| {
        let mut queue = SendQueue::new(wireless, 1_000, PERIOD, false, now);
        if let Some(bytes) = note {
            queue.note_frame(bytes);
        }
        for _ in 0..40 {
            queue.push(rtp_packet(1100));
        }
        queue.next_batch(now).paced_packets
    };
    assert_eq!(packets(None), 1);
    // 60 datagrams of 1166 bytes per frame period is 33.6 Mbps; with the quarter on top, 42.
    assert_eq!(packets(Some(60 * 1166)), 4);
}

#[test]
fn the_ceiling_is_a_real_cap() {
    let now = Instant::now();
    let rate = |pacing: Pacing, frame: usize| {
        let mut queue = SendQueue::new(pacing, 20_000, PERIOD, false, now);
        queue.note_frame(frame);
        queue.rate()
    };
    // A 300 KB frame (144 Mbps at 60 fps) on a 100 Mbps link is held to 4/5 of the link.
    let link = |link_bps, wireless| Pacing::Paced {
        configured_kbps: 0,
        link_bps,
        wireless,
    };
    let huge = 300_000;
    assert_eq!(rate(link(100_000_000, false), huge), 80_000_000);
    assert_eq!(rate(link(100_000_000, true), huge), 80_000_000);
    // No link known: wired 800 Mbps, and wireless is clamped to the same.
    assert_eq!(rate(link(0, false), huge), 800_000_000);
    assert_eq!(rate(link(0, true), huge * 10), 800_000_000);
    // A configured cap holds, whatever the frame needs.
    assert_eq!(rate(capped(50_000), huge), 50_000_000);
    // Never below what the stream itself needs (1.1 times its bitrate).
    assert_eq!(rate(capped(5_000), 0), 22_000_000);
    assert_eq!(rate(Pacing::Off, huge), u64::MAX);
}

#[test]
fn a_late_wake_catches_up_but_a_stall_earns_a_bounded_burst() {
    let now = Instant::now();
    // 12 Mbps with 1100-byte datagrams: one datagram, 0.78 ms of wire time, per group.
    let groups_at = |late: Duration| {
        let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, now);
        for _ in 0..200 {
            queue.push(rtp_packet(1100));
        }
        let first = queue.next_batch(now);
        queue.sent(now, &first);
        // The timer fires `late` after the first group was due again.
        let at = queue.due(now).unwrap() + late;
        let mut sent = 0;
        loop {
            let batch = queue.next_batch(at);
            if batch.paced_packets == 0 {
                return sent;
            }
            queue.sent(at, &batch);
            sent += batch.paced_packets;
        }
    };
    let on_time = groups_at(Duration::ZERO);
    let late_2ms = groups_at(Duration::from_millis(2));
    let stalled = groups_at(Duration::from_millis(500));
    assert_eq!(on_time, 1);
    assert!(
        late_2ms >= 3,
        "a 2 ms late wake sends the groups it missed: {late_2ms}"
    );
    assert!(
        (2..=8).contains(&stalled),
        "a 500 ms stall earns {CATCH_UP_CREDIT:?} of credit, not a burst of 400 groups: {stalled}"
    );
}

#[test]
fn a_backlog_beyond_the_cap_is_detected_but_a_lone_keyframe_is_not() {
    let now = Instant::now();
    // 80 Mbps: 50 ms of backlog is 500 KB.
    let mut queue = SendQueue::new(capped(80_000), 20_000, PERIOD, false, now);
    // A 1 MB keyframe is twice the 50 ms budget, but it is one frame.
    queue.note_frame(1_000_000);
    for _ in 0..860 {
        queue.push(rtp_packet(1100));
    }
    assert!(!queue.backlog_exceeded());
    // Two more of them (3 x frame > 2 x frame) is a pile-up.
    for _ in 0..1800 {
        queue.push(rtp_packet(1100));
    }
    assert!(queue.backlog_exceeded());
    assert_eq!(queue.drop_paced(), 2660);
    assert!(!queue.backlog_exceeded() && queue.is_empty());
    // Small frames: the 50 ms budget alone decides.
    queue.note_frame(20_000);
    for _ in 0..400 {
        queue.push(rtp_packet(1100));
    }
    assert!(!queue.backlog_exceeded());
    for _ in 0..200 {
        queue.push(rtp_packet(1100));
    }
    assert!(queue.backlog_exceeded(), "{}", queue.paced_wire_bytes);
}

#[test]
fn pacing_off_hands_everything_over_at_once() {
    let now = Instant::now();
    let mut queue = SendQueue::new(Pacing::Off, 0, PERIOD, false, now);
    for _ in 0..50 {
        queue.push(rtp_packet(1100));
    }
    assert!(!queue.backlog_exceeded());
    assert_eq!(queue.next_batch(now).packets.len(), 50);
    assert!(queue.is_empty());
}

fn detached(
    options: TransportOptions,
) -> (
    MediaHandle,
    mpsc::Receiver<Command>,
    Arc<Mutex<Vec<IdrReason>>>,
) {
    let idr = Arc::new(Mutex::new(Vec::new()));
    let sink = idr.clone();
    let hooks = Hooks::new(move |r| sink.lock().unwrap().push(r), |_| {});
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        wake: Signal::new().unwrap(),
        authenticated: Arc::new(AtomicBool::new(false)),
        pending_video: AtomicUsize::new(0),
        pending_audio: AtomicUsize::new(0),
        pending_datagrams: AtomicUsize::new(0),
        need_keyframe: AtomicBool::new(true),
        last_idr_ms: AtomicU64::new(0),
        epoch: Instant::now(),
        counters: Counters::default(),
        hooks,
        options,
        peer: std::sync::Mutex::new(None),
    });
    (
        MediaHandle {
            tx,
            shared,
            threads: Vec::new(),
            local: HOST.parse().unwrap(),
            audio_layout: None,
        },
        rx,
        idr,
    )
}

fn frame(keyframe: bool) -> VideoFrame {
    VideoFrame {
        data: vec![0; 10],
        capture: Instant::now(),
        keyframe,
    }
}

#[test]
fn a_full_frame_queue_drops_and_asks_for_a_keyframe_without_blocking() {
    let (handle, rx, idr) = detached(TransportOptions {
        video_queue: 2,
        ..TransportOptions::default()
    });
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    assert_eq!(handle.submit_video(frame(false)), Submit::Queued);
    // The media thread isn't draining: the third frame is dropped at once.
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(idr.lock().unwrap().as_slice(), [IdrReason::QueueFull]);
    // Later delta frames would reference the dropped one, so they are dropped too.
    while let Ok(Command::Video(_)) = rx.try_recv() {
        handle.shared.pending_video.fetch_sub(1, Ordering::AcqRel);
    }
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(
        handle.submit_video(frame(true)),
        Submit::Queued,
        "a keyframe goes through"
    );
    assert_eq!(
        handle.submit_video(frame(false)),
        Submit::Queued,
        "and delta frames follow it"
    );
    assert_eq!(handle.stats().video_dropped, 2);
}

#[test]
fn audio_queue_overflow_drops_without_blocking() {
    let (handle, _rx, idr) = detached(TransportOptions {
        audio_queue: 1,
        ..TransportOptions::default()
    });
    let packet = || AudioPacket {
        data: vec![1],
        capture: Instant::now(),
        position: 0,
    };
    assert_eq!(handle.submit_audio(packet()), Submit::Queued);
    assert_eq!(handle.submit_audio(packet()), Submit::Dropped);
    assert!(
        idr.lock().unwrap().is_empty(),
        "a lost audio packet needs no keyframe"
    );
    assert_eq!(handle.stats().audio_dropped, 1);
}

#[test]
fn the_audio_sender_shares_the_handles_queue_and_counters() {
    let (handle, rx, _idr) = detached(TransportOptions {
        audio_queue: 1,
        ..TransportOptions::default()
    });
    assert!(
        handle.audio_sender().is_none(),
        "no layout, no audio end: stereo and surround must both say what they encode"
    );
    let layout = OpusLayout::select(6, false, None).unwrap();
    let handle = handle.with_audio_layout(layout.clone());
    let sender = handle.audio_sender().unwrap();
    assert_eq!(sender.layout(), &layout);
    let packet = || AudioPacket {
        data: vec![7],
        capture: Instant::now(),
        position: 0,
    };
    assert_eq!(sender.submit(packet()), Submit::Queued);
    assert_eq!(
        handle.submit_audio(packet()),
        Submit::Dropped,
        "one queue, whichever end fills it"
    );
    assert!(matches!(rx.try_recv(), Ok(Command::Audio(p)) if p.data == [7]));
    assert_eq!(handle.stats().audio_dropped, 1);
    // The sender outlives nothing: with the handle's threads gone a packet is dropped.
    drop(rx);
    handle.shared.pending_audio.store(0, Ordering::Release);
    assert_eq!(sender.submit(packet()), Submit::Dropped);
    assert_eq!(handle.stats().audio_dropped, 2);
}

#[test]
fn the_threads_start_and_stop_on_a_real_socket() {
    let spec = spec(VideoCodec::H264, false);
    let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
    let local = socket.local_addr().unwrap();
    let (mut rtc, offer) = host_offer(&spec, local);
    let mut peer = client(&spec, CLIENT.parse().unwrap());
    let text = answer(&mut peer, &offer.sdp);
    let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
    let session = Session {
        rtc,
        ids,
        spec: &spec,
        authenticated: Arc::new(AtomicBool::new(false)),
    };
    let mut handle = MediaHandle::spawn(
        socket,
        local,
        session,
        TransportOptions::default(),
        Hooks::new(|_| {}, |_| {}),
    )
    .unwrap();
    // Not connected: the frame is accepted by the queue and then discarded by the thread.
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    let started = Instant::now();
    handle.close();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A STUN binding request (RFC 8489) with only a USERNAME: no integrity, so no real peer.
fn forged_stun(username: &str) -> Vec<u8> {
    let mut attr = username.as_bytes().to_vec();
    let length = attr.len() as u16;
    attr.resize(attr.len().div_ceil(4) * 4, 0);
    let mut message = vec![0, 1];
    message.extend(((attr.len() + 4) as u16).to_be_bytes());
    message.extend([0x21, 0x12, 0xa4, 0x42]);
    message.extend([7u8; 12]);
    message.extend([0, 6]);
    message.extend(length.to_be_bytes());
    message.extend(attr);
    message
}

#[test]
fn audio_goes_out_unpaced_beside_a_paced_keyframe() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 60_000), 0, true);
    link.audio(filler(160, 1), 0, 0);
    // str0m hands a frame over across a few polls; collect what the engine releases.
    let mut out = Vec::new();
    for ms in 0..20 {
        link.host
            .advance(link.now + Duration::from_millis(ms))
            .unwrap();
        out.extend(link.host.take_outgoing());
    }
    let by_pt = |pt: u8| -> Vec<&Outgoing> {
        out.iter()
            .filter(|o| is_media_rtp(&o.data) && o.data[1] & 0x7f == pt)
            .collect()
    };
    let (video, audio) = (by_pt(VIDEO_PT), by_pt(AUDIO_PT));
    assert!(
        video.len() > 20 && audio.len() == 1,
        "{} {}",
        video.len(),
        audio.len()
    );
    assert!(video.iter().all(|o| o.paced), "video waits for the pacer");
    assert!(audio.iter().all(|o| !o.paced), "audio does not");
    // Through a queue still holding the keyframe, the audio leaves in the first batch.
    let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, link.now);
    for o in out {
        queue.push(o);
    }
    let first = queue.next_batch(link.now);
    assert!(
        first
            .packets
            .iter()
            .any(|o| !o.paced && o.data[1] & 0x7f == AUDIO_PT)
    );
    assert!(first.paced_packets <= 2, "{}", first.paced_packets);
}

#[test]
fn foreign_and_spoofed_datagrams_cannot_end_or_hijack_the_session() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let client: SocketAddr = CLIENT.parse().unwrap();
    let attacker: SocketAddr = "203.0.113.9:4444".parse().unwrap();
    let rtp = {
        let mut p = vec![0x80, 96, 0, 1, 0, 0, 0, 1, 0xde, 0xad, 0xbe, 0xef];
        p.extend(filler(200, 1));
        p
    };
    let rtcp = vec![0x80, 201, 0, 1, 1, 2, 3, 4];
    let dtls = [
        vec![22, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4],
        filler(4, 1),
    ]
    .concat();
    let own = format!("{}:abcd", link.host_ufrag);
    let junk: Vec<Vec<u8>> = vec![
        rtp,
        rtcp,
        dtls,
        vec![0xff; 3000],
        vec![],
        vec![0],
        forged_stun("nobody:else"),
        forged_stun(&own),
    ];
    let now = link.now;
    for from in [attacker, client] {
        for data in &junk {
            // Whatever it is called, it is neither fatal nor a reason to stop.
            link.host.handle_datagram(now, from, data);
        }
    }
    // Foreign sources are refused outright, apart from STUN carrying our own ufrag.
    for data in &junk[..7] {
        assert_eq!(
            link.host.handle_datagram(now, attacker, data),
            Datagram::Rejected,
            "{:?}",
            &data[..data.len().min(8)]
        );
    }
    link.host.advance(link.now).unwrap();
    let out = link.host.take_outgoing();
    assert!(
        out.iter().all(|o| o.destination == client),
        "nothing is sent to the stranger: {:?}",
        out.iter().map(|o| o.destination).collect::<Vec<_>>()
    );
    assert!(link.host.is_connected() && !link.host.is_closed());
    let (rejected, _) = link.host.dropped_datagrams();
    assert!(rejected >= 14, "{rejected}");

    // The real client still gets its picture, to the address it nominated.
    link.video(h264_frame(true, 5_000), 0, true);
    link.run(Duration::from_millis(40));
    assert!(link.media.iter().any(|m| m.pt == 96), "video still flows");
    assert!(
        !link.host_events.contains(&TransportEvent::Closed)
            && !link.host_events.contains(&TransportEvent::Disconnected),
        "{:?}",
        link.host_events
    );
}

#[test]
fn an_ice_outage_is_surfaced_and_ends_the_session_after_the_timeout() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.video(h264_frame(true, 2_000), 0, true);
    link.run(Duration::from_millis(20));
    link.host_events.clear();
    link.idr.clear();
    link.partition = true;
    let started = link.now;
    while !link.host.is_disconnected() {
        link.step(Duration::from_millis(100));
        assert!(
            link.now - started < Duration::from_secs(60),
            "ICE never noticed the outage"
        );
    }
    let went_down = link.now;
    assert!(link.host_events.contains(&TransportEvent::Disconnected));
    assert!(!link.host.is_connected() && !link.host.is_closed());
    // While down, nothing is written, so the next frames wait for a keyframe.
    let frame = VideoFrame {
        data: h264_frame(true, 1_000),
        capture: link.at(0),
        keyframe: true,
    };
    assert!(!link.host.write_video(link.now, frame).unwrap());

    while !link.host.is_closed() {
        link.step(Duration::from_millis(50));
        assert!(
            link.now - went_down < Duration::from_secs(10),
            "never closed"
        );
    }
    let waited = link.now - went_down;
    assert!(
        waited >= DISCONNECT_TIMEOUT - Duration::from_millis(200)
            && waited <= DISCONNECT_TIMEOUT + Duration::from_millis(300),
        "{waited:?}"
    );
}

#[test]
fn an_outage_that_ends_in_time_resumes_with_a_keyframe_request() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.partition = true;
    let started = link.now;
    while !link.host.is_disconnected() {
        link.step(Duration::from_millis(100));
        assert!(link.now - started < Duration::from_secs(60));
    }
    link.host_events.clear();
    link.idr.clear();
    link.partition = false;
    for _ in 0..25 {
        link.step(Duration::from_millis(100));
        if link.host.is_connected() {
            break;
        }
    }
    assert!(link.host.is_connected(), "{:?}", link.host_events);
    assert!(link.host_events.contains(&TransportEvent::Reconnected));
    assert!(link.idr.contains(&IdrReason::Reconnected), "{:?}", link.idr);
}

#[test]
fn a_channel_closed_by_the_client_is_reported_and_stops_sends() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.authenticated.store(true, Ordering::Release);
    let input = link.client_opens("input");
    link.run(Duration::from_millis(60));
    assert!(
        link.host_events
            .contains(&TransportEvent::ChannelOpen("input".into()))
    );
    assert!(link.host.send_control(b"x", false).unwrap());
    for label in ["control", "input"] {
        let id = if label == "input" {
            input
        } else {
            link.client_channel(label)
        };
        link.client.direct_api().close_data_channel(id);
        link.run(Duration::from_millis(60));
        assert!(
            link.host_events.contains(&TransportEvent::ChannelClosed {
                label: label.into()
            }),
            "{label}: {:?}",
            link.host_events
        );
    }
    assert!(
        !link.host.send_control(b"x", false).unwrap(),
        "a closed channel takes nothing"
    );
    // The gamepad channel is untouched.
    assert!(link.host.send_gamepad(&[1], true).unwrap());
}

#[test]
fn the_input_channel_must_be_ordered_and_reliable() {
    use str0m::channel::ChannelConfig;
    let cases: [(&str, bool, Reliability, bool); 4] = [
        ("ordered and reliable", true, Reliability::Reliable, true),
        ("unordered", false, Reliability::Reliable, false),
        (
            "unreliable",
            true,
            Reliability::MaxRetransmits { retransmits: 0 },
            false,
        ),
        (
            "lossy and unordered",
            false,
            Reliability::MaxPacketLifetime { lifetime: 50 },
            false,
        ),
    ];
    for (name, ordered, reliability, accepted) in cases {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.authenticated.store(true, Ordering::Release);
        let mut api = link.client.sdp_api();
        api.add_channel_with_config(ChannelConfig {
            label: "input".into(),
            ordered,
            reliability,
            ..Default::default()
        });
        assert!(api.apply().is_none());
        link.run(Duration::from_millis(60));
        let open = link
            .host_events
            .contains(&TransportEvent::ChannelOpen("input".into()));
        let rejected = link
            .host_events
            .contains(&TransportEvent::ChannelRejected("input".into()));
        assert_eq!((open, rejected), (accepted, !accepted), "{name}");
    }
}

#[test]
fn messages_that_cannot_be_sent_are_counted() {
    let (handle, _rx, _idr) = detached(TransportOptions::default());
    let shared = &handle.shared;
    note_channel_send(shared, "control", Ok(true));
    note_channel_send(shared, "control", Ok(false));
    note_channel_send(shared, "gamepad", Ok(false));
    note_channel_send(shared, "gamepad", Err(anyhow!("channel write failed")));
    let stats = handle.stats();
    assert_eq!((stats.channel_sent, stats.channel_dropped), (1, 3));
    // Before the channels open, the engine itself says no rather than pretending.
    let mut link = Link::new(spec(VideoCodec::H264, false));
    assert!(!link.host.send_control(b"x", false).unwrap());
    assert!(!link.host.send_gamepad(&[1], true).unwrap());
}

#[test]
fn no_frame_but_a_keyframe_leaves_before_the_first_one() {
    let (handle, rx, _idr) = detached(TransportOptions::default());
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(handle.stats().video_dropped, 1);
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    assert_eq!(handle.submit_video(frame(false)), Submit::Queued);
    assert_eq!(rx.try_iter().count(), 2);
}

fn frame_of(data: Vec<u8>, keyframe: bool) -> VideoFrame {
    VideoFrame {
        data,
        capture: Instant::now(),
        keyframe,
    }
}

#[test]
fn stale_deltas_after_a_drop_are_discarded_by_the_thread() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let (handle, _rx, _idr) = detached(TransportOptions::default());
    let shared = &handle.shared;
    shared.pending_video.store(3, Ordering::Release);
    let command = |keyframe| Command::Video(frame_of(h264_frame(keyframe, 800), keyframe));
    handle_command(&mut link.host, shared, command(false));
    let stats = handle.stats();
    assert_eq!((stats.video_frames, stats.video_dropped), (0, 1));
    handle_command(&mut link.host, shared, command(true));
    assert!(!shared.need_keyframe.load(Ordering::Acquire));
    handle_command(&mut link.host, shared, command(false));
    let stats = handle.stats();
    assert_eq!((stats.video_frames, stats.video_dropped), (2, 1));
}

#[test]
fn a_pacer_pile_up_drops_the_backlog_and_asks_for_a_keyframe() {
    let (handle, _rx, idr) = detached(TransportOptions::default());
    let shared = &handle.shared;
    shared.need_keyframe.store(false, Ordering::Release);
    let now = Instant::now();
    let mut queue = SendQueue::new(capped(8_000), 5_000, PERIOD, false, now);
    queue.note_frame(10_000);
    for _ in 0..30 {
        queue.push(rtp_packet(1100));
    }
    enforce_backlog(&mut queue, shared);
    assert!(idr.lock().unwrap().is_empty(), "30 KB is no backlog yet");
    for _ in 0..300 {
        queue.push(rtp_packet(1100));
    }
    enforce_backlog(&mut queue, shared);
    assert_eq!(idr.lock().unwrap().as_slice(), [IdrReason::PacerBacklog]);
    assert!(shared.need_keyframe.load(Ordering::Acquire));
    assert_eq!(handle.stats().paced_dropped, 330);
    assert!(queue.is_empty());
}

#[test]
fn a_datagram_burst_beyond_the_queue_is_dropped_not_buffered() {
    let (handle, rx, _idr) = detached(TransportOptions {
        datagram_queue: 3,
        ..TransportOptions::default()
    });
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let target = socket.local_addr().unwrap();
    let (tx, shared) = (handle.tx.clone(), handle.shared.clone());
    let thread = std::thread::spawn(move || receive_loop(socket, tx, shared));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    for i in 0..10u8 {
        sender.send_to(&[i; 40], target).unwrap();
    }
    wait_for("ten datagrams", || handle.stats().datagrams_received == 10);
    let stats = handle.stats();
    assert_eq!(stats.datagrams_dropped, 7, "{stats:?}");
    assert_eq!(rx.try_iter().count(), 3, "only the first three are queued");
    handle.shared.stop.store(true, Ordering::Release);
    thread.join().unwrap();
}

fn ice(link: &mut Link, state: IceConnectionState) {
    link.host
        .on_event(link.now, Event::IceConnectionStateChange(state));
}

#[test]
fn checking_after_an_outage_is_not_back_and_does_not_reset_the_timeout() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.collect_host();
    link.host_events.clear();
    link.idr.clear();
    ice(&mut link, IceConnectionState::Disconnected);
    let down_at = link.host.disconnected_since.expect("down");
    link.now += Duration::from_secs(1);
    ice(&mut link, IceConnectionState::Checking);
    assert_eq!(
        link.host.disconnected_since,
        Some(down_at),
        "the clock runs on"
    );
    assert!(link.host.is_disconnected());
    link.collect_host();
    assert_eq!(link.host_events, [TransportEvent::Disconnected]);
    assert!(link.idr.is_empty(), "{:?}", link.idr);
    // Only a usable pair is back, and it asks for exactly one keyframe.
    ice(&mut link, IceConnectionState::Completed);
    ice(&mut link, IceConnectionState::Connected);
    link.collect_host();
    assert!(!link.host.is_disconnected());
    assert_eq!(
        link.host_events,
        [TransportEvent::Disconnected, TransportEvent::Reconnected]
    );
    assert_eq!(link.idr, [IdrReason::Reconnected]);
}

#[test]
fn checking_after_connecting_counts_as_down() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    link.collect_host();
    link.host_events.clear();
    // A Wi-Fi handover: Completed, then ICE goes looking for a new pair.
    ice(&mut link, IceConnectionState::Completed);
    ice(&mut link, IceConnectionState::Checking);
    assert!(link.host.is_disconnected());
    assert!(!link.host.is_connected());
    link.collect_host();
    assert_eq!(link.host_events, [TransportEvent::Disconnected]);
    // The timeout runs from the first sign of trouble and ends the session.
    link.now += DISCONNECT_TIMEOUT + Duration::from_millis(1);
    link.host.advance(link.now).unwrap();
    assert!(link.host.is_closed());
}

#[test]
fn checking_before_the_session_connected_is_not_an_outage() {
    let mut link = Link::new(spec(VideoCodec::H264, false));
    ice(&mut link, IceConnectionState::Checking);
    ice(&mut link, IceConnectionState::Disconnected);
    assert!(!link.host.is_disconnected());
    link.collect_host();
    assert!(link.host_events.is_empty());
}

#[test]
fn a_keyframe_cannot_overwrite_the_threads_demand_for_the_next_one() {
    let (handle, rx, _idr) = detached(TransportOptions::default());
    let shared = handle.shared.clone();
    let processed = Arc::new(AtomicUsize::new(0));
    let thread = {
        let (shared, processed) = (shared.clone(), processed.clone());
        std::thread::spawn(move || {
            // The media thread, discarding every keyframe at once.
            while let Ok(Command::Video(_)) = rx.recv() {
                shared.pending_video.fetch_sub(1, Ordering::AcqRel);
                shared.need_keyframe.store(true, Ordering::Release);
                processed.fetch_add(1, Ordering::Release);
            }
        })
    };
    for i in 1..=20_000 {
        shared.need_keyframe.store(true, Ordering::Release);
        assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
        while processed.load(Ordering::Acquire) < i {
            std::hint::spin_loop();
        }
        assert!(
            shared.need_keyframe.load(Ordering::Acquire),
            "round {i}: the clear overwrote the thread's demand"
        );
    }
    drop(handle);
    thread.join().unwrap();
}

#[test]
fn receive_errors_only_back_off_when_they_persist() {
    let mut consecutive = 0;
    for _ in 1..RECV_ERRORS_BEFORE_BACKOFF {
        assert_eq!(recv_error_backoff(&mut consecutive), None);
    }
    assert_eq!(
        recv_error_backoff(&mut consecutive),
        Some(RECV_ERROR_BACKOFF)
    );
    assert_eq!(
        recv_error_backoff(&mut consecutive),
        Some(RECV_ERROR_BACKOFF)
    );
    // A datagram (or a quiet timeout) in between starts the count again.
    consecutive = 0;
    assert_eq!(recv_error_backoff(&mut consecutive), None);
}

#[test]
fn a_media_thread_that_cannot_start_releases_the_port() {
    let (handle, _rx, _idr) = detached(TransportOptions::default());
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let port = socket.local_addr().unwrap().port();
    let (tx, _keep) = mpsc::channel::<Command>();
    let shared = handle.shared.clone();
    let result = start_threads(
        &handle.shared,
        || std::thread::Builder::new().spawn(move || receive_loop(socket, tx, shared)),
        || Err(std::io::Error::other("no threads left")),
    );
    let err = format!("{:#}", result.expect_err("the media thread failed"));
    assert!(err.contains("starting the media thread"), "{err}");
    // The receive thread was stopped and joined, so its socket is closed.
    UdpSocket::bind(("127.0.0.1", port)).expect("the port is free again");
    assert!(handle.shared.stop.load(Ordering::Acquire));
}

/// A real MediaHandle on loopback, against a peer that never answers.
pub(crate) fn real_handle(events: Arc<Mutex<Vec<TransportEvent>>>) -> (MediaHandle, SocketAddr) {
    let spec = spec(VideoCodec::H264, false);
    let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
    let local = socket.local_addr().unwrap();
    let (mut rtc, offer) = host_offer(&spec, local);
    let mut peer = client(&spec, CLIENT.parse().unwrap());
    let text = answer(&mut peer, &offer.sdp);
    let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
    let session = Session {
        rtc,
        ids,
        spec: &spec,
        authenticated: Arc::new(AtomicBool::new(false)),
    };
    let hooks = Hooks::new(|_| {}, move |e| events.lock().unwrap().push(e));
    let handle =
        MediaHandle::spawn(socket, local, session, TransportOptions::default(), hooks).unwrap();
    (handle, local)
}

#[test]
fn big_and_garbage_datagrams_on_the_real_socket_do_not_end_the_session() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (mut handle, local) = real_handle(events.clone());
    let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
    // 3000 bytes overflowed the old 2 KB buffer, and Windows then fails the receive.
    for size in [3000, 9000, 60_000, 1, 40] {
        stranger.send_to(&vec![0x5a; size], local).unwrap();
    }
    wait_for("the datagrams to be read and refused", || {
        let stats = handle.stats();
        stats.datagrams_received >= 5 && stats.datagrams_rejected >= 5
    });
    std::thread::sleep(Duration::from_millis(150));
    let stats = handle.stats();
    assert_eq!(stats.recv_errors, 0, "{stats:?}");
    assert!(handle.is_running(), "{stats:?}");
    assert!(
        events.lock().unwrap().is_empty(),
        "{:?}",
        events.lock().unwrap()
    );
    handle.close();
    assert!(!handle.is_running());
}

#[test]
fn a_frame_dropped_before_connecting_demands_a_keyframe_next() {
    let (mut handle, _) = real_handle(Arc::default());
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    // The thread discards it (no peer yet), so the next delta is refused again.
    wait_for("the keyframe to be discarded", || {
        handle.stats().video_dropped >= 2
    });
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    handle.close();
}

#[test]
fn dropping_the_handle_inside_a_hook_does_not_join_itself() {
    let slot: Arc<Mutex<Option<MediaHandle>>> = Arc::default();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let spec = spec(VideoCodec::H264, false);
    let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
    let local = socket.local_addr().unwrap();
    let (mut rtc, offer) = host_offer(&spec, local);
    let mut peer = client(&spec, CLIENT.parse().unwrap());
    let text = answer(&mut peer, &offer.sdp);
    let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
    let session = Session {
        rtc,
        ids,
        spec: &spec,
        authenticated: Arc::new(AtomicBool::new(false)),
    };
    let hook_slot = slot.clone();
    let hooks = Hooks::new(
        |_| {},
        move |event| {
            if event == TransportEvent::Closed {
                // The hook runs on the media thread, which must not wait for itself.
                let handle = hook_slot.lock().unwrap().take();
                drop(handle);
                let _ = done_tx.send(());
            }
        },
    );
    let handle =
        MediaHandle::spawn(socket, local, session, TransportOptions::default(), hooks).unwrap();
    let (tx, shared) = (handle.tx.clone(), handle.shared.clone());
    *slot.lock().unwrap() = Some(handle);
    // End the media thread from outside; its closing hook then drops the last handle.
    tx.send(Command::Stop).unwrap();
    shared.notify();
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the hook returned (no self-join)");
    assert!(slot.lock().unwrap().is_none());
}

/// `cargo test -p butterpollo --release -- --ignored --nocapture webrtc_bench`
#[test]
#[ignore = "measurement"]
fn webrtc_bench_keyframe_and_srtp_cost() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let keyframe = h264_frame(true, 1_000_000);
    let started = Instant::now();
    link.video(keyframe, 0, true);
    link.host.advance(link.now).unwrap();
    let out = link.host.take_outgoing();
    let elapsed = started.elapsed();
    let bytes: usize = out.iter().map(|o| o.data.len()).sum();
    println!(
        "webrtc_bench: 1 MB keyframe: {} datagrams, {} bytes, submit to last packet {:.2} ms (no pacing)",
        out.len(),
        bytes,
        elapsed.as_secs_f64() * 1000.
    );
    let rate = 800_000_000.0;
    println!(
        "webrtc_bench: at the 800 Mbps pacing ceiling the same frame leaves in {:.1} ms",
        bytes as f64 * 8. / rate * 1000.
    );

    let frames = 200;
    let size = 250_000;
    let mut total = 0usize;
    let mut spent = Duration::ZERO;
    for i in 0..frames {
        let data = h264_frame(false, size);
        let started = Instant::now();
        link.video(data, 10 * i, false);
        link.host.advance(link.now).unwrap();
        total += link
            .host
            .take_outgoing()
            .iter()
            .map(|o| o.data.len())
            .sum::<usize>();
        spent += started.elapsed();
    }
    let mbit = total as f64 * 8. / 1e6;
    println!(
        "webrtc_bench: packetize and SRTP (the built crypto provider): {:.1} us per KB, {:.2} s of one core per 1000 Mbit, so {:.1}% of a core at 100 Mbps",
        spent.as_secs_f64() * 1e6 / (total as f64 / 1000.),
        spent.as_secs_f64() / mbit * 1000.,
        spent.as_secs_f64() / mbit * 100. * 100.
    );
}
