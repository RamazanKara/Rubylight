//! Steam library discovery and the apps it maintains, as in Vibepollo 2.0.
//!
//! Only local files are read: `libraryfolders.vdf`, `appmanifest_*.acf`,
//! `userdata/*/config/localconfig.vdf`, `appcache/appinfo.vdf` and the
//! library cache artwork. Apps the sync adds carry `steam-managed: "auto"`,
//! a UUID derived from the Steam app ID and `synced-by: steam`; only the
//! stamp makes an app the sync's own, since Vibepollo writes the rest too.
use crate::{config::Config, library};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

mod tracker;
mod vdf;
pub use tracker::{Process, Tracked, Tracker};
use vdf::appinfo_entries;
pub use vdf::{Vdf, appinfo};

/// An installed (or recently played) Steam app.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Game {
    pub appid: u32,
    pub name: String,
    pub app_type: String,
    pub install_dir: PathBuf,
    pub library_path: PathBuf,
    pub installed: bool,
    /// Unix seconds; 0 when never played.
    pub last_played: u64,
    pub playtime_minutes: u64,
    pub portrait_path: Option<PathBuf>,
    pub header_path: Option<PathBuf>,
    pub icon_path: Option<PathBuf>,
}
impl Game {
    /// The best local cover: portrait, else header, else icon.
    pub fn artwork(&self) -> Option<&Path> {
        self.portrait_path
            .as_deref()
            .or(self.header_path.as_deref())
            .or(self.icon_path.as_deref())
    }
}

/// The `steamapps` folder of a Steam or library root.
pub fn steamapps(root: &Path) -> Option<PathBuf> {
    let named = root
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("steamapps"));
    if (named && root.is_dir()) || root.join("libraryfolders.vdf").is_file() {
        return Some(root.to_owned());
    }
    let nested = root.join("steamapps");
    nested.is_dir().then_some(nested)
}
/// Every installed app in the libraries of `roots` (Steam installations),
/// with play history, names and types from the app cache, and artwork.
pub fn discover(roots: &[PathBuf]) -> Vec<Game> {
    let libraries = libraries(roots);
    let mut games: BTreeMap<u32, Game> = BTreeMap::new();
    for steamapps in &libraries {
        for manifest in manifests(steamapps) {
            let Some((appid, state, install_dir)) = installed(steamapps, &manifest) else {
                continue;
            };
            if games.contains_key(&appid) {
                continue;
            }
            games.insert(
                appid,
                Game {
                    appid,
                    name: state.get("name").and_then(Vdf::text).unwrap_or("").into(),
                    app_type: state.get("type").and_then(Vdf::text).unwrap_or("").into(),
                    install_dir,
                    library_path: steamapps.parent().unwrap_or(steamapps).to_owned(),
                    installed: true,
                    ..Default::default()
                },
            );
        }
    }
    let steam_roots: Vec<&PathBuf> = roots
        .iter()
        .filter(|r| r.join("userdata").is_dir())
        .collect();
    for root in &steam_roots {
        apply_history(root, &mut games);
    }
    let wanted: BTreeSet<u32> = games.keys().copied().collect();
    for root in &steam_roots {
        if let Ok(data) = std::fs::read(root.join("appcache").join("appinfo.vdf")) {
            for (appid, (name, kind)) in appinfo(&data, &wanted) {
                let game = games.get_mut(&appid).unwrap();
                if !name.is_empty() {
                    game.name = name;
                }
                if !kind.is_empty() {
                    game.app_type = kind.to_ascii_lowercase();
                }
            }
            break;
        }
    }
    for game in games.values_mut() {
        let (portrait, header, icon) = artwork(game.appid, &game.library_path, roots);
        game.portrait_path = portrait;
        game.header_path = header;
        game.icon_path = icon;
    }
    games.into_values().filter(|g| !g.name.is_empty()).collect()
}
/// Whether `path` is on one of this PC's drives. The host runs as SYSTEM:
/// opening a network share would hand it the PC's account, so a library on
/// a share is skipped before anything opens it.
fn on_this_pc(path: &Path) -> bool {
    crate::nucleus::generic::on_this_pc(&path.to_string_lossy()).is_some()
}

