//! Where the encoded audio goes. `Media::audio` captures and encodes the same way for both
//! transports; only the last step differs: GameStream wraps each Opus packet in RTP with FEC and
//! sends it to the client's audio port, WebRTC hands it to the media thread's Opus track.

use super::qos::Tagged;
use crate::webrtc::media::{AudioPacket, AudioSender, Submit};
use anyhow::Result;
use butterpollo_core::packet::AudioPacketizer;
use std::{
    collections::HashMap,
    net::{SocketAddr, UdpSocket},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(super) trait AudioSink {
    /// Whether the far end is known. Audio is encoded either way (the capture must be drained),
    /// but a sink that never becomes ready ends the audio when the ping window passes.
    fn ready(&mut self) -> bool;
    /// Send one encoded Opus packet. Returns how many packets did not go out, which the caller
    /// counts as loss; only a broken transport is an error. `position` is where the packet starts
    /// in the audio thread's stream, in 48 kHz samples, counting every packet and any audio the
    /// endpoint lost, whether or not it was sent.
    fn send(&mut self, opus: &[u8], captured: Instant, position: u64) -> Result<usize>;
}

/// Where the audio thread is in its stream, in 48 kHz samples. It counts what the thread encoded
/// and sent, or would have sent, and the audio that never became a packet (Windows dropped it,
/// this host trimmed it, or the thread was too late to make silence for it), so a receiver
/// placing packets by their position hears a gap exactly where audio was lost and nowhere else.
/// It never reads a clock: the sound card's drifts against wall time, and a stall that lost
/// nothing is no gap.
#[derive(Default)]
pub(super) struct AudioClock {
    position: u64,
    /// The idle endpoint's silence has started: its first packet already counted the wait.
    silent: bool,
}

impl AudioClock {
    const RATE: u128 = 48_000;

    /// The start of the next packet of `frames` samples; the clock moves past it, sent or not.
    pub(super) fn packet(&mut self, frames: usize) -> u64 {
        let at = self.position;
        self.position += frames as u64;
        at
    }

    /// `lost` of audio that never became a packet, before the packets about to be read.
    pub(super) fn lost(&mut self, lost: Duration) {
        // Whole nanoseconds, so 90 ms is 4320 samples and not 4319.99.
        self.position += (lost.as_nanos() * Self::RATE / 1_000_000_000) as u64;
    }

    /// The same, counted in 48 kHz samples.
    pub(super) fn lost_samples(&mut self, samples: u64) {
        self.position += samples;
    }

    /// Audio arrived: the endpoint is no longer idle.
    pub(super) fn heard(&mut self) {
        self.silent = false;
    }

    /// The first silence packet of an idle endpoint is due `waited` after the last audio. The
    /// time before it carried no packets, and is part of the stream. Returns whether this was
    /// that first packet.
    pub(super) fn idle(&mut self, waited: Duration) -> bool {
        let first = !std::mem::replace(&mut self.silent, true);
        if first {
            self.lost(waited);
        }
        first
    }
}

/// How long an endpoint must be quiet before silence stands in for it.
const IDLE: Duration = Duration::from_millis(50);

/// What the audio thread decides each pass, apart from the sound card and the socket: when the
/// next packet is due, when silence replaces a quiet endpoint, and where each packet sits in the
/// stream. Its numbers go to the sink before anything is sent, so a packet the sink drops or the
/// peer never receives still holds its place.
pub(super) struct AudioPace {
    clock: AudioClock,
    interval: Duration,
    frames: usize,
    /// When audio last arrived.
    heard: Instant,
    /// When an idle endpoint's next silence is due.
    next: Instant,
}

impl AudioPace {
    pub(super) fn new(now: Instant, interval: Duration, frames: usize) -> Self {
        Self {
            clock: AudioClock::default(),
            interval,
            frames,
            heard: now,
            next: now,
        }
    }

    /// When the thread has something to do: the packet deadline, but not before the endpoint has
    /// been quiet for long enough to need silence.
    pub(super) fn due(&self) -> Instant {
        self.next.max(self.heard + IDLE)
    }

    /// Windows reported `skipped` of audio it had no room for. `lost` says whether the read
    /// that found it was late enough to explain it; a skip nothing explains is the endpoint
    /// falling quiet and starting again, and is no gap.
    pub(super) fn windows_dropped(&mut self, lost: bool, skipped: Duration) {
        if lost {
            self.clock.lost(skipped);
        }
    }

    /// This host discarded `samples` (48 kHz) of audio Windows had delivered.
    pub(super) fn host_dropped(&mut self, samples: u64) {
        self.clock.lost_samples(samples);
    }

    /// Numbers the packets read at `now`, in order. With none, and the endpoint quiet for 50 ms,
    /// one packet of `silence` is made for it. Returns each packet's position in the stream.
    pub(super) fn stamp(
        &mut self,
        now: Instant,
        packets: &mut Vec<Vec<f32>>,
        silence: &[f32],
    ) -> Vec<u64> {
        if !packets.is_empty() {
            self.heard = now;
            self.next = now + self.interval;
            self.clock.heard();
        } else if now >= self.next && now.saturating_duration_since(self.heard) >= IDLE {
            // WASAPI delivers roughly 10 ms chunks; allow for scheduling jitter
            // before treating a quiet capture queue as an idle endpoint.
            let first = self.clock.idle(now.saturating_duration_since(self.heard));
            packets.push(silence.to_vec());
            self.next += self.interval;
            if self.next < now {
                // One packet covers one interval; the rest of the late time (a blocked device
                // open, a reopen) is gone from the stream. The first packet's wait is already
                // counted by `idle`.
                if !first {
                    self.clock.lost(now - self.next);
                }
                self.next = now + self.interval;
            }
        }
        packets
            .iter()
            .map(|_| self.clock.packet(self.frames))
            .collect()
    }
}

/// RTP with FEC on the client's audio port.
pub(super) struct GameStreamAudio<'a> {
    socket: &'a Arc<UdpSocket>,
    peers: &'a Mutex<HashMap<(String, bool), SocketAddr>>,
    launch: &'a str,
    packetizer: AudioPacketizer,
    qos: Tagged,
    use_qos: bool,
    peer: Option<SocketAddr>,
}

