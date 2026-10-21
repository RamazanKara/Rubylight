//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

use egui::{
    vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2,
};

use crate::config::{
    AudioChannels, CodecPreference, DecoderPreference, HostDisplay, MouseMode, Overrides, Profiles, Settings, WindowMode,
};
use crate::hosts::{Apps, Art, Host, Hosts, Reach};
use crate::stream::{stream_mode, Display};
use crate::theme::*;

pub enum Command {
    Launch { host: u64, app: i32, resume: bool },
    QuitApp(u64),
    ControlAction { host: u64, app: i32 },
    QuitAndLaunch { host: u64, app: i32 },
    Pair(u64),
    PairOtp { host: u64, pin: String, passphrase: String },
    Wake(u64),
    HostSettings(u64),
    SaveHostSettings(u64),
    AppSettings { host: u64, app: nightjar_core::http::App },
    SaveAppSettings(u64),
    ProfileSettings(u64),
    SaveProfile(u64),
    SaveProfiles,
    ControllerSettings,
    EditController { key: String, name: String },
    SaveControllerMapping,
    CancelPair,
    AddHost(String),
    Unpair(u64),
    Remove(u64),
    Refresh(u64),
    OpenApps(u64),
    SaveSettings,
    CancelConnect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Hosts,
    Apps(u64),
    Settings,
    HostSettings(u64),
    AppSettings(u64),
    Profiles,
    ProfileSettings(u64),
    Controllers,
}

impl Screen {
    pub fn save_command(self) -> Option<Command> {
        match self {
            Self::Settings => Some(Command::SaveSettings),
            Self::HostSettings(id) => Some(Command::SaveHostSettings(id)),
            Self::AppSettings(id) => Some(Command::SaveAppSettings(id)),
            Self::ProfileSettings(id) => Some(Command::SaveProfile(id)),
            Self::Controllers => Some(Command::SaveControllerMapping),
            _ => None,
        }
    }
}

pub enum Dialog {
    None,
    AddHost { text: String, error: Option<String> },
    Pairing { host: u64, pin: String },
    Otp { host: u64, pin: String, passphrase: String, error: Option<String> },
    Message { title: String, body: String },
    ConfirmQuit { host: u64, start: Option<(i32, String)>, running: String },
    ConfirmAction { host: u64, app: i32, name: String },
    ProfileName { id: Option<u64>, name: String, settings: Overrides, error: Option<String> },
    DeleteProfile { id: u64, name: String },
}

/// The connecting card's content.
pub struct ConnectingView {
    pub app: String,
    pub host: String,
    pub stage: String,
}

pub struct Launcher {
    pub screen: Screen,
    pub dialog: Dialog,
    pub settings: Settings,
    pub profiles: Profiles,
    pub edit_settings: Settings,
    pub inherited_settings: Settings,
    pub edit_overrides: Overrides,
    pub editing_app: Option<nightjar_core::http::App>,
    pub edit_name: String,
    pub settings_dirty: bool,
    pub controller_key: String,
    pub controller_mapping: crate::gamepad::Mapping,
    pub controller_test: bool,
    logo: Option<TextureHandle>,
    focus_screen: bool,
    focus_dialog: bool,
    settings_return: Screen,
    /// The display's tearing support, for the VRR setting's note.
    pub tearing_supported: bool,
    pub display: Display,
    pub identity_ready: bool,
}

/// What the launcher shows, borrowed from the app each frame.
pub struct View<'a> {
    pub hosts: &'a Hosts,
    pub pads: &'a crate::gamepad::Gamepads,
    pub connecting: Option<ConnectingView>,
}

const CARD_W: f32 = 300.0;
const CARD_H: f32 = 112.0;
const POSTER_W: f32 = 168.0;
const POSTER_H: f32 = 224.0;
const CONTENT_MAX: f32 = 1180.0;

fn focus_ring(ui: &Ui, rect: Rect, response: &Response, radius: u8) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(3.0),
            CornerRadius::same(radius + 3),
            Stroke::new(2.0, MOON),
            StrokeKind::Outside,
        );
    } else if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(radius),
            Stroke::new(1.0, OUTLINE),
            StrokeKind::Inside,
        );
    }
}

/// A switch in the brand colours; returns whether it changed.
fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(44.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let enabled = ui.is_enabled();
    let track = if *on { MOON } else { SURFACE_HIGHEST };
    let track = if enabled { track } else { track.gamma_multiply(0.4) };
    ui.painter().rect_filled(rect, CornerRadius::same(12), track);
    let knob_x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
    let knob = if *on { ON_MOON } else { OUTLINE };
    ui.painter().circle_filled(Pos2::new(knob_x, rect.center().y), 8.0, knob);
    focus_ring(ui, rect, &response, 12);
    response
}

fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(ON_MOON).font(semibold(15.0)))
        .fill(MOON)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn quiet_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(ON_SURFACE).font(semibold(15.0)))
        .fill(SURFACE_HIGHEST)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn text_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(ON_SURFACE_VARIANT).font(semibold(15.0)))
        .frame(false)
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn spinner(ui: &mut Ui, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let center = rect.center();
    let start = t * 4.0;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = start + i as f32 / 24.0 * std::f32::consts::PI * 1.4;
            center + radius * 0.8 * vec2(a.cos(), a.sin())
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(2.5, MOON)));
    ui.ctx().request_repaint();
}

impl Launcher {
    pub fn new(settings: Settings, profiles: Profiles, display: Display) -> Self {
        Self {
            screen: Screen::Hosts,
            dialog: Dialog::None,
            settings,
            profiles,
            edit_settings: Settings::default(),
            inherited_settings: Settings::default(),
            edit_overrides: Overrides::default(),
            editing_app: None,
            edit_name: String::new(),
            settings_dirty: false,
            controller_key: String::new(),
            controller_mapping: crate::gamepad::Mapping::default(),
            controller_test: false,
            logo: None,
            focus_screen: true,
            focus_dialog: false,
            settings_return: Screen::Hosts,
            tearing_supported: false,
            display,
            identity_ready: false,
        }
    }

