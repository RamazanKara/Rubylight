//! What egui draws over the picture: the statistics overlay, the toasts and the stream menu.

use super::*;

/// The width of a list in the stream menu's rows.
const MENU_LIST: f32 = 190.0;

/// The overlay and toasts, drawn with egui over the picture.
/// Returns whether the user's shortcuts changed (saved or deleted in the stream menu) and want writing.
/// Whether a stream with `settings` goes on video planes, drawing straight to the display
/// (`kms_scanout::planned`).
#[cfg(target_os = "linux")]
pub(crate) fn kms_planned(settings: &config::Settings, renderer: *mut SDL_Renderer) -> bool {
    stream::kms_scanout::planned(settings, &sdl::renderer_name(renderer)).is_ok()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn kms_planned(_settings: &config::Settings, _renderer: *mut SDL_Renderer) -> bool {
    false
}

/// Whether the display Nightjar draws straight to takes HDR10 (`kms_scanout::display_hdr`).
#[cfg(target_os = "linux")]
pub(crate) fn kms_display_hdr(window: *mut SDL_Window) -> bool {
    match stream::kms_scanout::display_hdr(window) {
        Ok(()) => true,
        Err(why) => {
            eprintln!("nightjar: kms: no HDR on video planes: {why}");
            false
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn kms_display_hdr(_window: *mut SDL_Window) -> bool {
    false
}

pub(crate) fn draw_overlay(gui: &mut Gui, window: *mut SDL_Window, renderer: *mut SDL_Renderer, s: &mut Streaming, pads: &mut Gamepads, shortcuts: &mut shortcuts::Shortcuts) -> bool {
    // Glass holds nothing while a stream plays, and draws no live blur over its video.
    gui.set_over_video(true);
    // A stream started from the command line never drew the launcher, which loads these too.
    theme::fonts_for(&gui.ctx, strings::current());
    let mut shortcuts_changed = false;
    let toast = s.toast.as_ref().map(|(t, _)| t.clone());
    let input_only = s.facts.role == nightjar_core::http::AppRole::InputOnly;
    let small_stream = s.small_stream_input_only();
    let mic_only = s.mic_only();
    if !s.overlay_visible && toast.is_none() && !input_only && !s.menu_visible {
        return false;
    }
    let mode = s.overlay_mode;
    let (anchor, offset) = overlay_anchor(s.overlay_corner);
    let visible = s.overlay_visible;
    let title = format!("{} · {}", s.facts.app_name, s.facts.host_name);
    // Screen readers get the menu and the Remote Input screen, not every frame of the overlay.
    gui.set_readable(s.menu_visible || input_only);
    // The menu and the screens that take input always run egui. The overlay repaints the shapes it
    // drew while what it shows is unchanged: its lines change once a second, not every present.
    let live = s.menu_visible || mic_only || input_only;
    let digest = overlay_digest(s, mode, live, &title, toast.as_deref(), gui.surface(window, renderer));
    if !gui::overlay_must_run(gui.overlay_shown(), digest, live, gui.input_pending(), gui.animating()) {
        gui.overlay_repaint(renderer);
        return false;
    }
    let output = gui.run(window, renderer, |ui| {
        let ctx = ui.ctx().clone();
        if mic_only {
            egui::Area::new(egui::Id::unique("mic-only"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(egui::RichText::new(tr(mic_only_title(s.microphone_muted, s.microphone_unavailable, s.microphone_busy()))).font(theme::semibold(theme::text::TITLE)).color(theme::MOON));
                    ui.label(&s.facts.host_name);
                    ui.horizontal(|ui| {
                        // Unmuting also tries an unavailable microphone again.
                        let mute = if s.microphone_muted { tr("Unmute") } else { tr("Mute") };
                        if ui.button(mute).clicked() {
                            let muted = !s.microphone_muted;
                            s.mute_microphone(muted, pads);
                            s.needs_redraw = true;
                        }
                        if ui.button(tr("Stop")).clicked() {
                            s.exit = Some(Exit::User);
                        }
                    });
                });
        } else if input_only {
            egui::Area::new(egui::Id::unique("input-only"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(egui::RichText::new(tr("Remote Input connected")).font(theme::semibold(theme::text::TITLE)).color(theme::MOON));
                    ui.label(&title);
                    ui.label(tr("Keyboard, mouse and controllers control the PC."));
                    if let Some(keys) = shortcuts::chord_for(Scope::Stream, Action::Disconnect) {
                        ui.label(trf("{} disconnects", &[&keys.text()]));
                    }
                    if small_stream {
                        let mbps = format!("{:.1}", f64::from(nightjar_policy::launch::estimated_kbps()) / 1000.0);
                        ui.label(trf("The PC still streams small video (about {} Mbit/s); it isn’t shown.", &[&mbps]));
                    }
                });
        }
        if s.menu_visible && s.session_page.is_some() {
            let lines = s.connection_lines();
            let hints = s.session_hints();
            let mut outcome = session_ui::Outcome::Stay;
            if let Some(page) = s.session_page.as_mut() {
                menu_style::show(&ctx, tr("Session and graphics"), "session-and-graphics", |ui| menu_style::body(ui, |ui| {
                        gui::follow_focus(ui, &mut s.menu_focus);
                        menu_style::header(ui, tr("Session and graphics"), &title);
                        outcome = session_ui::show(ui, page, &lines, hints);
                    }));
            }
            match outcome {
                session_ui::Outcome::Stay => {}
                session_ui::Outcome::Resume => s.toggle_menu(window, pads),
                session_ui::Outcome::HostMenu => s.open_host_menu(window, pads),
                session_ui::Outcome::Back => s.close_session_page(),
                session_ui::Outcome::Disconnect | session_ui::Outcome::Leave => s.exit = Some(Exit::User),
            }
        }
        if s.menu_visible && s.session_page.is_none() && s.shortcut_sheet.is_some() {
            let from_menu = s.shortcut_sheet == Some(stream::Sheet::FromMenu);
            let style = glyphs::Style::of(pads);
            let offered = s.controller_mouse_offered(pads);
            let mut back = false;
            menu_style::show(&ctx, tr("Shortcuts & help"), "stream-controls", |ui| menu_style::body(ui, |ui| {
                    gui::follow_focus(ui, &mut s.menu_focus);
                    menu_style::header(ui, tr("Shortcuts & help"), &title);
                    back = menu_style::sheet(ui, style, pads.quit_combo(), pads.back_opens_menu(), offered, from_menu);
                }));
            if back {
                s.close_shortcut_sheet(window, pads);
            }
        } else if s.menu_visible && s.session_page.is_none() {
            let available = stream_menu::Available {
                mouse: s.controller_mouse_offered(pads).then(|| pads.mouse_active()),
                overlay: s.overlay_visible,
                shortcuts: s.facts.role != nightjar_core::http::AppRole::RemoteMonitor,
                microphone: s.microphone_available().then_some(!s.microphone_muted),
                quality: s.facts.runtime_bitrate || s.rate_change.is_some() || s.menu_controls().size_editor
                    || s.facts.role != nightjar_core::http::AppRole::InputOnly,
                clipboard: s.facts.clipboard,
                image_clipboard: s.facts.image_clipboard,
                commands: !s.facts.server_commands.is_empty(),
                session: s.session_page_available(),
                host_audio: !s.host_audio.switches().is_empty() || s.host_audio.problem().is_some(),
                controller_audio: s.controller_audio_available(),
                usb: s.usb.is_some() || s.usb_refused.is_some(),
                guests: s.guests.shown(),
                exits: s.game_exits(),
            };
            let layout = stream_menu::layout(&available, shortcuts.data.len(), pads.menu_buttons());
            let mut page = std::mem::take(&mut s.menu_page);
            let mut next = None;
            let grid_page = page == stream_menu::Page::Grid;
            let (heading, line) = if grid_page { s.menu_header() } else { (page.title().to_owned(), title.clone()) };
            let button = if grid_page { s.menu_hotkey.and_then(|key| stream_menu::button_leave(&layout, key)) } else { None };
            let mut leave = None;
            menu_style::show(&ctx, tr("Stream menu"), "stream-controls", |ui| {
                // On the grid the head and the ways out stay in view; only the tiles scroll.
                if grid_page && menu_style::head(ui, &heading, &line) {
                    leave = Some(stream_menu::Leave::Close);
                }
                let reserve = if grid_page { menu_style::pinned_room(&ctx, "stream-menu-footer") } else { 0.0 };
                menu_style::body_above(ui, reserve, |ui| {
                    use glyphs::Icon;
                    gui::follow_focus(ui, &mut s.menu_focus);
                    if !grid_page {
                        menu_style::header(ui, &heading, &line);
                    }
                    // Nothing focused: the menu just opened or a page changed. The first tile takes
                    // the focus, so A right after opening never leaves the stream.
                    let focus_first = ctx.memory(|m| m.focused().is_none());
                    match &mut page {
                        stream_menu::Page::Grid => {
                            match stream_menu::grid(ui, &layout, shortcuts, s.menu_hotkey.take(), focus_first) {
                                Some(stream_menu::Chosen::Tile(id)) => match id {
                                    "controller_mouse" => s.toggle_controller_mouse(pads),
                                    "overlay" => s.overlay_visible = !s.overlay_visible,
                                    "shortcuts" => next = Some(stream_menu::Page::Shortcuts),
                                    "microphone" => s.mute_microphone(!s.microphone_muted, pads),
                                    "bitrate" => next = Some(stream_menu::Page::Quality),
                                    "clipboard_send" => s.clipboard(true),
                                    "clipboard_receive" => s.clipboard(false),
                                    "server_commands" => next = Some(stream_menu::Page::Commands),
                                    "native_page" => s.session_page_requested = true,
                                    _ => next = Some(stream_menu::Page::More),
                                },
                                Some(stream_menu::Chosen::Shortcut(index)) => {
                                    let shortcut = shortcuts.data[index].clone();
                                    s.toggle_menu(window, pads);
                                    s.send_shortcut(&shortcut);
                                }
                                Some(stream_menu::Chosen::Add) => next = Some(stream_menu::Page::Editor(stream_menu::Draft::default())),
                                None => {}
                            }
                        }
                        stream_menu::Page::Confirm(exit) => match stream_menu::confirm(ui, *exit, focus_first) {
                            Some(true) => {
                                s.end_on_host(*exit);
                                next = Some(stream_menu::Page::Grid);
                            }
                            // Cancel goes back to where the question was asked from.
                            Some(false) => next = Some(stream_menu::back_from_confirm(*exit)),
                            None => {}
                        },
                        stream_menu::Page::ConfirmDisconnect => match stream_menu::confirm_disconnect(ui, focus_first) {
                            Some(true) => s.exit = Some(Exit::User),
                            Some(false) => next = Some(stream_menu::Page::Grid),
                            None => {}
                        },
                        stream_menu::Page::Shortcuts => match stream_menu::shortcuts_page(ui, shortcuts, focus_first) {
                            Some(stream_menu::ShortcutChoice::Send(shortcut)) => {
                                s.toggle_menu(window, pads);
                                s.send_shortcut(&shortcut);
                            }
                            Some(stream_menu::ShortcutChoice::Edit(index)) => {
                                next = Some(stream_menu::Page::Editor(stream_menu::Draft::edit(&shortcuts.data[index])));
                            }
                            Some(stream_menu::ShortcutChoice::Add) => next = Some(stream_menu::Page::Editor(stream_menu::Draft::default())),
                            None => {}
                        },
                        stream_menu::Page::Editor(draft) => match stream_menu::editor(ui, draft, shortcuts, focus_first) {
                            stream_menu::Edited::Stay => {}
                            stream_menu::Edited::Changed => {
                                shortcuts_changed = true;
                                next = Some(stream_menu::Page::Shortcuts);
                            }
                            stream_menu::Edited::Cancel => next = Some(stream_menu::Page::Shortcuts),
                        },
                        stream_menu::Page::Commands => {
                            for (index, name) in s.facts.server_commands.iter().enumerate() {
                                if !name.is_empty() && menu_style::action(ui, Icon::Terminal, name, None).clicked() {
                                    s.pending_command = Some(index);
                                }
                            }
                            if let Some(index) = s.pending_command {
                                menu_style::text_block(ui, &trf("Run ‘{}’ on the PC?", &[&s.facts.server_commands[index]]));
                                ui.horizontal(|ui| {
                                    if menu_style::button(ui, tr("Run command")).clicked() {
                                        s.pending_command = None;
                                        s.server_command(index);
                                    }
                                    if menu_style::button(ui, tr("Cancel")).clicked() { s.pending_command = None; }
                                });
                            }
                        }
                        stream_menu::Page::Quality => menu_style::group(ui, |ui| {
                            let controls = s.menu_controls();
                            if s.facts.runtime_bitrate {
                                if s.facts.role == nightjar_core::http::AppRole::Stream && !s.client_hint_active() {
                                    let mut enabled = s.adaptive_bitrate;
                                    if ui.add_enabled_ui(!s.bitrate_busy, |ui| menu_style::switch(ui, Icon::Gauge, tr("Lower bitrate when the network is busy"), &mut enabled)).inner.changed() {
                                        s.enable_adaptive_bitrate(enabled);
                                    }
                                }
                                if controls.http_bitrate && stream::menu_extras::bitrate_slider(ui, &mut s.bitrate_kbps, !s.adaptive_bitrate && !s.bitrate_busy) {
                                    s.set_bitrate();
                                }
                            }
                            if let Some(rates) = s.rate_change.as_mut().filter(|_| controls.rate_change) {
                                let busy = rates.menu_busy();
                                let mut apply = stream::menu_extras::bitrate_slider(ui, &mut rates.menu_kbps, !busy);
                                menu_style::fold(ui, "rate-change", Icon::Gauge, tr("Change bitrate / frame rate"), |ui| {
                                    let busy = rates.menu_busy();
                                    ui.add_enabled_ui(!busy, |ui| {
                                        ui.horizontal(|ui| {
                                            theme::step_value(ui, &mut rates.menu_kbps, 500..=800_000, 1000.0, |d| d.speed(100).suffix(" kbps"));
                                            theme::step_value(ui, &mut rates.menu_fps, 10.0..=500.0, 1.0, |d| d.suffix(" fps").max_decimals(3));
                                        });
                                        apply |= menu_style::button(ui, tr("Apply bitrate and frame rate")).clicked();
                                    });
                                    if busy { menu_style::text_block(ui, tr("Waiting for the PC…")); }
                                });
                                if apply { s.change_rates(); }
                            }
                            if s.facts.role != nightjar_core::http::AppRole::InputOnly {
                                menu_style::fold(ui, "stream-health", Icon::Pulse, tr("Why did my stream stutter?"), |ui| {
                                    let (text, offer) = s.health();
                                    menu_style::text_block(ui, &text);
                                    ui.add_space(theme::space::S);
                                    ui.horizontal(|ui| {
                                        if let Some(label) = stream::doctor::offer_label(offer) {
                                            if ui.add_enabled_ui(!s.bitrate_busy, |ui| menu_style::button(ui, label)).inner.clicked() { s.health_trial(offer); }
                                        }
                                        if menu_style::button(ui, tr("Copy report")).clicked() { s.copy_health(); }
                                    });
                                });
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
                                        ui.horizontal(|ui| {
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
                                        let apply = if controls.size_editor_fps { tr("Apply stream size and rate") } else { tr("Apply stream size") };
                                        if menu_style::button(ui, apply).clicked() { s.set_mode(s.requested_mode); }
                                    });
                                });
                            }
                        }),
                        stream_menu::Page::More => menu_style::group(ui, |ui| {
                            // The model's order (nightjar_policy::stream_menu::TILES), one row or group per ID.
                            for id in &layout.more {
                                match *id {
                                    "picture" => {
                                        menu_style::row(ui, Icon::Overlay, tr("Performance overlay"), |ui| {
                                            let shown = if s.overlay_visible { tr(s.overlay_mode.label()) } else { tr("Off") };
                                            Combo::from_id_salt("stream-overlay").width(MENU_LIST).selected_text(shown).show_ui(ui, |ui| {
                                                if ui.selectable_label(!s.overlay_visible, tr("Off")).clicked() { s.overlay_visible = false; }
                                                for mode in config::OverlayMode::ALL {
                                                    if ui.selectable_label(s.overlay_visible && s.overlay_mode == mode, tr(mode.label())).clicked() {
                                                        (s.overlay_visible, s.overlay_mode) = (true, mode);
                                                    }
                                                }
                                            });
                                        });
                                        if s.overlay_visible {
                                            menu_style::row(ui, Icon::Corner, tr("Overlay position"), |ui| {
                                                Combo::from_id_salt("stream-overlay-corner").width(MENU_LIST).selected_text(tr(s.overlay_corner.label())).show_ui(ui, |ui| {
                                                    for corner in config::OverlayCorner::ALL {
                                                        ui.selectable_value(&mut s.overlay_corner, corner, tr(corner.label()));
                                                    }
                                                });
                                            });
                                        }
                                        let current = s.video_scaling();
                                        let (mut scaling, top) = current;
                                        menu_style::row(ui, Icon::Screen, tr("Video size"), |ui| {
                                            Combo::from_id_salt("stream-scaling").width(MENU_LIST).selected_text(tr(scaling.label())).show_ui(ui, |ui| {
                                                use config::VideoScaling;
                                                for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                                                    ui.selectable_value(&mut scaling, mode, tr(mode.label()));
                                                }
                                            });
                                        });
                                        if scaling != current.0 { s.set_video_scaling(scaling, top); }
                                    }
                                    // The PC's night mode and headphone surround, as it answered when the menu opened.
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
                                        let outputs = controller_audio::outputs();
                                        let inputs = controller_audio::inputs();
                                        let mut selections = Vec::new();
                                        let mut input_selections = Vec::new();
                                        for pad in &mut pads.pads {
                                            if !pad.supports_audio() { continue; }
                                            let current = pad.audio.as_ref().map(|a| a.device);
                                            let mut selected = current;
                                            let label = current.and_then(|id| outputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                                            Combo::from_id_salt(("controller-audio", pad.index)).selected_text(trf("Controller {}: {}", &[&(pad.index + 1), &label])).show_ui(ui, |ui| {
                                                ui.selectable_value(&mut selected, None, tr("Off"));
                                                for (id, name) in &outputs { ui.selectable_value(&mut selected, Some(*id), name); }
                                            });
                                            if selected != current { selections.push((pad.index, selected)); }
                                            if let Some(audio) = &mut pad.audio {
                                                let mut input = audio.input;
                                                let label = input.and_then(|id| inputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                                                ui.add_enabled_ui(s.microphone_available(), |ui| {
                                                    Combo::from_id_salt(("controller-microphone", pad.index)).selected_text(trf("Microphone: {}", &[&label])).show_ui(ui, |ui| {
                                                        ui.selectable_value(&mut input, None, tr("Off"));
                                                        for (id, name) in &inputs { ui.selectable_value(&mut input, Some(*id), name); }
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
                                            menu_style::fold(ui, "usb", Icon::Usb, tr("USB devices"), |ui| usb::show(ui, panel));
                                        } else if let Some(why) = s.usb_refused {
                                            // Why there are no devices shows without opening anything.
                                            usb::show_refusal(ui, why);
                                        }
                                    }
                                    "guests" => {
                                        menu_style::fold(ui, "guests", Icon::Controller, tr("Guests"), |ui| s.show_guests(ui));
                                    }
                                    "export_statistics" => if menu_style::action(ui, Icon::Download, tr("Export statistics (last hour)"), None).clicked() { s.export_statistics = true; },
                                    "help" => {
                                        if menu_style::action(ui, Icon::Help, tr("Shortcuts & help"), Some(shortcuts::Action::StreamShortcuts)).clicked() {
                                            s.shortcut_sheet = Some(stream::Sheet::FromMenu);
                                            ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
                                        }
                                    }
                                    "reconnect" => if menu_style::action(ui, Icon::Reconnect, tr("Reconnect"), None).clicked() { s.exit = Some(Exit::Reconnect); },
                                    // Under More, not beside Disconnect: it reads too much like it. Asked first.
                                    "end_host_session" => if menu_style::action(ui, Icon::Power, tr(session_page::HostExit::End.label()), None).clicked() {
                                        next = Some(stream_menu::Page::Confirm(session_page::HostExit::End));
                                    },
                                    other => unreachable!("More has no row for {other}"),
                                }
                            }
                        }),
                    }
                    if !matches!(page, stream_menu::Page::Grid | stream_menu::Page::Confirm(_) | stream_menu::Page::ConfirmDisconnect) {
                        ui.add_space(theme::space::M);
                        if menu_style::action_keys(ui, Icon::Back, tr("Back"), Some(crate::shortcuts::Key::Escape.cap().as_ref())).clicked() {
                            next = Some(stream_menu::Page::Grid);
                        }
                    }
                });
                if grid_page {
                    leave = leave.or(menu_style::pinned(ui, "stream-menu-footer", |ui| stream_menu::footer(ui, &layout)));
                }
                });
            // From a button Disconnect asks first, as the ways out that end the game do.
            let leave = match (leave, button) {
                (None, Some(stream_menu::Leave::Disconnect)) => {
                    next = Some(stream_menu::Page::ConfirmDisconnect);
                    None
                }
                (None, button) => button,
                (clicked, _) => clicked,
            };
            match leave {
                Some(stream_menu::Leave::Close) => s.toggle_menu(window, pads),
                Some(stream_menu::Leave::Disconnect) => s.exit = Some(Exit::User),
                Some(stream_menu::Leave::Exit(exit)) => next = Some(stream_menu::Page::Confirm(exit)),
                None => {}
            }
            if let Some(next) = next {
                // The new page starts with nothing focused, so its first tile or row takes the focus.
                ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
                page = next;
            }
            // Closing from a page (Disconnect, a shortcut) already reset it to the grid.
            if s.menu_visible {
                s.menu_page = page;
            }
        } else {
            s.settle_bitrate_slider(&ctx);
        }
        if visible {
            let lines = s.overlay.lines(mode);
            let compact = (mode == config::OverlayMode::Compact).then_some((&s.overlay.compact, s.overlay.notice.as_ref()));
            egui::Area::new(egui::Id::unique("overlay"))
                .anchor(anchor, offset)
                .show(&ctx, |ui| {
                    // The stream's HUD glass, painted under the text once its size is known; no blur under it.
                    let place = ui.painter().add(egui::Shape::Noop);
                    let frame = egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            if let Some((line, notice)) = compact {
                                ui.horizontal(|ui| {
                                    let [r, g, b] = stream::overlay::health_rgb(line.health);
                                    ui.label(egui::RichText::new(line.health.glyph()).font(theme::semibold(theme::text::LABEL)).color(egui::Color32::from_rgb(r, g, b)));
                                    let text = if line.text.is_empty() { tr("Waiting for video").to_owned() } else { line.text.clone() };
                                    ui.label(egui::RichText::new(text).font(theme::regular(theme::text::LABEL)).color(theme::ON_SURFACE));
                                });
                                if let Some(notice) = notice {
                                    ui.label(egui::RichText::new(notice).font(theme::regular(theme::text::CAPTION)).color(theme::ON_SURFACE_VARIANT));
                                }
                                return;
                            }
                            ui.label(egui::RichText::new(&title).font(theme::semibold(theme::text::LABEL)).color(theme::MOON));
                            ui.add_space(2.0);
                            ui.spacing_mut().interact_size.y = 16.0;
                            egui::Grid::new("stats").num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
                                if lines.is_empty() {
                                    ui.label(egui::RichText::new(tr("Measuring…")).color(theme::ON_SURFACE_VARIANT));
                                    ui.end_row();
                                }
                                for (k, v) in lines {
                                    ui.label(egui::RichText::new(k).font(theme::regular(theme::text::CAPTION)).color(theme::OUTLINE));
                                    ui.label(egui::RichText::new(v).font(theme::regular(theme::text::CAPTION)).color(theme::ON_SURFACE));
                                    ui.end_row();
                                }
                            });
                        });
                    let mut glass = Vec::new();
                    ui::glass::hud_surface(&mut glass, ui.ctx(), frame.response.rect, 12.0);
                    ui.painter().set(place, egui::Shape::Vec(glass));
                });
        }
        if let Some(text) = &toast {
            egui::Area::new(egui::Id::unique("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
                .show(&ctx, |ui| {
                    let place = ui.painter().add(egui::Shape::Noop);
                    let frame = egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(18, 10))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(text).font(theme::regular(theme::text::LABEL)).color(theme::ON_SURFACE));
                        });
                    let mut glass = Vec::new();
                    ui::glass::hud_surface(&mut glass, ui.ctx(), frame.response.rect, 18.0);
                    ui.painter().set(place, egui::Shape::Vec(glass));
                });
        }
    });
    if s.menu_visible {
        gui.platform(window, &output.platform_output);
    }
    gui.overlay_paint(renderer, output, digest);
    shortcuts_changed
}

/// Digests what the overlay pass draws: the stream's overlay state, the title and toast, the
/// language, the size and scale it draws at (`surface`) and whether a live screen is up. Equal
/// digests draw the same shapes, so `gui::overlay_must_run` can repaint them.
fn overlay_digest(s: &Streaming, mode: config::OverlayMode, live: bool, title: &str, toast: Option<&str>, surface: ((i32, i32), f32)) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    live.hash(&mut hasher);
    std::mem::discriminant(&mode).hash(&mut hasher);
    std::mem::discriminant(&s.overlay_corner).hash(&mut hasher);
    s.overlay_visible.hash(&mut hasher);
    strings::current().hash(&mut hasher);
    surface.0.hash(&mut hasher);
    surface.1.to_bits().hash(&mut hasher);
    title.hash(&mut hasher);
    toast.hash(&mut hasher);
    for (key, value) in s.overlay.lines(mode) {
        key.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    s.overlay.compact.text.hash(&mut hasher);
    std::mem::discriminant(&s.overlay.compact.health).hash(&mut hasher);
    s.overlay.notice.hash(&mut hasher);
    hasher.finish()
}

/// Where the overlay sits: its corner, 16 points in from both edges.
pub(crate) fn overlay_anchor(corner: config::OverlayCorner) -> (egui::Align2, egui::Vec2) {
    use config::OverlayCorner;
    match corner {
        OverlayCorner::TopLeft => (egui::Align2::LEFT_TOP, egui::vec2(16.0, 16.0)),
        OverlayCorner::TopRight => (egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 16.0)),
        OverlayCorner::BottomLeft => (egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -16.0)),
        OverlayCorner::BottomRight => (egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0)),
    }
}
