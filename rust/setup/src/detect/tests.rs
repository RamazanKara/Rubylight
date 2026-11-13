use super::*;
fn product(name: &str, key: &str) -> Product {
    Product {
        key: key.into(),
        name: name.into(),
        version: "2.0.0".into(),
        location: None,
        uninstall: None,
        quiet_uninstall: None,
        msi: key.starts_with('{'),
        root: HKEY_LOCAL_MACHINE,
    }
}
fn program(path: &str) -> Option<PathBuf> {
    Some(PathBuf::from(path))
}
#[test]
fn an_apollo_service_with_another_program_is_never_rubylights() {
    let theirs = |name: &str| {
        (name == "ApolloService").then(|| PathBuf::from(r"C:\Vibepollo\tools\sunshinesvc.exe"))
    };
    assert!(!our_service(Path::new(
        r"C:\Vibepollo\tools\sunshinesvc.exe"
    )));
    assert!(our_service(Path::new(
        r"C:\Program Files\Rubylight\Butterpollo-Service.EXE"
    )));
    assert_eq!(other_services(theirs), ["ApolloService"]);
    let ours = |name: &str| {
        (name == "ApolloService")
            .then(|| PathBuf::from(r"C:\Program Files\Rubylight\butterpollo-service.exe"))
    };
    assert!(other_services(ours).is_empty());
    // Service names reach only what setup's scan and the uninstaller
    // check: the constant that named it unconditionally is gone.
    for source in [
        include_str!("../install.rs"),
        include_str!("../update.rs"),
        include_str!("../uninstall.rs"),
    ] {
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("\"ApolloService\""));
        assert!(!code.contains("detect::SERVICE"));
    }
}
/// With the record and the service both lost, a quiet run must not take the
/// Apps entry for a normal installation and remove the host it was kept beside.
#[test]
fn the_profiles_mark_keeps_an_installation_side_by_side() {
    let entry = || {
        let mut ours = product("Rubylight", "Butterpollo");
        ours.location = Some(PathBuf::from(r"C:\Program Files\Rubylight"));
        ours
    };
    let lost = Found {
        service: host_service::DEFAULT,
        butterpollo: Some(entry()),
        side_by_side_profile: true,
        ..Default::default()
    };
    assert!(lost.installed_side_by_side());
    assert!(lost.side_by_side(false).unwrap());
    assert!(lost.side_by_side(true).unwrap());
    // Without the mark, the Apps entry alone is a normal installation.
    let unmarked = Found {
        side_by_side_profile: false,
        ..lost
    };
    assert!(!unmarked.side_by_side(false).unwrap());
    assert!(unmarked.side_by_side(true).is_err());
    // Rubylight running as the default service stays what it is, over a
    // profile an earlier side-by-side installation left.
    let normal = Found {
        service: host_service::DEFAULT,
        service_install: Some(PathBuf::from(r"C:\Program Files\Rubylight")),
        butterpollo: Some(entry()),
        side_by_side_profile: true,
        ..Default::default()
    };
    assert!(!normal.installed_side_by_side());
    assert!(!normal.side_by_side(false).unwrap());
    // That profile with nothing installed: a new run still chooses.
    let uninstalled = Found {
        service: host_service::DEFAULT,
        side_by_side_profile: true,
        ..Default::default()
    };
    assert!(!uninstalled.side_by_side(false).unwrap());
    assert!(uninstalled.side_by_side(true).unwrap());
}
#[test]
fn the_steps_that_keep_shared_drivers_read_the_mark_too() {
    assert!(keeps_other_hosts(host_service::SIDE_BY_SIDE, true, false));
    assert!(keeps_other_hosts(host_service::SIDE_BY_SIDE, false, false));
    assert!(keeps_other_hosts(host_service::DEFAULT, false, true));
    assert!(!keeps_other_hosts(host_service::DEFAULT, true, true));
    assert!(!keeps_other_hosts(host_service::DEFAULT, false, false));
}
#[test]
fn a_host_known_only_by_its_service_has_a_folder() {
    let services = |name: &str| match name {
        "ApolloService" => program(r"C:\Other\tools\sunshinesvc.exe"),
        "SunshineService" => program(r"C:\Rubylight\butterpollo-service.exe"),
        _ => None,
    };
    assert_eq!(
        other_service_folders(services),
        [("ApolloService", PathBuf::from(r"C:\Other"))]
    );
}
/// Replace stays the default of existing upgrades. An installation stays
/// what it is, whatever a later run asks.
#[test]
fn the_mode_follows_the_installation_then_the_request() {
    let rubylight = Some(PathBuf::from(r"C:\Program Files\Rubylight"));
    let none = Found::default();
    assert!(!none.side_by_side(false).unwrap());
    // Without another host too, the flag gives the same layout.
    assert!(none.side_by_side(true).unwrap());
    let normal = Found {
        service: host_service::DEFAULT,
        service_install: rubylight.clone(),
        ..Default::default()
    };
    assert!(!normal.side_by_side(false).unwrap());
    let refused = format!("{:#}", normal.side_by_side(true).unwrap_err());
    assert!(refused.contains("Nothing has been changed"), "{refused}");
    assert!(refused.contains("Uninstall Rubylight first"), "{refused}");
    let side = Found {
        service: host_service::SIDE_BY_SIDE,
        service_install: rubylight,
        ..Default::default()
    };
    assert!(side.installed_side_by_side());
    assert!(side.side_by_side(false).unwrap());
    assert!(side.side_by_side(true).unwrap());
    // A record left by a failed setup, with no Rubylight: nothing to keep.
    let stale = Found {
        service: host_service::SIDE_BY_SIDE,
        ..Default::default()
    };
    assert!(!stale.installed_side_by_side());
    assert!(!stale.side_by_side(false).unwrap());
}
#[test]
fn the_choice_names_every_detected_host_and_says_replace_uninstalls() {
    assert!(host_choice(&Found::default()).is_none());
    let found = Found {
        packages: vec![product("Vibepollo", "{0A1B}")],
        vibepollo_entries: vec![product("Vibepollo", "Vibepollo")],
        legacy: vec![product("Apollo", "Apollo")],
        ..Default::default()
    };
    assert_eq!(found.other_hosts(), ["Vibepollo", "Apollo"]);
    let choice = host_choice(&found).unwrap();
    assert_eq!(choice.heading, "Replace Vibepollo and Apollo?");
    assert!(choice.replace.contains("uninstalls"), "{}", choice.replace);
    assert!(choice.keep.contains("alongside"), "{}", choice.keep);
    assert!(
        choice
            .text
            .contains("setup uninstalls Vibepollo and Apollo"),
        "{}",
        choice.text
    );
    assert!(choice.text.contains("new settings"), "{}", choice.text);
    // A service alone counts, and its name is all setup has to show.
    let service = Found {
        other_services: vec!["ApolloService"],
        ..Default::default()
    };
    let choice = host_choice(&service).unwrap();
    assert!(
        choice.heading.contains("ApolloService"),
        "{}",
        choice.heading
    );
}
#[test]
fn side_by_side_promises_no_change_and_no_replacement() {
    let found = Found {
        legacy: vec![product("Sunshine", "Sunshine")],
        ..Default::default()
    };
    let folder = default_folder();
    let replacing = summary(&found, &folder, false, false);
    assert!(replacing.contains("will be uninstalled and replaced"));
    let keeping = summary(&found, &folder, true, false);
    assert!(!keeping.contains("replaced"), "{keeping}");
    assert!(keeping.contains("Sunshine and its drivers are not changed"));
    assert!(keeping.contains("RubylightService"), "{keeping}");
    assert!(keeping.contains("nothing is imported"), "{keeping}");
    assert!(keeping.ends_with(SYSTEM_CHANGES));
    // An update of an installed Rubylight removes no host, and says so.
    let updating = summary(&found, &folder, false, true);
    assert!(!updating.contains("replaced"), "{updating}");
    assert!(updating.contains("does not change Sunshine"), "{updating}");
}
#[test]
fn a_conversion_is_refused_with_the_way_to_do_it() {
    let text = conversion_refusal(&["Vibepollo".to_owned()]);
    assert!(text.contains("To replace Vibepollo"), "{text}");
    assert!(text.contains("uninstall Rubylight"), "{text}");
    assert!(text.contains("Nothing has been changed"), "{text}");
}
#[test]
fn the_summary_names_system_changes_and_the_privacy_statement() {
    let text = summary(&Found::default(), &default_folder(), false, false);
    assert!(text.ends_with(SYSTEM_CHANGES), "{text}");
    assert!(text.contains("service") && text.contains("firewall rule"));
    assert!(text.contains("certificates") && text.contains("removes them"));
    assert!(text.contains("updates") && text.contains("privacy.html"));
}
#[test]
fn a_picked_folder_is_used_only_when_empty_or_already_rubylight() {
    let root = tempfile::tempdir().unwrap();
    let empty = root.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert_eq!(chosen_folder(&empty), empty);
    let missing = root.path().join("new");
    assert_eq!(chosen_folder(&missing), missing);
    let games = root.path().join("games");
    std::fs::create_dir(&games).unwrap();
    std::fs::write(games.join("game.exe"), b"").unwrap();
    assert_eq!(chosen_folder(&games), games.join("Rubylight"));
    let ours = root.path().join("ours");
    std::fs::create_dir(&ours).unwrap();
    std::fs::write(ours.join("manifest.json"), b"[]").unwrap();
    assert_eq!(chosen_folder(&ours), ours);
    let drive = std::path::Path::new("C:\\");
    assert_eq!(chosen_folder(drive), drive.join("Rubylight"));
}
#[test]
fn an_installation_that_cannot_stay_offers_program_files_instead() {
    let root = tempfile::tempdir().unwrap();
    let installed = root.path().join("Games").join("Rubylight");
    std::fs::create_dir_all(&installed).unwrap();
    let mut found = Found {
        service_install: Some(installed.clone()),
        ..Default::default()
    };
    assert_eq!(install_dir(&found, None), installed);
    assert!(!folder_choosable(&found));
    assert!(!summary(&found, &installed, false, false).contains("Rubylight moves out of"));
    found.relocate = Some("Games must be owned by Administrators".into());
    assert_eq!(install_dir(&found, None), default_folder());
    assert!(folder_choosable(&found));
    let text = summary(&found, &default_folder(), false, false);
    assert!(
        text.contains(&format!(
            "Rubylight moves out of {}: its service runs as SYSTEM",
            installed.display()
        )),
        "{text}"
    );
    assert!(
        text.contains("Settings and paired devices are kept."),
        "{text}"
    );
    // Picked again, the folder is refused before setup starts.
    assert!(summary(&found, &installed, false, false).contains("Choose another folder."));
    // A folder chosen on the command line is still used.
    let requested = root.path().join("Requested");
    assert_eq!(install_dir(&found, Some(requested.clone())), requested);
    // Without a service, the Apps entry names the folder.
    let entry = Found {
        butterpollo: Some(Product {
            key: "Butterpollo".into(),
            name: "Rubylight".into(),
            version: "2.1.0".into(),
            location: Some(installed.clone()),
            uninstall: None,
            quiet_uninstall: None,
            msi: false,
            root: HKEY_LOCAL_MACHINE,
        }),
        ..Default::default()
    };
    assert_eq!(entry.current(), Some(installed.as_path()));
    assert_eq!(install_dir(&entry, None), installed);
    assert!(!folder_choosable(&entry));
}
#[test]
fn upgrades_and_reinstalls_are_allowed_but_downgrades_are_not() {
    for (installed, incoming) in [
        ("2.0.0-rc.9", "2.0.0-rc.10"),
        ("2.0.0-rc.21", "2.0.0-rc.21"),
        ("2.0.0-rc.21", "2.0.0"),
        ("v2.0.0-rc.21+build.5", "2.0.0-rc.21"),
        ("", "2.0.0-rc.21"),
        ("unknown", "2.0.0-rc.21"),
    ] {
        assert!(check_version(installed, incoming).is_ok());
    }
    for (installed, incoming) in [
        ("2.0.0-rc.10", "2.0.0-rc.9"),
        ("2.0.0", "2.0.0-rc.21"),
        ("2.1.0", "2.0.0"),
        ("2.0.0-rc.21", "2.0.0-rc.9"),
    ] {
        assert!(check_version(installed, incoming).is_err());
    }
}
#[test]
fn locations_come_from_the_icon_the_uninstaller_or_the_service() {
    let path = |p: &str| Some(PathBuf::from(p));
    let sunshine = path(r"C:\Program Files\Sunshine");
    assert_eq!(
        resolve_location(Some(r" C:\Program Files\Sunshine\ "), None, None),
        sunshine
    );
    for (icon, uninstall) in [
        (Some(r"C:\Program Files\Sunshine\sunshine.exe,0"), None),
        (Some(r#""C:\Program Files\Sunshine\sunshine.exe",0"#), None),
        (Some(r"C:\Program Files\Sunshine\sunshine.ico"), None),
        (
            Some(""),
            Some(r#""C:\Program Files\Sunshine\uninstall.exe" /S"#),
        ),
        (None, Some(r"C:\Program Files\Sunshine\Uninstall.EXE /S")),
        (
            Some(r"C:\Windows\Installer\{0A1B}\ProductIcon.ico"),
            Some(r"C:\Program Files\Sunshine\tools\uninstall.exe"),
        ),
    ] {
        assert_eq!(
            resolve_location(Some("  "), icon, uninstall),
            sunshine,
            "{icon:?} {uninstall:?}"
        );
    }
    // Windows Installer's own programs and relative paths name no folder.
    assert_eq!(
        resolve_location(
            None,
            Some(r"C:\WINDOWS\Installer\{0A1B}\icon.ico,0"),
            Some(r"MsiExec.exe /X{0A1B}")
        ),
        None
    );
    assert_eq!(
        resolve_location(
            None,
            Some("sunshine.exe"),
            Some(r"C:\Windows\System32\msiexec.exe /X{0A1B}")
        ),
        None
    );
    assert_eq!(
        program_folder(Path::new(r"C:\Program Files\Apollo\tools\sunshinesvc.exe")),
        path(r"C:\Program Files\Apollo")
    );
}
#[test]
fn the_summary_promises_settings_only_when_there_are_some() {
    let root = tempfile::tempdir().unwrap();
    let product = Product {
        key: "{0A1B}".into(),
        name: "Apollo".into(),
        version: "0.4.6".into(),
        location: Some(root.path().to_path_buf()),
        uninstall: None,
        quiet_uninstall: None,
        msi: true,
        root: HKEY_LOCAL_MACHINE,
    };
    let found = Found {
        packages: vec![product],
        ..Default::default()
    };
    let install = Path::new(r"C:\Program Files\Butterpollo");
    assert!(summary(&found, install, false, false).contains("Its settings were not found"));
    std::fs::create_dir(root.path().join("config")).unwrap();
    std::fs::write(root.path().join("config").join("sunshine.conf"), "").unwrap();
    assert!(
        summary(&found, install, false, false)
            .contains("Its settings, paired devices, apps and drivers are kept.")
    );
}