/// The `steamapps` folders of every library of `roots` on this PC's drives.
fn libraries(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut libraries: Vec<PathBuf> = vec![];
    let add = |path: PathBuf, libraries: &mut Vec<PathBuf>| {
        if !libraries.iter().any(|known| same_path(known, &path)) {
            libraries.push(path);
        }
    };
    for root in roots.iter().filter(|root| on_this_pc(root)) {
        let Some(apps) = steamapps(root) else {
            continue;
        };
        add(apps.clone(), &mut libraries);
        let folders = std::fs::read_to_string(apps.join("libraryfolders.vdf"))
            .map(|text| Vdf::parse(&text))
            .unwrap_or(Vdf::Object(vec![]));
        let folders = folders.get("libraryfolders").unwrap_or(&folders);
        for (_, folder) in folders.entries() {
            // Old files give the path as the value itself.
            let path = folder.get("path").or(Some(folder)).and_then(Vdf::text);
            if let Some(path) = path.filter(|p| !p.is_empty() && on_this_pc(Path::new(p)))
                && let Some(library) = steamapps(Path::new(path))
            {
                add(library, &mut libraries);
            }
        }
    }
    libraries
}
/// The `appmanifest_*.acf` files of a `steamapps` folder, in name order.
fn manifests(steamapps: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(steamapps) else {
        return vec![];
    };
    let mut manifests: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    let name = name.to_ascii_lowercase();
                    name.starts_with("appmanifest_") && name.ends_with(".acf")
                })
        })
        .collect();
    manifests.sort();
    manifests
}
/// A manifest's app, its `AppState` and its install folder, when the folder
/// is there.
fn installed(steamapps: &Path, manifest: &Path) -> Option<(u32, Vdf, PathBuf)> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let state = Vdf::parse(&text).get("AppState")?.clone();
    let appid = state
        .get("appid")
        .and_then(Vdf::number)
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id > 0)?;
    let folder = state.get("installdir").and_then(Vdf::text).unwrap_or("");
    let install_dir = steamapps.join("common").join(folder);
    (!folder.is_empty() && install_dir.is_dir()).then_some((appid, state, install_dir))
}

/// How Steam starts an installed game on Windows ([`game_executable`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub exe: PathBuf,
    pub install_dir: PathBuf,
    /// The launch option's `arguments`, which Steam passes the .exe.
    pub arguments: String,
    /// Whether the store lists Valve Anti-Cheat for it (`common/category`
    /// `category_8`).
    pub vac: bool,
}

/// The .exe Steam starts for `appid` on Windows and the game's install
/// folder: the app cache's first launch option for Windows that is no
/// beta's and starts an .exe below the folder, a 64-bit one first.
pub fn game_executable(appid: u32, roots: &[PathBuf]) -> Result<Launch, String> {
    let install_dir = libraries(roots)
        .iter()
        .find_map(|steamapps| {
            let manifest = steamapps.join(format!("appmanifest_{appid}.acf"));
            installed(steamapps, &manifest).map(|(_, _, folder)| folder)
        })
        .ok_or_else(|| format!("Steam app {appid} is not installed"))?;
    let info = cached_app(appid, roots)?;
    let (relative, arguments) = launch_executable(&info)
        .ok_or_else(|| format!("Steam app {appid} has no launch option that starts an .exe"))?;
    let parts: Vec<&str> = relative
        .split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.iter().any(|part| *part == ".." || part.contains(':')) {
        return Err(format!(
            "Steam app {appid} starts {relative}, outside its folder"
        ));
    }
    let exe = parts
        .iter()
        .fold(install_dir.clone(), |path, part| path.join(part));
    if !exe.is_file() {
        return Err(format!(
            "{} is missing: verify the game's files in Steam",
            exe.display()
        ));
    }
    Ok(Launch {
        exe,
        install_dir,
        arguments,
        vac: vac(&info),
    })
}

/// The Steam app an .exe in a Steam library belongs to ([`library_app`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryApp {
    pub appid: u32,
    pub install_dir: PathBuf,
    /// As [`Launch::vac`].
    pub vac: bool,
}

