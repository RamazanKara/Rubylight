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
/// The line after [`MARKER`] that names the account that wrote the handler
/// (its SID): only that account's sweep removes it.
pub const OWNER: &str = "// Written for ";
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
    /// install folder, an Unreal game's folder above `<Project>\Binaries`
    /// (the scan stops at `<Project>` when no `Engine` is beside it), else
    /// the .exe's.
    pub root: String,
    /// The Steam app the game was found from. Nucleus Co-op starts with it
    /// as `SteamAppId`, so the game's .exe runs as Steam would start it
    /// rather than restarting through Steam.
    pub steam_app: Option<u32>,
    /// The anti-cheat the game is known to ship from elsewhere than its
    /// files: Steam's Valve Anti-Cheat flag.
    pub anti_cheat: Option<&'static str>,
    /// What the game starts with after its .exe: the app's arguments, or
    /// its Steam launch option's (Portal is `hl2.exe -game portal`).
    pub arguments: String,
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
            arguments: String::new(),
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

    /// The handler's text, as an earlier build wrote it: without an owner.
    pub fn handler(&self) -> String {
        self.handler_of(None)
    }

    /// The handler's text, written for the account `owner` (a SID).
    pub fn handler_of(&self, owner: Option<&str>) -> String {
        let name = if self.name.is_empty() {
            file_name(&self.exe_path)
        } else {
            &self.name
        };
        let mut head = vec![format!(
            "{MARKER} v{VERSION}: Rubylight writes this file for background streams and rewrites it when it changes. A hub handler for this game takes its place."
        )];
        if let Some(owner) = owner {
            head.push(format!("{OWNER}{owner}"));
        }
        head.extend([
            format!("Game.GUID = {};", js(&self.guid())),
            format!("Game.GameName = {};", js(name)),
            format!("Game.ExecutableName = {};", js(file_name(&self.exe_path))),
        ]);
        let arguments = self.arguments.trim();
        if !arguments.is_empty() {
            head.push(format!("Game.StartArguments = {};", js(arguments)));
        }
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
    let holder = parent(binaries);
    let root = parent(holder);
    // Never a drive's root: a game right on a drive (`D:\Game\Binaries`)
    // is its own.
    if drive_root(root) {
        return (!drive_root(holder)).then(|| holder.to_string());
    }
    Some(root.to_string())
}

/// Whether `folder` is a drive's root (`D:`, `\\?\D:`) or no folder at all.
fn drive_root(folder: &str) -> bool {
    let folder = folder.trim_end_matches('\\');
    let folder = folder
        .strip_prefix(r"\\?\")
        .or_else(|| folder.strip_prefix(r"\\.\"))
        .unwrap_or(folder);
    folder.len() <= 2 || !folder.contains('\\')
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
    let Some(local) = on_this_pc(exe_path) else {
        return Some(format!(
            "{exe_path} is on a network share: Rubylight starts only a game on this PC's own drives"
        ));
    };
    below(&local, &normalized(windows))
        .map(|_| format!("{exe_path} is part of Windows, not a game"))
}

/// `path` as a plain drive path (`C:\...`) when it is on one of this PC's
/// drives; None for `\\server\share`, `\\?\UNC\...`, `\\?\GLOBALROOT\...`,
/// every other `\\?\` or `\\.\` path but `\\?\C:\...`, and every other path
/// starting with `\`, such as NT's `\??\UNC\...`. The host runs as SYSTEM and
/// must not reach out to a share.
pub fn on_this_pc(path: &str) -> Option<String> {
    let path = normalized(path);
    let drive = |rest: &str| {
        let bytes = rest.as_bytes();
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == br":\"
    };
    // Any other path from the root of a namespace (`\??\UNC\...`,
    // `\Device\...`) is no drive path either.
    let local = match path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\\.\"))
    {
        Some(rest) => drive(rest).then_some(rest)?,
        None => (!path.starts_with('\\')).then_some(path.as_str())?,
    };
    Some(local.to_string())
}

/// Whether `exe_path` is a launcher rather than a game: Nucleus would hook
/// the launcher, which starts the game outside its hooks (Steam's
/// `-applaunch`, Bethesda's and Rockstar's launchers) or a whole library.
pub fn launcher(exe_path: &str) -> bool {
    let name = file_name(&normalized(exe_path)).to_ascii_lowercase();
    name.ends_with("launcher.exe")
        || [
            "steam.exe",
            "playnite.desktopapp.exe",
            "playnite.fullscreenapp.exe",
            "galaxyclient.exe",
            "eadesktop.exe",
            "origin.exe",
            "upc.exe",
            "ubisoftconnect.exe",
            "battle.net.exe",
            "playgtav.exe",
            "playrdr2.exe",
            "xboxpcapp.exe",
            // Frontends that start a library's games.
            "retrobat.exe",
            "emulationstation.exe",
            "launchbox.exe",
            "bigbox.exe",
        ]
        .contains(&name.as_str())
}

/// The game an Unreal root stub starts: for `<root>\<Game>.exe` beside
/// `<root>\Engine`, the one `<root>\<Project>\Binaries\Win64\
/// *-Win64-Shipping.exe`. The stub only starts it and exits, so Nucleus
/// would hook nothing. None when there is none or more than one.
pub fn unreal_shipping(exe_path: &str) -> Option<String> {
    let exe_path = normalized(exe_path);
    let root = parent(&exe_path);
    if shared_folder(root) || !Path::new(root).join("Engine").is_dir() {
        return None;
    }
    let mut found = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|project| project.file_type().is_ok_and(|kind| kind.is_dir()))
        .flat_map(|project| {
            std::fs::read_dir(project.path().join("Binaries").join("Win64"))
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with("-win64-shipping.exe")
            })
        });
    let first = found.next()?;
    found
        .next()
        .is_none()
        .then(|| first.to_string_lossy().into_owned())
}

