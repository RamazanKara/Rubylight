//! Installed launcher records and explicit, non-destructive library imports.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod battlenet;
mod local;
mod microsoft;
mod stores;
mod sync;

pub use battlenet::{
    battlenet, battlenet_database, battlenet_entries, battlenet_last_played, battlenet_uninstall,
};
pub use local::{LOCAL_SKIPPED_FOLDERS, Program, finish, local};
pub use microsoft::{StorePackage, microsoft, store_launch_target};
pub use stores::{ea, epic, epic_cover_key, epic_covers, gog, gog_registry, ubisoft};
pub use sync::{
    AppIndex, DISMISSED, MANAGED, SYNC_OWNER_VERSION, SYNCED_BY, Stamped, SyncChange, chosen,
    dismissal_keys, dismissed_key, hand_over, managed_key, playnite_dismissed_key, reconcile,
    reconcile_at, stamp_synced, synced_by,
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Steam,
    Epic,
    Battlenet,
    Ea,
    Ubisoft,
    Gog,
    Microsoft,
    Local,
}
impl Source {
    pub const ALL: [Self; 8] = [
        Self::Steam,
        Self::Epic,
        Self::Battlenet,
        Self::Ea,
        Self::Ubisoft,
        Self::Gog,
        Self::Microsoft,
        Self::Local,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::Epic => "epic",
            Self::Battlenet => "battlenet",
            Self::Ea => "ea",
            Self::Ubisoft => "ubisoft",
            Self::Gog => "gog",
            Self::Microsoft => "microsoft",
            Self::Local => "local",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Steam => "Steam",
            Self::Epic => "Epic Games",
            Self::Battlenet => "Battle.net",
            Self::Ea => "EA app",
            Self::Ubisoft => "Ubisoft Connect",
            Self::Gog => "GOG",
            Self::Microsoft => "Microsoft Store and Xbox",
            Self::Local => "Game folders",
        }
    }
    /// What Playnite calls the library plugin for this launcher, now and before the
    /// launcher was renamed, in [`name_key`] form.
    fn playnite_plugins(self) -> &'static [&'static str] {
        match self {
            Self::Steam => &["steam"],
            Self::Epic => &["epic"],
            Self::Battlenet => &["battlenet"],
            Self::Ea => &["ea", "origin"],
            Self::Ubisoft => &["ubisoft", "uplay"],
            Self::Gog => &["gog"],
            Self::Microsoft => &["xbox"],
            Self::Local => &[],
        }
    }
    /// The launcher whose library a Playnite library plugin (`playnite-plugin-name`) reads.
    pub fn of_playnite_plugin(plugin: &str) -> Option<Self> {
        let plugin = name_key(plugin);
        Self::ALL.into_iter().find(|source| {
            source
                .playnite_plugins()
                .iter()
                .any(|launcher| plugin.starts_with(launcher))
        })
    }
}

/// The sources the launcher sync keeps in step, each with Steam's options from the shared
/// Library sync group and its own switch ([`Options`]): every launcher but Steam, which keeps
/// its own sync (`crate::steam`), and the game folders.
pub const SYNCED: [Source; 7] = [
    Source::Epic,
    Source::Battlenet,
    Source::Ea,
    Source::Ubisoft,
    Source::Gog,
    Source::Microsoft,
    Source::Local,
];
impl Source {
    /// Whether the launcher records when each game was last played in a file the scan reads,
    /// which its recent-games options need ([`Game::last_played`]). Battle.net keeps it in
    /// `Battle.net.config`; Epic, EA, Ubisoft and the Microsoft Store keep none on the PC, GOG
    /// Galaxy only in its own database, and a game folder none at all.
    pub fn keeps_play_history(self) -> bool {
        matches!(self, Self::Steam | Self::Battlenet)
    }
    /// What the source's sync options are named after: `epic_auto_sync`, …, and
    /// `library_local_auto_sync` for the game folders, beside `library_local_folders`.
    fn option_prefix(self) -> &'static str {
        match self {
            Self::Local => "library_local",
            _ => self.key(),
        }
    }
}

