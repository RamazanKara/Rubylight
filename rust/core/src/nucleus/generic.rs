//! Rubylight's own one-player Nucleus Co-op handler, for background streams
//! of games no hub handler covers.
//!
//! Nucleus Co-op is the only way a game streams in the background, and it
//! starts a game only through a handler. For a one-player game without one
//! the host writes this handler into Nucleus' `handlers` folder and adds the
//! game to Nucleus' game list `content\userprofile.json`, the way Nucleus'
//! own "add game" does (2.4.2 reads no other list). Co-op still needs a hub handler, and a hub handler for the same
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
const VERSION: u32 = 3;

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
    /// How Nucleus Co-op injects ProtoInput's input hooks into the game.
    pub inject: Inject,
    /// The stream's size, which an Unreal game started in a window renders
    /// at ([`Setup::windowed_arguments`]); None when no stream asked.
    pub size: Option<(u32, u32)>,
    /// Whether the game's .exe imports DirectInput
    /// ([`super::imports::direct_input`]), which reads the PC's keyboard
    /// through raw input; ProtoInput then takes the game's raw input.
    pub direct_input: bool,
}

/// What Nucleus Co-op 2.4.2 says in a message box when ProtoInput's hooks
/// can't be injected into a running game or one it starts
/// (`ProtoInputLauncher.InjectRuntime`, `InjectStartup`), just before it
/// ends the game.
pub const INJECT_FAILED: [&str; 2] = [
    "ProtoInput failed to runtime inject",
    "ProtoInput failed to startup inject",
];

/// How Nucleus Co-op injects ProtoInput's input hooks into the game. Nucleus
/// ends a game it can't inject into ([`INJECT_FAILED`]), so a game that
/// refuses one way gets the next at its next start (#59), down to none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Inject {
    /// Into the running game, through EasyHook.
    #[default]
    Runtime,
    /// As Nucleus starts the game, before any of its code runs: in before a
    /// game that guards itself once it runs (one whose mods load as DLLs
    /// at its start).
    Startup,
    /// Into the running game from one of its own threads, for a game that
    /// refuses a thread made in it.
    Stealth,
    /// None, and no focus messages from Nucleus either: nothing would hold
    /// back what they wake in the game (#59).
    None,
}

impl Inject {
    pub const ALL: [Inject; 4] = [Self::Runtime, Self::Startup, Self::Stealth, Self::None];

    /// The way tried after this one is refused; None after the last.
    pub fn next(self) -> Option<Inject> {
        let at = Self::ALL.iter().position(|&way| way == self)?;
        Self::ALL.get(at + 1).copied()
    }

