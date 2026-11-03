//! What Home and the game page show, worked out from what the PCs reported. No egui and no
//! network in here, so the rules (which screen to open on, how libraries merge, Play or Resume,
//! when Close game is offered) are tested on their own. Same rules as Nightjar Android's console.

use nightjar_core::http::native_api::library::{Game, Library};
use nightjar_core::http::native_api::session::{ActionId, Session, State};

use crate::strings::{en, tr, trf};
use crate::config::{Favourite, HomeGroup, Recent};
use crate::hosts::{Host, Reach};

/// One PC as the console sees it.
pub struct Source<'a> {
    pub id: u64,
    /// The PC's unique id, empty until it answered once.
    pub unique_id: &'a str,
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
            unique_id: &host.record.unique_id,
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
    /// The PC lists it as installed (or says nothing either way).
    pub installed: bool,
    /// The store's key ("steam", "epic", ...), None for an app no store owns.
    pub store: Option<String>,
    /// The store's name as the PC writes it, else the one Nightjar knows.
    pub store_name: Option<String>,
    /// The cover's version, empty when the PC has no cover for it.
    pub cover_version: String,
    pub has_cover: bool,
}

/// A game in the merged library: the same game on several PCs is one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub title: String,
    /// The copy to open first: a running one, then an installed one, then one on a PC that is
    /// online, then PC order.
    pub copies: Vec<Copy>,
}

impl Entry {
    pub fn best(&self) -> &Copy {
        &self.copies[0]
    }

    pub fn running(&self) -> bool {
        self.copies.iter().any(|c| c.running)
    }

    /// Some PC has it installed; one that is not shows, but cannot start until it is.
    pub fn installed(&self) -> bool {
        self.copies.iter().any(|c| c.installed)
    }

    /// The store of the copy that is opened first: the entry's row and filter.
    pub fn store(&self) -> Option<&str> {
        self.best().store.as_deref()
    }

    /// The copy whose cover to draw.
    pub fn cover(&self) -> Option<&Copy> {
        self.copies.iter().find(|c| c.has_cover)
    }
}

/// The stores a host reads games from, by the key it sends, with their own product names (never
/// translated): Android's `Stores`. Every store behaves the same; the table only names them.
const STORES: [(&str, &str); 7] = [
    ("steam", "Steam"),
    ("epic", "Epic Games"),
    ("gog", "GOG"),
    ("ea", "EA app"),
    ("ubisoft", "Ubisoft Connect"),
    ("battlenet", "Battle.net"),
    ("microsoft", en("Xbox")),
];

/// A store's name: the PC's when it sends one, else the known one, else the key with a capital
/// ("itchio" reads "Itchio").
pub fn store_name(key: &str, from_pc: &str) -> String {
    if !from_pc.trim().is_empty() {
        return from_pc.to_owned();
    }
    STORES.iter().find(|(k, _)| *k == key).map_or_else(
        || {
            let mut chars = key.chars();
            chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
        },
        |(_, name)| (*name).to_owned(),
    )
}

/// A chip above All games: Android's `HomeModel.Filter` keys (all, installed, store:steam, other).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum LibraryFilter {
    #[default]
    All,
    /// Only what some PC has installed; offered only when something is not.
    Installed,
    /// One store's games, by its key.
    Store(String),
    /// Every store's games, without the apps of no store: the Games row's chip; offered only
    /// when the library has both.
    Games,
    /// The apps of no store; offered only beside stores.
    Other,
}

impl LibraryFilter {
    /// Whether `entry` shows under this filter.
    pub fn matches(&self, entry: &Entry) -> bool {
        match self {
            Self::All => true,
            Self::Installed => entry.installed(),
            Self::Store(key) => entry.store() == Some(key.as_str()),
            Self::Games => entry.store().is_some(),
            Self::Other => entry.store().is_none(),
        }
    }

    /// A key that names the filter itself, not its place among the chips, for focus.
    pub fn key(&self) -> String {
        match self {
            Self::All => "filter:all".into(),
            Self::Installed => "filter:installed".into(),
            Self::Store(key) => format!("filter:store:{key}"),
            Self::Games => "filter:games".into(),
            Self::Other => "filter:other".into(),
        }
    }
}

/// One chip: the filter, the store's name for a store chip, and how many games it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterChip {
    pub filter: LibraryFilter,
    pub store_name: Option<String>,
    pub count: usize,
}

/// The stores of `entries` by their game count, largest first, ties by key: (key, name, count).
fn stores_by_size(entries: &[Entry]) -> Vec<(String, String, usize)> {
    let mut stores: Vec<(String, String, usize)> = Vec::new();
    for entry in entries {
        let Some(key) = entry.store() else { continue };
        match stores.iter_mut().find(|(k, ..)| k == key) {
            Some(store) => store.2 += 1,
            None => stores.push((key.to_owned(), entry.best().store_name.clone().unwrap_or_else(|| store_name(key, "")), 1)),
        }
    }
    stores.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    stores
}

/// The chips above All games for what the library has: All, Installed (only when something is
/// not), Games, each store largest first, Other apps (Games and Other apps only when the library
/// has both). None for an empty library.
pub fn library_filters(entries: &[Entry]) -> Vec<FilterChip> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut chips = vec![FilterChip { filter: LibraryFilter::All, store_name: None, count: entries.len() }];
    let installed = entries.iter().filter(|e| e.installed()).count();
    if installed < entries.len() {
        chips.push(FilterChip { filter: LibraryFilter::Installed, store_name: None, count: installed });
    }
    let stores = stores_by_size(entries);
    let other = entries.iter().filter(|e| e.store().is_none()).count();
    let any_store = !stores.is_empty();
    if other > 0 && any_store {
        chips.push(FilterChip { filter: LibraryFilter::Games, store_name: None, count: entries.len() - other });
    }
    for (key, name, count) in stores {
        chips.push(FilterChip { filter: LibraryFilter::Store(key), store_name: Some(name), count });
    }
    if other > 0 && any_store {
        chips.push(FilterChip { filter: LibraryFilter::Other, store_name: None, count: other });
    }
    chips
}

