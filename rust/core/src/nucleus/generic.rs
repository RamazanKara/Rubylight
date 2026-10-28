//! Rubylight's own one-player Nucleus Co-op handler, for background streams
//! of games no hub handler covers.
//!
//! Nucleus Co-op is the only way a game streams in the background, and it
//! starts a game only through a handler. For a one-player game without one
//! the host writes this handler into Nucleus' `handlers` folder and adds the
//! game to the user's `userprofile.json`, the way Nucleus' own "add game"
//! does. Co-op still needs a hub handler, and a hub handler for the same
//! executable always wins: [`super::handler::Catalog`] skips ours.
//!
//! Every `Game.*` option below is one Nucleus Co-op 2.4.2 reads
//! (`GenericGameInfo.cs`, `GameHookInfo.cs`, `ProtoInputOptions.cs`).
use sha2::{Digest, Sha256};
use std::path::Path;

/// The start of our handlers' GUIDs and file names.
pub const PREFIX: &str = "Rubylight-";
/// The first line of every handler Rubylight writes: the file is ours to
/// rewrite. A file without it is never touched.
pub const MARKER: &str = "// Rubylight one-player handler";
/// Bumped when the template changes; a handler with another version is
/// rewritten.
const VERSION: u32 = 1;

/// Whether a handler's text is one Rubylight wrote.
pub fn owns(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').starts_with(MARKER)
}

fn normalized(path: &str) -> String {
    path.trim().replace('/', "\\")
}
fn parent(path: &str) -> &str {
    path.rfind('\\').map_or("", |at| &path[..at])
}
fn file_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}
/// `path` below `folder`, as a relative path; None when it is not below.
fn below<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    let folder = folder.trim_end_matches('\\');
    let rest = path.get(folder.len()..)?;
    (!folder.is_empty() && path[..folder.len()].eq_ignore_ascii_case(folder))
        .then_some(rest)
        .and_then(|rest| rest.strip_prefix('\\'))
}

/// The game a handler is written for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The game's .exe, as the app starts it.
    pub exe_path: String,
    /// The app's name, which Nucleus shows.
    pub name: String,
    /// The game's folder, relative to which the .exe sits in
    /// [`Setup::binaries`]; Nucleus links all of it into its instance.
    pub root: String,
    /// The .exe's folder below `root`, or None when it is `root`.
    pub binaries: Option<String>,
    pub steam_app: Option<u32>,
}

impl Setup {
    /// The setup for `exe_path`. `steam` is the game's Steam app and install
    /// folder, when Steam installed it.
    pub fn new(exe_path: &str, name: &str, steam: Option<(u32, &str)>) -> Self {
        let exe_path = normalized(exe_path);
        let exe_folder = parent(&exe_path).to_string();
        let steam_root = steam.and_then(|(_, folder)| {
            let folder = normalized(folder);
            below(&exe_path, &folder).map(|_| folder)
        });
        let root = steam_root
            .or_else(|| unreal_root(&exe_folder))
            .unwrap_or_else(|| exe_folder.clone());
        let binaries = below(&exe_folder, &root)
            .filter(|rel| !rel.is_empty())
            .map(str::to_string);
        // Nucleus finds the root by cutting the first occurrence of
        // `BinariesFolder` out of the exe's folder (StringUtil
        // .ReplaceCaseInsensitive); where that would cut elsewhere, the
        // instance gets the exe's folder alone.
        let (root, binaries) = match binaries {
            Some(rel)
                if exe_folder.to_lowercase().find(&rel.to_lowercase())
                    == Some(exe_folder.len() - rel.len()) =>
            {
                (root, Some(rel))
            }
            _ => (exe_folder, None),
        };
        Self {
            exe_path,
            name: name.trim().to_string(),
            root,
            binaries,
            steam_app: steam.map(|(app, _)| app),
        }
    }

