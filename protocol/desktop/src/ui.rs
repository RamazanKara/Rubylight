//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2,
};

use crate::config::{
    AudioChannels, CodecPreference, DecoderPreference, HostDisplay, MouseMode, Overrides, Profiles, Settings, WindowMode,
};
use crate::console::{self, EndAction, GameState, Playing, Source};
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
    /// Name a PC on this device only; empty brings back the PC's own name.
    Rename { host: u64, alias: String },
    HostSettings(u64),
    SaveHostSettings(u64),
    AppSettings { host: u64, app: nightjar_core::http::App },
    SaveAppSettings(u64),
    ProfileSettings(u64),
    SaveProfile(u64),
    SaveProfiles,
    NetworkTest { host: u64, calibrate: bool },
    CancelNetworkTest,
    ClearPace(u64),
    ControllerSettings,
    EditController { key: String, name: String },
    SaveControllerMapping,
    Shortcuts,
    SaveShortcuts,
    ImportShortcuts,
    ExportShortcuts,
    CancelPair,
    AddHost(String),
    Unpair(u64),
    Remove(u64),
    Refresh(u64),
    OpenApps(u64),
    /// Fetch a cover that scrolled into view (a new `version` fetches again).
    RequestArt { host: u64, app: i32, version: String },
    /// Close the running game through the PC's native API.
    CloseGame(u64),
    SaveSettings,
    CancelConnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Games to continue, the merged library and the PCs.
    Home,
    /// One game on one PC (`app` is the native library's app id).
    Game { host: u64, app: u32 },
    Hosts,
    Apps(u64),
    Settings,
    HostSettings(u64),
    AppSettings(u64),
    Profiles,
    ProfileSettings(u64),
    Controllers,
    Shortcuts,
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
    NetworkTest { stage: String },
    Otp { host: u64, pin: String, passphrase: String, error: Option<String> },
    Message { title: String, body: String },
    ConfirmQuit { host: u64, start: Option<(i32, String)>, running: String },
    ConfirmAction { host: u64, app: i32, name: String },
    ConfirmClose { host: u64, name: String },
    ProfileName { id: Option<u64>, name: String, settings: Overrides, error: Option<String> },
    DeleteProfile { id: u64, name: String },
    Shortcut { id: Option<String>, name: String, chord: String, error: Option<String> },
    DeleteShortcut { id: String, name: String },
    ImportShortcuts(crate::shortcuts::Shortcuts),
    /// A PC's actions in one list, in the order of `host_options`.
    HostOptions { host: u64 },
    Rename { host: u64, text: String },
    ConfirmRemove { host: u64, name: String },
    ConfirmUnpair { host: u64, name: String },
    /// Pairing did not work: what to do about it, and a way to try again.
    PairError { host: u64, body: String, one_time: bool },
}

/// The header's top-level destinations, left to right.
pub const DESTINATIONS: [(&str, Screen); 3] = [("Home", Screen::Home), ("PCs", Screen::Hosts), ("Settings", Screen::Settings)];

/// The settings sections in the order Android lists them (App only on the main settings).
const SETTINGS_SECTIONS: [&str; 7] = ["Video", "Display & image", "Controls", "Audio", "Streaming", "App", "Advanced"];

/// What a PC's options list can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostOption {
    OpenLibrary,
    QuitApp,
    Pair,
    PairOtp,
    Wake,
    Settings,
    Rename,
    Unpair,
    CheckAgain,
    Remove,
}

impl HostOption {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenLibrary => "Open library",
            Self::QuitApp => "Quit running app",
            Self::Pair => "Pair",
            Self::PairOtp => "Pair with one-time PIN",
            Self::Wake => "Wake up",
            Self::Settings => "PC settings",
            Self::Rename => "Rename",
            Self::Unpair => "Unpair",
            Self::CheckAgain => "Check again",
            Self::Remove => "Remove PC",
        }
    }
}

/// The options a PC offers now, in the order they are listed.
pub fn host_options(host: &Host) -> Vec<HostOption> {
    let mut options = Vec::new();
    if host.paired() {
        options.push(HostOption::OpenLibrary);
        if host.running_app().is_some() {
            options.push(HostOption::QuitApp);
        }
    } else {
        options.push(HostOption::Pair);
        if host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::OtpPairing)) {
            options.push(HostOption::PairOtp);
        }
    }
    if host.reach == Reach::Offline && host.record.mac.is_some() {
        options.push(HostOption::Wake);
    }
    options.push(HostOption::Settings);
    options.push(HostOption::Rename);
    // Unpairing asks the PC to forget this device, so it needs the PC to be reachable.
    if host.paired() && host.reach == Reach::Online {
        options.push(HostOption::Unpair);
    }
    options.push(HostOption::CheckAgain);
    options.push(HostOption::Remove);
    options
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
    pub network: crate::network::Network,
    home_wifi_name: String,
    pub edit_settings: Settings,
    pub inherited_settings: Settings,
    pub edit_overrides: Overrides,
    pub editing_app: Option<nightjar_core::http::App>,
    pub edit_name: String,
    pub settings_dirty: bool,
    pub controller_key: String,
    pub controller_mapping: crate::gamepad::Mapping,
    pub controller_test: bool,
    pub shortcuts: crate::shortcuts::Shortcuts,
    /// Games streamed before, latest first.
    pub recents: Vec<crate::config::Recent>,
    logo: Option<TextureHandle>,
    focus_screen: bool,
    focus_dialog: bool,
    settings_return: Screen,
    /// Where Back goes: the screens left by moving forward, with the item that had focus.
    trail: Vec<(Screen, Option<String>)>,
    /// The item with focus on the current screen, by its key.
    focused_key: Option<String>,
    /// The item to focus when the screen is drawn again after Back.
    restore_key: Option<String>,
    /// Some PC has a library to show, so Home has something on it.
    home_ready: bool,
    /// The width the header's actions took last frame, to keep them at the right edge.
    header_width: f32,
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

