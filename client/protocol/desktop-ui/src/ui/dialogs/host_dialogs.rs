//! Host menus, game menus, host/game confirmation, and settings dialogs.

use super::*;

impl Launcher {
    pub(super) fn draw_host_dialog(
        &mut self,
        ctx: &egui::Context,
        view: &View<'_>,
        commands: &mut Vec<Command>,
        dialog: &mut Dialog,
        flow: &mut DialogFlow,
    ) -> bool {
        let focus_first = flow.focus_first;
        let keys_on_screen = flow.keys_on_screen;
        let mut close = false;
        let mut next: Option<Dialog> = None;
        let mut leave: Option<Screen> = None;
        let mut focused = false;
        match dialog {
            Dialog::HostOptions { host: id } => {
                let id = *id;
                if let Some(host) = view.hosts.get(id) {
                    let options = host_options(host);
                    let enter = enter_option(host, self.settings.quick_resume);
                    let rows: Vec<MenuRow> = options
                        .iter()
                        .map(|o| MenuRow {
                            icon: o.icon(),
                            label: o.label(),
                            hint: (*o == enter).then_some(en("Enter")),
                            destructive: o.destructive(),
                            blocked: match o {
                                HostOption::Sleep => host.sleep_option().and_then(Result::err),
                                HostOption::RestartPc => host.pc_power_option(PcPower::Restart).and_then(Result::err),
                                HostOption::ShutDownPc => host.pc_power_option(PcPower::ShutDown).and_then(Result::err),
                                HostOption::PairAnother => host.pair_another_option().and_then(Result::err),
                                _ => None,
                            },
                        })
                        .collect();
                    // Says what the PCs page says of it: the running game's name, or Online; offline,
                    // why it did not answer.
                    let status = pcs::offline_reason(host).map(str::to_owned).unwrap_or_else(|| pcs::pc_line_of(host).words().to_owned());
                    // Wake needs the PC's address, learnt while pairing with it on.
                    let note = (host.reach == Reach::Offline && !host.record.can_wake()).then_some(en("Pair once while the PC is on to enable Wake up."));
                    let (outcome, took) = popover(ctx, "host-options", self.menu_anchor, host.display_name(), Some((status, *ON_SURFACE_VARIANT)), &rows, note, focus_first);
                    focused = took;
                    match outcome {
                        MenuOutcome::Chosen(n) => {
                            next = self.choose_host_option(host, options[n], commands);
                            close = next.is_none();
                        }
                        MenuOutcome::Dismissed => close = true,
                        MenuOutcome::Open => {}
                    }
                } else {
                    close = true;
                }
            }
            Dialog::PcSheet { current } => {
                let (outcome, took) = pc_sheet(ctx, self.menu_anchor, &view.hosts.list, *current, focus_first);
                focused = took;
                match outcome {
                    // What Enter does on this PC's card on the PCs page.
                    MenuOutcome::Chosen(n) => {
                        let host = &view.hosts.list[n];
                        commands.push(pcs::pc_action(host).command(host.id));
                        close = true;
                    }
                    MenuOutcome::Dismissed => close = true,
                    MenuOutcome::Open => {}
                }
            }
            Dialog::GameMenu { host, game } => {
                let (id, game) = (*host, *game);
                match view.hosts.get(id).and_then(|host| game_menu(host, view, game, &self.favourites, &self.hidden_games).map(|menu| (host, menu))) {
                    Some((host, (title, mut actions))) => {
                        actions.extend(open_on_start_action(host, game, &self.favourites, self.open_game.as_ref()));
                        let rows: Vec<MenuRow> = actions
                            .iter()
                            .enumerate()
                            .map(|(n, a)| a.row(n == 0 && *a != GameAction::Library, *a == GameAction::Unstar && matches!(game, GameTarget::Library { favourite: true, .. })))
                            .collect();
                        let on = trf("On {}", &[&host.display_name()]);
                        let (outcome, took) = popover(ctx, "game-menu", self.menu_anchor, &title, Some((on, *ON_SURFACE_VARIANT)), &rows, None, focus_first);
                        focused = took;
                        match outcome {
                            MenuOutcome::Chosen(n) => {
                                next = self.choose_game_action(host, game, actions[n], commands);
                                close = next.is_none();
                                if matches!(actions[n], GameAction::Hide | GameAction::Unhide) {
                                    let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
                                    let games = hide_keys_of(host, game, &sources);
                                    commands.push(Command::SetHidden { games, hidden: actions[n] == GameAction::Hide });
                                    // Its card goes from where it is and the focus to its neighbour,
                                    // as for Unstar below. Continue playing keeps a hidden game's card,
                                    // and so does the PC's page while it shows hidden games.
                                    let shown_page = self.show_hidden_apps;
                                    if !self.focused_key.as_deref().is_some_and(|key| key.starts_with("continue:") || (shown_page && key.starts_with("app:"))) {
                                        self.focused_key = self.focused_key.as_deref().and_then(|key| card_neighbour(&self.cards, key));
                                    }
                                }
                                // The Favourites card goes with its star: the focus goes back to
                                // its neighbour instead, as Delete on the card does.
                                if let (GameAction::Unstar, GameTarget::Library { app, favourite: true }) = (actions[n], game) {
                                    let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
                                    let starred = console::favourite_games(&sources, &self.favourites);
                                    if let Some(index) = starred.iter().position(|s| s.host == id && s.app_id == app) {
                                        self.focused_key = favourite_neighbour(&starred, index);
                                    }
                                }
                            }
                            MenuOutcome::Dismissed => close = true,
                            MenuOutcome::Open => {}
                        }
                    }
                    None => close = true,
                }
            }
            Dialog::HomeView => {
                // A row steps through its choices and Home changes behind the menu at once, so
                // the menu stays open to try the next.
                let s = &self.settings;
                let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
                let library = self.library.get(&sources, &self.hidden_games);
                let entries = library.shown();
                let groups = console::offered_groups(entries);
                let group = console::shown_group(s.home_group, entries);
                let sorts = console::offered_sorts(entries);
                let sort = console::Sort::shown(s, entries).by;
                let mut rows = vec![
                    MenuRow { icon: Icon::Grid, label: en("Layout"), hint: Some(s.home_layout.label()), destructive: false, blocked: None },
                    MenuRow { icon: Icon::Folder, label: en("Group by"), hint: Some(group.label()), destructive: false, blocked: None },
                    MenuRow { icon: Icon::Sliders, label: en("Sort by"), hint: Some(sort.label()), destructive: false, blocked: None },
                    MenuRow { icon: Icon::Down, label: en("Order"), hint: Some(s.home_order.label()), destructive: false, blocked: None },
                ];
                // A list's lines have one size, so Card size shows only for the covers' layouts.
                if s.home_layout != HomeLayout::List {
                    rows.push(MenuRow { icon: Icon::Monitor, label: en("Card size"), hint: Some(s.home_card_size.label()), destructive: false, blocked: None });
                }
                let (outcome, took) = popover(ctx, "home-view", self.menu_anchor, tr("View"), None, &rows, None, focus_first);
                focused = took;
                match outcome {
                    MenuOutcome::Chosen(n) => {
                        let s = &mut self.settings;
                        match n {
                            0 => s.home_layout = next_of(&HomeLayout::ALL, s.home_layout),
                            1 => s.home_group = next_of(&groups, group),
                            2 => s.home_sort = next_of(&sorts, sort),
                            3 => s.home_order = next_of(&HomeOrder::ALL, s.home_order),
                            _ => s.home_card_size = next_of(&CardSize::ALL, s.home_card_size),
                        }
                        commands.push(Command::SaveSettings);
                    }
                    MenuOutcome::Dismissed => close = true,
                    MenuOutcome::Open => {}
                }
            }
            Dialog::Rename { host, text } => {
                let id = *host;
                let own = view.hosts.get(id).map_or(String::new(), |h| h.record.name.clone());
                modal(ctx, "rename", |ui| {
                    ui.set_width(420.0);
                    let title = ui.label(RichText::new(tr("Rename PC")).font(semibold(text::SECTION)));
                    ui.add_space(8.0);
                    // Left empty, the PC goes by the name the placeholder shows.
                    let edit = ui.add(
                        egui::TextEdit::singleline(text)
                            .hint_text(hint(&own, regular(text::BODY)))
                            .char_limit(crate::config::ALIAS_MAX_CHARS)
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(12, 10)),
                    )
                    .labelled_by(title.id);
                    if keys_on_screen {
                        ui.add_space(space::S);
                        let first = text_pad(ui, text, Some(crate::config::ALIAS_MAX_CHARS));
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
            Dialog::WakeAddresses { host, text, invalid } => {
                let id = *host;
                modal(ctx, "wake-addresses", |ui| {
                    ui.set_width(440.0);
                    let title = ui.label(RichText::new(tr("Other wake addresses")).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("Wake-on-LAN packets also go to these MAC addresses, for a network adapter that can wake the PC when the one used for streaming cannot. One per line, for example 00:11:22:33:44:55.")).color(ON_SURFACE_VARIANT));
                    ui.add_space(8.0);
                    let edit = ui.add(
                        egui::TextEdit::multiline(text)
                            .hint_text(hint("00:11:22:33:44:55", regular(text::BODY)))
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(12, 10)),
                    )
                    .labelled_by(title.id);
                    if focus_first {
                        edit.request_focus();
                        focused = true;
                    }
                    if *invalid {
                        ui.label(RichText::new(tr("Enter up to 8 MAC addresses, such as 00:11:22:33:44:55")).color(ERROR));
                    }
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, "Save").clicked() {
                            match crate::wol::parse_macs(text) {
                                Some(macs) => {
                                    commands.push(Command::WakeAddresses { host: id, macs });
                                    close = true;
                                }
                                None => *invalid = true,
                            }
                        }
                        if quiet_button(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::ConfirmNeural { host, game } => {
                let (host, game) = (*host, *game);
                modal(ctx, "confirm-neural", |ui| {
                    ui.set_width(480.0);
                    ui.label(
                        RichText::new(tr("Install a third-party game mod?"))
                            .font(semibold(text::SECTION)),
                    );
                    ui.label(RichText::new(tr(NEURAL_NOTICE)).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, "Accept and install").clicked() {
                            self.neural.write(host, game, true, true);
                            close = true;
                        }
                        // Cancel is the default.
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
            Dialog::ConfirmRemove { host, name } => {
                let id = *host;
                let paired = view.hosts.get(id).is_some_and(|h| h.paired());
                modal(ctx, "confirm-remove", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("Remove {}?", &[&name])).font(semibold(text::SECTION)));
                    let body = if paired {
                        tr("This deletes the PC, its settings and its saved library from this device. The PC still lists this device as paired until you unpair it.")
                    } else {
                        tr("This deletes the PC, its settings and its saved library from this device.")
                    };
                    ui.label(RichText::new(body).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        if danger_button(ui, "Remove PC").clicked() {
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
            Dialog::ConfirmLink { link, shown } => {
                let now = ctx.input(|i| i.time);
                let waited = now - *shown.get_or_insert(now);
                let may_pair = waited >= PAIR_LINK_DELAY;
                if !may_pair {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(PAIR_LINK_DELAY - waited));
                }
                let name = link.shown_name();
                let shown: &str = if name.is_empty() { tr("the PC") } else { &name };
                let title = trf("Pair with {}", &[&shown]);
                modal(ctx, "confirm-link", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(title).font(semibold(text::SECTION)));
                    ui.label(RichText::new(trf("Address: {}", &[&link.shown_address()])).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        if ui.add_enabled_ui(may_pair, |ui| primary_button(ui, "Pair")).inner.clicked() {
                            commands.push(Command::PairLink(link.clone()));
                            close = true;
                        }
                        // Cancel has focus: a key or button already held down must not pair.
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
            Dialog::ConfirmInvite { link, shown } => {
                let now = ctx.input(|i| i.time);
                let waited = now - *shown.get_or_insert(now);
                let may_join = waited >= PAIR_LINK_DELAY;
                if !may_join {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                        PAIR_LINK_DELAY - waited,
                    ));
                }
                let name = link.shown_name();
                let shown: &str = if name.is_empty() { tr("the PC") } else { &name };
                let unix = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let expired = link.expired(unix);
                modal(ctx, "confirm-invite", |ui| {
                    ui.set_width(460.0);
                    ui.label(
                        RichText::new(trf("Join {} over the internet?", &[&shown]))
                            .font(semibold(text::SECTION)),
                    );
                    if expired {
                        ui.label(
                            RichText::new(tr("This invite has expired. Ask for a new one."))
                                .color(ON_SURFACE_VARIANT),
                        );
                    } else {
                        ui.label(
                            RichText::new(tr(
                                "This device pairs with that PC as a guest: you can join a game someone plays there when they let you in. The invite works once.",
                            ))
                            .color(ON_SURFACE_VARIANT),
                        );
                        ui.label(
                            RichText::new(trf(
                                "What you may do: {}",
                                &[&crate::stream_overlay_widgets::permission_label(&link.permission)],
                            ))
                            .color(ON_SURFACE_VARIANT),
                        );
                    }
                    dialog_buttons(ui, |ui| {
                        if !expired
                            && ui
                                .add_enabled_ui(may_join, |ui| primary_button(ui, "Join"))
                                .inner
                                .clicked()
                        {
                            commands.push(Command::RedeemInvite(link.clone()));
                            close = true;
                        }
                        // Cancel has focus: a key or button already held down must not join.
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
            Dialog::Inviting { name } => {
                modal(ctx, "inviting", |ui| {
                    ui.set_width(440.0);
                    ui.label(
                        RichText::new(trf("Connecting to {} over the internet…", &[name]))
                            .font(semibold(text::SECTION)),
                    );
                    ui.horizontal(|ui| {
                        spinner(ui, 10.0);
                        ui.label(tr("This can take up to a minute."));
                    });
                    dialog_buttons(ui, |ui| {
                        // Hiding it leaves the attempt running: its answer comes as a message.
                        let button = quiet_button(ui, "Close");
                        if focus_first {
                            button.request_focus();
                            focused = true;
                        }
                        if button.clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::ConfirmUnpair { host, name } => {
                let id = *host;
                modal(ctx, "confirm-unpair", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(trf("Unpair {}?", &[&name])).font(semibold(text::SECTION)));
                    ui.label(
                        RichText::new(tr("The PC drops this device and its saved library is forgotten. It stays in the list; pair again to stream."))
                            .color(ON_SURFACE_VARIANT),
                    );
                    dialog_buttons(ui, |ui| {
                        if danger_button(ui, "Unpair").clicked() {
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
            Dialog::ConfirmLeave { to } => {
                let to = *to;
                modal(ctx, "confirm-leave", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr("Cancel the transfers?")).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("Leaving this page cancels the transfers that have not finished.")).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, "Leave").clicked() {
                            leave = Some(to);
                            close = true;
                        }
                        let stay = quiet_button(ui, "Stay");
                        if focus_first {
                            stay.request_focus();
                            focused = true;
                        }
                        if stay.clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::OpenPlay { game, pc, left } => {
                modal(ctx, "open-play", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(game.as_str()).font(semibold(text::SECTION)));
                    let line = if *left == 0 { trf("Starting on {}", &[&pc]) } else { trf("Starting on {} in {}", &[&pc, &left]) };
                    ui.label(RichText::new(line).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        // The start is the default: a press of A or Enter here stops it.
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
            Dialog::ConfirmExit => {
                modal(ctx, "confirm-exit", |ui| {
                    ui.set_width(if self.power.is_empty() { 440.0 } else { 640.0 });
                    ui.label(RichText::new(tr("Close Nightjar?")).font(semibold(text::SECTION)));
                    dialog_buttons(ui, |ui| {
                        // Close Nightjar is the default, one press of A: with a controller it is the
                        // last button the D-pad reaches, and a press of B or Escape still keeps
                        // Nightjar open (`dialog_buttons`).
                        let close_button = primary_button(ui, "Close Nightjar");
                        if focus_first {
                            close_button.request_focus();
                            focused = true;
                        }
                        if close_button.clicked() {
                            commands.push(Command::Quit);
                            close = true;
                        }
                        // This PC's power, for one run by controller alone, as far as the system
                        // allows: each asked once more. Right to left, so Shut down ends up first
                        // after Cancel.
                        for offer in self.power.iter().rev() {
                            if quiet_button(ui, offer.action.label()).clicked() {
                                next = Some(Dialog::ConfirmPower(*offer));
                            }
                        }
                        // B or Escape cancels: the question never closes the app by accident.
                        let cancel = quiet_button(ui, "Cancel");
                        if cancel.clicked() {
                            close = true;
                        }
                    });
                });
            }
            // Android's PresetPreference: the preset's name, what it replaces, OK or Cancel.
            Dialog::ConfirmPreset(preset) => {
                let preset = *preset;
                modal(ctx, "confirm-preset", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr(preset_label(preset))).font(semibold(text::SECTION)));
                    ui.label(RichText::new(tr("This replaces resolution, frame rate, bitrate, frame pacing, upscaling, VRR and performance settings with this preset’s values. Codec and HDR choices stay as they are.")).color(ON_SURFACE_VARIANT));
                    dialog_buttons(ui, |ui| {
                        let ok = primary_button(ui, "OK");
                        if focus_first {
                            ok.request_focus();
                            focused = true;
                        }
                        if ok.clicked() {
                            apply_preset(&mut self.settings, preset);
                            (self.custom_size, self.custom_fps) = (None, None);
                            commands.push(Command::SaveSettings);
                            close = true;
                        }
                        if quiet_button(ui, "Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::ConfirmPower(offer) => {
                let offer = *offer;
                let action = offer.action;
                modal(ctx, "confirm-power", |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(tr(action.question())).font(semibold(text::SECTION)));
                    dialog_buttons(ui, |ui| {
                        if primary_button(ui, action.label()).clicked() {
                            commands.push(Command::Power(offer));
                            close = true;
                        }
                        // Focus starts on Cancel, as on the question before.
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
            _ => return false,
        }
        flow.close |= close;
        if next.is_some() { flow.next = next; }
        if leave.is_some() { flow.leave = leave; }
        flow.focused |= focused;
        true
    }
}
