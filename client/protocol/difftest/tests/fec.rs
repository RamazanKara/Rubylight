//! Reed-Solomon: nightjar-core's port against nanors.
#![cfg(difftest_c)]

use nightjar_core::fec::ReedSolomon;
use nightjar_difftest::c::{rs_decode, rs_encode};
use nightjar_difftest::net::Rng;

fn shards(rng: &mut Rng, count: usize, size: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|_| (0..size).map(|_| rng.byte()).collect())
        .collect()
}

fn rust_encode(data: usize, set: &mut [Vec<u8>], size: usize) {
    let rs = ReedSolomon::new(data, set.len() - data).unwrap();
    let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
    rs.encode(&mut refs, size).unwrap();
}

fn rust_decode(data: usize, set: &mut [Vec<u8>], missing: &[bool], size: usize) -> bool {
    let rs = ReedSolomon::new(data, set.len() - data).unwrap();
    let mut refs: Vec<&mut [u8]> = set.iter_mut().map(|s| &mut s[..]).collect();
    rs.decode(&mut refs, missing, size).is_ok()
}

#[test]
fn encode_matches_nanors() {
    let _guard = nightjar_difftest::C_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut rng = Rng::new(1);
    for _ in 0..120 {
        let data = 1 + rng.below(200);
        let parity = 1 + rng.below((255 - data).min(60));
        let size = 1 + rng.below(1200);
        let mut c = shards(&mut rng, data + parity, size);
        let mut r = c.clone();
        assert!(rs_encode(data, &mut c, size));
        rust_encode(data, &mut r, size);
        assert_eq!(c, r, "{data}+{parity} x {size}");
    }
}

#[test]
fn decode_matches_nanors() {
    let _guard = nightjar_difftest::C_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut rng = Rng::new(2);
    for round in 0..160 {
        let data = 1 + rng.below(220);
        let parity = 1 + rng.below((255 - data).min(40));
        let size = 1 + rng.below(700);
        let mut set = shards(&mut rng, data + parity, size);
        // Half the rounds decode consistent codewords, the rest arbitrary bytes.
        if round % 2 == 0 {
            rust_encode(data, &mut set, size);
        }
        let erasures = rng.below(parity + 2);
        let mut missing = vec![false; data + parity];
        for _ in 0..erasures {
            missing[rng.below(data + parity)] = true;
        }
        for (shard, &gone) in set.iter_mut().zip(&missing) {
            if gone {
                shard.fill(0xEE);
            }
        }
        let (mut c, mut r) = (set.clone(), set);
        let c_ok = rs_decode(data, &mut c, &missing, size);
        let r_ok = rust_decode(data, &mut r, &missing, size);
        assert_eq!(c_ok, r_ok, "{data}+{parity} missing {missing:?}");
        if c_ok {
            assert_eq!(c[..data], r[..data], "{data}+{parity} x {size}");
        }
    }
}

#[test]
fn rejects_what_nanors_rejects() {
    let _guard = nightjar_difftest::C_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for (data, parity) in [(0, 1), (1, 0), (200, 56), (255, 1), (254, 1)] {
        let mut set = vec![vec![1u8; 4]; data + parity];
        let c = data + parity > 0 && rs_encode(data, &mut set, 4);
        assert_eq!(
            c,
            ReedSolomon::new(data, parity).is_some(),
            "{data}+{parity}"
        );
    }
}
