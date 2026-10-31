use super::super::monitor;
use super::*;
#[test]
fn golden_restore_requires_all_active_saved_displays_but_not_inactive_ones() {
    let document = serde_json::json!({"topology":[["a"],["b"]],"primary":"a",
        "modes":{"a":{"w":1920,"h":1080,"num":60,"den":1},"b":{"w":1920,"h":1080,"num":60,"den":1}}});
    let mut snapshot = Snapshot::decode(&document).unwrap();
    assert!(snapshot.displays_present(&[monitor("a", 1, false), monitor("b", 2, false)]));
    assert!(!snapshot.displays_present(&[monitor("a", 1, false)]));
    assert!(!snapshot.displays_present(&[]));
    snapshot.nodes[1].active = false;
    assert!(snapshot.displays_present(&[monitor("a", 1, false)]));
    snapshot.nodes[0].active = false;
    assert!(!snapshot.displays_present(&[monitor("a", 1, false)]));
}
#[test]
fn a_layout_with_no_connected_display_left_has_nothing_to_turn_on() {
    // A headless host: its only display was the virtual one, gone after a crash.
    let document = serde_json::json!({"topology":[["virtual"]],"primary":"virtual",
        "modes":{"virtual":{"w":1920,"h":1080,"num":60,"den":1}}});
    let snapshot = Snapshot::decode(&document).unwrap();
    assert!(snapshot.active_ids(&[], &[], &[]).is_empty());
    assert!(
        snapshot
            .active_ids(&[], &[monitor("physical", 1, false)], &[])
            .is_empty()
    );
    // While it lives, the saved display or an excluded active one is turned on.
    let virtual_display = monitor("virtual", 1, false);
    assert_eq!(
        snapshot.active_ids(&[], std::slice::from_ref(&virtual_display), &[]),
        ["virtual"]
    );
    assert_eq!(
        snapshot.active_ids(
            std::slice::from_ref(&virtual_display),
            std::slice::from_ref(&virtual_display),
            &["VIRTUAL".into()]
        ),
        ["virtual"]
    );
}
#[test]
fn only_a_displays_sole_stream_changes_its_mode_or_hdr() {
    let (current, requested) = ((3840, 2160, 60000), (1920, 1080, 120000));
    assert_eq!(plan(1, None, &current, &requested), Plan::Change);
    assert_eq!(plan(1, None, &current, &current), Plan::Keep);
    // The first stream needed no change: the second keeps its mode.
    assert_eq!(plan(2, None, &current, &requested), Plan::Keep);
    assert_eq!(plan(2, Some(&current), &current, &requested), Plan::Keep);
    assert_eq!(plan(2, None, &false, &true), Plan::Keep);
    assert_eq!(plan(1, None, &false, &true), Plan::Change);
}
#[test]
fn a_pending_original_comes_back_rather_than_being_cleared() -> Result<()> {
    // A stream turned the TV's HDR on and ended while it was in standby:
    // the journal kept the entry and the TV came back with HDR on.
    let directory = tempfile::tempdir()?;
    let journal = directory.path().join("display-recovery.json");
    std::fs::write(
        &journal,
        serde_json::to_vec(&serde_json::json!({
            "pid": 0,
            "started": 0,
            "entries": {"tv": {"output": r"\\.\DISPLAY1", "hdr": [false, true]}},
        }))?,
    )?;
    let pending = crate::display_recovery::pending_in(&journal, "tv")?;
    // The next stream wants HDR too, so it changes nothing.
    let mut settings = Settings {
        users: 1,
        ..Default::default()
    };
    let unchanged = |_: &Monitor, _| -> Result<()> { panic!("HDR is already on") };
    lease_hdr(
        &mut settings,
        &monitor("tv", 1, true),
        Some(true),
        pending.hdr,
        |_, _| panic!("nothing to journal"),
        unchanged,
    )?;
    let no_mode = |_, _| -> Result<()> { panic!("no mode recorded") };
    let no_turn = |_, _| -> Result<()> { panic!("no rotation recorded") };
    let no_scale = |_, _| -> Result<()> { panic!("no scale recorded") };
    // Away again at its end: the entry stays for recovery.
    assert!(!restore_settings(
        &settings,
        "tv",
        "TV",
        false,
        Some(&[]),
        unchanged,
        no_mode,
        no_turn,
        no_scale
    ));
    assert_eq!(
        crate::display_recovery::pending_in(&journal, "tv")?,
        pending
    );
    // Present: the original comes back, then the entry is released.
    let mut restored = None;
    assert!(restore_settings(
        &settings,
        "tv",
        "TV",
        false,
        Some(&[monitor("tv", 2, true)]),
        |_, enabled| {
            restored = Some(enabled);
            Ok(())
        },
        no_mode,
        no_turn,
        no_scale
    ));
    assert_eq!(restored, Some(false));
    crate::display_recovery::release_in(&journal, "tv")?;
    assert_eq!(
        crate::display_recovery::pending_in(&journal, "tv")?,
        Default::default()
    );
    // Without a pending entry a stream that changes nothing records nothing.
    let mut settings = Settings {
        users: 1,
        ..Default::default()
    };
    lease_hdr(
        &mut settings,
        &monitor("tv", 1, true),
        Some(true),
        None,
        |_, _| panic!("nothing to journal"),
        unchanged,
    )?;
    assert!(settings.color.is_none());
    Ok(())
}
#[test]
fn a_stream_that_changes_a_setting_restores_the_pending_original() -> Result<()> {
    // The TV was left in HDR; this stream wants SDR.
    let mut settings = Settings {
        users: 1,
        ..Default::default()
    };
    let mut journaled = None;
    let mut set = None;
    lease_hdr(
        &mut settings,
        &monitor("tv", 1, true),
        Some(false),
        Some(false),
        |before, applied| {
            journaled = Some((before, applied));
            Ok(())
        },
        |_, enabled| {
            set = Some(enabled);
            Ok(())
        },
    )?;
    assert_eq!((journaled, set), (Some((true, false)), Some(false)));
    let (_, previous, applied) = settings.color.unwrap();
    assert_eq!((previous, applied), (false, false));
    // Left as it is: the original wins, unless the display has it.
    assert_eq!(adopt(Some(false), true), Some((false, true)));
    assert_eq!(adopt(Some(false), false), None);
    assert_eq!(adopt(None, true), None);
    Ok(())
}
#[test]
fn a_displays_sole_stream_turns_and_scales_it_and_teardown_turns_it_back_first() -> Result<()> {
    let mut settings = Settings {
        users: 1,
        ..Default::default()
    };
    let mut journaled = Vec::new();
    let mut set = Vec::new();
    // Upright (1) to 90° (2), recorded before it turns.
    assert!(lease_setting(
        settings.users,
        &mut settings.rotation,
        1,
        2,
        |before, applied| {
            journaled.push((before, applied));
            Ok(())
        },
        |rotation| {
            set.push(rotation);
            Ok(())
        },
    )?);
    assert!(lease_setting(
        settings.users,
        &mut settings.scale,
        100,
        150,
        |before, applied| {
            journaled.push((before, applied));
            Ok(())
        },
        |_| Ok(()),
    )?);
    assert_eq!((journaled, set), (vec![(1, 2), (100, 150)], vec![2]));
    assert_eq!(
        (settings.rotation, settings.scale),
        (Some((1, 2)), Some((100, 150)))
    );
    // Already at the angle, or a second stream: nothing changes.
    let mut kept = None;
    let nothing = |_, _| -> Result<()> { panic!("nothing to journal") };
    assert!(!lease_setting(1, &mut kept, 2, 2, nothing, |_| panic!(
        "not set"
    ))?);
    assert!(!lease_setting(2, &mut kept, 1, 2, nothing, |_| panic!(
        "not set"
    ))?);
    assert_eq!(kept, None);
    // An original an earlier stream left pending wins over the current value.
    let mut adopted = adopt(Some(1), 4);
    assert!(lease_setting(
        1,
        &mut adopted,
        4,
        2,
        |_, _| Ok(()),
        |_| Ok(())
    )?);
    assert_eq!(adopted, Some((1, 2)));
    // A refused turn keeps the record: teardown finds the display unturned
    // and leaves it, and recovery has the original until then.
    let mut refused = None;
    assert!(lease_setting(1, &mut refused, 1, 2, |_, _| Ok(()), |_| bail!("busy")).is_err());
    assert_eq!(refused, Some((1, 2)));
    settings.mode = Some(((2560, 1440, 60000), (2400, 1080, 120000)));
    let order = std::cell::RefCell::new(Vec::new());
    let present = [monitor("tv", 1, false)];
    assert!(restore_settings(
        &settings,
        "tv",
        "TV",
        false,
        Some(&present),
        |_, _| panic!("HDR untouched"),
        |previous, _| {
            order
                .borrow_mut()
                .push(format!("mode {}x{}", previous.0, previous.1));
            Ok(())
        },
        |previous, _| {
            order.borrow_mut().push(format!("rotation {previous}"));
            Ok(())
        },
        |previous, _| {
            order.borrow_mut().push(format!("scale {previous}"));
            Ok(())
        },
    ));
    // The mode was recorded in the shape the display had before it turned.
    assert_eq!(order.take(), ["rotation 1", "mode 2560x1440", "scale 100"]);
    // Gone at the end: nothing is turned and the journal entry stays.
    assert!(!restore_settings(
        &settings,
        "tv",
        "TV",
        false,
        Some(&[]),
        |_, _| panic!("HDR untouched"),
        |_, _| Ok(()),
        |_, _| panic!("a display that is gone is not turned"),
        |_, _| panic!("a display that is gone is not scaled"),
    ));
    // A failed turn back keeps the entry for recovery too.
    assert!(!restore_settings(
        &settings,
        "tv",
        "TV",
        false,
        Some(&present),
        |_, _| panic!("HDR untouched"),
        |_, _| Ok(()),
        |_, _| bail!("busy"),
        |_, _| Ok(()),
    ));
    Ok(())
}
#[test]
fn a_physical_stream_takes_the_named_display_or_the_primary_one() {
    let mut first = monitor("a", 1, false);
    first.monitor_device_path = r"\\?\DISPLAY#A".into();
    let mut primary = monitor("b", 2, false);
    primary.primary = true;
    primary.display_name = r"\\.\DISPLAY2".into();
    primary.monitor_device_path = r"\\?\DISPLAY#B".into();
    let choices = [first, primary];
    assert_eq!(
        choose(&choices, "a").map(|m| m.device_id.as_str()),
        Some("a")
    );
    assert_eq!(
        choose(&choices, "").map(|m| m.device_id.as_str()),
        Some("b")
    );
    assert!(choose(&choices, "missing").is_none());
    assert_eq!(
        choose(&choices[..1], "").map(|m| m.device_id.as_str()),
        Some("a")
    );
}
#[test]
fn previous_golden_snapshot_import_preserves_fractional_rates_clones_hdr_and_origins() {
    let document = serde_json::json!({
        "topology":[["a","b"]],"primary":"a",
        "modes":{"a":{"w":1920,"h":1080,"num":60000,"den":1001},"b":{"w":1920,"h":1080,"num":60000,"den":1001}},
        "hdr":{"a":"on","b":"off"}, "origins":{"a":{"x":-1920,"y":0},"b":{"x":-1920,"y":0}},
        "layouts":{"a":{"rotation":90},"b":{"rotation":0}}
    });
    let snapshot = Snapshot::decode(&document).unwrap();
    assert!((snapshot.nodes[0].mode.refresh_hz - 59.94005994).abs() < 1e-8);
    assert_eq!(snapshot.nodes[0].desired_position.x, -1920);
    assert!(snapshot.nodes[0].primary);
    assert_eq!(
        snapshot.clone_groups,
        [vec!["a".to_owned(), "b".to_owned()]]
    );
    assert!(snapshot.hdr["a"]);
    assert!(!snapshot.hdr["b"]);
    assert_eq!(snapshot.rotation["a"], 2);
    let mut malformed = document.clone();
    malformed["modes"]["a"]["den"] = 0.into();
    assert!(Snapshot::decode(&malformed).is_err());
    assert!(Snapshot::decode(&serde_json::json!({"version":2,"nodes":[],"hdr":{}})).is_err());
}
