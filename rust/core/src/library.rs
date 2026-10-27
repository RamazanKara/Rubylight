//! Installed launcher records and explicit, non-destructive library imports.
use anyhow::{Context, Result, bail, ensure};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
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
}

/// Which launchers the scan reads (`library_*` keys). Steam, Playnite and the
/// other launchers have their own settings; these are the ones added with the
/// game folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub epic: bool,
    pub gog: bool,
    /// Folders whose subfolders are games, one game per subfolder.
    pub local_folders: Vec<PathBuf>,
    /// Game folders the folder scan leaves out, by folder name or full path.
    pub local_exclude: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self::from_config(&crate::config::Config::default())
    }
}
impl Settings {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            epic: config.boolean("library_epic_enabled", true),
            gog: config.boolean("library_gog_enabled", true),
            local_folders: config
                .list("library_local_folders")
                .into_iter()
                .map(|folder| PathBuf::from(folder.trim()))
                .collect(),
            local_exclude: config
                .list("library_local_exclude")
                .into_iter()
                .map(|entry| entry.trim().to_owned())
                .collect(),
        }
    }
    /// Whether the scan reads `source`. Game folders are read when there are some.
    pub fn enabled(&self, source: Source) -> bool {
        match source {
            Source::Epic => self.epic,
            Source::Gog => self.gog,
            Source::Local => !self.local_folders.is_empty(),
            _ => true,
        }
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
fn name_key(name: &str) -> String {
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
                && crate::steam::app_id_of(app).is_some_and(|id| id.to_string() == self.id))
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

fn identifier(value: &str) -> Result<&str> {
    ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b)),
        "The launcher record has an invalid game identifier."
    );
    Ok(value)
}
fn uri_part(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn relative_file(folder: &Path, value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    ensure!(
        !value.is_empty()
            && !value.contains(['\0', ':'])
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        "The game record has an invalid relative file path."
    );
    Ok(folder.join(path))
}

pub fn epic(bytes: &[u8]) -> Result<Option<Game>> {
    let record: Value = serde_json::from_slice(bytes)?;
    let text = |key: &str| record[key].as_str().unwrap_or("");
    if record["bIsIncompleteInstall"] == true
        || (!text("MainGameAppName").is_empty() && text("MainGameAppName") != text("AppName"))
    {
        return Ok(None);
    }
    let app = identifier(text("AppName"))?;
    let namespace = identifier(text("CatalogNamespace"))?;
    let item = identifier(text("CatalogItemId"))?;
    ensure!(
        !text("DisplayName").trim().is_empty(),
        "The Epic game has no name."
    );
    ensure!(
        !text("InstallLocation").is_empty(),
        "The Epic game has no installation folder."
    );
    let folder = Path::new(text("InstallLocation"));
    // DLC and launcher components do not have a game executable.
    if text("LaunchExecutable").is_empty() {
        return Ok(None);
    }
    relative_file(folder, text("LaunchExecutable"))?;
    let id = format!("{namespace}:{item}:{app}");
    Ok(Some(Game::new(
        Source::Epic,
        id.clone(),
        text("DisplayName").into(),
        folder,
        Some(Launch::Uri {
            uri: format!(
                "com.epicgames.launcher://apps/{}?action=launch&silent=true",
                uri_part(&id)
            ),
        }),
    )))
}

#[derive(Deserialize)]
struct EaInstaller {
    #[serde(rename = "contentIDs")]
    content_ids: EaContentIds,
}
#[derive(Deserialize)]
struct EaContentIds {
    #[serde(rename = "contentID")]
    ids: Vec<String>,
}
pub fn ea(xml: &str, folder: &Path, name: &str) -> Result<Game> {
    let record: EaInstaller = quick_xml::de::from_str(xml)?;
    ensure!(
        !record.content_ids.ids.is_empty(),
        "The EA game has no content IDs."
    );
    let ids: Vec<_> = record
        .content_ids
        .ids
        .iter()
        .map(|id| identifier(id.trim()))
        .collect::<Result<_>>()?;
    let id = ids[0].to_owned();
    let offers = ids.into_iter().map(uri_part).collect::<Vec<_>>().join(",");
    Ok(Game::new(
        Source::Ea,
        id,
        name.into(),
        folder,
        Some(Launch::Uri {
            uri: format!("origin2://game/launch?offerIds={offers}"),
        }),
    ))
}

/// The part of `MicrosoftGame.config` that names the game and its programs.
#[derive(Deserialize)]
struct MicrosoftConfig {
    #[serde(rename = "Identity")]
    identity: MicrosoftIdentity,
    #[serde(rename = "ShellVisuals")]
    visuals: Option<MicrosoftVisuals>,
    #[serde(rename = "ExecutableList")]
    executables: Option<MicrosoftExecutables>,
}
#[derive(Deserialize)]
struct MicrosoftIdentity {
    #[serde(rename = "@Name")]
    name: String,
}
#[derive(Deserialize)]
struct MicrosoftVisuals {
    #[serde(rename = "@DefaultDisplayName", default)]
    name: String,
    #[serde(rename = "@Square150x150Logo", default)]
    logo: String,
}
#[derive(Deserialize)]
struct MicrosoftExecutables {
    #[serde(rename = "Executable", default)]
    list: Vec<MicrosoftExecutable>,
}
#[derive(Deserialize)]
struct MicrosoftExecutable {
    #[serde(rename = "@Name", default)]
    name: String,
    #[serde(rename = "@Id", default)]
    id: String,
    #[serde(rename = "@TargetDeviceFamily", default)]
    family: String,
}
/// The applications of an `AppxManifest.xml`.
#[derive(Deserialize)]
struct AppxManifest {
    #[serde(rename = "Applications")]
    applications: Option<AppxApplications>,
}
#[derive(Deserialize)]
struct AppxApplications {
    #[serde(rename = "Application", default)]
    list: Vec<AppxApplication>,
}
#[derive(Deserialize)]
struct AppxApplication {
    #[serde(rename = "@Id")]
    id: String,
    #[serde(rename = "@Executable", default)]
    executable: String,
}

/// An installed Microsoft Store or Xbox package, as Windows reports it.
pub struct StorePackage<'a> {
    /// `Name_PublisherId`.
    pub family: &'a str,
    /// The package's own display name; may be empty or a resource reference.
    pub display_name: &'a str,
    /// Its `AppxManifest.xml`.
    pub manifest: &'a str,
}

