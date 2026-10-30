use super::folder::{Package, Request, Response, Writer, handle};
use super::*;
use butterpollo_core::neural::{MANIFEST, Manifest};
use std::{io::Write as _, sync::Arc};

/// A folder of its own under the temp folder, deleted when dropped.
struct Temp(PathBuf);
impl Temp {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tempdir() -> Temp {
    let path = std::env::temp_dir().join(format!("rubylight-neural-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    Temp(path)
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A cached release `tag` in `root` with `files`, as [`extract`] leaves it.
fn package(root: &Path, tag: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let folder = root.join(tag);
    let mut placed = BTreeMap::new();
    for (path, bytes) in files {
        let file = folder.join("files").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, bytes).unwrap();
        placed.insert(
            path.to_string(),
            Placed {
                sha256: sha(bytes),
                size: bytes.len() as u64,
            },
        );
    }
    let package = Package {
        tag: tag.into(),
        asset: format!("AMDNR-v{tag}.zip"),
        digest: String::new(),
        runtime: None,
        runtime_digest: None,
        files: placed,
    };
    std::fs::write(
        folder.join(PACKAGE),
        serde_json::to_string(&package).unwrap(),
    )
    .unwrap();
    folder
}

fn release(root: &Path, tag: &str, dll: &[u8]) -> PathBuf {
    package(
        root,
        tag,
        &[
            ("OptiScaler.dll", dll),
            ("OptiScaler.ini", b"[Upscalers]\n"),
            ("LmxxfNrRuntime.dll", b"runtime"),
            ("LmxxfNrRuntime.pak", b"model"),
            ("OptiScaler/libxess.dll", b"xess"),
            ("README.md", b"the mod's readme"),
            ("SHA256SUMS.txt", b"sums"),
        ],
    )
}

/// A game folder with its .exe, a game-owned README and ReShade as dxgi.dll,
/// in a Steam library's `common`.
fn game(root: &Path) -> PathBuf {
    let folder = root.join("library/common/Game");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, bytes) in [
        ("Game.exe", &b"MZ game"[..]),
        ("README.md", b"the game's readme"),
        ("dxgi.dll", b"ReShade"),
    ] {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    folder.join("Game.exe")
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = walkdir(folder)
        .into_iter()
        .map(|path| {
            path.strip_prefix(folder)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    names.sort();
    names
}
fn walkdir(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        found.push(entry.path());
        if entry.file_type().unwrap().is_dir() {
            found.extend(walkdir(&entry.path()));
        }
    }
    found
}

/// The cached release `folder` as the host hands it to the helper.
fn cached_at(folder: &Path) -> super::folder::Cached {
    Package::read(folder).unwrap().with(folder.to_path_buf())
}

fn install(exe: Option<&Path>, install: Option<&Path>, package: &Path) -> Response {
    handle(
        Request::Install {
            exe: exe.map(Path::to_path_buf),
            install: install.map(Path::to_path_buf),
            package: cached_at(package),
        },
        Writer::User,
        &|_, _| {},
    )
}
fn remove(exe: &Path) -> Response {
    handle(
        Request::Remove {
            exe: exe.to_path_buf(),
        },
        Writer::User,
        &|_, _| {},
    )
}
fn prepare(exe: &Path, package: Option<&Path>) -> Response {
    handle(
        Request::Prepare {
            exe: exe.to_path_buf(),
            install: None,
            package: package.map(cached_at),
        },
        Writer::User,
        &|_, _| {},
    )
}

#[test]
fn the_mod_goes_beside_the_game_under_a_free_name_and_leaves_with_only_its_own_files() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);

    // ReShade holds dxgi.dll: the mod takes the next name and leaves it alone.
    assert_eq!(
        install(Some(&exe), None, &cache),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.8".into(),
            proxy: "d3d12.dll".into(),
            updated: false,
        }
    );
    assert_eq!(std::fs::read(folder.join("dxgi.dll")).unwrap(), b"ReShade");
    assert_eq!(
        std::fs::read(folder.join("d3d12.dll")).unwrap(),
        b"OptiScaler proxy"
    );
    assert_eq!(
        std::fs::read(folder.join("README.md")).unwrap(),
        b"the game's readme"
    );
    assert_eq!(
        names(folder),
        [
            "LmxxfNrRuntime.dll",
            "LmxxfNrRuntime.pak",
            "OptiScaler",
            "OptiScaler.ini",
            "OptiScaler/libxess.dll",
            "README.md",
            "d3d12.dll",
            "dxgi.dll",
            "Game.exe",
            MANIFEST,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    let manifest =
        Manifest::parse(&std::fs::read_to_string(folder.join(MANIFEST)).unwrap()).unwrap();
    assert!(
        !manifest.files.contains_key("README.md") && !manifest.files.contains_key("SHA256SUMS.txt")
    );
    assert_eq!(manifest.folders, ["OptiScaler"]);

    // The menu saved settings, the mod logged, someone replaced the runtime
    // and put a file of their own in the mod's folder.
    std::fs::write(
        folder.join("OptiScaler.ini"),
        "[Upscalers]\nDx12Upscaler=auto\n",
    )
    .unwrap();
    std::fs::write(folder.join("OptiScaler.log"), "log").unwrap();
    std::fs::write(folder.join("LmxxfNrRuntime.dll"), "someone else's").unwrap();
    std::fs::write(folder.join("OptiScaler/user.txt"), "mine").unwrap();
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec!["LmxxfNrRuntime.dll".into()]
        }
    );
    let mut expected = before.clone();
    expected.extend(["LmxxfNrRuntime.dll", "OptiScaler", "OptiScaler/user.txt"].map(String::from));
    expected.sort();
    assert_eq!(names(folder), expected);
    // Nothing left to remove.
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
}

#[test]
fn a_game_with_anti_cheat_or_another_upscaler_mod_is_refused() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    // An Unreal game: anti-cheat two folders above the .exe.
    let install_dir = temp.path().join("Unreal");
    let binaries = install_dir.join("Game/Binaries/Win64");
    std::fs::create_dir_all(&binaries).unwrap();
    std::fs::create_dir_all(install_dir.join("EasyAntiCheat")).unwrap();
    let exe = binaries.join("Game-Win64-Shipping.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let Response::Refused { reason } = install(None, Some(&install_dir), &cache) else {
        panic!()
    };
    assert!(reason.contains("Easy Anti-Cheat"), "{reason}");
    // Without its install folder too.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Refused { .. }
    ));
    assert_eq!(names(&binaries), ["Game-Win64-Shipping.exe"]);

    let exe = game(temp.path());
    std::fs::write(exe.with_file_name("OptiScaler.ini"), "").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("OptiScaler"), "{reason}");
    std::fs::remove_file(exe.with_file_name("OptiScaler.ini")).unwrap();
    // Another OptiScaler under a proxy name.
    std::fs::write(exe.with_file_name("winmm.dll"), "MZ ... OptiScaler ...").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("another copy of OptiScaler"), "{reason}");
    assert!(!exe.with_file_name(MANIFEST).exists());
}

#[test]
fn a_name_taken_by_the_game_is_never_overwritten() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap().to_path_buf();
    for name in butterpollo_core::neural::PROXIES {
        std::fs::write(folder.join(name), format!("game's {name}")).unwrap();
    }
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("every name"), "{reason}");
    std::fs::remove_file(folder.join("version.dll")).unwrap();
    // A file of the package the game has itself.
    std::fs::write(folder.join("LmxxfNrRuntime.dll"), "the game's").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("LmxxfNrRuntime.dll"), "{reason}");
    for name in butterpollo_core::neural::PROXIES
        .iter()
        .filter(|n| **n != "version.dll")
    {
        assert_eq!(
            std::fs::read_to_string(folder.join(name)).unwrap(),
            format!("game's {name}")
        );
    }
    assert_eq!(
        std::fs::read_to_string(folder.join("LmxxfNrRuntime.dll")).unwrap(),
        "the game's"
    );
}

#[test]
fn a_cached_file_that_changed_never_reaches_the_game() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    std::fs::write(cache.join("files/LmxxfNrRuntime.pak"), "tampered").unwrap();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let Response::Error { message } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(message.contains("does not match"), "{message}");
    // The first install goes again at once: nothing of it stays.
    assert_eq!(names(folder), before);
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(names(folder), before);
}

/// The helper copies against the hashes the host holds, never the cache's
/// package.json read again: a file changed with the record rewritten to
/// match after the host read it still never reaches the game. A package
/// naming a file outside the cache is refused.
#[test]
fn the_helper_checks_the_files_against_the_hashes_the_host_holds() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let held = cached_at(&cache);
    std::fs::write(cache.join("files/LmxxfNrRuntime.pak"), "tampered").unwrap();
    let mut rewritten = held.package.clone();
    rewritten.files.insert(
        "LmxxfNrRuntime.pak".into(),
        Placed {
            sha256: sha(b"tampered"),
            size: 8,
        },
    );
    std::fs::write(
        cache.join(PACKAGE),
        serde_json::to_string(&rewritten).unwrap(),
    )
    .unwrap();
    let exe = game(temp.path());
    let before = names(exe.parent().unwrap());
    let install = |package| {
        handle(
            Request::Install {
                exe: Some(exe.clone()),
                install: None,
                package,
            },
            Writer::User,
            &|_, _| {},
        )
    };
    let Response::Error { message } = install(held.clone()) else {
        panic!("a changed cache was copied")
    };
    assert!(message.contains("does not match its release"), "{message}");
    assert_eq!(names(exe.parent().unwrap()), before);
    let mut outside = held;
    outside.package.files.insert(
        "../evil.dll".into(),
        Placed {
            sha256: sha(b"evil"),
            size: 4,
        },
    );
    let Response::Error { message } = install(outside) else {
        panic!("a file outside the cache was named")
    };
    assert!(message.contains("names a file outside it"), "{message}");
    assert_eq!(names(exe.parent().unwrap()), before);
}

