//! Video receive: C and Rust on identical packet sequences.
#![cfg(difftest_c)]

use nightjar_core::video::VideoConfig;
use nightjar_difftest::host::{
    bitstream, VideoFrame, VideoHost, FRAME_AFTER_INVALIDATION, FRAME_IDR, FRAME_P,
};
use nightjar_difftest::net::{Impairment, Rng};
use nightjar_difftest::{run_video, Step};

const H264: u32 = 0x0001;
const HEVC: u32 = 0x0100;
const AV1: u32 = 0x1000;
const PYROWAVE: u32 = 0x01_0000;
const SUNSHINE: [i32; 4] = [7, 1, 431, -1];

struct Session {
    config: VideoConfig,
    host: VideoHost,
    rng: Rng,
    steps: Vec<Step>,
    now: u64,
    /// Complete queued frames after every packet with this decoder result.
    decode: Option<bool>,
}

impl Session {
    fn new(format: u32, seed: u64) -> Self {
        Self {
            config: VideoConfig {
                format,
                packet_size: 1024,
                app_version: SUNSHINE,
                rfi: false,
                pyrowave_records: false,
                early_chunks: false,
                colorspace: 1,
                encryption_key: None,
            },
            host: VideoHost::new(1024, 20),
            rng: Rng::new(seed),
            steps: Vec::new(),
            now: 1_000_000,
            decode: Some(true),
        }
    }

    fn payload(&mut self, idr: bool, len: usize) -> Vec<u8> {
        let extras = self.rng.below(3) == 0;
        let format = self.config.format;
        if format & 0xF != 0 {
            bitstream::h264(&mut self.rng, idr, len, extras)
        } else if format & 0xF00 != 0 {
            bitstream::hevc(&mut self.rng, idr, len, extras)
        } else {
            bitstream::body(&mut self.rng, len.max(1), true)
        }
    }

    /// A frame of about `packets` shards.
    fn frame(&mut self, frame_type: u8, packets: usize) -> VideoFrame {
        let slice = self.host.packet_size - 16;
        let len = (packets * slice).saturating_sub(8 + self.rng.below(slice));
        let payload = self.payload(frame_type == FRAME_IDR, len.max(1));
        let mut frame = VideoFrame::new(payload, frame_type, self.host.frame.wrapping_mul(1500));
        frame.latency_us = self.rng.below(60_000) as u64;
        frame.trim_last = self.rng.below(4) == 0;
        frame
    }

    fn deliver(&mut self, packets: Vec<Vec<u8>>) {
        for packet in packets {
            self.steps.push(Step::Packet(packet, self.now));
            self.now += 20 + self.rng.below(200) as u64;
            if let Some(ok) = self.decode {
                self.steps.push(Step::Complete(ok));
            }
        }
    }

    fn send(&mut self, frame: &VideoFrame, impairment: Impairment) {
        let packets = self.host.encode(frame);
        let packets = impairment.apply(&mut self.rng, packets);
        self.deliver(packets);
    }

    fn send_clean(&mut self, frame_type: u8, packets: usize) {
        let frame = self.frame(frame_type, packets);
        self.send(&frame, Impairment::default());
    }

    fn run(&self) -> usize {
        run_video(&self.config, 0, &self.steps)
    }
}

/// A stream of key frames and P-frames of random sizes.
fn stream(session: &mut Session, frames: usize, impairment: Impairment, key_interval: usize) {
    for n in 0..frames {
        let frame_type = if n % key_interval == 0 {
            FRAME_IDR
        } else {
            FRAME_P
        };
        let packets = match session.rng.below(4) {
            0 => 1,
            1 => 2 + session.rng.below(4),
            2 => 5 + session.rng.below(30),
            _ => 30 + session.rng.below(90),
        };
        let frame = session.frame(frame_type, packets);
        session.send(&frame, impairment);
    }
}

#[test]
fn in_order_streams() {
    for (format, records) in [
        (H264, false),
        (HEVC, false),
        (AV1, false),
        (PYROWAVE, false),
        (PYROWAVE, true),
    ] {
        let mut session = Session::new(format, u64::from(format) + 7);
        session.config.pyrowave_records = records;
        stream(&mut session, 40, Impairment::default(), 1000);
        assert_eq!(session.run(), 40, "format {format:#x} records {records}");
    }
}

