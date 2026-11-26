//! Self-detecting two-stage monitor for TV display compositor stalls and post-handoff judder.
//!
//! Tracks codec output release timestamps (`on_released`) against `MediaCodec.OnFrameRenderedListener`
//! callbacks (`on_rendered`) in 1-second buckets:
//!
//! 1. **Trigger 1 (dead render callbacks):** if callbacks expect delivery but fall below 15% for
//!    3 consecutive seconds after warmup, hold a 5-second keepalive probe (`KeepOn`). Lock ON if
//!    callbacks recover to >=50%, or stop and give up Trigger 1 on this stream if the HAL never
//!    delivers them.
//! 2. **Trigger 2 (late-onset post-handoff judder):** when callbacks are live and upstream decoder
//!    release cadence is steady (<5% abnormal gaps) while displayed `render_ns` intervals diverge
//!    from that cadence by >max(5 ms, 35% of frame period) on >=20% of frames for 4 consecutive
//!    seconds:
//!    - **Stage 1:** emit a single 500 ms `Pulse` to re-kick composition, wait 2 s, watch 20 s.
//!    - **Stage 2:** if judder persists for 3 s post-pulse (or relapses after >=2 pulses within
//!      15 min), promote to `KeepOn` for an 8-second probe and lock ON if pacing stabilizes.

use std::collections::VecDeque;

const WARMUP_NS: u64 = 3_000_000_000;
const BUCKET_MIN_NS: u64 = 850_000_000;
const IDLE_GAP_BREAK_NS: u64 = 1_000_000_000;
const MAX_RENDER_DRIFT_FROM_HANDOFF_NS: u64 = 5_000_000_000;
const MAX_SENTINEL_NS: u64 = (i64::MAX as u64) - 1_000_000_000;
const MAX_GAP_SAMPLES: usize = 512;

const TRIGGER1_MIN_RELEASES: u32 = 15;
const TRIGGER1_DEAD_STREAK_SECS: u8 = 3;
const TRIGGER1_PROBE_NS: u64 = 5_000_000_000;

const TRIGGER2_MIN_RELEASES: u32 = 20;
const TRIGGER2_MIN_RENDER_GAPS: usize = 10;
const TRIGGER2_JUDDER_STREAK_SECS: u8 = 4;
const STAGE1_SETTLE_NS: u64 = 2_000_000_000;
const STAGE1_WATCH_NS: u64 = 20_000_000_000;
const STAGE1_GOOD_STREAK_SECS: u8 = 3;
const STAGE1_BAD_STREAK_SECS: u8 = 3;
const STAGE1_COOLDOWN_NS: u64 = 30_000_000_000;
const PULSE_WINDOW_NS: u64 = 900_000_000_000; // 15 min
const PULSE_ESCALATE_COUNT: usize = 2;

const STAGE2_PROBE_NS: u64 = 8_000_000_000;
const STAGE2_GOOD_STREAK_SECS: u8 = 3;
const STAGE2_COOLDOWN_NS: u64 = 300_000_000_000; // 5 min
const STAGE2_MAX_FAILURES: u8 = 2;

const MIN_CADENCE_NS: u64 = 1_000_000; // 1000 Hz upper bound
const MAX_CADENCE_NS: u64 = 100_000_000; // 10 Hz lower bound
const MIN_REL_TOL_NS: u64 = 4_000_000;
const MIN_REN_TOL_NS: u64 = 5_000_000;

/// Action requested from `GameVideoOutput` / `CompositorHeartbeat` once per second.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositorAction {
    None = 0,
    Pulse = 1,
    KeepOn = 2,
    Stop = 3,
}

#[derive(Default)]
struct Bucket {
    released: u32,
    expected_cb: u32,
    rendered: u32,
    release_gaps_ns: Vec<u64>,
    render_gaps_ns: Vec<u64>,
}

