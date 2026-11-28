//! The Control Center's pages over a stream: a module's cards (`stream_menu::cards`), the bitrate
//! in its wide card with the stream's quality rows under it, the rows the 6.x menu kept one level
//! down, and the pages a card opens (the shortcuts, their editor, the PC's commands and the
//! question before a way out). Every action is the one the 6.x stream menu ran; what changed is
//! where it stands.

use nightjar_desktop_ui::ui::control_center_cards as cards;
use nightjar_policy::control_center::{Dock, Module};
use nightjar_policy::stream_menu::{Cell, Hotkey};

use super::*;

/// What a page needs of the app while it draws, and what it asks for.
pub(super) struct Host<'a> {
    pub(super) ctx: &'a egui::Context,
    pub(super) window: *mut SDL_Window,
    pub(super) s: &'a mut Streaming,
    pub(super) pads: &'a mut Gamepads,
    pub(super) shortcuts: &'a mut shortcuts::Shortcuts,
    pub(super) controller_audio: &'a mut overlay::ControllerAudioCache,
    pub(super) session_page_requested: &'a mut bool,
    pub(super) pending_exit: &'a mut Option<session_page::HostExit>,
    /// The page to show from the next frame on.
    pub(super) next: Option<stream_menu::Page>,
    /// The module to turn to.
    pub(super) select: Option<Module>,
    /// A shortcut was saved or deleted.
    pub(super) shortcuts_changed: bool,
    /// The bitrate slider was drawn this frame.
    pub(super) slider_drawn: bool,
    /// The controller audio fold body ran this frame.
    pub(super) audio_fold_drawn: bool,
}

