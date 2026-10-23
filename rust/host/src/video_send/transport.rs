//! Socket pacing consumes packets whose Moonlight timing was already written.
use super::*;
use butterpollo_core::{
    network_pacing,
    performance::Timing,
    stream_policy::{SendDrop, SendLoss, SendLossRecovery, SendOutage},
};
use butterpollo_windows::{
    net::{Batch, Link, WritableWaits},
    timing::Timer,
};

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

/// What the paced sender keeps of the frame it is sending. A streamed frame
/// arrives as one part per FEC block and this lives from its first part to
/// its last; a whole frame is a single part.
struct Sending {
    dequeued: Instant,
    /// When the first part was ready, which the send queue is counted from.
    prepared: Instant,
    bps: u64,
    packets: usize,
    frame_bytes: usize,
    wire_bytes: usize,
    remaining_wire_bytes: usize,
    recovery_overlap_safe: bool,
    dropped: u64,
    before: WritableWaits,
    pacing_wait: Duration,
    send_time: Duration,
    batches: usize,
    first_send: Option<Instant>,
    last_send: Option<Instant>,
    first_sent: Option<Instant>,
    lost: SendLoss,
    /// The socket refused more than FEC repairs: the rest is not sent.
    given_up: bool,
    abandoned: usize,
    sent_packets: usize,
}

struct Transport<'a> {
    shared: &'a Slot,
    socket: Arc<UdpSocket>,
    s: Arc<Session>,
    c: Config,
    h: Shared,
    start: Instant,
    timer: Timer,
    batch: Batch,
    pacer: network_pacing::Pacer,
    link: Option<Link>,
    link_due: Instant,
    reported_pacing: Option<u64>,
    slow_reported: Option<Instant>,
    outage: Option<SendOutage>,
    send_drop: Option<SendDrop>,
    send_loss: SendLossRecovery,
    batch_kb: usize,
}

