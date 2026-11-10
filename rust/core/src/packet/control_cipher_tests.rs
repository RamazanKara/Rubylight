use super::*;

/// The control stream's cipher as each message made it: the key expanded for that
/// message alone.
fn sealed_per_message(key: &[u8; 16], seq: u32, kind: u16, payload: &[u8], v2: bool) -> Vec<u8> {
    let (iv, iv_len) = control_iv(seq, v2, b'H');
    let mut p = Vec::new();
    p.extend_from_slice(&1u16.to_le_bytes());
    p.extend_from_slice(&((24 + payload.len()) as u16).to_le_bytes());
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&[0; 16]);
    p.extend_from_slice(&kind.to_le_bytes());
    p.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    p.extend_from_slice(payload);
    let (tag, sealed) = crypto::gcm_seal(key, &iv[..iv_len], &p[24..]).unwrap();
    p[24..].copy_from_slice(&sealed);
    p[8..24].copy_from_slice(&tag);
    p
}

/// A client's message `seq`, sealed as moonlight-common-c does (`C` for the client).
fn from_client(key: &[u8; 16], seq: u32, kind: u16, payload: &[u8], v2: bool) -> Vec<u8> {
    let (iv, iv_len) = control_iv(seq, v2, b'C');
    let mut plain = kind.to_le_bytes().to_vec();
    plain.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    plain.extend_from_slice(payload);
    let (tag, sealed) = crypto::gcm_seal(key, &iv[..iv_len], &plain).unwrap();
    let mut p = 1u16.to_le_bytes().to_vec();
    // The length counts what follows it: sequence, tag and the sealed kind, length and payload.
    p.extend_from_slice(&((20 + plain.len()) as u16).to_le_bytes());
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&tag);
    p.extend_from_slice(&sealed);
    p
}

#[test]
fn a_kept_control_cipher_seals_and_opens_as_one_made_per_message() {
    let key = std::array::from_fn(|i| (i * 7 + 3) as u8);
    for v2 in [false, true] {
        let cipher = ControlCipher::new(&key, v2);
        let sent: [(u32, &[u8]); 4] = [(0, b""), (1, b"input"), (255, &[9; 300]), (256, b"x")];
        for (seq, payload) in sent {
            let expected = sealed_per_message(&key, seq, 0x0206, payload, v2);
            assert_eq!(cipher.seal(seq, 0x0206, payload).unwrap(), expected);
            assert_eq!(
                encrypted_control(&key, seq, 0x0206, payload, v2).unwrap(),
                expected
            );
            let received = from_client(&key, seq, 0x0206, payload, v2);
            let opened = (seq, 0x0206, payload.to_vec());
            assert_eq!(cipher.open(&received).unwrap(), opened);
            assert_eq!(decrypt_control(&key, &received, v2).unwrap(), opened);
        }
        // Version 2 seals each direction under its own IV, so the host's own messages are not
        // the client's; version 1's IV is the sequence alone in both directions.
        let echoed = cipher.seal(4, 0x0206, b"input").unwrap();
        assert_eq!(cipher.open(&echoed).is_err(), v2);
        let mut forged = from_client(&key, 5, 0x0206, b"input", v2);
        forged[30] ^= 1;
        assert!(cipher.open(&forged).is_err());
        assert!(cipher.seal(0, 1, &[0; u16::MAX as usize]).is_err());
    }
}

#[test]
fn control_ciphers_are_made_once_per_key_and_again_when_it_changes() {
    let first = [1; 16];
    let rotated = [2; 16];
    let mut ciphers = ControlCiphers::default();
    for seq in 0..3 {
        let sealed = ciphers.get(&first, true).seal(seq, 7, b"a").unwrap();
        let expected = encrypted_control(&first, seq, 7, b"a", true).unwrap();
        assert_eq!(sealed, expected);
    }
    assert_eq!(ciphers.made, 1, "the same key reuses its cipher");
    // A new key: its messages, and no longer the old key's.
    let cipher = ciphers.get(&rotated, true);
    assert_eq!(
        cipher.seal(3, 7, b"a").unwrap(),
        encrypted_control(&rotated, 3, 7, b"a", true).unwrap()
    );
    assert!(cipher.open(&from_client(&first, 3, 7, b"a", true)).is_err());
    let received = from_client(&rotated, 3, 7, b"a", true);
    assert_eq!(cipher.open(&received).unwrap(), (3, 7, b"a".to_vec()));
    assert_eq!(ciphers.made, 2);
    // Another protocol version with the same key takes its own IVs.
    let cipher = ciphers.get(&rotated, false);
    assert_eq!(
        cipher.seal(4, 7, b"a").unwrap(),
        encrypted_control(&rotated, 4, 7, b"a", false).unwrap()
    );
    assert_eq!(ciphers.made, 3);
    // Back to the first key: made again, not the rotated one kept.
    let cipher = ciphers.get(&first, false);
    assert!(cipher.is_for(&first, false));
    assert!(!cipher.is_for(&rotated, false));
    assert!(!cipher.is_for(&first, true));
    ciphers.get(&first, false);
    assert_eq!(ciphers.made, 4);
}
