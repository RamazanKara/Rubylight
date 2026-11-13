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
            // No `console::active`: the host does not name another device's app to this one.
            Some(s) if s.owner == Owner::OtherDevice && console::live(s) => Self::Elsewhere,
            _ if read_failed => Self::Waiting,
            Some(_) => Self::Free,
            None => Self::Waiting,
        }
    }
}

/// The PC's session reads so far, counted in the order they started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionReads {
    pub asked: u64,
    pub answered: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSeen {
    pub reach: Reach,
    pub can_wake: bool,
    pub paired: bool,
    pub apps: AppsSeen,
    pub session: SessionSeen,
    /// `None` on a PC without the native API, which has no session to read.
    pub reads: Option<SessionReads>,
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
    /// The countdown is over: ask who plays now, as another device may have started during it.
    ReadSession,
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
    /// The session reads asked by the end of the countdown: only a later one says who plays now.
    session_reads_before: u64,
}

impl Run {
    pub fn new(target: Target) -> Self {
        Self { target, phase: Phase::Hold, at_sign_in: false, network_down_since: None, settled_at: None, session_reads_before: 0 }
    }

    /// Whether the guard is armed for this run: past the countdown.
    pub fn armed(&self) -> bool {
        !matches!(self.phase, Phase::Hold | Phase::Counting { .. })
    }

    /// What to say when this run's launch met another game, in the PC's report or in the host's
    /// answer: the player pressed nothing, so there is nothing to confirm. `None` for any other
    /// launch, which asks as usual.
    pub fn game_running(&self) -> Option<String> {
        matches!(self.phase, Phase::Launched).then(|| trf("A game is already running on {}, so Nightjar opened as usual.", &[&self.target.pc]))
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
                    self.session_reads_before = seen.host.as_ref().and_then(|h| h.reads).map_or(0, |r| r.asked);
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
            // Before what the session says: the other device may have stopped during the countdown too.
            Reach::Online if host.reads.is_some_and(|r| r.answered <= self.session_reads_before) => {
                let asked = host.reads.is_some_and(|r| r.asked > self.session_reads_before);
                in_time(if asked { Step::Wait } else { Step::ReadSession })
            }
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
                AppsSeen::Loaded(None) => Step::Failed(Some(tr("The PC’s app list has no entry for this game.").to_owned())),
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
            (None, Some((tr("Automatic start skipped").to_owned(), trf("{} didn’t start last time, so Nightjar opened as usual.", &[&title]))))
        }
    }
}

/// What the command line said about who started this copy.
#[derive(Clone, Copy, Debug, Default)]
pub struct Started {
    pub at_sign_in: bool,
    pub after_setup: bool,
}

/// Whether this start is the player opening Nightjar. A frontend's stream has its own game, and a
/// pairing link is about pairing: neither starts the game chosen for opening.
pub fn opened_by_player(frontend: bool, autostream: bool, link: bool) -> bool {
    !frontend && !autostream && !link
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
            message: matches!(dialog, Dialog::Message { .. } | Dialog::ConnectFailed { .. }),
            identity: self.hosts.identity().is_some(),
            host: host.map(|h| HostSeen {
                reach: h.reach,
                can_wake: h.record.can_wake(),
                paired: h.paired(),
                session: SessionSeen::of(h.native_api(), h.native.session.as_ref(), h.native.error.is_some()),
                reads: h.native_api().then(|| h.session_reads()),
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

    fn open_play_stop(&mut self, why: String) {
        self.close_countdown();
        self.open_play.run = None;
        self.open_play_armed(false);
        self.launcher.message(tr("Automatic start skipped"), why);
    }

    /// The launch of an automatic start met another game: it stops with one message. False when no
    /// automatic start is launching, and the launcher asks.
    pub(crate) fn open_play_game_running(&mut self) -> bool {
        let Some(why) = self.open_play.run.as_ref().and_then(Run::game_running) else { return false };
        self.open_play_stop(why);
        true
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
            Step::ReadSession => self.hosts.refresh_native(host, false),
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
                if matches!(dialog, Some(Dialog::ConfirmQuit { .. })) {
                    self.open_play_game_running();
                } else if let Some(dialog) = dialog {
                    // Not a start this run can make alone: the player decides now.
                    self.launcher.ask(dialog);
                    self.open_play.run = None;
                    self.open_play_armed(false);
                }
                for command in commands {
                    self.command(unasked(command));
                }
            }
            Step::Stopped(why) => self.open_play_stop(why),
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

/// What an automatic start sends: the read of who plays would hold the launch while the run looks
/// finished, and could put Continue here in front of a player who pressed nothing.
fn unasked(command: Command) -> Command {
    match command {
        Command::Launch { host, app, resume } => Command::LaunchUnasked { host, app, resume },
        other => other,
    }
}

#[cfg(test)]
mod tests;
