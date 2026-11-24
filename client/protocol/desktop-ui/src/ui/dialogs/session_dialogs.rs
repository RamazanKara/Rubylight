//! Session replace, connection test, frame-pace, hotkey, and stream-move dialogs.

use super::*;

impl Launcher {
    pub(super) fn draw_session_dialog(
        &self,
        ctx: &egui::Context,
        view: &View<'_>,
        commands: &mut Vec<Command>,
        dialog: &mut Dialog,
        flow: &mut DialogFlow,
    ) -> bool {
        let focus_first = flow.focus_first;
        let keys_on_screen = flow.keys_on_screen;
        let mut close = false;
        let mut focused = false;
        match dialog {
            Dialog::ConfirmReplace { app, running } => {
                modal(ctx, "confirm-replace", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("Close {} and start {}?", &[&running, &app])).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("The PC asks before it closes the running app. Unsaved progress in it is lost.")).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        if danger_button(ui, "Close and start").clicked() {
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
            Dialog::ContinueHere { host, app, resume, name, device, join } => {
                let (host, app, resume, join) = (*host, *app, *resume, *join);
                modal(ctx, "continue-here", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("{} is playing on {}", &[&name, &device])).font(semibold(text::SECTION)));
                    dialog_buttons(ui, |ui| {
                        let here = primary_button(ui, "Continue here");
                        if focus_first {
                            here.request_focus();
                            focused = true;
                        }
                        if here.clicked() {
                            commands.push(Command::ContinueHere { host, app });
                            close = true;
                        }
                        let too = if join { quiet_button(ui, "Join") } else { quiet_button(ui, "Stream here too") };
                        if too.clicked() {
                            commands.push(Command::StreamHereToo { host, app, resume });
                            close = true;
                        }
                        if quiet_button(ui, "Cancel").clicked() { close = true; }
                    });
                });
            }
            Dialog::NowPlaying { host, app, device } => {
                let (host, app) = (*host, *app);
                modal(ctx, "now-playing", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(now_playing_title(device)).font(semibold(text::SECTION)));
                    dialog_buttons(ui, |ui| {
                        let ok = primary_button(ui, "OK");
                        if focus_first {
                            ok.request_focus();
                            focused = true;
                        }
                        if ok.clicked() { close = true; }
                        if quiet_button(ui, "Continue here").clicked() {
                            commands.push(Command::ContinueHere { host, app });
                            close = true;
                        }
                    });
                });
            }
            Dialog::GameCameBack { host, app } => {
                let (host, app) = (*host, *app);
                modal(ctx, "game-came-back", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("The game came back to this device")).font(semibold(text::SECTION)));
                    dialog_buttons(ui, |ui| {
                        let resume = primary_button(ui, "Resume");
                        if focus_first {
                            resume.request_focus();
                            focused = true;
                        }
                        // The start asks for the PIN where the lock covers this PC, and asks
                        // again who plays: the game may have moved on since.
                        if resume.clicked() {
                            commands.push(Command::Launch { host, app, resume: true });
                            close = true;
                        }
                        if quiet_button(ui, "OK").clicked() { close = true; }
                    });
                });
            }
            Dialog::MoveFailed { host, app } => {
                let (host, app) = (*host, *app);
                modal(ctx, "move-failed", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("The game did not move to the other device")).font(semibold(text::SECTION)));
                    dialog_buttons(ui, |ui| {
                        let too = primary_button(ui, "Stream here too");
                        if focus_first {
                            too.request_focus();
                            focused = true;
                        }
                        if too.clicked() {
                            commands.push(Command::StreamTooAfterFailedMove { host, app });
                            close = true;
                        }
                        if quiet_button(ui, "OK").clicked() { close = true; }
                    });
                });
            }
            Dialog::ConfirmAction { host, app, name } => {
                modal(ctx, "host-action", |ui| {
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
                modal(ctx, "otp", |ui| {
                    ui.set_width(420.0);
                    ui.label(RichText::new(tr("Pair with one-time PIN")).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("Enter the PIN and passphrase created on the PC.")).color(ON_SURFACE_VARIANT));
                    ui.add_space(space::S);
                    let edit = dialog_field(ui, egui::TextEdit::singleline(pin).hint_text(hint(tr("PIN"), regular(text::BODY))));
                    if focus_first {
                        edit.request_focus();
                        focused = true;
                    }
                    dialog_field(ui, egui::TextEdit::singleline(passphrase).password(true).hint_text(hint(tr("Passphrase"), regular(text::BODY))));
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
                modal(ctx, "add-host", |ui| {
                    ui.set_width(420.0);
                    let title = ui.label(RichText::new(tr("Add a PC")).font(semibold(text::SECTION)));
                    ui.add_space(8.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(text)
                            .hint_text(hint(tr("IP address or name"), regular(text::BODY)))
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(12, 10)),
                    )
                    .labelled_by(title.id);
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(ERROR));
                    }
                    if keys_on_screen {
                        ui.add_space(space::S);
                        let first = text_pad(ui, text, None);
                        if focus_first {
                            first.request_focus();
                            focused = true;
                        }
                    } else if focus_first {
                        edit.request_focus();
                        focused = true;
                    }
                    let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    dialog_buttons(ui, |ui| {
                        if (primary_button(ui, "Add").clicked() || submitted) && !text.trim().is_empty() {
                            // An entry that does not read stays here with its reason, so a typo is
                            // corrected rather than typed again.
                            match crate::hosts::parse_entry(text) {
                                Ok(_) => {
                                    commands.push(Command::AddHost(text.trim().to_owned()));
                                    close = true;
                                }
                                Err(why) => {
                                    *error = Some(tr(why).into());
                                    edit.request_focus();
                                }
                            }
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
                modal(ctx, "pairing", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("Pair with {}", &[&name])).font(semibold(text::SECTION)));
                    ui.add_space(18.0);
                    let digits: Vec<char> = pin.chars().collect();
                    pin_slots(ui, &digits, digits.len(), false, 76.0, &pin);
                    if !pin.is_empty() {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(tr("Enter this PIN in the PC’s settings or web UI, usually on its PIN or pairing page."))
                                .color(ON_SURFACE_VARIANT),
                        );
                    }
                    ui.add_space(14.0);
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
                modal(ctx, "message", |ui| {
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
            Dialog::KeptSettings { pcs } => {
                modal(ctx, "kept-settings", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("Some PCs and games keep their own settings")).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("Older versions could keep a setting for a whole PC when it was only opened.")).color(ON_SURFACE_VARIANT));
                    ui.add_space(space::S);
                    for pc in pcs.iter() {
                        ui.label(pc.line());
                    }
                    dialog_buttons(ui, |ui| {
                        if danger_button(ui, "Reset all").clicked() {
                            commands.push(Command::ResetKeptSettings);
                            close = true;
                        }
                        // Keep has focus: some lines may be choices, which one A press must not drop.
                        let keep = quiet_button(ui, "Keep");
                        if focus_first { keep.request_focus(); focused = true; }
                        if keep.clicked() { close = true; }
                    });
                });
            }
            Dialog::ConfirmClose { host, name } => {
                let (host, name) = (*host, name.clone());
                modal(ctx, "confirm-close", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("Quit the game on the PC?")).font(semibold(text::SECTION)));
                    ui.label(RichText::new(trf("{} quits on the PC. Unsaved progress is lost.", &[&name])).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        let r = danger_button(ui, "Quit game");
                        if r.clicked() {
                            commands.push(Command::CloseGame(host));
                            close = true;
                        }
                        // Cancel has focus: two A presses must not lose the game's unsaved progress.
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
            Dialog::ConfirmQuit { host, start, running, resume, named } => {
                let (host, start, running, resume, named) = (*host, start.clone(), running.clone(), *resume, *named);
                modal(ctx, "confirm-quit", |ui| {
                    ui.set_width(440.0);
                    let title = match &start {
                        Some((_, name)) => trf("Quit {} and start {}?", &[&running, &name]),
                        None => trf("Quit {}?", &[&running]),
                    };
                    ui.label(RichText::new(title).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("Unsaved progress in the running game is lost.")).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        let r = danger_button(ui, if start.is_some() { en("Quit and start") } else { en("Quit game") });
                        // The question starts on the safe choice (Resume, else Cancel), never on the
                        // button that closes the game.
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
                        if focus_first && !(start.is_some() && resume.is_some()) {
                            cancel.request_focus();
                            focused = true;
                        }
                        if cancel.clicked() {
                            close = true;
                        }
                    });
                });
            }
            _ => return false,
        }
        flow.close |= close;
        flow.focused |= focused;
        true
    }
}