impl Host<'_> {
    /// One frame of what the dock shows: `module`'s page, or the page a card opened.
    pub(super) fn page(&mut self, ui: &mut egui::Ui, dock: &Dock, module: Module, page: &mut stream_menu::Page, pressed: Option<Hotkey>, focus_first: bool) {
        let (window, s, pads) = (self.window, &mut *self.s, &mut *self.pads);
        match page {
            stream_menu::Page::Confirm(exit) => match stream_menu::confirm(ui, *exit, focus_first) {
                Some(true) => {
                    if s.session_page_available() {
                        *self.pending_exit = Some(*exit);
                        *self.session_page_requested = true;
                    } else {
                        s.quit_app_and_leave();
                    }
                    self.next = Some(stream_menu::Page::Grid);
                }
                // Cancel goes back to the page the question was asked from.
                Some(false) => self.next = Some(stream_menu::Page::Grid),
                None => {}
            },
            stream_menu::Page::ConfirmDisconnect => match stream_menu::confirm_disconnect(ui, focus_first) {
                Some(true) => s.exit = Some(Exit::User),
                Some(false) => self.next = Some(stream_menu::Page::Grid),
                None => {}
            },
            stream_menu::Page::Shortcuts => match stream_menu::shortcuts_page(ui, self.shortcuts, focus_first) {
                Some(stream_menu::ShortcutChoice::Send(shortcut)) => {
                    s.toggle_menu(window, pads);
                    s.send_shortcut(&shortcut);
                }
                Some(stream_menu::ShortcutChoice::Edit(index)) => {
                    self.next = Some(stream_menu::Page::Editor(stream_menu::Draft::edit(&self.shortcuts.data[index])));
                }
                Some(stream_menu::ShortcutChoice::Add) => self.next = Some(stream_menu::Page::Editor(stream_menu::Draft::default())),
                None => {}
            },
            stream_menu::Page::Editor(draft) => match stream_menu::editor(ui, draft, self.shortcuts, focus_first) {
                stream_menu::Edited::Stay => {}
                stream_menu::Edited::Changed => {
                    self.shortcuts_changed = true;
                    self.next = Some(stream_menu::Page::Shortcuts);
                }
                stream_menu::Edited::Cancel => self.next = Some(stream_menu::Page::Shortcuts),
            },
            stream_menu::Page::Commands => self.commands(ui, focus_first),
            // A module's page. More, Quality and Guests were pages of the 6.x menu; what they held
            // stands on the modules' pages.
            stream_menu::Page::Grid | stream_menu::Page::More | stream_menu::Page::Quality | stream_menu::Page::Guests => {
                self.module(ui, dock, module, pressed, focus_first);
            }
        }
        if matches!(page, stream_menu::Page::Shortcuts | stream_menu::Page::Editor(_) | stream_menu::Page::Commands) {
            ui.add_space(theme::space::M);
            if menu_style::action_keys(ui, glyphs::Icon::Back, tr("Back"), Some(crate::shortcuts::Key::Escape.cap().as_ref())).clicked() {
                self.next = Some(stream_menu::Page::Grid);
            }
        }
    }

    /// The PC's commands. The question takes the list's place with Cancel focused, as the other
    /// confirmations do.
    fn commands(&mut self, ui: &mut egui::Ui, focus_first: bool) {
        let (ctx, s) = (self.ctx, &mut *self.s);
        match s.pending_command {
            Some(index) => {
                menu_style::text_block(ui, &trf("Run ‘{}’ on the PC?", &[&s.facts.server_commands[index]]));
                ui.horizontal(|ui| {
                    let run = menu_style::button(ui, tr("Run command"));
                    let cancel = menu_style::button(ui, tr("Cancel"));
                    if focus_first { cancel.request_focus(); }
                    if run.clicked() {
                        s.pending_command = None;
                        s.server_command(index);
                    }
                    if cancel.clicked() { s.pending_command = None; }
                });
            }
            None => {
                for (index, name) in s.facts.server_commands.iter().enumerate() {
                    if !name.is_empty() && menu_style::action(ui, glyphs::Icon::Terminal, name, None).clicked() {
                        s.pending_command = Some(index);
                        ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
                    }
                }
            }
        }
    }

    /// A module's page: a notice the launch left (on Quick), its cards, the bitrate's wide card
    /// with the quality rows (where the stream offers them), the PC's encoder load (on Picture)
    /// and the rows.
    fn module(&mut self, ui: &mut egui::Ui, dock: &Dock, module: Module, pressed: Option<Hotkey>, focus_first: bool) {
        // What the launch said still holds: it stays readable at the top of Quick.
        if module == Module::Quick && let Some(notice) = self.s.launch_notice() {
            cards::notice_row(ui, notice);
            ui.add_space(cards::GAP);
        }
        let chosen = stream_menu::cards(ui, dock, module, self.shortcuts, pressed, focus_first);
        self.chosen(chosen);
        let Some(page) = dock.page(module) else { return };
        let slider = page.cards.iter().any(|cell| matches!(cell, Cell::Tile { id, .. } if *id == stream_menu::SLIDER));
        if page.cards.len() > usize::from(slider) {
            ui.add_space(cards::GAP);
        }
        if slider {
            self.quality(ui);
            ui.add_space(cards::GAP);
        }
        // Read-only, and there only while the PC reports it (the statistics overlay asks for it).
        if module == Module::Picture && let Some(load) = self.s.encoder_load() {
            cards::status_row(ui, glyphs::Icon::Pulse, tr("Video encoder"), &load);
        }
        if module == Module::Clips && let Some(entries) = self.s.dvr_menu() {
            let rows = entries.each_ref().map(|e| (e.op, e.label.as_str(), e.detail.as_str(), e.enabled, e.problem));
            stream_menu::dvr::notes(ui, &rows);
        }
        if !page.rows.is_empty() {
            menu_style::group(ui, |ui| {
                for id in &page.rows {
                    self.row(ui, id);
                }
            });
        }
    }

    /// Runs the card that was chosen, as the 6.x grid ran its tile.
    fn chosen(&mut self, chosen: Option<stream_menu::Chosen>) {
        let (window, s, pads) = (self.window, &mut *self.s, &mut *self.pads);
        match chosen {
            Some(stream_menu::Chosen::Tile(id)) => match id {
                "controller_mouse" => s.toggle_controller_mouse(pads),
                "overlay" => s.overlay_visible = !s.overlay_visible,
                "shortcuts" => self.next = Some(stream_menu::Page::Shortcuts),
                "microphone" => {
                    let muted = !s.microphone_muted;
                    s.mute_microphone(muted, pads);
                }
                // Its card is the slider on Picture: a button given to it turns the page there.
                "bitrate" => self.select = Some(Module::Picture),
                "clipboard_send" => s.clipboard(true),
                "clipboard_receive" => s.clipboard(false),
                "server_commands" => self.next = Some(stream_menu::Page::Commands),
                "native_page" => *self.session_page_requested = true,
                "suspend_game" | "resume_game" => s.quick_suspend_tile(id),
                // Save clip, then Start / Stop recording; one the host cannot take now says why below.
                "save_clip" | "record" => {
                    if let Some(entries) = s.dvr_menu() {
                        let entry = &entries[usize::from(id == "record")];
                        if entry.enabled {
                            s.dvr_request(entry.op);
                        }
                    }
                }
                other => eprintln!("nightjar: the Control Center has no action for the card {other}"),
            },
            Some(stream_menu::Chosen::Shortcut(index)) => {
                let shortcut = self.shortcuts.data[index].clone();
                s.toggle_menu(window, pads);
                s.send_shortcut(&shortcut);
            }
            Some(stream_menu::Chosen::Add) => self.next = Some(stream_menu::Page::Editor(stream_menu::Draft::default())),
            None => {}
        }
    }

    /// The bitrate in its wide card, the track inside, and under it what the 6.x Quality page
    /// held: the adaptive bitrate, Keep my bitrate, the rates, the stutter report and the stream's
    /// size.
    fn quality(&mut self, ui: &mut egui::Ui) {
        use glyphs::Icon;
        use nightjar_core::http::AppRole;
        use nightjar_desktop_ui::stream_overlay_widgets::bitrate_slider;
        let window = self.window;
        let s = &mut *self.s;
        let controls = s.menu_controls();
        let adaptive = s.facts.runtime_bitrate && s.facts.role == AppRole::Stream && !s.client_hint_active();
        let http = s.facts.runtime_bitrate && controls.http_bitrate;
        let rate_change = controls.rate_change && s.rate_change.is_some();
        let report = s.facts.role != AppRole::InputOnly;
        let mut apply = false;
        if http || rate_change {
            self.slider_drawn = true;
            cards::wide(ui, Icon::Gauge, tr("Quality"), None, |ui| {
                if http && bitrate_slider(ui, &mut s.bitrate_kbps, !s.adaptive_bitrate && !s.bitrate_busy) {
                    s.set_bitrate();
                }
                if let Some(rates) = s.rate_change.as_mut().filter(|_| rate_change) {
                    // Disabled until the PC said what it counts the stream's bitrate as.
                    let (busy, locked) = (rates.menu_busy(), rates.bitrate_locked());
                    apply |= bitrate_slider(ui, &mut rates.menu_kbps, !busy && !locked);
                }
            });
        }
        if adaptive || rate_change || report || controls.size_editor {
            if http || rate_change {
                ui.add_space(cards::GAP);
            }
            menu_style::group(ui, |ui| {
                if adaptive {
                    let mut enabled = s.adaptive_bitrate;
                    if ui.add_enabled_ui(!s.bitrate_busy, |ui| menu_style::switch(ui, Icon::Gauge, tr("Lower bitrate when the network is busy"), &mut enabled)).inner.changed() {
                        s.enable_adaptive_bitrate(enabled);
                    }
                }
                // Keep my bitrate right under the bitrate, where a player who saw the bitrate fall looks.
                let kept = s.bitrate_kept();
                let mut keep_to = None;
                if let Some(rates) = s.rate_change.as_mut().filter(|_| rate_change) {
                    if let Some(mut keep) = kept
                        && menu_style::switch(ui, Icon::Gauge, tr("Keep my bitrate"), &mut keep).changed() { keep_to = Some(keep); }
                    menu_style::fold(ui, "rate-change", Icon::Gauge, tr("Change bitrate / frame rate"), |ui| {
                        let busy = rates.menu_busy();
                        ui.add_enabled_ui(!busy, |ui| {
                            // The dock is narrow: what does not fit beside the bitrate goes under it.
                            ui.horizontal_wrapped(|ui| {
                                ui.add_enabled_ui(!rates.bitrate_locked(), |ui| {
                                    theme::step_value(ui, &mut rates.menu_kbps, 500..=2_000_000, 1000.0, |d| d.speed(100).suffix(" kbps"));
                                });
                                theme::step_value(ui, &mut rates.menu_fps, 10.0..=500.0, 1.0, |d| d.suffix(" fps").max_decimals(3));
                            });
                            apply |= menu_style::button(ui, tr("Apply bitrate and frame rate")).clicked();
                        });
                        if busy { menu_style::text_block(ui, tr("Waiting for the PC…")); }
                    });
                }
                if let Some(keep) = keep_to { s.set_bitrate_kept(keep); }
                if report {
                    menu_style::fold(ui, "stream-health", Icon::Pulse, tr("Why did my stream stutter?"), |ui| doctor(ui, s, window));
                }
                if controls.size_editor {
                    menu_style::fold(ui, "stream-size", Icon::Screen, tr("Stream size"), |ui| {
                        let mut quality = s.quality_mode;
                        Combo::from_id_salt("stream-quality").selected_text(tr(quality.label())).show_ui(ui, |ui| {
                            for mode in [config::QualityMode::Off, config::QualityMode::Motion, config::QualityMode::Detail, config::QualityMode::Battery] {
                                ui.selectable_value(&mut quality, mode, tr(mode.label()));
                            }
                        });
                        if quality != s.quality_mode { s.set_quality(quality); }
                        ui.add_enabled_ui(s.quality_mode == config::QualityMode::Off, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                theme::step_value(ui, &mut s.requested_mode.width, 256..=8192, 16.0, |d| d.speed(2));
                                ui.label("×");
                                theme::step_value(ui, &mut s.requested_mode.height, 256..=8192, 16.0, |d| d.speed(2));
                                // With 0x553C the rate is set above and the size keeps the current one.
                                if controls.size_editor_fps {
                                    let mut fps = s.requested_mode.fps_millihz as f64 / 1000.0;
                                    if theme::step_value(ui, &mut fps, 10.0..=500.0, 1.0, |d| d.suffix(" fps").max_decimals(3)).changed() {
                                        s.requested_mode.fps_millihz = (fps * 1000.0).round() as u32;
                                    }
                                }
                            });
                            let label = if controls.size_editor_fps { tr("Apply stream size and rate") } else { tr("Apply stream size") };
                            if menu_style::button(ui, label).clicked() { s.set_mode(s.requested_mode); }
                        });
                    });
                }
            });
        }
        if apply { s.change_rates(); }
    }

    /// A row the 6.x menu kept under More, by its ID in the model's order
    /// (`nightjar_policy::stream_menu::TILES`): one row or group for each.
    fn row(&mut self, ui: &mut egui::Ui, id: &str) {
        use glyphs::Icon;
        let (ctx, s, pads, controller_audio, audio_fold_drawn) = (
            self.ctx,
            &mut *self.s,
            &mut *self.pads,
            &mut *self.controller_audio,
            &mut self.audio_fold_drawn,
        );
        match id {
            // A list is too wide to stand beside its label in the dock: it goes under it.
            "picture" => {
                cards::stacked(ui, Icon::Overlay, tr("Performance overlay"), |ui, width| {
                    let shown = if s.overlay_visible { tr(s.overlay_mode.label()) } else { tr("Off") };
                    Combo::from_id_salt("stream-overlay").width(width).selected_text(shown).show_ui(ui, |ui| {
                        if ui.selectable_label(!s.overlay_visible, tr("Off")).clicked() { s.overlay_visible = false; }
                        for mode in config::OverlayMode::ALL {
                            if ui.selectable_label(s.overlay_visible && s.overlay_mode == mode, tr(mode.label())).clicked() {
                                (s.overlay_visible, s.overlay_mode) = (true, mode);
                            }
                        }
                    });
                });
                if s.overlay_visible {
                    cards::stacked(ui, Icon::Corner, tr("Overlay position"), |ui, width| {
                        Combo::from_id_salt("stream-overlay-corner").width(width).selected_text(tr(s.overlay_corner.label())).show_ui(ui, |ui| {
                            for corner in config::OverlayCorner::ALL {
                                ui.selectable_value(&mut s.overlay_corner, corner, tr(corner.label()));
                            }
                        });
                    });
                }
                let current = s.video_scaling();
                let (mut scaling, top) = current;
                cards::stacked(ui, Icon::Screen, tr("Video size"), |ui, width| {
                    Combo::from_id_salt("stream-scaling").width(width).selected_text(tr(scaling.label())).show_ui(ui, |ui| {
                        use config::VideoScaling;
                        for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                            ui.selectable_value(&mut scaling, mode, tr(mode.label()));
                        }
                    });
                });
                if scaling != current.0 { s.set_video_scaling(scaling, top); }
            }
            // The PC's night mode and headphone surround, as it answered when the Control Center opened.
            "night_mode" => {
                for switch in s.host_audio.switches() {
                    let mut on = switch.on;
                    let label = stream::menu_extras::audio_label(switch.id);
                    if ui.add_enabled_ui(switch.enabled, |ui| menu_style::switch(ui, Icon::Speaker, label, &mut on)).inner.changed() { s.set_host_audio(switch.id, on); }
                    if let Some(note) = &switch.note { menu_style::note(ui, note); }
                }
                if let Some(problem) = s.host_audio.problem() { menu_style::note(ui, problem); }
            }
            "image_clipboard_send" => if menu_style::action(ui, Icon::Image, tr("Send image to PC"), None).clicked() { s.clipboard_image(true); },
            "image_clipboard_receive" => if menu_style::action(ui, Icon::Image, tr("Copy PC image here"), None).clicked() { s.clipboard_image(false); },
            "controller_audio" => {
                menu_style::fold(ui, "controller-audio", Icon::Controller, tr("DualSense USB audio and waveform haptics"), |ui| {
                    *audio_fold_drawn = true;
                    let (outputs, inputs) = controller_audio.devices();
                    let mut selections = Vec::new();
                    let mut input_selections = Vec::new();
                    for pad in &mut pads.pads {
                        if !pad.supports_audio() { continue; }
                        let current = pad.audio.as_ref().map(|a| a.device);
                        let mut selected = current;
                        let label = current.and_then(|id| outputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                        Combo::from_id_salt(("controller-audio", pad.index)).selected_text(trf("Controller {}: {}", &[&(pad.index + 1), &label])).show_ui(ui, |ui| {
                            ui.selectable_value(&mut selected, None, tr("Off"));
                            for (id, name) in outputs { ui.selectable_value(&mut selected, Some(*id), name); }
                        });
                        if selected != current { selections.push((pad.index, selected)); }
                        if let Some(audio) = &mut pad.audio {
                            let mut input = audio.input;
                            let label = input.and_then(|id| inputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                            ui.add_enabled_ui(s.microphone_available(), |ui| {
                                Combo::from_id_salt(("controller-microphone", pad.index)).selected_text(trf("Microphone: {}", &[&label])).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut input, None, tr("Off"));
                                    for (id, name) in inputs { ui.selectable_value(&mut input, Some(*id), name); }
                                });
                            });
                            if input != audio.input { input_selections.push((pad.index, input)); }
                            Combo::from_id_salt(("controller-headphones", pad.index))
                                .selected_text(match audio.headphone_override { None => tr("Automatic headset detection"), Some(true) => tr("Headset"), Some(false) => tr("Controller speaker") })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut audio.headphone_override, None, tr("Automatic headset detection"));
                                    ui.selectable_value(&mut audio.headphone_override, Some(true), tr("Headset"));
                                    ui.selectable_value(&mut audio.headphone_override, Some(false), tr("Controller speaker"));
                                });
                            if audio.headphone_override.is_none() && audio.detected_headphones.is_none() {
                                ui.weak(tr("Headset detection unavailable. Select Headset when using the controller jack."));
                            }
                        }
                    }
                    for (index, device) in selections {
                        match pads.set_audio(index, device) {
                            Ok(()) => s.session.input(|q| pads.announce_new(q)),
                            Err(error) => s.show_toast(trf("Controller audio unavailable: {}", &[&error])),
                        }
                    }
                    for (index, device) in input_selections {
                        if let Err(error) = pads.set_audio_input(index, device) { s.show_toast(error); }
                    }
                });
            }
            "usb_devices" => {
                if let Some(panel) = s.usb.as_mut() {
                    // Devices usbipd-win still lends since Nightjar closed show without opening the fold.
                    let (left, giving_back) = panel.usbipd_left();
                    if nightjar_desktop_ui::stream_overlay_widgets::usb_leftover_notice(ui, left, giving_back) {
                        panel.give_back_usbipd();
                    }
                    menu_style::fold(ui, "usb", Icon::Usb, tr("USB devices"), |ui| show_usb_section(ui, panel));
                } else if let Some(why) = s.usb_refused {
                    // Why there are no devices shows without opening anything.
                    nightjar_desktop_ui::stream_overlay_widgets::usb_show_refusal(ui, why);
                }
            }
            // The controller editor is Settings' Controller section, which the launcher shows: the first choice
            // says that the stream ends and the game keeps running, the second leaves for it.
            "controller_settings" => {
                if s.settings_asked {
                    menu_style::note(ui, tr("Opening controller settings leaves the stream; the game keeps running on the PC."));
                    if menu_style::action(ui, Icon::Controller, tr("Leave and open controller settings"), None).clicked() {
                        s.settings_after = Some(strings::en("Controller"));
                        s.exit = Some(Exit::User);
                    }
                } else if menu_style::action(ui, Icon::Controller, tr("Controller settings"), None).clicked() {
                    s.settings_asked = true;
                }
            }
            // The page is theirs alone: who waits to join and the guests show without a fold to open.
            "guests" => show_guests_section(ui, s),
            "export_statistics" => if menu_style::action(ui, Icon::Download, tr("Export statistics (last hour)"), None).clicked() { s.export_statistics = true; },
            "help" => {
                if menu_style::action(ui, Icon::Help, tr("Shortcuts & help"), Some(shortcuts::Action::StreamShortcuts)).clicked() {
                    s.shortcut_sheet = Some(stream::Sheet::FromMenu);
                    ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
                }
            }
            "reconnect" => if menu_style::action(ui, Icon::Reconnect, tr("Reconnect"), None).clicked() { s.exit = Some(Exit::Reconnect); },
            // On Power, not beside Disconnect: it reads too much like it. Asked first.
            "end_host_session" => if menu_style::action(ui, Icon::Power, tr(session_page::HostExit::End.label()), None).clicked() {
                self.next = Some(stream_menu::Page::Confirm(session_page::HostExit::End));
            },
            other => eprintln!("nightjar: the Control Center has no row for {other}"),
        }
    }
}