    /// Its name in the host's record of each game's way.
    pub fn name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Startup => "startup",
            Self::Stealth => "stealth",
            Self::None => "none",
        }
    }

    pub fn from_name(name: &str) -> Option<Inject> {
        Self::ALL.into_iter().find(|way| way.name() == name)
    }
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
            inject: Inject::Runtime,
            size: None,
            direct_input: false,
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

    /// The game's arguments with `-windowed` added for an Unreal game (its
    /// .exe in `<Project>\Binaries\Win64`): fullscreen, it covers the monitor
    /// nearest its window, the PC's, however often it is parked beside the
    /// desktop; in a window it stays parked (#56). The switch overrides the
    /// game's saved mode for this start only. Arguments that already choose
    /// a mode stay as they are. In a window it renders at the stream's
    /// [`Setup::size`] (`-ResX`, `-ResY`, and `-ForceRes`, without which a
    /// window larger than the primary display shrinks to fit it), not at its
    /// saved resolution,
    /// which showed a 16:9 picture on a 20:9 phone (#56), unless the
    /// arguments choose a resolution.
    pub fn windowed_arguments(&self) -> String {
        let arguments = self.arguments.trim();
        let words: Vec<String> = arguments
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        let has = |flags: &[&str]| {
            words
                .iter()
                .any(|word| flags.iter().any(|flag| word.starts_with(flag)))
        };
        if unreal_root(parent(&self.exe_path)).is_none() || has(&["-fullscreen", "-borderless"]) {
            return arguments.to_string();
        }
        let mut started = arguments.to_string();
        let mut add = |word: &str| {
            if !started.is_empty() {
                started.push(' ');
            }
            started.push_str(word);
        };
        if !has(&["-windowed"]) {
            add("-windowed");
        }
        if let Some((width, height)) = self.size
            && !has(&["-resx", "-resy"])
        {
            add(&format!("-ResX={width} -ResY={height}"));
            // Unreal shrinks a window larger than the primary display to fit
            // it (a 2400x1080 phone beside a 1920x1080 monitor) unless told
            // to keep the size.
            if !has(&["-forceres"]) {
                add("-ForceRes");
            }
        }
        started
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
        ];
        let way: &[&str] = match self.inject {
            Inject::Runtime => &[
                "Game.ProtoInput.InjectStartup = false;",
                "Game.ProtoInput.InjectRuntime_EasyHookMethod = true;",
            ],
            Inject::Startup => &[
                "// The game refused ProtoInput's hooks once it ran, so Nucleus Co-op injects them as it starts the game.",
                "Game.ProtoInput.InjectStartup = true;",
            ],
            Inject::Stealth => &[
                "// The game refused ProtoInput's hooks once it ran and as it started, so Nucleus Co-op injects them from one of its own threads.",
                "Game.ProtoInput.InjectStartup = false;",
                "Game.ProtoInput.InjectRuntime_EasyHookStealthMethod = true;",
            ],
            Inject::None => &[],
        };
        let focus = [
            "// The game keeps running and reading its pad without focus, and never sees the PC's keyboard or moves its cursor.",
            "Game.FakeFocus = true;",
        ];
        let hooks = [
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
        ];
        // Only for a game that reads DirectInput: ProtoInput takes all of a
        // game's raw input and passes on none of a pad's, so a game reading
        // its pad through raw input (SDL, Rewired) would lose it.
        let raw_input: &[&str] = if self.direct_input {
            &[
                "// Nor its raw input, through which DirectInput reads the PC's keyboard (NieR:Automata, #56): ProtoInput takes the game's raw input and passes on only the slot's keyboard and mouse, and a pad's slot has none. The game reads DirectInput, so its pad does not come through raw input.",
                "Game.ProtoInput.RegisterRawInputHook = true;",
                "Game.ProtoInput.GetRawInputDataHook = true;",
            ]
        } else {
            &[]
        };
        // No FakeFocus: Nucleus would tell the game every second that it is
        // active, and only ProtoInput's cursor hooks keep a game that believes
        // it from taking the mouse back from the person at the PC, towards
        // its window parked right of the desktop (#59). It never gave such a
        // game its pad, which it reads only when really in front.
        let unhooked = [
            "// The game refused ProtoInput's input hooks every way Nucleus Co-op injects them, so none are injected and a keyboard, mouse or pad on the PC can reach it too. It knows when it has focus, so it lets go of the PC's mouse when it loses it.",
            // Nucleus 2.4.2 lists XInput pads on its setup screen only for a
            // handler with ProtoInput's XInput hook or this switch
            // (`DevicesFunctions.UpdateDevices`). Without either it never
            // sees the profile's pad and stays on that screen (#59). The
            // switch alone injects nothing: Nucleus copies its XInput DLLs
            // only with Hook.CustomDllEnabled or XInputPlusDll, both off.
            "Game.Hook.XInputEnabled = true;",
        ];
        let options: Vec<&&str> = match self.inject {
            Inject::None => options.iter().chain(&unhooked).collect(),
            _ => options
                .iter()
                .chain(&focus)
                .chain(way)
                .chain(&hooks)
                .chain(raw_input)
                .collect(),
        };
        let lines: Vec<&str> = head
            .iter()
            .map(String::as_str)
            .chain(options.into_iter().copied())
            .collect();
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

/// What says which Epic game the game at `exe_path` is, from one of the Epic
/// Games Launcher's `.item` manifests, without the sign-in we don't have; None
/// when the manifest is another game's. The game may be any .exe below its
/// install folder, since many manifests name a launcher stub, and the manifest
/// of a DLC names none. Started from its .exe without these, a game that asks
/// the Epic SDK restarts through the launcher, outside Nucleus Co-op and on the
/// PC's monitor, and the instance exits (#56). `SteamAppId` does the same for a
/// Steam game ([`Setup::steam_app`]).
///
/// Any user can write these manifests and the game may run elevated, so only
/// names that can't carry an option reach its command line, and never the
/// manifest's `LaunchCommand`: an app that needs those options has its own
/// arguments.
pub fn epic_arguments(manifest: &[u8], exe_path: &str) -> Result<Option<String>, String> {
    let record: serde_json::Value =
        serde_json::from_slice(manifest).map_err(|error| error.to_string())?;
    let text = |key: &str| record[key].as_str().unwrap_or("").trim();
    let (folder, launched) = (text("InstallLocation"), text("LaunchExecutable"));
    if folder.is_empty() || launched.is_empty() {
        return Ok(None);
    }
    let folder = normalized(folder);
    let folder = folder.trim_end_matches('\\');
    let exe = normalized(exe_path);
    // A drive's root is no game's folder: it would claim every game on it.
    if !folder.contains('\\')
        || !exe.to_ascii_lowercase().ends_with(".exe")
        || below(&exe, folder).is_none()
    {
        return Ok(None);
    }
    let name = |value: &str| {
        (1..=128).contains(&value.len())
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    let (app, namespace) = (text("AppName"), text("CatalogNamespace"));
    if !name(app) || !name(namespace) {
        return Err(format!("{exe} has no usable app name or namespace"));
    }
    Ok(Some(format!(
        "-EpicPortal -epicapp={app} -epicenv=Prod -epicsandboxid={namespace}"
    )))
}

/// Why an Epic game's instance ended soon after it started.
pub fn epic_exit(exited: &str, title: &str) -> String {
    format!(
        "{exited}: {title} is an Epic Games game, and it may have started again through the Epic Games Launcher, on the PC's monitor and outside Nucleus Co-op. Close it there if it is open; a normal stream plays it"
    )
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

/// `json` (Nucleus' `content\userprofile.json`, None when there is none yet)
/// with the game added as Nucleus' "add game" does (`UserGameInfo
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
mod tests;
