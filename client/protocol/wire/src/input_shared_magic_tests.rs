//! Input magic 0x55000009 is two packets: Foundation Sunshine's one-finger touchpad packet
//! (40 bytes, length field 36), which [`crate::input::decode`] reads, and Nonary's 20-byte
//! controller speaker binding (length field 16), which [`Binding::decode`] reads. These tests
//! prove no packet is ever both, whatever its length or contents, cut short or with bytes after
//! it, so a binding can never move a touchpad finger and a touchpad packet never binds a speaker.

use crate::input::{Input, TOUCHPAD_MAGIC, decode};
use rubylight_protocol::ps_speaker_pcm::{BINDING_BYTES, BINDING_MAGIC, Binding};

/// A valid touchpad packet: finger 0x01020304 moved to (0.25, 0.5).
fn touchpad() -> Vec<u8> {
    let mut b = vec![0, 0, 0, 36];
    b.extend_from_slice(&TOUCHPAD_MAGIC.to_le_bytes());
    b.extend_from_slice(&[3, 1, 0xff, 0xff, 4, 3, 2, 1, 105, 0, 70, 0]);
    for v in [0.25f32, 0.5, 1.0, 0.0, 0.0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(b.len(), 40);
    b
}

fn binding() -> Vec<u8> {
    Binding {
        slot: 2,
        owner: 0x0102_0304_0506_0708,
    }
    .encode()
    .unwrap()
    .to_vec()
}

fn is_touchpad(b: &[u8]) -> bool {
    matches!(decode(b), Ok(Input::Touchpad { frame: false, .. }))
}

fn is_binding(b: &[u8]) -> bool {
    Binding::decode(b).is_some()
}

/// Neither reader takes `b`, or exactly one does and `b` has that packet's exact length.
fn assert_disjoint(b: &[u8]) {
    let (touchpad, binding) = (is_touchpad(b), is_binding(b));
    assert!(!(touchpad && binding), "both read {b:02x?}");
    if touchpad {
        assert_eq!((b.len(), &b[..4]), (40, &[0, 0, 0, 36][..]), "{b:02x?}");
    }
    if binding {
        assert_eq!((b.len(), &b[..4]), (20, &[0, 0, 0, 16][..]), "{b:02x?}");
    }
}

#[test]
fn the_magic_is_the_same() {
    assert_eq!(TOUCHPAD_MAGIC, BINDING_MAGIC);
    assert_eq!(BINDING_BYTES, 20);
}

#[test]
fn each_valid_packet_is_read_by_its_own_reader_only() {
    let (touchpad, binding) = (touchpad(), binding());
    assert!(is_touchpad(&touchpad));
    assert!(!is_binding(&touchpad));
    assert!(is_binding(&binding));
    assert!(
        decode(&binding).is_err(),
        "a binding never decodes as input"
    );
}

#[test]
fn cut_short_or_extended_neither_is_the_other() {
    for valid in [touchpad(), binding()] {
        for len in 0..valid.len() {
            let cut = &valid[..len];
            assert_disjoint(cut);
            assert!(
                !is_touchpad(cut) && !is_binding(cut),
                "{len} bytes of {valid:02x?}"
            );
            // The same prefix with its length field saying what is there.
            if len >= 8 {
                let mut framed = cut.to_vec();
                framed[..4].copy_from_slice(&(len as u32 - 4).to_be_bytes());
                assert_disjoint(&framed);
                assert!(
                    !is_touchpad(&framed) && !is_binding(&framed),
                    "{framed:02x?}"
                );
            }
        }
        for extra in 1..=48 {
            let mut long = valid.clone();
            long.resize(valid.len() + extra, 0);
            assert!(!is_touchpad(&long) && !is_binding(&long));
            let declared = (long.len() as u32 - 4).to_be_bytes();
            long[..4].copy_from_slice(&declared);
            assert_disjoint(&long);
            assert!(!is_binding(&long), "{} bytes", long.len());
            // A binding with 20 bytes after it and length field 36 has a touchpad packet's
            // length, so it is one, read as such; nothing longer is a touchpad packet.
            assert_eq!(is_touchpad(&long), long.len() == 40 && valid.len() == 20);
        }
    }
}

#[test]
fn a_length_field_of_the_other_packet_is_refused_by_both() {
    let mut binding_as_touchpad = binding();
    binding_as_touchpad[..4].copy_from_slice(&36u32.to_be_bytes());
    assert!(!is_touchpad(&binding_as_touchpad) && !is_binding(&binding_as_touchpad));
    let mut touchpad_as_binding = touchpad();
    touchpad_as_binding[..4].copy_from_slice(&16u32.to_be_bytes());
    assert!(!is_touchpad(&touchpad_as_binding) && !is_binding(&touchpad_as_binding));
    // A binding padded to 40 bytes is a touchpad packet's size, but not a binding.
    let mut padded = binding();
    padded.resize(40, 0);
    padded[..4].copy_from_slice(&36u32.to_be_bytes());
    assert!(!is_binding(&padded));
    assert_disjoint(&padded);
}

#[test]
fn no_packet_of_any_length_or_content_is_both() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for len in 0..=96usize {
        for round in 0..400 {
            let mut b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if len >= 8 {
                b[4..8].copy_from_slice(&BINDING_MAGIC.to_le_bytes());
                if round % 2 == 0 {
                    b[..4].copy_from_slice(&(len as u32 - 4).to_be_bytes());
                }
                // Plausible fields: small numbers where both readers check them.
                if round % 4 < 2 && len > 12 {
                    b[8] = (next() % 18) as u8;
                    b[9] = 0;
                    b[10] = (next() % 3) as u8;
                    b[11] = 0;
                }
            }
            assert_disjoint(&b);
        }
    }
}
