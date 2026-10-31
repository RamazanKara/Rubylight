//! A PC's own pages (details, address, co-op sessions, shared folders) and the profile,
//! shortcut and controller pages.

use super::*;

impl Launcher {
    pub(super) fn shortcuts_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        ui.label(tr("Send saved chords to the PC from stream controls. Import and export use Nightjar Android's shortcut format."));
        ui.horizontal(|ui| {
            let button = primary_button(ui, "New shortcut");
            if std::mem::take(&mut self.focus_screen) { button.request_focus(); }
            if button.clicked() {
                self.dialog = Dialog::Shortcut { id: None, name: String::new(), chord: String::new(), error: None };
                self.focus_dialog = true;
            }
            if quiet_button(ui, "Import…").clicked() { commands.push(Command::ImportShortcuts); }
            if quiet_button(ui, "Export…").clicked() { commands.push(Command::ExportShortcuts); }
        });
        ui.add_space(16.0);
        scroll_column(ui, "shortcuts", SETTINGS_W, |ui| {
            for shortcut in &self.shortcuts.data {
                row(ui, &shortcut.name, &shortcut.keys.join(" + "), |ui| {
                    ui.horizontal(|ui| {
                        if quiet_button(ui, "Edit").clicked() {
                            self.dialog = Dialog::Shortcut {
                                id: Some(shortcut.id.clone()), name: shortcut.name.clone(), chord: shortcut.keys.join(" + "), error: None,
                            };
                            self.focus_dialog = true;
                        }
                        let button = quiet_button(ui, "Delete");
                        if button.clicked() {
                            self.dialog = Dialog::DeleteShortcut { id: shortcut.id.clone(), name: shortcut.name.clone() };
                            self.focus_dialog = true;
                        }
                        button
                    }).inner
                });
            }
        });
    }

    pub(super) fn controllers_page(&mut self, ui: &mut Ui, pads: &crate::gamepad::Gamepads, commands: &mut Vec<Command>) {
        use crate::gamepad::{AXIS_NAMES, BUTTON_MAP, BUTTON_NAMES, CONNECT_AS, Mapping};
        ui.label(tr("Mappings apply to controllers of the same model. Local menu and disconnect chords always use physical buttons."));
        let selected = if self.controller_key.is_empty() { tr("Choose a controller") } else { &self.controller_mapping.name };
        let chooser = egui::ComboBox::from_id_salt("controller-model").selected_text(selected).show_ui(ui, |ui| {
            for pad in &pads.pads {
                if ui.selectable_label(pad.key == self.controller_key, &pad.name).clicked() {
                    commands.push(Command::EditController { key: pad.key.clone(), name: pad.name.clone() });
                }
            }
            for (key, mapping) in &pads.mappings {
                if !pads.pads.iter().any(|p| &p.key == key)
                    && ui.selectable_label(*key == self.controller_key, trf("{} (disconnected)", &[&mapping.name])).clicked() {
                    commands.push(Command::EditController { key: key.clone(), name: mapping.name.clone() });
                }
            }
        });
        if std::mem::take(&mut self.focus_screen) { chooser.response.request_focus(); }
        if self.controller_key.is_empty() {
            ui.label(tr("Connect a controller to create a mapping."));
            return;
        }
        ui.horizontal(|ui| {
            if quiet_button(ui, "Reset mapping").clicked() {
                self.controller_mapping = Mapping { name: self.controller_mapping.name.clone(), ..Default::default() };
            }
            ui.checkbox(&mut self.controller_test, tr("Test input (pause controller navigation)"));
        });
        let mapping = &mut self.controller_mapping;
        scroll_column(ui, "controller-mapping", SETTINGS_W, |ui| {
            if self.controller_test {
                let connected: Vec<_> = pads.pads.iter().filter(|p| p.key == self.controller_key).collect();
                if connected.is_empty() { ui.label(tr("This controller is disconnected.")); }
                for pad in connected {
                    let report = pad.preview(mapping, self.settings.swap_face_buttons);
                    let held = BUTTON_MAP.iter().zip(BUTTON_NAMES).filter_map(|(flag, name)| (report.buttons & flag != 0).then_some(tr(name))).collect::<Vec<_>>().join(", ");
                    ui.label(trf("Controller {}: {}", &[&(pad.index + 1), &if held.is_empty() { tr("No buttons held") } else { held.as_str() }]));
                    ui.label(trf("Left: {}, {}    Right: {}, {}    Triggers: {}, {}", &[&report.ls.0, &report.ls.1, &report.rs.0, &report.rs.1, &report.lt, &report.rt]));
                }
                ui.separator();
            }
            let selected = CONNECT_AS.iter().find(|(kind, _)| *kind == mapping.connect_as).map_or(CONNECT_AS[0].1, |(_, label)| label);
            row(ui, "Connect as", "The controller type games see. Automatic uses the type the controller reports; pick another if games show the wrong buttons. Xbox sends no motion or touchpad.", |ui| {
                egui::ComboBox::from_id_salt("connect-as").selected_text(tr(selected)).show_ui(ui, |ui| {
                    for (kind, label) in CONNECT_AS {
                        ui.selectable_value(&mut mapping.connect_as, kind, tr(label));
                    }
                }).response
            });
            row(ui, "Stick dead zone", "Ignore small movements near the center, then scale smoothly to full travel.", |ui| {
                ui.add(egui::Slider::new(&mut mapping.deadzone, 0..=50).suffix("%"))
            });
            section(ui, "Axes");
            for (i, name) in AXIS_NAMES.iter().enumerate() {
                let source = mapping.axes[i];
                let axis = tr(AXIS_NAMES[source.unsigned_abs().max(1) as usize - 1]);
                let selected = if source == 0 { tr("Disabled").to_owned() }
                    else if source < 0 { trf("{} (inverted)", &[&axis]) } else { axis.to_owned() };
                row(ui, name, "Choose the physical axis that drives this output.", |ui| {
                    egui::ComboBox::from_id_salt(("axis-mapping", i)).selected_text(selected).show_ui(ui, |ui| {
                        ui.selectable_value(&mut mapping.axes[i], 0, tr("Disabled"));
                        let range = if i < 4 { 0..4 } else { 4..6 };
                        for j in range {
                            ui.selectable_value(&mut mapping.axes[i], (j + 1) as i8, tr(AXIS_NAMES[j]));
                            if i < 4 { ui.selectable_value(&mut mapping.axes[i], -((j + 1) as i8), trf("{} (inverted)", &[&tr(AXIS_NAMES[j])])); }
                        }
                    }).response
                });
            }
            section(ui, "Buttons");
            for (i, name) in BUTTON_NAMES.iter().enumerate() {
                let selected = match mapping.buttons[i] {
                    None => tr("Default"),
                    Some(0) => tr("Disabled"),
                    Some(flag) => tr(BUTTON_NAMES[BUTTON_MAP.iter().position(|b| *b == flag).expect("validated button")]),
                };
                row(ui, name, "Default follows the Nintendo layout setting. Explicit mappings take priority.", |ui| {
                    egui::ComboBox::from_id_salt(("button-mapping", i)).selected_text(selected).show_ui(ui, |ui| {
                        ui.selectable_value(&mut mapping.buttons[i], None, tr("Default"));
                        ui.selectable_value(&mut mapping.buttons[i], Some(0), tr("Disabled"));
                        for (target, label) in BUTTON_MAP.iter().zip(BUTTON_NAMES) {
                            ui.selectable_value(&mut mapping.buttons[i], Some(*target), tr(label));
                        }
                    }).response
                });
            }
        });
    }

    /// A PC's details: its host software, its web UI where it has one, and the remote access
    /// check. What the PC can do decides what is shown, never its brand.
    pub(super) fn host_details(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
        use crate::host_page::{software_label, web_ui_url, CheckState};
        let Some(host) = view.hosts.get(id) else {
            self.go(Screen::Hosts);
            return;
        };
        let caps = host.info.as_ref().map_or(&nightjar_core::http::HostCapabilities::UNKNOWN, |i| &i.capabilities);
        let software = software_label(caps);
        let web_ui = web_ui_url(host);
        // "Open in browser" goes when the PC does: its focus, held or remembered, moves to the
        // check instead of nowhere.
        if !matches!(web_ui, Some(Ok(_)))
            && (self.focused_key.as_deref() == Some("web-ui") || self.restore_key.as_deref() == Some("web-ui"))
        {
            self.focused_key = None;
            self.restore_key = Some("remote-check".into());
        }
        let state = self.remote.state_for(id).cloned();
        let running = state == Some(CheckState::Running);
        scroll_column(ui, "host-details", SETTINGS_W, |ui| {
            ui.set_max_width(SETTINGS_W);
            section(ui, "Host");
            detail(ui, tr("Host software"), &software);
            ui.add_space(8.0);
            match &web_ui {
                None => {}
                Some(Ok(url)) => {
                    // The address goes inside the card it belongs to, not loose under it.
                    let about = format!("{}
{url}", tr("Manage the host's applications, paired devices and settings in its web UI. Sign in with the web UI's own account; pairing does not sign you in. Remote access requires a reachable web UI port."));
                    let button = row(ui, "Host web UI", &about, |ui| quiet_button(ui, "Open in browser"));
                    self.focus_item(&button, "web-ui", true);
                    if button.clicked() {
                        commands.push(Command::OpenWebUi(url.clone()));
                    }
                }
                // The PC has a web UI, but its saved address makes no link to it.
                Some(Err(why)) => {
                    ui.label(RichText::new(tr("Host web UI")).font(semibold(text::BODY)).color(ON_SURFACE));
                    ui.label(RichText::new(why).color(ERROR));
                }
            }
            self.connection_address(ui, host, commands);
            section(ui, "Play away from home");
            let button = row(ui, "Remote access", "Checks the paired PC's identity with five small HTTPS requests. It does not start a stream or download a speed-test file.", |ui| {
                if running {
                    quiet_button(ui, "Cancel")
                } else {
                    primary_button(ui, "Test this PC")
                }
            });
            self.focus_item(&button, "remote-check", !matches!(web_ui, Some(Ok(_))));
            if button.clicked() {
                if running {
                    self.remote.cancel();
                } else {
                    commands.push(Command::RemoteCheck(id));
                }
            }
            match &state {
                None => {}
                Some(CheckState::Running) => {
                    ui.horizontal(|ui| {
                        spinner(ui, 10.0);
                        ui.label(RichText::new(tr("Checking your paired PC…")).color(ON_SURFACE_VARIANT));
                    });
                }
                Some(CheckState::Cancelled) => {
                    ui.label(RichText::new(tr("Test cancelled. No host or network settings were changed.")).color(ON_SURFACE_VARIANT));
                }
                Some(CheckState::Done(report)) => remote_report(ui, report),
            }
            if caps.has(nightjar_core::http::Feature::SharedFolders) {
                self.shared_folders(ui, id, commands);
            }
            if caps.has(nightjar_core::http::Feature::Coop) {
                self.coop_sessions(ui, id, commands);
            }
        });
    }

    /// The details page's connection address: Automatic, or one of the PC's known addresses to
    /// always connect through (Moonlight V+ PC's address pinning). Shown once the PC has more
    /// than one address, or while one is pinned.
    pub(super) fn connection_address(&mut self, ui: &mut Ui, host: &Host, commands: &mut Vec<Command>) {
        let record = &host.record;
        let known: Vec<&String> = std::iter::once(&record.address).chain(&record.other_addresses).collect();
        if known.len() < 2 && record.pinned_address.is_none() {
            return;
        }
        section(ui, "Connection");
        let pinned = record.pinned_address.clone();
        let mut choice = pinned.clone();
        let shown = pinned.clone().unwrap_or_else(|| tr("Automatic").to_owned());
        let chooser = row(ui, "Connection address", "Automatic tries the address that answered last, then the PC's other addresses. Choose one to always connect through it.", |ui| {
            egui::ComboBox::from_id_salt("pinned-address").selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, None, tr("Automatic"));
                for address in &known {
                    ui.selectable_value(&mut choice, Some((*address).clone()), address.as_str());
                }
            }).response
        });
        self.focus_item(&chooser, "pinned-address", false);
        if choice != pinned {
            commands.push(Command::PinAddress { host: host.id, address: choice });
        }
    }

    /// The details page's Nucleus Co-op sessions (Experimental): their players, free slots and
    /// this device's own seat, and a join that then streams the slot.
    pub(super) fn coop_sessions(&mut self, ui: &mut Ui, id: u64, commands: &mut Vec<Command>) {
        use crate::coop_page::{join_label, phase_text, slot_text};
        section(ui, "Co-op");
        let Some(open) = self.coop.state_for(id) else {
            let button = row(ui, "Co-op sessions", "Play a game together with other devices, each in its own copy of the game on this PC, through Nucleus Co-op. Join a running session here, or start one with “<game> (Co-op)” in the library.", |ui| {
                quiet_button(ui, "Show sessions")
            });
            self.focus_item(&button, "coop", false);
            if button.clicked() {
                commands.push(Command::Coop(id));
            }
            if let Some(error) = self.coop.failure_for(id) {
                ui.label(RichText::new(error).color(ERROR));
            }
            return;
        };
        let (mut refresh, mut close, mut join) = (false, false, None);
        ui.horizontal_wrapped(|ui| {
            refresh = ui.add_enabled_ui(!open.loading, |ui| quiet_button(ui, "Refresh")).inner.clicked();
            close = text_button(ui, "Close").clicked();
        });
        if open.loading || open.joining.is_some() {
            ui.horizontal(|ui| {
                spinner(ui, 10.0);
                let doing = if open.joining.is_some() { tr("Joining…") } else { tr("Loading…") };
                ui.label(RichText::new(doing).color(ON_SURFACE_VARIANT));
            });
        }
        if let Some(error) = &open.error {
            ui.label(RichText::new(error).color(ERROR));
        }
        if let Some(done) = &open.done {
            ui.label(RichText::new(done).color(ON_SURFACE));
        }
        ui.add_space(6.0);
        if let Some(coop) = &open.coop {
            if !coop.available {
                let why = coop.reason.as_deref().unwrap_or_else(|| tr("The PC gave no reason."));
                ui.label(RichText::new(trf("The PC cannot run co-op sessions right now: {}", &[&why])).color(ERROR));
            }
            if coop.available && coop.sessions.is_empty() {
                ui.label(RichText::new(tr("No co-op session is running. Start one with “<game> (Co-op)” in the library.")).color(ON_SURFACE_VARIANT));
            }
            for session in &coop.sessions {
                let line = folder_line(ui, &session.title, &phase_text(session), |ui| {
                    if session.joinable {
                        ui.add_enabled_ui(open.joining.is_none(), |ui| primary_button(ui, join_label(session))).inner
                    } else {
                        ui.label(String::new())
                    }
                });
                if session.joinable && line.clicked() {
                    join = Some(session.id);
                }
                ui.label(RichText::new(trf("Free slots: {}", &[&session.free_slots])).font(regular(text::CAPTION)).color(OUTLINE));
                for player in &session.players {
                    let text = slot_text(player.slot, player.name.as_deref(), player.you, player.away);
                    let color = if player.you { ON_SURFACE } else { ON_SURFACE_VARIANT };
                    ui.label(RichText::new(text).color(color));
                }
                if let Some(refusal) = session.refusal.as_deref().filter(|_| !session.joinable) {
                    ui.label(RichText::new(refusal).color(ERROR));
                }
                ui.add_space(6.0);
            }
            if !coop.games.is_empty() {
                ui.label(RichText::new(tr("Co-op games on this PC")).font(semibold(text::BODY)).color(ON_SURFACE));
                for game in &coop.games {
                    folder_line(ui, &game.title, &trf("Up to {} players", &[&game.max_players]), |ui| ui.label(String::new()));
                }
            }
            ui.label(RichText::new(tr("Co-op streams take controllers only: keyboard and mouse stay on the PC.")).font(regular(text::CAPTION)).color(OUTLINE));
        }
        if refresh {
            self.coop.refresh();
        } else if close {
            self.coop.close();
        } else if let Some(session) = join {
            self.coop.join(id, session);
        }
    }

    /// The details page's browser of the folders the PC shares with this device (Experimental).
    pub(super) fn shared_folders(&mut self, ui: &mut Ui, id: u64, commands: &mut Vec<Command>) {
        use crate::folders_page::{size_text, Place};
        use nightjar_core::http::EntryKind;
        section(ui, "Shared folders");
        let Some(open) = self.folders.state_for(id) else {
            let button = row(ui, "Shared folders", "Browse the folders this PC shares with this device, download files from them and, with a write grant, upload to them. The PC opens them as the user signed in on it.", |ui| {
                quiet_button(ui, "Browse")
            });
            self.focus_item(&button, "shared-folders", false);
            if button.clicked() {
                commands.push(Command::SharedFolders(id));
            }
            if let Some(error) = self.folders.failure_for(id) {
                ui.label(RichText::new(error).color(ERROR));
            }
            return;
        };
        let busy = open.transfer.is_some();
        let (title, writable) = match &open.place {
            Place::Folders => (tr("Folders on this PC").to_owned(), false),
            Place::Dir { folder, path } if path.is_empty() => (folder.name.clone(), folder.writable()),
            Place::Dir { folder, path } => (format!("{} / {}", folder.name, path.replace('/', " / ")), folder.writable()),
        };
        let (mut up, mut refresh, mut close, mut cancel) = (false, false, false, false);
        let mut enter_folder = None;
        let mut enter_dir = None;
        ui.label(RichText::new(&title).font(semibold(text::BODY)).color(ON_SURFACE));
        ui.horizontal_wrapped(|ui| {
            if open.place != Place::Folders {
                up = quiet_button(ui, "Up").clicked();
            }
            refresh = quiet_button(ui, "Refresh").clicked();
            if writable && ui.add_enabled_ui(!busy, |ui| quiet_button(ui, "Upload a file")).inner.clicked() {
                commands.push(Command::FolderUpload(id));
            }
            close = text_button(ui, "Close").clicked();
        });
        if open.loading {
            ui.horizontal(|ui| {
                spinner(ui, 10.0);
                ui.label(RichText::new(tr("Loading…")).color(ON_SURFACE_VARIANT));
            });
        }
        if let Some(transfer) = &open.transfer {
            let (done, total) = transfer.progress.get();
            let template = if transfer.upload { en("Uploading {}: {} of {}") } else { en("Downloading {}: {} of {}") };
            ui.label(RichText::new(trf(template, &[&transfer.name, &size_text(done), &size_text(total)])).color(ON_SURFACE_VARIANT));
            ui.horizontal(|ui| {
                let fraction = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                ui.add(egui::ProgressBar::new(fraction).desired_width(320.0));
                cancel = quiet_button(ui, "Cancel").clicked();
            });
            ui.label(RichText::new(tr("Leaving this page cancels the transfer.")).font(regular(text::CAPTION)).color(OUTLINE));
        }
        if let Some(error) = &open.transfer_error {
            ui.label(RichText::new(error).color(ERROR));
        }
        if let Some(error) = &open.error {
            ui.label(RichText::new(error).color(ERROR));
        }
        if let Some(done) = &open.done {
            ui.label(RichText::new(done).color(ON_SURFACE));
        }
        ui.add_space(6.0);
        match &open.place {
            Place::Folders => {
                if !open.loading && open.error.is_none() && open.folders.is_empty() {
                    ui.label(RichText::new(tr("The PC shares no folders with this device.")).color(ON_SURFACE_VARIANT));
                }
                for folder in &open.folders {
                    let access = if folder.writable() { tr("Read and write") } else { tr("Read only") };
                    if folder_line(ui, &folder.name, access, |ui| quiet_button(ui, "Open")).clicked() {
                        enter_folder = Some(folder.clone());
                    }
                }
            }
            Place::Dir { .. } => {
                let entries = open.entries.as_ref().map_or(&[][..], |e| e.entries.as_slice());
                if !open.loading && open.error.is_none() && entries.is_empty() {
                    ui.label(RichText::new(tr("This folder is empty.")).color(ON_SURFACE_VARIANT));
                }
                for entry in entries {
                    match entry.kind {
                        EntryKind::Dir => {
                            if folder_line(ui, &entry.name, tr("Folder"), |ui| quiet_button(ui, "Open")).clicked() {
                                enter_dir = Some(entry.name.clone());
                            }
                        }
                        EntryKind::File => {
                            let download = folder_line(ui, &entry.name, &size_text(entry.size), |ui| {
                                ui.add_enabled_ui(!busy, |ui| quiet_button(ui, "Download")).inner
                            });
                            if download.clicked() {
                                commands.push(Command::FolderDownload { host: id, name: entry.name.clone(), size: entry.size });
                            }
                        }
                        // A kind this version does not know: shown, not opened.
                        EntryKind::Unknown => {
                            folder_line(ui, &entry.name, tr("Neither a file nor a folder"), |ui| ui.label(String::new()));
                        }
                    }
                }
            }
        }
        if up {
            self.folders.up(id);
        } else if refresh {
            self.folders.list();
        } else if close {
            self.folders.close();
        } else if let Some(folder) = enter_folder {
            self.folders.enter_folder(id, folder);
        } else if let Some(name) = enter_dir {
            self.folders.enter_dir(id, &name);
        }
        if cancel {
            self.folders.cancel_transfer(id);
        }
    }

    pub(super) fn profiles_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        ui.label(tr("Profiles change the settings used for your next stream. Each PC and game can override them."));
        let before_profiles = self.profiles.clone();
        let before = self.profiles.active;
        let active = self.profiles.entries.iter().find(|p| Some(p.id) == before)
            .map_or(tr("Main settings"), |p| p.name.as_str());
        row(ui, "Active profile", "Main settings apply wherever the profile has no override.", |ui| {
            egui::ComboBox::from_id_salt("active-profile").selected_text(active).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.profiles.active, None, tr("Main settings"));
                for profile in &self.profiles.entries {
                    ui.selectable_value(&mut self.profiles.active, Some(profile.id), &profile.name);
                }
            }).response
        });
        let button = primary_button(ui, "New profile");
        if std::mem::take(&mut self.focus_screen) { button.request_focus(); }
        if button.clicked() {
            self.dialog = Dialog::ProfileName { id: None, name: String::new(), settings: Overrides::default(), error: None };
            self.focus_dialog = true;
        }
        ui.add_space(16.0);
        scroll(ui, "profiles", |ui| {
            section(ui, "Network choices");
            ui.label(self.network.description(&self.profiles.home_wifi));
            if self.network.link_mbps > 0 { ui.label(trf("Link speed: {} Mbps", &[&self.network.link_mbps])); }
            let selected = self.profiles.selected(self.network.kind(&self.profiles.home_wifi));
            let selected_name = selected.map(|id| self.profiles.entries.iter().find(|p| p.id == id)
                .map_or(tr("Missing profile; main settings apply"), |p| p.name.as_str())).unwrap_or(tr("Main settings"));
            ui.label(trf("Next stream: {}, then PC and game overrides.", &[&selected_name]));
            row(ui, "Choose automatically", "Select a profile when a stream starts on this network.", |ui| toggle(ui, &mut self.profiles.automatic));
            ui.add_enabled_ui(self.profiles.automatic, |ui| {
                for (id, title, selected) in [("mobile-profile", en("Mobile or metered"), &mut self.profiles.mobile),
                    ("home-profile", en("Home Wi-Fi"), &mut self.profiles.home), ("wifi-profile", en("Other Wi-Fi"), &mut self.profiles.wifi)] {
                    let name = self.profiles.entries.iter().find(|p| Some(p.id) == *selected).map_or(tr("Active profile"), |p| p.name.as_str());
                    row(ui, title, "An unset rule follows your active profile.", |ui| {
                        egui::ComboBox::from_id_salt(id).selected_text(name).show_ui(ui, |ui| {
                            ui.selectable_value(selected, None, tr("Active profile"));
                            for profile in &self.profiles.entries { ui.selectable_value(selected, Some(profile.id), &profile.name); }
                        }).response
                    });
                }
                ui.label(tr("Home Wi-Fi names stay on this device. Names are case-sensitive."));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.home_wifi_name).hint_text(tr("Wi-Fi name")).char_limit(32));
                    let valid = !self.home_wifi_name.is_empty() && self.home_wifi_name.len() <= 32
                        && !self.profiles.home_wifi.contains(&self.home_wifi_name) && self.profiles.home_wifi.len() < 50;
                    if ui.add_enabled(valid, egui::Button::new(tr("Add"))).clicked() {
                        self.profiles.home_wifi.push(std::mem::take(&mut self.home_wifi_name));
                    }
                    if let Some(ssid) = &self.network.ssid {
                        if ui.add_enabled(!self.profiles.home_wifi.contains(ssid) && self.profiles.home_wifi.len() < 50, egui::Button::new(tr("Add current Wi-Fi"))).clicked() {
                            self.profiles.home_wifi.push(ssid.clone());
                        }
                    }
                });
                self.profiles.home_wifi.retain(|name| {
                    !ui.horizontal(|ui| { ui.label(name); text_button(ui, "Remove").clicked() }).inner
                });
            });
            row(ui, "Mobile data bitrate limit", "Also applies to metered Wi-Fi or Ethernet, after all profile, PC and game overrides. 0 means no limit.", |ui| {
                step_value(ui, &mut self.profiles.mobile_limit_kbps, 0..=500_000, 1000.0, |d| d.speed(100).suffix(" kbps"))
            });
            section(ui, "Saved profiles");
            for profile in &self.profiles.entries {
                ui.push_id(profile.id, |ui| {
                    row(ui, &profile.name, if Some(profile.id) == self.profiles.active { en("Active for new streams") } else { en("Saved profile") }, |ui| {
                        ui.horizontal(|ui| {
                            if quiet_button(ui, "Edit").clicked() { commands.push(Command::ProfileSettings(profile.id)); }
                            ui.menu_button(tr("More"), |ui| {
                                if ui.button(tr("Rename")).clicked() {
                                    self.dialog = Dialog::ProfileName { id: Some(profile.id), name: profile.name.clone(), settings: Overrides::default(), error: None };
                                    self.focus_dialog = true;
                                    ui.close();
                                }
                                if ui.button(tr("Duplicate")).clicked() {
                                    self.dialog = Dialog::ProfileName { id: None, name: String::new(), settings: profile.settings.clone(), error: None };
                                    self.focus_dialog = true;
                                    ui.close();
                                }
                                if ui.button(tr("Delete")).clicked() {
                                    self.dialog = Dialog::DeleteProfile { id: profile.id, name: profile.name.clone() };
                                    self.focus_dialog = true;
                                    ui.close();
                                }
                            }).response
                        }).inner
                    });
                });
            }
        });
        if self.profiles != before_profiles { commands.push(Command::SaveProfiles); }
    }
}

