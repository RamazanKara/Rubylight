//! `SS_ENC_VIDEO`: each datagram is a 32-byte `ENC_VIDEO_HEADER` (12-byte IV, frame number,
//! 16-byte tag) followed by the AES-128-GCM encrypted RTP packet, without associated data.

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::Aes128Gcm;

use super::ENC_VIDEO_HEADER_SIZE;

pub(super) struct Decryptor {
    cipher: Aes128Gcm,
}

impl Decryptor {
    pub(super) fn new(key: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128Gcm::new(GenericArray::from_slice(key)),
        }
    }

    /// Decrypts `datagram` into `out` and returns the plaintext length, or `None` if it does
    /// not authenticate or does not fit.
    pub(super) fn open(&self, datagram: &[u8], out: &mut [u8]) -> Option<usize> {
        let ciphertext = &datagram[ENC_VIDEO_HEADER_SIZE..];
        let plain = out.get_mut(..ciphertext.len())?;
        plain.copy_from_slice(ciphertext);
        self.cipher
            .decrypt_in_place_detached(
                GenericArray::from_slice(&datagram[..12]),
                &[],
                plain,
                GenericArray::from_slice(&datagram[16..32]),
            )
            .ok()?;
        Some(ciphertext.len())
    }
}
