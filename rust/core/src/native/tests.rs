use super::*;
use serde_json::json;

fn app(value: Value) -> App {
    serde_json::from_value(value).unwrap()
}
fn client(perm: u32, extra: Value) -> Client {
    let mut client = json!({"name": "Phone", "cert": "", "uuid": "me", "perm": perm});
    client
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(client).unwrap()
}
fn dota() -> App {
    app(json!({
        "name": "Dota 2", "uuid": "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f",
        "launcher-source": "steam", "launcher-id": "570", "image-path": "dota.png"
    }))
}
fn game(id: u32) -> SessionApp {
    SessionApp {
        install_id: "c6f1a9a4".into(),
        app_id: id,
        title: "Dota 2".into(),
    }
}
fn snapshot(state: State, owner: &str) -> SessionSnapshot {
    SessionSnapshot {
        revision: 9,
        state,
        game_running: true,
        owner: Some(owner.into()),
        app: Some(game(5)),
        since_unix_ms: Some(10),
        suspended_until_unix_ms: Some(20),
        launcher: None,
        radeon_profile: None,
    }
}
fn suspended(owner: &str) -> SessionSnapshot {
    snapshot(State::Suspended, owner)
}
fn allowed(session: &Session, id: ActionId) -> bool {
    session.actions.iter().find(|a| a.id == id).unwrap().allowed
}
fn reason(session: &Session, id: ActionId) -> Option<ErrorCode> {
    session.actions.iter().find(|a| a.id == id).unwrap().reason
}
const ALL: u32 = LIST | VIEW | LAUNCH;

#[test]
fn library_ids_and_store() {
    let library = library(&[dota()], None, None);
    let game = &library.games[0];
    assert_eq!(game.install_id, "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f");
    assert_eq!(game.title, "Dota 2");
    assert_eq!(game.game_id.as_deref(), Some("steam:570"));
    let store = game.store.as_ref().unwrap();
    assert_eq!(
        (
            store.source.as_str(),
            store.id.as_str(),
            store.name.as_str()
        ),
        ("steam", "570", "Steam")
    );
    assert_eq!(game.app_id, dota().id());
    assert_eq!(
        game.artwork.cover.as_deref(),
        Some(format!("/appasset?appid={}&AssetType=2&AssetIdx=0", dota().id()).as_str())
    );
    assert_eq!(game.installed, None);
    assert!(!game.running);
}

#[test]
fn manual_apps_have_no_game_id_store_or_installed_flag() {
    let desktop = app(json!({"name": "Desktop", "uuid": "8d4b3f55-0c2e-4f8a-9a61-2f1c6f0e9b17"}));
    let scan = Scan::new(["steam:570".into()]);
    let library = library(&[desktop], Some(&scan), None);
    let game = &library.games[0];
    assert!(game.game_id.is_none() && game.store.is_none() && game.installed.is_none());
}

#[test]
fn installed_comes_from_the_scan() {
    let gone =
        app(json!({"name": "Gone", "uuid": "g", "launcher-source": "epic", "launcher-id": "x"}));
    let scan = Scan::new(["steam:570".into()]);
    let library = library(&[dota(), gone], Some(&scan), None);
    assert_eq!(library.games[0].installed, Some(true));
    assert_eq!(library.games[1].installed, Some(false));
    assert_eq!(library.games[1].store.as_ref().unwrap().name, "Epic Games");
    let none = super::library(&[dota()], None, None);
    assert_eq!(none.games[0].installed, None);
}

