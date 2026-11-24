//! Sleep/power confirmations, pairing PIN/OTP/approval, and profile/shortcut dialogs.

use super::*;

impl Launcher {
    pub(super) fn draw_pair_dialog(
        &mut self,
        ctx: &egui::Context,
        view: &View<'_>,
        commands: &mut Vec<Command>,
        dialog: &mut Dialog,
        flow: &mut DialogFlow,
    ) -> bool {
        let focus_first = flow.focus_first;
        let bitrate_kbps = flow.bitrate_kbps;
        let mut close = false;
        let mut next: Option<Dialog> = None;
        let mut focused = false;
        match dialog {
            Dialog::ConfirmSleep { host, ask, name, offer } => {
                let (id, ask) = (*host, *ask);
                modal(ctx, "confirm-sleep", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("Put {} to sleep?", &[&name])).font(semibold(text::SECTION)));
                    ui.label(
                        RichText::new(tr("The PC sleeps only while no app or stream runs on it. Wake it with Wake up."))
                            .color(ON_SURFACE_VARIANT),
                    );
                    // The asking line sits left of the buttons, and Sleep PC is there from the
                    // start, only off: nothing moves under the pointer or the focus, so a press
                    // meant for Cancel never lands on Sleep PC, and no empty row waits above them.
                    let waiting = offer.is_none();
                    dialog_buttons(ui, |ui| {
                        let sleep = ui.add_enabled_ui(!waiting, |ui| primary_button(ui, "Sleep PC")).inner;
                        if let (true, Some((token, at))) = (sleep.clicked(), offer) {
                            commands.push(Command::Sleep { host: id, ask, token: token.clone(), at: *at });
                            close = true;
                        }
                        // Cancel has the focus: putting the PC to sleep is never one stray press away.
                        let cancel = quiet_button(ui, "Cancel");
                        if focus_first {
                            cancel.request_focus();
                            focused = true;
                        }
                        if cancel.clicked() {
                            close = true;
                        }
                        asking_line(ui, waiting);
                    });
                });
            }
            Dialog::ConfirmPcPower { host, ask, name, action, offer } => {
                let (id, ask, action) = (*host, *ask, *action);
                modal(ctx, "confirm-pc-power", |ui| {
                    ui.set_width(440.0);
                    let title = match action {
                        PcPower::Restart => trf("Restart {}?", &[&name]),
                        PcPower::ShutDown => trf("Shut down {}?", &[&name]),
                    };
                    ui.label(RichText::new(title).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("Apps with unsaved work on the PC can block this.")).color(ON_SURFACE_VARIANT));
                    // As Sleep PC: the asking line sits left of the buttons and the action button is
                    // there from the start, only off, so nothing moves under the pointer or the
                    // focus. Shut down keeps a line's room for the PC saying it cannot wake again.
                    let waiting = offer.is_none();
                    let no_wake = action == PcPower::ShutDown && matches!(offer, Some((_, _, false)));
                    if action == PcPower::ShutDown {
                        ui.add_space(space::M);
                        ui.add_visible(no_wake, egui::Label::new(RichText::new(tr("Once off, this PC may only start with its power button.")).color(ERROR)));
                    }
                    dialog_buttons(ui, |ui| {
                        let label = match action {
                            PcPower::Restart => en("Restart"),
                            PcPower::ShutDown => en("Shut down"),
                        };
                        let go = ui.add_enabled_ui(!waiting, |ui| primary_button(ui, label)).inner;
                        if let (true, Some((token, at, _))) = (go.clicked(), offer) {
                            commands.push(Command::PcPower { host: id, ask, action, token: token.clone(), at: *at });
                            close = true;
                        }
                        // Cancel has the focus: the PC never goes down from one stray press.
                        let cancel = quiet_button(ui, "Cancel");
                        if focus_first {
                            cancel.request_focus();
                            focused = true;
                        }
                        if cancel.clicked() {
                            close = true;
                        }
                        asking_line(ui, waiting);
                    });
                });
            }
            Dialog::ConfirmUpdate { host, name, version } => {
                let id = *host;
                modal(ctx, "confirm-update", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("Update the streaming app on {} to {}?", &[&name, &version])).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("The streaming app restarts to install it, and a stream from this device ends.")).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, "Update").clicked() {
                            commands.push(Command::InstallUpdate { host: id });
                            close = true;
                        }
                        let cancel = quiet_button(ui, "Cancel");
                        if focus_first {
                            cancel.request_focus();
                            focused = true;
                        }
                        if cancel.clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::PairRequests { only } => {
                let only = *only;
                modal(ctx, "pair-requests", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("Pair another device")).font(semibold(text::SECTION)));
                    let waiting: Vec<_> = view.hosts.list.iter().filter(|h| only.is_none_or(|id| h.id == id)).flat_map(|h| h.waiting_to_pair().iter().map(move |r| (h, r))).collect();
                    if waiting.is_empty() {
                        let pc = only.and_then(|id| view.hosts.get(id)).map_or("", |h| h.display_name());
                        ui.label(RichText::new(trf("On the new device, select {} in Nightjar or Moonlight.", &[&pc])).color(ON_SURFACE_VARIANT));
                    }
                    ui.add_space(space::S);
                    for (index, (host, request)) in waiting.iter().enumerate() {
                        // The address beside the name, which is the device's to choose.
                        let mut words = format!("{} · {}", short_device_name(&request.name), request.address);
                        if only.is_none() {
                            words = format!("{words} · {}", host.display_name());
                        }
                        let row = button(ui, Some(Icon::Key), &words, Tone::Plain, Size::CONTROL);
                        if focus_first && index == 0 {
                            row.request_focus();
                            focused = true;
                        }
                        if row.clicked() {
                            next = Some(approve_dialog(host, request));
                        }
                    }
                    dialog_buttons(ui, |ui| {
                        let done = quiet_button(ui, "Close");
                        if focus_first && waiting.is_empty() {
                            done.request_focus();
                            focused = true;
                        }
                        if done.clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::ApprovePairing { host, request, name, address, pc, pin, busy } => {
                let id = *host;
                modal(ctx, "approve-pairing", |ui| {
                    ui.set_width(380.0);
                    two_rows(ui, trf("{} wants to pair", &[&name]), semibold(text::SECTION));
                    // The one part of the request the device can't choose, first in every language.
                    two_rows(ui, trf("From {0}, for {1}", &[&address, &pc]), semibold(text::BODY));
                    ui.add_space(space::S);
                    let digits: Vec<char> = pin.chars().collect();
                    let said = trf("{} of {} digits entered", &[&digits.len(), &PIN_LENGTH]);
                    let short = short_window(ctx);
                    pin_slots(ui, &digits, PIN_LENGTH, false, if short { 42.0 } else { 76.0 }, &said);
                    if !short {
                        ui.add_space(space::S);
                    }
                    // Once sent the PIN is out of its hands: the pad gives way to the wait.
                    let mut filled = false;
                    if *busy {
                        ui.add_space(space::M);
                        ui.vertical_centered(|ui| ui.add(egui::Label::new(RichText::new(trf("Waiting for {}…", &[&name])).color(ON_SURFACE_VARIANT)).truncate()));
                    } else {
                        filled = pin_pad(ui, pin, focus_first, &mut focused);
                    }
                    let whole = valid_pairing_pin(pin);
                    dialog_buttons(ui, |ui| {
                        let approve = ui.add_enabled_ui(whole && !*busy, |ui| primary_button(ui, "Approve")).inner;
                        if filled {
                            approve.request_focus();
                        }
                        if approve.clicked() {
                            *busy = true;
                            commands.push(Command::ApprovePairing { host: id, request: request.clone(), name: name.clone(), pin: pin.clone() });
                        }
                        if ui.add_enabled_ui(!*busy, |ui| quiet_button(ui, "Deny")).inner.clicked() {
                            *busy = true;
                            commands.push(Command::DenyPairing { host: id, request: request.clone(), name: name.clone() });
                        }
                        if ui.add_enabled_ui(!*busy, |ui| quiet_button(ui, "Cancel")).inner.clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::PairingHelp => {
                modal(ctx, "pairing-help", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("Connect to your PC")).font(semibold(text::SECTION)));
                    for step in PAIRING_STEPS {
                        ui.label(RichText::new(tr(step)).color(ON_SURFACE_VARIANT));
                    }
                    dialog_buttons(ui, |ui| {
                        let done = primary_button(ui, "Got it");
                        if focus_first {
                            done.request_focus();
                            focused = true;
                        }
                        if done.clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::PairError { host, body, one_time } => {
                let (id, one_time) = (*host, *one_time);
                let name = view.hosts.get(id).map_or(tr("the PC"), |h| h.display_name()).to_owned();
                modal(ctx, "pair-error", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("Pairing with {} failed", &[&name])).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr(body.as_str())).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        let retry = primary_button(ui, "Try again");
                        if focus_first {
                            retry.request_focus();
                            focused = true;
                        }
                        if retry.clicked() {
                            if one_time {
                                next = Some(Dialog::Otp {
                                    host: id,
                                    pin: String::new(),
                                    passphrase: String::new(),
                                    error: None,
                                });
                            } else {
                                commands.push(Command::Pair(id));
                            }
                            close = true;
                        }
                        if quiet_button(ui, "Close").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::NetworkTest { stage, calibrate } => {
                let title = if *calibrate { tr("Network test") } else { tr("Connection test") };
                modal(ctx, "network-test", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(title).font(semibold(text::SECTION)));
                    ui.horizontal(|ui| { spinner(ui, 10.0); ui.label(tr(stage.as_str())); });
                    dialog_buttons(ui, |ui| {
                        let button = quiet_button(ui, "Cancel");
                        if focus_first { button.request_focus(); focused = true; }
                        if button.clicked() { commands.push(Command::CancelNetworkTest); close = true; }
                    });
                });
            }
            // What it means for streaming and the next step; Android's numbers (the delay line, then
            // the speed or why it is missing) under Details. OK, and Try again to run it once more.
            Dialog::ConnectionTest { host, result } => {
                let host = *host;
                let (delay, speed) = (result.delay_line(), result.speed_line());
                let verdict = result.verdict(bitrate_kbps);
                modal(ctx, "connection-test", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("Connection test")).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr(verdict.title)).font(semibold(text::BODY)).color(ON_SURFACE));
                    if let Some(step) = verdict.step {
                        ui.label(RichText::new(tr(step)).color(ON_SURFACE_VARIANT));
                    }
                    let open_id = Id::unique("connection-test-details");
                    let open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
                    if link_button(ui, "Details").clicked() {
                        ui.data_mut(|d| d.insert_temp(open_id, !open));
                    }
                    if open {
                        ui.label(RichText::new(delay).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
                        if let Some(speed) = speed {
                            ui.label(RichText::new(speed).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
                        }
                    }
                    dialog_buttons(ui, |ui| {
                        let ok = primary_button(ui, "OK");
                        if focus_first {
                            ok.request_focus();
                            focused = true;
                        }
                        if ok.clicked() {
                            close = true;
                        }
                        if quiet_button(ui, "Try again").clicked() {
                            commands.push(Command::NetworkTest { host, calibrate: false });
                            close = true;
                        }
                    });
                });
            }
            Dialog::Shortcut { id, name, chord, error } => {
                modal(ctx, "shortcut-editor", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(if id.is_some() { tr("Edit shortcut") } else { tr("New shortcut") }).font(semibold(text::SECTION)));
                    ui.add_space(space::S);
                    let edit = dialog_field(ui, egui::TextEdit::singleline(name).hint_text(hint(tr("Shortcut name"), regular(text::BODY))));
                    if focus_first { edit.request_focus(); focused = true; }
                    dialog_field(ui, egui::TextEdit::singleline(chord).hint_text(hint("CTRL + ALT + DELETE", regular(text::BODY))));
                    ui.label(RichText::new(tr("Use letters, digits, F1–F24, CTRL, SHIFT, ALT, WIN, TAB, ENTER, ESC, arrows or other key names.")).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
                    if let Some(error) = error { ui.label(RichText::new(error.as_str()).color(ERROR)); }
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, "Save").clicked() {
                            match crate::shortcuts::save(&mut self.shortcuts, id.as_deref(), name, chord) {
                                Ok(()) => { commands.push(Command::SaveShortcuts); close = true; }
                                Err(message) => *error = Some(message),
                            }
                        }
                        if quiet_button(ui, "Cancel").clicked() { close = true; }
                    });
                });
            }
            Dialog::StreamHotkey { id, error } => {
                let id = *id;
                // The keys come from SDL before egui ([`Launcher::hotkey_key_down`]).
                let title = crate::shortcuts::Action::from_hotkey(id).map_or(id, |action| tr(action.label()));
                modal(ctx, "stream-hotkey", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(title).font(semibold(text::SECTION)));
                    ui.add_space(space::S);
                    ui.label(RichText::new(tr("Press the new keys")).color(ON_SURFACE_VARIANT));
                    if let Some(error) = error { ui.label(RichText::new(error.as_str()).color(ERROR)); }
                    // Nothing takes focus: Enter or Space on a focused button would be recorded and click it.
                    dialog_buttons(ui, |ui| {
                        if quiet_button(ui, "Cancel").clicked() { close = true; }
                    });
                });
            }
            Dialog::DeleteShortcut { id, name } => {
                modal(ctx, "delete-shortcut", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(trf("Delete {}?", &[&name])).font(semibold(text::SECTION)));
                    dialog_buttons(ui, |ui| {
                        if danger_button(ui, "Delete").clicked() {
                            self.shortcuts.data.retain(|entry| entry.id != *id);
                            commands.push(Command::SaveShortcuts);
                            close = true;
                        }
                        let button = quiet_button(ui, "Cancel");
                        if focus_first { button.request_focus(); focused = true; }
                        if button.clicked() { close = true; }
                    });
                });
            }
            Dialog::ImportShortcuts(entries) => {
                modal(ctx, "import-shortcuts", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(tr("Import keyboard shortcuts?")).font(semibold(text::SECTION)));
                    ui.label(RichText::new(trf("Replace your {} saved shortcuts with {} from this file.", &[&self.shortcuts.data.len(), &entries.data.len()])).color(ON_SURFACE_VARIANT));
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for entry in &entries.data { ui.label(format!("{} · {}", entry.name, entry.keys.join(" + "))); }
                    });
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, "Import").clicked() {
                            self.shortcuts = entries.clone();
                            commands.push(Command::SaveShortcuts);
                            close = true;
                        }
                        let cancel = quiet_button(ui, "Cancel");
                        if focus_first { cancel.request_focus(); focused = true; }
                        if cancel.clicked() { close = true; }
                    });
                });
            }
            Dialog::ProfileName { id, name, settings, error } => {
                modal(ctx, "profile-name", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(if id.is_some() { tr("Rename profile") } else { tr("New profile") }).font(semibold(text::SECTION)));
                    ui.add_space(space::S);
                    let edit = dialog_field(ui, egui::TextEdit::singleline(name).hint_text(hint(tr("Profile name"), regular(text::BODY))));
                    if focus_first { edit.request_focus(); focused = true; }
                    if let Some(error) = error { ui.label(RichText::new(error.as_str()).color(ERROR)); }
                    let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, "Save").clicked() || submitted {
                            let result = if let Some(id) = *id {
                                self.profiles.check_name(name, Some(id)).map(|()| {
                                    if let Some(profile) = self.profiles.entries.iter_mut().find(|p| p.id == id) {
                                        profile.name = name.trim().to_owned();
                                    }
                                    id
                                })
                            } else {
                                self.profiles.create(name, settings.clone())
                            };
                            match result {
                                Ok(saved) => {
                                    commands.push(Command::SaveProfiles);
                                    if id.is_none() { commands.push(Command::ProfileSettings(saved)); }
                                    close = true;
                                }
                                Err(message) => *error = Some(tr(message).into()),
                            }
                        }
                        if quiet_button(ui, "Cancel").clicked() { close = true; }
                    });
                });
            }
            Dialog::DeleteProfile { id, name } => {
                modal(ctx, "delete-profile", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(trf("Delete {}?", &[&name])).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("If this profile is active, new streams will use main settings instead.")).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        if danger_button(ui, "Delete").clicked() {
                            self.profiles.delete(*id);
                            commands.push(Command::SaveProfiles);
                            close = true;
                        }
                        let button = quiet_button(ui, "Cancel");
                        if focus_first { button.request_focus(); focused = true; }
                        if button.clicked() { close = true; }
                    });
                });
            }
            _ => return false,
        }
        flow.close |= close;
        if next.is_some() { flow.next = next; }
        flow.focused |= focused;
        true
    }
}