/// The games a filter shows, in the order All games lists them (by title).
pub fn filtered<'a>(entries: &'a [Entry], filter: &LibraryFilter) -> Vec<&'a Entry> {
    entries.iter().filter(|e| filter.matches(e)).collect()
}

/// Which of Home's library rows: one per store, every store's games in one, or the apps of no store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    Store { key: String, name: String },
    /// The games of every store, apart from the apps of no store (Home grouped by type).
    Games,
    /// The apps of no store beside stores ("Other apps").
    Other,
    /// A library with no stores at all (a classic host): one plain row.
    Library,
}

/// A library row on Home: its games, installed first, and the All games filter its "All N"
/// tile opens. The row itself shows as many as fit; the tile stands for the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryRow<'a> {
    pub kind: RowKind,
    pub filter: LibraryFilter,
    pub games: Vec<&'a Entry>,
}

impl LibraryRow<'_> {
    /// A key that stays the same across refreshes, for focus.
    pub fn key(&self) -> String {
        match &self.kind {
            RowKind::Store { key, .. } => format!("row:store:{key}"),
            RowKind::Games => "row:games".into(),
            RowKind::Other => "row:other".into(),
            RowKind::Library => "row:library".into(),
        }
    }

    /// The games shown in a line of `room` covers: all that fit, else one fewer and the "All N"
    /// tile in the last place. Returns how many games to draw and whether the tile follows.
    pub fn fit(&self, room: usize) -> (usize, bool) {
        let room = room.max(1);
        if self.games.len() <= room {
            (self.games.len(), false)
        } else {
            (room - 1, true)
        }
    }
}

/// Home's library rows: one per store, largest first (ties by key), then the apps of no store.
/// Within a row the installed games come first, each part by title. Android's TV Home rows.
pub fn library_rows(entries: &[Entry]) -> Vec<LibraryRow<'_>> {
    let stores = stores_by_size(entries);
    let mut rows: Vec<LibraryRow<'_>> = stores
        .iter()
        .map(|(key, name, _)| LibraryRow {
            kind: RowKind::Store { key: key.clone(), name: name.clone() },
            filter: LibraryFilter::Store(key.clone()),
            games: installed_first(entries.iter().filter(|e| e.store() == Some(key.as_str())).collect()),
        })
        .collect();
    let other: Vec<&Entry> = entries.iter().filter(|e| e.store().is_none()).collect();
    if !other.is_empty() {
        let plain = stores.is_empty();
        rows.push(LibraryRow {
            kind: if plain { RowKind::Library } else { RowKind::Other },
            filter: if plain { LibraryFilter::All } else { LibraryFilter::Other },
            games: installed_first(other),
        });
    }
    rows
}

/// Home's library rows under its View: [`library_rows`] by store; by type, the games of every
/// store in one Games row and the apps of no store (launchers, the desktop) in Other apps; or the
/// whole library as one Library row that opens All games. Installed first, each part by title;
/// every entry, installed or not, as by store (Android's ungrouped grid alone keeps to installed
/// games). A title is a game when its PC says which store it is from: PCs send nothing else that
/// tells games from apps. A library with no stores at all is one Library row by type too.
pub fn home_rows(entries: &[Entry], group: HomeGroup) -> Vec<LibraryRow<'_>> {
    if group == HomeGroup::Store {
        return library_rows(entries);
    }
    if entries.is_empty() {
        return Vec::new();
    }
    let whole = || LibraryRow { kind: RowKind::Library, filter: LibraryFilter::All, games: installed_first(entries.iter().collect()) };
    if group == HomeGroup::None {
        return vec![whole()];
    }
    let (games, apps): (Vec<&Entry>, Vec<&Entry>) = entries.iter().partition(|e| e.store().is_some());
    if games.is_empty() {
        return vec![whole()];
    }
    // Its All tile opens the Games chip, which is there only beside apps; without them All is the same.
    let filter = if apps.is_empty() { LibraryFilter::All } else { LibraryFilter::Games };
    let mut rows = vec![LibraryRow { kind: RowKind::Games, filter, games: installed_first(games) }];
    if !apps.is_empty() {
        rows.push(LibraryRow { kind: RowKind::Other, filter: LibraryFilter::Other, games: installed_first(apps) });
    }
    rows
}

fn installed_first(mut games: Vec<&Entry>) -> Vec<&Entry> {
    // Stable: the titles stay in order within each part.
    games.sort_by_key(|e| !e.installed());
    games
}

fn installed(game: &Game) -> bool {
    game.installed != Some(false)
}

/// Whether the PC's session runs or holds a game (not idle, stopping or unknown).
fn active(session: &Session) -> bool {
    matches!(session.state, State::Preparing | State::Launching | State::Streaming | State::Suspended)
        && session.app.is_some()
}

/// The same store id. An empty id (a host that sent none) matches nothing, not everything.
fn same_install(a: &str, b: &str) -> bool {
    !a.is_empty() && a == b
}

fn running_here(source: &Source<'_>, game: &Game) -> bool {
    game.running
        || source.session.is_some_and(|s| {
            active(s) && s.app.as_ref().is_some_and(|a| same_install(&a.install_id, &game.install_id) || a.app_id == game.app_id)
        })
}

