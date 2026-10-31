//! Dialogs and overlays: their drawing and buttons, a PC's options list, pairing and sleep
//! answers, and the waking and connecting cards.

use super::*;

/// What the pairing help says, in the order of Nightjar Android's guide.
pub(super) const PAIRING_STEPS: [&str; 3] = [
    en("1. Start a Moonlight-compatible host on the PC, such as Apollo, Rubylight, Sunshine or Vibepollo."),
    en("2. Here, choose the PC on the same network and select Pair."),
    en("3. Enter the PIN shown here in your host's settings or web UI, usually on its PIN or pairing page."),
];

/// What a PC's options list can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostOption {
    OpenLibrary,
    Resume,
    QuitApp,
    Pair,
    PairOtp,
    Wake,
    Sleep,
    Files,
    Settings,
    Details,
    Rename,
    Unpair,
    PairingHelp,
    CheckAgain,
    Remove,
}

impl HostOption {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenLibrary => en("Open library"),
            Self::Resume => en("Resume"),
            Self::QuitApp => en("Quit running app"),
            Self::Pair => en("Pair"),
            Self::PairOtp => en("Pair with one-time PIN"),
            Self::Wake => en("Wake up"),
            Self::Sleep => en("Sleep PC"),
            Self::Files => en("Files"),
            Self::Settings => en("PC settings"),
            Self::Details => en("View details"),
            Self::Rename => en("Rename"),
            Self::Unpair => en("Unpair"),
            Self::PairingHelp => en("Pairing help"),
            Self::CheckAgain => en("Check again"),
            Self::Remove => en("Remove PC"),
        }
    }
}

/// The options a PC offers now, in the order they are listed.
pub fn host_options(host: &Host) -> Vec<HostOption> {
    let mut options = Vec::new();
    if host.paired() {
        options.push(HostOption::OpenLibrary);
        if running_entry(host).is_some() {
            options.push(HostOption::Resume);
        }
        if host.running_app().is_some() {
            options.push(HostOption::QuitApp);
        }
    } else {
        options.push(HostOption::Pair);
        if host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::OtpPairing)) {
            options.push(HostOption::PairOtp);
        }
    }
    // Not reached yet counts too: a PC that is asleep has not answered since this started.
    if host.reach != Reach::Online && host.record.mac.is_some() {
        options.push(HostOption::Wake);
    }
    // Only where the PC offers it; listed but not selectable when it does not allow this device.
    if host.sleep_option().is_some() {
        options.push(HostOption::Sleep);
    }
    if files::offers_files(host) {
        options.push(HostOption::Files);
    }
    options.push(HostOption::Settings);
    options.push(HostOption::Rename);
    options.push(HostOption::Details);
    // Unpairing asks the PC to forget this device, so it needs the PC to be reachable.
    if host.paired() && host.reach == Reach::Online {
        options.push(HostOption::Unpair);
    }
    if !host.paired() {
        options.push(HostOption::PairingHelp);
    }
    options.push(HostOption::CheckAgain);
    options.push(HostOption::Remove);
    options
}