/// A launch never applies another release on its own; turning the switch
/// on again does, and keeps the user's settings. A changed file fails the
/// launch.
#[test]
fn only_an_install_applies_another_release_and_a_launch_fails_on_a_changed_file() {
    let temp = tempdir();
    let old = release(temp.path(), "Alpha0.3.5.7", b"OptiScaler old");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    std::fs::remove_file(folder.join("dxgi.dll")).unwrap();
    // The older release had a file the newer one dropped.
    std::fs::write(old.join("files/old.dll"), "old").unwrap();
    let mut package = Package::read(&old).unwrap();
    package.files.insert(
        "old.dll".into(),
        Placed {
            sha256: sha(b"old"),
            size: 3,
        },
    );
    std::fs::write(old.join(PACKAGE), serde_json::to_string(&package).unwrap()).unwrap();
    assert!(matches!(
        install(Some(&exe), None, &old),
        Response::Placed { .. }
    ));
    assert_eq!(
        prepare(&exe, Some(&old)),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.7".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
        }
    );
    std::fs::write(folder.join("OptiScaler.ini"), "mine").unwrap();

    let new = release(&temp.path().join("new"), "Alpha0.3.5.8", b"OptiScaler new");
    assert_eq!(
        prepare(&exe, Some(&new)),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.7".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
        }
    );
    assert_eq!(
        std::fs::read(folder.join("dxgi.dll")).unwrap(),
        b"OptiScaler old"
    );
    assert_eq!(
        install(Some(&exe), None, &new),
        Response::Placed {
            exe: exe.clone(),
            release: "Alpha0.3.5.8".into(),
            proxy: "dxgi.dll".into(),
            updated: true,
        }
    );
    assert_eq!(
        std::fs::read(folder.join("dxgi.dll")).unwrap(),
        b"OptiScaler new"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("OptiScaler.ini")).unwrap(),
        "mine"
    );
    assert!(!folder.join("old.dll").exists());
    assert!(matches!(
        prepare(&exe, Some(&new)),
        Response::Placed { updated: false, .. }
    ));

    std::fs::write(folder.join("LmxxfNrRuntime.dll"), "patched").unwrap();
    let Response::Error { message } = prepare(&exe, Some(&new)) else {
        panic!()
    };
    assert!(message.contains("LmxxfNrRuntime.dll changed"), "{message}");
    std::fs::remove_file(folder.join(MANIFEST)).unwrap();
    let Response::Error { message } = prepare(&exe, None) else {
        panic!()
    };
    assert!(message.contains("gone"), "{message}");

    // The game gained anti-cheat since.
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    std::fs::write(exe.with_file_name("BEClient_x64.dll"), "").unwrap();
    let Response::Refused { reason } = prepare(&exe, Some(&cache)) else {
        panic!()
    };
    assert!(reason.contains("BattlEye"), "{reason}");
}

#[test]
fn the_games_program_is_found_in_its_install_or_chosen() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    // Unreal: the launcher stub at the top, the shipping build below.
    let unreal = temp.path().join("unreal");
    std::fs::create_dir_all(unreal.join("Game/Binaries/Win64")).unwrap();
    std::fs::write(unreal.join("Game.exe"), "MZ").unwrap();
    let shipping = unreal.join("Game/Binaries/Win64/Game-Win64-Shipping.exe");
    std::fs::write(&shipping, "MZ").unwrap();
    let Response::Placed { exe, .. } = install(None, Some(&unreal), &cache) else {
        panic!()
    };
    assert_eq!(exe, shipping);
    // Unity: the game beside its crash handler.
    let unity = temp.path().join("unity");
    std::fs::create_dir_all(&unity).unwrap();
    std::fs::write(unity.join("Game.exe"), "MZ").unwrap();
    std::fs::write(unity.join("UnityCrashHandler64.exe"), "MZ").unwrap();
    let Response::Placed { exe, .. } = install(None, Some(&unity), &cache) else {
        panic!()
    };
    assert_eq!(exe, unity.join("Game.exe"));
    // Two programs: the user picks.
    let two = temp.path().join("two");
    std::fs::create_dir_all(&two).unwrap();
    std::fs::write(two.join("GameDX11.exe"), "MZ").unwrap();
    std::fs::write(two.join("GameDX12.exe"), "MZ").unwrap();
    assert!(matches!(
        install(None, Some(&two), &cache),
        Response::ChooseExe { .. }
    ));
    assert!(matches!(
        install(None, None, &cache),
        Response::ChooseExe { .. }
    ));
    assert!(matches!(
        install(Some(&two.join("missing.exe")), None, &cache),
        Response::Error { .. }
    ));
}

/// An Unreal game's anti-cheat at its install's root is found however the
/// install's path is written, and when the chosen program is outside the
/// install; an install too big to search through is refused.
#[test]
fn anti_cheat_is_found_however_the_install_is_written_and_a_huge_one_is_refused() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let install_dir = temp.path().join("SteamApps/common/Game");
    let binaries = install_dir.join("Game/Plugins/Runtime/Binaries/Win64");
    std::fs::create_dir_all(&binaries).unwrap();
    std::fs::create_dir_all(install_dir.join("EasyAntiCheat")).unwrap();
    let exe = binaries.join("Game-Win64-Shipping.exe");
    std::fs::write(&exe, "MZ").unwrap();
    // The library as the launcher wrote it, not as the folder has it.
    let written = temp.path().join("steamapps/common/game");
    let Response::Refused { reason } = install(Some(&exe), Some(&written), &cache) else {
        panic!()
    };
    assert!(reason.contains("Easy Anti-Cheat"), "{reason}");
    // A program chosen outside the install is not the game's.
    let elsewhere = temp.path().join("Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("Game.exe"), "MZ").unwrap();
    let Response::ChooseExe { reason } = install(
        Some(&elsewhere.join("Game.exe")),
        Some(&install_dir),
        &cache,
    ) else {
        panic!()
    };
    assert!(reason.contains("not in the game's folder"), "{reason}");
    assert_eq!(names(&elsewhere), ["Game.exe"]);

    // More files than the search looks at (fewer in tests).
    let big = temp.path().join("Big");
    std::fs::create_dir_all(&big).unwrap();
    for index in 0..1_000 {
        std::fs::write(big.join(format!("{index}.pak")), "").unwrap();
    }
    std::fs::write(big.join("Game.exe"), "MZ").unwrap();
    let Response::Refused { reason } = install(Some(&big.join("Game.exe")), Some(&big), &cache)
    else {
        panic!()
    };
    assert!(reason.contains("too many files"), "{reason}");
    let Response::ChooseExe { reason } = install(None, Some(&big), &cache) else {
        panic!()
    };
    assert!(reason.contains("too many files"), "{reason}");
    assert!(!big.join(MANIFEST).exists());
}

/// With no install known, the search above the .exe stops at a Program
/// Files folder: anti-cheat installed there for other games is not this
/// game's. A studio's own beside its games still is.
#[test]
fn anti_cheat_installed_for_every_game_does_not_refuse_one() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let programs = temp.path().join("Program Files (x86)");
    std::fs::create_dir_all(programs.join("EasyAntiCheat_EOS")).unwrap();
    std::fs::create_dir_all(programs.join("Game")).unwrap();
    let exe = programs.join("Game/Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(names(&programs.join("Game")), ["Game.exe"]);

    let studio = programs.join("Studio");
    std::fs::create_dir_all(studio.join("EasyAntiCheat")).unwrap();
    std::fs::create_dir_all(studio.join("Game/Bin")).unwrap();
    let exe = studio.join("Game/Bin/Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("Easy Anti-Cheat"), "{reason}");
}

/// A game in the user's profile whose install is not known: BattlEye's own
/// folder in `%LOCALAPPDATA%` is no game's, so the search above the .exe
/// stops at the profile's folders, whatever their case. A studio's
/// anti-cheat beside its game below them still refuses it.
#[test]
fn anti_cheat_in_the_users_profile_does_not_refuse_a_game_there() {
    let temp = tempdir();
    let profile = temp.path().join("Player");
    let local = profile.join(r"AppData\Local");
    std::fs::create_dir_all(local.join("BattlEye")).unwrap();
    std::fs::create_dir_all(local.join(r"Games\Game")).unwrap();
    let exe = local.join(r"Games\Game\Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let shared = [
        profile.clone(),
        profile.join(r"AppData\Roaming"),
        PathBuf::from(local.to_string_lossy().to_uppercase()),
    ];
    let refusal =
        |exe: &Path, shared: &[PathBuf]| super::folder::anti_cheat_refusal(exe, None, shared);
    // Without the profile's folders the search reaches BattlEye's.
    let error = refusal(&exe, &[]).unwrap_err();
    assert!(error.contains("BattlEye"), "{error}");
    assert_eq!(refusal(&exe, &shared), Ok(()));

    let studio = local.join("Studio");
    std::fs::create_dir_all(studio.join("EasyAntiCheat")).unwrap();
    std::fs::create_dir_all(studio.join(r"Game\Bin")).unwrap();
    let exe = studio.join(r"Game\Bin\Game.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let error = refusal(&exe, &shared).unwrap_err();
    assert!(error.contains("Easy Anti-Cheat"), "{error}");

    // This account's own.
    let by_place = super::folder::shared_folders();
    for name in ["USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
        let root = PathBuf::from(std::env::var_os(name).unwrap());
        assert!(by_place.contains(&root), "{name}");
    }
}

/// `path` open without delete sharing, as a running game or a scanner may
/// hold a DLL: it can be neither replaced nor deleted until dropped.
fn hold(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ only.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .unwrap()
}

/// An update cut short (a file the game holds open) leaves each file it
/// replaces in one of its two forms, both the host's: the game does not
/// launch on half an update, the next launch with the release cached
/// finishes it, and turning the switch off takes every file, the mod's DLL
/// included.
#[test]
fn an_update_cut_short_leaves_nothing_a_removal_misses() {
    let temp = tempdir();
    let old = release(temp.path(), "Alpha0.3.5.7", b"OptiScaler old");
    let new = package(
        &temp.path().join("new"),
        "Alpha0.3.5.8",
        &[
            ("OptiScaler.dll", b"OptiScaler new"),
            ("OptiScaler.ini", b"[Upscalers]\n"),
            ("LmxxfNrRuntime.dll", b"runtime new"),
            ("LmxxfNrRuntime.pak", b"model"),
            ("OptiScaler/libxess.dll", b"xess new"),
            ("README.md", b"the mod's readme"),
        ],
    );
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let read = |path: &str| std::fs::read(folder.join(path)).unwrap();
    for finish in [false, true] {
        assert!(matches!(
            install(Some(&exe), None, &old),
            Response::Placed { .. }
        ));
        let held = hold(&folder.join("OptiScaler/libxess.dll"));
        let Response::Error { message } = install(Some(&exe), None, &new) else {
            panic!()
        };
        drop(held);
        assert!(message.contains("libxess.dll"), "{message}");
        // The runtime is the new release's, the mod's DLL still the old
        // one's (it goes last), and no partial copy stayed.
        assert_eq!(read("LmxxfNrRuntime.dll"), b"runtime new");
        assert_eq!(read("d3d12.dll"), b"OptiScaler old");
        assert_eq!(read("OptiScaler/libxess.dll"), b"xess");
        assert!(!names(folder).iter().any(|name| name.ends_with("-partial")));
        let Response::Error { message } = prepare(&exe, None) else {
            panic!()
        };
        assert!(message.contains("cut short"), "{message}");
        if finish {
            assert_eq!(
                prepare(&exe, Some(&new)),
                Response::Placed {
                    exe: exe.clone(),
                    release: "Alpha0.3.5.8".into(),
                    proxy: "d3d12.dll".into(),
                    updated: true,
                }
            );
            assert_eq!(read("d3d12.dll"), b"OptiScaler new");
            assert_eq!(read("OptiScaler/libxess.dll"), b"xess new");
            let manifest =
                Manifest::parse(&std::fs::read_to_string(folder.join(MANIFEST)).unwrap()).unwrap();
            assert!(manifest.previous.is_empty());
            assert!(matches!(
                prepare(&exe, Some(&new)),
                Response::Placed { updated: false, .. }
            ));
        }
        assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
        assert_eq!(names(folder), before);
    }
}

/// A first install that fails at any step, its last manifest write
/// included, takes back every file and folder it made.
#[test]
fn a_first_install_cut_short_leaves_nothing_behind() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    // A folder where the manifest is written first: every write of it fails.
    let blocker = folder.join(format!("{MANIFEST}.rubylight-partial"));
    std::fs::create_dir(&blocker).unwrap();
    let before = names(folder);
    let Response::Error { message } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(message.contains(MANIFEST), "{message}");
    assert_eq!(names(folder), before);

    // The first manifest is written; the last fails once every file is copied.
    std::fs::remove_dir(&blocker).unwrap();
    let before = names(folder);
    let response = handle(
        Request::Install {
            exe: Some(exe.clone()),
            install: None,
            package: cached_at(&cache),
        },
        Writer::User,
        &|done, total| {
            if done == total {
                let _ = std::fs::create_dir(&blocker);
            }
        },
    );
    let Response::Error { message } = response else {
        panic!("{response:?}")
    };
    assert!(message.contains(MANIFEST), "{message}");
    std::fs::remove_dir(&blocker).unwrap();
    assert_eq!(names(folder), before);
}

/// A junction at `link` to `target`, as another account sharing a library
/// could plant one.
fn junction(link: &Path, target: &Path) {
    // cmd takes a `/` in a path for an option.
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .args([link, target].map(|path| path.to_string_lossy().replace('/', "\\")))
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
}

/// A link in the game's folder, or the folder itself a link, sends neither
/// a copy nor a delete elsewhere.
#[test]
fn nothing_goes_through_a_link_in_the_games_folder() {
    let temp = tempdir();
    let old = release(temp.path(), "Alpha0.3.5.7", b"OptiScaler old");
    let new = release(&temp.path().join("new"), "Alpha0.3.5.8", b"OptiScaler new");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    assert!(matches!(
        install(Some(&exe), None, &old),
        Response::Placed { .. }
    ));
    // The mod's folder swapped for a junction to one with the same file.
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("libxess.dll"), "xess").unwrap();
    std::fs::remove_dir_all(folder.join("OptiScaler")).unwrap();
    junction(&folder.join("OptiScaler"), &elsewhere);
    let Response::Refused { reason } = remove(&exe) else {
        panic!()
    };
    assert!(reason.contains("link"), "{reason}");
    let Response::Refused { reason } = install(Some(&exe), None, &new) else {
        panic!()
    };
    assert!(reason.contains("link"), "{reason}");
    assert_eq!(names(&elsewhere), ["libxess.dll"]);
    assert_eq!(
        std::fs::read(elsewhere.join("libxess.dll")).unwrap(),
        b"xess"
    );
    std::fs::remove_dir(folder.join("OptiScaler")).unwrap();
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(names(folder), before);

    // The game's folder itself a link (moved and linked back with `mklink
    // /J`): the user's helper follows it once and works where it leads, so
    // the switch goes on and off; a SYSTEM host follows none.
    let linked = temp.path().join("library/common/Linked");
    junction(&linked, folder);
    let through = linked.join("Game.exe");
    let Response::Refused { reason } = handle(
        Request::Install {
            exe: Some(through.clone()),
            install: None,
            package: cached_at(&old),
        },
        Writer::System,
        &|_, _| {},
    ) else {
        panic!("a SYSTEM host wrote through a link")
    };
    assert!(reason.contains("link"), "{reason}");
    assert_eq!(names(folder), before);
    let Response::Placed { exe: placed, .. } = install(Some(&through), None, &old) else {
        panic!("the moved game's folder was refused")
    };
    assert_eq!(names(placed.parent().unwrap()), names(folder));
    assert_ne!(names(folder), before);
    assert!(matches!(
        prepare(&through, Some(&new)),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&through), Response::Removed { kept: vec![] });
    std::fs::remove_dir(&linked).unwrap();
    assert_eq!(names(folder), before);
}

