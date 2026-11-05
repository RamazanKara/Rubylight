//! Dialogs and overlays: their drawing and buttons, a PC's options list, pairing and sleep
//! answers, and the waking and connecting cards.

use super::*;
use nightjar_core::http::{valid_pairing_pin, PairingRequest};

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
    RestartPc,
    ShutDownPc,
    UpdateHost,
    PairAnother,
    Files,
    Settings,
    Details,
    Rename,
    Unpair,
    PairingHelp,
    CheckAgain,
    Remove,
    MicOnly,
}

impl HostOption {
    pub fn icon(self) -> Icon {
        match self {
            Self::OpenLibrary => Icon::Grid,
            Self::Resume => Icon::Play,
            Self::QuitApp => Icon::Stop,
            Self::Pair => Icon::Link,
            Self::PairOtp => Icon::Key,
            Self::Wake => Icon::Power,
            Self::Sleep => Icon::Moon,
            Self::RestartPc => Icon::Refresh,
            Self::ShutDownPc => Icon::Power,
            Self::UpdateHost => Icon::Gear,
            Self::PairAnother => Icon::Key,
            Self::Files => Icon::Folder,
            Self::Settings => Icon::Sliders,
            Self::Details => Icon::Info,
            Self::Rename => Icon::Pencil,
            Self::Unpair => Icon::Unlink,
            Self::PairingHelp => Icon::Help,
            Self::CheckAgain => Icon::Refresh,
            Self::Remove => Icon::Trash,
            Self::MicOnly => Icon::Mic,
        }
    }

    /// Unpair and Remove undo the pairing or the PC itself: they sit last, apart, in red.
    pub fn destructive(self) -> bool {
        matches!(self, Self::Unpair | Self::Remove)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenLibrary => en("Open library"),
            Self::Resume => en("Resume"),
            Self::QuitApp => en("Quit running app"),
            Self::Pair => en("Pair"),
            Self::PairOtp => en("Pair with one-time PIN"),
            Self::Wake => en("Wake up"),
            Self::Sleep => en("Sleep PC"),
            Self::RestartPc => en("Restart PC"),
            Self::ShutDownPc => en("Shut down PC"),
            Self::UpdateHost => en("Update the PC's host"),
            Self::PairAnother => en("Pair another device"),
            Self::Files => en("Files"),
            Self::Settings => en("PC settings"),
            Self::Details => en("View details"),
            Self::Rename => en("Rename"),
            Self::Unpair => en("Unpair"),
            Self::PairingHelp => en("Pairing help"),
            Self::CheckAgain => en("Check again"),
            Self::Remove => en("Remove PC"),
            Self::MicOnly => en("Microphone only"),
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
    if host.pc_power_option(PcPower::Restart).is_some() {
        options.push(HostOption::RestartPc);
    }
    // Not allowed for this device: Restart PC alone is listed off, its reason naming both.
    if host.pc_power_option(PcPower::ShutDown).is_some_and(|o| o.is_ok() || host.pc_power_option(PcPower::Restart).is_none()) {
        options.push(HostOption::ShutDownPc);
    }
    if host.update_offer().is_some() {
        options.push(HostOption::UpdateHost);
    }
    if host.pair_another_option().is_some() {
        options.push(HostOption::PairAnother);
    }
    if files::offers_files(host) {
        options.push(HostOption::Files);
    }
    if offers_mic_only(host) {
        options.push(HostOption::MicOnly);
    }
    options.push(HostOption::Settings);
    options.push(HostOption::Rename);
    options.push(HostOption::Details);
    if !host.paired() {
        options.push(HostOption::PairingHelp);
    }
    options.push(HostOption::CheckAgain);
    // Unpairing asks the PC to forget this device, so it needs the PC to be reachable.
    if host.paired() && host.reach == Reach::Online {
        options.push(HostOption::Unpair);
    }
    options.push(HostOption::Remove);
    options
}

/// Microphone only on a paired, online PC whose native capabilities offer it to this device.
pub fn offers_mic_only(host: &Host) -> bool {
    host.paired()
        && host.reach == Reach::Online
        && host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::MicOnly))
}

/// The option that selecting the PC's card does ([`host_clicked`]): its row shows Enter.
pub fn enter_option(host: &Host, quick_resume: bool) -> HostOption {
    if running_entry(host).is_some() && host.paired() && nightjar_policy::launch::resumes_on_pc_tap(quick_resume, true) {
        HostOption::Resume
    } else if host.paired() {
        HostOption::OpenLibrary
    } else if host.reach == Reach::Offline {
        HostOption::CheckAgain
    } else {
        HostOption::Pair
    }
}

/// Which game a game menu is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameTarget {
    /// A game of a PC's native library (Home, All games): `app` is its library id.
    /// `favourite` when opened on the Favourites row, where Delete takes the star off.
    Library { app: u32, favourite: bool },
    /// An entry of a PC's classic app list (its library page): `app` is the app id.
    Classic { app: i32 },
}

/// What a game menu offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameAction {
    /// The game's page (what selecting a library cover does).
    Open,
    Play,
    Resume,
    Star,
    Unstar,
    /// Off Home and All games on this device.
    Hide,
    Unhide,
    Settings,
    /// The PC's own library page.
    Library,
    /// Make this game the one "When Nightjar opens" starts.
    OpenOnStart,
    /// It is that game: back to the PCs.
    StopOpenOnStart,
}

