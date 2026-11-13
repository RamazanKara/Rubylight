use super::handle;
use crate::state::test_support::Fixture;
use serde_json::json;

#[test]
fn a_cover_the_browser_holds_is_not_sent_again_until_the_file_changes() {
    use axum::http::{HeaderMap, StatusCode, header};
    let folder = std::env::temp_dir().join(format!("rubylight-cover-tag-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("cover.png");
    std::fs::write(&path, b"first").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let ask = |headers: &HeaderMap, most: u64| runtime.block_on(super::image(&path, most, headers));
    let sent = ask(&HeaderMap::new(), 16).unwrap();
    assert_eq!(sent.status(), StatusCode::OK);
    assert_eq!(sent.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(sent.headers()[header::CACHE_CONTROL], "private, no-cache");
    let tag = sent.headers()[header::ETAG].clone();
    // The same file again: no body, also when the browser lists several or a weak one.
    let mut held = HeaderMap::new();
    held.insert(header::IF_NONE_MATCH, tag.clone());
    let kept = ask(&held, 16).unwrap();
    assert_eq!(kept.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(kept.headers()[header::ETAG], tag);
    let several = format!("\"other\", W/{}", tag.to_str().unwrap());
    held.insert(header::IF_NONE_MATCH, several.parse().unwrap());
    assert_eq!(ask(&held, 16).unwrap().status(), StatusCode::NOT_MODIFIED);
    // A changed file is sent in full under a new name.
    std::fs::write(&path, b"second cover").unwrap();
    held.insert(header::IF_NONE_MATCH, tag.clone());
    let changed = ask(&held, 16).unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert_ne!(changed.headers()[header::ETAG], tag);
    // One over the limit, or none, is no image.
    assert!(ask(&held, 4).is_none());
    std::fs::remove_dir_all(&folder).unwrap();
    assert!(ask(&HeaderMap::new(), 16).is_none());
}

#[test]
fn apps_list_marks_only_known_uninstalled_launcher_games_and_never_stores_it() {
    let f = Fixture::new();
    for app in [
        json!({"name": "Dota 2", "uuid": "dota", "steam-id": "570"}),
        json!({"name": "Gone", "uuid": "gone", "steam-id": "999"}),
        json!({"name": "Fortnite", "uuid": "fn", "launcher-source": "epic", "launcher-id": "x"}),
        json!({"name": "Tool", "uuid": "tool", "steam-id": "1007"}),
    ] {
        handle(&f.host, "POST", "/api/apps", &app).unwrap();
    }
    crate::library::seed(
        &f.host,
        butterpollo_core::native::Scan::new(["steam:570".into()])
            .adding(["steam:1007".into()])
            .skipping([butterpollo_core::library::Source::Epic]),
    );
    let list = handle(&f.host, "GET", "/api/apps", &json!({})).unwrap();
    let flagged: Vec<&str> = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|app| app["not-installed"] == json!(true))
        .map(|app| app["name"].as_str().unwrap())
        .collect();
    assert_eq!(flagged, ["Gone"]);
    let mut gone = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|app| app["name"] == "Gone")
        .unwrap()
        .clone();
    gone["name"] = json!("Gone (renamed)");
    handle(&f.host, "POST", "/api/apps", &gone).unwrap();
    let stored = f.host.apps.read().unwrap();
    let renamed = stored
        .iter()
        .find(|app| app.name == "Gone (renamed)")
        .unwrap();
    assert!(!renamed.extra.contains_key("not-installed"));
}

#[test]
fn apps_list_says_when_the_stores_last_played_a_game_and_never_stores_it() {
    let f = Fixture::new();
    for app in [
        json!({"name": "Dota 2", "uuid": "dota", "steam-id": "570"}),
        json!({"name": "Never played", "uuid": "never", "steam-id": "440"}),
        json!({"name": "Desktop", "uuid": "desktop"}),
    ] {
        handle(&f.host, "POST", "/api/apps", &app).unwrap();
    }
    crate::library::seed(
        &f.host,
        butterpollo_core::native::Scan::new(["steam:570".into(), "steam:440".into()]).playing([
            ("steam:570".to_owned(), 1_700_000_000),
            ("steam:440".to_owned(), 0),
        ]),
    );
    let list = handle(&f.host, "GET", "/api/apps", &json!({})).unwrap();
    let app = |name: &str| {
        list["apps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|app| app["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(app("Dota 2")["last-played"], json!(1_700_000_000u64));
    // Never played and no store at all: no time, rather than 0.
    assert!(app("Never played").get("last-played").is_none());
    assert!(app("Desktop").get("last-played").is_none());
    handle(&f.host, "POST", "/api/apps", &app("Dota 2")).unwrap();
    let stored = f.host.apps.read().unwrap();
    let dota = stored.iter().find(|app| app.name == "Dota 2").unwrap();
    assert!(!dota.extra.contains_key("last-played"));
}

#[test]
fn an_app_cannot_save_a_preset_the_host_does_not_know() {
    let f = Fixture::new();
    let save = |overrides: serde_json::Value| {
        handle(
            &f.host,
            "POST",
            "/api/apps",
            &json!({"name": "Game", "uuid": "preset-app", "config-overrides": overrides}),
        )
    };
    let stored = || {
        f.host
            .apps
            .read()
            .unwrap()
            .iter()
            .find(|app| app.name == "Game")
            .map(|app| app.extra["config-overrides"].clone())
    };
    save(json!({"stream_preset": "reference"})).unwrap();
    let good = stored();
    assert_eq!(good, Some(json!({"stream_preset": "reference"})));
    for bad in [
        json!("fastest"),
        json!(3),
        json!(true),
        json!(["reference"]),
    ] {
        let error = save(json!({"stream_preset": bad})).unwrap_err();
        assert!(error.to_string().contains("stream_preset"), "{error}");
        assert_eq!(stored(), good, "{bad}");
    }
    save(json!({"stream_preset": ""})).unwrap();
}

#[test]
fn an_app_saves_a_radeon_profile_the_host_can_apply_and_refuses_others() {
    let f = Fixture::new();
    let save = |extra: serde_json::Value| {
        let mut app = json!({"name": "Game", "uuid": "radeon-app"});
        app.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        handle(&f.host, "POST", "/api/apps", &app)
    };
    let stored = || {
        f.host
            .apps
            .read()
            .unwrap()
            .iter()
            .find(|app| app.name == "Game")
            .map(|app| {
                (
                    app.extra.get("radeon-profile").cloned(),
                    app.extra.get("radeon-settings").cloned(),
                )
            })
    };
    save(json!({"radeon-profile": "smoothness"})).unwrap();
    assert_eq!(stored(), Some((Some(json!("smoothness")), None)));
    save(json!({
        "radeon-profile": "custom",
        "radeon-settings": {"chill.enabled": true, "chill.max_fps": 90}
    }))
    .unwrap();
    let good = stored();
    assert_eq!(
        good.as_ref().unwrap().1,
        Some(json!({"chill.enabled": true, "chill.max_fps": 90}))
    );
    for (bad, why) in [
        (json!({"radeon-profile": "fastest"}), "radeon-profile"),
        (json!({"radeon-profile": "custom"}), "at least one"),
        (
            json!({"radeon-profile": "custom", "radeon-settings": {"x": 1}}),
            "not a Radeon setting id",
        ),
        (
            json!({"radeon-profile": "quality", "radeon-settings": {"a.b": 1}}),
            "only with the custom",
        ),
    ] {
        let error = save(bad.clone()).unwrap_err();
        assert!(error.to_string().contains(why), "{bad}: {error}");
        assert_eq!(stored(), good, "a refused save changes nothing: {bad}");
    }
    // An app with none saves as before.
    save(json!({})).unwrap();
    assert_eq!(stored(), Some((None, None)));
}

#[test]
fn an_app_saves_per_device_settings_and_game_files_and_refuses_what_a_launch_cannot_use() {
    let f = Fixture::new();
    let save = |extra: serde_json::Value| {
        let mut app = json!({"name": "Game", "uuid": "device-app"});
        app.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        handle(&f.host, "POST", "/api/apps", &app)
    };
    let good = json!({
        "game-files": [{"path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": "{stream_fps-1}"}],
        "device-overrides": {
            "phone": {
                "config-overrides": {"stream_preset": "reference", "frame_limiter_fps_limit": 60},
                "radeon-profile": "smoothness",
                "game-files": [{"path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": 60}]
            },
            // A device that is not paired keeps its block.
            "gone": {"config-overrides": {"max_bitrate": 20000}}
        }
    });
    save(good.clone()).unwrap();
    let saved = || stored(&f, "device-app").unwrap();
    assert_eq!(saved()["device-overrides"], good["device-overrides"]);
    for (bad, why) in [
        (
            json!({"device-overrides": {"phone": {"config-overrides": {"port": 50000}}}}),
            "port",
        ),
        (
            json!({"device-overrides": {"phone": {"config-overrides": {"stream_preset": "fastest"}}}}),
            "stream_preset",
        ),
        (
            json!({"device-overrides": {"phone": {"radeon-profile": "fastest"}}}),
            "radeon-profile",
        ),
        (
            json!({"device-overrides": {"phone": {"name": "x"}}}),
            "name",
        ),
        (
            json!({"game-files": [{"path": "..\\other\\x.ini", "section": "A", "key": "B", "value": 1}]}),
            "..",
        ),
        (
            json!({"game-files": [{"path": "settings.cfg", "section": "A", "key": "B", "value": 1}]}),
            ".ini",
        ),
        (
            json!({"game-files": [{"path": "a.ini", "section": "A", "key": "B", "value": "{base_fps}"}]}),
            "{stream_fps}",
        ),
    ] {
        let error = format!("{:#}", save(bad.clone()).unwrap_err());
        assert!(error.contains(why), "{bad}: {error}");
        assert_eq!(
            saved()["device-overrides"],
            good["device-overrides"],
            "a refused save changes nothing: {bad}"
        );
    }
}

fn stored(f: &Fixture, uuid: &str) -> Option<serde_json::Value> {
    f.host
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.extra.get("uuid").and_then(|u| u.as_str()) == Some(uuid))
        .map(|app| serde_json::to_value(app).unwrap())
}

#[test]
fn a_synced_app_saved_in_the_console_becomes_the_users_and_survives_uninstall() {
    use butterpollo_core::steam;
    let f = Fixture::new();
    let uuid = steam::canonical_uuid(620);
    crate::steam::update_apps(&f.host, |apps| {
        apps.push(json!({"name": "Portal 2", "uuid": uuid, "steam-id": "620",
            "steam-managed": "auto", "synced-by": "steam", "cmd": steam::launch_command(620)}));
        true
    })
    .unwrap();
    // The editor sends the app back with the user's name, without the stamp.
    handle(
        &f.host,
        "POST",
        "/api/apps",
        &json!({"name": "My Portal", "uuid": uuid, "steam-id": "620",
            "steam-managed": "auto", "cmd": steam::launch_command(620)}),
    )
    .unwrap();
    let app = stored(&f, &uuid).unwrap();
    assert_eq!(app["name"], "My Portal");
    assert_eq!(app["steam-managed"], "manual");
    assert_eq!(app.get("synced-by"), None);
    // Uninstalled, with a sync that removes missing games: it stays as saved.
    let settings = steam::Settings::from_config(
        &butterpollo_core::config::Config::parse(
            "steam_sync_all_installed=true\nsteam_autosync_remove_uninstalled=true",
        )
        .unwrap(),
    );
    let changed = crate::steam::update_apps(&f.host, |apps| {
        steam::reconcile(apps, &[], &settings, &Default::default(), &[])
    })
    .unwrap();
    assert!(!changed);
    assert_eq!(stored(&f, &uuid).unwrap(), app);
    // Only a sync stamps an app: a stamp sent with one is dropped.
    handle(
        &f.host,
        "POST",
        "/api/apps",
        &json!({"name": "Forged", "uuid": "forged", "synced-by": "steam",
            "steam-id": "440", "steam-managed": "auto"}),
    )
    .unwrap();
    let forged = stored(&f, "forged").unwrap();
    assert_eq!(forged.get("synced-by"), None);
    assert_eq!(forged["steam-managed"], "auto");
}

#[test]
fn deleting_any_app_that_starts_a_steam_game_dismisses_the_game() {
    let f = Fixture::new();
    handle(
        &f.host,
        "POST",
        "/api/apps",
        &json!({"name": "Mine", "uuid": "mine", "cmd": "cmd /c start steam://rungameid/620"}),
    )
    .unwrap();
    handle(
        &f.host,
        "POST",
        "/api/apps/delete",
        &json!({"uuid": "mine"}),
    )
    .unwrap();
    assert!(stored(&f, "mine").is_none());
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!(["steam:620"])
    );
    // An app without a launcher game dismisses nothing.
    handle(
        &f.host,
        "POST",
        "/api/apps",
        &json!({"name": "Tool", "uuid": "tool"}),
    )
    .unwrap();
    handle(
        &f.host,
        "POST",
        "/api/apps/delete",
        &json!({"uuid": "tool"}),
    )
    .unwrap();
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!(["steam:620"])
    );
}

#[test]
fn removing_several_apps_at_once_keeps_every_synced_game_out_in_one_save() {
    let f = Fixture::new();
    for app in [
        json!({"name": "Portal 2", "uuid": "portal", "steam-id": "620"}),
        json!({"name": "Mirage", "uuid": "mirage", "launcher-source": "ubisoft",
            "launcher-id": "13504"}),
        json!({"name": "Kept", "uuid": "kept", "launcher-source": "epic", "launcher-id": "k"}),
    ] {
        handle(&f.host, "POST", "/api/apps", &app).unwrap();
    }
    // A uuid already gone (a sync dropped its game meanwhile) is skipped, not an error.
    let removed = handle(
        &f.host,
        "POST",
        "/api/apps/delete",
        &json!({"uuids": ["portal", "mirage", "gone"]}),
    )
    .unwrap();
    assert_eq!(removed["deleted"], 2);
    assert!(stored(&f, "portal").is_none() && stored(&f, "mirage").is_none());
    assert!(stored(&f, "kept").is_some());
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&f.host.apps_path).unwrap()).unwrap();
    assert_eq!(
        saved["launcher-dismissed"],
        json!(["steam:620", "ubisoft:13504"])
    );
    let uuids: Vec<&str> = saved["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|app| app["uuid"].as_str())
        .collect();
    assert!(uuids.contains(&"kept"));
    assert!(!uuids.contains(&"portal") && !uuids.contains(&"mirage"));
    // Nothing to remove, nothing found, or a malformed list is refused and changes nothing.
    for bad in [
        json!({"uuids": []}),
        json!({"uuids": ["gone"]}),
        json!({"uuids": "kept"}),
        json!({"uuids": ["kept", 7]}),
    ] {
        assert!(
            handle(&f.host, "POST", "/api/apps/delete", &bad).is_err(),
            "{bad}"
        );
    }
    assert!(stored(&f, "kept").is_some());
}