impl Launcher {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, view: &View<'_>, commands: &mut Vec<Command>) {
        let frame = Frame::new()
            .fill(*SURFACE_HIGH)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(28))
            .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
        let mut close = false;
        let mut next: Option<Dialog> = None;
        let focus_first = self.focus_dialog;
        let mut focused = false;
        match &mut self.dialog {
            Dialog::None => {
                if let Some((title, body)) = self.queued.pop_front() {
                    self.message(title, body);
                    ctx.request_repaint();
                }
                return;
            }
            Dialog::HostOptions { host: id } => {
                let id = *id;
                let mut chosen = None;
                if let Some(host) = view.hosts.get(id) {
                    let options = host_options(host);
                    egui::Modal::new(Id::unique("host-options"))
                        .frame(frame)
                        .backdrop_color(Color32::from_black_alpha(150))
                        .show(ctx, |ui| {
                            ui.set_width(340.0);
                            dialog_takes_the_window(ui);
                            ui.label(RichText::new(host.display_name()).font(semibold(text::SECTION)));
                            ui.add_space(6.0);
                            // The list gets what the name and the Wake note leave; it scrolls only
                            // in a window too short for it (1280x720 at 150 %).
                            let note = (host.reach == Reach::Offline && host.record.mac.is_none()).then(|| {
                                let text = RichText::new(tr("Pair once while the PC is on to enable Wake.")).font(regular(text::CAPTION)).color(OUTLINE);
                                egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Wrap), ui.available_width(), egui::TextStyle::Body)
                            });
                            let below = note.as_ref().map_or(0.0, |n| 6.0 + n.size().y + ui.spacing().item_spacing.y);
                            let room = (ui.available_height() - below).max(CONTROL_HEIGHT);
                            // A thin bar that always shows when the list scrolls: its handle in the text
                            // colour, since the widget fill is the dialog's own.
                            ui.spacing_mut().scroll = egui::style::ScrollStyle { foreground_color: true, ..egui::style::ScrollStyle::thin() };
                            let list = egui::ScrollArea::vertical().id_salt("host-options").max_height(room).content_margin(Margin::same(6)).show(ui, |ui| {
                                let follow = Id::unique("host-options-focus");
                                let mut last = ui.ctx().data(|d| d.get_temp::<Option<Id>>(follow)).flatten();
                                crate::gui::follow_focus(ui, &mut last);
                                ui.ctx().data_mut(|d| d.insert_temp(follow, last));
                                for (n, option) in options.iter().enumerate() {
                                    if *option == HostOption::Remove {
                                        ui.separator();
                                    }
                                    let label = RichText::new(tr(option.label())).font(semibold(text::BODY));
                                    let label = if *option == HostOption::Remove { label.color(ERROR) } else { label };
                                    let blocked = match option {
                                        HostOption::Sleep => host.sleep_option().and_then(Result::err),
                                        _ => None,
                                    };
                                    let button = ui.add_enabled_ui(blocked.is_none(), |ui| {
                                        ui.add_sized(
                                            [ui.available_width(), 38.0],
                                            egui::Button::new(label).frame(false).fill(Color32::TRANSPARENT),
                                        )
                                    })
                                    .inner;
                                    if let Some(why) = blocked {
                                        ui.label(RichText::new(tr(why)).font(regular(text::CAPTION)).color(OUTLINE));
                                    }
                                    focus_ring(ui, button.rect, &button, 8);
                                    if n == 0 && focus_first {
                                        button.request_focus();
                                        focused = true;
                                    }
                                    if button.clicked() {
                                        chosen = Some(*option);
                                    }
                                }
                            });
                            scroll_fades(ui, &list, *SURFACE_HIGH);
                            if let Some(note) = note {
                                ui.add_space(6.0);
                                ui.label(note);
                            }
                        });
                    if let Some(option) = chosen {
                        next = self.choose_host_option(host, option, commands);
                        close = next.is_none();
                    }
                } else {
                    close = true;
                }
            }
            Dialog::Rename { host, text } => {
                let id = *host;
                let own = view.hosts.get(id).map_or(String::new(), |h| h.record.name.clone());
                egui::Modal::new(Id::unique("rename"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(tr("Rename PC")).font(semibold(text::SECTION)));
                        ui.label(
                            RichText::new(tr("The name is kept on this device only. Leave it empty to use the PC's own name."))
                                .color(ON_SURFACE_VARIANT),
                        );
                        ui.add_space(8.0);
                        let edit = ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text(own.as_str())
                                .char_limit(crate::config::ALIAS_MAX_CHARS)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(12, 10)),
                        );
                        if focus_first {
                            edit.request_focus();
                            focused = true;
                        }
                        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Save").clicked() || submitted {
                                commands.push(Command::Rename { host: id, alias: text.clone() });
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::ConfirmRemove { host, name } => {
                let id = *host;
                let paired = view.hosts.get(id).is_some_and(|h| h.paired());
                egui::Modal::new(Id::unique("confirm-remove"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Remove {}?", &[&name])).font(semibold(text::SECTION)));
                        let body = if paired {
                            tr("This deletes the PC, its settings and its saved library from this device. The PC still lists this device as paired until you unpair it.")
                        } else {
                            tr("This deletes the PC, its settings and its saved library from this device.")
                        };
                        ui.label(RichText::new(body).color(ON_SURFACE_VARIANT));
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Remove PC").clicked() {
                                commands.push(Command::Remove(id));
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
            Dialog::ConfirmUnpair { host, name } => {
                let id = *host;
                egui::Modal::new(Id::unique("confirm-unpair"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Unpair {}?", &[&name])).font(semibold(text::SECTION)));
                        ui.label(
                            RichText::new(tr("The PC drops this device and its saved library is forgotten. It stays in the list; pair again to stream."))
                                .color(ON_SURFACE_VARIANT),
                        );
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Unpair").clicked() {
                                commands.push(Command::Unpair(id));
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
            Dialog::ConfirmSleep { host, ask, name, offer } => {
                let (id, ask) = (*host, *ask);
                egui::Modal::new(Id::unique("confirm-sleep"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Put {} to sleep?", &[&name])).font(semibold(text::SECTION)));
                        ui.label(
                            RichText::new(tr("The PC sleeps only while no app or stream runs on it. Wake it with Wake up."))
                                .color(ON_SURFACE_VARIANT),
                        );
                        // The asking line keeps its room once the offer is in, and Sleep PC is there
                        // from the start, only off: nothing moves under the pointer or the focus,
                        // so a press meant for Cancel never lands on Sleep PC.
                        let waiting = offer.is_none();
                        ui.add_space(space::M);
                        ui.horizontal(|ui| {
                            if waiting {
                                spinner(ui, 9.0);
                            } else {
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            }
                            ui.add_visible(waiting, egui::Label::new(RichText::new(tr("Asking the PC…")).color(ON_SURFACE_VARIANT)));
                        });
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
                        });
                    });
            }
            Dialog::PairingHelp => {
                egui::Modal::new(Id::unique("pairing-help"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(tr("Connect to your PC")).font(semibold(text::SECTION)));
                        for step in PAIRING_STEPS {
                            ui.label(RichText::new(step).color(ON_SURFACE_VARIANT));
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
                egui::Modal::new(Id::unique("pair-error"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
            Dialog::NetworkTest { stage } => {
                egui::Modal::new(Id::unique("network-test")).frame(frame).backdrop_color(Color32::from_black_alpha(150)).show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(tr("Network test")).font(semibold(text::SECTION)));
                    ui.horizontal(|ui| { spinner(ui, 10.0); ui.label(tr(stage.as_str())); });
                    dialog_buttons(ui, |ui| {
                        let button = quiet_button(ui, "Cancel");
                        if focus_first { button.request_focus(); focused = true; }
                        if button.clicked() { commands.push(Command::CancelNetworkTest); close = true; }
                    });
                });
            }
            Dialog::Shortcut { id, name, chord, error } => {
                egui::Modal::new(Id::unique("shortcut-editor"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(if id.is_some() { tr("Edit shortcut") } else { tr("New shortcut") }).font(semibold(text::SECTION)));
                        ui.add_space(space::S);
                        let edit = dialog_field(ui, egui::TextEdit::singleline(name).hint_text(tr("Shortcut name")));
                        if focus_first { edit.request_focus(); focused = true; }
                        dialog_field(ui, egui::TextEdit::singleline(chord).hint_text("CTRL + ALT + DELETE"));
                        ui.label(RichText::new(tr("Use letters, digits, F1–F24, CTRL, SHIFT, ALT, WIN, TAB, ENTER, ESC, arrows or other key names.")).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
                        if let Some(error) = error { ui.label(RichText::new(error.as_str()).color(ERROR)); }
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Save").clicked() {
                                match self.shortcuts.save(id.as_deref(), name, chord) {
                                    Ok(()) => { commands.push(Command::SaveShortcuts); close = true; }
                                    Err(message) => *error = Some(message),
                                }
                            }
                            if quiet_button(ui, "Cancel").clicked() { close = true; }
                        });
                    });
            }
            Dialog::DeleteShortcut { id, name } => {
                egui::Modal::new(Id::unique("delete-shortcut"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(trf("Delete {}?", &[&name])).font(semibold(text::SECTION)));
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Delete").clicked() {
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
                egui::Modal::new(Id::unique("import-shortcuts"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
                egui::Modal::new(Id::unique("profile-name"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(if id.is_some() { tr("Rename profile") } else { tr("New profile") }).font(semibold(text::SECTION)));
                        ui.add_space(space::S);
                        let edit = dialog_field(ui, egui::TextEdit::singleline(name).hint_text(tr("Profile name")));
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
                egui::Modal::new(Id::unique("delete-profile"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(trf("Delete {}?", &[&name])).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("If this profile is active, new streams will use main settings instead.")).color(ON_SURFACE_VARIANT));
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Delete").clicked() {
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
            Dialog::ConfirmReplace { app, running } => {
                egui::Modal::new(Id::unique("confirm-replace"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Close {} and start {}?", &[&running, &app])).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("The PC asks before it closes the running app. Unsaved progress in it is lost.")).color(ON_SURFACE_VARIANT));
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Close and start").clicked() {
                                commands.push(Command::ConfirmReplace);
                                close = true;
                            }
                            // The safe choice has focus, never the button that closes the game.
                            let cancel = quiet_button(ui, "Cancel");
                            if focus_first {
                                cancel.request_focus();
                                focused = true;
                            }
                            if cancel.clicked() { close = true; }
                        });
                    });
            }
            Dialog::ConfirmAction { host, app, name } => {
                egui::Modal::new(Id::unique("host-action"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(format!("{name}?")).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("This changes the session running on the PC. Ending an app can lose unsaved progress.")).color(ON_SURFACE_VARIANT));
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Continue").clicked() {
                                commands.push(Command::ControlAction { host: *host, app: *app });
                                close = true;
                            }
                            let button = quiet_button(ui, "Cancel");
                            if focus_first {
                                button.request_focus();
                                focused = true;
                            }
                            if button.clicked() { close = true; }
                        });
                    });
            }
            Dialog::Otp { host, pin, passphrase, error } => {
                egui::Modal::new(Id::unique("otp"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(tr("Pair with one-time PIN")).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("Enter the PIN and passphrase created on the PC.")).color(ON_SURFACE_VARIANT));
                        ui.add_space(space::S);
                        let edit = dialog_field(ui, egui::TextEdit::singleline(pin).hint_text(tr("PIN")));
                        if focus_first {
                            edit.request_focus();
                            focused = true;
                        }
                        dialog_field(ui, egui::TextEdit::singleline(passphrase).password(true).hint_text(tr("Passphrase")));
                        if let Some(error) = error {
                            ui.label(RichText::new(error.as_str()).color(ERROR));
                        }
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Pair").clicked() {
                                if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) || passphrase.chars().count() < 4 {
                                    *error = Some(tr("Enter four PIN digits and a passphrase of at least four characters.").into());
                                } else {
                                    commands.push(Command::PairOtp {
                                        host: *host,
                                        pin: std::mem::take(pin),
                                        passphrase: std::mem::take(passphrase),
                                    });
                                    close = true;
                                }
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::AddHost { text, error } => {
                egui::Modal::new(Id::unique("add-host"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(tr("Add a PC")).font(semibold(text::SECTION)));
                        ui.label(
                            RichText::new(tr("Its IP address or name, with :port if the host uses another HTTP port."))
                                .color(ON_SURFACE_VARIANT),
                        );
                        ui.label(
                            RichText::new(tr("Or paste the pairing link from the PC's QR code to add and pair it at once."))
                                .color(ON_SURFACE_VARIANT),
                        );
                        ui.add_space(8.0);
                        let edit = ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text("192.168.1.20")
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(12, 10)),
                        );
                        if focus_first {
                            edit.request_focus();
                            focused = true;
                        }
                        if let Some(e) = error {
                            ui.label(RichText::new(e.as_str()).color(ERROR));
                        }
                        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        dialog_buttons(ui, |ui| {
                            if (primary_button(ui, "Add").clicked() || submitted) && !text.trim().is_empty() {
                                commands.push(Command::AddHost(text.trim().to_owned()));
                                close = true;
                            }
                            if quiet_button(ui, "Paste link").clicked() {
                                commands.push(Command::AddFromClipboard);
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::Pairing { host, pin } => {
                let name = view.hosts.get(*host).map_or(tr("the PC"), |h| h.display_name()).to_owned();
                let pin = pin.clone();
                egui::Modal::new(Id::unique("pairing"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Pair with {}", &[&name])).font(semibold(text::SECTION)));
                        if !pin.is_empty() {
                            ui.label(
                                RichText::new(tr("Enter this PIN in your host’s settings or web UI, usually on its PIN or pairing page."))
                                    .color(ON_SURFACE_VARIANT),
                            );
                        }
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            // The PIN is what the dialog is for: centred, so the eye lands on it.
                            let digits = pin.chars().count() as f32;
                            ui.add_space(((ui.available_width() - digits * 64.0 - (digits - 1.0).max(0.0) * 12.0) / 2.0).max(0.0));
                            for digit in pin.chars() {
                                let (rect, _) = ui.allocate_exact_size(vec2(64.0, 80.0), Sense::hover());
                                ui.painter().rect_filled(rect, CornerRadius::same(12), SURFACE_HIGHEST);
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    digit,
                                    semibold(44.0),
                                    *MOON,
                                );
                            }
                        });
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            spinner(ui, 9.0);
                            ui.label(RichText::new(tr("Waiting for the PC…")).color(ON_SURFACE_VARIANT));
                        });
                        dialog_buttons(ui, |ui| {
                            let r = quiet_button(ui, "Cancel");
                            if focus_first {
                                r.request_focus();
                                focused = true;
                            }
                            if r.clicked() {
                                commands.push(Command::CancelPair);
                                close = true;
                            }
                        });
                    });
            }
            Dialog::Message { title, body } => {
                egui::Modal::new(Id::unique("message"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(title.as_str()).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr(body.as_str())).color(ON_SURFACE_VARIANT));
                        dialog_buttons(ui, |ui| {
                            let r = primary_button(ui, "OK");
                            if focus_first {
                                r.request_focus();
                                focused = true;
                            }
                            if r.clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::ConfirmClose { host, name } => {
                let (host, name) = (*host, name.clone());
                egui::Modal::new(Id::unique("confirm-close"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(tr("Close the game on the PC?")).font(semibold(text::SECTION)));
                        ui.label(RichText::new(trf("{} closes on the PC. Unsaved progress is lost.", &[&name])).color(ON_SURFACE_VARIANT));
                        dialog_buttons(ui, |ui| {
                            let r = primary_button(ui, "Close game");
                            if focus_first {
                                r.request_focus();
                                focused = true;
                            }
                            if r.clicked() {
                                commands.push(Command::CloseGame(host));
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::ConfirmQuit { host, start, running, resume, named } => {
                let (host, start, running, resume, named) = (*host, start.clone(), running.clone(), *resume, *named);
                egui::Modal::new(Id::unique("confirm-quit"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        let title = match &start {
                            Some((_, name)) => trf("Quit {} and start {}?", &[&running, &name]),
                            None => trf("Quit {}?", &[&running]),
                        };
                        ui.label(RichText::new(title).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("Unsaved progress in the running app is lost.")).color(ON_SURFACE_VARIANT));
                        dialog_buttons(ui, |ui| {
                            let r = primary_button(ui, if start.is_some() { en("Quit and start") } else { en("Quit app") });
                            // Replacing a running game starts on the safe choice (Resume, else Cancel), never on
                            // the button that closes it; "Quit app" is what the player just asked for.
                            let replacing = start.is_some();
                            if focus_first && !replacing {
                                r.request_focus();
                                focused = true;
                            }
                            if r.clicked() {
                                commands.push(match start {
                                    Some((app, _)) => Command::QuitAndLaunch { host, app, named },
                                    None => Command::QuitApp(host),
                                });
                                close = true;
                            }
                            // Starting another game replaces the running one: carry on with it instead.
                            if let (Some(app), Some(_)) = (resume, &start) {
                                let r = quiet_button(ui, &trf("Resume {}", &[&running]));
                                if focus_first {
                                    r.request_focus();
                                    focused = true;
                                }
                                if r.clicked() {
                                    commands.push(Command::Launch { host, app, resume: true });
                                    close = true;
                                }
                            }
                            let cancel = quiet_button(ui, "Cancel");
                            if focus_first && replacing && resume.is_none() {
                                cancel.request_focus();
                                focused = true;
                            }
                            if cancel.clicked() {
                                close = true;
                            }
                        });
                    });
            }
        }
        if focused {
            self.focus_dialog = false;
        }
        if let Some(dialog) = next {
            self.dialog = dialog;
            self.focus_dialog = true;
        } else if close {
            // The card goes away with the PC, so only then is there nothing to give focus back
            // to; Cancel keeps it.
            if matches!(self.dialog, Dialog::ConfirmRemove { .. }) && commands.iter().any(|c| matches!(c, Command::Remove(_))) {
                self.focused_key = None;
            }
            self.close_dialog();
        }
    }

    /// Closes the dialog. Those opened from a PC's card give focus back to it, the rest to the
    /// screen's first item.
    pub(super) fn close_dialog(&mut self) {
        let from_card = matches!(
            self.dialog,
            Dialog::HostOptions { .. }
                | Dialog::Rename { .. }
                | Dialog::ConfirmRemove { .. }
                | Dialog::ConfirmUnpair { .. }
                | Dialog::ConfirmSleep { .. }
                | Dialog::PairError { .. }
                | Dialog::PairingHelp
        );
        self.dialog = Dialog::None;
        match self.focused_key.clone().filter(|_| from_card) {
            Some(key) => self.restore_key = Some(key),
            None => self.focus_screen = true,
        }
    }

    /// Acts on an entry of a PC's options; a dialog to ask first is returned.
    pub(super) fn choose_host_option(&mut self, host: &Host, option: HostOption, commands: &mut Vec<Command>) -> Option<Dialog> {
        let id = host.id;
        let name = host.display_name().to_owned();
        match option {
            HostOption::OpenLibrary => commands.push(Command::OpenApps(id)),
            HostOption::Resume => {
                if let Some(app) = running_entry(host) {
                    commands.push(Command::Launch { host: id, app: app.id, resume: true });
                }
            }
            HostOption::QuitApp => {
                return Some(Dialog::ConfirmQuit { host: id, start: None, running: running_name(host), resume: None, named: host.running_app() });
            }
            HostOption::Pair => commands.push(Command::Pair(id)),
            HostOption::PairOtp => {
                return Some(Dialog::Otp { host: id, pin: String::new(), passphrase: String::new(), error: None });
            }
            HostOption::Wake => commands.push(Command::Wake(id)),
            HostOption::Sleep => {
                if host.sleep_option() == Some(Ok(())) {
                    self.sleep_asks += 1;
                    let ask = self.sleep_asks;
                    commands.push(Command::SleepOffer { host: id, ask });
                    return Some(Dialog::ConfirmSleep { host: id, ask, name, offer: None });
                }
            }
            HostOption::Files => commands.push(Command::OpenFiles(id)),
            HostOption::Settings => commands.push(Command::HostSettings(id)),
            HostOption::Details => self.open(Screen::HostDetails(id)),
            HostOption::Rename => return Some(Dialog::Rename { host: id, text: host.rename_prefill().to_owned() }),
            HostOption::Unpair => return Some(Dialog::ConfirmUnpair { host: id, name }),
            HostOption::PairingHelp => return Some(Dialog::PairingHelp),
            HostOption::CheckAgain => commands.push(Command::Refresh(id)),
            HostOption::Remove => return Some(Dialog::ConfirmRemove { host: id, name }),
        }
        None
    }

    /// The PC answered a pairing request. Returns true when this device is now paired and the
    /// PIN dialog was still open. Pairing that was cancelled meanwhile says nothing: a failure
    /// the user walked away from is not shown as an error.
    pub fn pairing_finished(&mut self, host: u64, result: Result<(), String>) -> bool {
        let one_time = match &self.dialog {
            Dialog::Pairing { host: asked, pin } if *asked == host => pin.is_empty(),
            _ => return false,
        };
        match result {
            Ok(()) => {
                self.dialog = Dialog::None;
                true
            }
            Err(e) => {
                self.pair_failed(host, e, one_time);
                false
            }
        }
    }

    /// A Sleep PC step came back. The offer fills the confirmation that waits for it, matched by
    /// `ask`: after Cancel and Sleep PC again, the first request's late token (which the second
    /// request has replaced on the PC) is dropped. An offer or failure the user has walked away
    /// from says nothing. The confirmed request's answer is always shown, named after the PC,
    /// and waits for any other dialog the user is in (a PIN, a rename) to close.
    pub fn sleep_answered(&mut self, host: u64, ask: u64, name: &str, confirmed: bool, result: Result<SleepStep, String>) {
        let waiting = matches!(&self.dialog, Dialog::ConfirmSleep { host: h, ask: a, offer: None, .. } if *h == host && *a == ask);
        if !confirmed && !waiting {
            return;
        }
        match result {
            Ok(SleepStep::Offer { token, at }) => {
                if let Dialog::ConfirmSleep { offer, .. } = &mut self.dialog {
                    *offer = Some((token, at));
                    // The buttons moved: Cancel takes focus again.
                    self.focus_dialog = true;
                }
            }
            Ok(SleepStep::Accepted) => self.sleep_message(
                host,
                trf("{} is going to sleep", &[&name]),
                tr("The PC took the request. It shows as offline once it sleeps.").to_owned(),
            ),
            Err(e) => self.sleep_message(host, trf("Could not put {} to sleep", &[&name]), e),
        }
    }

    /// Shows a Sleep PC answer now when it replaces nothing but its own dialog, else after the
    /// dialog the user is in.
    pub(super) fn sleep_message(&mut self, host: u64, title: String, body: String) {
        match &self.dialog {
            Dialog::None => self.message(title, body),
            Dialog::ConfirmSleep { host: asked, .. } if *asked == host => self.message(title, body),
            _ => self.queued.push_back((title, body)),
        }
    }

    /// Shows why pairing failed, with a way to try again.
    pub fn pair_failed(&mut self, host: u64, body: impl Into<String>, one_time: bool) {
        self.dialog = Dialog::PairError { host, body: body.into(), one_time };
        self.focus_dialog = true;
    }

    pub(super) fn connecting(&mut self, ctx: &egui::Context, c: &ConnectingView, commands: &mut Vec<Command>) {
        let frame = Frame::new()
            .fill(*SURFACE_HIGH)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(28))
            .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
        egui::Modal::new(Id::unique("connecting"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(170))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(&c.app).font(semibold(text::SECTION)));
                ui.label(RichText::new(trf("on {}", &[&c.host])).color(ON_SURFACE_VARIANT));
                ui.add_space(space::M);
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new(tr(&c.stage)).color(ON_SURFACE));
                });
                dialog_buttons(ui, |ui| {
                    let r = quiet_button(ui, "Cancel");
                    r.request_focus();
                    if r.clicked() {
                        commands.push(Command::CancelConnect);
                    }
                });
            });
    }
}

