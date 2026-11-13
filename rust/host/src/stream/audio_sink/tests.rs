use super::*;
use std::net::UdpSocket;

#[test]
fn the_audio_clock_counts_packets_sent_or_not_and_what_windows_dropped() {
    let mut clock = AudioClock::default();
    assert_eq!(clock.packet(240), 0);
    assert_eq!(clock.packet(240), 240);
    // 40 ms dropped before the next read is 1920 samples of gap, no more and no less.
    clock.lost(Duration::from_millis(40));
    assert_eq!(clock.packet(240), 2400);
    // A hole the size of one sample at 48 kHz rounds down, never up past the audio.
    clock.lost(Duration::from_nanos(20_000));
    assert_eq!(clock.packet(240), 2640);
}

#[test]
fn a_session_that_is_ending_has_no_audio_ping_timeout() {
    let fixture = crate::state::test_support::Fixture::new();
    let launch = fixture.launch(fixture.client(0), butterpollo_core::session::Role::Stream);
    let session = Session::new(launch, butterpollo_core::rtsp::Negotiated::default());
    let host_stop = AtomicBool::new(false);
    let timeout = Duration::from_secs(10);
    let long_ago = Instant::now() - Duration::from_secs(70);
    // A running session whose client went quiet fails loudly, after the window only.
    assert!(ping_timed_out(&session, &host_stop, long_ago, timeout));
    assert!(!ping_timed_out(
        &session,
        &host_stop,
        Instant::now(),
        timeout
    ));
    // The host stopping all streams forgets their peers too: no warning.
    host_stop.store(true, Ordering::Release);
    assert!(!ping_timed_out(&session, &host_stop, long_ago, timeout));
    host_stop.store(false, Ordering::Release);
    // The quit stops the session and then forgets its peers: no warning.
    session.stop();
    assert!(!ping_timed_out(&session, &host_stop, long_ago, timeout));
}

#[test]
fn a_late_audio_ping_brings_the_sound_back_and_clears_the_warning() {
    let warnings = butterpollo_core::session::Warnings::default();
    let codes = || {
        warnings
            .snapshot()
            .into_iter()
            .map(|warning| warning.code)
            .collect::<Vec<_>>()
    };
    let mut ping = PingWait::default();
    // Inside the window nothing is raised.
    ping.update(false, || false, &warnings);
    assert!(codes().is_empty());
    // The window passes without a ping: warned once, and the thread keeps waiting.
    ping.update(false, || true, &warnings);
    assert_eq!(codes(), [PingWait::WARNING]);
    ping.update(
        false,
        || panic!("asked again while already warned"),
        &warnings,
    );
    // The ping arrives: sound goes out again and the warning is gone.
    ping.update(true, || panic!("a ready sink has no timeout"), &warnings);
    assert!(codes().is_empty());
    // A later gap (a roam that drops the address) warns again.
    ping.update(false, || true, &warnings);
    assert_eq!(codes(), [PingWait::WARNING]);
}

fn audio(count: usize) -> Vec<Vec<f32>> {
    vec![vec![0.5; 480]; count]
}

#[test]
fn a_late_wake_that_trims_its_backlog_leaves_a_gap_as_long_as_the_trim() {
    let base = Instant::now();
    let at = |ms| base + Duration::from_millis(ms);
    let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
    assert_eq!(pace.stamp(at(0), &mut audio(1), &[0.; 480]), [0]);
    // The thread wakes 100 ms late. 100 ms of audio is waiting, the capture keeps the newest
    // 10 ms and reports the 90 ms it dropped (4320 samples), then two packets are read.
    pace.host_dropped(90 * 48);
    let positions = pace.stamp(at(105), &mut audio(2), &[0.; 480]);
    assert_eq!(positions, [240 + 4320, 240 + 4320 + 240]);
    // The stream is back in step with the wall: its last packet ends at 105 ms, where the
    // thread woke, and the next pass carries on from there.
    assert_eq!(240 + 4320 + 2 * 240, 105 * 48);
    assert_eq!(pace.stamp(at(110), &mut audio(1), &[0.; 480]), [105 * 48]);
}

