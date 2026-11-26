//! Inline block-by-block send path for sliced P-frames (`video_slice_delivery`).

use super::super::super::*;
use super::{InlineSender, SendContext};

impl InlineSender {
    pub(super) fn send_sliced_unit(
        &mut self,
        ctx: &SendContext<'_>,
        mut frame: butterpollo_win_graphics::encoder::Encoded,
        peer: SocketAddr,
        call_latency: Duration,
        processing_started: Instant,
        polled: Instant,
    ) -> Result<()> {
        let micros = |d: Duration| d.as_micros().min(u128::from(u64::MAX)) as u64;
        // One unit of a P-frame sent block by block.
        let last = matches!(
            frame.unit,
            butterpollo_win_graphics::encoder::Unit::Last { .. }
        );
        if matches!(
            frame.unit,
            butterpollo_win_graphics::encoder::Unit::Slice { index: 0 }
                | butterpollo_win_graphics::encoder::Unit::Last { index: 0 }
        ) {
            let started = Instant::now();
            let encode = frame.latency.unwrap_or(call_latency);
            let claimed = polled.checked_sub(encode).unwrap_or(polled);
            let captured = frame.presentation.unwrap_or(claimed);
            let age = micros(claimed.saturating_duration_since(captured));
            let claim = ctx.claims.borrow_mut().take(frame.presentation);
            let stamp = self
                .present_stamper
                .as_mut()
                .map_or(captured, |stamper| {
                    stamper.stamp(captured, &ctx.prepared.output())
                })
                .max(self.last_stamp + Duration::from_nanos(11_112));
            self.last_stamp = stamp;
            let timestamp =
                (stamp.saturating_duration_since(self.start).as_secs_f64() * 90000.) as u64 as u32;
            self.packetizer.fec_percent = ctx.s.frame_fec_percent(false);
            let wire_frame = self.packetizer.frame;
            ctx.s
                .frame_warp
                .tag(wire_frame, claim.as_ref().map(|claim| claim.presented));
            let route = self.link_watch.get(peer);
            self.link = Some(route);
            let bitrate = ctx.s.bitrate.load(Ordering::Relaxed);
            let crate::video_send::recovery::Pace { bps, needed } =
                crate::video_send::recovery::pace(
                    ctx.s,
                    ctx.c,
                    &self.packetizer,
                    ctx.stream_period,
                    peer,
                    route,
                );
            let safe = butterpollo_media_policy::network_pacing::rate_bps(
                0, bitrate, route.bps, true, needed,
            );
            let bps = ctx.s.keyframe_loss_bps(bps, safe, wire_frame, false);
            self.inline_paced |= butterpollo_media_policy::network_pacing::paced(bps, needed);
            let packetize_started = Instant::now();
            let processing =
                micros(packetize_started.saturating_duration_since(processing_started));
            let cap = frame.frame_cap_bytes.unwrap_or(0) as usize;
            let blocks = self.delivery.blocks();
            // Ask before the buffer moves: `streamed_in` drops it,
            // capacity and all, when it refuses the frame.
            let streamed = if self.packetizer.streamable(blocks, cap) {
                self.packetizer.streamed_in(
                    std::mem::take(&mut self.slice_spare),
                    timestamp,
                    processing,
                    blocks,
                    blocks.max(ctx.s.config.slices as usize),
                    cap,
                )
            } else {
                Err(anyhow::anyhow!(
                    "video frame cannot be streamed in {blocks} blocks"
                ))
            };
            self.streaming = match streamed {
                Ok(streamed) => Some(slices::Streaming {
                    frame: streamed,
                    wire_frame,
                    timestamp,
                    claim,
                    claimed,
                    age,
                    started,
                    packetize_started,
                    first_polled: polled,
                    last_polled: polled,
                    payload_bytes: 0,
                    processing,
                    bps,
                    sent: slices::Sent::new(self.batch.dropped, self.batch.oversized),
                    failed: false,
                }),
                Err(error) => {
                    self.delivery.turn_off();
                    ctx.s.launch.warnings.event(
                        "network_frame",
                        format!(
                            "Encoded video frame dropped ({error:#}); requesting a recovery frame."
                        ),
                        butterpollo_session::session::EVENT_PERIOD,
                    );
                    ctx.s.request_host_keyframe();
                    self.next_wire_frame
                        .set(u64::from(slices::refuse_frame(&mut self.packetizer)));
                    None
                }
            };
        }
        let Some(st) = self.streaming.as_mut() else {
            return Ok(());
        };
        st.unit(polled, frame.bytes.len());
        if !st.failed && !st.sent.abandoned {
            let mut stopped = false;
            let mut fatal = None;
            let slices::Streaming {
                frame: streamed,
                sent,
                bps,
                wire_frame,
                ..
            } = st;
            let (bps, wire_frame) = (*bps, *wire_frame);
            let pushed = streamed.push(
                &mut self.packetizer,
                std::mem::take(&mut frame.bytes),
                last,
                &mut self.packets,
                |shards, block| {
                    sent.lost.push_block(block.data, block.parity);
                    if block.index == 0 {
                        // The frame's batches share one wait for a full socket.
                        self.batch.begin_frame_with_enobufs(
                            butterpollo_media_policy::network_pacing::frame_send_wait(
                                ctx.stream_period,
                            ),
                            butterpollo_media_policy::network_pacing::enobufs_frame_wait(
                                wire_frame,
                            ),
                        );
                        self.next_wire_frame
                            .set(u64::from(wire_frame.wrapping_add(1)));
                        ctx.s
                            .stats
                            .video_frame
                            .store(u64::from(wire_frame) + 1, Ordering::Release);
                    }
                    let mut remaining = &shards[..];
                    while !remaining.is_empty() {
                        if ctx.s.stopping() || ctx.h.stop.load(Ordering::Acquire) {
                            stopped = true;
                            return Ok(());
                        }
                        let now = Instant::now();
                        if self.network_pacer.due() > now {
                            ctx.timer.until_paced(self.network_pacer.due());
                        }
                        let budget =
                            butterpollo_media_policy::pacing_high_hz::startup_batch_payload_limit(
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
                        let result = match self
                            .send_outage
                            .filter(|outage| outage.active(self.start, Instant::now()))
                        {
                            Some(outage) if outage.in_air() => {
                                Ok((remaining[..count].iter().map(Vec::len).sum(), count))
                            }
                            Some(_) => {
                                self.batch.refuse(count);
                                Ok((0, 0))
                            }
                            None => match self.send_drop.as_mut() {
                                Some(drop) => slices::send_lossy(
                                    &mut self.batch,
                                    &ctx.m.video,
                                    &remaining[..count],
                                    peer,
                                    drop,
                                )
                                .map(|bytes| {
                                    let sent = count
                                        .saturating_sub((self.batch.dropped - refused) as usize);
                                    (bytes, sent)
                                }),
                                None => self
                                    .batch
                                    .send(&ctx.m.video, &remaining[..count], peer)
                                    .map(|bytes| (bytes, self.batch.sent_packets().min(count))),
                            },
                        };
                        let (bytes, sent_count) = match result {
                            Ok(pair) => pair,
                            Err(error) => {
                                fatal = Some(error);
                                return Ok(());
                            }
                        };
                        let ipv6 = peer.ip().to_canonical().is_ipv6();
                        let now = Instant::now();
                        super::record_inline_sent(
                            ctx.s,
                            &mut self.network_pacer,
                            &remaining[..count],
                            (bytes, sent_count),
                            ipv6,
                            bps,
                            send_started,
                            now,
                        );
                        sent.first_sent.get_or_insert(now);
                        remaining = &remaining[count..];
                        sent.packets += count;
                        sent.bytes += bytes;
                        ctx.s
                            .stats
                            .packets
                            .fetch_add(count as u64, Ordering::Relaxed);
                        ctx.s.stats.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
                        // As for a whole frame: once the socket refused more
                        // than FEC repairs, stop spending the link on it.
                        if sent
                            .lost
                            .sent(count, (self.batch.dropped - refused) as usize)
                        {
                            if self.send_loss.lost(
                                Instant::now(),
                                ctx.stream_period,
                                sent.lost.reached_socket(),
                            ) {
                                ctx.s.request_send_loss_recovery();
                            }
                            sent.abandoned = true;
                            break;
                        }
                    }
                    Ok(())
                },
            );
            if stopped {
                return Ok(());
            }
            if let Some(error) = fatal {
                return Err(error);
            }
            if let Err(error) = pushed {
                st.failed = true;
                if let Some(overflow) = slices::note_failure(&self.delivery, &ctx.s.stats, &error) {
                    slices::report_overflow(ctx.s, st.wire_frame, overflow);
                } else {
                    ctx.s.launch.warnings.event("network_frame", format!("Encoded video frame dropped ({error:#}); requesting a recovery frame. Lower bitrate or resolution to stay within Moonlight's packet limit."), butterpollo_session::session::EVENT_PERIOD);
                }
                ctx.s.request_host_keyframe();
                self.next_wire_frame.set(u64::from(self.packetizer.frame));
            }
        }
        if !last {
            return Ok(());
        }
        let Some(st) = self.streaming.take() else {
            return Ok(());
        };
        self.batch.end_frame();
        if st.failed {
            if st.sent.packets > 0 {
                ctx.s.keyframe_abandoned_as(st.wire_frame, false);
            }
            self.slice_spare = recycle_streamed_pieces(st.frame);
            return Ok(());
        }
        // The last unit is out: account for the frame as for a whole one.
        let slices::Streaming {
            frame: streamed,
            wire_frame,
            timestamp,
            claim,
            claimed,
            age,
            started,
            packetize_started,
            first_polled,
            last_polled,
            payload_bytes,
            processing,
            bps,
            sent,
            ..
        } = st;
        let unrecoverable = sent.lost.unrecoverable();
        let (link_packets, link_bytes) = crate::video_send::reached_socket(
            sent.packets,
            self.batch.dropped - sent.dropped_at_start,
            sent.bytes,
            peer.ip().to_canonical().is_ipv6(),
        );
        ctx.s.record_link_sent(wire_frame, link_packets, link_bytes);
        let refused = self.batch.dropped.saturating_sub(self.batch.oversized)
            != sent
                .dropped_at_start
                .saturating_sub(sent.oversized_at_start);
        if unrecoverable {
            ctx.s.keyframe_abandoned_as(wire_frame, refused);
            tracing::debug!(
                frame = wire_frame,
                finished = streamed.is_done(),
                "video frame lost on send; the rest of it was not sent"
            );
        } else {
            self.send_loss.delivered(false);
        }
        if refused {
            ctx.s.launch.warnings.event("network_send", "The PC could not send the video fast enough (its CPU or network adapter fell behind), so video packets were dropped. You may see stutter or recovery frames; close other work on the PC, or lower the bitrate, resolution or frame rate. The log includes the socket error code.", butterpollo_session::session::EVENT_PERIOD);
        }
        crate::video_send::report_oversized(ctx.s, &self.batch, sent.oversized_at_start);
        // As the whole-frame path judges it, from the frame's size, so an
        // abandoned frame reports as well as a finished one.
        if (streamed.fec_limited() || self.packetizer.fec_limited(payload_bytes))
            && self
                .fec_reported
                .is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
        {
            self.fec_reported = Some(Instant::now());
            ctx.s.launch.warnings.event("network_fec", "FEC was reduced or omitted for large video frames because they exceed Moonlight's four-block limit. Packet loss in those frames is harder to recover; lower bitrate or resolution to keep full FEC protection.", butterpollo_session::session::EVENT_PERIOD);
        }
        let latency = micros(frame.latency.unwrap_or(call_latency));
        ctx.s.stats.latency_us.store(latency, Ordering::Relaxed);
        ctx.s.stats.frames.fetch_add(1, Ordering::Relaxed);
        let packet_bytes =
            self.packetizer.packet_size + 16 + if self.packetizer.key.is_some() { 32 } else { 0 };
        let (frame_bytes, wire_bytes) = super::attempted_wire_bytes(
            sent.packets,
            packet_bytes,
            peer.ip().to_canonical().is_ipv6(),
        );
        let finished = Instant::now();
        crate::video_send::recovery::report(
            ctx.s,
            ctx.c,
            crate::video_send::recovery::RecoveryFrame {
                frame: wire_frame,
                idr: false,
                after_invalidation: false,
                payload_bytes,
                cap_bytes: frame.frame_cap_bytes,
                cap_target_bytes: frame.frame_cap_target,
                reencoded: frame.reencoded,
                fec_limited: streamed.fec_limited() || self.packetizer.fec_limited(payload_bytes),
                wire_bytes,
                pace_bps: bps,
                elapsed: finished.saturating_duration_since(started),
                lost: unrecoverable,
            },
        );
        {
            let mut performance = ctx.s.stats.performance.lock().unwrap();
            performance.record_timing(
                finished,
                butterpollo_media_policy::performance::Timing {
                    period: ctx.stream_period,
                    encode: latency,
                    host: processing,
                    age,
                    sent: micros(finished.saturating_duration_since(claimed)),
                },
                frame_bytes,
            );
            if let Some(claim) = claim.filter(|_| !unrecoverable) {
                let stamps = slices::stamps(
                    &claim,
                    (first_polled, last_polled),
                    streamed.early_blocks(),
                    packetize_started,
                    sent.first_sent,
                    finished,
                );
                performance.record_stages(&stamps, processing);
                ctx.s
                    .record_sent(wire_frame, timestamp, &stamps, self.start);
            }
        }
        self.slice_spare = recycle_streamed_pieces(streamed);
        Ok(())
    }
}
