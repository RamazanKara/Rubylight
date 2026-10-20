//! Audio receive: RTP reordering with 4+2 Reed-Solomon FEC and AES-CBC decryption, yielding
//! Opus packets in order with a marker for each packet that could not be recovered. Opus
//! decoding belongs to the app.
//!
//! Ported from moonlight-common-c (GPL-3.0, Cameron Gutman and contributors):
//! `RtpAudioQueue.c` and the receive and decrypt steps of `AudioStream.c`, with Nightjar's
//! patches. `protocol/difftest` checks it against that C.

mod queue;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, KeyIvInit};

use queue::{AddResult, AudioQueue, Queued, RtpHeader, RTP_PAYLOAD_TYPE_AUDIO};

/// Datagrams are read into a buffer of this size, as in the C core.
pub const MAX_PACKET_SIZE: usize = 1400;
/// Audio received in the first half second is the host's backlog and is dropped.
const INITIAL_DROP_MS: u32 = 500;

/// `remoteInputAesKey` and `remoteInputAesIv` when the host encrypts audio.
#[derive(Clone, Debug)]
pub struct AudioEncryption {
    pub key: [u8; 16],
    /// Only the first four bytes (the key id, big endian) are used.
    pub iv: [u8; 16],
}

#[derive(Clone, Debug)]
pub struct AudioConfig {
    /// Negotiated Opus packet duration (`AudioPacketDuration`), usually 5, 10 or 20 ms.
    pub packet_duration_ms: u32,
    /// The host's version quad from serverinfo.
    pub app_version: [i32; 4],
    pub encryption: Option<AudioEncryption>,
}

/// One step of audio for the decoder, in order.
#[derive(Debug, PartialEq, Eq)]
pub enum AudioOutput<'a> {
    /// An Opus packet.
    Packet(&'a [u8]),
    /// A packet was lost: run packet loss concealment for one packet duration.
    Lost,
}

/// `RTP_AUDIO_STATS` plus decryption failures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioStats {
    pub packet_count_audio: u32,
    pub packet_count_fec: u32,
    pub packet_count_fec_recovered: u32,
    pub packet_count_fec_failed: u32,
    pub packet_count_oos: u32,
    pub packet_count_invalid: u32,
    pub packet_count_fec_invalid: u32,
    pub decrypt_failures: u32,
}

/// The audio receive path of one stream. No sockets or threads: feed it each datagram.
pub struct AudioReceiver {
    queue: AudioQueue,
    decryptor: Option<([u8; 16], u32)>,
    packets_to_drop: u32,
    received_data: bool,
    packet: Vec<u8>,
    plain: Vec<u8>,
}

impl AudioReceiver {
    pub fn new(config: AudioConfig) -> Self {
        Self {
            queue: AudioQueue::new(config.packet_duration_ms, config.app_version),
            decryptor: config.encryption.map(|e| {
                (
                    e.key,
                    u32::from_be_bytes([e.iv[0], e.iv[1], e.iv[2], e.iv[3]]),
                )
            }),
            packets_to_drop: INITIAL_DROP_MS / config.packet_duration_ms.max(1),
            received_data: false,
            packet: Vec::with_capacity(MAX_PACKET_SIZE),
            plain: Vec::with_capacity(MAX_PACKET_SIZE + 16),
        }
    }

    /// Processes one datagram received at `now_us` (monotonic microseconds) and passes the
    /// packets now due to `out`.
    pub fn receive(&mut self, datagram: &[u8], now_us: u64, out: &mut impl FnMut(AudioOutput<'_>)) {
        let datagram = &datagram[..datagram.len().min(MAX_PACKET_SIZE)];
        if datagram.len() < 12 {
            return;
        }
        self.received_data = true;
        let header = RtpHeader::parse(datagram);
        if self.packets_to_drop > 0 {
            if header.packet_type == RTP_PAYLOAD_TYPE_AUDIO {
                self.packets_to_drop -= 1;
            }
            return;
        }

        match self.queue.add_packet(&header, datagram, now_us) {
            AddResult::HandleNow => Self::play(
                &self.decryptor,
                &mut self.plain,
                &mut self.queue.stats,
                header.seq,
                datagram,
                out,
            ),
            AddResult::Ready => {
                while let Some(next) = self.queue.next_packet(&mut self.packet) {
                    match next {
                        Queued::Lost => out(AudioOutput::Lost),
                        Queued::Packet { seq } => Self::play(
                            &self.decryptor,
                            &mut self.plain,
                            &mut self.queue.stats,
                            seq,
                            &self.packet,
                            out,
                        ),
                    }
                }
            }
            AddResult::Queued => {}
        }
    }

    /// The socket had nothing for a receive poll interval: once audio has arrived, there is no
    /// backlog left to drop.
    pub fn receive_timeout(&mut self) {
        if self.received_data {
            self.packets_to_drop = 0;
        }
    }

    pub fn stats(&self) -> AudioStats {
        self.queue.stats
    }

    /// `decodeInputData`: decrypts if needed and hands the Opus payload on.
    fn play(
        decryptor: &Option<([u8; 16], u32)>,
        plain: &mut Vec<u8>,
        stats: &mut AudioStats,
        seq: u16,
        packet: &[u8],
        out: &mut impl FnMut(AudioOutput<'_>),
    ) {
        let payload = &packet[12..];
        let Some((key, key_id)) = decryptor else {
            out(AudioOutput::Packet(payload));
            return;
        };
        // The IV is the key id plus the sequence number, big endian, then zeros.
        let mut iv = [0u8; 16];
        iv[..4].copy_from_slice(&key_id.wrapping_add(u32::from(seq)).to_be_bytes());
        plain.clear();
        plain.extend_from_slice(payload);
        let cipher = cbc::Decryptor::<aes::Aes128>::new(key.into(), &iv.into());
        match cipher.decrypt_padded_mut::<Pkcs7>(plain) {
            Ok(opus) => out(AudioOutput::Packet(opus)),
            Err(_) => stats.decrypt_failures += 1,
        }
    }
}

#[cfg(test)]
mod tests;
