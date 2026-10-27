//! Client roaming's known answers (`rubylight_protocol::roaming::vectors`) were computed with
//! OpenSSL. This reproduces them with RustCrypto's AES and AES-GCM, the ciphers nightjar-core
//! seals and opens roaming datagrams with, so the two implementations agree byte for byte.

use aes::cipher::BlockEncrypt;
use aes::Aes128;
use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::Aes128Gcm;
use rubylight_protocol::roaming::{
    vectors, Datagram, Header, Kind, Payload, DATAGRAM_BYTES, KEY_BLOCK, PAYLOAD_BYTES, SESSION_TAG_BYTES, TAG_BLOCK,
};

fn ecb(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let mut out = GenericArray::clone_from_slice(block);
    Aes128::new(GenericArray::from_slice(key)).encrypt_block(&mut out);
    out.into()
}

fn session_tag(rikey: &[u8; 16]) -> [u8; SESSION_TAG_BYTES] {
    let mut tag = [0; SESSION_TAG_BYTES];
    tag.copy_from_slice(&ecb(rikey, &TAG_BLOCK)[..SESSION_TAG_BYTES]);
    tag
}

fn seal(key: &[u8; 16], header: Header, payload: Payload) -> [u8; DATAGRAM_BYTES] {
    assert_eq!(header.kind, payload.kind());
    let mut ciphertext = payload.encode();
    let gcm_tag = Aes128Gcm::new(GenericArray::from_slice(key))
        .encrypt_in_place_detached(GenericArray::from_slice(&header.nonce()), &header.aad(), &mut ciphertext)
        .unwrap();
    Datagram { header, gcm_tag: gcm_tag.into(), ciphertext }.encode()
}

fn open(key: &[u8; 16], wire: &[u8]) -> Option<Payload> {
    let datagram = Datagram::decode(wire).ok()?;
    let mut plaintext: [u8; PAYLOAD_BYTES] = datagram.ciphertext;
    Aes128Gcm::new(GenericArray::from_slice(key))
        .decrypt_in_place_detached(
            GenericArray::from_slice(&datagram.header.nonce()),
            &datagram.header.aad(),
            &mut plaintext,
            GenericArray::from_slice(&datagram.gcm_tag),
        )
        .ok()?;
    Payload::decode(datagram.header.kind, &plaintext)
}

fn cases() -> [(Header, Payload, [u8; DATAGRAM_BYTES]); 4] {
    let header = |kind, seq| Header { kind, session_tag: vectors::SESSION_TAG, seq };
    [
        (
            header(Kind::Probe, 1),
            Payload::Probe { nonce_c: vectors::NONCE_C, flags: vectors::PROBE_FLAGS },
            vectors::PROBE_DATAGRAM,
        ),
        (
            header(Kind::Challenge, 1),
            Payload::Challenge { nonce_c: vectors::NONCE_C, challenge: vectors::CHALLENGE },
            vectors::CHALLENGE_DATAGRAM,
        ),
        (header(Kind::Confirm, 2), Payload::Confirm { challenge: vectors::CHALLENGE }, vectors::CONFIRM_DATAGRAM),
        (header(Kind::Migrated, 2), Payload::Migrated { challenge: vectors::CHALLENGE }, vectors::MIGRATED_DATAGRAM),
    ]
}

#[test]
fn key_and_session_tag_derive_from_the_rikey() {
    assert_eq!(ecb(&vectors::RIKEY, &KEY_BLOCK), vectors::KEY);
    assert_eq!(session_tag(&vectors::RIKEY), vectors::SESSION_TAG);
}

#[test]
fn sealed_datagrams_match_the_known_answers() {
    for (header, payload, expected) in cases() {
        assert_eq!(seal(&vectors::KEY, header, payload), expected, "{:?}", header.kind);
        assert_eq!(open(&vectors::KEY, &expected), Some(payload));
    }
}

#[test]
fn any_changed_bit_fails_to_open() {
    for (_, _, wire) in cases() {
        for bit in 0..DATAGRAM_BYTES * 8 {
            let mut altered = wire;
            altered[bit / 8] ^= 1 << (bit % 8);
            assert_eq!(open(&vectors::KEY, &altered), None, "bit {bit}");
        }
    }
    let mut other_key = vectors::KEY;
    other_key[0] ^= 1;
    assert_eq!(open(&other_key, &vectors::PROBE_DATAGRAM), None);
}