#[test]
fn a_wake_that_trimmed_nothing_leaves_the_positions_contiguous() {
    let base = Instant::now();
    let at = |ms| base + Duration::from_millis(ms);
    let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
    assert_eq!(pace.stamp(at(0), &mut audio(1), &[0.; 480]), [0]);
    pace.host_dropped(0);
    // A 25 ms late wake under the trim limit: five packets at once, no gap.
    let positions = pace.stamp(at(30), &mut audio(5), &[0.; 480]);
    assert_eq!(positions, [240, 480, 720, 960, 1200]);
}

#[test]
fn windows_drops_move_the_clock_only_when_a_late_read_explains_them() {
    let base = Instant::now();
    let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
    // The endpoint falling quiet and starting again: Windows skipped, nothing was lost.
    pace.windows_dropped(false, Duration::from_millis(40));
    assert_eq!(pace.stamp(base, &mut audio(1), &[0.; 480]), [0]);
    pace.windows_dropped(true, Duration::from_millis(40));
    assert_eq!(pace.stamp(base, &mut audio(1), &[0.; 480]), [240 + 1920]);
}

#[test]
fn packets_hold_their_place_whether_or_not_they_are_sent() {
    let base = Instant::now();
    let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
    // Three packets are numbered together before any is sent; dropping them upstream, or
    // having no peer, changes nothing about the next pass.
    let mut packets = audio(3);
    assert_eq!(pace.stamp(base, &mut packets, &[0.; 480]), [0, 240, 480]);
    assert_eq!(packets.len(), 3);
    assert_eq!(pace.stamp(base, &mut audio(2), &[0.; 480]), [720, 960]);
}

#[test]
fn an_idle_endpoints_silence_is_paced_and_counts_a_late_pass() {
    let base = Instant::now();
    let at = |ms| base + Duration::from_millis(ms);
    let silence = [0.; 480];
    let mut pace = AudioPace::new(base, Duration::from_millis(5), 240);
    assert_eq!(pace.stamp(at(0), &mut audio(1), &silence), [0]);
    // Quiet for less than 50 ms: no silence yet, and nothing is numbered.
    for ms in [10, 20, 45] {
        let mut packets = Vec::new();
        assert!(pace.stamp(at(ms), &mut packets, &silence).is_empty());
        assert!(packets.is_empty(), "{ms} ms");
    }
    assert_eq!(pace.due(), at(50));
    // The first silence is due after 52 ms. Its wait is in the stream once (52 ms after the
    // 5 ms packet), and the pacing reset is not counted again on top of it.
    let mut packets = Vec::new();
    assert_eq!(pace.stamp(at(52), &mut packets, &silence), [240 + 2496]);
    assert_eq!(packets, [silence.to_vec()]);
    // Then one packet every 5 ms, contiguous.
    assert_eq!(pace.stamp(at(57), &mut Vec::new(), &silence), [2976]);
    assert_eq!(pace.stamp(at(62), &mut Vec::new(), &silence), [3216]);
    // A pass 33 ms late (a blocked device open, say): its packet ends where the thread woke,
    // 100 ms in, and the 28 ms the pacing missed before it are a gap.
    assert_eq!(
        pace.stamp(at(100), &mut Vec::new(), &silence),
        [3456 + 1344]
    );
    assert_eq!(pace.stamp(at(105), &mut Vec::new(), &silence), [5040]);
    // Audio returns; the next quiet spell counts its own wait.
    assert_eq!(pace.stamp(at(110), &mut audio(1), &silence), [5280]);
    assert_eq!(
        pace.stamp(at(170), &mut Vec::new(), &silence),
        [5520 + 60 * 48]
    );
}

#[test]
fn an_idle_endpoints_first_silence_counts_the_wait_once() {
    let mut clock = AudioClock::default();
    assert_eq!(clock.packet(240), 0);
    clock.heard();
    // 50 ms of nothing, then silence every 5 ms: the wait is in the stream once.
    clock.idle(Duration::from_millis(50), 0);
    assert_eq!(clock.packet(240), 2640);
    clock.idle(Duration::from_millis(55), 0);
    clock.idle(Duration::from_millis(60), 0);
    assert_eq!(clock.packet(240), 2880);
    // Audio returns, and the next idle spell counts its own wait.
    clock.heard();
    assert_eq!(clock.packet(240), 3120);
    clock.idle(Duration::from_millis(50), 0);
    assert_eq!(clock.packet(240), 5760);
}

