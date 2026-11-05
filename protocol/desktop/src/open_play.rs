//! "When Nightjar opens" on the desktop: the countdown into a game, its start through the launcher's
//! own wake and launch, and the guard (`nightjar_policy::open_play`) that keeps a start which comes to
//! nothing from trapping the player. Also runs the sign-in setting (`autostart`) off the UI thread.

use std::fs::File;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::session::{Owner, Session};
use nightjar_policy::open_play::{self as policy, Action, Entry, Facts, Game};

use crate::autostart;
use crate::config::{OpenGame, OpenOnStart, Recent, Settings, Store};
use crate::console;
use crate::hosts::{Apps, Hosts, Reach};
use crate::self_update;
use crate::strings::{tr, trf};
use crate::ui::{Command, Dialog, Launcher};

/// How long the PC gets to answer and its app list to load, outside a wake's own wait.
const START_LIMIT: Duration = Duration::from_secs(45);

/// How long a sign-in start waits for the network before it opens as usual.
const NETWORK_LIMIT: Duration = Duration::from_secs(60);

/// After the network came back: longer than a poll, so the countdown never names a PC not yet heard from.
const NETWORK_SETTLE: Duration = Duration::from_secs(6);

/// The game an automatic start goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: u64,
    pub pc: String,
    pub game: OpenGame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The PIN screen or a dialog from before comes first.
    Hold,
    Counting { until: Instant },
    /// `shown`: the dialog with its Cancel stands in front, except during a wake, which has its own.
    Starting { wake_asked: bool, apps_asked: bool, since: Instant, shown: bool },
    /// The launch was asked for: the stream comes, or a prompt or the PIN stands in front of it.
    Launched,
    /// `since`: when the stream came up, without a break since; None while it reconnects.
    Streaming { since: Option<Instant> },
}

/// The app list as far as a start cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppsSeen {
    NotLoaded,
    Loading,
    Failed(String),
    Loaded(Option<i32>),
}

/// Whose game the PC is streaming, as far as a start cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSeen {
    /// A read that says is still to come.
    Waiting,
    /// Nothing of another device's, or a PC without the native API, which cannot tell (a host refuses a
    /// start it may not make).
    Free,
    /// Another device is playing there: an automatic start never moves that.
    Elsewhere,
}