#[test]
fn loss_reordering_and_duplicates() {
    let mut frames = 0;
    for seed in 0..6u64 {
        for format in [H264, HEVC, AV1] {
            for rfi in [false, true] {
                let mut session = Session::new(format, seed * 1000 + u64::from(format));
                session.config.rfi = rfi;
                let impairment = Impairment {
                    loss: [0.005, 0.02, 0.05, 0.1, 0.2, 0.01][seed as usize],
                    duplicate: 0.02,
                    reorder_window: [1, 4, 8, 16, 3, 32][seed as usize],
                };
                stream(&mut session, 60, impairment, 25);
                frames += session.run();
            }
        }
    }
    assert!(frames > 500, "{frames} frames delivered");
}

#[test]
fn shard_loss_within_and_beyond_parity() {
    for format in [H264, AV1] {
        for rfi in [false, true] {
            let mut session = Session::new(format, 11);
            session.config.rfi = rfi;
            session.send_clean(FRAME_IDR, 20);
            for case in 0..12usize {
                // Key frames, so each case decodes on its own.
                let frame = session.frame(FRAME_IDR, 10 + case);
                let mut packets = session.host.encode(&frame);
                let data = 10 + case;
                let parity = packets.len() - data;
                // Lose data shards, parity shards, or both; some cases exceed the parity.
                let lost: Vec<usize> = match case % 4 {
                    0 => (0..parity).map(|i| i * 3).collect(),
                    1 => vec![0, data - 1],
                    2 => (data..packets.len()).chain([1]).collect(),
                    _ => (0..=parity).map(|i| 2 * i).collect(),
                };
                let mut index = 0;
                packets.retain(|_| {
                    index += 1;
                    !lost.contains(&(index - 1))
                });
                if case % 3 == 0 {
                    packets.reverse();
                }
                session.deliver(packets);
            }
            session.send_clean(FRAME_P, 3);
            session.run();
        }
    }
}

#[test]
fn multi_block_frames() {
    for format in [HEVC, AV1] {
        for seed in 0..4u64 {
            let mut session = Session::new(format, 100 + seed);
            session.config.rfi = seed % 2 == 1;
            session.send_clean(FRAME_IDR, 40);
            for n in 0..30usize {
                let mut frame = {
                    // Key frames often enough that lost frames do not end the stream.
                    let frame_type = if n % 3 == 2 { FRAME_IDR } else { FRAME_P };
                    let packets = 8 + session.rng.below(60);
                    session.frame(frame_type, packets)
                };
                frame.min_blocks = 2 + n % 3;
                let packets = session.host.encode(&frame);
                let packets = match n % 6 {
                    // Lose one whole block.
                    1 => {
                        let block = session.rng.below(frame.min_blocks) as u8;
                        packets
                            .into_iter()
                            .filter(|p| (p[27] >> 4) & 3 != block)
                            .collect()
                    }
                    // Lose a few packets everywhere.
                    3 => Impairment {
                        loss: 0.08,
                        ..Default::default()
                    }
                    .apply(&mut session.rng, packets),
                    // Reorder across block boundaries.
                    5 => Impairment {
                        reorder_window: 12,
                        ..Default::default()
                    }
                    .apply(&mut session.rng, packets),
                    _ => packets,
                };
                session.deliver(packets);
            }
            session.run();
        }
    }
}

#[test]
fn key_frame_loss_and_recovery() {
    for format in [H264, HEVC, AV1] {
        for rfi in [false, true] {
            let mut session = Session::new(format, 5);
            session.config.rfi = rfi;
            // The first key frame is lost entirely.
            let lost = session.frame(FRAME_IDR, 12);
            session.host.encode(&lost);
            for _ in 0..5 {
                session.send_clean(FRAME_P, 3);
            }
            session.send_clean(FRAME_IDR, 15);
            for _ in 0..5 {
                session.send_clean(FRAME_P, 4);
            }
            // A P-frame is lost; with RFI the host answers with a post-invalidation frame.
            let lost = session.frame(FRAME_P, 6);
            session.host.encode(&lost);
            session.send_clean(FRAME_P, 2);
            session.send_clean(FRAME_AFTER_INVALIDATION, 5);
            session.send_clean(FRAME_P, 2);
            // A key frame loses half its packets.
            let frame = session.frame(FRAME_IDR, 20);
            let packets = session.host.encode(&frame);
            let half = packets.into_iter().step_by(2).collect();
            session.deliver(half);
            for _ in 0..4 {
                session.send_clean(FRAME_P, 3);
            }
            session.send_clean(FRAME_IDR, 8);
            session.send_clean(FRAME_P, 1);
            session.run();
        }
    }
}