    pub fn go(&mut self, screen: Screen) {
        if matches!(screen, Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_)) {
            self.settings_return = self.screen;
        }
        if self.screen == Screen::Settings && screen != Screen::Settings && self.settings_dirty {
            self.settings_dirty = false;
        }
        self.screen = screen;
        self.focus_screen = true;
    }

    /// Gives the screen's first item focus again (after a stream or a dialog).
    pub fn refocus(&mut self) {
        self.focus_screen = true;
    }

    pub fn message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.dialog = Dialog::Message {
            title: title.into(),
            body: body.into(),
        };
        self.focus_dialog = true;
    }

    fn logo(&mut self, ctx: &egui::Context) -> egui::TextureId {
        self.logo
            .get_or_insert_with(|| ctx.load_texture("logo", logo_image(128), egui::TextureOptions::LINEAR))
            .id()
    }

    /// Back: Escape or the controller's B button.
    fn back(&mut self, commands: &mut Vec<Command>) {
        match &self.dialog {
            Dialog::None => match self.screen {
                Screen::Hosts => {}
                Screen::Apps(_) => self.go(Screen::Hosts),
                Screen::Settings => {
                    commands.push(Command::SaveSettings);
                    self.go(Screen::Hosts);
                }
                Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => {
                    commands.extend(self.screen.save_command());
                    self.go(self.settings_return);
                }
                Screen::Profiles => self.go(Screen::Settings),
                Screen::Controllers => {
                    commands.push(Command::SaveControllerMapping);
                    self.go(Screen::Settings);
                }
            },
            Dialog::Pairing { .. } => {
                commands.push(Command::CancelPair);
                self.dialog = Dialog::None;
            }
            _ => self.dialog = Dialog::None,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, view: View<'_>) -> Vec<Command> {
        let mut commands = Vec::new();
        let ctx = ui.ctx().clone();
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if escape && view.connecting.is_none() {
            self.back(&mut commands);
        }
        let logo = self.logo(&ctx);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, NIGHT);
        // The banner's orbit rings, off the top right corner.
        let center = Pos2::new(full.right() + 40.0, full.top() + 40.0);
        for r in [140.0, 210.0, 280.0, 350.0] {
            ui.painter().circle_stroke(center, r, Stroke::new(1.5, ORBIT));
        }

        let margin = ((full.width() - CONTENT_MAX) / 2.0).max(32.0);
        let content = Rect::from_min_max(
            Pos2::new(full.left() + margin, full.top() + 24.0),
            Pos2::new(full.right() - margin, full.bottom() - 16.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
        let ui = &mut child;
        let blocked = !matches!(self.dialog, Dialog::None) || view.connecting.is_some();
        // Screens take focus only when nothing covers them.
        let held_focus = self.focus_screen;
        if blocked {
            self.focus_screen = false;
        }
        ui.add_enabled_ui(!blocked, |ui| {
            self.header(ui, logo, &view, &mut commands);
            ui.add_space(22.0);
            match self.screen {
                Screen::Hosts => self.hosts(ui, &view, &mut commands, logo),
                Screen::Apps(id) => self.apps(ui, &view, id, &mut commands),
                Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.settings_page(ui, &mut commands),
                Screen::Profiles => self.profiles_page(ui, &mut commands),
                Screen::Controllers => self.controllers_page(ui, view.pads, &mut commands),
            }
        });
        if blocked {
            self.focus_screen = held_focus;
        }
        self.dialogs(&ctx, &view, &mut commands);
        if let Some(c) = &view.connecting {
            self.connecting(&ctx, c, &mut commands);
        }
        commands
    }

    fn header(&mut self, ui: &mut Ui, logo: egui::TextureId, view: &View<'_>, commands: &mut Vec<Command>) {
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            match self.screen {
                Screen::Hosts => {
                    egui::Image::from_texture((logo, vec2(30.0, 30.0))).ui(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new("Nightjar").font(semibold(22.0)).color(PAPER));
                }
                Screen::Apps(id) => {
                    if text_button(ui, "‹  PCs").clicked() {
                        self.go(Screen::Hosts);
                    }
                    let name = view.hosts.get(id).map_or("", |h| h.display_name()).to_owned();
                    ui.label(RichText::new(name).font(semibold(22.0)).color(PAPER));
                }
                Screen::Settings => {
                    if text_button(ui, "‹  Back").clicked() {
                        commands.push(Command::SaveSettings);
                        self.go(Screen::Hosts);
                    }
                    ui.label(RichText::new("Settings").font(semibold(22.0)).color(PAPER));
                }
                Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
                    }
                    ui.label(RichText::new(&self.edit_name).font(semibold(22.0)).color(PAPER));
                }
                Screen::Profiles => {
                    if text_button(ui, "‹  Settings").clicked() {
                        self.go(Screen::Settings);
                    }
                    ui.label(RichText::new("Profiles").font(semibold(22.0)).color(PAPER));
                }
                Screen::Controllers => {
                    if text_button(ui, "‹  Settings").clicked() { self.back(commands); }
                    ui.label(RichText::new("Controller mappings").font(semibold(22.0)).color(PAPER));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 22.0;
                match self.screen {
                Screen::Hosts => {
                    if text_button(ui, "Settings").clicked() {
                        self.go(Screen::Settings);
                    }
                    if text_button(ui, "Add PC").clicked() {
                        self.dialog = Dialog::AddHost {
                            text: String::new(),
                            error: None,
                        };
                        self.focus_dialog = true;
                    }
                }
                Screen::Apps(id) => {
                    if text_button(ui, "PC settings").clicked() {
                        commands.push(Command::HostSettings(id));
                    }
                    if let Some(host) = view.hosts.get(id) {
                        if host.running_app().is_some() && text_button(ui, "Quit running app").clicked() {
                            self.dialog = Dialog::ConfirmQuit {
                                host: id,
                                start: None,
                                running: running_name(host),
                            };
                            self.focus_dialog = true;
                        }
                        if text_button(ui, "Refresh").clicked() {
                            commands.push(Command::Refresh(id));
                        }
                    }
                }
                Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::Profiles | Screen::ProfileSettings(_) | Screen::Controllers => {}
                }
            });
        });
    }

    fn hosts(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        ui.label(RichText::new("Your PCs").font(semibold(30.0)).color(ON_SURFACE));
        ui.label(
            RichText::new("PCs running a Moonlight-compatible host on this network appear here. Add one by address if it doesn't.")
                .color(ON_SURFACE_VARIANT),
        );
        ui.add_space(18.0);
        if view.hosts.list.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                egui::Image::from_texture((logo, vec2(72.0, 72.0))).ui(ui);
                ui.add_space(14.0);
                if self.identity_ready {
                    ui.label(RichText::new("Looking for PCs…").font(semibold(18.0)));
                } else {
                    ui.label(RichText::new("Creating this device's pairing key…").font(semibold(18.0)));
                }
                ui.label(
                    RichText::new("Start Sunshine, Apollo, Vibepollo, Rubylight or GeForce Experience on your PC.")
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
            });
            return;
        }
        scroll(ui, "hosts", |ui| {
            let columns = ((ui.available_width() + 16.0) / (CARD_W + 16.0)).floor().max(1.0) as usize;
            for (row, chunk) in view.hosts.list.chunks(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (i, host) in chunk.iter().enumerate() {
                        let response = host_card(ui, host);
                        if self.focus_screen && row == 0 && i == 0 {
                            response.request_focus();
                            self.focus_screen = false;
                        }
                        if response.clicked() {
                            if host.paired() {
                                commands.push(Command::OpenApps(host.id));
                            } else if host.reach == Reach::Offline {
                                commands.push(Command::Refresh(host.id));
                            } else {
                                commands.push(Command::Pair(host.id));
                            }
                        }
                        response.context_menu(|ui| {
                            if host.record.mac.is_some() && ui.button("Wake up").clicked() {
                                commands.push(Command::Wake(host.id));
                                ui.close();
                            }
                            if ui.button("PC settings").clicked() {
                                commands.push(Command::HostSettings(host.id));
                                ui.close();
                            }
                            if !host.paired()
                                && host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::OtpPairing))
                                && ui.button("Pair with one-time PIN").clicked()
                            {
                                self.dialog = Dialog::Otp {
                                    host: host.id,
                                    pin: String::new(),
                                    passphrase: String::new(),
                                    error: None,
                                };
                                self.focus_dialog = true;
                                ui.close();
                            }
                            if ui.button("Check again").clicked() {
                                commands.push(Command::Refresh(host.id));
                                ui.close();
                            }
                            if host.paired() && ui.button("Unpair").clicked() {
                                commands.push(Command::Unpair(host.id));
                                ui.close();
                            }
                            if ui.button("Remove").clicked() {
                                commands.push(Command::Remove(host.id));
                                ui.close();
                            }
                        });
                    }
                });
                ui.add_space(16.0);
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("Right-click a PC to unpair or remove it.")
                    .color(OUTLINE)
                    .font(regular(13.0)),
            );
        });
    }

    fn apps(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
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
                    ui.label(RichText::new("Loading the library…").color(ON_SURFACE_VARIANT));
                });
            }
            Apps::Failed(e) => {
                ui.label(RichText::new("The library could not be loaded.").font(semibold(18.0)));
                ui.label(RichText::new(e).color(ERROR));
                if quiet_button(ui, "Try again").clicked() {
                    commands.push(Command::Refresh(id));
                }
            }
            Apps::Loaded(apps) => {
                if apps.is_empty() {
                    ui.label(RichText::new("This PC lists no apps.").color(ON_SURFACE_VARIANT));
                    return;
                }
                ui.label(RichText::new("Game settings: right-click a game, press F2, or use the controller's left face button.").color(ON_SURFACE_VARIANT));
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
                                let focus = self.focus_screen
                                    && (if running.is_some() { is_running } else { row == 0 && i == 0 });
                                if focus {
                                    response.request_focus();
                                    self.focus_screen = false;
                                }
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
                                    } else if running.is_some() {
                                        self.dialog = Dialog::ConfirmQuit {
                                            host: id,
                                            start: Some((app.id, app.name.clone())),
                                            running: running_name(host),
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
                                        if ui.button("Game settings").clicked() {
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
                        ui.add_space(gap);
                    }
                    // A focused-but-unloaded first poster still gets focus next frame.
                    if self.focus_screen && running.is_some() {
                        self.focus_screen = false;
                    }
                });
            }
        }
    }

    fn controllers_page(&mut self, ui: &mut Ui, pads: &crate::gamepad::Gamepads, commands: &mut Vec<Command>) {
        use crate::gamepad::{AXIS_NAMES, BUTTON_MAP, BUTTON_NAMES, Mapping};
        ui.label("Mappings apply to controllers of the same model. Local menu and disconnect chords always use physical buttons.");
        let selected = if self.controller_key.is_empty() { "Choose a controller" } else { &self.controller_mapping.name };
        egui::ComboBox::from_id_salt("controller-model").selected_text(selected).show_ui(ui, |ui| {
            for pad in &pads.pads {
                if ui.selectable_label(pad.key == self.controller_key, &pad.name).clicked() {
                    commands.push(Command::EditController { key: pad.key.clone(), name: pad.name.clone() });
                }
            }
            for (key, mapping) in &pads.mappings {
                if !pads.pads.iter().any(|p| &p.key == key)
                    && ui.selectable_label(*key == self.controller_key, format!("{} (disconnected)", mapping.name)).clicked() {
                    commands.push(Command::EditController { key: key.clone(), name: mapping.name.clone() });
                }
            }
        });
        if self.controller_key.is_empty() {
            ui.label("Connect a controller to create a mapping.");
            return;
        }
        ui.horizontal(|ui| {
            if quiet_button(ui, "Reset mapping").clicked() {
                self.controller_mapping = Mapping { name: self.controller_mapping.name.clone(), ..Default::default() };
            }
            ui.checkbox(&mut self.controller_test, "Test input (pause controller navigation)");
        });
        let mapping = &mut self.controller_mapping;
        scroll(ui, "controller-mapping", |ui| {
            if self.controller_test {
                let connected: Vec<_> = pads.pads.iter().filter(|p| p.key == self.controller_key).collect();
                if connected.is_empty() { ui.label("This controller is disconnected."); }
                for pad in connected {
                    let report = pad.preview(mapping, self.settings.swap_face_buttons);
                    let held = BUTTON_MAP.iter().zip(BUTTON_NAMES).filter_map(|(flag, name)| (report.buttons & flag != 0).then_some(name)).collect::<Vec<_>>().join(", ");
                    ui.label(format!("Controller {}: {}", pad.index + 1, if held.is_empty() { "No buttons held" } else { &held }));
                    ui.label(format!("Left: {}, {}    Right: {}, {}    Triggers: {}, {}", report.ls.0, report.ls.1, report.rs.0, report.rs.1, report.lt, report.rt));
                }
                ui.separator();
            }
            row(ui, "Stick dead zone", "Ignore small movements near the center, then scale smoothly to full travel.", |ui| {
                ui.add(egui::Slider::new(&mut mapping.deadzone, 0..=50).suffix("%"))
            });
            section(ui, "Axes");
            for (i, name) in AXIS_NAMES.iter().enumerate() {
                let source = mapping.axes[i];
                let selected = if source == 0 { "Disabled".to_owned() }
                    else { format!("{}{}", AXIS_NAMES[source.unsigned_abs() as usize - 1], if source < 0 { " (inverted)" } else { "" }) };
                row(ui, name, "Choose the physical axis that drives this output.", |ui| {
                    egui::ComboBox::from_id_salt(("axis-mapping", i)).selected_text(selected).show_ui(ui, |ui| {
                        ui.selectable_value(&mut mapping.axes[i], 0, "Disabled");
                        let range = if i < 4 { 0..4 } else { 4..6 };
                        for j in range {
                            ui.selectable_value(&mut mapping.axes[i], (j + 1) as i8, AXIS_NAMES[j]);
                            if i < 4 { ui.selectable_value(&mut mapping.axes[i], -((j + 1) as i8), format!("{} (inverted)", AXIS_NAMES[j])); }
                        }
                    }).response
                });
            }
            section(ui, "Buttons");
            for (i, name) in BUTTON_NAMES.iter().enumerate() {
                let selected = match mapping.buttons[i] {
                    None => "Default",
                    Some(0) => "Disabled",
                    Some(flag) => BUTTON_NAMES[BUTTON_MAP.iter().position(|b| *b == flag).expect("validated button")],
                };
                row(ui, name, "Default follows the Nintendo layout setting. Explicit mappings take priority.", |ui| {
                    egui::ComboBox::from_id_salt(("button-mapping", i)).selected_text(selected).show_ui(ui, |ui| {
                        ui.selectable_value(&mut mapping.buttons[i], None, "Default");
                        ui.selectable_value(&mut mapping.buttons[i], Some(0), "Disabled");
                        for (target, label) in BUTTON_MAP.iter().zip(BUTTON_NAMES) {
                            ui.selectable_value(&mut mapping.buttons[i], Some(*target), label);
                        }
                    }).response
                });
            }
        });
    }

    fn profiles_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        ui.label("Profiles change the settings used for your next stream. Each PC and game can override them.");
        let before = self.profiles.active;
        let active = self.profiles.entries.iter().find(|p| Some(p.id) == before)
            .map_or("Main settings", |p| p.name.as_str());
        row(ui, "Active profile", "Main settings apply wherever the profile has no override.", |ui| {
            egui::ComboBox::from_id_salt("active-profile").selected_text(active).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.profiles.active, None, "Main settings");
                for profile in &self.profiles.entries {
                    ui.selectable_value(&mut self.profiles.active, Some(profile.id), &profile.name);
                }
            }).response
        });
        if before != self.profiles.active { commands.push(Command::SaveProfiles); }
        let button = primary_button(ui, "New profile");
        if std::mem::take(&mut self.focus_screen) { button.request_focus(); }
        if button.clicked() {
            self.dialog = Dialog::ProfileName { id: None, name: String::new(), settings: Overrides::default(), error: None };
            self.focus_dialog = true;
        }
        ui.add_space(16.0);
        scroll(ui, "profiles", |ui| {
            for profile in &self.profiles.entries {
                ui.push_id(profile.id, |ui| {
                    row(ui, &profile.name, if Some(profile.id) == self.profiles.active { "Active for new streams" } else { "Saved profile" }, |ui| {
                        ui.horizontal(|ui| {
                            if quiet_button(ui, "Edit").clicked() { commands.push(Command::ProfileSettings(profile.id)); }
                            ui.menu_button("More", |ui| {
                                if ui.button("Rename").clicked() {
                                    self.dialog = Dialog::ProfileName { id: Some(profile.id), name: profile.name.clone(), settings: Overrides::default(), error: None };
                                    self.focus_dialog = true;
                                    ui.close();
                                }
                                if ui.button("Duplicate").clicked() {
                                    self.dialog = Dialog::ProfileName { id: None, name: String::new(), settings: profile.settings.clone(), error: None };
                                    self.focus_dialog = true;
                                    ui.close();
                                }
                                if ui.button("Delete").clicked() {
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
    }

    fn settings_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        let display = self.display;
        let tearing = self.tearing_supported;
        let layered = self.screen != Screen::Settings;
        if layered {
            ui.label(match self.screen {
                Screen::ProfileSettings(_) => "Unchecked settings follow your main settings.",
                Screen::HostSettings(_) => "Unchecked settings follow the active profile and main settings.",
                _ => "Unchecked settings follow this PC, the active profile, and main settings.",
            });
            if text_button(ui, "Reset all to inherited").clicked() {
                self.edit_overrides = Overrides::default();
                self.edit_settings = self.inherited_settings.clone();
            }
        } else {
            let active = self.profiles.entries.iter().find(|p| Some(p.id) == self.profiles.active)
                .map_or("Main settings", |p| p.name.as_str());
            if row(ui, "Profiles", &format!("Active: {active}. PCs and games can override individual settings."), |ui| quiet_button(ui, "Manage profiles")).clicked() {
                commands.push(Command::SaveSettings);
                self.go(Screen::Profiles);
                return;
            }
            if row(ui, "Controller mappings", "Customize buttons, axes and stick dead zones for each controller model.", |ui| quiet_button(ui, "Configure controllers")).clicked() {
                commands.push(Command::ControllerSettings);
            }
        }
        let inherited = self.inherited_settings.clone();
        let overrides = &mut self.edit_overrides;
        let s = if layered { &mut self.edit_settings } else { &mut self.settings };
        let before = s.clone();
        let focus_first = std::mem::take(&mut self.focus_screen);
        scroll(ui, "settings", |ui| {
            ui.set_max_width(760.0);
            section(ui, "Video");
            let native = format!("Native ({}x{})", display.width, display.height);
            let current = if s.width == 0 {
                native.clone()
            } else {
                format!("{}x{}", s.width, s.height)
            };
            let first = setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["width", "height"], "Resolution", "The stream's size. Native matches this display.", |ui, s| {
                egui::ComboBox::from_id_salt("resolution")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        let mut v = (s.width, s.height);
                        if ui.selectable_value(&mut v, (0, 0), native).clicked() {
                            (s.width, s.height) = v;
                        }
                        for (w, h) in [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3440, 1440), (3840, 2160)] {
                            let mut v = (s.width, s.height);
                            if ui.selectable_value(&mut v, (w, h), format!("{w}x{h}")).clicked() {
                                (s.width, s.height) = v;
                            }
                        }
                    })
                    .response
            });
            if focus_first {
                first.request_focus();
            }
            if s.width != 0 {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["width", "height"], "Custom resolution", "Width and height in pixels.", |ui, s| {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::DragValue::new(&mut s.width).range(320..=7680));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut s.height).range(240..=4320));
                    }).response
                });
            }
            let display_fps = display.refresh_hz.round() as u32;
            let fps_text = if s.fps == 0 {
                format!("Display ({display_fps} Hz)")
            } else {
                format!("{} fps", s.fps)
            };
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["fps"], "Frame rate", "Display matches this monitor's refresh rate.", |ui, s| {
                egui::ComboBox::from_id_salt("fps")
                    .selected_text(fps_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.fps, 0, format!("Display ({display_fps} Hz)"));
                        for fps in [30, 60, 90, 120, 144, 165, 240] {
                            ui.selectable_value(&mut s.fps, fps, format!("{fps} fps"));
                        }
                    })
                    .response
            });
            let (w, h, fps, bitrate) = stream_mode(s, display);
            let recommended = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, display).3;
            let mut automatic = s.bitrate_kbps == 0;
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["bitrate_kbps"], "Automatic bitrate",
                &format!("{:.0} Mbps for {w}x{h} at {fps} fps.", f64::from(recommended) / 1000.0),
                |ui, s| {
                    let r = toggle(ui, &mut automatic);
                    if r.changed() {
                        s.bitrate_kbps = if automatic { 0 } else { recommended };
                    }
                    r
                },
            );
            if !automatic {
                let mut mbps = bitrate as f32 / 1000.0;
                let note = if s.codec.is_pyrowave() { "PyroWave maintains a minimum bitrate for this size and frame rate." }
                    else { "Higher is sharper and needs a faster network." };
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["bitrate_kbps"], "Bitrate",
                    note,
                    |ui, s| {
                        let r = ui.add(
                            egui::Slider::new(&mut mbps, 0.5..=2000.0)
                                .logarithmic(true)
                                .suffix(" Mbps")
                                .max_decimals(1)
                                .step_by(0.5),
                        );
                        if r.changed() {
                            s.bitrate_kbps = (mbps * 1000.0) as u32;
                        }
                        r
                    },
                );
            }
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["adaptive_bitrate"], "Adapt bitrate to the connection", "Reduce bitrate during sustained congestion and recover gradually. Requires a compatible host.", |ui, s| {
                toggle(ui, &mut s.adaptive_bitrate)
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["adaptive_quality"], "Automatic quality", "Adjust resolution and frame rate under sustained load. Requires a host that supports live stream changes.", |ui, s| {
                egui::ComboBox::from_id_salt("quality").selected_text(s.adaptive_quality.label()).show_ui(ui, |ui| {
                    for mode in [crate::config::QualityMode::Off, crate::config::QualityMode::Motion, crate::config::QualityMode::Detail, crate::config::QualityMode::Battery] {
                        ui.selectable_value(&mut s.adaptive_quality, mode, mode.label());
                    }
                }).response
            });
            if s.adaptive_quality != crate::config::QualityMode::Off {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["quality_min_short_edge"], "Minimum resolution", "Shortest edge, in pixels. Never increases the selected stream size.", |ui, s| {
                    ui.add(egui::DragValue::new(&mut s.quality_min_short_edge).range(256..=2160).speed(2).suffix(" px"))
                });
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["quality_min_fps"], "Minimum frame rate", "Automatic quality stays above this rate.", |ui, s| {
                    ui.add(egui::DragValue::new(&mut s.quality_min_fps).range(15..=240).suffix(" fps"))
                });
            }
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["codec"], "Codec", "Automatic picks HEVC, then H.264. PyroWave needs GPU rendering; restart after selecting it on Linux.", |ui, s| {
                egui::ComboBox::from_id_salt("codec")
                    .selected_text(match s.codec {
                        CodecPreference::Auto => "Automatic",
                        CodecPreference::H264 => "H.264",
                        CodecPreference::Hevc => "HEVC",
                        CodecPreference::Av1 => "AV1",
                        CodecPreference::PyroWave => "PyroWave 4:2:0",
                        CodecPreference::PyroWave444 => "PyroWave 4:4:4",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.codec, CodecPreference::Auto, "Automatic");
                        ui.selectable_value(&mut s.codec, CodecPreference::H264, "H.264");
                        ui.selectable_value(&mut s.codec, CodecPreference::Hevc, "HEVC");
                        ui.selectable_value(&mut s.codec, CodecPreference::Av1, "AV1");
                        ui.selectable_value(&mut s.codec, CodecPreference::PyroWave, "PyroWave 4:2:0");
                        ui.selectable_value(&mut s.codec, CodecPreference::PyroWave444, "PyroWave 4:4:4");
                    })
                    .response
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["upscaling"], "Upscaling", "Enhance smaller SDR streams. HDR uses bilinear. Restart after enabling on Linux.", |ui, s| {
                egui::ComboBox::from_id_salt("upscaling").selected_text(s.upscaling.label()).show_ui(ui, |ui| {
                    for mode in [crate::config::Upscaling::Bilinear, crate::config::Upscaling::Fsr1, crate::config::Upscaling::Sgsr1] {
                        ui.selectable_value(&mut s.upscaling, mode, mode.label());
                    }
                }).response
            });
            if s.upscaling != crate::config::Upscaling::Bilinear {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["upscale_sharpness"], "Sharpness", "Extra sharpening applied by the upscaler.", |ui, s| {
                    ui.add(egui::Slider::new(&mut s.upscale_sharpness, 0..=100).suffix("%"))
                });
            }
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["hdr"], "HDR",
                "Streams 10-bit HDR to an HDR-enabled display. Enable HDR in the operating system first.",
                |ui, s| toggle(ui, &mut s.hdr),
            );
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["experimental_packed_444"], "Experimental full chroma", "Requires the host option and HEVC or AV1. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave(), |ui| toggle(ui, &mut s.experimental_packed_444)).inner);
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["full_range"], "Full color range", "Request the full RGB range from the host.", |ui, s| toggle(ui, &mut s.full_range));
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["decoder"], "Decoder", "Automatic uses hardware decoding when available. PyroWave always uses GPU wavelets.", |ui, s| {
                ui.add_enabled_ui(!s.codec.is_pyrowave(), |ui| egui::ComboBox::from_id_salt("decoder")
                    .selected_text(match s.decoder {
                        DecoderPreference::Auto => "Automatic",
                        DecoderPreference::Software => "Software",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Auto, "Automatic");
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Software, "Software");
                    })
                    .response).inner
            });

            section(ui, "PC display");
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["virtual_display"], "Virtual display", "Use a separate display on hosts that support it.", |ui, s| toggle(ui, &mut s.virtual_display));
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["host_display"], "Display arrangement", "Applied only to normal streams on hosts that support display modes.", |ui, s| {
                egui::ComboBox::from_id_salt("host-display")
                    .selected_text(host_display_name(s.host_display))
                    .show_ui(ui, |ui| {
                        for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary,
                            HostDisplay::Extended, HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
                            ui.selectable_value(&mut s.host_display, mode, host_display_name(mode));
                        }
                    }).response
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["virtual_display_scale"], "PC display scaling", "Text and interface size on the virtual display.", |ui, s| {
                ui.add(egui::Slider::new(&mut s.virtual_display_scale, 100..=300).step_by(25.0).suffix("%"))
            });
            section(ui, "Display");
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["window_mode"], "Window", "Borderless covers the display without changing its mode.", |ui, s| {
                egui::ComboBox::from_id_salt("window")
                    .selected_text(match s.window_mode {
                        WindowMode::Fullscreen => "Fullscreen",
                        WindowMode::Borderless => "Borderless",
                        WindowMode::Windowed => "Windowed",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.window_mode, WindowMode::Fullscreen, "Fullscreen");
                        ui.selectable_value(&mut s.window_mode, WindowMode::Borderless, "Borderless");
                        ui.selectable_value(&mut s.window_mode, WindowMode::Windowed, "Windowed");
                    })
                    .response
            });
            let vrr_note = if tearing || cfg!(not(windows)) {
                "G-Sync, FreeSync or HDMI VRR is on for this display."
            } else {
                "This system cannot present with tearing, which VRR needs here."
            };
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["vrr"], "Variable refresh rate", vrr_note, |ui, s| toggle(ui, &mut s.vrr));
            ui.add_enabled_ui(s.vrr, |ui| {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["present_at_host_times"], "Show frames at the PC's frame times",
                    "Each frame appears at the moment the game made it, plus a steady delay, so motion stays even.",
                    |ui, s| toggle(ui, &mut s.present_at_host_times),
                );
            });
            ui.add_enabled_ui(!s.vrr, |ui| {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["vsync"], "V-Sync", "Without VRR: wait for the display's refresh, no tearing.", |ui, s| {
                    toggle(ui, &mut s.vsync)
                });
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["overlay"], "Performance overlay", "Frame rates, decode and network times. Ctrl+Alt+Shift+S toggles it.", |ui, s| {
                toggle(ui, &mut s.overlay)
            });

            section(ui, "Audio");
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["audio"], "Channels", "Surround needs a host and output device that support it.", |ui, s| {
                egui::ComboBox::from_id_salt("audio")
                    .selected_text(match s.audio {
                        AudioChannels::Stereo => "Stereo",
                        AudioChannels::Surround51 => "5.1 surround",
                        AudioChannels::Surround71 => "7.1 surround",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.audio, AudioChannels::Stereo, "Stereo");
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround51, "5.1 surround");
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround71, "7.1 surround");
                    })
                    .response
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["play_audio_on_host"], "Play audio on the PC too", "Keep sound on the PC's speakers while streaming.", |ui, s| {
                toggle(ui, &mut s.play_audio_on_host)
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["microphone"], "Microphone", "Send this device's default microphone to a compatible host while streaming.", |ui, s| {
                toggle(ui, &mut s.microphone)
            });

            section(ui, "Input");
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["mouse_mode"], "Mouse", "Captured suits games; desktop pointer suits remote work. Ctrl+Alt+Shift+M switches.", |ui, s| {
                egui::ComboBox::from_id_salt("mouse")
                    .selected_text(match s.mouse_mode {
                        MouseMode::Capture => "Captured",
                        MouseMode::Absolute => "Desktop pointer",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Capture, "Captured");
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Absolute, "Desktop pointer");
                    })
                    .response
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["capture_system_keys"], "Send system keys",
                "Alt+Tab, the Windows key and similar go to the PC while input is captured.",
                |ui, s| toggle(ui, &mut s.capture_system_keys),
            );
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["cursor_shapes"], "Local cursor", "Show the PC's cursor without video delay in desktop pointer mode when the host supports it.", |ui, s| {
                toggle(ui, &mut s.cursor_shapes)
            });
            section(ui, "Connection");
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["auto_reconnect"], "Reconnect after an interruption", "Try to resume the same running app up to five times within one minute.", |ui, s| {
                toggle(ui, &mut s.auto_reconnect)
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["network_grace_ms"], "Network interruption grace", "Keep the session alive through brief network interruptions.", |ui, s| {
                egui::ComboBox::from_id_salt("network-grace")
                    .selected_text(format!("{} seconds", s.network_grace_ms / 1000))
                    .show_ui(ui, |ui| {
                        for ms in [0, 3000, 5000, 10000, 15000] {
                            ui.selectable_value(&mut s.network_grace_ms, ms, format!("{} seconds", ms / 1000));
                        }
                    }).response
            });

            section(ui, "Controllers");
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["controllers"], "Controllers", "Every connected controller becomes a controller on the PC.", |ui, s| {
                toggle(ui, &mut s.controllers)
            });
            ui.add_enabled_ui(s.controllers, |ui| {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["swap_face_buttons"], "Nintendo button layout", "Swap A/B and X/Y.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["controller_motion"], "Motion controls", "Send gyro and accelerometer data when a game asks for it.", |ui, s| {
                    toggle(ui, &mut s.controller_motion)
                });
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["controller_audio"], "DualSense USB audio", "Enable waveform haptics and controller speaker audio. Select the four-channel controller output in stream controls if more than one is connected.", |ui, s| {
                    toggle(ui, &mut s.controller_audio)
                });
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["background_controllers"], "Input in the background", "Keep controllers working when Nightjar is not focused.", |ui, s| {
                    toggle(ui, &mut s.background_controllers)
                });
            });
            ui.add_space(18.0);
            ui.label(
                RichText::new(
                    "While streaming: Ctrl+Alt+Shift+Q leaves, +Z releases input, +X toggles fullscreen, +D minimizes. \
                     On a controller, Start+Select+LB+RB leaves.",
                )
                .color(OUTLINE)
                .font(regular(13.0)),
            );
            ui.add_space(24.0);
        });
        if *s != before {
            self.settings_dirty = true;
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, view: &View<'_>, commands: &mut Vec<Command>) {
        let frame = Frame::new()
            .fill(SURFACE_HIGH)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(28))
            .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
        let mut close = false;
        let focus_first = self.focus_dialog;
        let mut focused = false;
        match &mut self.dialog {
            Dialog::None => return,
            Dialog::ProfileName { id, name, settings, error } => {
                egui::Modal::new(Id::unique("profile-name"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(if id.is_some() { "Rename profile" } else { "New profile" }).font(semibold(22.0)));
                        let edit = ui.add(egui::TextEdit::singleline(name).hint_text("Profile name"));
                        if focus_first { edit.request_focus(); focused = true; }
                        if let Some(error) = error { ui.label(RichText::new(error.as_str()).color(ERROR)); }
                        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.horizontal(|ui| {
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
                                    Err(message) => *error = Some(message.into()),
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
                        ui.label(RichText::new(format!("Delete {name}?")).font(semibold(22.0)));
                        ui.label("If this profile is active, new streams will use main settings instead.");
                        ui.horizontal(|ui| {
                            let button = quiet_button(ui, "Cancel");
                            if focus_first { button.request_focus(); focused = true; }
                            if button.clicked() { close = true; }
                            if primary_button(ui, "Delete").clicked() {
                                self.profiles.delete(*id);
                                commands.push(Command::SaveProfiles);
                                close = true;
                            }
                        });
                    });
            }
            Dialog::ConfirmAction { host, app, name } => {
                egui::Modal::new(Id::unique("host-action"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(format!("{name}?")).font(semibold(22.0)));
                        ui.label("This changes the session running on the PC. Ending an app can lose unsaved progress.");
                        ui.horizontal(|ui| {
                            let button = quiet_button(ui, "Cancel");
                            if focus_first {
                                button.request_focus();
                                focused = true;
                            }
                            if button.clicked() { close = true; }
                            if primary_button(ui, "Continue").clicked() {
                                commands.push(Command::ControlAction { host: *host, app: *app });
                                close = true;
                            }
                        });
                    });
            }
            Dialog::Otp { host, pin, passphrase, error } => {
                egui::Modal::new(Id::unique("otp"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new("Pair with one-time PIN").font(semibold(22.0)));
                        ui.label("Enter the PIN and passphrase created on the PC.");
                        let edit = ui.add(egui::TextEdit::singleline(pin).hint_text("PIN"));
                        if focus_first {
                            edit.request_focus();
                            focused = true;
                        }
                        ui.add(egui::TextEdit::singleline(passphrase).password(true).hint_text("Passphrase"));
                        if let Some(error) = error {
                            ui.label(RichText::new(error.as_str()).color(ERROR));
                        }
                        ui.horizontal(|ui| {
                            if primary_button(ui, "Pair").clicked() {
                                if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) || passphrase.chars().count() < 4 {
                                    *error = Some("Enter four PIN digits and a passphrase of at least four characters.".into());
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
                        ui.label(RichText::new("Add a PC").font(semibold(22.0)));
                        ui.label(
                            RichText::new("Its IP address or name, with :port if the host uses another HTTP port.")
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
                        ui.add_space(8.0);
                        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.horizontal(|ui| {
                            if (primary_button(ui, "Add").clicked() || submitted) && !text.trim().is_empty() {
                                commands.push(Command::AddHost(text.trim().to_owned()));
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            Dialog::Pairing { host, pin } => {
                let name = view.hosts.get(*host).map_or("the PC", |h| h.display_name()).to_owned();
                let pin = pin.clone();
                egui::Modal::new(Id::unique("pairing"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(format!("Pair with {name}")).font(semibold(22.0)));
                        if !pin.is_empty() {
                            ui.label(
                                RichText::new("Enter this PIN on the PC: in the host's web page (Sunshine, Apollo, Vibepollo and Rubylight have a PIN page), or in the prompt GeForce Experience shows.")
                                    .color(ON_SURFACE_VARIANT),
                            );
                        }
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            for digit in pin.chars() {
                                let (rect, _) = ui.allocate_exact_size(vec2(64.0, 80.0), Sense::hover());
                                ui.painter().rect_filled(rect, CornerRadius::same(12), SURFACE_HIGHEST);
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    digit,
                                    semibold(44.0),
                                    MOON,
                                );
                            }
                        });
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            spinner(ui, 9.0);
                            ui.label(RichText::new("Waiting for the PC…").color(ON_SURFACE_VARIANT));
                        });
                        ui.add_space(10.0);
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
            }
            Dialog::Message { title, body } => {
                egui::Modal::new(Id::unique("message"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(title.as_str()).font(semibold(22.0)));
                        ui.label(RichText::new(body.as_str()).color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        let r = primary_button(ui, "OK");
                        if focus_first {
                            r.request_focus();
                            focused = true;
                        }
                        if r.clicked() {
                            close = true;
                        }
                    });
            }
            Dialog::ConfirmQuit { host, start, running } => {
                let (host, start, running) = (*host, start.clone(), running.clone());
                egui::Modal::new(Id::unique("confirm-quit"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        let title = match &start {
                            Some((_, name)) => format!("Quit {running} and start {name}?"),
                            None => format!("Quit {running}?"),
                        };
                        ui.label(RichText::new(title).font(semibold(22.0)));
                        ui.label(RichText::new("Unsaved progress in the running app is lost.").color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let r = primary_button(ui, if start.is_some() { "Quit and start" } else { "Quit app" });
                            if focus_first {
                                r.request_focus();
                                focused = true;
                            }
                            if r.clicked() {
                                commands.push(match start {
                                    Some((app, _)) => Command::QuitAndLaunch { host, app },
                                    None => Command::QuitApp(host),
                                });
                                close = true;
                            }
                            if quiet_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
            }
        }
        if focused {
            self.focus_dialog = false;
        }
        if close {
            self.dialog = Dialog::None;
            self.focus_screen = true;
        }
    }

    fn connecting(&mut self, ctx: &egui::Context, c: &ConnectingView, commands: &mut Vec<Command>) {
        let frame = Frame::new()
            .fill(SURFACE_HIGH)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(28))
            .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
        egui::Modal::new(Id::unique("connecting"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(170))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(&c.app).font(semibold(24.0)));
                ui.label(RichText::new(format!("on {}", c.host)).color(ON_SURFACE_VARIANT));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new(&c.stage).color(ON_SURFACE));
                });
                ui.add_space(14.0);
                let r = quiet_button(ui, "Cancel");
                r.request_focus();
                if r.clicked() {
                    commands.push(Command::CancelConnect);
                }
            });
    }
}