/// A game folder moved and linked back whose link leads nowhere now (the
/// drive it went to is gone for good) is unreachable, not refused: the
/// app can still be deleted, its removal pending.
#[test]
fn a_link_that_leads_nowhere_is_unreachable() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let moved = temp.path().join("moved");
    std::fs::create_dir(&moved).unwrap();
    let linked = temp.path().join("library/common/Linked");
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    junction(&linked, &moved);
    std::fs::remove_dir(&moved).unwrap();
    let exe = linked.join("Game.exe");
    assert_eq!(
        remove(&exe),
        Response::Unreachable {
            folder: linked.display().to_string()
        }
    );
    let app: App = serde_json::from_value(switched_on("Game", "game", &exe)).unwrap();
    drop(before_delete(h, &app).unwrap());
    assert_eq!(pending(h), [Pending::of(&app).unwrap()]);
    std::fs::remove_dir(&linked).unwrap();
}

/// A game folder moved and linked back: the program is found where the link
/// leads, as the install places it, and a removal recorded through the link
/// (an update that failed) leaves the mod its app still has on.
#[test]
fn a_linked_games_stale_removal_leaves_the_mod_its_app_has_on() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let linked = temp.path().join("library/common/Linked");
    junction(&linked, folder);
    let through = linked.join("Game.exe");
    let found = handle(
        Request::Find {
            exe: Some(through.clone()),
            install: Some(linked.clone()),
        },
        Writer::User,
        &|_, _| {},
    );
    let Response::Found { exe: real } = found else {
        panic!("{found:?}")
    };
    assert_eq!(super::folder::real(folder).unwrap(), real.parent().unwrap());
    // The install takes the program found, in the linked install.
    let Response::Placed { exe: placed, .. } = install(Some(&real), Some(&linked), &cache) else {
        panic!("the program found was refused")
    };
    assert_eq!(placed, real);
    crate::steam::update_library(h, |apps, _| {
        apps.push(switched_on("Game", "game", &placed));
        true
    })
    .unwrap();
    // A record from before, through the link.
    record_pending(
        h,
        &[Pending {
            name: "Game".into(),
            exe: through.clone(),
            uuid: Some("game".into()),
        }],
    )
    .unwrap();
    remove_pending(h);
    assert_ne!(names(folder), before, "the mod its app has on was removed");
    assert!(pending(h).is_empty());
    assert_eq!(remove(&placed), Response::Removed { kept: vec![] });
    std::fs::remove_dir(&linked).unwrap();
    assert_eq!(names(folder), before);
}

/// Everyone denied adding to `folder` until dropped, as Program Files is to users.
struct Unwritable<'a>(&'a Path);
impl<'a> Unwritable<'a> {
    fn new(folder: &'a Path) -> Self {
        let denied = std::process::Command::new("icacls")
            .arg(folder)
            .args(["/deny", "*S-1-1-0:(WD,AD)"])
            .output()
            .unwrap();
        assert!(denied.status.success(), "{denied:?}");
        Self(folder)
    }
}
impl Drop for Unwritable<'_> {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(self.0)
            .args(["/remove:d", "*S-1-1-0"])
            .output();
    }
}

/// A folder the user may not write is the helper's answer, not an error: a
/// SYSTEM host writes it only when nobody but administrators may change it
/// or a folder above it, and a host that is not SYSTEM refuses.
#[test]
fn a_folder_only_administrators_may_write_is_written_only_by_system_and_only_then() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let denied = Response::NotWritable {
        folder: folder.to_path_buf(),
    };
    {
        let _unwritable = Unwritable::new(folder);
        assert_eq!(install(Some(&exe), None, &cache), denied);
        assert_eq!(names(folder), before);
    }
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    {
        let _unwritable = Unwritable::new(folder);
        assert_eq!(remove(&exe), denied);
    }
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(names(folder), before);

    // This folder's user may change it: SYSTEM does not write it.
    let response = handle(
        Request::Install {
            exe: Some(exe.clone()),
            install: None,
            package: cached_at(&cache),
        },
        Writer::System,
        &|_, _| {},
    );
    let Response::Refused { reason } = response else {
        panic!("{response:?}")
    };
    assert!(reason.contains("nobody but administrators"), "{reason}");
    assert_eq!(names(folder), before);

    // What the host does with the answer.
    let placed = || Response::Placed {
        exe: exe.clone(),
        release: "Alpha0.3.5.8".into(),
        proxy: "dxgi.dll".into(),
        updated: false,
    };
    assert_eq!(as_system(denied.clone(), true, placed), placed());
    let Response::Refused { reason } = as_system(denied, false, || unreachable!()) else {
        panic!()
    };
    assert!(reason.contains(&folder.display().to_string()), "{reason}");
    assert_eq!(as_system(placed(), true, || unreachable!()), placed());

    // Who may change a folder: Program Files' owners only.
    let trusted = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
    let security = |link, owner: &str, writers: &[&str]| super::folder::Security {
        link,
        owner: owner.into(),
        writers: writers.iter().map(|sid| sid.to_string()).collect(),
    };
    let path = Path::new(r"C:\Program Files\Game");
    let admins = ["S-1-5-18", "S-1-5-32-544", trusted];
    assert_eq!(
        super::folder::verdict(path, &security(false, trusted, &admins)),
        Ok(())
    );
    assert_eq!(
        super::folder::verdict(path, &security(false, "S-1-5-32-544", &[])),
        Ok(())
    );
    // Accounts by name in this PC's language, never a raw SID or entry.
    let users = users();
    for (security, problem) in [
        (security(true, trusted, &admins), "link".to_string()),
        (
            security(false, "S-1-5-21-1-2-3-1001", &admins),
            "owned by another account".into(),
        ),
        (
            security(false, trusted, &["S-1-5-18", "S-1-5-32-545"]),
            format!("changed by {users}"),
        ),
        (
            security(false, trusted, &["S-1-5-18", "ACE type 9"]),
            "changed by another account".into(),
        ),
    ] {
        let error = super::folder::verdict(path, &security).unwrap_err();
        assert!(error.contains(&problem), "{error}");
        assert!(
            !error.contains("S-1-5-21") && !error.contains("ACE"),
            "{error}"
        );
    }
}

/// The Users group as Windows names it here (`BUILTIN\Users` in English).
fn users() -> String {
    let name = butterpollo_windows::process::account_name("S-1-5-32-545").unwrap();
    assert!(name.contains('\\'), "{name}");
    name
}