impl Bucket {
    fn clear(&mut self) {
        self.released = 0;
        self.expected_cb = 0;
        self.rendered = 0;
        self.release_gaps_ns.clear();
        self.render_gaps_ns.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Watching {
        dead_cb_streak: u8,
        judder_streak: u8,
        cooldown_until_ns: u64,
    },
    ProbingTrigger1 {
        started_ns: u64,
    },
    Stage1Settling {
        pulse_ns: u64,
        baseline_bad_pct: u16,
    },
    Stage1Watching {
        started_ns: u64,
        baseline_bad_pct: u16,
        dead_cb_streak: u8,
        bad_streak: u8,
        good_streak: u8,
    },
    ProbingTrigger2 {
        started_ns: u64,
        baseline_bad_pct: u16,
        good_streak: u8,
    },
    LockedOn {
        trigger: u8,
    },
}

struct CadenceStats {
    upstream_steady: bool,
    ren_bad_pct: u16,
}

/// Evaluates decoder release vs scanout callback behavior to trigger self-healing keepalive only
/// when needed.
pub struct CompositorWatch {
    state: State,
    warmup_start_ns: Option<u64>,
    bucket_start_ns: Option<u64>,
    prev_release_ns: Option<u64>,
    prev_render_ns: Option<u64>,
    bucket: Bucket,
    trigger1_gave_up: bool,
    keep_on_active: bool,
    recent_pulses_ns: VecDeque<u64>,
    stage2_failures: u8,
    events: VecDeque<String>,
}

impl Default for CompositorWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositorWatch {
    pub fn new() -> Self {
        Self {
            state: State::Watching {
                dead_cb_streak: 0,
                judder_streak: 0,
                cooldown_until_ns: 0,
            },
            warmup_start_ns: None,
            bucket_start_ns: None,
            prev_release_ns: None,
            prev_render_ns: None,
            bucket: Bucket::default(),
            trigger1_gave_up: false,
            keep_on_active: false,
            recent_pulses_ns: VecDeque::new(),
            stage2_failures: 0,
            events: VecDeque::new(),
        }
    }

    /// Resets warmup grace and active sampling window (e.g. decoder/display reconfigure) while
    /// preserving any already-proven `LockedOn` decision.
    pub fn reset_window(&mut self) {
        self.warmup_start_ns = None;
        self.bucket_start_ns = None;
        self.prev_release_ns = None;
        self.prev_render_ns = None;
        self.bucket.clear();
        if !self.is_locked_on() {
            let cooldown = match self.state {
                State::Watching {
                    cooldown_until_ns, ..
                } => cooldown_until_ns,
                _ => 0,
            };
            self.state = State::Watching {
                dead_cb_streak: 0,
                judder_streak: 0,
                cooldown_until_ns: cooldown,
            };
        }
    }

    /// Records a codec output buffer released for presentation (`render == true`).
    pub fn on_released(&mut self, release_ns: u64, expects_render_callback: bool) {
        self.bucket.released = self.bucket.released.saturating_add(1);
        if expects_render_callback {
            self.bucket.expected_cb = self.bucket.expected_cb.saturating_add(1);
        }
        if let Some(prev) = self.prev_release_ns
            && release_ns >= prev
        {
            let gap = release_ns - prev;
            if gap <= IDLE_GAP_BREAK_NS && self.bucket.release_gaps_ns.len() < MAX_GAP_SAMPLES {
                self.bucket.release_gaps_ns.push(gap);
            }
        }
        if self.prev_release_ns.is_none_or(|prev| release_ns >= prev) {
            self.prev_release_ns = Some(release_ns);
        }
    }

    /// Records a delivered `OnFrameRendered` callback (`render_ns` presentation timestamp).
    pub fn on_rendered(&mut self, handoff_ns: u64, render_ns: u64, _now_ns: u64) {
        if render_ns == 0
            || render_ns >= MAX_SENTINEL_NS
            || (handoff_ns != 0
                && render_ns.abs_diff(handoff_ns) > MAX_RENDER_DRIFT_FROM_HANDOFF_NS)
        {
            return;
        }
        self.bucket.rendered = self.bucket.rendered.saturating_add(1);
        if let Some(prev) = self.prev_render_ns
            && render_ns >= prev
        {
            let gap = render_ns - prev;
            if gap <= IDLE_GAP_BREAK_NS && self.bucket.render_gaps_ns.len() < MAX_GAP_SAMPLES {
                self.bucket.render_gaps_ns.push(gap);
            }
        }
        if self.prev_render_ns.is_none_or(|prev| render_ns >= prev) {
            self.prev_render_ns = Some(render_ns);
        }
    }

    pub fn is_locked_on(&self) -> bool {
        matches!(self.state, State::LockedOn { .. })
    }

    pub fn take_event(&mut self) -> Option<String> {
        self.events.pop_front()
    }

