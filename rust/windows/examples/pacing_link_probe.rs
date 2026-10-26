//! Send frame-sized UDP bursts through the stream's packet pacer to the LAN
//! receiver (`pacing_receiver` on the test NUC), to compare pacing rates on a
//! real switch port. The receiver records loss, socket overflow and each
//! frame's arrival span from kernel timestamps.
//! Usage: pacing_link_probe PEER PACING_MBPS FRAME_MBPS FPS SECONDS
//! FRAME_MBPS is the stream's wire rate (bitrate plus FEC); every fifth second
//! one frame is a keyframe eight times the size.
#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use butterpollo_core::network_pacing::{Pacer, batch_payload_limit};
    use butterpollo_windows::{
        net::{Batch, ReplySocket},
        timing::Timer,
    };
    use std::time::{Duration, Instant};
    const PACKET: usize = 1440;
    let mut args = std::env::args().skip(1);
    let mut next = |name: &str| args.next().with_context(|| format!("{name} required"));
    let peer: std::net::SocketAddr = next("receiver address")?.parse()?;
    let rate = next("pacing Mbps")?.parse::<u64>()? * 1_000_000;
    let frame_bps = next("frame Mbps")?.parse::<u64>()? * 1_000_000;
    let fps: u32 = next("fps")?.parse()?;
    let seconds: u32 = next("seconds")?.parse()?;
    anyhow::ensure!(
        (1_000_000..=2_500_000_000).contains(&rate) && fps > 0 && seconds > 0,
        "invalid arguments"
    );
    let socket = ReplySocket::bind("0.0.0.0:0")?;
    socket.set_nonblocking(true)?;
    butterpollo_windows::net::configure_udp(&socket)?;
    let timer = Timer::new()?;
    let mut batch = Batch::default();
    let start = Instant::now();
    let mut pacer = Pacer::new(start);
    let (mut packets_sent, mut spans) = (0u64, Vec::new());
    let frames = fps * seconds;
    for frame in 0..frames {
        timer.until_precise(start + Duration::from_secs_f64(f64::from(frame) / f64::from(fps)));
        let keyframe = frame % (fps * 5) == 0;
        let bytes = (frame_bps / 8 / u64::from(fps)) as usize * if keyframe { 8 } else { 1 };
        let count = bytes.div_ceil(PACKET);
        let packets: Vec<_> = (0..count)
            .map(|i| {
                let mut packet = vec![0xa5u8; PACKET];
                packet[..4].copy_from_slice(b"BPP1");
                packet[4..8].copy_from_slice(&frame.to_be_bytes());
                packet[8..10].copy_from_slice(&(i as u16).to_be_bytes());
                packet[10..12].copy_from_slice(&(count as u16).to_be_bytes());
                packet
            })
            .collect();
        let begin = Instant::now();
        let mut remaining = packets.as_slice();
        while !remaining.is_empty() {
            timer.until_precise(pacer.due());
            let budget = batch_payload_limit(rate, PACKET, false, 64 * 1024);
            let count = Batch::count(remaining, budget);
            let bytes = batch.send(&socket, &remaining[..count], peer)?;
            pacer.sent(
                Instant::now(),
                bytes,
                if bytes > 0 { count } else { 0 },
                false,
                rate,
            );
            packets_sent += count as u64;
            remaining = &remaining[count..];
        }
        spans.push((keyframe, begin.elapsed().as_secs_f64() * 1000.));
    }
    timer.until_precise(Instant::now() + Duration::from_millis(20));
    socket.send_to(format!("END {packets_sent}").as_bytes(), peer)?;
    let mean = |key: bool| {
        let values: Vec<f64> = spans
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, s)| *s)
            .collect();
        values.iter().sum::<f64>() / values.len().max(1) as f64
    };
    println!(
        "{}",
        serde_json::json!({
            "pacing_bps": rate, "frame_bps": frame_bps, "fps": fps, "frames": frames,
            "packets": packets_sent, "dropped": batch.dropped,
            "send_span_mean_ms": mean(false), "keyframe_send_span_mean_ms": mean(true),
        })
    );
    Ok(())
}
