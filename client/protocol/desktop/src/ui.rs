//! The launcher: PCs, pairing, the app library, settings, and the connecting card. It only
//! draws and collects [`Command`]s; `main.rs` carries them out.

mod backdrop;
mod cards;
mod chrome;
#[allow(dead_code, reason = "generated: every token is listed, the pages use them as they are redesigned")]
pub(crate) mod design_tokens;
#[cfg(any(feature = "demo-library", test))]
pub(crate) mod demo;
mod dialogs;
pub(crate) mod glass;
mod icons;
pub mod files;
mod first_run;
mod game;
mod help;
mod home;
mod library;
mod pages;
pub(crate) mod pcs;
mod settings;
mod spring;
mod widgets;

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder, Vec2,
};

use crate::config::{
    step_zoom, AudioChannels, CardSize, CodecPreference, DecoderPreference, ErrorCorrection, HomeGroup, HomeLayout, HostDisplay, Language,
    MouseMode, Overrides, Profiles, ReduceMotion, Settings, WindowMode, UI_ZOOM_STEPS,
};
use crate::console::{self, EndAction, GameState, Playing, Source};
use crate::hosts::{Apps, Art, Handed, Host, Hosts, PcPowerStep, Reach, SleepStep, UpdateStep};
use nightjar_core::http::PcPower;
use crate::strings::{en, tr, trf};
use crate::stream::{pyrowave_bitrate_warning, stream_mode, Display};
use crate::theme::*;
use cards::*;
use dialogs::*;
pub use dialogs::moves_in_hotkey_dialog;
pub use library::{asked_start, join_target, playing_elsewhere, policy_role, running_name};
use library::*;
pub(crate) use widgets::{column, focus_ring, focus_ring_around, glass_behind, group_row, lift_shapes, name_for_readers, page_scroll_from, pin_key, pin_slots, primary_button, quiet_button, round_button, row, section, toggle, Combo, ToTop, Tone, DIALOG_MARGIN};
pub(crate) use chrome::{BACK_SIZE, PAGE_TITLE};
use widgets::*;
pub(crate) use icons::Icon;

/// Where Nightjar's guides live, the pages Android's help opens.
pub const GUIDES: &str = "https://github.com/RamazanKara/nightjar/blob/main/docs/";

pub enum Command {
    /// The game a cold start goes on to, chosen in its menu; None takes it back.
    OpenOnStart(Option<crate::config::OpenGame>),
    /// "Start Nightjar when I sign in".
    SetSignIn(bool),
    Launch { host: u64, app: i32, resume: bool },
    /// A launch with nobody to ask: without [`Command::Launch`]'s read of who plays.
    LaunchUnasked { host: u64, app: i32, resume: bool },
    /// Input-only through a small ordinary stream, on a PC without its own Input-only mode.
    LaunchInputOnly { host: u64, app: i32, resume: bool },
    /// Only this device's microphone, on a PC that offers it (`Feature::MicOnly`).
    MicOnly { host: u64 },
    SetQuitOnLeave { host: u64, uuid: String, id: i32, enabled: bool },
    QuitApp(u64),
    ControlAction { host: u64, app: i32 },
    /// `named` is the app the prompt said would be closed; it is checked again before closing.
    QuitAndLaunch { host: u64, app: i32, named: Option<i32> },
    /// Start the app the PC refused again, closing the one it runs: the answer to its prompt.
    ConfirmReplace,
    /// Move the stream of the running game to this device, then resume it.
    ContinueHere { host: u64, app: i32 },
    /// Start a stream beside the other device's, as a start without the prompt does.
    StreamHereToo { host: u64, app: i32, resume: bool },
    /// Stream here too on "did not move": the answer may be old, so it asks who plays first.
    StreamTooAfterFailedMove { host: u64, app: i32 },
    Pair(u64),
    PairOtp { host: u64, pin: String, passphrase: String },
    /// Add and pair the PC of a link the user confirmed.
    PairLink(Box<rubylight_protocol::pairing_uri::PairingUri>),
    ApprovePairing { host: u64, request: String, name: String, pin: String },
    DenyPairing { host: u64, request: String, name: String },
    Wake(u64),
    /// Stop waking the PC ("Waking <PC>…").
    CancelWake,
    /// Ask the PC for a sleep token, which the confirmation then spends. `ask` numbers the
    /// request, so a late answer to an earlier one is not taken for it.
    SleepOffer { host: u64, ask: u64 },
    /// Put the PC to sleep with the token it handed out at `at`.
    Sleep { host: u64, ask: u64, token: String, at: Handed },
    /// Ask the PC for a Restart PC or Shut down PC token, as `SleepOffer`.
    PcPowerOffer { host: u64, ask: u64, action: PcPower },
    /// Restart or shut down the PC with the token it handed out at `at`.
    PcPower { host: u64, ask: u64, action: PcPower, token: String, at: Handed },
    /// Install the PC's host update and wait for it to come back.
    InstallUpdate { host: u64 },
    /// Name a PC on this device only; empty brings back the PC's own name.
    Rename { host: u64, alias: String },
    /// Always connect to the PC through this known address; None brings back the automatic pick.
    PinAddress { host: u64, address: Option<String> },
    HostSettings(u64),
    SaveHostSettings(u64),
    AppSettings { host: u64, app: nightjar_core::http::App },
    SaveAppSettings(u64),
    /// Every PC and game follows its profile or the main settings again (`Dialog::KeptSettings`).
    ResetKeptSettings,
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
    /// Settings → Controls → Stream hotkeys.
    StreamHotkeys,
    /// Save the stream hotkeys as `shortcuts::hotkey_bindings` holds them now.
    SaveHotkeys,
    ImportShortcuts,
    ExportShortcuts,
    /// Show the folder with Nightjar's log files in the file manager.
    OpenLogs,
    /// Check that the PC answers at its saved address (the details page's remote access test).
    RemoteCheck(u64),
    /// Read a PC's app list if it was never read: Home's Play starts from it.
    LoadApps(u64),
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
    /// Hides these games on this device, or shows them again.
    SetHidden { games: Vec<crate::config::Favourite>, hidden: bool },
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
    /// Shut down, restart or sleep this PC, once confirmed (`Dialog::ConfirmPower`).
    Power(crate::power::Offer),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Games to continue, the merged library and the PCs.
    Home,
    /// Every game of every PC, filtered by store or by what is installed.
    AllGames,
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
    StreamHotkeys,
    /// Getting started, the keyboard and controller shortcuts, sharing and common problems.
    Help,
}

