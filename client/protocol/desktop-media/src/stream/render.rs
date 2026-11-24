//! The stream's picture loop: taking decoded pictures and slices, pacing them, and drawing,
//! enhancing and presenting them, with the beam race and VRR checks around the present.

use super::*;

mod flip;
mod pacing;
mod pump;

use pump::{exclusive_fullscreen, UNSHOWN_BEFORE_FALLBACK};
pub(super) use pump::{frame_shares, StreamSizes, Unshown};
#[cfg(test)]
use pump::{ai_in_effect, mode_to_apply, setup_size, ModeApply};

/// The end of a stream whose packed 4:4:4 ("Sharper colored text") pictures cannot be shown.
const PACKED_UNSHOWN: &str = en("This device cannot show Sharper colored text for this stream. Turn off Sharper colored text in Settings, then start the stream again.");
/// Toast when AI enhancement fails mid-stream.
#[cfg_attr(not(windows), allow(dead_code))]
const AI_ENHANCE_UNAVAILABLE: &str = en("AI enhancement is unavailable; using bilinear scaling.");
/// Stream end notice when PyroWave pictures cannot be shown.
const PYROWAVE_UNSHOWN: &str = en("PyroWave pictures cannot be shown on this device. Choose another Video codec in Settings, then start the stream again.");

/// What the toast says when the PC did not confirm a requested size or frame rate in time:
/// automatic quality stops only where it was on for the stream.
fn unconfirmed_mode_toast(quality_on: bool) -> &'static str {
    if quality_on { tr("The PC did not confirm the requested size. Automatic quality stopped.") } else { tr("The PC did not apply the new size or frame rate.") }
}

/// A present of the texture: when the call was issued and returned (us), and the beam around it.
struct Flipped {
    issued_us: u64,
    returned_us: u64,
    scan: Option<race_probe::ScanReport>,
    /// The picture was not presented: the last one still waits for the display.
    refused: bool,
    /// The render device was lost or reset during this present: the swap chain must not be read.
    device_lost: bool,
}

