//! Nucleus Co-op handlers and the games the user added in Nucleus.
//!
//! A handler is a JavaScript file in Nucleus' `handlers` folder that sets
//! `Game.*` properties; only plain literal assignments are read here, never
//! run. Nucleus knows a game once the user added its executable, which it
//! keeps in `userprofile.json` as the handler's GUID and the exe path.
use regex::Regex;
use std::sync::OnceLock;

/// What the host needs from a handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handler {
    /// The handler's file name, for messages.
    pub file: String,
    /// `Game.GUID`: Nucleus' key for the game and its profile folder.
    pub guid: String,
    pub name: String,
    /// `Game.ExecutableName`, which Nucleus matches case-insensitively.
    pub executable: String,
    pub max_players: u32,
    pub supports_keyboard: bool,
    /// The handler hides the Windows taskbar, which the person at the PC
    /// would lose while the game runs.
    pub hides_taskbar: bool,
    /// Rubylight's own one-player handler ([`super::generic`]), which only
    /// background streams use.
    pub own: bool,
    /// The account (SID) one of Rubylight's own was written for; None in
    /// one an earlier build wrote.
    pub owner: Option<String>,
    /// The settings it uses that need Nucleus Co-op to run as administrator
    /// ([`needs_admin`]).
    pub admin: Vec<&'static str>,
}

/// Settings with which Nucleus Co-op 2.4.2 refuses to run without
/// administrator rights (`CheckGameRequirements`), or runs `mklink` or a
/// tool's DLL copy through `CmdUtil.ExecuteCommand`, which asks for `runas`
/// by default (`FolderUtil`, `WinDirectoryUtil`, `Tools\*`). HardlinkGame
/// still links folders; HardcopyGame alone copies without it.
const ADMIN_FLAGS: [&str; 16] = [
    "SymlinkGame",
    "HardlinkGame",
    "RequiresAdmin",
    "LaunchAsDifferentUsers",
    "LaunchAsDifferentUsersAlt",
    "ChangeIPPerInstance",
    "ChangeIPPerInstanceAlt",
    "UseX360ce",
    "UseDI8CoopLvlUnlocker",
    "UseNemirtingasGalaxyEmu",
    "UseNemirtingasEpicEmu",
    "UseSteamStubDRMPatcher",
    "UseGoldberg",
    "UseGoldbergNoOGSteamDlls",
    "NeedsSteamEmulation",
    "Hook.SDL2Enabled",
];
/// Lists and texts whose assignment alone does: files to link, the
/// XInputPlus DLLs and a Flawless Widescreen fix to copy.
const ADMIN_VALUES: [&str; 3] = ["SymlinkFiles", "XInputPlusDll", "FlawlessWidescreen"];

/// Whether `handler` needs Nucleus Co-op to run as administrator: without
/// it Nucleus would refuse the game or ask for elevation mid-start. Exact
/// only for the template Rubylight writes ([`super::generic`]), the only
/// handler that may ever run without administrator rights: a hub handler
/// always runs elevated, whatever this says.
pub fn needs_admin(handler: &Handler) -> bool {
    !handler.admin.is_empty()
}

#[derive(Clone, Debug, PartialEq)]
enum Literal {
    Text(String),
    Number(f64),
    Bool(bool),
    /// Anything else: a list, an object, a variable.
    Other,
}

fn assignment() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"^\s*Game\.([A-Za-z0-9_.]+)\s*=\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|-?[0-9]+(?:\.[0-9]+)?|true|false)\s*;?\s*(?://.*)?$"#)
            .unwrap()
    })
}

/// Any assignment to a `Game.*` setting.
fn any_assignment() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*Game\.([A-Za-z0-9_.]+)\s*=(?:[^=]|$)").unwrap())
}

fn unquote(text: &str) -> String {
    let inner = &text[1..text.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// How far a line moves the brace depth, outside strings and comments.
fn depth_change(line: &str) -> i32 {
    let mut change = 0;
    let mut quote = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => quote = Some(c),
            (None, '/') if chars.peek() == Some(&'/') => break,
            (None, '{') => change += 1,
            (None, '}') => change -= 1,
            _ => {}
        }
    }
    change
}

/// The top-level `Game.X = value` assignments; a value that is no literal is
/// [`Literal::Other`]. Those inside functions run only when Nucleus calls
/// them.
fn literals(text: &str) -> Vec<(String, Literal)> {
    let mut out = Vec::new();
    let mut in_comment = false;
    let mut depth = 0;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if in_comment {
            if trimmed.contains("*/") {
                in_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            in_comment = !trimmed.contains("*/");
            continue;
        }
        let top = depth == 0;
        depth = (depth + depth_change(line)).max(0);
        if !top {
            continue;
        }
        let Some(found) = assignment().captures(line) else {
            if let Some(other) = any_assignment().captures(line) {
                out.push((other[1].to_string(), Literal::Other));
            }
            continue;
        };
        let value = &found[2];
        let literal = match value {
            "true" => Literal::Bool(true),
            "false" => Literal::Bool(false),
            v if v.starts_with('"') || v.starts_with('\'') => Literal::Text(unquote(v)),
            v => v.parse().map_or(Literal::Other, Literal::Number),
        };
        out.push((found[1].to_string(), literal));
    }
    out
}

