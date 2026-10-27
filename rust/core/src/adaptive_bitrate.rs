//! Conservative, opt-in adaptation for Moonlight video streams.
//!
//! Stock Moonlight reports FEC only on loss. A validated report proves support;
//! silence alone proves nothing. Recovery needs recent standard control pings,
//! a moving, busy video stream, stable RTT and a sustained loss-free interval.
//! A client /bitrate request relinquishes host ownership until reconnection.
use std::time::{Duration, Instant};

use crate::{adaptive_fec::Block, packet::VideoPacketizer, rtsp::Negotiated};

#[derive(Clone, Copy, Default)]
pub struct Traffic {
    pub frames: u64,
    pub bytes: u64,
    pub send_losses: u64,
}

#[derive(Clone, Copy)]
pub struct WireBudget {
    pub packet_size: usize,
    pub min_fec: usize,
    pub encrypted: bool,
    pub audio_bps: u64,
    /// Records already budget wire bytes; legacy PyroWave budgets image bytes.
    pub pyrowave_records: Option<bool>,
}

impl WireBudget {
    pub fn of(stream: &Negotiated) -> Self {
        // Opus plus 4:2 audio FEC, encryption/framing and control traffic.
        // Reserve the negotiated maximum even when audio is muted/quiet.
        let audio_kbps =
            u64::from(stream.audio_channels) * if stream.audio_quality { 256 } else { 96 };
        Self {
            packet_size: stream.packet_size,
            min_fec: stream.min_fec,
            encrypted: stream.encryption & 2 != 0,
            audio_bps: audio_kbps * 1500 + 500_000,
            pyrowave_records: (stream.codec == 3).then_some(stream.pyrowave_records),
        }
    }

    pub fn video_bps(&self, kbps: u32, fec: usize, period: Duration) -> u64 {
        if let Some(records) = self.pyrowave_records {
            return crate::network_pacing::pyrowave_wire_rate_bps(
                kbps,
                self.packet_size,
                records,
                period,
            );
        }
        let packetizer = VideoPacketizer {
            sequence: 0,
            frame: 1,
            iv_counter: 0,
            packet_size: self.packet_size,
            fec_percent: fec,
            min_fec: self.min_fec,
            key: self.encrypted.then_some([0; 16]),
        };
        // IPv6 overhead is a conservative bound for both address families.
        crate::network_pacing::video_wire_rate_bps(&packetizer, kbps, period, true)
    }

