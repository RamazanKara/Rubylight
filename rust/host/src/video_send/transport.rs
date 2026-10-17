//! Socket pacing consumes packets whose Moonlight timing was already written.
use super::*;
use butterpollo_core::{network_pacing, performance::Timing, stream_policy::SendLossRecovery};
use butterpollo_windows::{net::Batch, timing::Timer};

impl Slot {
    fn transmit(&self, mut send: impl FnMut(&Prepared) -> Result<()>) -> Result<()> {
        loop {
            let frame = {
                let mut state = self.state.lock().unwrap();
                while state.ready.is_none() && !self.stop.load(Ordering::Acquire) {
                    state = self.changed.wait(state).unwrap();
                }
                if self.stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                let frame = state.ready.take().unwrap();
                state.transmitting = true;
                state.transmitting_keyframe = frame.idr;
                state.transmitting_remaining = None;
                self.changed.notify_all();
                frame
            };
            send(&frame)?;
            let mut state = self.state.lock().unwrap();
            state.transmitting = false;
            state.transmitting_keyframe = false;
            state.transmitting_remaining = None;
            state.recycle(frame.blocks);
            self.changed.notify_all();
        }
    }

    pub(super) fn recovery_progress(&self, remaining: Option<Duration>) {
        let mut state = self.state.lock().unwrap();
        if state.transmitting_keyframe {
            state.transmitting_remaining = remaining;
            self.changed.notify_all();
        }
    }
}

