//! Public UI command, screen, dialog, and frame view types shared across launcher pages.

use super::*;

/// Where Nightjar's guides live, the pages Android's help opens.
pub const GUIDES: &str = "https://github.com/RamazanKara/nightjar/blob/main/docs/";

pub enum Command {
    /// The game a cold start goes on to, chosen in its menu; None takes it back.
    OpenOnStart(Option<crate::config::OpenGame>),
    /// "Start Nightjar when I sign in".
    SetSignIn(bool),
    /// The SteamGridDB and IGDB keys of Settings → Advanced, to keep and use.
    SetGameInfoKeys(nightjar_policy::game_info::job::Keys),
    /// Look up the facts of these games (only the ones not asked for yet are).
    WantFacts(Vec<nightjar_policy::game_info::job::Game>),
    Launch { host: u64, app: i32, resume: bool },
    /// A launch with nobody to ask: without [`Command::Launch`]'s read of who plays.
    LaunchUnasked { host: u64, app: i32, resume: bool },
    /// Input-only through a small ordinary stream, on a PC without its own Input-only mode.
    LaunchInputOnly { host: u64, app: i32, resume: bool },
    /// Only this device's microphone, on a PC that offers it (`Feature::MicOnly`).
    MicOnly { host: u64 },
    SetQuitOnLeave { host: u64, uuid: String, id: i32, enabled: bool },
    QuitApp { host: u64, named: Option<i32> },
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
    /// Open an invite the user confirmed: reach the PC over the internet and pair as a guest.
    RedeemInvite(Box<rubylight_protocol::invite_uri::InviteUri>),
    ApprovePairing {
        host: u64,
        request: String,
        name: String,
        pin: String,
    },
    DenyPairing {
        host: u64,
        request: String,
        name: String,
    },
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
    /// Give back the USB devices Nightjar left shared in usbipd-win when it stopped (Windows).
    GiveUsbDevicesBack,
    /// Name a PC on this device only; empty brings back the PC's own name.
    Rename { host: u64, alias: String },
    /// Always connect to the PC through this known address; None brings back the automatic pick.
    PinAddress { host: u64, address: Option<String> },
    /// The MACs the user added to wake the PC by (`wol::parse_macs`); empty removes them.
    WakeAddresses { host: u64, macs: Vec<String> },
    HostSettings(u64),
    SaveHostSettings(u64),
    AppSettings { host: u64, app: nightjar_policy::http::App },
    SaveAppSettings(u64),
    /// Every PC and game follows its profile or the main settings again (`Dialog::KeptSettings`).
    ResetKeptSettings,
    ProfileSettings(u64),
    SaveProfile(u64),
    SaveProfiles,
    NetworkTest { host: u64, calibrate: bool },
    CancelNetworkTest,
    ClearPace(u64),
    /// Full chroma was switched on a settings page: every PC that refused it is asked again, and
    /// a start without it says so again (Android's `HostPackedModes.clearRefusals`).
    AskFullChromaAgain,
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
    /// Settings → App → Check for updates now: GitHub's latest release, asked once, now.
    CheckUpdatesNow,
    /// Settings → App → Report a problem: the redacted logs saved as one report in the logs folder,
    /// which then opens (Android's `ProblemReport`, which shares it instead).
    ReportProblem,
    /// About's Source and licenses: Nightjar's repository in the default browser.
    OpenSource,
    /// Settings → Advanced → Export latency CSV: the last stream's measurements, saved where the
    /// system's file dialog says.
    ExportLatencyCsv,
    /// The latency benchmark page's Start: the 10-second timing test and the decoder benchmark.
    StartLatencyBenchmark,
    /// The latency benchmark page's Stop, or leaving the page.
    StopLatencyBenchmark,
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
    /// Open a game page's Neural Rendering: the PC, the library app id and its install id.
    Neural {
        host: u64,
        game: u32,
        app: String,
    },
    CancelPair,
    /// An address, or a QR pairing link to add and pair with (see `hosts::parse_entry`).
    AddHost(String),
    /// Add PC from a pairing link on the clipboard.
    AddFromClipboard,
    Unpair(u64),
    Remove(u64),
    Refresh(u64),
    /// Reads the PC's app list again, also one that came without the game.
    ReadAppsAgain(u64),
    OpenApps(u64),
    /// Fetch a cover that scrolled into view (a new `version` fetches again).
    RequestArt { host: u64, app: i32, version: String },
    /// Close the running game through the PC's native API.
    CloseGame(u64),
    SaveSettings,
    CancelConnect,
    /// The first run: start looking for PCs on the network again, after it could not start.
    SearchAgain,
    /// The failed start's card: the same start again.
    RetryLaunch,
    /// The failed start's card: test whether this network lets a stream's ports out.
    TestConnection,
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
    Power(crate::power_types::Offer),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Settings → App → About: the version, the licence and the credits (`about`).
    About,
    /// Settings → Advanced → Latency benchmark: this device's display, input, timer and decoder
    /// timing without a PC (`latency_bench`).
    LatencyBenchmark,
}

