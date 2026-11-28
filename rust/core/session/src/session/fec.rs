//! Per-session adaptive and statistical FEC configuration, feedback handling, and sizing.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct WholePayloadCache {
    revision: u64,
    ceiling: usize,
    slice: usize,
    minimum: usize,
    fresh: bool,
    settled: bool,
    limit: usize,
}

impl<P, A> Session<P, A> {
    pub fn record_fec_status(&self, payload: &[u8]) {
        let last_sent = self
            .stats
            .video_frame
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(|v| v as u32);
        let block = self
            .stats
            .performance
            .lock()
            .unwrap()
            .record_fec_status(payload, last_sent);
        if let Some(block) = block {
            // Parsing and sent-frame/block validation above succeeded. The
            // additional controller fence rejects feedback from an older rate.
            let frame = u32::from_be_bytes(payload[..4].try_into().unwrap());
            let period = self.stream_mode().period();
            self.link_status(payload, frame, block);
            let (route_abandoned, keyframe) = {
                let mut loss = self.keyframe_loss.lock().unwrap();
                let route_abandoned = loss.is_route_abandoned(frame);
                let keyframe = loss.is_keyframe(frame);
                if matches!(
                    block,
                    butterpollo_media_policy::adaptive_fec::Block::Unrecoverable
                ) {
                    let latched =
                        loss.unrecoverable(frame, last_sent.unwrap(), period, Instant::now());
                    let ignored = loss.take_ignored();
                    drop(loss);
                    self.report_keyframe_loss(latched, ignored);
                }
                (route_abandoned, keyframe)
            };
            if !route_abandoned {
                self.automatic_pacing.lock().unwrap().on_block_kind(
                    frame,
                    last_sent.unwrap(),
                    period,
                    block,
                    keyframe,
                );
                self.adaptive_bitrate.lock().unwrap().on_block(block);
                let mut fec = self.adaptive_fec.lock().unwrap();
                if fec.enabled() {
                    fec.on_block(Instant::now(), block);
                    self.fec_percent
                        .store(fec.percent() as u32, Ordering::Release);
                }
            }
        }
    }
    /// Starts the stream's FEC at the configured percentage (`fec_percentage`,
    /// 0-100), which adaptive FEC never exceeds. Off, or with a client that never
    /// reports, every frame uses the configured percentage. Returns it.
    pub fn configure_fec(&self, adaptive: bool, configured: usize) -> usize {
        let mut fec = self.adaptive_fec.lock().unwrap();
        *fec = if self.config.codec == 3 {
            butterpollo_media_policy::adaptive_fec::AdaptiveFec::pyrowave(adaptive, configured)
        } else {
            butterpollo_media_policy::adaptive_fec::AdaptiveFec::new(adaptive, configured)
        };
        let percent = fec.percent();
        self.fec_percent.store(percent as u32, Ordering::Release);
        percent
    }
    /// Starts the stream's FEC as the client chose it with `rlFecPercentage`:
    /// the host's own (`adaptive` and `configured`, as [`Self::configure_fec`]),
    /// a fixed share capped at `maximum` (`FEC_MAX_PERCENTAGE`), or statistical
    /// FEC up to `maximum` that keeps `configured` until the client's reports
    /// arrive. PyroWave keeps its own protection. Returns the first percentage.
    pub fn configure_fec_choice(
        &self,
        choice: butterpollo_media_policy::extensions::FecChoice,
        adaptive: bool,
        configured: usize,
        maximum: u8,
    ) -> usize {
        use butterpollo_media_policy::extensions::FecChoice;
        let mut client = self.client_fec.lock().unwrap();
        *client = None;
        self.client_fec_on.store(false, Ordering::Release);
        self.fec_split_on.store(false, Ordering::Release);
        *self.last_fec_report_at.lock().unwrap() = None;
        self.fec_revision.fetch_add(1, Ordering::Release);
        if self.config.codec == 3 || choice == FecChoice::Host {
            drop(client);
            return self.configure_fec(adaptive, configured);
        }
        let fallback = configured.min(255) as u8;
        let percent = match choice {
            FecChoice::Fixed(percent) => percent.min(maximum),
            _ => {
                *client = Some(
                    butterpollo_media_policy::statistical_fec::StatisticalFec::new(
                        maximum,
                        fallback,
                        self.config.min_fec,
                        Instant::now(),
                    ),
                );
                self.client_fec_on.store(true, Ordering::Release);
                self.fec_revision.fetch_add(1, Ordering::Release);
                fallback
            }
        };
        *self.adaptive_fec.lock().unwrap() =
            butterpollo_media_policy::adaptive_fec::AdaptiveFec::fixed(percent);
        self.fec_percent
            .store(u32::from(percent), Ordering::Release);
        self.fec_split.store(u32::from(percent), Ordering::Release);
        self.fec_split_on.store(true, Ordering::Release);
        usize::from(percent)
    }
    /// Takes a 0x5539 FEC report: the report and whether statistical FEC
    /// used it (only under `rlFecPercentage=auto`), or why it was refused.
    pub fn record_fec_report(
        &self,
        payload: &[u8],
    ) -> std::result::Result<(butterpollo_media_policy::statistical_fec::FecReport, bool), String>
    {
        self.record_fec_report_at(payload, Instant::now())
    }
    pub(super) fn record_fec_report_at(
        &self,
        payload: &[u8],
        now: Instant,
    ) -> std::result::Result<(butterpollo_media_policy::statistical_fec::FecReport, bool), String>
    {
        let result = self.take_fec_report(payload, now);
        self.stats
            .performance
            .lock()
            .unwrap()
            .record_fec_loss_report(result.is_ok());
        result
    }
    fn take_fec_report(
        &self,
        payload: &[u8],
        now: Instant,
    ) -> std::result::Result<(butterpollo_media_policy::statistical_fec::FecReport, bool), String>
    {
        let report = butterpollo_media_policy::statistical_fec::FecReport::decode(payload)
            .ok_or_else(|| format!("malformed ({} bytes)", payload.len()))?;
        let (first, pending) = {
            let mut client = self.client_fec.lock().unwrap();
            let Some(fec) = client.as_mut() else {
                return Ok((report, false));
            };
            let first = !fec.reported();
            (first, fec.observe(now, &report).map_err(|e| e.to_string())?)
        };
        // The tables are built without the lock every frame takes: frames keep
        // the previous estimate until they are in.
        let estimate =
            pending.map(butterpollo_media_policy::statistical_fec::PendingEstimate::build);
        let mut client = self.client_fec.lock().unwrap();
        let Some(fec) = client.as_mut() else {
            // The stream left statistical FEC meanwhile.
            return Ok((report, true));
        };
        if let Some(estimate) = estimate {
            fec.install(estimate);
        }
        *self.last_fec_report_at.lock().unwrap() = Some(now);
        self.fec_revision.fetch_add(1, Ordering::Release);
        let percent = fec.percent(now, self.typical_fec_shards());
        self.fec_percent.store(percent as u32, Ordering::Release);
        drop(client);
        self.follow_fec_report(first);
        Ok((report, true))
    }
    /// Data shards of an average frame at the current bitrate: statistical
    /// FEC's percentage for frames sent in parts and for pacing estimates.
    fn typical_fec_shards(&self) -> usize {
        self.fec_shards(self.typical_frame_bytes())
    }
    /// Bytes of an average frame at the current bitrate.
    fn typical_frame_bytes(&self) -> usize {
        (u64::from(self.bitrate.load(Ordering::Relaxed)) * 1000
            / 8
            / u64::from(self.config.fps.max(1))) as usize
    }
    /// Data shards of a video frame of `payload_bytes`, as the packetizer splits it.
    fn fec_shards(&self, payload_bytes: usize) -> usize {
        (payload_bytes + 8).div_ceil(self.config.packet_size.saturating_sub(16).max(1))
    }
    /// The FEC percentage for a whole video frame of `payload_bytes` about to
    /// be packetized: under `rlFecPercentage=auto` the statistical choice for
    /// its size, and for a recovery frame at least the configured percentage
    /// as [`Self::frame_fec_percent`] gives it; otherwise
    /// [`Self::frame_fec_percent`].
    pub fn video_fec_percent(&self, recovery: bool, payload_bytes: usize) -> usize {
        if self.client_fec_on.load(Ordering::Acquire) {
            let statistical = self
                .client_fec
                .lock()
                .unwrap()
                .as_ref()
                .map(|fec| fec.percent(Instant::now(), self.fec_shards(payload_bytes)));
            if let Some(percent) = statistical {
                return if recovery {
                    percent.max(self.adaptive_fec.lock().unwrap().ceiling())
                } else {
                    percent
                };
            }
        }
        self.frame_fec_percent(recovery)
    }
    /// Whether the client sizes FEC per frame from its own reports
    /// (`rlFecPercentage=auto`) rather than taking a fixed percentage.
    pub fn statistical_fec(&self) -> bool {
        self.client_fec_on.load(Ordering::Acquire)
    }
    /// Under `rlFecPercentage=auto`, the largest recovery frame whose own
    /// [`Self::video_fec_percent`] still applies in full within Moonlight's
    /// four blocks (with no parity, the four-block packet limit), as
    /// [`butterpollo_media_policy::packet::VideoPacketizer::fec_whole_payload_limit`] finds it for
    /// a fixed percentage. The statistical choice depends on the frame's size,
    /// so every size is tried, from the largest down. `None` without
    /// statistical FEC.
    pub fn statistical_fec_whole_payload(&self, now: Instant) -> Option<usize> {
        if !self.client_fec_on.load(Ordering::Acquire) {
            return None;
        }
        let slice = self.config.packet_size.saturating_sub(16).max(1);
        let minimum = self.config.min_fec;
        let ceiling = self.adaptive_fec.lock().unwrap().ceiling();
        let rev = self.fec_revision.load(Ordering::Acquire);
        let last_at = *self.last_fec_report_at.lock().unwrap();
        let fresh = last_at.is_some_and(|at| {
            now.saturating_duration_since(at)
                <= butterpollo_media_policy::statistical_fec::FEEDBACK_TIMEOUT
        });
        let settled = last_at.is_some_and(|at| {
            now.saturating_duration_since(at) >= butterpollo_media_policy::statistical_fec::WINDOW
        });
        if let Some(cached) = *self.whole_payload_cache.lock().unwrap()
            && cached.revision == rev
            && cached.ceiling == ceiling
            && cached.slice == slice
            && cached.minimum == minimum
            && cached.fresh == fresh
            && cached.settled == settled
        {
            return Some(cached.limit);
        }
        let client = self.client_fec.lock().unwrap();
        let fec = client.as_ref()?;
        let rev = self.fec_revision.load(Ordering::Acquire);
        // If loss is unset, zero, or timed out, percent(now, shards) is constant across
        // shard counts, so read it once and release the lock before searching.
        let loss = fec.loss();
        let shards = if !fresh || loss.is_none_or(|l| l <= 0.0) {
            let percent = fec.percent(now, 1).max(ceiling);
            drop(client);
            let upper = ((4 * 255) * 100 / (100 + percent)).clamp(1, 4 * 255);
            (1..=upper)
                .rev()
                .find(|&shards| {
                    butterpollo_media_policy::packet::conventional_fec(shards, percent, minimum).1
                        >= percent
                })
                .unwrap_or(1)
        } else {
            let min_pct = fec.floor().max(ceiling);
            let upper = ((4 * 255) * 100 / (100 + min_pct)).clamp(1, 4 * 255);
            let found = (1..=upper)
                .rev()
                .find(|&shards| {
                    let percent = fec.percent(now, shards).max(ceiling);
                    butterpollo_media_policy::packet::conventional_fec(shards, percent, minimum).1
                        >= percent
                })
                .unwrap_or(1);
            drop(client);
            found
        };
        let limit = (shards * slice).saturating_sub(8);
        *self.whole_payload_cache.lock().unwrap() = Some(WholePayloadCache {
            revision: rev,
            ceiling,
            slice,
            minimum,
            fresh,
            settled,
            limit,
        });
        Some(limit)
    }
    /// The FEC percentage to budget a recovery frame's parity with before it
    /// is encoded: [`Self::video_fec_percent`] for an average frame.
    pub fn recovery_fec_percent(&self) -> usize {
        if self.client_fec_on.load(Ordering::Acquire) {
            return self.video_fec_percent(true, self.typical_frame_bytes());
        }
        self.frame_fec_percent(true)
    }
    /// The FEC percentage for the next video frame.
    pub fn fec_percent(&self) -> usize {
        self.fec_percent.load(Ordering::Acquire) as usize
    }
    /// The FEC percentage for a video frame about to be packetized. A
    /// keyframe or the frame after a reference invalidation (`recovery`)
    /// repairs a loss the client is waiting on, and a keyframe is several
    /// frames' worth of packets: losing it costs another round trip and
    /// another keyframe. Such a frame gets the configured percentage, however
    /// far a clean link let adaptive FEC fall (to 5%) before the loss.
    pub fn frame_fec_percent(&self, recovery: bool) -> usize {
        let current = self.fec_percent();
        if recovery {
            current.max(self.adaptive_fec.lock().unwrap().ceiling())
        } else {
            current
        }
    }
    /// Lets adaptive FEC fall on a clean link; call regularly. Returns the
    /// `(old, new)` percentage to log, at most once a second.
    pub fn tick_fec(&self, now: Instant) -> Option<(usize, usize)> {
        if let Some(fec) = self.client_fec.lock().unwrap().as_mut() {
            // Reports that stopped hand the stream back to the host's percentage.
            let percent = fec.percent(now, self.typical_fec_shards());
            self.fec_percent.store(percent as u32, Ordering::Release);
            return fec.log_due(now, percent);
        }
        let mut fec = self.adaptive_fec.lock().unwrap();
        if !fec.enabled() {
            return None;
        }
        fec.tick(now);
        self.fec_percent
            .store(fec.percent() as u32, Ordering::Release);
        fec.log_due(now)
    }
}
