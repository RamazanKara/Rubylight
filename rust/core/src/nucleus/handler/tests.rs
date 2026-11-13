use super::*;

/// The account at the console in these tests.
const USER: &str = "S-1-5-21-1-2-3-1001";

const HANDLER: &str = r#"
// A test handler in the hub's style.
Game.ExecutableName = "Racer.exe";
Game.GUID = "Racer Deluxe";
Game.GameName = "Racer \"Deluxe\"";
Game.MaxPlayers = 4; // split four ways
Game.SupportsKeyboard = true;
Game.HideTaskbar = false;
/* Game.HideTaskbar = true; in a block comment
   Game.MaxPlayers = 8; */
Game.ProtoInput.XinputHook = true;
Game.Play = function() {
    Game.MaxPlayers = 16;
    var text = "}";
};
Game.SupportsKeyboard = true; // { not a block
"#;

#[test]
fn a_handler_is_read_without_running_it() {
    let h = parse("Racer.js", HANDLER).unwrap();
    assert_eq!(h.guid, "Racer Deluxe");
    assert_eq!(h.name, "Racer \"Deluxe\"");
    assert_eq!(h.executable, "Racer.exe");
    // The assignment inside a function and those in comments do not count.
    assert_eq!(h.max_players, 4);
    assert!(h.supports_keyboard);
    assert!(!h.hides_taskbar);
}

#[test]
fn a_handler_with_steps_of_its_own_is_known() {
    assert!(!parse("Racer.js", HANDLER).unwrap().has_steps);
    for steps in [
        "Game.ShowOptionAsStep(\"Mode\", true, \"Pick one\");",
        "Game.CustomSteps = [];",
    ] {
        let text = format!("{HANDLER}\n{steps}");
        assert!(parse("Racer.js", &text).unwrap().has_steps, "{steps}");
    }
}

#[test]
fn a_handler_needs_administrator_rights_for_links_users_and_elevated_tools() {
    let with = |body: &str| {
        parse(
            "x.js",
            &format!("Game.GUID='A'\nGame.ExecutableName='a.exe'\n{body}"),
        )
        .unwrap()
    };
    assert!(!needs_admin(&with("")));
    // Copies alone, and settings turned off again, need no rights.
    assert!(!needs_admin(&with(
        "Game.HardcopyGame = true;\nGame.SymlinkGame = true;\nGame.SymlinkGame = false;"
    )));
    for flag in ADMIN_FLAGS {
        let h = with(&format!("Game.{flag} = true;"));
        assert_eq!(h.admin, [flag], "{flag}");
        assert!(needs_admin(&h));
        // A value the script works out may be true.
        assert!(
            needs_admin(&with(&format!("Game.{flag} = enabled;"))),
            "{flag}"
        );
    }
    // Lists and texts count by their assignment.
    let h = with(
        "Game.SymlinkFiles = [\"a.dll\",\n  \"b.dll\"];\nGame.XInputPlusDll = [ \"xinput1_3.dll\" ];\nGame.FlawlessWidescreen = \"Fix\";",
    );
    assert_eq!(h.admin, ADMIN_VALUES);
    // Inside a function, in a comment, or only compared: not set.
    let h = with(
        "Game.Play = function() {\n    Game.RequiresAdmin = true;\n};\n// Game.UseGoldberg = true;\nif (Game.UseX360ce == true) {}",
    );
    assert!(!needs_admin(&h), "{:?}", h.admin);
    // A setting set to a variable leaves the others as they were.
    assert_eq!(
        with("Game.MaxPlayers = 4;\nGame.MaxPlayers = players;").max_players,
        4
    );
}

#[test]
fn a_handler_without_its_keys_is_named_in_the_error() {
    assert_eq!(
        parse("x.js", "Game.ExecutableName = 'a.exe';").unwrap_err(),
        "x.js sets no Game.GUID"
    );
    assert_eq!(
        parse("x.js", "Game.GUID = 'A';").unwrap_err(),
        "x.js sets no Game.ExecutableName"
    );
    let h = parse(
        "x.js",
        "Game.GUID='A'\nGame.ExecutableName='a.exe'\nGame.HideTaskbar = true",
    )
    .unwrap();
    assert_eq!(
        (h.name.as_str(), h.max_players, h.hides_taskbar),
        ("A", 2, true)
    );
}

