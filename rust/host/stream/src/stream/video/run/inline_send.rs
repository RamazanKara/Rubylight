//! Inline video frame packetization and lazy handoff to the dedicated `video_send` thread.

mod sliced;
mod whole;

use super::super::*;

/// Borrowed context for sending encoded frames from the session loop.
pub(super) struct SendContext<'a> {
    pub(super) m: &'a Media,
    pub(super) h: &'a Shared,
    pub(super) s: &'a Arc<Session>,
    pub(super) c: &'a Config,
    pub(super) timer: &'a butterpollo_win_graphics::timing::Timer,
    pub(super) progress: &'a crate::stall_watch::Progress,
    pub(super) prepared: &'a Arc<crate::display_session::Ready>,
    pub(super) claims: &'a std::cell::RefCell<butterpollo_media_policy::performance::Claims>,
    pub(super) pyrowave_sender: &'a Option<crate::pyrowave_send::Sender>,
    pub(super) webrtc_sender: &'a Option<Box<dyn crate::webrtc_bridge::WebRtcVideoSink>>,
    pub(super) stream_period: Duration,
}

/// Inline video send path and its lazy handoff into `crate::video_send::Sender`.
pub(super) struct InlineSender {
    pub(super) automatic_pacing: bool,
    pub(super) packetizer: VideoPacketizer,
    pub(super) next_wire_frame: std::cell::Cell<u64>,
    pub(super) start: Instant,
    pub(super) present_stamper: Option<butterpollo_win_graphics::present_timing::Stamper>,
    pub(super) last_stamp: Instant,
    pub(super) batch: butterpollo_win_io::net::Batch,
    pub(super) packets: Vec<Vec<u8>>,
    pub(super) trace_send: bool,
    pub(super) network_pacer: butterpollo_media_policy::network_pacing::Pacer,
    pub(super) link: Option<butterpollo_win_io::net::Link>,
    pub(super) routed: Option<SocketAddr>,
    pub(super) pacing_report_due: Instant,
    pub(super) reported_pacing: Option<u64>,
    pub(super) link_watch: Arc<crate::video_send::link_watch::LinkWatch>,
    pub(super) fec_reported: Option<Instant>,
    pub(super) slow_send_reported: Option<Instant>,
    pub(super) send_outage: Option<butterpollo_media_policy::stream_policy::SendOutage>,
    pub(super) video_sender: std::cell::OnceCell<crate::video_send::Sender>,
    pub(super) send_thread: bool,
    pub(super) send_jitter: Option<butterpollo_media_policy::stream_policy::SendJitter>,
    pub(super) send_jitter_set: bool,
    pub(super) sender_decided: bool,
    pub(super) inline_paced: bool,
    pub(super) send_loss: butterpollo_media_policy::stream_policy::SendLossRecovery,
    pub(super) delivery: slices::Delivery,
    pub(super) slice_route: slices::Route,
    pub(super) streaming: Option<slices::Streaming>,
    pub(super) slice_spare: Vec<Vec<u8>>,
    pub(super) send_drop: Option<butterpollo_media_policy::stream_policy::SendDrop>,
    pub(super) batch_kb: usize,
}

