//! The stream's picture loop: taking decoded pictures and slices, pacing them, and drawing,
//! enhancing and presenting them, with the beam race and VRR checks around the present.

use super::*;

/// What the raced presenter guesses of a stream's slice times before it has seen a frame.
fn seeded_predictor(facts: &Facts) -> race::Predictor {
    let frame_bytes = f64::from(facts.bitrate_kbps.max(1)) * 1000.0 / 8.0 / f64::from(facts.fps.max(1));
    race::Predictor::seeded(facts.race_slices, facts.height.max(1) as u32, frame_bytes, facts.pace_mbps)
}

/// A present of the texture: when the call was issued and returned (us), and the beam around it.
struct Flipped {
    issued_us: u64,
    returned_us: u64,
    scan: Option<race_probe::ScanReport>,
}

impl Streaming {
    /// Moves decoded pictures into the pacer; sends a clock sync probe when due.
    pub fn pump(&mut self) {
        self.cursor.tick(Instant::now());
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
        for decoded in frames.drain(..) {
            if decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 { self.race_setup = Some(decoded.setup); }
            if let Some(frame) = decoded.picture.frame().filter(|_| decoded.setup.chroma_packing == 0 && decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE == 0) {
                let info = frame.info();
                self.facts.width = info.width;
                self.facts.height = info.height;
                if self.pending_mode.is_some_and(|(mode, _)| i32::from(mode.width) == info.width && i32::from(mode.height) == info.height) {
                    self.pending_mode = None;
                }
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
        let wait = match due {
            Some(due) => {
                let now = session::now_us();
                if due <= now {
                    Wait::Now
                } else {
                    Wait::For(Duration::from_micros(due - now))
                }
            }
            None => Wait::For(Duration::from_millis(if self.controller_mouse_active || self.audio_status.0.iter().any(|flags| *flags != 0) { 20 } else if self.overlay_visible { 250 } else { 500 })),
        };
        // A hide from the PC waiting out its delay wakes the loop when due.
        match (wait, self.cursor.hide_in(Instant::now())) {
            (Wait::For(wait), Some(hide)) => Wait::For(wait.min(hide)),
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
        let probe = self.race_probe.as_ref();
        let interval = self.vrr.step(&mut self.pacer.mode, probe.map(race_probe::RaceProbe::verdict));
        if self.pacer.mode != before {
            eprintln!("nightjar: pacing: {}, this display does not refresh on present (display check: {})",
                self.pacer.mode.label(), probe.map_or(String::new(), race_probe::RaceProbe::line));
        }
        if let Some(interval) = interval {
            // SAFETY: the live renderer belongs to the presenting thread.
            if !unsafe { SDL_SetRenderVSync(renderer, interval) } {
                eprintln!("nightjar: pacing: could not set the sync interval to {interval}: {}", crate::sdl::error());
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

    /// Draws the picture itself into the back buffer (`direct`), or returns false for SDL to draw
    /// the video texture.
    #[cfg(windows)]
    pub(super) fn draw_direct(&mut self, renderer: *mut SDL_Renderer, dst: &SDL_FRect, texture: *mut SDL_Texture) -> bool {
        self.texture.draw_direct(renderer, dst, texture)
    }

    #[cfg(not(windows))]
    pub(super) fn draw_direct(&mut self, _renderer: *mut SDL_Renderer, _dst: &SDL_FRect, _texture: *mut SDL_Texture) -> bool {
        false
    }

    /// Draws the texture and the overlay and presents. `probe` reads the beam around the present.
    fn render_flip(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), probe: bool) -> Flipped {
        // SAFETY: drawing on a live renderer.
        unsafe {
            SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
            SDL_RenderClear(renderer);
            let (mut w, mut h) = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h);
            let dst = viewport(self.texture.size(), (w, h), self.settings.video_scaling, self.settings.align_video_top);
            if !self.texture.texture.is_null() {
                let mut texture = self.texture.texture;
                let output = (dst.w as i32, dst.h as i32);
                let input = self.texture.size();
                let use_upscaler = self.settings.upscaling.is_shader() && !self.upscale_failed
                    && !self.texture.is_hdr() && output.0 >= input.0 && output.1 >= input.1 && output != input
                    && output.0 <= 16384 && output.1 <= 16384;
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
                    match self.upscaler.as_ref().and_then(|u| u.draw(renderer, texture, self.settings.upscale_sharpness)) {
                        Some(upscaled) => texture = upscaled,
                        None => {
                            self.upscaler = None;
                            self.upscale_failed = true;
                            self.show_toast(tr("GPU upscaling is unavailable; using bilinear scaling."));
                        }
                    }
                } else { self.upscaler = None; }
                if !self.draw_direct(renderer, &dst, texture) {
                    if texture == self.texture.texture { texture = self.texture.sdl_texture(renderer); }
                    SDL_RenderTexture(renderer, texture, std::ptr::null(), &dst);
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
        let issued_us = session::now_us();
        // SAFETY: a live renderer.
        unsafe { SDL_RenderPresent(renderer) };
        let returned_us = session::now_us();
        let mode_tears = self.pacer.mode != PacingMode::Vsync;
        let fps = f64::from(self.facts.fps);
        let scan = self.race_probe.as_mut().filter(|_| probe).map(|probe| {
            probe.after_present(renderer, beam_before, mode_tears, fps);
            probe.report(beam_before)
        });
        Flipped { issued_us, returned_us, scan }
    }

    /// Shows the due picture, if any. Returns whether something was drawn.
    pub fn present(
        &mut self,
        renderer: *mut SDL_Renderer,
        mut draw_overlay: impl FnMut(&mut Self, *mut SDL_Renderer),
    ) -> bool {
        if self.texture.needs_hdr_output() && !crate::sdl::hdr_available(renderer) {
            self.texture.destroy();
            self.exit = Some(Exit::Ended(Some("HDR presentation stopped because the current display/swapchain no longer supports HDR.".into())));
            self.needs_redraw = true;
        }
        self.sync_race(renderer);
        self.sync_vrr(renderer);
        // The slices of a frame still being received go out ahead of the beam.
        let sliced = self.present_slices(renderer, &mut draw_overlay, session::now_us());
        let now = session::now_us();
        let (picture, mut dropped) = self.pacer.take_due(now);
        // A newer frame's slices are on the screen: this older picture would step it back a frame.
        let picture = picture.filter(|p| {
            let older = self.racer.newer_on_screen(p.decoded.race_seq);
            dropped += u32::from(older);
            !older
        });
        self.window.dropped_pacing += dropped;
        self.shared.counters.replaced.fetch_add(u64::from(dropped), Ordering::Relaxed);
        let has_picture = picture.is_some();
        if !has_picture && !self.needs_redraw {
            return sliced;
        }
        let begin = session::now_us();
        // Whether the frame is under the racer, and which rows of it are left to draw.
        let mut raced = false;
        let mut rows = None;
        if let Some(p) = &picture {
            if self.facts.phase_lock && self.pacer.mode == PacingMode::Vsync && self.focused && !self.menu_visible {
                let timing = phase::timing(renderer, self.plan.display.refresh_hz);
                if let Some(report) = self.phase.sample(p.decoded.ready_us, begin, timing) {
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
            if !self.texture.upload(renderer, &p.decoded, rows) {
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
        let flipped = self.render_flip(renderer, &mut draw_overlay, has_picture);
        let presented = flipped.returned_us;
        let scan = flipped.scan;
        let flip = if self.timing_csv.is_some() && picture.is_some() { phase::present_stats(renderer) } else { Default::default() };
        self.needs_redraw = false;
        if let Some(p) = picture {
            let timeline = self.finish_frame(&p.decoded, raced, flipped.issued_us, flipped.returned_us);
            let w = &mut self.window;
            w.shown += 1;
            w.render_us += presented - begin;
            w.hold_us += p.hold_us;
            w.late += u32::from(p.late);
            w.network_us += p.decoded.enqueue_us.saturating_sub(p.decoded.receive_us);
            w.network_n += 1;
            self.doctor.shown(p.decoded.host_latency, p.decoded.ready_us, presented);
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
        }
        true
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