    /// `Game.GUID`: stable for the .exe's path, a folder name for Nucleus'
    /// profiles and instances, and a single command-line word.
    pub fn guid(&self) -> String {
        let stem = file_name(&self.exe_path);
        let stem = stem
            .rsplit_once('.')
            .map_or(stem, |(stem, _)| stem)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .take(40)
            .collect::<String>();
        let hash = Sha256::digest(self.exe_path.to_lowercase().as_bytes());
        format!("{PREFIX}{stem}-{}", hex::encode(&hash[..4]))
    }

    /// The handler's file name in Nucleus' `handlers` folder.
    pub fn file(&self) -> String {
        format!("{}.js", self.guid())
    }

    /// The handler's text.
    pub fn handler(&self) -> String {
        let name = if self.name.is_empty() {
            file_name(&self.exe_path)
        } else {
            &self.name
        };
        let mut lines = vec![
            format!(
                "{MARKER} v{VERSION}: Rubylight writes this file for background streams and rewrites it when it changes. A hub handler for this game takes its place."
            ),
            format!("Game.GUID = {};", js(&self.guid())),
            format!("Game.GameName = {};", js(name)),
            format!(
                "Game.ExecutableName = {};",
                js(file_name(&self.exe_path))
            ),
            "Game.MaxPlayers = 1;".into(),
            "Game.MaxPlayersOneMonitor = 1;".into(),
            "// The slot's pad only: keyboard and mouse stay with the person at the PC, and so does the taskbar.".into(),
            "Game.SupportsKeyboard = false;".into(),
            "Game.HideTaskbar = false;".into(),
            "// The instance runs from links in Nucleus' content folder, so the game's own folder stays as it is.".into(),
            "Game.SymlinkGame = true;".into(),
            "Game.SymlinkExe = false;".into(),
        ];
        if let Some(binaries) = &self.binaries {
            lines.push(format!("Game.BinariesFolder = {};", js(binaries)));
        }
        if let Some(app) = self.steam_app {
            lines.push(
                "// steam_appid.txt beside the linked .exe keeps the game from restarting through Steam."
                    .into(),
            );
            lines.push(format!("Game.SteamID = {};", js(&app.to_string())));
            lines.push("Game.CreateSteamAppIdByExe = true;".into());
        }
        lines.extend(
            [
                "// The game keeps running and reading its pad without focus, and never sees the PC's keyboard or moves its cursor.",
                "Game.FakeFocus = true;",
                "Game.Hook.CustomDllEnabled = false;",
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
            ]
            .map(String::from),
        );
        let mut text = lines.join("\r\n");
        text.push_str("\r\n");
        text
    }
}

/// An Unreal game's folder for an .exe in `<root>\<Project>\Binaries\Win64`:
/// the game's content sits beside `Binaries`.
fn unreal_root(exe_folder: &str) -> Option<String> {
    let platform = file_name(exe_folder);
    let binaries = parent(exe_folder);
    let is = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if !(["Win64", "Win32", "WinGDK"].iter().any(|p| is(platform, p))
        && is(file_name(binaries), "Binaries"))
    {
        return None;
    }
    let root = parent(parent(binaries));
    // Never a drive's root.
    (root.len() > 3 && root.contains('\\')).then(|| root.to_string())
}