/// The anti-cheat `setup`'s game ships: one known from elsewhere (Steam's
/// Valve Anti-Cheat flag), else from the .exe's name and the files and
/// folders beside it and in the folders above it up to the game's root,
/// never above it or in a folder many games share, else from Battle.net's
/// install marker ([`battle_net`]). Nucleus Co-op injects hooks, which
/// anti-cheat treats as cheating, so such a game is refused.
pub fn anti_cheat(setup: &Setup) -> Option<&'static str> {
    if let Some(found) = setup
        .anti_cheat
        .or_else(|| anti_cheat_of(file_name(&setup.exe_path)))
    {
        return Some(found);
    }
    let exe_folder = parent(&setup.exe_path);
    scan(exe_folder, scan_root(setup)).or_else(|| battle_net(exe_folder))
}

/// The anti-cheat in `folder` and the folders above it up to `root`.
fn scan(mut folder: &str, root: &str) -> Option<&'static str> {
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
        below(folder, root)?;
        folder = parent(folder);
    }
}

/// The folder the scan stops at: the game's root, but the folder holding an
/// Unreal game's `Binaries` when nothing says the folder above is the
/// game's. Unreal Engine 4 and 5 keep `<Game>\Engine`, or at least the root
/// stub `<Game>\<Name>.exe`, beside `<Game>\<Project>\Binaries`; Unreal
/// Engine 3 keeps `<Game>\Binaries` beside `<Game>\Engine`, and the folder
/// above `<Game>` is the user's games folder.
fn scan_root(setup: &Setup) -> &str {
    let exe_folder = parent(&setup.exe_path);
    let holder = parent(parent(exe_folder));
    match unreal_root(exe_folder) {
        Some(above)
            if setup.steam_app.is_none()
                && above.eq_ignore_ascii_case(&setup.root)
                && !above.eq_ignore_ascii_case(holder) =>
        {
            let game = Path::new(&above);
            let stub = || {
                !Path::new(holder).join("Engine").is_dir()
                    && game.join(stub_name(&setup.exe_path)).is_file()
            };
            if game.join("Engine").is_dir() || stub() {
                &setup.root
            } else {
                holder
            }
        }
        _ => &setup.root,
    }
}

/// The root stub's name for an Unreal game's .exe: `Stray.exe` for
/// `Stray-Win64-Shipping.exe`, else the .exe's own name.
fn stub_name(exe_path: &str) -> String {
    let name = file_name(exe_path);
    let lower = name.to_ascii_lowercase();
    [
        "-win64-shipping.exe",
        "-win32-shipping.exe",
        "-wingdk-shipping.exe",
    ]
    .iter()
    .find(|suffix| lower.ends_with(*suffix))
    .map_or_else(
        || name.to_string(),
        |suffix| format!("{}.exe", &name[..name.len() - suffix.len()]),
    )
}

/// Blizzard's anti-cheat for a game Battle.net installed: its `.build.info`
/// in the game's folder, which is the .exe's or up to two above it (World of
/// Warcraft's `_retail_`, StarCraft II's `Versions\Base<n>`).
fn battle_net(exe_folder: &str) -> Option<&'static str> {
    let mut folder = exe_folder;
    for _ in 0..3 {
        if shared_folder(folder) {
            return None;
        }
        if Path::new(folder).join(".build.info").is_file() {
            return Some("Blizzard's anti-cheat");
        }
        folder = parent(folder);
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

/// `json` (Nucleus' `userprofile.json`) without the games of `guid`, which
/// only Rubylight's own handler uses. Every other byte stays. None when
/// there is none.
pub fn remove_game(json: &str, guid: &str) -> Result<Option<String>, String> {
    let Some((open, close)) = games_array(json) else {
        return Ok(None);
    };
    // Each game: the byte offsets of its `{` and `}`.
    let bytes = json.as_bytes();
    let mut games = Vec::new();
    let (mut depth, mut start, mut at) = (0, 0, open + 1);
    while at < close {
        match bytes[at] {
            b'"' => {
                at += 1;
                while at < close && bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
            }
            b'{' | b'[' => {
                if depth == 0 {
                    start = at;
                }
                depth += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    games.push((start, at + 1));
                }
            }
            _ => {}
        }
        at += 1;
    }
    let ours = |&(start, end): &(usize, usize)| {
        serde_json::from_str::<serde_json::Value>(&json[start..end])
            .map_err(|e| format!("Nucleus Co-op's userprofile.json is not readable: {e}"))
            .map(|game| game.get("GameGuid").and_then(|g| g.as_str()) == Some(guid))
    };
    let mut kept = Vec::new();
    for game in &games {
        if !ours(game)? {
            kept.push(*game);
        }
    }
    if kept.len() == games.len() {
        return Ok(None);
    }
    let inside = match (games.first(), games.last(), kept.is_empty()) {
        (Some(&(first, _)), Some(&(_, last)), false) => {
            let separator = match games.get(..2) {
                Some([(_, end), (next, _)]) => &json[*end..*next],
                _ => ",",
            };
            let kept: Vec<&str> = kept.iter().map(|&(s, e)| &json[s..e]).collect();
            format!(
                "{}{}{}",
                &json[open + 1..first],
                kept.join(separator),
                &json[last..close]
            )
        }
        _ => String::new(),
    };
    Ok(Some(format!(
        "{}{inside}{}",
        &json[..=open],
        &json[close..]
    )))
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
        assert!(!text.contains("StartArguments"));
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
}