/// A scripted sound card: what each read returns, and what it reports about the time since
/// the last one.
#[derive(Default)]
struct Fake {
    reads: std::collections::VecDeque<std::result::Result<Option<Vec<f32>>, &'static str>>,
    waited: Option<Duration>,
    delivered: Option<Duration>,
    trimmed: u64,
    queued: u64,
}

impl Fake {
    fn playing(packets: usize) -> Self {
        Self {
            reads: (0..packets).map(|_| Ok(Some(vec![0.5; 480]))).collect(),
            ..Self::default()
        }
    }
}

impl AudioCapture for Fake {
    fn since_drained(&mut self) -> Option<Duration> {
        self.waited
    }
    fn read(&mut self, _: usize) -> Result<Option<Vec<f32>>> {
        match self.reads.pop_front() {
            Some(Ok(samples)) => Ok(samples),
            Some(Err(error)) => Err(anyhow::anyhow!(error)),
            None => Ok(None),
        }
    }
    fn take_delivered(&mut self) -> Option<Duration> {
        self.delivered.take()
    }
    fn buffer(&self) -> Duration {
        Duration::from_millis(20)
    }
    fn take_trimmed(&mut self) -> u64 {
        std::mem::take(&mut self.trimmed)
    }
    fn take_discarded(&mut self) -> u64 {
        self.take_trimmed() + std::mem::take(&mut self.queued)
    }
}

struct Thread {
    base: Instant,
    pace: AudioPace,
    loss: HostLoss,
    errors: Vec<String>,
}

impl Thread {
    fn new() -> Self {
        let base = Instant::now();
        Self {
            base,
            pace: AudioPace::new(base, Duration::from_millis(5), 240),
            loss: HostLoss::default(),
            errors: Vec::new(),
        }
    }

    fn at(&self, ms: u64) -> Instant {
        self.base + Duration::from_millis(ms)
    }

    /// One pass of the audio thread at `ms`: the positions of what it sends.
    fn pass(&mut self, capture: &mut Option<Fake>, ms: u64) -> Vec<u64> {
        let now = self.at(ms);
        let errors = &mut self.errors;
        let (_, positions) = collect(
            capture,
            240,
            &mut self.loss,
            &mut self.pace,
            &[0.; 480],
            || now,
            |error| errors.push(error.to_string()),
        );
        positions
    }
}

#[test]
fn an_endpoint_delivering_10_ms_per_wake_sends_two_contiguous_packets_each_wake() {
    let mut audio = Thread::new();
    let mut capture = Some(Fake::default());
    for wake in 0..50u64 {
        let card = capture.as_mut().unwrap();
        card.reads.extend(Fake::playing(2).reads);
        card.waited = (wake > 0).then_some(Duration::from_millis(10));
        card.delivered = Some(Duration::ZERO);
        assert_eq!(
            audio.pass(&mut capture, wake * 10),
            [wake * 480, wake * 480 + 240],
            "wake {wake}"
        );
        assert!(capture.as_ref().unwrap().reads.is_empty());
    }
    assert!(audio.errors.is_empty());
}

#[test]
fn the_audio_loop_counts_a_windows_skip_and_the_hosts_trim_before_numbering() {
    let mut audio = Thread::new();
    let mut capture = Some(Fake::playing(1));
    assert_eq!(audio.pass(&mut capture, 0), [0]);
    // A read 105 ms later finds 80 ms Windows skipped (explained by the late read, the sound
    // card keeps 20 ms) and 10 ms this host trimmed: two packets, 3840 + 480 samples after
    // the first packet.
    let card = capture.as_mut().unwrap();
    card.reads.extend(Fake::playing(2).reads);
    card.waited = Some(Duration::from_millis(105));
    card.delivered = Some(Duration::from_millis(80));
    card.trimmed = 480;
    assert_eq!(
        audio.pass(&mut capture, 105),
        [240 + 3840 + 480, 240 + 3840 + 480 + 240]
    );
}

