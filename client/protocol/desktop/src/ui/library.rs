//! PCs, Home and a PC's library: the pages that list PCs and games, a game's page, and what
//! selecting a PC or game does.

use super::*;

impl Launcher {
    pub(super) fn add_host_dialog(&mut self) {
        self.dialog = Dialog::AddHost {
            text: String::new(),
            error: None,
        };
        self.focus_dialog = true;
    }

    pub(super) fn hosts(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        ui.label(RichText::new(tr("Your PCs")).font(semibold(text::TITLE)).color(ON_SURFACE));
        ui.label(
            RichText::new(tr("PCs running a Moonlight-compatible host on this network appear here. Add one by address if it doesn't."))
                .color(ON_SURFACE_VARIANT),
        );
        ui.add_space(18.0);
        if view.hosts.list.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                egui::Image::from_texture((logo, vec2(72.0, 72.0))).ui(ui);
                ui.add_space(14.0);
                if self.identity_ready {
                    ui.label(RichText::new(tr("Looking for PCs…")).font(semibold(text::CARD_TITLE)));
                } else {
                    ui.label(RichText::new(tr("Creating this device's pairing key…")).font(semibold(text::CARD_TITLE)));
                }
                ui.label(
                    RichText::new(tr("Start a Moonlight-compatible host on your PC and keep both on the same network."))
                        .color(ON_SURFACE_VARIANT),
                );
                ui.add_space(12.0);
                let r = primary_button(ui, "Add a PC by address");
                if self.focus_screen {
                    r.request_focus();
                    self.focus_screen = false;
                }
                if r.clicked() {
                    self.dialog = Dialog::AddHost {
                        text: String::new(),
                        error: None,
                    };
                    self.focus_dialog = true;
                }
                how_to_connect(ui, commands);
            });
            return;
        }
        scroll(ui, "hosts", |ui| {
            let columns = ((ui.available_width() + 16.0) / (CARD_W + 16.0)).floor().max(1.0) as usize;
            for (row, chunk) in view.hosts.list.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (i, host) in chunk.iter().enumerate() {
                        let response = host_card(ui, host, None);
                        let options = options_button(ui, response.rect, response.id.with("options"));
                        self.focus_item(&response, &format!("host:{}", host.id), row == 0 && i == 0);
                        self.focus_item(&options, &format!("options:{}", host.id), false);
                        if response.clicked() {
                            host_clicked(host, self.settings.quick_resume, commands);
                        }
                        if options.clicked() || options_requested(ui, &[&response, &options]) {
                            self.dialog = Dialog::HostOptions { host: host.id };
                            self.focus_dialog = true;
                        }
                    }
                });
                ui.add_space(16.0);
            }
            // A short list may not be all of them: say the search goes on and where help is.
            if view.hosts.list.len() <= columns {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::S;
                    ui.label(RichText::new(tr("Looking for other PCs on this network")).color(ON_SURFACE_VARIANT));
                    let help = how_to_connect(ui, commands);
                    self.focus_item(&help, "how-to-connect", false);
                });
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(trf("Options for a PC: F2, right-click, the three dots, or the controller's {} button. Ctrl+N adds a PC.", &[&crate::glyphs::Style::of(view.pads).face(crate::glyphs::Face::West)]))
                    .color(OUTLINE)
                    .font(regular(text::CAPTION)),
            );
        });
    }

    pub(super) fn apps(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
        let Some(host) = view.hosts.get(id) else {
            self.go(Screen::Hosts);
            return;
        };
        let running = host.running_app();
        match &host.apps {
            Apps::NotLoaded | Apps::Loading => {
                ui.add_space(40.0);
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new(tr("Loading the library…")).color(ON_SURFACE_VARIANT));
                });
            }
            Apps::Failed(e) => {
                ui.label(RichText::new(tr("The library could not be loaded.")).font(semibold(text::CARD_TITLE)));
                ui.label(RichText::new(e).color(ERROR));
                if quiet_button(ui, "Try again").clicked() {
                    commands.push(Command::Refresh(id));
                }
            }
            Apps::Loaded(apps) => {
                if apps.is_empty() {
                    ui.label(RichText::new(tr("This PC lists no apps.")).color(ON_SURFACE_VARIANT));
                    return;
                }
                ui.label(RichText::new(trf("Game settings: right-click a game, press F2, or use the controller's {} button.", &[&crate::glyphs::Style::of(view.pads).face(crate::glyphs::Face::West)])).color(ON_SURFACE_VARIANT));
                ui.add_space(12.0);
                scroll(ui, "apps", |ui| {
                    let gap = 20.0;
                    let columns = ((ui.available_width() + gap) / (POSTER_W + gap)).floor().max(1.0) as usize;
                    for (row, chunk) in apps.chunks(columns).enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for (i, app) in chunk.iter().enumerate() {
                                let is_running = running.is_some_and(|r| app.matches_running_app(r, None));
                                let art = host.art.get(&app.id);
                                let response = poster(ui, &app.name, art, is_running);
                                let first = if running.is_some() { is_running } else { row == 0 && i == 0 };
                                self.focus_item(&response, &format!("app:{}", app.id), first);
                                if response.clicked() {
                                    if app.is_control_action() {
                                        self.dialog = Dialog::ConfirmAction { host: id, app: app.id, name: app.name.clone() };
                                        self.focus_dialog = true;
                                    } else if app.control != nightjar_core::http::AppControl::None {
                                        commands.push(Command::Launch { host: id, app: app.id, resume: false });
                                    } else if is_running {
                                        commands.push(Command::Launch {
                                            host: id,
                                            app: app.id,
                                            resume: true,
                                        });
                                    } else if running.is_some() && !host.confirms_replacement() {
                                        self.dialog = Dialog::ConfirmQuit {
                                            host: id,
                                            start: Some((app.id, app.name.clone())),
                                            running: running_name(host),
                                            resume: running_app_id(host),
                                            named: host.running_app(),
                                        };
                                        self.focus_dialog = true;
                                    } else {
                                        commands.push(Command::Launch {
                                            host: id,
                                            app: app.id,
                                            resume: false,
                                        });
                                    }
                                }
                                if !app.is_control_action() {
                                    response.context_menu(|ui| {
                                        if ui.button(tr("Game settings")).clicked() {
                                            commands.push(Command::AppSettings { host: id, app: app.clone() });
                                            ui.close();
                                        }
                                    });
                                    if response.has_focus() && ui.input(|i| i.key_pressed(Key::F2)) {
                                        commands.push(Command::AppSettings { host: id, app: app.clone() });
                                    }
                                }
                            }
                        });
                        ui.add_space(space::XS);
                    }
                    // A focused-but-unloaded first poster still gets focus next frame.
                    if self.focus_screen && running.is_some() {
                        self.focus_screen = false;
                    }
                });
            }
        }
    }

    pub(super) fn home(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        if view.hosts.list.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                egui::Image::from_texture((logo, vec2(72.0, 72.0))).ui(ui);
                ui.add_space(14.0);
                ui.label(RichText::new(tr("No PCs yet")).font(semibold(text::CARD_TITLE)));
                ui.label(RichText::new(tr("Pair Nightjar with a PC to see its games here.")).color(ON_SURFACE_VARIANT));
                ui.add_space(12.0);
                let r = primary_button(ui, "Open PCs");
                self.focus_item(&r, "open-hosts", true);
                if r.clicked() {
                    self.open(Screen::Hosts);
                }
                how_to_connect(ui, commands);
            });
            return;
        }
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let entries = console::merge_library(&sources);
        let playing = console::continue_playing(&sources);
        let recent = console::recently_played(&sources, &self.recents, &playing, 6);
        let starred = console::favourite_games(&sources, &self.favourites);
        let loading = view.hosts.list.iter().any(|h| {
            h.reach == Reach::Online && h.paired() && h.native_api() && h.native.library.is_none() && h.native.error.is_none()
        });
        scroll(ui, "home", |ui| {
            // A failed read stays on screen beside the last good data.
            for host in &view.hosts.list {
                if let Some(error) = &host.native.error {
                    ui.label(RichText::new(format!("{}: {}", host.display_name(), tr(error))).color(ERROR));
                }
            }
            let mut first = true;
            if !starred.is_empty() {
                section(ui, "Favourites");
                ui.label(RichText::new(tr("Delete (keyboard) or Y (gamepad) removes the focused favourite.")).font(semibold(text::CAPTION)).color(ON_SURFACE_VARIANT));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                    for (index, item) in starred.iter().enumerate() {
                        let line = if item.online { trf("On {}", &[&item.host_name]) } else { trf("{} is away", &[&item.host_name]) };
                        let response = continue_card(ui, &item.title, &line, false);
                        self.focus_item(&response, &format!("favourite:{}:{}", item.host, item.app_id), std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                        let star = item.favourite.clone();
                        if response.has_focus() && unstar_pressed(ui) {
                            commands.push(Command::ToggleFavourite(star.clone()));
                            // The focus moves to the next favourite (else the previous), not back to the top.
                            match console::neighbour(starred.len(), index) {
                                Some(near) => {
                                    self.restore_key = Some(format!("favourite:{}:{}", starred[near].host, starred[near].app_id));
                                }
                                None => self.focus_screen = true,
                            }
                        }
                        response.context_menu(|ui| {
                            if ui.button(tr("Remove from favourites")).clicked() {
                                commands.push(Command::ToggleFavourite(star));
                                ui.close();
                            }
                        });
                    }
                });
            }
            if !playing.is_empty() {
                section(ui, "Continue playing");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                    for item in &playing {
                        let (line, running) = match item.playing {
                            Playing::Running => (trf("Running on {}", &[&item.host_name]), true),
                            Playing::Suspended => (trf("Suspended on {}", &[&item.host_name]), false),
                        };
                        let response = continue_card(ui, &item.title, &line, running);
                        self.focus_item(&response, &format!("continue:{}:{}", item.host, item.app_id), std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                        favourite_menu(&response, &sources, &self.favourites, item.host, item.app_id, commands);
                    }
                });
            }
            if !recent.is_empty() {
                section(ui, "Recently played");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                    for item in &recent {
                        let line = if item.online { trf("On {}", &[&item.host_name]) } else { trf("{} is away", &[&item.host_name]) };
                        let response = continue_card(ui, &item.title, &line, false);
                        self.focus_item(&response, &format!("recent:{}:{}", item.host, item.app_id), std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                        favourite_menu(&response, &sources, &self.favourites, item.host, item.app_id, commands);
                    }
                });
            }
            section(ui, "Library");
            if entries.is_empty() {
                if loading {
                    ui.horizontal(|ui| {
                        spinner(ui, 10.0);
                        ui.label(RichText::new(tr("Loading the games…")).color(ON_SURFACE_VARIANT));
                    });
                } else {
                    ui.label(RichText::new(tr("No games to show yet. They appear here once a PC that shares its game library is online.")).color(ON_SURFACE_VARIANT));
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(20.0, space::XS);
                for entry in &entries {
                    let cover = entry.cover();
                    let art = cover
                        .and_then(|c| view.hosts.get(c.host).and_then(|h| h.art.get(&(c.app_id as i32))));
                    let response = poster(ui, &entry.title, art, entry.running());
                    if let Some(c) = cover {
                        request_cover(ui, view, c.host, c.app_id, &c.cover_version, &response, commands);
                    }
                    self.focus_item(&response, &format!("game:{}", entry.key), std::mem::take(&mut first));
                    if response.clicked() {
                        let best = entry.best();
                        self.open(Screen::Game { host: best.host, app: best.app_id });
                    }
                    favourite_menu(&response, &sources, &self.favourites, entry.best().host, entry.best().app_id, commands);
                }
            });
            section(ui, "PCs");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                for (host, source) in view.hosts.list.iter().zip(&sources) {
                    // A PC to pair has no games to speak of yet: its state alone fits the card.
                    let line = if host.paired() { console::host_line(source).text() } else { String::new() };
                    let response = host_card(ui, host, Some(&line));
                    self.focus_item(&response, &format!("host:{}", host.id), std::mem::take(&mut first));
                    if response.clicked() {
                        host_clicked(host, self.settings.quick_resume, commands);
                    }
                }
            });
            ui.add_space(8.0);
        });
    }

    /// Leaves the current screen for the one it was opened from.
    pub(super) fn leave(&mut self) {
        let root = self.root();
        self.pop(root);
    }

    pub(super) fn game(&mut self, ui: &mut Ui, view: &View<'_>, host_id: u64, app_id: u32, commands: &mut Vec<Command>) {
        let Some(host) = view.hosts.get(host_id) else {
            self.leave();
            return;
        };
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let source = sources.iter().find(|s| s.id == host_id).expect("source of a listed host");
        let Some(game) = source.library.and_then(|l| l.games.iter().find(|g| g.app_id == app_id)) else {
            ui.label(RichText::new(tr("This game is no longer in the PC's library.")).font(semibold(text::CARD_TITLE)));
            let r = primary_button(ui, "Back");
            self.focus_item(&r, "back", true);
            if r.clicked() {
                self.leave();
            }
            return;
        };
        let detail = console::detail(source, app_id, host.native_api());
        let classic = match &host.apps {
            Apps::Loaded(apps) => {
                let pairs: Vec<(i32, &str)> = apps.iter().map(|a| (a.id, a.uuid.as_str())).collect();
                console::classic_app(&pairs, &game.install_id, app_id).and_then(|id| apps.iter().find(|a| a.id == id)).cloned()
            }
            _ => None,
        };
        let online = host.reach == Reach::Online;
        // The classic app list can know a run the native session does not.
        let classic_running = online
            && classic.as_ref().zip(host.running_app()).is_some_and(|(app, running)| app.matches_running_app(running, None));
        let resumes = detail.resumes() || classic_running;
        let other_running = if resumes {
            None
        } else {
            detail
                .other_running
                .clone()
                .or_else(|| if online { host.running_app().map(|_| running_name(host)) } else { None })
        };
        let merged = console::merge_library(&sources);
        let others: Vec<console::Copy> = merged
            .iter()
            .find(|e| e.copies.iter().any(|c| c.host == host_id && c.app_id == app_id))
            .map(|e| e.copies.iter().filter(|c| !(c.host == host_id && c.app_id == app_id)).cloned().collect())
            .unwrap_or_default();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 28.0;
            let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H), Sense::hover());
            let art = host.art.get(&(app_id as i32));
            // The state chip beside the title says it; a badge on the cover would say it twice.
            paint_cover(ui, rect, &game.title, art, false);
            request_cover(ui, view, host_id, app_id, &game.artwork.version, &response, commands);
            ui.vertical(|ui| {
                ui.set_max_width(640.0);
                match detail.state {
                    GameState::Running => state_chip(ui, "Running", OK),
                    GameState::Suspended => state_chip(ui, "Suspended", TERTIARY),
                    GameState::Idle => {}
                }
                ui.label(RichText::new(&game.title).font(semibold(text::TITLE)).color(ON_SURFACE));
                if let Some(store) = &game.store {
                    ui.label(RichText::new(&store.name).color(ON_SURFACE_VARIANT));
                }
                pc_line(ui, source.name);
                ui.add_space(8.0);
                if host.reach == Reach::Offline {
                    ui.label(RichText::new(tr("This PC is offline. Your saved library is still here. Reconnect to play.")).color(ERROR));
                } else if host.reach == Reach::Unknown {
                    ui.label(RichText::new(tr("Checking the PC…")).color(ON_SURFACE_VARIANT));
                } else if !source.paired {
                    ui.label(RichText::new(tr("Pair Nightjar with this PC to play.")).color(ERROR));
                }
                if let Some(other) = &other_running {
                    ui.label(RichText::new(trf("{} is running. Playing this game closes it.", &[&at_sentence_start(other)])).color(TERTIARY));
                }
                ui.add_space(8.0);
                let can_play = detail.can_start && classic.is_some();
                ui.horizontal(|ui| {
                    let r = ui.add_enabled_ui(can_play, |ui| primary_button(ui, if resumes { en("Resume") } else { en("Play") })).inner;
                    self.focus_item(&r, "play", true);
                    if r.clicked() {
                        if let Some(app) = &classic {
                            if resumes {
                                commands.push(Command::Launch { host: host_id, app: app.id, resume: true });
                            } else if let (Some(running), false) = (&other_running, host.confirms_replacement()) {
                                self.dialog = Dialog::ConfirmQuit {
                                    host: host_id,
                                    start: Some((app.id, game.title.clone())),
                                    running: running.clone(),
                                    resume: running_app_id(host),
                                    named: host.running_app(),
                                };
                                self.focus_dialog = true;
                            } else {
                                commands.push(Command::Launch { host: host_id, app: app.id, resume: false });
                            }
                        }
                    }
                    let star = console::favourite_of(source, game);
                    let starred = console::is_starred(source, game, &self.favourites);
                    let r = ui
                        .add_enabled_ui(star.is_some(), |ui| {
                            quiet_button(ui, if starred { en("Remove from favourites") } else { en("Add to favourites") })
                        })
                        .inner;
                    self.focus_item(&r, "favourite", false);
                    if r.clicked() {
                        if let Some(star) = star {
                            commands.push(Command::ToggleFavourite(star));
                        }
                    }
                    if detail.end != EndAction::None {
                        let closing = host.native.acting;
                        let label = if detail.end == EndAction::CloseGame { en("Close game") } else { en("Quit game") };
                        let r = ui.add_enabled_ui(!closing, |ui| quiet_button(ui, label)).inner;
                        self.focus_item(&r, "end", !can_play);
                        if r.clicked() {
                            self.dialog = if detail.end == EndAction::CloseGame {
                                Dialog::ConfirmClose { host: host_id, name: game.title.clone() }
                            } else {
                                Dialog::ConfirmQuit { host: host_id, start: None, running: game.title.clone(), resume: None, named: host.running_app() }
                            };
                            self.focus_dialog = true;
                        }
                        if closing {
                            spinner(ui, 9.0);
                        }
                    }
                });
                // Why Play is not there yet: the app list is what starts a stream.
                if detail.can_start && classic.is_none() {
                    match &host.apps {
                        Apps::NotLoaded | Apps::Loading => {
                            ui.horizontal(|ui| {
                                spinner(ui, 9.0);
                                ui.label(RichText::new(tr("Loading the app list…")).color(ON_SURFACE_VARIANT));
                            });
                        }
                        Apps::Failed(_) | Apps::Loaded(_) => {
                            let why = match &host.apps {
                                Apps::Failed(e) => trf("The app list could not be loaded. {}", &[&e]),
                                _ => tr("The PC's app list has no entry for this game.").to_owned(),
                            };
                            ui.label(RichText::new(why).color(ERROR));
                            let r = quiet_button(ui, "Try again");
                            self.focus_item(&r, "retry", true);
                            if r.clicked() {
                                commands.push(Command::Refresh(host_id));
                            }
                        }
                    }
                }
                if let Some(app) = &classic {
                    self.leave_and_input_only(ui, host, app, can_play, resumes, commands);
                }
                ui.add_space(14.0);
                let kind = self.network.kind(&self.profiles.home_wifi);
                let mut settings = host.record.settings(&self.profiles.settings(&self.settings, kind));
                if let Some(overrides) = classic.as_ref().and_then(|a| host.record.app_settings(&a.uuid, a.id)) {
                    settings = overrides.apply(&settings);
                }
                let (w, h, fps, kbps) = stream_mode(&settings, self.display);
                let profile = self
                    .profiles
                    .selected(kind)
                    .and_then(|id| self.profiles.entries.iter().find(|p| p.id == id))
                    .map_or(tr("Main settings"), |p| p.name.as_str());
                row(ui, "Stream settings", &format!("{}  \u{b7}  {profile}", console::stream_summary(w, h, fps, kbps)), |ui| {
                    match &classic {
                        Some(app) => {
                            let r = quiet_button(ui, "Game settings");
                            self.focus_item(&r, "settings", false);
                            if r.clicked() {
                                commands.push(Command::AppSettings { host: host_id, app: app.clone() });
                            }
                            r
                        }
                        None => ui.add_enabled(false, egui::Button::new(tr("Game settings"))),
                    }
                });
                if !others.is_empty() {
                    section(ui, "Also on");
                    ui.horizontal_wrapped(|ui| {
                        for copy in &others {
                            let r = quiet_button(ui, &copy.host_name);
                            self.focus_item(&r, &format!("copy:{}", copy.host), false);
                            if r.clicked() {
                                self.go(Screen::Game { host: copy.host, app: copy.app_id });
                            }
                        }
                    });
                }
            });
        });
    }

    /// A game's quit on leave switch and, on a PC without its own mode, Input-only through a
    /// small stream: Android's game menu entries.
    pub(super) fn leave_and_input_only(&mut self, ui: &mut Ui, host: &Host, app: &nightjar_core::http::App, can_play: bool, running: bool, commands: &mut Vec<Command>) {
        use nightjar_policy::launch::{self, InputOnlyBlocker};
        if launch::offers_quit_on_leave(host.paired(), policy_control(app.control), policy_role(app.role(false))) {
            let mut on = host.record.quits_on_leave(&app.uuid, app.id);
            // A switch in a row like every other setting, its effect said under it.
            let note = if on { en("On: Disconnect also quits it on the PC") } else { en("Off: Disconnect leaves it running on the PC") };
            // As far below the buttons as the stream settings card sits below it.
            ui.add_space(space::S);
            let r = row(ui, "Quit the game when I leave the stream", note, |ui| toggle(ui, &mut on));
            self.focus_item(&r, "quit-on-leave", false);
            if r.changed() {
                commands.push(Command::SetQuitOnLeave { host: host.id, uuid: app.uuid.clone(), id: app.id, enabled: on });
            }
        }
        let Some(blocker) = small_stream_offer(host, app, running, self.settings.input_only_any_host) else { return };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ui.add_enabled_ui(blocker.is_none() && can_play, |ui| quiet_button(ui, "Input-only")).inner;
            self.focus_item(&r, "input-only", false);
            if r.clicked() {
                commands.push(Command::LaunchInputOnly { host: host.id, app: app.id, resume: running });
            }
            let note = match blocker {
                Some(InputOnlyBlocker::Pair) => tr("Pair with this PC first").to_owned(),
                Some(InputOnlyBlocker::ViewPermission) => tr("Allow viewing streams for this device on the host").to_owned(),
                Some(InputOnlyBlocker::InputPermission) => tr("Allow input for this device on the host").to_owned(),
                None => trf("Video still streams at the smallest size, about {} Mbit/s ({} MB an hour), but isn’t shown", &[
                    &format!("{:.1}", f64::from(launch::estimated_kbps()) / 1000.0),
                    &launch::estimated_megabytes_per_hour().to_string(),
                ]),
            };
            ui.label(RichText::new(note).color(if blocker.is_some() { ERROR } else { ON_SURFACE_VARIANT }));
        });
    }
}

