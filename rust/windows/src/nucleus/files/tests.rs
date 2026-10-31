use super::*;

const HANDLER: &str = r#"Game.GUID = "Borderlands 2";
Game.GameName = "Borderlands 2";
Game.ExecutableName = "Borderlands2.exe";
Game.MaxPlayers = 4;
"#;

fn player(xinput: u8) -> Player {
    Player {
        xinput,
        bounds: Rect {
            x: 4000 + i32::from(xinput) * 2000,
            y: 0,
            width: 1920,
            height: 1080,
        },
    }
}
const SCREEN: Rect = Rect {
    x: 0,
    y: 0,
    width: 2560,
    height: 1440,
};

#[test]
fn the_catalog_reads_handlers_and_only_the_list_nucleus_reads() {
    let nucleus = tempfile::tempdir().unwrap();
    let appdata = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
    std::fs::write(handlers.join("broken.js"), "Game.GameName = 1;").unwrap();
    std::fs::write(handlers.join("notes.txt"), HANDLER).unwrap();
    std::fs::create_dir_all(appdata.path().join("Nucleus Coop")).unwrap();
    std::fs::write(
        appdata.path().join("Nucleus Coop").join("userprofile.json"),
        r#"{"Games":[{"GameGuid":"Borderlands 2","ExePath":"D:\\Games\\BL2\\Borderlands2.exe"}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(nucleus.path().join("content")).unwrap();
    std::fs::write(
            nucleus.path().join("content").join("userprofile.json"),
            "\u{feff}{\"Games\":[{\"GameGuid\":\"Borderlands 2\",\"ExePath\":\"E:\\\\BL2\\\\Borderlands2.exe\"}]}",
        )
        .unwrap();
    let catalog = catalog(nucleus.path(), true, None).unwrap();
    assert_eq!(catalog.handlers.len(), 1);
    assert_eq!(catalog.handlers[0].executable, "Borderlands2.exe");
    // The AppData list's game is not one Nucleus Co-op shows.
    assert_eq!(catalog.games.len(), 1);
    let found = catalog
        .find("Borderlands 2", Some(r"E:\BL2\Borderlands2.exe"))
        .unwrap();
    assert_eq!(found.handler.guid, "Borderlands 2");
    // Another release at nucleus_path may read the AppData list: both
    // count.
    let other = super::catalog(nucleus.path(), true, Some(appdata.path())).unwrap();
    let exes: Vec<&str> = other.games.iter().map(|g| g.exe_path.as_str()).collect();
    assert_eq!(
        exes,
        [r"E:\BL2\Borderlands2.exe", r"D:\Games\BL2\Borderlands2.exe"]
    );

    // No list and no handlers: an empty catalog, not an error.
    let empty = tempfile::tempdir().unwrap();
    let catalog = super::catalog(empty.path(), false, None).unwrap();
    assert!(catalog.handlers.is_empty() && catalog.games.is_empty());

    // A game list Nucleus wrote but the host cannot read is an error.
    std::fs::create_dir_all(empty.path().join("content")).unwrap();
    std::fs::write(empty.path().join("content").join("userprofile.json"), "{").unwrap();
    let error = format!(
        "{:#}",
        super::catalog(empty.path(), false, None).unwrap_err()
    );
    assert!(error.contains("userprofile.json"), "{error}");
}

#[test]
fn nucleus_lists_the_game_rubylight_adds() {
    // #59: Nucleus Co-op 2.4.2 reads only content\userprofile.json, so a
    // game added to the AppData list was never shown, nothing was
    // selected, and the profile Rubylight never appeared.
    let nucleus = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let exe = game.path().join("AC4BFSP.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let nucleus_coop = roaming.map(|appdata| appdata.join("Nucleus Coop"));
    let there = nucleus_coop.as_ref().map(|folder| folder.exists());
    let setup = Setup::new(
        &exe.to_string_lossy(),
        "Assassin's Creed IV: Black Flag Resynced",
        None,
    );
    assert_eq!(
        listed_by_nucleus(nucleus.path()).unwrap(),
        Vec::<String>::new()
    );
    write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(listed_by_nucleus(nucleus.path()).unwrap(), [setup.guid()]);
    // Nothing went to the user's roaming folder.
    assert_eq!(nucleus_coop.as_ref().map(|folder| folder.exists()), there);

    // As Nucleus loads it: an entry without a handler, or whose .exe is
    // gone, is not shown.
    std::fs::remove_file(&exe).unwrap();
    assert_eq!(
        listed_by_nucleus(nucleus.path()).unwrap(),
        Vec::<String>::new()
    );
    std::fs::write(&exe, b"MZ").unwrap();
    std::fs::remove_file(nucleus.path().join("handlers").join(setup.file())).unwrap();
    assert_eq!(
        listed_by_nucleus(nucleus.path()).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn rubylights_games_go_from_the_appdata_list_and_nothing_else() {
    let appdata = tempfile::tempdir().unwrap();
    // No list: nothing to do.
    assert_eq!(forget_appdata_games(appdata.path()).unwrap(), 0);
    let path = appdata.path().join("Nucleus Coop").join("userprofile.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let users = "{\r\n  \"Games\": [\r\n    {\r\n      \"GameGuid\": \"Borderlands 2\",\r\n      \"ExePath\": \"D:\\\\BL2\\\\Borderlands2.exe\"\r\n    }\r\n  ]\r\n}";
    let mut text = users.to_string();
    for (exe, name) in [(r"D:\A\A.exe", "A"), (r"D:\B\B.exe", "B")] {
        let setup = Setup::new(exe, name, None);
        text = generic::add_game(Some(&text), &setup.guid(), exe)
            .unwrap()
            .unwrap();
    }
    std::fs::write(&path, &text).unwrap();
    assert_eq!(forget_appdata_games(appdata.path()).unwrap(), 2);
    // The user's game and the file's layout stay, byte for byte.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), users);
    assert_eq!(forget_appdata_games(appdata.path()).unwrap(), 0);
    // A list the host can't read is left as it is, loudly.
    std::fs::write(&path, "{").unwrap();
    let error = format!("{:#}", forget_appdata_games(appdata.path()).unwrap_err());
    assert!(error.contains("userprofile.json"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{");
}

/// The handler as written for the account running the tests.
fn ours(setup: &Setup) -> String {
    setup.handler_of(crate::process::user_sid().as_deref())
}

#[test]
fn rubylights_handler_is_written_and_its_game_added_beside_the_users() {
    let nucleus = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
    let list = list(nucleus.path());
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    let users = "{\r\n  \"Games\": [\r\n    {\r\n      \"GameGuid\": \"Borderlands 2\",\r\n      \"ExePath\": \"D:\\\\BL2\\\\Borderlands2.exe\"\r\n    }\r\n  ]\r\n}";
    std::fs::write(&list, users).unwrap();
    let setup = Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);

    let handler = write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(handler.guid, setup.guid());
    assert!(handler.own);
    let file = handlers.join(setup.file());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), ours(&setup));
    assert_eq!(
        std::fs::read_to_string(handlers.join("Borderlands 2.js")).unwrap(),
        HANDLER
    );
    let written = std::fs::read_to_string(&list).unwrap();
    // The user's game, byte for byte, then ours.
    let end = users.len() - "\r\n  ]\r\n}".len();
    assert!(written.starts_with(&users[..end]), "{written}");
    assert!(written[end..].starts_with(",\r\n    {"), "{written}");
    // Nucleus reads both, and co-op still has only the hub's handler.
    let catalog = catalog(nucleus.path(), true, None).unwrap();
    assert_eq!(catalog.games.len(), 2);
    assert!(catalog.handlers.iter().any(|h| h.own));
    assert!(catalog.find("Puzzle", Some(&setup.exe_path)).is_err());

    // Again: nothing changes. A changed template is rewritten.
    write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(std::fs::read_to_string(&list).unwrap(), written);
    std::fs::write(&file, format!("{}\r\n// v0", generic::MARKER)).unwrap();
    write_own(nucleus.path(), &setup).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), ours(&setup));
    assert_eq!(std::fs::read_to_string(&list).unwrap(), written);

    // A file with our name that someone else wrote stays as it is.
    std::fs::write(&file, HANDLER).unwrap();
    let error = write_own(nucleus.path(), &setup).unwrap_err().to_string();
    assert!(error.contains("is not Rubylight's handler"), "{error}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), HANDLER);

    // A game list the host cannot read is left alone, loudly.
    std::fs::write(&list, "{").unwrap();
    std::fs::remove_file(&file).unwrap();
    let error = format!("{:#}", write_own(nucleus.path(), &setup).unwrap_err());
    assert!(error.contains("not readable"), "{error}");
    assert_eq!(std::fs::read_to_string(&list).unwrap(), "{");

    // No game list yet: its folder and file are made.
    let fresh = tempfile::tempdir().unwrap();
    write_own(fresh.path(), &setup).unwrap();
    let games =
        handler::user_games(&std::fs::read_to_string(super::list(fresh.path())).unwrap()).unwrap();
    assert_eq!(games[0].guid, setup.guid());
}

/// #59: a game that refused the way ProtoInput's hooks were injected
/// gets a handler with the next way at its next start, down to none,
/// also after the host restarts (the file in the host's Nucleus folder
/// keeps it); other games keep the first.
#[test]
fn a_game_that_refused_the_hooks_gets_the_next_way_down_to_none() {
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Games\Black Flag\ACBlackFlag.exe", "Black Flag", None);
    let other = Setup::new(r"D:\Games\Other\Game.exe", "Other", None);
    let file = nucleus.path().join("handlers").join(setup.file());
    let kept = nucleus.path().join("Rubylight-injection.json");
    let written = |way: Inject| {
        write_own(nucleus.path(), &setup).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(generic::owns(&text));
        let wanted = Setup {
            inject: way,
            ..setup.clone()
        };
        let owner = crate::process::user_sid().unwrap();
        assert_eq!(text, wanted.handler_of(Some(&owner)), "{way:?}");
    };
    written(Inject::Runtime);
    assert_eq!(
        inject_for(nucleus.path(), &setup.exe_path).unwrap(),
        Inject::Runtime
    );
    for next in [Inject::Startup, Inject::Stealth, Inject::None] {
        // By its path, in any case.
        let exe = match next {
            Inject::Stealth => setup.exe_path.to_uppercase(),
            _ => setup.exe_path.clone(),
        };
        assert_eq!(refuse_inject(nucleus.path(), &exe).unwrap(), next);
        let text = std::fs::read_to_string(&kept).unwrap();
        assert_eq!(text.matches("acblackflag.exe").count(), 1, "{text}");
        assert_eq!(inject_for(nucleus.path(), &setup.exe_path).unwrap(), next);
        written(next);
    }
    // None is the last.
    assert_eq!(
        refuse_inject(nucleus.path(), &setup.exe_path).unwrap(),
        Inject::None
    );
    write_own(nucleus.path(), &other).unwrap();
    let other_text =
        std::fs::read_to_string(nucleus.path().join("handlers").join(other.file())).unwrap();
    assert!(other_text.contains("Game.ProtoInput.InjectRuntime_EasyHookMethod = true;"));
    // A game the test build before the ladder kept as refusing the
    // hooks starts at the second way; one kept since, at its own.
    let legacy = tempfile::tempdir().unwrap();
    std::fs::write(
        legacy.path().join("Rubylight-unhooked.json"),
        r#"{"Games": ["d:\\games\\black flag\\acblackflag.exe", "d:\\games\\other\\game.exe"]}"#,
    )
    .unwrap();
    assert_eq!(
        inject_for(legacy.path(), &setup.exe_path).unwrap(),
        Inject::Startup
    );
    assert_eq!(
        refuse_inject(legacy.path(), &other.exe_path).unwrap(),
        Inject::Stealth
    );
    assert_eq!(
        inject_for(legacy.path(), &other.exe_path).unwrap(),
        Inject::Stealth
    );
    assert_eq!(
        inject_for(legacy.path(), &setup.exe_path).unwrap(),
        Inject::Startup
    );
    // A file Rubylight can't read is loud, never taken for the first way.
    for broken in [
        "{",
        r#"{"Games": []}"#,
        r#"{"Games": {"d:\\a.exe": "sideways"}}"#,
    ] {
        std::fs::write(&kept, broken).unwrap();
        let error = format!("{:#}", write_own(nucleus.path(), &setup).unwrap_err());
        assert!(error.contains("delete it"), "{broken}: {error}");
    }
}

#[test]
fn ours_goes_once_the_user_adds_the_game_with_a_hub_handler() {
    let nucleus = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    let setup = Setup::new(r"D:\BL2\Borderlands2.exe", "Borderlands 2", None);
    write_own(nucleus.path(), &setup).unwrap();
    // The hub's handler comes, and the user adds the game with it.
    std::fs::write(handlers.join("Borderlands 2.js"), HANDLER).unwrap();
    let list = list(nucleus.path());
    let ours = std::fs::read_to_string(&list).unwrap();
    std::fs::write(
        &list,
        generic::add_game(Some(&ours), "Borderlands 2", &setup.exe_path)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let before = catalog(nucleus.path(), true, None).unwrap();
    let superseded = before.superseded(None);
    assert_eq!(superseded.len(), 1);
    remove_own(nucleus.path(), superseded[0]).unwrap();
    assert!(!handlers.join(setup.file()).exists());
    let after = catalog(nucleus.path(), true, None).unwrap();
    assert!(after.handlers.iter().all(|h| !h.own));
    assert_eq!(
        after
            .games
            .iter()
            .map(|g| g.guid.as_str())
            .collect::<Vec<_>>(),
        ["Borderlands 2"]
    );
    // A hub handler is never removed as ours.
    let error = remove_own(nucleus.path(), &after.handlers[0]).unwrap_err();
    assert!(
        error.to_string().contains("is not Rubylight's handler"),
        "{error}"
    );
    assert!(handlers.join("Borderlands 2.js").exists());
}

#[test]
fn a_handler_that_cannot_be_removed_keeps_its_game_and_is_found_again() {
    use std::os::windows::fs::OpenOptionsExt;
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    write_own(nucleus.path(), &setup).unwrap();
    let file = nucleus.path().join("handlers").join(setup.file());
    let ours = || {
        catalog(nucleus.path(), true, None)
            .unwrap()
            .handlers
            .into_iter()
            .find(|h| h.own)
    };
    let own = ours().unwrap();
    // Open elsewhere without sharing: the removal fails, loudly.
    let open = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let error = format!("{:#}", remove_own(nucleus.path(), &own).unwrap_err());
    assert!(error.contains(&setup.file()), "{error}");
    drop(open);
    // The game stays listed with it, so the next start finds both again.
    let left = catalog(nucleus.path(), true, None).unwrap();
    assert!(left.games.iter().any(|g| g.guid == setup.guid()));
    assert!(ours().is_some());
    remove_own(nucleus.path(), &own).unwrap();
    let after = catalog(nucleus.path(), true, None).unwrap();
    assert!(!file.exists());
    assert!(after.games.is_empty());
}

#[test]
fn an_unused_handler_of_ours_is_swept_and_a_foreign_one_never() {
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    write_own(nucleus.path(), &setup).unwrap();
    // A removal that took the game but not the handler, as before.
    let list = list(nucleus.path());
    let text = std::fs::read_to_string(&list).unwrap();
    std::fs::write(
        &list,
        generic::remove_game(&text, &setup.guid()).unwrap().unwrap(),
    )
    .unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::write(handlers.join("Other.js"), HANDLER).unwrap();
    let before = catalog(nucleus.path(), true, None).unwrap();
    let stale = before.stale(None);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].guid, setup.guid());
    remove_own(nucleus.path(), stale[0]).unwrap();
    assert!(!handlers.join(setup.file()).exists());
    assert!(handlers.join("Other.js").exists());
    let after = catalog(nucleus.path(), true, None).unwrap();
    assert!(after.stale(None).is_empty());
}

#[test]
fn ours_another_account_or_an_earlier_build_wrote_stays() {
    let nucleus = tempfile::tempdir().unwrap();
    let handlers = nucleus.path().join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    // Another account wrote one, which its sweep removes; an earlier
    // build wrote one without an owner.
    let theirs = Setup::new(r"D:\Theirs\Game.exe", "Theirs", None);
    std::fs::write(
        handlers.join(theirs.file()),
        theirs.handler_of(Some("S-1-5-21-9-9-9-1002")),
    )
    .unwrap();
    let earlier = Setup::new(r"D:\Earlier\Game.exe", "Earlier", None);
    std::fs::write(handlers.join(earlier.file()), earlier.handler()).unwrap();
    // This account's, whose entry is gone.
    let mine = Setup::new(r"D:\Mine\Game.exe", "Mine", None);
    write_own(nucleus.path(), &mine).unwrap();
    let list = list(nucleus.path());
    let text = std::fs::read_to_string(&list).unwrap();
    std::fs::write(
        &list,
        generic::remove_game(&text, &mine.guid()).unwrap().unwrap(),
    )
    .unwrap();
    let catalog = catalog(nucleus.path(), true, None).unwrap();
    assert_eq!(catalog.user, crate::process::user_sid());
    let stale: Vec<&str> = catalog
        .stale(None)
        .iter()
        .map(|h| h.guid.as_str())
        .collect();
    assert_eq!(stale, [mine.guid()]);
    // Without a signed-in user's list, none.
    let nobody = super::catalog(nucleus.path(), false, None).unwrap();
    assert!(nobody.user.is_none());
    assert!(nobody.stale(None).is_empty());
}

#[test]
fn a_game_entry_left_after_its_handler_went_is_taken_over_again() {
    use std::os::windows::fs::OpenOptionsExt;
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    let own = write_own(nucleus.path(), &setup).unwrap();
    let list = list(nucleus.path());
    let open = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&list)
        .unwrap();
    let error = format!("{:#}", remove_own(nucleus.path(), &own).unwrap_err());
    drop(open);
    // The warning says what stays and why it does no harm.
    assert!(error.contains("still lists its game"), "{error}");
    assert!(
        error.contains("leaves a game whose handler is gone out"),
        "{error}"
    );
    let left = catalog(nucleus.path(), true, None).unwrap();
    assert!(left.handlers.iter().all(|h| !h.own));
    assert!(left.stale(None).is_empty());
    assert!(left.playable().is_empty());
    // The game's next background stream writes the handler and keeps
    // the one entry.
    write_own(nucleus.path(), &setup).unwrap();
    let again = catalog(nucleus.path(), true, None).unwrap();
    assert_eq!(
        again
            .games
            .iter()
            .filter(|g| g.guid == setup.guid())
            .count(),
        1
    );
    assert!(again.handlers.iter().any(|h| h.own));
}

#[test]
fn a_game_list_the_host_cannot_edit_is_refused_before_anything_changes() {
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    // No list yet: a new one is written.
    check_list(nucleus.path(), Some(&setup), &[]).unwrap();
    let list = list(nucleus.path());
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    std::fs::write(
        &list,
        r#"{"Games":[{"GameGuid":"A","ExePath":"C:\\A.exe"}]}"#,
    )
    .unwrap();
    check_list(nucleus.path(), Some(&setup), &["Rubylight-x"]).unwrap();
    // A game list the catalog reads but no edit could keep whole.
    for broken in ["{", r#"{"Games":7}"#, "[1]"] {
        std::fs::write(&list, broken).unwrap();
        let error = format!(
            "{:#}",
            check_list(nucleus.path(), Some(&setup), &[]).unwrap_err()
        );
        assert!(error.contains("userprofile.json"), "{broken}: {error}");
    }
    std::fs::write(&list, r#"{"Games":[{"GameGuid":}]}"#).unwrap();
    let error = format!(
        "{:#}",
        check_list(nucleus.path(), None, &["Rubylight-x"]).unwrap_err()
    );
    assert!(error.contains("userprofile.json"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&list).unwrap(),
        r#"{"Games":[{"GameGuid":}]}"#
    );
}

#[test]
fn only_a_game_list_file_on_the_disk_of_sane_size_is_read_never_a_link() {
    let nucleus = tempfile::tempdir().unwrap();
    let appdata = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    let list = list(nucleus.path());
    let shown = list.display().to_string();
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    let theirs = r#"{"Games":[{"GameGuid":"A","ExePath":"C:\\A.exe"}]}"#;
    // A list as Nucleus writes it still reads.
    std::fs::write(&list, theirs).unwrap();
    let games = catalog(nucleus.path(), true, None).unwrap().games;
    assert_eq!(games.len(), 1);
    check_list(nucleus.path(), Some(&setup), &[]).unwrap();
    // Every read of the list refuses it, naming it, and nothing is written.
    let refused = |why: &str| {
        let errors = [
            catalog(nucleus.path(), true, None).map(|_| ()),
            check_list(nucleus.path(), Some(&setup), &[]),
            write_own(nucleus.path(), &setup).map(|_| ()),
            remove_game(&list, "A"),
        ];
        for error in errors {
            let error = format!("{:#}", error.unwrap_err());
            assert!(error.contains(&shown) && error.contains(why), "{error}");
        }
    };

    // Larger than any list: never read whole.
    let big = std::fs::File::create(&list).unwrap();
    big.set_len(LIST_CAP + 1).unwrap();
    drop(big);
    refused("larger than 16 MB");
    assert_eq!(std::fs::metadata(&list).unwrap().len(), LIST_CAP + 1);
    // A folder where the list should be.
    std::fs::remove_file(&list).unwrap();
    std::fs::create_dir(&list).unwrap();
    refused("is a folder, a link or a device");
    assert!(list.is_dir());
    std::fs::remove_dir(&list).unwrap();
    // A junction to a folder elsewhere.
    let elsewhere = appdata.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let junction = |link: &Path| {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(&elsewhere)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
    };
    junction(&list);
    refused("is a folder, a link or a device");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    std::fs::remove_dir(&list).unwrap();
    // A link to a file elsewhere, which takes Developer Mode or
    // administrator rights to make.
    let target = appdata.path().join("target.json");
    std::fs::write(&target, theirs).unwrap();
    match std::os::windows::fs::symlink_file(&target, &list) {
        Ok(()) => {
            refused("is a folder, a link or a device");
            assert_eq!(std::fs::read_to_string(&target).unwrap(), theirs);
            std::fs::remove_file(&list).unwrap();
        }
        Err(error) => eprintln!(
            "the symbolic link case is skipped: making one needs Developer Mode or administrator rights ({error})"
        ),
    }

    // The list is replaced through neither a link at its name nor one at
    // the temporary file's.
    std::fs::write(&list, theirs).unwrap();
    let temporary = list.with_extension("json.rubylight");
    junction(&temporary);
    let error = format!("{:#}", write_own(nucleus.path(), &setup).unwrap_err());
    assert!(
        error.contains(&temporary.display().to_string()) && error.contains("is a link"),
        "{error}"
    );
    assert_eq!(std::fs::read_to_string(&list).unwrap(), theirs);
    std::fs::remove_dir(&temporary).unwrap();
    std::fs::remove_file(&list).unwrap();
    junction(&list);
    let error = format!("{:#}", replace(&list, theirs).unwrap_err());
    assert!(
        error.contains(&shown) && error.contains("is a link"),
        "{error}"
    );
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    std::fs::remove_dir(&list).unwrap();
    // And with none, it is.
    write_own(nucleus.path(), &setup).unwrap();
    let games = catalog(nucleus.path(), true, None).unwrap().games;
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].guid, setup.guid());
}

fn ini(folder: &Path, section: &str, key: &str) -> Option<Vec<u8>> {
    let text = std::fs::read(folder.join("Settings.ini")).unwrap();
    profile::ini_value(&text, section, key)
}

/// A user's file in the PC's code page (Nucleus writes it through
/// kernel32's ANSI calls: ü is 0xFC), with the filters that hid the
/// game (#59).
const USERS: &[u8] = b"\xef\xbb\xbf[Misc]\r\nSettingsLocation=C:\\M\xfcnchen\r\nGameSortingOpt=Sort_2,Show_Type2\r\n[Dev]\r\nShowFavoriteOnly=True\r\nUseXinputIndex=False\r\n";

#[test]
fn settings_are_completed_forced_and_the_users_put_back_after_the_session() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    let path = folder.join("Settings.ini");
    std::fs::write(&path, USERS).unwrap();
    let mut guard = write_settings(folder).unwrap();
    let text = std::fs::read(&path).unwrap();
    // Edited as bytes: the BOM, the code page's ü and CRLF stay.
    assert!(
        text.starts_with(b"\xef\xbb\xbf[Misc]\r\nSettingsLocation=C:\\M\xfcnchen\r\n"),
        "{}",
        String::from_utf8_lossy(&text)
    );
    assert_eq!(ini(folder, "Dev", "UseXinputIndex").unwrap(), b"True");
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"True");
    assert_eq!(ini(folder, "Misc", "GameSortingOpt").unwrap(), b"Sort_2");
    // Completed from Nucleus' defaults.
    assert_eq!(ini(folder, "Dev", "Blur").unwrap(), b"4");
    assert!(settings_journal(folder).is_file());
    // Nucleus changed the list's order meanwhile: that stays.
    let changed = profile::ini_set(
        &std::fs::read(&path).unwrap(),
        &[("Misc", "GameSortingOpt", "Sort_3")],
    );
    std::fs::write(&path, changed).unwrap();
    guard.restore();
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"False");
    assert_eq!(ini(folder, "Misc", "GameSortingOpt").unwrap(), b"Sort_3");
    // Pads by index and the completed keys stay.
    assert_eq!(ini(folder, "Dev", "UseXinputIndex").unwrap(), b"True");
    assert_eq!(ini(folder, "Dev", "Blur").unwrap(), b"4");
    assert_eq!(
        ini(folder, "Misc", "SettingsLocation").unwrap(),
        b"C:\\M\xfcnchen"
    );
    assert!(!settings_journal(folder).exists());
    // Once only.
    std::fs::write(&path, b"[Dev]\r\nShowFavoriteOnly=False\r\n").unwrap();
    drop(guard);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
}