/// This PC's Program Files, as an ordinary user: where a SYSTEM host's check
/// finds that nobody but administrators may change a program's folder or a
/// folder above it, the user's helper may not write there and writes
/// nothing. The helper runs only on those folders, as it writes wherever the
/// user may:
/// `cargo test -p butterpollo neural::tests::program_files -- --ignored --nocapture`.
#[test]
#[ignore = "reads this PC's Program Files"]
fn program_files_folders_are_left_to_system() {
    assert!(
        !butterpollo_windows::process::elevated().unwrap(),
        "run it as an ordinary user"
    );
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let programs = PathBuf::from(std::env::var_os("ProgramFiles").unwrap());
    // Everything the helper writes starts at the folder's root; some
    // programs' subfolders are closed to the user.
    let names = |folder: &Path| {
        let mut names: Vec<_> = std::fs::read_dir(folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        names.sort();
        names
    };
    let mut closed = Vec::new();
    for entry in std::fs::read_dir(&programs).unwrap().flatten() {
        let folder = entry.path();
        let Some(exe) = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|e| e == "exe"))
        else {
            continue;
        };
        let system = super::folder::administrators_only(&folder);
        if let Err(error) = system {
            println!("{}: SYSTEM's check {error}", folder.display());
            continue;
        }
        let before = names(&folder);
        let user = install(Some(&exe), None, &cache);
        println!(
            "{}: SYSTEM's check Ok; the user's helper {user:?}",
            folder.display()
        );
        assert_eq!(
            names(&folder),
            before,
            "the helper wrote {}",
            folder.display()
        );
        assert!(
            matches!(
                user,
                Response::NotWritable { .. } | Response::Refused { .. }
            ),
            "{user:?}"
        );
        closed.push((folder, user));
    }
    let (folder, user) = closed
        .iter()
        .find(|(folder, _)| folder.ends_with("Rubylight"))
        .expect("Rubylight installed in Program Files");
    assert_eq!(
        *user,
        Response::NotWritable {
            folder: folder.clone()
        }
    );
}

/// The installed host's data folder, read only: a SYSTEM host keeps its
/// download in `neural` there (made writable by administrators only), and
/// the folder and every one above it pass its check:
/// `cargo test -p butterpollo neural::tests::the_installed -- --ignored`.
#[test]
#[ignore = "reads this PC's installed host"]
fn the_installed_hosts_data_folder_holds_a_system_cache() {
    let data = butterpollo_core::paths::data_root();
    assert!(data.is_dir(), "{} is not installed", data.display());
    super::trusted_root(&data, true).unwrap();
}

/// A folder above a SYSTEM host's cache passes when users may only add to
/// it, as to C:\ProgramData, and not when they may move or delete what it
/// holds: they could move the cache away and put their own in its place.
/// The folder's own permissions, its owner taken as Administrators, which
/// an unelevated test cannot give it to.
#[test]
fn a_folder_above_the_cache_lets_nobody_but_administrators_move_what_it_holds() {
    let temp = tempdir();
    let above = temp.path().join("above");
    std::fs::create_dir(&above).unwrap();
    let icacls = |args: &[&str]| {
        let changed = std::process::Command::new("icacls")
            .arg(&above)
            .args(args)
            .output()
            .unwrap();
        assert!(changed.status.success(), "{changed:?}");
    };
    icacls(&[
        "/inheritance:r",
        "/grant:r",
        "*S-1-5-18:(OI)(CI)F",
        "*S-1-5-32-544:(OI)(CI)F",
    ]);
    icacls(&["/grant", "*S-1-5-32-545:(RX)"]);
    icacls(&["/grant", "*S-1-5-32-545:(AD,WD)"]);
    let held = |whole| {
        super::folder::hold(&above, whole, |path, rights| {
            super::folder::security(path, rights).map(|security| super::folder::Security {
                owner: "S-1-5-32-544".into(),
                ..security
            })
        })
    };
    let users = format!("changed by {}", users());
    assert_eq!(held(false), Ok(()));
    // The cache itself is held only when users may not add to it either.
    let error = held(true).unwrap_err();
    assert!(error.contains(&users), "{error}");
    // Users who may delete what it holds could swap the cache's folder.
    icacls(&["/grant", "*S-1-5-32-545:(DC)"]);
    let error = held(false).unwrap_err();
    assert!(error.contains(&users), "{error}");
    // So could users who may delete or rename the folder itself.
    icacls(&["/remove:g", "*S-1-5-32-545"]);
    icacls(&["/grant", "*S-1-5-32-545:(RX)"]);
    icacls(&["/grant", "*S-1-5-32-545:(AD,WD)"]);
    assert_eq!(held(false), Ok(()));
    icacls(&["/grant", "*S-1-5-32-545:(D)"]);
    let error = held(false).unwrap_err();
    assert!(error.contains(&users), "{error}");
}

fn archive(path: &Path, files: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn sums(files: &[(&str, &[u8])]) -> String {
    files
        .iter()
        .map(|(name, bytes)| format!("{}  {name}\n", sha(bytes)))
        .collect()
}

fn file(asset: String) -> Download {
    Download {
        asset,
        url: String::new(),
        digest: String::new(),
        size: 0,
    }
}

/// A release whose package carries the runtime and its own checksum list.
fn listed(tag: &str) -> Release {
    Release {
        tag: tag.into(),
        package: file(format!("AMDNR-v{tag}.zip")),
        runtime: None,
        sums: None,
    }
}

#[test]
fn the_cache_takes_a_release_only_when_every_file_matches_its_checksums() {
    let temp = tempdir();
    let files: [(&str, &[u8]); 4] = [
        ("OptiScaler.dll", b"dll"),
        ("LmxxfNrRuntime.dll", b"runtime"),
        ("LmxxfNrRuntime.pak", b"model"),
        ("OptiScaler/libxess.dll", b"xess"),
    ];
    let text = sums(&files);
    let mut good = files.to_vec();
    good.push(("SHA256SUMS.txt", text.as_bytes()));
    let zip = temp.path().join("good.zip");
    archive(&zip, &good);
    let extract = |zip: &Path, release: &Release| extract(&[zip.to_owned()], temp.path(), release);
    let (folder, package) = extract(&zip, &listed("Alpha0.3.5.8")).unwrap();
    assert_eq!(folder, temp.path().join("Alpha0.3.5.8"));
    assert_eq!(package.files["LmxxfNrRuntime.pak"].sha256, sha(b"model"));
    assert_eq!(
        std::fs::read(folder.join("files/OptiScaler/libxess.dll")).unwrap(),
        b"xess"
    );
    assert_eq!(cached(temp.path()).unwrap().1, package);

    let mut changed = good.clone();
    changed[2] = ("LmxxfNrRuntime.pak", b"other model");
    let zip = temp.path().join("changed.zip");
    archive(&zip, &changed);
    let error = extract(&zip, &listed("Alpha0.3.5.9"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("LmxxfNrRuntime.pak does not match"),
        "{error}"
    );
    assert!(!temp.path().join("Alpha0.3.5.9").exists());

    let mut outside = good.clone();
    outside.push(("../evil.dll", b"evil"));
    let zip = temp.path().join("outside.zip");
    archive(&zip, &outside);
    assert!(extract(&zip, &listed("Alpha0.4")).is_err());
    assert!(!temp.path().join("evil.dll").exists());

    // No checksum list in the archive and none in the release.
    let zip = temp.path().join("unchecked.zip");
    archive(&zip, &files);
    let error = extract(&zip, &listed("Alpha0.5")).unwrap_err().to_string();
    assert!(error.contains("neither"), "{error}");
    // The good release is still the one cached.
    assert_eq!(cached(temp.path()).unwrap().1.tag, "Alpha0.3.5.8");
}

/// 0.3.5.8R2's layout: the package without the runtime, the runtime in the
/// release's component beside notes for the mod's launcher, and the
/// release's checksum list (checked before the download) instead of the
/// archives' own.
#[test]
fn a_release_with_a_runtime_component_takes_only_the_runtime_from_it() {
    let temp = tempdir();
    let package = temp.path().join("AMDNR-v0.3.5.8R2.zip.partial");
    archive(
        &package,
        &[
            ("OptiScaler.dll", b"dll"),
            ("OptiScaler.ini", b"[Upscalers]\n"),
            ("Licenses/lmxxf_LICENSE.txt", b"MIT"),
            ("R2-UPDATE.txt", b"the package's notes"),
        ],
    );
    let component = temp.path().join("AMDNR-lmxxf-0.3.5.8R2.zip.partial");
    archive(
        &component,
        &[
            (
                "Licenses/lmxxf_LICENSE.txt",
                b"MIT, as the component has it",
            ),
            ("LmxxfNrRuntime.dll", b"runtime"),
            ("LmxxfNrRuntime.pak", b"model"),
            ("Licenses/AMDNR_NOTICE.txt", b"notice"),
            ("README.txt", b"use the launcher"),
            ("R2-UPDATE.txt", b"the component's notes"),
        ],
    );
    let mut r2 = Release {
        tag: "Alpha0.3.5.8R2".into(),
        package: file("AMDNR-v0.3.5.8R2.zip".into()),
        runtime: Some(file("AMDNR-lmxxf-0.3.5.8R2.zip".into())),
        sums: Some(file("SHA256SUMS.txt".into())),
    };
    let (folder, taken) = extract(&[package.clone(), component.clone()], temp.path(), &r2).unwrap();
    assert_eq!(taken.runtime.as_deref(), Some("AMDNR-lmxxf-0.3.5.8R2.zip"));
    let names: Vec<_> = taken.files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "Licenses/AMDNR_NOTICE.txt",
            "Licenses/lmxxf_LICENSE.txt",
            "LmxxfNrRuntime.dll",
            "LmxxfNrRuntime.pak",
            "OptiScaler.dll",
            "OptiScaler.ini",
            "R2-UPDATE.txt",
        ]
    );
    let read = |path: &str| std::fs::read(folder.join("files").join(path)).unwrap();
    assert_eq!(read("R2-UPDATE.txt"), b"the package's notes");
    assert_eq!(read("Licenses/lmxxf_LICENSE.txt"), b"MIT");
    assert_eq!(read("LmxxfNrRuntime.pak"), b"model");

    // The package alone, its runtime component left out: not the whole mod.
    r2.runtime = None;
    let error = extract(std::slice::from_ref(&package), temp.path(), &r2)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no LmxxfNrRuntime.dll"), "{error}");
    // A component without a checksum list of its own or the release's.
    r2.runtime = Some(file("AMDNR-lmxxf-0.3.5.8R2.zip".into()));
    r2.sums = None;
    assert!(extract(&[package, component], temp.path(), &r2).is_err());
}

#[test]
fn a_download_must_match_its_published_digest_and_size() {
    let release = Download {
        asset: "AMDNR-v0.3.5.8.zip".into(),
        url: String::new(),
        digest: sha(b"archive"),
        size: 7,
    };
    assert!(verified(7, &sha(b"archive"), &release).is_ok());
    let error = verified(7, &sha(b"archivf"), &release)
        .unwrap_err()
        .to_string();
    assert!(error.contains("SHA-256"), "{error}");
    assert!(verified(8, &sha(b"archive"), &release).is_err());
}

/// A drive letter with nothing behind it on this PC: an unplugged drive.
fn unconnected_drive() -> PathBuf {
    (b'D'..=b'Z')
        .rev()
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .find(|root| !root.exists())
        .expect("a free drive letter")
}

fn switched_on(name: &str, uuid: &str, exe: &Path) -> Value {
    json!({
        "name": name,
        "uuid": uuid,
        KEY: {"exe": exe, "release": "Alpha0.3.5.8", "proxy": "d3d12.dll"},
    })
}

