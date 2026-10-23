//! What Home and the game page show, worked out from what the PCs reported. No egui and no
//! network in here, so the rules (which screen to open on, how libraries merge, Play or Resume,
//! when Close game is offered) are tested on their own. Same rules as Nightjar Android's console.

use nightjar_core::http::native_api::library::{Game, Library};
use nightjar_core::http::native_api::session::{ActionId, Session, State};

use crate::hosts::{Host, Reach};

/// One PC as the console sees it.
pub struct Source<'a> {
    pub id: u64,
    pub name: &'a str,
    pub online: bool,
    pub paired: bool,
    pub library: Option<&'a Library>,
    /// The session, only while the PC is reachable (an old one is not shown for a PC that is away).
    pub session: Option<&'a Session>,
    /// This device may end games on the PC.
    pub host_control: bool,
}

impl<'a> Source<'a> {
    pub fn of(host: &'a Host) -> Self {
        let online = host.reach == Reach::Online;
        Self {
            id: host.id,
            name: host.display_name(),
            online,
            paired: host.paired(),
            library: host.native.library.as_ref().filter(|_| host.paired()),
            session: host.native.session.as_ref().filter(|_| online),
            host_control: host.native.host_control(),
        }
    }
}

/// Whether the app starts on Home: the setting is on and some PC has a library to show, live or
/// saved from an earlier run.
pub fn opens_on_home(enabled: bool, sources: &[Source<'_>]) -> bool {
    enabled && has_library(sources)
}

pub fn has_library(sources: &[Source<'_>]) -> bool {
    sources.iter().any(|s| s.library.is_some_and(|l| !l.games.is_empty()))
}

/// A game on one PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub host: u64,
    pub host_name: String,
    pub online: bool,
    pub app_id: u32,
    pub install_id: String,
    pub running: bool,
    /// The cover's version, empty when the PC has no cover for it.
    pub cover_version: String,
    pub has_cover: bool,
}

/// A game in the merged library: the same game on several PCs is one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub title: String,
    /// The copy to open first: a running one, then one on a PC that is online, then PC order.
    pub copies: Vec<Copy>,
}

impl Entry {
    pub fn best(&self) -> &Copy {
        &self.copies[0]
    }

    pub fn running(&self) -> bool {
        self.copies.iter().any(|c| c.running)
    }

    /// The copy whose cover to draw.
    pub fn cover(&self) -> Option<&Copy> {
        self.copies.iter().find(|c| c.has_cover)
    }
}

fn installed(game: &Game) -> bool {
    game.installed != Some(false)
}

/// Whether the PC's session runs or holds a game (not idle, stopping or unknown).
fn active(session: &Session) -> bool {
    matches!(session.state, State::Preparing | State::Launching | State::Streaming | State::Suspended)
        && session.app.is_some()
}

fn running_here(source: &Source<'_>, game: &Game) -> bool {
    game.running
        || source.session.is_some_and(|s| {
            active(s) && s.app.as_ref().is_some_and(|a| a.install_id == game.install_id || a.app_id == game.app_id)
        })
}

/// Every PC's installed games, one entry per game (by the store's game id, else per PC and
/// app), sorted by title.
pub fn merge_library(sources: &[Source<'_>]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for source in sources {
        let Some(library) = source.library else { continue };
        for game in library.games.iter().filter(|g| installed(g)) {
            let key = match game.game_id.as_deref().filter(|id| !id.is_empty()) {
                Some(id) => format!("game:{id}"),
                None => format!("host:{}:{}", source.id, game.app_id),
            };
            let copy = Copy {
                host: source.id,
                host_name: source.name.to_owned(),
                online: source.online,
                app_id: game.app_id,
                install_id: game.install_id.clone(),
                running: source.online && running_here(source, game),
                cover_version: game.artwork.version.clone(),
                has_cover: game.artwork.cover.is_some(),
            };
            match entries.iter_mut().find(|e| e.key == key) {
                Some(entry) => entry.copies.push(copy),
                None => entries.push(Entry { key, title: game.title.clone(), copies: vec![copy] }),
            }
        }
    }
    for entry in &mut entries {
        // Stable: PC order decides between equals.
        entry.copies.sort_by_key(|c| (!c.running, !c.online));
    }
    entries.sort_by_cached_key(|e| e.title.to_lowercase());
    entries
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playing {
    Running,
    Suspended,
}

/// A game to carry on with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Continue {
    pub host: u64,
    pub host_name: String,
    pub app_id: u32,
    pub title: String,
    pub playing: Playing,
}

