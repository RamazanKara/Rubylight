//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2,
};

use crate::config::{
    step_zoom, AudioChannels, CodecPreference, DecoderPreference, HostDisplay, Language, MouseMode, Overrides, Profiles,
    ReduceMotion, Settings, WindowMode, UI_ZOOM_STEPS,
};
use crate::console::{self, EndAction, GameState, Playing, Source};
use crate::hosts::{Apps, Art, Handed, Host, Hosts, Reach, SleepStep};
use crate::strings::{en, tr, trf};
use crate::stream::{stream_mode, Display};
use crate::theme::*;

pub enum Command {
    Launch { host: u64, app: i32, resume: bool },
    QuitApp(u64),
    ControlAction { host: u64, app: i32 },
    /// `named` is the app the prompt said would be closed; it is checked again before closing.
    QuitAndLaunch { host: u64, app: i32, named: Option<i32> },
    Pair(u64),
    PairOtp { host: u64, pin: String, passphrase: String },
    Wake(u64),
    /// Stop waking the PC ("Waking <PC>…").
    CancelWake,
    /// Ask the PC for a sleep token, which the confirmation then spends. `ask` numbers the
    /// request, so a late answer to an earlier one is not taken for it.
    SleepOffer { host: u64, ask: u64 },
    /// Put the PC to sleep with the token it handed out at `at`.
    Sleep { host: u64, ask: u64, token: String, at: Handed },
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
    /// Show the folder with Nightjar's log files in the file manager.
    OpenLogs,
    /// Check that the PC answers at its saved address (the details page's remote access test).
    RemoteCheck(u64),
    /// Open a PC's web UI in the default browser.
    OpenWebUi(String),
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
    /// Star the game, or take its star off.
    ToggleFavourite(crate::config::Favourite),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Games to continue, the merged library and the PCs.
    Home,
    /// One game on one PC (`app` is the native library's app id).
    Game { host: u64, app: u32 },
    Hosts,
    Apps(u64),
    /// A PC's host software, web UI and remote access check.
    HostDetails(u64),
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
    /// Quit the running app, then start `start`. `resume` is the running app's id when the
    /// dialog can offer to carry on with it instead.
    ConfirmQuit { host: u64, start: Option<(i32, String)>, running: String, resume: Option<i32>, named: Option<i32> },
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
    /// Sleep PC: waiting for the PC's token (`offer` None), then asking to confirm. `ask` is
    /// the number of the token request this dialog waits for.
    ConfirmSleep { host: u64, ask: u64, name: String, offer: Option<(String, Handed)> },
    /// Pairing did not work: what to do about it, and a way to try again.
    PairError { host: u64, body: String, one_time: bool },
    /// How to pair, for a PC that is not paired yet.
    PairingHelp,
}

/// The header's top-level destinations, left to right.
pub const DESTINATIONS: [(&str, Screen); 3] = [(en("Home"), Screen::Home), (en("PCs"), Screen::Hosts), (en("Settings"), Screen::Settings)];

/// The settings sections in the order Android lists them (App only on the main settings).
const SETTINGS_SECTIONS: [&str; 7] = [en("Video"), en("Display & image"), en("Controls"), en("Audio"), en("Streaming"), en("App"), en("Advanced")];

/// What the pairing help says, in the order of Nightjar Android's guide.
const PAIRING_STEPS: [&str; 3] = [
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
    /// Starred games, newest star first.
    pub favourites: Vec<crate::config::Favourite>,
    logo: Option<TextureHandle>,
    focus_screen: bool,
    focus_dialog: bool,
    /// The last Sleep PC token request's number.
    sleep_asks: u64,
    /// Messages that came while the user was busy with another dialog, shown once it closes.
    queued: std::collections::VecDeque<(String, String)>,
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
    /// The renderer's Direct3D 11 device, which the PyroWave GPU decoder's settings probe runs on.
    pub gpu_device: Option<crate::stream::pipeline::GpuDevice>,
    pub display: Display,
    pub identity_ready: bool,
    /// The system's preferred languages, for the Language setting's default.
    pub system_languages: Vec<String>,
    /// The palette and motion egui's style was last built for.
    look: Option<(bool, bool)>,
    /// The details page's remote access check.
    pub remote: crate::host_page::RemoteAccess,
}

/// What the launcher shows, borrowed from the app each frame.
pub struct View<'a> {
    pub hosts: &'a Hosts,
    pub pads: &'a crate::gamepad::Gamepads,
    pub connecting: Option<ConnectingView>,
    /// The name of the PC being woken.
    pub waking: Option<String>,
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
            Stroke::new(focus_ring_width(), MOON),
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
    let track: Color32 = if *on { *MOON } else { *SURFACE_HIGHEST };
    let track = if enabled { track } else { track.gamma_multiply(0.4) };
    ui.painter().rect_filled(rect, CornerRadius::same(12), track);
    let knob_x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
    let knob = if *on { *ON_MOON } else { *OUTLINE };
    ui.painter().circle_filled(Pos2::new(knob_x, rect.center().y), 8.0, knob);
    focus_ring(ui, rect, &response, 12);
    response
}

fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(tr(text)).color(ON_MOON).font(semibold(15.0)))
        .fill(*MOON)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn quiet_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(tr(text)).color(ON_SURFACE).font(semibold(15.0)))
        .fill(*SURFACE_HIGHEST)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

fn text_button(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(tr(text)).color(ON_SURFACE_VARIANT).font(semibold(15.0)))
        .frame(false)
        .min_size(vec2(0.0, 36.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 8);
    response
}

/// A small pill that jumps to a section of the settings.
fn chip(ui: &mut Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(tr(text)).color(ON_SURFACE).font(semibold(13.5)))
        .fill(*SURFACE)
        .corner_radius(CornerRadius::same(14))
        .min_size(vec2(0.0, 28.0));
    let response = ui.add(button);
    focus_ring(ui, response.rect, &response, 14);
    response
}

