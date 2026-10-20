//! Microphone datagrams, client to host, on their own UDP port.
//!
//! Ported from Nightjar's `android_microphone_packet.h`, itself from ClassicOldSong's
//! moonlight-common-c `MicrophoneStream.c` 784fa1d0 (GPL-3.0), the client half of Apollo's
//! microphone passthrough, which Rubylight hosts also take.
//!
//! One datagram per Opus packet: flags 0, type 0x61, sequence (LE u16), client
//! milliseconds (LE u32), magic 0x12345678 (LE), then the Opus packet AES-128-CBC encrypted
//! under the launch's `rikey` with [`crate::crypto::rikey_iv`] and the reference's double
//! padding ([`CbcPadding::BlockThenPkcs7`]).

use crate::crypto::{cbc_encrypt, rikey_iv, CbcPadding, Key};

pub const PACKET_TYPE_OPUS: u8 = 0x61;
pub const MAGIC: u32 = 0x1234_5678;
pub const HEADER_BYTES: usize = 12;
pub const MAX_PACKET_BYTES: usize = 1400;
/// The SDP lines with which a host offers a microphone; the client sends mono either way.
pub const SDP_RTPMAP: &str = "a=rtpmap:96 opus/48000/1";
pub const SDP_RTPMAP_STEREO: &str = "a=rtpmap:96 opus/48000/2";

/// The datagram for one Opus packet, or `None` when it is empty or would exceed
/// [`MAX_PACKET_BYTES`].
pub fn build_packet(
    key: &Key,
    key_id: u32,
    sequence: u16,
    timestamp_ms: u32,
    opus: &[u8],
) -> Option<Vec<u8>> {
    if opus.is_empty() || opus.len() > MAX_PACKET_BYTES - HEADER_BYTES {
        return None;
    }
    let iv = rikey_iv(key_id, u32::from(sequence));
    let ciphertext = cbc_encrypt(key, &iv, opus, CbcPadding::BlockThenPkcs7).ok()?;
    if HEADER_BYTES + ciphertext.len() > MAX_PACKET_BYTES {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER_BYTES + ciphertext.len());
    out.extend_from_slice(&[0, PACKET_TYPE_OPUS]);
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&timestamp_ms.to_le_bytes());
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&ciphertext);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::mic;

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
}