    /// Evaluates the accumulated 1 s bucket and returns any keepalive action for the UI layer.
    pub fn poll(
        &mut self,
        now_ns: u64,
        overlay_visible: bool,
        manual_keepalive_on: bool,
    ) -> CompositorAction {
        let warmup_start = *self.warmup_start_ns.get_or_insert(now_ns);
        let bucket_start = *self.bucket_start_ns.get_or_insert(now_ns);
        let needs_keep_on = matches!(
            self.state,
            State::ProbingTrigger1 { .. } | State::ProbingTrigger2 { .. } | State::LockedOn { .. }
        );
        if self.keep_on_active && !needs_keep_on {
            self.keep_on_active = false;
            self.bucket.clear();
            self.bucket_start_ns = Some(now_ns);
            return CompositorAction::Stop;
        }

        if overlay_visible || manual_keepalive_on {
            self.bucket.clear();
            self.bucket_start_ns = Some(now_ns);
            let act = self.suppress_transient_state();
            return self.finish_action(act);
        }

        if now_ns.saturating_sub(warmup_start) < WARMUP_NS {
            self.bucket.clear();
            self.bucket_start_ns = Some(now_ns);
            return CompositorAction::None;
        }

        if matches!(self.state, State::LockedOn { .. }) {
            self.bucket.clear();
            self.bucket_start_ns = Some(now_ns);
            return CompositorAction::None;
        }

        if now_ns.saturating_sub(bucket_start) < BUCKET_MIN_NS {
            return CompositorAction::None;
        }
        self.bucket_start_ns = Some(now_ns);
        let act = self.evaluate_bucket(now_ns);
        self.finish_action(act)
    }

    fn finish_action(&mut self, action: CompositorAction) -> CompositorAction {
        match action {
            CompositorAction::KeepOn => self.keep_on_active = true,
            CompositorAction::Stop => self.keep_on_active = false,
            CompositorAction::None | CompositorAction::Pulse => {}
        }
        action
    }

    fn suppress_transient_state(&mut self) -> CompositorAction {
        let was_probing_keep_on = matches!(
            self.state,
            State::ProbingTrigger1 { .. } | State::ProbingTrigger2 { .. }
        );
        if !self.is_locked_on() {
            let cooldown = match self.state {
                State::Watching {
                    cooldown_until_ns, ..
                } => cooldown_until_ns,
                _ => 0,
            };
            self.state = State::Watching {
                dead_cb_streak: 0,
                judder_streak: 0,
                cooldown_until_ns: cooldown,
            };
        }
        if was_probing_keep_on {
            CompositorAction::Stop
        } else {
            CompositorAction::None
        }
    }

    fn push_event(&mut self, msg: String) {
        if self.events.len() >= 16 {
            self.events.pop_front();
        }
        self.events.push_back(msg);
    }