/// A spinning arc. With reduced motion (egui's animation time is 0) the arc stands still and
/// nothing asks for the next frame.
fn spinner(ui: &mut Ui, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(radius * 2.0), Sense::hover());
    let still = ui.style().animation_time <= 0.0;
    let t = if still { 0.0 } else { ui.input(|i| i.time) as f32 };
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
    if !still {
        ui.ctx().request_repaint();
    }
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
            favourites: Vec::new(),
            logo: None,
            focus_screen: true,
            focus_dialog: false,
            sleep_asks: 0,
            queued: Default::default(),
            settings_return: Screen::Hosts,
            trail: Vec::new(),
            focused_key: None,
            restore_key: None,
            home_ready: false,
            header_width: 0.0,
            tearing_supported: false,
            gpu_device: None,
            display,
            identity_ready: false,
            system_languages: Vec::new(),
            look: None,
            remote: Default::default(),
        }
    }

    /// Brings egui's style, the zoom and the language in line with the settings. Cheap when
    /// nothing changed: the style is rebuilt only for a new palette or motion setting.
    fn sync_look(&mut self, ctx: &egui::Context) {
        let s = &self.settings;
        let still = match s.reduce_motion {
            ReduceMotion::Reduce => true,
            ReduceMotion::Full => false,
            ReduceMotion::System => crate::theme::system_reduces_motion(),
        };
        if self.look != Some((s.high_contrast, still)) {
            set_look(ctx, s.high_contrast, still);
            self.look = Some((s.high_contrast, still));
        }
        let zoom = crate::config::nearest_zoom(s.ui_zoom) as f32 / 100.0;
        if (ctx.zoom_factor() - zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(zoom);
        }
        crate::strings::set(crate::strings::resolve(s.language, &self.system_languages));
    }

    /// Ctrl+= and Ctrl+- step the zoom, Ctrl+0 resets it. Returns whether it changed.
    fn zoom_keys(&mut self, ctx: &egui::Context) -> bool {
        // In a text field the keys belong to the field.
        if ctx.text_edit_focused() {
            return false;
        }
        let step = ctx.input(|i| {
            if !i.modifiers.command {
                None
            } else if i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus) {
                Some(1)
            } else if i.key_pressed(Key::Minus) {
                Some(-1)
            } else if i.key_pressed(Key::Num0) {
                Some(0)
            } else {
                None
            }
        });
        let Some(step) = step else { return false };
        let zoom = if step == 0 { 100 } else { step_zoom(self.settings.ui_zoom, step) };
        let changed = zoom != self.settings.ui_zoom;
        self.settings.ui_zoom = zoom;
        changed
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
            // A library or game page is not a destination: Settings opens over it, and Back
            // returns to the page with its item focused.
            Screen::Apps(_) | Screen::Game { .. } | Screen::HostDetails(_) => self.open(Screen::Settings),
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
                Screen::Apps(_) | Screen::Game { .. } | Screen::HostDetails(_) => {
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
        if self.zoom_keys(&ctx) {
            commands.push(Command::SaveSettings);
        }
        self.sync_look(&ctx);
        // A check whose page was left is cancelled here. Settings opened over the page does not
        // leave it: Back returns to the page and its check.
        let page = std::iter::once(&self.screen).chain(self.trail.iter().map(|(screen, _)| screen)).find_map(|screen| match screen {
            Screen::HostDetails(id) => Some(*id),
            _ => None,
        });
        self.remote.poll(page);
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        self.home_ready = console::has_library(&sources);
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if escape && view.connecting.is_none() && view.waking.is_none() {
            self.back(&mut commands);
        }
        if self.screen == Screen::Hosts
            && matches!(self.dialog, Dialog::None)
            && view.connecting.is_none()
            && view.waking.is_none()
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
        let blocked = !matches!(self.dialog, Dialog::None) || view.connecting.is_some() || view.waking.is_some();
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
                Screen::HostDetails(id) => self.host_details(ui, &view, id, &mut commands),
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
        } else if let Some(host) = &view.waking {
            waking(&ctx, host, &mut commands);
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
                Screen::HostDetails(id) => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
                    }
                    let name = view.hosts.get(id).map_or("", |h| h.display_name()).to_owned();
                    ui.label(RichText::new(name).font(semibold(22.0)).color(PAPER));
                }
                Screen::Settings => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
                    }
                    ui.label(RichText::new(tr("Settings")).font(semibold(22.0)).color(PAPER));
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
                    ui.label(RichText::new(tr("Profiles")).font(semibold(22.0)).color(PAPER));
                }
                Screen::Controllers => {
                    if text_button(ui, "‹  Settings").clicked() { self.back(commands); }
                    ui.label(RichText::new(tr("Controller mappings")).font(semibold(22.0)).color(PAPER));
                }
                Screen::Shortcuts => {
                    if text_button(ui, "‹  Settings").clicked() { self.back(commands); }
                    ui.label(RichText::new(tr("Keyboard shortcuts")).font(semibold(22.0)).color(PAPER));
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
                                        resume: None,
                                        named: host.running_app(),
                                    };
                                    self.focus_dialog = true;
                                }
                            }
                            if text_button(ui, "PC settings").clicked() {
                                commands.push(Command::HostSettings(id));
                            }
                        }
                        Screen::HostDetails(_) | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::Profiles | Screen::ProfileSettings(_) | Screen::Controllers | Screen::Shortcuts => {}
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
        let text = RichText::new(tr(name))
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
        ui.label(RichText::new(tr("Your PCs")).font(semibold(30.0)).color(ON_SURFACE));
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
                    ui.label(RichText::new(tr("Looking for PCs…")).font(semibold(18.0)));
                } else {
                    ui.label(RichText::new(tr("Creating this device's pairing key…")).font(semibold(18.0)));
                }
                ui.label(
                    RichText::new(tr("Start Sunshine, Apollo, Vibepollo, Rubylight or GeForce Experience on your PC."))
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
                RichText::new(tr("Options for a PC: F2, right-click, the three dots, or the controller's X button. Ctrl+N adds a PC."))
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
                    ui.label(RichText::new(tr("Loading the library…")).color(ON_SURFACE_VARIANT));
                });
            }
            Apps::Failed(e) => {
                ui.label(RichText::new(tr("The library could not be loaded.")).font(semibold(18.0)));
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
                ui.label(RichText::new(tr("Game settings: right-click a game, press F2, or use the controller's left face button.")).color(ON_SURFACE_VARIANT));
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
                ui.label(RichText::new(tr("No hosts yet")).font(semibold(18.0)));
                ui.label(RichText::new(tr("Pair Nightjar with a PC to see its games here.")).color(ON_SURFACE_VARIANT));
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
                ui.label(RichText::new(tr("Delete (keyboard) or Y (gamepad) removes the focused favourite.")).font(semibold(13.0)).color(ON_SURFACE_VARIANT));
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
                    ui.label(RichText::new(tr("No games to show. Games appear here once a Rubylight PC is online.")).color(ON_SURFACE_VARIANT));
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
                    favourite_menu(&response, &sources, &self.favourites, entry.best().host, entry.best().app_id, commands);
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
            ui.label(RichText::new(tr("This game is no longer in the PC's library.")).font(semibold(18.0)));
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
                    GameState::Running => (trf("Running on {}", &[&source.name]), MOON),
                    GameState::Suspended => (trf("Suspended on {}", &[&source.name]), TERTIARY),
                    GameState::Idle => (trf("On {}", &[&source.name]), ON_SURFACE_VARIANT),
                };
                ui.label(RichText::new(status).font(semibold(16.0)).color(color));
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

    fn shortcuts_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
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
        scroll(ui, "controller-mapping", |ui| {
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
    fn host_details(&mut self, ui: &mut Ui, view: &View<'_>, id: u64, commands: &mut Vec<Command>) {
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
        scroll(ui, "host-details", |ui| {
            section(ui, "Host");
            detail(ui, tr("Host software"), &software);
            ui.add_space(8.0);
            match &web_ui {
                None => {}
                Some(Ok(url)) => {
                    let button = row(ui, "Host web UI", "Manage the host's applications, paired devices and settings in its web UI. Sign in with the web UI's own account; pairing does not sign you in. Remote access requires a reachable web UI port.", |ui| {
                        quiet_button(ui, "Open in browser")
                    });
                    self.focus_item(&button, "web-ui", true);
                    if button.clicked() {
                        commands.push(Command::OpenWebUi(url.clone()));
                    }
                    ui.label(RichText::new(url).font(regular(13.0)).color(OUTLINE));
                }
                // The PC has a web UI, but its saved address makes no link to it.
                Some(Err(why)) => {
                    ui.label(RichText::new(tr("Host web UI")).font(semibold(15.0)).color(ON_SURFACE));
                    ui.label(RichText::new(why).color(ERROR));
                }
            }
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
        });
    }

    fn profiles_page(&mut self, ui: &mut Ui, commands: &mut Vec<Command>) {
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

    fn settings_page(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let display = self.display;
        let tearing = self.tearing_supported;
        let layered = self.screen != Screen::Settings;
        if layered {
            ui.label(match self.screen {
                Screen::ProfileSettings(_) => tr("Unchecked settings follow your main settings."),
                Screen::HostSettings(_) => tr("Unchecked settings follow the active profile and main settings."),
                _ => tr("Unchecked settings follow this PC, the active profile, and main settings."),
            });
            if text_button(ui, "Reset all to inherited").clicked() {
                self.edit_overrides = Overrides::default();
                self.edit_settings = self.inherited_settings.clone();
            }
        } else {
            let active = self.profiles.entries.iter().find(|p| Some(p.id) == self.profiles.active)
                .map_or(tr("Main settings"), |p| p.name.as_str());
            let manage = row(ui, "Profiles", &trf("Active: {}. PCs and games can override individual settings.", &[&active]), |ui| quiet_button(ui, "Manage profiles"));
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
            let native = trf("Native ({}x{})", &[&display.width, &display.height]);
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
                        step_value(ui, &mut s.width, 320..=7680, 16.0, |d| d);
                        ui.label("×");
                        step_value(ui, &mut s.height, 240..=4320, 16.0, |d| d);
                    }).response
                });
            }
            let display_fps = display.refresh_hz.round() as u32;
            let fps_text = if s.fps == 0 {
                trf("Display ({} Hz)", &[&display_fps])
            } else {
                trf("{} fps", &[&s.fps])
            };
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["fps"], "Frame rate", "Display matches this monitor's refresh rate.", |ui, s| {
                egui::ComboBox::from_id_salt("fps")
                    .selected_text(fps_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.fps, 0, trf("Display ({} Hz)", &[&display_fps]));
                        for fps in [30, 60, 90, 120, 144, 165, 240] {
                            ui.selectable_value(&mut s.fps, fps, trf("{} fps", &[&fps]));
                        }
                    })
                    .response
            });
            let (w, h, fps, bitrate) = stream_mode(s, display);
            let recommended = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, display).3;
            let mut automatic = s.bitrate_kbps == 0;
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["bitrate_kbps"], "Automatic bitrate",
                &trf("{} Mbps for {}x{} at {} fps.", &[&format!("{:.0}", f64::from(recommended) / 1000.0), &w, &h, &fps]),
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
                let note = if s.codec.is_pyrowave() { en("PyroWave maintains a minimum bitrate for this size and frame rate.") }
                    else { en("Higher is sharper and needs a faster network.") };
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
                egui::ComboBox::from_id_salt("quality").selected_text(tr(s.adaptive_quality.label())).show_ui(ui, |ui| {
                    for mode in [crate::config::QualityMode::Off, crate::config::QualityMode::Motion, crate::config::QualityMode::Detail, crate::config::QualityMode::Battery] {
                        ui.selectable_value(&mut s.adaptive_quality, mode, tr(mode.label()));
                    }
                }).response
            });
            if s.adaptive_quality != crate::config::QualityMode::Off {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["quality_min_short_edge"], "Minimum resolution", "Shortest edge, in pixels. Never increases the selected stream size.", |ui, s| {
                    step_value(ui, &mut s.quality_min_short_edge, 256..=2160, 8.0, |d| d.speed(2).suffix(" px"))
                });
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["quality_min_fps"], "Minimum frame rate", "Automatic quality stays above this rate.", |ui, s| {
                    step_value(ui, &mut s.quality_min_fps, 15..=240, 5.0, |d| d.suffix(" fps"))
                });
            }
            // The GPU decoder's probe runs in the background, on the renderer's adapter and the HDR
            // plane format, and only when it matters: PyroWave is selected or the codec menu is
            // open. PyroWave is not offered while it fails, and the reason is shown with the
            // decoder setting.
            let probe_device = self.gpu_device.map_or(std::ptr::null_mut(), |d| d.0);
            let (gpu_mode, ten_bit) = (s.pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu, s.hdr);
            let gpu_probe = move |ui: &Ui| {
                if !gpu_mode { return None; }
                let probe = crate::stream::pyrowave_mode::settings_probe(probe_device, ten_bit);
                if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
                probe.and_then(Result::err)
            };
            let gpu_decoder_error = if s.codec.is_pyrowave() { gpu_probe(ui) } else { None };
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["codec"], "Codec", "Automatic picks HEVC, then H.264. PyroWave needs GPU rendering; restart after selecting it on Linux.", |ui, s| {
                egui::ComboBox::from_id_salt("codec")
                    .selected_text(match s.codec {
                        CodecPreference::Auto => tr("Automatic"),
                        CodecPreference::H264 => "H.264",
                        CodecPreference::Hevc => "HEVC",
                        CodecPreference::Av1 => "AV1",
                        CodecPreference::PyroWave => "PyroWave 4:2:0",
                        CodecPreference::PyroWave444 => "PyroWave 4:4:4",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.codec, CodecPreference::Auto, tr("Automatic"));
                        ui.selectable_value(&mut s.codec, CodecPreference::H264, "H.264");
                        ui.selectable_value(&mut s.codec, CodecPreference::Hevc, "HEVC");
                        ui.selectable_value(&mut s.codec, CodecPreference::Av1, "AV1");
                        let menu_error = gpu_probe(ui);
                        if menu_error.is_none() {
                            ui.selectable_value(&mut s.codec, CodecPreference::PyroWave, "PyroWave 4:2:0");
                            ui.selectable_value(&mut s.codec, CodecPreference::PyroWave444, "PyroWave 4:4:4");
                        }
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
                egui::ComboBox::from_id_salt("video-scaling").selected_text(tr(s.video_scaling.label())).show_ui(ui, |ui| {
                    for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                        ui.selectable_value(&mut s.video_scaling, mode, tr(mode.label()));
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
                        DecoderPreference::Auto => tr("Automatic"),
                        DecoderPreference::Software => tr("Software"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Auto, tr("Automatic"));
                        ui.selectable_value(&mut s.decoder, DecoderPreference::Software, tr("Software"));
                    })
                    .response).inner
            });
            let decoder_note = match &gpu_decoder_error {
                Some(reason) => trf("CPU decodes on the CPU and reconstructs on the GPU. GPU decodes with the PyroWave SDK and never falls back to CPU. GPU decoding is unavailable here: {}", &[reason]),
                None => tr("CPU decodes on the CPU and reconstructs on the GPU. GPU decodes with the PyroWave SDK and never falls back to CPU.").to_owned(),
            };
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["pyrowave_decoder"], "PyroWave decoder", &decoder_note, |ui, s| {
                egui::ComboBox::from_id_salt("pyrowave-decoder")
                    .selected_text(tr(s.pyrowave_decoder.label()))
                    .show_ui(ui, |ui| {
                        for mode in [crate::config::PyroWaveDecoder::Cpu, crate::config::PyroWaveDecoder::Gpu] {
                            ui.selectable_value(&mut s.pyrowave_decoder, mode, tr(mode.label()));
                        }
                    })
                    .response
            });

            section_at(ui, "Display & image", &mut jump);
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["virtual_display"], "Virtual display", "Use a separate display on hosts that support it.", |ui, s| toggle(ui, &mut s.virtual_display));
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["host_display"], "Display arrangement", "Applied only to normal streams on hosts that support display modes.", |ui, s| {
                egui::ComboBox::from_id_salt("host-display")
                    .selected_text(tr(host_display_name(s.host_display)))
                    .show_ui(ui, |ui| {
                        for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary,
                            HostDisplay::Extended, HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
                            ui.selectable_value(&mut s.host_display, mode, tr(host_display_name(mode)));
                        }
                    }).response
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["virtual_display_scale"], "PC display scaling", "Text and interface size on the virtual display.", |ui, s| {
                ui.add(egui::Slider::new(&mut s.virtual_display_scale, 100..=300).step_by(25.0).suffix("%"))
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["window_mode"], "Window", "Borderless covers the display without changing its mode.", |ui, s| {
                egui::ComboBox::from_id_salt("window")
                    .selected_text(match s.window_mode {
                        WindowMode::Fullscreen => tr("Fullscreen"),
                        WindowMode::Borderless => tr("Borderless"),
                        WindowMode::Windowed => tr("Windowed"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.window_mode, WindowMode::Fullscreen, tr("Fullscreen"));
                        ui.selectable_value(&mut s.window_mode, WindowMode::Borderless, tr("Borderless"));
                        ui.selectable_value(&mut s.window_mode, WindowMode::Windowed, tr("Windowed"));
                    })
                    .response
            });
            let vrr_note = if tearing || cfg!(not(windows)) {
                en("G-Sync, FreeSync or HDMI VRR is on for this display.")
            } else {
                en("This system cannot present with tearing, which VRR needs here.")
            };
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["vrr"], "Variable refresh rate", vrr_note, |ui, s| toggle(ui, &mut s.vrr));
            let immediate_packed = cfg!(windows) && s.experimental_packed_444 && tearing;
            ui.add_enabled_ui(s.vrr && !immediate_packed, |ui| {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["present_at_host_times"], "Show frames at the PC's frame times",
                    if immediate_packed {
                        en("Full chroma presents frames immediately with VRR, without the extra timing delay.")
                    } else {
                        en("Each frame appears at the moment the game made it, plus a steady delay, so motion stays even.")
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
                        MouseMode::Capture => tr("Captured"),
                        MouseMode::Absolute => tr("Desktop pointer"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Capture, tr("Captured"));
                        ui.selectable_value(&mut s.mouse_mode, MouseMode::Absolute, tr("Desktop pointer"));
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
                        egui::ComboBox::from_id_salt("analog-scrolling").selected_text(match s.analog_scrolling { AnalogScrolling::Off => tr("Off"), AnalogScrolling::Left => tr("Left"), AnalogScrolling::Right => tr("Right") }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Off, tr("Off"));
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Left, tr("Left"));
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Right, tr("Right"));
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
                        AudioChannels::Stereo => tr("Stereo"),
                        AudioChannels::Surround51 => tr("5.1 surround"),
                        AudioChannels::Surround71 => tr("7.1 surround"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.audio, AudioChannels::Stereo, tr("Stereo"));
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround51, tr("5.1 surround"));
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround71, tr("7.1 surround"));
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
                    let button = ui.add_enabled(enabled, egui::Button::new(tr("Test speed")));
                    if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: false }); }
                    button
                });
                let note = host.record.pace.as_ref().map_or_else(|| en("Find a frame pace for this network. May take several minutes and uses substantial network data; stop other streams first.").into(),
                    |p| trf("Saved {} Mbps · {}. Recalibrate after changing the network or stream bitrate.", &[&p.pace_mbps, &if p.qualified { tr("confirmed") } else { tr("best available") }]));
                row(ui, "PyroWave frame pace", &note, |ui| {
                    ui.horizontal(|ui| {
                        let enabled = online && host.info.as_ref().is_some_and(|i| i.pyrowave_burst_probe);
                        let button = ui.add_enabled(enabled, egui::Button::new(tr("Calibrate")));
                        if button.clicked() { commands.push(Command::NetworkTest { host: host.id, calibrate: true }); }
                        if host.record.pace.is_some() && text_button(ui, "Clear").clicked() { commands.push(Command::ClearPace(host.id)); }
                        button
                    }).inner
                });
            }
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["auto_reconnect"], "Reconnect after an interruption", "Try to resume the same running app up to five times within one minute.", |ui, s| {
                toggle(ui, &mut s.auto_reconnect)
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["tell_host_wireless"], "Tell the PC when on Wi-Fi", "A Rubylight PC then paces the stream for Wi-Fi from the start. Other PCs are not told.", |ui, s| {
                toggle(ui, &mut s.tell_host_wireless)
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["tell_host_link_speed"], "Tell the PC the link speed", "Lets the PC limit its sending rate to this device's wired port speed.", |ui, s| {
                toggle(ui, &mut s.tell_host_link_speed)
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["network_grace_ms"], "Network interruption grace", "Keep the session alive through brief network interruptions.", |ui, s| {
                egui::ComboBox::from_id_salt("network-grace")
                    .selected_text(trf("{} seconds", &[&(s.network_grace_ms / 1000)]))
                    .show_ui(ui, |ui| {
                        for ms in [0, 3000, 5000, 10000, 15000] {
                            ui.selectable_value(&mut s.network_grace_ms, ms, trf("{} seconds", &[&(ms / 1000)]));
                        }
                    }).response
            });
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["client_roaming"], "Client roaming (Experimental)", "Keep the stream running when this device changes networks. Works with Rubylight PCs that support it.", |ui, s| {
                toggle(ui, &mut s.client_roaming)
            });
            if s.client_roaming {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["roaming_metered"], "Continue on mobile data", "Let the stream move onto mobile data or another metered network. Streaming uses a lot of data.", |ui, s| {
                    toggle(ui, &mut s.roaming_metered)
                });
            }
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["unbatched_input"], "Instant mouse and pen input", "Send mouse and pen movements immediately.", |ui, s| {
                toggle(ui, &mut s.unbatched_input)
            });
            // The engine sets socket and thread priorities on Linux only; on Windows the switch would do nothing.
            if cfg!(target_os = "linux") {
                setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["network_priority"], "Prioritize stream traffic", "Give stream traffic priority on this device.", |ui, s| {
                    toggle(ui, &mut s.network_priority)
                });
            }

            if !layered {
                section_at(ui, "App", &mut jump);
                row(ui, "Open on Home", "Start on the games to continue and your library. Off starts on the list of PCs.", |ui| {
                    toggle(ui, &mut s.console_home)
                });
                row(ui, "Language", "The language of the launcher and the stream menu. System default follows the operating system.", |ui| {
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(match s.language {
                            Language::System => tr("System default"),
                            Language::English => "English",
                            Language::German => "Deutsch",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.language, Language::System, tr("System default"));
                            ui.selectable_value(&mut s.language, Language::English, "English");
                            ui.selectable_value(&mut s.language, Language::German, "Deutsch");
                        })
                        .response
                });
                row(ui, "UI zoom", "Makes everything bigger or smaller. Ctrl+= and Ctrl+- change it, Ctrl+0 resets it.", |ui| {
                    egui::ComboBox::from_id_salt("ui-zoom")
                        .selected_text(format!("{}%", crate::config::nearest_zoom(s.ui_zoom)))
                        .show_ui(ui, |ui| {
                            for step in UI_ZOOM_STEPS {
                                ui.selectable_value(&mut s.ui_zoom, step, format!("{step}%"));
                            }
                        })
                        .response
                });
                row(ui, "Logs", "Nightjar keeps a log of the last two runs. Attach nightjar.log, and nightjar.prev.log if the problem was before a restart, to a bug report.", |ui| {
                    let button = quiet_button(ui, "Open logs folder");
                    if button.clicked() { commands.push(Command::OpenLogs); }
                    button
                });
                row(ui, "High contrast", "Black surfaces, white text and thicker outlines and focus rings.", |ui| {
                    toggle(ui, &mut s.high_contrast)
                });
                row(ui, "Reduce motion", "Stops fades, slides and the spinning wheel. Follow system uses the Windows animation setting; other systems do not report one, so motion stays on.", |ui| {
                    egui::ComboBox::from_id_salt("reduce-motion")
                        .selected_text(match s.reduce_motion {
                            ReduceMotion::System => tr("Follow system"),
                            ReduceMotion::Reduce => tr("Reduce"),
                            ReduceMotion::Full => tr("Full motion"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.reduce_motion, ReduceMotion::System, tr("Follow system"));
                            ui.selectable_value(&mut s.reduce_motion, ReduceMotion::Reduce, tr("Reduce"));
                            ui.selectable_value(&mut s.reduce_motion, ReduceMotion::Full, tr("Full motion"));
                        })
                        .response
                });
            }
            section_at(ui, "Advanced", &mut jump);
            setting_row(ui, s, layered.then_some((&inherited, &mut *overrides)), &["experimental_packed_444"], "Experimental full chroma", "Requires the host option and HEVC or AV1. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave(), |ui| toggle(ui, &mut s.experimental_packed_444)).inner);
            ui.add_space(18.0);
            ui.label(
                RichText::new(tr(
                    "While streaming: Ctrl+Alt+Shift+Q leaves, +Z releases input, +X toggles fullscreen, +D minimizes. \
                     On a controller, Start+Select+LB+RB leaves.",
                ))
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
                            ui.label(RichText::new(host.display_name()).font(semibold(22.0)));
                            ui.add_space(6.0);
                            // The longest list scrolls in a small window (1280x720 at 150 %): room
                            // for the name, the frame and the Wake note.
                            let room = ui.ctx().content_rect().height() - 160.0;
                            egui::ScrollArea::vertical().id_salt("host-options").max_height(room).content_margin(Margin::same(6)).show(ui, |ui| {
                                let follow = Id::unique("host-options-focus");
                                let mut last = ui.ctx().data(|d| d.get_temp::<Option<Id>>(follow)).flatten();
                                crate::gui::follow_focus(ui, &mut last);
                                ui.ctx().data_mut(|d| d.insert_temp(follow, last));
                                for (n, option) in options.iter().enumerate() {
                                    if *option == HostOption::Remove {
                                        ui.separator();
                                    }
                                    let label = RichText::new(tr(option.label())).font(semibold(16.0));
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
                                        ui.label(RichText::new(tr(why)).font(regular(13.0)).color(OUTLINE));
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
                            if host.reach == Reach::Offline && host.record.mac.is_none() {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(tr("Pair once while the PC is on to enable Wake."))
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
                        ui.label(RichText::new(tr("Rename PC")).font(semibold(22.0)));
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
                        ui.label(RichText::new(trf("Remove {}?", &[&name])).font(semibold(22.0)));
                        let body = if paired {
                            tr("This deletes the PC, its settings and its saved library from this device. The PC still lists this device as paired until you unpair it.")
                        } else {
                            tr("This deletes the PC, its settings and its saved library from this device.")
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
                        ui.label(RichText::new(trf("Unpair {}?", &[&name])).font(semibold(22.0)));
                        ui.label(
                            RichText::new(tr("The PC drops this device and its saved library is forgotten. It stays in the list; pair again to stream."))
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
            Dialog::ConfirmSleep { host, ask, name, offer } => {
                let (id, ask) = (*host, *ask);
                egui::Modal::new(Id::unique("confirm-sleep"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Put {} to sleep?", &[&name])).font(semibold(22.0)));
                        ui.label(
                            RichText::new(tr("Experimental. The PC sleeps only while no app or stream runs on it. Wake it with Wake up."))
                                .color(ON_SURFACE_VARIANT),
                        );
                        ui.add_space(12.0);
                        if offer.is_none() {
                            ui.horizontal(|ui| {
                                spinner(ui, 9.0);
                                ui.label(RichText::new(tr("Asking the PC…")).color(ON_SURFACE_VARIANT));
                            });
                            ui.add_space(10.0);
                        }
                        ui.horizontal(|ui| {
                            // Cancel first: putting the PC to sleep is never one stray press away.
                            let cancel = quiet_button(ui, "Cancel");
                            if focus_first {
                                cancel.request_focus();
                                focused = true;
                            }
                            if cancel.clicked() {
                                close = true;
                            }
                            if let Some((token, at)) = offer {
                                if primary_button(ui, "Sleep PC").clicked() {
                                    commands.push(Command::Sleep { host: id, ask, token: token.clone(), at: *at });
                                    close = true;
                                }
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
                        ui.label(RichText::new(tr("Connect to your PC")).font(semibold(22.0)));
                        for step in PAIRING_STEPS {
                            ui.label(RichText::new(step).color(ON_SURFACE_VARIANT));
                        }
                        ui.add_space(12.0);
                        let done = primary_button(ui, "Got it");
                        if focus_first {
                            done.request_focus();
                            focused = true;
                        }
                        if done.clicked() {
                            close = true;
                        }
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
                        ui.label(RichText::new(trf("Pairing with {} failed", &[&name])).font(semibold(22.0)));
                        ui.label(RichText::new(tr(body.as_str())).color(ON_SURFACE_VARIANT));
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
                    ui.label(RichText::new(tr("Network test")).font(semibold(22.0)));
                    ui.horizontal(|ui| { spinner(ui, 10.0); ui.label(tr(stage.as_str())); });
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
                        ui.label(RichText::new(if id.is_some() { tr("Edit shortcut") } else { tr("New shortcut") }).font(semibold(22.0)));
                        let edit = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Shortcut name")));
                        if focus_first { edit.request_focus(); focused = true; }
                        ui.add(egui::TextEdit::singleline(chord).hint_text("CTRL + ALT + DELETE"));
                        ui.label(tr("Use letters, digits, F1–F24, CTRL, SHIFT, ALT, WIN, TAB, ENTER, ESC, arrows or other key names."));
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
                        ui.label(RichText::new(trf("Delete {}?", &[&name])).font(semibold(22.0)));
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
                        ui.label(RichText::new(tr("Import keyboard shortcuts?")).font(semibold(22.0)));
                        ui.label(trf("Replace your {} saved shortcuts with {} from this file.", &[&self.shortcuts.data.len(), &entries.data.len()]));
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
                        ui.label(RichText::new(if id.is_some() { tr("Rename profile") } else { tr("New profile") }).font(semibold(22.0)));
                        let edit = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Profile name")));
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
                        ui.label(RichText::new(trf("Delete {}?", &[&name])).font(semibold(22.0)));
                        ui.label(tr("If this profile is active, new streams will use main settings instead."));
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
                        ui.label(tr("This changes the session running on the PC. Ending an app can lose unsaved progress."));
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
                        ui.label(RichText::new(tr("Pair with one-time PIN")).font(semibold(22.0)));
                        ui.label(tr("Enter the PIN and passphrase created on the PC."));
                        let edit = ui.add(egui::TextEdit::singleline(pin).hint_text(tr("PIN")));
                        if focus_first {
                            edit.request_focus();
                            focused = true;
                        }
                        ui.add(egui::TextEdit::singleline(passphrase).password(true).hint_text(tr("Passphrase")));
                        if let Some(error) = error {
                            ui.label(RichText::new(error.as_str()).color(ERROR));
                        }
                        ui.horizontal(|ui| {
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
                        ui.label(RichText::new(tr("Add a PC")).font(semibold(22.0)));
                        ui.label(
                            RichText::new(tr("Its IP address or name, with :port if the host uses another HTTP port."))
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
                let name = view.hosts.get(*host).map_or(tr("the PC"), |h| h.display_name()).to_owned();
                let pin = pin.clone();
                egui::Modal::new(Id::unique("pairing"))
                    .frame(frame)
                    .backdrop_color(Color32::from_black_alpha(150))
                    .show(ctx, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(trf("Pair with {}", &[&name])).font(semibold(22.0)));
                        if !pin.is_empty() {
                            ui.label(
                                RichText::new(tr("Enter this PIN on the PC: in the host's web page (Sunshine, Apollo, Vibepollo and Rubylight have a PIN page), or in the prompt GeForce Experience shows."))
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
                                    *MOON,
                                );
                            }
                        });
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            spinner(ui, 9.0);
                            ui.label(RichText::new(tr("Waiting for the PC…")).color(ON_SURFACE_VARIANT));
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
                        ui.label(RichText::new(tr(body.as_str())).color(ON_SURFACE_VARIANT));
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
                        ui.label(RichText::new(tr("Close the game on the PC?")).font(semibold(22.0)));
                        ui.label(RichText::new(trf("{} closes on the PC. Unsaved progress is lost.", &[&name])).color(ON_SURFACE_VARIANT));
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
                        ui.label(RichText::new(title).font(semibold(22.0)));
                        ui.label(RichText::new(tr("Unsaved progress in the running app is lost.")).color(ON_SURFACE_VARIANT));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
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
    fn close_dialog(&mut self) {
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
    fn choose_host_option(&mut self, host: &Host, option: HostOption, commands: &mut Vec<Command>) -> Option<Dialog> {
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
    fn sleep_message(&mut self, host: u64, title: String, body: String) {
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

    fn connecting(&mut self, ctx: &egui::Context, c: &ConnectingView, commands: &mut Vec<Command>) {
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
                ui.label(RichText::new(&c.app).font(semibold(24.0)));
                ui.label(RichText::new(trf("on {}", &[&c.host])).color(ON_SURFACE_VARIANT));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    spinner(ui, 10.0);
                    ui.label(RichText::new(tr(&c.stage)).color(ON_SURFACE));
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

/// "Waking <PC>…" while the wake packets go out, with a way to stop.
fn waking(ctx: &egui::Context, host: &str, commands: &mut Vec<Command>) {
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
            ui.label(RichText::new(trf("Waking {}…", &[&host])).font(semibold(24.0)));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                spinner(ui, 10.0);
                ui.label(RichText::new(tr("Sending Wake-on-LAN until the PC answers.")).color(ON_SURFACE));
            });
            ui.add_space(14.0);
            let r = quiet_button(ui, "Cancel");
            r.request_focus();
            if r.clicked() {
                commands.push(Command::CancelWake);
            }
        });
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

/// A line of a details page: what it is, and its value.
fn detail(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(label).font(semibold(15.0)).color(ON_SURFACE));
        ui.label(RichText::new(value).font(regular(15.0)).color(ON_SURFACE_VARIANT));
    });
}

/// What the remote access check found; an error as it came.
fn remote_report(ui: &mut Ui, report: &crate::host_page::Report) {
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
            .font(regular(13.0))
            .color(OUTLINE),
    );
}

fn host_display_name(mode: HostDisplay) -> &'static str {
    match mode {
        HostDisplay::HostDefault => en("Host default"),
        HostDisplay::Exclusive => en("Virtual display only"),
        HostDisplay::ExtendedPrimary => en("Extend as primary"),
        HostDisplay::Extended => en("Extend"),
        HostDisplay::ExtendedIsolated => en("Extend, isolated"),
        HostDisplay::ExtendedPrimaryIsolated => en("Extend as primary, isolated"),
        HostDisplay::Physical => en("Physical display"),
    }
}

/// What selecting a PC's card does: open its apps, check again, or pair.
/// The three dots on a PC's card: a second stop for focus that opens the options.
fn options_button(ui: &mut Ui, card: Rect, id: Id) -> Response {
    let rect = Rect::from_min_size(card.right_top() + vec2(-42.0, 10.0), vec2(32.0, 32.0));
    let response = ui.interact(rect, id, Sense::click()).on_hover_text(tr("Options"));
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

/// The library entry of the app the PC is running, when its library is known.
fn running_entry(host: &Host) -> Option<&nightjar_core::http::App> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None) && !a.is_control_action()),
        _ => None,
    }
}

/// A name that starts a sentence: the stand-in "the running app" gets its capital, a game's own
/// name stays as it is spelled.
fn at_sentence_start(name: &str) -> String {
    if name != tr("the running app") {
        return name.to_owned();
    }
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

fn running_name(host: &Host) -> String {
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
fn running_app_id(host: &Host) -> Option<i32> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None)).map(|a| a.id),
        _ => None,
    }
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
    ui.label(RichText::new(tr(title).to_uppercase()).font(semibold(13.0)).color(MOON));
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
            let response = ui.checkbox(&mut own, tr("Override"));
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
        .fill(*SURFACE)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let control_width = 240.0;
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - control_width - 16.0).max(200.0));
                    ui.label(RichText::new(tr(label)).font(semibold(15.0)).color(ON_SURFACE));
                    ui.label(RichText::new(tr(description)).font(regular(13.0)).color(ON_SURFACE_VARIANT));
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
        (Reach::Offline, _) => (tr("Offline").to_owned(), OUTLINE),
        (Reach::Unknown, _) => (tr("Checking…").to_owned(), OUTLINE),
        (Reach::Online, false) => (tr("Not paired · select to pair").to_owned(), TERTIARY),
        (Reach::Online, true) => match host.running_app() {
            Some(_) => (tr("Paired · an app is running").to_owned(), MOON),
            None => (tr("Paired · ready").to_owned(), MOON),
        },
    };
    // The moon dot: full when ready, a ring otherwise.
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + 30.0);
    if host.reach == Reach::Online && host.paired() {
        painter.circle_filled(dot, 7.0, MOON);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, color));
    }
    // A long name ends in "…" before the three dots.
    let mut name = egui::text::LayoutJob::simple_singleline(host.display_name().to_owned(), semibold(19.0), *ON_SURFACE);
    name.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 44.0 - 46.0);
    let name = painter.layout_job(name);
    painter.galley(Pos2::new(rect.left() + 44.0, rect.top() + 30.0 - name.size().y / 2.0), name, *ON_SURFACE);
    painter.text(
        Pos2::new(rect.left() + 20.0, rect.top() + 62.0),
        Align2::LEFT_CENTER,
        &status,
        regular(14.0),
        *color,
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
        *OUTLINE,
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
            painter.text(image.center(), Align2::CENTER_CENTER, initials, semibold(40.0), *OUTLINE);
        }
    }
    if running {
        let badge = Rect::from_min_size(image.left_top() + vec2(10.0, 10.0), vec2(76.0, 24.0));
        painter.rect_filled(badge, CornerRadius::same(12), MOON);
        painter.text(badge.center(), Align2::CENTER_CENTER, tr("Running"), semibold(12.5), *ON_MOON);
    }
}

