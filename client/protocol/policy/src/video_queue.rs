//! Decoder output queue, residency governor, display queue watch, present scheduler lateness,
//! recovery backoff, and pure hang/recovery helpers.
//!
//! Shared between Android (`DecoderQueuePolicy`, `DisplayQueueWatch`, `PresentScheduler`,
//! `DecoderRecoveryBackoff`, `MediaCodecDecoderRenderer` via `policy_video_queue.rs`) and policy
//! tests.

pub mod compositor_watch;
pub mod debt;
pub mod display_watch;
pub mod gate;
#[cfg(test)]
mod tests;

pub use compositor_watch::{CompositorAction, CompositorWatch};
pub use debt::{Debt, Residency};
pub use display_watch::{DisplayQueueWatch, PresentSchedulerState};
pub use gate::{DisplayGate, RecentLevel};

use crate::video_pacing;

/// Refill threshold and attempt limit for decoder recovery (`MediaCodecDecoderRenderer`).
pub const CR_MAX_TRIES: i32 = 10;
pub const CR_REFILL_FRAMES: i32 = 600;
pub const CR_REFILL_MS: i64 = 10_000;

const OMX_SCARCE_OUTPUT_FAMILIES: [&str; 8] = [
    "omx.mtk.",
    "omx.exynos.",
    "omx.nvidia.",
    "omx.brcm.",
    "omx.ti.",
    "omx.allwinner.",
    "omx.amlogic.",
    "omx.intel.",
];

/// Frames the decoder holds: submitted less returned since the last flush, whose frames are gone
/// (`base` is the difference at that moment). Never negative.
pub fn depth(frames_in: i32, frames_out: i32, base: i32) -> i32 {
    frames_in.wrapping_sub(frames_out).wrapping_sub(base).max(0)
}

/// Client-held output buffer limit for a MediaCodec decoder.
pub fn output_limit(
    decoder_name: &str,
    advertised_low_latency: bool,
    drop_late_frames: bool,
    recovered: bool,
) -> i32 {
    if drop_late_frames || recovered {
        return 1;
    }
    let name = decoder_name.to_lowercase();
    if OMX_SCARCE_OUTPUT_FAMILIES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return 1;
    }
    if advertised_low_latency { 2 } else { 1 }
}

/// Whether [`Debt`] watches the display's queue on this decoder (Shield TV NVIDIA families,
/// Nightjar issue 7).
pub fn watches_display_queue(decoder_name: &str) -> bool {
    let name = decoder_name.to_lowercase();
    name.starts_with("omx.nvidia.") || name.starts_with("c2.nvidia.")
}

/// Frames released to the compositor and not yet shown. A frame counts from its release until it
/// is rendered, or until the vsync it was planned for has passed.
#[derive(Debug, Clone)]
pub struct InFlight {
    pts_us: [i64; Self::SLOTS],
    due_ns: [i64; Self::SLOTS],
    next: usize,
    rendered_pts_us: i64,
}

impl InFlight {
    pub const MAX: i32 = 2;
    const SLOTS: usize = 8;

    pub fn new() -> Self {
        Self {
            pts_us: [0; Self::SLOTS],
            due_ns: [0; Self::SLOTS],
            next: 0,
            rendered_pts_us: i64::MIN,
        }
    }

    pub fn on_released(&mut self, codec_pts_us: i64, due_at_ns: i64) {
        self.pts_us[self.next] = codec_pts_us;
        self.due_ns[self.next] = due_at_ns;
        self.next = (self.next + 1) % Self::SLOTS;
    }

    pub fn on_rendered(&mut self, codec_pts_us: i64) {
        self.rendered_pts_us = self.rendered_pts_us.max(codec_pts_us);
    }

    pub fn count(&self, now_ns: i64) -> i32 {
        let mut count = 0;
        for i in 0..Self::SLOTS {
            if self.due_ns[i] > now_ns && self.pts_us[i] > self.rendered_pts_us {
                count += 1;
            }
        }
        count
    }

    /// Another release would put more than [`Self::MAX`] frames in front of the compositor.
    pub fn full(&self, now_ns: i64) -> bool {
        self.count(now_ns) >= Self::MAX
    }

    pub fn reset(&mut self) {
        self.due_ns.fill(0);
        self.pts_us.fill(0);
        self.rendered_pts_us = i64::MIN;
    }
}

impl Default for InFlight {
    fn default() -> Self {
        Self::new()
    }
}

/// Paces decoder recoveries that keep failing before the decoder puts out a frame.
#[derive(Debug, Clone)]
pub struct DecoderRecoveryBackoff {
    delays_ms: Vec<i64>,
    frames_out_at_last_recovery: i32,
    without_frame: i32,
}

impl DecoderRecoveryBackoff {
    /// Wait before the 1st, 2nd, ... recovery in a row without a frame; one more gives up.
    pub const DEFAULT_DELAYS_MS: [i64; 5] = [0, 250, 500, 1000, 2000];

    pub fn new() -> Self {
        Self::with_delays(Self::DEFAULT_DELAYS_MS.to_vec())
    }

    pub fn with_delays(delays_ms: Vec<i64>) -> Self {
        Self {
            delays_ms,
            frames_out_at_last_recovery: 0,
            without_frame: 0,
        }
    }