#[test]
fn a_missing_or_partial_settings_file_is_completed_from_nucleus_defaults() {
    // The host wrote the two-key file where none was; Nucleus then
    // stopped at its start on the first key missing (#59).
    for old in [
        None,
        Some(&b"[Dev]\r\nUseXinputIndex=True\r\n[Misc]\r\nDisableGameProfiles=False\r\n"[..]),
    ] {
        let nucleus = tempfile::tempdir().unwrap();
        let folder = nucleus.path();
        if let Some(old) = old {
            std::fs::write(folder.join("Settings.ini"), old).unwrap();
        }
        drop(write_settings(folder).unwrap());
        let text = std::fs::read(folder.join("Settings.ini")).unwrap();
        for (section, key) in [
            ("Hotkeys", "Close"),
            ("Dev", "ShowFavoriteOnly"),
            ("Dev", "ProfileAssignGamepadByButonPress"),
            ("Misc", "VGMOnly"),
            ("CustomLayout", "MaxPlayers"),
        ] {
            assert!(
                profile::ini_value(&text, section, key).is_some_and(|v| !v.is_empty()),
                "{section}.{key}"
            );
        }
        assert_eq!(ini(folder, "Dev", "UseXinputIndex").unwrap(), b"True");
        // The completed DisablePathCheck=False came back after the session.
        assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"False");
        assert!(!settings_journal(folder).exists());
    }
}

