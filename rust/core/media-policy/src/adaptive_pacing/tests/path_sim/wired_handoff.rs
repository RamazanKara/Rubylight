//! Wired post-loss pacing evaluation matrix across bottlenecks, queues, FEC levels, and RTT.

use super::*;
use crate::keyframe_loss::{KeyframeLoss, LatchKind};
use std::collections::VecDeque;

#[derive(Clone, Copy)]
enum WiredCandidate {
    FixedSafe,
    Bare40,
    Cand2Half,
    Cand2HalfLockPkt,
    Cand2ThreeQuarters,
    Cand2PktHalf,
    Cand3LowFec,
    Cand3PktHalfLockLowFec,
    Cand3PktHalf,
    Cand3ThreeQtrPktHalf,
}

impl WiredCandidate {
    fn label(self) -> &'static str {
        match self {
            Self::FixedSafe => "FixedSafe",
            Self::Bare40 => "Cand1_Bare40",
            Self::Cand2Half => "Cand2a_Half",
            Self::Cand2HalfLockPkt => "Cand2a_LockPkt",
            Self::Cand2ThreeQuarters => "Cand2b_3Q",
            Self::Cand2PktHalf => "Cand2c_PktHalf",
            Self::Cand3LowFec => "Cand3_3QCut",
            Self::Cand3PktHalfLockLowFec => "Cand3b_HalfLock",
            Self::Cand3PktHalf => "Cand3c_Winner",
            Self::Cand3ThreeQtrPktHalf => "Cand3d_3Q_Half",
        }
    }

    fn config(self) -> Option<PostLossConfig> {
        match self {
            Self::FixedSafe => None,
            Self::Bare40 => Some(PostLossConfig::BARE_40),
            Self::Cand2Half => Some(PostLossConfig::CAND_2_HALF),
            Self::Cand2HalfLockPkt => Some(PostLossConfig::CAND_2_HALF_LOCK_PKT),
            Self::Cand2ThreeQuarters => Some(PostLossConfig::CAND_2_THREE_QUARTERS),
            Self::Cand2PktHalf => Some(PostLossConfig::CAND_2_PKT_HALF),
            Self::Cand3LowFec => Some(PostLossConfig::CAND_3_3Q_GUARD),
            Self::Cand3PktHalfLockLowFec => Some(PostLossConfig::CAND_3_PKT_HALF_LOCK),
            Self::Cand3PktHalf => Some(PostLossConfig::DEFAULT),
            Self::Cand3ThreeQtrPktHalf => Some(PostLossConfig::CAND_3_3Q_PKT_HALF),
        }
    }
}

#[derive(Clone, Copy)]
enum FrameMix {
    DurKMixed,
    PeriodicKeyframes,
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    egress_bps: u64,
    bitrate_kbps: u32,
    fec_percent: usize,
    queue_bytes: usize,
    feedback_delay_ms: u64,
    mix: FrameMix,
    transient_outage_at_handoff: bool,
}

#[derive(Debug, Default)]
struct WiredMetrics {
    handoff_at_frame: u32,
    post_frames: usize,
    lost_packets: usize,
    recovered_frames: usize,
    unrecoverable_frames: usize,
    steady_unrecoverable: usize,
    rate_increases: usize,
    rate_decreases: usize,
    probe_losses: usize,
    packet_losses: usize,
    delivery_total_ms: f64,
    delivery_max_ms: f64,
    complete_frames: usize,
    final_bps: u64,
    max_bps: u64,
    learned_ceiling_bps: Option<u64>,
}

