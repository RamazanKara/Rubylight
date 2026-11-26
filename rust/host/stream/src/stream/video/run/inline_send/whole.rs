//! Inline conventional whole-frame packetization, paced batch send, and slow-send logging.

use super::super::super::*;
use super::{InlineSender, SendContext};

/// A frame that holds the stream thread this long is logged with where the time went, at
/// most every five seconds.
const SLOW_SEND: Duration = Duration::from_millis(100);

impl InlineSender {
    pub(super) fn send_whole_frame(
        &mut self,
        ctx: &SendContext<'_>,
        frame: butterpollo_win_graphics::encoder::Encoded,
        peer: SocketAddr,
        call_latency: Duration,
        processing_started: Instant,
        polled: Instant,
    ) -> Result<()> {
        let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
        let frame_started = Instant::now();
        let encode = frame.latency.unwrap_or(call_latency);
        let latency = micros(encode);
        ctx.s.stats.latency_us.store(latency, Ordering::Relaxed);
        let claimed = polled.checked_sub(encode).unwrap_or(polled);
        let captured = frame.presentation.unwrap_or(claimed);
        let age = micros(claimed.saturating_duration_since(captured));
        let claim = ctx.claims.borrow_mut().take(frame.presentation);
        let stamp_started = Instant::now();
        let stamp = self
            .present_stamper
            .as_mut()
            .map_or(captured, |stamper| {
                stamper.stamp(captured, &ctx.prepared.output())
            })
            .max(self.last_stamp + Duration::from_nanos(11_112));
        let stamp_time = stamp_started.elapsed();
        self.last_stamp = stamp;
        // Wrap like the previous host; a saturating cast
        // froze the clock after 13.25 hours.
        let timestamp =
            (stamp.saturating_duration_since(self.start).as_secs_f64() * 90000.) as u64 as u32;
        if frame.bytes.is_empty() {
            return Ok(());
        }
        self.packetizer.fec_percent = ctx
            .s
            .video_fec_percent(frame.idr || frame.after_invalidation, frame.bytes.len());
        let wire_frame = self.packetizer.frame;
        // Frame Warp tags the frame by the number the client decodes it as.
        ctx.s
            .frame_warp
            .tag(wire_frame, claim.as_ref().map(|claim| claim.presented));
        let plan = self.packetizer.conventional_plan(frame.bytes.len());
        let fec_limited = plan.limited(self.packetizer.fec_percent);
        let packet_bytes =
            self.packetizer.packet_size + 16 + if self.packetizer.key.is_some() { 32 } else { 0 };
        // A client that roamed takes another route.
        if crate::video_send::recovery::new_route(&mut self.routed, peer) {
            self.link = None;
        }
        let first_link = self.link.is_none();
        let link_started = Instant::now();
        let route = self.link_watch.get(peer);
        let link_time = link_started.elapsed();
        self.link = Some(route);
        if first_link {
            crate::video_send::recovery::warn_if_client_link_below_stream(ctx.s, route);
        }
        let bitrate = ctx.s.bitrate.load(Ordering::Relaxed);
        let crate::video_send::recovery::Pace { bps, needed } = crate::video_send::recovery::pace(
            ctx.s,
            ctx.c,
            &self.packetizer,
            ctx.stream_period,
            peer,
            route,
        );
        let safe =
            butterpollo_media_policy::network_pacing::rate_bps(0, bitrate, route.bps, true, needed);
        let bps = ctx.s.keyframe_loss_bps(bps, safe, wire_frame, frame.idr);
        self.inline_paced |= butterpollo_media_policy::network_pacing::paced(bps, needed);
        if Instant::now() >= self.pacing_report_due {
            self.pacing_report_due = Instant::now() + Duration::from_secs(2);
            butterpollo_media_policy::network_pacing::report_rate(
                &ctx.s.launch.warnings,
                bps,
                needed,
                ctx.c.manual_pacing_kbps().unwrap_or(0),
            );
            if self.reported_pacing != Some(bps) {
                tracing::info!(
                    pacing_bps = bps,
                    wire_budget_bps = needed,
                    link_bps = route.bps,
                    client_link_mbps = ctx.s.client_link_mbps(),
                    client_wireless = ctx.s.config.client_wireless,
                    configured_kbps = ctx.c.manual_pacing_kbps().unwrap_or(0),
                    "network pacing selected"
                );
                self.reported_pacing = Some(bps);
            }
        }
        // A frame beyond Moonlight's packet limit (very high
        // bitrates) costs that frame and a keyframe, not the
        // session.
        let packetize_started = Instant::now();
        let processing = micros(packetize_started.saturating_duration_since(processing_started));
        let prepared_blocks = self.packetizer.conventional_blocks_planned(
            &frame.bytes,
            frame.idr,
            frame.after_invalidation,
            timestamp,
            processing,
            &plan,
        );
        if let Err(error) = &prepared_blocks {
            ctx.s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_session::session::EVENT_PERIOD);
            ctx.s.request_host_keyframe();
            drop(prepared_blocks);
            self.next_wire_frame.set(u64::from(self.packetizer.frame));
            return Ok(());
        }
        let (packet_count, mut next_block) = prepared_blocks?;
        let mut packetize_time = packetize_started.elapsed();
        tracing::trace!(
            target: "pacing",
            stream_id = %ctx.s.launch.id,
            frame = wire_frame,
            presentation = ?frame.presentation,
            "packetized picture"
        );
        // A keyframe, as at the start of every stream, may be too
        // large for FEC; only ordinary frames make this worth showing.
        if !frame.idr
            && fec_limited
            && self
                .fec_reported
                .is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
        {
            self.fec_reported = Some(Instant::now());
            ctx.s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_session::session::EVENT_PERIOD);
        }
        let dropped = self.batch.dropped;
        let oversized = self.batch.oversized;
        let waits_before = self.batch.waits;
        let mut pacing_wait = Duration::ZERO;
        let mut send_time = Duration::ZERO;
        let mut batches = 0;
        let mut first_send = None;
        let mut last_send = None;
        let mut first_sent = None;
        let mut lost = butterpollo_media_policy::stream_policy::SendLoss::new(plan.layout);
        let mut abandoned = 0;
        let mut sent_packets = 0;
        let mut sent_bytes = 0;
        // The frame's batches share one wait for a full socket.
        self.batch.begin_frame_with_enobufs(
            butterpollo_media_policy::network_pacing::frame_send_wait(ctx.stream_period),
            butterpollo_media_policy::network_pacing::enobufs_frame_wait(wire_frame),
        );
        'blocks: loop {
            let block_started = Instant::now();
            let block_count = match next_block(&mut self.packets) {
                Ok(count) => count,
                Err(error) => {
                    ctx.s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_session::session::EVENT_PERIOD);
                    ctx.s.request_host_keyframe();
                    drop(next_block);
                    self.next_wire_frame.set(u64::from(self.packetizer.frame));
                    return Ok(());
                }
            };
            packetize_time += block_started.elapsed();
            if block_count == 0 {
                break;
            }
            self.next_wire_frame
                .set(u64::from(wire_frame.wrapping_add(1)));
            ctx.s
                .stats
                .video_frame
                .store(u64::from(wire_frame) + 1, Ordering::Release);
            let mut remaining = &self.packets[..block_count];
            while !remaining.is_empty() {
                if ctx.s.stopping() || ctx.h.stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                let now = Instant::now();
                if self.network_pacer.due() > now {
                    ctx.timer.until_paced(self.network_pacer.due());
                    pacing_wait += now.elapsed();
                }
                let budget = butterpollo_media_policy::pacing_high_hz::startup_batch_payload_limit(
                    wire_frame,
                    bps,
                    remaining[0].len(),
                    peer.ip().to_canonical().is_ipv6(),
                    self.batch_kb * 1024,
                    ctx.stream_period,
                );
                let count = butterpollo_win_io::net::Batch::count(remaining, budget);
                let send_started = Instant::now();
                let refused = self.batch.dropped;
                let (bytes, sent_count) = match self
                    .send_outage
                    .filter(|outage| outage.active(self.start, Instant::now()))
                {
                    // Lost after the socket: counted as sent, only the client sees it.
                    Some(outage) if outage.in_air() => {
                        (remaining[..count].iter().map(Vec::len).sum(), count)
                    }
                    Some(_) => {
                        self.batch.refuse(count);
                        (0, 0)
                    }
                    None => match self.send_drop.as_mut() {
                        Some(drop) => {
                            let bytes = slices::send_lossy(
                                &mut self.batch,
                                &ctx.m.video,
                                &remaining[..count],
                                peer,
                                drop,
                            )?;
                            let sent =
                                count.saturating_sub((self.batch.dropped - refused) as usize);
                            (bytes, sent)
                        }
                        None => {
                            let bytes = self.batch.send(&ctx.m.video, &remaining[..count], peer)?;
                            (bytes, self.batch.sent_packets().min(count))
                        }
                    },
                };
                let ipv6 = peer.ip().to_canonical().is_ipv6();
                let finished = Instant::now();
                super::record_inline_sent(
                    ctx.s,
                    &mut self.network_pacer,
                    &remaining[..count],
                    (bytes, sent_count),
                    ipv6,
                    bps,
                    send_started,
                    finished,
                );
                first_sent.get_or_insert(finished);
                send_time += finished.duration_since(send_started);
                if self.trace_send {
                    first_send.get_or_insert(send_started);
                    last_send = Some(finished);
                    batches += 1;
                }
                remaining = &remaining[count..];
                sent_packets += count;
                sent_bytes += bytes;
                ctx.s
                    .stats
                    .packets
                    .fetch_add(count as u64, Ordering::Relaxed);
                ctx.s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                // The socket refused more of this frame than FEC
                // repairs: the client cannot decode it. Stop
                // spending the link on it, and make the next
                // frame the keyframe the client will wait for.
                if lost.sent(count, (self.batch.dropped - refused) as usize) {
                    abandoned = packet_count - sent_packets;
                    if self
                        .send_loss
                        .lost(Instant::now(), ctx.stream_period, lost.reached_socket())
                    {
                        ctx.s.request_send_loss_recovery();
                    }
                    break 'blocks;
                }
            }
        }
        drop(next_block);
        self.batch.end_frame();
        let (link_packets, link_bytes) = crate::video_send::reached_socket(
            sent_packets,
            self.batch.dropped - dropped,
            sent_bytes,
            peer.ip().to_canonical().is_ipv6(),
        );
        ctx.s.record_link_sent(wire_frame, link_packets, link_bytes);
        let refused = self.batch.dropped.saturating_sub(self.batch.oversized)
            != dropped.saturating_sub(oversized);
        if lost.unrecoverable() {
            ctx.s.keyframe_abandoned_as(wire_frame, refused);
            tracing::debug!(
                frame = self.packetizer.frame.wrapping_sub(1),
                idr = frame.idr,
                abandoned,
                "video frame lost on send; the rest of it was not sent"
            );
        } else {
            self.send_loss.delivered(frame.idr);
        }
        if refused {
            ctx.s.launch.warnings.event("network_send", "The PC could not send the video fast enough (its CPU or network adapter fell behind), so video packets were dropped. You may see stutter or recovery frames; close other work on the PC, or lower the bitrate, resolution or frame rate. The log includes the socket error code.", butterpollo_session::session::EVENT_PERIOD);
        }
        let (frame_bytes, wire_bytes) = super::attempted_wire_bytes(
            sent_packets,
            packet_bytes,
            peer.ip().to_canonical().is_ipv6(),
        );
        crate::video_send::report_oversized(ctx.s, &self.batch, oversized);
        ctx.s.stats.frames.fetch_add(1, Ordering::Relaxed);
        let sent = Instant::now();
        crate::video_send::recovery::report(
            ctx.s,
            ctx.c,
            crate::video_send::recovery::RecoveryFrame {
                frame: wire_frame,
                idr: frame.idr,
                after_invalidation: frame.after_invalidation,
                payload_bytes: frame.bytes.len(),
                cap_bytes: frame.frame_cap_bytes,
                cap_target_bytes: frame.frame_cap_target,
                reencoded: frame.reencoded,
                fec_limited,
                wire_bytes,
                pace_bps: bps,
                elapsed: sent.saturating_duration_since(frame_started),
                lost: lost.unrecoverable(),
            },
        );
        if let (Some(first), Some(last), Some(before), Some(after)) =
            (first_send, last_send, waits_before, self.batch.waits)
        {
            tracing::trace!(
                target: "pacing",
                frame = self.packetizer.frame.wrapping_sub(1),
                packets = sent_packets,
                batches,
                pacing_wait_us = micros(pacing_wait),
                send_us = micros(send_time),
                first_send_us = micros(first.saturating_duration_since(self.start)),
                last_send_us = micros(last.saturating_duration_since(self.start)),
                writable_waits = after.count - before.count,
                writable_wait_us = micros(after.elapsed - before.elapsed),
                dropped = self.batch.dropped - dropped,
                stream_id = %ctx.s.launch.id,
                "send"
            );
        }
        {
            let mut performance = ctx.s.stats.performance.lock().unwrap();
            performance.record_timing(
                sent,
                butterpollo_media_policy::performance::Timing {
                    period: ctx.stream_period,
                    encode: latency,
                    host: processing,
                    age,
                    sent: micros(sent.saturating_duration_since(claimed)),
                },
                frame_bytes,
            );
            // A frame abandoned on send never had its last packet sent.
            if let Some(claim) = claim.filter(|_| !lost.unrecoverable()) {
                let stamps =
                    claim.sent(polled, packetize_started, first_sent.unwrap_or(sent), sent);
                performance.record_stages(&stamps, processing);
                ctx.s
                    .record_sent(wire_frame, timestamp, &stamps, self.start);
            }
        }
        let record_time = sent.elapsed();
        let took = frame_started.elapsed();
        if took >= SLOW_SEND
            && self
                .slow_send_reported
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            self.slow_send_reported = Some(Instant::now());
            let ms = |d: Duration| (d.as_secs_f64() * 10_000.).round() / 10.;
            tracing::warn!(
                frame_ms = ms(took),
                socket_ms = ms(send_time),
                pacing_ms = ms(pacing_wait),
                timestamp_ms = ms(stamp_time),
                packetize_ms = ms(packetize_time),
                timings_ms = ms(record_time),
                link_ms = ms(link_time),
                packets = sent_packets,
                bytes = frame_bytes,
                pacing_bps = bps,
                idr = frame.idr,
                "a video frame held the stream thread while it was sent; socket_ms is time in Windows' send calls (network driver and adapter), the other parts are the host's own work"
            );
        }
        Ok(())
    }
}