/// The Steam app whose install folder holds `exe`, an .exe started directly
/// with `arguments`: for `<library>\steamapps\common\<installdir>\...`, an
/// app of that library's `appmanifest_<id>.acf` files with the same
/// `installdir` (in any case), with its Valve Anti-Cheat flag from the app
/// cache, as a link to the app gives it ([`game_executable`]).
///
/// None only for an .exe in no `steamapps\common`, or in one that no Steam
/// installation of `roots` lists and whose manifests name no app there: a
/// plain folder. An app's files in a library no installation lists are
/// refused, as Steam's flag for them can't be trusted, and so is a folder
/// of a listed library that no manifest names, or an app the cache does
/// not list. Several apps in one folder (Half-Life 2 and its episodes)
/// narrow to [`one_of`] them. Nothing is opened for a path off this PC's
/// drives.
pub fn library_app(
    exe: &Path,
    arguments: &str,
    roots: &[PathBuf],
) -> Result<Option<LibraryApp>, String> {
    let Some((steamapps, folder, relative)) = library_folder(exe) else {
        return Ok(None);
    };
    let named: Vec<(u32, PathBuf)> = manifests(&steamapps)
        .iter()
        .filter_map(|manifest| {
            let (appid, state, install_dir) = installed(&steamapps, manifest)?;
            state
                .get("installdir")
                .and_then(Vdf::text)
                .is_some_and(|name| {
                    name.trim_end_matches(['\\', '/'])
                        .eq_ignore_ascii_case(&folder)
                })
                .then_some((appid, install_dir))
        })
        .collect();
    let listed = libraries(roots)
        .iter()
        .any(|library| same_library(library, &steamapps));
    match (named.first(), listed) {
        (None, false) => return Ok(None),
        (None, true) => {
            return Err(format!(
                "{} is in Steam's library {}, but no Steam app there installs to {folder}: verify the game's files in Steam",
                exe.display(),
                steamapps.display()
            ));
        }
        (Some((appid, _)), false) => {
            return Err(format!(
                "{} is Steam app {appid}'s, but no Steam installation on this PC lists its library {}: start Steam once while signed in, or start the game from its Steam link",
                exe.display(),
                steamapps.display()
            ));
        }
        (Some(_), true) => {}
    }
    let wanted: BTreeSet<u32> = named.iter().map(|(appid, _)| *appid).collect();
    let infos = cached_apps(&wanted, roots);
    if let Some(appid) = wanted.iter().find(|appid| !infos.contains_key(appid)) {
        return Err(not_cached(*appid));
    }
    let candidates: Vec<(u32, &Vdf)> = named
        .iter()
        .map(|(appid, _)| (*appid, &infos[appid]))
        .collect();
    let (appid, vac) = one_of(&candidates, &relative, arguments).ok_or_else(|| {
        let ids: Vec<String> = wanted.iter().map(u32::to_string).collect();
        format!(
            "Steam apps {} install to {folder}, and none of them starts {relative}: start the game from its Steam link",
            ids.join(", ")
        )
    })?;
    // Every candidate installs to the folder.
    let install_dir = named[0].1.clone();
    Ok(Some(LibraryApp {
        appid,
        install_dir,
        vac,
    }))
}

/// The app an .exe at `relative` in their shared install folder, started
/// with `arguments`, belongs to among `candidates`, and whether Valve
/// Anti-Cheat comes with it. One app is its own. Of several, those whose
/// launch option starts the .exe; then those whose option's arguments the
/// .exe's include, the most of them (Half-Life 2: Episode One is `hl2.exe
/// -game episodic`); then games before tools and demos. The lowest app id
/// stands for those left. Valve Anti-Cheat comes with it when any app that
/// starts the .exe has it: the arguments only choose the app id, and the
/// .exe may still load the anti-cheat app's game. None when several share
/// the folder and none starts the .exe.
fn one_of(candidates: &[(u32, &Vdf)], relative: &str, arguments: &str) -> Option<(u32, bool)> {
    if let [(appid, info)] = candidates {
        return Some((*appid, vac(info)));
    }
    // `-game "cstrike"` is `-game cstrike`.
    let tokens = |arguments: &str| -> Vec<String> {
        arguments
            .split_whitespace()
            .map(|token| token.trim_matches('"').to_ascii_lowercase())
            .filter(|token| !token.is_empty())
            .collect()
    };
    let given = tokens(arguments);
    // Each app that starts the .exe, with the most of its options' arguments
    // the .exe has, when all of one option's are.
    let starting: Vec<(u32, &Vdf, Option<usize>)> = candidates
        .iter()
        .filter_map(|(appid, info)| {
            let options: Vec<String> = launch_options(info)
                .into_iter()
                .filter(|(exe, _)| {
                    exe.replace('/', "\\")
                        .trim_start_matches(".\\")
                        .trim_start_matches('\\')
                        .eq_ignore_ascii_case(relative)
                })
                .map(|(_, arguments)| arguments)
                .collect();
            let matched = options
                .iter()
                .filter_map(|option| {
                    let tokens = tokens(option);
                    tokens
                        .iter()
                        .all(|token| given.contains(token))
                        .then_some(tokens.len())
                })
                .max();
            (!options.is_empty()).then_some((*appid, *info, matched))
        })
        .collect();
    let best = starting.iter().filter_map(|(_, _, matched)| *matched).max();
    let mut left: Vec<(u32, &Vdf)> = starting
        .iter()
        .filter(|(_, _, matched)| best.is_none() || *matched == best)
        .map(|(appid, info, _)| (*appid, *info))
        .collect();
    let game = |info: &Vdf| {
        info.at(&["common", "type"])
            .and_then(Vdf::text)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("game"))
    };
    if left.iter().any(|(_, info)| game(info)) {
        left.retain(|(_, info)| game(info));
    }
    let appid = left.iter().map(|(appid, _)| *appid).min()?;
    Some((appid, starting.iter().any(|(_, info, _)| vac(info))))
}