fn simulate_wired_handoff(sc: Scenario, cand: WiredCandidate) -> WiredMetrics {
    let period = Duration::from_secs_f64(1. / 60.);
    let packetizer = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1392,
        fec_percent: sc.fec_percent,
        min_fec: if sc.fec_percent == 0 { 0 } else { 2 },
        key: Some([0; 16]),
    };
    let needed = network_pacing::video_wire_rate_bps(&packetizer, sc.bitrate_kbps, period, false);
    let safe = network_pacing::rate_bps(0, sc.bitrate_kbps, 1_000_000_000, true, needed);
    let datagram = packetizer.packet_size + 16 + 32;
    let wire_packet = datagram + network_pacing::overhead(false);
    let host_time = Duration::from_secs_f64(wire_packet as f64 * 8. / 1_000_000_000.);
    let egress_time = Duration::from_secs_f64(wire_packet as f64 * 8. / sc.egress_bps as f64);
    let capacity = Duration::from_secs_f64(sc.queue_bytes as f64 * 8. / sc.egress_bps as f64);
    let feedback_delay = Duration::from_millis(sc.feedback_delay_ms);

    let start = Instant::now();
    let mut now = start;
    let mut drained_at = start;
    let mut pacer = network_pacing::Pacer::new(start);
    let mut loss_guard = KeyframeLoss::new(true);
    loss_guard.set_reports_losses(true);

    let mut pacing: Option<AdaptivePacing> = None;
    let mut fixed_safe_active = false;
    let mut traffic = Traffic::default();
    let mut pending: VecDeque<(Instant, u32, Block, bool)> = VecDeque::new();
    let mut metrics = WiredMetrics::default();
    let total_frames = 7200 + 180; // pre-handoff + 120 s (7200 frames) post-handoff

    for frame in 1..=total_frames {
        let available = start + period * (frame - 1);
        now = now.max(available);
        while pending.front().is_some_and(|(due, _, _, _)| *due <= now) {
            let (_, rep_frame, block, is_kf) = pending.pop_front().unwrap();
            if pacing.is_none() && !fixed_safe_active {
                if matches!(block, Block::Unrecoverable)
                    && let Some(latched) =
                        loss_guard.unrecoverable(rep_frame, frame - 1, period, now)
                    && latched.kind == LatchKind::Sustained
                    && !latched.host_only
                {
                    let handoff_frame = frame - 1;
                    metrics.handoff_at_frame = handoff_frame;
                    if let Some(cfg) = cand.config() {
                        let mut ap = AdaptivePacing::new_after_loss_with(
                            Some(handoff_frame),
                            Some(FAST),
                            cfg,
                        );
                        ap.set_fec_percent(sc.fec_percent);
                        ap.rate(FAST, needed, sc.bitrate_kbps);
                        ap.tick(
                            now,
                            traffic,
                            Some(handoff_frame),
                            period,
                            sc.bitrate_kbps,
                            feedback_delay,
                        );
                        pacing = Some(ap);
                    } else {
                        fixed_safe_active = true;
                    }
                }
            } else if let Some(ap) = pacing.as_mut() {
                ap.on_block_kind(rep_frame, frame - 1, period, block, is_kf);
            }
        }

        let in_post = pacing.is_some() || fixed_safe_active;
        let post_idx = if in_post {
            metrics.post_frames += 1;
            metrics.post_frames
        } else {
            0
        };
        let keyframe = frame == 1 || frame == 61 || (frame > 61 && (frame - 61) % 300 == 0);
        loss_guard.sending_at(frame, keyframe, now);
        let rate = if let Some(ap) = pacing.as_mut() {
            ap.rate(FAST, needed, sc.bitrate_kbps)
        } else if fixed_safe_active {
            safe
        } else {
            FAST
        };
        if in_post {
            metrics.max_bps = metrics.max_bps.max(rate);
        }

        let base_payload =
            (f64::from(sc.bitrate_kbps) * 125. * period.as_secs_f64()).ceil() as usize;
        let payload = match sc.mix {
            FrameMix::DurKMixed => {
                if keyframe {
                    let kf_idx = if frame <= 61 { 0 } else { (frame - 61) / 300 };
                    if kf_idx % 2 == 0 {
                        base_payload * 4
                    } else {
                        (base_payload * 5) / 2
                    }
                } else if frame > 61 && frame % 45 == 0 {
                    (base_payload * 33) / 10
                } else {
                    base_payload
                }
            }
            FrameMix::PeriodicKeyframes => {
                if keyframe {
                    base_payload * 4
                } else {
                    base_payload
                }
            }
        };

        let per_batch = (network_pacing::batch_payload_limit(rate, datagram, false, 64 * 1024)
            .min(65507)
            / datagram)
            .clamp(1, 64);
        let force_transient = sc.transient_outage_at_handoff && frame == 61;
        let mut failed_frame = false;
        let mut frame_lost_data = false;

        for (data, fec) in packetizer.block_layout(payload) {
            let mut lost_data = 0;
            let mut parity_received = 0;
            let mut packet = 0;
            while packet < data + fec {
                now = now.max(pacer.due());
                let count = (data + fec - packet).min(per_batch);
                for index in packet..packet + count {
                    now += host_time;
                    let done = drained_at.max(now) + egress_time;
                    if !force_transient && done.saturating_duration_since(now) <= capacity {
                        drained_at = done;
                        parity_received += usize::from(index >= data);
                    } else {
                        if in_post {
                            metrics.lost_packets += 1;
                        }
                        lost_data += usize::from(index < data);
                    }
                }
                traffic.packets += count as u64;
                pacer.sent(now, count * datagram, count, false, rate);
                packet += count;
            }
            let failed = lost_data > parity_received;
            failed_frame |= failed;
            frame_lost_data |= lost_data > 0;
            if lost_data > 0 {
                pending.push_back((
                    now + feedback_delay,
                    frame,
                    if failed {
                        Block::Unrecoverable
                    } else {
                        Block::Recovered {
                            lost: lost_data as u16,
                            data: data as u16,
                        }
                    },
                    keyframe,
                ));
            }
        }

        if in_post {
            if failed_frame {
                metrics.unrecoverable_frames += 1;
                if post_idx > 600 {
                    metrics.steady_unrecoverable += 1;
                }
            } else {
                if frame_lost_data {
                    metrics.recovered_frames += 1;
                }
                let delivery_ms = (drained_at - available).as_secs_f64() * 1000.0;
                metrics.complete_frames += 1;
                metrics.delivery_total_ms += delivery_ms;
                metrics.delivery_max_ms = metrics.delivery_max_ms.max(delivery_ms);
            }
        }

        traffic.frames += 1;
        traffic.bytes += payload as u64;

        if let Some(ap) = pacing.as_mut() {
            while pending.front().is_some_and(|(due, _, _, _)| *due <= now) {
                let (_, rep_frame, block, is_kf) = pending.pop_front().unwrap();
                ap.on_block_kind(rep_frame, frame, period, block, is_kf);
            }
            ap.on_ping(now, &PING);
            if let Some(step) = ap.tick_step(
                now,
                traffic,
                Some(frame),
                period,
                sc.bitrate_kbps,
                feedback_delay,
            ) {
                if step.new_bps > step.old_bps {
                    metrics.rate_increases += 1;
                } else if step.new_bps < step.old_bps {
                    metrics.rate_decreases += 1;
                }
                match step.reason {
                    PacingReason::ProbeLoss => metrics.probe_losses += 1,
                    PacingReason::PacketLoss => metrics.packet_losses += 1,
                    _ => {}
                }
            }
            metrics.final_bps = ap.rate(FAST, needed, sc.bitrate_kbps);
            metrics.learned_ceiling_bps = ap.post_loss.and_then(|p| p.learned_ceiling());
        } else if fixed_safe_active {
            metrics.final_bps = safe;
        }
        if post_idx == 7200 {
            break;
        }
    }
    metrics
}