/// The account one of Rubylight's own handlers was written for: the SID
/// on the line after its marker. None in another handler, or one an earlier
/// build wrote.
fn owner(text: &str) -> Option<String> {
    if !super::generic::owns(text) {
        return None;
    }
    let sid = text
        .lines()
        .nth(1)?
        .trim()
        .strip_prefix(super::generic::OWNER)?
        .trim();
    (sid.starts_with("S-1-") && sid[2..].chars().all(|c| c.is_ascii_digit() || c == '-'))
        .then(|| sid.to_string())
}

/// Read a handler's file. An error names what is missing.
pub fn parse(file: &str, text: &str) -> Result<Handler, String> {
    let values = literals(text);
    // The last assignment wins, as when the script runs.
    let last = |key: &str| values.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v);
    let get = |key: &str| {
        values
            .iter()
            .rev()
            .find(|(k, v)| k == key && *v != Literal::Other)
            .map(|(_, v)| v)
    };
    let text_of = |key: &str| match get(key) {
        Some(Literal::Text(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => None,
    };
    let guid = text_of("GUID").ok_or_else(|| format!("{file} sets no Game.GUID"))?;
    let executable =
        text_of("ExecutableName").ok_or_else(|| format!("{file} sets no Game.ExecutableName"))?;
    let max_players = match get("MaxPlayers") {
        Some(Literal::Number(n)) if *n >= 1.0 => *n as u32,
        // Nucleus' own default.
        _ => 2,
    };
    let flag = |key: &str| matches!(get(key), Some(Literal::Bool(true)));
    Ok(Handler {
        file: file.into(),
        name: text_of("GameName").unwrap_or_else(|| guid.clone()),
        guid,
        executable,
        max_players,
        supports_keyboard: flag("SupportsKeyboard"),
        hides_taskbar: flag("HideTaskbar"),
        own: super::generic::owns(text),
        owner: owner(text),
        // A value the script works out may be true.
        admin: ADMIN_FLAGS
            .into_iter()
            .filter(|key| matches!(last(key), Some(Literal::Bool(true) | Literal::Other)))
            .chain(ADMIN_VALUES.into_iter().filter(|key| last(key).is_some()))
            .collect(),
    })
}

/// A game the user added in Nucleus Co-op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub guid: String,
    pub exe_path: String,
}

/// The games in Nucleus' `userprofile.json`.
pub fn user_games(json: &str) -> Result<Vec<Game>, String> {
    let value: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("Nucleus Co-op's userprofile.json is not readable: {e}"))?;
    let Some(games) = value.get("Games") else {
        return Ok(Vec::new());
    };
    let games = games
        .as_array()
        .ok_or("Nucleus Co-op's userprofile.json has no game list")?;
    Ok(games
        .iter()
        .filter_map(|game| {
            Some(Game {
                guid: game.get("GameGuid")?.as_str()?.to_string(),
                exe_path: game.get("ExePath")?.as_str()?.to_string(),
            })
        })
        .collect())
}

/// Why a game cannot run in Nucleus Co-op. The text is what the client and
/// the console show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// The app does not start an executable the host can name.
    NoExecutable {
        app: String,
    },
    NoHandler {
        executable: String,
    },
    /// A handler exists, but the game was never added in Nucleus.
    NotAdded {
        executable: String,
        handler: String,
    },
    /// The game ships anti-cheat, which Nucleus Co-op's hooks would trip.
    AntiCheat {
        executable: String,
        anti_cheat: String,
    },
    /// Rubylight's own handler never starts it, for `reason`: a game on a
    /// network share, Windows' own program, a launcher, a missing .exe.
    Refused {
        reason: String,
    },
}
impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoExecutable { app } => write!(
                f,
                "Nucleus Co-op starts games by their .exe, and {app} does not name one: set the app's command to the game's .exe"
            ),
            Self::NoHandler { executable } => write!(
                f,
                "No Nucleus Co-op handler for {executable}: add one in Nucleus Co-op's handler hub"
            ),
            Self::NotAdded {
                executable,
                handler,
            } => write!(
                f,
                "Nucleus Co-op has the handler {handler} for {executable}, but the game is not added: open Nucleus Co-op and add it once"
            ),
            Self::AntiCheat {
                executable,
                anti_cheat,
            } => write!(
                f,
                "{executable} uses {anti_cheat}, and Rubylight never hooks a game with anti-cheat"
            ),
            Self::Refused { reason } => f.write_str(reason),
        }
    }
}

/// A game Nucleus can start, with its handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub handler: Handler,
    pub exe_path: String,
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}
fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim().replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// The handlers and games of one Nucleus Co-op folder.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub handlers: Vec<Handler>,
    pub games: Vec<Game>,
    /// The signed-in account (SID): only handlers written for it can be
    /// unused. The handlers folder and the game list are the Nucleus Co-op
    /// folder's, which every account shares; a handler written for another
    /// account is left to that account's sweep.
    pub user: Option<String>,
}
impl Catalog {
    /// A hub or user handler: Rubylight's own are for background streams
    /// alone, and never stand in for one.
    fn handler(&self, guid: &str) -> Option<&Handler> {
        self.handlers.iter().find(|h| h.guid == guid && !h.own)
    }

