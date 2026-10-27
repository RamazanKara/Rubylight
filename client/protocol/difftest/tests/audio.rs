//! Audio receive: C and Rust on identical packet sequences.
#![cfg(difftest_c)]

use nightjar_core::audio::{AudioConfig, AudioEncryption};
use nightjar_difftest::host::AudioHost;
use nightjar_difftest::net::{Impairment, Rng};
use nightjar_difftest::{run_audio, Step};

fn config(duration: u32) -> AudioConfig {
    AudioConfig {
        packet_duration_ms: duration,
        app_version: [7, 1, 431, -1],
        encryption: None,
    }
}

struct Session {
    host: AudioHost,
    rng: Rng,
    steps: Vec<Step>,
    now: u64,
}

impl Session {
    fn new(duration: u32, seed: u64) -> Self {
        Self {
            host: AudioHost::new(duration),
            rng: Rng::new(seed),
            steps: Vec::new(),
            now: 1_000_000,
        }
    }

    /// `count` Opus packets of `size` bytes (random when 0), through `impairment`.
    fn send(&mut self, count: usize, size: usize, impairment: Impairment) {
        let mut packets = Vec::new();
        for _ in 0..count {
            let len = if size == 0 {
                1 + self.rng.below(200)
            } else {
                size
            };
            let opus: Vec<u8> = (0..len).map(|_| self.rng.byte()).collect();
            packets.extend(self.host.encode(&opus));
        }
        let duration = u64::from(self.host.packet_ms) * 1000;
        for packet in impairment.apply(&mut self.rng, packets) {
            self.steps.push(Step::Packet(packet, self.now));
            // Data packets arrive one duration apart, with jitter.
            self.now += duration / 2 + self.rng.below(duration as usize) as u64;
        }
    }
}

#[test]
fn in_order_after_the_initial_backlog() {
    for duration in [5, 10, 20] {
        let mut session = Session::new(duration, u64::from(duration));
        session.send(1200 / duration as usize, 60, Impairment::default());
        let played = run_audio(&config(duration), &session.steps);
        assert!(played > 0);
    }
}

#[test]
fn loss_reordering_and_duplicates() {
    for seed in 0..12u64 {
        let duration = [5, 10, 20][seed as usize % 3];
        let mut session = Session::new(duration, 100 + seed);
        session.steps.push(Step::Timeout);
        session.send(2, 80, Impairment::default());
        session.steps.push(Step::Timeout);
        let impairment = Impairment {
            loss: [0.01, 0.05, 0.15, 0.3][seed as usize % 4],
            duplicate: 0.03,
            reorder_window: [1, 3, 7, 12][(seed as usize / 2) % 4],
        };
        session.send(800, 100, impairment);
        run_audio(&config(duration), &session.steps);
    }
}

#[test]
fn whole_blocks_lost_with_out_of_sequence_waits() {
    let mut session = Session::new(5, 7);
    session.steps.push(Step::Timeout);
    session.send(1, 50, Impairment::default());
    session.steps.push(Step::Timeout);
    for round in 0..40 {
        session.send(
            4 + round % 9,
            50,
            Impairment {
                reorder_window: 6,
                ..Default::default()
            },
        );
        // A burst loses consecutive blocks.
        let lost = session.rng.below(14);
        for _ in 0..lost {
            session.host.encode(&[0; 50]);
        }
        if round % 5 == 0 {
            session.now += 50_000;
        }
    }
    run_audio(&config(5), &session.steps);
}

#[test]
fn encrypted_audio() {
    let mut session = Session::new(5, 9);
    let (key, iv) = ([9u8; 16], [1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    session.host.encryption = Some((key, 0x0102_0304));
    session.send(
        400,
        77,
        Impairment {
            loss: 0.1,
            duplicate: 0.02,
            reorder_window: 4,
        },
    );
    // Corrupt ciphertext: decryption fails or yields different bytes in both.
    for step in session.steps.iter_mut().step_by(17) {
        if let Step::Packet(packet, _) = step {
            let at = packet.len() - 1;
            packet[at] ^= 0x55;
        }
    }
    let mut cfg = config(5);
    cfg.encryption = Some(AudioEncryption { key, iv });
    run_audio(&cfg, &session.steps);
}

#[test]
fn sequence_wraparound() {
    for start in [0xFF00u16, 0xFFF8, 0x7FF0] {
        let mut session = Session::new(5, u64::from(start));
        session.host.sequence = start;
        session.send(
            600,
            40,
            Impairment {
                loss: 0.05,
                reorder_window: 3,
                duplicate: 0.01,
            },
        );
        run_audio(&config(5), &session.steps);
    }
}

#[test]
fn incompatible_hosts() {
    // Old GFE: FEC disabled entirely.
    let mut session = Session::new(5, 11);
    session.send(
        300,
        40,
        Impairment {
            loss: 0.05,
            reorder_window: 3,
            duplicate: 0.02,
        },
    );
    let mut cfg = config(5);
    cfg.app_version = [7, 1, 400, 0];
    run_audio(&cfg, &session.steps);

    // Variable packet sizes: parity covers the longest payload, data packets differ.
    let mut session = Session::new(10, 12);
    session.send(
        300,
        0,
        Impairment {
            loss: 0.05,
            ..Default::default()
        },
    );
    run_audio(&config(10), &session.steps);

    // FEC blocks not aligned to four packets.
    let mut session = Session::new(5, 13);
    session.host.sequence = 2;
    session.send(
        300,
        40,
        Impairment {
            loss: 0.05,
            ..Default::default()
        },
    );
    run_audio(&config(5), &session.steps);
}

#[test]
fn corrupted_and_short_datagrams() {
    for seed in 0..4u64 {
        let mut session = Session::new(5, 200 + seed);
        session.steps.push(Step::Timeout);
        session.send(1, 30, Impairment::default());
        session.steps.push(Step::Timeout);
        session.send(
            500,
            30,
            Impairment {
                loss: 0.05,
                reorder_window: 4,
                duplicate: 0.02,
            },
        );
        let mut rng = Rng::new(seed);
        for step in &mut session.steps {
            if let Step::Packet(packet, _) = step {
                match rng.below(30) {
                    0 => packet.truncate(rng.below(packet.len())),
                    1 => {
                        let at = rng.below(packet.len());
                        packet[at] = rng.byte();
                    }
                    2 => packet[12] = rng.byte(),
                    3 => packet[1] = rng.byte(),
                    _ => {}
                }
            }
        }
        run_audio(&config(5), &session.steps);
    }
}
