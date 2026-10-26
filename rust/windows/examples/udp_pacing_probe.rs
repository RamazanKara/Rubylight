//! Send deterministic, frame-sized UDP bursts to an independent receiver.
//! No capture, codec, display changes or installed host are involved.
//! Automatic mode injects synthetic healthy control feedback. This measures
//! sender timing, not network capacity or the behavior of a physical client.
//! Usage: udp_pacing_probe PEER legacy|paced|automatic|automatic-no-feedback WIRE_MBPS [STALL_MS]
#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use butterpollo_core::{
        adaptive_pacing::{AdaptivePacing, Traffic},
        network_pacing::Pacer,
    };
    use butterpollo_windows::{
        net::{Batch, ReplySocket},
        timing::Timer,
    };
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };
    let mut args = std::env::args().skip(1);
    let peer: std::net::SocketAddr = args.next().context("receiver address required")?.parse()?;
    let mode = args
        .next()
        .context("legacy, paced, automatic or automatic-no-feedback required")?;
    anyhow::ensure!(
        matches!(
            mode.as_str(),
            "legacy" | "paced" | "automatic" | "automatic-no-feedback"
        ),
        "invalid mode"
    );
    let bps: u64 = args.next().context("wire Mbps required")?.parse::<u64>()? * 1_000_000;
    anyhow::ensure!(
        (1_000_000..=1_000_000_000).contains(&bps),
        "wire rate outside 1..1000 Mbps"
    );
    let stall = Duration::from_millis(args.next().map_or(Ok(0), |v| v.parse())?);
    anyhow::ensure!(
        stall <= Duration::from_millis(20) && args.next().is_none(),
        "invalid arguments"
    );
    // Keep this isolated probe from changing desktop, Wi-Fi or GPU policy.
    // Both revisions use ordinary process scheduling for the comparison.
    let socket = ReplySocket::bind(if peer.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })?;
    socket.set_nonblocking(true)?;
    butterpollo_windows::net::configure_udp(&socket)?;
    let timer = Timer::new()?;
    let mut batch = Batch::default();
    let start = Instant::now();
    let mut legacy_due = start;
    let mut pacer = Pacer::new(start);
    let mut packets_sent = 0u64;
    let mut bytes_sent = 0u64;
    let mut spans = Vec::new();
    let is_automatic = matches!(mode.as_str(), "automatic" | "automatic-no-feedback");
    let automatic = Mutex::new(AdaptivePacing::new(is_automatic));
    let mut rate_transitions = Vec::new();
    let mut first_ceiling = None;
    let mut initial_bps = None;
    let mut final_bps = bps;
    let ceiling = if is_automatic {
        butterpollo_core::network_pacing::rate_bps(0, 40_000, 0, false, 55_000_000).min(bps)
    } else {
        bps
    };
    for frame in 0..1200u32 {
        timer.until_precise(start + Duration::from_secs_f64(f64::from(frame) / 60.));
        // 50 Mbps at 60 fps, with a large keyframe every five seconds.
        let bytes: usize = 50_000_000 / 8 / 60 * if frame % 300 == 0 { 8 } else { 1 };
        let count = bytes.div_ceil(1400);
        let packets: Vec<_> = (0..count)
            .map(|i| {
                let mut packet = vec![0xa5u8; 1400];
                packet[..4].copy_from_slice(b"BPP1");
                packet[4..8].copy_from_slice(&frame.to_be_bytes());
                packet[8..10].copy_from_slice(&(i as u16).to_be_bytes());
                packet[10..12].copy_from_slice(&(count as u16).to_be_bytes());
                packet
            })
            .collect();
        let begin = Instant::now();
        let rate = if is_automatic {
            // Exercise the same short per-frame controller lock as Session.
            // The automatic case has SYNTHETIC standard pings and a stable
            // 2 ms RTT. The no-feedback case must retain its cautious start.
            // Neither case measures downstream capacity or a real client.
            let mut controller = automatic.lock().unwrap();
            controller.rate(ceiling, 55_000_000, 40_000);
            let rtt = if mode == "automatic" {
                controller.on_ping(begin, &[4, 0, 0, 0, 0, 0, 0, 0]);
                Duration::from_millis(2)
            } else {
                Duration::ZERO
            };
            controller.tick(
                begin,
                Traffic {
                    frames: u64::from(frame),
                    bytes: bytes_sent,
                    packets: packets_sent,
                    send_losses: 0,
                },
                frame.checked_sub(1),
                Duration::from_secs_f64(1. / 60.),
                40_000,
                rtt,
            );
            controller.rate(ceiling, 55_000_000, 40_000)
        } else {
            bps
        };
        initial_bps.get_or_insert(rate);
        let elapsed = begin.saturating_duration_since(start);
        if frame == 0 || rate != final_bps {
            rate_transitions.push(serde_json::json!({
                "frame": frame, "seconds": elapsed.as_secs_f64(), "bps": rate,
            }));
        }
        if rate == ceiling && first_ceiling.is_none() {
            first_ceiling = Some(serde_json::json!({
                "frame": frame, "seconds": elapsed.as_secs_f64(),
            }));
        }
        final_bps = rate;
        legacy_due = legacy_due.max(begin);
        let mut remaining = packets.as_slice();
        let mut batches = 0;
        while !remaining.is_empty() {
            let due = if mode == "legacy" {
                legacy_due
            } else {
                pacer.due()
            };
            timer.until_precise(due);
            // Reproduce a late scheduling/socket completion, with no network
            // or driver changes. Only keyframes get this controlled stall.
            if frame % 300 == 0 && batches == 2 {
                std::thread::sleep(stall);
            }
            let budget = if mode == "legacy" {
                (rate / 4000).clamp(1400, 64 * 1024) as usize
            } else {
                butterpollo_core::network_pacing::batch_payload_limit(
                    rate,
                    1400,
                    peer.is_ipv6(),
                    64 * 1024,
                )
            };
            let count = Batch::count(remaining, budget);
            let bytes = batch.send(&socket, &remaining[..count], peer)?;
            let completed = Instant::now();
            legacy_due += Duration::from_secs_f64(bytes as f64 * 8. / rate as f64);
            pacer.sent(
                completed,
                bytes,
                if bytes > 0 { count } else { 0 },
                peer.is_ipv6(),
                rate,
            );
            packets_sent += count as u64;
            bytes_sent += bytes as u64;
            remaining = &remaining[count..];
            batches += 1;
        }
        spans.push((elapsed, begin.elapsed().as_secs_f64() * 1000.));
    }
    timer.until_precise(Instant::now() + Duration::from_millis(20));
    let end = format!("END {packets_sent}");
    socket.send_to(end.as_bytes(), peer)?;
    let stats = |mut values: Vec<f64>| {
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        Some(serde_json::json!({
            "frames": values.len(),
            "mean_ms": values.iter().sum::<f64>() / values.len() as f64,
            "p99_ms": values[values.len() * 99 / 100],
            "max_ms": values[values.len() - 1],
        }))
    };
    let first_second = stats(
        spans
            .iter()
            .filter(|(at, _)| *at < Duration::from_secs(1))
            .map(|(_, span)| *span)
            .collect(),
    );
    let first_two_seconds = stats(
        spans
            .iter()
            .filter(|(at, _)| *at < Duration::from_secs(2))
            .map(|(_, span)| *span)
            .collect(),
    );
    let after_fifteen_seconds = stats(
        spans
            .iter()
            .filter(|(at, _)| *at >= Duration::from_secs(15))
            .map(|(_, span)| *span)
            .collect(),
    );
    let first_frame_send_ms = spans[0].1;
    let mut all_spans: Vec<_> = spans.into_iter().map(|(_, span)| span).collect();
    all_spans.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "mode": mode,
            "feedback": if mode == "automatic" { "synthetic_standard_ping_and_stable_2ms_rtt" } else { "none" },
            "scope": "isolated_sender_timing_not_network_capacity",
            "bps": bps,
            "ceiling_bps": ceiling,
            "initial_bps": initial_bps,
            "final_bps": final_bps,
            "first_ceiling": first_ceiling,
            "rate_transitions": rate_transitions,
            "controller": automatic.lock().unwrap().snapshot(),
            "stall_ms": stall.as_millis(),
            "frames": 1200,
            "packets": packets_sent,
            "bytes": bytes_sent,
            "dropped": batch.dropped,
            "system_calls": batch.system_calls,
            "seconds": start.elapsed().as_secs_f64(),
            "first_frame_send_ms": first_frame_send_ms,
            "startup_first_second": first_second,
            "startup_first_two_seconds": first_two_seconds,
            "steady_after_fifteen_seconds": after_fifteen_seconds,
            "frame_send_mean_ms": all_spans.iter().sum::<f64>() / all_spans.len() as f64,
            "frame_send_p99_ms": all_spans[all_spans.len() * 99 / 100],
            "frame_send_max_ms": all_spans[all_spans.len() - 1],
        })
    );
    Ok(())
}