#[test]
fn decoder_feedback_and_queue_overflow() {
    for format in [H264, AV1] {
        // The decoder asks for a key frame now and then.
        let mut session = Session::new(format, 21);
        session.config.rfi = true;
        session.decode = None;
        for n in 0..40 {
            let frame_type = if n % 15 == 0 { FRAME_IDR } else { FRAME_P };
            let frame = session.frame(frame_type, 1 + n % 7);
            session.send(&frame, Impairment::default());
            session.steps.push(Step::Complete(n % 9 != 4));
        }
        session.run();

        // Nobody drains the queue: it overflows at 15 frames, and then at 3.
        for bound in [0, 3] {
            let mut session = Session::new(format, 22);
            session.decode = None;
            for n in 0..50 {
                let frame_type = if n % 20 == 0 { FRAME_IDR } else { FRAME_P };
                session.send_clean(frame_type, 1 + n % 3);
                if n % 23 == 22 {
                    session.steps.push(Step::Complete(true));
                }
            }
            run_video(&session.config, bound, &session.steps);
        }
    }
}

#[test]
fn sequence_and_frame_wraparound() {
    for format in [H264, AV1] {
        // RTP sequence numbers cross 0xFFFF.
        let mut session = Session::new(format, 31);
        session.host.sequence = 0x7F00;
        session.send_clean(FRAME_IDR, 30);
        let impairment = Impairment {
            loss: 0.01,
            reorder_window: 6,
            duplicate: 0.01,
        };
        for _ in 0..250 {
            let frame = {
                let n = 60 + session.rng.below(100);
                session.frame(FRAME_P, n)
            };
            session.send(&frame, impairment);
        }
        session.run();

        // The 24-bit stream packet index wraps, by jumping the sequence between frames.
        let mut session = Session::new(format, 32);
        session.send_clean(FRAME_IDR, 4);
        for _ in 0..600 {
            session.host.sequence = session.host.sequence.wrapping_add(30_000);
            session.send_clean(FRAME_P, 1);
        }
        assert!(session.host.sequence > 0x0100_0000);
        session.run();

        // Frame numbers cross 0xFFFF and jump within the RTP queue's 16-bit window.
        for rfi in [false, true] {
            let mut session = Session::new(format, 33);
            session.config.rfi = rfi;
            session.send_clean(FRAME_IDR, 3);
            for jump in [20_000u32, 30_000, 25_000, 32_000, 1, 1] {
                session.host.frame = session.host.frame.wrapping_add(jump - 1);
                session.send_clean(FRAME_P, 2);
                session.send_clean(FRAME_IDR, 2);
                session.send_clean(FRAME_P, 2);
            }
            assert!(session.host.frame > 0x1_0000);
            session.run();
        }
    }
}

#[test]
fn pyrowave_records() {
    for seed in 0..6u64 {
        let mut session = Session::new(PYROWAVE, 40 + seed);
        session.config.pyrowave_records = true;
        session.config.rfi = seed % 2 == 0;
        for n in 0..40usize {
            let mut frame = {
                let n = 2 + session.rng.below(80);
                session.frame(FRAME_IDR, n)
            };
            frame.min_blocks = 1 + n % 4;
            let shards = (frame.payload.len() + 8).div_ceil(session.host.packet_size - 16);
            frame.record_starts = (0..shards).filter(|_| session.rng.below(3) == 0).collect();
            let impairment = match n % 5 {
                0 => Impairment::default(),
                1 => Impairment {
                    loss: 0.05,
                    ..Default::default()
                },
                2 => Impairment {
                    loss: 0.3,
                    ..Default::default()
                },
                3 => Impairment {
                    reorder_window: 8,
                    duplicate: 0.05,
                    ..Default::default()
                },
                _ => Impairment {
                    loss: 0.02,
                    reorder_window: 5,
                    duplicate: 0.0,
                },
            };
            session.send(&frame, impairment);
        }
        // A whole frame and a whole block go missing.
        let lost = session.frame(FRAME_IDR, 10);
        session.host.encode(&lost);
        let mut frame = session.frame(FRAME_IDR, 30);
        frame.min_blocks = 3;
        let packets: Vec<_> = session
            .host
            .encode(&frame)
            .into_iter()
            .filter(|p| (p[27] >> 4) & 3 != 1)
            .collect();
        session.deliver(packets);
        session.send_clean(FRAME_IDR, 5);
        session.run();
    }
}

