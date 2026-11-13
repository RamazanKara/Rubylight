use super::super::handler::{self, user_games};
use super::*;

fn steam_game() -> Setup {
    Setup::new(
        r"D:\SteamLibrary\steamapps\common\Black Flag\bin\AC4BFSP.exe",
        "Assassin's Creed \"IV\"",
        Some((242050, r"D:\SteamLibrary\steamapps\common\Black Flag")),
    )
}

#[test]
fn an_epic_manifest_gives_its_own_game_the_launchers_arguments() {
    let manifest = |command: &str, app: &str| {
        serde_json::json!({
            "AppName": app,
            "CatalogNamespace": "plague",
            "InstallLocation": r"D:\APlagueTale\",
            "LaunchExecutable": "bin/Game.exe",
            "LaunchCommand": command,
        })
        .to_string()
    };
    let exe = r"d:\aplaguetale\BIN\game.exe";
    let epic = "-EpicPortal -epicapp=Jackdaw -epicenv=Prod -epicsandboxid=plague";
    assert_eq!(
        epic_arguments(manifest("", "Jackdaw").as_bytes(), exe).unwrap(),
        Some(epic.into())
    );
    // A manifest that names a launcher stub still says what the game is.
    assert_eq!(
        epic_arguments(
            manifest("", "Jackdaw").as_bytes(),
            r"D:\APlagueTale\Other\Game.exe"
        )
        .unwrap(),
        Some(epic.into())
    );
    // Another game's, a folder that only starts the same, a launcher
    // component without an .exe, and what can't be used.
    for other in [
        r"D:\Other\Game.exe",
        r"D:\APlagueTale2\bin\Game.exe",
        r"D:\APlagueTale\bin\Game.bat",
    ] {
        assert_eq!(
            epic_arguments(manifest("", "Jackdaw").as_bytes(), other).unwrap(),
            None
        );
    }
    assert_eq!(epic_arguments(b"{}", exe).unwrap(), None);
    let no_exe = serde_json::json!({
        "AppName": "Jackdaw",
        "CatalogNamespace": "plague",
        "InstallLocation": r"D:\APlagueTale",
    });
    assert_eq!(
        epic_arguments(no_exe.to_string().as_bytes(), exe).unwrap(),
        None
    );
    assert!(epic_arguments(manifest("", "two words").as_bytes(), exe).is_err());
    assert!(epic_arguments(b"not json", exe).is_err());
    let why = epic_exit("Player 1's instance of Game exited", "Game");
    assert!(why.starts_with("Player 1's instance of Game exited: Game is an Epic Games game"));
}

