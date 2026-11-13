//! What Home and the game page show, worked out from what the PCs reported. No egui and no
//! network in here, so the rules (which screen to open on, how libraries merge, Play or Resume,
//! when Close game is offered) are tested on their own. Same rules as Nightjar Android's console.

use nightjar_core::http::native_api::library::{Game, Library};
use nightjar_core::http::native_api::session::{ActionId, Session, State};

use crate::strings::{en, tr, trf};
use crate::config::{Favourite, HomeGroup, Recent};
use nightjar_policy::collections;
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
    /// The library is the PC's classic app list ([`classic_library`]): a PC without the native
    /// library shows its games on Home, but does not make Home the start page.
    pub classic: bool,
    /// When the library's games were last played, by install id (`Native::last_played`).
    pub last_played: Option<&'a std::collections::BTreeMap<String, u64>>,
    /// The collections the library's games are in, by install id (`Native::collections`).
    pub collections: Option<&'a std::collections::BTreeMap<String, Vec<String>>>,
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
            library: host.library().filter(|_| host.paired()),
            classic: host.native.library.is_none() && host.classic.is_some(),
            last_played: host.paired().then_some(&host.native.last_played),
            collections: host.paired().then_some(&host.native.collections),
            session: host.native.session.as_ref().filter(|_| online),
            host_control: host.native.host_control(),
        }
    }
}

/// Whether the app starts on Home: the setting is on and some PC has a native library to show,
/// live or saved from an earlier run. App lists alone leave the list of PCs the start page, as on
/// Android.
pub fn opens_on_home(enabled: bool, sources: &[Source<'_>]) -> bool {
    enabled && has_native_library(sources)
}

/// Whether Home has games to show and is offered: some PC's native library, or the app list of a
/// paired PC without one (Android's `ShellBar.homeAvailable`).
pub fn has_library(sources: &[Source<'_>]) -> bool {
    sources.iter().any(|s| s.library.is_some_and(|l| !l.games.is_empty()))
}

/// [`has_library`] counting only native libraries: what makes Home the start page.
pub fn has_native_library(sources: &[Source<'_>]) -> bool {
    sources.iter().any(|s| !s.classic && s.library.is_some_and(|l| !l.games.is_empty()))
}

/// The revision of a library made from an app list; a native library's comes from its host.
pub const CLASSIC_REVISION: &str = "classic";

/// A PC's app list as a library, so Home, All games, the game pages and their menus show a PC
/// without the native library as Android's console does (`ConsoleRepository`, Status.CLASSIC).
/// Each app is its own entry (no store, no game id to merge by), keyed by its UUID and app id as
/// the PC's own app list keys stars and hidden games; a start goes through that list. `running`
/// is the app the PC reports (`currentgame`, its UUID deciding when both have one).
pub fn classic_library(apps: &crate::config::ClassicApps, running: Option<(i32, Option<&str>)>) -> Library {
    let games = apps
        .apps
        .iter()
        .map(|app| Game {
            install_id: app.uuid.clone(),
            app_id: app.id as u32,
            title: app.name.clone(),
            game_id: None,
            store: None,
            installed: None,
            running: running.is_some_and(|(id, uuid)| classic_running(app, id, uuid)),
            artwork: nightjar_core::http::native_api::library::Artwork { cover: Some(String::new()), version: app.art_version.clone() },
        })
        .collect();
    Library { revision: CLASSIC_REVISION.into(), games }
}

/// `App::matches_running_app` for a saved app.
fn classic_running(app: &crate::config::ClassicApp, running_id: i32, running_uuid: Option<&str>) -> bool {
    match running_uuid {
        Some(uuid) if !app.uuid.is_empty() && !uuid.is_empty() => app.uuid == uuid,
        _ => running_id != 0 && app.id == running_id,
    }
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
    pub has_cover: bool,
    /// When the PC says the game was last played (seconds since the epoch, `0` for never), None
    /// when it says nothing.
    pub last_played: Option<u64>,
    /// The collections its PC keeps it in, by name.
    pub collections: Vec<String>,
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

    /// The collections any of its PCs keeps it in, as they name them.
    pub fn collections(&self) -> Vec<String> {
        self.copies.iter().flat_map(|c| c.collections.iter().cloned()).collect()
    }

    fn in_collection(&self, name: &str) -> bool {
        let name = name.trim().to_lowercase();
        self.copies.iter().flat_map(|c| &c.collections).any(|c| c.trim().to_lowercase() == name)
    }

    fn in_no_collection(&self) -> bool {
        self.copies.iter().flat_map(|c| &c.collections).all(|c| c.trim().is_empty())
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
    /// One collection's games, by its name.
    Collection(String),
    /// The games in no collection; offered only beside collections.
    NoCollection,
    /// The games hidden on this device; offered only when there are any.
    Hidden,
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
            Self::Collection(name) => entry.in_collection(name),
            Self::NoCollection => entry.in_no_collection(),
            Self::Hidden => true,
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
            Self::Collection(name) => format!("filter:collection:{}", name.trim().to_lowercase()),
            Self::NoCollection => "filter:no-collection".into(),
            Self::Hidden => "filter:hidden".into(),
        }
    }
}

/// One chip: the filter, the store's or collection's name for such a chip, and how many games it
/// shows.
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
/// has both), each collection by name, then the games in none. None for an empty library.
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
    for group in collection_groups(entries) {
        let count = group.games.len();
        chips.push(match group.name {
            Some(name) => FilterChip { filter: LibraryFilter::Collection(name.clone()), store_name: Some(name), count },
            None => FilterChip { filter: LibraryFilter::NoCollection, store_name: None, count },
        });
    }
    chips
}

fn collection_groups(entries: &[Entry]) -> Vec<collections::Group> {
    let names: Vec<Vec<String>> = entries.iter().map(Entry::collections).collect();
    collections::groups(&names)
}

/// The grouping Home shows under the chosen one: by collection only while its PCs name any. The
/// choice itself stays, for when they do again.
pub fn shown_group(group: HomeGroup, entries: &[Entry]) -> HomeGroup {
    if group == HomeGroup::Collection && entries.iter().all(Entry::in_no_collection) {
        HomeGroup::Store
    } else {
        group
    }
}

/// The groupings the View menu steps through: Collection only while the PCs name any.
pub fn offered_groups(entries: &[Entry]) -> Vec<HomeGroup> {
    HomeGroup::ALL.into_iter().filter(|group| shown_group(*group, entries) == *group).collect()
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
    /// One collection's games (Home grouped by collection).
    Collection { name: String },
    /// The games in no collection, after the collections.
    NoCollection,
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
            RowKind::Collection { name } => format!("row:collection:{}", name.to_lowercase()),
            RowKind::NoCollection => "row:no-collection".into(),
        }
    }

}

