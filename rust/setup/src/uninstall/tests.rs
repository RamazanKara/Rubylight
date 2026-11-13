use super::*;
#[test]
fn only_rubylights_own_service_is_stopped_or_deleted() {
    let install = Path::new(r"C:\Program Files\Rubylight");
    let ours = install.join("butterpollo-service.exe");
    assert_eq!(service_steps(Some(&ours), install), (true, true));
    // The same folder as the service or the Apps entry spells it.
    let spelled = Path::new(r"c:\PROGRAM FILES\rubylight\butterpollo-service.exe");
    assert_eq!(service_steps(Some(spelled), install), (true, true));
    assert_eq!(
        service_steps(Some(&ours), Path::new(r"C:\Program Files\Rubylight\")),
        (true, true)
    );
    let beside = Path::new(r"C:\Program Files\Rubylight 2\butterpollo-service.exe");
    assert_eq!(service_steps(Some(beside), install), (true, false));
    // Rubylight's program in another folder is stopped, as before, and
    // stays registered.
    let elsewhere = Path::new(r"D:\Other\butterpollo-service.exe");
    assert_eq!(service_steps(Some(elsewhere), install), (true, false));
    // Another host's ApolloService is neither, wherever it is.
    let theirs = Path::new(r"C:\Program Files\Vibepollo\tools\sunshinesvc.exe");
    assert_eq!(service_steps(Some(theirs), install), (false, false));
    let inside = install.join("tools").join("sunshinesvc.exe");
    assert_eq!(service_steps(Some(&inside), install), (false, false));
    assert_eq!(service_steps(None, install), (false, false));
}
#[test]
fn side_by_side_removing_drivers_keeps_what_another_host_may_use() {
    assert!(SHARED_DRIVERS_KEPT.contains("gamepad"));
    assert!(SHARED_DRIVERS_KEPT.contains("libvirtualdisplay"));
    // A Windows checkout may have CRLF line ends.
    let code = include_str!("../uninstall.rs").replace("\r\n", "\n");
    assert!(code.contains(
        "if options.remove_drivers && side_by_side {\n        line(format!(\"note: {SHARED_DRIVERS_KEPT}\"));"
    ));
    assert!(code.contains(
        "} else if options.remove_drivers {\n        progress.set(\"Removing the virtual drivers…\");"
    ));
    // The record goes with the rest of the registration, or a later
    // normal installation would look side by side.
    assert!(code.contains("detect::forget_service();"));
}
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
#[test]
fn child_session_settings_are_restored_only_when_setup_recorded_them() {
    let install = tempfile::tempdir().unwrap();
    write(&install.path().join("butterpollo.exe"), "");
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
    // One in the profile, which its user can write, is ignored.
    write(&profile.join(CHILD_SESSION_SETUP_FILE), "{}");
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
    // Only the PC's record.
    let machine = child_session_record(&profile);
    assert_eq!(
        machine,
        root.path()
            .join("windows-settings")
            .join(CHILD_SESSION_SETUP_FILE)
    );
    write(&machine, "{}");
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        Some(install.path().join("butterpollo.exe"))
    );
    std::fs::remove_file(&machine).unwrap();
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
    write(&machine, "{}");
    std::fs::remove_file(install.path().join("butterpollo.exe")).unwrap();
    assert_eq!(
        child_session_restore_pending(&profile, install.path()),
        None
    );
}
/// HKLM as the uninstaller sees it: RDP on, as child-session setup left it.
#[derive(Default)]
struct FakeRegistry {
    values: std::collections::BTreeMap<(String, String), u32>,
    /// Writes fail (access denied).
    denied: bool,
    /// Writes succeed without changing the value.
    stuck: bool,
    writes: usize,
}
impl FakeRegistry {
    fn rdp_on() -> Self {
        let mut registry = Self::default();
        registry.set(TERMINAL_SERVER, 0);
        registry
    }
    fn set(&mut self, path: &str, value: u32) {
        self.values
            .insert((path.into(), DENY_TS_CONNECTIONS.into()), value);
    }
    fn rdp(&self) -> Option<u32> {
        self.read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)
            .unwrap()
    }
}
impl MachineRegistry for FakeRegistry {
    fn read_dword(&self, path: &str, name: &str) -> Result<Option<u32>> {
        Ok(self.values.get(&(path.into(), name.into())).copied())
    }
    fn write_dword(&mut self, path: &str, name: &str, value: u32) -> Result<()> {
        self.writes += 1;
        if self.denied {
            bail!("access denied");
        }
        if !self.stuck {
            self.values.insert((path.into(), name.into()), value);
        }
        Ok(())
    }
}
/// A recorded setup with the host installed (`exe`) or not.
fn recorded(exe: bool) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let install = tempfile::tempdir().unwrap();
    if exe {
        write(&install.path().join("butterpollo.exe"), "");
    }
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    write(&child_session_record(&profile), "{}");
    (install, root, profile)
}
#[test]
fn a_restore_that_cannot_run_turns_remote_desktop_off_itself() {
    // The host is gone: nothing ran, so the uninstaller turns RDP off and reads it back.
    let (install, _root, profile) = recorded(false);
    let mut registry = FakeRegistry::rdp_on();
    let note = restore_child_session_with(
        &profile,
        install.path(),
        &mut registry,
        || panic!("a record is there"),
        |_| panic!("there is no host to run"),
    )
    .unwrap();
    assert_eq!(registry.rdp(), Some(1));
    assert!(
        note.contains("butterpollo.exe is missing")
            && note.contains("Remote Desktop was turned off (fDenyTSConnections = 1)"),
        "{note}"
    );
    // The host does not start, times out (killed) or crashes: the same.
    let (install, _root, profile) = recorded(true);
    type Outcome = fn() -> Result<(i32, String)>;
    let outcomes: [Outcome; 4] = [
        || bail!("starting butterpollo.exe: access denied"),
        || bail!("butterpollo.exe did not finish within 180 s"),
        || Ok((-1073741819, String::new())),
        // A restore that failed: the host turned RDP off already; again does no harm.
        || Ok((1, "were not restored: access denied.".into())),
    ];
    for outcome in outcomes {
        let mut registry = FakeRegistry::rdp_on();
        let note = restore_child_session_with(
            &profile,
            install.path(),
            &mut registry,
            || panic!("a record is there"),
            |program| {
                assert_eq!(program, install.path().join("butterpollo.exe"));
                outcome()
            },
        )
        .unwrap();
        assert_eq!(registry.rdp(), Some(1), "{note}");
        assert!(note.contains("Remote Desktop was turned off"), "{note}");
        assert!(!note.contains(".."), "{note}");
    }
    // Restored, or nothing to restore: RDP is left as the restore put it.
    let mut registry = FakeRegistry::rdp_on();
    assert_eq!(
        restore_child_session_with(
            &profile,
            install.path(),
            &mut registry,
            || panic!("a record is there"),
            |_| Ok((0, String::new()))
        ),
        None
    );
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        restore_child_session_with(
            &root.path().join("config"),
            install.path(),
            &mut registry,
            || Ok(false),
            |_| panic!("nothing to restore")
        ),
        None
    );
    assert_eq!((registry.rdp(), registry.writes), (Some(0), 0));
}
#[test]
fn a_partly_restored_remote_desktop_is_left_as_the_user_had_it() {
    // The host put RDP back to the user's value (on), then failed on the rest: turned
    // off now, it would stay off, as a later restore skips RDP.
    let (install, _root, profile) = recorded(true);
    let mut registry = FakeRegistry::rdp_on();
    let note = restore_child_session_with(
        &profile,
        install.path(),
        &mut registry,
        || panic!("a record is there"),
        |_| {
            Ok((
                CHILD_SESSION_RESTORE_PARTLY_EXIT,
                "were partly restored: access denied.".into(),
            ))
        },
    )
    .unwrap();
    assert_eq!((registry.rdp(), registry.writes), (Some(0), 0), "{note}");
    assert!(
        note.contains("partly restored: access denied")
            && note.contains("Remote Desktop was left as it is")
            && !note.contains("turned off"),
        "{note}"
    );
    assert!(!note.contains(".."), "{note}");
}
#[test]
fn remote_desktop_setups_block_rule_outlived_its_record_for_is_turned_off() {
    // No record, but setup's block rule is still there: RDP on is setup's, and nothing
    // restores it, so it goes off as the host's enforce_unrecorded turns it off.
    let install = tempfile::tempdir().unwrap();
    write(&install.path().join("butterpollo.exe"), "");
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    let unrecorded = |registry: &mut FakeRegistry, block_rule: fn() -> Result<bool>| {
        restore_child_session_with(&profile, install.path(), registry, block_rule, |_| {
            panic!("there is no record to restore")
        })
    };
    let mut registry = FakeRegistry::rdp_on();
    let note = unrecorded(&mut registry, || Ok(true)).unwrap();
    assert_eq!(registry.rdp(), Some(1));
    assert!(
        note.contains("their record is gone")
            && note.contains(CHILD_SESSION_BLOCK_RULE)
            && note.contains("Remote Desktop was turned off (fDenyTSConnections = 1)"),
        "{note}"
    );
    // RDP is missing (on by default): the same.
    let mut registry = FakeRegistry::default();
    assert!(unrecorded(&mut registry, || Ok(true)).is_some());
    assert_eq!(registry.rdp(), Some(1));
    // Without the rule, RDP on is the user's own; with RDP off, nothing is open and the
    // rule is not even looked for. A rule that can't be read changes nothing.
    type BlockRule = fn() -> Result<bool>;
    let cases: [(FakeRegistry, BlockRule); 3] = [
        (FakeRegistry::rdp_on(), || Ok(false)),
        (FakeRegistry::rdp_on(), || {
            bail!("Windows Firewall is not running")
        }),
        (
            {
                let mut off = FakeRegistry::default();
                off.set(TERMINAL_SERVER, 1);
                off
            },
            || panic!("RDP is off"),
        ),
    ];
    for (mut registry, block_rule) in cases {
        let before = registry.rdp();
        assert_eq!(unrecorded(&mut registry, block_rule), None);
        assert_eq!((registry.rdp(), registry.writes), (before, 0));
    }
}
#[test]
fn a_firewall_rule_that_is_not_there_reads_as_missing() {
    // Read-only on this PC: no rule has this name.
    assert!(!system::firewall_rule_present("Rubylight test: no rule has this name (TCP)").unwrap());
}
#[test]
fn remote_desktop_that_stays_on_is_reported_loudly() {
    let (install, _root, profile) = recorded(false);
    let note = |registry: &mut FakeRegistry| {
        restore_child_session_with(
            &profile,
            install.path(),
            registry,
            || unreachable!(),
            |_| unreachable!(),
        )
        .unwrap()
    };
    let mut denied = FakeRegistry {
        denied: true,
        ..FakeRegistry::rdp_on()
    };
    let found = note(&mut denied);
    assert!(
        found.contains("Turning Remote Desktop off failed")
            && found.contains("access denied")
            && found.contains("turn it off in Windows' settings"),
        "{found}"
    );
    // Written but read back on.
    let mut stuck = FakeRegistry {
        stuck: true,
        ..FakeRegistry::rdp_on()
    };
    let found = note(&mut stuck);
    assert!(
        found.contains("Turning Remote Desktop off failed")
            && found.contains("it reads Some(0) after writing 1"),
        "{found}"
    );
    // Group policy wins over the value written.
    let mut policy = FakeRegistry::rdp_on();
    policy.set(TERMINAL_SERVICES_POLICY, 0);
    let found = note(&mut policy);
    assert_eq!(policy.rdp(), Some(1));
    assert!(
        found.contains("Turning Remote Desktop off failed") && found.contains("group policy"),
        "{found}"
    );
    // A policy that turns it off is no problem.
    let mut off = FakeRegistry::rdp_on();
    off.set(TERMINAL_SERVICES_POLICY, 1);
    assert!(note(&mut off).contains("Remote Desktop was turned off"));
}
#[test]
fn the_uninstaller_switches_remote_desktop_where_the_host_does() {
    use butterpollo_core::child_session as host;
    assert_eq!(TERMINAL_SERVER, host::TERMINAL_SERVER);
    assert_eq!(TERMINAL_SERVICES_POLICY, host::TERMINAL_SERVICES_POLICY);
    // Read-only on this PC: the value is a DWORD or absent, and a missing one reads as None.
    let value = NativeRegistry
        .read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)
        .unwrap();
    assert!(matches!(value, None | Some(0) | Some(1)), "{value:?}");
    assert_eq!(
        NativeRegistry
            .read_dword(r"SOFTWARE\Rubylight\No\Such\Key", "Missing")
            .unwrap(),
        None
    );
    // A value that is not a DWORD is an error, not a missing one.
    assert!(
        NativeRegistry
            .read_dword(
                r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
                "ProductName"
            )
            .is_err()
    );
}
#[test]
fn a_factory_reset_keeps_the_child_session_record_a_failed_restore_needs() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    let records = vec![child_session_record(&profile)];
    let fill = || {
        for record in &records {
            write(record, "{}");
        }
        write(&profile.join("sunshine.conf"), "x");
        write(&profile.join("credentials").join("cacert.pem"), "x");
        write(&profile.join("logs").join("butterpollo.log"), "x");
        write(&root.path().join("state.json"), "x");
        write(&records[0].with_file_name("other.json"), "x");
        // Not a record: it goes with the profile.
        write(&profile.join(CHILD_SESSION_SETUP_FILE), "{}");
    };
    fill();
    assert!(remove_profile(root.path(), &records).is_empty());
    let mut kept = records.clone();
    kept.sort();
    let mut left = files_in(root.path()).unwrap();
    left.sort();
    assert_eq!(
        left, kept,
        "the records stay for a retry, everything else goes"
    );
    assert!(!profile.exists());

    // Restored, or nothing to restore: the whole profile goes.
    fill();
    assert!(remove_profile(root.path(), &[]).is_empty());
    assert!(!root.path().exists());
    assert_eq!(profile_note(root.path(), &[]), None);
}
#[test]
fn a_factory_reset_that_leaves_files_says_which_and_that_pairings_may_remain() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("config");
    let records = [child_session_record(&profile)];
    let key = profile.join("credentials").join("cakey.pem");
    write(&key, "x");
    write(&profile.join("sunshine.conf"), "x");
    write(&records[0], "{}");
    // An indexer or antivirus holds the key open.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&key)
        .unwrap();
    for keep in [&records[..], &[]] {
        let left = remove_profile(root.path(), keep);
        assert_eq!(left, std::slice::from_ref(&key));
        assert!(!profile.join("sunshine.conf").exists());
        let note = profile_note(root.path(), &left).unwrap();
        assert!(note.contains("credentials, paired devices"), "{note}");
        assert!(note.contains(&key.display().to_string()), "{note}");
        assert!(note.contains(&root.path().display().to_string()), "{note}");
    }
    drop(held);
    // Long lists are cut short.
    let many: Vec<PathBuf> = (0..7).map(|n| root.path().join(n.to_string())).collect();
    let note = profile_note(root.path(), &many).unwrap();
    assert!(note.contains("2 more"), "{note}");
    assert!(!note.contains(&many[5].display().to_string()), "{note}");
}
#[test]
fn failed_driver_removals_become_notes() {
    let warning = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5\n";
    let left = "Its device (Root\\RubylightVDisplay) or driver package may still be installed";
    assert_eq!(
        removal_note("Rubylight display", left, Ok((0, warning.into()))).unwrap(),
        "The Rubylight display driver could not be removed: pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5. Its device (Root\\RubylightVDisplay) or driver package may still be installed: remove it in Device Manager; the setup log has the details."
    );
    let note = removal_note(
        "virtual gamepad",
        "It may still be installed",
        Ok((1, String::new())),
    )
    .unwrap();
    assert!(note.contains("exited with code 1"), "{note}");
    // The native libvirtualdisplay removal's error.
    let note = removal_note(
        crate::install::LIBVIRTUALDISPLAY,
        "It may still be installed",
        Err(anyhow::anyhow!(
            "pnputil /remove-device ROOT\\DISPLAY\\0001 failed with exit code 5"
        )),
    )
    .unwrap();
    assert_eq!(
        note,
        "The old libvirtualdisplay driver could not be removed: pnputil /remove-device ROOT\\DISPLAY\\0001 failed with exit code 5. It may still be installed: remove it in Device Manager; the setup log has the details."
    );
    assert_eq!(
        removal_note(
            "virtual gamepad",
            "It may still be installed",
            Ok((0, "done\n".into()))
        ),
        None
    );
}
#[test]
fn a_removal_whose_restore_failed_has_its_own_exit_code() {
    let removed = |unrestored| Removed {
        notes: Vec::new(),
        unrestored,
    };
    assert_eq!(removed(false).exit_code(), 0);
    let code = removed(true).exit_code();
    assert_eq!(code, UNRESTORED_EXIT);
    // Not done, failed, cancelled or done-but-restart.
    assert!(![0, 1, 1223, 3010].contains(&code));
}
#[test]
fn child_session_restore_uses_the_hosts_record_and_flag() {
    assert_eq!(
        CHILD_SESSION_SETUP_FILE,
        butterpollo_core::child_session::SETUP_FILE
    );
    assert_eq!(
        CHILD_SESSION_SETUP_FOLDER,
        butterpollo_core::child_session::SETUP_FOLDER
    );
    // Beside the installed profile: the PC's record is the host's.
    assert_eq!(
        child_session_record(&profile()),
        butterpollo_core::child_session::setup_record()
    );
    assert_eq!(
        CHILD_SESSION_RESTORE_FLAG,
        butterpollo_core::child_session::RESTORE_FLAG
    );
    assert_eq!(
        CHILD_SESSION_BLOCK_RULE,
        butterpollo_core::child_session::BLOCK_RULE
    );
    assert_eq!(
        CHILD_SESSION_RESTORE_PARTLY_EXIT,
        butterpollo_core::child_session::RESTORE_PARTLY_EXIT
    );
}
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