/// The executables and arguments of an app's launch options for Windows
/// that are no beta's.
fn launch_options(info: &Vdf) -> Vec<(String, String)> {
    let Some(options) = info.at(&["config", "launch"]) else {
        return vec![];
    };
    options
        .entries()
        .iter()
        .filter_map(|(_, option)| {
            let config = |key: &str| {
                option
                    .at(&["config", key])
                    .and_then(Vdf::text)
                    .unwrap_or("")
                    .to_ascii_lowercase()
            };
            let windows = config("oslist").is_empty() || config("oslist").contains("windows");
            let executable = option.get("executable").and_then(Vdf::text)?.trim();
            let arguments = option.get("arguments").and_then(Vdf::text).unwrap_or("");
            (windows && config("betakey").is_empty())
                .then(|| (executable.to_string(), arguments.trim().to_string()))
        })
        .collect()
}

/// Whether two `steamapps` folders on this PC's drives are one, also when
/// spelled differently (a junction, a short name, another case).
fn same_library(a: &Path, b: &Path) -> bool {
    same_path(a, b)
        || (on_this_pc(a)
            && on_this_pc(b)
            && matches!(
                (std::fs::canonicalize(a), std::fs::canonicalize(b)),
                (Ok(a), Ok(b)) if a == b
            ))
}

/// For an .exe below `<library>\steamapps\common\<folder>`, on this PC's
/// drives: that `steamapps` folder, `<folder>`, and the .exe's path below
/// it.
fn library_folder(exe: &Path) -> Option<(PathBuf, String, String)> {
    let local = crate::nucleus::generic::on_this_pc(&exe.to_string_lossy())?;
    let parts: Vec<&str> = local.split('\\').collect();
    let is = |at: usize, name: &str| parts.get(at).is_some_and(|p| p.eq_ignore_ascii_case(name));
    // The last `steamapps\common` with a folder and something in it below.
    let at = (1..parts.len().saturating_sub(3))
        .rev()
        .find(|&at| is(at, "steamapps") && is(at + 1, "common") && !parts[at + 2].is_empty())?;
    if parts[at + 2] == "." || parts[at + 2] == ".." || parts[at + 3..].contains(&"..") {
        return None;
    }
    Some((
        PathBuf::from(parts[..=at].join("\\")),
        parts[at + 2].to_string(),
        parts[at + 3..]
            .iter()
            .filter(|part| !part.is_empty() && **part != ".")
            .copied()
            .collect::<Vec<_>>()
            .join("\\"),
    ))
}

/// `appid`'s entry in the app cache of the first of `roots` that has one.
fn cached_app(appid: u32, roots: &[PathBuf]) -> Result<Vdf, String> {
    cached_apps(&BTreeSet::from([appid]), roots)
        .remove(&appid)
        .ok_or_else(|| not_cached(appid))
}

/// The entries of `wanted` in the app cache of the first of `roots` that
/// has one.
fn cached_apps(wanted: &BTreeSet<u32>, roots: &[PathBuf]) -> BTreeMap<u32, Vdf> {
    roots
        .iter()
        .filter(|root| on_this_pc(root))
        .find_map(|root| std::fs::read(root.join("appcache").join("appinfo.vdf")).ok())
        .map(|data| appinfo_entries(&data, wanted))
        .unwrap_or_default()
}

fn not_cached(appid: u32) -> String {
    format!("Steam's app cache does not list app {appid}: start it once in Steam")
}

/// Whether the store lists Valve Anti-Cheat for an app (`common/category`
/// `category_8`).
fn vac(info: &Vdf) -> bool {
    info.at(&["common", "category", "category_8"])
        .and_then(Vdf::text)
        .is_some_and(|flag| flag.trim() == "1")
}

/// Whether Steam lists Valve Anti-Cheat for a game: for a Steam game's link
/// (`appid`) its app cache entry's flag, for an .exe started directly its
/// Steam app's ([`library_app`]), and none for an .exe in a plain folder.
/// An error when Steam can't say.
pub fn valve_anti_cheat(
    appid: Option<u32>,
    exe: &Path,
    arguments: &str,
    roots: &[PathBuf],
) -> Result<bool, String> {
    match appid {
        Some(appid) => cached_app(appid, roots).map(|info| vac(&info)),
        None => Ok(library_app(exe, arguments, roots)?.is_some_and(|app| app.vac)),
    }
}

