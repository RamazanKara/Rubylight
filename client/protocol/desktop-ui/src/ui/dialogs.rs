//! Dialogs and overlays: their drawing and buttons, a PC's options list, pairing and sleep
//! answers, and the waking and connecting cards.

use super::*;
use super::settings::{apply_preset, preset_label};
use nightjar_policy::http::{valid_pairing_pin, PairingRequest};

/// What the pairing help says, in the order of Nightjar Android's guide.
pub(super) const PAIRING_STEPS: [&str; 3] = [
    en("1. Start a Moonlight-compatible streaming app on the PC, such as Apollo, Rubylight, Sunshine or Vibepollo."),
    en("2. Here, choose the PC on the same network and select Pair."),
    en("3. Enter the PIN shown here in the PC’s settings or web UI, usually on its PIN or pairing page."),
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
            Self::QuitApp => en("Quit game"),
            Self::Pair => en("Pair"),
            Self::PairOtp => en("Pair with one-time PIN"),
            Self::Wake => en("Wake up"),
            Self::Sleep => en("Sleep PC"),
            Self::RestartPc => en("Restart PC"),
            Self::ShutDownPc => en("Shut down PC"),
            Self::UpdateHost => en("Update the PC’s streaming app"),
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
        if host.running_app().is_some() {
            options.push(HostOption::Resume);
        }
        if host.running_app().is_some() {
            options.push(HostOption::QuitApp);
        }
    } else {
        options.push(HostOption::Pair);
        if host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_policy::http::Feature::OtpPairing)) {
            options.push(HostOption::PairOtp);
        }
    }
    // Not reached yet counts too: a PC that is asleep has not answered since this started.
    if host.reach != Reach::Online && host.record.can_wake() {
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
        && host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_policy::http::Feature::MicOnly))
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

/// The PC's one-time notice before frame generation's first install.
const NEURAL_NOTICE: &str = en("Frame generation puts a third-party mod into the game’s folder, where the game loads it in place of its own DLSS frame generation. The PC downloads it from its authors’ GitHub release; it is not made or endorsed by AMD or NVIDIA.\n\nThe PC refuses games with a known anti-cheat in their install and games Steam lists Valve Anti-Cheat for, but an online game may still treat a mod as cheating. Use it at your own risk.");

/// A caller without List or View is told that its stream moved, not where to.
pub(super) fn now_playing_title(device: &str) -> String {
    if device.is_empty() {
        tr("Now playing on another device").into()
    } else {
        trf("Now playing on {}", &[&device])
    }
}


/// Per-frame state threaded through dialog drawing helpers.
pub(super) struct DialogFlow {
    pub(super) close: bool,
    pub(super) next: Option<Dialog>,
    pub(super) leave: Option<Screen>,
    pub(super) focus_first: bool,
    pub(super) focused: bool,
    pub(super) bitrate_kbps: u32,
    pub(super) keys_on_screen: bool,
}

