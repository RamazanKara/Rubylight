//! Stream session startup (`Streaming::new`) and teardown (`Streaming::finish`).

use super::*;

impl Streaming {
    pub fn new(connecting: Connecting, mut session: Session, facts: Facts, display: Display) -> Self {
        let min_interval = display.refresh_interval_us().unwrap_or(0);
        crate::sdl::alt_f4_to_host(true);
        crate::sdl::keep_screen_awake(true);
        if facts.host_timing {
            let _ = session.send(ClientMessage::HostTimingRequest(HostTimingRequest { enabled: true }));
        }
        if facts.link_notice {
            let _ = session.send(ClientMessage::LinkNoticeRequest(LinkNoticeRequest { enabled: true }));
        }
        if facts.pipeline_rates {
            // Every second, as the overlay refreshes.
            let _ = session.send(ClientMessage::PipelineRatesRequest(PipelineRatesRequest { enabled: true, interval_ms: 1000 }));
        }
        let quick_suspend = quick_suspend::QuickSuspend::new(facts.quick_suspend);
        // Without the request the PC sends no notices, so the keyboard stays as the user leaves it.
        if facts.text_input && connecting.settings.keyboard_follows_text_fields
            && let Err(error) = session.send(ClientMessage::TextInputRequest(text_field::request())) {
                eprintln!("nightjar: text field: could not ask for notices: {error}");
            }
        let settings = connecting.settings.clone();
        let mic_refused = connecting.plan.mic_only && !session.microphone_available();
        let facts_host = facts.host_name.clone();
        let microphone = if settings.microphone && !connecting.plan.microphone_muted && facts.role != AppRole::RemoteMonitor {
            if session.microphone_available() {
                match microphone::Microphone::open(connecting.shared.clone(), microphone::echo_for(connecting.plan.mic_only, settings.mic_echo_cancel), settings.mic_gain, mic_dsp::Choice::of(&settings)) {
                    Ok(microphone) => Some(microphone),
                    Err(error) => {
                        connecting.shared.notify(Notice::MicrophoneFailed(error));
                        None
                    }
                }
            } else if !settings.controller_audio && !mic_refused {
                connecting.shared.notify(Notice::MicrophoneFailed("The PC did not offer an encrypted microphone stream.".into()));
                None
            } else { None }
        } else { None };
        let bitrate_kbps = facts.bitrate_kbps.max(1) as u32;
        let mode = Reconfigure { width: facts.width as u16, height: facts.height as u16, fps_millihz: facts.fps as u32 * 1000 };
        let mut quality_mode = if facts.reconfigure { settings.adaptive_quality } else { QualityMode::Off };
        let mut quality_failed = false;
        let quality_policy = match quality_mode {
            QualityMode::Off => None,
            _ => match quality::Quality::new(quality_mode, mode, settings.quality_min_short_edge, settings.quality_min_fps, 0) {
                Ok(mut policy) => {
                    // After an automatic reconnect it returns to the step it had reached.
                    if let Some(step) = connecting.plan.resume_quality { policy.resume_at(step); }
                    Some(policy)
                }
                Err(error) => {
                    eprintln!("nightjar: automatic quality off for {}x{}@{}: {error}", facts.width, facts.height, facts.fps);
                    quality_mode = QualityMode::Off;
                    quality_failed = true;
                    None
                }
            },
        };
        let rate_change = facts.rate_change.then(|| {
            let hint = settings.lower_bitrate.client_hints();
            let mut rates = rate_change::RateChange::new(
                bitrate_kbps, facts.fps.max(1) as u32 * 1000, hint, connecting.plan.bitrate_limit_kbps, 0);
            // A reconnect is a new session on the PC, whose controllers lower the bitrate again: Never,
            // and a bitrate the player chose before it, are sent once more.
            if rate_change::takeover_at_start(settings.lower_bitrate.keeps(), connecting.plan.bitrate_before_manual.is_some(), hint) { rates.keep_bitrate(); }
            // The player's bitrate came from this PC's answers: Keep my bitrate keeps it, whatever the query answers.
            if connecting.plan.bitrate_before_manual.is_some() { rates.chosen_before(bitrate_kbps); }
            // After an automatic reconnect the client-hint controller returns to the rate it had cut to.
            if let Some(kbps) = connecting.plan.resume_bitrate_kbps { rates.resume_hint(kbps); }
            rates
        });
        if settings.lower_bitrate.keeps() && rate_change.is_none() {
            eprintln!("nightjar: Never: keep my bitrate is chosen, but this PC takes no bitrate changes from the device; the stream runs as The PC decides");
        }
        let backlog = connecting.shared.pacer_backlog.clone();
        // The stream controls read the attachments each time they are drawn. If the host USB
        // capabilities ask is still in flight, defer setup until `Streaming::pump` receives it.
        let (usb, usb_refused) = if connecting.shared.usb_pending.load(std::sync::atomic::Ordering::Acquire) {
            (None, None)
        } else {
            Self::init_usb(facts.usb, &settings, &connecting.shared, &mut session)
        };
        let hdr_stream = present::hdr_negotiated(settings.hdr, facts.format);
        let vrr_fallback = present::without_vrr(&settings);
        let stream_fps = u32::try_from(facts.fps).unwrap_or(0);
        if settings.host_frame_times {
            eprintln!("nightjar: PC's frame times without VRR at {stream_fps} fps");
        } else if settings.frame_pacing != config::FramePacing::Latency {
            eprintln!("nightjar: frame pacing {:?} without VRR at {stream_fps} fps", settings.frame_pacing);
        }
        let vrr_check = checks_display_for_vrr(connecting.plan.vrr, connecting.pacing, vrr_fallback, env_flag("NIGHTJAR_VRR_CHECK"));
        #[cfg(target_os = "linux")]
        let keeps_mailbox = !present::prefers_immediate(true, crate::sdl::video_driver() == "wayland", crate::handheld::gamescope());
        #[cfg(not(target_os = "linux"))]
        let keeps_mailbox = false;
        let present_check = checks_display_for_present_mode(connecting.plan.vrr, connecting.pacing, vrr_check, keeps_mailbox, env_flag("NIGHTJAR_VRR_CHECK"));
        let sync_env = std::env::var("NIGHTJAR_VRR_SYNC").ok();
        let vrr_sync = if connecting.plan.vrr { present::VrrSync::choose(sync_env.as_deref(), settings.vrr_tear_free, cfg!(windows)) } else { present::VrrSync::Off };
        if sync_env.is_some() && connecting.plan.vrr { eprintln!("nightjar: VRR sync {} (NIGHTJAR_VRR_SYNC)", vrr_sync.label()); }
        // `NIGHTJAR_VRR_LFC=1` has the stream repeat pictures for a VRR panel itself
        // (`lfc::Keeper`), `count` only counts the pictures that meet a scan under way; without
        // it the stream reads no scan line for this. The scan line is read on Windows.
        let lfc_mode = std::env::var("NIGHTJAR_VRR_LFC").ok();
        let lfc_repeats = lfc_mode.as_deref().is_some_and(|mode| connect::flag_value(mode) == Some(true));
        let lfc_counts = lfc_mode.as_deref().is_some_and(|mode| mode.trim().eq_ignore_ascii_case("count"));
        let vrr_lfc = cfg!(windows) && connecting.plan.vrr && connecting.pacing != PacingMode::Vsync && (lfc_repeats || lfc_counts);
        if matches!(vrr_sync, present::VrrSync::Scan | present::VrrSync::Blank) && !vrr_check {
            eprintln!("nightjar: VRR sync {} reads the scan line; the display check's verdict changes no pacing on this stream", vrr_sync.label());
        }
        let sync_set = sync_env.as_deref().is_some_and(|sync| !sync.trim().is_empty());
        let vrr_pacing = present::VrrPacing::new(vrr_check, vrr_fallback, vrr_sync, connecting.pacing).with_present_check(present_check).with_lfc(vrr_lfc)
            .with_wait(settings.vrr_wait, sync_set);
        if vrr_pacing.distrusted {
            eprintln!("nightjar: VRR: tear-free presents wait for the refresh (sync interval 1), as Whole frames under VRR asks");
        } else if vrr_sync == present::VrrSync::Scan && settings.vrr_wait == config::VrrWait::Scan && !sync_set {
            eprintln!("nightjar: VRR: tear-free presents keep Nightjar's wait for the scan line, and go out with sync interval 1 (whole at the next refresh) when that wait would cost the next picture its turn, as Whole frames under VRR asks");
        }
        let mut doctor = doctor::Doctor::default();
        doctor.receive_buffer = session.video_receive_buffer();
        let direct_motion = direct_motion::DirectMotion::new(session.input_handle(), facts.touch);
        let warp_presenter = facts.frame_warp.clone().map(frame_warp::Presenter::new);
        phase::reset_display_timing();
        let reconnect_recovery = connecting.plan.recovery.is_some();
        let mut streaming = Self {
            usb,
            usb_refused,
            direct_motion,
            #[cfg(windows)]
            _wlan: facts.wlan_streaming.then(wlan::StreamingMode::begin).flatten(),
            session,
            shared: connecting.shared,
            facts,
            pacer: Pacer::with_backlog(connecting.pacing, min_interval, backlog).with_frame_pacing(settings.frame_pacing, stream_fps).with_drop_late_frames(settings.drop_late_frames)
                .with_host_frame_times(settings.host_frame_times),
            texture: VideoTexture::new(connecting.settings.upscaling == config::Upscaling::Bilinear, hdr_stream),
            upscaler: None,
            #[cfg(target_os = "macos")]
            metalfx: None,
            #[cfg(target_os = "macos")]
            metalfx_blocked: None,
            #[cfg(windows)]
            enhancer: None,
            upscale_failed: false,
            #[cfg(target_os = "linux")]
            kms_last_flip_us: None,
            #[cfg(target_os = "linux")]
            phase_source_logged: None,
            #[cfg(target_os = "linux")]
            phase_missing_logged: false,
            #[cfg(target_os = "linux")]
            phase_first_miss_us: None,
            phase: phase::Phase::default(),
            race_probe: None,
            // The sync interval as `App::begin_stream` set it.
            vrr: vrr_pacing,
            skip_over_waiting: env_flag("NIGHTJAR_PRESENT_SKIP").unwrap_or(true),
            present_deadline_us: std::env::var("NIGHTJAR_PRESENT_DEADLINE_US").ok().and_then(|us| us.parse().ok()),
            held_margin_us: std::env::var("NIGHTJAR_HELD_MARGIN_US").ok().and_then(|us| us.parse().ok()).inspect(|us| {
                eprintln!("nightjar: NIGHTJAR_HELD_MARGIN_US: fractional V-Sync's held pictures must make their turn {us} us before the refresh");
            }),
            held_guard_us: phase::held_guard_from_env(),
            held_report_refresh: std::env::var_os("NIGHTJAR_HELD_REPORT").is_some_and(|v| v == "0"),
            held_anchor: phase::HeldAnchor::default(),
            held_wait_us: None,
            held_draw: present::DrawTime::default(),
            held_clock: phase::HeldClock::default(),
            held_untimed_logged: false,
            held_logged: 1,
            held_stats: phase::StatsWatch::default(),
            no_wait_present: false,
            skipped: present::SkippedPicture::new(std::env::var("NIGHTJAR_PRESENT_HOLD_US").ok().and_then(|us| us.parse().ok()).map_or(present::REFUSED_HOLD, Duration::from_micros)),
            skipped_picture: None,
            uploaded_epoch: 0,
            miss_watch: present::MissWatch::default(),
            #[cfg(target_os = "linux")]
            gamescope_fifo: present::FifoWatch::default(),
            phase_steady_windows: std::env::var("NIGHTJAR_PHASE_STEADY_WINDOWS").ok().and_then(|windows| windows.parse().ok()).unwrap_or(phase::STEADY_WINDOWS),
            scan_waits: (0, 0),
            scan_tally: scan_sync::Tally::default(),
            scan_query: scan_sync::DrawQuery::new(),
            scan_settles: env_flag("NIGHTJAR_VRR_SETTLE") != Some(false),
            keeper: None,
            keeper_on: false,
            keeper_repeats: lfc_repeats,
            flip_guard: present::FlipGuard::default(),
            race_scan: None,
            racer: race::Racer::default(),
            predictor: None,
            latency: race::Latency::default(),
            race_margin_ms: hint_override("NIGHTJAR_BEAM_RACE_MARGIN_MS", settings.beam_race_margin_ms),
            race_present: env_flag("NIGHTJAR_BEAM_RACE_PRESENT").unwrap_or(true),
            race_setup: None,
            overlay_visible: settings.overlay,
            #[cfg(target_os = "linux")]
            kms: None,
            #[cfg(target_os = "linux")]
            _gpu_hold: settings.high_performance_gpu_power.then(gpu_hold::acquire),
            overlay_mode: settings.overlay_mode,
            overlay_corner: settings.overlay_position,
            overlay_kept: (settings.overlay, settings.overlay_mode, settings.overlay_position),
            overlay: Overlay::default(),
            hardware_poll: HardwarePoll::default(),
            hardware: None,
            link_notice: None,
            link_lowered_told: false,
            keep_change: None,
            pipeline_rates: None,
            // A PC that set up no microphone gives Microphone only nothing to do.
            exit: mic_refused.then(|| Exit::Ended(Some(trf(MSG_MIC_NOT_ACCEPTED, &[&facts_host])))),
            toast: None,
            network_lost: false,
            network_moved: false,
            retry_now: false,
            host_id: connecting.host_id,
            absolute: settings.mouse_mode == MouseMode::Absolute,
            settings,
            microphone,
            cursor: cursor::Cursor::default(),
            cursor_warning: false,
            text_field: text_field::TextField::default(),
            keyboard_layout: None,
            clipboard_busy: false,
            clipboard_sync: None,
            command_busy: false,
            last_command: None,
            audio_status: ([0; 16], Instant::now() - Duration::from_secs(1)),
            controller_mouse_active: false,
            guide_menu: false,
            guide_waiting: false,
            host_audio: Default::default(),
            guests: Default::default(),
            quick_suspend,
            pending_command: None,
            bitrate_busy: false,
            packed_hint_shown: false,
            bitrate_kbps,
            adaptive_bitrate: false,
            bitrate_policy: None,
            host_abr: None,
            host_abr_start: None,
            host_abr_stop: None,
            doctor,
            rate_change,
            quality_mode,
            quality_policy,
            quality_ceiling: mode,
            decode_window_us: session::now_us(),
            decode_generation: 0,
            requested_mode: mode,
            pending_mode: None,
            late_mode: None,
            fps_evidence: 0,
            mode_before: None,
            stream_sizes: render::StreamSizes::new((i32::from(mode.width), i32::from(mode.height))),
            display_change: None,
            last_arrival: None,
            taken_frames: VecDeque::new(),
            taken_slices: VecDeque::new(),
            jitter_ms: -1.0,
            focused: false,
            microphone_muted: connecting.plan.microphone_muted,
            microphone_unavailable: false,
            microphone_busy: None,
            plan: connecting.plan,
            controllers_off: Default::default(),
            export_statistics: false,
            statistics: VecDeque::new(),
            menu_visible: false,
            session_page_open: false,
            session_page_busy: false,
            close_session_page_requested: false,
            quit_on_leave: false,
            shortcut_sheet: None,
            keys_down: HashSet::new(),
            shortcut_held: None,
            mouse_down: HashSet::new(),
            touch: touch::Touch::default(),
            touchpad: touchpad::Touchpad::default(),
            captured: false,
            fullscreen: false,
            offset: ClockOffset::default(),
            timing_csv: open_timing_csv(reconnect_recovery),
            #[cfg(target_os = "linux")]
            glass: wl_presentation::Glass::default(),
            #[cfg(target_os = "linux")]
            window_target: WindowTargetCount::default(),
            #[cfg(target_os = "linux")]
            swapchain_hdr_metadata: None,
            rtp_clock: RtpClock::default(),
            last_probe: Instant::now(),
            window: Window::default(),
            last_counters: (0, 0, 0, 0, 0, 0, 0),
            last_video_bytes: 0,
            last_wire_bytes: 0,
            host_average: HostAverage::default(),
            last_early: (0, 0),
            last_video: (0, 0, 0),
            last_skipped: 0,
            behind: stats::Behind::default(),
            started: Instant::now(),
            first_frame: false,
            unshown: render::Unshown::default(),
            needs_redraw: true,
            hdr_output: hdr_stream.then(|| HdrOutput::new(true)),
            timings: VecDeque::new(),
            frame_warp: warp_presenter,
        };
        streaming.report_keyboard_layout();
        streaming.query_quick_suspend();
        // Before anything that sets a bitrate: what the PC counts this stream's bitrate as.
        if let Some(request) = streaming.rate_change.as_mut().map(|rates| rates.query_request(0)) { streaming.send_rate_query(request); }
        streaming.start_clipboard_sync();
        if quality_failed {
            streaming.show_toast(tr(MSG_AUTO_QUALITY_NO_MODE));
        }
        streaming
    }