    /// The Nucleus game for an app starting `exe` (a path or a file name).
    /// The user's added game with that exact path wins; otherwise the one
    /// added game with that file name. Rubylight's own handlers do not
    /// count: without a hub handler the game has none.
    pub fn find(&self, app: &str, exe: Option<&str>) -> Result<Match, Missing> {
        let Some(exe) = exe.map(str::trim).filter(|e| !e.is_empty()) else {
            return Err(Missing::NoExecutable { app: app.into() });
        };
        let name = file_name(exe);
        let added = |game: &&Game| self.handler(&game.guid).is_some();
        let by_path = self
            .games
            .iter()
            .filter(added)
            .find(|g| same_path(&g.exe_path, exe));
        let by_name = || {
            let mut named = self
                .games
                .iter()
                .filter(added)
                .filter(|g| file_name(&g.exe_path).eq_ignore_ascii_case(name));
            let first = named.next()?;
            // Two copies of the game added: only the exact path can choose.
            named.next().is_none().then_some(first)
        };
        if let Some(game) = by_path.or_else(by_name) {
            return Ok(Match {
                handler: self.handler(&game.guid).unwrap().clone(),
                exe_path: game.exe_path.clone(),
            });
        }
        match self
            .handlers
            .iter()
            .find(|h| !h.own && h.executable.eq_ignore_ascii_case(name))
        {
            Some(handler) => Err(Missing::NotAdded {
                executable: name.into(),
                handler: handler.name.clone(),
            }),
            None => Err(Missing::NoHandler {
                executable: name.into(),
            }),
        }
    }

    /// The user's added game at exactly `exe_path`, with its hub or user
    /// handler. No file-name match: another game's .exe of the same name
    /// (Portal's and Half-Life 2's hl2.exe) is not this game.
    pub fn added(&self, exe_path: &str) -> Option<Match> {
        self.games
            .iter()
            .filter(|game| same_path(&game.exe_path, exe_path))
            .find_map(|game| {
                Some(Match {
                    handler: self.handler(&game.guid)?.clone(),
                    exe_path: game.exe_path.clone(),
                })
            })
    }

    /// Rubylight's own handlers a hub or user handler replaces, but
    /// `current`, the GUID of the one a session is about to start with: one
    /// whose game the user has since added at the same path with a hub or
    /// user handler, and one for an .exe a hub or user handler is installed
    /// for. Nucleus Co-op's add-game search skips a path already in its list,
    /// so our entry would keep the user from adding the game with the hub's
    /// handler: ours and its game entry go.
    pub fn superseded(&self, current: Option<&str>) -> Vec<&Handler> {
        self.handlers
            .iter()
            .filter(|own| own.own && Some(own.guid.as_str()) != current)
            .filter(|own| {
                self.games
                    .iter()
                    .any(|game| game.guid == own.guid && self.added(&game.exe_path).is_some())
                    || self.handlers.iter().any(|hub| {
                        !hub.own
                            && file_name(&hub.executable)
                                .eq_ignore_ascii_case(file_name(&own.executable))
                    })
            })
            .collect()
    }

    /// Rubylight's own handlers written for [`Self::user`], but `current`'s,
    /// that no game in their list uses: left by a removal or a write that
    /// failed halfway. They go. Another account's, or one without an owner,
    /// may be in a list only that account reads, and stays.
    pub fn unused(&self, current: Option<&str>) -> Vec<&Handler> {
        let Some(user) = self.user.as_deref() else {
            return Vec::new();
        };
        self.handlers
            .iter()
            .filter(|own| own.own && Some(own.guid.as_str()) != current)
            .filter(|own| own.owner.as_deref() == Some(user))
            .filter(|own| !self.games.iter().any(|game| game.guid == own.guid))
            .collect()
    }

    /// [`Self::superseded`] and [`Self::unused`], each once: the handlers a
    /// session's start removes with their game entries.
    pub fn stale(&self, current: Option<&str>) -> Vec<&Handler> {
        let mut stale = self.superseded(current);
        for own in self.unused(current) {
            if !stale.iter().any(|h| h.guid == own.guid) {
                stale.push(own);
            }
        }
        stale
    }

    /// Every added game that has a handler, for the co-op library entries.
    pub fn playable(&self) -> Vec<Match> {
        let mut out: Vec<Match> = self
            .games
            .iter()
            .filter_map(|game| {
                Some(Match {
                    handler: self.handler(&game.guid)?.clone(),
                    exe_path: game.exe_path.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.handler.name.cmp(&b.handler.name));
        out.dedup_by(|a, b| a.handler.guid == b.handler.guid);
        out
    }
}

#[cfg(test)]
mod tests {
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
        let owners: Vec<Option<&str>> =
            c.handlers[3..].iter().map(|h| h.owner.as_deref()).collect();
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
}