#[test]
fn a_busy_or_failed_neural_rendering_removal_keeps_its_app_and_touches_nothing_early() {
    use std::cell::Cell;
    let f = Fixture::new();
    for app in [
        json!({"name": "Portal 2", "uuid": "portal", "steam-id": "620"}),
        json!({"name": "Mirage", "uuid": "mirage", "launcher-source": "ubisoft",
            "launcher-id": "13504"}),
        json!({"name": "Fortnite", "uuid": "fortnite", "launcher-source": "epic",
            "launcher-id": "f"}),
        json!({"name": "Installing", "uuid": "busy", "launcher-source": "gog",
            "launcher-id": "b"}),
    ] {
        handle(&f.host, "POST", "/api/apps", &app).unwrap();
    }
    let removals = Cell::new(0);
    let count = |_: &crate::neural::Removing, _: &butterpollo_core::state::App| {
        removals.set(removals.get() + 1);
        Ok(())
    };
    // An install still running: refused before any app's mod is taken out.
    crate::neural::start_job(&f.host, "busy", "install", "download").unwrap();
    let busy = super::delete_apps_with(&f.host, &["portal", "busy"], count).unwrap_err();
    assert!(
        format!("{busy:#}").contains("busy with Installing"),
        "{busy:#}"
    );
    assert_eq!(removals.get(), 0);
    assert!(stored(&f, "portal").is_some() && stored(&f, "busy").is_some());
    // Mirage's mod stays in its folder partway through: Mirage stays, the others go.
    let deleted = super::delete_apps_with(&f.host, &["portal", "mirage", "fortnite"], |_, app| {
        if app.name == "Mirage" {
            anyhow::bail!("a file is in use");
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(deleted.count, 2);
    assert_eq!(
        deleted.kept,
        [json!({"name": "Mirage", "error": "a file is in use"})]
    );
    assert!(stored(&f, "mirage").is_some());
    assert!(stored(&f, "portal").is_none() && stored(&f, "fortnite").is_none());
    assert_eq!(
        f.host.app_document.read().unwrap()["launcher-dismissed"],
        json!(["steam:620", "epic:f"])
    );
    // Every removal failing is the error itself, and the app stays.
    let failed =
        super::delete_apps_with(&f.host, &["mirage"], |_, _| anyhow::bail!("still in use"))
            .unwrap_err();
    assert_eq!(failed.to_string(), "still in use");
    assert!(stored(&f, "mirage").is_some());
}

/// Purging synced apps waits on a worker, so the web server answers
/// meanwhile: a game it drops loses Neural Rendering through a helper
/// process, which can take minutes.
#[tokio::test(flavor = "current_thread")]
async fn purging_synced_apps_never_blocks_the_web_server() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new();
    let h = f.host.clone();
    // The apps held elsewhere for a while, as a slow removal holds the purge.
    let (held, holding) = std::sync::mpsc::channel();
    let reader = h.clone();
    let holder = std::thread::spawn(move || {
        let _apps = reader.apps.read().unwrap();
        held.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
    });
    holding.recv().unwrap();
    let answered = std::sync::Arc::new(AtomicBool::new(false));
    let other = answered.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        other.store(true, Ordering::SeqCst);
    });
    let response = super::purge_autosync(&h).await;
    assert!(
        answered.load(Ordering::SeqCst),
        "nothing else ran during the purge"
    );
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    holder.join().unwrap();
}