    /// Ends the stream: releases input, stops the session on a reaper thread. Returns the request
    /// that turns the PC's automatic bitrate off while it runs (one closing Nightjar waits for it)
    /// and the teardown's handle, which a caller that owns the renderer or starts the next
    /// connection waits for where it must.
    pub fn finish(mut self, window: *mut SDL_Window, pads: &mut Gamepads) -> (Option<host_abr::AbrStop>, Teardown) {
        self.microphone = None;
        self.stop_host_abr();
        if let Some(client) = &*lock(&self.shared.host_client) {
            client.cancel_pending_requests();
        }
        self.raise_all_keys();
        self.session.input(|q| pads.release(q));
        self.text_field.stop(window);
        self.set_capture(window, false);
        let (direct, queued) = self.direct_motion.counts();
        if direct + queued > 0 {
            eprintln!("nightjar: mouse moves: {direct} sent as they arrived, {queued} through the main loop");
        }
        let input = self.direct_motion.input_count();
        if input > 0 {
            eprintln!("nightjar: keys, clicks and controller events: {input} sent during the present");
        }
        self.cursor.clear();
        crate::sdl::alt_f4_to_host(false);
        crate::sdl::keep_screen_awake(false);
        for controller in 0..16 {
            if self.audio_status.0[usize::from(controller)] != 0 {
                let _ = self.session.send(ClientMessage::ControllerAudioStatus { controller, flags: 0 });
            }
        }
        // SAFETY: a live window.
        unsafe { SDL_ShowCursor() };
        pads.reset();
        self.skipped_picture = None;
        self.taken_frames.clear();
        self.taken_slices.clear();
        clear_shared_pictures(&self.shared);
        self.upscaler = None;
        #[cfg(target_os = "macos")]
        { self.metalfx = None; }
        #[cfg(windows)]
        { self.enhancer = None; }
        self.texture.destroy();
        if self.facts.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            eprintln!("nightjar: decoder {}", lock(&self.shared.decoder_name));
        }
        if let Some(summary) = self.texture.pyrowave_stages.summary() {
            eprintln!("nightjar: PyroWave presenter stages {summary} (CPU submit time)");
        }
        #[cfg(target_os = "linux")]
        {
            self.glass.poll(session::now_us());
            for line in self.glass.lines() { eprintln!("nightjar: {line}"); }
            // SAFETY: a live window; its renderer, if any, too.
            let renderer = unsafe { SDL_GetRenderer(window) };
            if self.window_target.presents > 0 && !renderer.is_null() {
                eprintln!("nightjar: {}", window_target_report(&crate::sdl::renderer_name(renderer), self.window_target.direct(), self.window_target.presents));
            }
            // The window outlives the stream: the next one (or the launcher) must not inherit this
            // stream's mastering metadata.
            if matches!(self.swapchain_hdr_metadata, Some(Some(_))) {
                crate::sdl::set_swapchain_hdr_metadata(window, None);
            }
        }
        let stats = self.session.stats();
        eprintln!("{}", video_summary(stats.frames_submitted, stats.frames_skipped, stats.decode_queue_overflows,
            self.shared.counters.replaced.load(Ordering::Relaxed)));
        if self.facts.early_decode {
            let c = &self.shared.counters;
            eprintln!("{}", early_summary(c.early_frames.load(Ordering::Relaxed), c.early_fallbacks.load(Ordering::Relaxed),
                c.early_us_total.load(Ordering::Relaxed), stats.early_chunks_dropped));
        }
        if self.facts.beam_race {
            let c = &self.shared.counters;
            eprintln!("nightjar: beam race: {}", race_line(c.race_frames.load(Ordering::Relaxed), c.race_slices.load(Ordering::Relaxed),
                c.race_unordered.load(Ordering::Relaxed)));
            eprintln!("nightjar: beam race: presenter, {}", self.raced_line());
            eprintln!("nightjar: beam race: present timing, wake {} us, call p50 {} us p95 {} us (slowest {} us), spread {} us, margin {} us ahead of the beam",
                self.latency.wake_us(), self.latency.call_us(), self.latency.call_p95_us(), self.latency.slowest_call_us(), self.latency.spread_us(),
                self.latency.margin_us(self.race_margin_ms));
            let skipped = self.racer.totals.skipped;
            eprintln!("nightjar: beam race: frames not raced: {} with under 1 ms to gain, {} not in slices, {} with the first slice missed",
                skipped[0], skipped[1], skipped[2]);
        }
        if self.vrr.sync == present::VrrSync::Scan || self.vrr.distrusted || self.keeper.is_some() {
            let (waits, ns) = self.scan_waits;
            eprintln!("nightjar: VRR: {waits} presents waited for a scan under way, {:.2} ms each",
                if waits > 0 { ns as f64 / f64::from(waits) / 1e6 } else { 0.0 });
            for line in self.scan_tally.lines() { eprintln!("nightjar: {line}"); }
        }
        if let Some(keeper) = &self.keeper { eprintln!("nightjar: VRR low frame rate: {}", keeper.line()); }
        if self.vrr.sync == present::VrrSync::Blank {
            let guard = &self.flip_guard;
            eprintln!("nightjar: VRR blank sync: {} presents tore in the blank, {} synchronised near the last flip or after a gap, {} synchronised for a scan under way or missing evidence",
                guard.tearing, guard.planned, guard.protected);
        }
        lock(&self.shared.roam_watch).take();
        // `Session::stop` joins the receive, decode and audio threads, which can wait on a socket
        // timeout or a decoder call: a reaper thread stops it while the launcher comes back.
        let session = self.session;
        (self.host_abr_stop.take(), spawn_background(move || session.stop()))
    }
}