    pub fn encoder_kbps(&self, total_bps: u64, ceiling: u32, fec: usize, period: Duration) -> u32 {
        // Keep 10% video headroom for rate control overshoot/keyframes. Packet
        // rounding, minimum parity, encryption and IP framing all count.
        let video = total_bps.saturating_sub(self.audio_bps) * 9 / 10;
        let (mut lo, mut hi) = (1, ceiling.max(1));
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.video_bps(mid, fec, period) <= video {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    pub(crate) fn total_bps(&self, kbps: u32, fec: usize, period: Duration) -> u64 {
        // Round up so converting this wire allowance back to an encoder
        // bitrate cannot lose the last Kbps of a minimum viable frame.
        (self.video_bps(kbps, fec, period) * 10).div_ceil(9) + self.audio_bps
    }

    pub(crate) fn minimum_encoder_kbps(
        &self,
        ceiling: u32,
        fec: usize,
        period: Duration,
    ) -> Option<u32> {
        if ceiling == 0 {
            return None;
        }
        let Some(records) = self.pyrowave_records else {
            return Some(1);
        };
        // PyroWave skips encoding below 16 image bytes. Records must first
        // pay for whole wire packets and their minimum critical parity;
        // legacy framing also reserves parity inside its picture allowance.
        let fits = |kbps| {
            crate::pyrowave::budget_with_minimum(
                kbps,
                period,
                self.packet_size,
                records,
                fec.min(255) as u32,
                self.min_fec,
            ) >= 16
        };
        if !fits(ceiling) {
            return None;
        }
        let (mut lo, mut hi) = (1, ceiling);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if fits(mid) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        Some(lo)
    }
}

#[derive(Default)]
pub struct AdaptiveBitrate {
    budget: Option<WireBudget>,
    client_owned: bool,
    supported: bool,
    viable: bool,
    controller_pcm_bps: u64,
    /// The encoder reserves configured PyroWave parity even after the
    /// sender's adaptive FEC percentage falls on a clean link.
    encoder_fec: usize,
    minimum_kbps: u32,
    ceiling_kbps: u32,
    ceiling_bps: u64,
    current_bps: u64,
    floor_bps: u64,
    last_ping: Option<Instant>,
    last_sample: Option<(Instant, Traffic)>,
    last_change: Option<Instant>,
    clean_since: Option<Instant>,
    minimum_rtt: Option<Duration>,
    queued_loss: bool,
    severe_loss: bool,
    congested_samples: u8,
    reason: &'static str,
}

impl AdaptiveBitrate {
    pub fn configure(&mut self, enabled: bool, stream: &Negotiated, maximum_kbps: u32, fec: usize) {
        if !enabled {
            self.budget = None;
            return;
        }
        let mut budget = WireBudget::of(stream);
        budget.audio_bps += self.controller_pcm_bps;
        let period = crate::reconfigure::Mode::of(stream).period();
        let inferred = budget.total_bps(stream.bitrate_kbps, fec, period);
        let requested = if stream.configured_bitrate_kbps > 0 {
            u64::from(stream.configured_bitrate_kbps) * 1000
        } else {
            inferred
        };
        self.ceiling_bps = requested.min(if maximum_kbps > 0 {
            u64::from(maximum_kbps) * 1000
        } else {
            u64::MAX
        });
        if stream.codec == 3 {
            for ceiling in [
                u64::from(stream.pyrowave_client_pace_mbps) * 1_000_000,
                u64::from(stream.pyrowave_client_link_mbps) * 950_000,
            ] {
                if ceiling > 0 {
                    self.ceiling_bps = self.ceiling_bps.min(ceiling);
                }
            }
        }
        self.ceiling_kbps = stream.bitrate_kbps.min(if stream.codec == 3 {
            2_000_000
        } else {
            500_000
        });
        self.current_bps = self.ceiling_bps;
        self.encoder_fec = fec;
        self.budget = Some(budget);
        self.reason = "waiting_for_feedback";
        self.update_floor(fec, period);
        if self.client_owned {
            self.reason = "client_controlled";
        }
    }

    fn update_floor(&mut self, fec: usize, period: Duration) {
        let budget = self.budget.unwrap();
        // Require both a usable encoder frame and room for its wire traffic.
        // Do not raise a user ceiling when even the smallest frame cannot fit.
        let minimum = budget.minimum_encoder_kbps(self.ceiling_kbps, self.encoder_fec, period);
        self.viable =
            minimum.is_some_and(|kbps| budget.total_bps(kbps, fec, period) <= self.ceiling_bps);
        if self.viable {
            self.minimum_kbps = minimum.unwrap();
            self.floor_bps = budget
                .total_bps(
                    self.ceiling_kbps.min(2_000).max(self.minimum_kbps),
                    fec,
                    period,
                )
                .min(self.ceiling_bps);
            self.current_bps = self.current_bps.max(self.floor_bps).min(self.ceiling_bps);
        } else {
            self.minimum_kbps = 0;
            self.reason = "wire_budget_too_small";
            self.clean_since = None;
        }
    }

    pub fn budget_unusable(&self) -> bool {
        self.configured() && !self.viable
    }

    pub fn reserve_controller_pcm(&mut self, streams: usize, fec: usize, period: Duration) {
        let next = streams.min(48) as u64 * crate::controller_audio::PCM_WIRE_BPS;
        if next == self.controller_pcm_bps {
            return;
        }
        if let Some(budget) = &mut self.budget {
            budget.audio_bps = budget.audio_bps - self.controller_pcm_bps + next;
        }
        self.controller_pcm_bps = next;
        if self.configured() {
            self.update_floor(fec, period);
        }
    }

    /// The client's traffic takes another network path (a roam): the old path's
    /// minimum RTT, congestion and losses say nothing about it. A 2 ms Wi-Fi baseline
    /// would otherwise count every sample of a 40 ms mobile path as queued.
    pub fn new_path(&mut self) {
        self.minimum_rtt = None;
        self.congested_samples = 0;
        self.clean_since = None;
        self.last_change = None;
        self.queued_loss = false;
        self.severe_loss = false;
    }

    /// The lowest RTT seen on this path.
    #[cfg(test)]
    pub(crate) fn minimum_rtt(&self) -> Option<Duration> {
        self.minimum_rtt
    }

    pub fn client_takeover(&mut self) {
        self.client_owned = true;
        self.reason = "client_controlled";
    }

    pub fn on_ping(&mut self, now: Instant, payload: &[u8]) {
        if payload.len() == 8 && payload[..4] == 4u32.to_le_bytes() {
            self.last_ping = Some(now);
        }
    }

    /// Only call after wire validation, sent-frame checks and deduplication.
    pub fn on_block(&mut self, block: Block) {
        self.supported = true;
        match block {
            Block::Unrecoverable => {
                self.queued_loss = true;
                self.severe_loss = true;
            }
            Block::Recovered { lost, data } if u32::from(lost) * 100 >= u32::from(data) * 2 => {
                self.queued_loss = true;
            }
            Block::Recovered { lost, .. } if lost > 0 => self.clean_since = None,
            _ => {}
        }
    }

    pub fn configured(&self) -> bool {
        self.budget.is_some() && !self.client_owned
    }

    pub fn active(&self) -> bool {
        self.configured() && self.supported && self.viable
    }

    /// Video pacing ceiling, after reserving audio/control. Zero means leave
    /// the existing pacing policy alone (including clients without reports).
    pub fn video_pacing_bps(&self) -> u64 {
        if !self.active() {
            return 0;
        }
        self.current_bps
            .saturating_sub(self.budget.unwrap().audio_bps)
            .max(1)
    }

    /// At most two samples/second. Returns a target even when only the FEC
    /// percentage changed, so recovery packets cannot silently grow the budget.
    pub fn tick(
        &mut self,
        now: Instant,
        traffic: Traffic,
        rtt: Duration,
        fec: usize,
        period: Duration,
        current_kbps: u32,
    ) -> Option<u32> {
        if !self.configured() {
            return None;
        }
        let Some((previous, before)) = self.last_sample else {
            self.last_sample = Some((now, traffic));
            return None;
        };
        let elapsed = now.saturating_duration_since(previous);
        if elapsed < Duration::from_millis(500) {
            return None;
        }
        self.last_sample = Some((now, traffic));
        if !self.supported {
            return None;
        }
        self.update_floor(fec, period);
        if !self.viable {
            return None;
        }
        let ping = self
            .last_ping
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(2));
        let seconds = elapsed.as_secs_f64();
        let busy = elapsed <= Duration::from_secs(2)
            && traffic.frames.saturating_sub(before.frames) as f64
                >= seconds / period.as_secs_f64() * 0.6
            && traffic.bytes.saturating_sub(before.bytes) as f64 * 8.
                >= f64::from(current_kbps) * 1000. * seconds * 0.6;
        if ping && !rtt.is_zero() {
            self.minimum_rtt = Some(self.minimum_rtt.map_or(rtt, |minimum| minimum.min(rtt)));
        }
        let queued = !rtt.is_zero()
            && self
                .minimum_rtt
                .is_some_and(|minimum| rtt > minimum + (minimum / 2).max(Duration::from_millis(8)));
        self.congested_samples = if ping && busy && queued {
            self.congested_samples.saturating_add(1)
        } else {
            0
        };
        let loss =
            std::mem::take(&mut self.queued_loss) || traffic.send_losses > before.send_losses;
        let severe = std::mem::take(&mut self.severe_loss);
        let due = self
            .last_change
            .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(1));
        let mut recovering = false;
        if loss || self.congested_samples >= 2 {
            self.clean_since = None;
            if due {
                self.current_bps =
                    (self.current_bps * if severe { 75 } else { 85 } / 100).max(self.floor_bps);
                self.last_change = Some(now);
                self.reason = if loss { "packet_loss" } else { "rising_rtt" };
            }
        } else if ping && busy && !queued && !rtt.is_zero() {
            let clean = self.clean_since.get_or_insert(now);
            if now.saturating_duration_since(*clean) >= Duration::from_secs(8)
                && self
                    .last_change
                    .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(2))
            {
                self.current_bps =
                    (self.current_bps + (self.current_bps / 50).max(100_000)).min(self.ceiling_bps);
                self.last_change = Some(now);
                self.reason = "recovering";
                recovering = true;
            }
        } else {
            self.clean_since = None;
            self.reason = if !ping {
                "feedback_stale"
            } else if rtt.is_zero() {
                "rtt_unavailable"
            } else {
                "low_activity"
            };
        }
        let target =
            self.budget
                .unwrap()
                .encoder_kbps(self.current_bps, self.ceiling_kbps, fec, period);
        // A falling FEC percentage must not bypass the slow recovery policy.
        Some(
            (if recovering {
                target.min(current_kbps.saturating_add((current_kbps / 50).max(100)))
            } else {
                target.min(current_kbps)
            })
            // A higher FPS can require more bitrate just to produce a frame.
            // Allow this bounded increase without waiting for busy clean traffic
            // that a zero-frame encoder could never provide.
            .max(self.minimum_kbps),
        )
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "configured": self.budget.is_some(),
            "active": self.active(),
            "owner": if self.client_owned { "client" } else if self.active() { "host" } else { "fixed" },
            "reason": if self.reason.is_empty() { "disabled" } else { self.reason },
            "feedback_observed": self.supported,
            "wire_budget_kbps": self.current_bps / 1000,
            "wire_ceiling_kbps": self.ceiling_bps / 1000,
            "controller_pcm_reserved_kbps": self.controller_pcm_bps / 1000,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> AdaptiveBitrate {
        let mut c = AdaptiveBitrate::default();
        let stream = Negotiated {
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 30_000,
            ..Default::default()
        };
        c.configure(true, &stream, 0, 20);
        c
    }
    fn run(
        c: &mut AdaptiveBitrate,
        start: Instant,
        rounds: u64,
        busy: bool,
        ping: bool,
        fec: usize,
    ) -> u32 {
        let mut kbps = 20_000;
        let mut bytes = 0;
        for i in 0..rounds {
            let now = start + Duration::from_secs(i);
            bytes += if busy { u64::from(kbps) * 125 } else { 0 };
            if ping {
                c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            }
            if let Some(target) = c.tick(
                now,
                Traffic {
                    frames: i * if busy { 60 } else { 1 },
                    bytes,
                    send_losses: 0,
                },
                Duration::from_millis(8),
                fec,
                Duration::from_secs_f64(1. / 60.),
                kbps,
            ) {
                kbps = target;
            }
        }
        kbps
    }
    #[test]
    fn no_feedback_and_client_ownership_leave_fixed_bitrate_untouched() {
        let mut c = controller();
        assert_eq!(run(&mut c, Instant::now(), 60, true, true, 20), 20_000);
        assert_eq!(c.video_pacing_bps(), 0);
        c.on_block(Block::Unrecoverable);
        c.client_takeover();
        assert_eq!(run(&mut c, Instant::now(), 60, true, true, 20), 20_000);
        assert_eq!(c.video_pacing_bps(), 0);
    }
    #[test]
    fn losses_reduce_budget_then_busy_clean_streams_recover_slowly() {
        let start = Instant::now();
        let mut c = controller();
        c.on_block(Block::Unrecoverable);
        let low = run(&mut c, start, 2, true, true, 20);
        assert!(low < 20_000);
        let low_budget = c.current_bps;
        let restored = run(&mut c, start + Duration::from_secs(3), 90, true, true, 20);
        assert!(restored > low);
        assert!(c.current_bps > low_budget);
        assert!(c.current_bps <= c.ceiling_bps);
    }
    #[test]
    fn idle_video_and_stale_control_never_count_as_a_clean_link() {
        for (busy, ping) in [(false, true), (true, false), (false, false)] {
            let mut c = controller();
            c.on_block(Block::Unrecoverable);
            run(&mut c, Instant::now(), 2, true, true, 20);
            let budget = c.current_bps;
            run(
                &mut c,
                Instant::now() + Duration::from_secs(10),
                120,
                busy,
                ping,
                5,
            );
            assert_eq!(c.current_bps, budget);
        }
    }
    #[test]
    fn fec_rounding_audio_encryption_and_headers_fit_the_same_wire_ceiling() {
        for size in [256, 1024, 1400] {
            for fec in [0, 5, 20, 50, 100] {
                let stream = Negotiated {
                    packet_size: size,
                    min_fec: 2,
                    encryption: 2,
                    audio_channels: 8,
                    audio_quality: true,
                    ..Default::default()
                };
                let budget = WireBudget::of(&stream);
                let period = Duration::from_secs_f64(1. / 120.);
                let rate = budget.encoder_kbps(30_000_000, 100_000, fec, period);
                assert!(budget.video_bps(rate, fec, period) + budget.audio_bps <= 30_000_000);
            }
        }
    }
    /// Busy, pinged rounds at `rtt` from round `from`.
    fn steady(c: &mut AdaptiveBitrate, start: Instant, from: u64, rounds: u64, rtt: u64) {
        let mut rate = 20_000;
        for i in from..from + rounds {
            let now = start + Duration::from_secs(i);
            c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            rate = c
                .tick(
                    now,
                    Traffic {
                        frames: i * 60,
                        bytes: i * 2_500_000,
                        send_losses: 0,
                    },
                    Duration::from_millis(rtt),
                    20,
                    Duration::from_secs_f64(1. / 60.),
                    rate,
                )
                .unwrap_or(rate);
        }
    }
    #[test]
    fn a_new_path_learns_its_own_minimum_rtt() {
        let start = Instant::now();
        for roam in [false, true] {
            let mut c = controller();
            c.on_block(Block::Clean);
            steady(&mut c, start, 0, 5, 2);
            assert_eq!(c.current_bps, c.ceiling_bps);
            if roam {
                c.new_path();
            }
            steady(&mut c, start, 5, 10, 40);
            if roam {
                assert_eq!(
                    c.current_bps, c.ceiling_bps,
                    "a steady 40 ms path is not congested"
                );
            } else {
                assert!(
                    c.current_bps < c.ceiling_bps,
                    "40 ms over a 2 ms baseline is queueing"
                );
                assert_eq!(c.reason, "rising_rtt");
            }
        }
    }
    #[test]
    fn rtt_requires_sustained_congestion_and_unknown_rtt_does_not_poison_baseline() {
        let start = Instant::now();
        let mut c = controller();
        c.on_block(Block::Clean);
        let mut rate = 20_000;
        for (i, rtt) in [0, 8, 50, 8, 50, 50].into_iter().enumerate() {
            let now = start + Duration::from_secs(i as u64);
            c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            rate = c
                .tick(
                    now,
                    Traffic {
                        frames: i as u64 * 60,
                        bytes: i as u64 * 2_500_000,
                        send_losses: 0,
                    },
                    Duration::from_millis(rtt),
                    20,
                    Duration::from_secs_f64(1. / 60.),
                    rate,
                )
                .unwrap_or(rate);
            if i < 5 {
                assert_eq!(c.current_bps, c.ceiling_bps);
            }
        }
        assert!(c.current_bps < c.ceiling_bps);
        assert_eq!(c.reason, "rising_rtt");
    }
    #[test]
    fn configured_wire_cap_applies_with_parity_and_no_recovery_from_just_lower_fec() {
        let mut c = controller();
        let stream = Negotiated {
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 40_000,
            ..Default::default()
        };
        c.configure(true, &stream, 25_000, 20);
        c.on_block(Block::Unrecoverable);
        let start = Instant::now();
        let low = run(&mut c, start, 2, true, true, 20);
        assert!(c.current_bps < 25_000_000);
        let held = c
            .tick(
                start + Duration::from_secs(3),
                Traffic::default(),
                Duration::ZERO,
                0,
                Duration::from_secs_f64(1. / 60.),
                low,
            )
            .unwrap();
        assert_eq!(held, low);
        assert!(c.video_pacing_bps() + c.budget.unwrap().audio_bps <= 25_000_000);
    }
    #[test]
    fn disabled_streams_never_activate() {
        for (enabled, codec) in [(false, 0), (false, 3)] {
            let mut c = AdaptiveBitrate::default();
            c.configure(
                enabled,
                &Negotiated {
                    codec,
                    ..Default::default()
                },
                0,
                20,
            );
            c.on_block(Block::Unrecoverable);
            assert!(!c.active());
            assert_eq!(c.video_pacing_bps(), 0);
        }
    }
    #[test]
    fn a_budget_smaller_than_audio_and_minimum_packets_never_stalls_the_pacer() {
        let mut c = controller();
        let stream = Negotiated {
            bitrate_kbps: 500,
            configured_bitrate_kbps: 1_000,
            audio_channels: 8,
            audio_quality: true,
            ..Default::default()
        };
        c.configure(true, &stream, 0, 20);
        c.on_block(Block::Unrecoverable);
        assert!(c.budget_unusable());
        let start = Instant::now();
        for i in 0..4 {
            assert_eq!(
                c.tick(
                    start + Duration::from_secs(i),
                    Traffic::default(),
                    Duration::from_millis(8),
                    20,
                    Duration::from_secs_f64(1. / 60.),
                    500
                ),
                None
            );
        }
        assert_eq!(c.video_pacing_bps(), 0);
        assert_eq!(c.snapshot()["reason"], "wire_budget_too_small");
    }
    #[test]
    fn higher_fps_revalidates_the_minimum_wire_budget() {
        let mut c = controller();
        let stream = Negotiated {
            bitrate_kbps: 2_000,
            configured_bitrate_kbps: 6_000,
            audio_channels: 8,
            audio_quality: true,
            packet_size: 1024,
            min_fec: 2,
            ..Default::default()
        };
        c.configure(true, &stream, 0, 20);
        c.on_block(Block::Clean);
        assert!(c.active());
        let start = Instant::now();
        for i in 0..2 {
            c.tick(
                start + Duration::from_secs(i),
                Traffic::default(),
                Duration::from_millis(8),
                20,
                Duration::from_secs_f64(1. / 240.),
                2_000,
            );
        }
        assert!(!c.active());
        assert_eq!(c.video_pacing_bps(), 0);
    }

