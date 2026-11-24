//! Unit tests for the merged console library, home rows, filtering, and session actions.

use super::*;
use rubylight_native_api::capabilities::Capabilities;

const LIBRARY: &str = include_str!("../../../native-api/tests/fixtures/library.json");
const SESSION: &str = include_str!("../../../native-api/tests/fixtures/session.json");
const CAPABILITIES: &str = include_str!("../../../native-api/tests/fixtures/capabilities.json");

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
    Source { id, unique_id: "", name, online, paired: true, library, classic: false, last_played: None, playtime_minutes: None, collections: None, session, host_control: false }
}

const DOTA: u32 = 1093052211;
const DESKTOP: u32 = 881448767;

fn named(pairs: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Vec<String>> {
    pairs.iter().map(|(id, names)| ((*id).to_owned(), names.iter().map(|n| (*n).to_owned()).collect())).collect()
}

/// The fixture's first game in the collections, the others in none.
fn in_collections(lib: &Library) -> std::collections::BTreeMap<String, Vec<String>> {
    named(&[(lib.games[0].install_id.as_str(), &["VR", "Couch"])])
}

#[test]
fn home_groups_by_collection_with_a_game_in_each_and_the_rest_last() {
    let lib = library();
    let names = in_collections(&lib);
    let sources = [Source { collections: Some(&names), ..source(1, "A", true, Some(&lib), None) }];
    let entries = merge_library(&sources);
    let rows = home_rows(&entries, HomeGroup::Collection, Sort::default(), unplayed());
    let kinds: Vec<&RowKind> = rows.iter().map(|r| &r.kind).collect();
    assert_eq!(kinds, [&RowKind::Collection { name: "Couch".into() }, &RowKind::Collection { name: "VR".into() }, &RowKind::NoCollection]);
    assert_eq!(rows[0].games[0].title, lib.games[0].title);
    assert_eq!(rows[1].games[0].title, lib.games[0].title, "in each of its collections");
    assert_eq!(rows[2].games.len(), entries.len() - 1, "every other game once, last");
    assert_eq!(rows[0].filter, LibraryFilter::Collection("Couch".into()));
    assert_eq!(filtered(&entries, &rows[0].filter, Sort::default(), unplayed()).len(), 1);
    assert_eq!(filtered(&entries, &LibraryFilter::NoCollection, Sort::default(), unplayed()).len(), entries.len() - 1);
    let chips = library_filters(&entries);
    assert!(chips.iter().any(|c| c.filter == LibraryFilter::Collection("VR".into()) && c.count == 1), "{chips:?}");
    assert!(chips.iter().any(|c| c.filter == LibraryFilter::NoCollection), "{chips:?}");
    assert!(offered_groups(&entries).contains(&HomeGroup::Collection));
}

#[test]
fn with_no_collections_the_grouping_is_not_offered_and_a_saved_choice_shows_stores() {
    let lib = library();
    let sources = [source(1, "A", true, Some(&lib), None)];
    let entries = merge_library(&sources);
    assert!(!offered_groups(&entries).contains(&HomeGroup::Collection));
    assert_eq!(shown_group(HomeGroup::Collection, &entries), HomeGroup::Store);
    let chosen: Vec<RowKind> = home_rows(&entries, HomeGroup::Collection, Sort::default(), unplayed()).into_iter().map(|r| r.kind).collect();
    let by_store: Vec<RowKind> = home_rows(&entries, HomeGroup::Store, Sort::default(), unplayed()).into_iter().map(|r| r.kind).collect();
    assert_eq!(chosen, by_store);
    assert!(!library_filters(&entries).iter().any(|c| matches!(c.filter, LibraryFilter::Collection(_) | LibraryFilter::NoCollection)));
}

#[test]
fn hiding_takes_every_copy_of_the_title_and_a_pc_with_no_id_hides_nothing() {
    let lib = library();
    let game = &lib.games[0];
    let (one, two) = (Source { unique_id: "one", ..source(1, "A", true, Some(&lib), None) }, Source { unique_id: "two", ..source(2, "B", true, Some(&lib), None) });
    let sources = [one, two];
    let keys = hide_keys(&sources, 1, game.app_id);
    assert_eq!(keys.iter().map(|k| k.host.as_str()).collect::<Vec<_>>(), ["one", "two"], "{keys:?}");
    assert!(hide_keys(&sources, 1, 12345).is_empty(), "not listed");

    let (shown, away) = split_hidden(merge_library(&sources), &sources, &keys);
    assert!(!shown.iter().any(|e| e.title == game.title));
    assert_eq!(away.len(), 1);
    assert_eq!(away[0].copies.len(), 2);

    // Hidden on one PC, its copy on the other still shows.
    let (shown, away) = split_hidden(merge_library(&sources), &sources, &keys[..1]);
    assert_eq!(shown.iter().find(|e| e.title == game.title).map(|e| e.copies[0].host), Some(2));
    assert_eq!(away[0].copies[0].host, 1);

    // The PC's list knows the same game by its classic UUID.
    assert!(is_hidden(&sources[0], "uuid-of-the-pcs-list", game.app_id, &keys));

    let nameless = [source(1, "A", true, Some(&lib), None)];
    assert!(hide_keys(&nameless, 1, game.app_id).is_empty());
    assert!(!is_hidden(&nameless[0], &game.install_id, game.app_id, &keys));
}

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

fn classic_apps(apps: &[(i32, &str, &str)]) -> crate::config::ClassicApps {
    crate::config::ClassicApps {
        version: crate::config::CLASSIC_APPS_VERSION,
        apps: apps
            .iter()
            .map(|(id, name, uuid)| crate::config::ClassicApp { id: *id, name: (*name).into(), uuid: (*uuid).into(), art_version: format!("art{id}") })
            .collect(),
    }
}

fn classic_source<'a>(id: u64, unique_id: &'a str, online: bool, library: &'a Library) -> Source<'a> {
    Source { unique_id, classic: true, ..source(id, "Vibe", online, Some(library), None) }
}

#[test]
fn an_app_list_offers_home_but_only_a_native_library_opens_on_it() {
    let lib = library();
    let apps = classic_library(&classic_apps(&[(1, "Desktop", "D-1"), (2, "Hades", "")]), None);
    let empty = classic_library(&classic_apps(&[]), None);
    let classic = [classic_source(1, "pc", false, &apps)];
    // Android: Home appears once a PC's app list is saved, but the list of PCs stays the start.
    assert!(has_library(&classic), "an app list, saved while the PC sleeps, offers Home");
    assert!(!has_native_library(&classic));
    assert!(!opens_on_home(true, &classic), "app lists alone start on the list of PCs");
    assert!(!has_library(&[classic_source(1, "pc", true, &empty)]), "an empty list offers nothing");
    let both = [classic_source(1, "pc", true, &apps), source(2, "Rubylight", true, Some(&lib), None)];
    assert!(opens_on_home(true, &both), "a native library beside it still opens on Home");
}

#[test]
fn an_app_list_is_a_library_of_its_own_entries_with_the_running_app_marked() {
    let apps = classic_apps(&[(1, "Desktop", "D-1"), (2, "Hades", ""), (3, "Celeste", "C-3")]);
    let lib = classic_library(&apps, None);
    assert_eq!(lib.revision, CLASSIC_REVISION);
    let game = &lib.games[1];
    assert_eq!((game.app_id, game.title.as_str(), game.install_id.as_str(), game.installed, game.running), (2, "Hades", "", None, false));
    assert!(game.game_id.is_none() && game.store.is_none(), "nothing to merge by or shelve under");
    assert_eq!((game.artwork.cover.is_some(), game.artwork.version.as_str()), (true, "art2"), "the box art, at the list's version");
    // currentgame names the app; a UUID decides where both have one.
    let by_id = classic_library(&apps, Some((2, None)));
    assert_eq!(by_id.games.iter().map(|g| g.running).collect::<Vec<_>>(), [false, true, false]);
    let by_uuid = classic_library(&apps, Some((1, Some("C-3"))));
    assert_eq!(by_uuid.games.iter().map(|g| g.running).collect::<Vec<_>>(), [false, false, true]);
    assert!(classic_library(&apps, Some((0, None))).games.iter().all(|g| !g.running), "0 is no game");

    // Home: one entry per app (none merge with a native copy), Play or Resume and a classic quit.
    let running = classic_library(&apps, Some((2, None)));
    let sources = [classic_source(1, "pc", true, &running), classic_source(2, "pc2", true, &running)];
    let merged = merge_library(&sources);
    assert_eq!(merged.len(), 6, "each PC's apps are their own");
    assert_eq!(merged.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Celeste", "Celeste", "Desktop", "Desktop", "Hades", "Hades"]);
    let hades = merged.iter().find(|e| e.title == "Hades").unwrap();
    assert!(hades.running() && hades.installed() && hades.cover().is_some() && hades.store().is_none());
    let playing = continue_playing(&sources);
    assert_eq!(playing.iter().map(|c| (c.host, c.app_id, c.playing)).collect::<Vec<_>>(), [(1, 2, Playing::Running), (2, 2, Playing::Running)]);
    let d = detail(&sources[0], 2, false);
    assert_eq!((d.state, d.end, d.can_start), (GameState::Running, EndAction::Quit, true));
    let other = detail(&sources[0], 3, false);
    assert_eq!((other.state, other.other_running.as_deref()), (GameState::Idle, Some("Hades")));
    // Away: the saved list shows, nothing runs and nothing starts.
    let away = [classic_source(1, "pc", false, &running)];
    assert!(continue_playing(&away).is_empty());
    let d = detail(&away[0], 2, false);
    assert_eq!((d.state, d.can_start), (GameState::Idle, false));
    assert_eq!(host_line(&away[0]), HostLine::SavedGames(3));
}

#[test]
fn hiding_and_starring_an_app_list_entry_use_the_pcs_app_keys() {
    let lib = classic_library(&classic_apps(&[(1, "Desktop", "D-1"), (2, "Hades", "")]), None);
    let sources = [classic_source(1, "pc", true, &lib)];
    // The same key the PC's own app list hides by: its unique id, the app's UUID and id.
    let keys = hide_keys(&sources, 1, 1);
    assert_eq!(keys, [Favourite { host: "pc".into(), install_id: "D-1".into(), app_id: 1, title: "Desktop".into() }]);
    let (shown, hidden) = split_hidden(merge_library(&sources), &sources, &keys);
    assert_eq!((shown.len(), hidden[0].title.as_str()), (1, "Desktop"));
    let star = favourite_for(&sources, 1, 2).unwrap();
    assert_eq!((star.host.as_str(), star.install_id.as_str(), star.app_id), ("pc", "", 2));
    let stars = favourite_games(&sources, &[star]);
    assert_eq!(stars.iter().map(|s| (s.host, s.app_id, s.title.as_str())).collect::<Vec<_>>(), [(1, 2, "Hades")]);
    // A recent stream of it shows under Recently played.
    let recent = Recent { host: "pc".into(), install_id: "D-1".into(), app_id: 1, title: "Old".into(), at: 5 };
    let recents = recently_played(&sources, &[recent], &[], 5);
    assert_eq!(recents.iter().map(|r| (r.app_id, r.title.as_str())).collect::<Vec<_>>(), [(1, "Desktop")]);
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
            store: (!key.is_empty()).then(|| rubylight_native_api::library::Store {
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
            (LibraryFilter::NotInstalled, None, 2),
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
        assert_eq!(filtered(&entries, &chip.filter, Sort::default(), unplayed()).len(), chip.count, "{:?}", chip.filter);
    }
    let titles = |f: &LibraryFilter| filtered(&entries, f, Sort::default(), unplayed()).iter().map(|e| e.title.as_str()).collect::<Vec<_>>();
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
    let rows = library_rows(&entries, Sort::default(), unplayed());
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
    let rows = library_rows(&entries, Sort::default(), unplayed());
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
    let rows = library_rows(&entries, Sort::default(), unplayed());
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].kind.clone(), rows[0].filter.clone()), (RowKind::Library, LibraryFilter::All));
    assert_eq!(library_filters(&entries).len(), 1, "only All: no Other chip without stores");
}