/// A host that stopped mid-session: its guard never ran, and Windows
/// closed its lock.
fn stopped(mut guard: SettingsGuard) {
    guard._lock = None;
    std::mem::forget(guard);
}

#[test]
fn what_a_session_the_host_never_ended_forced_is_put_back_first() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    std::fs::write(folder.join("Settings.ini"), USERS).unwrap();
    stopped(write_settings(folder).unwrap());
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    // The next session keeps the user's value, not the last session's.
    stopped(write_settings(folder).unwrap());
    let journal: Vec<profile::Forced> =
        serde_json::from_slice(&std::fs::read(settings_journal(folder)).unwrap()).unwrap();
    let favourites = journal
        .iter()
        .find(|f| f.key == "ShowFavoriteOnly")
        .unwrap();
    assert_eq!(favourites.before, b"True");
    // As the host starts.
    assert_eq!(restore_settings_left(folder).unwrap(), Some(3));
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
    assert!(!settings_journal(folder).exists());
    assert_eq!(restore_settings_left(folder).unwrap(), Some(0));
    // An empty journal goes; one that can't be read stops the session
    // and names itself.
    std::fs::write(settings_journal(folder), b"").unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), Some(0));
    assert!(!settings_journal(folder).exists());
    std::fs::write(settings_journal(folder), b"{").unwrap();
    let error = format!("{:#}", write_settings(folder).err().unwrap());
    assert!(
        error.contains("Rubylight-settings.json can't be read") && error.contains("delete it"),
        "{error}"
    );
}

