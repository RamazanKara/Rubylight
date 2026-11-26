//! What the session and its threads report while streaming: notices and control events.

use super::*;

impl Streaming {
    /// Handles queued notices from the session threads. Control events that need the
    /// controllers are applied to `pads`.
    /// `away`: the window is minimized or hidden.
    pub fn drain_notices(&mut self, pads: &mut Gamepads, away: bool) {
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        for notice in notices {
            match notice {
                Notice::Terminated(code) => {
                    self.exit.get_or_insert(Exit::Terminated(code));
                }
                Notice::DecoderFallback(text) => self.show_toast(text),
                Notice::EchoCancelOff(text) => self.show_toast(text),
                Notice::PassthroughOpus => self.show_toast(tr("The PC did not offer PCM audio. This stream plays Opus.")),
                Notice::LaunchWarning(warning) => {
                    if let Some(text) = launch_warning_text(&warning) {
                        self.show_toast(text);
                    }
                }
                Notice::DecoderFailed(text) => {
                    self.exit.get_or_insert(Exit::Ended(Some(text)));
                }
                Notice::MicrophoneFailed(text) => {
                    self.microphone = None;
                    self.microphone_muted = true;
                    // Microphone only says so where it says "Microphone on", not as muted.
                    self.microphone_unavailable = self.plan.mic_only;
                    self.show_toast(text);
                }
                Notice::Clipboard(result) => {
                    self.clipboard_busy = false;
                    match result {
                        Ok(Some(text)) => {
                            let text = std::ffi::CString::new(text);
                            if text.as_ref().is_ok_and(|text| unsafe { SDL_SetClipboardText(text.as_ptr()) }) {
                                self.show_toast(tr("PC clipboard copied to this device."));
                            } else {
                                self.show_toast(tr("Could not copy the PC clipboard to this device."));
                            }
                        }
                        Ok(None) => self.show_toast(tr("Clipboard sent to the PC.")),
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::ClipboardImage(result) => {
                    self.clipboard_busy = false;
                    match result {
                        Ok(Some(image)) => match crate::clipboard::write(image) {
                            Ok(()) => self.show_toast(tr("PC image copied to this device.")),
                            Err(error) => self.show_toast(trf("Could not put the PC’s image on this device’s clipboard: {}", &[&error])),
                        },
                        Ok(None) => self.show_toast(tr("Image sent to the PC.")),
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::Control(event) => self.control_event(event, pads),
                Notice::ServerCommand(result) => {
                    self.command_busy = false;
                    match result {
                        Ok(payload) => match self.session.send(ClientMessage::ServerCommand(payload)) {
                            Ok(()) => self.show_toast(tr("Command sent to the PC.")),
                            Err(error) => self.show_toast(trf("Could not send the command: {}", &[&error])),
                        },
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::HostMenu(result) => {
                    if let Err(error) = result {
                        self.show_toast(error);
                    }
                }
                Notice::Guests(answer) => {
                    for toast in self.guests.answered(answer, Instant::now()) {
                        self.show_toast(toast);
                    }
                    if let Some(action) = self.guests.take_pending() {
                        self.guest_action(action);
                    }
                    self.needs_redraw = true;
                }
                Notice::HostAudio(answer) => {
                    if let Err(error) = self.host_audio.answered(answer) {
                        self.show_toast(error);
                    }
                    self.needs_redraw = true;
                }
                Notice::NetworkChanged(hint, after_loss) => {
                    // A session without roaming (off, or a host that does not roam) never roams,
                    // but still says whether its connection carries on.
                    let change = self.session.network_changed(hint);
                    self.network_changed(change);
                    if after_loss { self.network_returned(change, hint.metered, away); }
                }
                Notice::NetworkLost => self.network_lost(away),
                Notice::AdaptiveBitrate(result) => {
                    if let Some(error) = apply_adaptive_bitrate_notice(
                        result,
                        &mut self.bitrate_busy,
                        &mut self.adaptive_bitrate,
                        &mut self.facts.bitrate_kbps,
                        &mut self.bitrate_kbps,
                        &mut self.rate_change,
                        &mut self.bitrate_policy,
                        self.facts.format,
                        self.facts.fps.max(1) as u32,
                        (u32::from(self.requested_mode.width), u32::from(self.requested_mode.height)),
                        self.started.elapsed().as_millis() as u64,
                    ) {
                        self.show_toast(error);
                    }
                }
                Notice::HostAbr(result) => {
                    if let Some(error) = apply_host_abr_notice(
                        result,
                        &mut self.bitrate_busy,
                        &mut self.adaptive_bitrate,
                        &mut self.facts.bitrate_kbps,
                        &mut self.bitrate_kbps,
                        &mut self.host_abr,
                        &mut self.host_abr_start,
                        self.started.elapsed().as_millis() as u64,
                    ) {
                        self.show_toast(error);
                    }
                }
                Notice::HostAbrAction(action) => {
                    if let Some(kbps) = self.host_abr.as_mut().and_then(|abr| abr.answered(action.as_ref())) {
                        if let Some(action) = &action { eprintln!("nightjar: the PC set {kbps} kbps: {}", action.reason); }
                        self.facts.bitrate_kbps = i32::try_from(kbps).unwrap_or(i32::MAX);
                        self.bitrate_kbps = kbps;
                    }
                }
                Notice::Bitrate { requested, result } => {
                    self.bitrate_busy = false;
                    match result {
                        Ok(kbps) => {
                            self.facts.bitrate_kbps = i32::try_from(kbps).unwrap_or(i32::MAX);
                            self.bitrate_kbps = kbps;
                            if let Some(rates) = &mut self.rate_change { rates.bitrate_set(kbps, self.started.elapsed().as_millis() as u64); }
                            if let Some(policy) = &mut self.bitrate_policy {
                                policy.applied(requested, kbps, self.started.elapsed().as_millis() as u64);
                            } else {
                                // A manual change replaces the doctor's trial, and a reconnect keeps it.
                                self.keep_manual_bitrate(kbps);
                                self.doctor.trial.clear();
                                self.show_toast(trf("Bitrate set to {} kbps.", &[&kbps]));
                            }
                        }
                        Err(error) => {
                            self.adaptive_bitrate = false;
                            self.bitrate_policy = None;
                            self.show_toast(error);
                        }
                    }
                }
                Notice::HealthTrial(outcome) => {
                    self.bitrate_busy = false;
                    let (kbps, toast) = doctor::finished(&mut self.doctor.trial, outcome);
                    if let Some(kbps) = kbps.filter(|kbps| *kbps > 0) {
                        self.facts.bitrate_kbps = i32::try_from(kbps).unwrap_or(i32::MAX);
                        self.bitrate_kbps = kbps;
                        if let Some(rates) = &mut self.rate_change { rates.bitrate_set(kbps, self.started.elapsed().as_millis() as u64); }
                    }
                    // Automatic bitrate or quality started meanwhile: it owns the bitrate from here on.
                    if self.automatic_bitrate() { self.doctor.trial.clear(); }
                    self.show_toast(toast);
                }
                Notice::HardwareMetrics { generation, result } => {
                    let answer = hardware_answer(&result);
                    match &result {
                        Err(error) if answer == HardwareAnswer::Unavailable => eprintln!("nightjar: host hardware readings unavailable: {error}"),
                        Err(error) => eprintln!("nightjar: host hardware readings failed: {error}"),
                        Ok(_) => {}
                    }
                    if self.hardware_poll.answered(generation, self.started.elapsed().as_millis() as u64, answer) {
                        self.hardware = result.ok();
                    }
                }
                Notice::Stage(_) | Notice::Reconnecting(_) | Notice::Started => {}
            }
        }
        self.poll_guests();
        // A controller forwarded over USB pauses here and comes back once that stops.
        if pads.arbitration_due() {
            let failed = if self.sends_controllers() { self.session.input(|q| pads.arbitrate(Some(q))) } else { pads.arbitrate(None) };
            for why in failed { self.show_toast(why.message()); }
        }
        self.route_microphones(pads);
        *lock(&self.shared.controller_audio) = pads.pads.iter().filter_map(|p| p.audio.as_ref().map(|audio| Arc::downgrade(&audio.playback))).collect();
        self.controller_mouse_active = false;
        if self.settings.controllers && self.facts.role != AppRole::RemoteMonitor && !self.menu_visible && (self.focused || self.settings.background_controllers) {
            // Only while a controller drives the mouse: a pad's mouse sends nothing otherwise, and
            // each input lock wakes the control thread.
            if pads.mouse_active() {
                self.session.input(|q| self.controller_mouse_active = pads.update_mouse(q));
            }
            // Only while a Guide press is pending: otherwise no input lock or control-thread wake per loop.
            if pads.guide_waiting() {
                let now = Instant::now();
                self.session.input(|q| self.guide_menu |= pads.guide_tick(now, q));
            }
            self.guide_waiting = pads.guide_waiting();
        } else {
            self.guide_waiting = false;
        }
        let mut status = [0u8; 16];
        if self.controller_audio_available() {
            for pad in &pads.pads {
                if let (Some(slot), Some(audio)) = (pad.slot(), &pad.audio) {
                    status[usize::from(slot)] = audio.status(self.settings.microphone, self.microphone_muted);
                } else if let Some(slot) = pad.slot().filter(|_| pad.plays_pcm()) {
                    // Bluetooth: playback, no microphone.
                    status[usize::from(slot)] = 1;
                }
            }
        }
        if status != self.audio_status.0 || self.audio_status.1.elapsed() >= Duration::from_secs(1) {
            for (index, &flags) in status.iter().enumerate() {
                if flags != 0 || self.audio_status.0[index] != 0 {
                    let _ = self.session.send(ClientMessage::ControllerAudioStatus { controller: index as u16, flags });
                }
            }
            self.audio_status = (status, Instant::now());
        }
    }

    pub(super) fn control_event(&mut self, event: ControlEvent, pads: &mut Gamepads) {
        match event {
            ControlEvent::ControllerHaptics(pcm) if self.controller_audio_available() => pads.pcm(pcm, false),
            ControlEvent::ControllerSpeaker(pcm) if self.controller_audio_available() => pads.pcm(pcm, true),
            ControlEvent::ControllerAudioControl(payload) if self.controller_audio_available() => {
                pads.audio_control(&payload);
                if pads.pads.iter().any(|p| p.audio.as_ref().is_some_and(|a| a.host_muted)) {
                    self.microphone_muted = true;
                    self.microphone = None;
                }
            }
            ControlEvent::Rumble {
                controller,
                low_frequency,
                high_frequency,
            } => pads.rumble(controller, low_frequency, high_frequency),
            ControlEvent::RumbleTriggers { controller, left, right } => {
                pads.rumble_triggers(controller, left, right)
            }
            ControlEvent::SetControllerLed { controller, r, g, b } => pads.set_led(controller, r, g, b),
            ControlEvent::AdaptiveTriggers { controller, event_flags, type_left, type_right, left, right } => {
                pads.adaptive_triggers(controller, event_flags, type_left, type_right, &left, &right);
            }
            ControlEvent::SetMotionEventState {
                controller,
                motion_type,
                report_rate_hz,
            } => pads.set_motion(controller, motion_type, report_rate_hz),
            ControlEvent::HostTiming(records) => {
                if let Some(csv) = &self.timing_csv {
                    csv.reports(&records);
                }
                for (r, _) in records {
                    self.window.timing_host_us += u64::from(r.host_us);
                    self.window.timing_encode_us += u64::from(r.encode_us);
                    self.window.timing_n += 1;
                    self.doctor.host_sent(r.frame, r.host_us, r.send_us);
                    self.timings.push_back((r.frame, u64::from(r.host_us)));
                    if self.timings.len() > 64 {
                        self.timings.pop_front();
                    }
                }
            }
            ControlEvent::LinkNotice(notice) => {
                // A cut before the PC's answer to the bitrate query: that answer may hold it.
                if notice.state != LinkState::Full && let Some(rates) = &mut self.rate_change { rates.pc_lowered(); }
                if let Some(toast) = super::stats::link_notice_toast(&notice, self.link_lowered_told, self.overlay_visible, self.settings.lower_bitrate.keeps(), self.bitrate_kept()) {
                    self.link_lowered_told = true;
                    self.show_toast(toast);
                }
                self.link_notice = (notice.state != LinkState::Full).then_some(notice);
            }
            ControlEvent::PipelineRates(report) => self.pipeline_rates = Some(report),
            ControlEvent::RateChange(result) => self.rate_change_result(&result),
            ControlEvent::ClockSync { reply, received } => {
                let receive_us = session::now_us().saturating_sub(received.elapsed().as_micros() as u64);
                self.offset.add(&reply, receive_us);
            }
            ControlEvent::HdrMode { enabled, metadata } => {
                self.texture.set_hdr_mode(enabled, metadata);
                self.needs_redraw = true;
            }
            ControlEvent::CursorShape(shape) => {
                if let Err(error) = self.cursor.apply(shape) {
                    self.cursor_error(error);
                }
            }
            ControlEvent::CursorShapeRejected(reason) if self.cursor.enabled => {
                self.cursor.default_cursor();
                self.cursor_error(reason.to_owned());
            }
            // Steam Controller haptics and settings stay unhandled: no arrival advertises them.
            ControlEvent::SteamRejected { message_type, error } => {
                eprintln!(
                    "nightjar: dropping Steam Controller message {message_type:#06x}: {error:?}"
                );
            }
            ControlEvent::Roam(event) => eprintln!("nightjar: {event}"),
            ControlEvent::TextInput(notice) => self.text_field.notice(&notice),
            ControlEvent::ClipboardSync(frame) => {
                if let Some(step) = self.clipboard_sync.as_mut().map(|sync| sync.received(frame, Instant::now())) {
                    self.clipboard_sync_step(step);
                }
            }
            ControlEvent::ClipboardSyncRejected(bytes) => {
                if let Some(step) = self.clipboard_sync.as_mut().map(|sync| sync.rejected(bytes)) {
                    self.clipboard_sync_step(step);
                }
            }
            ControlEvent::Usb(message) => match &self.usb {
                Some(usb) => usb.on_message(message),
                None => eprintln!("nightjar: USB: ignored {message:?}: USB forwarding is off here"),
            },
            ControlEvent::FrameWarpModel(model) => {
                if let Some(line) = self.frame_warp.as_mut().and_then(|presenter| presenter.model(&model)) {
                    eprintln!("{line}");
                }
            }
            _ => {}
        }
    }

    fn cursor_error(&mut self, error: String) {
        eprintln!("nightjar: cursor: {error}");
        if !self.cursor_warning {
            self.cursor_warning = true;
            self.show_toast(tr("The PC’s cursor could not be drawn; using the default pointer."));
        }
    }
}

/// Applies a completed `Notice::AdaptiveBitrate` result. Always clears `bitrate_busy`; if adaptive
/// bitrate was disabled while the request was in flight, ignores the stale result.
#[allow(clippy::too_many_arguments)]
fn apply_adaptive_bitrate_notice(
    result: Result<(u32, u32), String>,
    bitrate_busy: &mut bool,
    adaptive_bitrate: &mut bool,
    facts_bitrate_kbps: &mut i32,
    bitrate_kbps: &mut u32,
    rate_change: &mut Option<rate_change::RateChange>,
    bitrate_policy: &mut Option<bitrate::Bitrate>,
    format: u32,
    fps: u32,
    mode: (u32, u32),
    now_ms: u64,
) -> Option<String> {
    *bitrate_busy = false;
    if !*adaptive_bitrate {
        return None;
    }
    match result {
        Ok((ceiling, current)) => {
            *facts_bitrate_kbps = i32::try_from(current).unwrap_or(i32::MAX);
            *bitrate_kbps = current;
            if let Some(rates) = rate_change { rates.bitrate_set(current, now_ms); }
            *bitrate_policy = Some(bitrate::Bitrate::new(format, ceiling, current, fps, mode, now_ms));
            None
        }
        Err(error) => {
            *adaptive_bitrate = false;
            Some(error)
        }
    }
}

/// Applies a completed `Notice::HostAbr` result. Always clears `bitrate_busy`; if host ABR was
/// cancelled (`host_abr_start` already taken by `stop_host_abr`), ignores the stale result.
#[allow(clippy::too_many_arguments)]
fn apply_host_abr_notice(
    result: Result<Option<nightjar_core::http::foundation::AbrConfigured>, String>,
    bitrate_busy: &mut bool,
    adaptive_bitrate: &mut bool,
    facts_bitrate_kbps: &mut i32,
    bitrate_kbps: &mut u32,
    host_abr: &mut Option<host_abr::HostAbr>,
    host_abr_start: &mut Option<host_abr::AbrStart>,
    now_ms: u64,
) -> Option<String> {
    *bitrate_busy = false;
    if host_abr_start.is_none() {
        return None;
    }
    match result {
        Ok(Some(configured)) => {
            let abr = host_abr::HostAbr::new(&configured, (*facts_bitrate_kbps).max(0) as u32, now_ms);
            eprintln!("nightjar: the PC's automatic bitrate is on: {} kbps, at most {} kbps",
                abr.current_kbps, configured.max_bitrate);
            *facts_bitrate_kbps = i32::try_from(abr.current_kbps).unwrap_or(i32::MAX);
            *bitrate_kbps = abr.current_kbps;
            *host_abr = Some(abr);
            *adaptive_bitrate = true;
            None
        }
        Ok(None) => {
            *host_abr_start = None;
            eprintln!("nightjar: the PC has no automatic bitrate");
            None
        }
        Err(error) => {
            *host_abr_start = None;
            Some(error)
        }
    }
}

/// What a launch warning shows: the PC's own cleaned text, else this app's text for a known code;
/// an unknown code without text shows nothing.
fn launch_warning_text(warning: &nightjar_core::http::LaunchWarning) -> Option<String> {
    use nightjar_core::http::launch_warning::{VIRTUAL_DISPLAY_DISABLED_BY_CLIENT, VIRTUAL_DISPLAY_UNAVAILABLE};
    if let Some(message) = &warning.message {
        return Some(message.clone());
    }
    match warning.code.as_str() {
        VIRTUAL_DISPLAY_UNAVAILABLE => Some(tr("The PC’s virtual display is unavailable, so this stream shows its physical display.").into()),
        VIRTUAL_DISPLAY_DISABLED_BY_CLIENT => Some(tr("This device asked for the PC’s own display, so the stream does not use a virtual display.").into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::foundation::AbrConfigured;

    #[test]
    fn launch_warnings_prefer_the_pcs_text_and_know_two_codes() {
        use nightjar_core::http::LaunchWarning;
        let warning = |code: &str, message: Option<&str>| LaunchWarning { code: code.into(), message: message.map(Into::into) };
        assert_eq!(launch_warning_text(&warning("display_scale", Some("Scale lowered."))).as_deref(), Some("Scale lowered."));
        assert!(launch_warning_text(&warning("virtual_display_unavailable", None)).is_some_and(|t| t.contains("physical display")));
        assert!(launch_warning_text(&warning("virtual_display_disabled_by_client", None)).is_some());
        assert_eq!(launch_warning_text(&warning("future_code", None)), None);
    }

    #[test]
    fn late_adaptive_bitrate_completion_after_disable_is_ignored_and_clears_busy() {
        let mut busy = true;
        let mut adaptive = false;
        let mut facts_kbps = 25_000;
        let mut kbps = 25_000;
        let mut rates = Some(rate_change::RateChange::new(25_000, 60_000, false, 0, 0));
        let mut policy = None;

        // Stale Ok completion after user disabled ABR: busy clears, policy stays None, bitrate unchanged.
        let toast = apply_adaptive_bitrate_notice(
            Ok((25_000, 12_000)),
            &mut busy,
            &mut adaptive,
            &mut facts_kbps,
            &mut kbps,
            &mut rates,
            &mut policy,
            VIDEO_FORMAT_H265,
            60,
            (1920, 1080),
            1_000,
        );
        assert!(toast.is_none());
        assert!(!busy);
        assert!(!adaptive);
        assert!(policy.is_none());
        assert_eq!((facts_kbps, kbps), (25_000, 25_000));

        // Stale Err completion after disable: no spurious error toast.
        busy = true;
        let toast = apply_adaptive_bitrate_notice(
            Err("stale failure".into()),
            &mut busy,
            &mut adaptive,
            &mut facts_kbps,
            &mut kbps,
            &mut rates,
            &mut policy,
            VIDEO_FORMAT_H265,
            60,
            (1920, 1080),
            2_000,
        );
        assert!(toast.is_none());
        assert!(!busy);
    }

    #[test]
    fn late_host_abr_completion_after_disable_is_ignored_while_active_start_succeeds() {
        let configured = AbrConfigured {
            success: true,
            enabled: true,
            initial_bitrate: 15_000,
            max_bitrate: 30_000,
            ..Default::default()
        };
        let mut busy = true;
        let mut adaptive = false;
        let mut facts_kbps = 30_000;
        let mut kbps = 30_000;
        let mut host_abr = None;
        // `stop_host_abr()` already took `host_abr_start` when ABR was disabled.
        let mut host_abr_start = None;

        let toast = apply_host_abr_notice(
            Ok(Some(configured.clone())),
            &mut busy,
            &mut adaptive,
            &mut facts_kbps,
            &mut kbps,
            &mut host_abr,
            &mut host_abr_start,
            1_000,
        );
        assert!(toast.is_none());
        assert!(!busy);
        assert!(!adaptive, "stale HostAbr must not re-enable adaptive_bitrate");
        assert!(host_abr.is_none());
        assert_eq!((facts_kbps, kbps), (30_000, 30_000));

        // Normal (non-cancelled) Foundation start: `host_abr_start` is Some while `adaptive` starts false.
        busy = true;
        host_abr_start = Some(host_abr::AbrStart::default());
        let toast = apply_host_abr_notice(
            Ok(Some(configured)),
            &mut busy,
            &mut adaptive,
            &mut facts_kbps,
            &mut kbps,
            &mut host_abr,
            &mut host_abr_start,
            2_000,
        );
        assert!(toast.is_none());
        assert!(!busy);
        assert!(adaptive);
        assert!(host_abr.is_some());
        assert_eq!((facts_kbps, kbps), (15_000, 15_000));
    }
}
