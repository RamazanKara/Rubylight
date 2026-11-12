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
    // Two launch helpers: neither is known to be the game's.
    let helpers = manifest(
        r#"<Application Id="A" Executable="GameLaunchHelper.exe" />
           <Application Id="B" Executable="GameLaunchHelper.exe" />"#,
    );
    assert!(racing(&helpers, "").is_err());
}

/// j-scrizz's 3.4.0 scan: Xbox PC games and their packages listed as errors.
#[test]
fn an_xbox_pc_game_starts_through_its_launch_helper_and_a_package_without_applications_is_none() {
    let installed = manifest(
        r#"<Application Id="Racing" Executable="GameLaunchHelper.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let game = racing(&installed, "").unwrap().unwrap();
    assert_eq!(
        game.launch.unwrap().command(),
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Racing"
    );
    // Nothing to start, as a DLC's package: not a game, not an error.
    assert!(racing(&manifest(""), "").unwrap().is_none());
    let bare = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"><Identity Name="Contoso.Racing" /></Package>"#;
    assert!(racing(bare, "").unwrap().is_none());
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
    // Steam's defaults for every launcher and the game folders.
    let steam = crate::steam::Settings::from_config(&crate::config::Config::default());
    for source in SYNCED {
        let options = defaults.options(source).unwrap();
        assert_eq!(options.auto_sync, steam.auto_sync, "{source:?}");
        assert_eq!(
            options.remove_uninstalled, steam.remove_uninstalled,
            "{source:?}"
        );
        assert_eq!(
            options.sync_all_installed, steam.sync_all_installed,
            "{source:?}"
        );
        assert_eq!(options.recent_games, steam.recent_games, "{source:?}");
        assert_eq!(
            options.recent_max_age_days, steam.recent_max_age_days,
            "{source:?}"
        );
        assert!(options.exclusions.is_empty(), "{source:?}");
    }
    assert!(
        defaults.options(Source::Steam).is_none(),
        "Steam keeps its own sync"
    );
    assert!(defaults.any_automatic());
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
    assert!(off.enabled(Source::Epic) && !off.options(Source::Epic).unwrap().auto_sync);
    assert!(!off.any_automatic());
    // Off for one launcher only: the automatic sync skips it and keeps the rest.
    let gog_by_hand = sync_settings("gog_auto_sync = false\n");
    let automatic = gog_by_hand.automatic();
    assert!(
        gog_by_hand.enabled(Source::Gog),
        "Sync now and Add games still read it"
    );
    assert!(!automatic.options(Source::Gog).unwrap().enabled);
    assert!(automatic.options(Source::Epic).unwrap().enabled);
    // Only Battle.net keeps a play history: the others always add every installed game.
    let recent = sync_settings(
        "battlenet_sync_all_installed = false\nepic_sync_all_installed = false\nbattlenet_recent_games = 3\n",
    );
    assert!(
        !recent
            .options(Source::Battlenet)
            .unwrap()
            .sync_all_installed
    );
    assert_eq!(recent.options(Source::Battlenet).unwrap().recent_games, 3);
    assert!(recent.options(Source::Epic).unwrap().sync_all_installed);
}

#[test]
fn a_launchers_games_to_leave_out_are_never_added_and_their_synced_apps_go() {
    let settings = sync_settings(
        "ubisoft_exclude_games = [{\"name\": \"MIRAGE\"}]\nepic_exclude_games = [\"namespace-1:item-2:Orion\"]\n",
    );
    let ubisoft = settings.options(Source::Ubisoft).unwrap();
    assert!(
        ubisoft.excludes("13504", "Mirage"),
        "by name, as names compare"
    );
    assert!(!ubisoft.excludes("13504", "Mirage 2"));
    // A plain entry is an id or a name.
    let plain = sync_settings("gog_exclude_games = [\"1207658924\", \"The Witcher\"]\n");
    let gog = plain.options(Source::Gog).unwrap();
    assert!(gog.excludes("1207658924", "Anything"));
    assert!(gog.excludes("1", "The Witcher™"));
    assert!(!gog.excludes("1", "The Witcher 2"));
    let games = vec![epic_game(), mirage()];
    let mut apps = vec![];
    assert_eq!(
        reconcile(&mut apps, &games, &settings, &Source::ALL, &[]).added,
        0,
        "{apps:?}"
    );
    // Synced before they were left out: they leave, whatever the read; an import stays.
    let mut apps = vec![];
    reconcile(&mut apps, &games, &Settings::default(), &Source::ALL, &[]);
    let mut kept = mirage();
    kept.id = "1".into();
    kept.name = "Mirage".into();
    let mut imported = vec![];
    import(&mut imported, &[kept.clone()], &[selected(&kept)]);
    apps.extend(imported);
    assert_eq!(apps.len(), 3);
    let change = reconcile(&mut apps, &games, &settings, &[], &[]);
    assert_eq!(change.removed, 2);
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["launcher-id"], "1");
}

