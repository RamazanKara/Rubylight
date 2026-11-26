//! Store manifest parsers for Epic Games, EA app, Ubisoft Connect, and GOG.
use super::{Game, Launch, Source, same_folder, within};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

pub(super) fn identifier(value: &str) -> Result<&str> {
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
pub(super) fn relative_file(folder: &Path, value: &str) -> Result<PathBuf> {
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
/// How the EA app starts an EA game by its first content id through its own `link2ea` scheme,
/// on a PC where Origin's `origin2` scheme, which [`ea`] uses, is not registered.
pub fn ea_link2ea(content_id: &str) -> Launch {
    Launch::Uri {
        uri: format!(
            "link2ea://launchgame/{}?platform=PCWIN",
            uri_part(content_id)
        ),
    }
}
/// Standard base64, as the Epic launcher writes its catalog cache. Line breaks are skipped.
pub(super) fn base64(text: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in text {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}
/// An https address on Epic's own image servers (`cdn1.epicgames.com`), the only covers the
/// host downloads for Epic games.
fn epic_image(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| {
            let host = host.to_ascii_lowercase();
            host.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                && host.ends_with(".epicgames.com")
        })
        && !url.contains(|c: char| c.is_whitespace() || matches!(c, '"' | '\\' | '<' | '>'))
}
/// Epic's portrait box art for the items in the launcher's catalog cache
/// (`Data\Catalog\catcache.bin`, base64 of a JSON list of catalog items), by
/// `namespace:item`, how an [`epic`] game's id begins. The tall box art the launcher shows
/// (`DieselGameBoxTall`) is taken, else the store's tall offer image.
pub fn epic_covers(bytes: &[u8]) -> Result<HashMap<String, String>> {
    let json = base64(bytes).context("The Epic catalog cache is not base64.")?;
    let items: Vec<Value> = serde_json::from_slice(&json)
        .context("The Epic catalog cache is not a list of catalog items.")?;
    let mut covers = HashMap::new();
    for item in &items {
        let (Some(namespace), Some(id)) = (item["namespace"].as_str(), item["id"].as_str()) else {
            continue;
        };
        let images = item["keyImages"].as_array().map_or(&[][..], Vec::as_slice);
        let url = ["DieselGameBoxTall", "OfferImageTall"]
            .iter()
            .find_map(|kind| {
                images
                    .iter()
                    .filter(|image| image["type"] == *kind)
                    .find_map(|image| image["url"].as_str().filter(|url| epic_image(url)))
            });
        if let Some(url) = url {
            covers.insert(format!("{namespace}:{id}"), url.to_owned());
        }
    }
    Ok(covers)
}
/// The [`epic_covers`] key of an Epic game id (`namespace:item:app`).
pub fn epic_cover_key(id: &str) -> Option<String> {
    let mut parts = id.splitn(3, ':');
    let (namespace, item) = (parts.next()?, parts.next()?);
    parts.next()?;
    Some(format!("{namespace}:{item}"))
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