/// Games running or suspended on PCs that are online: running first, then suspended, each in
/// PC order. A game the library marks running without a stream counts as running.
pub fn continue_playing(sources: &[Source<'_>]) -> Vec<Continue> {
    let mut out: Vec<Continue> = Vec::new();
    for source in sources.iter().filter(|s| s.online && s.paired) {
        let mut seen: Vec<u32> = Vec::new();
        if let Some(session) = source.session.filter(|s| active(s)) {
            let app = session.app.as_ref().expect("active session has an app");
            seen.push(app.app_id);
            out.push(Continue {
                host: source.id,
                host_name: source.name.to_owned(),
                app_id: app.app_id,
                title: app.title.clone(),
                playing: if session.state == State::Suspended { Playing::Suspended } else { Playing::Running },
            });
        }
        for game in source.library.into_iter().flat_map(|l| &l.games) {
            if game.running && !seen.contains(&game.app_id) {
                seen.push(game.app_id);
                out.push(Continue {
                    host: source.id,
                    host_name: source.name.to_owned(),
                    app_id: game.app_id,
                    title: game.title.clone(),
                    playing: Playing::Running,
                });
            }
        }
    }
    out.sort_by_key(|c| c.playing == Playing::Suspended);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameState {
    Idle,
    Running,
    Suspended,
}

/// How the game page ends a running game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndAction {
    None,
    /// Native Close game, which needs the PC's Host control permission.
    CloseGame,
    /// The classic quit of whatever the PC runs.
    Quit,
}

/// The game page's state for one game on one PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detail {
    pub state: GameState,
    /// The PC can be played on now (online and paired).
    pub can_start: bool,
    pub end: EndAction,
    /// Another game that runs on the PC; starting this one closes it.
    pub other_running: Option<String>,
}

impl Detail {
    /// Resume for a game that is running or suspended, Play otherwise.
    pub fn resumes(&self) -> bool {
        self.state != GameState::Idle
    }
}

/// `native` is whether the PC speaks the native API (Close game is only offered there).
pub fn detail(source: &Source<'_>, app_id: u32, native: bool) -> Detail {
    let can_start = source.online && source.paired;
    let game = source.library.into_iter().flat_map(|l| &l.games).find(|g| g.app_id == app_id);
    let session = source.session.filter(|s| active(s));
    // A PC that is away reports nothing live, whatever was saved.
    let mut state = GameState::Idle;
    let mut other_running = None;
    if source.online {
        let own = session.and_then(|s| s.app.as_ref()).is_some_and(|a| {
            a.app_id == app_id || game.is_some_and(|g| a.install_id == g.install_id)
        });
        if own {
            let suspended = session.is_some_and(|s| s.state == State::Suspended);
            state = if suspended { GameState::Suspended } else { GameState::Running };
        } else if session.is_none() && game.is_some_and(|g| g.running) {
            // No stream or hold on the PC, but the library says the game runs.
            state = GameState::Running;
        }
        if state == GameState::Idle {
            other_running = session
                .and_then(|s| s.app.as_ref())
                .map(|a| a.title.clone())
                .or_else(|| {
                    source.library.into_iter().flat_map(|l| &l.games).find(|g| g.running).map(|g| g.title.clone())
                });
        }
    }
    let close_allowed = source.host_control
        && session
            .and_then(|s| s.actions.iter().find(|a| a.id == ActionId::CloseGame))
            .is_none_or(|a| a.allowed);
    let end = match state {
        GameState::Idle => EndAction::None,
        _ if native && close_allowed => EndAction::CloseGame,
        _ => EndAction::Quit,
    };
    Detail { state, can_start, end, other_running }
}