impl Screen {
    /// The top-level destination this page belongs to, marked in the top bar.
    pub fn destination(self) -> Screen {
        match self {
            Self::Home | Self::AllGames | Self::Game { .. } => Self::Home,
            Self::Hosts | Self::Apps(_) | Self::HostDetails(_) | Self::Files(_) | Self::HostSettings(_) | Self::AppSettings(_) => Self::Hosts,
            Self::Settings | Self::Profiles | Self::ProfileSettings(_) | Self::Controllers | Self::Shortcuts | Self::StreamHotkeys => Self::Settings,
            Self::Help => Self::Help,
        }
    }

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
    /// A pairing link the system opened with Nightjar: any web page can open one, so nothing is
    /// added or paired until Pair is chosen here. `shown` is when it first appeared (egui's time):
    /// Pair stays off for a moment, so a click meant for what was there before cannot land on it.
    ConfirmLink { link: Box<rubylight_protocol::pairing_uri::PairingUri>, shown: Option<f64> },
    NetworkTest { stage: String },
    Otp { host: u64, pin: String, passphrase: String, error: Option<String> },
    Message { title: String, body: String },
    /// The countdown of an automatic start (`open_play`): closing it any way cancels the start.
    OpenPlay { game: String, pc: String, left: u64 },
    /// Quit the running app, then start `start`. `resume` is the running app's id when the
    /// dialog can offer to carry on with it instead.
    ConfirmQuit { host: u64, start: Option<(i32, String)>, running: String, resume: Option<i32>, named: Option<i32> },
    ConfirmAction { host: u64, app: i32, name: String },
    /// The PC refused to start `app` until the user confirms closing `running`. Closing the
    /// dialog any other way than confirming cancels the launch.
    ConfirmReplace { app: String, running: String },
    /// `name` runs on `device`: move it here, stream it here too, or neither.
    ContinueHere { host: u64, app: i32, resume: bool, name: String, device: String },
    /// This device's stream moved to `device`.
    NowPlaying { host: u64, app: i32, device: String },
    /// The move failed while "Now playing on ..." was open: the game is this device's again.
    GameCameBack { host: u64, app: i32 },
    /// The move failed while "Now playing on ..." was open, and the game is another device's.
    MoveFailed { host: u64, app: i32 },
    ConfirmClose { host: u64, name: String },
    ProfileName { id: Option<u64>, name: String, settings: Overrides, error: Option<String> },
    DeleteProfile { id: u64, name: String },
    Shortcut { id: Option<String>, name: String, chord: String, error: Option<String> },
    DeleteShortcut { id: String, name: String },
    /// Records the next chord pressed for the stream hotkey `id`; `error` says why the last was refused.
    StreamHotkey { id: &'static str, error: Option<String> },
    ImportShortcuts(crate::shortcuts::Shortcuts),
    /// A PC's actions in one list, in the order of `host_options`.
    HostOptions { host: u64 },
    /// The PCs, from the top bar's capsule with two or more: `current` is the one it shows.
    PcSheet { current: u64 },
    /// A game's actions: its page, favourites, its settings, starting it.
    GameMenu { host: u64, game: GameTarget },
    Rename { host: u64, text: String },
    ConfirmRemove { host: u64, name: String },
    ConfirmUnpair { host: u64, name: String },
    /// Sleep PC: waiting for the PC's token (`offer` None), then asking to confirm. `ask` is
    /// the number of the token request this dialog waits for.
    ConfirmSleep { host: u64, ask: u64, name: String, offer: Option<(String, Handed)> },
    /// Restart PC or Shut down PC, as `ConfirmSleep`; the offer also says whether the PC can
    /// be woken from off.
    ConfirmPcPower { host: u64, ask: u64, name: String, action: PcPower, offer: Option<(String, Handed, bool)> },
    /// Update the PC's host to `version`.
    ConfirmUpdate { host: u64, name: String, version: String },
    /// Devices waiting to pair: on one PC from its menu, on all from Home's banner.
    PairRequests { only: Option<u64> },
    /// The PIN a waiting device shows. `busy` while the PC finishes that device's handshake.
    ApprovePairing { host: u64, request: String, name: String, address: String, pc: String, pin: String, busy: bool },
    /// Pairing did not work: what to do about it, and a way to try again.
    PairError { host: u64, body: String, one_time: bool },
    /// How to pair, for a PC that is not paired yet.
    PairingHelp,
    /// Home's View, under its button: layout, grouping and card size, each changed in place.
    HomeView,
    /// Home or PCs in the top bar would leave a page whose transfers still run, which cancels
    /// them: asked first.
    ConfirmLeave { to: Screen },
    /// Back where Back ends (Home, or the PCs without Home): a controller's way to close Nightjar,
    /// or to shut down, restart or sleep this PC.
    ConfirmExit,
    /// Shut down, restart or sleep this PC? Asked once more from `ConfirmExit`.
    ConfirmPower(crate::power::Offer),
    /// Shown once after updating: the PCs that keep settings of their own or for games, one
    /// line each, and a reset of them all (`Launcher::show_kept_settings_once`).
    KeptSettings { pcs: Vec<KeptPc> },
}

/// A PC in `Dialog::KeptSettings`: its name, whether it has settings of its own, and for how
/// many games. Put into words as it is drawn, in the language chosen by then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptPc {
    pub name: String,
    pub own: bool,
    pub games: usize,
}

