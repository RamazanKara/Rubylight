use super::folder::{Package, Request, Response, handle};
use super::*;
use butterpollo_core::neural::{MANIFEST, Manifest};
use std::io::Write as _;

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
        runtime: None,
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

fn install(exe: Option<&Path>, install: Option<&Path>, package: &Path) -> Response {
    handle(
        Request::Install {
            exe: exe.map(Path::to_path_buf),
            install: install.map(Path::to_path_buf),
            package: package.to_path_buf(),
        },
        &|_, _| {},
    )
}
fn remove(exe: &Path) -> Response {
    handle(
        Request::Remove {
            exe: exe.to_path_buf(),
        },
        &|_, _| {},
    )
}
fn prepare(exe: &Path, package: Option<&Path>) -> Response {
    handle(
        Request::Prepare {
            exe: exe.to_path_buf(),
            install: None,
            package: package.map(Path::to_path_buf),
        },
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

#[test]
fn a_launch_applies_a_newer_release_keeps_the_settings_and_fails_on_a_changed_file() {
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
    // A program chosen outside the install.
    let elsewhere = temp.path().join("Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("Game.exe"), "MZ").unwrap();
    let Response::Refused { reason } = install(
        Some(&elsewhere.join("Game.exe")),
        Some(&install_dir),
        &cache,
    ) else {
        panic!()
    };
    assert!(reason.contains("Easy Anti-Cheat"), "{reason}");
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
/// launch on half an update, the next launch finishes it, and turning the
/// switch off takes every file, the mod's DLL included.
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
        let Response::Error { message } = prepare(&exe, Some(&new)) else {
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
            package: cache.clone(),
        },
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

#[test]
fn the_console_cannot_set_the_hosts_key() {
    let app = |exe: &str| App {
        name: "Game".into(),
        cmd: String::new(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: [(KEY.to_string(), json!({"exe": exe}))]
            .into_iter()
            .collect(),
    };
    let stored = app(r"C:\Games\Game.exe");
    let mut saved = app(r"C:\Windows\System32\x.exe");
    keep_key(Some(&stored), &mut saved);
    assert_eq!(saved.extra[KEY]["exe"], r"C:\Games\Game.exe");
    let mut new = app(r"C:\x.exe");
    keep_key(None, &mut new);
    assert!(!new.extra.contains_key(KEY));
}

#[test]
fn the_daily_check_waits_a_day_or_an_hour_after_a_failure() {
    let now = 1_000_000;
    assert_eq!(due_at(&json!({}), now), CHECK_INTERVAL);
    assert_eq!(
        due_at(&json!({"checked_at": now}), now),
        now + CHECK_INTERVAL
    );
    assert_eq!(
        due_at(&json!({"checked_at": now, "check_failures": 2}), now),
        now + RETRY_AFTER_FAILURE
    );
    assert_eq!(due_at(&json!({"checked_at": now + 7200}), now), 0);
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
        }],
        driver: Some("26.10.1".into()),
        error: None,
    });
    assert!(capability(h).is_err());
    h.neural.lock().probe = None;
    assert_eq!(capability(h), Ok("AMD Radeon RX 7900 XT".into()));
    assert!(matches!(enable(h, "missing", None), Err(Refusal::NotFound)));
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
    let (folder, package) = fetch(h, false, &|_, _| {}).await.unwrap();
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
        let response = prepare(&exe, Some(&folder));
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