    #[test]
    fn pyrowave_loss_floor_keeps_frames_encodable_with_configured_and_minimum_parity() {
        for records in [false, true] {
            for fps in [60, 120] {
                for fec in [0, 20, 255] {
                    for min_fec in [0, 2, 8] {
                        let stream = Negotiated {
                            codec: 3,
                            pyrowave_records: records,
                            packet_size: 1392,
                            min_fec,
                            fps,
                            bitrate_kbps: 500_000,
                            configured_bitrate_kbps: 800_000,
                            ..Default::default()
                        };
                        let mut c = AdaptiveBitrate::default();
                        c.configure(true, &stream, 0, fec);
                        let period = crate::reconfigure::Mode::of(&stream).period();
                        let start = Instant::now();
                        let mut rate = stream.bitrate_kbps;
                        for i in 0..64 {
                            c.on_block(Block::Unrecoverable);
                            rate = c
                                .tick(
                                    start + Duration::from_secs(i),
                                    Traffic::default(),
                                    Duration::from_millis(8),
                                    // A lower sender FEC percentage does not
                                    // change the encoder's configured reserve.
                                    0,
                                    period,
                                    rate,
                                )
                                .unwrap_or(rate);
                            assert!(rate <= stream.bitrate_kbps);
                            assert!(c.current_bps <= c.ceiling_bps);
                            assert!(
                                crate::pyrowave::budget_with_minimum(
                                    rate,
                                    period,
                                    stream.packet_size,
                                    records,
                                    fec as u32,
                                    min_fec,
                                ) >= 16,
                                "records={records}, fps={fps}, fec={fec}, minimum={min_fec}, rate={rate}"
                            );
                        }
                        assert!(c.active());
                        assert_eq!(c.current_bps, c.floor_bps);
                    }
                }
            }
        }
    }