/// A name worth showing: not empty and not an unresolved `ms-resource:` reference.
fn shown_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty() && !name.to_ascii_lowercase().starts_with("ms-resource:")).then_some(name)
}
fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// An application user model id (`Name_PublisherId!App`), which `shell:AppsFolder\` takes to
/// start the package. Only the characters packages are named with, so nothing in it reaches
/// the shell as anything but one name.
fn aumid(family: &str, app: &str) -> Result<String> {
    let name_chars = |text: &str, extra: &[u8]| {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    let (name, publisher) = family
        .split_once('_')
        .context("The package family name is not Name_PublisherId.")?;
    ensure!(
        name_chars(name, b".-") && name_chars(publisher, b".-"),
        "The package family name {family:?} has characters a package name cannot have."
    );
    ensure!(
        name_chars(app, b"._-"),
        "The application id {app:?} has characters an application id cannot have."
    );
    Ok(format!("{family}!{app}"))
}

/// The application user model id in a `shell:AppsFolder\` launch address, if that is all the
/// address holds. The host checks this again at launch, since `apps.json` can be edited.
pub fn store_launch_target(uri: &str) -> Option<&str> {
    let target = uri.strip_prefix("shell:AppsFolder\\")?;
    let (family, app) = target.split_once('!')?;
    aumid(family, app).ok().map(|_| target)
}

/// A game from the `MicrosoftGame.config` in `folder` (the game's install folder, where its
/// programs are). `package` is the registered package it belongs to; without one the game is
/// listed as unavailable, since nothing can start it. A config with no program for PCs is a
/// console-only title (`None`). Nothing here bypasses a licence or a sign in: the package's
/// own entry point starts, as from the Start menu.
pub fn microsoft(
    config: &str,
    folder: &Path,
    package: Option<&StorePackage>,
) -> Result<Option<Game>> {
    let config: MicrosoftConfig = quick_xml::de::from_str(config.trim_start_matches('\u{feff}'))
        .context("MicrosoftGame.config is not readable.")?;
    let id = identifier(config.identity.name.trim())?.to_owned();
    let programs: Vec<_> = config
        .executables
        .map(|list| list.list)
        .unwrap_or_default()
        .into_iter()
        .filter(|program| program.family.is_empty() || program.family.eq_ignore_ascii_case("PC"))
        .collect();
    // A console-only title, or a config that lists no program.
    let Some(program) = programs.first() else {
        return Ok(None);
    };
    let name = shown_name(config.visuals.as_ref().map_or("", |v| &v.name))
        .or_else(|| package.and_then(|p| shown_name(p.display_name)))
        .unwrap_or(&id)
        .to_owned();
    let launch = package
        .map(|package| -> Result<Launch> {
            let manifest: AppxManifest =
                quick_xml::de::from_str(package.manifest.trim_start_matches('\u{feff}'))
                    .context("AppxManifest.xml is not readable.")?;
            let apps = manifest.applications.map(|a| a.list).unwrap_or_default();
            // The config's Id is the manifest's application Id; the program's file name is
            // the second way to find it.
            let app = apps
                .iter()
                .find(|app| !program.id.is_empty() && app.id == program.id)
                .or_else(|| {
                    apps.iter().find(|app| {
                        file_name_of(&app.executable)
                            .eq_ignore_ascii_case(file_name_of(&program.name))
                    })
                })
                .with_context(|| {
                    format!(
                        "None of the package's applications starts {}.",
                        program.name
                    )
                })?;
            Ok(Launch::Uri {
                uri: format!("shell:AppsFolder\\{}", aumid(package.family, &app.id)?),
            })
        })
        .transpose()?;
    let mut game = Game::new(Source::Microsoft, id, name, folder, launch);
    if package.is_none() {
        game.unavailable_reason = Some(
            "This game is not installed for this Windows user. Install or repair it in the Xbox app or the Microsoft Store."
                .into(),
        );
    }
    game.cover = config
        .visuals
        .and_then(|v| relative_file(folder, v.logo.trim()).ok())
        .filter(|path| path.is_file());
    Ok(Some(game))
}

// Agent product.db fields: repeated installations (1), UID (1), product (2),
// installation settings (3), and the settings' absolute installation path (1).
#[derive(Clone, PartialEq, Message)]
struct BattleDatabase {
    #[prost(message, repeated, tag = "1")]
    games: Vec<BattleInstall>,
}
#[derive(Clone, PartialEq, Message)]
struct BattleInstall {
    #[prost(string, tag = "1")]
    uid: String,
    #[prost(string, tag = "2")]
    product: String,
    #[prost(message, optional, tag = "3")]
    settings: Option<BattleSettings>,
}
#[derive(Clone, PartialEq, Message)]
struct BattleSettings {
    #[prost(string, tag = "1")]
    path: String,
}
pub fn battlenet_database(bytes: &[u8]) -> Result<Vec<(String, PathBuf)>> {
    Ok(BattleDatabase::decode(bytes)?
        .games
        .into_iter()
        .filter_map(|game| {
            let settings = game.settings?;
            if settings.path.is_empty() {
                return None;
            }
            Some((game.uid, PathBuf::from(settings.path)))
        })
        .collect())
}
pub fn battlenet(uid: &str, folder: &Path, launcher: Option<&Path>) -> Option<Game> {
    let (id, name) = match uid.to_ascii_lowercase().as_str() {
        "bna" | "agent" | "battle.net" => return None,
        "wow" => ("WoW", "World of Warcraft"),
        "diablo3" => ("D3", "Diablo III"),
        "s2" => ("S2", "StarCraft II"),
        "s1" => ("S1", "StarCraft"),
        "hs_beta" => ("WTCG", "Hearthstone"),
        "heroes" => ("Hero", "Heroes of the Storm"),
        "prometheus" => ("Pro", "Overwatch 2"),
        "viper" => ("VIPR", "Call of Duty: Black Ops 4"),
        "odin" => ("ODIN", "Call of Duty: Modern Warfare"),
        "w3" => ("W3", "Warcraft III: Reforged"),
        "lazarus" => ("LAZR", "Call of Duty: Modern Warfare 2 Campaign Remastered"),
        "zeus" => ("ZEUS", "Call of Duty: Black Ops Cold War"),
        "wlby" => ("WLBY", "Crash Bandicoot 4"),
        "osi" => ("OSI", "Diablo II: Resurrected"),
        "rtro" => ("RTRO", "Blizzard Arcade Collection"),
        "fore" => ("FORE", "Call of Duty: Vanguard"),
        "anbs" => ("ANBS", "Diablo Immortal"),
        "auks" => ("AUKS", "Call of Duty: Modern Warfare II"),
        "fen" => ("Fen", "Diablo IV"),
        "d1" => ("D1", "Diablo"),
        "w1r" => ("W1R", "Warcraft: Remastered"),
        "w2r" => ("W2R", "Warcraft II: Remastered"),
        "w1" => ("W1", "Warcraft: Orcs & Humans"),
        "w2" => ("W2", "Warcraft II: Battle.net Edition"),
        "gryphon" => ("GRY", "Warcraft Rumble"),
        "aris" => ("ARIS", "Doom: The Dark Ages"),
        "scorpio" => ("SCOR", "Sea of Thieves"),
        "arkansas" => ("ARK", "The Outer Worlds 2"),
        "libra" => ("LBRA", "Tony Hawk's Pro Skater 3 + 4"),
        "pinta" => ("PNTA", "Call of Duty: Modern Warfare III"),
        "aqua" => ("AQUA", "Avowed"),
        _ => {
            let mut game = Game::new(Source::Battlenet, uid.into(), uid.into(), folder, None);
            game.unavailable_reason =
                Some("This Battle.net product is not recognized. Add it manually.".into());
            return Some(game);
        }
    };
    Some(Game::new(
        Source::Battlenet,
        uid.to_ascii_lowercase(),
        name.into(),
        folder,
        launcher.map(|path| Launch::Program {
            program: path.to_string_lossy().into_owned(),
            arguments: format!("--exec=\"launch {id}\""),
        }),
    ))
}
pub fn ubisoft(id: &str, folder: &Path, name: &str) -> Result<Game> {
    ensure!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        "Invalid Ubisoft game ID."
    );
    Ok(Game::new(
        Source::Ubisoft,
        id.into(),
        name.into(),
        folder,
        Some(Launch::Uri {
            uri: format!("uplay://launch/{id}"),
        }),
    ))
}
pub fn gog(bytes: &[u8], folder: &Path, galaxy: Option<&Path>) -> Result<Option<Game>> {
    let record: Value = serde_json::from_slice(bytes)?;
    let id = record["gameId"]
        .as_str()
        .context("The GOG record has no game ID.")?;
    ensure!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        "Invalid GOG game ID."
    );
    if record["rootGameId"]
        .as_str()
        .is_some_and(|root| !root.is_empty() && root != id)
    {
        return Ok(None);
    }
    let name = record["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("The GOG game has no name.")?;
    let actions = record["playTasks"]
        .as_array()
        .or_else(|| record["gameActions"].as_array())
        .context("The GOG game has no play actions.")?;
    let action = actions
        .iter()
        .find(|action| {
            action["isPrimary"] == true
                && action["type"].as_str().is_none_or(|kind| {
                    kind.eq_ignore_ascii_case("file") || kind.eq_ignore_ascii_case("FileTask")
                })
        })
        .context("The GOG game has no primary play action.")?;
    let executable = relative_file(folder, action["path"].as_str().unwrap_or(""))?;
    let launch = if let Some(galaxy) = galaxy {
        Launch::Program {
            program: galaxy.to_string_lossy().into_owned(),
            arguments: format!(
                "/launchViaAutostart /gameId={id} /command=runGame /path=\"{}\"",
                folder.display()
            ),
        }
    } else {
        Launch::Program {
            program: executable.to_string_lossy().into_owned(),
            arguments: action["arguments"].as_str().unwrap_or("").to_owned(),
        }
    };
    let mut game = Game::new(Source::Gog, id.into(), name.into(), folder, Some(launch));
    if galaxy.is_none()
        && let Some(working) = action["workingDir"]
            .as_str()
            .filter(|value| !value.is_empty())
    {
        game.working_dir = relative_file(folder, working)?;
    }
    Ok(Some(game))
}