/// A vertical scroll area with room for focus rings at its edges.
fn scroll(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    let rect = ui.available_rect_before_wrap();
    let rect = Rect::from_min_max(rect.min - vec2(6.0, 6.0), rect.max + vec2(6.0, 0.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        egui::ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink(false)
            .content_margin(Margin::same(6))
            .show(ui, add);
    });
}

fn host_display_name(mode: HostDisplay) -> &'static str {
    match mode {
        HostDisplay::HostDefault => "Host default",
        HostDisplay::Exclusive => "Virtual display only",
        HostDisplay::ExtendedPrimary => "Extend as primary",
        HostDisplay::Extended => "Extend",
        HostDisplay::ExtendedIsolated => "Extend, isolated",
        HostDisplay::ExtendedPrimaryIsolated => "Extend as primary, isolated",
        HostDisplay::Physical => "Physical display",
    }
}

fn running_name(host: &Host) -> String {
    let Some(running) = host.running_app() else {
        return "the running app".into();
    };
    if let Apps::Loaded(apps) = &host.apps {
        if let Some(app) = apps.iter().find(|a| a.matches_running_app(running, None)) {
            return app.name.clone();
        }
    }
    "the running app".into()
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title.to_uppercase()).font(semibold(13.0)).color(MOON));
    ui.add_space(2.0);
}