/// The launcher sync drops an uninstalled game without the console's
/// delete: the mod leaves its folder all the same, and a folder on a drive
/// that is not connected stays a pending removal that is tried again.
#[test]
fn a_game_the_library_sync_drops_loses_the_mod_or_has_its_removal_pending() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_ne!(names(folder), before);
    let away = unconnected_drive().join(r"Games\Away\Away.exe");
    crate::steam::update_library(h, |apps, _| {
        apps.push(switched_on("Game", "game", &exe));
        apps.push(switched_on("Away", "away", &away));
        apps.push(json!({"name": "Other", "uuid": "other"}));
        true
    })
    .unwrap();

    // The sync drops both games; the other app stays.
    crate::steam::update_library(h, |apps, _| {
        apps.retain(|app| app["uuid"] == "other");
        true
    })
    .unwrap();
    assert_eq!(names(folder), before);
    let away_pending = Pending {
        name: "Away".into(),
        exe: away.clone(),
        uuid: Some("away".into()),
    };
    assert_eq!(pending(h), std::slice::from_ref(&away_pending));
    // Tried again: the drive is still not there.
    assert!(removal_due(h));
    assert!(!removal_due(h), "not again before the retry interval");
    remove_pending(h);
    assert_eq!(pending(h), std::slice::from_ref(&away_pending));

    // A pending folder that can be reached again loses the mod and its record.
    let back = tempdir();
    let back_exe = game(back.path());
    let back_before = names(back_exe.parent().unwrap());
    assert!(matches!(
        install(Some(&back_exe), None, &cache),
        Response::Placed { .. }
    ));
    record_pending(
        h,
        &[Pending {
            name: "Back".into(),
            exe: back_exe.clone(),
            uuid: None,
        }],
    )
    .unwrap();
    remove_pending(h);
    assert_eq!(names(back_exe.parent().unwrap()), back_before);
    assert_eq!(pending(h), [away_pending]);

    // The console's delete of a game on that drive goes through, its removal pending too.
    let mut gone = h.apps.read().unwrap()[0].clone();
    gone.extra
        .insert(KEY.into(), json!({"exe": away.with_file_name("Other.exe")}));
    before_delete(h, &gone).unwrap();
    assert_eq!(pending(h).len(), 2);
}

fn app_named(name: &str, uuid: &str) -> App {
    App {
        name: name.into(),
        cmd: String::new(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: [("uuid".to_string(), json!(uuid))].into_iter().collect(),
    }
}

/// A Steam folder with app 242050 installed to `Black Flag` and its Valve
/// Anti-Cheat flag `vac` in the app cache (format 28).
fn steam_root(vac: &str) -> Temp {
    let root = tempdir();
    let apps = root.path().join("steamapps");
    std::fs::create_dir_all(apps.join(r"common\Black Flag")).unwrap();
    std::fs::write(
        apps.join("appmanifest_242050.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"242050\"\n\t\"installdir\"\t\t\"Black Flag\"\n}\n",
    )
    .unwrap();
    let mut kv = Vec::new();
    for key in ["appinfo", "common", "category"] {
        kv.push(0x00);
        kv.extend(key.as_bytes());
        kv.push(0);
    }
    kv.push(0x01);
    kv.extend(b"category_8\0");
    kv.extend(vac.as_bytes());
    kv.push(0);
    kv.extend([0x08; 5]);
    let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    data.extend(242050u32.to_le_bytes());
    data.extend(((60 + kv.len()) as u32).to_le_bytes());
    data.extend([0; 60]);
    data.extend(kv);
    data.extend(0u32.to_le_bytes());
    std::fs::create_dir_all(root.path().join("appcache")).unwrap();
    std::fs::write(root.path().join(r"appcache\appinfo.vdf"), data).unwrap();
    root
}

/// A Steam game Steam lists Valve Anti-Cheat for is refused, by its link or
/// by an .exe in its library, though no file in its install shows it; so is
/// one Steam can't say about. A game in a plain folder is not asked about.
#[test]
fn a_game_steam_lists_valve_anti_cheat_for_is_refused() {
    let (vac, clean) = (steam_root("1"), steam_root("0"));
    let mut link = app_named("Black Flag", "flag");
    link.cmd = "steam://rungameid/242050".into();
    let direct = app_named("Black Flag", "flag");
    for (steam, listed) in [(&vac, true), (&clean, false)] {
        let roots = [steam.path().to_path_buf()];
        let exe = steam
            .path()
            .join(r"steamapps\common\Black Flag\AC4BFSP.exe");
        for app in [&link, &direct] {
            let refusal = steam_refusal(app, &exe, &roots);
            assert_eq!(refusal.is_some(), listed, "{}: {refusal:?}", app.cmd);
            if let Some(reason) = refusal {
                assert!(
                    reason.starts_with("Steam lists Valve Anti-Cheat"),
                    "{reason}"
                );
            }
        }
        let plain = steam.path().join(r"Games\Game.exe");
        assert_eq!(steam_refusal(&direct, &plain, &roots), None);
    }
    let reason = steam_refusal(&link, &vac.path().join("Game.exe"), &[]).unwrap();
    assert!(
        reason.starts_with("Rubylight can't tell whether the game uses Valve Anti-Cheat"),
        "{reason}"
    );
}

/// A launcher is never the game: an app whose command starts Steam or
/// another store has the game's program found in its install, a launcher
/// picked is refused, and so is a program outside the game's install.
#[test]
fn the_mod_never_goes_beside_a_launcher_or_outside_the_install() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let launchers = temp.path().join("Launchers");
    std::fs::create_dir(&launchers).unwrap();
    let mut app = app_named("Game", "game");
    app.extra.insert("steam-install-dir".into(), json!(folder));
    for name in [
        "steam.exe",
        "upc.exe",
        "UbisoftConnect.exe",
        "EpicGamesLauncher.exe",
        "Battle.net.exe",
        "EADesktop.exe",
        "GalaxyClient.exe",
        "Playnite.DesktopApp.exe",
    ] {
        let launcher = launchers.join(name);
        std::fs::write(&launcher, "MZ").unwrap();
        let response = install(Some(&launcher), None, &cache);
        assert!(
            matches!(response, Response::ChooseExe { .. }),
            "{name}: {response:?}"
        );
        app.cmd = format!("\"{}\" -applaunch 620", launcher.display());
        assert_eq!(game_command(&app), None, "{name}");
    }
    assert_eq!(names(&launchers).len(), 8, "nothing beside a launcher");

    let other = temp.path().join(r"Other\Other.exe");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "MZ").unwrap();
    let Response::ChooseExe { reason } = install(Some(&other), Some(folder), &cache) else {
        panic!("a program outside the install was taken")
    };
    assert!(reason.contains("not in the game's folder"), "{reason}");
    app.cmd = format!("\"{}\"", other.display());
    assert_eq!(game_command(&app), None);
    app.cmd = format!("\"{}\"", exe.display());
    assert_eq!(game_command(&app), Some(exe.clone()));
    // With no install known, nothing is taken unasked.
    app.extra.remove("steam-install-dir");
    assert_eq!(game_command(&app), None);
    app.cmd = format!("\"{}\"", other.display());
    assert_eq!(game_command(&app), None);
}

/// A script host, an interpreter, another store's launcher or a program of
/// Windows is never the game, picked or in the app's command: an app that
/// runs `pwsh.exe -File x.ps1` with no install known asks for the game's
/// program, and nothing is written beside any of them.
#[test]
fn an_interpreter_or_windows_program_is_never_the_game() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let tools = temp.path().join("Tools");
    std::fs::create_dir(&tools).unwrap();
    let names_ = [
        "pwsh.exe",
        "powershell.exe",
        "python.exe",
        "python3.12.exe",
        "javaw.exe",
        "node.exe",
        "Origin.exe",
        "EALauncher.exe",
        "Launcher.exe",
        "Amazon Games.exe",
        "itch.exe",
        "heroic.exe",
    ];
    for name in names_ {
        let program = tools.join(name);
        std::fs::write(&program, "MZ").unwrap();
        let Response::ChooseExe { reason } = install(Some(&program), None, &cache) else {
            panic!("{name} was taken")
        };
        assert!(reason.contains("not the game"), "{reason}");
    }
    assert_eq!(names(&tools).len(), names_.len(), "nothing beside them");
    let windows =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\notepad.exe");
    let Response::ChooseExe { reason } = install(Some(&windows), None, &cache) else {
        panic!("a program of Windows was taken")
    };
    assert!(reason.contains("part of Windows"), "{reason}");

    // An install whose only top program is an interpreter has no game to pick.
    let bundled = temp.path().join("Bundled");
    std::fs::create_dir(&bundled).unwrap();
    std::fs::write(bundled.join("python.exe"), "MZ").unwrap();
    assert!(matches!(
        install(None, Some(&bundled), &cache),
        Response::ChooseExe { .. }
    ));

    // The switch's request for an app that runs a script, its install
    // unknown, as `enable` makes it.
    let mut app = app_named("Script", "script");
    app.cmd = format!("\"{}\" -File game.ps1", tools.join("pwsh.exe").display());
    assert_eq!(game_command(&app), None);
    let Response::ChooseExe { reason } = install(
        game_command(&app).as_deref(),
        super::install(&app).as_deref(),
        &cache,
    ) else {
        panic!("a program was taken for a script")
    };
    assert!(reason.contains("doesn't know where"), "{reason}");
    assert_eq!(names(&tools).len(), names_.len());
}

/// An app deleted or dropped by the library sync while its install runs
/// has no key to remove the mod by: a delete waits for the job, and an
/// install whose app is gone takes the mod out again or leaves its removal
/// pending.
#[test]
fn an_install_whose_app_went_meanwhile_takes_the_mod_out_again() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let gone = app_named("Gone", "gone");

    start_job(h, "gone", "install", "place").unwrap();
    let Err(error) = before_delete(h, &gone) else {
        panic!("deleted during the install")
    };
    assert!(error.is::<Busy>(), "{error:#}");
    let placed = install(Some(&exe), None, &cache);
    assert!(matches!(placed, Response::Placed { .. }), "{placed:?}");
    let recorded = Pending {
        name: "Gone".into(),
        exe: exe.clone(),
        uuid: Some("gone".into()),
    };
    record_pending(h, &[recorded]).unwrap();
    finish_install(h, "gone", &gone, Some(&exe), Ok(placed));
    assert_eq!(names(folder), before);
    let job = h.neural.lock().jobs["gone"].clone();
    assert_eq!(job.phase, "failed");
    assert!(job.error.unwrap().contains("taken out again"));
    assert!(pending(h).is_empty());
    before_delete(h, &gone).unwrap();

    // A folder that can't be reached now: its removal is pending.
    let away = unconnected_drive().join(r"Games\Away\Away.exe");
    start_job(h, "gone", "install", "place").unwrap();
    finish_install(
        h,
        "gone",
        &gone,
        None,
        Ok(Response::Placed {
            exe: away.clone(),
            release: "Alpha0.3.5.8".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
        }),
    );
    let job = h.neural.lock().jobs["gone"].clone();
    assert!(job.error.unwrap().contains("pending"));
    assert_eq!(
        pending(h),
        [Pending {
            name: "Gone".into(),
            exe: away,
            uuid: Some("gone".into()),
        }]
    );
}

