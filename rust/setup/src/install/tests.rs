/// 2.4.2 trusted the root only on a fresh install; an update, the way
/// every existing host got 2.4.2, left the console untrusted.
#[test]
fn every_start_of_the_new_host_trusts_the_console_root() {
    let ready = "Some(env!(\"CARGO_PKG_VERSION\")))?;";
    for (name, source) in [
        ("install.rs", include_str!("../install.rs")),
        ("update.rs", include_str!("../update.rs")),
    ] {
        let code = source.split("#[cfg(test)]").next().unwrap();
        let starts: Vec<_> = code.match_indices(ready).collect();
        assert!(!starts.is_empty(), "{name}");
        for (at, _) in starts {
            let after: String = code[at..].lines().skip(1).take(4).collect();
            assert!(after.contains("trust_console"), "{name}: {after}");
        }
    }
}
#[test]
fn setup_removes_the_root_by_the_name_the_host_gives_it() {
    assert_eq!(
        super::CONSOLE_ROOT_NAME,
        butterpollo_core::crypto::CONSOLE_ROOT_NAME
    );
}
use super::*;
#[test]
fn vibepollos_nvidia_undo_runs_only_with_its_journal_and_program() {
    let base = std::env::temp_dir().join(format!("nvprefs-undo-{}", std::process::id()));
    let (data, previous) = (base.join("data"), base.join("previous"));
    std::fs::create_dir_all(data.join("Sunshine")).unwrap();
    std::fs::create_dir_all(&previous).unwrap();
    assert!(!nvprefs_undo_pending(&data, &previous));
    std::fs::write(data.join("Sunshine\\nvprefs_undo.json"), "{}").unwrap();
    assert!(!nvprefs_undo_pending(&data, &previous));
    std::fs::write(previous.join("sunshine.exe"), "").unwrap();
    assert!(nvprefs_undo_pending(&data, &previous));
    std::fs::remove_file(data.join("Sunshine\\nvprefs_undo.json")).unwrap();
    assert!(!nvprefs_undo_pending(&data, &previous));
    std::fs::remove_dir_all(&base).unwrap();
}
#[test]
fn driver_reboot_exit_codes_are_successful_but_warnings_are_shown() {
    let mut notes = Vec::new();
    assert!(driver_result(3010, "", &mut notes, "display"));
    assert!(notes.is_empty());
    assert!(!driver_result(0, "", &mut notes, "gamepad"));
    assert!(!driver_result(
        0,
        "VIRTUAL_DISPLAY_DRIVER_WARNING",
        &mut notes,
        "display"
    ));
    assert_eq!(notes.len(), 1);
    assert!(!driver_result(1, "failed", &mut notes, "gamepad"));
    assert_eq!(notes.len(), 2);
    // The script's own reason reaches the note.
    driver_result(
        0,
        "WARNING: [SunshineVirtualDisplay] VIRTUAL_DISPLAY_DRIVER_WARNING: Optional virtual display driver setup did not complete.\nWARNING: [SunshineVirtualDisplay] Installer best-effort driver action failed: nefconc.exe failed with exit code 5.\n",
        &mut notes,
        "virtual display",
    );
    assert!(
        notes[2].starts_with(
            "The virtual display driver could not be set up: nefconc.exe failed with exit code 5."
        ),
        "{}",
        notes[2]
    );
    assert!(driver_result(
        1,
        "RESTART_REQUIRED\nUSBIP_DRIVER_WARNING: Restart Windows or repair USBip separately.",
        &mut notes,
        "USB audio and haptics",
    ));
    assert!(notes[3].contains("Run setup again with USB audio and haptics selected"));
    assert!(!notes[3].contains("when its service starts"));
}
#[test]
fn a_driver_script_past_the_deadline_is_not_reported_as_failed() {
    let late = anyhow::Error::from(system::StillRunning {
        program: "powershell.exe".into(),
        seconds: 600,
    });
    let note = driver_error_note(&late, "USB audio and haptics");
    assert!(
        note.starts_with(
            "The USB audio and haptics driver was still being set up after 600 s and goes on in the background."
        ),
        "{note}"
    );
    assert!(!note.contains("could not"), "{note}");
    // The task's own log is removed at the deadline, so its result
    // never reaches the setup log.
    assert!(!note.contains("setup log"), "{note}");
    let late = late.context("waiting for the task");
    assert!(driver_error_note(&late, "virtual gamepad").contains("still being set up"));
    let failed = anyhow::anyhow!("creating the setup task failed (1)");
    assert_eq!(
        driver_error_note(&failed, "virtual gamepad"),
        "The virtual gamepad driver could not be set up: creating the setup task failed (1)"
    );
}
#[test]
fn nsis_uninstallers_run_in_place_so_setup_waits_for_them() -> Result<()> {
    let product = |uninstall: Option<&str>, quiet: Option<&str>| detect::Product {
        key: "Apollo".into(),
        name: "Apollo".into(),
        version: "0.4.6".into(),
        location: None,
        uninstall: uninstall.map(Into::into),
        quiet_uninstall: quiet.map(Into::into),
        msi: false,
        root: HKEY_LOCAL_MACHINE,
    };
    let apollo = product(Some(r#""C:\Program Files\Apollo\Uninstall.exe""#), None);
    assert_eq!(
        legacy_uninstall(&apollo, |_| true),
        Some((
            r#""C:\Program Files\Apollo\Uninstall.exe" /S _?=C:\Program Files\Apollo"#.into(),
            Some(PathBuf::from(r"C:\Program Files\Apollo\Uninstall.exe"))
        ))
    );
    // Other uninstallers, and a command that already runs in place, are
    // run as registered.
    assert_eq!(
        legacy_uninstall(&apollo, |_| false),
        Some((r#""C:\Program Files\Apollo\Uninstall.exe" /S"#.into(), None))
    );
    let quiet = product(None, Some(r"C:\Apollo\Uninstall.exe /S _?=C:\Apollo"));
    assert_eq!(
        legacy_uninstall(&quiet, |_| true),
        Some((r"C:\Apollo\Uninstall.exe /S _?=C:\Apollo".into(), None))
    );
    assert_eq!(legacy_uninstall(&product(None, None), |_| true), None);

    let folder = tempfile::tempdir()?;
    let nsis = folder.path().join("Uninstall.exe");
    std::fs::write(
        &nsis,
        [&b"MZ"[..], &[0; 4096], b"\xef\xbe\xad\xdeNullsoftInst"].concat(),
    )?;
    let other = folder.path().join("unins000.exe");
    std::fs::write(&other, [&b"MZ"[..], &[0; 4096], b"Inno Setup"].concat())?;
    assert!(is_nsis(&nsis));
    assert!(!is_nsis(&other));
    assert!(!is_nsis(&folder.path().join("missing.exe")));
    Ok(())
}
#[test]
fn a_pc_that_refuses_the_locally_signed_driver_gets_a_note_naming_why() {
    let mut notes = Vec::new();
    let output = "WARNUNG: [RubylightVirtualDisplay] DRIVER_UNSUPPORTED: This PC refuses a display driver signed on the PC (Smart App Control is on). A signed Rubylight release installs it; until then streams use a physical display.\n";
    assert!(!driver_result(0, output, &mut notes, "virtual display"));
    assert_eq!(
        notes,
        [
            "The virtual display driver was not installed: This PC refuses a display driver signed on the PC (Smart App Control is on). A signed Rubylight release installs it; until then streams use a physical display."
        ]
    );
}
#[test]
fn libvirtualdisplay_goes_only_once_rubylights_device_runs() {
    assert!(libvirtualdisplay_replaced(false, false, false));
    // Smart App Control, S mode, App Control or a failed install: the
    // driver step added a note, and the old driver stays.
    let mut notes = Vec::new();
    let refused = "WARNING: [RubylightVirtualDisplay] DRIVER_UNSUPPORTED: Smart App Control is on, and it refuses a display driver signed on this PC, so streams use a physical display.\n";
    let restart = driver_result(0, refused, &mut notes, "virtual display");
    assert!(!libvirtualdisplay_replaced(
        false,
        !notes.is_empty(),
        restart
    ));
    let mut notes = Vec::new();
    let failed = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: Windows did not start the display driver (Error, problem 31)\n";
    let restart = driver_result(0, failed, &mut notes, "virtual display");
    assert!(!libvirtualdisplay_replaced(
        false,
        !notes.is_empty(),
        restart
    ));
    // Installed, but the device starts only after a restart.
    let mut notes = Vec::new();
    let restart = driver_result(3010, "RESTART_REQUIRED\n", &mut notes, "virtual display");
    assert!(!libvirtualdisplay_replaced(
        false,
        !notes.is_empty(),
        restart
    ));
}
/// A kept host may still use SudoVDA: its driver and HDR layer stay
/// through every run, even with Rubylight's own device up and running.
#[test]
fn side_by_side_never_replaces_libvirtualdisplay() {
    assert!(!libvirtualdisplay_replaced(true, false, false));
    let display_step = include_str!("../install.rs")
        .split("fn display_step(")
        .nth(1)
        .and_then(|rest| rest.split("/// The installer of").next())
        .unwrap();
    assert!(display_step.contains("detect::side_by_side()"));
}
#[test]
fn replacing_stays_as_it_was_and_side_by_side_touches_only_rubylight() {
    let replacing = takeover(false);
    assert_eq!(replacing.stop, OLD_SERVICES);
    assert_eq!(replacing.end, HOST_PROCESSES);
    assert!(replacing.remove_hosts);
    assert_eq!(
        replacing.firewall_rules,
        ["Butterpollo", "Vibepollo", "Vibepollo Service", "Apollo"]
    );
    let keeping = takeover(true);
    assert_eq!(keeping.stop, [host_service::SIDE_BY_SIDE]);
    assert_eq!(keeping.end, OWN_PROCESSES);
    assert!(!keeping.remove_hosts);
    assert_eq!(keeping.firewall_rules, ["Butterpollo"]);
    for service in OLD_SERVICES {
        assert!(!keeping.stop.contains(&service), "{service}");
    }
    assert_eq!(HOST_PROCESSES[..4], OWN_PROCESSES);
    for other in &HOST_PROCESSES[4..] {
        assert!(!keeping.end.contains(other), "{other}");
    }
}
/// Every removal of a host goes through the plan: its steps stay gated by
/// `remove_hosts`, the import by the side-by-side branch.
#[test]
fn a_full_installation_gates_each_removal_on_the_plan() {
    // A Windows checkout may have CRLF line ends.
    let source = include_str!("../install.rs").replace("\r\n", "\n");
    let code = source
        .split("fn install_with(")
        .nth(1)
        .and_then(|rest| rest.split("/// Uninstall the hosts `found` lists.").next())
        .unwrap();
    for gated in [
        "if plan.remove_hosts {\n        restart_needed |= remove_hosts(",
        "if plan.remove_hosts {\n        // Remaining old services",
        "if plan.remove_hosts {\n        let _ = std::fs::remove_dir_all(",
        "if plan.remove_hosts {\n        crate::uninstall::remove_stale_layers();",
        "for rule in plan.firewall_rules {",
        "system::kill(plan.end);",
        "        plan.stop,\n",
    ] {
        assert!(code.contains(gated), "{gated}");
    }
    for ungated in [
        "        &OLD_SERVICES,\n",
        "system::kill(&HOST_PROCESSES)",
        "for rule in [",
    ] {
        assert!(!code.contains(ungated), "{ungated}");
    }
    assert!(code.contains(
        "let previous = if side_by_side {\n        None\n    } else {\n        migration_source("
    ));
    // Recorded before the service in both: an update heals a lost record,
    // and a new installation's driver steps read it.
    assert_eq!(
        code.matches("detect::record_service(side_by_side)?;")
            .count(),
        2
    );
    // Before anything is stopped or written, on every path but an update
    // in place: a move returns early, and took the folder unchecked.
    let at = |text: &str| code.find(text).unwrap_or_else(|| panic!("{text}"));
    let guard = at("outside_other_hosts(&found, &install)?;");
    assert!(guard < at("if let Target::Move(old) = &target {"));
    assert!(guard > at("let updating = target == Target::InPlace;"));
    assert!(at("refuse_foreign_service(") < at("prepare_profile(&profile"));
}
#[test]
fn side_by_side_stays_out_of_a_folder_a_host_knows_only_by_its_service() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let host = dir.path().join("host");
    std::fs::create_dir(&host)?;
    let found = detect::Found {
        other_service_folders: vec![("ApolloService", host.clone())],
        ..Default::default()
    };
    for inside in [host.clone(), host.join("Rubylight")] {
        let refused = format!("{:#}", outside_other_hosts(&found, &inside).unwrap_err());
        assert!(refused.contains("ApolloService"), "{refused}");
        assert!(refused.contains("Nothing has been changed"), "{refused}");
    }
    outside_other_hosts(&found, &dir.path().join("Rubylight"))?;
    // A service whose folder is gone holds nothing to overwrite.
    let gone = detect::Found {
        other_service_folders: vec![("ApolloService", dir.path().join("gone"))],
        ..Default::default()
    };
    outside_other_hosts(&gone, &dir.path().join("gone"))?;
    Ok(())
}
#[test]
fn another_programs_rubylight_service_is_refused() {
    refuse_foreign_service(None).unwrap();
    refuse_foreign_service(Some(Path::new(
        r"C:\Program Files\Rubylight\butterpollo-service.exe",
    )))
    .unwrap();
    let refused = format!(
        "{:#}",
        refuse_foreign_service(Some(Path::new(r"C:\Other\other.exe"))).unwrap_err()
    );
    assert!(refused.contains("RubylightService"), "{refused}");
    assert!(refused.contains(r"C:\Other\other.exe"), "{refused}");
    assert!(refused.contains("Nothing has been changed"), "{refused}");
}
/// Setup's rollback must not start or stop the other host: it only ever
/// acts on Rubylight's own service, whatever else runs.
#[test]
fn a_failed_side_by_side_setup_leaves_the_other_hosts_services_alone() -> Result<()> {
    let mut stopped = Vec::new();
    let mut started = Vec::new();
    let restart = stop_running(
        takeover(true).stop,
        |_| true,
        |service| {
            stopped.push(service.to_owned());
            Ok(())
        },
        |service: &str| {
            started.push(service.to_owned());
            Ok(())
        },
    )?;
    drop(restart);
    assert_eq!(stopped, ["RubylightService"]);
    assert_eq!(started, ["RubylightService"]);
    Ok(())
}
#[test]
fn the_range_checked_covers_every_port_the_host_derives() {
    use butterpollo_core::config::{Config, Ports};
    for base in SIDE_BY_SIDE_PORTS {
        let range = ports_of(base);
        let ports = Ports::from_base(base);
        for port in [
            ports.http,
            ports.https,
            ports.web,
            ports.guest,
            ports.video,
            ports.control,
            ports.audio,
            ports.mic,
            ports.rtsp,
        ] {
            assert!(range.contains(&port), "{port} for {base}");
        }
        let config = Config::parse(&side_by_side_conf(base, None)).unwrap();
        assert_eq!(config.port().unwrap(), base);
        assert!(range.contains(&config.webrtc_port()));
        // The guests' media ports follow the media port (MAX_GUESTS, 4).
        assert!(range.contains(&(config.webrtc_media_port() + 4)));
        // Clear of the default range other hosts use, and of the ephemeral one.
        assert!(*range.start() > 48010 + 5, "{base}");
        assert!(*range.end() < 49152, "{base}");
    }
    for pair in SIDE_BY_SIDE_PORTS.windows(2) {
        assert!(ports_of(pair[0]).end() < ports_of(pair[1]).start());
    }
}
#[test]
fn the_base_port_needs_its_whole_range_free_with_a_fallback_then_an_error() {
    assert_eq!(pick_base_port(|_| true).unwrap(), SIDE_BY_SIDE_PORTS[0]);
    // Any one port of a range rules out the base.
    for taken in [48584, 48589, 48590, 48610, 48619, 48624] {
        assert_eq!(
            pick_base_port(|port| port != taken).unwrap(),
            SIDE_BY_SIDE_PORTS[1],
            "{taken}"
        );
    }
    let error = format!("{:#}", pick_base_port(|_| false).unwrap_err());
    assert!(error.contains("no free ports"), "{error}");
    assert!(error.contains("Nothing has been changed"), "{error}");
}
#[test]
fn a_port_a_program_holds_is_not_free() -> Result<()> {
    let tcp = std::net::TcpListener::bind(("0.0.0.0", 0))?;
    assert!(!port_free(tcp.local_addr()?.port()));
    let udp = std::net::UdpSocket::bind(("0.0.0.0", 0))?;
    assert!(!port_free(udp.local_addr()?.port()));
    // A port just let go can be taken at once by another test or program (CI flaked on it),
    // so a free one is looked for among a few released ones.
    let mut free = false;
    for _ in 0..20 {
        let port = {
            let probe = std::net::TcpListener::bind(("0.0.0.0", 0))?;
            probe.local_addr()?.port()
        };
        if port_free(port) {
            free = true;
            break;
        }
    }
    assert!(free, "no released port was free");
    Ok(())
}
/// Two hosts with one PC's name would collide on mDNS and look the same
/// in a client's list.
#[test]
fn the_new_profile_has_its_own_ports_and_name() {
    use butterpollo_core::config::Config;
    let conf = side_by_side_conf(48589, Some("GAMING-PC"));
    let config = Config::parse(&conf).unwrap();
    assert_eq!(config.get("sunshine_name", ""), "GAMING-PC (Rubylight)");
    assert_eq!(config.webrtc_port(), 48619);
    assert_eq!(config.webrtc_media_port(), 48620);
    let unnamed = Config::parse(&side_by_side_conf(48589, Some(" "))).unwrap();
    assert_eq!(unnamed.get("sunshine_name", ""), "Rubylight");
}
#[test]
fn a_side_by_side_profile_is_new_and_never_another_hosts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("config");
    let mut moved = None;
    let free = |_: u16| true;
    // Nothing there: a new profile on the first free base port.
    assert!(prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        free,
        &mut moved
    )?);
    assert_eq!(web_port(&profile), 48590);
    assert_eq!(probe(&profile).port(), 48589);
    assert!(moved.is_none());
    // Left by an earlier side-by-side installation: kept, settings and all.
    std::fs::write(profile.join("sunshine_state.json"), b"paired")?;
    assert!(!prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        free,
        &mut moved
    )?);
    assert_eq!(
        std::fs::read(profile.join("sunshine_state.json"))?,
        b"paired"
    );
    // Its port is taken now: said, not changed.
    let error = prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        |port| port != 48589,
        &mut moved,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("base port 48589"),
        "{error:#}"
    );
    // Left by a normal installation, whose identity may be imported from
    // another host: moved aside unchanged, and the new one starts fresh.
    std::fs::remove_file(profile.join(SIDE_BY_SIDE_MARK))?;
    std::fs::write(profile.join("sunshine.conf"), "port = 47989\n")?;
    assert!(prepare_side_by_side_profile(
        &profile,
        false,
        Some("PC"),
        free,
        &mut moved
    )?);
    assert_eq!(web_port(&profile), 48590);
    assert!(!profile.join("sunshine_state.json").exists());
    let aside = moved.take().context("the earlier profile moved aside")?;
    assert!(aside.parent() == profile.parent());
    assert_eq!(std::fs::read(aside.join("sunshine_state.json"))?, b"paired");
    assert_eq!(
        std::fs::read_to_string(aside.join("sunshine.conf"))?,
        "port = 47989\n"
    );
    assert!(earlier_settings_moved(&aside).contains(&aside.display().to_string()));
    // No free ports: refused before the earlier settings moved.
    std::fs::remove_file(profile.join(SIDE_BY_SIDE_MARK))?;
    assert!(prepare_side_by_side_profile(&profile, false, None, |_| false, &mut moved).is_err());
    assert!(moved.is_none());
    assert!(profile.join("sunshine.conf").is_file());
    // An installed side-by-side Rubylight keeps its profile as it is.
    std::fs::write(profile.join("sunshine.conf"), "port = 50000\n")?;
    assert!(!prepare_side_by_side_profile(
        &profile,
        true,
        None,
        |_| false,
        &mut moved
    )?);
    assert_eq!(web_port(&profile), 50001);
    Ok(())
}
/// A failed first side-by-side setup leaves no profile to take for
/// Rubylight's own on the next try.
#[test]
fn a_failed_first_install_removes_the_profile_it_made() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("config");
    let new = prepare_side_by_side_profile(&profile, false, None, |_| true, &mut None)?;
    assert!(new);
    let failed = undo_import_on_error(&profile, new, || bail!("no space on the disk"));
    assert!(failed.is_err());
    assert!(!profile.exists());
    Ok(())
}
#[test]
fn the_result_gives_the_address_with_its_port() {
    let text = side_by_side_result(48590, "192.168.4.12", &["Vibepollo".to_owned()]);
    assert!(text.contains("beside Vibepollo"), "{text}");
    assert!(text.contains("RubylightService"), "{text}");
    assert!(text.contains("add this PC as 192.168.4.12:48589"), "{text}");
    let alone = side_by_side_result(48590, "PC", &[]);
    assert!(alone.contains("Rubylight runs as the"), "{alone}");
}
/// The script reloads the device, and Vibepollo's family shares it.
#[test]
fn the_gamepad_device_tool_says_whether_its_device_answers() {
    let answered = r#"{"root_device_count":1,"phantom_root_devices":0,"source_interface_ready":true,"source_interface_state":"answered","instance_id":"ROOT\\VIBESHINEVIRTUALGAMEPAD\\0000","device_status":25174027,"problem_code":0}"#;
    assert_eq!(gamepad_device_ready(answered), Some(true));
    // #86: the package stayed in the Driver Store, its device went with Vibepollo.
    let missing = "note on stderr\r\n{\"root_device_count\":0,\"phantom_root_devices\":0,\"source_interface_ready\":false,\"source_interface_state\":\"absent\"}\r\n";
    assert_eq!(gamepad_device_ready(missing), Some(false));
    assert_eq!(
        gamepad_device_ready("Usage:\n  VibeshineVhfGamepadDeviceSetup.exe status"),
        None
    );
}

