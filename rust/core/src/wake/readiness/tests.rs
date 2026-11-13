use super::*;

fn this_pc() -> Vec<Adapter> {
    serde_json::from_str(include_str!("../fixtures/adapters_this_pc.json")).unwrap()
}
fn report(name: &str) -> NicReport {
    NicReport::parse(match name {
        "de" => include_str!("../fixtures/nics_read_de.json"),
        "en" => include_str!("../fixtures/nics_read_en.json"),
        _ => unreachable!("{name}"),
    })
    .unwrap()
}
fn state(checks: &[Check], id: &str) -> Option<CheckState> {
    checks.iter().find(|c| c.id == id).map(|c| c.state)
}
const S3: SystemFacts = SystemFacts {
    s3: true,
    modern_standby: false,
    hibernate: false,
    hiberboot: Some(true),
};

#[test]
fn this_pcs_realtek_is_ready_in_any_language() {
    let adapters = this_pc();
    for language in ["de", "en"] {
        let report = report(language);
        let local = Some("192.168.4.20".parse().unwrap());
        let (primary, targets) = targets(&adapters, Ok(&report), false, local);
        assert_eq!(primary.as_deref(), Some("02:7C:16:B4:13:4F"), "{language}");
        // The Realtek and the disconnected WLAN; never vEthernet or Wi-Fi Direct.
        assert_eq!(targets.len(), 2, "{language}");
        let realtek = &targets[0];
        assert_eq!(realtek.kind, AdapterKind::Ethernet);
        assert!(realtek.route && realtek.connected);
        assert_eq!(realtek.broadcasts, ["192.168.4.255"]);
        assert_eq!(realtek.state, TargetState::Ready, "{language}");
        assert_eq!(
            state(&realtek.checks, check::WAKE_ENABLED),
            Some(CheckState::Ok)
        );
        assert_eq!(
            state(&realtek.checks, check::MAGIC_PACKET),
            Some(CheckState::Ok)
        );
        assert_eq!(
            state(&realtek.checks, check::MAGIC_PACKET_ONLY),
            Some(CheckState::Warning)
        );
        assert_eq!(
            state(&realtek.checks, check::WAKE_ON_PATTERN),
            Some(CheckState::Ok)
        );
        assert_eq!(
            state(&realtek.checks, check::SHUTDOWN_WAKE),
            Some(CheckState::Ok)
        );
        // Not on a Modern Standby PC.
        assert_eq!(
            state(&realtek.checks, check::MODERN_STANDBY_MAGIC_PACKET),
            None
        );
        let wlan = &targets[1];
        assert_eq!(wlan.kind, AdapterKind::Wifi);
        assert_eq!(wlan.state, TargetState::Experimental);
        assert!(!wlan.route && !wlan.connected);
    }
}

#[test]
fn the_english_and_german_reports_differ_only_in_localized_text() {
    let (de, en) = (report("de"), report("en"));
    assert_eq!(de.adapters.len(), en.adapters.len());
    let localized = |r: &NicReport| -> Vec<_> {
        r.adapters
            .iter()
            .map(|a| (a.name.clone(), a.errors.clone()))
            .collect()
    };
    assert_ne!(localized(&de), localized(&en));
    for (de, en) in de.adapters.iter().zip(&en.adapters) {
        assert_eq!(
            (de.guid.as_str(), &de.keywords, de.wake_enable),
            (en.guid.as_str(), &en.keywords, en.wake_enable)
        );
        assert_eq!(de.magic_packet_only, en.magic_packet_only);
    }
}

