//! Tests for Epic, EA, Battle.net, Ubisoft, GOG parsers and explicit imports.
use super::super::battlenet::{BattleDatabase, BattleInstall, BattleSettings};
use super::*;
use prost::Message;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

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
    let mut record: Value = serde_json::from_slice(EPIC_ITEM).unwrap();
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
    let game = ea(EA_XML, Path::new(r"F:\Игры\EA"), "Mass Effect").unwrap();
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

/// Without Origin's `origin2` scheme the EA app's own `link2ea` scheme starts the game.
#[test]
fn ea_link2ea_launches_the_first_content_id() {
    assert_eq!(
        ea_link2ea("OFB-EAST:109552316").command(),
        "link2ea://launchgame/OFB-EAST%3A109552316?platform=PCWIN"
    );
}

#[test]
fn a_folder_is_the_same_whatever_its_case_slashes_or_quotes() {
    let folder = r"D:\Games\EA Games\Battlefield 2042";
    for other in [
        r"d:\games\ea games\battlefield 2042",
        "D:/Games/EA Games/Battlefield 2042/",
        r#""D:\Games\EA Games\Battlefield 2042\""#,
        r" D:\Games\EA Games\Battlefield 2042 ",
    ] {
        assert!(same_folder(folder, other), "{other}");
    }
    assert!(!same_folder(r"D:\Games\EA Games\Battlefield", folder));
    assert!(!same_folder("", ""), "an empty folder is no folder");
    assert!(!same_folder(r#""""#, ""));
}

#[test]
fn battlenet_database_and_registered_products_share_verified_commands() {
    let database = battlenet_database(PRODUCT_DB).unwrap();
    assert_eq!(
        database,
        vec![(
            "fen".into(),
            "fen".into(),
            PathBuf::from(r"E:\Jeux\Diablo IV")
        )]
    );
    let registrations: Vec<Value> = serde_json::from_str(BATTLENET_JSON).unwrap();
    let launcher = Path::new(r"D:\Launchers\Battle.net Launcher.exe");
    let games: Vec<Game> = registrations
        .iter()
        .filter_map(|record| {
            battlenet(
                record["id"].as_str().unwrap(),
                "",
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
        battlenet("fen", "", Path::new("E:/Games/Diablo IV"), None)
            .unwrap()
            .app()
            .is_err()
    );
    assert!(battlenet_database(&[0x0a, 0xff]).is_err());
}

/// Current Battle.net writes a long uid (`fenris`) beside the product code (`fen`) the
/// launcher starts it by.
#[test]
fn battlenet_finds_a_product_by_its_code_when_the_uid_is_long() {
    let bytes = BattleDatabase {
        games: vec![
            BattleInstall {
                uid: "fenris".into(),
                product: "fen".into(),
                settings: Some(BattleSettings {
                    path: r"E:\Jeux\Diablo IV".into(),
                }),
            },
            BattleInstall {
                uid: "wow_retail".into(),
                product: "wow".into(),
                settings: Some(BattleSettings {
                    path: r"E:\Jeux\World of Warcraft".into(),
                }),
            },
            BattleInstall {
                uid: "bna".into(),
                product: "bna".into(),
                settings: Some(BattleSettings {
                    path: r"C:\Battle.net".into(),
                }),
            },
        ],
    }
    .encode_to_vec();
    let launcher = Path::new(r"D:\Launchers\Battle.net Launcher.exe");
    let games: Vec<Game> = battlenet_database(&bytes)
        .unwrap()
        .iter()
        .filter_map(|(uid, product, folder)| battlenet(uid, product, folder, Some(launcher)))
        .collect();
    assert_eq!(games.len(), 2);
    let diablo = &games[0];
    assert_eq!(diablo.name, "Diablo IV");
    assert_eq!(diablo.id, "fenris");
    assert_eq!(diablo.install_dir, r"E:\Jeux\Diablo IV");
    assert_eq!(diablo.unavailable_reason, None);
    assert_eq!(
        diablo.launch.as_ref().unwrap().command(),
        r#""D:\Launchers\Battle.net Launcher.exe" --exec="launch Fen""#
    );
    assert_eq!(games[1].name, "World of Warcraft");
    assert_eq!(games[1].id, "wow_retail");
    assert_eq!(games[1].unavailable_reason, None);
    assert_eq!(
        games[1].launch.as_ref().unwrap().command(),
        r#""D:\Launchers\Battle.net Launcher.exe" --exec="launch WoW""#
    );
    // World of Warcraft: Forever (`WoWF` / `wow_forever`) resolves to its own product code (#137).
    for (uid, product, expected_id) in [
        ("wowf", "", "wowf"),
        ("wowf", "wow", "wowf"),
        ("wow_forever", "", "wow_forever"),
        ("", "WoWF", "wowf"),
    ] {
        let game = battlenet(
            uid,
            product,
            Path::new(r"E:\Jeux\World of Warcraft"),
            Some(launcher),
        )
        .unwrap();
        assert_eq!(
            game.name, "World of Warcraft: Forever",
            "{uid:?} {product:?}"
        );
        assert_eq!(game.id, expected_id, "{uid:?} {product:?}");
        assert_eq!(game.unavailable_reason, None, "{uid:?} {product:?}");
        assert_eq!(
            game.launch.as_ref().unwrap().command(),
            r#""D:\Launchers\Battle.net Launcher.exe" --exec="launch WoWF""#,
            "{uid:?} {product:?}"
        );
    }
    // The same id each scan, so a synced app keeps matching its game.
    assert_eq!(
        battlenet("fenris", "fen", Path::new("E:/Jeux/Diablo IV"), None)
            .unwrap()
            .id,
        "fenris"
    );
    // A known long uid without a product code, and a code without a uid.
    for (uid, product) in [("fenris", ""), ("", "FEN"), ("Prometheus", "")] {
        let game = battlenet(uid, product, Path::new("E:/Games/Game"), Some(launcher)).unwrap();
        assert_eq!(game.unavailable_reason, None, "{uid:?} {product:?}");
    }
    assert_eq!(
        battlenet("", "FEN", Path::new("E:/Games/Diablo IV"), None)
            .unwrap()
            .id,
        "fen"
    );
    // The uid decides before the product code: a variant with a known uid of its own keeps
    // its own name and launch code even if its product code names another game.
    let variant = battlenet("diablo3", "fen", Path::new("E:/Games/D3"), Some(launcher)).unwrap();
    assert_eq!(variant.name, "Diablo III");
    assert_eq!(
        variant.launch.unwrap().command(),
        r#""D:\Launchers\Battle.net Launcher.exe" --exec="launch D3""#
    );
}

/// Written byte by byte rather than with the decoder's own structs: uid is field 1 and the
/// product code field 2 of each installation (field 1), whose settings (field 3) hold the
/// path (field 1).
#[test]
fn battlenet_database_reads_the_uid_and_product_code_from_their_own_fields() {
    let bytes = PRODUCT_FENRIS_DB;
    assert_eq!(&bytes[..4], b"\x0a\x22\x0a\x06");
    assert_eq!(
        battlenet_database(bytes).unwrap(),
        vec![(
            "fenris".into(),
            "fen".into(),
            PathBuf::from(r"E:\Jeux\Diablo IV")
        )]
    );
}

#[test]
fn a_battlenet_game_product_db_lists_is_not_listed_again_from_the_registry() {
    let entry = |uid: &str, product: &str, folder: &str| {
        (uid.to_owned(), product.to_owned(), PathBuf::from(folder))
    };
    let database = vec![
        entry("fenris", "fen", r"E:\Jeux\Diablo IV"),
        entry("prometheus", "", r"E:\Jeux\Overwatch"),
    ];
    let registry = vec![
        // The same product code, without a uid.
        entry("", "FEN", r"E:\Jeux\Diablo IV"),
        // The same uid.
        entry("Prometheus", "pro", r"E:\Jeux\Overwatch"),
        // No uid and a product code product.db lacks, but the same known product.
        entry("", "pro", r"E:\Jeux\Overwatch"),
        // Only the registry knows these.
        entry("", "osi", r"E:\Jeux\Diablo II Resurrected"),
        entry("w3", "w3", r"E:\Jeux\Warcraft III"),
        entry("", "ngm", r"E:\Jeux\New"),
        // The registry lists one twice.
        entry("", "ngm", r"E:\Jeux\New"),
    ];
    let entries = battlenet_entries(database, registry);
    let ids: Vec<String> = entries
        .iter()
        .filter_map(|(uid, product, folder)| battlenet(uid, product, folder, None))
        .map(|game| game.id)
        .collect();
    assert_eq!(ids, ["fenris", "prometheus", "osi", "w3", "ngm"]);
    // Without product.db the registry alone still lists Diablo IV, under its product code.
    let alone = battlenet_entries(vec![], vec![entry("", "fen", r"E:\Jeux\Diablo IV")]);
    assert_eq!(alone, [entry("", "fen", r"E:\Jeux\Diablo IV")]);
}

#[test]
fn an_unknown_battlenet_product_is_named_after_its_folder() {
    let launcher = Path::new(r"D:\Launchers\Battle.net Launcher.exe");
    let game = battlenet(
        "newgame",
        "ngm",
        Path::new("E:/Jeux/Brand New Game"),
        Some(launcher),
    )
    .unwrap();
    assert_eq!(game.name, "Brand New Game");
    assert_eq!(game.id, "newgame");
    assert!(game.launch.is_none());
    assert!(
        game.unavailable_reason
            .as_deref()
            .unwrap()
            .contains("not recognized")
    );
    // Without a folder name the raw id is all there is.
    assert_eq!(
        battlenet("newgame", "", Path::new(""), Some(launcher))
            .unwrap()
            .name,
        "newgame"
    );
    assert_eq!(
        battlenet("", "ngm", Path::new("/"), None).unwrap().name,
        "ngm"
    );
}

#[test]
fn battlenet_uninstall_commands_name_the_uid_and_product_code() {
    assert_eq!(
        battlenet_uninstall(
            r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS --uid=fenris --productcode=fen --displayname="Diablo IV""#
        ),
        Some(("fenris".into(), "fen".into()))
    );
    assert_eq!(
        battlenet_uninstall(
            r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --ProductCode="pro" --UID="prometheus""#
        ),
        Some(("prometheus".into(), "pro".into()))
    );
    assert_eq!(
        battlenet_uninstall(r#""C:\Battle.net\Uninstaller.exe" --uid=wow"#),
        Some(("wow".into(), String::new()))
    );
    assert_eq!(
        battlenet_uninstall(r#""C:\Battle.net\Uninstaller.exe" --productcode=osi"#),
        Some((String::new(), "osi".into()))
    );
    assert_eq!(
        battlenet_uninstall(r#""C:\Program Files (x86)\Battle.net\Battle.net Uninstaller.exe""#),
        None
    );
    assert_eq!(
        battlenet_uninstall("Uninstaller.exe --uid= --productcode="),
        None
    );
}

#[test]
fn ubisoft_uses_registered_game_id_not_title_or_installation_drive() {
    let record: Value = serde_json::from_str(UBISOFT_JSON).unwrap();
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
    let data = GOG_INFO;
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
    let mut record: Value = serde_json::from_slice(GOG_INFO).unwrap();
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
    let mut record: Value = serde_json::from_slice(EPIC_ITEM).unwrap();
    record["AppName"] = json!("bad&parameter");
    record["MainGameAppName"] = record["AppName"].clone();
    assert!(epic(&serde_json::to_vec(&record).unwrap()).is_err());
}

#[test]
fn repeated_imports_preserve_customizations_and_same_title_store_copies() {
    let epic = epic_game();
    let gog = gog(GOG_INFO, Path::new("G:/Games"), None).unwrap().unwrap();
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
    let mut settings =
        crate::steam::Settings::from_config(&butterpollo_config::config::Config::default());
    settings.sync_all_installed = true;
    settings.remove_uninstalled = true;
    assert!(!crate::steam::reconcile(
        &mut apps,
        &[],
        &settings,
        &Default::default(),
        &[]
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
#[test]
fn a_gog_registry_record_without_an_info_file_is_a_game() {
    let values = gog_registry_values();
    let folder = Path::new(r"D:\GOG Games\Unreal Tournament 2004");
    let game = gog_registry(&values).unwrap().unwrap();
    assert_eq!(
        (game.source, game.id.as_str(), game.name.as_str()),
        (Source::Gog, "1207658924", "Unreal Tournament 2004")
    );
    assert_eq!(game.install_dir, folder.to_string_lossy());
    assert_eq!(game.working_dir, folder.join("System"));
    // Its own program: Galaxy needs the play tasks of a goggame-<id>.info.
    assert_eq!(
        game.launch.clone().unwrap(),
        Launch::Program {
            program: r"D:\GOG Games\Unreal Tournament 2004\System\UT2004.exe".into(),
            arguments: "-nohomedir".into()
        }
    );
    // The same id as its goggame-<id>.info would give, so the scan lists it once.
    assert!(crate::native::Scan::from_games(&[game]).contains("gog:1207658924"));
}

#[test]
fn gog_registry_dlc_and_records_pointing_outside_their_folder_are_not_games() {
    let mut values = gog_registry_values();
    values.insert("dependsOn".into(), "1207658924".into());
    assert!(gog_registry(&values).unwrap().is_none());
    for outside in [
        r"C:\Windows\System32\cmd.exe",
        r"D:\GOG Games\Unreal Tournament 2004\..\..\Windows\System32\cmd.exe",
    ] {
        let mut values = gog_registry_values();
        values.insert("exe".into(), outside.into());
        assert!(gog_registry(&values).is_err(), "{outside}");
    }
    let mut values = gog_registry_values();
    values.insert("gameID".into(), "12 & calc".into());
    assert!(gog_registry(&values).is_err());
    let mut values = gog_registry_values();
    values.remove("gameName");
    assert!(gog_registry(&values).is_err());
}