/// "Waking <PC>…" while the wake packets go out, with a way to stop.
pub(super) fn waking(ctx: &egui::Context, host: &str, commands: &mut Vec<Command>) {
    let frame = Frame::new()
        .fill(*SURFACE_HIGH)
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(28))
        .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
    egui::Modal::new(Id::unique("waking"))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(170))
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new(trf("Waking {}…", &[&host])).font(semibold(text::SECTION)));
            ui.add_space(space::M);
            ui.horizontal(|ui| {
                spinner(ui, 10.0);
                ui.label(RichText::new(tr("Sending Wake-on-LAN until the PC answers.")).color(ON_SURFACE));
            });
            dialog_buttons(ui, |ui| {
                let r = quiet_button(ui, "Cancel");
                r.request_focus();
                if r.clicked() {
                    commands.push(Command::CancelWake);
                }
            });
        });
}

/// A dialog's buttons, the same on every dialog: at its foot, aligned right, the one primary
/// action at the right edge and the other choices left of it, Cancel leftmost (Android's order).
/// Add them primary first: the layout runs right to left. Escape or B cancels any dialog.
pub(super) fn dialog_buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(space::L);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        add(ui);
    });
}

/// A dialog's one-line text field: full width, with room around its text.
pub(super) fn dialog_field(ui: &mut Ui, edit: egui::TextEdit<'_>) -> Response {
    ui.add(edit.desired_width(f32::INFINITY).margin(Margin::symmetric(12, 10)))
}

/// Lets a dialog grow to the window's height less a margin and its frame, and no further. egui
/// lays a new area out in at most 600x400 points and from then on in the size it took, so a list
/// in a dialog never got more than that and its last row was cut in any window.
pub(super) fn dialog_takes_the_window(ui: &mut Ui) {
    const FRAME: f32 = 28.0;
    ui.set_max_height(ui.ctx().content_rect().height() - 2.0 * (space::L + FRAME));
}