/// A JavaScript string literal, in the escapes `handler::parse` reads.
fn js(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars().filter(|c| !c.is_control()) {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The anti-cheat a file or folder name belongs to.
fn anti_cheat_of(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    let starts = |prefix: &str| name.starts_with(prefix);
    let any = |names: &[&str]| names.contains(&name.as_str());
    Some(
        if starts("easyanticheat") || any(&["start_protected_game.exe"]) {
            "Easy Anti-Cheat"
        } else if any(&["battleye"]) || starts("beservice") || starts("beclient") {
            "BattlEye"
        } else if any(&[
            "riot vanguard",
            "vgc.exe",
            "vgk.sys",
            "valorant.exe",
            "valorant-win64-shipping.exe",
            "leagueclient.exe",
            "league of legends.exe",
        ]) {
            "Riot Vanguard"
        } else if any(&["gameguard", "gamemon.des", "gameguard.des"]) {
            "nProtect GameGuard"
        } else if any(&["xigncode", "x3.xem", "xhunter1.sys"]) {
            "XIGNCODE3"
        } else if starts("randgrid") || starts("atvi-randgrid") {
            "Ricochet"
        } else if starts("eaanticheat") {
            "EA Javelin"
        } else if any(&["pnkbstra.exe", "pnkbstrb.exe"]) {
            "PunkBuster"
        } else if starts("mhyprot") || any(&["hoyokprotect.sys"]) {
            "HoYoverse's anti-cheat"
        } else if any(&["anticheatexpert"]) {
            "Anti-Cheat Expert"
        } else if any(&["robloxplayerbeta.exe"]) {
            "Hyperion"
        } else {
            return None;
        },
    )
}

/// The anti-cheat `setup`'s game ships, from the files and folders beside
/// its .exe and in the folders above it, up to the game's root and at least
/// two up (anti-cheat often sits beside a `bin` folder), never a drive's
/// root. Nucleus Co-op injects hooks, which anti-cheat treats as cheating,
/// so such a game is refused.
pub fn anti_cheat(setup: &Setup) -> Option<&'static str> {
    if let Some(found) = anti_cheat_of(file_name(&setup.exe_path)) {
        return Some(found);
    }
    let mut folder = parent(&setup.exe_path);
    let mut up = 0;
    while folder.len() > 3 && (up < 3 || folder.len() >= setup.root.len()) {
        let entries = std::fs::read_dir(Path::new(folder)).into_iter().flatten();
        for entry in entries.flatten() {
            if let Some(found) = anti_cheat_of(&entry.file_name().to_string_lossy()) {
                return Some(found);
            }
        }
        folder = parent(folder);
        up += 1;
    }
    None
}

/// `json` (Nucleus' `userprofile.json`, None when there is none yet) with
/// the game added as Nucleus' "add game" does (`UserGameInfo
/// .InitializeDefault`). Every other byte stays: the user's games, their
/// order and the file's layout. None when the game is already there.
pub fn add_game(json: Option<&str>, guid: &str, exe_path: &str) -> Result<Option<String>, String> {
    let exe_path = exe_path.replace('I', "i");
    let text = json.unwrap_or_default();
    let body = text.trim_start_matches('\u{feff}');
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let entry = |indent: &str| {
        format!(
            "{indent}{{{newline}{indent}  \"GameGuid\": {},{newline}{indent}  \"ExePath\": {}{newline}{indent}}}",
            serde_json::Value::from(guid),
            serde_json::Value::from(exe_path.as_str()),
        )
    };
    if body.trim().is_empty() {
        return Ok(Some(format!(
            "{{{newline}  \"Games\": [{newline}{}{newline}  ]{newline}}}",
            entry("    ")
        )));
    }
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| format!("Nucleus Co-op's userprofile.json is not readable: {e}"))?;
    let object = value
        .as_object()
        .ok_or("Nucleus Co-op's userprofile.json is not an object")?;
    match object.get("Games") {
        Some(serde_json::Value::Array(games)) => {
            if games
                .iter()
                .any(|game| game.get("GameGuid").and_then(|g| g.as_str()) == Some(guid))
            {
                return Ok(None);
            }
            let (open, close) = games_array(text)
                .ok_or("Nucleus Co-op's userprofile.json has an unreadable game list")?;
            let inside = &text[open + 1..close];
            Ok(Some(if inside.trim().is_empty() {
                format!(
                    "{}[{newline}{}{newline}  ]{}",
                    &text[..open],
                    entry("    "),
                    &text[close + 1..]
                )
            } else {
                let last = open + 1 + inside.trim_end().len();
                format!(
                    "{},{newline}{}{}",
                    &text[..last],
                    entry("    "),
                    &text[last..]
                )
            }))
        }
        // Nucleus leaves out an empty list.
        None | Some(serde_json::Value::Null) => {
            let mut object = object.clone();
            object.insert(
                "Games".into(),
                serde_json::json!([{ "GameGuid": guid, "ExePath": exe_path }]),
            );
            let bom = if text.starts_with('\u{feff}') {
                "\u{feff}"
            } else {
                ""
            };
            let pretty = serde_json::to_string_pretty(&object).map_err(|e| e.to_string())?;
            Ok(Some(format!("{bom}{}", pretty.replace('\n', newline))))
        }
        Some(_) => Err("Nucleus Co-op's userprofile.json has no game list".into()),
    }
}