/// A GOG game from the values of its `SOFTWARE\GOG.com\Games\<id>` registry key, for an
/// install with no `goggame-<id>.info` (older offline installers write only the registry).
/// Its own program starts it: Galaxy runs a game from the play tasks in that file, which
/// such an install lacks. DLC (`dependsOn`) is not a game of its own.
pub fn gog_registry(values: &BTreeMap<String, String>) -> Result<Option<Game>> {
    let get = |key: &str| {
        values
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map_or("", |(_, value)| value.trim())
    };
    let id = get("gameID");
    ensure!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        "Invalid GOG game ID."
    );
    if !get("dependsOn").is_empty() {
        return Ok(None);
    }
    let name = get("gameName");
    ensure!(!name.is_empty(), "The GOG game {id} has no name.");
    let folder = get("path");
    ensure!(
        !folder.is_empty(),
        "The GOG game {name} has no installation folder."
    );
    let program = get("exe");
    ensure!(
        within(program, folder) && !same_folder(program, folder),
        "The GOG game {name} has no program in its installation folder."
    );
    let launch = Launch::Program {
        program: program.into(),
        arguments: get("launchParam").into(),
    };
    let mut game = Game::new(
        Source::Gog,
        id.into(),
        name.into(),
        Path::new(folder),
        Some(launch),
    );
    let working = get("workingDir");
    if within(working, &game.install_dir) {
        game.working_dir = working.into();
    }
    Ok(Some(game))
}