#[test]
fn home_rows_not_by_store_are_one_library_row_of_every_game_installed_first() {
    let lib = mixed();
    let sources = [source(1, "A", true, Some(&lib), None)];
    let entries = merge_library(&sources);
    assert_eq!(home_rows(&entries, HomeGroup::Store, Sort::default(), unplayed()), library_rows(&entries, Sort::default(), unplayed()));
    let rows = home_rows(&entries, HomeGroup::None, Sort::default(), unplayed());
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].kind.clone(), rows[0].filter.clone(), rows[0].key()), (RowKind::Library, LibraryFilter::All, "row:library".to_owned()));
    assert_eq!(rows[0].games.len(), entries.len(), "every game, whatever its store");
    let installed: Vec<bool> = rows[0].games.iter().map(|e| e.installed()).collect();
    assert!(installed.windows(2).all(|w| w[0] || !w[1]), "installed first: {installed:?}");
    assert!(installed.contains(&false), "the fixture has a game that is not installed");
    assert!(home_rows(&[], HomeGroup::None, Sort::default(), unplayed()).is_empty());
}

#[test]
fn home_rows_by_type_are_every_stores_games_then_the_apps_of_no_store() {
    let lib = mixed();
    let sources = [source(1, "A", true, Some(&lib), None)];
    let entries = merge_library(&sources);
    let rows = home_rows(&entries, HomeGroup::Type, Sort::default(), unplayed());
    let got: Vec<(String, LibraryFilter)> = rows.iter().map(|r| (r.key(), r.filter.clone())).collect();
    assert_eq!(got, [("row:games".to_owned(), LibraryFilter::Games), ("row:other".to_owned(), LibraryFilter::Other)]);
    // Each row is exactly its All tile's chip: the same games, the same count.
    let chips = library_filters(&entries);
    for row in &rows {
        let chip = chips.iter().find(|c| c.filter == row.filter).expect("the row's chip");
        assert_eq!(chip.count, row.games.len(), "{:?}", row.filter);
        let mut shown: Vec<&str> = filtered(&entries, &row.filter, Sort::default(), unplayed()).iter().map(|e| e.title.as_str()).collect();
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
    assert_eq!(home_rows(&entries, HomeGroup::Type, Sort::default(), unplayed()), home_rows(&entries, HomeGroup::None, Sort::default(), unplayed()));
    // Only games: just the Games row.
    let games = store_library(&[("Portal", "steam", "", true)]);
    let sources = [source(1, "A", true, Some(&games), None)];
    let entries = merge_library(&sources);
    assert_eq!(home_rows(&entries, HomeGroup::Type, Sort::default(), unplayed()).iter().map(LibraryRow::key).collect::<Vec<_>>(), ["row:games"]);
    assert!(home_rows(&[], HomeGroup::Type, Sort::default(), unplayed()).is_empty());
}

#[test]
fn a_row_shows_what_fits_and_ends_in_the_all_tile_when_more_follow() {
    let lib = mixed();
    let sources = [source(1, "A", true, Some(&lib), None)];
    let entries = merge_library(&sources);
    let steam = &library_rows(&entries, Sort::default(), unplayed())[0];
    assert_eq!(fit_row(steam.games.len(), 3), (3, false), "three fit whole");
    assert_eq!(fit_row(steam.games.len(), 8), (3, false));
    assert_eq!(fit_row(steam.games.len(), 2), (1, true), "one game and the All tile");
    assert_eq!(fit_row(steam.games.len(), 0), (0, true), "a window too narrow for two still shows the tile");
    assert!(library_filters(&[]).is_empty() && library_rows(&[], Sort::default(), unplayed()).is_empty());
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

fn unplayed() -> Played<'static> {
    Played::default()
}

mod history_and_cache_tests;
