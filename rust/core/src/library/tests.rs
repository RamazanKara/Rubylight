use super::*;

fn selected(game: &Game) -> Selection {
    Selection {
        source: game.source,
        id: game.id.clone(),
    }
}
fn epic_game() -> Game {
    epic(include_bytes!("fixtures/epic.item")).unwrap().unwrap()
}

#[test]
fn epic_manifest_uses_identifiers_and_preserves_unicode_nondefault_installations() {
    let game = epic_game();
    assert_eq!(game.name, "Étoile");
    assert_eq!(game.install_dir, r"E:\Jeux\Étoile");
    assert_eq!(game.id, "namespace-1:item-2:Orion");
    assert_eq!(
        game.launch.unwrap().command(),
        "com.epicgames.launcher://apps/namespace-1%3Aitem-2%3AOrion?action=launch&silent=true"
    );
    let mut record: Value = serde_json::from_slice(include_bytes!("fixtures/epic.item")).unwrap();
    record["bIsIncompleteInstall"] = json!(true);
    assert!(
        epic(&serde_json::to_vec(&record).unwrap())
            .unwrap()
            .is_none()
    );
    record["bIsIncompleteInstall"] = json!(false);
    record["MainGameAppName"] = json!("Parent");
    assert!(
        epic(&serde_json::to_vec(&record).unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn ea_content_ids_are_read_from_xml_and_escaped_for_the_launcher() {
    let game = ea(
        include_str!("fixtures/ea.xml"),
        Path::new(r"F:\Игры\EA"),
        "Mass Effect",
    )
    .unwrap();
    assert_eq!(game.id, "OFB-EAST:109552316");
    assert_eq!(game.install_dir, r"F:\Игры\EA");
    assert_eq!(
        game.launch.unwrap().command(),
        "origin2://game/launch?offerIds=OFB-EAST%3A109552316,Origin.OFR.50.0000462"
    );
    assert!(
        ea(
            "<DiPManifest><contentIDs/></DiPManifest>",
            Path::new("E:/Games"),
            "EA"
        )
        .is_err()
    );
    assert!(ea("<DiPManifest><contentIDs>", Path::new("E:/Games"), "EA").is_err());
}

#[test]
fn battlenet_database_and_registered_products_share_verified_commands() {
    let database = battlenet_database(include_bytes!("fixtures/product.db")).unwrap();
    assert_eq!(
        database,
        vec![("fen".into(), PathBuf::from(r"E:\Jeux\Diablo IV"))]
    );
    let registrations: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/battlenet.json")).unwrap();
    let launcher = Path::new(r"D:\Launchers\Battle.net Launcher.exe");
    let games: Vec<Game> = registrations
        .iter()
        .filter_map(|record| {
            battlenet(
                record["id"].as_str().unwrap(),
                Path::new(record["folder"].as_str().unwrap()),
                Some(launcher),
            )
        })
        .collect();
    assert_eq!(games.len(), 2);
    assert_eq!(games[0].name, "Diablo IV");
    assert_eq!(
        games[0].launch.as_ref().unwrap().command(),
        r#""D:\Launchers\Battle.net Launcher.exe" --exec="launch Fen""#
    );
    assert!(games[1].launch.is_none());
    assert!(
        games[1]
            .unavailable_reason
            .as_ref()
            .unwrap()
            .contains("not recognized")
    );
    assert!(
        battlenet("fen", Path::new("E:/Games/Diablo IV"), None)
            .unwrap()
            .app()
            .is_err()
    );
    assert!(battlenet_database(&[0x0a, 0xff]).is_err());
}

#[test]
fn ubisoft_uses_registered_game_id_not_title_or_installation_drive() {
    let record: Value = serde_json::from_str(include_str!("fixtures/ubisoft.json")).unwrap();
    let game = ubisoft(
        record["id"].as_str().unwrap(),
        Path::new(record["folder"].as_str().unwrap()),
        record["name"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(game.launch.unwrap().command(), "uplay://launch/13504");
    assert_eq!(game.install_dir, r"F:\Игры\Mirage");
    assert!(ubisoft("13504/0", Path::new("E:/Games"), "Game").is_err());
}

#[test]
fn gog_prefers_galaxy_and_falls_back_to_the_primary_action_with_its_working_directory() {
    let data = include_bytes!("fixtures/gog.info");
    let folder = Path::new("G:/GOG/Étoile");
    let galaxy = Path::new("D:/Launchers/GOG Galaxy/GalaxyClient.exe");
    let game = gog(data, folder, Some(galaxy)).unwrap().unwrap();
    assert!(
        game.launch
            .unwrap()
            .command()
            .contains("/gameId=1207664663 /command=runGame")
    );
    let game = gog(data, folder, None).unwrap().unwrap();
    assert_eq!(game.working_dir, folder.join("bin/x64"));
    assert_eq!(
        game.launch.unwrap(),
        Launch::Program {
            program: folder
                .join("bin/x64/witcher3.exe")
                .to_string_lossy()
                .into_owned(),
            arguments: "-language en".into()
        }
    );
    let mut dlc: Value = serde_json::from_slice(data).unwrap();
    dlc["gameId"] = json!("1234");
    assert!(
        gog(&serde_json::to_vec(&dlc).unwrap(), folder, Some(galaxy))
            .unwrap()
            .is_none()
    );
}

#[test]
fn malformed_records_and_unsafe_actions_are_rejected() {
    assert!(epic(b"{broken").is_err());
    assert!(gog(b"{}", Path::new("E:/Games"), None).is_err());
    let mut record: Value = serde_json::from_slice(include_bytes!("fixtures/gog.info")).unwrap();
    record["playTasks"][0]["path"] = json!("../another.exe");
    assert!(
        gog(
            &serde_json::to_vec(&record).unwrap(),
            Path::new("E:/Games"),
            None
        )
        .is_err()
    );
    record["playTasks"][0]["path"] = json!("C:/another.exe");
    assert!(
        gog(
            &serde_json::to_vec(&record).unwrap(),
            Path::new("E:/Games"),
            None
        )
        .is_err()
    );
    let mut record: Value = serde_json::from_slice(include_bytes!("fixtures/epic.item")).unwrap();
    record["AppName"] = json!("bad&parameter");
    record["MainGameAppName"] = record["AppName"].clone();
    assert!(epic(&serde_json::to_vec(&record).unwrap()).is_err());
}

#[test]
fn repeated_imports_preserve_customizations_and_same_title_store_copies() {
    let epic = epic_game();
    let gog = gog(
        include_bytes!("fixtures/gog.info"),
        Path::new("G:/Games"),
        None,
    )
    .unwrap()
    .unwrap();
    let gog = Game {
        name: epic.name.clone(),
        ..gog
    };
    let games = vec![epic, gog];
    let selections: Vec<_> = games.iter().map(selected).collect();
    let mut apps = vec![json!({ "uuid": "manual", "name": "Desktop", "custom": {"keep": true} })];
    let results = import(&mut apps, &games, &selections);
    assert!(results.iter().all(|result| result.result == "added"));
    assert_ne!(apps[1]["uuid"], apps[2]["uuid"]);
    apps[1]["name"] = json!("My game");
    apps[1]["cmd"] = json!("my-command");
    apps[1]["image-path"] = json!("F:/My cover.png");
    apps[1]["prep-cmd"] = json!([{"do": "before", "undo": "after"}]);
    apps[1]["custom"] = json!(17);
    let saved = apps.clone();
    assert!(
        import(&mut apps, &games, &selections)
            .iter()
            .all(|result| result.result == "already_present")
    );
    assert_eq!(apps, saved);
    assert!(
        import(&mut apps, &[], &selections)
            .iter()
            .all(|result| result.result == "failed")
    );
    assert_eq!(
        apps, saved,
        "a failed scan must never remove or change existing entries"
    );
}

#[test]
fn a_missing_launcher_fails_only_that_selection() {
    let mut missing = epic_game();
    missing.unavailable_reason = Some("Epic Games Launcher is unavailable".into());
    let working = ubisoft("13504", Path::new("E:/Games/Mirage"), "Mirage").unwrap();
    let games = [missing, working];
    let mut apps = vec![];
    let results = import(
        &mut apps,
        &games,
        &games.iter().map(selected).collect::<Vec<_>>(),
    );
    assert_eq!(results[0].result, "failed");
    assert_eq!(results[1].result, "added");
    assert_eq!(apps.len(), 1);
}

#[test]
fn manually_imported_steam_games_survive_automatic_removal_and_keep_existing_metadata() {
    let game = Game::new(
        Source::Steam,
        "570".into(),
        "Dota 2".into(),
        Path::new("F:/Steam/steamapps/common/dota 2 beta"),
        Some(Launch::Uri {
            uri: "steam://rungameid/570".into(),
        }),
    );
    let mut apps = vec![];
    assert_eq!(
        import(&mut apps, std::slice::from_ref(&game), &[selected(&game)])[0].result,
        "added"
    );
    assert_eq!(apps[0]["steam-managed"], "manual");
    assert_eq!(apps[0]["cmd"], crate::steam::launch_command(570));
    let mut settings = crate::steam::Settings::from_config(&crate::config::Config::default());
    settings.sync_all_installed = true;
    settings.remove_uninstalled = true;
    assert!(!crate::steam::reconcile(
        &mut apps,
        &[],
        &settings,
        &Default::default()
    ));
    assert_eq!(apps.len(), 1);
    apps[0]["steam-source"] = json!("custom metadata");
    let saved = apps.clone();
    assert_eq!(
        import(
            &mut apps,
            &[game],
            &[Selection {
                source: Source::Steam,
                id: "570".into()
            }]
        )[0]
        .result,
        "already_present"
    );
    assert_eq!(apps, saved);
}
