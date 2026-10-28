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

const MICROSOFT_CONFIG: &str = include_str!("fixtures/microsoft_game.config");
const FAMILY: &str = "Contoso.Racing_8wekyb3d8bbwe";
fn manifest(application: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="Contoso.Racing" Publisher="CN=Contoso" Version="1.4.0.0" />
  <Applications>{application}</Applications>
</Package>"#
    )
}
fn racing(package_manifest: &str, display_name: &str) -> Result<Option<Game>> {
    microsoft(
        MICROSOFT_CONFIG,
        Path::new(r"C:\XboxGames\Contoso Racing\Content"),
        Some(&StorePackage {
            family: FAMILY,
            display_name,
            manifest: package_manifest,
        }),
    )
}

#[test]
fn a_microsoft_game_starts_through_its_application_id_and_ignores_console_programs() {
    let installed = manifest(
        r#"<Application Id="Console" Executable="Launcher.exe" EntryPoint="Windows.FullTrustApplication" />
           <Application Id="Game" Executable="Racing_Shipping.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let game = racing(&installed, "ms-resource:AppName").unwrap().unwrap();
    assert_eq!(game.source, Source::Microsoft);
    assert_eq!(game.id, "Contoso.Racing");
    assert_eq!(game.name, "Contoso Racing: Étoile");
    assert_eq!(game.unavailable_reason, None);
    // The PC program's Id, not the Xbox one that comes first in the manifest.
    assert_eq!(
        game.launch.as_ref().unwrap().command(),
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game"
    );
    assert_eq!(game.install_dir, r"C:\XboxGames\Contoso Racing\Content");

    // It imports like any other source, keyed by the identity name.
    let mut apps = vec![];
    assert_eq!(
        import(
            &mut apps,
            &[game],
            &[Selection {
                source: Source::Microsoft,
                id: "Contoso.Racing".into()
            }]
        )[0]
        .result,
        "added"
    );
    assert_eq!(apps[0]["launcher-source"], "microsoft");
    assert_eq!(
        apps[0]["cmd"],
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game"
    );
}

#[test]
fn the_program_name_finds_the_application_when_the_ids_differ() {
    let installed = manifest(
        r#"<Application Id="App" Executable="bin\Racing_Shipping.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let game = racing(&installed, "").unwrap().unwrap();
    assert_eq!(
        game.launch.unwrap().command(),
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!App"
    );
}

#[test]
fn a_microsoft_game_nothing_in_the_package_starts_is_an_error_not_a_guess() {
    let installed = manifest(
        r#"<Application Id="Other" Executable="Other.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let error = racing(&installed, "").err().unwrap().to_string();
    assert!(error.contains("Racing_Shipping.exe"), "{error}");
    let none = manifest("");
    assert!(racing(&none, "").is_err());
}

#[test]
fn a_package_or_application_name_with_odd_characters_never_becomes_a_launch() {
    let hostile =
        manifest(r#"<Application Id="Game&amp;calc" Executable="Racing_Shipping.exe" />"#);
    assert!(racing(&hostile, "").is_err());
    for family in [
        "NoUnderscore",
        "Name_Pub lisher",
        r"a\b_c",
        "_pub",
        "name_",
        "a_b_c",
    ] {
        let installed = manifest(r#"<Application Id="Game" Executable="Racing_Shipping.exe" />"#);
        let package = StorePackage {
            family,
            display_name: "",
            manifest: &installed,
        };
        assert!(
            microsoft(MICROSOFT_CONFIG, Path::new("C:/g"), Some(&package)).is_err(),
            "{family}"
        );
    }
}

#[test]
fn a_console_only_or_programless_config_is_not_a_pc_game() {
    let xbox_only =
        MICROSOFT_CONFIG.replace(r#"TargetDeviceFamily="PC""#, r#"TargetDeviceFamily="Xbox""#);
    assert!(
        microsoft(&xbox_only, Path::new("C:/g"), None)
            .unwrap()
            .is_none()
    );
    let empty = r#"<Game><Identity Name="Contoso.Empty" /></Game>"#;
    assert!(microsoft(empty, Path::new("C:/g"), None).unwrap().is_none());
    // A program with no family listed is for PCs.
    let bare = r#"<Game><Identity Name="Contoso.Bare" /><ExecutableList><Executable Name="bare.exe" Id="Game" /></ExecutableList></Game>"#;
    assert_eq!(
        microsoft(bare, Path::new("C:/g"), None)
            .unwrap()
            .unwrap()
            .name,
        "Contoso.Bare"
    );
    assert!(microsoft("<Game><not-closed>", Path::new("C:/g"), None).is_err());
}

#[test]
fn a_game_with_no_registered_package_is_listed_but_cannot_start() {
    let game = microsoft(
        MICROSOFT_CONFIG,
        Path::new("D:/XboxGames/Racing/Content"),
        None,
    )
    .unwrap()
    .unwrap();
    assert!(game.launch.is_none());
    let reason = game.unavailable_reason.as_deref().unwrap();
    assert!(
        reason.contains("not installed for this Windows user"),
        "{reason}"
    );
    assert!(
        import(
            &mut vec![],
            &[game],
            &[Selection {
                source: Source::Microsoft,
                id: "Contoso.Racing".into()
            }]
        )[0]
        .error
        .is_some()
    );
}

#[test]
fn the_cover_is_the_config_logo_when_the_file_is_there() {
    let folder = tempfile::tempdir().unwrap();
    let package = manifest(r#"<Application Id="Game" Executable="Racing_Shipping.exe" />"#);
    let with = |folder: &Path| {
        microsoft(
            MICROSOFT_CONFIG,
            folder,
            Some(&StorePackage {
                family: FAMILY,
                display_name: "",
                manifest: &package,
            }),
        )
        .unwrap()
        .unwrap()
    };
    assert_eq!(with(folder.path()).cover, None);
    std::fs::create_dir(folder.path().join("Assets")).unwrap();
    std::fs::write(folder.path().join("Assets").join("Logo150.png"), b"png").unwrap();
    assert_eq!(
        with(folder.path()).cover,
        Some(folder.path().join(r"Assets\Logo150.png"))
    );
}

#[test]
fn microsoft_is_a_library_source_with_its_own_key() {
    assert_eq!(Source::ALL.len(), 8);
    assert_eq!(Source::Microsoft.key(), "microsoft");
    assert_eq!(
        serde_json::to_value(Source::Microsoft).unwrap(),
        "microsoft"
    );
}

#[test]
fn only_a_plain_application_id_is_a_store_launch_target() {
    assert_eq!(
        store_launch_target(r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game"),
        Some("Contoso.Racing_8wekyb3d8bbwe!Game")
    );
    for uri in [
        "steam://rungameid/570",
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game & calc",
        r"shell:AppsFolder\..\Windows\System32\cmd.exe",
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe",
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!",
        r"shell:Startup",
        r"shell:AppsFolder\{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\cmd.exe",
    ] {
        assert_eq!(store_launch_target(uri), None, "{uri}");
    }
}

fn gog_registry_values() -> BTreeMap<String, String> {
    serde_json::from_str(include_str!("fixtures/gog_registry.json")).unwrap()
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

/// The game folders in `fixtures/local_folders.json`, as the folder scan lists them.
fn local_folders() -> Vec<(PathBuf, Vec<Program>)> {
    let folders: BTreeMap<String, Vec<(String, u64)>> =
        serde_json::from_str(include_str!("fixtures/local_folders.json")).unwrap();
    folders
        .into_iter()
        .map(|(name, files)| {
            (
                Path::new(r"D:\Games").join(name),
                files
                    .into_iter()
                    .map(|(path, size)| Program {
                        path: path.into(),
                        size,
                    })
                    .collect(),
            )
        })
        .collect()
}
fn local_settings() -> Settings {
    Settings::from_config(
        &crate::config::Config::parse(
            r#"library_local_folders = ["D:\\Games", "E:/More games"]
library_local_exclude = ["portal clone", "F:\\Old\\Thing"]
library_gog_enabled = false
"#,
        )
        .unwrap(),
    )
}
/// Celeste, Hades and The Witcher 3, in that order.
fn local_games() -> Vec<Game> {
    let settings = local_settings();
    local_folders()
        .into_iter()
        .filter(|(folder, _)| !settings.excluded(folder))
        .filter_map(|(folder, programs)| local(&folder, &programs))
        .collect()
}

#[test]
fn library_settings_turn_launchers_on_by_default_and_read_the_game_folders() {
    let defaults = Settings::default();
    assert!(defaults.enabled(Source::Epic) && defaults.enabled(Source::Gog));
    assert!(
        !defaults.enabled(Source::Local),
        "no game folders by default"
    );
    assert!(defaults.enabled(Source::Steam) && defaults.enabled(Source::Microsoft));
    let settings = local_settings();
    assert!(settings.enabled(Source::Epic) && !settings.enabled(Source::Gog));
    assert!(settings.enabled(Source::Local));
    assert_eq!(
        settings.local_folders,
        [PathBuf::from(r"D:\Games"), PathBuf::from("E:/More games")]
    );
    assert!(settings.excluded(Path::new(r"D:\Games\Portal Clone")));
    assert!(settings.excluded(Path::new("f:/old/thing/")));
    assert!(!settings.excluded(Path::new(r"D:\Games\Hades")));
    let by_name = Settings {
        local_exclude: vec![r"PORTAL CLONE\".into()],
        ..Settings::default()
    };
    assert!(by_name.excluded(Path::new(r"D:\Games\Portal Clone")));
    // The scan reads a configured folder's subfolders that are not left out.
    assert!(settings.reads_local(r"d:\games\hades"));
    assert!(settings.reads_local(r"e:\more games\celeste"));
    assert!(!settings.reads_local(r"d:\games\portal clone"));
    assert!(!settings.reads_local(r"d:\games\hades\bin"));
    assert!(!settings.reads_local(r"c:\games\hades"));
}

#[test]
fn a_game_folder_starts_its_game_not_its_installers_redistributables_or_crash_reporters() {
    let games = local_games();
    let found: Vec<_> = games
        .iter()
        .map(|game| (game.name.as_str(), game.launch.as_ref().unwrap().command()))
        .collect();
    assert_eq!(
        found,
        [
            ("Celeste", r#""D:\Games\Celeste\Celeste.exe""#.to_owned()),
            ("Hades", r#""D:\Games\Hades\Hades.exe""#.to_owned()),
            (
                "The Witcher 3",
                r#""D:\Games\The Witcher 3\bin\x64\witcher3.exe""#.to_owned()
            ),
        ],
        "Runtimes holds only helpers, Too Deep's program is three folders down, Portal Clone is excluded"
    );
    let witcher = &games[2];
    assert_eq!(witcher.source, Source::Local);
    assert_eq!(witcher.id, r"d:\games\the witcher 3");
    assert_eq!(witcher.install_dir, r"D:\Games\The Witcher 3");
    assert_eq!(
        witcher.working_dir,
        Path::new(r"D:\Games\The Witcher 3\bin\x64")
    );
    assert!(witcher.unavailable_reason.is_none());
    // Known by its folder: a new program in it does not make it a new game.
    let (folder, mut programs) = local_folders().remove(4);
    programs.push(Program {
        path: "bin/x64/witcher3_dx12.exe".into(),
        size: 99_999_999,
    });
    let again = local(&folder, &programs).unwrap();
    assert_eq!(
        (again.id.as_str(), again.uuid()),
        (witcher.id.as_str(), witcher.uuid())
    );
    // "Crash" in the game's own name does not make it a crash reporter.
    let crashlands = local(
        Path::new(r"D:\Games\Crashlands"),
        &[
            Program {
                path: "Crashlands.exe".into(),
                size: 1_000_000,
            },
            Program {
                path: "CrashSender1403.exe".into(),
                size: 9_000_000,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        crashlands.launch.unwrap().command(),
        r#""D:\Games\Crashlands\Crashlands.exe""#
    );
}

#[test]
fn a_game_folder_a_launcher_already_lists_is_listed_once_under_the_launcher() {
    let mut epic = epic_game();
    epic.install_dir = "d:/games/hades/".into();
    let steam = Game::new(
        Source::Steam,
        "570".into(),
        "Dota 2".into(),
        Path::new(r"D:\Games\Steam\steamapps\common\dota 2 beta"),
        None,
    );
    let mut games = local_games();
    games.extend(local(
        Path::new(r"D:\Games\Steam"),
        &[Program {
            path: "steam.exe".into(),
            size: 4_000_000,
        }],
    ));
    // A launcher record pointing at the folder of game folders hides none of them.
    let library = Game::new(
        Source::Ea,
        "1".into(),
        "Broken record".into(),
        Path::new(r"D:\Games\"),
        None,
    );
    games.extend([epic, steam, library]);
    games.extend(local_games());
    let names: Vec<_> = finish(games)
        .into_iter()
        .map(|game| (game.name, game.source))
        .collect();
    assert_eq!(
        names,
        [
            ("Broken record".to_owned(), Source::Ea),
            ("Celeste".to_owned(), Source::Local),
            ("Dota 2".to_owned(), Source::Steam),
            ("The Witcher 3".to_owned(), Source::Local),
            ("Étoile".to_owned(), Source::Epic),
        ],
        "Hades is Epic's, D:\\Games\\Steam holds Steam's games, and repeats are dropped"
    );
}

#[test]
fn games_steam_or_playnite_already_keep_are_already_added() {
    let epic = epic_game();
    let witcher = local_games().remove(2);
    let playnite = |name: &str, plugin: &str| json!({"uuid": "P1", "name": name, "playnite-id": "p1", "playnite-plugin-name": plugin});
    // Playnite's Epic library added Étoile: the Epic scan's copy is that game.
    assert!(epic.matches(&playnite("ÉTOILE™", "Epic")));
    // Playnite's GOG copy of the same title is another copy, as with two launchers.
    assert!(!epic.matches(&playnite("Étoile", "GOG")));
    assert!(!epic.matches(&playnite("Another game", "Epic")));
    // Playnite's EA library from before the rename is still EA's.
    let ea = Game::new(
        Source::Ea,
        "1".into(),
        "Mass Effect".into(),
        Path::new(r"D:\EA\Mass Effect"),
        None,
    );
    assert!(ea.matches(&playnite("Mass Effect", "Origin")));
    assert!(ea.matches(&playnite("Mass Effect", "EA app")));
    assert!(!ea.matches(&playnite("Mass Effect", "Steam")));
    // A game folder is the game any Steam or Playnite app of that name is,
    // and any app started from that folder.
    assert!(witcher.matches(&playnite("The Witcher 3", "GOG")));
    assert!(witcher.matches(&json!({"name": "The Witcher 3", "steam-id": "292030"})));
    assert!(witcher.matches(
        &json!({"name": "Mine", "working-dir": "d:/games/the witcher 3/", "cmd": "w3.exe"})
    ));
    for started in [
        json!({"name": "Mine", "working-dir": r"D:\Games\The Witcher 3\bin\x64"}),
        json!({"name": "Mine", "cmd": r#""D:\Games\The Witcher 3\bin\x64\witcher3.exe" -dx12"#}),
        json!({"name": "Mine", "cmd": r"d:\games\the witcher 3\bin\x64\witcher3.exe -dx12"}),
    ] {
        assert!(witcher.matches(&started), "{started}");
    }
    for elsewhere in [
        json!({"name": "The Witcher 3", "cmd": "other.exe"}),
        json!({"name": "Mine", "cmd": r#""D:\Games\The Witcher 3 Mods\tool.exe""#}),
        json!({"name": "Mine", "working-dir": r"D:\Games\The Witcher 3\..\Hades"}),
    ] {
        assert!(!witcher.matches(&elsewhere), "{elsewhere}");
    }
    let mut apps = vec![json!({"name": "The Witcher 3", "steam-id": "292030"})];
    let results = import(
        &mut apps,
        std::slice::from_ref(&witcher),
        &[selected(&witcher)],
    );
    assert_eq!(results[0].result, "already_present");
    assert_eq!(apps.len(), 1);
}

#[test]
fn a_game_folder_import_starts_its_program_from_its_folder() {
    let hades = local_games().remove(1);
    let mut apps = vec![];
    assert_eq!(
        import(&mut apps, std::slice::from_ref(&hades), &[selected(&hades)])[0].result,
        "added"
    );
    let app = &apps[0];
    assert_eq!(app["cmd"], r#""D:\Games\Hades\Hades.exe""#);
    assert_eq!(app["launcher-source"], "local");
    assert_eq!(app["launcher-id"], r"d:\games\hades");
    assert_eq!(app["working-dir"], r"D:\Games\Hades");
    assert!(hades.matches(app));
}

fn sync_settings(extra: &str) -> Settings {
    Settings::from_config(&crate::config::Config::parse(extra).unwrap())
}
fn mirage() -> Game {
    ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage").unwrap()
}

#[test]
fn every_launcher_is_on_and_synced_by_default_like_steam() {
    let defaults = Settings::default();
    for source in [
        Source::Epic,
        Source::Battlenet,
        Source::Ea,
        Source::Ubisoft,
        Source::Gog,
        Source::Microsoft,
    ] {
        assert!(defaults.enabled(source), "{source:?}");
    }
    assert!(defaults.auto_sync && defaults.remove_uninstalled);
    let off = sync_settings(
        "library_battlenet_enabled = false\nlibrary_ea_enabled = false\nlibrary_ubisoft_enabled = false\nlibrary_microsoft_enabled = false\nlibrary_auto_sync = false\n",
    );
    for source in [
        Source::Battlenet,
        Source::Ea,
        Source::Ubisoft,
        Source::Microsoft,
    ] {
        assert!(!off.enabled(source), "{source:?}");
    }
    assert!(!off.auto_sync && off.enabled(Source::Epic));
}

#[test]
fn the_launcher_sync_adds_installed_games_once_and_leaves_steam_imports_and_dismissed_games() {
    let settings = Settings::default();
    let epic = epic_game();
    let mut broken = mirage();
    broken.id = "999".into();
    broken.name = "Broken".into();
    broken.unavailable_reason = Some("Install or repair Ubisoft Connect.".into());
    let mut dismissed = mirage();
    dismissed.id = "777".into();
    dismissed.name = "Deleted by the user".into();
    let steam = Game::new(
        Source::Steam,
        "570".into(),
        "Dota 2".into(),
        Path::new("C:/Steam/dota"),
        Some(Launch::Uri {
            uri: "steam://rungameid/570".into(),
        }),
    );
    let games = vec![epic.clone(), mirage(), broken, dismissed, steam];
    let mut apps = vec![json!({"name": "Desktop", "uuid": "d"})];
    let change = reconcile(
        &mut apps,
        &games,
        &settings,
        &Source::ALL,
        &["ubisoft:777".into()],
    );
    assert_eq!(
        change,
        SyncChange {
            added: 2,
            removed: 0
        }
    );
    let synced: Vec<&str> = apps
        .iter()
        .filter_map(|app| managed_key(app).map(|_| app["name"].as_str().unwrap()))
        .collect();
    assert_eq!(synced, [epic.name.as_str(), "Mirage"]);
    assert!(apps.iter().all(|app| app["launcher-source"] != "steam"));
    // Idempotent.
    assert_eq!(
        reconcile(
            &mut apps,
            &games,
            &settings,
            &Source::ALL,
            &["ubisoft:777".into()]
        ),
        SyncChange::default()
    );
    // A game imported by hand is not synced twice.
    let mut manual = vec![];
    import(&mut manual, &games, &[selected(&epic)]);
    assert_eq!(
        reconcile(&mut manual, &games[..1], &settings, &Source::ALL, &[]),
        SyncChange::default()
    );
    // Off: nothing changes.
    let off = sync_settings("library_auto_sync = false\n");
    let mut none = vec![];
    assert!(!reconcile(&mut none, &games, &off, &Source::ALL, &[]).changed() && none.is_empty());
    // A launcher that is off syncs nothing.
    let no_ubisoft = sync_settings("library_ubisoft_enabled = false\n");
    let mut epic_only = vec![];
    reconcile(&mut epic_only, &games, &no_ubisoft, &Source::ALL, &[]);
    assert_eq!(epic_only.len(), 1);
}

#[test]
fn an_uninstalled_synced_game_leaves_only_after_a_clean_read_and_manual_apps_stay() {
    let settings = Settings::default();
    let games = vec![epic_game(), mirage()];
    let mut apps = vec![];
    reconcile(&mut apps, &games, &settings, &Source::ALL, &[]);
    let mut kept = mirage();
    kept.id = "1".into();
    kept.name = "Imported by hand".into();
    import(&mut apps, &[kept.clone()], &[selected(&kept)]);
    assert_eq!(apps.len(), 3);
    // Ubisoft's read failed: its games stay although the scan has none.
    let read: Vec<Source> = Source::ALL
        .into_iter()
        .filter(|s| *s != Source::Ubisoft)
        .collect();
    assert_eq!(
        reconcile(&mut apps, &games[..1], &settings, &read, &[]).removed,
        0
    );
    // A clean read without Mirage removes the synced copy, never the import.
    let change = reconcile(&mut apps, &games[..1], &settings, &Source::ALL, &[]);
    assert_eq!(
        change,
        SyncChange {
            added: 0,
            removed: 1
        }
    );
    assert!(apps.iter().any(|app| app["name"] == "Imported by hand"));
    assert!(!apps.iter().any(|app| app["name"] == "Mirage"));
    // Without remove-uninstalled the synced copy stays.
    let keep = sync_settings("library_autosync_remove_uninstalled = false\n");
    let mut apps = vec![];
    reconcile(&mut apps, &games, &keep, &Source::ALL, &[]);
    assert_eq!(
        reconcile(&mut apps, &[], &keep, &Source::ALL, &[]).removed,
        0
    );
    assert_eq!(apps.len(), 2);
}