/// The Stream Doctor card inside its fold: the stream's look for this frame, which reads the PC's
/// doctor while the fold is open (and only then), and what the player picked on it.
fn doctor(ui: &mut egui::Ui, s: &mut Streaming, window: *mut SDL_Window) {
    // Not `Action`: `every_bound_key_has_a_handler` looks for shortcut actions by that name.
    use nightjar_desktop_ui::ui::doctor_card::{self, Action as Asked};
    use stream::doctor_v2::Pick;
    let (mut width, mut height) = (0, 0);
    // SAFETY: the stream's own window, alive while its Control Center draws.
    unsafe { SDL_GetWindowSizeInPixels(window, &mut width, &mut height) };
    let look = s.doctor_look((width.max(0) as u32, height.max(0) as u32));
    let view = doctor_card::View {
        diagnosis: &look.diagnosis,
        host: look.host.as_ref(),
        owner: look.owner,
        upscaler: look.upscaler,
        busy: look.busy,
        undo_bitrate: look.undo_bitrate,
        undo_resolution: look.undo_resolution,
        replaced: look.replaced,
        technical: &look.technical,
    };
    match doctor_card::show(ui, &view) {
        Some(Asked::Apply(remedy)) => s.doctor_pick(Pick::Apply(remedy)),
        Some(Asked::UndoBitrate) => s.doctor_pick(Pick::UndoBitrate),
        Some(Asked::UndoResolution) => s.doctor_pick(Pick::UndoResolution),
        Some(Asked::Copy) => s.copy_report(&doctor_card::report(&view)),
        None => {}
    }
}

