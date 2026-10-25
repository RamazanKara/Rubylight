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
    time::Instant,
};

pub(super) trait AudioSink {
    /// Whether the far end is known. Audio is encoded either way (the capture must be drained),
    /// but a sink that never becomes ready ends the audio when the ping window passes.
    fn ready(&mut self) -> bool;
    /// Send one encoded Opus packet. Returns how many packets did not go out, which the caller
    /// counts as loss; only a broken transport is an error.
    fn send(&mut self, opus: &[u8], captured: Instant) -> Result<usize>;
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

    fn send(&mut self, opus: &[u8], _captured: Instant) -> Result<usize> {
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

    fn send(&mut self, opus: &[u8], captured: Instant) -> Result<usize> {
        Ok(
            match self.0.submit(AudioPacket {
                data: opus.to_vec(),
                capture: captured,
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
                assert_eq!(sink.send(&opus, Instant::now()).unwrap(), 0);
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
                    sink.send(&[0xa0 + index, 1, 2, 3], Instant::now()).unwrap(),
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
