//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

mod cards;
mod dialogs;
pub mod files;
mod library;
mod pages;
mod settings;
mod widgets;

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2,
};

use crate::config::{
    step_zoom, AudioChannels, CodecPreference, DecoderPreference, ErrorCorrection, HostDisplay, Language, MouseMode, Overrides, Profiles,
    ReduceMotion, Settings, WindowMode, UI_ZOOM_STEPS,
};
use crate::console::{self, EndAction, GameState, Playing, Source};
use crate::hosts::{Apps, Art, Handed, Host, Hosts, Reach, SleepStep};
use crate::strings::{en, tr, trf};
use crate::stream::{pyrowave_bitrate_warning, stream_mode, Display};
use crate::theme::*;
use cards::*;
use dialogs::*;
pub use library::{policy_role, running_name};
use library::*;
pub(crate) use widgets::{focus_ring, name_for_readers, primary_button, quiet_button, toggle};
use widgets::*;

/// Where Nightjar's guides live, the pages Android's help opens.
pub const GUIDES: &str = "https://github.com/RamazanKara/nightjar/blob/main/docs/";

pub enum Command {
    Launch { host: u64, app: i32, resume: bool },
    /// Input-only through a small ordinary stream, on a PC without its own Input-only mode.
    LaunchInputOnly { host: u64, app: i32, resume: bool },
    SetQuitOnLeave { host: u64, uuid: String, id: i32, enabled: bool },
    QuitApp(u64),
    ControlAction { host: u64, app: i32 },
    /// `named` is the app the prompt said would be closed; it is checked again before closing.
    QuitAndLaunch { host: u64, app: i32, named: Option<i32> },
    /// Start the app the PC refused again, closing the one it runs: the answer to its prompt.
    ConfirmReplace,
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
    /// Always connect to the PC through this known address; None brings back the automatic pick.
    PinAddress { host: u64, address: Option<String> },
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
    /// Open one of Nightjar's guides (a page under [`GUIDES`]) in the default browser.
    OpenGuide(&'static str),
    /// Open the details page's browser of the folders a PC shares with this device.
    SharedFolders(u64),
    /// Choose a folder here and download the file `name` of the listed directory into it.
    FolderDownload { host: u64, name: String, size: u64 },
    /// Choose a file here and upload it into the listed directory.
    FolderUpload(u64),
    /// Open the details page's list of a PC's Nucleus Co-op sessions.
    Coop(u64),
    CancelPair,
    /// An address, or a QR pairing link to add and pair with (see `hosts::parse_entry`).
    AddHost(String),
    /// Add PC from a pairing link on the clipboard.
    AddFromClipboard,
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
    /// Open a PC's Files page.
    OpenFiles(u64),
    /// Show the system's file dialog for files to send from the open Files page.
    ChooseFiles,
    /// The PIN screen let a start through (or unlocked the app): the start that waited goes on.
    PinPassed,
    /// The PIN screen for a stream start was cancelled: the start that waited is dropped.
    PinCancelled,
    /// Close Nightjar (the app lock's way out).
    Quit,
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
    /// A PC's transfer folder: send, list, download (Experimental).
    Files(u64),
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
    /// The PC refused to start `app` until the user confirms closing `running`. Closing the
    /// dialog any other way than confirming cancels the launch.
    ConfirmReplace { app: String, running: String },
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
    /// The settings sections showing their folded rows; none when the app starts.
    unfolded: std::collections::BTreeSet<&'static str>,
    /// The settings page where Custom… was chosen for the resolution and for the frame rate: its
    /// row shows there even at a preset, until a preset is chosen.
    custom_size: Option<Screen>,
    custom_fps: Option<Screen>,
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
    /// The details page's shared folder browser.
    pub folders: crate::folders_page::SharedFolders,
    /// The details page's list of Nucleus Co-op sessions.
    pub coop: crate::coop_page::CoopSessions,
    /// The Files page and its transfers.
    pub files: files::Files,
    /// A short notice over the launcher, such as the game quitting after leaving its stream.
    toast: Option<(String, std::time::Instant)>,
    /// The PIN lock; None in tests that do not look at it.
    pub pin: Option<crate::pin_ui::PinLock>,
}

/// What the launcher shows, borrowed from the app each frame.
pub struct View<'a> {
    pub hosts: &'a Hosts,
    pub pads: &'a crate::gamepad::Gamepads,
    pub connecting: Option<ConnectingView>,
    /// The name of the PC being woken.
    pub waking: Option<String>,
}