#[test]
fn settings_that_are_off_need_setup_and_unreadable_ones_are_unknown() {
    let adapters = this_pc();
    let realtek = &adapters[0];
    let mut nic = report("en").get(realtek).unwrap().clone();
    nic.wake_enable = Some(false);
    let (state, checks) = judge(realtek, Ok(&nic), false);
    assert_eq!(state, TargetState::NeedsSetup);
    assert_eq!(state_of(&checks, check::WAKE_ENABLED), CheckState::Error);
    nic.wake_enable = None;
    assert_eq!(judge(realtek, Ok(&nic), false).0, TargetState::Unknown);
    nic.wake_enable = Some(true);
    nic.keywords
        .insert(MAGIC_PACKET.to_ascii_uppercase(), "0".into());
    assert_eq!(judge(realtek, Ok(&nic), false).0, TargetState::NeedsSetup);
    // Modern Standby needs its own keyword when the adapter has it.
    nic.keywords
        .insert(MAGIC_PACKET.to_ascii_uppercase(), "1".into());
    nic.keywords
        .insert(MODERN_STANDBY_MAGIC_PACKET.to_ascii_uppercase(), "0".into());
    assert_eq!(judge(realtek, Ok(&nic), false).0, TargetState::Ready);
    assert_eq!(judge(realtek, Ok(&nic), true).0, TargetState::NeedsSetup);
    // A refused reading: every check unknown, never ready.
    let (state, checks) = judge(realtek, Err("WMI refused access"), false);
    assert_eq!(state, TargetState::Unknown);
    assert!(checks.iter().all(|c| c.state == CheckState::Unknown));
    let (primary, targets) = targets(&adapters, Err("blocked"), false, None);
    assert_eq!(primary, None);
    assert!(targets.iter().all(|t| t.state != TargetState::Ready));
}
fn state_of(checks: &[Check], id: &str) -> CheckState {
    state(checks, id).unwrap()
}

#[test]
fn keywords_are_matched_in_any_case_and_a_bom_is_ignored() {
    let report = NicReport::parse(
        "\u{feff}{\"adapters\":[{\"guid\":\"{875511c7-0ef5-45f7-876c-3be50726ac42}\",\
             \"keywords\":{\"*wakeonmagicpacket\":\" 1 \"},\"wake_enable\":true}]}",
    )
    .unwrap();
    let nic = report.get(&this_pc()[0]).unwrap();
    assert_eq!(nic.keyword(MAGIC_PACKET), Some("1"));
    assert!(nic.armed());
    // An empty keyword map comes as `{}` or `null`.
    let report = NicReport::parse("{\"adapters\":[{\"keywords\":null}]}").unwrap();
    assert!(report.adapters[0].keywords.is_empty());
}

#[test]
fn system_facts_map_to_the_api() {
    let system = system(Some(&S3));
    assert_eq!(system.sleep, Sleep::S3);
    // Hiberboot without a hibernation file has no effect.
    assert_eq!(system.fast_startup, FastStartup::Inactive);
    assert_eq!(system.shutdown_wake, ShutdownWake::FirmwareUnknown);
    let hibernate = SystemFacts {
        hibernate: true,
        ..S3
    };
    assert_eq!(
        super::system(Some(&hibernate)).fast_startup,
        FastStartup::Active
    );
    let off = SystemFacts {
        hiberboot: Some(false),
        ..hibernate
    };
    assert_eq!(super::system(Some(&off)).fast_startup, FastStartup::Off);
    let standby = SystemFacts {
        s3: false,
        modern_standby: true,
        ..S3
    };
    assert_eq!(super::system(Some(&standby)).sleep, Sleep::None);
    assert_eq!(super::system(None).sleep, Sleep::Unknown);
}

#[test]
fn the_fix_plan_names_each_keyword_and_wmi_change() {
    let adapters = this_pc();
    // This PC: nothing but "magic packet only", which is off.
    let plan = FixPlan::new(&adapters, &report("de"), &S3, true);
    assert_eq!(
        plan,
        FixPlan {
            adapters: vec![AdapterFix {
                guid: "{875511C7-0EF5-45F7-876C-3BE50726AC42}".into(),
                pnp: "PCI\\VEN_10EC&DEV_8125&SUBSYS_7C911462&REV_05\\0100000000000000000".into(),
                name: "Ethernet".into(),
                keywords: BTreeMap::new(),
                wake_enable: false,
                magic_packet_only: true,
            }],
            // Fast Startup isn't active without hibernate.
            fast_startup_off: false,
            skipped: Vec::new(),
        }
    );
    assert_eq!(
        plan.changes(),
        ["Ethernet: only allow a magic packet to wake the computer"]
    );
    // Everything off on a Modern Standby PC with Fast Startup active.
    let mut report = report("en");
    let realtek = report
        .adapters
        .iter_mut()
        .find(|a| a.name == "Ethernet")
        .unwrap();
    realtek.wake_enable = Some(false);
    realtek
        .keywords
        .insert(MAGIC_PACKET.to_ascii_uppercase(), "0".into());
    realtek
        .keywords
        .insert(MODERN_STANDBY_MAGIC_PACKET.to_ascii_uppercase(), "0".into());
    let standby = SystemFacts {
        s3: false,
        modern_standby: true,
        hibernate: true,
        hiberboot: Some(true),
    };
    let plan = FixPlan::new(&adapters, &report, &standby, true);
    assert_eq!(
        plan.adapters.len(),
        1,
        "the WLAN isn't touched on a PC with Ethernet"
    );
    let fix = &plan.adapters[0];
    assert!(fix.wake_enable && fix.magic_packet_only);
    assert_eq!(
        fix.keywords,
        BTreeMap::from([
            (MAGIC_PACKET.into(), "1".into()),
            (MODERN_STANDBY_MAGIC_PACKET.into(), "1".into()),
        ])
    );
    assert!(plan.fast_startup_off);
    assert_eq!(
        plan.changes(),
        [
            "Ethernet: allow this device to wake the computer",
            "Ethernet: wake on magic packet",
            "Ethernet: wake from Modern Standby",
            "Ethernet: only allow a magic packet to wake the computer",
            "Ethernet: restarts, so the network drops for a few seconds",
            "Turn off Fast Startup",
        ]
    );
    // The plan travels to `--apply-wake-settings` as JSON with the
    // registry's keywords.
    let json = serde_json::to_string(&plan).unwrap();
    assert!(json.contains("\"*WakeOnMagicPacket\":\"1\""), "{json}");
    // Without consent to Fast Startup, or when all is set, nothing changes.
    assert!(!FixPlan::new(&adapters, &report, &standby, false).fast_startup_off);
    let mut set = super::NicReport::parse(include_str!("../fixtures/nics_read_de.json")).unwrap();
    set.adapters
        .iter_mut()
        .for_each(|a| a.magic_packet_only = Some(true));
    assert!(FixPlan::new(&adapters, &set, &S3, true).is_empty());
}