/// The byte offsets of the `[` and `]` of a JSON object's top-level
/// `"Games"` array.
fn games_array(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut games_key = false;
    let mut open = None;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                let start = at + 1;
                at += 1;
                while at < bytes.len() && bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
                if depth == 1 && text.get(start..at) == Some("Games") {
                    games_key = true;
                }
            }
            b',' if depth == 1 => games_key = false,
            b'[' | b'{' => {
                if bytes[at] == b'[' && depth == 1 && games_key && open.is_none() {
                    open = Some(at);
                }
                depth += 1;
            }
            b']' | b'}' => {
                depth -= 1;
                if let (1, b']', Some(open)) = (depth, bytes[at], open) {
                    return Some((open, at));
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

#[cfg(test)]
mod tests {
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
        for line in [
            r#"Game.BinariesFolder = "bin";"#,
            r#"Game.SteamID = "242050";"#,
            "Game.CreateSteamAppIdByExe = true;",
            "Game.SymlinkGame = true;",
            "Game.FakeFocus = true;",
            "Game.ProtoInput.FocusHooks = true;",
            "Game.ProtoInput.XinputHook = true;",
            "Game.ProtoInput.UseOpenXinput = true;",
            "Game.ProtoInput.InjectRuntime_EasyHookMethod = true;",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} in\n{text}");
        }
        // Nothing beyond one instance on one screen.
        assert!(!text.contains("Game.HideTaskbar = true"));
        assert!(
            text.lines()
                .skip(1)
                .all(|l| l.starts_with("Game.") || l.starts_with("//"))
        );
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
    fn nucleus_links_the_whole_game_folder() {
        let steam = steam_game();
        assert_eq!(steam.root, r"D:\SteamLibrary\steamapps\common\Black Flag");
        assert_eq!(steam.binaries.as_deref(), Some("bin"));
        let plain = Setup::new(r"C:\Games\Puzzle\Puzzle.exe", "Puzzle", None);
        assert_eq!(plain.root, r"C:\Games\Puzzle");
        assert_eq!(plain.binaries, None);
        assert!(!plain.handler().contains("BinariesFolder"));
        assert!(!plain.handler().contains("SteamID"));
        let unreal = Setup::new(
            r"D:\Epic\Hero\HeroGame\Binaries\Win64\Hero.exe",
            "Hero",
            None,
        );
        assert_eq!(unreal.root, r"D:\Epic\Hero");
        assert_eq!(unreal.binaries.as_deref(), Some(r"HeroGame\Binaries\Win64"));
        assert!(
            unreal
                .handler()
                .contains(r#"Game.BinariesFolder = "HeroGame\\Binaries\\Win64";"#)
        );
        // Nucleus would cut "bin" out of "binaries": the exe's folder alone.
        let ambiguous = Setup::new(
            r"D:\binaries\Game\bin\Game.exe",
            "Game",
            Some((1, r"D:\binaries\Game")),
        );
        assert_eq!(
            (ambiguous.root.as_str(), ambiguous.binaries),
            (r"D:\binaries\Game\bin", None)
        );
        // A Steam folder the exe is not in does not count.
        let elsewhere = Setup::new(
            r"C:\Other\Game.exe",
            "Game",
            Some((7, r"D:\Steam\common\Game")),
        );
        assert_eq!(elsewhere.root, r"C:\Other");
        assert_eq!(elsewhere.steam_app, Some(7));
    }

    #[test]
    fn anti_cheat_is_found_beside_the_exe_and_up_to_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Shooter");
        let bin = root.join("Shooter").join("Binaries").join("Win64");
        std::fs::create_dir_all(&bin).unwrap();
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
}