impl InlineSender {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        s: &Session,
        c: &Config,
        encoder: &Option<Encoder>,
        workers: &Workers,
        automatic_pacing: bool,
        packetizer: VideoPacketizer,
        next_wire_frame: std::cell::Cell<u64>,
        start: Instant,
        present_stamper: Option<butterpollo_win_graphics::present_timing::Stamper>,
        trace_send: bool,
    ) -> Self {
        let mut batch = butterpollo_win_io::net::Batch::default();
        batch.dont_fragment = crate::stream::video_refuses_fragmentation();
        batch.waits = trace_send.then(Default::default);
        let send_thread = config::video_send_thread(c);
        // A test send jitter stalls only the sender thread's paced sends,
        // which a stall never holds back from capture.
        let send_jitter = butterpollo_media_policy::stream_policy::SendJitter::from_env();
        if let Some(jitter) = &send_jitter {
            let ignored_by = if workers.pyrowave_sender.is_some() {
                Some("pyrowave")
            } else if workers.webrtc_sender.is_some() {
                Some("webrtc")
            } else if !send_thread {
                Some("video_send_thread=false")
            } else {
                None
            };
            jitter.warn(ignored_by);
        }
        let send_jitter_set = send_jitter.is_some();
        // `video_slice_delivery`: AMF returns a P-frame in slices and
        // they are sent in FEC blocks as they arrive, here or on the
        // sender thread. Off for the rest of the session after a
        // frame passes its byte cap. The block count follows the
        // encoder, which `auto` builds from the picture size.
        let delivery = slices::Delivery::new(
            encoder
                .as_ref()
                .map_or(0, |e| usize::from(e.slice_delivery())),
        );
        tracing::info!(
            client = %s.launch.client.name,
            slice_blocks = delivery.blocks(),
            "video slice delivery"
        );
        Self {
            automatic_pacing,
            packetizer,
            next_wire_frame,
            start,
            present_stamper,
            last_stamp: start,
            batch,
            packets: Vec::new(),
            trace_send,
            network_pacer: butterpollo_media_policy::network_pacing::Pacer::new(Instant::now()),
            link: None,
            routed: None,
            pacing_report_due: Instant::now(),
            reported_pacing: None,
            link_watch: Arc::new(crate::video_send::link_watch::LinkWatch::default()),
            fec_reported: None,
            slow_send_reported: None,
            send_outage: butterpollo_media_policy::stream_policy::SendOutage::from_env(),
            video_sender: std::cell::OnceCell::new(),
            send_thread,
            send_jitter,
            send_jitter_set,
            sender_decided: false,
            inline_paced: false,
            send_loss: butterpollo_media_policy::stream_policy::SendLossRecovery::default(),
            delivery,
            slice_route: slices::Route::default(),
            streaming: None,
            slice_spare: Vec::new(),
            send_drop: butterpollo_media_policy::stream_policy::SendDrop::from_env(),
            batch_kb: config::video_max_batch_size_kb(c),
        }
    }

    pub(super) fn send_frames(
        &mut self,
        ctx: &SendContext<'_>,
        output: Vec<butterpollo_win_graphics::encoder::Encoded>,
        peer: SocketAddr,
        call_latency: Duration,
        source: &Source,
        processing_started: Instant,
    ) -> Result<()> {
        ctx.progress.mark(crate::stall_watch::Phase::Send);
        if !output.is_empty() {
            ctx.s.launch.warnings.clear("encoder_recovery");
            source.device_recovered(&source.warnings);
        }
        crate::stream::dvr::tap_video(ctx.s, &output);
        if let Some(sender) = ctx.pyrowave_sender {
            return sender.submit(
                output,
                peer,
                call_latency,
                processing_started,
                &mut ctx.claims.borrow_mut(),
            );
        }
        if let Some(sender) = ctx.webrtc_sender {
            return sender.submit(
                ctx.s,
                output,
                call_latency,
                processing_started,
                &mut ctx.claims.borrow_mut(),
            );
        }
        self.maybe_handoff(ctx, &output, peer)?;
        let output = self.route_slices(ctx, output);
        if let Some(sender) = self.video_sender.get() {
            return sender.submit_tick(
                output,
                peer,
                call_latency,
                &mut ctx.claims.borrow_mut(),
                processing_started,
            );
        }
        let polled = Instant::now();
        for frame in output {
            if frame.unit != butterpollo_win_graphics::encoder::Unit::Whole {
                self.send_sliced_unit(ctx, frame, peer, call_latency, processing_started, polled)?;
            } else {
                self.send_whole_frame(ctx, frame, peer, call_latency, processing_started, polled)?;
            }
        }
        Ok(())
    }

    fn maybe_handoff(
        &mut self,
        ctx: &SendContext<'_>,
        output: &[butterpollo_win_graphics::encoder::Encoded],
        peer: SocketAddr,
    ) -> Result<()> {
        // A large picture or feedback can make even a fast
        // route block capture. Hand off before that frame sends.
        let loss_paced =
            self.send_thread && self.video_sender.get().is_none() && ctx.s.keyframe_loss_active();
        // A streamed frame stays on this path until its last unit.
        if !(self.send_thread
            && self.video_sender.get().is_none()
            && !slices::open(&self.slice_route, &self.streaming)
            && (!self.sender_decided || !output.is_empty() || self.inline_paced || loss_paced))
        {
            return Ok(());
        }
        let mid_stream = self.sender_decided;
        self.sender_decided = true;
        let route = self.link_watch.get(peer);
        self.link = Some(route);
        self.packetizer.fec_percent = ctx.s.fec_percent();
        let bitrate = ctx.s.bitrate.load(Ordering::Relaxed);
        let crate::video_send::recovery::Pace { bps, needed } = crate::video_send::recovery::pace(
            ctx.s,
            ctx.c,
            &self.packetizer,
            ctx.stream_period,
            peer,
            route,
        );
        let mut frame_packetizer = self.packetizer.clone();
        let large_frame = output.iter().any(|frame| {
            frame_packetizer.fec_percent = ctx
                .s
                .video_fec_percent(frame.idr || frame.after_invalidation, frame.bytes.len());
            let wire = butterpollo_media_policy::network_pacing::video_frame_wire_rate_bps(
                &frame_packetizer,
                frame.bytes.len(),
                ctx.stream_period,
                peer.ip().to_canonical().is_ipv6(),
            );
            butterpollo_media_policy::network_pacing::paced(bps, wire)
        });
        if std::mem::take(&mut self.inline_paced)
            || loss_paced
            || large_frame
            || self.automatic_pacing
            || self.send_jitter_set
            || butterpollo_media_policy::network_pacing::paced(bps, needed)
        {
            // The sender continues the inline sends' frame,
            // packet and IV numbering; nothing sends inline
            // after this.
            let handoff = crate::video_send::Handoff {
                packetizer: self.packetizer.clone(),
                present_stamper: self.present_stamper.take(),
                last_stamp: self.last_stamp,
                pacing_due: self.network_pacer.due(),
                send_loss: std::mem::take(&mut self.send_loss),
                slice_blocks: self.delivery.blocks(),
                slices: ctx.s.config.slices as usize,
                send_jitter: self.send_jitter.take(),
                link: self.link_watch.clone(),
            };
            let _ = self.video_sender.set(crate::video_send::Sender::new(
                ctx.m.video.clone(),
                ctx.s.clone(),
                ctx.c.clone(),
                ctx.h.clone(),
                self.start,
                handoff,
            )?);
            tracing::info!(
                pacing_bps = bps,
                bitrate_kbps = bitrate,
                wire_budget_bps = needed,
                automatic_pacing = self.automatic_pacing,
                large_frame,
                send_jitter = self.send_jitter_set,
                mid_stream,
                wire_frame = self.packetizer.frame,
                "video frames are sent on their own thread for current or adaptive pacing"
            );
        }
        Ok(())
    }

    fn route_slices(
        &mut self,
        ctx: &SendContext<'_>,
        output: Vec<butterpollo_win_graphics::encoder::Encoded>,
    ) -> Vec<butterpollo_win_graphics::encoder::Encoded> {
        // Units of a sliced frame: stream them here, or join them
        // into the whole frame the other paths send.
        if !output
            .iter()
            .any(|frame| frame.unit != butterpollo_win_graphics::encoder::Unit::Whole)
            && !self.slice_route.streaming()
        {
            return output;
        }
        let threaded = self.video_sender.get().is_some();
        self.delivery.sync(&ctx.s.stats);
        self.packetizer.fec_percent = ctx.s.frame_fec_percent(false);
        let mut routed = Vec::with_capacity(output.len());
        for frame in output {
            let result = self.slice_route.route(frame, |first| {
                self.delivery.streams()
                    && first.frame_cap_bytes.is_some_and(|cap| {
                        self.packetizer
                            .streamable(self.delivery.blocks(), cap as usize)
                    })
            });
            // On the sender thread, it sees the new frame begin and drops the old one itself.
            if result.interrupted && !threaded {
                // Its index is consumed if no block left, or the client sees no gap.
                if let Some(st) = &self.streaming {
                    self.batch.end_frame();
                    if st.sent.packets > 0 {
                        ctx.s.keyframe_abandoned_as(st.wire_frame, false);
                    }
                }
                let next = slices::drop_frame(
                    &mut self.streaming,
                    &mut self.packetizer,
                    &mut self.slice_spare,
                );
                ctx.s.launch.warnings.event(
                    "network_frame",
                    "A streamed video frame lost its last slice; requesting a recovery frame.",
                    butterpollo_session::session::EVENT_PERIOD,
                );
                ctx.s.request_host_keyframe();
                self.next_wire_frame.set(u64::from(next));
            }
            routed.extend(result.frame);
        }
        routed
    }
}

