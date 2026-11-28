//! Capture-key deduplication, weak display-target tracking, and multi-consumer claim-grid
//! pacing state for [`super::Source`].

use super::super::*;
use std::sync::{Arc, Mutex, PoisonError, Weak};

#[derive(Debug, PartialEq, Eq, Hash)]
pub(in crate::stream) struct CaptureKey {
    kind: String,
    output: String,
    /// The user's Capture method was Automatic, which the fullscreen fallback
    /// may move to Desktop Duplication. An explicit method never shares its
    /// capture, or the fallback would move it behind the user's back.
    automatic: bool,
    hdr: bool,
    adapter: String,
    adapter_id: String,
    phase: String,
    /// A session following an app window (`capture_window = window`) or targeting a specific
    /// window must not share a capture source with a plain display stream or another session's
    /// window target.
    capture_window: bool,
    window_session: String,
    compute: bool,
    wgc_compute: bool,
    wgc_user_helper: bool,
    wgc_high_rate: bool,
    wgc_drain: bool,
    wgc_cursor: bool,
    wgc_helper_scheduling: bool,
    idd_fence_ring: bool,
    background: bool,
    constant_rate: bool,
}

impl CaptureKey {
    pub(in crate::stream) fn new(
        kind: &str,
        output: &str,
        hdr: bool,
        config: &Config,
        phase: &str,
        constant_rate: bool,
    ) -> Self {
        let capture_window = butterpollo_media_policy::app_window::settings(config).is_some();
        let is_window_target = butterpollo_win_graphics::capture::WindowTarget::parse(output)
            .ok()
            .flatten()
            .is_some();
        Self {
            kind: kind.into(),
            output: output.into(),
            automatic: butterpollo_media_policy::capture_policy::automatic_capture(
                config::capture_setting(config),
            ),
            hdr,
            adapter: config::adapter_name(config).into(),
            adapter_id: config::adapter_pnp_id(config).into(),
            phase: if config::wgc_slot_aligned_publish(config) {
                phase
            } else {
                ""
            }
            .into(),
            capture_window,
            window_session: if capture_window || is_window_target {
                phase
            } else {
                ""
            }
            .into(),
            compute: config::gpu_compute_conversion(config),
            wgc_compute: config::wgc_compute_copy(config),
            wgc_user_helper: config::wgc_user_helper(config),
            wgc_high_rate: !matches!(kind, "ddx" | "dxgi") && config::wgc_high_rate_capture(config),
            wgc_drain: !matches!(kind, "ddx" | "dxgi") && config::wgc_drain_to_newest(config),
            wgc_cursor: !matches!(kind, "ddx" | "dxgi") && config::wgc_cursor_capture(config),
            wgc_helper_scheduling: !matches!(kind, "ddx" | "dxgi")
                && config::wgc_helper_streaming_scope(config),
            idd_fence_ring: config.idd_push_fence_ring(),
            background: butterpollo_session::background::enabled(config),
            constant_rate,
        }
    }
}

/// Weak references to every active stream's [`crate::display_session::Ready`] sharing a
/// [`super::Source`]. Holding weak refs ensures the capture worker never pins an exited
/// stream's display lease alive while another consumer stays attached.
pub(super) struct Targets {
    inner: Mutex<TargetsState>,
}

struct TargetsState {
    attached: Vec<Weak<crate::display_session::Ready>>,
    last_capture: (String, u64),
    last_display: (String, u64),
}

impl Targets {
    pub(super) fn new(prepared: &Arc<crate::display_session::Ready>) -> Self {
        Self {
            inner: Mutex::new(TargetsState {
                attached: vec![Arc::downgrade(prepared)],
                last_capture: prepared.capture_target(),
                last_display: prepared.display_target(),
            }),
        }
    }

    pub(super) fn attach(&self, prepared: &Arc<crate::display_session::Ready>) {
        let capture = prepared.capture_target();
        let display = prepared.display_target();
        let weak = Arc::downgrade(prepared);
        let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        state.attached.retain(|entry| entry.strong_count() > 0);
        if !state.attached.iter().any(|entry| entry.ptr_eq(&weak)) {
            state.attached.push(weak);
        }
        state.last_capture = capture;
        state.last_display = display;
    }

    pub(super) fn capture_target(&self) -> (String, u64) {
        let (live, fallback) = {
            let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            state.attached.retain(|entry| entry.strong_count() > 0);
            let live = state.attached.iter().find_map(Weak::upgrade);
            (live, state.last_capture.clone())
        };
        let Some(ready) = live else {
            return fallback;
        };
        let target = ready.capture_target();
        drop(ready);
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_capture = target.clone();
        target
    }

