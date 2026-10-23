//! Microsoft Store and Xbox (GDK) games, found from the packages Windows has registered for the
//! signed-in user, and from the `XboxGames` folders of the fixed drives for a game that is on
//! disk but not registered. A game has a `MicrosoftGame.config` next to its programs and is
//! started through its application user model id (`shell:AppsFolder\<AUMID>`), the way the Start
//! menu does. Scanning installs, signs in and starts nothing.
use super::{add, bytes, files};
use anyhow::{Context, Result};
use butterpollo_core::library::{self, Game, StorePackage};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use windows::{
    ApplicationModel::Package, Management::Deployment::PackageManager, Win32::Storage::FileSystem,
    core::HSTRING,
};

pub struct Scan {
    pub games: Vec<Game>,
    pub errors: Vec<String>,
    /// Whether there is anything of this kind on the PC: a game, or an `XboxGames` folder.
    pub available: bool,
}

/// A registered package that can hold a game.
struct Installed {
    family: String,
    display_name: String,
    root: PathBuf,
}

/// `GetDriveTypeW` for a fixed disk.
const DRIVE_FIXED: u32 = 3;
const CONFIG: &str = "MicrosoftGame.config";

/// The packages registered for the signed-in user. Frameworks, resources and bundles are never
/// games and are skipped.
fn packages() -> Result<Vec<Installed>> {
    // Windows Runtime needs the thread to be in an apartment. A thread that already is in one
    // of the other kind still activates, so a failure here is not the end of the scan.
    let _com = crate::capture::ComGuard::new();
    let manager = PackageManager::new().context("Windows could not open the package manager")?;
    // For SYSTEM the user is named; for the user themself no name is needed and no elevation.
    let sid = crate::process::user_sid().unwrap_or_default();
    let found = manager
        .FindPackagesByUserSecurityId(&HSTRING::from(sid))
        .context("Windows could not list the user's packages")?;
    let mut installed = vec![];
    for package in found {
        let skip = |what: windows::core::Result<bool>| what.unwrap_or(true);
        if skip(package.IsFramework())
            || skip(package.IsResourcePackage())
            || skip(package.IsBundle())
        {
            continue;
        }
        if let Some(entry) = installed_of(&package) {
            installed.push(entry);
        }
    }
    Ok(installed)
}
fn installed_of(package: &Package) -> Option<Installed> {
    let family = package.Id().ok()?.FamilyName().ok()?.to_string();
    let root = PathBuf::from(package.InstalledPath().ok()?.to_string());
    let display_name = package
        .DisplayName()
        .map(|name| name.to_string())
        .unwrap_or_default();
    (!family.is_empty() && !root.as_os_str().is_empty()).then_some(Installed {
        family,
        display_name,
        root,
    })
}

/// The `MicrosoftGame.config` of an install folder and the folder it is in: the folder itself,
/// or its `Content` folder.
fn config_in(root: &Path) -> Option<(PathBuf, PathBuf)> {
    [root.to_path_buf(), root.join("Content")]
        .into_iter()
        .map(|folder| (folder.join(CONFIG), folder))
        .find(|(config, _)| config.is_file())
}

fn text(path: &Path) -> Result<String> {
    String::from_utf8(bytes(path)?).with_context(|| format!("{} is not UTF-8", path.display()))
}

/// The game of a registered package, if it is one.
fn game_in(package: &Installed) -> Result<Option<Game>> {
    let Some((config, folder)) = config_in(&package.root) else {
        return Ok(None);
    };
    let manifest = text(&package.root.join("AppxManifest.xml"))?;
    library::microsoft(
        &text(&config)?,
        &folder,
        Some(&StorePackage {
            family: &package.family,
            display_name: &package.display_name,
            manifest: &manifest,
        }),
    )
    .with_context(|| config.display().to_string())
}

/// Games in `<drive>XboxGames\<title>` whose id is not in `known`: on disk, not registered.
fn on_disk(drives: &[PathBuf], known: &BTreeSet<String>, scan: &mut Scan) {
    for drive in drives {
        let root = drive.join("XboxGames");
        if root.is_dir() {
            scan.available = true;
        }
        for title in files(&root, &mut scan.errors)
            .into_iter()
            .filter(|path| path.is_dir())
        {
            let Some((config, folder)) = config_in(&title) else {
                continue;
            };
            let result = text(&config)
                .and_then(|xml| library::microsoft(&xml, &folder, None))
                .with_context(|| config.display().to_string())
                .map(|game| game.filter(|game| !known.contains(&game.id)));
            add(result, &mut scan.games, &mut scan.errors);
        }
    }
}

/// The local fixed drives, as roots such as `D:\`.
fn fixed_drives() -> Vec<PathBuf> {
    crate::files::drives()
        .into_iter()
        .filter(|root| {
            let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            // SAFETY: the root is NUL-terminated and outlives the call.
            let kind = unsafe { FileSystem::GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr())) };
            kind == DRIVE_FIXED
        })
        .map(PathBuf::from)
        .collect()
}