/// The switch's job for `uuid` once it ended (or after 20 s).
async fn ended(h: &Shared, uuid: &str) -> Job {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let job = h.neural.lock().jobs[uuid].clone();
        if matches!(job.phase, "done" | "failed") || Instant::now() > deadline {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A first install records its removal before anything is written, so one
/// cut short is taken out by the pending removals; one that ends well
/// leaves no record.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_install_cut_short_is_taken_out_by_the_pending_removals() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: "AMD Radeon RX 7900 XT".into(),
            arch: "gfx1100".into(),
            compute_units: 84,
            luid: None,
        }],
        driver: Some("26.10.1".into()),
        error: None,
    });
    accept(h).unwrap();
    crate::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Game", "uuid": "game"}));
        true
    })
    .unwrap();
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let cache = release(&cache_root(h), "Alpha0.3.5.8", b"OptiScaler proxy");
    pin_to(h, &cache);

    enable(h, "game", Some(exe.clone())).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    assert!(pending(h).is_empty());
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    set_key(h, "game", None).unwrap();

    // The cache, checked once at this host start, lost a file since: the
    // install fails partway.
    std::fs::remove_file(cache.join("files/LmxxfNrRuntime.pak")).unwrap();
    enable(h, "game", Some(exe.clone())).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "failed", "{job:?}");
    assert_eq!(
        pending(h),
        [Pending {
            name: "Game".into(),
            exe: exe.clone(),
            uuid: Some("game".into()),
        }]
    );
    remove_pending(h);
    assert_eq!(names(folder), before);
    assert!(pending(h).is_empty());
    // The install's failure still shows, not the removal behind it.
    let now = h.neural.lock().jobs["game"].clone();
    assert_eq!((now.phase, now.error), (job.phase, job.error));
}

/// A delete holds the switch's slot until the app is gone, so no launch's
/// update writes the folder meanwhile; a pending removal waits while the
/// switch is busy with its game.
#[test]
fn a_removal_holds_the_switch_until_the_app_is_gone() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let placed = || {
        let placed = install(Some(&exe), None, &cache);
        assert!(matches!(placed, Response::Placed { .. }), "{placed:?}");
    };
    placed();
    let app: App = serde_json::from_value(switched_on("Game", "game", &exe)).unwrap();
    let removing = before_delete(h, &app).unwrap();
    assert_eq!(names(folder), before);
    let error = format!("{:#}", before_launch(h, &app).unwrap_err());
    assert!(error.contains("busy"), "{error}");
    drop(removing);
    assert_eq!(h.neural.lock().jobs["game"].phase, "done");

    placed();
    record_pending(h, &[Pending::of(&app).unwrap()]).unwrap();
    start_job(h, "game", "update", "place").unwrap();
    remove_pending(h);
    assert_ne!(names(folder), before);
    assert_eq!(pending(h).len(), 1);
    update_job(h, "game", |job| job.phase = "done");
    remove_pending(h);
    assert_eq!(names(folder), before);
    assert!(pending(h).is_empty());
}

/// A launch's check and update hold the switch's slot, so a disable or a
/// delete can't race them, and give it back however they end.
#[test]
fn a_launch_holds_the_switch_while_it_checks() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    let mut app = app_named("Game", "game");
    app.extra.insert(KEY.into(), json!({"exe": exe}));
    before_launch(h, &app).unwrap();
    let job = h.neural.lock().jobs["game"].clone();
    assert_eq!((job.action, job.phase), ("update", "done"));
    std::fs::remove_file(exe.with_file_name(MANIFEST)).unwrap();
    assert!(before_launch(h, &app).is_err());
    assert_eq!(h.neural.lock().jobs["game"].phase, "failed");
    start_job(h, "game", "remove", "remove").unwrap();
    let error = before_launch(h, &app).unwrap_err().to_string();
    assert!(error.contains("busy"), "{error}");
    assert_eq!(h.neural.lock().jobs["game"].action, "remove");
}

#[test]
fn the_console_cannot_set_the_hosts_key() {
    let app = |exe: &str| {
        let mut app = app_named("Game", "game");
        app.extra = [(KEY.to_string(), json!({"exe": exe}))]
            .into_iter()
            .collect();
        app
    };
    let stored = app(r"C:\Games\Game.exe");
    let mut saved = app(r"C:\Windows\System32\x.exe");
    keep_key(Some(&stored), &mut saved);
    assert_eq!(saved.extra[KEY]["exe"], r"C:\Games\Game.exe");
    let mut new = app(r"C:\x.exe");
    keep_key(None, &mut new);
    assert!(!new.extra.contains_key(KEY));
}

#[tokio::test]
async fn the_switch_is_offered_only_on_a_capable_gpu_after_the_notice() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: "AMD Radeon RX 6800 XT".into(),
            arch: "gfx1030".into(),
            compute_units: 72,
            luid: None,
        }],
        driver: Some("26.10.1".into()),
        error: None,
    });
    let status = status(h, "missing");
    assert_eq!(status["available"], false);
    assert!(
        status["reason"]
            .as_str()
            .unwrap()
            .contains("RDNA 3 and RDNA 4")
    );
    assert_eq!(status["accepted"], false);
    assert!(matches!(
        enable(h, "missing", None),
        Err(Refusal::NotAccepted)
    ));
    accept(h).unwrap();
    assert!(accepted(h));
    assert!(matches!(
        enable(h, "missing", None),
        Err(Refusal::Unavailable(_))
    ));
    // The probe is kept: a new answer only after a while.
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: "AMD Radeon RX 7900 XT".into(),
            arch: "gfx1100".into(),
            compute_units: 84,
            luid: None,
        }],
        driver: Some("26.10.1".into()),
        error: None,
    });
    assert!(capability(h).is_err());
    h.neural.lock().probe = None;
    assert_eq!(capability(h), Ok("AMD Radeon RX 7900 XT".into()));
    assert!(matches!(enable(h, "missing", None), Err(Refusal::NotFound)));
}

/// A GPU found capable stays so until the host restarts, but not past a
/// change of the display or Graphics card a stream uses: the GPU it named
/// may no longer be the one that streams.
#[test]
fn the_gpu_check_is_asked_again_once_the_streaming_gpu_is_set_otherwise() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let answer = |name: &str| {
        *h.neural.probe.lock().unwrap() = Some(Probe {
            gpus: vec![Gpu {
                name: name.into(),
                arch: "gfx1100".into(),
                compute_units: 84,
                luid: None,
            }],
            driver: Some("26.10.1".into()),
            error: None,
        });
    };
    answer("AMD Radeon RX 7900 XT");
    assert_eq!(capability(h), Ok("AMD Radeon RX 7900 XT".into()));
    answer("AMD Radeon RX 7900 XTX");
    assert_eq!(capability(h), Ok("AMD Radeon RX 7900 XT".into()));
    // Each of the three settings, changed alone, asks again.
    for (n, config) in [
        "adapter_name=AMD Radeon RX 7900 XTX",
        "adapter_name=AMD Radeon RX 7900 XTX\nadapter_pnp_id=PCI\\VEN_1002&DEV_744C",
        "adapter_name=AMD Radeon RX 7900 XTX\nadapter_pnp_id=PCI\\VEN_1002&DEV_744C\noutput_name=\\\\.\\DISPLAY2",
    ]
    .into_iter()
    .enumerate()
    {
        *h.config.write().unwrap() = butterpollo_core::config::Config::parse(config).unwrap();
        let name = format!("AMD Radeon PRO W7900 {n}");
        answer(&name);
        assert_eq!(capability(h), Ok(name.clone()), "{config}");
        answer("AMD Radeon RX 9070 XT");
        assert_eq!(capability(h), Ok(name), "{config}");
    }
}

/// A SYSTEM host asks of the cache itself who may change it, and of each
/// folder above it who may move it or what it holds.
#[test]
fn only_the_cache_itself_is_held_whole() {
    let temp = tempdir();
    let root = temp.path().join("data/neural");
    std::fs::create_dir_all(&root).unwrap();
    let asked = Mutex::new(Vec::new());
    super::trusted_root_with(&root, true, |path, whole| {
        asked.lock().unwrap().push((path.to_path_buf(), whole));
        Ok(())
    })
    .unwrap();
    let asked = asked.into_inner().unwrap();
    assert_eq!(asked.len(), root.ancestors().count());
    for (path, whole) in asked {
        assert_eq!(whole, path == root, "{}", path.display());
    }
    // An ordinary host asks nothing of the kind.
    super::trusted_root_with(&root, false, |_, _| panic!("asked")).unwrap();
}

/// A record that can't be read is never taken for empty and written over:
/// it holds the acceptance and the pending removals.
#[test]
fn a_damaged_record_is_never_written_over() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let path = h.directory.join(SETTINGS_FILE);
    assert!(!path.exists());
    assert!(!accepted(h));
    accept(h).unwrap();
    assert!(accepted(h));
    for damaged in [&b"{\"accepted_at\": 1, \"pending_rem"[..], b"[]"] {
        std::fs::write(&path, damaged).unwrap();
        let error = format!("{:#}", accept(h).unwrap_err());
        assert!(error.contains("damaged"), "{error}");
        assert!(record_pending(h, &[]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), damaged);
    }
    // A pending removals list that doesn't parse fails each change, and
    // nothing writes over the removals it holds.
    let damaged = br#"{"accepted_at": 1, "pending_removals": [{"name": "Game", "exe": 7}]}"#;
    std::fs::write(&path, damaged).unwrap();
    let away = Pending {
        name: "Away".into(),
        exe: unconnected_drive().join(r"Games\Away\Away.exe"),
        uuid: None,
    };
    let error = format!("{:#}", record_pending(h, &[away.clone()]).unwrap_err());
    assert!(error.contains("pending removals are damaged"), "{error}");
    clear_pending(h, &away.exe);
    try_pending(h, std::slice::from_ref(&away));
    remove_pending(h);
    assert!(pending(h).is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
}