#[test]
fn a_wifi_only_pc_fixes_its_wifi() {
    let laptop: Vec<Adapter> =
        serde_json::from_str(include_str!("../fixtures/adapters_wifi_laptop.json")).unwrap();
    let report = NicReport::parse(
        r#"{"adapters":[{"guid":"{00000000-0000-4000-8000-000000000112}","pnp":"PCI\\VEN_14C3","name":"Wi-Fi","wake_enable":false,"magic_packet_only":null,"keywords":{"*WakeOnMagicPacket":"1"}}]}"#,
    )
    .unwrap();
    let plan = FixPlan::new(&laptop, &report, &S3, false);
    assert_eq!(plan.adapters.len(), 1);
    assert!(plan.adapters[0].wake_enable && plan.adapters[0].keywords.is_empty());
}

#[test]
fn a_plan_may_only_turn_wake_on() {
    let modern = SystemFacts {
        modern_standby: true,
        ..S3
    };
    let mut report = report("en");
    report.adapters.iter_mut().for_each(|nic| {
        nic.keywords
            .values_mut()
            .for_each(|value| *value = "0".into())
    });
    let plan = FixPlan::new(&this_pc(), &report, &modern, false);
    assert!(!plan.adapters[0].keywords.is_empty(), "{plan:?}");
    assert_eq!(plan.refusal(), None);
    let mut forged = plan.clone();
    forged.adapters[0]
        .keywords
        .insert("*SpeedDuplex".into(), "1".into());
    assert!(forged.refusal().unwrap().contains("*SpeedDuplex"));
    let mut forged = plan;
    forged.adapters[0]
        .keywords
        .insert(MAGIC_PACKET.into(), "0".into());
    assert!(forged.refusal().unwrap().contains("\"0\""));
}

#[test]
fn an_adapter_without_a_pnp_id_is_skipped_with_the_reason() {
    let adapters = this_pc();
    let mut report = report("en");
    let realtek = report
        .adapters
        .iter_mut()
        .find(|a| a.name == "Ethernet")
        .unwrap();
    // Its WMI instances can't be told apart from every other adapter's.
    realtek.pnp = " ".into();
    realtek.wake_enable = Some(false);
    let plan = FixPlan::new(&adapters, &report, &S3, false);
    assert!(plan.adapters.is_empty() && plan.is_empty());
    assert_eq!(
        plan.skipped,
        ["Ethernet: Windows reports no device id for it, so its wake settings can't be changed"]
    );
    assert!(plan.changes().is_empty());
    // Nothing to change on it: nothing to report either.
    let mut set = report.clone();
    set.adapters.iter_mut().for_each(|a| {
        a.wake_enable = Some(true);
        a.magic_packet_only = Some(true);
    });
    assert!(FixPlan::new(&adapters, &set, &S3, false).skipped.is_empty());
}