impl Transport<'_> {
    /// The frame a part belongs to begins: its pace and counters.
    fn begin(&mut self, frame: &Prepared) -> Sending {
        let dequeued = Instant::now();
        let (s, c) = (&self.s, &self.c);
        let period = s.stream_mode().period();
        let peer = frame.peer;
        let route = *self
            .link
            .get_or_insert_with(|| butterpollo_windows::net::routed_link(peer));
        let bitrate = s.bitrate.load(Ordering::Relaxed);
        let recovery::Pace { bps, needed } =
            recovery::pace(s, c, &frame.packetizer, period, peer, route);
        let safe = network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
        let bps = s.keyframe_loss_bps(bps, safe, frame.wire_frame, frame.idr);
        if Instant::now() >= self.link_due {
            network_pacing::report_rate(
                &s.launch.warnings,
                bps,
                needed,
                c.integer("pacing_max_bitrate_kbps", 0),
            );
            if self.reported_pacing != Some(bps) {
                tracing::info!(
                    pacing_bps = bps,
                    wire_budget_bps = needed,
                    link_bps = route.bps,
                    configured_kbps = c.integer("pacing_max_bitrate_kbps", 0),
                    "network pacing selected"
                );
                self.reported_pacing = Some(bps);
            }
        }
        s.stats
            .video_frame
            .store(u64::from(frame.wire_frame) + 1, Ordering::Release);
        let waits = self.batch.waits.unwrap();
        Sending {
            dequeued,
            prepared: frame.prepared,
            bps,
            packets: 0,
            frame_bytes: 0,
            wire_bytes: 0,
            remaining_wire_bytes: 0,
            recovery_overlap_safe: true,
            dropped: self.batch.dropped,
            before: waits,
            pacing_wait: Duration::ZERO,
            send_time: Duration::ZERO,
            batches: 0,
            first_send: None,
            last_send: None,
            first_sent: None,
            lost: if frame.stream.is_some() {
                SendLoss::streamed()
            } else {
                SendLoss::new(frame.layout.clone())
            },
            given_up: false,
            abandoned: 0,
            sent_packets: 0,
        }
    }

    /// Send the blocks of one part. Returns whether the stream is stopping.
    fn send_blocks(&mut self, frame: &Prepared, cur: &mut Sending) -> Result<bool> {
        let s = &self.s;
        let period = s.stream_mode().period();
        let peer = frame.peer;
        let bps = cur.bps;
        let part_packets: usize = frame.counts.iter().sum();
        let part_bytes: usize = frame
            .blocks
            .iter()
            .zip(&frame.counts)
            .map(|(block, count)| block[..*count].iter().map(Vec::len).sum::<usize>())
            .sum();
        let part_wire = part_bytes + part_packets * network_pacing::overhead(peer.is_ipv6());
        cur.packets += part_packets;
        cur.frame_bytes += part_bytes;
        cur.wire_bytes += part_wire;
        cur.remaining_wire_bytes += part_wire;
        if frame.stream.is_some() {
            for &(data, parity) in &frame.layout {
                cur.lost.push_block(data, parity);
            }
        }
        'blocks: for (block, count) in frame.blocks.iter().zip(&frame.counts) {
            let mut remaining = &block[..*count];
            while !remaining.is_empty() && !cur.given_up {
                if self.shared.stop.load(Ordering::Acquire)
                    || s.stopping()
                    || self.h.stop.load(Ordering::Acquire)
                {
                    self.shared.stop();
                    return Ok(true);
                }
                let now = Instant::now();
                if self.pacer.due() > now {
                    self.timer.until_paced(self.pacer.due());
                    cur.pacing_wait += now.elapsed();
                }
                let budget = network_pacing::batch_payload_limit(
                    bps,
                    remaining[0].len(),
                    peer.is_ipv6(),
                    self.batch_kb * 1024,
                );
                let count = Batch::count(remaining, budget);
                let batch_wire_bytes = if frame.idr {
                    remaining[..count].iter().map(Vec::len).sum::<usize>()
                        + count * network_pacing::overhead(peer.is_ipv6())
                } else {
                    0
                };
                let send_started = Instant::now();
                let refused = self.batch.dropped;
                let bytes = match self
                    .outage
                    .filter(|outage| outage.active(self.start, Instant::now()))
                {
                    Some(outage) if outage.in_air() => {
                        remaining[..count].iter().map(Vec::len).sum()
                    }
                    Some(_) => {
                        self.batch.refuse(count);
                        0
                    }
                    None => match self.send_drop.as_mut() {
                        Some(drop) => crate::stream::slices::send_lossy(
                            &mut self.batch,
                            &self.socket,
                            &remaining[..count],
                            peer,
                            drop,
                        )?,
                        None => self.batch.send(&self.socket, &remaining[..count], peer)?,
                    },
                };
                let finished = Instant::now();
                cur.first_send.get_or_insert(send_started);
                cur.last_send = Some(finished);
                cur.first_sent.get_or_insert(finished);
                cur.send_time += finished.duration_since(send_started);
                cur.batches += 1;
                remaining = &remaining[count..];
                cur.sent_packets += count;
                self.pacer.sent(
                    finished,
                    bytes,
                    if bytes > 0 { count } else { 0 },
                    peer.ip().to_canonical().is_ipv6(),
                    bps,
                );
                if frame.idr {
                    cur.remaining_wire_bytes =
                        cur.remaining_wire_bytes.saturating_sub(batch_wire_bytes);
                    // A stalled socket is not predictable from the configured
                    // pace. Keep the strict barrier for the rest of that IDR.
                    cur.recovery_overlap_safe &= self.batch.dropped == cur.dropped
                        && self.batch.waits.unwrap().count == cur.before.count
                        && finished.saturating_duration_since(send_started)
                            <= network_pacing::SEND_SLACK;
                    self.shared
                        .recovery_progress(cur.recovery_overlap_safe.then(|| {
                            recovery_remaining(
                                finished,
                                self.pacer.due(),
                                cur.remaining_wire_bytes,
                                bps,
                            )
                        }));
                }
                s.stats.packets.fetch_add(count as u64, Ordering::Relaxed);
                s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                if cur
                    .lost
                    .sent(count, (self.batch.dropped - refused) as usize)
                {
                    if self
                        .send_loss
                        .lost(Instant::now(), period, cur.lost.reached_socket())
                    {
                        s.request_send_loss_recovery();
                    }
                    cur.given_up = true;
                    break 'blocks;
                }
            }
        }
        if cur.given_up {
            cur.abandoned = cur.packets - cur.sent_packets;
        }
        Ok(false)
    }

    /// The frame's last part is out: account for it.
    fn finish(&mut self, frame: &Prepared, cur: Sending) {
        let (s, c) = (&self.s, &self.c);
        let period = s.stream_mode().period();
        let peer = frame.peer;
        let start = self.start;
        let Sending {
            dequeued,
            prepared,
            bps,
            packets: packet_count,
            frame_bytes,
            wire_bytes,
            dropped,
            before,
            pacing_wait,
            send_time,
            batches,
            first_send,
            last_send,
            first_sent,
            lost,
            abandoned,
            ..
        } = cur;
        if lost.unrecoverable() {
            s.keyframe_abandoned(frame.wire_frame);
            tracing::debug!(
                frame = frame.wire_frame,
                idr = frame.idr,
                abandoned,
                "video frame lost on send; the rest of it was not sent"
            );
        } else {
            self.send_loss.delivered(frame.idr);
        }
        if self.batch.dropped != dropped {
            s.launch.warnings.event("network_send", "Video packets were dropped by the host after transient socket send failures. You may see stutter or recovery frames; lower bitrate and check the network adapter. The log includes the socket error code.", butterpollo_core::session::EVENT_PERIOD);
        }
        s.stats.latency_us.store(frame.latency, Ordering::Relaxed);
        s.stats.frames.fetch_add(1, Ordering::Relaxed);
        let sent = Instant::now();
        let queue_wait = dequeued.saturating_duration_since(prepared);
        let elapsed = sent.saturating_duration_since(dequeued);
        recovery::report(
            s,
            c,
            recovery::RecoveryFrame {
                frame: frame.wire_frame,
                idr: frame.idr,
                after_invalidation: frame.after_invalidation,
                payload_bytes: frame.payload_bytes,
                cap_bytes: frame.frame_cap_bytes,
                wire_bytes: wire_bytes as u64,
                pace_bps: bps,
                elapsed,
                lost: lost.unrecoverable(),
            },
        );
        let after = self.batch.waits.unwrap();
        tracing::trace!(target: "pacing", frame=frame.wire_frame, packets=packet_count, batches,
            payload_bytes=frame.payload_bytes, wire_bytes, pacing_bps=bps,
            packetizer_queue_us=micros(frame.packetized.saturating_duration_since(frame.polled)),
            queue_wait_us=micros(queue_wait), pacing_wait_us=micros(pacing_wait), send_us=micros(send_time),
            first_send_us=micros(first_send.unwrap_or(dequeued).saturating_duration_since(start)),
            last_send_us=micros(last_send.unwrap_or(sent).saturating_duration_since(start)),
            writable_waits=after.count-before.count, writable_wait_us=micros(after.elapsed-before.elapsed),
            dropped=self.batch.dropped-dropped, stream_id=%s.launch.id, "send");
        if (elapsed >= period * 2 || queue_wait >= period)
            && self
                .slow_reported
                .is_none_or(|at| sent.saturating_duration_since(at) >= Duration::from_secs(5))
        {
            self.slow_reported = Some(sent);
            tracing::info!(
                frame = frame.wire_frame,
                payload_bytes = frame.payload_bytes,
                wire_bytes,
                pacing_bps = bps,
                ideal_wire_ms = wire_bytes as f64 * 8000. / bps.max(1) as f64,
                queue_ms = queue_wait.as_secs_f64() * 1000.,
                elapsed_ms = elapsed.as_secs_f64() * 1000.,
                pacing_wait_ms = pacing_wait.as_secs_f64() * 1000.,
                socket_ms = send_time.as_secs_f64() * 1000.,
                writable_wait_ms = (after.elapsed - before.elapsed).as_secs_f64() * 1000.,
                dropped = self.batch.dropped - dropped,
                "paced video frame delayed"
            );
        }
        let mut performance = s.stats.performance.lock().unwrap();
        performance.record_timing(
            sent,
            Timing {
                period,
                encode: frame.latency,
                host: frame.processing,
                age: frame.age,
                sent: micros(sent.saturating_duration_since(frame.claimed)),
            },
            frame_bytes as u64,
        );
        let stamps = frame.claim.filter(|_| !lost.unrecoverable()).map(|claim| {
            // A streamed frame is encoded when its last unit is in hand, which can be
            // after its first block was sent; its first unit and early blocks go with it.
            let mut stamps = claim.sent(
                frame.last_polled,
                frame.packetized,
                first_sent.unwrap_or(sent),
                sent,
            );
            stamps.prepared = prepared;
            if frame.stream.is_some() {
                stamps = stamps.with_streamed(frame.polled, frame.early_blocks);
            }
            stamps
        });
        if let Some(stamps) = &stamps {
            performance.record_stages(stamps, frame.processing);
        }
        drop(performance);
        if let Some(stamps) = &stamps {
            s.record_sent(frame.wire_frame, frame.timestamp, stamps, start);
        }
        if Instant::now() >= self.link_due {
            self.link = Some(butterpollo_windows::net::routed_link(peer));
            self.link_due = Instant::now() + Duration::from_secs(2);
        }
    }

    /// One part off the queue: a whole frame, or one block of a streamed one.
    fn part(&mut self, frame: &Prepared, sending: &mut Option<Sending>) -> Result<()> {
        // A streamed frame the host dropped part way leaves its sender state
        // behind; the next frame's first part starts over.
        if frame.stream.is_none_or(|part| part.first) {
            *sending = Some(self.begin(frame));
        }
        let Some(cur) = sending.as_mut() else {
            // The start of this frame was never seen.
            return Ok(());
        };
        if self.send_blocks(frame, cur)? {
            return Ok(());
        }
        if frame.stream.is_some_and(|part| !part.last) {
            return Ok(());
        }
        if let Some(cur) = sending.take() {
            self.finish(frame, cur);
        }
        Ok(())
    }
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
    send_loss: SendLossRecovery,
) -> Result<()> {
    run_with(
        shared,
        socket,
        s,
        c,
        h,
        start,
        pacing_due,
        send_loss,
        SendDrop::from_env(),
    )
}

/// [`run`] with the test loss given instead of read from the environment.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_with(
    shared: &Slot,
    socket: Arc<UdpSocket>,
    s: Arc<Session>,
    c: Config,
    h: Shared,
    start: Instant,
    pacing_due: Instant,
    send_loss: SendLossRecovery,
    send_drop: Option<SendDrop>,
) -> Result<()> {
    let _priority = butterpollo_windows::capture::Priority::new();
    let mut batch = Batch::default();
    batch.waits = Some(Default::default());
    let batch_kb = match c.integer("video_max_batch_size_kb", 64) {
        16 => 16,
        32 => 32,
        _ => 64,
    };
    let mut transport = Transport {
        shared,
        socket,
        s,
        c,
        h,
        start,
        timer: Timer::new()?,
        batch,
        pacer: network_pacing::Pacer::new(pacing_due),
        link: None,
        link_due: Instant::now(),
        reported_pacing: None,
        slow_reported: None,
        outage: SendOutage::from_env(),
        send_drop,
        send_loss,
        batch_kb,
    };
    let mut sending = None;
    shared.transmit(|frame| transport.part(frame, &mut sending))
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
