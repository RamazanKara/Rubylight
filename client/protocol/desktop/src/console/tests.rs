use super::*;
use nightjar_core::http::native_api::capabilities::Capabilities;

const LIBRARY: &str = include_str!("../../../client/tests/fixtures/native-api/library.json");
const SESSION: &str = include_str!("../../../client/tests/fixtures/native-api/session.json");
const CAPABILITIES: &str = include_str!("../../../client/tests/fixtures/native-api/capabilities.json");

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
    Source { id, unique_id: "", name, online, paired: true, library, classic: false, last_played: None, collections: None, session, host_control: false }
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
    let rows = home_rows(&entries, HomeGroup::Collection);
    let kinds: Vec<&RowKind> = rows.iter().map(|r| &r.kind).collect();
    assert_eq!(kinds, [&RowKind::Collection { name: "Couch".into() }, &RowKind::Collection { name: "VR".into() }, &RowKind::NoCollection]);
    assert_eq!(rows[0].games[0].title, lib.games[0].title);
    assert_eq!(rows[1].games[0].title, lib.games[0].title, "in each of its collections");
    assert_eq!(rows[2].games.len(), entries.len() - 1, "every other game once, last");
    assert_eq!(rows[0].filter, LibraryFilter::Collection("Couch".into()));
    assert_eq!(filtered(&entries, &rows[0].filter).len(), 1);
    assert_eq!(filtered(&entries, &LibraryFilter::NoCollection).len(), entries.len() - 1);
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
    let chosen: Vec<RowKind> = home_rows(&entries, HomeGroup::Collection).into_iter().map(|r| r.kind).collect();
    let by_store: Vec<RowKind> = home_rows(&entries, HomeGroup::Store).into_iter().map(|r| r.kind).collect();
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
    assert_eq!(fit_row(steam.games.len(), 3), (3, false), "three fit whole");
    assert_eq!(fit_row(steam.games.len(), 8), (3, false));
    assert_eq!(fit_row(steam.games.len(), 2), (1, true), "one game and the All tile");
    assert_eq!(fit_row(steam.games.len(), 0), (0, true), "a window too narrow for two still shows the tile");
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