#[test]
fn the_users_games_come_from_userprofile_json() {
    let json = "\u{feff}{\"Games\":[{\"GameGuid\":\"Racer Deluxe\",\"ExePath\":\"D:\\\\Games\\\\Racer\\\\Racer.exe\"},{\"GameGuid\":7}],\"Other\":1}";
    assert_eq!(
        user_games(json).unwrap(),
        vec![Game {
            guid: "Racer Deluxe".into(),
            exe_path: "D:\\Games\\Racer\\Racer.exe".into()
        }]
    );
    assert!(user_games("{}").unwrap().is_empty());
    assert!(
        user_games("not json")
            .unwrap_err()
            .contains("userprofile.json")
    );
}

fn catalog() -> Catalog {
    Catalog {
        handlers: vec![
            parse("Racer.js", HANDLER).unwrap(),
            parse(
                "Shooter.js",
                "Game.GUID='Shooter'\nGame.ExecutableName='Shooter.exe'",
            )
            .unwrap(),
            parse(
                "Twin.js",
                "Game.GUID='Twin'\nGame.ExecutableName='Twin.exe'",
            )
            .unwrap(),
        ],
        games: vec![
            Game {
                guid: "Racer Deluxe".into(),
                exe_path: "D:\\Games\\Racer\\Racer.exe".into(),
            },
            Game {
                guid: "Twin".into(),
                exe_path: "C:\\A\\Twin.exe".into(),
            },
            Game {
                guid: "Twin".into(),
                exe_path: "C:\\B\\Twin.exe".into(),
            },
            Game {
                guid: "Deleted".into(),
                exe_path: "C:\\Old\\Old.exe".into(),
            },
        ],
        user: Some(USER.into()),
    }
}

#[test]
fn an_app_finds_its_game_by_path_or_by_its_one_file_name() {
    let c = catalog();
    let by_path = c.find("Racer", Some("d:/games/racer/RACER.EXE")).unwrap();
    assert_eq!(by_path.handler.guid, "Racer Deluxe");
    assert_eq!(by_path.exe_path, "D:\\Games\\Racer\\Racer.exe");
    // Another copy of the game: the one added game with that name.
    let by_name = c.find("Racer", Some("E:\\Copy\\racer.exe")).unwrap();
    assert_eq!(by_name.exe_path, "D:\\Games\\Racer\\Racer.exe");
    // Two copies added: only the exact path chooses.
    assert_eq!(
        c.find("Twin", Some("C:\\B\\Twin.exe")).unwrap().exe_path,
        "C:\\B\\Twin.exe"
    );
    assert!(matches!(
        c.find("Twin", Some("E:\\Twin.exe")),
        Err(Missing::NotAdded { .. })
    ));
}

#[test]
fn only_the_exact_path_is_the_users_added_game() {
    // Portal's hl2.exe found Half-Life 2's handler by its file name.
    let c = catalog();
    let added = c.added("d:/games/racer/RACER.EXE").unwrap();
    assert_eq!(added.handler.guid, "Racer Deluxe");
    assert_eq!(c.added("E:\\Copy\\Racer.exe"), None);
    // An added game whose handler was deleted has none.
    assert_eq!(c.added("C:\\Old\\Old.exe"), None);
}

#[test]
fn ours_is_superseded_once_the_user_adds_the_game_with_a_hub_handler() {
    let mut c = catalog();
    let mut own = c.handlers[0].clone();
    own.own = true;
    own.guid = "Rubylight-racer".into();
    own.file = "Rubylight-racer.js".into();
    c.handlers.push(own);
    c.games.push(Game {
        guid: "Rubylight-racer".into(),
        exe_path: "d:/games/racer/racer.exe".into(),
    });
    let superseded = c.superseded(None);
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0].file, "Rubylight-racer.js");
    // A session about to start with it keeps it.
    assert!(c.superseded(Some("Rubylight-racer")).is_empty());
}