/// A program below a game folder: its path relative to the folder, and its size.
pub struct Program {
    pub path: PathBuf,
    pub size: u64,
}
/// Folders below a game folder that never hold the game's own program.
pub const LOCAL_SKIPPED_FOLDERS: [&str; 16] = [
    "_commonredist",
    "commonredist",
    "redist",
    "redistributables",
    "_redist",
    "__installer",
    "installer",
    "installers",
    "directx",
    "dotnet",
    "vcredist",
    "prereqs",
    "support",
    "easyanticheat",
    "battleye",
    "crashreporter",
];
/// Whether a program (by file name) is part of a game's install, not the game: setup,
/// uninstallers, redistributables, crash reporters, updaters and anti-cheat services.
fn helper_program(file: &str) -> bool {
    let stem = file.to_lowercase();
    let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
    [
        "unins",
        "setup",
        "install",
        "redist",
        "directx",
        "dotnet",
        "crashhandler",
        "crashpad",
        "crashsender",
        "crashdump",
        "bugreport",
        "errorreport",
        "reporter",
        "update",
        "patcher",
        "prereq",
        "easyanticheat",
        "battleye",
        "uploader",
        "helper",
        "cefprocess",
        "touchup",
        "cleanup",
        "activation",
        "config",
    ]
    .iter()
    .any(|part| stem.contains(part))
        || matches!(stem, "eac" | "be" | "be_service" | "dxsetup")
}
/// The game in `folder`, a subfolder of one of the configured game folders, started by
/// the program `programs` lists below it (at most two folders deep). Helpers
/// ([`helper_program`]) and anything in [`LOCAL_SKIPPED_FOLDERS`] are never it. The program
/// nearest the top wins, then one named like the folder, then the largest. The game is
/// known by its folder, so a newer program does not make it a new game.
pub fn local(folder: &Path, programs: &[Program]) -> Option<Game> {
    let name = folder.file_name()?.to_string_lossy().trim().to_owned();
    let title = name_key(&name);
    let program = programs
        .iter()
        .filter(|program| {
            let mut parts = program.path.components().rev();
            let file = parts.next().map(|part| part.as_os_str().to_string_lossy());
            file.is_some_and(|file| file.to_lowercase().ends_with(".exe") && !helper_program(&file))
                && program.path.components().count() <= 3
                && parts.all(|part| {
                    matches!(part, Component::Normal(_))
                        && !LOCAL_SKIPPED_FOLDERS
                            .contains(&part.as_os_str().to_string_lossy().to_lowercase().as_str())
                })
        })
        .min_by_key(|program| {
            let stem = name_key(
                &program
                    .path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
            );
            let named = stem.len() >= 3 && (title.contains(&stem) || stem.contains(&title));
            (
                program.path.components().count(),
                !named,
                std::cmp::Reverse(program.size),
                program.path.clone(),
            )
        })?;
    let program = folder.join(program.path.components().collect::<PathBuf>());
    let mut game = Game::new(
        Source::Local,
        folder_key(&folder.to_string_lossy()),
        name,
        folder,
        Some(Launch::Program {
            program: program.to_string_lossy().into_owned(),
            arguments: String::new(),
        }),
    );
    if let Some(parent) = program.parent() {
        game.working_dir = parent.into();
    }
    Some(game)
}

/// The scan's games once every source is read, sorted by name: one per launcher and id,
/// and no game-folder entry for a folder a launcher already lists or one that holds a
/// launcher's games (a library, not a game). A launcher's folder that holds game folders
/// (a record pointing at `D:\Games`) hides none of them.
pub fn finish(mut games: Vec<Game>) -> Vec<Game> {
    let mut seen = std::collections::BTreeSet::new();
    games.retain(|game| seen.insert((game.source, game.id.clone())));
    let launcher_folders: Vec<String> = games
        .iter()
        .filter(|game| game.source != Source::Local)
        .map(|game| game.install_dir.clone())
        .collect();
    games.retain(|game| {
        game.source != Source::Local
            || !launcher_folders
                .iter()
                .any(|folder| within(folder, &game.install_dir))
    });
    games.sort_by_cached_key(|game| (game.name.to_lowercase(), game.source, game.id.clone()));
    games
}

#[cfg(test)]
mod tests;