impl KeptPc {
    pub fn line(&self) -> String {
        let mut kept = Vec::new();
        if self.own { kept.push(tr("PC settings").to_string()); }
        match self.games {
            0 => {}
            1 => kept.push(tr("1 game").to_string()),
            n => kept.push(trf("{} games", &[&n])),
        }
        format!("{}: {}", self.name, kept.join(", "))
    }
}

/// The header's top-level destinations, left to right.
pub const DESTINATIONS: [(&str, Screen); 4] =
    [(en("Home"), Screen::Home), (en("PCs"), Screen::Hosts), (en("Settings"), Screen::Settings), (en("Help"), Screen::Help)];

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
    /// The layers under the one being edited that can give an unchecked row its value, from the
    /// top, with what the row then says: the PC and the profile in use for a game, the profile
    /// for a PC, none for a profile.
    pub inherited_from: Vec<(Overrides, String)>,
    pub editing_app: Option<nightjar_core::http::App>,
    pub edit_name: String,
    pub settings_dirty: bool,
    /// The settings page's open section, search and pending Reset all.
    settings_view: settings::SettingsView,
    /// The settings page where Custom… was chosen for the resolution and for the frame rate: its
    /// row shows there even at a preset, until a preset is chosen.
    custom_size: Option<Screen>,
    custom_fps: Option<Screen>,
    pub controller_key: String,
    pub controller_mapping: crate::gamepad::Mapping,
    pub controller_test: bool,
    /// The device in the hand: what the button hints name.
    pub input_source: crate::input_source::InputSource,
    pub shortcuts: crate::shortcuts::Shortcuts,
    /// Games streamed before, latest first.
    pub recents: Vec<crate::config::Recent>,
    /// Starred games, newest star first.
    pub favourites: Vec<crate::config::Favourite>,
    /// The game chosen for "When Nightjar opens".
    pub open_game: Option<crate::config::OpenGame>,
    /// Whether Nightjar starts at sign-in; None hides the row (no way to set it here).
    pub sign_in: Option<bool>,
    /// The games hidden on this device (hidden-games.json).
    pub hidden_games: Vec<crate::config::Favourite>,
    /// A PC's library page lists its hidden games too.
    show_hidden_apps: bool,
    logo: Option<TextureHandle>,
    focus_screen: bool,
    focus_dialog: bool,
    /// This frame's Escape came from a controller's B, which alone asks to close Nightjar where Back ends.
    pad_back: bool,
    /// The last Sleep PC token request's number.
    sleep_asks: u64,
    /// Messages that came while the user was busy with another dialog, shown once it closes.
    queued: std::collections::VecDeque<Dialog>,
    settings_return: Screen,
    /// Where Back goes: the screens left by moving forward, with the item that had focus.
    trail: Vec<(Screen, Option<String>)>,
    /// The item with focus on the current screen, by its key.
    focused_key: Option<String>,
    /// Where the open menu (PC options, a game's menu) is anchored.
    menu_anchor: Anchor,
    /// The All games page's filter.
    library_filter: console::LibraryFilter,
    /// The item to focus when the screen is drawn again after Back.
    restore_key: Option<String>,
    /// The cards a menu's Hide can take off this frame's screen, in the order drawn, with their game.
    cards: Vec<(String, String)>,
    /// Some PC has a library to show, so Home has something on it.
    home_ready: bool,
    /// The width the header's actions took last frame, to keep them at the right edge.
    header_width: f32,
    /// The game Home is about; the chrome's PC capsule speaks of its PC.
    hero: Option<home::Hero>,
    /// What lies behind the page.
    ambient: backdrop::Ambient,
    /// The top bar's search field is open on All games, with these words.
    search_open: bool,
    search_focus: bool,
    /// All games was opened for the search, so leaving the search leaves it too.
    search_pushed: bool,
    library_query: String,
    /// How far under the chrome this page's content scrolls: an item revealed rests below it.
    scroll_inset: f32,
    /// How far Home has scrolled, for the scrim under the chrome.
    page_scrolled: f32,
    /// The stream menu's demo page, over its still.
    #[cfg(any(test, feature = "demo-library"))]
    pub(crate) demo_menu: Option<demo::DemoMenu>,
    #[cfg(any(test, feature = "demo-library"))]
    demo_video: Option<egui::TextureHandle>,
    /// The focus went up into the chrome: Home goes back to its start beneath it.
    page_to_top: bool,
    /// Back was used on the page header this frame.
    back_pressed: bool,
    /// The PC whose empty list is being read again: its page holds still until the answer.
    refreshing_empty: Option<u64>,
    /// The first run shows a found PC: when it first does, its Pair takes the focus.
    found_shown: bool,
    /// Play pressed before the PC's app list arrived: it starts when the list lands.
    pending_play: Option<library::PendingPlay>,
    /// Where the page's Back has its slot this frame: None on a page without one.
    back_slot: Option<f32>,
    /// The display's tearing support, for the VRR setting's note.
    pub tearing_supported: bool,
    /// The renderer's Direct3D 11 device, which the PyroWave GPU decoder's settings probe runs on.
    pub gpu_device: Option<crate::stream::pipeline::GpuDevice>,
    pub display: Display,
    pub identity_ready: bool,
    /// What this PC's power allows (`power::probe`): the Close Nightjar question offers these.
    /// Empty until known.
    pub power: Vec<crate::power::Offer>,
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

