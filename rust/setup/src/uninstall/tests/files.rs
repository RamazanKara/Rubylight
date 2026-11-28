//! Tests for package, layer, leftover folder, staging and password cleanup on uninstall.
use super::super::*;
use super::write;
use std::collections::BTreeSet;

fn package(root: &Path) {
    write(
        &root.join("manifest.json"),
        r#"[{"path":"butterpollo.exe","sha256":""},{"path":"assets/web/index.html","sha256":""}]"#,
    );
    write(&root.join("butterpollo.exe"), "exe");
    write(&root.join("assets\\web\\index.html"), "html");
    write(&root.join("uninstall.exe"), "stub");
    write(&root.join("drivers\\display\\install.ps1"), "script");
    write(&root.join("drivers\\display\\driver.inf"), "inf");
}
/// A package that also ships licenses\README.txt, as 3.0 does.
fn licensed_package(root: &Path) {
    package(root);
    write(
        &root.join("manifest.json"),
        r#"[{"path":"butterpollo.exe","sha256":""},{"path":"assets/web/index.html","sha256":""},{"path":"licenses/README.txt","sha256":""}]"#,
    );
    write(&root.join("licenses\\README.txt"), "ours");
}
#[test]
fn only_a_folder_holding_the_package_counts_as_an_installation() {
    let downloads = tempfile::tempdir().unwrap();
    write(&downloads.path().join("holiday.jpg"), "photo");
    assert!(!is_package(downloads.path()));
    write(
        &downloads.path().join("manifest.json"),
        r#"[{"path":"other.exe","sha256":""}]"#,
    );
    assert!(!is_package(downloads.path()));
    let install = tempfile::tempdir().unwrap();
    package(install.path());
    assert!(is_package(install.path()));
}
#[test]
fn uninstalling_removes_the_package_but_keeps_the_users_own_files() {
    let install = tempfile::tempdir().unwrap();
    let root = install.path();
    package(root);
    write(&root.join("notes.txt"), "mine");
    write(&root.join("assets\\mine.png"), "mine");
    write(&root.join("drivers\\other\\tool.sys"), "mine");
    let profile_files = [
        "sunshine.conf",
        "sunshine_state.json",
        "sunshine_credentials.json",
        "apps.json",
        "vibeshine_state.json",
        "credentials/cacert.pem",
        "credentials/cakey.pem",
        "covers/game.png",
    ];
    for name in profile_files {
        write(&root.join("config").join(name), "user profile");
    }
    remove_package(root);
    for gone in [
        "butterpollo.exe",
        "assets\\web\\index.html",
        "assets\\web",
        "manifest.json",
        "uninstall.exe",
        "drivers\\display",
    ] {
        assert!(!root.join(gone).exists(), "{gone} remains");
    }
    for kept in ["notes.txt", "assets\\mine.png", "drivers\\other\\tool.sys"] {
        assert!(root.join(kept).exists(), "{kept} was deleted");
    }
    for name in profile_files {
        assert_eq!(
            std::fs::read(root.join("config").join(name)).unwrap(),
            b"user profile"
        );
    }
}
#[test]
fn the_hdr_layer_the_host_registered_verbatim_goes_with_its_folder() {
    let install = Path::new(r"C:\Program Files\ButterpolloRust");
    for registered in [
        r"\\?\C:\Program Files\ButterpolloRust\vulkan-layer\VkLayer_butterpollo_hdr.json",
        r"C:\Program Files\ButterpolloRust\vulkan-layer/VkLayer_butterpollo_hdr.json",
        r"c:\program files\butterpollorust\vulkan-layer\VkLayer_butterpollo_hdr.json",
    ] {
        assert!(layer_in(registered, install), "{registered} stays");
    }
    assert!(layer_in(
        r"\\?\UNC\server\share\Rubylight\vulkan-layer\VkLayer_butterpollo_hdr.json",
        Path::new(r"\\server\share\Rubylight")
    ));
    for other in [
        r"\\?\C:\Program Files\ButterpolloRust2\vulkan-layer\VkLayer_butterpollo_hdr.json",
        r"C:\Program Files\Rubylight\vulkan-layer/VkLayer_butterpollo_hdr.json",
        r"C:\games\Steam\SteamOverlayVulkanLayer64.json",
    ] {
        assert!(!layer_in(other, install), "{other} was removed");
    }
}
#[test]
fn an_earlier_folders_hdr_layer_goes_once_its_manifest_is_gone() {
    let gone = |_: &Path| false;
    assert!(stale_layer(
        r"\\?\C:\Program Files\ButterpolloRust\vulkan-layer\VkLayer_butterpollo_hdr.json",
        gone
    ));
    assert!(!stale_layer(
        r"C:\Program Files\Rubylight\vulkan-layer/VkLayer_butterpollo_hdr.json",
        |_| true
    ));
    // Other programs' layers stay, even with their files gone.
    assert!(!stale_layer(
        r"C:\games\Steam\SteamOverlayVulkanLayer64.json",
        gone
    ));
}
#[test]
fn an_earlier_folder_goes_when_only_empty_folders_remain() {
    let temp = tempfile::tempdir().unwrap();
    let earlier = temp.path().join("ButterpolloRust");
    std::fs::create_dir_all(earlier.join("drivers\\display\\vulkan-layer")).unwrap();
    std::fs::create_dir_all(earlier.join("drivers\\rubylight-display")).unwrap();
    remove_empty_folders(&earlier);
    assert!(!earlier.exists());
    let kept = temp.path().join("Kept");
    std::fs::create_dir_all(kept.join("drivers\\display\\vulkan-layer")).unwrap();
    write(&kept.join("drivers\\rubylight-display\\notes.txt"), "mine");
    remove_empty_folders(&kept);
    assert!(!kept.join("drivers\\display").exists());
    assert!(kept.join("drivers\\rubylight-display\\notes.txt").exists());
}
#[test]
fn the_install_folder_goes_with_the_folders_an_earlier_version_left() {
    // 2.6.x shipped licenses\libvpl; an update to 3.0 deleted its files
    // and left the folders, so the uninstaller found them behind.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Rubylight");
    licensed_package(&root);
    std::fs::create_dir_all(root.join("licenses\\libvpl\\third-party\\empty")).unwrap();
    // A libvpl file an update could not delete is Rubylight's too.
    write(&root.join("licenses\\libvpl\\LICENSE"), "intel");
    write(&root.join("licenses\\libvpl\\x\\y.txt"), "intel");
    let touched = remove_package(&root);
    remove_leftover_folders(&root, &touched);
    assert!(!root.join("licenses").exists());
    // Nothing else is left, so the uninstaller's final rd removes it.
    std::fs::remove_dir(&root).unwrap();
}
#[test]
fn empty_folders_of_the_user_survive_the_uninstall() {
    // The install folder may be one the user chose, with empty folders of
    // their own in it, and not every folder in it is one of ours.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Games");
    licensed_package(&root);
    std::fs::create_dir_all(root.join("licenses\\libvpl")).unwrap();
    std::fs::create_dir_all(root.join("licenses\\mine\\empty")).unwrap();
    std::fs::create_dir_all(root.join("saves\\empty")).unwrap();
    std::fs::create_dir_all(root.join("vulkan-layer")).unwrap();
    let touched = remove_package(&root);
    remove_leftover_folders(&root, &touched);
    assert!(!root.join("licenses\\libvpl").exists());
    assert!(root.join("licenses\\mine").is_dir());
    for kept in ["licenses\\mine\\empty", "saves\\empty", "vulkan-layer"] {
        assert!(root.join(kept).is_dir(), "{kept} was removed");
    }
    // A drive root is never swept.
    let drive = Path::new("C:\\");
    assert!(drive.parent().is_none());
    remove_leftover_folders(drive, &BTreeSet::from([PathBuf::from("C:\\Windows")]));
    assert!(drive.join("Windows").is_dir());
}
/// A junction at `at`. Under a full test run `cmd` can fail to start at all
/// (0xC0000142, its DLLs did not initialize), which says nothing about the code under test.
fn junction(at: &Path, to: &Path) {
    const DLL_INIT_FAILED: i32 = 0xC0000142_u32 as i32;
    let mut tries = 0;
    loop {
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(at)
            .arg(to)
            .output()
            .unwrap();
        tries += 1;
        if tries < 5 && made.status.code() == Some(DLL_INIT_FAILED) {
            std::thread::sleep(std::time::Duration::from_millis(200));
            continue;
        }
        assert!(made.status.success(), "{made:?}");
        return;
    }
}
#[test]
fn leftover_folders_keep_the_users_files_and_never_follow_a_link_out() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Rubylight");
    package(&root);
    write(&root.join("notes.txt"), "mine");
    write(&root.join("licenses\\mine\\note.txt"), "mine");
    std::fs::create_dir_all(root.join("licenses\\libvpl\\deeper")).unwrap();
    // An empty tree and a file outside, reachable by a junction inside.
    let outside = temp.path().join("outside");
    write(&outside.join("libvpl\\keep.txt"), "not ours");
    std::fs::create_dir_all(outside.join("empty")).unwrap();
    let link = junction;
    link(&root.join("through"), &outside);
    let touched = remove_package(&root);
    remove_leftover_folders(&root, &touched);
    assert!(root.join("notes.txt").exists());
    assert!(root.join("licenses\\mine\\note.txt").exists());
    assert!(!root.join("licenses\\libvpl").exists());
    assert!(outside.join("libvpl\\keep.txt").exists());
    assert!(outside.join("empty").exists());
    // A junction named licenses, standing where the legacy folder's
    // parent is, leads nowhere either.
    let other = temp.path().join("Other");
    package(&other);
    link(&other.join("licenses"), &outside);
    let touched = remove_package(&other);
    remove_leftover_folders(&other, &touched);
    assert!(outside.join("libvpl\\keep.txt").exists());
    assert!(outside.join("empty").exists());
}
#[test]
fn setup_staging_goes_with_the_uninstall_and_its_links_lead_nowhere() {
    let temp = tempfile::tempdir().unwrap();
    let link = junction;
    let outside = temp.path().join("outside");
    write(&outside.join("keep.txt"), "not ours");
    // A transaction cut short, an unpacked package and a download.
    let root = temp.path().join("Rubylight");
    package(&root);
    let staging = root.join(STAGING);
    write(&staging.join("package\\butterpollo.exe"), "new");
    write(
        &staging.join("transaction-42-7\\package\\butterpollo.exe"),
        "new",
    );
    write(
        &staging.join("transaction-42-7\\previous\\butterpollo.exe"),
        "old",
    );
    write(&staging.join("Rubylight-setup.exe"), "download");
    link(&staging.join("transaction-42-7\\through"), &outside);
    let touched = remove_package(&root);
    remove_leftover_folders(&root, &touched);
    assert!(!staging.exists());
    assert!(outside.join("keep.txt").exists());
    std::fs::remove_dir(&root).unwrap();
    // The staging folder itself a junction: the junction goes, not what
    // it leads to.
    let other = temp.path().join("Other");
    package(&other);
    link(&other.join(STAGING), &outside);
    let touched = remove_package(&other);
    remove_leftover_folders(&other, &touched);
    assert!(std::fs::symlink_metadata(other.join(STAGING)).is_err());
    assert!(outside.join("keep.txt").exists());
    // A file of that name is not setup's folder and stays.
    write(&other.join(STAGING), "mine");
    remove_leftover_folders(&other, &BTreeSet::new());
    assert!(other.join(STAGING).is_file());
}
#[test]
fn a_staging_file_in_use_goes_at_the_next_restart_with_its_folders() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let staging = temp.path().join(STAGING);
    let held = staging.join("transaction-42-7\\package\\butterpollo.exe");
    write(&held, "new");
    write(&staging.join("Rubylight-setup.exe"), "download");
    std::fs::create_dir_all(staging.join("empty")).unwrap();
    // Open without sharing, as a running updater would hold it.
    let open = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&held)
        .unwrap();
    let mut later = Vec::new();
    remove_staging(&staging, &mut |path: &Path| later.push(path.to_path_buf()));
    // The file in use and every folder above it, deepest first; what was
    // left besides is handed over too, and an empty folder goes now.
    for path in held.ancestors().take(4) {
        assert!(later.iter().any(|p| p == path), "{path:?} not in {later:?}");
    }
    let download = staging.join("Rubylight-setup.exe");
    assert!(!download.exists() || later.contains(&download), "{later:?}");
    assert!(!staging.join("empty").exists());
    assert_eq!(later.last(), Some(&staging), "{later:?}");
    for (at, path) in later.iter().enumerate() {
        assert!(path.starts_with(&staging), "{path:?}");
        assert!(
            !later[at + 1..]
                .iter()
                .any(|after| after.starts_with(path) && after != path),
            "{path:?} before what it holds: {later:?}"
        );
    }
    drop(open);
}
#[test]
fn an_installation_of_only_the_package_leaves_nothing_behind() {
    let install = tempfile::tempdir().unwrap();
    let root = install.path().join("Butterpollo");
    package(&root);
    remove_package(&root);
    std::fs::remove_dir(&root).unwrap();
    assert!(!root.exists());
}
#[test]
fn the_saved_child_session_passwords_go_without_a_record() {
    // No record anywhere: a restore can have removed it and left the password.
    let root = tempfile::tempdir().unwrap();
    // By its long name: the temp folder can be an 8.3 short one (CI's
    // C:\Users\RUNNER~1), which the wipe's link check refuses as leading
    // elsewhere. ProfileList and the known folders give long names.
    let base = std::fs::canonicalize(root.path()).unwrap();
    let base = base.to_str().unwrap();
    let base = PathBuf::from(base.strip_prefix(r"\\?\").unwrap_or(base));
    let folder = |profile: &Path| {
        profile
            .join("AppData")
            .join("Local")
            .join(CHILD_SESSION_PASSWORD_FOLDER)
    };
    let profiles = [base.join("ramaz"), base.join("guest")];
    for profile in &profiles {
        for name in CHILD_SESSION_PASSWORD_FILES {
            write(&folder(profile).join(name), "DPAPI blob");
        }
    }
    write(&folder(&profiles[1]).join("other.txt"), "kept");
    let kept = base.join("kept");
    write(
        &folder(&kept).join(CHILD_SESSION_PASSWORD_FILES[0]),
        "DPAPI blob",
    );
    let held_path = folder(&profiles[0]).join(CHILD_SESSION_PASSWORD_FILES[0]);
    let held = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held_path)
            .unwrap()
    };
    let remove_empty = |folder: &Path| match std::fs::remove_dir(folder) {
        Err(error) if error.kind() != std::io::ErrorKind::DirectoryNotEmpty => Err(error),
        _ => Ok(()),
    };
    let (deleted, failed) = delete_child_session_passwords_in(
        &[
            profiles[0].clone(),
            profiles[1].clone(),
            PathBuf::from("relative"),
            base.join("x").join("..").join("kept"),
        ],
        // The console user's own AppData\Local, here the guest's again: once.
        &[profiles[1].join("AppData").join("Local")],
        wipe_file,
        remove_empty,
    );
    drop(held);
    // The held file is said with its path; every other one goes.
    assert_eq!(deleted.len(), 3, "{deleted:?}");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(
        failed[0].contains(&held_path.display().to_string())
            && failed[0].contains("delete it by hand"),
        "{failed:?}"
    );
    assert!(
        !folder(&profiles[0])
            .join(CHILD_SESSION_PASSWORD_FILES[1])
            .exists()
    );
    // The guest's folder holds something else, and stays; the skipped one is untouched.
    assert_eq!(std::fs::read_dir(folder(&profiles[1])).unwrap().count(), 1);
    assert!(
        folder(&kept)
            .join(CHILD_SESSION_PASSWORD_FILES[0])
            .is_file()
    );
    // Run again: the held one goes now, then its emptied folder; nothing else is said.
    let (deleted, failed) =
        delete_child_session_passwords_in(&profiles, &[], wipe_file, remove_empty);
    assert_eq!((deleted, failed), (vec![held_path], Vec::<String>::new()));
    assert!(!folder(&profiles[0]).exists());
    assert_eq!(
        expand_environment(r"%SystemDrive%\Users\ramaz"),
        format!(r"{}\Users\ramaz", std::env::var("SystemDrive").unwrap())
    );
    assert_eq!(expand_environment(r"C:\100%\%NOPE_X%"), r"C:\100%\%NOPE_X%");
}
#[test]
fn remove_package_with_skips_running_uninstaller_executable() {
    let install = tempfile::tempdir().unwrap();
    let root = install.path();
    package(root);
    let self_exe = root.join("uninstall.exe");
    let mut removed = Vec::new();
    remove_package_with(root, Some(&self_exe), &mut |path| {
        removed.push(path.to_path_buf());
        let _ = std::fs::remove_file(path);
    });
    assert!(
        !removed.iter().any(|p| same_file_path(p, &self_exe)),
        "running uninstall.exe must not be scheduled for reboot deletion: {removed:?}"
    );
    assert!(self_exe.is_file());
    assert!(!root.join("butterpollo.exe").exists());
    assert!(!root.join("manifest.json").exists());
}
#[test]
fn pending_rename_pruning_drops_stale_install_deletions_and_keeps_unrelated_and_old_entries() {
    let encode = |pairs: &[(&str, &str)]| -> Vec<u16> {
        let mut words = Vec::new();
        for (src, dst) in pairs {
            words.extend(src.encode_utf16());
            words.push(0);
            words.extend(dst.encode_utf16());
            words.push(0);
        }
        words.push(0);
        words
    };
    let install = Path::new(r"C:\Program Files\Rubylight");
    let initial = encode(&[
        (r"\??\C:\Program Files\Rubylight\uninstall.exe", ""),
        (r"\??\C:\Other\a.dll", r"\??\C:\Other\b.dll"),
        (r"\??\C:\Program Files\Rubylight2\uninstall.exe", ""),
        (r"\??\C:\Program Files\Rubylight\.butterpollo-old-123", ""),
        (
            r"\??\C:\Program Files\Rubylight\butterpollo.exe",
            r"\??\C:\Program Files\Rubylight\next.exe",
        ),
        (r"\??\C:\Other\c.dll", ""),
        (r"\\?\C:\program files\rubylight\butterpollo.dll", ""),
    ]);
    let pruned = system::prune_pending_rename_words(&initial, install).unwrap();
    let expected = encode(&[
        (r"\??\C:\Other\a.dll", r"\??\C:\Other\b.dll"),
        (r"\??\C:\Program Files\Rubylight2\uninstall.exe", ""),
        (r"\??\C:\Program Files\Rubylight\.butterpollo-old-123", ""),
        (
            r"\??\C:\Program Files\Rubylight\butterpollo.exe",
            r"\??\C:\Program Files\Rubylight\next.exe",
        ),
        (r"\??\C:\Other\c.dll", ""),
    ]);
    assert_eq!(pruned, expected);

    // Pruning the only entry returns an empty Vec so the caller deletes the value.
    let only_ours = encode(&[(r"\??\C:\Program Files\Rubylight\uninstall.exe", "")]);
    assert_eq!(
        system::prune_pending_rename_words(&only_ours, install),
        Some(Vec::new())
    );
    // Unchanged list returns None so the registry is not rewritten.
    assert_eq!(system::prune_pending_rename_words(&expected, install), None);
    // Truncated odd-string buffer fails closed.
    let mut truncated: Vec<u16> = r"\??\C:\Program Files\Rubylight\uninstall.exe"
        .encode_utf16()
        .collect();
    truncated.push(0);
    assert_eq!(
        system::prune_pending_rename_words(&truncated, install),
        None
    );
    // UNC paths are normalized consistently.
    let unc = encode(&[(r"\??\UNC\server\share\Rubylight\uninstall.exe", "")]);
    assert_eq!(
        system::prune_pending_rename_words(&unc, Path::new(r"\\server\share\Rubylight")),
        Some(Vec::new())
    );
}
#[test]
fn rename_aside_for_delete_moves_regular_files_and_skips_dirs_or_already_renamed() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("butterpollo.exe");
    write(&file, "binary");
    let aside = system::rename_aside_for_delete(&file).unwrap();
    assert!(!file.exists());
    assert!(aside.is_file());
    assert!(
        aside
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".butterpollo-old-")
    );
    // Already-renamed files and directories are not renamed again.
    assert!(system::rename_aside_for_delete(&aside).is_none());
    let dir = temp.path().join("subdir");
    std::fs::create_dir(&dir).unwrap();
    assert!(system::rename_aside_for_delete(&dir).is_none());
}