/// A cache is used only as this host keeps it: no link on the way to it,
/// for SYSTEM nobody but administrators may change it, its files checked
/// once per host start, and its archives the pinned ones.
#[test]
fn a_cache_is_used_only_as_this_host_keeps_it() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let root = temp.path().join("neural");
    std::fs::create_dir(&root).unwrap();
    trusted_root(&root, false).unwrap();
    // This test's folders are its user's to change.
    let error = format!("{:#}", trusted_root(&root, true).unwrap_err());
    assert!(error.contains("nobody but administrators"), "{error}");
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir_all(elsewhere.join("neural")).unwrap();
    let linked = temp.path().join("linked");
    junction(&linked, &elsewhere);
    for root in [linked.clone(), linked.join("neural")] {
        let error = format!("{:#}", trusted_root(&root, false).unwrap_err());
        assert!(error.contains("is a link"), "{error}");
    }
    std::fs::remove_dir(&linked).unwrap();

    // A launch takes the cached release without hashing it: the copy checks
    // each file as it goes, so a changed one never reaches the game.
    assert_eq!(launch_cache(h).unwrap(), None);
    let folder = release(&cache_root(h), "Alpha0.3.5.8", b"OptiScaler proxy");
    let pak = folder.join("files/LmxxfNrRuntime.pak");
    std::fs::write(&pak, "tampered").unwrap();
    // A release Rubylight does not pin is never applied.
    assert_eq!(launch_cache(h).unwrap(), None);
    pin_to(h, &folder);
    assert_eq!(launch_cache(h).unwrap(), Some(cached_at(&folder)));
    assert_eq!(h.neural.lock().verified, None);
    let exe = game(temp.path());
    let before = names(exe.parent().unwrap());
    let Response::Error { message } = install(Some(&exe), None, &folder) else {
        panic!("a changed cache was copied")
    };
    assert!(message.contains("does not match its release"), "{message}");
    assert_eq!(names(exe.parent().unwrap()), before);
    // A fetch's cache hit hashes it once per host start.
    let package = Package::read(&folder).unwrap();
    let error = format!("{:#}", verify_cache(h, &folder, &package).unwrap_err());
    assert!(error.contains("LmxxfNrRuntime.pak in"), "{error}");
    std::fs::write(&pak, "model").unwrap();
    verify_cache(h, &folder, &package).unwrap();
    std::fs::write(&pak, "tampered").unwrap();
    verify_cache(h, &folder, &package).unwrap();

    // The same tag with other archives is another release.
    let mut package = Package::read(&cache_root(h).join("Alpha0.3.5.8")).unwrap();
    let (one, two, three) = ("1".repeat(64), "2".repeat(64), "3".repeat(64));
    package.digest = one.clone();
    let asset = package.asset.clone();
    let mut pin = neural::Pin {
        tag: "Alpha0.3.5.8",
        package: neural::PinnedFile {
            asset: &asset,
            sha256: &one,
        },
        runtime: Some(neural::PinnedFile {
            asset: "AMDNR-lmxxf-0.3.5.8.zip",
            sha256: &two,
        }),
    };
    assert!(serves(&package, &pin));
    package.runtime = Some("AMDNR-lmxxf-0.3.5.8.zip".into());
    package.runtime_digest = Some(two.clone());
    assert!(serves(&package, &pin));
    pin.runtime.as_mut().unwrap().sha256 = &three;
    assert!(!serves(&package, &pin));
    pin.runtime = None;
    assert!(!serves(&package, &pin));
    package.runtime = None;
    assert!(serves(&package, &pin));
    pin.tag = "Alpha0.3.5.8R2";
    assert!(!serves(&package, &pin));
    pin.tag = "Alpha0.3.5.8";
    pin.package.sha256 = &three;
    assert!(!serves(&package, &pin));
}