const CONTENT_MAX: f32 = 1760.0;
/// Seconds a pairing link's question shows before Pair can be chosen.
const PAIR_LINK_DELAY: f64 = 1.0;
/// The column of settings rows takes what the page gives it up to this, so that on a very wide
/// window a row's label and its control stay in one glance. No window under about 1400 reaches it.
pub(crate) const SETTINGS_W: f32 = 1300.0;

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
            inherited_from: Vec::new(),
            editing_app: None,
            edit_name: String::new(),
            settings_dirty: false,
            settings_view: Default::default(),
            custom_size: None,
            custom_fps: None,
            controller_key: String::new(),
            controller_mapping: crate::gamepad::Mapping::default(),
            controller_test: false,
            input_source: crate::input_source::InputSource::None,
            shortcuts,
            recents: Vec::new(),
            favourites: Vec::new(),
            open_game: None,
            sign_in: None,
            hidden_games: Vec::new(),
            show_hidden_apps: false,
            logo: None,
            focus_screen: true,
            focus_dialog: false,
            pad_back: false,
            sleep_asks: 0,
            queued: Default::default(),
            settings_return: Screen::Hosts,
            trail: Vec::new(),
            focused_key: None,
            menu_anchor: Anchor::Center,
            library_filter: console::LibraryFilter::All,
            restore_key: None,
            cards: Vec::new(),
            home_ready: false,
            header_width: 0.0,
            hero: None,
            ambient: Default::default(),
            search_open: false,
            search_focus: false,
            search_pushed: false,
            library_query: String::new(),
            scroll_inset: 0.0,
            page_scrolled: 0.0,
            #[cfg(any(test, feature = "demo-library"))]
            demo_menu: None,
            #[cfg(any(test, feature = "demo-library"))]
            demo_video: None,
            page_to_top: false,
            back_pressed: false,
            refreshing_empty: None,
            found_shown: false,
            pending_play: None,
            back_slot: None,
            tearing_supported: false,
            gpu_device: None,
            display,
            identity_ready: false,
            power: Vec::new(),
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
        let lang = crate::strings::resolve(s.language, &self.system_languages);
        crate::strings::set(lang);
        crate::theme::fonts_for(ctx, lang);
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
        // The search lives on All games and the game pages it opens.
        if !matches!(screen, Screen::AllGames | Screen::Game { .. }) {
            self.search_open = false;
            self.search_pushed = false;
            self.library_query.clear();
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
            Screen::Profiles | Screen::Controllers | Screen::Shortcuts | Screen::StreamHotkeys => self.leave_subpage(),
            Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.go(Screen::Settings),
            // A library or game page is not a destination: Settings opens over it, and Back
            // returns to the page with its item focused.
            Screen::Apps(_) | Screen::Game { .. } | Screen::AllGames | Screen::HostDetails(_) | Screen::Files(_) => self.open(Screen::Settings),
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

    /// [`Self::focus_item`] for a card of a game, noted for the focus to move on when Hide takes it.
    fn focus_card(&mut self, response: &Response, key: &str, game: &str, first: bool) {
        self.cards.push((key.to_owned(), game.to_owned()));
        self.focus_item(response, key, first);
    }

    /// Gives an item focus: the first one on a new screen, or the one that had it before the
    /// user went forward. Records which item has focus so `open` can remember it.
    fn focus_item(&mut self, response: &Response, key: &str, first: bool) {
        // Arrow keys and the D-pad move the focus at the end of a pass, where egui never reports
        // `gained_focus`: the move shows here as a new focused key, and the item scrolls into view
        // on that too (Tab moves report both; scrolling twice is one scroll).
        if response.has_focus() && self.focused_key.as_deref() != Some(key) {
            self.focused_key = Some(key.to_owned());
            self.reveal(response);
        }
        if response.gained_focus() {
            self.reveal(response);
        }
        if !response.enabled() {
            return;
        }
        if self.restore_key.as_deref() == Some(key) {
            self.restore_key = None;
            self.focus_screen = false;
            response.request_focus();
            self.reveal(response);
        } else if first && self.focus_screen && self.restore_key.is_none() {
            self.focus_screen = false;
            response.request_focus();
        }
    }

    /// Scrolls an item into view, clear of the chrome content passes beneath.
    fn reveal(&self, response: &Response) {
        let mut clear = response.clone();
        clear.rect = clear.rect.with_min_y(clear.rect.top() - self.scroll_inset);
        clear.scroll_to_me(None);
    }

    /// Gives the screen's first item focus again (after a stream or a dialog).
    pub fn refocus(&mut self) {
        self.focus_screen = true;
    }

    /// The events a controller's D-pad, stick or A (as `key`) gives egui. With nothing focused (the
    /// window just opened, a click on blank space, B, which drops egui's focus), egui moves nowhere
    /// and the press was lost: it puts focus on the open dialog's or the page's first item instead,
    /// with Tab for a page that names none (egui then focuses its first widget).
    pub fn controller_key(&mut self, ctx: &egui::Context, key: Key) -> Vec<egui::Event> {
        if key == Key::Escape {
            self.pad_back = true;
        }
        let lost = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Enter)
            && ctx.memory(|m| m.focused().is_none())
            && !egui::Popup::is_any_open(ctx);
        if !lost {
            return crate::gui::tap(key).to_vec();
        }
        if matches!(self.dialog, Dialog::None) {
            self.focus_screen = true;
        } else {
            self.focus_dialog = true;
        }
        ctx.request_repaint();
        crate::gui::tap(Key::Tab).to_vec()
    }

    /// Opens `dialog` with focus on its first control.
    pub fn ask(&mut self, dialog: Dialog) {
        self.dialog = dialog;
        self.focus_dialog = true;
    }

    /// Opens `dialog` once the open one is closed: for an answer that arrives by itself, which must
    /// not take away what the player is reading or answering.
    pub fn ask_after(&mut self, dialog: Dialog) {
        if matches!(self.dialog, Dialog::None) {
            self.ask(dialog);
        } else {
            self.queued.push_back(dialog);
        }
    }

    /// The "Now playing on ..." that is open or waits, if any.
    pub fn moved_prompt(&self) -> Option<(u64, i32)> {
        std::iter::once(&self.dialog).chain(&self.queued).find_map(|dialog| match dialog {
            Dialog::NowPlaying { host, app, .. } => Some((*host, *app)),
            _ => None,
        })
    }

    /// The PC gave the game back: what said it moved now says that, and offers to resume it.
    pub fn game_came_back(&mut self, host: u64, app: i32) {
        self.correct_moved(host, app, || Dialog::GameCameBack { host, app });
    }

    /// The press failed and the game is another device's: what said it moved now says that, and
    /// offers to stream beside it.
    pub fn move_failed(&mut self, host: u64, app: i32) {
        self.correct_moved(host, app, || Dialog::MoveFailed { host, app });
    }

    fn correct_moved(&mut self, host: u64, app: i32, correction: impl Fn() -> Dialog) {
        let said = |dialog: &Dialog| matches!(dialog, Dialog::NowPlaying { host: h, app: a, .. } if (*h, *a) == (host, app));
        if said(&self.dialog) {
            self.ask(correction());
        }
        for dialog in self.queued.iter_mut().filter(|dialog| said(dialog)) {
            *dialog = correction();
        }
    }

    /// Opens the next waiting dialog, if one waits.
    pub(crate) fn next_queued(&mut self) -> bool {
        let next = self.queued.pop_front();
        let any = next.is_some();
        if let Some(dialog) = next {
            self.ask(dialog);
        }
        any
    }

    /// The profile a stream on the current network uses, None for the main settings.
    pub fn active_profile(&self) -> Option<&crate::config::Profile> {
        self.profiles.profile(self.network.kind(&self.profiles.home_wifi))
    }

    /// A PC's own settings, over the profile in use and the main settings.
    pub fn open_host_settings(&mut self, id: u64, host: &Host) {
        self.inherited_settings = self.profiles.settings(&self.settings, self.network.kind(&self.profiles.home_wifi));
        self.edit_overrides = host.record.settings.clone().unwrap_or_default();
        self.edit_settings = self.edit_overrides.apply(&self.inherited_settings);
        self.inherited_from = inherited_from(None, self.active_profile());
        self.edit_name = trf("{} settings", &[&host.display_name()]);
        self.go(Screen::HostSettings(id));
    }

    /// A game's own settings, over its PC's, the profile in use and the main settings.
    pub fn open_app_settings(&mut self, id: u64, host: &Host, app: nightjar_core::http::App) {
        self.inherited_settings = host.record.settings(&self.profiles.settings(&self.settings, self.network.kind(&self.profiles.home_wifi)));
        self.edit_overrides = host.record.app_settings(&app.uuid, app.id).cloned().unwrap_or_default();
        self.edit_settings = self.edit_overrides.apply(&self.inherited_settings);
        self.inherited_from = inherited_from(host.record.settings.as_ref(), self.active_profile());
        self.edit_name = trf("{} settings", &[&app.name]);
        self.editing_app = Some(app);
        self.go(Screen::AppSettings(id));
    }

    pub fn message(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.dialog = Dialog::Message {
            title: title.into(),
            body: body.into(),
        };
        self.focus_dialog = true;
    }

    /// Whether a link from the system can be shown now, in front of nothing the user is doing.
    pub fn can_open_link(&self) -> bool {
        matches!(self.dialog, Dialog::None) && !self.pin.as_ref().is_some_and(crate::pin_ui::PinLock::app_locked)
    }

    /// Asks about a link from the system, or refuses it. The link is not shown back: it holds a
    /// PIN and passphrase.
    pub fn open_link(&mut self, text: &str) {
        match crate::hosts::parse_link(text) {
            Ok(link) => self.ask(Dialog::ConfirmLink { link, shown: None }),
            Err(why) => self.message(tr("Could not pair"), tr(why)),
        }
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

    /// Back: Escape, the Back button or the controller's B button (`pad`).
    fn back(&mut self, commands: &mut Vec<Command>, pad: bool) {
        match &self.dialog {
            Dialog::None => match self.screen {
                // The end of the line: B asks to close Nightjar, so a controller alone can. A
                // keyboard has Alt+F4 and Escape stays harmless.
                Screen::Home => if pad { self.ask(Dialog::ConfirmExit) },
                Screen::Hosts => {
                    if !self.trail.is_empty() || self.root() == Screen::Home {
                        let root = self.root();
                        self.pop(root);
                    } else if pad {
                        self.ask(Dialog::ConfirmExit);
                    }
                }
                Screen::Apps(_) | Screen::Game { .. } | Screen::AllGames | Screen::HostDetails(_) | Screen::Files(_) | Screen::Help => {
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
                Screen::Profiles | Screen::Shortcuts | Screen::StreamHotkeys => self.leave_subpage(),
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
        #[cfg(any(test, feature = "demo-library"))]
        if let Some(menu) = self.demo_menu {
            self.demo_stream_menu(ui, menu);
            return commands;
        }
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
            if pin.up() {
                ui.painter().extend(self.ambient.paint(&ctx, backdrop::Want::Sky(backdrop::Moon::High)));
            }
            if pin.ui(ui, &pcs, &mut commands) {
                return commands;
            }
        }
        let sources: Vec<Source> = view.hosts.list.iter().map(Source::of).collect();
        self.home_ready = console::has_library(&sources);
        // With a menu open, Escape (B) only closes it: the menu closes itself as it is drawn.
        let menu_open = egui::Popup::is_any_open(&ctx);
        let escape = ctx.input(|i| i.key_pressed(Key::Escape)) && !menu_open;
        let pad = std::mem::take(&mut self.pad_back);
        // On a settings page and on All games, Escape leaves the search before the page.
        if escape && view.connecting.is_none() && view.waking.is_none() && !(matches!(self.dialog, Dialog::None) && (self.escape_search() || self.escape_library_search())) {
            self.back(&mut commands, pad);
        } else if view.connecting.is_none() && view.waking.is_none() {
            self.help_keys(&ctx, &mut commands);
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
        // Home's hero is known before anything is drawn: the backdrop and the chrome speak of it.
        let mut home_data = (self.screen == Screen::Home).then(|| home::HomeData::gather(self, &sources));
        if let Some(data) = &home_data {
            self.hero.clone_from(&data.hero);
        }
        // Home has nothing to say with no PCs: the first run is on PCs, where they are found.
        if self.screen == Screen::Home && view.hosts.list.is_empty() {
            self.go(Screen::Hosts);
        }
        let want = self.page_backdrop(&view, &sources);
        self.ambient.set_orbit(self.screen == Screen::Hosts && first_run::first_run_of(view.hosts).is_some() && view.hosts.discovering);
        ui.painter().extend(self.ambient.paint(&ctx, want));
        let margin = chrome::page_margin(full.width());
        let content = Rect::from_min_max(
            Pos2::new(full.left() + margin, full.top() + chrome::PAGE_TOP),
            Pos2::new(full.right() - margin, full.bottom() - 16.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
        let ui = &mut child;
        self.cards.clear();
        // A menu by its card leaves the page as it is (it holds the input itself); a dialog dims it.
        let menu = matches!(self.dialog, Dialog::HostOptions { .. } | Dialog::PcSheet { .. } | Dialog::GameMenu { .. } | Dialog::HomeView) && self.menu_anchor != Anchor::Center;
        let blocked = (!matches!(self.dialog, Dialog::None) && !menu) || view.connecting.is_some() || view.waking.is_some();
        // Screens take focus only when nothing covers them.
        let held_focus = self.focus_screen;
        if blocked {
            self.focus_screen = false;
        }
        self.settle_play(&view, &sources, &mut commands);
        let brand = self.top_bar(&ctx, &view, blocked, &mut commands);
        self.back_slot = None;
        // Every page scrolls whole beneath the chrome, so whatever is revealed rests below it.
        self.scroll_inset = chrome::PAGE_TOP;
        ui.add_enabled_ui(!blocked, |ui| {
            match self.screen {
                Screen::Home => {
                    let data = home_data.take().unwrap_or_else(|| home::HomeData::gather(self, &sources));
                    self.home(ui, &view, &sources, data, &mut commands)
                }
                Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.settings_page(ui, &view, &mut commands),
                screen => {
                    let to_top = self.chrome_to_top();
                    self.page_scrolled = page_scroll(ui, page_area(screen), to_top, |ui| {
                        self.header(ui, &view, &mut commands);
                        match screen {
                            Screen::AllGames => self.all_games(ui, &view, &mut commands),
                            Screen::Game { host, app } => self.game(ui, &view, host, app, &mut commands),
                            Screen::Hosts => self.hosts(ui, &view, &mut commands),
                            Screen::Apps(id) => self.apps(ui, &view, id, &mut commands),
                            Screen::HostDetails(id) => self.host_details(ui, &view, id, &mut commands),
                            Screen::Files(id) => self.files_page(ui, &view, id, &mut commands),
                            Screen::Profiles => self.profiles_page(ui, &mut commands),
                            Screen::Controllers => self.controllers_page(ui, view.pads, &mut commands),
                            Screen::Shortcuts => self.shortcuts_page(ui, &mut commands),
                            Screen::StreamHotkeys => self.stream_hotkeys_page(ui, &mut commands),
                            Screen::Help => self.help_page(ui, &view, &mut commands),
                            Screen::Home | Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => {}
                        }
                    });
                }
            }
            // Back, once the page it leaves is drawn: the next one starts in its own scroll area.
            if std::mem::take(&mut self.back_pressed) {
                self.back(&mut commands, false);
            }
            self.paint_brand(&ctx, brand, logo);
            // What rose over its neighbours, under the chrome and whatever covers the page.
            glass::flush_deferred(ui);
            let veil = self.ambient.veil(&ctx, full, chrome::brand_rect(full), self.page_scrolled);
            ui.painter().with_clip_rect(full).extend(veil);
        });
        if blocked {
            self.focus_screen = held_focus;
        } else if self.restore_key.take().is_some() {
            // The remembered item is gone (or not loaded yet): the first one takes focus.
            ctx.request_repaint();
        }
        // A or a screen reader's click in an open menu closes it as a mouse click does (egui only
        // closes on the pointer), so the next B leaves the page rather than closing a used menu.
        let activated = ctx.input(|i| i.key_pressed(Key::Enter)
            || i.events.iter().any(|e| matches!(e, egui::Event::AccessKitActionRequest(r) if r.action == egui::accesskit::Action::Click)));
        if menu_open && activated && egui::Popup::is_any_open(&ctx) {
            egui::Popup::close_all(&ctx);
        }
        self.dialogs(&ctx, &view, &mut commands);
        if let Some(c) = &view.connecting {
            self.connecting(&ctx, c, &mut commands);
        } else if let Some(host) = &view.waking {
            waking(&ctx, host, &mut commands);
        }
        self.button_hints(&ctx, &view);
        self.draw_toast(&ctx);
        commands
    }

    /// A dialog or a connecting or waking card is over the page.
    fn panel_is_up(&self, view: &View<'_>) -> bool {
        !matches!(self.dialog, Dialog::None) || view.connecting.is_some() || view.waking.is_some()
    }

    /// Focus that went up into the chrome brings the page back to its top.
    fn chrome_to_top(&mut self) -> ToTop {
        if std::mem::take(&mut self.page_to_top) {
            ToTop::Glide
        } else {
            ToTop::No
        }
    }

    /// What the page wants behind it: its game's ambient, the sky with its moon on a page of cards or
    /// rows. A page that belongs to what came before it keeps what shows.
    fn page_backdrop(&self, view: &View<'_>, sources: &[Source<'_>]) -> backdrop::Want {
        use backdrop::{Moon, Want};
        match self.screen {
            Screen::Home => match &self.hero {
                Some(hero) => home::ambient_for(view, hero.host, hero.app, &hero.title),
                None => Want::Sky(Moon::High),
            },
            Screen::Game { host, app } => {
                let game = sources.iter().find(|s| s.id == host).and_then(|s| s.library).and_then(|l| l.games.iter().find(|g| g.app_id == app));
                match game {
                    Some(game) => home::ambient_for(view, host, app, &game.title),
                    None => Want::Keep,
                }
            }
            // The game the PC runs lights its page; with none, what shows stays.
            Screen::Apps(id) => match view.hosts.get(id).and_then(|host| Some((host, running_app_id(host)?))) {
                Some((host, app)) => home::ambient_for(view, id, app as u32, &running_name(host)),
                None => Want::Keep,
            },
            Screen::AllGames => Want::Keep,
            Screen::Hosts => Want::Sky(first_run::first_run_of(view.hosts).map_or(Moon::Low, first_run::FirstRun::moon)),
            _ => Want::Sky(Moon::High),
        }
    }

    /// Whether the page leads back: anything below the top level.
    fn has_back(&self) -> bool {
        !matches!(self.screen, Screen::Home | Screen::Hosts | Screen::Settings | Screen::Help)
    }

    /// Under the chrome, below the top level: a row that leads back and names the page, with that
    /// page's actions at its right.
    fn header(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let title = match self.screen {
            Screen::Home | Screen::Hosts | Screen::Settings | Screen::Help => return,
            // The page itself is headed by the game's title, beside its cover.
            Screen::Game { .. } => String::new(),
            Screen::AllGames => tr("All games").to_owned(),
            Screen::Apps(id) | Screen::HostDetails(id) | Screen::Files(id) => view.hosts.get(id).map_or("", |h| h.display_name()).to_owned(),
            Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => self.edit_name.clone(),
            Screen::Profiles => tr("Profiles").to_owned(),
            Screen::Controllers => tr("Controller mappings").to_owned(),
            Screen::Shortcuts => tr("Keyboard shortcuts").to_owned(),
            Screen::StreamHotkeys => tr("Stream hotkeys").to_owned(),
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.set_min_height(chrome::BACK_SIZE);
            // Back names where it leads when that is a destination's page, so a screen reader tells
            // it from the destination of the same name in the row above.
            let said = match self.screen {
                Screen::Apps(_) if self.trail.last().is_none_or(|(s, _)| *s == Screen::Hosts) => en("Back to PCs"),
                Screen::Profiles | Screen::Controllers | Screen::Shortcuts | Screen::StreamHotkeys => en("Back to Settings"),
                _ => en("Back"),
            };
            let (slot, _) = ui.allocate_exact_size(Vec2::splat(chrome::BACK_SIZE), Sense::hover());
            if self.sticky_back(ui, slot, tr(said)).clicked() {
                self.back_pressed = true;
            }
            if !title.is_empty() {
                // A long name ends in an ellipsis before the page's actions.
                let room = ui.available_width() - self.header_width - 2.0 * space::M;
                ui.allocate_ui(vec2(room.max(0.0), chrome::BACK_SIZE), |ui| {
                    ui.add(egui::Label::new(RichText::new(title).font(semibold(chrome::PAGE_TITLE)).color(PAPER)).truncate());
                });
            }
            self.actions(ui, view, commands);
        });
        ui.add_space(space::L);
    }

    /// The destinations the top bar offers now: Home waits for a library, unless it is shown.
    fn offered_destinations(&self) -> Vec<Screen> {
        let marked = self.screen.destination();
        DESTINATIONS.iter().map(|(_, s)| *s).filter(|s| *s != Screen::Home || self.home_ready || marked == Screen::Home).collect()
    }

    /// L1 and R1: to the destination left or right of the current one, round from the last to the
    /// first. Not over a dialog or while a text field takes the keys.
    pub fn step_destination(&mut self, ctx: &egui::Context, forward: bool, commands: &mut Vec<Command>) {
        if !matches!(self.dialog, Dialog::None) || ctx.text_edit_focused() {
            return;
        }
        let offered = self.offered_destinations();
        let Some(at) = offered.iter().position(|s| *s == self.screen.destination()) else { return };
        let to = offered[if forward { (at + 1) % offered.len() } else { (at + offered.len() - 1) % offered.len() }];
        if to != self.screen {
            self.go_destination(to, commands);
            self.focus_screen = true;
        }
    }

    /// Opens destination `screen` as its top-bar button does.
    fn go_destination(&mut self, screen: Screen, commands: &mut Vec<Command>) {
        match screen {
            // As Start does: over a library, game or PC page, so Back returns to it and the
            // page keeps its check or transfers.
            Screen::Settings => {
                if let Some(save) = self.screen.save_command() {
                    commands.push(save);
                }
                self.open_settings();
            }
            // Home and PCs drop the trail, and with it a Files or PC page and its transfers.
            _ if self.transferring() => {
                self.dialog = Dialog::ConfirmLeave { to: screen };
                self.focus_dialog = true;
            }
            _ => self.leave_for(screen, commands),
        }
    }

    /// Moves to destination `to`, saving the page it leaves.
    pub(super) fn leave_for(&mut self, to: Screen, commands: &mut Vec<Command>) {
        if let Some(save) = self.screen.save_command() {
            commands.push(save);
        }
        self.switch(to);
    }

    /// Whether a file transfer runs, on a Files page or a PC page's shared folders.
    fn transferring(&self) -> bool {
        self.files.transferring() || self.folders.transferring()
    }

    /// The page's actions at the right edge of its row, registered left to right so Tab reaches
    /// them in reading order. What the row's title leaves them is said to it next frame.
    pub(super) fn actions(&mut self, ui: &mut Ui, view: &View<'_>, commands: &mut Vec<Command>) {
        let mut used = 0.0;
        if let Screen::Apps(id) = self.screen {
            if let Some(host) = view.hosts.get(id) {
                used = self.running_actions(ui, host, commands);
            }
        }
        if (used - self.header_width).abs() > 0.5 {
            self.header_width = used;
            ui.ctx().request_repaint();
        }
    }
}

/// A destination's icon in the top bar.
fn destination_icon(screen: Screen) -> Icon {
    match screen {
        Screen::Home => Icon::Gamepad,
        Screen::Hosts => Icon::Monitor,
        Screen::Settings => Icon::Gear,
        _ => Icon::Help,
    }
}

/// [`Launcher::inherited_from`] for a game (`pc` is its PC's own settings) or a PC (`pc` None).
pub fn inherited_from(pc: Option<&Overrides>, profile: Option<&crate::config::Profile>) -> Vec<(Overrides, String)> {
    let pc = pc.map(|o| (o.clone(), tr("Set for this PC").to_owned()));
    let profile = profile.map(|p| (p.settings.clone(), trf("From profile {}", &[&p.name])));
    pc.into_iter().chain(profile).collect()
}

/// Which scroll area a page keeps its place in. Each game has its own.
fn page_area(screen: Screen) -> Id {
    match screen {
        Screen::Game { host, app } => Id::unique(("game", host, app)),
        Screen::Home => Id::unique("home"),
        Screen::AllGames => Id::unique("all-games"),
        Screen::Hosts => Id::unique("hosts"),
        Screen::Apps(_) => Id::unique("apps"),
        Screen::HostDetails(_) => Id::unique("host-details"),
        Screen::Files(_) => Id::unique("files"),
        Screen::Profiles => Id::unique("profiles"),
        Screen::Controllers => Id::unique("controller-mapping"),
        Screen::Shortcuts => Id::unique("shortcuts"),
        Screen::StreamHotkeys => Id::unique("stream-hotkeys"),
        Screen::Help => Id::unique("help"),
        Screen::Settings | Screen::HostSettings(_) | Screen::AppSettings(_) | Screen::ProfileSettings(_) => Id::unique("settings"),
    }
}

#[cfg(test)]
mod glass_shots;
#[cfg(test)]
use settings::*;
#[cfg(test)]
mod tests;