    fn evaluate_bucket(&mut self, now_ns: u64) -> CompositorAction {
        let released = self.bucket.released;
        let expected_cb = self.bucket.expected_cb;
        let rendered = self.bucket.rendered;
        let callbacks_dead = !self.trigger1_gave_up
            && expected_cb >= TRIGGER1_MIN_RELEASES
            && u64::from(rendered) * 100 < u64::from(expected_cb) * 15;
        let callbacks_recovered = expected_cb >= TRIGGER1_MIN_RELEASES
            && u64::from(rendered) * 100 >= u64::from(expected_cb) * 50;
        let stats = compute_cadence_stats(&self.bucket);
        self.bucket.clear();

        match self.state {
            State::LockedOn { .. } => CompositorAction::None,
            State::Watching {
                mut dead_cb_streak,
                mut judder_streak,
                cooldown_until_ns,
            } => {
                if callbacks_dead {
                    dead_cb_streak = dead_cb_streak.saturating_add(1);
                    judder_streak = 0;
                    if dead_cb_streak >= TRIGGER1_DEAD_STREAK_SECS {
                        self.state = State::ProbingTrigger1 { started_ns: now_ns };
                        self.push_event(format!(
                            "CompositorWatch: dead onFrameRendered callbacks (rendered={rendered}/{expected_cb}); starting 5s keepalive probe"
                        ));
                        return CompositorAction::KeepOn;
                    }
                    self.state = State::Watching {
                        dead_cb_streak,
                        judder_streak,
                        cooldown_until_ns,
                    };
                    return CompositorAction::None;
                }
                dead_cb_streak = 0;

                if now_ns >= cooldown_until_ns && self.stage2_failures < STAGE2_MAX_FAILURES {
                    if let Some(s) = stats
                        && s.upstream_steady
                        && s.ren_bad_pct >= 20
                    {
                        judder_streak = judder_streak.saturating_add(1);
                        if judder_streak >= TRIGGER2_JUDDER_STREAK_SECS {
                            return self.start_trigger2(now_ns, s.ren_bad_pct);
                        }
                    } else {
                        judder_streak = 0;
                    }
                } else {
                    judder_streak = 0;
                }

                self.state = State::Watching {
                    dead_cb_streak,
                    judder_streak,
                    cooldown_until_ns,
                };
                CompositorAction::None
            }
            State::ProbingTrigger1 { started_ns } => {
                if callbacks_recovered {
                    self.state = State::LockedOn { trigger: 1 };
                    self.push_event(format!(
                        "CompositorWatch: render callbacks recovered (rendered={rendered}/{expected_cb}, released={released}); locking compositor keepalive ON"
                    ));
                    return CompositorAction::None;
                }
                if now_ns.saturating_sub(started_ns) >= TRIGGER1_PROBE_NS {
                    self.trigger1_gave_up = true;
                    self.state = State::Watching {
                        dead_cb_streak: 0,
                        judder_streak: 0,
                        cooldown_until_ns: now_ns,
                    };
                    self.push_event(format!(
                        "CompositorWatch: render callbacks did not recover within 5s (rendered={rendered}/{expected_cb}); stopping keepalive"
                    ));
                    return CompositorAction::Stop;
                }
                CompositorAction::None
            }
            State::Stage1Settling {
                pulse_ns,
                baseline_bad_pct,
            } => {
                if now_ns.saturating_sub(pulse_ns) < STAGE1_SETTLE_NS {
                    return CompositorAction::None;
                }
                self.state = State::Stage1Watching {
                    started_ns: now_ns,
                    baseline_bad_pct,
                    dead_cb_streak: 0,
                    bad_streak: 0,
                    good_streak: 0,
                };
                self.evaluate_stage1_watch(now_ns, callbacks_dead, stats)
            }
            State::Stage1Watching { .. } => {
                self.evaluate_stage1_watch(now_ns, callbacks_dead, stats)
            }
            State::ProbingTrigger2 {
                started_ns,
                baseline_bad_pct,
                mut good_streak,
            } => {
                if let Some(s) = stats
                    && s.upstream_steady
                {
                    let half_baseline = s.ren_bad_pct <= (baseline_bad_pct / 2).max(1);
                    if s.ren_bad_pct < 10 || half_baseline {
                        good_streak = good_streak.saturating_add(1);
                        if good_streak >= STAGE2_GOOD_STREAK_SECS {
                            self.state = State::LockedOn { trigger: 2 };
                            self.push_event(format!(
                                "CompositorWatch: keepalive cleared post-handoff judder (bad={}% vs baseline={baseline_bad_pct}%); locking ON",
                                s.ren_bad_pct
                            ));
                            return CompositorAction::None;
                        }
                    } else {
                        good_streak = 0;
                    }
                }

                if now_ns.saturating_sub(started_ns) >= STAGE2_PROBE_NS {
                    self.stage2_failures = self.stage2_failures.saturating_add(1);
                    self.state = State::Watching {
                        dead_cb_streak: 0,
                        judder_streak: 0,
                        cooldown_until_ns: now_ns.saturating_add(STAGE2_COOLDOWN_NS),
                    };
                    self.push_event(format!(
                        "CompositorWatch: keepalive did not clear judder within 8s (attempt {}/{STAGE2_MAX_FAILURES}); stopping",
                        self.stage2_failures
                    ));
                    return CompositorAction::Stop;
                }

                self.state = State::ProbingTrigger2 {
                    started_ns,
                    baseline_bad_pct,
                    good_streak,
                };
                CompositorAction::None
            }
        }
    }

    fn start_trigger2(&mut self, now_ns: u64, baseline_bad_pct: u16) -> CompositorAction {
        while let Some(&ts) = self.recent_pulses_ns.front() {
            if now_ns.saturating_sub(ts) > PULSE_WINDOW_NS {
                self.recent_pulses_ns.pop_front();
            } else {
                break;
            }
        }
        if self.recent_pulses_ns.len() < PULSE_ESCALATE_COUNT {
            self.recent_pulses_ns.push_back(now_ns);
            self.state = State::Stage1Settling {
                pulse_ns: now_ns,
                baseline_bad_pct,
            };
            self.push_event(format!(
                "CompositorWatch: steady upstream with post-handoff judder (bad={baseline_bad_pct}%); sending 500ms compositor pulse"
            ));
            CompositorAction::Pulse
        } else {
            self.state = State::ProbingTrigger2 {
                started_ns: now_ns,
                baseline_bad_pct,
                good_streak: 0,
            };
            self.push_event(format!(
                "CompositorWatch: repeated post-handoff judder (bad={baseline_bad_pct}%); promoting directly to Stage 2 keepalive probe"
            ));
            CompositorAction::KeepOn
        }
    }

