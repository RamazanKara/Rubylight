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
}