/// A small pill that jumps to a section of the settings.
fn chip(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(ON_SURFACE).font(semibold(13.5)))
        .fill(SURFACE)
        .corner_radius(CornerRadius::same(14))
        .min_size(vec2(0.0, 28.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 14);
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
    pub fn new(settings: Settings, profiles: Profiles, shortcuts: crate::shortcuts::Shortcuts, display: Display) -> Self {
        Self {
            screen: Screen::Hosts,
            dialog: Dialog::None,
            settings,
            profiles,
            network: crate::network::Network::default(),
            home_wifi_name: String::new(),
            edit_settings: Settings::default(),
            inherited_settings: Settings::default(),
            edit_overrides: Overrides::default(),
            editing_app: None,
            edit_name: String::new(),
            settings_dirty: false,
            controller_key: String::new(),
            controller_mapping: crate::gamepad::Mapping::default(),
            controller_test: false,
            shortcuts,
            recents: Vec::new(),
            logo: None,
            focus_screen: true,
            focus_dialog: false,
            settings_return: Screen::Hosts,
            trail: Vec::new(),
            focused_key: None,
            restore_key: None,
            home_ready: false,
            header_width: 0.0,
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
        self.focused_key = None;
        self.restore_key = None;
    }

    /// Start button and the Settings link: Settings from anywhere, once.
    pub fn open_settings(&mut self) {
        match self.screen {
            Screen::Settings => {}
            Screen::Profiles | Screen::Controllers | Screen::Shortcuts => self.leave_subpage(),
            Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.go(Screen::Settings),
            _ => self.switch(Screen::Settings),
        }
    }

    /// Moves to another top-level destination (Home, PCs, Settings). They replace each other, so
    /// they never stack: Back from a destination that is not the root goes to the root.
    pub fn switch(&mut self, destination: Screen) {
        if destination == self.screen {
            return;
        }
        let root = self.root();
        if destination == root {
            // Back to the root this came from (with its item focused), else a fresh one.
            match self.trail.iter().rposition(|(screen, _)| *screen == root) {
                Some(at) => {
                    self.trail.truncate(at + 1);
                    self.pop(root);
                }
                None => {
                    self.trail.clear();
                    self.go(root);
                }
            }
            return;
        }
        let remembered = if self.screen == root {
            Some((root, self.focused_key.take()))
        } else {
            self.trail.iter().find(|(screen, _)| *screen == root).cloned()
        };
        self.trail.clear();
        self.trail.extend(remembered);
        self.go(destination);
    }

    /// Back from a Settings page (Profiles, Controllers, Shortcuts) to the Settings row that
    /// opened it.
    fn leave_subpage(&mut self) {
        if self.trail.last().is_some_and(|(screen, _)| *screen == Screen::Settings) {
            self.pop(Screen::Settings);
        } else {
            self.go(Screen::Settings);
        }
    }

    /// Moves forward, so Back returns here with the same item focused.
    pub fn open(&mut self, screen: Screen) {
        if screen == self.screen {
            return;
        }
        if self.trail.len() >= 16 {
            self.trail.remove(0);
        }
        self.trail.push((self.screen, self.focused_key.take()));
        self.go(screen);
    }

    /// Back to where the user came from, else `fallback`.
    fn pop(&mut self, fallback: Screen) {
        match self.trail.pop() {
            Some((screen, key)) => {
                self.go(screen);
                self.restore_key = key;
            }
            None => self.go(fallback),
        }
    }

    /// Where the app starts and where Back ends: Home when it is on and has games.
    pub fn root(&self) -> Screen {
        if self.settings.console_home && self.home_ready {
            Screen::Home
        } else {
            Screen::Hosts
        }
    }

    /// Gives an item focus: the first one on a new screen, or the one that had it before the
    /// user went forward. Records which item has focus so `open` can remember it.
    fn focus_item(&mut self, response: &Response, key: &str, first: bool) {
        if response.has_focus() && self.focused_key.as_deref() != Some(key) {
            self.focused_key = Some(key.to_owned());
        }
        if response.gained_focus() {
            response.scroll_to_me(None);
        }
        if !response.enabled() {
            return;
        }
        if self.restore_key.as_deref() == Some(key) {
            self.restore_key = None;
            self.focus_screen = false;
            response.request_focus();
            response.scroll_to_me(None);
        } else if first && self.focus_screen && self.restore_key.is_none() {
            self.focus_screen = false;
            response.request_focus();
        }
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
                Screen::Home => {}
                Screen::Hosts => {
                    if !self.trail.is_empty() || self.root() == Screen::Home {
                        let root = self.root();
                        self.pop(root);
                    }
                }
                Screen::Apps(_) | Screen::Game { .. } => {
                    let root = self.root();
                    self.pop(root);
                }
                Screen::Settings => {
                    commands.push(Command::SaveSettings);
                    let root = self.root();
                    self.pop(root);
                }
                Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => {
                    commands.extend(self.screen.save_command());
                    self.go(self.settings_return);
                }
                Screen::Profiles | Screen::Shortcuts => self.leave_subpage(),
                Screen::Controllers => {
                    commands.push(Command::SaveControllerMapping);
                    self.leave_subpage();
                }
            },
            Dialog::NetworkTest { .. } => { commands.push(Command::CancelNetworkTest); self.dialog = Dialog::None; }
            Dialog::Pairing { .. } => {
                commands.push(Command::CancelPair);
                self.dialog = Dialog::None;
            }
            _ => self.close_dialog(),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, view: View<'_>) -> Vec<Command> {
        let mut commands = Vec::new();
        let ctx = ui.ctx().clone();
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        self.home_ready = console::has_library(&sources);
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if escape && view.connecting.is_none() {
            self.back(&mut commands);
        }
        if self.screen == Screen::Hosts
            && matches!(self.dialog, Dialog::None)
            && view.connecting.is_none()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(Key::N))
        {
            self.add_host_dialog();
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
                Screen::Home => self.home(ui, &view, &mut commands, logo),
                Screen::Game { host, app } => self.game(ui, &view, host, app, &mut commands),
                Screen::Hosts => self.hosts(ui, &view, &mut commands, logo),
                Screen::Apps(id) => self.apps(ui, &view, id, &mut commands),
                Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.settings_page(ui, &view, &mut commands),
                Screen::Profiles => self.profiles_page(ui, &mut commands),
                Screen::Controllers => self.controllers_page(ui, view.pads, &mut commands),
                Screen::Shortcuts => self.shortcuts_page(ui, &mut commands),
            }
        });
        if blocked {
            self.focus_screen = held_focus;
        } else if self.restore_key.take().is_some() {
            // The remembered item is gone (or not loaded yet): the first one takes focus.
            ctx.request_repaint();
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
                Screen::Home => {
                    egui::Image::from_texture((logo, vec2(30.0, 30.0))).ui(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new("Nightjar").font(semibold(22.0)).color(PAPER));
                }
                Screen::Game { host, app } => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
                    }
                    let name = view
                        .hosts
                        .get(host)
                        .and_then(|h| h.native.library.as_ref())
                        .and_then(|l| l.games.iter().find(|g| g.app_id == app))
                        .map_or("", |g| g.title.as_str());
                    ui.label(RichText::new(name).font(semibold(22.0)).color(PAPER));
                }
                Screen::Hosts => {
                    egui::Image::from_texture((logo, vec2(30.0, 30.0))).ui(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new("Nightjar").font(semibold(22.0)).color(PAPER));
                }
                Screen::Apps(id) => {
                    if text_button(ui, "‹  PCs").clicked() {
                        self.back(commands);
                    }
                    let name = view.hosts.get(id).map_or("", |h| h.display_name()).to_owned();
                    ui.label(RichText::new(name).font(semibold(22.0)).color(PAPER));
                }
                Screen::Settings => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
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
                        self.back(commands);
                    }
                    ui.label(RichText::new("Profiles").font(semibold(22.0)).color(PAPER));
                }
                Screen::Controllers => {
                    if text_button(ui, "‹  Settings").clicked() { self.back(commands); }
                    ui.label(RichText::new("Controller mappings").font(semibold(22.0)).color(PAPER));
                }
                Screen::Shortcuts => {
                    if text_button(ui, "‹  Settings").clicked() { self.back(commands); }
                    ui.label(RichText::new("Keyboard shortcuts").font(semibold(22.0)).color(PAPER));
                }
            }
            // The actions sit at the right edge but are registered left to right, so Tab walks
            // Home, PCs, Settings, Add PC. Their width is last frame's; a change repaints.
            let slot = ui.available_rect_before_wrap();
            let width = self.header_width.min(slot.width());
            let rect = Rect::from_min_max(pos2(slot.right() - width, slot.top()), slot.right_bottom());
            let used = ui
                .scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)), |ui| {
                    ui.spacing_mut().item_spacing.x = 22.0;
                    match self.screen {
                        Screen::Home | Screen::Hosts | Screen::Settings => {
                            for (name, screen) in DESTINATIONS {
                                if screen != Screen::Home || self.home_ready || self.screen == Screen::Home {
                                    self.destination(ui, name, screen, commands);
                                }
                            }
                            if self.screen == Screen::Hosts && text_button(ui, "Add PC").clicked() {
                                self.add_host_dialog();
                            }
                        }
                        Screen::Game { .. } => {}
                        Screen::Apps(id) => {
                            if let Some(host) = view.hosts.get(id) {
                                if text_button(ui, "Refresh").clicked() {
                                    commands.push(Command::Refresh(id));
                                }
                                if host.running_app().is_some() && text_button(ui, "Quit running app").clicked() {
                                    self.dialog = Dialog::ConfirmQuit {
                                        host: id,
                                        start: None,
                                        running: running_name(host),
                                    };
                                    self.focus_dialog = true;
                                }
                            }
                            if text_button(ui, "PC settings").clicked() {
                                commands.push(Command::HostSettings(id));
                            }
                        }
                        Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::Profiles | Screen::ProfileSettings(_) | Screen::Controllers | Screen::Shortcuts => {}
                    }
                    ui.min_rect().width()
                })
                .inner;
            if (used - self.header_width).abs() > 0.5 {
                self.header_width = used;
                ui.ctx().request_repaint();
            }
        });
    }

    /// One of the header's top-level destinations; the current one is marked.
    fn destination(&mut self, ui: &mut Ui, name: &str, screen: Screen, commands: &mut Vec<Command>) {
        let current = self.screen == screen;
        let text = RichText::new(name)
            .font(semibold(15.0))
            .color(if current { PAPER } else { ON_SURFACE_VARIANT });
        let response = ui.add(egui::Button::new(if current { text.underline() } else { text }).frame(false).min_size(vec2(0.0, 36.0)));
        focus_ring(ui, response.rect, &response, 8);
        if response.clicked() && !current {
            if let Some(save) = self.screen.save_command() {
                commands.push(save);
            }
            self.switch(screen);
        }
    }

    fn add_host_dialog(&mut self) {
        self.dialog = Dialog::AddHost {
            text: String::new(),
            error: None,
        };
        self.focus_dialog = true;
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
                        let response = host_card(ui, host, None);
                        let options = options_button(ui, response.rect, response.id.with("options"));
                        self.focus_item(&response, &format!("host:{}", host.id), row == 0 && i == 0);
                        self.focus_item(&options, &format!("options:{}", host.id), false);
                        if response.clicked() {
                            host_clicked(host, commands);
                        }
                        if options.clicked() || options_requested(ui, &[&response, &options]) {
                            self.dialog = Dialog::HostOptions { host: host.id };
                            self.focus_dialog = true;
                        }
                    }
                });
                ui.add_space(16.0);
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("Options for a PC: F2, right-click, the three dots, or the controller's X button. Ctrl+N adds a PC.")
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

    fn home(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
        if view.hosts.list.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                egui::Image::from_texture((logo, vec2(72.0, 72.0))).ui(ui);
                ui.add_space(14.0);
                ui.label(RichText::new("No hosts yet").font(semibold(18.0)));
                ui.label(RichText::new("Pair Nightjar with a PC to see its games here.").color(ON_SURFACE_VARIANT));
                ui.add_space(12.0);
                let r = primary_button(ui, "Open hosts");
                self.focus_item(&r, "open-hosts", true);
                if r.clicked() {
                    self.open(Screen::Hosts);
                }
            });
            return;
        }
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let entries = console::merge_library(&sources);
        let playing = console::continue_playing(&sources);
        let recent = console::recently_played(&sources, &self.recents, &playing, 6);
        let loading = view.hosts.list.iter().any(|h| {
            h.reach == Reach::Online && h.paired() && h.native_api() && h.native.library.is_none() && h.native.error.is_none()
        });
        scroll(ui, "home", |ui| {
            // A failed read stays on screen beside the last good data.
            for host in &view.hosts.list {
                if let Some(error) = &host.native.error {
                    ui.label(RichText::new(format!("{}: {error}", host.display_name())).color(ERROR));
                }
            }
            let mut first = true;
            if !playing.is_empty() {
                section(ui, "Continue playing");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                    for item in &playing {
                        let (line, running) = match item.playing {
                            Playing::Running => (format!("Running on {}", item.host_name), true),
                            Playing::Suspended => (format!("Suspended on {}", item.host_name), false),
                        };
                        let response = continue_card(ui, &item.title, &line, running);
                        self.focus_item(&response, &format!("continue:{}:{}", item.host, item.app_id), std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                    }
                });
            }
            if !recent.is_empty() {
                section(ui, "Recently played");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                    for item in &recent {
                        let line = if item.online { format!("On {}", item.host_name) } else { format!("{} is away", item.host_name) };
                        let response = continue_card(ui, &item.title, &line, false);
                        self.focus_item(&response, &format!("recent:{}:{}", item.host, item.app_id), std::mem::take(&mut first));
                        if response.clicked() {
                            self.open(Screen::Game { host: item.host, app: item.app_id });
                        }
                    }
                });
            }
            section(ui, "Library");
            if entries.is_empty() {
                if loading {
                    ui.horizontal(|ui| {
                        spinner(ui, 10.0);
                        ui.label(RichText::new("Loading the games…").color(ON_SURFACE_VARIANT));
                    });
                } else {
                    ui.label(RichText::new("No games to show. Games appear here once a Rubylight PC is online.").color(ON_SURFACE_VARIANT));
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(20.0, 20.0);
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
                }
            });
            section(ui, "Hosts");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                for (host, source) in view.hosts.list.iter().zip(&sources) {
                    let line = console::host_line(source).text();
                    let response = host_card(ui, host, Some(&line));
                    self.focus_item(&response, &format!("host:{}", host.id), std::mem::take(&mut first));
                    if response.clicked() {
                        host_clicked(host, commands);
                    }
                }
            });
            ui.add_space(8.0);
        });
    }

    /// Leaves the current screen for the one it was opened from.
    fn leave(&mut self) {
        let root = self.root();
        self.pop(root);
    }

    fn game(&mut self, ui: &mut Ui, view: &View<'_>, host_id: u64, app_id: u32, commands: &mut Vec<Command>) {
        let Some(host) = view.hosts.get(host_id) else {
            self.leave();
            return;
        };
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        let source = sources.iter().find(|s| s.id == host_id).expect("source of a listed host");
        let Some(game) = source.library.and_then(|l| l.games.iter().find(|g| g.app_id == app_id)) else {
            ui.label(RichText::new("This game is no longer in the PC's library.").font(semibold(18.0)));
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
            paint_cover(ui, rect, &game.title, art, detail.state != GameState::Idle);
            request_cover(ui, view, host_id, app_id, &game.artwork.version, &response, commands);
            ui.vertical(|ui| {
                ui.set_max_width(640.0);
                ui.label(RichText::new(&game.title).font(semibold(30.0)).color(ON_SURFACE));
                if let Some(store) = &game.store {
                    ui.label(RichText::new(&store.name).color(ON_SURFACE_VARIANT));
                }
                let (status, color) = match detail.state {
                    GameState::Running => (format!("Running on {}", source.name), MOON),
                    GameState::Suspended => (format!("Suspended on {}", source.name), TERTIARY),
                    GameState::Idle => (format!("On {}", source.name), ON_SURFACE_VARIANT),
                };
                ui.label(RichText::new(status).font(semibold(16.0)).color(color));
                ui.add_space(8.0);
                if host.reach == Reach::Offline {
                    ui.label(RichText::new("This PC is offline. Your saved library is still here. Reconnect to play.").color(ERROR));
                } else if host.reach == Reach::Unknown {
                    ui.label(RichText::new("Checking the PC…").color(ON_SURFACE_VARIANT));
                } else if !source.paired {
                    ui.label(RichText::new("Pair Nightjar with this PC to play.").color(ERROR));
                }
                if let Some(other) = &other_running {
                    ui.label(RichText::new(format!("{other} is running. Playing this game closes it.")).color(TERTIARY));
                }
                ui.add_space(8.0);
                let can_play = detail.can_start && classic.is_some();
                ui.horizontal(|ui| {
                    let r = ui.add_enabled_ui(can_play, |ui| primary_button(ui, if resumes { "Resume" } else { "Play" })).inner;
                    self.focus_item(&r, "play", true);
                    if r.clicked() {
                        if let Some(app) = &classic {
                            if resumes {
                                commands.push(Command::Launch { host: host_id, app: app.id, resume: true });
                            } else if let Some(running) = &other_running {
                                self.dialog = Dialog::ConfirmQuit {
                                    host: host_id,
                                    start: Some((app.id, game.title.clone())),
                                    running: running.clone(),
                                };
                                self.focus_dialog = true;
                            } else {
                                commands.push(Command::Launch { host: host_id, app: app.id, resume: false });
                            }
                        }
                    }
                    if detail.end != EndAction::None {
                        let closing = host.native.acting;
                        let label = if detail.end == EndAction::CloseGame { "Close game" } else { "Quit game" };
                        let r = ui.add_enabled_ui(!closing, |ui| quiet_button(ui, label)).inner;
                        self.focus_item(&r, "end", !can_play);
                        if r.clicked() {
                            self.dialog = if detail.end == EndAction::CloseGame {
                                Dialog::ConfirmClose { host: host_id, name: game.title.clone() }
                            } else {
                                Dialog::ConfirmQuit { host: host_id, start: None, running: game.title.clone() }
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
                                ui.label(RichText::new("Loading the app list…").color(ON_SURFACE_VARIANT));
                            });
                        }
                        Apps::Failed(_) | Apps::Loaded(_) => {
                            let why = match &host.apps {
                                Apps::Failed(e) => format!("The app list could not be loaded. {e}"),
                                _ => "The PC's app list has no entry for this game.".to_owned(),
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
                    .map_or("Main settings", |p| p.name.as_str());
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
                        None => ui.add_enabled(false, egui::Button::new("Game settings")),
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

    fn shortcuts_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
        ui.label("Send saved chords to the PC from stream controls. Import and export use Nightjar Android's shortcut format.");
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
        scroll(ui, "shortcuts", |ui| {
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

    fn controllers_page(&mut self, ui: &mut Ui, pads: &crate::gamepad::Gamepads, commands: &mut Vec<Command>) {
        use crate::gamepad::{AXIS_NAMES, BUTTON_MAP, BUTTON_NAMES, Mapping};
        ui.label("Mappings apply to controllers of the same model. Local menu and disconnect chords always use physical buttons.");
        let selected = if self.controller_key.is_empty() { "Choose a controller" } else { &self.controller_mapping.name };
        let chooser = egui::ComboBox::from_id_salt("controller-model").selected_text(selected).show_ui(ui, |ui| {
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
        if std::mem::take(&mut self.focus_screen) { chooser.response.request_focus(); }
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
        let before_profiles = self.profiles.clone();
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
            if self.network.link_mbps > 0 { ui.label(format!("Link speed: {} Mbps", self.network.link_mbps)); }
            let selected = self.profiles.selected(self.network.kind(&self.profiles.home_wifi));
            let selected_name = selected.map(|id| self.profiles.entries.iter().find(|p| p.id == id)
                .map_or("Missing profile; main settings apply", |p| p.name.as_str())).unwrap_or("Main settings");
            ui.label(format!("Next stream: {selected_name}, then PC and game overrides."));
            row(ui, "Choose automatically", "Select a profile when a stream starts on this network.", |ui| toggle(ui, &mut self.profiles.automatic));
            ui.add_enabled_ui(self.profiles.automatic, |ui| {
                for (id, title, selected) in [("mobile-profile", "Mobile or metered", &mut self.profiles.mobile),
                    ("home-profile", "Home Wi-Fi", &mut self.profiles.home), ("wifi-profile", "Other Wi-Fi", &mut self.profiles.wifi)] {
                    let name = self.profiles.entries.iter().find(|p| Some(p.id) == *selected).map_or("Active profile", |p| p.name.as_str());
                    row(ui, title, "An unset rule follows your active profile.", |ui| {
                        egui::ComboBox::from_id_salt(id).selected_text(name).show_ui(ui, |ui| {
                            ui.selectable_value(selected, None, "Active profile");
                            for profile in &self.profiles.entries { ui.selectable_value(selected, Some(profile.id), &profile.name); }
                        }).response
                    });
                }
                ui.label("Home Wi-Fi names stay on this device. Names are case-sensitive.");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.home_wifi_name).hint_text("Wi-Fi name").char_limit(32));
                    let valid = !self.home_wifi_name.is_empty() && self.home_wifi_name.len() <= 32
                        && !self.profiles.home_wifi.contains(&self.home_wifi_name) && self.profiles.home_wifi.len() < 50;
                    if ui.add_enabled(valid, egui::Button::new("Add")).clicked() {
                        self.profiles.home_wifi.push(std::mem::take(&mut self.home_wifi_name));
                    }
                    if let Some(ssid) = &self.network.ssid {
                        if ui.add_enabled(!self.profiles.home_wifi.contains(ssid) && self.profiles.home_wifi.len() < 50, egui::Button::new("Add current Wi-Fi")).clicked() {
                            self.profiles.home_wifi.push(ssid.clone());
                        }
                    }
                });
                self.profiles.home_wifi.retain(|name| {
                    !ui.horizontal(|ui| { ui.label(name); text_button(ui, "Remove").clicked() }).inner
                });
            });
            row(ui, "Mobile data bitrate limit", "Also applies to metered Wi-Fi or Ethernet, after all profile, PC and game overrides. 0 means no limit.", |ui| {
                ui.add(egui::DragValue::new(&mut self.profiles.mobile_limit_kbps).range(0..=500_000).speed(100).suffix(" kbps"))
            });
            section(ui, "Saved profiles");
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
        if self.profiles != before_profiles { commands.push(Command::SaveProfiles); }
    }

    fn settings_page(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
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
            let manage = row(ui, "Profiles", &format!("Active: {active}. PCs and games can override individual settings."), |ui| quiet_button(ui, "Manage profiles"));
            self.focus_item(&manage, "profiles-link", false);
            if manage.clicked() {
                commands.push(Command::SaveSettings);
                self.open(Screen::Profiles);
                return;
            }
        }
        let mut jump: Option<&'static str> = None;
        ui.horizontal_wrapped(|ui| {
            for name in SETTINGS_SECTIONS {
                // A profile or PC override has no app-wide section.
                if layered && name == "App" {
                    continue;
                }
                if chip(ui, name).clicked() {
                    jump = Some(name);
                }
            }
        });
        ui.add_space(6.0);
        let mut links: Vec<(&str, Response)> = Vec::new();
        let test_host = if let Screen::HostSettings(id) = self.screen { view.hosts.get(id) } else { None };
        let inherited = self.inherited_settings.clone();
        let overrides = &mut self.edit_overrides;
        let s = if layered { &mut self.edit_settings } else { &mut self.settings };
        let before = s.clone();
        // Coming back from a sub-page, the link that opened it takes focus instead.
        let focus_first = self.focus_screen && self.restore_key.is_none();
        if focus_first {
            self.focus_screen = false;
        }
        scroll(ui, "settings", |ui| {
            ui.set_max_width(760.0);
            section_at(ui, "Video", &mut jump);
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
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["video_scaling"], "Video size", "Fit shows the whole stream; Fill crops the edges; Native uses one display pixel per stream pixel.", |ui, s| {
                use crate::config::VideoScaling;
                egui::ComboBox::from_id_salt("video-scaling").selected_text(s.video_scaling.label()).show_ui(ui, |ui| {
                    for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                        ui.selectable_value(&mut s.video_scaling, mode, mode.label());
                    }
                }).response
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["align_video_top"], "Align video to top", "Moves Fit and Native video to the top of the window.", |ui, s| toggle(ui, &mut s.align_video_top));
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["hdr"], "HDR",
                "Streams 10-bit HDR to an HDR-enabled display. Enable HDR in the operating system first.",
                |ui, s| toggle(ui, &mut s.hdr),
            );
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

            section_at(ui, "Display & image", &mut jump);
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
            let immediate_packed = cfg!(windows) && s.experimental_packed_444 && tearing;
            ui.add_enabled_ui(s.vrr && !immediate_packed, |ui| {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["present_at_host_times"], "Show frames at the PC's frame times",
                    if immediate_packed {
                        "Full chroma presents frames immediately with VRR, without the extra timing delay."
                    } else {
                        "Each frame appears at the moment the game made it, plus a steady delay, so motion stays even."
                    },
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

            section_at(ui, "Controls", &mut jump);
            if !layered {
                row(ui, "Input customization", "Controller mappings, stick dead zones and saved keyboard chords.", |ui| {
                    ui.horizontal(|ui| {
                        let controllers = quiet_button(ui, "Controllers");
                        if controllers.clicked() { commands.push(Command::ControllerSettings); }
                        let shortcuts = quiet_button(ui, "Shortcuts");
                        if shortcuts.clicked() { commands.push(Command::Shortcuts); }
                        links.push(("controllers-link", controllers));
                        links.push(("shortcuts-link", shortcuts.clone()));
                        shortcuts
                    }).inner
                });
            }
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
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["mouse_speed"], "Mouse speed", "Sensitivity of the captured pointer.", |ui, s| ui.add(egui::Slider::new(&mut s.mouse_speed, 25..=400).suffix("%")));
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["mouse_nav_buttons"], "Mouse navigation buttons", "Send the mouse's Back and Forward buttons to the PC.", |ui, s| toggle(ui, &mut s.mouse_nav_buttons));
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["right_alt_as_meta"], "Right Alt as Windows key", "Use Right Alt for Windows-key shortcuts on the PC.", |ui, s| toggle(ui, &mut s.right_alt_as_meta));
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["controllers"], "Controllers", "Every connected controller becomes a controller on the PC.", |ui, s| {
                toggle(ui, &mut s.controllers)
            });
            ui.add_enabled_ui(s.controllers, |ui| {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["back_opens_menu"], "Back opens stream controls", "Use the controller's Back / Select button to open the menu.", |ui, s| toggle(ui, &mut s.back_opens_menu));
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["controller_mouse"], "Controller mouse mode", "Hold Start for 750 ms, then release. A/B click and the D-pad scrolls.", |ui, s| toggle(ui, &mut s.controller_mouse));
                if s.controller_mouse {
                    setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["analog_scrolling"], "Stick for scrolling", "The other stick moves the pointer. Off lets both sticks move it.", |ui, s| {
                        use crate::config::AnalogScrolling;
                        egui::ComboBox::from_id_salt("analog-scrolling").selected_text(match s.analog_scrolling { AnalogScrolling::Off => "Off", AnalogScrolling::Left => "Left", AnalogScrolling::Right => "Right" }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Off, "Off");
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Left, "Left");
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Right, "Right");
                        }).response
                    });
                    setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["mouse_fixed_step"], "Precise controller mouse", "Hold X for fixed pixel steps. Press Y to double the step from 1 to 128 pixels.", |ui, s| toggle(ui, &mut s.mouse_fixed_step));
                }
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["gamepad_touchpad_as_mouse"], "Controller touchpad as mouse", "Move with one finger, scroll with two. Press the pad for a left click, or with two fingers for a right click.", |ui, s| toggle(ui, &mut s.gamepad_touchpad_as_mouse));
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["disable_rumble"], "Disable vibration", "Turn off controller rumble, trigger rumble, and waveform haptics.", |ui, s| toggle(ui, &mut s.disable_rumble));
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["swap_face_buttons"], "Nintendo button layout", "Swap A/B and X/Y.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["controller_motion"], "Motion controls", "Send gyro and accelerometer data when a game asks for it.", |ui, s| {
                    toggle(ui, &mut s.controller_motion)
                });
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["controller_audio"], "DualSense USB audio", "Use waveform haptics, speaker, headset and microphone. Choose devices in stream controls; microphone forwarding also needs Microphone enabled.", |ui, s| {
                    toggle(ui, &mut s.controller_audio)
                });
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["background_controllers"], "Input in the background", "Keep controllers working when Nightjar is not focused.", |ui, s| {
                    toggle(ui, &mut s.background_controllers)
                });
            });
            section_at(ui, "Audio", &mut jump);
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

            section_at(ui, "Streaming", &mut jump);
            if let Some(host) = test_host {
                let online = host.paired() && host.reach == Reach::Online;
                row(ui, "Bandwidth", "Receive 32 MiB from this PC. Uses network data and requires a supported paired host.", |ui| {
                    let enabled = online && host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::PyroWaveBandwidthProbe));
                    let button = ui.add_enabled(enabled, egui::Button::new("Test speed"));
                    if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: false }); }
                    button
                });
                let note = host.record.pace.as_ref().map_or_else(|| "Find a frame pace for this network. May take several minutes and uses substantial network data; stop other streams first.".into(),
                    |p| format!("Saved {} Mbps · {}. Recalibrate after changing the network or stream bitrate.", p.pace_mbps, if p.qualified { "confirmed" } else { "best available" }));
                row(ui, "PyroWave frame pace", &note, |ui| {
                    ui.horizontal(|ui| {
                        let enabled = online && host.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe);
                        let button = ui.add_enabled(enabled, egui::Button::new("Calibrate"));
                        if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: true }); }
                        if host.record.pace.is_some() && text_button(ui, "Clear").clicked() { commands.push(Command::ClearPace(host.id)); }
                        button
                    }).inner
                });
            }
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

            if !layered {
                section_at(ui, "App", &mut jump);
                row(ui, "Open on Home", "Start on the games to continue and your library. Off starts on the list of PCs.", |ui| {
                    toggle(ui, &mut s.console_home)
                });
            }
            section_at(ui, "Advanced", &mut jump);
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["experimental_packed_444"], "Experimental full chroma", "Requires the host option and HEVC or AV1. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave(), |ui| toggle(ui, &mut s.experimental_packed_444)).inner);
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
        for (key, response) in links {
            self.focus_item(&response, key, false);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, view: &View<'_>, commands: &mut Vec<Command>) {
        let frame = Frame::new()
            .fill(SURFACE_HIGH)
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::same(28))
            .stroke(Stroke::new(1.0, OUTLINE_VARIANT));
        let mut close = false;
        let mut next: Option<Dialog> = None;
        let focus_first = self.focus_dialog;
        let mut focused = false;
        match &mut self.dialog {
            Dialog::None => return,
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
                            ui.label(RichText::new(host.display_name()).font(semibold(22.0)));
                            ui.add_space(6.0);
                            for (n, option) in options.iter().enumerate() {
                                if *option == HostOption::Remove {
                                    ui.separator();
                                }
                                let label = RichText::new(option.label()).font(semibold(16.0));
                                let label = if *option == HostOption::Remove { label.color(ERROR) } else { label };
                                let button = ui.add_sized(
                                    [ui.available_width(), 38.0],
                                    egui::Button::new(label).frame(false).fill(Color32::TRANSPARENT),
                                );
                                focus_ring(ui, button.rect, &button, 8);
                                if n == 0 && focus_first {
                                    button.request_focus();
                                    focused = true;
                                }
                                if button.clicked() {
                                    chosen = Some(*option);
                                }
                            }
                            if host.reach == Reach::Offline && host.record.mac.is_none() {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("Pair once while the PC is on to enable Wake.")
                                        .font(regular(13.0))
                                        .color(OUTLINE),
                                );
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
                        ui.label(RichText::new("Rename PC").font(semibold(22.0)));
                        ui.label(
                            RichText::new("The name is kept on this device only. Leave it empty to use the PC's own name.")
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
                        ui.add_space(8.0);
                        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.horizontal(|ui| {
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
                        ui.label(RichText::new(format!("Remove {name}?")).font(semibold(22.0)));
                        let body = if paired {
                            "This deletes the PC, its settings and its saved library from this device. The PC still lists this device as paired until you unpair it."
                        } else {
                            "This deletes the PC, its settings and its saved library from this device."
                        };
                        ui.label(RichText::new(body).color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let cancel = quiet_button(ui, "Cancel");
                            if focus_first {
                                cancel.request_focus();
                                focused = true;
                            }
                            if cancel.clicked() {
                                close = true;
                            }
                            if primary_button(ui, "Remove PC").clicked() {
                                commands.push(Command::Remove(id));
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
                        ui.label(RichText::new(format!("Unpair {name}?")).font(semibold(22.0)));
                        ui.label(
                            RichText::new("The PC drops this device and its saved library is forgotten. It stays in the list; pair again to stream.")
                                .color(ON_SURFACE_VARIANT),
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let cancel = quiet_button(ui, "Cancel");
                            if focus_first {
                                cancel.request_focus();
                                focused = true;
                            }
                            if cancel.clicked() {
                                close = true;
                            }
                            if primary_button(ui, "Unpair").clicked() {
                                commands.push(Command::Unpair(id));
                                close = true;
                            }
                        });
                    });
            }
            Dialog::PairError { host, body, one_time } => {
                let (id, one_time) = (*host, *one_time);
                let name = view.hosts.get(id).map_or("the PC", |h| h.display_name()).to_owned();
                egui::Modal::new(Id::unique("pair-error"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(format!("Pairing with {name} failed")).font(semibold(22.0)));
                        ui.label(RichText::new(body.as_str()).color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
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
                    ui.label(RichText::new("Network test").font(semibold(22.0)));
                    ui.horizontal(|ui| { spinner(ui, 10.0); ui.label(stage.as_str()); });
                    let button = quiet_button(ui, "Cancel");
                    if focus_first { button.request_focus(); focused = true; }
                    if button.clicked() { commands.push(Command::CancelNetworkTest); close = true; }
                });
            }
            Dialog::Shortcut { id, name, chord, error } => {
                egui::Modal::new(Id::unique("shortcut-editor"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(if id.is_some() { "Edit shortcut" } else { "New shortcut" }).font(semibold(22.0)));
                        let edit = ui.add(egui::TextEdit::singleline(name).hint_text("Shortcut name"));
                        if focus_first { edit.request_focus(); focused = true; }
                        ui.add(egui::TextEdit::singleline(chord).hint_text("CTRL + ALT + DELETE"));
                        ui.label("Use letters, digits, F1–F24, CTRL, SHIFT, ALT, WIN, TAB, ENTER, ESC, arrows or other key names.");
                        if let Some(error) = error { ui.label(RichText::new(error.as_str()).color(ERROR)); }
                        ui.horizontal(|ui| {
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
                        ui.label(RichText::new(format!("Delete {name}?")).font(semibold(22.0)));
                        ui.horizontal(|ui| {
                            let button = quiet_button(ui, "Cancel");
                            if focus_first { button.request_focus(); focused = true; }
                            if button.clicked() { close = true; }
                            if primary_button(ui, "Delete").clicked() {
                                self.shortcuts.data.retain(|entry| entry.id != *id);
                                commands.push(Command::SaveShortcuts);
                                close = true;
                            }
                        });
                    });
            }
            Dialog::ImportShortcuts(entries) => {
                egui::Modal::new(Id::unique("import-shortcuts"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new("Import keyboard shortcuts?").font(semibold(22.0)));
                        ui.label(format!("Replace your {} saved shortcuts with {} from this file.", self.shortcuts.data.len(), entries.data.len()));
                        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                            for entry in &entries.data { ui.label(format!("{} · {}", entry.name, entry.keys.join(" + "))); }
                        });
                        ui.horizontal(|ui| {
                            if quiet_button(ui, "Cancel").clicked() { close = true; }
                            if primary_button(ui, "Import").clicked() {
                                self.shortcuts = entries.clone();
                                commands.push(Command::SaveShortcuts);
                                close = true;
                            }
                        });
                    });
            }
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
            Dialog::ConfirmClose { host, name } => {
                let (host, name) = (*host, name.clone());
                egui::Modal::new(Id::unique("confirm-close"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new("Close the game on the PC?").font(semibold(22.0)));
                        ui.label(RichText::new(format!("{name} closes on the PC. Unsaved progress is lost.")).color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
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
    fn close_dialog(&mut self) {
        let from_card = matches!(
            self.dialog,
            Dialog::HostOptions { .. }
                | Dialog::Rename { .. }
                | Dialog::ConfirmRemove { .. }
                | Dialog::ConfirmUnpair { .. }
                | Dialog::PairError { .. }
        );
        self.dialog = Dialog::None;
        match self.focused_key.clone().filter(|_| from_card) {
            Some(key) => self.restore_key = Some(key),
            None => self.focus_screen = true,
        }
    }

    /// Acts on an entry of a PC's options; a dialog to ask first is returned.
    fn choose_host_option(&mut self, host: &Host, option: HostOption, commands: &mut Vec<Command>) -> Option<Dialog> {
        let id = host.id;
        let name = host.display_name().to_owned();
        match option {
            HostOption::OpenLibrary => commands.push(Command::OpenApps(id)),
            HostOption::QuitApp => {
                return Some(Dialog::ConfirmQuit { host: id, start: None, running: running_name(host) });
            }
            HostOption::Pair => commands.push(Command::Pair(id)),
            HostOption::PairOtp => {
                return Some(Dialog::Otp { host: id, pin: String::new(), passphrase: String::new(), error: None });
            }
            HostOption::Wake => commands.push(Command::Wake(id)),
            HostOption::Settings => commands.push(Command::HostSettings(id)),
            HostOption::Rename => return Some(Dialog::Rename { host: id, text: host.rename_prefill().to_owned() }),
            HostOption::Unpair => return Some(Dialog::ConfirmUnpair { host: id, name }),
            HostOption::CheckAgain => commands.push(Command::Refresh(id)),
            HostOption::Remove => return Some(Dialog::ConfirmRemove { host: id, name }),
        }
        None
    }

    /// Shows why pairing failed, with a way to try again.
    pub fn pair_failed(&mut self, host: u64, body: impl Into<String>, one_time: bool) {
        self.dialog = Dialog::PairError { host, body: body.into(), one_time };
        self.focus_dialog = true;
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

/// What selecting a PC's card does: open its apps, check again, or pair.
/// The three dots on a PC's card: a second stop for focus that opens the options.
fn options_button(ui: &mut Ui, card: Rect, id: Id) -> Response {
    let rect = Rect::from_min_size(card.right_top() + vec2(-42.0, 10.0), vec2(32.0, 32.0));
    let response = ui.interact(rect, id, Sense::click()).on_hover_text("Options");
    if ui.is_rect_visible(rect) {
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, CornerRadius::same(16), SURFACE_HIGHEST);
        }
        for dy in [-6.0, 0.0, 6.0] {
            ui.painter().circle_filled(rect.center() + vec2(0.0, dy), 2.0, ON_SURFACE_VARIANT);
        }
        focus_ring(ui, rect, &response, 16);
    }
    response
}

/// Right-click, F2 or Shift+F10 on the card or its dots.
fn options_requested(ui: &Ui, responses: &[&Response]) -> bool {
    responses.iter().any(|r| r.secondary_clicked())
        || (responses.iter().any(|r| r.has_focus())
            && ui.input(|i| i.key_pressed(Key::F2) || (i.modifiers.shift && i.key_pressed(Key::F10))))
}

fn host_clicked(host: &Host, commands: &mut Vec<Command>) {
    if host.paired() {
        commands.push(Command::OpenApps(host.id));
    } else if host.reach == Reach::Offline {
        commands.push(Command::Refresh(host.id));
    } else {
        commands.push(Command::Pair(host.id));
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
    section_at(ui, title, &mut None);
}

/// A section heading; scrolls itself to the top when `jump` names it (a section chip was used).
fn section_at(ui: &mut Ui, title: &str, jump: &mut Option<&'static str>) {
    ui.add_space(14.0);
    if jump.is_some_and(|j| j == title) {
        ui.scroll_to_cursor(Some(Align::TOP));
        *jump = None;
    }
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

/// A PC's card. `line` replaces the address and GPU line (Home shows what it knows of the games).
fn host_card(ui: &mut Ui, host: &Host, line: Option<&str>) -> Response {
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
    let detail = match (line, host.info.as_ref().and_then(|i| i.gpu_type.clone())) {
        (Some(line), _) => line.to_owned(),
        (None, Some(gpu)) if !gpu.is_empty() => format!("{}  ·  {gpu}", host.record.address),
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

/// The cover art, or the night surface with the banner's orbits when the PC has none.
fn paint_cover(ui: &Ui, image: Rect, name: &str, art: Option<&Art>, running: bool) {
    let painter = ui.painter_at(image.expand(6.0));
    match art {
        Some(Art::Ready(texture)) => {
            egui::Image::from_texture((texture.id(), image.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, image);
        }
        _ => {
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
}

fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H + 34.0), Sense::click());
    let image = Rect::from_min_size(rect.min, vec2(POSTER_W, POSTER_H));
    paint_cover(ui, image, name, art, running);
    let painter = ui.painter_at(rect.expand(6.0));
    let label_color = if response.has_focus() { MOON } else { ON_SURFACE };
    let galley = painter.layout(name.to_owned(), semibold(14.5), label_color, POSTER_W);
    painter.galley(Pos2::new(rect.left() + 2.0, image.bottom() + 8.0), galley, label_color);
    focus_ring(ui, image, &response, 10);
    response
}

/// A game to carry on with: title, and where it runs.
fn continue_card(ui: &mut Ui, title: &str, line: &str, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, 84.0), Sense::click());
    let painter = ui.painter_at(rect.expand(6.0));
    let fill = if response.hovered() || response.has_focus() { SURFACE_HIGH } else { SURFACE };
    painter.rect_filled(rect, CornerRadius::same(12), fill);
    let color = if running { MOON } else { TERTIARY };
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + 28.0);
    if running {
        painter.circle_filled(dot, 7.0, color);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, color));
    }
    let text = ui.painter_at(rect);
    let galley = text.layout_no_wrap(title.to_owned(), semibold(18.0), ON_SURFACE);
    text.galley(Pos2::new(rect.left() + 44.0, rect.top() + 28.0 - galley.size().y / 2.0), galley, ON_SURFACE);
    text.text(Pos2::new(rect.left() + 20.0, rect.top() + 60.0), Align2::LEFT_CENTER, line, regular(14.0), color);
    focus_ring(ui, rect, &response, 12);
    response
}

/// Asks for a cover when it is on screen and the PC can send it.
fn request_cover(
    ui: &Ui,
    view: &View<'_>,
    host: u64,
    app: u32,
    version: &str,
    response: &Response,
    commands: &mut Vec<Command>,
) {
    let wanted = view
        .hosts
        .get(host)
        .is_some_and(|h| h.reach == Reach::Online && h.art_wanted(app as i32, version));
    if wanted && ui.is_rect_visible(response.rect) {
        commands.push(Command::RequestArt { host, app: app as i32, version: version.to_owned() });
    }
}

trait UiExt {
    fn ui(self, ui: &mut Ui) -> Response;
}

impl UiExt for egui::Image<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.add(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher(console_home: bool, home_ready: bool) -> Launcher {
        let settings = Settings { console_home, ..Default::default() };
        let display = Display { width: 1920, height: 1080, refresh_hz: 60.0 };
        let mut launcher = Launcher::new(settings, Profiles::default(), crate::shortcuts::Shortcuts::default(), display);
        launcher.home_ready = home_ready;
        launcher
    }

    #[test]
    fn back_returns_to_the_screen_and_the_item_it_left() {
        let mut l = launcher(true, true);
        l.go(Screen::Home);
        l.focused_key = Some("game:steam:570".into());
        l.open(Screen::Game { host: 1, app: 5 });
        assert_eq!(l.screen, Screen::Game { host: 1, app: 5 });
        l.focused_key = Some("play".into());
        let mut commands = Vec::new();
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Home);
        assert_eq!(l.restore_key.as_deref(), Some("game:steam:570"));
        assert!(l.focus_screen, "the first item takes focus if the remembered one is gone");
        // Home is the end of the line.
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Home);
    }

    #[test]
    fn back_ends_on_home_when_it_is_on_and_on_the_pcs_when_it_is_not() {
        let mut on = launcher(true, true);
        on.go(Screen::Hosts);
        on.open(Screen::Apps(1));
        let mut commands = Vec::new();
        on.back(&mut commands);
        assert_eq!(on.screen, Screen::Hosts);
        on.back(&mut commands);
        assert_eq!(on.screen, Screen::Home, "the PCs list goes back to Home");

        let mut off = launcher(false, true);
        off.go(Screen::Hosts);
        off.open(Screen::Apps(1));
        off.back(&mut commands);
        off.back(&mut commands);
        assert_eq!(off.screen, Screen::Hosts, "Open on Home is off: the PCs are the root");

        let mut empty = launcher(true, false);
        empty.go(Screen::Hosts);
        empty.back(&mut commands);
        assert_eq!(empty.screen, Screen::Hosts, "no library, no Home to go back to");
    }

    #[test]
    fn settings_open_once_and_back_leaves_them_where_they_were_opened() {
        let mut l = launcher(true, true);
        l.go(Screen::Home);
        l.open_settings();
        l.open_settings();
        l.go(Screen::Profiles); // a settings subpage, reached the usual way
        l.open_settings();
        assert_eq!(l.screen, Screen::Settings);
        assert_eq!(l.trail.len(), 1, "Start inside Settings does not stack screens");
        let mut commands = Vec::new();
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Home);
        assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "leaving Settings saves them");
    }

    #[test]
    fn a_game_page_for_another_copy_replaces_the_page() {
        let mut l = launcher(true, true);
        l.go(Screen::Home);
        l.open(Screen::Game { host: 1, app: 5 });
        l.go(Screen::Game { host: 2, app: 9 });
        let mut commands = Vec::new();
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Home, "one Back, not one per copy");
    }

    // ---- Headless frames: the PCs list driven by keys and the pointer. ----

    use crate::config::HostRecord;
    use crate::gamepad::Gamepads;
    use std::sync::Arc;

    fn record(name: &str, paired: bool, mac: bool) -> HostRecord {
        HostRecord {
            unique_id: name.into(),
            name: name.into(),
            address: "192.0.2.7".into(),
            server_certificate: paired.then(|| "pem".into()),
            mac: mac.then(|| "00:1a:2b:3c:4d:5e".into()),
            manual: true,
            ..Default::default()
        }
    }

    /// A launcher on the PCs screen with a headless egui context.
    struct Frames {
        ctx: egui::Context,
        l: Launcher,
        hosts: Hosts,
        pads: Gamepads,
        time: f64,
    }

    impl Frames {
        fn new(record: HostRecord, reach: Reach) -> Self {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut l = launcher(false, false);
            l.go(Screen::Hosts);
            let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
            hosts.list[0].reach = reach;
            let mut frames = Self { ctx, l, hosts, pads: Gamepads::default(), time: 0.0 };
            frames.settle();
            frames
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Command> {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))),
                time: Some(self.time),
                events,
                focused: true,
                ..Default::default()
            };
            let mut commands = Vec::new();
            let mut output = self.ctx.run_ui(input, |ui| {
                commands = self.l.ui(ui, View { hosts: &self.hosts, pads: &self.pads, connecting: None });
            });
            output.textures_delta.clear();
            commands
        }

        fn settle(&mut self) -> Vec<Command> {
            let mut commands = Vec::new();
            for _ in 0..4 {
                commands.extend(self.frame(Vec::new()));
            }
            commands
        }

        fn press(&mut self, key: Key) -> Vec<Command> {
            let mut commands = self.frame(crate::gui::tap(key).to_vec());
            commands.extend(self.settle());
            commands
        }

        fn press_with(&mut self, key: Key, modifiers: egui::Modifiers) -> Vec<Command> {
            let mut events = vec![egui::Event::ModifiersChanged(modifiers)];
            events.extend(crate::gui::tap(key));
            let mut commands = self.frame(events);
            commands.extend(self.frame(vec![egui::Event::ModifiersChanged(Default::default())]));
            commands.extend(self.settle());
            commands
        }

        fn click_at(&mut self, pos: Pos2, button: egui::PointerButton) -> Vec<Command> {
            let event = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE };
            let mut commands = self.frame(vec![egui::Event::PointerMoved(pos)]);
            commands.extend(self.frame(vec![event(true)]));
            commands.extend(self.frame(vec![event(false)]));
            commands.extend(self.settle());
            commands
        }

        /// The card (first stop) and its three dots, as laid out on screen.
        fn card_and_dots(&self) -> (Rect, Rect) {
            let focused = self.ctx.memory(|m| m.focused()).expect("the first PC has focus");
            let card = self.ctx.read_response(focused).expect("the card was laid out").rect;
            let dots = self.ctx.read_response(focused.with("options")).expect("the dots were laid out").rect;
            (card, dots)
        }

        fn options_open(&self) -> bool {
            matches!(self.l.dialog, Dialog::HostOptions { .. })
        }

        fn no_dialog(&self) -> bool {
            matches!(self.l.dialog, Dialog::None)
        }
    }

    fn every_state() -> Vec<(&'static str, HostRecord, Reach)> {
        vec![
            ("unpaired", record("Unpaired", false, false), Reach::Online),
            ("offline, can wake", record("Offline", true, true), Reach::Offline),
            ("paired", record("Paired", true, false), Reach::Online),
        ]
    }

    fn removes(commands: &[Command]) -> bool {
        commands.iter().any(|c| matches!(c, Command::Remove(_)))
    }

    #[test]
    fn the_destinations_read_home_pcs_settings() {
        let names: Vec<&str> = DESTINATIONS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["Home", "PCs", "Settings"]);
        let screens: Vec<Screen> = DESTINATIONS.iter().map(|(_, screen)| *screen).collect();
        assert_eq!(screens, [Screen::Home, Screen::Hosts, Screen::Settings]);
    }

    #[test]
    fn destinations_replace_each_other_and_back_ends_at_the_root() {
        let mut l = launcher(true, true);
        l.go(Screen::Home);
        for screen in [Screen::Hosts, Screen::Settings, Screen::Hosts, Screen::Settings] {
            l.switch(screen);
            assert_eq!(l.screen, screen);
            assert_eq!(l.trail.len(), 1, "one step back to Home, however many destinations were visited");
        }
        l.switch(Screen::Home);
        assert_eq!((l.screen, l.trail.len()), (Screen::Home, 0), "Home is the root again");
        l.switch(Screen::Hosts);
        let mut commands = Vec::new();
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Home);
    }

    #[test]
    fn profiles_and_shortcuts_pop_back_to_settings() {
        let mut l = launcher(true, true);
        l.go(Screen::Home);
        l.open_settings();
        l.focused_key = Some("profiles-link".into());
        l.open(Screen::Profiles);
        let mut commands = Vec::new();
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Settings);
        assert_eq!(l.restore_key.as_deref(), Some("profiles-link"), "the row that opened it takes focus");
        assert_eq!(l.trail.len(), 1, "Settings still goes back to Home in one step");
        l.open(Screen::Shortcuts);
        l.back(&mut commands);
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Home);
    }

    #[test]
    fn settings_sections_follow_androids_order() {
        let source = include_str!("ui.rs");
        let body = &source[source.find("fn settings_page").unwrap()..];
        let found: Vec<&str> = body
            .match_indices("section_at(ui, \"")
            .map(|(at, text)| {
                let rest = &body[at + text.len()..];
                &rest[..rest.find('"').unwrap()]
            })
            .take(SETTINGS_SECTIONS.len())
            .collect();
        assert_eq!(found, SETTINGS_SECTIONS, "the page and its chips list the same sections in the same order");
    }

    #[test]
    fn the_options_list_follows_the_pc_state() {
        let mut host = Frames::new(record("PC", false, false), Reach::Online).hosts.list.remove(0);
        assert_eq!(
            host_options(&host),
            [HostOption::Pair, HostOption::Settings, HostOption::Rename, HostOption::CheckAgain, HostOption::Remove]
        );
        host.record.server_certificate = Some("pem".into());
        host.reach = Reach::Offline;
        host.record.mac = Some("00:1a:2b:3c:4d:5e".into());
        assert_eq!(
            host_options(&host),
            [
                HostOption::OpenLibrary,
                HostOption::Wake,
                HostOption::Settings,
                HostOption::Rename,
                HostOption::CheckAgain,
                HostOption::Remove
            ]
        );
        host.record.mac = None;
        assert!(!host_options(&host).contains(&HostOption::Wake), "no MAC, no Wake");
        assert!(!host_options(&host).contains(&HostOption::Unpair), "an offline PC cannot be asked to unpair");
        host.reach = Reach::Online;
        assert!(host_options(&host).contains(&HostOption::Unpair));
    }

    #[test]
    fn f2_opens_the_options_for_the_focused_pc() {
        for (state, record, reach) in every_state() {
            let mut f = Frames::new(record, reach);
            assert!(!f.options_open(), "{state}");
            f.press(Key::F2);
            assert!(f.options_open(), "{state}: F2, which the controller's West button sends, opens the options");
            f.press(Key::Escape);
            assert!(f.no_dialog(), "{state}: Escape closes them");
            f.press_with(Key::F10, egui::Modifiers::SHIFT);
            assert!(f.options_open(), "{state}: Shift+F10");
        }
    }

    #[test]
    fn right_click_and_the_three_dots_open_the_options() {
        let mut f = Frames::new(record("Paired", true, false), Reach::Online);
        let (card, dots) = f.card_and_dots();
        let body = card.left_bottom() + vec2(20.0, -10.0);
        f.click_at(body, egui::PointerButton::Secondary);
        assert!(f.options_open(), "right-click");
        f.press(Key::Escape);
        f.click_at(dots.center(), egui::PointerButton::Primary);
        assert!(f.options_open(), "the visible dots");
        f.press(Key::Escape);
        let commands = f.click_at(body, egui::PointerButton::Primary);
        assert!(f.no_dialog(), "a plain click still takes the card's own action");
        assert!(commands.iter().any(|c| matches!(c, Command::OpenApps(_))));
    }

    #[test]
    fn every_option_is_reachable_with_the_keyboard() {
        for (state, record, reach) in every_state() {
            let options = host_options(&Frames::new(record.clone(), reach).hosts.list[0]);
            for (n, option) in options.iter().enumerate() {
                let mut f = Frames::new(record.clone(), reach);
                f.press(Key::F2);
                assert!(f.options_open(), "{state}");
                for _ in 0..n {
                    f.press(Key::ArrowDown);
                }
                let commands = f.press(Key::Enter);
                let said = |what: fn(&Command) -> bool| commands.iter().any(what);
                let ok = match option {
                    HostOption::OpenLibrary => said(|c| matches!(c, Command::OpenApps(_))),
                    HostOption::Pair => said(|c| matches!(c, Command::Pair(_))),
                    HostOption::Wake => said(|c| matches!(c, Command::Wake(_))),
                    HostOption::Settings => said(|c| matches!(c, Command::HostSettings(_))),
                    HostOption::CheckAgain => said(|c| matches!(c, Command::Refresh(_))),
                    HostOption::Rename => matches!(f.l.dialog, Dialog::Rename { .. }),
                    HostOption::Unpair => matches!(f.l.dialog, Dialog::ConfirmUnpair { .. }),
                    HostOption::Remove => matches!(f.l.dialog, Dialog::ConfirmRemove { .. }),
                    HostOption::PairOtp => matches!(f.l.dialog, Dialog::Otp { .. }),
                    HostOption::QuitApp => matches!(f.l.dialog, Dialog::ConfirmQuit { .. }),
                };
                assert!(ok, "{state}: {} by keys (entry {n})", option.label());
            }
        }
    }

    #[test]
    fn removing_a_pc_asks_first() {
        let mut f = Frames::new(record("Paired", true, false), Reach::Online);
        let last = host_options(&f.hosts.list[0]).len() - 1;
        let open_remove = |f: &mut Frames, round: &str| {
            f.press(Key::F2);
            for _ in 0..last {
                f.press(Key::ArrowDown);
            }
            let commands = f.press(Key::Enter);
            assert!(matches!(f.l.dialog, Dialog::ConfirmRemove { .. }), "{round}: the confirmation opens");
            assert!(!removes(&commands), "{round}: choosing Remove does not remove");
        };
        // Cancel has focus: Enter and Escape both leave the PC alone.
        open_remove(&mut f, "enter");
        let commands = f.press(Key::Enter);
        assert!(f.no_dialog());
        assert!(!removes(&commands));
        assert!(f.l.focused_key.is_some(), "Cancel keeps the card's focus");
        assert!(f.ctx.memory(|m| m.focused()).is_some(), "and the card has it again");
        open_remove(&mut f, "escape");
        let commands = f.press(Key::Escape);
        assert!(f.no_dialog());
        assert!(!removes(&commands));
        assert!(f.l.focused_key.is_some());
        // Only the Remove button does it.
        open_remove(&mut f, "confirm");
        f.press(Key::ArrowRight);
        let commands = f.frame(crate::gui::tap(Key::Enter).to_vec());
        assert!(removes(&commands));
        assert!(f.l.focused_key.is_none(), "the card goes with the PC");
    }

    #[test]
    fn rename_keeps_the_name_on_this_device() {
        let mut f = Frames::new(record("Gaming PC", true, false), Reach::Online);
        f.press(Key::F2);
        f.press(Key::ArrowDown);
        f.press(Key::ArrowDown);
        f.press(Key::Enter);
        let Dialog::Rename { text, .. } = &mut f.l.dialog else { panic!("the rename dialog") };
        assert_eq!(text, "Gaming PC", "prefilled with the name shown");
        *text = "  Living room  ".into();
        let commands = f.press(Key::Enter);
        let alias = commands.iter().find_map(|c| match c {
            Command::Rename { alias, .. } => Some(alias.clone()),
            _ => None,
        });
        assert_eq!(alias.as_deref(), Some("  Living room  "));
        let id = f.hosts.list[0].id;
        assert!(f.hosts.rename(id, "  Living room  "));
        assert_eq!(f.hosts.list[0].display_name(), "Living room");
        assert!(f.hosts.rename(id, ""));
        assert_eq!(f.hosts.list[0].display_name(), "Gaming PC", "empty brings the PC's own name back");
        assert!(f.hosts.rename(id, "Gaming PC"));
        assert_eq!(f.hosts.list[0].record.alias, None, "the PC's own name is not stored as an alias");
    }

    #[test]
    fn a_pc_that_never_reported_a_name_starts_the_rename_empty() {
        let mut unnamed = record("x", true, false);
        unnamed.name = String::new();
        let mut f = Frames::new(unnamed, Reach::Online);
        let id = f.hosts.list[0].id;
        assert_eq!(f.hosts.list[0].display_name(), "192.0.2.7", "the card falls back to the address");
        f.press(Key::F2);
        f.press(Key::ArrowDown);
        f.press(Key::ArrowDown);
        f.press(Key::Enter);
        let Dialog::Rename { text, .. } = &f.l.dialog else { panic!("the rename dialog") };
        assert_eq!(text, "", "the address is not offered as a name");
        // Saving the address back, or nothing, leaves no alias.
        assert!(f.hosts.rename(id, "192.0.2.7"));
        assert_eq!(f.hosts.list[0].record.alias, None);
        assert!(f.hosts.rename(id, ""));
        assert_eq!(f.hosts.list[0].record.alias, None);
    }

    /// The tab stops in the header band, in the order Tab reaches them.
    fn header_stops(f: &mut Frames) -> Vec<Rect> {
        let mut stops: Vec<Rect> = Vec::new();
        for _ in 0..14 {
            f.press(Key::Tab);
            let Some(id) = f.ctx.memory(|m| m.focused()) else { continue };
            let Some(rect) = f.ctx.read_response(id).map(|r| r.rect) else { continue };
            if rect.center().y < 80.0 && !stops.contains(&rect) {
                stops.push(rect);
            }
        }
        stops
    }

    #[test]
    fn tab_walks_the_header_left_to_right_and_the_actions_sit_at_the_right_edge() {
        // PCs: Home hides while there is no library, Add PC follows Settings.
        let mut f = Frames::new(record("Paired", true, false), Reach::Online);
        let stops = header_stops(&mut f);
        assert_eq!(stops.len(), 3, "PCs, Settings and Add PC: {stops:?}");
        assert!(stops.windows(2).all(|w| w[0].right() < w[1].left()), "in reading order: {stops:?}");
        assert!(1280.0 - stops[2].right() < 80.0, "Add PC is at the right edge: {:?}", stops[2]);

        // Home shows all three destinations in order.
        let mut f = Frames::new(record("Paired", true, false), Reach::Online);
        f.l.go(Screen::Home);
        f.settle();
        let stops = header_stops(&mut f);
        assert_eq!(stops.len(), 3, "Home, PCs and Settings: {stops:?}");
        assert!(stops.windows(2).all(|w| w[0].right() < w[1].left()), "in reading order: {stops:?}");
        assert!(1280.0 - stops[2].right() < 80.0, "Settings is at the right edge");
    }

    #[test]
    fn a_failed_pairing_offers_to_try_again() {
        let mut f = Frames::new(record("PC", false, false), Reach::Online);
        let id = f.hosts.list[0].id;
        f.l.pair_failed(id, crate::hosts::PAIR_PIN_WRONG, false);
        f.settle();
        let commands = f.press(Key::Enter);
        assert!(commands.iter().any(|c| matches!(c, Command::Pair(host) if *host == id)), "Try again pairs again");
        f.l.pair_failed(id, crate::hosts::PAIR_OTP_PIN_WRONG, true);
        f.settle();
        f.press(Key::Enter);
        assert!(matches!(f.l.dialog, Dialog::Otp { .. }), "a one-time PIN failure goes back to the PIN form");
    }
}