#[test]
fn ours_is_superseded_as_soon_as_a_hub_handler_for_its_exe_is_installed() {
    // Nucleus' add-game search skips a path in its list, so our entry
    // would keep the user from adding the game with the hub's handler.
    let mut c = catalog();
    let mut own = parse(
        "Rubylight-shooter.js",
        "// Rubylight one-player handler v1\nGame.GUID='Rubylight-shooter'\nGame.ExecutableName='SHOOTER.exe'",
    )
    .unwrap();
    assert!(own.own);
    c.handlers.push(own.clone());
    c.games.push(Game {
        guid: "Rubylight-shooter".into(),
        exe_path: "E:\\Shooter\\Shooter.exe".into(),
    });
    let names = |handlers: Vec<&Handler>| -> Vec<String> {
        handlers.into_iter().map(|h| h.guid.clone()).collect()
    };
    assert_eq!(names(c.superseded(None)), ["Rubylight-shooter"]);
    // Except the one a session starts with now (Portal's hl2.exe beside
    // Half-Life 2's handler): it goes at another game's session and is
    // written again at its own, never removed under it.
    assert!(c.superseded(Some("Rubylight-shooter")).is_empty());
    assert!(c.stale(Some("Rubylight-shooter")).is_empty());
    // Another of ours for the same .exe is no hub handler.
    c.handlers.retain(|h| h.guid != "Shooter");
    own.guid = "Rubylight-shooter-2".into();
    c.handlers.push(own);
    assert!(c.superseded(None).is_empty());
}

#[test]
fn ours_without_a_game_in_the_list_is_unused() {
    let mut c = catalog();
    let own = |guid: &str| {
        parse(
            &format!("{guid}.js"),
            &format!(
                "// Rubylight one-player handler v1\n// Written for {USER}\nGame.GUID='{guid}'\nGame.ExecutableName='Puzzle.exe'"
            ),
        )
        .unwrap()
    };
    c.handlers.push(own("Rubylight-left"));
    c.handlers.push(own("Rubylight-listed"));
    c.handlers.push(own("Rubylight-now"));
    c.games.push(Game {
        guid: "Rubylight-listed".into(),
        exe_path: "C:\\Puzzle\\Puzzle.exe".into(),
    });
    let guids = |handlers: Vec<&Handler>| -> Vec<String> {
        handlers.into_iter().map(|h| h.guid.clone()).collect()
    };
    assert_eq!(guids(c.unused(Some("Rubylight-now"))), ["Rubylight-left"]);
    assert_eq!(guids(c.stale(Some("Rubylight-now"))), ["Rubylight-left"]);
    // A hub handler for its .exe: listed or not, each goes once.
    c.handlers.push(
        parse(
            "Puzzle.js",
            "Game.GUID='Puzzle'\nGame.ExecutableName='Binaries\\\\puzzle.exe'",
        )
        .unwrap(),
    );
    assert_eq!(
        guids(c.stale(Some("Rubylight-now"))),
        ["Rubylight-left", "Rubylight-listed"]
    );
    // A hub or user handler is never ours to remove.
    assert!(c.stale(None).iter().all(|h| h.own));
}

#[test]
fn only_ours_written_for_this_account_can_be_unused() {
    // The handlers are the Nucleus folder's, shared by every account;
    // each account's list is in its own profile, which the host never
    // reads for another account.
    let mut c = catalog();
    for (guid, owner) in [
        ("Rubylight-mine", format!("\n// Written for {USER}")),
        (
            "Rubylight-theirs",
            "\n// Written for S-1-5-21-9-9-9-1002".into(),
        ),
        ("Rubylight-earlier", String::new()),
        ("Rubylight-forged", "\n// Written for ..\\x".into()),
    ] {
        c.handlers.push(
            parse(
                &format!("{guid}.js"),
                &format!(
                    "// Rubylight one-player handler v1{owner}\nGame.GUID='{guid}'\nGame.ExecutableName='{guid}.exe'"
                ),
            )
            .unwrap(),
        );
    }
    let owners: Vec<Option<&str>> = c.handlers[3..].iter().map(|h| h.owner.as_deref()).collect();
    assert_eq!(
        owners,
        [Some(USER), Some("S-1-5-21-9-9-9-1002"), None, None]
    );
    let unused: Vec<&str> = c.unused(None).iter().map(|h| h.guid.as_str()).collect();
    assert_eq!(unused, ["Rubylight-mine"]);
    // Nobody signed in: none.
    c.user = None;
    assert!(c.unused(None).is_empty());
    assert!(c.stale(None).is_empty());
    // A hub handler's comment never makes it ours.
    let hub = parse(
        "Hub.js",
        &format!("// Written for {USER}\nGame.GUID='Hub'\nGame.ExecutableName='Hub.exe'"),
    )
    .unwrap();
    assert_eq!((hub.own, hub.owner), (false, None));
}