#[test]
fn a_launcher_with_a_play_history_can_keep_only_its_recent_games_as_steam_does() {
    const DAY: u64 = 86_400;
    let now = 1_800_000_000;
    let game = |uid: &str, played: Option<u64>| {
        let mut game = battlenet(
            uid,
            Path::new(&format!("C:/Games/{uid}")),
            Some(Path::new("C:/Battle.net/Battle.net.exe")),
        )
        .unwrap();
        game.last_played = played;
        game
    };
    let games = vec![
        game("wow", Some(now - DAY)),
        game("fen", Some(now - 2 * DAY)),
        game("odin", Some(now - 40 * DAY)),
        game("w3", None),
    ];
    let recent =
        sync_settings("battlenet_sync_all_installed = false\nbattlenet_recent_games = 1\n");
    let mut apps = vec![];
    let change = reconcile_at(&mut apps, &games, &recent, &Source::ALL, &[], now);
    assert_eq!(change.added, 1);
    assert_eq!(apps[0]["name"], "World of Warcraft");
    // Two, played within 30 days: Diablo IV joins; Modern Warfare is too old, Warcraft III never played.
    let two = sync_settings("battlenet_sync_all_installed = false\nbattlenet_recent_games = 2\n");
    assert_eq!(
        reconcile_at(&mut apps, &games, &two, &Source::ALL, &[], now).added,
        1
    );
    let any_age = sync_settings(
        "battlenet_sync_all_installed = false\nbattlenet_recent_games = 5\nbattlenet_recent_max_age_days = 0\n",
    );
    assert_eq!(chosen(&games, &any_age, now).len(), 3);
    // Fallen out of the list: the synced app leaves even with Remove uninstalled games off, as Steam's.
    let one_kept = sync_settings(
        "battlenet_sync_all_installed = false\nbattlenet_recent_games = 1\nbattlenet_autosync_remove_uninstalled = false\n",
    );
    let change = reconcile_at(&mut apps, &games, &one_kept, &Source::ALL, &[], now);
    assert_eq!(change.removed, 1);
    assert_eq!(apps.len(), 1);
    // None at all: every recent app leaves.
    let none = sync_settings("battlenet_sync_all_installed = false\nbattlenet_recent_games = 0\n");
    assert_eq!(
        reconcile_at(&mut apps, &games, &none, &Source::ALL, &[], now).removed,
        1
    );
    // Every installed game, the default: all four.
    assert_eq!(
        reconcile_at(
            &mut apps,
            &games,
            &Settings::default(),
            &Source::ALL,
            &[],
            now
        )
        .added,
        4
    );
}