/// Quick resume: a paired PC running a game from its known library goes straight back into it.
pub(super) fn host_clicked(host: &Host, quick_resume: bool, commands: &mut Vec<Command>) {
    let running = running_entry(host).filter(|_| host.paired());
    if let Some(app) = running.filter(|_| nightjar_policy::launch::resumes_on_pc_tap(quick_resume, true)) {
        commands.push(Command::Launch { host: host.id, app: app.id, resume: true });
    } else if host.paired() {
        commands.push(Command::OpenApps(host.id));
    } else if host.reach == Reach::Offline {
        commands.push(Command::Refresh(host.id));
    } else {
        commands.push(Command::Pair(host.id));
    }
}

/// Whether a game offers Input-only through a small stream, and what blocks it now: only an
/// ordinary game on a PC without its own Input-only mode, with the setting on, that leaves the
/// PC's running game alone.
pub(super) fn small_stream_offer(host: &Host, app: &nightjar_core::http::App, running: bool, any_host: bool) -> Option<Option<nightjar_policy::launch::InputOnlyBlocker>> {
    use nightjar_policy::launch::{self, InputOnlyMode};
    let lists_input_only = matches!(&host.apps, Apps::Loaded(apps) if apps.iter().any(|a| a.role(false) == nightjar_core::http::AppRole::InputOnly));
    let tiles = host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::RemoteSessionTiles));
    let offered = app.control == nightjar_core::http::AppControl::None
        && launch::keeps_running_game(host.running_app().is_some(), running)
        && launch::input_only_mode(any_host, tiles, lists_input_only) == InputOnlyMode::SmallStream;
    let permission = host.info.as_ref().and_then(|i| i.permission).map(u64::from);
    offered.then(|| launch::small_stream_blocker(host.paired(), permission))
}