/// Every PC's games, one entry per game (by the store's game id, else per PC and app), sorted by
/// title. A game no PC has installed is in it too, marked so (Android's library rows and All
/// games show it, and the Installed chip leaves it out).
pub fn merge_library(sources: &[Source<'_>]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    // Where each key's entry is, so a large library merges in one pass.
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for source in sources {
        let Some(library) = source.library else { continue };
        for game in &library.games {
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
                installed: installed(game),
                store: game.store.as_ref().map(|s| s.source.clone()).filter(|s| !s.is_empty()),
                store_name: game.store.as_ref().filter(|s| !s.source.is_empty()).map(|s| store_name(&s.source, &s.name)),
                cover_version: game.artwork.version.clone(),
                has_cover: game.artwork.cover.is_some(),
            };
            match index.get(&key) {
                Some(&at) => entries[at].copies.push(copy),
                None => {
                    index.insert(key.clone(), entries.len());
                    entries.push(Entry { key, title: game.title.clone(), copies: vec![copy] });
                }
            }
        }
    }
    for entry in &mut entries {
        // Stable: PC order decides between equals.
        entry.copies.sort_by_key(|c| (!c.running, !c.installed, !c.online));
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

/// A game streamed before, found in a PC's library now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recently {
    pub host: u64,
    pub host_name: String,
    pub online: bool,
    pub app_id: u32,
    pub title: String,
}

/// The recent games, latest first, that a paired PC's library still lists (installed), without
/// the ones already under "Continue playing". The title is the library's, not the saved one.
pub fn recently_played(sources: &[Source<'_>], recents: &[Recent], playing: &[Continue], limit: usize) -> Vec<Recently> {
    let mut out: Vec<Recently> = Vec::new();
    for recent in recents {
        let Some(source) = sources.iter().find(|s| !s.unique_id.is_empty() && s.unique_id == recent.host) else { continue };
        let Some(library) = source.library else { continue };
        let found = library.games.iter().filter(|g| installed(g)).find(|g| {
            if !recent.install_id.is_empty() && !g.install_id.is_empty() {
                g.install_id.eq_ignore_ascii_case(&recent.install_id)
            } else {
                g.app_id == recent.app_id
            }
        });
        let Some(game) = found else { continue };
        let listed = out.iter().any(|r| r.host == source.id && r.app_id == game.app_id);
        if playing.iter().any(|c| c.host == source.id && c.app_id == game.app_id) || listed {
            continue;
        }
        out.push(Recently {
            host: source.id,
            host_name: source.name.to_owned(),
            online: source.online,
            app_id: game.app_id,
            title: game.title.clone(),
        });
        if out.len() == limit {
            break;
        }
    }
    out
}

/// When the entry was last streamed, in seconds since the epoch: the latest recent game of any of
/// its copies, matched as "Recently played" matches them (install id, else app id).
pub fn last_played(entry: &Entry, sources: &[Source<'_>], recents: &[Recent]) -> Option<u64> {
    recents
        .iter()
        .filter(|recent| {
            entry.copies.iter().any(|copy| {
                let pc = sources.iter().any(|s| s.id == copy.host && !s.unique_id.is_empty() && s.unique_id == recent.host);
                let game = if !recent.install_id.is_empty() && !copy.install_id.is_empty() {
                    copy.install_id.eq_ignore_ascii_case(&recent.install_id)
                } else {
                    copy.app_id == recent.app_id
                };
                pc && game
            })
        })
        .map(|recent| recent.at)
        .max()
}

/// A List line's second line: the store (none for an app no store owns) and when the game was
/// last played (seconds since the epoch, `0` for never), by whole days before `now`. Nothing
/// about playing when `played` is unknown.
pub fn list_line(entry: &Entry, played: Option<u64>, now: u64) -> String {
    let when = match played {
        None => None,
        Some(0) => Some(tr("Not played yet").to_owned()),
        Some(at) => Some(match now.saturating_sub(at) / 86_400 {
            0 => tr("Played today").to_owned(),
            1 => tr("Played yesterday").to_owned(),
            days => trf("Played {} days ago", &[&days]),
        }),
    };
    match (&entry.best().store_name, when) {
        (Some(store), Some(when)) => format!("{store} · {when}"),
        (Some(store), None) => store.clone(),
        (None, when) => when.unwrap_or_default(),
    }
}

/// A starred game as Home shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Starred {
    pub host: u64,
    pub host_name: String,
    pub online: bool,
    pub app_id: u32,
    pub title: String,
    /// The stored star, to take it off again.
    pub favourite: Favourite,
}

/// The favourite a star on `game` stores, or none when the PC has not told its unique id yet
/// (nothing to key the star by).
pub fn favourite_of(source: &Source<'_>, game: &Game) -> Option<Favourite> {
    (!source.unique_id.is_empty()).then(|| Favourite {
        host: source.unique_id.to_owned(),
        install_id: game.install_id.clone(),
        app_id: game.app_id,
        title: game.title.clone(),
    })
}

/// The favourite for game `app_id` on the PC `host`, from that PC's library.
pub fn favourite_for(sources: &[Source<'_>], host: u64, app_id: u32) -> Option<Favourite> {
    let source = sources.iter().find(|s| s.id == host)?;
    let game = source.library?.games.iter().find(|g| g.app_id == app_id)?;
    favourite_of(source, game)
}

/// Whether `game` on this PC is starred.
pub fn is_starred(source: &Source<'_>, game: &Game, favourites: &[Favourite]) -> bool {
    favourite_of(source, game).is_some_and(|f| favourites.iter().any(|s| s.same_game(&f)))
}

/// The starred games of the PCs that are paired here, newest star first. A game the PC's library
/// no longer lists (or lists as not installed) is hidden and its star kept, in case it comes back.
/// A PC whose library is not known yet (away, nothing saved) shows the star with the saved title.
pub fn favourite_games(sources: &[Source<'_>], favourites: &[Favourite]) -> Vec<Starred> {
    let mut out: Vec<Starred> = Vec::new();
    for star in favourites {
        let Some(source) = sources.iter().find(|s| !s.unique_id.is_empty() && s.unique_id == star.host) else { continue };
        let (app_id, title) = match source.library {
            Some(library) => {
                let found = library.games.iter().filter(|g| installed(g)).find(|g| {
                    let probe = Favourite { host: star.host.clone(), install_id: g.install_id.clone(), app_id: g.app_id, title: String::new() };
                    star.same_game(&probe)
                });
                let Some(game) = found else { continue };
                (game.app_id, game.title.clone())
            }
            None => (star.app_id, star.title.clone()),
        };
        if out.iter().any(|s| s.host == source.id && s.app_id == app_id) {
            continue;
        }
        out.push(Starred {
            host: source.id,
            host_name: source.name.to_owned(),
            online: source.online,
            app_id,
            title,
            favourite: star.clone(),
        });
    }
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
    /// The game can be played now: the PC is online and paired, and the game installed there
    /// (one that runs or is suspended can be resumed whatever the library says).
    pub can_start: bool,
    /// The PC lists the game as installed, or says nothing either way. Android's NOT_INSTALLED.
    pub installed: bool,
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
    let game = source.library.into_iter().flat_map(|l| &l.games).find(|g| g.app_id == app_id);
    let installed = game.is_none_or(installed);
    let session = source.session.filter(|s| active(s));
    // A PC that is away reports nothing live, whatever was saved.
    let mut state = GameState::Idle;
    let mut other_running = None;
    if source.online {
        let own = session.and_then(|s| s.app.as_ref()).is_some_and(|a| {
            a.app_id == app_id || game.is_some_and(|g| same_install(&a.install_id, &g.install_id))
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
    // Android's GameDetailModel.blocked(): only an idle game is held back by the library.
    let can_start = source.online && source.paired && (installed || state != GameState::Idle);
    Detail { state, can_start, installed, end, other_running }
}

/// What a PC's card says about its games.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostLine {
    Games(usize),
    SavedGames(usize),
    Offline,
    NoLibrary,
}

/// Counts every game the PC lists, installed or not, as Home's rows and their "All N" do.
pub fn host_line(source: &Source<'_>) -> HostLine {
    let games = source.library.map(|l| l.games.len());
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
            Self::Games(n) => trf("Games: {}", &[&n]),
            Self::SavedGames(n) => trf("Offline. Saved games: {}", &[&n]),
            Self::Offline => tr("Offline").into(),
            Self::NoLibrary => tr("Games not loaded yet").into(),
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
/// What the PC runs now, compared with the app a prompt named when it opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Running {
    /// Still the named app (or nothing was named to compare with).
    Same,
    /// Nothing runs any more: there is nothing to close.
    Gone,
    /// Another app than the named one: closing it is not what the player agreed to.
    Changed,
}

pub fn recheck_running(named: Option<i32>, now: Option<i32>) -> Running {
    match (named, now) {
        (_, None) => Running::Gone,
        (Some(named), Some(now)) if named != now => Running::Changed,
        _ => Running::Same,
    }
}

/// The card that takes the focus when card `index` of `len` leaves the row: the next one, else the previous.
pub fn neighbour(len: usize, index: usize) -> Option<usize> {
    if index + 1 < len {
        Some(index + 1)
    } else {
        index.checked_sub(1).filter(|_| len > 1)
    }
}

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
        Source { id, unique_id: "", name, online, paired: true, library, session, host_control: false }
    }

    const DOTA: u32 = 1093052211;
    const DESKTOP: u32 = 881448767;

    #[test]
    fn an_empty_install_id_matches_nothing() {
        // A host that sent no store id for the game or for the session must not tie them together.
        let mut lib = library();
        for game in &mut lib.games {
            game.install_id.clear();
            game.running = false;
        }
        let mut running = session();
        running.state = State::Streaming;
        running.app.as_mut().unwrap().install_id.clear();
        assert_eq!(running.app.as_ref().unwrap().app_id, DOTA);
        let sources = [source(1, "A", true, Some(&lib), Some(&running))];
        assert_eq!(detail(&sources[0], DESKTOP, true).state, GameState::Idle, "another game is not the running one");
        assert_ne!(detail(&sources[0], DOTA, true).state, GameState::Idle, "the same app id still matches");
        let merged = merge_library(&sources);
        assert!(merged.iter().any(|e| e.title == "Dota 2"));
    }

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
        b.games[1].installed = Some(false); // not installed: listed, marked so
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
        // alpha, Desktop (A and C, no game id so one per PC), Dota 2 (A+B by game id), Zed.
        assert_eq!(titles, ["alpha", "Desktop", "Desktop", "Dota 2", "Zed"]);
        let dota = merged.iter().find(|e| e.key == "game:steam:570").unwrap();
        // The copy on the online PC comes first.
        assert_eq!(dota.copies.iter().map(|c| (c.host, c.app_id)).collect::<Vec<_>>(), [(2, 7), (1, DOTA)]);
        assert_eq!(dota.best().host, 2);
        assert_eq!((dota.store(), dota.best().store_name.as_deref()), (Some("steam"), Some("Steam")));
        let alpha = merged.iter().find(|e| e.title == "alpha").unwrap();
        assert!(!alpha.installed(), "a game no PC has installed is marked so");
        assert!(merged.iter().filter(|e| e.title != "alpha").all(Entry::installed));
    }

    #[test]
    fn an_installed_copy_is_opened_before_one_that_is_not() {
        let mut a = library();
        a.games[0].installed = Some(false);
        let b = library();
        let sources = [source(1, "A", true, Some(&a), None), source(2, "B", true, Some(&b), None)];
        let dota = merge_library(&sources).into_iter().find(|e| e.title == "Dota 2").unwrap();
        assert_eq!(dota.best().host, 2, "B has it installed");
        assert!(dota.installed());
    }

    /// A library of `games` (title, store key, store name as the PC sends it, installed).
    fn store_library(games: &[(&str, &str, &str, bool)]) -> Library {
        let mut lib = library();
        let template = lib.games[0].clone();
        lib.games = games
            .iter()
            .enumerate()
            .map(|(n, (title, key, name, installed))| Game {
                install_id: format!("install-{n}"),
                app_id: 100 + n as u32,
                title: (*title).into(),
                game_id: (!key.is_empty()).then(|| format!("{key}:{n}")),
                store: (!key.is_empty()).then(|| nightjar_core::http::native_api::library::Store {
                    source: (*key).into(),
                    id: n.to_string(),
                    name: (*name).into(),
                }),
                installed: Some(*installed),
                ..template.clone()
            })
            .collect();
        lib
    }

    fn mixed() -> Library {
        store_library(&[
            ("Halo", "microsoft", "", true),
            ("Portal", "steam", "Steam", true),
            ("Diablo", "battlenet", "", false),
            ("Celeste", "steam", "", true),
            ("Fortnite", "epic", "Epic Games Store", true),
            ("Notepad", "", "", true),
            ("Hades", "steam", "", false),
            ("Forza", "microsoft", "", true),
        ])
    }

    #[test]
    fn store_names_are_the_pcs_else_the_known_product_name_else_the_key_with_a_capital() {
        assert_eq!(store_name("epic", "Epic Games Store"), "Epic Games Store");
        assert_eq!(store_name("microsoft", ""), "Xbox");
        assert_eq!(store_name("battlenet", " "), "Battle.net");
        // A store Nightjar does not know, and the PC did not name, reads as a name, not a key.
        assert_eq!(store_name("itchio", ""), "Itchio");
        assert_eq!(store_name("itchio", "itch.io"), "itch.io");
    }

    #[test]
    fn a_filters_focus_key_names_the_filter_not_its_place() {
        let keys = [LibraryFilter::All, LibraryFilter::Installed, LibraryFilter::Store("steam".into()), LibraryFilter::Other].map(|f| f.key());
        assert_eq!(keys, ["filter:all", "filter:installed", "filter:store:steam", "filter:other"]);
    }

    /// The merge keeps its order: entries by title, equal titles in the order the PCs listed them,
    /// and each game's copies by running, installed, online, then PC order.
    #[test]
    fn the_merged_library_keeps_title_then_listing_order() {
        let a = store_library(&[("Same", "", "", true), ("beta", "steam", "", true), ("Alpha", "steam", "", true)]);
        let mut b = store_library(&[("same", "", "", true), ("Beta", "steam", "", false)]);
        // B's "Beta" is A's "beta" by its game id; its "same" has no store, so it stays its own.
        b.games[1].game_id = a.games[1].game_id.clone();
        let sources = [source(1, "A", true, Some(&a), None), source(2, "B", true, Some(&b), None)];
        let merged = merge_library(&sources);
        let got: Vec<(&str, Vec<u64>)> = merged.iter().map(|e| (e.title.as_str(), e.copies.iter().map(|c| c.host).collect())).collect();
        assert_eq!(got, [("Alpha", vec![1]), ("beta", vec![1, 2]), ("Same", vec![1]), ("same", vec![2])]);
        // B listed first: its not-installed copy still comes after A's installed one.
        let sources = [source(2, "B", true, Some(&b), None), source(1, "A", true, Some(&a), None)];
        let beta = merge_library(&sources).into_iter().find(|e| e.title == "Beta").unwrap();
        assert_eq!(beta.copies.iter().map(|c| c.host).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn the_filters_are_all_installed_each_store_by_size_and_other_apps() {
        let lib = mixed();
        let sources = [source(1, "A", true, Some(&lib), None)];
        let entries = merge_library(&sources);
        let chips = library_filters(&entries);
        let got: Vec<(LibraryFilter, Option<&str>, usize)> = chips.iter().map(|c| (c.filter.clone(), c.store_name.as_deref(), c.count)).collect();
        let store = |key: &str| LibraryFilter::Store(key.into());
        assert_eq!(
            got,
            [
                (LibraryFilter::All, None, 8),
                (LibraryFilter::Installed, None, 6),
                (LibraryFilter::Games, None, 7),
                (store("steam"), Some("Steam"), 3),
                (store("microsoft"), Some("Xbox"), 2),
                // Ties by key: battlenet before epic.
                (store("battlenet"), Some("Battle.net"), 1),
                (store("epic"), Some("Epic Games Store"), 1),
                (LibraryFilter::Other, None, 1),
            ]
        );
        // Every chip shows exactly as many games as it says, by title.
        for chip in &chips {
            assert_eq!(filtered(&entries, &chip.filter).len(), chip.count, "{:?}", chip.filter);
        }
        let titles = |f: &LibraryFilter| filtered(&entries, f).iter().map(|e| e.title.as_str()).collect::<Vec<_>>();
        assert_eq!(titles(&store("steam")), ["Celeste", "Hades", "Portal"]);
        assert_eq!(titles(&LibraryFilter::Installed), ["Celeste", "Fortnite", "Forza", "Halo", "Notepad", "Portal"]);
        assert_eq!(titles(&LibraryFilter::Other), ["Notepad"]);
    }

    #[test]
    fn epic_battle_net_and_xbox_behave_exactly_like_steam() {
        let lib = store_library(&[("A", "steam", "", true), ("B", "epic", "", true), ("C", "battlenet", "", true), ("D", "microsoft", "", true)]);
        let sources = [source(1, "A", true, Some(&lib), None)];
        let entries = merge_library(&sources);
        let chips = library_filters(&entries);
        let rows = library_rows(&entries);
        for key in ["steam", "epic", "battlenet", "microsoft"] {
            let filter = LibraryFilter::Store(key.into());
            assert_eq!(chips.iter().filter(|c| c.filter == filter).map(|c| c.count).collect::<Vec<_>>(), [1], "{key}: one chip");
            assert_eq!(rows.iter().filter(|r| r.filter == filter).map(|r| r.games.len()).collect::<Vec<_>>(), [1], "{key}: one row");
        }
        // Nothing uninstalled and no store-less apps: neither Installed nor Other is offered.
        assert!(!chips.iter().any(|c| matches!(c.filter, LibraryFilter::Installed | LibraryFilter::Other)));
    }

    #[test]
    fn home_rows_are_one_per_store_largest_first_installed_first_then_other_apps() {
        let lib = mixed();
        let sources = [source(1, "A", true, Some(&lib), None)];
        let entries = merge_library(&sources);
        let rows = library_rows(&entries);
        let got: Vec<(String, Vec<&str>)> = rows.iter().map(|r| (r.key(), r.games.iter().map(|e| e.title.as_str()).collect())).collect();
        assert_eq!(
            got,
            [
                ("row:store:steam".to_owned(), vec!["Celeste", "Portal", "Hades"]),
                ("row:store:microsoft".to_owned(), vec!["Forza", "Halo"]),
                ("row:store:battlenet".to_owned(), vec!["Diablo"]),
                ("row:store:epic".to_owned(), vec!["Fortnite"]),
                ("row:other".to_owned(), vec!["Notepad"]),
            ]
        );
        assert_eq!(rows[0].filter, LibraryFilter::Store("steam".into()), "All N opens the store's chip");
        assert_eq!(rows[4].filter, LibraryFilter::Other);
        // A library with no stores at all is one plain row under All.
        let plain = store_library(&[("B", "", "", true), ("A", "", "", true)]);
        let sources = [source(1, "A", true, Some(&plain), None)];
        let entries = merge_library(&sources);
        let rows = library_rows(&entries);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind.clone(), rows[0].filter.clone()), (RowKind::Library, LibraryFilter::All));
        assert_eq!(library_filters(&entries).len(), 1, "only All: no Other chip without stores");
    }

    #[test]
    fn home_rows_not_by_store_are_one_library_row_of_every_game_installed_first() {
        let lib = mixed();
        let sources = [source(1, "A", true, Some(&lib), None)];
        let entries = merge_library(&sources);
        assert_eq!(home_rows(&entries, HomeGroup::Store), library_rows(&entries));
        let rows = home_rows(&entries, HomeGroup::None);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind.clone(), rows[0].filter.clone(), rows[0].key()), (RowKind::Library, LibraryFilter::All, "row:library".to_owned()));
        assert_eq!(rows[0].games.len(), entries.len(), "every game, whatever its store");
        let installed: Vec<bool> = rows[0].games.iter().map(|e| e.installed()).collect();
        assert!(installed.windows(2).all(|w| w[0] || !w[1]), "installed first: {installed:?}");
        assert!(installed.contains(&false), "the fixture has a game that is not installed");
        assert!(home_rows(&[], HomeGroup::None).is_empty());
    }

    #[test]
    fn home_rows_by_type_are_every_stores_games_then_the_apps_of_no_store() {
        let lib = mixed();
        let sources = [source(1, "A", true, Some(&lib), None)];
        let entries = merge_library(&sources);
        let rows = home_rows(&entries, HomeGroup::Type);
        let got: Vec<(String, LibraryFilter)> = rows.iter().map(|r| (r.key(), r.filter.clone())).collect();
        assert_eq!(got, [("row:games".to_owned(), LibraryFilter::Games), ("row:other".to_owned(), LibraryFilter::Other)]);
        // Each row is exactly its All tile's chip: the same games, the same count.
        let chips = library_filters(&entries);
        for row in &rows {
            let chip = chips.iter().find(|c| c.filter == row.filter).expect("the row's chip");
            assert_eq!(chip.count, row.games.len(), "{:?}", row.filter);
            let mut shown: Vec<&str> = filtered(&entries, &row.filter).iter().map(|e| e.title.as_str()).collect();
            let mut held: Vec<&str> = row.games.iter().map(|e| e.title.as_str()).collect();
            shown.sort_unstable();
            held.sort_unstable();
            assert_eq!(shown, held, "{:?}", row.filter);
        }
        assert_eq!(rows[0].games.len() + rows[1].games.len(), entries.len(), "every game in one of them");
        assert!(rows[0].games.iter().all(|e| e.store().is_some()), "no app among the games");
        assert_eq!(rows[1].games.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Notepad"]);
        let installed: Vec<bool> = rows[0].games.iter().map(|e| e.installed()).collect();
        assert!(installed.windows(2).all(|w| w[0] || !w[1]), "installed first: {installed:?}");
        // No stores at all: games and apps cannot be told apart, one Library row as ungrouped.
        let plain = store_library(&[("B", "", "", true), ("A", "", "", true)]);
        let sources = [source(1, "A", true, Some(&plain), None)];
        let entries = merge_library(&sources);
        assert_eq!(home_rows(&entries, HomeGroup::Type), home_rows(&entries, HomeGroup::None));
        // Only games: just the Games row.
        let games = store_library(&[("Portal", "steam", "", true)]);
        let sources = [source(1, "A", true, Some(&games), None)];
        let entries = merge_library(&sources);
        assert_eq!(home_rows(&entries, HomeGroup::Type).iter().map(LibraryRow::key).collect::<Vec<_>>(), ["row:games"]);
        assert!(home_rows(&[], HomeGroup::Type).is_empty());
    }

    #[test]
    fn a_row_shows_what_fits_and_ends_in_the_all_tile_when_more_follow() {
        let lib = mixed();
        let sources = [source(1, "A", true, Some(&lib), None)];
        let entries = merge_library(&sources);
        let steam = &library_rows(&entries)[0];
        assert_eq!(steam.fit(3), (3, false), "three fit whole");
        assert_eq!(steam.fit(8), (3, false));
        assert_eq!(steam.fit(2), (1, true), "one game and the All tile");
        assert_eq!(steam.fit(0), (0, true), "a window too narrow for two still shows the tile");
        assert!(library_filters(&[]).is_empty() && library_rows(&[]).is_empty());
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
        // A game the PC has not installed counts, as it does in Home's rows and their "All N".
        let mut some_missing = library();
        some_missing.games[1].installed = Some(false);
        assert_eq!(host_line(&source(1, "A", true, Some(&some_missing), None)), HostLine::Games(2));
        let entries = merge_library(&[source(1, "A", true, Some(&some_missing), None)]);
        assert_eq!(library_rows(&entries).iter().map(|r| r.games.len()).sum::<usize>(), 2);
    }

    #[test]
    fn the_stream_summary_reads_like_the_android_one() {
        assert_eq!(stream_summary(1920, 1080, 60, 20_000), "1920 × 1080, 60 FPS, 20 Mbps");
        assert_eq!(stream_summary(2560, 1440, 120, 37_500), "2560 × 1440, 120 FPS, 37.5 Mbps");
    }

    /// Android's GameDetailModel: a game the PC lists as not installed cannot be started, but one
    /// that runs or is suspended there can still be resumed.
    #[test]
    fn a_game_the_pc_has_not_installed_cannot_start_but_resumes() {
        let mut lib = library();
        lib.games[0].installed = Some(false); // Dota 2
        let idle = idle();
        let d = detail(&source(1, "A", true, Some(&lib), Some(&idle)), DOTA, true);
        assert_eq!((d.installed, d.can_start), (false, false), "not installed: no Play");
        let suspended = session(); // Dota 2 suspended
        let d = detail(&source(1, "A", true, Some(&lib), Some(&suspended)), DOTA, true);
        assert_eq!((d.installed, d.can_start, d.resumes()), (false, true, true), "a suspended game resumes");
        // Saying nothing either way counts as installed.
        lib.games[0].installed = None;
        let d = detail(&source(1, "A", true, Some(&lib), Some(&idle)), DOTA, true);
        assert_eq!((d.installed, d.can_start), (true, true));
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

    fn recent(host: &str, install: &str, app: u32, title: &str) -> Recent {
        Recent { host: host.into(), install_id: install.into(), app_id: app, title: title.into(), at: 0 }
    }

    #[test]
    fn recently_played_lists_known_games_latest_first() {
        let lib = library();
        let mut a = source(1, "Desk", true, Some(&lib), None);
        a.unique_id = "uid-a";
        let recents = [
            recent("uid-a", "8D4B3F55-0C2E-4F8A-9A61-2F1C6F0E9B17", 0, "old name"),
            recent("uid-a", "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 1093052211, "Dota 2"),
            // A game the PC no longer lists, a PC that is not here, a repeat.
            recent("uid-a", "gone", 5, "Gone"),
            recent("uid-z", "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 1093052211, "Dota 2"),
            recent("uid-a", "", 1093052211, "Dota 2"),
        ];
        let sources = [a];
        let list = recently_played(&sources, &recents, &[], 6);
        // Matched by install id without case, and titled from the library.
        assert_eq!(list.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Desktop", "Dota 2"]);
        assert_eq!((list[0].host, list[0].host_name.as_str(), list[0].online), (1, "Desk", true));
        assert_eq!(recently_played(&sources, &recents, &[], 1).len(), 1, "capped");
    }

    #[test]
    fn a_list_line_names_the_store_and_when_the_game_was_last_played() {
        let lib = library();
        let mut a = source(1, "Desk", true, Some(&lib), None);
        a.unique_id = "uid-a";
        let sources = [a];
        let entries = merge_library(&sources);
        let dota = entries.iter().find(|e| e.title == "Dota 2").expect("Dota 2");
        let at = |r: Recent, at: u64| Recent { at, ..r };
        let recents = [
            at(recent("uid-a", "", 1093052211, "Dota 2"), 1_000),
            at(recent("uid-a", "C6F1A9A4-4B51-5F43-9D0B-7A2B1C2D3E4F", 0, "Dota 2"), 5_000),
            // Another PC's copy of the same game is not this one.
            at(recent("uid-z", "", 1093052211, "Dota 2"), 9_000),
        ];
        assert_eq!(last_played(dota, &sources, &recents), Some(5_000), "the latest, by install id without case");
        assert_eq!(last_played(dota, &sources, &[]), None);
        let store = dota.best().store_name.clone();
        let line = |played, now| list_line(dota, played, now);
        let with_store = |when: &str| match &store { Some(s) => format!("{s} · {when}"), None => when.to_owned() };
        assert_eq!(line(Some(0), 9_000), with_store("Not played yet"), "0 is the host's never played");
        assert_eq!(line(None, 9_000), store.clone().unwrap_or_default(), "unknown says nothing about playing");
        assert_eq!(line(Some(5_000), 5_000 + 3_600), with_store("Played today"));
        assert_eq!(line(Some(5_000), 5_000 + 86_400 + 60), with_store("Played yesterday"));
        assert_eq!(line(Some(5_000), 5_000 + 9 * 86_400), with_store("Played 9 days ago"));
        assert_eq!(line(Some(9_000), 5_000), with_store("Played today"), "a clock that went back reads as today");
    }

    #[test]
    fn recently_played_skips_what_is_under_continue_playing_and_uninstalled_games() {
        let mut lib = library();
        let mut a = source(1, "Desk", true, Some(&lib), None);
        a.unique_id = "uid-a";
        let recents = [recent("uid-a", "", 1093052211, "Dota 2"), recent("uid-a", "", 881448767, "Desktop")];
        let playing = [Continue { host: 1, host_name: "Desk".into(), app_id: 1093052211, title: "Dota 2".into(), playing: Playing::Running }];
        let list = recently_played(&[a], &recents, &playing, 6);
        assert_eq!(list.iter().map(|r| r.app_id).collect::<Vec<_>>(), [881448767]);
        lib.games[1].installed = Some(false);
        let mut a = source(1, "Desk", true, Some(&lib), None);
        a.unique_id = "uid-a";
        assert!(recently_played(&[a], &recents[1..], &[], 6).is_empty(), "an uninstalled game is not offered");
        // A PC that answered no unique id yet matches nothing, and a PC away still lists its games.
        let away = source(2, "Away", false, Some(&lib), None);
        assert!(recently_played(&[away], &recents, &[], 6).is_empty());
        let mut away = source(2, "Away", false, Some(&lib), None);
        away.unique_id = "uid-a";
        let list = recently_played(&[away], &recents[..1], &[], 6);
        assert_eq!((list.len(), list[0].online), (1, false));
    }

    fn star(host: &str, install: &str, app: u32, title: &str) -> Favourite {
        Favourite { host: host.into(), install_id: install.into(), app_id: app, title: title.into() }
    }

    #[test]
    fn favourites_list_the_starred_games_newest_first_titled_from_the_library() {
        let lib = library();
        let mut a = source(1, "Desk", true, Some(&lib), None);
        a.unique_id = "uid-a";
        let mut b = source(2, "Den", false, Some(&lib), None);
        b.unique_id = "uid-b";
        let stars = [
            star("uid-b", "", 881448767, "old name"),
            star("uid-a", "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 0, "Dota 2"),
            // Not listed any more, a PC that is not here, a repeat.
            star("uid-a", "gone", 5, "Gone"),
            star("uid-z", "", 881448767, "Desktop"),
            star("uid-b", "", 881448767, "again"),
        ];
        let sources = [a, b];
        let list = favourite_games(&sources, &stars);
        assert_eq!(list.iter().map(|s| (s.host, s.title.as_str(), s.online)).collect::<Vec<_>>(),
            [(2, "Desktop", false), (1, "Dota 2", true)]);
        // Two PCs with the same game are two favourites, and each star only marks its own PC.
        let game = &lib.games[0];
        assert!(is_starred(&sources[0], game, &stars));
        assert!(!is_starred(&sources[1], game, &stars));
    }

    #[test]
    fn a_hidden_favourite_comes_back_and_a_pc_without_a_library_shows_the_saved_title() {
        let mut lib = library();
        let stars = [star("uid-a", "", 881448767, "Desktop")];
        lib.games[1].installed = Some(false);
        let mut a = source(1, "Desk", true, Some(&lib), None);
        a.unique_id = "uid-a";
        assert!(favourite_games(&[a], &stars).is_empty(), "an uninstalled game is not offered");
        lib.games[1].installed = None;
        let mut a = source(1, "Desk", true, Some(&lib), None);
        a.unique_id = "uid-a";
        assert_eq!(favourite_games(&[a], &stars).len(), 1);
        let mut unknown = source(1, "Desk", false, None, None);
        unknown.unique_id = "uid-a";
        let list = favourite_games(&[unknown], &stars);
        assert_eq!((list[0].title.as_str(), list[0].app_id), ("Desktop", 881448767));
        // A PC that told no unique id yet cannot be starred on.
        let nameless = source(3, "New", true, Some(&lib), None);
        assert!(favourite_of(&nameless, &lib.games[0]).is_none());
    }

    #[test]
    fn the_running_app_is_checked_again_before_it_is_closed() {
        assert_eq!(recheck_running(Some(11), Some(11)), Running::Same);
        assert_eq!(recheck_running(Some(11), None), Running::Gone);
        assert_eq!(recheck_running(Some(11), Some(12)), Running::Changed);
        // Nothing was named (the app list was not loaded): only a stopped app is told apart.
        assert_eq!(recheck_running(None, Some(12)), Running::Same);
        assert_eq!(recheck_running(None, None), Running::Gone);
    }

    #[test]
    fn a_removed_favourite_hands_focus_to_the_next_card_else_the_previous() {
        assert_eq!(neighbour(3, 0), Some(1));
        assert_eq!(neighbour(3, 1), Some(2));
        assert_eq!(neighbour(3, 2), Some(1));
        assert_eq!(neighbour(1, 0), None);
        assert_eq!(neighbour(0, 0), None);
    }
}
