use super::*;
#[test]
fn newer_apps_find_their_portrait_in_the_hashed_folder_and_on_the_cdn() {
    let root = std::env::temp_dir().join(format!("rubylight-steam-art-{}", std::process::id()));
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let folder = root
        .join("appcache")
        .join("librarycache")
        .join("2050650")
        .join(hash);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("library_600x900.jpg"), b"jpg").unwrap();
    let (portrait, _, _) = artwork(2050650, &root, &[]);
    let portrait = portrait.unwrap();
    assert_eq!(portrait, folder.join("library_600x900.jpg"));
    assert_eq!(
        hashed_store_portrait_url(2050650, &portrait).as_deref(),
        Some(
            "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/2050650/0123456789abcdef0123456789abcdef01234567/library_600x900_2x.jpg"
        )
    );
    let capsule = folder.join("library_capsule.png");
    assert!(
        hashed_store_portrait_url(7, &capsule)
            .unwrap()
            .ends_with(&format!("/7/{hash}/library_capsule_2x.jpg"))
    );
    // Not a content hash, or not a portrait: keep to the fixed path.
    assert_eq!(
        hashed_store_portrait_url(7, Path::new("cache/7/library_600x900.jpg")),
        None
    );
    assert_eq!(
        hashed_store_portrait_url(7, &folder.join("library_header.jpg")),
        None
    );
    assert_eq!(
        hashed_store_portrait_url(
            7,
            &folder
                .with_file_name("ABCDEF0123456789ABCDEF0123456789ABCDEF01")
                .join("library_600x900.jpg")
        ),
        None
    );
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn text_vdf_reads_library_folders_and_manifests() {
    let folders = Vdf::parse(
        r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps" { "228980" "1" }
	}
	// a comment
	"1" { "path" "D:\\SteamLibrary" }
	}
}"#,
    );
    let paths: Vec<_> = folders
        .get("libraryfolders")
        .unwrap()
        .entries()
        .iter()
        .filter_map(|(_, f)| f.get("path").and_then(Vdf::text))
        .collect();
    assert_eq!(
        paths,
        ["C:\\Program Files (x86)\\Steam", "D:\\SteamLibrary"]
    );
    let manifest = Vdf::parse(
        "\"AppState\" { \"appid\" \"570\" \"name\" \"Dota 2\" \"installdir\" \"dota 2 beta\" }",
    );
    assert_eq!(
        manifest.at(&["AppState", "appid"]).and_then(Vdf::number),
        Some(570)
    );
    assert_eq!(
        manifest.at(&["appstate", "NAME"]).and_then(Vdf::text),
        Some("Dota 2")
    );
}
fn binary_entry(appid: u32, kv: &[u8]) -> Vec<u8> {
    let mut entry = appid.to_le_bytes().to_vec();
    entry.extend(((60 + kv.len()) as u32).to_le_bytes());
    entry.extend([0; 60]);
    entry.extend(kv);
    entry
}
#[test]
fn binary_appinfo_gives_names_and_types() {
    // Format 29: keys are indices into a string table.
    let keys = ["appinfo", "common", "name", "type"];
    let mut kv = vec![0x00];
    kv.extend(0u32.to_le_bytes());
    kv.push(0x00);
    kv.extend(1u32.to_le_bytes());
    kv.push(0x01);
    kv.extend(2u32.to_le_bytes());
    kv.extend(b"Half-Life\0");
    kv.push(0x01);
    kv.extend(3u32.to_le_bytes());
    kv.extend(b"Game\0");
    kv.extend([0x08, 0x08, 0x08]);
    let mut data = 0x0756_4429u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    data.extend([0; 8]);
    data.extend(binary_entry(70, &kv));
    data.extend(binary_entry(80, &kv));
    data.extend(0u32.to_le_bytes());
    let table = data.len() as u64;
    data[8..16].copy_from_slice(&table.to_le_bytes());
    data.extend((keys.len() as u32).to_le_bytes());
    for key in keys {
        data.extend(key.as_bytes());
        data.push(0);
    }
    let found = appinfo(&data, &BTreeSet::from([70]));
    assert_eq!(found.len(), 1);
    assert_eq!(found[&70], ("Half-Life".into(), "Game".into()));
    assert!(appinfo(&data[..40], &BTreeSet::from([70])).is_empty());
}
/// Binary VDF in format 28: an object with its children, a text.
fn object(key: &str, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0x00];
    out.extend(key.as_bytes());
    out.push(0);
    children.iter().for_each(|child| out.extend(child));
    out.push(0x08);
    out
}
fn text(key: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x01];
    for part in [key, value] {
        out.extend(part.as_bytes());
        out.push(0);
    }
    out
}
fn option(key: &str, executable: &str, kind: &str, config: &[(&str, &str)]) -> Vec<u8> {
    let config: Vec<_> = config.iter().map(|(k, v)| text(k, v)).collect();
    object(
        key,
        &[
            text("executable", executable),
            text("type", kind),
            object("config", &config),
        ],
    )
}
/// A Steam folder with Black Flag (242050) installed and its launch
/// options in the app cache, Steam's order first.
fn steam_with_launch_options(options: &[Vec<u8>]) -> tempfile::TempDir {
    steam_with_appinfo(options, &[])
}
/// The same with `common` holding `common`.
fn steam_with_appinfo(options: &[Vec<u8>], common: &[Vec<u8>]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let apps = root.path().join("steamapps");
    let game = apps.join("common").join("Black Flag");
    std::fs::create_dir_all(game.join("bin")).unwrap();
    std::fs::write(game.join("bin").join("AC4BFSP.exe"), b"").unwrap();
    std::fs::write(
        apps.join("appmanifest_242050.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"242050\"\n\t\"installdir\"\t\t\"Black Flag\"\n}\n",
    )
    .unwrap();
    let mut kv = object(
        "appinfo",
        &[
            object("common", common),
            object("config", &[object("launch", options)]),
        ],
    );
    kv.extend([0x08, 0x08]);
    let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    data.extend(binary_entry(242050, &kv));
    data.extend(0u32.to_le_bytes());
    std::fs::create_dir_all(root.path().join("appcache")).unwrap();
    std::fs::write(root.path().join("appcache").join("appinfo.vdf"), data).unwrap();
    root
}
#[test]
fn a_steam_game_starts_the_exe_of_its_windows_launch_option() {
    let steam = steam_with_launch_options(&[
        option("0", "AC4BFSP.app", "default", &[("oslist", "macos")]),
        option("1", "bin32\\AC4BFSP.exe", "default", &[("osarch", "32")]),
        option("2", "Launcher.exe", "option1", &[]),
        option(
            "3",
            "beta\\AC4BFSP.exe",
            "default",
            &[("betakey", "public_test")],
        ),
        option(
            "4",
            "bin/AC4BFSP.exe",
            "default",
            &[("oslist", "windows"), ("osarch", "64")],
        ),
    ]);
    let roots = [steam.path().to_path_buf()];
    let game = steam
        .path()
        .join("steamapps")
        .join("common")
        .join("Black Flag");
    assert_eq!(
        game_executable(242050, &roots).unwrap(),
        Launch {
            exe: game.join("bin").join("AC4BFSP.exe"),
            install_dir: game.clone(),
            arguments: String::new(),
            vac: false,
        }
    );
    assert_eq!(
        game_executable(7, &roots).unwrap_err(),
        "Steam app 7 is not installed"
    );
}
#[test]
fn a_steam_launch_options_arguments_come_with_its_exe() {
    // Portal starts hl2.exe -game portal: without them it is Half-Life 2.
    let mut launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let end = launch.len() - 1;
    launch.splice(end..end, text("arguments", " -game portal "));
    let steam = steam_with_launch_options(&[launch]);
    let found = game_executable(242050, &[steam.path().to_path_buf()]).unwrap();
    assert_eq!(found.arguments, "-game portal");
}
#[test]
fn a_steam_game_with_valve_anti_cheat_says_so() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let vac = steam_with_appinfo(
        std::slice::from_ref(&launch),
        &[object(
            "category",
            &[text("category_2", "1"), text("category_8", "1")],
        )],
    );
    assert!(
        game_executable(242050, &[vac.path().to_path_buf()])
            .unwrap()
            .vac
    );
    let other = steam_with_appinfo(&[launch], &[object("category", &[text("category_2", "1")])]);
    assert!(
        !game_executable(242050, &[other.path().to_path_buf()])
            .unwrap()
            .vac
    );
}
/// A link's app and an .exe in a Steam library both answer with their
/// app's flag; a plain folder has none, and a cache that can't say is
/// an error.
#[test]
fn valve_anti_cheat_is_read_for_a_link_and_for_an_exe_in_a_library() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let category = |flag| object("category", &[text("category_8", flag)]);
    let vac = steam_with_appinfo(std::slice::from_ref(&launch), &[category("1")]);
    let clean = steam_with_appinfo(&[launch], &[category("0")]);
    let exe = |steam: &tempfile::TempDir| {
        steam
            .path()
            .join(r"steamapps\common\Black Flag\bin\AC4BFSP.exe")
    };
    for (steam, listed) in [(&vac, true), (&clean, false)] {
        let roots = [steam.path().to_path_buf()];
        let plain = steam.path().join(r"Games\Game.exe");
        assert_eq!(
            valve_anti_cheat(Some(242050), &plain, "", &roots),
            Ok(listed)
        );
        assert_eq!(valve_anti_cheat(None, &exe(steam), "", &roots), Ok(listed));
        assert_eq!(valve_anti_cheat(None, &plain, "", &roots), Ok(false));
    }
    assert_eq!(
        valve_anti_cheat(Some(7), &exe(&vac), "", &[vac.path().to_path_buf()]),
        Err("Steam's app cache does not list app 7: start it once in Steam".into())
    );
}
#[test]
fn an_exe_started_directly_in_a_steam_library_is_its_apps() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let steam = steam_with_appinfo(
        std::slice::from_ref(&launch),
        &[object("category", &[text("category_8", "1")])],
    );
    let roots = [steam.path().to_path_buf()];
    let common = steam.path().join("steamapps").join("common");
    // The folder in another case than the manifest's.
    let exe = common.join("BLACK FLAG").join("bin").join("AC4BFSP.exe");
    assert_eq!(
        library_app(&exe, "", &roots).unwrap(),
        Some(LibraryApp {
            appid: 242050,
            install_dir: common.join("Black Flag"),
            vac: true,
        })
    );
    let other = steam_with_appinfo(&[launch], &[]);
    let exe = other
        .path()
        .join(r"steamapps\common\Black Flag\AC4BFSP.exe");
    let others = [other.path().to_path_buf()];
    let found = library_app(&exe, "", &others).unwrap();
    assert!(!found.unwrap().vac);
    // Like a link: a game the app cache does not list is refused.
    std::fs::remove_file(other.path().join(r"appcache\appinfo.vdf")).unwrap();
    assert_eq!(
        library_app(&exe, "", &others).unwrap_err(),
        "Steam's app cache does not list app 242050: start it once in Steam"
    );
    // An .exe right in `common`, or none in a library: no Steam app.
    for exe in [
        common.join("Black Flag.exe"),
        steam.path().join("Black Flag").join("AC4BFSP.exe"),
    ] {
        assert_eq!(
            library_app(&exe, "", &roots).unwrap(),
            None,
            "{}",
            exe.display()
        );
    }
    // A folder in a listed library that no manifest names is refused.
    std::fs::create_dir_all(common.join("Mod")).unwrap();
    let error = library_app(&common.join(r"Mod\Mod.exe"), "", &roots).unwrap_err();
    assert!(
        error.ends_with("but no Steam app there installs to Mod: verify the game's files in Steam"),
        "{error}"
    );
}
#[test]
fn a_steam_apps_files_in_a_library_no_steam_lists_are_refused() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let steam = steam_with_appinfo(&[launch], &[object("category", &[text("category_8", "1")])]);
    let exe = steam
        .path()
        .join(r"steamapps\common\Black Flag\bin\AC4BFSP.exe");
    // Its manifest names the folder, but no Steam is found: the Valve
    // Anti-Cheat flag can't be read, so it never streams as a plain .exe.
    let error = library_app(&exe, "", &[]).unwrap_err();
    assert!(
        error.contains(
            "is Steam app 242050's, but no Steam installation on this PC lists its library"
        ),
        "{error}"
    );
    // Nor beside a Steam installation that doesn't list it.
    let installed = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(installed.path().join("steamapps")).unwrap();
    assert!(library_app(&exe, "", &[installed.path().to_path_buf()]).is_err());
    // A `steamapps` no installation lists and no manifest names the
    // folder of is a plain folder.
    let plain = tempfile::tempdir().unwrap();
    let game = plain.path().join(r"steamapps\common\Puzzle");
    std::fs::create_dir_all(&game).unwrap();
    assert_eq!(
        library_app(&game.join("Puzzle.exe"), "", &[]).unwrap(),
        None
    );
}
#[cfg(windows)]
#[test]
fn a_library_spelled_another_way_is_the_one_steam_lists() {
    let launch = option("0", r"bin\AC4BFSP.exe", "default", &[]);
    let steam = steam_with_launch_options(&[launch]);
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("Link");
    let made = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(steam.path())
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let exe = link.join(r"steamapps\common\Black Flag\bin\AC4BFSP.exe");
    let found = library_app(&exe, "", &[steam.path().to_path_buf()]).unwrap();
    assert_eq!(found.unwrap().appid, 242050);
    std::fs::remove_dir(&link).unwrap();
}
/// A Steam folder with apps that all install to `Half-Life 2`, each with
/// its type, launch options (executable, arguments) and VAC flag.
/// An app: its id, type, launch options (executable, arguments) and
/// whether Steam lists Valve Anti-Cheat for it.
type TestApp<'a> = (u32, &'a str, &'a [(&'a str, &'a str)], bool);
fn half_life_2(apps: &[TestApp]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let steamapps = root.path().join("steamapps");
    std::fs::create_dir_all(steamapps.join(r"common\Half-Life 2")).unwrap();
    let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    for (appid, kind, options, vac) in apps {
        std::fs::write(
            steamapps.join(format!("appmanifest_{appid}.acf")),
            format!("\"AppState\" {{ \"appid\" \"{appid}\" \"installdir\" \"Half-Life 2\" }}"),
        )
        .unwrap();
        let mut common = vec![text("type", kind)];
        if *vac {
            common.push(object("category", &[text("category_8", "1")]));
        }
        let options: Vec<Vec<u8>> = options
            .iter()
            .enumerate()
            .map(|(at, (exe, arguments))| {
                object(
                    &at.to_string(),
                    &[
                        text("executable", exe),
                        text("arguments", arguments),
                        text("type", "default"),
                    ],
                )
            })
            .collect();
        let mut kv = object(
            "appinfo",
            &[
                object("common", &common),
                object("config", &[object("launch", &options)]),
            ],
        );
        kv.extend([0x08, 0x08]);
        data.extend(binary_entry(*appid, &kv));
    }
    data.extend(0u32.to_le_bytes());
    std::fs::create_dir_all(root.path().join("appcache")).unwrap();
    std::fs::write(root.path().join(r"appcache\appinfo.vdf"), data).unwrap();
    root
}
#[test]
fn half_life_2_and_its_episodes_in_one_folder_are_told_apart() {
    let steam = half_life_2(&[
        (220, "Game", &[("hl2.exe", "")], false),
        (340, "Game", &[("hl2.exe", "-game lostcoast")], false),
        (380, "Game", &[("hl2.exe", "-game episodic")], false),
        (420, "Game", &[("hl2.exe", "-game ep2")], false),
        (218, "Tool", &[("hl2.exe", "")], false),
    ]);
    let roots = [steam.path().to_path_buf()];
    let exe = steam.path().join(r"steamapps\common\Half-Life 2\hl2.exe");
    let app = |arguments: &str| library_app(&exe, arguments, &roots).unwrap().unwrap();
    assert_eq!(app("").appid, 220);
    assert_eq!(app("-novid").appid, 220);
    assert_eq!(app("-game episodic -novid").appid, 380);
    assert_eq!(app("-GAME ep2").appid, 420);
    // Apps that can't be told apart: the lowest stands for them, with
    // Valve Anti-Cheat when any has it.
    let steam = half_life_2(&[
        (220, "Game", &[("hl2.exe", "")], false),
        (221, "Game", &[(r".\hl2.exe", "")], true),
    ]);
    let exe = steam.path().join(r"steamapps\common\Half-Life 2\hl2.exe");
    let found = library_app(&exe, "", &[steam.path().to_path_buf()])
        .unwrap()
        .unwrap();
    assert_eq!((found.appid, found.vac), (220, true));
    // Several apps there and none starts the .exe: refused.
    let other = steam.path().join(r"steamapps\common\Half-Life 2\other.exe");
    assert_eq!(
        library_app(&other, "", &[steam.path().to_path_buf()]).unwrap_err(),
        "Steam apps 220, 221 install to Half-Life 2, and none of them starts other.exe: start the game from its Steam link"
    );
}
#[test]
fn valve_anti_cheat_of_any_app_starting_the_exe_is_never_narrowed_away() {
    // The .exe loads Counter-Strike's game with `-game cstrike`, whichever
    // app id stands for it.
    let steam = half_life_2(&[
        (70, "Game", &[("hl.exe", "")], false),
        (10, "Game", &[("hl.exe", "-game cstrike")], true),
        (90, "Game", &[("other.exe", "")], true),
    ]);
    let roots = [steam.path().to_path_buf()];
    let exe = steam.path().join(r"steamapps\common\Half-Life 2\hl.exe");
    let app = |arguments: &str| {
        let found = library_app(&exe, arguments, &roots).unwrap().unwrap();
        (found.appid, found.vac)
    };
    // Quoted, the argument still names the anti-cheat app.
    assert_eq!(app(r#"-game "cstrike""#), (10, true));
    // The argument match drops it, and its anti-cheat stays.
    assert_eq!(app("-novid"), (70, true));
    // The type filter drops it, and its anti-cheat stays.
    let steam = half_life_2(&[
        (70, "Game", &[("hl.exe", "")], false),
        (80, "Tool", &[("hl.exe", "")], true),
    ]);
    let found = library_app(&exe_in(&steam, "hl.exe"), "", &[steam.path().to_path_buf()])
        .unwrap()
        .unwrap();
    assert_eq!((found.appid, found.vac), (70, true));
    // An app that starts another .exe in the folder says nothing about
    // this one.
    let steam = half_life_2(&[
        (70, "Game", &[("hl.exe", "")], false),
        (90, "Game", &[("other.exe", "")], true),
    ]);
    let found = library_app(&exe_in(&steam, "hl.exe"), "", &[steam.path().to_path_buf()])
        .unwrap()
        .unwrap();
    assert_eq!((found.appid, found.vac), (70, false));
}
fn exe_in(steam: &tempfile::TempDir, name: &str) -> PathBuf {
    steam
        .path()
        .join(r"steamapps\common\Half-Life 2")
        .join(name)
}
#[test]
fn a_library_folder_is_read_from_the_exes_path_on_this_pc_only() {
    let found = |exe: &str| library_folder(Path::new(exe));
    assert_eq!(
        found(r"\\?\D:\SteamLibrary\SteamApps\Common\Game\bin\g.exe"),
        Some((
            PathBuf::from(r"D:\SteamLibrary\SteamApps"),
            "Game".into(),
            r"bin\g.exe".into()
        ))
    );
    assert_eq!(
        found("D:/Steam/steamapps/common/Game/g.exe"),
        Some((
            PathBuf::from(r"D:\Steam\steamapps"),
            "Game".into(),
            "g.exe".into()
        ))
    );
    for exe in [
        r"\\nas\games\steamapps\common\Game\g.exe",
        r"\\?\UNC\nas\games\steamapps\common\Game\g.exe",
        r"\\?\GLOBALROOT\Device\Mup\nas\steamapps\common\Game\g.exe",
        r"\\.\pipe\steamapps\common\Game\g.exe",
        r"D:\Steam\steamapps\common\Game.exe",
        r"D:\Steam\steamapps\common\Game\..\..\..\Other\g.exe",
        r"D:\Games\Game\g.exe",
    ] {
        assert_eq!(found(exe), None, "{exe}");
    }
}
#[test]
fn a_library_on_a_network_share_is_never_opened() {
    let steam = tempfile::tempdir().unwrap();
    let apps = steam.path().join("steamapps");
    std::fs::create_dir_all(&apps).unwrap();
    // A library only a share path reaches, and one on a drive.
    let reached = tempfile::tempdir().unwrap();
    let reached_apps = reached.path().join("steamapps");
    std::fs::create_dir_all(reached_apps.join("common").join("Portal 2")).unwrap();
    std::fs::write(
        reached_apps.join("appmanifest_620.acf"),
        "\"AppState\" { \"appid\" \"620\" \"name\" \"Portal 2\" \"installdir\" \"Portal 2\" }",
    )
    .unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(local.path().join("steamapps")).unwrap();
    let globalroot = format!(r"\\?\GLOBALROOT\GLOBAL??\{}", reached.path().display());
    // NT's namespace, which Windows resolves as well.
    let nt = format!(r"\??\{}", reached.path().display());
    if cfg!(windows) {
        // The paths reach the library: skipping them is what keeps it out.
        assert!(steamapps(Path::new(&globalroot)).is_some());
        assert!(steamapps(Path::new(&nt)).is_some());
    }
    let listed = [
        r"\\nas\games".to_string(),
        r"\\?\UNC\nas\games".into(),
        r"\\.\UNC\nas\games".into(),
        r"\??\UNC\nas\games".into(),
        globalroot,
        nt,
        local.path().display().to_string(),
    ];
    let folders: String = listed
        .iter()
        .enumerate()
        .map(|(at, path)| {
            format!(
                "\"{at}\" {{ \"path\" \"{}\" }}\n",
                path.replace('\\', "\\\\")
            )
        })
        .collect();
    std::fs::write(
        apps.join("libraryfolders.vdf"),
        format!("\"libraryfolders\" {{\n{folders}}}"),
    )
    .unwrap();
    let roots = [steam.path().to_path_buf()];
    assert_eq!(libraries(&roots), [apps, local.path().join("steamapps")]);
    assert_eq!(
        game_executable(620, &roots).unwrap_err(),
        "Steam app 620 is not installed"
    );
    assert!(discover(&roots).is_empty());
    // A Steam folder on a share is no root either.
    assert!(libraries(&[PathBuf::from(r"\\nas\Steam")]).is_empty());
}
#[test]
fn a_steam_launch_option_outside_its_folder_or_missing_is_refused() {
    let steam = steam_with_launch_options(&[option("0", "..\\Other\\Other.exe", "", &[])]);
    let roots = [steam.path().to_path_buf()];
    let error = game_executable(242050, &roots).unwrap_err();
    assert!(error.contains("outside its folder"), "{error}");
    let steam = steam_with_launch_options(&[option("0", "Gone.exe", "", &[])]);
    let error = game_executable(242050, &[steam.path().to_path_buf()]).unwrap_err();
    assert!(error.contains("Gone.exe is missing"), "{error}");
    let steam = steam_with_launch_options(&[option("0", "run.sh", "", &[])]);
    let error = game_executable(242050, &[steam.path().to_path_buf()]).unwrap_err();
    assert!(
        error.contains("no launch option that starts an .exe"),
        "{error}"
    );
    // Names and types still read from the same cache.
    let data = std::fs::read(steam.path().join("appcache").join("appinfo.vdf")).unwrap();
    assert_eq!(
        appinfo(&data, &BTreeSet::from([242050]))[&242050],
        (String::new(), String::new())
    );
}
fn game(appid: u32, name: &str, last_played: u64) -> Game {
    Game {
        appid,
        name: name.into(),
        installed: true,
        last_played,
        install_dir: PathBuf::from(format!("C:/Steam/steamapps/common/{name}")),
        ..Default::default()
    }
}
#[test]
fn every_installed_game_is_the_default_and_an_explicit_false_keeps_recent() {
    assert!(Settings::from_config(&Config::default()).sync_all_installed);
    assert_eq!(
        Settings::from_config(&Config::default()).source(),
        "installed"
    );
    let recent = Settings::from_config(&Config::parse("steam_sync_all_installed = false").unwrap());
    assert!(!recent.sync_all_installed);
    assert_eq!(recent.source(), "recent");
}
#[test]
fn selection_and_identity_follow_vibepollo() {
    assert_eq!(canonical_uuid(570), "53544541-4d00-5000-8000-00000000023a");
    assert_eq!(app_id_of(&json!({"uuid": canonical_uuid(570)})), Some(570));
    assert_eq!(app_id_of(&json!({"steam-id": "steam-730"})), Some(730));
    let games = vec![
        game(1, "Old", 100),
        game(2, "New", 300),
        game(3, "Never", 0),
        game(228980, "Steamworks Common Redistributables", 400),
        Game {
            app_type: "Tool".into(),
            ..game(4, "SDK", 500)
        },
    ];
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = false;
    settings.recent_max_age_days = 0;
    let ids = |s: &Settings| {
        select(&games, s, 1000)
            .iter()
            .map(|g| g.appid)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&settings), [2, 1]);
    settings.recent_games = 1;
    assert_eq!(ids(&settings), [2]);
    settings.recent_games = 10;
    settings.exclusions = id_names(r#"[{"id":"","name":"new"}]"#);
    assert_eq!(ids(&settings), [1]);
    settings.sync_all_installed = true;
    settings.exclusions = id_names("1");
    assert_eq!(ids(&settings), [2, 3]);
    settings.include_tools = true;
    assert_eq!(ids(&settings), [2, 3, 228980, 4]);
}
#[test]
fn reconcile_owns_only_managed_apps() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    settings.remove_uninstalled = true;
    let dota = game(570, "Dota 2", 10);
    let cs = game(730, "Counter-Strike 2", 20);
    let mut apps = vec![
        json!({"name":"Desktop"}),
        // Added by hand: only its missing cover is filled in.
        json!({"name":"My CS","steam-id":"730","cmd":"custom","image-path":""}),
        // Synced but no longer installed.
        json!({"name":"Gone","uuid":canonical_uuid(440),"steam-id":"440","steam-managed":"auto","synced-by":"steam"}),
    ];
    let covers = HashMap::from([(730, PathBuf::from("C:\\covers\\steam_730.png"))]);
    assert!(reconcile(&mut apps, &[&dota, &cs], &settings, &covers, &[]));
    let names: Vec<_> = apps.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Desktop", "My CS", "Dota 2"]);
    assert_eq!(apps[1]["cmd"], "custom");
    assert_eq!(apps[1]["image-path"], "C:/covers/steam_730.png");
    assert_eq!(apps[2]["uuid"], canonical_uuid(570));
    assert_eq!(apps[2]["cmd"], "cmd /c start \"\" steam://rungameid/570");
    assert_eq!(apps[2]["image-path"], FALLBACK_IMAGE);
    assert_eq!(
        apps[2]["steam-install-dir"],
        "C:/Steam/steamapps/common/Dota 2"
    );
    // A second pass changes nothing.
    assert!(!reconcile(
        &mut apps,
        &[&dota, &cs],
        &settings,
        &covers,
        &[]
    ));
    // A cover the user picked stays.
    apps[2]["image-path"] = json!("C:/mine.png");
    assert!(!reconcile(
        &mut apps,
        &[&dota, &cs],
        &settings,
        &covers,
        &[]
    ));
    assert_eq!(apps[2]["image-path"], "C:/mine.png");
}
/// Every sync that could rename, remove or duplicate `apps`: the game installed and
/// selected, excluded, uninstalled, and outside the recent window. Each leaves them as they are.
fn untouched_by_every_sync(apps: Vec<Value>, appid: u32) {
    let portal = game(appid, "Portal 2", 1000);
    let mut installed = Settings::from_config(&Config::default());
    installed.sync_all_installed = true;
    installed.remove_uninstalled = true;
    let mut excluded = installed.clone();
    excluded.exclusions = id_names(&appid.to_string());
    let mut recent = Settings::from_config(&Config::default());
    recent.sync_all_installed = false;
    recent.recent_games = 1;
    for (case, settings, selected) in [
        ("installed", &installed, vec![&portal]),
        ("excluded", &excluded, vec![&portal]),
        ("uninstalled", &installed, vec![]),
        ("outside the recent window", &recent, vec![]),
    ] {
        let mut synced = apps.clone();
        assert!(
            !reconcile(&mut synced, &selected, settings, &HashMap::new(), &[]),
            "{case}"
        );
        assert_eq!(synced, apps, "{case}");
    }
}
#[test]
fn a_legacy_steam_uuid_app_keeps_its_name_command_and_detach_through_every_sync() {
    let mine = json!({"name":"My Portal","uuid":canonical_uuid(620),"cmd":"C:/portal.bat",
            "auto-detach":false,"image-path":"C:/portal.png"});
    // A second copy, as another host left it: both are the user's.
    let copy = json!({"name":"Portal (couch)","uuid":canonical_uuid(620),"cmd":"C:/couch.bat",
            "image-path":"C:/couch.png"});
    untouched_by_every_sync(vec![mine, copy], 620);
}
#[test]
fn an_unstamped_vibepollo_synced_app_is_the_users() {
    let vibepollo = json!({"name":"Portal 2 (mine)","uuid":canonical_uuid(620),"steam-id":"620",
            "steam-managed":"auto","steam-source":"recent","cmd":"C:/portal.bat","auto-detach":false,
            "image-path":"C:/portal.png"});
    untouched_by_every_sync(vec![vibepollo.clone(), vibepollo], 620);
}
#[test]
fn no_second_app_joins_a_users_app_for_the_same_game() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    let games = [
        game(570, "Dota 2", 10),
        game(620, "Portal 2", 10),
        game(730, "Counter-Strike 2", 10),
        game(440, "Team Fortress 2", 10),
    ];
    let selected: Vec<&Game> = games.iter().collect();
    let mut apps = vec![
        json!({"name":"Dota","steam-id":"570"}),
        // An "Add games" import and a hand-made app that starts the game through Steam.
        json!({"name":"Portal","launcher-source":"steam","launcher-id":"620","cmd":"C:/portal.bat"}),
        json!({"name":"CS","cmd":"cmd /c start steam://rungameid/730"}),
        // The sync's own copy of a game the user also has: the user's stays, alone.
        json!({"name":"Dota 2","uuid":canonical_uuid(570),"steam-id":"570","steam-managed":"auto","synced-by":"steam"}),
    ];
    assert!(reconcile(
        &mut apps,
        &selected,
        &settings,
        &HashMap::new(),
        &[]
    ));
    let names: Vec<_> = apps.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Dota", "Portal", "CS", "Team Fortress 2"]);
    assert_eq!(game_of(&apps[2]), Some(730));
}
#[test]
fn the_sync_updates_and_removes_only_the_apps_it_stamped() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    settings.remove_uninstalled = true;
    let dota = game(570, "Dota 2", 10);
    let mut apps = vec![
        json!({"name":"Old name","uuid":canonical_uuid(570),"steam-id":"570",
            "steam-managed":"auto","synced-by":"steam","cmd":"old","auto-detach":false}),
    ];
    assert!(reconcile(
        &mut apps,
        &[&dota],
        &settings,
        &HashMap::new(),
        &[]
    ));
    assert_eq!(apps[0]["name"], "Dota 2");
    assert_eq!(apps[0]["cmd"], launch_command(570));
    assert_eq!(apps[0]["auto-detach"], true);
    assert_eq!(apps[0]["synced-by"], "steam");
    assert!(reconcile(&mut apps, &[], &settings, &HashMap::new(), &[]));
    assert!(apps.is_empty(), "uninstalled");
    // In recent mode a game that left the window takes only the stamped app with it.
    let mut recent = Settings::from_config(&Config::default());
    recent.sync_all_installed = false;
    recent.recent_games = 1;
    let stamped = json!({"name":"Dota 2","uuid":canonical_uuid(570),"steam-id":"570",
            "steam-managed":"auto","synced-by":"steam"});
    let unstamped = json!({"name":"Portal 2","uuid":canonical_uuid(620),"steam-id":"620",
            "steam-managed":"auto"});
    let mut apps = vec![stamped, unstamped.clone()];
    assert!(reconcile(&mut apps, &[], &recent, &HashMap::new(), &[]));
    assert_eq!(apps, [unstamped]);
}
#[test]
fn a_steam_game_deleted_from_the_library_is_not_added_again() {
    let mut settings = Settings::from_config(&Config::default());
    settings.sync_all_installed = true;
    let (dota, cs) = (game(570, "Dota 2", 10), game(730, "Counter-Strike 2", 10));
    let mut apps = vec![];
    assert!(reconcile(
        &mut apps,
        &[&dota, &cs],
        &settings,
        &HashMap::new(),
        &["steam:570".into()]
    ));
    let names: Vec<_> = apps.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Counter-Strike 2"]);
    // Any app that starts a Steam game dismisses it when deleted, whoever made it.
    assert_eq!(library::dismissal_keys(&apps[0]), ["steam:730"]);
    assert_eq!(
        library::dismissal_keys(&json!({"name":"Mine","cmd":"steam://rungameid/620"})),
        ["steam:620"]
    );
    assert_eq!(
        library::dismissal_keys(
            &json!({"launcher-source":"steam","launcher-id":"440","steam-id":"440"})
        ),
        ["steam:440"]
    );
}
#[test]
fn tracker_follows_the_game_and_its_children() {
    let process = |pid, parent, name: &str| Process {
        pid,
        parent,
        started: u64::from(pid) * 10,
        name: name.into(),
    };
    let before = [process(1, 0, "explorer.exe"), process(2, 1, "steam.exe")];
    let mut tracker = Tracker::new(
        &before,
        "C:\\Games\\Portal",
        std::time::Duration::from_secs(15),
    )
    .with_exit_grace(std::time::Duration::ZERO);
    let images = |pid| match pid {
        3 => Some("c:\\games\\portal\\portal.exe".to_owned()),
        5 => Some("C:\\Games\\Portal2\\other.exe".to_owned()),
        _ => None,
    };
    assert_eq!(tracker.update(&before, images), Tracked::Waiting);
    let launched = [
        process(1, 0, "explorer.exe"),
        process(2, 1, "steam.exe"),
        process(3, 2, "portal.exe"),
        process(4, 3, "crashhandler.exe"),
        process(5, 1, "other.exe"),
    ];
    assert_eq!(tracker.update(&launched, images), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [3, 4]);
    assert_eq!(tracker.update(&before, images), Tracked::Exited);
}
#[test]
fn the_paths_an_update_asks_for_are_the_ones_listed_before_it() {
    let process = |pid, parent, started, name: &str| Process {
        pid,
        parent,
        started,
        name: name.into(),
    };
    let before = [
        process(1, 0, 10, "explorer.exe"),
        process(2, 1, 20, "steam.exe"),
    ];
    let mut tracker = Tracker::new(
        &before,
        "C:\\Games\\Portal",
        std::time::Duration::from_secs(15),
    );
    let images = |pid| (pid == 3).then(|| "c:\\games\\portal\\portal.exe".to_owned());
    let rounds = [
        vec![
            process(1, 0, 10, "explorer.exe"),
            process(2, 1, 20, "steam.exe"),
            process(3, 2, 30, "portal.exe"),
            process(4, 3, 40, "crashhandler.exe"),
            process(5, 1, 50, "other.exe"),
            process(6, 1, 60, "chrome.exe"),
        ],
        // Looked at once: the same processes are not asked about again,
        // while a new one and a reused id are.
        vec![
            process(1, 0, 10, "explorer.exe"),
            process(3, 2, 30, "portal.exe"),
            process(5, 1, 50, "other.exe"),
            process(7, 1, 70, "notes.exe"),
            process(4, 1, 80, "reused.exe"),
        ],
    ];
    let mut listed = vec![];
    for now in rounds {
        let unchecked = tracker.unchecked(&now);
        let mut asked = vec![];
        tracker.update(&now, |pid| {
            asked.push(pid);
            images(pid)
        });
        assert!(
            asked.iter().all(|pid| unchecked.contains(pid)),
            "{asked:?} not in {unchecked:?}"
        );
        listed.push(unchecked);
    }
    assert_eq!(listed, [vec![3, 4, 5], vec![7, 4]]);
}
#[test]
fn a_game_restarting_through_steam_and_a_browser_it_opens_do_not_end_or_extend_it() {
    let process = |pid, parent, name: &str| Process {
        pid,
        parent,
        started: u64::from(pid) * 10,
        name: name.into(),
    };
    let before = [process(1, 0, "explorer.exe"), process(2, 1, "steam.exe")];
    let images = |pid| match pid {
        3 | 6 => Some("c:\\games\\portal\\portal.exe".to_owned()),
        _ => None,
    };
    let mut tracker = Tracker::new(
        &before,
        "C:\\Games\\Portal",
        std::time::Duration::from_secs(15),
    );
    let running = [
        process(1, 0, "explorer.exe"),
        process(2, 1, "steam.exe"),
        process(3, 2, "portal.exe"),
        process(4, 3, "chrome.exe"),
    ];
    assert_eq!(tracker.update(&running, images), Tracked::Running);
    // The browser the game opened is not the game.
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [3]);
    // Restarting through Steam: no game process for a moment.
    assert_eq!(tracker.update(&before, images), Tracked::Running);
    let restarted = [
        process(1, 0, "explorer.exe"),
        process(2, 1, "steam.exe"),
        process(6, 2, "portal.exe"),
    ];
    assert_eq!(tracker.update(&restarted, images), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [6]);
}
#[test]
fn launcher_handoff_never_owns_shared_launchers_even_inside_the_game_folder() {
    let process = |pid, parent, name: &str| Process {
        pid,
        parent,
        started: u64::from(pid),
        name: name.into(),
    };
    let mut tracker = Tracker::new(
        &[],
        r"E:\Games\Diablo IV",
        std::time::Duration::from_secs(15),
    );
    let launchers = [
        process(1, 0, "Battle.net Launcher.exe"),
        process(2, 1, "Agent.exe"),
    ];
    let image = |_| Some(r"E:\Games\Diablo IV\program.exe".into());
    assert_eq!(tracker.update(&launchers, image), Tracked::Waiting);
    assert!(tracker.tracked.is_empty());
    let started = [
        launchers[0].clone(),
        launchers[1].clone(),
        process(3, 1, "Diablo IV.exe"),
        process(4, 3, "Battle.net.exe"),
    ];
    assert_eq!(tracker.update(&started, image), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [3]);
    assert_eq!(
        tracker.update(&launchers, image),
        Tracked::Running,
        "launcher handoff gets the existing restart grace period"
    );
    let restarted = [process(5, 1, "Diablo IV.exe")];
    assert_eq!(tracker.update(&restarted, image), Tracked::Running);
    assert_eq!(tracker.tracked.keys().copied().collect::<Vec<_>>(), [5]);
    tracker.grace = std::time::Duration::ZERO;
    assert_eq!(tracker.update(&launchers, image), Tracked::Exited);
    assert!(tracker.tracked.is_empty());
}