#[test]
fn a_side_by_side_host_reuses_the_shared_gamepad_device_or_installs_one() {
    let device = |text: &str| {
        let text = text.to_owned();
        move || -> Result<Vec<String>> { Ok(vec![text.clone()]) }
    };
    let vhf = r"ROOT\VIBESHINEVIRTUALGAMEPAD\0000";
    assert!(matches!(
        gamepad_step(false, || panic!("a normal install does not look")),
        GamepadStep::Install
    ));
    assert!(matches!(
        gamepad_step(true, device(vhf)),
        GamepadStep::Reuse
    ));
    assert!(matches!(
        gamepad_step(true, device(r"root\vibeshinevirtualgamepad\0001")),
        GamepadStep::Reuse
    ));
    assert!(matches!(
        gamepad_step(true, device(r"ROOT\RUBYLIGHTVDISPLAY\0000")),
        GamepadStep::Install
    ));
    assert!(matches!(
        gamepad_step(true, || bail!("denied")),
        GamepadStep::Unknown(_)
    ));
    // The prefix the bundled scripts own their device by.
    assert_eq!(GAMEPAD_DEVICE_PREFIX, "ROOT\\VIBESHINEVIRTUALGAMEPAD\\");
}
#[test]
fn a_failed_libvirtualdisplay_removal_is_a_note_about_removing_it() {
    use crate::libvirtualdisplay::Removal;
    let failed = Removal {
        restart: false,
        error: Some(anyhow::anyhow!(
            "pnputil /delete-driver oem7.inf /uninstall /force failed with exit code 5"
        )),
    };
    assert_eq!(
        libvirtualdisplay_note(&failed).as_deref(),
        Some(
            "The old libvirtualdisplay driver could not be removed: pnputil /delete-driver oem7.inf /uninstall /force failed with exit code 5 Setup tries again when it next runs; the setup log has the details."
        )
    );
    // A display still in use goes at the next restart, without a note.
    let later = Removal {
        restart: true,
        error: None,
    };
    assert_eq!(libvirtualdisplay_note(&later), None);
}
#[test]
fn a_failed_rubylight_display_install_becomes_a_setup_note() {
    let mut notes = Vec::new();
    let output = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: pnputil /add-driver failed with exit code 5\n";
    assert!(!driver_result(0, output, &mut notes, "Rubylight display"));
    assert_eq!(
        notes,
        [
            "The Rubylight display driver could not be set up: pnputil /add-driver failed with exit code 5 The Rubylight service tries again within a day, or run \"butterpollo-setup.exe --repair-drivers --now\" in an administrator shell; the setup log has the details."
        ]
    );
    let mut notes = Vec::new();
    assert!(driver_result(
        0,
        "RESTART_REQUIRED\n",
        &mut notes,
        "Rubylight display"
    ));
    assert!(notes.is_empty());
}
#[test]
fn the_display_driver_is_repaired_once_a_day_per_version_unless_declined() {
    let day = 86_400;
    let record = |text: &str| Some(text.as_bytes().to_vec());
    let skipped =
        |text: Option<Vec<u8>>, now| repair_skipped(text.as_deref(), "2.0.0-rc.27", now, false);
    // A person running the repair (--now) is never held back by an
    // automatic attempt that failed earlier the same day, nor by a
    // declined choice. The default, which the service runs, is limited.
    for text in [
        r#"{"version":"2.0.0-rc.27","unix":86400}"#,
        r#"{"declined":true}"#,
    ] {
        assert_eq!(
            repair_skipped(Some(text.as_bytes()), "2.0.0-rc.27", day + 60, true),
            None
        );
    }
    assert_eq!(skipped(None, day), None);
    assert_eq!(skipped(record("not json"), day), None);
    let tried = record(r#"{"version":"2.0.0-rc.27","unix":86400}"#);
    assert!(skipped(tried.clone(), day + 60).is_some());
    assert_eq!(skipped(tried, 2 * day), None);
    assert_eq!(
        skipped(
            record(r#"{"version":"2.0.0-rc.26","unix":86400}"#),
            day + 60
        ),
        None
    );
    assert!(skipped(record(r#"{"declined":true}"#), day).is_some());
}
#[test]
fn the_unattended_repair_needs_a_positive_idle_answer() {
    let idle = |counts: [&str; 3]| {
        Ok(format!(
            "<RustHostSessionCount>{}</RustHostSessionCount><RustHostPendingSessionCount>{}</RustHostPendingSessionCount><RustHostApplicationActive>{}</RustHostApplicationActive>",
            counts[0], counts[1], counts[2]
        ))
    };
    let refused = |answer: std::io::Result<String>, running| {
        ensure_idle_positive(&answer, || running, false).is_err()
    };
    assert!(!refused(idle(["0", "0", "0"]), true));
    // A stream, a queued launch or a running app.
    assert!(refused(idle(["1", "0", "0"]), true));
    assert!(refused(idle(["0", "1", "0"]), true));
    assert!(refused(idle(["0", "0", "1"]), true));
    // A LAN caller sees blank counts, an old host none: it may stream.
    assert!(refused(idle(["", "", ""]), false));
    assert!(refused(idle(["0", "0", ""]), false));
    assert!(refused(Ok("<root/>".into()), false));
    assert!(refused(Ok(String::new()), false));
    // No answer: busy while a host process runs, idle when none does.
    let silent = || Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
    assert!(refused(silent(), true));
    assert!(!refused(silent(), false));
    // --now refuses the same streams and a host that runs but is silent,
    // but takes blank counts (a LAN bind_address blanks them for this
    // PC too), which would otherwise leave that host unrepairable.
    let by_hand = |answer: std::io::Result<String>, running| {
        ensure_idle_positive(&answer, || running, true).is_err()
    };
    assert!(by_hand(idle(["1", "0", "0"]), true));
    assert!(by_hand(idle(["0", "0", "1"]), true));
    assert!(by_hand(silent(), true));
    assert!(!by_hand(silent(), false));
    assert!(!by_hand(idle(["0", "0", "0"]), true));
    assert!(!by_hand(idle(["", "", ""]), true));
}
#[test]
fn a_repair_that_finds_the_driver_lock_taken_does_not_report_success() {
    // The service's run waits for the service to be restarted and asks
    // again; a person is told nothing was repaired.
    assert!(!when_locked(false).unwrap());
    let error = when_locked(true).unwrap_err().to_string();
    assert_eq!(
        error,
        "Setup is installing the drivers right now; try again when it finishes."
    );
}
#[test]
fn only_the_display_drivers_own_notes_fail_the_repair() {
    let notes = [
        "The virtual display driver could not be set up: pnputil failed".to_owned(),
        "The old libvirtualdisplay driver could not be removed: busy".to_owned(),
    ];
    // The display step added the first one: that is the failure.
    let error = display_failed(&notes, 1).unwrap_err().to_string();
    assert!(error.contains("could not be set up"), "{error}");
    assert!(!error.contains("libvirtualdisplay"), "{error}");
    // Only the removal warned: the driver is repaired, the note stays.
    assert!(display_failed(&notes[1..], 0).is_ok());
    assert!(display_failed(&[], 0).is_ok());
}
#[test]
fn setup_refuses_streams_pending_connections_and_apps() {
    for counts in [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]] {
        let response = format!(
            "<RustHostSessionCount>{}</RustHostSessionCount><RustHostPendingSessionCount>{}</RustHostPendingSessionCount><RustHostApplicationActive>{}</RustHostApplicationActive>",
            counts[0], counts[1], counts[2]
        );
        assert_eq!(check_idle(&response).is_ok(), counts == [0, 0, 0]);
    }
    // Older hosts, and a Rust host asked on its LAN bind_address, cannot
    // say; they are stopped as before rather than blocking setup forever.
    assert!(check_idle("<state>SUNSHINE_SERVER_FREE</state>").is_ok());
    assert!(check_idle("<RustHostVersion>2.0.0-rc.21</RustHostVersion><RustHostSessionCount></RustHostSessionCount><RustHostPendingSessionCount></RustHostPendingSessionCount><RustHostApplicationActive></RustHostApplicationActive>").is_ok());
    assert!(check_idle("").is_ok());
}
#[test]
fn a_stopped_or_unresponsive_host_does_not_block_setup() -> Result<()> {
    // Windows refuses a closed loopback port only after about two seconds.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
    ensure_idle(closed, false)?;
    let silent = std::net::TcpListener::bind("127.0.0.1:0")?;
    ensure_idle(silent.local_addr()?, false)?;
    // A busy host is still refused over a real connection, unless the
    // user chose to end its stream.
    let busy = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = busy.local_addr()?;
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        use std::io::{Read, Write};
        for _ in 0..2 {
            let (mut socket, _) = busy.accept()?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte)?;
                request.push(byte[0]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n<root><RustHostSessionCount>1</RustHostSessionCount></root>")?;
        }
        Ok(())
    });
    assert!(ensure_idle(address, false).is_err());
    ensure_idle(address, true)?;
    server.join().unwrap()?;
    Ok(())
}
#[test]
fn upgrades_repair_legacy_profile_ownership_and_preserve_settings() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    std::fs::create_dir_all(profile.join("credentials"))?;
    let settings = profile.join("sunshine.conf");
    std::fs::write(&settings, b"port=48123\n")?;
    let (_, identity) = system::run(
        &system::system32("whoami.exe"),
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(10),
    )?;
    let sid = identity
        .trim()
        .split(',')
        .nth(1)
        .context("whoami omitted SID")?
        .trim_matches('"');
    for path in [&root, &settings] {
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &path.display().to_string(),
                "/setowner",
                &format!("*{sid}"),
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &path.display().to_string(),
                "/grant:r",
                "*S-1-1-0:(F)",
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
    }
    assert_eq!(prepare_profile(&profile, true)?, None);
    system::trusted_ancestors(&root)?;
    system::trusted_folder(&settings)?;
    assert_eq!(std::fs::read(&settings)?, b"port=48123\n");
    let credentials = profile.join("credentials");
    for (path, sid) in [
        (&root, "*S-1-1-0"),
        (&settings, "*S-1-1-0"),
        (&credentials, "*S-1-5-32-545"),
    ] {
        let (_, output) = system::run(
            &system::system32("icacls.exe"),
            &[&path.display().to_string(), "/findsid", sid, "/Q"],
            Duration::from_secs(10),
        )?;
        assert!(!output.contains(&path.display().to_string()), "{output}");
    }
    Ok(())
}
#[test]
fn credentials_are_locked_again_when_resetting_the_profile_fails() -> Result<()> {
    // The reset makes the credentials inherit the profile's read access
    // for users; a file it cannot reset must not leave them that way.
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("Butterpollo").join("config");
    let mut restricted = Vec::new();
    let error = secure_profile_with(
        &profile,
        |folder, users_read| {
            restricted.push((folder.to_owned(), users_read));
            Ok(())
        },
        |_| bail!("resetting permissions failed (5)"),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("resetting permissions failed"));
    assert_eq!(
        restricted,
        [
            (temp.path().join("Butterpollo"), true),
            (profile.join("credentials"), false)
        ]
    );
    Ok(())
}
#[test]
fn legacy_profile_repair_rejects_reparse_points_and_untrusted_parents() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    let settings = outside.join("sunshine.conf");
    std::fs::write(&settings, b"unchanged")?;
    let linked = temp.path().join("linked");
    std::os::windows::fs::symlink_dir(&outside, &linked)?;
    for profile in [linked.join("config"), linked.join("Butterpollo/config")] {
        let error = prepare_profile(&profile, true).unwrap_err();
        assert!(error.to_string().contains("reparse point"), "{error:#}");
    }
    let profile = temp.path().join("Butterpollo/config");
    std::fs::create_dir_all(&profile)?;
    std::os::windows::fs::symlink_dir(&outside, profile.join("credentials"))?;
    let error = prepare_profile(&profile, true).unwrap_err();
    assert!(error.to_string().contains("reparse point"), "{error:#}");
    assert_eq!(std::fs::read(&settings)?, b"unchanged");
    assert!(!outside.join("config").exists());
    assert!(!outside.join("Butterpollo").exists());
    let untrusted = temp.path().join("untrusted");
    std::fs::create_dir(&untrusted)?;
    // Not an administrator's: an administrator's account may own a parent.
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[
            &untrusted.display().to_string(),
            "/setowner",
            "*S-1-5-32-545",
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    for updating in [true, false] {
        let error = prepare_profile(&untrusted.join("Butterpollo/config"), updating).unwrap_err();
        assert!(error.to_string().contains("must be owned by"), "{error:#}");
        assert!(!untrusted.join("Butterpollo").exists());
    }
    Ok(())
}
/// `icacls` on `path` with `args`, which must succeed.
fn icacls(path: &Path, args: &[&str]) -> Result<()> {
    let path = path.display().to_string();
    let args: Vec<&str> = [path.as_str()]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &args,
        Duration::from_secs(10),
    )?;
    anyhow::ensure!(code == 0, "icacls {args:?}: {output}");
    Ok(())
}
/// Whether `path`'s permissions name `sid`.
fn names(path: &Path, sid: &str) -> Result<bool> {
    let (_, output) = system::run(
        &system::system32("icacls.exe"),
        &[&path.display().to_string(), "/findsid", sid, "/Q"],
        Duration::from_secs(10),
    )?;
    Ok(output.contains(&path.display().to_string()))
}
/// `path`'s permissions as icacls lists them.
fn acl(path: &Path) -> Result<String> {
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[&path.display().to_string()],
        Duration::from_secs(10),
    )?;
    anyhow::ensure!(code == 0, "{output}");
    // icacls aligns its lines with the name.
    let output = output.replace(&path.display().to_string(), "");
    Ok(output.split_whitespace().collect::<Vec<_>>().join(" "))
}
/// A user's Nucleus copy below `root`, in folders with the permissions
/// the host gives them (`user_copy.rs`), Users standing in for the user:
/// they may write in the copy, and own what they put there. With
/// `link_to`, that includes a junction to it.
fn nucleus_copy(root: &Path, link_to: Option<&Path>) -> Result<PathBuf> {
    let nucleus = nucleus_user_root(root);
    // The host's `ROOT`.
    system::create_restricted(&nucleus, false)?;
    let folder = nucleus.join("S-1-5-21-1");
    let copy = folder.join("0123456789abcdef");
    std::fs::create_dir(&folder)?;
    std::fs::create_dir(&copy)?;
    for (path, user) in [
        (&folder, "*S-1-5-32-545:(RX)"),
        (&copy, "*S-1-5-32-545:(OI)(CI)(M)"),
    ] {
        icacls(
            path,
            &[
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)(F)",
                "*S-1-5-32-544:(OI)(CI)(F)",
                user,
            ],
        )?;
    }
    // Named like what setup resets in the profile: a file icacls would
    // also look for in every folder below, and a folder.
    std::fs::write(copy.join("handler.js"), b"user's")?;
    std::fs::create_dir(copy.join("config"))?;
    icacls(&copy.join("handler.js"), &["/setowner", "*S-1-5-32-545"])?;
    icacls(&copy.join("config"), &["/setowner", "*S-1-5-32-545"])?;
    if let Some(target) = link_to {
        let (code, output) = system::run(
            &system::system32("cmd.exe"),
            &[
                "/c",
                "mklink",
                "/J",
                &copy.join("linked").display().to_string(),
                &target.display().to_string(),
            ],
            Duration::from_secs(10),
        )?;
        anyhow::ensure!(code == 0, "mklink: {output}");
    }
    Ok(copy)
}
/// The copy and its folders still have the host's permissions: setup
/// changed none of them.
fn assert_nucleus_copy_kept(root: &Path, copy: &Path) -> Result<()> {
    let nucleus = nucleus_user_root(root);
    // Users may not even list the folder of everyone's copies.
    assert!(!names(&nucleus, "*S-1-5-32-545")?);
    assert!(names(copy, "*S-1-5-32-545")?);
    for theirs in [copy.join("handler.js"), copy.join("config")] {
        let error = system::trusted_folder(&theirs).unwrap_err();
        assert!(
            error.to_string().contains("must be owned by"),
            "{}: {error:#}",
            theirs.display()
        );
    }
    assert_eq!(std::fs::read(copy.join("handler.js"))?, b"user's");
    Ok(())
}
#[test]
fn reinstalls_leave_the_users_nucleus_copies_as_the_host_made_them() -> Result<()> {
    // A reinstall reset every Nucleus copy to the profile's permissions,
    // so the host could no longer write the user's copy as the user, and
    // a junction the user made in theirs failed every reinstall.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    system::create_restricted(&root, true)?;
    std::fs::create_dir_all(profile.join("credentials"))?;
    std::fs::write(root.join("handler.js"), b"setup's")?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("sunshine.conf"), b"unchanged")?;
    let copy = nucleus_copy(&root, Some(&outside))?;
    let nucleus = nucleus_user_root(&root);
    let linked = [outside.clone(), outside.join("sunshine.conf")];
    let acls = || {
        linked
            .iter()
            .map(|path| acl(path))
            .collect::<Result<Vec<_>>>()
    };
    let before = acls()?;
    // An update, then a reinstall's or a full installation's steps.
    for installed in [true, false] {
        assert_eq!(prepare_profile(&profile, installed)?, None);
    }
    secure_profile(&profile)?;
    assert_nucleus_copy_kept(&root, &copy)?;
    system::trusted_tree(&root, std::slice::from_ref(&nucleus))?;
    assert_eq!(std::fs::read(outside.join("sunshine.conf"))?, b"unchanged");
    assert_eq!(acls()?, before);
    // The copies' folder itself is still checked: a link in its place
    // is refused, and setup leaves it as it is.
    std::fs::remove_dir_all(&nucleus)?;
    std::os::windows::fs::symlink_dir(&outside, &nucleus)?;
    for error in [
        secure_profile(&profile).unwrap_err(),
        prepare_profile(&profile, true).unwrap_err(),
    ] {
        assert!(error.to_string().contains("reparse point"), "{error:#}");
    }
    assert!(
        std::fs::symlink_metadata(&nucleus)?
            .file_type()
            .is_symlink()
    );
    assert_eq!(acls()?, before);
    std::fs::remove_dir(&nucleus)?;
    // So is one users may change: the repair leaves it, and fails.
    std::fs::create_dir(&nucleus)?;
    icacls(&nucleus, &["/grant", "*S-1-5-32-545:(M)"])?;
    let error = format!("{:#}", prepare_profile(&profile, true).unwrap_err());
    for said in [
        format!("an administrator can delete {}", nucleus.display()),
        format!("{} may be changed by", nucleus.display()),
    ] {
        assert!(error.contains(&said), "{error}");
    }
    assert!(names(&nucleus, "*S-1-5-32-545")?);
    Ok(())
}
#[test]
fn a_legacy_repair_that_failed_partway_is_finished_by_the_next_run() -> Result<()> {
    // The repair ran only while the root had another owner: once it got
    // past the root, what it had not reached yet stayed the user's.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    std::fs::create_dir_all(profile.join("credentials"))?;
    let settings = profile.join("sunshine.conf");
    std::fs::write(&settings, b"port=48123\n")?;
    let copy = nucleus_copy(&root, None)?;
    let nucleus = nucleus_user_root(&root);
    // As earlier installers left it.
    for path in [&root, &settings] {
        icacls(path, &["/setowner", "*S-1-5-32-545"])?;
    }
    icacls(&settings, &["/grant", "*S-1-5-32-545:(M)"])?;
    let error = secure_profile_with(
        &profile,
        |folder, users_read| {
            system::restrict_except(folder, users_read, std::slice::from_ref(&nucleus))
        },
        |_| bail!("resetting permissions failed (5)"),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("resetting permissions failed"));
    // The root is the service's now, its settings still the user's.
    system::trusted_parents(&profile)?;
    assert!(system::trusted_tree(&root, std::slice::from_ref(&nucleus)).is_err());
    // Every later run repairs until all of it is the service's, leaving
    // the Nucleus copies as they are.
    for _ in 0..2 {
        assert_eq!(prepare_profile(&profile, true)?, None);
        system::trusted_tree(&root, std::slice::from_ref(&nucleus))?;
        assert_nucleus_copy_kept(&root, &copy)?;
    }
    assert_eq!(std::fs::read(&settings)?, b"port=48123\n");
    Ok(())
}
#[test]
fn an_installed_profile_gone_after_setup_looked_is_an_error() -> Result<()> {
    // The repair found nothing to check and went on; the update then
    // made the profile with the permissions ProgramData gives everyone.
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("Butterpollo");
    let error = repair_profile(&root.join("config")).unwrap_err();
    assert!(
        format!("{error:#}").starts_with(&format!("checking {}", root.display())),
        "{error:#}"
    );
    assert!(!root.exists());
    Ok(())
}
#[test]
fn fresh_installs_move_an_untrusted_profile_aside_unchanged() -> Result<()> {
    // #61: a ProgramData\Butterpollo that the user owned, with no
    // Rubylight installed, stopped every fresh install.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let root = temp.path().join("Butterpollo");
    let profile = root.join("config");
    std::fs::create_dir_all(&profile)?;
    std::fs::write(profile.join("sunshine.conf"), b"planted")?;
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[
            &root.display().to_string(),
            "/setowner",
            "*S-1-5-32-545",
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    let aside = prepare_profile(&profile, false)?.context("the profile stayed")?;
    // A new root, locked down and empty, replaces it.
    system::trusted_folder(&root)?;
    assert_eq!(std::fs::read_dir(&root)?.count(), 0);
    assert_eq!(aside.parent(), Some(temp.path()));
    assert!(
        aside
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("Butterpollo.untrusted-")),
        "{}",
        aside.display()
    );
    assert_eq!(
        std::fs::read(aside.join("config/sunshine.conf"))?,
        b"planted"
    );
    assert!(system::trusted_folder(&aside).is_err());
    // A trusted profile stays where it is.
    std::fs::create_dir_all(&profile)?;
    assert_eq!(prepare_profile(&profile, false)?, None);
    assert!(profile.is_dir());
    // So is a trusted root holding something someone else owns: its owner
    // could change its permissions back.
    let (code, output) = system::run(
        &system::system32("icacls.exe"),
        &[
            &profile.display().to_string(),
            "/setowner",
            "*S-1-5-32-545",
            "/Q",
        ],
        Duration::from_secs(10),
    )?;
    assert_eq!(code, 0, "{output}");
    let child_aside = prepare_profile(&profile, false)?.context("the child stayed")?;
    // A second move, usually within the same second, gets its own name.
    assert_ne!(child_aside, aside);
    assert!(child_aside.join("config").is_dir());
    assert_eq!(std::fs::read_dir(&root)?.count(), 0);
    // A missing root is made locked down from the start, also for an
    // installed service whose profile was deleted.
    for installed in [false, true] {
        std::fs::remove_dir_all(&root)?;
        assert_eq!(prepare_profile(&profile, installed)?, None);
        system::trusted_tree(&root, &[])?;
    }
    // Something users may change or that they own moves aside too.
    let icacls = |args: &[&str]| -> Result<()> {
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            args,
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
        Ok(())
    };
    let profile_text = profile.display().to_string();
    for change in [
        ["/grant", "*S-1-5-32-545:(OI)(CI)(M)"],
        ["/setowner", "*S-1-5-32-545"],
    ] {
        std::fs::create_dir(&profile)?;
        icacls(&[&profile_text, change[0], change[1], "/Q"])?;
        let error = system::trusted_tree(&root, &[]).unwrap_err();
        assert!(
            error.downcast_ref::<system::Untrusted>().is_some(),
            "{error:#}"
        );
        assert!(prepare_profile(&profile, false)?.is_some());
        assert!(!profile.exists());
    }
    // The host lets users write in their Nucleus copies and checks them
    // itself: a profile holding one stays, unless the copies' folder
    // itself is open to users.
    let nucleus = nucleus_user_root(&root);
    assert_eq!(
        nucleus,
        butterpollo_core::nucleus::install::user_root(&root)
    );
    let copy = nucleus.join("S-1-5-21-1").join("0123456789abcdef");
    std::fs::create_dir_all(&copy)?;
    std::fs::write(copy.join("handler.js"), b"user's")?;
    let copy_text = copy.display().to_string();
    icacls(&[&copy_text, "/grant", "*S-1-5-32-545:(OI)(CI)(M)", "/Q"])?;
    icacls(&[&copy_text, "/setowner", "*S-1-5-32-545", "/T", "/Q"])?;
    assert_eq!(prepare_profile(&profile, false)?, None);
    assert_eq!(std::fs::read(copy.join("handler.js"))?, b"user's");
    let nucleus_text = nucleus.display().to_string();
    icacls(&[&nucleus_text, "/grant", "*S-1-5-32-545:(M)", "/Q"])?;
    assert!(prepare_profile(&profile, false)?.is_some());
    assert!(!nucleus.exists());
    Ok(())
}
#[test]
fn folders_created_restricted_match_restricted_ones() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    for users_read in [true, false] {
        let created = temp.path().join(format!("created-{users_read}"));
        let restricted = temp.path().join(format!("restricted-{users_read}"));
        system::create_restricted(&created, users_read)?;
        assert!(system::create_restricted(&created, users_read).is_err());
        std::fs::create_dir(&restricted)?;
        system::restrict(&restricted, users_read)?;
        assert_eq!(acl(&created)?, acl(&restricted)?);
        system::trusted_tree(&created, &[])?;
    }
    Ok(())
}
#[test]
fn fresh_installs_move_a_linked_profile_aside_not_its_target() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("sunshine.conf"), b"unchanged")?;
    let linked = temp.path().join("linked");
    std::os::windows::fs::symlink_dir(&outside, &linked)?;
    let aside = prepare_profile(&linked.join("config"), false)?.context("the link stayed")?;
    assert!(std::fs::symlink_metadata(&aside)?.file_type().is_symlink());
    assert!(!std::fs::symlink_metadata(&linked)?.file_type().is_symlink());
    system::trusted_folder(&linked)?;
    assert_eq!(std::fs::read(outside.join("sunshine.conf"))?, b"unchanged");
    assert_eq!(std::fs::read_dir(&outside)?.count(), 1);
    Ok(())
}
#[test]
fn a_damaged_installation_is_repaired_by_the_full_installer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let installed = temp.path().join("Butterpollo");
    std::fs::create_dir(&installed)?;
    let service = |folder: &Path| detect::Found {
        service_install: Some(folder.to_owned()),
        ..Default::default()
    };
    assert_eq!(
        target(&detect::Found::default(), &installed)?,
        Target::Fresh
    );
    // A deleted folder or a missing or broken manifest leaves nothing to
    // roll back to, so these do not block a reinstall.
    let deleted = temp.path().join("deleted");
    assert_eq!(target(&service(&deleted), &deleted)?, Target::Fresh);
    assert_eq!(target(&service(&installed), &installed)?, Target::Fresh);
    std::fs::write(installed.join("manifest.json"), "{")?;
    assert_eq!(target(&service(&installed), &installed)?, Target::Fresh);
    std::fs::write(installed.join("manifest.json"), "[]")?;
    assert_eq!(target(&service(&installed), &installed)?, Target::InPlace);
    // Another folder would leave the service without its backup.
    let other = temp.path().join("other");
    assert!(target(&service(&installed), &other).is_err());
    std::fs::create_dir(&other)?;
    assert!(target(&service(&installed), &other).is_err());
    Ok(())
}
#[test]
fn an_installation_that_cannot_stay_moves_to_another_folder() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let installed = temp.path().join("Games/Rubylight");
    std::fs::create_dir_all(&installed)?;
    std::fs::write(installed.join("manifest.json"), "[]")?;
    let found = detect::Found {
        service_install: Some(installed.clone()),
        relocate: Some("Games must be owned by Administrators".into()),
        ..Default::default()
    };
    let program_files = temp.path().join("Program Files/Rubylight");
    assert_eq!(
        target(&found, &program_files)?,
        Target::Move(installed.clone())
    );
    std::fs::create_dir_all(&program_files)?;
    assert_eq!(
        target(&found, &program_files)?,
        Target::Move(installed.clone())
    );
    // Kept in its folder it is updated there, where prepare_install
    // refuses it.
    assert_eq!(target(&found, &installed)?, Target::InPlace);
    Ok(())
}
#[test]
fn a_failed_move_points_the_service_back_at_the_previous_folder() {
    let steps = std::cell::RefCell::new(Vec::new());
    let step = |name: &'static str, ok: bool| {
        let steps = &steps;
        move || {
            steps.borrow_mut().push(name);
            if ok { Ok(()) } else { bail!("{name} failed") }
        }
    };
    assert!(switch_service(step("new", true), step("start", true), step("old", true)).is_ok());
    assert_eq!(steps.take(), ["new", "start"]);
    // The service does not start from the new folder: back to the old
    // one, after it was started there.
    let failure =
        switch_service(step("new", true), step("start", false), step("old", true)).unwrap_err();
    assert_eq!(steps.take(), ["new", "start", "old"]);
    assert!(failure.error.to_string().contains("start failed"));
    assert!(failure.back.is_ok());
    let failure =
        switch_service(step("new", true), step("start", false), step("old", false)).unwrap_err();
    assert_eq!(steps.take(), ["new", "start", "old"]);
    assert!(failure.back.is_err());
    // Not pointed at the new folder: nothing to undo or start there.
    let failure =
        switch_service(step("new", false), step("start", true), step("old", true)).unwrap_err();
    assert_eq!(steps.take(), ["new"]);
    assert!(failure.error.to_string().contains("new failed"));
    assert!(failure.back.is_ok());
}
/// Makes the user running the tests own `path`, with full access for
/// everyone, as a folder that user created and earlier setups locked
/// down but left to that user.
fn give_to_user(path: &Path) -> Result<()> {
    let (_, identity) = system::run(
        &system::system32("whoami.exe"),
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(10),
    )?;
    let sid = identity
        .trim()
        .split(',')
        .nth(1)
        .context("whoami omitted SID")?
        .trim_matches('"');
    give_to(path, sid)
}
/// As [`give_to_user`], for the Users group: not an administrator, so
/// not trusted with a folder above the installation either.
fn give_to_users(path: &Path) -> Result<()> {
    give_to(path, "S-1-5-32-545")
}
fn give_to(path: &Path, sid: &str) -> Result<()> {
    let owner = format!("*{sid}");
    for arguments in [["/setowner", owner.as_str()], ["/grant:r", "*S-1-1-0:(F)"]] {
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &path.display().to_string(),
                arguments[0],
                arguments[1],
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
    }
    Ok(())
}
#[test]
fn updates_repair_a_user_owned_install_folder_only_under_trusted_parents() -> Result<()> {
    // Taking ownership for SYSTEM and Administrators needs an administrator.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    // D:\Rubylight, created by the user and locked down by 2.2.1.
    let install = temp.path().join("Rubylight");
    std::fs::create_dir_all(install.join("assets"))?;
    let host = install.join("butterpollo-service.exe");
    std::fs::write(&host, b"service")?;
    give_to_user(&install)?;
    give_to_user(&host)?;
    let error = system::trusted_ancestors(&install).unwrap_err();
    assert!(error.to_string().contains("must be owned by"), "{error:#}");
    assert!(unrepairable(&install).is_none());
    prepare_install(&install)?;
    system::trusted_ancestors(&install)?;
    system::trusted_folder(&host)?;
    assert_eq!(std::fs::read(&host)?, b"service");
    for path in [&install, &host] {
        let (_, output) = system::run(
            &system::system32("icacls.exe"),
            &[&path.display().to_string(), "/findsid", "*S-1-1-0", "/Q"],
            Duration::from_secs(10),
        )?;
        assert!(!output.contains(&path.display().to_string()), "{output}");
    }
    // D:\Games\Rubylight: whoever owns D:\Games can put another folder
    // in its place, so nothing is changed and the error says to move.
    let games = temp.path().join("Games");
    let nested = games.join("Rubylight");
    std::fs::create_dir_all(&nested)?;
    give_to_users(&games)?;
    give_to_user(&nested)?;
    assert!(unrepairable(&nested).is_some());
    let error = prepare_install(&nested).unwrap_err().to_string();
    assert!(
        error.contains(&format!("cannot be updated in {}", nested.display())),
        "{error}"
    );
    assert!(error.contains("must be owned by"), "{error}");
    assert!(
        error.contains(&format!(
            "moves Rubylight to {}",
            detect::default_folder().display()
        )),
        "{error}"
    );
    assert!(system::trusted_folder(&nested).is_err());
    // Nor is a folder with a link inside repaired.
    let linked = temp.path().join("Linked");
    std::fs::create_dir(&linked)?;
    give_to_user(&linked)?;
    std::os::windows::fs::symlink_dir(&games, linked.join("assets"))?;
    assert!(unrepairable(&linked).is_some());
    assert!(prepare_install(&linked).is_err());
    assert!(system::trusted_folder(&games).is_err());
    Ok(())
}
#[test]
fn a_previous_host_is_removed_only_after_its_profile_can_be_imported() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let old = temp.path().join("Vibepollo");
    let profile = temp.path().join("Butterpollo/config");
    let install = temp.path().join("programs/Butterpollo");
    std::fs::create_dir_all(old.join("config"))?;
    std::fs::write(
        old.join("config/sunshine.conf"),
        "encoder=amdvce_experimental",
    )?;
    let product = detect::Product {
        key: "Vibepollo".into(),
        name: "Vibepollo".into(),
        version: "2.0.0".into(),
        location: Some(old.clone()),
        uninstall: None,
        quiet_uninstall: None,
        msi: false,
        root: HKEY_LOCAL_MACHINE,
    };
    let mut found = detect::Found {
        legacy: vec![product.clone()],
        ..Default::default()
    };
    let source = system::win32_path(&old.canonicalize()?)?;
    assert_eq!(migration_source(&found, &profile, &install)?, Some(source));
    assert!(migration_source(&found, &profile, &old).is_err());
    assert!(migration_source(&found, &profile, &old.join("nested")).is_err());
    std::fs::create_dir_all(&profile)?;
    std::fs::write(profile.join("sunshine_state.json"), "existing pairings")?;
    assert!(migration_source(&found, &profile, &install).is_err());
    assert_eq!(
        std::fs::read(profile.join("sunshine_state.json"))?,
        b"existing pairings"
    );
    std::fs::remove_file(profile.join("sunshine_state.json"))?;
    let other = temp.path().join("Sunshine");
    std::fs::create_dir(&other)?;
    std::fs::write(other.join("sunshine.conf"), "encoder=software")?;
    found.legacy.push(detect::Product {
        location: Some(other),
        ..product.clone()
    });
    assert!(migration_source(&found, &profile, &install).is_err());
    found.legacy.pop();
    // The same host listed twice (its MSI and its own entry) is one source.
    found.vibepollo_entries.push(product.clone());
    assert!(migration_source(&found, &profile, &install)?.is_some());
    found.vibepollo_entries.clear();
    // Hosts with nothing to import are replaced as the summary says, and
    // stale entries without a folder do not block setup.
    for location in [None, Some(temp.path().join("deleted"))] {
        found.legacy.push(detect::Product {
            location,
            ..product.clone()
        });
    }
    std::fs::remove_file(old.join("config/sunshine.conf"))?;
    assert_eq!(migration_source(&found, &profile, &install)?, None);
    // Even then Rubylight is not installed where a host is removed.
    assert!(migration_source(&found, &profile, &old.join("nested")).is_err());
    std::fs::write(profile.join("sunshine_state.json"), "existing pairings")?;
    assert_eq!(migration_source(&found, &profile, &install)?, None);
    assert_eq!(
        migration_source(&detect::Found::default(), &profile, &install)?,
        None
    );
    Ok(())
}
#[test]
fn an_install_that_fails_after_the_import_can_be_run_again() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir()?;
    let old = temp.path().join("Vibepollo");
    let profile = temp.path().join("Butterpollo/config");
    let install = temp.path().join("programs/Butterpollo");
    std::fs::create_dir_all(old.join("config"))?;
    std::fs::write(old.join("config/sunshine.conf"), "encoder=amdvce")?;
    let found = detect::Found {
        legacy: vec![detect::Product {
            key: "Vibepollo".into(),
            name: "Vibepollo".into(),
            version: "2.0.0".into(),
            location: Some(old.clone()),
            uninstall: None,
            quiet_uninstall: None,
            msi: false,
            root: HKEY_LOCAL_MACHINE,
        }],
        ..Default::default()
    };
    let import = || -> Result<()> {
        std::fs::create_dir_all(profile.join("credentials"))?;
        std::fs::write(profile.join("sunshine.conf"), "encoder=amdvce")?;
        std::fs::write(profile.join("credentials/cakey.pem"), "key")?;
        Ok(())
    };
    assert!(migration_source(&found, &profile, &install)?.is_some());
    // The file copy after the import fails: the error is kept and the
    // import is gone, so the retry imports again.
    let error = undo_import_on_error(&profile, true, || {
        import()?;
        bail!("disk full")
    })
    .unwrap_err();
    assert_eq!(format!("{error:#}"), "disk full");
    assert!(!profile.exists());
    assert!(migration_source(&found, &profile, &install)?.is_some());
    // A successful run keeps it.
    undo_import_on_error(&profile, true, import)?;
    assert!(profile.join("sunshine.conf").is_file());
    // Without an import nothing is removed.
    assert!(undo_import_on_error(&profile, false, || bail!("disk full")).is_err());
    assert!(profile.join("sunshine.conf").is_file());
    // An import that cannot be removed says so and where.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(profile.join("credentials/cakey.pem"))?;
    let error = undo_import_on_error(&profile, true, || bail!("disk full")).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.starts_with("disk full"), "{message}");
    assert!(
        message.contains(&profile.display().to_string()),
        "{message}"
    );
    drop(held);
    Ok(())
}
#[test]
fn legacy_migrations_keep_complete_profiles_and_own_configured_files_and_covers() -> Result<()> {
    use crate::profile_tests::{assert_usable, package, rc23_profile, snapshot};
    use butterpollo_core::{config::Config, migration, state};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    let temp = tempfile::tempdir()?;
    let template = temp.path().join("rc.23/config");
    rc23_profile(&template)?;
    let staged = temp.path().join("staged");
    let entries = package(&staged, env!("CARGO_PKG_VERSION"), "new.dll")?;
    for family in ["Vibepollo", "Apollo", "Sunshine"] {
        for external in [false, true] {
            let case = temp.path().join(format!("{family}-{external}"));
            let old = case.join(family);
            let source = if family == "Sunshine" {
                old.clone()
            } else {
                old.join("config")
            };
            let profile = case.join("Butterpollo/config");
            let install = case.join("programs/Butterpollo");
            copy_tree(&template, &source)?;
            let mut apps = state::load_json(&source.join("apps.json"), Value::Null)?;
            apps["apps"][1]["image-path"] = json!(source.join("covers/custom/Épopée.png"));
            state::write_json(&source.join("apps.json"), &apps)?;
            let mut config = Config::load(&source.join("sunshine.conf"))?;
            config.values.insert(
                "log_path".into(),
                source.join("butterpollo.log").display().to_string(),
            );
            let configured = [
                ("file_state", "sunshine_state.json"),
                ("file_apps", "apps.json"),
                ("vibeshine_file_state", "vibeshine_state.json"),
                ("credentials_file", "sunshine_credentials.json"),
                ("cert", "credentials/cacert.pem"),
                ("pkey", "credentials/cakey.pem"),
            ];
            if external {
                let shared = old.join("shared files");
                for (key, name) in configured {
                    let target = shared.join(name);
                    replace_file(&source.join(name), &target)?;
                    config
                        .values
                        .insert(key.into(), target.display().to_string());
                }
                let cover = shared.join("Épopée.png");
                replace_file(&source.join("covers/custom/Épopée.png"), &cover)?;
                apps["apps"][1]["image-path"] = json!(cover);
                state::write_json(&shared.join("apps.json"), &apps)?;
            } else if family == "Sunshine" {
                // Sunshine can keep the web password in the paired-state document.
                std::fs::remove_file(source.join("sunshine_credentials.json"))?;
            }
            std::fs::write(source.join("sunshine.conf"), config.text())?;
            let source_before = snapshot(&old)?;
            let mut expected = snapshot(&source)?;
            expected.retain(|name, _| !name.starts_with("logs"));
            for (key, name) in configured {
                if external {
                    expected.insert(name.into(), std::fs::read(config.path(key, &source, name))?);
                    config.values.insert(key.into(), name.into());
                }
            }
            config
                .values
                .insert("log_path".into(), "butterpollo.log".into());
            expected.insert("sunshine.conf".into(), config.text().into_bytes());
            let product = detect::Product {
                key: family.into(),
                name: family.into(),
                version: "2.0.0".into(),
                location: Some(old.clone()),
                uninstall: None,
                quiet_uninstall: None,
                msi: false,
                root: HKEY_LOCAL_MACHINE,
            };
            let found = detect::Found {
                legacy: vec![product],
                ..Default::default()
            };
            let selected = migration_source(&found, &profile, &install)?.unwrap();
            assert_eq!(selected, system::win32_path(&old.canonicalize()?)?);
            let selected_profile = if selected.join("config/sunshine.conf").is_file() {
                selected.join("config")
            } else {
                selected
            };
            // --import-config in the staged host calls this exact importer.
            migration::import(&selected_profile, &profile)?;
            copy_package(&staged, &install, &entries)?;
            for app in apps["apps"].as_array_mut().unwrap() {
                let original = source.join(app["image-path"].as_str().unwrap());
                let bytes = std::fs::read(original)?;
                let name =
                    PathBuf::from("covers").join(format!("{:x}.png", Sha256::digest(&bytes)));
                app["image-path"] = json!(profile.canonicalize()?.join(&name));
                expected.insert(name, bytes);
            }
            expected.insert("apps.json".into(), serde_json::to_vec_pretty(&apps)?);
            assert_eq!(
                snapshot(&profile)?,
                expected,
                "{family}, external={external}"
            );
            assert_eq!(snapshot(&old)?, source_before);
            assert_usable(&profile)?;
            assert!(migration_source(&found, &profile, &install).is_err());
            assert!(migration::import(&source, &profile).is_err());
            assert_eq!(snapshot(&profile)?, expected);
            assert_eq!(snapshot(&old)?, source_before);
            assert!(old.canonicalize()?.starts_with(temp.path().canonicalize()?));
            std::fs::remove_dir_all(&old)?;
            assert_usable(&profile)?;
            for app in apps["apps"].as_array().unwrap() {
                let cover = PathBuf::from(app["image-path"].as_str().unwrap());
                let relative = cover.strip_prefix(profile.canonicalize()?)?;
                assert_eq!(std::fs::read(&cover)?, expected[relative]);
            }
            assert_eq!(snapshot(&profile)?, expected);
        }
    }
    Ok(())
}
#[test]
fn the_health_check_asks_the_configured_bind_address() {
    let local = IpAddr::from([127, 0, 0, 1]);
    for (conf, expected) in [
        ("", local),
        ("port = 48000\n", local),
        ("bind_address =\n", local),
        ("bind_address = 0.0.0.0\n", local),
        ("bind_address = ::\n", local),
        ("bind_address = Ethernet\n", local),
        ("# bind_address = 192.168.1.50\n", local),
        (
            "bind_address = 192.168.1.50\n",
            IpAddr::from([192, 168, 1, 50]),
        ),
        (
            "bind_address = \"192.168.1.50\" # LAN only\r\n",
            IpAddr::from([192, 168, 1, 50]),
        ),
        (
            "bind_address =\nbind_address = 10.0.0.2\nbind_address = 10.0.0.3\n",
            IpAddr::from([10, 0, 0, 2]),
        ),
        ("bind_address = fd00::5\n", "fd00::5".parse().unwrap()),
    ] {
        assert_eq!(probe_address(conf), expected, "{conf:?}");
    }
    let profile = tempfile::tempdir().unwrap();
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(
        "port = 48000\r\nbind_address = 192.168.1.50\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    std::fs::write(profile.path().join("sunshine.conf"), utf16).unwrap();
    assert_eq!(
        probe(profile.path()),
        SocketAddr::from(([192, 168, 1, 50], 48000))
    );
    std::fs::write(
        profile.path().join("sunshine.conf"),
        b"\xef\xbb\xbfport = 48000\n",
    )
    .unwrap();
    assert_eq!(
        probe(profile.path()),
        SocketAddr::from(([127, 0, 0, 1], 48000))
    );
    // The probe reaches a host listening only on that address.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let host = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 512];
        let length = socket.read(&mut request).unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\n\r\n<RustHostVersion>9.9</RustHostVersion>")
            .unwrap();
        String::from_utf8_lossy(&request[..length]).into_owned()
    });
    wait_ready(address, Some("9.9")).unwrap();
    assert!(host.join().unwrap().contains("Host: 127.0.0.1\r\n"));
}
#[test]
fn the_import_error_and_its_causes_are_shown() {
    let output = "2026-10-07T10:00:00Z  WARN butterpollo_core::migration: profile file not imported file=C:\\old\\dump.bin reason=\"it is larger than 64 MiB\"\n\
Error: the settings imported from C:\\old would keep Rubylight from starting\n\
\n\
Caused by:\n    0: invalid JSON in C:\\ProgramData\\Butterpollo\\.butterpollo-import-1\\vibeshine_state.json\n    1: expected value at line 1 column 1\n";
    assert_eq!(
        import_error(output).unwrap(),
        "the settings imported from C:\\old would keep Rubylight from starting\n\n\
Caused by:\n    0: invalid JSON in C:\\ProgramData\\Butterpollo\\.butterpollo-import-1\\vibeshine_state.json\n    1: expected value at line 1 column 1"
    );
    assert_eq!(
        import_error("Error: no sunshine.conf\n").unwrap(),
        "no sunshine.conf"
    );
    assert!(import_error("thread 'main' panicked\n").is_none());
    assert!(import_error("").is_none());
}
#[test]
fn a_failed_stop_aborts_installation_and_restarts_the_previous_services() {
    let mut stopped = Vec::new();
    let mut started = Vec::new();
    let result = stop_running(
        &OLD_SERVICES,
        |_| true,
        |service| {
            stopped.push(service.to_owned());
            if service == "SunshineService" {
                bail!("the service did not stop");
            }
            Ok(())
        },
        |service: &str| {
            started.push(service.to_owned());
            Ok(())
        },
    );
    assert!(result.is_err());
    drop(result);
    assert_eq!(stopped, ["ApolloService", "SunshineService"]);
    assert_eq!(started, stopped);
}
#[test]
fn only_previously_running_services_restart_on_failure() -> Result<()> {
    let mut started = Vec::new();
    let restart = stop_running(
        &OLD_SERVICES,
        |service| service == "ApolloService",
        |_| Ok(()),
        |service: &str| {
            started.push(service.to_owned());
            Ok(())
        },
    )?;
    drop(restart);
    assert_eq!(started, ["ApolloService"]);
    let mut restart = Restart {
        services: vec!["ApolloService"],
        start: |_: &str| panic!("successful installation must not restart twice"),
    };
    restart.services.clear();
    drop(restart);
    Ok(())
}
/// Setup staged the package in ProgramData\Butterpollo\setup and ran the
/// staged host from there elevated; Defender's cloud model quarantined
/// the staged service. It stages in the installation folder now, which
/// it secures first, and leaves nothing behind.
#[test]
fn the_package_is_staged_in_the_installation_folder() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let install = temp.path().join("Rubylight");
    let created = create_install(&install)?;
    assert!(created);
    let staging = prepare_staging(&install, created)?;
    assert_eq!(staging.path(), install.join(".staging").join("package"));
    assert!(
        !staging
            .path()
            .starts_with(system::program_data().join("Butterpollo"))
    );
    system::trusted_tree(&staging_root(&install), &[])?;
    std::fs::write(staging.path().join("butterpollo.exe"), b"staged")?;
    drop(staging);
    // The folder this run created goes with the staging folder.
    assert!(!install.exists());

    std::fs::create_dir(&install)?;
    std::fs::write(install.join("own.txt"), b"the user's")?;
    drop(prepare_staging(&install, true)?);
    assert!(!staging_root(&install).exists());
    assert_eq!(std::fs::read(install.join("own.txt"))?, b"the user's");
    Ok(())
}
#[test]
fn an_interrupted_setups_staging_is_replaced_and_update_backups_kept() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let install = temp.path().join("Rubylight");
    create_install(&install)?;
    let root = staging_root(&install);
    std::fs::create_dir_all(root.join("package"))?;
    std::fs::write(root.join("package").join("stale.exe"), b"old")?;
    let backup = root.join("transaction-1-2").join("previous");
    std::fs::create_dir_all(&backup)?;
    std::fs::write(backup.join("butterpollo.exe"), b"previous")?;
    let staging = prepare_staging(&install, false)?;
    assert!(!staging.path().join("stale.exe").exists());
    system::trusted_tree(&root, &[])?;
    drop(staging);
    assert_eq!(std::fs::read(backup.join("butterpollo.exe"))?, b"previous");
    Ok(())
}
#[test]
fn a_link_in_the_staging_folder_is_refused_not_followed() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let install = temp.path().join("Rubylight");
    create_install(&install)?;
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(outside.join("package"))?;
    std::fs::write(outside.join("file.dll"), b"unchanged")?;
    std::fs::write(outside.join("package").join("file.dll"), b"unchanged")?;
    let root = staging_root(&install);
    for link in [root.clone(), root.join("package")] {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::os::windows::fs::symlink_dir(&outside, &link)?;
        let error = prepare_staging(&install, false)
            .err()
            .context("a link was followed")?;
        assert!(format!("{error:#}").contains("reparse point"), "{error:#}");
        for file in [
            outside.join("file.dll"),
            outside.join("package").join("file.dll"),
        ] {
            assert_eq!(std::fs::read(file)?, b"unchanged");
        }
        std::fs::remove_dir(&link)?;
    }
    Ok(())
}
#[test]
fn the_earlier_staging_folder_in_program_data_is_removed() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let setup = temp.path().join("Butterpollo").join("setup");
    let profile = temp.path().join("Butterpollo").join("config");
    std::fs::create_dir_all(setup.join("drivers"))?;
    std::fs::create_dir_all(&profile)?;
    std::fs::write(setup.join("butterpollo-service.exe"), b"staged")?;
    std::fs::write(profile.join("sunshine.conf"), b"kept")?;
    remove_legacy_staging(temp.path())?;
    assert!(!setup.exists());
    assert_eq!(std::fs::read(profile.join("sunshine.conf"))?, b"kept");
    // Nothing to remove is no error.
    remove_legacy_staging(temp.path())?;
    if system::elevated() {
        // A link there goes itself; what it leads to stays.
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("file"), b"unchanged")?;
        std::os::windows::fs::symlink_dir(&outside, &setup)?;
        remove_legacy_staging(temp.path())?;
        assert!(std::fs::symlink_metadata(&setup).is_err());
        assert_eq!(std::fs::read(outside.join("file"))?, b"unchanged");
    }
    Ok(())
}