#[test]
fn a_journal_left_beside_no_settings_file_goes_and_the_file_is_written_whole() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    let path = folder.join("Settings.ini");
    std::fs::write(&path, USERS).unwrap();
    stopped(write_settings(folder).unwrap());
    // The user deleted the file meanwhile.
    std::fs::remove_file(&path).unwrap();
    let guard = write_settings(folder).unwrap();
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"True");
    assert!(ini(folder, "Hotkeys", "Close").is_some_and(|v| !v.is_empty()));
    // The journal is the new session's, put back when it ends.
    drop(guard);
    assert!(!settings_journal(folder).exists());
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"False");
    // Nor does the service fail on it.
    stopped(write_settings(folder).unwrap());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), Some(0));
    assert!(!settings_journal(folder).exists());
}

#[test]
fn the_service_puts_nothing_back_under_a_session_or_another_restore() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    std::fs::write(folder.join("Settings.ini"), USERS).unwrap();
    // A session starting or running: the user's values wait for it.
    let guard = write_settings(folder).unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), None);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    assert!(settings_journal(folder).is_file());
    drop(guard);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
    // A restore elsewhere holds it.
    stopped(write_settings(folder).unwrap());
    let other = Lock::try_take(&settings_journal(folder)).unwrap().unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), None);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    drop(other);
    assert_eq!(restore_settings_left(folder).unwrap(), Some(3));
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
}

