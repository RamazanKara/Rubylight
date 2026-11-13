//! Playnite: the plugin's messages and the apps a sync keeps, as in
//! Vibepollo 2.0.
//!
//! The Playnite plugin (`SunshinePlaynite`) serves a named pipe and sends
//! one JSON message per line. A sync matches the library's apps against
//! Playnite's games and adds the recently played, chosen category, chosen
//! plugin or all installed games as apps marked `playnite-managed: "auto"`
//! and stamped `synced-by: playnite`; only those are the sync's to change.
use crate::{config::Config, library};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

/// A game as the plugin describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub exe: String,
    pub args: String,
    pub working_dir: String,
    pub install_dir: String,
    pub categories: Vec<String>,
    pub plugin_id: String,
    pub plugin_name: String,
    pub playtime_minutes: u64,
    /// Unix seconds, if Playnite has a valid last played time.
    pub last_played: Option<i64>,
    /// Playnite sent a last played time that doesn't parse: when is unknown, not never.
    pub last_played_unknown: bool,
    pub box_art_path: String,
    pub icon_path: String,
    pub installed: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    SnapshotStart,
    SnapshotComplete,
    Plugins(Vec<(String, String)>),
    Categories(Vec<(String, String)>),
    Games(Vec<Game>),
    /// gameStarted, gameStopped, stopRequested or playniteExiting.
    Status {
        name: String,
        id: String,
        install_dir: String,
        exe: String,
    },
    /// The answer to a command that asked for one (`set-cover`).
    CommandResult {
        command: String,
        request_id: String,
        success: bool,
        error: String,
    },
    Other,
}
fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}
fn pairs(value: Option<&Value>) -> Vec<(String, String)> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| (text(item, "id"), text(item, "name")))
                .filter(|(id, name)| !id.is_empty() || !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}