#[test]
fn not_installed_is_only_for_a_known_launcher_id_the_scan_did_not_find() {
    let steam = |name: &str, id: u32| app(json!({"name": name, "uuid": name, "steam-id": id}));
    let epic = app(
        json!({"name": "Fortnite", "uuid": "f", "launcher-source": "epic", "launcher-id": "x"}),
    );
    let manual = app(json!({"name": "Desktop", "uuid": "d"}));
    let scan = Scan::new(["steam:570".into()]).skipping([Source::Epic]);
    assert_eq!(super::installed(&steam("Dota 2", 570), &scan), Some(true));
    assert_eq!(super::installed(&steam("Gone", 999), &scan), Some(false));
    assert_eq!(super::installed(&epic, &scan), None, "Epic was not read");
    assert_eq!(super::installed(&manual, &scan), None, "no launcher id");
    let tools = Scan::new([]).adding(["steam:1007".into()]);
    assert_eq!(super::installed(&steam("SDK", 1007), &tools), Some(true));
}

#[test]
fn a_source_turned_off_leaves_its_games_installed_state_unknown() {
    let epic =
        app(json!({"name": "Kept", "uuid": "k", "launcher-source": "epic", "launcher-id": "x"}));
    let scan = Scan::new(["steam:570".into()]).skipping([Source::Epic]);
    let library = library(&[dota(), epic], Some(&scan), None);
    assert_eq!(library.games[0].installed, Some(true));
    assert_eq!(library.games[1].installed, None);
}

#[test]
fn a_game_folder_the_scan_no_longer_reads_has_an_unknown_installed_state() {
    let folder = |name: &str, id: &str| {
        app(json!({"name": name, "uuid": name, "launcher-source": "local", "launcher-id": id}))
    };
    let settings = crate::library::Settings {
        local_folders: vec!["D:/Games/".into()],
        local_exclude: vec!["Left Out".into()],
        ..Default::default()
    };
    let scan = Scan::new([r"local:d:\games\hades".into()]).reading(settings);
    let apps = [
        folder("Hades", r"d:\games\hades"),
        folder("Removed", r"d:\games\removed"),
        folder("Left out", r"d:\games\left out"),
        folder("Elsewhere", r"e:\old games\celeste"),
    ];
    let library = library(&apps, Some(&scan), None);
    let installed: Vec<_> = library.games.iter().map(|game| game.installed).collect();
    assert_eq!(installed, [Some(true), Some(false), None, None]);
}

#[test]
fn scan_from_discovered_games() {
    let found = crate::library::Game::new(
        Source::Gog,
        "1207658930".into(),
        "Witcher".into(),
        std::path::Path::new("C:/GOG/Witcher"),
        None,
    );
    let scan = Scan::from_games(&[found]);
    assert!(scan.contains("gog:1207658930"));
    assert!(!scan.contains("gog:1"));
}

#[test]
fn steam_apps_without_launcher_keys_use_steam_id() {
    let steam = app(json!({"name": "Half-Life", "uuid": "x", "steam-id": "70"}));
    let game = &library(&[steam], None, None).games[0];
    assert_eq!(game.game_id.as_deref(), Some("steam:70"));
    assert_eq!(game.store.as_ref().unwrap().source, "steam");
    let canonical = app(json!({"name": "Dota", "uuid": crate::steam::canonical_uuid(570)}));
    assert_eq!(
        library(&[canonical], None, None).games[0]
            .game_id
            .as_deref(),
        Some("steam:570")
    );
}

#[test]
fn running_marks_the_current_game_only() {
    let other = app(json!({"name": "Other", "uuid": "o"}));
    let current = dota().id();
    let library = library(&[dota(), other], None, Some(current));
    assert!(library.games[0].running);
    assert!(!library.games[1].running);
}

#[test]
fn control_tiles_are_left_out() {
    let tiles = [
        app(json!({"name": "Resume", "uuid": "9a1c5a25-58fe-40e0-b9aa-7d3f00000001"})),
        app(json!({"name": "Terminate", "uuid": "t", "id": 2147483504})),
    ];
    let library = library(&[tiles[0].clone(), dota(), tiles[1].clone()], None, None);
    assert_eq!(library.games.len(), 1);
    assert_eq!(library.games[0].title, "Dota 2");
}