fn show_guests_section(ui: &mut egui::Ui, s: &mut Streaming) {
    use nightjar_desktop_ui::stream_overlay_widgets::{show_guests, GuestUiAction, GuestsInviteView};
    let Some(read) = s.guests.read().cloned() else {
        ui.weak(tr("Waiting for the PC…"));
        return;
    };
    let invite_view = match s.guests.invite() {
        stream::guests::Invite::None => GuestsInviteView::None,
        stream::guests::Invite::Making => GuestsInviteView::Making,
        stream::guests::Invite::Made(made) => GuestsInviteView::Made(made),
    };
    for action in show_guests(ui, &read, s.guests.may_invite(), invite_view) {
        match action {
            GuestUiAction::Guest(action) => s.guest_action(action),
            GuestUiAction::CreateInvite => s.create_invite(),
            GuestUiAction::ResetInvite => s.reset_invite(),
            GuestUiAction::Toast(msg) => s.show_toast(tr(msg)),
        }
    }
}

fn show_usb_section(ui: &mut egui::Ui, panel: &mut usb::Panel) {
    use nightjar_desktop_ui::stream_overlay_widgets::{
        usb_show, UsbDeviceView, UsbFailedAttachment, UsbPanelView, UsbUiAction,
    };
    if let Some(failed) = panel.take_opening_failed() {
        panel.error = Some(failed);
    }
    let local = panel.local().to_vec();
    let attachments = panel.attachments();
    let devices = local
        .iter()
        .map(|d| UsbDeviceView {
            busid: d.busid.clone(),
            vid: d.vid,
            pid: d.pid,
            name: d.name.clone(),
            blocked_message: usb::blocked_message(d),
            usbipd: d.usbipd == usb::UsbipdOffer::Ready,
            can_auto: d.blocked.is_none(),
            auto_on: panel.is_auto(d),
            opening: panel.is_opening(d),
            active: panel.active(d).map(|a| (a.request_id, usb::phase_label(&a.phase))),
        })
        .collect();
    let mut shown = std::collections::HashSet::new();
    let mut failed_attachments = Vec::new();
    for attachment in attachments.iter().rev() {
        if attachment.is_active()
            || !usb::phase_failed(&attachment.phase)
            || !shown.insert(attachment.busid.clone())
            || panel.active_busid(&attachment.busid)
        {
            continue;
        }
        failed_attachments.push(UsbFailedAttachment {
            busid: attachment.busid.clone(),
            name: attachment.name.clone(),
            phase_label: usb::phase_label(&attachment.phase),
        });
    }
    let view = UsbPanelView {
        devices,
        failed_attachments,
        error: panel.error.clone(),
        // Above the fold, where it shows without opening it.
        usbipd_left: 0,
        usbipd_giving_back: false,
    };
    for action in usb_show(ui, &view) {
        match action {
            UsbUiAction::Refresh => {
                panel.error = None;
                panel.refresh();
            }
            UsbUiAction::Attach(busid) => panel.attach_busid(&busid),
            UsbUiAction::AttachUsbipd(busid) => panel.attach_usbipd_busid(&busid),
            UsbUiAction::Detach(id) => panel.detach(id),
            UsbUiAction::SetAuto(busid, on) => panel.set_auto_busid(&busid, on),
            UsbUiAction::GiveBackUsbipd => panel.give_back_usbipd(),
        }
    }
}