/// One line from the plugin (a UTF-8 BOM is allowed), reading times without an
/// offset as UTC.
pub fn parse(line: &str) -> Message {
    parse_with(line, &Some)
}
/// [`parse`], with `local` turning a time without an offset (this PC's wall clock,
/// read as if UTC) into Unix seconds; `None` when it can't.
pub fn parse_with(line: &str, local: &dyn Fn(i64) -> Option<i64>) -> Message {
    let Ok(message) = serde_json::from_str::<Value>(line.trim_start_matches('\u{feff}').trim())
    else {
        return Message::Other;
    };
    match message.get("type").and_then(Value::as_str).unwrap_or("") {
        "snapshotStart" => Message::SnapshotStart,
        "snapshotComplete" => Message::SnapshotComplete,
        "plugins" => Message::Plugins(pairs(message.get("payload"))),
        "categories" => Message::Categories(pairs(message.get("payload"))),
        "games" => Message::Games(
            message
                .get("payload")
                .and_then(Value::as_array)
                .map(|games| games.iter().filter_map(|g| game(g, local)).collect())
                .unwrap_or_default(),
        ),
        "status" => {
            let status = message.get("status").cloned().unwrap_or_default();
            Message::Status {
                name: text(&status, "name"),
                id: text(&status, "id"),
                install_dir: text(&status, "installDir"),
                exe: text(&status, "exe"),
            }
        }
        "commandResult" => Message::CommandResult {
            command: text(&message, "command"),
            request_id: text(&message, "requestId"),
            success: message.get("success").and_then(Value::as_bool) == Some(true),
            error: text(&message, "error"),
        },
        _ => Message::Other,
    }
}
fn game(value: &Value, local: &dyn Fn(i64) -> Option<i64>) -> Option<Game> {
    let id = text(value, "id");
    if uuid::Uuid::parse_str(&id).is_err() {
        return None;
    }
    let flag = |key: &str| value.get(key).map(|v| v.as_bool().unwrap_or(false));
    // Without either flag the game counts as installed, as in Vibepollo.
    let installed = match (flag("installed"), flag("isInstalled")) {
        (None, None) => true,
        (a, b) => a.unwrap_or(false) || b.unwrap_or(false),
    };
    // The plugin sends "" for never played.
    let played = text(value, "lastPlayed");
    let last_played = parse_time_with(&played, local);
    Some(Game {
        name: text(value, "name"),
        exe: text(value, "exe"),
        args: text(value, "args"),
        working_dir: text(value, "workingDir"),
        install_dir: text(value, "installDir"),
        categories: value
            .get("categories")
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        plugin_id: text(value, "pluginId"),
        plugin_name: text(value, "pluginName"),
        playtime_minutes: text(value, "playtimeMinutes").parse().unwrap_or(0),
        last_played,
        last_played_unknown: !played.trim().is_empty() && last_played.is_none(),
        box_art_path: text(value, "boxArtPath"),
        icon_path: text(value, "iconPath"),
        installed,
        id,
    })
}
/// An ISO 8601 time (`2026-08-29T17:49:56.1234567+02:00`, `+0200`, `+02`, `...Z`)
/// as Unix seconds; one without an offset is read as UTC.
pub fn parse_time(value: &str) -> Option<i64> {
    parse_time_with(value, &Some)
}
/// [`parse_time`], with `local` for a time without an offset: .NET's round-trip
/// format, which the plugin sends, leaves the offset out of a wall-clock time.
pub fn parse_time_with(value: &str, local: &dyn Fn(i64) -> Option<i64>) -> Option<i64> {
    let value = value.trim();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = value.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let separators = value.as_bytes();
    if separators.len() < 19
        || separators[4] != b'-'
        || separators[7] != b'-'
        || !matches!(separators[10], b'T' | b't' | b' ')
        || separators[13] != b':'
        || separators[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &value[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        rest = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest.as_bytes().first() {
        None => None,
        Some(b'Z' | b'z') if rest.len() == 1 => Some(0),
        Some(sign @ (b'+' | b'-')) => {
            let zone = &rest[1..];
            // Only digits and a colon are a zone, and `split_at` needs a character boundary.
            if !zone.is_ascii() {
                return None;
            }
            // `+02:00`, `+0200` or `+02`.
            let (h, m) = match zone.split_once(':') {
                Some(parts) => parts,
                None if zone.len() == 4 => zone.split_at(2),
                None if zone.len() == 2 => (zone, "00"),
                None => return None,
            };
            if h.len() != 2
                || m.len() != 2
                || !(h.bytes().chain(m.bytes())).all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let minutes = h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?;
            Some(if *sign == b'+' { minutes } else { -minutes })
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let wall = days * 86400 + hour * 3600 + minute * 60 + second;
    match offset {
        Some(offset) => Some(wall - offset * 60),
        None => local(wall),
    }
}
/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86400);
    let rest = seconds.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// The `playnite_*` settings, with Vibepollo's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub auto_sync: bool,
    pub sync_all_installed: bool,
    pub recent_games: usize,
    pub recent_max_age_days: i64,
    pub delete_after_days: i64,
    pub remove_uninstalled: bool,
    pub require_replacement: bool,
    /// Category names.
    pub sync_categories: Vec<String>,
    pub exclude_categories: Vec<String>,
    /// Plugin and game ids.
    pub sync_plugins: Vec<String>,
    pub exclude_plugins: Vec<String>,
    pub exclude_games: Vec<String>,
    pub focus_attempts: i64,
    pub focus_timeout_secs: i64,
    pub focus_exit_on_first: bool,
    pub fullscreen_entry: bool,
    /// How long a game must stay closed before its stream ends
    /// (`playnite_exit_wait_secs`, at most [`EXIT_WAIT_MAX`]).
    pub exit_wait: Duration,
}
/// The default of `playnite_exit_wait_secs`: Playnite reports a game stopped
/// when its launcher hands over to the game, or when the game restarts itself,
/// and the stream has to outlast that.
pub const EXIT_WAIT: Duration = Duration::from_secs(15);
pub const EXIT_WAIT_MAX: Duration = Duration::from_secs(300);
impl Settings {
    pub fn from_config(config: &Config) -> Self {
        let count = |key: &str, default: i64| config.integer(key, default).max(0);
        // Category lists hold names, the others ids; a plain string is
        // whichever the list holds.
        let list = |key: &str, by_name: bool| -> Vec<String> {
            crate::steam::id_names(config.get(key, ""))
                .into_iter()
                .map(|(id, name)| {
                    if by_name && !name.is_empty() {
                        name
                    } else {
                        id
                    }
                })
                .filter(|value| !value.is_empty())
                .collect()
        };
        Self {
            enabled: config.boolean("playnite_enabled", true),
            auto_sync: config.boolean("playnite_auto_sync", true),
            sync_all_installed: config.boolean("playnite_sync_all_installed", true),
            recent_games: count("playnite_recent_games", 10) as usize,
            recent_max_age_days: count("playnite_recent_max_age_days", 30),
            delete_after_days: count("playnite_autosync_delete_after_days", 14),
            remove_uninstalled: config.boolean("playnite_autosync_remove_uninstalled", true),
            require_replacement: config.boolean("playnite_autosync_require_replacement", true),
            sync_categories: list("playnite_sync_categories", true),
            exclude_categories: list("playnite_exclude_categories", true),
            sync_plugins: list("playnite_sync_plugins", false),
            exclude_plugins: list("playnite_exclude_plugins", false),
            exclude_games: list("playnite_exclude_games", false),
            focus_attempts: count("playnite_focus_attempts", 3),
            focus_timeout_secs: count("playnite_focus_timeout_secs", 15),
            focus_exit_on_first: config.boolean("playnite_focus_exit_on_first", false),
            fullscreen_entry: config.boolean("playnite_fullscreen_entry_enabled", false),
            exit_wait: Duration::from_secs(count(
                "playnite_exit_wait_secs",
                EXIT_WAIT.as_secs() as i64,
            ) as u64)
            .min(EXIT_WAIT_MAX),
        }
    }
}

/// Bringing a started game, or the fullscreen menu, to the front, as
/// Vibepollo's launcher does with the `playnite_focus_*` settings: once a
/// second for `focus_timeout_secs`, until the window was confirmed in front
/// `focus_attempts` times (once with `focus_exit_on_first`). Launchers and
/// splash screens take the foreground back while a game starts, so a single
/// success is not always the end of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Focus {
    left: i64,
    deadline: Instant,
    next: Instant,
}
impl Focus {
    pub const INTERVAL: Duration = Duration::from_secs(1);
    /// Start focusing at `now`; None when the settings turn it off.
    pub fn arm(settings: &Settings, now: Instant) -> Option<Self> {
        if settings.focus_attempts <= 0 || settings.focus_timeout_secs <= 0 {
            return None;
        }
        Some(Self {
            left: if settings.focus_exit_on_first {
                1
            } else {
                settings.focus_attempts
            },
            deadline: now + Duration::from_secs(settings.focus_timeout_secs as u64),
            next: now,
        })
    }
    /// Whether to check the window, and bring it forward, now.
    pub fn due(&self, now: Instant) -> bool {
        now >= self.next && !self.finished(now)
    }
    pub fn finished(&self, now: Instant) -> bool {
        self.left <= 0 || now >= self.deadline
    }
    /// Record a check at `now`; `focused` when the window was in front.
    pub fn checked(&mut self, now: Instant, focused: bool) {
        if focused {
            self.left -= 1;
        }
        self.next = now + Self::INTERVAL;
    }
}

const RECENT: u8 = 1;
const CATEGORY: u8 = 2;
const PLUGIN: u8 = 4;
const INSTALLED: u8 = 8;
fn source_label(flags: u8) -> String {
    let parts: Vec<&str> = [
        (RECENT, "recent"),
        (CATEGORY, "category"),
        (PLUGIN, "plugin"),
        (INSTALLED, "installed"),
    ]
    .iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, name)| *name)
    .collect();
    if parts.is_empty() {
        "unknown".into()
    } else {
        parts.join("+")
    }
}
fn key(id: &str) -> String {
    id.to_ascii_lowercase()
}
fn path_key(path: &str) -> String {
    path.replace('"', "").replace('/', "\\").to_lowercase()
}
/// Match a game or emulator after its launcher hands it off. Store clients
/// remaining open do not prove that the game is still running.
pub fn game_process(path: &str, install_dir: &str, exe: &str) -> bool {
    let path = path_key(path);
    let name = path.rsplit('\\').next().unwrap_or("");
    if matches!(
        name,
        "steam.exe"
            | "steamwebhelper.exe"
            | "steamservice.exe"
            | "gameoverlayui.exe"
            | "epicgameslauncher.exe"
            | "eadesktop.exe"
            | "upc.exe"
            | "ubisoftconnect.exe"
            | "battle.net.exe"
            | "galaxyclient.exe"
            | "playnite.desktopapp.exe"
            | "playnite.fullscreenapp.exe"
    ) {
        return false;
    }
    let folder = path_key(install_dir);
    let folder = folder.trim_end_matches('\\');
    (!folder.is_empty()
        && path
            .strip_prefix(folder)
            .is_some_and(|rest| rest.starts_with('\\')))
        || (!exe.is_empty() && path == path_key(exe))
}
fn name_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn command_program(command: &str) -> String {
    let command = command.trim_start_matches([' ', '\t']);
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split([' ', '\t']).next().unwrap_or(""),
    };
    path_key(program)
}
fn field<'a>(app: &'a Value, name: &str) -> &'a str {
    app.get(name).and_then(Value::as_str).unwrap_or("")
}
/// An app this sync added and still owns.
fn owned(app: &Value) -> bool {
    library::synced_by(app, "playnite")
}
/// Vibepollo's UUID for a Playnite app: the game id in capitals.
pub fn app_uuid(id: &str) -> String {
    id.to_ascii_uppercase()
}
/// Converted cover and icon files for a game, by lower-case id.
pub type Artwork = HashMap<String, (Option<PathBuf>, Option<PathBuf>)>;
fn apply_game(app: &mut Value, game: &Game, artwork: &Artwork) {
    if !app.is_object() {
        *app = json!({});
    }
    let fields = app.as_object_mut().unwrap();
    if !game.name.is_empty() {
        fields.insert("name".into(), json!(game.name));
    }
    fields.insert("playnite-id".into(), json!(game.id));
    // Playnite starts the game; the app has no command of its own.
    fields.remove("cmd");
    fields.remove("working-dir");
    for (name, value) in [
        ("playnite-plugin-id", &game.plugin_id),
        ("playnite-plugin-name", &game.plugin_name),
    ] {
        if value.is_empty() {
            fields.remove(name);
        } else {
            fields.insert(name.into(), json!(value));
        }
    }
    let (cover, icon) = artwork.get(&key(&game.id)).cloned().unwrap_or_default();
    if let Some(cover) = cover {
        fields.insert(
            "image-path".into(),
            json!(cover.to_string_lossy().replace('\\', "/")),
        );
    }
    match icon {
        Some(icon) => {
            fields.insert(
                "playnite-icon-path".into(),
                json!(icon.to_string_lossy().replace('\\', "/")),
            );
        }
        None => {
            fields.remove("playnite-icon-path");
        }
    }
    fields.insert("uuid".into(), json!(app_uuid(&game.id)));
    // Earlier syncs wrote the 10 s exit timeout into every game; without it the game follows
    // the host's `exit_timeout`, 10 s unless changed. An app the user saved is not the sync's.
    drop_written_exit_timeout(fields);
}
/// Remove the `exit-timeout` of 10 that syncs before the host-wide default wrote into the apps
/// they own, so they follow `exit_timeout`. True when one was removed.
fn drop_written_exit_timeout(fields: &mut serde_json::Map<String, Value>) -> bool {
    if fields.get("exit-timeout").and_then(Value::as_u64) == Some(10) {
        fields.remove("exit-timeout");
        return true;
    }
    false
}
fn excluded(
    game: &Game,
    ids: &HashSet<String>,
    categories: &HashSet<String>,
    plugins: &HashSet<String>,
) -> bool {
    ids.contains(&key(&game.id))
        || game
            .categories
            .iter()
            .any(|c| categories.contains(&c.to_lowercase()))
        || (!game.plugin_id.is_empty() && plugins.contains(&key(&game.plugin_id)))
}
/// Bring the library in line with Playnite's games. With `manage` (auto
/// sync) the selection adds and removes the apps this sync owns; without it
/// only the owned apps get their game's current name and artwork. An app the
/// user made or edited only keeps its game from being added twice, and gets a
/// cover when it has none. Games in `dismissed` (`playnite:<id>`, deleted
/// from the library) are not added. Returns whether anything changed.
pub fn reconcile(
    apps: &mut Vec<Value>,
    games: &[Game],
    settings: &Settings,
    now: i64,
    artwork: &Artwork,
    manage: bool,
    dismissed: &[String],
) -> bool {
    let before = apps.clone();
    if manage {
        let mut seen = HashSet::new();
        apps.retain(|app| {
            let id = field(app, "playnite-id");
            !owned(app) || id.is_empty() || seen.insert(key(id))
        });
    }
    let installed: Vec<&Game> = games.iter().filter(|g| g.installed).collect();
    let uninstalled: HashSet<String> = games
        .iter()
        .filter(|g| !g.installed)
        .map(|g| key(&g.id))
        .collect();
    let lower = |values: &[String]| {
        values
            .iter()
            .map(|v| v.to_lowercase())
            .collect::<HashSet<_>>()
    };
    let (mut ids, categories, plugins) = (
        lower(&settings.exclude_games),
        lower(&settings.exclude_categories),
        lower(&settings.exclude_plugins),
    );
    ids.extend(
        games
            .iter()
            .filter(|game| dismissed.contains(&library::playnite_dismissed_key(&game.id)))
            .map(|game| key(&game.id)),
    );
    let mut flags: HashMap<String, u8> = HashMap::new();
    let mut selected: Vec<&Game> = vec![];
    fn pick<'a>(
        game: &'a Game,
        source: u8,
        flags: &mut HashMap<String, u8>,
        selected: &mut Vec<&'a Game>,
    ) {
        let entry = flags.entry(key(&game.id)).or_insert(0);
        if *entry == 0 {
            selected.push(game);
        }
        *entry |= source;
    }
    if manage {
        if settings.recent_games > 0 {
            let mut ordered = installed.clone();
            ordered.sort_by_key(|game| std::cmp::Reverse(game.last_played));
            let cutoff = now.saturating_sub(settings.recent_max_age_days.saturating_mul(86400));
            let mut taken = 0;
            for game in ordered {
                if taken >= settings.recent_games {
                    break;
                }
                if excluded(game, &ids, &categories, &plugins)
                    || (settings.recent_max_age_days > 0
                        && game.last_played.is_none_or(|at| at < cutoff))
                {
                    continue;
                }
                taken += 1;
                pick(game, RECENT, &mut flags, &mut selected);
            }
        }
        let wanted = lower(&settings.sync_categories);
        let wanted_plugins = lower(&settings.sync_plugins);
        for game in installed.iter().copied() {
            if excluded(game, &ids, &categories, &plugins) {
                continue;
            }
            if game
                .categories
                .iter()
                .any(|c| wanted.contains(&c.to_lowercase()))
            {
                pick(game, CATEGORY, &mut flags, &mut selected);
            }
            if !game.plugin_id.is_empty() && wanted_plugins.contains(&key(&game.plugin_id)) {
                pick(game, PLUGIN, &mut flags, &mut selected);
            }
            if settings.sync_all_installed {
                pick(game, INSTALLED, &mut flags, &mut selected);
            }
        }
    }
    // Match existing apps: by id against every game, then by program,
    // working folder or a name only one selected game has.
    let by_id: HashMap<String, &Game> = games.iter().map(|g| (key(&g.id), g)).collect();
    let mut by_exe = HashMap::new();
    let mut by_dir = HashMap::new();
    let mut by_name: HashMap<String, Option<&Game>> = HashMap::new();
    for game in &selected {
        if !game.exe.is_empty() {
            by_exe.insert(path_key(&game.exe), *game);
        }
        if !game.working_dir.is_empty() {
            by_dir.insert(path_key(&game.working_dir), *game);
        }
        let name = name_key(&game.name);
        if !name.is_empty() {
            by_name
                .entry(name)
                .and_modify(|found| *found = None)
                .or_insert(Some(*game));
        }
    }
    let mut matched = HashSet::new();
    for app in apps.iter_mut() {
        let id = field(app, "playnite-id").to_owned();
        if owned(app) && !id.is_empty() && uninstalled.contains(&key(&id)) {
            continue;
        }
        let game = Some(id.as_str())
            .filter(|id| !id.is_empty())
            .and_then(|id| by_id.get(&key(id)).copied())
            .or_else(|| {
                Some(field(app, "cmd"))
                    .filter(|c| !c.is_empty())
                    .and_then(|c| by_exe.get(&command_program(c)).copied())
            })
            .or_else(|| {
                Some(field(app, "working-dir"))
                    .filter(|d| !d.is_empty())
                    .and_then(|d| by_dir.get(&path_key(d)).copied())
            })
            .or_else(|| {
                by_name
                    .get(&name_key(field(app, "name")))
                    .copied()
                    .flatten()
            });
        let Some(game) = game else {
            continue;
        };
        matched.insert(key(&game.id));
        if !owned(app) {
            if field(app, "image-path").is_empty()
                && let Some((Some(cover), _)) = artwork.get(&key(&game.id))
            {
                app["image-path"] = json!(cover.to_string_lossy().replace('\\', "/"));
            }
            continue;
        }
        apply_game(app, game, artwork);
        if let Some(source) = flags.get(&key(&game.id)) {
            app["playnite-source"] = json!(source_label(*source));
            app["playnite-managed"] = json!("auto");
        }
    }
    if manage {
        let selected_ids: HashSet<String> = selected.iter().map(|g| key(&g.id)).collect();
        let current: HashSet<String> = apps
            .iter()
            .filter(|app| owned(app))
            .map(|app| key(field(app, "playnite-id")))
            .filter(|id| !id.is_empty())
            .chain(matched.iter().cloned())
            .collect();
        let mut replacements = selected_ids.difference(&current).count();
        let last_played: HashMap<String, i64> = installed
            .iter()
            .filter_map(|g| g.last_played.map(|at| (key(&g.id), at)))
            .collect();
        apps.retain(|app| {
            let id = key(field(app, "playnite-id"));
            if !owned(app) || id.is_empty() {
                return true;
            }
            let added = parse_time(field(app, "playnite-added-at")).unwrap_or(now);
            let expired = settings.delete_after_days > 0
                && now >= added.saturating_add(settings.delete_after_days.saturating_mul(86400))
                && last_played.get(&id).is_none_or(|played| *played < added);
            let mut remove = (settings.remove_uninstalled && uninstalled.contains(&id)) || expired;
            if !remove && !selected_ids.contains(&id) {
                if !settings.sync_all_installed && field(app, "playnite-source") == "installed" {
                    remove = true;
                } else if settings.recent_games > 0
                    && settings.require_replacement
                    && replacements > 0
                {
                    replacements -= 1;
                    remove = true;
                }
            }
            !remove
        });
        for game in &selected {
            if matched.contains(&key(&game.id)) {
                continue;
            }
            let mut app = json!({});
            apply_game(&mut app, game, artwork);
            app["playnite-source"] = json!(source_label(flags[&key(&game.id)]));
            app["playnite-managed"] = json!("auto");
            app[library::SYNCED_BY] = json!("playnite");
            app["playnite-added-at"] = json!(format_time(now));
            apps.push(app);
        }
    }
    *apps != before
}
/// The "Playnite (Fullscreen)" app: present while `wanted`, removed
/// otherwise. An app the user made with that name or Playnite's fullscreen
/// command counts as present but is never removed: only the one marked
/// `playnite-fullscreen` is. Returns whether anything changed.
pub fn fullscreen_entry(apps: &mut Vec<Value>, wanted: bool) -> bool {
    let made_here = |app: &Value| match app.get("playnite-fullscreen") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        Some(Value::String(s)) => matches!(s.to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
        _ => false,
    };
    let is_entry = |app: &Value| {
        let cmd = field(app, "cmd").to_ascii_lowercase();
        made_here(app)
            || (cmd.contains("playnite-launcher") && cmd.contains("--fullscreen"))
            || field(app, "name") == "Playnite (Fullscreen)"
    };
    let present = apps.iter().any(is_entry);
    if wanted && !present {
        apps.push(json!({
            "name": "Playnite (Fullscreen)",
            "image-path": "playnite_boxart.png",
            "playnite-fullscreen": true,
            "auto-detach": true,
            "wait-all": true,
        }));
        return true;
    }
    if wanted {
        // The 10 earlier syncs wrote goes, so the entry follows the host's exit timeout. A 10
        // set by hand on this entry cannot be told apart from it and goes too; any other value
        // stays.
        let mut changed = false;
        for app in apps.iter_mut().filter(|app| made_here(app)) {
            if let Some(fields) = app.as_object_mut() {
                changed |= drop_written_exit_timeout(fields);
            }
        }
        return changed;
    }
    if !wanted && apps.iter().any(made_here) {
        apps.retain(|app| !made_here(app));
        return true;
    }
    false
}
/// Remove every app the sync added and still owns.
pub fn purge(apps: &mut Vec<Value>) -> usize {
    let before = apps.len();
    apps.retain(|app| !owned(app));
    before - apps.len()
}
/// Category names and plugins seen in a game list, for the console.
pub fn names(games: &[Game]) -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    let categories = games
        .iter()
        .flat_map(|g| g.categories.iter().cloned())
        .collect();
    let plugins = games
        .iter()
        .filter(|g| !g.plugin_id.is_empty())
        .map(|g| (g.plugin_id.clone(), g.plugin_name.clone()))
        .collect();
    (categories, plugins)
}
/// `Version: 0.4.14` from an `extension.yaml`.
pub fn plugin_version(manifest: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case("version")
            .then(|| {
                value
                    .trim()
                    .trim_matches(['"', '\''])
                    .trim_start_matches(['v', 'V'])
                    .to_owned()
            })
            .filter(|v| !v.is_empty())
    })
}

#[cfg(test)]
mod tests;