#[test]
fn order_is_the_console_order() {
    let a = app(json!({"name": "B", "uuid": "1"}));
    let b = app(json!({"name": "A", "uuid": "2"}));
    let titles: Vec<_> = library(&[a, b], None, None)
        .games
        .into_iter()
        .map(|g| g.title)
        .collect();
    assert_eq!(titles, ["B", "A"]);
}

#[test]
fn install_id_survives_a_rename_and_a_missing_uuid_is_derived() {
    let mut renamed = dota();
    renamed.name = "Dota 2 (beta)".into();
    assert_eq!(install_id(&renamed), install_id(&dota()));
    let bare = app(json!({"name": "Bare"}));
    assert_eq!(install_id(&bare), install_id(&bare));
    assert_ne!(
        install_id(&bare),
        install_id(&app(json!({"name": "Other"})))
    );
}

#[test]
fn revision_ignores_running_and_follows_everything_else() {
    let apps = [dota()];
    let idle = library(&apps, None, None);
    let running = library(&apps, None, Some(dota().id()));
    assert_eq!(idle.revision, running.revision);
    assert_eq!(idle.revision.len(), 8);
    let mut renamed = dota();
    renamed.name = "Dota 3".into();
    assert_ne!(library(&[renamed], None, None).revision, idle.revision);
    let scan = Scan::new(["steam:570".into()]);
    assert_ne!(library(&apps, Some(&scan), None).revision, idle.revision);
}

#[test]
fn artwork_version_follows_the_cover_fingerprint() {
    let apps = [dota()];
    let a = library_with(&apps, None, None, |_| "dota.png|10|1".into());
    let b = library_with(&apps, None, None, |_| "dota.png|11|1".into());
    assert_ne!(a.games[0].artwork.version, b.games[0].artwork.version);
    assert_ne!(a.revision, b.revision);
}

#[test]
fn host_control_is_off_unless_true() {
    assert!(!host_control(&client(u32::MAX, json!({}))));
    assert!(host_control(&client(0, json!({"host_control": true}))));
    assert!(host_control(&client(0, json!({"host_control": "true"}))));
    for value in [
        json!(false),
        json!("yes"),
        json!(1),
        json!("1"),
        json!(null),
        json!(""),
    ] {
        assert!(
            !host_control(&client(u32::MAX, json!({"host_control": value}))),
            "{value}"
        );
    }
    let mut disabled = client(0, json!({"host_control": true}));
    disabled.enabled = false;
    assert!(!host_control(&disabled));
}

#[test]
fn the_spec_example_for_a_suspended_owner() {
    let session = session_view(&suspended("me"), &client(ALL, json!({})));
    assert_eq!(session.state, State::Suspended);
    assert_eq!(session.owner, Owner::You);
    assert!(session.app.is_some());
    assert_eq!(session.suspended_until_unix_ms, Some(20));
    let actions: Vec<_> = session
        .actions
        .iter()
        .map(|a| (a.id, a.allowed, a.reason))
        .collect();
    assert_eq!(
        actions,
        [
            (ActionId::Resume, true, None),
            (ActionId::Disconnect, false, Some(ErrorCode::NotStreaming)),
            (ActionId::End, true, None),
            (
                ActionId::CloseGame,
                false,
                Some(ErrorCode::HostControlRequired)
            ),
        ]
    );
}

#[test]
fn a_streaming_owner_can_disconnect_end_and_close_with_host_control() {
    let caller = client(ALL, json!({"host_control": true}));
    let session = session_view(&snapshot(State::Streaming, "me"), &caller);
    assert_eq!(session.state, State::Streaming);
    // Resume supersedes the same device's stream: the reconnect after a
    // crash.
    for id in [
        ActionId::Resume,
        ActionId::Disconnect,
        ActionId::End,
        ActionId::CloseGame,
    ] {
        assert!(allowed(&session, id), "{id:?}");
    }
    assert_eq!(session.suspended_until_unix_ms, None);
    assert_eq!(session.since_unix_ms, Some(10));
}