/// The launch policy's names for an app's role and control tile.
pub fn policy_role(role: nightjar_core::http::AppRole) -> nightjar_policy::launch::Role {
    use nightjar_core::http::AppRole;
    use nightjar_policy::launch::Role;
    match role {
        AppRole::Stream => Role::Stream,
        AppRole::RemoteMonitor => Role::RemoteMonitor,
        AppRole::InputOnly => Role::InputOnly,
    }
}

pub(super) fn policy_control(control: nightjar_core::http::AppControl) -> nightjar_policy::launch::Control {
    use nightjar_core::http::AppControl;
    use nightjar_policy::launch::Control;
    match control {
        AppControl::None => Control::None,
        AppControl::Resume => Control::Resume,
        AppControl::DisconnectMonitor => Control::DisconnectMonitor,
        AppControl::DisconnectInput => Control::DisconnectInput,
        AppControl::Terminate => Control::Terminate,
        AppControl::Monitor => Control::Monitor,
        AppControl::Input => Control::Input,
        AppControl::RunningGame => Control::RunningGame,
    }
}

/// The library entry of the app the PC is running, when its library is known.
pub(super) fn running_entry(host: &Host) -> Option<&nightjar_core::http::App> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None) && !a.is_control_action()),
        _ => None,
    }
}

/// A name that starts a sentence: the stand-in "the running app" gets its capital, a game's own
/// name stays as it is spelled.
pub(super) fn at_sentence_start(name: &str) -> String {
    if name != tr("the running app") {
        return name.to_owned();
    }
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

pub fn running_name(host: &Host) -> String {
    let Some(running) = host.running_app() else {
        return tr("the running app").into();
    };
    if let Apps::Loaded(apps) = &host.apps {
        if let Some(app) = apps.iter().find(|a| a.matches_running_app(running, None)) {
            return app.name.clone();
        }
    }
    tr("the running app").into()
}

/// The classic app id of the app the PC runs now, once the app list is loaded.
pub(super) fn running_app_id(host: &Host) -> Option<i32> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None)).map(|a| a.id),
        _ => None,
    }
}
