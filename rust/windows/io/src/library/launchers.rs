//! Per-launcher scan implementations for Steam, Epic, Battle.net, EA, Ubisoft, and GOG.
use super::{
    add, bytes, files,
    registry::{Installed, folders, locations, protocol, records},
};
use crate::steam::registry_string;
use anyhow::Context;
use butterpollo_session::library::{self, Game, Launch, Source};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// The uninstall entry's name for a launcher's game folder, compared as Windows compares paths
/// (any case, either slash, a trailing slash or quotes: the EA app, Ubisoft Connect and the
/// uninstall entry each write the folder their own way), else the folder's own name.
pub(super) fn display_name(installs: &[Installed], folder: &Path) -> String {
    let folder_text = folder.to_string_lossy();
    installs
        .iter()
        .find(|item| {
            !item.name.is_empty()
                && library::same_folder(&item.folder.to_string_lossy(), &folder_text)
        })
        .map(|item| item.name.clone())
        .unwrap_or_else(|| {
            folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
}

pub(super) fn program_folders() -> Vec<PathBuf> {
    let user = crate::process::user_environment().unwrap_or_default();
    ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(|name| {
            user.get(name)
                .map(PathBuf::from)
                .or_else(|| std::env::var_os(name).map(PathBuf::from))
        })
        .collect()
}

/// The signed-in user's `Battle.net.config`, in their roaming application data.
fn battlenet_config() -> Option<PathBuf> {
    crate::process::user_environment()
        .ok()
        .and_then(|user| user.get("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .map(|roaming| roaming.join(r"Battle.net\Battle.net.config"))
}

fn launcher(installs: &[Installed], name: &str, relative: &[&str]) -> Option<PathBuf> {
    let paths = installs
        .iter()
        .filter(|item| item.name.to_lowercase().contains(name))
        .filter(|item| !item.folder.as_os_str().is_empty() && super::on_this_pc(&item.folder))
        .flat_map(|item| relative.iter().map(|file| item.folder.join(file)));
    paths.into_iter().find(|path| path.is_file())
}

/// The values of a `SOFTWARE\GOG.com\Games\<id>` key that [`library::gog_registry`] reads.
const GOG_VALUES: [&str; 7] = [
    "gameID",
    "gameName",
    "path",
    "exe",
    "launchParam",
    "workingDir",
    "dependsOn",
];

/// Whether a file is a GOG game's `goggame-<id>.info`.
fn gog_info(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.starts_with("goggame-") && name.ends_with(".info")
    })
}

/// The GOG games only the registry knows. A record whose folder has a
/// `goggame-*.info` is left to that file, which is the game or makes the record a
/// DLC or edition of one.
pub(super) fn gog_registered(
    registered: &[BTreeMap<String, String>],
    games: &mut Vec<Game>,
    errors: &mut Vec<String>,
) {
    for values in registered {
        let id = values.get("gameID").map(|id| id.trim());
        // The .info scan already reported a folder it could not read.
        let has_info = values.get("path").is_some_and(|folder| {
            let folder = Path::new(folder.trim());
            super::on_this_pc(folder)
                && files(folder, &mut vec![]).iter().any(|path| gog_info(path))
        });
        if has_info
            || games
                .iter()
                .any(|game| game.source == Source::Gog && Some(game.id.as_str()) == id)
        {
            continue;
        }
        add(library::gog_registry(values), games, errors);
    }
}

pub(super) fn scan_steam(games: &mut Vec<Game>, errors: &mut Vec<String>) -> bool {
    let mut roots = crate::steam::roots();
    for (root, key) in locations(r"SOFTWARE\Valve\Steam") {
        for name in ["SteamPath", "InstallPath"] {
            if let Some(folder) = registry_string(root, &key, name) {
                let candidate = PathBuf::from(folder);
                if !super::on_this_pc(&candidate) {
                    continue;
                }
                let Ok(canonical) = std::fs::canonicalize(&candidate) else {
                    continue;
                };
                if !super::on_this_pc(&canonical) {
                    continue;
                }
                let clean = canonical.to_string_lossy();
                let clean = PathBuf::from(clean.strip_prefix(r"\\?\").unwrap_or(&clean));
                let exists = clean.join("steamapps").is_dir() || clean.join("steam.exe").is_file();
                if exists
                    && !roots.iter().any(|existing| {
                        crate::user_folder::same_name(existing.as_os_str(), clean.as_os_str())
                    })
                {
                    roots.push(clean);
                }
            }
        }
    }
    for game in butterpollo_session::steam::discover(&roots) {
        if !butterpollo_session::steam::importable(&game, false) {
            continue;
        }
        let mut entry = Game::new(
            Source::Steam,
            game.appid.to_string(),
            game.name.clone(),
            Path::new(&game.install_dir),
            Some(Launch::Uri {
                uri: format!("steam://rungameid/{}", game.appid),
            }),
        );
        entry.cover = game.artwork().map(Path::to_path_buf);
        add(Ok(Some(entry)), games, errors);
    }
    !roots.is_empty()
}

pub(super) fn scan_epic(
    program_data: &Path,
    games: &mut Vec<Game>,
    errors: &mut Vec<String>,
) -> bool {
    let folder = program_data.join(r"Epic\EpicGamesLauncher\Data\Manifests");
    let can_launch = protocol("com.epicgames.launcher");
    if !super::on_this_pc(&folder) {
        return can_launch;
    }
    // The launcher's box art, from its catalog cache. Covers are a nicety: a cache
    // that cannot be read leaves the games without, and the scan is not incomplete.
    let catalog = program_data.join(r"Epic\EpicGamesLauncher\Data\Catalog\catcache.bin");
    let covers = if super::on_this_pc(&catalog) && catalog.is_file() {
        bytes(&catalog)
            .and_then(|data| library::epic_covers(&data))
            .unwrap_or_else(|error| {
                tracing::debug!(error = %format!("{error:#}"), "Epic box art unavailable");
                Default::default()
            })
    } else {
        Default::default()
    };
    for path in files(&folder, errors)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "item"))
    {
        let result = bytes(&path)
            .and_then(|data| library::epic(&data))
            .with_context(|| path.display().to_string());
        add(
            result.map(|game| {
                game.map(|mut game| {
                    if !can_launch {
                        game.unavailable_reason = Some(
                            "Install or repair Epic Games Launcher to launch this game.".into(),
                        );
                    }
                    game.cover_url =
                        library::epic_cover_key(&game.id).and_then(|key| covers.get(&key).cloned());
                    game
                })
            }),
            games,
            errors,
        );
    }
    can_launch
}

pub(super) fn scan_battlenet(
    installs: &[Installed],
    program_folders: &[PathBuf],
    program_data: &Path,
    games: &mut Vec<Game>,
    errors: &mut Vec<String>,
) -> bool {
    let executable = launcher(
        installs,
        "battle.net",
        &["Battle.net Launcher.exe", "Battle.net.exe"],
    )
    .or_else(|| {
        program_folders
            .iter()
            .filter(|root| super::on_this_pc(root))
            .flat_map(|root| {
                ["Battle.net Launcher.exe", "Battle.net.exe"]
                    .map(|file| root.join("Battle.net").join(file))
            })
            .find(|path| path.is_file())
    });
    let database = program_data.join(r"Battle.net\Agent\product.db");
    let mut entries = vec![];
    if super::on_this_pc(&database) && database.exists() {
        match bytes(&database).and_then(|data| library::battlenet_database(&data)) {
            Ok(found) => entries.extend(found),
            Err(error) => errors.push(format!("Battle.net product database: {error}")),
        }
    }
    let registry = installs
        .iter()
        .filter(|item| item.uninstall.to_lowercase().contains("battle.net"))
        .filter_map(|item| {
            let (uid, product) = library::battlenet_uninstall(&item.uninstall)?;
            Some((uid, product, item.folder.clone()))
        })
        .collect();
    let entries = library::battlenet_entries(entries, registry);
    // When each game was last played, for the recent-games options: the signed-in
    // user's Battle.net.config. Without one no game counts as played.
    let played = battlenet_config()
        .filter(|path| super::on_this_pc(path) && path.is_file())
        .and_then(|path| {
            bytes(&path)
                .and_then(|data| library::battlenet_last_played(&data))
                .map_err(|error| {
                    tracing::debug!(error = %format!("{error:#}"), "Battle.net play history unavailable")
                })
                .ok()
        })
        .unwrap_or_default();
    for (uid, product, folder) in entries {
        let mut game = library::battlenet(&uid, &product, &folder, executable.as_deref());
        if let Some(game) = &mut game {
            game.last_played = played.get(&game.id).copied();
        }
        add(Ok(game), games, errors);
    }
    executable.is_some()
}

pub(super) fn scan_ea(
    installs: &[Installed],
    program_folders: &[PathBuf],
    games: &mut Vec<Game>,
    errors: &mut Vec<String>,
) -> bool {
    let mut candidates: Vec<PathBuf> = installs
        .iter()
        .filter(|item| item.publisher.to_lowercase().contains("electronic arts"))
        .map(|item| item.folder.clone())
        .filter(|path| !path.as_os_str().is_empty())
        .collect();
    for key in [
        r"SOFTWARE\EA Games",
        r"SOFTWARE\Origin Games",
        r"SOFTWARE\EA SPORTS",
        r"SOFTWARE\Electronic Arts\EA Games",
    ] {
        candidates.extend(folders(key, errors).into_iter().map(|(_, folder)| folder));
    }
    for root in program_folders
        .iter()
        .filter(|root| super::on_this_pc(root))
    {
        for folder in ["EA Games", "Origin Games"] {
            candidates.extend(
                files(&root.join(folder), errors)
                    .into_iter()
                    .filter(|path| path.is_dir()),
            );
        }
    }
    let (origin2, link2ea) = (protocol("origin2"), protocol("link2ea"));
    let can_launch = origin2 || link2ea;
    for folder in unique_folders(candidates) {
        if !super::on_this_pc(&folder) {
            continue;
        }
        let manifest = folder.join(r"__Installer\installerdata.xml");
        if !manifest.exists() {
            continue;
        }
        let name = display_name(installs, &folder);
        let result = bytes(&manifest)
            .and_then(|data| library::ea(std::str::from_utf8(&data)?, &folder, &name))
            .map(|mut game| {
                if !can_launch {
                    game.unavailable_reason =
                        Some("Install or repair the EA app to launch this game.".into());
                } else if !origin2 {
                    // The EA app without Origin's old scheme: its own.
                    game.launch = Some(library::ea_link2ea(&game.id));
                }
                Some(game)
            });
        add(result, games, errors);
    }
    can_launch
}

/// The folders, each once however the registry and the folder listing spell it (case,
/// slashes, a trailing slash, quotes), in a stable order.
pub(super) fn unique_folders(folders: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut unique: Vec<PathBuf> = folders
        .into_iter()
        .map(|folder| PathBuf::from(folder.to_string_lossy().trim().trim_matches('"').trim()))
        .filter(|folder| seen.insert(library::folder_key(&folder.to_string_lossy())))
        .collect();
    unique.sort_by_key(|folder| library::folder_key(&folder.to_string_lossy()));
    unique
}

pub(super) fn scan_ubisoft(
    installs: &[Installed],
    games: &mut Vec<Game>,
    errors: &mut Vec<String>,
) -> bool {
    let can_launch = protocol("uplay");
    let entries = folders(r"SOFTWARE\Ubisoft\Launcher\Installs", errors);
    let available = can_launch;
    for (id, folder) in entries {
        let name = display_name(installs, &folder);
        add(
            library::ubisoft(&id, &folder, &name).map(|mut game| {
                if !can_launch {
                    game.unavailable_reason =
                        Some("Install or repair Ubisoft Connect to launch this game.".into());
                }
                Some(game)
            }),
            games,
            errors,
        );
    }
    available
}

pub(super) fn scan_gog(
    installs: &[Installed],
    program_folders: &[PathBuf],
    games: &mut Vec<Game>,
    errors: &mut Vec<String>,
) -> bool {
    let galaxy = locations(r"SOFTWARE\GOG.com\GalaxyClient\paths")
        .into_iter()
        .filter_map(|(root, key)| registry_string(root, &key, "client"))
        .map(PathBuf::from)
        .filter(|path| super::on_this_pc(path))
        .map(|path| {
            if path.is_dir() {
                path.join("GalaxyClient.exe")
            } else {
                path
            }
        })
        .find(|path| path.is_file())
        .or_else(|| {
            program_folders
                .iter()
                .filter(|root| super::on_this_pc(root))
                .map(|root| root.join(r"GOG Galaxy\GalaxyClient.exe"))
                .find(|path| path.is_file())
        });
    // Each game's registry values, for installs without a goggame-<id>.info.
    let registered: Vec<BTreeMap<String, String>> = records(r"SOFTWARE\GOG.com\Games", errors)
        .into_iter()
        .map(|(root, path, id)| {
            let mut values: BTreeMap<String, String> = GOG_VALUES
                .into_iter()
                .filter_map(|name| {
                    registry_string(root, &path, name).map(|value| (name.to_owned(), value))
                })
                .collect();
            values.entry("gameID".into()).or_insert(id);
            values
        })
        .collect();
    let mut folders: BTreeSet<PathBuf> = registered
        .iter()
        .filter_map(|values| values.get("path"))
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| super::on_this_pc(path))
        .collect();
    folders.extend(
        installs
            .iter()
            .filter(|item| {
                item.publisher.to_lowercase().contains("gog") && item.key.ends_with("_is1")
            })
            .map(|item| item.folder.clone())
            .filter(|path| !path.as_os_str().is_empty() && super::on_this_pc(path)),
    );
    let available = galaxy.is_some() || !folders.is_empty();
    for folder in folders {
        for path in files(&folder, errors)
            .into_iter()
            .filter(|path| gog_info(path))
        {
            let result =
                bytes(&path).and_then(|data| library::gog(&data, &folder, galaxy.as_deref()));
            add(
                result.map(|game| {
                    game.map(|mut game| {
                        let icon = folder.join(format!("goggame-{}.ico", game.id));
                        game.cover = icon.is_file().then_some(icon);
                        game
                    })
                }),
                games,
                errors,
            );
        }
    }
    gog_registered(&registered, games, errors);
    available
}