#[test]
fn a_saved_game_whose_install_id_is_gone_is_not_listed_unless_its_entry_has_no_uuid() {
    let apps = [(11, ""), (12, "C6F1A9A4-4B51-5F43-9D0B-7A2B1C2D3E4F"), (13, "another-game")];
    assert_eq!(saved_classic_app(&apps, "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f", 99), Some(12));
    assert_eq!(saved_classic_app(&apps, "gone", 13), None);
    assert_eq!(saved_classic_app(&apps, "gone", 11), Some(11), "the entry has no UUID to tell it by");
    assert_eq!(saved_classic_app(&apps, "gone", 99), None);
    assert_eq!(saved_classic_app(&apps, "", 13), Some(13), "saved without one: the app id is all there is");
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
    // What the PCs say comes first: the newest across the copies, then never (0) over the
    // recents, which keep only the last few.
    let mut b = source(2, "Den", false, Some(&lib), None);
    b.unique_id = "uid-b";
    let install = &lib.games.iter().find(|g| g.title == "Dota 2").unwrap().install_id;
    let (older, newer) = (std::collections::BTreeMap::from([(install.clone(), 6_000)]), std::collections::BTreeMap::from([(install.clone(), 8_000)]));
    let mut a2 = source(1, "Desk", true, Some(&lib), None);
    a2.unique_id = "uid-a";
    let mut b2 = b;
    a2.last_played = Some(&older);
    b2.last_played = Some(&newer);
    let both = [a2, b2];
    let merged = merge_library(&both);
    let dota2 = merged.iter().find(|e| e.title == "Dota 2").expect("Dota 2");
    assert_eq!(dota2.copies.len(), 2);
    assert_eq!(last_played(dota2, &both, &recents), Some(8_000), "the newest a PC says, not the recents");
    let never = std::collections::BTreeMap::from([(install.clone(), 0)]);
    let mut a3 = source(1, "Desk", true, Some(&lib), None);
    a3.unique_id = "uid-a";
    a3.last_played = Some(&never);
    let one = [a3];
    let merged = merge_library(&one);
    let dota3 = merged.iter().find(|e| e.title == "Dota 2").expect("Dota 2");
    assert_eq!(last_played(dota3, &one, &recents), Some(0), "the PC says never played");
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
fn a_hidden_game_takes_no_place_among_the_recent_ones() {
    let lib = library();
    let mut a = source(1, "Desk", true, Some(&lib), None);
    a.unique_id = "uid-a";
    let sources = [a];
    let recents = [recent("uid-a", "", 1093052211, "Dota 2"), recent("uid-a", "", 881448767, "Desktop")];
    let keys = hide_keys(&sources, 1, 1093052211);
    assert_eq!(keys.len(), 1);
    let list = recently_shown(&sources, &recents, &[], &keys, 1);
    assert_eq!(list.iter().map(|r| r.app_id).collect::<Vec<_>>(), [881448767], "the next one has its place");
    assert_eq!(recently_shown(&sources, &recents, &[], &[], 1)[0].app_id, 1093052211);
    assert!(recently_shown(&sources, &recents[..1], &[], &keys, 6).is_empty());
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

/// Everything a [`Source`] shows of one PC, owned, so a test can change any of it.
#[derive(Clone)]
struct Pc {
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

fn sources_of(pcs: &[Pc]) -> Vec<Source<'_>> {
    pcs.iter()
        .map(|pc| Source {
            id: pc.id,
            unique_id: &pc.unique_id,
            name: &pc.name,
            online: pc.online,
            paired: pc.paired,
            library: pc.library.as_ref(),
            classic: pc.classic,
            last_played: pc.last_played.as_ref(),
            collections: pc.collections.as_ref(),
            session: pc.session.as_ref(),
            host_control: pc.host_control,
        })
        .collect()
}

/// Two PCs with the fixture's library, the second one's Desktop hidden on this device.
fn two_pcs() -> (Vec<Pc>, Vec<Favourite>) {
    let lib = library();
    let pc = |id: u64, unique_id: &str, name: &str| Pc {
        id,
        unique_id: unique_id.into(),
        name: name.into(),
        online: true,
        paired: true,
        library: Some(lib.clone()),
        classic: false,
        last_played: Some([(lib.games[0].install_id.clone(), 100)].into()),
        collections: Some(in_collections(&lib)),
        session: Some(idle()),
        host_control: false,
    };
    let hidden = vec![star("two", &lib.games[1].install_id, DESKTOP, "")];
    (vec![pc(1, "one", "Den"), pc(2, "two", "Loft")], hidden)
}

type Change = fn(&mut Vec<Pc>, &mut Vec<Favourite>);

/// Each thing the library is made from, changed on its own. `true` where the change shows in
/// the merged library; the others are in a `Source` without the merge reading them today.
const CHANGES: &[(&str, bool, Change)] = &[
    ("a game's title", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].title = "Dota 3".into()),
    ("a game starts", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].running = true),
    ("a game is uninstalled", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].installed = Some(false)),
    ("a game's store", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].store.as_mut().unwrap().source = "epic".into()),
    ("a store's name", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].store.as_mut().unwrap().name = "Valve".into()),
    ("a game leaves its store", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].store = None),
    ("a cover goes", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].artwork.cover = None),
    ("a cover is replaced", false, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].artwork.version = "new".into()),
    ("a game's id", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].game_id = Some("steam:571".into())),
    ("a game's install id", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].install_id = "another".into()),
    ("a game's app id", true, |pcs, _| pcs[0].library.as_mut().unwrap().games[0].app_id = 7),
    ("a game is removed", true, |pcs, _| drop(pcs[0].library.as_mut().unwrap().games.remove(0))),
    ("a game is added", true, |pcs, _| {
        let games = &mut pcs[1].library.as_mut().unwrap().games;
        let mut game = games[0].clone();
        (game.title, game.game_id, game.app_id, game.install_id) = ("Another".into(), Some("steam:1".into()), 1, "new".into());
        games.push(game);
    }),
    ("the games change places", false, |pcs, _| pcs[0].library.as_mut().unwrap().games.swap(0, 1)),
    ("a library's revision", false, |pcs, _| pcs[0].library.as_mut().unwrap().revision = "next".into()),
    ("a library goes", true, |pcs, _| pcs[1].library = None),
    ("a game was played", true, |pcs, _| *pcs[0].last_played.as_mut().unwrap().values_mut().next().unwrap() = 200),
    ("the last played times go", true, |pcs, _| pcs[0].last_played = None),
    ("a game leaves a collection", true, |pcs, _| drop(pcs[0].collections.as_mut().unwrap().values_mut().next().unwrap().pop())),
    ("the collections go", true, |pcs, _| pcs[0].collections = None),
    ("a session starts", true, |pcs, _| pcs[1].session = Some(session())),
    ("a session's revision", false, |pcs, _| pcs[1].session.as_mut().unwrap().revision += 1),
    ("a session goes", false, |pcs, _| pcs[1].session = None),
    ("a pc goes away", true, |pcs, _| pcs[0].online = false),
    ("a pc is renamed", true, |pcs, _| pcs[0].name = "Study".into()),
    ("a pc's id", true, |pcs, _| pcs[0].id = 9),
    ("a pc tells another unique id", true, |pcs, _| pcs[1].unique_id = "zwei".into()),
    ("a pc is unpaired", false, |pcs, _| pcs[0].paired = false),
    ("a pc gives host control", false, |pcs, _| pcs[0].host_control = true),
    ("a library turns out to be an app list", false, |pcs, _| pcs[0].classic = true),
    ("a pc is removed", true, |pcs, _| drop(pcs.remove(1))),
    ("a pc is added", true, |pcs, _| {
        let mut pc = pcs[0].clone();
        (pc.id, pc.unique_id, pc.name) = (3, "three".into(), "Attic".into());
        pcs.push(pc);
    }),
    ("the pcs change places", true, |pcs, _| pcs.swap(0, 1)),
    ("a game is hidden", true, |pcs, hidden| {
        let game = &pcs[0].library.as_ref().unwrap().games[0];
        hidden.push(star("one", &game.install_id, game.app_id, ""));
    }),
    ("a game is shown again", true, |_, hidden| hidden.clear()),
    ("a hidden game's saved title", false, |_, hidden| hidden[0].title = "Desktop".into()),
];