/// The executable of the launch option (`config/launch`) Steam starts on
/// Windows, relative to the install folder, and its arguments.
fn launch_executable(info: &Vdf) -> Option<(String, String)> {
    let options = info.at(&["config", "launch"])?.entries();
    options
        .iter()
        .filter_map(|(_, option)| {
            let executable = option.get("executable").and_then(Vdf::text)?.trim();
            let config = |key: &str| {
                option
                    .at(&["config", key])
                    .and_then(Vdf::text)
                    .unwrap_or("")
                    .to_ascii_lowercase()
            };
            let windows = config("oslist").is_empty() || config("oslist").contains("windows");
            let kind = option
                .get("type")
                .and_then(Vdf::text)
                .unwrap_or("")
                .to_ascii_lowercase();
            (windows
                && config("betakey").is_empty()
                && executable.to_ascii_lowercase().ends_with(".exe")
                && matches!(kind.as_str(), "" | "default" | "none"))
            .then(|| {
                let arguments = option.get("arguments").and_then(Vdf::text).unwrap_or("");
                (
                    config("osarch") == "32",
                    (executable.to_string(), arguments.trim().to_string()),
                )
            })
        })
        // 64-bit first, then Steam's order.
        .min_by_key(|(only_32, _)| *only_32)
        .map(|(_, launch)| launch)
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(
            b.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\'),
        )
}
/// LastPlayed and Playtime from every signed-in user's local config.
fn apply_history(root: &Path, games: &mut BTreeMap<u32, Game>) {
    let Ok(users) = std::fs::read_dir(root.join("userdata")) else {
        return;
    };
    for user in users.flatten() {
        let Ok(text) = std::fs::read_to_string(user.path().join("config").join("localconfig.vdf"))
        else {
            continue;
        };
        let document = Vdf::parse(&text);
        let Some(apps) =
            document.at(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"])
        else {
            continue;
        };
        for (id, app) in apps.entries() {
            let Some(game) = id.parse().ok().and_then(|id: u32| games.get_mut(&id)) else {
                continue;
            };
            if let Some(last) = app.get("LastPlayed").and_then(Vdf::number) {
                game.last_played = game.last_played.max(last);
            }
            if let Some(minutes) = app.get("Playtime").and_then(Vdf::number) {
                game.playtime_minutes = game.playtime_minutes.max(minutes);
            }
        }
    }
}
type Artwork = (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>);
/// Portrait, header and icon from Steam's library cache, or the user's
/// custom grid artwork.
pub fn artwork(appid: u32, library: &Path, roots: &[PathBuf]) -> Artwork {
    let mut caches = vec![];
    let mut grids = vec![];
    for root in std::iter::once(library).chain(roots.iter().map(PathBuf::as_path)) {
        caches.push(root.join("appcache").join("librarycache"));
        caches.push(root.join("librarycache"));
        if let Ok(users) = std::fs::read_dir(root.join("userdata")) {
            let mut users: Vec<_> = users.flatten().map(|u| u.path()).collect();
            users.sort();
            grids.extend(users.into_iter().map(|u| u.join("config").join("grid")));
        }
    }
    let id = appid.to_string();
    let flat = |dirs: &[PathBuf], suffixes: &[String]| {
        dirs.iter()
            .flat_map(|dir| suffixes.iter().map(|s| dir.join(format!("{id}{s}"))))
            .find(|path| path.is_file())
    };
    let nested = |names: &[String]| {
        caches
            .iter()
            .flat_map(|dir| names.iter().map(|n| dir.join(&id).join(n)))
            .find(|path| path.is_file())
    };
    // Current clients keep store assets under <appid>/<content hash>/.
    let hashed = |names: &[String]| {
        caches.iter().find_map(|dir| {
            let mut folders: Vec<_> = std::fs::read_dir(dir.join(&id))
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            folders.sort();
            folders
                .iter()
                .flat_map(|folder| names.iter().map(|n| folder.join(n)))
                .find(|path| path.is_file())
        })
    };
    let images = |stem: &str| ["jpg", "png", "webp"].map(|ext| format!("{stem}.{ext}"));
    let grid = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    let portrait = flat(&caches, &images("_library_600x900_2x"))
        .or_else(|| nested(&images("library_600x900_2x")))
        .or_else(|| flat(&caches, &images("_library_600x900")))
        .or_else(|| nested(&images("library_600x900")))
        .or_else(|| {
            // Older hashed folders hold library_capsule, newer apps library_600x900.
            let mut names = images("library_capsule").to_vec();
            names.extend(images("library_600x900"));
            hashed(&names)
        })
        .or_else(|| flat(&grids, &grid(&["p.png", "p.jpg", "_p.png", "_p.jpg"])));
    let header = flat(&caches, &images("_header"))
        .or_else(|| nested(&images("header")))
        .or_else(|| hashed(&images("library_header")))
        .or_else(|| flat(&grids, &grid(&["_hero.png", "_hero.jpg"])));
    let icon = flat(&caches, &images("_icon"))
        .or_else(|| nested(&images("icon")))
        .or_else(|| flat(&grids, &grid(&["_icon.png", "_icon.jpg", ".png"])));
    (portrait, header, icon)
}

/// Steam's store portrait at its fixed CDN path. Newer apps publish it only
/// under the content-hash folder, see [`hashed_store_portrait_url`].
pub fn store_portrait_url(appid: u32) -> String {
    format!(
        "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900_2x.jpg"
    )
}
/// The CDN's 2x portrait in the same content-hash folder as a cached
/// `<appid>/<40 hex>/library_600x900.*` or `library_capsule.*`, as Steam
/// serves it for apps without a fixed-path portrait.
pub fn hashed_store_portrait_url(appid: u32, local: &Path) -> Option<String> {
    let hash = local.parent()?.file_name()?.to_str()?;
    let stem = local.file_stem()?.to_str()?;
    let hex = hash.len() == 40 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (hex && matches!(stem, "library_600x900" | "library_capsule")).then(|| {
        format!(
            "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{appid}/{hash}/{stem}_2x.jpg"
        )
    })
}

/// The Steam settings (`steam_*` keys, Vibepollo's defaults).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub auto_sync: bool,
    pub sync_all_installed: bool,
    pub recent_games: usize,
    pub recent_max_age_days: u64,
    pub remove_uninstalled: bool,
    pub include_tools: bool,
    pub exclusions: Vec<(String, String)>,
}
impl Settings {
    pub fn from_config(config: &Config) -> Self {
        let count = |key: &str, default: i64| config.integer(key, default).max(0) as u64;
        Self {
            enabled: config.boolean("steam_enabled", true),
            auto_sync: config.boolean("steam_auto_sync", true),
            sync_all_installed: config.boolean("steam_sync_all_installed", true),
            recent_games: count("steam_recent_games", 10) as usize,
            recent_max_age_days: count("steam_recent_max_age_days", 30),
            remove_uninstalled: config.boolean("steam_autosync_remove_uninstalled", true),
            include_tools: config.boolean("steam_include_tools", false),
            exclusions: id_names(config.get("steam_exclude_games", "")),
        }
    }
    /// Whether a sync removes managed apps that are no longer selected.
    pub fn removes_missing(&self) -> bool {
        !self.sync_all_installed || self.remove_uninstalled
    }
    pub fn source(&self) -> &'static str {
        if self.sync_all_installed {
            "installed"
        } else {
            "recent"
        }
    }
}
/// `[{"id":"570","name":"Dota 2"}, "730"]` or `570, 730`: plain entries are IDs.
pub fn id_names(value: &str) -> Vec<(String, String)> {
    let value = value.trim();
    if value.is_empty() {
        return vec![];
    }
    match serde_json::from_str::<Value>(value) {
        Ok(Value::Array(entries)) => entries
            .iter()
            .filter_map(|entry| match entry {
                Value::String(id) => Some((id.trim().to_owned(), String::new())),
                Value::Number(id) => Some((id.to_string(), String::new())),
                Value::Object(fields) => {
                    let text = |key: &str| match fields.get(key) {
                        Some(Value::String(s)) => s.trim().to_owned(),
                        Some(Value::Number(n)) => n.to_string(),
                        _ => String::new(),
                    };
                    Some((text("id"), text("name")))
                }
                _ => None,
            })
            .filter(|(id, name)| !id.is_empty() || !name.is_empty())
            .collect(),
        _ => value
            .split(',')
            .map(|id| (id.trim().to_owned(), String::new()))
            .filter(|(id, _)| !id.is_empty())
            .collect(),
    }
}
/// Not a game: tools, runtimes, redistributables and DLC.
pub fn importable(game: &Game, include_tools: bool) -> bool {
    if include_tools {
        return true;
    }
    if matches!(
        game.app_type.to_ascii_lowercase().as_str(),
        "tool" | "config" | "dlc" | "driver" | "music" | "video"
    ) || [228980, 1070560, 1391110, 1628350, 1824220, 1493710, 4183110].contains(&game.appid)
    {
        return false;
    }
    let name = game.name.to_ascii_lowercase();
    !name.starts_with("proton ")
        && !name.starts_with("steam linux runtime")
        && name != "steamworks common redistributables"
}
fn excluded(id: &str, name: &str, exclusions: &[(String, String)]) -> bool {
    exclusions.iter().any(|(excluded_id, excluded_name)| {
        (!excluded_id.is_empty() && excluded_id == id)
            || (!excluded_name.is_empty() && excluded_name.eq_ignore_ascii_case(name))
    })
}
pub fn is_excluded(game: &Game, exclusions: &[(String, String)]) -> bool {
    excluded(&game.appid.to_string(), &game.name, exclusions)
}
/// The games a sync keeps: every installed one, or the most recently played.
pub fn select<'a>(games: &'a [Game], settings: &Settings, now: u64) -> Vec<&'a Game> {
    let mut selected: Vec<&Game> = games
        .iter()
        .filter(|g| importable(g, settings.include_tools) && !is_excluded(g, &settings.exclusions))
        .filter(|g| g.installed && (settings.sync_all_installed || g.last_played > 0))
        .collect();
    if settings.sync_all_installed {
        return selected;
    }
    if settings.recent_games == 0 {
        return vec![];
    }
    if settings.recent_max_age_days > 0 {
        let age = settings.recent_max_age_days * 86400;
        selected.retain(|g| g.last_played <= now && now - g.last_played <= age);
    }
    selected.sort_by(|a, b| {
        b.last_played
            .cmp(&a.last_played)
            .then(a.appid.cmp(&b.appid))
    });
    selected.truncate(settings.recent_games);
    selected
}
/// Vibepollo's UUID for a Steam app: "STEAM" in the first fields and the
/// app ID in the last 48 bits.
pub fn canonical_uuid(appid: u32) -> String {
    format!("53544541-4d00-5000-8000-{appid:012x}")
}
/// The Steam app ID an app entry names, from `steam-id` or its UUID.
pub fn app_id_of(app: &Value) -> Option<u32> {
    match app.get("steam-id") {
        Some(Value::Number(id)) => {
            if let Some(id) = id.as_u64().and_then(|id| u32::try_from(id).ok()) {
                return Some(id);
            }
        }
        Some(Value::String(raw)) => {
            if let Ok(id) = raw.parse() {
                return Some(id);
            }
            if raw.to_ascii_lowercase().starts_with("steam-") {
                return raw[6..].parse().ok();
            }
        }
        _ => {}
    }
    let uuid = app.get("uuid")?.as_str()?.to_ascii_lowercase();
    if let Some(id) = uuid.strip_prefix("steam-") {
        return id.parse().ok();
    }
    uuid.strip_prefix("53544541-4d00-5000-8000-")
        .filter(|hex| hex.len() == 12)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
}
/// The Steam game an app starts, by any of its marks: [`app_id_of`], an "Add games" import's
/// launcher id, or a `steam://rungameid/` or `steam://run/` command.
pub fn game_of(app: &Value) -> Option<u32> {
    app_id_of(app)
        .or_else(|| {
            (app.get("launcher-source").and_then(Value::as_str) == Some("steam"))
                .then(|| app.get("launcher-id")?.as_str()?.parse().ok())
                .flatten()
        })
        .or_else(|| command_app_id(app.get("cmd")?.as_str()?))
}
/// The app ID in a `steam://rungameid/<id>` or `steam://run/<id>` command.
fn command_app_id(command: &str) -> Option<u32> {
    let lower = command.to_ascii_lowercase();
    let rest = ["steam://rungameid/", "steam://run/"]
        .iter()
        .find_map(|prefix| lower.find(prefix).map(|at| &lower[at + prefix.len()..]))?;
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..digits].parse().ok()
}
fn id_of(app: &Value) -> String {
    app_id_of(app).map_or_else(
        || {
            app.get("steam-id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        },
        |id| id.to_string(),
    )
}
/// How a Steam game starts: through the Steam client, so Steam's overlay,
/// cloud saves and launch options apply.
pub fn launch_command(appid: u32) -> String {
    format!("cmd /c start \"\" steam://rungameid/{appid}")
}
/// The fallback cover for Steam apps without artwork.
pub const FALLBACK_IMAGE: &str = "./assets/steam.png";
fn generic(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
fn set_or_remove(app: &mut serde_json::Map<String, Value>, key: &str, value: Option<String>) {
    match value.filter(|v| !v.is_empty()) {
        Some(value) => {
            app.insert(key.into(), Value::String(value));
        }
        None => {
            app.remove(key);
        }
    }
}
/// Write a managed app's fields for `game`; `cover` is its PNG, if any.
fn update(app: &mut Value, game: &Game, source: &str, cover: Option<&Path>) {
    if !app.is_object() {
        *app = json!({});
    }
    let fields = app.as_object_mut().unwrap();
    let name = if game.name.is_empty() {
        format!("Steam {}", game.appid)
    } else {
        game.name.clone()
    };
    fields.insert("name".into(), json!(name));
    fields.insert("uuid".into(), json!(canonical_uuid(game.appid)));
    fields.insert("steam-id".into(), json!(game.appid.to_string()));
    fields.insert("steam-managed".into(), json!("auto"));
    fields.insert(library::SYNCED_BY.into(), json!("steam"));
    fields.insert("steam-source".into(), json!(source));
    set_or_remove(fields, "steam-app-type", Some(game.app_type.clone()));
    fields.insert("cmd".into(), json!(launch_command(game.appid)));
    fields.insert("auto-detach".into(), json!(true));
    fields.insert("wait-all".into(), json!(false));
    set_or_remove(
        fields,
        "steam-install-dir",
        Some(generic(&game.install_dir)),
    );
    set_or_remove(
        fields,
        "steam-library-path",
        Some(generic(&game.library_path)),
    );
    set_or_remove(
        fields,
        "steam-icon-path",
        game.icon_path.as_deref().map(generic),
    );
    set_or_remove(
        fields,
        "steam-header-path",
        game.header_path.as_deref().map(generic),
    );
    set_or_remove(
        fields,
        "steam-boxart-path",
        game.portrait_path.as_deref().map(generic),
    );
    let artwork = game.artwork();
    set_or_remove(fields, "steam-artwork-path", artwork.map(generic));
    set_or_remove(
        fields,
        "steam-artwork-format",
        artwork
            .and_then(|p| p.extension())
            .map(|e| e.to_string_lossy().to_ascii_lowercase()),
    );
    match cover {
        Some(cover) => {
            let cover = generic(cover);
            fields.insert("steam-artwork-client-path".into(), json!(cover));
            fields.insert("steam-artwork-client-compatible".into(), json!(true));
            fields.insert("image-path".into(), json!(cover));
        }
        None => {
            // Keep a cover the user chose; replace only the one this sync set.
            let previous = fields
                .remove("steam-artwork-client-path")
                .and_then(|v| v.as_str().map(str::to_owned));
            fields.remove("steam-artwork-client-compatible");
            let image = fields
                .get("image-path")
                .and_then(Value::as_str)
                .unwrap_or("");
            if image.is_empty() || previous.as_deref() == Some(image) {
                fields.insert("image-path".into(), json!(FALLBACK_IMAGE));
            }
        }
    }
}
/// Bring the Steam apps this sync owns ([`library::SYNCED_BY`]) in `apps` in line with
/// `selected`. Every other app belongs to the user, whatever its marker or UUID: it is never
/// renamed, changed or removed, no second app is added for its game, and only a missing
/// Steam cover is filled in. Games in `dismissed` (`steam:<appid>`, deleted from the library)
/// are not added. Returns whether anything changed.
pub fn reconcile(
    apps: &mut Vec<Value>,
    selected: &[&Game],
    settings: &Settings,
    covers: &HashMap<u32, PathBuf>,
    dismissed: &[String],
) -> bool {
    let games: Vec<&Game> = selected
        .iter()
        .copied()
        .filter(|g| {
            importable(g, settings.include_tools)
                && !is_excluded(g, &settings.exclusions)
                && !dismissed.contains(&library::dismissed_key(
                    library::Source::Steam,
                    &g.appid.to_string(),
                ))
        })
        .collect();
    let by_id: HashMap<String, &Game> = games.iter().map(|g| (g.appid.to_string(), *g)).collect();
    // The games the user's own apps already have.
    let users: BTreeSet<u32> = apps
        .iter()
        .filter(|app| !library::synced_by(app, "steam"))
        .filter_map(game_of)
        .collect();
    let source = settings.source();
    let remove_missing = settings.removes_missing();
    let mut changed = false;
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < apps.len() {
        let app = &mut apps[index];
        let steam_id = id_of(app);
        let parsed = app_id_of(app);
        if steam_id.is_empty() || !library::synced_by(app, "steam") {
            if let Some(game) = game_of(app).and_then(|id| by_id.get(&id.to_string()))
                && let Some(cover) = covers.get(&game.appid)
            {
                let image = app.get("image-path").and_then(Value::as_str).unwrap_or("");
                let previous = app
                    .get("steam-artwork-client-path")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if image.is_empty()
                    || image == FALLBACK_IMAGE
                    || (!previous.is_empty() && image == previous)
                {
                    let before = app.clone();
                    let cover = generic(cover);
                    app["image-path"] = json!(cover);
                    app["steam-artwork-client-path"] = json!(cover);
                    app["steam-artwork-client-compatible"] = json!(true);
                    changed |= *app != before;
                }
            }
            index += 1;
            continue;
        }
        let name = app
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if excluded(&steam_id, &name, &settings.exclusions)
            || seen.contains(&steam_id)
            || parsed.is_some_and(|id| users.contains(&id))
        {
            apps.remove(index);
            changed = true;
            continue;
        }
        seen.insert(steam_id.clone());
        let before = app.clone();
        if let Some(game) = by_id.get(&steam_id) {
            update(
                app,
                game,
                source,
                covers.get(&game.appid).map(PathBuf::as_path),
            );
        } else {
            if let Some(id) = parsed {
                app["uuid"] = json!(canonical_uuid(id));
                app["steam-id"] = json!(id.to_string());
                app["steam-managed"] = json!("auto");
            }
            if remove_missing {
                apps.remove(index);
                changed = true;
                continue;
            }
        }
        changed |= *app != before;
        index += 1;
    }
    for game in games {
        let key = game.appid.to_string();
        if seen.contains(&key)
            || users.contains(&game.appid)
            || apps.iter().any(|app| id_of(app) == key)
        {
            continue;
        }
        let mut app = json!({});
        update(
            &mut app,
            game,
            source,
            covers.get(&game.appid).map(PathBuf::as_path),
        );
        apps.push(app);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests;
