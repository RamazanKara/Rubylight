#[test]
fn background_mode_answers_and_ends_also_on_a_panic() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
        THREAD_MODE_BACKGROUND_END,
    };
    // Beginning fails while the thread is in the mode: it tells whether the mode ended.
    let outside = || {
        // SAFETY: the current thread's pseudo-handle, no pointers.
        let began =
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }.is_ok();
        if began {
            // SAFETY: as above.
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END) }.unwrap();
        }
        began
    };
    assert!(outside());
    assert_eq!(super::in_background(|| super::in_background(|| 7)), 7);
    assert!(
        outside(),
        "a nested call leaves the mode to the outer one, which ends it"
    );
    assert!(std::panic::catch_unwind(|| super::in_background(|| panic!("boom"))).is_err());
    assert!(outside(), "the mode ended with the panic");
}

use super::*;

#[test]
fn only_a_store_application_id_launches_through_the_shell() {
    let try_launch = |uri: &str| {
        launch(
            &Launch::Uri { uri: uri.into() },
            None,
            false,
            &Default::default(),
        )
    };
    for uri in [
        r"shell:Startup",
        r"shell:AppsFolder\..\..\Windows\System32\cmd.exe",
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game & calc",
    ] {
        let error = try_launch(uri).unwrap_err().to_string();
        assert!(error.contains("Microsoft Store or Xbox"), "{uri}: {error}");
    }
}

#[test]
fn file_discovery_reports_bad_records_and_ignores_missing_locations() {
    let temp = tempfile::tempdir().unwrap();
    let mut errors = vec![];
    assert!(files(&temp.path().join("missing"), &mut errors).is_empty());
    assert!(errors.is_empty());
    let bad = temp.path().join("bad.item");
    std::fs::write(&bad, b"{broken").unwrap();
    let mut games = vec![];
    for file in files(temp.path(), &mut errors) {
        add(
            bytes(&file).and_then(|data| library::epic(&data)),
            &mut games,
            &mut errors,
        );
    }
    assert!(games.is_empty());
    assert_eq!(errors.len(), 1);
    let oversized = temp.path().join("large.item");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        bytes(&oversized)
            .unwrap_err()
            .to_string()
            .contains("too large")
    );
}

#[test]
fn game_folders_are_read_two_folders_deep_and_a_missing_one_hides_no_other() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Spiele");
    let write = |relative: &str, size: u64| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(path).unwrap().set_len(size).unwrap();
    };
    write("Hades/Hades.exe", 2048);
    write("Hades/unins000.exe", 4096);
    write("Hades/cover.jpg", 10);
    write("Witcher/bin/x64/witcher3.exe", 4096);
    write("Witcher/_CommonRedist/game.exe", 8192);
    write("Deep/a/b/c/deep.exe", 4096);
    write("Leave Out/game.exe", 4096);
    write("loose.exe", 4096);
    write("$RECYCLE.BIN/S-1-5-21/$RABC123.exe", 4096);
    let (mut programs_found, cut_short) = programs(&root.join("Witcher"), &mut vec![]);
    assert!(!cut_short);
    programs_found.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        programs_found
            .iter()
            .map(|program| (program.path.clone(), program.size))
            .collect::<Vec<_>>(),
        [(PathBuf::from(r"bin\x64\witcher3.exe"), 4096)],
        "redistributable folders are not read"
    );
    let settings = library::Settings {
        local_folders: vec![temp.path().join("missing"), root.clone()],
        local_exclude: vec!["leave out".into()],
        ..Default::default()
    };
    let (mut games, mut errors) = (vec![], vec![]);
    local(&settings, &mut games, &mut errors);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("missing") && errors[0].contains("does not exist"));
    let found: Vec<_> = library::finish(games)
        .into_iter()
        .map(|game| (game.name, game.launch.unwrap().command(), game.cover))
        .collect();
    assert_eq!(
        found,
        [
            (
                "Hades".to_owned(),
                format!("\"{}\"", root.join(r"Hades\Hades.exe").display()),
                Some(root.join("Hades").join("cover.jpg"))
            ),
            (
                "Witcher".to_owned(),
                format!(
                    "\"{}\"",
                    root.join(r"Witcher\bin\x64\witcher3.exe").display()
                ),
                None
            ),
        ],
        "the recycle bin is not a game"
    );
}