#[test]
fn the_kept_library_is_merged_again_when_anything_it_was_made_from_changes() {
    for (what, shows, change) in CHANGES {
        let (before, hidden_before) = two_pcs();
        let memo = LibraryMemo::default();
        let first = memo.get(&sources_of(&before), &hidden_before);
        assert_eq!(*first, Merged::of(&sources_of(&before), &hidden_before), "{what}");

        let (mut after, mut hidden_after) = (before.clone(), hidden_before.clone());
        change(&mut after, &mut hidden_after);
        let changed = memo.get(&sources_of(&after), &hidden_after);
        assert_eq!(memo.merges(), 2, "{what}: merged again");
        assert_eq!(*changed, Merged::of(&sources_of(&after), &hidden_after), "{what}: what a merge now gives");
        assert_eq!(*changed != *first, *shows, "{what}: whether it shows");

        // And back: the kept one is never one of an earlier state.
        let back = memo.get(&sources_of(&before), &hidden_before);
        assert_eq!(memo.merges(), 3, "{what}: merged again on the way back");
        assert_eq!(*back, *first, "{what}");
    }
}

#[test]
fn the_library_is_merged_once_while_nothing_changes() {
    let (pcs, hidden) = two_pcs();
    let memo = LibraryMemo::default();
    let first = memo.get(&sources_of(&pcs), &hidden);
    // The same values from other places in memory, as each frame's `Source`s are.
    let (copies, hidden_copy) = (pcs.clone(), hidden.clone());
    for _ in 0..3 {
        let again = memo.get(&sources_of(&copies), &hidden_copy);
        assert!(std::rc::Rc::ptr_eq(&first, &again));
    }
    assert_eq!(memo.merges(), 1);

    // No PC at all is a library too (the empty one), and is not merged over and over either.
    let empty = LibraryMemo::default();
    assert!(empty.get(&[], &[]).all().is_empty());
    assert!(empty.get(&[], &[]).shown().is_empty());
    assert_eq!(empty.merges(), 1);
}

#[test]
fn the_kept_library_is_the_merge_and_its_split() {
    let (pcs, hidden) = two_pcs();
    let sources = sources_of(&pcs);
    let memo = LibraryMemo::default();

    let merged = memo.get(&sources, &hidden);
    let (shown, away) = split_hidden(merge_library(&sources), &sources, &hidden);
    assert_eq!(merged.all(), merge_library(&sources));
    assert_eq!(merged.shown(), shown);
    assert_eq!(merged.hidden(), away);
    assert_eq!(away.len(), 1, "the second PC's Desktop");
    assert_eq!(shown.iter().map(|e| e.copies.len()).sum::<usize>(), 3);

    // With nothing hidden all of it shows.
    let merged = memo.get(&sources, &[]);
    assert_eq!(merged.shown(), merge_library(&sources));
    assert!(merged.hidden().is_empty());
}