/// Drains pictures and slice pins held inside [`Shared`] so detached threads holding
/// `Arc<Shared>` (such as `start_host_abr`) can never drop GPU frames or plane-set pins
/// after teardown or renderer destruction.
fn clear_shared_pictures(shared: &Shared) {
    lock(&shared.frames).clear();
    lock(&shared.pacer_backlog).clear();
    lock(&shared.slices).clear();
}

#[cfg(test)]
mod pipeline_finish_cleanup_tests {
    use super::*;

    #[test]
    fn clear_shared_pictures_drains_frames_pacer_backlog_and_slices_with_detached_arc() {
        let shared = Shared::new(Arc::new(|| {}), 4);
        let detached = shared.clone();
        lock(&shared.frames).push_back(present::tests::decoded(1));
        lock(&shared.pacer_backlog).push_back(present::Pending {
            decoded: present::tests::decoded(2),
            present_us: 2,
            hold_us: 0,
            late: false,
            uploaded: false,
        });
        // Unix (Linux and macOS): gpu_planes::Pin is a unit struct without a live D3D11 device.
        #[cfg(unix)]
        lock(&shared.slices).push_back(pyrowave::SliceReady {
            seq: 1,
            band: pyrowave::BandReady { slice: 0, first_row: 0, end_row: 64, fence_value: 1, ready_us: 1 },
            pin: gpu_planes::Pin,
        });

        clear_shared_pictures(&shared);
        drop(shared);

        assert!(lock(&detached.frames).is_empty());
        assert!(lock(&detached.pacer_backlog).is_empty());
        assert!(lock(&detached.slices).is_empty());
    }
}