    pub(super) fn display_target(&self) -> (String, u64) {
        let (live, fallback) = {
            let mut state = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            state.attached.retain(|entry| entry.strong_count() > 0);
            let live = state.attached.iter().find_map(Weak::upgrade);
            (live, state.last_display.clone())
        };
        let Some(ready) = live else {
            return fallback;
        };
        let target = ready.display_target();
        drop(ready);
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_display = target.clone();
        target
    }
}

/// Per-consumer pacing registry for a shared [`butterpollo_win_graphics::capture::ClaimGrid`].
/// Always programs the shared grid from the fastest live consumer so a slower guest stream
/// never degrades a faster owner stream's claim cadence.
#[derive(Default)]
pub(super) struct ClaimGridTracker {
    consumers: Mutex<Vec<ConsumerGrid>>,
}

struct ConsumerGrid {
    consumer: Weak<capture::Consumer>,
    period: Duration,
    anchor: Instant,
}

impl ClaimGridTracker {
    pub(super) fn update(
        &self,
        grid: &Mutex<butterpollo_win_graphics::capture::ClaimGrid>,
        consumer: &Arc<capture::Consumer>,
        period: Duration,
        anchor: Instant,
    ) {
        let chosen = {
            let mut entries = self
                .consumers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            entries.retain(|entry| entry.consumer.strong_count() > 0);
            let weak = Arc::downgrade(consumer);
            if let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.consumer.ptr_eq(&weak))
            {
                entry.period = period;
                entry.anchor = anchor;
            } else {
                entries.push(ConsumerGrid {
                    consumer: weak,
                    period,
                    anchor,
                });
            }
            let mut best: Option<(Duration, Instant)> = None;
            for entry in entries.iter() {
                if best.is_none_or(|(best_period, _)| entry.period < best_period) {
                    best = Some((entry.period, entry.anchor));
                }
            }
            best
        };
        if let Some((best_period, best_anchor)) = chosen {
            let mut shared = grid.lock().unwrap_or_else(PoisonError::into_inner);
            shared.period = best_period;
            shared.anchor = best_anchor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_key_separates_window_capture_and_window_targets_from_shared_display_streams() {
        let display_cfg = Config::parse("wgc_slot_aligned_publish=false\n").unwrap();
        let window_cfg =
            Config::parse("wgc_slot_aligned_publish=false\ncapture_window=window\n").unwrap();

        // Plain display streams with slot-aligned publish off share one capture across sessions.
        assert_eq!(
            CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &display_cfg, "sess-a", false),
            CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &display_cfg, "sess-b", false),
        );

        // A `capture_window=window` stream never shares a capture with a display stream or
        // another session's window capture.
        assert_ne!(
            CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &display_cfg, "sess-a", false),
            CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &window_cfg, "sess-a", false),
        );
        assert_ne!(
            CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &window_cfg, "sess-a", false),
            CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &window_cfg, "sess-b", false),
        );

        // Explicit `window:` targets are also isolated per session.
        assert_ne!(
            CaptureKey::new(
                "wgc",
                r"window:42@\\.\DISPLAY1",
                false,
                &display_cfg,
                "sess-a",
                false,
            ),
            CaptureKey::new(
                "wgc",
                r"window:42@\\.\DISPLAY1",
                false,
                &display_cfg,
                "sess-b",
                false,
            ),
        );
    }

    #[test]
    fn claim_grid_tracker_keeps_fastest_consumer_and_recovers_when_it_leaves() {
        let latest = Latest::new();
        let fast = latest.subscribe(Weak::new()).unwrap();
        let slow = latest.subscribe(Weak::new()).unwrap();

        let t0 = Instant::now();
        let t_fast = t0 + Duration::from_millis(8);
        let t_slow = t0 + Duration::from_millis(16);
        let fast_period = Duration::from_micros(8_333);
        let slow_period = Duration::from_micros(16_667);

        let grid = Mutex::new(butterpollo_win_graphics::capture::ClaimGrid {
            anchor: t0,
            period: slow_period,
        });
        let tracker = ClaimGridTracker::default();

        tracker.update(&grid, &fast, fast_period, t_fast);
        {
            let g = grid.lock().unwrap();
            assert_eq!(g.period, fast_period);
            assert_eq!(g.anchor, t_fast);
        }

        // A slower guest joining must not overwrite the 120 Hz owner's period or anchor.
        tracker.update(&grid, &slow, slow_period, t_slow);
        {
            let g = grid.lock().unwrap();
            assert_eq!(g.period, fast_period);
            assert_eq!(g.anchor, t_fast);
        }

        // Once the fast consumer exits, the remaining slow consumer cleanly takes over.
        drop(fast);
        let t_slow_2 = t0 + Duration::from_millis(33);
        tracker.update(&grid, &slow, slow_period, t_slow_2);
        {
            let g = grid.lock().unwrap();
            assert_eq!(g.period, slow_period);
            assert_eq!(g.anchor, t_slow_2);
        }
    }
}