#[test]
fn encrypted_video() {
    for format in [H264, AV1] {
        let mut session = Session::new(format, 50);
        let key = [0x42u8; 16];
        session.config.encryption_key = Some(key);
        session.host.key = Some(key);
        session.send_clean(FRAME_IDR, 10);
        let mut replay = Vec::new();
        for n in 0..60usize {
            let frame = {
                let n = 1 + session.rng.below(40);
                session.frame(FRAME_P, n)
            };
            let packets = session.host.encode(&frame);
            let mut packets = Impairment {
                loss: 0.03,
                reorder_window: 4,
                duplicate: 0.02,
            }
            .apply(&mut session.rng, packets);
            if n % 7 == 3 && !packets.is_empty() {
                // A forged packet next to the real one.
                let at = session.rng.below(packets.len());
                let mut forged = packets[at].clone();
                let byte = 32 + session.rng.below(forged.len() - 32);
                forged[byte] ^= 0x10;
                packets.insert(at, forged);
            }
            if n == 10 {
                replay = packets.clone();
            }
            if n == 40 {
                // Old frames: dropped by the frame number check or the RTP queue.
                packets.extend(replay.iter().cloned());
            }
            session.deliver(packets);
        }
        session.run();
    }
}

#[test]
fn older_host_versions() {
    let versions: [([i32; 4], u8); 8] = [
        ([7, 1, 450, 0], 0x01),
        ([7, 1, 446, 0], 0x81),
        ([7, 1, 415, 0], 0x81),
        ([7, 1, 415, 0], 0x01),
        ([7, 1, 350, 0], 0x01),
        ([7, 1, 320, 0], 0x01),
        ([5, 0, 0, 0], 0x01),
        ([3, 1, 0, 0], 0x01),
    ];
    for (version, header) in versions {
        for format in [H264, HEVC, AV1] {
            let mut session = Session::new(format, 60);
            session.config.app_version = version;
            session.host.header_byte = header;
            let impairment = Impairment {
                loss: 0.02,
                reorder_window: 3,
                duplicate: 0.0,
            };
            stream(&mut session, 25, impairment, 10);
            session.run();
        }
    }
}

#[test]
fn hdr_ltr_and_frame_types() {
    for format in [H264, HEVC, AV1] {
        let mut session = Session::new(format, 70);
        session.config.rfi = true;
        session.send_clean(FRAME_IDR, 5);
        for n in 0..30usize {
            if n % 10 == 5 {
                session.steps.push(Step::Hdr(n % 20 == 5));
            }
            let frame_type = [FRAME_P, 4, FRAME_AFTER_INVALIDATION, 104, 9][n % 5];
            let mut frame = session.frame(frame_type, 1 + n % 4);
            frame.ltr = n % 3 == 0;
            if n % 8 == 7 {
                session.host.encode(&frame);
            } else {
                session.send(&frame, Impairment::default());
            }
        }
        session.run();
    }
}

#[test]
fn corrupted_and_short_datagrams() {
    for format in [H264, HEVC, AV1, PYROWAVE] {
        for seed in 0..3u64 {
            let mut session = Session::new(format, 80 + seed);
            session.config.pyrowave_records = format == PYROWAVE && seed == 1;
            session.config.rfi = seed == 2;
            for n in 0..50usize {
                let frame_type = if n % 12 == 0 { FRAME_IDR } else { FRAME_P };
                let frame = {
                    let n = 1 + session.rng.below(20);
                    session.frame(frame_type, n)
                };
                let mut packets = session.host.encode(&frame);
                for packet in &mut packets {
                    // Flip payload bytes, and now and then a header byte.
                    if session.rng.below(10) == 0 {
                        let at = 32 + session.rng.below(packet.len() - 32);
                        packet[at] = session.rng.byte();
                    }
                    if session.rng.below(40) == 0 {
                        let at = [1, 12, 13, 16, 17, 25, 26, 32, 33, 35][session.rng.below(10)];
                        packet[at] = session.rng.byte();
                    }
                    if session.rng.below(50) == 0 {
                        let len = session.rng.below(packet.len());
                        packet.truncate(len);
                    }
                }
                if session.rng.below(5) == 0 {
                    let len = session.rng.below(60);
                    packets.push((0..len).map(|_| session.rng.byte()).collect());
                }
                session.deliver(packets);
            }
            session.run();
        }
    }
}