const CONTENT_MAX: f32 = 1180.0;
/// The column of settings rows: settings, a PC's details, the profile and shortcut lists.
const SETTINGS_W: f32 = 760.0;

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
            unfolded: Default::default(),
            custom_size: None,
            custom_fps: None,
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
            folders: Default::default(),
            coop: Default::default(),
            files: Default::default(),
            toast: None,
            pin: None,
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
            Screen::Apps(_) | Screen::Game { .. } | Screen::HostDetails(_) | Screen::Files(_) => self.open(Screen::Settings),
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
        // Arrow keys and the D-pad move the focus at the end of a pass, where egui never reports
        // `gained_focus`: the move shows here as a new focused key, and the item scrolls into view
        // on that too (Tab moves report both; scrolling twice is one scroll).
        if response.has_focus() && self.focused_key.as_deref() != Some(key) {
            self.focused_key = Some(key.to_owned());
            response.scroll_to_me(None);
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

    /// Opens `dialog` with focus on its first control.
    pub fn ask(&mut self, dialog: Dialog) {
        self.dialog = dialog;
        self.focus_dialog = true;
    }

    pub fn message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.dialog = Dialog::Message {
            title: title.into(),
            body: body.into(),
        };
        self.focus_dialog = true;
    }

    /// Shows `text` over the launcher for a few seconds, like Android's toasts.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), std::time::Instant::now()));
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        const SHOWN: std::time::Duration = std::time::Duration::from_secs(4);
        let Some((notice, at)) = &self.toast else { return };
        let Some(left) = SHOWN.checked_sub(at.elapsed()) else {
            self.toast = None;
            ctx.request_repaint();
            return;
        };
        egui::Area::new(Id::unique("launcher-toast"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .interactable(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(0x28, 0x2A, 0x36, 235))
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(Margin::symmetric(18, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new(notice).font(regular(text::LABEL)).color(ON_SURFACE));
                    });
            });
        ctx.request_repaint_after(left);
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
                Screen::Apps(_) | Screen::Game { .. } | Screen::HostDetails(_) | Screen::Files(_) => {
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
        self.folders.poll(page);
        // A co-op join that went through streams its slot: the session's entry, launched as any.
        if let Some((host, app)) = self.coop.poll(page) {
            commands.push(Command::Launch { host, app, resume: false });
        }
        // Likewise a Files page that was left cancels its transfers.
        let files = std::iter::once(&self.screen).chain(self.trail.iter().map(|(screen, _)| screen)).find_map(|screen| match screen {
            Screen::Files(id) => Some(*id),
            _ => None,
        });
        self.files.poll(files);
        // The PIN screen and the PIN lock page cover the launcher while they are up. The polls
        // above run under them, so a check, transfer or join in flight still ends or is cancelled.
        if let Some(pin) = &mut self.pin {
            let pcs: Vec<(String, String)> = view.hosts.list.iter().map(|h| (h.record.unique_id.clone(), h.display_name().to_owned())).collect();
            if pin.ui(ui, &pcs, &mut commands) {
                return commands;
            }
        }
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
                Screen::Files(id) => self.files_page(ui, &view, id, &mut commands),
                Screen::Settings |Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.settings_page(ui, &view, &mut commands),
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
        self.draw_toast(&ctx);
        commands
    }

    fn header(&mut self, ui: &mut Ui, logo: egui::TextureId, view: &View<'_>, commands: &mut Vec<Command>) {
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            match self.screen {
                Screen::Home => {
                    egui::Image::from_texture((logo, vec2(30.0, 30.0))).ui(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new("Nightjar").font(semibold(text::SECTION)).color(PAPER));
                }
                // The page itself is headed by the game's title, beside its cover.
                Screen::Game { .. } => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
                    }
                }
                Screen::Hosts => {
                    egui::Image::from_texture((logo, vec2(30.0, 30.0))).ui(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new("Nightjar").font(semibold(text::SECTION)).color(PAPER));
                }
                Screen::Apps(id) => {
                    if text_button(ui, "‹  PCs").clicked() {
                        self.back(commands);
                    }
                    let name = view.hosts.get(id).map_or("", |h| h.display_name()).to_owned();
                    ui.label(RichText::new(name).font(semibold(text::SECTION)).color(PAPER));
                }
                Screen::HostDetails(id) | Screen::Files(id) => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
                    }
                    let name = view.hosts.get(id).map_or("", |h| h.display_name()).to_owned();
                    ui.label(RichText::new(name).font(semibold(text::SECTION)).color(PAPER));
                }
                // A top-level page, as Home and PCs: the top bar leads away, Escape or B still goes back.
                Screen::Settings => {
                    ui.label(RichText::new(tr("Settings")).font(semibold(text::SECTION)).color(PAPER));
                }
                Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => {
                    if text_button(ui, "‹  Back").clicked() {
                        self.back(commands);
                    }
                    ui.label(RichText::new(&self.edit_name).font(semibold(text::SECTION)).color(PAPER));
                }
                Screen::Profiles => {
                    if text_button(ui, "‹  Settings").clicked() {
                        self.back(commands);
                    }
                    ui.label(RichText::new(tr("Profiles")).font(semibold(text::SECTION)).color(PAPER));
                }
                Screen::Controllers => {
                    if text_button(ui, "‹  Settings").clicked() { self.back(commands); }
                    ui.label(RichText::new(tr("Controller mappings")).font(semibold(text::SECTION)).color(PAPER));
                }
                Screen::Shortcuts => {
                    if text_button(ui, "‹  Settings").clicked() { self.back(commands); }
                    ui.label(RichText::new(tr("Keyboard shortcuts")).font(semibold(text::SECTION)).color(PAPER));
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
                            if self.screen == Screen::Hosts && header_button(ui, "Add PC").clicked() {
                                self.add_host_dialog();
                            }
                        }
                        Screen::Game { .. } => {}
                        Screen::Apps(id) => {
                            if let Some(host) = view.hosts.get(id) {
                                if header_button(ui, "Refresh").clicked() {
                                    commands.push(Command::Refresh(id));
                                }
                                if host.running_app().is_some() && header_button(ui, "Quit running app").clicked() {
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
                            if header_button(ui, "PC settings").clicked() {
                                commands.push(Command::HostSettings(id));
                            }
                        }
                        Screen::HostDetails(_) | Screen::Files(_) | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::Profiles | Screen::ProfileSettings(_) | Screen::Controllers | Screen::Shortcuts => {}
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
        // The same size as the header's other actions (`text_button`).
        let label = RichText::new(tr(name))
            .font(semibold(text::LABEL))
            .color(if current { PAPER } else { ON_SURFACE_VARIANT });
        let response = ui.add(egui::Button::new(if current { label.underline() } else { label }).frame(false).min_size(vec2(0.0, 36.0)));
        focus_ring(ui, response.rect, &response, radius::CONTROL);
        if response.clicked() && !current {
            if let Some(save) = self.screen.save_command() {
                commands.push(save);
            }
            self.switch(screen);
        }
    }
}

#[cfg(test)]
use settings::*;
#[cfg(test)]
mod tests;