impl Screen {
    /// The top-level destination this page belongs to, marked in the top bar.
    pub fn destination(self) -> Screen {
        match self {
            Self::Home | Self::AllGames | Self::Game { .. } => Self::Home,
            Self::Hosts | Self::Apps(_) | Self::HostDetails(_) | Self::Files(_) | Self::HostSettings(_) | Self::AppSettings(_) => Self::Hosts,
            Self::Settings | Self::Profiles | Self::ProfileSettings(_) | Self::Controllers | Self::Shortcuts | Self::StreamHotkeys
            | Self::About | Self::LatencyBenchmark => Self::Settings,
            Self::Help => Self::Help,
        }
    }

    /// How often the network is read while this page is open. Every page takes its profile from
    /// the kind of network, which the system tells of when it changes; these two also show the
    /// link's speed or keep it with a test's result, and a Wi-Fi link's speed moves on its own.
    pub fn network_pace(self) -> crate::network::Pace {
        match self {
            Self::Profiles | Self::HostSettings(_) => crate::network::Pace::Live,
            Self::Home | Self::AllGames | Self::Game { .. } | Self::Hosts | Self::Apps(_) | Self::HostDetails(_) | Self::Files(_)
            | Self::Settings | Self::AppSettings(_) | Self::ProfileSettings(_) | Self::Controllers | Self::Shortcuts
            | Self::StreamHotkeys | Self::Help | Self::About | Self::LatencyBenchmark => crate::network::Pace::Idle,
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
    ConfirmLink {
        link: Box<rubylight_protocol::pairing_uri::PairingUri>,
        shown: Option<f64>,
    },
    /// An invite to play over the internet the system opened, or one pasted: asked about the same
    /// way, since any web page or message can carry one.
    ConfirmInvite {
        link: Box<rubylight_protocol::invite_uri::InviteUri>,
        shown: Option<f64>,
    },
    /// An invite being opened: the PC is looked for over the internet, which takes up to a minute.
    Inviting {
        name: String,
    },
    /// A network test running: Connection test, or PyroWave's frame-pace calibration with
    /// `calibrate`. `stage` is in English; the dialog translates it.
    NetworkTest { stage: String, calibrate: bool },
    /// Connection test's result (`network_test::Connection`): the delay, the speed or why it was
    /// not measured, and Try again.
    ConnectionTest { host: u64, result: crate::network_test::Connection },
    /// Settings' Presets: this preset replaces the stream settings it sets, once confirmed.
    ConfirmPreset(nightjar_policy::presets::Preset),
    Otp { host: u64, pin: String, passphrase: String, error: Option<String> },
    Message { title: String, body: String },
    /// The countdown of an automatic start (`open_play`): closing it any way cancels the start.
    OpenPlay { game: String, pc: String, left: u64 },
    /// Quit the running app, then start `start`. `resume` is the running app's id when the
    /// dialog can offer to carry on with it instead.
    ConfirmQuit { host: u64, start: Option<(i32, String)>, running: String, resume: Option<i32>, named: Option<i32> },
    ConfirmAction { host: u64, app: i32, name: String },
    /// The PC's one-time notice before frame generation's first install in a game (library app
    /// id `game`): Accept turns it on, anything else leaves it off.
    ConfirmNeural { host: u64, game: u32 },
    /// The PC refused to start `app` until the user confirms closing `running`. Closing the
    /// dialog any other way than confirming cancels the launch.
    ConfirmReplace { app: String, running: String },
    /// `name` runs on `device`: move it here, stream it here too, or neither. `join`: the PC makes
    /// the second stream a guest of `device`, which lets it in first, so the button says Join.
    ContinueHere { host: u64, app: i32, resume: bool, name: String, device: String, join: bool },
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
    /// The PC's own wake addresses being edited; `invalid` once Save found one that is not a MAC.
    WakeAddresses { host: u64, text: String, invalid: bool },
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
    /// After a crash: Nightjar left `count` USB devices shared in usbipd-win. Give back asks
    /// Windows for administrator rights; Not now leaves them to the stream menu's USB section.
    ConfirmUsbGiveBack { count: usize },
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
    ConfirmPower(crate::power_types::Offer),
    /// Shown once after updating: the PCs that keep settings of their own or for games, one
    /// line each, and a reset of them all (`Launcher::show_kept_settings_once`).
    KeptSettings { pcs: Vec<KeptPc> },
    /// A start that failed before its first frame: the launch scene's card, with the start's
    /// stream settings in a line and the network test it offers (`launch_scene`).
    ConnectFailed { launch: ConnectingView, body: String, settings: Option<String>, test: launch_scene::CardTest },
}

/// The header's top-level destinations, left to right.
pub const DESTINATIONS: [(&str, Screen); 4] =
    [(en("Home"), Screen::Home), (en("PCs"), Screen::Hosts), (en("Settings"), Screen::Settings), (en("Help"), Screen::Help)];

/// The launch scene's content: the game, its PC and the stage the start is at. The PC's id and the
/// app's id in its list find the cover and its ambient.
#[derive(Clone)]
pub struct ConnectingView {
    pub app: String,
    pub host: String,
    pub stage: String,
    pub host_id: u64,
    pub app_id: i32,
}

/// What the launcher shows, borrowed from the app each frame.
pub struct View<'a> {
    pub hosts: &'a Hosts,
    pub pads: &'a crate::pads_view::PadsView,
    pub connecting: Option<ConnectingView>,
    /// The name of the PC being woken.
    pub waking: Option<String>,
}
