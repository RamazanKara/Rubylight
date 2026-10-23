//! Local launcher discovery. Scanning never starts a launcher or changes its files.
use crate::steam::registry_string;
use anyhow::{Context, Result};
use butterpollo_core::library::{self, Game, Launch, Source};
use serde::Serialize;
use std::{
    collections::BTreeSet,
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

pub fn discover() -> Discovery {
    let mut registry_errors = vec![];
    let installs = installed(&mut registry_errors);
    let program_folders = program_folders();
    let program_data =
        PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()));
    let mut sources = vec![];
    let mut games = vec![];
    for source in Source::ALL {
        let mut errors = if matches!(source, Source::Steam | Source::Epic | Source::Microsoft) {
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
                let mut folders: BTreeSet<PathBuf> =
                    folders(r"SOFTWARE\GOG.com\Games", &mut errors)
                        .into_iter()
                        .map(|(_, path)| path)
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
                    for path in files(&folder, &mut errors).into_iter().filter(|path| {
                        path.file_name().is_some_and(|name| {
                            let name = name.to_string_lossy();
                            name.starts_with("goggame-") && name.ends_with(".info")
                        })
                    }) {
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
                available
            }
            Source::Microsoft => {
                // Packages, not the registry: the scan reports its own errors.
                let scan = store::discover();
                errors.extend(scan.errors);
                games.extend(scan.games);
                scan.available
            }
        };
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
    let mut seen = BTreeSet::new();
    games.retain(|game| seen.insert((game.source, game.id.clone())));
    games.sort_by_cached_key(|game| (game.name.to_lowercase(), game.source, game.id.clone()));
    Discovery { sources, games }
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
