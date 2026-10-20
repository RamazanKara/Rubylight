//! Control stream framing: message types, ENet channels and the encrypted envelope.

use crate::crypto::{control_iv, Gcm, Key, Origin, TAG_BYTES};

/// Control message types of the encrypted (Gen 7, 7.1.431+) control stream.
pub mod ptype {
    /// Start A on the encrypted stream is an IDR request.
    pub const REQUEST_IDR_FRAME: u16 = 0x0302;
    pub const START_B: u16 = 0x0307;
    pub const INVALIDATE_REF_FRAMES: u16 = 0x0301;
    pub const LOSS_STATS: u16 = 0x0201;
    pub const PERIODIC_PING: u16 = 0x0200;
    pub const INPUT_DATA: u16 = 0x0206;
    pub const RUMBLE: u16 = 0x010b;
    /// Extended termination, carrying an HRESULT (or a 16-bit reason from older hosts).
    pub const TERMINATION: u16 = 0x0109;
    pub const HDR_MODE: u16 = 0x010e;
    pub const LTR_FRAME_ACK: u16 = 0x0350;
    /// Sunshine extensions (host to client).
    pub const RUMBLE_TRIGGERS: u16 = 0x5500;
    pub const SET_MOTION_EVENT: u16 = 0x5501;
    pub const SET_RGB_LED: u16 = 0x5502;
    pub const ADAPTIVE_TRIGGERS: u16 = 0x5503;
    /// Sunshine's per-frame FEC report (client to host). Shares its id with
    /// [`SET_RGB_LED`], which goes the other way.
    pub const FRAME_FEC_STATUS: u16 = 0x5502;
    /// Apollo's server commands (client to host).
    pub const SERVER_COMMAND: u16 = 0x3000;
    /// Nonary's PCM controller haptics (host to client).
    pub const CONTROLLER_HAPTICS: u16 = 0x5601;
    /// Rubylight's USB controller audio: speaker PCM (host to client), microphone PCM
    /// (client to host) and status (both ways).
    pub const CONTROLLER_SPEAKER: u16 = 0x5533;
    pub const CONTROLLER_MICROPHONE: u16 = 0x5534;
    pub const CONTROLLER_AUDIO_CONTROL: u16 = 0x5535;
    pub const PHASE_LOCK_REPORT: u16 = rubylight_protocol::phase_lock::REPORT_MESSAGE_TYPE;
    pub const DISPLAY_CAPS: u16 = rubylight_protocol::control::DISPLAY_CAPS_MESSAGE_TYPE;
    pub const RECONFIGURE: u16 = rubylight_protocol::control::RECONFIGURE_MESSAGE_TYPE;
    pub const HOST_TIMING: u16 = rubylight_protocol::control::HOST_TIMING_MESSAGE_TYPE;
    pub const CLOCK_SYNC: u16 = rubylight_protocol::control::CLOCK_SYNC_MESSAGE_TYPE;
    pub const CURSOR_SHAPE_REQUEST: u16 = 0x5540;
    pub const CURSOR_SHAPE: u16 = 0x5541;
}

/// ENet channels of the control stream (`CTRL_CHANNEL_*`).
pub mod channel {
    pub const GENERIC: u8 = 0x00;
    /// IDR, LTR ACK and RFI.
    pub const URGENT: u8 = 0x01;
    pub const KEYBOARD: u8 = 0x02;
    pub const MOUSE: u8 = 0x03;
    pub const PEN: u8 = 0x04;
    pub const TOUCH: u8 = 0x05;
    pub const UTF8: u8 = 0x06;
    /// Apollo's server commands and controller audio and haptics.
    pub const EXTENSION: u8 = 0x08;
    /// 0x10 to 0x1F by controller number.
    pub const GAMEPAD_BASE: u8 = 0x10;
    /// 0x20 to 0x2F by controller number.
    pub const SENSOR_BASE: u8 = 0x20;
    /// Channels the client opens.
    pub const COUNT: usize = 0x30;
}

/// How ENet delivers a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Acknowledged and resent (`ENET_PACKET_FLAG_RELIABLE`).
    Reliable,
    /// Sequenced, not resent; a late one is dropped (flags 0).
    Unreliable,
    /// Neither sequenced nor resent (`ENET_PACKET_FLAG_UNSEQUENCED`).
    Unsequenced,
}

/// Size of the encrypted envelope header: type 0x0001, length, sequence.
pub const ENCRYPTED_HEADER_BYTES: usize = 8;
/// Envelope type of an encrypted message.
pub const ENCRYPTED_HEADER_TYPE: u16 = 0x0001;
/// The plaintext V2 header inside the envelope: type and payload length.
pub const INNER_HEADER_BYTES: usize = 4;
/// Largest payload the 16-bit envelope length can carry.
pub const MAX_PAYLOAD: usize = u16::MAX as usize - 4 - TAG_BYTES - INNER_HEADER_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Shorter than its headers or its declared length.
    Runt,
    /// Not an encrypted envelope; the encrypted stream drops plaintext messages.
    Unencrypted(u16),
    /// The tag did not verify.
    Authentication,
    /// A sequence number seen before or too old to tell.
    Replay(u32),
}