    /// Before a recovery, with the frames the decoder has put out so far: how long to wait first,
    /// or `-1` when that many recoveries in a row brought no frame and the decoder is given up.
    pub fn before_recovery(&mut self, frames_out: i32) -> i64 {
        self.without_frame = if frames_out == self.frames_out_at_last_recovery {
            self.without_frame + 1
        } else {
            1
        };
        self.frames_out_at_last_recovery = frames_out;
        let index = usize::try_from(self.without_frame - 1).ok();
        match index.and_then(|i| self.delays_ms.get(i)) {
            Some(&delay_ms) => delay_ms,
            None => -1,
        }
    }

    /// Recoveries in a row that brought no frame, the one just asked about included.
    pub fn recoveries_without_frame(&self) -> i32 {
        self.without_frame
    }
}

impl Default for DecoderRecoveryBackoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Why `wants_catch_up` asks the renderer to drop the held output for a newer decoded one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchUpReason {
    None = 0,
    CatchUp = 1,
    Governor = 2,
}

/// Pure output-drain catch-up decision combining age budget (`FramePacingPolicy`) and
/// [`Residency`] governor. Returns `None` if `fps_milli_hz <= 0` when evaluating age drain.
#[allow(clippy::too_many_arguments)]
pub fn wants_catch_up(
    mode: i32,
    enqueue_ns: i64,
    now_ns: i64,
    fps_milli_hz: i32,
    drained: i32,
    max_output_drain: i32,
    scheduled: bool,
    frames_in: i32,
    frames_out: i32,
    depth_base: i32,
    residency: &Residency,
) -> Option<CatchUpReason> {
    if video_pacing::should_drain_output(mode, enqueue_ns, now_ns, fps_milli_hz, drained)? {
        return Some(CatchUpReason::CatchUp);
    }
    if scheduled
        && drained < max_output_drain
        && residency.should_drop(depth(frames_in, frames_out, depth_base))
    {
        return Some(CatchUpReason::Governor);
    }
    Some(CatchUpReason::None)
}

/// Whether recovery attempts are refilled after sustained healthy decoding, and whether any
/// attempt remains: `(attempts_left, refilled)`.
pub fn recovery_attempts_left(
    attempts: i32,
    frames_out: i32,
    frames_out_at_last_recovery: i32,
    uptime_ms: i64,
    uptime_at_last_recovery_ms: i64,
) -> (bool, bool) {
    let frames_since = frames_out.wrapping_sub(frames_out_at_last_recovery);
    let elapsed_ms = uptime_ms.wrapping_sub(uptime_at_last_recovery_ms);
    if attempts > 0 && frames_since >= CR_REFILL_FRAMES && elapsed_ms >= CR_REFILL_MS {
        (true, true)
    } else {
        (attempts < CR_MAX_TRIES, false)
    }
}

/// Whether the decoder ran well since its last hang reset.
pub fn ran_well_since_hang_reset(
    frames_out: i32,
    frames_out_at_hang_reset: i32,
    uptime_ms: i64,
    uptime_at_hang_reset_ms: i64,
) -> bool {
    frames_out.wrapping_sub(frames_out_at_hang_reset) >= CR_REFILL_FRAMES
        && uptime_ms.wrapping_sub(uptime_at_hang_reset_ms) >= CR_REFILL_MS
}

/// Pure decision for `MediaCodecDecoderRenderer.resetHungDecoder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HungDecoderAction {
    GiveUp = 0,
    ResetWithKeys = 1,
    RebuildWithoutKeys = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HungDecoderDecision {
    pub action: HungDecoderAction,
    pub refilled_attempts: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn hung_decoder_decision(
    has_vendor_keys: bool,
    has_decoder: bool,
    already_hung_with_keys: bool,
    attempts: i32,
    frames_out: i32,
    frames_out_at_last_recovery: i32,
    frames_out_at_hang_reset: i32,
    uptime_ms: i64,
    uptime_at_last_recovery_ms: i64,
    uptime_at_hang_reset_ms: i64,
) -> HungDecoderDecision {
    let mut refilled = false;
    let eligible_without_keys = has_vendor_keys && has_decoder && !already_hung_with_keys;
    let without_keys = if eligible_without_keys {
        if frames_out_at_hang_reset >= 0
            && !ran_well_since_hang_reset(
                frames_out,
                frames_out_at_hang_reset,
                uptime_ms,
                uptime_at_hang_reset_ms,
            )
        {
            true
        } else {
            let (left, did_refill) = recovery_attempts_left(
                attempts,
                frames_out,
                frames_out_at_last_recovery,
                uptime_ms,
                uptime_at_last_recovery_ms,
            );
            refilled = did_refill;
            !left
        }
    } else {
        false
    };

    if without_keys {
        return HungDecoderDecision {
            action: HungDecoderAction::RebuildWithoutKeys,
            refilled_attempts: refilled,
        };
    }

    if frames_out_at_hang_reset == frames_out {
        return HungDecoderDecision {
            action: HungDecoderAction::GiveUp,
            refilled_attempts: refilled,
        };
    }

    let current_attempts = if refilled { 0 } else { attempts };
    let (left, did_refill) = recovery_attempts_left(
        current_attempts,
        frames_out,
        frames_out_at_last_recovery,
        uptime_ms,
        uptime_at_last_recovery_ms,
    );
    refilled |= did_refill;

    HungDecoderDecision {
        action: if left {
            HungDecoderAction::ResetWithKeys
        } else {
            HungDecoderAction::GiveUp
        },
        refilled_attempts: refilled,
    }
}