#[test]
fn ours_names_the_account_it_was_written_for() {
    let setup = super::super::generic::Setup::new(r"D:\Puzzle\Puzzle.exe", "Puzzle", None);
    let written = parse(&setup.file(), &setup.handler_of(Some(USER))).unwrap();
    assert!(written.own);
    assert_eq!(written.owner.as_deref(), Some(USER));
    assert_eq!(written.guid, setup.guid());
    let earlier = parse(&setup.file(), &setup.handler()).unwrap();
    assert_eq!((earlier.own, earlier.owner), (true, None));
}

#[test]
fn a_game_nucleus_cannot_start_says_why() {
    let c = catalog();
    let none = c
        .find("Puzzle", Some("C:\\Puzzle\\Puzzle.exe"))
        .unwrap_err();
    assert_eq!(
        none.to_string(),
        "No Nucleus Co-op handler for Puzzle.exe: add one in Nucleus Co-op's handler hub"
    );
    let not_added = c.find("Shooter", Some("C:\\Shooter.exe")).unwrap_err();
    assert_eq!(
        not_added.to_string(),
        "Nucleus Co-op has the handler Shooter for Shooter.exe, but the game is not added: open Nucleus Co-op and add it once"
    );
    // An added game whose handler was deleted has no handler.
    assert!(matches!(
        c.find("Old", Some("C:\\Old\\Old.exe")),
        Err(Missing::NoHandler { .. })
    ));
    let steam = c.find("Steam Big Picture", None).unwrap_err();
    assert!(
        steam
            .to_string()
            .contains("Steam Big Picture does not name one"),
        "{steam}"
    );
    assert!(matches!(
        c.find("x", Some("  ")),
        Err(Missing::NoExecutable { .. })
    ));
    let anti_cheat = Missing::AntiCheat {
        executable: "Shooter.exe".into(),
        anti_cheat: "BattlEye".into(),
    };
    assert_eq!(
        anti_cheat.to_string(),
        "Shooter.exe uses BattlEye, and Rubylight never hooks a game with anti-cheat"
    );
}

#[test]
fn rubylights_own_handler_never_stands_in_for_a_hub_handler() {
    let ours = super::super::generic::Setup::new(r"C:\Puzzle\Puzzle.exe", "Puzzle", None);
    let mut c = catalog();
    c.handlers
        .push(parse(&ours.file(), &ours.handler()).unwrap());
    c.games.push(Game {
        guid: ours.guid(),
        exe_path: ours.exe_path.clone(),
    });
    // Added with ours alone, the game has no handler for co-op.
    assert!(matches!(
        c.find("Puzzle", Some(r"C:\Puzzle\Puzzle.exe")),
        Err(Missing::NoHandler { .. })
    ));
    assert!(c.playable().iter().all(|m| !m.handler.own));
    // A hub handler wins once the user adds the game with it.
    c.handlers.push(
        parse(
            "Puzzle.js",
            "Game.GUID='Puzzle'\nGame.ExecutableName='puzzle.exe'",
        )
        .unwrap(),
    );
    assert!(matches!(
        c.find("Puzzle", Some(r"C:\Puzzle\Puzzle.exe")),
        Err(Missing::NotAdded { .. })
    ));
    c.games.push(Game {
        guid: "Puzzle".into(),
        exe_path: r"C:\Puzzle\Puzzle.exe".into(),
    });
    let found = c.find("Puzzle", Some(r"c:\puzzle\puzzle.exe")).unwrap();
    assert_eq!(found.handler.guid, "Puzzle");
    assert!(!found.handler.own);
}

#[test]
fn the_coop_library_lists_each_added_game_with_a_handler_once() {
    let names: Vec<_> = catalog()
        .playable()
        .into_iter()
        .map(|m| m.handler.guid)
        .collect();
    assert_eq!(names, ["Racer Deluxe", "Twin"]);
}