/// A folder or file of the shared folder browser: its name as the PC spells it (never
/// translated), what it is, and its button.
pub(super) fn folder_line(ui: &mut Ui, name: &str, info: &str, control: impl FnOnce(&mut Ui) -> Response) -> Response {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).font(semibold(text::BODY)).color(ON_SURFACE));
        ui.label(RichText::new(info).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
        ui.with_layout(Layout::right_to_left(Align::Center), control).inner
    })
    .inner
}

/// What the remote access check found; an error as it came.
pub(super) fn remote_report(ui: &mut Ui, report: &crate::host_page::Report) {
    ui.label(RichText::new(tr(report.summary)).color(ON_SURFACE));
    if let Some(error) = &report.error {
        ui.label(RichText::new(error).color(ERROR));
    }
    if !report.verified {
        return;
    }
    if let Some(route) = report.route {
        ui.add_space(6.0);
        ui.label(RichText::new(tr(route)).color(ON_SURFACE_VARIANT));
    }
    ui.add_space(6.0);
    let unmeasured = || tr("Not measured").to_owned();
    let ms = |v: Option<f64>| v.map_or_else(unmeasured, |v| trf("{} ms", &[&format!("{v:.1}")]));
    detail(ui, tr("Network path"), tr(report.path.unwrap_or(en("Unavailable"))));
    detail(ui, tr("HTTPS request average"), &ms(report.latency_ms));
    detail(ui, tr("Jitter"), &ms(report.jitter_ms));
    detail(ui, tr("Failed requests"), &report.loss_percent.map_or_else(unmeasured, |v| trf("{} %", &[&format!("{v:.0}")])));
    ui.label(
        RichText::new(tr("These are application-request measurements, not video packet loss or input latency."))
            .font(regular(text::CAPTION))
            .color(OUTLINE),
    );
}
