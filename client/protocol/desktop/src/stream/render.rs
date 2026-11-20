//! The stream's picture loop: taking decoded pictures and slices, pacing them, and drawing,
//! enhancing and presenting them, with the beam race and VRR checks around the present.

use super::*;

/// What the raced presenter guesses of a stream's slice times before it has seen a frame.
fn seeded_predictor(facts: &Facts) -> race::Predictor {
    let frame_bytes = f64::from(facts.bitrate_kbps.max(1)) * 1000.0 / 8.0 / f64::from(facts.fps.max(1));
    race::Predictor::seeded(facts.race_slices, facts.height.max(1) as u32, frame_bytes, facts.pace_mbps)
}

/// The picture size a decoded frame's setup gives where the frame itself does not tell it (PyroWave,
/// packed 4:4:4): the logical size, which a packed frame's coded size is twice as wide as.
fn setup_size(setup: &session::VideoSetup) -> (i32, i32) {
    if setup.logical_width > 0 && setup.logical_height > 0 { (setup.logical_width, setup.logical_height) } else { (setup.width, setup.height) }
}

/// One refresh of a display refreshing `refresh_hz` times a second, in microseconds.
fn refresh_period_us(refresh_hz: f32) -> u64 {
    (1_000_000.0 / refresh_hz.max(1.0)) as u64
}

/// How long after a size change is asked for a picture that fits both the old and the new size
/// plus alignment padding is still taken for the old one: AMD codes a 1920 and a 1904 wide AV1
/// stream both 1920 wide, and the PC's old pictures are on their way for a while.
const SWITCH_GRACE: Duration = Duration::from_secs(1);

/// The patched SDL GPU renderer's properties for video-only frames and read-backs
/// (`vendor/sdl3-src/patches/window-target.patch`, `GPU_FrameEligible`).
#[cfg(target_os = "linux")]
const VIDEO_ONLY_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.video_only";
#[cfg(target_os = "linux")]
const VIDEO_ONLY_FRAME_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.video_only_frame";
#[cfg(target_os = "linux")]
const VIDEO_ONLY_DROPPED_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.video_only_dropped_draws";
#[cfg(target_os = "linux")]
const READ_BACK_PROPERTY: &std::ffi::CStr = c"SDL.renderer.gpu.read_back";

/// The sizes a decoded picture may be of, to show it without an encoder's alignment padding
/// (`crop::visible_size`): the size last asked of the PC, the one the stream is at, and for
/// `SWITCH_GRACE` after a change the one before it.
pub(super) struct StreamSizes {
    /// The size last sent to the PC; the stream menu's unapplied edits are not.
    asked: (i32, i32),
    /// The negotiated size, then each size the PC confirmed.
    current: (i32, i32),
    /// The size before `current`, and when `current` was confirmed.
    previous: Option<((i32, i32), Instant)>,
}

impl StreamSizes {
    pub(super) fn new(size: (i32, i32)) -> Self { Self { asked: size, current: size, previous: None } }

    /// A size sent to the PC.
    pub(super) fn ask(&mut self, size: (i32, i32)) { self.asked = size; }

    /// The stream's part of a picture of `coded` size arriving at `now`, and whether it confirms
    /// `pending`, the size asked for and when, not confirmed yet.
    #[cfg(test)]
    pub(super) fn arrive(&mut self, coded: (i32, i32), pending: Option<((i32, i32), Instant)>, now: Instant) -> ((i32, i32), bool) {
        let (visible, confirmed, _) = self.arrive_mode(coded, pending, now);
        (visible, confirmed)
    }

    /// [`arrive`](Self::arrive), also saying whether the size is one the PC applied after the
    /// timeout dropped its pending request: no confirmation, but the asked mode is applied with it.
    pub(super) fn arrive_mode(&mut self, coded: (i32, i32), pending: Option<((i32, i32), Instant)>, now: Instant) -> ((i32, i32), bool, bool) {
        use nightjar_core::video::crop::visible_size;
        if self.previous.is_some_and(|(_, at)| now.saturating_duration_since(at) >= SWITCH_GRACE) { self.previous = None; }
        let sizes = [self.asked, self.current];
        let exact = sizes.contains(&coded) || self.previous.is_some_and(|(size, _)| size == coded);
        // A pending size asked for SWITCH_GRACE ago takes a picture that fits its padding, so the
        // new stream's pictures confirm it.
        let settled = pending.filter(|&(_, at)| !exact && now.saturating_duration_since(at) >= SWITCH_GRACE)
            .map(|(size, _)| visible_size(coded, &[size])).filter(|&visible| visible != coded);
        let visible = settled.unwrap_or_else(|| {
            let visible = visible_size(coded, &sizes);
            // Pictures still queued from before a change, only when no current size fits them.
            match self.previous { Some((size, _)) if visible == coded => visible_size(coded, &[size]), _ => visible }
        });
        let confirmed = pending.is_some_and(|(size, _)| size == visible);
        // A size the PC applied after the timeout dropped it from pending is the stream's too.
        let late = pending.is_none() && visible == self.asked && visible != self.current;
        if confirmed || late {
            self.previous = Some((self.current, now));
            self.current = visible;
        }
        (visible, confirmed, late)
    }
}

/// What a picture whose size applies a reconfigure asks for: a confirmation only moves the rate
/// (the size and settings were set when it was asked), while a late apply after the timeout must
/// set the whole mode, which the timeout had put back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModeApply {
    Rate(Reconfigure),
    Whole(Reconfigure),
}

fn mode_to_apply(confirmed: bool, late: bool, pending: Option<Reconfigure>, late_mode: Option<Reconfigure>) -> Option<ModeApply> {
    if confirmed {
        pending.map(ModeApply::Rate)
    } else if late {
        late_mode.map(ModeApply::Whole)
    } else {
        None
    }
}

/// The end of a stream whose packed 4:4:4 ("Sharper colored text") pictures cannot be shown.
const PACKED_UNSHOWN: &str = en("This device cannot show Sharper colored text for this stream. Turn off Sharper colored text in Settings, then start the stream again.");

/// Whether AI enhancement is the upscaling in effect: chosen, not failed this stream, and the
/// picture is one it reads (`VideoTexture::enhance_source`: hardware-decoded NV12 or P010 in the
/// video texture). Otherwise the picture is drawn as with bilinear scaling, at its cost, and the
/// statistics say so (`upscaling_line`).
fn ai_in_effect(upscaling: config::Upscaling, failed: bool, readable: bool) -> bool {
    upscaling == config::Upscaling::Ai && !failed && readable
}

/// What the toast says when the PC did not confirm a requested size or frame rate in time:
/// automatic quality stops only where it was on for the stream.
fn unconfirmed_mode_toast(quality_on: bool) -> &'static str {
    if quality_on { tr("The PC did not confirm the requested size. Automatic quality stopped.") } else { tr("The PC did not apply the new size or frame rate.") }
}

/// Pictures in a row the presenter could not show before the decoder that made them gives way
/// (`Shared::present_refused`): one that fails alone may be a passing fault, not its format.
const UNSHOWN_BEFORE_FALLBACK: u32 = 5;

/// Pictures in a row the presenter could not show.
#[derive(Debug, Default)]
pub(super) struct Unshown {
    count: u32,
}

impl Unshown {
    /// A picture was shown.
    pub(super) fn shown(&mut self) { self.count = 0; }

    /// A picture ready at `ready_us` could not be shown: Some(the time to hand the decoder,
    /// never 0) once `UNSHOWN_BEFORE_FALLBACK` did in a row; the next decoder's pictures count
    /// anew.
    pub(super) fn failed(&mut self, ready_us: u64) -> Option<u64> {
        self.count += 1;
        if self.count < UNSHOWN_BEFORE_FALLBACK { return None; }
        self.count = 0;
        Some(ready_us.max(1))
    }
}

/// A shown frame's two shares in microseconds: Nightjar's own work on the render thread, from the
/// first look at the picture (`begin_us`) to the present call (upload or mapping, the draws, the
/// overlay, a wait for the scan), and the present call itself, where the thread waits on what is
/// not its own code: SDL's run of the queued draws, the driver, the refresh with V-Sync, the
/// compositor (gamescope's Vulkan layer takes about 1.15 ms a present on an Iris Plus 655,
/// rust/PERFORMANCE.md, November 12), and a GPU that is behind, whoever loaded it. The overlay's
/// Render counts the first only; the second has its own line, Present wait.
pub(super) fn frame_shares(begin_us: u64, issued_us: u64, returned_us: u64) -> (u64, u64) {
    (issued_us.saturating_sub(begin_us), returned_us.saturating_sub(issued_us))
}

/// A present of the texture: when the call was issued and returned (us), and the beam around it.
struct Flipped {
    issued_us: u64,
    returned_us: u64,
    scan: Option<race_probe::ScanReport>,
    /// The picture was not presented: the last one still waits for the display.
    refused: bool,
}

