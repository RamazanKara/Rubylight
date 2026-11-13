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
    pub(super) fn arrive(&mut self, coded: (i32, i32), pending: Option<((i32, i32), Instant)>, now: Instant) -> ((i32, i32), bool) {
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
        (visible, confirmed)
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
                let ((width, height), confirmed) = self.stream_sizes.arrive(coded, pending, Instant::now());
                decoded.visible = Some((width, height));
                self.facts.width = width;
                self.facts.height = height;
                if confirmed {
                    if let Some((mode, _)) = self.pending_mode.take() { self.stream_rate_changed(mode.fps_millihz.div_ceil(1000)); }
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
            }
            self.last_arrival = Some((decoded.rtp, decoded.receive_us));
            self.doctor.arrived(decoded.rtp, decoded.host_latency, decoded.receive_us, self.facts.fps.max(0) as u32);
            // Frames still waiting to be decoded: a queue that stays non-empty makes this
            // picture's lateness the backlog's.
            if self.pacer.mode == PacingMode::HostTimes {
                self.pacer.set_decode_queue(self.session.decode_queue_len());
            }
            self.pacer.accept(decoded);
        }
        self.taken_frames = frames;
        self.observe_slices(&mut slices);
        self.taken_slices = slices;
        if self.pending_mode.is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(5)) {
            self.pending_mode = None;
            self.quality_policy = None;
            self.quality_mode = QualityMode::Off;
            self.show_toast(tr("The PC did not confirm the requested size. Automatic quality stopped."));
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

    /// Turns a VRR stream to the pacing it would have without VRR once the display check finds the
    /// display scanning at its own rate (`present::VrrPacing`), and gives the renderer the sync
    /// interval the pacing and `NIGHTJAR_VRR_SYNC` ask for.
    pub(super) fn sync_vrr(&mut self, renderer: *mut SDL_Renderer) {
        let before = self.pacer.mode;
        let display_vrr = self.vrr.display_vrr();
        let now = session::now_us();
        let probe = self.race_probe.as_ref();
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

    /// `VrrSync::Scan`: while the beam is in the picture's rows, waits for it to leave the last one,
    /// so the present shows its picture whole. Returns how long it waited (ns).
    fn wait_out_scan(&mut self) -> u64 {
        // The tearing modes that present whole pictures; the beam race times its own presents.
        if self.vrr.sync != present::VrrSync::Scan || !matches!(self.pacer.mode, PacingMode::Immediate | PacingMode::HostTimes) { return 0; }
        let Some(probe) = &self.race_probe else { return 0 };
        let (Some(beam), Some(sample)) = (probe.beam(), probe.before()) else { return 0 };
        let Some(left) = beam.scan_left_ns(&sample) else { return 0 };
        // Spinning: the wait is under one scan (4.2 ms at 240 Hz), shorter than a sleep can be exact.
        let until = Instant::now() + Duration::from_nanos(left);
        while Instant::now() < until { std::hint::spin_loop(); }
        self.scan_waits.0 += 1;
        self.scan_waits.1 += left;
        left
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
                let reason = self.texture.failure.take().unwrap_or_else(|| "PyroWave GPU reconstruction failed.".into());
                self.exit = Some(Exit::Ended(Some(reason)));
                return true;
            }
        }
        // One shift for all of a frame's slices and its own present (`Presenter::draw`).
        if let Some(warp) = self.frame_warp.as_mut() { self.texture.warp = warp.draw(Some(self.racer.seq())); }
        let flipped = self.render_flip(renderer, draw_overlay, true);
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

    /// Whether Frame Warp shows the held picture again now, its shift having moved
    /// (`frame_warp::Repeat`): at most once a refresh, never into the refresh a picture that is
    /// waiting or expected is due for, and not while slices race the beam (their frame keeps the
    /// shift of its first slice).
    fn warp_repeat(&mut self, now: u64) -> bool {
        if self.frame_warp.is_none() || self.pacer.mode == PacingMode::BeamRace { return false; }
        let next_picture_us = match (self.pacer.next_due(), self.skipped.due_us()) {
            (Some(due), Some(skipped)) => Some(due.min(skipped)),
            (due, skipped) => due.or(skipped),
        };
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

    #[cfg(windows)]
    fn enhance_failed(&mut self, error: String) -> Option<*mut SDL_Texture> {
        eprintln!("nightjar: AI enhancement failed: {error}; the stream continues with bilinear scaling");
        self.enhancer = None;
        self.upscale_failed = true;
        self.show_toast(tr("AI enhancement is unavailable; using bilinear scaling."));
        None
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
        if self.upscale_failed { "Bilinear (GPU unavailable)".into() }
        else if self.texture.is_hdr() { "Bilinear (HDR)".into() }
        else if self.upscaler.is_some() { self.settings.upscaling.label().into() }
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
        // What `draw_overlay` in main.rs draws: nothing, or egui meshes, which write PQ themselves
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

    /// The render state `draw_overlay` in main.rs hands egui's painter for this frame: null unless
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

    /// Draws the texture and the overlay and presents. `probe` reads the beam around the present.
    fn render_flip(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), probe: bool) -> Flipped {
        // SAFETY: drawing on a live renderer.
        unsafe {
            let (mut w, mut h) = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h);
            let dst = viewport(self.texture.size(), (w, h), self.settings.video_scaling, self.settings.align_video_top);
            let output = (dst.w as i32, dst.h as i32);
            let input = self.texture.size();
            let use_upscaler = self.settings.upscaling.is_shader() && !self.upscale_failed
                && !self.texture.is_hdr() && output.0 >= input.0 && output.1 >= input.1 && output != input
                && output.0 <= 16384 && output.1 <= 16384;
            // The video texture's picture goes into the window's target as it is, by the plane
            // pass or SDL, not through an upscaler or AI enhancement.
            let unscaled = !self.texture.texture.is_null() && !use_upscaler && self.settings.upscaling != config::Upscaling::Ai;
            self.clear_window(renderer, unscaled);
            #[cfg(windows)]
            if !unscaled { self.texture.release_back_buffers(); }
            if !self.texture.texture.is_null() {
                let mut texture = self.texture.texture;
                if self.settings.upscaling == config::Upscaling::Ai {
                    // AI enhancement reads the YUV picture in the video texture.
                    self.texture.copy_held();
                    self.upscaler = None;
                    if let Some(enhanced) = self.enhanced(renderer, output) { texture = enhanced; }
                } else if use_upscaler {
                    // The upscalers read the picture as SDL would draw it.
                    texture = self.texture.sdl_texture(renderer);
                    if !self.upscaler.as_ref().is_some_and(|u| u.matches(input, output, self.settings.upscaling)) {
                        self.upscaler = upscale::Upscaler::new(renderer, input, output, self.settings.upscaling);
                    }
                    // Frame Warp shifts the picture on its way into the upscaler, in the picture's own
                    // pixels; the upscaled picture is drawn as it is.
                    let warp = std::mem::take(&mut self.texture.warp);
                    match self.upscaler.as_ref().and_then(|u| u.draw_warped(renderer, texture, self.settings.upscale_sharpness, &warp, &mut self.texture.warp_mesh)) {
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
                self.exit = Some(Exit::Ended(Some("Packed full-chroma presentation failed.".into())));
            }
        }
        draw_overlay(self, renderer);
        // The display probe reads the scan line around the present (nothing with the beam race off,
        // no VRR check and no VRR sync on the scan).
        if let (Some(topic), true, true) = (display_probe(self.facts.beam_race, &self.vrr), probe, self.race_probe.is_none()) {
            self.race_probe = Some(race_probe::RaceProbe::begin(renderer, self.pacer.mode != PacingMode::Vsync, f64::from(self.facts.fps), topic));
        }
        if probe { self.wait_out_scan(); }
        let beam_before = self.race_probe.as_ref().filter(|_| probe).and_then(race_probe::RaceProbe::before);
        // Keys, clicks and controllers leave from their own threads while V-Sync holds the present.
        let mode_tears = self.pacer.mode != PacingMode::Vsync;
        if !mode_tears {
            // SAFETY: a live renderer.
            self.direct_motion.presenting(Some(self.direct_rules(unsafe { SDL_GetRenderWindow(renderer) })));
        }
        let skips = probe && self.skip_over_waiting && !self.skipped.must_wait()
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
        let issued_us = session::now_us();
        // SAFETY: a live renderer.
        let refused = unsafe {
            let done = if skips { present::present_without_waiting(renderer, self.skipped.hold()) } else { present::NoWait::Failed };
            if skips { self.no_wait_present = done != present::NoWait::Failed; }
            if done == present::NoWait::Failed { SDL_RenderPresent(renderer); }
            done == present::NoWait::Refused
        };
        let returned_us = session::now_us();
        if let Some(warp) = self.frame_warp.as_mut() { warp.presented(returned_us); }
        #[cfg(target_os = "linux")]
        {
            self.window_target.after_present(crate::sdl::window_target_frames(renderer));
            self.video_only_presented(renderer);
        }
        if !mode_tears {
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
        // A display plane shows HDR itself, whatever the renderer's swapchain takes.
        if self.texture.needs_hdr_output() && !self.kms_usable() && !crate::sdl::hdr_available(renderer) {
            self.texture.destroy();
            self.exit = Some(Exit::Ended(Some("HDR presentation stopped because the current display/swapchain no longer supports HDR.".into())));
            self.needs_redraw = true;
        }
        self.sync_race(renderer);
        self.sync_vrr(renderer);
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
        // Without a picture or a redraw, Frame Warp may show the held picture again, shifted anew.
        if !has_picture && !self.needs_redraw && (sliced || !self.warp_repeat(now)) {
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
                // SAFETY: a live renderer and its window.
                let exclusive = unsafe {
                    let window = SDL_GetRenderWindow(renderer);
                    SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0 && !SDL_GetWindowFullscreenMode(window).is_null()
                };
                let aim_us = present::phase_lock_aim_us(skipping, exclusive, self.present_deadline_us, refresh_hz);
                let timing = phase::aimed(phase::timing(renderer, refresh_hz), aim_us);
                let steady_windows = if skipping { self.phase_steady_windows } else { 0 };
                if let Some(report) = self.phase.sample(p.decoded.ready_us, begin, timing, steady_windows) {
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
            if !self.scan_out(&p.decoded) && !self.texture.upload(renderer, &p.decoded, rows) {
                if p.decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                    let reason = self.texture.failure.take().unwrap_or_else(|| "PyroWave GPU reconstruction failed.".into());
                    self.exit = Some(Exit::Ended(Some(reason)));
                } else if p.decoded.setup.chroma_packing != 0 {
                    self.exit = Some(Exit::Ended(Some("Packed full-chroma reconstruction failed: the decoded planes, GPU, or HDR output are unsupported.".into())));
                } else {
                    let format = p.decoded.picture.format_name();
                    let toast = match self.texture.failure.take() {
                        Some(reason) => trf("Cannot show {} pictures: {}", &[&format, &reason]),
                        None => trf("Cannot show {} pictures", &[&format]),
                    };
                    self.show_toast(toast);
                }
            }
            if !self.first_frame {
                self.first_frame = true;
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(p) = &picture { self.glass.next_picture(p.decoded.number, p.decoded.ready_us); }
        // Frame Warp's shift for the picture, as late before its draw as it can be.
        if let Some(warp) = self.frame_warp.as_mut() {
            if let Some(p) = &picture { warp.picture(p.decoded.number, p.decoded.race_seq, p.decoded.ready_us); }
            self.texture.warp = warp.draw(None);
        }
        let flipped = match self.kms_flip(&mut draw_overlay) {
            Some(flipped) => flipped,
            None => self.render_flip(renderer, &mut draw_overlay, has_picture),
        };
        self.needs_redraw = false;
        // The pictures the compositor answered for: the doctor's display time runs to the glass.
        #[cfg(target_os = "linux")]
        let (glass_row, to_glass) = {
            let mut row = crate::timing_csv::Glass::default();
            for shown in self.glass.poll(flipped.returned_us) {
                if let Some(glass_us) = shown.glass_us() { self.doctor.shown(0, shown.ready_us, glass_us); }
                if let wl_presentation::Outcome::Presented { to_glass_ns, flags } = shown.outcome {
                    row = crate::timing_csv::Glass { frame: Some(shown.frame), to_glass_us: to_glass_ns.map(|ns| ns / 1000), flags: Some(flags) };
                }
            }
            (row, self.glass.timing(flipped.returned_us))
        };
        #[cfg(not(target_os = "linux"))]
        let (glass_row, to_glass) = (crate::timing_csv::Glass::default(), false);
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
        let flip = if (self.timing_csv.is_some() || watches) && picture.is_some() { phase::present_stats(renderer) } else { Default::default() };
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
    fn kms_flip(&mut self, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer)) -> Option<Flipped> {
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
        let issued_us = session::now_us();
        let committed = self.kms.as_mut().map_or(Ok(()), kms_scanout::Scanout::commit);
        let returned_us = session::now_us();
        if committed.is_err() {
            self.add_toast(tr("Video planes are unavailable; the stream is drawn by the renderer. The log says why."));
            self.needs_redraw = true;
            return None;
        }
        Some(Flipped { issued_us, returned_us, scan: None, refused: false })
    }

    #[cfg(not(target_os = "linux"))]
    fn kms_flip(&mut self, _draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer)) -> Option<Flipped> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