#[test]
fn another_device_sees_state_and_owner_only() {
    let caller = client(ALL, json!({"host_control": true}));
    for snapshot in [snapshot(State::Streaming, "someone"), suspended("someone")] {
        let session = session_view(&snapshot, &caller);
        assert_eq!(session.owner, Owner::OtherDevice);
        assert!(session.app.is_none());
        assert!(session.since_unix_ms.is_none() && session.suspended_until_unix_ms.is_none());
        assert!(session.game_running);
        for action in &session.actions {
            assert!(!action.allowed, "{:?}", action.id);
            assert_eq!(action.reason, Some(ErrorCode::NotSessionOwner));
        }
    }
}

#[test]
fn without_view_or_launch_the_app_is_hidden_and_actions_need_permission() {
    let caller = client(LIST, json!({"host_control": true}));
    let session = session_view(&suspended("me"), &caller);
    assert_eq!(session.owner, Owner::You);
    assert!(session.app.is_none());
    for action in &session.actions {
        assert!(!action.allowed);
        assert_eq!(
            action.reason,
            Some(ErrorCode::PermissionRequired),
            "{:?}",
            action.id
        );
    }
    // View alone shows the app and may resume and disconnect, not end.
    let viewer = client(VIEW, json!({}));
    let session = session_view(&snapshot(State::Streaming, "me"), &viewer);
    assert!(session.app.is_some());
    assert!(allowed(&session, ActionId::Disconnect));
    assert_eq!(
        reason(&session, ActionId::End),
        Some(ErrorCode::PermissionRequired)
    );
    assert_eq!(
        reason(&session, ActionId::CloseGame),
        Some(ErrorCode::PermissionRequired)
    );
}

#[test]
fn a_disabled_device_may_do_nothing() {
    let mut caller = client(ALL, json!({"host_control": true}));
    caller.enabled = false;
    let session = session_view(&suspended("me"), &caller);
    assert!(session.app.is_none());
    assert!(session.actions.iter().all(|a| !a.allowed));
}

#[test]
fn idle_has_nothing_to_resume_end_or_disconnect() {
    let caller = client(ALL, json!({"host_control": true}));
    let session = session_view(&SessionSnapshot::default(), &caller);
    assert_eq!(session.state, State::Idle);
    assert_eq!(session.owner, Owner::None);
    assert!(session.app.is_none() && !session.game_running);
    for id in [
        ActionId::Resume,
        ActionId::Disconnect,
        ActionId::End,
        ActionId::CloseGame,
    ] {
        assert!(!allowed(&session, id), "{id:?}");
        assert_eq!(reason(&session, id), Some(ErrorCode::NoSession), "{id:?}");
    }
}

#[test]
fn what_the_host_noticed_about_the_game_is_the_owners_to_see() {
    use crate::native_api::session::{LauncherKind, ProfileState};
    let mut noticed = snapshot(State::Streaming, "me");
    noticed.launcher = Some(LauncherNotice {
        launcher: "steam".into(),
        kind: LauncherKind::Update,
        window_title: "Updating Dota 2".into(),
        since_unix_ms: 5,
    });
    noticed.radeon_profile = Some(RadeonProfile {
        profile: "quality".into(),
        state: ProfileState::Applied,
        message: None,
        settings: vec!["anisotropic_filtering.enabled".into()],
    });
    let mine = session_view(&noticed, &client(ALL, json!({})));
    assert_eq!(mine.launcher, noticed.launcher);
    assert_eq!(mine.radeon_profile, noticed.radeon_profile);
    // Another device, or one that may neither view nor launch, sees neither.
    let mut other = client(ALL, json!({}));
    other.uuid = "someone-else".into();
    let theirs = session_view(&noticed, &other);
    assert!(theirs.launcher.is_none() && theirs.radeon_profile.is_none());
    let blind = session_view(&noticed, &client(0, json!({})));
    assert!(blind.launcher.is_none() && blind.radeon_profile.is_none());
    // Nothing noticed, nothing on the wire.
    let plain = session_view(&snapshot(State::Streaming, "me"), &client(ALL, json!({})));
    let text = serde_json::to_string(&plain).unwrap();
    assert!(!text.contains("launcher") && !text.contains("radeon_profile"));
}