fn setting_row(
    ui: &mut Ui,
    settings: &mut Settings,
    layer: Option<(&Settings, &mut Overrides)>,
    keys: &[&str],
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui, &mut Settings) -> Response,
) -> Response {
    let Some((inherited, overrides)) = layer else {
        return row(ui, label, description, |ui| control(ui, settings));
    };
    row(ui, label, description, |ui| {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.set_width(240.0);
            let mut own = overrides.has(keys);
            let response = ui.checkbox(&mut own, "Override");
            if response.changed() && !own {
                overrides.clear(keys);
                *settings = overrides.apply(inherited);
            }
            let value = ui.add_enabled_ui(own, |ui| control(ui, settings)).inner;
            if own { overrides.capture(settings, keys); }
            response.union(value)
        }).inner
    })
}

/// A settings row: label and description on the left, the control on the right.
fn row(ui: &mut Ui, label: &str, description: &str, control: impl FnOnce(&mut Ui) -> Response) -> Response {
    let mut out = None;
    Frame::new()
        .fill(SURFACE)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let control_width = 240.0;
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - control_width - 16.0).max(200.0));
                    ui.label(RichText::new(label).font(semibold(15.0)).color(ON_SURFACE));
                    ui.label(RichText::new(description).font(regular(13.0)).color(ON_SURFACE_VARIANT));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    out = Some(control(ui));
                });
            });
        });
    ui.add_space(6.0);
    out.expect("control drawn")
}