#[test]
fn a_stall_that_delivers_less_than_a_packet_counts_its_wait_once() {
    let mut audio = Thread::new();
    let mut capture = Some(Fake::playing(1));
    assert_eq!(audio.pass(&mut capture, 0), [0]);
    // 100 ms stall: Windows dropped 80 ms of it, and what remained is under one packet. The
    // silence the quiet endpoint gets covers the rest, not the 80 ms again: the packet ends
    // where the thread woke.
    let card = capture.as_mut().unwrap();
    card.waited = Some(Duration::from_millis(100));
    card.delivered = Some(Duration::from_millis(80));
    assert_eq!(audio.pass(&mut capture, 100), [240 + 100 * 48]);
}

#[test]
fn an_idle_endpoints_silence_still_counts_its_wait_when_nothing_else_did() {
    let mut audio = Thread::new();
    let mut capture = Some(Fake::playing(1));
    assert_eq!(audio.pass(&mut capture, 0), [0]);
    capture.as_mut().unwrap().waited = Some(Duration::from_millis(80));
    assert_eq!(audio.pass(&mut capture, 80), [240 + 80 * 48]);
}

#[test]
fn a_failed_capture_counts_what_it_lost_and_the_time_until_the_next_one() {
    let mut audio = Thread::new();
    let mut capture = Some(Fake::playing(0));
    capture
        .as_mut()
        .unwrap()
        .reads
        .extend([Ok(Some(vec![0.5; 480])), Err("the endpoint went away")]);
    let card = capture.as_mut().unwrap();
    card.waited = Some(Duration::from_millis(100));
    // 80 ms skipped by Windows (a late read explains it), 10 ms trimmed, 20 ms still queued.
    card.delivered = Some(Duration::from_millis(80));
    card.trimmed = 480;
    card.queued = 960;
    let positions = audio.pass(&mut capture, 0);
    assert!(capture.is_none());
    assert_eq!(audio.errors, ["the endpoint went away"]);
    // The packet read before the failure sits after the skip and the trim, which happened
    // before it, and before the 20 ms the capture still queued, which was never read.
    assert_eq!(positions, [3840 + 480]);
    // 30 ms later a reopened capture delivers: the time between, under the idle limit and
    // sent no silence, is lost as well.
    let mut capture = Some(Fake::playing(1));
    assert_eq!(
        audio.pass(&mut capture, 30),
        [3840 + 480 + 960 + 240 + 30 * 48]
    );
    // Counted once: the next packet follows it.
    capture
        .as_mut()
        .unwrap()
        .reads
        .extend(Fake::playing(1).reads);
    assert_eq!(
        audio.pass(&mut capture, 35),
        [3840 + 480 + 960 + 240 + 30 * 48 + 240]
    );
}

#[test]
fn a_capture_dropped_for_a_new_sink_counts_its_residue_and_the_reopen_gap() {
    let mut audio = Thread::new();
    let mut capture = Some(Fake::playing(1));
    assert_eq!(audio.pass(&mut capture, 0), [0]);
    // The sink changes at 10 ms: 40 ms of skip a late read explains, 10 ms trimmed and
    // 20 ms the resampler still held.
    let mut old = Fake {
        waited: Some(Duration::from_millis(60)),
        delivered: Some(Duration::from_millis(40)),
        trimmed: 480,
        queued: 960,
        ..Fake::default()
    };
    let waited = old.since_drained();
    let closed = audio.at(10);
    retire(&mut old, waited, &mut audio.loss, &mut audio.pace, closed);
    // The new capture delivers at 25 ms: the 15 ms without one is lost too.
    let mut capture = Some(Fake::playing(1));
    assert_eq!(
        audio.pass(&mut capture, 25),
        [240 + 1920 + 480 + 960 + 15 * 48]
    );
}