/// Include the next batch's due time and all remaining bytes. Counting even
/// the final batch's wire interval deliberately opens admission a little late.
fn recovery_remaining(now: Instant, due: Instant, wire_bytes: usize, bps: u64) -> Duration {
    due.saturating_duration_since(now)
        + Duration::from_secs_f64(wire_bytes as f64 * 8. / bps.max(1) as f64)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    shared: &Slot,
    socket: Arc<UdpSocket>,
    s: Arc<Session>,
    c: Config,
    h: Shared,
    start: Instant,
    pacing_due: Instant,
    mut send_loss: SendLossRecovery,
) -> Result<()> {
    let _priority = butterpollo_windows::capture::Priority::new();
    let timer = Timer::new()?;
    let mut batch = Batch::default();
    batch.waits = Some(Default::default());
    let mut pacer = network_pacing::Pacer::new(pacing_due);
    let mut link = None;
    let mut link_due = Instant::now();
    let mut reported_pacing = None;
    let mut slow_reported: Option<Instant> = None;
    let outage = butterpollo_core::stream_policy::SendOutage::from_env();
    let batch_kb = match c.integer("video_max_batch_size_kb", 64) {
        16 => 16,
        32 => 32,
        _ => 64,
    };
    shared.transmit(|frame| {
        let dequeued = Instant::now();
        let period = s.stream_mode().period();
        let peer = frame.peer;
        let route = *link.get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
        let bitrate = s.bitrate.load(Ordering::Relaxed);
        let needed = network_pacing::video_wire_rate_bps(&frame.packetizer, bitrate, period, peer.is_ipv6());
        let base = network_pacing::rate_bps(c.integer("pacing_max_bitrate_kbps", 0), bitrate, route.bps, route.wireless, needed);
        let bps = s.video_pacing_bps(base, needed);
        let safe = network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
        let bps = s.keyframe_loss_bps(bps, safe, frame.wire_frame, frame.idr);
        if Instant::now() >= link_due {
            network_pacing::report_rate(&s.launch.warnings, bps, needed, c.integer("pacing_max_bitrate_kbps", 0));
            if reported_pacing != Some(bps) {
                tracing::info!(pacing_bps=bps, wire_budget_bps=needed, link_bps=route.bps, configured_kbps=c.integer("pacing_max_bitrate_kbps", 0), "network pacing selected");
                reported_pacing = Some(bps);
            }
        }
        let packet_count: usize = frame.counts.iter().sum();
        let frame_bytes: usize = frame.blocks.iter().zip(&frame.counts)
            .map(|(block, count)| block[..*count].iter().map(Vec::len).sum::<usize>()).sum();
        let wire_bytes = frame_bytes + packet_count * network_pacing::overhead(peer.is_ipv6());
        let mut remaining_wire_bytes = wire_bytes;
        let mut recovery_overlap_safe = true;
        let dropped = batch.dropped;
        let before = batch.waits.unwrap();
        let mut pacing_wait = Duration::ZERO;
        let mut send_time = Duration::ZERO;
        let mut batches = 0;
        let mut first_send = None;
        let mut last_send = None;
        let mut first_sent = None;
        let mut lost = butterpollo_core::stream_policy::SendLoss::new(frame.layout.clone());
        let mut abandoned = 0;
        let mut sent_packets = 0;
        s.stats.video_frame.store(u64::from(frame.wire_frame) + 1, Ordering::Release);
        'blocks: for (block, count) in frame.blocks.iter().zip(&frame.counts) {
            let mut remaining = &block[..*count];
            while !remaining.is_empty() {
                if shared.stop.load(Ordering::Acquire) || s.stopping() || h.stop.load(Ordering::Acquire) {
                    shared.stop();
                    return Ok(());
                }
                let now = Instant::now();
                if pacer.due() > now {
                    timer.until_precise(pacer.due());
                    pacing_wait += now.elapsed();
                }
                let budget = network_pacing::batch_payload_limit(bps, remaining[0].len(), peer.is_ipv6(), batch_kb * 1024);
                let count = Batch::count(remaining, budget);
                let batch_wire_bytes = if frame.idr {
                    remaining[..count].iter().map(Vec::len).sum::<usize>()
                        + count * network_pacing::overhead(peer.is_ipv6())
                } else { 0 };
                let send_started = Instant::now();
                let refused = batch.dropped;
                let bytes = match outage.filter(|outage| outage.active(start, Instant::now())) {
                    Some(outage) if outage.in_air() => remaining[..count].iter().map(Vec::len).sum(),
                    Some(_) => { batch.refuse(count); 0 },
                    None => batch.send(&socket, &remaining[..count], peer)?,
                };
                let finished = Instant::now();
                first_send.get_or_insert(send_started);
                last_send = Some(finished);
                first_sent.get_or_insert(finished);
                send_time += finished.duration_since(send_started);
                batches += 1;
                remaining = &remaining[count..];
                sent_packets += count;
                pacer.sent(finished, bytes, if bytes > 0 { count } else { 0 }, peer.ip().to_canonical().is_ipv6(), bps);
                if frame.idr {
                    remaining_wire_bytes = remaining_wire_bytes.saturating_sub(batch_wire_bytes);
                    // A stalled socket is not predictable from the configured
                    // pace. Keep the strict barrier for the rest of that IDR.
                    recovery_overlap_safe &= batch.dropped == dropped
                        && batch.waits.unwrap().count == before.count
                        && finished.saturating_duration_since(send_started) <= network_pacing::SEND_SLACK;
                    shared.recovery_progress(recovery_overlap_safe.then(|| recovery_remaining(
                        finished, pacer.due(), remaining_wire_bytes, bps,
                    )));
                }
                s.stats.packets.fetch_add(count as u64, Ordering::Relaxed);
                s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                if lost.sent(count, (batch.dropped - refused) as usize) {
                    abandoned = packet_count - sent_packets;
                    if send_loss.lost(Instant::now(), period, lost.reached_socket()) { s.request_send_loss_recovery(); }
                    break 'blocks;
                }
            }
        }
        if lost.unrecoverable() {
            tracing::debug!(frame=frame.wire_frame, idr=frame.idr, abandoned, "video frame lost on send; the rest of it was not sent");
        } else { send_loss.delivered(frame.idr); }
        if batch.dropped != dropped {
            s.launch.warnings.event("network_send", "Video packets were dropped by the host after transient socket send failures. You may see stutter or recovery frames; lower bitrate and check the network adapter. The log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
        }
        s.stats.latency_us.store(frame.latency, Ordering::Relaxed);
        s.stats.frames.fetch_add(1, Ordering::Relaxed);
        let sent = Instant::now();
        let queue_wait = dequeued.saturating_duration_since(frame.prepared);
        let elapsed = sent.saturating_duration_since(dequeued);
        let after = batch.waits.unwrap();
        tracing::trace!(target: "pacing", frame=frame.wire_frame, packets=packet_count, batches,
            payload_bytes=frame.payload_bytes, wire_bytes, pacing_bps=bps,
            packetizer_queue_us=micros(frame.packetized.saturating_duration_since(frame.polled)),
            queue_wait_us=micros(queue_wait), pacing_wait_us=micros(pacing_wait), send_us=micros(send_time),
            first_send_us=micros(first_send.unwrap_or(dequeued).saturating_duration_since(start)),
            last_send_us=micros(last_send.unwrap_or(sent).saturating_duration_since(start)),
            writable_waits=after.count-before.count, writable_wait_us=micros(after.elapsed-before.elapsed),
            dropped=batch.dropped-dropped, stream_id=%s.launch.id, "send");
        if (elapsed >= period * 2 || queue_wait >= period)
            && slow_reported.is_none_or(|at| sent.saturating_duration_since(at) >= Duration::from_secs(5))
        {
            slow_reported = Some(sent);
            tracing::info!(frame=frame.wire_frame, payload_bytes=frame.payload_bytes, wire_bytes, pacing_bps=bps,
                ideal_wire_ms=wire_bytes as f64 * 8000. / bps.max(1) as f64,
                queue_ms=queue_wait.as_secs_f64()*1000., elapsed_ms=elapsed.as_secs_f64()*1000.,
                pacing_wait_ms=pacing_wait.as_secs_f64()*1000., socket_ms=send_time.as_secs_f64()*1000.,
                writable_wait_ms=(after.elapsed-before.elapsed).as_secs_f64()*1000.,
                dropped=batch.dropped-dropped, "paced video frame delayed");
        }
        let mut performance = s.stats.performance.lock().unwrap();
        performance.record_timing(sent, Timing { period, encode:frame.latency, host:frame.processing, age:frame.age,
            sent:micros(sent.saturating_duration_since(frame.claimed)) }, frame_bytes as u64);
        let stamps = frame.claim.filter(|_| !lost.unrecoverable()).map(|claim| {
            let mut stamps = claim.sent(frame.polled, frame.packetized, first_sent.unwrap_or(sent), sent);
            stamps.prepared = frame.prepared;
            stamps
        });
        if let Some(stamps) = &stamps { performance.record_stages(stamps, frame.processing); }
        drop(performance);
        if let Some(stamps) = &stamps { s.host_timings.lock().unwrap().record(frame.wire_frame, stamps); }
        if Instant::now() >= link_due {
            link = Some(butterpollo_windows::net::routed_link(peer));
            link_due = Instant::now() + Duration::from_secs(2);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_estimate_includes_existing_pacing_debt_and_remaining_wire_bytes() {
        let now = Instant::now();
        assert_eq!(
            recovery_remaining(now, now + Duration::from_millis(1), 50_000, 80_000_000),
            Duration::from_millis(6)
        );
        assert_eq!(
            recovery_remaining(now, now - Duration::from_millis(20), 50_000, 80_000_000),
            Duration::from_millis(5)
        );
        assert_eq!(
            recovery_remaining(now, now + Duration::from_millis(1), 0, 80_000_000),
            Duration::from_millis(1)
        );
    }
}