#[test]
fn a_game_folder_too_big_to_read_whole_is_read_from_the_top_and_says_so() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for game in ["Big", "Huge"] {
        let data = root.join(game).join("zdata");
        std::fs::create_dir_all(&data).unwrap();
        for file in 0..=GAME_FOLDER_ENTRIES {
            std::fs::File::create(data.join(format!("{file}.pak"))).unwrap();
        }
    }
    std::fs::create_dir_all(root.join(r"Big\a")).unwrap();
    std::fs::File::create(root.join(r"Big\a\Big.exe")).unwrap();
    let settings = library::Settings {
        local_folders: vec![root.to_owned()],
        ..Default::default()
    };
    let (mut games, mut errors) = (vec![], vec![]);
    local(&settings, &mut games, &mut errors);
    assert_eq!(
        games
            .iter()
            .map(|game| game.name.as_str())
            .collect::<Vec<_>>(),
        ["Big"],
        "a program near the top is found before the limit"
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("Huge"), "{errors:?}");
    // A drive the host cannot see is told apart from a missing folder.
    let free = (b'D'..=b'Z')
        .rev()
        .map(|letter| PathBuf::from(format!(r"{}:\Games", letter as char)))
        .find(|path| !path.ancestors().last().unwrap().exists())
        .unwrap();
    let unseen = unreadable_root(&free).unwrap();
    assert!(unseen.contains("network path"), "{unseen}");
    assert!(
        unreadable_root(Path::new("Games"))
            .unwrap()
            .contains("full path")
    );
}

#[test]
fn a_gog_registry_record_is_a_game_only_when_its_folder_has_no_info_file() {
    let temp = tempfile::tempdir().unwrap();
    let record = |id: &str, folder: &str| {
        let path = temp.path().join(folder);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::File::create(path.join("game.exe")).unwrap();
        BTreeMap::from(
            [
                ("gameID", id.to_owned()),
                ("gameName", format!("Game {id}")),
                ("path", path.display().to_string()),
                ("exe", path.join("game.exe").display().to_string()),
            ]
            .map(|(key, value)| (key.to_owned(), value)),
        )
    };
    let registered = [
        record("1", "Edition"),
        record("2", "Offline"),
        record("3", "Listed"),
    ];
    // The edition's folder has the base game's info file, under another id.
    std::fs::File::create(temp.path().join(r"Edition\goggame-9.info")).unwrap();
    let mut games = vec![Game::new(
        Source::Gog,
        "3".into(),
        "Listed".into(),
        &temp.path().join("Listed"),
        None,
    )];
    let mut errors = vec![];
    gog_registered(&registered, &mut games, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        games
            .iter()
            .map(|game| (game.id.as_str(), game.name.as_str()))
            .collect::<Vec<_>>(),
        [("3", "Listed"), ("2", "Game 2")]
    );
}

#[test]
fn installed_records_keep_unicode_paths_and_local_artwork() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("Étoile Игры");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("cover.png"), b"local artwork").unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../core/src/library/fixtures/epic.item"
    ))
    .unwrap();
    record["InstallLocation"] = serde_json::json!(folder);
    let mut games = vec![];
    let mut errors = vec![];
    add(
        library::epic(&serde_json::to_vec(&record).unwrap()),
        &mut games,
        &mut errors,
    );
    assert!(errors.is_empty());
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].install_dir, folder.to_string_lossy());
    assert_eq!(games[0].cover, Some(folder.join("cover.png")));
    record["InstallLocation"] = serde_json::json!(temp.path().join("uninstalled"));
    add(
        library::epic(&serde_json::to_vec(&record).unwrap()),
        &mut games,
        &mut errors,
    );
    assert_eq!(games.len(), 1);
}