#[test]
fn wired_handoff_matrix_comparison() {
    let scenarios = [
        Scenario {
            name: "A_DurK_300M_q16K_fec20",
            egress_bps: 300_000_000,
            bitrate_kbps: 31_000,
            fec_percent: 20,
            queue_bytes: 16 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::DurKMixed,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "A_DurK_300M_q64K_fec20",
            egress_bps: 300_000_000,
            bitrate_kbps: 31_000,
            fec_percent: 20,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::DurKMixed,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q16K_fec20",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 20,
            queue_bytes: 16 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q16K_fec10",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 10,
            queue_bytes: 16 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q16K_fec05",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 5,
            queue_bytes: 16 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q16K_fec00",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 0,
            queue_bytes: 16 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q64K_fec20",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 20,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q64K_fec10",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 10,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q64K_fec05",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 5,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "B_Onn_100M_q64K_fec00",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 0,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "C_1GbE_40M_q64K_fec20",
            egress_bps: 1_000_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 20,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: true,
        },
        Scenario {
            name: "C_1GbE_100M_q64K_fec20",
            egress_bps: 1_000_000_000,
            bitrate_kbps: 100_000,
            fec_percent: 20,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 2,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: true,
        },
        Scenario {
            name: "D_DurK_300M_rtt20ms",
            egress_bps: 300_000_000,
            bitrate_kbps: 31_000,
            fec_percent: 20,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 20,
            mix: FrameMix::DurKMixed,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "D_DurK_300M_rtt80ms",
            egress_bps: 300_000_000,
            bitrate_kbps: 31_000,
            fec_percent: 20,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 80,
            mix: FrameMix::DurKMixed,
            transient_outage_at_handoff: false,
        },
        Scenario {
            name: "D_Onn_100M_rtt80ms",
            egress_bps: 100_000_000,
            bitrate_kbps: 40_000,
            fec_percent: 20,
            queue_bytes: 64 * 1024,
            feedback_delay_ms: 80,
            mix: FrameMix::PeriodicKeyframes,
            transient_outage_at_handoff: false,
        },
    ];
    let candidates = [
        WiredCandidate::FixedSafe,
        WiredCandidate::Bare40,
        WiredCandidate::Cand2Half,
        WiredCandidate::Cand2HalfLockPkt,
        WiredCandidate::Cand2ThreeQuarters,
        WiredCandidate::Cand2PktHalf,
        WiredCandidate::Cand3LowFec,
        WiredCandidate::Cand3PktHalfLockLowFec,
        WiredCandidate::Cand3PktHalf,
        WiredCandidate::Cand3ThreeQtrPktHalf,
    ];
    println!(
        "scenario | candidate | post | lost_pkts | recov_f | unrec_f | steady_unrec | probe_loss | pkt_loss | inc/dec | mean_ms | max_ms | final_M | cap_M"
    );
    for sc in scenarios {
        for cand in candidates {
            let m = simulate_wired_handoff(sc, cand);
            assert_eq!(m.post_frames, 7200, "{} {}", sc.name, cand.label());
            let mean_ms = if m.complete_frames > 0 {
                m.delivery_total_ms / m.complete_frames as f64
            } else {
                0.0
            };
            println!(
                "{} | {} | {} | {} | {} | {} | {} | {} | {} | {}/{} | {:.2} | {:.2} | {:.1} | {}",
                sc.name,
                cand.label(),
                m.post_frames,
                m.lost_packets,
                m.recovered_frames,
                m.unrecoverable_frames,
                m.steady_unrecoverable,
                m.probe_losses,
                m.packet_losses,
                m.rate_increases,
                m.rate_decreases,
                mean_ms,
                m.delivery_max_ms,
                m.final_bps as f64 / 1e6,
                m.learned_ceiling_bps
                    .map_or("-".to_string(), |v| format!("{:.1}", v as f64 / 1e6))
            );
            if matches!(cand, WiredCandidate::Cand3PktHalf) {
                if sc.name.starts_with("A_DurK") || sc.name.starts_with("D_DurK") {
                    assert_eq!(m.unrecoverable_frames, 0, "{}", sc.name);
                    assert_eq!(m.probe_losses, 1, "{}", sc.name);
                    assert!(m.learned_ceiling_bps.is_some(), "{}", sc.name);
                    assert!(mean_ms < 5.0, "{} mean_ms={mean_ms}", sc.name);
                    assert!(m.final_bps > 150_000_000, "{}", sc.name);
                } else if sc.name.starts_with("B_Onn") && sc.fec_percent < 10 {
                    assert_eq!(m.lost_packets, 0, "{}", sc.name);
                    assert_eq!(m.unrecoverable_frames, 0, "{}", sc.name);
                    assert_eq!(m.final_bps, 80_000_000, "{}", sc.name);
                } else if sc.name.starts_with("B_Onn") && sc.queue_bytes == 16 * 1024 {
                    assert_eq!(m.unrecoverable_frames, 0, "{}", sc.name);
                    assert_eq!(m.probe_losses, 1, "{}", sc.name);
                } else if sc.name.starts_with("C_1GbE") {
                    assert_eq!(m.lost_packets, 0, "{}", sc.name);
                    assert_eq!(m.unrecoverable_frames, 0, "{}", sc.name);
                    assert_eq!(m.final_bps, 400_000_000, "{}", sc.name);
                } else if sc.name == "D_Onn_100M_rtt80ms" {
                    assert_eq!(m.steady_unrecoverable, 0, "{}", sc.name);
                    assert_eq!(m.probe_losses, 1, "{}", sc.name);
                }
            }
        }
    }
}
