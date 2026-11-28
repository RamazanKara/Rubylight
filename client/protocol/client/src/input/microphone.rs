//! Microphone datagrams, client to host, on their own UDP port.
//!
//! Ported from Nightjar's `android_microphone_packet.h`, itself from ClassicOldSong's
//! moonlight-common-c `MicrophoneStream.c` 784fa1d0 (GPL-3.0), the client half of Apollo's
//! microphone passthrough, which Rubylight hosts also take.
//!
//! One datagram per Opus packet: flags 0, type 0x61, sequence (LE u16), client
//! milliseconds (LE u32), magic 0x12345678 (LE), then the Opus packet AES-128-CBC encrypted
//! under the launch's `rikey` with [`crate::crypto::rikey_iv`] and the reference's double
//! padding ([`crate::crypto::CbcPadding::BlockThenPkcs7`]).

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockEncryptMut, KeyInit};
use cbc::cipher::InnerIvInit;

use crate::crypto::{BLOCK_BYTES, Key, rikey_iv};

pub const PACKET_TYPE_OPUS: u8 = 0x61;
pub const MAGIC: u32 = 0x1234_5678;
pub const HEADER_BYTES: usize = 12;
pub const MAX_PACKET_BYTES: usize = 1400;
/// The SDP lines with which a host offers a microphone; the client sends mono either way.
pub const SDP_RTPMAP: &str = "a=rtpmap:96 opus/48000/1";
pub const SDP_RTPMAP_STEREO: &str = "a=rtpmap:96 opus/48000/2";

/// Writes one microphone datagram into `out` using a pre-expanded AES-128 cipher and returns
/// its byte length, or `None` when `opus` is empty or the datagram exceeds [`MAX_PACKET_BYTES`]
/// or `out.len()`.
pub fn build_packet_into(
    cipher: &aes::Aes128,
    key_id: u32,
    sequence: u16,
    timestamp_ms: u32,
    opus: &[u8],
    out: &mut [u8],
) -> Option<usize> {
    if opus.is_empty() {
        return None;
    }
    let pad = (BLOCK_BYTES - opus.len() % BLOCK_BYTES) % BLOCK_BYTES;
    let padded_len = opus.len().checked_add(pad)?;
    let cipher_len = padded_len.checked_add(BLOCK_BYTES)?;
    let total = HEADER_BYTES.checked_add(cipher_len)?;
    if total > MAX_PACKET_BYTES || total > out.len() {
        return None;
    }
    out[0] = 0;
    out[1] = PACKET_TYPE_OPUS;
    out[2..4].copy_from_slice(&sequence.to_le_bytes());
    out[4..8].copy_from_slice(&timestamp_ms.to_le_bytes());
    out[8..12].copy_from_slice(&MAGIC.to_le_bytes());
    let body = &mut out[HEADER_BYTES..total];
    body[..opus.len()].copy_from_slice(opus);
    body[opus.len()..padded_len].fill(pad as u8);
    let iv = rikey_iv(key_id, u32::from(sequence));
    let encryptor = cbc::Encryptor::<&aes::Aes128>::inner_iv_init(cipher, &iv.into());
    let written = encryptor.encrypt_padded_mut::<Pkcs7>(body, padded_len).ok()?.len();
    debug_assert_eq!(written, cipher_len);
    Some(HEADER_BYTES + written)
}

/// Writes one single-PKCS#7 microphone datagram (`x-rl-mic.framing:1`) into `out`, avoiding the
/// legacy double-padding ambiguity on 16-byte-aligned Opus frames ending in `0x01`/`[k; k]`.
pub fn build_packet_single_into(
    cipher: &aes::Aes128,
    key_id: u32,
    sequence: u16,
    timestamp_ms: u32,
    opus: &[u8],
    out: &mut [u8],
) -> Option<usize> {
    if opus.is_empty() {
        return None;
    }
    let cipher_len = (opus.len() / BLOCK_BYTES).checked_add(1)?.checked_mul(BLOCK_BYTES)?;
    let total = HEADER_BYTES.checked_add(cipher_len)?;
    if total > MAX_PACKET_BYTES || total > out.len() {
        return None;
    }
    out[0] = 0;
    out[1] = PACKET_TYPE_OPUS;
    out[2..4].copy_from_slice(&sequence.to_le_bytes());
    out[4..8].copy_from_slice(&timestamp_ms.to_le_bytes());
    out[8..12].copy_from_slice(&MAGIC.to_le_bytes());
    let body = &mut out[HEADER_BYTES..total];
    body[..opus.len()].copy_from_slice(opus);
    let iv = rikey_iv(key_id, u32::from(sequence));
    let encryptor = cbc::Encryptor::<&aes::Aes128>::inner_iv_init(cipher, &iv.into());
    let written = encryptor.encrypt_padded_mut::<Pkcs7>(body, opus.len()).ok()?.len();
    debug_assert_eq!(written, cipher_len);
    Some(HEADER_BYTES + written)
}