#[test]
fn silence_covers_a_reopen_that_outlasts_the_idle_limit_without_counting_it_twice() {
    let mut audio = Thread::new();
    let mut capture = Some(Fake::playing(1));
    assert_eq!(audio.pass(&mut capture, 0), [0]);
    let mut old = Fake::default();
    let closed = audio.at(10);
    retire(&mut old, None, &mut audio.loss, &mut audio.pace, closed);
    // No capture for 60 ms: the first silence counts the wait since the last audio.
    let mut none: Option<Fake> = None;
    assert_eq!(audio.pass(&mut none, 60), [240 + 60 * 48]);
    // Audio returns: the silence already covered the gap.
    let mut capture = Some(Fake::playing(1));
    assert_eq!(audio.pass(&mut capture, 65), [240 + 60 * 48 + 240]);
}

const KEY: [u8; 16] = *b"0123456789abcdef";

/// What GameStream clients receive for a run of Opus packets, byte for byte: the sink must add
/// nothing to, and take nothing from, what the packetizer produces.
#[test]
fn gamestream_audio_is_the_packetizer_output_on_the_wire() {
    for encrypted in [false, true] {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
        let peers = Mutex::new(crate::stream::Peers::default());
        let mut sink = GameStreamAudio::new(
            &socket,
            &peers,
            "launch",
            AudioPacketizer::new(KEY, 77, encrypted, 5),
            false,
        );
        assert!(!sink.ready(), "no endpoint yet");
        peers
            .lock()
            .unwrap()
            .insert("launch", true, receiver.local_addr().unwrap());
        assert!(sink.ready());

        let mut reference = AudioPacketizer::new(KEY, 77, encrypted, 5);
        let mut expected = Vec::new();
        for index in 0..9u8 {
            let opus: Vec<u8> = (0..40 + usize::from(index))
                .map(|i| i as u8 ^ index)
                .collect();
            expected.extend(reference.encode(&opus).unwrap());
            assert_eq!(sink.send(&opus, Instant::now(), 0).unwrap(), 0);
        }
        // Nine data packets and two FEC groups of two parity packets.
        assert_eq!(expected.len(), 9 + 4);
        let mut buffer = [0u8; 2048];
        for (index, want) in expected.iter().enumerate() {
            let n = receiver.recv(&mut buffer).unwrap();
            assert_eq!(
                &buffer[..n],
                want.as_slice(),
                "packet {index}, encrypted={encrypted}"
            );
        }
    }
}

use butterpollo_core::audio::OpusLayout;

/// 5.1 as Chromium's multiopus decodes it, and as Moonlight's own mapping orders it.
fn five_one(mapping: [u8; 6]) -> OpusLayout {
    OpusLayout {
        channels: 6,
        streams: 4,
        coupled: 2,
        mapping: mapping.to_vec(),
        bitrate: 256_000,
    }
}

fn session() -> Arc<crate::state::Session> {
    let f = crate::state::test_support::Fixture::new();
    f.session(f.client(u32::MAX), butterpollo_core::session::Role::Stream)
}

#[test]
fn the_audio_thread_encodes_the_negotiated_layout_and_else_the_launch_one() {
    let s = session();
    let negotiated = five_one([0, 4, 1, 2, 3, 5]);
    assert_eq!(
        super::super::audio_layout(&s, Some(&negotiated)).unwrap(),
        negotiated
    );
    // Not the launch's own 2-channel default, and not core's Moonlight mapping.
    let other = five_one([0, 1, 4, 5, 2, 3]);
    assert_eq!(super::super::audio_layout(&s, Some(&other)).unwrap(), other);
    assert_eq!(
        super::super::audio_layout(&s, None).unwrap(),
        OpusLayout::select(
            s.config.audio_channels as usize,
            s.config.audio_quality,
            s.launch.options.get("surroundParams").map(String::as_str),
        )
        .unwrap()
    );
}