/// `games` shown in a line of `room` covers: all that fit, else one fewer and the "All N" tile in
/// the last place. Returns how many games to draw and whether the tile follows.
pub fn fit_row(games: usize, room: usize) -> (usize, bool) {
    let room = room.max(1);
    if games <= room {
        (games, false)
    } else {
        (room - 1, true)
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
/// tells games from apps. A library with no stores at all is one Library row by type too. By
/// collection, a row per collection by name, a game in several in each, then the games in none;
/// by store while no game is in one ([`shown_group`]).
pub fn home_rows(entries: &[Entry], group: HomeGroup) -> Vec<LibraryRow<'_>> {
    let group = shown_group(group, entries);
    if group == HomeGroup::Store {
        return library_rows(entries);
    }
    if group == HomeGroup::Collection {
        return collection_groups(entries)
            .into_iter()
            .map(|part| {
                let games = installed_first(part.games.iter().map(|at| &entries[*at]).collect());
                match part.name {
                    Some(name) => LibraryRow { filter: LibraryFilter::Collection(name.clone()), kind: RowKind::Collection { name }, games },
                    None => LibraryRow { kind: RowKind::NoCollection, filter: LibraryFilter::NoCollection, games },
                }
            })
            .collect();
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
pub(crate) fn active(session: &Session) -> bool {
    live(session) && session.app.is_some()
}

/// Whether a session is under way, whoever's. The host does not name another device's app to this one.
pub(crate) fn live(session: &Session) -> bool {
    matches!(session.state, State::Preparing | State::Launching | State::Streaming | State::Suspended)
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

/// The key of the entry a PC's copy of a game is in.
pub fn entry_key_of(entries: &[Entry], host: u64, app_id: u32) -> Option<&String> {
    entries.iter().find(|entry| entry.copies.iter().any(|copy| copy.host == host && copy.app_id == app_id)).map(|entry| &entry.key)
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
                has_cover: game.artwork.cover.is_some(),
                last_played: source.last_played.and_then(|played| played.get(&game.install_id).copied()),
                collections: source.collections.and_then(|names| names.get(&game.install_id).cloned()).unwrap_or_default(),
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

/// Whether game `app_id` of PC `host` is not hidden on this device; one its PC does not list has
/// nothing to hide it by.
pub fn not_hidden(sources: &[Source<'_>], host: u64, app_id: u32, hidden: &[Favourite]) -> bool {
    let source = sources.iter().find(|s| s.id == host);
    let game = source.and_then(|s| s.library).and_then(|l| l.games.iter().find(|g| g.app_id == app_id));
    source.zip(game).is_none_or(|(s, g)| !is_hidden(s, &g.install_id, app_id, hidden))
}

/// [`recently_played`] without the games this device hides, then capped: a hidden game takes no
/// place in the row.
pub fn recently_shown(sources: &[Source<'_>], recents: &[Recent], playing: &[Continue], hidden: &[Favourite], limit: usize) -> Vec<Recently> {
    let mut shown = recently_played(sources, recents, playing, usize::MAX);
    shown.retain(|r| not_hidden(sources, r.host, r.app_id, hidden));
    shown.truncate(limit);
    shown
}

/// When the entry was last played, in seconds since the epoch (`0` for never): the newest of what
/// its copies' PCs say, as on Android. Only when no PC says, this device's own recent games (they
/// keep the last few only), matched as "Recently played" matches them (install id, else app id).
pub fn last_played(entry: &Entry, sources: &[Source<'_>], recents: &[Recent]) -> Option<u64> {
    if let Some(at) = entry.copies.iter().filter_map(|copy| copy.last_played).max() {
        return Some(at);
    }
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

/// What keeps game `app_id` hidden on the PC `source` when `install_id` is its install id: the
/// same keys as a star, with no title to show.
fn hide_key(source: &Source<'_>, install_id: &str, app_id: u32) -> Option<Favourite> {
    (!source.unique_id.is_empty()).then(|| Favourite { host: source.unique_id.to_owned(), install_id: install_id.to_owned(), app_id, title: String::new() })
}

/// Whether the game is hidden on this device. A PC that has not told its unique id hides nothing.
pub fn is_hidden(source: &Source<'_>, install_id: &str, app_id: u32, hidden: &[Favourite]) -> bool {
    hide_key(source, install_id, app_id).is_some_and(|key| hidden.iter().any(|h| h.same_hidden(&key)))
}

/// The library apart: what Home and All games show, and what this device hides. A title hidden
/// on one PC still shows for its copies on the others.
pub fn split_hidden(entries: Vec<Entry>, sources: &[Source<'_>], hidden: &[Favourite]) -> (Vec<Entry>, Vec<Entry>) {
    if hidden.is_empty() {
        return (entries, Vec::new());
    }
    let (mut shown, mut away) = (Vec::new(), Vec::new());
    for entry in entries {
        let (gone, kept): (Vec<Copy>, Vec<Copy>) = entry.copies.into_iter().partition(|copy| {
            sources.iter().find(|s| s.id == copy.host).is_some_and(|s| is_hidden(s, &copy.install_id, copy.app_id, hidden))
        });
        if !kept.is_empty() {
            shown.push(Entry { key: entry.key.clone(), title: entry.title.clone(), copies: kept });
        }
        if !gone.is_empty() {
            away.push(Entry { key: entry.key, title: entry.title, copies: gone });
        }
    }
    (shown, away)
}

/// Every PC's games merged ([`merge_library`]), and the same apart by what this device hides
/// ([`split_hidden`]).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Merged {
    all: Vec<Entry>,
    /// What shows and what is hidden, only when something is: with nothing hidden all of it shows.
    split: Option<(Vec<Entry>, Vec<Entry>)>,
}

impl Merged {
    fn of(sources: &[Source<'_>], hidden: &[Favourite]) -> Self {
        let all = merge_library(sources);
        let split = (!hidden.is_empty()).then(|| split_hidden(all.clone(), sources, hidden));
        Self { all, split }
    }

    /// Every game, the hidden ones too.
    pub fn all(&self) -> &[Entry] {
        &self.all
    }

    /// What Home and All games show.
    pub fn shown(&self) -> &[Entry] {
        self.split.as_ref().map_or(self.all.as_slice(), |(shown, _)| shown.as_slice())
    }

    /// What this device hides.
    pub fn hidden(&self) -> &[Entry] {
        self.split.as_ref().map_or(&[][..], |(_, hidden)| hidden.as_slice())
    }
}

/// One PC as the last merge saw it: a [`Source`] that owns what it showed.
#[derive(PartialEq)]
struct Seen {
    id: u64,
    unique_id: String,
    name: String,
    online: bool,
    paired: bool,
    library: Option<Library>,
    classic: bool,
    last_played: Option<std::collections::BTreeMap<String, u64>>,
    collections: Option<std::collections::BTreeMap<String, Vec<String>>>,
    session: Option<Session>,
    host_control: bool,
}

impl Seen {
    // Both ways take a `Source` apart by name, with no `..`: a field added to it does not build
    // until it is kept and compared here, so the merge cannot come to read something this misses.
    fn of(source: &Source<'_>) -> Self {
        let Source { id, unique_id, name, online, paired, library, classic, last_played, collections, session, host_control } = *source;
        Self {
            id,
            unique_id: unique_id.to_owned(),
            name: name.to_owned(),
            online,
            paired,
            library: library.cloned(),
            classic,
            last_played: last_played.cloned(),
            collections: collections.cloned(),
            session: session.cloned(),
            host_control,
        }
    }

    fn is(&self, source: &Source<'_>) -> bool {
        let Source { id, unique_id, name, online, paired, library, classic, last_played, collections, session, host_control } = *source;
        self.id == id
            && self.unique_id == unique_id
            && self.name == name
            && self.online == online
            && self.paired == paired
            && self.host_control == host_control
            && self.classic == classic
            && self.session.as_ref() == session
            && self.last_played.as_ref() == last_played
            && self.collections.as_ref() == collections
            && self.library.as_ref() == library
    }
}

/// The merged library, kept between frames. Merging allocates for every game of every PC and
/// sorts them, and Home, All games, a game's page and Home's View menu each asked for it on every
/// frame they drew.
///
/// The merge and the split read nothing but their `Source`s and the hidden list, so the kept one
/// holds for exactly as long as those compare equal to the ones it was made from: all of them,
/// whether the merge reads a field today or not. Comparing costs no allocation and is a small
/// part of a merge; a PC's answer that changes anything, wherever it was written, shows on the
/// next frame.
#[derive(Default)]
pub struct LibraryMemo {
    kept: std::cell::RefCell<Kept>,
}

#[derive(Default)]
struct Kept {
    seen: Vec<Seen>,
    hidden: Vec<Favourite>,
    merged: std::rc::Rc<Merged>,
    /// How often the library was merged, for the tests.
    merges: u64,
}

impl LibraryMemo {
    /// The library of `sources` with `hidden` taken out: the kept one when both are what it was
    /// made from, else merged now.
    pub fn get(&self, sources: &[Source<'_>], hidden: &[Favourite]) -> std::rc::Rc<Merged> {
        let mut kept = self.kept.borrow_mut();
        let same = kept.merges > 0
            && kept.hidden == hidden
            && kept.seen.len() == sources.len()
            && kept.seen.iter().zip(sources).all(|(seen, source)| seen.is(source));
        if !same {
            kept.merged = std::rc::Rc::new(Merged::of(sources, hidden));
            kept.seen = sources.iter().map(Seen::of).collect();
            kept.hidden = hidden.to_vec();
            kept.merges += 1;
        }
        kept.merged.clone()
    }

    #[cfg(test)]
    pub fn merges(&self) -> u64 {
        self.kept.borrow().merges
    }
}

/// What hiding the game `app_id` of PC `host` stores: every copy of its title, so the card it is
/// on goes from Home whichever PCs have it. Empty when the game is not listed.
pub fn hide_keys(sources: &[Source<'_>], host: u64, app_id: u32) -> Vec<Favourite> {
    let entries = merge_library(sources);
    let Some(entry) = entries.iter().find(|e| e.copies.iter().any(|c| c.host == host && c.app_id == app_id)) else {
        return Vec::new();
    };
    entry
        .copies
        .iter()
        .filter_map(|copy| {
            let source = sources.iter().find(|s| s.id == copy.host)?;
            let key = hide_key(source, &copy.install_id, copy.app_id)?;
            Some(Favourite { title: entry.title.clone(), ..key })
        })
        .collect()
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

/// The same for a game saved earlier, which nothing on screen vouches for: an app id may belong to another
/// game by now, so a saved install id the list lacks means the game is not listed. Only an entry with no UUID of
/// its own goes by the app id: the library names a game without one by a UUID from its name, and the list by nothing.
pub fn saved_classic_app(apps: &[(i32, &str)], install_id: &str, app_id: u32) -> Option<i32> {
    if install_id.is_empty() {
        return classic_app(apps, install_id, app_id);
    }
    apps.iter()
        .find(|(_, uuid)| !uuid.is_empty() && uuid.eq_ignore_ascii_case(install_id))
        .or_else(|| apps.iter().find(|(id, uuid)| uuid.is_empty() && *id as u32 == app_id))
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests;