impl SessionSeen {
    /// `read_failed`: the latest read did not answer, so a free session from before is old news. The
    /// next poll reads again, within the start's limit.
    fn of(native_api: bool, session: Option<&Session>, read_failed: bool) -> Self {
        match session {
            _ if !native_api => Self::Free,
            Some(s) if s.owner == Owner::OtherDevice && console::active(s) => Self::Elsewhere,
            _ if read_failed => Self::Waiting,
            Some(_) => Self::Free,
            None => Self::Waiting,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSeen {
    pub reach: Reach,
    pub can_wake: bool,
    pub paired: bool,
    pub apps: AppsSeen,
    pub session: SessionSeen,
}

/// What the launcher shows now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub pin_screen: bool,
    pub no_dialog: bool,
    pub countdown: bool,
    pub message: bool,
    pub identity: bool,
    pub host: Option<HostSeen>,
    pub waking: bool,
    /// Only looked at before the countdown.
    pub network: bool,
    /// A launch, connection, PIN or prompt is under way.
    pub busy: bool,
    pub streaming: bool,
    pub left_by_player: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// The network is back: ask the PC now rather than at the next poll.
    Repoll,
    /// Show the countdown with this many seconds left; 0 once it ran out and the start is under way.
    Count(u64),
    /// The countdown ran out: the guard is armed from here.
    Begin,
    Wake,
    LoadApps,
    Start(i32),
    /// Nothing failed, the start just must not happen: one line says why, and the guard is off.
    Stopped(String),
    /// The start failed; a message says why unless one is up already. The guard stays armed.
    Failed(Option<String>),
    /// It is over; `disarm` says it ended well or was the player's doing.
    Over { disarm: bool },
}

/// One automatic start, one step per frame.
pub struct Run {
    pub target: Target,
    phase: Phase,
    /// Started by the system at sign-in, which may be ahead of the network and of the PC.
    at_sign_in: bool,
    network_down_since: Option<Instant>,
    settled_at: Option<Instant>,
}

impl Run {
    pub fn new(target: Target) -> Self {
        Self { target, phase: Phase::Hold, at_sign_in: false, network_down_since: None, settled_at: None }
    }

    /// Whether the guard is armed for this run: past the countdown.
    pub fn armed(&self) -> bool {
        !matches!(self.phase, Phase::Hold | Phase::Counting { .. })
    }

    /// Before the countdown, while the network still counts.
    fn holding(&self) -> bool {
        self.phase == Phase::Hold
    }

    pub fn next(&mut self, seen: &Seen, now: Instant) -> Step {
        match self.phase {
            Phase::Hold => {
                // The player got there first, and nothing is armed yet.
                if seen.busy || seen.streaming {
                    return Step::Over { disarm: false };
                }
                if seen.pin_screen || !seen.no_dialog {
                    return Step::Wait;
                }
                if !seen.network {
                    let down_since = *self.network_down_since.get_or_insert(now);
                    if self.at_sign_in && now.saturating_duration_since(down_since) < NETWORK_LIMIT {
                        return Step::Wait;
                    }
                    return Step::Stopped(tr("No network, so Nightjar opened as usual.").to_owned());
                }
                if self.network_down_since.take().is_some() {
                    self.settled_at = Some(now + NETWORK_SETTLE);
                    return Step::Repoll;
                }
                if self.settled_at.is_some_and(|at| now < at) {
                    return Step::Wait;
                }
                self.phase = Phase::Counting { until: now + Duration::from_millis(policy::COUNTDOWN_MS) };
                Step::Count(policy::COUNTDOWN_MS.div_ceil(1000))
            }
            Phase::Counting { until } => {
                // Cancel, B, Back, Escape, or anything that took the dialog's place.
                if !seen.countdown {
                    return Step::Over { disarm: false };
                }
                if now >= until {
                    self.phase = Phase::Starting { wake_asked: false, apps_asked: false, since: now, shown: true };
                    return Step::Begin;
                }
                Step::Count((until - now).as_millis().div_ceil(1000) as u64)
            }
            Phase::Starting { .. } => self.starting(seen, now),
            Phase::Launched => {
                if seen.streaming {
                    self.phase = Phase::Streaming { since: Some(now) };
                    Step::Wait
                } else if seen.message {
                    Step::Failed(None)
                } else if seen.busy || !seen.no_dialog {
                    Step::Wait
                } else {
                    // Closed without a stream or a word: the PIN or a prompt was cancelled.
                    Step::Over { disarm: true }
                }
            }
            Phase::Streaming { since } => {
                let lasted = since.map_or(0, |since| now.saturating_duration_since(since).as_millis() as u64);
                if seen.streaming {
                    // Only a stream that is up now can have lasted: the clock starts over after a reconnect.
                    if since.is_none() {
                        self.phase = Phase::Streaming { since: Some(now) };
                        Step::Wait
                    } else if lasted >= policy::SETTLED_MS {
                        Step::Over { disarm: true }
                    } else {
                        Step::Wait
                    }
                } else if seen.busy {
                    self.phase = Phase::Streaming { since: None };
                    Step::Wait
                } else {
                    Step::Over { disarm: policy::stream_end_disarms(lasted, seen.left_by_player) }
                }
            }
        }
    }

    fn starting(&mut self, seen: &Seen, now: Instant) -> Step {
        let Phase::Starting { wake_asked, apps_asked, since, shown } = self.phase else { return Step::Wait };
        let pc = self.target.pc.clone();
        // The player got there first: this run must not start a game on top of it.
        if seen.busy || seen.streaming {
            return Step::Over { disarm: true };
        }
        if seen.message {
            return Step::Failed(None);
        }
        let Some(host) = &seen.host else {
            return Step::Failed(Some(trf("{} is no longer saved.", &[&pc])));
        };
        if seen.waking {
            // The wake has its own limit and its own message.
            self.phase = Phase::Starting { wake_asked, apps_asked, since: now, shown };
            return Step::Wait;
        }
        // Cancel, B, Back or Escape while the PC answers and its list loads.
        if shown && !seen.countdown {
            return Step::Over { disarm: true };
        }
        let late = now.saturating_duration_since(since) > START_LIMIT;
        let in_time = |step: Step| if late { Step::Failed(Some(trf("{} did not answer in time.", &[&pc]))) } else { step };
        if !seen.identity {
            return in_time(Step::Wait);
        }
        match host.reach {
            Reach::Unknown => in_time(Step::Wait),
            Reach::Offline if wake_asked => {
                // The wait is over without the PC or a message: the player stopped it.
                Step::Over { disarm: true }
            }
            Reach::Offline if host.can_wake => {
                self.phase = Phase::Starting { wake_asked: true, apps_asked, since: now, shown: false };
                Step::Wake
            }
            // Right after sign-in the PC may simply not have been reached yet.
            Reach::Offline if self.at_sign_in => in_time(Step::Wait),
            Reach::Offline => Step::Failed(Some(trf("{} is offline and has no address to wake it at.", &[&pc]))),
            Reach::Online if !host.paired => Step::Failed(Some(trf("{} is not paired.", &[&pc]))),
            Reach::Online if host.session == SessionSeen::Elsewhere => {
                Step::Stopped(trf("A game on {} is playing on another device, so Nightjar opened as usual.", &[&pc]))
            }
            Reach::Online if !shown => {
                // Back from the wake: Cancel again for the rest of the way.
                self.phase = Phase::Starting { wake_asked, apps_asked, since, shown: true };
                Step::Count(0)
            }
            Reach::Online if host.session == SessionSeen::Waiting => in_time(Step::Wait),
            Reach::Online => match &host.apps {
                AppsSeen::NotLoaded if !apps_asked => {
                    self.phase = Phase::Starting { wake_asked, apps_asked: true, since, shown };
                    Step::LoadApps
                }
                AppsSeen::NotLoaded => in_time(Step::Wait),
                AppsSeen::Loading => {
                    // A list that drops back to unloaded later is asked for again.
                    self.phase = Phase::Starting { wake_asked, apps_asked: false, since, shown };
                    in_time(Step::Wait)
                }
                AppsSeen::Failed(why) => Step::Failed(Some(trf("The app list could not be loaded. {}", &[why]))),
                AppsSeen::Loaded(None) => Step::Failed(Some(tr("The PC's app list has no entry for this game.").to_owned())),
                AppsSeen::Loaded(Some(app)) => {
                    self.phase = Phase::Launched;
                    Step::Start(*app)
                }
            },
        }
    }
}

/// What a start of Nightjar does: the run to make, and the one line to say when it skips one.
pub fn at_start(store: &Store, settings: &Settings, recents: &[Recent], launcher_start: bool, host_of: impl Fn(&str) -> Option<(u64, String)>) -> (Option<Run>, Option<(String, String)>) {
    let mut state = store.open_play();
    let target_of = |game: &OpenGame| host_of(&game.host).map(|(host, pc)| Target { host, pc, game: game.clone() });
    let last = recents.first().map(|r| OpenGame { host: r.host.clone(), install_id: r.install_id.clone(), app_id: r.app_id, title: r.title.clone() }).as_ref().and_then(target_of);
    let chosen = state.game.as_ref().and_then(target_of);
    let decision = policy::decide(Facts {
        entry: Entry { first_screen: true, launcher: launcher_start, restored: false, from_recents: false },
        opens: settings.open_on_start.opens(),
        has_last: last.is_some(),
        has_chosen: chosen.is_some(),
        armed: state.armed,
    });
    if decision.disarm {
        state.armed = false;
        if let Err(e) = store.save_open_play(&state) {
            eprintln!("nightjar: saving the automatic start failed: {e}");
        }
    }
    let target = |game: Game| match game {
        Game::Last => last.clone(),
        Game::Chosen => chosen.clone(),
    };
    match decision.action {
        Action::Stay => (None, None),
        Action::Countdown(game) => (target(game).map(Run::new), None),
        Action::Skip(game) => {
            let title = target(game).map_or(String::new(), |t| t.game.title);
            (None, Some((tr("Automatic start skipped").to_owned(), trf("{} didn't start last time, so Nightjar opened as usual.", &[&title]))))
        }
    }
}

/// What the command line said about who started this copy.
#[derive(Clone, Copy, Debug, Default)]
pub struct Started {
    pub at_sign_in: bool,
    pub after_setup: bool,
}

/// Whether this start of Nightjar may start a game by itself. An update's restart and a second copy
/// are not the player opening Nightjar, and a second copy must not touch what the first one runs.
fn may_start_a_game(launcher_start: bool, after_setup: bool, stream_elsewhere: bool, window_elsewhere: bool) -> bool {
    launcher_start && !after_setup && !stream_elsewhere && !window_elsewhere
}

/// How often a copy without the claim asks for it again.
const CLAIM_RETRY: Duration = Duration::from_secs(1);

/// This copy's claim on the config folder, held for as long as it runs; `Ok(None)` when another copy
/// holds it. The system lets go of it however a copy ends, so a crash leaves nothing behind.
#[cfg(windows)]
fn claim_window(dir: &Path) -> std::io::Result<Option<File>> {
    use std::os::windows::fs::OpenOptionsExt;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    match File::options().create(true).truncate(false).write(true).share_mode(0).open(dir.join("window.lock")) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(not(windows))]
fn claim_window(dir: &Path) -> std::io::Result<Option<File>> {
    use std::os::fd::AsRawFd;
    let file = File::options().create(true).truncate(false).write(true).open(dir.join("window.lock"))?;
    // SAFETY: the descriptor is open for the whole call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let e = std::io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(None)
    } else {
        Err(e)
    }
}

/// Sets up a start of Nightjar: decides the run and gives the settings page and game menu their state.
pub fn at_launch(store: &Store, launcher: &mut Launcher, hosts: &Hosts, launcher_start: bool, started: Started) -> OpenPlay {
    let window = claim_window(store.dir());
    let window_elsewhere = !matches!(window, Ok(Some(_)));
    // No way to know it is alone, and starting a game unasked is the worse mistake: the player hears why.
    let claim_notice = window.as_ref().err().and_then(|e| {
        eprintln!("nightjar: checking for another window failed: {e}");
        claim_failed(may_start_a_game(launcher_start, started.after_setup, false, false) && launcher.settings.open_on_start != OpenOnStart::Pcs, e)
    });
    let launcher_start = may_start_a_game(launcher_start, started.after_setup, self_update::mutex_exists(self_update::STREAM_MUTEX), window_elsewhere);
    let at_sign_in = started.at_sign_in;
    let host_of = |unique: &str| {
        (!unique.is_empty()).then(|| hosts.list.iter().find(|h| h.record.unique_id == unique)).flatten().map(|h| (h.id, h.display_name().to_owned()))
    };
    let (run, notice) = at_start(store, &launcher.settings, &launcher.recents, launcher_start, host_of);
    let run = run.map(|run| Run { at_sign_in, ..run });
    let sign_in = autostart::system(store.dir());
    let mut open_play = OpenPlay::new(run, notice.or(claim_notice), sign_in);
    open_play._window = window.ok().flatten();
    open_play.claim_at = open_play._window.is_none().then(|| Instant::now() + CLAIM_RETRY);
    launcher.open_game = store.open_play().game;
    launcher.sign_in = open_play.sign_in_now();
    open_play
}

/// The one line for a claim that failed, which stops a start this copy would otherwise have made.
fn claim_failed(would_start: bool, why: &std::io::Error) -> Option<(String, String)> {
    would_start.then(|| (tr("Automatic start skipped").to_owned(), trf("Nightjar could not check for another open window, so it opened as usual. {}", &[why])))
}

/// `App`'s part in all this.
#[derive(Default)]
pub struct OpenPlay {
    pub run: Option<Run>,
    /// The stream under way was ended by the player (`end_stream`).
    pub left_by_player: bool,
    pub notice: Option<(String, String)>,
    pub sign_in: Option<Arc<dyn autostart::Writer>>,
    /// The sign-in setting being written: the value asked for and the writer's answer.
    pub sign_in_answer: Option<(bool, Receiver<Result<(), String>>)>,
    /// Kept for its lock (`claim_window`).
    _window: Option<File>,
    /// When a copy without the claim asks again: the claim passes on when its holder ends, so that a
    /// copy opened after that does not start a game beside this one.
    claim_at: Option<Instant>,
}

impl OpenPlay {
    pub fn new(run: Option<Run>, notice: Option<(String, String)>, sign_in: Option<Arc<dyn autostart::Writer>>) -> Self {
        Self { run, notice, sign_in, ..Self::default() }
    }

