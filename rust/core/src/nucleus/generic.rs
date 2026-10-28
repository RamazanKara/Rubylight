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
    /// The game's folder, which [`anti_cheat`] looks through: its Steam
    /// install folder, an Unreal game's folder above `Binaries`, else the
    /// .exe's.
    pub root: String,
    /// The Steam app the game was found from. Nucleus Co-op starts with it
    /// as `SteamAppId`, so the game's .exe runs as Steam would start it
    /// rather than restarting through Steam.
    pub steam_app: Option<u32>,
    /// The anti-cheat the game is known to ship from elsewhere than its
    /// files: Steam's Valve Anti-Cheat flag.
    pub anti_cheat: Option<&'static str>,
}

impl Setup {
    /// The setup for `exe_path`; `steam` is the Steam app it was found from
    /// and the folder Steam installed it to, when it was.
    pub fn new(exe_path: &str, name: &str, steam: Option<(u32, &str)>) -> Self {
        let exe_path = normalized(exe_path);
        let exe_folder = parent(&exe_path).to_string();
        let root = steam
            .map(|(_, install)| normalized(install))
            .filter(|folder| below(&exe_path, folder).is_some())
            .map(|folder| folder.trim_end_matches('\\').to_string())
            .or_else(|| unreal_root(&exe_folder))
            .unwrap_or(exe_folder);
        Self {
            exe_path,
            name: name.trim().to_string(),
            root,
            steam_app: steam.map(|(app, _)| app),
            anti_cheat: None,
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
        let head = [
            format!(
                "{MARKER} v{VERSION}: Rubylight writes this file for background streams and rewrites it when it changes. A hub handler for this game takes its place."
            ),
            format!("Game.GUID = {};", js(&self.guid())),
            format!("Game.GameName = {};", js(name)),
            format!("Game.ExecutableName = {};", js(file_name(&self.exe_path))),
        ];
        let options = [
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
        ];
        let lines: Vec<&str> = head.iter().map(String::as_str).chain(options).collect();
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
/// Nucleus' ES5 engine (Jint) ends a line at U+2028 and U+2029, even in a
/// string, so they are escaped.
fn js(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars().filter(|c| !c.is_control()) {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
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
        } else if any(&["cs2.exe", "csgo.exe", "dota2.exe", "tf.exe", "tf_win64.exe"]) {
            "Valve Anti-Cheat"
        } else if any(&["overwatch.exe"]) {
            "Blizzard's anti-cheat"
        } else if starts("faceit") {
            "FACEIT"
        } else if any(&["tslgame.exe", "rainbowsix.exe", "rainbowsix_be.exe"]) {
            "BattlEye"
        } else if any(&["fortniteclient-win64-shipping.exe", "r5apex.exe"]) {
            "Easy Anti-Cheat"
        } else {
            return None;
        },
    )
}

/// A folder many games share, whose files say nothing about one game: a
/// drive's root, Program Files, Steam's and Epic's game folders, XboxGames.
fn shared_folder(folder: &str) -> bool {
    let name = file_name(folder);
    let is = |other: &str| name.eq_ignore_ascii_case(other);
    folder.trim_end_matches('\\').len() <= 3
        || !folder.contains('\\')
        || [
            "Program Files",
            "Program Files (x86)",
            "Epic Games",
            "XboxGames",
            "WindowsApps",
            "steamapps",
        ]
        .iter()
        .any(|shared| is(shared))
        || (is("common") && file_name(parent(folder)).eq_ignore_ascii_case("steamapps"))
}

/// Why Rubylight never writes its handler for `exe_path`, checked before
/// anything opens the path: the host runs as SYSTEM, which must not reach
/// out to a network share, and Windows' own programs (`windows`, the
/// Windows folder) are no game. A folder in the user's profile is fine.
pub fn refused_path(exe_path: &str, windows: &str) -> Option<String> {
    let path = normalized(exe_path);
    // `\\server\share`, `\\?\UNC\...` and device paths; `\\?\C:\` is local.
    let local = path
        .strip_prefix(r"\\?\")
        .filter(|rest| {
            !rest
                .get(..4)
                .is_some_and(|unc| unc.eq_ignore_ascii_case(r"UNC\"))
        })
        .unwrap_or(&path);
    if local.starts_with(r"\\") {
        return Some(format!(
            "{exe_path} is on a network share: Rubylight starts only a game on this PC's own drives"
        ));
    }
    below(local, &normalized(windows)).map(|_| format!("{exe_path} is part of Windows, not a game"))
}

/// The anti-cheat `setup`'s game ships: one known from elsewhere (Steam's
/// Valve Anti-Cheat flag), else from the .exe's name and the files and
/// folders beside it and in the folders above it up to the game's root,
/// never above it or in a folder many games share. Nucleus Co-op injects
/// hooks, which anti-cheat treats as cheating, so such a game is refused.
pub fn anti_cheat(setup: &Setup) -> Option<&'static str> {
    if let Some(found) = setup
        .anti_cheat
        .or_else(|| anti_cheat_of(file_name(&setup.exe_path)))
    {
        return Some(found);
    }
    let mut folder = parent(&setup.exe_path);
    loop {
        if shared_folder(folder) {
            return None;
        }
        let entries = std::fs::read_dir(Path::new(folder)).into_iter().flatten();
        for entry in entries.flatten() {
            if let Some(found) = anti_cheat_of(&entry.file_name().to_string_lossy()) {
                return Some(found);
            }
        }
        // Up to the game's root, never past it.
        below(folder, &setup.root)?;
        folder = parent(folder);
    }
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
}