/// Any user can write a manifest and the game may run elevated: nothing
/// in one puts an option on its command line.
#[test]
fn a_hostile_epic_manifest_puts_no_option_on_the_command_line() {
    let exe = r"D:\Game\Game.exe";
    let manifest = |app: &str, namespace: &str, command: &str, folder: &str| {
        serde_json::json!({
            "AppName": app,
            "CatalogNamespace": namespace,
            "InstallLocation": folder,
            "LaunchExecutable": "Game.exe",
            "LaunchCommand": command,
        })
        .to_string()
    };
    assert_eq!(
        epic_arguments(
            manifest("App", "Space", r#"-ExecCmds="quit" -log=C:\x"#, r"D:\Game").as_bytes(),
            exe
        )
        .unwrap(),
        Some("-EpicPortal -epicapp=App -epicenv=Prod -epicsandboxid=Space".into())
    );
    for (app, namespace) in [
        (r#"App" -ExecCmds="quit"#, "Space"),
        ("App", r#"Space"x"#),
        ("App -log", "Space"),
        ("App", r"C:\x"),
        ("", "Space"),
        (&*"a".repeat(129), "Space"),
    ] {
        let bytes = manifest(app, namespace, "", r"D:\Game");
        assert!(epic_arguments(bytes.as_bytes(), exe).is_err(), "{app}");
    }
    // Nor does it claim every game on a drive.
    for root in [r"D:\", "D:", r"D:/"] {
        assert_eq!(
            epic_arguments(manifest("App", "Space", "", root).as_bytes(), exe).unwrap(),
            None,
            "{root}"
        );
    }
}

#[test]
fn the_handler_reads_back_as_a_one_player_controller_handler_of_ours() {
    let setup = steam_game();
    let text = setup.handler();
    assert!(owns(&text));
    let h = handler::parse(&setup.file(), &text).unwrap();
    assert_eq!(h.guid, setup.guid());
    assert!(h.guid.starts_with("Rubylight-ac4bfsp-"), "{}", h.guid);
    assert_eq!(setup.file(), format!("{}.js", h.guid));
    assert_eq!(h.name, "Assassin's Creed \"IV\"");
    assert_eq!(h.executable, "AC4BFSP.exe");
    assert_eq!(h.max_players, 1);
    assert!(!h.supports_keyboard);
    assert!(!h.hides_taskbar);
    assert!(h.own);
    // A Nucleus Co-op that is not elevated runs it without a prompt.
    assert!(!handler::needs_admin(&h), "{:?}", h.admin);
    for line in [
        "Game.Hook.CustomDllEnabled = false;",
        "Game.FakeFocus = true;",
        "Game.ProtoInput.FocusHooks = true;",
        "Game.ProtoInput.XinputHook = true;",
        "Game.ProtoInput.UseOpenXinput = true;",
        "Game.ProtoInput.InjectRuntime_EasyHookMethod = true;",
    ] {
        assert!(text.lines().any(|l| l == line), "{line} in\n{text}");
    }
    // Nothing beyond one instance on one screen, and nothing written
    // beside the game.
    assert!(!text.contains("Game.HideTaskbar = true"));
    for option in [
        "Symlink",
        "Hardlink",
        "Hardcopy",
        "SteamAppId",
        "BinariesFolder",
    ] {
        assert!(!text.contains(option), "{option}");
    }
    assert!(
        text.lines()
            .skip(1)
            .all(|l| l.starts_with("Game.") || l.starts_with("//"))
    );
    assert!(!text.contains("StartArguments"));
}

/// Nucleus Co-op ends a game ProtoInput can't be injected into (#59): a
/// game that refused one way gets the next, as Nucleus starts it, then
/// from one of its own threads, then none, with the same hooks each
/// time and a handler otherwise the same.
#[test]
fn a_game_that_refused_the_hooks_gets_the_next_way_down_to_none() {
    let owner = Some("S-1-5-21-1-2-3-1001");
    let hooked = steam_game();
    assert_eq!(hooked.inject, Inject::Runtime);
    let runtime = hooked.handler_of(owner);
    // The first way's text, whole: v2 filtered the PC's keyboard and
    // mouse messages too (#56), v3 a DirectInput game's raw input
    // (NieR:Automata), so the handlers earlier versions wrote are
    // rewritten.
    let runtime_v3 = [
        "// Rubylight one-player handler v3: Rubylight writes this file for background streams and rewrites it when it changes. A hub handler for this game takes its place.",
        "// Written for S-1-5-21-1-2-3-1001",
        format!("Game.GUID = \"{}\";", hooked.guid()).as_str(),
        "Game.GameName = \"Assassin's Creed \\\"IV\\\"\";",
        "Game.ExecutableName = \"AC4BFSP.exe\";",
        "Game.MaxPlayers = 1;",
        "Game.MaxPlayersOneMonitor = 1;",
        "// The slot's pad only: keyboard and mouse stay with the person at the PC, and so does the taskbar.",
        "Game.SupportsKeyboard = false;",
        "Game.HideTaskbar = false;",
        "// The game runs from its own folder and nothing is written there: no links, copies, DLLs or tools that need administrator rights.",
        "Game.Hook.CustomDllEnabled = false;",
        "// The game keeps running and reading its pad without focus, and never sees the PC's keyboard or moves its cursor.",
        "Game.FakeFocus = true;",
        "Game.ProtoInput.InjectStartup = false;",
        "Game.ProtoInput.InjectRuntime_EasyHookMethod = true;",
        "Game.ProtoInput.FocusHooks = true;",
        "Game.ProtoInput.XinputHook = true;",
        "Game.ProtoInput.UseOpenXinput = true;",
        "Game.ProtoInput.GetKeyStateHook = true;",
        "Game.ProtoInput.GetAsyncKeyStateHook = true;",
        "Game.ProtoInput.GetKeyboardStateHook = true;",
        "Game.ProtoInput.GetCursorPosHook = true;",
        "Game.ProtoInput.SetCursorPosHook = true;",
        "Game.ProtoInput.ClipCursorHook = true;",
        "// Even in front, the game's window messages carry no key, click, wheel or cursor position from the PC's keyboard and mouse (#56).",
        "Game.ProtoInput.MessageFilterHook = true;",
        "Game.ProtoInput.KeyboardButtonFilter = true;",
        "Game.ProtoInput.MouseButtonFilter = true;",
        "Game.ProtoInput.MouseWheelFilter = true;",
        "Game.ProtoInput.MouseMoveFilter = true;",
    ]
    .join("\r\n")
        + "\r\n";
    assert_eq!(runtime, runtime_v3);
    // A game reading DirectInput (NieR:Automata) gets its raw input
    // taken too, after the rest; any other keeps it, so a pad it reads
    // through raw input (SDL, Rewired) still reaches it.
    let direct_input = Setup {
        direct_input: true,
        ..steam_game()
    }
    .handler_of(owner);
    let raw_input = [
        "// Nor its raw input, through which DirectInput reads the PC's keyboard (NieR:Automata, #56): ProtoInput takes the game's raw input and passes on only the slot's keyboard and mouse, and a pad's slot has none. The game reads DirectInput, so its pad does not come through raw input.",
        "Game.ProtoInput.RegisterRawInputHook = true;",
        "Game.ProtoInput.GetRawInputDataHook = true;",
    ];
    assert_eq!(
        direct_input,
        runtime_v3.clone() + &raw_input.join("\r\n") + "\r\n"
    );
    assert!(!runtime.contains("RawInput"), "{runtime}");
    let hook_lines = |text: &str| {
        text.lines()
            .filter(|l| l.starts_with("Game.ProtoInput.") && l.contains("Hook"))
            .filter(|l| !l.contains("Inject"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let mut ways = vec![Inject::Runtime];
    while let Some(next) = ways.last().unwrap().next() {
        ways.push(next);
    }
    assert_eq!(ways, Inject::ALL);
    let mut texts = Vec::new();
    for way in Inject::ALL {
        assert_eq!(Inject::from_name(way.name()), Some(way));
        let setup = Setup {
            inject: way,
            ..steam_game()
        };
        let text = setup.handler_of(owner);
        assert!(owns(&text));
        let says = |line: &str| text.lines().any(|l| l == line);
        // Nucleus' own focus messages are no hook, but without ProtoInput's
        // cursor hooks they make a game take the PC's mouse back every
        // second, however often its person clicks away (#59).
        assert_eq!(
            says("Game.FakeFocus = true;"),
            way != Inject::None,
            "{text}"
        );
        match way {
            Inject::Runtime => assert_eq!(text, runtime),
            Inject::Startup => {
                assert!(says("Game.ProtoInput.InjectStartup = true;"), "{text}");
                assert!(!text.contains("InjectRuntime"), "{text}");
            }
            Inject::Stealth => {
                assert!(says("Game.ProtoInput.InjectStartup = false;"), "{text}");
                assert!(
                    says("Game.ProtoInput.InjectRuntime_EasyHookStealthMethod = true;"),
                    "{text}"
                );
                assert!(!text.contains("InjectRuntime_EasyHookMethod"), "{text}");
            }
            Inject::None => {
                assert!(!text.contains("Game.ProtoInput"), "{text}");
                assert!(!text.contains("Inject"), "{text}");
                // Else the setup screen lists no pad and the profile never
                // starts the game (#59).
                assert!(says("Game.Hook.XInputEnabled = true;"), "{text}");
                assert!(says("Game.Hook.CustomDllEnabled = false;"), "{text}");
            }
        }
        if way != Inject::None {
            assert_eq!(hook_lines(&text), hook_lines(&runtime), "{way:?}");
        }
        let h = handler::parse(&setup.file(), &text).unwrap();
        assert_eq!(h.guid, hooked.guid());
        assert_eq!(setup.file(), hooked.file());
        assert_eq!(h.max_players, 1);
        assert!(!h.supports_keyboard);
        assert!(!handler::needs_admin(&h), "{way:?}: {:?}", h.admin);
        assert!(
            text.lines()
                .skip(1)
                .all(|l| l.starts_with("Game.") || l.starts_with("//"))
        );
        texts.push(text);
    }
    // Each its own text: the handler written before is rewritten.
    texts.sort();
    texts.dedup();
    assert_eq!(texts.len(), Inject::ALL.len());
}

#[test]
fn a_launcher_is_no_game() {
    for exe in [
        r"C:\Program Files (x86)\Steam\steam.exe",
        r"C:\Program Files\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe",
        r"C:\Users\Player\AppData\Local\Playnite\Playnite.FullscreenApp.exe",
        r"D:\Steam\steamapps\common\Fallout 4\Fallout4Launcher.exe",
        r"D:\Steam\steamapps\common\Skyrim Special Edition\SkyrimSELauncher.exe",
        r"D:\Games\GTAV\PlayGTAV.exe",
        // Named like no launcher.
        r"D:\Games\RDR2\PlayRDR2.exe",
        r"D:\games\RetroBat\RetroBat.exe",
        r"C:\Users\Player\LaunchBox\LaunchBox.exe",
        r"C:\Users\Player\LaunchBox\BigBox.exe",
    ] {
        assert!(launcher(exe), "{exe}");
    }
    for game in [
        r"D:\Steam\steamapps\common\Fallout 4\Fallout4.exe",
        r"D:\Games\GTAV\GTA5.exe",
        r"D:\Games\Ice\Ice.exe",
    ] {
        assert!(!launcher(game), "{game}");
    }
}

#[test]
fn an_unreal_root_stub_starts_its_one_shipping_exe() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("Stray");
    let win64 = game.join("Hk_project").join("Binaries").join("Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir_all(game.join("Engine").join("Binaries").join("Win64")).unwrap();
    std::fs::write(game.join("Stray.exe"), b"").unwrap();
    std::fs::write(win64.join("Stray-Win64-Shipping.exe"), b"").unwrap();
    let stub = game.join("Stray.exe").to_string_lossy().into_owned();
    assert_eq!(
        unreal_shipping(&stub).unwrap(),
        win64.join("Stray-Win64-Shipping.exe").to_string_lossy()
    );
    // The Shipping exe itself is the game.
    let shipping = win64.join("Stray-Win64-Shipping.exe");
    assert_eq!(unreal_shipping(&shipping.to_string_lossy()), None);
    // Two to choose from: neither.
    let other = game.join("Other").join("Binaries").join("Win64");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Other-Win64-Shipping.exe"), b"").unwrap();
    assert_eq!(unreal_shipping(&stub), None);
    // Not an Unreal game: no Engine beside it.
    std::fs::remove_dir_all(other.parent().unwrap().parent().unwrap()).unwrap();
    std::fs::remove_dir_all(game.join("Engine")).unwrap();
    assert_eq!(unreal_shipping(&stub), None);
}

#[test]
fn the_game_starts_with_its_arguments() {
    // Portal is hl2.exe -game portal: without them it is Half-Life 2.
    let setup = Setup {
        arguments: r#" -game portal +map "a b\c" "#.into(),
        ..Setup::new(r"D:\Steam\steamapps\common\Portal\hl2.exe", "Portal", None)
    };
    let text = setup.handler();
    assert!(
        text.lines()
            .any(|l| l == r#"Game.StartArguments = "-game portal +map \"a b\\c\"";"#),
        "{text}"
    );
    assert!(handler::parse(&setup.file(), &text).is_ok());
}

#[test]
fn unreal_games_start_in_a_window_unless_their_arguments_choose() {
    // #56: Clair Obscur and Silent Hill f went borderless fullscreen on
    // the PC's monitor, wherever their window was parked.
    let unreal = |arguments: &str| Setup {
        arguments: arguments.into(),
        ..Setup::new(
            r"D:\Steam\steamapps\common\Expedition 33\Sandfall\Binaries\Win64\SandFall-Win64-Shipping.exe",
            "Clair Obscur",
            None,
        )
    };
    assert_eq!(unreal("").windowed_arguments(), "-windowed");
    assert_eq!(unreal(" -dx12 ").windowed_arguments(), "-dx12 -windowed");
    assert_eq!(unreal("-FullScreen").windowed_arguments(), "-FullScreen");
    assert_eq!(
        unreal("-windowed -ResX=1280").windowed_arguments(),
        "-windowed -ResX=1280"
    );
    // #56: in its window it renders at the stream's size, the phone's
    // 20:9, not at its saved 16:9; a resolution the arguments choose stays.
    let streamed = |arguments: &str| Setup {
        size: Some((2400, 1080)),
        ..unreal(arguments)
    };
    assert_eq!(
        streamed("").windowed_arguments(),
        "-windowed -ResX=2400 -ResY=1080 -ForceRes"
    );
    assert_eq!(
        streamed("-dx12 -windowed").windowed_arguments(),
        "-dx12 -windowed -ResX=2400 -ResY=1080 -ForceRes"
    );
    // Larger than the PC's monitor it keeps its size (-ForceRes), once.
    assert_eq!(
        streamed("-forceres").windowed_arguments(),
        "-forceres -windowed -ResX=2400 -ResY=1080"
    );
    assert_eq!(
        streamed("-ResY=720").windowed_arguments(),
        "-ResY=720 -windowed"
    );
    // Never in fullscreen, which would change the PC monitor's mode.
    assert_eq!(streamed("-fullscreen").windowed_arguments(), "-fullscreen");
    // Any other game keeps its own arguments: no switch is known for it.
    let portal = Setup {
        arguments: " -game portal ".into(),
        size: Some((2400, 1080)),
        ..Setup::new(r"D:\Steam\steamapps\common\Portal\hl2.exe", "Portal", None)
    };
    assert_eq!(portal.windowed_arguments(), "-game portal");
}

#[test]
fn the_guid_is_stable_for_a_path_and_differs_between_copies() {
    let a = Setup::new(r"C:\Games\Puzzle\Puzzle Game.exe", "Puzzle", None);
    let again = Setup::new("c:/games/puzzle/PUZZLE GAME.EXE", "Renamed", None);
    let copy = Setup::new(r"E:\Puzzle\Puzzle Game.exe", "Puzzle", None);
    assert_eq!(a.guid(), again.guid());
    assert_ne!(a.guid(), copy.guid());
    assert!(a.guid().starts_with("Rubylight-puzzle_game-"));
    assert_eq!(a.guid().len(), "Rubylight-puzzle_game-".len() + 8);
    assert!(!a.guid().contains(' '));
}

#[test]
fn line_and_paragraph_separators_are_escaped_for_nucleus_es5_engine() {
    let setup = Setup::new(r"C:\Games\Odd\Odd.exe", "Line\u{2028}Para\u{2029}End", None);
    let text = setup.handler();
    assert!(!text.contains('\u{2028}') && !text.contains('\u{2029}'));
    assert!(
        text.contains(r#"Game.GameName = "Line\u2028Para\u2029End";"#),
        "{text}"
    );
    assert_eq!(
        handler::parse(&setup.file(), &text).unwrap().name,
        "Line\u{2028}Para\u{2029}End"
    );
}

#[test]
fn the_games_folder_is_its_steam_or_unreal_folder_or_the_exes() {
    assert_eq!(
        steam_game().root,
        r"D:\SteamLibrary\steamapps\common\Black Flag"
    );
    let plain = Setup::new(r"C:\Games\Puzzle\Puzzle.exe", "Puzzle", None);
    assert_eq!(plain.root, r"C:\Games\Puzzle");
    let unreal = Setup::new(
        r"D:\Epic\Hero\HeroGame\Binaries\Win64\Hero.exe",
        "Hero",
        None,
    );
    assert_eq!(unreal.root, r"D:\Epic\Hero");
    // A game right on a drive is its own folder, never the drive's.
    for (exe, root) in [
        (r"D:\Game\Binaries\Win32\Game.exe", r"D:\Game"),
        (r"\\?\D:\Game\Binaries\Win64\Game.exe", r"\\?\D:\Game"),
        (r"D:\Binaries\Win64\Game.exe", r"D:\Binaries\Win64"),
    ] {
        assert_eq!(Setup::new(exe, "Game", None).root, root, "{exe}");
    }
    // A Steam folder the exe is not in does not count.
    let elsewhere = Setup::new(
        r"C:\Other\Game.exe",
        "Game",
        Some((7, r"D:\Steam\common\Game\")),
    );
    assert_eq!(elsewhere.root, r"C:\Other");
    assert_eq!(elsewhere.steam_app, Some(7));
    assert_eq!(plain.steam_app, None);
    let inside = Setup::new(
        r"D:\Steam\common\Game\Game.exe",
        "Game",
        Some((7, r"D:\Steam\common\Game\")),
    );
    assert_eq!(inside.root, r"D:\Steam\common\Game");
}

#[test]
fn anti_cheat_is_found_beside_the_exe_and_up_to_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Shooter");
    let bin = root.join("Shooter").join("Binaries").join("Win64");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir(root.join("Engine")).unwrap();
    let exe = bin.join("Shooter.exe");
    std::fs::write(&exe, b"").unwrap();
    let setup = Setup::new(&exe.to_string_lossy(), "Shooter", None);
    assert_eq!(setup.root, root.to_string_lossy());
    assert_eq!(anti_cheat(&setup), None);
    std::fs::create_dir(root.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    std::fs::remove_dir(root.join("EasyAntiCheat")).unwrap();
    std::fs::write(bin.join("BEClient_x64.dll"), b"").unwrap();
    assert_eq!(anti_cheat(&setup), Some("BattlEye"));
    // Above the root is another game's business.
    std::fs::remove_file(bin.join("BEClient_x64.dll")).unwrap();
    std::fs::create_dir(dir.path().join("BattlEye")).unwrap();
    assert_eq!(anti_cheat(&setup), None);
    for (name, found) in [
        ("vgc.exe", "Riot Vanguard"),
        ("GameGuard", "nProtect GameGuard"),
        ("randgrid.sys", "Ricochet"),
        ("EAAntiCheat.GameServiceLauncher.exe", "EA Javelin"),
        ("XIGNCODE", "XIGNCODE3"),
        ("PnkBstrA.exe", "PunkBuster"),
    ] {
        assert_eq!(anti_cheat_of(name), Some(found), "{name}");
    }
    assert_eq!(anti_cheat_of("Shooter.exe"), None);
    let protected = Setup::new(r"C:\G\start_protected_game.exe", "G", None);
    assert_eq!(anti_cheat(&protected), Some("Easy Anti-Cheat"));
}

#[test]
fn an_unreal_scan_stops_at_the_game_holding_binaries_unless_engine_is_above() {
    let dir = tempfile::tempdir().unwrap();
    let games = dir.path().join("Games");
    // Another game's anti-cheat in the user's games folder.
    std::fs::create_dir_all(games.join("BattlEye")).unwrap();
    // Unreal Engine 3: Binaries and Engine in the game's folder.
    let borderlands = games.join("Borderlands 2");
    let win32 = borderlands.join("Binaries").join("Win32");
    std::fs::create_dir_all(&win32).unwrap();
    std::fs::create_dir(borderlands.join("Engine")).unwrap();
    let setup = Setup::new(
        &win32.join("Borderlands2.exe").to_string_lossy(),
        "Borderlands 2",
        None,
    );
    assert_eq!(anti_cheat(&setup), None);
    std::fs::create_dir(borderlands.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    // Unreal Engine 4 and 5: `<Game>\<Project>\Binaries`, Engine in
    // `<Game>`, whose anti-cheat counts.
    let shooter = games.join("Shooter");
    let win64 = shooter.join("Shooter").join("Binaries").join("Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir(shooter.join("Engine")).unwrap();
    let exe = win64.join("Shooter-Win64-Shipping.exe");
    std::fs::write(&exe, b"").unwrap();
    let setup = Setup::new(&exe.to_string_lossy(), "Shooter", None);
    assert_eq!(anti_cheat(&setup), None);
    std::fs::create_dir(shooter.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    // The root stub's game, as `prepared` starts it.
    std::fs::write(shooter.join("Shooter.exe"), b"").unwrap();
    let mut stub = Setup::new(
        &shooter.join("Shooter.exe").to_string_lossy(),
        "Shooter",
        None,
    );
    stub.exe_path = unreal_shipping(&stub.exe_path).unwrap();
    assert_eq!(stub.exe_path, exe.to_string_lossy());
    assert_eq!(anti_cheat(&stub), Some("Easy Anti-Cheat"));
    std::fs::remove_dir(shooter.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&stub), None);
    // No Engine folder, but the root stub beside the project: the game's
    // folder is the stub's, and the games folder still is not read.
    let hero = games.join("Hero");
    let win64 = hero.join("HeroGame").join("Binaries").join("Win64");
    std::fs::create_dir_all(&win64).unwrap();
    std::fs::create_dir(hero.join("EasyAntiCheat")).unwrap();
    let exe = win64.join("Hero-Win64-Shipping.exe");
    std::fs::write(&exe, b"").unwrap();
    let setup = Setup::new(&exe.to_string_lossy(), "Hero", None);
    assert_eq!(anti_cheat(&setup), None);
    std::fs::write(hero.join("Hero.exe"), b"").unwrap();
    assert_eq!(anti_cheat(&setup), Some("Easy Anti-Cheat"));
    std::fs::remove_dir(hero.join("EasyAntiCheat")).unwrap();
    assert_eq!(anti_cheat(&setup), None);
    assert_eq!(stub_name(r"C:\G\Stray-WIN64-Shipping.exe"), "Stray.exe");
    assert_eq!(stub_name(r"C:\G\HogwartsLegacy.exe"), "HogwartsLegacy.exe");
}

#[test]
fn a_battle_net_game_has_blizzards_anti_cheat() {
    let dir = tempfile::tempdir().unwrap();
    for (game, exe) in [
        ("World of Warcraft", r"_retail_\Wow.exe"),
        ("StarCraft II", r"Versions\Base90136\SC2_x64.exe"),
        ("Diablo IV", "Diablo IV.exe"),
    ] {
        let game = dir.path().join(game);
        let exe = game.join(exe);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let setup = Setup::new(&exe.to_string_lossy(), "G", None);
        assert_eq!(anti_cheat(&setup), None, "{}", exe.display());
        std::fs::write(game.join(".build.info"), b"Branch!STRING:0").unwrap();
        assert_eq!(
            anti_cheat(&setup),
            Some("Blizzard's anti-cheat"),
            "{}",
            exe.display()
        );
    }
    // Never above the game's folder or in a folder games share.
    let deep = dir.path().join(r"Mods\a\b\c\Mod.exe");
    std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
    std::fs::write(dir.path().join(r"Mods\.build.info"), b"").unwrap();
    assert_eq!(
        anti_cheat(&Setup::new(&deep.to_string_lossy(), "G", None)),
        None
    );
    let programs = dir.path().join("Program Files (x86)");
    std::fs::create_dir_all(programs.join("Puzzle")).unwrap();
    std::fs::write(programs.join(".build.info"), b"").unwrap();
    let puzzle = programs.join(r"Puzzle\Puzzle.exe");
    assert_eq!(
        anti_cheat(&Setup::new(&puzzle.to_string_lossy(), "G", None)),
        None
    );
}

#[test]
fn anti_cheat_without_files_in_the_game_is_known_by_name_or_steam() {
    for (exe, found) in [
        (
            r"D:\S\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe",
            "Valve Anti-Cheat",
        ),
        (
            r"D:\S\common\dota 2 beta\game\bin\win64\dota2.exe",
            "Valve Anti-Cheat",
        ),
        (
            r"D:\S\common\Team Fortress 2\tf_win64.exe",
            "Valve Anti-Cheat",
        ),
        (
            r"C:\Games\Overwatch\_retail_\Overwatch.exe",
            "Blizzard's anti-cheat",
        ),
        (r"C:\Games\PUBG\TslGame.exe", "BattlEye"),
    ] {
        assert_eq!(
            anti_cheat(&Setup::new(exe, "G", None)),
            Some(found),
            "{exe}"
        );
    }
    assert_eq!(anti_cheat_of("FACEITClient.exe"), Some("FACEIT"));
    // Steam's Valve Anti-Cheat flag, for a game whose files say nothing.
    let flagged = Setup {
        anti_cheat: Some("Valve Anti-Cheat"),
        ..Setup::new(r"Z:\NoSuch\Portal\hl2.exe", "Portal", None)
    };
    assert_eq!(anti_cheat(&flagged), Some("Valve Anti-Cheat"));
}

#[test]
fn a_network_share_or_windows_own_program_is_refused_before_it_opens() {
    let windows = r"C:\Windows";
    for share in [
        r"\\nas\games\Ice\Ice.exe",
        "//nas/games/Ice/Ice.exe",
        r"\\?\UNC\nas\games\Ice\Ice.exe",
        r"\\.\pipe\Ice.exe",
        r"\\?\GLOBALROOT\Device\Mup\nas\games\Ice.exe",
        r"\\.\UNC\nas\games\Ice\Ice.exe",
        r"\\?\Volume{0c0d2a1e-0000-0000-0000-100000000000}\Ice.exe",
        // NT's own namespace, which Windows resolves too.
        r"\??\UNC\nas\games\Ice\Ice.exe",
        r"\??\GLOBALROOT\Device\Mup\nas\games\Ice.exe",
        r"\??\C:\Games\Ice\Ice.exe",
        "/??/UNC/nas/games/Ice/Ice.exe",
        r"\Device\Mup\nas\games\Ice.exe",
        r"\Games\Ice\Ice.exe",
    ] {
        let why = refused_path(share, windows).unwrap();
        assert!(
            why.ends_with(
                "is on a network share: Rubylight starts only a game on this PC's own drives"
            ),
            "{why}"
        );
    }
    assert_eq!(
        refused_path(r"C:\Windows\notepad.exe", windows).unwrap(),
        r"C:\Windows\notepad.exe is part of Windows, not a game"
    );
    assert!(refused_path(r"\\?\c:\windows\System32\mspaint.exe", windows).is_some());
    // The user's own folders and other drives are fine.
    for game in [
        r"C:\Users\Player\Games\Ice\Ice.exe",
        r"\\?\D:\Games\Ice\Ice.exe",
        r"C:\WindowsGames\Ice.exe",
        r"D:\Games\Windows\Ice.exe",
    ] {
        assert_eq!(refused_path(game, windows), None, "{game}");
    }
}

#[test]
fn the_scan_stays_in_the_games_folder_and_out_of_shared_ones() {
    let dir = tempfile::tempdir().unwrap();
    // Epic's system-wide EasyAntiCheat_EOS beside a plain game's folder.
    let programs = dir.path().join("Programs");
    let puzzle = programs.join("Puzzle");
    std::fs::create_dir_all(&puzzle).unwrap();
    std::fs::create_dir(programs.join("EasyAntiCheat_EOS")).unwrap();
    let setup = Setup::new(&puzzle.join("Puzzle.exe").to_string_lossy(), "Puzzle", None);
    assert_eq!(anti_cheat(&setup), None);
    // An .exe right in Program Files (x86) or a Steam library's common
    // folder: those are never read.
    for shared in [
        dir.path().join("Program Files (x86)"),
        dir.path().join("steamapps").join("common"),
        dir.path().join("Epic Games"),
    ] {
        std::fs::create_dir_all(shared.join("EasyAntiCheat_EOS")).unwrap();
        let setup = Setup::new(&shared.join("Game.exe").to_string_lossy(), "Game", None);
        assert_eq!(anti_cheat(&setup), None, "{}", shared.display());
    }
    assert!(shared_folder(r"C:\") && shared_folder("C:"));
    assert!(!shared_folder(r"C:\Games\Puzzle"));
}

const PROFILE: &str = "\u{feff}{\r\n  \"Games\": [\r\n    {\r\n      \"GameGuid\": \"Portal 2\",\r\n      \"ExePath\": \"C:\\\\Games\\\\portal2.exe\"\r\n    },\r\n    {\r\n      \"GameGuid\": 7\r\n    }\r\n  ],\r\n  \"Theme\": \"dark\"\r\n}";

#[test]
fn the_game_is_added_after_the_users_games_which_stay_as_they_are() {
    let added = add_game(Some(PROFILE), "Rubylight-Ice-12345678", r"D:\Ice\Ice.exe")
        .unwrap()
        .unwrap();
    // Every byte of the user's file is still there, ours goes last.
    let cut = PROFILE.find("\r\n  ],").unwrap();
    assert!(added.starts_with(&PROFILE[..cut]));
    assert!(added.ends_with(&PROFILE[cut..]));
    assert_eq!(
        &added[cut..added.len() - (PROFILE.len() - cut)],
        ",\r\n    {\r\n      \"GameGuid\": \"Rubylight-Ice-12345678\",\r\n      \"ExePath\": \"D:\\\\ice\\\\ice.exe\"\r\n    }"
    );
    let games = user_games(&added).unwrap();
    assert_eq!(games.len(), 2);
    assert_eq!(games[1].exe_path, r"D:\ice\ice.exe");
    // Added once: the second time changes nothing.
    assert_eq!(
        add_game(Some(&added), "Rubylight-Ice-12345678", r"D:\Ice\Ice.exe"),
        Ok(None)
    );
}

#[test]
fn a_new_or_empty_profile_gets_a_game_list() {
    for json in [
        None,
        Some(""),
        Some("{}"),
        Some("{\"Games\":null}"),
        Some("{\"Games\": []}"),
    ] {
        let added = add_game(json, "Rubylight-A-00000000", r"C:\A\a.exe")
            .unwrap()
            .unwrap();
        let games = user_games(&added).unwrap();
        assert_eq!(games.len(), 1, "{added}");
        assert_eq!(games[0].guid, "Rubylight-A-00000000");
    }
    // A file Nucleus could not read either is left alone.
    assert!(
        add_game(Some("{\"Games\": ["), "g", "x")
            .unwrap_err()
            .contains("not readable")
    );
    assert!(add_game(Some("{\"Games\": 3}"), "g", "x").is_err());
    // A key named like the list inside a string does not count.
    let tricky = "{\"Note\": \"Games\", \"Games\": [ ]}";
    let added = add_game(Some(tricky), "g", "x").unwrap().unwrap();
    assert!(
        added.starts_with("{\"Note\": \"Games\", \"Games\": ["),
        "{added}"
    );
    assert_eq!(user_games(&added).unwrap().len(), 1);
}

#[test]
fn a_template_change_keeps_the_guid_and_the_added_game() {
    let setup = steam_game();
    let added = add_game(Some(PROFILE), &setup.guid(), &setup.exe_path)
        .unwrap()
        .unwrap();
    let mut moved = setup.clone();
    moved.name = "Black Flag".into();
    assert_ne!(moved.handler(), setup.handler());
    assert_eq!(moved.guid(), setup.guid());
    assert_eq!(
        add_game(Some(&added), &moved.guid(), &moved.exe_path),
        Ok(None)
    );
    // A handler someone else wrote is never ours, even with our name.
    assert!(!owns("// Rubylight's handler\nGame.GUID = 'Rubylight-x';"));
}

#[test]
fn our_game_leaves_the_list_and_every_other_byte_stays() {
    let json = "{\r\n  \"Games\": [\r\n    {\r\n      \"GameGuid\": \"Portal 2\",\r\n      \"ExePath\": \"C:\\\\Games\\\\Portal 2\\\\portal2.exe\"\r\n    },\r\n    {\r\n      \"GameGuid\": \"Rubylight-ice\",\r\n      \"ExePath\": \"C:\\\\Ice\\\\Ice.exe\"\r\n    },\r\n    {\r\n      \"GameGuid\": \"Racer\",\r\n      \"ExePath\": \"D:\\\\Racer.exe\"\r\n    }\r\n  ],\r\n  \"Theme\": \"[x]\"\r\n}";
    let removed = remove_game(json, "Rubylight-ice").unwrap().unwrap();
    assert_eq!(
        removed,
        json.replace(
            "    {\r\n      \"GameGuid\": \"Rubylight-ice\",\r\n      \"ExePath\": \"C:\\\\Ice\\\\Ice.exe\"\r\n    },\r\n",
            ""
        )
    );
    let games = user_games(&removed).unwrap();
    assert_eq!(
        games.iter().map(|g| g.guid.as_str()).collect::<Vec<_>>(),
        ["Portal 2", "Racer"]
    );
    // Last in the list, or alone in it.
    let last = remove_game(&removed, "Racer").unwrap().unwrap();
    assert_eq!(user_games(&last).unwrap().len(), 1);
    let alone = remove_game(&last, "Portal 2").unwrap().unwrap();
    assert!(user_games(&alone).unwrap().is_empty());
    assert!(
        alone.ends_with("\"Games\": [],\r\n  \"Theme\": \"[x]\"\r\n}"),
        "{alone}"
    );
    // Not there: nothing to write.
    assert_eq!(remove_game(json, "Rubylight-other").unwrap(), None);
    assert_eq!(remove_game("{}", "Rubylight-ice").unwrap(), None);
}