    /// Asks for the claim again when it is time. It starts no game by itself: this copy's start was decided.
    fn keep_claiming(&mut self, dir: &Path, now: Instant) {
        if self.claim_at.is_none_or(|at| now < at) {
            return;
        }
        match claim_window(dir) {
            Ok(Some(file)) => {
                self._window = Some(file);
                self.claim_at = None;
            }
            // A failure was said at launch; it stays worth asking.
            Ok(None) | Err(_) => self.claim_at = Some(now + CLAIM_RETRY),
        }
    }

    /// What the settings page shows at start: the writer's word, or off when it cannot say.
    pub fn sign_in_now(&self) -> Option<bool> {
        let writer = self.sign_in.as_ref()?;
        Some(writer.is_on().unwrap_or_else(|e| {
            eprintln!("nightjar: reading the sign-in start failed: {e}");
            false
        }))
    }
}

impl crate::App {
    fn open_play_armed(&self, armed: bool) {
        let mut state = self.store.open_play();
        state.armed = armed;
        if let Err(e) = self.store.save_open_play(&state) {
            eprintln!("nightjar: saving the automatic start failed: {e}");
        }
    }

    fn close_countdown(&mut self) {
        if matches!(self.launcher.dialog, Dialog::OpenPlay { .. }) {
            self.launcher.dialog = Dialog::None;
        }
    }

