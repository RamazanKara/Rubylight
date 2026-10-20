//! AES primitives of the Moonlight protocol: AES-128-GCM for the control stream (and
//! Sunshine's encrypted video and audio), AES-128-CBC for audio, microphone and the legacy
//! input stream.
//!
//! Ported from moonlight-common-c's `PlatformCrypto.c` and the IV rules in `ControlStream.c`
//! (GPL-3.0, Cameron Gutman and contributors).

use aes_gcm::aead::consts::U16;
use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::aes::Aes128;
use aes_gcm::{Aes128Gcm, AesGcm};
use cbc::cipher::block_padding::{NoPadding, Pkcs7};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};

pub const KEY_BYTES: usize = 16;
pub const TAG_BYTES: usize = 16;
pub const BLOCK_BYTES: usize = 16;

/// The launch's remote input key (`rikey`), which also keys the control stream.
pub type Key = [u8; KEY_BYTES];

/// Encryption or authentication failed, or a length was wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CryptoError;

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AES operation failed")
    }
}

impl std::error::Error for CryptoError {}

/// An AES-128-GCM key with its schedule expanded once. Takes the 12-byte IVs of Sunshine's
/// streams and the 16-byte IVs of GFE's original encrypted control stream.
pub struct Gcm {
    iv12: Aes128Gcm,
    iv16: AesGcm<Aes128, U16>,
}

impl Gcm {
    pub fn new(key: &Key) -> Self {
        Self {
            iv12: Aes128Gcm::new(key.into()),
            iv16: AesGcm::new(key.into()),
        }
    }

    /// Encrypts `buffer` in place and returns the tag.
    pub fn seal_in_place(&self, iv: &[u8], buffer: &mut [u8]) -> Result<[u8; TAG_BYTES], CryptoError> {
        let tag = match iv.len() {
            12 => self
                .iv12
                .encrypt_in_place_detached(GenericArray::from_slice(iv), b"", buffer),
            16 => self
                .iv16
                .encrypt_in_place_detached(GenericArray::from_slice(iv), b"", buffer),
            _ => return Err(CryptoError),
        }
        .map_err(|_| CryptoError)?;
        Ok(tag.into())
    }

    /// Returns the tag and the ciphertext.
    pub fn seal(&self, iv: &[u8], plaintext: &[u8]) -> Result<([u8; TAG_BYTES], Vec<u8>), CryptoError> {
        let mut buffer = plaintext.to_vec();
        let tag = self.seal_in_place(iv, &mut buffer)?;
        Ok((tag, buffer))
    }

    /// Authenticates and decrypts; a wrong tag, key or IV is an error.
    pub fn open(&self, iv: &[u8], tag: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if tag.len() != TAG_BYTES {
            return Err(CryptoError);
        }
        let mut buffer = ciphertext.to_vec();
        let tag = GenericArray::from_slice(tag);
        match iv.len() {
            12 => self
                .iv12
                .decrypt_in_place_detached(GenericArray::from_slice(iv), b"", &mut buffer, tag),
            16 => self
                .iv16
                .decrypt_in_place_detached(GenericArray::from_slice(iv), b"", &mut buffer, tag),
            _ => return Err(CryptoError),
        }
        .map_err(|_| CryptoError)?;
        Ok(buffer)
    }
}

/// Who sealed a control message; part of the V2 IV so the two directions never share one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Client,
    Host,
}

/// An IV of 12 or 16 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Iv {
    bytes: [u8; 16],
    len: usize,
}