/// One source's sync options, Steam's (`steam_*`) under the launcher's own key: a launcher's
/// switch is `library_<launcher>_enabled` and games to leave out are `<launcher>_exclude_games`.
/// The rest is the one shared Library sync group (`library_auto_sync`,
/// `library_autosync_remove_uninstalled`, `library_sync_all_installed`, `library_recent_games`
/// and `library_recent_max_age_days`); a launcher whose earlier `<launcher>_*` key is still set
/// keeps it as an override of the shared value. Defaults are Steam's. The game folders are on
/// while there are some and leave folders out with `library_local_exclude` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The scan reads the launcher and the sync keeps its games.
    pub enabled: bool,
    /// The automatic sync keeps its games in step; Sync now does either way, as for Steam.
    pub auto_sync: bool,
    /// A synced game that is no longer installed leaves.
    pub remove_uninstalled: bool,
    /// Every installed game; off, only the most recently played (launchers that keep a play
    /// history, [`Source::keeps_play_history`]; always on for the others).
    pub sync_all_installed: bool,
    /// How many recently played games, when not every installed one.
    pub recent_games: usize,
    /// Recent games played longer ago than this many days are left out; 0 keeps them all.
    pub recent_max_age_days: u64,
    /// Games the sync never adds, by launcher id or name, as `steam_exclude_games`.
    pub exclusions: Vec<(String, String)>,
}
impl Options {
    fn from_config(config: &butterpollo_config::config::Config, source: Source) -> Self {
        let prefix = source.option_prefix();
        let name = |option: &str| format!("{prefix}_{option}");
        let count =
            |option: &str, default: i64| config.integer(&name(option), default).max(0) as u64;
        let local = source == Source::Local;
        // The shared Library sync value, which a launcher's own key of earlier
        // releases overrides (kept when the launcher differed at the fold).
        let shared_boolean =
            |option: &str, default: bool| config.boolean(&format!("library_{option}"), default);
        let shared_count =
            |option: &str, default: i64| config.integer(&format!("library_{option}"), default);
        Self {
            // Game folders: on while there are some ([`Settings::from_config`]).
            enabled: local || config.boolean(&format!("library_{prefix}_enabled"), true),
            auto_sync: config.boolean(&name("auto_sync"), shared_boolean("auto_sync", true)),
            remove_uninstalled: config.boolean(
                &name("autosync_remove_uninstalled"),
                shared_boolean("autosync_remove_uninstalled", true),
            ),
            sync_all_installed: !source.keeps_play_history()
                || config.boolean(
                    &name("sync_all_installed"),
                    shared_boolean("sync_all_installed", true),
                ),
            recent_games: count("recent_games", shared_count("recent_games", 10)) as usize,
            recent_max_age_days: count(
                "recent_max_age_days",
                shared_count("recent_max_age_days", 30),
            ),
            exclusions: if local {
                vec![]
            } else {
                crate::steam::id_names(config.get(&name("exclude_games"), ""))
            },
        }
    }
    /// Whether the sync leaves out the game `id` named `name`: an entry with its id, or its name
    /// as [`name_key`] compares names. A plain entry is either.
    pub fn excludes(&self, id: &str, name: &str) -> bool {
        let same_name = |entry: &str| {
            let entry = name_key(entry);
            !entry.is_empty() && entry == name_key(name)
        };
        self.exclusions.iter().any(|(entry_id, entry_name)| {
            (!entry_id.is_empty() && (entry_id.eq_ignore_ascii_case(id) || same_name(entry_id)))
                || same_name(entry_name)
        })
    }
}