    fn evaluate_stage1_watch(
        &mut self,
        now_ns: u64,
        callbacks_dead: bool,
        stats: Option<CadenceStats>,
    ) -> CompositorAction {
        let State::Stage1Watching {
            started_ns,
            baseline_bad_pct,
            mut dead_cb_streak,
            mut bad_streak,
            mut good_streak,
        } = self.state
        else {
            return CompositorAction::None;
        };

        if callbacks_dead {
            dead_cb_streak = dead_cb_streak.saturating_add(1);
            bad_streak = 0;
            good_streak = 0;
            if dead_cb_streak >= TRIGGER1_DEAD_STREAK_SECS {
                self.state = State::ProbingTrigger1 { started_ns: now_ns };
                self.push_event(
                    "CompositorWatch: callbacks went dead during post-pulse watch; starting keepalive probe"
                        .to_string(),
                );
                return CompositorAction::KeepOn;
            }
        } else {
            dead_cb_streak = 0;
            if let Some(s) = stats
                && s.upstream_steady
            {
                if s.ren_bad_pct < 10 {
                    good_streak = good_streak.saturating_add(1);
                    bad_streak = 0;
                    if good_streak >= STAGE1_GOOD_STREAK_SECS {
                        self.state = State::Watching {
                            dead_cb_streak: 0,
                            judder_streak: 0,
                            cooldown_until_ns: now_ns.saturating_add(STAGE1_COOLDOWN_NS),
                        };
                        self.push_event(
                            "CompositorWatch: 500ms compositor pulse cleared judder; returning to watch"
                                .to_string(),
                        );
                        return CompositorAction::None;
                    }
                } else if s.ren_bad_pct >= 20 {
                    bad_streak = bad_streak.saturating_add(1);
                    good_streak = 0;
                    if bad_streak >= STAGE1_BAD_STREAK_SECS {
                        self.state = State::ProbingTrigger2 {
                            started_ns: now_ns,
                            baseline_bad_pct: baseline_bad_pct.max(s.ren_bad_pct),
                            good_streak: 0,
                        };
                        self.push_event(format!(
                            "CompositorWatch: judder persisted post-pulse (bad={}% vs baseline={baseline_bad_pct}%); escalating to Stage 2 keepalive",
                            s.ren_bad_pct
                        ));
                        return CompositorAction::KeepOn;
                    }
                } else {
                    good_streak = 0;
                    bad_streak = 0;
                }
            }
        }

        if now_ns.saturating_sub(started_ns) >= STAGE1_WATCH_NS {
            self.state = State::Watching {
                dead_cb_streak: 0,
                judder_streak: 0,
                cooldown_until_ns: now_ns.saturating_add(STAGE1_COOLDOWN_NS),
            };
            self.push_event(
                "CompositorWatch: post-pulse watch elapsed without sustained relapse; returning to watch"
                    .to_string(),
            );
            return CompositorAction::None;
        }

        self.state = State::Stage1Watching {
            started_ns,
            baseline_bad_pct,
            dead_cb_streak,
            bad_streak,
            good_streak,
        };
        CompositorAction::None
    }
}

fn compute_cadence_stats(bucket: &Bucket) -> Option<CadenceStats> {
    if bucket.released < TRIGGER2_MIN_RELEASES
        || bucket.expected_cb < TRIGGER1_MIN_RELEASES
        || u64::from(bucket.rendered) * 100 < u64::from(bucket.expected_cb) * 50
        || bucket.release_gaps_ns.len() < TRIGGER1_MIN_RELEASES as usize
        || bucket.render_gaps_ns.len() < TRIGGER2_MIN_RENDER_GAPS
    {
        return None;
    }

    let mut rel = bucket.release_gaps_ns.clone();
    rel.sort_unstable();
    let cadence_ns = rel[rel.len() / 2];
    if !(MIN_CADENCE_NS..=MAX_CADENCE_NS).contains(&cadence_ns) {
        return None;
    }

    let rel_tol_ns = MIN_REL_TOL_NS.max(cadence_ns * 30 / 100);
    let rel_bad = rel
        .iter()
        .filter(|&&g| g.abs_diff(cadence_ns) > rel_tol_ns)
        .count();
    let upstream_steady = rel_bad * 100 < rel.len() * 5;

    let ren_tol_ns = MIN_REN_TOL_NS.max(cadence_ns * 35 / 100);
    let ren_bad = bucket
        .render_gaps_ns
        .iter()
        .filter(|&&g| g.abs_diff(cadence_ns) > ren_tol_ns)
        .count();
    let ren_bad_pct = ((ren_bad * 100) / bucket.render_gaps_ns.len()) as u16;

    Some(CadenceStats {
        upstream_steady,
        ren_bad_pct,
    })
}

#[cfg(test)]
#[path = "compositor_watch_tests.rs"]
mod tests;
