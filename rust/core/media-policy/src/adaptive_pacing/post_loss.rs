//! Post-loss wired pacing policy scoped strictly to `AdaptivePacing::new_after_loss`.

/// Policy knobs for post-loss wired handoff (used to compare candidates in `path_sim`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PostLossConfig {
    /// Optional fraction `(num, den)` of `failed_bps` capping the post-loss ceiling.
    pub(super) failed_cap: Option<(u64, u64)>,
    /// Lock `learned_ceiling = Some(probe.stable)` when a probe drops packets.
    pub(super) lock_on_probe_loss: bool,
    /// Also lock `learned_ceiling` when non-probe `PacketLoss` cuts the rate.
    pub(super) lock_on_packet_loss: bool,
    /// Cut fraction `(num, den)` on non-probe `PacketLoss`.
    pub(super) packet_loss_cut: (u64, u64),
    /// Treat a single unrecoverable keyframe as substantial loss.
    pub(super) single_keyframe_cut: bool,
    /// Minimum FEC percentage required to probe upward (`0` allows probing at any FEC).
    pub(super) min_probe_fec_percent: usize,
}

impl PostLossConfig {
    /// Bare #40 baseline: no ceiling cap, no learned ceiling lock, standard 2-frame /2 cuts.
    #[cfg(test)]
    pub(super) const BARE_40: Self = Self {
        failed_cap: None,
        lock_on_probe_loss: false,
        lock_on_packet_loss: false,
        packet_loss_cut: (1, 2),
        single_keyframe_cut: false,
        min_probe_fec_percent: 0,
    };