    #[test]
    fn pyrowave_minimum_frame_never_raises_an_impossible_user_ceiling() {
        for fps in [60, 120] {
            for (bitrate_kbps, configured_bitrate_kbps, maximum_kbps) in [
                (2_000, 30_000, 0),
                (20_000, 2_000, 0),
                (20_000, 30_000, 2_000),
            ] {
                let stream = Negotiated {
                    codec: 3,
                    pyrowave_records: true,
                    packet_size: 1392,
                    min_fec: 2,
                    fps,
                    bitrate_kbps,
                    configured_bitrate_kbps,
                    ..Default::default()
                };
                let mut c = AdaptiveBitrate::default();
                c.configure(true, &stream, maximum_kbps, 20);
                c.on_block(Block::Unrecoverable);
                assert!(c.budget_unusable());
                let ceiling = c.ceiling_bps;
                let start = Instant::now();
                for i in 0..4 {
                    assert_eq!(
                        c.tick(
                            start + Duration::from_secs(i),
                            Traffic::default(),
                            Duration::from_millis(8),
                            0,
                            crate::reconfigure::Mode::of(&stream).period(),
                            bitrate_kbps,
                        ),
                        None
                    );
                }
                assert_eq!(c.current_bps, ceiling);
                assert_eq!(c.video_pacing_bps(), 0);
                assert_eq!(c.snapshot()["reason"], "wire_budget_too_small");
            }
        }
    }