pub fn discover() -> Scan {
    discover_from(packages(), &fixed_drives())
}

fn discover_from(packages: Result<Vec<Installed>>, drives: &[PathBuf]) -> Scan {
    let mut scan = Scan {
        games: vec![],
        errors: vec![],
        available: false,
    };
    match packages {
        Ok(found) => {
            for package in found {
                let result = game_in(&package).with_context(|| package.family.clone());
                add(result, &mut scan.games, &mut scan.errors);
            }
        }
        // Loud: without the list a registered game would look as if it were not installed.
        Err(error) => scan
            .errors
            .push(format!("Microsoft Store packages: {error:#}")),
    }
    scan.available = !scan.games.is_empty();
    let known: BTreeSet<String> = scan.games.iter().map(|game| game.id.clone()).collect();
    on_disk(drives, &known, &mut scan);
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(folder: &Path, content: bool) -> PathBuf {
        let game = if content {
            folder.join("Content")
        } else {
            folder.to_path_buf()
        };
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            game.join(CONFIG),
            include_str!("../../../core/src/library/fixtures/microsoft_game.config"),
        )
        .unwrap();
        std::fs::write(
            folder.join("AppxManifest.xml"),
            r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
                 <Applications><Application Id="Game" Executable="Racing_Shipping.exe" /></Applications>
               </Package>"#,
        )
        .unwrap();
        game
    }
    fn installed(root: &Path) -> Installed {
        Installed {
            family: "Contoso.Racing_8wekyb3d8bbwe".into(),
            display_name: "ms-resource:AppName".into(),
            root: root.into(),
        }
    }

    #[test]
    fn a_registered_package_with_a_config_is_a_game_in_its_content_folder() {
        let temp = tempfile::tempdir().unwrap();
        let content = fixture(temp.path(), true);
        let game = game_in(&installed(temp.path())).unwrap().unwrap();
        assert_eq!(game.id, "Contoso.Racing");
        assert_eq!(game.install_dir, content.to_string_lossy());
        assert_eq!(
            game.launch.unwrap().command(),
            r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game"
        );
        // The config can also sit in the package root.
        let other = tempfile::tempdir().unwrap();
        fixture(other.path(), false);
        let game = game_in(&installed(other.path())).unwrap().unwrap();
        assert_eq!(game.install_dir, other.path().to_string_lossy());
    }

    #[test]
    fn a_package_without_a_config_is_not_a_game_and_a_broken_one_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        assert!(game_in(&installed(temp.path())).unwrap().is_none());
        std::fs::write(temp.path().join(CONFIG), "<Game><broken").unwrap();
        std::fs::write(temp.path().join("AppxManifest.xml"), "<Package />").unwrap();
        assert!(game_in(&installed(temp.path())).is_err());
        // A config with no manifest beside it cannot be started, and says so.
        let lone = tempfile::tempdir().unwrap();
        std::fs::write(
            lone.path().join(CONFIG),
            include_str!("../../../core/src/library/fixtures/microsoft_game.config"),
        )
        .unwrap();
        assert!(game_in(&installed(lone.path())).is_err());
    }

    #[test]
    fn xbox_games_folders_add_only_games_that_are_not_registered() {
        let drive = tempfile::tempdir().unwrap();
        let title = drive.path().join("XboxGames").join("Contoso Racing");
        fixture(&title, true);
        let unregistered = discover_from(Ok(vec![]), &[drive.path().to_path_buf()]);
        assert!(unregistered.available);
        assert!(unregistered.errors.is_empty(), "{:?}", unregistered.errors);
        let game = &unregistered.games[0];
        assert!(game.launch.is_none());
        assert!(game.unavailable_reason.is_some());

        // The same game, registered: found once, with its launch.
        let registered = discover_from(Ok(vec![installed(&title)]), &[drive.path().to_path_buf()]);
        assert_eq!(registered.games.len(), 1);
        assert!(registered.games[0].launch.is_some());
    }

    #[test]
    fn a_failed_package_list_is_an_error_not_an_empty_library() {
        let scan = discover_from(Err(anyhow::anyhow!("access denied")), &[]);
        assert!(scan.games.is_empty() && !scan.available);
        assert_eq!(scan.errors.len(), 1);
        assert!(
            scan.errors[0].contains("access denied"),
            "{:?}",
            scan.errors
        );
    }

    /// Reads this PC's real package list. Run by hand: `cargo test -p butterpollo-windows
    /// -- --ignored real_package_list`. Passes whether or not the PC has any Xbox game.
    #[test]
    #[ignore = "reads the PC's installed packages"]
    fn real_package_list() {
        let listed = packages().expect("the package list");
        println!("{} packages that are not frameworks", listed.len());
        assert!(!listed.is_empty(), "a Windows user has packages");
        let scan = discover();
        println!("{} games, errors {:?}", scan.games.len(), scan.errors);
        for game in &scan.games {
            println!("{} {} {:?}", game.id, game.name, game.launch);
        }
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
    }
}