impl Streaming {
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
        if self.vrr.recheck_due(now)
            && let Some(probe) = self.race_probe.as_mut() { probe.recheck(); }
        if let Some(interval) = interval {
            if crate::render_device::event_pending() || self.plan.device.is_removed() { return; }
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

    /// Draws the texture and the overlay and presents. `fresh` is a whole newly decoded picture
    /// (for MetalFX reuse); `probe` reads the beam around the present; `due_us` is when the
    /// picture was due, for the wait's budget against the next one.
    fn render_flip(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), probe: bool, #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] fresh: bool, due_us: Option<u64>) -> Flipped {
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
            let ai = self.ai_enhances(renderer, output);
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
                                None => {
                                    // `pass.draw` waits synchronously on the GPU (`[command waitUntilCompleted]`).
                                    self.direct_motion.presenting(Some(self.direct_rules(SDL_GetRenderWindow(renderer))));
                                    let drawn = self.metalfx.as_mut().and_then(|pass| pass.draw(renderer, buffer, &info));
                                    self.direct_motion.presenting(None);
                                    drawn
                                }
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
                    let mut refused = None;
                    if !self.upscaler.as_ref().is_some_and(|u| u.matches(input, output, self.settings.upscaling)) {
                        self.upscaler = upscale::Upscaler::new(renderer, input, output, self.settings.upscaling).map_err(|e| refused = Some(e)).ok();
                    }
                    let drawn = match self.upscaler.as_mut() {
                        Some(u) => u.draw_warped(renderer, texture, self.settings.upscale_sharpness, &warp, &mut self.texture.warp_mesh),
                        None => Err(refused.unwrap_or_default()),
                    };
                    match drawn {
                        Ok(upscaled) => texture = upscaled,
                        Err(reason) => {
                            eprintln!("nightjar: GPU upscaling failed: {reason}; the stream continues with bilinear scaling");
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
        let holds = !mode_tears
            || self.vrr.distrusted
            || (cfg!(target_os = "macos") && self.pacer.mode == PacingMode::HostTimes)
            || self.vrr.sync == present::VrrSync::Present1
            || (probe && matches!(self.vrr.sync, present::VrrSync::Blank | present::VrrSync::Scan))
            || (probe && self.pacer.mode == PacingMode::HostTimes && self.keeper_on && self.keeper_repeats);
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
        if self.check_render_device_lost() { if holds { self.direct_motion.presenting(None); } let now = session::now_us(); return Flipped { issued_us: now, returned_us: now, scan: None, refused: false, device_lost: true }; }
        // With Frame Warp the hold came before its shift (`present`).
        if probe && self.frame_warp.is_none() { self.hold_for_turn(renderer); }
        let issued_us = session::now_us();
        let blank = self.blank_present(renderer, probe);
        // SAFETY: a live renderer.
        let mut latched = false;
        let (refused, present_failed) = unsafe {
            let done = if skips { present::present_without_waiting(renderer, self.skipped.hold(), self.plan.display.refresh_hz) } else { present::NoWait::Failed };
            if skips { self.no_wait_present = done != present::NoWait::Failed; }
            let failed = if let Some(ok) = blank { !ok } else if done != present::NoWait::Failed { false } else {
                // Sync interval 1 shows the picture whole at the next refresh when the wait for
                // the scan would have cost the next picture its turn (`scan_sync::scan_wait`).
                latched = latch && present::present_adaptive(renderer, true);
                !latched && !SDL_RenderPresent(renderer)
            };
            (done == present::NoWait::Refused, failed)
        };
        let device_lost = crate::render_device::after_present_lost(present_failed, &self.plan.device);
        if device_lost { self.note_render_device_lost(crate::render_device::lost_reason().into()); } else if !refused { self.texture.mark_d3d11_flushed(); #[cfg(target_os = "linux")] if !skips || !self.no_wait_present { phase::note_external_present(); } }
        self.flip_guard.last_flip_us = Some(issued_us);
        self.flip_guard.last_flip_latched = latched;
        if latched { self.scan_tally.latched += 1; }
        let returned_us = session::now_us();
        if let Some(warp) = self.frame_warp.as_mut() { warp.presented(returned_us); }
        #[cfg(target_os = "linux")]
        if !device_lost {
            self.window_target.after_present(crate::sdl::window_target_frames(renderer));
            self.video_only_presented(renderer);
        }
        if holds { self.direct_motion.presenting(None); }
        let (fps, probe_tears) = (f64::from(self.facts.fps), self.vrr.probe_tears(self.pacer.mode));
        let scan = self.race_probe.as_mut().filter(|_| probe && !refused && !device_lost).map(|probe| {
            probe.after_present(renderer, beam_before, probe_tears, fps);
            probe.report(beam_before)
        });
        Flipped { issued_us, returned_us, scan, refused, device_lost }
    }

    /// Shows the due picture, if any. Returns whether something was drawn.
    pub fn present(&mut self, renderer: *mut SDL_Renderer, mut draw_overlay: impl FnMut(&mut Self, *mut SDL_Renderer)) -> bool {
        if self.exit.is_some() || self.check_render_device_lost() { return false; }
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
        if self.exit.is_some() || self.check_render_device_lost() { return sliced; }
        let now = session::now_us();
        let overdue = self.skipped.take_due(now);
        let (picture, mut dropped) = self.pacer.take_due(now);
        // A newer frame's slices are on the screen: this older picture would step it back a frame.
        let picture = picture.filter(|p| {
            let older = self.racer.newer_on_screen(p.decoded.race_seq);
            dropped += u32::from(older);
            !older
        });
        let (mut picture, replaced) = present::take_over_kept(picture, &mut self.skipped_picture, overdue);
        dropped += replaced;
        self.window.dropped_pacing += dropped;
        self.shared.counters.replaced.fetch_add(u64::from(dropped), Ordering::Relaxed);
        let has_picture = picture.is_some();
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
        if let Some(p) = &mut picture {
            if self.facts.phase_lock && self.pacer.mode == PacingMode::Vsync && self.focused && !self.menu_visible {
                // With no refreshes to spare a picture has to make the desktop's deadline, which comes before the refresh.
                let refresh_hz = self.plan.display.refresh_hz;
                let skipping = self.no_wait_present && !self.skipped.waits() && self.pacer.skips_instead_of_waiting(refresh_hz);
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
                let timing = clocked.or_else(|| self.display_timing(renderer, begin, refresh_hz, report_held, true));
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
            if self.scan_out(&p.decoded) || self.texture.upload_take(renderer, &mut p.decoded, rows) {
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
        if self.exit.is_some() || self.check_render_device_lost() { return false; }
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
            None => (self.render_flip(renderer, &mut draw_overlay, has_picture, has_picture, picture.as_ref().map(|p| p.present_us)), false),
        };
        self.needs_redraw = false;
        if flipped.device_lost { return true; }
        if !flipped.refused && let Some(frame) = picture.as_ref().and_then(|p| p.decoded.picture.frame()) { frame.mark_d3d11_flushed(); }
        #[cfg(target_os = "linux")]
        if by_kms && !flipped.refused { self.kms_last_flip_us = Some(flipped.returned_us); }
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
            let (refresh_us, was_waiting) = ((1_000_000.0 / self.plan.display.refresh_hz.max(1.0)) as u64, self.skipped.waits());
            if let Some(hold) = self.skipped.skipped(flipped.returned_us, self.pacer.frame_us(), refresh_us) {
                eprintln!("nightjar: pacing: many presents find the last picture waiting, each now tries for {hold:?}");
            } else if !was_waiting && self.skipped.waits() { eprintln!("nightjar: pacing: presents still refused after a whole refresh, waiting for each picture's turn"); }
            return true;
        }
        // A redraw for the menu or the overlay leaves a kept picture to be shown and counted.
        if has_picture && self.skipped.shown() { eprintln!("nightjar: pacing: trying no-wait presents again (hold {:?})", self.skipped.hold()); }
        let presented = flipped.returned_us;
        let scan = flipped.scan;
        let watches = has_picture && self.no_wait_present && !self.skipped.waits() && self.pacer.skips_instead_of_waiting(self.plan.display.refresh_hz);
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
        // picture or the KMS commit can block on a refresh, as in the renderer's present (`render_flip`);
        // the overlay is drawn before. With Frame Warp the hold came before its shift and gave the
        // input back (`hold_for_turn_lending`), so it is lent again only for the commit's wait.
        let holds = picture && self.vrr.held(self.pacer.mode) > 1;
        let lends = holds || self.kms.as_ref().is_some_and(kms_scanout::Scanout::may_block);
        if lends { self.direct_motion.presenting(Some(self.direct_rules(window))); }
        // KMS has no margin to call for a renderer: `held_margin_us` answers before it would.
        if holds && self.frame_warp.is_none() { self.hold_for_turn(std::ptr::null_mut()); }
        let issued_us = session::now_us();
        let committed = self.kms.as_mut().map_or(Ok(()), kms_scanout::Scanout::commit);
        let returned_us = session::now_us();
        if lends { self.direct_motion.presenting(None); }
        if committed.is_err() {
            self.add_toast(tr("Video planes are unavailable; the stream is drawn by the renderer. The log says why."));
            self.needs_redraw = true;
            return None;
        }
        Some(Flipped { issued_us, returned_us, scan: None, refused: false, device_lost: false })
    }

    #[cfg(not(target_os = "linux"))]
    fn kms_flip(&mut self, _window: *mut SDL_Window, _draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), _picture: bool) -> Option<Flipped> {
        None
    }
}

#[cfg(test)]
mod tests;