#[test]
fn close_game_works_without_a_session_when_a_game_is_left_running() {
    let left = SessionSnapshot {
        game_running: true,
        revision: 4,
        ..Default::default()
    };
    let with = client(ALL, json!({"host_control": true}));
    let session = session_view(&left, &with);
    assert_eq!(session.state, State::Idle);
    assert!(session.game_running);
    assert!(allowed(&session, ActionId::CloseGame));
    assert!(!allowed(&session, ActionId::End));
    // Without host control or Launch it stays closed.
    let session = session_view(&left, &client(ALL, json!({})));
    assert_eq!(
        reason(&session, ActionId::CloseGame),
        Some(ErrorCode::HostControlRequired)
    );
    let session = session_view(&left, &client(VIEW, json!({"host_control": true})));
    assert_eq!(
        reason(&session, ActionId::CloseGame),
        Some(ErrorCode::PermissionRequired)
    );
}

#[test]
fn a_launch_in_progress_is_busy_for_every_action() {
    let caller = client(ALL, json!({"host_control": true}));
    for expected in [State::Preparing, State::Launching] {
        let session = session_view(&snapshot(expected, "me"), &caller);
        assert_eq!(session.state, expected);
        for id in [ActionId::Resume, ActionId::End, ActionId::CloseGame] {
            assert!(!allowed(&session, id), "{expected:?} {id:?}");
            assert_eq!(
                reason(&session, id),
                Some(ErrorCode::SessionBusy),
                "{expected:?} {id:?}"
            );
        }
        assert_eq!(
            reason(&session, ActionId::Disconnect),
            Some(ErrorCode::NotStreaming)
        );
    }
}

#[test]
fn stopping_allows_end_and_close_game_again_but_not_resume() {
    let caller = client(ALL, json!({"host_control": true}));
    let session = session_view(&snapshot(State::Stopping, "me"), &caller);
    assert_eq!(session.state, State::Stopping);
    assert!(allowed(&session, ActionId::End));
    assert!(allowed(&session, ActionId::CloseGame));
    assert_eq!(
        reason(&session, ActionId::Resume),
        Some(ErrorCode::SessionBusy)
    );
}

#[test]
fn resume_needs_view() {
    let launcher = client(LAUNCH, json!({}));
    let session = session_view(&snapshot(State::Streaming, "me"), &launcher);
    assert_eq!(
        reason(&session, ActionId::Resume),
        Some(ErrorCode::PermissionRequired)
    );
    assert!(allowed(&session, ActionId::End));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "needs an owner")]
fn a_session_without_an_owner_is_a_host_bug() {
    let mut orphan = suspended("me");
    orphan.owner = None;
    session_view(&orphan, &client(ALL, json!({})));
}

#[test]
fn an_idle_session_needs_no_owner() {
    let session = session_view(&SessionSnapshot::default(), &client(ALL, json!({})));
    assert_eq!(session.owner, Owner::None);
}

#[test]
fn a_blank_uuid_is_replaced_like_a_missing_one() {
    let blank = app(json!({"name": "Blank", "uuid": "  "}));
    let missing = app(json!({"name": "Blank"}));
    assert_eq!(install_id(&blank), install_id(&missing));
    assert_ne!(install_id(&blank), "  ");
}

#[test]
fn the_session_serialises_like_the_spec_example() {
    let session = session_view(&suspended("me"), &client(ALL, json!({})));
    let value = serde_json::to_value(&session).unwrap();
    assert_eq!(value["state"], "suspended");
    assert_eq!(value["owner"], "you");
    assert_eq!(
        value["actions"][1],
        json!({"id": "disconnect", "allowed": false, "reason": "not_streaming"})
    );
    assert_eq!(
        value["actions"][0],
        json!({"id": "resume", "allowed": true})
    );
}