/// Which launchers the scan reads and how the launcher sync keeps each one (`library_*` and
/// `<launcher>_*` keys, [`Options`]), and the game folders Add games lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The options of each of [`SYNCED`].
    pub sources: BTreeMap<Source, Options>,
    /// Folders whose subfolders are games, one game per subfolder.
    pub local_folders: Vec<PathBuf>,
    /// Game folders the folder scan leaves out, by folder name or full path.
    pub local_exclude: Vec<String>,
    /// Steam's switch, `steam_enabled` as Steam's own sync reads it (`crate::steam::Settings`):
    /// off, the scan skips Steam as it skips any launcher that is off.
    pub steam: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self::from_config(&butterpollo_config::config::Config::default())
    }
}
impl Settings {
    pub fn from_config(config: &butterpollo_config::config::Config) -> Self {
        let local_folders: Vec<PathBuf> = config
            .list("library_local_folders")
            .into_iter()
            .map(|folder| PathBuf::from(folder.trim()))
            .collect();
        let mut sources: BTreeMap<Source, Options> = SYNCED
            .into_iter()
            .map(|source| (source, Options::from_config(config, source)))
            .collect();
        if let Some(local) = sources.get_mut(&Source::Local) {
            local.enabled = !local_folders.is_empty();
        }
        Self {
            sources,
            local_folders,
            local_exclude: config
                .list("library_local_exclude")
                .into_iter()
                .map(|entry| entry.trim().to_owned())
                .collect(),
            steam: crate::steam::Settings::from_config(config).enabled,
        }
    }
    /// A synced source's options; `None` for Steam.
    pub fn options(&self, source: Source) -> Option<&Options> {
        self.sources.get(&source)
    }
    /// Whether the scan reads `source`. Game folders are read when there are some, and Steam
    /// while its own switch is on, so the Library's switch shows the setting.
    pub fn enabled(&self, source: Source) -> bool {
        match source {
            Source::Local => !self.local_folders.is_empty(),
            Source::Steam => self.steam,
            _ => self.options(source).is_some_and(|options| options.enabled),
        }
    }
    /// What the automatic sync keeps: the sources that are on and sync automatically. One it
    /// skips keeps its apps as they are, as one that is off does. Sync now keeps them all.
    pub fn automatic(&self) -> Self {
        let mut automatic = self.clone();
        for options in automatic.sources.values_mut() {
            options.enabled &= options.auto_sync;
        }
        automatic
    }
    /// Whether any source is on and syncs automatically.
    pub fn any_automatic(&self) -> bool {
        self.sources
            .values()
            .any(|options| options.enabled && options.auto_sync)
    }
    /// Whether the sync keeps `source`'s games: it is on, and not Steam, which syncs itself.
    fn syncs(&self, source: Source) -> Option<&Options> {
        self.options(source).filter(|options| options.enabled)
    }
    /// Whether the folder scan leaves out the game folder `folder`.
    pub fn excluded(&self, folder: &Path) -> bool {
        let name = folder.file_name().map(|name| name.to_string_lossy());
        self.local_exclude.iter().any(|entry| {
            same_folder(entry, &folder.to_string_lossy())
                || name.as_deref().is_some_and(|name| same_folder(entry, name))
        })
    }
    /// Whether the folder scan reads the game folder a game-folder id names: a
    /// subfolder of a configured folder that is not left out. The scan says
    /// nothing about one it does not read.
    pub fn reads_local(&self, id: &str) -> bool {
        let folder = folder_key(id);
        folder.rsplit_once('\\').is_some_and(|(parent, _)| {
            self.local_folders
                .iter()
                .any(|root| same_folder(&root.to_string_lossy(), parent))
        }) && !self.excluded(Path::new(&folder))
    }
}

/// Letters and digits only, in lower case: "DOOM Eternal™" and "Doom Eternal" are one name.
pub(crate) fn name_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
/// A folder path compared as Windows does: any case, either slash, no trailing slash.
fn folder_key(path: &str) -> String {
    path.trim()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}