#[test]
fn battlenet_play_history_and_epic_box_art_are_read_from_the_launchers_files() {
    let config = br#"{"Client": {}, "Games": {"battle_net": {"Resumable": "false"},
        "W3": {"LastPlayed": "1785312857"}, "odin": {"LastPlayed": 1767294556}, "fen": {"LastPlayed": "0"}}}"#;
    let mut with_bom = b"\xef\xbb\xbf".to_vec();
    with_bom.extend_from_slice(config);
    let played = battlenet_last_played(&with_bom).unwrap();
    assert_eq!(played.len(), 2, "{played:?}");
    assert_eq!(played["w3"], 1_785_312_857);
    assert_eq!(played["odin"], 1_767_294_556);
    assert!(battlenet_last_played(b"not json").is_err());
    // catcache.bin: base64 of the catalog items, as the launcher writes it.
    let items = json!([
        {"namespace": "namespace-1", "id": "item-2", "keyImages": [
            {"type": "DieselGameBox", "url": "https://cdn1.epicgames.com/wide.jpg"},
            {"type": "DieselGameBoxTall", "url": "https://cdn1.epicgames.com/item/tall_1200x1600.jpg"}]},
        {"namespace": "other", "id": "offer", "keyImages": [
            {"type": "OfferImageTall", "url": "https://cdn1.epicgames.com/offer.png"}]},
        {"namespace": "evil", "id": "1", "keyImages": [
            {"type": "DieselGameBoxTall", "url": "https://cdn1.epicgames.com.example.org/x.jpg"}]},
        {"namespace": "evil", "id": "2", "keyImages": [
            {"type": "DieselGameBoxTall", "url": "http://cdn1.epicgames.com/x.jpg"}]},
        {"namespace": "evil", "id": "3", "keyImages": [
            {"type": "DieselGameBoxTall", "url": "https://user@cdn1.epicgames.com/x.jpg"}]},
        {"namespace": "none", "id": "4"},
    ]);
    let encoded = base64_for_test(items.to_string().as_bytes());
    let covers = epic_covers(encoded.as_bytes()).unwrap();
    assert_eq!(covers.len(), 2, "{covers:?}");
    let game = epic_game();
    let key = epic_cover_key(&game.id).unwrap();
    assert_eq!(key, "namespace-1:item-2");
    assert_eq!(
        covers[&key],
        "https://cdn1.epicgames.com/item/tall_1200x1600.jpg"
    );
    assert_eq!(
        covers["other:offer"],
        "https://cdn1.epicgames.com/offer.png"
    );
    assert_eq!(epic_cover_key("no-app"), None);
    assert!(epic_covers(b"not base64!").is_err());
    // Every length decodes, with and without padding and line breaks.
    for length in 0..8 {
        let bytes: Vec<u8> = (0..length).map(|at| (at * 37 % 256) as u8).collect();
        let text = base64_for_test(&bytes);
        assert_eq!(base64(text.as_bytes()).unwrap(), bytes, "{text}");
        assert_eq!(
            base64(text.trim_end_matches('=').as_bytes()).unwrap(),
            bytes
        );
        let wrapped = format!("{}\r\n{}", &text[..text.len() / 2], &text[text.len() / 2..]);
        assert_eq!(base64(wrapped.as_bytes()).unwrap(), bytes);
    }
}
/// Standard base64 with padding, to write what the Epic launcher writes.
fn base64_for_test(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (at, byte)| {
            value | (u32::from(*byte) << (16 - 8 * at))
        });
        for at in 0..4 {
            if at <= chunk.len() {
                text.push(ALPHABET[((value >> (18 - 6 * at)) & 63) as usize] as char);
            } else {
                text.push('=');
            }
        }
    }
    text
}

