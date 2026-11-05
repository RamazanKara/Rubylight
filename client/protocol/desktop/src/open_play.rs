//! "When Nightjar opens" on the desktop: the countdown into a game, its start through the launcher's
//! own wake and launch, and the guard (`nightjar_policy::open_play`) that keeps a start which comes to
//! nothing from trapping the player. Also runs the sign-in setting (`autostart`) off the UI thread.

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::session::{Owner, Session};
use nightjar_policy::open_play::{self as policy, Action, Entry, Facts, Game};

use crate::autostart;
use crate::config::{OpenGame, OpenOnStart, Recent, Settings, Store};
use crate::console;
use crate::hosts::{Apps, Hosts, Reach};
use crate::strings::{tr, trf};
use crate::ui::{Command, Dialog, Launcher};

/// How long the PC gets to answer and its app list to load, outside a wake's own wait.
const START_LIMIT: Duration = Duration::from_secs(45);

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
    Streaming { since: Instant },
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
    /// Its first read after answering is still to come.
    Waiting,
    /// Nothing of another device's, or a PC that cannot tell (a host refuses a start it may not make).
    Free,
    /// Another device is playing there: an automatic start never moves that.
    Elsewhere,
}

impl SessionSeen {
    fn of(native_api: bool, session: Option<&Session>, read_failed: bool) -> Self {
        match session {
            _ if !native_api => Self::Free,
            Some(s) if s.owner == Owner::OtherDevice && console::active(s) => Self::Elsewhere,
            Some(_) => Self::Free,
            None if read_failed => Self::Free,
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
    /// A launch, connection, PIN or prompt is under way.
    pub busy: bool,
    pub streaming: bool,
    pub left_by_player: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
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
}

impl Run {
    pub fn new(target: Target) -> Self {
        Self { target, phase: Phase::Hold }
    }

    /// Whether the guard is armed for this run: past the countdown.
    pub fn armed(&self) -> bool {
        !matches!(self.phase, Phase::Hold | Phase::Counting { .. })
    }

    pub fn next(&mut self, seen: &Seen, now: Instant) -> Step {
        match self.phase {
            Phase::Hold => {
                if seen.pin_screen || !seen.no_dialog {
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
                    self.phase = Phase::Streaming { since: now };
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
                let lasted = now.saturating_duration_since(since).as_millis() as u64;
                if lasted >= policy::SETTLED_MS {
                    Step::Over { disarm: true }
                } else if seen.streaming || seen.busy {
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

/// Sets up a start of Nightjar: decides the run and gives the settings page and game menu their state.
pub fn at_launch(store: &Store, launcher: &mut Launcher, hosts: &Hosts, launcher_start: bool) -> OpenPlay {
    let host_of = |unique: &str| {
        (!unique.is_empty()).then(|| hosts.list.iter().find(|h| h.record.unique_id == unique)).flatten().map(|h| (h.id, h.display_name().to_owned()))
    };
    let (run, notice) = at_start(store, &launcher.settings, &launcher.recents, launcher_start, host_of);
    let sign_in = autostart::system(store.dir());
    let open_play = OpenPlay::new(run, notice, sign_in);
    launcher.open_game = store.open_play().game;
    launcher.sign_in = open_play.sign_in_now();
    open_play
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
}

impl OpenPlay {
    pub fn new(run: Option<Run>, notice: Option<(String, String)>, sign_in: Option<Arc<dyn autostart::Writer>>) -> Self {
        Self { run, notice, sign_in, ..Self::default() }
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
                        AppsSeen::Loaded(game.and_then(|g| console::classic_app(&pairs, &g.install_id, g.app_id)))
                    }
                },
            }),
            waking: self.waking.host().is_some(),
            busy: self.connecting.is_some() || self.network_launch.is_some() || self.pin_pending.is_some() || self.pending_replace.is_some() || self.pending_launch.is_some(),
            streaming: self.streaming.is_some(),
            left_by_player: self.open_play.left_by_player,
        }
    }

    /// Drives the sign-in setting and the automatic start, whichever screen is up.
    pub(crate) fn open_play_step(&mut self) {
        self.sign_in_step();
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
        Seen { no_dialog: true, identity: true, host: online(AppsSeen::Loaded(Some(7))), ..Seen::default() }
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
        assert_eq!(SessionSeen::of(true, None, true), SessionSeen::Free, "the host refuses what it may not do");
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