fn same_folder(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && folder_key(a) == folder_key(b)
}
/// Whether `path` is `folder` or inside it. A path that climbs out with `..` is in no folder.
fn within(path: &str, folder: &str) -> bool {
    let (path, folder) = (folder_key(path), folder_key(folder));
    !folder.is_empty()
        && ![&path, &folder]
            .iter()
            .any(|key| key.split('\\').any(|part| part == ".."))
        && (path == folder
            || path
                .strip_prefix(&folder)
                .is_some_and(|rest| rest.starts_with('\\')))
}
/// The program a command line starts: its quoted first part, or everything up to `.exe`.
fn command_program(command: &str) -> &str {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        return rest.split('"').next().unwrap_or_default();
    }
    command
        .to_ascii_lowercase()
        .find(".exe")
        .map_or(command, |end| &command[..end + 4])
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Launch {
    Uri { uri: String },
    Program { program: String, arguments: String },
}
impl Launch {
    pub fn command(&self) -> String {
        match self {
            Self::Uri { uri } => uri.clone(),
            Self::Program { program, arguments } => {
                format!("\"{program}\" {arguments}").trim().into()
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Game {
    pub source: Source,
    pub id: String,
    pub name: String,
    pub install_dir: String,
    pub unavailable_reason: Option<String>,
    pub already_added: bool,
    #[serde(skip)]
    pub launch: Option<Launch>,
    #[serde(skip)]
    pub cover: Option<PathBuf>,
    #[serde(skip)]
    pub working_dir: PathBuf,
    /// The launcher's cover on its own image server, when it names one and the PC has none:
    /// Epic's portrait box art. The host downloads it once, as Steam's store portrait.
    #[serde(skip)]
    pub cover_url: Option<String>,
    /// When the game was last played (Unix seconds), for a launcher that records it
    /// ([`Source::keeps_play_history`]); `None` for one never played or not known.
    #[serde(skip)]
    pub last_played: Option<u64>,
}
impl Game {
    pub fn new(
        source: Source,
        id: String,
        name: String,
        folder: &Path,
        launch: Option<Launch>,
    ) -> Self {
        Self {
            source,
            id,
            name,
            install_dir: folder.to_string_lossy().into_owned(),
            unavailable_reason: launch
                .is_none()
                .then(|| "The launcher could not be found.".into()),
            already_added: false,
            launch,
            cover: None,
            working_dir: folder.into(),
            cover_url: None,
            last_played: None,
        }
    }
    pub fn uuid(&self) -> String {
        if self.source == Source::Steam
            && let Ok(id) = self.id.parse()
        {
            return crate::steam::canonical_uuid(id);
        }
        uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("rubylight:library:{}:{}", self.source.key(), self.id).as_bytes(),
        )
        .to_string()
    }
    pub fn matches(&self, app: &Value) -> bool {
        (app["launcher-source"].as_str() == Some(self.source.key())
            && app["launcher-id"].as_str() == Some(&self.id))
            || app["uuid"].as_str() == Some(&self.uuid())
            || (self.source == Source::Steam
                && crate::steam::game_of(app).is_some_and(|id| id.to_string() == self.id))
            || self.synced_copy(app)
    }
    /// An app Steam or Playnite already keeps for this game, or one that starts
    /// from this game folder. Playnite starts its games itself and keeps no
    /// folder, so its apps are known by name and library plugin. The same title
    /// from two launchers stays two games.
    fn synced_copy(&self, app: &Value) -> bool {
        let text = |key: &str| app[key].as_str().unwrap_or("");
        let name = name_key(&self.name);
        let same_name = !name.is_empty() && name_key(text("name")) == name;
        if same_name && !text("playnite-id").is_empty() {
            let plugin = name_key(text("playnite-plugin-name"));
            let launchers = self.source.playnite_plugins();
            return launchers.is_empty()
                || launchers
                    .iter()
                    .any(|launcher| plugin.starts_with(launcher));
        }
        self.source == Source::Local
            && ((same_name && !text("steam-id").is_empty())
                || ["working-dir", "launcher-install-dir", "steam-install-dir"]
                    .into_iter()
                    .map(text)
                    .chain([command_program(text("cmd"))])
                    .any(|path| within(path, &self.install_dir)))
    }
    fn app(&self) -> Result<Value> {
        if let Some(reason) = &self.unavailable_reason {
            bail!("{reason}");
        }
        let launch = self
            .launch
            .as_ref()
            .context("No launch action was found.")?;
        let command = if self.source == Source::Steam {
            crate::steam::launch_command(self.id.parse()?)
        } else {
            launch.command()
        };
        let mut app = json!({
            "uuid": self.uuid(), "name": self.name, "cmd": command,
            "working-dir": self.working_dir, "auto-detach": true,
            "launcher-source": self.source, "launcher-id": self.id,
            "launcher-install-dir": self.install_dir,
            "launcher-command": command, "launcher-launch": launch,
        });
        if let Some(cover) = &self.cover {
            app["image-path"] = json!(cover.to_string_lossy().replace('\\', "/"));
        }
        if self.source == Source::Steam {
            app["steam-id"] = json!(self.id);
            app["steam-managed"] = json!("manual");
            app["steam-install-dir"] = json!(self.install_dir);
        }
        Ok(app)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Selection {
    pub source: Source,
    pub id: String,
}
#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub source: Source,
    pub id: String,
    pub result: &'static str,
    pub error: Option<String>,
}

pub fn import(apps: &mut Vec<Value>, games: &[Game], selected: &[Selection]) -> Vec<ImportResult> {
    selected
        .iter()
        .map(|selection| {
            let mut result = ImportResult {
                source: selection.source,
                id: selection.id.clone(),
                result: "failed",
                error: None,
            };
            let Some(game) = games
                .iter()
                .find(|g| g.source == selection.source && g.id == selection.id)
            else {
                result.error = Some(
                    "The game is no longer installed or could not be read. Scan again.".into(),
                );
                return result;
            };
            if apps.iter().any(|app| game.matches(app)) {
                result.result = "already_present";
            } else {
                match game.app() {
                    Ok(app) => {
                        apps.push(app);
                        result.result = "added";
                    }
                    Err(error) => result.error = Some(error.to_string()),
                }
            }
            result
        })
        .collect()
}

#[cfg(test)]
mod tests;