/// A negotiated mapping reaches libopus: the same tone on the same channel encodes to
/// different packets under two mappings, and to identical ones under the same.
#[test]
#[ignore = "requires the packaged libopus DLL (BUTTERPOLLO_TEST_OPUS_ROOT)"]
fn a_negotiated_mapping_reaches_the_opus_encoder() {
    let root = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_OPUS_ROOT").expect("set BUTTERPOLLO_TEST_OPUS_ROOT"),
    );
    let s = session();
    let encode = |layout: &OpusLayout| -> Vec<Vec<u8>> {
        let (mut opus, used) = super::super::open_audio_encoder(&root, &s, Some(layout)).unwrap();
        assert_eq!(&used, layout);
        let frames = 48 * usize::from(s.config.audio_packet_ms);
        (0..20)
            .map(|packet| {
                let samples: Vec<f32> = (0..frames)
                    .flat_map(|frame| {
                        (0..6).map(move |channel| {
                            // Front right carries a tone; the rest stay silent.
                            if channel == 1 {
                                ((packet * frames + frame) as f32 * 0.07).sin() * 0.4
                            } else {
                                0.0
                            }
                        })
                    })
                    .collect();
                opus.encode(&samples).unwrap()
            })
            .collect()
    };
    let chromium = five_one([0, 4, 1, 2, 3, 5]);
    let moonlight = five_one([0, 1, 4, 5, 2, 3]);
    assert_eq!(encode(&chromium), encode(&chromium));
    assert_ne!(encode(&chromium), encode(&moonlight));
}

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// Four fixed Opus packets (`a0 01 02 03` .. `a3 01 02 03`) from key `0123456789abcdef`,
/// key id 77 and 5 ms packets, as a Moonlight client must receive them. Fixed bytes, so a
/// change to the constructor, the flags or the packetizer shows here and not only as a
/// difference between two copies of the same code. RTP: 0x80, payload type 97, sequence,
/// timestamp (+5 per packet), SSRC 0; the FEC packets (type 127) name the group they cover
/// and carry the two Reed-Solomon rows over it (0x82 = 0x77*a0 ^ 0x40*a1 ^ 0x38*a2 ^ 0x0e*a3
/// in GF(256), and 0xa9 for the second row).
const PLAIN: [&str; 6] = [
    "806100000000000000000000a0010203",
    "806100010000000500000000a1010203",
    "806100020000000a00000000a2010203",
    "806100030000000f00000000a3010203",
    "807f0004000000140000000000610000000000000000000082010203",
    "807f00050000001400000000016100000000000000000000a9010203",
];
/// The same with AES-CBC payloads (IV = key id + sequence, big endian, then zeros).
const SEALED: [&str; 6] = [
    "80610000000000000000000032b4fab2a3e6aacde5d4ca2f5b308994",
    "8061000100000005000000006623c01143909d9d3bfdfd6bca44cb9e",
    "806100020000000a00000000f1dd481c950a76ce04029c943d175893",
    "806100030000000f000000004b69739cb70aaffcf81d4d3d42e5fb42",
    "807f00040000001400000000006100000000000000000000cdd824dbbdd2b00f460c633313a52ab6",
    "807f000500000014000000000161000000000000000000009c8c956051ffe7293901e7b176084948",
];

#[test]
fn gamestream_audio_wire_bytes_are_pinned() {
    for (encrypted, qos) in [(false, false), (true, false), (false, true), (true, true)] {
        let want = if encrypted { SEALED } else { PLAIN };
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
        let peers = Mutex::new(crate::stream::Peers::default());
        peers
            .lock()
            .unwrap()
            .insert("launch", true, receiver.local_addr().unwrap());
        let mut sink = GameStreamAudio::new(
            &socket,
            &peers,
            "launch",
            AudioPacketizer::new(KEY, 77, encrypted, 5),
            qos,
        );
        assert!(sink.ready(), "qos={qos}");
        for index in 0..4u8 {
            assert_eq!(
                sink.send(
                    &[0xa0 + index, 1, 2, 3],
                    Instant::now(),
                    u64::from(index) * 240
                )
                .unwrap(),
                0
            );
        }
        let mut buffer = [0u8; 2048];
        for (index, hex) in want.iter().enumerate() {
            let n = receiver.recv(&mut buffer).unwrap();
            assert_eq!(
                &buffer[..n],
                bytes(hex).as_slice(),
                "packet {index}, encrypted={encrypted}, qos={qos}"
            );
        }
    }
}