impl Iv {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// The IV of control message `sequence`. With `SS_ENC_CONTROL_V2` it is 12 bytes: the
/// sequence little-endian, then `'C'` or `'H'` for the origin and `'C'` for the control
/// stream at bytes 10 and 11. Otherwise it is GFE's 16 bytes holding only the sequence's low
/// byte.
pub fn control_iv(sequence: u32, v2: bool, origin: Origin) -> Iv {
    let mut bytes = [0; 16];
    if v2 {
        bytes[..4].copy_from_slice(&sequence.to_le_bytes());
        bytes[10] = match origin {
            Origin::Client => b'C',
            Origin::Host => b'H',
        };
        bytes[11] = b'C';
        Iv { bytes, len: 12 }
    } else {
        bytes[0] = sequence as u8;
        Iv { bytes, len: 16 }
    }
}

/// The CBC IV of the audio and microphone streams: the big-endian 32-bit sum (wrapping) of
/// the launch's `rikeyid` and the packet's sequence number, then 12 zero bytes.
pub fn rikey_iv(key_id: u32, sequence: u32) -> [u8; 16] {
    let mut iv = [0; 16];
    iv[..4].copy_from_slice(&key_id.wrapping_add(sequence).to_be_bytes());
    iv
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CbcPadding {
    /// The input must be a whole number of blocks.
    None,
    /// PKCS#7: always appends 1 to 16 bytes.
    Pkcs7,
    /// moonlight-common-c's `CIPHER_FLAG_PAD_TO_BLOCK_SIZE` followed by OpenSSL's PKCS#7: the
    /// input is first padded to a whole block with bytes holding the pad length (none when it
    /// already is one), then a full PKCS#7 block follows. Microphone packets use it.
    BlockThenPkcs7,
}

pub fn cbc_encrypt(key: &Key, iv: &[u8; 16], data: &[u8], padding: CbcPadding) -> Result<Vec<u8>, CryptoError> {
    let encryptor = cbc::Encryptor::<Aes128>::new(key.into(), iv.into());
    match padding {
        CbcPadding::None => {
            if data.len() % BLOCK_BYTES != 0 {
                return Err(CryptoError);
            }
            Ok(encryptor.encrypt_padded_vec_mut::<NoPadding>(data))
        }
        CbcPadding::Pkcs7 => Ok(encryptor.encrypt_padded_vec_mut::<Pkcs7>(data)),
        CbcPadding::BlockThenPkcs7 => {
            let pad = (BLOCK_BYTES - data.len() % BLOCK_BYTES) % BLOCK_BYTES;
            let mut padded = Vec::with_capacity(data.len() + pad);
            padded.extend_from_slice(data);
            padded.resize(data.len() + pad, pad as u8);
            Ok(encryptor.encrypt_padded_vec_mut::<Pkcs7>(&padded))
        }
    }
}

/// Decrypts and, with `pkcs7`, removes and checks PKCS#7 padding.
pub fn cbc_decrypt(key: &Key, iv: &[u8; 16], data: &[u8], pkcs7: bool) -> Result<Vec<u8>, CryptoError> {
    if data.is_empty() || data.len() % BLOCK_BYTES != 0 {
        return Err(CryptoError);
    }
    let decryptor = cbc::Decryptor::<Aes128>::new(key.into(), iv.into());
    if pkcs7 {
        decryptor
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .map_err(|_| CryptoError)
    } else {
        decryptor
            .decrypt_padded_vec_mut::<NoPadding>(data)
            .map_err(|_| CryptoError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: Key = *b"0123456789abcdef";

    #[test]
    fn control_ivs_follow_the_c() {
        let iv = control_iv(0x0403_0201, true, Origin::Client);
        assert_eq!(iv.as_slice(), &[1, 2, 3, 4, 0, 0, 0, 0, 0, 0, b'C', b'C']);
        let iv = control_iv(0x0403_0201, true, Origin::Host);
        assert_eq!(iv.as_slice()[10], b'H');
        let iv = control_iv(0x0403_0201, false, Origin::Client);
        assert_eq!(iv.as_slice().len(), 16);
        assert_eq!(iv.as_slice()[0], 1);
        assert!(iv.as_slice()[1..].iter().all(|&b| b == 0));
    }

    #[test]
    fn gcm_round_trips_with_both_iv_sizes_and_rejects_tampering() {
        let gcm = Gcm::new(&KEY);
        for iv_len in [12, 16] {
            let iv = vec![7; iv_len];
            let (tag, sealed) = gcm.seal(&iv, b"hello control").unwrap();
            assert_ne!(sealed, b"hello control");
            assert_eq!(gcm.open(&iv, &tag, &sealed).unwrap(), b"hello control");
            let mut bad = sealed.clone();
            bad[0] ^= 1;
            assert_eq!(gcm.open(&iv, &tag, &bad), Err(CryptoError));
            assert_eq!(gcm.open(&iv[..iv_len - 1], &tag, &sealed), Err(CryptoError));
        }
        assert_eq!(gcm.seal(&[0; 8], b"x"), Err(CryptoError));
    }

    #[test]
    fn gcm_matches_the_hosts_implementation() {
        let iv = control_iv(9, true, Origin::Client);
        let (tag, sealed) = Gcm::new(&KEY).seal(iv.as_slice(), b"interop").unwrap();
        let opened = butterpollo_core::crypto::gcm_open(&KEY, iv.as_slice(), &tag, &sealed).unwrap();
        assert_eq!(opened, b"interop");
        let (tag, sealed) = butterpollo_core::crypto::gcm_seal(&KEY, &[3; 16], b"legacy").unwrap();
        assert_eq!(Gcm::new(&KEY).open(&[3; 16], &tag, &sealed).unwrap(), b"legacy");
    }

    #[test]
    fn cbc_paddings() {
        let iv = rikey_iv(0xffff_fffe, 3);
        assert_eq!(&iv[..4], &[0, 0, 0, 1]);
        assert!(iv[4..].iter().all(|&b| b == 0));

        let sealed = cbc_encrypt(&KEY, &iv, &[1; 16], CbcPadding::Pkcs7).unwrap();
        assert_eq!(sealed.len(), 32);
        assert_eq!(cbc_decrypt(&KEY, &iv, &sealed, true).unwrap(), [1; 16]);
        assert_eq!(butterpollo_core::crypto::cbc_open(&KEY, &iv, &sealed).unwrap(), [1; 16]);

        let sealed = cbc_encrypt(&KEY, &iv, &[1; 16], CbcPadding::None).unwrap();
        assert_eq!(sealed.len(), 16);
        assert_eq!(cbc_encrypt(&KEY, &iv, &[1; 15], CbcPadding::None), Err(CryptoError));

        let sealed = cbc_encrypt(&KEY, &iv, &[9; 13], CbcPadding::BlockThenPkcs7).unwrap();
        assert_eq!(sealed.len(), 32);
        let opened = cbc_decrypt(&KEY, &iv, &sealed, true).unwrap();
        assert_eq!(&opened[..13], &[9; 13]);
        assert_eq!(&opened[13..], &[3, 3, 3]);
        assert_eq!(cbc_decrypt(&KEY, &iv, &sealed[..31], true), Err(CryptoError));
    }
}