    #[test]
    fn higher_pyrowave_fps_can_raise_the_frame_minimum_without_clean_traffic() {
        let stream = Negotiated {
            codec: 3,
            pyrowave_records: true,
            packet_size: 1392,
            min_fec: 2,
            fps: 60,
            bitrate_kbps: 20_000,
            configured_bitrate_kbps: 30_000,
            ..Default::default()
        };
        let mut c = AdaptiveBitrate::default();
        c.configure(true, &stream, 0, 20);
        let start = Instant::now();
        let period = crate::reconfigure::Mode::of(&stream).period();
        let mut rate = stream.bitrate_kbps;
        for i in 0..32 {
            c.on_block(Block::Unrecoverable);
            rate = c
                .tick(
                    start + Duration::from_secs(i),
                    Traffic::default(),
                    Duration::ZERO,
                    0,
                    period,
                    rate,
                )
                .unwrap_or(rate);
        }
        let faster = Duration::from_secs_f64(1. / 120.);
        assert!(crate::pyrowave::budget(rate, faster, 1392, true, 20) < 16);
        let next = c
            .tick(
                start + Duration::from_secs(33),
                Traffic::default(),
                Duration::ZERO,
                0,
                faster,
                rate,
            )
            .unwrap();
        assert!(next > rate);
        assert!(next <= stream.bitrate_kbps);
        assert!(crate::pyrowave::budget(next, faster, 1392, true, 20) >= 16);
        assert!(c.current_bps <= c.ceiling_bps);
    }

