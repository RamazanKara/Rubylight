//! Send ledger, client arrival clock unwrapping, report continuity, and session control.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{
    Change, Congestion, Feedback, MAX_ENTRIES, MAX_GAP_REPORTS, PacketResult, STALE_US,
    TransportFeedback,
};

/// The host's record of each video packet it sent, by packet id: one store per packet, no
/// lock, written by the sender and read by the control thread. Off until [`Self::enable`],
/// and then 64 Ki slots (512 KiB) per stream.
pub struct SendLedger {
    started: Instant,
    live: AtomicBool,
    encrypted: AtomicBool,
    /// Per id: `(sent µs since started + 1, wrapping) << 32 | wire bytes`; 0 for none.
    slots: OnceLock<Box<[AtomicU64]>>,
}

impl SendLedger {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            live: AtomicBool::new(false),
            encrypted: AtomicBool::new(false),
            slots: OnceLock::new(),
        }
    }

    /// Starts recording; `encrypted` says where the packet id is (see
    /// [`rubylight_protocol::transport_feedback::packet_id`]).
    pub fn enable(&self, encrypted: bool) {
        self.slots
            .get_or_init(|| (0..=u16::MAX).map(|_| AtomicU64::new(0)).collect());
        self.encrypted.store(encrypted, Ordering::Relaxed);
        self.live.store(true, Ordering::Release);
    }

    pub fn disable(&self) {
        self.live.store(false, Ordering::Release);
    }

    pub fn live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// µs since the ledger started.
    pub fn micros(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.started).as_micros() as u64
    }

    /// Packets the sender handed to the socket at `at`, each with `overhead` bytes of IP and
    /// UDP framing on top. Call from one thread at a time.
    pub fn record<T: AsRef<[u8]>>(&self, packets: &[T], overhead: usize, at: Instant) {
        if !self.live.load(Ordering::Acquire) {
            return;
        }
        let Some(slots) = self.slots.get() else {
            return;
        };
        let encrypted = self.encrypted.load(Ordering::Relaxed);
        let stamp = (self.micros(at) as u32).wrapping_add(1).max(1);
        for packet in packets {
            let packet = packet.as_ref();
            let Some(id) = rubylight_protocol::transport_feedback::packet_id(packet, encrypted)
            else {
                continue;
            };
            let bytes = (packet.len() + overhead).min(u32::MAX as usize) as u64;
            slots[usize::from(id)].store(u64::from(stamp) << 32 | bytes, Ordering::Relaxed);
        }
    }

    /// The send time (µs since started) and wire bytes of packet `id`, read at `now_us`; `None`
    /// when none was recorded or the record is older than [`STALE_US`] (an id from a wrap ago).
    pub fn lookup(&self, id: u16, now_us: u64) -> Option<(u64, u32)> {
        let word = self.slots.get()?[usize::from(id)].load(Ordering::Relaxed);
        if word == 0 {
            return None;
        }
        let stamp = (word >> 32) as u32;
        let now_stamp = (now_us as u32).wrapping_add(1);
        let delta = stamp.wrapping_sub(now_stamp);
        // A send thread can stamp `at` slightly after the feedback thread read `now`.
        let age = if delta <= 50_000 {
            0
        } else {
            now_stamp.wrapping_sub(stamp)
        };
        if u64::from(age) > STALE_US {
            return None;
        }
        Some((now_us.saturating_sub(u64::from(age)), word as u32))
    }
}

/// The client's arrival clock, unwrapped from the reports' 32-bit base.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClientClock {
    last: Option<(u32, u64)>,
}

impl ClientClock {
    /// The report's base arrival as a 64-bit µs count, continuing the previous ones.
    pub fn unwrap(&mut self, base: u32) -> u64 {
        let value = match self.last {
            None => u64::from(base),
            Some((raw, value)) => {
                let delta = base.wrapping_sub(raw) as i32;
                value.saturating_add_signed(i64::from(delta))
            }
        };
        self.last = Some((base, value));
        value
    }
}

/// The expected next id after a report, for counting a skip as loss.
#[derive(Clone, Copy, Debug, Default)]
pub struct Continuity {
    next: Option<u16>,
}

