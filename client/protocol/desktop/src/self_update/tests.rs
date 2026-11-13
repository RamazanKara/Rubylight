use super::*;

fn folder(name: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("nj-self-update-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    // Windows reuses process ids: an earlier run's folder of the same name may still be there.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn only_the_folder_the_setup_installed_is_installed() {
    let installed = folder("installed");
    let zip = folder("zip");
    std::fs::write(installed.join("nightjar.exe"), b"").unwrap();
    std::fs::write(zip.join("nightjar.exe"), b"").unwrap();
    assert_eq!(windows_copy(&installed.join("nightjar.exe"), Some(&installed)), CopyKind::Installed);
    let upper = PathBuf::from(installed.to_string_lossy().to_uppercase());
    if cfg!(windows) { assert_eq!(windows_copy(&installed.join("nightjar.exe"), Some(&upper)), CopyKind::Installed, "Windows paths ignore case"); }
    assert_eq!(windows_copy(&zip.join("nightjar.exe"), Some(&installed)), CopyKind::Portable, "a zip on a PC that also has it installed");
    assert_eq!(windows_copy(&zip.join("nightjar.exe"), None), CopyKind::Portable, "no uninstall entry");
    assert_eq!(windows_copy(&zip.join("nightjar.exe"), Some(&zip.join("gone"))), CopyKind::Portable, "an entry for a folder that's gone");
}

#[test]
fn a_portable_copy_has_no_asset_and_nothing_to_run() {
    let net: Arc<dyn Transport> = Arc::new(crate::update_net::Native::new("test".into()));
    assert_eq!(CopyKind::Portable.asset_name("4.8.0"), None);
    assert!(Steps::for_copy(&CopyKind::Portable, net.clone()).is_none());
    assert!(Steps::for_copy(&CopyKind::Managed, net).is_none());
    assert!(CopyKind::Portable.checks() && !CopyKind::Managed.checks(), "portable keeps its link; a package manager's copy stays out of it");
}

#[test]
fn an_appimage_is_its_runtimes_word_and_where_this_program_runs() {
    let dir = folder("appimage");
    let image = dir.join("Nightjar.AppImage");
    let appdir = dir.join("mount");
    std::fs::create_dir_all(&appdir).unwrap();
    std::fs::write(&image, b"").unwrap();
    std::fs::write(appdir.join("nightjar"), b"").unwrap();
    std::fs::write(dir.join("nightjar"), b"").unwrap();
    let exe = appdir.join("nightjar");
    assert_eq!(appimage_copy(Some(&image), Some(&appdir), &exe), CopyKind::AppImage(image.canonicalize().unwrap()));
    assert_eq!(appimage_copy(None, None, &exe), CopyKind::Managed, "a deb, an rpm or a Flatpak");
    assert_eq!(appimage_copy(Some(&image), None, &exe), CopyKind::Managed, "no runtime");
    assert_eq!(appimage_copy(Some(&image), Some(&appdir), &dir.join("nightjar")), CopyKind::Managed, "a variable leaked into something else");
    assert_eq!(appimage_copy(Some(&dir.join("gone")), Some(&appdir), &exe), CopyKind::Managed);
}

#[test]
fn an_install_updates_to_the_setup_of_the_pcs_own_architecture() {
    let machine = windows_machine();
    assert!(["x86_64", "arm64"].contains(&machine), "{machine}");
    if cfg!(target_arch = "aarch64") {
        assert_eq!(machine, "arm64", "an ARM64 build never offers the x86_64 setup");
    }
    #[cfg(windows)]
    {
        let native = native_machine().expect("IsWow64Process2 exists on every Windows this test runs on");
        let expected = if native == windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_ARM64.0 { "arm64" } else { "x86_64" };
        assert_eq!(machine, expected, "native machine {native:#06x}");
    }
}

#[test]
fn the_asset_names_are_the_ones_the_release_publishes() {
    assert_eq!(windows_setup_asset("4.7.0", "x86_64"), "nightjar-desktop-4.7.0-windows-x86_64-setup.exe");
    assert_eq!(windows_setup_asset("4.7.0", "arm64"), "nightjar-desktop-4.7.0-windows-arm64-setup.exe");
    assert_eq!(CopyKind::Installed.asset_name("4.7.0").unwrap(), windows_setup_asset("4.7.0", windows_machine()));
    assert_eq!(windows_setup_assets("4.7.0", "x86_64", "x86_64"), ["nightjar-desktop-4.7.0-windows-x86_64-setup.exe"]);
    assert_eq!(windows_setup_assets("4.7.0", "arm64", "arm64"), ["nightjar-desktop-4.7.0-windows-arm64-setup.exe"]);
    assert_eq!(
        windows_setup_assets("4.7.0", "arm64", "x86_64"),
        ["nightjar-desktop-4.7.0-windows-arm64-setup.exe", "nightjar-desktop-4.7.0-windows-x86_64-setup.exe"],
        "the emulated x86_64 build moves to the native setup, and stays on its own when a release lacks it"
    );
    assert_eq!(appimage_asset("4.7.0", "x86_64"), "nightjar-desktop-4.7.0-x86_64.AppImage");
    assert_eq!(appimage_asset("4.7.0", "aarch64"), "nightjar-desktop-4.7.0-aarch64.AppImage", "an ARM handheld must not update to the x86_64 image");
    assert_eq!(CopyKind::AppImage("x".into()).asset_name("4.7.0").unwrap(), appimage_asset("4.7.0", std::env::consts::ARCH));
}

#[test]
fn only_githubs_https_hosts_are_allowed() {
    let allowed = |a: &str| download_host(&Url::parse(a).unwrap());
    assert!(allowed("https://github.com/RamazanKara/nightjar/releases/download/v1/x"));
    assert!(allowed("https://release-assets.githubusercontent.com/x?sig=1"));
    assert!(allowed("https://objects.githubusercontent.com/x"));
    assert!(!allowed("http://github.com/x"), "no plain http");
    assert!(!allowed("https://github.com:8443/x"), "no other port");
    assert!(!allowed("https://evil.example/x"));
    assert!(!allowed("https://github.com.evil.example/x"));
    assert!(!allowed("https://evilgithub.com/x"));
    assert!(!allowed("https://api.github.com/x"));
    assert!(api_host(&Url::parse("https://api.github.com/x").unwrap()));
    assert!(!api_host(&Url::parse("https://github.com/x").unwrap()));
}

#[test]
fn the_silent_setup_keeps_its_folder_and_starts_nightjar_again() {
    let arguments = setup_arguments(Path::new("C:\\a b\\setup.log"));
    for wanted in ["/SILENT", "/SUPPRESSMSGBOXES", "/NOCANCEL", "/NORESTART", "/SP-", "/RELAUNCH", "/LOG=C:\\a b\\setup.log"] {
        assert!(arguments.iter().any(|a| a == wanted), "{wanted}");
    }
    assert!(!arguments.iter().any(|a| a.to_uppercase().starts_with("/DIR")), "the firewall asks once per path");
}

#[test]
fn the_setup_keeps_what_the_update_relies_on() {
    let iss = include_str!("../../packaging/windows/nightjar.iss");
    assert!(iss.contains("PrivilegesRequired=lowest"), "no UAC prompt");
    assert!(iss.contains("AppId={{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}"), "the same install is upgraded");
    assert!(iss.contains("DisableDirPage=yes") && !iss.contains("UsePreviousAppDir=no"), "the same folder");
    assert!(iss.contains("procedure DeinitializeSetup") && iss.contains("'/RELAUNCH'"), "Nightjar comes back");
    assert!(iss.contains(&format!("AppMutex={STREAM_MUTEX}\n")), "the setup stops for a stream");
    assert!(iss.contains("RegDeleteValue(HKCU, 'Software\\Microsoft\\Windows\\CurrentVersion\\Run', 'Nightjar')"), "the uninstaller takes the sign-in start along");
    assert!(iss.contains(&format!("SetupMutex={SETUP_MUTEX}\n")), "a launch can tell the setup runs");
    assert!(iss.contains(&format!("'{AFTER_SETUP}'")) && iss.contains(&format!("Parameters: \"{AFTER_SETUP}\"")), "and the setup's own start says so");
    let data = iss.split("\n[InstallDelete]\n").nth(1).unwrap().split("\n[Files]\n").next().unwrap();
    assert!(!data.contains("userappdata") && !data.contains("config"), "an upgrade deletes nothing of the user's");
    assert!(!data.lines().any(|line| line.starts_with("Type:") && line.contains(".dll")), "no library goes before [Files]: an abort wouldn't bring it back");
    assert!(iss.contains("InstallFinished := True;\n    if WasUpgrade then\n      RemoveStaleDlls()"), "old libraries go only after success");
    assert!(iss.contains("if CurStep = ssInstall then begin\n    WasUpgrade := IsUpgradeOfThisFolder();\n    if WasUpgrade then\n      BackUpDlls();"), "the libraries [Files] replaces are kept first");
    let closing = iss.split("procedure DeinitializeSetup").nth(1).unwrap();
    assert!(closing.find("RestoreDlls()").unwrap() < closing.find("nightjar.exe").unwrap(), "an aborted install gets them back before Nightjar starts");
    assert!(iss.contains("if not InstallFinished and FindFirst(DllBackup"), "a finished one keeps the new ones");
    let files = iss.split("\n[Files]\n").nth(1).unwrap().split("\n[Icons]\n").next().unwrap();
    let last = files.lines().rfind(|line| line.starts_with("Source:")).unwrap();
    assert!(last.contains(r"\nightjar.exe"), "the program is replaced last: {last}");
    #[cfg(windows)]
    assert!(UNINSTALL_KEY.contains("{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}_is1") && iss.contains(UNINSTALL_KEY));
}

#[test]
fn the_installer_smoke_runs_what_update_now_runs() {
    let smoke = include_str!("../../packaging/windows/build-installer.ps1");
    let flags = smoke.lines().find_map(|line| line.strip_prefix("$updateFlags = ")).expect("the smoke's flags");
    let listed: Vec<String> = setup_arguments(Path::new("x")).iter().filter(|a| !a.starts_with("/LOG=")).map(|a| format!("'{a}'")).collect();
    assert_eq!(flags, format!("@({})", listed.join(", ")));
    assert!(smoke.contains(&format!("'{STREAM_MUTEX}'")) && smoke.contains(&format!("'{SETUP_MUTEX}'")), "the same mutexes");
    assert!(smoke.contains(&format!("'{AFTER_SETUP}'")));
}

#[cfg(windows)]
#[test]
fn a_mark_is_seen_from_outside_until_it_goes() {
    let name = format!("Local\\NightjarTest-{}", std::process::id());
    assert!(!mutex_exists(&name));
    let (first, second) = (Mark::new(&name).unwrap(), Mark::new(&name).unwrap());
    assert!(mutex_exists(&name));
    drop(first);
    assert!(mutex_exists(&name), "two streams: the other one still counts");
    drop(second);
    assert!(!mutex_exists(&name));
}

/// A command line that runs `script` in the system's shell.
fn shell(script: &str) -> (PathBuf, Vec<String>) {
    let (program, flag) = if cfg!(windows) { ("cmd", "/c") } else { ("sh", "-c") };
    (program.into(), vec![flag.to_owned(), script.to_owned()])
}

#[test]
fn a_program_that_fails_at_once_is_noticed() {
    let (program, arguments) = shell("exit 3");
    let failure = start_watched(&program, &arguments, Duration::from_secs(10), || false).unwrap_err();
    assert!(failure.contains('3'), "{failure}");
    let (program, arguments) = shell("exit 0");
    let started = Instant::now();
    assert_eq!(start_watched(&program, &arguments, Duration::from_secs(10), || false), Ok(()));
    assert!(started.elapsed() < Duration::from_secs(9), "no reason to wait on");
}

#[test]
fn a_program_that_keeps_running_is_left_running() {
    let (program, arguments) = shell(if cfg!(windows) { "ping -n 4 127.0.0.1 >nul" } else { "sleep 3" });
    let started = Instant::now();
    assert_eq!(start_watched(&program, &arguments, Duration::from_millis(200), || false), Ok(()));
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// Slow to start is no reason to miss a failure: a setup that has not taken its mutex is watched
/// well past the second an AppImage gets, and one that holds it is left at once.
#[test]
fn a_setup_is_watched_until_it_holds_its_mutex() {
    let (program, arguments) = shell(if cfg!(windows) { "ping -n 3 127.0.0.1 >nul & exit 3" } else { "sleep 2; exit 3" });
    let failure = start_watched(&program, &arguments, SETUP_STARTING, || false).unwrap_err();
    assert!(failure.contains('3'), "{failure}");
    let (program, arguments) = shell(if cfg!(windows) { "ping -n 4 127.0.0.1 >nul" } else { "sleep 3" });
    let started = Instant::now();
    assert_eq!(start_watched(&program, &arguments, SETUP_STARTING, || true), Ok(()));
    assert!(started.elapsed() < Duration::from_secs(2), "it holds the mutex: it runs");
}

#[test]
fn a_setup_that_ends_at_once_with_a_failure_keeps_nightjar_open() {
    let dir = folder("early-exit");
    let part = dir.join("download.part");
    // A real program that refuses the setup's arguments.
    #[cfg(windows)]
    std::fs::copy(std::env::var_os("SystemRoot").map(PathBuf::from).unwrap().join("System32\\where.exe"), &part).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&part, b"#!/bin/sh\nexit 3\n").unwrap();
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let name = "nightjar-desktop-9.0.0-windows-x86_64-setup.exe";
    let asset = Asset { name: name.into(), url: String::new(), digest: String::new(), size: 0 };
    assert!(matches!(run_setup(&part, &asset), Err(Failure::Apply(_))));
    assert!(!dir.join(name).exists(), "the setup goes again");
}

#[test]
fn the_setups_word_never_reaches_the_command_line() {
    let args = |list: &[&str]| list.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    assert_eq!(split_after_setup(restart_arguments().into_iter().map(Into::into).collect()), (args(&[]), true), "an AppImage's restart");
    assert_eq!(split_after_setup(args(&[])), (args(&[]), false));
    assert_eq!(split_after_setup(args(&["--host", "x"])), (args(&["--host", "x"]), false));
    assert_eq!(split_after_setup(args(&[AFTER_SETUP])), (args(&[]), true));
    assert_eq!(split_after_setup(args(&[AFTER_SETUP, "--host", "x"])), (args(&["--host", "x"]), true));
    assert!(!install_running(true), "the setup's own start always goes on");
}

#[test]
fn a_setup_that_has_not_taken_its_mutex_yet_is_running() {
    let (young, old) = (Some(Duration::from_secs(1)), Some(Duration::from_secs(60)));
    assert_eq!(installing(young, false), cfg!(windows));
    assert!(installing(old, true));
    assert!(!installing(old, false), "a setup that ended and left its marker");
    assert!(!installing(None, true), "another way of running the setup");
    assert!(!installing(None, false));
}

#[test]
fn the_setup_waits_in_the_users_own_folder() {
    let (config, local) = (PathBuf::from("config"), PathBuf::from("local"));
    assert_eq!(update_dir(None, Some(local.clone())), Some(local.join("Nightjar").join("update")));
    assert_eq!(update_dir(Some(config.clone()), Some(local)), Some(config.join("update")), "a sandboxed run stays in its sandbox");
    assert_eq!(update_dir(None, None), None);
    let iss = include_str!("../../packaging/windows/nightjar.iss");
    assert!(iss.contains(r#"Name: "{localappdata}\Nightjar\update""#), "and it goes with the uninstall");
}

#[test]
fn a_setup_that_cannot_take_its_name_leaves_no_file() {
    let dir = folder("rename");
    let part = dir.join("download.part");
    std::fs::write(&part, b"setup").unwrap();
    let name = "nightjar-desktop-9.0.0-windows-x86_64-setup.exe";
    std::fs::create_dir(dir.join(name)).unwrap();
    let asset = Asset { name: name.into(), url: String::new(), digest: String::new(), size: 5 };
    assert!(matches!(run_setup(&part, &asset), Err(Failure::Apply(_))));
    assert!(!part.exists());
}

#[test]
fn a_setup_that_cannot_start_keeps_nightjar_open() {
    let dir = folder("setup");
    let part = dir.join("download.part");
    std::fs::write(&part, b"not a program").unwrap();
    let asset = Asset { name: "nightjar-desktop-9.0.0-windows-x86_64-setup.txt".into(), url: String::new(), digest: String::new(), size: 13 };
    assert!(matches!(run_setup(&part, &asset), Err(Failure::Apply(_))));
}

#[test]
fn the_replacement_is_whole_and_runnable_or_not_at_all() {
    let dir = folder("replace");
    let target = dir.join("Nightjar.AppImage");
    std::fs::write(&target, b"old").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
    }
    let part = dir.join(".nightjar-update-1.part");
    assert!(replace_file(&part, &target).is_err(), "nothing to put there");
    assert_eq!(std::fs::read(&target).unwrap(), b"old", "and the old one stays");
    std::fs::write(&part, b"new").unwrap();
    replace_file(&part, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(!part.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o750);
    }
    let gone = dir.join("gone.AppImage");
    std::fs::write(&part, b"new").unwrap();
    assert!(replace_file(&part, &gone).is_err(), "the AppImage was removed meanwhile");
    assert!(!part.exists() && !gone.exists());
}

#[test]
fn only_old_leftovers_of_ours_are_cleared() {
    let dir = old_files("stale");
    remove_stale(&dir, true);
    assert_eq!(left_in(&dir), ["mine.txt", "nightjar-desktop-4.8.0-x86_64.AppImage"]);
}

#[test]
fn beside_an_appimage_the_users_setup_stays() {
    let dir = old_files("beside-appimage");
    remove_stale(&dir, false);
    assert_eq!(left_in(&dir), ["mine.txt", "nightjar-desktop-4.8.0-windows-x86_64-setup.exe", "nightjar-desktop-4.8.0-x86_64.AppImage"]);
}

/// One young set of files proves nothing goes early; then all of it is two hours old.
fn old_files(name: &str) -> PathBuf {
    let dir = folder(name);
    for name in [".nightjar-update-1-2.part", "nightjar-desktop-4.8.0-windows-x86_64-setup.exe", "nightjar-desktop-4.8.0-x86_64.AppImage", "mine.txt"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    remove_stale(&dir, true);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4, "young ones may be another Nightjar's");
    let long_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        File::options().write(true).open(entry.path()).unwrap().set_modified(long_ago).unwrap();
    }
    dir
}

fn left_in(dir: &Path) -> Vec<String> {
    let mut left: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    left
}

#[test]
fn an_attempt_is_read_once_and_only_from_github() {
    let path = folder("attempt").join("update-pending.json");
    assert_eq!(take_attempt(&path), None);
    let attempt = Attempt { version: "4.8.0".into(), url: "https://github.com/RamazanKara/nightjar/releases/tag/v4.8.0".into() };
    write_attempt(&path, &attempt).unwrap();
    assert_eq!(take_attempt(&path), Some(attempt));
    assert_eq!(take_attempt(&path), None);
    std::fs::write(&path, br#"{"version":"4.8.0","url":"https://evil.example/"}"#).unwrap();
    assert_eq!(take_attempt(&path), None);
    std::fs::write(&path, b"<garbage>").unwrap();
    assert_eq!(take_attempt(&path), None);
    assert!(!path.exists());
}