#[test]
fn a_settings_file_saved_as_utf16_is_refused_and_left_as_it_is() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    let path = folder.join("Settings.ini");
    let utf16 = |bom: [u8; 2], big: bool| {
        let mut bytes = bom.to_vec();
        for unit in "[Dev]\r\nShowFavoriteOnly=True\r\n".encode_utf16() {
            let pair = if big {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            };
            bytes.extend_from_slice(&pair);
        }
        bytes
    };
    for text in [utf16([0xff, 0xfe], false), utf16([0xfe, 0xff], true)] {
        std::fs::write(&path, &text).unwrap();
        let error = format!("{:#}", write_settings(folder).err().unwrap());
        assert!(
            error.contains("Settings.ini") && error.contains("UTF-16"),
            "{error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), text);
        assert!(!settings_journal(folder).exists());
    }
    // Saved as UTF-16 while a session ran: its journal stays.
    std::fs::write(&path, USERS).unwrap();
    stopped(write_settings(folder).unwrap());
    let text = utf16([0xff, 0xfe], false);
    std::fs::write(&path, &text).unwrap();
    let error = format!("{:#}", restore_settings_left(folder).unwrap_err());
    assert!(error.contains("UTF-16"), "{error}");
    assert!(settings_journal(folder).is_file());
    assert_eq!(std::fs::read(&path).unwrap(), text);
}