impl Continuity {
    /// Packets skipped before `report` that count as lost: none for a continuous report or a
    /// long jump (a stall or renumbering).
    pub fn skipped(&mut self, report: &TransportFeedback) -> u32 {
        let skipped = match self.next {
            Some(next)
                if report.flags & rubylight_protocol::transport_feedback::FLAG_SKIPPED != 0 =>
            {
                let gap = u32::from(report.base.wrapping_sub(next));
                let stride = u32::from(report.stride.max(1));
                let limit = (MAX_GAP_REPORTS * MAX_ENTRIES) as u32 * stride;
                if gap < limit { gap } else { 0 }
            }
            _ => 0,
        };
        self.next = Some(report.next_id());
        skipped
    }
}

/// Encoder kbps the controller never goes below on H.264, HEVC and AV1; PyroWave stops at its
/// test-scene detail level, as the link-rate controller does.
pub const FLOOR_KBPS: u32 = 2_000;

/// The controller with the session's bookkeeping around it, behind one mutex: the joins of a
/// report with the [`SendLedger`], the conversion between wire and encoder rates, and the
/// combination with the link-rate controller's limit.
///
/// While it is on, the session writes the encoder bitrate only through [`Control::combine`]:
/// the lower of the two controllers' limits, and the bitrate from before either when both let
/// go. In shadow mode (`BUTTERPOLLO_CONGESTION_SHADOW=1`, for the A/B) it measures and logs
/// everything, but its own limit and pace are never applied.
#[derive(Default)]
pub struct Control {
    cc: Option<Congestion>,
    shadow: bool,
    clock: ClientClock,
    continuity: Continuity,
    last_seq: Option<u32>,
    buffer: Vec<PacketResult>,
    budget: Option<crate::adaptive_bitrate::WireBudget>,
    pub(super) ceiling_kbps: u32,
    pub(super) floor_kbps: u32,
    /// What the link-rate controller wants, encoder kbps; `None` for no limit.
    pub link_wanted: Option<u32>,
    /// What this controller wants, encoder kbps.
    pub(super) wanted: Option<u32>,
    /// The bitrate from before either limit.
    restore: Option<u32>,
    /// The client set the bitrate: off for the rest of the stream, also when it is rebuilt.
    taken: bool,
}

impl Control {
    /// A controller for `stream`; `maximum_kbps` is the user's `max_bitrate` (0: none). Off when
    /// `enabled` is false.
    pub fn new(
        stream: &crate::rtsp::Negotiated,
        enabled: bool,
        maximum_kbps: u32,
        shadow: bool,
    ) -> Self {
        if !enabled {
            return Self::default();
        }
        let ceiling = stream
            .bitrate_kbps
            .min(if maximum_kbps > 0 {
                maximum_kbps
            } else {
                u32::MAX
            })
            .max(1);
        let floor = if stream.codec == 3 {
            let lowest = crate::link_rate::pyrowave_floor_kbps(stream);
            if lowest >= ceiling {
                (ceiling / 2).max(1)
            } else {
                lowest
            }
        } else {
            FLOOR_KBPS.min(ceiling)
        };
        let budget = crate::adaptive_bitrate::WireBudget::of(stream);
        let period = crate::reconfigure::Mode::of(stream).period();
        let (min, max) = Self::bounds(&budget, floor, ceiling, 20, period);
        let mut cc = Congestion::new(min, max);
        cc.set_frame_period(period);
        Self {
            cc: Some(cc),
            shadow,
            budget: Some(budget),
            ceiling_kbps: ceiling,
            floor_kbps: floor,
            ..Self::default()
        }
    }

    /// Video wire bps of the floor and the ceiling, with the headroom
    /// [`WireBudget::encoder_kbps`](crate::adaptive_bitrate::WireBudget::encoder_kbps) takes
    /// back, so the ceiling converts back to the requested bitrate.
    pub(super) fn bounds(
        budget: &crate::adaptive_bitrate::WireBudget,
        floor: u32,
        ceiling: u32,
        fec: usize,
        period: Duration,
    ) -> (u64, u64) {
        let video = |kbps| {
            budget
                .total_bps(kbps, fec, period)
                .saturating_sub(budget.audio_bps)
        };
        (video(floor), video(ceiling))
    }

    /// Whether the controller runs for this stream (also in shadow mode).
    pub fn active(&self) -> bool {
        self.cc.is_some()
    }

    pub fn shadow(&self) -> bool {
        self.shadow
    }

    /// A controller for the stream after a change of mode or bitrate; a client takeover stays.
    pub fn rebuild(
        &self,
        stream: &crate::rtsp::Negotiated,
        enabled: bool,
        maximum_kbps: u32,
        shadow: bool,
    ) -> Self {
        if self.taken {
            return Self {
                taken: true,
                ..Self::default()
            };
        }
        Self::new(stream, enabled, maximum_kbps, shadow)
    }