    /// Candidate 2 (`failed_bps / 2` cap, `ProbeLoss` lock, `old * 3 / 4` cut, 1-keyframe cut).
    #[cfg(test)]
    pub(super) const CAND_2_HALF: Self = Self {
        failed_cap: Some((1, 2)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: false,
        packet_loss_cut: (3, 4),
        single_keyframe_cut: true,
        min_probe_fec_percent: 0,
    };

    /// Candidate 2 variant (`failed_bps * 3 / 4` cap).
    #[cfg(test)]
    pub(super) const CAND_2_THREE_QUARTERS: Self = Self {
        failed_cap: Some((3, 4)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: false,
        packet_loss_cut: (3, 4),
        single_keyframe_cut: true,
        min_probe_fec_percent: 0,
    };

    /// Candidate 2 variant that also locks `learned_ceiling` on `PacketLoss`.
    #[cfg(test)]
    pub(super) const CAND_2_HALF_LOCK_PKT: Self = Self {
        failed_cap: Some((1, 2)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: true,
        packet_loss_cut: (3, 4),
        single_keyframe_cut: true,
        min_probe_fec_percent: 0,
    };

    /// Candidate 2 variant with standard `/ 2` PacketLoss cut + ProbeLoss lock.
    #[cfg(test)]
    pub(super) const CAND_2_PKT_HALF: Self = Self {
        failed_cap: Some((1, 2)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: false,
        packet_loss_cut: (1, 2),
        single_keyframe_cut: true,
        min_probe_fec_percent: 0,
    };

    /// Candidate 3 variant with `/ 2` PacketLoss cut, lock on PacketLoss/ProbeLoss, and low-FEC guard.
    #[cfg(test)]
    pub(super) const CAND_3_PKT_HALF_LOCK: Self = Self {
        failed_cap: Some((1, 2)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: true,
        packet_loss_cut: (1, 2),
        single_keyframe_cut: true,
        min_probe_fec_percent: 10,
    };

    /// Candidate 3 (`failed_bps / 2` + `fec_percent < 10` probe guard) without `PacketLoss` `/2` cut.
    #[cfg(test)]
    pub(super) const CAND_3_3Q_GUARD: Self = Self {
        failed_cap: Some((1, 2)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: false,
        packet_loss_cut: (3, 4),
        single_keyframe_cut: true,
        min_probe_fec_percent: 10,
    };

    /// Candidate 3 (`failed_bps * 3/4`, `ProbeLoss` lock, `/2` PacketLoss cut, 1-kf cut, `fec >= 10`).
    #[cfg(test)]
    pub(super) const CAND_3_3Q_PKT_HALF: Self = Self {
        failed_cap: Some((3, 4)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: false,
        packet_loss_cut: (1, 2),
        single_keyframe_cut: true,
        min_probe_fec_percent: 10,
    };

    /// Proven safest post-loss wired pacing policy: `failed_bps / 2` ceiling cap,
    /// `ProbeLoss` learned ceiling lock, `/2` PacketLoss cut, single-keyframe cut,
    /// and `fec_percent >= 10` probe guard.
    pub(super) const DEFAULT: Self = Self {
        failed_cap: Some((1, 2)),
        lock_on_probe_loss: true,
        lock_on_packet_loss: false,
        packet_loss_cut: (1, 2),
        single_keyframe_cut: true,
        min_probe_fec_percent: 10,
    };
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PostLoss {
    pub(super) config: PostLossConfig,
    failed_bps: Option<u64>,
    learned_ceiling: Option<u64>,
    fec_percent: usize,
    keyframe_failed: bool,
}

impl PostLoss {
    pub(super) fn new(failed_bps: Option<u64>, config: PostLossConfig) -> Self {
        Self {
            config,
            failed_bps: failed_bps.filter(|&bps| bps > 0),
            learned_ceiling: None,
            fec_percent: 20,
            keyframe_failed: false,
        }
    }

    pub(super) fn set_fec_percent(&mut self, fec_percent: usize) {
        self.fec_percent = fec_percent;
    }

    pub(super) fn ceiling(&self, route_ceiling: u64, needed: u64, encoder_kbps: u32) -> u64 {
        let min_floor = needed.saturating_add(needed / 4).max(1_000_000);
        let seed_floor = u64::from(encoder_kbps).saturating_mul(2000).max(min_floor);
        let mut cap = route_ceiling.max(1);
        if let (Some(failed), Some((num, den))) = (self.failed_bps, self.config.failed_cap) {
            let bounded = (failed.saturating_mul(num) / den).max(seed_floor);
            cap = cap.min(bounded);
        }
        if let Some(learned) = self.learned_ceiling {
            cap = cap.min(learned.max(min_floor));
        }
        cap.max(1)
    }

    pub(super) fn cap_ceiling(&self, ceiling: u64, floor: u64) -> u64 {
        self.learned_ceiling
            .map_or(ceiling, |learned| ceiling.min(learned.max(floor)))
            .max(1)
    }

    pub(super) fn on_failed_block(&mut self, keyframe: bool) {
        if keyframe && self.config.single_keyframe_cut {
            self.keyframe_failed = true;
        }
    }

    pub(super) fn clear_keyframe_failed(&mut self) {
        self.keyframe_failed = false;
    }

    pub(super) fn take_keyframe_failed(&mut self) -> bool {
        std::mem::take(&mut self.keyframe_failed)
    }

    pub(super) fn on_probe_loss(&mut self, stable: u64) {
        if self.config.lock_on_probe_loss {
            self.learned_ceiling = Some(self.learned_ceiling.map_or(stable, |c| c.min(stable)));
        }
    }

    pub(super) fn packet_loss_rate(&mut self, old: u64, floor: u64, ceiling: u64) -> u64 {
        let (num, den) = self.config.packet_loss_cut;
        let next = (old.saturating_mul(num) / den).clamp(floor, ceiling);
        if self.config.lock_on_packet_loss {
            self.learned_ceiling = Some(self.learned_ceiling.map_or(next, |c| c.min(next)));
        }
        next
    }

    pub(super) fn probe_allowed(&self) -> bool {
        self.fec_percent >= self.config.min_probe_fec_percent
    }

    #[cfg(test)]
    pub(super) fn learned_ceiling(&self) -> Option<u64> {
        self.learned_ceiling
    }
}
