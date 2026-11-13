//! The primitives internet play uses, all from ring: HKDF-SHA256 for every derived key, Ed25519
//! for the DHT items (BEP 44 requires it), ChaCha20-Poly1305 for records and the punched path,
//! and SHA-1 only because BEP 44 names an item by it.

use ring::{
    aead, digest, hkdf,
    rand::{SecureRandom, SystemRandom},
    signature::{self, KeyPair},
};

pub type Key = [u8; 32];

/// The HKDF salt of every derivation: it binds the keys to this protocol and version.
const DOMAIN: &[u8] = b"rubylight internet play v1";

struct Len(usize);
impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF-SHA256 of `secret` with `label` and `context` as its info, filling `out` (at most
/// 8160 bytes).
pub fn derive(secret: &[u8], label: &[u8], context: &[u8], out: &mut [u8]) {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, DOMAIN).extract(secret);
    let info = [label, context];
    prk.expand(&info, Len(out.len()))
        .and_then(|okm| okm.fill(out))
        .expect("an HKDF output of at most 255 blocks");
}

pub fn derive_key(secret: &[u8], label: &[u8], context: &[u8]) -> Key {
    let mut key = [0; 32];
    derive(secret, label, context, &mut key);
    key
}

/// The Ed25519 public key of the key pair `seed` makes.
pub fn public_key(seed: &Key) -> [u8; 32] {
    let pair =
        signature::Ed25519KeyPair::from_seed_unchecked(seed).expect("any 32 bytes are a seed");
    let mut public = [0; 32];
    public.copy_from_slice(pair.public_key().as_ref());
    public
}

pub fn sign(seed: &Key, message: &[u8]) -> [u8; 64] {
    let pair =
        signature::Ed25519KeyPair::from_seed_unchecked(seed).expect("any 32 bytes are a seed");
    let mut signed = [0; 64];
    signed.copy_from_slice(pair.sign(message).as_ref());
    signed
}

pub fn verify(public: &[u8; 32], message: &[u8], signed: &[u8]) -> bool {
    signature::UnparsedPublicKey::new(&signature::ED25519, public)
        .verify(message, signed)
        .is_ok()
}

pub fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut context = digest::Context::new(&digest::SHA1_FOR_LEGACY_USE_ONLY);
    for part in parts {
        context.update(part);
    }
    let mut out = [0; 20];
    out.copy_from_slice(context.finish().as_ref());
    out
}

/// Random bytes from the system's generator.
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system random generator works");
    bytes
}

/// A ChaCha20-Poly1305 key, set up once for many seals.
pub struct Aead(aead::LessSafeKey);

pub const TAG_BYTES: usize = 16;

impl Aead {
    pub fn new(key: &Key) -> Self {
        Self(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key).expect("a 32-byte key"),
        ))
    }
    /// Encrypts `data` in place and appends the tag. Each nonce must be used once per key.
    pub fn seal(&self, nonce: [u8; 12], aad: &[u8], data: &mut Vec<u8>) {
        self.0
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad),
                data,
            )
            .expect("a message within ChaCha20-Poly1305's limit");
    }
    /// Encrypts `out[aad_bytes..]` in place under the AAD `out[..aad_bytes]`, then appends the
    /// tag: a datagram sealed in the buffer that carries it. Each nonce must be used once per key.
    pub fn seal_after(&self, nonce: [u8; 12], out: &mut Vec<u8>, aad_bytes: usize) {
        let (aad, body) = out.split_at_mut(aad_bytes);
        let tag = self
            .0
            .seal_in_place_separate_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&*aad),
                body,
            )
            .expect("a message within ChaCha20-Poly1305's limit");
        out.extend_from_slice(tag.as_ref());
    }
    /// Decrypts `data` (ciphertext and tag) in place, leaving the plaintext; false when it
    /// does not authenticate.
    pub fn open(&self, nonce: [u8; 12], aad: &[u8], data: &mut Vec<u8>) -> bool {
        let length = match self.0.open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            data,
        ) {
            Ok(plain) => plain.len(),
            Err(_) => return false,
        };
        data.truncate(length);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivations_depend_on_every_input() {
        let a = derive_key(b"secret", b"label", b"context");
        assert_eq!(a, derive_key(b"secret", b"label", b"context"));
        assert_ne!(a, derive_key(b"secret!", b"label", b"context"));
        assert_ne!(a, derive_key(b"secret", b"label!", b"context"));
        assert_ne!(a, derive_key(b"secret", b"label", b"context!"));
    }

    #[test]
    fn signatures_verify_only_for_their_message_and_key() {
        let seed = [7; 32];
        let public = public_key(&seed);
        let signed = sign(&seed, b"message");
        assert!(verify(&public, b"message", &signed));
        assert!(!verify(&public, b"massage", &signed));
        assert!(!verify(&public_key(&[8; 32]), b"message", &signed));
    }

    #[test]
    fn seals_open_only_unchanged() {
        let key = Aead::new(&[1; 32]);
        let mut data = b"hello".to_vec();
        key.seal([0; 12], b"aad", &mut data);
        assert_eq!(data.len(), 5 + TAG_BYTES);
        let mut opened = data.clone();
        assert!(key.open([0; 12], b"aad", &mut opened));
        assert_eq!(opened, b"hello");
        let mut wrong_aad = data.clone();
        assert!(!key.open([0; 12], b"aaD", &mut wrong_aad));
        let mut flipped = data.clone();
        flipped[0] ^= 1;
        assert!(!key.open([0; 12], b"aad", &mut flipped));
        assert!(!Aead::new(&[2; 32]).open([0; 12], b"aad", &mut data));
    }

    #[test]
    fn sha1_of_parts_is_sha1_of_their_concatenation() {
        // FIPS 180 test vector "abc".
        assert_eq!(
            sha1(&[b"a", b"bc"]),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d
            ]
        );
    }
}
