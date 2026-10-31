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
                Notice::DecoderFailed(text) => {
                    self.exit.get_or_insert(Exit::Ended(Some(text)));
                }
                Notice::MicrophoneFailed(text) => {
                    self.microphone = None;
                    self.microphone_muted = true;
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
                            Err(error) => self.show_toast(trf("Could not put the PC's image on this device's clipboard: {}", &[&error])),
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
                    if after_loss { self.network_returned(change, hint.metered, away); }
                }
                Notice::NetworkLost => self.network_lost(away),
                Notice::AdaptiveBitrate(result) => {
                    self.bitrate_busy = false;
                    match result {
                        Ok((ceiling, current)) => {
                            self.facts.bitrate_kbps = current as i32;
                            self.bitrate_kbps = current;
                            if let Some(rates) = &mut self.rate_change { rates.bitrate_set(current, self.started.elapsed().as_millis() as u64); }
                            self.bitrate_policy = Some(bitrate::Bitrate::new(self.facts.format, ceiling, current, self.facts.fps.max(1) as u32,
                                (u32::from(self.requested_mode.width), u32::from(self.requested_mode.height)),
                                self.started.elapsed().as_millis() as u64));
                        }
                        Err(error) => {
                            self.adaptive_bitrate = false;
                            self.show_toast(error);
                        }
                    }
                }
                Notice::HostAbr(result) => {
                    self.bitrate_busy = false;
                    match result {
                        Ok(Some(configured)) => {
                            let abr = host_abr::HostAbr::new(&configured, self.facts.bitrate_kbps.max(0) as u32,
                                self.started.elapsed().as_millis() as u64);
                            eprintln!("nightjar: the PC's automatic bitrate is on: {} kbps, at most {} kbps",
                                abr.current_kbps, configured.max_bitrate);
                            self.facts.bitrate_kbps = abr.current_kbps as i32;
                            self.bitrate_kbps = abr.current_kbps;
                            self.host_abr = Some(abr);
                            self.adaptive_bitrate = true;
                        }
                        Ok(None) => eprintln!("nightjar: the PC has no automatic bitrate"),
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::HostAbrAction(action) => {
                    if let Some(kbps) = self.host_abr.as_mut().and_then(|abr| abr.answered(action.as_ref())) {
                        if let Some(action) = &action { eprintln!("nightjar: the PC set {kbps} kbps: {}", action.reason); }
                        self.facts.bitrate_kbps = kbps as i32;
                        self.bitrate_kbps = kbps;
                    }
                }
                Notice::Bitrate { requested, result } => {
                    self.bitrate_busy = false;
                    match result {
                        Ok(kbps) => {
                            self.facts.bitrate_kbps = kbps as i32;
                            self.bitrate_kbps = kbps;
                            if let Some(rates) = &mut self.rate_change { rates.bitrate_set(kbps, self.started.elapsed().as_millis() as u64); }
                            if let Some(policy) = &mut self.bitrate_policy {
                                policy.applied(requested, kbps, self.started.elapsed().as_millis() as u64);
                            } else {
                                // A manual change replaces the doctor's trial.
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
                        self.facts.bitrate_kbps = kbps as i32;
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
        self.route_microphones(pads);
        *lock(&self.shared.controller_audio) = pads.pads.iter().filter_map(|p| p.audio.as_ref().map(|audio| Arc::downgrade(&audio.playback))).collect();
        self.controller_mouse_active = false;
        if self.settings.controllers && self.facts.role != AppRole::RemoteMonitor && !self.menu_visible && (self.focused || self.settings.background_controllers) {
            self.session.input(|q| self.controller_mouse_active = pads.update_mouse(q));
        }
        let mut status = [0u8; 16];
        if self.controller_audio_available() {
            for pad in &pads.pads {
                if let Some(audio) = &pad.audio { status[usize::from(pad.index)] = audio.status(self.settings.microphone, self.microphone_muted); }
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
                    self.doctor.host_sent(r.host_us);
                    self.timings.push_back((r.frame, u64::from(r.host_us)));
                    if self.timings.len() > 64 {
                        self.timings.pop_front();
                    }
                }
            }
            ControlEvent::LinkNotice(notice) => {
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
            _ => {}
        }
    }

    fn cursor_error(&mut self, error: String) {
        eprintln!("nightjar: cursor: {error}");
        if !self.cursor_warning {
            self.cursor_warning = true;
            self.show_toast(tr("The PC's cursor could not be drawn; using the default pointer."));
        }
    }
}