/// Delete, or Y on a gamepad (mapped to F3), takes the star off the focused favourite.
fn unstar_pressed(ui: &Ui) -> bool {
    ui.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) | i.consume_key(egui::Modifiers::NONE, egui::Key::F3)
    })
}

/// The right-click menu of a game card: star the game or take its star off.
fn favourite_menu(
    response: &Response,
    sources: &[Source<'_>],
    favourites: &[crate::config::Favourite],
    host: u64,
    app: u32,
    commands: &mut Vec<Command>,
) {
    let Some(star) = console::favourite_for(sources, host, app) else { return };
    let starred = favourites.iter().any(|f| f.same_game(&star));
    response.context_menu(|ui| {
        if ui.button(if starred { tr("Remove from favourites") } else { tr("Add to favourites") }).clicked() {
            commands.push(Command::ToggleFavourite(star));
            ui.close();
        }
    });
}

fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H + 34.0), Sense::click());
    let image = Rect::from_min_size(rect.min, vec2(POSTER_W, POSTER_H));
    paint_cover(ui, image, name, art, running);
    let painter = ui.painter_at(rect.expand(6.0));
    let label_color = if response.has_focus() { MOON } else { ON_SURFACE };
    let galley = painter.layout(name.to_owned(), semibold(14.5), *label_color, POSTER_W);
    painter.galley(Pos2::new(rect.left() + 2.0, image.bottom() + 8.0), galley, *label_color);
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
    let galley = text.layout_no_wrap(title.to_owned(), semibold(18.0), *ON_SURFACE);
    text.galley(Pos2::new(rect.left() + 44.0, rect.top() + 28.0 - galley.size().y / 2.0), galley, *ON_SURFACE);
    text.text(Pos2::new(rect.left() + 20.0, rect.top() + 60.0), Align2::LEFT_CENTER, line, regular(14.0), *color);
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

    #[test]
    fn the_stand_in_names_fit_their_sentences() {
        assert_eq!(at_sentence_start(tr("the running app")), "the running app".replacen('t', "T", 1));
        assert_eq!(at_sentence_start("osu!"), "osu!");
        crate::strings::set(crate::strings::Lang::De);
        assert_eq!(at_sentence_start(tr("the running app")), "Die laufende App");
        assert_eq!(trf("Pairing with {} failed", &[&tr("the PC")]), "Koppeln mit dem PC fehlgeschlagen");
        assert_eq!(trf("Pair with {}", &[&tr("the PC")]), "Mit dem PC koppeln");
        assert_eq!(trf("Quit {}?", &[&tr("the running app")]), "die laufende App beenden?");
    }

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
    fn start_from_a_library_or_game_page_returns_to_it() {
        for page in [Screen::Apps(1), Screen::Game { host: 1, app: 5 }] {
            let mut l = launcher(true, true);
            l.go(Screen::Home);
            l.open(page);
            l.focused_key = Some("play".into());
            l.open_settings();
            assert_eq!(l.screen, Screen::Settings);
            let mut commands = Vec::new();
            l.back(&mut commands);
            assert_eq!(l.screen, page, "B returns to where Start was pressed");
            assert_eq!(l.restore_key.as_deref(), Some("play"));
            l.back(&mut commands);
            assert_eq!(l.screen, Screen::Home, "and the page still goes back to the root");
        }
        // From a destination, Settings replaces it as before.
        let mut l = launcher(false, false);
        l.go(Screen::Hosts);
        l.open_settings();
        let mut commands = Vec::new();
        l.back(&mut commands);
        assert_eq!(l.screen, Screen::Hosts);
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
        /// The window; tall, a test can see a whole page at once.
        size: egui::Vec2,
        /// The text drawn by the last frame, in paint order.
        drawn: Vec<String>,
        /// The same text with where it landed and the clip it was drawn in.
        placed: Vec<(String, Rect, Rect)>,
        waking: Option<String>,
    }

    impl Frames {
        fn new(record: HostRecord, reach: Reach) -> Self {
            Self::with(record, reach, None, Vec::new())
        }

        /// A PC as the app sees it after the PC answered (`info`) and its classic list loaded.
        fn with(record: HostRecord, reach: Reach, info: Option<nightjar_core::http::ServerInfo>, apps: Vec<nightjar_core::http::App>) -> Self {
            crate::theme::own_palette();
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            let mut l = launcher(false, false);
            l.go(Screen::Hosts);
            let mut hosts = Hosts::without_discovery(vec![record], Arc::new(|| {}));
            hosts.list[0].reach = reach;
            hosts.list[0].info = info;
            if !apps.is_empty() {
                hosts.list[0].apps = Apps::Loaded(apps);
            }
            let mut frames = Self { ctx, l, hosts, pads: Gamepads::default(), time: 0.0, size: vec2(1280.0, 720.0), drawn: Vec::new(), placed: Vec::new(), waking: None };
            frames.settle();
            frames
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Command> {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                focused: true,
                ..Default::default()
            };
            let mut commands = Vec::new();
            let mut output = self.ctx.run_ui(input, |ui| {
                commands = self.l.ui(ui, View { hosts: &self.hosts, pads: &self.pads, connecting: None, waking: self.waking.clone() });
            });
            output.textures_delta.clear();
            self.drawn.clear();
            self.placed.clear();
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut self.drawn);
                collect_placed(&clipped.shape, clipped.clip_rect, &mut self.placed);
            }
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

        /// Turns the mouse wheel down over the middle of the window, far enough to reach the end
        /// of what scrolls there.
        fn scroll_to_end(&mut self) {
            let centre = (self.size / 2.0).to_pos2();
            self.frame(vec![egui::Event::PointerMoved(centre)]);
            self.frame(vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, -1000.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            }]);
            for _ in 0..10 {
                self.settle();
            }
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

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    fn collect_placed(shape: &egui::Shape, clip: Rect, out: &mut Vec<(String, Rect, Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text.galley.rect.translate(text.pos.to_vec2()), clip)),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_placed(s, clip, out)),
            _ => {}
        }
    }

    /// What the PC said last, as the host sends it.
    fn answer(paired: bool, running: i32) -> nightjar_core::http::ServerInfo {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>{}</PairStatus>             <currentgame>{running}</currentgame><state>SUNSHINE_SERVER_{}</state><Permission>1</Permission></root>",
            u8::from(paired),
            if running == 0 { "FREE" } else { "BUSY" },
        );
        nightjar_core::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
    }

    fn game(id: i32) -> nightjar_core::http::App {
        nightjar_core::http::App {
            name: "Game".into(),
            id,
            hdr_supported: false,
            uuid: String::new(),
            host_index: None,
            art_version: String::new(),
            control: nightjar_core::http::AppControl::None,
            monitor_resume: false,
        }
    }

    /// One PC in one state, enough to reach every entry of the options list.
    struct State {
        name: &'static str,
        record: HostRecord,
        reach: Reach,
        info: Option<nightjar_core::http::ServerInfo>,
        apps: Vec<nightjar_core::http::App>,
    }

    impl State {
        fn frames(&self) -> Frames {
            Frames::with(self.record.clone(), self.reach, self.info.clone(), self.apps.clone())
        }
    }

    fn every_state() -> Vec<State> {
        let state = |name, record, reach, info, apps| State { name, record, reach, info, apps };
        vec![
            // Has the one-time PIN and the pairing help.
            state("unpaired", record("Unpaired", false, false), Reach::Online, Some(answer(false, 0)), Vec::new()),
            state("offline, can wake", record("Offline", true, true), Reach::Offline, None, Vec::new()),
            state("not reached yet, can wake", record("Asleep", true, true), Reach::Unknown, None, Vec::new()),
            state("paired", record("Paired", true, false), Reach::Online, Some(answer(true, 0)), Vec::new()),
            // Has Resume and Quit.
            state("running a game", record("Busy", true, false), Reach::Online, Some(answer(true, 7)), vec![game(7)]),
            state("can sleep", record("Sleepy", true, false), Reach::Online, Some(sleep_answer(true)), Vec::new()),
            // Lists Sleep PC, not selectable, with the reason.
            state("sleep not allowed", record("Guarded", true, false), Reach::Online, Some(sleep_answer(false)), Vec::new()),
        ]
    }

    /// A paired PC's answer with the power-action fields, sleep allowed for this device or not.
    fn sleep_answer(allowed: bool) -> nightjar_core::http::ServerInfo {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid>             <HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame>             <state>SUNSHINE_SERVER_FREE</state><PowerActionVersion>1</PowerActionVersion><SleepSupported>1</SleepSupported>             <SleepAllowed>{}</SleepAllowed></root>",
            u8::from(allowed),
        );
        nightjar_core::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
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
        let mut f = Frames::new(record("PC", true, false), Reach::Online);
        f.size = vec2(1280.0, 12000.0);
        f.l.go(Screen::Settings);
        f.settle();
        // The chips come first, then the page's headings in capitals.
        let chips: Vec<&str> = f.drawn.iter().map(String::as_str).filter(|t| SETTINGS_SECTIONS.contains(t)).collect();
        assert_eq!(chips, SETTINGS_SECTIONS, "the chips list the sections in order");
        let upper: Vec<String> = SETTINGS_SECTIONS.iter().map(|s| s.to_uppercase()).collect();
        let headings: Vec<&String> = f.drawn.iter().filter(|t| upper.contains(t)).collect();
        assert_eq!(headings, upper.iter().collect::<Vec<_>>(), "and the page has them in the same order");
    }

    #[test]
    fn the_pyrowave_gpu_probe_starts_only_with_pyrowave_selected_or_the_codec_menu_open() {
        use crate::config::{CodecPreference, PyroWaveDecoder};
        let started = || crate::stream::pyrowave_mode::settings_probe_started(true);
        let mut f = Frames::new(record("PC", true, false), Reach::Online);
        f.size = vec2(1280.0, 12000.0);
        f.l.settings.hdr = true;
        f.l.settings.pyrowave_decoder = PyroWaveDecoder::Gpu;
        f.l.settings.codec = CodecPreference::Hevc;
        f.l.go(Screen::Settings);
        f.settle();
        assert!(!started(), "an ordinary settings view does not probe");
        f.l.settings.codec = CodecPreference::PyroWave;
        f.settle();
        assert!(started(), "PyroWave selected probes, with the HDR plane format");
    }

    /// A 1280x720 window at 150 % zoom is 853x480 points. Everything drawn must stay inside it
    /// sideways (a cut-off button label shows as text outside its clip); a dialog, which does not
    /// scroll, must also stay inside it from top to bottom.
    fn fit_150(f: &mut Frames) {
        f.l.settings.ui_zoom = 150;
        f.l.settings.language = crate::config::Language::German;
        f.size = vec2(1280.0 / 1.5, 720.0 / 1.5);
        f.settle();
        assert!((f.ctx.zoom_factor() - 1.5).abs() < 1e-6, "the zoom setting reached egui");
    }

    fn assert_fits(f: &Frames, what: &str, whole_height: bool) {
        let screen = Rect::from_min_size(Pos2::ZERO, f.size);
        assert!(!f.placed.is_empty(), "{what}: nothing was drawn");
        for (text, rect, clip) in &f.placed {
            if text.trim().is_empty() {
                continue;
            }
            assert!(
                rect.left() >= clip.left() - 1.0 && rect.right() <= clip.right() + 1.0 && rect.right() <= screen.right() + 1.0,
                "{what}: {text:?} runs past its edge: text {rect:?}, clip {clip:?}, window {screen:?}"
            );
            if whole_height {
                assert!(rect.top() >= -1.0 && rect.bottom() <= screen.bottom() + 1.0, "{what}: {text:?} leaves the window: {rect:?} in {screen:?}");
            }
        }
    }

    #[test]
    fn at_150_percent_in_german_a_1280x720_window_clips_no_button() {
        for state in every_state() {
            let mut f = state.frames();
            fit_150(&mut f);
            assert_fits(&f, &format!("PCs, {}", state.name), false);
            // Every dialog a PC can open, at the busiest state.
            let id = f.hosts.list[0].id;
            let dialogs = [
                Dialog::HostOptions { host: id },
                Dialog::AddHost { text: String::new(), error: Some("Cannot reach the host.".into()) },
                Dialog::Otp { host: id, pin: "1234".into(), passphrase: String::new(), error: None },
                Dialog::PairingHelp,
                Dialog::Rename { host: id, text: String::new() },
                Dialog::ConfirmRemove { host: id, name: "A long name for a gaming PC".into() },
                Dialog::ConfirmUnpair { host: id, name: "A long name for a gaming PC".into() },
                Dialog::ConfirmClose { host: id, name: "A game with a rather long title".into() },
                Dialog::ConfirmQuit { host: id, start: Some((8, "Another long game title".into())), running: "A game with a rather long title".into(), resume: Some(7), named: None },
                Dialog::PairError { host: id, body: "PIN rejected. Generate a new one-time PIN in your host's web UI and check the passphrase.".into(), one_time: true },
                Dialog::Message { title: "Network test failed".into(), body: "The PC did not answer.".into() },
                Dialog::ConfirmSleep { host: id, ask: 1, name: "A long name for a gaming PC".into(), offer: None },
                Dialog::ConfirmSleep { host: id, ask: 1, name: "A long name for a gaming PC".into(), offer: Some(("t".into(), Handed::now())) },
            ];
            for dialog in dialogs {
                let name = format!("{:?}", std::mem::discriminant(&dialog));
                let options = matches!(dialog, Dialog::HostOptions { .. });
                f.l.dialog = dialog;
                f.settle();
                assert_fits(&f, &format!("dialog {name} on {}", state.name), true);
                // The options a long list hides at first are checked too.
                if options {
                    f.scroll_to_end();
                    assert!(f.drawn.iter().any(|t| t == tr(HostOption::Remove.label())), "{}: the list ends with Remove: {:?}", state.name, f.drawn);
                    assert_fits(&f, &format!("dialog {name} on {}, scrolled to the end", state.name), true);
                }
                f.l.dialog = Dialog::None;
                f.settle();
            }
        }
    }

    #[test]
    fn at_150_percent_the_longest_options_list_scrolls_to_the_option_with_focus() {
        let mut f = every_state().into_iter().find(|s| s.name == "sleep not allowed").unwrap().frames();
        fit_150(&mut f);
        f.press(Key::F2);
        assert!(f.options_open());
        let screen = Rect::from_min_size(Pos2::ZERO, f.size);
        let labels: Vec<&str> = host_options(&f.hosts.list[0]).iter().map(|o| tr(o.label())).collect();
        // Sleep is listed but cannot take focus here.
        for _ in 2..labels.len() {
            f.press(Key::ArrowDown);
            // egui animates the scroll for up to 0.3 s; the frames keep coming until it ends.
            f.settle();
            f.settle();
            let focused = f.ctx.memory(|m| m.focused()).expect("an option has focus");
            let rect = f.ctx.read_response(focused).expect("the option was laid out").rect;
            // The list's visible area is the clip its labels are drawn in.
            let clip = f
                .placed
                .iter()
                .find(|(text, at, _)| labels.contains(&text.as_str()) && rect.contains(at.center()))
                .map(|(_, _, clip)| *clip)
                .unwrap_or_else(|| panic!("the option's label was drawn: {rect:?}"));
            assert!(clip.height() < screen.height() - 100.0 && screen.expand(1.0).contains_rect(clip), "the list scrolls inside the window: {clip:?}");
            assert!(clip.expand(1.0).contains_rect(rect), "the option with focus is in the list's visible area: {rect:?} in {clip:?}");
        }
        f.press(Key::Enter);
        assert!(matches!(f.l.dialog, Dialog::ConfirmRemove { .. }), "the last option is Remove");
    }

    #[test]
    fn at_150_percent_in_german_a_40_character_pc_name_fits_its_card_options_and_details() {
        let name = "Wohnzimmer-PC mit Radeon RX 7900 XT 20GB";
        assert_eq!(name.chars().count(), 40);
        let mut record = record("uid", true, true);
        record.name = name.into();
        let mut f = Frames::with(record, Reach::Online, Some(answer(true, 7)), Vec::new());
        fit_150(&mut f);
        assert!(shows(&f, name), "{:?}", f.drawn);
        // The card ends it in "…" before its edge.
        assert_fits(&f, "PCs", false);
        let id = f.hosts.list[0].id;
        f.l.dialog = Dialog::HostOptions { host: id };
        f.settle();
        assert!(shows(&f, name), "{:?}", f.drawn);
        assert_fits(&f, "options", true);
        f.scroll_to_end();
        assert!(shows(&f, tr(HostOption::Remove.label())), "the list ends with Remove: {:?}", f.drawn);
        assert_fits(&f, "options, scrolled to the end", true);
        f.l.dialog = Dialog::None;
        f.l.open(Screen::HostDetails(id));
        f.settle();
        assert!(shows(&f, name), "{:?}", f.drawn);
        assert_fits(&f, "details page", false);
    }

    #[test]
    fn at_150_percent_in_german_every_settings_page_fits_the_width() {
        let mut f = Frames::new(record("PC", true, false), Reach::Online);
        fit_150(&mut f);
        let id = f.hosts.list[0].id;
        for screen in [Screen::Home, Screen::Settings, Screen::HostSettings(id), Screen::Profiles, Screen::Controllers, Screen::Shortcuts] {
            f.l.go(screen);
            f.settle();
            assert_fits(&f, &format!("{screen:?}"), false);
        }
        for dialog in [
            Dialog::ProfileName { id: None, name: String::new(), settings: Default::default(), error: None },
            Dialog::Shortcut { id: None, name: String::new(), chord: String::new(), error: Some("Use between 1 and 8 keys in each chord.".into()) },
            Dialog::DeleteProfile { id: 1, name: "Travel".into() },
            Dialog::DeleteShortcut { id: "a".into(), name: "Task manager".into() },
        ] {
            let name = format!("{:?}", std::mem::discriminant(&dialog));
            f.l.dialog = dialog;
            f.settle();
            assert_fits(&f, &format!("dialog {name}"), true);
            f.l.dialog = Dialog::None;
            f.settle();
        }
    }

    #[test]
    fn at_150_percent_in_german_the_roaming_rows_fit_and_mobile_data_shows_only_with_roaming() {
        let mut f = Frames::new(record("PC", true, false), Reach::Online);
        fit_150(&mut f);
        // The 1280-pixel width is what is checked; the height lets the network rows be drawn
        // without scrolling to them.
        f.size.y = 8000.0;
        let shown = |f: &Frames, text: &str| f.placed.iter().any(|(t, _, _)| t.contains(text));
        let id = f.hosts.list[0].id;
        for (screen, global) in [(Screen::Settings, true), (Screen::HostSettings(id), false)] {
            f.l.settings.client_roaming = true;
            f.l.go(screen);
            f.settle();
            assert!(shown(&f, "Netzwerkwechsel (Experimentell)"), "{screen:?}: the roaming row is drawn in German");
            assert!(shown(&f, "Über mobile Daten fortsetzen"), "{screen:?}: mobile data shows while roaming is on");
            assert_fits(&f, &format!("{screen:?} with roaming"), false);
            if global {
                f.l.settings.client_roaming = false;
                f.settle();
                assert!(shown(&f, "Netzwerkwechsel (Experimentell)"));
                assert!(!shown(&f, "Über mobile Daten fortsetzen"), "mobile data hides while roaming is off");
            }
        }
    }

    #[test]
    fn reduced_motion_turns_off_animated_transitions_and_full_motion_keeps_them() {
        use crate::config::ReduceMotion;
        let mut f = Frames::new(record("PC", true, false), Reach::Online);
        f.l.settings.reduce_motion = ReduceMotion::Reduce;
        f.settle();
        assert_eq!(f.ctx.global_style().animation_time, 0.0, "fades and slides jump");
        assert_eq!(f.ctx.global_style().scroll_animation.points_per_second, f32::INFINITY, "scrolling jumps");
        f.l.settings.reduce_motion = ReduceMotion::Full;
        f.settle();
        assert!(f.ctx.global_style().animation_time > 0.0);
        // The system setting decides only on Follow system; the tests' machine may say either.
        f.l.settings.reduce_motion = ReduceMotion::System;
        f.settle();
        assert_eq!(f.ctx.global_style().animation_time == 0.0, crate::theme::system_reduces_motion());
    }

    #[test]
    fn the_spinner_stands_still_and_asks_for_no_frames_when_motion_is_reduced() {
        for still in [true, false] {
            crate::theme::own_palette();
            let ctx = egui::Context::default();
            set_look(&ctx, false, still);
            // The first frames ask for repaints of their own (fonts, layout); look at a settled one.
            let mut delay = std::time::Duration::ZERO;
            for n in 0..6 {
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0))), time: Some(1.0 + f64::from(n) * 0.1), ..Default::default() };
                let mut output = ctx.run_ui(input, |ui| spinner(ui, 10.0));
                output.textures_delta.clear();
                delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            }
            if still {
                assert!(delay > std::time::Duration::from_secs(1), "no repaint is requested: {delay:?}");
            } else {
                assert_eq!(delay, std::time::Duration::ZERO, "the spinning wheel asks for the next frame");
            }
        }
    }

    #[test]
    fn the_high_contrast_setting_reaches_the_style() {
        let mut f = Frames::new(record("PC", true, false), Reach::Online);
        f.l.settings.high_contrast = true;
        f.settle();
        assert_eq!(f.ctx.global_style().visuals.panel_fill, Color32::BLACK);
        f.l.settings.high_contrast = false;
        f.settle();
        assert_ne!(f.ctx.global_style().visuals.panel_fill, Color32::BLACK);
    }

    #[test]
    fn ctrl_equals_minus_and_zero_step_the_zoom_except_in_a_text_field() {
        let mut f = Frames::new(record("PC", true, true), Reach::Online);
        assert_eq!(f.l.settings.ui_zoom, 100);
        f.press_with(Key::Equals, egui::Modifiers::COMMAND);
        assert_eq!(f.l.settings.ui_zoom, 115);
        f.press_with(Key::Equals, egui::Modifiers::COMMAND);
        f.press_with(Key::Equals, egui::Modifiers::COMMAND);
        f.press_with(Key::Equals, egui::Modifiers::COMMAND);
        f.press_with(Key::Equals, egui::Modifiers::COMMAND);
        assert_eq!(f.l.settings.ui_zoom, 150, "the top step is the last one");
        f.press_with(Key::Minus, egui::Modifiers::COMMAND);
        assert_eq!(f.l.settings.ui_zoom, 130);
        f.press_with(Key::Num0, egui::Modifiers::COMMAND);
        assert_eq!(f.l.settings.ui_zoom, 100);
        // The Rename dialog's name field has the keyboard.
        f.press(Key::F2);
        f.press(Key::ArrowDown);
        f.press(Key::ArrowDown);
        f.press(Key::Enter);
        assert!(matches!(f.l.dialog, Dialog::Rename { .. }));
        assert!(f.ctx.text_edit_focused(), "the name field is focused");
        f.press_with(Key::Equals, egui::Modifiers::COMMAND);
        assert_eq!(f.l.settings.ui_zoom, 100, "a text field keeps Ctrl+=");
    }

    #[test]
    fn the_options_list_follows_the_pc_state() {
        let mut host = Frames::new(record("PC", false, false), Reach::Online).hosts.list.remove(0);
        assert_eq!(
            host_options(&host),
            [HostOption::Pair, HostOption::Settings, HostOption::Rename, HostOption::Details, HostOption::PairingHelp, HostOption::CheckAgain, HostOption::Remove]
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
                HostOption::Details,
                HostOption::CheckAgain,
                HostOption::Remove
            ]
        );
        host.record.mac = None;
        assert!(!host_options(&host).contains(&HostOption::Wake), "no MAC, no Wake");
        assert!(!host_options(&host).contains(&HostOption::Unpair), "an offline PC cannot be asked to unpair");
        host.reach = Reach::Online;
        assert!(host_options(&host).contains(&HostOption::Unpair));
        assert!(!host_options(&host).contains(&HostOption::PairingHelp), "the help is for a PC that is not paired");
    }

    #[test]
    fn waking_shows_the_pc_and_cancel_stops_it() {
        let mut f = Frames::new(record("Den", true, true), Reach::Offline);
        f.waking = Some("Den".into());
        f.settle();
        assert!(f.drawn.iter().any(|t| t == "Waking Den…"), "{:?}", f.drawn);
        assert!(f.press(Key::Escape).is_empty(), "Back does not leave the PC half woken");
        assert!(matches!(f.press(Key::Enter)[..], [Command::CancelWake]), "Cancel has focus");
        f.l.settings.language = Language::German;
        f.settle();
        assert!(f.drawn.iter().any(|t| t == "Den wird aufgeweckt…"), "{:?}", f.drawn);
    }

    #[test]
    fn wake_is_offered_until_the_pc_answers_and_resume_while_it_runs_a_game() {
        let states = every_state();
        let by = |name: &str| states.iter().find(|s| s.name == name).unwrap().frames().hosts.list.remove(0);
        let asleep = host_options(&by("not reached yet, can wake"));
        assert!(asleep.contains(&HostOption::Wake), "a PC not reached yet with a MAC");
        let mut no_mac = by("not reached yet, can wake");
        no_mac.record.mac = None;
        assert!(!host_options(&no_mac).contains(&HostOption::Wake));
        assert!(!host_options(&by("paired")).contains(&HostOption::Wake), "a PC that answers is awake");

        let busy = host_options(&by("running a game"));
        let at = |option| busy.iter().position(|o| *o == option).unwrap();
        assert!(at(HostOption::OpenLibrary) < at(HostOption::Resume) && at(HostOption::Resume) < at(HostOption::QuitApp));
        assert!(!host_options(&by("paired")).contains(&HostOption::Resume));
        // Without its library the running game cannot be named, so there is nothing to resume.
        let mut unlisted = by("running a game");
        unlisted.apps = Apps::NotLoaded;
        let options = host_options(&unlisted);
        assert!(!options.contains(&HostOption::Resume) && options.contains(&HostOption::QuitApp));
    }

    #[test]
    fn f2_opens_the_options_for_the_focused_pc() {
        for State { name: state, .. } in every_state() {
            let mut f = every_state().into_iter().find(|s| s.name == state).unwrap().frames();
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
        let mut seen: Vec<HostOption> = Vec::new();
        for scenario in every_state() {
            let state = scenario.name;
            let frames = scenario.frames();
            let host = &frames.hosts.list[0];
            let options = host_options(host);
            seen.extend(&options);
            // Sleep PC listed for a device the PC does not allow cannot take focus.
            let reachable = options.iter().filter(|o| **o != HostOption::Sleep || host.sleep_option() == Some(Ok(())));
            for (n, option) in reachable.enumerate() {
                let mut f = scenario.frames();
                f.press(Key::F2);
                assert!(f.options_open(), "{state}");
                for _ in 0..n {
                    f.press(Key::ArrowDown);
                }
                let commands = f.press(Key::Enter);
                let said = |what: fn(&Command) -> bool| commands.iter().any(what);
                let ok = match option {
                    HostOption::OpenLibrary => said(|c| matches!(c, Command::OpenApps(_))),
                    HostOption::Resume => said(|c| matches!(c, Command::Launch { app: 7, resume: true, .. })),
                    HostOption::PairingHelp => matches!(f.l.dialog, Dialog::PairingHelp),
                    HostOption::Pair => said(|c| matches!(c, Command::Pair(_))),
                    HostOption::Wake => said(|c| matches!(c, Command::Wake(_))),
                    HostOption::Sleep => {
                        said(|c| matches!(c, Command::SleepOffer { .. })) && matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. })
                    }
                    HostOption::Settings => said(|c| matches!(c, Command::HostSettings(_))),
                    HostOption::Details => matches!(f.l.screen, Screen::HostDetails(_)) && f.no_dialog(),
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
        use HostOption::*;
        for option in [OpenLibrary, Resume, QuitApp, Pair, PairOtp, Wake, Sleep, Settings, Details, Rename, Unpair, PairingHelp, CheckAgain, Remove] {
            assert!(seen.contains(&option), "no state reaches {}", option.label());
        }
    }

    // ---- A PC's details page. ----

    use crate::host_page::tests::{info as host_answer, FakeProbe};
    use crate::host_page::CheckState;
    use std::sync::atomic::Ordering;

    const RUBYLIGHT: &str = "<RustHostVersion>3.0.0</RustHostVersion>";

    /// A paired PC's details page; `state` is its serverinfo state, `extra` more of its fields.
    fn details(reach: Reach, state: &str, extra: &str) -> Frames {
        let mut f = Frames::with(record("uid", true, false), reach, Some(host_answer(state, extra)), Vec::new());
        let id = f.hosts.list[0].id;
        f.l.open(Screen::HostDetails(id));
        f.settle();
        f
    }

    fn shows(f: &Frames, text: &str) -> bool {
        f.drawn.iter().any(|t| t == text)
    }

    /// Frames until the check is no longer running, as the app's waker would bring them.
    fn finish_check(f: &mut Frames) {
        for _ in 0..500 {
            f.frame(Vec::new());
            if !f.l.remote.running() {
                f.settle();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the check did not finish");
    }

    fn relay_check() -> Result<nightjar_core::http::RemoteCheck, nightjar_core::http::Error> {
        use nightjar_core::http::{ConnectionTestStats, NetworkPath, RemoteCheck, RemoteOutcome};
        let mut stats = ConnectionTestStats::default();
        [10.0, 14.0, f64::NAN, 12.0, 12.0].into_iter().for_each(|ms| stats.add(ms));
        let server_info = Box::new(host_answer("SUNSHINE_SERVER_FREE", RUBYLIGHT));
        Ok(RemoteCheck { outcome: RemoteOutcome::Verified { server_info, path: Ok(NetworkPath::Relay) }, stats })
    }

    #[test]
    fn the_details_page_shows_what_the_pc_offers_not_its_brand() {
        let f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
        assert!(shows(&f, "Rubylight 3.0.0") && shows(&f, "Open in browser") && shows(&f, "https://192.0.2.7:47990"), "{:?}", f.drawn);
        assert!(shows(&f, "Test this PC"));
        let f = details(Reach::Online, "SUNSHINE_SERVER_FREE", "<Permission>1</Permission>");
        assert!(shows(&f, "Apollo-compatible host") && shows(&f, "Open in browser"), "a known host has a web UI: {:?}", f.drawn);
        // A stock host: the generic details and the check, no web UI.
        let f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
        assert!(shows(&f, "NVIDIA GameStream") && shows(&f, "Test this PC"), "{:?}", f.drawn);
        assert!(!shows(&f, "Open in browser"));
        let f = details(Reach::Online, "SUNSHINE_SERVER_FREE", "");
        assert!(shows(&f, "Moonlight-compatible host") && !shows(&f, "Open in browser"), "an unknown host's web UI is not guessed");
        let f = details(Reach::Offline, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
        assert!(!shows(&f, "Open in browser"), "an offline PC's web UI cannot be reached");
        // A saved address no browser opens: no button, and the page says why.
        let mut record = record("uid", true, false);
        record.address = "fe80::1%12".into();
        let mut f = Frames::with(record, Reach::Online, Some(host_answer("SUNSHINE_SERVER_FREE", RUBYLIGHT)), Vec::new());
        f.l.open(Screen::HostDetails(f.hosts.list[0].id));
        f.settle();
        assert!(!shows(&f, "Open in browser") && f.drawn.iter().any(|t| t.contains("zone ID (fe80::1%12)")), "{:?}", f.drawn);
        let commands = f.press(Key::Enter);
        assert!(commands.iter().any(|c| matches!(c, Command::RemoteCheck(_))), "the check is the first item then");
    }

    /// Whether `text` is drawn on the widget with focus.
    fn focus_shows(f: &Frames, text: &str) -> bool {
        let Some(rect) = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect) else {
            return false;
        };
        f.placed.iter().any(|(t, r, _)| t == text && rect.contains(r.center()))
    }

    #[test]
    fn focus_on_the_web_ui_moves_to_the_check_when_the_pc_goes_away() {
        let mut f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
        let id = f.hosts.list[0].id;
        assert!(focus_shows(&f, "Open in browser"), "the web UI is the first item");
        f.hosts.list[0].reach = Reach::Offline;
        f.settle();
        assert!(!shows(&f, "Open in browser"));
        assert!(focus_shows(&f, "Test this PC"), "the check has focus, not nothing");
        assert!(f.press(Key::Enter).iter().any(|c| matches!(c, Command::RemoteCheck(h) if *h == id)));

        // Start, then Back: the page comes back without the item it remembers.
        let mut f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
        f.l.open_settings();
        f.settle();
        assert_eq!(f.l.screen, Screen::Settings);
        f.hosts.list[0].reach = Reach::Unknown;
        f.frame(crate::gui::tap(Key::Escape).to_vec());
        assert_eq!(f.l.screen, Screen::HostDetails(id));
        assert!(focus_shows(&f, "Test this PC"), "the check takes focus on the frame Back lands, not one later");
        f.settle();
        assert!(focus_shows(&f, "Test this PC"));
    }

    #[test]
    fn view_details_opens_the_page_and_back_returns_to_the_card() {
        let mut f = Frames::with(record("uid", true, false), Reach::Online, Some(host_answer("SUNSHINE_SERVER_FREE", RUBYLIGHT)), Vec::new());
        let card = f.ctx.memory(|m| m.focused());
        f.press(Key::F2);
        let at = host_options(&f.hosts.list[0]).iter().position(|o| *o == HostOption::Details).unwrap();
        for _ in 0..at {
            f.press(Key::ArrowDown);
        }
        f.press(Key::Enter);
        assert_eq!(f.l.screen, Screen::HostDetails(f.hosts.list[0].id));
        // The web UI is the first item; Enter opens it.
        let commands = f.press(Key::Enter);
        assert!(commands.iter().any(|c| matches!(c, Command::OpenWebUi(url) if url == "https://192.0.2.7:47990")));
        f.press(Key::Escape);
        assert_eq!(f.l.screen, Screen::Hosts);
        assert_eq!(f.ctx.memory(|m| m.focused()), card, "the card that opened it has focus again");
    }

    #[test]
    fn the_remote_check_runs_shows_its_measurements_and_errors_verbatim() {
        let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
        let id = f.hosts.list[0].id;
        // Without a web UI the check is the first item.
        let commands = f.press(Key::Enter);
        assert!(commands.iter().any(|c| matches!(c, Command::RemoteCheck(h) if *h == id)), "Test asks the app to start it");
        let probe = FakeProbe::answering(relay_check());
        f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
        f.settle();
        assert!(shows(&f, "Checking your paired PC…") && shows(&f, "Cancel"), "{:?}", f.drawn);
        probe.release();
        finish_check(&mut f);
        assert!(f.drawn.iter().any(|t| t.starts_with("The paired PC responded on this network.")), "{:?}", f.drawn);
        for text in ["Relay (DERP)", "12.0 ms", "2.0 ms", "20 %", "Test this PC"] {
            assert!(shows(&f, text), "{text}: {:?}", f.drawn);
        }

        // A failure says what the client said, word for word.
        let refused = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "No connection could be made (os error 10061)");
        let probe = FakeProbe::answering(Ok(nightjar_core::http::RemoteCheck {
            outcome: nightjar_core::http::RemoteOutcome::Unreachable(nightjar_core::http::Error::Io(refused)),
            stats: Default::default(),
        }));
        f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
        probe.release();
        finish_check(&mut f);
        assert!(shows(&f, "No connection could be made (os error 10061)"), "{:?}", f.drawn);
        assert!(f.drawn.iter().any(|t| t.starts_with("The paired PC could not be verified.")));
        assert!(!shows(&f, "Relay (DERP)"), "the last check's route is gone");
    }

    #[test]
    fn cancel_and_leaving_the_page_stop_the_check() {
        let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
        let id = f.hosts.list[0].id;
        let probe = FakeProbe::answering(relay_check());
        f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
        f.settle();
        // The button with focus is Cancel now.
        let commands = f.press(Key::Enter);
        assert!(!commands.iter().any(|c| matches!(c, Command::RemoteCheck(_))));
        assert_eq!(probe.cancels.load(Ordering::SeqCst), 1, "the requests in flight are cancelled");
        assert!(shows(&f, "Test cancelled. No host or network settings were changed."), "{:?}", f.drawn);
        assert!(!f.l.remote.running());

        let probe = FakeProbe::answering(relay_check());
        f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
        f.settle();
        f.press(Key::Escape);
        assert_eq!(f.l.screen, Screen::Hosts);
        assert_eq!(probe.cancels.load(Ordering::SeqCst), 1, "leaving the page cancels it");
        assert!(!f.l.remote.running() && f.l.remote.state.is_none());
        f.l.open(Screen::HostDetails(id));
        f.settle();
        assert!(!shows(&f, "Test cancelled. No host or network settings were changed."), "the page opens fresh");
    }

    #[test]
    fn settings_opened_over_the_page_keep_its_check() {
        let mut f = details(Reach::Online, "MJOLNIR_STATE_FREE", "");
        let id = f.hosts.list[0].id;
        let probe = FakeProbe::answering(relay_check());
        f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
        probe.release();
        finish_check(&mut f);
        assert!(shows(&f, "Relay (DERP)"), "{:?}", f.drawn);
        // Start, then Back: the result is still there.
        f.l.open_settings();
        f.settle();
        assert_eq!(f.l.screen, Screen::Settings);
        f.press(Key::Escape);
        assert_eq!(f.l.screen, Screen::HostDetails(id));
        assert!(shows(&f, "Relay (DERP)"), "the result survives Settings: {:?}", f.drawn);

        // A running check goes on meanwhile and reports on the page.
        let probe = FakeProbe::answering(relay_check());
        f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
        f.settle();
        f.l.open_settings();
        f.settle();
        assert!(f.l.remote.running() && probe.cancels.load(Ordering::SeqCst) == 0, "Settings do not cancel it");
        f.press(Key::Escape);
        assert!(shows(&f, "Checking your paired PC…"), "{:?}", f.drawn);
        probe.release();
        finish_check(&mut f);
        assert!(shows(&f, "Relay (DERP)"), "{:?}", f.drawn);

        // Settings that replace the page (PCs from there) leave it: the check is cancelled.
        let probe = FakeProbe::answering(relay_check());
        f.l.remote.start(id, probe.clone(), "uid".into(), Arc::new(|| {}));
        f.l.open_settings();
        f.settle();
        f.l.switch(Screen::Hosts);
        f.settle();
        assert_eq!(probe.cancels.load(Ordering::SeqCst), 1);
        assert!(!f.l.remote.running() && f.l.remote.state.is_none());
    }

    #[test]
    fn at_150_percent_in_german_the_details_page_fits_the_width() {
        let mut f = details(Reach::Online, "SUNSHINE_SERVER_FREE", RUBYLIGHT);
        fit_150(&mut f);
        let id = f.hosts.list[0].id;
        let mut failed = crate::host_page::report(Err(nightjar_core::http::Error::NotPaired));
        failed.error = Some("A rather long error the host gave, word for word, about why it did not answer".into());
        for state in [
            None,
            Some(CheckState::Running),
            Some(CheckState::Cancelled),
            Some(CheckState::Done(crate::host_page::report(relay_check()))),
            Some(CheckState::Done(failed)),
        ] {
            let name = format!("{state:?}");
            f.l.remote.state = state.map(|s| (id, s));
            f.settle();
            assert!(shows(&f, "Im Browser öffnen"), "German is in use: {:?}", f.drawn);
            assert_fits(&f, &format!("details page, {name}"), false);
        }
    }

    #[test]
    fn sleep_pc_waits_for_the_offer_and_spends_it_only_when_confirmed() {
        let mut f = every_state().into_iter().find(|s| s.name == "can sleep").unwrap().frames();
        let id = f.hosts.list[0].id;
        let at = Handed::now();
        let offer = || Ok(SleepStep::Offer { token: "tok".into(), at });
        let sleeps = |commands: &[Command]| commands.iter().any(|c| matches!(c, Command::Sleep { .. }));
        let waiting = || Dialog::ConfirmSleep { host: id, ask: 1, name: "PC".into(), offer: None };

        // Cancelled while the PC is asked: its offer, or its failure, says nothing afterwards.
        f.l.dialog = waiting();
        f.l.focus_dialog = true;
        f.settle();
        assert!(!sleeps(&f.press(Key::Enter)), "Cancel has focus");
        assert!(f.no_dialog());
        f.l.sleep_answered(id, 1, "Gaming PC", false, offer());
        f.l.sleep_answered(id, 1, "Gaming PC", false, Err("The PC did not answer.".into()));
        assert!(f.no_dialog(), "the user walked away from it");

        // The offer fills the confirmation; one for another PC does not.
        f.l.dialog = waiting();
        f.l.focus_dialog = true;
        f.settle();
        f.l.sleep_answered(id + 1, 1, "Gaming PC", false, offer());
        assert!(matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. }));
        f.l.sleep_answered(id, 1, "Gaming PC", false, offer());
        assert!(matches!(&f.l.dialog, Dialog::ConfirmSleep { offer: Some((token, _)), .. } if token == "tok"));
        f.settle();
        assert!(!sleeps(&f.press(Key::Enter)), "Cancel still has focus once the offer is in");
        f.l.dialog = waiting();
        f.l.sleep_answered(id, 1, "Gaming PC", false, offer());
        f.settle();
        f.press(Key::ArrowRight);
        let commands = f.press(Key::Enter);
        assert!(commands.iter().any(|c| matches!(c, Command::Sleep { host, ask: 1, token, at: when } if *host == id && token == "tok" && *when == at)));
        assert!(f.no_dialog());

        // The answers to the confirmed request are shown, the host's refusal word for word.
        f.l.sleep_answered(id, 1, "Gaming PC", true, Ok(SleepStep::Accepted));
        assert!(matches!(&f.l.dialog, Dialog::Message { title, .. } if title == "Gaming PC is going to sleep"));
        f.l.dialog = Dialog::None;
        let refusal = "End all applications and sessions before requesting sleep";
        f.l.sleep_answered(id, 1, "Gaming PC", true, Err(refusal.into()));
        assert!(matches!(&f.l.dialog, Dialog::Message { title, body } if title == "Could not put Gaming PC to sleep" && body == refusal));
        // A failed offer the user still waits for is shown too.
        f.l.dialog = waiting();
        f.l.sleep_answered(id, 1, "Gaming PC", false, Err(crate::hosts::SLEEP_BUSY.into()));
        assert!(matches!(&f.l.dialog, Dialog::Message { body, .. } if body == crate::hosts::SLEEP_BUSY));
    }

    #[test]
    fn a_sleep_pc_answer_waits_for_the_dialog_the_user_is_in() {
        let mut f = every_state().into_iter().find(|s| s.name == "can sleep").unwrap().frames();
        let id = f.hosts.list[0].id;
        let other = id + 1;
        f.l.dialog = Dialog::Rename { host: other, text: "Living r".into() };
        f.settle();
        f.l.sleep_answered(id, 1, "Gaming PC", true, Ok(SleepStep::Accepted));
        f.l.sleep_answered(id, 2, "Gaming PC", true, Err("The PC did not answer.".into()));
        f.settle();
        assert!(matches!(&f.l.dialog, Dialog::Rename { text, .. } if text == "Living r"), "the rename keeps its text");
        f.l.dialog = Dialog::None;
        f.settle();
        assert!(matches!(&f.l.dialog, Dialog::Message { title, .. } if title == "Gaming PC is going to sleep"), "then it is shown");
        f.l.dialog = Dialog::None;
        f.settle();
        assert!(matches!(&f.l.dialog, Dialog::Message { title, .. } if title == "Could not put Gaming PC to sleep"), "in order");
        f.l.dialog = Dialog::None;
        f.settle();
        assert!(f.no_dialog(), "each once");
    }

    #[test]
    fn a_late_offer_from_a_cancelled_sleep_pc_is_not_taken_for_the_new_one() {
        let mut f = every_state().into_iter().find(|s| s.name == "can sleep").unwrap().frames();
        let id = f.hosts.list[0].id;
        let at = Handed::now();
        let offer = |token: &str| Ok(SleepStep::Offer { token: token.into(), at });
        let ask = |f: &mut Frames| {
            let mut commands = Vec::new();
            f.l.dialog = f.l.choose_host_option(&f.hosts.list[0], HostOption::Sleep, &mut commands).unwrap();
            match commands[..] {
                [Command::SleepOffer { host, ask }] if host == id => ask,
                _ => panic!("Sleep PC asks the PC for a token"),
            }
        };
        let first = ask(&mut f);
        f.l.dialog = Dialog::None;
        let second = ask(&mut f);
        assert_ne!(first, second);

        // The first request's token comes back after the second request replaced it on the PC.
        f.l.sleep_answered(id, first, "Gaming PC", false, offer("old"));
        assert!(matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. }), "the old token is dropped");
        f.l.sleep_answered(id, second, "Gaming PC", false, offer("new"));
        assert!(matches!(&f.l.dialog, Dialog::ConfirmSleep { offer: Some((token, _)), .. } if token == "new"));
        f.l.focus_dialog = true;
        f.settle();
        f.press(Key::ArrowRight);
        let commands = f.press(Key::Enter);
        assert!(commands.iter().any(|c| matches!(c, Command::Sleep { token, ask, .. } if token == "new" && *ask == second)));
        // A failure of the cancelled request is not shown over the new one either.
        f.l.dialog = Dialog::ConfirmSleep { host: id, ask: second, name: "PC".into(), offer: None };
        f.l.sleep_answered(id, first, "Gaming PC", false, Err("The PC did not answer.".into()));
        assert!(matches!(f.l.dialog, Dialog::ConfirmSleep { offer: None, .. }));
    }

    #[test]
    fn sleep_pc_not_allowed_is_listed_with_the_reason_and_does_nothing() {
        let mut f = every_state().into_iter().find(|s| s.name == "sleep not allowed").unwrap().frames();
        assert!(host_options(&f.hosts.list[0]).contains(&HostOption::Sleep));
        f.press(Key::F2);
        assert!(f.drawn.iter().any(|t| t == crate::hosts::SLEEP_NOT_ALLOWED), "the reason is shown");
        let mut commands = Vec::new();
        let next = f.l.choose_host_option(&f.hosts.list[0], HostOption::Sleep, &mut commands);
        assert!(next.is_none() && commands.is_empty(), "nothing is asked of the PC");
        // A stock host never lists it.
        let stock = every_state().into_iter().find(|s| s.name == "paired").unwrap().frames();
        assert!(!host_options(&stock.hosts.list[0]).contains(&HostOption::Sleep));
    }

    #[test]
    fn the_replace_prompt_starts_on_resume_else_cancel_never_on_the_button_that_quits() {
        for (resume, name) in [(Some(7), "with something to resume"), (None, "with nothing to resume")] {
            let mut f = Frames::new(record("Paired", true, false), Reach::Online);
            f.l.dialog = Dialog::ConfirmQuit {
                host: f.hosts.list[0].id,
                start: Some((9, "Zelda".into())),
                running: "Portal".into(),
                resume,
                named: Some(7),
            };
            f.l.focus_dialog = true;
            f.settle();
            let commands = f.press(Key::Enter);
            assert!(!commands.iter().any(|c| matches!(c, Command::QuitAndLaunch { .. })), "{name}: Enter must not quit");
            assert_eq!(
                commands.iter().any(|c| matches!(c, Command::Launch { app: 7, resume: true, .. })),
                resume.is_some(),
                "{name}: Enter resumes, or cancels when there is nothing to resume"
            );
            assert!(f.no_dialog(), "{name}");
        }
    }

    #[test]
    fn a_pc_that_confirms_replacement_starts_without_the_client_prompt() {
        let info = |extension: &str| {
            let xml = format!(
                r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>7</currentgame><state>SUNSHINE_SERVER_BUSY</state><Permission>1</Permission>{extension}</root>"#
            );
            nightjar_core::http::ServerInfo::parse(&xml, 47989).expect("the answer parses")
        };
        for (extension, confirms) in [("", false), ("<RustHostVersion>3.0.0</RustHostVersion>", true)] {
            let mut f = Frames::with(record("Paired", true, false), Reach::Online, Some(info(extension)), vec![game(7), game(9)]);
            assert_eq!(f.hosts.list[0].confirms_replacement(), confirms);
            let id = f.hosts.list[0].id;
            f.l.go(Screen::Apps(id));
            f.settle();
            // The running game has focus first; the next poster is the other game.
            f.press(Key::ArrowRight);
            let commands = f.press(Key::Enter);
            let launched = commands.iter().any(|c| matches!(c, Command::Launch { app: 9, resume: false, .. }));
            assert_eq!(launched, confirms, "{extension}");
            assert_eq!(matches!(f.l.dialog, Dialog::ConfirmQuit { .. }), !confirms, "{extension}");
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
    fn a_pairing_that_was_cancelled_never_shows_an_error() {
        let mut f = Frames::new(record("PC", false, false), Reach::Online);
        let id = f.hosts.list[0].id;
        let waiting = |f: &mut Frames| {
            f.l.dialog = Dialog::Pairing { host: id, pin: "1234".into() };
            f.l.focus_dialog = true;
            f.settle();
        };
        waiting(&mut f);
        let commands = f.press(Key::Escape);
        assert!(commands.iter().any(|c| matches!(c, Command::CancelPair)), "Escape cancels the request");
        assert!(!f.l.pairing_finished(id, Err(crate::hosts::PAIR_NETWORK.into())), "the late failure");
        assert!(f.no_dialog(), "is not shown");
        assert!(!f.l.pairing_finished(id, Ok(())), "and a late success does not open the library");
        assert!(f.no_dialog());
        // A pairing the user still waits for does report both.
        waiting(&mut f);
        assert!(!f.l.pairing_finished(id, Err(crate::hosts::PAIR_PIN_WRONG.into())));
        assert!(matches!(f.l.dialog, Dialog::PairError { one_time: false, .. }));
        waiting(&mut f);
        assert!(f.l.pairing_finished(id, Ok(())));
        assert!(f.no_dialog());
        // Another PC's answer is not this dialog's.
        waiting(&mut f);
        assert!(!f.l.pairing_finished(id + 1, Err(crate::hosts::PAIR_NETWORK.into())));
        assert!(matches!(f.l.dialog, Dialog::Pairing { .. }));
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