impl Streaming {
    /// Moves decoded pictures into the pacer; sends a clock sync probe when due.
    pub fn pump(&mut self) {
        // The keyboard hook keeps the shell's keys only while this loop runs.
        system_keys::alive();
        self.cursor.tick(Instant::now());
        self.release_shortcut_when_due(Instant::now());
        if let Some(Err(error)) = self.microphone.as_mut().map(microphone::Microphone::poll) {
            self.shared.notify(Notice::MicrophoneFailed(error));
        }
        if let Some(microphone) = &self.microphone {
            while let Ok(packet) = microphone.packets.try_recv() {
                if let Err(error) = self.session.send_microphone(&packet) {
                    self.shared.notify(Notice::MicrophoneFailed(format!("Microphone forwarding stopped: {error}")));
                    break;
                }
            }
        }
        // Frames first: the slices of every frame taken here were queued before the frame was.
        let mut frames = std::mem::take(&mut self.taken_frames);
        std::mem::swap(&mut *lock(&self.shared.frames), &mut frames);
        // With the race off nothing queues a slice, and nothing here takes the lock.
        let mut slices = std::mem::take(&mut self.taken_slices);
        if self.facts.beam_race { std::mem::swap(&mut *lock(&self.shared.slices), &mut slices); }
        for mut decoded in frames.drain(..) {
            if decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 { self.race_setup = Some(decoded.setup); }
            let coded = decoded.picture.frame().filter(|_| decoded.setup.chroma_packing == 0 && decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE == 0)
                .map(|frame| { let info = frame.info(); (info.width, info.height) });
            if let Some(coded) = coded {
                // The size without an encoder's alignment padding, worked out once on arrival: the
                // presenter shows that part of this picture whatever was asked for since.
                let pending = self.pending_mode.map(|(mode, at)| ((i32::from(mode.width), i32::from(mode.height)), at));
                let ((width, height), confirmed, late) = self.stream_sizes.arrive_mode(coded, pending, Instant::now());
                decoded.visible = Some((width, height));
                self.facts.width = width;
                self.facts.height = height;
                if confirmed || late {
                    self.fps_evidence = 0;
                    self.mode_before = None;
                    let pending = self.pending_mode.map(|(mode, _)| mode);
                    match mode_to_apply(confirmed, late, pending, self.late_mode) {
                        Some(ModeApply::Rate(mode)) => {
                            self.pending_mode = None;
                            self.late_mode = None;
                            self.stream_rate_changed(mode.fps_millihz.div_ceil(1000));
                        }
                        // The timeout already put the old mode back: apply the whole asked mode.
                        Some(ModeApply::Whole(mode)) => {
                            self.late_mode = None;
                            self.apply_mode(mode);
                        }
                        None => {}
                    }
                }
            } else if decoded.setup.chroma_packing != 0 || decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                // The menu's header and the session page name the stream's size: PyroWave and packed
                // 4:4:4 tell it in their setup, a reconfigure's new setup included.
                (self.facts.width, self.facts.height) = setup_size(&decoded.setup);
            }
            if let Some((rtp, received)) = self.last_arrival {
                let ticks = decoded.rtp.wrapping_sub(rtp) as i32;
                if ticks > 0 && decoded.receive_us >= received {
                    let delta = ((decoded.receive_us - received) as f64 / 1000.0 - f64::from(ticks) / 90.0).abs();
                    self.jitter_ms = if self.jitter_ms < 0.0 { delta } else { self.jitter_ms + (delta - self.jitter_ms) / 16.0 };
                }
                self.fps_evidence_step(ticks);
            }
            self.last_arrival = Some((decoded.rtp, decoded.receive_us));
            self.doctor.arrived(decoded.number, decoded.rtp, decoded.host_latency, decoded.receive_us, self.facts.fps.max(0) as u32);
            // Frames still waiting to be decoded: a queue that stays non-empty makes this
            // picture's lateness the backlog's.
            if self.pacer.mode == PacingMode::HostTimes || self.pacer.host_grid() {
                self.pacer.set_decode_queue(self.session.decode_queue_len());
            }
            self.pacer.accept(decoded);
        }
        self.taken_frames = frames;
        self.observe_slices(&mut slices);
        self.taken_slices = slices;
        if self.pending_mode.is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(5)) {
            // Kept for a picture of its size that arrives late: the PC may apply it after all.
            self.late_mode = self.pending_mode.take().map(|(mode, _)| mode);
            self.fps_evidence = 0;
            // The host applied nothing: rate, size, settings, the request in flight, the bitrate
            // policy and the menu's rate go back together, so no field keeps the asked mode.
            if let Some(before) = self.mode_before.take() {
                before.restore(&mut self.facts.fps, &self.shared.fps_millihz, &mut self.settings, &mut self.requested_mode);
                self.plan.settings = self.settings.clone();
                self.decode_generation += 1;
                self.decode_window_us = session::now_us();
                *lock(&self.shared.decode_window) = pipeline::DecodeWindow::default();
                if let Some(rate) = &mut self.bitrate_policy {
                    rate.set_mode(u32::from(before.requested.width), u32::from(before.requested.height), before.requested.fps_millihz.div_ceil(1000));
                }
                if let Some(rates) = &mut self.rate_change { rates.fps_set(before.fps_millihz); }
                self.stream_rate_changed(before.fps_millihz.div_ceil(1000).max(1));
            }
            let quality_on = self.quality_mode != QualityMode::Off;
            self.quality_policy = None;
            self.quality_mode = QualityMode::Off;
            self.show_toast(unconfirmed_mode_toast(quality_on));
        }
        if self.facts.clock_sync && self.last_probe.elapsed() >= Duration::from_millis(250) {
            self.last_probe = Instant::now();
            let _ = self.session.send_clock_sync_probe(session::now_us());
        }
    }

    /// How long the loop may sleep before the next picture is due.
    pub fn wait(&self) -> Wait {
        let racing = if self.pacer.mode == PacingMode::BeamRace { self.racer.due_us() } else { None };
        let due = match (self.pacer.next_due(), racing) {
            (Some(picture), Some(slice)) => Some(picture.min(slice)),
            (picture, slice) => picture.or(slice),
        };
        let due = match (due, self.skipped.due_us()) {
            (Some(due), Some(skipped)) => Some(due.min(skipped)),
            (due, skipped) => due.or(skipped),
        };
        // Frame Warp's next look at whether the held picture goes out again with a new shift.
        let repeat = self.frame_warp.as_ref()
            .and_then(|warp| warp.repeat_due_us(refresh_period_us(self.plan.display.refresh_hz), self.pacer.mode == PacingMode::BeamRace));
        let due = match (due, repeat) {
            (Some(due), Some(repeat)) => Some(due.min(repeat)),
            (due, repeat) => due.or(repeat),
        };
        // The held picture again, before a VRR panel refreshes by itself.
        let keep = self.keeper.as_ref().filter(|_| self.keeper_on && self.keeper_repeats).and_then(|keeper| keeper.due_us(self.next_picture_us()));
        let due = match (due, keep) {
            (Some(due), Some(keep)) => Some(due.min(keep)),
            (due, keep) => due.or(keep),
        };
        let wait = match due {
            Some(due) => {
                let now = session::now_us();
                if due <= now {
                    Wait::Now
                } else {
                    Wait::For(Duration::from_micros(due - now))
                }
            }
            None => Wait::For(Duration::from_millis(if self.controller_mouse_active || self.guide_waiting || self.audio_status.0.iter().any(|flags| *flags != 0) { 20 } else if self.overlay_visible { 250 } else { 500 })),
        };
        // A hide from the PC waiting out its delay, and a held shortcut's release, wake the loop when due.
        let now = Instant::now();
        let wake = match (self.cursor.hide_in(now), self.shortcut_release_in(now)) {
            (Some(hide), Some(release)) => Some(hide.min(release)),
            (hide, release) => hide.or(release),
        };
        match (wait, wake) {
            (Wait::For(wait), Some(wake)) => Wait::For(wait.min(wake)),
            (wait, _) => wait,
        }
    }

    /// Works out whether the display allows racing slices and, if so, how the picture sits on its
    /// beam; switches the pacing between `Immediate` and `BeamRace` when that changes.
    pub(super) fn sync_race(&mut self, renderer: *mut SDL_Renderer) {
        if !self.facts.beam_race { return; }
        let timing = self.race_probe.as_ref().and_then(race_probe::RaceProbe::racing);
        let size = self.texture.size();
        self.race_scan = timing.filter(|_| size.0 > 0 && size.1 > 0).map(|(beam, delta_ns)| {
            let (mut w, mut h) = (0, 0);
            // SAFETY: the live renderer belongs to the presenting thread.
            unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
            let dst = viewport(size, (w, h), self.settings.video_scaling, self.settings.align_video_top);
            race::Scan::for_picture(&beam, delta_ns, size.1 as u32, dst.y.max(0.0) as u32, dst.h as u32)
        });
        match (self.pacer.mode, self.race_scan.is_some() && self.race_present) {
            (PacingMode::Immediate, true) => {
                self.pacer.mode = PacingMode::BeamRace;
                eprintln!("nightjar: beam race: presenting slices ahead of the beam");
            }
            (PacingMode::BeamRace, false) => {
                self.pacer.mode = PacingMode::Immediate;
                // The slices it held let go of their plane sets now, not at the next frame.
                self.racer.reset();
                eprintln!("nightjar: beam race: the display no longer allows racing, presenting whole pictures");
            }
            _ => {}
        }
    }

    /// Refreshes each V-Sync picture is held for (fractional V-Sync,
    /// `nightjar_policy::vsync::refreshes_per_frame`): the multiple the display's refresh is of the
    /// stream's rate, with the setting on, V-Sync pacing and VRR not chosen (VRR shows pictures as
    /// they come; chosen, the settings screen hides the row, also where the display falls back).
    /// The stream holds each picture itself (`hold_for_turn`), whatever the swap chain's sync interval.
    fn vsync_refreshes(&mut self) -> u32 {
        let Some(refresh_hz) = self.plan.display.reported_refresh_hz() else { return 1 };
        let fps = f64::from(self.shared.fps_millihz.load(Ordering::Relaxed)) / 1000.0;
        let vrr = present::fractional_vsync_stands_aside(self.plan.vrr, self.pacer.mode, self.settings.host_frame_times);
        let refreshes = nightjar_policy::vsync::refreshes_per_frame(f64::from(refresh_hz), fps, vrr, self.settings.fractional_vsync);
        if refreshes <= 1 { return refreshes; }
        match self.held_times() {
            phase::HeldTimes::Told => refreshes,
            // Wayland's feedback before its first answer was due: nothing to say yet.
            phase::HeldTimes::Unknown => 1,
            phase::HeldTimes::Untold => {
                if !std::mem::replace(&mut self.held_untimed_logged, true) {
                    eprintln!("nightjar: pacing: fractional V-Sync needs the times the display shows pictures at (Windows' swap chain statistics, Wayland's presentation feedback, KMS), none here: each picture shows at the next refresh");
                }
                1
            }
        }
    }

    /// Whether this display tells when it showed a picture, which fractional V-Sync's turns count
    /// from: Windows' swap chain statistics while they can be read, a KMS commit, or Wayland's
    /// presentation feedback with times in the last second.
    fn held_times(&self) -> phase::HeldTimes {
        #[cfg(windows)]
        { self.held_stats.times(session::now_us()) }
        #[cfg(target_os = "linux")]
        { if self.kms_engaged() { phase::HeldTimes::Told } else { self.glass.times(session::now_us()) } }
        #[cfg(not(any(windows, target_os = "linux")))]
        { phase::HeldTimes::Untold }
    }

    /// The deadline a held picture's turn must make: the desktop's, or 2 ms in Windows' exclusive
    /// fullscreen, which nothing composes (`NIGHTJAR_HELD_MARGIN_US` sets this base for a
    /// measurement). It only says which turn a picture that comes late takes: the present itself
    /// goes out just after the refresh that opens the turn, and the phase report aims the host's
    /// frames at that present lead (`phase::held_present_lead_us`), not at this deadline. With Frame
    /// Warp the hold comes before the draw, so the draw's recent time is added
    /// (`present::held_margin_us`).
    fn held_margin_us(&self, renderer: *mut SDL_Renderer) -> u64 {
        #[cfg(target_os = "linux")]
        if self.kms_engaged() { return present::held_base_margin_us(self.held_margin_us, true, self.plan.display.refresh_hz); }
        let uncomposed = present::uncomposed_fullscreen(exclusive_fullscreen(renderer), cfg!(windows));
        let refresh_hz = self.plan.display.refresh_hz;
        let base_us = present::held_base_margin_us(self.held_margin_us, uncomposed, refresh_hz);
        let draw_us = if self.frame_warp.is_some() { self.held_draw.p90_us() } else { None };
        present::held_margin_us(base_us, draw_us, (1e6 / f64::from(refresh_hz.max(1.0))) as u64)
    }

    /// Fractional V-Sync: waits before the picture's present until the refresh that opens its turn
    /// comes (`phase::HeldAnchor::hold`), so the swap chain latches the flip into the turn; its
    /// margin (`held_margin_us`) is the desktop's deadline the turn must make, and its guard
    /// (`held_guard_us`) how long after the opening refresh the present goes out.
    fn hold_for_turn(&mut self, renderer: *mut SDL_Renderer) {
        let held = self.vrr.held(self.pacer.mode);
        if held <= 1 { return; }
        let margin_us = self.held_margin_us(renderer);
        let refresh_hz = self.plan.display.refresh_hz;
        let wait_us = if cfg!(windows) {
            // Without the statistics nothing is held: the next pictures show at the next refresh
            // and the phase report aims at it (`held_times`).
            let wait = phase::held_wait_us(renderer, refresh_hz, held, margin_us, self.held_guard_us, &mut self.held_anchor);
            self.held_stats.read(wait.is_some(), session::now_us());
            wait.unwrap_or(0)
        } else {
            self.held_clock.hold(session::now_us(), held, (1e6 / f64::from(refresh_hz.max(1.0))) as u64, margin_us,
                self.held_guard_us)
        };
        self.held_wait_us = Some(wait_us);
        if wait_us > 0 {
            // SAFETY: a plain wait.
            unsafe { SDL_DelayPrecise(wait_us * 1000) };
        }
    }

    /// `hold_for_turn` before the present lends the input, as Frame Warp's hold before its draw
    /// does: keys, clicks and controllers leave from their own threads while it waits
    /// (`direct_motion`), so Frame Warp's shift takes them in. Lent only for the wait: the draws
    /// after it, the overlay's with the lent controllers among them, run with the main loop's input
    /// as without the hold, and the present lends it again for its own wait (`render_flip`).
    fn hold_for_turn_lending(&mut self, renderer: *mut SDL_Renderer, window: *mut SDL_Window) {
        if self.vrr.held(self.pacer.mode) <= 1 { return; }
        self.direct_motion.presenting(Some(self.direct_rules(window)));
        self.hold_for_turn(renderer);
        self.direct_motion.presenting(None);
    }

    /// Turns a VRR stream to the pacing it would have without VRR once the display check finds the
    /// display scanning at its own rate (`present::VrrPacing`), and gives the renderer the sync
    /// interval the pacing and `NIGHTJAR_VRR_SYNC` ask for.
    pub(super) fn sync_vrr(&mut self, renderer: *mut SDL_Renderer) {
        let before = self.pacer.mode;
        let display_vrr = self.vrr.display_vrr();
        let now = session::now_us();
        // Fractional V-Sync follows the display's refresh and the stream's rate as they change.
        let held = self.vrr.held(self.pacer.mode);
        let refreshes = self.vsync_refreshes();
        // The turns are on a new grid, and a display mode may restart its refresh count: the old
        // grid's aim and lateness say nothing about them. A stream at one rate keeps its turns.
        self.held_anchor.restart_on_rate_change(self.vrr.refreshes, refreshes);
        self.vrr.set_refreshes(refreshes);
        let probe = self.race_probe.as_ref();
        // Once per hold: a swap chain whose statistics failed tries the hold again every few seconds.
        if self.vrr.held(self.pacer.mode) != held && self.vrr.refreshes > 1
            && std::mem::replace(&mut self.held_logged, self.vrr.refreshes) != self.vrr.refreshes {
            eprintln!("nightjar: pacing: fractional V-Sync, each picture shown for {} refreshes", self.vrr.refreshes);
        }
        if let Some(probe) = probe { Self::distrust_scan_line(&mut self.vrr, probe, self.settings.vrr_wait); }
        let interval = self.vrr.step(&mut self.pacer.mode, probe.map(race_probe::RaceProbe::verdict), now);
        // MAILBOX for VRR on Wayland only while the display refreshes on present; a fixed refresh
        // one tears like any stream without V-Sync (`prefers_immediate`). SDL picks the mode
        // again when it is given a sync interval, the current one if it did not change.
        #[cfg(target_os = "linux")]
        let interval = if self.vrr.display_vrr() != display_vrr {
            let wayland = crate::sdl::video_driver() == "wayland";
            crate::sdl::prefer_immediate(renderer, present::prefers_immediate(self.vrr.display_vrr(), wayland, crate::handheld::gamescope()));
            interval.or(Some(self.vrr.interval))
        } else { interval };
        #[cfg(not(target_os = "linux"))]
        let _ = display_vrr;
        if self.pacer.mode != before {
            let line = probe.map_or(String::new(), race_probe::RaceProbe::line);
            if self.vrr.fell_back {
                eprintln!("nightjar: pacing: {}, this display does not refresh on present (display check: {line})", self.pacer.mode.label());
            } else {
                eprintln!("nightjar: pacing: {}, the display refreshes on present again (display check: {line})", self.pacer.mode.label());
            }
        }
        if self.vrr.recheck_due(now) {
            if let Some(probe) = self.race_probe.as_mut() { probe.recheck(); }
        }
        if let Some(interval) = interval {
            // SAFETY: the live renderer belongs to the presenting thread.
            let set = unsafe { SDL_SetRenderVSync(renderer, interval) };
            if !set {
                eprintln!("nightjar: pacing: could not set the sync interval to {interval}: {}", crate::sdl::error());
            }
            #[cfg(target_os = "linux")]
            if set {
                eprintln!("nightjar: pacing: sync interval {interval}, present mode {}", crate::sdl::present_mode_name(crate::sdl::present_mode(renderer)));
            }
        }
    }

    /// Turns Tear-free VRR to sync interval 1 once the display check finds presents to an idle
    /// display that start no scan (`scan_sync::UNTRUSTED_MISSES`), or a stream of this run found
    /// that on the same display, and says so once (`present::VrrPacing::distrust_scan`).
    fn distrust_scan_line(vrr: &mut present::VrrPacing, probe: &race_probe::RaceProbe, wait: crate::config::VrrWait) {
        let display = probe.display_id();
        // Nightjar's wait does not take the display check's switch to Windows V-Sync, so choosing
        // it forgets a display an earlier stream found not refreshing on present: Automatic starts
        // afresh there. (A wait that would cost the next picture its turn still presents with sync
        // interval 1, `scan_sync::scan_wait`; that is the wait's own budget, not this verdict.)
        if wait == crate::config::VrrWait::Scan {
            if let Some(display) = display { scan_sync::clear_untrusted(display); }
        }
        let scan = vrr.sync == present::VrrSync::Scan;
        if !scan || vrr.sync_forced { return; }
        let check = probe.restart_check();
        let marked = display.is_some_and(scan_sync::untrusted);
        let distrust = scan_sync::distrusts(check.missed, marked, scan, vrr.sync_forced);
        if let (true, Some(display)) = (distrust.remember, display) { scan_sync::remember_untrusted(display); }
        if !distrust.turn || !vrr.distrust_scan() { return; }
        let why = if distrust.remember {
            format!("{} of {} presents to an idle display started no scan within {} ms", check.missed, check.tested, scanline::RESTART_WINDOW_NS / 1_000_000)
        } else {
            "an earlier stream found presents to this display starting no scan".to_owned()
        };
        eprintln!("nightjar: VRR: tear-free presents wait for the refresh (sync interval 1) from here on: {why}, so the scan line does not tell when the display shows a picture");
    }

    /// Before a picture's present: what the display is doing goes to the keeper, and while the
    /// beam is in the picture's rows the present waits for it to leave the last one, so the picture
    /// is shown whole. That wait is for `VrrSync::Scan` and for Real VRR on a display the keeper
    /// repeats pictures for, where a scan under way is rare; a stream that shows pictures as they
    /// come tears instead, as it asked to. Returns whether the present goes out with sync interval 1
    /// instead (`scan_sync::scan_wait`): waiting for the scan and its landing would have run past
    /// the next picture's due time, and sync interval 1 shows this one whole at the next refresh.
    ///
    /// The beam alone says too little (`scan_sync`): `VrrSync::Scan` also waits for the present
    /// before this one to start its scan while that one is still on its way, and has the GPU
    /// finish the drawing of a picture that waits, so that its present is all that is left.
    fn wait_out_scan(&mut self, renderer: *mut SDL_Renderer, due_us: Option<u64>) -> bool {
        // The tearing modes that present whole pictures; the beam race times its own presents.
        if !matches!(self.pacer.mode, PacingMode::Immediate | PacingMode::HostTimes) { return false; }
        let keeping = self.keeper_on && self.keeper.is_some();
        let syncs = self.vrr.sync == present::VrrSync::Scan;
        if !syncs && !keeping { return false; }
        let Some(beam) = self.race_probe.as_ref().and_then(race_probe::RaceProbe::beam) else { return false };
        let Some(mut sample) = self.race_probe.as_ref().and_then(race_probe::RaceProbe::before) else { return false };
        if let Some(keeper) = self.keeper.as_mut().filter(|_| keeping) {
            let seen = keeper.observe(session::now_us(), beam.since_scan_ns(&sample) / 1000, beam.row_of(&sample).is_some(), true);
            if let Some(floor) = seen.floor_us { eprintln!("nightjar: VRR low frame rate: the panel refreshes by itself after {:.1} ms", floor as f64 / 1e3); }
        }
        if !syncs && (self.pacer.mode != PacingMode::HostTimes || !self.keeper_repeats) { return false; }
        // A picture that has to wait has its drawing finished meanwhile, so that its present is
        // all that is left when it goes out. One that meets an idle display goes out at once:
        // finished first, it reached a 240 Hz display 1.5 ms later at the median on uneven content.
        if syncs && self.scan_settles && (beam.scan_left_ns(&sample).is_some() || self.flip_on_its_way(&beam, &sample).is_some()) {
            // SAFETY: the live renderer of this thread.
            match unsafe { scan_sync::finish_drawing(renderer, Duration::from_nanos(scan_sync::DRAWING_LIMIT_NS)) } {
                Some(took) => self.scan_tally.drawing.push(took.as_nanos() as u64),
                None => self.scan_tally.undrawn += 1,
            }
            match self.race_probe.as_ref().and_then(race_probe::RaceProbe::before) {
                Some(reading) => sample = reading,
                None => return false,
            }
        }
        if syncs { sample = self.wait_for_landing(&beam, sample); }
        // The next picture is taken to be due one frame after this one's. A wait whose landing
        // would pass that time would show this picture after the next one is due, which can then
        // replace it before it is shown; at a stream rate equal to the display's each picture
        // starts a scan a landing after the last one ends (`scan_sync::scan_wait`). Sync interval
        // 1 shows this picture whole at the next refresh instead, and the present returns there,
        // without a landing after it.
        if syncs {
            if let (Some(left), Some(due)) = (beam.scan_left_ns(&sample), due_us) {
                let budget_ns = scan_sync::wait_budget_ns(due, self.pacer.frame_us(), session::now_us());
                let landing_ns = self.scan_tally.landing.median_ns().unwrap_or(scan_sync::TEAR_LANDING_NS);
                if scan_sync::scan_wait(left, budget_ns, landing_ns) == scan_sync::ScanWait::Latch { return true; }
            }
        }
        // Until the beam leaves the last row, reading it again after each sleep: at most one scan
        // plus `SCAN_WAIT_SLACK_NS`, since a reading can be wrong (`scan_sync::wait_out`).
        let begun = Instant::now();
        let probe = self.race_probe.as_ref();
        let Some(waited) = scan_sync::wait_out(&beam, sample, || probe.and_then(race_probe::RaceProbe::before),
            |ns| std::thread::sleep(Duration::from_nanos(ns)), || begun.elapsed().as_nanos() as u64) else { return false };
        if waited.cut_short { self.scan_tally.cut_short += 1; }
        if waited.wrapped { self.scan_tally.wrapped += 1; }
        self.scan_waits.0 += 1;
        self.scan_waits.1 += waited.ns;
        false
    }

    /// How long ago the present before this one was issued (ns), while it is still on its way to a
    /// display that `sample` reads as idle (`scan_sync::on_its_way`); None once it has started its
    /// scan, or `scan_sync::LANDING_LIMIT_NS` after it.
    fn flip_on_its_way(&self, beam: &scanline::Beam, sample: &scanline::Sample) -> Option<u64> {
        let since_present_ns = session::now_us().saturating_sub(self.flip_guard.last_flip_us?) * 1000;
        let idle = beam.scan_left_ns(sample).is_none();
        (idle && since_present_ns < scan_sync::LANDING_LIMIT_NS && scan_sync::on_its_way(beam.since_scan_ns(sample), since_present_ns, beam.scan_ns))
            .then_some(since_present_ns)
    }

    /// The present before this one, seen from `sample` (a reading of the beam just now): timed if
    /// it has started its scan, and waited for while it is still on its way to an idling display,
    /// where a second present would land in the scan the first one starts
    /// (`scan_sync::on_its_way`). Returns the reading to go on with: of the scan that began, or
    /// the last one of a wait that saw none begin within `scan_sync::LANDING_LIMIT_NS` of that
    /// present.
    fn wait_for_landing(&mut self, beam: &scanline::Beam, sample: scanline::Sample) -> scanline::Sample {
        let latched_before = self.flip_guard.last_flip_latched;
        let Some(since_present_ns) = self.flip_on_its_way(beam, &sample) else {
            if let Some(last_us) = self.flip_guard.last_flip_us {
                let since_present_ns = session::now_us().saturating_sub(last_us) * 1000;
                let took = scan_sync::landing_ns(beam.since_scan_ns(&sample), since_present_ns);
                self.scan_tally.landing_measured(took, latched_before);
            }
            self.scan_tally.found_landed();
            return sample;
        };
        self.scan_tally.behind += 1;
        if !self.scan_settles || !self.scan_tally.waits() { return sample; }
        let Some(probe) = &self.race_probe else { return sample };
        // Spinning, as the wait for the scan's end does: a flip lands within a millisecond or so.
        let begun = Instant::now();
        let limit = Duration::from_nanos(scan_sync::LANDING_LIMIT_NS - since_present_ns);
        let mut last = sample;
        let landed = loop {
            if let Some(reading) = probe.before() {
                last = reading;
                if beam.scan_left_ns(&last).is_some() { break true; }
            }
            if begun.elapsed() >= limit { break false; }
            std::hint::spin_loop();
        };
        let waited = begun.elapsed().as_nanos() as u64;
        if landed {
            let took = scan_sync::landing_ns(beam.since_scan_ns(&last), since_present_ns + waited);
            self.scan_tally.landing_measured(took, latched_before);
        }
        if self.scan_tally.waited_for(waited, landed) {
            eprintln!("nightjar: VRR: {} presents in a row started no scan within {} ms, the next {} do not wait for the one before",
                scan_sync::GIVE_UPS_IN_A_ROW, scan_sync::LANDING_LIMIT_NS / 1_000_000, scan_sync::REST_PRESENTS);
        }
        last
    }

    /// `VrrSync::Blank` on Windows: presents with tearing only when the display is in its vertical
    /// blank and the last present has left the queue, otherwise with sync interval 1. Until the scan
    /// line can be read no present tears, as upstream has it: unknown scanout authorises no tearing
    /// flip. Returns whether it presented; false leaves the present to the caller.
    fn blank_present(&mut self, renderer: *mut SDL_Renderer, probe: bool) -> bool {
        if !cfg!(windows) || !probe || self.vrr.sync != present::VrrSync::Blank
            || !matches!(self.pacer.mode, PacingMode::Immediate | PacingMode::HostTimes) { return false; }
        let now = session::now_us();
        let since = self.flip_guard.last_flip_us.map(|last| now.saturating_sub(last));
        // The display's mode gives the period until the scan is calibrated.
        let period_us = self.race_probe.as_ref().and_then(race_probe::RaceProbe::beam)
            .map_or_else(|| refresh_period_us(self.plan.display.refresh_hz), |beam| beam.scan_ns / 1000);
        let planned = present::planned_latch(since, period_us);
        let probe_ref = self.race_probe.as_ref();
        let latched = present::latch(planned, period_us, || {
            // SAFETY: the renderer is live on this thread.
            let (predecessor_known, statistics_valid, pending) = unsafe { present::flip_statistics(renderer) };
            let mut observation = present::FlipObservation { predecessor_known, statistics_valid, pending, ..Default::default() };
            if statistics_valid && !pending {
                let read_start = session::now_us();
                let sample = probe_ref.and_then(race_probe::RaceProbe::before);
                let read_end = session::now_us();
                observation.raster_valid = sample.is_some();
                observation.in_vertical_blank = sample.is_some_and(|sample| sample.vblank);
                observation.raster_query_start_us = read_start;
                observation.raster_query_end_us = read_end;
            }
            observation
        }, session::now_us);
        if planned {
            self.flip_guard.planned += 1;
        } else if latched {
            self.flip_guard.protected += 1;
        } else {
            self.flip_guard.tearing += 1;
        }
        // SAFETY: a live renderer.
        unsafe {
            if !present::present_adaptive(renderer, latched) { SDL_RenderPresent(renderer); }
        }
        true
    }

    /// Takes the slices the decoder finished since the last call into the racer.
    fn observe_slices(&mut self, slices: &mut VecDeque<pyrowave::SliceReady>) {
        // Anything the racer does not take is dropped here, which lets go of its plane set.
        let (Some(scan), PacingMode::BeamRace, Some(_)) = (self.race_scan, self.pacer.mode, self.race_setup) else {
            slices.clear();
            return;
        };
        let margin = self.latency.margin_us(self.race_margin_ms);
        let now = session::now_us();
        let facts = &self.facts;
        let predictor = self.predictor.get_or_insert_with(|| seeded_predictor(facts));
        for ready in slices.drain(..) {
            self.racer.observe(ready.seq, ready.band, ready.pin, now, &scan, predictor, &self.latency, margin);
        }
    }

    /// Presents the slices of the frame being received that are due: the first at the planned
    /// scan start, the others as they are ready. Returns whether it presented.
    #[cfg(windows)]
    fn present_slices(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), now: u64) -> bool {
        if self.pacer.mode != PacingMode::BeamRace { return false; }
        let (Some(scan), Some(setup)) = (self.race_scan, self.race_setup) else { return false };
        // The picture is decoded already: its own present shows the rows, a slice present first
        // would only keep it waiting (a present costs 0.4 ms of this thread).
        if self.pacer.front_race().is_some_and(|(seq, due)| seq != 0 && seq == self.racer.seq() && due <= now) { return false; }
        let batch = self.racer.take(now);
        if batch.is_empty() { return false; }
        for run in race::runs(batch.iter().map(|(band, pin)| (*band, pin.set()))) {
            let Some(rows) = race::scissor(run.first_row, run.end_row, setup.height as u32) else { continue };
            if !self.texture.upload_slice(renderer, setup, &batch[run.last].1, run.fence_value, rows) {
                self.end_pyrowave_unshown();
                return true;
            }
        }
        // One shift for all of a frame's slices and its own present (`Presenter::draw`).
        if let Some(warp) = self.frame_warp.as_mut() { self.texture.warp = warp.draw(Some(self.racer.seq())); }
        let flipped = self.render_flip(renderer, draw_overlay, true, false, false, None);
        self.needs_redraw = false;
        self.racer.presented(flipped.issued_us, &scan);
        for (band, _) in &batch {
            self.latency.record(self.racer.presentable_us(band.ready_us), flipped.issued_us, flipped.returned_us, flipped.returned_us);
        }
        true
    }

    #[cfg(not(windows))]
    fn present_slices(&mut self, _renderer: *mut SDL_Renderer, _draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), _now: u64) -> bool {
        false
    }

    /// The PC's frame times at V-Sync: the display's refresh grid and the desktop's deadline before
    /// a refresh, for the pictures accepted next (`Pacer::set_grid`). Exclusive fullscreen, which
    /// nothing composes, has no deadline before the refresh.
    fn sync_host_grid(&mut self, renderer: *mut SDL_Renderer) {
        let refresh_hz = self.plan.display.refresh_hz;
        let deadline_us = if exclusive_fullscreen(renderer) { 0 } else {
            self.present_deadline_us.unwrap_or_else(|| present::deadline_before_refresh(refresh_hz))
        };
        // The refresh grid itself: a hold of 1 never reads the anchor.
        let grid = phase::timing(renderer, refresh_hz, 1, &mut self.held_anchor).map(|(refresh_us, period_ns)| present::refresh_grid(refresh_us, period_ns));
        self.pacer.set_grid(grid, deadline_us);
    }

    /// When the next picture that already waits is due.
    fn next_picture_us(&self) -> Option<u64> {
        match (self.pacer.next_due(), self.skipped.due_us()) {
            (Some(due), Some(skipped)) => Some(due.min(skipped)),
            (due, skipped) => due.or(skipped),
        }
    }

    /// Turns the keeper on while the stream tears whole pictures onto a display the check found
    /// refreshing on present and whose scan it calibrated, and off otherwise (`lfc::Keeper`).
    pub(super) fn sync_keeper(&mut self) {
        let scan_us = (self.vrr.lfc && matches!(self.pacer.mode, PacingMode::Immediate | PacingMode::HostTimes) && self.vrr.display_vrr())
            .then(|| self.race_probe.as_ref().filter(|probe| probe.verdict().refreshes_on_present()).and_then(race_probe::RaceProbe::beam))
            .flatten()
            .map(|beam| beam.scan_ns / 1000);
        match (scan_us, self.keeper.as_mut()) {
            (Some(scan_us), Some(keeper)) => keeper.set_scan_us(scan_us),
            (Some(scan_us), None) => self.keeper = Some(lfc::Keeper::new(scan_us)),
            (None, Some(keeper)) if self.keeper_on => keeper.forget(),
            (None, _) => {}
        }
        if scan_us.is_some() != self.keeper_on {
            self.keeper_on = scan_us.is_some();
            match scan_us {
                Some(scan_us) if self.keeper_repeats => eprintln!("nightjar: VRR low frame rate: the stream repeats pictures before the panel does (NIGHTJAR_VRR_LFC=1; a scan takes {:.2} ms)", scan_us as f64 / 1e3),
                Some(_) => eprintln!("nightjar: VRR low frame rate: counting the pictures that meet a scan (NIGHTJAR_VRR_LFC=count), the display repeats pictures itself"),
                None => eprintln!("nightjar: VRR low frame rate: off, the display's scan is not known"),
            }
        }
    }

    /// Whether the held picture goes to the display again now, so a VRR panel does not refresh
    /// by itself into the next picture (`lfc::Keeper`). The scan line is read first: a scan under
    /// way, the panel's own or one that began late, is the refresh already.
    fn lfc_repeat(&mut self, now: u64) -> bool {
        if !self.keeper_on || !self.keeper_repeats || !self.first_frame || self.pacer.mode == PacingMode::BeamRace { return false; }
        let next = self.next_picture_us();
        let reading = self.race_probe.as_ref().and_then(|probe| Some((probe.beam()?, probe.before()?)));
        let Some(keeper) = self.keeper.as_mut() else { return false };
        if !keeper.due_us(next).is_some_and(|due| now >= due) { return false; }
        let Some((beam, sample)) = reading else {
            // The scan line is gone: nothing says when the panel refreshes.
            keeper.forget();
            return false;
        };
        let since_scan_us = beam.since_scan_ns(&sample) / 1000;
        let scanning = beam.row_of(&sample).is_some();
        let seen = keeper.observe(now, since_scan_us, scanning, false);
        if let Some(floor) = seen.floor_us { eprintln!("nightjar: VRR low frame rate: the panel refreshes by itself after {:.1} ms", floor as f64 / 1e3); }
        if scanning {
            keeper.scan_under_way(now, since_scan_us);
            return false;
        }
        !seen.foreign
    }

    /// Whether Frame Warp shows the held picture again now, its shift having moved
    /// (`frame_warp::Repeat`): at most once a refresh, never into the refresh a picture that is
    /// waiting or expected is due for, and not while slices race the beam (their frame keeps the
    /// shift of its first slice).
    fn warp_repeat(&mut self, now: u64) -> bool {
        if self.frame_warp.is_none() || self.pacer.mode == PacingMode::BeamRace { return false; }
        let next_picture_us = self.next_picture_us();
        let timing = frame_warp::Timing {
            refresh_us: refresh_period_us(self.plan.display.refresh_hz),
            vsync: self.pacer.mode == PacingMode::Vsync,
            next_picture_us,
        };
        let frame_us = self.pacer.frame_us();
        self.frame_warp.as_mut().is_some_and(|warp| warp.repeat_now(now, timing, frame_us))
    }

    /// The current picture through AI enhancement at `output`, or None to draw the picture itself:
    /// a picture it does not read (software-decoded, PyroWave, packed), or the mode failed, which
    /// is logged and shown once and leaves the stream on bilinear scaling.
    #[cfg(windows)]
    pub(super) fn enhanced(&mut self, renderer: *mut SDL_Renderer, output: (i32, i32)) -> Option<*mut SDL_Texture> {
        if self.upscale_failed { return None; }
        let Some(source) = self.texture.enhance_source() else { self.enhancer = None; return None };
        if output.0 <= 0 || output.1 <= 0 || output.0 > 16384 || output.1 > 16384 { return None; }
        let key = enhance::Key { source, output, hdr_display: crate::sdl::hdr_available(renderer) };
        if !self.enhancer.as_ref().is_some_and(|enhancer| enhancer.matches(&key)) {
            self.enhancer = None;
            let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
            let made = enhance::stream_probe(device)
                .and_then(|capability| capability.ok_or_else(|| "this GPU has neither video super resolution nor RTX Video HDR".to_owned()))
                .and_then(|capability| enhance::Enhancer::new(renderer, key, capability, self.facts.fps.max(1) as u32));
            match made {
                Ok(enhancer) => {
                    eprintln!("nightjar: {} for {}x{} to {}x{}", enhancer.label, source.picture.0, source.picture.1, output.0, output.1);
                    self.enhancer = Some(enhancer);
                }
                Err(error) => return self.enhance_failed(error),
            }
        }
        match self.enhancer.as_mut()?.draw(renderer) {
            Ok(texture) => Some(texture),
            Err(error) => self.enhance_failed(error),
        }
    }

    #[cfg(not(windows))]
    pub(super) fn enhanced(&mut self, _renderer: *mut SDL_Renderer, _output: (i32, i32)) -> Option<*mut SDL_Texture> {
        None
    }

    /// Whether AI enhancement draws the current picture (`ai_in_effect`); where it does not, its
    /// enhancer goes.
    #[cfg(windows)]
    fn ai_enhances(&mut self) -> bool {
        let ai = ai_in_effect(self.settings.upscaling, self.upscale_failed, self.texture.enhance_source().is_some());
        if !ai { self.enhancer = None; }
        ai
    }

    /// AI enhancement runs on Direct3D 11 only.
    #[cfg(not(windows))]
    fn ai_enhances(&mut self) -> bool {
        ai_in_effect(self.settings.upscaling, self.upscale_failed, false)
    }

    #[cfg(windows)]
    fn enhance_failed(&mut self, error: String) -> Option<*mut SDL_Texture> {
        eprintln!("nightjar: AI enhancement failed: {error}; the stream continues with bilinear scaling");
        self.enhancer = None;
        self.upscale_failed = true;
        self.show_toast(tr("AI enhancement is unavailable; using bilinear scaling."));
        None
    }

    /// Temporal upscaling was refused or failed: SGSR 1 stands in for the rest of the stream,
    /// said in the log, once in a toast and in the stats line.
    fn temporal_unavailable(&mut self, reason: &str) {
        eprintln!("nightjar: temporal upscaling is unavailable: {reason}; the stream continues with SGSR 1");
        self.temporal_failed = true;
        self.show_toast(tr("Temporal upscaling is unavailable; using SGSR 1."));
    }

    /// The stats line for the upscaling setting.
    pub(super) fn upscaling_line(&self) -> String {
        if self.settings.upscaling == config::Upscaling::Ai {
            #[cfg(windows)]
            if let Some(enhancer) = &self.enhancer { return enhancer.label.clone(); }
            return if self.upscale_failed { "Bilinear (AI enhancement unavailable)".into() }
                else if cfg!(windows) { "Bilinear (not a hardware-decoded picture)".into() }
                else { "Bilinear (AI enhancement needs Direct3D 11)".into() };
        }
        #[cfg(target_os = "macos")]
        if self.settings.upscaling == config::Upscaling::MetalFx {
            return match (&self.metalfx, self.metalfx_blocked) {
                (Some(pass), None) => format!("MetalFX ({:.2} ms GPU)", pass.gpu_ms),
                (_, Some(reason)) => format!("Bilinear ({reason})"),
                (None, None) => "Bilinear (no picture yet)".into(),
            };
        }
        if self.upscale_failed { "Bilinear (GPU unavailable)".into() }
        else if self.texture.is_hdr() { "Bilinear (HDR)".into() }
        else if self.upscaler.is_some() { upscale::running_label(self.settings.upscaling, self.temporal_failed, self.frame_warp.is_some()).into() }
        else { "Bilinear (no upscale needed)".into() }
    }

    /// Draws the picture itself into the back buffer (`direct` on Windows, the plane pass on
    /// Linux), or returns false for SDL to draw the video texture.
    #[cfg(any(windows, unix))]
    pub(super) fn draw_direct(&mut self, renderer: *mut SDL_Renderer, dst: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        self.texture.draw_direct(renderer, dst, texture)
    }

    #[cfg(not(any(windows, unix)))]
    pub(super) fn draw_direct(&mut self, _renderer: *mut SDL_Renderer, _dst: &SDL_FRect, _texture: *mut SDL_Texture) -> bool {
        false
    }

    /// Clears the window's target for a frame, black. On a PQ output, a frame that will draw only
    /// the picture with the plane pass (`unscaled`) and what of the overlay's writes PQ too asks the patched SDL
    /// for a video-only frame (`present::VideoOnly`): SDL settles it on the frame's first run of
    /// commands on the window's target, which the flush of the clear is (a clear takes no swapchain
    /// image, so V-Sync's wait stays at the frame's first draw). The property is true only around
    /// that flush, so nothing else (the launcher, a later frame) inherits it; the plane pass writes
    /// PQ only when SDL reports the frame video only, and scRGB otherwise.
    ///
    /// # Safety
    /// `renderer` is the live renderer of the presenting thread.
    #[cfg(target_os = "linux")]
    unsafe fn clear_window(&mut self, renderer: *mut SDL_Renderer, unscaled: bool) {
        let props = SDL_GetRendererProperties(renderer);
        let pq_output = crate::sdl::output_decision() == Some(Ok(crate::handheld::Output::Pq));
        let planes_direct = unscaled && self.texture.draws_planes_direct();
        // What `draw_overlay` in app/overlay.rs draws: nothing, or egui meshes, which write PQ themselves
        // (`ui_pq`) where a frame could be video only but for them.
        let meshes = self.overlay_visible || self.toast.is_some() || self.menu_visible;
        let frame = present::VideoOnlyFrame {
            pq_output,
            planes_direct,
            nothing_else: self.facts.role != nightjar_core::http::AppRole::InputOnly
                && (!meshes || (pq_output && planes_direct && self.texture.ui_draws_pq(renderer))),
            read_back: SDL_GetBooleanProperty(props, READ_BACK_PROPERTY.as_ptr(), false),
        };
        let (asked, line) = self.texture.video_only().wanted(frame, session::now_us());
        if let Some(line) = line { eprintln!("{line}"); }
        if asked {
            SDL_SetBooleanProperty(props, VIDEO_ONLY_FRAME_PROPERTY.as_ptr(), false);
            SDL_SetBooleanProperty(props, VIDEO_ONLY_PROPERTY.as_ptr(), true);
        }
        SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
        SDL_RenderClear(renderer);
        let mut pq_target = false;
        if asked {
            // SDL_FlushRenderer's failure leaves the frame undecided, which the reset below settles
            // as not video only; the settled value says which it was either way.
            SDL_FlushRenderer(renderer);
            SDL_SetBooleanProperty(props, VIDEO_ONLY_PROPERTY.as_ptr(), false);
            pq_target = SDL_GetBooleanProperty(props, VIDEO_ONLY_FRAME_PROPERTY.as_ptr(), false);
            if let Some(line) = self.texture.video_only().settled(pq_target) { eprintln!("{line}"); }
        }
        self.texture.set_pq_target(pq_target);
    }

    #[cfg(not(target_os = "linux"))]
    unsafe fn clear_window(&mut self, renderer: *mut SDL_Renderer, _unscaled: bool) {
        SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
        SDL_RenderClear(renderer);
    }

    /// The render state `draw_overlay` in app/overlay.rs hands egui's painter for this frame: null unless
    /// the frame is video only on a PQ output, where SDL would drop its own draws (`ui_pq`).
    #[cfg(target_os = "linux")]
    pub fn ui_state(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_GPURenderState {
        // SAFETY: the live renderer of the presenting thread.
        unsafe { self.texture.ui_pq_state(renderer) }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn ui_state(&mut self, _renderer: *mut SDL_Renderer) -> *mut SDL_GPURenderState {
        std::ptr::null_mut()
    }

    /// After a present: a video-only frame in which SDL dropped draws of its own holds video-only
    /// frames off (`present::VideoOnly::presented`).
    #[cfg(target_os = "linux")]
    fn video_only_presented(&mut self, renderer: *mut SDL_Renderer) {
        self.texture.set_pq_target(false);
        // SAFETY: a live renderer and a property name constant.
        let dropped = unsafe { SDL_GetNumberProperty(SDL_GetRendererProperties(renderer), VIDEO_ONLY_DROPPED_PROPERTY.as_ptr(), 0) };
        if let Some(line) = self.texture.video_only().presented(dropped, session::now_us()) { eprintln!("{line}"); }
    }

    /// Ends a stream whose PyroWave pictures cannot be shown; the reason goes to the log.
    fn end_pyrowave_unshown(&mut self) {
        eprintln!("nightjar: PyroWave pictures cannot be shown: {}", self.texture.failure.take().as_deref().unwrap_or("GPU reconstruction failed"));
        self.exit = Some(Exit::Ended(Some(tr("PyroWave pictures cannot be shown on this device. Choose another Video codec in Settings, then start the stream again.").to_owned())));
    }

    /// Draws the texture and the overlay and presents. `fresh` is a whole newly decoded picture,
    /// which advances temporal upscaling; `reset` drops the history it would advance, for a
    /// picture that does not follow the last one drawn (`temporal::history_stale`). `probe` reads
    /// the beam around the present; `due_us` is when the picture was due, for the wait's budget
    /// against the next one.
    fn render_flip(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), probe: bool, fresh: bool, reset: bool, due_us: Option<u64>) -> Flipped {
        // SAFETY: drawing on a live renderer.
        unsafe {
            let (mut w, mut h) = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h);
            let dst = viewport(self.texture.size(), (w, h), self.settings.video_scaling, self.settings.align_video_top);
            let output = (dst.w as i32, dst.h as i32);
            let input = self.texture.size();
            #[cfg(target_os = "macos")]
            if self.settings.upscaling != config::Upscaling::MetalFx { self.metalfx = None; }
            let use_upscaler = self.settings.upscaling.is_shader() && !self.upscale_failed
                && !self.texture.is_hdr() && output.0 >= input.0 && output.1 >= input.1 && output != input
                && output.0 <= 16384 && output.1 <= 16384;
            // AI enhancement only where it runs: a picture it does not read, or a failed
            // enhancer, costs what bilinear scaling costs.
            let ai = self.ai_enhances();
            // The video texture's picture goes into the window's target as it is, by the plane
            // pass or SDL, not through an upscaler or AI enhancement.
            let unscaled = !self.texture.texture.is_null() && !use_upscaler && !ai;
            // The plane pass draws only a picture the display places one texel to one pixel.
            self.clear_window(renderer, unscaled && present::draws_one_to_one(&dst, input));
            #[cfg(windows)]
            if !unscaled { self.texture.release_back_buffers(); }
            if !self.texture.texture.is_null() {
                let mut texture = self.texture.texture;
                if ai {
                    // AI enhancement reads the YUV picture in the video texture.
                    self.texture.copy_held();
                    self.upscaler = None;
                    if let Some(enhanced) = self.enhanced(renderer, output) { texture = enhanced; }
                } else if self.settings.upscaling == config::Upscaling::MetalFx {
                    self.upscaler = None;
                    #[cfg(target_os = "macos")]
                    {
                        let source = self.texture.metalfx_source();
                        // Decided by the stream's settings and picture, not by this present's Frame
                        // Warp shift, so the pass is not dropped and rebuilt as the shift comes and goes.
                        self.metalfx_blocked = super::metalfx::blocker(&super::metalfx::Conditions {
                            failed: self.upscale_failed,
                            hdr: self.texture.is_hdr(),
                            frame_warp: self.frame_warp.is_some(),
                            metal_renderer: !SDL_GetRenderMetalLayer(renderer).is_null(),
                            hardware: source.is_some(),
                            bt2020: source.as_ref().is_some_and(|(_, info)| matches!(info.color_space, 9 | 10)),
                            input,
                            output,
                        });
                        if self.metalfx_blocked.is_none() {
                            if !self.metalfx.as_ref().is_some_and(|p| p.matches(input, output)) {
                                self.metalfx = super::metalfx::Spatial::new(renderer, input, output);
                                if self.metalfx.is_some() { eprintln!("nightjar: MetalFX upscaling {}x{} to {}x{}", input.0, input.1, output.0, output.1); }
                            }
                            let (buffer, info) = source.unwrap();
                            // A present without a new picture (an overlay redraw, a repeat) shows
                            // the last MetalFX output again, not a second pass of the same picture.
                            let reused = if fresh { None } else { self.metalfx.as_ref().and_then(|pass| pass.output()) };
                            let drawn = match reused {
                                Some(scaled) => Some(scaled),
                                None => self.metalfx.as_mut().and_then(|pass| pass.draw(renderer, buffer, &info)),
                            };
                            match drawn {
                                Some(scaled) => texture = scaled,
                                None => {
                                    eprintln!("nightjar: MetalFX {} failed for {}x{} to {}x{}; bilinear for this stream", if self.metalfx.is_some() { "submission" } else { "creation" }, input.0, input.1, output.0, output.1);
                                    self.metalfx = None;
                                    self.upscale_failed = true;
                                    self.show_toast(tr("GPU upscaling is unavailable; using bilinear scaling."));
                                }
                            }
                        } else { self.metalfx = None; }
                    }
                } else if use_upscaler {
                    // The upscalers read the picture as SDL would draw it.
                    texture = self.texture.sdl_texture(renderer);
                    // Frame Warp shifts the picture on its way into the upscaler, in the picture's own
                    // pixels; the upscaled picture is drawn as it is.
                    let warp = std::mem::take(&mut self.texture.warp);
                    // Temporal upscaling that cannot be made or fails to draw gives way to SGSR 1
                    // for the rest of the stream, and draws this picture with it.
                    // Frame Warp shifts each redraw between pictures, which temporal upscaling
                    // cannot show without feeding a picture into its history twice: SGSR 1 draws
                    // while Frame Warp is on, said in the log and the stats line.
                    let warp_on = self.frame_warp.is_some();
                    if warp_on && self.settings.upscaling == config::Upscaling::Sgsr2 && !self.temporal_warp_logged {
                        eprintln!("nightjar: temporal upscaling does not run with Frame Warp; the stream uses SGSR 1");
                        self.temporal_warp_logged = true;
                    }
                    let upscaled = loop {
                        let mode = upscale::effective_mode(self.settings.upscaling, self.temporal_failed || warp_on);
                        let mut refused = None;
                        if !self.upscaler.as_ref().is_some_and(|u| u.matches(input, output, mode)) {
                            self.upscaler = upscale::Upscaler::new(renderer, input, output, mode).map_err(|e| refused = Some(e)).ok();
                        }
                        let drawn = match self.upscaler.as_mut() {
                            // Only a whole newly decoded picture (`fresh`) moves temporal upscaling's
                            // history on; a repeat or a beam race's slice present shows its last output.
                            Some(u) => u.draw_warped(renderer, texture, self.settings.upscale_sharpness, &warp, &mut self.texture.warp_mesh,
                                upscale::History { advance: fresh, reset }),
                            None => Err(refused.unwrap_or_default()),
                        };
                        match drawn {
                            Ok(upscaled) => break Some(upscaled),
                            Err(reason) if mode == config::Upscaling::Sgsr2 => {
                                self.upscaler = None;
                                self.temporal_unavailable(&reason);
                            }
                            Err(reason) => {
                                eprintln!("nightjar: GPU upscaling failed: {reason}; the stream continues with bilinear scaling");
                                break None;
                            }
                        }
                    };
                    match upscaled {
                        Some(upscaled) => texture = upscaled,
                        None => {
                            self.texture.warp = warp;
                            self.upscaler = None;
                            self.upscale_failed = true;
                            self.show_toast(tr("GPU upscaling is unavailable; using bilinear scaling."));
                        }
                    }
                } else { self.upscaler = None; }
                if !self.draw_direct(renderer, &dst, texture) {
                    if texture == self.texture.texture { texture = self.texture.sdl_texture(renderer); }
                    // The video texture's alignment padding stays out of the picture.
                    let source = self.texture.source_rect(texture);
                    frame_warp::render_texture(renderer, texture, source.as_ref(), &dst, &self.texture.warp, &mut self.texture.warp_mesh);
                }
            } else if !self.texture.draw(renderer, &dst) {
                eprintln!("nightjar: packed full-chroma presentation failed");
                self.exit = Some(Exit::Ended(Some(tr(PACKED_UNSHOWN).to_owned())));
            }
        }
        draw_overlay(self, renderer);
        // The display probe reads the scan line around the present (nothing with the beam race off,
        // no VRR check and no VRR sync on the scan).
        if let (Some(topic), true, true) = (display_probe(self.facts.beam_race, &self.vrr), probe, self.race_probe.is_none()) {
            self.race_probe = Some(race_probe::RaceProbe::begin(renderer, self.pacer.mode != PacingMode::Vsync, f64::from(self.facts.fps), topic));
        }
        // Keys, clicks and controllers leave from their own threads while V-Sync holds the present,
        // and while a VRR present waits for the picture's drawing and the scan under way: nearly
        // every present of a stream at the display's top rate does, for most of a scan. So does a
        // VRR present with sync interval 1 once Tear-free VRR turned to it, or when the wait would
        // cost the next picture its turn (`scan_sync::ScanWait::Latch`), in the swap chain.
        let mode_tears = self.pacer.mode != PacingMode::Vsync;
        let holds = !mode_tears || (probe && self.vrr.sync == present::VrrSync::Scan && self.scan_settles) || self.vrr.distrusted;
        if holds {
            // SAFETY: a live renderer.
            self.direct_motion.presenting(Some(self.direct_rules(unsafe { SDL_GetRenderWindow(renderer) })));
        }
        let latch = if probe { self.wait_out_scan(renderer, due_us) } else { false };
        let beam_before = self.race_probe.as_ref().filter(|_| probe).and_then(race_probe::RaceProbe::before);
        let skips = probe && self.skip_over_waiting && !self.skipped.must_wait() && self.vrr.sync != present::VrrSync::Blank
            && self.pacer.skips_instead_of_waiting(self.plan.display.refresh_hz);
        #[cfg(target_os = "linux")]
        if probe {
            // SAFETY: a live renderer.
            self.glass.request(unsafe { SDL_GetRenderWindow(renderer) }, session::now_us());
        }
        #[cfg(target_os = "linux")]
        {
            self.window_target.before_present(renderer as usize, crate::sdl::window_target_frames(renderer));
            // The host's mastering metadata for an HDR10 swapchain, again when it or the peak changes.
            // Once HDR stops, the swapchain gets unknown metadata instead of keeping the last stream's.
            let stream = self.texture.hdr_metadata().map(|m| crate::sdl::swapchain_hdr_metadata(&m, crate::sdl::output_peak_nits(renderer)));
            if let Some(metadata) = swapchain_metadata_change(stream, self.swapchain_hdr_metadata) {
                if crate::sdl::output_decision() == Some(Ok(crate::handheld::Output::Pq)) {
                    match metadata {
                        Some(m) => eprintln!("nightjar: HDR10 swapchain metadata: mastering {:.4}-{:.0} nits, MaxCLL {:.0}, MaxFALL {:.0}",
                            m.min_luminance, m.max_luminance, m.max_content_light_level, m.max_frame_average_light_level),
                        None => eprintln!("nightjar: HDR10 swapchain metadata: unknown (none passed on)"),
                    }
                }
                // SAFETY: a live renderer.
                crate::sdl::set_swapchain_hdr_metadata(unsafe { SDL_GetRenderWindow(renderer) }, metadata);
                self.swapchain_hdr_metadata = Some(metadata);
            }
        }
        // With Frame Warp the hold came before its shift (`present`).
        if probe && self.frame_warp.is_none() { self.hold_for_turn(renderer); }
        let issued_us = session::now_us();
        let blank = self.blank_present(renderer, probe);
        // SAFETY: a live renderer.
        let mut latched = false;
        let refused = unsafe {
            let done = if skips { present::present_without_waiting(renderer, self.skipped.hold()) } else { present::NoWait::Failed };
            if skips { self.no_wait_present = done != present::NoWait::Failed; }
            if done == present::NoWait::Failed && !blank {
                // Sync interval 1 shows the picture whole at the next refresh when the wait for
                // the scan would have cost the next picture its turn (`scan_sync::scan_wait`).
                // SDL's own present tears, so only a successful adaptive present is a latch.
                latched = latch && present::present_adaptive(renderer, true);
                if !latched { SDL_RenderPresent(renderer); }
            }
            done == present::NoWait::Refused
        };
        self.flip_guard.last_flip_us = Some(issued_us);
        self.flip_guard.last_flip_latched = latched;
        if latched { self.scan_tally.latched += 1; }
        let returned_us = session::now_us();
        if let Some(warp) = self.frame_warp.as_mut() { warp.presented(returned_us); }
        #[cfg(target_os = "linux")]
        {
            self.window_target.after_present(crate::sdl::window_target_frames(renderer));
            self.video_only_presented(renderer);
        }
        if holds {
            self.direct_motion.presenting(None);
        }
        let fps = f64::from(self.facts.fps);
        let probe_tears = self.vrr.probe_tears(self.pacer.mode);
        // A refused present reached no display, so it says nothing about the scan.
        let scan = self.race_probe.as_mut().filter(|_| probe && !refused).map(|probe| {
            probe.after_present(renderer, beam_before, probe_tears, fps);
            probe.report(beam_before)
        });
        Flipped { issued_us, returned_us, scan, refused }
    }

    /// Shows the due picture, if any. Returns whether something was drawn.
    pub fn present(
        &mut self,
        renderer: *mut SDL_Renderer,
        mut draw_overlay: impl FnMut(&mut Self, *mut SDL_Renderer),
    ) -> bool {
        // An HDR stream whose output stops showing HDR for a held second plays on in SDR: an HDR10
        // picture is drawn tone mapped to the output (SDL's draw of the texture, or the direct and
        // plane passes, with SDL's tone map to the output's headroom), and HDR output that comes
        // back for a held second is used again. A display plane shows HDR itself.
        if self.hdr_output.is_some() && !self.kms_usable() {
            let shown = crate::sdl::hdr_available(renderer);
            if let Some(on) = self.hdr_output.as_mut().and_then(|output| output.observe(shown, Instant::now())) {
                eprintln!("nightjar: an HDR stream's output is now {}", if on { "HDR" } else { "SDR, tone mapped" });
                self.show_toast(if on { tr("HDR is back on this display.") } else { tr("This display stopped showing HDR. The stream is drawn in SDR.") });
                self.needs_redraw = true;
            }
        }
        self.sync_race(renderer);
        self.sync_vrr(renderer);
        self.sync_keeper();
        if self.pacer.host_grid() { self.sync_host_grid(renderer); }
        // The slices of a frame still being received go out ahead of the beam.
        // Slices are drawn by the renderer, which a display plane's pictures pass by.
        let sliced = !self.kms_engaged() && self.present_slices(renderer, &mut draw_overlay, session::now_us());
        let now = session::now_us();
        let overdue = self.skipped.take_due(now);
        let (picture, mut dropped) = self.pacer.take_due(now);
        // A newer frame's slices are on the screen: this older picture would step it back a frame.
        let picture = picture.filter(|p| {
            let older = self.racer.newer_on_screen(p.decoded.race_seq);
            dropped += u32::from(older);
            !older
        });
        let (picture, replaced) = present::take_over_kept(picture, &mut self.skipped_picture, overdue);
        dropped += replaced;
        self.window.dropped_pacing += dropped;
        self.shared.counters.replaced.fetch_add(u64::from(dropped), Ordering::Relaxed);
        let has_picture = picture.is_some();
        // Temporal upscaling's history is the same scene's until the decode side reports a key
        // frame or a gap. The count is cumulative, so pictures the pacer drops carry it on:
        // skipping pictures alone keeps the history, a break in a dropped one still resets it.
        let temporal_reset = picture.as_ref().is_some_and(|p| temporal::history_stale(self.drawn_history, p.decoded.follows, p.decoded.history_breaks));
        if let Some(p) = &picture { self.drawn_history = p.decoded.history_breaks; }
        // Without a picture or a redraw, the held picture may go out again: for a VRR panel
        // about to refresh by itself, or shifted anew by Frame Warp.
        let kept = !has_picture && !self.needs_redraw && !sliced && self.lfc_repeat(now);
        if !has_picture && !self.needs_redraw && !kept && (sliced || !self.warp_repeat(now)) {
            return sliced;
        }
        let begin = session::now_us();
        // Whether the frame is under the racer, and which rows of it are left to draw.
        let mut raced = false;
        let mut rows = None;
        if let Some(p) = &picture {
            if self.facts.phase_lock && self.pacer.mode == PacingMode::Vsync && self.focused && !self.menu_visible {
                // With no refreshes to spare a picture has to make the desktop's deadline, which comes before the refresh.
                let refresh_hz = self.plan.display.refresh_hz;
                let skipping = self.no_wait_present && self.pacer.skips_instead_of_waiting(refresh_hz);
                let exclusive = exclusive_fullscreen(renderer);
                let room_us = if self.pacer.host_grid() { present::HOST_GRID_LATCH_ROOM_US } else { 0 };
                let held = self.vrr.held(self.pacer.mode);
                let refresh_us = (1e6 / f64::from(refresh_hz.max(1.0))) as u64;
                // A hold longer than a report's period (20 fps held 3 refreshes of 60 Hz) reports
                // over a refresh instead.
                let mut report_held = if self.held_report_refresh { 1 } else { nightjar_policy::vsync::report_refreshes(held, refresh_us * 1000) };
                // Elsewhere a held stream's grid is the clocked one (`phase::HeldClock`); until its
                // clock knows a picture's time nothing is held, and the report counts refreshes.
                let clocked = if !cfg!(windows) && report_held > 1 { self.held_clock.timing(begin, report_held, refresh_us) } else { None };
                let unclocked = !cfg!(windows) && held > 1 && clocked.is_none();
                if unclocked { report_held = 1; }
                let timing = clocked.or_else(|| phase::timing(renderer, refresh_hz, report_held, &mut self.held_anchor));
                // Fractional V-Sync: the lock aims at the hold's own target, the present lead
                // (`phase::held_present_lead_us`, the guard `held_guard_us` into the refresh), not at
                // the deadline the turn must make: aimed at the deadline, the host delivered the
                // picture after the refresh that opens its turn, so the hold never waited and its
                // present raced the refresh.
                let aim_us = if held > 1 && !unclocked {
                    phase::held_present_lead_us(refresh_us, self.held_guard_us) as i64
                } else {
                    present::phase_lock_aim_us(skipping, exclusive, self.present_deadline_us, refresh_hz) + room_us
                };
                let timing = phase::aimed(timing, aim_us);
                let steady_windows = if skipping { self.phase_steady_windows } else { 0 };
                if let Some(report) = self.phase.sample(p.decoded.ready_us, begin, timing, steady_windows) {
                    if self.timing_csv.is_some() {
                        eprintln!("nightjar: phase report: lead {} us, spread {} us, period {} us, aim {aim_us} us, held {held}",
                            report.lead_ns / 1000, report.spread_ns / 1000, report.period_ns / 1000);
                    }
                    let _ = self.session.send(ClientMessage::PhaseLockReport(report));
                }
            } else { self.phase.reset(); }
            if let (PacingMode::BeamRace, Some(scan), true) = (self.pacer.mode, self.race_scan, p.decoded.race_seq != 0) {
                raced = true;
                let first_row = p.decoded.bands.last().map_or(0, |band| band.first_row);
                if let race::Final::Rows(first, end) = self.racer.finish(p.decoded.race_seq, first_row, begin, &scan, &self.latency) {
                    rows = race::scissor(first, end, p.decoded.setup.height as u32);
                }
            }
            // Only a picture on the screen counts as the stream's video (`has_picture`, the
            // no-video watchdog, `video_started`).
            if self.scan_out(&p.decoded) || self.texture.upload(renderer, &p.decoded, rows) {
                self.unshown.shown();
                self.first_frame = true;
            } else if p.decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                self.end_pyrowave_unshown();
            } else if p.decoded.setup.chroma_packing != 0 {
                eprintln!("nightjar: packed full-chroma reconstruction failed: the decoded planes, GPU, or HDR output are unsupported");
                self.exit = Some(Exit::Ended(Some(tr(PACKED_UNSHOWN).to_owned())));
            } else {
                // A picture format this presenter cannot show: after a few in a row the decoder
                // gives way as a refused one does (the next backend, then software, from a key
                // frame), saying so; when software's cannot be shown either, the stream ends.
                let reason = self.texture.failure.take();
                if let Some(ready_us) = self.unshown.failed(p.decoded.ready_us) {
                    eprintln!("nightjar: {UNSHOWN_BEFORE_FALLBACK} {} pictures in a row could not be shown ({}); the decoder gives way",
                        p.decoded.picture.format_name(), reason.as_deref().unwrap_or("no reason given"));
                    self.shared.present_refused.store(ready_us, Ordering::Relaxed);
                }
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(p) = &picture { self.glass.next_picture(p.decoded.number, p.decoded.ready_us); }
        // The pictures the compositor answered for: the doctor's display time runs to the glass.
        // Before the present and its hold, which count the turn from the last picture's answer:
        // it comes at that picture's refresh, after the poll that followed its own present.
        #[cfg(target_os = "linux")]
        let (glass_row, to_glass) = {
            let now = session::now_us();
            let mut row = crate::timing_csv::Glass::default();
            for shown in self.glass.poll(now) {
                if let Some(glass_us) = shown.glass_us() {
                    self.doctor.shown(0, shown.ready_us, glass_us);
                    self.held_clock.shown(shown.frame, glass_us);
                }
                if let wl_presentation::Outcome::Presented { to_glass_ns, flags } = shown.outcome {
                    row = crate::timing_csv::Glass { frame: Some(shown.frame), to_glass_us: to_glass_ns.map(|ns| ns / 1000), flags: Some(flags) };
                }
            }
            (row, self.glass.timing(now))
        };
        #[cfg(not(target_os = "linux"))]
        let (glass_row, to_glass) = (crate::timing_csv::Glass::default(), false);
        // Fractional V-Sync holds the picture before Frame Warp's shift, so the shift is as fresh as
        // without the hold. Without Frame Warp it holds after the draw (`render_flip`, `kms_flip`),
        // whose time then stays out of the hold's margin.
        let held_before_draw = has_picture && self.frame_warp.is_some() && self.vrr.held(self.pacer.mode) > 1;
        // SAFETY: a live renderer.
        let window = unsafe { SDL_GetRenderWindow(renderer) };
        if held_before_draw { self.hold_for_turn_lending(renderer, window); }
        let draw_begin_us = session::now_us();
        // Frame Warp's shift for the picture, as late before its draw as it can be.
        if let Some(warp) = self.frame_warp.as_mut() {
            if let Some(p) = &picture { warp.picture(p.decoded.number, p.decoded.race_seq, p.decoded.ready_us); }
            self.texture.warp = warp.draw(None);
        }
        let (flipped, by_kms) = match self.kms_flip(window, &mut draw_overlay, has_picture) {
            Some(flipped) => (flipped, true),
            None => (self.render_flip(renderer, &mut draw_overlay, has_picture, has_picture, temporal_reset, picture.as_ref().map(|p| p.present_us)), false),
        };
        self.needs_redraw = false;
        // The draw after the hold, which the hold's margin has to leave time for.
        if held_before_draw && !flipped.refused {
            self.held_draw.record(flipped.issued_us.saturating_sub(draw_begin_us));
        } else if self.frame_warp.is_none() {
            self.held_draw.reset();
        }
        // A held stream's turns count from its pictures, not its redraws (`phase::HeldAnchor`).
        if let Some(p) = picture.as_ref().filter(|_| !flipped.refused && self.vrr.held(self.pacer.mode) > 1) {
            if cfg!(windows) {
                self.held_anchor.presented(phase::last_present_count(renderer));
            } else {
                self.held_clock.presented(p.decoded.number);
                // A KMS commit returns at the refresh that shows it.
                if by_kms { self.held_clock.shown(p.decoded.number, flipped.returned_us); }
            }
        }
        // Every present starts a scan on a display that refreshes on present.
        if let Some(keeper) = self.keeper.as_mut().filter(|_| self.keeper_on && !flipped.refused) {
            match &picture {
                Some(p) => keeper.picture(flipped.issued_us, p.decoded.rtp),
                None if kept => keeper.repeated(flipped.issued_us),
                None => keeper.redrawn(flipped.issued_us),
            }
        }
        if flipped.refused {
            self.skipped_picture = picture;
            let refresh_us = (1_000_000.0 / self.plan.display.refresh_hz.max(1.0)) as u64;
            if let Some(hold) = self.skipped.skipped(flipped.returned_us, self.pacer.frame_us(), refresh_us) {
                eprintln!("nightjar: pacing: many presents find the last picture waiting, each now tries for {hold:?}");
            }
            return true;
        }
        // A redraw for the menu or the overlay leaves a kept picture to be shown and counted.
        if has_picture { self.skipped.shown(); }
        let presented = flipped.returned_us;
        let scan = flipped.scan;
        let watches = has_picture && self.no_wait_present && self.pacer.skips_instead_of_waiting(self.plan.display.refresh_hz);
        let mut flip = if (self.timing_csv.is_some() || watches) && picture.is_some() { phase::present_stats(renderer) } else { Default::default() };
        (flip.held_wait_us, flip.issued_us) = (self.held_wait_us.take(), flipped.issued_us);
        if watches {
            let refresh_us = (1_000_000.0 / self.plan.display.refresh_hz.max(1.0)) as u64;
            self.miss_watch.presented(flip.present_count, flipped.issued_us);
            if self.miss_watch.seen(flip, refresh_us) {
                self.phase.missed();
                if let Some(hold) = self.skipped.missed(self.pacer.frame_us(), refresh_us) {
                    eprintln!("nightjar: pacing: many pictures miss their refresh here, each now waits its turn ({hold:?})");
                }
            }
        }
        if let Some(p) = picture {
            let timeline = self.finish_frame(&p.decoded, raced, flipped.issued_us, flipped.returned_us);
            let w = &mut self.window;
            w.shown += 1;
            let (drawing, present_call) = frame_shares(begin, flipped.issued_us, flipped.returned_us);
            w.render_us += drawing;
            w.present_us += present_call;
            w.hold_us += p.hold_us;
            w.late += u32::from(p.late);
            w.network_us += p.decoded.enqueue_us.saturating_sub(p.decoded.receive_us);
            w.network_n += 1;
            // While the glass times arrive, they give the display time instead of the present call.
            // Only Linux on Wayland has them: on Windows and X11 the doctor's display time still
            // ends at the present call, so the platforms' figures differ by the compositor's part.
            self.doctor.shown(p.decoded.host_latency, if to_glass { 0 } else { p.decoded.ready_us }, presented);
            if let Some(csv) = &self.timing_csv {
                let d = &p.decoded;
                csv.row(crate::timing_csv::Row {
                    number: d.number,
                    rtp: d.rtp,
                    receive_us: d.receive_us,
                    enqueue_us: d.enqueue_us,
                    input_us: d.input_us,
                    output_us: d.ready_us,
                    render_us: presented,
                    host_latency: d.host_latency,
                    drops: self.shared.counters.replaced.load(Ordering::Relaxed),
                    clock: self.offset.offset_at(d.enqueue_us as i64).zip(self.offset.rtt_us()),
                    early: d.early,
                    first_block_us: d.first_block_us,
                    race: d.race,
                    scan: scan.unwrap_or_default(),
                    presented: timeline,
                    due_us: p.present_us,
                    begin_us: begin,
                    flip,
                    glass: glass_row,
                });
            }
            let host_us = self.rtp_clock.host_us(p.decoded.rtp);
            if let Some(offset) = self.offset.offset_at(presented as i64) {
                let latency = presented as i64 - (host_us + offset);
                if (0..2_000_000).contains(&latency) {
                    w.latency_us += latency;
                    w.latency_n += 1;
                }
            }
            // A picture on a display plane stays out of the decoder's pool while it shows.
            #[cfg(target_os = "linux")]
            if let Some(kms) = self.kms.as_mut().filter(|kms| kms.engaged()) {
                kms.keep(p);
            }
        }
        true
    }

    /// Whether a display plane may still take the pictures (`kms_scanout`).
    fn kms_usable(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.kms.as_ref().is_some_and(kms_scanout::Scanout::usable);
        #[cfg(not(target_os = "linux"))]
        false
    }

    /// Readies `decoded` for a display plane instead of the video texture; false leaves it to the
    /// renderer, as before, with a notice the first time the planes give up.
    #[cfg(target_os = "linux")]
    fn scan_out(&mut self, decoded: &pipeline::Decoded) -> bool {
        let Some(kms) = self.kms.as_mut().filter(|kms| kms.usable()) else { return false };
        let (scaling, top) = (self.settings.video_scaling, self.settings.align_video_top);
        let result = match self.texture.scanout_hdr(decoded) {
            Ok(hdr) => kms.prepare(decoded, hdr, |picture, area| {
                let r = viewport(picture, area, scaling, top);
                (r.x, r.y, r.w, r.h)
            }),
            Err(why) => {
                kms.give_up(&why);
                Err(why)
            }
        };
        if result.is_err() {
            // Often right after the start notice (the first picture): it joins it, not replaces it.
            self.add_toast(tr("Video planes are unavailable; the stream is drawn by the renderer. The log says why."));
            self.needs_redraw = true;
        }
        result.is_ok()
    }

    #[cfg(not(target_os = "linux"))]
    fn scan_out(&mut self, _decoded: &pipeline::Decoded) -> bool {
        false
    }

    /// Shows the readied picture on its display plane, the overlay drawn into its own plane above
    /// it, in one commit; None while the planes do not show the stream, or when they just gave up
    /// (then the renderer draws this frame).
    #[cfg(target_os = "linux")]
    fn kms_flip(&mut self, window: *mut SDL_Window, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), picture: bool) -> Option<Flipped> {
        if !self.kms_engaged() {
            return None;
        }
        // What `draw_overlay` in main.rs draws something for.
        let meshes = self.overlay_visible || self.toast.is_some() || self.menu_visible;
        match self.kms.as_mut().and_then(|kms| kms.overlay_renderer().filter(|_| meshes)) {
            Some(overlay) => {
                draw_overlay(self, overlay);
                if let Some(kms) = self.kms.as_mut() { kms.overlay_drawn(); }
            }
            None => {
                if let Some(kms) = self.kms.as_mut() { kms.overlay_hidden(); }
            }
        }
        // Keys, clicks and controllers leave from their own threads while fractional V-Sync holds the
        // picture and the commit waits for its refresh, as in the renderer's present (`render_flip`);
        // the overlay is drawn before. With Frame Warp the hold came before its shift and gave the
        // input back (`hold_for_turn_lending`), so it is lent again only for the commit's wait.
        let lends = picture && self.vrr.held(self.pacer.mode) > 1;
        if lends { self.direct_motion.presenting(Some(self.direct_rules(window))); }
        // KMS has no margin to call for a renderer: `held_margin_us` answers before it would.
        if lends && self.frame_warp.is_none() { self.hold_for_turn(std::ptr::null_mut()); }
        let issued_us = session::now_us();
        let committed = self.kms.as_mut().map_or(Ok(()), kms_scanout::Scanout::commit);
        let returned_us = session::now_us();
        if lends { self.direct_motion.presenting(None); }
        if committed.is_err() {
            self.add_toast(tr("Video planes are unavailable; the stream is drawn by the renderer. The log says why."));
            self.needs_redraw = true;
            return None;
        }
        Some(Flipped { issued_us, returned_us, scan: None, refused: false })
    }

    #[cfg(not(target_os = "linux"))]
    fn kms_flip(&mut self, _window: *mut SDL_Window, _draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), _picture: bool) -> Option<Flipped> {
        None
    }

    /// A frame is on the screen: closes it in the racer, teaches the predictor its slice times and
    /// gives the timing CSV what the beam race columns need (empty with the race off).
    pub(super) fn finish_frame(&mut self, decoded: &pipeline::Decoded, raced: bool, issued_us: u64, returned_us: u64) -> crate::timing_csv::Presented {
        let mut timeline = crate::timing_csv::Presented::default();
        if !self.facts.beam_race { return timeline; }
        if decoded.bands.len() >= 2 {
            let facts = &self.facts;
            self.predictor.get_or_insert_with(|| seeded_predictor(facts)).learn(&decoded.bands);
        }
        let mut presents = vec![issued_us];
        // The picture's own present costs what a slice's does, whether or not the frame was raced.
        self.latency.record_call(issued_us, returned_us, returned_us);
        if let (true, Some(scan)) = (raced, self.race_scan) {
            // Only the frame the racer follows: a picture of an older frame closes nothing.
            if let Some(record) = self.racer.close(decoded.race_seq, issued_us, &scan) {
                timeline.late_slices = Some(record.late);
                presents = record.presents_us;
            }
        }
        timeline.set_presents(&presents);
        if let Some(scan) = self.race_scan {
            let first = presents.first().copied().unwrap_or(issued_us);
            timeline.scan_start_us = scan.reaches_us(first, 0);
            timeline.mid_row_age_ns = race::mid_row_age_ns(&scan, first, &decoded.bands);
        }
        timeline
    }
}