#[test]
fn the_profile_takes_its_own_number_or_the_next() {
    let nucleus = tempfile::tempdir().unwrap();
    let profiles = nucleus.path().join("game profiles").join("Borderlands 2");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(profiles.join("Profile[1].json"), r#"{"Title":"Mine"}"#).unwrap();
    std::fs::write(profiles.join("Profile[2].json"), r#"{"Title":""}"#).unwrap();
    let screens = [SCREEN];
    let number = write_profile(
        nucleus.path(),
        "Borderlands 2",
        &[player(0), player(1)],
        &screens,
    )
    .unwrap();
    assert_eq!(number, 3);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profiles.join("Profile[3].json")).unwrap()).unwrap();
    assert_eq!(written["Title"], profile::TITLE);
    assert_eq!(written["Player(s)"], 2);
    assert_eq!(written["Data"][1]["GamepadGuid"], profile::gamepad_guid(1));
    assert_eq!(written["Data"][1]["MonitorBounds"]["X"], 6000);
    // The user's profiles are left as they were.
    assert_eq!(
        std::fs::read_to_string(profiles.join("Profile[1].json")).unwrap(),
        r#"{"Title":"Mine"}"#
    );

    // Next time the same file is rewritten, for one player.
    let again = write_profile(nucleus.path(), "Borderlands 2", &[player(2)], &screens).unwrap();
    assert_eq!(again, 3);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profiles.join("Profile[3].json")).unwrap()).unwrap();
    assert_eq!(written["Player(s)"], 1);
    assert_eq!(written["Data"][0]["GamepadGuid"], profile::gamepad_guid(2));
    assert_eq!(std::fs::read_dir(&profiles).unwrap().count(), 3);
}