/// Seals client messages. One sequence counter runs across every message and channel,
/// input included, and supplies the AES-GCM IV.
pub struct Sealer {
    gcm: Gcm,
    v2: bool,
    next_sequence: u32,
}

impl Sealer {
    pub fn new(key: &Key, v2: bool) -> Self {
        Self {
            gcm: Gcm::new(key),
            v2,
            next_sequence: 0,
        }
    }

    pub fn next_sequence(&self) -> u32 {
        self.next_sequence
    }

    /// The datagram for one message: envelope header, tag, then the encrypted V2 header and
    /// payload. `None` when the payload does not fit the envelope.
    pub fn seal(&mut self, message_type: u16, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() > MAX_PAYLOAD {
            return None;
        }
        let sequence = self.next_sequence;
        let iv = control_iv(sequence, self.v2, Origin::Client);
        let body = INNER_HEADER_BYTES + payload.len();
        let mut out = Vec::with_capacity(ENCRYPTED_HEADER_BYTES + TAG_BYTES + body);
        out.extend_from_slice(&ENCRYPTED_HEADER_TYPE.to_le_bytes());
        out.extend_from_slice(&((4 + TAG_BYTES + body) as u16).to_le_bytes());
        out.extend_from_slice(&sequence.to_le_bytes());
        out.extend_from_slice(&[0; TAG_BYTES]);
        out.extend_from_slice(&message_type.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        out.extend_from_slice(payload);
        let start = ENCRYPTED_HEADER_BYTES + TAG_BYTES;
        let tag = self
            .gcm
            .seal_in_place(iv.as_slice(), &mut out[start..])
            .ok()?;
        out[ENCRYPTED_HEADER_BYTES..start].copy_from_slice(&tag);
        self.next_sequence = sequence.wrapping_add(1);
        Some(out)
    }
}

/// Opens host messages.
pub struct Opener {
    gcm: Gcm,
    v2: bool,
    replay: ReplayWindow,
}

impl Opener {
    pub fn new(key: &Key, v2: bool) -> Self {
        Self {
            gcm: Gcm::new(key),
            v2,
            replay: ReplayWindow::default(),
        }
    }

    /// Returns the message type and payload. As in moonlight-common-c, bytes past the
    /// envelope's declared length are ignored, and so is the inner length field.
    pub fn open(&mut self, datagram: &[u8]) -> Result<(u16, Vec<u8>), OpenError> {
        if datagram.len() < 2 {
            return Err(OpenError::Runt);
        }
        let kind = u16::from_le_bytes([datagram[0], datagram[1]]);
        if kind != ENCRYPTED_HEADER_TYPE {
            return Err(OpenError::Unencrypted(kind));
        }
        if datagram.len() < ENCRYPTED_HEADER_BYTES {
            return Err(OpenError::Runt);
        }
        let length = usize::from(u16::from_le_bytes([datagram[2], datagram[3]]));
        if datagram.len() < length + 4 || length < 4 + TAG_BYTES + INNER_HEADER_BYTES {
            return Err(OpenError::Runt);
        }
        let sequence = u32::from_le_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]);
        let iv = control_iv(sequence, self.v2, Origin::Host);
        let tag = &datagram[ENCRYPTED_HEADER_BYTES..ENCRYPTED_HEADER_BYTES + TAG_BYTES];
        let ciphertext = &datagram[ENCRYPTED_HEADER_BYTES + TAG_BYTES..length + 4];
        let plain = self
            .gcm
            .open(iv.as_slice(), tag, ciphertext)
            .map_err(|_| OpenError::Authentication)?;
        if !self.replay.accept(sequence) {
            return Err(OpenError::Replay(sequence));
        }
        let message_type = u16::from_le_bytes([plain[0], plain[1]]);
        Ok((message_type, plain[INNER_HEADER_BYTES..].to_vec()))
    }
}

const REPLAY_WINDOW: u32 = 4096;

/// Rejects a host sequence number seen before, after authentication. The host numbers
/// messages with one counter across channels that ENet orders only within themselves, so
/// a reliable message can arrive far behind newer ones; the window is wide for that.
pub struct ReplayWindow {
    newest: Option<u32>,
    seen: Box<[u64; REPLAY_WINDOW as usize / 64]>,
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self {
            newest: None,
            seen: Box::new([0; REPLAY_WINDOW as usize / 64]),
        }
    }
}