fn host_card(ui: &mut Ui, host: &Host) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, CARD_H), Sense::click());
    let painter = ui.painter_at(rect.expand(6.0));
    let fill = if response.hovered() || response.has_focus() {
        SURFACE_HIGH
    } else {
        SURFACE
    };
    painter.rect_filled(rect, CornerRadius::same(12), fill);
    let (status, color) = match (host.reach, host.paired()) {
        (Reach::Offline, _) => ("Offline".to_owned(), OUTLINE),
        (Reach::Unknown, _) => ("Checking…".to_owned(), OUTLINE),
        (Reach::Online, false) => ("Not paired · select to pair".to_owned(), TERTIARY),
        (Reach::Online, true) => match host.running_app() {
            Some(_) => ("Paired · an app is running".to_owned(), MOON),
            None => ("Paired · ready".to_owned(), MOON),
        },
    };
    // The moon dot: full when ready, a ring otherwise.
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + 30.0);
    if host.reach == Reach::Online && host.paired() {
        painter.circle_filled(dot, 7.0, MOON);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, color));
    }
    painter.text(
        Pos2::new(rect.left() + 44.0, rect.top() + 30.0),
        Align2::LEFT_CENTER,
        host.display_name(),
        semibold(19.0),
        ON_SURFACE,
    );
    painter.text(
        Pos2::new(rect.left() + 20.0, rect.top() + 62.0),
        Align2::LEFT_CENTER,
        &status,
        regular(14.0),
        color,
    );
    let detail = match host.info.as_ref().and_then(|i| i.gpu_type.clone()) {
        Some(gpu) if !gpu.is_empty() => format!("{}  ·  {gpu}", host.record.address),
        _ => host.record.address.clone(),
    };
    painter.text(
        Pos2::new(rect.left() + 20.0, rect.top() + 88.0),
        Align2::LEFT_CENTER,
        detail,
        regular(13.0),
        OUTLINE,
    );
    focus_ring(ui, rect, &response, 12);
    response
}

fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H + 34.0), Sense::click());
    let image = Rect::from_min_size(rect.min, vec2(POSTER_W, POSTER_H));
    let painter = ui.painter_at(rect.expand(6.0));
    match art {
        Some(Art::Ready(texture)) => {
            egui::Image::from_texture((texture.id(), image.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, image);
        }
        _ => {
            // No art from the host: the night surface with the banner's orbits.
            painter.rect_filled(image, CornerRadius::same(10), SURFACE);
            let orbits = ui.painter_at(image.shrink(1.0));
            for r in [70.0, 105.0, 140.0] {
                orbits.circle_stroke(image.right_bottom(), r, Stroke::new(1.5, ORBIT));
            }
            let initials: String = name
                .split_whitespace()
                .filter_map(|w| w.chars().next())
                .take(2)
                .collect::<String>()
                .to_uppercase();
            painter.text(image.center(), Align2::CENTER_CENTER, initials, semibold(40.0), OUTLINE);
        }
    }
    if running {
        let badge = Rect::from_min_size(image.left_top() + vec2(10.0, 10.0), vec2(76.0, 24.0));
        painter.rect_filled(badge, CornerRadius::same(12), MOON);
        painter.text(badge.center(), Align2::CENTER_CENTER, "Running", semibold(12.5), ON_MOON);
    }
    let label_color = if response.has_focus() { MOON } else { ON_SURFACE };
    let galley = painter.layout(name.to_owned(), semibold(14.5), label_color, POSTER_W);
    painter.galley(Pos2::new(rect.left() + 2.0, image.bottom() + 8.0), galley, label_color);
    focus_ring(ui, image, &response, 10);
    response
}

trait UiExt {
    fn ui(self, ui: &mut Ui) -> Response;
}

impl UiExt for egui::Image<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.add(self)
    }
}
