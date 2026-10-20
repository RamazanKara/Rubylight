//! Installed launcher records and explicit, non-destructive library imports.
use anyhow::{Context, Result, bail, ensure};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
}
impl Source {
    pub const ALL: [Self; 6] = [
        Self::Steam,
        Self::Epic,
        Self::Battlenet,
        Self::Ea,
        Self::Ubisoft,
        Self::Gog,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::Epic => "epic",
            Self::Battlenet => "battlenet",
            Self::Ea => "ea",
            Self::Ubisoft => "ubisoft",
            Self::Gog => "gog",
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
        }
    }
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

#[cfg(test)]
mod tests;
