//! Picture pump, loop sleep calculation, alignment crop state, and KMS prepare helpers.

use super::super::*;
use super::unconfirmed_mode_toast;

/// What the raced presenter guesses of a stream's slice times before it has seen a frame.
pub(super) fn seeded_predictor(facts: &Facts) -> race::Predictor {
    let frame_bytes = f64::from(facts.bitrate_kbps.max(1)) * 1000.0 / 8.0 / f64::from(facts.fps.max(1));
    race::Predictor::seeded(facts.race_slices, facts.height.max(1) as u32, frame_bytes, facts.pace_mbps)
}

/// The picture size a decoded frame's setup gives where the frame itself does not tell it (PyroWave,
/// packed 4:4:4): the logical size, which a packed frame's coded size is twice as wide as.
pub(super) fn setup_size(setup: &session::VideoSetup) -> (i32, i32) {
    if setup.logical_width > 0 && setup.logical_height > 0 { (setup.logical_width, setup.logical_height) } else { (setup.width, setup.height) }
}

/// One refresh of a display refreshing `refresh_hz` times a second, in microseconds.
pub(super) fn refresh_period_us(refresh_hz: f32) -> u64 {
    (1_000_000.0 / refresh_hz.max(1.0)) as u64
}

/// How long after a size change is asked for a picture that fits both the old and the new size
/// plus alignment padding is still taken for the old one: AMD codes a 1920 and a 1904 wide AV1
/// stream both 1920 wide, and the PC's old pictures are on their way for a while.
const SWITCH_GRACE: Duration = Duration::from_secs(1);

/// The sizes a decoded picture may be of, to show it without an encoder's alignment padding
/// (`crop::visible_size`): the size last asked of the PC, the one the stream is at, and for
/// `SWITCH_GRACE` after a change the one before it.
pub(in crate::stream) struct StreamSizes {
    /// The size last sent to the PC; the stream menu's unapplied edits are not.
    asked: (i32, i32),
    /// The negotiated size, then each size the PC confirmed.
    current: (i32, i32),
    /// The size before `current`, and when `current` was confirmed.
    previous: Option<((i32, i32), Instant)>,
}

impl StreamSizes {
    pub(in crate::stream) fn new(size: (i32, i32)) -> Self { Self { asked: size, current: size, previous: None } }

    /// A size sent to the PC.
    pub(in crate::stream) fn ask(&mut self, size: (i32, i32)) { self.asked = size; }

    /// The stream's part of a picture of `coded` size arriving at `now`, and whether it confirms
    /// `pending`, the size asked for and when, not confirmed yet.
    #[cfg(test)]
    pub(in crate::stream) fn arrive(&mut self, coded: (i32, i32), pending: Option<((i32, i32), Instant)>, now: Instant) -> ((i32, i32), bool) {
        let (visible, confirmed, _) = self.arrive_mode(coded, pending, now);
        (visible, confirmed)
    }

    /// [`arrive`](Self::arrive), also saying whether the size is one the PC applied after the
    /// timeout dropped its pending request: no confirmation, but the asked mode is applied with it.
    pub(in crate::stream) fn arrive_mode(&mut self, coded: (i32, i32), pending: Option<((i32, i32), Instant)>, now: Instant) -> ((i32, i32), bool, bool) {
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
pub(super) enum ModeApply {
    Rate(Reconfigure),
    Whole(Reconfigure),
}

pub(super) fn mode_to_apply(confirmed: bool, late: bool, pending: Option<Reconfigure>, late_mode: Option<Reconfigure>) -> Option<ModeApply> {
    if confirmed {
        pending.map(ModeApply::Rate)
    } else if late {
        late_mode.map(ModeApply::Whole)
    } else {
        None
    }
}

/// Whether AI enhancement is the upscaling in effect: chosen, not failed this stream, and the
/// picture is one it reads (`VideoTexture::enhance_source`: hardware-decoded NV12 or P010 in the
/// video texture). Otherwise the picture is drawn as with bilinear scaling, at its cost, and the
/// statistics say so (`upscaling_line`).
pub(super) fn ai_in_effect(upscaling: config::Upscaling, failed: bool, readable: bool) -> bool {
    upscaling == config::Upscaling::Ai && !failed && readable
}

/// Pictures in a row the presenter could not show before the decoder that made them gives way
/// (`Shared::present_refused`): one that fails alone may be a passing fault, not its format.
pub(super) const UNSHOWN_BEFORE_FALLBACK: u32 = 5;

/// Pictures in a row the presenter could not show.
#[derive(Debug, Default)]
pub(in crate::stream) struct Unshown {
    count: u32,
}

impl Unshown {
    /// A picture was shown.
    pub(in crate::stream) fn shown(&mut self) { self.count = 0; }

    /// A picture ready at `ready_us` could not be shown: Some(the time to hand the decoder,
    /// never 0) once `UNSHOWN_BEFORE_FALLBACK` did in a row; the next decoder's pictures count
    /// anew.
    pub(in crate::stream) fn failed(&mut self, ready_us: u64) -> Option<u64> {
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
pub(in crate::stream) fn frame_shares(begin_us: u64, issued_us: u64, returned_us: u64) -> (u64, u64) {
    (issued_us.saturating_sub(begin_us), returned_us.saturating_sub(issued_us))
}

/// Whether the renderer's window is in exclusive fullscreen, where nothing composes its pictures.
pub(super) fn exclusive_fullscreen(renderer: *mut SDL_Renderer) -> bool {
    // SAFETY: a live renderer and its window.
    unsafe {
        let window = SDL_GetRenderWindow(renderer);
        SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0 && !SDL_GetWindowFullscreenMode(window).is_null()
    }
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
        let racing_due = racing.is_some() && due == racing;
        let wait = match due {
            Some(due) => {
                let now = session::now_us();
                if due <= now {
                    Wait::Now
                } else if racing_due {
                    Wait::Precise(Duration::from_micros(due - now))
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
            (Wait::Precise(wait), Some(wake)) => Wait::Precise(wait.min(wake)),
            (wait, _) => wait,
        }
    }

    /// Whether a display plane may still take the pictures (`kms_scanout`).
    pub(super) fn kms_usable(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.kms.as_ref().is_some_and(kms_scanout::Scanout::usable);
        #[cfg(not(target_os = "linux"))]
        false
    }

    /// Readies `decoded` for a display plane instead of the video texture; false leaves it to the
    /// renderer, as before, with a notice the first time the planes give up.
    #[cfg(target_os = "linux")]
    pub(super) fn scan_out(&mut self, decoded: &pipeline::Decoded) -> bool {
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
    pub(super) fn scan_out(&mut self, _decoded: &pipeline::Decoded) -> bool {
        false
    }

    /// A frame is on the screen: closes it in the racer, teaches the predictor its slice times and
    /// gives the timing CSV what the beam race columns need (empty with the race off).
    pub(in crate::stream) fn finish_frame(&mut self, decoded: &pipeline::Decoded, raced: bool, issued_us: u64, returned_us: u64) -> crate::timing_csv::Presented {
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
