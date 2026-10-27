//! Local launcher discovery. Scanning never starts a launcher or changes its files.
use crate::steam::registry_string;
use anyhow::{Context, Result};
use butterpollo_core::library::{self, Game, Launch, Source};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, RegCloseKey,
            RegEnumKeyExW, RegOpenKeyExW,
        },
    },
    core::{PCWSTR, PWSTR},
};

mod store;

#[derive(Serialize)]
pub struct SourceStatus {
    pub source: Source,
    pub name: &'static str,
    pub state: &'static str,
    pub errors: Vec<String>,
}
pub struct Discovery {
    pub sources: Vec<SourceStatus>,
    pub games: Vec<Game>,
}
struct Installed {
    key: String,
    name: String,
    publisher: String,
    folder: PathBuf,
    uninstall: String,
}

fn subkeys(root: HKEY, path: &str) -> Result<Vec<String>> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut key = HKEY::default();
    // SAFETY: the path is terminated; key is an out-pointer. Every successfully
    // opened key is closed after enumeration, including an enumeration error.
    let opened = unsafe { RegOpenKeyExW(root, PCWSTR(wide.as_ptr()), Some(0), KEY_READ, &mut key) };
    if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
        return Ok(vec![]);
    }
    opened
        .ok()
        .with_context(|| format!("Reading registry {path}"))?;
    let result = (|| {
        let mut entries = vec![];
        for index in 0.. {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            // SAFETY: the live key and output buffer remain valid for this call.
            let result = unsafe {
                RegEnumKeyExW(
                    key,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if result == ERROR_NO_MORE_ITEMS {
                return Ok(entries);
            }
            result
                .ok()
                .with_context(|| format!("Listing registry {path}"))?;
            entries.push(String::from_utf16_lossy(&name[..length as usize]));
        }
        unreachable!()
    })();
    // SAFETY: key was opened above and has not been closed.
    let _ = unsafe { RegCloseKey(key) };
    result
}
fn locations(key: &str) -> Vec<(HKEY, String)> {
    let mut roots = vec![
        (HKEY_LOCAL_MACHINE, key.into()),
        (
            HKEY_LOCAL_MACHINE,
            key.replacen("SOFTWARE\\", "SOFTWARE\\WOW6432Node\\", 1),
        ),
    ];
    if let Some(sid) = crate::process::user_sid() {
        roots.push((HKEY_USERS, format!("{sid}\\{key}")));
        roots.push((
            HKEY_USERS,
            format!(
                "{sid}\\{}",
                key.replacen("SOFTWARE\\", "SOFTWARE\\WOW6432Node\\", 1)
            ),
        ));
    } else {
        roots.push((HKEY_CURRENT_USER, key.into()));
    }
    roots
}
fn records(key: &str, errors: &mut Vec<String>) -> Vec<(HKEY, String, String)> {
    let mut entries = vec![];
    for (root, path) in locations(key) {
        match subkeys(root, &path) {
            Ok(keys) => entries.extend(
                keys.into_iter()
                    .map(|id| (root, format!("{path}\\{id}"), id)),
            ),
            Err(error) => errors.push(error.to_string()),
        }
    }
    entries
}
fn installed(errors: &mut Vec<String>) -> Vec<Installed> {
    records(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        errors,
    )
    .into_iter()
    .map(|(root, path, key)| {
        let get = |name| registry_string(root, &path, name).unwrap_or_default();
        Installed {
            key,
            name: get("DisplayName"),
            publisher: get("Publisher"),
            folder: PathBuf::from(get("InstallLocation")),
            uninstall: get("UninstallString"),
        }
    })
    .collect()
}
fn folders(key: &str, errors: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    records(key, errors)
        .into_iter()
        .filter_map(|(root, path, id)| {
            [
                "InstallDir",
                "Install Dir",
                "InstallLocation",
                "InstallDirectory",
                "path",
            ]
            .into_iter()
            .find_map(|name| registry_string(root, &path, name))
            .map(|folder| (id, PathBuf::from(folder)))
        })
        .collect()
}
fn protocol(scheme: &str) -> bool {
    locations(&format!(r"SOFTWARE\Classes\{scheme}\shell\open\command"))
        .into_iter()
        .any(|(root, path)| registry_string(root, &path, "").is_some())
}
fn bytes(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("Reading {}", path.display()))?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "Launcher record is too large: {}",
        path.display()
    );
    Ok(bytes)
}
fn files(folder: &Path, errors: &mut Vec<String>) -> Vec<PathBuf> {
    match std::fs::read_dir(folder) {
        Ok(entries) => entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry.path()),
                Err(error) => {
                    errors.push(format!("Reading {}: {error}", folder.display()));
                    None
                }
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => {
            errors.push(format!("Reading {}: {error}", folder.display()));
            vec![]
        }
    }
}
fn add(result: Result<Option<Game>>, games: &mut Vec<Game>, errors: &mut Vec<String>) {
    match result {
        Ok(Some(mut game)) => {
            if !Path::new(&game.install_dir).is_dir() {
                return;
            }
            if game.cover.is_none() {
                game.cover = ["cover.png", "cover.jpg", "folder.jpg"]
                    .iter()
                    .map(|file| Path::new(&game.install_dir).join(file))
                    .find(|file| file.is_file());
            }
            games.push(game);
        }
        Ok(None) => {}
        Err(error) => errors.push(error.to_string()),
    }
}
fn program_folders() -> Vec<PathBuf> {
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
fn launcher(installs: &[Installed], name: &str, relative: &[&str]) -> Option<PathBuf> {
    let paths = installs
        .iter()
        .filter(|item| item.name.to_lowercase().contains(name))
        .filter(|item| !item.folder.as_os_str().is_empty())
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
fn gog_registered(
    registered: &[BTreeMap<String, String>],
    games: &mut Vec<Game>,
    errors: &mut Vec<String>,
) {
    for values in registered {
        let id = values.get("gameID").map(|id| id.trim());
        // The .info scan already reported a folder it could not read.
        let has_info = values.get("path").is_some_and(|folder| {
            files(Path::new(folder.trim()), &mut vec![])
                .iter()
                .any(|path| gog_info(path))
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

/// How many files and folders the scan looks at in one game folder.
const GAME_FOLDER_ENTRIES: usize = 2000;
/// The programs in a game folder and up to two folders below it, the top first, leaving
/// out the folders that never hold the game's own program, and not following links.
/// Whether the scan stopped at [`GAME_FOLDER_ENTRIES`] before it read them all.
fn programs(folder: &Path, errors: &mut Vec<String>) -> (Vec<library::Program>, bool) {
    let mut found = vec![];
    let mut pending = std::collections::VecDeque::from([(PathBuf::new(), 0)]);
    let mut seen = 0;
    while let Some((relative, depth)) = pending.pop_front() {
        for path in files(&folder.join(&relative), errors) {
            seen += 1;
            if seen > GAME_FOLDER_ENTRIES {
                return (found, true);
            }
            let (Some(name), Ok(metadata)) = (path.file_name(), std::fs::symlink_metadata(&path))
            else {
                continue;
            };
            let lower = name.to_string_lossy().to_lowercase();
            let relative = relative.join(name);
            if metadata.is_dir() {
                if depth < 2 && !library::LOCAL_SKIPPED_FOLDERS.contains(&lower.as_str()) {
                    pending.push_back((relative, depth + 1));
                }
            } else if metadata.is_file() && lower.ends_with(".exe") {
                found.push(library::Program {
                    path: relative,
                    size: metadata.len(),
                });
            }
        }
    }
    (found, false)
}

/// Why a configured game folder cannot be read, if it cannot. The host runs as a
/// service, which does not see the drive letters a user mapped.
fn unreadable_root(root: &Path) -> Option<String> {
    let shown = root.display();
    if !root.is_absolute() {
        return Some(format!("The game folder {shown} is not a full path."));
    }
    if root.is_dir() {
        return None;
    }
    if root.exists() {
        return Some(format!("The game folder {shown} is a file, not a folder."));
    }
    let drive = root.ancestors().last().unwrap_or(root);
    Some(if drive.exists() {
        format!("The game folder {shown} does not exist.")
    } else {
        format!(
            "The game folder {shown} is on a drive the host cannot see. A mapped network drive belongs to one user: enter its network path (\\\\server\\share) instead."
        )
    })
}
/// Folders a drive or a folder of games holds that are not games: hidden and system
/// folders, such as `$RECYCLE.BIN` and `System Volume Information`.
fn hidden_folder(folder: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
    folder
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('$'))
        || std::fs::metadata(folder)
            .is_ok_and(|metadata| metadata.file_attributes() & HIDDEN_OR_SYSTEM != 0)
}

/// One game per subfolder of the configured game folders. A folder that is missing is an
/// error of its own; the other folders are still read.
fn local(settings: &library::Settings, games: &mut Vec<Game>, errors: &mut Vec<String>) {
    for root in &settings.local_folders {
        if let Some(error) = unreadable_root(root) {
            errors.push(error);
            continue;
        }
        for folder in files(root, errors) {
            if !folder.is_dir() || hidden_folder(&folder) || settings.excluded(&folder) {
                continue;
            }
            let (found, cut_short) = programs(&folder, errors);
            let game = library::local(&folder, &found);
            if cut_short {
                if game.is_none() {
                    errors.push(format!(
                        "No game program is among the first {GAME_FOLDER_ENTRIES} files in {}.",
                        folder.display()
                    ));
                } else {
                    // The top folders came first, so the program found is the top one.
                    tracing::debug!(folder = %folder.display(), "the game folder scan read only its first {GAME_FOLDER_ENTRIES} files");
                }
            }
            add(Ok(game), games, errors);
        }
    }
}

impl Discovery {
    /// The sources the settings turned off, which the scan did not read.
    pub fn skipped(&self) -> impl Iterator<Item = Source> + '_ {
        self.sources
            .iter()
            .filter(|status| status.state == "disabled")
            .map(|status| status.source)
    }
}

pub fn discover(settings: &library::Settings) -> Discovery {
    let mut registry_errors = vec![];
    let installs = installed(&mut registry_errors);
    let program_folders = program_folders();
    let program_data =
        PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()));
    let mut sources = vec![];
    let mut games = vec![];
    for source in Source::ALL {
        if !settings.enabled(source) {
            sources.push(SourceStatus {
                source,
                name: source.name(),
                state: "disabled",
                errors: vec![],
            });
            continue;
        }
        let mut errors = if matches!(
            source,
            Source::Steam | Source::Epic | Source::Microsoft | Source::Local
        ) {
            vec![]
        } else {
            registry_errors.clone()
        };
        let available = match source {
            Source::Steam => {
                let mut roots = crate::steam::roots();
                for (root, key) in locations(r"SOFTWARE\Valve\Steam") {
                    for name in ["SteamPath", "InstallPath"] {
                        if let Some(folder) = registry_string(root, &key, name) {
                            let path = PathBuf::from(folder);
                            if (path.join("steamapps").is_dir() || path.join("steam.exe").is_file())
                                && !roots.contains(&path)
                            {
                                roots.push(path);
                            }
                        }
                    }
                }
                for game in butterpollo_core::steam::discover(&roots) {
                    if !butterpollo_core::steam::importable(&game, false) {
                        continue;
                    }
                    let mut entry = Game::new(
                        source,
                        game.appid.to_string(),
                        game.name.clone(),
                        Path::new(&game.install_dir),
                        Some(Launch::Uri {
                            uri: format!("steam://rungameid/{}", game.appid),
                        }),
                    );
                    entry.cover = game.artwork().map(Path::to_path_buf);
                    add(Ok(Some(entry)), &mut games, &mut errors);
                }
                !roots.is_empty()
            }
            Source::Epic => {
                let folder = program_data.join(r"Epic\EpicGamesLauncher\Data\Manifests");
                let can_launch = protocol("com.epicgames.launcher");
                for path in files(&folder, &mut errors)
                    .into_iter()
                    .filter(|path| path.extension().is_some_and(|ext| ext == "item"))
                {
                    let result = bytes(&path)
                        .and_then(|data| library::epic(&data))
                        .with_context(|| path.display().to_string());
                    add(result.map(|game| game.map(|mut game| {
                        if !can_launch { game.unavailable_reason = Some("Install or repair Epic Games Launcher to launch this game.".into()); }
                        game
                    })), &mut games, &mut errors);
                }
                can_launch
            }
            Source::Battlenet => {
                let executable = launcher(
                    &installs,
                    "battle.net",
                    &["Battle.net Launcher.exe", "Battle.net.exe"],
                )
                .or_else(|| {
                    program_folders
                        .iter()
                        .flat_map(|root| {
                            ["Battle.net Launcher.exe", "Battle.net.exe"]
                                .map(|file| root.join("Battle.net").join(file))
                        })
                        .find(|path| path.is_file())
                });
                let database = program_data.join(r"Battle.net\Agent\product.db");
                let mut entries = vec![];
                if database.exists() {
                    match bytes(&database).and_then(|data| library::battlenet_database(&data)) {
                        Ok(found) => entries.extend(found),
                        Err(error) => errors.push(format!("Battle.net product database: {error}")),
                    }
                }
                for item in &installs {
                    if !item.uninstall.to_lowercase().contains("battle.net") {
                        continue;
                    }
                    if let Some((_, rest)) = item.uninstall.split_once("--uid=") {
                        let id = rest
                            .trim_start_matches('"')
                            .split(['"', ' '])
                            .next()
                            .unwrap_or("");
                        if !id.is_empty() {
                            entries.push((id.into(), item.folder.clone()));
                        }
                    }
                }
                for (uid, folder) in entries {
                    add(
                        Ok(library::battlenet(&uid, &folder, executable.as_deref())),
                        &mut games,
                        &mut errors,
                    );
                }
                executable.is_some()
            }
            Source::Ea => {
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
                    candidates.extend(
                        folders(key, &mut errors)
                            .into_iter()
                            .map(|(_, folder)| folder),
                    );
                }
                for root in &program_folders {
                    for folder in ["EA Games", "Origin Games"] {
                        candidates.extend(
                            files(&root.join(folder), &mut errors)
                                .into_iter()
                                .filter(|path| path.is_dir()),
                        );
                    }
                }
                let can_launch = protocol("origin2");
                for folder in candidates.into_iter().collect::<BTreeSet<_>>() {
                    let manifest = folder.join(r"__Installer\installerdata.xml");
                    if !manifest.exists() {
                        continue;
                    }
                    let name = installs
                        .iter()
                        .find(|item| item.folder == folder && !item.name.is_empty())
                        .map(|item| item.name.clone())
                        .unwrap_or_else(|| {
                            folder
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        });
                    let result = bytes(&manifest)
                        .and_then(|data| library::ea(std::str::from_utf8(&data)?, &folder, &name))
                        .map(|mut game| {
                            if !can_launch {
                                game.unavailable_reason = Some(
                                    "Install or repair the EA app to launch this game.".into(),
                                );
                            }
                            Some(game)
                        });
                    add(result, &mut games, &mut errors);
                }
                can_launch
            }
            Source::Ubisoft => {
                let can_launch = protocol("uplay");
                let entries = folders(r"SOFTWARE\Ubisoft\Launcher\Installs", &mut errors);
                let available = can_launch;
                for (id, folder) in entries {
                    let name = installs
                        .iter()
                        .find(|item| item.folder == folder && !item.name.is_empty())
                        .map(|item| item.name.clone())
                        .unwrap_or_else(|| {
                            folder
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        });
                    add(
                        library::ubisoft(&id, &folder, &name).map(|mut game| {
                            if !can_launch {
                                game.unavailable_reason = Some(
                                    "Install or repair Ubisoft Connect to launch this game.".into(),
                                );
                            }
                            Some(game)
                        }),
                        &mut games,
                        &mut errors,
                    );
                }
                available
            }
            Source::Gog => {
                let galaxy = locations(r"SOFTWARE\GOG.com\GalaxyClient\paths")
                    .into_iter()
                    .filter_map(|(root, key)| registry_string(root, &key, "client"))
                    .map(PathBuf::from)
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
                            .map(|root| root.join(r"GOG Galaxy\GalaxyClient.exe"))
                            .find(|path| path.is_file())
                    });
                // Each game's registry values, for installs without a goggame-<id>.info.
                let registered: Vec<BTreeMap<String, String>> =
                    records(r"SOFTWARE\GOG.com\Games", &mut errors)
                        .into_iter()
                        .map(|(root, path, id)| {
                            let mut values: BTreeMap<String, String> = GOG_VALUES
                                .into_iter()
                                .filter_map(|name| {
                                    registry_string(root, &path, name)
                                        .map(|value| (name.to_owned(), value))
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
                    .collect();
                folders.extend(
                    installs
                        .iter()
                        .filter(|item| {
                            item.publisher.to_lowercase().contains("gog")
                                && item.key.ends_with("_is1")
                        })
                        .map(|item| item.folder.clone())
                        .filter(|path| !path.as_os_str().is_empty()),
                );
                let available = galaxy.is_some() || !folders.is_empty();
                for folder in folders {
                    for path in files(&folder, &mut errors)
                        .into_iter()
                        .filter(|path| gog_info(path))
                    {
                        let result = bytes(&path)
                            .and_then(|data| library::gog(&data, &folder, galaxy.as_deref()));
                        add(
                            result.map(|game| {
                                game.map(|mut game| {
                                    let icon = folder.join(format!("goggame-{}.ico", game.id));
                                    game.cover = icon.is_file().then_some(icon);
                                    game
                                })
                            }),
                            &mut games,
                            &mut errors,
                        );
                    }
                }
                gog_registered(&registered, &mut games, &mut errors);
                available
            }
            Source::Local => {
                local(settings, &mut games, &mut errors);
                true
            }
            Source::Microsoft => {
                // Packages, not the registry: the scan reports its own errors.
                let scan = store::discover();
                errors.extend(scan.errors);
                games.extend(scan.games);
                scan.available
            }
        };
        if !errors.is_empty() {
            // One launcher's trouble never hides the others: its games found so far stay.
            tracing::warn!(
                source = source.key(),
                ?errors,
                "the launcher scan is incomplete"
            );
        }
        sources.push(SourceStatus {
            source,
            name: source.name(),
            state: if !errors.is_empty() {
                "error"
            } else if available {
                "available"
            } else {
                "unavailable"
            },
            errors,
        });
    }
    Discovery {
        sources,
        games: library::finish(games),
    }
}

pub fn launch(
    action: &Launch,
    directory: Option<&Path>,
    elevated: bool,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    use crate::process::{Process, Target, quote};
    let (program, arguments) = match action {
        Launch::Uri { uri } => {
            // A Store or Xbox game starts through the shell by its application id. That is
            // no protocol handler, and the id is checked again since `apps.json` is editable.
            if uri.starts_with("shell:") {
                anyhow::ensure!(
                    library::store_launch_target(uri).is_some(),
                    "{uri:?} is not the launch address of a Microsoft Store or Xbox game."
                );
            } else {
                let scheme = uri.split_once(':').context("Invalid launcher URI")?.0;
                anyhow::ensure!(
                    protocol(scheme),
                    "The {scheme} launcher is unavailable. Install or repair the launcher."
                );
            }
            let program =
                PathBuf::from(std::env::var_os("WINDIR").context("Windows directory unavailable")?)
                    .join("explorer.exe");
            (program, quote(std::ffi::OsStr::new(uri))?)
        }
        Launch::Program { program, arguments } => (PathBuf::from(program), arguments.clone()),
    };
    let line = format!("{} {arguments}", quote(program.as_os_str())?);
    // The launcher may already serve other games. Own only the new game
    // processes discovered by the tracker, never the shared launcher job.
    Process::spawn_command(
        &program,
        &line,
        directory,
        Target::User { elevated },
        environment,
        true,
        true,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_store_application_id_launches_through_the_shell() {
        let try_launch = |uri: &str| {
            launch(
                &Launch::Uri { uri: uri.into() },
                None,
                false,
                &Default::default(),
            )
        };
        for uri in [
            r"shell:Startup",
            r"shell:AppsFolder\..\..\Windows\System32\cmd.exe",
            r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game & calc",
        ] {
            let error = try_launch(uri).unwrap_err().to_string();
            assert!(error.contains("Microsoft Store or Xbox"), "{uri}: {error}");
        }
    }

    #[test]
    fn file_discovery_reports_bad_records_and_ignores_missing_locations() {
        let temp = tempfile::tempdir().unwrap();
        let mut errors = vec![];
        assert!(files(&temp.path().join("missing"), &mut errors).is_empty());
        assert!(errors.is_empty());
        let bad = temp.path().join("bad.item");
        std::fs::write(&bad, b"{broken").unwrap();
        let mut games = vec![];
        for file in files(temp.path(), &mut errors) {
            add(
                bytes(&file).and_then(|data| library::epic(&data)),
                &mut games,
                &mut errors,
            );
        }
        assert!(games.is_empty());
        assert_eq!(errors.len(), 1);
        let oversized = temp.path().join("large.item");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(16 * 1024 * 1024 + 1)
            .unwrap();
        assert!(
            bytes(&oversized)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
    }

    #[test]
    fn game_folders_are_read_two_folders_deep_and_a_missing_one_hides_no_other() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Spiele");
        let write = |relative: &str, size: u64| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::File::create(path).unwrap().set_len(size).unwrap();
        };
        write("Hades/Hades.exe", 2048);
        write("Hades/unins000.exe", 4096);
        write("Hades/cover.jpg", 10);
        write("Witcher/bin/x64/witcher3.exe", 4096);
        write("Witcher/_CommonRedist/game.exe", 8192);
        write("Deep/a/b/c/deep.exe", 4096);
        write("Leave Out/game.exe", 4096);
        write("loose.exe", 4096);
        write("$RECYCLE.BIN/S-1-5-21/$RABC123.exe", 4096);
        let (mut programs_found, cut_short) = programs(&root.join("Witcher"), &mut vec![]);
        assert!(!cut_short);
        programs_found.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            programs_found
                .iter()
                .map(|program| (program.path.clone(), program.size))
                .collect::<Vec<_>>(),
            [(PathBuf::from(r"bin\x64\witcher3.exe"), 4096)],
            "redistributable folders are not read"
        );
        let settings = library::Settings {
            local_folders: vec![temp.path().join("missing"), root.clone()],
            local_exclude: vec!["leave out".into()],
            ..Default::default()
        };
        let (mut games, mut errors) = (vec![], vec![]);
        local(&settings, &mut games, &mut errors);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("missing") && errors[0].contains("does not exist"));
        let found: Vec<_> = library::finish(games)
            .into_iter()
            .map(|game| (game.name, game.launch.unwrap().command(), game.cover))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "Hades".to_owned(),
                    format!("\"{}\"", root.join(r"Hades\Hades.exe").display()),
                    Some(root.join("Hades").join("cover.jpg"))
                ),
                (
                    "Witcher".to_owned(),
                    format!(
                        "\"{}\"",
                        root.join(r"Witcher\bin\x64\witcher3.exe").display()
                    ),
                    None
                ),
            ],
            "the recycle bin is not a game"
        );
    }

    #[test]
    fn a_game_folder_too_big_to_read_whole_is_read_from_the_top_and_says_so() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for game in ["Big", "Huge"] {
            let data = root.join(game).join("zdata");
            std::fs::create_dir_all(&data).unwrap();
            for file in 0..=GAME_FOLDER_ENTRIES {
                std::fs::File::create(data.join(format!("{file}.pak"))).unwrap();
            }
        }
        std::fs::create_dir_all(root.join(r"Big\a")).unwrap();
        std::fs::File::create(root.join(r"Big\a\Big.exe")).unwrap();
        let settings = library::Settings {
            local_folders: vec![root.to_owned()],
            ..Default::default()
        };
        let (mut games, mut errors) = (vec![], vec![]);
        local(&settings, &mut games, &mut errors);
        assert_eq!(
            games
                .iter()
                .map(|game| game.name.as_str())
                .collect::<Vec<_>>(),
            ["Big"],
            "a program near the top is found before the limit"
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("Huge"), "{errors:?}");
        // A drive the host cannot see is told apart from a missing folder.
        let free = (b'D'..=b'Z')
            .rev()
            .map(|letter| PathBuf::from(format!(r"{}:\Games", letter as char)))
            .find(|path| !path.ancestors().last().unwrap().exists())
            .unwrap();
        let unseen = unreadable_root(&free).unwrap();
        assert!(unseen.contains("network path"), "{unseen}");
        assert!(
            unreadable_root(Path::new("Games"))
                .unwrap()
                .contains("full path")
        );
    }

    #[test]
    fn a_gog_registry_record_is_a_game_only_when_its_folder_has_no_info_file() {
        let temp = tempfile::tempdir().unwrap();
        let record = |id: &str, folder: &str| {
            let path = temp.path().join(folder);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::File::create(path.join("game.exe")).unwrap();
            BTreeMap::from(
                [
                    ("gameID", id.to_owned()),
                    ("gameName", format!("Game {id}")),
                    ("path", path.display().to_string()),
                    ("exe", path.join("game.exe").display().to_string()),
                ]
                .map(|(key, value)| (key.to_owned(), value)),
            )
        };
        let registered = [
            record("1", "Edition"),
            record("2", "Offline"),
            record("3", "Listed"),
        ];
        // The edition's folder has the base game's info file, under another id.
        std::fs::File::create(temp.path().join(r"Edition\goggame-9.info")).unwrap();
        let mut games = vec![Game::new(
            Source::Gog,
            "3".into(),
            "Listed".into(),
            &temp.path().join("Listed"),
            None,
        )];
        let mut errors = vec![];
        gog_registered(&registered, &mut games, &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            games
                .iter()
                .map(|game| (game.id.as_str(), game.name.as_str()))
                .collect::<Vec<_>>(),
            [("3", "Listed"), ("2", "Game 2")]
        );
    }

    #[test]
    fn installed_records_keep_unicode_paths_and_local_artwork() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Étoile Игры");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("cover.png"), b"local artwork").unwrap();
        let mut record: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../core/src/library/fixtures/epic.item"))
                .unwrap();
        record["InstallLocation"] = serde_json::json!(folder);
        let mut games = vec![];
        let mut errors = vec![];
        add(
            library::epic(&serde_json::to_vec(&record).unwrap()),
            &mut games,
            &mut errors,
        );
        assert!(errors.is_empty());
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].install_dir, folder.to_string_lossy());
        assert_eq!(games[0].cover, Some(folder.join("cover.png")));
        record["InstallLocation"] = serde_json::json!(temp.path().join("uninstalled"));
        add(
            library::epic(&serde_json::to_vec(&record).unwrap()),
            &mut games,
            &mut errors,
        );
        assert_eq!(games.len(), 1);
    }
}
