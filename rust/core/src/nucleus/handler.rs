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
}

#[derive(Clone, Debug, PartialEq)]
enum Literal {
    Text(String),
    Number(f64),
    Bool(bool),
}

fn assignment() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"^\s*Game\.([A-Za-z0-9_.]+)\s*=\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|-?[0-9]+(?:\.[0-9]+)?|true|false)\s*;?\s*(?://.*)?$"#)
            .unwrap()
    })
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

/// The top-level `Game.X = literal` assignments. Those inside functions run
/// only when Nucleus calls them.
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
        let Some(found) = assignment().captures(line).filter(|_| top) else {
            continue;
        };
        let value = &found[2];
        let literal = match value {
            "true" => Literal::Bool(true),
            "false" => Literal::Bool(false),
            v if v.starts_with('"') || v.starts_with('\'') => Literal::Text(unquote(v)),
            v => match v.parse() {
                Ok(n) => Literal::Number(n),
                Err(_) => continue,
            },
        };
        out.push((found[1].to_string(), literal));
    }
    out
}

/// Read a handler's file. An error names what is missing.
pub fn parse(file: &str, text: &str) -> Result<Handler, String> {
    let values = literals(text);
    // The last assignment wins, as when the script runs.
    let get = |key: &str| values.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v);
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
}
impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoExecutable { app } => write!(
                f,
                "Nucleus Co-op starts games by their .exe, and {app} does not name one: set the app's command to the game's .exe, or use a child session"
            ),
            Self::NoHandler { executable } => write!(
                f,
                "No Nucleus Co-op handler for {executable}: add one in Nucleus Co-op's handler hub, or use a child session"
            ),
            Self::NotAdded {
                executable,
                handler,
            } => write!(
                f,
                "Nucleus Co-op has the handler {handler} for {executable}, but the game is not added: open Nucleus Co-op and add it once"
            ),
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
}
impl Catalog {
    fn handler(&self, guid: &str) -> Option<&Handler> {
        self.handlers.iter().find(|h| h.guid == guid)
    }

    /// The Nucleus game for an app starting `exe` (a path or a file name).
    /// The user's added game with that exact path wins; otherwise the one
    /// added game with that file name.
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
            .find(|h| h.executable.eq_ignore_ascii_case(name))
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
    fn a_game_nucleus_cannot_start_says_why() {
        let c = catalog();
        let none = c
            .find("Puzzle", Some("C:\\Puzzle\\Puzzle.exe"))
            .unwrap_err();
        assert_eq!(
            none.to_string(),
            "No Nucleus Co-op handler for Puzzle.exe: add one in Nucleus Co-op's handler hub, or use a child session"
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