/// Dispatches single-PKCS#7 (`single_pkcs7 == true`) vs legacy double-padded microphone framing.
pub fn build_packet_framed_into(
    cipher: &aes::Aes128,
    key_id: u32,
    sequence: u16,
    timestamp_ms: u32,
    opus: &[u8],
    single_pkcs7: bool,
    out: &mut [u8],
) -> Option<usize> {
    if single_pkcs7 {
        build_packet_single_into(cipher, key_id, sequence, timestamp_ms, opus, out)
    } else {
        build_packet_into(cipher, key_id, sequence, timestamp_ms, opus, out)
    }
}

/// The single-PKCS#7 datagram for one Opus packet, or `None` when empty or over [`MAX_PACKET_BYTES`].
pub fn build_packet_single(
    key: &Key,
    key_id: u32,
    sequence: u16,
    timestamp_ms: u32,
    opus: &[u8],
) -> Option<Vec<u8>> {
    let cipher = aes::Aes128::new(key.into());
    let mut buf = [0u8; MAX_PACKET_BYTES];
    let len = build_packet_single_into(&cipher, key_id, sequence, timestamp_ms, opus, &mut buf)?;
    Some(buf[..len].to_vec())
}

/// The datagram for one Opus packet, or `None` when it is empty or would exceed
/// [`MAX_PACKET_BYTES`].
pub fn build_packet(
    key: &Key,
    key_id: u32,
    sequence: u16,
    timestamp_ms: u32,
    opus: &[u8],
) -> Option<Vec<u8>> {
    let cipher = aes::Aes128::new(key.into());
    let mut buf = [0u8; MAX_PACKET_BYTES];
    let len = build_packet_into(&cipher, key_id, sequence, timestamp_ms, opus, &mut buf)?;
    Some(buf[..len].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_wire::mic;

    const KEY: Key = *b"mic-key-16-bytes";

    #[test]
    fn matches_the_hosts_reference_and_opens_there() {
        for len in [1, 15, 16, 17, 200, 1360] {
            let opus: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let ours = build_packet(&KEY, 0xffff_fff0, 40, 1234, &opus).unwrap();
            assert_eq!(
                ours,
                mic::seal(&KEY, 0xffff_fff0, 40, 1234, &opus),
                "len {len}"
            );
            let packet = mic::parse(&ours).unwrap();
            assert_eq!(mic::open(&KEY, 0xffff_fff0, &packet).unwrap(), opus);
        }
        assert!(build_packet(&KEY, 0, 0, 0, &[]).is_none());
        assert!(build_packet(&KEY, 0, 0, 0, &[0; 1361]).is_none());
    }

    #[test]
    fn single_pkcs7_round_trips_aligned_frames_ending_in_padding_patterns() {
        for pad in 1u8..16 {
            let mut opus = vec![0x5c; 32];
            opus[32 - usize::from(pad)..].fill(pad);
            let ours = build_packet_single(&KEY, 0x1234, u16::from(pad), 555, &opus).unwrap();
            assert_eq!(
                ours,
                mic::seal_single(&KEY, 0x1234, u16::from(pad), 555, &opus),
                "pad {pad}"
            );
            let packet = mic::parse(&ours).unwrap();
            assert_eq!(
                mic::open_single(&KEY, 0x1234, &packet).unwrap(),
                opus,
                "pad {pad}"
            );
        }
        for len in [1, 15, 16, 17, 200, 1375] {
            let opus: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(3)).collect();
            let ours = build_packet_single(&KEY, 0x99, len as u16, 7, &opus).unwrap();
            let packet = mic::parse(&ours).unwrap();
            assert_eq!(mic::open_single(&KEY, 0x99, &packet).unwrap(), opus);
        }
        assert!(build_packet_single(&KEY, 0, 0, 0, &[]).is_none());
        assert!(build_packet_single(&KEY, 0, 0, 0, &[0; 1376]).is_none());
    }
}