/// GitHub as [`fetch`] sees it, on this PC: the pinned release by its tag
/// and each file, a part of one by range; every request logged in order.
struct Server {
    base: String,
    log: Arc<Mutex<Vec<String>>>,
}
type Files = Arc<BTreeMap<String, Vec<u8>>>;
async fn serve(listener: tokio::net::TcpListener, list: Value, files: Files) -> Server {
    use axum::{
        extract::{Path as Url, State},
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::get,
    };
    type Shared = (Arc<Value>, Files, Arc<Mutex<Vec<String>>>);
    async fn list_route(State((list, _, log)): State<Shared>) -> axum::Json<Value> {
        log.lock().unwrap().push("list".into());
        axum::Json((*list).clone())
    }
    async fn file_route(
        State((_, files, log)): State<Shared>,
        Url((_, name)): Url<(String, String)>,
        headers: HeaderMap,
    ) -> axum::response::Response {
        let Some(bytes) = files.get(&name) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let range = headers
            .get(header::RANGE)
            .and_then(|range| range.to_str().ok()?.strip_prefix("bytes=")?.split_once('-'))
            .map(|(start, end)| {
                (
                    start.parse::<usize>().unwrap(),
                    end.parse::<usize>().unwrap(),
                )
            });
        match range {
            Some((start, end)) => {
                log.lock().unwrap().push(format!("{name} (part)"));
                (StatusCode::PARTIAL_CONTENT, bytes[start..=end].to_vec()).into_response()
            }
            None => {
                log.lock().unwrap().push(name);
                bytes.clone().into_response()
            }
        }
    }
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let repository = neural::REPOSITORY;
    let app = axum::Router::new()
        .route(
            &format!("/api/repos/{repository}/releases/tags/{{tag}}"),
            get(list_route),
        )
        .route(
            &format!("/{repository}/releases/download/{{tag}}/{{name}}"),
            get(file_route),
        )
        .with_state((Arc::new(list), files, log.clone()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    Server { base, log }
}

/// [`fetch_using`] without holding the release: one fetch at a time.
async fn fetch(h: &Shared, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(PathBuf, Package)> {
    let _one = h.neural.fetching.lock().await;
    fetch_locked(h, progress).await
}

/// The cached release `folder` held as [`fetch_using`] holds it for an install.
fn pin(h: &Shared, folder: &Path) -> Using {
    *h.neural
        .lock()
        .using
        .entry(folder.to_path_buf())
        .or_default() += 1;
    Using {
        h: h.clone(),
        folder: folder.to_path_buf(),
    }
}

/// The cached release `folder` as the one this host pins: [`fetch`] takes
/// it without asking GitHub.
fn pin_to(h: &Shared, folder: &Path) {
    let package = Package::read(folder).unwrap();
    let file = |asset: String, sha256: String| neural::PinnedFile {
        asset: asset.leak(),
        sha256: sha256.leak(),
    };
    *h.neural.pin.lock().unwrap() = Some(neural::Pin {
        tag: package.tag.leak(),
        package: file(package.asset, package.digest),
        runtime: package
            .runtime
            .zip(package.runtime_digest)
            .map(|(asset, sha256)| file(asset, sha256)),
    });
}

/// `files` as the release this host pins, 0.3.5.8R2's names with their
/// digests.
fn pin_files(h: &Shared, files: &BTreeMap<String, Vec<u8>>) {
    let file = |asset: &'static str| neural::PinnedFile {
        asset,
        sha256: sha(&files[asset]).leak(),
    };
    *h.neural.pin.lock().unwrap() = Some(neural::Pin {
        tag: "Alpha0.3.5.8R2",
        package: file("AMDNR-v0.3.5.8R2.zip"),
        runtime: Some(file("AMDNR-lmxxf-0.3.5.8R2.zip")),
    });
}

/// 0.3.5.8R2 as GitHub lists it: the package without the runtime, the
/// runtime component and the release's checksum list; `sums` may lie.
fn r2_files() -> BTreeMap<String, Vec<u8>> {
    let temp = tempdir();
    let zip = |name: &str, files: &[(&str, &[u8])]| {
        let path = temp.path().join(name);
        archive(&path, files);
        (name.to_owned(), std::fs::read(path).unwrap())
    };
    let mut files: BTreeMap<String, Vec<u8>> = [
        zip(
            "AMDNR-v0.3.5.8R2.zip",
            &[
                ("OptiScaler.dll", b"dll"),
                ("OptiScaler.ini", b"[Upscalers]\n"),
            ],
        ),
        zip(
            "AMDNR-lmxxf-0.3.5.8R2.zip",
            &[
                ("LmxxfNrRuntime.dll", b"runtime"),
                ("LmxxfNrRuntime.pak", b"model"),
            ],
        ),
    ]
    .into_iter()
    .collect();
    let sums: String = files
        .iter()
        .map(|(name, bytes)| format!("{}  {name}\n", sha(bytes)))
        .collect();
    files.insert("SHA256SUMS.txt".into(), sums.into_bytes());
    files
}

/// The release `tag` with `files` as GitHub lists it by its tag.
fn release_list(base: &str, tag: &str, files: &BTreeMap<String, Vec<u8>>) -> Value {
    let assets: Vec<Value> = files
        .iter()
        .map(|(name, bytes)| {
            json!({
                "name": name,
                "browser_download_url": format!("{base}/{}/releases/download/{tag}/{name}", neural::REPOSITORY),
                "digest": format!("sha256:{}", sha(bytes)),
                "size": bytes.len(),
            })
        })
        .collect();
    json!({"tag_name": tag, "published_at": "2026-10-01T00:00:00Z", "assets": assets})
}

/// Only a test's own server gets a plain client: GitHub gets the updater's,
/// HTTPS only and with the User-Agent its API requires (without one it
/// answers 403).
#[tokio::test]
async fn github_gets_the_updaters_client() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let error = super::http(h, Duration::from_secs(5))
        .unwrap()
        .get("http://127.0.0.1:9/")
        .send()
        .await
        .unwrap_err();
    // Refused before it connects: the updater's client is HTTPS only.
    assert!(error.is_builder(), "{error:#}");
}

/// The whole download against a server of its own: the pinned release, the
/// archives' file lists, the checksum list checked against GitHub's digests
/// before any archive comes, the archives, then the cache. An older release
/// in the cache goes only once no copy reads it; GitHub listing other
/// digests than the pin's, a lie in the checksum list or a file other than
/// GitHub's digest says caches nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_goes_list_checksums_archives_cache() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let tag = "Alpha0.3.5.8R2";
    let files = r2_files();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = serve(
        listener,
        release_list(&base, tag, &files),
        Arc::new(files.clone()),
    )
    .await;
    *h.neural.origin.lock().unwrap() = Some(server.base.clone());
    pin_files(h, &files);
    let stale = release(&cache_root(h), "Alpha0.3.5.6", b"OptiScaler proxy");
    let older = release(&cache_root(h), "Alpha0.3.5.7", b"OptiScaler proxy");
    // An install about to copy the older release: it outlasts the newer
    // one's download.
    let using = pin(h, &older);

    // A copy from the older release holds the cache: it stays until then.
    let (held, done) = (mpsc::channel(), mpsc::channel::<()>());
    let copy = {
        let h = h.clone();
        std::thread::spawn(move || {
            let _reading = h.neural.reading();
            held.0.send(()).unwrap();
            let _ = done.1.recv();
        })
    };
    held.1.recv().unwrap();
    let fetching = {
        let h = h.clone();
        tokio::spawn(async move { fetch(&h, &|_, _| {}).await })
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while server.log.lock().unwrap().len() < 6 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!fetching.is_finished());
    assert!(older.exists());
    done.0.send(()).unwrap();
    copy.join().unwrap();
    let (folder, package) = fetching.await.unwrap().unwrap();
    assert_eq!(
        *server.log.lock().unwrap(),
        [
            "list",
            "AMDNR-v0.3.5.8R2.zip (part)",
            "AMDNR-lmxxf-0.3.5.8R2.zip (part)",
            "SHA256SUMS.txt",
            "AMDNR-v0.3.5.8R2.zip",
            "AMDNR-lmxxf-0.3.5.8R2.zip",
        ]
    );
    assert_eq!(folder, cache_root(h).join(tag));
    assert!(!stale.exists());
    assert!(older.exists(), "an install was about to copy it");
    // It goes with the last install that held it; the newest stays.
    let again = pin(h, &older);
    drop(using);
    assert!(older.exists(), "another install still copies it");
    drop(again);
    assert!(!older.exists());
    drop(pin(h, &folder));
    assert!(folder.exists());
    assert!(h.neural.lock().using.is_empty());
    assert_eq!(package.digest, sha(&files["AMDNR-v0.3.5.8R2.zip"]));
    assert_eq!(
        package.runtime_digest,
        Some(sha(&files["AMDNR-lmxxf-0.3.5.8R2.zip"]))
    );
    assert_eq!(
        std::fs::read(folder.join("files/LmxxfNrRuntime.pak")).unwrap(),
        b"model"
    );
    // The same release again: from the cache, without asking GitHub.
    server.log.lock().unwrap().clear();
    assert_eq!(fetch(h, &|_, _| {}).await.unwrap().1, package);
    assert!(server.log.lock().unwrap().is_empty());
    // The same release damaged in the cache, held for an install and one of
    // its files open: it comes again, and the one there is never left half
    // deleted, whether or not it could be replaced meanwhile.
    let whole = names(&folder);
    let ini = folder.join("files/OptiScaler.ini");
    std::fs::write(&ini, "changed").unwrap();
    h.neural.lock().verified = None;
    let using = pin(h, &folder);
    let held = hold(&folder.join("files/LmxxfNrRuntime.pak"));
    let again = fetch(h, &|_, _| {}).await.map(|(folder, _)| folder);
    drop(held);
    assert_eq!(names(&folder), whole, "{again:?}");
    if again.is_err() {
        // Once nothing holds it, it is replaced.
        fetch(h, &|_, _| {}).await.unwrap();
    }
    drop(using);
    assert_eq!(names(&folder), whole);
    assert_eq!(std::fs::read(&ini).unwrap(), b"[Upscalers]\n");
    assert_eq!(cached(&cache_root(h)).unwrap().0, folder);
    std::fs::remove_dir_all(cache_root(h)).unwrap();

    // A stream starts while the switch's own download runs: it stops.
    f.session(f.client(u32::MAX), butterpollo_core::session::Role::Stream);
    let error = fetch(h, &|_, _| {}).await.unwrap_err();
    assert!(error.is::<StreamStarted>(), "{error:#}");
    assert!(cached(&cache_root(h)).is_none());
    h.sessions.lock().unwrap().active.clear();

    // The checksum list doesn't name the package's digest: no archive comes.
    let mut lying = files.clone();
    lying.insert(
        "SHA256SUMS.txt".into(),
        format!(
            "{}  AMDNR-v0.3.5.8R2.zip\n{}  AMDNR-lmxxf-0.3.5.8R2.zip\n",
            sha(b"another"),
            sha(&files["AMDNR-lmxxf-0.3.5.8R2.zip"])
        )
        .into_bytes(),
    );
    // A file other than GitHub's digest says, its file list the same.
    let mut tampered = files.clone();
    let component = tampered.get_mut("AMDNR-lmxxf-0.3.5.8R2.zip").unwrap();
    let at = component.windows(5).position(|w| w == b"model").unwrap();
    component[at + 1] = b'a';
    // A package other than the pinned one, as GitHub lists it.
    let mut other = files.clone();
    other.insert(
        "AMDNR-v0.3.5.8R2.zip".into(),
        files["AMDNR-v0.3.5.8R2.zip"]
            .iter()
            .rev()
            .copied()
            .collect(),
    );
    for (served, listed, problem) in [
        (other.clone(), other, "another SHA-256 than Rubylight pins"),
        (
            lying.clone(),
            lying,
            "does not match the release's SHA256SUMS.txt",
        ),
        (tampered, files.clone(), "SHA-256"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let list = release_list(&base, tag, &listed);
        let server = serve(listener, list, Arc::new(served)).await;
        *h.neural.origin.lock().unwrap() = Some(server.base.clone());
        let error = format!("{:#}", fetch(h, &|_, _| {}).await.unwrap_err());
        assert!(error.contains(problem), "{error}");
        assert!(cached(&cache_root(h)).is_none());
        let log = server.log.lock().unwrap().clone();
        if problem.contains("pins") {
            assert_eq!(log, ["list"]);
        }
        if problem.contains("SHA256SUMS") {
            assert_eq!(log.last().unwrap(), "SHA256SUMS.txt", "{log:?}");
        }
    }
}

/// The switch's download waits until no stream runs, and one that fails
/// tells the console so without the mod's file names; the log has them.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_waits_for_no_stream_and_its_failure_names_no_files() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: "AMD Radeon RX 7900 XT".into(),
            arch: "gfx1100".into(),
            compute_units: 84,
            luid: None,
        }],
        driver: Some("26.10.1".into()),
        error: None,
    });
    accept(h).unwrap();
    crate::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Game", "uuid": "game"}));
        true
    })
    .unwrap();
    f.session(f.client(u32::MAX), butterpollo_core::session::Role::Stream);
    assert!(matches!(enable(h, "game", None), Err(Refusal::Streaming)));
    assert!(!h.neural.lock().jobs.contains_key("game"));
    // A release downloaded already changes nothing: its copy of about 1 GB
    // into the game's folder would take the stream's disk.
    let cache = release(&cache_root(h), "Alpha0.3.5.8", b"OptiScaler proxy");
    assert!(matches!(enable(h, "game", None), Err(Refusal::Streaming)));
    assert!(!h.neural.lock().jobs.contains_key("game"));
    h.sessions.lock().unwrap().active.clear();
    // Once it ends, the switch goes on from the cache, without GitHub.
    pin_to(h, &cache);
    enable(h, "game", None).unwrap();
    let job = ended(h, "game").await;
    // The app names no program or install: the user chooses the game's.
    assert!(job.choose_exe, "{job:?}");
    std::fs::remove_dir_all(cache.parent().unwrap()).unwrap();

    let mut files = r2_files();
    files.insert(
        "SHA256SUMS.txt".into(),
        format!(
            "{}  AMDNR-v0.3.5.8R2.zip\n{}  AMDNR-lmxxf-0.3.5.8R2.zip\n",
            sha(b"another"),
            sha(&files["AMDNR-lmxxf-0.3.5.8R2.zip"])
        )
        .into_bytes(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let list = release_list(&base, "Alpha0.3.5.8R2", &files);
    pin_files(h, &files);
    let server = serve(listener, list, Arc::new(files)).await;
    *h.neural.origin.lock().unwrap() = Some(server.base);
    enable(h, "game", None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let job = loop {
        let job = h.neural.lock().jobs["game"].clone();
        if job.phase == "failed" || Instant::now() > deadline {
            break job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(job.phase, "failed");
    assert_eq!(job.error.as_deref(), Some(DOWNLOAD_FAILED));
}

/// The real release into `C:\tests\neural\fakegame` and out again, then
/// the update to it from an older form of it, cut short by a placed file
/// held open, and turned off; then cut short, finished and turned off:
/// `cargo test -p butterpollo neural::tests::real_release -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "downloads about 1 GB from GitHub"]
async fn real_release_into_a_fake_game() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let base = PathBuf::from(r"C:\tests\neural");
    let game = base.join("fakegame");
    std::fs::create_dir_all(&game).unwrap();
    let exe = game.join("FakeGame.exe");
    std::fs::write(&exe, "MZ dummy").unwrap();
    std::fs::write(game.join("dxgi.dll"), "a dummy loader of another program").unwrap();
    let show = |when: &str| {
        println!("== {when}");
        for name in names(&game) {
            println!("   {name}");
        }
    };
    show("before install");
    let started = Instant::now();
    let (folder, package) = fetch(h, &|_, _| {}).await.unwrap();
    println!(
        "cached {} from {} and {:?} ({} files) in {:?} at {}",
        package.tag,
        package.asset,
        package.runtime,
        package.files.len(),
        started.elapsed(),
        folder.display()
    );
    let response = install(Some(&exe), None, &folder);
    println!("{response:?}");
    assert!(matches!(response, Response::Placed { .. }));
    show("after install");
    assert!(matches!(
        prepare(&exe, Some(&folder)),
        Response::Placed { updated: false, .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    show("after removal");
    assert_eq!(names(&game), ["FakeGame.exe", "dxgi.dll"]);

    // An older form of the release: its first file, the runtime and the
    // mod's DLL differ. The update to the real one renames the first,
    // fails on the runtime the game holds open, and never reaches the DLL.
    let older = base.join("older");
    let _ = std::fs::remove_dir_all(&older);
    let mut previous = package.clone();
    previous.tag = format!("{}-older", package.tag);
    let first = previous.files.keys().next().unwrap().clone();
    for (path, placed) in &mut previous.files {
        let (source, copy) = (
            folder.join("files").join(path),
            older.join("files").join(path),
        );
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        if [first.as_str(), "LmxxfNrRuntime.dll", "OptiScaler.dll"].contains(&path.as_str()) {
            let mut bytes = std::fs::read(&source).unwrap();
            bytes.extend_from_slice(b" older");
            std::fs::write(&copy, &bytes).unwrap();
            *placed = Placed {
                sha256: sha(&bytes),
                size: bytes.len() as u64,
            };
        } else {
            std::fs::hard_link(&source, &copy)
                .or_else(|_| std::fs::copy(&source, &copy).map(drop))
                .unwrap();
        }
    }
    std::fs::write(
        older.join(PACKAGE),
        serde_json::to_string(&previous).unwrap(),
    )
    .unwrap();
    let runtime = game.join("LmxxfNrRuntime.dll");
    for finish in [false, true] {
        assert!(matches!(
            install(Some(&exe), None, &older),
            Response::Placed { .. }
        ));
        show(&format!("after installing {}", previous.tag));
        let held = hold(&runtime);
        let response = install(Some(&exe), None, &folder);
        drop(held);
        println!(
            "update to {} with the runtime held open: {response:?}",
            package.tag
        );
        assert!(matches!(response, Response::Error { .. }));
        let manifest =
            Manifest::parse(&std::fs::read_to_string(game.join(MANIFEST)).unwrap()).unwrap();
        println!(
            "   manifest: release {}, both forms kept for {:?}",
            manifest.release,
            manifest.previous.keys().collect::<Vec<_>>()
        );
        show("after the update was cut short");
        if finish {
            let response = prepare(&exe, Some(&folder));
            println!("the next launch: {response:?}");
            assert!(matches!(response, Response::Placed { updated: true, .. }));
            show("after the update finished");
        }
        assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
        show("after turning it off");
        assert_eq!(names(&game), ["FakeGame.exe", "dxgi.dll"]);
    }
    let _ = std::fs::remove_dir_all(&older);
}