/// Whether the renderer's window is in exclusive fullscreen, where nothing composes its pictures.
fn exclusive_fullscreen(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: a live renderer and its window.
    unsafe {
        let window = SDL_GetRenderWindow(renderer);
        SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0 && !SDL_GetWindowFullscreenMode(window).is_null()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With Upscaling set to AI, a picture AI enhancement does not read, or a failed enhancer,
    /// draws as bilinear scaling does: straight into the window's target, with no copy for it.
    #[test]
    fn ai_enhancement_is_in_effect_only_where_it_runs() {
        use config::Upscaling;
        assert!(ai_in_effect(Upscaling::Ai, false, true));
        assert!(!ai_in_effect(Upscaling::Ai, true, true), "failed this stream");
        assert!(!ai_in_effect(Upscaling::Ai, false, false), "a picture it does not read");
        assert!(!ai_in_effect(Upscaling::default(), false, true));
    }

    /// A size or frame rate the PC did not confirm says automatic quality stopped only when it
    /// was on: a change from the stream menu or a window on another monitor says the change did
    /// not apply.
    #[test]
    fn an_unconfirmed_mode_says_automatic_quality_stopped_only_when_it_was_on() {
        assert!(unconfirmed_mode_toast(true).contains("Automatic quality stopped"));
        let manual = unconfirmed_mode_toast(false);
        assert!(!manual.contains("Automatic quality"), "{manual}");
        assert!(manual.contains("did not apply"), "{manual}");
    }

    /// A picture format the presenter cannot show hands the decoder over after a few pictures in
    /// a row, with the time the last was ready (never 0, which means none); a shown picture
    /// starts the count over, and so does the hand-over, for the next decoder's pictures.
    #[test]
    fn pictures_that_cannot_be_shown_hand_the_decoder_over_after_a_few_in_a_row() {
        let mut unshown = Unshown::default();
        for ready_us in 1..UNSHOWN_BEFORE_FALLBACK as u64 { assert_eq!(unshown.failed(ready_us), None); }
        unshown.shown();
        for ready_us in 1..UNSHOWN_BEFORE_FALLBACK as u64 { assert_eq!(unshown.failed(ready_us), None, "a shown picture starts over"); }
        assert_eq!(unshown.failed(9_000), Some(9_000));
        for _ in 1..UNSHOWN_BEFORE_FALLBACK { assert_eq!(unshown.failed(9_500), None, "the next decoder counts anew"); }
        assert_eq!(unshown.failed(0), Some(1), "never 0");
    }

    /// The overlay's Render ends where the present call begins: a V-Sync wait or a compositor's
    /// layer inside the call is the system's share, not Nightjar's drawing.
    #[test]
    fn a_frames_drawing_ends_at_the_present_call() {
        assert_eq!(super::frame_shares(1_000, 1_600, 9_900), (600, 8_300));
        assert_eq!(super::frame_shares(1_000, 1_000, 1_000), (0, 0));
        // Clocks that step back count nothing rather than wrap.
        assert_eq!(super::frame_shares(2_000, 1_500, 1_400), (0, 0));
    }

    /// `pump`'s crop over a stream: pictures of `coded` size arriving at `t0 + ms`, with `asked`
    /// pending since `asked_ms` until a picture confirms it, as `pump` clears it. Returns each
    /// picture's visible size and whether it confirmed.
    fn stream(start: (i32, i32), asked: Option<((i32, i32), u64)>, pictures: &[((i32, i32), u64)]) -> Vec<((i32, i32), bool)> {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut sizes = StreamSizes::new(start);
        let mut pending = asked.map(|(size, ms)| { sizes.ask(size); (size, at(ms)) });
        pictures.iter().map(|&(coded, ms)| {
            let (visible, confirmed) = sizes.arrive(coded, pending, at(ms));
            if confirmed { pending = None; }
            (visible, confirmed)
        }).collect()
    }

    #[test]
    fn old_pictures_during_a_size_change_keep_their_columns_and_do_not_confirm_it() {
        // An AV1 1920x1080 stream (coded 1920x1082) asked to step its width down to 1904, which AMD
        // codes 1920x1082 as well.
        let shown = stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1082), 100), ((1920, 1082), 900)]);
        assert_eq!(shown, [((1920, 1080), false); 2], "the old size's pictures, before SWITCH_GRACE");
        // After SWITCH_GRACE the PC's new pictures confirm the new size.
        let shown = stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1082), 100), ((1920, 1082), 1100)]);
        assert_eq!(shown, [((1920, 1080), false), ((1904, 1080), true)]);
        // A picture that is exactly an old size stays whole whatever is pending.
        assert_eq!(stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1080), 1100)]), [((1920, 1080), false)]);
        // Sizes far apart need no grace: 2184x1968 (coded 2240x1968) after 1920x1080.
        let shown = stream((1920, 1080), Some(((2184, 1968), 0)), &[((1920, 1082), 10), ((2240, 1968), 20)]);
        assert_eq!(shown, [((1920, 1080), false), ((2184, 1968), true)]);
    }

    #[test]
    fn pictures_after_a_confirmed_step_down_stay_cropped_to_the_new_size() {
        // 1920x1080 changed to 1904x1080: both coded 1920x1082. Once confirmed, every later
        // picture is the new stream's, cropped to 1904, during and after SWITCH_GRACE.
        let shown = stream((1920, 1080), Some(((1904, 1080), 0)), &[((1920, 1082), 1100), ((1920, 1082), 1200), ((1920, 1082), 2500), ((1920, 1082), 60_000)]);
        assert_eq!(shown, [((1904, 1080), true), ((1904, 1080), false), ((1904, 1080), false), ((1904, 1080), false)]);
    }



    #[test]
    fn a_late_size_applies_the_whole_mode_and_a_confirmation_only_the_rate() {
        let asked = Reconfigure { width: 2184, height: 1968, fps_millihz: 90_000 };
        let old = Reconfigure { width: 1920, height: 1080, fps_millihz: 60_000 };
        assert_eq!(mode_to_apply(true, false, Some(asked), Some(old)), Some(ModeApply::Rate(asked)));
        assert_eq!(mode_to_apply(false, true, None, Some(asked)), Some(ModeApply::Whole(asked)), "the timeout put the whole mode back");
        assert_eq!(mode_to_apply(false, false, Some(asked), Some(old)), None);
        assert_eq!(mode_to_apply(false, true, None, None), None);
    }
    #[test]
    fn a_size_applied_after_the_timeout_reports_itself_for_its_mode() {
        let t0 = Instant::now();
        let mut sizes = StreamSizes::new((1920, 1080));
        sizes.ask((2184, 1968));
        // The timeout cleared the pending request; the PC switches later.
        assert_eq!(sizes.arrive_mode((2240, 1968), None, t0 + Duration::from_secs(6)), ((2184, 1968), false, true));
        // A picture of the size the stream already shows is no late apply.
        let (_, _, late) = sizes.arrive_mode((2240, 1968), None, t0 + Duration::from_secs(7));
        assert!(!late);
    }
    #[test]
    fn a_size_the_pc_applies_after_the_timeout_is_still_cropped() {
        // The 5 s timeout cleared the pending 2184x1968 (`pump` drops it); the PC switches later.
        let t0 = Instant::now();
        let mut sizes = StreamSizes::new((1920, 1080));
        sizes.ask((2184, 1968));
        assert_eq!(sizes.arrive((2240, 1968), None, t0 + Duration::from_secs(6)), ((2184, 1968), false));
        // Another request the PC never answers: the late size stays the stream's.
        sizes.ask((2560, 1440));
        let pending = Some(((2560, 1440), t0 + Duration::from_secs(7)));
        assert_eq!(sizes.arrive((2240, 1968), pending, t0 + Duration::from_secs(8)), ((2184, 1968), false));
        assert_eq!(sizes.arrive((2240, 1968), None, t0 + Duration::from_secs(13)), ((2184, 1968), false), "after the second timeout");
    }

    #[test]
    fn pictures_of_the_size_before_a_confirmed_change_are_still_cropped() {
        // 2184x1968 confirmed while 1920x1080 pictures (coded 1920x1082) are still queued.
        let shown = stream((1920, 1080), Some(((2184, 1968), 0)), &[((2240, 1968), 10), ((1920, 1082), 20), ((2240, 1968), 30), ((1920, 1082), 1500)]);
        assert_eq!(shown, [((2184, 1968), true), ((1920, 1080), false), ((2184, 1968), false), ((1920, 1082), false)], "the old size expires after SWITCH_GRACE");
    }

    #[test]
    fn a_size_edited_in_the_menu_but_not_applied_does_not_change_the_crop() {
        // A 2184x1968 stream (coded 2240x1968); the menu's size editor writes `requested_mode`
        // while the user steps to 2200, but only `request_mode` (Apply) asks `StreamSizes`.
        let t0 = Instant::now();
        let mut sizes = StreamSizes::new((2184, 1968));
        assert_eq!(sizes.arrive((2240, 1968), None, t0), ((2184, 1968), false));
        // Applied, the PC's new pictures are cropped to it.
        sizes.ask((2200, 1968));
        assert_eq!(sizes.arrive((2240, 1968), Some(((2200, 1968), t0)), t0 + Duration::from_millis(10)), ((2200, 1968), true));
    }

    /// The header named a packed 4:4:4 stream by the setting asked for: its size is the logical one.
    #[test]
    fn a_packed_or_pyrowave_setup_gives_the_pictures_size() {
        let packed = session::VideoSetup { format: nightjar_core::sdp::VIDEO_FORMAT_H264, width: 3840, height: 1080, fps: 60, chroma_packing: 1, logical_width: 1920, logical_height: 1080, color_space: 1, color_range: 0 };
        assert_eq!(setup_size(&packed), (1920, 1080));
        assert_eq!(setup_size(&session::VideoSetup { logical_width: 0, logical_height: 0, chroma_packing: 0, width: 2560, height: 1440, ..packed }), (2560, 1440));
    }
}