impl GameAction {
    pub fn row(self, enter: bool, delete: bool) -> MenuRow {
        let (icon, label) = match self {
            Self::Open => (Icon::Info, en("Game details")),
            Self::Play => (Icon::Play, en("Play")),
            Self::Resume => (Icon::Play, en("Resume")),
            Self::Star => (Icon::Star, en("Add to favourites")),
            Self::Unstar => (Icon::StarOff, en("Remove from favourites")),
            Self::Hide => (Icon::EyeOff, en("Hide game")),
            Self::Unhide => (Icon::Eye, en("Show game")),
            Self::Settings => (Icon::Sliders, en("Game settings")),
            Self::Library => (Icon::Grid, en("Open library")),
            Self::OpenOnStart => (Icon::Play, en("Open this when Nightjar starts")),
            Self::StopOpenOnStart => (Icon::Stop, en("Stop opening this when Nightjar starts")),
        };
        let hint = if enter {
            Some(en("Enter"))
        } else if delete {
            Some(en("Del"))
        } else {
            None
        };
        MenuRow { icon, label, hint, destructive: false, blocked: None }
    }
}

/// A caller without List or View is told that its stream moved, not where to.
pub(super) fn now_playing_title(device: &str) -> String {
    if device.is_empty() {
        tr("Now playing on another device").into()
    } else {
        trf("Now playing on {}", &[&device])
    }
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
        // The destination ConfirmLeave goes to once it is closed.
        let mut leave: Option<Screen> = None;
        let focus_first = self.focus_dialog;
        let mut focused = false;
        match &mut self.dialog {
            Dialog::None => {
                if self.next_queued() {
                    ctx.request_repaint();
                }
                return;
            }
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
                    let (status, color, _) = host_status(host);
                    // Wake needs the PC's address, learnt while pairing with it on.
                    let note = (host.reach == Reach::Offline && host.record.mac.is_none()).then_some(en("Pair once while the PC is on to enable Wake."));
                    let (outcome, took) = popover(ctx, "host-options", self.menu_anchor, host.display_name(), Some((status, *color)), &rows, note, focus_first);
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
                let (entries, _) = console::split_hidden(console::merge_library(&sources), &sources, &self.hidden_games);
                let groups = console::offered_groups(&entries);
                let group = console::shown_group(s.home_group, &entries);
                let mut rows = vec![
                    MenuRow { icon: Icon::Grid, label: en("Layout"), hint: Some(s.home_layout.label()), destructive: false, blocked: None },
                    MenuRow { icon: Icon::Folder, label: en("Group by"), hint: Some(group.label()), destructive: false, blocked: None },
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
                egui::Modal::new(Id::unique("confirm-link"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
            Dialog::ConfirmLeave { to } => {
                let to = *to;
                egui::Modal::new(Id::unique("confirm-leave"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
                egui::Modal::new(Id::unique("open-play"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
                egui::Modal::new(Id::unique("confirm-exit"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(if self.power.is_empty() { 440.0 } else { 640.0 });
                        ui.label(RichText::new(tr("Close Nightjar?")).font(semibold(text::SECTION)));
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Close Nightjar").clicked() {
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
                            // Focus starts on Cancel: a second B or a stray A never closes the app.
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
            Dialog::ConfirmPower(offer) => {
                let offer = *offer;
                let action = offer.action;
                egui::Modal::new(Id::unique("confirm-power"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
            Dialog::ConfirmPcPower { host, ask, name, action, offer } => {
                let (id, ask, action) = (*host, *ask, *action);
                egui::Modal::new(Id::unique("confirm-pc-power"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        let title = match action {
                            PcPower::Restart => trf("Restart {}?", &[&name]),
                            PcPower::ShutDown => trf("Shut down {}?", &[&name]),
                        };
                        ui.label(RichText::new(title).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("Apps with unsaved work on the PC can block this.")).color(ON_SURFACE_VARIANT));
                        // As Sleep PC: the asking line keeps its room and the action button is there
                        // from the start, only off, so nothing moves under the pointer or the focus.
                        let waiting = offer.is_none();
                        let no_wake = action == PcPower::ShutDown && matches!(offer, Some((_, _, false)));
                        ui.add_space(space::M);
                        ui.horizontal(|ui| {
                            if waiting {
                                spinner(ui, 9.0);
                            } else {
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            }
                            let line = if no_wake { tr("Once off, this PC may only start with its power button.") } else { tr("Asking the PC…") };
                            let color = if no_wake { ERROR } else { ON_SURFACE_VARIANT };
                            ui.add_visible(waiting || no_wake, egui::Label::new(RichText::new(line).color(color)));
                        });
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
                        });
                    });
            }
            Dialog::ConfirmUpdate { host, name, version } => {
                let id = *host;
                egui::Modal::new(Id::unique("confirm-update"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Update the host on {} to {}?", &[&name, &version])).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("The host restarts to install it, and a stream from this device ends.")).color(ON_SURFACE_VARIANT));
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
                egui::Modal::new(Id::unique("pair-requests"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
                egui::Modal::new(Id::unique("approve-pairing"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(380.0);
                        ui.label(RichText::new(trf("{} wants to pair", &[&name])).font(semibold(text::SECTION)));
                        // The one part of the request the device can't choose.
                        ui.label(RichText::new(trf("From {0}, for {1}", &[&address, &pc])).font(semibold(text::BODY)));
                        ui.add_space(space::S);
                        let shown = if *busy { trf("Waiting for {}…", &[&name]) } else { pin.clone() };
                        ui.label(RichText::new(shown).font(semibold(text::SECTION)));
                        ui.add_space(space::S);
                        let mut filled = false;
                        ui.add_enabled_ui(!*busy, |ui| {
                            filled = pin_pad(ui, pin, focus_first, &mut focused);
                        });
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
                egui::Modal::new(Id::unique("stream-hotkey"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
            Dialog::ContinueHere { host, app, resume, name, device } => {
                let (host, app, resume) = (*host, *app, *resume);
                egui::Modal::new(Id::unique("continue-here"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
                            if quiet_button(ui, "Stream here too").clicked() {
                                commands.push(Command::StreamHereToo { host, app, resume });
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() { close = true; }
                        });
                    });
            }
            Dialog::NowPlaying { host, app, device } => {
                let (host, app) = (*host, *app);
                egui::Modal::new(Id::unique("now-playing"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
                egui::Modal::new(Id::unique("game-came-back"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
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
            Dialog::KeptSettings { pcs } => {
                egui::Modal::new(Id::unique("kept-settings"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(tr("Some PCs and games keep their own settings")).font(semibold(text::SECTION)));
                        ui.label(RichText::new(tr("Older versions could keep a setting for a whole PC when it was only opened.")).color(ON_SURFACE_VARIANT));
                        ui.add_space(space::S);
                        for pc in pcs.iter() {
                            ui.label(pc.line());
                        }
                        dialog_buttons(ui, |ui| {
                            if primary_button(ui, "Reset all").clicked() {
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
        if let Some(to) = leave {
            self.leave_for(to, commands);
        }
    }

    /// Whether the stream hotkey dialog is waiting for keys.
    pub fn recording_hotkey(&self) -> bool {
        matches!(self.dialog, Dialog::StreamHotkey { .. })
    }

    /// A key pressed while the stream hotkey dialog records, straight from SDL: before the
    /// window's own F11 and F12, and for every key, where egui has no event for F3 to F9, Shift
    /// with a digit, most punctuation or the numpad. Escape alone cancels, a modifier alone waits,
    /// an arrow or Enter alone moves in the dialog ([`moves_in_hotkey_dialog`]), another key alone
    /// gets Ctrl+Alt+Shift; anything else is saved or its refusal shown. Returns the
    /// command to save the change.
    pub fn hotkey_key_down(&mut self, scancode: sdl3_sys::everything::SDL_Scancode, mods: sdl3_sys::everything::SDL_Keymod, repeat: bool) -> Option<Command> {
        use sdl3_sys::everything::*;
        let Dialog::StreamHotkey { id, error } = &mut self.dialog else { return None };
        if repeat {
            return None;
        }
        let held = SDL_KMOD_CTRL.0 | SDL_KMOD_ALT.0 | SDL_KMOD_SHIFT.0 | SDL_KMOD_GUI.0;
        if scancode == SDL_SCANCODE_ESCAPE && mods.0 & held == 0 {
            self.close_dialog();
            return None;
        }
        if moves_in_hotkey_dialog(scancode, mods) {
            return None;
        }
        let keys = match crate::shortcuts::recorded_keys(scancode, mods) {
            Some(keys) => keys,
            // Ctrl, Shift, Alt, Windows on either side, and AltGr where SDL names it apart.
            None if (SDL_SCANCODE_LCTRL.0..=SDL_SCANCODE_RGUI.0).contains(&scancode.0) || scancode == SDL_SCANCODE_MODE => return None,
            None => {
                // SAFETY: SDL's name table, valid for any scancode.
                let name = unsafe { std::ffi::CStr::from_ptr(SDL_GetScancodeName(scancode)) }.to_string_lossy().into_owned();
                *error = Some(trf("Unsupported key: {}", &[&name]));
                return None;
            }
        };
        match crate::shortcuts::record_hotkey(id, &keys) {
            Ok(()) => {
                self.close_dialog();
                Some(Command::SaveHotkeys)
            }
            Err(message) => {
                *error = Some(message);
                None
            }
        }
    }

    /// Closes the dialog. Those opened from a PC's card give focus back to it, the rest to the
    /// screen's first item.
    pub(super) fn close_dialog(&mut self) {
        let from_card = matches!(
            self.dialog,
            Dialog::HostOptions { .. }
                | Dialog::GameMenu { .. }
                | Dialog::Rename { .. }
                | Dialog::ConfirmRemove { .. }
                | Dialog::ConfirmUnpair { .. }
                | Dialog::ConfirmSleep { .. }
                | Dialog::ConfirmPcPower { .. }
                | Dialog::ConfirmUpdate { .. }
                | Dialog::PairRequests { .. }
                | Dialog::ApprovePairing { .. }
                | Dialog::PairError { .. }
                | Dialog::PairingHelp
                | Dialog::HomeView
                | Dialog::ConfirmExit
                | Dialog::ConfirmPower(_)
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
            HostOption::RestartPc | HostOption::ShutDownPc => {
                let action = if option == HostOption::RestartPc { PcPower::Restart } else { PcPower::ShutDown };
                if host.pc_power_option(action) == Some(Ok(())) {
                    self.sleep_asks += 1;
                    let ask = self.sleep_asks;
                    commands.push(Command::PcPowerOffer { host: id, ask, action });
                    return Some(Dialog::ConfirmPcPower { host: id, ask, name, action, offer: None });
                }
            }
            HostOption::UpdateHost => {
                if let Some(version) = host.update_offer() {
                    return Some(Dialog::ConfirmUpdate { host: id, name, version: version.to_owned() });
                }
            }
            HostOption::PairAnother => return Some(Dialog::PairRequests { only: Some(id) }),
            HostOption::Files => commands.push(Command::OpenFiles(id)),
            HostOption::Settings => commands.push(Command::HostSettings(id)),
            HostOption::Details => self.open(Screen::HostDetails(id)),
            HostOption::Rename => return Some(Dialog::Rename { host: id, text: host.rename_prefill().to_owned() }),
            HostOption::Unpair => return Some(Dialog::ConfirmUnpair { host: id, name }),
            HostOption::PairingHelp => return Some(Dialog::PairingHelp),
            HostOption::CheckAgain => commands.push(Command::Refresh(id)),
            HostOption::Remove => return Some(Dialog::ConfirmRemove { host: id, name }),
            HostOption::MicOnly => commands.push(Command::MicOnly { host: id }),
        }
        None
    }

    /// Acts on an entry of a game's menu; a dialog to ask first is returned.
    pub(super) fn choose_game_action(&mut self, host: &Host, game: GameTarget, action: GameAction, commands: &mut Vec<Command>) -> Option<Dialog> {
        let id = host.id;
        match (action, game) {
            (GameAction::Open, GameTarget::Library { app, .. }) => self.open(Screen::Game { host: id, app }),
            (GameAction::Star | GameAction::Unstar, GameTarget::Library { app, .. }) => {
                let sources: Vec<Source> = std::iter::once(Source::of(host)).collect();
                // From the library, else the stored star of a favourite whose library is not known.
                let star = console::favourite_for(&sources, id, app).or_else(|| stored_favourite(host, app, &self.favourites).cloned());
                if let Some(star) = star {
                    commands.push(Command::ToggleFavourite(star));
                }
            }
            (GameAction::Library, _) => commands.push(Command::OpenApps(id)),
            (GameAction::OpenOnStart, _) => {
                if let Some(chosen) = open_game_of(host, game, &self.favourites) {
                    commands.push(Command::OpenOnStart(Some(chosen)));
                }
            }
            (GameAction::StopOpenOnStart, _) => commands.push(Command::OpenOnStart(None)),
            (GameAction::Settings, _) => {
                if let Some(app) = classic_of(host, game, &self.favourites) {
                    commands.push(Command::AppSettings { host: id, app: app.clone() });
                }
            }
            (GameAction::Play | GameAction::Resume, _) => {
                if let Some(app) = classic_of(host, game, &self.favourites) {
                    return self.start_app(host, app, commands);
                }
            }
            (GameAction::Open | GameAction::Star | GameAction::Unstar, GameTarget::Classic { .. }) => {}
            // The caller has every PC's library, which a title's copies take.
            (GameAction::Hide | GameAction::Unhide, _) => {}
        }
        None
    }

    /// Opens `dialog`, a menu, at `anchor`, with `key` (the card it belongs to) remembered so focus
    /// goes back to that card when it closes.
    pub(super) fn open_menu(&mut self, dialog: Dialog, anchor: Anchor, key: String) {
        self.focused_key = Some(key);
        self.menu_anchor = anchor;
        self.dialog = dialog;
        self.focus_dialog = true;
    }

    /// Once per install: lists the PCs that keep settings of their own or for games, which
    /// before 4.1.1 an A press on an inherited row could pin without a choice. Waits while
    /// another dialog is open. True when `settings.kept_settings_checked` changed and needs saving.
    pub fn show_kept_settings_once(&mut self, hosts: &[Host]) -> bool {
        if self.settings.kept_settings_checked || !matches!(self.dialog, Dialog::None) {
            return false;
        }
        self.settings.kept_settings_checked = true;
        // Words come when drawn: at start the language setting isn't applied yet.
        let pcs: Vec<KeptPc> = hosts.iter().filter_map(|host| {
            let (own, games) = host.record.own_settings();
            (own || games > 0).then(|| KeptPc { name: host.display_name().to_owned(), own, games })
        }).collect();
        if !pcs.is_empty() {
            self.dialog = Dialog::KeptSettings { pcs };
            self.focus_dialog = true;
        }
        true
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

    /// A Restart PC or Shut down PC step came back; matched and shown as [`Self::sleep_answered`].
    pub fn pc_power_answered(&mut self, host: u64, ask: u64, name: &str, action: PcPower, confirmed: bool, result: Result<PcPowerStep, String>) {
        let waiting = matches!(&self.dialog, Dialog::ConfirmPcPower { host: h, ask: a, offer: None, .. } if *h == host && *a == ask);
        if !confirmed && !waiting {
            return;
        }
        match (result, action) {
            (Ok(PcPowerStep::Offer { token, at, wake_from_off }), _) => {
                if let Dialog::ConfirmPcPower { offer, .. } = &mut self.dialog {
                    *offer = Some((token, at, wake_from_off));
                    self.focus_dialog = true;
                }
            }
            (Ok(PcPowerStep::Accepted), PcPower::Restart) => {
                self.sleep_message(host, trf("{} is restarting", &[&name]), tr("The PC took the request. It shows as online again once it is back.").to_owned())
            }
            (Ok(PcPowerStep::Accepted), PcPower::ShutDown) => {
                self.sleep_message(host, trf("{} is shutting down", &[&name]), tr("The PC took the request. It shows as offline once it is off.").to_owned())
            }
            // The PC may still go down: no failure's title.
            (Ok(PcPowerStep::Unconfirmed(said)), PcPower::Restart) => self.sleep_message(host, trf("{} may still restart", &[&name]), said),
            (Ok(PcPowerStep::Unconfirmed(said)), PcPower::ShutDown) => self.sleep_message(host, trf("{} may still shut down", &[&name]), said),
            (Err(e), PcPower::Restart) => self.sleep_message(host, trf("Could not restart {}", &[&name]), e),
            (Err(e), PcPower::ShutDown) => self.sleep_message(host, trf("Could not shut down {}", &[&name]), e),
        }
    }

    /// The PC's host update: started, waiting for another device's stream, back on the new
    /// version, or why not.
    /// What the PC said to an approval or a denial. A refusal is in its words, in a dialog; one
    /// that lands after the dialog was left still gets said.
    pub fn pairing_answered(&mut self, host: u64, request: &str, name: &str, denied: bool, result: Result<(), String>) {
        let asked = matches!(&self.dialog, Dialog::ApprovePairing { host: h, request: r, .. } if *h == host && r == request);
        if asked {
            self.close_dialog();
        }
        match result {
            Ok(()) if denied => {}
            Ok(()) => self.toast(trf("{} is paired", &[&name])),
            Err(reason) => self.message(trf("{} wants to pair", &[&name]), reason),
        }
    }

    pub fn update_answered(&mut self, host: u64, name: &str, result: Result<UpdateStep, String>) {
        let (title, body) = match result {
            Ok(UpdateStep::Started(version)) => {
                (trf("Updating {}", &[&name]), trf("The host installs version {} and comes back on its own.", &[&version]))
            }
            Ok(UpdateStep::Waiting(reason)) => (trf("Updating {}", &[&name]), trf("{} The update starts once that stream ends.", &[&reason])),
            Ok(UpdateStep::Back(version)) => (trf("{} is back", &[&name]), trf("The host now runs version {}.", &[&version])),
            Err(e) => (trf("Could not update {}", &[&name]), e),
        };
        self.sleep_message(host, title, body);
    }

    /// Shows a Sleep PC answer now when it replaces nothing but its own dialog, else after the
    /// dialog the user is in.
    pub(super) fn sleep_message(&mut self, host: u64, title: String, body: String) {
        match &self.dialog {
            Dialog::None => self.message(title, body),
            Dialog::ConfirmSleep { host: asked, .. } | Dialog::ConfirmPcPower { host: asked, .. } if *asked == host => self.message(title, body),
            _ => self.queued.push_back(Dialog::Message { title, body }),
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

/// Whether a key pressed in the stream hotkey dialog moves in it or confirms instead of being
/// recorded: an arrow or Enter with no modifier held, as Android's `movesInDialog`. With a
/// modifier they are recorded. Tab alone is recorded on both (the stream menu's own key).
pub fn moves_in_hotkey_dialog(scancode: sdl3_sys::everything::SDL_Scancode, mods: sdl3_sys::everything::SDL_Keymod) -> bool {
    use sdl3_sys::everything::*;
    let held = SDL_KMOD_CTRL.0 | SDL_KMOD_ALT.0 | SDL_KMOD_SHIFT.0 | SDL_KMOD_GUI.0;
    mods.0 & held == 0
        && matches!(scancode, SDL_SCANCODE_UP | SDL_SCANCODE_DOWN | SDL_SCANCODE_LEFT | SDL_SCANCODE_RIGHT | SDL_SCANCODE_RETURN | SDL_SCANCODE_KP_ENTER)
}

/// A game's menu: its title and what it offers, the action selecting it does first. None when the
/// game is gone from the PC. A favourite whose PC's library is not known (away with nothing saved,
/// not read yet, or the game gone) still has its menu, from the stored star: its card is on Home.
pub(super) fn game_menu(host: &Host, view: &View<'_>, game: GameTarget, favourites: &[crate::config::Favourite], hidden: &[crate::config::Favourite]) -> Option<(String, Vec<GameAction>)> {
    let _ = view;
    match game {
        GameTarget::Library { app, favourite } => {
            let listed = host.native.library.as_ref().and_then(|l| l.games.iter().find(|g| g.app_id == app));
            let (title, starred) = match listed {
                Some(entry) => {
                    let sources: Vec<Source> = std::iter::once(Source::of(host)).collect();
                    let star = console::favourite_for(&sources, host.id, app);
                    (entry.title.clone(), star.map(|star| favourites.iter().any(|f| f.same_game(&star))))
                }
                None if favourite => (stored_favourite(host, app, favourites)?.title.clone(), Some(true)),
                None => return None,
            };
            let mut actions = vec![GameAction::Open];
            if let Some(starred) = starred {
                actions.push(if starred { GameAction::Unstar } else { GameAction::Star });
            }
            if let Some(entry) = listed.filter(|_| !host.record.unique_id.is_empty()) {
                let hides = console::is_hidden(&Source::of(host), &entry.install_id, app, hidden);
                actions.push(if hides { GameAction::Unhide } else { GameAction::Hide });
            }
            if classic_of(host, game, favourites).is_some() {
                actions.push(GameAction::Settings);
            }
            if host.paired() {
                actions.push(GameAction::Library);
            }
            Some((title, actions))
        }
        GameTarget::Classic { app } => {
            let Apps::Loaded(apps) = &host.apps else { return None };
            let entry = apps.iter().find(|a| a.id == app)?;
            let running = host.running_app().is_some_and(|r| entry.matches_running_app(r, None));
            let mut actions = vec![if running { GameAction::Resume } else { GameAction::Play }];
            if !entry.is_control_action() {
                actions.push(GameAction::Settings);
                if !host.record.unique_id.is_empty() {
                    let hides = console::is_hidden(&Source::of(host), &entry.uuid, entry.id as u32, hidden);
                    actions.push(if hides { GameAction::Unhide } else { GameAction::Hide });
                }
            }
            Some((entry.name.clone(), actions))
        }
    }
}

/// The game a menu is about, as "When Nightjar opens" keeps it; None for what is not a game (a
/// control action) or a PC without a unique id to find it by again.
pub(super) fn open_game_of(host: &Host, game: GameTarget, favourites: &[crate::config::Favourite]) -> Option<crate::config::OpenGame> {
    let (install_id, app_id, title) = match game {
        GameTarget::Library { app, .. } => match host.native.library.as_ref().and_then(|l| l.games.iter().find(|g| g.app_id == app)) {
            Some(entry) => (entry.install_id.clone(), app, entry.title.clone()),
            None => {
                let star = stored_favourite(host, app, favourites)?;
                (star.install_id.clone(), app, star.title.clone())
            }
        },
        GameTarget::Classic { app } => {
            let Apps::Loaded(apps) = &host.apps else { return None };
            let entry = apps.iter().find(|a| a.id == app).filter(|a| !a.is_control_action())?;
            (entry.uuid.clone(), entry.id as u32, entry.name.clone())
        }
    };
    let unique_id = host.record.unique_id.clone();
    (!unique_id.is_empty()).then_some(crate::config::OpenGame { host: unique_id, install_id, app_id, title })
}

/// The game menu's row for "When Nightjar opens": take it back on the chosen game, else choose it.
pub(super) fn open_on_start_action(host: &Host, game: GameTarget, favourites: &[crate::config::Favourite], chosen: Option<&crate::config::OpenGame>) -> Option<GameAction> {
    let this = open_game_of(host, game, favourites)?;
    Some(if chosen.is_some_and(|c| c.same_game(&this)) { GameAction::StopOpenOnStart } else { GameAction::OpenOnStart })
}

/// What hiding a game menu's game stores: every copy of a library title, or the one classic entry.
fn hide_keys_of(host: &Host, game: GameTarget, sources: &[Source<'_>]) -> Vec<crate::config::Favourite> {
    match game {
        GameTarget::Library { app, .. } => console::hide_keys(sources, host.id, app),
        GameTarget::Classic { app } => {
            let Apps::Loaded(apps) = &host.apps else { return Vec::new() };
            apps.iter()
                .find(|a| a.id == app)
                .map(|a| crate::config::Favourite { host: host.record.unique_id.clone(), install_id: a.uuid.clone(), app_id: a.id as u32, title: a.name.clone() })
                .into_iter()
                .collect()
        }
    }
}

/// The classic app entry a game menu's game starts through, once the PC's app list is loaded. A
/// game its library does not list is found by its stored star, if it has one.
pub(super) fn classic_of<'a>(host: &'a Host, game: GameTarget, favourites: &[crate::config::Favourite]) -> Option<&'a nightjar_core::http::App> {
    let Apps::Loaded(apps) = &host.apps else { return None };
    match game {
        GameTarget::Classic { app } => apps.iter().find(|a| a.id == app),
        GameTarget::Library { app, .. } => {
            let install_id = match host.native.library.as_ref().and_then(|l| l.games.iter().find(|g| g.app_id == app)) {
                Some(entry) => entry.install_id.as_str(),
                None => stored_favourite(host, app, favourites)?.install_id.as_str(),
            };
            let pairs: Vec<(i32, &str)> = apps.iter().map(|a| (a.id, a.uuid.as_str())).collect();
            console::classic_app(&pairs, install_id, app).and_then(|id| apps.iter().find(|a| a.id == id))
        }
    }
}

/// The star stored for game `app` on `host`, matched as Home's Favourites row matches a star
/// whose library is not known: by the PC's unique id and the saved app id.
fn stored_favourite<'a>(host: &Host, app: u32, favourites: &'a [crate::config::Favourite]) -> Option<&'a crate::config::Favourite> {
    let unique_id = host.record.unique_id.as_str();
    favourites.iter().find(|f| !unique_id.is_empty() && f.host == unique_id && f.app_id == app)
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

/// The dialog for one waiting device.
pub(super) fn approve_dialog(host: &Host, request: &PairingRequest) -> Dialog {
    Dialog::ApprovePairing {
        host: host.id,
        request: request.id.clone(),
        name: short_device_name(&request.name),
        address: request.address.clone(),
        pc: host.display_name().to_owned(),
        pin: String::new(),
        busy: false,
    }
}

/// A device names itself, at any length; a row stays one row.
pub(super) fn short_device_name(name: &str) -> String {
    const MOST: usize = 32;
    if name.chars().count() <= MOST { name.to_owned() } else { name.chars().take(MOST).chain(['…']).collect() }
}

/// The digit pad of a PIN, for a controller or the mouse; the keyboard's digits work beside it.
/// True when this frame's digit filled the PIN.
fn pin_pad(ui: &mut Ui, pin: &mut String, focus_first: bool, focused: &mut bool) -> bool {
    const LENGTH: usize = 4;
    const KEY: Size = Size { height: Size::CONTROL.height, pad: 26.0, radius: Size::CONTROL.radius, font: Size::CONTROL.font };
    let before = pin.len();
    let enabled = ui.is_enabled();
    if enabled {
        let (typed, erase) = ui.input(|i| {
            let typed: String = i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
            (typed, i.key_pressed(Key::Backspace))
        });
        pin.extend(typed.chars().filter(char::is_ascii_digit));
        if erase {
            pin.pop();
        }
    }
    let row = |ui: &mut Ui, digits: &[char], pin: &mut String, focused: &mut bool| {
        for digit in digits {
            let key = button(ui, None, digit.encode_utf8(&mut [0; 4]), Tone::Quiet, KEY);
            if focus_first && *digit == '1' {
                key.request_focus();
                *focused = true;
            }
            if key.clicked() {
                pin.push(*digit);
            }
        }
    };
    for digits in [['1', '2', '3'], ['4', '5', '6'], ['7', '8', '9']] {
        ui.horizontal(|ui| row(ui, &digits, pin, focused));
    }
    ui.horizontal(|ui| {
        let erase = button(ui, Some(Icon::Back), "", Tone::Quiet, KEY).on_hover_text(tr("Delete digit"));
        if erase.clicked() {
            pin.pop();
        }
        row(ui, &['0'], pin, focused);
    });
    pin.truncate(LENGTH);
    before < LENGTH && pin.len() == LENGTH
}

/// Where a popover menu opens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Under the ⋮ that opened it, right edges lined up (above it when there is no room below).
    Below(Rect),
    /// Beside the focused card it belongs to (F2, Shift+F10, X), at its top.
    Beside(Rect),
    /// At the pointer of a right-click.
    Point(Pos2),
    /// Nothing to point at: in the middle of the window, over a dimmed page.
    Center,
}

/// Where a menu of `size` goes for `anchor`, kept inside `screen` with a margin: flipped above or
/// to the other side where it would not fit, then pushed in.
pub fn place(anchor: Anchor, size: Vec2, screen: Rect) -> Pos2 {
    const GAP: f32 = 6.0;
    const EDGE: f32 = 8.0;
    let fits_below = |y: f32| y + size.y <= screen.bottom() - EDGE;
    let fits_right = |x: f32| x + size.x <= screen.right() - EDGE;
    let (x, y) = match anchor {
        Anchor::Below(r) => (r.right() - size.x, if fits_below(r.bottom() + GAP) { r.bottom() + GAP } else { r.top() - GAP - size.y }),
        Anchor::Beside(r) => (if fits_right(r.right() + GAP) { r.right() + GAP } else { r.left() - GAP - size.x }, r.top()),
        Anchor::Point(p) => (if fits_right(p.x) { p.x } else { p.x - size.x }, if fits_below(p.y) { p.y } else { p.y - size.y }),
        Anchor::Center => (screen.center().x - size.x / 2.0, screen.center().y - size.y / 2.0),
    };
    let x = x.min(screen.right() - EDGE - size.x).max(screen.left() + EDGE);
    let y = y.min(screen.bottom() - EDGE - size.y).max(screen.top() + EDGE);
    Pos2::new(x, y)
}

/// The choice after `now` in `all`, back to the first after the last.
fn next_of<T: Copy + PartialEq>(all: &[T], now: T) -> T {
    let at = all.iter().position(|v| *v == now).map_or(0, |i| (i + 1) % all.len());
    all[at]
}

/// One row of a popover menu.
pub struct MenuRow {
    pub icon: Icon,
    /// The English label (translated when drawn).
    pub label: &'static str,
    /// The key that does the same from the card, shown at the right.
    pub hint: Option<&'static str>,
    /// Destructive rows sit last, under a divider, in the error colour.
    pub destructive: bool,
    /// Why the row cannot be chosen now; shown under its label.
    pub blocked: Option<&'static str>,
}

/// What a menu did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    Open,
    Chosen(usize),
    /// A press outside it: it closes, as any menu does.
    Dismissed,
}

/// A popover menu: a raised panel by its anchor with a heading (what it is for) and rows of icon,
/// label and shortcut. It takes the keyboard and the pointer while open but leaves the page as it
/// is (no dimming), and scrolls with faded edges in a window too short for it.
#[allow(clippy::too_many_arguments)]
pub fn popover(
    ctx: &egui::Context,
    name: &str,
    anchor: Anchor,
    title: &str,
    subtitle: Option<(String, Color32)>,
    rows: &[MenuRow],
    note: Option<&str>,
    focus_first: bool,
) -> (MenuOutcome, bool) {
    let id = Id::unique(("menu", name));
    let screen = ctx.content_rect();
    let last = ctx.memory(|m| m.area_rect(id));
    let size = last.map_or(vec2(280.0, 320.0), |r| r.size());
    // A clear layer between the page and the menu takes the pointer, so the page under the menu
    // neither lights up nor takes the click that closes it. With nothing to point at, it dims
    // the page as a dialog does.
    let catch = egui::Area::new(id.with("backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            if anchor == Anchor::Center {
                ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(150));
            }
            let catch = ui.allocate_rect(screen, Sense::click());
            catch.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr("Close menu")));
            catch
        })
        .inner;
    let mut outcome = MenuOutcome::Open;
    let mut focused = false;
    let high = high_contrast();
    let frame = Frame::new()
        .fill(*SURFACE_HIGH)
        .corner_radius(CornerRadius::same(radius::PANEL))
        .inner_margin(Margin::same(4))
        .stroke(Stroke::new(if high { 1.5 } else { 1.0 }, OUTLINE_VARIANT))
        .shadow(ctx.global_style().visuals.popup_shadow);
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(place(anchor, size, screen)).constrain(false).show(ctx, |ui| {
        frame.show(ui, |ui| {
            // The rows' widths: the longest label and hint, at least a comfortable width.
            let measure = |text: &str, font: egui::FontId| ui.painter().layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x;
            let widest = rows
                .iter()
                .map(|r| 12.0 + 18.0 + 12.0 + measure(tr(r.label), semibold(text::LABEL)) + r.hint.map_or(0.0, |h| 24.0 + measure(tr(h), regular(text::CAPTION))) + 14.0)
                .fold(260.0_f32, f32::max);
            let title_w = measure(title, semibold(text::BODY)) + 24.0;
            let width = widest.max(title_w.min(380.0)).min(screen.width() - 32.0);
            ui.set_width(width);
            // egui lays a new area out in at most 600x400 points: let the menu take the window's
            // height less its margin instead, so it scrolls only when the window is too short.
            ui.set_max_height(screen.height() - 16.0 - 10.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            let top = ui.cursor().top();
            // The heading: what the menu is for.
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut job = egui::text::LayoutJob::simple_singleline(title.to_owned(), semibold(text::BODY), *ON_SURFACE);
                    job.wrap = egui::text::TextWrapping::truncate_at_width(width - 20.0);
                    ui.label(job);
                    if let Some((line, color)) = &subtitle {
                        ui.label(RichText::new(line).font(regular(text::CAPTION)).color(*color));
                    }
                });
            });
            ui.add_space(4.0);
            divider(ui);
            let note = note.map(|n| {
                let text = RichText::new(tr(n)).font(regular(text::CAPTION)).color(OUTLINE);
                egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Wrap), width - 20.0, egui::TextStyle::Body)
            });
            let below = note.as_ref().map_or(0.0, |n| 10.0 + n.size().y);
            // The window's height less its margins, the frame, the heading, the list's own margin and
            // the note: wherever the menu is placed, it then fits inside the window.
            let heading = ui.cursor().top() - top;
            let room = (screen.height() - 16.0 - 10.0 - heading - 8.0 - 4.0 - below).max(CONTROL_HEIGHT);
            // A thin bar that always shows when the list scrolls.
            ui.spacing_mut().scroll = egui::style::ScrollStyle { foreground_color: true, ..egui::style::ScrollStyle::thin() };
            let list = egui::ScrollArea::vertical().id_salt(id.with("rows")).max_height(room).content_margin(Margin::same(4)).show(ui, |ui| {
                let follow = id.with("focus");
                let mut last = ui.ctx().data(|d| d.get_temp::<Option<Id>>(follow)).flatten();
                crate::gui::follow_focus(ui, &mut last);
                ui.ctx().data_mut(|d| d.insert_temp(follow, last));
                let mut divided = false;
                for (n, row) in rows.iter().enumerate() {
                    if row.destructive && !divided && n > 0 {
                        divider(ui);
                    }
                    divided |= row.destructive;
                    let response = ui.add_enabled_ui(row.blocked.is_none(), |ui| menu_row(ui, row)).inner;
                    if focus_first && !focused && row.blocked.is_none() {
                        response.request_focus();
                        focused = true;
                    }
                    if response.clicked() {
                        outcome = MenuOutcome::Chosen(n);
                    }
                }
            });
            scroll_fades(ui, &list, *SURFACE_HIGH);
            if let Some(note) = note {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(note);
                });
                ui.add_space(4.0);
            }
        });
    });
    // The menu holds the keyboard while open; a click outside closes it (on the release, so
    // nothing under it takes that click).
    ctx.memory_mut(|m| m.set_modal_layer(area.response.layer_id));
    if (catch.clicked() || catch.secondary_clicked()) && outcome == MenuOutcome::Open {
        outcome = MenuOutcome::Dismissed;
    }
    (outcome, focused)
}

/// A thin line between a menu's parts.
fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(rect.x_range().shrink(6.0), rect.center().y, Stroke::new(1.0, OUTLINE_VARIANT));
}

/// A menu row: icon, label, and the shortcut at the right; why it is off under the label.
fn menu_row(ui: &mut Ui, row: &MenuRow) -> Response {
    let width = ui.available_width();
    let (hovered, pressed) = last_state(ui);
    let enabled = ui.is_enabled();
    // Why it is off wraps under the label.
    let why = row.blocked.map(|why| ui.painter().layout(tr(why).to_owned(), regular(text::CAPTION), *OUTLINE, width - 42.0 - 14.0));
    let height = why.as_ref().map_or(40.0, |w| 20.0 + 11.0 + w.size().y + 9.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let label = tr(row.label);
    // A screen reader hears why a row is off too, not only the painted line under it.
    let said = match row.blocked {
        Some(why) => format!("{label}, {}", tr(why)),
        None => label.to_owned(),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &said));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let fill = if pressed && enabled {
        *SURFACE_BRIGHT
    } else if hovered && enabled {
        *SURFACE_HIGHEST
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(8), fill);
    let (ink, icon_ink) = if row.destructive { (*ERROR, *ERROR) } else if hovered { (*ON_SURFACE, *ON_SURFACE) } else { (*ON_SURFACE, *ON_SURFACE_VARIANT) };
    let top = if row.blocked.is_some() { rect.top() + 20.0 } else { rect.center().y };
    icons::paint(painter, row.icon, Rect::from_center_size(Pos2::new(rect.left() + 21.0, top), Vec2::splat(18.0)), icon_ink);
    let words = painter.layout_no_wrap(label.to_owned(), semibold(text::LABEL), ink);
    painter.galley(Pos2::new(rect.left() + 42.0, top - words.size().y / 2.0), words, ink);
    if let Some(hint) = row.hint {
        let keys = painter.layout_no_wrap(tr(hint).to_owned(), regular(text::CAPTION), *OUTLINE);
        painter.galley(Pos2::new(rect.right() - 14.0 - keys.size().x, top - keys.size().y / 2.0), keys, *OUTLINE);
    }
    if let Some(why) = why {
        painter.galley(Pos2::new(rect.left() + 42.0, top + 11.0), why, *OUTLINE);
    }
    focus_ring(ui, rect, &response, 8);
    response
}