impl Launcher {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context, view: &View<'_>, commands: &mut Vec<Command>) {
        if matches!(self.dialog, Dialog::None) {
            if self.next_queued() {
                ctx.request_repaint();
            }
            return;
        }
        let tested = match &self.dialog {
            Dialog::ConnectionTest { host, .. } => Some(*host),
            _ => None,
        };
        let mut flow = DialogFlow {
            close: false,
            next: None,
            leave: None,
            focus_first: self.focus_dialog,
            focused: false,
            bitrate_kbps: self.stream_bitrate_kbps(view.hosts, tested),
            keys_on_screen: self.wants_text_pad(),
        };
        let mut dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        if !matches!(dialog, Dialog::ConnectFailed { .. }) {
            let _ = self.draw_host_dialog(ctx, view, commands, &mut dialog, &mut flow)
                || self.draw_pair_dialog(ctx, view, commands, &mut dialog, &mut flow)
                || self.draw_session_dialog(ctx, view, commands, &mut dialog, &mut flow);
        }
        self.dialog = dialog;
        if flow.focused {
            self.focus_dialog = false;
        }
        if let Some(dialog) = flow.next {
            self.dialog = dialog;
            self.focus_dialog = true;
        } else if flow.close {
            // The card goes away with the PC, so only then is there nothing to give focus back
            // to; Cancel keeps it.
            if matches!(self.dialog, Dialog::ConfirmRemove { .. }) && commands.iter().any(|c| matches!(c, Command::Remove(_))) {
                self.focused_key = None;
            }
            self.close_dialog();
        }
        if let Some(to) = flow.leave {
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
    pub fn hotkey_input(&mut self, input: crate::platform_hooks::HotkeyInput) -> Option<Command> {
        use crate::platform_hooks::HotkeyInput;
        let Dialog::StreamHotkey { id, error } = &mut self.dialog else { return None };
        let keys = match input {
            HotkeyInput::Cancel => {
                self.close_dialog();
                return None;
            }
            HotkeyInput::Navigate | HotkeyInput::ModifierOnly => return None,
            HotkeyInput::Unsupported { name } => {
                *error = Some(trf("Unsupported key: {}", &[&name]));
                return None;
            }
            HotkeyInput::Recorded(keys) => keys,
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
                | Dialog::PcSheet { .. }
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
                if let Some(app) = resume_target(host) {
                    commands.push(Command::Launch { host: id, app, resume: true });
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
            Ok(SleepStep::Accepted) => self.sleep_toast(host, trf("{} is going to sleep", &[&name])),
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
            (Ok(PcPowerStep::Accepted), PcPower::Restart) => self.sleep_toast(host, trf("{} is restarting", &[&name])),
            (Ok(PcPowerStep::Accepted), PcPower::ShutDown) => self.sleep_toast(host, trf("{} is shutting down", &[&name])),
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
        match result {
            Ok(UpdateStep::Started(_)) => self.sleep_toast(host, trf("Updating {}", &[&name])),
            Ok(UpdateStep::Waiting(reason)) => self.sleep_toast(host, trf("{} The update starts once that stream ends.", &[&reason])),
            Ok(UpdateStep::Back(version)) => self.sleep_toast(host, trf("{} now runs version {}", &[&name, &version])),
            Err(e) => self.sleep_message(host, trf("Could not update {}", &[&name]), e),
        }
    }

    /// A request the PC took: a notice that asks for nothing, closing only the question it answers.
    pub(super) fn sleep_toast(&mut self, host: u64, text: String) {
        if matches!(&self.dialog, Dialog::ConfirmSleep { host: asked, .. } | Dialog::ConfirmPcPower { host: asked, .. } if *asked == host) {
            self.close_dialog();
        }
        self.toast(text);
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

    /// Whether text fields get [`text_pad`]: a controller is in the hand and the system shows no
    /// keyboard on screen.
    pub(super) fn wants_text_pad(&self) -> bool {
        !self.screen_keyboard && matches!(self.input_source, crate::input_source::InputSource::Pad(_))
    }

    /// The bitrate a stream to PC `host` asks for, worked out as a launch does: the profile in use,
    /// then the PC's own settings. The one set, else the default for their size and rate (1080p
    /// and 60 when they follow the screen).
    fn stream_bitrate_kbps(&self, hosts: &Hosts, host: Option<u64>) -> u32 {
        let mut s = self.profiles.settings(&self.settings, self.network.kind(&self.profiles.home_wifi));
        if let Some(host) = host.and_then(|id| hosts.get(id)) {
            s = host.record.settings(&s);
        }
        if s.bitrate_kbps != 0 {
            return s.bitrate_kbps;
        }
        let (width, height) = if s.width == 0 || s.height == 0 { (1920, 1080) } else { (s.width, s.height) };
        crate::config::default_bitrate_kbps(width, height, if s.fps == 0 { 60 } else { s.fps })
    }

    /// Shows why pairing failed, with a way to try again.
    pub fn pair_failed(&mut self, host: u64, body: impl Into<String>, one_time: bool) {
        self.dialog = Dialog::PairError { host, body: body.into(), one_time };
        self.focus_dialog = true;
    }
}

mod host_dialogs;
mod menus;
mod pair_dialogs;
mod session_dialogs;

pub use menus::{pc_sheet, popover, Anchor, MenuOutcome, MenuRow};
#[cfg(test)]
pub use menus::place;
pub(super) use menus::{
    approve_dialog, classic_of, dialog_buttons, dialog_field, game_menu, hide_keys_of,
    hover_takes_focus, open_game_of, open_on_start_action, short_device_name, waking,
};
#[cfg(test)]
pub(super) use menus::MENU_FADE;
use menus::{asking_line, next_of, pin_pad, short_window, stored_favourite, two_rows, PIN_LENGTH};