/// Returns `(frame_bytes, wire_bytes)` for the `sent_packets` actually
/// attempted (excluding unreached abandoned blocks), matching `sliced` and `whole`.
pub(super) fn attempted_wire_bytes(
    sent_packets: usize,
    packet_bytes: usize,
    ipv6: bool,
) -> (u64, u64) {
    let frame_bytes = (sent_packets * packet_bytes) as u64;
    let overhead = (sent_packets * butterpollo_media_policy::network_pacing::overhead(ipv6)) as u64;
    (frame_bytes, frame_bytes.saturating_add(overhead))
}

/// Records only the `sent_count` packets that actually reached the socket
/// (clamped to `packets.len()`) into congestion tracking (at pre-send `send_at`)
/// and network pacing (at post-send `finished`), matching `video_send::transport`.
#[allow(clippy::too_many_arguments)]
pub(super) fn record_inline_sent(
    session: &Session,
    pacer: &mut butterpollo_media_policy::network_pacing::Pacer,
    packets: &[Vec<u8>],
    (sent_bytes, sent_count): (usize, usize),
    ipv6: bool,
    bps: u64,
    send_at: Instant,
    finished: Instant,
) {
    let sent = sent_count.min(packets.len());
    if sent > 0 && session.transport_sent.live() {
        session.transport_sent.record(
            &packets[..sent],
            butterpollo_media_policy::network_pacing::overhead(ipv6),
            send_at,
        );
    }
    pacer.sent(finished, sent_bytes, sent, ipv6, bps);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_session::session::Role;

    #[test]
    fn partial_batch_send_records_only_packets_that_reached_the_socket() {
        let f = Fixture::new();
        let session = f.session(f.client(u32::MAX), Role::Stream);
        let now = Instant::now();
        let mut pacer = butterpollo_media_policy::network_pacing::Pacer::new(now);
        // Construct 4 synthetic RTP-like packets with sequence numbers 1..=4.
        let packets: Vec<Vec<u8>> = (1u16..=4)
            .map(|seq| {
                let mut p = vec![0u8; 1024];
                p[0] = 0x80;
                p[2..4].copy_from_slice(&seq.to_be_bytes());
                p
            })
            .collect();
        // Simulate a partial batch where only 2 of 4 packets left the socket.
        record_inline_sent(
            &session,
            &mut pacer,
            &packets,
            (2048, 2),
            false,
            80_000_000,
            now,
            now,
        );
        // Pacer due time must reflect 2 packets' overhead, not 4.
        let expected_wire = 2048 + 2 * butterpollo_media_policy::network_pacing::overhead(false);
        let expected_ns = (expected_wire as f64 * 8.0 / 80_000_000.0 * 1e9) as u64;
        let actual_ns = pacer.due().saturating_duration_since(now).as_nanos() as u64;
        assert!(
            actual_ns.abs_diff(expected_ns) <= 2_000,
            "expected ~{expected_ns} ns for 2 packets, got {actual_ns} ns"
        );
        // Zero sent packets must not advance the pacer.
        let due_before = pacer.due();
        record_inline_sent(
            &session,
            &mut pacer,
            &packets,
            (0, 0),
            false,
            80_000_000,
            now,
            now,
        );
        assert_eq!(pacer.due(), due_before);
    }

    #[test]
    fn attempted_wire_bytes_excludes_abandoned_tail_packets() {
        let overhead_v4 = butterpollo_media_policy::network_pacing::overhead(false) as u64;
        let (frame_bytes, wire_bytes) = attempted_wire_bytes(3, 1040, false);
        assert_eq!(frame_bytes, 3 * 1040);
        assert_eq!(wire_bytes, 3 * (1040 + overhead_v4));
    }
}
