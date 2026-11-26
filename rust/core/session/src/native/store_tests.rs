//! A Playnite app gets the store `game_id` of the installed launcher game it stands for, so a
//! client merges it with another PC's own copy of that game (roadmap item 09, part 3).

use super::*;
use crate::library::Game as LauncherGame;
use serde_json::json;
use std::path::Path;

fn app(value: Value) -> App {
    serde_json::from_value(value).unwrap()
}

/// A Playnite app as the Playnite sync writes it (`playnite::apply_game`).
fn playnite(name: &str, id: &str, plugin: &str) -> App {
    app(json!({
        "name": name, "playnite-id": id, "uuid": id.to_ascii_uppercase(),
        "playnite-plugin-id": "00000000-0000-0000-0000-000000000001", "playnite-plugin-name": plugin,
        "synced-by": "playnite"
    }))
}

fn launcher(source: Source, id: &str, name: &str) -> LauncherGame {
    LauncherGame::new(
        source,
        id.into(),
        name.into(),
        Path::new("C:/Games/x"),
        None,
    )
}

#[test]
fn a_playnite_game_takes_the_store_id_of_the_same_installed_game() {
    let scan = Scan::from_games(&[
        launcher(Source::Epic, "Fortnite", "Fortnite"),
        launcher(
            Source::Ea,
            "Origin.OFR.50.0002694",
            "Mass Effect™ Legendary Edition",
        ),
    ])
    .titled([(Source::Steam, "Dota 2".to_owned(), "steam:570".to_owned())]);
    let apps = [
        playnite("Dota 2", "a5f8c1d2-0000-4000-8000-000000000001", "Steam"),
        playnite(
            "Fortnite",
            "a5f8c1d2-0000-4000-8000-000000000002",
            "Epic Games",
        ),
        playnite(
            "Mass Effect Legendary Edition",
            "a5f8c1d2-0000-4000-8000-000000000003",
            "Origin",
        ),
    ];
    let library = library(&apps, Some(&scan), None);
    let ids: Vec<_> = library.games.iter().map(|g| g.game_id.as_deref()).collect();
    assert_eq!(
        ids,
        [
            Some("steam:570"),
            Some("epic:Fortnite"),
            Some("ea:Origin.OFR.50.0002694")
        ]
    );
    let store = library.games[1].store.as_ref().unwrap();
    assert_eq!(
        (
            store.source.as_str(),
            store.id.as_str(),
            store.name.as_str()
        ),
        ("epic", "Fortnite", "Epic Games")
    );
    assert_eq!(library.games[1].installed, Some(true));
    assert_eq!(installed(&apps[1], &scan), Some(true));
}

#[test]
fn a_playnite_game_without_a_sure_match_keeps_no_store_id() {
    let scan = Scan::from_games(&[
        launcher(Source::Epic, "Fortnite", "Fortnite"),
        // Two games of one store share a title: neither is the Playnite one for sure.
        launcher(Source::Gog, "1", "Doom"),
        launcher(Source::Gog, "2", "DOOM"),
    ]);
    let apps = [
        // Another store's plugin: the Epic game with this title is not this one.
        playnite("Fortnite", "a5f8c1d2-0000-4000-8000-000000000004", "Xbox"),
        playnite("Doom", "a5f8c1d2-0000-4000-8000-000000000005", "GOG"),
        // A plugin no launcher's library is, and a game the scan did not find.
        playnite(
            "Fortnite",
            "a5f8c1d2-0000-4000-8000-000000000006",
            "Emulators",
        ),
        playnite("Kart", "a5f8c1d2-0000-4000-8000-000000000007", "Epic Games"),
        // Not a Playnite app: a hand-made one with the same name stays as it is.
        app(json!({"name": "Fortnite", "uuid": "b5f8c1d2-0000-4000-8000-000000000008"})),
    ];
    let library = library(&apps, Some(&scan), None);
    for game in &library.games {
        assert!(
            game.game_id.is_none() && game.store.is_none() && game.installed.is_none(),
            "{}",
            game.title
        );
    }
    // Without a scan, as before.
    assert!(
        super::library(&apps[..1], None, None).games[0]
            .game_id
            .is_none()
    );
}

#[test]
fn an_app_with_its_own_store_id_keeps_it() {
    let scan = Scan::from_games(&[launcher(Source::Epic, "Other", "Fortnite")]);
    let own = app(json!({
        "name": "Fortnite", "playnite-id": "a5f8c1d2-0000-4000-8000-000000000009", "playnite-plugin-name": "Epic",
        "launcher-source": "epic", "launcher-id": "Fortnite"
    }));
    let library = library(&[own], Some(&scan), None);
    assert_eq!(library.games[0].game_id.as_deref(), Some("epic:Fortnite"));
}

#[test]
fn playnite_plugins_name_their_launchers() {
    for (plugin, source) in [
        ("Steam", Some(Source::Steam)),
        ("Epic Games", Some(Source::Epic)),
        ("EA app", Some(Source::Ea)),
        ("Origin", Some(Source::Ea)),
        ("Ubisoft Connect", Some(Source::Ubisoft)),
        ("Uplay", Some(Source::Ubisoft)),
        ("GOG", Some(Source::Gog)),
        ("Battle.net", Some(Source::Battlenet)),
        ("Xbox", Some(Source::Microsoft)),
        ("Emulators", None),
        ("", None),
    ] {
        assert_eq!(Source::of_playnite_plugin(plugin), source, "{plugin}");
    }
}