/// What a PC's card says about its games.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostLine {
    Games(usize),
    SavedGames(usize),
    Offline,
    NoLibrary,
}

pub fn host_line(source: &Source<'_>) -> HostLine {
    let games = source.library.map(|l| l.games.iter().filter(|g| installed(g)).count());
    match (source.online, games) {
        (true, Some(n)) => HostLine::Games(n),
        (false, Some(n)) if n > 0 => HostLine::SavedGames(n),
        (false, _) => HostLine::Offline,
        (true, None) => HostLine::NoLibrary,
    }
}

impl HostLine {
    pub fn text(self) -> String {
        match self {
            Self::Games(n) => format!("Games: {n}"),
            Self::SavedGames(n) => format!("Offline. Saved games: {n}"),
            Self::Offline => "Offline".into(),
            Self::NoLibrary => "Games not loaded yet".into(),
        }
    }
}

/// "1920 × 1080, 60 FPS, 20 Mbps" for the game page.
pub fn stream_summary(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> String {
    let mbps = f64::from(bitrate_kbps) / 1000.0;
    let mbps = if (mbps - mbps.round()).abs() < 0.05 { format!("{}", mbps.round() as u64) } else { format!("{mbps:.1}") };
    format!("{width} × {height}, {fps} FPS, {mbps} Mbps")
}

/// The classic app list entry for a library game: by the install id the PC gives both lists,
/// else by app id. `apps` are `(id, uuid)`.
pub fn classic_app(apps: &[(i32, &str)], install_id: &str, app_id: u32) -> Option<i32> {
    apps.iter()
        .find(|(_, uuid)| !uuid.is_empty() && uuid.eq_ignore_ascii_case(install_id))
        .or_else(|| apps.iter().find(|(id, _)| *id as u32 == app_id))
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::native_api::capabilities::Capabilities;

    const LIBRARY: &str = include_str!("../../client/tests/fixtures/native-api/library.json");
    const SESSION: &str = include_str!("../../client/tests/fixtures/native-api/session.json");
    const CAPABILITIES: &str = include_str!("../../client/tests/fixtures/native-api/capabilities.json");

    fn library() -> Library {
        serde_json::from_str(LIBRARY).unwrap()
    }

    fn session() -> Session {
        serde_json::from_str(SESSION).unwrap()
    }

    fn idle() -> Session {
        let mut s = session();
        s.state = State::Idle;
        s.app = None;
        s.game_running = false;
        s
    }

    fn source<'a>(id: u64, name: &'a str, online: bool, library: Option<&'a Library>, session: Option<&'a Session>) -> Source<'a> {
        Source { id, name, online, paired: true, library, session, host_control: false }
    }

    const DOTA: u32 = 1093052211;
    const DESKTOP: u32 = 881448767;

    #[test]
    fn the_launch_screen_follows_the_setting_and_a_library() {
        let lib = library();
        let empty = Library { revision: "x".into(), games: Vec::new() };
        let with = [source(1, "A", true, Some(&lib), None)];
        let without = [source(1, "A", true, None, None), source(2, "B", true, Some(&empty), None)];
        assert!(opens_on_home(true, &with));
        assert!(!opens_on_home(false, &with), "off starts on the list of PCs");
        assert!(!opens_on_home(true, &without), "no library, no Home");
        // A saved library counts for a PC that is away.
        assert!(opens_on_home(true, &[source(1, "A", false, Some(&lib), None)]));
    }

    #[test]
    fn libraries_merge_by_game_id_and_sort_by_title() {
        let a = library();
        let mut b = library();
        b.games[0].app_id = 7;
        b.games[0].install_id = "other".into();
        b.games[1].title = "alpha".into();
        b.games[1].installed = Some(false); // not installed: left out
        let mut c = library();
        c.games[0].game_id = None;
        c.games[0].title = "Zed".into();
        let sources = [
            source(1, "A", false, Some(&a), None),
            source(2, "B", true, Some(&b), None),
            source(3, "C", true, Some(&c), None),
        ];
        let merged = merge_library(&sources);
        let titles: Vec<_> = merged.iter().map(|e| e.title.as_str()).collect();
        // Desktop (A and C, no game id so one per PC), Dota 2 (A+B by game id), Zed.
        assert_eq!(titles, ["Desktop", "Desktop", "Dota 2", "Zed"]);
        let dota = merged.iter().find(|e| e.key == "game:steam:570").unwrap();
        // The copy on the online PC comes first.
        assert_eq!(dota.copies.iter().map(|c| (c.host, c.app_id)).collect::<Vec<_>>(), [(2, 7), (1, DOTA)]);
        assert_eq!(dota.best().host, 2);
        assert!(merged.iter().all(|e| e.title != "alpha"));
    }

    #[test]
    fn a_running_copy_is_opened_first() {
        let a = library();
        let b = library();
        let mut live = session();
        live.state = State::Streaming;
        let sources = [source(1, "A", true, Some(&a), None), source(2, "B", true, Some(&b), Some(&live))];
        let dota = merge_library(&sources).into_iter().find(|e| e.title == "Dota 2").unwrap();
        assert!(dota.running());
        assert_eq!(dota.best().host, 2);
    }

    #[test]
    fn continue_playing_lists_running_before_suspended_on_online_pcs() {
        let mut lib = library();
        lib.games[1].running = false;
        let suspended = session(); // fixture: Dota 2, suspended
        let mut running = session();
        running.state = State::Streaming;
        running.app.as_mut().unwrap().title = "Other".into();
        running.app.as_mut().unwrap().app_id = 5;
        let desktop = library(); // the fixture has Desktop running
        let quiet = idle();
        let sources = [
            source(1, "A", true, Some(&lib), Some(&suspended)),
            source(2, "B", true, None, Some(&running)),
            source(3, "C", false, Some(&lib), Some(&running)), // away: nothing live
            source(4, "D", true, Some(&desktop), Some(&quiet)), // Desktop runs without a stream
        ];
        let list = continue_playing(&sources);
        let got: Vec<_> = list.iter().map(|c| (c.host, c.title.as_str(), c.playing)).collect();
        assert_eq!(
            got,
            [
                (2, "Other", Playing::Running),
                (4, "Desktop", Playing::Running),
                (1, "Dota 2", Playing::Suspended),
            ]
        );
        // Idle sessions and unknown states are not shown.
        let mut unknown = session();
        unknown.state = State::Unknown;
        assert!(continue_playing(&[source(1, "A", true, None, Some(&unknown))]).is_empty());
    }

    #[test]
    fn play_becomes_resume_for_a_running_or_suspended_game() {
        let lib = library();
        let suspended = session();
        let idle = idle();
        let play = detail(&source(1, "A", true, Some(&lib), Some(&idle)), DOTA, true);
        assert_eq!((play.state, play.resumes(), play.can_start), (GameState::Idle, false, true));
        let resume = detail(&source(1, "A", true, Some(&lib), Some(&suspended)), DOTA, true);
        assert_eq!((resume.state, resume.resumes()), (GameState::Suspended, true));
        let mut streaming = session();
        streaming.state = State::Streaming;
        let running = detail(&source(1, "A", true, Some(&lib), Some(&streaming)), DOTA, true);
        assert_eq!(running.state, GameState::Running);
        // Matched by the install id when the app id differs.
        let mut renamed = library();
        renamed.games[0].app_id = 9;
        let by_install = detail(&source(1, "A", true, Some(&renamed), Some(&streaming)), 9, true);
        assert_eq!(by_install.state, GameState::Running);
    }

    #[test]
    fn a_game_that_runs_elsewhere_asks_before_replacing() {
        let lib = library();
        let suspended = session(); // Dota 2 suspended
        let other = detail(&source(1, "A", true, Some(&lib), Some(&suspended)), DESKTOP, true);
        assert_eq!(other.state, GameState::Idle);
        assert_eq!(other.other_running.as_deref(), Some("Dota 2"));
        assert_eq!(other.end, EndAction::None);
        // Desktop runs by the library's flag alone.
        let idle = idle();
        let by_flag = detail(&source(1, "A", true, Some(&lib), Some(&idle)), DOTA, true);
        assert_eq!(by_flag.other_running.as_deref(), Some("Desktop"));
    }

    #[test]
    fn a_pc_that_is_away_is_idle_and_cannot_start() {
        let lib = library();
        let suspended = session();
        let d = detail(&source(1, "A", false, Some(&lib), Some(&suspended)), DOTA, true);
        assert_eq!((d.state, d.can_start, d.end, d.other_running), (GameState::Idle, false, EndAction::None, None));
        let mut unpaired = source(1, "A", true, Some(&lib), None);
        unpaired.paired = false;
        assert!(!detail(&unpaired, DOTA, true).can_start);
    }

    #[test]
    fn close_game_needs_host_control_on_a_native_pc() {
        let lib = library();
        let mut streaming = session();
        streaming.state = State::Streaming;
        for a in &mut streaming.actions {
            a.allowed = true; // the PC lists every action as allowed
        }
        let mut s = source(1, "A", true, Some(&lib), Some(&streaming));
        assert_eq!(detail(&s, DOTA, true).end, EndAction::Quit, "no Host control: classic quit");
        s.host_control = true;
        assert_eq!(detail(&s, DOTA, true).end, EndAction::CloseGame);
        assert_eq!(detail(&s, DOTA, false).end, EndAction::Quit, "a PC without the native API");
        // The PC's own word wins: it lists Close game as not allowed.
        let fixture = session();
        let mut s = source(1, "A", true, Some(&lib), Some(&fixture));
        s.host_control = true;
        assert_eq!(detail(&s, DOTA, true).end, EndAction::Quit);
    }

    #[test]
    fn the_fixture_capabilities_give_no_host_control() {
        let caps: Capabilities = serde_json::from_str(CAPABILITIES).unwrap();
        assert!(!caps.permissions.host_control);
    }

    #[test]
    fn host_cards_say_what_they_know() {
        let lib = library();
        let none: Option<&Library> = None;
        assert_eq!(host_line(&source(1, "A", true, Some(&lib), None)), HostLine::Games(2));
        assert_eq!(host_line(&source(1, "A", false, Some(&lib), None)), HostLine::SavedGames(2));
        assert_eq!(host_line(&source(1, "A", false, none, None)), HostLine::Offline);
        assert_eq!(host_line(&source(1, "A", true, none, None)), HostLine::NoLibrary);
        assert_eq!(HostLine::Games(2).text(), "Games: 2");
        assert_eq!(HostLine::SavedGames(2).text(), "Offline. Saved games: 2");
        assert_eq!(HostLine::NoLibrary.text(), "Games not loaded yet");
        let mut some_missing = library();
        some_missing.games[1].installed = Some(false);
        assert_eq!(host_line(&source(1, "A", true, Some(&some_missing), None)), HostLine::Games(1));
    }

    #[test]
    fn the_stream_summary_reads_like_the_android_one() {
        assert_eq!(stream_summary(1920, 1080, 60, 20_000), "1920 × 1080, 60 FPS, 20 Mbps");
        assert_eq!(stream_summary(2560, 1440, 120, 37_500), "2560 × 1440, 120 FPS, 37.5 Mbps");
    }

    #[test]
    fn play_finds_the_classic_entry_by_install_id_then_app_id() {
        let apps = [(11, ""), (12, "C6F1A9A4-4B51-5F43-9D0B-7A2B1C2D3E4F"), (13, "x")];
        assert_eq!(classic_app(&apps, "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 99), Some(12));
        assert_eq!(classic_app(&apps, "none", 13), Some(13));
        assert_eq!(classic_app(&apps, "none", 99), None);
        // An empty uuid never matches an empty install id.
        assert_eq!(classic_app(&apps, "", 99), None);
    }
}