    /// The game menu's choice, or taking it back: the setting follows it, so the choice does what it says.
    pub(crate) fn open_play_choose(&mut self, game: Option<OpenGame>) {
        let mut state = self.store.open_play();
        state.game = game.clone();
        if let Err(e) = self.store.save_open_play(&state) {
            self.launcher.message(tr("Could not save this choice"), e.to_string());
            return;
        }
        match &game {
            Some(game) => {
                self.launcher.settings.open_on_start = OpenOnStart::Chosen;
                self.launcher.toast(trf("{} opens when Nightjar starts", &[&game.title]));
            }
            None if self.launcher.settings.open_on_start == OpenOnStart::Chosen => self.launcher.settings.open_on_start = OpenOnStart::Pcs,
            None => {}
        }
        self.launcher.open_game = game;
        if let Err(e) = self.store.save_settings(&self.launcher.settings) {
            eprintln!("nightjar: saving settings failed: {e}");
        }
    }

    /// Writes the sign-in setting off the UI thread: a portal may ask the user first.
    pub(crate) fn sign_in_set(&mut self, on: bool) {
        let Some(writer) = self.open_play.sign_in.clone().filter(|_| self.open_play.sign_in_answer.is_none()) else { return };
        let (sender, answer) = channel();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let _ = sender.send(writer.set(on));
            waker();
        });
        self.open_play.sign_in_answer = Some((on, answer));
    }

    fn sign_in_step(&mut self) {
        let Some((on, answer)) = &self.open_play.sign_in_answer else { return };
        let Ok(result) = answer.try_recv() else { return };
        let on = *on;
        self.open_play.sign_in_answer = None;
        match result {
            Ok(()) => self.launcher.sign_in = Some(on),
            Err(why) => self.launcher.message(tr("Could not change the start at sign-in"), why),
        }
    }

    fn open_play_seen(&self, host: u64) -> Seen {
        let host = self.hosts.get(host);
        let dialog = &self.launcher.dialog;
        let holding = self.open_play.run.as_ref().is_some_and(Run::holding);
        Seen {
            pin_screen: self.launcher.pin.as_ref().is_some_and(crate::pin_ui::PinLock::app_locked),
            no_dialog: matches!(dialog, Dialog::None),
            countdown: matches!(dialog, Dialog::OpenPlay { .. }),
            message: matches!(dialog, Dialog::Message { .. }),
            identity: self.hosts.identity().is_some(),
            host: host.map(|h| HostSeen {
                reach: h.reach,
                can_wake: h.record.mac.is_some(),
                paired: h.paired(),
                session: SessionSeen::of(h.native_api(), h.native.session.as_ref(), h.native.error.is_some()),
                apps: match &h.apps {
                    Apps::NotLoaded => AppsSeen::NotLoaded,
                    Apps::Loading => AppsSeen::Loading,
                    Apps::Failed(why) => AppsSeen::Failed(why.clone()),
                    Apps::Loaded(apps) => {
                        let game = self.open_play.run.as_ref().map(|r| &r.target.game);
                        let pairs: Vec<(i32, &str)> = apps.iter().map(|a| (a.id, a.uuid.as_str())).collect();
                        AppsSeen::Loaded(game.and_then(|g| console::saved_classic_app(&pairs, &g.install_id, g.app_id)))
                    }
                },
            }),
            network: !holding || host.is_none_or(|h| crate::network::has_route(&h.record.address)),
            waking: self.waking.host().is_some(),
            busy: self.connecting.is_some() || self.network_launch.is_some() || self.pin_pending.is_some() || self.pending_replace.is_some() || self.pending_launch.is_some(),
            streaming: self.streaming.is_some(),
            left_by_player: self.open_play.left_by_player,
        }
    }

    /// Drives the sign-in setting and the automatic start, whichever screen is up.
    pub(crate) fn open_play_step(&mut self) {
        self.sign_in_step();
        self.open_play.keep_claiming(self.store.dir(), Instant::now());
        if self.open_play.notice.is_some() && matches!(self.launcher.dialog, Dialog::None) && !self.launcher.pin.as_ref().is_some_and(crate::pin_ui::PinLock::app_locked) {
            if let Some((title, body)) = self.open_play.notice.take() {
                self.launcher.message(title, body);
            }
        }
        let Some(run) = self.open_play.run.as_ref() else { return };
        let host = run.target.host;
        if !self.running {
            // Closing Nightjar is the player's way out, whatever was under way.
            if run.armed() {
                self.open_play_armed(false);
            }
            self.open_play.run = None;
            return;
        }
        let seen = self.open_play_seen(host);
        let Some(run) = self.open_play.run.as_mut() else { return };
        match run.next(&seen, Instant::now()) {
            Step::Wait => {}
            Step::Repoll => self.hosts.poll_one(host),
            Step::Count(left) => {
                let (game, pc) = (run.target.game.title.clone(), run.target.pc.clone());
                if let Dialog::OpenPlay { left: shown, .. } = &mut self.launcher.dialog {
                    *shown = left;
                } else {
                    eprintln!("nightjar: counting down to {game} on {pc}");
                    self.launcher.ask(Dialog::OpenPlay { game, pc, left });
                }
            }
            Step::Begin => {
                if let Dialog::OpenPlay { left, .. } = &mut self.launcher.dialog {
                    *left = 0;
                }
                // Armed before anything can go wrong: only a good ending clears it.
                self.open_play_armed(true);
            }
            Step::Wake => {
                self.close_countdown();
                self.command(Command::Wake(host));
            }
            Step::LoadApps => self.hosts.load_apps(host),
            Step::Start(app) => {
                self.close_countdown();
                let mut commands = Vec::new();
                let dialog = match self.hosts.get(host) {
                    Some(h) => match &h.apps {
                        Apps::Loaded(apps) => apps.iter().find(|a| a.id == app).and_then(|a| self.launcher.start_app(h, a, &mut commands)),
                        _ => None,
                    },
                    None => None,
                };
                if let Some(dialog) = dialog {
                    // Another game runs there: the player decides now, so this is no longer an automatic start.
                    self.launcher.ask(dialog);
                    self.open_play.run = None;
                    self.open_play_armed(false);
                }
                for command in commands {
                    self.command(command);
                }
            }
            Step::Stopped(why) => {
                self.close_countdown();
                self.open_play.run = None;
                self.open_play_armed(false);
                self.launcher.message(tr("Automatic start skipped"), why);
            }
            Step::Failed(why) => {
                self.open_play.run = None;
                if let Some(why) = why {
                    self.launcher.message(tr("The stream could not start"), why);
                }
            }
            Step::Over { disarm } => {
                self.close_countdown();
                self.open_play.run = None;
                if disarm {
                    self.open_play_armed(false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target { host: 1, pc: "Desk".into(), game: OpenGame { host: "uid".into(), install_id: "x".into(), app_id: 7, title: "Dota 2".into() } }
    }

    fn online(apps: AppsSeen) -> Option<HostSeen> {
        Some(HostSeen { reach: Reach::Online, can_wake: true, paired: true, apps, session: SessionSeen::Free })
    }

    fn ready() -> Seen {
        Seen { no_dialog: true, identity: true, network: true, host: online(AppsSeen::Loaded(Some(7))), ..Seen::default() }
    }

    /// The dialog stands in front while the start is under way.
    fn going() -> Seen {
        Seen { countdown: true, no_dialog: false, ..ready() }
    }

    fn offline(can_wake: bool) -> Option<HostSeen> {
        Some(HostSeen { reach: Reach::Offline, can_wake, paired: true, apps: AppsSeen::NotLoaded, session: SessionSeen::Waiting })
    }

    /// A run past its countdown.
    fn begun(now: Instant) -> Run {
        let mut run = Run::new(target());
        assert_eq!(run.next(&ready(), now), Step::Count(3));
        let counting = Seen { countdown: true, no_dialog: false, ..ready() };
        assert_eq!(run.next(&counting, now + Duration::from_millis(2100)), Step::Count(1));
        assert_eq!(run.next(&counting, now + Duration::from_secs(3)), Step::Begin);
        assert!(run.armed());
        run
    }

    #[test]
    fn the_pin_screen_and_a_dialog_from_before_come_first() {
        let now = Instant::now();
        let mut run = Run::new(target());
        assert_eq!(run.next(&Seen { pin_screen: true, ..ready() }, now), Step::Wait);
        assert_eq!(run.next(&Seen { no_dialog: false, ..ready() }, now), Step::Wait);
        assert!(!run.armed());
        assert_eq!(run.next(&ready(), now), Step::Count(3), "the full countdown after them");
    }

    #[test]
    fn a_start_with_no_network_opens_as_usual_and_never_arms() {
        let now = Instant::now();
        let mut run = Run::new(target());
        let step = run.next(&Seen { network: false, ..ready() }, now);
        assert_eq!(step, Step::Stopped("No network, so Nightjar opened as usual.".into()));
        assert!(!run.armed());
    }

    #[test]
    fn a_sign_in_start_waits_for_the_network_and_then_hears_from_the_pc_first() {
        let now = Instant::now();
        let mut run = Run { at_sign_in: true, ..Run::new(target()) };
        let down = Seen { network: false, ..ready() };
        assert_eq!(run.next(&down, now), Step::Wait);
        assert_eq!(run.next(&down, now + Duration::from_secs(59)), Step::Wait);
        assert!(!run.armed());
        let back = now + Duration::from_secs(59);
        assert_eq!(run.next(&ready(), back), Step::Repoll);
        assert_eq!(run.next(&ready(), back + Duration::from_secs(5)), Step::Wait, "a poll's time to answer");
        assert_eq!(run.next(&ready(), back + NETWORK_SETTLE), Step::Count(3));
    }

    #[test]
    fn a_sign_in_start_gives_up_on_the_network_in_the_end() {
        let now = Instant::now();
        let mut run = Run { at_sign_in: true, ..Run::new(target()) };
        let down = Seen { network: false, ..ready() };
        assert_eq!(run.next(&down, now), Step::Wait);
        assert!(matches!(run.next(&down, now + NETWORK_LIMIT), Step::Stopped(_)));
        assert!(!run.armed());
    }

    #[test]
    fn a_network_there_from_the_start_needs_no_settling() {
        let now = Instant::now();
        let mut run = Run { at_sign_in: true, ..Run::new(target()) };
        assert_eq!(run.next(&ready(), now), Step::Count(3));
    }

    #[test]
    fn a_pc_not_reached_yet_gets_its_time_at_sign_in_only() {
        let now = Instant::now();
        let mut run = Run { at_sign_in: true, ..begun(now) };
        let away = Seen { host: offline(false), ..going() };
        assert_eq!(run.next(&away, now), Step::Wait);
        assert_eq!(run.next(&going(), now), Step::Start(7), "it answered");
        let mut run = Run { at_sign_in: true, ..begun(now) };
        assert!(matches!(run.next(&away, now + Duration::from_secs(3) + START_LIMIT + Duration::from_secs(1)), Step::Failed(Some(_))));
    }

    #[test]
    fn a_game_started_before_the_countdown_ends_the_run_without_touching_the_guard() {
        let now = Instant::now();
        let mut run = Run { at_sign_in: true, ..Run::new(target()) };
        assert_eq!(run.next(&Seen { busy: true, network: false, ..ready() }, now), Step::Over { disarm: false });
    }

    #[test]
    fn cancelling_the_countdown_starts_nothing_and_leaves_the_guard_alone() {
        let now = Instant::now();
        let mut run = Run::new(target());
        run.next(&ready(), now);
        assert_eq!(run.next(&ready(), now), Step::Over { disarm: false }, "the dialog is gone");
    }

    #[test]
    fn an_online_pc_starts_the_game_and_a_lasting_stream_disarms() {
        let now = Instant::now();
        let mut run = begun(now);
        assert_eq!(run.next(&going(), now), Step::Start(7));
        assert_eq!(run.next(&Seen { busy: true, ..ready() }, now), Step::Wait);
        let streaming = Seen { streaming: true, ..ready() };
        assert_eq!(run.next(&streaming, now), Step::Wait);
        assert_eq!(run.next(&streaming, now + Duration::from_secs(9)), Step::Wait);
        assert_eq!(run.next(&streaming, now + Duration::from_secs(10)), Step::Over { disarm: true });
    }

    #[test]
    fn a_stream_that_dies_young_keeps_the_guard_unless_the_player_left_it() {
        let now = Instant::now();
        let start = |seen: &Seen| {
            let mut run = begun(now);
            run.next(&going(), now);
            run.next(&Seen { streaming: true, ..ready() }, now);
            run.next(seen, now + Duration::from_secs(4))
        };
        assert_eq!(start(&ready()), Step::Over { disarm: false });
        assert_eq!(start(&Seen { left_by_player: true, ..ready() }), Step::Over { disarm: true });
        assert_eq!(start(&Seen { busy: true, ..ready() }), Step::Wait, "reconnecting");
    }

    #[test]
    fn a_stream_that_died_young_is_not_cleared_by_a_reconnect_across_the_mark() {
        let now = Instant::now();
        let at = |secs| now + Duration::from_secs(secs);
        let reconnecting = Seen { busy: true, ..ready() };
        let streaming = Seen { streaming: true, ..ready() };
        let mut run = begun(now);
        run.next(&going(), now);
        assert_eq!(run.next(&streaming, now), Step::Wait);
        assert_eq!(run.next(&reconnecting, at(4)), Step::Wait, "the host died");
        assert_eq!(run.next(&reconnecting, at(11)), Step::Wait, "no stream, so nothing lasted");
        assert_eq!(run.next(&ready(), at(12)), Step::Over { disarm: false }, "the reconnect gave up");

        let mut run = begun(now);
        run.next(&going(), now);
        run.next(&streaming, now);
        run.next(&reconnecting, at(4));
        assert_eq!(run.next(&streaming, at(8)), Step::Wait, "back, and its clock starts over");
        assert_eq!(run.next(&streaming, at(17)), Step::Wait, "9 s of it");
        assert_eq!(run.next(&streaming, at(18)), Step::Over { disarm: true });
    }

    #[test]
    fn a_failed_launch_ends_on_its_message_with_the_guard_armed() {
        let now = Instant::now();
        let mut run = begun(now);
        run.next(&going(), now);
        assert_eq!(run.next(&Seen { message: true, no_dialog: false, ..ready() }, now), Step::Failed(None));
    }

    #[test]
    fn a_prompt_waits_and_its_cancel_is_the_players_doing() {
        let now = Instant::now();
        let mut run = begun(now);
        run.next(&going(), now);
        assert_eq!(run.next(&Seen { no_dialog: false, ..ready() }, now), Step::Wait);
        assert_eq!(run.next(&ready(), now), Step::Over { disarm: true });
    }

    #[test]
    fn an_offline_pc_is_woken_and_then_started() {
        let now = Instant::now();
        let mut run = begun(now);
        assert_eq!(run.next(&Seen { host: offline(true), ..going() }, now), Step::Wake);
        let away = Seen { host: offline(true), ..ready() };
        assert_eq!(run.next(&Seen { waking: true, ..away }, now + Duration::from_secs(120)), Step::Wait, "the wake has its own limit");
        assert_eq!(run.next(&Seen { host: online(AppsSeen::NotLoaded), ..ready() }, now + Duration::from_secs(120)), Step::Count(0), "Cancel again");
        assert_eq!(run.next(&Seen { host: online(AppsSeen::NotLoaded), ..going() }, now + Duration::from_secs(120)), Step::LoadApps);
        assert_eq!(run.next(&Seen { host: online(AppsSeen::Loading), ..going() }, now + Duration::from_secs(121)), Step::Wait);
        assert_eq!(run.next(&going(), now + Duration::from_secs(122)), Step::Start(7));
    }

    #[test]
    fn a_wake_that_times_out_fails_on_its_message_and_a_stopped_one_is_the_players() {
        let now = Instant::now();
        let away = Seen { host: offline(true), ..ready() };
        let mut run = begun(now);
        run.next(&Seen { host: offline(true), ..going() }, now);
        assert_eq!(run.next(&Seen { message: true, ..away.clone() }, now), Step::Failed(None));
        let mut run = begun(now);
        run.next(&Seen { host: offline(true), ..going() }, now);
        assert_eq!(run.next(&away, now), Step::Over { disarm: true });
    }

    #[test]
    fn the_player_can_still_say_no_while_the_list_loads() {
        let now = Instant::now();
        let mut run = begun(now);
        assert_eq!(run.next(&Seen { host: online(AppsSeen::NotLoaded), ..going() }, now), Step::LoadApps);
        let cancelled = Seen { host: online(AppsSeen::Loading), ..ready() };
        assert_eq!(run.next(&cancelled, now), Step::Over { disarm: true }, "Cancel closed the dialog");
    }

    #[test]
    fn a_game_the_player_started_meanwhile_ends_the_run() {
        let now = Instant::now();
        for (name, seen) in [
            ("connecting to it", Seen { busy: true, ..going() }),
            ("streaming it", Seen { streaming: true, ..ready() }),
        ] {
            let mut run = begun(now);
            assert_eq!(run.next(&Seen { host: online(AppsSeen::Loading), ..going() }, now), Step::Wait);
            assert_eq!(run.next(&seen, now), Step::Over { disarm: true }, "{name}");
        }
    }

    #[test]
    fn a_game_playing_on_another_device_stops_the_start_and_is_never_moved() {
        let now = Instant::now();
        let mut there = online(AppsSeen::Loaded(Some(7))).unwrap();
        there.session = SessionSeen::Elsewhere;
        let mut run = begun(now);
        let step = run.next(&Seen { host: Some(there), ..going() }, now);
        assert!(matches!(&step, Step::Stopped(why) if why.contains("Desk")), "{step:?}");
    }

    #[test]
    fn the_start_waits_for_the_first_read_of_the_session() {
        let now = Instant::now();
        let mut there = online(AppsSeen::Loaded(Some(7))).unwrap();
        there.session = SessionSeen::Waiting;
        let waiting = Seen { host: Some(there), ..going() };
        let mut run = begun(now);
        assert_eq!(run.next(&waiting, now), Step::Wait);
        assert!(matches!(run.next(&waiting, now + Duration::from_secs(3) + START_LIMIT + Duration::from_secs(1)), Step::Failed(Some(_))), "a read that never comes");
        let mut run = begun(now);
        assert_eq!(run.next(&waiting, now), Step::Wait);
        assert_eq!(run.next(&going(), now), Step::Start(7), "the read says it is free");
    }

    #[test]
    fn only_a_live_session_of_another_device_counts_as_elsewhere() {
        let session = |state: &str, owner: &str| -> Session {
            serde_json::from_str(&include_str!("../../client/tests/fixtures/native-api/session.json").replace("\"suspended\"", &format!("\"{state}\"")).replace("\"you\"", &format!("\"{owner}\""))).unwrap()
        };
        assert_eq!(SessionSeen::of(true, Some(&session("streaming", "other_device")), false), SessionSeen::Elsewhere);
        assert_eq!(SessionSeen::of(true, Some(&session("suspended", "other_device")), false), SessionSeen::Elsewhere);
        assert_eq!(SessionSeen::of(true, Some(&session("streaming", "you")), false), SessionSeen::Free);
        assert_eq!(SessionSeen::of(true, Some(&session("idle", "other_device")), false), SessionSeen::Free);
        assert_eq!(SessionSeen::of(true, None, false), SessionSeen::Waiting);
        assert_eq!(SessionSeen::of(true, None, true), SessionSeen::Waiting, "a read that failed says nothing");
        assert_eq!(SessionSeen::of(true, Some(&session("idle", "you")), true), SessionSeen::Waiting, "and free before is old news");
        assert_eq!(SessionSeen::of(true, Some(&session("streaming", "other_device")), true), SessionSeen::Elsewhere);
        assert_eq!(SessionSeen::of(false, None, false), SessionSeen::Free, "no native API, no owner to wait for");
    }

    #[test]
    fn what_cannot_start_says_why() {
        let now = Instant::now();
        let failed = |seen: Seen| matches!(begun(now).next(&seen, now), Step::Failed(Some(_)));
        assert!(failed(Seen { host: None, ..going() }), "the PC was removed");
        assert!(failed(Seen { host: offline(false), ..going() }), "no address");
        assert!(failed(Seen { host: Some(HostSeen { reach: Reach::Online, can_wake: true, paired: false, apps: AppsSeen::NotLoaded, session: SessionSeen::Waiting }), ..going() }), "not paired");
        assert!(failed(Seen { host: online(AppsSeen::Loaded(None)), ..going() }), "not in the list");
        assert!(failed(Seen { host: online(AppsSeen::Failed("x".into())), ..going() }), "no list");
        let mut run = begun(now);
        let unknown = Seen { host: Some(HostSeen { reach: Reach::Unknown, can_wake: true, paired: true, apps: AppsSeen::NotLoaded, session: SessionSeen::Waiting }), ..going() };
        assert_eq!(run.next(&unknown, now), Step::Wait);
        assert!(matches!(run.next(&unknown, now + START_LIMIT + Duration::from_secs(10)), Step::Failed(Some(_))), "a PC that never answers");
    }

    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("nightjar-open-play-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Store::at(dir.clone()).unwrap(), dir)
    }

    fn recent() -> Recent {
        Recent { host: "uid".into(), install_id: "x".into(), app_id: 7, title: "Dota 2".into(), at: 1 }
    }

    fn known(unique: &str) -> Option<(u64, String)> {
        (unique == "uid").then(|| (1, "Desk".to_owned()))
    }

    #[test]
    fn a_failed_start_skips_the_next_one_says_so_once_and_then_it_runs_again() {
        let (store, dir) = store("guard");
        let settings = Settings { open_on_start: OpenOnStart::Last, ..Settings::default() };
        let recents = [recent()];
        let (run, notice) = at_start(&store, &settings, &recents, true, known);
        assert_eq!(run.map(|r| r.target), Some(target()));
        assert!(notice.is_none());
        // The start failed, or the app died during it: nothing cleared the guard.
        store.save_open_play(&crate::config::OpenPlayState { game: None, armed: true }).unwrap();
        let (run, notice) = at_start(&store, &settings, &recents, true, known);
        assert!(run.is_none());
        assert!(notice.is_some_and(|(_, body)| body.contains("Dota 2")));
        let (run, notice) = at_start(&store, &settings, &recents, true, known);
        assert!(run.is_some() && notice.is_none(), "one failure skips one start");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_launcher_start_counts_and_the_default_starts_nothing() {
        let (store, dir) = store("entry");
        let last = Settings { open_on_start: OpenOnStart::Last, ..Settings::default() };
        let recents = [recent()];
        assert!(at_start(&store, &last, &recents, false, known).0.is_none(), "a command line");
        assert!(at_start(&store, &Settings::default(), &recents, true, known).0.is_none());
        assert!(at_start(&store, &last, &[], true, known).0.is_none(), "nothing played yet");
        assert!(at_start(&store, &last, &recents, true, |_| None).0.is_none(), "its PC is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_copy_the_player_opened_alone_starts_a_game() {
        assert!(may_start_a_game(true, false, false, false));
        assert!(!may_start_a_game(false, false, false, false), "a command line");
        assert!(!may_start_a_game(true, true, false, false), "an update's restart");
        assert!(!may_start_a_game(true, false, true, false), "a stream in another copy");
        assert!(!may_start_a_game(true, false, false, true), "another window");
    }

    #[test]
    fn a_second_copy_leaves_the_guard_of_the_first_alone() {
        let (store, dir) = store("second");
        let settings = Settings { open_on_start: OpenOnStart::Last, ..Settings::default() };
        store.save_open_play(&crate::config::OpenPlayState { game: None, armed: true }).unwrap();
        let (run, notice) = at_start(&store, &settings, &[recent()], may_start_a_game(true, false, false, true), known);
        assert!(run.is_none() && notice.is_none());
        assert!(store.open_play().armed, "the first copy's start is still under way");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_one_copy_holds_the_window_until_it_ends() {
        let (store, dir) = store("window");
        let first = claim_window(store.dir()).unwrap();
        assert!(first.is_some());
        assert!(claim_window(store.dir()).unwrap().is_none(), "the second copy sees the first");
        drop(first);
        assert!(claim_window(store.dir()).unwrap().is_some(), "it ended");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_that_lost_the_claim_takes_it_when_its_holder_ends_so_a_later_copy_stands_aside() {
        let (store, dir) = store("claim-again");
        let now = Instant::now();
        let first = claim_window(store.dir()).unwrap();
        let mut second = OpenPlay { claim_at: Some(now), ..OpenPlay::default() };
        second.keep_claiming(store.dir(), now);
        assert!(second._window.is_none(), "the first still holds it");
        assert_eq!(second.claim_at, Some(now + CLAIM_RETRY), "and it asks again");
        second.keep_claiming(store.dir(), now);
        assert_eq!(second.claim_at, Some(now + CLAIM_RETRY), "not before its time");
        drop(first);
        second.keep_claiming(store.dir(), now + CLAIM_RETRY);
        assert!(second._window.is_some());
        assert_eq!(second.claim_at, None);
        assert!(claim_window(store.dir()).unwrap().is_none(), "a third copy sees the second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_claim_that_fails_says_so_when_a_start_was_due() {
        let why = std::io::Error::other("no locks");
        assert!(claim_failed(false, &why).is_none(), "nothing was going to start");
        let (title, body) = claim_failed(true, &why).unwrap();
        assert_eq!(title, "Automatic start skipped");
        assert!(body.contains("no locks"), "{body}");
    }

    #[test]
    fn a_chosen_game_is_the_one_that_starts() {
        let (store, dir) = store("chosen");
        let settings = Settings { open_on_start: OpenOnStart::Chosen, ..Settings::default() };
        let chosen = OpenGame { host: "uid".into(), install_id: "y".into(), app_id: 9, title: "Hades".into() };
        assert!(at_start(&store, &settings, &[recent()], true, known).0.is_none(), "none chosen yet");
        store.save_open_play(&crate::config::OpenPlayState { game: Some(chosen.clone()), armed: false }).unwrap();
        let (run, _) = at_start(&store, &settings, &[recent()], true, known);
        assert_eq!(run.map(|r| r.target.game), Some(chosen));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