#[test]
fn an_app_of_a_launchers_game_without_a_cover_gets_the_launchers_and_a_users_cover_stays() {
    let mut covered = mirage();
    covered.cover = Some(PathBuf::from(r"C:\Profile\covers\launcher_mirage.png"));
    let mut apps = vec![];
    // Synced before covers were read.
    reconcile(
        &mut apps,
        &[mirage()],
        &Settings::default(),
        &Source::ALL,
        &[],
    );
    assert!(apps[0].get("image-path").is_none());
    let mut imported_game = mirage();
    imported_game.id = "2".into();
    imported_game.name = "Imported".into();
    import(
        &mut apps,
        &[imported_game.clone()],
        &[selected(&imported_game)],
    );
    apps.push(json!({"name": "Mine", "launcher-source": "ubisoft", "launcher-id": "3", "image-path": "D:/mine.png"}));
    let mut mine = mirage();
    mine.id = "3".into();
    mine.name = "Mine".into();
    mine.cover = covered.cover.clone();
    let mut imported_cover = imported_game.clone();
    imported_cover.cover = covered.cover.clone();
    let change = reconcile(
        &mut apps,
        &[covered, imported_cover, mine],
        &Settings::default(),
        &Source::ALL,
        &[],
    );
    assert_eq!(change.covered, 2);
    assert_eq!(
        apps[0]["image-path"],
        "C:/Profile/covers/launcher_mirage.png"
    );
    assert_eq!(
        apps[1]["image-path"],
        "C:/Profile/covers/launcher_mirage.png"
    );
    assert_eq!(apps[2]["image-path"], "D:/mine.png");
    // Once: nothing more to fill.
    assert!(!reconcile(&mut apps, &[], &Settings::default(), &[], &[]).changed());
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
            ..SyncChange::default()
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
    // Not synced automatically: the automatic sync changes nothing; Sync now adds the games,
    // as Steam's does.
    let off = sync_settings("library_auto_sync = false\n");
    let mut none = vec![];
    assert!(
        !reconcile(&mut none, &games, &off.automatic(), &Source::ALL, &[]).changed()
            && none.is_empty()
    );
    assert_eq!(
        reconcile(
            &mut none,
            &games,
            &off,
            &Source::ALL,
            &["ubisoft:777".into()]
        )
        .added,
        2
    );
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
            removed: 1,
            ..SyncChange::default()
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

#[test]
fn the_ownership_stamp_runs_once_and_takes_steam_apps_only_after_the_old_opt_in() {
    let document = || {
        json!({"apps": [
            {"name": "Vibepollo's Portal", "steam-id": "620", "steam-managed": "auto"},
            {"name": "Mirage", "launcher-source": "ubisoft", "launcher-id": "13504", "launcher-managed": "auto"},
            {"name": "Kart", "playnite-id": "aa", "playnite-managed": "auto"},
            {"name": "Imported", "steam-id": "570", "steam-managed": "manual"},
            {"name": "Desktop"},
        ]})
    };
    let mut without = document();
    assert_eq!(
        stamp_synced(&mut without, false),
        Some(Stamped {
            steam: 0,
            launcher: 1,
            playnite: 1
        })
    );
    let stamps: Vec<&Value> = without["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|app| &app[SYNCED_BY])
        .collect();
    assert_eq!(
        stamps,
        [
            &Value::Null,
            &json!("launcher"),
            &json!("playnite"),
            &Value::Null,
            &Value::Null
        ]
    );
    assert_eq!(without[SYNC_OWNER_VERSION], 1);
    // Once: a later start stamps nothing, even an app that lost its stamp.
    let ran = without.clone();
    assert_eq!(stamp_synced(&mut without, true), None);
    assert_eq!(without, ran);
    let mut opted_in = document();
    assert_eq!(stamp_synced(&mut opted_in, true).unwrap().steam, 1);
    assert_eq!(opted_in["apps"][0][SYNCED_BY], "steam");
    assert_eq!(opted_in["apps"][3].get(SYNCED_BY), None);
}

#[test]
fn a_handed_over_app_loses_its_stamp_and_the_launcher_sync_leaves_it() {
    let settings = Settings::default();
    let games = vec![mirage()];
    let mut apps = vec![];
    reconcile(&mut apps, &games, &settings, &Source::ALL, &[]);
    assert_eq!(apps[0][SYNCED_BY], "launcher");
    hand_over(&mut apps[0], "launcher");
    assert_eq!(apps[0].get(SYNCED_BY), None);
    assert_eq!(apps[0][MANAGED], "manual");
    // Uninstalled: the user's app stays, and the game is not added a second time.
    assert_eq!(
        reconcile(&mut apps, &[], &settings, &Source::ALL, &[]),
        SyncChange::default()
    );
    assert_eq!(
        reconcile(&mut apps, &games, &settings, &Source::ALL, &[]),
        SyncChange::default()
    );
    assert_eq!(apps.len(), 1);
    // An unstamped `launcher-managed: auto` app is the user's as well.
    let mut theirs = vec![json!({"name": "Mirage", "launcher-source": "ubisoft",
        "launcher-id": "13504", "launcher-managed": "auto"})];
    assert!(!reconcile(&mut theirs, &[], &settings, &Source::ALL, &[]).changed());
    assert_eq!(theirs.len(), 1);
    assert_eq!(dismissal_keys(&theirs[0]), ["ubisoft:13504"]);
}

fn program(path: &str) -> Option<Launch> {
    Some(Launch::Program {
        program: path.into(),
        arguments: String::new(),
    })
}
/// Apps every kind of [`Game::matches`] finds, near misses and apps it never finds.
fn indexed_apps() -> Vec<Value> {
    vec![
        json!({"name": "Desktop", "cmd": ""}),
        json!({"name": "The Witcher 3", "steam-id": "292030"}),
        json!({"name": "ÉTOILE", "playnite-id": "p1", "playnite-plugin-name": "Epic Library"}),
        json!({"name": "Étoile", "playnite-id": "p2", "playnite-plugin-name": "GOG Library"}),
        json!({"name": "Mass Effect", "playnite-id": "p3", "playnite-plugin-name": "Origin"}),
        json!({"name": "Celeste (copy)", "working-dir": "d:/GAMES/celeste/"}),
        json!({"name": "Hades tools", "cmd": "\"D:\\Games\\Hades\\x64\\Hades.exe\" -dx12"}),
        json!({"name": "Climber", "working-dir": "D:\\Games\\..\\Hades"}),
        json!({"name": "Mods", "launcher-install-dir": "D:\\Games\\The Witcher 3 Mods"}),
        json!({"name": "Dota", "cmd": "steam://rungameid/570"}),
        json!({"name": "Mirage", "launcher-source": "ubisoft", "launcher-id": "13504"}),
        json!({"name": "Root", "working-dir": "D:\\"}),
        json!({"name": "Hades", "playnite-id": "p4", "working-dir": "E:\\Elsewhere"}),
        Value::Null,
        json!(["not", "an", "app"]),
        json!({"uuid": epic_game().uuid()}),
    ]
}
/// Games of every launcher, and games of no name or folder.
fn indexed_games() -> Vec<Game> {
    let mut games = vec![epic_game(), mirage()];
    games.extend(local_games());
    games.extend([
        Game::new(
            Source::Ea,
            "mass-effect".into(),
            "Mass Effect".into(),
            Path::new(r"C:\EA\Mass Effect"),
            program(r"C:\EA\Mass Effect\MassEffect.exe"),
        ),
        Game::new(
            Source::Ea,
            "broken".into(),
            "Broken".into(),
            Path::new(r"C:\EA\Broken"),
            None,
        ),
        Game::new(
            Source::Gog,
            "1".into(),
            "Étoile".into(),
            Path::new(r"C:\GOG\Etoile"),
            program(r"C:\GOG\Etoile\Etoile.exe"),
        ),
        Game::new(
            Source::Steam,
            "570".into(),
            "Dota 2".into(),
            Path::new(r"F:\Steam\steamapps\common\dota 2 beta"),
            None,
        ),
        Game::new(
            Source::Steam,
            "292030".into(),
            "The Witcher 3".into(),
            Path::new(r"F:\Steam\steamapps\common\The Witcher 3"),
            None,
        ),
        Game::new(
            Source::Local,
            r"d:\games".into(),
            "Games".into(),
            Path::new("D:/Games/"),
            program(r"D:\Games\games.exe"),
        ),
        Game::new(
            Source::Local,
            r"d:\".into(),
            "Root".into(),
            Path::new(r"D:\"),
            None,
        ),
        Game::new(
            Source::Epic,
            "nameless".into(),
            "!!!".into(),
            Path::new(""),
            program(r"C:\Epic\nameless.exe"),
        ),
    ]);
    games
}

#[test]
fn the_app_index_finds_what_trying_every_app_finds() {
    let apps = indexed_apps();
    let games = indexed_games();
    let (mut found, mut missed) = (0, 0);
    // Every list of apps from none to all, as the sync grows it.
    for end in 0..=apps.len() {
        let apps = &apps[..end];
        let index = AppIndex::new(apps);
        for game in &games {
            let expected = apps.iter().any(|app| game.matches(app));
            assert_eq!(
                index.matches(apps, game),
                expected,
                "{} among {end} apps",
                game.name
            );
            found += usize::from(expected);
            missed += usize::from(!expected);
        }
    }
    assert!(found > 0 && missed > 0, "{found} found, {missed} missed");
}

/// `reconcile` without the app index: every game tries every app.
fn reconcile_by_search(
    apps: &mut Vec<Value>,
    games: &[Game],
    settings: &Settings,
    read: &[Source],
    dismissed: &[String],
) -> SyncChange {
    let mut change = SyncChange::default();
    let wanted = chosen(games, settings, u64::MAX / 2);
    let before = apps.len();
    apps.retain(|app| {
        let Some(source) = app["launcher-source"]
            .as_str()
            .and_then(|key| Source::ALL.into_iter().find(|source| source.key() == key))
        else {
            return true;
        };
        let Some(options) = settings.options(source).filter(|options| options.enabled) else {
            return true;
        };
        let id = app["launcher-id"].as_str().unwrap_or("");
        if !synced_by(app, "launcher") {
            return true;
        }
        if options.excludes(id, app["name"].as_str().unwrap_or("")) {
            return false;
        }
        let installed = games
            .iter()
            .any(|game| game.source == source && game.id == id);
        !read.contains(&source)
            || (installed && (options.sync_all_installed || wanted.contains(&(source, id))))
            || (!installed && options.sync_all_installed && !options.remove_uninstalled)
    });
    change.removed = before - apps.len();
    for game in games {
        if !wanted.contains(&(game.source, game.id.as_str()))
            || dismissed.contains(&dismissed_key(game.source, &game.id))
            || apps.iter().any(|app| game.matches(app))
        {
            continue;
        }
        if let Ok(mut app) = game.app() {
            app[MANAGED] = json!("auto");
            app[SYNCED_BY] = json!("launcher");
            apps.push(app);
            change.added += 1;
        }
    }
    change
}

#[test]
fn the_launcher_sync_adds_and_removes_the_apps_it_did_in_the_same_order() {
    let mut apps = indexed_apps();
    // Synced apps whose games are gone, still installed, or of a launcher that is off.
    for (name, source, id) in [
        ("Gone", "epic", "gone"),
        ("Mirage", "ubisoft", "13504"),
        ("Old GOG", "gog", "old"),
        ("Left Local", "local", r"d:\games\left"),
    ] {
        let mut app = json!({"name": name, "launcher-source": source, "launcher-id": id});
        app[MANAGED] = json!("auto");
        app[SYNCED_BY] = json!("launcher");
        apps.insert(1, app);
    }
    // Hades is added before the `D:\Games` folder game, which then finds Hades' app.
    let mut games = local_games();
    games.extend(indexed_games());
    let all_settings = [
        Settings::default(),
        local_settings(),
        sync_settings(concat!(
            "library_autosync_remove_uninstalled = false\n",
            r#"library_local_folders = ["D:\\Games"]"#,
        )),
        sync_settings("library_auto_sync = false\n").automatic(),
        sync_settings(concat!(
            "battlenet_sync_all_installed = false\n",
            "epic_exclude_games = [{\"name\": \"Étoile\"}]\n",
            "ubisoft_exclude_games = [\"13504\"]\n",
        )),
    ];
    let reads: [&[Source]; 2] = [&Source::ALL, &[Source::Epic, Source::Local]];
    let dismissals = [
        vec![],
        vec![dismissed_key(Source::Local, r"d:\games\hades")],
    ];
    let (mut added, mut removed) = (0, 0);
    for settings in &all_settings {
        for read in reads {
            for dismissed in &dismissals {
                let (mut indexed, mut searched) = (apps.clone(), apps.clone());
                for pass in 0..2 {
                    let change = reconcile(&mut indexed, &games, settings, read, dismissed);
                    let expected =
                        reconcile_by_search(&mut searched, &games, settings, read, dismissed);
                    assert_eq!(change, expected, "pass {pass} with {settings:?}");
                    assert_eq!(indexed, searched, "pass {pass} with {settings:?}");
                    added += change.added;
                    removed += change.removed;
                }
            }
        }
    }
    assert!(added > 0 && removed > 0, "{added} added, {removed} removed");
}