#[test]
fn a_foreign_profile_in_the_next_place_is_never_overwritten() {
    let nucleus = tempfile::tempdir().unwrap();
    let profiles = nucleus.path().join("game profiles").join("game");
    std::fs::create_dir_all(&profiles).unwrap();
    // Numbers 1 and 3: the list has two, so a new profile would be the 3rd.
    std::fs::write(profiles.join("Profile[1].json"), r#"{"Title":"A"}"#).unwrap();
    std::fs::write(profiles.join("Profile[3].json"), r#"{"Title":"B"}"#).unwrap();
    let error = write_profile(nucleus.path(), "game", &[player(0)], &[SCREEN])
        .unwrap_err()
        .to_string();
    assert!(error.contains("Profile[3].json"), "{error}");
    assert_eq!(
        std::fs::read_to_string(profiles.join("Profile[3].json")).unwrap(),
        r#"{"Title":"B"}"#
    );

    // Nucleus lists every file of the folder, so a stray one shifts the
    // places: a new profile there would never load and is taken back.
    let stray = nucleus.path().join("game profiles").join("stray");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join("Profile[1].json"), r#"{"Title":"A"}"#).unwrap();
    std::fs::write(stray.join("Profile[1].json.bak"), r#"{"Title":"A"}"#).unwrap();
    let error = write_profile(nucleus.path(), "stray", &[player(0)], &[SCREEN])
        .unwrap_err()
        .to_string();
    assert!(error.contains("would not load"), "{error}");
    assert!(
        error.contains("Profile[1].json, Profile[3].json, Profile[1].json.bak"),
        "{error}"
    );
    assert!(!stray.join("Profile[3].json").exists());
}

#[test]
fn a_guid_that_names_another_folder_is_refused() {
    let nucleus = tempfile::tempdir().unwrap();
    for guid in ["..", r"..\..\x", "a/b", "C:x", " ", "a\nb"] {
        let error = write_profile(nucleus.path(), guid, &[player(0)], &[SCREEN])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot name a profile folder"),
            "{guid:?}: {error}"
        );
    }
    assert!(!nucleus.path().join("game profiles").exists());
}