    /// The client set the bitrate itself: it owns it from now on.
    pub fn takeover(&mut self) {
        self.taken = true;
        self.cc = None;
        self.wanted = None;
        self.link_wanted = None;
        self.restore = None;
    }

    /// The bitrate from before either limit, taken for a rebuilt stream.
    pub fn take_restore(&mut self) -> Option<u32> {
        self.restore.take()
    }

    /// The bitrate from before either limit, while one is in force.
    pub fn limited_from(&self) -> Option<u32> {
        self.restore
    }

    /// One 0x5547 payload at host time `now_us`, joined with `ledger`. Returns whether it was
    /// a valid report.
    pub fn on_report(&mut self, ledger: &SendLedger, now_us: u64, payload: &[u8]) -> bool {
        let Some(cc) = self.cc.as_mut() else {
            return false;
        };
        let Some(report) = TransportFeedback::decode(payload) else {
            return false;
        };
        if self
            .last_seq
            .is_some_and(|prev| report.seq == prev || report.seq.wrapping_sub(prev) >= 0x8000_0000)
        {
            return false;
        }
        self.last_seq = Some(report.seq);
        let base = self.clock.unwrap(report.base_arrival_us);
        let skipped = self.continuity.skipped(&report);
        self.buffer.clear();
        let mut unmatched = 0;
        for (id, offset) in report.packets() {
            match ledger.lookup(id, now_us) {
                Some((sent_us, wire_bytes)) => self.buffer.push(PacketResult {
                    sent_us,
                    arrival_us: offset.map(|offset| base.saturating_add(offset)),
                    wire_bytes,
                }),
                None => unmatched += 1,
            }
        }
        cc.on_unmatched(unmatched * u64::from(report.stride));
        cc.on_feedback(
            now_us,
            Feedback {
                packets: &self.buffer,
                stride: report.stride,
                late: report.late,
                skipped,
            },
            payload.len(),
        );
        true
    }

    /// Runs the rate control. Returns the pace to apply, bps (`None`: the host's own).
    pub fn tick(
        &mut self,
        now_us: u64,
        rtt: Duration,
        fec: usize,
        period: Duration,
    ) -> Option<u64> {
        let (Some(cc), Some(budget)) = (self.cc.as_mut(), self.budget.as_ref()) else {
            return None;
        };
        let (min, max) = Self::bounds(budget, self.floor_kbps, self.ceiling_kbps, fec, period);
        cc.set_bounds(min, max);
        cc.set_frame_period(period);
        let decision = cc.tick(now_us, rtt, fec.min(u32::MAX as usize) as u32);
        let (ceiling, floor) = (self.ceiling_kbps, self.floor_kbps);
        self.wanted = decision.video_bps.map(|bps| {
            budget
                .encoder_kbps(bps + budget.audio_bps, ceiling, fec, period)
                .clamp(floor, ceiling)
        });
        if self.shadow { None } else { decision.pace_bps }
    }

    /// What this controller wants, encoder kbps, applied or (in shadow mode) not.
    pub fn wanted(&self) -> Option<u32> {
        self.wanted
    }

    /// The encoder bitrate to use now that the bitrate in effect is `current`: the lowest of
    /// the bitrate from before any limit and both controllers' limits.
    pub fn combine(&mut self, current: u32) -> u32 {
        let own = if self.shadow { None } else { self.wanted };
        let limit = match (self.link_wanted, own) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match limit {
            Some(kbps) => kbps.min(*self.restore.get_or_insert(current)),
            None => self.restore.take().unwrap_or(current),
        }
    }

    /// The last change worth a log line.
    pub fn take_change(&mut self) -> Option<Change> {
        self.cc.as_mut()?.take_change()
    }

    /// Queueing delay p50/p95 since the last call, for the `stream timings` line.
    pub fn take_window_delay(&mut self) -> (Option<f64>, Option<f64>, u64) {
        self.cc
            .as_mut()
            .map_or((None, None, 0), Congestion::take_window_delay)
    }

    pub fn snapshot(&self, now_us: u64) -> serde_json::Value {
        match &self.cc {
            Some(cc) => {
                let mut value = cc.snapshot(now_us);
                value["shadow"] = self.shadow.into();
                value["wanted_kbps"] = self.wanted.into();
                value["link_wanted_kbps"] = self.link_wanted.into();
                value
            }
            None => serde_json::json!({ "state": "off" }),
        }
    }
}
