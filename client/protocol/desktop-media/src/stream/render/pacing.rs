//! VRR pacing, fractional V-Sync hold, scan-line wait, beam-race slices, and LFC/warp repeats.

use super::super::*;
use super::pump::{exclusive_fullscreen, refresh_period_us, seeded_predictor};

impl Streaming {
    /// Works out whether the display allows racing slices and, if so, how the picture sits on its
    /// beam; switches the pacing between `Immediate` and `BeamRace` when that changes.
    pub(in crate::stream) fn sync_race(&mut self, renderer: *mut SDL_Renderer) {
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
    pub(super) fn vsync_refreshes(&mut self) -> u32 {
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
    pub(super) fn hold_for_turn(&mut self, renderer: *mut SDL_Renderer) {
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
            // Kernel high-resolution timer sleep (no CPU busy-spin): fractional V-Sync still has
            // the rest of the opening refresh minus HELD_LATCH_GUARD_US to latch the flip.
            // SAFETY: a plain wait.
            unsafe { SDL_DelayNS(wait_us * 1000) };
        }
    }

    /// Turns Tear-free VRR to sync interval 1 once the display check finds presents to an idle
    /// display that start no scan (`scan_sync::UNTRUSTED_MISSES`), or a stream of this run found
    /// that on the same display, and says so once (`present::VrrPacing::distrust_scan`).
    pub(super) fn distrust_scan_line(vrr: &mut present::VrrPacing, probe: &race_probe::RaceProbe, wait: crate::config::VrrWait) {
        let display = probe.display_id();
        // Nightjar's wait does not take the display check's switch to Windows V-Sync, so choosing
        // it forgets a display an earlier stream found not refreshing on present: Automatic starts
        // afresh there. (A wait that would cost the next picture its turn still presents with sync
        // interval 1, `scan_sync::scan_wait`; that is the wait's own budget, not this verdict.)
        if wait == crate::config::VrrWait::Scan
            && let Some(display) = display { scan_sync::clear_untrusted(display); }
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
    pub(super) fn wait_out_scan(&mut self, renderer: *mut SDL_Renderer, due_us: Option<u64>) -> bool {
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
            match unsafe { scan_sync::finish_drawing(renderer, Duration::from_nanos(scan_sync::DRAWING_LIMIT_NS), &mut self.scan_query) } {
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
        if syncs
            && let (Some(left), Some(due)) = (beam.scan_left_ns(&sample), due_us) {
                let budget_ns = scan_sync::wait_budget_ns(due, self.pacer.frame_us(), session::now_us());
                let landing_ns = self.scan_tally.landing.median_ns().unwrap_or(scan_sync::TEAR_LANDING_NS);
                if scan_sync::scan_wait(left, budget_ns, landing_ns) == scan_sync::ScanWait::Latch { return true; }
            }
        // Until the beam leaves the last row, reading it again after each sleep: at most one scan
        // plus `SCAN_WAIT_SLACK_NS`, since a reading can be wrong (`scan_sync::wait_out`).
        let begun = Instant::now();
        let probe = self.race_probe.as_ref();
        // SAFETY: SDL_DelayNS is a plain high-resolution timer sleep (0.5 ms granularity on Windows).
        let Some(waited) = scan_sync::wait_out(&beam, sample, || probe.and_then(race_probe::RaceProbe::before),
            |ns| unsafe { SDL_DelayNS(ns) }, || begun.elapsed().as_nanos() as u64) else { return false };
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
        // SAFETY: SDL_DelayNS is a plain high-resolution timer sleep.
        let (last, waited, landed) = scan_sync::wait_landing(beam, sample,
            Duration::from_nanos(scan_sync::LANDING_LIMIT_NS - since_present_ns),
            || probe.before(), |ns| unsafe { SDL_DelayNS(ns) });
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
    /// flip. Returns `Some(ok)` when it presented, or `None` to leave the present to the caller.
    pub(super) fn blank_present(&mut self, renderer: *mut SDL_Renderer, probe: bool) -> Option<bool> {
        if !cfg!(windows) || !probe || self.vrr.sync != present::VrrSync::Blank
            || !matches!(self.pacer.mode, PacingMode::Immediate | PacingMode::HostTimes) { return None; }
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
        Some(unsafe { present::present_adaptive(renderer, latched) || SDL_RenderPresent(renderer) })
    }

    /// Takes the slices the decoder finished since the last call into the racer.
    pub(super) fn observe_slices(&mut self, slices: &mut VecDeque<pyrowave::SliceReady>) {
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
    pub(super) fn present_slices(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), now: u64) -> bool {
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
        let flipped = self.render_flip(renderer, draw_overlay, true, false, None);
        self.needs_redraw = false;
        if flipped.device_lost { return true; }
        self.racer.presented(flipped.issued_us, &scan);
        for (band, _) in &batch {
            self.latency.record(self.racer.presentable_us(band.ready_us), flipped.issued_us, flipped.returned_us, flipped.returned_us);
        }
        true
    }

    #[cfg(not(windows))]
    pub(super) fn present_slices(&mut self, _renderer: *mut SDL_Renderer, _draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), _now: u64) -> bool {
        false
    }

    /// The PC's frame times at V-Sync: the display's refresh grid and the desktop's deadline before
    /// a refresh, for the pictures accepted next (`Pacer::set_grid`). Exclusive fullscreen, which
    /// nothing composes, has no deadline before the refresh.
    pub(super) fn sync_host_grid(&mut self, renderer: *mut SDL_Renderer) {
        let refresh_hz = self.plan.display.refresh_hz;
        let deadline_us = if exclusive_fullscreen(renderer) { 0 } else {
            self.present_deadline_us.unwrap_or_else(|| present::deadline_before_refresh(refresh_hz))
        };
        // The refresh grid itself: a hold of 1 never reads the anchor.
        let grid = self.display_timing(renderer, session::now_us(), refresh_hz, 1, false).map(|(refresh_us, period_ns)| present::refresh_grid(refresh_us, period_ns));
        self.pacer.set_grid(grid, deadline_us);
    }

    /// When the next picture that already waits is due.
    pub(super) fn next_picture_us(&self) -> Option<u64> {
        match (self.pacer.next_due(), self.skipped.due_us()) {
            (Some(due), Some(skipped)) => Some(due.min(skipped)),
            (due, skipped) => due.or(skipped),
        }
    }

    /// Turns the keeper on while the stream tears whole pictures onto a display the check found
    /// refreshing on present and whose scan it calibrated, and off otherwise (`lfc::Keeper`).
    pub(in crate::stream) fn sync_keeper(&mut self) {
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
    pub(super) fn lfc_repeat(&mut self, now: u64) -> bool {
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
    pub(super) fn warp_repeat(&mut self, now: u64) -> bool {
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
}