    #[test]
    fn missing_rtt_never_counts_as_stable_latency_for_recovery() {
        let mut c = controller();
        c.on_block(Block::Unrecoverable);
        let start = Instant::now();
        let rate = run(&mut c, start, 2, true, true, 20);
        let budget = c.current_bps;
        for i in 2..60 {
            let now = start + Duration::from_secs(i);
            c.on_ping(now, &[4, 0, 0, 0, 0, 0, 0, 0]);
            c.tick(
                now,
                Traffic {
                    frames: i * 60,
                    bytes: i * u64::from(rate) * 125,
                    send_losses: 0,
                },
                Duration::ZERO,
                20,
                Duration::from_secs_f64(1. / 60.),
                rate,
            );
        }
        assert_eq!(c.current_bps, budget);
        assert_eq!(c.reason, "rtt_unavailable");
    }
    #[test]
    fn controller_pcm_shares_the_budget_and_released_reserves_recover_slowly() {
        let mut c = controller();
        c.on_block(Block::Clean);
        let start = Instant::now();
        let original = run(&mut c, start, 2, true, true, 20);
        let pacing = c.video_pacing_bps();
        let period = Duration::from_secs_f64(1. / 60.);
        c.reserve_controller_pcm(3, 20, period);
        assert_eq!(pacing - c.video_pacing_bps(), 6_000_000);
        let reduced = c
            .tick(
                start + Duration::from_secs(2),
                Traffic::default(),
                Duration::from_millis(8),
                20,
                period,
                original,
            )
            .unwrap();
        assert!(reduced < original);
        assert!(
            c.budget.unwrap().video_bps(reduced, 20, period) + c.budget.unwrap().audio_bps
                <= c.ceiling_bps
        );
        c.reserve_controller_pcm(0, 20, period);
        let held = c
            .tick(
                start + Duration::from_secs(3),
                Traffic::default(),
                Duration::from_millis(8),
                20,
                period,
                reduced,
            )
            .unwrap();
        assert_eq!(held, reduced);
        c.reserve_controller_pcm(usize::MAX, 20, period);
        assert_eq!(c.controller_pcm_bps, 96_000_000);
        assert_eq!(c.video_pacing_bps(), 0);
        c.reserve_controller_pcm(0, 20, period);
        assert!(c.active());
        c.client_takeover();
        c.reserve_controller_pcm(48, 20, period);
        assert_eq!(c.video_pacing_bps(), 0);
        assert_eq!(c.reason, "client_controlled");
        assert!(!c.budget_unusable());
    }
}
