//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

mod cards;
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
fn dialog_buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(space::L);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        add(ui);
    });
}

/// A dialog's one-line text field: full width, with room around its text.
fn dialog_field(ui: &mut Ui, edit: egui::TextEdit<'_>) -> Response {
    ui.add(edit.desired_width(f32::INFINITY).margin(Margin::symmetric(12, 10)))
}

/// Lets a dialog grow to the window's height less a margin and its frame, and no further. egui
/// lays a new area out in at most 600x400 points and from then on in the size it took, so a list
/// in a dialog never got more than that and its last row was cut in any window.
fn dialog_takes_the_window(ui: &mut Ui) {
    const FRAME: f32 = 28.0;
    ui.set_max_height(ui.ctx().content_rect().height() - 2.0 * (space::L + FRAME));
}

#[cfg(test)]
use settings::*;
#[cfg(test)]
mod tests;
