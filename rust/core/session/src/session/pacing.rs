//! Keyframe recovery, reference invalidation, automatic pacing, and keyframe-loss guard.
use super::*;

impl<P, A> Session<P, A> {
    pub fn request_idr(&self) {
        self.stats.idr_requests.fetch_add(1, Ordering::Relaxed);
        // Intra-only frames already recover; feedback must not bypass cadence.
        if self.config.codec != 3 {
            self.idr.store(true, Ordering::Release);
            self.wake_recovery();
        }
    }
    /// A keyframe the host makes on its own initiative (a recreated encoder,
    /// a frame beyond the packet limit). Whatever the client requests around
    /// it is a reaction to the host, not evidence about the network.
    pub fn request_host_keyframe(&self) {
        self.keyframe_loss.lock().unwrap().host_keyframe();
        self.request_idr();
    }
    /// A keyframe a second after a reference invalidation the decoder may not
    /// have followed. It answers loss, so it is counted like loss.
    pub fn request_confirming_keyframe(&self) {
        self.keyframe_loss.lock().unwrap().loss_keyframe();
        self.idr.store(true, Ordering::Release);
    }
    /// A keyframe the client needs after losing frames: its request (0x0302),
    /// or an invalidation the encoder cannot repair with a reference frame.
    pub fn request_recovery_keyframe(&self) {
        let (latched, ignored, path_epoch) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            let path_epoch = self.path_epoch.load(Ordering::Acquire);
            let latched = loss.requested(Instant::now());
            if self.config.codec == 3 {
                // Intra-only: no keyframe will answer it.
                loss.unanswerable();
            }
            (latched, loss.take_ignored(), path_epoch)
        };
        self.report_keyframe_loss_for_epoch(latched, ignored, path_epoch);
        self.request_idr();
    }
    /// The client asked for a keyframe (0x0302): the same, and evidence for
    /// the bitrate controller when it is the network and not the host or the
    /// client's decode queue that caused the request.
    pub fn request_client_keyframe(&self) {
        self.link_request();
        self.request_recovery_keyframe();
    }
    /// The host could not send enough of a frame for the client to rebuild
    /// it, so the next frame is a keyframe. Moonlight, waiting for one,
    /// then recovers on that frame and sends no request of its own.
    pub fn request_send_loss_recovery(&self) {
        self.stats
            .send_loss_recoveries
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec != 3 {
            self.keyframe_loss.lock().unwrap().loss_keyframe();
            self.idr.store(true, Ordering::Release);
        }
    }
    pub fn request_invalidation(&self, first: u64, last: u64) {
        self.stats
            .reference_invalidations
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec == 3 {
            return;
        }
        self.link_invalidation(first, last);
        if first == 0 || first > last {
            self.request_recovery_keyframe();
            return;
        }
        let mut pending = self.invalidation.lock().unwrap();
        *pending = Some(pending.map_or((first, last), |(a, b)| (a.min(first), b.max(last))));
        drop(pending);
        self.wake_recovery();
    }
    fn wake_recovery(&self) {
        if let Some(wake) = self.recovery_wake.lock().unwrap().as_ref() {
            wake();
        }
    }
    pub(super) fn wireless_unknown_link(&self) -> bool {
        self.config.client_wireless && self.client_link_mbps() == 0
    }

    pub fn configure_automatic_pacing(&self, enabled: bool) {
        let mut pacing = butterpollo_media_policy::adaptive_pacing::AdaptivePacing::new(enabled);
        pacing.set_wireless_unknown_link(self.wireless_unknown_link());
        let mut guard = self.automatic_pacing.lock().unwrap();
        *guard = pacing;
        self.automatic_pacing_seeded.store(false, Ordering::Release);
        self.automatic_pacing_on.store(enabled, Ordering::Release);
    }
    /// Allow sustained client-reported loss (or a remembered lossy route) to
    /// hand packet pacing over to `AdaptivePacing` on wired conventional streams
    /// whose client announces `ML_FF_FEC_STATUS`.
    pub fn configure_adaptive_pacing_on_loss(&self, enabled: bool) {
        let allow = enabled
            && self.config.codec <= 2
            && !self.config.client_wireless
            && (self.config.moonlight_features
                & butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE
                != 0);
        self.adaptive_pacing_on_loss.store(allow, Ordering::Release);
    }
    /// Hand pacing over to `AdaptivePacing` after sustained loss, seeded at
    /// twice the bitrate and fenced at the latest sent frame. Caller must hold
    /// `self.automatic_pacing` and must never hold `self.keyframe_loss`.
    fn activate_adaptive_pacing_locked(
        &self,
        pacing: &mut butterpollo_media_policy::adaptive_pacing::AdaptivePacing,
    ) -> bool {
        if !self.adaptive_pacing_on_loss.load(Ordering::Acquire) {
            return false;
        }
        if pacing.enabled() {
            return true;
        }
        let last_sent = self
            .stats
            .video_frame
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(|v| v as u32);
        let mut next = butterpollo_media_policy::adaptive_pacing::AdaptivePacing::new_after_loss(
            last_sent,
            match self.last_fast_bps.load(Ordering::Acquire) {
                0 => None,
                bps => Some(bps),
            },
        );
        next.set_fec_percent(self.fec_percent());
        next.set_wireless_unknown_link(self.wireless_unknown_link());
        *pacing = next;
        self.automatic_pacing_seeded.store(false, Ordering::Release);
        self.automatic_pacing_on.store(true, Ordering::Release);
        true
    }
    /// Whether the client reports a wireless route (`x-rl-video[0].clientWireless`)
    /// and no adaptive controller owns this stream. Then the route base is
    /// paced as Wi-Fi from the first frame. When the controller owns the stream
    /// the base stays the host route's, so the controller starts conservatively
    /// at its own seed and can probe up toward the real ceiling.
    pub fn client_wireless_sets_route_pacing(&self) -> bool {
        self.config.client_wireless && !self.automatic_pacing_enabled()
    }
    /// Read per frame, so it is the flag set at stream start or on loss handoff, not the mutex.
    pub fn automatic_pacing_enabled(&self) -> bool {
        self.automatic_pacing_on.load(Ordering::Acquire)
    }
    pub fn video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let mut pacing = self.automatic_pacing.lock().unwrap();
        pacing.set_fec_percent(self.fec_percent());
        pacing.set_wireless_unknown_link(self.wireless_unknown_link());
        let paced = pacing.rate(base, needed, self.bitrate.load(Ordering::Acquire));
        if pacing.enabled() {
            self.automatic_pacing_seeded.store(true, Ordering::Release);
        }
        drop(pacing);
        self.adaptive_pacing_bps(paced)
    }
    /// Encoder-side budget queries must not change the sender's feedback
    /// state when recovery parity differs from the current ordinary-frame FEC.
    pub fn preview_video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let paced = self.automatic_pacing.lock().unwrap().preview_rate_for(
            base,
            needed,
            self.bitrate.load(Ordering::Acquire),
            self.wireless_unknown_link(),
        );
        self.adaptive_pacing_bps(paced)
    }
    /// Pacing slows for the session once the client loses a keyframe or
    /// repeatedly reports unrecoverable ordinary frames
    /// (`keyframe_loss_pacing`), including PyroWave's independent intra frames.
    pub fn configure_keyframe_loss(&self, enabled: bool) {
        self.configure_remembered_keyframe_loss(enabled, false);
    }
    /// Restore only an eligible route's recent loss history. Explicit pacing,
    /// disabled adaptation and PyroWave retain their own policies.
    pub fn configure_remembered_keyframe_loss(&self, enabled: bool, remembered: bool) {
        let remembered = remembered && enabled && self.config.codec != 3;
        let mut loss = if remembered {
            butterpollo_media_policy::keyframe_loss::KeyframeLoss::remembered(
                "remembered from the previous session",
            )
        } else {
            butterpollo_media_policy::keyframe_loss::KeyframeLoss::new(enabled)
        };
        loss.set_reports_losses(
            self.config.moonlight_features & butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE
                != 0,
        );
        let path_epoch = {
            let mut guard = self.keyframe_loss.lock().unwrap();
            let path_epoch = self.path_epoch.load(Ordering::Acquire);
            *guard = loss;
            path_epoch
        };
        let mut pacing = self.automatic_pacing.lock().unwrap();
        if self.path_epoch.load(Ordering::Acquire) != path_epoch {
            return;
        }
        if remembered {
            if self.activate_adaptive_pacing_locked(&mut pacing) {
                tracing::info!(stream_id = %self.launch.id, "starting adaptive packet pacing from the first frame: remembered from the previous session");
                self.launch.warnings.set("network_keyframe_loss", "This device lost video frames in a previous stream at this address. Video starts at the Wi-Fi pacing rate and probes up as the link stays clean to limit packet bursts without reducing the encoder bitrate. This memory expires after 24 hours and the host periodically tests the fast rate. Reset it in Devices after changing the network.");
            } else {
                tracing::info!(stream_id = %self.launch.id, "pacing video like Wi-Fi from the first frame: remembered from the previous session");
                self.launch.warnings.set("network_keyframe_loss", "This device lost video frames in a previous stream at this address. Video starts at the Wi-Fi pacing rate to limit packet bursts without reducing the encoder bitrate. This memory expires after 24 hours and the host periodically tests the fast rate. Reset it in Devices after changing the network.");
            }
        } else {
            self.launch.warnings.clear("network_keyframe_loss");
        }
    }
    #[cfg(test)]
    pub(super) fn report_keyframe_loss(
        &self,
        latched: Option<butterpollo_media_policy::keyframe_loss::Latched>,
        ignored: Option<String>,
    ) {
        self.report_keyframe_loss_for_epoch(
            latched,
            ignored,
            self.path_epoch.load(Ordering::Acquire),
        );
    }
    pub(super) fn report_keyframe_loss_for_epoch(
        &self,
        latched: Option<butterpollo_media_policy::keyframe_loss::Latched>,
        ignored: Option<String>,
        path_epoch: u64,
    ) {
        if self.path_epoch.load(Ordering::Acquire) != path_epoch {
            return;
        }
        if let Some(why) = ignored {
            tracing::info!(stream_id = %self.launch.id, why, "video loss evidence not counted");
        }
        if let Some(butterpollo_media_policy::keyframe_loss::Latched {
            reason,
            detail,
            kind,
            confirmed,
            from,
            host_only,
        }) = latched
        {
            use butterpollo_media_policy::keyframe_loss::LatchKind;
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                // A provisional latch confirmed by more evidence is the same
                // latch, already counted when it began.
                link.rate.on_latch(
                    Instant::now(),
                    self.pace_was_fast.load(Ordering::Relaxed),
                    !confirmed,
                );
            }
            let mut pacing = self.automatic_pacing.lock().unwrap();
            if self.path_epoch.load(Ordering::Acquire) != path_epoch {
                return;
            }
            let adaptive = kind == LatchKind::Sustained
                && !host_only
                && self.activate_adaptive_pacing_locked(&mut pacing);
            let adaptive_active = pacing.enabled();
            if adaptive {
                if confirmed && matches!(from, Some(LatchKind::Send | LatchKind::Sustained)) {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "the client reported video loss after the host's send loss; handing packet pacing over to adaptive pacing");
                } else if confirmed {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "more video loss after the start-up keyframe; handing packet pacing over to adaptive pacing");
                } else {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "video frames are not arriving; handing packet pacing over to adaptive pacing");
                }
                self.launch.warnings.set("network_keyframe_loss", format!("Video frames were lost on the way to this device ({reason}), so adaptive packet pacing is now active starting from twice the bitrate and probing up as the link stays clean without reducing the encoder bitrate. Check the network path if loss continues."));
            } else if host_only && adaptive_active {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the host's socket refused video packets while adaptive packet pacing is active; keeping adaptive packet pacing");
                self.launch.warnings.set("network_keyframe_loss", format!("The PC could not send video packets fast enough ({reason}; its CPU or network adapter fell behind). Adaptive packet pacing remains active without reducing the encoder bitrate. The device reported no loss; close other work on the PC, or check the network adapter and its driver, if this continues."));
            } else {
                if confirmed && matches!(from, Some(LatchKind::Send | LatchKind::Sustained)) {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "the client reported video loss after the host's send loss; pacing video like a Wi-Fi route for the rest of the stream");
                } else if confirmed {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "more video loss after the start-up keyframe; pacing video like a Wi-Fi route for the rest of the stream");
                } else if kind == LatchKind::Send {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "the host's socket refused video packets; pacing video like a Wi-Fi route until it has been clean for 10 s");
                } else if kind == LatchKind::Startup {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "the stream's first keyframe was lost; pacing video like a Wi-Fi route until it has been clean for 10 s");
                } else if host_only {
                    // After a lift, or on a remembered route: held, never saved.
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "the host's socket refused video packets in a stream that already slowed down; pacing video like a Wi-Fi route for the rest of the stream");
                } else {
                    tracing::warn!(stream_id = %self.launch.id, reason, detail, "video frames are not arriving; pacing video like a Wi-Fi route for the rest of the stream");
                }
                if kind == LatchKind::Startup {
                    self.launch.warnings.set("network_keyframe_loss", format!("The first video frame was lost on the way to this device ({reason}), so video is paced like Wi-Fi for now. This limits packet bursts without reducing the encoder bitrate. The host returns to the normal rate after 10 seconds without further loss."));
                } else if kind == LatchKind::Send {
                    self.launch.warnings.set("network_keyframe_loss", format!("The PC could not send video packets fast enough ({reason}; its CPU or network adapter fell behind), so video is paced like Wi-Fi for now. This limits packet bursts without reducing the encoder bitrate. The host returns to the normal rate after 10 seconds without further loss."));
                } else if host_only {
                    self.launch.warnings.set("network_keyframe_loss", format!("The PC could not send video packets fast enough ({reason}; its CPU or network adapter fell behind), so video is paced like Wi-Fi for the rest of the stream. This limits packet bursts without reducing the encoder bitrate. The device reported no loss; close other work on the PC, or check the network adapter and its driver, if this continues."));
                } else {
                    self.launch.warnings.set("network_keyframe_loss", format!("Video frames were lost on the way to this device ({reason}), so video is now paced like Wi-Fi for the rest of the stream. This limits packet bursts without reducing the encoder bitrate. Check the network path if loss continues."));
                }
            }
        }
    }
    /// Read the loss guard without recording a frame that has not been sent.
    /// Sender selection uses this before deciding whether to send inline.
    pub fn keyframe_loss_rate_bps(&self, base: u64, safe: u64) -> u64 {
        if self.automatic_pacing_enabled() {
            base
        } else {
            self.keyframe_loss.lock().unwrap().bps(base, safe)
        }
    }
    /// The host dropped this frame because the client's network route was
    /// unavailable: mark it abandoned for `KeyframeLoss` so client repeats or FEC
    /// reports do not treat it as in-transit loss, without penalizing send loss
    /// counters, pacing, or link rate.
    pub fn keyframe_abandoned_route(&self, wire_frame: u32, reached_socket: bool) {
        self.keyframe_loss
            .lock()
            .unwrap()
            .route_abandoned(wire_frame, reached_socket);
    }
    /// The host stopped sending this frame part way because its socket
    /// refused packets: loss on send. See [`Self::keyframe_abandoned_as`].
    pub fn keyframe_abandoned(&self, wire_frame: u32) {
        self.keyframe_abandoned_as(wire_frame, true);
    }
    /// The host stopped sending this frame part way: loss on send. It counts
    /// toward slower pacing under its own reason, and the client's report of
    /// the same frame is not counted again. `refused` is whether the socket
    /// refused packets of it (a full buffer); a frame lost only to datagrams
    /// too large for the path does not make the PC's send path the limit.
    pub fn keyframe_abandoned_as(&self, wire_frame: u32, refused: bool) {
        self.stats
            .frames_lost_on_send
            .fetch_add(1, Ordering::Relaxed);
        let period = self.stream_mode().period();
        let (latched, ignored, path_epoch) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            let path_epoch = self.path_epoch.load(Ordering::Acquire);
            (
                loss.abandoned(wire_frame, period, Instant::now()),
                loss.take_ignored(),
                path_epoch,
            )
        };
        if self.link_sent.live() {
            let mut link = self.link_rate.lock().unwrap();
            link.drain(&self.link_sent);
            if refused {
                link.rate.on_send_loss(wire_frame, Instant::now());
            } else {
                link.rate.on_send_too_large(wire_frame, Instant::now());
            }
        }
        self.report_keyframe_loss_for_epoch(latched, ignored, path_epoch);
    }
    pub fn keyframe_loss_active(&self) -> bool {
        self.keyframe_loss.lock().unwrap().latched()
    }
    pub fn keyframe_loss_reason(&self) -> Option<&'static str> {
        self.keyframe_loss.lock().unwrap().reason()
    }
    pub fn keyframe_loss_observed_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().observed_at()
    }
    /// When loss last proved sustained: the only loss the next session
    /// inherits. A provisional (start-up) latch has none.
    pub fn keyframe_loss_sustained_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().sustained_at()
    }
    /// Any latch happened in this stream, even a provisional one that lifted.
    pub fn keyframe_loss_ever_latched(&self) -> bool {
        self.keyframe_loss.lock().unwrap().ever_latched()
    }
    /// The pacing rate for the frame about to be sent as `wire_frame`: the
    /// route policy's `base`, or after confirmed losses at most `safe`, the
    /// Wi-Fi route's rate for this stream.
    pub fn keyframe_loss_bps(&self, base: u64, safe: u64, wire_frame: u32, keyframe: bool) -> u64 {
        self.keyframe_loss_bps_at(base, safe, wire_frame, keyframe, Instant::now())
    }
    pub(super) fn keyframe_loss_bps_at(
        &self,
        base: u64,
        safe: u64,
        wire_frame: u32,
        keyframe: bool,
        now: Instant,
    ) -> u64 {
        let (bps, lifted) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            loss.sending_at(wire_frame, keyframe, now);
            let automatic = self.automatic_pacing_enabled();
            let bps = if automatic && self.automatic_pacing_seeded.load(Ordering::Acquire) {
                base
            } else {
                loss.bps(base, safe)
            };
            if !loss.latched() && !automatic {
                self.last_fast_bps
                    .store(if bps > safe { bps } else { 0 }, Ordering::Release);
            }
            (bps, loss.take_lifted())
        };
        // Fast is what was applied to this frame, not what the route allows.
        let fast = bps > safe;
        self.pace_was_fast.store(fast, Ordering::Relaxed);
        // Every PyroWave picture is intra and its loss guard is off, so no
        // latch watch needs them, and the sender must not wait for this lock.
        if keyframe && self.config.codec != 3 && self.link_sent.live() {
            self.link_rate
                .lock()
                .unwrap()
                .rate
                .on_keyframe_sent(wire_frame, now);
        }
        if let Some(why) = lifted {
            tracing::info!(stream_id = %self.launch.id, why, "returning to the route's pacing rate: the loss did not recur");
            self.launch.warnings.clear("network_keyframe_loss");
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                link.rate.on_unlatch(now, fast);
            }
        }
        bps
    }
    /// Whether legacy `AdaptiveBitrate` is configured; skips its mutex on control pings when off.
    pub fn adaptive_bitrate_enabled(&self) -> bool {
        self.adaptive_bitrate_on.load(Ordering::Acquire)
    }

    pub fn record_control_ping(&self, now: Instant, payload: &[u8]) {
        if self.adaptive_bitrate_enabled() {
            self.adaptive_bitrate.lock().unwrap().on_ping(now, payload);
        }
        if self.link_sent.live() {
            self.link_rate.lock().unwrap().rate.on_ping(now, payload);
        }
        self.automatic_pacing.lock().unwrap().on_ping(now, payload);
    }
    pub fn tick_automatic_pacing(&self, now: Instant, rtt: Duration) -> Option<(u64, u64)> {
        self.tick_automatic_pacing_step(now, rtt)
            .map(|step| (step.old_bps, step.new_bps))
    }

    pub fn tick_automatic_pacing_step(
        &self,
        now: Instant,
        rtt: Duration,
    ) -> Option<butterpollo_media_policy::adaptive_pacing::PacingStep> {
        let period = self.stream_mode().period();
        let mut pacing = self.automatic_pacing.lock().unwrap();
        pacing.set_fec_percent(self.fec_percent());
        pacing.set_wireless_unknown_link(self.wireless_unknown_link());
        pacing.tick_step(
            now,
            butterpollo_media_policy::adaptive_pacing::Traffic {
                frames: self.stats.frames.load(Ordering::Relaxed),
                bytes: self.stats.bytes.load(Ordering::Relaxed),
                packets: self.stats.packets.load(Ordering::Relaxed),
                send_losses: self.stats.send_loss_recoveries.load(Ordering::Relaxed),
            },
            self.stats
                .video_frame
                .load(Ordering::Acquire)
                .checked_sub(1)
                .map(|v| v as u32),
            period,
            self.bitrate.load(Ordering::Acquire),
            rtt,
        )
    }
}
