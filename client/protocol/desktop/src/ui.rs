//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

pub mod files;
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

/// The settings sections in the order Android lists them (App only on the main settings).
const SETTINGS_SECTIONS: [&str; 7] = [en("Video"), en("Display & image"), en("Controls"), en("Audio"), en("Streaming"), en("App"), en("Advanced")];

/// The Resolution menu after Native: Android's 16:9 sizes with common PC and laptop ones, then
/// 16:10, then ultrawide. Custom… takes any other size.
const RESOLUTIONS: [(u32, u32); 15] = [
    (640, 360), (854, 480), (1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160),
    (1280, 800), (1920, 1200), (2560, 1600), (3840, 2400),
    (2560, 1080), (3440, 1440), (5120, 1440),
];

/// The Frame rate menu after Display: Android's 30-120 and common monitor rates. Custom… takes
/// any rate in `CUSTOM_FPS`, as the stream menu's rate does.
const FRAME_RATES: [u32; 7] = [30, 60, 90, 120, 144, 165, 240];
const CUSTOM_FPS: std::ops::RangeInclusive<u32> = 10..=500;

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

const CARD_W: f32 = 300.0;
const CARD_H: f32 = 112.0;
/// Home's cards: a game to continue, or a PC with one line of state.
const COMPACT_CARD_H: f32 = 84.0;
/// The card's inner margin left and right of its status and detail lines.
const CARD_PAD: f32 = 20.0;
const POSTER_W: f32 = 168.0;
const POSTER_H: f32 = 224.0;
/// Room under a cover for its title, which shows while it has focus.
const POSTER_CAPTION: f32 = 28.0;
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

    fn add_host_dialog(&mut self) {
        self.dialog = Dialog::AddHost {
            text: String::new(),
            error: None,
        };
        self.focus_dialog = true;
    }

    fn hosts(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
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

    fn home(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>, logo: egui::TextureId) {
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
    fn leave_and_input_only(&mut self, ui: &mut Ui, host: &Host, app: &nightjar_core::http::App, can_play: bool, running: bool, commands: &mut Vec<Command>) {
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

    fn controllers_page(&mut self, ui: &mut Ui, pads: &crate::gamepad::Gamepads, commands: &mut Vec<Command>) {
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
    fn connection_address(&mut self, ui: &mut Ui, host: &Host, commands: &mut Vec<Command>) {
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
    fn coop_sessions(&mut self, ui: &mut Ui, id: u64, commands: &mut Vec<Command>) {
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
    fn shared_folders(&mut self, ui: &mut Ui, id: u64, commands: &mut Vec<Command>) {
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
            let about = trf("Active: {}. PCs and games can override individual settings.", &[&active]);
            let manage = ui.scope(|ui| {
                ui.set_max_width(SETTINGS_W);
                row(ui, "Profiles", &about, |ui| quiet_button(ui, "Manage profiles"))
            }).inner;
            self.focus_item(&manage, "profiles-link", false);
            if manage.clicked() {
                commands.push(Command::SaveSettings);
                self.open(Screen::Profiles);
                return;
            }
        }
        let mut jump: Option<&'static str> = None;
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(SETTINGS_W);
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
        let mut rows = Rows { layer: layered.then_some((&inherited, overrides)), unfolded: &mut self.unfolded, section: "", folded: 0 };
        let screen = self.screen;
        let (custom_size_on, custom_fps_on) = (&mut self.custom_size, &mut self.custom_fps);
        let s = if layered { &mut self.edit_settings } else { &mut self.settings };
        let before = s.clone();
        // Coming back from a sub-page, the link that opened it takes focus instead.
        let focus_first = self.focus_screen && self.restore_key.is_none();
        if focus_first {
            self.focus_screen = false;
        }
        scroll_column(ui, "settings", SETTINGS_W, |ui| {
            ui.set_max_width(SETTINGS_W);
            rows.section(ui, "Video", &mut jump);
            let native = trf("Native ({}x{})", &[&display.width, &display.height]);
            let custom_size = s.width != 0 && (*custom_size_on == Some(screen) || !RESOLUTIONS.contains(&(s.width, s.height)));
            let current = if s.width == 0 {
                native.clone()
            } else if custom_size {
                trf("Custom ({}x{})", &[&s.width, &s.height])
            } else {
                format!("{}x{}", s.width, s.height)
            };
            let mut chose = None;
            let first = setting_row(ui, s, &mut rows, &["width", "height"], "Resolution", "The stream's size. Native matches this display.", |ui, s| {
                // Taller than egui's default menu; it scrolls where the window is shorter.
                egui::ComboBox::from_id_salt("resolution")
                    .selected_text(current)
                    .height(640.0)
                    .show_ui(ui, |ui| {
                        let mut v = (s.width, s.height);
                        if ui.selectable_value(&mut v, (0, 0), native).clicked() {
                            ((s.width, s.height), chose) = (v, Some(false));
                        }
                        for (w, h) in RESOLUTIONS {
                            let mut v = (s.width, s.height);
                            if ui.selectable_value(&mut v, (w, h), format!("{w}x{h}")).clicked() {
                                ((s.width, s.height), chose) = (v, Some(false));
                            }
                        }
                        if ui.selectable_label(custom_size, tr("Custom…")).clicked() {
                            if s.width == 0 { (s.width, s.height) = (display.width, display.height); }
                            chose = Some(true);
                        }
                    })
                    .response
            });
            if let Some(custom) = chose { *custom_size_on = custom.then_some(screen); }
            if let Some(first) = first.filter(|_| focus_first) {
                first.request_focus();
            }
            // Read again after the menu: a row drawn at Native or Display would clamp it to its range.
            // Stepping onto a preset keeps the row.
            if s.width != 0 && (*custom_size_on == Some(screen) || !RESOLUTIONS.contains(&(s.width, s.height))) {
                let before = (s.width, s.height);
                setting_row(ui, s, &mut rows, &["width", "height"], "Custom resolution", "Width and height in pixels.", |ui, s| {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        step_value(ui, &mut s.width, 320..=7680, 16.0, |d| d);
                        ui.label("×");
                        step_value(ui, &mut s.height, 240..=4320, 16.0, |d| d);
                    }).response
                });
                if (s.width, s.height) != before { *custom_size_on = Some(screen); }
            }
            let display_fps = display.refresh_hz.round() as u32;
            let custom_fps = s.fps != 0 && (*custom_fps_on == Some(screen) || !FRAME_RATES.contains(&s.fps));
            let fps_text = if s.fps == 0 {
                trf("Display ({} Hz)", &[&display_fps])
            } else if custom_fps {
                trf("Custom ({} fps)", &[&s.fps])
            } else {
                trf("{} fps", &[&s.fps])
            };
            let mut chose = None;
            setting_row(ui, s, &mut rows, &["fps"], "Frame rate", "Display matches this monitor's refresh rate.", |ui, s| {
                egui::ComboBox::from_id_salt("fps")
                    .selected_text(fps_text)
                    .height(640.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut s.fps, 0, trf("Display ({} Hz)", &[&display_fps])).clicked() { chose = Some(false); }
                        for fps in FRAME_RATES {
                            if ui.selectable_value(&mut s.fps, fps, trf("{} fps", &[&fps])).clicked() { chose = Some(false); }
                        }
                        if ui.selectable_label(custom_fps, tr("Custom…")).clicked() {
                            if s.fps == 0 { s.fps = display_fps.clamp(*CUSTOM_FPS.start(), *CUSTOM_FPS.end()); }
                            chose = Some(true);
                        }
                    })
                    .response
            });
            if let Some(custom) = chose { *custom_fps_on = custom.then_some(screen); }
            if s.fps != 0 && (*custom_fps_on == Some(screen) || !FRAME_RATES.contains(&s.fps)) {
                let before = s.fps;
                setting_row(ui, s, &mut rows, &["fps"], "Custom frame rate", "Frames per second.", |ui, s| {
                    step_value(ui, &mut s.fps, CUSTOM_FPS, 1.0, |d| d.suffix(" fps"))
                });
                if s.fps != before { *custom_fps_on = Some(screen); }
            }
            let (w, h, fps, bitrate) = stream_mode(s, display);
            let recommended = stream_mode(&Settings { bitrate_kbps: 0, ..s.clone() }, display).3;
            let mut automatic = s.bitrate_kbps == 0;
            setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Automatic bitrate",
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
                // A chosen bitrate below PyroWave's minimum stays; the row says what it costs.
                let warning = pyrowave_bitrate_warning(s, w, h, fps);
                let note = match &warning {
                    Some(warning) => warning.as_str(),
                    None if s.codec.is_pyrowave() => en("PyroWave needs a high bitrate: below its minimum the picture breaks up."),
                    None => en("Higher is sharper and needs a faster network."),
                };
                setting_row(ui, s, &mut rows, &["bitrate_kbps"], "Bitrate",
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
            setting_row(ui, s, &mut rows, &["adaptive_bitrate"], "Device controls bitrate", "Reduce bitrate during sustained congestion and recover gradually. Requires a compatible host.", |ui, s| {
                toggle(ui, &mut s.adaptive_bitrate)
            });
            setting_row(ui, s, &mut rows, &["adaptive_quality"], "Automatic quality", "Adjust resolution and frame rate under sustained load. Requires a host that supports live stream changes.", |ui, s| {
                egui::ComboBox::from_id_salt("quality").selected_text(tr(s.adaptive_quality.label())).show_ui(ui, |ui| {
                    for mode in [crate::config::QualityMode::Off, crate::config::QualityMode::Motion, crate::config::QualityMode::Detail, crate::config::QualityMode::Battery] {
                        ui.selectable_value(&mut s.adaptive_quality, mode, tr(mode.label()));
                    }
                }).response
            });
            if s.adaptive_quality != crate::config::QualityMode::Off {
                setting_row(ui, s, &mut rows, &["quality_min_short_edge"], "Minimum resolution", "Shortest edge, in pixels. Never increases the selected stream size.", |ui, s| {
                    step_value(ui, &mut s.quality_min_short_edge, 256..=2160, 8.0, |d| d.speed(2).suffix(" px"))
                });
                setting_row(ui, s, &mut rows, &["quality_min_fps"], "Minimum frame rate", "Automatic quality stays above this rate.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["codec"], "Codec", "Automatic picks HEVC, then H.264. PyroWave needs GPU rendering; restart after selecting it on Linux.", |ui, s| {
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
            let upscaling_help = if s.upscaling == crate::config::Upscaling::Ai {
                en("Uses the GPU's AI video upscaling, for SDR and HDR. On NVIDIA with an HDR display it also turns SDR streams into HDR. Costs GPU power.")
            } else {
                en("Enhance smaller SDR streams. HDR uses bilinear. Restart after enabling on Linux.")
            };
            setting_row(ui, s, &mut rows, &["upscaling"], "Upscaling", upscaling_help, |ui, s| {
                egui::ComboBox::from_id_salt("upscaling").selected_text(tr(s.upscaling.label())).show_ui(ui, |ui| {
                    // AI enhancement is offered only where the GPU's probe finds the vendor's super
                    // resolution or RTX Video HDR. It loads the AMF runtime, so it runs once per
                    // renderer device, in the background, when the menu is first opened.
                    let probe = crate::stream::enhance::settings_probe(probe_device);
                    if probe.is_none() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(200)); }
                    let ai_offered = probe.flatten().is_some();
                    for mode in [crate::config::Upscaling::Bilinear, crate::config::Upscaling::Fsr1, crate::config::Upscaling::Sgsr1, crate::config::Upscaling::Ai] {
                        if mode == crate::config::Upscaling::Ai && !ai_offered && s.upscaling != mode { continue; }
                        ui.selectable_value(&mut s.upscaling, mode, tr(mode.label()));
                    }
                }).response
            });
            if s.upscaling.is_shader() {
                setting_row(ui, s, &mut rows, &["upscale_sharpness"], "Sharpness", "Extra sharpening applied by the upscaler.", |ui, s| {
                    ui.add(egui::Slider::new(&mut s.upscale_sharpness, 0..=100).suffix("%"))
                });
            }
            setting_row(ui, s, &mut rows, &["hdr"], "HDR",
                "Streams 10-bit HDR to an HDR-enabled display. Enable HDR in the operating system first.",
                |ui, s| toggle(ui, &mut s.hdr),
            );
            setting_row(ui, s, &mut rows, &["prefer_yuv444"], "Prefer YUV 4:4:4",
                "Sharper colored text when the PC and this device support it. HDR takes priority. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave() && !s.experimental_packed_444, |ui| toggle(ui, &mut s.prefer_yuv444)).inner);
            setting_row(ui, s, &mut rows, &["sdr_10bit"], "10-bit color (SDR)",
                "Streams SDR in 10-bit HEVC or AV1 to reduce banding. Needs a Main10 decoder on this device; the PC’s display must be in SDR. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave() && !s.experimental_packed_444, |ui| toggle(ui, &mut s.sdr_10bit)).inner);
            setting_row(ui, s, &mut rows, &["decoder"], "Decoder", "Automatic uses hardware decoding when available. PyroWave always uses GPU wavelets.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["pyrowave_decoder"], "PyroWave decoder", &decoder_note, |ui, s| {
                egui::ComboBox::from_id_salt("pyrowave-decoder")
                    .selected_text(tr(s.pyrowave_decoder.label()))
                    .show_ui(ui, |ui| {
                        for mode in [crate::config::PyroWaveDecoder::Cpu, crate::config::PyroWaveDecoder::Gpu] {
                            ui.selectable_value(&mut s.pyrowave_decoder, mode, tr(mode.label()));
                        }
                    })
                    .response
            });

            rows.section(ui, "Display & image", &mut jump);
            setting_row(ui, s, &mut rows, &["video_scaling"], "Video size", "Fit shows the whole stream; Fill crops the edges; Native uses one display pixel per stream pixel.", |ui, s| {
                use crate::config::VideoScaling;
                egui::ComboBox::from_id_salt("video-scaling").selected_text(tr(s.video_scaling.label())).show_ui(ui, |ui| {
                    for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                        ui.selectable_value(&mut s.video_scaling, mode, tr(mode.label()));
                    }
                }).response
            });
            setting_row(ui, s, &mut rows, &["align_video_top"], "Align video to top", "Moves Fit and Native video to the top of the window.", |ui, s| toggle(ui, &mut s.align_video_top));
            setting_row(ui, s, &mut rows, &["virtual_display"], "Virtual display", "Use a separate display on hosts that support it.", |ui, s| toggle(ui, &mut s.virtual_display));
            setting_row(ui, s, &mut rows, &["host_display"], "Display arrangement", "Applied only to normal streams on hosts that support display modes.", |ui, s| {
                egui::ComboBox::from_id_salt("host-display")
                    .selected_text(tr(host_display_name(s.host_display)))
                    .show_ui(ui, |ui| {
                        for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary,
                            HostDisplay::Extended, HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
                            ui.selectable_value(&mut s.host_display, mode, tr(host_display_name(mode)));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["virtual_display_scale"], "PC render resolution", "Percent of the stream resolution that the PC renders at. The size of the transmitted video does not change. To send fewer pixels and upscale on this device, lower Resolution.", |ui, s| {
                ui.add(egui::Slider::new(&mut s.virtual_display_scale, crate::config::RENDER_SCALE).step_by(5.0).suffix("%"))
            });
            // A PC's own settings offer it only if its host reads rlDisplayScale (DisplayScale).
            let dpi_gate = display_scale_gate(test_host.and_then(|h| h.info.as_ref())
                .map_or(nightjar_core::http::Availability::Unknown, |i| i.capabilities.availability(nightjar_core::http::Feature::DisplayScale)));
            if let Some(note) = dpi_gate {
                let description = match note {
                    Some(note) => format!("{} {}", tr("Text and interface size on the PC’s virtual display."), tr(note)),
                    None => tr("Text and interface size on the PC’s virtual display.").to_owned(),
                };
                ui.add_enabled_ui(note.is_none(), |ui| {
                    setting_row(ui, s, &mut rows, &["virtual_display_dpi"], "PC display scaling", &description, |ui, s| {
                        egui::ComboBox::from_id_salt("virtual-display-dpi")
                            .selected_text(display_scale_name(s.virtual_display_dpi))
                            .show_ui(ui, |ui| {
                                for percent in crate::config::DISPLAY_SCALES {
                                    ui.selectable_value(&mut s.virtual_display_dpi, percent, display_scale_name(percent));
                                }
                            }).response
                    });
                });
            }
            setting_row(ui, s, &mut rows, &["window_mode"], "Window", "Borderless covers the display without changing its mode.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["vrr"], "Variable refresh rate", vrr_note, |ui, s| toggle(ui, &mut s.vrr));
            let immediate_packed = cfg!(windows) && s.experimental_packed_444 && tearing;
            ui.add_enabled_ui(s.vrr && !immediate_packed, |ui| {
                setting_row(ui, s, &mut rows, &["present_at_host_times"], "Show frames at the PC's frame times",
                    if immediate_packed {
                        en("Full chroma presents frames immediately with VRR, without the extra timing delay.")
                    } else {
                        en("Each frame appears at the moment the game made it, plus a steady delay, so motion stays even.")
                    },
                    |ui, s| toggle(ui, &mut s.present_at_host_times),
                );
            });
            ui.add_enabled_ui(!s.vrr, |ui| {
                setting_row(ui, s, &mut rows, &["frame_pacing"], "Frame pacing",
                    "Without VRR: Lowest latency shows the newest frame at once. Balanced waits for the display's refresh. With an FPS limit, frames that come early wait for their turn. Smoothest video shows every frame in turn and adds up to a frame of delay.",
                    |ui, s| {
                        egui::ComboBox::from_id_salt("frame-pacing").selected_text(tr(s.frame_pacing.label())).show_ui(ui, |ui| {
                            for mode in crate::config::FramePacing::ALL {
                                ui.selectable_value(&mut s.frame_pacing, mode, tr(mode.label()));
                            }
                        }).response
                    });
            });
            ui.add_enabled_ui(!s.vrr && !s.frame_pacing.forces_vsync(), |ui| {
                setting_row(ui, s, &mut rows, &["vsync"], "V-Sync", "Without VRR: wait for the display's refresh, no tearing.", |ui, s| {
                    toggle(ui, &mut s.vsync)
                });
            });

            rows.section(ui, "Controls", &mut jump);
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
            setting_row(ui, s, &mut rows, &["mouse_mode"], "Mouse", "Captured suits games; desktop pointer suits remote work. Ctrl+Alt+Shift+M switches.", |ui, s| {
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
            setting_row(ui, s, &mut rows, &["capture_system_keys"], "Send system keys",
                "Alt+Tab, the Windows key and similar go to the PC while input is captured.",
                |ui, s| toggle(ui, &mut s.capture_system_keys),
            );
            setting_row(ui, s, &mut rows, &["cursor_shapes"], "Local cursor", "Show the PC's cursor without video delay in desktop pointer mode when the host supports it.", |ui, s| {
                toggle(ui, &mut s.cursor_shapes)
            });
            setting_row(ui, s, &mut rows, &["mouse_speed"], "Mouse speed", "Sensitivity of the captured pointer.", |ui, s| ui.add(egui::Slider::new(&mut s.mouse_speed, 25..=400).suffix("%")));
            setting_row(ui, s, &mut rows, &["mouse_nav_buttons"], "Mouse navigation buttons", "Send the mouse's Back and Forward buttons to the PC.", |ui, s| toggle(ui, &mut s.mouse_nav_buttons));
            if cfg!(windows) {
                setting_row(ui, s, &mut rows, &["native_touchpad"], "Touchpad gestures on the PC", "Send the touchpad's fingers to a PC that takes them, so its gestures work there. Other PCs get the pointer as before.", |ui, s| toggle(ui, &mut s.native_touchpad));
            }
            setting_row(ui, s, &mut rows, &["right_alt_as_meta"], "Right Alt as Windows key", "Use Right Alt for Windows-key shortcuts on the PC.", |ui, s| toggle(ui, &mut s.right_alt_as_meta));
            setting_row(ui, s, &mut rows, &["controllers"], "Controllers", "Every connected controller becomes a controller on the PC.", |ui, s| {
                toggle(ui, &mut s.controllers)
            });
            ui.add_enabled_ui(s.controllers, |ui| {
                setting_row(ui, s, &mut rows, &["back_opens_menu"], "Back opens stream controls", "Use the controller's Back / Select button to open the menu.", |ui, s| toggle(ui, &mut s.back_opens_menu));
                setting_row(ui, s, &mut rows, &["gamepad_quit_combo"], "Controller quit combo", "Hold these buttons to leave the stream. Pick another if a game or your controller needs the default.", |ui, s| {
                    let style = crate::glyphs::Style::of(view.pads);
                    egui::ComboBox::from_id_salt("gamepad-quit-combo").selected_text(style.quit_combo(s.gamepad_quit_combo)).show_ui(ui, |ui| {
                        for combo in crate::config::GamepadQuitCombo::ALL {
                            ui.selectable_value(&mut s.gamepad_quit_combo, combo, style.quit_combo(combo));
                        }
                    }).response
                });
                setting_row(ui, s, &mut rows, &["controller_mouse"], "Controller mouse mode", "Hold Start for 750 ms, then release. A/B click and the D-pad scrolls.", |ui, s| toggle(ui, &mut s.controller_mouse));
                if s.controller_mouse {
                    setting_row(ui, s, &mut rows, &["analog_scrolling"], "Stick for scrolling", "The other stick moves the pointer. Off lets both sticks move it.", |ui, s| {
                        use crate::config::AnalogScrolling;
                        egui::ComboBox::from_id_salt("analog-scrolling").selected_text(match s.analog_scrolling { AnalogScrolling::Off => tr("Off"), AnalogScrolling::Left => tr("Left"), AnalogScrolling::Right => tr("Right") }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Off, tr("Off"));
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Left, tr("Left"));
                            ui.selectable_value(&mut s.analog_scrolling, AnalogScrolling::Right, tr("Right"));
                        }).response
                    });
                    setting_row(ui, s, &mut rows, &["mouse_fixed_step"], "Precise controller mouse", "Hold X for fixed pixel steps. Press Y to double the step from 1 to 128 pixels.", |ui, s| toggle(ui, &mut s.mouse_fixed_step));
                }
                setting_row(ui, s, &mut rows, &["gamepad_touchpad_as_mouse"], "Controller touchpad as mouse", "Move with one finger, scroll with two. Press the pad for a left click, or with two fingers for a right click.", |ui, s| toggle(ui, &mut s.gamepad_touchpad_as_mouse));
                setting_row(ui, s, &mut rows, &["disable_rumble"], "Disable vibration", "Turn off controller rumble, trigger rumble, and waveform haptics.", |ui, s| toggle(ui, &mut s.disable_rumble));
                setting_row(ui, s, &mut rows, &["swap_face_buttons"], "Nintendo button layout", "Swap A/B and X/Y.", |ui, s| toggle(ui, &mut s.swap_face_buttons));
                setting_row(ui, s, &mut rows, &["controller_motion"], "Motion controls", "Send gyro and accelerometer data when a game asks for it.", |ui, s| {
                    toggle(ui, &mut s.controller_motion)
                });
                setting_row(ui, s, &mut rows, &["controller_audio"], "DualSense USB audio", "Use waveform haptics, speaker, headset and microphone. Choose devices in stream controls; microphone forwarding also needs Microphone enabled.", |ui, s| {
                    toggle(ui, &mut s.controller_audio)
                });
                setting_row(ui, s, &mut rows, &["background_controllers"], "Input in the background", "Keep controllers working when Nightjar is not focused.", |ui, s| {
                    toggle(ui, &mut s.background_controllers)
                });
            });
            setting_row(ui, s, &mut rows, &["clipboard_sync"], "Clipboard sync", "Text copied here goes on the PC's clipboard, and text copied on the PC comes here. The PC must allow this device to read and write its clipboard.", |ui, s| {
                toggle(ui, &mut s.clipboard_sync)
            });
            rows.section(ui, "Audio", &mut jump);
            setting_row(ui, s, &mut rows, &["audio"], "Channels", "Surround needs a host and output device that support it.", |ui, s| {
                egui::ComboBox::from_id_salt("audio")
                    .selected_text(match s.audio {
                        AudioChannels::Stereo => tr("Stereo"),
                        AudioChannels::Surround51 => tr("5.1 surround"),
                        AudioChannels::Surround71 => tr("7.1 surround"),
                        AudioChannels::Surround714 => tr("7.1.4 surround"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.audio, AudioChannels::Stereo, tr("Stereo"));
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround51, tr("5.1 surround"));
                        ui.selectable_value(&mut s.audio, AudioChannels::Surround71, tr("7.1 surround"));
                        if crate::stream::offers_714() {
                            ui.selectable_value(&mut s.audio, AudioChannels::Surround714, tr("7.1.4 surround"));
                        }
                    })
                    .response
            });
            setting_row(ui, s, &mut rows, &["audio_passthrough"], "Audio passthrough", "Ask a Rubylight PC for uncompressed PCM audio instead of Opus: lossless, at 1.5 Mbps for stereo up to 6 Mbps for 7.1. Other PCs keep Opus.", |ui, s| {
                toggle(ui, &mut s.audio_passthrough)
            });
            setting_row(ui, s, &mut rows, &["play_audio_on_host"], "Play audio on the PC too", "Keep sound on the PC's speakers while streaming.", |ui, s| {
                toggle(ui, &mut s.play_audio_on_host)
            });
            setting_row(ui, s, &mut rows, &["microphone"], "Microphone", "Send this device's default microphone to a compatible host while streaming.", |ui, s| {
                toggle(ui, &mut s.microphone)
            });
            ui.add_enabled_ui(s.microphone, |ui| {
                setting_row(ui, s, &mut rows, &["mic_echo_cancel"], "Echo cancellation",
                    "Keeps the stream's sound from the speakers out of the microphone. On Windows, Automatic turns it off when the sound plays through headphones or a headset. On Linux it needs PipeWire's or PulseAudio's echo-cancel module.",
                    |ui, s| {
                        egui::ComboBox::from_id_salt("mic-echo-cancel").selected_text(tr(s.mic_echo_cancel.label())).show_ui(ui, |ui| {
                            for mode in crate::config::MicEcho::ALL {
                                ui.selectable_value(&mut s.mic_echo_cancel, mode, tr(mode.label()));
                            }
                        }).response
                    });
            });

            rows.section(ui, "Streaming", &mut jump);
            setting_row(ui, s, &mut rows, &["overlay", "overlay_mode"], "Performance overlay",
                "Compact shows one line with the stream's health, Statistics the frame rates, network, host and decode times, Debug every measurement. Ctrl+Alt+Shift+S and the stream menu change it too.", |ui, s| {
                let shown = if s.overlay { tr(s.overlay_mode.label()) } else { tr("Off") };
                egui::ComboBox::from_id_salt("overlay-mode").selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(!s.overlay, tr("Off")).clicked() { s.overlay = false; }
                    for mode in crate::config::OverlayMode::ALL {
                        if ui.selectable_label(s.overlay && s.overlay_mode == mode, tr(mode.label())).clicked() {
                            (s.overlay, s.overlay_mode) = (true, mode);
                        }
                    }
                }).response
            });
            ui.add_enabled_ui(s.overlay, |ui| {
                setting_row(ui, s, &mut rows, &["overlay_position"], "Overlay position", "The corner the performance overlay sits in.", |ui, s| {
                    egui::ComboBox::from_id_salt("overlay-position").selected_text(tr(s.overlay_position.label())).show_ui(ui, |ui| {
                        for corner in crate::config::OverlayCorner::ALL {
                            ui.selectable_value(&mut s.overlay_position, corner, tr(corner.label()));
                        }
                    }).response
                });
            });
            if let Some(host) = test_host {
                let online = host.paired() && host.reach == Reach::Online;
                row(ui, "Bandwidth", "Receive 32 MiB from this PC. Uses network data and requires a supported paired host.", |ui| {
                    let enabled = online && host.bandwidth_test();
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
            setting_row(ui, s, &mut rows, &["auto_reconnect"], "Recover interrupted streams", "Try to resume the same running app up to five times within one minute.", |ui, s| {
                toggle(ui, &mut s.auto_reconnect)
            });
            setting_row(ui, s, &mut rows, &["error_correction"], "Error correction", "How much recovery data the PC adds to the video. Automatic follows this device's losses. PCs that don't take this choice use their own setting.", |ui, s| {
                egui::ComboBox::from_id_salt("error-correction")
                    .selected_text(error_correction_name(s.error_correction))
                    .show_ui(ui, |ui| {
                        let fixed = ErrorCorrection::PERCENTAGES.map(ErrorCorrection::Percent);
                        for mode in [ErrorCorrection::Host, ErrorCorrection::Auto].into_iter().chain(fixed) {
                            ui.selectable_value(&mut s.error_correction, mode, error_correction_name(mode));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["network_grace_ms"], "Network interruption grace", "Keep the session alive through brief network interruptions.", |ui, s| {
                egui::ComboBox::from_id_salt("network-grace")
                    .selected_text(trf("{} seconds", &[&(s.network_grace_ms / 1000)]))
                    .show_ui(ui, |ui| {
                        for ms in [0, 3000, 5000, 10000, 15000] {
                            ui.selectable_value(&mut s.network_grace_ms, ms, trf("{} seconds", &[&(ms / 1000)]));
                        }
                    }).response
            });
            setting_row(ui, s, &mut rows, &["client_roaming"], "Client roaming", "Keep the stream running when this device changes networks. Works with Rubylight PCs that support it.", |ui, s| {
                toggle(ui, &mut s.client_roaming)
            });
            if s.client_roaming {
                setting_row(ui, s, &mut rows, &["roaming_metered"], "Continue on mobile data", "Let the stream move onto mobile data or another metered network. Streaming uses a lot of data.", |ui, s| {
                    toggle(ui, &mut s.roaming_metered)
                });
            }
            setting_row(ui, s, &mut rows, &["usb_forwarding"], "USB forwarding", "Attach USB devices plugged into this computer to the PC from the stream controls. The PC must allow USB devices for this client. On Windows a device needs the WinUSB driver.", |ui, s| {
                toggle(ui, &mut s.usb_forwarding)
            });
            setting_row(ui, s, &mut rows, &["unbatched_input"], "Instant mouse and pen input", "Send mouse and pen movements immediately.", |ui, s| {
                toggle(ui, &mut s.unbatched_input)
            });
            // The engine sets socket and thread priorities on Linux only; on Windows the switch would do nothing.
            if cfg!(target_os = "linux") {
                setting_row(ui, s, &mut rows, &["network_priority"], "Prioritize stream traffic", "Give stream traffic priority on this device.", |ui, s| {
                    toggle(ui, &mut s.network_priority)
                });
            }

            if !layered {
                rows.section(ui, "App", &mut jump);
                row(ui, "Open on Home", "Start on the games to continue and your library. Off starts on the list of PCs.", |ui| {
                    toggle(ui, &mut s.console_home)
                });
                row(ui, "Quick resume", "Clicking a PC that is running a game goes straight back into the game instead of opening its library.", |ui| {
                    toggle(ui, &mut s.quick_resume)
                });
                if rows.shows(&["input_only_any_host"]) {
                    row(ui, "Input-only on any PC", "Adds Input-only to a game’s menu on PCs whose host has no Input-only mode of its own. The PC still sends video and audio at the smallest size, under 1 Mbit/s; Nightjar doesn’t show or play them. Hosts with their own mode keep using it.", |ui| {
                        toggle(ui, &mut s.input_only_any_host)
                    });
                }
                row(ui, "PIN lock", "Ask for a PIN to open Nightjar or to start a stream.", |ui| {
                    let set = self.pin.as_ref().is_some_and(|p| p.store.has_pin());
                    let button = quiet_button(ui, if set { en("Change PIN lock") } else { en("Set up") });
                    if button.clicked() {
                        if let Some(pin) = self.pin.as_mut() { pin.open_settings(); }
                    }
                    button
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
                if cfg!(windows) && rows.shows(&["check_updates"]) {
                    row(ui, "Check for updates", "Once per launch, asks GitHub whether a newer Nightjar release is out and says so here. Nothing is downloaded or installed.", |ui| {
                        toggle(ui, &mut s.check_updates)
                    });
                }
                if rows.shows(&["ui_zoom"]) {
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
                }
                row(ui, "Logs", "Nightjar keeps a log of the last two runs. Attach nightjar.log, and nightjar.prev.log if the problem was before a restart, to a bug report.", |ui| {
                    let button = quiet_button(ui, "Open logs folder");
                    if button.clicked() { commands.push(Command::OpenLogs); }
                    button
                });
                if rows.shows(&["high_contrast"]) {
                    row(ui, "High contrast", "Black surfaces, white text and thicker outlines and focus rings.", |ui| {
                        toggle(ui, &mut s.high_contrast)
                    });
                }
                if rows.shows(&["reduce_motion"]) {
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
            }
            rows.section(ui, "Advanced", &mut jump);
            setting_row(ui, s, &mut rows, &["full_range"], "Full color range", "Request the full RGB range from the host.", |ui, s| toggle(ui, &mut s.full_range));
            setting_row(ui, s, &mut rows, &["client_hint_bitrate"], "Loss-based bitrate (Rust engine)", "Lower the bitrate when frames are lost and raise it again after clean seconds, from the next frame on. Works with Rubylight PCs that take mid-stream rate changes, and replaces Device controls bitrate there.", |ui, s| {
                toggle(ui, &mut s.client_hint_bitrate)
            });
            setting_row(ui, s, &mut rows, &["experimental_packed_444"], "Full chroma", "Requires the host option and HEVC or AV1. Restart Nightjar after changing this setting on Linux.",
                |ui, s| ui.add_enabled_ui(!s.codec.is_pyrowave(), |ui| toggle(ui, &mut s.experimental_packed_444)).inner);
            rows.group(ui, "Diagnostics");
            setting_row(ui, s, &mut rows, &["tell_host_wireless"], "Tell the PC when on Wi-Fi", "A Rubylight PC then paces the stream for Wi-Fi from the start. Other PCs are not told.", |ui, s| {
                toggle(ui, &mut s.tell_host_wireless)
            });
            setting_row(ui, s, &mut rows, &["tell_host_link_speed"], "Tell the PC the link speed", "Lets the PC limit its sending rate to this device's wired port speed.", |ui, s| {
                toggle(ui, &mut s.tell_host_link_speed)
            });
            setting_row(ui, s, &mut rows, &["link_probe"], "Link test before the first bitrate", "A new stream at the automatic bitrate tests the link to a Rubylight PC first and starts no higher than it holds.", |ui, s| {
                toggle(ui, &mut s.link_probe)
            });
            rows.finish(ui);
            ui.add_space(18.0);
            ui.label(
                RichText::new(trf(
                    "While streaming: Ctrl+Alt+Shift+Q leaves, +Z releases input, +X toggles fullscreen, +D minimizes. \
                     On a controller, {} leaves.",
                    &[&crate::glyphs::Style::of(view.pads).quit_combo(s.gamepad_quit_combo)],
                ))
                .color(OUTLINE)
                .font(regular(text::CAPTION)),
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

/// A folder or file of the shared folder browser: its name as the PC spells it (never
/// translated), what it is, and its button.
fn folder_line(ui: &mut Ui, name: &str, info: &str, control: impl FnOnce(&mut Ui) -> Response) -> Response {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).font(semibold(text::BODY)).color(ON_SURFACE));
        ui.label(RichText::new(info).font(regular(text::CAPTION)).color(ON_SURFACE_VARIANT));
        ui.with_layout(Layout::right_to_left(Align::Center), control).inner
    })
    .inner
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
            .font(regular(text::CAPTION))
            .color(OUTLINE),
    );
}

/// How a PC's own settings offer PC display scaling, as Android's per-PC "dpi" row: None hides
/// it on a host that can never read `rlDisplayScale`; Some(note) draws it, turned off with the
/// note on a host that could (an older Rubylight, a virtual display that is off or not ready).
/// The main settings and a PC not reached yet offer it; the launch sends it only to hosts with
/// `Feature::DisplayScale`.
fn display_scale_gate(availability: nightjar_core::http::Availability) -> Option<Option<&'static str>> {
    use nightjar_core::http::{Availability, Reason};
    match availability {
        Availability::Supported | Availability::Unknown => Some(None),
        Availability::Unsupported(Some(Reason::UpdateHost)) => Some(Some(en("Update the host software to use this."))),
        Availability::Unsupported(Some(Reason::VirtualDisplayOff)) => Some(Some(en("Turn on the virtual display in the host’s settings."))),
        Availability::Unsupported(Some(Reason::VirtualDisplayDriver)) => Some(Some(en("The host’s virtual display driver isn’t ready."))),
        Availability::Unsupported(_) => None,
    }
}

fn display_scale_name(percent: u32) -> String {
    if percent == 0 { tr("PC’s own").into() } else { format!("{percent}%") }
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

/// Quick resume: a paired PC running a game from its known library goes straight back into it.
fn host_clicked(host: &Host, quick_resume: bool, commands: &mut Vec<Command>) {
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
fn small_stream_offer(host: &Host, app: &nightjar_core::http::App, running: bool, any_host: bool) -> Option<Option<nightjar_policy::launch::InputOnlyBlocker>> {
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

fn policy_control(control: nightjar_core::http::AppControl) -> nightjar_policy::launch::Control {
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
fn running_app_id(host: &Host) -> Option<i32> {
    let running = host.running_app()?;
    match &host.apps {
        Apps::Loaded(apps) => apps.iter().find(|a| a.matches_running_app(running, None)).map(|a| a.id),
        _ => None,
    }
}

fn error_correction_name(mode: ErrorCorrection) -> String {
    match mode {
        ErrorCorrection::Host => tr("PC's setting").to_owned(),
        ErrorCorrection::Auto => tr("Automatic").to_owned(),
        ErrorCorrection::Percent(percentage) => trf("{} %", &[&percentage]),
    }
}

/// The main rows of each section, as `client/app/src/main/res/values/settings_tiers.xml` lists them
/// by Android preference key, in `SETTINGS_SECTIONS` order. A test keeps the two equal.
const MAIN_KEYS: [(&str, &[&str]); 7] = [
    ("video", &["list_resolution", "list_fps", "seekbar_bitrate_kbps", "checkbox_auto_bitrate", "list_mobile_bitrate_kbps", "video_format", "checkbox_enable_hdr"]),
    ("display", &["checkbox_stretch_video", "stream_orientation"]),
    ("input", &["checkbox_multi_controller", "controller_button_mapping", "controller_test", "checkbox_flip_face_buttons", "checkbox_touchscreen_trackpad", "checkbox_show_onscreen_controls", "checkbox_keyboard_follows_text_fields", "checkbox_clipboard_sync"]),
    ("audio", &["list_audio_config", "checkbox_host_audio", "checkbox_enable_mic", "checkbox_mic_button"]),
    ("streaming", &["checkbox_enable_perf_overlay", "checkbox_auto_reconnect"]),
    ("app", &["checkbox_always_dark", "list_languages", "checkbox_console_home", "checkbox_quick_resume", "pin_lock", "help", "report_problem", "check_updates_now"]),
    ("advanced", &["checkbox_rust_engine", "low_latency_decoding", "low_latency_customize", "gpu_driver", "checkbox_gpu_max_clocks", "checkbox_texture_view", "checkbox_vulkan_render", "checkbox_full_range"]),
];

/// Desktop rows and the Android preference each one matches. A row is main when its match is.
const ANDROID_KEYS: [(&str, &str); 35] = [
    ("width", "list_resolution"), ("fps", "list_fps"), ("bitrate_kbps", "seekbar_bitrate_kbps"),
    ("adaptive_bitrate", "checkbox_auto_bitrate"), ("codec", "video_format"), ("hdr", "checkbox_enable_hdr"),
    ("video_scaling", "checkbox_stretch_video"), ("controllers", "checkbox_multi_controller"),
    ("swap_face_buttons", "checkbox_flip_face_buttons"), ("clipboard_sync", "checkbox_clipboard_sync"),
    ("audio", "list_audio_config"), ("play_audio_on_host", "checkbox_host_audio"), ("microphone", "checkbox_enable_mic"),
    ("overlay", "checkbox_enable_perf_overlay"), ("auto_reconnect", "checkbox_auto_reconnect"),
    ("console_home", "checkbox_console_home"), ("quick_resume", "checkbox_quick_resume"), ("language", "list_languages"),
    ("full_range", "checkbox_full_range"),
    ("client_hint_bitrate", "checkbox_rate_change_bitrate"), ("experimental_packed_444", "experimental_packed_444"),
    ("tell_host_wireless", "checkbox_tell_host_wireless"), ("tell_host_link_speed", "checkbox_tell_host_link_speed"),
    ("link_probe", "checkbox_link_probe"), ("client_roaming", "checkbox_client_roaming"),
    ("audio_passthrough", "checkbox_audio_passthrough"), ("usb_forwarding", "checkbox_usb_forwarding"),
    ("unbatched_input", "checkbox_unbatched_input"), ("network_priority", "checkbox_network_priority"),
    ("input_only_any_host", "checkbox_input_only_any_host"),
    ("gamepad_quit_combo", "list_gamepad_quit_combo"), ("check_updates", "checkbox_check_updates"),
    ("frame_pacing", "frame_pacing"), ("mic_echo_cancel", "list_mic_echo_cancel"),
    ("pin_lock", "pin_lock"),
];

/// Main rows Android has no match for.
const DESKTOP_MAIN: [&str; 2] = ["window_mode", "mouse_mode"];

fn main_row(key: &str) -> bool {
    DESKTOP_MAIN.contains(&key)
        || ANDROID_KEYS.iter().any(|&(desktop, android)| desktop == key && MAIN_KEYS.iter().any(|(_, main)| main.contains(&android)))
}

/// The settings page's rows as it draws them: each section shows its main rows and folds the rest
/// behind a "More … settings" button. A folded row a PC or profile overrides stays in view.
struct Rows<'a> {
    layer: Option<(&'a Settings, &'a mut Overrides)>,
    unfolded: &'a mut std::collections::BTreeSet<&'static str>,
    section: &'static str,
    /// The section's folded rows so far, drawn or not.
    folded: usize,
}

impl Rows<'_> {
    fn open(&self) -> bool {
        self.unfolded.contains(self.section)
    }

    /// Whether the row for `keys` is drawn.
    fn shows(&mut self, keys: &[&str]) -> bool {
        if keys.first().is_some_and(|k| main_row(k)) {
            return true;
        }
        self.folded += 1;
        self.open() || self.layer.as_ref().is_some_and(|(_, overrides)| overrides.has(keys))
    }

    /// Ends the section before, then starts `title`.
    fn section(&mut self, ui: &mut Ui, title: &'static str, jump: &mut Option<&'static str>) {
        self.finish(ui);
        section_at(ui, title, jump);
        (self.section, self.folded) = (title, 0);
    }

    /// A group heading inside the fold, such as Advanced's Diagnostics.
    fn group(&self, ui: &mut Ui, title: &str) {
        if self.open() {
            ui.add_space(6.0);
            ui.label(RichText::new(tr(title)).font(semibold(text::LABEL)).color(ON_SURFACE_VARIANT));
            ui.add_space(4.0);
        }
    }

    /// The section's fold button, when it has folded rows.
    fn finish(&mut self, ui: &mut Ui) {
        if self.folded == 0 {
            return;
        }
        let open = self.open();
        let label = if open { trf("Fewer {} settings", &[&tr(self.section)]) } else { trf("More {} settings ({})", &[&tr(self.section), &self.folded]) };
        let button = fold_row(ui, &label, open);
        if button.clicked() && !self.unfolded.remove(self.section) {
            self.unfolded.insert(self.section);
        }
        ui.add_space(6.0);
        self.folded = 0;
    }
}

/// A section's "More … settings" switch: a row like the settings it folds, its chevron pointing
/// down while they are hidden and up while they show.
fn fold_row(ui: &mut Ui, label: &str, open: bool) -> Response {
    let width = ui.available_width().min(SETTINGS_W);
    let (rect, response) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT + 4.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, label));
    let painter = ui.painter_at(rect.expand(6.0));
    let lit = response.hovered() || response.has_focus();
    painter.rect_filled(rect, CornerRadius::same(radius::CONTROL), if lit { SURFACE_HIGH } else { SURFACE });
    let galley = painter.layout_no_wrap(label.to_owned(), semibold(text::BODY), *ON_SURFACE);
    painter.galley(Pos2::new(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0), galley, *ON_SURFACE);
    let c = Pos2::new(rect.right() - 18.0 - 6.0, rect.center().y);
    let (tip, wing) = if open { (-3.0, 3.0) } else { (3.0, -3.0) };
    let stroke = Stroke::new(2.0, *ON_SURFACE_VARIANT);
    painter.line_segment([c + vec2(-6.0, wing), c + vec2(0.0, tip)], stroke);
    painter.line_segment([c + vec2(0.0, tip), c + vec2(6.0, wing)], stroke);
    focus_ring(ui, rect, &response, radius::CONTROL);
    ui.add_space(2.0);
    response
}

/// A setting's row, or None while its section folds it away.
fn setting_row(
    ui: &mut Ui,
    settings: &mut Settings,
    rows: &mut Rows<'_>,
    keys: &[&str],
    label: &str,
    description: &str,
    control: impl FnOnce(&mut Ui, &mut Settings) -> Response,
) -> Option<Response> {
    if !rows.shows(keys) {
        return None;
    }
    let Some((inherited, overrides)) = rows.layer.as_mut() else {
        return Some(row(ui, label, description, |ui| control(ui, settings)));
    };
    let inherited = *inherited;
    Some(row(ui, label, description, |ui| {
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.set_width(240.0);
            let mut own = overrides.has(keys);
            let response = ui.checkbox(&mut own, tr("Override"));
            if response.changed() && !own {
                overrides.clear(keys);
                *settings = overrides.apply(inherited);
            }
            let value = ui.add_enabled_ui(own, |ui| control(ui, settings)).inner;
            name_for_readers(ui, &value, label, description);
            if own { overrides.capture(settings, keys); }
            response.union(value)
        }).inner
    }))
}

/// A PC's card. With `line` (Home: what it knows of the games) the card is as compact as Home's
/// game cards, and the line follows the state on one row instead of the address and GPU.
fn host_card(ui: &mut Ui, host: &Host, line: Option<&str>) -> Response {
    let height = if line.is_some() { COMPACT_CARD_H } else { CARD_H };
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, height), Sense::click());
    let painter = ui.painter_at(rect.expand(6.0));
    let fill = if response.hovered() || response.has_focus() {
        SURFACE_HIGH
    } else {
        SURFACE
    };
    painter.rect_filled(rect, CornerRadius::same(radius::PANEL), fill);
    let (status, color, ring) = host_status(host);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{}, {status}", host.display_name())));
    // The status dot: full and green when the PC is up and paired, a ring otherwise. Amber stays the
    // action's colour, so it never stands for a state.
    let top = if line.is_some() { 28.0 } else { 30.0 };
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + top);
    if host.reach == Reach::Online && host.paired() {
        painter.circle_filled(dot, 7.0, OK);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, ring));
    }
    // A long name ends in "…" before the three dots.
    let mut name = egui::text::LayoutJob::simple_singleline(host.display_name().to_owned(), semibold(text::CARD_TITLE), *ON_SURFACE);
    name.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 44.0 - 46.0);
    let name = painter.layout_job(name);
    painter.galley(Pos2::new(rect.left() + 44.0, rect.top() + top - name.size().y / 2.0), name, *ON_SURFACE);
    if let Some(line) = line {
        // "Offline" twice said nothing more.
        let both = if line.is_empty() {
            status.clone()
        } else if line.starts_with(&status[..]) {
            line.to_owned()
        } else {
            format!("{status}  ·  {line}")
        };
        let cut = card_line(&painter, rect, 58.0, &both, regular(text::LABEL), *color);
        focus_ring(ui, rect, &response, radius::PANEL);
        return if cut { response.on_hover_text(both) } else { response };
    }
    let detail = match host.info.as_ref().and_then(|i| i.gpu_type.clone()) {
        Some(gpu) if !gpu.is_empty() => format!("{}  ·  {gpu}", host.record.address),
        _ => host.record.address.clone(),
    };
    // The state and the address with the GPU end in "…" inside the card's padding; whatever was cut
    // is in the tooltip.
    let status_cut = card_line(&painter, rect, 62.0, &status, regular(text::LABEL), *color);
    let detail_cut = card_line(&painter, rect, 88.0, &detail, regular(text::CAPTION), *OUTLINE);
    let tooltip = match (status_cut, detail_cut) {
        (false, false) => None,
        (true, false) => Some(status),
        (false, true) => Some(detail),
        (true, true) => Some(format!("{status}\n{detail}")),
    };
    focus_ring(ui, rect, &response, radius::PANEL);
    match tooltip {
        Some(text) => response.on_hover_text(text),
        None => response,
    }
}

/// One line of a card at `y` from its top, cut with "…" at the card's padding. True when it was cut.
fn card_line(painter: &egui::Painter, card: Rect, y: f32, text: &str, font: egui::FontId, color: Color32) -> bool {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(card.width() - 2.0 * CARD_PAD);
    let galley = painter.layout_job(job);
    let cut = galley.elided;
    painter.galley(Pos2::new(card.left() + CARD_PAD, card.top() + y - galley.size().y / 2.0), galley, color);
    cut
}

/// What a PC's card says of it: the words, their colour, and the colour of the dot's ring.
fn host_status(host: &Host) -> (String, Role, Role) {
    match (host.reach, host.paired()) {
        (Reach::Offline, _) => (tr("Offline").to_owned(), OUTLINE, OUTLINE),
        (Reach::Unknown, _) => (tr("Checking…").to_owned(), ON_SURFACE_VARIANT, ON_SURFACE_VARIANT),
        (Reach::Online, false) => (tr("Ready to pair").to_owned(), TERTIARY, ON_SURFACE_VARIANT),
        (Reach::Online, true) => match host.running_app() {
            Some(_) => (tr("Paired · an app is running").to_owned(), OK, OK),
            None => (tr("Paired · ready").to_owned(), OK, OK),
        },
    }
}

/// The cover art, or the night surface with the banner's orbits when the PC has none.
/// The part of art of size `art` that fills a box of size `box_size` without stretching it: the
/// middle of it, cut at the sides or at top and bottom (CSS's `object-fit: cover`).
fn cover_uv(art: Vec2, box_size: Vec2) -> Rect {
    if art.x <= 0.0 || art.y <= 0.0 || box_size.x <= 0.0 || box_size.y <= 0.0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let (art_aspect, box_aspect) = (art.x / art.y, box_size.x / box_size.y);
    if art_aspect > box_aspect {
        let w = box_aspect / art_aspect;
        Rect::from_min_max(Pos2::new((1.0 - w) / 2.0, 0.0), Pos2::new((1.0 + w) / 2.0, 1.0))
    } else {
        let h = art_aspect / box_aspect;
        Rect::from_min_max(Pos2::new(0.0, (1.0 - h) / 2.0), Pos2::new(1.0, (1.0 + h) / 2.0))
    }
}

fn paint_cover(ui: &Ui, image: Rect, name: &str, art: Option<&Art>, running: bool) {
    let painter = ui.painter_at(image.expand(6.0));
    match art {
        Some(Art::Ready(texture)) => {
            egui::Image::from_texture((texture.id(), image.size()))
                .uv(cover_uv(texture.size_vec2(), image.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, image);
        }
        _ => {
            painter.rect_filled(image, CornerRadius::same(10), SURFACE);
            let orbits = ui.painter_at(image.shrink(1.0));
            for r in [70.0, 105.0, 140.0] {
                orbits.circle_stroke(image.right_bottom(), r, Stroke::new(1.5, ORBIT));
            }
            // No art: the whole title, as Android's cover text, instead of initials ("C2" said nothing).
            let mut job = egui::text::LayoutJob::single_section(
                name.to_owned(),
                egui::TextFormat { font_id: semibold(text::CARD_TITLE), color: *ON_SURFACE_VARIANT, ..Default::default() },
            );
            job.halign = Align::Center;
            job.wrap = egui::text::TextWrapping { max_width: image.width() - 2.0 * space::L, max_rows: 4, break_anywhere: false, overflow_character: Some('…') };
            let galley = painter.layout_job(job);
            painter.galley(Pos2::new(image.center().x, image.center().y - galley.size().y / 2.0), galley, *ON_SURFACE_VARIANT);
        }
    }
    if running {
        running_badge(&painter, image.left_top() + vec2(space::S, space::S));
    }
}

/// "Running" over a cover: a green dot and white words on the scrim, which reads over any art
/// (the console's media pill). Returns where it went.
fn running_badge(painter: &egui::Painter, at: Pos2) -> Rect {
    const DOT: f32 = 4.0;
    let words = painter.layout_no_wrap(tr("Running").to_owned(), semibold(text::CAPTION), Color32::WHITE);
    let height = space::XL;
    // The dot sits closer to the edge than the words do: it is lighter.
    let width = space::S + DOT * 2.0 + space::S + words.size().x + space::M;
    let badge = Rect::from_min_size(at, vec2(width, height));
    painter.rect_filled(badge, CornerRadius::same((height / 2.0) as u8), SCRIM);
    let dot = Pos2::new(badge.left() + space::S + DOT, badge.center().y);
    painter.circle_filled(dot, DOT, OK);
    let words_at = Pos2::new(dot.x + DOT + space::S, badge.center().y - words.size().y / 2.0);
    painter.galley(words_at, words, Color32::WHITE);
    badge
}

/// A game's state over its title, as Android's game page: a dot and the word in the state's
/// colour on a pill of the same hue.
fn state_chip(ui: &mut Ui, label: &str, color: Role) {
    const DOT: f32 = 4.0;
    let color: Color32 = color.into();
    let words = ui.painter().layout_no_wrap(tr(label).to_owned(), semibold(text::CAPTION), color);
    let size = vec2(space::M + DOT * 2.0 + space::S + words.size().x + space::M, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(13), color.gamma_multiply(0.16));
    let dot = Pos2::new(rect.left() + space::M + DOT, rect.center().y);
    painter.circle_filled(dot, DOT, color);
    painter.galley(Pos2::new(dot.x + DOT + space::S, rect.center().y - words.size().y / 2.0), words, color);
    name_for_readers(ui, &response, tr(label), "");
    ui.add_space(space::XS);
}

/// The PC a game is on: a monitor and its name on their own line.
fn pc_line(ui: &mut Ui, name: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        let ink: Color32 = ON_SURFACE_VARIANT.into();
        let stroke = Stroke::new(1.5, ink);
        let screen = Rect::from_min_max(rect.left_top() + vec2(1.0, 2.0), rect.right_top() + vec2(-1.0, 13.0));
        ui.painter().rect_stroke(screen, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
        let foot = rect.center_bottom() - vec2(0.0, 2.0);
        ui.painter().line_segment([Pos2::new(foot.x, screen.bottom()), foot], stroke);
        ui.painter().line_segment([foot - vec2(4.0, 0.0), foot + vec2(4.0, 0.0)], stroke);
        ui.label(RichText::new(name).font(regular(text::BODY)).color(ON_SURFACE_VARIANT));
    });
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

/// A game's cover. Its title shows on one line under it only while it has focus or the pointer:
/// cover art carries the title already, and a cover without art shows it inside. The focused cover
/// lifts a little (not with reduced motion).
fn poster(ui: &mut Ui, name: &str, art: Option<&Art>, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(POSTER_W, POSTER_H + POSTER_CAPTION), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if running { trf("{}, running", &[&name]) } else { name.to_owned() }));
    let lifted = response.has_focus() || response.hovered();
    let lift = ui.ctx().animate_bool_responsive(response.id.with("lift"), lifted);
    let image = Rect::from_min_size(rect.min, vec2(POSTER_W, POSTER_H)).expand(lift * 3.0);
    paint_cover(ui, image, name, art, running);
    if lifted && poster_caption(art) {
        let painter = ui.painter_at(rect.expand(6.0));
        let color = if response.has_focus() { MOON } else { ON_SURFACE };
        let mut job = egui::text::LayoutJob::simple_singleline(name.to_owned(), semibold(text::LABEL), *color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(POSTER_W);
        let galley = painter.layout_job(job);
        painter.galley(Pos2::new(rect.left() + 2.0, image.bottom() + space::S), galley, *color);
    }
    focus_ring(ui, image, &response, 10);
    response
}

/// Whether a lifted cover names its game underneath: only over art, as a cover without art shows
/// the full title inside already.
fn poster_caption(art: Option<&Art>) -> bool {
    matches!(art, Some(Art::Ready(_)))
}

/// A game to carry on with: title, and where it runs.
fn continue_card(ui: &mut Ui, title: &str, line: &str, running: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, COMPACT_CARD_H), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{title}, {line}")));
    let painter = ui.painter_at(rect.expand(6.0));
    let fill = if response.hovered() || response.has_focus() { SURFACE_HIGH } else { SURFACE };
    painter.rect_filled(rect, CornerRadius::same(radius::PANEL), fill);
    let color = if running { OK } else { TERTIARY };
    let dot = Pos2::new(rect.left() + 26.0, rect.top() + 28.0);
    if running {
        painter.circle_filled(dot, 7.0, color);
    } else {
        painter.circle_stroke(dot, 6.0, Stroke::new(2.0, color));
    }
    let inside = ui.painter_at(rect);
    let mut job = egui::text::LayoutJob::simple_singleline(title.to_owned(), semibold(text::CARD_TITLE), *ON_SURFACE);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 44.0 - CARD_PAD);
    let galley = inside.layout_job(job);
    inside.galley(Pos2::new(rect.left() + 44.0, rect.top() + 28.0 - galley.size().y / 2.0), galley, *ON_SURFACE);
    card_line(&inside, rect, 58.0, line, regular(text::LABEL), *color);
    focus_ring(ui, rect, &response, radius::PANEL);
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

#[cfg(test)]
mod tests;