impl<'a> GameStreamAudio<'a> {
    pub(super) fn new(
        socket: &'a Arc<UdpSocket>,
        peers: &'a Mutex<HashMap<(String, bool), SocketAddr>>,
        launch: &'a str,
        packetizer: AudioPacketizer,
        use_qos: bool,
    ) -> Self {
        Self {
            socket,
            peers,
            launch,
            packetizer,
            qos: Tagged::default(),
            use_qos,
            peer: None,
        }
    }
}

impl AudioSink for GameStreamAudio<'_> {
    fn ready(&mut self) -> bool {
        self.peer = self
            .peers
            .lock()
            .unwrap()
            .get(&(self.launch.to_owned(), true))
            .copied();
        if let (Some(peer), true) = (self.peer, self.use_qos) {
            self.qos.follow(self.socket, peer, true);
        }
        self.peer.is_some()
    }

    fn send(&mut self, opus: &[u8], _captured: Instant, _position: u64) -> Result<usize> {
        let Some(peer) = self.peer else {
            return Ok(1);
        };
        let mut unsent = 0;
        for packet in self.packetizer.encode(opus)? {
            // A lost audio packet is concealed by the client; only a broken socket stops the audio.
            if !butterpollo_windows::net::send_datagram(self.socket, &packet, peer)? {
                unsent += 1;
            }
        }
        Ok(unsent)
    }
}

/// The WebRTC Opus track. The media thread sends nothing until the peer connects, and a full
/// queue drops the packet, which Opus conceals; neither is an error here.
pub(super) struct WebRtcAudio(pub(super) AudioSender);

impl AudioSink for WebRtcAudio {
    fn ready(&mut self) -> bool {
        true
    }

    fn send(&mut self, opus: &[u8], captured: Instant, position: u64) -> Result<usize> {
        Ok(
            match self.0.submit(AudioPacket {
                data: opus.to_vec(),
                capture: captured,
                position,
            }) {
                Submit::Queued => 0,
                Submit::Dropped => 1,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        clock.idle(Duration::from_millis(50));
        assert_eq!(clock.packet(240), 2640);
        clock.idle(Duration::from_millis(55));
        clock.idle(Duration::from_millis(60));
        assert_eq!(clock.packet(240), 2880);
        // Audio returns, and the next idle spell counts its own wait.
        clock.heard();
        assert_eq!(clock.packet(240), 3120);
        clock.idle(Duration::from_millis(50));
        assert_eq!(clock.packet(240), 5760);
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
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
            let peers = Mutex::new(HashMap::new());
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
                .insert(("launch".into(), true), receiver.local_addr().unwrap());
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
            let (mut opus, used) =
                super::super::open_audio_encoder(&root, &s, Some(layout)).unwrap();
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
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
            let peers = Mutex::new(HashMap::new());
            peers
                .lock()
                .unwrap()
                .insert(("launch".into(), true), receiver.local_addr().unwrap());
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
}