impl ReplayWindow {
    fn slot(sequence: u32) -> (usize, u64) {
        let index = (sequence % REPLAY_WINDOW) as usize;
        (index / 64, 1 << (index % 64))
    }

    pub fn accept(&mut self, sequence: u32) -> bool {
        let Some(newest) = self.newest else {
            self.newest = Some(sequence);
            let (word, bit) = Self::slot(sequence);
            self.seen[word] |= bit;
            return true;
        };
        let ahead = sequence.wrapping_sub(newest);
        if ahead != 0 && ahead < u32::MAX / 2 {
            if ahead >= REPLAY_WINDOW {
                self.seen.fill(0);
            } else {
                for skipped in 1..=ahead {
                    let (word, bit) = Self::slot(newest.wrapping_add(skipped));
                    self.seen[word] &= !bit;
                }
            }
            self.newest = Some(sequence);
            let (word, bit) = Self::slot(sequence);
            self.seen[word] |= bit;
            return true;
        }
        let behind = newest.wrapping_sub(sequence);
        let (word, bit) = Self::slot(sequence);
        if behind >= REPLAY_WINDOW || self.seen[word] & bit != 0 {
            return false;
        }
        self.seen[word] |= bit;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::packet;

    const KEY: Key = *b"fedcba9876543210";

    #[test]
    fn sealed_messages_decrypt_with_the_host() {
        for v2 in [true, false] {
            let mut sealer = Sealer::new(&KEY, v2);
            for sequence in 0..3u32 {
                let payload = vec![sequence as u8; 5 + sequence as usize];
                let datagram = sealer.seal(0x5531, &payload).unwrap();
                let (seq, kind, body) = packet::decrypt_control(&KEY, &datagram, v2).unwrap();
                assert_eq!((seq, kind, body), (sequence, 0x5531, payload));
            }
            assert_eq!(sealer.next_sequence(), 3);
        }
    }

    #[test]
    fn large_payloads_seal_and_oversized_ones_do_not() {
        let mut sealer = Sealer::new(&KEY, true);
        let payload = vec![0xa5; 972];
        let datagram = sealer.seal(0x5534, &payload).unwrap();
        assert_eq!(
            packet::decrypt_control(&KEY, &datagram, true).unwrap().2,
            payload
        );
        assert!(sealer.seal(1, &vec![0; MAX_PAYLOAD + 1]).is_none());
        assert_eq!(sealer.next_sequence(), 1);
    }

    #[test]
    fn host_messages_open_and_replays_fail() {
        let mut opener = Opener::new(&KEY, true);
        let datagram = packet::encrypted_control(&KEY, 7, 0x010b, &[1, 2, 3], true).unwrap();
        assert_eq!(opener.open(&datagram), Ok((0x010b, vec![1, 2, 3])));
        assert_eq!(opener.open(&datagram), Err(OpenError::Replay(7)));
        let older = packet::encrypted_control(&KEY, 3, 0x010e, &[0], true).unwrap();
        assert_eq!(opener.open(&older), Ok((0x010e, vec![0])));

        let mut tampered = packet::encrypted_control(&KEY, 8, 0x010b, &[1], true).unwrap();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x40;
        assert_eq!(opener.open(&tampered), Err(OpenError::Authentication));
        // The tampered copy did not burn the sequence number.
        let genuine = packet::encrypted_control(&KEY, 8, 0x010b, &[1], true).unwrap();
        assert!(opener.open(&genuine).is_ok());

        assert_eq!(
            opener.open(&[0x0b, 0x01, 0, 0]),
            Err(OpenError::Unencrypted(0x010b))
        );
        assert_eq!(opener.open(&datagram[..20]), Err(OpenError::Runt));
        let mut trailing = packet::encrypted_control(&KEY, 9, 0x010b, &[4], true).unwrap();
        trailing.extend_from_slice(&[0xee; 3]);
        assert_eq!(opener.open(&trailing), Ok((0x010b, vec![4])));

        let mut legacy = Opener::new(&KEY, false);
        let datagram = packet::encrypted_control(&KEY, 300, 0x0109, &[0, 0, 0, 0], false).unwrap();
        assert_eq!(legacy.open(&datagram), Ok((0x0109, vec![0; 4])));
    }

    #[test]
    fn replay_window_accepts_late_unseen_sequences() {
        let mut window = ReplayWindow::default();
        assert!(window.accept(10));
        assert!(window.accept(500));
        assert!(window.accept(11));
        assert!(!window.accept(11));
        assert!(window.accept(4000));
        assert!(window.accept(9000));
        assert!(!window.accept(9000 - REPLAY_WINDOW));
        assert!(window.accept(100_000));
        assert!(window.accept(99_999));
        assert!(!window.accept(99_999));
    }
}
